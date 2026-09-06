use super::*;

const RESOURCE_SOURCE_ROWS_SQL: &str = r#"
SELECT COALESCE(jsonb_agg(to_jsonb(point)
           ORDER BY point.client_id, point.bucket_start,
                    point.latest_observed_at, point.bucket_secs), '[]'::JSONB)
FROM telemetry_resource_points_source($1, $2, $3, $4, $5) point
"#;

// The unchanged all-client relation is the canonical setwise oracle. Apply
// exact ownership and per-owner ordering afterward, independently of the
// optimized helper's owner arrays and indexed top-N merge.
const RESOURCE_SOURCE_REFERENCE_SQL: &str = r#"
WITH ranked AS (
    SELECT point.*, row_number() OVER (
        PARTITION BY point.client_id
        ORDER BY point.bucket_start DESC, point.latest_observed_at DESC,
                 point.bucket_secs ASC
    ) AS recency_rank
    FROM telemetry_resource_points_source(NULL::TEXT[], $2, $3, $4, NULL) point
    WHERE point.client_id = ANY($1)
)
SELECT COALESCE(jsonb_agg(to_jsonb(point) - 'recency_rank'
           ORDER BY point.client_id, point.bucket_start,
                    point.latest_observed_at, point.bucket_secs), '[]'::JSONB)
FROM ranked point
WHERE $5::BIGINT IS NULL OR point.recency_rank <= $5
"#;

