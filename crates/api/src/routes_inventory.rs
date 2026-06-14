use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};

use crate::{
    data_source_builtin_presets::DATA_SOURCE_DOMAINS,
    error::ApiError,
    model::{
        AgentView, AssignDataSourcePresetRequest, AssignTagRequest, BulkResolveRequest,
        BulkResolveResponse, BulkTagMutationRequest, CloneDataSourcePresetRequest,
        CreateDataSourcePresetRequest, CreateTagRequest, DataSourceHotConfigQuery,
        DataSourceHotConfigView, DataSourcePresetAssignmentQuery, DataSourcePresetAssignmentView,
        DataSourcePresetDiffRequest, DataSourcePresetDiffView, DataSourcePresetQuery,
        DataSourcePresetTestView, DataSourcePresetView, DataSourceStatusQuery,
        DataSourceStatusView, DeleteAgentRequest, DeleteAgentResponse, DeleteTagRequest,
        FleetSummary, GatewaySessionView, HistoryQuery, HotConfigRuleTemplateRenderView,
        HotConfigRuleTemplateView, RenderHotConfigRuleTemplateRequest, TagMutationResponse,
        TagView, TelemetryNetworkRateQuery, TelemetryNetworkRateView, TelemetryRollupQuery,
        TelemetryRollupView, TelemetryTunnelQuery, TelemetryTunnelView,
        TestDataSourcePresetRequest, UpdateAgentAliasRequest, UpdateDataSourcePresetRequest,
        UpdateDataSourcePresetResponse, UpsertHotConfigRuleTemplateRequest, WsEvent,
    },
    privilege::{verify_privilege_intent, DbPrivilegeIntent},
    selector_expression::parse_selector_expression,
    state::AppState,
    util::limit_or_default,
};

const MAX_PRESET_NAME_BYTES: usize = 128;
const MAX_PRESET_DESCRIPTION_BYTES: usize = 1024;
const MAX_PRESET_DEFINITION_BYTES: usize = 16 * 1024;
const MAX_PRESET_ARGV_ITEMS: usize = 32;
const MAX_PRESET_ARG_BYTES: usize = 512;
const MAX_RULE_TEMPLATE_BODY_BYTES: usize = 16 * 1024;
const TELEMETRY_NETWORK_RATE_LIMIT_MAX: i64 = 5_000;

pub(crate) async fn fleet_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<FleetSummary>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(state.repo.fleet_summary().await?))
}

pub(crate) async fn list_agents(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<AgentView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(state.repo.list_agents().await?))
}

pub(crate) async fn update_agent_alias(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<String>,
    Json(request): Json<UpdateAgentAliasRequest>,
) -> Result<Json<AgentView>, ApiError> {
    let _operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_agent_alias(&request.display_name)?;
    let agent = state
        .repo
        .update_agent_alias(&client_id, request.display_name.trim())
        .await?;
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
    validate_delete_agent_request(&request)?;
    let response = state
        .repo
        .delete_agent(&client_id, &request, &operator)
        .await
        .map_err(agent_mutation_error)?;
    state.publish(WsEvent::AgentUpdated {
        client_id,
        gateway_id: "inventory_delete".to_string(),
    });
    Ok(Json(response))
}

pub(crate) async fn list_gateway_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<GatewaySessionView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
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
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
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
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
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
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
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
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(state.repo.list_tags().await?))
}

pub(crate) async fn list_data_source_presets(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DataSourcePresetQuery>,
) -> Result<Json<Vec<DataSourcePresetView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    validate_optional_domain(query.domain.as_deref())?;
    Ok(Json(
        state
            .repo
            .list_data_source_presets(query.domain.as_deref())
            .await?,
    ))
}

pub(crate) async fn list_hot_config_rule_templates(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<HotConfigRuleTemplateView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(state.repo.list_hot_config_rule_templates().await?))
}

pub(crate) async fn upsert_hot_config_rule_template(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpsertHotConfigRuleTemplateRequest>,
) -> Result<Json<HotConfigRuleTemplateView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "config:write")
        .await?;
    validate_hot_config_rule_template(&request)?;
    Ok(Json(
        state
            .repo
            .upsert_hot_config_rule_template(&request, &operator)
            .await
            .map_err(hot_config_rule_template_error)?,
    ))
}

