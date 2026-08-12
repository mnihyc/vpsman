use anyhow::{ensure, Result};
use sqlx::{PgPool, Row};

const PROMOTION_SOURCE_ROW_LIMIT: i64 = 20_000;
const PROMOTION_GROUP_LIMIT: i64 = 2_000;
const LIFECYCLE_PRUNE_LIMIT: i64 = 20_000;

/// Exact automatic reachability remains available for two days. Older rows
/// move through these UTC-aligned native resolutions without affecting manual
/// probes, speed tests, or network-status evidence.
pub(crate) const NETWORK_OBSERVATION_TIERS: &[(i32, i32, i32)] = &[
    (0, 300, 2),
    (300, 1_800, 8),
    (1_800, 3_600, 31),
    (3_600, 10_800, 91),
    (10_800, 21_600, 181),
    (21_600, 86_400, 366),
];

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NetworkObservationRetentionRun {
    pub(crate) source_rows_promoted: u64,
    pub(crate) destination_rows_written: u64,
    pub(crate) destination_conflicts: u64,
    pub(crate) expired_exact_rows_pruned: u64,
    pub(crate) expired_rollup_rows_pruned: u64,
    pub(crate) inactive_latest_pruned: u64,
    pub(crate) inactive_series_pruned: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct NetworkObservationRetentionPolicy {
    pub(crate) enabled: bool,
    pub(crate) retention_days: i32,
    pub(crate) prune_limit: i32,
}

pub(crate) async fn process_network_observation_retention(
    pool: &PgPool,
    policy: NetworkObservationRetentionPolicy,
) -> Result<NetworkObservationRetentionRun> {
    let mut run = NetworkObservationRetentionRun::default();
    let promotion_horizon_days = policy.enabled.then_some(policy.retention_days);
    if policy.enabled {
        run.expired_exact_rows_pruned =
            prune_expired_exact_observations(pool, policy.retention_days, policy.prune_limit)
                .await?;
        let remaining_prune_limit =
            u64::try_from(policy.prune_limit)?.saturating_sub(run.expired_exact_rows_pruned);
        if remaining_prune_limit > 0 {
            run.expired_rollup_rows_pruned = prune_expired_rollups(
                pool,
                policy.retention_days,
                i32::try_from(remaining_prune_limit)?,
            )
            .await?;
        }
    }
    // Pure raw backlog jumps directly to its age-required tier. If a bucket
    // already contains retained rows, leave raw rows for the fallback 5m pass;
    // the mixed-rollup pass below then combines all non-overlapping finer tiers.
    let mut upper_age_days = None;
    for &(_, destination_bucket_secs, retain_days) in NETWORK_OBSERVATION_TIERS.iter().rev() {
        let promoted = promote_exact_observations(
            pool,
            destination_bucket_secs,
            retain_days,
            upper_age_days,
            false,
            promotion_horizon_days,
        )
        .await?;
        add_promotion_result(&mut run, 0, destination_bucket_secs, promoted);
        upper_age_days = Some(retain_days);
    }
    let fallback =
        promote_exact_observations(pool, 300, 2, None, true, promotion_horizon_days).await?;
    add_promotion_result(&mut run, 0, 300, fallback);

    upper_age_days = None;
    for &(_, destination_bucket_secs, retain_days) in NETWORK_OBSERVATION_TIERS.iter().rev() {
        let promoted = promote_rollups(
            pool,
            destination_bucket_secs,
            retain_days,
            upper_age_days,
            promotion_horizon_days,
        )
        .await?;
        add_promotion_result(&mut run, -1, destination_bucket_secs, promoted);
        upper_age_days = Some(retain_days);
    }
    run.inactive_latest_pruned = prune_inactive_latest(pool).await?;
    run.inactive_series_pruned = prune_empty_inactive_series(pool).await?;
    Ok(run)
}

fn add_promotion_result(
    run: &mut NetworkObservationRetentionRun,
    source_bucket_secs: i32,
    destination_bucket_secs: i32,
    promoted: PromotionResult,
) {
    run.source_rows_promoted = run
        .source_rows_promoted
        .saturating_add(promoted.source_rows);
    run.destination_rows_written = run
        .destination_rows_written
        .saturating_add(promoted.destination_rows);
    run.destination_conflicts = run
        .destination_conflicts
        .saturating_add(promoted.conflict_groups);
    if promoted.conflict_groups > 0 {
        tracing::warn!(
            source_bucket_secs,
            destination_bucket_secs,
            conflict_groups = promoted.conflict_groups,
            "network observation promotion preserved sources after destination conflict"
        );
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct PromotionResult {
    source_rows: u64,
    destination_rows: u64,
    conflict_groups: u64,
}

async fn promote_exact_observations(
    pool: &PgPool,
    destination_bucket_secs: i32,
    retain_days: i32,
    upper_age_days: Option<i32>,
    exclude_any_retained: bool,
    promotion_horizon_days: Option<i32>,
) -> Result<PromotionResult> {
    let mut tx = pool.begin().await?;
    let rows = sqlx::query(EXACT_PROMOTION_QUERY)
        .bind(destination_bucket_secs)
        .bind(retain_days)
        .bind(PROMOTION_SOURCE_ROW_LIMIT)
        .bind(PROMOTION_GROUP_LIMIT)
        .bind(upper_age_days)
        .bind(exclude_any_retained)
        .bind(promotion_horizon_days)
        .fetch_one(&mut *tx)
        .await?;
    let result = promotion_result_from_row(&rows)?;
    tx.commit().await?;
    Ok(result)
}

async fn promote_rollups(
    pool: &PgPool,
    destination_bucket_secs: i32,
    retain_days: i32,
    upper_age_days: Option<i32>,
    promotion_horizon_days: Option<i32>,
) -> Result<PromotionResult> {
    ensure!(
        destination_bucket_secs >= 300,
        "network observation tier is invalid"
    );
    let mut tx = pool.begin().await?;
    let rows = sqlx::query(ROLLUP_PROMOTION_QUERY)
        .bind(destination_bucket_secs)
        .bind(retain_days)
        .bind(PROMOTION_SOURCE_ROW_LIMIT)
        .bind(PROMOTION_GROUP_LIMIT)
        .bind(upper_age_days)
        .bind(promotion_horizon_days)
        .fetch_one(&mut *tx)
        .await?;
    let result = promotion_result_from_row(&rows)?;
    tx.commit().await?;
    Ok(result)
}

fn promotion_result_from_row(row: &sqlx::postgres::PgRow) -> Result<PromotionResult> {
    Ok(PromotionResult {
        source_rows: u64::try_from(row.try_get::<i64, _>("source_rows")?)?,
        destination_rows: u64::try_from(row.try_get::<i64, _>("destination_rows")?)?,
        conflict_groups: u64::try_from(row.try_get::<i64, _>("conflict_groups")?)?,
    })
}

async fn prune_expired_exact_observations(
    pool: &PgPool,
    retention_days: i32,
    prune_limit: i32,
) -> Result<u64> {
    let result = sqlx::query(
        r#"
        WITH candidates AS (
            SELECT observation.ctid
            FROM network_observations observation
            WHERE observation.source = 'automatic'
              AND observation.kind = 'tunnel_reachability'
              AND observation.automatic_series_id IS NOT NULL
              AND observation.observed_at <= (
                    date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                  ) - make_interval(days => $1)
            ORDER BY observation.observed_at, observation.automatic_series_id, observation.id
            FOR UPDATE OF observation SKIP LOCKED
            LIMIT $2
        )
        DELETE FROM network_observations observation
        USING candidates
        WHERE observation.ctid = candidates.ctid
        "#,
    )
    .bind(retention_days)
    .bind(prune_limit)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

async fn prune_expired_rollups(
    pool: &PgPool,
    retention_days: i32,
    prune_limit: i32,
) -> Result<u64> {
    let result = sqlx::query(
        r#"
        WITH candidates AS (
            SELECT ctid
            FROM network_observation_rollups
            WHERE bucket_start + make_interval(secs => bucket_secs) <= (
                    date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                  ) - make_interval(days => $1)
            ORDER BY bucket_start, series_id, health_state, reason_key
            FOR UPDATE SKIP LOCKED
            LIMIT $2
        )
        DELETE FROM network_observation_rollups rollup
        USING candidates
        WHERE rollup.ctid = candidates.ctid
        "#,
    )
    .bind(retention_days)
    .bind(prune_limit)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

async fn prune_inactive_latest(pool: &PgPool) -> Result<u64> {
    let result = sqlx::query(
        r#"
        WITH candidates AS (
            SELECT latest.series_id
            FROM network_observation_latest latest
            JOIN network_observation_series series ON series.id = latest.series_id
            WHERE series.active = FALSE
              AND latest.observed_at < now() - interval '2 days'
            ORDER BY latest.observed_at, latest.series_id
            FOR UPDATE OF latest SKIP LOCKED
            LIMIT $1
        )
        DELETE FROM network_observation_latest latest
        USING candidates
        WHERE latest.series_id = candidates.series_id
        "#,
    )
    .bind(LIFECYCLE_PRUNE_LIMIT)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

async fn prune_empty_inactive_series(pool: &PgPool) -> Result<u64> {
    let result = sqlx::query(
        r#"
        WITH candidates AS (
            SELECT series.id
            FROM network_observation_series series
            WHERE series.active = FALSE
              AND series.last_seen_at < now() - interval '2 days'
              AND NOT EXISTS (
                  SELECT 1
                  FROM network_observations observation
                  WHERE observation.automatic_series_id = series.id
              )
              AND NOT EXISTS (
                  SELECT 1
                  FROM network_observation_rollups rollup
                  WHERE rollup.series_id = series.id
              )
              AND NOT EXISTS (
                  SELECT 1
                  FROM network_observation_latest latest
                  WHERE latest.series_id = series.id
              )
            ORDER BY series.last_seen_at, series.id
            FOR UPDATE OF series SKIP LOCKED
            LIMIT $1
        )
        DELETE FROM network_observation_series series
        USING candidates
        WHERE series.id = candidates.id
        "#,
    )
    .bind(LIFECYCLE_PRUNE_LIMIT)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

const EXACT_PROMOTION_QUERY: &str = r#"
WITH oldest_group_seeds AS MATERIALIZED (
    SELECT
        series.id AS series_id,
        oldest.destination_start
    FROM network_observation_series series
    JOIN LATERAL (
        SELECT to_timestamp(
            floor(extract(epoch FROM observation.observed_at) / $1) * $1
        ) AS destination_start
        FROM network_observations observation
        WHERE observation.automatic_series_id = series.id
          AND observation.source = 'automatic'
          AND observation.kind = 'tunnel_reachability'
          AND ($7::integer IS NULL OR observation.observed_at > (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
              ) - make_interval(days => $7))
          AND observation.observed_at < now() - interval '2 hours'
          AND observation.observed_at < (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
              ) - make_interval(days => $2)
          AND to_timestamp(
                floor(extract(epoch FROM observation.observed_at) / $1) * $1
              ) + make_interval(secs => $1)
              <= (
                    date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                 ) - make_interval(days => $2)
          AND ($5::integer IS NULL
               OR observation.observed_at >= (
                    date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                  ) - make_interval(days => $5))
          AND NOT EXISTS (
              SELECT 1
              FROM network_observation_rollups retained
              WHERE retained.series_id = observation.automatic_series_id
                AND ($6 OR retained.bucket_secs <> $1)
                AND retained.bucket_start
                    < to_timestamp(
                        floor(extract(epoch FROM observation.observed_at) / $1) * $1
                      ) + make_interval(secs => $1)
                AND retained.bucket_start + make_interval(secs => retained.bucket_secs)
                    > to_timestamp(
                        floor(extract(epoch FROM observation.observed_at) / $1) * $1
                      )
          )
        ORDER BY observation.observed_at, observation.id
        LIMIT 1
    ) oldest ON TRUE
    ORDER BY oldest.destination_start, series.id
    LIMIT $4
),
candidate_groups AS MATERIALIZED (
    SELECT
        seed.series_id,
        seed.destination_start,
        COUNT(*)::bigint AS source_count
    FROM oldest_group_seeds seed
    JOIN network_observations observation
      ON observation.automatic_series_id = seed.series_id
     AND observation.observed_at >= seed.destination_start
     AND observation.observed_at
           < seed.destination_start + make_interval(secs => $1)
    WHERE observation.source = 'automatic'
      AND observation.kind = 'tunnel_reachability'
      AND ($7::integer IS NULL OR observation.observed_at > (
            date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
          ) - make_interval(days => $7))
      AND observation.observed_at < now() - interval '2 hours'
      AND observation.observed_at < (
            date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
          ) - make_interval(days => $2)
      AND to_timestamp(
            floor(extract(epoch FROM observation.observed_at) / $1) * $1
          ) + make_interval(secs => $1)
          <= (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
             ) - make_interval(days => $2)
      AND ($5::integer IS NULL
           OR observation.observed_at >= (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
              ) - make_interval(days => $5))
      AND NOT EXISTS (
          SELECT 1
          FROM network_observation_rollups retained
          WHERE retained.series_id = seed.series_id
            AND ($6 OR retained.bucket_secs <> $1)
            AND retained.bucket_start
                < seed.destination_start + make_interval(secs => $1)
            AND retained.bucket_start + make_interval(secs => retained.bucket_secs)
                > seed.destination_start
      )
    GROUP BY seed.series_id, seed.destination_start
),
bounded_groups AS MATERIALIZED (
    SELECT series_id, destination_start, source_count
    FROM (
        SELECT candidate_groups.*,
               SUM(source_count) OVER (
                   ORDER BY destination_start, series_id
               ) AS cumulative_count,
               row_number() OVER (
                   ORDER BY destination_start, series_id
               ) AS group_number
        FROM candidate_groups
    ) ranked
    WHERE cumulative_count <= $3 AND group_number <= $4
),
locked AS MATERIALIZED (
    SELECT observation.ctid AS source_ctid,
           observation.*,
           bounded.series_id,
           bounded.destination_start,
           bounded.source_count
    FROM network_observations observation
    JOIN bounded_groups bounded
      ON bounded.series_id = observation.automatic_series_id
     AND observation.observed_at >= bounded.destination_start
     AND observation.observed_at
           < bounded.destination_start + make_interval(secs => $1)
    WHERE observation.source = 'automatic'
      AND observation.kind = 'tunnel_reachability'
      AND ($7::integer IS NULL OR observation.observed_at > (
            date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
          ) - make_interval(days => $7))
      AND observation.observed_at < now() - interval '2 hours'
      AND observation.observed_at < (
            date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
          ) - make_interval(days => $2)
      AND bounded.destination_start + make_interval(secs => $1)
          <= (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
             ) - make_interval(days => $2)
      AND ($5::integer IS NULL
           OR observation.observed_at >= (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
              ) - make_interval(days => $5))
      AND NOT EXISTS (
          SELECT 1
          FROM network_observation_rollups retained
          WHERE retained.series_id = observation.automatic_series_id
            AND ($6 OR retained.bucket_secs <> $1)
            AND retained.bucket_start
                < bounded.destination_start + make_interval(secs => $1)
            AND retained.bucket_start + make_interval(secs => retained.bucket_secs)
                > bounded.destination_start
      )
    FOR UPDATE OF observation SKIP LOCKED
),
complete_groups AS MATERIALIZED (
    SELECT automatic_series_id AS series_id, destination_start
    FROM locked
    GROUP BY automatic_series_id, destination_start, source_count
    HAVING COUNT(*) = source_count
),
normalized AS MATERIALIZED (
    SELECT
        locked.automatic_series_id AS series_id,
        locked.destination_start,
        CASE WHEN locked.healthy IS TRUE THEN 1
             WHEN locked.healthy IS FALSE THEN 0 ELSE -1 END::smallint AS health_state,
        COALESCE(locked.reason, '') AS reason_key,
        1::bigint AS sample_count,
        COALESCE(locked.transmitted, 0)::numeric AS transmitted_total,
        (locked.transmitted IS NOT NULL)::integer::bigint AS transmitted_sample_count,
        COALESCE(locked.received, 0)::numeric AS received_total,
        (locked.received IS NOT NULL)::integer::bigint AS received_sample_count,
        COALESCE(locked.latency_avg_ms, 0.0) AS latency_sum_ms,
        (locked.latency_avg_ms IS NOT NULL)::integer::bigint AS latency_sample_count,
        locked.latency_avg_ms AS latency_min_ms,
        locked.latency_avg_ms AS latency_max_ms,
        COALESCE(locked.latency_mdev_ms, 0.0) AS latency_mdev_sum_ms,
        (locked.latency_mdev_ms IS NOT NULL)::integer::bigint
            AS latency_mdev_sample_count,
        COALESCE(locked.packet_loss_ratio, 0.0) AS packet_loss_sum_ratio,
        (locked.packet_loss_ratio IS NOT NULL)::integer::bigint
            AS packet_loss_sample_count,
        locked.packet_loss_ratio AS packet_loss_min_ratio,
        locked.packet_loss_ratio AS packet_loss_max_ratio,
        locked.id AS latest_observation_id,
        COALESCE(locked.stale_after_secs, 180) AS latest_stale_after_secs,
        COALESCE(locked.healthy, FALSE) AS latest_healthy,
        COALESCE(locked.transmitted, 0) AS latest_transmitted,
        COALESCE(locked.received, 0) AS latest_received,
        locked.latency_min_ms AS latest_latency_min_ms,
        locked.latency_avg_ms AS latest_latency_avg_ms,
        locked.latency_max_ms AS latest_latency_max_ms,
        locked.latency_mdev_ms AS latest_latency_mdev_ms,
        COALESCE(locked.packet_loss_ratio, 1.0) AS latest_packet_loss_ratio,
        locked.reason AS latest_reason,
        locked.observed_at AS latest_observed_at,
        locked.received_at AS latest_received_at
    FROM locked
    JOIN complete_groups complete
      ON complete.series_id = locked.automatic_series_id
     AND complete.destination_start = locked.destination_start
),
aggregated AS MATERIALIZED (
    SELECT
        series_id,
        destination_start,
        health_state,
        reason_key,
        SUM(sample_count)::bigint AS sample_count,
        SUM(transmitted_total)::numeric AS transmitted_total,
        SUM(transmitted_sample_count)::bigint AS transmitted_sample_count,
        SUM(received_total)::numeric AS received_total,
        SUM(received_sample_count)::bigint AS received_sample_count,
        SUM(latency_sum_ms)::double precision AS latency_sum_ms,
        SUM(latency_sample_count)::bigint AS latency_sample_count,
        MIN(latency_min_ms)::double precision AS latency_min_ms,
        MAX(latency_max_ms)::double precision AS latency_max_ms,
        SUM(latency_mdev_sum_ms)::double precision AS latency_mdev_sum_ms,
        SUM(latency_mdev_sample_count)::bigint AS latency_mdev_sample_count,
        SUM(packet_loss_sum_ratio)::double precision AS packet_loss_sum_ratio,
        SUM(packet_loss_sample_count)::bigint AS packet_loss_sample_count,
        MIN(packet_loss_min_ratio)::double precision AS packet_loss_min_ratio,
        MAX(packet_loss_max_ratio)::double precision AS packet_loss_max_ratio,
        (array_agg(latest_observation_id
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_observation_id,
        (array_agg(latest_stale_after_secs
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_stale_after_secs,
        (array_agg(latest_healthy
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_healthy,
        (array_agg(latest_transmitted
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_transmitted,
        (array_agg(latest_received
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_received,
        (array_agg(latest_latency_min_ms
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_latency_min_ms,
        (array_agg(latest_latency_avg_ms
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_latency_avg_ms,
        (array_agg(latest_latency_max_ms
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_latency_max_ms,
        (array_agg(latest_latency_mdev_ms
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_latency_mdev_ms,
        (array_agg(latest_packet_loss_ratio
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_packet_loss_ratio,
        (array_agg(latest_reason
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_reason,
        MAX(latest_observed_at) AS latest_observed_at,
        (array_agg(latest_received_at
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_received_at
    FROM normalized
    GROUP BY series_id, destination_start, health_state, reason_key
),
conflicted_groups AS MATERIALIZED (
    SELECT DISTINCT aggregated.series_id, aggregated.destination_start
    FROM aggregated
    WHERE EXISTS (
        SELECT 1
        FROM network_observation_rollups destination
        WHERE destination.series_id = aggregated.series_id
          AND destination.bucket_secs = $1
          AND destination.bucket_start = aggregated.destination_start
    )
),
eligible AS MATERIALIZED (
    SELECT aggregated.*
    FROM aggregated
    LEFT JOIN conflicted_groups conflict
      ON conflict.series_id = aggregated.series_id
     AND conflict.destination_start = aggregated.destination_start
    WHERE conflict.series_id IS NULL
),
inserted AS (
    INSERT INTO network_observation_rollups (
        series_id, bucket_secs, bucket_start, health_state, reason_key,
        sample_count, transmitted_total, transmitted_sample_count,
        received_total, received_sample_count, latency_sum_ms,
        latency_sample_count, latency_min_ms, latency_max_ms,
        latency_mdev_sum_ms, latency_mdev_sample_count,
        packet_loss_sum_ratio, packet_loss_sample_count,
        packet_loss_min_ratio, packet_loss_max_ratio,
        latest_observation_id, latest_stale_after_secs, latest_healthy,
        latest_transmitted, latest_received, latest_latency_min_ms,
        latest_latency_avg_ms, latest_latency_max_ms,
        latest_latency_mdev_ms, latest_packet_loss_ratio, latest_reason,
        latest_observed_at, latest_received_at
    )
    SELECT
        series_id, $1, destination_start, health_state, reason_key,
        sample_count, transmitted_total, transmitted_sample_count,
        received_total, received_sample_count, latency_sum_ms,
        latency_sample_count, latency_min_ms, latency_max_ms,
        latency_mdev_sum_ms, latency_mdev_sample_count,
        packet_loss_sum_ratio, packet_loss_sample_count,
        packet_loss_min_ratio, packet_loss_max_ratio,
        latest_observation_id, latest_stale_after_secs, latest_healthy,
        latest_transmitted, latest_received, latest_latency_min_ms,
        latest_latency_avg_ms, latest_latency_max_ms,
        latest_latency_mdev_ms, latest_packet_loss_ratio, latest_reason,
        latest_observed_at, latest_received_at
    FROM eligible
    ON CONFLICT DO NOTHING
    RETURNING series_id, bucket_start, health_state, reason_key
),
fully_inserted_groups AS MATERIALIZED (
    SELECT eligible.series_id, eligible.destination_start
    FROM eligible
    LEFT JOIN inserted
      ON inserted.series_id = eligible.series_id
     AND inserted.bucket_start = eligible.destination_start
     AND inserted.health_state = eligible.health_state
     AND inserted.reason_key = eligible.reason_key
    GROUP BY eligible.series_id, eligible.destination_start
    HAVING COUNT(*) = COUNT(inserted.series_id)
),
deleted AS (
    DELETE FROM network_observations observation
    USING locked, fully_inserted_groups inserted_group
    WHERE observation.ctid = locked.source_ctid
      AND inserted_group.series_id = locked.automatic_series_id
      AND inserted_group.destination_start = locked.destination_start
    RETURNING observation.id
)
SELECT
    (SELECT COUNT(*)::bigint FROM deleted) AS source_rows,
    (SELECT COUNT(*)::bigint FROM inserted) AS destination_rows,
    (SELECT COUNT(*)::bigint FROM conflicted_groups) AS conflict_groups
"#;

const ROLLUP_PROMOTION_QUERY: &str = r#"
WITH oldest_group_seeds AS MATERIALIZED (
    SELECT
        series.id AS series_id,
        oldest.destination_start
    FROM network_observation_series series
    JOIN LATERAL (
        SELECT to_timestamp(
            floor(extract(epoch FROM source.bucket_start) / $1) * $1
        ) AS destination_start
        FROM network_observation_rollups source
        WHERE source.series_id = series.id
          AND source.bucket_secs < $1
          AND ($6::integer IS NULL
               OR source.bucket_start + make_interval(secs => source.bucket_secs) > (
                    date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                  ) - make_interval(days => $6))
          AND source.bucket_start < now() - interval '2 hours'
          AND source.bucket_start < (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
              ) - make_interval(days => $2)
          AND to_timestamp(
                floor(extract(epoch FROM source.bucket_start) / $1) * $1
              ) + make_interval(secs => $1)
              <= (
                    date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                 ) - make_interval(days => $2)
          AND ($5::integer IS NULL
               OR source.bucket_start >= (
                    date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                  ) - make_interval(days => $5))
        ORDER BY source.bucket_start, source.bucket_secs,
                 source.health_state, source.reason_key
        LIMIT 1
    ) oldest ON TRUE
    ORDER BY oldest.destination_start, series.id
    LIMIT $4
),
candidate_groups AS MATERIALIZED (
    SELECT
        seed.series_id,
        seed.destination_start,
        COUNT(*)::bigint AS source_count
    FROM oldest_group_seeds seed
    JOIN network_observation_rollups source
      ON source.series_id = seed.series_id
     AND source.bucket_start >= seed.destination_start
     AND source.bucket_start
           < seed.destination_start + make_interval(secs => $1)
    WHERE source.bucket_secs < $1
      AND ($6::integer IS NULL
           OR source.bucket_start + make_interval(secs => source.bucket_secs) > (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
              ) - make_interval(days => $6))
      AND source.bucket_start < now() - interval '2 hours'
      AND source.bucket_start < (
            date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
          ) - make_interval(days => $2)
      AND to_timestamp(
            floor(extract(epoch FROM source.bucket_start) / $1) * $1
          ) + make_interval(secs => $1)
          <= (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
             ) - make_interval(days => $2)
      AND ($5::integer IS NULL
           OR source.bucket_start >= (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
              ) - make_interval(days => $5))
    GROUP BY seed.series_id, seed.destination_start
),
bounded_groups AS MATERIALIZED (
    SELECT series_id, destination_start, source_count
    FROM (
        SELECT candidate_groups.*,
               SUM(source_count) OVER (
                   ORDER BY destination_start, series_id
               ) AS cumulative_count,
               row_number() OVER (
                   ORDER BY destination_start, series_id
               ) AS group_number
        FROM candidate_groups
    ) ranked
    WHERE cumulative_count <= $3 AND group_number <= $4
),
locked AS MATERIALIZED (
    SELECT source.ctid AS source_ctid, source.*, bounded.destination_start, bounded.source_count
    FROM network_observation_rollups source
    JOIN bounded_groups bounded
      ON bounded.series_id = source.series_id
     AND source.bucket_start >= bounded.destination_start
     AND source.bucket_start < bounded.destination_start + make_interval(secs => $1)
    WHERE source.bucket_secs < $1
      AND ($6::integer IS NULL
           OR source.bucket_start + make_interval(secs => source.bucket_secs) > (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
              ) - make_interval(days => $6))
      AND source.bucket_start < now() - interval '2 hours'
      AND source.bucket_start < (
            date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
          ) - make_interval(days => $2)
      AND bounded.destination_start + make_interval(secs => $1)
          <= (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
             ) - make_interval(days => $2)
      AND ($5::integer IS NULL
           OR source.bucket_start >= (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
              ) - make_interval(days => $5))
    FOR UPDATE OF source SKIP LOCKED
),
complete_groups AS MATERIALIZED (
    SELECT series_id, destination_start
    FROM locked
    GROUP BY series_id, destination_start, source_count
    HAVING COUNT(*) = source_count
),
normalized AS MATERIALIZED (
    SELECT locked.*
    FROM locked
    JOIN complete_groups complete USING (series_id, destination_start)
),
overlapping_groups AS MATERIALIZED (
    SELECT DISTINCT left_row.series_id, left_row.destination_start
    FROM normalized left_row
    JOIN normalized right_row
      ON right_row.series_id = left_row.series_id
     AND right_row.destination_start = left_row.destination_start
     AND right_row.source_ctid > left_row.source_ctid
     AND right_row.bucket_start
           < left_row.bucket_start + make_interval(secs => left_row.bucket_secs)
     AND left_row.bucket_start
           < right_row.bucket_start + make_interval(secs => right_row.bucket_secs)
),
valid_normalized AS MATERIALIZED (
    SELECT normalized.*
    FROM normalized
    LEFT JOIN overlapping_groups overlap USING (series_id, destination_start)
    WHERE overlap.series_id IS NULL
),
aggregated AS MATERIALIZED (
    SELECT
        series_id,
        destination_start,
        health_state,
        reason_key,
        SUM(sample_count)::bigint AS sample_count,
        SUM(transmitted_total)::numeric AS transmitted_total,
        SUM(transmitted_sample_count)::bigint AS transmitted_sample_count,
        SUM(received_total)::numeric AS received_total,
        SUM(received_sample_count)::bigint AS received_sample_count,
        SUM(latency_sum_ms)::double precision AS latency_sum_ms,
        SUM(latency_sample_count)::bigint AS latency_sample_count,
        MIN(latency_min_ms)::double precision AS latency_min_ms,
        MAX(latency_max_ms)::double precision AS latency_max_ms,
        SUM(latency_mdev_sum_ms)::double precision AS latency_mdev_sum_ms,
        SUM(latency_mdev_sample_count)::bigint AS latency_mdev_sample_count,
        SUM(packet_loss_sum_ratio)::double precision AS packet_loss_sum_ratio,
        SUM(packet_loss_sample_count)::bigint AS packet_loss_sample_count,
        MIN(packet_loss_min_ratio)::double precision AS packet_loss_min_ratio,
        MAX(packet_loss_max_ratio)::double precision AS packet_loss_max_ratio,
        (array_agg(latest_observation_id
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_observation_id,
        (array_agg(latest_stale_after_secs
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_stale_after_secs,
        (array_agg(latest_healthy
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_healthy,
        (array_agg(latest_transmitted
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_transmitted,
        (array_agg(latest_received
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_received,
        (array_agg(latest_latency_min_ms
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_latency_min_ms,
        (array_agg(latest_latency_avg_ms
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_latency_avg_ms,
        (array_agg(latest_latency_max_ms
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_latency_max_ms,
        (array_agg(latest_latency_mdev_ms
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_latency_mdev_ms,
        (array_agg(latest_packet_loss_ratio
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_packet_loss_ratio,
        (array_agg(latest_reason
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_reason,
        MAX(latest_observed_at) AS latest_observed_at,
        (array_agg(latest_received_at
            ORDER BY latest_observed_at DESC, latest_observation_id DESC))[1]
            AS latest_received_at
    FROM valid_normalized
    GROUP BY series_id, destination_start, health_state, reason_key
),
destination_conflicts AS MATERIALIZED (
    SELECT DISTINCT aggregated.series_id, aggregated.destination_start
    FROM aggregated
    WHERE EXISTS (
        SELECT 1
        FROM network_observation_rollups destination
        WHERE destination.series_id = aggregated.series_id
          AND destination.bucket_secs = $1
          AND destination.bucket_start = aggregated.destination_start
    )
),
conflicted_groups AS MATERIALIZED (
    SELECT series_id, destination_start FROM destination_conflicts
    UNION
    SELECT series_id, destination_start FROM overlapping_groups
),
eligible AS MATERIALIZED (
    SELECT aggregated.*
    FROM aggregated
    LEFT JOIN conflicted_groups conflict
      ON conflict.series_id = aggregated.series_id
     AND conflict.destination_start = aggregated.destination_start
    WHERE conflict.series_id IS NULL
),
inserted AS (
    INSERT INTO network_observation_rollups (
        series_id, bucket_secs, bucket_start, health_state, reason_key,
        sample_count, transmitted_total, transmitted_sample_count,
        received_total, received_sample_count, latency_sum_ms,
        latency_sample_count, latency_min_ms, latency_max_ms,
        latency_mdev_sum_ms, latency_mdev_sample_count,
        packet_loss_sum_ratio, packet_loss_sample_count,
        packet_loss_min_ratio, packet_loss_max_ratio,
        latest_observation_id, latest_stale_after_secs, latest_healthy,
        latest_transmitted, latest_received, latest_latency_min_ms,
        latest_latency_avg_ms, latest_latency_max_ms,
        latest_latency_mdev_ms, latest_packet_loss_ratio, latest_reason,
        latest_observed_at, latest_received_at
    )
    SELECT
        series_id, $1, destination_start, health_state, reason_key,
        sample_count, transmitted_total, transmitted_sample_count,
        received_total, received_sample_count, latency_sum_ms,
        latency_sample_count, latency_min_ms, latency_max_ms,
        latency_mdev_sum_ms, latency_mdev_sample_count,
        packet_loss_sum_ratio, packet_loss_sample_count,
        packet_loss_min_ratio, packet_loss_max_ratio,
        latest_observation_id, latest_stale_after_secs, latest_healthy,
        latest_transmitted, latest_received, latest_latency_min_ms,
        latest_latency_avg_ms, latest_latency_max_ms,
        latest_latency_mdev_ms, latest_packet_loss_ratio, latest_reason,
        latest_observed_at, latest_received_at
    FROM eligible
    ON CONFLICT DO NOTHING
    RETURNING series_id, bucket_start, health_state, reason_key
),
fully_inserted_groups AS MATERIALIZED (
    SELECT eligible.series_id, eligible.destination_start
    FROM eligible
    LEFT JOIN inserted
      ON inserted.series_id = eligible.series_id
     AND inserted.bucket_start = eligible.destination_start
     AND inserted.health_state = eligible.health_state
     AND inserted.reason_key = eligible.reason_key
    GROUP BY eligible.series_id, eligible.destination_start
    HAVING COUNT(*) = COUNT(inserted.series_id)
),
deleted AS (
    DELETE FROM network_observation_rollups source
    USING locked, fully_inserted_groups inserted_group
    WHERE source.ctid = locked.source_ctid
      AND inserted_group.series_id = locked.series_id
      AND inserted_group.destination_start = locked.destination_start
    RETURNING source.series_id
)
SELECT
    (SELECT COUNT(*)::bigint FROM deleted) AS source_rows,
    (SELECT COUNT(*)::bigint FROM inserted) AS destination_rows,
    (SELECT COUNT(*)::bigint FROM conflicted_groups) AS conflict_groups
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::PgWorkerTestDb;
    use uuid::Uuid;

    fn default_policy() -> NetworkObservationRetentionPolicy {
        NetworkObservationRetentionPolicy {
            enabled: true,
            retention_days: 3_650,
            prune_limit: 20_000,
        }
    }

    #[test]
    fn tier_schedule_matches_the_bounded_lts_contract() {
        assert_eq!(
            NETWORK_OBSERVATION_TIERS,
            &[
                (0, 300, 2),
                (300, 1_800, 8),
                (1_800, 3_600, 31),
                (3_600, 10_800, 91),
                (10_800, 21_600, 181),
                (21_600, 86_400, 366),
            ]
        );
    }

    #[test]
    fn exact_promotion_is_narrow_and_concurrency_safe() {
        assert!(EXACT_PROMOTION_QUERY.contains("source = 'automatic'"));
        assert!(EXACT_PROMOTION_QUERY.contains("kind = 'tunnel_reachability'"));
        assert!(EXACT_PROMOTION_QUERY.contains("oldest_group_seeds"));
        assert!(EXACT_PROMOTION_QUERY.contains("ORDER BY observation.observed_at"));
        assert!(EXACT_PROMOTION_QUERY.contains("FOR UPDATE OF observation SKIP LOCKED"));
        assert!(EXACT_PROMOTION_QUERY.contains("complete_groups"));
        assert!(!EXACT_PROMOTION_QUERY.contains("network_speed_test"));
    }

    #[test]
    fn rollup_promotion_never_overwrites_an_existing_destination() {
        assert!(ROLLUP_PROMOTION_QUERY.contains("oldest_group_seeds"));
        assert!(ROLLUP_PROMOTION_QUERY.contains("ORDER BY source.bucket_start"));
        assert!(ROLLUP_PROMOTION_QUERY.contains("ON CONFLICT DO NOTHING"));
        assert!(ROLLUP_PROMOTION_QUERY.contains("fully_inserted_groups"));
        assert!(ROLLUP_PROMOTION_QUERY.contains("FOR UPDATE OF source SKIP LOCKED"));
    }

    #[test]
    fn promotion_age_cutoffs_cannot_split_an_aligned_destination_bucket() {
        for query in [EXACT_PROMOTION_QUERY, ROLLUP_PROMOTION_QUERY] {
            assert!(!query.contains("now() - make_interval(days => $2)"));
            assert!(!query.contains("now() - make_interval(days => $5)"));
            assert!(
                query.contains("date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'")
            );
        }
        for &(_, destination_bucket_secs, _) in NETWORK_OBSERVATION_TIERS {
            assert_eq!(86_400 % destination_bucket_secs, 0);
        }
    }

    #[test]
    fn promotion_horizon_is_policy_driven_and_can_be_unbounded() {
        assert!(!EXACT_PROMOTION_QUERY.contains("interval '3650 days'"));
        assert!(!ROLLUP_PROMOTION_QUERY.contains("interval '3650 days'"));
        assert!(EXACT_PROMOTION_QUERY.contains("$7::integer IS NULL"));
        assert!(EXACT_PROMOTION_QUERY.contains("make_interval(days => $7)"));
        assert!(ROLLUP_PROMOTION_QUERY.contains("$6::integer IS NULL"));
        assert!(ROLLUP_PROMOTION_QUERY.contains("make_interval(days => $6)"));
    }

    #[tokio::test]
    async fn postgres_oldest_bucket_discovery_keeps_many_series_moving() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        sqlx::query(
            r#"
            INSERT INTO clients (id, display_name, public_key, status)
            VALUES
                ('tunnel-fair-left', 'tunnel-fair-left', decode('', 'hex'), 'online'),
                ('tunnel-fair-right', 'tunnel-fair-right', decode('', 'hex'), 'online')
            "#,
        )
        .execute(&db.pool)
        .await
        .unwrap();
        let plan_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO tunnel_plans (
                id, name, kind, left_client_id, right_client_id, input, plan
            ) VALUES ($1, 'tunnel-fair-plan', 'wireguard',
                'tunnel-fair-left', 'tunnel-fair-right', '{}'::jsonb, '{}'::jsonb)
            "#,
        )
        .bind(plan_id)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            WITH inserted_series AS (
                INSERT INTO network_observation_series (
                    plan_id, topology_identity_hash, plan_name, interface_name,
                    client_id, peer_client_id, endpoint_side, address_family, target
                )
                SELECT $1, 'fair-identity', 'tunnel-fair-plan',
                    'tun-' || ordinal::text, 'tunnel-fair-left',
                    'tunnel-fair-right', 'left', 'ipv4',
                    'probe-' || ordinal::text
                FROM generate_series(1, 120) ordinal
                RETURNING id, interface_name, target
            )
            INSERT INTO network_observations (
                id, client_id, kind, source, role, plan_id,
                topology_identity_hash, plan_name, interface_name,
                peer_client_id, target, endpoint_side, address_family,
                stale_after_secs, healthy, transmitted, received,
                latency_avg_ms, packet_loss_ratio, automatic_series_id,
                observed_at, received_at
            )
            SELECT md5(id::text || $1::text)::uuid,
                'tunnel-fair-left', 'tunnel_reachability', 'automatic',
                'endpoint', $1, 'fair-identity', 'tunnel-fair-plan',
                interface_name, 'tunnel-fair-right', target, 'left', 'ipv4',
                180, TRUE, 3, 3, 10.0, 0.0, id,
                date_trunc('day', now() - interval '400 days') + interval '1 minute',
                date_trunc('day', now() - interval '400 days') + interval '1 minute'
            FROM inserted_series
            "#,
        )
        .bind(plan_id)
        .execute(&db.pool)
        .await
        .unwrap();

        let run = process_network_observation_retention(&db.pool, default_policy())
            .await
            .unwrap();
        assert_eq!(run.source_rows_promoted, 120);
        let promoted_series: i64 = sqlx::query_scalar(
            "SELECT count(DISTINCT series_id) FROM network_observation_rollups WHERE bucket_secs = 86400",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(promoted_series, 120);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_promotes_only_automatic_reachability_and_preserves_latest() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        sqlx::query(
            r#"
            INSERT INTO clients (id, display_name, public_key, status)
            VALUES
                ('tunnel-tier-left', 'tunnel-tier-left', decode('', 'hex'), 'online'),
                ('tunnel-tier-right', 'tunnel-tier-right', decode('', 'hex'), 'online')
            "#,
        )
        .execute(&db.pool)
        .await
        .unwrap();
        let plan_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO tunnel_plans (
                id, name, kind, left_client_id, right_client_id, input, plan
            ) VALUES ($1, 'tunnel-tier-plan', 'wireguard',
                'tunnel-tier-left', 'tunnel-tier-right', '{}'::jsonb, '{}'::jsonb)
            "#,
        )
        .bind(plan_id)
        .execute(&db.pool)
        .await
        .unwrap();
        let series_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO network_observation_series (
                plan_id, topology_identity_hash, plan_name, interface_name,
                client_id, peer_client_id, endpoint_side, address_family, target
            ) VALUES ($1, 'tier-identity', 'tunnel-tier-plan', 'tun-tier',
                'tunnel-tier-left', 'tunnel-tier-right', 'left', 'ipv4', '10.0.0.2')
            RETURNING id
            "#,
        )
        .bind(plan_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let automatic_ids = [Uuid::new_v4(), Uuid::new_v4()];
        for (offset, observation_id) in automatic_ids.into_iter().enumerate() {
            sqlx::query(
                r#"
                INSERT INTO network_observations (
                    id, client_id, kind, source, role, plan_id,
                    topology_identity_hash, plan_name, interface_name,
                    peer_client_id, target, endpoint_side, address_family,
                    stale_after_secs, healthy, transmitted, received,
                    latency_min_ms, latency_avg_ms, latency_max_ms,
                    latency_mdev_ms, packet_loss_ratio, reason,
                    automatic_series_id, observed_at, received_at
                ) VALUES (
                    $1, 'tunnel-tier-left', 'tunnel_reachability', 'automatic',
                    'endpoint', $2, 'tier-identity', 'tunnel-tier-plan', 'tun-tier',
                    'tunnel-tier-right', '10.0.0.2', 'left', 'ipv4', 180,
                    TRUE, 3, 3, $3, $3, $3, 0.1, 0.0, NULL, $4,
                    date_trunc('day', now() - interval '400 days')
                        + make_interval(mins => $5),
                    date_trunc('day', now() - interval '400 days')
                        + make_interval(mins => $5)
                )
                "#,
            )
            .bind(observation_id)
            .bind(plan_id)
            .bind(if offset == 0 { 10.0 } else { 30.0 })
            .bind(series_id)
            .bind(i32::try_from(offset * 10).unwrap())
            .execute(&db.pool)
            .await
            .unwrap();
        }
        sqlx::query(
            r#"
            INSERT INTO network_observation_latest (
                series_id, observation_id, stale_after_secs, healthy,
                transmitted, received, latency_min_ms, latency_avg_ms,
                latency_max_ms, latency_mdev_ms, packet_loss_ratio,
                observed_at, received_at
            ) VALUES (
                $1, $2, 180, TRUE, 3, 3, 30.0, 30.0, 30.0, 0.1, 0.0,
                date_trunc('day', now() - interval '400 days') + interval '10 minutes',
                date_trunc('day', now() - interval '400 days') + interval '10 minutes'
            )
            "#,
        )
        .bind(series_id)
        .bind(automatic_ids[1])
        .execute(&db.pool)
        .await
        .unwrap();
        let manual_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO network_observations (
                id, client_id, kind, source, role, plan_id,
                topology_identity_hash, plan_name, interface_name,
                peer_client_id, target, endpoint_side, address_family,
                healthy, throughput_mbps, bytes, observed_at, received_at
            ) VALUES (
                $1, 'tunnel-tier-left', 'network_speed_test', 'manual', 'client',
                $2, 'tier-identity', 'tunnel-tier-plan', 'tun-tier',
                'tunnel-tier-right', '10.0.0.2:5201', 'left', 'ipv4',
                TRUE, 50.0, 1048576, now() - interval '400 days',
                now() - interval '400 days'
            )
            "#,
        )
        .bind(manual_id)
        .bind(plan_id)
        .execute(&db.pool)
        .await
        .unwrap();

        let run = process_network_observation_retention(&db.pool, default_policy())
            .await
            .unwrap();
        assert_eq!(run.source_rows_promoted, 2);
        assert_eq!(run.destination_conflicts, 0);
        let retained: (i32, i64, f64) = sqlx::query_as(
            r#"
            SELECT bucket_secs, sample_count, latency_sum_ms
            FROM network_observation_rollups
            WHERE series_id = $1
            "#,
        )
        .bind(series_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(retained, (86_400, 2, 40.0));
        let remaining_manual: i64 =
            sqlx::query_scalar("SELECT count(*) FROM network_observations WHERE id = $1")
                .bind(manual_id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(remaining_manual, 1);
        let latest_exact: i64 = sqlx::query_scalar(
            r#"
            SELECT count(*) FROM network_observation_exact_evidence
            WHERE source = 'automatic' AND id = $1
            "#,
        )
        .bind(automatic_ids[1])
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(latest_exact, 1);

        let late_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO network_observations (
                id, client_id, kind, source, role, plan_id,
                topology_identity_hash, plan_name, interface_name,
                peer_client_id, target, endpoint_side, address_family,
                stale_after_secs, healthy, transmitted, received,
                latency_avg_ms, packet_loss_ratio, automatic_series_id,
                observed_at, received_at
            )
            SELECT $1, 'tunnel-tier-left', 'tunnel_reachability', 'automatic',
                'endpoint', $2, 'tier-identity', 'tunnel-tier-plan', 'tun-tier',
                'tunnel-tier-right', '10.0.0.2', 'left', 'ipv4', 180,
                TRUE, 3, 3, 20.0, 0.0, $3,
                bucket_start + interval '20 minutes',
                bucket_start + interval '20 minutes'
            FROM network_observation_rollups
            WHERE series_id = $3 AND bucket_secs = 86400
            LIMIT 1
            "#,
        )
        .bind(late_id)
        .bind(plan_id)
        .bind(series_id)
        .execute(&db.pool)
        .await
        .unwrap();
        let conflict = process_network_observation_retention(&db.pool, default_policy())
            .await
            .unwrap();
        assert!(conflict.destination_conflicts >= 1);
        let late_still_exact: i64 =
            sqlx::query_scalar("SELECT count(*) FROM network_observations WHERE id = $1")
                .bind(late_id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(late_still_exact, 1);

        let expired_exact_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO network_observations (
                id, client_id, kind, source, role, plan_id,
                topology_identity_hash, plan_name, interface_name,
                peer_client_id, target, endpoint_side, address_family,
                stale_after_secs, healthy, transmitted, received,
                latency_avg_ms, packet_loss_ratio, automatic_series_id,
                observed_at, received_at
            ) VALUES (
                $1, 'tunnel-tier-left', 'tunnel_reachability', 'automatic',
                'endpoint', $2, 'tier-identity', 'tunnel-tier-plan', 'tun-tier',
                'tunnel-tier-right', '10.0.0.2', 'left', 'ipv4', 180,
                TRUE, 3, 3, 20.0, 0.0, $3,
                date_trunc('day', now()) - interval '3651 days',
                date_trunc('day', now()) - interval '3651 days'
            )
            "#,
        )
        .bind(expired_exact_id)
        .bind(plan_id)
        .bind(series_id)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            WITH widths(bucket_secs) AS (
                VALUES (300), (1800), (3600), (10800), (21600), (86400)
            )
            INSERT INTO network_observation_rollups
            SELECT (jsonb_populate_record(
                NULL::network_observation_rollups,
                to_jsonb(template) || jsonb_build_object(
                    'bucket_secs', widths.bucket_secs,
                    'bucket_start', date_trunc('day', now()) - interval '3651 days',
                    'latest_observed_at', date_trunc('day', now()) - interval '3651 days',
                    'latest_received_at', date_trunc('day', now()) - interval '3651 days'
                )
            )).*
            FROM (
                SELECT * FROM network_observation_rollups
                WHERE series_id = $1 AND bucket_secs = 86400
                LIMIT 1
            ) template CROSS JOIN widths
            "#,
        )
        .bind(series_id)
        .execute(&db.pool)
        .await
        .unwrap();
        let cap_run = process_network_observation_retention(&db.pool, default_policy())
            .await
            .unwrap();
        assert_eq!(cap_run.expired_exact_rows_pruned, 1);
        assert_eq!(cap_run.expired_rollup_rows_pruned, 6);
        let expired_exact_remaining: i64 =
            sqlx::query_scalar("SELECT count(*) FROM network_observations WHERE id = $1")
                .bind(expired_exact_id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(expired_exact_remaining, 0);
        let latest_snapshot_remaining: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM network_observation_latest WHERE series_id = $1",
        )
        .bind(series_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(latest_snapshot_remaining, 1);

        let mixed_series_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO network_observation_series (
                plan_id, topology_identity_hash, plan_name, interface_name,
                client_id, peer_client_id, endpoint_side, address_family, target
            ) VALUES ($1, 'tier-identity', 'tunnel-tier-plan', 'tun-tier',
                'tunnel-tier-left', 'tunnel-tier-right', 'left', 'ipv6', 'fd00::2')
            RETURNING id
            "#,
        )
        .bind(plan_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            WITH template AS (
                SELECT *
                FROM network_observation_rollups
                WHERE series_id = $1 AND bucket_secs = 86400
                LIMIT 1
            ), variants(bucket_secs, offset_secs, samples, latency_sum) AS (
                VALUES (300, 0, 1, 10.0),
                       (1800, 1800, 2, 40.0),
                       (3600, 3600, 3, 90.0)
            )
            INSERT INTO network_observation_rollups
            SELECT (jsonb_populate_record(
                NULL::network_observation_rollups,
                to_jsonb(template) || jsonb_build_object(
                    'series_id', $2,
                    'bucket_secs', variants.bucket_secs,
                    'bucket_start', template.bucket_start
                        + make_interval(secs => variants.offset_secs),
                    'sample_count', variants.samples,
                    'latency_sum_ms', variants.latency_sum,
                    'latency_sample_count', variants.samples,
                    'latest_observed_at', template.bucket_start
                        + make_interval(secs => variants.offset_secs),
                    'latest_received_at', template.bucket_start
                        + make_interval(secs => variants.offset_secs)
                )
            )).*
            FROM template CROSS JOIN variants
            "#,
        )
        .bind(series_id)
        .bind(mixed_series_id)
        .execute(&db.pool)
        .await
        .unwrap();
        let mixed_run = process_network_observation_retention(&db.pool, default_policy())
            .await
            .unwrap();
        assert!(mixed_run.source_rows_promoted >= 3);
        let mixed_daily: (i64, f64) = sqlx::query_as(
            r#"
            SELECT sample_count, latency_sum_ms
            FROM network_observation_rollups
            WHERE series_id = $1 AND bucket_secs = 86400
            "#,
        )
        .bind(mixed_series_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(mixed_daily, (6, 140.0));
        let mixed_finer: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM network_observation_rollups WHERE series_id = $1 AND bucket_secs < 86400",
        )
        .bind(mixed_series_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(mixed_finer, 0);

        let empty_inactive_series_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO network_observation_series (
                plan_id, topology_identity_hash, plan_name, interface_name,
                client_id, peer_client_id, endpoint_side, address_family, target,
                active, last_seen_at
            ) VALUES ($1, 'inactive-empty', 'tunnel-tier-plan', 'tun-tier-old',
                'tunnel-tier-left', 'tunnel-tier-right', 'left', 'ipv4', '10.0.0.3',
                FALSE, now() - interval '3 days')
            RETURNING id
            "#,
        )
        .bind(plan_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let retained_inactive_series_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO network_observation_series (
                plan_id, topology_identity_hash, plan_name, interface_name,
                client_id, peer_client_id, endpoint_side, address_family, target,
                active, last_seen_at
            ) VALUES ($1, 'inactive-retained', 'tunnel-tier-plan', 'tun-tier-old',
                'tunnel-tier-left', 'tunnel-tier-right', 'left', 'ipv6', 'fd00::3',
                FALSE, now() - interval '3 days')
            RETURNING id
            "#,
        )
        .bind(plan_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO network_observation_rollups
            SELECT (jsonb_populate_record(
                NULL::network_observation_rollups,
                to_jsonb(template) || jsonb_build_object(
                    'series_id', $2,
                    'bucket_start', date_trunc('day', now() - interval '400 days')
                )
            )).*
            FROM (
                SELECT *
                FROM network_observation_rollups
                WHERE series_id = $1 AND bucket_secs = 86400
                LIMIT 1
            ) template
            "#,
        )
        .bind(series_id)
        .bind(retained_inactive_series_id)
        .execute(&db.pool)
        .await
        .unwrap();
        let cleanup_run = process_network_observation_retention(&db.pool, default_policy())
            .await
            .unwrap();
        assert_eq!(cleanup_run.inactive_series_pruned, 1);
        let inactive_series: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM network_observation_series WHERE id = ANY($1) ORDER BY id",
        )
        .bind(vec![empty_inactive_series_id, retained_inactive_series_id])
        .fetch_all(&db.pool)
        .await
        .unwrap();
        assert_eq!(inactive_series, vec![retained_inactive_series_id]);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_policy_disables_only_terminal_pruning_and_bounds_custom_horizon_deletes() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        sqlx::query(
            r#"
            INSERT INTO clients (id, display_name, public_key, status)
            VALUES
                ('policy-tier-left', 'policy-tier-left', decode('', 'hex'), 'online'),
                ('policy-tier-right', 'policy-tier-right', decode('', 'hex'), 'online')
            "#,
        )
        .execute(&db.pool)
        .await
        .unwrap();
        let plan_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO tunnel_plans (
                id, name, kind, left_client_id, right_client_id, input, plan
            ) VALUES ($1, 'policy-tier-plan', 'wireguard',
                'policy-tier-left', 'policy-tier-right', '{}'::jsonb, '{}'::jsonb)
            "#,
        )
        .bind(plan_id)
        .execute(&db.pool)
        .await
        .unwrap();
        let series_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO network_observation_series (
                plan_id, topology_identity_hash, plan_name, interface_name,
                client_id, peer_client_id, endpoint_side, address_family, target
            ) VALUES ($1, 'policy-tier-identity', 'policy-tier-plan', 'tun-policy',
                'policy-tier-left', 'policy-tier-right', 'left', 'ipv4', '10.1.0.2')
            RETURNING id
            "#,
        )
        .bind(plan_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let ancient_id = Uuid::new_v4();
        let compactable_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO network_observations (
                id, client_id, kind, source, role, plan_id,
                topology_identity_hash, plan_name, interface_name,
                peer_client_id, target, endpoint_side, address_family,
                stale_after_secs, healthy, transmitted, received,
                latency_avg_ms, packet_loss_ratio, automatic_series_id,
                observed_at, received_at
            ) VALUES
                ($1, 'policy-tier-left', 'tunnel_reachability', 'automatic',
                    'endpoint', $3, 'policy-tier-identity', 'policy-tier-plan',
                    'tun-policy', 'policy-tier-right', '10.1.0.2', 'left', 'ipv4',
                    180, TRUE, 3, 3, 10.0, 0.0, $4,
                    date_trunc('day', now()) - interval '4001 days',
                    date_trunc('day', now()) - interval '4001 days'),
                ($2, 'policy-tier-left', 'tunnel_reachability', 'automatic',
                    'endpoint', $3, 'policy-tier-identity', 'policy-tier-plan',
                    'tun-policy', 'policy-tier-right', '10.1.0.2', 'left', 'ipv4',
                    180, TRUE, 3, 3, 20.0, 0.0, $4,
                    date_trunc('day', now()) - interval '3 days' + interval '1 minute',
                    date_trunc('day', now()) - interval '3 days' + interval '1 minute')
            "#,
        )
        .bind(ancient_id)
        .bind(compactable_id)
        .bind(plan_id)
        .bind(series_id)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO history_retention_policies (
                domain, retention_days, prune_limit, enabled,
                metadata_only, export_enabled
            ) VALUES ('network_observations', 10, 1, FALSE, FALSE, TRUE)
            "#,
        )
        .execute(&db.pool)
        .await
        .unwrap();

        let disabled = crate::history_retention::process_telemetry_history_retention(&db.pool)
            .await
            .unwrap();
        assert_eq!(disabled.network_observation_expired_exact_rows_pruned, 0);
        assert_eq!(disabled.network_observation_expired_rollup_rows_pruned, 0);
        assert_eq!(disabled.network_observation_source_rows_promoted, 2);
        let disabled_ancient_remaining: i64 =
            sqlx::query_scalar("SELECT count(*) FROM network_observations WHERE id = $1")
                .bind(ancient_id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(disabled_ancient_remaining, 0);
        let disabled_ancient_retained: i64 = sqlx::query_scalar(
            r#"
            SELECT count(*) FROM network_observation_rollups
            WHERE series_id = $1
              AND bucket_secs = 86400
              AND bucket_start = date_trunc('day', now()) - interval '4001 days'
            "#,
        )
        .bind(series_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(disabled_ancient_retained, 1);
        let fixed_tier_rows: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM network_observation_rollups WHERE series_id = $1 AND bucket_secs = 300",
        )
        .bind(series_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(fixed_tier_rows, 1);

        sqlx::query(
            r#"
            INSERT INTO network_observations (
                id, client_id, kind, source, role, plan_id,
                topology_identity_hash, plan_name, interface_name,
                peer_client_id, target, endpoint_side, address_family,
                stale_after_secs, healthy, transmitted, received,
                latency_avg_ms, packet_loss_ratio, automatic_series_id,
                observed_at, received_at
            )
            SELECT md5($1::text || ordinal::text)::uuid,
                'policy-tier-left', 'tunnel_reachability', 'automatic',
                'endpoint', $2, 'policy-tier-identity', 'policy-tier-plan',
                'tun-policy', 'policy-tier-right', '10.1.0.2', 'left', 'ipv4',
                180, TRUE, 3, 3, 30.0, 0.0, $3,
                date_trunc('day', now()) - make_interval(days => 4001 + ordinal),
                date_trunc('day', now()) - make_interval(days => 4001 + ordinal)
            FROM generate_series(1, 3) ordinal
            "#,
        )
        .bind(series_id)
        .bind(plan_id)
        .bind(series_id)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            WITH template AS (
                SELECT *
                FROM network_observation_rollups
                WHERE series_id = $1 AND bucket_secs = 300
                LIMIT 1
            )
            INSERT INTO network_observation_rollups
            SELECT (jsonb_populate_record(
                NULL::network_observation_rollups,
                to_jsonb(template) || jsonb_build_object(
                    'bucket_secs', 86400,
                    'bucket_start', date_trunc('day', now())
                        - make_interval(days => 4001 + ordinal),
                    'latest_observed_at', date_trunc('day', now())
                        - make_interval(days => 4001 + ordinal),
                    'latest_received_at', date_trunc('day', now())
                        - make_interval(days => 4001 + ordinal)
                )
            )).*
            FROM template CROSS JOIN generate_series(1, 3) ordinal
            "#,
        )
        .bind(series_id)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            UPDATE history_retention_policies
            SET enabled = TRUE, retention_days = 10, prune_limit = 2
            WHERE domain = 'network_observations'
            "#,
        )
        .execute(&db.pool)
        .await
        .unwrap();

        let first_enabled = crate::history_retention::process_telemetry_history_retention(&db.pool)
            .await
            .unwrap();
        assert_eq!(
            first_enabled.network_observation_expired_exact_rows_pruned,
            2
        );
        assert_eq!(
            first_enabled.network_observation_expired_rollup_rows_pruned,
            0
        );
        assert_eq!(first_enabled.network_observation_source_rows_promoted, 0);
        assert_eq!(
            first_enabled.network_observation_expired_exact_rows_pruned
                + first_enabled.network_observation_expired_rollup_rows_pruned,
            2
        );
        let expired_exact_remaining: i64 = sqlx::query_scalar(
            r#"
            SELECT count(*) FROM network_observations
            WHERE automatic_series_id = $1
              AND observed_at < date_trunc('day', now()) - interval '10 days'
            "#,
        )
        .bind(series_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(expired_exact_remaining, 1);
        let expired_rollups_remaining: i64 = sqlx::query_scalar(
            r#"
            SELECT count(*) FROM network_observation_rollups
            WHERE series_id = $1
              AND bucket_start + make_interval(secs => bucket_secs)
                    <= date_trunc('day', now()) - interval '10 days'
            "#,
        )
        .bind(series_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(expired_rollups_remaining, 4);

        let second_enabled =
            crate::history_retention::process_telemetry_history_retention(&db.pool)
                .await
                .unwrap();
        assert_eq!(
            second_enabled.network_observation_expired_exact_rows_pruned,
            1
        );
        assert_eq!(
            second_enabled.network_observation_expired_rollup_rows_pruned,
            1
        );
        assert_eq!(
            second_enabled.network_observation_expired_exact_rows_pruned
                + second_enabled.network_observation_expired_rollup_rows_pruned,
            2
        );
        let third_enabled = crate::history_retention::process_telemetry_history_retention(&db.pool)
            .await
            .unwrap();
        assert_eq!(
            third_enabled.network_observation_expired_exact_rows_pruned,
            0
        );
        assert_eq!(
            third_enabled.network_observation_expired_rollup_rows_pruned,
            2
        );
        assert_eq!(
            third_enabled.network_observation_expired_exact_rows_pruned
                + third_enabled.network_observation_expired_rollup_rows_pruned,
            2
        );
        let final_expired_rollups: i64 = sqlx::query_scalar(
            r#"
            SELECT count(*) FROM network_observation_rollups
            WHERE series_id = $1
              AND bucket_start + make_interval(secs => bucket_secs)
                    <= date_trunc('day', now()) - interval '10 days'
            "#,
        )
        .bind(series_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(final_expired_rollups, 1);
        db.cleanup().await;
    }
}
