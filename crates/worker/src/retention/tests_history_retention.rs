use super::{
    coalesce_ready_telemetry_due_events, database_deadline, process_telemetry_history_retention,
    promote_network_rate_rollups, promote_network_rate_tier, promote_ping_rollups,
    promote_ping_tier, promote_ping_tier_in_tx, promote_resource_rollups, promote_resource_tier,
    promote_system_metric_rollups, prune_domain, prune_domain_has_remaining_work, prune_query,
    sample_prune_owner_query, sample_prune_query, RetentionDeadline, RetentionNextAt,
    RetentionOwnerState, RetentionOwnerStatus, RetentionPhase, RetentionPolicy,
    TelemetryHistoryRetentionDrain, TelemetryHistoryRetentionStep, WakeContract,
    EXTERNAL_WRITER_FRONTIERS, NETWORK_RATE_ROLLUP_DOMAIN, PING_ROLLUP_DOMAIN,
    RESOURCE_ROLLUP_DOMAIN, RETENTION_PHASES,
};
use crate::network_observation_retention::NetworkObservationRetentionPhase;
use crate::test_support::PgWorkerTestDb;
use crate::traffic_retention::TrafficRetentionPhase;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde_json::Value;
use sqlx::PgPool;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Barrier;
use vpsman_common::TELEMETRY_HISTORY_TIERS;

async fn coalesce_until_runnable_span(pool: &PgPool, domain: &str) {
    loop {
        let runnable: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM telemetry_history_due_spans
                WHERE domain = $1
                  AND due_at <= clock_timestamp()
            )
            "#,
        )
        .bind(domain)
        .fetch_one(pool)
        .await
        .unwrap();
        if runnable {
            return;
        }

        let coalescing = coalesce_ready_telemetry_due_events(pool).await.unwrap();
        assert!(
            coalescing.coalesced > 0,
            "producer evidence did not reach the {domain} retention owner"
        );
    }
}

