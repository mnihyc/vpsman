use anyhow::anyhow;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use tracing::warn;
use uuid::Uuid;
use vpsman_common::{
    encode_json, job_command_requires_confirmation, payload_hash, CommandOutput,
    GatewayCommandDispatchResult, JobCancelRequest as GatewayJobCancelRequest, JobCommand,
    OutputStream,
};
use vpsman_server_core::{
    CapabilitySkip, TargetCapability, JOB_STATUS_FAILED, JOB_STATUS_QUEUED, JOB_STATUS_REJECTED,
    JOB_STATUS_RUNNING, TARGET_STATUS_AGENT_TIMEOUT, TARGET_STATUS_CANCELED,
    TARGET_STATUS_COMPLETED, TARGET_STATUS_CONTROL_TIMEOUT, TARGET_STATUS_DISPATCHING,
    TARGET_STATUS_FAILED, TARGET_STATUS_QUEUED, TARGET_STATUS_REJECTED, TARGET_STATUS_RUNNING,
    TARGET_STATUS_SKIPPED,
};

use crate::{
    error::ApiError,
    job_target_validation::validate_network_apply_target,
    model::{
        AgentView, AuthContext, CancelJobRequest, CancelJobResponse, CancelJobTargetResult,
        CreateJobRequest, CreateJobResponse, CreateJobTargetCounts, WsEvent,
    },
    privilege::{verify_privilege_intent, JobPrivilegeIntent, JobPrivilegeIntentInput},
    repository_job_outputs::JobOutputPersistConfig,
    selector_expression::parse_selector_expression,
    state::AppState,
};

pub(crate) async fn create_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateJobRequest>,
) -> Result<(StatusCode, Json<CreateJobResponse>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    create_job_with_operator(&state, &operator, request).await
}

pub(crate) async fn cancel_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
    Json(request): Json<CancelJobRequest>,
) -> Result<(StatusCode, Json<CancelJobResponse>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    if state.repo.get_job(job_id).await?.is_none() {
        return Err(ApiError::not_found("job_not_found"));
    }
    let reason = bounded_cancel_reason(request.reason.as_deref());
    let plan = state
        .repo
        .request_job_cancel(job_id, operator.operator.id, reason.as_deref())
        .await?;
    let mut cancel_acks = Vec::with_capacity(plan.cancel_targets.len());
    for client_id in &plan.cancel_targets {
        state
            .repo
            .record_job_target_cancel_sent(job_id, client_id)
            .await?;
        let result = state
            .gateway
            .cancel(
                client_id,
                GatewayJobCancelRequest {
                    job_id,
                    reason: reason.clone(),
                },
            )
            .await;
        match result {
            Ok(cancel) => {
                state
                    .repo
                    .record_job_target_cancel_result(
                        job_id,
                        client_id,
                        cancel.accepted,
                        cancel.acked,
                        cancel.applied,
                        &cancel.message,
                    )
                    .await?;
                cancel_acks.push(CancelJobTargetResult {
                    client_id: client_id.clone(),
                    acked: cancel.acked,
                    accepted: cancel.accepted,
                    applied: cancel.applied,
                    message: cancel.message,
                });
            }
            Err(error) => {
                let message = format!("cancel delivery failed: {error}");
                warn!(%error, %job_id, client_id, "job cancel delivery failed");
                state
                    .repo
                    .record_job_target_cancel_result(
                        job_id, client_id, false, false, false, &message,
                    )
                    .await?;
                cancel_acks.push(CancelJobTargetResult {
                    client_id: client_id.clone(),
                    acked: false,
                    accepted: false,
                    applied: false,
                    message,
                });
            }
        }
    }
    let refreshed = state.repo.refresh_job_status_from_targets(job_id).await?;
    if let Some(status) = &refreshed {
        if !matches!(status.as_str(), JOB_STATUS_QUEUED | JOB_STATUS_RUNNING) {
            state.publish(WsEvent::JobFinished {
                job_id,
                status: status.clone(),
            });
        }
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(CancelJobResponse {
            job_id,
            requested_targets: plan.cancel_targets.len() + plan.pending_canceled,
            pending_canceled: plan.pending_canceled,
            cancel_acks,
            status: refreshed,
        }),
    ))
}

