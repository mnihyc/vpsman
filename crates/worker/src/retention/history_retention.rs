use anyhow::Result;
use sqlx::{PgPool, Row};
use vpsman_common::{
    DEFAULT_TELEMETRY_RETENTION_PRUNE_LIMIT, DEFAULT_TELEMETRY_ROLLUP_RETENTION_DAYS,
    DEFAULT_TELEMETRY_SAMPLE_RETENTION_DAYS, MIN_TRAFFIC_COUNTER_RETENTION_DAYS,
};

const TELEMETRY_COMPACTION_LIMIT: i64 = 2_000;
// Ping results may arrive up to 65 minutes after they were checked. Compact
// only fully settled history so a late accepted minute can never overlap a
// previously merged span.
const TELEMETRY_COMPACTION_MIN_AGE_SECS: i32 = 2 * 60 * 60;
const TELEMETRY_COMPACTION_MAX_SPAN_SECS: i32 = 24 * 60 * 60;

#[derive(Clone, Copy)]
struct RetentionPolicy {
    enabled: bool,
    prune_limit: i32,
    retention_days: i32,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TelemetryHistoryRetentionRun {
    pub(crate) network_rate_spans_merged: u64,
    pub(crate) network_rates_pruned: u64,
    pub(crate) ping_spans_merged: u64,
    pub(crate) ping_rollups_pruned: u64,
    pub(crate) resource_spans_merged: u64,
    pub(crate) rollups_pruned: u64,
    pub(crate) samples_pruned: u64,
    pub(crate) traffic_counter_samples_pruned: u64,
}

pub(crate) async fn process_telemetry_history_retention(
    pool: &PgPool,
) -> Result<TelemetryHistoryRetentionRun> {
    let samples = load_policy(pool, "telemetry_samples").await?;
    let rollups = load_policy(pool, "telemetry_rollups").await?;
    let network_rates = load_policy(pool, "telemetry_network_rates").await?;
    let ping_rollups = load_policy(pool, "telemetry_ping_rollups").await?;
    let traffic_counter_samples = load_policy(pool, "traffic_counter_samples").await?;
    let resource_spans_merged = compact_resource_rollups(pool).await?;
    let network_rate_spans_merged = compact_network_rate_rollups(pool).await?;
    let ping_spans_merged = compact_ping_rollups(pool).await?;
    Ok(TelemetryHistoryRetentionRun {
        resource_spans_merged,
        network_rate_spans_merged,
        ping_spans_merged,
        samples_pruned: prune_domain(pool, "telemetry_samples", samples).await?,
        rollups_pruned: prune_domain(pool, "telemetry_rollups", rollups).await?,
        network_rates_pruned: prune_domain(pool, "telemetry_network_rates", network_rates).await?,
        ping_rollups_pruned: prune_domain(pool, "telemetry_ping_rollups", ping_rollups).await?,
        traffic_counter_samples_pruned: prune_domain(
            pool,
            "traffic_counter_samples",
            traffic_counter_samples,
        )
        .await?,
    })
}

async fn compact_resource_rollups(pool: &PgPool) -> Result<u64> {
    let result = sqlx::query(
        r#"
        WITH candidate_edges AS (
            SELECT
                head.ctid AS head_ctid,
                tail.next_ctid,
                head.client_id,
                head.bucket_start,
                head.bucket_secs,
                head.bucket_secs + tail.bucket_secs AS merged_bucket_secs,
                LEAST(
                    head.sample_count::bigint + tail.sample_count::bigint,
                    2147483647
                )::integer AS merged_sample_count,
                LEAST(
                    head.cpu_usage_sample_count::bigint
                        + tail.cpu_usage_sample_count::bigint,
                    2147483647
                )::integer AS merged_cpu_usage_sample_count,
                LEAST(
                    head.swap_sample_count::bigint
                        + tail.swap_sample_count::bigint,
                    2147483647
                )::integer AS merged_swap_sample_count,
                LEAST(
                    head.connections_sample_count::bigint
                        + tail.connections_sample_count::bigint,
                    2147483647
                )::integer AS merged_connections_sample_count,
                tail.connections_observed_at AS merged_connections_observed_at,
                tail.latest_observed_at AS merged_latest_observed_at,
                GREATEST(head.updated_at, tail.updated_at) AS merged_updated_at,
                row_number() OVER (
                    PARTITION BY client_id
                    ORDER BY head.bucket_start, head.bucket_secs, tail.bucket_secs
                ) AS candidate_row
            FROM telemetry_rollups head
            JOIN LATERAL (
                SELECT
                    next.ctid AS next_ctid,
                    next.bucket_secs,
                    next.sample_count,
                    next.cpu_usage_sample_count,
                    next.cpu_usage_avg,
                    next.cpu_usage_max,
                    next.cpu_cores_max,
                    next.cpu_load_1_avg,
                    next.cpu_load_1_max,
                    next.cpu_load_5_avg,
                    next.cpu_load_5_max,
                    next.cpu_load_15_avg,
                    next.cpu_load_15_max,
                    next.memory_total_bytes_max,
                    next.memory_available_bytes_avg,
                    next.memory_available_bytes_min,
                    next.memory_used_ratio_avg,
                    next.memory_used_ratio_max,
                    next.swap_sample_count,
                    next.swap_total_bytes_max,
                    next.swap_available_bytes_avg,
                    next.swap_available_bytes_min,
                    next.swap_used_ratio_avg,
                    next.swap_used_ratio_max,
                    next.disk_total_bytes_max,
                    next.disk_available_bytes_avg,
                    next.disk_available_bytes_min,
                    next.disk_used_ratio_avg,
                    next.disk_used_ratio_max,
                    next.network_rx_bytes_max,
                    next.network_tx_bytes_max,
                    next.connections_sample_count,
                    next.tcp_sockets_latest,
                    next.udp_sockets_latest,
                    next.connections_observed_at,
                    next.latest_observed_at,
                    next.updated_at
                FROM telemetry_rollups next
                WHERE next.client_id = head.client_id
                  AND next.bucket_start
                        = head.bucket_start + make_interval(secs => head.bucket_secs)
                  AND next.bucket_start + make_interval(secs => next.bucket_secs)
                        <= now() - make_interval(secs => $1)
                  AND head.bucket_secs + next.bucket_secs <= $3
                  AND head.sample_count::bigint * next.bucket_secs::bigint
                        = next.sample_count::bigint * head.bucket_secs::bigint
                  AND head.cpu_usage_sample_count::bigint * next.bucket_secs::bigint
                        = next.cpu_usage_sample_count::bigint * head.bucket_secs::bigint
                  AND head.swap_sample_count::bigint * next.bucket_secs::bigint
                        = next.swap_sample_count::bigint * head.bucket_secs::bigint
                  AND head.connections_sample_count::bigint * next.bucket_secs::bigint
                        = next.connections_sample_count::bigint * head.bucket_secs::bigint
                  AND head.cpu_usage_avg IS NOT DISTINCT FROM next.cpu_usage_avg
                  AND head.cpu_usage_max IS NOT DISTINCT FROM next.cpu_usage_max
                  AND head.cpu_cores_max = next.cpu_cores_max
                  AND head.cpu_load_1_avg = next.cpu_load_1_avg
                  AND head.cpu_load_1_max = next.cpu_load_1_max
                  AND head.cpu_load_5_avg = next.cpu_load_5_avg
                  AND head.cpu_load_5_max = next.cpu_load_5_max
                  AND head.cpu_load_15_avg = next.cpu_load_15_avg
                  AND head.cpu_load_15_max = next.cpu_load_15_max
                  AND head.memory_total_bytes_max = next.memory_total_bytes_max
                  AND head.memory_available_bytes_avg = next.memory_available_bytes_avg
                  AND head.memory_available_bytes_min = next.memory_available_bytes_min
                  AND head.memory_used_ratio_avg = next.memory_used_ratio_avg
                  AND head.memory_used_ratio_max = next.memory_used_ratio_max
                  AND head.swap_total_bytes_max IS NOT DISTINCT FROM next.swap_total_bytes_max
                  AND head.swap_available_bytes_avg
                        IS NOT DISTINCT FROM next.swap_available_bytes_avg
                  AND head.swap_available_bytes_min
                        IS NOT DISTINCT FROM next.swap_available_bytes_min
                  AND head.swap_used_ratio_avg
                        IS NOT DISTINCT FROM next.swap_used_ratio_avg
                  AND head.swap_used_ratio_max
                        IS NOT DISTINCT FROM next.swap_used_ratio_max
                  AND head.disk_total_bytes_max = next.disk_total_bytes_max
                  AND head.disk_available_bytes_avg = next.disk_available_bytes_avg
                  AND head.disk_available_bytes_min = next.disk_available_bytes_min
                  AND head.disk_used_ratio_avg = next.disk_used_ratio_avg
                  AND head.disk_used_ratio_max = next.disk_used_ratio_max
                  AND head.network_rx_bytes_max = next.network_rx_bytes_max
                  AND head.network_tx_bytes_max = next.network_tx_bytes_max
                  AND head.tcp_sockets_latest IS NOT DISTINCT FROM next.tcp_sockets_latest
                  AND head.udp_sockets_latest IS NOT DISTINCT FROM next.udp_sockets_latest
                  AND head.latest_observed_at
                        - (
                            head.bucket_start
                            + make_interval(secs => head.bucket_secs - 60)
                        )
                        = next.latest_observed_at
                        - (
                            next.bucket_start
                            + make_interval(secs => next.bucket_secs - 60)
                        )
                  AND (
                        head.connections_observed_at IS NULL
                        OR head.connections_observed_at
                            - (
                                head.bucket_start
                                + make_interval(secs => head.bucket_secs - 60)
                            )
                            = next.connections_observed_at
                            - (
                                next.bucket_start
                                + make_interval(secs => next.bucket_secs - 60)
                            )
                  )
                  AND NOT EXISTS (
                        SELECT 1
                        FROM telemetry_rollups conflict
                        WHERE conflict.client_id = head.client_id
                          AND conflict.bucket_start = head.bucket_start
                          AND conflict.bucket_secs = head.bucket_secs + next.bucket_secs
                  )
                ORDER BY next.bucket_secs
                LIMIT 1
            ) tail ON TRUE
            WHERE head.bucket_secs >= 60
              AND head.bucket_secs % 60 = 0
              AND head.bucket_start + make_interval(secs => head.bucket_secs)
                    <= now() - make_interval(secs => $1)
        ), selected AS (
            SELECT *
            FROM candidate_edges
            WHERE mod(
                    candidate_row
                        + floor(extract(epoch FROM now()) / 60)::bigint,
                    2
                  ) = 0
            ORDER BY client_id, bucket_start, bucket_secs
            LIMIT $2
        ), locked AS (
            SELECT selected.*
            FROM selected
            JOIN telemetry_rollups head ON head.ctid = selected.head_ctid
            JOIN telemetry_rollups next ON next.ctid = selected.next_ctid
            FOR UPDATE OF head, next SKIP LOCKED
        ), deleted AS (
            DELETE FROM telemetry_rollups stored
            USING locked
            WHERE stored.ctid = locked.next_ctid
            RETURNING stored.ctid
        )
        UPDATE telemetry_rollups stored
        SET
            bucket_secs = locked.merged_bucket_secs,
            sample_count = locked.merged_sample_count,
            cpu_usage_sample_count = locked.merged_cpu_usage_sample_count,
            swap_sample_count = locked.merged_swap_sample_count,
            connections_sample_count = locked.merged_connections_sample_count,
            connections_observed_at = locked.merged_connections_observed_at,
            latest_observed_at = locked.merged_latest_observed_at,
            updated_at = locked.merged_updated_at
        FROM locked
        WHERE stored.ctid = locked.head_ctid
          AND EXISTS (SELECT 1 FROM deleted WHERE deleted.ctid = locked.next_ctid)
        "#,
    )
    .bind(TELEMETRY_COMPACTION_MIN_AGE_SECS)
    .bind(TELEMETRY_COMPACTION_LIMIT)
    .bind(TELEMETRY_COMPACTION_MAX_SPAN_SECS)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

async fn compact_network_rate_rollups(pool: &PgPool) -> Result<u64> {
    let result = sqlx::query(
        r#"
        WITH candidate_edges AS (
            SELECT
                head.ctid AS head_ctid,
                tail.next_ctid,
                head.client_id,
                head.interface,
                head.bucket_start,
                head.bucket_secs,
                head.bucket_secs + tail.bucket_secs AS merged_bucket_secs,
                LEAST(
                    head.sample_count::bigint + tail.sample_count::bigint,
                    2147483647
                )::integer AS merged_sample_count,
                GREATEST(head.updated_at, tail.updated_at) AS merged_updated_at,
                row_number() OVER (
                    PARTITION BY head.client_id, head.interface
                    ORDER BY head.bucket_start, head.bucket_secs, tail.bucket_secs
                ) AS candidate_row
            FROM telemetry_network_rates head
            JOIN LATERAL (
                SELECT
                    next.ctid AS next_ctid,
                    next.bucket_secs,
                    next.sample_count,
                    next.updated_at
                FROM telemetry_network_rates next
                WHERE next.client_id = head.client_id
                  AND next.interface = head.interface
                  AND next.bucket_start
                        = head.bucket_start + make_interval(secs => head.bucket_secs)
                  AND next.bucket_start + make_interval(secs => next.bucket_secs)
                        <= now() - make_interval(secs => $1)
                  AND head.bucket_secs + next.bucket_secs <= $3
                  AND head.sample_count::bigint * next.bucket_secs::bigint
                        = next.sample_count::bigint * head.bucket_secs::bigint
                  AND head.rx_bytes_avg = next.rx_bytes_avg
                  AND head.tx_bytes_avg = next.tx_bytes_avg
                  AND head.rx_bytes_last = next.rx_bytes_last
                  AND head.tx_bytes_last = next.tx_bytes_last
                  AND head.rx_counter_epoch = next.rx_counter_epoch
                  AND head.tx_counter_epoch = next.tx_counter_epoch
                  AND NOT EXISTS (
                        SELECT 1
                        FROM telemetry_network_rates conflict
                        WHERE conflict.client_id = head.client_id
                          AND conflict.interface = head.interface
                          AND conflict.bucket_start = head.bucket_start
                          AND conflict.bucket_secs = head.bucket_secs + next.bucket_secs
                  )
                ORDER BY next.bucket_secs
                LIMIT 1
            ) tail ON TRUE
            WHERE head.bucket_secs >= 60
              AND head.bucket_secs % 60 = 0
              AND head.bucket_start + make_interval(secs => head.bucket_secs)
                    <= now() - make_interval(secs => $1)
        ), selected AS (
            SELECT *
            FROM candidate_edges
            WHERE mod(
                    candidate_row
                        + floor(extract(epoch FROM now()) / 60)::bigint,
                    2
                  ) = 0
            ORDER BY client_id, interface, bucket_start, bucket_secs
            LIMIT $2
        ), locked AS (
            SELECT selected.*
            FROM selected
            JOIN telemetry_network_rates head ON head.ctid = selected.head_ctid
            JOIN telemetry_network_rates next ON next.ctid = selected.next_ctid
            FOR UPDATE OF head, next SKIP LOCKED
        ), deleted AS (
            DELETE FROM telemetry_network_rates stored
            USING locked
            WHERE stored.ctid = locked.next_ctid
            RETURNING stored.ctid
        )
        UPDATE telemetry_network_rates stored
        SET
            bucket_secs = locked.merged_bucket_secs,
            sample_count = locked.merged_sample_count,
            updated_at = locked.merged_updated_at
        FROM locked
        WHERE stored.ctid = locked.head_ctid
          AND EXISTS (SELECT 1 FROM deleted WHERE deleted.ctid = locked.next_ctid)
        "#,
    )
    .bind(TELEMETRY_COMPACTION_MIN_AGE_SECS)
    .bind(TELEMETRY_COMPACTION_LIMIT)
    .bind(TELEMETRY_COMPACTION_MAX_SPAN_SECS)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

async fn compact_ping_rollups(pool: &PgPool) -> Result<u64> {
    let result = sqlx::query(
        r#"
        WITH candidate_edges AS (
            SELECT
                head.ctid AS head_ctid,
                tail.next_ctid,
                head.client_id,
                head.target_id,
                head.generation,
                head.bucket_start,
                head.bucket_secs,
                head.bucket_secs + tail.bucket_secs AS merged_bucket_secs,
                LEAST(
                    head.sample_count::bigint + tail.sample_count::bigint,
                    2147483647
                )::integer AS merged_sample_count,
                LEAST(
                    head.success_count::bigint + tail.success_count::bigint,
                    2147483647
                )::integer AS merged_success_count,
                tail.latest_checked_at AS merged_latest_checked_at,
                GREATEST(head.updated_at, tail.updated_at) AS merged_updated_at,
                row_number() OVER (
                    PARTITION BY head.client_id, head.target_id, head.generation
                    ORDER BY head.bucket_start, head.bucket_secs, tail.bucket_secs
                ) AS candidate_row
            FROM telemetry_ping_rollups head
            JOIN LATERAL (
                SELECT
                    next.ctid AS next_ctid,
                    next.bucket_secs,
                    next.sample_count,
                    next.success_count,
                    next.latest_checked_at,
                    next.updated_at
                FROM telemetry_ping_rollups next
                WHERE next.client_id = head.client_id
                  AND next.target_id = head.target_id
                  AND next.generation = head.generation
                  AND next.bucket_start
                        = head.bucket_start + make_interval(secs => head.bucket_secs)
                  AND next.bucket_start + make_interval(secs => next.bucket_secs)
                        <= now() - make_interval(secs => $1)
                  AND head.bucket_secs + next.bucket_secs <= $3
                  AND head.sample_count::bigint * next.bucket_secs::bigint
                        = next.sample_count::bigint * head.bucket_secs::bigint
                  AND head.success_count::bigint * next.bucket_secs::bigint
                        = next.success_count::bigint * head.bucket_secs::bigint
                  AND head.latency_avg_ms IS NOT DISTINCT FROM next.latency_avg_ms
                  AND head.latency_min_ms IS NOT DISTINCT FROM next.latency_min_ms
                  AND head.latency_max_ms IS NOT DISTINCT FROM next.latency_max_ms
                  AND head.loss_ratio_avg = next.loss_ratio_avg
                  AND head.loss_ratio_max = next.loss_ratio_max
                  AND head.latest_status = next.latest_status
                  AND head.latest_reason IS NOT DISTINCT FROM next.latest_reason
                  AND head.latest_checked_at
                        - (
                            head.bucket_start
                            + make_interval(secs => head.bucket_secs - 60)
                        )
                        = next.latest_checked_at
                        - (
                            next.bucket_start
                            + make_interval(secs => next.bucket_secs - 60)
                        )
                  AND NOT EXISTS (
                        SELECT 1
                        FROM telemetry_ping_rollups conflict
                        WHERE conflict.client_id = head.client_id
                          AND conflict.target_id = head.target_id
                          AND conflict.generation = head.generation
                          AND conflict.bucket_start = head.bucket_start
                          AND conflict.bucket_secs = head.bucket_secs + next.bucket_secs
                  )
                ORDER BY next.bucket_secs
                LIMIT 1
            ) tail ON TRUE
            WHERE head.bucket_secs >= 60
              AND head.bucket_secs % 60 = 0
              AND head.bucket_start + make_interval(secs => head.bucket_secs)
                    <= now() - make_interval(secs => $1)
        ), selected AS (
            SELECT *
            FROM candidate_edges
            WHERE mod(
                    candidate_row
                        + floor(extract(epoch FROM now()) / 60)::bigint,
                    2
                  ) = 0
            ORDER BY client_id, target_id, generation, bucket_start, bucket_secs
            LIMIT $2
        ), locked AS (
            SELECT selected.*
            FROM selected
            JOIN telemetry_ping_rollups head ON head.ctid = selected.head_ctid
            JOIN telemetry_ping_rollups next ON next.ctid = selected.next_ctid
            FOR UPDATE OF head, next SKIP LOCKED
        ), deleted AS (
            DELETE FROM telemetry_ping_rollups stored
            USING locked
            WHERE stored.ctid = locked.next_ctid
            RETURNING stored.ctid
        )
        UPDATE telemetry_ping_rollups stored
        SET
            bucket_secs = locked.merged_bucket_secs,
            sample_count = locked.merged_sample_count,
            success_count = locked.merged_success_count,
            latest_checked_at = locked.merged_latest_checked_at,
            updated_at = locked.merged_updated_at
        FROM locked
        WHERE stored.ctid = locked.head_ctid
          AND EXISTS (SELECT 1 FROM deleted WHERE deleted.ctid = locked.next_ctid)
        "#,
    )
    .bind(TELEMETRY_COMPACTION_MIN_AGE_SECS)
    .bind(TELEMETRY_COMPACTION_LIMIT)
    .bind(TELEMETRY_COMPACTION_MAX_SPAN_SECS)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

async fn load_policy(pool: &PgPool, domain: &str) -> Result<RetentionPolicy> {
    let minimum_retention_days = if domain == "traffic_counter_samples" {
        MIN_TRAFFIC_COUNTER_RETENTION_DAYS
    } else {
        1
    };
    let row = sqlx::query(
        r#"
        SELECT retention_days, prune_limit, enabled
        FROM history_retention_policies
        WHERE domain = $1
        "#,
    )
    .bind(domain)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(RetentionPolicy {
            enabled: true,
            prune_limit: DEFAULT_TELEMETRY_RETENTION_PRUNE_LIMIT,
            retention_days: if domain == "telemetry_samples" {
                DEFAULT_TELEMETRY_SAMPLE_RETENTION_DAYS
            } else {
                DEFAULT_TELEMETRY_ROLLUP_RETENTION_DAYS
            },
        });
    };
    Ok(RetentionPolicy {
        enabled: row.try_get("enabled")?,
        prune_limit: row.try_get::<i32, _>("prune_limit")?.clamp(1, 100_000),
        retention_days: row
            .try_get::<i32, _>("retention_days")?
            .clamp(minimum_retention_days, 3_650),
    })
}

async fn prune_domain(pool: &PgPool, domain: &str, policy: RetentionPolicy) -> Result<u64> {
    if !policy.enabled {
        return Ok(0);
    }
    let query = match domain {
        "telemetry_samples" => sample_prune_query(),
        "telemetry_rollups" => prune_query("telemetry_rollups"),
        "telemetry_network_rates" => prune_query("telemetry_network_rates"),
        "telemetry_ping_rollups" => prune_query("telemetry_ping_rollups"),
        "traffic_counter_samples" => traffic_counter_prune_query(),
        _ => return Ok(0),
    };
    let result = sqlx::query(&query)
        .bind(policy.retention_days)
        .bind(policy.prune_limit)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

fn sample_prune_query() -> String {
    r#"
        WITH candidates AS (
            SELECT ctid
            FROM telemetry_samples
            WHERE observed_at < (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
            ) - make_interval(days => $1)
            ORDER BY observed_at ASC
            LIMIT $2
            FOR UPDATE SKIP LOCKED
        )
        DELETE FROM telemetry_samples
        WHERE ctid IN (SELECT ctid FROM candidates)
        "#
    .to_string()
}

fn prune_query(table: &str) -> String {
    format!(
        r#"
        WITH candidates AS (
            SELECT ctid
            FROM {table}
            WHERE bucket_start
                + make_interval(secs => GREATEST(bucket_secs, 1)) <= (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
            ) - make_interval(days => $1)
            ORDER BY bucket_start ASC
            LIMIT $2
            FOR UPDATE SKIP LOCKED
        )
        DELETE FROM {table}
        WHERE ctid IN (SELECT ctid FROM candidates)
        "#
    )
}

fn traffic_counter_prune_query() -> String {
    r#"
        WITH candidates AS (
            SELECT sample.ctid
            FROM traffic_counter_samples sample
            WHERE sample.observed_at < (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
            ) - make_interval(days => $1)
              AND EXISTS (
                  SELECT 1
                  FROM traffic_counter_samples newer
                  WHERE newer.client_id = sample.client_id
                    AND newer.source_kind = sample.source_kind
                    AND newer.interface = sample.interface
                    AND newer.observed_at < (
                        date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                    ) - make_interval(days => $1)
                    AND newer.observed_at > sample.observed_at
              )
            ORDER BY
                sample.observed_at ASC,
                sample.client_id ASC,
                sample.source_kind ASC,
                sample.interface ASC
            LIMIT $2
            FOR UPDATE OF sample SKIP LOCKED
        )
        DELETE FROM traffic_counter_samples
        WHERE ctid IN (SELECT ctid FROM candidates)
        "#
    .to_string()
}

#[cfg(test)]
#[path = "tests_history_retention.rs"]
mod tests;
