use super::{process_telemetry_history_retention, prune_query, traffic_counter_prune_query};
use crate::test_support::PgWorkerTestDb;

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
            2048, 1024, 1024, 1024, 0.5, 0.5, 0.5, 0, 0,
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
        INSERT INTO telemetry_rollups (
            client_id, bucket_start, bucket_secs, sample_count,
            cpu_usage_sample_count, cpu_usage_avg, cpu_usage_max,
            cpu_cores_max, cpu_load_1_avg, cpu_load_1_sum, cpu_load_1_max,
            cpu_load_5_avg, cpu_load_5_sum, cpu_load_5_max, cpu_load_15_avg,
            cpu_load_15_sum, cpu_load_15_max, memory_total_bytes_max,
            memory_available_bytes_avg, memory_available_bytes_sum,
            memory_available_bytes_min, memory_used_ratio_avg,
            memory_used_ratio_sum, memory_used_ratio_max,
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
            2048, 1024, 1024, 1024, 0.5, 0.5, 0.5, 0, 0,
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

    let run = process_telemetry_history_retention(&db.pool).await.unwrap();
    assert!(run.resource_spans_merged >= 1);
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
    assert!(run.system_metric_promotion_conflicts >= 1);
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
