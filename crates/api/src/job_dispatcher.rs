use std::time::Duration;

use anyhow::Result;
use futures_util::{stream, StreamExt};
use tracing::{debug, warn};
use uuid::Uuid;
use vpsman_common::{CommandOutput, JobCommand, JobRequest, OutputStream};

use crate::{
    backup_auto_artifacts::try_auto_record_backup_artifact,
    model::{AuthContext, BackupRequestStatus, CreateBackupRequest, WsEvent},
    repository_backups::BackupRequestSourceLink,
    repository_job_outputs::JobOutputPersistConfig,
    repository_jobs::ClaimedJobTarget,
    state::AppState,
    TargetDispatchOutcome,
};

const DISPATCH_BATCH_LIMIT: i64 = 32;
const DISPATCH_LEASE_SECS: i64 = 300;
const DISPATCH_INTERVAL_SECS: u64 = 1;
const DISPATCH_MAX_IN_FLIGHT: usize = 8;

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
    let mut outcome = match state.gateway.dispatch(&claimed.client_id, request).await {
        Ok(result) => crate::routes_jobs::target_outcome_from_gateway(result),
        Err(error) => {
            let message = error.to_string();
            warn!(
                job_id = %claimed.job_id,
                client_id = %claimed.client_id,
                error = %message,
                "gateway command dispatch failed"
            );
            TargetDispatchOutcome {
                status: "dispatch_failed".to_string(),
                exit_code: None,
                command_version: None,
                accepted: false,
                message,
                outputs: Vec::new(),
            }
        }
    };
    if let Some(reason) =
        crate::routes_jobs::protocol_mismatch_reason(&outcome, command_version, &claimed.operation)
    {
        outcome.message = crate::routes_jobs::stale_target_message(&outcome.message, &reason);
        state
            .repo
            .mark_agent_stale(
                &claimed.client_id,
                &reason,
                serde_json::json!({
                    "job_id": claimed.job_id,
                    "command_type": &claimed.command_type,
                    "requested_command_version": command_version,
                    "response_command_version": outcome.command_version,
                    "message": &outcome.message,
                }),
            )
            .await?;
    }
    finish_claimed_target(state, &claimed, outcome).await
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
    if matches!(&claimed.operation, JobCommand::Backup { .. }) && outcome.status == "completed" {
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
        if !matches!(status.as_str(), "queued" | "dispatching" | "running") {
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
    if actor_id == Uuid::nil() {
        return Ok(Some(AuthContext {
            operator: crate::model::OperatorView {
                id: Uuid::nil(),
                username: "memory-dev".to_string(),
                role: "admin".to_string(),
                scopes: crate::security::default_operator_scopes("admin"),
                preferences: crate::model::OperatorPreferences::default(),
                totp_enabled: false,
            },
            session_id: Uuid::nil(),
        }));
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
        "status": "dispatch_failed",
        "message": message,
    });
    TargetDispatchOutcome {
        status: "dispatch_failed".to_string(),
        exit_code: None,
        command_version: None,
        accepted: false,
        message: message.to_string(),
        outputs: vec![CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&status).unwrap_or_else(|_| message.as_bytes().to_vec()),
            exit_code: None,
            done: true,
        }],
    }
}
