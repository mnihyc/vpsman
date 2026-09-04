use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        OnceLock,
    },
    time::{Duration, Instant},
};

use anyhow::Result;
use futures_util::{stream, StreamExt};
use tokio::sync::{oneshot, Notify};
use tracing::{debug, warn};
use uuid::Uuid;
use vpsman_common::{
    CommandOutput, GatewayClientDispatchFenceOwner, JobCommand, JobRequest, OutputStream,
};
use vpsman_server_core::{
    operator_is_active_authorized, TARGET_STATUS_COMPLETED, TARGET_STATUS_CONTROL_TIMEOUT,
    TARGET_STATUS_FAILED, TARGET_STATUS_REJECTED,
};

use crate::{
    gateway_client::GatewayClientTimeouts,
    internal_operator::{server_issued_job_actor, system_operator},
    job_traffic_import::wake_network_traffic_import_finalizer,
    model::{AuthContext, BackupRequestStatus, CreateBackupRequest},
    repository_backups::BackupRequestSourceLink,
    repository_job_outputs::{JobOutputPersistConfig, JobOutputWriteResult},
    repository_jobs::{ClaimedJobTarget, ClaimedJobTerminalEnrichment},
    state::AppState,
    TargetDispatchOutcome,
};

const DISPATCH_INTERVAL_SECS: u64 = 1;
const DEADLINE_EXPIRY_INTERVAL_SECS: u64 = 1;

struct DispatcherWakeState {
    notify: Notify,
    dispatching: AtomicBool,
    pending: AtomicBool,
    sweeps_started: AtomicU64,
    sweeps_coalesced: AtomicU64,
    targets_claimed: AtomicU64,
    dispatch_latency_micros_total: AtomicU64,
    dispatch_latency_samples: AtomicU64,
    gateway_dispatch_errors: AtomicU64,
}

struct AbortTaskOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortTaskOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Default for DispatcherWakeState {
    fn default() -> Self {
        Self {
            notify: Notify::new(),
            dispatching: AtomicBool::new(false),
            pending: AtomicBool::new(false),
            sweeps_started: AtomicU64::new(0),
            sweeps_coalesced: AtomicU64::new(0),
            targets_claimed: AtomicU64::new(0),
            dispatch_latency_micros_total: AtomicU64::new(0),
            dispatch_latency_samples: AtomicU64::new(0),
            gateway_dispatch_errors: AtomicU64::new(0),
        }
    }
}

static DISPATCHER_WAKE_STATE: OnceLock<DispatcherWakeState> = OnceLock::new();
static TERMINAL_EVENT_WAKE: OnceLock<Notify> = OnceLock::new();
static TERMINAL_ENRICHMENT_WAKE: OnceLock<Notify> = OnceLock::new();

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct DispatcherMetricsSnapshot {
    pub(crate) dispatcher_sweeps_started: u64,
    pub(crate) dispatcher_sweeps_coalesced: u64,
    pub(crate) targets_claimed: u64,
    pub(crate) dispatch_latency_micros_total: u64,
    pub(crate) dispatch_latency_samples: u64,
    pub(crate) gateway_dispatch_errors: u64,
}

fn dispatcher_wake_state() -> &'static DispatcherWakeState {
    DISPATCHER_WAKE_STATE.get_or_init(DispatcherWakeState::default)
}

pub(crate) fn dispatcher_metrics_snapshot() -> DispatcherMetricsSnapshot {
    let wake_state = dispatcher_wake_state();
    DispatcherMetricsSnapshot {
        dispatcher_sweeps_started: wake_state.sweeps_started.load(Ordering::Relaxed),
        dispatcher_sweeps_coalesced: wake_state.sweeps_coalesced.load(Ordering::Relaxed),
        targets_claimed: wake_state.targets_claimed.load(Ordering::Relaxed),
        dispatch_latency_micros_total: wake_state
            .dispatch_latency_micros_total
            .load(Ordering::Relaxed),
        dispatch_latency_samples: wake_state.dispatch_latency_samples.load(Ordering::Relaxed),
        gateway_dispatch_errors: wake_state.gateway_dispatch_errors.load(Ordering::Relaxed),
    }
}

