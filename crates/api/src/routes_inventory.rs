use std::collections::{BTreeSet, HashSet};

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};

use crate::{
    error::ApiError,
    job_request::{fixed_target_selection, normalized_target_client_ids},
    lifecycle_outcome::{gateway_disconnect_outcome, terminal_reconciliation_outcome},
    model::{
        AgentView, AssignTagRequest, BulkResolveRequest, BulkResolveResponse,
        BulkTagMutationRequest, CreateTagRequest, DeleteAgentRequest, DeleteAgentResponse,
        DeleteRuntimeConfigPatchGeneratorRequest, DeleteTagRequest, FleetSummary,
        GatewaySessionView, HistoryQuery, RenderRuntimeConfigPatchGeneratorRequest,
        RuntimeConfigApplyStateView, RuntimeConfigPatchGeneratorRenderView,
        RuntimeConfigPatchGeneratorView, RuntimeConfigPatchRequest, RuntimeConfigPatchResponse,
        TagMutationResponse, TagView, TelemetryNetworkRateQuery, TelemetryNetworkRateView,
        TelemetryRollupQuery, TelemetryRollupView, TelemetrySampleQuery, TelemetrySampleView,
        TelemetryTunnelQuery, TelemetryTunnelView, UpdateAgentAliasRequest, UpdateTagOrderRequest,
        UpsertRuntimeConfigPatchGeneratorRequest, WsEvent,
    },
    privilege::{verify_privilege_intent, DbPrivilegeIntent},
    runtime_config::{dispatch_runtime_config_for_clients, validate_runtime_config_patch_toml},
    security::{SCOPE_CONFIG_READ, SCOPE_FLEET_READ},
    selector_expression::parse_selector_expression,
    state::AppState,
    util::limit_or_default,
};
use vpsman_common::{payload_hash, MAX_RUNTIME_CONFIG_FIELD_BYTES};

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
    let deleted = state
        .repo
        .delete_agent(&client_id, request.reason.as_deref(), &operator)
        .await
        .map_err(agent_mutation_error)?;
    let tunnel_peer_client_ids =
        peer_client_ids_for_deleted_agent(&client_id, deleted.retired_tunnel_endpoint_pairs);
    let gateway_disconnect = gateway_disconnect_outcome(
        state
            .disconnect_gateway_session_for_lifecycle(&client_id, "vps_deleted")
            .await,
        &client_id,
        "VPS deletion",
    );
    let runtime_sync = dispatch_runtime_config_for_clients(
        &state,
        &operator,
        tunnel_peer_client_ids,
        "agent_deleted_tunnel_peer_cleanup",
    )
    .await;
    state.publish(WsEvent::AgentUpdated {
        client_id: client_id.clone(),
        gateway_id: "inventory_delete".to_string(),
    });
    let terminal_reconciliation = terminal_reconciliation_outcome(
        state.process_job_terminal_events(500).await,
        "VPS deletion",
    );
    Ok(Json(DeleteAgentResponse {
        client_id: deleted.client_id,
        deleted: true,
        deleted_at: deleted.deleted_at,
        post_commit: vec![gateway_disconnect, terminal_reconciliation],
        runtime_sync,
    }))
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
    let rows = if query.latest {
        state
            .repo
            .list_latest_telemetry_rollups(
                limit_or_default(query.limit),
                query.client_id.as_deref(),
                query.bucket_secs,
            )
            .await?
    } else {
        state
            .repo
            .list_telemetry_rollups(
                limit_or_default(query.limit),
                query.client_id.as_deref(),
                query.bucket_secs,
                true,
            )
            .await?
    };
    Ok(Json(rows))
}

