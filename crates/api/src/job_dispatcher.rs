use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        OnceLock,
    },
    time::{Duration, Instant},
};

use anyhow::Result;
use futures_util::{stream, StreamExt};
use tokio::sync::Notify;
use tracing::{debug, warn};
use uuid::Uuid;
use vpsman_common::{CommandOutput, JobCommand, JobRequest, OutputStream};
use vpsman_server_core::{
    JOB_STATUS_QUEUED, JOB_STATUS_RUNNING, TARGET_STATUS_COMPLETED, TARGET_STATUS_CONTROL_TIMEOUT,
    TARGET_STATUS_FAILED, TARGET_STATUS_REJECTED,
};

use crate::{
    backup_auto_artifacts::try_auto_record_backup_artifact,
    model::{AuthContext, BackupRequestStatus, CreateBackupRequest, WsEvent},
    repository_backups::BackupRequestSourceLink,
    repository_job_outputs::{JobOutputPersistConfig, JobOutputWriteResult},
    repository_jobs::ClaimedJobTarget,
    state::AppState,
    TargetDispatchOutcome,
};

const DISPATCH_LEASE_SECS: i64 = 30;
const DISPATCH_INTERVAL_SECS: u64 = 1;
const DEADLINE_EXPIRE_LIMIT: i64 = 128;

struct DispatcherWakeState {
    notify: Notify,
    dispatching: AtomicBool,
    pending: AtomicBool,
    loop_started: AtomicBool,
    sweeps_started: AtomicU64,
    sweeps_coalesced: AtomicU64,
    targets_claimed: AtomicU64,
    dispatch_latency_micros_total: AtomicU64,
    dispatch_latency_samples: AtomicU64,
    gateway_dispatch_errors: AtomicU64,
}

impl Default for DispatcherWakeState {
    fn default() -> Self {
        Self {
            notify: Notify::new(),
            dispatching: AtomicBool::new(false),
            pending: AtomicBool::new(false),
            loop_started: AtomicBool::new(false),
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

pub(crate) fn spawn_job_dispatcher(state: AppState) {
    let wake_state = dispatcher_wake_state();
    wake_state.loop_started.store(true, Ordering::Release);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(DISPATCH_INTERVAL_SECS));
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = wake_state.notify.notified() => {}
            }
            if let Err(error) = run_dispatcher_sweep(&state).await {
                warn!(%error, "durable job dispatcher tick failed");
            }
        }
    });
}

pub(crate) fn wake_job_dispatcher(state: AppState) {
    let wake_state = dispatcher_wake_state();
    wake_state.pending.store(true, Ordering::Release);
    if wake_state.dispatching.load(Ordering::Acquire) {
        wake_state.sweeps_coalesced.fetch_add(1, Ordering::Relaxed);
    }
    wake_state.notify.notify_one();
    if !wake_state.loop_started.load(Ordering::Acquire) {
        tokio::spawn(async move {
            if let Err(error) = run_dispatcher_sweep(&state).await {
                warn!(%error, "durable job dispatcher wake failed");
            }
        });
    }
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
    expire_control_timeout_targets(state).await?;
    let dispatcher_config = state.dispatcher_runtime_config();
    let claimed = state
        .repo
        .claim_due_job_targets(
            dispatcher_config.batch_limit,
            DISPATCH_LEASE_SECS,
            dispatcher_config.control_deadline_extra_secs(),
        )
        .await?;
    let claimed_count = claimed.len();
    if claimed_count == 0 {
        return Ok(0);
    }
    dispatcher_wake_state()
        .targets_claimed
        .fetch_add(claimed_count as u64, Ordering::Relaxed);
    debug!(claimed_count, "durable job dispatcher claimed targets");
    stream::iter(claimed)
        .for_each_concurrent(dispatcher_config.in_flight, |claimed| {
            let state = state.clone();
            async move {
                if let Err(error) = dispatch_claimed_target(&state, claimed).await {
                    warn!(%error, "durable job target dispatch failed");
                }
            }
        })
        .await;
    Ok(claimed_count)
}

async fn dispatch_claimed_target(state: &AppState, claimed: ClaimedJobTarget) -> Result<()> {
    if !state.gateway.configured() {
        dispatcher_wake_state()
            .gateway_dispatch_errors
            .fetch_add(1, Ordering::Relaxed);
        let outcome = dispatch_error_outcome(claimed.job_id, "gateway control URL missing");
        return finish_claimed_target(state, &claimed, outcome).await;
    }

    if let Err(error) = record_backup_request_for_claim(state, &claimed).await {
        warn!(
            %error,
            job_id = %claimed.job_id,
            client_id = %claimed.client_id,
            "backup request pre-record failed"
        );
        let outcome = dispatch_error_outcome(claimed.job_id, "backup request pre-record failed");
        return finish_claimed_target(state, &claimed, outcome).await;
    }

    let command_version = crate::job_request::job_command_protocol_version(&claimed.operation);
    debug_assert!(
        command_version
            >= crate::job_request::job_command_min_supported_protocol_version(&claimed.operation)
    );
    let request = JobRequest {
        job_id: claimed.job_id,
        command_version,
        command: claimed.operation.clone(),
        timeout_secs: claimed.timeout_secs.clamp(1, 3600),
    };
    state.refresh_gateway_dispatch_timeouts();
    let outcome = match state
        .gateway
        .dispatch(
            &claimed.client_id,
            request,
            claimed.process_incarnation_id,
            claimed.payload_hash.clone(),
        )
        .await
    {
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
                    )
                    .await?;
                if let Some(status) = refreshed {
                    if !matches!(status.as_str(), JOB_STATUS_QUEUED | JOB_STATUS_RUNNING) {
                        state.publish(WsEvent::JobFinished {
                            job_id: claimed.job_id,
                            status,
                        });
                    }
                }
            } else {
                state
                    .repo
                    .record_job_target_delivery_error(claimed.job_id, &claimed.client_id, &message)
                    .await?;
            }
            return Ok(());
        }
    };
    if outcome.status == TARGET_STATUS_REJECTED || outcome.outputs.iter().any(|output| output.done)
    {
        return finish_claimed_target(state, &claimed, outcome).await;
    }
    state
        .repo
        .mark_job_target_running(claimed.job_id, &claimed.client_id, &outcome.message)
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

