use super::{process_telemetry_history_retention, prune_query, traffic_counter_prune_query};
use crate::test_support::PgWorkerTestDb;

#[tokio::test]
async fn compaction_reaches_mergeable_rows_after_unmergeable_prefix() {
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
            cpu_cores_max, cpu_load_1_avg, cpu_load_1_max,
            cpu_load_5_avg, cpu_load_5_max, cpu_load_15_avg,
            cpu_load_15_max, memory_total_bytes_max,
            memory_available_bytes_avg, memory_available_bytes_min,
            memory_used_ratio_avg, memory_used_ratio_max,
            swap_sample_count, swap_total_bytes_max,
            swap_available_bytes_avg, swap_available_bytes_min,
            swap_used_ratio_avg, swap_used_ratio_max,
            disk_total_bytes_max, disk_available_bytes_avg,
            disk_available_bytes_min, disk_used_ratio_avg,
            disk_used_ratio_max, network_rx_bytes_max,
            network_tx_bytes_max, latest_observed_at
        )
        SELECT
            'compaction-fairness',
            date_trunc('minute', now()) - interval '36 hours'
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
            0, 0, 0, 0, 1024, 512, 512, 0.5, 0.5,
            1, 1024, 512, 512, 0.5, 0.5,
            2048, 1024, 1024, 0.5, 0.5, 0, 0,
            date_trunc('minute', now()) - interval '36 hours'
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
            cpu_cores_max, cpu_load_1_avg, cpu_load_1_max,
            cpu_load_5_avg, cpu_load_5_max, cpu_load_15_avg,
            cpu_load_15_max, memory_total_bytes_max,
            memory_available_bytes_avg, memory_available_bytes_min,
            memory_used_ratio_avg, memory_used_ratio_max,
            disk_total_bytes_max, disk_available_bytes_avg,
            disk_available_bytes_min, disk_used_ratio_avg,
            disk_used_ratio_max, network_rx_bytes_max,
            network_tx_bytes_max, latest_observed_at
        )
        SELECT
            'compaction-offset',
            date_trunc('minute', now()) - interval '40 hours'
                + series.minute_index * interval '1 minute',
            60, 1, 0, NULL, NULL, 1, 4, 4, 0, 0, 0, 0,
            1024, 512, 512, 0.5, 0.5,
            2048, 1024, 1024, 0.5, 0.5, 0, 0,
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
