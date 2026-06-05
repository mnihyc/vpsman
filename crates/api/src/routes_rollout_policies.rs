use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};

use crate::{
    error::ApiError,
    model_rollout_policies::{
        AgentUpdateRolloutPolicyQuery, AgentUpdateRolloutPolicyView,
        CreateAgentUpdateRolloutPolicyRequest,
    },
    repository_rollouts::{
        ROLLOUT_HEALTH_GATE_HEARTBEAT_VERIFIED, ROLLOUT_HEALTH_GATE_MANUAL_AFTER_CANARY,
        ROLLOUT_HEALTH_GATE_MANUAL_ONLY,
    },
    state::AppState,
    util::limit_or_default,
};

const MAX_POLICY_NAME_BYTES: usize = 128;
const MAX_POLICY_SCOPE_VALUE_BYTES: usize = 256;
const MAX_POLICY_CHANNEL_BYTES: usize = 64;
const MAX_POLICY_NOTES_BYTES: usize = 1024;

pub(crate) async fn list_agent_update_rollout_policies(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AgentUpdateRolloutPolicyQuery>,
) -> Result<Json<Vec<AgentUpdateRolloutPolicyView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    validate_policy_query(&query)?;
    Ok(Json(
        state
            .repo
            .list_agent_update_rollout_policies(
                limit_or_default(query.limit),
                query.enabled,
                query.channel.as_deref(),
            )
            .await?,
    ))
}

pub(crate) async fn create_agent_update_rollout_policy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateAgentUpdateRolloutPolicyRequest>,
) -> Result<(StatusCode, Json<AgentUpdateRolloutPolicyView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    validate_create_agent_update_rollout_policy(&request)?;
    Ok((
        StatusCode::CREATED,
        Json(
            state
                .repo
                .upsert_agent_update_rollout_policy(&request, &operator)
                .await?,
        ),
    ))
}

pub(crate) fn validate_create_agent_update_rollout_policy(
    request: &CreateAgentUpdateRolloutPolicyRequest,
) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::conflict(
            "agent_update_rollout_policy_confirmation_required",
        ));
    }
    validate_text_field(
        &request.name,
        1,
        MAX_POLICY_NAME_BYTES,
        "agent_update_rollout_policy_name_invalid",
    )?;
    validate_scope(&request.scope_kind, request.scope_value.as_deref())?;
    if let Some(channel) = request.channel.as_deref() {
        validate_text_field(
            channel,
            1,
            MAX_POLICY_CHANNEL_BYTES,
            "agent_update_rollout_policy_channel_invalid",
        )?;
    }
    if let Some(canary_count) = request.canary_count {
        if !(0..=10_000).contains(&canary_count) {
            return Err(ApiError::bad_request(
                "agent_update_rollout_policy_canary_count_out_of_range",
            ));
        }
    }
    if let Some(health_gate) = request.automation_health_gate.as_deref() {
        validate_health_gate(health_gate)?;
    }
    if !(-100_000..=100_000).contains(&request.priority) {
        return Err(ApiError::bad_request(
            "agent_update_rollout_policy_priority_out_of_range",
        ));
    }
    if let Some(notes) = request.notes.as_deref() {
        validate_text_field(
            notes,
            0,
            MAX_POLICY_NOTES_BYTES,
            "agent_update_rollout_policy_notes_invalid",
        )?;
    }
    Ok(())
}

fn validate_policy_query(query: &AgentUpdateRolloutPolicyQuery) -> Result<(), ApiError> {
    if let Some(channel) = query.channel.as_deref() {
        validate_text_field(
            channel,
            1,
            MAX_POLICY_CHANNEL_BYTES,
            "agent_update_rollout_policy_channel_invalid",
        )?;
    }
    Ok(())
}

fn validate_scope(scope_kind: &str, scope_value: Option<&str>) -> Result<(), ApiError> {
    let scope_kind = scope_kind.trim();
    if !matches!(scope_kind, "global" | "tag" | "provider") {
        return Err(ApiError::bad_request(
            "agent_update_rollout_policy_scope_kind_invalid",
        ));
    }
    let scope_value = scope_value.map(str::trim).filter(|value| !value.is_empty());
    if scope_kind == "global" {
        if scope_value.is_some() {
            return Err(ApiError::bad_request(
                "agent_update_rollout_policy_global_scope_value_forbidden",
            ));
        }
        return Ok(());
    }
    let Some(scope_value) = scope_value else {
        return Err(ApiError::bad_request(
            "agent_update_rollout_policy_scope_value_required",
        ));
    };
    validate_text_field(
        scope_value,
        1,
        MAX_POLICY_SCOPE_VALUE_BYTES,
        "agent_update_rollout_policy_scope_value_invalid",
    )
}

fn validate_health_gate(health_gate: &str) -> Result<(), ApiError> {
    if !matches!(
        health_gate.trim(),
        ROLLOUT_HEALTH_GATE_HEARTBEAT_VERIFIED
            | ROLLOUT_HEALTH_GATE_MANUAL_AFTER_CANARY
            | ROLLOUT_HEALTH_GATE_MANUAL_ONLY
    ) {
        return Err(ApiError::bad_request(
            "agent_update_rollout_policy_health_gate_invalid",
        ));
    }
    Ok(())
}

fn validate_text_field(
    value: &str,
    min_len: usize,
    max_len: usize,
    error_code: &'static str,
) -> Result<(), ApiError> {
    let value = value.trim();
    if value.len() < min_len || value.len() > max_len || value.chars().any(char::is_control) {
        return Err(ApiError::bad_request(error_code));
    }
    Ok(())
}
