use super::*;
use crate::model::JobHistoryView;
use crate::repository_job_search::{
    JobSearch, JobSearchRequest, JobSearchSort, JobSearchValuesQuery,
};

async fn search(db: &PgReliabilityTestDb, q: &str) -> Vec<JobHistoryView> {
    db.repo
        .search_jobs(
            &JobSearch::parse(&JobSearchRequest {
                q: q.into(),
                limit: Some(100),
                ..Default::default()
            })
            .unwrap(),
        )
        .await
        .unwrap()
        .rows
}

async fn history_fixture(db: &PgReliabilityTestDb) -> Vec<Uuid> {
    sqlx::query(
        "INSERT INTO jobs(id,command_type,status,target_count,payload_hash,request_fingerprint,created_at,completed_at)
         SELECT md5('history-automation-' || n)::uuid,'network_routing_status','completed',0,'fixture','fixture',now()-n*interval '1 second',now()
         FROM generate_series(1,1205) n"
    ).execute(&db.pool).await.unwrap();
    let mut ids = Vec::new();
    for (index, status) in [
        "failed",
        "completed",
        "completed",
        "queued",
        "running",
        "completed",
        "completed",
    ]
    .into_iter()
    .enumerate()
    {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO jobs(id,command_type,status,target_count,payload_hash,request_fingerprint,created_at,completed_at,alert_terminal_at)
             VALUES ($1,'shell_argv',$2,2,'history-special','fixture',
               '2026-01-02T00:00:00Z'::timestamptz + $3*interval '1 microsecond',
               CASE WHEN $2 IN ('queued','running') THEN NULL ELSE '2026-01-02T00:01:00Z'::timestamptz + $3*interval '2 microseconds' END,
               CASE WHEN $2='failed' THEN '2026-01-02T00:01:00Z'::timestamptz ELSE NULL END)"
        ).bind(id).bind(status).bind(index as i32).execute(&db.pool).await.unwrap();
        // Deliberately no clients row for gone-vps: historical identity survives.
        for (client, status, exit) in [("gone-vps", "completed", 0), ("another-vps", "failed", 2)] {
            sqlx::query("INSERT INTO job_targets(job_id,client_id,status,exit_code,message,started_at,completed_at) VALUES($1,$2,$3,$4,'fixture timeout','2026-01-02T00:00:20Z','2026-01-02T00:01:00Z')")
                .bind(id).bind(client).bind(status).bind(exit).execute(&db.pool).await.unwrap();
        }
        ids.push(id);
    }
    ids
}