pub(crate) async fn render_hot_config_rule_template(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(template_id): Path<uuid::Uuid>,
    Json(request): Json<RenderHotConfigRuleTemplateRequest>,
) -> Result<Json<HotConfigRuleTemplateRenderView>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(
        state
            .repo
            .render_hot_config_rule_template(template_id, &request)
            .await
            .map_err(hot_config_rule_template_error)?,
    ))
}

pub(crate) async fn delete_hot_config_rule_template(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(template_id): Path<uuid::Uuid>,
) -> Result<StatusCode, ApiError> {
    let _operator = state
        .require_operator_role_and_scope(&headers, "operator", "config:write")
        .await?;
    state
        .repo
        .delete_hot_config_rule_template(template_id)
        .await
        .map_err(hot_config_rule_template_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn create_data_source_preset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateDataSourcePresetRequest>,
) -> Result<Json<DataSourcePresetView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_create_data_source_preset(&request)?;
    Ok(Json(
        state
            .repo
            .create_data_source_preset(&request, &operator)
            .await?,
    ))
}

pub(crate) async fn clone_data_source_preset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(preset_id): Path<uuid::Uuid>,
    Json(request): Json<CloneDataSourcePresetRequest>,
) -> Result<Json<DataSourcePresetView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_clone_data_source_preset(&request)?;
    Ok(Json(
        state
            .repo
            .clone_data_source_preset(preset_id, &request, &operator)
            .await
            .map_err(data_source_preset_lifecycle_error)?,
    ))
}

pub(crate) async fn diff_data_source_preset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(preset_id): Path<uuid::Uuid>,
    Json(request): Json<DataSourcePresetDiffRequest>,
) -> Result<Json<DataSourcePresetDiffView>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    validate_data_source_preset_candidate(&request.description, &request.definition)?;
    Ok(Json(
        state
            .repo
            .diff_data_source_preset(preset_id, &request)
            .await
            .map_err(data_source_preset_lifecycle_error)?,
    ))
}

pub(crate) async fn test_data_source_preset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(preset_id): Path<uuid::Uuid>,
    Json(request): Json<TestDataSourcePresetRequest>,
) -> Result<Json<DataSourcePresetTestView>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    validate_preset_definition(&request.definition)?;
    Ok(Json(
        state
            .repo
            .test_data_source_preset(preset_id, &request)
            .await
            .map_err(data_source_preset_lifecycle_error)?,
    ))
}

pub(crate) async fn update_data_source_preset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(preset_id): Path<uuid::Uuid>,
    Json(request): Json<UpdateDataSourcePresetRequest>,
) -> Result<Json<UpdateDataSourcePresetResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_data_source_preset_candidate(&request.description, &request.definition)?;
    Ok(Json(
        state
            .repo
            .update_data_source_preset(preset_id, &request, &operator)
            .await
            .map_err(data_source_preset_lifecycle_error)?,
    ))
}

pub(crate) async fn list_data_source_assignments(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DataSourcePresetAssignmentQuery>,
) -> Result<Json<Vec<DataSourcePresetAssignmentView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
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
            .list_data_source_assignments(query.client_id.as_deref(), query.domain.as_deref())
            .await?,
    ))
}

pub(crate) async fn list_data_source_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DataSourceStatusQuery>,
) -> Result<Json<Vec<DataSourceStatusView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    validate_optional_domain(query.domain.as_deref())?;
    if let Some(client_id) = query.client_id.as_deref() {
        validate_client_id(client_id)?;
    }
    Ok(Json(
        state
            .list_data_source_status(query.client_id.as_deref(), query.domain.as_deref())
            .await?,
    ))
}

pub(crate) async fn render_data_source_hot_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DataSourceHotConfigQuery>,
) -> Result<Json<DataSourceHotConfigView>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    validate_client_id(&query.client_id)?;
    Ok(Json(
        state
            .repo
            .render_data_source_hot_config(&query.client_id)
            .await?,
    ))
}

