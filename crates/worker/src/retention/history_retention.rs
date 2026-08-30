use crate::{
    network_observation_retention::{
        network_observation_retention_phase_has_remaining_work,
        network_observation_retention_phase_next_at, process_network_observation_retention_phase,
        NetworkObservationRetentionPhase, NetworkObservationRetentionPolicy,
    },
    telemetry_minute_materialization::{
        materialize_next_telemetry_minute, telemetry_minute_consumer_has_ready_work,
        telemetry_minute_consumer_next_at, TelemetryMinuteConsumer,
    },
    traffic_retention::{
        process_traffic_retention_phase, traffic_retention_phase_has_remaining_work,
        traffic_retention_phase_next_at, TrafficRetentionPhase,
    },
};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::time::{Duration, Instant};
use vpsman_common::{
    DEFAULT_NETWORK_OBSERVATION_RETENTION_PRUNE_LIMIT, DEFAULT_TELEMETRY_RETENTION_PRUNE_LIMIT,
    DEFAULT_TELEMETRY_ROLLUP_RETENTION_DAYS, DEFAULT_TELEMETRY_SAMPLE_RETENTION_DAYS,
    TELEMETRY_HISTORY_TIERS,
};

// One recovery watchdog repairs a missed producer handoff or a transient owner
// failure. It never shortens a concrete DB-derived owner deadline and is not a
// per-owner processing cadence.
pub(crate) const TELEMETRY_HISTORY_RETENTION_RECOVERY_INTERVAL: Duration = Duration::from_secs(5);

const RESOURCE_ROLLUP_DOMAIN: &str = "telemetry_rollups";
const NETWORK_RATE_ROLLUP_DOMAIN: &str = "telemetry_network_rates";
const PING_ROLLUP_DOMAIN: &str = "telemetry_ping_rollups";
const SYSTEM_METRIC_ROLLUP_DOMAIN: &str = "system_metric_rollups";

// Each ordinary promotion page is one natural UTC destination span. Producers
// append exact due events; the coalescer alone updates the small span ledger.
// Promotion locks one span, and only a complete source replacement removes it.
// Large history tables are therefore never scanned to discover or disprove
// work.
//
// PostgreSQL 16 does not propagate column statistics across materialized CTEs.
// Keep membership, overlap, lock-completeness, and insert-success state on the
// source rows with windows; rejoining materialized owner lists makes the
// planner collapse fleet cardinality to one and choose quadratic nested loops.

#[derive(Clone, Copy, Debug, Default)]
struct PromotionResult {
    promoted: u64,
    #[cfg(test)]
    examined_source_rows: u64,
    #[cfg(test)]
    source_rows: u64,
}

#[derive(Clone, Debug, Default)]
struct OrdinaryPromotionResult {
    promotion: PromotionResult,
    has_remaining_work: bool,
}

#[derive(Clone, Debug, Default)]
struct PingPromotionResult {
    promotion: PromotionResult,
    complete: bool,
}

#[derive(Clone, Debug, Default)]
struct ResourcePromotionResult {
    promotion: PromotionResult,
    complete: bool,
}

#[derive(Clone, Debug, Default)]
struct NetworkRatePromotionResult {
    promotion: PromotionResult,
    complete: bool,
}

#[derive(Clone, Debug, Default)]
struct SystemMetricPromotionResult {
    promotion: PromotionResult,
    complete: bool,
}

#[derive(Clone, Debug)]
struct OrdinaryDueSpan {
    domain: String,
    source_bucket_secs: i32,
    destination_bucket_secs: i32,
    owner_identity: Vec<String>,
    destination_start: DateTime<Utc>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct DueEventCoalescingResult {
    pub(super) coalesced: u64,
    pub(super) has_remaining_work: bool,
}

#[derive(Clone, Copy)]
struct RetentionPolicy {
    enabled: bool,
    prune_limit: i32,
    retention_days: i32,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TelemetryHistoryRetentionRun {
    pub(crate) core_minute_source_rows: u64,
    pub(crate) core_minute_derived_rows: u64,
    pub(crate) traffic_minute_source_rows: u64,
    pub(crate) traffic_minute_derived_rows: u64,
    pub(crate) due_events_coalesced: u64,
    pub(crate) network_rate_spans_merged: u64,
    pub(crate) network_rates_pruned: u64,
    pub(crate) ping_spans_merged: u64,
    pub(crate) ping_rollups_pruned: u64,
    pub(crate) resource_spans_merged: u64,
    pub(crate) rollups_pruned: u64,
    pub(crate) samples_pruned: u64,
    pub(crate) system_metric_rollups_pruned: u64,
    pub(crate) system_metric_spans_merged: u64,
    pub(crate) ping_facts_pruned: u64,
    pub(crate) ping_current_pruned: u64,
    pub(crate) ping_series_pruned: u64,
    pub(crate) traffic_counter_samples_pruned: u64,
    pub(crate) traffic_raw_rows_promoted: u64,
    pub(crate) traffic_rollup_rows_promoted: u64,
    pub(crate) traffic_rollup_rows_pruned: u64,
    pub(crate) network_observation_source_rows_promoted: u64,
    pub(crate) network_observation_destination_rows_written: u64,
    pub(crate) network_observation_expired_exact_rows_pruned: u64,
    pub(crate) network_observation_expired_rollup_rows_pruned: u64,
    pub(crate) network_observation_inactive_latest_pruned: u64,
    pub(crate) network_observation_inactive_series_pruned: u64,
}

impl TelemetryHistoryRetentionRun {
    pub(crate) fn has_activity(self) -> bool {
        self.has_mutations()
    }

    fn has_mutations(self) -> bool {
        self.core_minute_source_rows > 0
            || self.core_minute_derived_rows > 0
            || self.traffic_minute_source_rows > 0
            || self.traffic_minute_derived_rows > 0
            || self.due_events_coalesced > 0
            || self.network_rate_spans_merged > 0
            || self.network_rates_pruned > 0
            || self.ping_spans_merged > 0
            || self.ping_rollups_pruned > 0
            || self.resource_spans_merged > 0
            || self.rollups_pruned > 0
            || self.samples_pruned > 0
            || self.system_metric_rollups_pruned > 0
            || self.system_metric_spans_merged > 0
            || self.ping_facts_pruned > 0
            || self.ping_current_pruned > 0
            || self.ping_series_pruned > 0
            || self.traffic_raw_rows_promoted > 0
            || self.traffic_rollup_rows_promoted > 0
            || self.traffic_rollup_rows_pruned > 0
            || self.network_observation_source_rows_promoted > 0
            || self.network_observation_destination_rows_written > 0
            || self.network_observation_expired_exact_rows_pruned > 0
            || self.network_observation_expired_rollup_rows_pruned > 0
            || self.network_observation_inactive_latest_pruned > 0
            || self.network_observation_inactive_series_pruned > 0
    }

