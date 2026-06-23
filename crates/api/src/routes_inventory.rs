use std::collections::HashSet;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};

use crate::{
    error::ApiError,
    job_request::{fixed_target_selection, normalized_target_client_ids},
    model::{
        AgentView, AssignSourceTemplateRequest, AssignTagRequest, BulkResolveRequest,
        BulkResolveResponse, BulkTagMutationRequest, CloneSourceTemplateRequest,
        CreateSourceTemplateRequest, CreateTagRequest, DeleteAgentRequest, DeleteAgentResponse,
        DeleteRuntimeConfigPatchGeneratorRequest, DeleteTagRequest, FleetSummary,
        GatewaySessionView, HistoryQuery, RenderRuntimeConfigPatchGeneratorRequest,
        RuntimeConfigPatchGeneratorRenderView, RuntimeConfigPatchGeneratorView,
        RuntimeConfigPatchRequest, RuntimeConfigPatchResponse, SourceStatusQuery, SourceStatusView,
        SourceTemplateAssignmentQuery, SourceTemplateAssignmentView, SourceTemplateDiffRequest,
        SourceTemplateDiffView, SourceTemplateQuery, SourceTemplateTestView, SourceTemplateView,
        TagMutationResponse, TagView, TelemetryNetworkRateQuery, TelemetryNetworkRateView,
        TelemetryRollupQuery, TelemetryRollupView, TelemetryTunnelQuery, TelemetryTunnelView,
        TemplateRuntimeConfigQuery, TemplateRuntimeConfigView, TestSourceTemplateRequest,
        UpdateAgentAliasRequest, UpdateSourceTemplateRequest, UpdateSourceTemplateResponse,
        UpdateTagOrderRequest, UpsertRuntimeConfigPatchGeneratorRequest, WsEvent,
    },
    privilege::{verify_privilege_intent, DbPrivilegeIntent},
    runtime_config::{push_runtime_config_for_clients, validate_runtime_config_patch_toml},
    security::{SCOPE_CONFIG_READ, SCOPE_FLEET_READ},
    selector_expression::parse_selector_expression,
    source_template_builtins::SOURCE_TEMPLATE_DOMAINS,
    state::AppState,
    util::limit_or_default,
};
use tracing::warn;
use vpsman_common::{payload_hash, MAX_RUNTIME_CONFIG_FIELD_BYTES};

const MAX_TEMPLATE_NAME_BYTES: usize = 128;
const MAX_TEMPLATE_DESCRIPTION_BYTES: usize = 1024;
const MAX_TEMPLATE_DEFINITION_BYTES: usize = 16 * 1024;
const MAX_TEMPLATE_ARGV_ITEMS: usize = 32;
const MAX_TEMPLATE_ARG_BYTES: usize = 512;
const MAX_PATCH_GENERATOR_BODY_BYTES: usize = 16 * 1024;
const TELEMETRY_NETWORK_RATE_LIMIT_MAX: i64 = 5_000;

pub(crate) async fn fleet_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<FleetSummary>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    Ok(Json(state.repo.fleet_summary().await?))
}

pub(crate) async fn list_agents(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<AgentView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    Ok(Json(state.repo.list_agents().await?))
}

pub(crate) async fn update_agent_alias(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<String>,
    Json(request): Json<UpdateAgentAliasRequest>,
) -> Result<Json<AgentView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_agent_alias(&request.display_name)?;
    validate_agent_alias_confirmation(&request)?;
    let agent = state
        .repo
        .update_agent_alias(&client_id, request.display_name.trim(), &operator)
        .await
        .map_err(agent_mutation_error)?;
    state.publish(WsEvent::AgentUpdated {
        client_id,
        gateway_id: "inventory_alias".to_string(),
    });
    Ok(Json(agent))
}

pub(crate) async fn delete_agent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<String>,
    Json(request): Json<DeleteAgentRequest>,
) -> Result<Json<DeleteAgentResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_client_id(&client_id)?;
    validate_delete_agent_request(&request)?;
    let targets = vec![client_id.clone()];
    let intent = DbPrivilegeIntent::new("agent.delete", &client_id, None, &targets, true, None);
    verify_privilege_intent(&state, &intent, request.privilege_assertion.clone()).await?;
    let response = state
        .repo
        .delete_agent(&client_id, &request, &operator)
        .await
        .map_err(agent_mutation_error)?;
    if let Err(error) = state
        .disconnect_gateway_session_for_lifecycle(&client_id, "vps_deleted")
        .await
    {
        warn!(
            ?error,
            client_id, "post-commit gateway disconnect failed after agent delete"
        );
    }
    state.publish(WsEvent::AgentUpdated {
        client_id,
        gateway_id: "inventory_delete".to_string(),
    });
    state.process_job_terminal_events(500).await?;
    Ok(Json(response))
}