pub(crate) fn spawn_job_dispatcher(state: AppState) -> tokio::task::JoinHandle<()> {
    let wake_state = dispatcher_wake_state();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(DISPATCH_INTERVAL_SECS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = wake_state.notify.notified() => {}
            }
            if let Err(error) = run_dispatcher_sweep(&state).await {
                warn!(%error, "durable job dispatcher tick failed");
            }
        }
    })
}

/// Owns elapsed control deadlines independently of dispatch backlog. A full
/// page is redrained immediately; the one-second timer is consulted only once
/// no immediately claimable deadline remains.
pub(crate) fn spawn_job_deadline_expiry_consumer(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(Duration::from_secs(DEADLINE_EXPIRY_INTERVAL_SECS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            if let Err(error) = drain_control_timeout_targets(&state).await {
                warn!(%error, "durable job deadline-expiry consumer failed");
            }
        }
    })
}

/// Owns durable job-terminal side effects. Producers only commit their terminal
/// transition and wake this task; request and dispatch paths never consume the
/// global terminal queue themselves.
pub(crate) fn spawn_job_terminal_event_consumer(state: AppState) -> tokio::task::JoinHandle<()> {
    let wake = TERMINAL_EVENT_WAKE.get_or_init(Notify::new);
    tokio::spawn(async move {
        let mut recovery = tokio::time::interval(Duration::from_secs(DISPATCH_INTERVAL_SECS));
        recovery.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = recovery.tick() => {}
                _ = wake.notified() => {}
            }
            if let Err(error) = drain_job_terminal_events(&state).await {
                warn!(%error, "durable job terminal-event consumer failed");
            }
        }
    })
}

pub(crate) fn wake_job_terminal_event_consumer() {
    TERMINAL_EVENT_WAKE.get_or_init(Notify::new).notify_one();
}

/// Owns external terminal enrichment after the repository-stage consumer has
/// committed an exact durable handoff. Work is independent across targets and
/// uses the dispatcher's configured batch and in-flight capacity.
pub(crate) fn spawn_job_terminal_enrichment_consumer(
    state: AppState,
) -> tokio::task::JoinHandle<()> {
    let wake = TERMINAL_ENRICHMENT_WAKE.get_or_init(Notify::new);
    tokio::spawn(async move {
        let mut recovery = tokio::time::interval(Duration::from_secs(DISPATCH_INTERVAL_SECS));
        recovery.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = recovery.tick() => {}
                _ = wake.notified() => {}
            }
            if let Err(error) = drain_job_terminal_enrichments(&state).await {
                warn!(%error, "durable job terminal-enrichment consumer failed");
            }
        }
    })
}

fn wake_job_terminal_enrichment_consumer() {
    TERMINAL_ENRICHMENT_WAKE
        .get_or_init(Notify::new)
        .notify_one();
}

async fn drain_job_terminal_events(state: &AppState) -> Result<usize> {
    let mut handled_total = 0_usize;
    loop {
        // Repository ownership is one durable terminal row per claim. Keep that
        // exact transaction boundary and drain until no immediately due row remains.
        let batch = state.process_job_terminal_events(1).await?;
        let handled = batch.targets.len().saturating_add(batch.jobs.len());
        if !batch.targets.is_empty() {
            wake_job_terminal_enrichment_consumer();
        }
        handled_total = handled_total.saturating_add(handled);
        if handled == 0 {
            return Ok(handled_total);
        }
    }
}

async fn drain_job_terminal_enrichments(state: &AppState) -> Result<usize> {
    let mut handled_total = 0_usize;
    loop {
        let config = state.dispatcher_runtime_config();
        let lease_secs = config.dispatch_lease_secs();
        let claimed = state
            .repo
            .claim_job_terminal_enrichments(config.immediate_claim_limit(), lease_secs as i64)
            .await?;
        if claimed.is_empty() {
            return Ok(handled_total);
        }
        handled_total = handled_total.saturating_add(claimed.len());
        stream::iter(claimed)
            .for_each_concurrent(config.in_flight, |work| async move {
                if let Err(error) =
                    process_claimed_job_terminal_enrichment(state, work, lease_secs).await
                {
                    warn!(%error, "durable job terminal enrichment owner failed");
                }
            })
            .await;
    }
}