    fn merge(&mut self, page: Self) {
        macro_rules! add {
            ($($field:ident),+ $(,)?) => {
                $(self.$field = self.$field.saturating_add(page.$field);)+
            };
        }
        add!(
            core_minute_source_rows,
            core_minute_derived_rows,
            traffic_minute_source_rows,
            traffic_minute_derived_rows,
            due_events_coalesced,
            network_rate_spans_merged,
            network_rates_pruned,
            ping_spans_merged,
            ping_rollups_pruned,
            resource_spans_merged,
            rollups_pruned,
            samples_pruned,
            system_metric_rollups_pruned,
            system_metric_spans_merged,
            ping_facts_pruned,
            ping_current_pruned,
            ping_series_pruned,
            traffic_counter_samples_pruned,
            traffic_raw_rows_promoted,
            traffic_rollup_rows_promoted,
            traffic_rollup_rows_pruned,
            network_observation_source_rows_promoted,
            network_observation_destination_rows_written,
            network_observation_expired_exact_rows_pruned,
            network_observation_expired_rollup_rows_pruned,
            network_observation_inactive_latest_pruned,
            network_observation_inactive_series_pruned,
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RetentionPhase {
    CoreMinuteMaterialization,
    TrafficMinuteMaterialization,
    DueEventCoalescing,
    ResourcePromotion,
    NetworkRatePromotion,
    PingPromotion,
    SystemMetricPromotion,
    Traffic(TrafficRetentionPhase),
    NetworkObservation(NetworkObservationRetentionPhase),
    SamplePrune,
    PingFactsPrune,
    PingCurrentPrune,
    PingSeriesPrune,
    ResourcePrune,
    NetworkRatePrune,
    PingRollupPrune,
    SystemMetricPrune,
}

/// Compile-time wake ownership for every logical phase. `NotifiedProducer`
/// frontiers are invalidated by their commit notification, `DerivedProducer`
/// frontiers by the exact parent page that can create them, and `Expiry`
/// frontiers by the oldest indexed row's eligibility timestamp. The recovery
/// watchdog is loss recovery for producer sentinels, never an owner cadence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WakeContract {
    NotifiedProducer,
    DerivedProducer,
    Expiry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RetentionOwner {
    phase: RetentionPhase,
    wake_contract: WakeContract,
}

/// The exhaustive match is the compile-time part of the contract: adding a
/// scheduler phase (including an inner traffic/observation phase) cannot
/// compile until its normal wake ownership is deliberately classified.
const fn wake_contract_for(phase: RetentionPhase) -> WakeContract {
    match phase {
        RetentionPhase::CoreMinuteMaterialization
        | RetentionPhase::TrafficMinuteMaterialization
        | RetentionPhase::DueEventCoalescing => WakeContract::NotifiedProducer,
        RetentionPhase::ResourcePromotion
        | RetentionPhase::NetworkRatePromotion
        | RetentionPhase::PingPromotion
        | RetentionPhase::SystemMetricPromotion
        | RetentionPhase::NetworkObservation(NetworkObservationRetentionPhase::RollupToDay)
        | RetentionPhase::NetworkObservation(NetworkObservationRetentionPhase::RollupToSixHours)
        | RetentionPhase::NetworkObservation(
            NetworkObservationRetentionPhase::RollupToThreeHours,
        )
        | RetentionPhase::NetworkObservation(NetworkObservationRetentionPhase::RollupToHour)
        | RetentionPhase::NetworkObservation(
            NetworkObservationRetentionPhase::RollupToThirtyMinutes,
        )
        | RetentionPhase::NetworkObservation(
            NetworkObservationRetentionPhase::RollupToFiveMinutes,
        )
        | RetentionPhase::PingCurrentPrune
        | RetentionPhase::PingSeriesPrune => WakeContract::DerivedProducer,
        RetentionPhase::Traffic(TrafficRetentionPhase::RawPromotion)
        | RetentionPhase::Traffic(TrafficRetentionPhase::RollupToDay)
        | RetentionPhase::Traffic(TrafficRetentionPhase::RollupToSixHours)
        | RetentionPhase::Traffic(TrafficRetentionPhase::RollupToThreeHours)
        | RetentionPhase::Traffic(TrafficRetentionPhase::TerminalPrune)
        | RetentionPhase::NetworkObservation(NetworkObservationRetentionPhase::TerminalPrune)
        | RetentionPhase::NetworkObservation(
            NetworkObservationRetentionPhase::InactiveLatestPrune,
        )
        | RetentionPhase::NetworkObservation(
            NetworkObservationRetentionPhase::InactiveSeriesPrune,
        )
        | RetentionPhase::SamplePrune
        | RetentionPhase::PingFactsPrune
        | RetentionPhase::ResourcePrune
        | RetentionPhase::NetworkRatePrune
        | RetentionPhase::PingRollupPrune
        | RetentionPhase::SystemMetricPrune => WakeContract::Expiry,
    }
}

const fn owner(phase: RetentionPhase) -> RetentionOwner {
    RetentionOwner {
        phase,
        wake_contract: wake_contract_for(phase),
    }
}

const RETENTION_PHASES: [RetentionOwner; 29] = [
    owner(RetentionPhase::CoreMinuteMaterialization),
    owner(RetentionPhase::TrafficMinuteMaterialization),
    owner(RetentionPhase::DueEventCoalescing),
    owner(RetentionPhase::ResourcePromotion),
    owner(RetentionPhase::NetworkRatePromotion),
    owner(RetentionPhase::PingPromotion),
    owner(RetentionPhase::SystemMetricPromotion),
    owner(RetentionPhase::Traffic(TrafficRetentionPhase::RawPromotion)),
    owner(RetentionPhase::Traffic(TrafficRetentionPhase::RollupToDay)),
    owner(RetentionPhase::Traffic(
        TrafficRetentionPhase::RollupToSixHours,
    )),
    owner(RetentionPhase::Traffic(
        TrafficRetentionPhase::RollupToThreeHours,
    )),
    owner(RetentionPhase::Traffic(
        TrafficRetentionPhase::TerminalPrune,
    )),
    owner(RetentionPhase::NetworkObservation(
        NetworkObservationRetentionPhase::TerminalPrune,
    )),
    owner(RetentionPhase::NetworkObservation(
        NetworkObservationRetentionPhase::RollupToDay,
    )),
    owner(RetentionPhase::NetworkObservation(
        NetworkObservationRetentionPhase::RollupToSixHours,
    )),
    owner(RetentionPhase::NetworkObservation(
        NetworkObservationRetentionPhase::RollupToThreeHours,
    )),
    owner(RetentionPhase::NetworkObservation(
        NetworkObservationRetentionPhase::RollupToHour,
    )),
    owner(RetentionPhase::NetworkObservation(
        NetworkObservationRetentionPhase::RollupToThirtyMinutes,
    )),
    owner(RetentionPhase::NetworkObservation(
        NetworkObservationRetentionPhase::RollupToFiveMinutes,
    )),
    owner(RetentionPhase::NetworkObservation(
        NetworkObservationRetentionPhase::InactiveLatestPrune,
    )),
    owner(RetentionPhase::NetworkObservation(
        NetworkObservationRetentionPhase::InactiveSeriesPrune,
    )),
    owner(RetentionPhase::SamplePrune),
    owner(RetentionPhase::PingFactsPrune),
    owner(RetentionPhase::PingCurrentPrune),
    owner(RetentionPhase::PingSeriesPrune),
    owner(RetentionPhase::ResourcePrune),
    owner(RetentionPhase::NetworkRatePrune),
    owner(RetentionPhase::PingRollupPrune),
    owner(RetentionPhase::SystemMetricPrune),
];

/// Frontiers that can move earlier through writers outside this process-local
/// retention graph. A listener reconnect invalidates exactly this named union
/// because a notification may have committed while disconnected.
const EXTERNAL_WRITER_FRONTIERS: [RetentionPhase; 17] = [
    RetentionPhase::CoreMinuteMaterialization,
    RetentionPhase::TrafficMinuteMaterialization,
    RetentionPhase::DueEventCoalescing,
    RetentionPhase::Traffic(TrafficRetentionPhase::RawPromotion),
    RetentionPhase::Traffic(TrafficRetentionPhase::RollupToDay),
    RetentionPhase::Traffic(TrafficRetentionPhase::RollupToSixHours),
    RetentionPhase::Traffic(TrafficRetentionPhase::RollupToThreeHours),
    RetentionPhase::Traffic(TrafficRetentionPhase::TerminalPrune),
    RetentionPhase::NetworkObservation(NetworkObservationRetentionPhase::TerminalPrune),
    RetentionPhase::NetworkObservation(NetworkObservationRetentionPhase::InactiveLatestPrune),
    RetentionPhase::NetworkObservation(NetworkObservationRetentionPhase::InactiveSeriesPrune),
    RetentionPhase::SamplePrune,
    RetentionPhase::PingCurrentPrune,
    RetentionPhase::ResourcePrune,
    RetentionPhase::NetworkRatePrune,
    RetentionPhase::PingRollupPrune,
    RetentionPhase::SystemMetricPrune,
];

struct RetentionPhaseExecution {
    run: TelemetryHistoryRetentionRun,
    status: RetentionOwnerStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RetentionOwnerStatus {
    Current(RetentionNextAt),
    StillDue,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RetentionNextAt {
    At(RetentionDeadline),
    ProducerOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RetentionDeadline {
    database_at: DateTime<Utc>,
    monotonic_at: Instant,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct DatabaseDeadline {
    pub(super) database_at: DateTime<Utc>,
    pub(super) remaining: Duration,
}

pub(super) fn database_deadline(
    database_at: DateTime<Utc>,
    remaining_seconds: f64,
) -> Result<DatabaseDeadline> {
    anyhow::ensure!(
        remaining_seconds.is_finite() && remaining_seconds >= 0.0,
        "database retention deadline returned an invalid remaining duration"
    );
    Ok(DatabaseDeadline {
        database_at,
        remaining: Duration::from_secs_f64(remaining_seconds),
    })
}

pub(super) fn optional_database_deadline(
    row: Option<(DateTime<Utc>, f64)>,
) -> Result<Option<DatabaseDeadline>> {
    row.map(|(database_at, remaining_seconds)| database_deadline(database_at, remaining_seconds))
        .transpose()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RetentionOwnerState {
    Unchecked,
    Due,
    Current { next_at: RetentionNextAt },
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TelemetryHistoryRetentionStep {
    MoreWork,
    CurrentUntil(Instant),
}

#[derive(Debug)]
pub(crate) enum TelemetryHistoryRetentionPage {
    MoreWork,
    CurrentUntil(Instant),
    OwnerFailed(anyhow::Error),
}

/// Result of proving one eligible owner's durable frontier before processing
/// that exact owner. An idle proof records either its exact indexed deadline or
/// a producer-only sentinel; a ready frontier is revalidated by its owner.
#[derive(Debug)]
pub(crate) enum TelemetryHistoryRetentionPageReadiness {
    Ready,
    NoWork(TelemetryHistoryRetentionPage),
}

/// Stateful fair rotation across the independent retention owners. Each
/// external page invokes exactly one eligible owner. Concrete Current proofs
/// wake only at their DB-derived deadline; producer-only proofs are invalidated
/// by typed commit notifications, with one recovery watchdog for a lost
/// handoff. StillDue owners remain immediately runnable and fairly rotated.
pub(crate) struct TelemetryHistoryRetentionDrain {
    total: TelemetryHistoryRetentionRun,
    next_phase: usize,
    recovery_interval: Duration,
    recovery_at: Instant,
    owner_states: [RetentionOwnerState; RETENTION_PHASES.len()],
}

impl TelemetryHistoryRetentionDrain {
    pub(crate) fn new(recovery_interval: Duration) -> Self {
        let now = Instant::now();
        Self {
            total: TelemetryHistoryRetentionRun::default(),
            next_phase: 0,
            recovery_interval,
            recovery_at: now + recovery_interval,
            owner_states: [RetentionOwnerState::Unchecked; RETENTION_PHASES.len()],
        }
    }

    pub(crate) fn next_step(&mut self) -> TelemetryHistoryRetentionStep {
        self.next_step_at(Instant::now())
    }

    pub(crate) async fn process_page(
        &mut self,
        pool: &PgPool,
    ) -> Result<TelemetryHistoryRetentionPage> {
        let now = Instant::now();
        let Some(phase_index) = self.next_eligible_phase_at(now) else {
            return Ok(self.next_step_at(now).into());
        };
        let phase = RETENTION_PHASES[phase_index].phase;
        let execution = match process_retention_phase(pool, phase).await {
            Ok(execution) => execution,
            Err(error) if retention_error_requires_global_backoff(&error) => return Err(error),
            Err(error) => {
                self.observe_phase_failure(phase_index, Instant::now())?;
                return Ok(TelemetryHistoryRetentionPage::OwnerFailed(error));
            }
        };
        self.total.merge(execution.run);
        self.observe_phase(phase_index, execution.status)?;
        Ok(self.next_step().into())
    }

    /// Proves whether the next logical owner has work using the same bounded
    /// durable frontier that its processor uses. This deliberately happens
    /// before exact-owner processing: empty owners need neither exclusion nor
    /// an observability-row write. A later producer commit invalidates exactly
    /// the consumer frontier named by its typed effect; the global watchdog is
    /// only missed-handoff/crash recovery for producer-only proofs.
    pub(crate) async fn prepare_page(
        &mut self,
        pool: &PgPool,
    ) -> Result<TelemetryHistoryRetentionPageReadiness> {
        let now = Instant::now();
        let Some(phase_index) = self.next_eligible_phase_at(now) else {
            return Ok(TelemetryHistoryRetentionPageReadiness::NoWork(
                self.next_step_at(now).into(),
            ));
        };
        let phase = RETENTION_PHASES[phase_index].phase;
        match retention_phase_readiness(pool, phase).await {
            Ok(RetentionPhaseReadiness::Ready) => Ok(TelemetryHistoryRetentionPageReadiness::Ready),
            Ok(RetentionPhaseReadiness::NextAt(next_at)) => {
                self.observe_phase(phase_index, RetentionOwnerStatus::Current(next_at))?;
                Ok(TelemetryHistoryRetentionPageReadiness::NoWork(
                    self.next_step().into(),
                ))
            }
            Err(error) if retention_error_requires_global_backoff(&error) => Err(error),
            Err(error) => {
                self.observe_phase_failure(phase_index, Instant::now())?;
                Ok(TelemetryHistoryRetentionPageReadiness::NoWork(
                    TelemetryHistoryRetentionPage::OwnerFailed(error),
                ))
            }
        }
    }

    pub(crate) fn take_run(&mut self) -> TelemetryHistoryRetentionRun {
        std::mem::take(&mut self.total)
    }

    pub(crate) fn finish(self) -> TelemetryHistoryRetentionRun {
        self.total
    }

    fn next_step_at(&mut self, now: Instant) -> TelemetryHistoryRetentionStep {
        self.recover_producer_sentinels_if_due(now);
        if self.next_eligible_phase_at(now).is_some() {
            return TelemetryHistoryRetentionStep::MoreWork;
        }

        let earliest_recheck = self
            .owner_states
            .iter()
            .filter_map(|state| self.owner_wake_at(*state, now))
            .chain(std::iter::once(self.recovery_at))
            .min()
            .expect("retention owner registry is non-empty");
        TelemetryHistoryRetentionStep::CurrentUntil(earliest_recheck)
    }

    fn next_eligible_phase_at(&self, now: Instant) -> Option<usize> {
        (0..RETENTION_PHASES.len())
            .map(|offset| (self.next_phase + offset) % RETENTION_PHASES.len())
            .find(|&phase_index| match self.owner_states[phase_index] {
                RetentionOwnerState::Unchecked | RetentionOwnerState::Due => true,
                RetentionOwnerState::Current {
                    next_at: RetentionNextAt::At(deadline),
                } => now >= deadline.monotonic_at,
                RetentionOwnerState::Current {
                    next_at: RetentionNextAt::ProducerOnly,
                }
                | RetentionOwnerState::Failed => false,
            })
    }

    fn owner_wake_at(&self, state: RetentionOwnerState, now: Instant) -> Option<Instant> {
        match state {
            RetentionOwnerState::Unchecked | RetentionOwnerState::Due => Some(now),
            RetentionOwnerState::Current {
                next_at: RetentionNextAt::At(deadline),
            } => Some(deadline.monotonic_at),
            RetentionOwnerState::Current {
                next_at: RetentionNextAt::ProducerOnly,
            }
            | RetentionOwnerState::Failed => None,
        }
    }

    fn observe_phase(&mut self, phase_index: usize, status: RetentionOwnerStatus) -> Result<()> {
        anyhow::ensure!(
            phase_index < RETENTION_PHASES.len(),
            "retention phase index is outside the owner registry"
        );
        self.owner_states[phase_index] = match status {
            RetentionOwnerStatus::Current(next_at) => RetentionOwnerState::Current { next_at },
            RetentionOwnerStatus::StillDue => RetentionOwnerState::Due,
        };
        self.next_phase = (phase_index + 1) % RETENTION_PHASES.len();
        Ok(())
    }

    fn observe_phase_failure(&mut self, phase_index: usize, _failed_at: Instant) -> Result<()> {
        anyhow::ensure!(
            phase_index < RETENTION_PHASES.len(),
            "failed retention phase index is outside the owner registry"
        );
        self.owner_states[phase_index] = RetentionOwnerState::Failed;
        self.next_phase = (phase_index + 1) % RETENTION_PHASES.len();
        Ok(())
    }

    fn recover_producer_sentinels_if_due(&mut self, now: Instant) {
        if now < self.recovery_at {
            return;
        }
        for (phase_index, _owner) in RETENTION_PHASES.iter().enumerate() {
            let producer_sentinel = matches!(
                self.owner_states[phase_index],
                RetentionOwnerState::Current {
                    next_at: RetentionNextAt::ProducerOnly,
                }
            );
            if producer_sentinel
                || matches!(self.owner_states[phase_index], RetentionOwnerState::Failed)
            {
                self.owner_states[phase_index] = RetentionOwnerState::Unchecked;
            }
        }
        self.recovery_at = now + self.recovery_interval;
    }

    fn notify_phase_database_deadline(&mut self, phase_index: usize, database_at: DateTime<Utc>) {
        debug_assert!(phase_index < RETENTION_PHASES.len());
        self.owner_states[phase_index] = match self.owner_states[phase_index] {
            RetentionOwnerState::Unchecked | RetentionOwnerState::Due => {
                self.owner_states[phase_index]
            }
            RetentionOwnerState::Current {
                next_at: RetentionNextAt::At(existing),
            } if existing.database_at <= database_at => self.owner_states[phase_index],
            RetentionOwnerState::Current { .. } => RetentionOwnerState::Unchecked,
            RetentionOwnerState::Failed => RetentionOwnerState::Failed,
        };
    }

    fn phase_index(phase: RetentionPhase) -> usize {
        RETENTION_PHASES
            .iter()
            .position(|owner| owner.phase == phase)
            .expect("typed retention consumer is absent from the owner registry")
    }

    fn notify_phase_now(&mut self, phase: RetentionPhase) {
        let phase_index = Self::phase_index(phase);
        if !matches!(self.owner_states[phase_index], RetentionOwnerState::Failed) {
            self.owner_states[phase_index] = RetentionOwnerState::Due;
        }
    }

    pub(crate) fn notify_projection_minute_ready_at(&mut self, database_at: DateTime<Utc>) {
        self.notify_phase_database_deadline(
            Self::phase_index(RetentionPhase::CoreMinuteMaterialization),
            database_at,
        );
        self.notify_phase_database_deadline(
            Self::phase_index(RetentionPhase::TrafficMinuteMaterialization),
            database_at,
        );
    }

    pub(crate) fn notify_due_events_ready_at(&mut self, database_at: DateTime<Utc>) {
        self.notify_phase_database_deadline(
            Self::phase_index(RetentionPhase::DueEventCoalescing),
            database_at,
        );
    }

    pub(crate) fn notify_due_span_published_at(
        &mut self,
        domain: &str,
        source_bucket_secs: i32,
        destination_bucket_secs: i32,
        database_at: DateTime<Utc>,
    ) -> Result<()> {
        let phase = due_span_consumer(domain, source_bucket_secs, destination_bucket_secs)?;
        self.notify_phase_database_deadline(Self::phase_index(phase), database_at);
        Ok(())
    }

    pub(crate) fn notify_core_minute_frontier_advanced_now(&mut self) {
        self.notify_phase_now(RetentionPhase::CoreMinuteMaterialization);
        self.notify_phase_now(RetentionPhase::SamplePrune);
    }

    pub(crate) fn notify_traffic_minute_frontier_advanced_now(&mut self) {
        self.notify_phase_now(RetentionPhase::TrafficMinuteMaterialization);
        self.notify_phase_now(RetentionPhase::SamplePrune);
    }

    pub(crate) fn notify_ping_facts_published_now(&mut self) {
        self.notify_phase_now(RetentionPhase::PingFactsPrune);
    }

    pub(crate) fn notify_ping_facts_deleted_now(&mut self) {
        self.notify_phase_now(RetentionPhase::PingCurrentPrune);
    }

    pub(crate) fn notify_ping_current_deleted_now(&mut self) {
        self.notify_phase_now(RetentionPhase::PingSeriesPrune);
    }

    pub(crate) fn notify_telemetry_samples_deleted_now(&mut self) {
        self.notify_phase_now(RetentionPhase::NetworkObservation(
            NetworkObservationRetentionPhase::InactiveSeriesPrune,
        ));
    }

    pub(crate) fn notify_ordinary_rollup_published_now(&mut self, domain: &str) -> Result<()> {
        let phase = match domain {
            RESOURCE_ROLLUP_DOMAIN => RetentionPhase::ResourcePrune,
            NETWORK_RATE_ROLLUP_DOMAIN => RetentionPhase::NetworkRatePrune,
            PING_ROLLUP_DOMAIN => RetentionPhase::PingRollupPrune,
            SYSTEM_METRIC_ROLLUP_DOMAIN => RetentionPhase::SystemMetricPrune,
            "network_observation_rollups" => {
                RetentionPhase::NetworkObservation(NetworkObservationRetentionPhase::TerminalPrune)
            }
            domain => anyhow::bail!("unsupported published rollup domain {domain}"),
        };
        self.notify_phase_now(phase);
        Ok(())
    }

    pub(crate) fn recover_external_writer_frontiers(&mut self) {
        for phase in EXTERNAL_WRITER_FRONTIERS {
            let phase_index = Self::phase_index(phase);
            if !matches!(self.owner_states[phase_index], RetentionOwnerState::Due) {
                self.owner_states[phase_index] = RetentionOwnerState::Unchecked;
            }
        }
    }

    pub(crate) fn notify_sample_prune_now(&mut self) {
        self.notify_phase_now(RetentionPhase::SamplePrune);
    }

    pub(crate) fn notify_sample_prune_ready_at(&mut self, database_at: DateTime<Utc>) {
        self.notify_phase_database_deadline(
            Self::phase_index(RetentionPhase::SamplePrune),
            database_at,
        );
    }

    pub(crate) fn notify_manual_network_observation_now(&mut self) {
        self.notify_phase_now(RetentionPhase::NetworkObservation(
            NetworkObservationRetentionPhase::TerminalPrune,
        ));
    }

    pub(crate) fn notify_network_observation_series_deactivated_now(&mut self) {
        self.notify_phase_now(RetentionPhase::NetworkObservation(
            NetworkObservationRetentionPhase::InactiveLatestPrune,
        ));
        self.notify_phase_now(RetentionPhase::NetworkObservation(
            NetworkObservationRetentionPhase::InactiveSeriesPrune,
        ));
    }

    pub(crate) fn notify_traffic_rollup_published(&mut self, bucket_secs: i32) -> Result<()> {
        let next_tier = match bucket_secs {
            3_600 => Some(TrafficRetentionPhase::RollupToThreeHours),
            10_800 => Some(TrafficRetentionPhase::RollupToSixHours),
            21_600 => Some(TrafficRetentionPhase::RollupToDay),
            86_400 => None,
            bucket_secs => {
                anyhow::bail!("unsupported published traffic bucket width {bucket_secs}")
            }
        };
        if let Some(next_tier) = next_tier {
            self.notify_phase_now(RetentionPhase::Traffic(next_tier));
        }
        self.notify_phase_now(RetentionPhase::Traffic(
            TrafficRetentionPhase::TerminalPrune,
        ));
        Ok(())
    }

    pub(crate) fn notify_traffic_samples_published_now(&mut self) -> Result<()> {
        self.notify_phase_now(RetentionPhase::Traffic(TrafficRetentionPhase::RawPromotion));
        Ok(())
    }

    pub(crate) fn notify_ping_topology_changed_now(&mut self) {
        self.notify_phase_now(RetentionPhase::PingCurrentPrune);
    }

    pub(crate) fn notify_ping_rollups_deleted_now(&mut self) -> Result<()> {
        self.notify_phase_now(RetentionPhase::PingCurrentPrune);
        Ok(())
    }

    pub(crate) fn notify_network_observation_history_deleted_now(&mut self) -> Result<()> {
        self.notify_phase_now(RetentionPhase::NetworkObservation(
            NetworkObservationRetentionPhase::InactiveSeriesPrune,
        ));
        Ok(())
    }

    pub(crate) fn notify_network_observation_latest_deleted_now(&mut self) -> Result<()> {
        self.notify_phase_now(RetentionPhase::NetworkObservation(
            NetworkObservationRetentionPhase::InactiveSeriesPrune,
        ));
        Ok(())
    }

    pub(crate) fn notify_retention_policy_changed_now(&mut self, domain: &str) -> Result<()> {
        let phase = match domain {
            "telemetry_rollups" => RetentionPhase::ResourcePrune,
            "telemetry_network_rates" => RetentionPhase::NetworkRatePrune,
            "telemetry_ping_rollups" => RetentionPhase::PingRollupPrune,
            "system_metric_rollups" => RetentionPhase::SystemMetricPrune,
            "network_observations" => {
                RetentionPhase::NetworkObservation(NetworkObservationRetentionPhase::TerminalPrune)
            }
            "traffic_counter_rollups" => {
                RetentionPhase::Traffic(TrafficRetentionPhase::TerminalPrune)
            }
            _ => anyhow::bail!("unsupported telemetry retention policy wake domain {domain}"),
        };
        self.notify_phase_now(phase);
        Ok(())
    }
}

fn due_span_consumer(
    domain: &str,
    published_source_bucket_secs: i32,
    published_destination_bucket_secs: i32,
) -> Result<RetentionPhase> {
    let ordinary_edge = TELEMETRY_HISTORY_TIERS.windows(2).any(|tiers| {
        tiers[0].bucket_secs == published_source_bucket_secs
            && tiers[1].bucket_secs == published_destination_bucket_secs
    });
    let phase = match domain {
        RESOURCE_ROLLUP_DOMAIN if ordinary_edge => RetentionPhase::ResourcePromotion,
        NETWORK_RATE_ROLLUP_DOMAIN if ordinary_edge => RetentionPhase::NetworkRatePromotion,
        PING_ROLLUP_DOMAIN if ordinary_edge => RetentionPhase::PingPromotion,
        SYSTEM_METRIC_ROLLUP_DOMAIN if ordinary_edge => RetentionPhase::SystemMetricPromotion,
        "network_observation_rollups" => RETENTION_PHASES
            .iter()
            .map(|owner| owner.phase)
            .find(|phase| {
                matches!(
                    phase,
                    RetentionPhase::NetworkObservation(observation_phase)
                        if observation_phase.due_span_key().is_some_and(
                            |(_, source_bucket_secs, destination_bucket_secs)| {
                                source_bucket_secs == published_source_bucket_secs
                                    && destination_bucket_secs
                                        == published_destination_bucket_secs
                            }
                        )
                )
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "network-observation due span has unsupported {}s -> {}s edge",
                    published_source_bucket_secs,
                    published_destination_bucket_secs
                )
            })?,
        domain => anyhow::bail!("unsupported telemetry due-span domain {domain}"),
    };
    Ok(phase)
}

impl Default for TelemetryHistoryRetentionDrain {
    fn default() -> Self {
        Self::new(TELEMETRY_HISTORY_RETENTION_RECOVERY_INTERVAL)
    }
}

impl From<TelemetryHistoryRetentionStep> for TelemetryHistoryRetentionPage {
    fn from(step: TelemetryHistoryRetentionStep) -> Self {
        match step {
            TelemetryHistoryRetentionStep::MoreWork => Self::MoreWork,
            TelemetryHistoryRetentionStep::CurrentUntil(deadline) => Self::CurrentUntil(deadline),
        }
    }
}

fn retention_error_requires_global_backoff(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<sqlx::Error>())
        .any(|error| match error {
            sqlx::Error::Configuration(_)
            | sqlx::Error::Io(_)
            | sqlx::Error::Tls(_)
            | sqlx::Error::Protocol(_)
            | sqlx::Error::PoolTimedOut
            | sqlx::Error::PoolClosed
            | sqlx::Error::WorkerCrashed
            | sqlx::Error::BeginFailed => true,
            sqlx::Error::Database(database) => database.code().is_some_and(|code| {
                code.starts_with("08")
                    || code.starts_with("53")
                    || code.starts_with("57P")
                    || code.starts_with("58")
            }),
            _ => false,
        })
}

async fn telemetry_due_events_have_remaining_work(pool: &PgPool) -> Result<bool> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM telemetry_history_due_events
            WHERE coalesce_ready_at <= now()
        )
        "#,
    )
    .fetch_one(pool)
    .await?)
}

/// Transfers one exact, currently-ready natural owner coordinate into the
/// promotion authority. The single-row seed chooses an owner; it does not cap
/// that owner's evidence. Open coordinates and events committed after this
/// statement snapshot remain in the append-only buffer for an immediate next
/// pass. Contention with promotion is therefore confined to the same owner and
/// UTC span, while producers never touch the span authority.
pub(super) async fn coalesce_ready_telemetry_due_events(
    pool: &PgPool,
) -> Result<DueEventCoalescingResult> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        r#"
        WITH seed AS MATERIALIZED (
            SELECT event_id, domain, source_bucket_secs,
                   destination_bucket_secs, owner_identity,
                   destination_start, due_at
            FROM telemetry_history_due_events
            WHERE coalesce_ready_at <= now()
            ORDER BY coalesce_ready_at, event_id
            LIMIT 1
            FOR UPDATE SKIP LOCKED
        ), ready_events AS MATERIALIZED (
            SELECT event.event_id, event.domain, event.source_bucket_secs,
                   event.destination_bucket_secs, event.owner_identity,
                   event.destination_start, event.due_at
            FROM telemetry_history_due_events event
            JOIN seed ON seed.domain = event.domain
                     AND seed.source_bucket_secs = event.source_bucket_secs
                     AND seed.destination_bucket_secs
                            = event.destination_bucket_secs
                     AND seed.owner_identity = event.owner_identity
                     AND seed.destination_start = event.destination_start
                     AND seed.due_at = event.due_at
            WHERE event.coalesce_ready_at <= now()
            ORDER BY event.event_id
            FOR UPDATE OF event SKIP LOCKED
        ), coordinates AS MATERIALIZED (
            SELECT domain, source_bucket_secs, destination_bucket_secs,
                   owner_identity, destination_start, due_at
            FROM ready_events
            GROUP BY domain, source_bucket_secs, destination_bucket_secs,
                     owner_identity, destination_start, due_at
        ), ensured_spans AS (
            -- If promotion owns this exact span, its delete either commits
            -- before this insert recreates it or rolls back and leaves it.
            -- No unrelated owner is part of this transaction.
            INSERT INTO telemetry_history_due_spans AS current (
                domain, source_bucket_secs, destination_bucket_secs,
                owner_identity, destination_start, due_at
            )
            SELECT domain, source_bucket_secs, destination_bucket_secs,
                   owner_identity, destination_start, due_at
            FROM coordinates
            ORDER BY domain, owner_identity, source_bucket_secs,
                     destination_bucket_secs, destination_start
            ON CONFLICT (
                domain, source_bucket_secs,
                destination_bucket_secs, destination_start, owner_identity
            ) DO UPDATE SET due_at = current.due_at
            WHERE FALSE
            RETURNING domain
        ), ensured AS MATERIALIZED (
            -- Referencing the data-modifying CTE makes the handoff explicit:
            -- captured evidence is deleted only in the same atomic statement
            -- that has attempted every corresponding span insertion.
            SELECT count(*) AS inserted_spans FROM ensured_spans
        ), deleted AS (
            DELETE FROM telemetry_history_due_events event
            USING ready_events ready, ensured
            WHERE event.event_id = ready.event_id
            RETURNING event.event_id
        )
        SELECT count(*)::bigint AS deleted_rows
        FROM deleted
        "#,
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    let has_remaining_work = telemetry_due_events_have_remaining_work(pool).await?;
    let coalesced = row.try_get::<i64, _>("deleted_rows")?.max(0) as u64;
    Ok(DueEventCoalescingResult {
        coalesced,
        has_remaining_work,
    })
}

async fn claim_ordinary_due_span(
    tx: &mut Transaction<'_, Postgres>,
    domain: &str,
) -> Result<Option<OrdinaryDueSpan>> {
    let row = sqlx::query(
        r#"
        SELECT domain, source_bucket_secs, destination_bucket_secs,
               owner_identity, destination_start
        FROM telemetry_history_due_spans
        WHERE domain = $1
          AND due_at <= now()
        ORDER BY due_at, source_bucket_secs, destination_bucket_secs,
                 destination_start
        LIMIT 1
        FOR UPDATE SKIP LOCKED
        "#,
    )
    .bind(domain)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| {
        Ok(OrdinaryDueSpan {
            domain: row.try_get("domain")?,
            source_bucket_secs: row.try_get("source_bucket_secs")?,
            destination_bucket_secs: row.try_get("destination_bucket_secs")?,
            owner_identity: row.try_get("owner_identity")?,
            destination_start: row.try_get("destination_start")?,
        })
    })
    .transpose()
}

async fn ordinary_due_spans_have_remaining_work(pool: &PgPool, domain: &str) -> Result<bool> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM telemetry_history_due_spans
            WHERE domain = $1
              AND due_at <= now()
        )
        "#,
    )
    .bind(domain)
    .fetch_one(pool)
    .await?)
}