pub(crate) async fn list_gateway_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<GatewaySessionView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    Ok(Json(
        state
            .repo
            .list_gateway_sessions(limit_or_default(query.limit))
            .await?,
    ))
}

pub(crate) async fn list_telemetry_rollups(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TelemetryRollupQuery>,
) -> Result<Json<Vec<TelemetryRollupView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    validate_telemetry_rollup_query(&query)?;
    Ok(Json(
        state
            .repo
            .list_telemetry_rollups(
                limit_or_default(query.limit),
                query.client_id.as_deref(),
                query.bucket_secs,
            )
            .await?,
    ))
}

pub(crate) async fn list_telemetry_network_rates(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TelemetryNetworkRateQuery>,
) -> Result<Json<Vec<TelemetryNetworkRateView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    validate_telemetry_network_rate_query(&query)?;
    Ok(Json(
        state
            .repo
            .list_telemetry_network_rates(
                telemetry_network_rate_limit_or_default(query.limit),
                query.client_id.as_deref(),
                query.interface.as_deref(),
                query.bucket_secs,
            )
            .await?,
    ))
}

pub(crate) async fn list_telemetry_tunnels(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TelemetryTunnelQuery>,
) -> Result<Json<Vec<TelemetryTunnelView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    validate_telemetry_tunnel_query(&query)?;
    Ok(Json(
        state
            .repo
            .list_telemetry_tunnels(
                limit_or_default(query.limit),
                query.client_id.as_deref(),
                query.interface.as_deref(),
            )
            .await?,
    ))
}

pub(crate) async fn list_tags(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<TagView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    Ok(Json(state.repo.list_tags().await?))
}

pub(crate) async fn update_tag_order(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpdateTagOrderRequest>,
) -> Result<Json<Vec<TagView>>, ApiError> {
    let _operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_tag_order_request(&request, &state.repo.list_tags().await?)?;
    Ok(Json(
        state
            .repo
            .update_tag_order(&request)
            .await
            .map_err(tag_order_error)?,
    ))
}

pub(crate) async fn list_source_templates(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SourceTemplateQuery>,
) -> Result<Json<Vec<SourceTemplateView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_CONFIG_READ)
        .await?;
    validate_optional_domain(query.domain.as_deref())?;
    Ok(Json(
        state
            .repo
            .list_source_templates(query.domain.as_deref())
            .await?,
    ))
}

pub(crate) async fn list_runtime_config_patch_generators(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<RuntimeConfigPatchGeneratorView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_CONFIG_READ)
        .await?;
    Ok(Json(
        state.repo.list_runtime_config_patch_generators().await?,
    ))
}

pub(crate) async fn upsert_runtime_config_patch_generator(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpsertRuntimeConfigPatchGeneratorRequest>,
) -> Result<Json<RuntimeConfigPatchGeneratorView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "config:write")
        .await?;
    validate_runtime_config_patch_generator(&request)?;
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "runtime_config_patch_generator_confirmation_required",
        ));
    }
    Ok(Json(
        state
            .repo
            .upsert_runtime_config_patch_generator(&request, &operator)
            .await
            .map_err(runtime_config_patch_generator_error)?,
    ))
}

pub(crate) async fn render_runtime_config_patch_generator(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(generator_id): Path<uuid::Uuid>,
    Json(request): Json<RenderRuntimeConfigPatchGeneratorRequest>,
) -> Result<Json<RuntimeConfigPatchGeneratorRenderView>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_CONFIG_READ)
        .await?;
    Ok(Json(
        state
            .repo
            .render_runtime_config_patch_generator(generator_id, &request)
            .await
            .map_err(runtime_config_patch_generator_error)?,
    ))
}