#[tokio::test]
async fn tier_promotion_replaces_natural_spans_and_preserves_counts() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ('compaction-fairness', 'compaction-fairness', decode('', 'hex'), 'online')",
    )
    .execute(&db.pool)
    .await
    .unwrap();

    sqlx::query(
        r#"
        INSERT INTO telemetry_network_rates (
            client_id, interface, bucket_start, bucket_secs, sample_count,
            rx_bytes_sum, tx_bytes_sum, rx_bytes_avg, tx_bytes_avg,
            rx_bytes_last, tx_bytes_last, rx_counter_epoch, tx_counter_epoch,
            latest_observed_at
        ) SELECT 'compaction-fairness', 'eth0', bucket_start, 60, 1,
            rx, tx, rx, tx, rx, tx, 0, 0, bucket_start
        FROM (VALUES
            (date_trunc('hour', now()) - interval '40 days', 100::bigint, 200::bigint),
            (date_trunc('hour', now()) - interval '40 days' + interval '1 minute', 300::bigint, 500::bigint)
        ) sample(bucket_start, rx, tx)
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO ping_targets (id, name, host, probe_kind) VALUES ('10000000-0000-0000-0000-000000000001', 'tier-test', '127.0.0.1', 'icmp')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO telemetry_ping_series (client_id, target_id, generation) VALUES (
            'compaction-fairness',
            '10000000-0000-0000-0000-000000000001',
            1
        )
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO telemetry_ping_rollups (
            series_id, bucket_start, bucket_secs, sample_count, success_count,
            latency_sum_ms, latency_avg_ms, latency_min_ms, latency_max_ms,
            loss_ratio_avg, loss_ratio_sum, loss_ratio_max,
            latest_status, latest_checked_at
        ) SELECT series.id, sample.bucket_start, 60, 1, 1,
            latency, latency, latency, latency, loss, loss, loss,
            CASE WHEN loss = 0 THEN 'ok' ELSE 'degraded' END,
            sample.bucket_start
        FROM telemetry_ping_series series
        CROSS JOIN (VALUES
            (date_trunc('hour', now()) - interval '40 days', 10.0::double precision, 0.0::double precision),
            (date_trunc('hour', now()) - interval '40 days' + interval '1 minute', 20.0::double precision, 0.5::double precision)
        ) sample(bucket_start, latency, loss)
        WHERE series.client_id = 'compaction-fairness'
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ('compaction-offset', 'compaction-offset', decode('', 'hex'), 'online')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO telemetry_rollups (
            client_id, bucket_start, bucket_secs, sample_count,
            cpu_usage_sample_count, cpu_usage_avg, cpu_usage_max,
            cpu_cores_max, cpu_load_1_avg, cpu_load_1_sum, cpu_load_1_max,
            cpu_load_5_avg, cpu_load_5_sum, cpu_load_5_max, cpu_load_15_avg,
            cpu_load_15_sum, cpu_load_15_max, memory_total_bytes_max,
            memory_available_bytes_avg, memory_available_bytes_sum,
            memory_available_bytes_min, memory_used_ratio_avg,
            memory_used_ratio_sum, memory_used_ratio_max,
            swap_sample_count, swap_total_bytes_max,
            swap_available_bytes_avg, swap_available_bytes_sum,
            swap_available_bytes_min, swap_used_ratio_avg,
            swap_used_ratio_sum, swap_used_ratio_max,
            disk_sample_count,
            disk_total_bytes_max, disk_available_bytes_avg,
            disk_available_bytes_sum, disk_available_bytes_min,
            disk_used_ratio_avg, disk_used_ratio_sum, disk_used_ratio_max,
            latest_observed_at
        )
        SELECT
            'compaction-fairness',
            date_trunc('hour', now()) - interval '96 hours'
                + series.minute_index * interval '1 minute',
            60, 1, 0, NULL, NULL, 1,
            CASE WHEN series.minute_index < 2000
                THEN (series.minute_index % 2)::double precision
                ELSE 9::double precision
            END,
            CASE WHEN series.minute_index < 2000
                THEN (series.minute_index % 2)::double precision
                ELSE 9::double precision
            END,
            CASE WHEN series.minute_index < 2000
                THEN (series.minute_index % 2)::double precision
                ELSE 9::double precision
            END,
            0, 0, 0, 0, 0, 0, 1024, 512, 512, 512, 0.5, 0.5, 0.5,
            1, 1024, 512, 512, 512, 0.5, 0.5, 0.5,
            1, 2048, 1024, 1024, 1024, 0.5, 0.5, 0.5,
            date_trunc('hour', now()) - interval '96 hours'
                + series.minute_index * interval '1 minute'
        FROM generate_series(0, 2003) AS series(minute_index)
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE telemetry_rollups
        SET
            disk_sample_count = 0,
            disk_total_bytes_max = 999999,
            disk_available_bytes_avg = 999999,
            disk_available_bytes_sum = 999999,
            disk_available_bytes_min = 999999,
            disk_used_ratio_avg = 0.99,
            disk_used_ratio_sum = 0.99,
            disk_used_ratio_max = 0.99
        WHERE ctid = (
            SELECT ctid FROM telemetry_rollups
            WHERE client_id = 'compaction-fairness'
            ORDER BY bucket_start
            LIMIT 1
        )
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO telemetry_rollups (
            client_id, bucket_start, bucket_secs, sample_count,
            cpu_usage_sample_count, cpu_usage_avg, cpu_usage_max,
            cpu_cores_max, cpu_load_1_avg, cpu_load_1_sum, cpu_load_1_max,
            cpu_load_5_avg, cpu_load_5_sum, cpu_load_5_max, cpu_load_15_avg,
            cpu_load_15_sum, cpu_load_15_max, memory_total_bytes_max,
            memory_available_bytes_avg, memory_available_bytes_sum,
            memory_available_bytes_min, memory_used_ratio_avg,
            memory_used_ratio_sum, memory_used_ratio_max,
            disk_sample_count,
            disk_total_bytes_max, disk_available_bytes_avg,
            disk_available_bytes_sum, disk_available_bytes_min,
            disk_used_ratio_avg, disk_used_ratio_sum, disk_used_ratio_max,
            latest_observed_at
        )
        SELECT
            'compaction-offset',
            date_trunc('minute', now()) - interval '40 hours'
                + series.minute_index * interval '1 minute',
            60, 1, 0, NULL, NULL, 1, 4, 4, 4, 0, 0, 0, 0, 0, 0,
            1024, 512, 512, 512, 0.5, 0.5, 0.5,
            1, 2048, 1024, 1024, 1024, 0.5, 0.5, 0.5,
            date_trunc('minute', now()) - interval '40 hours'
                + series.minute_index * interval '1 minute'
                + CASE series.minute_index WHEN 0 THEN interval '5 seconds'
                    ELSE interval '17 seconds' END
        FROM generate_series(0, 1) AS series(minute_index)
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    sqlx::query(
        r#"
        INSERT INTO telemetry_ping_current (
            series_id, latest_status, latency_avg_ms, rolling_loss_ratio,
            latest_reason, latest_checked_at
        )
        SELECT series.id, 'degraded', 20, 0.25, 'retained exact current',
            date_trunc('hour', now()) - interval '40 days' + interval '1 minute'
        FROM telemetry_ping_series series
        WHERE series.client_id = 'compaction-fairness'
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let lower_tiers = [60, 300, 1_800];
    let limited_ping = promote_ping_tier(&db.pool, 3_600, 1_800, 1, &lower_tiers)
        .await
        .unwrap();
    for limited in [
        promote_resource_tier(&db.pool, 3_600, 1_800, 1, &lower_tiers)
            .await
            .unwrap(),
        promote_network_rate_tier(&db.pool, 3_600, 1_800, 1, &lower_tiers)
            .await
            .unwrap(),
        limited_ping.promotion,
    ] {
        assert_eq!(limited.promoted, 0);
        assert_eq!(limited.source_rows, 0);
    }

    let mut resource_spans_merged = 0;
    // Exercise the producer evidence, global coalescer, and exact domain owner
    // for three natural spans. Global event order is intentionally independent
    // of owner order, so a fixed number of whole registry rotations is not a
    // deterministic assertion of any one owner's progress.
    for _ in 0..3 {
        coalesce_until_runnable_span(&db.pool, RESOURCE_ROLLUP_DOMAIN).await;
        let resource = promote_resource_rollups(&db.pool).await.unwrap();
        assert_eq!(resource.promotion.promoted, 1);
        resource_spans_merged += resource.promotion.promoted;

        coalesce_until_runnable_span(&db.pool, NETWORK_RATE_ROLLUP_DOMAIN).await;
        let network = promote_network_rate_rollups(&db.pool).await.unwrap();
        assert_eq!(network.promotion.promoted, 1);

        coalesce_until_runnable_span(&db.pool, PING_ROLLUP_DOMAIN).await;
        let ping = promote_ping_rollups(&db.pool).await.unwrap();
        assert_eq!(ping.promotion.promoted, 1);
    }
    assert_eq!(resource_spans_merged, 3);
    let retained: (i64, i64) = sqlx::query_as(
        r#"
        SELECT count(*), COALESCE(sum(sample_count), 0)::bigint
        FROM telemetry_rollups
        WHERE client_id = 'compaction-fairness'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(retained, (1992, 2004));
    let retained_swap_samples: i64 = sqlx::query_scalar(
        "SELECT COALESCE(sum(swap_sample_count), 0)::bigint FROM telemetry_rollups WHERE client_id = 'compaction-fairness'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(retained_swap_samples, 2004);
    let retained_disk: (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(sum(disk_sample_count), 0)::bigint, max(disk_total_bytes_max) FILTER (WHERE disk_sample_count > 0)::bigint FROM telemetry_rollups WHERE client_id = 'compaction-fairness'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(retained_disk, (2003, 2048));
    let incomplete_swap_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM telemetry_rollups WHERE client_id = 'compaction-fairness' AND (swap_sample_count = 0 OR swap_total_bytes_max IS NULL OR swap_available_bytes_avg IS NULL OR swap_available_bytes_min IS NULL OR swap_used_ratio_avg IS NULL OR swap_used_ratio_max IS NULL)",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(incomplete_swap_rows, 0);
    let differing_offsets_retained: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM telemetry_rollups WHERE client_id = 'compaction-offset'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(differing_offsets_retained, 2);
    let resource_tier_rows: (i64, i64) = sqlx::query_as(
        r#"
        SELECT count(*) FILTER (WHERE bucket_secs = 60),
               count(*) FILTER (WHERE bucket_secs = 300)
        FROM telemetry_rollups
        WHERE client_id = 'compaction-fairness'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(resource_tier_rows, (1989, 3));
    let network: (i32, i32, i64, i64) = sqlx::query_as(
        "SELECT bucket_secs, sample_count, rx_bytes_avg, tx_bytes_avg FROM telemetry_network_rates WHERE client_id = 'compaction-fairness' AND interface = 'eth0'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(network, (3600, 2, 200, 350));
    let ping: (i32, i32, i32, f64, f64) = sqlx::query_as(
        "SELECT bucket_secs, sample_count, success_count, latency_avg_ms, loss_ratio_avg FROM telemetry_ping_rollups",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(ping, (3600, 2, 2, 15.0, 0.25));
    let resource_latest: (i32, f64) = sqlx::query_as(
        "SELECT sample_count, cpu_load_1_avg FROM telemetry_resource_points_source(ARRAY['compaction-fairness']) ORDER BY latest_observed_at DESC LIMIT 1",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(resource_latest, (1, 9.0));
    let ping_current: (String, Option<f64>, f64, Option<String>) = sqlx::query_as(
        "SELECT latest_status, latency_avg_ms, rolling_loss_ratio, latest_reason FROM telemetry_ping_current",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        ping_current,
        (
            "degraded".to_string(),
            Some(20.0),
            0.25,
            Some("retained exact current".to_string()),
        )
    );
    db.cleanup().await;
}

#[tokio::test]
async fn ping_promotion_obeys_ingest_parent_before_child_lock_order() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ('ping-lock-order', 'ping-lock-order', decode('', 'hex'), 'online')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO ping_targets (id, name, host, probe_kind) VALUES ('20000000-0000-0000-0000-000000000001', 'ping-lock-order', '127.0.0.1', 'icmp')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let series_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO telemetry_ping_series (client_id, target_id, generation)
        VALUES (
            'ping-lock-order',
            '20000000-0000-0000-0000-000000000001',
            1
        )
        RETURNING id
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO telemetry_ping_rollups (
            series_id, bucket_start, bucket_secs, sample_count, success_count,
            latency_sum_ms, latency_avg_ms, latency_min_ms, latency_max_ms,
            loss_ratio_avg, loss_ratio_sum, loss_ratio_max,
            latest_status, latest_checked_at
        )
        SELECT $1, sample.bucket_start, 60, 1, 1,
            10, 10, 10, 10, 0, 0, 0, 'ok', sample.bucket_start
        FROM (
            SELECT date_trunc('hour', now()) - interval '40 days'
                + minute_index * interval '1 minute' AS bucket_start
            FROM generate_series(0, 1) AS minute(minute_index)
        ) sample
        "#,
    )
    .bind(series_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let mut ingest = db.pool.begin().await.unwrap();
    sqlx::query_scalar::<_, i64>("SELECT id FROM telemetry_ping_series WHERE id = $1 FOR UPDATE")
        .bind(series_id)
        .fetch_one(&mut *ingest)
        .await
        .unwrap();

    let promotion_pool = db.pool.clone();
    let mut promotion =
        tokio::spawn(async move { promote_ping_tier(&promotion_pool, 300, 60, 1, &[60]).await });
    let skipped = match tokio::time::timeout(Duration::from_secs(5), &mut promotion).await {
        Ok(joined) => joined
            .expect("Ping promotion task should not panic")
            .expect("Ping promotion should skip a parent held by ingest"),
        Err(_) => {
            promotion.abort();
            let _ = promotion.await;
            ingest.rollback().await.unwrap();
            db.cleanup().await;
            panic!("Ping promotion waited after locking child rows");
        }
    };
    assert_eq!(skipped.promotion.promoted, 0);
    assert_eq!(skipped.promotion.source_rows, 0);
    assert!(!skipped.complete);

    tokio::time::timeout(
        Duration::from_secs(5),
        sqlx::query("UPDATE telemetry_ping_rollups SET updated_at = now() WHERE series_id = $1")
            .bind(series_id)
            .execute(&mut *ingest),
    )
    .await
    .expect("ingest must acquire child rows after its parent without a lock cycle")
    .unwrap();
    ingest.rollback().await.unwrap();

    let mut child_holder = db.pool.begin().await.unwrap();
    sqlx::query(
        r#"
        SELECT series_id
        FROM telemetry_ping_rollups
        WHERE series_id = $1 AND bucket_secs = 60
        ORDER BY bucket_start
        LIMIT 1
        FOR UPDATE
        "#,
    )
    .bind(series_id)
    .fetch_one(&mut *child_holder)
    .await
    .unwrap();
    let child_skipped = promote_ping_tier(&db.pool, 300, 60, 1, &[60])
        .await
        .unwrap();
    assert_eq!(child_skipped.promotion.promoted, 0);
    assert_eq!(child_skipped.promotion.examined_source_rows, 2);
    assert_eq!(child_skipped.promotion.source_rows, 0);
    assert!(!child_skipped.complete);
    child_holder.rollback().await.unwrap();

    let barrier = Arc::new(Barrier::new(2));
    let promote = |pool: sqlx::PgPool, barrier: Arc<Barrier>| async move {
        barrier.wait().await;
        promote_ping_tier(&pool, 300, 60, 1, &[60]).await.unwrap()
    };
    let (left, right) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(
            promote(db.pool.clone(), barrier.clone()),
            promote(db.pool.clone(), barrier)
        )
    })
    .await
    .expect("dual Ping promoters must not deadlock");
    assert_eq!(left.promotion.promoted + right.promotion.promoted, 1);
    assert_eq!(left.promotion.source_rows + right.promotion.source_rows, 2);
    let rows_after_promotion: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            count(*) FILTER (WHERE bucket_secs = 60),
            count(*) FILTER (WHERE bucket_secs = 300)
        FROM telemetry_ping_rollups
        WHERE series_id = $1
        "#,
    )
    .bind(series_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(rows_after_promotion, (0, 1));

    db.cleanup().await;
}

#[tokio::test]
async fn network_promotion_queues_exact_dashboard_coordinates() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "network-compaction-provenance";
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1, $1, decode('', 'hex'), 'online')",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO telemetry_dashboard_network_generations (
            client_id, generation, select_all, interfaces, interface_width
        ) VALUES ($1, 2, true, ARRAY['eth0'], 1)
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE telemetry_dashboard_network_projection_heads
        SET network_generation = 2,
            network_select_all = true,
            network_generation_interfaces = ARRAY['eth0'],
            network_interface_width = 1
        WHERE client_id = $1
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch,
            sample_source
        )
        SELECT $1, 'host', 'eth0', anchor.bucket_start + sample.minute_offset,
               sample.rx_bytes, sample.tx_bytes, 0, 0, 'agent_networks'
        FROM (
            SELECT date_trunc('hour', now()) - interval '40 days' AS bucket_start
        ) anchor
        CROSS JOIN (VALUES
            (interval '0 minutes', 100::bigint, 200::bigint),
            (interval '1 minute', 300::bigint, 500::bigint)
        ) sample(minute_offset, rx_bytes, tx_bytes)
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    // This test starts at the established day-one ownership boundary: the
    // traffic owner retained only its sequencing evidence, while the exact
    // minute coordinates moved to the ordinary network history owner.
    sqlx::query(
        r#"
        INSERT INTO telemetry_network_rates (
            client_id, interface, bucket_start, bucket_secs,
            sample_count, rx_bytes_sum, tx_bytes_sum,
            rx_bytes_avg, tx_bytes_avg, rx_bytes_last, tx_bytes_last,
            rx_counter_epoch, tx_counter_epoch,
            latest_observed_at, updated_at
        )
        SELECT
            client_id, interface, observed_at, 60,
            sample_count, rx_bytes_sum, tx_bytes_sum,
            round(rx_bytes_sum / sample_count::numeric)::bigint,
            round(tx_bytes_sum / sample_count::numeric)::bigint,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch,
            latest_observed_at, updated_at
        FROM traffic_counter_samples
        WHERE client_id = $1 AND source_kind = 'host'
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE traffic_counter_samples SET inbound_promoted = TRUE WHERE client_id = $1")
        .bind(client_id)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(coalesce_all_due_events(&db.pool).await, 1);

    sqlx::query(
        "DELETE FROM telemetry_dashboard_block_events WHERE client_id = $1 AND domain = 'network'",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let promoted = promote_network_rate_rollups(&db.pool).await.unwrap();
    assert_eq!(
        (promoted.promotion.promoted, promoted.promotion.source_rows),
        (1, 2)
    );
    assert!(!promoted.has_remaining_work);

    let queued_work: Vec<(i32, String, i64, bool)> = sqlx::query_as(
        r#"
        SELECT source_bucket_secs, event_kind, count(*),
               bool_and(bucket_start_unix IS NOT NULL)
        FROM telemetry_dashboard_block_events
        WHERE client_id = $1 AND domain = 'network'
        GROUP BY source_bucket_secs, event_kind
        ORDER BY source_bucket_secs, event_kind
        "#,
    )
    .bind(client_id)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        queued_work,
        vec![
            (60, "coordinate".to_string(), 2, true),
            (300, "coordinate".to_string(), 1, true),
        ]
    );

    db.cleanup().await;
}

#[test]
fn resource_and_network_promotions_are_exact_and_only_prune_requests_full_blocks() {
    let worker = include_str!("history_retention.rs");
    let marker = "vpsman.telemetry_history_compaction";
    assert_eq!(worker.matches(marker).count(), 1);

    let resource = worker
        .split_once("async fn promote_resource_tier_in_tx")
        .unwrap()
        .1
        .split_once("async fn promote_network_rate_rollups")
        .unwrap()
        .0;
    let network = worker
        .split_once("async fn promote_network_rate_tier_in_tx")
        .unwrap()
        .1
        .split_once("async fn promote_ping_rollups")
        .unwrap()
        .0;
    let prune = worker
        .split_once("async fn prune_domain(")
        .unwrap()
        .1
        .split_once("async fn prune_domain_has_remaining_work")
        .unwrap()
        .0;
    assert!(!resource.contains(marker));
    assert!(!network.contains(marker));
    assert_eq!(prune.matches(marker).count(), 1);
    assert!(prune.contains("matches!(domain, \"telemetry_rollups\" | \"telemetry_network_rates\")"));

    let ping = worker
        .split_once("async fn promote_ping_tier_in_tx")
        .unwrap()
        .1
        .split_once("async fn promote_system_metric_rollups")
        .unwrap()
        .0;
    assert!(!ping.contains(marker));
}

#[test]
fn every_network_rate_partition_has_its_retention_frontier_index() {
    let migration = include_str!("../../../../migrations/0003_telemetry_core.sql");
    for partition in ["minute", "coarse"] {
        let expected = format!(
            "CREATE INDEX telemetry_network_rates_{partition}_retention_idx ON public.telemetry_network_rates_{partition} USING btree (bucket_start);"
        );
        assert!(
            migration.contains(&expected),
            "the {partition} network-rate partition cannot prove/prune its bucket_start retention frontier without scanning retained history"
        );
    }
}

#[test]
fn due_event_producers_never_touch_the_unique_span_owner() {
    let migration = include_str!("../../../../migrations/0003_telemetry_core.sql");
    let start = migration
        .find("CREATE FUNCTION public.enqueue_telemetry_history_due_events()")
        .unwrap();
    let end = migration[start..].find("\n$$;").unwrap() + start;
    let producer = &migration[start..end];
    assert!(producer.contains("INSERT INTO public.telemetry_history_due_events"));
    assert!(producer.contains("SELECT DISTINCT"));
    assert!(producer.contains("'system_metric_rollups'::text"));
    assert!(producer.contains("AS coalesce_ready_at"));
    assert!(producer.contains("END AS owner_identity"));
    assert!(producer.contains("ARRAY[to_jsonb(row) ->> 'client_id']"));
    assert!(producer.contains("to_jsonb(row) ->> 'interface'"));
    assert!(producer.contains("ARRAY[to_jsonb(row) ->> 'series_id']"));
    assert!(producer.contains("ARRAY[to_jsonb(row) ->> 'metric']"));
    assert!(producer.contains("date_bin("));
    assert!(producer.contains("phase.destination_bucket_secs"));
    assert!(producer.contains("SELECT min(coalesce_ready_at)"));
    assert!(producer.contains("'vpsman_telemetry_retention'"));
    assert!(producer.contains("'effect', 'ordinary_rollup_published'"));
    assert!(producer.contains("'domain', TG_ARGV[0]"));
    assert!(producer.contains("jsonb_strip_nulls"));
    assert!(producer.contains("'ready_at_unix'"));
    assert_eq!(producer.matches("PERFORM pg_notify(").count(), 1);
    assert!(!producer.contains("IF earliest_coalesce_ready_at IS NOT NULL"));
    assert!(!producer.contains("vpsman.network_observation_promotion"));
    assert!(producer.contains("row.bucket_secs = phase.source_bucket_secs"));
    assert!(!producer.contains("telemetry_history_due_spans"));
    assert!(!producer.contains("ON CONFLICT"));
    assert!(!producer.contains("FOR KEY SHARE"));
    assert!(migration.contains("CREATE INDEX telemetry_history_due_events_coordinate_ready_idx"));
    assert!(migration.contains("destination_start, owner_identity, coalesce_ready_at, event_id"));

    let worker = include_str!("history_retention.rs");
    let start = worker
        .find("async fn coalesce_ready_telemetry_due_events(")
        .unwrap();
    let end = worker[start..]
        .find("async fn claim_ordinary_due_span(")
        .unwrap()
        + start;
    let coalescer = &worker[start..end];
    assert!(coalescer.contains("FROM telemetry_history_due_events"));
    assert_eq!(worker.matches("coalesce_ready_at <= now()").count(), 3);
    assert_eq!(coalescer.matches("coalesce_ready_at <= now()").count(), 2);
    assert!(coalescer.contains("WITH seed AS MATERIALIZED"));
    assert!(coalescer.contains("ready_events AS MATERIALIZED"));
    assert!(coalescer.contains("coordinates AS MATERIALIZED"));
    assert!(coalescer.contains("GROUP BY domain, source_bucket_secs"));
    assert!(coalescer.contains("owner_identity, destination_start, due_at"));
    assert!(coalescer.contains("ORDER BY coalesce_ready_at, event_id"));
    assert!(coalescer.contains("seed.owner_identity = event.owner_identity"));
    assert!(coalescer.contains("FOR UPDATE SKIP LOCKED"));
    assert!(coalescer.contains("INSERT INTO telemetry_history_due_spans AS current"));
    assert!(coalescer.contains("DO UPDATE SET due_at = current.due_at"));
    assert!(coalescer.contains("WHERE FALSE"));
    assert!(coalescer.contains("DELETE FROM telemetry_history_due_events"));
    assert!(coalescer.contains("event.event_id = ready.event_id"));
    assert_eq!(coalescer.matches("LIMIT 1").count(), 1);
}

#[tokio::test]
async fn ordinary_rollup_trigger_publishes_one_exact_prune_and_optional_due_deadline() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let mut listener = db.notification_listener().await.unwrap();
    listener.listen("vpsman_telemetry_retention").await.unwrap();
    let minute = sqlx::query_scalar::<_, DateTime<Utc>>(
        "SELECT date_trunc('minute', clock_timestamp()) - interval '1 hour'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    insert_system_point(&db.pool, "notification.minute", minute, 60, 1.0).await;
    let notification = tokio::time::timeout(Duration::from_secs(2), listener.recv())
        .await
        .expect("60-second publication notification timeout")
        .unwrap();
    let payload: Value = serde_json::from_str(notification.payload()).unwrap();
    assert_eq!(notification.channel(), "vpsman_telemetry_retention");
    assert_eq!(payload["owner"], "history_retention");
    assert_eq!(payload["effect"], "ordinary_rollup_published");
    assert_eq!(payload["domain"], "system_metric_rollups");
    assert!(payload["ready_at_unix"].as_i64().is_some());

    let day = sqlx::query_scalar::<_, DateTime<Utc>>(
        "SELECT date_trunc('day', clock_timestamp()) - interval '10 days'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    insert_system_point(&db.pool, "notification.day", day, 86_400, 2.0).await;
    let notification = tokio::time::timeout(Duration::from_secs(2), listener.recv())
        .await
        .expect("terminal publication notification timeout")
        .unwrap();
    let payload: Value = serde_json::from_str(notification.payload()).unwrap();
    assert_eq!(payload["effect"], "ordinary_rollup_published");
    assert_eq!(payload["domain"], "system_metric_rollups");
    assert!(payload.get("ready_at_unix").is_none());
    db.cleanup().await;
}

#[test]
fn ordinary_promotions_claim_and_scan_one_exact_natural_owner() {
    let source = include_str!("history_retention.rs");

    let claim_start = source.find("async fn claim_ordinary_due_span(").unwrap();
    let claim_end = source[claim_start..]
        .find("async fn ordinary_due_spans_have_remaining_work(")
        .unwrap()
        + claim_start;
    let claim = &source[claim_start..claim_end];
    assert!(claim.contains("FROM telemetry_history_due_spans"));
    assert!(claim.contains("due_at <= now()"));
    assert!(claim.contains("owner_identity, destination_start"));
    assert!(claim.contains("ORDER BY due_at, source_bucket_secs, destination_bucket_secs"));
    assert_eq!(claim.matches("LIMIT 1").count(), 1);
    assert_eq!(claim.matches("FOR UPDATE SKIP LOCKED").count(), 1);
    for table in [
        "telemetry_rollups",
        "telemetry_network_rates",
        "telemetry_ping_rollups",
        "system_metric_rollups",
    ] {
        assert!(!claim.contains(table));
    }

    for (start, end, domain, owner_predicate) in [
        (
            "async fn promote_resource_tier_in_tx(",
            "async fn promote_network_rate_rollups(",
            "resource",
            "row.client_id = $5",
        ),
        (
            "async fn promote_network_rate_tier_in_tx(",
            "async fn promote_ping_rollups(",
            "network rate",
            "row.interface = $6",
        ),
        (
            "async fn promote_ping_tier_in_tx(",
            "async fn promote_system_metric_rollups(",
            "Ping",
            "row.series_id = $5",
        ),
        (
            "async fn promote_system_metric_tier_in_tx(",
            "fn reject_promotion_conflicts(",
            "system metric",
            "row.metric = $5",
        ),
    ] {
        let body_start = source.find(start).unwrap();
        let body_end = source[body_start..].find(end).unwrap() + body_start;
        let body = &source[body_start..body_end];
        assert_eq!(
            body.matches("span_rows AS MATERIALIZED").count(),
            1,
            "{domain} must read one natural destination span once",
        );
        assert!(body.contains("row.bucket_secs = ANY($4::integer[])"));
        assert!(body.contains(owner_predicate));
        assert!(body.contains("row.bucket_start >= $3::timestamptz"));
        assert!(body.contains("row.bucket_start < $3::timestamptz"));
        assert!(body.contains("FROM span_rows row"));
        assert!(body.contains("bool_or(row.bucket_secs = $2) OVER"));
        assert!(body.contains("candidate_rows AS MATERIALIZED"));
        assert!(body.contains("count(*) OVER"));
        assert!(body.contains("bool_and("));
        assert!(body.contains("group_complete"));
        assert!(body.contains("inserted_succeeded"));
        assert!(body.contains("FOR UPDATE OF row SKIP LOCKED"));
        assert!(body.contains("immediate_source_rows"));
        assert!(body.contains("deleted_immediate_source_rows"));
        assert!(!body.contains("candidate_keys AS MATERIALIZED"));
        assert!(!body.contains("expanded_rows AS MATERIALIZED"));
        assert!(!body.contains("lockable_groups AS MATERIALIZED"));
        assert!(!body.contains("complete_groups AS"));
        assert!(!body.contains("LATERAL"));
        assert!(!body.contains("seed_rows"));
        assert!(!body.contains("candidate_frontiers"));
        assert!(!body.contains("LIMIT $"));
        assert!(!body.contains("history_retention_scan_cursors"));
    }

    let ping_start = source.find("async fn promote_ping_tier_in_tx(").unwrap();
    let ping_end = source[ping_start..]
        .find("async fn promote_system_metric_rollups(")
        .unwrap()
        + ping_start;
    let ping = &source[ping_start..ping_end];
    let parent = ping
        .find("locked_candidate_series AS MATERIALIZED")
        .unwrap();
    let child_lock = ping.find("locked_source AS MATERIALIZED").unwrap();
    assert!(parent < child_lock);
    assert!(ping[parent..child_lock].contains("FOR NO KEY UPDATE OF series SKIP LOCKED"));
}

#[test]
fn canonical_tier_schemas_reject_unknown_or_misaligned_widths() {
    let telemetry = include_str!("../../../../migrations/0003_telemetry_core.sql");
    for table in ["telemetry_ping_rollups", "telemetry_rollups"] {
        let start = telemetry
            .find(&format!("CREATE TABLE public.{table} ("))
            .unwrap();
        let body = &telemetry[start..telemetry[start..].find("\n);\n").unwrap() + start];
        assert!(body.contains("= ANY (ARRAY[60, 300, 1800, 3600, 10800, 21600, 86400])"));
        assert!(body.contains("mod(extract(epoch FROM"));
    }

    assert!(!telemetry.contains("telemetry_resource_active"));
    assert!(!telemetry.contains("telemetry_ping_active"));
    assert!(!telemetry.contains("telemetry_network_detail_current"));
    assert!(telemetry.contains("CREATE FUNCTION public.telemetry_resource_points_source("));
    assert!(telemetry.contains("CREATE VIEW public.telemetry_ping_points AS"));

    let network_start = telemetry
        .find("CREATE TABLE public.telemetry_network_rates (")
        .unwrap();
    let network_end = telemetry[network_start..]
        .find("\n) PARTITION BY LIST (bucket_secs);\n")
        .unwrap()
        + network_start;
    let network = &telemetry[network_start..network_end];
    assert!(network.contains("= ANY (ARRAY[60, 300, 1800, 3600, 10800, 21600, 86400])"));
    assert!(network.contains("mod(extract(epoch FROM bucket_start)::bigint, bucket_secs) = 0"));
    assert!(telemetry.contains("CREATE FUNCTION public.telemetry_network_rate_points_source("));
    assert!(telemetry.contains("FROM public.traffic_counter_samples sample"));
    assert!(telemetry.contains("AND NOT sample.inbound_promoted"));

    let system = include_str!("../../../../migrations/0010_system_metrics.sql");
    let start = system
        .find("CREATE TABLE public.system_metric_rollups (")
        .unwrap();
    let body = &system[start..system[start..].find("\n);\n").unwrap() + start];
    assert!(body.contains("= ANY (ARRAY[60, 300, 1800, 3600, 10800, 21600, 86400])"));
    assert!(body.contains("mod(extract(epoch FROM bucket_start)::bigint, bucket_secs) = 0"));
}

#[test]
fn supported_promotions_reject_conflicts_before_cursor_commit() {
    let source = include_str!("history_retention.rs");
    assert_eq!(source.matches("reject_promotion_conflicts(").count(), 5);
    let rejection = source.find("fn reject_promotion_conflicts(").unwrap();
    let body = &source[rejection..];
    assert!(body.contains("conflicts == 0"));
    assert!(!body.contains("retained sources because destination"));
}

#[test]
fn ordinary_remaining_work_probes_only_the_due_ledger() {
    let source = include_str!("history_retention.rs");
    let start = source
        .find("async fn ordinary_due_spans_have_remaining_work(")
        .unwrap();
    let end = source[start..]
        .find("fn ordinary_due_span_lower_tiers(")
        .unwrap()
        + start;
    let probe = &source[start..end];
    assert!(probe.contains("FROM telemetry_history_due_spans"));
    assert!(probe.contains("domain = $1"));
    assert!(probe.contains("due_at <= now()"));
    for table in [
        "telemetry_rollups",
        "telemetry_network_rates",
        "telemetry_ping_rollups",
        "system_metric_rollups",
    ] {
        assert!(!probe.contains(table));
    }
}

#[test]
fn prune_completion_probes_preserve_their_bounded_index_seek() {
    let history = include_str!("history_retention.rs");
    assert_eq!(history.matches(") bounded_due").count(), 5);
    assert!(history.contains(
        "If it is\n    // written directly inside EXISTS, PostgreSQL may discard the ordering"
    ));

    let observations = include_str!("network_observation_retention.rs")
        .split_once("#[cfg(test)]\nmod tests")
        .map_or_else(
            || include_str!("network_observation_retention.rs"),
            |(production, _)| production,
        );
    assert!(!observations.contains(") bounded_automatic"));
    for boundary in [") bounded_manual", ") bounded_rollup"] {
        assert_eq!(observations.matches(boundary).count(), 1);
    }
    for owner in [
        "NetworkObservationRetentionPhase::InactiveLatestPrune",
        "NetworkObservationRetentionPhase::InactiveSeriesPrune",
        "async fn observation_rollup_promotion_is_due(",
    ] {
        assert!(observations.contains(owner));
    }
    // InactiveLatest, InactiveSeries, and the rollup-promotion due-ledger
    // probe each preserve their own bounded index seek.
    assert_eq!(observations.matches(") bounded_due").count(), 3);
}

#[test]
fn scheduler_contract_registry_assigns_one_compile_time_wake_contract_to_every_owner() {
    assert_eq!(RETENTION_PHASES.len(), 29);
    for (index, owner) in RETENTION_PHASES.iter().enumerate() {
        assert!(
            !RETENTION_PHASES[..index]
                .iter()
                .any(|existing| existing.phase == owner.phase),
            "duplicate retention owner {:?}",
            owner.phase
        );
    }
    assert_eq!(
        RETENTION_PHASES
            .iter()
            .filter(|owner| owner.wake_contract == WakeContract::NotifiedProducer)
            .count(),
        3
    );
    assert_eq!(
        RETENTION_PHASES
            .iter()
            .filter(|owner| owner.wake_contract == WakeContract::DerivedProducer)
            .count(),
        12
    );
    assert_eq!(
        RETENTION_PHASES
            .iter()
            .filter(|owner| owner.wake_contract == WakeContract::Expiry)
            .count(),
        14
    );
}

fn drain_with_all_owners_current(next_at: Instant) -> TelemetryHistoryRetentionDrain {
    let mut drain = TelemetryHistoryRetentionDrain::new(Duration::from_secs(5));
    drain.owner_states = [RetentionOwnerState::Current {
        next_at: RetentionNextAt::At(test_retention_deadline(next_at)),
    }; RETENTION_PHASES.len()];
    drain
}

fn test_retention_deadline(monotonic_at: Instant) -> RetentionDeadline {
    RetentionDeadline {
        database_at: DateTime::<Utc>::from_timestamp(1_900_000_000, 0).unwrap(),
        monotonic_at,
    }
}

#[test]
fn database_deadline_accepts_only_postgres_relative_nonnegative_durations() {
    let database_at = DateTime::<Utc>::from_timestamp(1_900_000_000, 0).unwrap();
    let deadline = database_deadline(database_at, 1.25).unwrap();
    assert_eq!(deadline.database_at, database_at);
    assert_eq!(deadline.remaining, Duration::from_millis(1_250));
    assert!(database_deadline(database_at, -0.001).is_err());
    assert!(database_deadline(database_at, f64::NAN).is_err());
    assert!(database_deadline(database_at, f64::INFINITY).is_err());
}

#[test]
fn database_owned_next_at_queries_never_use_process_wall_clock() {
    for (name, source) in [
        ("history", include_str!("history_retention.rs")),
        (
            "minute",
            include_str!("telemetry_minute_materialization.rs"),
        ),
        ("traffic", include_str!("traffic_retention.rs")),
        (
            "network observation",
            include_str!("network_observation_retention.rs"),
        ),
    ] {
        let production = source
            .rsplit_once("#[cfg(test)]\nmod tests")
            .map_or(source, |(production, _)| production);
        assert!(
            production.contains("clock_timestamp()") && production.contains("remaining_seconds"),
            "{name} NextAt does not carry a same-statement PostgreSQL-relative duration"
        );
        assert!(
            !production.contains("Utc::now()"),
            "{name} NextAt subtracts the process wall clock"
        );
    }
}

#[test]
fn sample_next_at_uses_one_parameterized_min_seek_per_projection_owner() {
    let source = include_str!("history_retention.rs");
    let start = source.find("async fn prune_domain_next_at(").unwrap();
    let end = source[start..]
        .find("async fn prune_ping_fact_rows(")
        .unwrap()
        + start;
    let query = &source[start..end];

    assert!(query.contains("WITH cutoff AS MATERIALIZED ("));
    assert!(query.contains("owners AS MATERIALIZED ("));
    assert!(query.contains("CROSS JOIN LATERAL ("));
    assert!(query.contains("SELECT min(boundary.observed_at)"));
    assert!(query.contains("boundary.client_id = owner.client_id"));
    assert!(query.contains(
        "boundary.observed_at\n                                    > owner.observed_after"
    ));
    assert!(query.contains("candidate.accepted_seq <= owner.projected_seq"));
    assert!(query.contains("ORDER BY sample.accepted_seq"));
    assert!(!query.contains("sample.observed_at > clock_timestamp()"));
    assert!(!query.contains("sample.accepted_seq IS NULL"));
}

#[test]
fn sample_readiness_uses_each_compact_consumer_owner_before_the_time_cutoff() {
    let source = include_str!("history_retention.rs");
    let start = source
        .find("async fn prune_domain_has_remaining_work(")
        .unwrap();
    let end = source[start..]
        .find("async fn prune_domain_next_at(")
        .unwrap()
        + start;
    let query = &source[start..end];

    assert!(query.contains("WITH owners AS MATERIALIZED ("));
    assert!(query.contains("JOIN telemetry_webhook_cursors webhook USING (client_id)"));
    assert!(query.contains("JOIN telemetry_minute_materialization_heads core_minute"));
    assert!(query.contains("JOIN traffic_counter_minute_heads traffic_minute"));
    assert!(query.contains("LEAST("));
    assert!(query.contains("AS safe_seq"));
    assert!(query.contains("sample.accepted_seq <= owner.safe_seq"));
    assert!(query.contains("ORDER BY sample.accepted_seq\n                    LIMIT 1"));
    assert!(query.contains(
        "WHERE frontier.observed_at\n                            < now() - make_interval(days => $1)"
    ));
    assert!(!query.contains("FOR UPDATE OF head"));
    assert!(!query.contains("sample.accepted_seq IS NULL"));
}

fn owner_state(
    drain: &TelemetryHistoryRetentionDrain,
    phase: RetentionPhase,
) -> RetentionOwnerState {
    drain.owner_states[TelemetryHistoryRetentionDrain::phase_index(phase)]
}

#[test]
fn scheduler_contract_frontier_advancement_wakes_exact_minute_owner_and_sample_prune() {
    let future = Instant::now() + Duration::from_secs(60);
    let mut core = drain_with_all_owners_current(future);
    core.notify_core_minute_frontier_advanced_now();
    assert_eq!(
        owner_state(&core, RetentionPhase::CoreMinuteMaterialization),
        RetentionOwnerState::Due
    );
    assert_eq!(
        owner_state(&core, RetentionPhase::TrafficMinuteMaterialization),
        RetentionOwnerState::Current {
            next_at: RetentionNextAt::At(test_retention_deadline(future))
        }
    );
    assert_eq!(
        owner_state(&core, RetentionPhase::SamplePrune),
        RetentionOwnerState::Due
    );
    let mut traffic = drain_with_all_owners_current(future);
    traffic.notify_traffic_minute_frontier_advanced_now();
    assert_eq!(
        owner_state(&traffic, RetentionPhase::TrafficMinuteMaterialization),
        RetentionOwnerState::Due
    );
    assert_eq!(
        owner_state(&traffic, RetentionPhase::CoreMinuteMaterialization),
        RetentionOwnerState::Current {
            next_at: RetentionNextAt::At(test_retention_deadline(future))
        }
    );
    assert_eq!(
        owner_state(&traffic, RetentionPhase::SamplePrune),
        RetentionOwnerState::Due
    );
    assert_eq!(
        owner_state(
            &traffic,
            RetentionPhase::Traffic(TrafficRetentionPhase::RawPromotion)
        ),
        RetentionOwnerState::Current {
            next_at: RetentionNextAt::At(test_retention_deadline(future))
        }
    );
}

#[test]
fn scheduler_contract_due_span_notification_invalidates_only_an_earlier_database_deadline() {
    let cached = Instant::now() + Duration::from_secs(120);
    let mut drain = drain_with_all_owners_current(cached);
    let later = DateTime::<Utc>::from_timestamp(1_900_000_100, 0).unwrap();
    drain
        .notify_due_span_published_at(RESOURCE_ROLLUP_DOMAIN, 60, 300, later)
        .unwrap();
    assert_eq!(
        owner_state(&drain, RetentionPhase::ResourcePromotion),
        RetentionOwnerState::Current {
            next_at: RetentionNextAt::At(test_retention_deadline(cached))
        }
    );

    let earlier = DateTime::<Utc>::from_timestamp(1_899_999_900, 0).unwrap();
    drain
        .notify_due_span_published_at(RESOURCE_ROLLUP_DOMAIN, 60, 300, earlier)
        .unwrap();
    assert_eq!(
        owner_state(&drain, RetentionPhase::ResourcePromotion),
        RetentionOwnerState::Unchecked
    );
}

#[test]
fn scheduler_contract_external_writer_wrappers_advance_later_cached_frontiers() {
    let future = Instant::now() + Duration::from_secs(60);
    let earlier = DateTime::<Utc>::from_timestamp(1_899_999_900, 0).unwrap();

    let mut drain = drain_with_all_owners_current(future);
    drain.notify_projection_minute_ready_at(earlier);
    for phase in [
        RetentionPhase::CoreMinuteMaterialization,
        RetentionPhase::TrafficMinuteMaterialization,
    ] {
        assert_eq!(owner_state(&drain, phase), RetentionOwnerState::Unchecked);
    }

    let mut drain = drain_with_all_owners_current(future);
    for phase in [
        RetentionPhase::CoreMinuteMaterialization,
        RetentionPhase::TrafficMinuteMaterialization,
    ] {
        let phase_index = TelemetryHistoryRetentionDrain::phase_index(phase);
        drain.owner_states[phase_index] = RetentionOwnerState::Current {
            next_at: RetentionNextAt::ProducerOnly,
        };
    }
    drain.notify_projection_minute_ready_at(earlier);
    for phase in [
        RetentionPhase::CoreMinuteMaterialization,
        RetentionPhase::TrafficMinuteMaterialization,
    ] {
        assert_eq!(owner_state(&drain, phase), RetentionOwnerState::Unchecked);
    }

    let mut drain = drain_with_all_owners_current(future);
    drain.notify_due_events_ready_at(earlier);
    assert_eq!(
        owner_state(&drain, RetentionPhase::DueEventCoalescing),
        RetentionOwnerState::Unchecked
    );

    let mut drain = drain_with_all_owners_current(future);
    drain.notify_sample_prune_ready_at(earlier);
    assert_eq!(
        owner_state(&drain, RetentionPhase::SamplePrune),
        RetentionOwnerState::Unchecked
    );

    let mut drain = drain_with_all_owners_current(future);
    drain.notify_sample_prune_now();
    assert_eq!(
        owner_state(&drain, RetentionPhase::SamplePrune),
        RetentionOwnerState::Due
    );

    let mut drain = drain_with_all_owners_current(future);
    drain.notify_manual_network_observation_now();
    assert_eq!(
        owner_state(
            &drain,
            RetentionPhase::NetworkObservation(NetworkObservationRetentionPhase::TerminalPrune)
        ),
        RetentionOwnerState::Due
    );

    let mut drain = drain_with_all_owners_current(future);
    drain.notify_network_observation_series_deactivated_now();
    for phase in [
        NetworkObservationRetentionPhase::InactiveLatestPrune,
        NetworkObservationRetentionPhase::InactiveSeriesPrune,
    ] {
        assert_eq!(
            owner_state(&drain, RetentionPhase::NetworkObservation(phase)),
            RetentionOwnerState::Due
        );
    }

    let mut drain = drain_with_all_owners_current(future);
    drain.notify_traffic_samples_published_now().unwrap();
    assert_eq!(
        owner_state(
            &drain,
            RetentionPhase::Traffic(TrafficRetentionPhase::RawPromotion)
        ),
        RetentionOwnerState::Due
    );

    for (bucket_secs, expected) in [
        (3_600, Some(TrafficRetentionPhase::RollupToThreeHours)),
        (10_800, Some(TrafficRetentionPhase::RollupToSixHours)),
        (21_600, Some(TrafficRetentionPhase::RollupToDay)),
        (86_400, None),
    ] {
        let mut drain = drain_with_all_owners_current(future);
        drain.notify_traffic_rollup_published(bucket_secs).unwrap();
        assert_eq!(
            owner_state(
                &drain,
                RetentionPhase::Traffic(TrafficRetentionPhase::TerminalPrune)
            ),
            RetentionOwnerState::Due
        );
        if let Some(expected) = expected {
            assert_eq!(
                owner_state(&drain, RetentionPhase::Traffic(expected)),
                RetentionOwnerState::Due
            );
        }
    }

    for (domain, expected) in [
        ("telemetry_rollups", RetentionPhase::ResourcePrune),
        ("telemetry_network_rates", RetentionPhase::NetworkRatePrune),
        ("telemetry_ping_rollups", RetentionPhase::PingRollupPrune),
        ("system_metric_rollups", RetentionPhase::SystemMetricPrune),
        (
            "network_observation_rollups",
            RetentionPhase::NetworkObservation(NetworkObservationRetentionPhase::TerminalPrune),
        ),
    ] {
        let mut drain = drain_with_all_owners_current(future);
        drain.notify_ordinary_rollup_published_now(domain).unwrap();
        assert_eq!(owner_state(&drain, expected), RetentionOwnerState::Due);
        assert_eq!(
            drain
                .owner_states
                .iter()
                .filter(|state| matches!(state, RetentionOwnerState::Due))
                .count(),
            1,
            "ordinary rollup publication must wake only {expected:?}"
        );
    }

    for (domain, expected) in [
        ("telemetry_rollups", RetentionPhase::ResourcePrune),
        ("telemetry_network_rates", RetentionPhase::NetworkRatePrune),
        ("telemetry_ping_rollups", RetentionPhase::PingRollupPrune),
        ("system_metric_rollups", RetentionPhase::SystemMetricPrune),
        (
            "network_observations",
            RetentionPhase::NetworkObservation(NetworkObservationRetentionPhase::TerminalPrune),
        ),
        (
            "traffic_counter_rollups",
            RetentionPhase::Traffic(TrafficRetentionPhase::TerminalPrune),
        ),
    ] {
        let mut drain = drain_with_all_owners_current(future);
        drain.notify_retention_policy_changed_now(domain).unwrap();
        assert_eq!(owner_state(&drain, expected), RetentionOwnerState::Due);
    }

    let mut drain = drain_with_all_owners_current(future);
    drain.notify_ping_topology_changed_now();
    assert_eq!(
        owner_state(&drain, RetentionPhase::PingCurrentPrune),
        RetentionOwnerState::Due
    );
    assert_eq!(
        owner_state(&drain, RetentionPhase::PingSeriesPrune),
        RetentionOwnerState::Current {
            next_at: RetentionNextAt::At(test_retention_deadline(future))
        }
    );

    let mut drain = drain_with_all_owners_current(future);
    drain.notify_ping_rollups_deleted_now().unwrap();
    assert_eq!(
        owner_state(&drain, RetentionPhase::PingCurrentPrune),
        RetentionOwnerState::Due
    );

    let mut drain = drain_with_all_owners_current(future);
    drain
        .notify_network_observation_history_deleted_now()
        .unwrap();
    assert_eq!(
        owner_state(
            &drain,
            RetentionPhase::NetworkObservation(
                NetworkObservationRetentionPhase::InactiveSeriesPrune
            )
        ),
        RetentionOwnerState::Due
    );
}

#[test]
fn scheduler_contract_minute_owner_contention_parks_without_a_ready_recheck() {
    let source = include_str!("history_retention.rs");
    let (_, processing) = source
        .split_once("async fn process_retention_phase(")
        .expect("retention phase processor");
    let (processing, _) = processing
        .split_once("fn raw_sample_policy()")
        .expect("retention phase processor boundary");
    assert_eq!(processing.matches("if minute.owner_contended").count(), 2);
    assert_eq!(
        processing
            .matches("RetentionOwnerStatus::Current(RetentionNextAt::ProducerOnly)")
            .count(),
        2
    );
    for consumer in ["Core", "Traffic"] {
        let arm = processing
            .split_once(&format!(
                "RetentionPhase::{consumer}MinuteMaterialization =>"
            ))
            .expect("minute owner arm")
            .1;
        let arm = arm
            .split_once("has_remaining_work =")
            .expect("minute readiness recheck")
            .0;
        assert!(arm.contains("if minute.owner_contended"));
        assert!(arm.contains("RetentionNextAt::ProducerOnly"));
    }
}

#[test]
fn scheduler_contract_reconnect_recovers_exactly_named_external_writer_frontiers() {
    let future = Instant::now() + Duration::from_secs(60);
    let mut drain = drain_with_all_owners_current(future);
    drain.recover_external_writer_frontiers();
    for owner in RETENTION_PHASES {
        let expected = if EXTERNAL_WRITER_FRONTIERS.contains(&owner.phase) {
            RetentionOwnerState::Unchecked
        } else {
            RetentionOwnerState::Current {
                next_at: RetentionNextAt::At(test_retention_deadline(future)),
            }
        };
        assert_eq!(
            owner_state(&drain, owner.phase),
            expected,
            "{:?}",
            owner.phase
        );
    }
}

#[test]
fn scheduler_contract_sample_delete_cascade_wakes_observation_series_owner() {
    let migration = include_str!("../../../../migrations/0004_network_tunnels.sql");
    assert!(migration.contains(
        "automatic_sample_id) REFERENCES public.telemetry_samples(id) ON DELETE CASCADE"
    ));
    let future = Instant::now() + Duration::from_secs(60);
    let mut drain = drain_with_all_owners_current(future);
    drain.notify_telemetry_samples_deleted_now();
    assert_eq!(
        owner_state(
            &drain,
            RetentionPhase::NetworkObservation(
                NetworkObservationRetentionPhase::InactiveSeriesPrune
            )
        ),
        RetentionOwnerState::Due
    );
}

#[test]
fn scheduler_contract_current_owners_wake_only_at_their_exact_next_at() {
    let now = Instant::now();
    let exact = now + Duration::from_secs(40);
    let mut drain = TelemetryHistoryRetentionDrain::new(Duration::from_secs(5));
    drain.recovery_at = now + Duration::from_secs(60);
    for phase_index in 0..RETENTION_PHASES.len() {
        drain
            .observe_phase(
                phase_index,
                RetentionOwnerStatus::Current(RetentionNextAt::At(test_retention_deadline(exact))),
            )
            .unwrap();
    }

    assert_eq!(
        drain.next_eligible_phase_at(exact - Duration::from_nanos(1)),
        None
    );
    assert_eq!(drain.next_eligible_phase_at(exact), Some(0));
    assert_eq!(
        drain.next_step_at(exact - Duration::from_nanos(1)),
        TelemetryHistoryRetentionStep::CurrentUntil(exact)
    );
}

#[test]
fn scheduler_contract_still_due_is_immediate_and_fair_without_a_cadence_deadline() {
    let now = Instant::now();
    let future = now + Duration::from_secs(60);
    let mut drain = TelemetryHistoryRetentionDrain::new(Duration::from_secs(5));
    drain.recovery_at = future;
    for phase_index in 0..RETENTION_PHASES.len() {
        drain
            .observe_phase(
                phase_index,
                RetentionOwnerStatus::Current(RetentionNextAt::At(test_retention_deadline(future))),
            )
            .unwrap();
    }
    drain
        .observe_phase(10, RetentionOwnerStatus::StillDue)
        .unwrap();
    drain
        .observe_phase(12, RetentionOwnerStatus::StillDue)
        .unwrap();

    assert_eq!(drain.next_eligible_phase_at(now), Some(10));
    drain
        .observe_phase(10, RetentionOwnerStatus::StillDue)
        .unwrap();
    assert_eq!(drain.next_eligible_phase_at(now), Some(12));
    assert_eq!(
        drain.next_step_at(now),
        TelemetryHistoryRetentionStep::MoreWork
    );
    assert_eq!(drain.owner_states[10], RetentionOwnerState::Due);
}

#[test]
fn scheduler_contract_one_watchdog_recovers_only_sentinels_and_failures() {
    let now = Instant::now();
    let recovery_at = now + Duration::from_secs(5);
    let exact = now + Duration::from_secs(60);
    let mut drain = TelemetryHistoryRetentionDrain::new(Duration::from_secs(5));
    drain.recovery_at = recovery_at;
    drain.owner_states = [RetentionOwnerState::Current {
        next_at: RetentionNextAt::At(test_retention_deadline(exact)),
    }; RETENTION_PHASES.len()];
    drain.owner_states[0] = RetentionOwnerState::Current {
        next_at: RetentionNextAt::ProducerOnly,
    };
    drain.owner_states[7] = RetentionOwnerState::Current {
        next_at: RetentionNextAt::ProducerOnly,
    };
    drain.owner_states[8] = RetentionOwnerState::Failed;

    assert_eq!(drain.next_eligible_phase_at(now), None);
    assert_eq!(
        drain.next_step_at(recovery_at),
        TelemetryHistoryRetentionStep::MoreWork
    );
    assert_eq!(drain.owner_states[0], RetentionOwnerState::Unchecked);
    assert_eq!(drain.owner_states[7], RetentionOwnerState::Unchecked);
    assert_eq!(drain.owner_states[8], RetentionOwnerState::Unchecked);
    assert_eq!(
        drain.owner_states[9],
        RetentionOwnerState::Current {
            next_at: RetentionNextAt::At(test_retention_deadline(exact))
        }
    );
}

#[test]
fn transport_failure_keeps_global_backoff_while_owner_failure_is_isolated() {
    assert!(super::retention_error_requires_global_backoff(
        &sqlx::Error::PoolClosed.into()
    ));
    assert!(!super::retention_error_requires_global_backoff(
        &anyhow::anyhow!("one retention owner rejected its page")
    ));
}

#[test]
fn telemetry_pruning_is_bounded_and_concurrency_safe() {
    let query = prune_query("telemetry_rollups");
    assert!(query.contains("LIMIT $2"));
    assert!(query.contains("FOR UPDATE OF row SKIP LOCKED"));
    assert!(query.contains("bucket_start"));
    assert!(query.contains("row.tableoid AS source_tableoid"));
    assert!(query.contains("row.ctid AS source_ctid"));
    assert!(query.contains("row.tableoid = candidate.source_tableoid"));
    assert!(query.contains("row.ctid = candidate.source_ctid"));
    assert!(!query.contains("ctid IN"));
    assert!(!query.contains("DELETE FROM telemetry_network_rates"));
}

#[test]
fn network_rate_promotion_uses_partition_safe_physical_row_identity() {
    let source = include_str!("history_retention.rs");
    let start = source
        .find("async fn promote_network_rate_tier_in_tx(")
        .unwrap();
    let end = source[start..]
        .find("async fn promote_ping_rollups(")
        .unwrap()
        + start;
    let promotion = &source[start..end];

    assert!(promotion.contains("row.tableoid AS source_tableoid"));
    assert!(promotion.contains("row.ctid AS source_ctid"));
    assert!(promotion.contains("eligible.source_tableoid = row.tableoid"));
    assert!(promotion.contains("eligible.source_ctid = row.ctid"));
    assert!(promotion.contains("row.tableoid = source.source_tableoid"));
    assert!(promotion.contains("row.ctid = source.source_ctid"));
    assert!(promotion.contains("RETURNING row.tableoid, row.ctid, row.bucket_secs"));
}

#[test]
fn raw_sample_prune_waits_for_both_independent_consumers() {
    let owner_query = sample_prune_owner_query();
    let page_query = sample_prune_query();
    assert!(owner_query.contains("FROM telemetry_projection_heads head"));
    assert!(owner_query.contains("JOIN telemetry_webhook_cursors webhook USING (client_id)"));
    assert!(owner_query.contains("JOIN telemetry_minute_materialization_heads core_minute"));
    assert!(owner_query.contains("JOIN traffic_counter_minute_heads traffic_minute"));
    assert!(owner_query.contains("webhook.last_sample_seq"));
    assert!(owner_query.contains("core_minute.materialized_seq"));
    assert!(owner_query.contains("traffic_minute.materialized_seq"));
    assert!(owner_query.contains("LEAST("));
    assert!(owner_query.contains("AS safe_seq"));
    assert!(owner_query.contains("owner_frontiers AS MATERIALIZED"));
    assert!(owner_query.contains("selected_owner AS MATERIALIZED"));
    assert!(owner_query.contains("bounded_owner AS MATERIALIZED"));
    assert!(owner_query.contains("ORDER BY sample.accepted_seq\n                LIMIT 1"));
    assert!(owner_query.contains("WHERE first_sample.observed_at < owner.expires_before"));
    assert!(owner_query.contains("ORDER BY owner.observed_at, owner.client_id"));
    assert!(owner_query.contains("FROM owner_frontiers owner\n            ORDER BY owner.observed_at, owner.client_id\n            LIMIT 1"));
    assert!(owner_query.contains("owner.safe_seq"));
    assert!(owner_query.contains("sample.observed_at = ("));
    assert!(owner_query.contains("SELECT min(boundary.observed_at)"));
    assert!(owner_query.contains("boundary.client_id = owner.client_id"));
    assert!(owner_query.contains("boundary.observed_at >= owner.expires_before"));
    assert!(owner_query.contains("ORDER BY sample.accepted_seq"));
    assert!(owner_query.contains("AS due_through_seq"));
    assert!(owner_query.contains("FROM bounded_owner owner"));
    assert!(page_query.contains("WHERE sample.client_id = $1"));
    assert!(page_query.contains("sample.accepted_seq <= $2"));
    assert!(page_query.contains("sample.id IS DISTINCT FROM $3"));
    assert!(page_query.contains("sample.observed_at < $4"));
    assert!(page_query.contains("ORDER BY sample.accepted_seq"));
    assert!(page_query.contains("LIMIT $5"));
    assert!(page_query.contains("FOR UPDATE OF sample SKIP LOCKED"));
    assert!(page_query.contains("ARRAY(SELECT candidate.ctid FROM candidates candidate)"));
    assert!(!owner_query.contains("FOR UPDATE OF head"));
    assert!(!page_query.contains("FOR UPDATE OF head"));
    assert!(!owner_query.contains("sample.accepted_seq IS NULL"));
    assert!(!page_query.contains("sample.accepted_seq IS NULL"));
}

#[tokio::test]
async fn generic_prune_matches_tableoid_and_ctid_across_partitions() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let mut connection = db.pool.acquire().await.unwrap();
    for statement in [
        r#"
        CREATE TEMP TABLE partitioned_prune_probe (
            shard INTEGER NOT NULL,
            bucket_start TIMESTAMPTZ NOT NULL,
            bucket_secs INTEGER NOT NULL
        ) PARTITION BY LIST (shard)
        "#,
        r#"
        CREATE TEMP TABLE partitioned_prune_probe_one
            PARTITION OF partitioned_prune_probe FOR VALUES IN (1)
        "#,
        r#"
        CREATE TEMP TABLE partitioned_prune_probe_two
            PARTITION OF partitioned_prune_probe FOR VALUES IN (2)
        "#,
        r#"
        INSERT INTO partitioned_prune_probe (shard, bucket_start, bucket_secs)
        VALUES
            (1, date_trunc('day', now()) - interval '10 days', 60),
            (2, date_trunc('day', now()) - interval '10 days', 60)
        "#,
    ] {
        sqlx::query(statement)
            .execute(&mut *connection)
            .await
            .unwrap();
    }

    let duplicate_physical_ids: bool = sqlx::query_scalar(
        r#"
        SELECT count(*) = 2 AND count(DISTINCT ctid) = 1
        FROM partitioned_prune_probe
        "#,
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert!(duplicate_physical_ids);

    let deleted = sqlx::query(&prune_query("partitioned_prune_probe"))
        .bind(1_i32)
        .bind(1_i32)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(deleted, 1);
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM partitioned_prune_probe")
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    assert_eq!(remaining, 1);

    drop(connection);
    db.cleanup().await;
}

#[test]
fn telemetry_pruning_retains_bucket_crossing_cutoff() {
    let query = prune_query("telemetry_rollups");

    assert!(query.contains(
        "row.bucket_start\n                + make_interval(secs => GREATEST(row.bucket_secs, 1)) <= ("
    ));
    assert!(query.contains("WHERE row.bucket_start < ("));
}

async fn retention_test_anchor(pool: &PgPool) -> DateTime<Utc> {
    sqlx::query_scalar("SELECT date_trunc('hour', now()) - interval '40 days'")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn insert_test_client(pool: &PgPool, client_id: &str) {
    sqlx::query(
        r#"
        INSERT INTO clients (id, display_name, public_key, status)
        VALUES ($1, $1, decode('', 'hex'), 'online')
        ON CONFLICT (id) DO NOTHING
        "#,
    )
    .bind(client_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_resource_point<'e, E>(
    executor: E,
    client_id: &str,
    bucket_start: DateTime<Utc>,
    bucket_secs: i32,
    value: f64,
) where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query(
        r#"
        INSERT INTO telemetry_rollups (
            client_id, bucket_start, bucket_secs, sample_count,
            cpu_load_1_avg, cpu_load_1_sum, cpu_load_1_max,
            memory_total_bytes_max, memory_available_bytes_avg,
            memory_available_bytes_sum, memory_available_bytes_min,
            memory_used_ratio_avg, memory_used_ratio_sum,
            memory_used_ratio_max, latest_observed_at
        ) VALUES (
            $1, $2, $3, 1, $4, $4, $4,
            1024, 512, 512, 512, 0.5, 0.5, 0.5,
            $2 + make_interval(secs => LEAST($3 - 1, 59))
        )
        "#,
    )
    .bind(client_id)
    .bind(bucket_start)
    .bind(bucket_secs)
    .bind(value)
    .execute(executor)
    .await
    .unwrap();
}

async fn insert_network_point(
    pool: &PgPool,
    client_id: &str,
    interface: &str,
    bucket_start: DateTime<Utc>,
    bucket_secs: i32,
    counter: i64,
    epoch: i64,
) {
    sqlx::query(
        r#"
        INSERT INTO telemetry_network_rates (
            client_id, interface, bucket_start, bucket_secs, sample_count,
            rx_bytes_sum, tx_bytes_sum, rx_bytes_avg, tx_bytes_avg,
            rx_bytes_last, tx_bytes_last, rx_counter_epoch, tx_counter_epoch,
            latest_observed_at
        ) VALUES (
            $1, $2, $3, $4, 1,
            $5, $5 * 2, $5, $5 * 2,
            $5, $5 * 2, $6, $6,
            $3 + make_interval(secs => LEAST($4 - 1, 59))
        )
        "#,
    )
    .bind(client_id)
    .bind(interface)
    .bind(bucket_start)
    .bind(bucket_secs)
    .bind(counter)
    .bind(epoch)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_ping_point(
    pool: &PgPool,
    series_id: i64,
    bucket_start: DateTime<Utc>,
    bucket_secs: i32,
    latency: f64,
) {
    sqlx::query(
        r#"
        INSERT INTO telemetry_ping_rollups (
            series_id, bucket_start, bucket_secs, sample_count, success_count,
            latency_sum_ms, latency_avg_ms, latency_min_ms, latency_max_ms,
            loss_ratio_avg, loss_ratio_sum, loss_ratio_max,
            latest_status, latest_checked_at
        ) VALUES (
            $1, $2, $3, 1, 1, $4, $4, $4, $4,
            0, 0, 0, 'ok',
            $2 + make_interval(secs => LEAST($3 - 1, 59))
        )
        "#,
    )
    .bind(series_id)
    .bind(bucket_start)
    .bind(bucket_secs)
    .bind(latency)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_system_point(
    pool: &PgPool,
    metric: &str,
    bucket_start: DateTime<Utc>,
    bucket_secs: i32,
    value: f64,
) {
    sqlx::query(
        r#"
        INSERT INTO system_metric_rollups (
            metric, bucket_start, bucket_secs, sample_count, value_sum,
            avg_value, max_value, latest_value, latest_observed_at
        ) VALUES (
            $1, $2, $3, 1, $4, $4, $4, $4,
            $2 + make_interval(secs => LEAST($3 - 1, 59))
        )
        "#,
    )
    .bind(metric)
    .bind(bucket_start)
    .bind(bucket_secs)
    .bind(value)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_test_ping_series(
    pool: &PgPool,
    client_id: &str,
    target_id: &str,
    target_name: &str,
) -> i64 {
    sqlx::query(
        r#"
        INSERT INTO ping_targets (id, name, host, probe_kind)
        VALUES ($1::uuid, $2, '127.0.0.1', 'icmp')
        "#,
    )
    .bind(target_id)
    .bind(target_name)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        r#"
        INSERT INTO telemetry_ping_series (client_id, target_id, generation)
        VALUES ($1, $2::uuid, 1)
        RETURNING id
        "#,
    )
    .bind(client_id)
    .bind(target_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn coalesce_all_due_events(pool: &PgPool) -> u64 {
    let mut total = 0_u64;
    loop {
        let coalescing = coalesce_ready_telemetry_due_events(pool).await.unwrap();
        total = total.saturating_add(coalescing.coalesced);
        if !coalescing.has_remaining_work {
            return total;
        }
    }
}

#[tokio::test]
async fn ping_dashboard_arrival_owns_only_changed_series_and_monotonic_client_edge() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "ping-dashboard-exact-arrival";
    insert_test_client(&db.pool, client_id).await;
    let first_series = insert_test_ping_series(
        &db.pool,
        client_id,
        "51000000-0000-0000-0000-000000000001",
        "ping-dashboard-exact-first",
    )
    .await;
    let second_series = insert_test_ping_series(
        &db.pool,
        client_id,
        "51000000-0000-0000-0000-000000000002",
        "ping-dashboard-exact-second",
    )
    .await;
    let anchor = DateTime::parse_from_rfc3339("2024-01-02T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    // Holding the retired synthetic lock proves that arrival no longer waits
    // on it. The touched series primary key is now the only serialization
    // owner for its envelope.
    let mut obsolete_lock = db.pool.begin().await.unwrap();
    sqlx::query(
        r#"
        SELECT pg_advisory_xact_lock(hashtextextended(
            'telemetry-dashboard-ping-bound:' || $1::text, 0
        ))
        "#,
    )
    .bind(first_series)
    .execute(&mut *obsolete_lock)
    .await
    .unwrap();
    tokio::time::timeout(
        Duration::from_secs(2),
        insert_ping_point(&db.pool, first_series, anchor, 60, 10.0),
    )
    .await
    .expect("Ping arrival still waited on the retired advisory lock");
    obsolete_lock.rollback().await.unwrap();

    insert_ping_point(
        &db.pool,
        first_series,
        anchor + ChronoDuration::minutes(2),
        60,
        20.0,
    )
    .await;
    insert_ping_point(
        &db.pool,
        second_series,
        anchor - ChronoDuration::minutes(1),
        60,
        30.0,
    )
    .await;

    let envelope: (DateTime<Utc>, DateTime<Utc>, DateTime<Utc>) = sqlx::query_as(
        r#"
        SELECT head.ping_first_at,
               bounds.first_bucket_start, bounds.last_bucket_start
        FROM telemetry_dashboard_ping_projection_heads head
        JOIN telemetry_dashboard_ping_series_bounds bounds
          ON bounds.series_id = $2
        WHERE head.client_id = $1
        "#,
    )
    .bind(client_id)
    .bind(first_series)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        envelope,
        (
            anchor - ChronoDuration::minutes(1),
            anchor,
            anchor + ChronoDuration::minutes(2)
        )
    );

    sqlx::query("DELETE FROM telemetry_ping_rollups WHERE series_id = $1")
        .bind(second_series)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, DateTime<Utc>>(
            "SELECT ping_first_at FROM telemetry_dashboard_ping_projection_heads WHERE client_id = $1",
        )
        .bind(client_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        anchor,
        "the retention/delete consumer must repair an inward-moving edge"
    );

    let insert_owner: String = sqlx::query_scalar(
        "SELECT pg_get_functiondef('maintain_telemetry_ping_dashboard_after_insert()'::regprocedure)",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(!insert_owner.contains("pg_advisory"));
    assert!(!insert_owner.contains("refresh_telemetry_dashboard_ping_heads"));
    db.cleanup().await;
}

#[tokio::test]
async fn absent_due_span_producers_append_without_cross_client_blocking() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let anchor = retention_test_anchor(&db.pool).await;
    for client_id in ["due-event-held", "due-event-independent"] {
        insert_test_client(&db.pool, client_id).await;
    }

    let mut held_producer = db.pool.begin().await.unwrap();
    insert_resource_point(&mut *held_producer, "due-event-held", anchor, 60, 1.0).await;

    tokio::time::timeout(
        Duration::from_secs(5),
        insert_resource_point(&db.pool, "due-event-independent", anchor, 60, 2.0),
    )
    .await
    .expect("an absent-span producer blocked behind another producer");

    let before_held_commit: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM telemetry_history_due_events
             WHERE domain = 'telemetry_rollups'
               AND source_bucket_secs = 60
               AND destination_bucket_secs = 300
               AND destination_start = $1),
            (SELECT count(*) FROM telemetry_history_due_spans
             WHERE domain = 'telemetry_rollups'
               AND source_bucket_secs = 60
               AND destination_bucket_secs = 300
               AND destination_start = $1)
        "#,
    )
    .bind(anchor)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(before_held_commit, (1, 0));

    held_producer.commit().await.unwrap();
    assert_eq!(coalesce_all_due_events(&db.pool).await, 2);
    let after_coalescing: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM telemetry_history_due_events
             WHERE domain = 'telemetry_rollups'
               AND source_bucket_secs = 60
               AND destination_bucket_secs = 300
               AND destination_start = $1),
            (SELECT count(*) FROM telemetry_history_due_spans
             WHERE domain = 'telemetry_rollups'
               AND source_bucket_secs = 60
               AND destination_bucket_secs = 300
               AND destination_start = $1)
        "#,
    )
    .bind(anchor)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(after_coalescing, (0, 2));

    db.cleanup().await;
}

#[tokio::test]
async fn exact_coordinate_pages_drain_ready_owners_and_preserve_open_events() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    for client_id in ["due-open", "due-closed-a", "due-closed-b", "due-closed-c"] {
        insert_test_client(&db.pool, client_id).await;
    }
    let open_start: DateTime<Utc> = sqlx::query_scalar(
        r#"
        SELECT date_bin(
            interval '5 minutes', now(),
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        ) + interval '5 minutes'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let closed_start = retention_test_anchor(&db.pool).await;
    insert_resource_point(&db.pool, "due-open", open_start, 60, 1.0).await;
    insert_resource_point(&db.pool, "due-closed-a", closed_start, 60, 2.0).await;
    insert_resource_point(&db.pool, "due-closed-b", closed_start, 60, 3.0).await;
    insert_resource_point(
        &db.pool,
        "due-closed-c",
        closed_start + ChronoDuration::minutes(5),
        60,
        4.0,
    )
    .await;

    assert_eq!(coalesce_all_due_events(&db.pool).await, 3);
    let owners: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            count(*) FILTER (WHERE destination_start = $1),
            count(*) FILTER (WHERE destination_start = $2),
            count(*) FILTER (WHERE destination_start = $3),
            (SELECT count(*) FROM telemetry_history_due_spans
             WHERE domain = 'telemetry_rollups'
               AND destination_start = $2),
            (SELECT count(*) FROM telemetry_history_due_spans
             WHERE domain = 'telemetry_rollups'
               AND destination_start = $3)
        FROM telemetry_history_due_events
        WHERE domain = 'telemetry_rollups'
        "#,
    )
    .bind(open_start)
    .bind(closed_start)
    .bind(closed_start + ChronoDuration::minutes(5))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(owners, (1, 0, 0, 2, 1));

    db.cleanup().await;
}

#[tokio::test]
async fn producer_coalescer_and_retention_delete_preserve_late_events() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let anchor = retention_test_anchor(&db.pool).await;
    for client_id in ["due-race-seed", "due-race-post-snapshot"] {
        insert_test_client(&db.pool, client_id).await;
    }
    insert_resource_point(&db.pool, "due-race-seed", anchor, 60, 1.0).await;
    let coalescing = coalesce_ready_telemetry_due_events(&db.pool).await.unwrap();
    assert_eq!(coalescing.coalesced, 1);
    assert!(!coalescing.has_remaining_work);

    let mut retention_delete = db.pool.begin().await.unwrap();
    let retention_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *retention_delete)
        .await
        .unwrap();
    let deleted = sqlx::query(
        r#"
        DELETE FROM telemetry_history_due_spans
        WHERE domain = 'telemetry_rollups'
          AND source_bucket_secs = 60
          AND destination_bucket_secs = 300
          AND destination_start = $1
          AND owner_identity = ARRAY['due-race-seed']
        "#,
    )
    .bind(anchor)
    .execute(&mut *retention_delete)
    .await
    .unwrap();
    assert_eq!(deleted.rows_affected(), 1);

    tokio::time::timeout(Duration::from_secs(5), async {
        sqlx::query(
            r#"
            UPDATE telemetry_rollups
            SET updated_at = updated_at
            WHERE client_id = 'due-race-seed'
              AND bucket_secs = 60
              AND bucket_start = $1
            "#,
        )
        .bind(anchor)
        .execute(&db.pool)
        .await
        .unwrap();
    })
    .await
    .expect("producer blocked behind retention's unique due-span delete");

    let coalescer_pool = db.pool.clone();
    let mut coalescer = tokio::spawn(async move {
        coalesce_ready_telemetry_due_events(&coalescer_pool)
            .await
            .unwrap()
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                r#"
                SELECT EXISTS (
                    SELECT 1
                    FROM pg_stat_activity activity
                    WHERE activity.datname = current_database()
                      AND activity.pid <> pg_backend_pid()
                      AND activity.state = 'active'
                      AND activity.wait_event_type = 'Lock'
                      AND $1 = ANY(pg_blocking_pids(activity.pid))
                )
                "#,
            )
            .bind(retention_pid)
            .fetch_one(&db.pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("coalescer did not synchronize with retention's exact span delete");
    assert!(!coalescer.is_finished());

    tokio::time::timeout(
        Duration::from_secs(5),
        insert_resource_point(
            &db.pool,
            "due-race-post-snapshot",
            anchor + ChronoDuration::minutes(5),
            60,
            3.0,
        ),
    )
    .await
    .expect("post-snapshot producer blocked behind the ready frontier");

    retention_delete.commit().await.unwrap();
    let coalescing = tokio::time::timeout(Duration::from_secs(5), &mut coalescer)
        .await
        .expect("coalescer did not finish after retention released the span")
        .expect("coalescer task panicked");
    assert_eq!(coalescing.coalesced, 1);
    assert!(
        coalescing.has_remaining_work,
        "the post-snapshot event must remain runnable"
    );

    let between_passes: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM telemetry_history_due_events
             WHERE domain = 'telemetry_rollups'),
            (SELECT count(*) FROM telemetry_history_due_spans
             WHERE domain = 'telemetry_rollups'
               AND destination_start = $1),
            (SELECT count(*) FROM telemetry_history_due_spans
             WHERE domain = 'telemetry_rollups'
               AND destination_start = $2)
        "#,
    )
    .bind(anchor)
    .bind(anchor + ChronoDuration::minutes(5))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(between_passes, (1, 1, 0));

    let coalescing = coalesce_ready_telemetry_due_events(&db.pool).await.unwrap();
    assert_eq!(coalescing.coalesced, 1);
    assert!(!coalescing.has_remaining_work);

    let first_promotion = promote_resource_rollups(&db.pool).await.unwrap();
    assert_eq!(first_promotion.promotion.promoted, 1);
    assert!(first_promotion.has_remaining_work);
    let second_promotion = promote_resource_rollups(&db.pool).await.unwrap();
    assert_eq!(second_promotion.promotion.promoted, 1);
    assert!(!second_promotion.has_remaining_work);
    let final_owners: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM telemetry_history_due_events
             WHERE domain = 'telemetry_rollups'
               AND source_bucket_secs = 60
               AND destination_bucket_secs = 300),
            (SELECT count(*) FROM telemetry_history_due_spans
             WHERE domain = 'telemetry_rollups'
               AND source_bucket_secs = 60
               AND destination_bucket_secs = 300)
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(final_owners, (0, 0));

    db.cleanup().await;
}
#[tokio::test]
async fn resource_due_spans_bound_each_page_to_one_natural_owner() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let anchor = retention_test_anchor(&db.pool).await;
    for client_id in ["a", "b", "c", "d", "e", "f", "g"] {
        insert_test_client(&db.pool, client_id).await;
        let offset = if client_id == "a" { 0 } else { 4 };
        insert_resource_point(
            &db.pool,
            client_id,
            anchor + ChronoDuration::minutes(offset),
            60,
            (offset + 1) as f64,
        )
        .await;
    }
    assert_eq!(coalesce_all_due_events(&db.pool).await, 7);

    let page = promote_resource_rollups(&db.pool).await.unwrap();
    assert_eq!(page.promotion.promoted, 1);
    assert_eq!(page.promotion.source_rows, 1);
    assert!(page.has_remaining_work);
    let after_one_page: (i64, i64) = sqlx::query_as(
        r#"
        SELECT count(*) FILTER (WHERE bucket_secs = 60),
               count(*) FILTER (WHERE bucket_secs = 300)
        FROM telemetry_rollups
        WHERE client_id = ANY($1::text[])
        "#,
    )
    .bind(vec!["a", "b", "c", "d", "e", "f", "g"])
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(after_one_page, (6, 1));

    let mut promoted = 1_u64;
    for remaining_page in (0..6).rev() {
        let page = promote_resource_rollups(&db.pool).await.unwrap();
        promoted = promoted.saturating_add(page.promotion.promoted);
        assert_eq!(page.promotion.promoted, 1);
        assert_eq!(page.has_remaining_work, remaining_page > 0);
    }
    assert_eq!(promoted, 7);
    let shape: (i64, i64) = sqlx::query_as(
        r#"
        SELECT count(*) FILTER (WHERE bucket_secs = 60),
               count(*) FILTER (WHERE bucket_secs = 300)
        FROM telemetry_rollups
        WHERE client_id = ANY($1::text[])
        "#,
    )
    .bind(vec!["a", "b", "c", "d", "e", "f", "g"])
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(shape, (0, 7));
    let due_span_exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM telemetry_history_due_spans
            WHERE domain = 'telemetry_rollups'
              AND source_bucket_secs = 60
              AND destination_bucket_secs = 300
              AND destination_start = $1
        )
        "#,
    )
    .bind(anchor)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(!due_span_exists);
    db.cleanup().await;
}