async fn ordinary_due_spans_next_at(
    pool: &PgPool,
    domain: &str,
) -> Result<Option<DatabaseDeadline>> {
    optional_database_deadline(
        sqlx::query_as(
            r#"
        WITH frontier AS (
            SELECT due_at AS database_at
            FROM telemetry_history_due_spans
            WHERE domain = $1
            ORDER BY due_at, source_bucket_secs, destination_bucket_secs,
                     destination_start
            LIMIT 1
        )
        SELECT database_at,
               GREATEST(
                   EXTRACT(EPOCH FROM database_at - clock_timestamp()), 0
               )::DOUBLE PRECISION AS remaining_seconds
        FROM frontier
        "#,
        )
        .bind(domain)
        .fetch_optional(pool)
        .await?,
    )
}

fn ordinary_due_span_lower_tiers(span: &OrdinaryDueSpan) -> Result<Vec<i32>> {
    anyhow::ensure!(
        matches!(
            span.domain.as_str(),
            RESOURCE_ROLLUP_DOMAIN
                | NETWORK_RATE_ROLLUP_DOMAIN
                | PING_ROLLUP_DOMAIN
                | SYSTEM_METRIC_ROLLUP_DOMAIN
        ),
        "unsupported ordinary telemetry due-span domain {}",
        span.domain
    );
    let Some(source_index) = TELEMETRY_HISTORY_TIERS
        .iter()
        .position(|tier| tier.bucket_secs == span.source_bucket_secs)
    else {
        anyhow::bail!(
            "{} due span has unsupported {}s source tier",
            span.domain,
            span.source_bucket_secs
        );
    };
    let Some(destination) = TELEMETRY_HISTORY_TIERS.get(source_index + 1) else {
        anyhow::bail!("{} due span cannot promote the terminal tier", span.domain);
    };
    anyhow::ensure!(
        destination.bucket_secs == span.destination_bucket_secs,
        "{} due span has non-successor {}s to {}s transition",
        span.domain,
        span.source_bucket_secs,
        span.destination_bucket_secs
    );
    Ok(TELEMETRY_HISTORY_TIERS[..=source_index]
        .iter()
        .map(|tier| tier.bucket_secs)
        .collect())
}

fn exact_owner_identity<'a>(
    span: &'a OrdinaryDueSpan,
    expected_domain: &str,
    component_count: usize,
) -> Result<&'a [String]> {
    anyhow::ensure!(
        span.domain == expected_domain && span.owner_identity.len() == component_count,
        "{} due span has an invalid natural owner identity",
        span.domain
    );
    Ok(&span.owner_identity)
}

async fn delete_completed_ordinary_due_span(
    tx: &mut Transaction<'_, Postgres>,
    span: &OrdinaryDueSpan,
) -> Result<()> {
    let deleted = sqlx::query(
        r#"
        DELETE FROM telemetry_history_due_spans
        WHERE domain = $1
          AND source_bucket_secs = $2
          AND destination_bucket_secs = $3
          AND destination_start = $4
          AND owner_identity = $5
        "#,
    )
    .bind(&span.domain)
    .bind(span.source_bucket_secs)
    .bind(span.destination_bucket_secs)
    .bind(span.destination_start)
    .bind(&span.owner_identity)
    .execute(&mut **tx)
    .await?;
    anyhow::ensure!(
        deleted.rows_affected() == 1,
        "locked {} due span disappeared before completion",
        span.domain
    );
    Ok(())
}

/// Exact, read-only readiness proof for one retention owner. Every arm uses
/// the same durable queue, due span, cursor frontier, or indexed expiry
/// predicate as its processing path. It is not a rate limit: a true result
/// still runs the existing page immediately and the owner remains runnable
/// without delay while its post-page proof reports more work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RetentionPhaseReadiness {
    Ready,
    NextAt(RetentionNextAt),
}

