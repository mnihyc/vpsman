use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use vpsman_common::{
    is_fleet_alert_notification_delivery_process_status,
    is_fleet_alert_notification_delivery_status,
};

use crate::{
    error::ApiError,
    model::{FleetAlertQuery, FleetAlertView},
    model_alert_notifications::{
        CreateFleetAlertNotificationChannelRequest, DeleteFleetAlertNotificationChannelRequest,
        FleetAlertNotificationChannelQuery, FleetAlertNotificationChannelView,
        FleetAlertNotificationDeliveryQuery, FleetAlertNotificationDeliveryView,
        FleetAlertNotificationDispatchRequest, FleetAlertNotificationProcessRequest,
    },
    model_alert_policies::{
        CreateFleetAlertPolicyRequest, DeleteFleetAlertPolicyRequest, FleetAlertPolicyOverrideView,
        FleetAlertPolicyQuery,
    },
    model_alert_states::{
        FleetAlertExportView, FleetAlertStateQuery, FleetAlertStateView,
        UpdateFleetAlertStateRequest,
    },
    security::{
        operator_has_scope, SCOPE_BACKUPS_READ, SCOPE_FLEET_READ, SCOPE_INTEGRATIONS_READ,
        SCOPE_INTEGRATIONS_WRITE,
    },
    state::AppState,
    unix_now,
    util::limit_or_default,
};

pub(crate) async fn list_fleet_alerts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FleetAlertQuery>,
) -> Result<Json<Vec<FleetAlertView>>, ApiError> {
    let operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    if !operator_has_scope(&operator.operator.scopes, SCOPE_BACKUPS_READ) {
        return Err(ApiError::forbidden("operator_scope_insufficient"));
    }
    validate_alert_query(&query)?;
    Ok(Json(state.list_fleet_alerts(query).await?))
}

pub(crate) async fn export_fleet_alerts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FleetAlertQuery>,
) -> Result<Json<FleetAlertExportView>, ApiError> {
    let operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    if !operator_has_scope(&operator.operator.scopes, SCOPE_BACKUPS_READ) {
        return Err(ApiError::forbidden("operator_scope_insufficient"));
    }
    validate_alert_query(&query)?;
    let query_summary = serde_json::json!({
        "limit": query.limit,
        "client_id": &query.client_id,
        "severity": &query.severity,
        "category": &query.category,
        "operator_state": &query.operator_state,
        "include_muted": query.include_muted,
    });
    let alerts = state.list_fleet_alerts(query).await?;
    Ok(Json(FleetAlertExportView {
        generated_at: unix_now().to_string(),
        query: query_summary,
        alerts,
    }))
}

pub(crate) async fn list_fleet_alert_states(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FleetAlertStateQuery>,
) -> Result<Json<Vec<FleetAlertStateView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    validate_alert_state_query(&query)?;
    Ok(Json(
        state
            .repo
            .list_fleet_alert_states(
                query.limit.unwrap_or(50).clamp(1, 1000),
                query.state.as_deref(),
            )
            .await?,
    ))
}

pub(crate) async fn update_fleet_alert_state(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpdateFleetAlertStateRequest>,
) -> Result<Json<FleetAlertStateView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_INTEGRATIONS_WRITE)
        .await?;
    validate_alert_state_request(&request)?;
    Ok(Json(
        state
            .repo
            .update_fleet_alert_state(&request, &operator)
            .await?,
    ))
}

pub(crate) async fn list_fleet_alert_policies(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FleetAlertPolicyQuery>,
) -> Result<Json<Vec<FleetAlertPolicyOverrideView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    validate_alert_policy_query(&query)?;
    Ok(Json(
        state
            .repo
            .list_fleet_alert_policies(
                limit_or_default(query.limit),
                query.enabled,
                query.scope_kind.as_deref(),
                query.scope_value.as_deref(),
            )
            .await?,
    ))
}

pub(crate) async fn upsert_fleet_alert_policy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateFleetAlertPolicyRequest>,
) -> Result<Json<FleetAlertPolicyOverrideView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_INTEGRATIONS_WRITE)
        .await?;
    validate_alert_policy_request(&request)?;
    Ok(Json(
        state
            .repo
            .upsert_fleet_alert_policy(&request, &operator)
            .await?,
    ))
}