#[tokio::test]
async fn system_metric_due_events_promote_one_closed_natural_span() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let anchor = retention_test_anchor(&db.pool).await;
    for minute in 0..5 {
        insert_system_point(
            &db.pool,
            "system.closed-span",
            anchor + ChronoDuration::minutes(minute),
            60,
            1.0 + minute as f64,
        )
        .await;
    }
    assert_eq!(coalesce_all_due_events(&db.pool).await, 5);

    let page = promote_system_metric_rollups(&db.pool).await.unwrap();
    assert_eq!(page.promotion.promoted, 1);
    assert!(!page.has_remaining_work);
    let aggregate: (i32, i32, f64, f64, f64, f64) = sqlx::query_as(
        r#"
        SELECT bucket_secs, sample_count, value_sum, avg_value,
               max_value, latest_value
        FROM system_metric_rollups
        WHERE metric = 'system.closed-span'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(aggregate, (300, 5, 15.0, 3.0, 5.0, 5.0));
    let owners: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM telemetry_history_due_spans
             WHERE domain = 'system_metric_rollups'
               AND source_bucket_secs = 60
               AND destination_bucket_secs = 300
               AND destination_start = $1),
            (SELECT count(*) FROM telemetry_history_due_events
             WHERE domain = 'system_metric_rollups'
               AND source_bucket_secs = 300
               AND destination_bucket_secs = 1800)
        "#,
    )
    .bind(anchor)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(owners, (0, 1));

    db.cleanup().await;
}

