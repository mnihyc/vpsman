use super::*;

const OLD_PING_POINTS: &str = r#"
    SELECT point.* FROM telemetry_ping_points point
    WHERE point.series_id = ANY($1::BIGINT[])
      AND point.bucket_start >= COALESCE(to_timestamp($2::DOUBLE PRECISION), '-infinity'::TIMESTAMPTZ)
      AND point.bucket_start <= COALESCE(to_timestamp($3::DOUBLE PRECISION), 'infinity'::TIMESTAMPTZ)
      AND ($4::INTEGER IS NULL OR point.bucket_secs = $4)
"#;
const SCOPED_PING_POINTS: &str = r#"
    SELECT point.* FROM telemetry_ping_points_source(
        $1::BIGINT[], to_timestamp($2::DOUBLE PRECISION),
        to_timestamp($3::DOUBLE PRECISION), $4::INTEGER
    ) point
"#;

async fn ping_points_json(
    pool: &PgPool,
    source: &str,
    series: &[i64],
    start: Option<i64>,
    end: Option<i64>,
    width: Option<i32>,
) -> Value {
    sqlx::query_scalar(&format!(
        "SELECT COALESCE(jsonb_agg(to_jsonb(point) ORDER BY point.series_id, point.bucket_secs, point.bucket_start), '[]'::JSONB) FROM ({source}) point"
    ))
    .bind(series)
    .bind(start)
    .bind(end)
    .bind(width)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn postgres_ping_history_scoped_source_preserves_rebased_corrections_and_coarse_overlap() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client = "ping-scoped-corrections";
    insert_client(&db.pool, client, None).await;
    let target = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ping_targets(id,name,host,probe_kind,selector_expression,generation) VALUES($1,'Scoped Ping','192.0.2.71','icmp','*',2)",
    ).bind(target).execute(&db.pool).await.unwrap();
    sqlx::query(
        "INSERT INTO ping_target_assignments(target_id,client_id,is_primary) VALUES($1,$2,TRUE)",
    )
    .bind(target)
    .bind(client)
    .execute(&db.pool)
    .await
    .unwrap();
    let active: i64 = sqlx::query_scalar(
        "INSERT INTO telemetry_ping_series(client_id,target_id,generation) VALUES($1,$2,2) RETURNING id",
    ).bind(client).bind(target).fetch_one(&db.pool).await.unwrap();
    let historical: i64 = sqlx::query_scalar(
        "INSERT INTO telemetry_ping_series(client_id,target_id,generation) VALUES($1,$2,1) RETURNING id",
    ).bind(client).bind(target).fetch_one(&db.pool).await.unwrap();
    let base = (crate::unix_now() / 300 * 300 - 900) as i64;
    sqlx::query(
        r#"
        INSERT INTO telemetry_ping_rollups(
            series_id,bucket_start,bucket_secs,sample_count,success_count,
            latency_sum_ms,latency_avg_ms,latency_min_ms,latency_max_ms,
            loss_ratio_avg,loss_ratio_sum,loss_ratio_max,latest_status,
            latest_reason,latest_checked_at
        )
        SELECT series_id,to_timestamp($3+offset_secs),width,1,1,
               latency,latency,latency,latency,0,0,0,'ok','retained',
               to_timestamp($3+offset_secs+width-1)
        FROM (VALUES
            ($1::BIGINT,0,60,500.0),($1::BIGINT,60,60,500.0),
            ($1::BIGINT,0,300,99.0),($2::BIGINT,60,60,111.0)
        ) point(series_id,offset_secs,width,latency)
    "#,
    )
    .bind(active)
    .bind(historical)
    .bind(base)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO telemetry_ping_facts(
            series_id,observed_at,evidence_id,source_checked_unix,checked_unix,
            status,latency_avg_ms,loss_ratio,reason
        )
        SELECT $1,to_timestamp($2+offset_secs),$3,$2+offset_secs,$2+offset_secs,
               'ok',offset_secs::DOUBLE PRECISION,0,'durable fact'
        FROM unnest(ARRAY[10,20]) offset_secs
    "#,
    )
    .bind(active)
    .bind(base)
    .bind(Uuid::new_v4())
    .execute(&db.pool)
    .await
    .unwrap();
    // The source identity stays in minute A; trusted checked time is rebased
    // into B. The later ordinal wins inside the second accepted envelope.
    for (sequence, checked, entries) in [(1_i64, base + 10, 1), (2, base + 70, 2)] {
        let sample = insert_projected_raw_telemetry_fixture(
            &db.pool,
            client,
            checked as u64,
            &AgentMetrics::default(),
            sequence,
            &[],
            &[],
        )
        .await;
        let pings = (0..entries)
            .map(|ordinal| {
                let down = sequence == 2 && ordinal == 1;
                json!({
                    "target_id":target, "generation":2, "checked_unix":checked,
                    "status":if down {"down"} else {"ok"},
                    "latency_avg_ms":if down {None} else {Some(30.0)},
                    "loss_ratio":if down {1.0} else {0.0},
                    "reason":if down {"rebased winner"} else {"superseded"}
                })
            })
            .collect::<Vec<_>>();
        sqlx::query(
            "UPDATE telemetry_samples SET payload=jsonb_set(payload,'{ping_results}',$2), ping_source_checked_unix=$3 WHERE id=$1",
        ).bind(sample).bind(json!(pings)).bind(vec![base+10;entries as usize])
            .execute(&db.pool).await.unwrap();
    }
    sqlx::query(
        "UPDATE telemetry_projection_heads SET accepted_seq=2,projected_seq=2 WHERE client_id=$1",
    )
    .bind(client)
    .execute(&db.pool)
    .await
    .unwrap();

    for ids in [
        vec![active],
        vec![historical],
        vec![active, historical],
        vec![],
    ] {
        for (start, end, width) in [
            (None, None, None),
            (Some(base), Some(base + 59), Some(60)),
            (Some(base + 60), Some(base + 119), Some(60)),
            (Some(base - 86400), Some(base + 90), None),
        ] {
            assert_eq!(
                ping_points_json(&db.pool, SCOPED_PING_POINTS, &ids, start, end, width).await,
                ping_points_json(&db.pool, OLD_PING_POINTS, &ids, start, end, width).await,
                "all-field canonical parity failed for {ids:?} {start:?} {end:?} {width:?}"
            );
        }
    }
    let null_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM telemetry_ping_points_source(NULL::BIGINT[])")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(null_count, 0);
    let before = db
        .repo
        .list_raw_ping_results(client, Some(base as u64), Some((base + 59) as u64), 16, 60)
        .await
        .unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].sample_count, 1);
    assert_eq!(
        before[0].latency_avg_ms,
        Some(20.0),
        "the other fact in the old touched minute must survive the correction"
    );
    let after = db
        .repo
        .list_raw_ping_results(
            client,
            Some((base + 61) as u64),
            Some((base + 90) as u64),
            16,
            60,
        )
        .await
        .unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].generation, 2);
    assert_eq!(after[0].sample_count, 1);
    assert_eq!(after[0].latest_status, "down");
    assert_eq!(after[0].latest_reason.as_deref(), Some("rebased winner"));
    let primary = db
        .repo
        .list_raw_primary_ping_results_for_clients(
            &[client.to_string()],
            (base + 61) as u64,
            (base + 90) as u64,
            16,
            60,
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(primary).unwrap(),
        serde_json::to_value(&after).unwrap()
    );
    let coarse = db
        .repo
        .list_ping_rollups(
            client,
            Some((base + 61) as u64),
            Some((base + 90) as u64),
            16,
            60,
        )
        .await
        .unwrap();
    assert_eq!(coarse.len(), 1);
    assert_eq!(coarse[0].bucket_secs, 300);
    assert_eq!(coarse[0].latency_avg_ms, Some(99.0));
    assert_eq!(
        crate::util::parse_timestamp_unix(&coarse[0].latest_checked_at),
        Some((base + 299) as u64)
    );
    // Assignment removal changes reader ownership, not generic source data.
    sqlx::query("DELETE FROM ping_target_assignments WHERE target_id=$1 AND client_id=$2")
        .bind(target)
        .bind(client)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(db
        .repo
        .list_raw_ping_results(client, None, None, 16, 60)
        .await
        .unwrap()
        .is_empty());
    db.cleanup().await;
}