async fn retention_phase_readiness(
    pool: &PgPool,
    phase: RetentionPhase,
) -> Result<RetentionPhaseReadiness> {
    if retention_phase_has_ready_work(pool, phase).await? {
        return Ok(RetentionPhaseReadiness::Ready);
    }
    Ok(RetentionPhaseReadiness::NextAt(
        retention_phase_next_at(pool, phase).await?,
    ))
}

async fn retention_phase_has_ready_work(pool: &PgPool, phase: RetentionPhase) -> Result<bool> {
    match phase {
        RetentionPhase::CoreMinuteMaterialization => {
            telemetry_minute_consumer_has_ready_work(pool, TelemetryMinuteConsumer::Core).await
        }
        RetentionPhase::TrafficMinuteMaterialization => {
            telemetry_minute_consumer_has_ready_work(pool, TelemetryMinuteConsumer::Traffic).await
        }
        RetentionPhase::DueEventCoalescing => telemetry_due_events_have_remaining_work(pool).await,
        RetentionPhase::ResourcePromotion => {
            ordinary_due_spans_have_remaining_work(pool, RESOURCE_ROLLUP_DOMAIN).await
        }
        RetentionPhase::NetworkRatePromotion => {
            ordinary_due_spans_have_remaining_work(pool, NETWORK_RATE_ROLLUP_DOMAIN).await
        }
        RetentionPhase::PingPromotion => {
            ordinary_due_spans_have_remaining_work(pool, PING_ROLLUP_DOMAIN).await
        }
        RetentionPhase::SystemMetricPromotion => {
            ordinary_due_spans_have_remaining_work(pool, SYSTEM_METRIC_ROLLUP_DOMAIN).await
        }
        RetentionPhase::Traffic(traffic_phase) => {
            traffic_retention_phase_has_remaining_work(pool, traffic_phase).await
        }
        RetentionPhase::NetworkObservation(observation_phase) => {
            let policy = load_policy(pool, "network_observations").await?;
            network_observation_retention_phase_has_remaining_work(
                pool,
                NetworkObservationRetentionPolicy {
                    enabled: policy.enabled,
                    retention_days: policy.retention_days,
                    prune_limit: policy.prune_limit,
                },
                observation_phase,
            )
            .await
        }
        RetentionPhase::SamplePrune => {
            prune_domain_has_remaining_work(pool, "telemetry_samples", raw_sample_policy()).await
        }
        RetentionPhase::PingFactsPrune => {
            ping_fact_prune_has_remaining_work(pool, raw_sample_policy()).await
        }
        RetentionPhase::PingCurrentPrune => {
            ping_current_prune_has_remaining_work(pool, raw_sample_policy()).await
        }
        RetentionPhase::PingSeriesPrune => {
            ping_series_prune_has_remaining_work(pool, raw_sample_policy()).await
        }
        RetentionPhase::ResourcePrune => {
            let policy = load_policy(pool, "telemetry_rollups").await?;
            prune_domain_has_remaining_work(pool, "telemetry_rollups", policy).await
        }
        RetentionPhase::NetworkRatePrune => {
            let policy = load_policy(pool, "telemetry_network_rates").await?;
            prune_domain_has_remaining_work(pool, "telemetry_network_rates", policy).await
        }
        RetentionPhase::PingRollupPrune => {
            let policy = load_policy(pool, "telemetry_ping_rollups").await?;
            prune_domain_has_remaining_work(pool, "telemetry_ping_rollups", policy).await
        }
        RetentionPhase::SystemMetricPrune => {
            let policy = load_policy(pool, "system_metric_rollups").await?;
            prune_domain_has_remaining_work(pool, "system_metric_rollups", policy).await
        }
    }
}

fn database_next_at(next_at: Option<DatabaseDeadline>) -> RetentionNextAt {
    let Some(next_at) = next_at else {
        return RetentionNextAt::ProducerOnly;
    };
    let monotonic_now = Instant::now();
    RetentionNextAt::At(RetentionDeadline {
        database_at: next_at.database_at,
        monotonic_at: monotonic_now
            .checked_add(next_at.remaining)
            .unwrap_or(monotonic_now),
    })
}

async fn retention_phase_next_at(pool: &PgPool, phase: RetentionPhase) -> Result<RetentionNextAt> {
    let next_at = match phase {
        RetentionPhase::CoreMinuteMaterialization => {
            telemetry_minute_consumer_next_at(pool, TelemetryMinuteConsumer::Core).await?
        }
        RetentionPhase::TrafficMinuteMaterialization => {
            telemetry_minute_consumer_next_at(pool, TelemetryMinuteConsumer::Traffic).await?
        }
        RetentionPhase::DueEventCoalescing => optional_database_deadline(
            sqlx::query_as(
                r#"
                WITH frontier AS (
                    SELECT coalesce_ready_at AS database_at
                    FROM telemetry_history_due_events
                    ORDER BY coalesce_ready_at, event_id
                    LIMIT 1
                )
                SELECT database_at,
                       GREATEST(
                           EXTRACT(EPOCH FROM database_at - clock_timestamp()),
                           0
                       )::DOUBLE PRECISION AS remaining_seconds
                FROM frontier
                "#,
            )
            .fetch_optional(pool)
            .await?,
        )?,
        RetentionPhase::ResourcePromotion => {
            ordinary_due_spans_next_at(pool, RESOURCE_ROLLUP_DOMAIN).await?
        }
        RetentionPhase::NetworkRatePromotion => {
            ordinary_due_spans_next_at(pool, NETWORK_RATE_ROLLUP_DOMAIN).await?
        }
        RetentionPhase::PingPromotion => {
            ordinary_due_spans_next_at(pool, PING_ROLLUP_DOMAIN).await?
        }
        RetentionPhase::SystemMetricPromotion => {
            ordinary_due_spans_next_at(pool, SYSTEM_METRIC_ROLLUP_DOMAIN).await?
        }
        RetentionPhase::Traffic(traffic_phase) => {
            traffic_retention_phase_next_at(pool, traffic_phase).await?
        }
        RetentionPhase::NetworkObservation(observation_phase) => {
            let policy = load_policy(pool, "network_observations").await?;
            network_observation_retention_phase_next_at(
                pool,
                NetworkObservationRetentionPolicy {
                    enabled: policy.enabled,
                    retention_days: policy.retention_days,
                    prune_limit: policy.prune_limit,
                },
                observation_phase,
            )
            .await?
        }
        RetentionPhase::SamplePrune => {
            prune_domain_next_at(pool, "telemetry_samples", raw_sample_policy()).await?
        }
        RetentionPhase::PingFactsPrune => {
            ping_fact_prune_next_at(pool, raw_sample_policy()).await?
        }
        RetentionPhase::PingCurrentPrune | RetentionPhase::PingSeriesPrune => None,
        RetentionPhase::ResourcePrune => {
            let policy = load_policy(pool, "telemetry_rollups").await?;
            prune_domain_next_at(pool, "telemetry_rollups", policy).await?
        }
        RetentionPhase::NetworkRatePrune => {
            let policy = load_policy(pool, "telemetry_network_rates").await?;
            prune_domain_next_at(pool, "telemetry_network_rates", policy).await?
        }
        RetentionPhase::PingRollupPrune => {
            let policy = load_policy(pool, "telemetry_ping_rollups").await?;
            prune_domain_next_at(pool, "telemetry_ping_rollups", policy).await?
        }
        RetentionPhase::SystemMetricPrune => {
            let policy = load_policy(pool, "system_metric_rollups").await?;
            prune_domain_next_at(pool, "system_metric_rollups", policy).await?
        }
    };
    Ok(database_next_at(next_at))
}

async fn process_retention_phase(
    pool: &PgPool,
    phase: RetentionPhase,
) -> Result<RetentionPhaseExecution> {
    let mut run = TelemetryHistoryRetentionRun::default();
    let has_remaining_work;
    match phase {
        RetentionPhase::CoreMinuteMaterialization => {
            let minute = materialize_next_telemetry_minute(pool, TelemetryMinuteConsumer::Core)
                .await
                .context("materializing one natural core telemetry minute")?;
            run.core_minute_source_rows = minute.source_rows;
            run.core_minute_derived_rows = minute.derived_rows;
            if minute.owner_contended {
                return Ok(RetentionPhaseExecution {
                    run,
                    status: RetentionOwnerStatus::Current(RetentionNextAt::ProducerOnly),
                });
            }
            has_remaining_work =
                telemetry_minute_consumer_has_ready_work(pool, TelemetryMinuteConsumer::Core)
                    .await?;
        }
        RetentionPhase::TrafficMinuteMaterialization => {
            let minute = materialize_next_telemetry_minute(pool, TelemetryMinuteConsumer::Traffic)
                .await
                .context("materializing one natural traffic-counter minute")?;
            run.traffic_minute_source_rows = minute.source_rows;
            run.traffic_minute_derived_rows = minute.derived_rows;
            if minute.owner_contended {
                return Ok(RetentionPhaseExecution {
                    run,
                    status: RetentionOwnerStatus::Current(RetentionNextAt::ProducerOnly),
                });
            }
            has_remaining_work =
                telemetry_minute_consumer_has_ready_work(pool, TelemetryMinuteConsumer::Traffic)
                    .await?;
        }
        RetentionPhase::DueEventCoalescing => {
            let coalescing = coalesce_ready_telemetry_due_events(pool)
                .await
                .context("coalescing the ready telemetry history due-event frontier")?;
            run.due_events_coalesced = coalescing.coalesced;
            has_remaining_work = coalescing.has_remaining_work;
        }
        RetentionPhase::ResourcePromotion => {
            let promotion = promote_resource_rollups(pool)
                .await
                .context("promoting resource history tiers")?;
            run.resource_spans_merged = promotion.promotion.promoted;
            has_remaining_work = promotion.has_remaining_work;
        }
        RetentionPhase::NetworkRatePromotion => {
            let promotion = promote_network_rate_rollups(pool)
                .await
                .context("promoting network-rate history tiers")?;
            run.network_rate_spans_merged = promotion.promotion.promoted;
            has_remaining_work = promotion.has_remaining_work;
        }
        RetentionPhase::PingPromotion => {
            let promotion = promote_ping_rollups(pool)
                .await
                .context("promoting Ping history tiers")?;
            run.ping_spans_merged = promotion.promotion.promoted;
            has_remaining_work = promotion.has_remaining_work;
        }
        RetentionPhase::SystemMetricPromotion => {
            let promotion = promote_system_metric_rollups(pool)
                .await
                .context("promoting system-metric history tiers")?;
            run.system_metric_spans_merged = promotion.promotion.promoted;
            has_remaining_work = promotion.has_remaining_work;
        }
        RetentionPhase::Traffic(traffic_phase) => {
            let traffic = process_traffic_retention_phase(pool, traffic_phase)
                .await
                .context("processing one traffic retention phase")?;
            run.traffic_counter_samples_pruned = traffic.run.raw_rows_promoted;
            run.traffic_raw_rows_promoted = traffic.run.raw_rows_promoted;
            run.traffic_rollup_rows_promoted = traffic.run.rollup_rows_promoted;
            run.traffic_rollup_rows_pruned = traffic.run.rollup_rows_pruned;
            has_remaining_work = if let Some(remaining) = traffic.terminal_has_remaining_work {
                remaining
            } else if traffic.attempted {
                traffic_retention_phase_has_remaining_work(pool, traffic_phase)
                    .await
                    .context("checking one traffic retention phase frontier")?
            } else {
                false
            };
        }
        RetentionPhase::NetworkObservation(observation_phase) => {
            let policy = load_policy(pool, "network_observations").await?;
            let observation_policy = NetworkObservationRetentionPolicy {
                enabled: policy.enabled,
                retention_days: policy.retention_days,
                prune_limit: policy.prune_limit,
            };
            let observation = process_network_observation_retention_phase(
                pool,
                observation_policy,
                observation_phase,
            )
            .await
            .context("processing one network-observation retention phase")?;
            run.network_observation_source_rows_promoted = observation.run.source_rows_promoted;
            run.network_observation_destination_rows_written =
                observation.run.destination_rows_written;
            run.network_observation_expired_exact_rows_pruned =
                observation.run.expired_exact_rows_pruned;
            run.network_observation_expired_rollup_rows_pruned =
                observation.run.expired_rollup_rows_pruned;
            run.network_observation_inactive_latest_pruned = observation.run.inactive_latest_pruned;
            run.network_observation_inactive_series_pruned = observation.run.inactive_series_pruned;
            has_remaining_work = if observation.attempted {
                network_observation_retention_phase_has_remaining_work(
                    pool,
                    observation_policy,
                    observation_phase,
                )
                .await
                .context("checking one network-observation retention phase frontier")?
            } else {
                false
            };
        }
        RetentionPhase::SamplePrune => {
            let policy = raw_sample_policy();
            run.samples_pruned = prune_domain(pool, "telemetry_samples", policy).await?;
            has_remaining_work =
                prune_domain_has_remaining_work(pool, "telemetry_samples", policy).await?;
        }
        RetentionPhase::PingFactsPrune => {
            let policy = raw_sample_policy();
            run.ping_facts_pruned = prune_ping_fact_rows(pool, policy).await?;
            has_remaining_work = ping_fact_prune_has_remaining_work(pool, policy).await?;
        }
        RetentionPhase::PingCurrentPrune => {
            let policy = raw_sample_policy();
            run.ping_current_pruned = prune_ping_current(pool, policy).await?;
            has_remaining_work = ping_current_prune_has_remaining_work(pool, policy).await?;
        }
        RetentionPhase::PingSeriesPrune => {
            let policy = raw_sample_policy();
            run.ping_series_pruned = prune_ping_series(pool, policy).await?;
            has_remaining_work = ping_series_prune_has_remaining_work(pool, policy).await?;
        }
        RetentionPhase::ResourcePrune => {
            let policy = load_policy(pool, "telemetry_rollups").await?;
            run.rollups_pruned = prune_domain(pool, "telemetry_rollups", policy).await?;
            has_remaining_work =
                prune_domain_has_remaining_work(pool, "telemetry_rollups", policy).await?;
        }
        RetentionPhase::NetworkRatePrune => {
            let policy = load_policy(pool, "telemetry_network_rates").await?;
            run.network_rates_pruned =
                prune_domain(pool, "telemetry_network_rates", policy).await?;
            has_remaining_work =
                prune_domain_has_remaining_work(pool, "telemetry_network_rates", policy).await?;
        }
        RetentionPhase::PingRollupPrune => {
            let policy = load_policy(pool, "telemetry_ping_rollups").await?;
            run.ping_rollups_pruned = prune_domain(pool, "telemetry_ping_rollups", policy).await?;
            has_remaining_work =
                prune_domain_has_remaining_work(pool, "telemetry_ping_rollups", policy).await?;
        }
        RetentionPhase::SystemMetricPrune => {
            let policy = load_policy(pool, "system_metric_rollups").await?;
            run.system_metric_rollups_pruned =
                prune_domain(pool, "system_metric_rollups", policy).await?;
            has_remaining_work =
                prune_domain_has_remaining_work(pool, "system_metric_rollups", policy).await?;
        }
    }
    let status = if has_remaining_work {
        RetentionOwnerStatus::StillDue
    } else {
        RetentionOwnerStatus::Current(retention_phase_next_at(pool, phase).await?)
    };
    Ok(RetentionPhaseExecution { run, status })
}

