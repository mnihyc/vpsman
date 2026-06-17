use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};

use crate::{
    error::ApiError,
    model_webhook_rules::{
        CreateWebhookRuleRequest, WebhookDeliveryRotationRequest, WebhookDeliveryRotationResponse,
        WebhookRuleDeliveryQuery, WebhookRuleDeliveryView, WebhookRuleDispatchRequest,
        WebhookRuleDryRunRequest, WebhookRuleDryRunView, WebhookRuleProcessRequest,
        WebhookRuleQuery, WebhookRuleView,
    },
    repository_webhook_rules::validate_webhook_rule_target,
    security::SCOPE_INTEGRATIONS_READ,
    selector_expression::parse_selector_expression,
    state::AppState,
    util::limit_or_default,
};
use vpsman_common::{
    is_webhook_rule_delivery_history_status, is_webhook_rule_delivery_process_status,
    validate_template,
};

pub(crate) async fn list_webhook_rules(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<WebhookRuleQuery>,
) -> Result<Json<Vec<WebhookRuleView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_INTEGRATIONS_READ)
        .await?;
    validate_webhook_rule_query(&query)?;
    Ok(Json(
        state
            .repo
            .list_webhook_rules(limit_or_default(query.limit), query.enabled)
            .await?,
    ))
}

pub(crate) async fn upsert_webhook_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateWebhookRuleRequest>,
) -> Result<Json<WebhookRuleView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_webhook_rule_request(&request)?;
    Ok(Json(
        state.repo.upsert_webhook_rule(&request, &operator).await?,
    ))
}

pub(crate) async fn delete_webhook_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(rule_id): Path<uuid::Uuid>,
) -> Result<StatusCode, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    state.repo.delete_webhook_rule(rule_id, &operator).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn dry_run_webhook_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<WebhookRuleDryRunRequest>,
) -> Result<Json<WebhookRuleDryRunView>, ApiError> {
    let operator = state
        .require_operator_scope(&headers, SCOPE_INTEGRATIONS_READ)
        .await?;
    validate_webhook_rule_dry_run_request(&request)?;
    Ok(Json(state.dry_run_webhook_rule(&request, &operator).await?))
}

pub(crate) async fn dispatch_webhook_rules(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<WebhookRuleDispatchRequest>,
) -> Result<Json<Vec<WebhookRuleDeliveryView>>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_webhook_rule_dispatch_request(&request)?;
    Ok(Json(
        state.dispatch_webhook_rules(&request, &operator).await?,
    ))
}

pub(crate) async fn rotate_webhook_delivery_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<WebhookDeliveryRotationRequest>,
) -> Result<Json<WebhookDeliveryRotationResponse>, ApiError> {
    let _operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_webhook_delivery_rotation_request(&request)?;
    Ok(Json(
        state.repo.rotate_webhook_delivery_history(&request).await?,
    ))
}

pub(crate) async fn list_webhook_rule_deliveries(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<WebhookRuleDeliveryQuery>,
) -> Result<Json<Vec<WebhookRuleDeliveryView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_INTEGRATIONS_READ)
        .await?;
    validate_webhook_rule_delivery_query(&query)?;
    Ok(Json(
        state
            .repo
            .list_webhook_rule_deliveries(
                limit_or_default(query.limit),
                query.rule_id,
                query.event_kind.as_deref(),
                query.status.as_deref(),
            )
            .await?,
    ))
}

pub(crate) async fn process_webhook_rule_deliveries(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<WebhookRuleProcessRequest>,
) -> Result<Json<Vec<WebhookRuleDeliveryView>>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_webhook_rule_process_request(&request)?;
    Ok(Json(
        state
            .process_webhook_rule_deliveries(&request, &operator)
            .await?,
    ))
}

fn validate_webhook_rule_query(query: &WebhookRuleQuery) -> Result<(), ApiError> {
    if let Some(limit) = query.limit {
        if !(1..=1000).contains(&limit) {
            return Err(ApiError::bad_request("webhook_rule_limit_invalid"));
        }
    }
    Ok(())
}

fn validate_webhook_rule_request(request: &CreateWebhookRuleRequest) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::bad_request("webhook_rule_confirmation_required"));
    }
    validate_required_text(&request.name, 128, "webhook_rule_name_invalid")?;
    validate_required_text(&request.expression, 4096, "webhook_rule_expression_invalid")?;
    parse_selector_expression(&request.expression)
        .map_err(|_| ApiError::bad_request("webhook_rule_expression_invalid"))?;
    validate_webhook_rule_target(&request.target)
        .map_err(|_| ApiError::bad_request("webhook_rule_target_invalid"))?;
    if request.body_template.len() > 4096 {
        return Err(ApiError::bad_request("webhook_rule_body_template_too_long"));
    }
    if !request.body_template.trim().is_empty() {
        validate_template(&request.body_template)
            .map_err(|_| ApiError::bad_request("webhook_rule_template_invalid"))?;
    }
    if let Some(cooldown_secs) = request.cooldown_secs {
        if !(0..=30 * 24 * 60 * 60).contains(&cooldown_secs) {
            return Err(ApiError::bad_request("webhook_rule_cooldown_invalid"));
        }
    }
    if let Some(notes) = request.notes.as_deref() {
        if notes.len() > 1024 {
            return Err(ApiError::bad_request("webhook_rule_notes_too_long"));
        }
    }
    Ok(())
}