pub(crate) async fn create_job_with_operator(
    state: &AppState,
    operator: &AuthContext,
    request: CreateJobRequest,
) -> Result<(StatusCode, Json<CreateJobResponse>), ApiError> {
    create_job_inner(
        state,
        operator,
        request,
        JobPrivilegeSource::RequestAssertion,
    )
    .await
}

pub(crate) async fn create_job_from_saved_schedule(
    state: &AppState,
    operator: &AuthContext,
    request: CreateJobRequest,
    schedule_id: Uuid,
) -> Result<(StatusCode, Json<CreateJobResponse>), ApiError> {
    create_job_inner(
        state,
        operator,
        request,
        JobPrivilegeSource::SavedSchedule(schedule_id),
    )
    .await
}

enum JobPrivilegeSource {
    RequestAssertion,
    SavedSchedule(Uuid),
}

async fn create_job_inner(
    state: &AppState,
    operator: &AuthContext,
    request: CreateJobRequest,
    privilege_source: JobPrivilegeSource,
) -> Result<(StatusCode, Json<CreateJobResponse>), ApiError> {
    validate_selector_expression(&request.selector_expression)?;
    if request.destructive && !request.confirmed {
        return Err(ApiError::conflict("destructive_confirmation_required"));
    }
    let job_command = request.job_command()?;
    if !request.confirmed && job_command_requires_confirmation(&job_command) {
        return Err(ApiError::conflict(confirmation_error_code(&job_command)));
    }
    let command_payload = encode_json(&job_command).map_err(|error| {
        ApiError::from(anyhow!(
            "failed to encode job command for authorization: {error}"
        ))
    })?;
    let command_hash = payload_hash(&command_payload);
    let job_id = request.job_id.unwrap_or_else(Uuid::new_v4);
    let fixed_target_ids = request.fixed_target_ids()?;
    let target_selection = request.target_selection()?;
    let resolved_agents = state
        .repo
        .resolve_bulk_targets(&target_selection)
        .await?
        .targets;
    let resolved_targets = fixed_target_ids;
    if resolved_targets
        .iter()
        .any(|client_id| !resolved_agents.iter().any(|agent| agent.id == *client_id))
    {
        return Err(ApiError::conflict("fixed_target_not_found"));
    }
    if matches!(job_command, JobCommand::ConfigRead) && resolved_targets.len() != 1 {
        return Err(ApiError::conflict("config_read_requires_single_target"));
    }
    let source_schedule_id = match privilege_source {
        JobPrivilegeSource::RequestAssertion => None,
        JobPrivilegeSource::SavedSchedule(schedule_id) => Some(schedule_id),
    };
    let request_fingerprint = request_fingerprint_for_job(
        &request,
        &command_hash,
        &resolved_targets,
        source_schedule_id,
    )?;
    if let Some(response) =
        existing_job_response_for_id(state, operator, job_id, &request_fingerprint).await?
    {
        return Ok((StatusCode::OK, Json(response)));
    }

    if !request.privileged {
        return reject_job(
            state,
            job_id,
            &request,
            &command_hash,
            &request_fingerprint,
            operator,
            JOB_STATUS_REJECTED,
            "all non-telemetry jobs require privilege unlock",
        )
        .await;
    }
    if resolved_targets.is_empty() {
        return reject_job(
            state,
            job_id,
            &request,
            &command_hash,
            &request_fingerprint,
            operator,
            JOB_STATUS_FAILED,
            "job has no resolved targets",
        )
        .await;
    }
    validate_network_apply_target(&job_command, &resolved_targets)?;
    validate_agent_command_timeout_cap(
        request.timeout_secs.unwrap_or(30),
        &resolved_targets,
        &resolved_agents,
    )?;
    if !agent_update_release_policy_allows(state, &job_command).await? {
        return reject_job(
            state,
            job_id,
            &request,
            &command_hash,
            &request_fingerprint,
            operator,
            "failed",
            "registered agent update release missing",
        )
        .await;
    }
    if matches!(privilege_source, JobPrivilegeSource::RequestAssertion) {
        let privilege_intent = JobPrivilegeIntent::new(JobPrivilegeIntentInput {
            selector_expression: &request.selector_expression,
            command_type: request.command_type_label(),
            operation_payload_hash: &command_hash,
            resolved_targets: &resolved_targets,
            timeout_secs: request.timeout_secs.unwrap_or(30),
            force_unprivileged: request.force_unprivileged,
            privileged: request.privileged,
        });
        verify_privilege_intent(
            state,
            &privilege_intent,
            request.privilege_assertion.clone(),
        )
        .await?;
    }
    let (dispatch_targets, capability_skips) = split_targets_by_capability(
        &job_command,
        &resolved_targets,
        &resolved_agents,
        request.force_unprivileged,
    );
    if !dispatch_targets.is_empty() && !state.gateway.configured() {
        return reject_job(
            state,
            job_id,
            &request,
            &command_hash,
            &request_fingerprint,
            operator,
            JOB_STATUS_FAILED,
            "gateway control URL missing",
        )
        .await;
    }

    if let Some(schedule_id) = source_schedule_id {
        state
            .repo
            .record_dispatching_job_from_schedule(
                job_id,
                &request,
                &command_hash,
                &request_fingerprint,
                operator,
                &resolved_targets,
                schedule_id,
            )
            .await?
    } else {
        state
            .repo
            .record_dispatching_job(
                job_id,
                &request,
                &command_hash,
                &request_fingerprint,
                operator,
                &resolved_targets,
            )
            .await?
    };
    precomplete_capability_skips(state, job_id, &job_command, capability_skips).await?;
    let status = state
        .repo
        .refresh_job_status_from_targets(job_id)
        .await?
        .unwrap_or_else(|| JOB_STATUS_RUNNING.to_string());
    crate::job_dispatcher::wake_job_dispatcher(state.clone());
    let target_counts = create_job_target_counts(state, job_id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(CreateJobResponse {
            job_id,
            target_count: resolved_targets.len(),
            status,
            target_counts,
        }),
    ))
}