fn raw_sample_policy() -> RetentionPolicy {
    RetentionPolicy {
        enabled: true,
        prune_limit: DEFAULT_TELEMETRY_RETENTION_PRUNE_LIMIT,
        retention_days: DEFAULT_TELEMETRY_SAMPLE_RETENTION_DAYS,
    }
}

/// One registry rotation retained only as a focused kernel-test helper. The
/// production worker always enters through `TelemetryHistoryRetentionDrain`
/// and therefore executes one bounded logical phase per external page.
#[cfg(test)]
pub(crate) async fn process_telemetry_history_retention(
    pool: &PgPool,
) -> Result<TelemetryHistoryRetentionRun> {
    let mut total = TelemetryHistoryRetentionRun::default();
    for owner in RETENTION_PHASES {
        total.merge(process_retention_phase(pool, owner.phase).await?.run);
    }
    Ok(total)
}

async fn promote_resource_rollups(pool: &PgPool) -> Result<OrdinaryPromotionResult> {
    let mut tx = pool.begin().await?;
    let Some(span) = claim_ordinary_due_span(&mut tx, RESOURCE_ROLLUP_DOMAIN).await? else {
        tx.commit().await?;
        return Ok(OrdinaryPromotionResult {
            has_remaining_work: ordinary_due_spans_have_remaining_work(
                pool,
                RESOURCE_ROLLUP_DOMAIN,
            )
            .await?,
            ..OrdinaryPromotionResult::default()
        });
    };
    let lower_bucket_secs = ordinary_due_span_lower_tiers(&span)?;
    let owner = exact_owner_identity(&span, RESOURCE_ROLLUP_DOMAIN, 1)?;
    let result = promote_resource_tier_in_tx(
        &mut tx,
        span.destination_bucket_secs,
        span.source_bucket_secs,
        span.destination_start,
        &lower_bucket_secs,
        &owner[0],
    )
    .await?;
    if result.complete {
        delete_completed_ordinary_due_span(&mut tx, &span).await?;
    }
    tx.commit().await?;
    Ok(OrdinaryPromotionResult {
        promotion: result.promotion,
        has_remaining_work: ordinary_due_spans_have_remaining_work(pool, RESOURCE_ROLLUP_DOMAIN)
            .await?,
    })
}

#[cfg(test)]
async fn promote_resource_tier(
    pool: &PgPool,
    destination_secs: i32,
    source_bucket_secs: i32,
    source_days: i32,
    lower_bucket_secs: &[i32],
) -> Result<PromotionResult> {
    let mut tx = pool.begin().await?;
    let coordinate = sqlx::query_as::<_, (DateTime<Utc>, String)>(
        r#"
        SELECT to_timestamp(floor(extract(epoch FROM bucket_start) / $1) * $1),
               client_id
        FROM telemetry_rollups
        WHERE bucket_secs = $2
          AND bucket_start < to_timestamp(floor(extract(epoch FROM (
                now() - make_interval(days => $3))) / $1) * $1)
        ORDER BY bucket_start DESC, client_id
        LIMIT 1
        "#,
    )
    .bind(destination_secs)
    .bind(source_bucket_secs)
    .bind(source_days)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((destination_start, client_id)) = coordinate else {
        tx.commit().await?;
        return Ok(PromotionResult::default());
    };
    let result = promote_resource_tier_in_tx(
        &mut tx,
        destination_secs,
        source_bucket_secs,
        destination_start,
        lower_bucket_secs,
        &client_id,
    )
    .await?;
    tx.commit().await?;
    Ok(result.promotion)
}

async fn promote_resource_tier_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    destination_secs: i32,
    source_bucket_secs: i32,
    destination_start: DateTime<Utc>,
    lower_bucket_secs: &[i32],
    client_id: &str,
) -> Result<ResourcePromotionResult> {
    // Promotion replaces one natural tier span. Dashboard publication owns
    // the exact inserted/deleted coordinates; full-block repair is reserved
    // for arbitrary prune pages.
    let result = sqlx::query(
        r#"
        WITH span_rows AS MATERIALIZED (
            SELECT row.ctid AS source_ctid, row.*
            FROM telemetry_rollups row
            WHERE row.bucket_secs = ANY($4::integer[])
              AND row.client_id = $5
              AND row.bucket_start >= $3::timestamptz
              AND row.bucket_start < $3::timestamptz
                    + make_interval(secs => $1)
        ), ordered_rows AS MATERIALIZED (
            SELECT row.*, $3::timestamptz AS destination_start,
                bool_or(row.bucket_secs = $2) OVER (
                    PARTITION BY row.client_id
                ) AS has_immediate_source,
                row_number() OVER (
                    PARTITION BY row.client_id
                    ORDER BY row.bucket_start, row.bucket_secs
                ) AS group_ordinal,
                lag(row.bucket_start + make_interval(secs => row.bucket_secs)) OVER (
                    PARTITION BY row.client_id
                    ORDER BY row.bucket_start, row.bucket_secs
                ) AS previous_end
            FROM span_rows row
        ), candidate_rows AS MATERIALIZED (
            SELECT row.*
            FROM ordered_rows row
            WHERE row.has_immediate_source
        ), annotated_rows AS MATERIALIZED (
            SELECT row.*,
                count(*) OVER (
                    PARTITION BY row.client_id, row.destination_start
                )::bigint AS source_rows,
                bool_and(
                    (row.previous_end IS NULL OR row.previous_end <= row.bucket_start)
                    AND row.bucket_start + make_interval(secs => row.bucket_secs)
                        <= row.destination_start + make_interval(secs => $1)
                ) OVER (
                    PARTITION BY row.client_id, row.destination_start
                ) AS non_overlapping
            FROM candidate_rows row
        ), destination_checked_rows AS MATERIALIZED (
            SELECT row.*,
                CASE WHEN row.group_ordinal = 1
                           AND row.non_overlapping THEN EXISTS (
                    SELECT 1 FROM telemetry_rollups destination
                    WHERE destination.client_id = row.client_id
                      AND destination.bucket_secs = $1
                      AND destination.bucket_start = row.destination_start
                ) ELSE NULL END AS destination_probe
            FROM annotated_rows row
        ), publication_rows AS MATERIALIZED (
            SELECT row.*,
                COALESCE(bool_or(row.destination_probe) OVER (
                    PARTITION BY row.client_id, row.destination_start
                ), FALSE) AS destination_exists
            FROM destination_checked_rows row
        ), overlap_conflicts AS MATERIALIZED (
            SELECT DISTINCT row.client_id, row.destination_start
            FROM publication_rows row
            WHERE NOT row.non_overlapping
        ), preexisting_destination_conflicts AS MATERIALIZED (
            SELECT DISTINCT row.client_id, row.destination_start
            FROM publication_rows row
            WHERE row.non_overlapping AND row.destination_exists
        ), eligible_rows AS MATERIALIZED (
            SELECT row.*
            FROM publication_rows row
            WHERE row.non_overlapping AND NOT row.destination_exists
        ), locked_rows AS MATERIALIZED (
            SELECT eligible.*
            FROM telemetry_rollups row
            JOIN eligible_rows eligible ON eligible.source_ctid = row.ctid
            ORDER BY eligible.destination_start, eligible.client_id,
                eligible.bucket_start, eligible.bucket_secs
            FOR UPDATE OF row SKIP LOCKED
        ), counted_locked_rows AS MATERIALIZED (
            SELECT row.*,
                count(*) OVER (
                    PARTITION BY row.client_id, row.destination_start
                )::bigint AS locked_source_rows
            FROM locked_rows row
        ), completion_rows AS MATERIALIZED (
            SELECT row.*,
                row.locked_source_rows = row.source_rows AS group_complete
            FROM counted_locked_rows row
        ), source AS MATERIALIZED (
            SELECT row.*
            FROM completion_rows row
            WHERE row.group_complete
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
                connections_sample_count,
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
                LEAST(sum(connections_sample_count)::bigint, 2147483647)::integer,
                (array_agg(tcp_sockets_latest ORDER BY connections_observed_at DESC)
                    FILTER (WHERE connections_observed_at IS NOT NULL))[1],
                (array_agg(udp_sockets_latest ORDER BY connections_observed_at DESC)
                    FILTER (WHERE connections_observed_at IS NOT NULL))[1],
                max(connections_observed_at), max(latest_observed_at), max(updated_at)
            FROM source GROUP BY client_id, destination_start
            ON CONFLICT (client_id, bucket_secs, bucket_start) DO NOTHING
            RETURNING client_id, bucket_start
        ), insertion_checked_rows AS MATERIALIZED (
            SELECT source.*,
                CASE WHEN source.group_ordinal = 1 THEN EXISTS (
                    SELECT 1 FROM inserted
                    WHERE inserted.client_id = source.client_id
                      AND inserted.bucket_start = source.destination_start
                ) ELSE NULL END AS inserted_probe
            FROM source
        ), insertion_rows AS MATERIALIZED (
            SELECT row.*,
                COALESCE(bool_or(row.inserted_probe) OVER (
                    PARTITION BY row.client_id, row.destination_start
                ), FALSE) AS inserted_succeeded
            FROM insertion_checked_rows row
        ), deleted AS (
            DELETE FROM telemetry_rollups row USING insertion_rows source
            WHERE row.ctid = source.source_ctid
              AND source.inserted_succeeded
            RETURNING row.ctid, row.bucket_secs
        ), destination_conflicts AS MATERIALIZED (
            SELECT conflict.client_id, conflict.destination_start
            FROM preexisting_destination_conflicts conflict
            UNION
            SELECT DISTINCT row.client_id, row.destination_start
            FROM insertion_rows row
            WHERE NOT row.inserted_succeeded
        ), conflicts AS MATERIALIZED (
            SELECT client_id, destination_start FROM overlap_conflicts
            UNION
            SELECT client_id, destination_start FROM destination_conflicts
        )
        SELECT
            (SELECT count(*)::bigint FROM inserted) AS promoted,
            (SELECT count(*)::bigint FROM conflicts) AS conflicts,
            (SELECT count(*)::bigint FROM candidate_rows) AS examined_source_rows,
            (SELECT count(*)::bigint FROM deleted) AS source_rows,
            (SELECT count(*)::bigint FROM candidate_rows
             WHERE bucket_secs = $2) AS immediate_source_rows,
            (SELECT count(*)::bigint FROM deleted
             WHERE bucket_secs = $2) AS deleted_immediate_source_rows
        "#,
    )
    .bind(destination_secs)
    .bind(source_bucket_secs)
    .bind(destination_start)
    .bind(lower_bucket_secs)
    .bind(client_id)
    .fetch_one(&mut **tx)
    .await?;
    let promoted = result.try_get::<i64, _>("promoted")?.max(0) as u64;
    reject_promotion_conflicts(
        "telemetry_rollups",
        source_bucket_secs,
        destination_secs,
        result.try_get("conflicts")?,
    )?;
    let immediate_source_rows = result.try_get::<i64, _>("immediate_source_rows")?.max(0) as u64;
    let deleted_immediate_source_rows = result
        .try_get::<i64, _>("deleted_immediate_source_rows")?
        .max(0) as u64;
    Ok(ResourcePromotionResult {
        promotion: PromotionResult {
            promoted,
            #[cfg(test)]
            examined_source_rows: result.try_get::<i64, _>("examined_source_rows")?.max(0) as u64,
            #[cfg(test)]
            source_rows: result.try_get::<i64, _>("source_rows")?.max(0) as u64,
        },
        complete: immediate_source_rows == deleted_immediate_source_rows,
    })
}

async fn promote_network_rate_rollups(pool: &PgPool) -> Result<OrdinaryPromotionResult> {
    let mut tx = pool.begin().await?;
    let Some(span) = claim_ordinary_due_span(&mut tx, NETWORK_RATE_ROLLUP_DOMAIN).await? else {
        tx.commit().await?;
        return Ok(OrdinaryPromotionResult {
            has_remaining_work: ordinary_due_spans_have_remaining_work(
                pool,
                NETWORK_RATE_ROLLUP_DOMAIN,
            )
            .await?,
            ..OrdinaryPromotionResult::default()
        });
    };
    let lower_bucket_secs = ordinary_due_span_lower_tiers(&span)?;
    let owner = exact_owner_identity(&span, NETWORK_RATE_ROLLUP_DOMAIN, 2)?;
    let result = promote_network_rate_tier_in_tx(
        &mut tx,
        span.destination_bucket_secs,
        span.source_bucket_secs,
        span.destination_start,
        &lower_bucket_secs,
        &owner[0],
        &owner[1],
    )
    .await?;
    if result.complete {
        delete_completed_ordinary_due_span(&mut tx, &span).await?;
    }
    tx.commit().await?;
    Ok(OrdinaryPromotionResult {
        promotion: result.promotion,
        has_remaining_work: ordinary_due_spans_have_remaining_work(
            pool,
            NETWORK_RATE_ROLLUP_DOMAIN,
        )
        .await?,
    })
}

#[cfg(test)]
async fn promote_network_rate_tier(
    pool: &PgPool,
    destination_secs: i32,
    source_bucket_secs: i32,
    source_days: i32,
    lower_bucket_secs: &[i32],
) -> Result<PromotionResult> {
    let mut tx = pool.begin().await?;
    let coordinate = sqlx::query_as::<_, (DateTime<Utc>, String, String)>(
        r#"
        SELECT to_timestamp(floor(extract(epoch FROM bucket_start) / $1) * $1),
               client_id, interface
        FROM telemetry_network_rates
        WHERE bucket_secs = $2
          AND bucket_start < to_timestamp(floor(extract(epoch FROM (
                now() - make_interval(days => $3))) / $1) * $1)
        ORDER BY bucket_start DESC, client_id, interface
        LIMIT 1
        "#,
    )
    .bind(destination_secs)
    .bind(source_bucket_secs)
    .bind(source_days)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((destination_start, client_id, interface)) = coordinate else {
        tx.commit().await?;
        return Ok(PromotionResult::default());
    };
    let result = promote_network_rate_tier_in_tx(
        &mut tx,
        destination_secs,
        source_bucket_secs,
        destination_start,
        lower_bucket_secs,
        &client_id,
        &interface,
    )
    .await?;
    tx.commit().await?;
    Ok(result.promotion)
}