pub(crate) async fn delete_runtime_config_patch_generator(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(generator_id): Path<uuid::Uuid>,
    Json(request): Json<DeleteRuntimeConfigPatchGeneratorRequest>,
) -> Result<StatusCode, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "config:write")
        .await?;
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "runtime_config_patch_generator_delete_confirmation_required",
        ));
    }
    validate_short_required_value(
        &request.reviewed_name,
        "runtime_config_patch_generator_delete_review_invalid",
    )?;
    let existing = state
        .repo
        .list_runtime_config_patch_generators()
        .await?
        .into_iter()
        .find(|generator| generator.id == generator_id)
        .ok_or_else(|| ApiError::not_found("runtime_config_patch_generator_not_found"))?;
    if existing.name != request.reviewed_name.trim() {
        return Err(ApiError::conflict(
            "runtime_config_patch_generator_delete_review_stale",
        ));
    }
    state
        .repo
        .delete_runtime_config_patch_generator(generator_id, &operator)
        .await
        .map_err(runtime_config_patch_generator_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn create_source_template(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateSourceTemplateRequest>,
) -> Result<Json<SourceTemplateView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "config:write")
        .await?;
    validate_create_source_template(&request)?;
    Ok(Json(
        state
            .repo
            .create_source_template(&request, &operator)
            .await
            .map_err(source_template_lifecycle_error)?,
    ))
}

pub(crate) async fn clone_source_template(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(template_id): Path<uuid::Uuid>,
    Json(request): Json<CloneSourceTemplateRequest>,
) -> Result<Json<SourceTemplateView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "config:write")
        .await?;
    validate_clone_source_template(&request)?;
    Ok(Json(
        state
            .repo
            .clone_source_template(template_id, &request, &operator)
            .await
            .map_err(source_template_lifecycle_error)?,
    ))
}

pub(crate) async fn diff_source_template(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(template_id): Path<uuid::Uuid>,
    Json(request): Json<SourceTemplateDiffRequest>,
) -> Result<Json<SourceTemplateDiffView>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_CONFIG_READ)
        .await?;
    validate_source_template_candidate(&request.description, &request.definition)?;
    Ok(Json(
        state
            .repo
            .diff_source_template(template_id, &request)
            .await
            .map_err(source_template_lifecycle_error)?,
    ))
}

pub(crate) async fn test_source_template(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(template_id): Path<uuid::Uuid>,
    Json(request): Json<TestSourceTemplateRequest>,
) -> Result<Json<SourceTemplateTestView>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_CONFIG_READ)
        .await?;
    validate_template_definition(&request.definition)?;
    Ok(Json(
        state
            .repo
            .test_source_template(template_id, &request)
            .await
            .map_err(source_template_lifecycle_error)?,
    ))
}

pub(crate) async fn update_source_template(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(template_id): Path<uuid::Uuid>,
    Json(request): Json<UpdateSourceTemplateRequest>,
) -> Result<Json<UpdateSourceTemplateResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "config:write")
        .await?;
    validate_source_template_candidate(&request.description, &request.definition)?;
    let response = state
        .repo
        .update_source_template(template_id, &request, &operator)
        .await
        .map_err(source_template_lifecycle_error)?;
    if !response.confirmation_required && response.affected_client_count > 0 {
        let assignments = state
            .repo
            .list_source_template_assignments(None, Some(&response.template.domain))
            .await?;
        let affected = assignments
            .into_iter()
            .filter(|assignment| assignment.template_id == response.template.id)
            .map(|assignment| assignment.client_id)
            .collect::<Vec<_>>();
        let _sync_jobs =
            push_runtime_config_for_clients(&state, &operator, affected, "source_template_updated")
                .await?;
    }
    Ok(Json(response))
}

pub(crate) async fn list_source_template_assignments(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SourceTemplateAssignmentQuery>,
) -> Result<Json<Vec<SourceTemplateAssignmentView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_CONFIG_READ)
        .await?;
    validate_optional_domain(query.domain.as_deref())?;
    if query
        .client_id
        .as_ref()
        .is_some_and(|client_id| client_id.is_empty() || client_id.len() > 128)
    {
        return Err(ApiError::bad_request("invalid_client_id"));
    }
    Ok(Json(
        state
            .repo
            .list_source_template_assignments(query.client_id.as_deref(), query.domain.as_deref())
            .await?,
    ))
}

pub(crate) async fn list_source_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SourceStatusQuery>,
) -> Result<Json<Vec<SourceStatusView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    validate_optional_domain(query.domain.as_deref())?;
    if let Some(client_id) = query.client_id.as_deref() {
        validate_client_id(client_id)?;
    }
    Ok(Json(
        state
            .list_source_status(query.client_id.as_deref(), query.domain.as_deref())
            .await?,
    ))
}

pub(crate) async fn render_template_runtime_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TemplateRuntimeConfigQuery>,
) -> Result<Json<TemplateRuntimeConfigView>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_CONFIG_READ)
        .await?;
    validate_client_id(&query.client_id)?;
    Ok(Json(
        state
            .repo
            .render_template_runtime_config(&query.client_id)
            .await?,
    ))
}