pub(crate) async fn assign_data_source_preset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<AssignDataSourcePresetRequest>,
) -> Result<Json<crate::model::AssignDataSourcePresetResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_assign_data_source_preset(&request)?;
    Ok(Json(
        state
            .repo
            .assign_data_source_preset(&request, &operator)
            .await?,
    ))
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
    let intent = DbPrivilegeIntent::new("tag.create", &request.name, None, &targets, true);
    verify_privilege_intent(&state, &intent, request.privilege_assertion.clone()).await?;
    Ok(Json(state.repo.create_tag(request).await?))
}

pub(crate) async fn bulk_mutate_tags(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BulkTagMutationRequest>,
) -> Result<Json<TagMutationResponse>, ApiError> {
    let _operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_persisted_tag_name(&request.tag)?;
    validate_bulk_selector_expression(&request.selector_expression)?;
    if request.confirmed {
        let resolved_targets = state
            .repo
            .resolve_bulk_targets(&BulkResolveRequest {
                selector_expression: request.selector_expression.clone(),
            })
            .await?
            .targets
            .into_iter()
            .map(|agent| agent.id)
            .collect::<Vec<_>>();
        let action = match request.action {
            crate::model::BulkTagMutationAction::Add => "tag.bulk_add",
            crate::model::BulkTagMutationAction::Remove => "tag.bulk_remove",
        };
        let intent = DbPrivilegeIntent::new(
            action,
            &request.tag,
            Some(&request.selector_expression),
            &resolved_targets,
            request.confirmed,
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
        let intent = DbPrivilegeIntent::new("tag.delete", &tag, None, &affected_targets, true);
        verify_privilege_intent(&state, &intent, request.privilege_assertion.clone()).await?;
    }
    Ok(Json(state.repo.delete_tag(&tag, request.confirmed).await?))
}

fn validate_create_data_source_preset(
    request: &CreateDataSourcePresetRequest,
) -> Result<(), ApiError> {
    validate_domain(&request.domain)?;
    validate_preset_name(&request.name)?;
    validate_data_source_preset_scope(&request.scope, request.owner_client_id.as_deref())?;
    validate_data_source_preset_candidate(&request.description, &request.definition)
}

fn validate_hot_config_rule_template(
    request: &UpsertHotConfigRuleTemplateRequest,
) -> Result<(), ApiError> {
    for value in [
        request.name.as_str(),
        request.category.as_str(),
        request.domain.as_str(),
        request.description.as_str(),
    ] {
        if value.trim().is_empty() || value.len() > 512 {
            return Err(ApiError::bad_request("hot_config_rule_template_invalid"));
        }
    }
    if request.raw_generator_body.trim().is_empty()
        || request.raw_generator_body.len() > MAX_RULE_TEMPLATE_BODY_BYTES
    {
        return Err(ApiError::bad_request(
            "hot_config_rule_template_body_invalid",
        ));
    }
    if !request.field_schema.is_object() || !request.docs_metadata.is_object() {
        return Err(ApiError::bad_request(
            "hot_config_rule_template_metadata_invalid",
        ));
    }
    Ok(())
}

fn hot_config_rule_template_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("not_found") {
        ApiError::not_found("hot_config_rule_template_not_found")
    } else if message.contains("builtin_immutable") {
        ApiError::conflict("hot_config_rule_template_builtin_immutable")
    } else {
        ApiError::from(error)
    }
}

fn validate_clone_data_source_preset(
    request: &CloneDataSourcePresetRequest,
) -> Result<(), ApiError> {
    validate_preset_name(&request.name)?;
    validate_data_source_preset_scope(&request.scope, request.owner_client_id.as_deref())?;
    if request
        .description
        .as_ref()
        .is_some_and(|description| description.len() > MAX_PRESET_DESCRIPTION_BYTES)
    {
        return Err(ApiError::bad_request(
            "data_source_preset_description_too_large",
        ));
    }
    Ok(())
}

fn validate_data_source_preset_candidate(
    description: &Option<String>,
    definition: &serde_json::Value,
) -> Result<(), ApiError> {
    if description
        .as_ref()
        .is_some_and(|description| description.len() > MAX_PRESET_DESCRIPTION_BYTES)
    {
        return Err(ApiError::bad_request(
            "data_source_preset_description_too_large",
        ));
    }
    validate_preset_definition(definition)
}

