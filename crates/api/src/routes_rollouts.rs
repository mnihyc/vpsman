use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use uuid::Uuid;

use crate::{
    error::ApiError,
    model::{
        AgentUpdateActivationDelegationRequest, AgentUpdateActivationDelegationView,
        AgentUpdateRollbackDelegationRequest, AgentUpdateRollbackDelegationView,
        AgentUpdateRolloutControlRequest, AgentUpdateRolloutView, HistoryQuery,
    },
    repository_rollouts::{
        ROLLOUT_HEALTH_GATE_HEARTBEAT_VERIFIED, ROLLOUT_HEALTH_GATE_MANUAL_AFTER_CANARY,
        ROLLOUT_HEALTH_GATE_MANUAL_ONLY,
    },
    state::AppState,
    util::limit_or_default,
};

const MAX_ROLLOUT_PAUSE_REASON_BYTES: usize = 512;
const MAX_ROLLOUT_DELEGATED_TARGETS: usize = 200;

pub(crate) async fn list_agent_update_rollouts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<AgentUpdateRolloutView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(
        state
            .repo
            .list_agent_update_rollouts(limit_or_default(query.limit))
            .await?,
    ))
}

pub(crate) async fn update_agent_update_rollout_control(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(rollout_id): Path<Uuid>,
    Json(request): Json<AgentUpdateRolloutControlRequest>,
) -> Result<(StatusCode, Json<AgentUpdateRolloutView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    validate_agent_update_rollout_control_request(&request)?;
    let rollout = state
        .repo
        .update_agent_update_rollout_control(rollout_id, &request, &operator)
        .await
        .map_err(|error| {
            if error.to_string().contains("agent_update_rollout_not_found") {
                ApiError::not_found("agent_update_rollout_not_found")
            } else {
                ApiError::from(error)
            }
        })?;
    Ok((StatusCode::OK, Json(rollout)))
}

pub(crate) async fn record_agent_update_rollback_delegation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(rollout_id): Path<Uuid>,
    Json(request): Json<AgentUpdateRollbackDelegationRequest>,
) -> Result<(StatusCode, Json<AgentUpdateRollbackDelegationView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    validate_agent_update_rollback_delegation_request(&request)?;
    let summary = state
        .repo
        .record_agent_update_rollback_delegation(rollout_id, &request, &operator)
        .await
        .map_err(|error| {
            let message = error.to_string();
            if message.contains("agent_update_rollout_not_found") {
                ApiError::not_found("agent_update_rollout_not_found")
            } else if message.contains("agent_update_rollout_delegation") {
                ApiError::bad_request("agent_update_rollout_delegation_invalid")
            } else {
                ApiError::from(error)
            }
        })?;
    Ok((StatusCode::CREATED, Json(summary)))
}

pub(crate) async fn record_agent_update_activation_delegation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(rollout_id): Path<Uuid>,
    Json(request): Json<AgentUpdateActivationDelegationRequest>,
) -> Result<(StatusCode, Json<AgentUpdateActivationDelegationView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    validate_agent_update_activation_delegation_request(&request)?;
    let summary = state
        .repo
        .record_agent_update_activation_delegation(rollout_id, &request, &operator)
        .await
        .map_err(|error| {
            let message = error.to_string();
            if message.contains("agent_update_rollout_not_found") {
                ApiError::not_found("agent_update_rollout_not_found")
            } else if message.contains("agent_update_rollout_delegation") {
                ApiError::bad_request("agent_update_rollout_delegation_invalid")
            } else {
                ApiError::from(error)
            }
        })?;
    Ok((StatusCode::CREATED, Json(summary)))
}

pub(crate) fn validate_agent_update_rollout_control_request(
    request: &AgentUpdateRolloutControlRequest,
) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::conflict(
            "agent_update_rollout_control_confirmation_required",
        ));
    }
    if request.paused.is_none() && request.automation_health_gate.is_none() {
        return Err(ApiError::bad_request("agent_update_rollout_control_empty"));
    }
    if request
        .pause_reason
        .as_ref()
        .is_some_and(|reason| reason.len() > MAX_ROLLOUT_PAUSE_REASON_BYTES)
    {
        return Err(ApiError::bad_request(
            "agent_update_rollout_pause_reason_too_long",
        ));
    }
    if let Some(health_gate) = request.automation_health_gate.as_deref() {
        if !matches!(
            health_gate,
            ROLLOUT_HEALTH_GATE_HEARTBEAT_VERIFIED
                | ROLLOUT_HEALTH_GATE_MANUAL_AFTER_CANARY
                | ROLLOUT_HEALTH_GATE_MANUAL_ONLY
        ) {
            return Err(ApiError::bad_request(
                "agent_update_rollout_health_gate_invalid",
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_agent_update_rollback_delegation_request(
    request: &AgentUpdateRollbackDelegationRequest,
) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::conflict(
            "agent_update_rollback_delegation_confirmation_required",
        ));
    }
    if request.envelopes.is_empty() {
        return Err(ApiError::bad_request(
            "agent_update_rollback_delegation_envelopes_required",
        ));
    }
    if request.envelopes.len() > MAX_ROLLOUT_DELEGATED_TARGETS {
        return Err(ApiError::bad_request(
            "agent_update_rollback_delegation_too_many_targets",
        ));
    }
    if let Some(rollback_sha256_hex) = request.rollback_sha256_hex.as_deref() {
        if !is_sha256_hex(rollback_sha256_hex) {
            return Err(ApiError::bad_request(
                "agent_update_rollback_delegation_rollback_hash_invalid",
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_agent_update_activation_delegation_request(
    request: &AgentUpdateActivationDelegationRequest,
) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::conflict(
            "agent_update_activation_delegation_confirmation_required",
        ));
    }
    if request.envelopes.is_empty() {
        return Err(ApiError::bad_request(
            "agent_update_activation_delegation_envelopes_required",
        ));
    }
    if request.envelopes.len() > MAX_ROLLOUT_DELEGATED_TARGETS {
        return Err(ApiError::bad_request(
            "agent_update_activation_delegation_too_many_targets",
        ));
    }
    Ok(())
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.as_bytes().iter().all(u8::is_ascii_hexdigit)
}
