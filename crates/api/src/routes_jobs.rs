use std::collections::HashMap;

use anyhow::{anyhow, Context};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use futures_util::future::join_all;
use tracing::warn;
use uuid::Uuid;
use vpsman_common::{
    encode_json, payload_hash, AgentPrivilegeMode, CommandEnvelope, CommandOutput,
    GatewayCommandDispatchResult, JobCommand, JobRequest, OutputStream,
};

use crate::{
    backup_auto_artifacts::try_auto_record_backup_artifact,
    error::ApiError,
    job_target_validation::validate_network_apply_target,
    model::{
        AgentView, AuthContext, BackupRequestStatus, CreateBackupRequest, CreateJobRequest,
        CreateJobResponse, JobHistoryView, WsEvent,
    },
    privilege::{verify_privilege_intent, JobPrivilegeIntent, JobPrivilegeIntentInput},
    repository_backups::BackupRequestSourceLink,
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
    if let Some(response) =
        idempotent_job_response(state, operator, &request, &command_hash).await?
    {
        return Ok((StatusCode::OK, Json(response)));
    }
    let resolved_agents = state
        .repo
        .resolve_bulk_targets(&request.target_selection())
        .await?
        .targets;
    let resolved_targets = resolved_agents
        .iter()
        .map(|agent| agent.id.clone())
        .collect::<Vec<_>>();
    if matches!(job_command, JobCommand::ConfigRead) && resolved_targets.len() != 1 {
        return Err(ApiError::conflict("config_read_requires_single_target"));
    }

    let Some(signing_key) = state.server_signing_key.as_deref() else {
        return reject_job(
            state,
            &request,
            &command_hash,
            operator,
            "server signing key missing",
        )
        .await;
    };
    if !request.privileged {
        return reject_job(
            state,
            &request,
            &command_hash,
            operator,
            "all non-telemetry jobs require privilege unlock",
        )
        .await;
    }
    if resolved_targets.is_empty() {
        return reject_job(
            state,
            &request,
            &command_hash,
            operator,
            "job has no resolved targets",
        )
        .await;
    }
    validate_network_apply_target(&job_command, &resolved_targets)?;
    if !agent_update_release_policy_allows(state, &job_command).await? {
        return reject_job(
            state,
            &request,
            &command_hash,
            operator,
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
    let signed_envelopes =
        match request.signed_envelopes_for_targets(&resolved_targets, &command_hash, signing_key) {
            Ok(envelopes) => envelopes,
            Err(error) => {
                warn!(%error, command_hash, "job rejected because command envelope signing failed");
                return reject_job(
                    state,
                    &request,
                    &command_hash,
                    operator,
                    "command envelope signing failed",
                )
                .await;
            }
        };
    let (dispatch_targets, capability_skips) = split_targets_by_capability(
        &job_command,
        &resolved_targets,
        &resolved_agents,
        request.force_unprivileged,
    );
    if !dispatch_targets.is_empty() && !state.gateway.configured() {
        return reject_job(
            state,
            &request,
            &command_hash,
            operator,
            "gateway control URL missing",
        )
        .await;
    }

    let source_schedule_id = match privilege_source {
        JobPrivilegeSource::RequestAssertion => None,
        JobPrivilegeSource::SavedSchedule(schedule_id) => Some(schedule_id),
    };
    let job_id = if let Some(schedule_id) = source_schedule_id {
        state
            .repo
            .record_dispatching_job_from_schedule(
                &request,
                &command_hash,
                operator,
                &resolved_targets,
                schedule_id,
            )
            .await?
    } else {
        state
            .repo
            .record_dispatching_job(&request, &command_hash, operator, &resolved_targets)
            .await?
    };
    let precompleted_statuses =
        precomplete_capability_skips(state, job_id, &job_command, capability_skips).await?;
    let accepted_targets = dispatch_targets.len();
    let dispatch_state = state.clone();
    let dispatch_batch = GatewayDispatchBatch {
        job_id,
        job_command,
        timeout_secs: request.timeout_secs.unwrap_or(30),
        targets: dispatch_targets,
        signed_envelopes,
        precompleted_statuses,
        target_count: resolved_targets.len(),
        source_schedule_id,
        context: DispatchContext {
            operator: operator.clone(),
            command_hash: command_hash.clone(),
        },
    };
    tokio::spawn(async move {
        if let Err(error) = dispatch_to_gateway(&dispatch_state, dispatch_batch).await {
            warn!(?error, job_id = %job_id, "background gateway dispatch failed");
            if let Err(finish_error) = dispatch_state
                .repo
                .finish_job(job_id, "dispatch_failed")
                .await
            {
                warn!(?finish_error, job_id = %job_id, "failed to mark background dispatch failure");
            }
            dispatch_state.publish(WsEvent::JobFinished {
                job_id,
                accepted_targets: 0,
                status: "dispatch_failed".to_string(),
            });
        }
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(CreateJobResponse {
            job_id,
            accepted_targets,
            status: "dispatching".to_string(),
        }),
    ))
}

fn validate_job_reconnect_metadata(request: &CreateJobRequest) -> Result<(), ApiError> {
    if let Some(key) = request.idempotency_key.as_deref() {
        let key = key.trim();
        if key.is_empty()
            || key.len() > 128
            || !key.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':')
            })
        {
            return Err(ApiError::bad_request("job_idempotency_key_invalid"));
        }
    }
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

