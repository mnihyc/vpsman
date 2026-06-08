use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use uuid::Uuid;

use crate::{
    error::ApiError,
    model::{AgentUpdateRolloutControlRequest, AgentUpdateRolloutView, HistoryQuery},
    repository_rollouts::{
        ROLLOUT_HEALTH_GATE_HEARTBEAT_VERIFIED, ROLLOUT_HEALTH_GATE_MANUAL_AFTER_CANARY,
        ROLLOUT_HEALTH_GATE_MANUAL_ONLY,
    },
    state::AppState,
    util::limit_or_default,
};

const MAX_ROLLOUT_PAUSE_REASON_BYTES: usize = 512;

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