// End-to-end tests use the production repository and enrichment owners in
// their causal order. The second repository drain publishes terminal rows
// whose durable enrichment handoff was acknowledged by the middle stage.
#[cfg(test)]
pub(crate) async fn drain_job_terminal_workflow_for_test(state: &AppState) -> Result<usize> {
    let repository_before = drain_job_terminal_events(state).await?;
    let enriched = drain_job_terminal_enrichments(state).await?;
    let repository_after = drain_job_terminal_events(state).await?;
    Ok(repository_before
        .saturating_add(enriched)
        .saturating_add(repository_after))
}

async fn process_claimed_job_terminal_enrichment(
    state: &AppState,
    work: ClaimedJobTerminalEnrichment,
    lease_secs: u64,
) -> Result<()> {
    let heartbeat_repo = state.repo.clone();
    let heartbeat_work = work.clone();
    let (heartbeat_stop, heartbeat_stop_rx) = oneshot::channel();
    let heartbeat = tokio::spawn(renew_job_terminal_enrichment_owner_until_stopped(
        heartbeat_repo,
        heartbeat_work,
        lease_secs,
        heartbeat_stop_rx,
    ));
    let enrichment = state.enrich_job_terminal_target(&work).await;
    let _ = heartbeat_stop.send(());
    let ownership_current = heartbeat.await??;
    if !ownership_current {
        warn!(
            event_id = %work.event_id,
            job_id = %work.job_id,
            client_id = %work.client_id,
            "terminal enrichment ownership changed while work was active"
        );
        return Ok(());
    }
    match enrichment {
        Ok(()) => {
            if state
                .repo
                .acknowledge_job_terminal_enrichment(work.event_id, work.owner_token)
                .await?
            {
                wake_job_terminal_event_consumer();
            } else {
                warn!(
                    event_id = %work.event_id,
                    job_id = %work.job_id,
                    client_id = %work.client_id,
                    "terminal enrichment completed after ownership changed"
                );
            }
        }
        Err(error) => {
            warn!(
                %error,
                event_id = %work.event_id,
                job_id = %work.job_id,
                client_id = %work.client_id,
                "durable job target enrichment deferred for retry"
            );
            if !state
                .repo
                .defer_job_terminal_enrichment(work.event_id, work.owner_token, &error.to_string())
                .await?
            {
                warn!(
                    event_id = %work.event_id,
                    job_id = %work.job_id,
                    client_id = %work.client_id,
                    "terminal enrichment retry was not recorded after ownership changed"
                );
            }
        }
    }
    Ok(())
}

async fn renew_job_terminal_enrichment_owner_until_stopped(
    repo: crate::repository::Repository,
    work: ClaimedJobTerminalEnrichment,
    lease_secs: u64,
    mut stop: oneshot::Receiver<()>,
) -> Result<bool> {
    let renewal_interval = Duration::from_secs((lease_secs / 3).max(1));
    loop {
        tokio::select! {
            _ = &mut stop => return Ok(true),
            _ = tokio::time::sleep(renewal_interval) => {
                if !repo
                    .renew_job_terminal_enrichment_owner(
                        work.event_id,
                        work.owner_token,
                        lease_secs as i64,
                    )
                    .await?
                {
                    return Ok(false);
                }
            }
        }
    }
}

pub(crate) fn wake_job_dispatcher() {
    let wake_state = dispatcher_wake_state();
    wake_state.pending.store(true, Ordering::Release);
    if wake_state.dispatching.load(Ordering::Acquire) {
        wake_state.sweeps_coalesced.fetch_add(1, Ordering::Relaxed);
    }
    wake_state.notify.notify_one();
}

