use super::*;
use std::io::Read;

async fn seed_job(db: &PgReliabilityTestDb) -> Uuid {
    let job_id = Uuid::new_v4();
    sqlx::query("INSERT INTO jobs (id,command_type,status,target_count,payload_hash,request_fingerprint) VALUES ($1,'shell_argv','completed',2,'download-fixture',$2)")
        .bind(job_id).bind(format!("download-{job_id}"))
        .execute(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO job_targets (job_id,client_id,status,exit_code) VALUES ($1,'v-21','completed',0),($1,'retained-target','queued',NULL)")
        .bind(job_id).execute(&db.pool).await.unwrap();
    job_id
}

async fn output(db: &PgReliabilityTestDb, job: Uuid, seq: i32, stream: &str, data: &[u8]) {
    sqlx::query("INSERT INTO job_outputs (job_id,client_id,seq,stream,data,storage) VALUES ($1,'v-21',$2,$3,$4,'inline')")
        .bind(job).bind(seq).bind(stream).bind(data).execute(&db.pool).await.unwrap();
}

async fn get(state: AppState, headers: &HeaderMap, path: &str) -> axum::response::Response {
    let mut request = Request::builder().uri(path).body(Body::empty()).unwrap();
    *request.headers_mut() = headers.clone();
    crate::routes::build_router(state)
        .oneshot(request)
        .await
        .unwrap()
}

async fn archive(response: axum::response::Response) -> BTreeMap<String, Vec<u8>> {
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], "application/x-tar");
    let expected_size: usize = response.headers()["content-length"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.len(), expected_size);
    let mut result = BTreeMap::new();
    for entry in tar::Archive::new(body.as_ref()).entries().unwrap() {
        let mut entry = entry.unwrap();
        let name = entry.path().unwrap().to_str().unwrap().to_string();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        assert!(
            result.insert(name, bytes).is_none(),
            "archive paths must not collide"
        );
    }
    result
}

