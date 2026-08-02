use std::collections::HashSet;

use anyhow::anyhow;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use tracing::warn;
use uuid::Uuid;
use vpsman_common::{
    encode_json, job_command_requires_confirmation, payload_hash, CommandOutput,
    GatewayCommandDispatchResult, JobCancelRequest as GatewayJobCancelRequest, JobCommand,
    OutputStream, RuntimeTunnelManager, TunnelEndpointSide, DEFAULT_MAX_JOB_TIMEOUT_SECS,
};
use vpsman_server_core::{
    CapabilitySkip, TargetCapability, JOB_STATUS_FAILED, JOB_STATUS_REJECTED, JOB_STATUS_RUNNING,
    JOB_STATUS_SKIPPED, TARGET_STATUS_AGENT_LOST, TARGET_STATUS_AGENT_TIMEOUT,
    TARGET_STATUS_CANCELED, TARGET_STATUS_COMPLETED, TARGET_STATUS_CONTROL_TIMEOUT,
    TARGET_STATUS_DISPATCHING, TARGET_STATUS_FAILED, TARGET_STATUS_QUEUED, TARGET_STATUS_REJECTED,
    TARGET_STATUS_RUNNING, TARGET_STATUS_SKIPPED,
};

use crate::{
    error::{public_recovery, ApiError},
    internal_operator::server_issued_job_actor,
    model::{
        AgentView, AuthContext, CancelJobRequest, CancelJobResponse, CancelJobTargetResult,
        CreateJobApprovalRequest, CreateJobRequest, CreateJobResponse, CreateJobTargetCounts,
        DecideJobApprovalRequest, JobApprovalDecisionResponse, JobApprovalView, ListQuery, WsEvent,
    },
    privilege::{verify_privilege_intent, JobPrivilegeIntent, JobPrivilegeIntentInput},
    repository_jobs::PrecompletedJobTarget,
    routes_ingest::{record_network_routing_terminal_result, status_output_message},
    security::SCOPE_JOBS_READ,
    state::AppState,
    unix_now,
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
    if !request.confirmed {
        return Err(ApiError::conflict("job_cancel_requires_confirmation"));
    }
    if state.repo.get_job(job_id).await?.is_none() {
        return Err(ApiError::not_found("job_not_found"));
    }
    let reason = bounded_cancel_reason(request.reason.as_deref());
    let plan = state
        .repo
        .request_job_cancel(job_id, &operator, reason.as_deref())
        .await?;
    if plan.pending_canceled > 0 {
        if let Err(error) =
            record_network_routing_terminal_result(&state, job_id, "", TARGET_STATUS_CANCELED, None)
                .await
        {
            warn!(?error, %job_id, "network routing state update failed after queued cancellation");
        }
    }
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
    state
        .process_job_terminal_events_or_publish_refresh(500, job_id, refreshed.clone())
        .await?;
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

pub(crate) async fn list_job_approvals(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<JobApprovalView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_JOBS_READ)
        .await?;
    Ok(Json(state.repo.query_job_approvals(&query).await?))
}

pub(crate) async fn create_job_approval(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateJobApprovalRequest>,
) -> Result<(StatusCode, Json<JobApprovalView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    let (approval, request) = prepare_job_approval(&state, &operator, request).await?;
    let approval = state
        .repo
        .record_job_approval(approval, &request, &operator)
        .await
        .map_err(map_job_approval_repo_error)?;
    Ok((StatusCode::CREATED, Json(approval)))
}

pub(crate) async fn approve_job_approval(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(approval_id): Path<Uuid>,
    Json(request): Json<DecideJobApprovalRequest>,
) -> Result<Json<JobApprovalDecisionResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    if !request.confirmed {
        return Err(ApiError::conflict(
            "job_approval_decision_requires_confirmation",
        ));
    }
    let (approval, frozen_request) = state
        .repo
        .get_job_approval_request(approval_id)
        .await?
        .ok_or_else(|| ApiError::not_found("job_approval_not_found"))?;
    if approval.status == "rejected" {
        return Err(ApiError::conflict("job_approval_not_pending"));
    }
    let decision_reason = bounded_review_reason(request.reason.as_deref());
    let approval = state
        .repo
        .decide_job_approval(
            approval_id,
            "approved",
            &operator,
            decision_reason.as_deref(),
        )
        .await
        .map_err(map_job_approval_repo_error)?;
    let (_, Json(job)) = create_job_inner(
        &state,
        &operator,
        frozen_request,
        JobPrivilegeSource::ApprovedRequest(approval_id),
    )
    .await?;
    Ok(Json(JobApprovalDecisionResponse {
        approval,
        job: Some(job),
    }))
}

pub(crate) async fn reject_job_approval(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(approval_id): Path<Uuid>,
    Json(request): Json<DecideJobApprovalRequest>,
) -> Result<Json<JobApprovalDecisionResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    if !request.confirmed {
        return Err(ApiError::conflict(
            "job_approval_decision_requires_confirmation",
        ));
    }
    let reason = bounded_review_reason(request.reason.as_deref())
        .ok_or_else(|| ApiError::bad_request("job_approval_rejection_reason_required"))?;
    let approval = state
        .repo
        .decide_job_approval(approval_id, "rejected", &operator, Some(reason.as_str()))
        .await
        .map_err(map_job_approval_repo_error)?;
    Ok(Json(JobApprovalDecisionResponse {
        approval,
        job: None,
    }))
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

pub(crate) async fn create_job_from_internal_operator_mutation(
    state: &AppState,
    operator: &AuthContext,
    request: CreateJobRequest,
) -> Result<(StatusCode, Json<CreateJobResponse>), ApiError> {
    create_job_inner(
        state,
        operator,
        request,
        JobPrivilegeSource::InternalOperatorMutation,
    )
    .await
}

enum JobPrivilegeSource {
    RequestAssertion,
    ApprovedRequest(Uuid),
    SavedSchedule(Uuid),
    InternalOperatorMutation,
}