pub(crate) async fn list_telemetry_samples(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TelemetrySampleQuery>,
) -> Result<Json<Vec<TelemetrySampleView>>, ApiError> {
    state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    validate_telemetry_sample_query(&query)?;
    Ok(Json(
        state
            .repo
            .list_telemetry_samples(
                limit_or_default(query.limit),
                query.client_id.as_deref(),
                query.start_unix,
                query.end_unix,
                true,
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
    let rows = if query.latest {
        state
            .repo
            .list_latest_telemetry_network_rates(
                telemetry_network_rate_limit_or_default(query.limit),
                query.client_id.as_deref(),
                query.interface.as_deref(),
                query.bucket_secs,
            )
            .await?
    } else {
        state
            .repo
            .list_telemetry_network_rates(
                telemetry_network_rate_limit_or_default(query.limit),
                query.client_id.as_deref(),
                query.interface.as_deref(),
                query.bucket_secs,
                true,
            )
            .await?
    };
    Ok(Json(rows))
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
    state
        .repo
        .delete_runtime_config_patch_generator(generator_id, &request.reviewed_name, &operator)
        .await
        .map_err(runtime_config_patch_generator_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn list_runtime_config_apply_states(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<RuntimeConfigApplyStateView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_CONFIG_READ)
        .await?;
    Ok(Json(
        state.repo.list_runtime_config_apply_states(None).await?,
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
        .await
        .map_err(runtime_config_override_error)?;
    let sync = dispatch_runtime_config_for_clients(
        &state,
        &operator,
        request.target_client_ids.clone(),
        &reason,
    )
    .await;
    Ok(Json(RuntimeConfigPatchResponse {
        target_count: request.target_client_ids.len(),
        overrides,
        sync_job_ids: sync.iter().filter_map(|outcome| outcome.job_id).collect(),
        sync,
    }))
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
    match &request.action {
        crate::model::BulkTagMutationAction::Add => {
            validate_persisted_tag_name(&request.tag)?;
        }
        crate::model::BulkTagMutationAction::Remove => {
            validate_legacy_tag_name_for_cleanup(&request.tag)?;
        }
    }
    validate_bulk_selector_expression(&request.selector_expression)?;
    request.target_client_ids = normalized_target_client_ids(&request.target_client_ids)?;
    let fixed_targets = verified_fixed_target_ids(
        &state,
        &request.target_client_ids,
        "tag_fixed_targets_not_found",
    )
    .await?;
    if request.confirmed {
        let mut preview_request = request.clone();
        preview_request.confirmed = false;
        preview_request.preview_hash = None;
        preview_request.privilege_assertion = None;
        let preview = state.repo.bulk_mutate_tags(&preview_request).await?;
        require_matching_preview_hash(
            request.preview_hash.as_deref(),
            &preview.preview_hash,
            "tag_mutation_preview_hash_required",
            "tag_mutation_preview_hash_mismatch",
        )?;
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
    validate_legacy_tag_name_for_cleanup(&tag)?;
    if request.confirmed {
        let preview = state.repo.delete_tag(&tag, false).await?;
        require_matching_preview_hash(
            request.preview_hash.as_deref(),
            &preview.preview_hash,
            "tag_delete_preview_hash_required",
            "tag_delete_preview_hash_mismatch",
        )?;
        let affected_targets = preview
            .affected
            .iter()
            .map(|client| client.id.clone())
            .collect::<Vec<_>>();
        let intent =
            DbPrivilegeIntent::new("tag.delete", &tag, None, &affected_targets, true, None);
        verify_privilege_intent(&state, &intent, request.privilege_assertion.clone()).await?;
    }
    Ok(Json(state.repo.delete_tag(&tag, request.confirmed).await?))
}

fn require_matching_preview_hash(
    submitted: Option<&str>,
    expected: &str,
    required_code: &'static str,
    mismatch_code: &'static str,
) -> Result<(), ApiError> {
    let Some(submitted) = submitted.map(str::trim).filter(|value| !value.is_empty()) else {
        return Err(ApiError::conflict(required_code));
    };
    if submitted != expected {
        return Err(ApiError::conflict(mismatch_code));
    }
    Ok(())
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
    } else if message.contains("runtime_config_patch_generator_delete_review_stale") {
        ApiError::conflict("runtime_config_patch_generator_delete_review_stale")
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
    } else if message.contains("runtime_config_patch_configuration_preset_field_forbidden") {
        ApiError::bad_request("runtime_config_patch_configuration_preset_field_forbidden")
    } else if message.contains("runtime_config_patch_managed_tunnel_plans_forbidden") {
        ApiError::bad_request("runtime_config_patch_managed_tunnel_plans_forbidden")
    } else if message.contains("runtime_config_patch_managed_port_forwarding_forbidden") {
        ApiError::bad_request("runtime_config_patch_managed_port_forwarding_forbidden")
    } else if message.contains("runtime_config_patch_toml_invalid")
        || message.contains("failed to parse runtime config patch TOML")
    {
        ApiError::bad_request("runtime_config_patch_toml_invalid")
    } else {
        ApiError::bad_request("runtime_config_patch_invalid")
    }
}

fn runtime_config_override_error(error: anyhow::Error) -> ApiError {
    if error
        .to_string()
        .contains("runtime_config_target_no_longer_available")
    {
        ApiError::conflict("runtime_config_target_no_longer_available")
    } else {
        ApiError::from(error)
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
        .is_some_and(|bucket_secs| bucket_secs < 60 || bucket_secs % 60 != 0)
    {
        return Err(ApiError::bad_request("invalid_bucket_secs"));
    }
    Ok(())
}

fn validate_telemetry_sample_query(query: &TelemetrySampleQuery) -> Result<(), ApiError> {
    if query
        .client_id
        .as_ref()
        .is_some_and(|client_id| client_id.is_empty() || client_id.len() > 128)
    {
        return Err(ApiError::bad_request("invalid_client_id"));
    }
    if query
        .start_unix
        .zip(query.end_unix)
        .is_some_and(|(start, end)| start > end)
    {
        return Err(ApiError::bad_request("invalid_telemetry_time_range"));
    }
    Ok(())
}

fn validate_persisted_tag_name(tag: &str) -> Result<(), ApiError> {
    validate_legacy_tag_name_for_cleanup(tag)?;
    if tag.split(':').any(str::is_empty) {
        return Err(ApiError::bad_request("invalid_tag_name"));
    }
    Ok(())
}

fn validate_legacy_tag_name_for_cleanup(tag: &str) -> Result<(), ApiError> {
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

fn peer_client_ids_for_deleted_agent(
    client_id: &str,
    endpoint_pairs: impl IntoIterator<Item = (String, String)>,
) -> BTreeSet<String> {
    let mut peers = BTreeSet::new();
    for (left_client_id, right_client_id) in endpoint_pairs {
        if left_client_id == client_id && right_client_id != client_id {
            peers.insert(right_client_id);
        } else if right_client_id == client_id && left_client_id != client_id {
            peers.insert(left_client_id);
        }
    }
    peers
}

fn agent_mutation_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("agent_not_found") {
        ApiError::not_found("agent_not_found")
    } else if message.contains("agent_port_forwarding_cleanup_required") {
        ApiError::conflict("agent_port_forwarding_cleanup_required")
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
        .is_some_and(|bucket_secs| bucket_secs < 60 || bucket_secs % 60 != 0)
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
    use super::{
        peer_client_ids_for_deleted_agent, runtime_config_patch_validation_error,
        telemetry_network_rate_limit_or_default, validate_legacy_tag_name_for_cleanup,
        validate_persisted_tag_name, validate_telemetry_network_rate_query,
        validate_telemetry_rollup_query,
    };
    use crate::model::{TelemetryNetworkRateQuery, TelemetryRollupQuery};
    use axum::http::StatusCode;

    #[test]
    fn runtime_config_patch_reports_server_managed_port_forwarding() {
        let error = runtime_config_patch_validation_error(anyhow::anyhow!(
            "runtime_config_patch_managed_port_forwarding_forbidden"
        ));
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(
            error.code,
            "runtime_config_patch_managed_port_forwarding_forbidden"
        );
    }

    #[test]
    fn persisted_tags_reject_inner_selector_prefixes() {
        validate_persisted_tag_name("provider:alpha").unwrap();
        validate_persisted_tag_name("country:US").unwrap();
        validate_persisted_tag_name("region:legacy-name").unwrap();

        assert!(validate_persisted_tag_name("id:edge-a").is_err());
        assert!(validate_persisted_tag_name("name:edge-a").is_err());
        assert!(validate_persisted_tag_name("provider:").is_err());
        assert!(validate_persisted_tag_name(":alpha").is_err());
        assert!(validate_persisted_tag_name("role::edge").is_err());
        validate_legacy_tag_name_for_cleanup("provider:").unwrap();
        validate_legacy_tag_name_for_cleanup(":alpha").unwrap();
        validate_legacy_tag_name_for_cleanup("role::edge").unwrap();
    }

    #[test]
    fn telemetry_network_rates_allow_fleet_scale_limits() {
        assert_eq!(telemetry_network_rate_limit_or_default(None), 100);
        assert_eq!(telemetry_network_rate_limit_or_default(Some(5_000)), 5_000);
        assert_eq!(telemetry_network_rate_limit_or_default(Some(50_000)), 5_000);
    }

    #[test]
    fn telemetry_queries_accept_adaptive_minute_aligned_spans() {
        for bucket_secs in [60, 120, 300, 86_400] {
            validate_telemetry_rollup_query(&TelemetryRollupQuery {
                limit: None,
                client_id: None,
                bucket_secs: Some(bucket_secs),
                latest: false,
            })
            .unwrap();
            validate_telemetry_network_rate_query(&TelemetryNetworkRateQuery {
                limit: None,
                client_id: None,
                interface: None,
                bucket_secs: Some(bucket_secs),
                latest: false,
            })
            .unwrap();
        }

        for bucket_secs in [-60, 0, 59, 61] {
            assert!(validate_telemetry_rollup_query(&TelemetryRollupQuery {
                limit: None,
                client_id: None,
                bucket_secs: Some(bucket_secs),
                latest: false,
            })
            .is_err());
            assert!(
                validate_telemetry_network_rate_query(&TelemetryNetworkRateQuery {
                    limit: None,
                    client_id: None,
                    interface: None,
                    bucket_secs: Some(bucket_secs),
                    latest: false,
                })
                .is_err()
            );
        }
    }

    #[test]
    fn deleting_agent_collects_each_declared_tunnel_peer_once() {
        let peers = peer_client_ids_for_deleted_agent(
            "edge-a",
            [
                ("edge-a".to_string(), "edge-b".to_string()),
                ("edge-c".to_string(), "edge-a".to_string()),
                ("edge-a".to_string(), "edge-b".to_string()),
                ("edge-c".to_string(), "edge-d".to_string()),
            ],
        );
        assert_eq!(peers.into_iter().collect::<Vec<_>>(), ["edge-b", "edge-c"]);
    }
}
