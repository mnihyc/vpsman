use anyhow::{ensure, Result};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use vpsman_common::TELEMETRY_HISTORY_TIERS;

use crate::history_retention::{optional_database_deadline, DatabaseDeadline};

const LIFECYCLE_PRUNE_LIMIT: i64 = 20_000;

/// Closed telemetry minutes write the common 60-second tier. Retention moves
/// each disjoint fragment through one adjacent UTC-aligned edge at a time;
/// every destination write durably publishes its successor. Sparse automatic
/// locators retain one-day detail while manual job evidence remains exact.
pub(crate) const NETWORK_OBSERVATION_TIERS: &[(i32, i32, i32)] = &[
    (
        TELEMETRY_HISTORY_TIERS[0].bucket_secs,
        TELEMETRY_HISTORY_TIERS[1].bucket_secs,
        TELEMETRY_HISTORY_TIERS[0].retain_days,
    ),
    (
        TELEMETRY_HISTORY_TIERS[1].bucket_secs,
        TELEMETRY_HISTORY_TIERS[2].bucket_secs,
        TELEMETRY_HISTORY_TIERS[1].retain_days,
    ),
    (
        TELEMETRY_HISTORY_TIERS[2].bucket_secs,
        TELEMETRY_HISTORY_TIERS[3].bucket_secs,
        TELEMETRY_HISTORY_TIERS[2].retain_days,
    ),
    (
        TELEMETRY_HISTORY_TIERS[3].bucket_secs,
        TELEMETRY_HISTORY_TIERS[4].bucket_secs,
        TELEMETRY_HISTORY_TIERS[3].retain_days,
    ),
    (
        TELEMETRY_HISTORY_TIERS[4].bucket_secs,
        TELEMETRY_HISTORY_TIERS[5].bucket_secs,
        TELEMETRY_HISTORY_TIERS[4].retain_days,
    ),
    (
        TELEMETRY_HISTORY_TIERS[5].bucket_secs,
        TELEMETRY_HISTORY_TIERS[6].bucket_secs,
        TELEMETRY_HISTORY_TIERS[5].retain_days,
    ),
];

const NETWORK_OBSERVATION_BUCKET_SECS: [i32; TELEMETRY_HISTORY_TIERS.len()] = [
    TELEMETRY_HISTORY_TIERS[0].bucket_secs,
    TELEMETRY_HISTORY_TIERS[1].bucket_secs,
    TELEMETRY_HISTORY_TIERS[2].bucket_secs,
    TELEMETRY_HISTORY_TIERS[3].bucket_secs,
    TELEMETRY_HISTORY_TIERS[4].bucket_secs,
    TELEMETRY_HISTORY_TIERS[5].bucket_secs,
    TELEMETRY_HISTORY_TIERS[6].bucket_secs,
];

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NetworkObservationRetentionRun {
    pub(crate) source_rows_promoted: u64,
    pub(crate) destination_rows_written: u64,
    pub(crate) expired_exact_rows_pruned: u64,
    pub(crate) expired_rollup_rows_pruned: u64,
    pub(crate) inactive_latest_pruned: u64,
    pub(crate) inactive_series_pruned: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NetworkObservationRetentionPhaseOutcome {
    pub(crate) run: NetworkObservationRetentionRun,
    /// False only when this adapter's indexed preprobe (or disabled policy)
    /// proved the phase Current without attempting a mutation page.
    pub(crate) attempted: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct NetworkObservationRetentionPolicy {
    pub(crate) enabled: bool,
    pub(crate) retention_days: i32,
    pub(crate) prune_limit: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NetworkObservationRetentionPhase {
    TerminalPrune,
    RollupToDay,
    RollupToSixHours,
    RollupToThreeHours,
    RollupToHour,
    RollupToThirtyMinutes,
    RollupToFiveMinutes,
    InactiveLatestPrune,
    InactiveSeriesPrune,
}

impl NetworkObservationRetentionPhase {
    fn rollup_tier_index(self) -> Option<usize> {
        match self {
            Self::RollupToDay => Some(5),
            Self::RollupToSixHours => Some(4),
            Self::RollupToThreeHours => Some(3),
            Self::RollupToHour => Some(2),
            Self::RollupToThirtyMinutes => Some(1),
            Self::RollupToFiveMinutes => Some(0),
            _ => None,
        }
    }

    pub(crate) fn due_span_key(self) -> Option<(&'static str, i32, i32)> {
        self.rollup_tier_index().map(|index| {
            (
                "network_observation_rollups",
                NETWORK_OBSERVATION_TIERS[index].0,
                NETWORK_OBSERVATION_TIERS[index].1,
            )
        })
    }
}

/// Executes one bounded network-observation lifecycle phase. Manual evidence,
/// automatic-series grouping, health-state aggregates, and tier horizons stay
/// inside this domain adapter; only fair page selection is shared.
pub(crate) async fn process_network_observation_retention_phase(
    pool: &PgPool,
    policy: NetworkObservationRetentionPolicy,
    phase: NetworkObservationRetentionPhase,
) -> Result<NetworkObservationRetentionPhaseOutcome> {
    let mut run = NetworkObservationRetentionRun::default();
    if phase == NetworkObservationRetentionPhase::TerminalPrune {
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
        return Ok(NetworkObservationRetentionPhaseOutcome {
            run,
            attempted: policy.enabled,
        });
    }
    if let Some(index) = phase.rollup_tier_index() {
        let (source_bucket_secs, destination_bucket_secs, retain_days) =
            NETWORK_OBSERVATION_TIERS[index];
        let promoted = promote_rollups(
            pool,
            source_bucket_secs,
            destination_bucket_secs,
            retain_days,
            policy.enabled.then_some(policy.retention_days),
        )
        .await?;
        add_promotion_result(
            &mut run,
            source_bucket_secs,
            destination_bucket_secs,
            promoted.result,
        );
        return Ok(NetworkObservationRetentionPhaseOutcome {
            run,
            attempted: promoted.attempted,
        });
    }
    if phase == NetworkObservationRetentionPhase::InactiveLatestPrune {
        run.inactive_latest_pruned = prune_inactive_latest(pool).await?;
    }
    if phase == NetworkObservationRetentionPhase::InactiveSeriesPrune {
        run.inactive_series_pruned = prune_empty_inactive_series(pool).await?;
    }
    Ok(NetworkObservationRetentionPhaseOutcome {
        run,
        attempted: true,
    })
}

/// Re-check the exact frontier owned by one observation lifecycle phase after
/// its bounded page. Locked rows remain visible to these probes, so a skipped
/// row is StillDue rather than a false completion.
pub(crate) async fn network_observation_retention_phase_has_remaining_work(
    pool: &PgPool,
    policy: NetworkObservationRetentionPolicy,
    phase: NetworkObservationRetentionPhase,
) -> Result<bool> {
    if let Some(index) = phase.rollup_tier_index() {
        let (_, source_bucket_secs, destination_bucket_secs) = phase
            .due_span_key()
            .expect("network-observation rollup phase has a durable due-span owner");
        let (_, configured_destination_bucket_secs, _) = NETWORK_OBSERVATION_TIERS[index];
        debug_assert_eq!(destination_bucket_secs, configured_destination_bucket_secs);
        return observation_rollup_promotion_is_due(
            pool,
            source_bucket_secs,
            destination_bucket_secs,
        )
        .await;
    }

    // Keep each ordered LIMIT behind a derived-table boundary. EXISTS alone
    // does not preserve ordering and may otherwise generic-plan a full scan.
    match phase {
        NetworkObservationRetentionPhase::TerminalPrune if policy.enabled => {
            Ok(sqlx::query_scalar(
                r#"
                SELECT EXISTS (
                    SELECT 1 FROM (
                        SELECT 1
                        FROM network_observations observation
                        WHERE observation.source = 'manual'
                          AND observation.observed_at
                                < now() - make_interval(days => $1)
                        ORDER BY observation.observed_at
                        LIMIT 1
                    ) bounded_manual
                ) OR EXISTS (
                    SELECT 1
                    FROM unnest($2::integer[]) AS tier(bucket_secs)
                    JOIN LATERAL (
                        SELECT 1
                        FROM network_observation_rollups rollup
                        WHERE rollup.bucket_secs = tier.bucket_secs
                          AND rollup.bucket_start <= now()
                                - make_interval(days => $1)
                                - make_interval(secs => tier.bucket_secs)
                        ORDER BY rollup.bucket_start, rollup.series_id,
                                 rollup.health_state
                        LIMIT 1
                    ) bounded_rollup ON TRUE
                )
                "#,
            )
            .bind(policy.retention_days)
            .bind(NETWORK_OBSERVATION_BUCKET_SECS.as_slice())
            .fetch_one(pool)
            .await?)
        }
        NetworkObservationRetentionPhase::TerminalPrune => Ok(false),
        NetworkObservationRetentionPhase::InactiveLatestPrune => Ok(sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM (
                    SELECT 1
                    FROM network_observation_latest latest
                    JOIN network_observation_series series
                      ON series.id = latest.series_id
                    WHERE series.active = FALSE
                      AND latest.observed_at < now() - interval '2 days'
                    ORDER BY latest.observed_at, latest.series_id
                    LIMIT 1
                ) bounded_due
            )
            "#,
        )
        .fetch_one(pool)
        .await?),
        NetworkObservationRetentionPhase::InactiveSeriesPrune => Ok(sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM (
                    SELECT 1
                    FROM network_observation_series series
                    WHERE series.active = FALSE
                      AND series.last_seen_at < now() - interval '2 days'
                      AND NOT EXISTS (
                          SELECT 1 FROM network_observations observation
                          WHERE observation.automatic_series_id = series.id
                      )
                      AND NOT EXISTS (
                          SELECT 1 FROM network_observation_rollups rollup
                          WHERE rollup.series_id = series.id
                      )
                      AND NOT EXISTS (
                          SELECT 1 FROM network_observation_latest latest
                          WHERE latest.series_id = series.id
                      )
                    ORDER BY series.last_seen_at, series.id
                    LIMIT 1
                ) bounded_due
            )
            "#,
        )
        .fetch_one(pool)
        .await?),
        NetworkObservationRetentionPhase::RollupToDay
        | NetworkObservationRetentionPhase::RollupToSixHours
        | NetworkObservationRetentionPhase::RollupToThreeHours
        | NetworkObservationRetentionPhase::RollupToHour
        | NetworkObservationRetentionPhase::RollupToThirtyMinutes
        | NetworkObservationRetentionPhase::RollupToFiveMinutes => {
            unreachable!("rollup phases return from their exact due probe")
        }
    }
}

