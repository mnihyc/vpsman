use anyhow::{ensure, Result};
use sqlx::{postgres::PgRow, PgPool, Postgres, Row, Transaction};
use tokio::time;
use uuid::Uuid;
use vpsman_common::{
    DEFAULT_TELEMETRY_RETENTION_PRUNE_LIMIT, TRAFFIC_COUNTER_HISTORY_TIERS,
    TRAFFIC_COUNTER_RAW_RETENTION_DAYS,
};
use vpsman_server_core::{
    process_traffic_terminal_retention_page, traffic_terminal_retention_cutoff_unix,
    traffic_terminal_retention_has_remaining_work,
};

use crate::history_retention::{optional_database_deadline, DatabaseDeadline};

const DEFAULT_FINAL_RETENTION_DAYS: i32 = 3_650;
const GROUP_BATCH: i64 = 128;
const PROMOTION_SOURCE_ROW_LIMIT: i64 = 20_000;
const PROMOTION_RAW_PREFIX_LIMIT: i64 = PROMOTION_SOURCE_ROW_LIMIT;
// These durations fence a crashed consumer; they never limit ready work. A
// healthy owner renews twice before expiry, and an abandoned claim is
// recoverable after 30 seconds.
const ACTIVE_CYCLE_REBUILD_LEASE_SECS: i32 = 30;
const ACTIVE_CYCLE_REBUILD_RENEW_SECS: u64 = 10;
const ACTIVE_CYCLE_REBUILD_RETRY_SECS: i32 = 5;
#[cfg(test)]
const MAX_RAW_UNIT_SOURCE_ROWS: i64 = 86_400 / 60 + 1;

#[derive(Clone, Debug)]
struct TrafficActiveCycleRebuildOwner {
    client_id: String,
    requested_revision: i64,
    lease_id: Uuid,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum TrafficActiveCycleRebuildOutcome {
    Current,
    Published,
    Deferred { client_id: String, error: String },
}

enum TrafficActiveCycleRebuildFailure {
    OwnerLocal(String),
    Infrastructure(anyhow::Error),
}

/// Consumes one exact client revision. The rule trigger only advances the
/// durable owner row; retained-hour reconstruction therefore never runs in a
/// request transaction. `Current` is an indexed proof that no revision is due;
/// a successfully deferred owner is durable progress and must not stop healthy
/// owners or unrelated worker lanes.
pub(crate) async fn process_next_traffic_active_cycle_rebuild(
    pool: &PgPool,
) -> Result<TrafficActiveCycleRebuildOutcome> {
    let Some(owner) = claim_traffic_active_cycle_rebuild(pool).await? else {
        return Ok(TrafficActiveCycleRebuildOutcome::Current);
    };

    match rebuild_owned_traffic_active_cycle(pool, &owner).await {
        Ok(()) => {}
        Err(TrafficActiveCycleRebuildFailure::OwnerLocal(error)) => {
            let released = defer_traffic_active_cycle_rebuild(pool, &owner, &error).await?;
            ensure!(
                released,
                "traffic active-cycle rebuild ownership lost after failure"
            );
            return Ok(TrafficActiveCycleRebuildOutcome::Deferred {
                client_id: owner.client_id,
                error,
            });
        }
        Err(TrafficActiveCycleRebuildFailure::Infrastructure(error)) => return Err(error),
    }
    ensure!(
        finish_traffic_active_cycle_rebuild(pool, &owner).await?,
        "traffic active-cycle rebuild ownership lost during publication"
    );
    Ok(TrafficActiveCycleRebuildOutcome::Published)
}

async fn claim_traffic_active_cycle_rebuild(
    pool: &PgPool,
) -> Result<Option<TrafficActiveCycleRebuildOwner>> {
    let lease_id = Uuid::new_v4();
    let row = sqlx::query(
        r#"
        WITH candidate AS (
            SELECT client_id
            FROM traffic_counter_active_cycle_rebuild_work
            WHERE materialized_revision < requested_revision
              AND next_attempt_at <= now()
              AND (lease_until IS NULL OR lease_until <= now())
            ORDER BY next_attempt_at, requested_at, client_id
            LIMIT 1
            FOR UPDATE SKIP LOCKED
        )
        UPDATE traffic_counter_active_cycle_rebuild_work work
        SET lease_id = $1,
            lease_until = now() + ($2::int * interval '1 second'),
            last_error = NULL,
            updated_at = now()
        FROM candidate
        WHERE work.client_id = candidate.client_id
        RETURNING work.client_id, work.requested_revision, work.lease_id
        "#,
    )
    .bind(lease_id)
    .bind(ACTIVE_CYCLE_REBUILD_LEASE_SECS)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        Ok(TrafficActiveCycleRebuildOwner {
            client_id: row.try_get("client_id")?,
            requested_revision: row.try_get("requested_revision")?,
            lease_id: row.try_get("lease_id")?,
        })
    })
    .transpose()
}

async fn rebuild_owned_traffic_active_cycle(
    pool: &PgPool,
    owner: &TrafficActiveCycleRebuildOwner,
) -> std::result::Result<(), TrafficActiveCycleRebuildFailure> {
    let rebuild =
        sqlx::query("SELECT refresh_traffic_counter_active_cycle_usage(ARRAY[$1]::text[])")
            .bind(&owner.client_id)
            .execute(pool);
    tokio::pin!(rebuild);
    let mut renewal = time::interval(time::Duration::from_secs(ACTIVE_CYCLE_REBUILD_RENEW_SECS));
    renewal.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    renewal.tick().await;

    loop {
        tokio::select! {
            result = &mut rebuild => {
                return match result {
                    Ok(_) => Ok(()),
                    Err(error) => {
                        let owner_local = error
                            .as_database_error()
                            .and_then(|database_error| database_error.code())
                            .as_deref()
                            == Some("PZ030");
                        let contextual = anyhow::Error::new(error).context(format!(
                            "failed to rebuild traffic active cycle for {}",
                            owner.client_id
                        ));
                        if owner_local {
                            Err(TrafficActiveCycleRebuildFailure::OwnerLocal(
                                format!("{contextual:#}"),
                            ))
                        } else {
                            Err(TrafficActiveCycleRebuildFailure::Infrastructure(contextual))
                        }
                    }
                };
            }
            _ = renewal.tick() => {
                let renewed = sqlx::query(
                    r#"
                    UPDATE traffic_counter_active_cycle_rebuild_work
                    SET lease_until = now() + ($3::int * interval '1 second'),
                        updated_at = now()
                    WHERE client_id = $1
                      AND lease_id = $2
                      AND lease_until > now()
                    "#,
                )
                .bind(&owner.client_id)
                .bind(owner.lease_id)
                .bind(ACTIVE_CYCLE_REBUILD_LEASE_SECS)
                .execute(pool)
                .await
                .map_err(|error| {
                    TrafficActiveCycleRebuildFailure::Infrastructure(
                        anyhow::Error::new(error).context(format!(
                            "failed to renew traffic active-cycle rebuild lease for {}",
                            owner.client_id
                        )),
                    )
                })?;
                if renewed.rows_affected() != 1 {
                    return Err(TrafficActiveCycleRebuildFailure::Infrastructure(
                        anyhow::anyhow!(
                            "traffic active-cycle rebuild lease lost for {}",
                            owner.client_id
                        ),
                    ));
                }
            }
        }
    }
}

async fn finish_traffic_active_cycle_rebuild(
    pool: &PgPool,
    owner: &TrafficActiveCycleRebuildOwner,
) -> Result<bool> {
    let finished = sqlx::query(
        r#"
        UPDATE traffic_counter_active_cycle_rebuild_work
        SET materialized_revision = GREATEST(materialized_revision, $3),
            lease_id = NULL,
            lease_until = NULL,
            next_attempt_at = now(),
            last_error = NULL,
            updated_at = now()
        WHERE client_id = $1
          AND lease_id = $2
          AND lease_until > now()
        "#,
    )
    .bind(&owner.client_id)
    .bind(owner.lease_id)
    .bind(owner.requested_revision)
    .execute(pool)
    .await?;
    Ok(finished.rows_affected() == 1)
}

async fn defer_traffic_active_cycle_rebuild(
    pool: &PgPool,
    owner: &TrafficActiveCycleRebuildOwner,
    error: &str,
) -> Result<bool> {
    let released = sqlx::query(
        r#"
        UPDATE traffic_counter_active_cycle_rebuild_work
        SET lease_id = NULL,
            lease_until = NULL,
            next_attempt_at = now() + ($4::int * interval '1 second'),
            last_error = left($3, 1000),
            updated_at = now()
        WHERE client_id = $1
          AND lease_id = $2
          AND lease_until > now()
        "#,
    )
    .bind(&owner.client_id)
    .bind(owner.lease_id)
    .bind(error)
    .bind(ACTIVE_CYCLE_REBUILD_RETRY_SECS)
    .execute(pool)
    .await?;
    Ok(released.rows_affected() == 1)
}

#[cfg(test)]
tokio::task_local! {
    static FRONTIER_QUERY_COUNT: std::cell::Cell<u64>;
}

#[inline]
fn record_frontier_query() {
    #[cfg(test)]
    let _ = FRONTIER_QUERY_COUNT.try_with(|count| count.set(count.get().saturating_add(1)));
}