fn confirmation_error_code(command: &JobCommand) -> &'static str {
    match command {
        JobCommand::Backup { .. } => "backup_confirmation_required",
        JobCommand::HotConfig { .. }
        | JobCommand::DataSourceConfigPatch { .. }
        | JobCommand::UpdateAgent { .. }
        | JobCommand::AgentUpdateActivate { .. }
        | JobCommand::AgentUpdateRollback { .. }
        | JobCommand::AgentUpdateCheck { .. } => "config_update_confirmation_required",
        JobCommand::FilePush { .. }
        | JobCommand::FilePushChunked { .. }
        | JobCommand::FileTransferStart { .. }
        | JobCommand::FileTransferChunk { .. }
        | JobCommand::FileTransferCommit { .. }
        | JobCommand::FileTransferAbort { .. }
        | JobCommand::FileWriteText { .. }
        | JobCommand::FileMkdir { .. }
        | JobCommand::FileRename { .. }
        | JobCommand::FileDelete { .. }
        | JobCommand::FileChmod { .. }
        | JobCommand::FileChown { .. }
        | JobCommand::FileCopy { .. } => "file_operation_confirmation_required",
        _ => "command_confirmation_required",
    }
}

fn request_fingerprint_for_job(
    request: &CreateJobRequest,
    command_hash: &str,
    resolved_targets: &[String],
    source_schedule_id: Option<Uuid>,
) -> Result<String, ApiError> {
    let mut targets = resolved_targets.to_vec();
    targets.sort();
    let bytes = serde_json::to_vec(&serde_json::json!({
        "selector_expression": request.selector_expression.trim(),
        "command_type": request.command_type_label(),
        "operation_payload_hash": command_hash,
        "targets": targets,
        "timeout_secs": request.timeout_secs.unwrap_or(30),
        "privileged": request.privileged,
        "force_unprivileged": request.force_unprivileged,
        "source_schedule_id": source_schedule_id,
    }))
    .map_err(|error| ApiError::from(anyhow!("failed to encode job fingerprint: {error}")))?;
    Ok(payload_hash(&bytes))
}