async fn promote_network_rate_tier_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    destination_secs: i32,
    source_bucket_secs: i32,
    destination_start: DateTime<Utc>,
    lower_bucket_secs: &[i32],
    client_id: &str,
    interface: &str,
) -> Result<NetworkRatePromotionResult> {
    // This transaction replaces one natural tier span. Dashboard publication
    // owns the exact inserted/deleted coordinates; full-block repair is
    // reserved for arbitrary prune pages. The bounded network-current head is
    // intentionally not rebuilt by retained-history promotion.
    let result = sqlx::query(
        r#"
        WITH span_rows AS MATERIALIZED (
            SELECT row.tableoid AS source_tableoid,
                   row.ctid AS source_ctid,
                   row.*
            FROM telemetry_network_rates row
            WHERE row.bucket_secs = ANY($4::integer[])
              AND row.client_id = $5
              AND row.interface = $6
              AND row.bucket_start >= $3::timestamptz
              AND row.bucket_start < $3::timestamptz
                    + make_interval(secs => $1)
        ), ordered_source AS MATERIALIZED (
            SELECT row.*, $3::timestamptz AS destination_start,
                bool_or(row.bucket_secs = $2) OVER (
                    PARTITION BY row.client_id, row.interface
                ) AS has_immediate_source,
                row_number() OVER (
                    PARTITION BY row.client_id, row.interface
                    ORDER BY row.bucket_start, row.bucket_secs
                ) AS group_ordinal,
                lag(row.bucket_start + make_interval(secs => row.bucket_secs)) OVER (
                    PARTITION BY row.client_id, row.interface
                    ORDER BY row.bucket_start, row.bucket_secs
                ) AS previous_end
            FROM span_rows row
        ), candidate_rows AS MATERIALIZED (
            SELECT row.*
            FROM ordered_source row
            WHERE row.has_immediate_source
        ), annotated_rows AS MATERIALIZED (
            SELECT row.*,
                count(*) OVER (
                    PARTITION BY row.client_id, row.interface,
                                 row.destination_start
                )::bigint AS source_rows,
                bool_and(
                    (row.previous_end IS NULL OR row.previous_end <= row.bucket_start)
                    AND row.bucket_start + make_interval(secs => row.bucket_secs)
                        <= row.destination_start + make_interval(secs => $1)
                ) OVER (
                    PARTITION BY row.client_id, row.interface,
                                 row.destination_start
                ) AS non_overlapping
            FROM candidate_rows row
        ), destination_checked_rows AS MATERIALIZED (
            SELECT row.*,
                CASE WHEN row.group_ordinal = 1
                           AND row.non_overlapping THEN EXISTS (
                    SELECT 1 FROM telemetry_network_rates destination
                    WHERE destination.client_id = row.client_id
                      AND destination.interface = row.interface
                      AND destination.bucket_secs = $1
                      AND destination.bucket_start = row.destination_start
                ) ELSE NULL END AS destination_probe
            FROM annotated_rows row
        ), publication_rows AS MATERIALIZED (
            SELECT row.*,
                COALESCE(bool_or(row.destination_probe) OVER (
                    PARTITION BY row.client_id, row.interface,
                                 row.destination_start
                ), FALSE) AS destination_exists
            FROM destination_checked_rows row
        ), overlap_conflicts AS MATERIALIZED (
            SELECT DISTINCT row.client_id, row.interface,
                            row.destination_start
            FROM publication_rows row
            WHERE NOT row.non_overlapping
        ), preexisting_destination_conflicts AS MATERIALIZED (
            SELECT DISTINCT row.client_id, row.interface,
                            row.destination_start
            FROM publication_rows row
            WHERE row.non_overlapping AND row.destination_exists
        ), eligible_rows AS MATERIALIZED (
            SELECT row.*
            FROM publication_rows row
            WHERE row.non_overlapping AND NOT row.destination_exists
        ), locked_source AS MATERIALIZED (
            SELECT eligible.*
            FROM telemetry_network_rates row
            JOIN eligible_rows eligible
              ON eligible.source_tableoid = row.tableoid
             AND eligible.source_ctid = row.ctid
            ORDER BY eligible.destination_start, eligible.client_id,
                eligible.interface, eligible.bucket_start,
                eligible.bucket_secs
            FOR UPDATE OF row SKIP LOCKED
        ), counted_locked_rows AS MATERIALIZED (
            SELECT row.*,
                count(*) OVER (
                    PARTITION BY row.client_id, row.interface,
                                 row.destination_start
                )::bigint AS locked_source_rows
            FROM locked_source row
        ), completion_rows AS MATERIALIZED (
            SELECT row.*,
                row.locked_source_rows = row.source_rows AS group_complete
            FROM counted_locked_rows row
        ), source AS MATERIALIZED (
            SELECT row.*
            FROM completion_rows row
            WHERE row.group_complete
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
        ), insertion_checked_rows AS MATERIALIZED (
            SELECT source.*,
                CASE WHEN source.group_ordinal = 1 THEN EXISTS (
                    SELECT 1 FROM inserted
                    WHERE inserted.client_id = source.client_id
                      AND inserted.interface = source.interface
                      AND inserted.bucket_start = source.destination_start
                ) ELSE NULL END AS inserted_probe
            FROM source
        ), insertion_rows AS MATERIALIZED (
            SELECT row.*,
                COALESCE(bool_or(row.inserted_probe) OVER (
                    PARTITION BY row.client_id, row.interface,
                                 row.destination_start
                ), FALSE) AS inserted_succeeded
            FROM insertion_checked_rows row
        ), deleted AS (
            DELETE FROM telemetry_network_rates row USING insertion_rows source
            WHERE row.tableoid = source.source_tableoid
              AND row.ctid = source.source_ctid
              AND source.inserted_succeeded
            RETURNING row.tableoid, row.ctid, row.bucket_secs
        ), destination_conflicts AS MATERIALIZED (
            SELECT conflict.client_id, conflict.interface,
                   conflict.destination_start
            FROM preexisting_destination_conflicts conflict
            UNION
            SELECT DISTINCT row.client_id, row.interface,
                            row.destination_start
            FROM insertion_rows row
            WHERE NOT row.inserted_succeeded
        ), conflicts AS MATERIALIZED (
            SELECT client_id, interface, destination_start FROM overlap_conflicts
            UNION
            SELECT client_id, interface, destination_start FROM destination_conflicts
        ) SELECT
            (SELECT count(*)::bigint FROM inserted) AS promoted,
            (SELECT count(*)::bigint FROM conflicts) AS conflicts,
            (SELECT count(*)::bigint FROM candidate_rows) AS examined_source_rows,
            (SELECT count(*)::bigint FROM deleted) AS source_rows,
            (SELECT count(*)::bigint FROM candidate_rows
             WHERE bucket_secs = $2) AS immediate_source_rows,
            (SELECT count(*)::bigint FROM deleted
             WHERE bucket_secs = $2) AS deleted_immediate_source_rows
    "#,
    )
    .bind(destination_secs)
    .bind(source_bucket_secs)
    .bind(destination_start)
    .bind(lower_bucket_secs)
    .bind(client_id)
    .bind(interface)
    .fetch_one(&mut **tx)
    .await?;
    let promoted = result.try_get::<i64, _>("promoted")?.max(0) as u64;
    reject_promotion_conflicts(
        "telemetry_network_rates",
        source_bucket_secs,
        destination_secs,
        result.try_get("conflicts")?,
    )?;
    let immediate_source_rows = result.try_get::<i64, _>("immediate_source_rows")?.max(0) as u64;
    let deleted_immediate_source_rows = result
        .try_get::<i64, _>("deleted_immediate_source_rows")?
        .max(0) as u64;
    Ok(NetworkRatePromotionResult {
        promotion: PromotionResult {
            promoted,
            #[cfg(test)]
            examined_source_rows: result.try_get::<i64, _>("examined_source_rows")?.max(0) as u64,
            #[cfg(test)]
            source_rows: result.try_get::<i64, _>("source_rows")?.max(0) as u64,
        },
        complete: immediate_source_rows == deleted_immediate_source_rows,
    })
}

async fn promote_ping_rollups(pool: &PgPool) -> Result<OrdinaryPromotionResult> {
    let mut tx = pool.begin().await?;
    let Some(span) = claim_ordinary_due_span(&mut tx, PING_ROLLUP_DOMAIN).await? else {
        tx.commit().await?;
        return Ok(OrdinaryPromotionResult {
            has_remaining_work: ordinary_due_spans_have_remaining_work(pool, PING_ROLLUP_DOMAIN)
                .await?,
            ..OrdinaryPromotionResult::default()
        });
    };
    let lower_bucket_secs = ordinary_due_span_lower_tiers(&span)?;
    let owner = exact_owner_identity(&span, PING_ROLLUP_DOMAIN, 1)?;
    let series_id = owner[0]
        .parse::<i64>()
        .context("decoding Ping due-span series owner")?;
    let result = promote_ping_tier_in_tx(
        &mut tx,
        span.destination_bucket_secs,
        span.source_bucket_secs,
        span.destination_start,
        &lower_bucket_secs,
        series_id,
    )
    .await?;
    if result.complete {
        delete_completed_ordinary_due_span(&mut tx, &span).await?;
    }
    tx.commit().await?;
    Ok(OrdinaryPromotionResult {
        promotion: result.promotion,
        has_remaining_work: ordinary_due_spans_have_remaining_work(pool, PING_ROLLUP_DOMAIN)
            .await?,
    })
}

#[cfg(test)]
async fn promote_ping_tier(
    pool: &PgPool,
    destination_secs: i32,
    source_bucket_secs: i32,
    source_days: i32,
    lower_bucket_secs: &[i32],
) -> Result<PingPromotionResult> {
    let mut tx = pool.begin().await?;
    let coordinate = sqlx::query_as::<_, (DateTime<Utc>, i64)>(
        r#"
        SELECT to_timestamp(floor(extract(epoch FROM bucket_start) / $1) * $1),
               series_id
        FROM telemetry_ping_rollups
        WHERE bucket_secs = $2
          AND bucket_secs < 86400
          AND bucket_start < to_timestamp(floor(extract(epoch FROM (
                now() - make_interval(days => $3))) / $1) * $1)
        ORDER BY bucket_start DESC, series_id
        LIMIT 1
        "#,
    )
    .bind(destination_secs)
    .bind(source_bucket_secs)
    .bind(source_days)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((destination_start, series_id)) = coordinate else {
        tx.commit().await?;
        return Ok(PingPromotionResult::default());
    };
    let result = promote_ping_tier_in_tx(
        &mut tx,
        destination_secs,
        source_bucket_secs,
        destination_start,
        lower_bucket_secs,
        series_id,
    )
    .await?;
    tx.commit().await?;
    Ok(result)
}

async fn promote_ping_tier_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    destination_secs: i32,
    source_bucket_secs: i32,
    destination_start: DateTime<Utc>,
    lower_bucket_secs: &[i32],
    series_id: i64,
) -> Result<PingPromotionResult> {
    let result = sqlx::query(
        r#"
        WITH span_rows AS MATERIALIZED (
            SELECT row.ctid AS source_ctid, row.*
            FROM telemetry_ping_rollups row
            WHERE row.bucket_secs = ANY($4::integer[])
              AND row.series_id = $5
              AND row.bucket_start >= $3::timestamptz
              AND row.bucket_start < $3::timestamptz
                    + make_interval(secs => $1)
        ), ordered_source AS MATERIALIZED (
            SELECT row.*, $3::timestamptz AS destination_start,
                bool_or(row.bucket_secs = $2) OVER (
                    PARTITION BY row.series_id
                ) AS has_immediate_source,
                row_number() OVER (
                    PARTITION BY row.series_id
                    ORDER BY row.bucket_start, row.bucket_secs
                ) AS group_ordinal,
                lag(row.bucket_start + make_interval(secs => row.bucket_secs)) OVER (
                    PARTITION BY row.series_id
                    ORDER BY row.bucket_start, row.bucket_secs
                ) AS previous_end
            FROM span_rows row
        ), unlocked_candidate_rows AS MATERIALIZED (
            SELECT row.*
            FROM ordered_source row
            WHERE row.has_immediate_source
        ), candidate_series AS MATERIALIZED (
            SELECT DISTINCT row.series_id
            FROM unlocked_candidate_rows row
        ), locked_candidate_series AS MATERIALIZED (
            SELECT series.id
            FROM candidate_series candidate
            JOIN telemetry_ping_series series ON series.id = candidate.series_id
            ORDER BY series.id
            FOR NO KEY UPDATE OF series SKIP LOCKED
        ), candidate_rows AS MATERIALIZED (
            SELECT candidate.*
            FROM unlocked_candidate_rows candidate
            JOIN locked_candidate_series series
              ON series.id = candidate.series_id
        ), annotated_rows AS MATERIALIZED (
            SELECT row.*,
                count(*) OVER (
                    PARTITION BY row.series_id, row.destination_start
                )::bigint AS source_rows,
                bool_and(
                    (row.previous_end IS NULL OR row.previous_end <= row.bucket_start)
                    AND row.bucket_start + make_interval(secs => row.bucket_secs)
                        <= row.destination_start + make_interval(secs => $1)
                ) OVER (
                    PARTITION BY row.series_id, row.destination_start
                ) AS non_overlapping
            FROM candidate_rows row
        ), destination_checked_rows AS MATERIALIZED (
            SELECT row.*,
                CASE WHEN row.group_ordinal = 1
                           AND row.non_overlapping THEN EXISTS (
                    SELECT 1 FROM telemetry_ping_rollups destination
                    WHERE destination.series_id = row.series_id
                      AND destination.bucket_secs = $1
                      AND destination.bucket_start = row.destination_start
                ) ELSE NULL END AS destination_probe
            FROM annotated_rows row
        ), publication_rows AS MATERIALIZED (
            SELECT row.*,
                COALESCE(bool_or(row.destination_probe) OVER (
                    PARTITION BY row.series_id, row.destination_start
                ), FALSE) AS destination_exists
            FROM destination_checked_rows row
        ), overlap_conflicts AS MATERIALIZED (
            SELECT DISTINCT row.series_id, row.destination_start
            FROM publication_rows row
            WHERE NOT row.non_overlapping
        ), preexisting_destination_conflicts AS MATERIALIZED (
            SELECT DISTINCT row.series_id, row.destination_start
            FROM publication_rows row
            WHERE row.non_overlapping AND row.destination_exists
        ), eligible_rows AS MATERIALIZED (
            SELECT row.*
            FROM publication_rows row
            WHERE row.non_overlapping AND NOT row.destination_exists
        ), locked_source AS MATERIALIZED (
            SELECT eligible.*
            FROM telemetry_ping_rollups row
            JOIN eligible_rows eligible ON eligible.source_ctid = row.ctid
            ORDER BY eligible.destination_start, eligible.series_id,
                eligible.bucket_start, eligible.bucket_secs
            FOR UPDATE OF row SKIP LOCKED
        ), counted_locked_rows AS MATERIALIZED (
            SELECT row.*,
                count(*) OVER (
                    PARTITION BY row.series_id, row.destination_start
                )::bigint AS locked_source_rows
            FROM locked_source row
        ), completion_rows AS MATERIALIZED (
            SELECT row.*,
                row.locked_source_rows = row.source_rows AS group_complete
            FROM counted_locked_rows row
        ), source AS MATERIALIZED (
            SELECT row.*
            FROM completion_rows row
            WHERE row.group_complete
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
        ), insertion_checked_rows AS MATERIALIZED (
            SELECT source.*,
                CASE WHEN source.group_ordinal = 1 THEN EXISTS (
                    SELECT 1 FROM inserted
                    WHERE inserted.series_id = source.series_id
                      AND inserted.bucket_start = source.destination_start
                ) ELSE NULL END AS inserted_probe
            FROM source
        ), insertion_rows AS MATERIALIZED (
            SELECT row.*,
                COALESCE(bool_or(row.inserted_probe) OVER (
                    PARTITION BY row.series_id, row.destination_start
                ), FALSE) AS inserted_succeeded
            FROM insertion_checked_rows row
        ), deleted AS (
            DELETE FROM telemetry_ping_rollups row USING insertion_rows source
            WHERE row.ctid = source.source_ctid
              AND source.inserted_succeeded
            RETURNING row.ctid, row.bucket_secs
        ), destination_conflicts AS MATERIALIZED (
            SELECT conflict.series_id, conflict.destination_start
            FROM preexisting_destination_conflicts conflict
            UNION
            SELECT DISTINCT row.series_id, row.destination_start
            FROM insertion_rows row
            WHERE NOT row.inserted_succeeded
        ), conflicts AS MATERIALIZED (
            SELECT series_id, destination_start FROM overlap_conflicts
            UNION
            SELECT series_id, destination_start FROM destination_conflicts
        ) SELECT
            (SELECT count(*)::bigint FROM inserted) AS promoted,
            (SELECT count(*)::bigint FROM conflicts) AS conflicts,
            (SELECT count(*)::bigint FROM span_rows) AS examined_source_rows,
            (SELECT count(*)::bigint FROM deleted) AS source_rows,
            (SELECT count(*)::bigint FROM span_rows
             WHERE bucket_secs = $2) AS immediate_source_rows,
            (SELECT count(*)::bigint FROM deleted
             WHERE bucket_secs = $2) AS deleted_immediate_source_rows
    "#,
    )
    .bind(destination_secs)
    .bind(source_bucket_secs)
    .bind(destination_start)
    .bind(lower_bucket_secs)
    .bind(series_id)
    .fetch_one(&mut **tx)
    .await?;
    let promoted = result.try_get::<i64, _>("promoted")?.max(0) as u64;
    reject_promotion_conflicts(
        "telemetry_ping_rollups",
        source_bucket_secs,
        destination_secs,
        result.try_get("conflicts")?,
    )?;
    let immediate_source_rows = result.try_get::<i64, _>("immediate_source_rows")?.max(0) as u64;
    let deleted_immediate_source_rows = result
        .try_get::<i64, _>("deleted_immediate_source_rows")?
        .max(0) as u64;
    Ok(PingPromotionResult {
        promotion: PromotionResult {
            promoted,
            #[cfg(test)]
            examined_source_rows: result.try_get::<i64, _>("examined_source_rows")?.max(0) as u64,
            #[cfg(test)]
            source_rows: result.try_get::<i64, _>("source_rows")?.max(0) as u64,
        },
        complete: immediate_source_rows == deleted_immediate_source_rows,
    })
}