#[tokio::test]
async fn postgres_job_search_finds_older_records_and_correlates_target_evidence() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let ids = history_fixture(&db).await;
    let recent = db
        .repo
        .query_jobs(&ListQuery {
            limit: Some(1000),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(recent.len(), 1000);
    assert!(!recent.iter().any(|row| ids.contains(&row.id)));
    let rows = search(&db, r#"type = shell_argv && status = failed && target = gone-vps && created_at >= "2026-01-01T08:00:00+08:00" && created_at < 2026-02-01 && duration >= 1m && timeout = 30s && target_count = 2 && privileged = false && source = automation"#).await;
    assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![ids[0]]);
    assert!(
        search(&db, r#"target_result = "id = gone-vps && status = failed""#)
            .await
            .is_empty()
    );
    assert_eq!(search(&db,r#"target_result = "id = another-vps && status = failed && exit_code != 0 && duration = 40s""#).await.len(),7);
    assert_eq!(
        search(
            &db,
            r#"type = shell_argv && target_result != "id = gone-vps && status = failed""#
        )
        .await
        .len(),
        7
    );
    assert!(search(&db, "type = shell_argv && target != gone-vps")
        .await
        .is_empty());
    assert_eq!(
        search(&db, "type = shell_argv && target not in [nobody, missing]")
            .await
            .len(),
        7
    );
    assert_eq!(
        search(&db, "type = shell_argv && duration = null")
            .await
            .len(),
        2
    );
    assert_eq!(
        search(&db, "type = shell_argv && !(duration >= 30s)")
            .await
            .len(),
        0,
        "unknown duration must not turn into a known comparison"
    );
    assert_eq!(
        search(&db, "type = shell_argv && completed_at != null")
            .await
            .len(),
        5
    );
    assert_eq!(
        search(
            &db,
            "(status = failed || status = queued) && type in [shell_argv]"
        )
        .await
        .len(),
        2
    );
    assert_eq!(
        search(
            &db,
            "type in [/^shell_argv$/] && payload_hash = history-special"
        )
        .await
        .len(),
        7
    );
    assert_eq!(search(&db, "history-special && gone-vps").await.len(), 7);
    assert!(
        search(&db, "type = shellXargv").await.is_empty(),
        "underscore is literal, not a LIKE wildcard"
    );
    assert!(search(&db, r#"type = "' OR true --""#).await.is_empty());
    let hints = db
        .repo
        .job_search_values(&JobSearchValuesQuery {
            field: "target".into(),
            prefix: "gone".into(),
        })
        .await
        .unwrap();
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0].value, "gone-vps");
    assert_eq!(
        search(&db, "age > 1d && created_at < now-1d").await.len(),
        7
    );
    // Compile and execute every advertised scalar against the actual schema,
    // including less common dispatch/resource fields and nullable evidence.
    for field in crate::repository_job_search::search_fields() {
        if matches!(field.kind, "target" | "expression" | "lineage") {
            continue;
        }
        let clause = format!("{} = null", field.name);
        let query = if field.scope == "target" {
            format!(
                "target_result = {}",
                serde_json::to_string(&clause).unwrap()
            )
        } else {
            clause
        };
        search(&db, &query).await;
    }
    let lineage = Uuid::new_v4();
    sqlx::query("UPDATE jobs SET schedule_lineage=ARRAY[$1]::uuid[] WHERE id=$2")
        .bind(lineage)
        .bind(ids[0])
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        search(&db, &format!("schedule_lineage = {lineage}"))
            .await
            .len(),
        1
    );
    assert_eq!(
        search(
            &db,
            &format!("type = shell_argv && schedule_lineage != {lineage}")
        )
        .await
        .len(),
        6
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_job_search_pages_every_sort_without_duplicates_or_offset_horizon() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let ids = history_fixture(&db).await;
    for field in [
        "operation",
        "targets",
        "result",
        "duration",
        "startedBy",
        "age",
        "completed_at",
    ] {
        for desc in [true, false] {
            let mut request = JobSearchRequest {
                q: "type = shell_argv".into(),
                limit: Some(2),
                sort: vec![JobSearchSort {
                    id: field.into(),
                    desc,
                }],
                ..Default::default()
            };
            let expected = db
                .repo
                .search_jobs(
                    &JobSearch::parse(&JobSearchRequest {
                        limit: Some(100),
                        ..request.clone()
                    })
                    .unwrap(),
                )
                .await
                .unwrap()
                .rows;
            let mut actual = Vec::new();
            let mut as_of = None;
            loop {
                let page = db
                    .repo
                    .search_jobs(&JobSearch::parse(&request).unwrap())
                    .await
                    .unwrap();
                if let Some(now) = &as_of {
                    assert_eq!(&page.as_of, now);
                } else {
                    as_of = Some(page.as_of.clone());
                }
                actual.extend(page.rows.iter().map(|r| r.id));
                assert!(
                    actual.len() <= ids.len(),
                    "cursor failed to advance for {field}/{desc}"
                );
                request.cursor = page.next_cursor;
                if request.cursor.is_none() {
                    break;
                }
            }
            assert_eq!(
                actual,
                expected.iter().map(|r| r.id).collect::<Vec<_>>(),
                "{field}/{desc}"
            );
        }
    }
    let mut request = JobSearchRequest {
        q: "type = shell_argv".into(),
        limit: Some(2),
        sort: vec![
            JobSearchSort {
                id: "result".into(),
                desc: false,
            },
            JobSearchSort {
                id: "age".into(),
                desc: true,
            },
        ],
        ..Default::default()
    };
    let first = db
        .repo
        .search_jobs(&JobSearch::parse(&request).unwrap())
        .await
        .unwrap();
    request.cursor = first.next_cursor;
    assert!(JobSearch::parse(&JobSearchRequest {
        q: "status = failed".into(),
        ..request.clone()
    })
    .is_err());
    assert!(JobSearch::parse(&JobSearchRequest {
        limit: Some(50),
        ..request.clone()
    })
    .is_err());
    let newcomer = Uuid::new_v4();
    sqlx::query("INSERT INTO jobs(id,command_type,status,target_count,payload_hash,request_fingerprint) VALUES($1,'shell_argv','queued',0,'new','new')")
        .bind(newcomer).execute(&db.pool).await.unwrap();
    let mut seen = first.rows.iter().map(|r| r.id).collect::<Vec<_>>();
    while request.cursor.is_some() {
        let page = db
            .repo
            .search_jobs(&JobSearch::parse(&request).unwrap())
            .await
            .unwrap();
        seen.extend(page.rows.iter().map(|r| r.id));
        request.cursor = page.next_cursor;
    }
    assert_eq!(seen.len(), ids.len());
    assert!(!seen.contains(&newcomer));
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), seen.len());
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_job_search_routes_validate_expressions_and_preserve_access_boundaries() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let (viewer, headers) = postgres_operator_session(&db.repo, "history-search-admin").await;
    let router = crate::routes::build_router(postgres_app_state(&db));
    for q in [
        "type =",
        "nonsense = value",
        "status = faield",
        "duration > banana",
        "created_at > tomorrow",
        "target_count > 1.5",
        "privileged = perhaps",
        r#"target_result = "not_a_field = value""#,
        r"type in [/\p{Greek}/]",
    ] {
        let mut request = Request::builder().method("POST").uri("/api/v1/jobs/search");
        *request.headers_mut().unwrap() = headers.clone();
        let response = router
            .clone()
            .oneshot(
                request
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(json!({"q":q}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{q}");
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["error"], "invalid_job_search", "{body}");
    }
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/jobs/search")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    // The existing operator and schedule list permissions also govern optional
    // name searches/hints; fleet history access alone must not reveal them.
    let viewer_headers = headers;
    sqlx::query("UPDATE operators SET role='viewer',scopes='[\"fleet:read\"]'::jsonb WHERE id=$1")
        .bind(viewer.operator.id)
        .execute(&db.pool)
        .await
        .unwrap();
    for q in ["actor = admin", "schedule = secret"] {
        let error = crate::routes_job_search::search_jobs(
            State(postgres_app_state(&db)),
            viewer_headers.clone(),
            Json(JobSearchRequest {
                q: q.into(),
                ..Default::default()
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, StatusCode::FORBIDDEN);
    }
    let Json(fields) = crate::routes_job_search::job_search_fields(
        State(postgres_app_state(&db)),
        viewer_headers.clone(),
    )
    .await
    .unwrap();
    assert!(!fields
        .iter()
        .any(|f| f.name == "actor" || f.name == "schedule"));
    let type_hints = &fields
        .iter()
        .find(|field| field.name == "type")
        .unwrap()
        .values;
    assert!(type_hints.iter().any(|value| value == "shell_argv"));
    assert!(
        !type_hints.iter().any(|value| value == "shell"),
        "hints must use stored job types, not operation names"
    );
    let Json(page) = crate::routes_job_search::search_jobs(
        State(postgres_app_state(&db)),
        viewer_headers,
        Json(JobSearchRequest::default()),
    )
    .await
    .unwrap();
    assert!(page.rows.is_empty());
    db.cleanup().await;
}

#[tokio::test]
#[ignore = "interactive browser fixture; requires VPSMAN_JOB_SEARCH_BROWSER_DIR inside the workspace"]
async fn postgres_job_search_browser_fixture() {
    use std::os::unix::fs::OpenOptionsExt;
    let directory =
        std::path::PathBuf::from(std::env::var("VPSMAN_JOB_SEARCH_BROWSER_DIR").unwrap());
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    assert!(directory.is_absolute() && directory.starts_with(workspace));
    std::fs::create_dir_all(&directory).unwrap();
    assert!(!directory.join("stop").exists());
    let db = PgReliabilityTestDb::maybe_new()
        .await
        .expect("real test PostgreSQL required");
    let ids = history_fixture(&db).await;
    let (_, headers) = postgres_operator_session(&db.repo, "history-browser").await;
    if std::env::var("VPSMAN_JOB_SEARCH_SCALE").as_deref() == Ok("1") {
        sqlx::query("INSERT INTO jobs(id,command_type,status,target_count,payload_hash,request_fingerprint,created_at,completed_at)
            SELECT md5('history-scale-' || n)::uuid,'network_routing_status','completed',1,'scale','scale',now()-n*interval '1 second',now()
            FROM generate_series(1,100000) n").execute(&db.pool).await.unwrap();
        sqlx::query("INSERT INTO job_targets(job_id,client_id,status,started_at,completed_at)
            SELECT md5('history-scale-' || n)::uuid,'bench-vps-' || (n%100),'completed',now()-interval '1 second',now()
            FROM generate_series(1,100000) n").execute(&db.pool).await.unwrap();
        insert_client(&db.pool, "bench-vps-7", None).await;
        sqlx::query("UPDATE clients SET display_name='Benchmark target' WHERE id='bench-vps-7'")
            .execute(&db.pool)
            .await
            .unwrap();
        sqlx::query("ANALYZE jobs").execute(&db.pool).await.unwrap();
        sqlx::query("ANALYZE job_targets")
            .execute(&db.pool)
            .await
            .unwrap();
        let mut plans = Vec::new();
        for q in [
            "",
            "type = shell_argv",
            "status = failed",
            "target = gone-vps",
            "target = bench-vps-7",
            "target = \"Benchmark target\"",
            "created_at >= now-1h && type = network_routing_status",
        ] {
            let query = JobSearch::parse(&JobSearchRequest {
                q: q.into(),
                ..Default::default()
            })
            .unwrap();
            plans.push(json!({"q":q,"plans":query.explain(&db.pool).await.unwrap()}));
        }
        std::fs::write(
            directory.join("plans.json"),
            serde_json::to_vec_pretty(&plans).unwrap(),
        )
        .unwrap();
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:41018")
        .await
        .unwrap();
    let app = crate::routes::build_router(postgres_app_state(&db));
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(directory.join("browser.json"))
        .unwrap();
    serde_json::to_writer(
        file,
        &json!({
            "authorization":headers.get(AUTHORIZATION).unwrap().to_str().unwrap(),
            "job_id":ids[0],
            "db_name":db.db_name,
        }),
    )
    .unwrap();
    eprintln!("Job history browser fixture ready on 127.0.0.1:41018");
    while !directory.join("stop").exists() {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    task.abort();
    let _ = task.await;
    db.cleanup().await;
}