async fn existing_job_response_for_id(
    state: &AppState,
    operator: &AuthContext,
    job_id: Uuid,
    request_fingerprint: &str,
) -> Result<Option<CreateJobResponse>, ApiError> {
    let Some(existing) = state.repo.get_job(job_id).await? else {
        return Ok(None);
    };
    if existing.actor_id != Some(operator.operator.id) {
        return Err(ApiError::conflict("job_id_reused_by_different_actor"));
    }
    let stored_fingerprint = state
        .repo
        .get_job_request_fingerprint(job_id)
        .await?
        .unwrap_or_default();
    if stored_fingerprint != request_fingerprint {
        return Err(ApiError::conflict("job_id_reused_with_different_request"));
    }
    Ok(Some(CreateJobResponse {
        job_id: existing.id,
        target_count: existing.target_count.max(0) as usize,
        status: existing.status,
        target_counts: create_job_target_counts(state, existing.id).await?,
    }))
}

async fn create_job_target_counts(
    state: &AppState,
    job_id: Uuid,
) -> Result<CreateJobTargetCounts, ApiError> {
    let targets = state.repo.list_job_targets(job_id).await?;
    let mut counts = CreateJobTargetCounts {
        total: targets.len(),
        queued: 0,
        dispatching: 0,
        running: 0,
        completed: 0,
        skipped: 0,
        rejected: 0,
        failed: 0,
        agent_timeout: 0,
        control_timeout: 0,
        canceled: 0,
    };
    for target in targets {
        match target.status.as_str() {
            TARGET_STATUS_QUEUED => counts.queued += 1,
            TARGET_STATUS_DISPATCHING => counts.dispatching += 1,
            TARGET_STATUS_RUNNING => counts.running += 1,
            TARGET_STATUS_COMPLETED => counts.completed += 1,
            TARGET_STATUS_SKIPPED => counts.skipped += 1,
            TARGET_STATUS_REJECTED => counts.rejected += 1,
            TARGET_STATUS_FAILED => counts.failed += 1,
            TARGET_STATUS_AGENT_TIMEOUT => counts.agent_timeout += 1,
            TARGET_STATUS_CONTROL_TIMEOUT => counts.control_timeout += 1,
            TARGET_STATUS_CANCELED => counts.canceled += 1,
            _ => counts.failed += 1,
        }
    }
    Ok(counts)
}

async fn agent_update_release_policy_allows(
    state: &AppState,
    job_command: &JobCommand,
) -> Result<bool, ApiError> {
    if !state.require_registered_agent_updates {
        return Ok(true);
    }
    let JobCommand::UpdateAgent {
        sha256_hex,
        artifact_signing_key_hex,
        ..
    } = job_command
    else {
        return Ok(true);
    };
    state
        .repo
        .agent_update_release_exists_for_artifact(sha256_hex, artifact_signing_key_hex.as_deref())
        .await
        .map_err(ApiError::from)
}

async fn precomplete_capability_skips(
    state: &AppState,
    job_id: Uuid,
    job_command: &JobCommand,
    capability_skips: Vec<CapabilitySkip>,
) -> Result<Vec<String>, ApiError> {
    let mut precompleted_statuses = Vec::new();
    for skip in capability_skips {
        let outcome = capability_degraded_outcome(job_id, &skip, job_command)?;
        precompleted_statuses.push(outcome.status.clone());
        state
            .repo
            .update_job_target_result(job_id, &skip.client_id, &outcome)
            .await?;
        state
            .repo
            .record_job_outputs_with_config(
                job_id,
                &skip.client_id,
                &outcome.outputs,
                JobOutputPersistConfig {
                    object_store: state.backup_object_store.as_ref(),
                    artifact_min_bytes: state.job_output_artifact_min_bytes,
                },
            )
            .await?;
        state.publish(WsEvent::JobOutputRecorded {
            job_id,
            client_id: skip.client_id,
            seq: 0,
            done: true,
        });
    }
    Ok(precompleted_statuses)
}

#[cfg(test)]
pub(crate) fn stale_target_message(message: &str, reason: &str) -> String {
    let trimmed = message.trim();
    if trimmed.to_ascii_lowercase().contains("stale") {
        return trimmed.to_string();
    }
    if trimmed.is_empty() || trimmed == reason {
        return format!("stale: {reason}");
    }
    format!("stale: {reason}; {trimmed}")
}

fn target_status_needs_reason(status: &str) -> bool {
    !matches!(status, TARGET_STATUS_RUNNING | TARGET_STATUS_COMPLETED)
}