async fn run_dispatcher_sweep(state: &AppState) -> Result<usize> {
    let wake_state = dispatcher_wake_state();
    if wake_state.dispatching.swap(true, Ordering::AcqRel) {
        wake_state.pending.store(true, Ordering::Release);
        wake_state.sweeps_coalesced.fetch_add(1, Ordering::Relaxed);
        debug!(
            dispatcher_sweeps_coalesced = dispatcher_metrics_snapshot().dispatcher_sweeps_coalesced,
            "durable job dispatcher wake coalesced"
        );
        return Ok(0);
    }

    wake_state.sweeps_started.fetch_add(1, Ordering::Relaxed);
    let started_at = Instant::now();
    let result = async {
        let mut total = 0;
        loop {
            wake_state.pending.store(false, Ordering::Release);
            total += dispatch_due_job_targets(state).await?;
            if !wake_state.pending.swap(false, Ordering::AcqRel) {
                break;
            }
            debug!("durable job dispatcher draining coalesced wake");
        }
        Ok(total)
    }
    .await;

    let elapsed_micros = u64::try_from(started_at.elapsed().as_micros()).unwrap_or(u64::MAX);
    wake_state
        .dispatch_latency_micros_total
        .fetch_add(elapsed_micros, Ordering::Relaxed);
    wake_state
        .dispatch_latency_samples
        .fetch_add(1, Ordering::Relaxed);
    wake_state.dispatching.store(false, Ordering::Release);
    let metrics = dispatcher_metrics_snapshot();
    debug!(
        dispatcher_sweeps_started = metrics.dispatcher_sweeps_started,
        dispatcher_sweeps_coalesced = metrics.dispatcher_sweeps_coalesced,
        targets_claimed = metrics.targets_claimed,
        dispatch_latency_micros_total = metrics.dispatch_latency_micros_total,
        dispatch_latency_samples = metrics.dispatch_latency_samples,
        gateway_dispatch_errors = metrics.gateway_dispatch_errors,
        "durable job dispatcher metrics"
    );
    result
}

pub(crate) async fn dispatch_due_job_targets(state: &AppState) -> Result<usize> {
    state.repo.reconcile_job_rollouts(500).await?;
    let mut total = 0_usize;
    loop {
        // Claim and execute a wave from one coherent timeout snapshot. Hot
        // reloads apply to the next wave and can never outgrow this wave's
        // durable lease or control deadline after it has been claimed.
        let dispatcher_config = state.dispatcher_runtime_config();
        let claim_limit = dispatcher_config.immediate_claim_limit();
        let gateway_timeouts = GatewayClientTimeouts {
            connect: Duration::from_secs(dispatcher_config.internal_http_connect_secs),
            write: Duration::from_secs(dispatcher_config.internal_http_write_secs),
            read: Duration::from_secs(
                dispatcher_config
                    .dispatch_ack_secs
                    .max(dispatcher_config.internal_http_read_secs),
            ),
        };
        // Command dispatch receives this immutable snapshot directly. The
        // shared client is updated from the same snapshot for cancel,
        // terminal, suspension and other control calls that are not owned by
        // a claimed command wave.
        state.gateway.set_read_timeout(gateway_timeouts.read);
        let claimed = state
            .repo
            .claim_due_job_targets(
                claim_limit,
                dispatcher_config.gateway_dispatch_attempt_lease_secs() as i64,
                dispatcher_config.control_deadline_extra_secs(),
            )
            .await?;
        let claimed_count = claimed.len();
        if claimed_count == 0 {
            return Ok(total);
        }
        total = total.saturating_add(claimed_count);
        dispatcher_wake_state()
            .targets_claimed
            .fetch_add(claimed_count as u64, Ordering::Relaxed);
        debug!(claimed_count, "durable job dispatcher claimed targets");
        stream::iter(claimed)
            .for_each_concurrent(dispatcher_config.in_flight, |claimed| {
                let state = state.clone();
                async move {
                    // Poll each claimed target as a runtime task root. The command-output,
                    // terminalization, and backup-handoff state machines are independently bounded,
                    // but nesting all of their debug-build poll frames under FuturesUnordered can
                    // exhaust the standard worker stack before any operation yields.
                    let mut dispatch_task = AbortTaskOnDrop(tokio::spawn(async move {
                        dispatch_claimed_target(&state, claimed, gateway_timeouts).await
                    }));
                    match (&mut dispatch_task.0).await {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => warn!(%error, "durable job target dispatch failed"),
                        Err(error) => warn!(%error, "durable job target dispatch task failed"),
                    }
                }
            })
            .await;
        // Terminal work from this completed wave has its own owner and may
        // start while the dispatcher immediately claims the next free wave.
        wake_job_terminal_event_consumer();
        state.repo.reconcile_job_rollouts(500).await?;
    }
}

