use std::collections::HashMap;

use anyhow::{anyhow, Context};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
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
        AgentView, AuthContext, BackupRequestStatus, BulkResolveRequest, CreateBackupRequest,
        CreateJobRequest, CreateJobResponse, DispatchScheduledJobRequest, JobHistoryView, WsEvent,
    },
    repository_backups::BackupRequestSourceLink,
    repository_job_outputs::JobOutputPersistConfig,
    state::AppState,
};

#[derive(Debug, Deserialize)]
pub(crate) struct CancelJobRequest {
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CancelJobResponse {
    pub(crate) job_id: Uuid,
    pub(crate) canceled: bool,
    pub(crate) status: String,
    pub(crate) canceled_targets: i64,
    pub(crate) cancel_requested_targets: i64,
}

pub(crate) async fn cancel_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
    Json(request): Json<CancelJobRequest>,
) -> Result<Json<CancelJobResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    validate_cancel_job_request(&request)?;
    let cancellation = state
        .repo
        .cancel_pending_job(job_id, &operator, request.reason.as_deref())
        .await?
        .ok_or_else(|| ApiError::not_found("job_not_found"))?;
    if cancellation.canceled || cancellation.status == "canceled" {
        return Ok(Json(CancelJobResponse {
            job_id: cancellation.job_id,
            canceled: cancellation.canceled,
            status: cancellation.status,
            canceled_targets: cancellation.canceled_targets,
            cancel_requested_targets: 0,
        }));
    }
    if matches!(
        cancellation.status.as_str(),
        "dispatching" | "cancel_requested"
    ) {
        if !state.gateway.configured() {
            return Err(ApiError::conflict("gateway_control_url_missing"));
        }
        let active = state
            .repo
            .request_active_job_cancel(job_id, &operator, request.reason.as_deref())
            .await?
            .ok_or_else(|| ApiError::not_found("job_not_found"))?;
        if !active.requested {
            return Err(ApiError::conflict("job_not_cancelable"));
        }
        let cancel_results = join_all(active.target_clients.iter().map(|client_id| {
            let gateway = state.gateway.clone();
            let reason = request.reason.clone();
            async move {
                gateway
                    .cancel(client_id, job_id, reason.as_deref())
                    .await
                    .ok()
                    .is_some_and(|result| result.canceled)
            }
        }))
        .await;
        let cancel_requested_targets = cancel_results
            .iter()
            .filter(|requested| **requested)
            .count() as i64;
        return Ok(Json(CancelJobResponse {
            job_id: active.job_id,
            canceled: cancel_requested_targets > 0,
            status: active.status,
            canceled_targets: 0,
            cancel_requested_targets,
        }));
    }
    Err(ApiError::conflict("job_not_cancelable"))
}