fn ping_plan_nodes<'a>(node: &'a Value, out: &mut Vec<&'a Value>) {
    out.push(node);
    if let Some(children) = node.get("Plans").and_then(Value::as_array) {
        for child in children {
            ping_plan_nodes(child, out);
        }
    }
}

#[tokio::test]
async fn postgres_ping_history_scoped_source_bounds_one_owner_among_120_with_raw_suffix() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let prefix = "ping-read-owner-";
    sqlx::query(
        r#"
        INSERT INTO clients(id,display_name,public_key,status,internal_build_number,capabilities)
        SELECT $1||lpad(owner::TEXT,3,'0'),$1||owner,
               decode(md5($1||owner)||md5($1||owner||'-key'),'hex'),
               'online',1,'{}'::JSONB
        FROM generate_series(1,120) owner
    "#,
    )
    .bind(prefix)
    .execute(&db.pool)
    .await
    .unwrap();
    let target = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ping_targets(id,name,host,probe_kind,selector_expression) VALUES($1,'Owner Ping','192.0.2.72','icmp','*')",
    ).bind(target).execute(&db.pool).await.unwrap();
    sqlx::query(
        "INSERT INTO telemetry_ping_series(client_id,target_id,generation) SELECT id,$1,1 FROM clients WHERE id LIKE $2||'%'",
    ).bind(target).bind(prefix).execute(&db.pool).await.unwrap();
    let base = (crate::unix_now() / 60 * 60 - 900) as i64;
    sqlx::query(
        r#"
        INSERT INTO telemetry_ping_rollups(
            series_id,bucket_start,bucket_secs,sample_count,success_count,
            latency_sum_ms,latency_avg_ms,latency_min_ms,latency_max_ms,
            loss_ratio_avg,loss_ratio_sum,loss_ratio_max,latest_status,latest_checked_at
        )
        SELECT series.id,to_timestamp($1+minute_no*60),60,1,1,
               10,10,10,10,0,0,0,'ok',to_timestamp($1+minute_no*60+30)
        FROM telemetry_ping_series series
        CROSS JOIN (
            SELECT generate_series(-2880,-60,60) AS minute_no
            UNION ALL SELECT generate_series(0,15)
        ) minute
    "#,
    )
    .bind(base)
    .execute(&db.pool)
    .await
    .unwrap();
    for owner in 1..=120 {
        let client = format!("{prefix}{owner:03}");
        for (sequence, minute) in [(1, 0), (2, 15)] {
            let checked = (base + minute * 60 + 30) as u64;
            insert_projected_raw_telemetry_fixture(
                &db.pool,
                &client,
                checked,
                &AgentMetrics {
                    observed_unix: checked,
                    ping_results: vec![PingTargetResult {
                        target_id: target.to_string(),
                        generation: 1,
                        checked_unix: checked,
                        status: "ok".to_string(),
                        latency_avg_ms: Some(10.0),
                        loss_ratio: 0.0,
                        reason: None,
                    }],
                    ..Default::default()
                },
                sequence,
                &[],
                &[],
            )
            .await;
        }
    }
    sqlx::query(
        "UPDATE telemetry_projection_heads SET accepted_seq=2,projected_seq=2 WHERE client_id LIKE $1||'%'",
    ).bind(prefix).execute(&db.pool).await.unwrap();
    for relation in [
        "telemetry_ping_rollups",
        "telemetry_samples",
        "telemetry_ping_facts",
    ] {
        sqlx::query(&format!("ANALYZE {relation}"))
            .execute(&db.pool)
            .await
            .unwrap();
    }
    let series: i64 = sqlx::query_scalar(
        "SELECT id FROM telemetry_ping_series WHERE client_id=$1 AND target_id=$2",
    )
    .bind(format!("{prefix}001"))
    .bind(target)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let ids = vec![series];
    let (owned_fact_rows, touched_fact_minutes): (i64, i64) = sqlx::query_as(
        "SELECT count(*),count(DISTINCT checked_unix/60) FROM telemetry_ping_facts WHERE series_id=$1",
    )
    .bind(series)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!((owned_fact_rows, touched_fact_minutes), (2, 2));
    let old = ping_points_json(
        &db.pool,
        OLD_PING_POINTS,
        &ids,
        Some(base),
        Some(base + 900),
        Some(60),
    )
    .await;
    let new = ping_points_json(
        &db.pool,
        SCOPED_PING_POINTS,
        &ids,
        Some(base),
        Some(base + 900),
        Some(60),
    )
    .await;
    assert_eq!(old.as_array().unwrap().len(), 16);
    assert_eq!(new, old, "retained + raw all-field source parity");
    for mode in ["force_custom_plan", "force_generic_plan"] {
        let mut tx = db.pool.begin().await.unwrap();
        sqlx::query(&format!("SET LOCAL plan_cache_mode='{mode}'"))
            .execute(&mut *tx)
            .await
            .unwrap();
        for (label, source) in [
            ("existing view", OLD_PING_POINTS),
            ("scoped source", SCOPED_PING_POINTS),
        ] {
            let plan: Value =
                sqlx::query_scalar(&format!("EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) {source}"))
                    .bind(&ids)
                    .bind(Some(base))
                    .bind(Some(base + 900))
                    .bind(Some(60_i32))
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
            let root = &plan[0]["Plan"];
            if label == "scoped source" {
                let mut nodes = Vec::new();
                ping_plan_nodes(root, &mut nodes);
                let relation_work = |relation: &str| -> f64 {
                    nodes
                        .iter()
                        .filter(|node| node["Relation Name"].as_str() == Some(relation))
                        .map(|node| {
                            (node["Actual Rows"].as_f64().unwrap_or_default()
                                + node["Rows Removed by Filter"].as_f64().unwrap_or_default())
                                * node["Actual Loops"].as_f64().unwrap_or_default()
                        })
                        .sum()
                };
                assert!(
                    relation_work("telemetry_samples") <= 2.0,
                    "unrelated raw owners scanned: {plan}"
                );
                assert!(
                    // Each touched minute needs its own fact lookup. With
                    // only two owned facts, a valid series-only index probe
                    // may inspect both and filter one per minute. Bound the
                    // work by that exact fixture ownership, not an index-plan
                    // preference: unrelated series still cannot be scanned.
                    relation_work("telemetry_ping_facts")
                        <= (owned_fact_rows * touched_fact_minutes) as f64,
                    "unrelated fact owners scanned: {plan}"
                );
                for fact in nodes
                    .iter()
                    .filter(|node| node["Relation Name"].as_str() == Some("telemetry_ping_facts"))
                {
                    let condition = fact
                        .get("Index Cond")
                        .or_else(|| fact.get("Recheck Cond"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    assert!(
                        condition.contains("series_id ="),
                        "fact access lost its indexed exact-series owner: {fact}"
                    );
                }
                assert!(
                    relation_work("telemetry_ping_rollups") <= 16.0,
                    "aged/unrelated retained rows scanned: {plan}"
                );
                for retained in nodes
                    .iter()
                    .filter(|node| node["Relation Name"].as_str() == Some("telemetry_ping_rollups"))
                {
                    let condition = retained
                        .get("Index Cond")
                        .or_else(|| retained.get("Recheck Cond"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    assert!(
                        condition.contains("bucket_start >=")
                            && condition.contains("bucket_start <="),
                        "retained access did not use both physical time bounds: {retained}"
                    );
                }
                let expanded = nodes
                    .iter()
                    .find(|node| node["Subplan Name"].as_str() == Some("CTE expanded"))
                    .unwrap();
                assert_eq!(expanded["Actual Rows"].as_f64(), Some(2.0));
                assert_eq!(expanded["Actual Loops"].as_f64(), Some(1.0));
                let raw = nodes
                    .iter()
                    .find(|node| node["Subplan Name"].as_str() == Some("CTE raw_evidence"))
                    .unwrap();
                assert_eq!(raw["Actual Rows"].as_f64(), Some(2.0));
                assert_eq!(raw["Actual Loops"].as_f64(), Some(1.0));
            }
            eprintln!(
                "Ping source same-fixture A/B ({mode}, {label}): one requested owner among120, sparse2-day retained history,240 raw minutes; planning={}ms execution={}ms shared_read={} shared_hits={} temp_written={}; query only, not concurrent endpoint latency",
                plan[0]["Planning Time"],plan[0]["Execution Time"],
                root["Shared Read Blocks"],root["Shared Hit Blocks"],root["Temp Written Blocks"]
            );
        }
        tx.rollback().await.unwrap();
    }
    db.cleanup().await;
}
