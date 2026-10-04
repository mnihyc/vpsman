use super::*;

fn import_fixture(
    start: i64,
    end: i64,
) -> (NetworkTrafficImportResult, Vec<NetworkTrafficImportBucket>) {
    (
        NetworkTrafficImportResult {
            r#type: "network_traffic_import_vnstat".into(),
            status: "collected".into(),
            requested_start_unix: start as u64,
            collected_until_unix: end as u64,
            interfaces: vec!["eth0".into()],
            sources: vec![NetworkTrafficImportSource {
                interface: "eth0".into(),
                database_created_unix: Some(start as u64),
                retained_start_unix: start as u64,
                source_updated_unix: Some(end as u64),
            }],
            batch_count: 1,
            bucket_count: 1,
            message: String::new(),
        },
        vec![NetworkTrafficImportBucket {
            interface: "eth0".into(),
            start_unix: start as u64,
            duration_secs: (end - start) as u32,
            rx_bytes: ((end - start) / 60 * 10) as u64,
            tx_bytes: ((end - start) / 60 * 20) as u64,
        }],
    )
}

async fn live_counter(db: &PgReliabilityTestDb, client: &str, at: i64, bytes: i64) {
    sqlx::query("INSERT INTO traffic_counter_samples (client_id,source_kind,interface,observed_at,rx_bytes,tx_bytes,sample_source) VALUES ($1,'host','eth0',to_timestamp($2::double precision),$3,$3*2,'interface_counters')")
        .bind(client).bind(at as f64).bind(bytes).execute(&db.pool).await.unwrap();
}