/// Exact future eligibility from the same bounded observation frontier used by
/// the corresponding readiness/claim path. An empty frontier is producer-only
/// and is recovered by parent invalidation plus the scheduler watchdog.
pub(crate) async fn network_observation_retention_phase_next_at(
    pool: &PgPool,
    policy: NetworkObservationRetentionPolicy,
    phase: NetworkObservationRetentionPhase,
) -> Result<Option<DatabaseDeadline>> {
    if let Some(index) = phase.rollup_tier_index() {
        let (_, source_bucket_secs, destination_bucket_secs) = phase
            .due_span_key()
            .expect("network-observation rollup phase has a durable due-span owner");
        let (_, configured_destination_bucket_secs, retain_days) = NETWORK_OBSERVATION_TIERS[index];
        debug_assert_eq!(destination_bucket_secs, configured_destination_bucket_secs);
        return optional_database_deadline(
            sqlx::query_as(
                r#"
            WITH frontier AS (
                SELECT destination_start
                         + make_interval(
                             secs => ($2 + $3 * 86400)::double precision
                         ) AS database_at
                FROM telemetry_history_due_spans
                WHERE domain = 'network_observation_rollups'
                  AND source_bucket_secs = $1
                  AND destination_bucket_secs = $2
                ORDER BY destination_start, owner_identity
                LIMIT 1
            )
            SELECT database_at,
                   GREATEST(
                       EXTRACT(EPOCH FROM database_at - clock_timestamp()), 0
                   )::DOUBLE PRECISION AS remaining_seconds
            FROM frontier
            "#,
            )
            .bind(source_bucket_secs)
            .bind(destination_bucket_secs)
            .bind(retain_days)
            .fetch_optional(pool)
            .await?,
        );
    }

    match phase {
        NetworkObservationRetentionPhase::TerminalPrune if policy.enabled => {
            optional_database_deadline(
                sqlx::query_as(
                    r#"
                WITH candidates AS (
                    (
                        SELECT observation.observed_at
                                 + make_interval(days => $1)
                                 + interval '1 microsecond' AS next_at
                        FROM network_observations observation
                        WHERE observation.source = 'manual'
                        ORDER BY observation.observed_at
                        LIMIT 1
                    )
                    UNION ALL
                    (
                        SELECT oldest.bucket_start
                                 + make_interval(secs => tier.bucket_secs)
                                 + make_interval(days => $1) AS next_at
                        FROM unnest($2::integer[]) AS tier(bucket_secs)
                        JOIN LATERAL (
                            SELECT rollup.bucket_start
                            FROM network_observation_rollups rollup
                            WHERE rollup.bucket_secs = tier.bucket_secs
                            ORDER BY rollup.bucket_start, rollup.series_id,
                                     rollup.health_state
                            LIMIT 1
                        ) oldest ON TRUE
                    )
                ), frontier AS (
                    SELECT min(next_at) AS database_at
                    FROM candidates
                )
                SELECT database_at,
                       GREATEST(
                           EXTRACT(EPOCH FROM database_at - clock_timestamp()), 0
                       )::DOUBLE PRECISION AS remaining_seconds
                FROM frontier
                WHERE database_at IS NOT NULL
                "#,
                )
                .bind(policy.retention_days)
                .bind(NETWORK_OBSERVATION_BUCKET_SECS.as_slice())
                .fetch_optional(pool)
                .await?,
            )
        }
        NetworkObservationRetentionPhase::TerminalPrune => Ok(None),
        NetworkObservationRetentionPhase::InactiveLatestPrune => optional_database_deadline(
            sqlx::query_as(
                r#"
            WITH frontier AS (
                SELECT latest.observed_at + interval '2 days 1 microsecond'
                         AS database_at
                FROM network_observation_latest latest
                JOIN network_observation_series series ON series.id = latest.series_id
                WHERE series.active = FALSE
                ORDER BY latest.observed_at, latest.series_id
                LIMIT 1
            )
            SELECT database_at,
                   GREATEST(
                       EXTRACT(EPOCH FROM database_at - clock_timestamp()), 0
                   )::DOUBLE PRECISION AS remaining_seconds
            FROM frontier
            "#,
            )
            .fetch_optional(pool)
            .await?,
        ),
        NetworkObservationRetentionPhase::InactiveSeriesPrune => optional_database_deadline(
            sqlx::query_as(
                r#"
            WITH frontier AS (
                SELECT series.last_seen_at + interval '2 days 1 microsecond'
                         AS database_at
                FROM network_observation_series series
                WHERE series.active = FALSE
                  AND NOT EXISTS (
                      SELECT 1 FROM network_observations observation
                      WHERE observation.automatic_series_id = series.id
                  )
                  AND NOT EXISTS (
                      SELECT 1 FROM network_observation_rollups rollup
                      WHERE rollup.series_id = series.id
                  )
                  AND NOT EXISTS (
                      SELECT 1 FROM network_observation_latest latest
                      WHERE latest.series_id = series.id
                  )
                ORDER BY series.last_seen_at, series.id
                LIMIT 1
            )
            SELECT database_at,
                   GREATEST(
                       EXTRACT(EPOCH FROM database_at - clock_timestamp()), 0
                   )::DOUBLE PRECISION AS remaining_seconds
            FROM frontier
            "#,
            )
            .fetch_optional(pool)
            .await?,
        ),
        NetworkObservationRetentionPhase::RollupToDay
        | NetworkObservationRetentionPhase::RollupToSixHours
        | NetworkObservationRetentionPhase::RollupToThreeHours
        | NetworkObservationRetentionPhase::RollupToHour
        | NetworkObservationRetentionPhase::RollupToThirtyMinutes
        | NetworkObservationRetentionPhase::RollupToFiveMinutes => {
            unreachable!("rollup phases return from their exact due-span frontier")
        }
    }
}

fn add_promotion_result(
    run: &mut NetworkObservationRetentionRun,
    _source_bucket_secs: i32,
    _destination_bucket_secs: i32,
    promoted: PromotionResult,
) {
    run.source_rows_promoted = run
        .source_rows_promoted
        .saturating_add(promoted.source_rows);
    run.destination_rows_written = run
        .destination_rows_written
        .saturating_add(promoted.destination_rows);
}

#[derive(Clone, Copy, Debug, Default)]
struct PromotionResult {
    source_rows: u64,
    destination_rows: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct PromotionPhaseOutcome {
    result: PromotionResult,
    attempted: bool,
}

/// The trigger-owned ledger is the only promotion frontier. A phase recheck is
/// therefore independent of retained-history size and never scans observation
/// rows merely to prove that the phase is current.
async fn observation_rollup_promotion_is_due(
    pool: &PgPool,
    source_bucket_secs: i32,
    destination_bucket_secs: i32,
) -> Result<bool> {
    let retain_days = NETWORK_OBSERVATION_TIERS
        .iter()
        .find_map(
            |(configured_source, configured_destination, configured_days)| {
                (*configured_source == source_bucket_secs
                    && *configured_destination == destination_bucket_secs)
                    .then_some(*configured_days)
            },
        )
        .ok_or_else(|| {
            anyhow::anyhow!(
                "network observation due probe does not match the durable tier schedule"
            )
        })?;
    Ok(sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM (
                SELECT 1
                FROM telemetry_history_due_spans due
                WHERE due.domain = 'network_observation_rollups'
                  AND due.source_bucket_secs = $1
                  AND due.destination_bucket_secs = $2
                  AND due.destination_start <= now()
                        - make_interval(
                            secs => ($2 + $3 * 86400)::double precision
                        )
                ORDER BY due.destination_start, due.owner_identity
                LIMIT 1
            ) bounded_due
        )
        "#,
    )
    .bind(source_bucket_secs)
    .bind(destination_bucket_secs)
    .bind(retain_days)
    .fetch_one(pool)
    .await?)
}