pub(crate) async fn delete_fleet_alert_policy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(policy_id): Path<uuid::Uuid>,
    Json(request): Json<DeleteFleetAlertPolicyRequest>,
) -> Result<StatusCode, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_INTEGRATIONS_WRITE)
        .await?;
    validate_delete_confirmation(
        request.confirmed,
        &request.reviewed_name,
        "fleet_alert_policy_delete_confirmation_required",
        "fleet_alert_policy_delete_review_invalid",
    )?;
    let existing = state
        .repo
        .list_fleet_alert_policies(1000, None, None, None)
        .await?
        .into_iter()
        .find(|policy| policy.id == policy_id)
        .ok_or_else(|| ApiError::not_found("fleet_alert_policy_not_found"))?;
    if existing.name != request.reviewed_name.trim() {
        return Err(ApiError::conflict("fleet_alert_policy_delete_review_stale"));
    }
    state
        .repo
        .delete_fleet_alert_policy(policy_id, &operator)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn list_fleet_alert_notification_channels(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FleetAlertNotificationChannelQuery>,
) -> Result<Json<Vec<FleetAlertNotificationChannelView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_INTEGRATIONS_READ)
        .await?;
    validate_alert_notification_channel_query(&query)?;
    Ok(Json(
        state
            .repo
            .list_fleet_alert_notification_channels(
                limit_or_default(query.limit),
                query.enabled,
                query.scope_kind.as_deref(),
                query.scope_value.as_deref(),
                query.delivery_kind.as_deref(),
            )
            .await?,
    ))
}

pub(crate) async fn upsert_fleet_alert_notification_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateFleetAlertNotificationChannelRequest>,
) -> Result<Json<FleetAlertNotificationChannelView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_INTEGRATIONS_WRITE)
        .await?;
    validate_alert_notification_channel_request(&request)?;
    Ok(Json(
        state
            .repo
            .upsert_fleet_alert_notification_channel(&request, &operator)
            .await?,
    ))
}

pub(crate) async fn delete_fleet_alert_notification_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(channel_id): Path<uuid::Uuid>,
    Json(request): Json<DeleteFleetAlertNotificationChannelRequest>,
) -> Result<StatusCode, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_INTEGRATIONS_WRITE)
        .await?;
    validate_delete_confirmation(
        request.confirmed,
        &request.reviewed_name,
        "fleet_alert_notification_channel_delete_confirmation_required",
        "fleet_alert_notification_channel_delete_review_invalid",
    )?;
    let existing = state
        .repo
        .list_fleet_alert_notification_channels(1000, None, None, None, None)
        .await?
        .into_iter()
        .find(|channel| channel.id == channel_id)
        .ok_or_else(|| ApiError::not_found("fleet_alert_notification_channel_not_found"))?;
    if existing.name != request.reviewed_name.trim() {
        return Err(ApiError::conflict(
            "fleet_alert_notification_channel_delete_review_stale",
        ));
    }
    state
        .repo
        .delete_fleet_alert_notification_channel(channel_id, &operator)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn list_fleet_alert_notifications(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FleetAlertNotificationDeliveryQuery>,
) -> Result<Json<Vec<FleetAlertNotificationDeliveryView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_INTEGRATIONS_READ)
        .await?;
    validate_alert_notification_delivery_query(&query)?;
    Ok(Json(
        state
            .repo
            .list_fleet_alert_notification_deliveries(
                limit_or_default(query.limit),
                query.channel_id,
                query.alert_id.as_deref(),
                query.status.as_deref(),
            )
            .await?,
    ))
}

pub(crate) async fn dispatch_fleet_alert_notifications(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<FleetAlertNotificationDispatchRequest>,
) -> Result<Json<Vec<FleetAlertNotificationDeliveryView>>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_INTEGRATIONS_WRITE)
        .await?;
    validate_alert_notification_dispatch_request(&request)?;
    Ok(Json(
        state
            .dispatch_fleet_alert_notifications(&request, &operator)
            .await
            .map_err(alert_notification_delivery_error)?,
    ))
}

pub(crate) async fn process_fleet_alert_notifications(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<FleetAlertNotificationProcessRequest>,
) -> Result<Json<Vec<FleetAlertNotificationDeliveryView>>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_INTEGRATIONS_WRITE)
        .await?;
    validate_alert_notification_process_request(&request)?;
    Ok(Json(
        state
            .process_fleet_alert_notifications(&request, &operator)
            .await
            .map_err(alert_notification_delivery_error)?,
    ))
}

fn alert_notification_delivery_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("preview_hash_mismatch") {
        return ApiError::conflict("fleet_alert_notification_preview_hash_mismatch");
    }
    ApiError::from(error)
}