pub(crate) async fn create_server_runtime_config_patch_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut request): Json<RuntimeConfigPatchRequest>,
) -> Result<Json<RuntimeConfigPatchResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "config:write")
        .await?;
    validate_server_runtime_config_patch_request(&request)?;
    let target_client_ids = if request.selector_expression.trim().is_empty() {
        verified_fixed_target_ids(
            &state,
            &request.target_client_ids,
            "runtime_config_patch_targets_not_found",
        )
        .await?
    } else {
        parse_selector_expression(&request.selector_expression)
            .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?;
        state
            .repo
            .resolve_bulk_targets(&BulkResolveRequest {
                selector_expression: request.selector_expression.trim().to_string(),
            })
            .await?
            .targets
            .into_iter()
            .map(|agent| agent.id)
            .collect::<Vec<_>>()
    };
    if target_client_ids.is_empty() {
        return Err(ApiError::bad_request(
            "runtime_config_patch_targets_required",
        ));
    }
    request.target_client_ids = target_client_ids;
    let patch_hash = payload_hash(request.toml.as_bytes());
    let selector_expression = request.selector_expression.trim().to_string();
    let selector_for_intent =
        (!selector_expression.is_empty()).then_some(selector_expression.as_str());
    let intent = DbPrivilegeIntent::new(
        "runtime_config.patch",
        "runtime_config",
        selector_for_intent,
        &request.target_client_ids,
        true,
        Some(&patch_hash),
    );
    verify_privilege_intent(&state, &intent, request.privilege_assertion.clone()).await?;
    let reason = request
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("operator_bulk_runtime_config_patch")
        .to_string();
    let overrides = state
        .repo
        .upsert_runtime_config_overrides(
            &request.target_client_ids,
            &request.toml,
            &reason,
            &operator,
        )
        .await?;
    let sync_jobs = push_runtime_config_for_clients(
        &state,
        &operator,
        request.target_client_ids.clone(),
        &reason,
    )
    .await?;
    Ok(Json(RuntimeConfigPatchResponse {
        target_count: request.target_client_ids.len(),
        overrides,
        sync_job_ids: sync_jobs.into_iter().map(|job| job.job_id).collect(),
    }))
}

pub(crate) async fn assign_source_template(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut request): Json<AssignSourceTemplateRequest>,
) -> Result<Json<crate::model::AssignSourceTemplateResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "config:write")
        .await?;
    validate_assign_source_template(&request)?;
    request.target_client_ids = normalized_target_client_ids(&request.target_client_ids)?;
    verified_fixed_target_ids(
        &state,
        &request.target_client_ids,
        "source_template_assignment_targets_not_found",
    )
    .await?;
    let response = state
        .repo
        .assign_source_template(&request, &operator)
        .await?;
    if !response.confirmation_required && response.target_count > 0 {
        let affected = response
            .assignments
            .iter()
            .map(|assignment| assignment.client_id.clone())
            .collect::<Vec<_>>();
        let _sync_jobs = push_runtime_config_for_clients(
            &state,
            &operator,
            affected,
            "source_template_assigned",
        )
        .await?;
    }
    Ok(Json(response))
}

fn validate_client_id(client_id: &str) -> Result<(), ApiError> {
    if client_id.is_empty() || client_id.len() > 128 {
        return Err(ApiError::bad_request("invalid_client_id"));
    }
    Ok(())
}

pub(crate) async fn create_tag(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateTagRequest>,
) -> Result<Json<TagView>, ApiError> {
    let _operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    if !request.confirmed {
        return Err(ApiError::conflict("tag_mutation_confirmation_required"));
    }
    validate_persisted_tag_name(&request.name)?;
    let targets = Vec::<String>::new();
    let intent = DbPrivilegeIntent::new("tag.create", &request.name, None, &targets, true, None);
    verify_privilege_intent(&state, &intent, request.privilege_assertion.clone()).await?;
    Ok(Json(state.repo.create_tag(request).await?))
}