#[tokio::test]
async fn postgres_resource_history_owner_merge_preserves_every_field_and_top_n() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let raw_client = "resource-owned-raw";
    let retained_client = "resource-owned-retained";
    let unrelated_client = "resource-unrequested";
    for client_id in [raw_client, retained_client, unrelated_client] {
        insert_client(&db.pool, client_id, None).await;
    }
    let first = (crate::unix_now() / 86_400 - 1) * 86_400;
    for (index, offset) in [10, 20, 75, 145].into_iter().enumerate() {
        let present = index != 3;
        let metrics = AgentMetrics {
            observed_unix: first + offset,
            cpu: CpuStat {
                utilization_ratio: present.then_some(0.25),
                cores: 4,
                load: LoadAverage {
                    one: (index + 1) as f64,
                    five: (index + 2) as f64,
                    fifteen: (index + 3) as f64,
                },
            },
            memory: vpsman_common::MemoryStat {
                total_bytes: 1_000,
                available_bytes: 300 + 100 * index as u64,
                swap_total_bytes: present.then_some(500),
                swap_available_bytes: present.then_some(250),
            },
            disks: if present {
                vec![vpsman_common::DiskStat {
                    mountpoint: "/".to_string(),
                    total_bytes: 4_000,
                    available_bytes: 1_000 + 100 * index as u64,
                }]
            } else {
                Vec::new()
            },
            connections: present.then_some(vpsman_common::ConnectionStat {
                tcp: 10 + index as u64,
                udp: 20 + index as u64,
            }),
            ..Default::default()
        };
        insert_raw_telemetry_fixture(&db.pool, raw_client, metrics.observed_unix, &metrics).await;
    }
    // The first sample was already consumed, but the second sample reopens
    // that same minute: both must still contribute to its replacement.
    sqlx::query(
        "UPDATE telemetry_minute_materialization_heads SET materialized_seq=1, materialized_at=clock_timestamp() WHERE client_id=$1",
    )
    .bind(raw_client)
    .execute(&db.pool)
    .await
    .unwrap();
    // Accepted but not projected evidence must not enter the read owner.
    insert_projected_raw_telemetry_fixture(
        &db.pool,
        raw_client,
        first + 160,
        &AgentMetrics::default(),
        5,
        &[],
        &[],
    )
    .await;
    sqlx::query("UPDATE telemetry_projection_heads SET accepted_seq=5 WHERE client_id=$1")
        .bind(raw_client)
        .execute(&db.pool)
        .await
        .unwrap();
    for client_id in [raw_client, retained_client, unrelated_client] {
        sqlx::query(
            r#"
            INSERT INTO telemetry_rollups (
                client_id, bucket_start, bucket_secs, sample_count,
                cpu_load_1_avg, cpu_load_1_sum, cpu_load_1_max,
                memory_total_bytes_max, memory_available_bytes_avg,
                memory_available_bytes_sum, memory_available_bytes_min,
                memory_used_ratio_avg, memory_used_ratio_sum,
                memory_used_ratio_max, latest_observed_at
            )
            SELECT $1, to_timestamp($2::BIGINT + coordinate.offset_secs),
                   coordinate.bucket_secs, 3, 9, 27, 9,
                   1000, 500, 1500, 500, 0.5, 1.5, 0.5,
                   to_timestamp($2::BIGINT + coordinate.offset_secs + 30)
            FROM (VALUES
                (-86400, 86400), (0, 60), (60, 60),
                (120, 60), (180, 60), (0, 300)
            ) coordinate(offset_secs, bucket_secs)
            "#,
        )
        .bind(client_id)
        .bind(first as i64)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    let owners = vec![
        raw_client.to_string(),
        retained_client.to_string(),
        raw_client.to_string(),
        "resource-missing".to_string(),
    ];
    let start = chrono::DateTime::from_timestamp(first as i64, 0).unwrap();
    let cases = [
        (None, None, None::<i32>, None::<i64>),
        (None, None, None, Some(0)),
        (None, None, None, Some(1)),
        (None, None, None, Some(2)),
        (None, None, Some(60), Some(2)),
        (None, None, Some(300), Some(1)),
        (Some(start), Some(start), None, None),
        (
            Some(start + chrono::Duration::seconds(1)),
            Some(start + chrono::Duration::seconds(179)),
            None,
            Some(2),
        ),
    ];
    for (min, max, bucket, limit) in cases {
        let actual = sqlx::query_scalar::<_, Value>(RESOURCE_SOURCE_ROWS_SQL)
            .bind(&owners)
            .bind(min)
            .bind(max)
            .bind(bucket)
            .bind(limit)
            .fetch_one(&db.pool)
            .await
            .unwrap();
        let expected = sqlx::query_scalar::<_, Value>(RESOURCE_SOURCE_REFERENCE_SQL)
            .bind(&owners)
            .bind(min)
            .bind(max)
            .bind(bucket)
            .bind(limit)
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(
            actual, expected,
            "min={min:?} max={max:?} bucket={bucket:?} limit={limit:?}"
        );
    }
    let rows = sqlx::query_scalar::<_, Value>(RESOURCE_SOURCE_ROWS_SQL)
        .bind(&owners)
        .bind(Some(start))
        .bind(Some(start))
        .bind(None::<i32>)
        .bind(None::<i64>)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let raw_minute = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["client_id"] == raw_client && row["bucket_secs"] == 60)
        .unwrap();
    assert_eq!(raw_minute["sample_count"], 2);
    assert_eq!(raw_minute["cpu_load_1_avg"], 1.5);
    assert_eq!(raw_minute["tcp_sockets_latest"], 11);
    assert!(
        rows.as_array()
            .unwrap()
            .iter()
            .any(|row| { row["client_id"] == raw_client && row["bucket_secs"] == 300 }),
        "raw minute replacement must not shadow a coarser bucket"
    );
    let empty = sqlx::query_scalar::<_, Value>(RESOURCE_SOURCE_ROWS_SQL)
        .bind(Vec::<String>::new())
        .bind(None::<chrono::DateTime<Utc>>)
        .bind(None::<chrono::DateTime<Utc>>)
        .bind(None::<i32>)
        .bind(None::<i64>)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(empty, json!([]));
    // The public all-client source intentionally has no per-owner limit.
    let all = sqlx::query_scalar::<_, Value>(RESOURCE_SOURCE_ROWS_SQL)
        .bind(None::<Vec<String>>)
        .bind(None::<chrono::DateTime<Utc>>)
        .bind(None::<chrono::DateTime<Utc>>)
        .bind(None::<i32>)
        .bind(Some(0_i64))
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(all.as_array().unwrap().len(), 18);
    db.cleanup().await;
}

