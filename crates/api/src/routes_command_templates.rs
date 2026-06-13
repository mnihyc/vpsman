use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};

use crate::{
    error::ApiError,
    model_command_templates::{
        CommandTemplateQuery, CommandTemplateView, UpsertCommandTemplateRequest,
    },
    repository_command_templates::validate_command_template_request,
    state::AppState,
    util::limit_or_default,
};

pub(crate) async fn list_command_templates(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CommandTemplateQuery>,
) -> Result<Json<Vec<CommandTemplateView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
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
    Ok(Json(
        state
            .repo
            .upsert_command_template(&request, &operator)
            .await?,
    ))
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