#[cfg(test)]
fn observation_rollup_source_bucket_secs(destination_bucket_secs: i32) -> Vec<i32> {
    NETWORK_OBSERVATION_TIERS
        .iter()
        // Every tuple names the physical source tier promoted by that edge.
        // Starting from destination widths would omit the ingest-owned 60s
        // tier, making the first 60s -> 300s edge permanently appear idle.
        .map(|(source_secs, _, _)| *source_secs)
        .filter(|source_bucket_secs| {
            NETWORK_OBSERVATION_TIERS
                .iter()
                .any(|(source, destination, _)| {
                    source == source_bucket_secs && *destination == destination_bucket_secs
                })
        })
        .collect()
}

async fn promote_rollups(
    pool: &PgPool,
    source_bucket_secs: i32,
    destination_bucket_secs: i32,
    retain_days: i32,
    promotion_horizon_days: Option<i32>,
) -> Result<PromotionPhaseOutcome> {
    ensure!(
        destination_bucket_secs >= 300,
        "network observation tier is invalid"
    );
    ensure!(
        NETWORK_OBSERVATION_TIERS.iter().any(
            |(configured_source, configured_destination, configured_days)| {
                *configured_source == source_bucket_secs
                    && *configured_destination == destination_bucket_secs
                    && *configured_days == retain_days
            }
        ),
        "network observation tier does not match the durable due-span schedule"
    );
    let mut tx = pool.begin().await?;
    let Some(owner) = sqlx::query(
        r#"
        SELECT due.destination_start,
               (due.owner_identity[1])::bigint AS series_id
        FROM telemetry_history_due_spans due
        WHERE due.domain = 'network_observation_rollups'
          AND due.source_bucket_secs = $1
          AND due.destination_bucket_secs = $2
          AND due.destination_start <= now()
                - make_interval(
                    secs => ($2 + $3 * 86400)::double precision
                )
        ORDER BY due.destination_start, due.owner_identity
        FOR UPDATE SKIP LOCKED
        LIMIT 1
        "#,
    )
    .bind(source_bucket_secs)
    .bind(destination_bucket_secs)
    .bind(retain_days)
    .fetch_optional(&mut *tx)
    .await?
    else {
        // SKIP LOCKED distinguishes no row from no claim only after releasing
        // this transaction. Preserve StillDue when another owner holds the
        // tiny ledger row; false remains reserved for a genuinely current
        // phase.
        tx.commit().await?;
        return Ok(PromotionPhaseOutcome {
            result: PromotionResult::default(),
            attempted: observation_rollup_promotion_is_due(
                pool,
                source_bucket_secs,
                destination_bucket_secs,
            )
            .await?,
        });
    };
    let destination_start: DateTime<Utc> = owner.try_get("destination_start")?;
    let series_id: i64 = owner.try_get("series_id")?;
    let rows = sqlx::query(ROLLUP_PROMOTION_QUERY)
        .bind(source_bucket_secs)
        .bind(destination_bucket_secs)
        .bind(destination_start)
        .bind(promotion_horizon_days)
        .bind(series_id)
        .fetch_one(&mut *tx)
        .await?;
    let result = promotion_result_from_row(&rows)?;
    tx.commit().await?;
    Ok(PromotionPhaseOutcome {
        result,
        attempted: true,
    })
}

fn promotion_result_from_row(row: &sqlx::postgres::PgRow) -> Result<PromotionResult> {
    Ok(PromotionResult {
        source_rows: u64::try_from(row.try_get::<i64, _>("source_rows")?)?,
        destination_rows: u64::try_from(row.try_get::<i64, _>("destination_rows")?)?,
    })
}

async fn prune_expired_exact_observations(
    pool: &PgPool,
    terminal_retention_days: i32,
    prune_limit: i32,
) -> Result<u64> {
    let result = sqlx::query(
        r#"
        WITH candidates AS (
            SELECT observation.ctid
            FROM network_observations observation
            WHERE observation.source = 'manual'
              AND observation.observed_at
                    < now() - make_interval(days => $1)
            ORDER BY observation.observed_at, observation.id
            FOR UPDATE OF observation SKIP LOCKED
            LIMIT $2
        )
        DELETE FROM network_observations observation
        USING candidates
        WHERE observation.ctid = candidates.ctid
        "#,
    )
    .bind(terminal_retention_days)
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
        WITH tier_frontiers AS MATERIALIZED (
            SELECT
                tier.bucket_secs,
                oldest.bucket_start
                    + make_interval(secs => tier.bucket_secs) AS bucket_end
            FROM unnest($3::integer[]) AS tier(bucket_secs)
            JOIN LATERAL (
                SELECT rollup.bucket_start
                FROM network_observation_rollups rollup
                WHERE rollup.bucket_secs = tier.bucket_secs
                  AND rollup.bucket_start <= now()
                        - make_interval(days => $1)
                        - make_interval(secs => tier.bucket_secs)
                ORDER BY rollup.bucket_start, rollup.series_id,
                         rollup.health_state
                LIMIT 1
            ) oldest ON TRUE
        ), selected_tier AS MATERIALIZED (
            SELECT bucket_secs
            FROM tier_frontiers
            ORDER BY bucket_end, bucket_secs
            LIMIT 1
        ), candidates AS (
            SELECT rollup.ctid
            FROM network_observation_rollups rollup
            JOIN selected_tier tier
              ON tier.bucket_secs = rollup.bucket_secs
            WHERE rollup.bucket_start <= now()
                    - make_interval(days => $1)
                    - make_interval(secs => tier.bucket_secs)
            ORDER BY rollup.bucket_start, rollup.series_id,
                     rollup.health_state
            FOR UPDATE OF rollup SKIP LOCKED
            LIMIT $2
        )
        DELETE FROM network_observation_rollups rollup
        USING candidates
        WHERE rollup.ctid = candidates.ctid
        "#,
    )
    .bind(retention_days)
    .bind(prune_limit)
    .bind(NETWORK_OBSERVATION_BUCKET_SECS.as_slice())
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

