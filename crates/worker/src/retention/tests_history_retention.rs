use super::{
    process_telemetry_history_retention, promote_network_rate_tier, promote_ping_tier,
    promote_resource_tier, promote_system_metric_tier, promotion_limits, prune_query,
    traffic_counter_prune_query,
};
use crate::test_support::PgWorkerTestDb;
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use tokio::sync::Barrier;

#[tokio::test]
async fn tier_promotion_reaches_old_rows_and_preserves_counts() {
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
        INSERT INTO system_metric_rollups (
            metric, bucket_start, bucket_secs, sample_count, value_sum,
            avg_value, max_value, latest_value, latest_observed_at
        ) VALUES
            ('worker.mixed', date_trunc('hour', now()) - interval '40 days',
                300, 5, 10, 2, 2, 2,
                date_trunc('hour', now()) - interval '40 days' + interval '4 minutes'),
            ('worker.mixed', date_trunc('hour', now()) - interval '40 days' + interval '5 minutes',
                60, 1, 4, 4, 4, 4,
                date_trunc('hour', now()) - interval '40 days' + interval '5 minutes'),
            ('worker.overlap', date_trunc('hour', now()) - interval '40 days',
                300, 5, 10, 2, 2, 2,
                date_trunc('hour', now()) - interval '40 days' + interval '4 minutes'),
            ('worker.overlap', date_trunc('hour', now()) - interval '40 days' + interval '1 minute',
                60, 1, 4, 4, 4, 4,
                date_trunc('hour', now()) - interval '40 days' + interval '1 minute')
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO system_metric_rollups (
            metric, bucket_start, bucket_secs, sample_count, value_sum,
            avg_value, max_value, latest_value, latest_observed_at
        ) SELECT 'worker.queue', bucket_start, 60, 1, value, value, value, value, bucket_start
        FROM (VALUES
            (date_trunc('hour', now()) - interval '40 days', 2.0::double precision),
            (date_trunc('hour', now()) - interval '40 days' + interval '1 minute', 4.0::double precision)
        ) sample(bucket_start, value)
        "#,
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
            network_rx_bytes_max,
            network_tx_bytes_max, latest_observed_at
        )
        SELECT
            'compaction-fairness',
            date_trunc('minute', now()) - interval '96 hours'
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
            1, 2048, 1024, 1024, 1024, 0.5, 0.5, 0.5, 0, 0,
            date_trunc('minute', now()) - interval '96 hours'
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
            network_rx_bytes_max,
            network_tx_bytes_max, latest_observed_at
        )
        SELECT
            'compaction-offset',
            date_trunc('minute', now()) - interval '40 hours'
                + series.minute_index * interval '1 minute',
            60, 1, 0, NULL, NULL, 1, 4, 4, 4, 0, 0, 0, 0, 0, 0,
            1024, 512, 512, 512, 0.5, 0.5, 0.5,
            1, 2048, 1024, 1024, 1024, 0.5, 0.5, 0.5, 0, 0,
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
        INSERT INTO telemetry_resource_latest
        SELECT source.*
        FROM telemetry_rollups source
        WHERE source.client_id = 'compaction-fairness'
        ORDER BY source.latest_observed_at DESC
        LIMIT 1
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
    let limited_ping = promote_ping_tier(&db.pool, 3_600, 1_800, 1, &lower_tiers, 1, 0)
        .await
        .unwrap();
    for limited in [
        promote_resource_tier(&db.pool, 3_600, 1_800, 1, &lower_tiers, 1)
            .await
            .unwrap(),
        promote_network_rate_tier(&db.pool, 3_600, 1_800, 1, &lower_tiers, 1)
            .await
            .unwrap(),
        limited_ping.promotion,
        promote_system_metric_tier(&db.pool, 3_600, 1_800, 1, &lower_tiers, 1)
            .await
            .unwrap(),
    ] {
        assert_eq!(limited.promoted, 0);
        assert_eq!(limited.source_rows, 0);
    }

    let mut resource_spans_merged = 0;
    let mut system_metric_promotion_conflicts = 0;
    // Exact predecessor seeding intentionally advances backlog one configured
    // tier per descending pass; three passes take old minute data through 1h.
    for _ in 0..3 {
        let run = process_telemetry_history_retention(&db.pool).await.unwrap();
        resource_spans_merged += run.resource_spans_merged;
        system_metric_promotion_conflicts += run.system_metric_promotion_conflicts;
    }
    assert!(resource_spans_merged >= 1);
    let retained: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM telemetry_rollups WHERE client_id = 'compaction-fairness'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(retained < 2004);
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
    let resource_minute_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM telemetry_rollups WHERE client_id = 'compaction-fairness' AND bucket_secs = 60",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(resource_minute_rows, 0);
    let system: (i32, i32, f64, f64) = sqlx::query_as(
        "SELECT bucket_secs, sample_count, value_sum, avg_value FROM system_metric_rollups WHERE metric = 'worker.queue'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(system, (3600, 2, 6.0, 3.0));
    let mixed: (i32, i32, f64, f64) = sqlx::query_as(
        "SELECT bucket_secs, sample_count, value_sum, avg_value FROM system_metric_rollups WHERE metric = 'worker.mixed'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(mixed, (3600, 6, 14.0, 14.0 / 6.0));
    let overlapping_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM system_metric_rollups WHERE metric = 'worker.overlap'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(overlapping_rows, 2);
    assert!(system_metric_promotion_conflicts >= 1);
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
        "SELECT sample_count, cpu_load_1_avg FROM telemetry_resource_latest WHERE client_id = 'compaction-fairness'",
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
    let mut promotion = tokio::spawn(async move {
        promote_ping_tier(&promotion_pool, 300, 60, 1, &[60], 20_000, 0).await
    });
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
    assert_eq!(skipped.promotion.conflicts, 0);
    assert_eq!(skipped.promotion.source_rows, 0);
    assert_eq!(skipped.next_series_cursor, Some(series_id));

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
    let child_skipped = promote_ping_tier(&db.pool, 300, 60, 1, &[60], 20_000, 0)
        .await
        .unwrap();
    assert_eq!(child_skipped.promotion.promoted, 0);
    assert_eq!(child_skipped.promotion.conflicts, 0);
    assert_eq!(child_skipped.promotion.source_rows, 0);
    assert_eq!(child_skipped.next_series_cursor, Some(series_id));
    child_holder.rollback().await.unwrap();

    let limited = promote_ping_tier(&db.pool, 300, 60, 1, &[60], 1, 0)
        .await
        .unwrap();
    assert_eq!(limited.promotion.promoted, 0);
    assert_eq!(limited.promotion.conflicts, 0);
    assert_eq!(limited.promotion.source_rows, 0);
    let rows_after_limit: (i64, i64) = sqlx::query_as(
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
    assert_eq!(rows_after_limit, (2, 0));

    let barrier = Arc::new(Barrier::new(2));
    let promote = |pool: sqlx::PgPool, barrier: Arc<Barrier>| async move {
        barrier.wait().await;
        promote_ping_tier(&pool, 300, 60, 1, &[60], 20_000, 0)
            .await
            .unwrap()
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
    assert_eq!(left.promotion.conflicts + right.promotion.conflicts, 0);
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

    sqlx::query(
        "INSERT INTO ping_targets (id, name, host, probe_kind) VALUES ('20000000-0000-0000-0000-000000000002', 'ping-empty-parent', '127.0.0.2', 'icmp')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let empty_series_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO telemetry_ping_series (client_id, target_id, generation)
        VALUES (
            'ping-lock-order',
            '20000000-0000-0000-0000-000000000002',
            1
        )
        RETURNING id
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let empty_batch = promote_ping_tier(&db.pool, 300, 60, 1, &[60], 6, series_id)
        .await
        .unwrap();
    assert_eq!(empty_batch.promotion.promoted, 0);
    assert_eq!(empty_batch.promotion.conflicts, 0);
    assert_eq!(empty_batch.promotion.source_rows, 0);
    assert_eq!(empty_batch.next_series_cursor, Some(empty_series_id));
    let wrapped_batch = promote_ping_tier(&db.pool, 300, 60, 1, &[60], 6, empty_series_id)
        .await
        .unwrap();
    assert_eq!(wrapped_batch.promotion.promoted, 0);
    assert_eq!(wrapped_batch.promotion.conflicts, 0);
    assert_eq!(wrapped_batch.promotion.source_rows, 0);
    assert_eq!(wrapped_batch.next_series_cursor, Some(series_id));
    db.cleanup().await;
}

#[tokio::test]
async fn promotion_seed_plans_skip_unrelated_history_tiers() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ('seed-plan', 'seed-plan', decode('', 'hex'), 'online')",
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
        )
        SELECT 'seed-plan', 'eth0', sample.bucket_start, sample.bucket_secs, 1,
            1, 1, 1, 1, 1, 1, 0, 0, sample.bucket_start
        FROM (
            SELECT date_trunc('hour', now()) - interval '180 days'
                    + minute_index * interval '1 minute' AS bucket_start,
                60 AS bucket_secs
            FROM generate_series(0, 49_999) minute(minute_index)
            UNION ALL
            SELECT date_trunc('hour', now()) - interval '180 days'
                    + sample_index * interval '5 minutes' AS bucket_start,
                300 AS bucket_secs
            FROM generate_series(0, 199) sample(sample_index)
        ) sample
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("ANALYZE telemetry_network_rates")
        .execute(&db.pool)
        .await
        .unwrap();
    let network_plan: Value = sqlx::query_scalar(
        r#"
        EXPLAIN (ANALYZE, FORMAT JSON)
        SELECT client_id, interface, bucket_start
        FROM telemetry_network_rates
        WHERE bucket_secs = 300
          AND bucket_start < date_trunc('day', now() - interval '1 day')
        ORDER BY bucket_start, client_id DESC, interface DESC
        LIMIT 20
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_seed_index_scan(&network_plan, "telemetry_network_rates_latest_idx", 40);

    sqlx::query(
        "INSERT INTO ping_targets (id, name, host, probe_kind) VALUES ('30000000-0000-0000-0000-000000000001', 'seed-plan', '127.0.0.1', 'icmp')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let series_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO telemetry_ping_series (client_id, target_id, generation)
        VALUES ('seed-plan', '30000000-0000-0000-0000-000000000001', 1)
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
        SELECT $1, sample.bucket_start, sample.bucket_secs, 1, 1,
            1, 1, 1, 1, 0, 0, 0, 'ok', sample.bucket_start
        FROM (
            SELECT date_trunc('hour', now()) - interval '180 days'
                    + minute_index * interval '1 minute' AS bucket_start,
                60 AS bucket_secs
            FROM generate_series(0, 49_999) minute(minute_index)
            UNION ALL
            SELECT date_trunc('hour', now()) - interval '180 days'
                    + sample_index * interval '5 minutes' AS bucket_start,
                300 AS bucket_secs
            FROM generate_series(0, 199) sample(sample_index)
        ) sample
        "#,
    )
    .bind(series_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("ANALYZE telemetry_ping_rollups")
        .execute(&db.pool)
        .await
        .unwrap();
    let ping_plan: Value = sqlx::query_scalar(
        r#"
        EXPLAIN (ANALYZE, FORMAT JSON)
        SELECT bucket_start
        FROM telemetry_ping_rollups
        WHERE series_id = $1
          AND bucket_secs = 300
          AND bucket_start < date_trunc('day', now() - interval '1 day')
        ORDER BY bucket_start
        LIMIT 20
        "#,
    )
    .bind(series_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_seed_index_scan(&ping_plan, "telemetry_ping_rollups_pkey", 20);
    db.cleanup().await;
}

#[tokio::test]
async fn mixed_tier_expansion_obeys_source_row_budget() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ('mixed-budget', 'mixed-budget', decode('', 'hex'), 'online')",
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
        )
        SELECT 'mixed-budget', 'eth0', source.bucket_start, source.bucket_secs, 1,
            1, 1, 1, 1, 1, 1, 0, 0, source.bucket_start
        FROM generate_series(0, 9) destination(group_index)
        CROSS JOIN LATERAL (
            SELECT date_trunc('hour', now()) - interval '40 days'
                + destination.group_index * interval '30 minutes' AS destination_start
        ) aligned
        CROSS JOIN LATERAL (
            SELECT aligned.destination_start
                    + minute_index * interval '1 minute' AS bucket_start,
                60 AS bucket_secs
            FROM generate_series(0, 14) minute(minute_index)
            UNION ALL
            SELECT aligned.destination_start
                    + minute_index * interval '1 minute' AS bucket_start,
                300 AS bucket_secs
            FROM (VALUES (15), (20), (25)) minute(minute_index)
        ) source
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let promoted = promote_network_rate_tier(&db.pool, 1_800, 300, 1, &[60, 300], 100)
        .await
        .unwrap();
    assert_eq!(promoted.promoted, 3);
    assert_eq!(promoted.conflicts, 0);
    assert_eq!(promoted.source_rows, 54);
    assert!(promoted.source_rows <= 100);
    let rows: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            count(*) FILTER (WHERE bucket_secs < 1800),
            count(*) FILTER (WHERE bucket_secs = 1800)
        FROM telemetry_network_rates
        WHERE client_id = 'mixed-budget'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(rows, (126, 3));
    db.cleanup().await;
}

fn assert_seed_index_scan(plan: &Value, index_name: &str, max_examined_rows: u64) {
    let node = find_index_plan_node(plan, index_name)
        .unwrap_or_else(|| panic!("expected bounded seed index {index_name} in {plan}"));
    let index_condition = node
        .get("Index Cond")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(index_condition.contains("bucket_secs"));
    assert!(index_condition.contains("bucket_start"));
    let actual_rows = node
        .get("Actual Rows")
        .and_then(Value::as_f64)
        .unwrap_or_default() as u64;
    let removed_rows = node
        .get("Rows Removed by Filter")
        .and_then(Value::as_f64)
        .unwrap_or_default() as u64;
    assert!(
        actual_rows + removed_rows <= max_examined_rows,
        "seed index examined {} rows, budget is {max_examined_rows}: {node}",
        actual_rows + removed_rows
    );
}

fn find_index_plan_node<'a>(value: &'a Value, index_name: &str) -> Option<&'a Value> {
    if value.get("Index Name").and_then(Value::as_str) == Some(index_name) {
        return Some(value);
    }
    if let Some(children) = value.get("Plans").and_then(Value::as_array) {
        for child in children {
            if let Some(found) = find_index_plan_node(child, index_name) {
                return Some(found);
            }
        }
    }
    if let Some(array) = value.as_array() {
        for child in array {
            if let Some(found) = find_index_plan_node(child, index_name) {
                return Some(found);
            }
        }
    }
    if let Some(object) = value.as_object() {
        for child in object.values() {
            if let Some(found) = find_index_plan_node(child, index_name) {
                return Some(found);
            }
        }
    }
    None
}

#[test]
fn tier_promotions_bound_candidates_before_grouping_and_locks() {
    let source = include_str!("history_retention.rs");
    for (start, end, domain) in [
        (
            "async fn promote_resource_tier(",
            "async fn promote_network_rate_rollups(",
            "resource",
        ),
        (
            "async fn promote_network_rate_tier(",
            "async fn promote_ping_rollups(",
            "network rate",
        ),
        (
            "async fn promote_ping_tier(",
            "async fn promote_system_metric_rollups(",
            "Ping",
        ),
        (
            "async fn promote_system_metric_tier(",
            "fn warn_promotion_conflicts(",
            "system metric",
        ),
    ] {
        let body_start = source.find(start).unwrap();
        let body_end = source[body_start..].find(end).unwrap() + body_start;
        let body = &source[body_start..body_end];
        let seed = body.find("seed_rows AS MATERIALIZED").unwrap();
        let candidate_keys = body.find("candidate_keys AS MATERIALIZED").unwrap();
        let seed_body = &body[seed..candidate_keys];
        let candidate_limit = seed_body.find("LIMIT $5").unwrap();
        assert!(seed_body.contains("bucket_secs = $2"));
        assert!(seed_body.contains("bucket_start < to_timestamp"));
        assert!(!seed_body.contains("extract(epoch FROM bucket_start)"));
        let expansion = body.find("expanded_rows AS MATERIALIZED").unwrap();
        let first_group = body.find("GROUP BY").unwrap();
        assert!(
            seed + candidate_limit < first_group,
            "{domain} promotion must limit source candidates before grouping"
        );
        let expansion_limit = body[expansion..first_group].find("LIMIT $6").unwrap();
        assert!(expansion + expansion_limit < first_group);
        assert!(body[expansion..first_group].contains("unnest($7::integer[])"));
        assert!(body.contains("FOR UPDATE OF row SKIP LOCKED"));
        assert!(body.contains("(SELECT count(*)::bigint FROM deleted) AS source_rows"));
    }

    let ping_start = source.find("async fn promote_ping_tier(").unwrap();
    let ping_end = source[ping_start..]
        .find("async fn promote_system_metric_rollups(")
        .unwrap()
        + ping_start;
    let ping = &source[ping_start..ping_end];
    let parent = ping
        .find("locked_candidate_series AS MATERIALIZED")
        .unwrap();
    let child_probe = ping.find("seed_rows AS MATERIALIZED").unwrap();
    let child_lock = ping.find("locked_source AS MATERIALIZED").unwrap();
    assert!(parent < child_probe && child_probe < child_lock);
    assert!(ping[parent..child_probe].contains("ORDER BY candidate.pass, series.id"));
    assert!(ping[parent..child_probe].contains("FOR NO KEY UPDATE OF series SKIP LOCKED"));
}

#[test]
fn tier_promotion_limits_bound_seed_and_expansion_rows() {
    for (source, destination) in [
        (60, 300),
        (300, 1_800),
        (1_800, 3_600),
        (3_600, 10_800),
        (10_800, 21_600),
        (21_600, 86_400),
    ] {
        let limits = promotion_limits(destination, source, 20_000).unwrap();
        assert!(limits.group_limit <= 3_000);
        assert!(limits.group_limit * limits.group_source_limit <= 20_000);
        assert!(limits.seed_row_limit <= 20_000);
        assert_eq!(limits.seed_rows_per_group, i64::from(destination / source));
        assert_eq!(limits.group_source_limit, i64::from(destination / 60 + 1));
    }
}

#[test]
fn telemetry_pruning_is_bounded_and_concurrency_safe() {
    let query = prune_query("telemetry_rollups");
    assert!(query.contains("LIMIT $2"));
    assert!(query.contains("FOR UPDATE SKIP LOCKED"));
    assert!(query.contains("bucket_start"));
    assert!(!query.contains("DELETE FROM telemetry_network_rates"));
}

#[test]
fn telemetry_pruning_retains_bucket_crossing_cutoff() {
    let query = prune_query("telemetry_rollups");

    assert!(query.contains(
        "bucket_start\n                + make_interval(secs => GREATEST(bucket_secs, 1)) <= ("
    ));
    assert!(!query.contains("WHERE bucket_start < ("));
}

#[test]
fn traffic_counter_pruning_preserves_each_stream_baseline() {
    let query = traffic_counter_prune_query();
    assert!(query.contains("newer.client_id = sample.client_id"));
    assert!(query.contains("newer.source_kind = sample.source_kind"));
    assert!(query.contains("newer.interface = sample.interface"));
    assert!(query.contains("newer.observed_at > sample.observed_at"));
    assert!(query.contains("LIMIT $2"));
    assert!(query.contains("FOR UPDATE OF sample SKIP LOCKED"));
}