#[tokio::test]
async fn postgres_job_downloads_include_current_names_all_pages_and_separate_file_payloads() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let (_, headers) = postgres_operator_session(&db.repo, "job-download-admin").await;
    insert_client(&db.pool, "v-21", None).await;
    let job = seed_job(&db).await;
    let alias = "Renamed edge / 香港";
    sqlx::query("UPDATE clients SET display_name=$1 WHERE id='v-21'")
        .bind(alias)
        .execute(&db.pool)
        .await
        .unwrap();
    let payload = b"\0file bytes\xff";
    output(&db, job, 2, "stdout", &payload[5..]).await;
    output(&db, job, 0, "stdout", &payload[..5]).await;
    output(&db, job, 1, "stderr", b"diagnostic\n").await;
    let file_status = json!({"type":"file_download","status":"completed","filename":"status.json","content_type":null,"size_bytes":payload.len(),"sha256_hex":payload_hash(payload)});
    output(
        &db,
        job,
        3,
        "status",
        &serde_json::to_vec(&file_status).unwrap(),
    )
    .await;
    // More than one API page; every structured status chunk must be included.
    sqlx::query("INSERT INTO job_outputs (job_id,client_id,seq,stream,data,storage) SELECT $1,'v-21',n,'status',convert_to(n::text || E'\\n','UTF8'),'inline' FROM generate_series(4,1005) n")
        .bind(job).execute(&db.pool).await.unwrap();
    output(&db, job, 1006, "pty", b"\x1b[31mterminal\x1b[0m").await;
    sqlx::query("INSERT INTO job_outputs (job_id,client_id,seq,stream,data,storage) VALUES ($1,'retained-target',0,'stdout',$2,'inline')")
        .bind(job).bind(b"other target\n".as_slice()).execute(&db.pool).await.unwrap();
    let state = postgres_app_state(&db);
    let all = archive(
        get(
            state.clone(),
            &headers,
            &format!("/api/v1/jobs/{job}/download"),
        )
        .await,
    )
    .await;
    let targets: Value = serde_json::from_slice(&all["targets.json"]).unwrap();
    assert_eq!(targets.as_array().unwrap().len(), 2);
    assert_eq!(targets[0]["client_id"], "retained-target");
    assert!(targets[0]["display_name"].is_null());
    assert_eq!(targets[1]["display_name"], alias);
    assert_eq!(targets[1]["client_id"], "v-21");
    assert_eq!(
        serde_json::from_slice::<Value>(&all["v-21_status.json"]).unwrap(),
        targets[1]
    );
    assert_eq!(all["outputs/v-21/stdout.bin"], payload);
    assert_eq!(all["outputs/v-21/stderr.bin"], b"diagnostic\n");
    assert_eq!(all["outputs/retained-target/stdout.bin"], b"other target\n");
    assert_eq!(all["outputs/v-21/pty.bin"], b"\x1b[31mterminal\x1b[0m");
    let mut status_bytes = serde_json::to_vec(&file_status).unwrap();
    for n in 4..=1005 {
        status_bytes.extend_from_slice(format!("{n}\n").as_bytes());
    }
    assert_eq!(all["outputs/v-21/status.bin"], status_bytes);
    assert_eq!(all["files/v-21/status.json"], payload);
    assert_eq!(
        serde_json::from_slice::<Value>(&all["files/v-21_status.json"]).unwrap(),
        file_status
    );
    assert_eq!(all.len(), 10);

    let statuses = archive(
        get(
            state.clone(),
            &headers,
            &format!("/api/v1/jobs/{job}/targets/download"),
        )
        .await,
    )
    .await;
    assert_eq!(statuses.len(), 3);
    assert_eq!(statuses["targets.json"], all["targets.json"]);
    let outputs = archive(
        get(
            state.clone(),
            &headers,
            &format!("/api/v1/jobs/{job}/outputs/archive"),
        )
        .await,
    )
    .await;
    assert_eq!(
        outputs.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![
            "retained-target/stdout.bin",
            "v-21/stderr.bin",
            "v-21/stdout.bin"
        ]
    );
    assert_eq!(outputs["v-21/stdout.bin"], payload);
    let files = archive(
        get(
            state,
            &headers,
            &format!("/api/v1/jobs/{job}/outputs/download-bundle"),
        )
        .await,
    )
    .await;
    assert_eq!(files.len(), 2);
    assert_eq!(files["v-21/status.json"], payload);
    let exact = db
        .repo
        .list_exact_job_target_statuses(&[(job, "v-21".into())])
        .await
        .unwrap();
    assert_eq!(exact[0].display_name.as_deref(), Some(alias));
    let dashboard = db
        .repo
        .list_dashboard_job_targets(&[job], &["v-21".into()])
        .await
        .unwrap();
    assert_eq!(dashboard[0].display_name.as_deref(), Some(alias));
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_job_downloads_preserve_permissions_and_targets_without_output() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let (operator, headers) = postgres_operator_session(&db.repo, "job-download-scope-admin").await;
    let job = seed_job(&db).await;
    let state = postgres_app_state(&db);
    let path = format!("/api/v1/jobs/{job}/download");
    assert_eq!(
        get(state.clone(), &HeaderMap::new(), &path).await.status(),
        StatusCode::UNAUTHORIZED
    );
    for scopes in [json!(["fleet:read"]), json!(["jobs:read"])] {
        sqlx::query("UPDATE operators SET scopes=$1 WHERE id=$2")
            .bind(SqlJson(scopes))
            .bind(operator.operator.id)
            .execute(&db.pool)
            .await
            .unwrap();
        assert_eq!(
            get(state.clone(), &headers, &path).await.status(),
            StatusCode::FORBIDDEN
        );
    }
    sqlx::query("UPDATE operators SET scopes=$1 WHERE id=$2")
        .bind(SqlJson(json!(["fleet:read", "jobs:read"])))
        .bind(operator.operator.id)
        .execute(&db.pool)
        .await
        .unwrap();
    let entries = archive(get(state.clone(), &headers, &path).await).await;
    assert_eq!(
        entries.len(),
        3,
        "empty or queued targets still have execution metadata"
    );
    sqlx::query("DELETE FROM job_targets WHERE job_id=$1")
        .bind(job)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET target_count=0 WHERE id=$1")
        .bind(job)
        .execute(&db.pool)
        .await
        .unwrap();
    let empty = archive(get(state.clone(), &headers, &path).await).await;
    assert_eq!(empty.len(), 1);
    assert_eq!(
        serde_json::from_slice::<Value>(&empty["targets.json"]).unwrap(),
        json!([])
    );
    assert_eq!(
        get(
            state,
            &headers,
            &format!("/api/v1/jobs/{}/download", Uuid::new_v4())
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_job_downloads_keep_integrity_and_aggregate_limits_including_named_files() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let (_, headers) = postgres_operator_session(&db.repo, "job-download-budget-admin").await;
    let job = seed_job(&db).await;
    let path = format!("/api/v1/jobs/{job}/download");
    // One copy fits the existing 1 MiB minimum; two copies do not.
    let payload = vec![b'x'; 600 * 1024];
    output(&db, job, 0, "stdout", &payload).await;
    let mut status = json!({"type":"file_download","status":"completed","filename":"stdout.bin","size_bytes":payload.len(),"sha256_hex":"0".repeat(64)});
    output(&db, job, 1, "status", &serde_json::to_vec(&status).unwrap()).await;
    let state = postgres_app_state(&db);
    assert_eq!(
        get(state.clone(), &headers, &path).await.status(),
        StatusCode::CONFLICT
    );
    status["sha256_hex"] = json!(payload_hash(&payload));
    let status_bytes = serde_json::to_vec(&status).unwrap();
    sqlx::query("UPDATE job_outputs SET data=$1 WHERE job_id=$2 AND seq=1")
        .bind(&status_bytes)
        .bind(job)
        .execute(&db.pool)
        .await
        .unwrap();
    let mut limited = state.clone();
    limited.artifact_max_bytes = 1024 * 1024;
    let response = get(limited, &headers, &path).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(error["error"], "job_archive_too_large");
    let entries = archive(get(state.clone(), &headers, &path).await).await;
    assert_eq!(entries["files/v-21/stdout.bin"], payload);
    assert_eq!(entries["outputs/v-21/stdout.bin"], payload);

    // An empty completed file needs no stdout chunk, and remains downloadable.
    sqlx::query("DELETE FROM job_outputs WHERE job_id=$1 AND seq=0")
        .bind(job)
        .execute(&db.pool)
        .await
        .unwrap();
    status["size_bytes"] = json!(0);
    status["sha256_hex"] = json!(payload_hash(b""));
    sqlx::query("UPDATE job_outputs SET data=$1 WHERE job_id=$2 AND seq=1")
        .bind(serde_json::to_vec(&status).unwrap())
        .bind(job)
        .execute(&db.pool)
        .await
        .unwrap();
    let entries = archive(get(state, &headers, &path).await).await;
    assert!(entries["files/v-21/stdout.bin"].is_empty());
    assert!(!entries.contains_key("outputs/v-21/stdout.bin"));
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_job_downloads_reject_deleted_payloads_and_excess_rows() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let (_, headers) = postgres_operator_session(&db.repo, "job-download-retention-admin").await;
    let job = seed_job(&db).await;
    let path = format!("/api/v1/jobs/{job}/download");
    output(&db, job, 0, "stdout", b"retained output").await;
    sqlx::query("UPDATE job_outputs SET storage='artifact_deleted' WHERE job_id=$1")
        .bind(job)
        .execute(&db.pool)
        .await
        .unwrap();
    let state = postgres_app_state(&db);
    assert_eq!(
        get(state.clone(), &headers, &path).await.status(),
        StatusCode::GONE
    );
    sqlx::query("UPDATE job_outputs SET storage='inline' WHERE job_id=$1")
        .bind(job)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO job_outputs (job_id,client_id,seq,stream,data,storage) SELECT $1,'v-21',n,'status',''::bytea,'inline' FROM generate_series(1,10000) n")
        .bind(job).execute(&db.pool).await.unwrap();
    let response = get(state.clone(), &headers, &path).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(error["error"], "job_archive_output_limit_exceeded");
    sqlx::query("DELETE FROM job_outputs WHERE job_id=$1 AND seq=10000")
        .bind(job)
        .execute(&db.pool)
        .await
        .unwrap();
    let at_limit = archive(get(state, &headers, &path).await).await;
    assert_eq!(at_limit["outputs/v-21/stdout.bin"], b"retained output");
    assert!(at_limit.contains_key("outputs/v-21/status.bin"));
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_job_downloads_export_full_object_bytes_and_reject_corruption() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let (_, headers) = postgres_operator_session(&db.repo, "job-download-object-admin").await;
    let job = seed_job(&db).await;
    let root = std::env::temp_dir().join(format!("vpsman-job-download-test-{job}"));
    let store = crate::object_store::BackupObjectStore::filesystem(root.clone()).unwrap();
    let bytes = vec![0x81; 40_000];
    store.put_new("outputs/full.bin", &bytes).await.unwrap();
    sqlx::query("INSERT INTO job_outputs (job_id,client_id,seq,stream,data,storage,object_key,data_sha256_hex,data_size_bytes) VALUES ($1,'v-21',0,'stdout',$2,'object_store','outputs/full.bin',$3,$4)")
        .bind(job).bind(&bytes[..32]).bind(payload_hash(&bytes)).bind(bytes.len() as i64)
        .execute(&db.pool).await.unwrap();
    output(&db, job, 1, "stderr", b"shell error\n").await;
    output(&db, job, 2, "status", br#"{"exit_code":17}"#).await;
    sqlx::query(
        "UPDATE job_targets SET status='failed',exit_code=17 WHERE job_id=$1 AND client_id='v-21'",
    )
    .bind(job)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE jobs SET status='failed' WHERE id=$1")
        .bind(job)
        .execute(&db.pool)
        .await
        .unwrap();
    let mut state = postgres_app_state(&db);
    state.backup_object_store = Some(store);
    let path = format!("/api/v1/jobs/{job}/download");
    let entries = archive(get(state.clone(), &headers, &path).await).await;
    assert_eq!(entries["outputs/v-21/stdout.bin"], bytes);
    assert_eq!(entries["outputs/v-21/stderr.bin"], b"shell error\n");
    assert_eq!(entries["outputs/v-21/status.bin"], br#"{"exit_code":17}"#);
    assert_eq!(
        serde_json::from_slice::<Value>(&entries["v-21_status.json"]).unwrap()["exit_code"],
        17
    );
    assert!(
        !entries.keys().any(|path| path.starts_with("files/")),
        "shell output does not require file-download metadata"
    );
    sqlx::query("UPDATE job_outputs SET data_sha256_hex=repeat('0',64) WHERE job_id=$1")
        .bind(job)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        get(state, &headers, &path).await.status(),
        StatusCode::CONFLICT
    );
    tokio::fs::remove_dir_all(root).await.unwrap();
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_job_downloads_preserve_distinct_valid_client_paths() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let (operator, headers) = postgres_operator_session(&db.repo, "job-download-path-admin").await;
    let job = seed_job(&db).await;
    for (index, client) in ["edge:a", "edge_a", "edge..a", ".", ".."]
        .into_iter()
        .enumerate()
    {
        crate::routes_key_lifecycle::validate_client_id(client).unwrap();
        // Explicit identity import preserves supplied IDs; auto-generated IDs
        // are only a default. Exercise the actual identity repository path.
        let identity = UpsertAgentIdentityRequest {
            client_id: Some(client.to_string()),
            client_public_key_hex: format!("{:02x}", index + 1).repeat(32),
            display_name: Some(format!("Path check {}", index + 1)),
            tags: Vec::new(),
            replace_existing_key: false,
            confirmed: true,
            privilege_assertion: None,
        };
        db.repo
            .preflight_agent_identity_upsert(&identity)
            .await
            .unwrap();
        let imported = db
            .repo
            .upsert_agent_identity(&identity, &operator)
            .await
            .unwrap();
        assert_eq!(imported.client_id, client);
        sqlx::query("INSERT INTO job_targets (job_id,client_id,status,exit_code) VALUES ($1,$2,'completed',0)")
            .bind(job).bind(client).execute(&db.pool).await.unwrap();
        sqlx::query("INSERT INTO job_outputs (job_id,client_id,seq,stream,data,storage) VALUES ($1,$2,0,'stdout',$3,'inline')")
            .bind(job).bind(client).bind(client.as_bytes()).execute(&db.pool).await.unwrap();
    }
    sqlx::query("UPDATE jobs SET target_count=7 WHERE id=$1")
        .bind(job)
        .execute(&db.pool)
        .await
        .unwrap();
    let file_status = json!({"type":"file_download","status":"completed","filename":"status.json","size_bytes":6,"sha256_hex":payload_hash(b"edge:a")});
    sqlx::query("INSERT INTO job_outputs (job_id,client_id,seq,stream,data,storage) VALUES ($1,'edge:a',1,'status',$2,'inline')")
        .bind(job).bind(serde_json::to_vec(&file_status).unwrap()).execute(&db.pool).await.unwrap();
    let state = postgres_app_state(&db);
    let combined = archive(
        get(
            state.clone(),
            &headers,
            &format!("/api/v1/jobs/{job}/download"),
        )
        .await,
    )
    .await;
    let statuses = archive(
        get(
            state.clone(),
            &headers,
            &format!("/api/v1/jobs/{job}/targets/download"),
        )
        .await,
    )
    .await;
    let outputs = archive(
        get(
            state.clone(),
            &headers,
            &format!("/api/v1/jobs/{job}/outputs/archive"),
        )
        .await,
    )
    .await;
    let files = archive(
        get(
            state,
            &headers,
            &format!("/api/v1/jobs/{job}/outputs/download-bundle"),
        )
        .await,
    )
    .await;
    assert_eq!(combined["files/edge%3Aa/status.json"], b"edge:a");
    assert_eq!(files["edge%3Aa/status.json"], b"edge:a");
    for (id, component) in [
        ("edge:a", "edge%3Aa"),
        ("edge_a", "edge_a"),
        ("edge..a", "edge..a"),
        (".", "%2E"),
        ("..", "%2E%2E"),
    ] {
        assert_eq!(
            combined[&format!("outputs/{component}/stdout.bin")],
            id.as_bytes()
        );
        assert_eq!(outputs[&format!("{component}/stdout.bin")], id.as_bytes());
        let status_path = format!("{component}_status.json");
        let target: Value = serde_json::from_slice(&combined[&status_path]).unwrap();
        assert_eq!(target["client_id"], id);
        assert_eq!(statuses[&status_path], combined[&status_path]);
    }
    db.cleanup().await;
}