async fn dispatch_claimed_target(
    state: &AppState,
    claimed: ClaimedJobTarget,
    gateway_timeouts: GatewayClientTimeouts,
) -> Result<()> {
    if !state.gateway.configured() {
        dispatcher_wake_state()
            .gateway_dispatch_errors
            .fetch_add(1, Ordering::Relaxed);
        let outcome = dispatch_error_outcome(claimed.job_id, "gateway control URL missing");
        return Box::pin(finish_claimed_target(state, &claimed, outcome)).await;
    }

    if let Err(error) = record_backup_request_for_claim(state, &claimed).await {
        warn!(
            %error,
            job_id = %claimed.job_id,
            client_id = %claimed.client_id,
            "backup request pre-record failed"
        );
        let outcome = dispatch_error_outcome(claimed.job_id, "backup request pre-record failed");
        return Box::pin(finish_claimed_target(state, &claimed, outcome)).await;
    }
    if auth_context_for_claim(state, &claimed).await?.is_none() {
        warn!(
            job_id = %claimed.job_id,
            client_id = %claimed.client_id,
            "job actor authority revoked before dispatch"
        );
        let outcome = dispatch_rejected_outcome(claimed.job_id, "actor_authority_revoked");
        return Box::pin(finish_claimed_target(state, &claimed, outcome)).await;
    }
    // Bind the durable DB proof to the gateway process observed before that
    // read. A restart or expired lifecycle lease challenges this exact
    // request and permits one fresh proof/retry, never a global cached proof.
    let mut expected_gateway_epoch = state.gateway.command_gateway_epoch();
    if !state.repo.claimed_job_target_dispatchable(&claimed).await? {
        wake_job_terminal_event_consumer();
        return Ok(());
    }

    let command_version =
        crate::job_request::job_command_dispatch_protocol_version(&claimed.operation);
    debug_assert!(
        command_version
            >= crate::job_request::job_command_min_supported_protocol_version(&claimed.operation)
    );
    let request = JobRequest {
        job_id: claimed.job_id,
        command_version,
        command: claimed.operation.clone(),
        max_timeout_secs: claimed.max_timeout_secs.max(1),
    };
    let first_dispatch = state
        .gateway
        .dispatch(
            &claimed.client_id,
            request.clone(),
            claimed.process_incarnation_id,
            expected_gateway_epoch,
            None,
            claimed.payload_hash.clone(),
            gateway_timeouts,
        )
        .await;
    let dispatch = match first_dispatch {
        Err(error) => {
            let message = error.to_string();
            let lifecycle_recheck = parse_lifecycle_recheck_owner(&message);
            let gateway_epoch_recheck = parse_gateway_epoch_recheck(&message);
            if lifecycle_recheck.is_none() && gateway_epoch_recheck.is_none() {
                Err(error)
            } else {
                if let Some(gateway_epoch) = gateway_epoch_recheck {
                    state.gateway.observe_command_gateway_epoch(gateway_epoch);
                    expected_gateway_epoch = Some(gateway_epoch);
                }
                if !state.repo.claimed_job_target_dispatchable(&claimed).await? {
                    wake_job_terminal_event_consumer();
                    return Ok(());
                }
                state
                    .gateway
                    .dispatch(
                        &claimed.client_id,
                        request,
                        claimed.process_incarnation_id,
                        expected_gateway_epoch,
                        lifecycle_recheck,
                        claimed.payload_hash.clone(),
                        gateway_timeouts,
                    )
                    .await
            }
        }
        result => result,
    };
    let outcome = match dispatch {
        Ok(result) => crate::routes_jobs::target_outcome_from_gateway(result),
        Err(error) => {
            dispatcher_wake_state()
                .gateway_dispatch_errors
                .fetch_add(1, Ordering::Relaxed);
            let message = error.to_string();
            warn!(
                job_id = %claimed.job_id,
                client_id = %claimed.client_id,
                error = %message,
                "gateway command dispatch failed"
            );
            if message.contains("agent_incarnation_mismatch") {
                let refreshed = state
                    .repo
                    .record_agent_lost_target(
                        claimed.job_id,
                        &claimed.client_id,
                        &message,
                        Some(claimed.process_incarnation_id),
                        parse_agent_incarnation_mismatch_actual(&message),
                        claimed.dispatch_attempt,
                    )
                    .await?;
                let _ = refreshed;
                wake_job_terminal_event_consumer();
            } else {
                state
                    .repo
                    .record_job_target_delivery_error(
                        claimed.job_id,
                        &claimed.client_id,
                        &message,
                        claimed.dispatch_attempt,
                    )
                    .await?;
            }
            return Ok(());
        }
    };
    if outcome.status == TARGET_STATUS_REJECTED || outcome.outputs.iter().any(|output| output.done)
    {
        return Box::pin(finish_claimed_target(state, &claimed, outcome)).await;
    }
    state
        .repo
        .mark_claimed_job_target_running(
            claimed.job_id,
            &claimed.client_id,
            &outcome.message,
            claimed.dispatch_attempt,
        )
        .await?;
    Ok(())
}