#[cfg(test)]
async fn count_frontier_queries_for_test<F>(future: F) -> (F::Output, u64)
where
    F: std::future::Future,
{
    FRONTIER_QUERY_COUNT
        .scope(std::cell::Cell::new(0), async move {
            let output = future.await;
            let count = FRONTIER_QUERY_COUNT.with(std::cell::Cell::get);
            (output, count)
        })
        .await
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TrafficRetentionRun {
    pub(crate) raw_rows_promoted: u64,
    pub(crate) rollup_rows_promoted: u64,
    pub(crate) rollup_rows_pruned: u64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TrafficRetentionPhaseOutcome {
    pub(crate) run: TrafficRetentionRun,
    /// True only after candidate discovery selected a page, including a page
    /// whose rows were subsequently lock-deferred. A false value is the
    /// adapter's exact indexed proof that this phase is already Current.
    pub(crate) attempted: bool,
    /// Terminal pruning derives its UTC boundary once from PostgreSQL, then
    /// reuses that exact boundary for this post-page frontier proof. Other
    /// phases leave completion proof to their existing owner-specific path.
    pub(crate) terminal_has_remaining_work: Option<bool>,
    pub(crate) network_rate_rows_published: u64,
}

impl TrafficRetentionPhaseOutcome {
    fn attempted(run: TrafficRetentionRun) -> Self {
        Self {
            run,
            attempted: true,
            terminal_has_remaining_work: None,
            network_rate_rows_published: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct Tier {
    source_secs: &'static [i32],
    destination_secs: i32,
    source_retention_days: i32,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct TrafficStreamKey {
    client_id: String,
    source_kind: String,
    interface: String,
}

#[derive(Clone, Debug)]
struct TrafficRetentionPolicy {
    pruning_enabled: bool,
    final_retention_days: i32,
    prune_limit: i32,
}

#[derive(Clone, Debug)]
struct TrafficPhaseCursor {
    stream: Option<TrafficStreamKey>,
    lane: Option<String>,
    frontier_start: Option<chrono::DateTime<chrono::Utc>>,
    scan_after: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Clone, Debug)]
struct TrafficPhaseCandidate {
    stream: TrafficStreamKey,
    lane: String,
    frontier_start: chrono::DateTime<chrono::Utc>,
    bucket_start: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug)]
struct TrafficPhaseReservation {
    candidate: TrafficPhaseCandidate,
    cursor_revision: chrono::DateTime<chrono::Utc>,
    work_available: bool,
}

const TIERS: [Tier; 3] = [
    Tier {
        source_secs: &[3_600, 10_800, 21_600],
        destination_secs: TRAFFIC_COUNTER_HISTORY_TIERS[3].bucket_secs,
        source_retention_days: TRAFFIC_COUNTER_HISTORY_TIERS[2].retain_days,
    },
    Tier {
        source_secs: &[3_600, 10_800],
        destination_secs: TRAFFIC_COUNTER_HISTORY_TIERS[2].bucket_secs,
        source_retention_days: TRAFFIC_COUNTER_HISTORY_TIERS[1].retain_days,
    },
    Tier {
        source_secs: &[3_600],
        destination_secs: TRAFFIC_COUNTER_HISTORY_TIERS[1].bucket_secs,
        source_retention_days: TRAFFIC_COUNTER_HISTORY_TIERS[0].retain_days,
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TrafficRetentionPhase {
    RawPromotion,
    RollupToDay,
    RollupToSixHours,
    RollupToThreeHours,
    TerminalPrune,
}

#[cfg(test)]
pub(crate) const TRAFFIC_RETENTION_PHASES: [TrafficRetentionPhase; 5] = [
    TrafficRetentionPhase::RawPromotion,
    TrafficRetentionPhase::RollupToDay,
    TrafficRetentionPhase::RollupToSixHours,
    TrafficRetentionPhase::RollupToThreeHours,
    TrafficRetentionPhase::TerminalPrune,
];

impl TrafficRetentionPhase {
    pub(crate) fn cursor_key(self) -> (&'static str, i32, i32) {
        let (source_bucket_secs, destination_bucket_secs) = match self {
            Self::RawPromotion => (0, -1),
            Self::RollupToDay => (21_600, 86_400),
            Self::RollupToSixHours => (10_800, 21_600),
            Self::RollupToThreeHours => (3_600, 10_800),
            Self::TerminalPrune => (0, 0),
        };
        (
            "traffic_counter_samples",
            source_bucket_secs,
            destination_bucket_secs,
        )
    }

    fn tier(self) -> Option<Tier> {
        match self {
            Self::RollupToDay => Some(TIERS[0]),
            Self::RollupToSixHours => Some(TIERS[1]),
            Self::RollupToThreeHours => Some(TIERS[2]),
            Self::RawPromotion | Self::TerminalPrune => None,
        }
    }
}

/// Executes one bounded traffic-counter lifecycle phase. Page scheduling and
/// catch-up cadence belong to the common retention coordinator; the traffic
/// adapter retains ownership of counter/reset reconstruction and cursor SQL.
pub(crate) async fn process_traffic_retention_phase(
    pool: &PgPool,
    phase: TrafficRetentionPhase,
) -> Result<TrafficRetentionPhaseOutcome> {
    let outcome = match phase {
        TrafficRetentionPhase::RawPromotion => process_raw_promotion_phase(pool).await?,
        TrafficRetentionPhase::RollupToDay
        | TrafficRetentionPhase::RollupToSixHours
        | TrafficRetentionPhase::RollupToThreeHours => {
            process_rollup_promotion_phase(
                pool,
                phase.tier().expect("rollup traffic phase has a tier"),
            )
            .await?
        }
        TrafficRetentionPhase::TerminalPrune => {
            let policy = load_traffic_retention_policy(pool).await?;
            if !policy.pruning_enabled {
                TrafficRetentionPhaseOutcome::default()
            } else {
                let cutoff_unix =
                    traffic_terminal_retention_cutoff_unix(pool, policy.final_retention_days)
                        .await?;
                let page =
                    process_traffic_terminal_retention_page(pool, cutoff_unix, policy.prune_limit)
                        .await?;
                let terminal_has_remaining_work = if page.attempted {
                    Some(traffic_terminal_retention_has_remaining_work(pool, cutoff_unix).await?)
                } else {
                    Some(false)
                };
                TrafficRetentionPhaseOutcome {
                    run: TrafficRetentionRun {
                        rollup_rows_pruned: page.pruned_rows,
                        ..TrafficRetentionRun::default()
                    },
                    attempted: page.attempted,
                    terminal_has_remaining_work,
                    network_rate_rows_published: 0,
                }
            }
        }
    };
    Ok(outcome)
}

/// Re-check the exact durable frontier owned by one traffic phase after its
/// bounded page commits. This is completion evidence only: it neither advances
/// the cursor nor changes the phase's batch limits or reconstruction rules.
pub(crate) async fn traffic_retention_phase_has_remaining_work(
    pool: &PgPool,
    phase: TrafficRetentionPhase,
) -> Result<bool> {
    if phase == TrafficRetentionPhase::TerminalPrune {
        let policy = load_traffic_retention_policy(pool).await?;
        if !policy.pruning_enabled {
            return Ok(false);
        }
        let cutoff_unix =
            traffic_terminal_retention_cutoff_unix(pool, policy.final_retention_days).await?;
        return traffic_terminal_retention_has_remaining_work(pool, cutoff_unix).await;
    }

    let (domain, source_bucket_secs, destination_bucket_secs) = phase.cursor_key();
    debug_assert_eq!(domain, "traffic_counter_samples");
    let mut tx = pool.begin().await?;
    let cursor =
        read_traffic_phase_cursor(&mut tx, source_bucket_secs, destination_bucket_secs).await?;
    let has_remaining_work = match phase {
        TrafficRetentionPhase::RawPromotion => find_raw_phase_candidate(&mut tx, &cursor, &[])
            .await?
            .is_some(),
        TrafficRetentionPhase::RollupToDay
        | TrafficRetentionPhase::RollupToSixHours
        | TrafficRetentionPhase::RollupToThreeHours => {
            let tier = phase.tier().expect("traffic rollup phase has a tier");
            find_rollup_phase_candidate(
                &mut tx,
                &cursor,
                source_bucket_secs,
                tier.source_retention_days,
                &[],
            )
            .await?
            .is_some()
        }
        TrafficRetentionPhase::TerminalPrune => unreachable!("terminal phase returned above"),
    };
    tx.rollback().await?;
    Ok(has_remaining_work)
}

/// Exact future eligibility of the oldest indexed traffic frontier. A missing
/// row is producer-only; it is not replaced with a polling interval. These
/// probes preserve the same natural frontier and UTC boundary predicates used
/// by phase discovery, without advancing or locking the cursor.
pub(crate) async fn traffic_retention_phase_next_at(
    pool: &PgPool,
    phase: TrafficRetentionPhase,
) -> Result<Option<DatabaseDeadline>> {
    match phase {
        TrafficRetentionPhase::RawPromotion => optional_database_deadline(
            sqlx::query_as(
                r#"
            WITH frontier AS (
                SELECT date_bin(
                           interval '1 hour', first_unpromoted_observed_at,
                           TIMESTAMPTZ '1970-01-01 00:00:00+00'
                       ) + interval '1 hour' + make_interval(days => $1)
                         AS database_at
                FROM traffic_counter_streams
                WHERE first_unpromoted_observed_at IS NOT NULL
                ORDER BY first_unpromoted_observed_at,
                         client_id, source_kind, interface
                LIMIT 1
            )
            SELECT database_at,
                   GREATEST(
                       EXTRACT(EPOCH FROM database_at - clock_timestamp()), 0
                   )::DOUBLE PRECISION AS remaining_seconds
            FROM frontier
            "#,
            )
            .bind(TRAFFIC_COUNTER_RAW_RETENTION_DAYS)
            .fetch_optional(pool)
            .await?,
        ),
        TrafficRetentionPhase::RollupToDay
        | TrafficRetentionPhase::RollupToSixHours
        | TrafficRetentionPhase::RollupToThreeHours => {
            let tier = phase.tier().expect("traffic rollup phase has a tier");
            let source_bucket_secs = *tier
                .source_secs
                .last()
                .expect("traffic rollup tier has an immediate predecessor");
            optional_database_deadline(
                sqlx::query_as(
                    r#"
                WITH frontier AS (
                    SELECT (
                        date_trunc(
                            'day',
                            (
                                bucket_start + make_interval(secs => $1)
                                    - interval '1 microsecond'
                            ) AT TIME ZONE 'UTC'
                        ) + interval '1 day'
                    ) AT TIME ZONE 'UTC' + make_interval(days => $2)
                      AS database_at
                    FROM traffic_counter_rollups
                    WHERE bucket_secs = $1
                    ORDER BY bucket_start, client_id, source_kind,
                             interface, origin_kind
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
                .bind(tier.source_retention_days)
                .fetch_optional(pool)
                .await?,
            )
        }
        TrafficRetentionPhase::TerminalPrune => {
            let policy = load_traffic_retention_policy(pool).await?;
            if !policy.pruning_enabled {
                return Ok(None);
            }
            optional_database_deadline(
                sqlx::query_as(
                    r#"
                WITH tiers(bucket_secs) AS (
                    VALUES (3600), (10800), (21600), (86400)
                ), oldest AS (
                    SELECT source.bucket_start, tiers.bucket_secs
                    FROM tiers
                    JOIN LATERAL (
                        SELECT bucket_start
                        FROM traffic_counter_rollups
                        WHERE bucket_secs = tiers.bucket_secs
                        ORDER BY bucket_start, client_id, source_kind,
                                 interface, origin_kind
                        LIMIT 1
                    ) source ON TRUE
                ), frontier AS (
                    SELECT min((
                        date_trunc(
                            'day',
                            (
                                bucket_start + make_interval(secs => bucket_secs)
                                    - interval '1 microsecond'
                            ) AT TIME ZONE 'UTC'
                        ) + interval '1 day'
                    ) AT TIME ZONE 'UTC' + make_interval(days => $1)) AS database_at
                    FROM oldest
                )
                SELECT database_at,
                       GREATEST(
                           EXTRACT(EPOCH FROM database_at - clock_timestamp()), 0
                       )::DOUBLE PRECISION AS remaining_seconds
                FROM frontier
                WHERE database_at IS NOT NULL
                "#,
                )
                .bind(policy.final_retention_days)
                .fetch_optional(pool)
                .await?,
            )
        }
    }
}

/// Materializes old counter transitions before deleting exact endpoints, then
/// promotes them through the fixed LTS tiers. The phase cursor is only a short
/// keyset-discovery frontier. The existing client-row -> traffic-advisory ->
/// source-row order owns exact work and serializes it with ingest and vnStat
/// replacement. No outer telemetry-history lease is part of this boundary.
#[cfg(test)]
pub(crate) async fn process_traffic_retention(pool: &PgPool) -> Result<TrafficRetentionRun> {
    let mut run = TrafficRetentionRun::default();
    for phase in TRAFFIC_RETENTION_PHASES {
        let outcome = process_traffic_retention_phase(pool, phase).await?;
        merge_run(&mut run, outcome.run);
        if outcome.attempted {
            // Mirror the production coordinator: only an attempted page needs
            // a post-page frontier proof. An empty preprobe is already exact.
            if outcome.terminal_has_remaining_work.is_none() {
                let _ = traffic_retention_phase_has_remaining_work(pool, phase).await?;
            }
        }
    }
    Ok(run)
}

async fn load_traffic_retention_policy(pool: &PgPool) -> Result<TrafficRetentionPolicy> {
    let policy = sqlx::query(
        r#"
        SELECT enabled, retention_days, prune_limit
        FROM history_retention_policies
        WHERE domain = 'traffic_counter_rollups'
        "#,
    )
    .fetch_optional(pool)
    .await?;
    let pruning_enabled = policy
        .as_ref()
        .map(|row| row.try_get::<bool, _>("enabled"))
        .transpose()?
        .unwrap_or(true);
    let final_retention_days = policy
        .as_ref()
        .map(|row| row.try_get::<i32, _>("retention_days"))
        .transpose()?
        .unwrap_or(DEFAULT_FINAL_RETENTION_DAYS)
        .clamp(
            vpsman_common::MIN_TRAFFIC_COUNTER_ROLLUP_RETENTION_DAYS,
            DEFAULT_FINAL_RETENTION_DAYS,
        );
    let prune_limit = policy
        .as_ref()
        .map(|row| row.try_get::<i32, _>("prune_limit"))
        .transpose()?
        .unwrap_or(DEFAULT_TELEMETRY_RETENTION_PRUNE_LIMIT)
        .clamp(1, 100_000);
    Ok(TrafficRetentionPolicy {
        pruning_enabled,
        final_retention_days,
        prune_limit,
    })
}

async fn process_raw_promotion_phase(pool: &PgPool) -> Result<TrafficRetentionPhaseOutcome> {
    let Some(reservation) = reserve_raw_phase_candidate(pool).await? else {
        return Ok(TrafficRetentionPhaseOutcome::default());
    };
    if !reservation.work_available {
        return Ok(TrafficRetentionPhaseOutcome::attempted(
            TrafficRetentionRun::default(),
        ));
    }
    let candidate = &reservation.candidate;

    // The singleton phase frontier has already committed its short discovery
    // turn. Exact work now owns only the client row, the traffic advisory, and
    // the selected source/destination coordinates; another replica can use the
    // frontier immediately to discover an independent stream.
    let mut tx = pool.begin().await?;
    if !try_lock_client_row_then_traffic(&mut tx, &candidate.stream.client_id).await? {
        // The reservation advanced only a scheduling hint. The untouched
        // source remains durably due and wrap will revisit it.
        tx.rollback().await?;
        return Ok(TrafficRetentionPhaseOutcome::attempted(
            TrafficRetentionRun::default(),
        ));
    };
    ensure_traffic_active_cycle_ready(&mut tx, &candidate.stream).await?;

    // Retention deletes exact rows only after the equivalent rollup is
    // durable. Its bounded hourly repair runs below; the generic statement
    // trigger would otherwise turn a >4096-row page into a whole-stream scan.
    sqlx::query("SELECT set_config('vpsman.traffic_retention_hourly_delete_managed', 'on', true)")
        .execute(&mut *tx)
        .await?;
    // The admitted host minute remains the same already-closed network point
    // while this transaction changes only its retained physical owner.
    // Dashboard publication is therefore silent; the ordinary history due
    // producer remains independent and schedules its day-two promotion.
    sqlx::query("SELECT set_config('vpsman.telemetry_retained_ownership_transfer', 'on', true)")
        .execute(&mut *tx)
        .await?;
    let row = sqlx::query(raw_promotion_sql())
        .bind(&candidate.stream.client_id)
        .bind(vec![candidate.stream.source_kind.as_str()])
        .bind(vec![candidate.stream.interface.as_str()])
        .bind(TRAFFIC_COUNTER_RAW_RETENTION_DAYS)
        .bind(GROUP_BATCH)
        .bind(PROMOTION_SOURCE_ROW_LIMIT)
        .bind(PROMOTION_RAW_PREFIX_LIMIT)
        .bind(candidate.bucket_start)
        .fetch_one(&mut *tx)
        .await?;
    let insert_race_conflicts = row.try_get::<i64, _>("insert_race_conflicts")?;
    if insert_race_conflicts > 0 {
        // The client advisory makes this unreachable for supported writers.
        // Roll back the complete source replacement rather than publishing a
        // partial origin set after an out-of-band destination race. The cursor
        // reservation is only a scheduling hint; unchanged source evidence is
        // still discovered on keyset wrap and cannot certify completion.
        tx.rollback().await?;
        anyhow::bail!(
            "unsupported concurrent traffic destination insert caused {insert_race_conflicts} conflicts"
        );
    }
    let conflicts = row.try_get::<i64, _>("conflicts")?.max(0) as u64;
    if conflicts > 0 {
        tx.rollback().await?;
        anyhow::bail!(
            "traffic raw promotion found {conflicts} unsupported pre-existing destination conflicts"
        );
    }

    let deleted_rows = row.try_get::<i64, _>("deleted_rows")?.max(0) as u64;
    let network_rate_rows_published =
        row.try_get::<i64, _>("network_rate_rows_published")?.max(0) as u64;
    if deleted_rows > 0 {
        let client_ids = row.try_get::<Vec<String>, _>("hourly_client_ids")?;
        let source_kinds = row.try_get::<Vec<String>, _>("hourly_source_kinds")?;
        let interfaces = row.try_get::<Vec<String>, _>("hourly_interfaces")?;
        let observed_at =
            row.try_get::<Vec<chrono::DateTime<chrono::Utc>>, _>("hourly_observed_at")?;
        // Statement-level edge triggers can observe an intermediate boundary
        // registry while one CTE both marks the new endpoint and deletes the
        // previous one. Normalize final edges before the hourly revision bump,
        // then align their revision once more after that exact bounded repair.
        sqlx::query(
            r#"
            SELECT refresh_traffic_counter_sample_edges(
                $1::text[], $2::text[], $3::text[]
            )
            "#,
        )
        .bind(&client_ids)
        .bind(&source_kinds)
        .bind(&interfaces)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            SELECT refresh_traffic_counter_hourly_usage(
                $1::text[], $2::text[], $3::text[], $4::timestamptz[], FALSE
            )
            "#,
        )
        .bind(&client_ids)
        .bind(&source_kinds)
        .bind(&interfaces)
        .bind(&observed_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            SELECT refresh_traffic_counter_sample_edges(
                $1::text[], $2::text[], $3::text[]
            )
            "#,
        )
        .bind(&client_ids)
        .bind(&source_kinds)
        .bind(&interfaces)
        .execute(&mut *tx)
        .await?;
        // Retention is a representation change, not a reconstruction owner.
        // Every derived delta must leave the active prefix aligned; crossing an
        // hour boundary or finding damaged state rolls this page back visibly.
        ensure_traffic_active_cycle_ready(&mut tx, &candidate.stream).await?;
    }
    sqlx::query("SELECT set_config('vpsman.traffic_retention_hourly_delete_managed', 'off', true)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT set_config('vpsman.telemetry_retained_ownership_transfer', 'off', true)")
        .execute(&mut *tx)
        .await?;

    let next_scan_after =
        row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("next_scan_after")?;
    tx.commit().await?;

    // A complete page may have an immediate same-stream successor. Publish it
    // only if no other discovery turn has moved the frontier since this page
    // was reserved. This CAS is a post-commit locality hint; source rows remain
    // the durable truth across a failed CAS or process crash.
    if let Some(next_scan_after) = next_scan_after {
        publish_raw_phase_successor(pool, &reservation, next_scan_after).await?;
    }
    let mut outcome = TrafficRetentionPhaseOutcome::attempted(TrafficRetentionRun {
        raw_rows_promoted: deleted_rows,
        ..TrafficRetentionRun::default()
    });
    outcome.network_rate_rows_published = network_rate_rows_published;
    Ok(outcome)
}

async fn ensure_traffic_active_cycle_ready(
    tx: &mut Transaction<'_, Postgres>,
    stream: &TrafficStreamKey,
) -> Result<()> {
    let requires_rebuild = sqlx::query_scalar(
        r#"
        WITH reset AS (
            SELECT
                CASE
                    WHEN rule.value_json->>'day' ~ '^-?[0-9]+$'
                     AND (rule.value_json->>'day')::integer
                            BETWEEN -1 AND 31
                     AND (rule.value_json->>'day')::integer <> 0
                    THEN (rule.value_json->>'day')::integer
                    ELSE 1
                END AS day,
                CASE
                    WHEN rule.value_json->>'hour' ~ '^[0-9]+$'
                     AND (rule.value_json->>'hour')::integer BETWEEN 0 AND 23
                    THEN (rule.value_json->>'hour')::integer
                    ELSE 0
                END AS hour
            FROM (SELECT 1) seed
            LEFT JOIN vps_rule_values rule
              ON rule.client_id = $1
             AND rule.key = 'traffic.reset_day'
        ), bounds AS (
            SELECT
                reset.*,
                traffic_counter_cycle_start_utc(
                    reset.day, reset.hour, statement_timestamp()
                ) AS cycle_start,
                date_bin(
                    interval '1 hour', statement_timestamp(),
                    TIMESTAMPTZ '1970-01-01 00:00:00+00'
                ) AS completed_through
            FROM reset
        )
        SELECT NOT (
            stream.source_revision = stream.materialized_revision
            AND stream.sample_edge_revision = stream.materialized_revision
            AND stream.promoted_boundary_safe
            AND (
                bounds.day = -1
                OR (
                    active.client_id IS NOT NULL
                    AND active.source_revision = active.materialized_revision
                    AND (
                        (
                            active.cycle_start = bounds.cycle_start
                            AND active.completed_through <=
                                bounds.completed_through
                            AND stream.latest_sample_observed_at <
                                active.completed_through + interval '1 hour'
                        )
                        OR (
                            active.cycle_start < bounds.cycle_start
                            AND stream.latest_sample_observed_at <
                                bounds.cycle_start
                        )
                    )
                )
            )
        )
        FROM traffic_counter_streams stream
        CROSS JOIN bounds
        LEFT JOIN traffic_counter_active_cycle_usage active
          ON active.client_id = stream.client_id
         AND active.source_kind = stream.source_kind
         AND active.interface = stream.interface
        WHERE stream.client_id = $1
          AND stream.source_kind = $2
          AND stream.interface = $3
        "#,
    )
    .bind(&stream.client_id)
    .bind(&stream.source_kind)
    .bind(&stream.interface)
    .fetch_optional(&mut **tx)
    .await?;
    anyhow::ensure!(
        matches!(requires_rebuild, Some(false)),
        "traffic retention found an unready active-cycle authority for {}/{}/{}",
        stream.client_id,
        stream.source_kind,
        stream.interface
    );
    Ok(())
}

async fn process_rollup_promotion_phase(
    pool: &PgPool,
    tier: Tier,
) -> Result<TrafficRetentionPhaseOutcome> {
    let source_bucket_secs = *tier
        .source_secs
        .last()
        .expect("traffic tier has an immediate predecessor");
    let Some(reservation) = reserve_rollup_phase_candidate(pool, tier).await? else {
        return Ok(TrafficRetentionPhaseOutcome::default());
    };
    if !reservation.work_available {
        return Ok(TrafficRetentionPhaseOutcome::attempted(
            TrafficRetentionRun::default(),
        ));
    }
    let candidate = &reservation.candidate;

    let mut tx = pool.begin().await?;
    if !try_lock_client_row_then_traffic(&mut tx, &candidate.stream.client_id).await? {
        tx.rollback().await?;
        return Ok(TrafficRetentionPhaseOutcome::attempted(
            TrafficRetentionRun::default(),
        ));
    }

    let row = sqlx::query(rollup_promotion_sql())
        .bind(&candidate.stream.client_id)
        .bind(vec![candidate.stream.source_kind.as_str()])
        .bind(vec![candidate.stream.interface.as_str()])
        .bind(tier.source_secs.to_vec())
        .bind(tier.destination_secs)
        .bind(source_bucket_secs)
        .bind(tier.source_retention_days)
        .bind(GROUP_BATCH)
        .bind(PROMOTION_SOURCE_ROW_LIMIT)
        .bind(&candidate.lane)
        .bind(candidate.bucket_start)
        .fetch_one(&mut *tx)
        .await?;
    let conflicts = row.try_get::<i64, _>("conflicts")?.max(0) as u64;
    if conflicts > 0 {
        tx.rollback().await?;
        anyhow::bail!(
            "traffic rollup promotion from {source_bucket_secs}s to {}s found {conflicts} unsupported destination conflicts",
            tier.destination_secs
        );
    }
    let promoted = row.try_get::<i64, _>("deleted_rows")?.max(0) as u64;
    let run = TrafficRetentionRun {
        rollup_rows_promoted: promoted,
        ..TrafficRetentionRun::default()
    };
    tx.commit().await?;
    Ok(TrafficRetentionPhaseOutcome::attempted(run))
}

async fn read_traffic_phase_cursor(
    tx: &mut Transaction<'_, Postgres>,
    source_bucket_secs: i32,
    destination_bucket_secs: i32,
) -> Result<TrafficPhaseCursor> {
    let row = sqlx::query(
        r#"
        SELECT traffic_client_id, traffic_source_kind, traffic_interface,
               traffic_lane, traffic_frontier_start, traffic_scan_after
        FROM traffic_history_retention_cursors
        WHERE domain = 'traffic_counter_samples'
          AND source_bucket_secs = $1
          AND destination_bucket_secs = $2
        "#,
    )
    .bind(source_bucket_secs)
    .bind(destination_bucket_secs)
    .fetch_one(&mut **tx)
    .await?;
    traffic_phase_cursor_from_row(row, source_bucket_secs, destination_bucket_secs)
}

async fn lock_traffic_phase_cursor(
    tx: &mut Transaction<'_, Postgres>,
    source_bucket_secs: i32,
    destination_bucket_secs: i32,
) -> Result<TrafficPhaseCursor> {
    let row = sqlx::query(
        r#"
        SELECT traffic_client_id, traffic_source_kind, traffic_interface,
               traffic_lane, traffic_frontier_start, traffic_scan_after
        FROM traffic_history_retention_cursors
        WHERE domain = 'traffic_counter_samples'
          AND source_bucket_secs = $1
          AND destination_bucket_secs = $2
        FOR UPDATE
        "#,
    )
    .bind(source_bucket_secs)
    .bind(destination_bucket_secs)
    .fetch_one(&mut **tx)
    .await?;
    traffic_phase_cursor_from_row(row, source_bucket_secs, destination_bucket_secs)
}

fn traffic_phase_cursor_from_row(
    row: PgRow,
    source_bucket_secs: i32,
    destination_bucket_secs: i32,
) -> Result<TrafficPhaseCursor> {
    let client_id = row.try_get::<Option<String>, _>("traffic_client_id")?;
    let source_kind = row.try_get::<Option<String>, _>("traffic_source_kind")?;
    let interface = row.try_get::<Option<String>, _>("traffic_interface")?;
    let lane = row.try_get::<Option<String>, _>("traffic_lane")?;
    let frontier_start =
        row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("traffic_frontier_start")?;
    let scan_after =
        row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("traffic_scan_after")?;
    let stream = match (client_id, source_kind, interface) {
        (Some(client_id), Some(source_kind), Some(interface)) => Some(TrafficStreamKey {
            client_id,
            source_kind,
            interface,
        }),
        (None, None, None) => None,
        _ => anyhow::bail!("traffic phase cursor has an invalid partial stream key"),
    };
    anyhow::ensure!(
        stream.is_some() == lane.is_some() && lane.is_some() == scan_after.is_some(),
        "traffic phase cursor has an invalid partial frontier"
    );
    if source_bucket_secs == 0 && destination_bucket_secs == -1 {
        anyhow::ensure!(
            stream.is_some() == frontier_start.is_some(),
            "traffic raw cursor has an invalid partial global frontier"
        );
    } else {
        anyhow::ensure!(
            frontier_start.is_none(),
            "non-raw traffic cursor unexpectedly owns a raw global frontier"
        );
    }
    Ok(TrafficPhaseCursor {
        stream,
        lane,
        frontier_start,
        scan_after,
    })
}

async fn update_traffic_phase_cursor(
    tx: &mut Transaction<'_, Postgres>,
    source_bucket_secs: i32,
    destination_bucket_secs: i32,
    stream: Option<&TrafficStreamKey>,
    lane: Option<&str>,
    frontier_start: Option<chrono::DateTime<chrono::Utc>>,
    scan_after: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<chrono::DateTime<chrono::Utc>> {
    let updated_at = sqlx::query_scalar(
        r#"
        UPDATE traffic_history_retention_cursors
        SET traffic_client_id = $1,
            traffic_source_kind = $2,
            traffic_interface = $3,
            traffic_lane = $4,
            traffic_frontier_start = $5,
            traffic_scan_after = $6,
            updated_at = clock_timestamp()
        WHERE domain = 'traffic_counter_samples'
          AND source_bucket_secs = $7
          AND destination_bucket_secs = $8
        RETURNING updated_at
        "#,
    )
    .bind(stream.map(|value| value.client_id.as_str()))
    .bind(stream.map(|value| value.source_kind.as_str()))
    .bind(stream.map(|value| value.interface.as_str()))
    .bind(lane)
    .bind(frontier_start)
    .bind(scan_after)
    .bind(source_bucket_secs)
    .bind(destination_bucket_secs)
    .fetch_one(&mut **tx)
    .await?;
    Ok(updated_at)
}

async fn clear_traffic_phase_cursor_if_positioned(
    tx: &mut Transaction<'_, Postgres>,
    source_bucket_secs: i32,
    destination_bucket_secs: i32,
    cursor: &TrafficPhaseCursor,
) -> Result<()> {
    if cursor.stream.is_some() {
        let _ = update_traffic_phase_cursor(
            tx,
            source_bucket_secs,
            destination_bucket_secs,
            None,
            None,
            None,
            None,
        )
        .await?;
    }
    Ok(())
}

async fn reserve_raw_phase_candidate(pool: &PgPool) -> Result<Option<TrafficPhaseReservation>> {
    let mut tx = pool.begin().await?;
    let cursor = lock_traffic_phase_cursor(&mut tx, 0, -1).await?;
    let mut unavailable_clients = Vec::new();
    let mut last_unavailable = None;
    let (candidate, work_available) = loop {
        let Some(candidate) =
            find_raw_phase_candidate(&mut tx, &cursor, &unavailable_clients).await?
        else {
            let Some(candidate) = last_unavailable else {
                // A stable empty indexed frontier is itself the completion
                // proof. Do not rotate through every stream merely to
                // rediscover that no work is due.
                clear_traffic_phase_cursor_if_positioned(&mut tx, 0, -1, &cursor).await?;
                tx.commit().await?;
                return Ok(None);
            };
            break (candidate, false);
        };
        if try_lock_client_row_then_traffic(&mut tx, &candidate.stream.client_id).await? {
            break (candidate, true);
        }
        unavailable_clients.push(candidate.stream.client_id.clone());
        last_unavailable = Some(candidate);
    };

    // Reserve a scheduling turn, not the source data. `raw_deferred` makes the
    // next replica continue after this global key while the exact work below
    // owns the selected client/stream. A crash leaves the source durably due;
    // the indexed wrap revisits it without recovery state.
    let cursor_revision = update_traffic_phase_cursor(
        &mut tx,
        0,
        -1,
        Some(&candidate.stream),
        Some("raw_deferred"),
        Some(candidate.frontier_start),
        Some(candidate.bucket_start),
    )
    .await?;
    tx.commit().await?;
    Ok(Some(TrafficPhaseReservation {
        candidate,
        cursor_revision,
        work_available,
    }))
}

async fn reserve_rollup_phase_candidate(
    pool: &PgPool,
    tier: Tier,
) -> Result<Option<TrafficPhaseReservation>> {
    let source_bucket_secs = *tier
        .source_secs
        .last()
        .expect("traffic tier has an immediate predecessor");
    let mut tx = pool.begin().await?;
    let cursor =
        lock_traffic_phase_cursor(&mut tx, source_bucket_secs, tier.destination_secs).await?;
    let mut unavailable_clients = Vec::new();
    let mut last_unavailable = None;
    let (candidate, work_available) = loop {
        let Some(candidate) = find_rollup_phase_candidate(
            &mut tx,
            &cursor,
            source_bucket_secs,
            tier.source_retention_days,
            &unavailable_clients,
        )
        .await?
        else {
            let Some(candidate) = last_unavailable else {
                clear_traffic_phase_cursor_if_positioned(
                    &mut tx,
                    source_bucket_secs,
                    tier.destination_secs,
                    &cursor,
                )
                .await?;
                tx.commit().await?;
                return Ok(None);
            };
            break (candidate, false);
        };
        if try_lock_client_row_then_traffic(&mut tx, &candidate.stream.client_id).await? {
            break (candidate, true);
        }
        unavailable_clients.push(candidate.stream.client_id.clone());
        last_unavailable = Some(candidate);
    };
    let cursor_revision = update_traffic_phase_cursor(
        &mut tx,
        source_bucket_secs,
        tier.destination_secs,
        Some(&candidate.stream),
        Some(&candidate.lane),
        None,
        Some(candidate.bucket_start),
    )
    .await?;
    tx.commit().await?;
    Ok(Some(TrafficPhaseReservation {
        candidate,
        cursor_revision,
        work_available,
    }))
}

async fn publish_raw_phase_successor(
    pool: &PgPool,
    reservation: &TrafficPhaseReservation,
    scan_after: chrono::DateTime<chrono::Utc>,
) -> Result<()> {
    let candidate = &reservation.candidate;
    sqlx::query(
        r#"
        UPDATE traffic_history_retention_cursors
        SET traffic_lane = 'raw',
            traffic_scan_after = $1,
            updated_at = clock_timestamp()
        WHERE domain = 'traffic_counter_samples'
          AND source_bucket_secs = 0
          AND destination_bucket_secs = -1
          AND updated_at = $2
          AND traffic_client_id = $3
          AND traffic_source_kind = $4
          AND traffic_interface = $5
          AND traffic_lane = 'raw_deferred'
          AND traffic_frontier_start = $6
          AND traffic_scan_after = $7
        "#,
    )
    .bind(scan_after)
    .bind(reservation.cursor_revision)
    .bind(&candidate.stream.client_id)
    .bind(&candidate.stream.source_kind)
    .bind(&candidate.stream.interface)
    .bind(candidate.frontier_start)
    .bind(candidate.bucket_start)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
fn merge_run(total: &mut TrafficRetentionRun, current: TrafficRetentionRun) {
    total.raw_rows_promoted = total
        .raw_rows_promoted
        .saturating_add(current.raw_rows_promoted);
    total.rollup_rows_promoted = total
        .rollup_rows_promoted
        .saturating_add(current.rollup_rows_promoted);
    total.rollup_rows_pruned = total
        .rollup_rows_pruned
        .saturating_add(current.rollup_rows_pruned);
}

fn phase_candidate_from_row(row: PgRow) -> Result<TrafficPhaseCandidate> {
    Ok(TrafficPhaseCandidate {
        stream: TrafficStreamKey {
            client_id: row.try_get("client_id")?,
            source_kind: row.try_get("source_kind")?,
            interface: row.try_get("interface")?,
        },
        lane: row.try_get("lane")?,
        frontier_start: row.try_get("frontier_start")?,
        bucket_start: row.try_get("bucket_start")?,
    })
}

fn raw_frontier_start_sql() -> &'static str {
    r#"
        SELECT client_id, source_kind, interface,
               'raw'::text AS lane,
               first_unpromoted_observed_at AS frontier_start,
               first_unpromoted_observed_at AS bucket_start
        FROM traffic_counter_streams
        WHERE first_unpromoted_observed_at <
              date_bin(
                  interval '1 hour', now() - make_interval(days => $1),
                  TIMESTAMPTZ '1970-01-01 00:00:00+00'
              )
          AND NOT (client_id = ANY($2::text[]))
        ORDER BY first_unpromoted_observed_at,
                 client_id, source_kind, interface
        LIMIT 1
    "#
}

fn raw_frontier_after_sql() -> &'static str {
    r#"
        SELECT client_id, source_kind, interface,
               'raw'::text AS lane,
               first_unpromoted_observed_at AS frontier_start,
               first_unpromoted_observed_at AS bucket_start
        FROM traffic_counter_streams
        WHERE first_unpromoted_observed_at <
              date_bin(
                  interval '1 hour', now() - make_interval(days => $1),
                  TIMESTAMPTZ '1970-01-01 00:00:00+00'
              )
          AND (first_unpromoted_observed_at,
               client_id, source_kind, interface) > ($2, $3, $4, $5)
          AND NOT (client_id = ANY($6::text[]))
        ORDER BY first_unpromoted_observed_at,
                 client_id, source_kind, interface
        LIMIT 1
    "#
}

fn raw_stream_resume_sql() -> &'static str {
    r#"
        SELECT client_id, source_kind, interface,
               'raw'::text AS lane,
               $6::timestamptz AS frontier_start,
               observed_at AS bucket_start
        FROM traffic_counter_samples
        WHERE client_id = $1
          AND source_kind = $2
          AND interface = $3
          AND observed_at >= $4
          AND observed_at <
              date_bin(
                  interval '1 hour', now() - make_interval(days => $5),
                  TIMESTAMPTZ '1970-01-01 00:00:00+00'
              )
          AND NOT inbound_promoted
        ORDER BY client_id, source_kind, interface, observed_at
        LIMIT 1
    "#
}

fn rollup_frontier_start_sql() -> &'static str {
    r#"
        SELECT client_id, source_kind, interface,
               origin_kind AS lane, bucket_start AS frontier_start,
               bucket_start
        FROM traffic_counter_rollups
        WHERE bucket_secs = $1
          AND bucket_start <=
              (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                  - make_interval(days => $2)
                  - make_interval(secs => $1)
          AND NOT (client_id = ANY($3::text[]))
        ORDER BY bucket_start, client_id, source_kind, interface, origin_kind
        LIMIT 1
    "#
}

fn rollup_frontier_after_sql() -> &'static str {
    r#"
        SELECT client_id, source_kind, interface,
               origin_kind AS lane, bucket_start AS frontier_start,
               bucket_start
        FROM traffic_counter_rollups
        WHERE bucket_secs = $1
          AND bucket_start <=
              (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                  - make_interval(days => $2)
                  - make_interval(secs => $1)
          AND (bucket_start, client_id, source_kind, interface, origin_kind)
              > ($3, $4, $5, $6, $7)
          AND NOT (client_id = ANY($8::text[]))
        ORDER BY bucket_start, client_id, source_kind, interface, origin_kind
        LIMIT 1
    "#
}

async fn find_raw_phase_candidate(
    tx: &mut Transaction<'_, Postgres>,
    cursor: &TrafficPhaseCursor,
    unavailable_clients: &[String],
) -> Result<Option<TrafficPhaseCandidate>> {
    let Some(stream) = cursor.stream.as_ref() else {
        record_frontier_query();
        return sqlx::query(raw_frontier_start_sql())
            .bind(TRAFFIC_COUNTER_RAW_RETENTION_DAYS)
            .bind(unavailable_clients)
            .fetch_optional(&mut **tx)
            .await?
            .map(phase_candidate_from_row)
            .transpose();
    };

    // A successful page advances by a complete destination bucket. Resume that
    // same stream first so a multi-page backlog cannot hide later raw buckets.
    // Destination conflicts abort and roll back; only a transient lock uses the
    // `raw_deferred` lane and gives another global stream one turn.
    if cursor.lane.as_deref() == Some("raw") && !unavailable_clients.contains(&stream.client_id) {
        let scan_after = cursor
            .scan_after
            .ok_or_else(|| anyhow::anyhow!("traffic raw cursor is missing its bucket"))?;
        let frontier_start = cursor
            .frontier_start
            .ok_or_else(|| anyhow::anyhow!("traffic raw cursor is missing its global frontier"))?;
        record_frontier_query();
        if let Some(row) = sqlx::query(raw_stream_resume_sql())
            .bind(&stream.client_id)
            .bind(&stream.source_kind)
            .bind(&stream.interface)
            .bind(scan_after)
            .bind(TRAFFIC_COUNTER_RAW_RETENTION_DAYS)
            .bind(frontier_start)
            .fetch_optional(&mut **tx)
            .await?
        {
            return phase_candidate_from_row(row).map(Some);
        }
    }

    // The raw cursor owns both dimensions: this registry timestamp is the
    // durable global keyset frontier, while `scan_after` above is the complete
    // destination-bucket successor within the current stream. Keeping them
    // separate lets a multi-page same-stream backlog resume across restarts while
    // transiently locked streams do not prevent other streams from progressing.
    let frontier_start = cursor
        .frontier_start
        .ok_or_else(|| anyhow::anyhow!("traffic raw cursor is missing its global frontier"))?;
    record_frontier_query();
    if let Some(row) = sqlx::query(raw_frontier_after_sql())
        .bind(TRAFFIC_COUNTER_RAW_RETENTION_DAYS)
        .bind(frontier_start)
        .bind(&stream.client_id)
        .bind(&stream.source_kind)
        .bind(&stream.interface)
        .bind(unavailable_clients)
        .fetch_optional(&mut **tx)
        .await?
    {
        return phase_candidate_from_row(row).map(Some);
    }
    record_frontier_query();
    sqlx::query(raw_frontier_start_sql())
        .bind(TRAFFIC_COUNTER_RAW_RETENTION_DAYS)
        .bind(unavailable_clients)
        .fetch_optional(&mut **tx)
        .await?
        .map(phase_candidate_from_row)
        .transpose()
}

async fn find_rollup_phase_candidate(
    tx: &mut Transaction<'_, Postgres>,
    cursor: &TrafficPhaseCursor,
    source_bucket_secs: i32,
    source_retention_days: i32,
    unavailable_clients: &[String],
) -> Result<Option<TrafficPhaseCandidate>> {
    let mut row = if let (Some(stream), Some(lane), Some(bucket_start)) = (
        cursor.stream.as_ref(),
        cursor.lane.as_deref(),
        cursor.scan_after,
    ) {
        record_frontier_query();
        sqlx::query(rollup_frontier_after_sql())
            .bind(source_bucket_secs)
            .bind(source_retention_days)
            .bind(bucket_start)
            .bind(&stream.client_id)
            .bind(&stream.source_kind)
            .bind(&stream.interface)
            .bind(lane)
            .bind(unavailable_clients)
            .fetch_optional(&mut **tx)
            .await?
    } else {
        record_frontier_query();
        sqlx::query(rollup_frontier_start_sql())
            .bind(source_bucket_secs)
            .bind(source_retention_days)
            .bind(unavailable_clients)
            .fetch_optional(&mut **tx)
            .await?
    };
    if row.is_none() && cursor.stream.is_some() {
        record_frontier_query();
        row = sqlx::query(rollup_frontier_start_sql())
            .bind(source_bucket_secs)
            .bind(source_retention_days)
            .bind(unavailable_clients)
            .fetch_optional(&mut **tx)
            .await?;
    }
    row.map(phase_candidate_from_row).transpose()
}

#[cfg(test)]
pub(crate) async fn reset_traffic_phase_cursors_for_test(pool: &PgPool) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE traffic_history_retention_cursors
        SET traffic_client_id = NULL,
            traffic_source_kind = NULL,
            traffic_interface = NULL,
            traffic_lane = NULL,
            traffic_frontier_start = NULL,
            traffic_scan_after = NULL,
            updated_at = clock_timestamp()
        WHERE domain = 'traffic_counter_samples'
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn try_lock_client_row_then_traffic(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
) -> Result<bool> {
    // Telemetry and vnStat replacement acquire the client row before this
    // advisory. Take the same FK-strength row lock first, without waiting, so a
    // retention INSERT can never create the inverse advisory -> client order.
    let key = format!("traffic-counters:{client_id}");
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        WITH client AS MATERIALIZED (
            SELECT id
            FROM clients
            WHERE id = $1
            FOR KEY SHARE SKIP LOCKED
        )
        SELECT CASE WHEN EXISTS (SELECT 1 FROM client)
            THEN pg_try_advisory_xact_lock(hashtextextended($2, 0))
            ELSE FALSE
        END
        "#,
    )
    .bind(client_id)
    .bind(key)
    .fetch_one(&mut **tx)
    .await?)
}

fn raw_promotion_sql() -> &'static str {
    r#"
        WITH requested AS MATERIALIZED (
            SELECT source_kind, interface
            FROM UNNEST($2::text[], $3::text[])
                AS stream(source_kind, interface)
        ), cutoff AS MATERIALIZED (
            SELECT
                (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                    AS today,
                date_bin(
                    interval '1 hour', now() - make_interval(days => $4),
                    TIMESTAMPTZ '1970-01-01 00:00:00+00'
                ) AS raw_cutoff
        ), seed_prefix AS MATERIALIZED (
            SELECT requested.source_kind, requested.interface,
                   seed.observed_at, seed.sample_source, seed.inbound_promoted
            FROM requested
            CROSS JOIN cutoff
            JOIN LATERAL (
                WITH seek AS MATERIALIZED (
                    SELECT sample.client_id, sample.source_kind,
                           sample.interface, sample.observed_at,
                           sample.sample_source, sample.inbound_promoted
                    FROM traffic_counter_samples sample
                    WHERE (sample.client_id, sample.source_kind,
                           sample.interface, sample.observed_at) >= (
                            $1, requested.source_kind, requested.interface,
                            COALESCE($8::timestamptz,
                                     '-infinity'::timestamptz)
                    )
                    ORDER BY sample.client_id, sample.source_kind,
                             sample.interface, sample.observed_at
                    LIMIT $7
                )
                SELECT seek.observed_at, seek.sample_source,
                       seek.inbound_promoted
                FROM seek
                WHERE seek.client_id = $1
                  AND seek.source_kind = requested.source_kind
                  AND seek.interface = requested.interface
                  AND seek.observed_at < cutoff.raw_cutoff
            ) seed ON TRUE
        ), classified_prefix AS MATERIALIZED (
            SELECT source_kind, interface, observed_at,
                CASE
                    WHEN observed_at >= cutoff.today - interval '91 days' THEN 3600
                    WHEN observed_at >= cutoff.today - interval '181 days' THEN 10800
                    WHEN observed_at >= cutoff.today - interval '366 days' THEN 21600
                    ELSE 86400
                END::integer AS destination_secs,
                date_bin(
                    make_interval(secs => CASE
                        WHEN observed_at >= cutoff.today - interval '91 days' THEN 3600
                        WHEN observed_at >= cutoff.today - interval '181 days' THEN 10800
                        WHEN observed_at >= cutoff.today - interval '366 days' THEN 21600
                        ELSE 86400
                    END),
                    observed_at,
                    TIMESTAMPTZ '1970-01-01 00:00:00+00'
                ) AS bucket_start
            FROM seed_prefix
            CROSS JOIN cutoff
            WHERE NOT inbound_promoted
        ), unique_units AS MATERIALIZED (
            SELECT DISTINCT ON (
                       source_kind, interface,
                       destination_secs, bucket_start
                   )
                   source_kind, interface, observed_at,
                   destination_secs, bucket_start
            FROM classified_prefix
            ORDER BY source_kind, interface,
                     destination_secs, bucket_start, observed_at
        ), unbudgeted_units AS MATERIALIZED (
            SELECT source_kind, interface, destination_secs, bucket_start
            FROM unique_units
            ORDER BY bucket_start, source_kind, interface
            LIMIT $5
        ), costed_units AS MATERIALIZED (
            SELECT units.*,
                   units.destination_secs::bigint / 60 + 1 AS maximum_rows,
                   sum(units.destination_secs::bigint / 60 + 1) OVER (
                       ORDER BY units.bucket_start, units.source_kind,
                                units.interface
                   ) AS running_rows
            FROM unbudgeted_units units
        ), candidate_units AS MATERIALIZED (
            SELECT *
            FROM costed_units
            WHERE running_rows <= $6
        ), expanded_range AS MATERIALIZED (
            SELECT units.source_kind, units.interface,
                   units.destination_secs, units.bucket_start,
                   units.maximum_rows,
                   source.source_ctid, source.observed_at,
                   source.rx_bytes, source.tx_bytes,
                   source.rx_counter_epoch, source.tx_counter_epoch,
                   source.sample_count, source.rx_bytes_sum,
                   source.tx_bytes_sum, source.latest_observed_at,
                   source.rx_usage_bytes, source.tx_usage_bytes,
                   source.rx_valid_count, source.tx_valid_count,
                   source.any_valid_count,
                   source.rx_reset_count, source.tx_reset_count,
                   source.any_reset_count, source.usage_authoritative,
                   source.updated_at,
                   source.sample_source, source.inbound_promoted,
                   CASE WHEN source.sample_source LIKE 'vnstat_import:%'
                        THEN 'vnstat_import' ELSE 'live' END AS row_origin_kind
            FROM candidate_units units
            JOIN LATERAL (
                WITH seek AS MATERIALIZED (
                    SELECT sample.ctid AS source_ctid, sample.client_id,
                           sample.source_kind, sample.interface,
                           sample.observed_at, sample.rx_bytes, sample.tx_bytes,
                           sample.rx_counter_epoch, sample.tx_counter_epoch,
                           sample.sample_count, sample.rx_bytes_sum,
                           sample.tx_bytes_sum, sample.latest_observed_at,
                           sample.rx_usage_bytes, sample.tx_usage_bytes,
                           sample.rx_valid_count, sample.tx_valid_count,
                           sample.any_valid_count,
                           sample.rx_reset_count, sample.tx_reset_count,
                           sample.any_reset_count,
                           sample.usage_authoritative,
                           sample.updated_at,
                           sample.sample_source, sample.inbound_promoted
                    FROM traffic_counter_samples sample
                    WHERE (sample.client_id, sample.source_kind,
                           sample.interface, sample.observed_at) >= (
                            $1, units.source_kind, units.interface,
                            units.bucket_start
                    )
                    ORDER BY sample.client_id, sample.source_kind,
                             sample.interface, sample.observed_at
                    LIMIT units.maximum_rows
                )
                SELECT seek.source_ctid, seek.observed_at,
                       seek.rx_bytes, seek.tx_bytes,
                       seek.rx_counter_epoch, seek.tx_counter_epoch,
                       seek.sample_count, seek.rx_bytes_sum,
                       seek.tx_bytes_sum, seek.latest_observed_at,
                       seek.rx_usage_bytes, seek.tx_usage_bytes,
                       seek.rx_valid_count, seek.tx_valid_count,
                       seek.any_valid_count,
                       seek.rx_reset_count, seek.tx_reset_count,
                       seek.any_reset_count, seek.usage_authoritative,
                       seek.updated_at,
                       seek.sample_source, seek.inbound_promoted
                FROM seek
                WHERE seek.client_id = $1
                  AND seek.source_kind = units.source_kind
                  AND seek.interface = units.interface
                  AND seek.observed_at < units.bucket_start
                        + make_interval(secs => units.destination_secs)
            ) source ON TRUE
        ), predecessors AS MATERIALIZED (
            SELECT units.source_kind, units.interface,
                   units.destination_secs, units.bucket_start,
                   units.maximum_rows,
                   predecessor.source_ctid, predecessor.observed_at,
                   predecessor.rx_bytes, predecessor.tx_bytes,
                   predecessor.rx_counter_epoch, predecessor.tx_counter_epoch,
                   predecessor.sample_count, predecessor.rx_bytes_sum,
                   predecessor.tx_bytes_sum, predecessor.latest_observed_at,
                   predecessor.rx_usage_bytes, predecessor.tx_usage_bytes,
                   predecessor.rx_valid_count, predecessor.tx_valid_count,
                   predecessor.any_valid_count,
                   predecessor.rx_reset_count, predecessor.tx_reset_count,
                   predecessor.any_reset_count,
                   predecessor.usage_authoritative,
                   predecessor.updated_at,
                   predecessor.sample_source, predecessor.inbound_promoted,
                   CASE WHEN predecessor.sample_source LIKE 'vnstat_import:%'
                        THEN 'vnstat_import' ELSE 'live' END AS row_origin_kind
            FROM candidate_units units
            JOIN LATERAL (
                WITH seek AS MATERIALIZED (
                    SELECT sample.ctid AS source_ctid, sample.client_id,
                           sample.source_kind, sample.interface,
                           sample.observed_at, sample.rx_bytes, sample.tx_bytes,
                           sample.rx_counter_epoch, sample.tx_counter_epoch,
                           sample.sample_count, sample.rx_bytes_sum,
                           sample.tx_bytes_sum, sample.latest_observed_at,
                           sample.rx_usage_bytes, sample.tx_usage_bytes,
                           sample.rx_valid_count, sample.tx_valid_count,
                           sample.any_valid_count,
                           sample.rx_reset_count, sample.tx_reset_count,
                           sample.any_reset_count,
                           sample.usage_authoritative,
                           sample.updated_at,
                           sample.sample_source, sample.inbound_promoted
                    FROM traffic_counter_samples sample
                    WHERE (sample.client_id, sample.source_kind,
                           sample.interface, sample.observed_at) < (
                            $1, units.source_kind, units.interface,
                            units.bucket_start
                    )
                    ORDER BY sample.client_id DESC, sample.source_kind DESC,
                             sample.interface DESC, sample.observed_at DESC
                    LIMIT 1
                )
                SELECT seek.source_ctid, seek.observed_at,
                       seek.rx_bytes, seek.tx_bytes,
                       seek.rx_counter_epoch, seek.tx_counter_epoch,
                       seek.sample_count, seek.rx_bytes_sum,
                       seek.tx_bytes_sum, seek.latest_observed_at,
                       seek.rx_usage_bytes, seek.tx_usage_bytes,
                       seek.rx_valid_count, seek.tx_valid_count,
                       seek.any_valid_count,
                       seek.rx_reset_count, seek.tx_reset_count,
                       seek.any_reset_count, seek.usage_authoritative,
                       seek.updated_at,
                       seek.sample_source, seek.inbound_promoted
                FROM seek
                WHERE seek.client_id = $1
                  AND seek.source_kind = units.source_kind
                  AND seek.interface = units.interface
                  AND seek.observed_at < units.bucket_start
            ) predecessor ON TRUE
        ), sequencing_rows AS MATERIALIZED (
            SELECT expanded_range.*, TRUE AS in_range
            FROM expanded_range
            UNION ALL
            SELECT predecessors.*, FALSE AS in_range
            FROM predecessors
        ), sequenced AS MATERIALIZED (
            SELECT sequencing_rows.*,
                lag(rx_bytes) OVER stream AS previous_rx_bytes,
                lag(tx_bytes) OVER stream AS previous_tx_bytes,
                lag(rx_counter_epoch) OVER stream AS previous_rx_counter_epoch,
                lag(tx_counter_epoch) OVER stream AS previous_tx_counter_epoch,
                lag(sample_source) OVER stream AS previous_sample_source
            FROM sequencing_rows
            WINDOW stream AS (
                PARTITION BY source_kind, interface,
                             destination_secs, bucket_start
                ORDER BY observed_at
            )
        ), unit_state AS MATERIALIZED (
            SELECT units.source_kind, units.interface,
                   units.destination_secs, units.bucket_start,
                   units.maximum_rows,
                   count(sequencing_rows.source_ctid) FILTER (
                       WHERE sequencing_rows.in_range
                   )::bigint AS range_rows,
                   count(sequencing_rows.source_ctid) FILTER (
                       WHERE sequencing_rows.in_range
                         AND NOT sequencing_rows.inbound_promoted
                   )::bigint AS expected_rows,
                   count(sequencing_rows.source_ctid) FILTER (
                       WHERE sequencing_rows.inbound_promoted
                   )::bigint AS boundary_rows
            FROM candidate_units units
            LEFT JOIN sequencing_rows USING (
                source_kind, interface,
                destination_secs, bucket_start, maximum_rows
            )
            GROUP BY units.source_kind, units.interface,
                     units.destination_secs, units.bucket_start,
                     units.maximum_rows
        ), eligible_units AS MATERIALIZED (
            SELECT *
            FROM unit_state
            WHERE range_rows < maximum_rows
              AND expected_rows > 0
              AND boundary_rows <= 1
        ), candidate_rows AS MATERIALIZED (
            SELECT sequenced.*
            FROM sequenced
            JOIN eligible_units units USING (
                source_kind, interface,
                destination_secs, bucket_start, maximum_rows
            )
            WHERE sequenced.in_range
              AND NOT sequenced.inbound_promoted
        ), origin_groups AS MATERIALIZED (
            SELECT source_kind, interface, destination_secs, bucket_start,
                   row_origin_kind AS origin_kind, count(*)::bigint AS origin_rows
            FROM candidate_rows
            GROUP BY source_kind, interface, destination_secs, bucket_start,
                     row_origin_kind
        ), destination_conflicts AS MATERIALIZED (
            SELECT DISTINCT groups.source_kind, groups.interface,
                   groups.destination_secs, groups.bucket_start
            FROM origin_groups groups
            WHERE EXISTS (
                WITH seek AS MATERIALIZED (
                    SELECT destination.client_id, destination.source_kind,
                           destination.interface, destination.origin_kind,
                           destination.bucket_secs, destination.bucket_start
                    FROM traffic_counter_rollups destination
                    WHERE (destination.client_id, destination.source_kind,
                           destination.interface, destination.origin_kind,
                           destination.bucket_secs,
                           destination.bucket_start) >= (
                            $1, groups.source_kind, groups.interface,
                            groups.origin_kind, groups.destination_secs,
                            groups.bucket_start
                    )
                    ORDER BY destination.client_id, destination.source_kind,
                             destination.interface, destination.origin_kind,
                             destination.bucket_secs, destination.bucket_start
                    LIMIT 1
                )
                SELECT 1
                FROM seek
                WHERE seek.client_id = $1
                  AND seek.source_kind = groups.source_kind
                  AND seek.interface = groups.interface
                  AND seek.origin_kind = groups.origin_kind
                  AND seek.bucket_secs = groups.destination_secs
                  AND seek.bucket_start = groups.bucket_start
            )
        ), lockable_units AS MATERIALIZED (
            SELECT units.*
            FROM eligible_units units
            WHERE NOT EXISTS (
                SELECT 1
                FROM destination_conflicts conflict
                WHERE conflict.source_kind = units.source_kind
                  AND conflict.interface = units.interface
                  AND conflict.destination_secs = units.destination_secs
                  AND conflict.bucket_start = units.bucket_start
            )
        ), boundary_targets AS MATERIALIZED (
            SELECT DISTINCT ON (
                       sequencing.source_kind, sequencing.interface,
                       sequencing.destination_secs, sequencing.bucket_start
                   )
                   sequencing.source_kind, sequencing.interface,
                   sequencing.destination_secs, sequencing.bucket_start,
                   sequencing.source_ctid
            FROM sequencing_rows sequencing
            JOIN lockable_units USING (
                source_kind, interface, destination_secs,
                bucket_start, maximum_rows
            )
            WHERE sequencing.inbound_promoted
            ORDER BY sequencing.source_kind, sequencing.interface,
                     sequencing.destination_secs, sequencing.bucket_start,
                     sequencing.observed_at DESC
        ), lock_targets AS MATERIALIZED (
            SELECT rows.source_kind, rows.interface,
                   rows.destination_secs, rows.bucket_start,
                   rows.source_ctid,
                   FALSE AS is_boundary
            FROM candidate_rows rows
            JOIN lockable_units USING (
                source_kind, interface, destination_secs,
                bucket_start, maximum_rows
            )
            UNION ALL
            SELECT source_kind, interface,
                   destination_secs, bucket_start, source_ctid,
                   TRUE AS is_boundary
            FROM boundary_targets
        ), locked_targets AS MATERIALIZED (
            SELECT targets.*
            FROM lock_targets targets
            JOIN traffic_counter_samples source
              ON source.ctid = targets.source_ctid
            ORDER BY targets.bucket_start, targets.source_kind,
                     targets.interface, targets.source_ctid
            FOR UPDATE OF source SKIP LOCKED
        ), complete_units AS MATERIALIZED (
            SELECT units.source_kind, units.interface,
                   units.destination_secs, units.bucket_start
            FROM lockable_units units
            LEFT JOIN locked_targets targets USING (
                source_kind, interface,
                destination_secs, bucket_start
            )
            GROUP BY units.source_kind, units.interface,
                     units.destination_secs, units.bucket_start,
                     units.expected_rows
            HAVING count(targets.source_ctid) FILTER (
                       WHERE NOT targets.is_boundary
                   ) = units.expected_rows
               AND count(targets.source_ctid) FILTER (
                       WHERE targets.is_boundary
                   ) = (
                       SELECT count(*)
                       FROM boundary_targets boundary
                       WHERE boundary.source_kind = units.source_kind
                         AND boundary.interface = units.interface
                         AND boundary.destination_secs = units.destination_secs
                         AND boundary.bucket_start = units.bucket_start
                   )
        ), first_lock_hole AS MATERIALIZED (
            SELECT min(units.bucket_start) AS bucket_start
            FROM lockable_units units
            LEFT JOIN complete_units complete USING (
                source_kind, interface, destination_secs, bucket_start
            )
            WHERE complete.bucket_start IS NULL
        ), safe_complete_units AS MATERIALIZED (
            SELECT complete.*
            FROM complete_units complete
            CROSS JOIN first_lock_hole hole
            WHERE hole.bucket_start IS NULL
               OR complete.bucket_start < hole.bucket_start
        ), locked AS MATERIALIZED (
            SELECT candidate_rows.*
            FROM candidate_rows
            JOIN safe_complete_units safe USING (
                source_kind, interface, destination_secs, bucket_start
            )
            JOIN locked_targets targets
             ON targets.source_ctid = candidate_rows.source_ctid
             AND targets.source_kind = candidate_rows.source_kind
             AND targets.interface = candidate_rows.interface
             AND targets.destination_secs = candidate_rows.destination_secs
             AND targets.bucket_start = candidate_rows.bucket_start
             AND NOT targets.is_boundary
        ), aggregated AS MATERIALIZED (
            SELECT
                $1::text AS client_id,
                locked.source_kind,
                locked.interface,
                locked.row_origin_kind AS origin_kind,
                locked.destination_secs AS bucket_secs,
                locked.bucket_start,
                COALESCE(sum(CASE
                    WHEN usage_authoritative THEN rx_usage_bytes
                    WHEN rx_counter_epoch = previous_rx_counter_epoch
                     AND rx_bytes >= previous_rx_bytes
                    THEN rx_bytes - previous_rx_bytes ELSE 0 END), 0)::bigint
                    AS rx_bytes,
                COALESCE(sum(CASE
                    WHEN usage_authoritative THEN tx_usage_bytes
                    WHEN tx_counter_epoch = previous_tx_counter_epoch
                     AND tx_bytes >= previous_tx_bytes
                    THEN tx_bytes - previous_tx_bytes ELSE 0 END), 0)::bigint
                    AS tx_bytes,
                LEAST(sum(CASE
                    WHEN usage_authoritative THEN rx_valid_count
                    WHEN rx_counter_epoch = previous_rx_counter_epoch
                     AND rx_bytes >= previous_rx_bytes
                    THEN 1 ELSE 0 END), 2147483647)::integer
                    AS rx_valid_count,
                LEAST(sum(CASE
                    WHEN usage_authoritative THEN tx_valid_count
                    WHEN tx_counter_epoch = previous_tx_counter_epoch
                     AND tx_bytes >= previous_tx_bytes
                    THEN 1 ELSE 0 END), 2147483647)::integer
                    AS tx_valid_count,
                LEAST(sum(CASE
                    WHEN usage_authoritative THEN any_valid_count
                    WHEN (rx_counter_epoch = previous_rx_counter_epoch
                          AND rx_bytes >= previous_rx_bytes)
                      OR (tx_counter_epoch = previous_tx_counter_epoch
                          AND tx_bytes >= previous_tx_bytes)
                    THEN 1 ELSE 0 END), 2147483647)::integer
                    AS any_valid_count,
                LEAST(sum(CASE
                    WHEN usage_authoritative THEN rx_reset_count
                    WHEN previous_rx_counter_epoch IS NOT NULL
                     AND rx_counter_epoch <> previous_rx_counter_epoch
                     AND NOT (previous_sample_source LIKE 'vnstat_import:%'
                              AND sample_source NOT LIKE 'vnstat_import:%')
                    THEN 1 ELSE 0 END), 2147483647)::integer
                    AS rx_reset_count,
                LEAST(sum(CASE
                    WHEN usage_authoritative THEN tx_reset_count
                    WHEN previous_tx_counter_epoch IS NOT NULL
                     AND tx_counter_epoch <> previous_tx_counter_epoch
                     AND NOT (previous_sample_source LIKE 'vnstat_import:%'
                              AND sample_source NOT LIKE 'vnstat_import:%')
                    THEN 1 ELSE 0 END), 2147483647)::integer
                    AS tx_reset_count,
                LEAST(sum(CASE
                    WHEN usage_authoritative THEN any_reset_count
                    WHEN previous_rx_counter_epoch IS NOT NULL
                     AND (rx_counter_epoch <> previous_rx_counter_epoch
                          OR tx_counter_epoch <> previous_tx_counter_epoch)
                     AND NOT (previous_sample_source LIKE 'vnstat_import:%'
                              AND sample_source NOT LIKE 'vnstat_import:%')
                    THEN 1 ELSE 0 END), 2147483647)::integer
                    AS any_reset_count,
                min(observed_at) AS first_observed_at,
                max(observed_at) AS latest_observed_at
            FROM locked
            JOIN safe_complete_units USING (
                source_kind, interface,
                destination_secs, bucket_start
            )
            GROUP BY locked.source_kind, locked.interface,
                     locked.row_origin_kind, locked.destination_secs,
                     locked.bucket_start
        ), inserted AS (
            INSERT INTO traffic_counter_rollups (
                client_id, source_kind, interface, origin_kind,
                bucket_secs, bucket_start, rx_bytes, tx_bytes,
                rx_valid_count, tx_valid_count, any_valid_count,
                rx_reset_count, tx_reset_count, any_reset_count,
                first_observed_at, latest_observed_at
            )
            SELECT
                client_id, source_kind, interface, origin_kind,
                bucket_secs, bucket_start, rx_bytes, tx_bytes,
                rx_valid_count, tx_valid_count, any_valid_count,
                rx_reset_count, tx_reset_count, any_reset_count,
                first_observed_at, latest_observed_at
            FROM aggregated
            ON CONFLICT DO NOTHING
            RETURNING source_kind, interface, origin_kind,
                      bucket_secs, bucket_start
        ), insert_state AS MATERIALIZED (
            SELECT aggregated.source_kind, aggregated.interface,
                   aggregated.bucket_secs AS destination_secs,
                   aggregated.bucket_start,
                   count(*)::bigint AS expected_origins,
                   count(inserted.origin_kind)::bigint AS inserted_origins
            FROM aggregated
            LEFT JOIN inserted
              ON inserted.source_kind = aggregated.source_kind
             AND inserted.interface = aggregated.interface
             AND inserted.origin_kind = aggregated.origin_kind
             AND inserted.bucket_secs = aggregated.bucket_secs
             AND inserted.bucket_start = aggregated.bucket_start
            GROUP BY aggregated.source_kind, aggregated.interface,
                     aggregated.bucket_secs, aggregated.bucket_start
        ), successful_units AS MATERIALIZED (
            SELECT source_kind, interface, destination_secs, bucket_start
            FROM insert_state
            WHERE inserted_origins = expected_origins
        ), traffic_insert_race_conflicts AS MATERIALIZED (
            SELECT source_kind, interface, destination_secs, bucket_start
            FROM insert_state
            WHERE inserted_origins <> expected_origins
        ), network_transfer_candidates AS MATERIALIZED (
            SELECT
                $1::text AS client_id,
                locked.source_kind,
                locked.interface,
                locked.destination_secs,
                locked.bucket_start AS traffic_bucket_start,
                locked.observed_at AS network_bucket_start,
                locked.sample_count,
                locked.rx_bytes_sum,
                locked.tx_bytes_sum,
                round(locked.rx_bytes_sum / locked.sample_count::numeric)::bigint
                    AS rx_bytes_avg,
                round(locked.tx_bytes_sum / locked.sample_count::numeric)::bigint
                    AS tx_bytes_avg,
                locked.rx_bytes AS rx_bytes_last,
                locked.tx_bytes AS tx_bytes_last,
                locked.rx_counter_epoch,
                locked.tx_counter_epoch,
                locked.latest_observed_at,
                locked.updated_at
            FROM locked
            JOIN successful_units USING (
                source_kind, interface, destination_secs, bucket_start
            )
            WHERE locked.source_kind = 'host'
        ), network_transferred AS (
            INSERT INTO telemetry_network_rates (
                client_id, interface, bucket_start, bucket_secs,
                sample_count, rx_bytes_sum, tx_bytes_sum,
                rx_bytes_avg, tx_bytes_avg, rx_bytes_last, tx_bytes_last,
                rx_counter_epoch, tx_counter_epoch,
                latest_observed_at, updated_at
            )
            SELECT
                client_id, interface, network_bucket_start, 60,
                sample_count, rx_bytes_sum, tx_bytes_sum,
                rx_bytes_avg, tx_bytes_avg, rx_bytes_last, tx_bytes_last,
                rx_counter_epoch, tx_counter_epoch,
                latest_observed_at, updated_at
            FROM network_transfer_candidates
            ORDER BY interface, network_bucket_start
            ON CONFLICT DO NOTHING
            RETURNING client_id, interface, bucket_start
        ), network_transfer_state AS MATERIALIZED (
            SELECT
                candidate.source_kind,
                candidate.interface,
                candidate.destination_secs,
                candidate.traffic_bucket_start AS bucket_start,
                count(*)::bigint AS expected_rows,
                count(transferred.client_id)::bigint AS inserted_rows
            FROM network_transfer_candidates candidate
            LEFT JOIN network_transferred transferred
              ON transferred.client_id = candidate.client_id
             AND transferred.interface = candidate.interface
             AND transferred.bucket_start = candidate.network_bucket_start
            GROUP BY candidate.source_kind, candidate.interface,
                     candidate.destination_secs,
                     candidate.traffic_bucket_start
        ), network_insert_race_conflicts AS MATERIALIZED (
            SELECT source_kind, interface, destination_secs, bucket_start
            FROM network_transfer_state
            WHERE inserted_rows <> expected_rows
        ), fully_successful_units AS MATERIALIZED (
            SELECT successful.*
            FROM successful_units successful
            WHERE NOT EXISTS (
                SELECT 1
                FROM network_insert_race_conflicts conflict
                WHERE conflict.source_kind = successful.source_kind
                  AND conflict.interface = successful.interface
                  AND conflict.destination_secs = successful.destination_secs
                  AND conflict.bucket_start = successful.bucket_start
            )
        ), promoted_rows AS MATERIALIZED (
            SELECT locked.*
            FROM locked
            JOIN fully_successful_units USING (
                source_kind, interface, destination_secs, bucket_start
            )
        ), promoted_boundaries AS MATERIALIZED (
            -- Several adjacent units may be promoted in one bounded page.
            -- Their intermediate endpoints have already contributed to the
            -- following aggregate, so only the newest endpoint per stream is
            -- still required to sequence the remaining exact tail.
            SELECT DISTINCT ON (source_kind, interface)
                   source_kind, interface, source_ctid
            FROM promoted_rows
            ORDER BY source_kind, interface, observed_at DESC
        ), locked_prior_boundaries AS MATERIALIZED (
            SELECT targets.source_kind, targets.interface,
                   targets.destination_secs,
                   targets.bucket_start, targets.source_ctid
            FROM locked_targets targets
            JOIN fully_successful_units USING (
                source_kind, interface,
                destination_secs, bucket_start
            )
            WHERE targets.is_boundary
        ), marked_boundary AS (
            UPDATE traffic_counter_samples source
            SET inbound_promoted = TRUE
            FROM promoted_boundaries boundary
            WHERE source.ctid = boundary.source_ctid
            RETURNING source.ctid
        ), deleted_new AS (
            DELETE FROM traffic_counter_samples source
            USING promoted_rows promoted, promoted_boundaries boundary
            WHERE source.ctid = promoted.source_ctid
              AND boundary.source_kind = promoted.source_kind
              AND boundary.interface = promoted.interface
              AND promoted.source_ctid <> boundary.source_ctid
            RETURNING source.client_id, source.source_kind,
                      source.interface, source.observed_at
        ), deleted_accounted AS (
            DELETE FROM traffic_counter_samples source
            USING locked_prior_boundaries boundary
            WHERE source.ctid = boundary.source_ctid
            RETURNING source.client_id, source.source_kind,
                      source.interface, source.observed_at
        ), overflow_conflicts AS MATERIALIZED (
            SELECT source_kind, interface,
                   destination_secs, bucket_start
            FROM unit_state
            WHERE range_rows >= maximum_rows OR boundary_rows > 1
        ), deleted_samples AS MATERIALIZED (
            SELECT * FROM deleted_new
            UNION ALL
            SELECT * FROM deleted_accounted
        ), affected_hours AS MATERIALIZED (
            SELECT DISTINCT client_id, source_kind, interface,
                   date_bin(
                       interval '1 hour', observed_at,
                       TIMESTAMPTZ '1970-01-01 00:00:00+00'
                   ) AS observed_at
            FROM deleted_samples
        ), cursor_advance AS MATERIALIZED (
            SELECT max(
                       units.bucket_start
                           + make_interval(secs => units.destination_secs)
                   ) AS scan_after
            FROM candidate_units units
            CROSS JOIN first_lock_hole hole
            WHERE hole.bucket_start IS NULL
               OR units.bucket_start < hole.bucket_start
        )
        SELECT
            ((SELECT count(*) FROM deleted_new)
                + (SELECT count(*) FROM deleted_accounted))::bigint AS deleted_rows,
            ((SELECT count(*) FROM overflow_conflicts)
                + (SELECT count(*) FROM destination_conflicts)
                + (SELECT count(*) FROM traffic_insert_race_conflicts)
                + (SELECT count(*) FROM network_insert_race_conflicts))::bigint AS conflicts,
            ((SELECT count(*) FROM traffic_insert_race_conflicts)
                + (SELECT count(*) FROM network_insert_race_conflicts))::bigint
                AS insert_race_conflicts,
            COALESCE((SELECT array_agg(client_id ORDER BY client_id,
                        source_kind, interface, observed_at)
                      FROM affected_hours), ARRAY[]::text[])
                AS hourly_client_ids,
            COALESCE((SELECT array_agg(source_kind ORDER BY client_id,
                        source_kind, interface, observed_at)
                      FROM affected_hours), ARRAY[]::text[])
                AS hourly_source_kinds,
            COALESCE((SELECT array_agg(interface ORDER BY client_id,
                        source_kind, interface, observed_at)
                      FROM affected_hours), ARRAY[]::text[])
                AS hourly_interfaces,
            COALESCE((SELECT array_agg(observed_at ORDER BY client_id,
                        source_kind, interface, observed_at)
                      FROM affected_hours), ARRAY[]::timestamptz[])
                AS hourly_observed_at,
            (SELECT count(*)::bigint FROM network_transferred)
                AS network_rate_rows_published,
            (SELECT scan_after FROM cursor_advance) AS next_scan_after
    "#
}

fn rollup_promotion_sql() -> &'static str {
    r#"
        WITH requested AS MATERIALIZED (
            SELECT source_kind, interface
            FROM UNNEST($2::text[], $3::text[])
                AS stream(source_kind, interface)
        ), cutoff AS MATERIALIZED (
            SELECT (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                - make_interval(days => $7) AS value
        ), origins(origin_kind) AS (
            SELECT origin_kind
            FROM (VALUES ('live'::text), ('vnstat_import'::text))
                AS available(origin_kind)
            WHERE $10::text IS NULL OR origin_kind = $10
        ), immediate_predecessor_seeds AS MATERIALIZED (
            SELECT requested.source_kind, requested.interface,
                   origins.origin_kind, seed.bucket_start
            FROM requested
            CROSS JOIN origins
            CROSS JOIN cutoff
            JOIN LATERAL (
                WITH seek AS MATERIALIZED (
                    SELECT source.client_id, source.source_kind,
                           source.interface, source.origin_kind,
                           source.bucket_secs, source.bucket_start
                    FROM traffic_counter_rollups source
                    WHERE (source.client_id, source.source_kind,
                           source.interface, source.origin_kind,
                           source.bucket_secs, source.bucket_start) >= (
                            $1, requested.source_kind, requested.interface,
                            origins.origin_kind, $6,
                            COALESCE($11::timestamptz,
                                     '-infinity'::timestamptz)
                    )
                    ORDER BY source.client_id, source.source_kind,
                             source.interface, source.origin_kind,
                             source.bucket_secs, source.bucket_start
                    LIMIT LEAST($9, $8 * ($5 / $6))
                )
                SELECT seek.bucket_start
                FROM seek
                WHERE seek.client_id = $1
                  AND seek.source_kind = requested.source_kind
                  AND seek.interface = requested.interface
                  AND seek.origin_kind = origins.origin_kind
                  AND seek.bucket_secs = $6
                  AND seek.bucket_start <= cutoff.value
                        - make_interval(secs => $6)
            ) seed ON TRUE
        ), unbudgeted_groups AS MATERIALIZED (
            SELECT DISTINCT source_kind, interface, origin_kind,
                   date_bin(
                       make_interval(secs => $5), bucket_start,
                       TIMESTAMPTZ '1970-01-01 00:00:00+00'
                   ) AS destination_start
            FROM immediate_predecessor_seeds
            ORDER BY destination_start, source_kind, interface, origin_kind
            LIMIT $8
        ), costed_groups AS MATERIALIZED (
            SELECT groups.*,
                   source_budget.maximum_rows,
                   sum(source_budget.maximum_rows) OVER (
                       ORDER BY groups.destination_start, groups.source_kind,
                                groups.interface, groups.origin_kind
                   ) AS running_rows
            FROM unbudgeted_groups groups
            CROSS JOIN LATERAL (
                SELECT (sum($5::bigint / source_secs) + 1)::bigint
                    AS maximum_rows
                FROM UNNEST($4::integer[]) source(source_secs)
            ) source_budget
        ), candidate_groups AS MATERIALIZED (
            SELECT *
            FROM costed_groups
            WHERE running_rows <= $9
        ), expanded_range AS MATERIALIZED (
            SELECT groups.source_kind, groups.interface, groups.origin_kind,
                   groups.destination_start, groups.maximum_rows,
                   source.source_ctid, source.bucket_secs,
                   source.bucket_start, source.rx_bytes, source.tx_bytes,
                   source.rx_valid_count, source.tx_valid_count,
                   source.any_valid_count, source.rx_reset_count,
                   source.tx_reset_count, source.any_reset_count,
                   source.first_observed_at, source.latest_observed_at
            FROM candidate_groups groups
            JOIN LATERAL (
                SELECT tier_source.*
                FROM UNNEST($4::integer[]) tier(bucket_secs)
                CROSS JOIN LATERAL (
                    WITH seek AS MATERIALIZED (
                        SELECT source.ctid AS source_ctid, source.client_id,
                               source.source_kind, source.interface,
                               source.origin_kind, source.bucket_secs,
                               source.bucket_start, source.rx_bytes,
                               source.tx_bytes, source.rx_valid_count,
                               source.tx_valid_count, source.any_valid_count,
                               source.rx_reset_count, source.tx_reset_count,
                               source.any_reset_count, source.first_observed_at,
                               source.latest_observed_at
                        FROM traffic_counter_rollups source
                        WHERE (source.client_id, source.source_kind,
                               source.interface, source.origin_kind,
                               source.bucket_secs, source.bucket_start) >= (
                                $1, groups.source_kind, groups.interface,
                                groups.origin_kind, tier.bucket_secs,
                                groups.destination_start
                        )
                        ORDER BY source.client_id, source.source_kind,
                                 source.interface, source.origin_kind,
                                 source.bucket_secs, source.bucket_start
                        LIMIT ($5 / tier.bucket_secs)
                    )
                    SELECT seek.source_ctid, seek.bucket_secs,
                           seek.bucket_start, seek.rx_bytes, seek.tx_bytes,
                           seek.rx_valid_count, seek.tx_valid_count,
                           seek.any_valid_count, seek.rx_reset_count,
                           seek.tx_reset_count, seek.any_reset_count,
                           seek.first_observed_at, seek.latest_observed_at
                    FROM seek
                    WHERE seek.client_id = $1
                      AND seek.source_kind = groups.source_kind
                      AND seek.interface = groups.interface
                      AND seek.origin_kind = groups.origin_kind
                      AND seek.bucket_secs = tier.bucket_secs
                      AND seek.bucket_start <= groups.destination_start
                            + make_interval(secs => $5 - tier.bucket_secs)
                ) tier_source
                ORDER BY tier_source.bucket_start, tier_source.bucket_secs
                LIMIT groups.maximum_rows
            ) source ON TRUE
        ), group_state AS MATERIALIZED (
            SELECT groups.source_kind, groups.interface, groups.origin_kind,
                   groups.destination_start, groups.maximum_rows,
                   count(expanded.source_ctid)::bigint AS expected_rows
            FROM candidate_groups groups
            LEFT JOIN expanded_range expanded USING (
                source_kind, interface, origin_kind,
                destination_start, maximum_rows
            )
            GROUP BY groups.source_kind, groups.interface, groups.origin_kind,
                     groups.destination_start, groups.maximum_rows
        ), overflow_groups AS MATERIALIZED (
            SELECT source_kind, interface, origin_kind, destination_start
            FROM group_state
            WHERE expected_rows >= maximum_rows
        ), bounded_groups AS MATERIALIZED (
            SELECT *
            FROM group_state
            WHERE expected_rows > 0
              AND expected_rows < maximum_rows
        ), destination_conflicts AS MATERIALIZED (
            SELECT groups.source_kind, groups.interface, groups.origin_kind,
                   groups.destination_start
            FROM bounded_groups groups
            WHERE EXISTS (
                WITH seek AS MATERIALIZED (
                    SELECT destination.client_id, destination.source_kind,
                           destination.interface, destination.origin_kind,
                           destination.bucket_secs, destination.bucket_start
                    FROM traffic_counter_rollups destination
                    WHERE (destination.client_id, destination.source_kind,
                           destination.interface, destination.origin_kind,
                           destination.bucket_secs,
                           destination.bucket_start) >= (
                            $1, groups.source_kind, groups.interface,
                            groups.origin_kind, $5,
                            groups.destination_start
                    )
                    ORDER BY destination.client_id, destination.source_kind,
                             destination.interface, destination.origin_kind,
                             destination.bucket_secs, destination.bucket_start
                    LIMIT 1
                )
                SELECT 1
                FROM seek
                WHERE seek.client_id = $1
                  AND seek.source_kind = groups.source_kind
                  AND seek.interface = groups.interface
                  AND seek.origin_kind = groups.origin_kind
                  AND seek.bucket_secs = $5
                  AND seek.bucket_start = groups.destination_start
            )
        ), lockable_groups AS MATERIALIZED (
            SELECT groups.*
            FROM bounded_groups groups
            WHERE NOT EXISTS (
                SELECT 1
                FROM destination_conflicts conflict
                WHERE conflict.source_kind = groups.source_kind
                  AND conflict.interface = groups.interface
                  AND conflict.origin_kind = groups.origin_kind
                  AND conflict.destination_start = groups.destination_start
            )
        ), candidate_rows AS MATERIALIZED (
            SELECT expanded.*, groups.expected_rows
            FROM expanded_range expanded
            JOIN lockable_groups groups USING (
                source_kind, interface, origin_kind,
                destination_start, maximum_rows
            )
        ), locked AS MATERIALIZED (
            SELECT candidate_rows.*
            FROM candidate_rows
            JOIN traffic_counter_rollups source
              ON source.ctid = candidate_rows.source_ctid
            ORDER BY candidate_rows.destination_start,
                     candidate_rows.source_kind, candidate_rows.interface,
                     candidate_rows.origin_kind, candidate_rows.bucket_start,
                     candidate_rows.bucket_secs
            FOR UPDATE OF source SKIP LOCKED
        ), complete_groups AS MATERIALIZED (
            SELECT source_kind, interface, origin_kind, destination_start
            FROM locked
            GROUP BY source_kind, interface, origin_kind,
                     destination_start, expected_rows
            HAVING count(*) = expected_rows
        ), ordered_locked AS MATERIALIZED (
            SELECT
                locked.*,
                lag(
                    locked.bucket_start
                        + make_interval(secs => locked.bucket_secs)
                ) OVER (
                    PARTITION BY locked.source_kind, locked.interface,
                                 locked.origin_kind, locked.destination_start
                    ORDER BY locked.bucket_start, locked.bucket_secs
                ) AS previous_end
            FROM locked
        ), aggregated AS MATERIALIZED (
            SELECT
                $1::text AS client_id,
                locked.source_kind,
                locked.interface,
                locked.origin_kind,
                $5::integer AS bucket_secs,
                locked.destination_start AS bucket_start,
                sum(locked.rx_bytes)::bigint AS rx_bytes,
                sum(locked.tx_bytes)::bigint AS tx_bytes,
                LEAST(sum(locked.rx_valid_count), 2147483647)::integer
                    AS rx_valid_count,
                LEAST(sum(locked.tx_valid_count), 2147483647)::integer
                    AS tx_valid_count,
                LEAST(sum(locked.any_valid_count), 2147483647)::integer
                    AS any_valid_count,
                LEAST(sum(locked.rx_reset_count), 2147483647)::integer
                    AS rx_reset_count,
                LEAST(sum(locked.tx_reset_count), 2147483647)::integer
                    AS tx_reset_count,
                LEAST(sum(locked.any_reset_count), 2147483647)::integer
                    AS any_reset_count,
                min(locked.first_observed_at) AS first_observed_at,
                max(locked.latest_observed_at) AS latest_observed_at
            FROM ordered_locked locked
            JOIN complete_groups USING (
                source_kind, interface, origin_kind, destination_start
            )
            GROUP BY locked.source_kind, locked.interface,
                     locked.origin_kind, locked.destination_start
            HAVING count(*) FILTER (
                    WHERE locked.previous_end > locked.bucket_start
               ) = 0
        ), inserted AS (
            INSERT INTO traffic_counter_rollups (
                client_id, source_kind, interface, origin_kind,
                bucket_secs, bucket_start, rx_bytes, tx_bytes,
                rx_valid_count, tx_valid_count, any_valid_count,
                rx_reset_count, tx_reset_count, any_reset_count,
                first_observed_at, latest_observed_at
            )
            SELECT
                client_id, source_kind, interface, origin_kind,
                bucket_secs, bucket_start, rx_bytes, tx_bytes,
                rx_valid_count, tx_valid_count, any_valid_count,
                rx_reset_count, tx_reset_count, any_reset_count,
                first_observed_at, latest_observed_at
            FROM aggregated
            ON CONFLICT DO NOTHING
            RETURNING source_kind, interface, origin_kind,
                      bucket_secs, bucket_start
        ), deleted AS (
            DELETE FROM traffic_counter_rollups source
            USING locked, inserted
            WHERE source.ctid = locked.source_ctid
              AND inserted.source_kind = locked.source_kind
              AND inserted.interface = locked.interface
              AND inserted.origin_kind = locked.origin_kind
              AND inserted.bucket_secs = $5
              AND inserted.bucket_start = locked.destination_start
            RETURNING source.ctid
        ), overlap_conflicts AS MATERIALIZED (
            SELECT complete.source_kind, complete.interface,
                   complete.origin_kind, complete.destination_start
            FROM complete_groups complete
            WHERE NOT EXISTS (
                SELECT 1
                FROM aggregated
                WHERE aggregated.source_kind = complete.source_kind
                  AND aggregated.interface = complete.interface
                  AND aggregated.origin_kind = complete.origin_kind
                  AND aggregated.bucket_start = complete.destination_start
            )
        )
        SELECT
            (SELECT count(*) FROM deleted)::bigint AS deleted_rows,
            ((SELECT count(*) FROM overflow_groups)
                + (SELECT count(*) FROM destination_conflicts)
                + (SELECT count(*) FROM overlap_conflicts)
                + (SELECT count(*) FROM aggregated)
                - (SELECT count(*) FROM inserted))::bigint AS conflicts
    "#
}

#[cfg(test)]
#[path = "tests_traffic_retention.rs"]
mod tests;