#[tokio::test]
async fn resource_due_ledger_retries_locked_evidence_and_claims_pages_once() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let anchor = retention_test_anchor(&db.pool).await;
    insert_test_client(&db.pool, "cursor-held").await;
    insert_resource_point(&db.pool, "cursor-held", anchor, 60, 1.0).await;
    assert_eq!(coalesce_all_due_events(&db.pool).await, 1);

    let mut holder = db.pool.begin().await.unwrap();
    sqlx::query(
        r#"
        SELECT client_id FROM telemetry_rollups
        WHERE client_id = 'cursor-held' AND bucket_secs = 60
        FOR UPDATE
        "#,
    )
    .fetch_one(&mut *holder)
    .await
    .unwrap();
    let skipped = promote_resource_rollups(&db.pool).await.unwrap();
    assert_eq!(
        (skipped.promotion.promoted, skipped.promotion.source_rows),
        (0, 0)
    );
    let due_span_retained: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM telemetry_history_due_spans
            WHERE domain = 'telemetry_rollups'
              AND source_bucket_secs = 60
              AND destination_bucket_secs = 300
              AND destination_start = $1
        )
        "#,
    )
    .bind(anchor)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(due_span_retained);
    holder.rollback().await.unwrap();

    let restarted_pool = db.additional_pool(4).await.unwrap();
    let retried = promote_resource_rollups(&restarted_pool).await.unwrap();
    assert_eq!(
        (retried.promotion.promoted, retried.promotion.source_rows),
        (1, 1)
    );

    for (client_id, offset) in [("cursor-serial-new", 10), ("cursor-serial-old", 5)] {
        insert_test_client(&restarted_pool, client_id).await;
        insert_resource_point(
            &restarted_pool,
            client_id,
            anchor + ChronoDuration::minutes(offset),
            60,
            offset as f64,
        )
        .await;
    }
    assert!(coalesce_all_due_events(&restarted_pool).await >= 2);
    let barrier = Arc::new(Barrier::new(2));
    let promote = |pool: PgPool, barrier: Arc<Barrier>| async move {
        barrier.wait().await;
        promote_resource_rollups(&pool).await.unwrap()
    };
    let (left, right) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(
            promote(restarted_pool.clone(), barrier.clone()),
            promote(restarted_pool.clone(), barrier)
        )
    })
    .await
    .expect("independent due-span rows must be claimed without duplication");
    assert_eq!(left.promotion.promoted + right.promotion.promoted, 2);
    assert_eq!(left.promotion.source_rows + right.promotion.source_rows, 2);
    let serial_shape: (i64, i64) = sqlx::query_as(
        r#"
        SELECT count(*) FILTER (WHERE bucket_secs = 60),
               count(*) FILTER (WHERE bucket_secs = 300)
        FROM telemetry_rollups
        WHERE client_id LIKE 'cursor-serial-%'
        "#,
    )
    .fetch_one(&restarted_pool)
    .await
    .unwrap();
    assert_eq!(serial_shape, (0, 2));
    restarted_pool.close().await;
    db.cleanup().await;
}