fn parse_agent_incarnation_mismatch_actual(message: &str) -> Option<Uuid> {
    let actual = message.split("actual=").nth(1)?;
    let token = actual
        .split(|ch: char| !(ch.is_ascii_hexdigit() || ch == '-'))
        .next()
        .filter(|value| !value.is_empty())?;
    Uuid::parse_str(token).ok()
}

fn parse_gateway_epoch_recheck(message: &str) -> Option<Uuid> {
    parse_uuid_after_marker(message, "agent_gateway_epoch_recheck_required:")
}

fn parse_lifecycle_recheck_owner(message: &str) -> Option<GatewayClientDispatchFenceOwner> {
    let suffix = message.split("agent_lifecycle_recheck_required:").nth(1)?;
    let mut fields = suffix.split(':');
    let gateway_epoch = parse_uuid_prefix(fields.next()?)?;
    let generation = fields.next()?.parse().ok()?;
    let token = parse_uuid_prefix(fields.next()?)?;
    Some(GatewayClientDispatchFenceOwner {
        token,
        gateway_epoch,
        generation,
    })
}

fn parse_uuid_after_marker(message: &str, marker: &str) -> Option<Uuid> {
    parse_uuid_prefix(message.split(marker).nth(1)?)
}

fn parse_uuid_prefix(value: &str) -> Option<Uuid> {
    let token = value
        .split(|ch: char| !(ch.is_ascii_hexdigit() || ch == '-'))
        .next()
        .filter(|value| !value.is_empty())?;
    Uuid::parse_str(token).ok()
}

async fn drain_control_timeout_targets(state: &AppState) -> Result<usize> {
    let dispatcher_config = state.dispatcher_runtime_config();
    let claim_limit = dispatcher_config.immediate_claim_limit();
    let mut total = 0_usize;
    loop {
        // The page equals work that this process can begin now. It bounds one
        // database transaction without acting as a throughput allowance.
        let expired = state
            .repo
            .expire_control_timeout_targets(claim_limit)
            .await?;
        let expired_count = expired.len();
        if expired_count == 0 {
            return Ok(total);
        }
        total = total.saturating_add(expired_count);
        wake_job_terminal_event_consumer();
        let results =
            stream::iter(expired)
                .map(|target| async move {
                    expire_control_timeout_target_side_effects(state, target).await
                })
                .buffer_unordered(dispatcher_config.in_flight)
                .collect::<Vec<_>>()
                .await;
        for result in results {
            result?;
        }
        if expired_count < claim_limit as usize {
            return Ok(total);
        }
    }
}

async fn expire_control_timeout_target_side_effects(
    state: &AppState,
    target: crate::repository_jobs::DeadlineExpiredJobTarget,
) -> Result<()> {
    if target.status != TARGET_STATUS_CONTROL_TIMEOUT {
        return Ok(());
    }
    state
        .repo
        .record_job_target_cancel_sent(target.job_id, &target.client_id)
        .await?;
    match state
        .gateway
        .cancel(
            &target.client_id,
            vpsman_common::JobCancelRequest {
                job_id: target.job_id,
                reason: Some("control_deadline_elapsed".to_string()),
            },
        )
        .await
    {
        Ok(cancel) => {
            state
                .repo
                .record_job_target_cancel_result(
                    target.job_id,
                    &target.client_id,
                    cancel.accepted,
                    cancel.acked,
                    cancel.applied,
                    &cancel.message,
                )
                .await?;
        }
        Err(error) => {
            let message = format!("deadline cancel delivery failed: {error}");
            warn!(
                %error,
                job_id = %target.job_id,
                client_id = %target.client_id,
                "deadline cancel delivery failed"
            );
            state
                .repo
                .record_job_target_cancel_result(
                    target.job_id,
                    &target.client_id,
                    false,
                    false,
                    false,
                    &message,
                )
                .await?;
        }
    }
    Ok(())
}

