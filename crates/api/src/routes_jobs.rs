use anyhow::anyhow;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use tracing::warn;
use uuid::Uuid;
use vpsman_common::{
    encode_json, payload_hash, CommandOutput, GatewayCommandDispatchResult, JobCommand,
    OutputStream,
};
use vpsman_server_core::{CapabilitySkip, TargetCapability};

use crate::{
    error::ApiError,
    job_target_validation::validate_network_apply_target,
    model::{AgentView, AuthContext, CreateJobRequest, CreateJobResponse, WsEvent},
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
    validate_job_reconnect_metadata(&request)?;
    validate_selector_expression(&request.selector_expression)?;
    if request.destructive && !request.confirmed {
        return Err(ApiError::conflict("destructive_confirmation_required"));
    }
    let job_command = request.job_command()?;
    if !request.confirmed {
        match &job_command {
            JobCommand::Backup { .. } => {
                return Err(ApiError::conflict("backup_confirmation_required"));
            }
            JobCommand::HotConfig { .. }
            | JobCommand::DataSourceConfigPatch { .. }
            | JobCommand::UpdateAgent { .. }
            | JobCommand::AgentUpdateActivate { .. }
            | JobCommand::AgentUpdateRollback { .. }
            | JobCommand::AgentUpdateCheck { .. } => {
                return Err(ApiError::conflict("config_update_confirmation_required"));
            }
            JobCommand::FileWriteText { .. }
            | JobCommand::FileMkdir { .. }
            | JobCommand::FileRename { .. }
            | JobCommand::FileDelete { .. }
            | JobCommand::FileChmod { .. }
            | JobCommand::FileChown { .. }
            | JobCommand::FileCopy { .. } => {
                return Err(ApiError::conflict("file_operation_confirmation_required"));
            }
            _ => {}
        }
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
            "rejected_authorization_required",
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
            "dispatch_failed",
            "job has no resolved targets",
        )
        .await;
    }
    validate_network_apply_target(&job_command, &resolved_targets)?;
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
            "dispatch_failed",
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
    let _ = state.repo.refresh_job_status_from_targets(job_id).await?;
    crate::job_dispatcher::wake_job_dispatcher(state.clone());
    Ok((
        StatusCode::ACCEPTED,
        Json(CreateJobResponse {
            job_id,
            target_count: resolved_targets.len(),
            accepted_targets: 0,
            status: "dispatching".to_string(),
        }),
    ))
}

fn validate_job_reconnect_metadata(request: &CreateJobRequest) -> Result<(), ApiError> {
    if let Some(policy) = request.reconnect_policy.as_ref() {
        if !policy.is_object() {
            return Err(ApiError::bad_request("job_reconnect_policy_invalid"));
        }
        if serde_json::to_vec(policy)
            .map(|bytes| bytes.len() > 4096)
            .unwrap_or(true)
        {
            return Err(ApiError::bad_request("job_reconnect_policy_too_large"));
        }
    }
    Ok(())
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
        accepted_targets: state.repo.count_job_accepted_targets(existing.id).await?,
        status: existing.status,
    }))
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
    !matches!(status, "accepted" | "completed")
}

fn target_message_from_outputs(outputs: &[CommandOutput], fallback: &str, status: &str) -> String {
    if let Some(message) = outputs.iter().rev().find_map(status_output_message) {
        return message;
    }
    let trimmed = fallback.trim();
    if trimmed.is_empty() || trimmed == "accepted" {
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
        "status": "degraded_unprivileged",
        "client_id": skip.client_id,
        "command_type": crate::job_request::job_command_type_label(command),
        "reason": skip.failure.reason,
        "hint": skip.failure.hint,
    });
    Ok(TargetDispatchOutcome {
        status: "degraded_unprivileged".to_string(),
        exit_code: Some(1),
        command_version: None,
        accepted: false,
        message: skip.failure.message.to_string(),
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
        accepted_targets: 0,
        status: status.clone(),
    });
    Ok((
        StatusCode::FORBIDDEN,
        Json(CreateJobResponse {
            job_id,
            target_count,
            accepted_targets: 0,
            status,
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

#[derive(Debug)]
pub(crate) struct TargetDispatchOutcome {
    pub(crate) status: String,
    pub(crate) exit_code: Option<i32>,
    pub(crate) command_version: Option<u16>,
    pub(crate) accepted: bool,
    pub(crate) message: String,
    pub(crate) outputs: Vec<CommandOutput>,
}

pub(crate) fn target_outcome_from_gateway(
    result: GatewayCommandDispatchResult,
) -> TargetDispatchOutcome {
    if !result.accepted {
        let message =
            target_message_from_outputs(&result.outputs, &result.message, "rejected_by_agent");
        return TargetDispatchOutcome {
            status: "rejected_by_agent".to_string(),
            exit_code: None,
            command_version: Some(result.command_version),
            accepted: false,
            message,
            outputs: result.outputs,
        };
    }
    let final_output = result.outputs.iter().rev().find(|output| output.done);
    let exit_code = final_output.and_then(|output| output.exit_code);
    let status = if final_output.is_some_and(output_indicates_timeout) {
        "timed_out"
    } else {
        match exit_code {
            Some(0) => "completed",
            Some(_) => "failed",
            None => "accepted",
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
        command_version: Some(result.command_version),
        accepted: true,
        message,
        outputs: result.outputs,
    }
}

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
