use crate::{
    network_observation_retention::{
        process_network_observation_retention, NetworkObservationRetentionPolicy,
    },
    traffic_retention::process_traffic_retention,
};
use anyhow::{Context, Result};
use sqlx::{PgPool, Row};
use std::sync::atomic::{AtomicI64, Ordering};
use vpsman_common::{
    DEFAULT_NETWORK_OBSERVATION_RETENTION_PRUNE_LIMIT, DEFAULT_TELEMETRY_RETENTION_PRUNE_LIMIT,
    DEFAULT_TELEMETRY_ROLLUP_RETENTION_DAYS, DEFAULT_TELEMETRY_SAMPLE_RETENTION_DAYS,
    MIN_TRAFFIC_COUNTER_RETENTION_DAYS, TELEMETRY_HISTORY_TIERS,
};

const TELEMETRY_PROMOTION_GROUP_LIMIT: i64 = 3_000;
const TELEMETRY_PROMOTION_SOURCE_ROW_LIMIT: i64 = 20_000;
static PING_PROMOTION_SERIES_CURSOR: AtomicI64 = AtomicI64::new(0);

// Promotion seeds only the immediate predecessor tier so its first range scan
// stays index-bounded. Because tiers run from coarse to fine, an old fine-tier
// backlog advances one configured tier per scheduled run and converges without
// ever reopening an unbounded scan across every lower tier.

#[derive(Clone, Copy, Debug, Default)]
struct PromotionResult {
    promoted: u64,
    conflicts: u64,
    source_rows: u64,
}

#[derive(Clone, Copy, Debug)]
struct PromotionLimits {
    group_limit: i64,
    group_source_limit: i64,
    seed_row_limit: i64,
    seed_rows_per_group: i64,
}

#[derive(Clone, Copy, Debug, Default)]
struct PingPromotionResult {
    promotion: PromotionResult,
    next_series_cursor: Option<i64>,
}

fn promotion_limits(
    destination_secs: i32,
    source_bucket_secs: i32,
    source_row_limit: i64,
) -> Option<PromotionLimits> {
    let minimum_bucket_secs = i64::from(TELEMETRY_HISTORY_TIERS[0].bucket_secs);
    let destination_secs = i64::from(destination_secs);
    let source_bucket_secs = i64::from(source_bucket_secs);
    if source_row_limit <= 0
        || source_bucket_secs < minimum_bucket_secs
        || destination_secs <= source_bucket_secs
        || destination_secs % source_bucket_secs != 0
        || destination_secs % minimum_bucket_secs != 0
    {
        return None;
    }
    // One extra row proves that a destination group cannot be a valid,
    // non-overlapping sequence of minimum-width source buckets. Dividing the
    // global budget by this per-key cap bounds expansion before grouping/locks.
    let group_source_limit = destination_secs / minimum_bucket_secs + 1;
    let group_limit = TELEMETRY_PROMOTION_GROUP_LIMIT
        .min(source_row_limit / group_source_limit)
        .max(0);
    if group_limit == 0 {
        return None;
    }
    let seed_rows_per_group = destination_secs / source_bucket_secs;
    Some(PromotionLimits {
        group_limit,
        group_source_limit,
        seed_row_limit: group_limit.saturating_mul(seed_rows_per_group),
        seed_rows_per_group,
    })
}