async fn finish_claimed_target(
    state: &AppState,
    claimed: &ClaimedJobTarget,
    outcome: TargetDispatchOutcome,
) -> Result<()> {
    let persist_config = JobOutputPersistConfig {
        object_store: state.backup_object_store.as_ref(),
        artifact_min_bytes: state.job_output_artifact_min_bytes(),
    };
    if outcome.status == TARGET_STATUS_COMPLETED
        && matches!(
            &claimed.operation,
            JobCommand::NetworkTrafficImportVnstat { .. }
        )
    {
        let write_results = state
            .repo
            .record_claimed_job_outputs_checked_with_config(
                claimed.job_id,
                &claimed.client_id,
                &outcome.outputs,
                persist_config,
                claimed.dispatch_attempt,
            )
            .await?;
        if reject_conflicting_dispatch_outputs(state, claimed, &write_results).await? {
            return Ok(());
        }
        state
            .repo
            .mark_claimed_job_target_running(
                claimed.job_id,
                &claimed.client_id,
                "vnStat history collected; server import pending",
                claimed.dispatch_attempt,
            )
            .await?;
        if !outcome.outputs.is_empty() {
            state.invalidate_job_details(claimed.job_id);
        }
        wake_network_traffic_import_finalizer();
        return Ok(());
    }
    let final_output_index = outcome.outputs.iter().position(|output| output.done);
    if final_output_index.is_some_and(|index| index + 1 != outcome.outputs.len()) {
        state
            .repo
            .record_job_target_delivery_error(
                claimed.job_id,
                &claimed.client_id,
                "job_output_after_final_marker",
                claimed.dispatch_attempt,
            )
            .await?;
        return Ok(());
    }
    let prefix_end = final_output_index.unwrap_or(outcome.outputs.len());
    let prefix_results = state
        .repo
        .record_claimed_job_outputs_checked_with_config(
            claimed.job_id,
            &claimed.client_id,
            &outcome.outputs[..prefix_end],
            persist_config,
            claimed.dispatch_attempt,
        )
        .await?;
    if reject_conflicting_dispatch_outputs(state, claimed, &prefix_results).await? {
        return Ok(());
    }
    if let Some(final_index) = final_output_index {
        let final_output = &outcome.outputs[final_index];
        let received_at = outcome
            .received_at
            .clone()
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
        let mut final_outcome = outcome.clone();
        final_outcome.received_at = Some(received_at.clone());
        let record = state
            .repo
            .record_claimed_final_job_output_and_target_result_with_config(
                claimed.job_id,
                &claimed.client_id,
                i32::try_from(final_index)?,
                final_output,
                Some(received_at),
                persist_config,
                &final_outcome,
                claimed.dispatch_attempt,
            )
            .await?;
        if reject_conflicting_dispatch_outputs(state, claimed, &[record.write_result]).await? {
            return Ok(());
        }
    } else {
        state
            .repo
            .update_claimed_job_target_result(
                claimed.job_id,
                &claimed.client_id,
                &outcome,
                claimed.dispatch_attempt,
            )
            .await?;
    }
    if !outcome.outputs.is_empty() {
        state.invalidate_job_details(claimed.job_id);
    }
    Ok(())
}

async fn reject_conflicting_dispatch_outputs(
    state: &AppState,
    claimed: &ClaimedJobTarget,
    write_results: &[JobOutputWriteResult],
) -> Result<bool> {
    if !write_results.contains(&JobOutputWriteResult::DuplicateConflict) {
        return Ok(false);
    }
    // A conflicting duplicate sequence means the gateway/agent replay stream is corrupt.
    // Retrying this event forever could terminalize from evidence we did not store, so keep
    // the target active for normal lifecycle handling and record the protocol error.
    state
        .repo
        .record_job_target_delivery_error(
            claimed.job_id,
            &claimed.client_id,
            "job_output_sequence_conflict",
            claimed.dispatch_attempt,
        )
        .await?;
    Ok(true)
}