#[tokio::test]
async fn ping_due_span_rollback_restart_and_destination_conflict_is_fail_closed() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let anchor = retention_test_anchor(&db.pool).await;
    insert_test_client(&db.pool, "ping-cursor").await;
    let series_id = insert_test_ping_series(
        &db.pool,
        "ping-cursor",
        "40000000-0000-0000-0000-000000000001",
        "ping-cursor",
    )
    .await;
    for minute in 0..5 {
        insert_ping_point(
            &db.pool,
            series_id,
            anchor + ChronoDuration::minutes(minute),
            60,
            10.0 + minute as f64,
        )
        .await;
    }
    assert_eq!(coalesce_all_due_events(&db.pool).await, 5);
    let mut rolled_back = db.pool.begin().await.unwrap();
    let attempted = promote_ping_tier_in_tx(&mut rolled_back, 300, 60, anchor, &[60], series_id)
        .await
        .unwrap();
    assert_eq!(attempted.promotion.promoted, 1);
    assert!(attempted.complete);
    rolled_back.rollback().await.unwrap();

    let after_rollback: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            count(*) FILTER (WHERE bucket_secs = 60),
            count(*) FILTER (WHERE bucket_secs = 300)
        FROM telemetry_ping_rollups
        WHERE series_id = $1
        "#,
    )
    .bind(series_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(after_rollback, (5, 0));

    let restarted_pool = db.additional_pool(2).await.unwrap();
    db.pool.close().await;
    let promoted = promote_ping_rollups(&restarted_pool).await.unwrap();
    assert_eq!(
        (promoted.promotion.promoted, promoted.promotion.source_rows,),
        (1, 5)
    );

    for minute in 5..10 {
        insert_ping_point(
            &restarted_pool,
            series_id,
            anchor + ChronoDuration::minutes(minute),
            60,
            10.0 + minute as f64,
        )
        .await;
    }
    insert_ping_point(
        &restarted_pool,
        series_id,
        anchor + ChronoDuration::minutes(5),
        300,
        99.0,
    )
    .await;
    assert!(coalesce_all_due_events(&restarted_pool).await >= 2);

    for _ in 0..2 {
        let conflict = promote_ping_rollups(&restarted_pool).await.unwrap_err();
        assert_eq!(
            conflict.to_string(),
            "telemetry_ping_rollups promotion from 60s to 300s found 1 unsupported destination or overlap conflicts",
            "a stable destination conflict must remain terminal on every retry",
        );
    }
    let final_shape: (i64, i64, bool) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM telemetry_ping_rollups
             WHERE series_id = $1 AND bucket_secs = 60),
            (SELECT count(*) FROM telemetry_ping_rollups
             WHERE series_id = $1 AND bucket_secs = 300),
            EXISTS (
                SELECT 1 FROM telemetry_history_due_spans
                WHERE domain = 'telemetry_ping_rollups'
                  AND source_bucket_secs = 60
                  AND destination_bucket_secs = 300
                  AND destination_start = $2
            )
        "#,
    )
    .bind(series_id)
    .bind(anchor + ChronoDuration::minutes(5))
    .fetch_one(&restarted_pool)
    .await
    .unwrap();
    assert_eq!(final_shape, (5, 2, true));
    restarted_pool.close().await;
    db.cleanup().await;
}