fn validate_alert_query(query: &FleetAlertQuery) -> Result<(), ApiError> {
    if let Some(limit) = query.limit {
        if !(1..=200).contains(&limit) {
            return Err(ApiError::bad_request("fleet_alert_limit_invalid"));
        }
    }
    if let Some(client_id) = query.client_id.as_deref() {
        if client_id.is_empty() || client_id.len() > 128 {
            return Err(ApiError::bad_request("fleet_alert_client_id_invalid"));
        }
    }
    if let Some(severity) = query.severity.as_deref() {
        if !matches!(severity, "critical" | "warning" | "info") {
            return Err(ApiError::bad_request("fleet_alert_severity_invalid"));
        }
    }
    if let Some(category) = query.category.as_deref() {
        validate_alert_token(category, "fleet_alert_category_invalid")?;
    }
    if let Some(operator_state) = query.operator_state.as_deref() {
        validate_alert_state_value(operator_state, "fleet_alert_operator_state_invalid")?;
    }
    Ok(())
}

fn validate_delete_confirmation(
    confirmed: bool,
    reviewed_name: &str,
    confirmation_error: &'static str,
    reviewed_name_error: &'static str,
) -> Result<(), ApiError> {
    if !confirmed {
        return Err(ApiError::bad_request(confirmation_error));
    }
    validate_short_required_value(reviewed_name, reviewed_name_error)?;
    Ok(())
}

fn validate_alert_state_query(query: &FleetAlertStateQuery) -> Result<(), ApiError> {
    if let Some(limit) = query.limit {
        if !(1..=1000).contains(&limit) {
            return Err(ApiError::bad_request("fleet_alert_state_limit_invalid"));
        }
    }
    if let Some(state) = query.state.as_deref() {
        validate_alert_state_value(state, "fleet_alert_state_invalid")?;
    }
    Ok(())
}

fn validate_alert_state_request(request: &UpdateFleetAlertStateRequest) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "fleet_alert_state_confirmation_required",
        ));
    }
    validate_alert_id(&request.alert_id)?;
    match request.action.trim() {
        "acknowledge" | "escalate" | "clear" => {}
        "mute" => {
            if let Some(seconds) = request.muted_for_secs {
                if !(60..=90 * 24 * 60 * 60).contains(&seconds) {
                    return Err(ApiError::bad_request("fleet_alert_mute_duration_invalid"));
                }
            }
        }
        _ => return Err(ApiError::bad_request("fleet_alert_state_action_invalid")),
    }
    if let Some(reason) = request.reason.as_deref() {
        if reason.len() > 1024 {
            return Err(ApiError::bad_request("fleet_alert_state_reason_too_long"));
        }
    }
    Ok(())
}

fn validate_alert_notification_channel_query(
    query: &FleetAlertNotificationChannelQuery,
) -> Result<(), ApiError> {
    if let Some(limit) = query.limit {
        if !(1..=1000).contains(&limit) {
            return Err(ApiError::bad_request(
                "fleet_alert_notification_channel_limit_invalid",
            ));
        }
    }
    if let Some(scope_kind) = query.scope_kind.as_deref() {
        validate_alert_policy_scope_kind(scope_kind)?;
    }
    if let Some(scope_value) = query.scope_value.as_deref() {
        validate_short_required_value(scope_value, "fleet_alert_notification_scope_value_invalid")?;
    }
    if let Some(delivery_kind) = query.delivery_kind.as_deref() {
        validate_alert_token(
            delivery_kind,
            "fleet_alert_notification_delivery_kind_invalid",
        )?;
    }
    Ok(())
}

fn validate_alert_notification_channel_request(
    request: &CreateFleetAlertNotificationChannelRequest,
) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "fleet_alert_notification_channel_confirmation_required",
        ));
    }
    validate_short_required_value(
        &request.name,
        "fleet_alert_notification_channel_name_invalid",
    )?;
    validate_alert_policy_scope_kind(&request.scope_kind)?;
    if request.scope_kind.trim() == "global" {
        if request
            .scope_value
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            return Err(ApiError::bad_request(
                "fleet_alert_notification_global_scope_value_invalid",
            ));
        }
    } else if request
        .scope_value
        .as_deref()
        .is_none_or(|value| value.trim().is_empty() || value.len() > 128)
    {
        return Err(ApiError::bad_request(
            "fleet_alert_notification_scope_value_required",
        ));
    }
    if let Some(min_severity) = request.min_severity.as_deref() {
        validate_alert_severity(
            min_severity,
            "fleet_alert_notification_min_severity_invalid",
        )?;
    }
    validate_alert_token_list(
        request.categories.as_deref().unwrap_or(&[]),
        "fleet_alert_notification_category_invalid",
    )?;
    for state in request.operator_states.as_deref().unwrap_or(&[]) {
        validate_alert_state_value(state, "fleet_alert_notification_operator_state_invalid")?;
    }
    validate_alert_token(
        &request.delivery_kind,
        "fleet_alert_notification_delivery_kind_invalid",
    )?;
    let target = request.target.trim();
    if target.is_empty() || target.len() > 512 || target.as_bytes().contains(&0) {
        return Err(ApiError::bad_request(
            "fleet_alert_notification_target_invalid",
        ));
    }
    if let Some(cooldown_secs) = request.cooldown_secs {
        if !(0..=30 * 24 * 60 * 60).contains(&cooldown_secs) {
            return Err(ApiError::bad_request(
                "fleet_alert_notification_cooldown_invalid",
            ));
        }
    }
    if let Some(notes) = request.notes.as_deref() {
        if notes.len() > 1024 {
            return Err(ApiError::bad_request(
                "fleet_alert_notification_notes_too_long",
            ));
        }
    }
    Ok(())
}