fn resource_suffix_scan_work(plan: &Value) -> (f64, f64) {
    let mut work = (0.0, 0.0);
    if plan.get("CTE Name").and_then(Value::as_str) == Some("projected_suffix") {
        let loops = plan["Actual Loops"].as_f64().unwrap_or(0.0);
        work.0 += loops;
        work.1 += loops
            * (plan["Actual Rows"].as_f64().unwrap_or(0.0)
                + plan["Rows Removed by Filter"].as_f64().unwrap_or(0.0));
    }
    if let Some(children) = plan.get("Plans").and_then(Value::as_array) {
        for child in children {
            let child_work = resource_suffix_scan_work(child);
            work.0 += child_work.0;
            work.1 += child_work.1;
        }
    }
    work
}

#[tokio::test]
async fn postgres_resource_history_fleet_suffix_is_scanned_once_per_query() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let first = (crate::unix_now() / 60 - 16) * 60;
    // Match the existing Cards scale: 120 independently owned VPS resources,
    // sixteen open minutes each. This is fixture size, not a runtime cap.
    sqlx::query(
        r#"
        INSERT INTO clients (id, display_name, public_key, status,
                             internal_build_number, capabilities)
        SELECT 'resource-scale-' || owner, 'Resource ' || owner,
               decode(md5(owner::TEXT) || md5(owner::TEXT || '-key'), 'hex'),
               'online', 1, '{}'::JSONB
        FROM generate_series(1, 120) owner
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO telemetry_samples (
            id, client_id, observed_at, cpu_utilization_ratio, cpu_cores,
            cpu_load_1, cpu_load_5, cpu_load_15,
            memory_total_bytes, memory_available_bytes,
            tcp_sockets, udp_sockets, payload, accepted_seq, accepted_at,
            source_gateway_id, source_gateway_session_id,
            source_process_incarnation_id, source_telemetry_seq,
            reported_observed_unix
        )
        SELECT md5(owner::TEXT || ':' || minute)::UUID,
               'resource-scale-' || owner,
               to_timestamp($1 + minute * 60 + 30), 0.25, 4, 1, 2, 3,
               1000, 500, 10, 20,
               '{"connections":{"tcp":10,"udp":20}}'::JSONB,
               minute + 1, clock_timestamp(), 'resource-fixture',
               '10000000-0000-4000-8000-000000000001'::UUID,
               '10000000-0000-4000-8000-000000000002'::UUID,
               minute + 1, $1 + minute * 60 + 30
        FROM generate_series(1, 120) owner
        CROSS JOIN generate_series(0, 15) minute
        "#,
    )
    .bind(first as i64)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE telemetry_projection_heads SET accepted_seq=16, projected_seq=16")
        .execute(&db.pool)
        .await
        .unwrap();
    // Same-coordinate durable rows exercise shadow rejection; the older
    // coordinate exercises an ordinary retained point within this range.
    sqlx::query(
        r#"
        INSERT INTO telemetry_rollups (
            client_id, bucket_start, bucket_secs, sample_count,
            cpu_load_1_avg, cpu_load_1_sum, cpu_load_1_max,
            memory_total_bytes_max, memory_available_bytes_avg,
            memory_available_bytes_sum, memory_available_bytes_min,
            memory_used_ratio_avg, memory_used_ratio_sum,
            memory_used_ratio_max, latest_observed_at
        )
        SELECT 'resource-scale-' || owner,
               to_timestamp($1 + minute * 60), 60, 1, 9, 9, 9,
               1000, 500, 500, 500, 0.5, 0.5, 0.5,
               to_timestamp($1 + minute * 60 + 30)
        FROM generate_series(1, 120) owner
        CROSS JOIN generate_series(-1, 15) minute
        "#,
    )
    .bind(first as i64)
    .execute(&db.pool)
    .await
    .unwrap();
    for relation in [
        "clients",
        "telemetry_projection_heads",
        "telemetry_minute_materialization_heads",
        "telemetry_samples",
        "telemetry_rollups",
    ] {
        sqlx::query(&format!("ANALYZE {relation}"))
            .execute(&db.pool)
            .await
            .unwrap();
    }
    let mut tx = db.pool.begin().await.unwrap();
    // Execute the released source under an isolated reference name against
    // this same fixture. This compares returned data and actual query plans,
    // not migration text or implementation spelling.
    let baseline = include_str!("../../../../../migrations/0003_telemetry_core.sql");
    let start = baseline
        .find("CREATE FUNCTION public.telemetry_resource_points_source(")
        .unwrap();
    let (definition, _) = baseline[start..].split_once("\n$$;").unwrap();
    let reference = format!(
        "{}\n$$;",
        definition.replacen(
            "telemetry_resource_points_source(",
            "resource_points_reference(",
            1,
        )
    );
    sqlx::raw_sql(&reference).execute(&mut *tx).await.unwrap();
    sqlx::query(
        "PREPARE resource_owner_history(TEXT[],TIMESTAMPTZ,TIMESTAMPTZ,INTEGER,BIGINT) AS SELECT * FROM telemetry_resource_points_source($1,$2,$3,$4,$5)",
    ).execute(&mut *tx).await.unwrap();
    sqlx::query(
        "PREPARE resource_reference_history(TEXT[],TIMESTAMPTZ,TIMESTAMPTZ,INTEGER,BIGINT) AS SELECT * FROM resource_points_reference($1,$2,$3,$4,$5)",
    ).execute(&mut *tx).await.unwrap();
    let owners = (1..=120)
        .map(|owner| format!("'resource-scale-{owner}'"))
        .collect::<Vec<_>>()
        .join(",");
    let explain = format!(
        "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) EXECUTE resource_owner_history(ARRAY[{owners}]::TEXT[], to_timestamp({}), to_timestamp({}), NULL, 16)",
        first - 60, first + 959,
    );
    let owner_ids = (1..=120)
        .map(|owner| format!("resource-scale-{owner}"))
        .collect::<Vec<_>>();
    let mut full_rows = Vec::new();
    for query in [
        RESOURCE_SOURCE_ROWS_SQL.to_string(),
        RESOURCE_SOURCE_ROWS_SQL.replace(
            "telemetry_resource_points_source",
            "resource_points_reference",
        ),
    ] {
        full_rows.push(
            sqlx::query_scalar::<_, Value>(&query)
                .bind(&owner_ids)
                .bind(chrono::DateTime::from_timestamp(first as i64 - 60, 0))
                .bind(chrono::DateTime::from_timestamp(first as i64 + 959, 0))
                .bind(None::<i32>)
                .bind(16_i64)
                .fetch_one(&mut *tx)
                .await
                .unwrap(),
        );
    }
    assert_eq!(
        full_rows[0], full_rows[1],
        "every returned field must match the released source on the same fleet"
    );
    for mode in ["force_custom_plan", "force_generic_plan"] {
        sqlx::query(&format!("SET LOCAL plan_cache_mode = {mode}"))
            .execute(&mut *tx)
            .await
            .unwrap();
        for (label, query) in [
            (
                "before",
                explain.replace("resource_owner_history", "resource_reference_history"),
            ),
            ("after", explain.clone()),
        ] {
            let plan = sqlx::query_scalar::<_, Value>(&query)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            let (scans, examined) = resource_suffix_scan_work(&plan[0]["Plan"]);
            if label == "after" {
                assert_eq!(
                    scans, 1.0,
                    "{mode}: a fleet suffix must not be rescanned by each owner: {plan}"
                );
                assert_eq!(
                    examined,
                    120.0 * 16.0,
                    "{mode}: each projected minute is read once from the shared suffix"
                );
                assert_eq!(plan[0]["Plan"]["Actual Rows"], 120 * 16);
            }
            eprintln!("resource history {label} {mode}: 120 clients × 16 points; suffix scans={scans}, examined={examined}, execution_ms={}, shared_hits={}, shared_reads={}, temp_reads={}, temp_writes={}", plan[0]["Execution Time"], plan[0]["Plan"]["Shared Hit Blocks"], plan[0]["Plan"]["Shared Read Blocks"], plan[0]["Plan"]["Temp Read Blocks"], plan[0]["Plan"]["Temp Written Blocks"]);
        }
    }
    sqlx::query("DEALLOCATE resource_owner_history")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("DEALLOCATE resource_reference_history")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    db.cleanup().await;
}
