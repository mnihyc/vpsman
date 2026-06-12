use std::time::Duration;

use anyhow::Result;
use futures_util::{stream, StreamExt};
use tracing::{debug, warn};
use uuid::Uuid;
use vpsman_common::{CommandOutput, JobCommand, JobRequest, OutputStream};
use vpsman_server_core::{
    JOB_STATUS_PENDING, JOB_STATUS_RUNNING, TARGET_STATUS_FAILED, TARGET_STATUS_REJECTED,
    TARGET_STATUS_SUCCEEDED,
};

use crate::{
    backup_auto_artifacts::try_auto_record_backup_artifact,
    model::{AuthContext, BackupRequestStatus, CreateBackupRequest, WsEvent},
    repository_backups::BackupRequestSourceLink,
    repository_job_outputs::JobOutputPersistConfig,
    repository_jobs::ClaimedJobTarget,
    state::AppState,
    TargetDispatchOutcome,
};

const DISPATCH_BATCH_LIMIT: i64 = 128;
const DISPATCH_LEASE_SECS: i64 = 30;
const DISPATCH_INTERVAL_SECS: u64 = 1;
const DISPATCH_MAX_IN_FLIGHT: usize = 64;
const DEADLINE_EXPIRE_LIMIT: i64 = 128;

pub(crate) fn spawn_job_dispatcher(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(DISPATCH_INTERVAL_SECS));
        loop {
            interval.tick().await;
            if let Err(error) = dispatch_due_job_targets(&state).await {
                warn!(%error, "durable job dispatcher tick failed");
            }
        }
    });
}

pub(crate) fn wake_job_dispatcher(state: AppState) {
    tokio::spawn(async move {
        if let Err(error) = dispatch_due_job_targets(&state).await {
            warn!(%error, "durable job dispatcher wake failed");
        }
    });
}

pub(crate) async fn dispatch_due_job_targets(state: &AppState) -> Result<usize> {
    expire_control_timed_out_targets(state).await?;
    let claimed = state
        .repo
        .claim_due_job_targets(DISPATCH_BATCH_LIMIT, DISPATCH_LEASE_SECS)
        .await?;
    let claimed_count = claimed.len();
    if claimed_count == 0 {
        return Ok(0);
    }
    debug!(claimed_count, "durable job dispatcher claimed targets");
    stream::iter(claimed)
        .for_each_concurrent(DISPATCH_MAX_IN_FLIGHT, |claimed| {
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
        let outcome = dispatch_failed_outcome(claimed.job_id, "gateway control URL missing");
        return finish_claimed_target(state, &claimed, outcome).await;
    }

    if let Err(error) = record_backup_request_for_claim(state, &claimed).await {
        warn!(
            %error,
            job_id = %claimed.job_id,
            client_id = %claimed.client_id,
            "backup request pre-record failed"
        );
        let outcome = dispatch_failed_outcome(claimed.job_id, "backup request pre-record failed");
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
    let outcome = match state.gateway.dispatch(&claimed.client_id, request).await {
        Ok(result) => crate::routes_jobs::target_outcome_from_gateway(result),
        Err(error) => {
            let message = error.to_string();
            warn!(
                job_id = %claimed.job_id,
                client_id = %claimed.client_id,
                error = %message,
                "gateway command dispatch failed"
            );
            state
                .repo
                .record_job_target_delivery_error(claimed.job_id, &claimed.client_id, &message)
                .await?;
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

async fn expire_control_timed_out_targets(state: &AppState) -> Result<()> {
    let expired = state
        .repo
        .expire_control_timed_out_targets(DEADLINE_EXPIRE_LIMIT)
        .await?;
    for target in expired {
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
        if let Some((status, accepted_targets)) = state
            .repo
            .refresh_job_status_from_targets(target.job_id)
            .await?
        {
            if !matches!(status.as_str(), JOB_STATUS_PENDING | JOB_STATUS_RUNNING) {
                state.publish(WsEvent::JobFinished {
                    job_id: target.job_id,
                    accepted_targets,
                    status,
                });
            }
        }
    }
    Ok(())
}

async fn finish_claimed_target(
    state: &AppState,
    claimed: &ClaimedJobTarget,
    outcome: TargetDispatchOutcome,
) -> Result<()> {
    state
        .repo
        .update_job_target_result(claimed.job_id, &claimed.client_id, &outcome)
        .await?;
    state
        .repo
        .record_job_outputs_with_config(
            claimed.job_id,
            &claimed.client_id,
            &outcome.outputs,
            JobOutputPersistConfig {
                object_store: state.backup_object_store.as_ref(),
                artifact_min_bytes: state.job_output_artifact_min_bytes,
            },
        )
        .await?;
    if let Some((seq, output)) = outcome.outputs.iter().enumerate().next_back() {
        state.publish(WsEvent::JobOutputRecorded {
            job_id: claimed.job_id,
            client_id: claimed.client_id.clone(),
            seq: seq as i32,
            done: output.done,
        });
    }
    if matches!(&claimed.operation, JobCommand::Backup { .. })
        && outcome.status == TARGET_STATUS_SUCCEEDED
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
    if let Some((status, accepted_targets)) = state
        .repo
        .refresh_job_status_from_targets(claimed.job_id)
        .await?
    {
        if !matches!(status.as_str(), JOB_STATUS_PENDING | JOB_STATUS_RUNNING) {
            state.publish(WsEvent::JobFinished {
                job_id: claimed.job_id,
                accepted_targets,
                status,
            });
        }
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

fn dispatch_failed_outcome(job_id: Uuid, message: &str) -> TargetDispatchOutcome {
    let status = serde_json::json!({
        "type": "dispatch_failed",
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