pub(crate) async fn bulk_mutate_tags(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut request): Json<BulkTagMutationRequest>,
) -> Result<Json<TagMutationResponse>, ApiError> {
    let _operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_persisted_tag_name(&request.tag)?;
    validate_bulk_selector_expression(&request.selector_expression)?;
    request.target_client_ids = normalized_target_client_ids(&request.target_client_ids)?;
    let fixed_targets = verified_fixed_target_ids(
        &state,
        &request.target_client_ids,
        "tag_fixed_targets_not_found",
    )
    .await?;
    if request.confirmed {
        let action = match request.action {
            crate::model::BulkTagMutationAction::Add => "tag.bulk_add",
            crate::model::BulkTagMutationAction::Remove => "tag.bulk_remove",
        };
        let intent = DbPrivilegeIntent::new(
            action,
            &request.tag,
            Some(&request.selector_expression),
            &fixed_targets,
            request.confirmed,
            None,
        );
        verify_privilege_intent(&state, &intent, request.privilege_assertion.clone()).await?;
    }
    Ok(Json(state.repo.bulk_mutate_tags(&request).await?))
}

pub(crate) async fn delete_tag(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(tag): Path<String>,
    Json(request): Json<DeleteTagRequest>,
) -> Result<Json<TagMutationResponse>, ApiError> {
    let _operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_persisted_tag_name(&tag)?;
    if request.confirmed {
        let affected_targets = state
            .repo
            .list_tags()
            .await?
            .into_iter()
            .find(|candidate| candidate.name == tag)
            .map(|tag| {
                tag.clients
                    .into_iter()
                    .map(|client| client.id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let intent =
            DbPrivilegeIntent::new("tag.delete", &tag, None, &affected_targets, true, None);
        verify_privilege_intent(&state, &intent, request.privilege_assertion.clone()).await?;
    }
    Ok(Json(state.repo.delete_tag(&tag, request.confirmed).await?))
}

fn validate_create_source_template(request: &CreateSourceTemplateRequest) -> Result<(), ApiError> {
    validate_domain(&request.domain)?;
    validate_template_name(&request.name)?;
    validate_source_template_scope(&request.scope, request.owner_client_id.as_deref())?;
    validate_source_template_candidate(&request.description, &request.definition)
}

fn validate_runtime_config_patch_generator(
    request: &UpsertRuntimeConfigPatchGeneratorRequest,
) -> Result<(), ApiError> {
    for value in [
        request.name.as_str(),
        request.category.as_str(),
        request.domain.as_str(),
        request.description.as_str(),
    ] {
        if value.trim().is_empty() || value.len() > MAX_RUNTIME_CONFIG_FIELD_BYTES {
            return Err(ApiError::bad_request(
                "runtime_config_patch_generator_invalid",
            ));
        }
    }
    if request.raw_generator_body.trim().is_empty()
        || request.raw_generator_body.len() > MAX_PATCH_GENERATOR_BODY_BYTES
    {
        return Err(ApiError::bad_request(
            "runtime_config_patch_generator_body_invalid",
        ));
    }
    if !request.field_schema.is_object() || !request.docs_metadata.is_object() {
        return Err(ApiError::bad_request(
            "runtime_config_patch_generator_metadata_invalid",
        ));
    }
    Ok(())
}

fn runtime_config_patch_generator_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("not_found") {
        ApiError::not_found("runtime_config_patch_generator_not_found")
    } else if message.contains("runtime_config_patch_generator_builtin_immutable") {
        ApiError::conflict("runtime_config_patch_generator_builtin_immutable")
    } else {
        ApiError::from(error)
    }
}

fn validate_short_required_value(value: &str, error: &'static str) -> Result<(), ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 128 {
        return Err(ApiError::bad_request(error));
    }
    Ok(())
}

fn validate_clone_source_template(request: &CloneSourceTemplateRequest) -> Result<(), ApiError> {
    validate_template_name(&request.name)?;
    validate_source_template_scope(&request.scope, request.owner_client_id.as_deref())?;
    if request
        .description
        .as_ref()
        .is_some_and(|description| description.len() > MAX_TEMPLATE_DESCRIPTION_BYTES)
    {
        return Err(ApiError::bad_request(
            "source_template_description_too_large",
        ));
    }
    Ok(())
}

fn validate_source_template_candidate(
    description: &Option<String>,
    definition: &serde_json::Value,
) -> Result<(), ApiError> {
    if description
        .as_ref()
        .is_some_and(|description| description.len() > MAX_TEMPLATE_DESCRIPTION_BYTES)
    {
        return Err(ApiError::bad_request(
            "source_template_description_too_large",
        ));
    }
    validate_template_definition(definition)
}

fn validate_source_template_scope(
    scope: &str,
    owner_client_id: Option<&str>,
) -> Result<(), ApiError> {
    match scope {
        "shared" => {
            if owner_client_id.is_some() {
                return Err(ApiError::bad_request(
                    "shared_template_must_not_have_owner_client",
                ));
            }
        }
        "vps_local" => {
            let Some(owner) = owner_client_id else {
                return Err(ApiError::bad_request(
                    "vps_local_template_requires_owner_client",
                ));
            };
            if owner.is_empty() || owner.len() > 128 {
                return Err(ApiError::bad_request("invalid_owner_client_id"));
            }
        }
        "built_in" => {
            return Err(ApiError::bad_request(
                "built_in_templates_are_managed_by_server",
            ));
        }
        _ => return Err(ApiError::bad_request("invalid_source_template_scope")),
    }
    Ok(())
}

fn validate_assign_source_template(request: &AssignSourceTemplateRequest) -> Result<(), ApiError> {
    validate_domain(&request.domain)?;
    if request.selector_expression.trim().is_empty() {
        return Err(ApiError::bad_request(
            "source_template_assignment_targets_required",
        ));
    }
    parse_selector_expression(&request.selector_expression)
        .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?;
    Ok(())
}

fn validate_server_runtime_config_patch_request(
    request: &RuntimeConfigPatchRequest,
) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::conflict(
            "runtime_config_patch_confirmation_required",
        ));
    }
    if request.selector_expression.trim().is_empty() && request.target_client_ids.is_empty() {
        return Err(ApiError::bad_request(
            "runtime_config_patch_targets_required",
        ));
    }
    if !request.selector_expression.trim().is_empty() {
        parse_selector_expression(&request.selector_expression)
            .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?;
    }
    if request.toml.trim().is_empty()
        || request.toml.len() > vpsman_common::MAX_RUNTIME_CONFIG_PATCH_BYTES
    {
        return Err(ApiError::bad_request("runtime_config_patch_toml_invalid"));
    }
    validate_runtime_config_patch_toml(&request.toml)
        .map_err(runtime_config_patch_validation_error)?;
    if let Some(reason) = request.reason.as_deref() {
        if reason.len() > vpsman_common::MAX_RUNTIME_CONFIG_REASON_BYTES
            || reason.chars().any(char::is_control)
        {
            return Err(ApiError::bad_request("runtime_config_patch_reason_invalid"));
        }
    }
    Ok(())
}