fn target_message_from_outputs(outputs: &[CommandOutput], fallback: &str, status: &str) -> String {
    if let Some(message) = outputs.iter().rev().find_map(status_output_message) {
        return message;
    }
    let trimmed = fallback.trim();
    if trimmed.is_empty() || trimmed == TARGET_STATUS_RUNNING {
        status.to_string()
    } else {
        trimmed.to_string()
    }
}

fn status_output_message(output: &CommandOutput) -> Option<String> {
    if output.stream != OutputStream::Status {
        return None;
    }
    let value = serde_json::from_slice::<serde_json::Value>(&output.data).ok()?;
    status_value_message(&value)
}

fn status_value_message(value: &serde_json::Value) -> Option<String> {
    let kind = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let primary = ["message", "error", "reason", "hint", "status"]
        .iter()
        .find_map(|field| {
            value
                .get(*field)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        });
    match (kind, primary) {
        (Some(kind), Some(primary)) if kind != primary => Some(format!("{kind}: {primary}")),
        (Some(kind), _) => Some(kind.to_string()),
        (_, Some(primary)) => Some(primary.to_string()),
        _ => None,
    }
}

fn split_targets_by_capability(
    command: &JobCommand,
    targets: &[String],
    agents: &[AgentView],
    force_unprivileged: bool,
) -> (Vec<String>, Vec<CapabilitySkip>) {
    let capabilities = agents
        .iter()
        .map(|agent| TargetCapability {
            client_id: agent.id.clone(),
            capabilities: agent.capabilities.clone(),
        })
        .collect::<Vec<_>>();
    vpsman_server_core::split_targets_by_capability(
        command,
        targets,
        &capabilities,
        force_unprivileged,
    )
}

fn capability_degraded_outcome(
    job_id: Uuid,
    skip: &CapabilitySkip,
    command: &JobCommand,
) -> Result<TargetDispatchOutcome, ApiError> {
    let status = serde_json::json!({
        "type": "capability_degraded",
        "status": TARGET_STATUS_SKIPPED,
        "client_id": skip.client_id,
        "command_type": crate::job_request::job_command_type_label(command),
        "reason": skip.failure.reason,
        "hint": skip.failure.hint,
    });
    Ok(TargetDispatchOutcome {
        status: TARGET_STATUS_SKIPPED.to_string(),
        exit_code: Some(0),
        #[cfg(test)]
        command_version: None,
        accepted: false,
        message: skip.failure.message.to_string(),
        received_at: None,
        outputs: vec![CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&status).map_err(|error| ApiError::from(anyhow!(error)))?,
            exit_code: Some(1),
            done: true,
        }],
    })
}

async fn reject_job(
    state: &AppState,
    job_id: Uuid,
    request: &CreateJobRequest,
    command_hash: &str,
    request_fingerprint: &str,
    operator: &AuthContext,
    status: &'static str,
    reason: &'static str,
) -> Result<(StatusCode, Json<CreateJobResponse>), ApiError> {
    let job_id = state
        .repo
        .record_rejected_job(
            job_id,
            request,
            command_hash,
            request_fingerprint,
            operator,
            status,
            reason,
        )
        .await?;
    let status = status.to_string();
    let target_count = state
        .repo
        .get_job(job_id)
        .await?
        .map(|job| job.target_count.max(0) as usize)
        .unwrap_or_default();
    warn!(
        selector_expression = %request.selector_expression,
        privileged = request.privileged,
        command_hash,
        reason,
        "job rejected before dispatch"
    );
    state.publish(WsEvent::JobRejected {
        job_id,
        status: status.clone(),
    });
    let target_counts = create_job_target_counts(state, job_id).await?;
    Ok((
        StatusCode::FORBIDDEN,
        Json(CreateJobResponse {
            job_id,
            target_count,
            status,
            target_counts,
        }),
    ))
}

fn validate_selector_expression(selector_expression: &str) -> Result<(), ApiError> {
    let selector_expression = selector_expression.trim();
    if selector_expression.is_empty() {
        return Ok(());
    }
    parse_selector_expression(selector_expression)
        .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?;
    Ok(())
}