fn validate_alert_notification_delivery_query(
    query: &FleetAlertNotificationDeliveryQuery,
) -> Result<(), ApiError> {
    if let Some(limit) = query.limit {
        if !(1..=1000).contains(&limit) {
            return Err(ApiError::bad_request(
                "fleet_alert_notification_delivery_limit_invalid",
            ));
        }
    }
    if let Some(alert_id) = query.alert_id.as_deref() {
        validate_alert_id(alert_id)?;
    }
    if let Some(status) = query.status.as_deref() {
        if !is_fleet_alert_notification_delivery_status(status) {
            return Err(ApiError::bad_request(
                "fleet_alert_notification_status_invalid",
            ));
        }
    }
    Ok(())
}

fn validate_alert_notification_dispatch_request(
    request: &FleetAlertNotificationDispatchRequest,
) -> Result<(), ApiError> {
    if !request.dry_run.unwrap_or(false) && !request.confirmed {
        return Err(ApiError::bad_request(
            "fleet_alert_notification_dispatch_confirmation_required",
        ));
    }
    if !request.dry_run.unwrap_or(false)
        && request
            .preview_hash
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
    {
        return Err(ApiError::bad_request(
            "fleet_alert_notification_dispatch_preview_hash_required",
        ));
    }
    validate_alert_query(&FleetAlertQuery {
        limit: request.limit,
        client_id: request.client_id.clone(),
        severity: request.severity.clone(),
        category: request.category.clone(),
        operator_state: request.operator_state.clone(),
        include_muted: request.include_muted,
    })?;
    Ok(())
}

fn validate_alert_notification_process_request(
    request: &FleetAlertNotificationProcessRequest,
) -> Result<(), ApiError> {
    if !request.dry_run.unwrap_or(false) && !request.confirmed {
        return Err(ApiError::bad_request(
            "fleet_alert_notification_process_confirmation_required",
        ));
    }
    if !request.dry_run.unwrap_or(false)
        && request
            .preview_hash
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
    {
        return Err(ApiError::bad_request(
            "fleet_alert_notification_process_preview_hash_required",
        ));
    }
    if let Some(limit) = request.limit {
        if !(1..=200).contains(&limit) {
            return Err(ApiError::bad_request(
                "fleet_alert_notification_process_limit_invalid",
            ));
        }
    }
    if let Some(status) = request.status.as_deref() {
        if !is_fleet_alert_notification_delivery_process_status(status) {
            return Err(ApiError::bad_request(
                "fleet_alert_notification_process_status_invalid",
            ));
        }
    }
    if let Some(delivery_kind) = request.delivery_kind.as_deref() {
        validate_alert_token(
            delivery_kind,
            "fleet_alert_notification_delivery_kind_invalid",
        )?;
    }
    Ok(())
}

fn validate_alert_policy_query(query: &FleetAlertPolicyQuery) -> Result<(), ApiError> {
    if let Some(limit) = query.limit {
        if !(1..=1000).contains(&limit) {
            return Err(ApiError::bad_request("fleet_alert_policy_limit_invalid"));
        }
    }
    if let Some(scope_kind) = query.scope_kind.as_deref() {
        validate_alert_policy_scope_kind(scope_kind)?;
    }
    if let Some(scope_value) = query.scope_value.as_deref() {
        if scope_value.is_empty() || scope_value.len() > 128 {
            return Err(ApiError::bad_request(
                "fleet_alert_policy_scope_value_invalid",
            ));
        }
    }
    Ok(())
}