fn validate_data_source_preset_scope(
    scope: &str,
    owner_client_id: Option<&str>,
) -> Result<(), ApiError> {
    match scope {
        "shared" => {
            if owner_client_id.is_some() {
                return Err(ApiError::bad_request(
                    "shared_preset_must_not_have_owner_client",
                ));
            }
        }
        "vps_local" => {
            let Some(owner) = owner_client_id else {
                return Err(ApiError::bad_request(
                    "vps_local_preset_requires_owner_client",
                ));
            };
            if owner.is_empty() || owner.len() > 128 {
                return Err(ApiError::bad_request("invalid_owner_client_id"));
            }
        }
        "built_in" => {
            return Err(ApiError::bad_request(
                "built_in_presets_are_managed_by_server",
            ));
        }
        _ => return Err(ApiError::bad_request("invalid_data_source_preset_scope")),
    }
    Ok(())
}

fn validate_assign_data_source_preset(
    request: &AssignDataSourcePresetRequest,
) -> Result<(), ApiError> {
    validate_domain(&request.domain)?;
    if request.selector_expression.trim().is_empty() {
        return Err(ApiError::bad_request(
            "data_source_assignment_targets_required",
        ));
    }
    parse_selector_expression(&request.selector_expression)
        .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?;
    Ok(())
}

fn validate_optional_domain(domain: Option<&str>) -> Result<(), ApiError> {
    if let Some(domain) = domain {
        validate_domain(domain)?;
    }
    Ok(())
}

fn validate_domain(domain: &str) -> Result<(), ApiError> {
    if DATA_SOURCE_DOMAINS
        .iter()
        .any(|candidate| candidate == &domain)
    {
        Ok(())
    } else {
        Err(ApiError::bad_request("invalid_data_source_domain"))
    }
}

fn validate_preset_name(name: &str) -> Result<(), ApiError> {
    if name.trim().is_empty() || name.len() > MAX_PRESET_NAME_BYTES {
        return Err(ApiError::bad_request("invalid_data_source_preset_name"));
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_' | b'.'))
    {
        return Err(ApiError::bad_request("invalid_data_source_preset_name"));
    }
    Ok(())
}

fn validate_preset_definition(definition: &serde_json::Value) -> Result<(), ApiError> {
    if !definition.is_object() {
        return Err(ApiError::bad_request(
            "data_source_preset_definition_must_be_object",
        ));
    }
    let size = serde_json::to_vec(definition)
        .map_err(|_| ApiError::bad_request("invalid_data_source_preset_definition"))?
        .len();
    if size > MAX_PRESET_DEFINITION_BYTES {
        return Err(ApiError::bad_request(
            "data_source_preset_definition_too_large",
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
        return Err(ApiError::bad_request(
            "data_source_preset_argv_must_be_array",
        ));
    };
    if items.is_empty() || items.len() > MAX_PRESET_ARGV_ITEMS {
        return Err(ApiError::bad_request("data_source_preset_argv_invalid"));
    }
    for item in items {
        let Some(arg) = item.as_str() else {
            return Err(ApiError::bad_request("data_source_preset_argv_invalid"));
        };
        if arg.is_empty() || arg.len() > MAX_PRESET_ARG_BYTES {
            return Err(ApiError::bad_request("data_source_preset_argv_invalid"));
        }
    }
    let executable = items[0].as_str().unwrap_or_default();
    if !executable.starts_with('/') {
        return Err(ApiError::bad_request(
            "data_source_preset_argv_executable_must_be_absolute",
        ));
    }
    Ok(())
}

fn data_source_preset_lifecycle_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("data_source_preset_not_found") {
        ApiError::not_found("data_source_preset_not_found")
    } else if message.contains("data_source_preset_builtin_immutable")
        || message.contains("data_source_preset_clone_target_exists")
    {
        ApiError::conflict("data_source_preset_lifecycle_conflict")
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
        let intent = DbPrivilegeIntent::new("tag.assign", &request.tag, None, &targets, true);
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
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
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
    if error.to_string().contains("agent_not_found") {
        ApiError::not_found("agent_not_found")
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