fn validate_webhook_rule_dry_run_request(
    request: &WebhookRuleDryRunRequest,
) -> Result<(), ApiError> {
    if let Some(name) = request.name.as_deref() {
        validate_required_text(name, 128, "webhook_rule_name_invalid")?;
    }
    validate_required_text(&request.expression, 4096, "webhook_rule_expression_invalid")?;
    parse_selector_expression(&request.expression)
        .map_err(|_| ApiError::bad_request("webhook_rule_expression_invalid"))?;
    if let Some(target) = request.target.as_deref() {
        validate_webhook_rule_target(target)
            .map_err(|_| ApiError::bad_request("webhook_rule_target_invalid"))?;
    }
    validate_required_text(&request.event_kind, 128, "webhook_rule_event_kind_invalid")?;
    if let Some(event_id) = request.event_id.as_deref() {
        validate_required_text(event_id, 256, "webhook_rule_event_id_invalid")?;
    }
    if request.body_template.len() > 4096 {
        return Err(ApiError::bad_request("webhook_rule_body_template_too_long"));
    }
    if !request.body_template.trim().is_empty() {
        validate_template(&request.body_template)
            .map_err(|_| ApiError::bad_request("webhook_rule_template_invalid"))?;
    }
    if let Some(cooldown_secs) = request.cooldown_secs {
        if !(0..=30 * 24 * 60 * 60).contains(&cooldown_secs) {
            return Err(ApiError::bad_request("webhook_rule_cooldown_invalid"));
        }
    }
    if let Some(notes) = request.notes.as_deref() {
        if notes.len() > 1024 {
            return Err(ApiError::bad_request("webhook_rule_notes_too_long"));
        }
    }
    Ok(())
}

fn validate_webhook_rule_dispatch_request(
    request: &WebhookRuleDispatchRequest,
) -> Result<(), ApiError> {
    if !request.dry_run.unwrap_or(false) && !request.confirmed {
        return Err(ApiError::bad_request(
            "webhook_rule_dispatch_confirmation_required",
        ));
    }
    validate_required_text(&request.event_kind, 128, "webhook_rule_event_kind_invalid")?;
    if let Some(event_id) = request.event_id.as_deref() {
        validate_required_text(event_id, 256, "webhook_rule_event_id_invalid")?;
    }
    if let Some(limit) = request.limit {
        if !(1..=1000).contains(&limit) {
            return Err(ApiError::bad_request("webhook_rule_limit_invalid"));
        }
    }
    Ok(())
}

fn validate_webhook_rule_delivery_query(query: &WebhookRuleDeliveryQuery) -> Result<(), ApiError> {
    if let Some(limit) = query.limit {
        if !(1..=1000).contains(&limit) {
            return Err(ApiError::bad_request("webhook_rule_delivery_limit_invalid"));
        }
    }
    if let Some(event_kind) = query.event_kind.as_deref() {
        validate_required_text(event_kind, 128, "webhook_rule_event_kind_invalid")?;
    }
    if let Some(status) = query.status.as_deref() {
        if !is_webhook_rule_delivery_history_status(status) {
            return Err(ApiError::bad_request(
                "webhook_rule_delivery_status_invalid",
            ));
        }
    }
    Ok(())
}

fn validate_webhook_delivery_rotation_request(
    request: &WebhookDeliveryRotationRequest,
) -> Result<(), ApiError> {
    if request.older_than.is_none() && request.older_than_days.is_none() {
        return Err(ApiError::bad_request(
            "webhook_delivery_rotation_age_required",
        ));
    }
    if request.older_than.is_some() && request.older_than_days.is_some() {
        return Err(ApiError::bad_request(
            "webhook_delivery_rotation_age_conflict",
        ));
    }
    if let Some(older_than) = request.older_than.as_deref() {
        validate_required_text(
            older_than,
            64,
            "webhook_delivery_rotation_older_than_invalid",
        )?;
        chrono::DateTime::parse_from_rfc3339(older_than)
            .map_err(|_| ApiError::bad_request("webhook_delivery_rotation_older_than_invalid"))?;
    }
    if let Some(days) = request.older_than_days {
        if !(1..=3650).contains(&days) {
            return Err(ApiError::bad_request(
                "webhook_delivery_rotation_days_invalid",
            ));
        }
    }
    if let Some(status) = request.status.as_deref() {
        if !is_webhook_rule_delivery_history_status(status) {
            return Err(ApiError::bad_request(
                "webhook_delivery_rotation_status_invalid",
            ));
        }
    }
    Ok(())
}

fn validate_webhook_rule_process_request(
    request: &WebhookRuleProcessRequest,
) -> Result<(), ApiError> {
    if !request.dry_run.unwrap_or(false) && !request.confirmed {
        return Err(ApiError::bad_request(
            "webhook_rule_delivery_process_confirmation_required",
        ));
    }
    if let Some(limit) = request.limit {
        if !(1..=200).contains(&limit) {
            return Err(ApiError::bad_request(
                "webhook_rule_delivery_process_limit_invalid",
            ));
        }
    }
    if let Some(status) = request.status.as_deref() {
        if !is_webhook_rule_delivery_process_status(status) {
            return Err(ApiError::bad_request(
                "webhook_rule_delivery_process_status_invalid",
            ));
        }
    }
    Ok(())
}

fn validate_required_text(
    value: &str,
    max_bytes: usize,
    code: &'static str,
) -> Result<(), ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > max_bytes || value.as_bytes().contains(&0) {
        return Err(ApiError::bad_request(code));
    }
    Ok(())
}