async fn idempotent_job_response(
    state: &AppState,
    operator: &AuthContext,
    request: &CreateJobRequest,
    command_hash: &str,
) -> Result<Option<CreateJobResponse>, ApiError> {
    let Some(key) = request.idempotency_key.as_deref() else {
        return Ok(None);
    };
    let Some(existing) = state
        .repo
        .find_job_by_idempotency_key(operator.operator.id, key)
        .await?
    else {
        return Ok(None);
    };
    if existing.payload_hash != command_hash {
        return Err(ApiError::conflict(
            "job_idempotency_key_reused_with_different_payload",
        ));
    }
    Ok(Some(CreateJobResponse {
        job_id: existing.id,
        accepted_targets: accepted_targets_for_existing_job(&existing),
        status: existing.status,
    }))
}

fn accepted_targets_for_existing_job(existing: &JobHistoryView) -> usize {
    if existing.status.starts_with("rejected") {
        0
    } else {
        existing.target_count.max(0) as usize
    }
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

struct DispatchContext {
    operator: AuthContext,
    command_hash: String,
}

struct GatewayDispatchBatch {
    job_id: Uuid,
    job_command: JobCommand,
    timeout_secs: u64,
    targets: Vec<String>,
    signed_envelopes: HashMap<String, CommandEnvelope>,
    precompleted_statuses: Vec<String>,
    target_count: usize,
    source_schedule_id: Option<Uuid>,
    context: DispatchContext,
}

async fn dispatch_to_gateway(
    state: &AppState,
    batch: GatewayDispatchBatch,
) -> Result<(usize, String), ApiError> {
    record_backup_requests_for_dispatch(state, &batch).await?;
    let mut requests = Vec::new();
    let command_version = crate::job_request::job_command_protocol_version(&batch.job_command);
    debug_assert!(
        command_version
            >= crate::job_request::job_command_min_supported_protocol_version(&batch.job_command)
    );
    for client_id in &batch.targets {
        let request = JobRequest {
            job_id: batch.job_id,
            command_version,
            command: batch.job_command.clone(),
            envelope: batch
                .signed_envelopes
                .get(client_id)
                .cloned()
                .with_context(|| format!("missing signed envelope for {client_id}"))?,
            timeout_secs: batch.timeout_secs.clamp(1, 3600),
        };
        requests.push((client_id.clone(), request));
    }
    state
        .repo
        .mark_job_targets_dispatching(batch.job_id, &batch.targets)
        .await?;
    let dispatches = requests.into_iter().map(|(client_id, request)| {
        let gateway = state.gateway.clone();
        async move {
            let result = gateway
                .dispatch(&client_id, request)
                .await
                .map(target_outcome_from_gateway);
            let outcome = match result {
                Ok(outcome) => outcome,
                Err(error) => TargetDispatchOutcome {
                    status: "dispatch_failed".to_string(),
                    exit_code: None,
                    command_version: None,
                    accepted: false,
                    message: {
                        let message = error.to_string();
                        warn!(
                            client_id,
                            job_id = %batch.job_id,
                            error = %message,
                            "gateway command dispatch failed"
                        );
                        message
                    },
                    outputs: Vec::new(),
                },
            };
            (client_id, outcome)
        }
    });
    let dispatch_results = join_all(dispatches).await;
    let mut accepted_targets = 0_usize;
    let mut target_statuses = batch.precompleted_statuses.clone();
    for (client_id, mut outcome) in dispatch_results {
        if outcome.accepted {
            accepted_targets += 1;
        }
        let stale_reason = protocol_mismatch_reason(&outcome, command_version, &batch.job_command);
        if let Some(reason) = stale_reason.as_deref() {
            outcome.message = stale_target_message(&outcome.message, reason);
        }
        target_statuses.push(outcome.status.clone());
        state
            .repo
            .update_job_target_result(batch.job_id, &client_id, &outcome)
            .await?;
        if let Some(reason) = stale_reason {
            state
                .repo
                .mark_agent_stale(
                    &client_id,
                    &reason,
                    serde_json::json!({
                        "job_id": batch.job_id,
                        "command_type": crate::job_request::job_command_type_label(&batch.job_command),
                        "requested_command_version": command_version,
                        "response_command_version": outcome.command_version,
                        "message": outcome.message,
                    }),
                )
                .await?;
        }
        state
            .repo
            .record_job_outputs_with_config(
                batch.job_id,
                &client_id,
                &outcome.outputs,
                JobOutputPersistConfig {
                    object_store: state.backup_object_store.as_ref(),
                    artifact_min_bytes: state.job_output_artifact_min_bytes,
                },
            )
            .await?;
        if let Some((seq, output)) = outcome.outputs.iter().enumerate().next_back() {
            state.publish(WsEvent::JobOutputRecorded {
                job_id: batch.job_id,
                client_id: client_id.clone(),
                seq: seq as i32,
                done: output.done,
            });
        }
        if matches!(&batch.job_command, JobCommand::Backup { .. }) && outcome.status == "completed"
        {
            if let Err(error) = try_auto_record_backup_artifact(
                state,
                &batch.context.operator,
                &client_id,
                &batch.context.command_hash,
                batch.job_id,
                &outcome.outputs,
            )
            .await
            {
                warn!(%error, job_id = %batch.job_id, client_id, "backup artifact auto-record failed");
            }
        }
    }
    let status = aggregate_job_status(&target_statuses, batch.target_count);
    state.repo.finish_job(batch.job_id, status).await?;
    state.publish(WsEvent::JobFinished {
        job_id: batch.job_id,
        accepted_targets,
        status: status.to_string(),
    });
    Ok((accepted_targets, status.to_string()))
}

fn stale_target_message(message: &str, reason: &str) -> String {
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

async fn record_backup_requests_for_dispatch(
    state: &AppState,
    batch: &GatewayDispatchBatch,
) -> Result<(), ApiError> {
    let JobCommand::Backup {
        paths,
        include_config,
        recipient_public_key_hex,
    } = &batch.job_command
    else {
        return Ok(());
    };
    for client_id in &batch.targets {
        if let Some(request) = state
            .repo
            .find_open_backup_request_for_artifact(client_id, &batch.context.command_hash)
            .await?
        {
            state
                .repo
                .attach_backup_request_source(
                    request.id,
                    Some(batch.job_id),
                    batch.source_schedule_id,
                    &batch.context.operator,
                )
                .await?;
            continue;
        }
        let envelope = batch
            .signed_envelopes
            .get(client_id)
            .with_context(|| format!("missing signed envelope for {client_id}"))?;
        let request = CreateBackupRequest {
            client_id: client_id.clone(),
            paths: paths.clone(),
            include_config: *include_config,
            recipient_public_key_hex: recipient_public_key_hex.clone(),
            confirmed: true,
            note: Some(format!("auto-linked from backup job {}", batch.job_id)),
            privilege_assertion: None,
        };
        state
            .repo
            .record_backup_request_with_source(
                &request,
                &batch.context.command_hash,
                envelope,
                &batch.context.operator,
                BackupRequestStatus::RequestedMetadataOnly,
                BackupRequestSourceLink {
                    job_id: Some(batch.job_id),
                    schedule_id: batch.source_schedule_id,
                },
            )
            .await?;
    }
    Ok(())
}

fn aggregate_job_status(target_statuses: &[String], target_count: usize) -> &'static str {
    let completed = target_statuses
        .iter()
        .filter(|status| status.as_str() == "completed")
        .count();
    if target_count > 0 && completed == target_count {
        return "completed";
    }
    if completed > 0 {
        return "partially_completed";
    }
    if target_statuses
        .iter()
        .any(|status| status.as_str() == "degraded_unprivileged")
    {
        return "degraded_unprivileged";
    }
    if target_statuses
        .iter()
        .any(|status| status.as_str() == "timed_out")
    {
        return "timed_out";
    }
    if target_statuses
        .iter()
        .any(|status| matches!(status.as_str(), "failed" | "rejected_by_agent"))
    {
        return "failed";
    }
    if target_statuses
        .iter()
        .any(|status| status.as_str() == "accepted")
    {
        return "accepted";
    }
    "dispatch_failed"
}

fn split_targets_by_capability(
    command: &JobCommand,
    targets: &[String],
    agents: &[AgentView],
    force_unprivileged: bool,
) -> (Vec<String>, Vec<CapabilitySkip>) {
    if force_unprivileged {
        return (targets.to_vec(), Vec::new());
    }
    let mut dispatch_targets = Vec::new();
    let mut skipped_targets = Vec::new();
    for client_id in targets {
        if let Some(failure) = agents
            .iter()
            .find(|agent| agent.id == *client_id)
            .and_then(|agent| target_capability_failure(command, agent))
        {
            skipped_targets.push(CapabilitySkip {
                client_id: client_id.clone(),
                failure,
            });
        } else {
            dispatch_targets.push(client_id.clone());
        }
    }
    (dispatch_targets, skipped_targets)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CapabilityFailure {
    reason: &'static str,
    hint: &'static str,
    message: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CapabilitySkip {
    client_id: String,
    failure: CapabilityFailure,
}

fn target_capability_failure(command: &JobCommand, agent: &AgentView) -> Option<CapabilityFailure> {
    if target_lacks_root_network_capability(command, agent) {
        return Some(CapabilityFailure {
            reason: "target_agent_lacks_root_runtime_network_capability",
            hint: "agent reported unprivileged mode or no root runtime network capability; root-only network mutation was not dispatched unless force_unprivileged is set",
            message: "target agent lacks root runtime network capability",
        });
    }
    if target_lacks_process_limit_capability(command, agent) {
        return Some(CapabilityFailure {
            reason: "target_agent_lacks_process_limit_capability",
            hint: "agent reported unprivileged mode or no process-limit capability; process start with resource limits was not dispatched unless force_unprivileged is set",
            message: "target agent lacks process limit capability",
        });
    }
    if target_lacks_agent_update_capability(command, agent) {
        return Some(CapabilityFailure {
            reason: "target_agent_lacks_agent_update_capability",
            hint: "agent reported unprivileged mode or no agent-update host-mutation capability; agent update was not dispatched unless force_unprivileged is set",
            message: "target agent lacks agent update capability",
        });
    }
    if target_lacks_restore_capability(command, agent) {
        return Some(CapabilityFailure {
            reason: "target_agent_lacks_restore_capability",
            hint: "agent reported unprivileged mode or no privileged host-mutation capability; restore mutation was not dispatched unless force_unprivileged is set",
            message: "target agent lacks restore capability",
        });
    }
    None
}

fn target_lacks_root_network_capability(command: &JobCommand, agent: &AgentView) -> bool {
    let root_network_operation = matches!(
        command,
        JobCommand::NetworkApply { .. }
            | JobCommand::NetworkRollback { .. }
            | JobCommand::NetworkOspfCostUpdate { .. }
    );
    if !root_network_operation {
        return false;
    }
    match agent.capabilities.privilege_mode {
        AgentPrivilegeMode::Unprivileged => true,
        AgentPrivilegeMode::Root => !agent.capabilities.can_manage_runtime_tunnels,
        AgentPrivilegeMode::Unknown => false,
    }
}

fn target_lacks_process_limit_capability(command: &JobCommand, agent: &AgentView) -> bool {
    let JobCommand::ProcessStart { limits, .. } = command else {
        return false;
    };
    if limits.is_default() {
        return false;
    }
    match agent.capabilities.privilege_mode {
        AgentPrivilegeMode::Unprivileged => true,
        AgentPrivilegeMode::Root => !agent.capabilities.can_apply_process_limits,
        AgentPrivilegeMode::Unknown => false,
    }
}

fn target_lacks_agent_update_capability(command: &JobCommand, agent: &AgentView) -> bool {
    let agent_update_operation = matches!(
        command,
        JobCommand::ConfigRead
            | JobCommand::HotConfig { .. }
            | JobCommand::DataSourceConfigPatch { .. }
            | JobCommand::UpdateAgent { .. }
            | JobCommand::AgentUpdateActivate { .. }
            | JobCommand::AgentUpdateRollback { .. }
            | JobCommand::AgentUpdateCheck { .. }
    );
    if !agent_update_operation {
        return false;
    }
    target_lacks_privileged_host_mutation_capability(agent)
}

fn target_lacks_restore_capability(command: &JobCommand, agent: &AgentView) -> bool {
    let restore_operation = matches!(
        command,
        JobCommand::Restore { .. } | JobCommand::RestoreRollback { .. }
    );
    if !restore_operation {
        return false;
    }
    target_lacks_privileged_host_mutation_capability(agent)
}

fn target_lacks_privileged_host_mutation_capability(agent: &AgentView) -> bool {
    match agent.capabilities.privilege_mode {
        AgentPrivilegeMode::Unprivileged => true,
        AgentPrivilegeMode::Root => !agent.capabilities.can_attempt_privileged_ops,
        AgentPrivilegeMode::Unknown => false,
    }
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
    request: &CreateJobRequest,
    command_hash: &str,
    operator: &AuthContext,
    reason: &'static str,
) -> Result<(StatusCode, Json<CreateJobResponse>), ApiError> {
    let job_id = state
        .repo
        .record_rejected_job(request, command_hash, operator)
        .await?;
    let status = "rejected_authorization_required".to_string();
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
            accepted_targets: 0,
            status,
        }),
    ))
}

fn validate_selector_expression(selector_expression: &str) -> Result<(), ApiError> {
    if selector_expression.trim().is_empty() {
        return Err(ApiError::bad_request("selector_expression_required"));
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

fn target_outcome_from_gateway(result: GatewayCommandDispatchResult) -> TargetDispatchOutcome {
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

fn protocol_mismatch_reason(
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

fn output_indicates_timeout(output: &CommandOutput) -> bool {
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