fn validate_alert_severity(severity: &str, error: &'static str) -> Result<(), ApiError> {
    if !matches!(severity, "critical" | "warning" | "info") {
        return Err(ApiError::bad_request(error));
    }
    Ok(())
}

fn validate_alert_token_list(values: &[String], error: &'static str) -> Result<(), ApiError> {
    if values.len() > 64 {
        return Err(ApiError::bad_request(error));
    }
    for value in values {
        validate_alert_token(value, error)?;
    }
    Ok(())
}

fn validate_short_required_value(value: &str, error: &'static str) -> Result<(), ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 128 {
        return Err(ApiError::bad_request(error));
    }
    Ok(())
}

fn validate_alert_policy_request(request: &CreateFleetAlertPolicyRequest) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "fleet_alert_policy_confirmation_required",
        ));
    }
    let name = request.name.trim();
    if name.is_empty() || name.len() > 128 {
        return Err(ApiError::bad_request("fleet_alert_policy_name_invalid"));
    }
    validate_alert_policy_scope_kind(&request.scope_kind)?;
    if request.scope_kind.trim() == "global" {
        if request
            .scope_value
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        {
            return Err(ApiError::bad_request(
                "fleet_alert_global_scope_value_invalid",
            ));
        }
    } else if request
        .scope_value
        .as_deref()
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        return Err(ApiError::bad_request(
            "fleet_alert_policy_scope_value_required",
        ));
    }
    if !has_any_threshold(request) {
        return Err(ApiError::bad_request(
            "fleet_alert_policy_threshold_required",
        ));
    }
    validate_optional_ratio(
        request.memory_available_warning_ratio,
        "fleet_alert_policy_memory_warning_invalid",
    )?;
    validate_optional_ratio(
        request.memory_available_critical_ratio,
        "fleet_alert_policy_memory_critical_invalid",
    )?;
    validate_optional_ratio(
        request.disk_available_warning_ratio,
        "fleet_alert_policy_disk_warning_invalid",
    )?;
    validate_optional_ratio(
        request.disk_available_critical_ratio,
        "fleet_alert_policy_disk_critical_invalid",
    )?;
    validate_optional_positive(
        request.cpu_load_warning,
        "fleet_alert_policy_cpu_warning_invalid",
    )?;
    validate_optional_positive(
        request.cpu_load_critical,
        "fleet_alert_policy_cpu_critical_invalid",
    )?;
    Ok(())
}

fn validate_alert_id(alert_id: &str) -> Result<(), ApiError> {
    let alert_id = alert_id.trim();
    if alert_id.is_empty() || alert_id.len() > 192 {
        return Err(ApiError::bad_request("fleet_alert_id_invalid"));
    }
    if !alert_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_' | b'.'))
    {
        return Err(ApiError::bad_request("fleet_alert_id_invalid"));
    }
    Ok(())
}

fn validate_alert_token(value: &str, code: &'static str) -> Result<(), ApiError> {
    if value.trim().is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_' | b'.'))
    {
        return Err(ApiError::bad_request(code));
    }
    Ok(())
}

fn validate_alert_state_value(state: &str, code: &'static str) -> Result<(), ApiError> {
    if matches!(
        state.trim(),
        "open" | "acknowledged" | "muted" | "escalated"
    ) {
        Ok(())
    } else {
        Err(ApiError::bad_request(code))
    }
}

fn validate_alert_policy_scope_kind(scope_kind: &str) -> Result<(), ApiError> {
    if matches!(scope_kind.trim(), "global" | "provider" | "tag" | "client") {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "fleet_alert_policy_scope_kind_invalid",
        ))
    }
}

fn has_any_threshold(request: &CreateFleetAlertPolicyRequest) -> bool {
    request.memory_available_warning_ratio.is_some()
        || request.memory_available_critical_ratio.is_some()
        || request.disk_available_warning_ratio.is_some()
        || request.disk_available_critical_ratio.is_some()
        || request.cpu_load_warning.is_some()
        || request.cpu_load_critical.is_some()
}

fn validate_optional_ratio(value: Option<f64>, code: &'static str) -> Result<(), ApiError> {
    if let Some(value) = value {
        if !value.is_finite() || !(0.0..1.0).contains(&value) {
            return Err(ApiError::bad_request(code));
        }
    }
    Ok(())
}

fn validate_optional_positive(value: Option<f64>, code: &'static str) -> Result<(), ApiError> {
    if let Some(value) = value {
        if !value.is_finite() || value <= 0.0 {
            return Err(ApiError::bad_request(code));
        }
    }
    Ok(())
}