#[derive(Clone, Copy)]
struct RetentionPolicy {
    enabled: bool,
    prune_limit: i32,
    retention_days: i32,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TelemetryHistoryRetentionRun {
    pub(crate) network_rate_spans_merged: u64,
    pub(crate) network_rate_promotion_conflicts: u64,
    pub(crate) network_rates_pruned: u64,
    pub(crate) ping_spans_merged: u64,
    pub(crate) ping_promotion_conflicts: u64,
    pub(crate) ping_rollups_pruned: u64,
    pub(crate) resource_spans_merged: u64,
    pub(crate) resource_promotion_conflicts: u64,
    pub(crate) rollups_pruned: u64,
    pub(crate) samples_pruned: u64,
    pub(crate) system_metric_spans_merged: u64,
    pub(crate) system_metric_promotion_conflicts: u64,
    pub(crate) system_metric_rollups_pruned: u64,
    pub(crate) ping_facts_pruned: u64,
    pub(crate) traffic_counter_samples_pruned: u64,
    pub(crate) traffic_raw_rows_promoted: u64,
    pub(crate) traffic_rollup_rows_promoted: u64,
    pub(crate) traffic_rollup_rows_pruned: u64,
    pub(crate) traffic_promotion_conflicts: u64,
    pub(crate) network_observation_source_rows_promoted: u64,
    pub(crate) network_observation_destination_rows_written: u64,
    pub(crate) network_observation_destination_conflicts: u64,
    pub(crate) network_observation_expired_exact_rows_pruned: u64,
    pub(crate) network_observation_expired_rollup_rows_pruned: u64,
    pub(crate) network_observation_inactive_latest_pruned: u64,
    pub(crate) network_observation_inactive_series_pruned: u64,
}

pub(crate) async fn process_telemetry_history_retention(
    pool: &PgPool,
) -> Result<TelemetryHistoryRetentionRun> {
    let samples = load_policy(pool, "telemetry_samples").await?;
    let rollups = load_policy(pool, "telemetry_rollups").await?;
    let network_rates = load_policy(pool, "telemetry_network_rates").await?;
    let ping_rollups = load_policy(pool, "telemetry_ping_rollups").await?;
    let system_metric_rollups = load_policy(pool, "system_metric_rollups").await?;
    let network_observations = load_policy(pool, "network_observations").await?;
    let resource_promotion = promote_resource_rollups(pool)
        .await
        .context("promoting resource history tiers")?;
    let network_rate_promotion = promote_network_rate_rollups(pool)
        .await
        .context("promoting network-rate history tiers")?;
    let ping_promotion = promote_ping_rollups(pool)
        .await
        .context("promoting Ping history tiers")?;
    let system_metric_promotion = promote_system_metric_rollups(pool)
        .await
        .context("promoting system-metric history tiers")?;
    let traffic_retention = process_traffic_retention(pool)
        .await
        .context("promoting traffic history tiers")?;
    let network_observation_retention = process_network_observation_retention(
        pool,
        NetworkObservationRetentionPolicy {
            enabled: network_observations.enabled,
            retention_days: network_observations.retention_days,
            prune_limit: network_observations.prune_limit,
        },
    )
    .await
    .context("promoting automatic network-observation history tiers")?;
    Ok(TelemetryHistoryRetentionRun {
        resource_spans_merged: resource_promotion.promoted,
        resource_promotion_conflicts: resource_promotion.conflicts,
        network_rate_spans_merged: network_rate_promotion.promoted,
        network_rate_promotion_conflicts: network_rate_promotion.conflicts,
        ping_spans_merged: ping_promotion.promoted,
        ping_promotion_conflicts: ping_promotion.conflicts,
        system_metric_spans_merged: system_metric_promotion.promoted,
        system_metric_promotion_conflicts: system_metric_promotion.conflicts,
        traffic_raw_rows_promoted: traffic_retention.raw_rows_promoted,
        traffic_rollup_rows_promoted: traffic_retention.rollup_rows_promoted,
        traffic_rollup_rows_pruned: traffic_retention.rollup_rows_pruned,
        traffic_promotion_conflicts: traffic_retention.conflicts,
        network_observation_source_rows_promoted: network_observation_retention
            .source_rows_promoted,
        network_observation_destination_rows_written: network_observation_retention
            .destination_rows_written,
        network_observation_destination_conflicts: network_observation_retention
            .destination_conflicts,
        network_observation_expired_exact_rows_pruned: network_observation_retention
            .expired_exact_rows_pruned,
        network_observation_expired_rollup_rows_pruned: network_observation_retention
            .expired_rollup_rows_pruned,
        network_observation_inactive_latest_pruned: network_observation_retention
            .inactive_latest_pruned,
        network_observation_inactive_series_pruned: network_observation_retention
            .inactive_series_pruned,
        samples_pruned: prune_domain(pool, "telemetry_samples", samples).await?,
        ping_facts_pruned: prune_ping_facts(pool, samples).await?,
        rollups_pruned: prune_domain(pool, "telemetry_rollups", rollups).await?,
        network_rates_pruned: prune_domain(pool, "telemetry_network_rates", network_rates).await?,
        ping_rollups_pruned: prune_domain(pool, "telemetry_ping_rollups", ping_rollups).await?,
        system_metric_rollups_pruned: prune_domain(
            pool,
            "system_metric_rollups",
            system_metric_rollups,
        )
        .await?,
        // Traffic retirement is coupled to successful delta materialization;
        // never run the legacy independent raw-counter deleter.
        traffic_counter_samples_pruned: traffic_retention.raw_rows_promoted,
    })
}

async fn promote_resource_rollups(pool: &PgPool) -> Result<PromotionResult> {
    let mut result = PromotionResult::default();
    for destination_index in (1..TELEMETRY_HISTORY_TIERS.len()).rev() {
        let remaining = TELEMETRY_PROMOTION_SOURCE_ROW_LIMIT - result.source_rows as i64;
        if remaining <= 0 {
            break;
        }
        let source_tier = TELEMETRY_HISTORY_TIERS[destination_index - 1];
        let source_bucket_secs = TELEMETRY_HISTORY_TIERS[..destination_index]
            .iter()
            .map(|tier| tier.bucket_secs)
            .collect::<Vec<_>>();
        let pass = promote_resource_tier(
            pool,
            TELEMETRY_HISTORY_TIERS[destination_index].bucket_secs,
            source_tier.bucket_secs,
            source_tier.retain_days,
            &source_bucket_secs,
            remaining,
        )
        .await?;
        result.promoted += pass.promoted;
        result.conflicts += pass.conflicts;
        result.source_rows += pass.source_rows;
    }
    Ok(result)
}

async fn promote_resource_tier(
    pool: &PgPool,
    destination_secs: i32,
    source_bucket_secs: i32,
    source_days: i32,
    lower_bucket_secs: &[i32],
    source_row_limit: i64,
) -> Result<PromotionResult> {
    let Some(limits) = promotion_limits(destination_secs, source_bucket_secs, source_row_limit)
    else {
        return Ok(PromotionResult::default());
    };
    let result = sqlx::query(
        r#"
        WITH seed_rows AS MATERIALIZED (
            SELECT client_id, bucket_start
            FROM telemetry_rollups
            WHERE bucket_secs = $2
              AND bucket_start < to_timestamp(floor(extract(epoch FROM (
                    now() - make_interval(days => $3))) / $1) * $1)
            ORDER BY bucket_start, client_id DESC
            LIMIT $5
        ), candidate_keys AS MATERIALIZED (
            SELECT DISTINCT client_id,
                to_timestamp(floor(extract(epoch FROM bucket_start) / $1) * $1)
                    AS destination_start
            FROM seed_rows
            ORDER BY destination_start, client_id
            LIMIT $4
        ), expanded_rows AS MATERIALIZED (
            SELECT source.*
            FROM candidate_keys candidate
            CROSS JOIN LATERAL (
                SELECT tier_source.*, candidate.destination_start
                FROM unnest($7::integer[]) tier(bucket_secs)
                CROSS JOIN LATERAL (
                    SELECT row.ctid AS source_ctid, row.*
                    FROM telemetry_rollups row
                    WHERE row.client_id = candidate.client_id
                      AND row.bucket_secs = tier.bucket_secs
                      AND row.bucket_start >= candidate.destination_start
                      AND row.bucket_start < candidate.destination_start
                            + make_interval(secs => $1)
                ) tier_source
                LIMIT $6
            ) source
        ), ordered_rows AS MATERIALIZED (
            SELECT row.*,
                lag(row.bucket_start + make_interval(secs => row.bucket_secs)) OVER (
                    PARTITION BY row.client_id, row.destination_start
                    ORDER BY row.bucket_start, row.bucket_secs
                ) AS previous_end
            FROM expanded_rows row
        ), group_state AS MATERIALIZED (
            SELECT row.client_id, row.destination_start,
                count(*)::bigint AS source_rows,
                bool_and(
                    (row.previous_end IS NULL OR row.previous_end <= row.bucket_start)
                    AND row.bucket_start + make_interval(secs => row.bucket_secs)
                        <= row.destination_start + make_interval(secs => $1)
                ) AS non_overlapping
            FROM ordered_rows row
            GROUP BY row.client_id, row.destination_start
        ), overlap_conflicts AS MATERIALIZED (
            SELECT client_id, destination_start
            FROM group_state
            WHERE source_rows >= $6 OR NOT non_overlapping
        ), eligible_groups AS MATERIALIZED (
            SELECT client_id, destination_start, source_rows
            FROM group_state
            WHERE source_rows < $6 AND non_overlapping
        ), lockable_groups AS MATERIALIZED (
            SELECT eligible.*
            FROM eligible_groups eligible
            WHERE NOT EXISTS (
                SELECT 1 FROM telemetry_rollups destination
                WHERE destination.client_id = eligible.client_id
                  AND destination.bucket_secs = $1
                  AND destination.bucket_start = eligible.destination_start
            )
        ), locked_rows AS MATERIALIZED (
            SELECT expanded.*
            FROM telemetry_rollups row
            JOIN expanded_rows expanded ON expanded.source_ctid = row.ctid
            JOIN lockable_groups group_row
              ON group_row.client_id = expanded.client_id
             AND group_row.destination_start = expanded.destination_start
            ORDER BY expanded.destination_start, expanded.client_id,
                expanded.bucket_start, expanded.bucket_secs
            FOR UPDATE OF row SKIP LOCKED
        ), complete_groups AS (
            SELECT group_row.client_id, group_row.destination_start
            FROM lockable_groups group_row
            JOIN locked_rows row ON row.client_id = group_row.client_id
              AND row.destination_start = group_row.destination_start
            GROUP BY group_row.client_id, group_row.destination_start, group_row.source_rows
            HAVING count(*) = group_row.source_rows
        ), source AS (
            SELECT row.*
            FROM locked_rows row JOIN complete_groups group_row USING (client_id)
            WHERE row.destination_start = group_row.destination_start
        ), inserted AS (
            INSERT INTO telemetry_rollups (
                client_id, bucket_start, bucket_secs, sample_count,
                cpu_usage_sample_count, cpu_usage_sum, cpu_usage_avg, cpu_usage_max,
                cpu_cores_max, cpu_load_1_avg, cpu_load_1_sum, cpu_load_1_max,
                cpu_load_5_avg, cpu_load_5_sum, cpu_load_5_max,
                cpu_load_15_avg, cpu_load_15_sum, cpu_load_15_max,
                memory_total_bytes_max, memory_available_bytes_avg,
                memory_available_bytes_sum, memory_available_bytes_min,
                memory_used_ratio_avg, memory_used_ratio_sum, memory_used_ratio_max,
                swap_sample_count, swap_total_bytes_max, swap_available_bytes_avg,
                swap_available_bytes_sum, swap_available_bytes_min,
                swap_used_ratio_avg, swap_used_ratio_sum, swap_used_ratio_max,
                disk_sample_count,
                disk_total_bytes_max, disk_available_bytes_avg,
                disk_available_bytes_sum, disk_available_bytes_min,
                disk_used_ratio_avg, disk_used_ratio_sum, disk_used_ratio_max,
                network_rx_bytes_max, network_tx_bytes_max, connections_sample_count,
                tcp_sockets_latest, udp_sockets_latest, connections_observed_at,
                latest_observed_at, updated_at
            )
            SELECT client_id, destination_start, $1,
                LEAST(sum(sample_count)::bigint, 2147483647)::integer,
                LEAST(sum(cpu_usage_sample_count)::bigint, 2147483647)::integer,
                sum(cpu_usage_sum),
                sum(cpu_usage_sum) / NULLIF(sum(cpu_usage_sample_count), 0),
                max(cpu_usage_max), max(cpu_cores_max),
                sum(cpu_load_1_sum) / sum(sample_count), sum(cpu_load_1_sum),
                max(cpu_load_1_max),
                sum(cpu_load_5_sum) / sum(sample_count), sum(cpu_load_5_sum),
                max(cpu_load_5_max),
                sum(cpu_load_15_sum) / sum(sample_count), sum(cpu_load_15_sum),
                max(cpu_load_15_max), max(memory_total_bytes_max),
                round(sum(memory_available_bytes_sum) / sum(sample_count))::bigint,
                sum(memory_available_bytes_sum), min(memory_available_bytes_min),
                sum(memory_used_ratio_sum) / sum(sample_count), sum(memory_used_ratio_sum),
                max(memory_used_ratio_max),
                LEAST(sum(swap_sample_count)::bigint, 2147483647)::integer,
                max(swap_total_bytes_max),
                CASE WHEN sum(swap_sample_count) > 0 THEN
                    round(sum(swap_available_bytes_sum) / sum(swap_sample_count))::bigint
                    WHEN max(swap_total_bytes_max) = 0 THEN 0 ELSE NULL END,
                sum(swap_available_bytes_sum),
                CASE WHEN sum(swap_sample_count) > 0 THEN
                    min(swap_available_bytes_min) FILTER (WHERE swap_sample_count > 0)
                    WHEN max(swap_total_bytes_max) = 0 THEN 0 ELSE NULL END,
                CASE WHEN sum(swap_sample_count) > 0 THEN
                    sum(swap_used_ratio_sum) / sum(swap_sample_count) ELSE NULL END,
                sum(swap_used_ratio_sum), max(swap_used_ratio_max),
                LEAST(sum(disk_sample_count)::bigint, 2147483647)::integer,
                COALESCE(max(disk_total_bytes_max)
                    FILTER (WHERE disk_sample_count > 0), 0),
                COALESCE(round((sum(disk_available_bytes_sum)
                    FILTER (WHERE disk_sample_count > 0))
                    / NULLIF(sum(disk_sample_count), 0)), 0)::bigint,
                COALESCE(sum(disk_available_bytes_sum)
                    FILTER (WHERE disk_sample_count > 0), 0),
                COALESCE(min(disk_available_bytes_min)
                    FILTER (WHERE disk_sample_count > 0), 0),
                COALESCE((sum(disk_used_ratio_sum)
                    FILTER (WHERE disk_sample_count > 0))
                    / NULLIF(sum(disk_sample_count), 0), 0),
                COALESCE(sum(disk_used_ratio_sum)
                    FILTER (WHERE disk_sample_count > 0), 0),
                COALESCE(max(disk_used_ratio_max)
                    FILTER (WHERE disk_sample_count > 0), 0),
                max(network_rx_bytes_max),
                max(network_tx_bytes_max),
                LEAST(sum(connections_sample_count)::bigint, 2147483647)::integer,
                (array_agg(tcp_sockets_latest ORDER BY connections_observed_at DESC)
                    FILTER (WHERE connections_observed_at IS NOT NULL))[1],
                (array_agg(udp_sockets_latest ORDER BY connections_observed_at DESC)
                    FILTER (WHERE connections_observed_at IS NOT NULL))[1],
                max(connections_observed_at), max(latest_observed_at), max(updated_at)
            FROM source GROUP BY client_id, destination_start
            ON CONFLICT (client_id, bucket_secs, bucket_start) DO NOTHING
            RETURNING client_id, bucket_start
        ), deleted AS (
            DELETE FROM telemetry_rollups row USING source, inserted
            WHERE row.ctid = source.source_ctid
              AND inserted.client_id = source.client_id
              AND inserted.bucket_start = source.destination_start
            RETURNING row.ctid
        ), destination_conflicts AS MATERIALIZED (
            SELECT eligible.client_id, eligible.destination_start
            FROM eligible_groups eligible
            WHERE EXISTS (
                SELECT 1 FROM telemetry_rollups destination
                WHERE destination.client_id = eligible.client_id
                  AND destination.bucket_secs = $1
                  AND destination.bucket_start = eligible.destination_start
            )
            UNION
            SELECT complete.client_id, complete.destination_start
            FROM complete_groups complete
            WHERE NOT EXISTS (
                SELECT 1 FROM inserted
                WHERE inserted.client_id = complete.client_id
                  AND inserted.bucket_start = complete.destination_start
            )
        ), conflicts AS MATERIALIZED (
            SELECT client_id, destination_start FROM overlap_conflicts
            UNION
            SELECT client_id, destination_start FROM destination_conflicts
        )
        SELECT
            (SELECT count(*)::bigint FROM inserted) AS promoted,
            (SELECT count(*)::bigint FROM conflicts) AS conflicts,
            (SELECT count(*)::bigint FROM deleted) AS source_rows
        "#,
    )
    .bind(destination_secs)
    .bind(source_bucket_secs)
    .bind(source_days)
    .bind(limits.group_limit)
    .bind(limits.seed_row_limit)
    .bind(limits.group_source_limit)
    .bind(lower_bucket_secs)
    .fetch_one(pool)
    .await?;
    let promoted = result.try_get::<i64, _>("promoted")?.max(0) as u64;
    warn_promotion_conflicts(
        "telemetry_rollups",
        source_bucket_secs,
        destination_secs,
        result.try_get("conflicts")?,
    );
    Ok(PromotionResult {
        promoted,
        conflicts: result.try_get::<i64, _>("conflicts")?.max(0) as u64,
        source_rows: result.try_get::<i64, _>("source_rows")?.max(0) as u64,
    })
}

async fn promote_network_rate_rollups(pool: &PgPool) -> Result<PromotionResult> {
    let mut result = PromotionResult::default();
    for destination_index in (1..TELEMETRY_HISTORY_TIERS.len()).rev() {
        let remaining = TELEMETRY_PROMOTION_SOURCE_ROW_LIMIT - result.source_rows as i64;
        if remaining <= 0 {
            break;
        }
        let source_tier = TELEMETRY_HISTORY_TIERS[destination_index - 1];
        let source_bucket_secs = TELEMETRY_HISTORY_TIERS[..destination_index]
            .iter()
            .map(|tier| tier.bucket_secs)
            .collect::<Vec<_>>();
        let pass = promote_network_rate_tier(
            pool,
            TELEMETRY_HISTORY_TIERS[destination_index].bucket_secs,
            source_tier.bucket_secs,
            source_tier.retain_days,
            &source_bucket_secs,
            remaining,
        )
        .await?;
        result.promoted += pass.promoted;
        result.conflicts += pass.conflicts;
        result.source_rows += pass.source_rows;
    }
    Ok(result)
}

async fn promote_network_rate_tier(
    pool: &PgPool,
    destination_secs: i32,
    source_bucket_secs: i32,
    source_days: i32,
    lower_bucket_secs: &[i32],
    source_row_limit: i64,
) -> Result<PromotionResult> {
    let Some(limits) = promotion_limits(destination_secs, source_bucket_secs, source_row_limit)
    else {
        return Ok(PromotionResult::default());
    };
    let result = sqlx::query(
        r#"
        WITH seed_rows AS MATERIALIZED (
            SELECT client_id, interface, bucket_start
            FROM telemetry_network_rates
            WHERE bucket_secs = $2
              AND bucket_start < to_timestamp(floor(extract(epoch FROM (
                    now() - make_interval(days => $3))) / $1) * $1)
            ORDER BY bucket_start, client_id DESC, interface DESC
            LIMIT $5
        ), candidate_keys AS MATERIALIZED (
            SELECT DISTINCT client_id, interface,
                to_timestamp(floor(extract(epoch FROM bucket_start) / $1) * $1) destination_start
            FROM seed_rows
            ORDER BY destination_start, client_id, interface LIMIT $4
        ), expanded_rows AS MATERIALIZED (
            SELECT source.*
            FROM candidate_keys candidate
            CROSS JOIN LATERAL (
                SELECT tier_source.*, candidate.destination_start
                FROM unnest($7::integer[]) tier(bucket_secs)
                CROSS JOIN LATERAL (
                    SELECT row.ctid AS source_ctid, row.*
                    FROM telemetry_network_rates row
                    WHERE row.client_id = candidate.client_id
                      AND row.interface = candidate.interface
                      AND row.bucket_secs = tier.bucket_secs
                      AND row.bucket_start >= candidate.destination_start
                      AND row.bucket_start < candidate.destination_start
                            + make_interval(secs => $1)
                ) tier_source
                LIMIT $6
            ) source
        ), ordered_source AS MATERIALIZED (
            SELECT row.*,
                lag(row.bucket_start + make_interval(secs => row.bucket_secs)) OVER (
                    PARTITION BY row.client_id, row.interface, row.destination_start
                    ORDER BY row.bucket_start, row.bucket_secs
                ) AS previous_end
            FROM expanded_rows row
        ), group_state AS MATERIALIZED (
            SELECT row.client_id, row.interface, row.destination_start,
                count(*)::bigint AS source_rows,
                bool_and(
                    (row.previous_end IS NULL OR row.previous_end <= row.bucket_start)
                    AND row.bucket_start + make_interval(secs => row.bucket_secs)
                        <= row.destination_start + make_interval(secs => $1)
                ) AS non_overlapping
            FROM ordered_source row
            GROUP BY row.client_id, row.interface, row.destination_start
        ), overlap_conflicts AS MATERIALIZED (
            SELECT client_id, interface, destination_start
            FROM group_state
            WHERE source_rows >= $6 OR NOT non_overlapping
        ), eligible_groups AS MATERIALIZED (
            SELECT client_id, interface, destination_start, source_rows
            FROM group_state
            WHERE source_rows < $6 AND non_overlapping
        ), lockable_groups AS MATERIALIZED (
            SELECT eligible.*
            FROM eligible_groups eligible
            WHERE NOT EXISTS (
                SELECT 1 FROM telemetry_network_rates destination
                WHERE destination.client_id = eligible.client_id
                  AND destination.interface = eligible.interface
                  AND destination.bucket_secs = $1
                  AND destination.bucket_start = eligible.destination_start
            )
        ), locked_source AS MATERIALIZED (
            SELECT expanded.*
            FROM telemetry_network_rates row
            JOIN expanded_rows expanded ON expanded.source_ctid = row.ctid
            JOIN lockable_groups groups
              ON groups.client_id = expanded.client_id
             AND groups.interface = expanded.interface
             AND groups.destination_start = expanded.destination_start
            ORDER BY expanded.destination_start, expanded.client_id, expanded.interface,
                expanded.bucket_start, expanded.bucket_secs
            FOR UPDATE OF row SKIP LOCKED
        ), complete_groups AS (
            SELECT groups.client_id, groups.interface, groups.destination_start
            FROM lockable_groups groups
            JOIN locked_source row
              ON row.client_id = groups.client_id
             AND row.interface = groups.interface
             AND row.destination_start = groups.destination_start
            GROUP BY groups.client_id, groups.interface, groups.destination_start,
                groups.source_rows
            HAVING count(*) = groups.source_rows
        ), source AS MATERIALIZED (
            SELECT row.* FROM locked_source row
            JOIN complete_groups groups USING (client_id, interface, destination_start)
        ), inserted AS (
            INSERT INTO telemetry_network_rates (
                client_id, interface, bucket_start, bucket_secs, sample_count,
                rx_bytes_sum, tx_bytes_sum, rx_bytes_avg, tx_bytes_avg,
                rx_bytes_last, tx_bytes_last, rx_counter_epoch, tx_counter_epoch,
                latest_observed_at, updated_at
            ) SELECT client_id, interface, destination_start, $1,
                LEAST(sum(sample_count)::bigint, 2147483647)::integer,
                sum(rx_bytes_sum), sum(tx_bytes_sum),
                round(sum(rx_bytes_sum) / sum(sample_count))::bigint,
                round(sum(tx_bytes_sum) / sum(sample_count))::bigint,
                (array_agg(rx_bytes_last ORDER BY latest_observed_at DESC))[1],
                (array_agg(tx_bytes_last ORDER BY latest_observed_at DESC))[1],
                (array_agg(rx_counter_epoch ORDER BY latest_observed_at DESC))[1],
                (array_agg(tx_counter_epoch ORDER BY latest_observed_at DESC))[1],
                max(latest_observed_at), max(updated_at)
            FROM source GROUP BY client_id, interface, destination_start
            ON CONFLICT (client_id, interface, bucket_secs, bucket_start) DO NOTHING
            RETURNING client_id, interface, bucket_start
        ), deleted AS (
            DELETE FROM telemetry_network_rates row USING source, inserted
            WHERE row.ctid = source.source_ctid AND inserted.client_id = source.client_id
              AND inserted.interface = source.interface
              AND inserted.bucket_start = source.destination_start RETURNING row.ctid
        ), destination_conflicts AS MATERIALIZED (
            SELECT eligible.client_id, eligible.interface, eligible.destination_start
            FROM eligible_groups eligible
            WHERE EXISTS (
                SELECT 1 FROM telemetry_network_rates destination
                WHERE destination.client_id = eligible.client_id
                  AND destination.interface = eligible.interface
                  AND destination.bucket_secs = $1
                  AND destination.bucket_start = eligible.destination_start
            )
            UNION
            SELECT complete.client_id, complete.interface, complete.destination_start
            FROM complete_groups complete
            WHERE NOT EXISTS (
                SELECT 1 FROM inserted
                WHERE inserted.client_id = complete.client_id
                  AND inserted.interface = complete.interface
                  AND inserted.bucket_start = complete.destination_start
            )
        ), conflicts AS MATERIALIZED (
            SELECT client_id, interface, destination_start FROM overlap_conflicts
            UNION
            SELECT client_id, interface, destination_start FROM destination_conflicts
        ) SELECT
            (SELECT count(*)::bigint FROM inserted) AS promoted,
            (SELECT count(*)::bigint FROM conflicts) AS conflicts,
            (SELECT count(*)::bigint FROM deleted) AS source_rows
    "#,
    )
    .bind(destination_secs)
    .bind(source_bucket_secs)
    .bind(source_days)
    .bind(limits.group_limit)
    .bind(limits.seed_row_limit)
    .bind(limits.group_source_limit)
    .bind(lower_bucket_secs)
    .fetch_one(pool)
    .await?;
    let promoted = result.try_get::<i64, _>("promoted")?.max(0) as u64;
    warn_promotion_conflicts(
        "telemetry_network_rates",
        source_bucket_secs,
        destination_secs,
        result.try_get("conflicts")?,
    );
    Ok(PromotionResult {
        promoted,
        conflicts: result.try_get::<i64, _>("conflicts")?.max(0) as u64,
        source_rows: result.try_get::<i64, _>("source_rows")?.max(0) as u64,
    })
}

async fn promote_ping_rollups(pool: &PgPool) -> Result<PromotionResult> {
    let mut result = PromotionResult::default();
    let mut series_cursor = PING_PROMOTION_SERIES_CURSOR.load(Ordering::Relaxed).max(0);
    for destination_index in (1..TELEMETRY_HISTORY_TIERS.len()).rev() {
        let remaining = TELEMETRY_PROMOTION_SOURCE_ROW_LIMIT - result.source_rows as i64;
        if remaining <= 0 {
            break;
        }
        let source_tier = TELEMETRY_HISTORY_TIERS[destination_index - 1];
        let source_bucket_secs = TELEMETRY_HISTORY_TIERS[..destination_index]
            .iter()
            .map(|tier| tier.bucket_secs)
            .collect::<Vec<_>>();
        let pass = promote_ping_tier(
            pool,
            TELEMETRY_HISTORY_TIERS[destination_index].bucket_secs,
            source_tier.bucket_secs,
            source_tier.retain_days,
            &source_bucket_secs,
            remaining,
            series_cursor,
        )
        .await?;
        if let Some(next_cursor) = pass.next_series_cursor {
            series_cursor = next_cursor;
            // This cursor only distributes bounded parent probes over successive
            // runs. Every selected group remains independently atomic, so
            // restart/cursor loss affects scheduling, never data correctness.
            PING_PROMOTION_SERIES_CURSOR.store(series_cursor, Ordering::Relaxed);
        }
        result.promoted += pass.promotion.promoted;
        result.conflicts += pass.promotion.conflicts;
        result.source_rows += pass.promotion.source_rows;
    }
    Ok(result)
}

async fn promote_ping_tier(
    pool: &PgPool,
    destination_secs: i32,
    source_bucket_secs: i32,
    source_days: i32,
    lower_bucket_secs: &[i32],
    source_row_limit: i64,
    series_cursor: i64,
) -> Result<PingPromotionResult> {
    let Some(limits) = promotion_limits(destination_secs, source_bucket_secs, source_row_limit)
    else {
        return Ok(PingPromotionResult::default());
    };
    let result = sqlx::query(
        r#"
        WITH candidate_series AS MATERIALIZED (
            SELECT id, pass
            FROM (
                (SELECT id, 0::integer AS pass
                 FROM telemetry_ping_series
                 WHERE id > $8
                 ORDER BY id
                 LIMIT $4)
                UNION ALL
                (SELECT id, 1::integer AS pass
                 FROM telemetry_ping_series
                 WHERE id <= $8
                 ORDER BY id
                 LIMIT $4)
            ) candidate
            ORDER BY pass, id
            LIMIT $4
        ), locked_candidate_series AS MATERIALIZED (
            SELECT series.id, candidate.pass
            FROM candidate_series candidate
            JOIN telemetry_ping_series series ON series.id = candidate.id
            ORDER BY candidate.pass, series.id
            FOR NO KEY UPDATE OF series SKIP LOCKED
        ), seed_rows AS MATERIALIZED (
            SELECT series.id AS series_id, seed.bucket_start
            FROM locked_candidate_series series
            CROSS JOIN LATERAL (
                SELECT row.bucket_start
                FROM telemetry_ping_rollups row
                WHERE row.series_id = series.id
                  AND row.bucket_secs = $2
                  AND row.bucket_start < to_timestamp(floor(extract(epoch FROM (
                        now() - make_interval(days => $3))) / $1) * $1)
                ORDER BY row.bucket_start
                LIMIT $5
            ) seed
        ), candidate_keys AS MATERIALIZED (
            SELECT DISTINCT series_id,
                to_timestamp(floor(extract(epoch FROM bucket_start) / $1) * $1) destination_start
            FROM seed_rows
            ORDER BY destination_start, series_id LIMIT $4
        ), expanded_rows AS MATERIALIZED (
            SELECT source.*
            FROM candidate_keys candidate
            CROSS JOIN LATERAL (
                SELECT tier_source.*, candidate.destination_start
                FROM unnest($7::integer[]) tier(bucket_secs)
                CROSS JOIN LATERAL (
                    SELECT row.ctid AS source_ctid, row.*
                    FROM telemetry_ping_rollups row
                    WHERE row.series_id = candidate.series_id
                      AND row.bucket_secs = tier.bucket_secs
                      AND row.bucket_start >= candidate.destination_start
                      AND row.bucket_start < candidate.destination_start
                            + make_interval(secs => $1)
                ) tier_source
                LIMIT $6
            ) source
        ), ordered_source AS MATERIALIZED (
            SELECT row.*,
                lag(row.bucket_start + make_interval(secs => row.bucket_secs)) OVER (
                    PARTITION BY row.series_id, row.destination_start
                    ORDER BY row.bucket_start, row.bucket_secs
                ) AS previous_end
            FROM expanded_rows row
        ), group_state AS MATERIALIZED (
            SELECT row.series_id, row.destination_start,
                count(*)::bigint AS source_rows,
                bool_and(
                    (row.previous_end IS NULL OR row.previous_end <= row.bucket_start)
                    AND row.bucket_start + make_interval(secs => row.bucket_secs)
                        <= row.destination_start + make_interval(secs => $1)
                ) AS non_overlapping
            FROM ordered_source row
            GROUP BY row.series_id, row.destination_start
        ), overlap_conflicts AS MATERIALIZED (
            SELECT series_id, destination_start
            FROM group_state
            WHERE source_rows >= $6 OR NOT non_overlapping
        ), eligible_groups AS MATERIALIZED (
            SELECT series_id, destination_start, source_rows
            FROM group_state
            WHERE source_rows < $6 AND non_overlapping
        ), lockable_groups AS MATERIALIZED (
            SELECT eligible.*
            FROM eligible_groups eligible
            WHERE NOT EXISTS (
                SELECT 1 FROM telemetry_ping_rollups destination
                WHERE destination.series_id = eligible.series_id
                  AND destination.bucket_secs = $1
                  AND destination.bucket_start = eligible.destination_start
            )
        ), locked_source AS MATERIALIZED (
            SELECT expanded.*
            FROM telemetry_ping_rollups row
            JOIN expanded_rows expanded ON expanded.source_ctid = row.ctid
            JOIN lockable_groups groups
              ON groups.series_id = expanded.series_id
             AND groups.destination_start = expanded.destination_start
            ORDER BY expanded.destination_start, expanded.series_id,
                expanded.bucket_start, expanded.bucket_secs
            FOR UPDATE OF row SKIP LOCKED
        ), complete_groups AS (
            SELECT groups.series_id, groups.destination_start
            FROM lockable_groups groups
            JOIN locked_source row
              ON row.series_id = groups.series_id
             AND row.destination_start = groups.destination_start
            GROUP BY groups.series_id, groups.destination_start, groups.source_rows
            HAVING count(*) = groups.source_rows
        ), source AS MATERIALIZED (
            SELECT row.* FROM locked_source row
            JOIN complete_groups groups USING (series_id, destination_start)
        ), inserted AS (
            INSERT INTO telemetry_ping_rollups (
                series_id, bucket_start, bucket_secs, sample_count, success_count,
                latency_sum_ms, latency_avg_ms, latency_min_ms, latency_max_ms,
                loss_ratio_avg, loss_ratio_sum, loss_ratio_max,
                latest_status, latest_reason, latest_checked_at, updated_at
            ) SELECT series_id, destination_start, $1,
                LEAST(sum(sample_count)::bigint, 2147483647)::integer,
                LEAST(sum(success_count)::bigint, 2147483647)::integer,
                sum(latency_sum_ms), sum(latency_sum_ms) / NULLIF(sum(success_count), 0),
                min(latency_min_ms), max(latency_max_ms),
                sum(loss_ratio_sum) / sum(sample_count), sum(loss_ratio_sum), max(loss_ratio_max),
                (array_agg(latest_status ORDER BY latest_checked_at DESC))[1],
                (array_agg(latest_reason ORDER BY latest_checked_at DESC))[1],
                max(latest_checked_at), max(updated_at)
            FROM source GROUP BY series_id, destination_start
            ON CONFLICT (series_id, bucket_secs, bucket_start) DO NOTHING
            RETURNING series_id, bucket_start
        ), deleted AS (
            DELETE FROM telemetry_ping_rollups row USING source, inserted
            WHERE row.ctid = source.source_ctid AND inserted.series_id = source.series_id
              AND inserted.bucket_start = source.destination_start RETURNING row.ctid
        ), destination_conflicts AS MATERIALIZED (
            SELECT eligible.series_id, eligible.destination_start
            FROM eligible_groups eligible
            WHERE EXISTS (
                SELECT 1 FROM telemetry_ping_rollups destination
                WHERE destination.series_id = eligible.series_id
                  AND destination.bucket_secs = $1
                  AND destination.bucket_start = eligible.destination_start
            )
            UNION
            SELECT complete.series_id, complete.destination_start
            FROM complete_groups complete
            WHERE NOT EXISTS (
                SELECT 1 FROM inserted
                WHERE inserted.series_id = complete.series_id
                  AND inserted.bucket_start = complete.destination_start
            )
        ), conflicts AS MATERIALIZED (
            SELECT series_id, destination_start FROM overlap_conflicts
            UNION
            SELECT series_id, destination_start FROM destination_conflicts
        ) SELECT
            (SELECT count(*)::bigint FROM inserted) AS promoted,
            (SELECT count(*)::bigint FROM conflicts) AS conflicts,
            (SELECT count(*)::bigint FROM deleted) AS source_rows,
            (SELECT id FROM candidate_series ORDER BY pass DESC, id DESC LIMIT 1)
                AS next_series_cursor
    "#,
    )
    .bind(destination_secs)
    .bind(source_bucket_secs)
    .bind(source_days)
    .bind(limits.group_limit)
    .bind(limits.seed_rows_per_group)
    .bind(limits.group_source_limit)
    .bind(lower_bucket_secs)
    .bind(series_cursor)
    .fetch_one(pool)
    .await?;
    let promoted = result.try_get::<i64, _>("promoted")?.max(0) as u64;
    warn_promotion_conflicts(
        "telemetry_ping_rollups",
        source_bucket_secs,
        destination_secs,
        result.try_get("conflicts")?,
    );
    Ok(PingPromotionResult {
        promotion: PromotionResult {
            promoted,
            conflicts: result.try_get::<i64, _>("conflicts")?.max(0) as u64,
            source_rows: result.try_get::<i64, _>("source_rows")?.max(0) as u64,
        },
        next_series_cursor: result.try_get("next_series_cursor")?,
    })
}

async fn promote_system_metric_rollups(pool: &PgPool) -> Result<PromotionResult> {
    let mut result = PromotionResult::default();
    for destination_index in (1..TELEMETRY_HISTORY_TIERS.len()).rev() {
        let remaining = TELEMETRY_PROMOTION_SOURCE_ROW_LIMIT - result.source_rows as i64;
        if remaining <= 0 {
            break;
        }
        let source_tier = TELEMETRY_HISTORY_TIERS[destination_index - 1];
        let source_bucket_secs = TELEMETRY_HISTORY_TIERS[..destination_index]
            .iter()
            .map(|tier| tier.bucket_secs)
            .collect::<Vec<_>>();
        let pass = promote_system_metric_tier(
            pool,
            TELEMETRY_HISTORY_TIERS[destination_index].bucket_secs,
            source_tier.bucket_secs,
            source_tier.retain_days,
            &source_bucket_secs,
            remaining,
        )
        .await?;
        result.promoted += pass.promoted;
        result.conflicts += pass.conflicts;
        result.source_rows += pass.source_rows;
    }
    Ok(result)
}

async fn promote_system_metric_tier(
    pool: &PgPool,
    destination_secs: i32,
    source_bucket_secs: i32,
    source_days: i32,
    lower_bucket_secs: &[i32],
    source_row_limit: i64,
) -> Result<PromotionResult> {
    let Some(limits) = promotion_limits(destination_secs, source_bucket_secs, source_row_limit)
    else {
        return Ok(PromotionResult::default());
    };
    let result = sqlx::query(
        r#"
        WITH seed_rows AS MATERIALIZED (
            SELECT metric, bucket_start
            FROM system_metric_rollups
            WHERE bucket_secs = $2
              AND bucket_start < to_timestamp(floor(extract(epoch FROM (
                    now() - make_interval(days => $3))) / $1) * $1)
            ORDER BY bucket_start, metric DESC
            LIMIT $5
        ), candidate_keys AS MATERIALIZED (
            SELECT DISTINCT metric,
                to_timestamp(floor(extract(epoch FROM bucket_start) / $1) * $1) destination_start
            FROM seed_rows
            ORDER BY destination_start, metric LIMIT $4
        ), expanded_rows AS MATERIALIZED (
            SELECT source.*
            FROM candidate_keys candidate
            CROSS JOIN LATERAL (
                SELECT tier_source.*, candidate.destination_start
                FROM unnest($7::integer[]) tier(bucket_secs)
                CROSS JOIN LATERAL (
                    SELECT row.ctid AS source_ctid, row.*
                    FROM system_metric_rollups row
                    WHERE row.metric = candidate.metric
                      AND row.bucket_secs = tier.bucket_secs
                      AND row.bucket_start >= candidate.destination_start
                      AND row.bucket_start < candidate.destination_start
                            + make_interval(secs => $1)
                ) tier_source
                LIMIT $6
            ) source
        ), ordered_source AS MATERIALIZED (
            SELECT row.*,
                lag(row.bucket_start + make_interval(secs => row.bucket_secs)) OVER (
                    PARTITION BY row.metric, row.destination_start
                    ORDER BY row.bucket_start, row.bucket_secs
                ) AS previous_end
            FROM expanded_rows row
        ), group_state AS MATERIALIZED (
            SELECT row.metric, row.destination_start,
                count(*)::bigint AS source_rows,
                bool_and(
                    (row.previous_end IS NULL OR row.previous_end <= row.bucket_start)
                    AND row.bucket_start + make_interval(secs => row.bucket_secs)
                        <= row.destination_start + make_interval(secs => $1)
                ) AS non_overlapping
            FROM ordered_source row
            GROUP BY row.metric, row.destination_start
        ), overlap_conflicts AS MATERIALIZED (
            SELECT metric, destination_start
            FROM group_state
            WHERE source_rows >= $6 OR NOT non_overlapping
        ), eligible_groups AS MATERIALIZED (
            SELECT metric, destination_start, source_rows
            FROM group_state
            WHERE source_rows < $6 AND non_overlapping
        ), lockable_groups AS MATERIALIZED (
            SELECT eligible.*
            FROM eligible_groups eligible
            WHERE NOT EXISTS (
                SELECT 1 FROM system_metric_rollups destination
                WHERE destination.metric = eligible.metric
                  AND destination.bucket_secs = $1
                  AND destination.bucket_start = eligible.destination_start
            )
        ), locked_source AS MATERIALIZED (
            SELECT expanded.*
            FROM system_metric_rollups row
            JOIN expanded_rows expanded ON expanded.source_ctid = row.ctid
            JOIN lockable_groups groups
              ON groups.metric = expanded.metric
             AND groups.destination_start = expanded.destination_start
            ORDER BY expanded.destination_start, expanded.metric,
                expanded.bucket_start, expanded.bucket_secs
            FOR UPDATE OF row SKIP LOCKED
        ), complete_groups AS (
            SELECT groups.metric, groups.destination_start
            FROM lockable_groups groups
            JOIN locked_source row
              ON row.metric = groups.metric
             AND row.destination_start = groups.destination_start
            GROUP BY groups.metric, groups.destination_start, groups.source_rows
            HAVING count(*) = groups.source_rows
        ), source AS MATERIALIZED (
            SELECT row.* FROM locked_source row
            JOIN complete_groups groups USING (metric, destination_start)
        ), inserted AS (
            INSERT INTO system_metric_rollups (
                metric, bucket_start, bucket_secs, sample_count, value_sum,
                avg_value, max_value, latest_value, latest_observed_at, updated_at
            ) SELECT metric, destination_start, $1,
                LEAST(sum(sample_count)::bigint, 2147483647)::integer, sum(value_sum),
                sum(value_sum) / sum(sample_count), max(max_value),
                (array_agg(latest_value ORDER BY latest_observed_at DESC))[1],
                max(latest_observed_at), max(updated_at)
            FROM source GROUP BY metric, destination_start
            ON CONFLICT (metric, bucket_secs, bucket_start) DO NOTHING
            RETURNING metric, bucket_start
        ), deleted AS (
            DELETE FROM system_metric_rollups row USING source, inserted
            WHERE row.ctid = source.source_ctid AND inserted.metric = source.metric
              AND inserted.bucket_start = source.destination_start RETURNING row.ctid
        ), destination_conflicts AS MATERIALIZED (
            SELECT eligible.metric, eligible.destination_start
            FROM eligible_groups eligible
            WHERE EXISTS (
                SELECT 1 FROM system_metric_rollups destination
                WHERE destination.metric = eligible.metric
                  AND destination.bucket_secs = $1
                  AND destination.bucket_start = eligible.destination_start
            )
            UNION
            SELECT complete.metric, complete.destination_start
            FROM complete_groups complete
            WHERE NOT EXISTS (
                SELECT 1 FROM inserted
                WHERE inserted.metric = complete.metric
                  AND inserted.bucket_start = complete.destination_start
            )
        ), conflicts AS MATERIALIZED (
            SELECT metric, destination_start FROM overlap_conflicts
            UNION
            SELECT metric, destination_start FROM destination_conflicts
        ) SELECT
            (SELECT count(*)::bigint FROM inserted) AS promoted,
            (SELECT count(*)::bigint FROM conflicts) AS conflicts,
            (SELECT count(*)::bigint FROM deleted) AS source_rows
    "#,
    )
    .bind(destination_secs)
    .bind(source_bucket_secs)
    .bind(source_days)
    .bind(limits.group_limit)
    .bind(limits.seed_row_limit)
    .bind(limits.group_source_limit)
    .bind(lower_bucket_secs)
    .fetch_one(pool)
    .await?;
    let promoted = result.try_get::<i64, _>("promoted")?.max(0) as u64;
    warn_promotion_conflicts(
        "system_metric_rollups",
        source_bucket_secs,
        destination_secs,
        result.try_get("conflicts")?,
    );
    Ok(PromotionResult {
        promoted,
        conflicts: result.try_get::<i64, _>("conflicts")?.max(0) as u64,
        source_rows: result.try_get::<i64, _>("source_rows")?.max(0) as u64,
    })
}

fn warn_promotion_conflicts(
    domain: &str,
    source_bucket_secs: i32,
    destination_bucket_secs: i32,
    conflicts: i64,
) {
    if conflicts > 0 {
        tracing::warn!(
            domain,
            source_bucket_secs,
            destination_bucket_secs,
            conflicts,
            "history tier promotion retained sources because destination or overlap conflicts exist"
        );
    }
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
            prune_limit: if domain == "network_observations" {
                DEFAULT_NETWORK_OBSERVATION_RETENTION_PRUNE_LIMIT
            } else {
                DEFAULT_TELEMETRY_RETENTION_PRUNE_LIMIT
            },
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
        "system_metric_rollups" => prune_query("system_metric_rollups"),
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

async fn prune_ping_facts(pool: &PgPool, policy: RetentionPolicy) -> Result<u64> {
    if !policy.enabled {
        return Ok(0);
    }
    let result = sqlx::query(
        r#"
        WITH candidates AS (
            SELECT ctid FROM telemetry_ping_facts
            WHERE observed_at < (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
            ) - make_interval(days => $1)
            ORDER BY observed_at, series_id, checked_unix, source_checked_unix
            LIMIT $2 FOR UPDATE SKIP LOCKED
        )
        DELETE FROM telemetry_ping_facts WHERE ctid IN (SELECT ctid FROM candidates)
        "#,
    )
    .bind(policy.retention_days)
    .bind(policy.prune_limit)
    .execute(pool)
    .await?;
    let pruned = result.rows_affected();
    sqlx::query(
        r#"
        WITH candidates AS (
            SELECT current.series_id
            FROM telemetry_ping_current current
            JOIN telemetry_ping_series series ON series.id = current.series_id
            WHERE NOT EXISTS (
                SELECT 1 FROM telemetry_ping_facts fact WHERE fact.series_id = series.id
            )
              AND NOT EXISTS (
                SELECT 1 FROM telemetry_ping_rollups rollup WHERE rollup.series_id = series.id
            )
              AND NOT EXISTS (
                SELECT 1
                FROM ping_targets target
                JOIN ping_target_assignments assignment
                  ON assignment.target_id = target.id
                 AND assignment.client_id = series.client_id
                WHERE target.id = series.target_id
                  AND target.generation = series.generation
              )
            ORDER BY current.latest_checked_at, current.series_id
            LIMIT $1
            FOR UPDATE OF current SKIP LOCKED
        )
        DELETE FROM telemetry_ping_current current
        USING candidates
        WHERE current.series_id = candidates.series_id
        "#,
    )
    .bind(policy.prune_limit)
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
        WITH candidates AS (
            SELECT series.id
            FROM telemetry_ping_series series
            WHERE NOT EXISTS (
                SELECT 1 FROM telemetry_ping_facts fact WHERE fact.series_id = series.id
            )
              AND NOT EXISTS (
                SELECT 1 FROM telemetry_ping_rollups rollup WHERE rollup.series_id = series.id
            )
              AND NOT EXISTS (
                SELECT 1 FROM telemetry_ping_current current WHERE current.series_id = series.id
            )
            ORDER BY series.id
            LIMIT $1
            FOR UPDATE OF series SKIP LOCKED
        )
        DELETE FROM telemetry_ping_series series
        USING candidates
        WHERE series.id = candidates.id
        "#,
    )
    .bind(policy.prune_limit)
    .execute(pool)
    .await?;
    Ok(pruned)
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
#[path = "tests_pressure_proof.rs"]
mod pressure_proof;
#[cfg(test)]
#[path = "tests_history_retention.rs"]
mod tests;