fn validate_agent_command_timeout_cap(
    requested_timeout_secs: u64,
    resolved_targets: &[String],
    resolved_agents: &[AgentView],
) -> Result<(), ApiError> {
    let requested_timeout_secs = requested_timeout_secs.clamp(1, 3600);
    let timeout_too_low = resolved_targets.iter().any(|client_id| {
        resolved_agents
            .iter()
            .find(|agent| agent.id == *client_id)
            .is_some_and(|agent| agent.capabilities.command_timeout_secs < requested_timeout_secs)
    });
    if timeout_too_low {
        return Err(ApiError::conflict("agent_command_timeout_too_low"));
    }
    Ok(())
}

fn bounded_cancel_reason(reason: Option<&str>) -> Option<String> {
    reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(512).collect())
}

#[derive(Debug)]
pub(crate) struct TargetDispatchOutcome {
    pub(crate) status: String,
    pub(crate) exit_code: Option<i32>,
    #[cfg(test)]
    pub(crate) command_version: Option<u16>,
    pub(crate) accepted: bool,
    pub(crate) message: String,
    pub(crate) received_at: Option<String>,
    pub(crate) outputs: Vec<CommandOutput>,
}

pub(crate) fn target_outcome_from_gateway(
    result: GatewayCommandDispatchResult,
) -> TargetDispatchOutcome {
    if !result.accepted {
        let message =
            target_message_from_outputs(&result.outputs, &result.message, TARGET_STATUS_REJECTED);
        return TargetDispatchOutcome {
            status: TARGET_STATUS_REJECTED.to_string(),
            exit_code: None,
            #[cfg(test)]
            command_version: Some(result.command_version),
            accepted: false,
            message,
            received_at: None,
            outputs: result.outputs,
        };
    }
    let final_output = result.outputs.iter().rev().find(|output| output.done);
    let exit_code = final_output.and_then(|output| output.exit_code);
    let status = if final_output.is_some_and(output_indicates_timeout) {
        TARGET_STATUS_AGENT_TIMEOUT
    } else {
        match exit_code {
            Some(0) => TARGET_STATUS_COMPLETED,
            Some(_) => TARGET_STATUS_FAILED,
            None => TARGET_STATUS_RUNNING,
        }
    };
    let message = if target_status_needs_reason(status) {
        target_message_from_outputs(&result.outputs, &result.message, status)
    } else {
        result.message
    };
    TargetDispatchOutcome {
        status: status.to_string(),
        exit_code,
        #[cfg(test)]
        command_version: Some(result.command_version),
        accepted: true,
        message,
        received_at: None,
        outputs: result.outputs,
    }
}

#[cfg(test)]
pub(crate) fn protocol_mismatch_reason(
    outcome: &TargetDispatchOutcome,
    expected_command_version: u16,
    command: &JobCommand,
) -> Option<String> {
    if outcome
        .command_version
        .is_some_and(|seen| seen < expected_command_version)
    {
        return Some("agent_returned_lower_command_version".to_string());
    }
    if outcome.message == "unsupported_command_version" {
        return Some("agent_rejected_unsupported_command_version".to_string());
    }
    let command_type = crate::job_request::job_command_type_label(command);
    outcome.outputs.iter().find_map(|output| {
        if output.stream != OutputStream::Status {
            return None;
        }
        let value = serde_json::from_slice::<serde_json::Value>(&output.data).ok()?;
        let kind = value.get("type").and_then(serde_json::Value::as_str)?;
        if kind == "unsupported_command_version" {
            return Some(format!(
                "agent_rejected_unsupported_{command_type}_command_version"
            ));
        }
        let response_version = value
            .get("command_version")
            .and_then(serde_json::Value::as_u64)
            .and_then(|version| u16::try_from(version).ok())?;
        (response_version < expected_command_version)
            .then(|| format!("agent_returned_lower_{command_type}_command_version"))
    })
}

pub(crate) fn output_indicates_timeout(output: &CommandOutput) -> bool {
    if output.stream != OutputStream::Status {
        return false;
    }
    serde_json::from_slice::<serde_json::Value>(&output.data)
        .ok()
        .and_then(|value| {
            value
                .get("type")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .is_some_and(|kind| kind == "command_timeout")
}

#[cfg(test)]
mod tests;