#[tokio::test]
async fn future_due_spans_are_quiet_until_their_absolute_deadline() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let recent: DateTime<Utc> = sqlx::query_scalar(
        r#"
        SELECT date_bin(
            interval '5 minutes', now(),
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        ) - interval '5 minutes'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    insert_test_client(&db.pool, "due-quiet").await;
    insert_resource_point(&db.pool, "due-quiet", recent, 60, 1.0).await;
    insert_network_point(&db.pool, "due-quiet", "eth0", recent, 60, 1, 0).await;
    let series_id = insert_test_ping_series(
        &db.pool,
        "due-quiet",
        "40000000-0000-0000-0000-000000000002",
        "due-quiet",
    )
    .await;
    insert_ping_point(&db.pool, series_id, recent, 60, 1.0).await;
    insert_system_point(&db.pool, "due.quiet", recent, 60, 1.0).await;
    assert_eq!(coalesce_all_due_events(&db.pool).await, 4);

    let due_now: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*)
        FROM telemetry_history_due_spans
        WHERE domain = ANY(ARRAY[
            'telemetry_rollups', 'telemetry_network_rates',
            'telemetry_ping_rollups', 'system_metric_rollups'
        ])
          AND due_at <= clock_timestamp()
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(due_now, 0);
    let ledger_before: Value = sqlx::query_scalar(
        r#"
        SELECT jsonb_agg(to_jsonb(due) ORDER BY domain, source_bucket_secs)
        FROM telemetry_history_due_spans due
        WHERE domain = ANY(ARRAY[
            'telemetry_rollups', 'telemetry_network_rates',
            'telemetry_ping_rollups', 'system_metric_rollups'
        ])
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let run = process_telemetry_history_retention(&db.pool).await.unwrap();
    assert!(!run.has_activity());
    let ledger_after: Value = sqlx::query_scalar(
        r#"
        SELECT jsonb_agg(to_jsonb(due) ORDER BY domain, source_bucket_secs)
        FROM telemetry_history_due_spans due
        WHERE domain = ANY(ARRAY[
            'telemetry_rollups', 'telemetry_network_rates',
            'telemetry_ping_rollups', 'system_metric_rollups'
        ])
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(ledger_after, ledger_before);
    let retained: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM telemetry_rollups
             WHERE client_id = 'due-quiet' AND bucket_secs = 60),
            (SELECT count(*) FROM telemetry_network_rates
             WHERE client_id = 'due-quiet' AND bucket_secs = 60),
            (SELECT count(*) FROM telemetry_ping_rollups
             WHERE series_id = $1 AND bucket_secs = 60),
            (SELECT count(*) FROM system_metric_rollups
             WHERE metric = 'due.quiet' AND bucket_secs = 60)
        "#,
    )
    .bind(series_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(retained, (1, 1, 1, 1));
    db.cleanup().await;
}