fn validate_cancel_job_request(request: &CancelJobRequest) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::conflict("job_cancel_confirmation_required"));
    }
    if let Some(reason) = request.reason.as_deref() {
        let reason = reason.trim();
        if reason.is_empty() || reason.len() > 240 || reason.chars().any(char::is_control) {
            return Err(ApiError::bad_request("job_cancel_reason_invalid"));
        }
    }
    Ok(())
}

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
    validate_job_reconnect_metadata(&request)?;
    if request.destructive && !request.confirmed {
        return Err(ApiError::conflict("destructive_confirmation_required"));
    }
    let job_command = request.job_command()?;
    validate_rollout_options(&request, &job_command)?;
    if !request.confirmed {
        match &job_command {
            JobCommand::Backup { .. } => {
                return Err(ApiError::conflict("backup_confirmation_required"));
            }
            JobCommand::HotConfig { .. }
            | JobCommand::DataSourceConfigPatch { .. }
            | JobCommand::AuthProofKeyRotate { .. } => {
                return Err(ApiError::conflict("config_update_confirmation_required"));
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
            "all non-telemetry jobs require privileged proof",
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
    validate_network_apply_target(&job_command, &resolved_targets)?;
    let rollout_policy = state
        .repo
        .resolve_agent_update_rollout_policy(&request, &job_command, &resolved_agents)
        .await?;
    let signed_envelopes =
        match request.signed_envelopes_for_targets(&resolved_targets, &command_hash, signing_key) {
            Ok(envelopes) => envelopes,
            Err(error) => {
                warn!(%error, command_hash, "job rejected because command envelope is invalid");
                return reject_job(
                    state,
                    &request,
                    &command_hash,
                    operator,
                    "invalid command envelope",
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
    let (dispatch_targets, staged_targets) =
        split_targets_by_canary(dispatch_targets, request.canary_count);
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

    let job_id = state
        .repo
        .record_dispatching_job_with_rollout_policy(
            &request,
            &command_hash,
            operator,
            &resolved_targets,
            &rollout_policy,
        )
        .await?;
    let precompleted_statuses =
        precomplete_capability_skips(state, job_id, &job_command, capability_skips).await?;
    let precompleted_statuses = precomplete_staged_targets(
        state,
        job_id,
        &job_command,
        staged_targets,
        precompleted_statuses,
    )
    .await?;
    let (accepted_targets, status) = dispatch_to_gateway(
        state,
        GatewayDispatchBatch {
            job_id,
            job_command: &job_command,
            timeout_secs: request.timeout_secs.unwrap_or(30),
            targets: &dispatch_targets,
            signed_envelopes: &signed_envelopes,
            precompleted_statuses,
            target_count: resolved_targets.len(),
            source_schedule_id: None,
            context: DispatchContext {
                operator,
                command_hash: &command_hash,
            },
        },
    )
    .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(CreateJobResponse {
            job_id,
            accepted_targets,
            status,
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

fn validate_rollout_options(
    request: &CreateJobRequest,
    _job_command: &JobCommand,
) -> Result<(), ApiError> {
    let Some(canary_count) = request.canary_count else {
        return Ok(());
    };
    if !(0..=10_000).contains(&canary_count) {
        return Err(ApiError::bad_request("canary_count_out_of_range"));
    }
    Ok(())
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

pub(crate) async fn dispatch_scheduled_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
    Json(request): Json<DispatchScheduledJobRequest>,
) -> Result<(StatusCode, Json<CreateJobResponse>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    if !request.confirmed {
        return Err(ApiError::conflict(
            "scheduled_dispatch_confirmation_required",
        ));
    }
    let scheduled = state
        .repo
        .load_scheduled_job_for_dispatch(job_id)
        .await?
        .ok_or_else(|| ApiError::not_found("scheduled_job_approval_not_found"))?;
    if scheduled.targets.is_empty() {
        return Err(ApiError::conflict("scheduled_job_has_no_targets"));
    }
    let command_payload = encode_json(&scheduled.operation).map_err(|error| {
        ApiError::from(anyhow!(
            "failed to encode scheduled job command for authorization: {error}"
        ))
    })?;
    let command_hash = payload_hash(&command_payload);
    if command_hash != scheduled.payload_hash {
        return Err(ApiError::conflict("scheduled_job_payload_hash_mismatch"));
    }
    let Some(signing_key) = state.server_signing_key.as_deref() else {
        return Err(ApiError::conflict("server_signing_key_missing"));
    };
    validate_network_apply_target(&scheduled.operation, &scheduled.targets)?;
    let signed_envelopes = request
        .signed_envelopes_for_targets(&scheduled.targets, &command_hash, signing_key)
        .map_err(|error| {
            warn!(%error, command_hash, job_id = %scheduled.job_id, "scheduled job proof envelope is invalid");
            ApiError::forbidden("invalid_command_envelope")
        })?;
    let scheduled_agents = state
        .repo
        .resolve_bulk_targets(&BulkResolveRequest {
            clients: scheduled.targets.clone(),
            pools: Vec::new(),
            tags: Vec::new(),
            tag_mode: None,
            destructive: false,
            confirmed: true,
        })
        .await?
        .targets;
    let (dispatch_targets, capability_skips) = split_targets_by_capability(
        &scheduled.operation,
        &scheduled.targets,
        &scheduled_agents,
        request.force_unprivileged,
    );
    if !dispatch_targets.is_empty() && !state.gateway.configured() {
        return Err(ApiError::conflict("gateway_control_url_missing"));
    }
    state
        .repo
        .mark_scheduled_job_dispatching(&scheduled, &operator, signed_envelopes.len())
        .await?;
    let precompleted_statuses = precomplete_capability_skips(
        &state,
        scheduled.job_id,
        &scheduled.operation,
        capability_skips,
    )
    .await?;
    let (accepted_targets, status) = dispatch_to_gateway(
        &state,
        GatewayDispatchBatch {
            job_id: scheduled.job_id,
            job_command: &scheduled.operation,
            timeout_secs: request.timeout_secs.unwrap_or(30),
            targets: &dispatch_targets,
            signed_envelopes: &signed_envelopes,
            precompleted_statuses,
            target_count: scheduled.targets.len(),
            source_schedule_id: scheduled.source_schedule_id,
            context: DispatchContext {
                operator: &operator,
                command_hash: &command_hash,
            },
        },
    )
    .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(CreateJobResponse {
            job_id: scheduled.job_id,
            accepted_targets,
            status,
        }),
    ))
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

async fn precomplete_staged_targets(
    state: &AppState,
    job_id: Uuid,
    job_command: &JobCommand,
    staged_targets: Vec<String>,
    mut precompleted_statuses: Vec<String>,
) -> Result<Vec<String>, ApiError> {
    for client_id in staged_targets {
        let outcome = staged_pending_outcome(job_id, &client_id, job_command)?;
        precompleted_statuses.push(outcome.status.clone());
        state
            .repo
            .update_job_target_result(job_id, &client_id, &outcome)
            .await?;
        state
            .repo
            .record_job_outputs_with_config(
                job_id,
                &client_id,
                &outcome.outputs,
                JobOutputPersistConfig {
                    object_store: state.backup_object_store.as_ref(),
                    artifact_min_bytes: state.job_output_artifact_min_bytes,
                },
            )
            .await?;
        state.publish(WsEvent::JobOutputRecorded {
            job_id,
            client_id,
            seq: 0,
            done: true,
        });
    }
    Ok(precompleted_statuses)
}

struct DispatchContext<'a> {
    operator: &'a AuthContext,
    command_hash: &'a str,
}

struct GatewayDispatchBatch<'a> {
    job_id: Uuid,
    job_command: &'a JobCommand,
    timeout_secs: u64,
    targets: &'a [String],
    signed_envelopes: &'a HashMap<String, CommandEnvelope>,
    precompleted_statuses: Vec<String>,
    target_count: usize,
    source_schedule_id: Option<Uuid>,
    context: DispatchContext<'a>,
}

async fn dispatch_to_gateway(
    state: &AppState,
    batch: GatewayDispatchBatch<'_>,
) -> Result<(usize, String), ApiError> {
    record_backup_requests_for_dispatch(state, &batch).await?;
    let mut requests = Vec::new();
    for client_id in batch.targets {
        let request = JobRequest {
            job_id: batch.job_id,
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
    let mut target_statuses = batch.precompleted_statuses;
    for (client_id, outcome) in dispatch_results {
        if outcome.accepted {
            accepted_targets += 1;
        }
        target_statuses.push(outcome.status.clone());
        state
            .repo
            .update_job_target_result(batch.job_id, &client_id, &outcome)
            .await?;
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
        if matches!(batch.job_command, JobCommand::Backup { .. }) && outcome.status == "completed" {
            if let Err(error) = try_auto_record_backup_artifact(
                state,
                batch.context.operator,
                &client_id,
                batch.context.command_hash,
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

async fn record_backup_requests_for_dispatch(
    state: &AppState,
    batch: &GatewayDispatchBatch<'_>,
) -> Result<(), ApiError> {
    let JobCommand::Backup {
        paths,
        include_config,
        recipient_public_key_hex,
    } = batch.job_command
    else {
        return Ok(());
    };
    for client_id in batch.targets {
        if let Some(request) = state
            .repo
            .find_open_backup_request_for_artifact(client_id, batch.context.command_hash)
            .await?
        {
            state
                .repo
                .attach_backup_request_source(
                    request.id,
                    Some(batch.job_id),
                    batch.source_schedule_id,
                    batch.context.operator,
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
            envelope: Some(envelope.clone()),
        };
        state
            .repo
            .record_backup_request_with_source(
                &request,
                batch.context.command_hash,
                envelope,
                batch.context.operator,
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
        .any(|status| status.as_str() == "staged_pending")
    {
        return "staged_pending";
    }
    if target_count > 0
        && target_statuses
            .iter()
            .filter(|status| status.as_str() == "canceled")
            .count()
            == target_count
    {
        return "canceled";
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

fn split_targets_by_canary(
    targets: Vec<String>,
    canary_count: Option<i32>,
) -> (Vec<String>, Vec<String>) {
    let Some(canary_count) = canary_count else {
        return (targets, Vec::new());
    };
    if canary_count <= 0 || canary_count as usize >= targets.len() {
        return (targets, Vec::new());
    }
    let canary_count = canary_count as usize;
    let mut dispatch_targets = targets;
    let staged_targets = dispatch_targets.split_off(canary_count);
    (dispatch_targets, staged_targets)
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
        JobCommand::UpdateAgent { .. }
            | JobCommand::AgentUpdateActivate { .. }
            | JobCommand::AgentUpdateRollback { .. }
            | JobCommand::AuthProofKeyRotate { .. }
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

fn staged_pending_outcome(
    job_id: Uuid,
    client_id: &str,
    command: &JobCommand,
) -> Result<TargetDispatchOutcome, ApiError> {
    let status = serde_json::json!({
        "type": "bulk_stage_pending",
        "status": "staged_pending",
        "client_id": client_id,
        "command_type": crate::job_request::job_command_type_label(command),
        "hint": "target is held for a later rollout stage after the canary batch is reviewed",
    });
    Ok(TargetDispatchOutcome {
        status: "staged_pending".to_string(),
        exit_code: None,
        accepted: false,
        message: "target staged for later rollout".to_string(),
        outputs: vec![CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&status).map_err(|error| ApiError::from(anyhow!(error)))?,
            exit_code: Some(0),
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
        targets = request.targets.len(),
        clients = request.clients.len(),
        pools = request.pools.len(),
        tags = request.tags.len(),
        privileged = request.privileged,
        has_legacy_envelope = request.envelope.is_some(),
        envelope_count = request.envelopes.len(),
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

#[derive(Debug)]
pub(crate) struct TargetDispatchOutcome {
    pub(crate) status: String,
    pub(crate) exit_code: Option<i32>,
    pub(crate) accepted: bool,
    pub(crate) message: String,
    pub(crate) outputs: Vec<CommandOutput>,
}

fn target_outcome_from_gateway(result: GatewayCommandDispatchResult) -> TargetDispatchOutcome {
    if !result.accepted {
        return TargetDispatchOutcome {
            status: "rejected_by_agent".to_string(),
            exit_code: None,
            accepted: false,
            message: result.message,
            outputs: result.outputs,
        };
    }
    let final_output = result.outputs.iter().rev().find(|output| output.done);
    let exit_code = final_output.and_then(|output| output.exit_code);
    let status = if final_output.is_some_and(output_indicates_timeout) {
        "timed_out"
    } else if final_output.is_some_and(output_indicates_canceled) {
        "canceled"
    } else {
        match exit_code {
            Some(0) => "completed",
            Some(_) => "failed",
            None => "accepted",
        }
    };
    TargetDispatchOutcome {
        status: status.to_string(),
        exit_code,
        accepted: true,
        message: result.message,
        outputs: result.outputs,
    }
}

fn output_indicates_canceled(output: &CommandOutput) -> bool {
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
        .is_some_and(|kind| kind == "command_canceled")
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