async fn record_backup_request_for_claim(
    state: &AppState,
    claimed: &ClaimedJobTarget,
) -> Result<()> {
    let JobCommand::Backup {
        paths,
        include_config,
        follow_symlinks,
        missing_path_policy,
    } = &claimed.operation
    else {
        return Ok(());
    };
    let Some(operator) = auth_context_for_claim(state, claimed).await? else {
        warn!(
            job_id = %claimed.job_id,
            client_id = %claimed.client_id,
            "backup job has no actor; skipping backup request pre-record"
        );
        return Ok(());
    };
    let source = BackupRequestSourceLink {
        job_id: Some(claimed.job_id),
        schedule_id: claimed.source_schedule_id,
        causation_id: claimed.causation_id,
        schedule_lineage: claimed.schedule_lineage.clone(),
    };
    if let Some(request) = state
        .repo
        .find_open_backup_request_for_source(&claimed.client_id, &claimed.payload_hash, &source)
        .await?
    {
        if state
            .repo
            .attach_backup_request_source(request.id, &source)
            .await?
            .is_some()
        {
            return Ok(());
        }
    }
    let request = CreateBackupRequest {
        client_id: claimed.client_id.clone(),
        paths: paths.clone(),
        include_config: *include_config,
        follow_symlinks: *follow_symlinks,
        missing_path_policy: *missing_path_policy,
        confirmed: true,
        note: Some(format!("auto-linked from backup job {}", claimed.job_id)),
        privilege_assertion: None,
    };
    let command_scope = format!("client:{}", request.client_id);
    state
        .repo
        .record_backup_request_with_source(
            &request,
            &claimed.payload_hash,
            &command_scope,
            &operator,
            BackupRequestStatus::RequestedMetadataOnly,
            source,
        )
        .await?;
    Ok(())
}

async fn auth_context_for_claim(
    state: &AppState,
    claimed: &ClaimedJobTarget,
) -> Result<Option<AuthContext>> {
    let Some(actor_id) = claimed.actor_id else {
        if claimed.source_schedule_id.is_some() {
            return Ok(None);
        }
        return Ok(server_issued_job_actor(&claimed.operation).map(system_operator));
    };
    if actor_id.is_nil() {
        return Ok(None);
    }
    let Some(operator) = state.repo.operator_by_id(actor_id).await? else {
        return Ok(None);
    };
    if !operator_is_active_authorized(
        &operator.status,
        &operator.role,
        &operator.scopes,
        "operator",
        &["jobs:write"],
    ) {
        return Ok(None);
    }
    Ok(Some(AuthContext {
        operator: operator.view(),
        session_id: None,
    }))
}

fn dispatch_rejected_outcome(job_id: Uuid, message: &str) -> TargetDispatchOutcome {
    let status = serde_json::json!({
        "type": "dispatch_error",
        "status": TARGET_STATUS_REJECTED,
        "message": message,
    });
    TargetDispatchOutcome {
        status: TARGET_STATUS_REJECTED.to_string(),
        exit_code: None,
        #[cfg(test)]
        command_version: None,
        accepted: false,
        message: message.to_string(),
        received_at: None,
        outputs: vec![CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&status).unwrap_or_else(|_| message.as_bytes().to_vec()),
            exit_code: None,
            done: true,
        }],
    }
}

fn dispatch_error_outcome(job_id: Uuid, message: &str) -> TargetDispatchOutcome {
    let status = serde_json::json!({
        "type": "dispatch_error",
        "status": TARGET_STATUS_FAILED,
        "message": message,
    });
    TargetDispatchOutcome {
        status: TARGET_STATUS_FAILED.to_string(),
        exit_code: None,
        #[cfg(test)]
        command_version: None,
        accepted: false,
        message: message.to_string(),
        received_at: None,
        outputs: vec![CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&status).unwrap_or_else(|_| message.as_bytes().to_vec()),
            exit_code: None,
            done: true,
        }],
    }
}

#[cfg(test)]
mod task_boundary_tests {
    use super::AbortTaskOnDrop;

    struct DropSignal(Option<tokio::sync::oneshot::Sender<()>>);

    impl Drop for DropSignal {
        fn drop(&mut self) {
            if let Some(signal) = self.0.take() {
                let _ = signal.send(());
            }
        }
    }

    #[tokio::test]
    async fn dropping_dispatch_task_guard_aborts_in_flight_work() {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (dropped_tx, dropped_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _drop_signal = DropSignal(Some(dropped_tx));
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
        });
        let guard = AbortTaskOnDrop(task);
        started_rx.await.unwrap();

        drop(guard);

        tokio::time::timeout(std::time::Duration::from_secs(1), dropped_rx)
            .await
            .expect("aborted dispatch task was not dropped")
            .expect("dispatch task drop signal was lost");
    }
}