fn runtime_config_patch_validation_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("runtime_config_patch_bootstrap_field_forbidden") {
        ApiError::bad_request("runtime_config_patch_bootstrap_field_forbidden")
    } else if message.contains("runtime_config_patch_managed_tunnel_plans_forbidden") {
        ApiError::bad_request("runtime_config_patch_managed_tunnel_plans_forbidden")
    } else if message.contains("runtime_config_patch_toml_invalid")
        || message.contains("failed to parse runtime config patch TOML")
    {
        ApiError::bad_request("runtime_config_patch_toml_invalid")
    } else {
        ApiError::bad_request("runtime_config_patch_invalid")
    }
}

async fn verified_fixed_target_ids(
    state: &AppState,
    target_client_ids: &[String],
    error_code: &'static str,
) -> Result<Vec<String>, ApiError> {
    let target_client_ids = normalized_target_client_ids(target_client_ids)?;
    let resolved = state
        .repo
        .resolve_bulk_targets(&fixed_target_selection(&target_client_ids)?)
        .await?
        .targets
        .into_iter()
        .map(|agent| agent.id)
        .collect::<Vec<_>>();
    let missing = target_client_ids
        .iter()
        .filter(|client_id| !resolved.iter().any(|resolved_id| resolved_id == *client_id))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(ApiError::conflict(error_code));
    }
    Ok(target_client_ids)
}

fn validate_optional_domain(domain: Option<&str>) -> Result<(), ApiError> {
    if let Some(domain) = domain {
        validate_domain(domain)?;
    }
    Ok(())
}

fn validate_domain(domain: &str) -> Result<(), ApiError> {
    if SOURCE_TEMPLATE_DOMAINS
        .iter()
        .any(|candidate| candidate == &domain)
    {
        Ok(())
    } else {
        Err(ApiError::bad_request("invalid_source_template_domain"))
    }
}