#[tokio::test]
async fn configured_rollup_prune_is_bounded_exact_and_cannot_be_disabled() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    insert_test_client(&db.pool, "prune-policy").await;
    let cutoff: DateTime<Utc> = sqlx::query_scalar(
        r#"
        SELECT (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
            - interval '1 day'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();

    for tier in TELEMETRY_HISTORY_TIERS {
        let metric = format!("system.retention.{}", tier.bucket_secs);
        insert_system_point(
            &db.pool,
            &metric,
            cutoff - ChronoDuration::seconds(i64::from(tier.bucket_secs)),
            tier.bucket_secs,
            1.0,
        )
        .await;
        insert_system_point(&db.pool, &metric, cutoff, tier.bucket_secs, 2.0).await;
    }
    let system_pruned = prune_domain(
        &db.pool,
        "system_metric_rollups",
        RetentionPolicy {
            enabled: true,
            prune_limit: 100,
            retention_days: 1,
        },
    )
    .await
    .unwrap();
    assert_eq!(system_pruned, TELEMETRY_HISTORY_TIERS.len() as u64);
    let retained_system: Vec<(i32, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT bucket_secs, bucket_start
        FROM system_metric_rollups
        WHERE metric LIKE 'system.retention.%'
        ORDER BY bucket_secs
        "#,
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(retained_system.len(), TELEMETRY_HISTORY_TIERS.len());
    for tier in TELEMETRY_HISTORY_TIERS {
        assert_eq!(
            retained_system
                .iter()
                .filter(|(bucket_secs, _)| *bucket_secs == tier.bucket_secs)
                .count(),
            1,
            "system-metric tier {} ignored the configured final horizon",
            tier.bucket_secs,
        );
        assert!(retained_system
            .iter()
            .any(
                |(bucket_secs, bucket_start)| *bucket_secs == tier.bucket_secs
                    && *bucket_start == cutoff
            ));
    }

    insert_resource_point(
        &db.pool,
        "prune-policy",
        cutoff - ChronoDuration::minutes(10),
        300,
        1.0,
    )
    .await;
    insert_resource_point(
        &db.pool,
        "prune-policy",
        cutoff - ChronoDuration::minutes(5),
        300,
        2.0,
    )
    .await;
    sqlx::query(
        r#"
        INSERT INTO history_retention_policies (
            domain, retention_days, prune_limit, enabled,
            metadata_only, export_enabled
        ) VALUES ('telemetry_rollups', 1, 1, TRUE, FALSE, TRUE)
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let first = process_telemetry_history_retention(&db.pool).await.unwrap();
    assert_eq!(first.rollups_pruned, 1);
    let remaining: Vec<DateTime<Utc>> = sqlx::query_scalar(
        r#"
        SELECT bucket_start FROM telemetry_rollups
        WHERE client_id = 'prune-policy'
        ORDER BY bucket_start
        "#,
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(remaining, vec![cutoff - ChronoDuration::minutes(5)]);

    insert_resource_point(
        &db.pool,
        "prune-policy",
        cutoff - ChronoDuration::minutes(20),
        300,
        3.0,
    )
    .await;
    let disable_error = sqlx::query(
        "UPDATE history_retention_policies SET enabled = FALSE WHERE domain = 'telemetry_rollups'",
    )
    .execute(&db.pool)
    .await
    .unwrap_err();
    assert_eq!(
        disable_error
            .as_database_error()
            .and_then(|database_error| database_error.constraint()),
        Some("history_retention_policies_bounded_domains_enabled_check"),
    );
    let second = process_telemetry_history_retention(&db.pool).await.unwrap();
    assert_eq!(second.rollups_pruned, 1);
    let remaining_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM telemetry_rollups WHERE client_id = 'prune-policy'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(remaining_count, 1);
    let policy = RetentionPolicy {
        enabled: true,
        prune_limit: 1,
        retention_days: 1,
    };
    assert!(
        prune_domain_has_remaining_work(&db.pool, "telemetry_rollups", policy)
            .await
            .unwrap(),
        "a bounded final owner must report StillDue while an eligible row remains",
    );
    assert_eq!(
        prune_domain(&db.pool, "telemetry_rollups", policy)
            .await
            .unwrap(),
        1,
    );
    assert!(
        !prune_domain_has_remaining_work(&db.pool, "telemetry_rollups", policy)
            .await
            .unwrap(),
        "the exact post-page frontier must report Current after the last row",
    );
    db.cleanup().await;
}