const ROLLUP_PROMOTION_QUERY: &str = r#"
WITH boundaries AS MATERIALIZED (
    SELECT
        CASE WHEN $4::integer IS NULL THEN NULL ELSE
            now() - make_interval(days => $4)
        END AS terminal_after
),
eligible_source AS MATERIALIZED (
    -- The durable owner supplies one natural UTC-aligned destination range.
    -- History age and series count cannot widen this scan. Carrying the full
    -- group cardinality on every candidate lets the post-lock pipeline prove
    -- completeness without joining separately materialized group summaries.
    SELECT source.ctid AS source_ctid, source.series_id,
           $3::timestamptz AS destination_start,
           COUNT(*) OVER (
               PARTITION BY source.series_id, $3::timestamptz
           ) AS expected_source_rows
    FROM network_observation_rollups source
    CROSS JOIN boundaries
    WHERE source.bucket_start >= $3::timestamptz
      AND source.bucket_start
            < $3::timestamptz + make_interval(secs => $2)
      AND source.series_id = $5
      AND source.bucket_secs = $1
      AND (boundaries.terminal_after IS NULL
           OR source.bucket_start + make_interval(secs => source.bucket_secs)
                > boundaries.terminal_after)
),
candidate_series AS MATERIALIZED (
    SELECT DISTINCT series_id
    FROM eligible_source
),
locked_series AS MATERIALIZED (
    -- Stable order prevents inversion with series lifecycle work. NO KEY
    -- UPDATE remains compatible with ordinary FK-backed ingestion; SKIP
    -- LOCKED leaves a contended series wholly owned by the other lifecycle.
    SELECT series.id
    FROM network_observation_series series
    JOIN candidate_series candidate ON candidate.series_id = series.id
    ORDER BY series.id
    FOR NO KEY UPDATE OF series SKIP LOCKED
),
locked AS MATERIALIZED (
    SELECT
        source.ctid AS source_ctid,
        source.*,
        candidate.destination_start,
        candidate.expected_source_rows
    FROM eligible_source candidate
    JOIN locked_series series ON series.id = candidate.series_id
    JOIN network_observation_rollups source
      ON source.ctid = candidate.source_ctid
    ORDER BY source.series_id, source.bucket_start,
             source.bucket_secs, source.health_state
    FOR UPDATE OF source SKIP LOCKED
),
locked_state AS (
    SELECT
        locked.*,
        COUNT(*) OVER (
            PARTITION BY locked.series_id, locked.destination_start
        ) AS locked_source_rows
    FROM locked
),
complete_source AS MATERIALIZED (
    -- A skipped series or source row makes every row in that series fail
    -- closed. Acquired rows are a unique subset of eligible rows, so >= is
    -- exactly equality while giving PostgreSQL a range-selectivity estimate
    -- instead of collapsing this row pipeline to one estimated row.
    SELECT locked_state.*
    FROM locked_state
    WHERE locked_source_rows >= expected_source_rows
),
aggregated AS MATERIALIZED (
    SELECT
        series_id,
        destination_start,
        health_state,
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
    FROM complete_source
    GROUP BY series_id, destination_start, health_state
),
eligible AS MATERIALIZED (
    SELECT aggregated.*
    FROM aggregated
),
inserted AS (
    INSERT INTO network_observation_rollups AS current (
        series_id, bucket_secs, bucket_start, health_state,
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
        series_id, $2, destination_start, health_state,
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
    ON CONFLICT (series_id, bucket_secs, bucket_start, health_state)
    DO UPDATE SET
        sample_count = current.sample_count + EXCLUDED.sample_count,
        transmitted_total = current.transmitted_total + EXCLUDED.transmitted_total,
        transmitted_sample_count = current.transmitted_sample_count
            + EXCLUDED.transmitted_sample_count,
        received_total = current.received_total + EXCLUDED.received_total,
        received_sample_count = current.received_sample_count
            + EXCLUDED.received_sample_count,
        latency_sum_ms = current.latency_sum_ms + EXCLUDED.latency_sum_ms,
        latency_sample_count = current.latency_sample_count
            + EXCLUDED.latency_sample_count,
        latency_min_ms = CASE
            WHEN current.latency_min_ms IS NULL THEN EXCLUDED.latency_min_ms
            WHEN EXCLUDED.latency_min_ms IS NULL THEN current.latency_min_ms
            ELSE LEAST(current.latency_min_ms, EXCLUDED.latency_min_ms)
        END,
        latency_max_ms = CASE
            WHEN current.latency_max_ms IS NULL THEN EXCLUDED.latency_max_ms
            WHEN EXCLUDED.latency_max_ms IS NULL THEN current.latency_max_ms
            ELSE GREATEST(current.latency_max_ms, EXCLUDED.latency_max_ms)
        END,
        latency_mdev_sum_ms = current.latency_mdev_sum_ms
            + EXCLUDED.latency_mdev_sum_ms,
        latency_mdev_sample_count = current.latency_mdev_sample_count
            + EXCLUDED.latency_mdev_sample_count,
        packet_loss_sum_ratio = current.packet_loss_sum_ratio
            + EXCLUDED.packet_loss_sum_ratio,
        packet_loss_sample_count = current.packet_loss_sample_count
            + EXCLUDED.packet_loss_sample_count,
        packet_loss_min_ratio = CASE
            WHEN current.packet_loss_min_ratio IS NULL
                THEN EXCLUDED.packet_loss_min_ratio
            WHEN EXCLUDED.packet_loss_min_ratio IS NULL
                THEN current.packet_loss_min_ratio
            ELSE LEAST(
                current.packet_loss_min_ratio,
                EXCLUDED.packet_loss_min_ratio
            )
        END,
        packet_loss_max_ratio = CASE
            WHEN current.packet_loss_max_ratio IS NULL
                THEN EXCLUDED.packet_loss_max_ratio
            WHEN EXCLUDED.packet_loss_max_ratio IS NULL
                THEN current.packet_loss_max_ratio
            ELSE GREATEST(
                current.packet_loss_max_ratio,
                EXCLUDED.packet_loss_max_ratio
            )
        END,
        latest_observation_id = CASE WHEN
            (EXCLUDED.latest_observed_at, EXCLUDED.latest_observation_id)
                > (current.latest_observed_at, current.latest_observation_id)
            THEN EXCLUDED.latest_observation_id ELSE current.latest_observation_id
        END,
        latest_stale_after_secs = CASE WHEN
            (EXCLUDED.latest_observed_at, EXCLUDED.latest_observation_id)
                > (current.latest_observed_at, current.latest_observation_id)
            THEN EXCLUDED.latest_stale_after_secs ELSE current.latest_stale_after_secs
        END,
        latest_healthy = CASE WHEN
            (EXCLUDED.latest_observed_at, EXCLUDED.latest_observation_id)
                > (current.latest_observed_at, current.latest_observation_id)
            THEN EXCLUDED.latest_healthy ELSE current.latest_healthy
        END,
        latest_transmitted = CASE WHEN
            (EXCLUDED.latest_observed_at, EXCLUDED.latest_observation_id)
                > (current.latest_observed_at, current.latest_observation_id)
            THEN EXCLUDED.latest_transmitted ELSE current.latest_transmitted
        END,
        latest_received = CASE WHEN
            (EXCLUDED.latest_observed_at, EXCLUDED.latest_observation_id)
                > (current.latest_observed_at, current.latest_observation_id)
            THEN EXCLUDED.latest_received ELSE current.latest_received
        END,
        latest_latency_min_ms = CASE WHEN
            (EXCLUDED.latest_observed_at, EXCLUDED.latest_observation_id)
                > (current.latest_observed_at, current.latest_observation_id)
            THEN EXCLUDED.latest_latency_min_ms ELSE current.latest_latency_min_ms
        END,
        latest_latency_avg_ms = CASE WHEN
            (EXCLUDED.latest_observed_at, EXCLUDED.latest_observation_id)
                > (current.latest_observed_at, current.latest_observation_id)
            THEN EXCLUDED.latest_latency_avg_ms ELSE current.latest_latency_avg_ms
        END,
        latest_latency_max_ms = CASE WHEN
            (EXCLUDED.latest_observed_at, EXCLUDED.latest_observation_id)
                > (current.latest_observed_at, current.latest_observation_id)
            THEN EXCLUDED.latest_latency_max_ms ELSE current.latest_latency_max_ms
        END,
        latest_latency_mdev_ms = CASE WHEN
            (EXCLUDED.latest_observed_at, EXCLUDED.latest_observation_id)
                > (current.latest_observed_at, current.latest_observation_id)
            THEN EXCLUDED.latest_latency_mdev_ms ELSE current.latest_latency_mdev_ms
        END,
        latest_packet_loss_ratio = CASE WHEN
            (EXCLUDED.latest_observed_at, EXCLUDED.latest_observation_id)
                > (current.latest_observed_at, current.latest_observation_id)
            THEN EXCLUDED.latest_packet_loss_ratio ELSE current.latest_packet_loss_ratio
        END,
        latest_reason = CASE WHEN
            (EXCLUDED.latest_observed_at, EXCLUDED.latest_observation_id)
                > (current.latest_observed_at, current.latest_observation_id)
            THEN EXCLUDED.latest_reason ELSE current.latest_reason
        END,
        latest_observed_at = GREATEST(
            current.latest_observed_at, EXCLUDED.latest_observed_at
        ),
        latest_received_at = CASE WHEN
            (EXCLUDED.latest_observed_at, EXCLUDED.latest_observation_id)
                > (current.latest_observed_at, current.latest_observation_id)
            THEN EXCLUDED.latest_received_at ELSE current.latest_received_at
        END,
        updated_at = clock_timestamp()
    RETURNING series_id, bucket_start, health_state
),
promotion_rows AS MATERIALIZED (
    -- Keep source identities and destination acknowledgements in one tagged
    -- stream. PostgreSQL can retain its row cardinality through the append;
    -- no statistics-free materialized-CTE key join is needed here.
    SELECT
        complete_source.series_id,
        complete_source.destination_start,
        complete_source.source_ctid,
        0::integer AS expected_destination_row,
        0::integer AS inserted_destination_row
    FROM complete_source
    UNION ALL
    SELECT
        eligible.series_id,
        eligible.destination_start,
        NULL::tid AS source_ctid,
        1::integer AS expected_destination_row,
        0::integer AS inserted_destination_row
    FROM eligible
    UNION ALL
    SELECT
        inserted.series_id,
        inserted.bucket_start AS destination_start,
        NULL::tid AS source_ctid,
        0::integer AS expected_destination_row,
        1::integer AS inserted_destination_row
    FROM inserted
),
destination_state AS MATERIALIZED (
    SELECT
        promotion_rows.*,
        SUM(expected_destination_row) OVER (
            PARTITION BY series_id, destination_start
        ) AS expected_destination_rows,
        SUM(inserted_destination_row) OVER (
            PARTITION BY series_id, destination_start
        ) AS inserted_destination_rows
    FROM promotion_rows
),
deletable_source AS (
    SELECT source_ctid
    FROM destination_state
    WHERE source_ctid IS NOT NULL
      -- Insert acknowledgements are a unique subset of expected rows; >= is
      -- therefore exact equality without a one-row equality estimate.
      AND inserted_destination_rows >= expected_destination_rows
),
deleted AS (
    DELETE FROM network_observation_rollups source
    USING deletable_source
    WHERE source.ctid = deletable_source.source_ctid
    RETURNING source.ctid AS source_ctid
),
span_coverage AS MATERIALIZED (
    SELECT
        (SELECT COUNT(*) FROM eligible_source) AS expected_source_rows,
        (SELECT COUNT(*) FROM deleted) AS deleted_source_rows
),
cleared_due_span AS (
    DELETE FROM telemetry_history_due_spans due
    USING span_coverage
    WHERE due.domain = 'network_observation_rollups'
      AND due.source_bucket_secs = $1
      AND due.destination_bucket_secs = $2
      AND due.destination_start = $3::timestamptz
      AND due.owner_identity = ARRAY[$5::bigint::text]
      AND span_coverage.deleted_source_rows
            = span_coverage.expected_source_rows
    RETURNING due.destination_start
)
SELECT
    (SELECT COUNT(*)::bigint FROM deleted) AS source_rows,
    (SELECT COUNT(*)::bigint FROM inserted) AS destination_rows,
    (SELECT COUNT(*)::bigint FROM cleared_due_span) AS due_spans_cleared
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_retention::coalesce_ready_telemetry_due_events;
    use crate::test_support::PgWorkerTestDb;
    use chrono::{Duration, Timelike};
    use uuid::Uuid;

    #[test]
    fn observation_tiers_share_the_common_telemetry_boundaries() {
        assert_eq!(
            NETWORK_OBSERVATION_TIERS,
            &[
                (60, 300, 2),
                (300, 1_800, 8),
                (1_800, 3_600, 31),
                (3_600, 10_800, 91),
                (10_800, 21_600, 181),
                (21_600, 86_400, 366),
            ]
        );
        assert_eq!(
            NetworkObservationRetentionPhase::RollupToFiveMinutes.due_span_key(),
            Some(("network_observation_rollups", 60, 300))
        );
        assert_eq!(observation_rollup_source_bucket_secs(300), vec![60]);
        assert_eq!(observation_rollup_source_bucket_secs(86_400), vec![21_600]);
    }

    #[tokio::test]
    async fn network_writer_triggers_publish_only_their_exact_committed_effects() {
        async fn next_effect(listener: &mut sqlx::postgres::PgListener) -> String {
            let notification =
                tokio::time::timeout(std::time::Duration::from_secs(2), listener.recv())
                    .await
                    .expect("network retention notification timeout")
                    .unwrap();
            let payload: serde_json::Value = serde_json::from_str(notification.payload()).unwrap();
            assert_eq!(payload["owner"], "history_retention");
            payload["effect"].as_str().unwrap().to_string()
        }

        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let (plan_id, series_id) = seed_test_series(&db).await;
        let automatic_id = insert_exact_observation(
            &db,
            plan_id,
            Some(series_id),
            "automatic",
            Utc::now() - Duration::hours(1),
        )
        .await;

        let mut listener = db.notification_listener().await.unwrap();
        listener.listen("vpsman_telemetry_retention").await.unwrap();
        sqlx::query("UPDATE network_observations SET received_at = received_at WHERE id = $1")
            .bind(automatic_id)
            .execute(&db.pool)
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), listener.recv(),)
                .await
                .is_err(),
            "automatic raw publication must wait for the core-minute frontier"
        );

        let _manual_id = insert_exact_observation(
            &db,
            plan_id,
            None,
            "manual",
            Utc::now() - Duration::hours(1),
        )
        .await;
        assert_eq!(
            next_effect(&mut listener).await,
            "network_observation_history_published"
        );

        sqlx::query("DELETE FROM network_observations WHERE id = $1")
            .bind(automatic_id)
            .execute(&db.pool)
            .await
            .unwrap();
        assert_eq!(
            next_effect(&mut listener).await,
            "network_observation_history_deleted"
        );

        sqlx::query("UPDATE network_observation_series SET active = FALSE WHERE id = $1")
            .bind(series_id)
            .execute(&db.pool)
            .await
            .unwrap();
        assert_eq!(
            next_effect(&mut listener).await,
            "network_observation_series_deactivated"
        );

        let latest_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO network_observation_latest (
                series_id, observation_id, stale_after_secs, healthy,
                transmitted, received, packet_loss_ratio, observed_at, received_at
            ) VALUES ($1, $2, 180, TRUE, 1, 1, 0.0, now(), now())
            "#,
        )
        .bind(series_id)
        .bind(latest_id)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query("DELETE FROM network_observation_latest WHERE series_id = $1")
            .bind(series_id)
            .execute(&db.pool)
            .await
            .unwrap();
        assert_eq!(
            next_effect(&mut listener).await,
            "network_observation_latest_deleted"
        );

        let rollup_start: DateTime<Utc> =
            sqlx::query_scalar("SELECT date_trunc('minute', now() - interval '1 day')")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        insert_rollup_fragment(&db, series_id, 60, rollup_start, 1).await;
        assert_eq!(
            next_effect(&mut listener).await,
            "ordinary_rollup_published"
        );
        sqlx::query(
            "DELETE FROM network_observation_rollups WHERE series_id = $1 AND bucket_secs = 60 AND bucket_start = $2",
        )
        .bind(series_id)
        .bind(rollup_start)
        .execute(&db.pool)
        .await
        .unwrap();
        assert_eq!(
            next_effect(&mut listener).await,
            "network_observation_history_deleted"
        );
        db.cleanup().await;
    }

    #[tokio::test]
    async fn terminal_rollup_frontier_uses_exact_bucket_end_across_tiers() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let (_, series_id) = seed_test_series(&db).await;
        let day_start: DateTime<Utc> =
            sqlx::query_scalar("SELECT date_trunc('day', now() - interval '2 days')")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        let five_minute_start = day_start + Duration::minutes(5);
        insert_rollup_fragment(&db, series_id, 86_400, day_start, 1).await;
        insert_rollup_fragment(&db, series_id, 300, five_minute_start, 1).await;

        let next_at = network_observation_retention_phase_next_at(
            &db.pool,
            NetworkObservationRetentionPolicy {
                enabled: true,
                retention_days: 0,
                prune_limit: 1,
            },
            NetworkObservationRetentionPhase::TerminalPrune,
        )
        .await
        .unwrap();
        assert_eq!(
            next_at.map(|deadline| deadline.database_at),
            Some(five_minute_start + Duration::minutes(5))
        );

        assert_eq!(prune_expired_rollups(&db.pool, 0, 1).await.unwrap(), 1);
        assert_eq!(rollup_row_count(&db, series_id, 300).await, 0);
        assert_eq!(rollup_row_count(&db, series_id, 86_400).await, 1);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn manual_terminal_frontier_excludes_automatic_locator_scale_by_index() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let (_, series_id) = seed_test_series(&db).await;
        let (client_id, plan_name): (String, String) = sqlx::query_as(
            r#"
            SELECT series.client_id, plan.name
            FROM network_observation_series series
            JOIN tunnel_plans plan ON plan.id = series.plan_id
            WHERE series.id = $1
            "#,
        )
        .bind(series_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO telemetry_samples (
                id, client_id, observed_at, cpu_cores,
                cpu_load_1, cpu_load_5, cpu_load_15,
                memory_total_bytes, memory_available_bytes,
                disk_total_bytes, disk_available_bytes,
                tcp_sockets, udp_sockets, payload,
                accepted_seq, accepted_at, source_gateway_id,
                source_gateway_session_id, source_process_incarnation_id,
                source_telemetry_seq, reported_observed_unix
            )
            SELECT md5('manual-index-sample-' || sample_number)::uuid,
                   $1, now() - interval '2 days', 0,
                   0.0, 0.0, 0.0, 0, 0, 0, 0, 0, 0, '{}'::jsonb,
                   sample_number, clock_timestamp(),
                   'network-retention-test-fixture',
                   '20000000-0000-4000-8000-000000000001'::uuid,
                   '20000000-0000-4000-8000-000000000002'::uuid,
                   sample_number,
                   floor(extract(epoch FROM now() - interval '2 days'))::bigint
            FROM generate_series(1, 8) AS sample_number
            "#,
        )
        .bind(&client_id)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            UPDATE telemetry_projection_heads
            SET accepted_seq=8, projected_seq=8,
                latest_projected_sample_id=
                    md5('manual-index-sample-8')::uuid,
                accepted_at=clock_timestamp(), projected_at=clock_timestamp()
            WHERE client_id=$1
            "#,
        )
        .bind(&client_id)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO network_observations (
                id, source, automatic_series_id, automatic_sample_id,
                automatic_payload_ordinal, plan_name, observed_at, received_at
            )
            SELECT md5('manual-index-observation-' || observation_number)::uuid,
                   'automatic', $1,
                   md5(
                       'manual-index-sample-'
                       || (((observation_number - 1) / 512) + 1)
                   )::uuid,
                   (((observation_number - 1) % 512) + 1)::smallint,
                   $2,
                   now() - interval '2 days'
                       - observation_number * interval '1 second',
                   now()
            FROM generate_series(1, 4096) AS observation_number
            "#,
        )
        .bind(series_id)
        .bind(plan_name)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query("ANALYZE network_observations")
            .execute(&db.pool)
            .await
            .unwrap();

        let plan: serde_json::Value = sqlx::query_scalar(
            r#"
            EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)
            SELECT 1
            FROM network_observations observation
            WHERE observation.source = 'manual'
              AND observation.observed_at < now() - interval '1 day'
            ORDER BY observation.observed_at, observation.id
            LIMIT 1
            "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert!(explain_uses_index(
            &plan,
            "network_observations_manual_retention_idx"
        ));
        assert!(!explain_uses_sequential_scan(&plan, "network_observations"));

        db.cleanup().await;
    }

    #[tokio::test]
    async fn exact_rollup_edge_frontiers_use_the_due_span_primary_key() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        sqlx::query(
            r#"
            WITH edge AS (
                SELECT date_bin(
                    interval '30 minutes', now() - interval '10 days',
                    TIMESTAMPTZ '1970-01-01 00:00:00+00'
                ) AS destination_start
            )
            INSERT INTO telemetry_history_due_spans (
                domain, source_bucket_secs, destination_bucket_secs,
                owner_identity, destination_start, due_at
            )
            SELECT 'network_observation_rollups', 300, 1800,
                   ARRAY[owner_number::text], edge.destination_start,
                   edge.destination_start
                       + make_interval(secs => 1800 + 8 * 86400)
            FROM edge
            CROSS JOIN generate_series(1, 10000) AS owner_number
            "#,
        )
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query("ANALYZE telemetry_history_due_spans")
            .execute(&db.pool)
            .await
            .unwrap();

        let absent_plan: serde_json::Value = sqlx::query_scalar(
            r#"
            EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)
            SELECT EXISTS (
                SELECT 1 FROM (
                    SELECT 1
                    FROM telemetry_history_due_spans due
                    WHERE due.domain = 'network_observation_rollups'
                      AND due.source_bucket_secs = 60
                      AND due.destination_bucket_secs = 300
                      AND due.destination_start <= now()
                            - make_interval(secs => 300 + 2 * 86400)
                    ORDER BY due.destination_start, due.owner_identity
                    LIMIT 1
                ) bounded_due
            )
            "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert!(explain_uses_index(
            &absent_plan,
            "telemetry_history_due_spans_pkey"
        ));
        assert!(!explain_uses_sequential_scan(
            &absent_plan,
            "telemetry_history_due_spans"
        ));

        sqlx::query(
            r#"
            WITH edge AS (
                SELECT date_bin(
                    interval '5 minutes', now() - interval '3 days',
                    TIMESTAMPTZ '1970-01-01 00:00:00+00'
                ) AS destination_start
            )
            INSERT INTO telemetry_history_due_spans (
                domain, source_bucket_secs, destination_bucket_secs,
                owner_identity, destination_start, due_at
            )
            SELECT 'network_observation_rollups', 60, 300,
                   ARRAY['10001'], edge.destination_start,
                   edge.destination_start
                       + make_interval(secs => 300 + 2 * 86400)
            FROM edge
            "#,
        )
        .execute(&db.pool)
        .await
        .unwrap();
        let claim_plan: serde_json::Value = sqlx::query_scalar(
            r#"
            EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)
            SELECT due.destination_start, due.owner_identity
            FROM telemetry_history_due_spans due
            WHERE due.domain = 'network_observation_rollups'
              AND due.source_bucket_secs = 60
              AND due.destination_bucket_secs = 300
              AND due.destination_start <= now()
                    - make_interval(secs => 300 + 2 * 86400)
            ORDER BY due.destination_start, due.owner_identity
            FOR UPDATE SKIP LOCKED
            LIMIT 1
            "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert!(explain_uses_index(
            &claim_plan,
            "telemetry_history_due_spans_pkey"
        ));
        assert!(!explain_uses_sequential_scan(
            &claim_plan,
            "telemetry_history_due_spans"
        ));
        db.cleanup().await;
    }

    #[tokio::test]
    async fn observation_fragment_publishes_only_its_adjacent_complete_edge() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let (_, series_id) = seed_test_series(&db).await;
        let source_start: DateTime<Utc> = sqlx::query_scalar(
            r#"
            SELECT date_trunc('minute', now()) + interval '1 minute'
            "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        insert_rollup_fragment(&db, series_id, 60, source_start, 1).await;

        let event_shape: (i64, i64, i64, DateTime<Utc>, DateTime<Utc>) = sqlx::query_as(
            r#"
            SELECT count(*), count(DISTINCT destination_bucket_secs),
                   count(DISTINCT coalesce_ready_at),
                   min(coalesce_ready_at), max(coalesce_ready_at)
            FROM telemetry_history_due_events
            WHERE domain = 'network_observation_rollups'
            "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(event_shape.0, 1);
        assert_eq!(event_shape.1, 1);
        assert_eq!(event_shape.2, 1);
        let destination_start =
            source_start - Duration::minutes(i64::from(source_start.minute() % 5));
        assert_eq!(event_shape.3, destination_start + Duration::minutes(5));
        assert_eq!(event_shape.4, event_shape.3);

        let coalescing = coalesce_ready_telemetry_due_events(&db.pool).await.unwrap();
        assert_eq!(coalescing.coalesced, 0);
        assert!(
            !coalescing.has_remaining_work,
            "open observation evidence is not runnable work"
        );
        let owners: (i64, i64) = sqlx::query_as(
            r#"
            SELECT
                (SELECT count(*) FROM telemetry_history_due_events),
                (SELECT count(*) FROM telemetry_history_due_spans)
            "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(owners, (1, 0));

        // A direct import of an already-rolled source tier is still a producer:
        // it must reconstruct every later tier rather than depending on an
        // earlier raw row that the import may not contain.
        sqlx::query("DELETE FROM telemetry_history_due_events")
            .execute(&db.pool)
            .await
            .unwrap();
        let imported_start = source_start - Duration::minutes(i64::from(source_start.minute() % 5))
            + Duration::minutes(5);
        insert_rollup_fragment(&db, series_id, 300, imported_start, 1).await;
        let imported_shape: (i64, i64, i32) = sqlx::query_as(
            r#"
            SELECT count(*), count(DISTINCT destination_bucket_secs),
                   min(destination_bucket_secs)
            FROM telemetry_history_due_events
            WHERE domain = 'network_observation_rollups'
            "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(imported_shape, (1, 1, 1_800));
        db.cleanup().await;
    }

    #[tokio::test]
    async fn promotion_boundaries_assign_complete_groups_once_and_ignore_the_open_head() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };

        // Exercise the real 3h -> 6h edge across former age bands. Every
        // closed source must traverse this edge; none may be stranded merely
        // because a one-off reimport is already old.
        let (_, band_series_id) = seed_test_series(&db).await;
        let (older_boundary, newer_boundary): (DateTime<Utc>, DateTime<Utc>) = sqlx::query_as(
            r#"
                SELECT
                    to_timestamp(floor(extract(epoch FROM (
                        now() - interval '366 days')) / 86400) * 86400),
                    to_timestamp(floor(extract(epoch FROM (
                        now() - interval '181 days')) / 21600) * 21600)
                "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let before_older = older_boundary - Duration::hours(6);
        let at_older = older_boundary;
        let ending_at_newer = newer_boundary - Duration::hours(6);
        // Keep the incomplete group one full destination ahead of the captured
        // boundary so a clock tick at the 6h edge cannot make it eligible.
        let open_head = newer_boundary + Duration::hours(6);
        for bucket_start in [before_older, at_older, ending_at_newer, open_head] {
            insert_rollup_fragment(&db, band_series_id, 10_800, bucket_start, 1).await;
        }
        coalesce_ready_due_events(&db).await;

        assert!(
            observation_rollup_promotion_is_due(&db.pool, 10_800, 21_600)
                .await
                .unwrap()
        );
        let stale_band = promote_rollups(&db.pool, 10_800, 21_600, 181, None)
            .await
            .unwrap();
        assert!(stale_band.attempted);
        assert_eq!(stale_band.result.source_rows, 1);
        assert_eq!(stale_band.result.destination_rows, 1);
        let first_band = promote_rollups(&db.pool, 10_800, 21_600, 181, None)
            .await
            .unwrap();
        assert_eq!(first_band.result.source_rows, 1);
        assert!(first_band.attempted);
        assert_eq!(first_band.result.destination_rows, 1);
        assert!(
            observation_rollup_promotion_is_due(&db.pool, 10_800, 21_600)
                .await
                .unwrap()
        );
        let second_band = promote_rollups(&db.pool, 10_800, 21_600, 181, None)
            .await
            .unwrap();
        assert_eq!(second_band.result.source_rows, 1);
        assert!(second_band.attempted);
        assert_eq!(second_band.result.destination_rows, 1);
        let remaining_sources = sqlx::query_scalar::<_, DateTime<Utc>>(
            r#"
            SELECT bucket_start
            FROM network_observation_rollups
            WHERE series_id = $1 AND bucket_secs = 10800
            ORDER BY bucket_start
            "#,
        )
        .bind(band_series_id)
        .fetch_all(&db.pool)
        .await
        .unwrap();
        assert_eq!(remaining_sources, vec![open_head]);
        let owned_destinations = sqlx::query_scalar::<_, DateTime<Utc>>(
            r#"
            SELECT bucket_start
            FROM network_observation_rollups
            WHERE series_id = $1 AND bucket_secs = 21600
            ORDER BY bucket_start
            "#,
        )
        .bind(band_series_id)
        .fetch_all(&db.pool)
        .await
        .unwrap();
        assert_eq!(
            owned_destinations,
            vec![before_older, at_older, ending_at_newer]
        );
        assert!(
            !observation_rollup_promotion_is_due(&db.pool, 10_800, 21_600)
                .await
                .unwrap()
        );
        sqlx::query("DELETE FROM network_observation_series WHERE id = $1")
            .bind(band_series_id)
            .execute(&db.pool)
            .await
            .unwrap();

        // The final day edge has no O boundary. Bracket its rolling H boundary
        // with wide margins: the expired fragment remains outside promotion,
        // while the overlapping retained fragment is promoted exactly once.
        let (_, horizon_series_id) = seed_test_series(&db).await;
        let terminal_after: DateTime<Utc> =
            sqlx::query_scalar("SELECT now() - interval '3650 days'")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        let terminal_six_hour =
            DateTime::from_timestamp(terminal_after.timestamp().div_euclid(21_600) * 21_600, 0)
                .unwrap();
        let expired = terminal_six_hour - Duration::hours(6);
        let retained = terminal_six_hour + Duration::hours(6);
        assert!(expired + Duration::hours(6) <= terminal_after);
        assert!(retained > terminal_after);
        insert_rollup_fragment(&db, horizon_series_id, 21_600, expired, 1).await;
        insert_rollup_fragment(&db, horizon_series_id, 21_600, retained, 1).await;
        coalesce_ready_due_events(&db).await;

        assert!(
            observation_rollup_promotion_is_due(&db.pool, 21_600, 86_400)
                .await
                .unwrap()
        );
        // The earlier boundary case can leave source-empty daily coordinates
        // after its series is deleted. They remain valid ledger work, so drain
        // the authority to Current instead of assuming this fixture owns two
        // and only two coordinates.
        let horizon = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let mut horizon = PromotionResult::default();
            while observation_rollup_promotion_is_due(&db.pool, 21_600, 86_400)
                .await
                .unwrap()
            {
                let page = promote_rollups(&db.pool, 21_600, 86_400, 366, Some(3_650))
                    .await
                    .unwrap();
                assert!(page.attempted);
                horizon.source_rows += page.result.source_rows;
                horizon.destination_rows += page.result.destination_rows;
            }
            horizon
        })
        .await
        .expect("daily observation promotion did not converge");
        assert_eq!(horizon.source_rows, 1);
        assert_eq!(horizon.destination_rows, 1);
        let remaining_horizon_sources = sqlx::query_scalar::<_, DateTime<Utc>>(
            r#"
            SELECT bucket_start
            FROM network_observation_rollups
            WHERE series_id = $1 AND bucket_secs = 21600
            ORDER BY bucket_start
            "#,
        )
        .bind(horizon_series_id)
        .fetch_all(&db.pool)
        .await
        .unwrap();
        assert_eq!(remaining_horizon_sources, vec![expired]);
        let retained_day =
            DateTime::from_timestamp(retained.timestamp().div_euclid(86_400) * 86_400, 0).unwrap();
        assert_eq!(
            rollup_sample_count(&db, horizon_series_id, 86_400, retained_day).await,
            1
        );
        assert!(
            !observation_rollup_promotion_is_due(&db.pool, 21_600, 86_400)
                .await
                .unwrap()
        );

        db.cleanup().await;
    }

    #[tokio::test]
    async fn late_disjoint_minute_fragments_merge_once_into_an_existing_tier() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let (_, series_id) = seed_test_series(&db).await;
        let anchor = (Utc::now() - Duration::days(3))
            .with_second(0)
            .unwrap()
            .with_nanosecond(0)
            .unwrap();
        let anchor = anchor - Duration::minutes(i64::from(anchor.minute() % 5));

        insert_rollup_fragment(&db, series_id, 60, anchor, 1).await;
        insert_rollup_fragment(&db, series_id, 60, anchor + Duration::minutes(1), 1).await;
        coalesce_ready_due_events(&db).await;
        let first = promote_rollups(&db.pool, 60, 300, 2, Some(3_650))
            .await
            .unwrap();
        assert_eq!(first.result.source_rows, 2);
        assert!(first.attempted);
        assert_eq!(first.result.destination_rows, 1);
        assert_eq!(rollup_sample_count(&db, series_id, 300, anchor).await, 2);
        assert_eq!(rollup_row_count(&db, series_id, 60).await, 0);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM telemetry_history_due_events WHERE domain = 'network_observation_rollups'",
            )
            .fetch_one(&db.pool)
            .await
            .unwrap(),
            1,
            "an adjacent destination write must publish exactly its successor edge",
        );

        // This fragment arrived after the original minute fragments moved to
        // 5m. Its exact-row insert would be the idempotency boundary in the API,
        // so the fragment is new evidence even though its wall-time overlaps.
        insert_rollup_fragment(&db, series_id, 60, anchor + Duration::minutes(2), 1).await;
        coalesce_ready_due_events(&db).await;
        let late = promote_rollups(&db.pool, 60, 300, 2, Some(3_650))
            .await
            .unwrap();
        assert_eq!(late.result.source_rows, 1);
        assert!(late.attempted);
        assert_eq!(late.result.destination_rows, 1);
        assert_eq!(rollup_sample_count(&db, series_id, 300, anchor).await, 3);
        assert_eq!(rollup_row_count(&db, series_id, 60).await, 0);

        let idle = promote_rollups(&db.pool, 60, 300, 2, Some(3_650))
            .await
            .unwrap();
        assert_eq!(idle.result.source_rows, 0);
        assert!(!idle.attempted);
        assert_eq!(rollup_sample_count(&db, series_id, 300, anchor).await, 3);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn locked_fragment_keeps_the_group_and_due_span_until_complete() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let (_, series_id) = seed_test_series(&db).await;
        let anchor = (Utc::now() - Duration::days(3))
            .with_second(0)
            .unwrap()
            .with_nanosecond(0)
            .unwrap();
        let anchor = anchor - Duration::minutes(i64::from(anchor.minute() % 5));
        insert_rollup_fragment(&db, series_id, 60, anchor, 1).await;
        insert_rollup_fragment(&db, series_id, 60, anchor + Duration::minutes(1), 1).await;
        coalesce_ready_due_events(&db).await;

        let mut owner_blocker = db.pool.begin().await.unwrap();
        sqlx::query(
            r#"
            SELECT destination_start
            FROM telemetry_history_due_spans
            WHERE domain = 'network_observation_rollups'
              AND source_bucket_secs = 60
              AND destination_bucket_secs = 300
              AND destination_start = $1
            FOR UPDATE
            "#,
        )
        .bind(anchor)
        .fetch_one(&mut *owner_blocker)
        .await
        .unwrap();
        let contended_owner = promote_rollups(&db.pool, 60, 300, 2, Some(3_650))
            .await
            .unwrap();
        assert!(contended_owner.attempted);
        assert_eq!(contended_owner.result.source_rows, 0);
        assert_eq!(contended_owner.result.destination_rows, 0);
        owner_blocker.rollback().await.unwrap();

        let mut blocker = db.pool.begin().await.unwrap();
        sqlx::query(
            r#"
            SELECT 1
            FROM network_observation_rollups
            WHERE series_id = $1
              AND bucket_secs = 60
              AND bucket_start = $2
            FOR UPDATE
            "#,
        )
        .bind(series_id)
        .bind(anchor + Duration::minutes(1))
        .fetch_one(&mut *blocker)
        .await
        .unwrap();

        let skipped = promote_rollups(&db.pool, 60, 300, 2, Some(3_650))
            .await
            .unwrap();
        assert!(skipped.attempted);
        assert_eq!(skipped.result.source_rows, 0);
        assert_eq!(skipped.result.destination_rows, 0);
        assert_eq!(rollup_row_count(&db, series_id, 60).await, 2);
        assert_eq!(rollup_row_count(&db, series_id, 300).await, 0);
        assert!(observation_rollup_promotion_is_due(&db.pool, 60, 300)
            .await
            .unwrap());

        blocker.rollback().await.unwrap();
        let completed = promote_rollups(&db.pool, 60, 300, 2, Some(3_650))
            .await
            .unwrap();
        assert_eq!(completed.result.source_rows, 2);
        assert_eq!(completed.result.destination_rows, 1);
        assert_eq!(rollup_row_count(&db, series_id, 60).await, 0);
        assert_eq!(rollup_sample_count(&db, series_id, 300, anchor).await, 2);
        assert!(!observation_rollup_promotion_is_due(&db.pool, 60, 300)
            .await
            .unwrap());
        db.cleanup().await;
    }

    #[tokio::test]
    async fn automatic_locators_follow_the_raw_sample_owner_while_manual_is_terminal_owned() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let (plan_id, series_id) = seed_test_series(&db).await;
        let automatic_id = insert_exact_observation(
            &db,
            plan_id,
            Some(series_id),
            "automatic",
            Utc::now() - Duration::days(9),
        )
        .await;
        let retained_manual_id =
            insert_exact_observation(&db, plan_id, None, "manual", Utc::now() - Duration::days(9))
                .await;
        let expired_manual_id = insert_exact_observation(
            &db,
            plan_id,
            None,
            "manual",
            Utc::now() - Duration::days(3_700),
        )
        .await;

        let pruned = prune_expired_exact_observations(&db.pool, 3_650, 100)
            .await
            .unwrap();
        assert_eq!(pruned, 1);
        assert!(observation_exists(&db, automatic_id).await);
        assert!(observation_exists(&db, retained_manual_id).await);
        assert!(!observation_exists(&db, expired_manual_id).await);

        let sample_id: Uuid = sqlx::query_scalar(
            "SELECT automatic_sample_id FROM network_observations WHERE id = $1",
        )
        .bind(automatic_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let minute_start = (Utc::now() - Duration::days(9))
            .with_second(0)
            .unwrap()
            .with_nanosecond(0)
            .unwrap();
        insert_rollup_fragment(&db, series_id, 60, minute_start, 1).await;
        sqlx::query("DELETE FROM telemetry_samples WHERE id = $1")
            .bind(sample_id)
            .execute(&db.pool)
            .await
            .unwrap();
        assert!(!observation_exists(&db, automatic_id).await);
        assert_eq!(rollup_row_count(&db, series_id, 60).await, 1);
        db.cleanup().await;
    }

    fn explain_uses_index(plan: &serde_json::Value, index_name: &str) -> bool {
        match plan {
            serde_json::Value::Object(fields) => {
                fields.get("Index Name").and_then(serde_json::Value::as_str) == Some(index_name)
                    || fields
                        .values()
                        .any(|value| explain_uses_index(value, index_name))
            }
            serde_json::Value::Array(values) => values
                .iter()
                .any(|value| explain_uses_index(value, index_name)),
            _ => false,
        }
    }

    fn explain_uses_sequential_scan(plan: &serde_json::Value, relation_name: &str) -> bool {
        match plan {
            serde_json::Value::Object(fields) => {
                let owns_scan = fields
                    .get("Relation Name")
                    .and_then(serde_json::Value::as_str)
                    == Some(relation_name)
                    && fields
                        .get("Node Type")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|node_type| node_type.contains("Seq Scan"));
                owns_scan
                    || fields
                        .values()
                        .any(|value| explain_uses_sequential_scan(value, relation_name))
            }
            serde_json::Value::Array(values) => values
                .iter()
                .any(|value| explain_uses_sequential_scan(value, relation_name)),
            _ => false,
        }
    }

    async fn seed_test_series(db: &PgWorkerTestDb) -> (Uuid, i64) {
        let left = format!("observation-left-{}", Uuid::new_v4());
        let right = format!("observation-right-{}", Uuid::new_v4());
        for client_id in [&left, &right] {
            sqlx::query(
                "INSERT INTO clients (id, display_name, public_key, status) \
                 VALUES ($1, $1, decode('', 'hex'), 'online')",
            )
            .bind(client_id)
            .execute(&db.pool)
            .await
            .unwrap();
        }
        let plan_id = Uuid::new_v4();
        let plan_name = format!("retention-{plan_id}");
        sqlx::query(
            r#"
            INSERT INTO tunnel_plans (
                id, name, kind, left_client_id, right_client_id, input, plan
            ) VALUES ($1, $4, 'wireguard', $2, $3,
                      '{}'::jsonb, '{}'::jsonb)
            "#,
        )
        .bind(plan_id)
        .bind(&left)
        .bind(&right)
        .bind(&plan_name)
        .execute(&db.pool)
        .await
        .unwrap();
        let series_id = sqlx::query_scalar(
            r#"
            INSERT INTO network_observation_series (
                plan_id, topology_identity_hash, plan_name, interface_name,
                client_id, peer_client_id, endpoint_side, address_family, target
            ) VALUES ($1, $2, $5, 'tun-test', $3, $4,
                      'left', 'ipv4', '192.0.2.1')
            RETURNING id
            "#,
        )
        .bind(plan_id)
        .bind("0".repeat(64))
        .bind(&left)
        .bind(&right)
        .bind(&plan_name)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        (plan_id, series_id)
    }

    async fn coalesce_ready_due_events(db: &PgWorkerTestDb) {
        loop {
            let coalescing = coalesce_ready_telemetry_due_events(&db.pool).await.unwrap();
            if !coalescing.has_remaining_work {
                return;
            }
            assert!(
                coalescing.coalesced > 0,
                "ready due-event drain made no progress"
            );
        }
    }

    async fn insert_rollup_fragment(
        db: &PgWorkerTestDb,
        series_id: i64,
        bucket_secs: i32,
        bucket_start: DateTime<Utc>,
        sample_count: i64,
    ) {
        sqlx::query(
            r#"
            INSERT INTO network_observation_rollups (
                series_id, bucket_secs, bucket_start, health_state,
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
            ) VALUES (
                $1, $2, $3, 1,
                $4, ($4 * 10)::numeric, $4, ($4 * 9)::numeric, $4,
                $4::double precision * 5.0, $4, 5.0, 5.0,
                $4::double precision, $4,
                $4::double precision * 0.1, $4, 0.1, 0.1,
                $5, 180, TRUE, 10, 9, 4.0, 5.0, 6.0, 1.0, 0.1, NULL,
                $3 + interval '30 seconds', $3 + interval '31 seconds'
            )
            "#,
        )
        .bind(series_id)
        .bind(bucket_secs)
        .bind(bucket_start)
        .bind(sample_count)
        .bind(Uuid::new_v4())
        .execute(&db.pool)
        .await
        .unwrap();
    }

    async fn insert_exact_observation(
        db: &PgWorkerTestDb,
        plan_id: Uuid,
        series_id: Option<i64>,
        source: &str,
        observed_at: DateTime<Utc>,
    ) -> Uuid {
        let id = Uuid::new_v4();
        let (client_id, peer_client_id, plan_name): (String, String, String) = sqlx::query_as(
            "SELECT left_client_id, right_client_id, name FROM tunnel_plans WHERE id=$1",
        )
        .bind(plan_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        if source == "automatic" {
            let series_id = series_id.expect("automatic fixture requires a series");
            let sample_id = Uuid::new_v4();
            sqlx::query(
                r#"
                INSERT INTO telemetry_samples (
                    id, client_id, observed_at, cpu_cores,
                    cpu_load_1, cpu_load_5, cpu_load_15,
                    memory_total_bytes, memory_available_bytes,
                    disk_total_bytes, disk_available_bytes,
                    tcp_sockets, udp_sockets, payload,
                    accepted_seq, accepted_at, source_gateway_id,
                    source_gateway_session_id, source_process_incarnation_id,
                    source_telemetry_seq, reported_observed_unix
                ) VALUES (
                    $1, $2, $3, 0, 0.0, 0.0, 0.0,
                    0, 0, 0, 0, 0, 0, $4,
                    1, now(), 'retention-test', $5, $6, 1, $7
                )
                "#,
            )
            .bind(sample_id)
            .bind(&client_id)
            .bind(observed_at)
            .bind(serde_json::json!({
                "tunnel_reachability": [{
                    "id": id,
                    "stale_after_secs": 180,
                    "healthy": true,
                    "transmitted": 10,
                    "received": 10,
                    "latency_min_ms": null,
                    "latency_avg_ms": null,
                    "latency_max_ms": null,
                    "latency_mdev_ms": null,
                    "packet_loss_ratio": 0.0,
                    "reason": null,
                }]
            }))
            .bind(Uuid::new_v4())
            .bind(Uuid::new_v4())
            .bind(observed_at.timestamp())
            .execute(&db.pool)
            .await
            .unwrap();
            sqlx::query(
                r#"
                UPDATE telemetry_projection_heads
                SET accepted_seq = 1, projected_seq = 1,
                    accepted_at = now(), projected_at = now()
                WHERE client_id = $1
                "#,
            )
            .bind(&client_id)
            .execute(&db.pool)
            .await
            .unwrap();
            sqlx::query(
                r#"
                UPDATE telemetry_minute_materialization_heads
                SET materialized_seq = 1, materialized_at = now(),
                    updated_at = now()
                WHERE client_id = $1
                "#,
            )
            .bind(&client_id)
            .execute(&db.pool)
            .await
            .unwrap();
            sqlx::query(
                r#"
                INSERT INTO network_observations (
                    id, source, automatic_series_id, automatic_sample_id,
                    automatic_payload_ordinal, plan_name,
                    observed_at, received_at
                ) VALUES ($1, 'automatic', $2, $3, 1, $4, $5, $5)
                "#,
            )
            .bind(id)
            .bind(series_id)
            .bind(sample_id)
            .bind(plan_name)
            .bind(observed_at)
            .execute(&db.pool)
            .await
            .unwrap();
            return id;
        }
        assert_eq!(source, "manual");
        sqlx::query(
            r#"
            INSERT INTO network_observations (
                id, client_id, kind, source, role, plan_id,
                topology_identity_hash, plan_name, interface_name,
                peer_client_id, target, endpoint_side, address_family,
                stale_after_secs, healthy, transmitted, received,
                packet_loss_ratio, observed_at, received_at
            ) VALUES (
                $1, $2, 'tunnel_reachability', 'manual', 'endpoint', $3,
                $4, $5, 'tun-test', $6, '192.0.2.1',
                'left', 'ipv4', 180, TRUE, 10, 10, 0.0, $7, $7
            )
            "#,
        )
        .bind(id)
        .bind(client_id)
        .bind(plan_id)
        .bind("0".repeat(64))
        .bind(plan_name)
        .bind(peer_client_id)
        .bind(observed_at)
        .execute(&db.pool)
        .await
        .unwrap();
        id
    }

    async fn rollup_sample_count(
        db: &PgWorkerTestDb,
        series_id: i64,
        bucket_secs: i32,
        bucket_start: DateTime<Utc>,
    ) -> i64 {
        sqlx::query_scalar(
            "SELECT sample_count FROM network_observation_rollups \
             WHERE series_id=$1 AND bucket_secs=$2 AND bucket_start=$3",
        )
        .bind(series_id)
        .bind(bucket_secs)
        .bind(bucket_start)
        .fetch_one(&db.pool)
        .await
        .unwrap()
    }

    async fn rollup_row_count(db: &PgWorkerTestDb, series_id: i64, bucket_secs: i32) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM network_observation_rollups \
             WHERE series_id=$1 AND bucket_secs=$2",
        )
        .bind(series_id)
        .bind(bucket_secs)
        .fetch_one(&db.pool)
        .await
        .unwrap()
    }

    async fn observation_exists(db: &PgWorkerTestDb, id: Uuid) -> bool {
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM network_observations WHERE id=$1)")
            .bind(id)
            .fetch_one(&db.pool)
            .await
            .unwrap()
    }
}