fn validate_template_name(name: &str) -> Result<(), ApiError> {
    if name.trim().is_empty() || name.len() > MAX_TEMPLATE_NAME_BYTES {
        return Err(ApiError::bad_request("invalid_source_template_name"));
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_' | b'.'))
    {
        return Err(ApiError::bad_request("invalid_source_template_name"));
    }
    Ok(())
}

fn validate_template_definition(definition: &serde_json::Value) -> Result<(), ApiError> {
    if !definition.is_object() {
        return Err(ApiError::bad_request(
            "source_template_definition_must_be_object",
        ));
    }
    let size = serde_json::to_vec(definition)
        .map_err(|_| ApiError::bad_request("invalid_source_template_definition"))?
        .len();
    if size > MAX_TEMPLATE_DEFINITION_BYTES {
        return Err(ApiError::bad_request(
            "source_template_definition_too_large",
        ));
    }
    validate_argv_fields(definition)
}

fn validate_argv_fields(value: &serde_json::Value) -> Result<(), ApiError> {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(argv) = map.get("argv") {
                validate_argv_array(argv)?;
            }
            for child in map.values() {
                validate_argv_fields(child)?;
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                validate_argv_fields(child)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_argv_array(value: &serde_json::Value) -> Result<(), ApiError> {
    let Some(items) = value.as_array() else {
        return Err(ApiError::bad_request("source_template_argv_must_be_array"));
    };
    if items.is_empty() || items.len() > MAX_TEMPLATE_ARGV_ITEMS {
        return Err(ApiError::bad_request("source_template_argv_invalid"));
    }
    for item in items {
        let Some(arg) = item.as_str() else {
            return Err(ApiError::bad_request("source_template_argv_invalid"));
        };
        if arg.is_empty() || arg.len() > MAX_TEMPLATE_ARG_BYTES {
            return Err(ApiError::bad_request("source_template_argv_invalid"));
        }
    }
    let executable = items[0].as_str().unwrap_or_default();
    if !executable.starts_with('/') {
        return Err(ApiError::bad_request(
            "source_template_argv_executable_must_be_absolute",
        ));
    }
    Ok(())
}

fn source_template_lifecycle_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("source_template_not_found") {
        ApiError::not_found("source_template_not_found")
    } else if message.contains("source_template_builtin_immutable")
        || message.contains("source_template_clone_target_exists")
        || message.contains("source_template_duplicate")
    {
        ApiError::conflict("source_template_lifecycle_conflict")
    } else {
        ApiError::from(error)
    }
}

pub(crate) async fn assign_agent_tag(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<String>,
    Json(request): Json<AssignTagRequest>,
) -> Result<Json<TagMutationResponse>, ApiError> {
    let _operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_persisted_tag_name(&request.tag)?;
    if request.confirmed {
        let targets = vec![client_id.clone()];
        let intent = DbPrivilegeIntent::new("tag.assign", &request.tag, None, &targets, true, None);
        verify_privilege_intent(&state, &intent, request.privilege_assertion.clone()).await?;
    }
    Ok(Json(
        state
            .repo
            .assign_agent_tag_mutation(&client_id, &request.tag, request.confirmed)
            .await?,
    ))
}

pub(crate) async fn resolve_bulk_targets(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BulkResolveRequest>,
) -> Result<Json<BulkResolveResponse>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    validate_bulk_selector_expression(&request.selector_expression)?;
    Ok(Json(state.repo.resolve_bulk_targets(&request).await?))
}

fn validate_bulk_selector_expression(selector_expression: &str) -> Result<(), ApiError> {
    if selector_expression.trim().is_empty() {
        return Err(ApiError::bad_request("selector_expression_required"));
    }
    parse_selector_expression(selector_expression)
        .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?;
    Ok(())
}

fn validate_telemetry_rollup_query(query: &TelemetryRollupQuery) -> Result<(), ApiError> {
    if query
        .client_id
        .as_ref()
        .is_some_and(|client_id| client_id.is_empty() || client_id.len() > 128)
    {
        return Err(ApiError::bad_request("invalid_client_id"));
    }
    if query
        .bucket_secs
        .is_some_and(|bucket_secs| bucket_secs != 60)
    {
        return Err(ApiError::bad_request("invalid_bucket_secs"));
    }
    Ok(())
}

fn validate_persisted_tag_name(tag: &str) -> Result<(), ApiError> {
    if tag.is_empty() || tag.len() > 128 {
        return Err(ApiError::bad_request("invalid_tag_name"));
    }
    if tag.starts_with("id:") || tag.starts_with("name:") {
        return Err(ApiError::bad_request("reserved_inner_tag_selector"));
    }
    if !tag
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(ApiError::bad_request("invalid_tag_name"));
    }
    Ok(())
}