async fn promote_system_metric_rollups(pool: &PgPool) -> Result<OrdinaryPromotionResult> {
    let mut tx = pool.begin().await?;
    let Some(span) = claim_ordinary_due_span(&mut tx, SYSTEM_METRIC_ROLLUP_DOMAIN).await? else {
        tx.commit().await?;
        return Ok(OrdinaryPromotionResult {
            has_remaining_work: ordinary_due_spans_have_remaining_work(
                pool,
                SYSTEM_METRIC_ROLLUP_DOMAIN,
            )
            .await?,
            ..OrdinaryPromotionResult::default()
        });
    };
    let lower_bucket_secs = ordinary_due_span_lower_tiers(&span)?;
    let owner = exact_owner_identity(&span, SYSTEM_METRIC_ROLLUP_DOMAIN, 1)?;
    let result = promote_system_metric_tier_in_tx(
        &mut tx,
        span.destination_bucket_secs,
        span.source_bucket_secs,
        span.destination_start,
        &lower_bucket_secs,
        &owner[0],
    )
    .await?;
    if result.complete {
        delete_completed_ordinary_due_span(&mut tx, &span).await?;
    }
    tx.commit().await?;
    Ok(OrdinaryPromotionResult {
        promotion: result.promotion,
        has_remaining_work: ordinary_due_spans_have_remaining_work(
            pool,
            SYSTEM_METRIC_ROLLUP_DOMAIN,
        )
        .await?,
    })
}

async fn promote_system_metric_tier_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    destination_secs: i32,
    source_bucket_secs: i32,
    destination_start: DateTime<Utc>,
    lower_bucket_secs: &[i32],
    metric: &str,
) -> Result<SystemMetricPromotionResult> {
    let result = sqlx::query(
        r#"
        WITH span_rows AS MATERIALIZED (
            SELECT row.ctid AS source_ctid, row.*
            FROM system_metric_rollups row
            WHERE row.bucket_secs = ANY($4::integer[])
              AND row.metric = $5
              AND row.bucket_start >= $3::timestamptz
              AND row.bucket_start < $3::timestamptz
                    + make_interval(secs => $1)
        ), ordered_rows AS MATERIALIZED (
            SELECT row.*, $3::timestamptz AS destination_start,
                bool_or(row.bucket_secs = $2) OVER (
                    PARTITION BY row.metric
                ) AS has_immediate_source,
                row_number() OVER (
                    PARTITION BY row.metric
                    ORDER BY row.bucket_start, row.bucket_secs
                ) AS group_ordinal,
                lag(row.bucket_start + make_interval(secs => row.bucket_secs)) OVER (
                    PARTITION BY row.metric
                    ORDER BY row.bucket_start, row.bucket_secs
                ) AS previous_end
            FROM span_rows row
        ), candidate_rows AS MATERIALIZED (
            SELECT row.*
            FROM ordered_rows row
            WHERE row.has_immediate_source
        ), annotated_rows AS MATERIALIZED (
            SELECT row.*,
                count(*) OVER (
                    PARTITION BY row.metric, row.destination_start
                )::bigint AS source_rows,
                bool_and(
                    (row.previous_end IS NULL OR row.previous_end <= row.bucket_start)
                    AND row.bucket_start + make_interval(secs => row.bucket_secs)
                        <= row.destination_start + make_interval(secs => $1)
                ) OVER (
                    PARTITION BY row.metric, row.destination_start
                ) AS non_overlapping
            FROM candidate_rows row
        ), destination_checked_rows AS MATERIALIZED (
            SELECT row.*,
                CASE WHEN row.group_ordinal = 1
                           AND row.non_overlapping THEN EXISTS (
                    SELECT 1 FROM system_metric_rollups destination
                    WHERE destination.metric = row.metric
                      AND destination.bucket_secs = $1
                      AND destination.bucket_start = row.destination_start
                ) ELSE NULL END AS destination_probe
            FROM annotated_rows row
        ), publication_rows AS MATERIALIZED (
            SELECT row.*,
                COALESCE(bool_or(row.destination_probe) OVER (
                    PARTITION BY row.metric, row.destination_start
                ), FALSE) AS destination_exists
            FROM destination_checked_rows row
        ), overlap_conflicts AS MATERIALIZED (
            SELECT DISTINCT row.metric, row.destination_start
            FROM publication_rows row
            WHERE NOT row.non_overlapping
        ), preexisting_destination_conflicts AS MATERIALIZED (
            SELECT DISTINCT row.metric, row.destination_start
            FROM publication_rows row
            WHERE row.non_overlapping AND row.destination_exists
        ), eligible_rows AS MATERIALIZED (
            SELECT row.*
            FROM publication_rows row
            WHERE row.non_overlapping AND NOT row.destination_exists
        ), locked_rows AS MATERIALIZED (
            SELECT eligible.*
            FROM system_metric_rollups row
            JOIN eligible_rows eligible ON eligible.source_ctid = row.ctid
            ORDER BY eligible.destination_start, eligible.metric,
                eligible.bucket_start, eligible.bucket_secs
            FOR UPDATE OF row SKIP LOCKED
        ), counted_locked_rows AS MATERIALIZED (
            SELECT row.*,
                count(*) OVER (
                    PARTITION BY row.metric, row.destination_start
                )::bigint AS locked_source_rows
            FROM locked_rows row
        ), completion_rows AS MATERIALIZED (
            SELECT row.*,
                row.locked_source_rows = row.source_rows AS group_complete
            FROM counted_locked_rows row
        ), source AS MATERIALIZED (
            SELECT row.*
            FROM completion_rows row
            WHERE row.group_complete
        ), inserted AS (
            INSERT INTO system_metric_rollups (
                metric, bucket_start, bucket_secs, sample_count, value_sum,
                avg_value, max_value, latest_value, latest_observed_at, updated_at
            )
            SELECT metric, destination_start, $1,
                LEAST(sum(sample_count)::bigint, 2147483647)::integer,
                sum(value_sum),
                sum(value_sum) / sum(sample_count),
                max(max_value),
                (array_agg(latest_value ORDER BY latest_observed_at DESC,
                    bucket_start DESC, bucket_secs DESC))[1],
                max(latest_observed_at), max(updated_at)
            FROM source
            GROUP BY metric, destination_start
            ON CONFLICT (metric, bucket_secs, bucket_start) DO NOTHING
            RETURNING metric, bucket_start
        ), insertion_checked_rows AS MATERIALIZED (
            SELECT source.*,
                CASE WHEN source.group_ordinal = 1 THEN EXISTS (
                    SELECT 1 FROM inserted
                    WHERE inserted.metric = source.metric
                      AND inserted.bucket_start = source.destination_start
                ) ELSE NULL END AS inserted_probe
            FROM source
        ), insertion_rows AS MATERIALIZED (
            SELECT row.*,
                COALESCE(bool_or(row.inserted_probe) OVER (
                    PARTITION BY row.metric, row.destination_start
                ), FALSE) AS inserted_succeeded
            FROM insertion_checked_rows row
        ), deleted AS (
            DELETE FROM system_metric_rollups row
            USING insertion_rows source
            WHERE row.ctid = source.source_ctid
              AND source.inserted_succeeded
            RETURNING row.ctid, row.bucket_secs
        ), destination_conflicts AS MATERIALIZED (
            SELECT conflict.metric, conflict.destination_start
            FROM preexisting_destination_conflicts conflict
            UNION
            SELECT DISTINCT row.metric, row.destination_start
            FROM insertion_rows row
            WHERE NOT row.inserted_succeeded
        ), conflicts AS MATERIALIZED (
            SELECT metric, destination_start FROM overlap_conflicts
            UNION
            SELECT metric, destination_start FROM destination_conflicts
        )
        SELECT
            (SELECT count(*)::bigint FROM inserted) AS promoted,
            (SELECT count(*)::bigint FROM conflicts) AS conflicts,
            (SELECT count(*)::bigint FROM candidate_rows) AS examined_source_rows,
            (SELECT count(*)::bigint FROM deleted) AS source_rows,
            (SELECT count(*)::bigint FROM candidate_rows
             WHERE bucket_secs = $2) AS immediate_source_rows,
            (SELECT count(*)::bigint FROM deleted
             WHERE bucket_secs = $2) AS deleted_immediate_source_rows
        "#,
    )
    .bind(destination_secs)
    .bind(source_bucket_secs)
    .bind(destination_start)
    .bind(lower_bucket_secs)
    .bind(metric)
    .fetch_one(&mut **tx)
    .await?;
    let promoted = result.try_get::<i64, _>("promoted")?.max(0) as u64;
    reject_promotion_conflicts(
        SYSTEM_METRIC_ROLLUP_DOMAIN,
        source_bucket_secs,
        destination_secs,
        result.try_get("conflicts")?,
    )?;
    let immediate_source_rows = result.try_get::<i64, _>("immediate_source_rows")?.max(0) as u64;
    let deleted_immediate_source_rows = result
        .try_get::<i64, _>("deleted_immediate_source_rows")?
        .max(0) as u64;
    Ok(SystemMetricPromotionResult {
        promotion: PromotionResult {
            promoted,
            #[cfg(test)]
            examined_source_rows: result.try_get::<i64, _>("examined_source_rows")?.max(0) as u64,
            #[cfg(test)]
            source_rows: result.try_get::<i64, _>("source_rows")?.max(0) as u64,
        },
        complete: immediate_source_rows == deleted_immediate_source_rows,
    })
}

fn reject_promotion_conflicts(
    domain: &str,
    source_bucket_secs: i32,
    destination_bucket_secs: i32,
    conflicts: i64,
) -> Result<()> {
    anyhow::ensure!(
        conflicts == 0,
        "{domain} promotion from {source_bucket_secs}s to {destination_bucket_secs}s found {conflicts} unsupported destination or overlap conflicts"
    );
    Ok(())
}

async fn load_policy(pool: &PgPool, domain: &str) -> Result<RetentionPolicy> {
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
            retention_days: DEFAULT_TELEMETRY_ROLLUP_RETENTION_DAYS,
        });
    };
    Ok(RetentionPolicy {
        enabled: row.try_get("enabled")?,
        prune_limit: row.try_get::<i32, _>("prune_limit")?.clamp(1, 100_000),
        retention_days: row.try_get::<i32, _>("retention_days")?.clamp(1, 3_650),
    })
}

async fn prune_domain(pool: &PgPool, domain: &str, policy: RetentionPolicy) -> Result<u64> {
    if !policy.enabled {
        return Ok(0);
    }
    if !matches!(
        domain,
        "telemetry_samples"
            | "telemetry_rollups"
            | "telemetry_network_rates"
            | "telemetry_ping_rollups"
            | "system_metric_rollups"
    ) {
        return Ok(0);
    }
    let mut tx = pool.begin().await?;
    let rows_affected = if domain == "telemetry_samples" {
        prune_sample_domain_in_tx(&mut tx, policy).await?
    } else {
        let query = prune_query(domain);
        if matches!(domain, "telemetry_rollups" | "telemetry_network_rates") {
            // A bounded prune page changes many retained coordinates as one history
            // operation. Dashboard owns the corresponding full-block rebuild.
            sqlx::query("SELECT set_config('vpsman.telemetry_history_compaction', 'on', true)")
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query(&query)
            .bind(policy.retention_days)
            .bind(policy.prune_limit)
            .execute(&mut *tx)
            .await?
            .rows_affected()
    };
    tx.commit().await?;
    Ok(rows_affected)
}

async fn prune_sample_domain_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    policy: RetentionPolicy,
) -> Result<u64> {
    let owner = sqlx::query(sample_prune_owner_query())
        .bind(policy.retention_days)
        .fetch_optional(&mut **tx)
        .await?;
    let Some(owner) = owner else {
        return Ok(0);
    };
    let client_id: String = owner.try_get("client_id")?;
    let due_through_seq: i64 = owner.try_get("due_through_seq")?;
    let latest_projected_sample_id: Option<uuid::Uuid> =
        owner.try_get("latest_projected_sample_id")?;
    let expires_before: DateTime<Utc> = owner.try_get("expires_before")?;

    // Owner cursors and the canonical acceptance clock advance monotonically.
    // Between these two statements a stale frontier/cutoff can therefore only
    // retain extra rows. A newly projected latest row has a sequence above the
    // selected safe frontier, so the page cannot delete it. Keeping the exact
    // owner as bind parameters also gives generic prepared plans one ordered
    // client/accepted-sequence seek instead of a history-sized bitmap sort.
    Ok(sqlx::query(sample_prune_query())
        .bind(client_id)
        .bind(due_through_seq)
        .bind(latest_projected_sample_id)
        .bind(expires_before)
        .bind(policy.prune_limit)
        .execute(&mut **tx)
        .await?
        .rows_affected())
}

