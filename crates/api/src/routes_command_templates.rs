use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    Json,
};
use uuid::Uuid;

use crate::{
    error::ApiError,
    model_command_templates::{
        CommandTemplateQuery, CommandTemplateView, DeleteCommandTemplateRequest,
        UpsertCommandTemplateRequest,
    },
    repository_command_templates::{
        command_template_id_is_builtin, command_template_name_scope_is_builtin,
        validate_command_template_request,
    },
    security::SCOPE_TEMPLATES_READ,
    state::AppState,
    util::limit_or_default,
};

pub(crate) async fn list_command_templates(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CommandTemplateQuery>,
) -> Result<Json<Vec<CommandTemplateView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_TEMPLATES_READ)
        .await?;
    validate_template_query(&query)?;
    Ok(Json(
        state
            .repo
            .list_command_templates(
                limit_or_default(query.limit),
                query.scope_kind.as_deref(),
                query.scope_value.as_deref(),
                query.command_type.as_deref(),
                query.display_group.as_deref(),
            )
            .await?,
    ))
}

pub(crate) async fn upsert_command_template(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpsertCommandTemplateRequest>,
) -> Result<Json<CommandTemplateView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    if !request.confirmed {
        return Err(ApiError::conflict("command_template_confirmation_required"));
    }
    validate_command_template_request(&request)
        .map_err(|_| ApiError::bad_request("command_template_invalid"))?;
    if command_template_name_scope_is_builtin(
        &request.name,
        &request.scope_kind,
        request.scope_value.as_deref(),
    ) {
        return Err(ApiError::conflict("command_template_builtin_immutable"));
    }
    Ok(Json(
        state
            .repo
            .upsert_command_template(&request, &operator)
            .await?,
    ))
}

pub(crate) async fn delete_command_template(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(template_id): Path<Uuid>,
    Json(request): Json<DeleteCommandTemplateRequest>,
) -> Result<Json<CommandTemplateView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    if !request.confirmed {
        return Err(ApiError::conflict(
            "command_template_delete_confirmation_required",
        ));
    }
    if request.reviewed_name.trim().is_empty() || request.reviewed_name.len() > 128 {
        return Err(ApiError::bad_request(
            "command_template_delete_review_invalid",
        ));
    }
    if command_template_id_is_builtin(template_id) {
        return Err(ApiError::conflict("command_template_builtin_immutable"));
    }
    let existing = state
        .repo
        .list_command_templates(1000, None, None, None, None)
        .await?
        .into_iter()
        .find(|template| template.id == template_id)
        .ok_or_else(|| ApiError::not_found("command_template_not_found"))?;
    if existing.name != request.reviewed_name.trim() {
        return Err(ApiError::conflict("command_template_delete_review_stale"));
    }
    state
        .repo
        .delete_command_template(template_id, &operator)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("command_template_not_found"))
}

fn validate_template_query(query: &CommandTemplateQuery) -> Result<(), ApiError> {
    if let Some(scope_kind) = query.scope_kind.as_deref() {
        if !matches!(scope_kind, "global" | "provider" | "tag" | "client") {
            return Err(ApiError::bad_request("command_template_scope_invalid"));
        }
    }
    if query.command_type.as_deref().is_some_and(str::is_empty)
        || query.display_group.as_deref().is_some_and(str::is_empty)
    {
        return Err(ApiError::bad_request("command_template_filter_invalid"));
    }
    Ok(())
}