fn validate_tag_order_request(
    request: &UpdateTagOrderRequest,
    current: &[TagView],
) -> Result<(), ApiError> {
    if request.ordered_tags.len() > 1000 {
        return Err(ApiError::bad_request("too_many_ordered_tags"));
    }
    let current_names = current
        .iter()
        .map(|tag| tag.name.as_str())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    for tag in &request.ordered_tags {
        validate_persisted_tag_name(tag)?;
        if !current_names.contains(tag.as_str()) {
            return Err(ApiError::bad_request("unknown_tag"));
        }
        if !seen.insert(tag.as_str()) {
            return Err(ApiError::bad_request("duplicate_tag"));
        }
    }
    Ok(())
}

fn tag_order_error(error: anyhow::Error) -> ApiError {
    match error.to_string().as_str() {
        "unknown_tag" => ApiError::bad_request("unknown_tag"),
        "duplicate_tag" => ApiError::bad_request("duplicate_tag"),
        _ => error.into(),
    }
}

fn validate_agent_alias(display_name: &str) -> Result<(), ApiError> {
    let display_name = display_name.trim();
    if display_name.is_empty()
        || display_name.len() > 160
        || display_name.chars().any(|character| character.is_control())
    {
        return Err(ApiError::bad_request("agent_alias_invalid"));
    }
    Ok(())
}

fn validate_agent_alias_confirmation(request: &UpdateAgentAliasRequest) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::conflict("agent_alias_confirmation_required"));
    }
    Ok(())
}

fn validate_delete_agent_request(request: &DeleteAgentRequest) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::conflict("agent_delete_confirmation_required"));
    }
    if request
        .reason
        .as_deref()
        .is_some_and(|reason| reason.trim().len() > 240 || reason.chars().any(char::is_control))
    {
        return Err(ApiError::bad_request("agent_delete_reason_invalid"));
    }
    Ok(())
}

fn agent_mutation_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("agent_not_found") {
        ApiError::not_found("agent_not_found")
    } else if message.contains("display_name_already_exists")
        || message.contains("clients_visible_display_name_key_idx")
    {
        ApiError::conflict("display_name_already_exists")
    } else {
        ApiError::from(error)
    }
}

fn validate_telemetry_network_rate_query(
    query: &TelemetryNetworkRateQuery,
) -> Result<(), ApiError> {
    if query
        .client_id
        .as_ref()
        .is_some_and(|client_id| client_id.is_empty() || client_id.len() > 128)
    {
        return Err(ApiError::bad_request("invalid_client_id"));
    }
    if query
        .interface
        .as_ref()
        .is_some_and(|interface| interface.is_empty() || interface.len() > 64)
    {
        return Err(ApiError::bad_request("invalid_network_interface"));
    }
    if query
        .bucket_secs
        .is_some_and(|bucket_secs| bucket_secs != 60)
    {
        return Err(ApiError::bad_request("invalid_bucket_secs"));
    }
    Ok(())
}

fn telemetry_network_rate_limit_or_default(limit: Option<i64>) -> i64 {
    limit
        .unwrap_or(100)
        .clamp(1, TELEMETRY_NETWORK_RATE_LIMIT_MAX)
}

fn validate_telemetry_tunnel_query(query: &TelemetryTunnelQuery) -> Result<(), ApiError> {
    if query
        .client_id
        .as_ref()
        .is_some_and(|client_id| client_id.is_empty() || client_id.len() > 128)
    {
        return Err(ApiError::bad_request("invalid_client_id"));
    }
    if query
        .interface
        .as_ref()
        .is_some_and(|interface| interface.is_empty() || interface.len() > 64)
    {
        return Err(ApiError::bad_request("invalid_tunnel_interface"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{telemetry_network_rate_limit_or_default, validate_persisted_tag_name};

    #[test]
    fn persisted_tags_reject_inner_selector_prefixes() {
        validate_persisted_tag_name("provider:alpha").unwrap();
        validate_persisted_tag_name("country:US").unwrap();
        validate_persisted_tag_name("region:legacy-name").unwrap();

        assert!(validate_persisted_tag_name("id:edge-a").is_err());
        assert!(validate_persisted_tag_name("name:edge-a").is_err());
    }

    #[test]
    fn telemetry_network_rates_allow_fleet_scale_limits() {
        assert_eq!(telemetry_network_rate_limit_or_default(None), 100);
        assert_eq!(telemetry_network_rate_limit_or_default(Some(5_000)), 5_000);
        assert_eq!(telemetry_network_rate_limit_or_default(Some(50_000)), 5_000);
    }
}