async fn prepare_job_approval(
    state: &AppState,
    operator: &AuthContext,
    request: CreateJobApprovalRequest,
) -> Result<(JobApprovalView, CreateJobRequest), ApiError> {
    let mut job = request.job;
    validate_job_audit_selector(&job.selector_expression)?;
    if job.job_id.is_none() {
        job.job_id = Some(Uuid::new_v4());
    }
    let job_id = job
        .job_id
        .ok_or_else(|| ApiError::conflict("job_id_required"))?;
    if job_id == Uuid::nil() {
        return Err(ApiError::bad_request("job_id_invalid"));
    }
    if !job.confirmed {
        return Err(ApiError::conflict("job_approval_requires_confirmed_job"));
    }
    if !job.privileged {
        return Err(ApiError::forbidden("job_approval_requires_privileged_job"));
    }
    if state.repo.get_job(job_id).await?.is_some() {
        return Err(ApiError::conflict("job_approval_job_id_already_exists"));
    }
    bind_declared_network_plan(state, &mut job).await?;
    let job_command = job.job_command()?;
    validate_job_command_source(&job_command, &JobPrivilegeSource::RequestAssertion)?;
    let command_payload = encode_json(&job_command).map_err(|error| {
        ApiError::from(anyhow!(
            "failed to encode job command for approval authorization: {error}"
        ))
    })?;
    let command_hash = payload_hash(&command_payload);
    let fixed_target_ids = job.fixed_target_ids()?;
    let target_selection = job.target_selection()?;
    let resolved_agents = state
        .repo
        .resolve_bulk_targets(&target_selection)
        .await?
        .targets;
    let missing_fixed_targets = fixed_target_ids
        .iter()
        .filter(|client_id| {
            !resolved_agents
                .iter()
                .any(|agent| agent.id == client_id.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    if !missing_fixed_targets.is_empty() {
        return Err(ApiError::conflict("fixed_target_not_found"));
    }
    if matches!(job_command, JobCommand::ConfigRead) && fixed_target_ids.len() != 1 {
        return Err(ApiError::conflict("config_read_requires_single_target"));
    }
    vpsman_server_core::validate_network_command_targets(&job_command, &fixed_target_ids)
        .map_err(|error| ApiError::bad_request(error.code()))?;
    validate_job_rollout(job.rollout.as_ref(), &job_command, &fixed_target_ids)?;
    validate_restore_archive_binding(state, &job_command, &fixed_target_ids).await?;
    let effective_max_timeout_secs =
        effective_job_max_timeout_secs(job.max_timeout_secs, state.max_job_timeout_secs())?;
    job.max_timeout_secs = Some(effective_max_timeout_secs);
    let request_fingerprint =
        request_fingerprint_for_job(&job, &command_hash, &fixed_target_ids, None)?;
    let rollout_policy_hash = job_rollout_policy_hash(job.rollout.as_ref())?;
    let privilege_intent = JobPrivilegeIntent::new(JobPrivilegeIntentInput {
        selector_expression: &job.selector_expression,
        command_type: job.command_type_label(),
        operation_payload_hash: &command_hash,
        rollout_policy_hash: rollout_policy_hash.as_deref(),
        resolved_targets: &fixed_target_ids,
        max_timeout_secs: job.max_timeout_secs.unwrap_or(DEFAULT_MAX_JOB_TIMEOUT_SECS),
        force_unprivileged: job.force_unprivileged,
        privileged: job.privileged,
    });
    verify_privilege_intent(state, &privilege_intent, job.privilege_assertion.clone()).await?;
    job.privilege_assertion = None;
    job.target_client_ids = fixed_target_ids.clone();
    let risk = normalized_job_approval_risk(request.risk.as_deref(), &job)?;
    let reason = bounded_review_reason(request.reason.as_deref());
    let requested_at = unix_now().to_string();
    Ok((
        JobApprovalView {
            id: request.approval_id.unwrap_or_else(Uuid::new_v4),
            status: "pending".to_string(),
            job_id,
            command_type: job.command_type_label().to_string(),
            selector_expression: job.selector_expression.trim().to_string(),
            target_client_ids: fixed_target_ids,
            target_count: job.target_client_ids.len(),
            privileged: job.privileged,
            destructive: job.destructive,
            force_unprivileged: job.force_unprivileged,
            max_timeout_secs: effective_max_timeout_secs,
            payload_hash: command_hash,
            request_fingerprint,
            requester_id: Some(operator.operator.id),
            requester_username: operator.operator.username.clone(),
            requester_role: operator.operator.role.clone(),
            requested_at,
            request_reason: reason,
            risk,
            decision_by: None,
            decision_username: None,
            decision_reason: None,
            decided_at: None,
        },
        job,
    ))
}

#[derive(Clone, Debug)]
struct FixedTargetUnavailableSkip {
    client_id: String,
}

async fn create_job_inner(
    state: &AppState,
    operator: &AuthContext,
    mut request: CreateJobRequest,
    privilege_source: JobPrivilegeSource,
) -> Result<(StatusCode, Json<CreateJobResponse>), ApiError> {
    validate_job_audit_selector(&request.selector_expression)?;
    let job_id = request
        .job_id
        .ok_or_else(|| ApiError::conflict("job_id_required"))?;
    if job_id == Uuid::nil() {
        return Err(ApiError::bad_request("job_id_invalid"));
    }
    if request.destructive && !request.confirmed {
        return Err(ApiError::conflict("destructive_confirmation_required"));
    }
    bind_declared_network_plan(state, &mut request).await?;
    let job_command = request.job_command()?;
    validate_job_command_source(&job_command, &privilege_source)?;
    if !request.confirmed && job_command_requires_dispatch_confirmation(&job_command) {
        return Err(ApiError::conflict(confirmation_error_code(&job_command)));
    }
    let command_payload = encode_json(&job_command).map_err(|error| {
        ApiError::from(anyhow!(
            "failed to encode job command for authorization: {error}"
        ))
    })?;
    let command_hash = payload_hash(&command_payload);
    let fixed_target_ids = request.fixed_target_ids()?;
    let target_selection = request.target_selection()?;
    let resolved_agents = state
        .repo
        .resolve_bulk_targets(&target_selection)
        .await?
        .targets;
    let resolved_targets = fixed_target_ids;
    validate_job_rollout(request.rollout.as_ref(), &job_command, &resolved_targets)?;
    let missing_fixed_targets = resolved_targets
        .iter()
        .filter(|client_id| {
            !resolved_agents
                .iter()
                .any(|agent| agent.id == client_id.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    let allow_unavailable_fixed_targets =
        matches!(&privilege_source, JobPrivilegeSource::SavedSchedule(_));
    let unavailable_visible_targets = resolved_targets
        .iter()
        .filter(|client_id| {
            resolved_agents.iter().any(|agent| {
                agent.id == client_id.as_str()
                    && matches!(agent.status.as_str(), "revoked" | "deleted")
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    if !allow_unavailable_fixed_targets {
        if !missing_fixed_targets.is_empty() {
            return Err(ApiError::conflict("fixed_target_not_found"));
        }
        if !unavailable_visible_targets.is_empty() {
            return Err(ApiError::conflict("fixed_target_unavailable"));
        }
    }
    let fixed_target_unavailable_skips = if allow_unavailable_fixed_targets {
        missing_fixed_targets
            .into_iter()
            .chain(unavailable_visible_targets)
            .map(|client_id| FixedTargetUnavailableSkip { client_id })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let fixed_target_unavailable_skip_set = fixed_target_unavailable_skips
        .iter()
        .map(|skip| skip.client_id.clone())
        .collect::<HashSet<_>>();
    let never_connected_skips = never_connected_target_skips(&resolved_targets, &resolved_agents);
    let never_connected_skip_set = never_connected_skips
        .iter()
        .map(|skip| skip.client_id.clone())
        .collect::<HashSet<_>>();
    let claimable_targets = resolved_targets
        .iter()
        .filter(|client_id| {
            !never_connected_skip_set.contains(client_id.as_str())
                && !fixed_target_unavailable_skip_set.contains(client_id.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    if matches!(job_command, JobCommand::ConfigRead) && resolved_targets.len() != 1 {
        return Err(ApiError::conflict("config_read_requires_single_target"));
    }
    if matches!(job_command, JobCommand::TerminalOpen { .. }) && resolved_targets.len() != 1 {
        return Err(ApiError::conflict("terminal_open_requires_single_target"));
    }
    vpsman_server_core::validate_network_command_targets(&job_command, &resolved_targets)
        .map_err(|error| ApiError::bad_request(error.code()))?;
    validate_restore_archive_binding(state, &job_command, &resolved_targets).await?;
    let source_schedule_id = match &privilege_source {
        JobPrivilegeSource::RequestAssertion => None,
        JobPrivilegeSource::ApprovedRequest(_) => None,
        JobPrivilegeSource::SavedSchedule(schedule_id) => Some(*schedule_id),
        JobPrivilegeSource::InternalOperatorMutation => None,
    };
    let approval_id = match &privilege_source {
        JobPrivilegeSource::ApprovedRequest(approval_id) => Some(*approval_id),
        _ => None,
    };
    let effective_max_timeout_secs =
        effective_job_max_timeout_secs(request.max_timeout_secs, state.max_job_timeout_secs())?;
    request.max_timeout_secs = Some(effective_max_timeout_secs);
    let request_fingerprint = request_fingerprint_for_job(
        &request,
        &command_hash,
        &resolved_targets,
        source_schedule_id,
    )?;
    if let JobPrivilegeSource::ApprovedRequest(approval_id) = &privilege_source {
        validate_approved_job_binding(
            state,
            *approval_id,
            job_id,
            &command_hash,
            &request_fingerprint,
        )
        .await?;
    }
    if let Some(response) = existing_job_response_for_id(
        state,
        operator,
        job_id,
        &request_fingerprint,
        matches!(&privilege_source, JobPrivilegeSource::ApprovedRequest(_)),
    )
    .await?
    {
        return Ok((StatusCode::OK, Json(response)));
    }

    if !request.privileged && !job_allows_unprivileged_submission(&job_command) {
        return reject_job(
            state,
            job_id,
            &request,
            &command_hash,
            &request_fingerprint,
            operator,
            JOB_STATUS_REJECTED,
            "job_privilege_unlock_required",
            "job requires privilege unlock",
            StatusCode::FORBIDDEN,
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
            JOB_STATUS_SKIPPED,
            "job_no_resolved_targets",
            "job has no resolved targets",
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }
    if request.privileged && matches!(privilege_source, JobPrivilegeSource::RequestAssertion) {
        let rollout_policy_hash = job_rollout_policy_hash(request.rollout.as_ref())?;
        let privilege_intent = JobPrivilegeIntent::new(JobPrivilegeIntentInput {
            selector_expression: &request.selector_expression,
            command_type: request.command_type_label(),
            operation_payload_hash: &command_hash,
            rollout_policy_hash: rollout_policy_hash.as_deref(),
            resolved_targets: &resolved_targets,
            max_timeout_secs: request
                .max_timeout_secs
                .unwrap_or(DEFAULT_MAX_JOB_TIMEOUT_SECS),
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
    let target_capabilities = target_capabilities_from_agents(&resolved_agents);
    let (dispatch_targets, capability_skips) = vpsman_server_core::split_targets_by_capability(
        &job_command,
        &claimable_targets,
        &target_capabilities,
        request.force_unprivileged,
    );
    let busy_update_skips =
        busy_update_skip_targets(state, job_id, &job_command, &dispatch_targets).await?;
    let busy_update_skip_set = busy_update_skips
        .iter()
        .map(|skip| skip.client_id.as_str())
        .collect::<HashSet<_>>();
    let mut dispatch_targets_after_precomplete = dispatch_targets
        .iter()
        .filter(|client_id| !busy_update_skip_set.contains(client_id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let network_speed_peer_skips =
        network_speed_test_peer_skips(&job_command, &dispatch_targets_after_precomplete);
    let network_speed_peer_skip_set = network_speed_peer_skips
        .iter()
        .map(|skip| skip.client_id.as_str())
        .collect::<HashSet<_>>();
    dispatch_targets_after_precomplete
        .retain(|client_id| !network_speed_peer_skip_set.contains(client_id.as_str()));
    if !agent_update_release_policy_allows(
        state,
        &job_command,
        &dispatch_targets_after_precomplete,
        &target_capabilities,
    )
    .await?
    {
        return reject_job(
            state,
            job_id,
            &request,
            &command_hash,
            &request_fingerprint,
            operator,
            "failed",
            "agent_update_release_missing",
            "registered agent update release missing",
            StatusCode::CONFLICT,
        )
        .await;
    }
    let precompleted_targets = precompleted_target_outcomes(
        job_id,
        &job_command,
        never_connected_skips,
        fixed_target_unavailable_skips,
        capability_skips,
        busy_update_skips,
        network_speed_peer_skips,
    )?;
    if !dispatch_targets_after_precomplete.is_empty() && !state.gateway.configured() {
        return reject_job(
            state,
            job_id,
            &request,
            &command_hash,
            &request_fingerprint,
            operator,
            JOB_STATUS_FAILED,
            "gateway_control_url_missing",
            "gateway control URL missing",
            StatusCode::SERVICE_UNAVAILABLE,
        )
        .await;
    }

    if let Some(schedule_id) = source_schedule_id {
        state
            .repo
            .record_dispatching_job_from_schedule_with_precompleted(
                job_id,
                &request,
                &command_hash,
                &request_fingerprint,
                operator,
                &resolved_targets,
                schedule_id,
                &precompleted_targets,
            )
            .await
            .map_err(map_job_recording_error)?
    } else {
        state
            .repo
            .record_dispatching_job_with_precompleted(
                job_id,
                &request,
                &command_hash,
                &request_fingerprint,
                operator,
                &resolved_targets,
                &precompleted_targets,
                approval_id,
            )
            .await
            .map_err(map_job_recording_error)?
    };
    for precompleted in &precompleted_targets {
        state.publish(WsEvent::JobOutputRecorded {
            job_id,
            client_id: precompleted.client_id.clone(),
            seq: 0,
            done: true,
        });
    }
    let refreshed = state.repo.refresh_job_status_from_targets(job_id).await?;
    let status = state
        .terminal_job_status_after_refresh(job_id, refreshed)
        .await?
        .unwrap_or_else(|| JOB_STATUS_RUNNING.to_string());
    if let Err(error) = state.process_job_terminal_events(500).await {
        warn!(
            ?error,
            %job_id,
            "job was accepted, but terminal event reconciliation was deferred to the durable dispatcher"
        );
    }
    crate::job_dispatcher::wake_job_dispatcher(state.clone());
    let target_counts = create_job_target_counts(state, job_id).await?;
    let control_deadline_extra_secs = state
        .dispatcher_runtime_config()
        .control_deadline_extra_secs();
    Ok((
        StatusCode::ACCEPTED,
        Json(CreateJobResponse {
            job_id,
            target_count: resolved_targets.len(),
            status,
            max_timeout_secs: request
                .max_timeout_secs
                .unwrap_or(DEFAULT_MAX_JOB_TIMEOUT_SECS),
            max_job_timeout_secs: state.max_job_timeout_secs(),
            control_deadline_extra_secs,
            target_counts,
            error: None,
            message: None,
            recovery: None,
        }),
    ))
}

pub(crate) async fn bind_declared_network_plan(
    state: &AppState,
    request: &mut CreateJobRequest,
) -> Result<(), ApiError> {
    let Some((plan_id, supplied_plan, status_only)) =
        request
            .operation
            .as_ref()
            .and_then(|operation| match operation {
                JobCommand::NetworkStatus { plan_id, plan, .. } => {
                    Some((plan_id.as_str(), plan.as_ref(), true))
                }
                JobCommand::NetworkProbe { plan_id, plan, .. }
                | JobCommand::NetworkSpeedTest { plan_id, plan, .. } => {
                    Some((plan_id.as_str(), plan.as_ref(), false))
                }
                _ => None,
            })
    else {
        return Ok(());
    };

    let plan_id = Uuid::parse_str(plan_id)
        .map_err(|_| ApiError::bad_request("network_diagnostic_plan_id_invalid"))?;
    let declared = state
        .repo
        .get_tunnel_plan(plan_id)
        .await?
        .ok_or_else(|| ApiError::not_found("network_diagnostic_plan_not_found"))?;
    if !declared.enabled && !status_only {
        return Err(ApiError::conflict("network_diagnostic_plan_disabled"));
    }
    if declared.plan != *supplied_plan {
        return Err(ApiError::conflict("network_diagnostic_plan_snapshot_stale"));
    }

    let Some(JobCommand::NetworkStatus {
        plan,
        side,
        runtime_adapter,
        ..
    }) = request.operation.as_mut()
    else {
        return Ok(());
    };
    *runtime_adapter = None;
    if plan.runtime_control.manager != RuntimeTunnelManager::ExternalManagedAdapter {
        return Ok(());
    }
    let (definition_id, client_id) = match side {
        TunnelEndpointSide::Left => (
            plan.runtime_control.left_adapter_definition_id.as_deref(),
            plan.left_client_id.as_str(),
        ),
        TunnelEndpointSide::Right => (
            plan.runtime_control.right_adapter_definition_id.as_deref(),
            plan.right_client_id.as_str(),
        ),
    };
    let definition_id = definition_id
        .ok_or_else(|| ApiError::bad_request("runtime_tunnel_adapter_definition_required"))?;
    *runtime_adapter = Some(
        state
            .repo
            .resolve_runtime_tunnel_adapter(definition_id)
            .await
            .map_err(|error| {
                warn!(%error, %client_id, %definition_id, "network status adapter binding failed");
                ApiError::conflict("runtime_tunnel_adapter_unavailable")
            })?,
    );
    Ok(())
}

fn job_allows_unprivileged_submission(command: &JobCommand) -> bool {
    matches!(
        command,
        JobCommand::ConfigRead
            | JobCommand::ProcessList { .. }
            | JobCommand::ServiceInventory { .. }
            | JobCommand::ServiceLogs { .. }
            | JobCommand::PackageUpdatePlan {
                refresh_metadata: false,
                ..
            }
            | JobCommand::NetworkStatus { .. }
            | JobCommand::NetworkRoutingStatus { .. }
    ) && !job_command_requires_confirmation(command)
}

fn validate_job_command_source(
    job_command: &JobCommand,
    privilege_source: &JobPrivilegeSource,
) -> Result<(), ApiError> {
    let internal_mutation = matches!(
        privilege_source,
        JobPrivilegeSource::InternalOperatorMutation
    );
    if internal_mutation && server_issued_job_actor(job_command).is_none() {
        return Err(ApiError::bad_request(
            "internal_operator_job_not_server_issued",
        ));
    }
    if matches!(job_command, JobCommand::RuntimeConfigSync { .. }) && !internal_mutation {
        return Err(ApiError::bad_request(
            "runtime_config_sync_is_server_issued",
        ));
    }
    if matches!(
        job_command,
        JobCommand::NetworkRoutingStatus { .. } | JobCommand::NetworkRoutingApply { .. }
    ) && !internal_mutation
    {
        return Err(ApiError::bad_request(
            "network_routing_jobs_are_server_issued",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct BusyUpdateSkip {
    client_id: String,
}

#[derive(Clone, Debug)]
struct NeverConnectedSkip {
    client_id: String,
}

#[derive(Clone, Debug)]
struct NetworkSpeedPeerSkip {
    client_id: String,
    peer_client_id: String,
}

async fn busy_update_skip_targets(
    state: &AppState,
    job_id: Uuid,
    job_command: &JobCommand,
    dispatch_targets: &[String],
) -> Result<Vec<BusyUpdateSkip>, ApiError> {
    if !is_update_lifecycle_command(job_command) || dispatch_targets.is_empty() {
        return Ok(Vec::new());
    }
    let active_clients = state
        .repo
        .active_job_target_client_ids(dispatch_targets, job_id)
        .await?;
    Ok(dispatch_targets
        .iter()
        .filter(|client_id| active_clients.contains(*client_id))
        .cloned()
        .map(|client_id| BusyUpdateSkip { client_id })
        .collect())
}

fn is_update_lifecycle_command(command: &JobCommand) -> bool {
    matches!(
        command,
        JobCommand::UpdateAgent { .. }
            | JobCommand::AgentUpdateActivate { .. }
            | JobCommand::AgentUpdateRollback { .. }
            | JobCommand::AgentUpdateCheck { .. }
    )
}

pub(crate) async fn validate_restore_archive_binding(
    state: &AppState,
    command: &JobCommand,
    resolved_targets: &[String],
) -> Result<(), ApiError> {
    let JobCommand::Restore {
        source_backup_request_id,
        archive_transfer_session_id,
        archive_path,
        archive_size_bytes,
        archive_sha256_hex,
        ..
    } = command
    else {
        return Ok(());
    };
    if resolved_targets.len() != 1 {
        return Err(ApiError::conflict("restore_requires_single_target"));
    }
    if archive_transfer_session_id.is_nil() {
        return Err(ApiError::bad_request(
            "restore_archive_transfer_session_required",
        ));
    }
    let target_client_id = &resolved_targets[0];
    let source_backup = state
        .repo
        .find_backup_request(*source_backup_request_id)
        .await?
        .ok_or_else(|| ApiError::bad_request("restore_source_backup_not_found"))?;
    let artifact_id = source_backup
        .artifact_id
        .ok_or_else(|| ApiError::conflict("restore_source_backup_artifact_required"))?;
    let artifact = state
        .repo
        .find_backup_artifact(artifact_id)
        .await?
        .ok_or_else(|| ApiError::conflict("restore_source_backup_artifact_not_found"))?;
    if artifact.client_id != source_backup.client_id {
        return Err(ApiError::conflict(
            "restore_source_backup_artifact_client_mismatch",
        ));
    }
    if !artifact.content_available {
        return Err(ApiError::conflict(
            "restore_source_backup_artifact_not_available",
        ));
    }
    let transfers = state
        .repo
        .list_file_transfer_sessions(
            1,
            Some(target_client_id),
            Some(*archive_transfer_session_id),
        )
        .await?;
    let transfer = transfers
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::conflict("restore_archive_transfer_not_found"))?;
    if transfer.client_id != *target_client_id
        || transfer.direction != "upload"
        || transfer.status != "completed"
    {
        return Err(ApiError::conflict("restore_archive_transfer_invalid"));
    }
    let transfer_size = transfer
        .size_bytes
        .ok_or_else(|| ApiError::conflict("restore_archive_transfer_size_missing"))?;
    let transfer_sha = transfer
        .sha256_hex
        .as_deref()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::conflict("restore_archive_transfer_sha256_missing"))?;
    let archive_path = archive_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad_request("restore_archive_path_required"))?;
    let archive_size_bytes =
        archive_size_bytes.ok_or_else(|| ApiError::bad_request("restore_archive_size_required"))?;
    let archive_sha256_hex = archive_sha256_hex
        .as_deref()
        .map(|value| value.trim().to_ascii_lowercase())
        .ok_or_else(|| ApiError::bad_request("restore_archive_sha256_required"))?;
    if transfer.path != archive_path {
        return Err(ApiError::conflict("restore_archive_transfer_path_mismatch"));
    }
    if transfer_size <= 0 || transfer_size != artifact.size_bytes {
        return Err(ApiError::conflict("restore_archive_transfer_size_mismatch"));
    }
    if archive_size_bytes != transfer_size as u64 {
        return Err(ApiError::conflict("restore_archive_size_transfer_mismatch"));
    }
    let artifact_sha = artifact.sha256_hex.to_ascii_lowercase();
    if transfer_sha != artifact_sha {
        return Err(ApiError::conflict(
            "restore_archive_transfer_sha256_mismatch",
        ));
    }
    if archive_sha256_hex != transfer_sha {
        return Err(ApiError::conflict(
            "restore_archive_sha256_transfer_mismatch",
        ));
    }
    Ok(())
}

fn never_connected_target_skips(
    targets: &[String],
    agents: &[AgentView],
) -> Vec<NeverConnectedSkip> {
    targets
        .iter()
        .filter_map(|client_id| {
            agents
                .iter()
                .find(|agent| agent.id == *client_id)
                .filter(|agent| target_has_never_connected(agent))
                .map(|_| NeverConnectedSkip {
                    client_id: client_id.clone(),
                })
        })
        .collect()
}

fn target_has_never_connected(agent: &AgentView) -> bool {
    !matches!(agent.status.as_str(), "revoked" | "deleted")
        && (agent.process_incarnation_id.is_none() || agent.status == "never")
}

fn network_speed_test_peer_skips(
    job_command: &JobCommand,
    dispatch_targets: &[String],
) -> Vec<NetworkSpeedPeerSkip> {
    let JobCommand::NetworkSpeedTest { plan, .. } = job_command else {
        return Vec::new();
    };
    let left_dispatchable = dispatch_targets
        .iter()
        .any(|target| target == &plan.left_client_id);
    let right_dispatchable = dispatch_targets
        .iter()
        .any(|target| target == &plan.right_client_id);
    if left_dispatchable == right_dispatchable {
        return Vec::new();
    }
    if left_dispatchable {
        return vec![NetworkSpeedPeerSkip {
            client_id: plan.left_client_id.clone(),
            peer_client_id: plan.right_client_id.clone(),
        }];
    }
    vec![NetworkSpeedPeerSkip {
        client_id: plan.right_client_id.clone(),
        peer_client_id: plan.left_client_id.clone(),
    }]
}

fn confirmation_error_code(command: &JobCommand) -> &'static str {
    match command {
        JobCommand::Backup { .. } => "backup_confirmation_required",
        JobCommand::NetworkSpeedTest { .. } => "network_speed_test_confirmation_required",
        JobCommand::UpdateAgent { .. }
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

fn job_command_requires_dispatch_confirmation(command: &JobCommand) -> bool {
    if matches!(command, JobCommand::Restore { dry_run: true, .. }) {
        return false;
    }
    job_command_requires_confirmation(command)
}

pub(crate) fn request_fingerprint_for_job(
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
        "max_timeout_secs": request
            .max_timeout_secs
            .unwrap_or(DEFAULT_MAX_JOB_TIMEOUT_SECS),
        "privileged": request.privileged,
        "force_unprivileged": request.force_unprivileged,
        "source_schedule_id": source_schedule_id,
        "rollout": request.rollout,
    }))
    .map_err(|error| ApiError::from(anyhow!("failed to encode job fingerprint: {error}")))?;
    Ok(payload_hash(&bytes))
}

fn validate_job_rollout(
    rollout: Option<&crate::model::JobRolloutPolicy>,
    command: &JobCommand,
    resolved_targets: &[String],
) -> Result<(), ApiError> {
    let Some(rollout) = rollout else {
        return Ok(());
    };
    if resolved_targets.len() < 2 {
        return Err(ApiError::bad_request(
            "job_rollout_requires_multiple_targets",
        ));
    }
    if matches!(command, JobCommand::NetworkSpeedTest { .. }) {
        return Err(ApiError::bad_request(
            "job_rollout_network_speed_test_unsupported",
        ));
    }
    if rollout.canary_client_ids.is_empty() || rollout.canary_client_ids.len() > 25 {
        return Err(ApiError::bad_request(
            "job_rollout_canary_count_out_of_range",
        ));
    }
    if rollout.canary_client_ids.len() >= resolved_targets.len() {
        return Err(ApiError::bad_request(
            "job_rollout_canary_must_leave_remaining_targets",
        ));
    }
    let resolved = resolved_targets
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut unique_canaries = HashSet::new();
    for client_id in &rollout.canary_client_ids {
        if !unique_canaries.insert(client_id.as_str()) {
            return Err(ApiError::bad_request(
                "job_rollout_canary_target_duplicated",
            ));
        }
        if !resolved.contains(client_id.as_str()) {
            return Err(ApiError::bad_request(
                "job_rollout_canary_target_not_resolved",
            ));
        }
    }
    if !(1..=100).contains(&rollout.batch_size) {
        return Err(ApiError::bad_request("job_rollout_batch_size_out_of_range"));
    }
    if rollout.max_failures > 100 {
        return Err(ApiError::bad_request(
            "job_rollout_max_failures_out_of_range",
        ));
    }
    if rollout.batch_delay_secs > 86_400 {
        return Err(ApiError::bad_request(
            "job_rollout_batch_delay_out_of_range",
        ));
    }
    Ok(())
}

fn job_rollout_policy_hash(
    rollout: Option<&crate::model::JobRolloutPolicy>,
) -> Result<Option<String>, ApiError> {
    rollout
        .map(|rollout| {
            encode_json(rollout)
                .map(|payload| payload_hash(&payload))
                .map_err(|error| {
                    ApiError::from(anyhow!("failed to encode rollout policy: {error}"))
                })
        })
        .transpose()
}

async fn existing_job_response_for_id(
    state: &AppState,
    operator: &AuthContext,
    job_id: Uuid,
    request_fingerprint: &str,
    allow_approved_request_actor_mismatch: bool,
) -> Result<Option<CreateJobResponse>, ApiError> {
    let Some(existing) = state.repo.get_job(job_id).await? else {
        return Ok(None);
    };
    if !allow_approved_request_actor_mismatch && existing.actor_id != Some(operator.operator.id) {
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
        max_timeout_secs: existing.max_timeout_secs,
        max_job_timeout_secs: state.max_job_timeout_secs(),
        control_deadline_extra_secs: state
            .dispatcher_runtime_config()
            .control_deadline_extra_secs(),
        target_counts: create_job_target_counts(state, existing.id).await?,
        error: None,
        message: None,
        recovery: None,
    }))
}

async fn validate_approved_job_binding(
    state: &AppState,
    approval_id: Uuid,
    job_id: Uuid,
    command_hash: &str,
    request_fingerprint: &str,
) -> Result<(), ApiError> {
    let (approval, _) = state
        .repo
        .get_job_approval_request(approval_id)
        .await?
        .ok_or_else(|| ApiError::not_found("job_approval_not_found"))?;
    if approval.status != "approved" {
        return Err(ApiError::conflict("job_approval_not_pending"));
    }
    if approval.job_id != job_id
        || approval.payload_hash != command_hash
        || approval.request_fingerprint != request_fingerprint
    {
        return Err(ApiError::conflict("job_approval_binding_mismatch"));
    }
    Ok(())
}

pub(crate) async fn create_job_target_counts(
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
        agent_lost: 0,
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
            TARGET_STATUS_AGENT_LOST => counts.agent_lost += 1,
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
    _dispatch_targets: &[String],
    _target_capabilities: &[TargetCapability],
) -> Result<bool, ApiError> {
    if !state.require_registered_agent_updates() {
        return Ok(true);
    }
    match job_command {
        JobCommand::UpdateAgent { sha256_hex, .. }
        | JobCommand::AgentUpdateActivate {
            staged_sha256_hex: sha256_hex,
            ..
        } => state
            .repo
            .agent_update_release_exists_for_artifact(sha256_hex)
            .await
            .map_err(ApiError::from),
        JobCommand::AgentUpdateRollback {
            rollback_sha256_hex: Some(sha256_hex),
        } => state
            .repo
            .agent_update_release_exists_for_rollback_artifact(sha256_hex)
            .await
            .map_err(ApiError::from),
        JobCommand::AgentUpdateRollback {
            rollback_sha256_hex: None,
        } => Ok(false),
        JobCommand::AgentUpdateCheck { .. } => Ok(true),
        _ => Ok(true),
    }
}

fn precompleted_target_outcomes(
    job_id: Uuid,
    job_command: &JobCommand,
    never_connected_skips: Vec<NeverConnectedSkip>,
    fixed_target_unavailable_skips: Vec<FixedTargetUnavailableSkip>,
    capability_skips: Vec<CapabilitySkip>,
    busy_skips: Vec<BusyUpdateSkip>,
    peer_skips: Vec<NetworkSpeedPeerSkip>,
) -> Result<Vec<PrecompletedJobTarget>, ApiError> {
    let mut targets = Vec::with_capacity(
        never_connected_skips.len()
            + fixed_target_unavailable_skips.len()
            + capability_skips.len()
            + busy_skips.len()
            + peer_skips.len(),
    );
    for skip in never_connected_skips {
        let outcome = never_connected_skip_outcome(job_id, &skip, job_command)?;
        targets.push(PrecompletedJobTarget {
            client_id: skip.client_id,
            outcome,
        });
    }
    for skip in fixed_target_unavailable_skips {
        let outcome = fixed_target_unavailable_skip_outcome(job_id, &skip, job_command)?;
        targets.push(PrecompletedJobTarget {
            client_id: skip.client_id,
            outcome,
        });
    }
    for skip in capability_skips {
        let outcome = capability_degraded_outcome(job_id, &skip, job_command)?;
        targets.push(PrecompletedJobTarget {
            client_id: skip.client_id,
            outcome,
        });
    }
    for skip in busy_skips {
        let outcome = busy_update_skip_outcome(job_id, &skip, job_command)?;
        targets.push(PrecompletedJobTarget {
            client_id: skip.client_id,
            outcome,
        });
    }
    for skip in peer_skips {
        let outcome = network_speed_peer_skip_outcome(job_id, &skip, job_command)?;
        targets.push(PrecompletedJobTarget {
            client_id: skip.client_id,
            outcome,
        });
    }
    Ok(targets)
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

pub(crate) const COMMAND_COMPLETED_WITHOUT_EXIT_CODE_MESSAGE: &str =
    "command completed without numeric exit code";

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

pub(crate) fn target_message_for_status(
    outputs: &[CommandOutput],
    fallback: &str,
    status: &str,
    final_output: Option<&CommandOutput>,
) -> String {
    if status == TARGET_STATUS_FAILED
        && final_output.is_some_and(|output| output.done && output.exit_code.is_none())
        && outputs
            .iter()
            .rev()
            .find_map(status_output_message)
            .is_none()
    {
        return COMMAND_COMPLETED_WITHOUT_EXIT_CODE_MESSAGE.to_string();
    }
    target_message_from_outputs(outputs, fallback, status)
}

pub(crate) fn target_capabilities_from_agents(agents: &[AgentView]) -> Vec<TargetCapability> {
    agents
        .iter()
        .map(|agent| TargetCapability {
            client_id: agent.id.clone(),
            arch: agent.arch.clone(),
            capabilities: agent.capabilities.clone(),
        })
        .collect()
}

#[cfg(test)]
fn split_targets_by_capability(
    command: &JobCommand,
    targets: &[String],
    agents: &[AgentView],
    force_unprivileged: bool,
) -> (Vec<String>, Vec<CapabilitySkip>) {
    let capabilities = target_capabilities_from_agents(agents);
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
            exit_code: Some(0),
            done: true,
        }],
    })
}

fn busy_update_skip_outcome(
    job_id: Uuid,
    skip: &BusyUpdateSkip,
    command: &JobCommand,
) -> Result<TargetDispatchOutcome, ApiError> {
    let status = serde_json::json!({
        "type": "busy_update_skipped",
        "status": TARGET_STATUS_SKIPPED,
        "client_id": skip.client_id,
        "command_type": crate::job_request::job_command_type_label(command),
        "reason": "busy_agent_active_jobs",
        "hint": "update command was not dispatched because the client already has another active job target",
    });
    Ok(TargetDispatchOutcome {
        status: TARGET_STATUS_SKIPPED.to_string(),
        exit_code: Some(0),
        #[cfg(test)]
        command_version: None,
        accepted: true,
        message: "busy_agent_active_jobs: target has another active job; update skipped"
            .to_string(),
        received_at: None,
        outputs: vec![CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&status).map_err(|error| ApiError::from(anyhow!(error)))?,
            exit_code: Some(0),
            done: true,
        }],
    })
}

fn network_speed_peer_skip_outcome(
    job_id: Uuid,
    skip: &NetworkSpeedPeerSkip,
    command: &JobCommand,
) -> Result<TargetDispatchOutcome, ApiError> {
    let message = "network_speed_test_peer_unavailable: peer target was skipped; speed test requires both endpoints";
    let status = serde_json::json!({
        "type": "network_speed_test_peer_unavailable",
        "status": TARGET_STATUS_SKIPPED,
        "client_id": skip.client_id,
        "peer_client_id": skip.peer_client_id,
        "command_type": crate::job_request::job_command_type_label(command),
        "reason": "network_speed_test_peer_unavailable",
        "hint": "network speed tests require both tunnel endpoints to remain dispatchable after availability filtering",
        "message": message,
    });
    Ok(TargetDispatchOutcome {
        status: TARGET_STATUS_SKIPPED.to_string(),
        exit_code: Some(0),
        #[cfg(test)]
        command_version: None,
        accepted: false,
        message: message.to_string(),
        received_at: None,
        outputs: vec![CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&status).map_err(|error| ApiError::from(anyhow!(error)))?,
            exit_code: Some(0),
            done: true,
        }],
    })
}

fn never_connected_skip_outcome(
    job_id: Uuid,
    skip: &NeverConnectedSkip,
    command: &JobCommand,
) -> Result<TargetDispatchOutcome, ApiError> {
    let status = serde_json::json!({
        "type": "target_never_connected",
        "status": TARGET_STATUS_SKIPPED,
        "client_id": skip.client_id,
        "command_type": crate::job_request::job_command_type_label(command),
        "reason": "target_never_connected",
        "hint": "target has no accepted agent process incarnation; start or reconnect the agent before dispatch",
        "message": "target_never_connected: target has never connected; job skipped",
    });
    Ok(TargetDispatchOutcome {
        status: TARGET_STATUS_SKIPPED.to_string(),
        exit_code: Some(0),
        #[cfg(test)]
        command_version: None,
        accepted: false,
        message: "target_never_connected: target has never connected; job skipped".to_string(),
        received_at: None,
        outputs: vec![CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&status).map_err(|error| ApiError::from(anyhow!(error)))?,
            exit_code: Some(0),
            done: true,
        }],
    })
}

fn fixed_target_unavailable_skip_outcome(
    job_id: Uuid,
    skip: &FixedTargetUnavailableSkip,
    command: &JobCommand,
) -> Result<TargetDispatchOutcome, ApiError> {
    let message = "fixed_target_unavailable: saved schedule target no longer resolves to a dispatchable VPS; target skipped";
    let status = serde_json::json!({
        "type": "fixed_target_unavailable",
        "status": TARGET_STATUS_SKIPPED,
        "client_id": skip.client_id,
        "command_type": crate::job_request::job_command_type_label(command),
        "reason": "fixed_target_unavailable",
        "hint": "review the saved schedule target list before the next run",
        "message": message,
    });
    Ok(TargetDispatchOutcome {
        status: TARGET_STATUS_SKIPPED.to_string(),
        exit_code: Some(0),
        #[cfg(test)]
        command_version: None,
        accepted: false,
        message: message.to_string(),
        received_at: None,
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
    job_id: Uuid,
    request: &CreateJobRequest,
    command_hash: &str,
    request_fingerprint: &str,
    operator: &AuthContext,
    status: &'static str,
    error_code: &'static str,
    reason: &'static str,
    response_status: StatusCode,
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
        response_status,
        Json(CreateJobResponse {
            job_id,
            target_count,
            status,
            max_timeout_secs: request
                .max_timeout_secs
                .unwrap_or_else(|| DEFAULT_MAX_JOB_TIMEOUT_SECS.min(state.max_job_timeout_secs())),
            max_job_timeout_secs: state.max_job_timeout_secs(),
            control_deadline_extra_secs: state
                .dispatcher_runtime_config()
                .control_deadline_extra_secs(),
            target_counts,
            error: Some(error_code.to_string()),
            message: Some(reason.to_string()),
            recovery: Some(public_recovery(response_status, error_code).to_string()),
        }),
    ))
}

fn validate_job_audit_selector(selector_expression: &str) -> Result<(), ApiError> {
    let selector_expression = selector_expression.trim();
    if selector_expression.len() > 2048 || selector_expression.chars().any(char::is_control) {
        return Err(ApiError::bad_request("invalid_selector_expression"));
    }
    Ok(())
}

pub(crate) fn effective_job_max_timeout_secs(
    requested_max_timeout_secs: Option<u64>,
    max_job_timeout_secs: u64,
) -> Result<u64, ApiError> {
    let max_job_timeout_secs = max_job_timeout_secs.max(1);
    let default_max_timeout_secs = DEFAULT_MAX_JOB_TIMEOUT_SECS.min(max_job_timeout_secs);
    let max_timeout_secs = requested_max_timeout_secs
        .unwrap_or(default_max_timeout_secs)
        .max(1);
    if max_timeout_secs > max_job_timeout_secs {
        return Err(ApiError::bad_request(
            "max_timeout_exceeds_configured_job_max",
        ));
    }
    Ok(max_timeout_secs)
}

fn bounded_cancel_reason(reason: Option<&str>) -> Option<String> {
    reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(512).collect())
}

fn bounded_review_reason(reason: Option<&str>) -> Option<String> {
    reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(1024).collect())
}

fn normalized_job_approval_risk(
    risk: Option<&str>,
    request: &CreateJobRequest,
) -> Result<String, ApiError> {
    let default_risk = if request.destructive {
        "destructive"
    } else if request.privileged {
        "privileged"
    } else {
        "standard"
    };
    let risk = risk.unwrap_or(default_risk).trim().to_ascii_lowercase();
    if risk.is_empty() || risk.len() > 64 || risk.chars().any(char::is_control) {
        return Err(ApiError::bad_request("job_approval_risk_invalid"));
    }
    Ok(risk)
}

fn map_job_approval_repo_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.starts_with("job_approval_not_found") {
        ApiError::not_found("job_approval_not_found")
    } else if message.starts_with("job_approval_not_pending") {
        ApiError::conflict("job_approval_not_pending")
    } else if message.starts_with("job_approval_id_reused") {
        ApiError::conflict("job_approval_id_reused")
    } else if message.starts_with("job_approval_decision_invalid") {
        ApiError::bad_request("job_approval_decision_invalid")
    } else {
        ApiError::from(error)
    }
}

fn map_job_recording_error(error: anyhow::Error) -> ApiError {
    if error
        .to_string()
        .starts_with("job_target_no_longer_available")
    {
        ApiError::conflict("job_target_no_longer_available")
    } else {
        ApiError::from(error)
    }
}

#[derive(Clone, Debug)]
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
    let (status, exit_code) = target_status_from_final_output(final_output);
    let message = if target_status_needs_reason(status) {
        target_message_for_status(&result.outputs, &result.message, status, final_output)
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

pub(crate) fn target_status_from_final_output(
    final_output: Option<&CommandOutput>,
) -> (&'static str, Option<i32>) {
    let Some(final_output) = final_output else {
        return (TARGET_STATUS_RUNNING, None);
    };
    let exit_code = final_output.exit_code;
    if output_indicates_rejected(final_output) {
        (TARGET_STATUS_REJECTED, exit_code)
    } else if output_indicates_timeout(final_output) {
        (TARGET_STATUS_AGENT_TIMEOUT, exit_code)
    } else if output_indicates_canceled(final_output) {
        (TARGET_STATUS_CANCELED, exit_code)
    } else {
        match exit_code {
            Some(0) => (TARGET_STATUS_COMPLETED, exit_code),
            Some(_) | None => (TARGET_STATUS_FAILED, exit_code),
        }
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

pub(crate) fn output_indicates_canceled(output: &CommandOutput) -> bool {
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

pub(crate) fn output_indicates_rejected(output: &CommandOutput) -> bool {
    if output.stream != OutputStream::Status {
        return false;
    }
    serde_json::from_slice::<serde_json::Value>(&output.data)
        .ok()
        .is_some_and(|value| {
            value
                .get("status")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|status| status == TARGET_STATUS_REJECTED)
                || value
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|kind| kind == "unsupported_command_version")
        })
}

#[cfg(test)]
#[path = "tests_routes_jobs.rs"]
mod tests;