async fn expire_control_timeout_targets(state: &AppState) -> Result<()> {
    let dispatcher_config = state.dispatcher_runtime_config();
    let expired = state
        .repo
        .expire_control_timeout_targets(
            DEADLINE_EXPIRE_LIMIT,
            dispatcher_config.control_deadline_extra_secs(),
        )
        .await?;
    for target in expired {
        if target.status == TARGET_STATUS_CONTROL_TIMEOUT {
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
        }
        let refreshed = state
            .repo
            .refresh_job_status_from_targets(target.job_id)
            .await?;
        state
            .publish_job_finished_after_refresh(target.job_id, refreshed)
            .await?;
    }
    Ok(())
}

async fn finish_claimed_target(
    state: &AppState,
    claimed: &ClaimedJobTarget,
    outcome: TargetDispatchOutcome,
) -> Result<()> {
    let write_results = state
        .repo
        .record_job_outputs_checked_with_config(
            claimed.job_id,
            &claimed.client_id,
            &outcome.outputs,
            JobOutputPersistConfig {
                object_store: state.backup_object_store.as_ref(),
                artifact_min_bytes: state.job_output_artifact_min_bytes(),
            },
        )
        .await?;
    if write_results.contains(&JobOutputWriteResult::DuplicateConflict) {
        // A conflicting duplicate sequence means the gateway/agent replay stream is corrupt.
        // Retrying this event forever could terminalize from evidence we did not store, so keep
        // the target active for normal lifecycle handling and record the protocol error.
        state
            .repo
            .record_job_target_delivery_error(
                claimed.job_id,
                &claimed.client_id,
                "job_output_sequence_conflict",
            )
            .await?;
        return Ok(());
    }
    let target_terminalized = state
        .repo
        .update_job_target_result(claimed.job_id, &claimed.client_id, &outcome)
        .await?;
    if let Some((seq, output)) = outcome.outputs.iter().enumerate().next_back() {
        state.publish(WsEvent::JobOutputRecorded {
            job_id: claimed.job_id,
            client_id: claimed.client_id.clone(),
            seq: seq as i32,
            done: output.done,
        });
    }
    if target_terminalized
        && matches!(&claimed.operation, JobCommand::Backup { .. })
        && outcome.status == TARGET_STATUS_COMPLETED
    {
        if let Some(operator) = auth_context_for_claim(state, claimed).await? {
            if let Err(error) = try_auto_record_backup_artifact(
                state,
                &operator,
                &claimed.client_id,
                &claimed.payload_hash,
                claimed.job_id,
                &outcome.outputs,
            )
            .await
            {
                warn!(%error, job_id = %claimed.job_id, client_id = %claimed.client_id, "backup artifact auto-record failed");
            }
        }
    }
    if target_terminalized {
        let refreshed = state
            .repo
            .refresh_job_status_from_targets(claimed.job_id)
            .await?;
        state
            .publish_job_finished_after_refresh(claimed.job_id, refreshed)
            .await?;
    }
    Ok(())
}

async fn record_backup_request_for_claim(
    state: &AppState,
    claimed: &ClaimedJobTarget,
) -> Result<()> {
    let JobCommand::Backup {
        paths,
        include_config,
        recipient_public_key_hex,
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
    if let Some(request) = state
        .repo
        .find_open_backup_request_for_artifact(&claimed.client_id, &claimed.payload_hash)
        .await?
    {
        state
            .repo
            .attach_backup_request_source(
                request.id,
                Some(claimed.job_id),
                claimed.source_schedule_id,
                &operator,
            )
            .await?;
        return Ok(());
    }
    let request = CreateBackupRequest {
        client_id: claimed.client_id.clone(),
        paths: paths.clone(),
        include_config: *include_config,
        recipient_public_key_hex: recipient_public_key_hex.clone(),
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
            BackupRequestSourceLink {
                job_id: Some(claimed.job_id),
                schedule_id: claimed.source_schedule_id,
            },
        )
        .await?;
    Ok(())
}

async fn auth_context_for_claim(
    state: &AppState,
    claimed: &ClaimedJobTarget,
) -> Result<Option<AuthContext>> {
    let Some(actor_id) = claimed.actor_id else {
        return Ok(None);
    };
    if actor_id.is_nil() {
        return Ok(None);
    }
    let Some(operator) = state.repo.operator_by_id(actor_id).await? else {
        return Ok(None);
    };
    Ok(Some(AuthContext {
        operator: operator.view(),
        session_id: Uuid::nil(),
    }))
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