async fn prune_domain_has_remaining_work(
    pool: &PgPool,
    domain: &str,
    policy: RetentionPolicy,
) -> Result<bool> {
    if !policy.enabled {
        return Ok(false);
    }
    // The ordered LIMIT must live in a derived table below EXISTS. If it is
    // written directly inside EXISTS, PostgreSQL may discard the ordering and
    // choose a generic sequential plan over the complete retained relation.
    let query = match domain {
        "telemetry_samples" => r#"
            WITH owners AS MATERIALIZED (
                SELECT head.client_id,
                       head.latest_projected_sample_id,
                       LEAST(
                           head.projected_seq,
                           webhook.last_sample_seq,
                           core_minute.materialized_seq,
                           traffic_minute.materialized_seq
                       ) AS safe_seq
                FROM telemetry_projection_heads head
                JOIN telemetry_webhook_cursors webhook USING (client_id)
                JOIN telemetry_minute_materialization_heads core_minute
                  USING (client_id)
                JOIN traffic_counter_minute_heads traffic_minute
                  USING (client_id)
            ),
            owner_frontiers AS (
                SELECT owner.client_id, candidate.observed_at
                FROM owners owner
                CROSS JOIN LATERAL (
                    SELECT sample.observed_at
                    FROM telemetry_samples sample
                    WHERE sample.client_id = owner.client_id
                      AND sample.accepted_seq <= owner.safe_seq
                      AND sample.id IS DISTINCT FROM
                            owner.latest_projected_sample_id
                    ORDER BY sample.accepted_seq
                    LIMIT 1
                ) candidate
            )
            SELECT EXISTS (
                SELECT 1 FROM (
                    SELECT 1
                    FROM owner_frontiers frontier
                    WHERE frontier.observed_at
                            < now() - make_interval(days => $1)
                    ORDER BY frontier.observed_at, frontier.client_id
                    LIMIT 1
                ) bounded_due
            )
        "#
        .to_string(),
        "telemetry_rollups"
        | "telemetry_network_rates"
        | "telemetry_ping_rollups"
        | "system_metric_rollups" => {
            format!(
                r#"
                SELECT EXISTS (
                    SELECT 1 FROM (
                        SELECT 1
                        FROM {domain}
                        WHERE bucket_start < (
                            date_trunc('day', now() AT TIME ZONE 'UTC')
                                AT TIME ZONE 'UTC'
                        ) - make_interval(days => $1)
                          AND bucket_start
                                + make_interval(
                                    secs => GREATEST(bucket_secs, 1)
                                ) <= (
                            date_trunc('day', now() AT TIME ZONE 'UTC')
                                AT TIME ZONE 'UTC'
                        ) - make_interval(days => $1)
                        ORDER BY bucket_start ASC
                        LIMIT 1
                    ) bounded_due
                )
                "#
            )
        }
        _ => return Ok(false),
    };
    Ok(sqlx::query_scalar(&query)
        .bind(policy.retention_days)
        .fetch_one(pool)
        .await?)
}

async fn prune_domain_next_at(
    pool: &PgPool,
    domain: &str,
    policy: RetentionPolicy,
) -> Result<Option<DatabaseDeadline>> {
    if !policy.enabled {
        return Ok(None);
    }
    let query = match domain {
        // The expiry clock belongs to the oldest projected, non-latest sample;
        // webhook/core/traffic heads are producer gates, not clocks. Once a
        // deadline passes while one gate is behind, omit that past deadline so
        // the owner becomes ProducerOnly instead of spinning. Each advancing
        // gate emits the typed SamplePrune effect that rechecks readiness.
        "telemetry_samples" => r#"
            WITH cutoff AS MATERIALIZED (
                SELECT clock_timestamp()
                         - make_interval(days => $1)
                         - interval '1 microsecond' AS observed_after
            ),
            owners AS MATERIALIZED (
                SELECT head.client_id, head.projected_seq,
                       head.latest_projected_sample_id,
                       cutoff.observed_after
                FROM telemetry_projection_heads head
                CROSS JOIN cutoff
            ),
            owner_frontiers AS (
                SELECT owner.client_id, candidate.observed_at
                FROM owners owner
                CROSS JOIN LATERAL (
                    SELECT sample.id, sample.observed_at,
                           sample.accepted_seq
                    FROM telemetry_samples sample
                    WHERE sample.client_id = owner.client_id
                      AND sample.observed_at = (
                            SELECT min(boundary.observed_at)
                            FROM telemetry_samples boundary
                            WHERE boundary.client_id = owner.client_id
                              AND boundary.observed_at
                                    > owner.observed_after
                      )
                    ORDER BY sample.accepted_seq
                    LIMIT 1
                ) candidate
                WHERE candidate.accepted_seq <= owner.projected_seq
                  AND candidate.id IS DISTINCT FROM
                        owner.latest_projected_sample_id
            ),
            frontier AS (
                SELECT candidate.observed_at
                         + make_interval(days => $1)
                         + interval '1 microsecond' AS database_at
                FROM owner_frontiers candidate
                ORDER BY candidate.observed_at, candidate.client_id
                LIMIT 1
            )
            SELECT database_at,
                   GREATEST(
                       EXTRACT(EPOCH FROM database_at - clock_timestamp()), 0
                   )::DOUBLE PRECISION AS remaining_seconds
            FROM frontier
        "#
        .to_string(),
        "telemetry_rollups"
        | "telemetry_network_rates"
        | "telemetry_ping_rollups"
        | "system_metric_rollups" => format!(
            r#"
            WITH frontier AS (
                SELECT (
                    date_trunc(
                        'day',
                        (
                            bucket_start
                                + make_interval(secs => GREATEST(bucket_secs, 1))
                                - interval '1 microsecond'
                        ) AT TIME ZONE 'UTC'
                    ) + interval '1 day'
                ) AT TIME ZONE 'UTC' + make_interval(days => $1)
                    AS database_at
                FROM {domain}
                ORDER BY bucket_start ASC
                LIMIT 1
            )
            SELECT database_at,
                   GREATEST(
                       EXTRACT(EPOCH FROM database_at - clock_timestamp()), 0
                   )::DOUBLE PRECISION AS remaining_seconds
            FROM frontier
            "#,
        ),
        _ => return Ok(None),
    };
    optional_database_deadline(
        sqlx::query_as(&query)
            .bind(policy.retention_days)
            .fetch_optional(pool)
            .await?,
    )
}

async fn prune_ping_fact_rows(pool: &PgPool, policy: RetentionPolicy) -> Result<u64> {
    if !policy.enabled {
        return Ok(0);
    }
    let result = sqlx::query(ping_fact_prune_query())
        .bind(policy.retention_days)
        .bind(policy.prune_limit)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

async fn ping_fact_prune_has_remaining_work(
    pool: &PgPool,
    policy: RetentionPolicy,
) -> Result<bool> {
    if !policy.enabled {
        return Ok(false);
    }
    Ok(sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM (
                SELECT 1
                FROM telemetry_ping_facts fact
                WHERE fact.observed_at < now() - make_interval(days => $1)
                ORDER BY fact.observed_at, fact.series_id,
                         fact.checked_unix, fact.source_checked_unix
                LIMIT 1
            ) bounded_due
        )
        "#,
    )
    .bind(policy.retention_days)
    .fetch_one(pool)
    .await?)
}

async fn ping_fact_prune_next_at(
    pool: &PgPool,
    policy: RetentionPolicy,
) -> Result<Option<DatabaseDeadline>> {
    if !policy.enabled {
        return Ok(None);
    }
    optional_database_deadline(
        sqlx::query_as(
            r#"
            WITH frontier AS (
                SELECT fact.observed_at
                         + make_interval(days => $1)
                         + interval '1 microsecond' AS database_at
                FROM telemetry_ping_facts fact
                ORDER BY fact.observed_at, fact.series_id,
                         fact.checked_unix, fact.source_checked_unix
                LIMIT 1
            )
            SELECT database_at,
                   GREATEST(
                       EXTRACT(EPOCH FROM database_at - clock_timestamp()), 0
                   )::DOUBLE PRECISION AS remaining_seconds
            FROM frontier
            "#,
        )
        .bind(policy.retention_days)
        .fetch_optional(pool)
        .await?,
    )
}

async fn prune_ping_current(pool: &PgPool, policy: RetentionPolicy) -> Result<u64> {
    if !policy.enabled {
        return Ok(0);
    }
    let result = sqlx::query(
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
    Ok(result.rows_affected())
}

async fn ping_current_prune_has_remaining_work(
    pool: &PgPool,
    policy: RetentionPolicy,
) -> Result<bool> {
    if !policy.enabled {
        return Ok(false);
    }
    Ok(sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM (
                SELECT 1
                FROM telemetry_ping_current current
                JOIN telemetry_ping_series series ON series.id = current.series_id
                WHERE NOT EXISTS (
                    SELECT 1 FROM telemetry_ping_facts fact
                    WHERE fact.series_id = series.id
                )
                  AND NOT EXISTS (
                    SELECT 1 FROM telemetry_ping_rollups rollup
                    WHERE rollup.series_id = series.id
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
                LIMIT 1
            ) bounded_due
        )
        "#,
    )
    .fetch_one(pool)
    .await?)
}

async fn prune_ping_series(pool: &PgPool, policy: RetentionPolicy) -> Result<u64> {
    if !policy.enabled {
        return Ok(0);
    }
    let result = sqlx::query(
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
    Ok(result.rows_affected())
}

async fn ping_series_prune_has_remaining_work(
    pool: &PgPool,
    policy: RetentionPolicy,
) -> Result<bool> {
    if !policy.enabled {
        return Ok(false);
    }
    Ok(sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM (
                SELECT 1
                FROM telemetry_ping_series series
                WHERE NOT EXISTS (
                    SELECT 1 FROM telemetry_ping_facts fact
                    WHERE fact.series_id = series.id
                )
                  AND NOT EXISTS (
                    SELECT 1 FROM telemetry_ping_rollups rollup
                    WHERE rollup.series_id = series.id
                )
                  AND NOT EXISTS (
                    SELECT 1 FROM telemetry_ping_current current
                    WHERE current.series_id = series.id
                )
                ORDER BY series.id
                LIMIT 1
            ) bounded_due
        )
        "#,
    )
    .fetch_one(pool)
    .await?)
}

fn ping_fact_prune_query() -> &'static str {
    r#"
        WITH candidates AS (
            SELECT ctid FROM telemetry_ping_facts
            WHERE observed_at < now() - make_interval(days => $1)
            ORDER BY observed_at, series_id, checked_unix, source_checked_unix
            LIMIT $2 FOR UPDATE SKIP LOCKED
        )
        DELETE FROM telemetry_ping_facts WHERE ctid IN (SELECT ctid FROM candidates)
    "#
}

fn sample_prune_owner_query() -> &'static str {
    r#"
        WITH owners AS MATERIALIZED (
            SELECT head.client_id,
                   head.latest_projected_sample_id,
                   LEAST(
                       head.projected_seq,
                       webhook.last_sample_seq,
                       core_minute.materialized_seq,
                       traffic_minute.materialized_seq
                   ) AS safe_seq,
                   now() - make_interval(days => $1) AS expires_before
            FROM telemetry_projection_heads head
            JOIN telemetry_webhook_cursors webhook USING (client_id)
            JOIN telemetry_minute_materialization_heads core_minute
              USING (client_id)
            JOIN traffic_counter_minute_heads traffic_minute
              USING (client_id)
        ),
        owner_frontiers AS MATERIALIZED (
            SELECT owner.client_id, owner.latest_projected_sample_id,
                   owner.safe_seq, owner.expires_before,
                   first_sample.observed_at
            FROM owners owner
            CROSS JOIN LATERAL (
                SELECT sample.observed_at
                FROM telemetry_samples sample
                WHERE sample.client_id = owner.client_id
                  AND sample.accepted_seq <= owner.safe_seq
                  -- Keep the one canonical row backing latest/current views.
                  AND sample.id IS DISTINCT FROM
                        owner.latest_projected_sample_id
                ORDER BY sample.accepted_seq
                LIMIT 1
            ) first_sample
            WHERE first_sample.observed_at < owner.expires_before
        ),
        selected_owner AS MATERIALIZED (
            SELECT owner.*
            FROM owner_frontiers owner
            ORDER BY owner.observed_at, owner.client_id
            LIMIT 1
        ),
        bounded_owner AS MATERIALIZED (
            SELECT owner.client_id, owner.latest_projected_sample_id,
                   owner.expires_before,
                   LEAST(
                       owner.safe_seq,
                       COALESCE(first_current.accepted_seq - 1, owner.safe_seq)
                   ) AS due_through_seq
            FROM selected_owner owner
            LEFT JOIN LATERAL (
                SELECT sample.accepted_seq
                FROM telemetry_samples sample
                WHERE sample.client_id = owner.client_id
                  AND sample.observed_at = (
                        SELECT min(boundary.observed_at)
                        FROM telemetry_samples boundary
                        WHERE boundary.client_id = owner.client_id
                          AND boundary.observed_at >= owner.expires_before
                  )
                ORDER BY sample.accepted_seq
                LIMIT 1
            ) first_current ON TRUE
        )
        SELECT owner.client_id, owner.due_through_seq,
               owner.latest_projected_sample_id, owner.expires_before
        FROM bounded_owner owner
        "#
}

fn sample_prune_query() -> &'static str {
    r#"
        WITH candidates AS MATERIALIZED (
            SELECT sample.ctid
            FROM telemetry_samples sample
            WHERE sample.client_id = $1
              AND sample.accepted_seq <= $2
              AND sample.id IS DISTINCT FROM $3
              AND sample.observed_at < $4
            ORDER BY sample.accepted_seq
            LIMIT $5
            FOR UPDATE OF sample SKIP LOCKED
        )
        DELETE FROM telemetry_samples sample
        WHERE sample.ctid = ANY (
            ARRAY(SELECT candidate.ctid FROM candidates candidate)
        )
        "#
}

fn prune_query(table: &str) -> String {
    format!(
        r#"
        WITH candidates AS (
            SELECT row.tableoid AS source_tableoid,
                   row.ctid AS source_ctid
            FROM {table} row
            WHERE row.bucket_start < (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
            ) - make_interval(days => $1)
              AND row.bucket_start
                + make_interval(secs => GREATEST(row.bucket_secs, 1)) <= (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
            ) - make_interval(days => $1)
            ORDER BY row.bucket_start ASC
            LIMIT $2
            FOR UPDATE OF row SKIP LOCKED
        )
        DELETE FROM {table} row
        USING candidates candidate
        WHERE row.tableoid = candidate.source_tableoid
          AND row.ctid = candidate.source_ctid
        "#
    )
}

#[cfg(test)]
#[path = "tests_history_retention.rs"]
mod tests;