#[tokio::test]
async fn postgres_vnstat_ranges_fill_interior_gap_and_narrow_rerun_preserves_prefix() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client = "vnstat-range-gap";
    insert_client(&db.pool, client, None).await;
    let now = Utc::now().timestamp().div_euclid(60) * 60;
    let start = now.div_euclid(86400) * 86400 - 12 * 86400;
    let first = start + 86400;
    let before = first + 86400;
    let after = before + 7 * 86400;
    for (at, bytes) in [(first, 1000), (before, 2000), (after, 102800)] {
        live_counter(&db, client, at, bytes).await;
    }
    let (mut result, buckets) = import_fixture(start, now);
    db.repo
        .import_vnstat_traffic_history(
            Uuid::new_v4(),
            client,
            &["e*".into(), "absent0".into()],
            start as u64,
            &result,
            &buckets,
            now as u64,
        )
        .await
        .unwrap();
    let gap: i64 = sqlx::query_scalar("SELECT COALESCE(sum(rx_bytes),0)::bigint FROM traffic_counter_rollups WHERE client_id=$1 AND origin_kind='vnstat_import' AND bucket_start>=to_timestamp($2::double precision) AND bucket_start<to_timestamp($3::double precision)")
        .bind(client).bind(before as f64).bind(after as f64).fetch_one(&db.pool).await.unwrap();
    assert_eq!(
        gap,
        (7 * 86400 / 60 - 1) * 10,
        "every missing minute of the seven-day gap must be recovered"
    );
    let live: Vec<(i64,i64,i64,i64)> = sqlx::query_as("SELECT extract(epoch FROM observed_at)::bigint,rx_bytes,tx_bytes,rx_counter_epoch FROM traffic_counter_samples WHERE client_id=$1 AND sample_source='interface_counters' ORDER BY observed_at")
        .bind(client).fetch_all(&db.pool).await.unwrap();
    assert_eq!(
        live,
        vec![
            (first, 1000, 2000, 0),
            (before, 2000, 4000, 0),
            (after, 102800, 205600, 0)
        ]
    );
    let usage: i64 = sqlx::query_scalar("SELECT rx_bytes FROM traffic_counter_hourly_usage WHERE client_id=$1 AND bucket_start=to_timestamp($2::double precision)")
        .bind(client).bind(after as f64).fetch_one(&db.pool).await.unwrap();
    assert_eq!(
        usage, 10,
        "resumed counter must not count the imported gap twice"
    );
    let prefix: serde_json::Value = sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(row) ORDER BY bucket_start) FROM traffic_counter_rollups row WHERE client_id=$1 AND bucket_start<to_timestamp($2::double precision)")
        .bind(client).bind(before as f64).fetch_one(&db.pool).await.unwrap();
    result.requested_start_unix = before as u64;
    let rerun = db
        .repo
        .import_vnstat_traffic_history(
            Uuid::new_v4(),
            client,
            &result.interfaces,
            before as u64,
            &result,
            &buckets,
            now as u64,
        )
        .await
        .unwrap();
    assert!(
        rerun.message.contains("0 interface(s) updated"),
        "{}",
        rerun.message
    );
    let later: serde_json::Value=sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(row) ORDER BY bucket_start) FROM traffic_counter_rollups row WHERE client_id=$1 AND bucket_start<to_timestamp($2::double precision)")
        .bind(client).bind(before as f64).fetch_one(&db.pool).await.unwrap();
    assert_eq!(
        prefix, later,
        "narrow reimport must not even rewrite outside-range lineage/timestamps"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_vnstat_ranges_all_absent_finishes_without_ledger_changes() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client = "vnstat-range-absent";
    insert_client(&db.pool, client, None).await;
    let now = Utc::now().timestamp().div_euclid(60) * 60;
    let (mut result, _) = import_fixture(now - 3600, now);
    result.interfaces.clear();
    result.sources.clear();
    result.bucket_count = 0;
    result.batch_count = 0;
    let output = db
        .repo
        .import_vnstat_traffic_history(
            Uuid::new_v4(),
            client,
            &["e*".into()],
            (now - 3600) as u64,
            &result,
            &[],
            now as u64,
        )
        .await
        .unwrap();
    assert!(output.message.contains("0 interface(s) updated"));
    assert!(output.message.contains("1 unmatched selector(s)"));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM traffic_counter_streams WHERE client_id=$1")
            .bind(client)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_vnstat_ranges_preserve_partial_old_bucket_and_opaque_gap() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client = "vnstat-range-preserved";
    insert_client(&db.pool, client, None).await;
    let now = Utc::now().timestamp().div_euclid(60) * 60;
    let start = now.div_euclid(86400) * 86400 - 400 * 86400;
    for (at, origin) in [
        (start, "vnstat_import"),
        (start + 86400, "live"),
        (start + 9 * 86400, "live"),
    ] {
        sqlx::query("INSERT INTO traffic_counter_rollups (client_id,source_kind,interface,origin_kind,bucket_secs,bucket_start,rx_bytes,tx_bytes,rx_valid_count,tx_valid_count,any_valid_count,first_observed_at,latest_observed_at) VALUES ($1,'host','eth0',$2,86400,to_timestamp($3::double precision),100,200,1440,1440,1440,to_timestamp($3::double precision),to_timestamp(($3+86340)::double precision))")
            .bind(client).bind(origin).bind(at as f64).execute(&db.pool).await.unwrap();
    }
    let before:serde_json::Value=sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(row) ORDER BY bucket_start) FROM traffic_counter_rollups row WHERE client_id=$1").bind(client).fetch_one(&db.pool).await.unwrap();
    let (mut result, buckets) = import_fixture(start, start + 10 * 86400);
    result.requested_start_unix = (start + 3600) as u64;
    let output = db
        .repo
        .import_vnstat_traffic_history(
            Uuid::new_v4(),
            client,
            &result.interfaces,
            result.requested_start_unix,
            &result,
            &buckets,
            now as u64,
        )
        .await
        .unwrap();
    assert!(
        output.message.contains("0 interface(s) updated"),
        "{}",
        output.message
    );
    let after:serde_json::Value=sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(row) ORDER BY bucket_start) FROM traffic_counter_rollups row WHERE client_id=$1").bind(client).fetch_one(&db.pool).await.unwrap();
    assert_eq!(before, after);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_vnstat_ranges_preserve_counter_evidence_and_partial_live_minutes() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let now = Utc::now().timestamp().div_euclid(60) * 60;
    let start = now - 7200;
    let end = start + 3600;
    for (client, reset, samples, bytes, expected_rows, expected_live_usage) in [
        ("range-same-epoch", false, 1, 1600, 59, 10),
        ("range-reset", true, 1, 10, 59, 0),
        ("range-ambiguous", false, 2, 1600, 0, 600),
        ("range-inconsistent", false, 1, 1100, 0, 100),
    ] {
        insert_client(&db.pool, client, None).await;
        sqlx::query("INSERT INTO vps_rule_values (client_id,key,value_raw,value_json) VALUES ($1,'traffic.selectors','eth0','{\"mode\":\"exact\",\"selectors\":[{\"source\":\"host\",\"interface\":\"eth0\",\"direction\":\"total\",\"canonical\":\"eth0\"}]}'::jsonb)")
            .bind(client).execute(&db.pool).await.unwrap();
        live_counter(&db, client, start, 1000).await;
        live_counter(&db, client, end, bytes).await;
        sqlx::query("UPDATE traffic_counter_samples SET sample_count=$2,latest_observed_at=observed_at+make_interval(secs=>$3),rx_counter_epoch=$4,tx_counter_epoch=$4,usage_authoritative=TRUE,rx_usage_bytes=$5,tx_usage_bytes=$5*2,rx_valid_count=$6,tx_valid_count=$6,any_valid_count=$6,rx_reset_count=$7,tx_reset_count=$7,any_reset_count=$7 WHERE client_id=$1 AND observed_at=to_timestamp($8::double precision)")
            .bind(client).bind(samples).bind(if samples==1 {0.0} else {30.0})
            .bind(i64::from(reset)).bind(if reset {0} else {bytes-1000})
            .bind(i32::from(!reset)).bind(i32::from(reset)).bind(end as f64)
            .execute(&db.pool).await.unwrap();
        let before: serde_json::Value=sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(row) ORDER BY observed_at) FROM traffic_counter_samples row WHERE client_id=$1")
            .bind(client).fetch_one(&db.pool).await.unwrap();
        let (result, buckets) = import_fixture(start, end + 60);
        db.repo
            .import_vnstat_traffic_history(
                Uuid::new_v4(),
                client,
                &result.interfaces,
                start as u64,
                &result,
                &buckets,
                now as u64,
            )
            .await
            .unwrap();
        let raw: (i64,i64)=sqlx::query_as("SELECT count(*)::bigint,coalesce(sum(rx_usage_bytes),0)::bigint FROM traffic_counter_samples WHERE client_id=$1 AND sample_source LIKE 'vnstat_import:%'")
            .bind(client).fetch_one(&db.pool).await.unwrap();
        assert_eq!(raw, (expected_rows, expected_rows * 10), "{client}");
        let live: (i64,i64,i64,i64,i32)=sqlx::query_as("SELECT rx_bytes,tx_bytes,rx_counter_epoch,rx_usage_bytes,rx_reset_count FROM traffic_counter_samples WHERE client_id=$1 AND observed_at=to_timestamp($2::double precision)")
            .bind(client).bind(end as f64).fetch_one(&db.pool).await.unwrap();
        assert_eq!(
            live,
            (
                bytes,
                bytes * 2,
                i64::from(reset),
                expected_live_usage,
                i32::from(reset)
            ),
            "{client}"
        );
        if expected_rows == 0 {
            let after:serde_json::Value=sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(row) ORDER BY observed_at) FROM traffic_counter_samples row WHERE client_id=$1")
                .bind(client).fetch_one(&db.pool).await.unwrap();
            assert_eq!(
                before, after,
                "ambiguous/inconsistent evidence is completely preserved"
            );
        } else {
            let history = db
                .repo
                .list_traffic_history(client, start as u64, (end + 60) as u64, 60)
                .await
                .unwrap();
            assert_eq!(
                history.iter().filter_map(|row| row.rx_bytes).sum::<i64>(),
                expected_rows * 10 + expected_live_usage
            );
            let replay = db
                .repo
                .import_vnstat_traffic_history(
                    Uuid::new_v4(),
                    client,
                    &result.interfaces,
                    start as u64,
                    &result,
                    &buckets,
                    now as u64,
                )
                .await
                .unwrap();
            assert!(
                replay.message.contains("0 interface(s) updated"),
                "{client}: {}",
                replay.message
            );
        }
    }
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_vnstat_ranges_update_only_selected_raw_minutes_and_preserve_other_sources() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client = "range-local-raw";
    insert_client(&db.pool, client, None).await;
    let now = Utc::now().timestamp().div_euclid(60) * 60;
    let start = now - 7200;
    let (mut result, mut buckets) = import_fixture(start, now - 3600);
    db.repo
        .import_vnstat_traffic_history(
            Uuid::new_v4(),
            client,
            &result.interfaces,
            start as u64,
            &result,
            &buckets,
            now as u64,
        )
        .await
        .unwrap();
    let before:serde_json::Value=sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(row) ORDER BY observed_at) FROM traffic_counter_samples row WHERE client_id=$1 AND observed_at<to_timestamp($2::double precision)")
        .bind(client).bind((start+1800) as f64).fetch_one(&db.pool).await.unwrap();
    result.requested_start_unix = (start + 1800) as u64;
    buckets[0].rx_bytes *= 2;
    buckets[0].tx_bytes *= 2;
    result.interfaces.push("unavailable0".into());
    let mut excluded_source = result.sources[0].clone();
    excluded_source.interface = "docker0".into();
    result.sources.push(excluded_source);
    result.interfaces.push("docker0".into());
    let mut excluded_bucket = buckets[0].clone();
    excluded_bucket.interface = "docker0".into();
    buckets.push(excluded_bucket);
    result.bucket_count = 2;
    let output = db
        .repo
        .import_vnstat_traffic_history(
            Uuid::new_v4(),
            client,
            &["*".into()],
            result.requested_start_unix,
            &result,
            &buckets,
            now as u64,
        )
        .await
        .unwrap();
    assert!(output.message.contains("1 interface(s) updated"));
    assert!(output.message.contains("2 interface(s) with unavailable"));
    let after:serde_json::Value=sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(row) ORDER BY observed_at) FROM traffic_counter_samples row WHERE client_id=$1 AND observed_at<to_timestamp($2::double precision)")
        .bind(client).bind((start+1800) as f64).fetch_one(&db.pool).await.unwrap();
    assert_eq!(before, after);
    let stored:Vec<(String,i64,i64)>=sqlx::query_as("SELECT interface,count(*)::bigint,sum(rx_usage_bytes)::bigint FROM traffic_counter_samples WHERE client_id=$1 GROUP BY interface")
        .bind(client).fetch_all(&db.pool).await.unwrap();
    assert_eq!(stored, vec![("eth0".into(), 60, 900)]);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_vnstat_ranges_empty_or_unavailable_output_terminalizes_and_replays() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let now = Utc::now().timestamp().div_euclid(60) * 60;
    for (client, interfaces) in [
        ("range-no-match", vec![]),
        ("range-no-source", vec!["eth0".into()]),
    ] {
        insert_client(&db.pool, client, Some(Uuid::new_v4())).await;
        let job = Uuid::new_v4();
        insert_job_target_with_operation(
            &db.pool,
            job,
            client,
            JobCommand::NetworkTrafficImportVnstat {
                interfaces: vec!["e*".into()],
                start_unix: (now - 3600) as u64,
            },
            "network_traffic_import_vnstat",
            None,
            "running",
            true,
            Some(Uuid::new_v4()),
            300,
            false,
        )
        .await;
        let (mut result, _) = import_fixture(now - 3600, now);
        result.interfaces = interfaces;
        result.sources.clear();
        result.batch_count = 0;
        result.bucket_count = 0;
        let output = CommandOutput {
            job_id: job,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&result).unwrap(),
            exit_code: Some(0),
            done: true,
        };
        db.repo
            .record_active_job_outputs_checked_with_config(
                job,
                client,
                &[output],
                JobOutputPersistConfig {
                    object_store: None,
                    artifact_min_bytes: usize::MAX,
                },
            )
            .await
            .unwrap();
        // A fresh state stands in for a restarted supervisor after collection.
        assert_eq!(
            crate::job_traffic_import::finalize_pending_network_traffic_imports(
                &postgres_app_state(&db)
            )
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            target_status(&db.pool, job, client).await,
            TARGET_STATUS_COMPLETED
        );
        assert_eq!(job_status(&db.pool, job).await, JOB_STATUS_COMPLETED);
        assert_eq!(
            crate::job_traffic_import::finalize_pending_network_traffic_imports(
                &postgres_app_state(&db)
            )
            .await
            .unwrap(),
            0
        );
        let pending: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM network_traffic_import_finalizations WHERE job_id=$1",
        )
        .bind(job)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(pending, 0);
    }
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_vnstat_ranges_cached_snapshot_queries_stay_scoped_and_bounded() {
    use crate::repository_network_traffic_import::ranges::{
        raw_snapshot_sql, FIRST_LIVE_SQL, ROLLUPS_SQL,
    };
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client = "range-plan-scope";
    insert_client(&db.pool, client, None).await;
    let start = Utc::now().timestamp().div_euclid(86400) * 86400 - 20 * 86400;
    sqlx::query("INSERT INTO traffic_counter_samples(client_id,source_kind,interface,observed_at,rx_bytes,tx_bytes,sample_source) SELECT $1,'host',format('eth%s',stream),to_timestamp(($2+minute*60)::double precision),minute,minute,'interface_counters' FROM generate_series(0,7) stream CROSS JOIN generate_series(0,1999) minute")
        .bind(client).bind(start).execute(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO traffic_counter_rollups(client_id,source_kind,interface,origin_kind,bucket_secs,bucket_start,rx_bytes,tx_bytes,rx_valid_count,tx_valid_count,any_valid_count,first_observed_at,latest_observed_at) SELECT $1,'host',format('eth%s',stream),'live',3600,to_timestamp(($2+hour*3600)::double precision),10,20,1,1,1,to_timestamp(($2+hour*3600)::double precision),to_timestamp(($2+hour*3600)::double precision) FROM generate_series(0,7) stream CROSS JOIN generate_series(0,399) hour")
        .bind(client).bind(start).execute(&db.pool).await.unwrap();
    sqlx::query("ANALYZE traffic_counter_samples")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("ANALYZE traffic_counter_rollups")
        .execute(&db.pool)
        .await
        .unwrap();
    let mut connection = db.pool.acquire().await.unwrap();
    for (name, args, query) in [
        (
            "range_raw",
            "(text,text,double precision,double precision,bigint)",
            raw_snapshot_sql(),
        ),
        ("range_first", "(text,text)", FIRST_LIVE_SQL.to_string()),
        (
            "range_rollups",
            "(text,text,double precision,double precision)",
            ROLLUPS_SQL.to_string(),
        ),
    ] {
        sqlx::query(&format!("PREPARE {name}{args} AS {query}"))
            .execute(&mut *connection)
            .await
            .unwrap();
    }
    for mode in ["force_custom_plan", "force_generic_plan"] {
        sqlx::query("SELECT set_config('plan_cache_mode',$1,false)")
            .bind(mode)
            .execute(&mut *connection)
            .await
            .unwrap();
        for (name, args, max_rows) in [
            (
                "range_raw",
                format!("'{client}','eth0',{},{},1502", start + 600, start + 1200),
                12.0,
            ),
            ("range_first", format!("'{client}','eth0'"), 5.0),
            (
                "range_rollups",
                format!("'{client}','eth0',{},{},", start + 86400, start + 90000)
                    .trim_end_matches(',')
                    .to_string(),
                25.0,
            ),
        ] {
            let plan: serde_json::Value = sqlx::query_scalar(&format!(
                "EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) EXECUTE {name}({args})"
            ))
            .fetch_one(&mut *connection)
            .await
            .unwrap();
            fn audit(plan: &serde_json::Value, rows: &mut f64) {
                if matches!(
                    plan["Relation Name"].as_str(),
                    Some("traffic_counter_samples" | "traffic_counter_rollups")
                ) {
                    assert_ne!(plan["Node Type"], "Seq Scan", "{plan}");
                    let cond = plan
                        .get("Index Cond")
                        .or_else(|| plan.get("Recheck Cond"))
                        .and_then(serde_json::Value::as_str)
                        .expect("indexed stream predicate");
                    assert!(
                        cond.contains("client_id") && cond.contains("interface"),
                        "{plan}"
                    );
                    *rows += (plan["Actual Rows"].as_f64().unwrap_or(0.0)
                        + plan["Rows Removed by Filter"].as_f64().unwrap_or(0.0))
                        * plan["Actual Loops"].as_f64().unwrap_or(1.0);
                }
                assert_eq!(
                    plan["Temp Written Blocks"].as_i64().unwrap_or(0),
                    0,
                    "{plan}"
                );
                if let Some(children) = plan["Plans"].as_array() {
                    for child in children {
                        audit(child, rows);
                    }
                }
            }
            let mut rows = 0.0;
            audit(&plan[0]["Plan"], &mut rows);
            assert!(
                rows <= max_rows,
                "{mode} {name} visited {rows} rows: {plan}"
            );
        }
    }
    sqlx::query("SET plan_cache_mode=auto")
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    let rows = sqlx::query(&raw_snapshot_sql())
        .bind(client)
        .bind("eth0")
        .bind(start as f64)
        .bind((start + 2000 * 60) as f64)
        .bind(1502_i64)
        .fetch_all(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        rows.len(),
        1502,
        "oversized raw history is detected with a bounded sentinel"
    );
    let (result, buckets) = import_fixture(start, start + 2000 * 60);
    let output = db
        .repo
        .import_vnstat_traffic_history(
            Uuid::new_v4(),
            client,
            &result.interfaces,
            start as u64,
            &result,
            &buckets,
            Utc::now().timestamp() as u64,
        )
        .await
        .unwrap();
    assert!(output.message.contains("0 interface(s) updated"));
    assert!(output.message.contains("1 interface(s) with unavailable"));
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_vnstat_ranges_revalidate_live_arrival_before_publication() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client = "range-live-race";
    insert_client(&db.pool, client, None).await;
    let now = Utc::now().timestamp().div_euclid(60) * 60;
    let start = now - 3600;
    let (result, buckets) = import_fixture(start, start + 1800);
    let mut owner = db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM clients WHERE id=$1 FOR UPDATE")
        .bind(client)
        .fetch_one(&mut *owner)
        .await
        .unwrap();
    let repo = db.repo.clone();
    let task = tokio::spawn(async move {
        repo.import_vnstat_traffic_history(
            Uuid::new_v4(),
            client,
            &result.interfaces,
            start as u64,
            &result,
            &buckets,
            now as u64,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(10),async {
        loop {
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT id FROM clients WHERE id = $1 FOR UPDATE%')")
                .fetch_one(&db.pool).await.unwrap();
            if waiting {break;}
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("importer must reach client lock after read-only preparation");
    sqlx::query("INSERT INTO traffic_counter_samples(client_id,source_kind,interface,observed_at,rx_bytes,tx_bytes,sample_source) VALUES ($1,'host','eth0',to_timestamp($2::double precision),100,200,'interface_counters')")
        .bind(client).bind((start+900) as f64).execute(&mut *owner).await.unwrap();
    owner.commit().await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let imported:(i64,i64)=sqlx::query_as("SELECT count(*)::bigint,extract(epoch FROM max(observed_at))::bigint FROM traffic_counter_samples WHERE client_id=$1 AND sample_source LIKE 'vnstat_import:%'")
        .bind(client).fetch_one(&db.pool).await.unwrap();
    assert_eq!(
        imported,
        (15, start + 840),
        "stale full-range preparation must be discarded"
    );
    let live:(i64,i64)=sqlx::query_as("SELECT rx_bytes,tx_bytes FROM traffic_counter_samples WHERE client_id=$1 AND sample_source='interface_counters'")
        .bind(client).fetch_one(&db.pool).await.unwrap();
    assert_eq!(live, (100, 200));
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_vnstat_ranges_clamp_each_available_source_independently() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client = "range-source-bounds";
    insert_client(&db.pool, client, None).await;
    let now = Utc::now().timestamp().div_euclid(60) * 60;
    let start = now - 3600;
    let (mut result, mut buckets) = import_fixture(start, start + 1800);
    let mut later = result.sources[0].clone();
    later.interface = "eth1".into();
    later.database_created_unix = Some((start + 600) as u64);
    later.retained_start_unix = (start + 600) as u64;
    // A database can be newer than its last retained traffic bucket. Import
    // the available portion instead of discarding it with the missing tail.
    later.source_updated_unix = Some((start + 1500) as u64);
    result.sources.push(later);
    result.interfaces.push("eth1".into());
    result.bucket_count = 2;
    buckets.push(NetworkTrafficImportBucket {
        interface: "eth1".into(),
        start_unix: (start + 600) as u64,
        duration_secs: 600,
        rx_bytes: 100,
        tx_bytes: 200,
    });
    db.repo
        .import_vnstat_traffic_history(
            Uuid::new_v4(),
            client,
            &["e*".into()],
            start as u64,
            &result,
            &buckets,
            now as u64,
        )
        .await
        .unwrap();
    let ranges:Vec<(String,i64,i64,i64,i64)>=sqlx::query_as("SELECT interface,extract(epoch FROM min(observed_at))::bigint,extract(epoch FROM max(observed_at))::bigint,count(*)::bigint,sum(rx_usage_bytes)::bigint FROM traffic_counter_samples WHERE client_id=$1 GROUP BY interface ORDER BY interface")
        .bind(client).fetch_all(&db.pool).await.unwrap();
    assert_eq!(
        ranges,
        vec![
            ("eth0".into(), start, start + 1740, 30, 300),
            ("eth1".into(), start + 600, start + 1140, 10, 100)
        ]
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_vnstat_ranges_multiple_gaps_and_pending_retention_do_not_double_count() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client = "range-multiple-gaps";
    insert_client(&db.pool, client, None).await;
    let now = Utc::now().timestamp().div_euclid(60) * 60;
    let start = now - 3600;
    sqlx::query("INSERT INTO traffic_counter_samples(client_id,source_kind,interface,observed_at,rx_bytes,tx_bytes,rx_counter_epoch,tx_counter_epoch,sample_source) SELECT $1,'host','eth0',to_timestamp(($2+minute*60)::double precision),bytes,bytes*2,epoch,epoch,'interface_counters' FROM (VALUES (0,1000,0),(10,1100,0),(11,1105,0),(20,20,1),(21,35,1)) sample(minute,bytes,epoch)")
        .bind(client).bind(start).execute(&db.pool).await.unwrap();
    let (result, buckets) = import_fixture(start, start + 1500);
    db.repo
        .import_vnstat_traffic_history(
            Uuid::new_v4(),
            client,
            &result.interfaces,
            start as u64,
            &result,
            &buckets,
            now as u64,
        )
        .await
        .unwrap();
    let imported:(i64,i64)=sqlx::query_as("SELECT count(*)::bigint,sum(rx_usage_bytes)::bigint FROM traffic_counter_samples WHERE client_id=$1 AND sample_source LIKE 'vnstat_import:%'")
        .bind(client).fetch_one(&db.pool).await.unwrap();
    assert_eq!(imported, (17, 170));
    let usage:(i64,i64)=sqlx::query_as("SELECT sum(rx_bytes)::bigint,sum(tx_bytes)::bigint FROM traffic_counter_hourly_usage WHERE client_id=$1")
        .bind(client).fetch_one(&db.pool).await.unwrap();
    assert_eq!(usage, (200, 400));
    let replay = db
        .repo
        .import_vnstat_traffic_history(
            Uuid::new_v4(),
            client,
            &result.interfaces,
            start as u64,
            &result,
            &buckets,
            now as u64,
        )
        .await
        .unwrap();
    assert!(replay.message.contains("0 interface(s) updated"));

    let client = "range-retention-frontier";
    insert_client(&db.pool, client, None).await;
    let cutoff:i64=sqlx::query_scalar("SELECT extract(epoch FROM date_bin('1 hour',now()-make_interval(days=>$1),TIMESTAMPTZ '1970-01-01'))::bigint")
        .bind(TRAFFIC_COUNTER_RAW_RETENTION_DAYS).fetch_one(&db.pool).await.unwrap();
    let start = cutoff - 3600;
    // An old unpromoted import must remain owned by the retention worker. A
    // rerun cannot install a rollup over these still-active raw contributions.
    sqlx::query("INSERT INTO traffic_counter_samples(client_id,source_kind,interface,observed_at,rx_bytes,tx_bytes,sample_source,rx_usage_bytes,tx_usage_bytes,rx_valid_count,tx_valid_count,any_valid_count,usage_authoritative) SELECT $1,'host','eth0',to_timestamp(($2+minute*60)::double precision),minute*10,minute*20,'vnstat_import:old',10,20,1,1,1,TRUE FROM generate_series(0,59) minute")
        .bind(client).bind(start).execute(&db.pool).await.unwrap();
    let (result, buckets) = import_fixture(start, cutoff);
    let output = db
        .repo
        .import_vnstat_traffic_history(
            Uuid::new_v4(),
            client,
            &result.interfaces,
            start as u64,
            &result,
            &buckets,
            now as u64,
        )
        .await
        .unwrap();
    assert!(output.message.contains("0 interface(s) updated"));
    let rollups: i64 =
        sqlx::query_scalar("SELECT count(*) FROM traffic_counter_rollups WHERE client_id=$1")
            .bind(client)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(rollups, 0);
    let usage:(i64,i64)=sqlx::query_as("SELECT sum(rx_bytes)::bigint,sum(tx_bytes)::bigint FROM traffic_counter_hourly_usage WHERE client_id=$1").bind(client).fetch_one(&db.pool).await.unwrap();
    assert_eq!(usage, (600, 1200));
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_vnstat_ranges_replay_preserves_disjoint_gaps_in_one_old_rollup() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client = "range-disjoint-old-rollup";
    insert_client(&db.pool, client, Some(Uuid::new_v4())).await;
    let initial_job = Uuid::new_v4();
    let now = Utc::now().timestamp().div_euclid(60) * 60;
    let start = now.div_euclid(86400) * 86400 - 3 * 86400;
    // Two independently reconcilable gaps share an old hourly bucket. Large
    // live bridge residuals expose accidental subtraction on a later replay.
    for (minute, bytes) in [(0, 1000), (10, 2000), (11, 2010), (20, 3010), (21, 3020)] {
        live_counter(&db, client, start + minute * 60, bytes).await;
    }
    let (result, buckets) = import_fixture(start, start + 1500);
    insert_job_target_with_operation(
        &db.pool,
        initial_job,
        client,
        JobCommand::NetworkTrafficImportVnstat {
            interfaces: result.interfaces.clone(),
            start_unix: start as u64,
        },
        "network_traffic_import_vnstat",
        None,
        "running",
        true,
        Some(Uuid::new_v4()),
        300,
        false,
    )
    .await;
    let batch = vpsman_common::NetworkTrafficImportBatch {
        r#type: "network_traffic_import_vnstat_batch".into(),
        batch_index: 0,
        buckets: buckets.clone(),
    };
    let outputs = [
        CommandOutput {
            job_id: initial_job,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&batch).unwrap(),
            exit_code: None,
            done: false,
        },
        CommandOutput {
            job_id: initial_job,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&result).unwrap(),
            exit_code: Some(0),
            done: true,
        },
    ];
    db.repo
        .record_active_job_outputs_checked_with_config(
            initial_job,
            client,
            &outputs,
            JobOutputPersistConfig {
                object_store: None,
                artifact_min_bytes: usize::MAX,
            },
        )
        .await
        .unwrap();
    let abandoned_owner = db
        .repo
        .claim_network_traffic_import_finalization(30)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(abandoned_owner.job_id, initial_job);
    db.repo
        .import_vnstat_traffic_history(
            initial_job,
            client,
            &result.interfaces,
            start as u64,
            &result,
            &buckets,
            now as u64,
        )
        .await
        .unwrap();
    let rollup: (i64, i32) = sqlx::query_as("SELECT rx_bytes,any_valid_count FROM traffic_counter_rollups WHERE client_id=$1 AND origin_kind='vnstat_import'")
        .bind(client).fetch_one(&db.pool).await.unwrap();
    assert_eq!(rollup, (170, 17));
    let snapshot = "SELECT jsonb_build_object('raw',(SELECT jsonb_agg(to_jsonb(s) ORDER BY observed_at) FROM traffic_counter_samples s WHERE client_id=$1),'rollups',(SELECT jsonb_agg(to_jsonb(r) ORDER BY bucket_start) FROM traffic_counter_rollups r WHERE client_id=$1),'usage',(SELECT jsonb_agg(to_jsonb(u) ORDER BY bucket_start) FROM traffic_counter_hourly_usage u WHERE client_id=$1))";
    let before: serde_json::Value = sqlx::query_scalar(snapshot)
        .bind(client)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    // Identical replay must be idempotent. Changed source totals cannot safely
    // replace this sparse rollup either: it no longer retains each gap's bytes.
    for multiplier in [1, 2] {
        let mut revised = buckets.clone();
        revised[0].rx_bytes *= multiplier;
        revised[0].tx_bytes *= multiplier;
        let output = db
            .repo
            .import_vnstat_traffic_history(
                Uuid::new_v4(),
                client,
                &result.interfaces,
                start as u64,
                &result,
                &revised,
                now as u64,
            )
            .await
            .unwrap();
        let after: serde_json::Value = sqlx::query_scalar(snapshot)
            .bind(client)
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(
            before, after,
            "preserved rollups must also preserve live accounting boundaries"
        );
        assert!(output.message.contains("0 interface(s) updated"));
    }
    // Crash after data commit but before recording target completion. A new
    // supervisor must recover the expired claim without changing accounting.
    sqlx::query("UPDATE network_traffic_import_finalizations SET lease_until=now()-interval '1 second' WHERE job_id=$1")
        .bind(initial_job).execute(&db.pool).await.unwrap();
    assert_eq!(
        crate::job_traffic_import::finalize_pending_network_traffic_imports(&postgres_app_state(
            &db
        ))
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        target_status(&db.pool, initial_job, client).await,
        TARGET_STATUS_COMPLETED
    );
    assert_eq!(
        job_status(&db.pool, initial_job).await,
        JOB_STATUS_COMPLETED
    );
    assert!(!abandoned_owner.acknowledge().await.unwrap());
    let after: serde_json::Value = sqlx::query_scalar(snapshot)
        .bind(client)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        crate::job_traffic_import::finalize_pending_network_traffic_imports(&postgres_app_state(
            &db
        ))
        .await
        .unwrap(),
        0
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_vnstat_ranges_preserve_rollups_overlapping_late_live_evidence() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client = "range-late-rollup-evidence";
    insert_client(&db.pool, client, None).await;
    let now = Utc::now().timestamp().div_euclid(60) * 60;
    let start = now.div_euclid(86400) * 86400 - 3 * 86400;
    live_counter(&db, client, start + 600, 2000).await;
    let (result, buckets) = import_fixture(start, start + 1500);
    db.repo
        .import_vnstat_traffic_history(
            Uuid::new_v4(),
            client,
            &result.interfaces,
            start as u64,
            &result,
            &buckets,
            now as u64,
        )
        .await
        .unwrap();
    // Late live evidence now overlaps the previously imported dense prefix.
    // Another repairable gap shares its bucket, so publication must preserve
    // both the old rollup and the new gap's accounting boundary together.
    for (minute, bytes) in [(5, 1500), (11, 2010), (20, 3010)] {
        live_counter(&db, client, start + minute * 60, bytes).await;
    }
    let snapshot = "SELECT jsonb_build_object('raw',(SELECT jsonb_agg(to_jsonb(s) ORDER BY observed_at) FROM traffic_counter_samples s WHERE client_id=$1),'rollups',(SELECT jsonb_agg(to_jsonb(r) ORDER BY bucket_start) FROM traffic_counter_rollups r WHERE client_id=$1))";
    let before: serde_json::Value = sqlx::query_scalar(snapshot)
        .bind(client)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let output = db
        .repo
        .import_vnstat_traffic_history(
            Uuid::new_v4(),
            client,
            &result.interfaces,
            start as u64,
            &result,
            &buckets,
            now as u64,
        )
        .await
        .unwrap();
    let after: serde_json::Value = sqlx::query_scalar(snapshot)
        .bind(client)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    assert!(output.message.contains("0 interface(s) updated"));
    assert!(output.message.contains("1 interface(s) with unavailable"));
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_vnstat_ranges_do_not_fabricate_network_rates_at_live_boundaries() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client = "range-rate-boundary";
    insert_client(&db.pool, client, None).await;
    let now = Utc::now().timestamp().div_euclid(60) * 60;
    let start = now - 3600;
    live_counter(&db, client, start + 600, 1_000_000_000).await;
    live_counter(&db, client, start + 660, 1_000_000_100).await;
    let (result, buckets) = import_fixture(start, start + 1200);
    db.repo
        .import_vnstat_traffic_history(
            Uuid::new_v4(),
            client,
            &result.interfaces,
            start as u64,
            &result,
            &buckets,
            now as u64,
        )
        .await
        .unwrap();
    let rates = db
        .repo
        .list_telemetry_network_rates(100, Some(client), Some("eth0"), Some(60), false)
        .await
        .unwrap();
    assert!(!rates.is_empty());
    assert!(
        rates
            .iter()
            .all(|rate| rate.rx_bytes_delta <= 100 && rate.tx_bytes_delta <= 200),
        "synthetic counters must never bridge into the physical billion-byte counter: {rates:?}"
    );
    assert!(
        rates
            .iter()
            .any(|rate| rate.rx_bytes_delta == 100 && rate.tx_bytes_delta == 200),
        "later live rates stay intact"
    );
    let epochs:Vec<i64>=sqlx::query_scalar("SELECT DISTINCT rx_counter_epoch FROM traffic_counter_samples WHERE client_id=$1 AND sample_source='interface_counters'").bind(client).fetch_all(&db.pool).await.unwrap();
    assert_eq!(epochs, vec![0], "only synthetic epochs change");
    db.cleanup().await;
}
