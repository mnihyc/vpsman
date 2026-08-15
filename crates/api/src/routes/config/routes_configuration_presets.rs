use std::collections::{BTreeSet, HashMap};

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use futures_util::{stream, StreamExt};
use uuid::Uuid;
use vpsman_common::{runtime_config_content_hash, AgentRuntimeConfig};

use crate::{
    model::{
        ApplyConfigurationSourceOverrideRequest, ApplyConfigurationSourceOverrideResponse,
        BulkResolveRequest, CloneConfigurationPresetRequest, ConfigurationOverrideAction,
        ConfigurationPresetPreviewView, ConfigurationPresetQuery, ConfigurationPresetView,
        ConfigurationSourceQuery, ConfigurationSourceView, CreateConfigurationPresetRequest,
        EffectiveAgentConfigQuery, EffectiveAgentConfigView, NetworkAdapterDefinitionQuery,
        NetworkAdapterDefinitionView, PreviewConfigurationPresetRequest,
        PreviewConfigurationSourceOverrideRequest, UpdateConfigurationPresetRequest,
        UpdateConfigurationPresetResponse, UpsertNetworkAdapterDefinitionRequest,
    },
    privilege::{verify_privilege_intent, DbPrivilegeIntent},
    repository_configuration_presets::{
        validate_configuration_behavior, validate_configuration_preset_request,
        validate_network_adapter_definition,
    },
    runtime_config::{
        clear_runtime_tunnel_credentials, compose_runtime_config_for_agent_with_read_model,
        dispatch_runtime_config_for_clients,
    },
    security::{require_vps_rule_selector_scope, SCOPE_CONFIG_READ, SCOPE_NETWORK_READ},
    selector_expression::parse_selector_expression,
    state::AppState,
    ApiError,
};

const CONFIGURATION_SOURCE_SYNC_CONCURRENCY: usize = 8;

pub(crate) async fn list_configuration_presets(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ConfigurationPresetQuery>,
) -> Result<Json<Vec<ConfigurationPresetView>>, ApiError> {
    state
        .require_operator_scope(&headers, SCOPE_CONFIG_READ)
        .await?;
    if let Some(behavior) = query.behavior.as_deref() {
        validate_configuration_behavior(behavior).map_err(configuration_preset_error)?;
    }
    Ok(Json(
        state
            .repo
            .list_configuration_presets(query.behavior.as_deref())
            .await
            .map_err(ApiError::internal_mapper(
                "configuration_presets_unavailable",
                "The configuration presets could not be loaded.",
            ))?,
    ))
}

pub(crate) async fn create_configuration_preset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateConfigurationPresetRequest>,
) -> Result<Json<ConfigurationPresetView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "config:write")
        .await?;
    validate_configuration_preset_request(
        &request.behavior,
        &request.name,
        request.description.as_deref(),
        &request.definition,
    )
    .map_err(configuration_preset_error)?;
    Ok(Json(
        state
            .repo
            .create_configuration_preset(&request, &operator)
            .await
            .map_err(configuration_preset_error)?,
    ))
}

pub(crate) async fn clone_configuration_preset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(preset_id): Path<Uuid>,
    Json(request): Json<CloneConfigurationPresetRequest>,
) -> Result<Json<ConfigurationPresetView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "config:write")
        .await?;
    validate_name_and_description(&request.name, request.description.as_deref())?;
    Ok(Json(
        state
            .repo
            .clone_configuration_preset(
                preset_id,
                &request.name,
                request.description.as_deref(),
                &operator,
            )
            .await
            .map_err(configuration_preset_error)?,
    ))
}

pub(crate) async fn preview_configuration_preset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(preset_id): Path<Uuid>,
    Json(request): Json<PreviewConfigurationPresetRequest>,
) -> Result<Json<ConfigurationPresetPreviewView>, ApiError> {
    state
        .require_operator_scope(&headers, SCOPE_CONFIG_READ)
        .await?;
    Ok(Json(
        state
            .repo
            .preview_configuration_preset_update(preset_id, &request)
            .await
            .map_err(configuration_preset_error)?,
    ))
}

pub(crate) async fn update_configuration_preset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(preset_id): Path<Uuid>,
    Json(request): Json<UpdateConfigurationPresetRequest>,
) -> Result<Json<UpdateConfigurationPresetResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "config:write")
        .await?;
    let preview = state
        .repo
        .preview_configuration_preset_update(
            preset_id,
            &PreviewConfigurationPresetRequest {
                description: request.description,
                definition: request.definition,
            },
        )
        .await
        .map_err(configuration_preset_error)?;
    require_configuration_preset_changes(&preview.changed_keys)?;
    require_preview_hash(&request.preview_hash, &preview.preview_hash)?;
    if !preview.affected_client_ids.is_empty() {
        let target = format!("configuration_preset:{preset_id}");
        verify_privilege_intent(
            &state,
            &DbPrivilegeIntent::new(
                "configuration_preset.update",
                &target,
                None,
                &preview.affected_client_ids,
                true,
                Some(&preview.preview_hash),
            ),
            request.privilege_assertion,
        )
        .await?;
    }
    let preset = state
        .repo
        .update_configuration_preset(preset_id, &preview, &operator)
        .await
        .map_err(configuration_preset_error)?;
    let sync = if preview.current_definition != preview.candidate_definition {
        dispatch_runtime_config_for_clients(
            &state,
            &operator,
            preview.affected_client_ids.clone(),
            "configuration_preset_updated",
        )
        .await
    } else {
        Vec::new()
    };
    Ok(Json(UpdateConfigurationPresetResponse {
        preset,
        preview,
        sync,
    }))
}

pub(crate) async fn delete_configuration_preset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(preset_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "config:write")
        .await?;
    state
        .repo
        .delete_configuration_preset(preset_id, &operator)
        .await
        .map_err(configuration_preset_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn list_configuration_sources(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ConfigurationSourceQuery>,
) -> Result<Json<Vec<ConfigurationSourceView>>, ApiError> {
    state
        .require_operator_scope(&headers, SCOPE_CONFIG_READ)
        .await?;
    if let Some(client_id) = query.client_id.as_deref() {
        validate_client_id(client_id)?;
    }
    if let Some(behavior) = query.behavior.as_deref() {
        validate_configuration_behavior(behavior).map_err(configuration_preset_error)?;
    }
    let mut rows = state
        .repo
        .list_configuration_sources(query.client_id.as_deref(), query.behavior.as_deref())
        .await
        .map_err(ApiError::internal_mapper(
            "configuration_sources_unavailable",
            "The configuration sources could not be loaded.",
        ))?;
    if query.client_id.is_some() && rows.is_empty() {
        return Err(ApiError::not_found("configuration_client_not_found"));
    }
    let _ = enrich_runtime_sync(&state, &mut rows).await?;
    Ok(Json(rows))
}

pub(crate) async fn preview_configuration_source_override(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut request): Json<PreviewConfigurationSourceOverrideRequest>,
) -> Result<Json<crate::model::ConfigurationSourceOverridePreviewView>, ApiError> {
    state
        .require_operator_scope(&headers, SCOPE_CONFIG_READ)
        .await?;
    request.target_client_ids = resolve_override_targets(
        &state,
        &request.target_client_ids,
        &request.selector_expression,
    )
    .await?;
    Ok(Json(
        state
            .repo
            .preview_configuration_source_override(&request)
            .await
            .map_err(configuration_preset_error)?,
    ))
}

pub(crate) async fn apply_configuration_source_override(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ApplyConfigurationSourceOverrideRequest>,
) -> Result<Json<ApplyConfigurationSourceOverrideResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "config:write")
        .await?;
    if let Some(expression) = parse_selector_expression(&request.selector_expression)
        .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?
    {
        require_vps_rule_selector_scope(&operator.operator.scopes, &expression)?;
    }
    let resolved_targets = resolve_override_targets(
        &state,
        &request.target_client_ids,
        &request.selector_expression,
    )
    .await?;
    let preview = state
        .repo
        .preview_configuration_source_override(&PreviewConfigurationSourceOverrideRequest {
            action: request.action,
            behavior: request.behavior.clone(),
            preset_id: request.preset_id,
            selector_expression: request.selector_expression.clone(),
            target_client_ids: resolved_targets,
        })
        .await
        .map_err(configuration_preset_error)?;
    require_preview_hash(&request.preview_hash, &preview.preview_hash)?;
    let target_ids = preview
        .targets
        .iter()
        .map(|target| target.client_id.clone())
        .collect::<Vec<_>>();
    let target = match request.action {
        ConfigurationOverrideAction::Set => format!(
            "configuration_preset:{}",
            request.preset_id.ok_or_else(|| ApiError::bad_request(
                "configuration_source_override_preset_required"
            ))?
        ),
        ConfigurationOverrideAction::Reset => {
            format!("configuration_behavior:{}", request.behavior)
        }
    };
    let selector_expression = request.selector_expression.trim();
    let selector = (!selector_expression.is_empty()).then_some(selector_expression);
    verify_privilege_intent(
        &state,
        &DbPrivilegeIntent::new(
            "configuration_source_override.apply",
            &target,
            selector,
            &target_ids,
            true,
            Some(&preview.preview_hash),
        ),
        request.privilege_assertion,
    )
    .await?;
    state
        .repo
        .apply_configuration_source_override(&preview, &operator)
        .await
        .map_err(configuration_preset_error)?;
    let sync = dispatch_runtime_config_for_clients(
        &state,
        &operator,
        target_ids,
        "configuration_source_override_applied",
    )
    .await;
    Ok(Json(ApplyConfigurationSourceOverrideResponse {
        preview,
        sync,
    }))
}

pub(crate) async fn effective_agent_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<EffectiveAgentConfigQuery>,
) -> Result<Json<EffectiveAgentConfigView>, ApiError> {
    state
        .require_operator_scope(&headers, SCOPE_CONFIG_READ)
        .await?;
    validate_client_id(&query.client_id)?;
    let mut sources = state
        .repo
        .list_configuration_sources(Some(&query.client_id), None)
        .await
        .map_err(ApiError::internal_mapper(
            "configuration_sources_unavailable",
            "The configuration sources could not be loaded.",
        ))?;
    let mut desired_configs = enrich_runtime_sync(&state, &mut sources).await?;
    let mut config = desired_configs
        .remove(&query.client_id)
        .ok_or_else(|| ApiError::not_found("runtime_config_client_not_found"))?;
    clear_runtime_tunnel_credentials(&mut config.network);
    let sections = serde_json::to_value(&config).map_err(|error| {
        ApiError::internal(
            "effective_agent_config_projection_failed",
            "The effective agent configuration could not be displayed.",
            anyhow::Error::from(error),
        )
    })?;
    let toml = toml::to_string_pretty(&config).map_err(|error| {
        ApiError::internal(
            "effective_agent_config_projection_failed",
            "The effective agent configuration could not be displayed.",
            anyhow::Error::from(error),
        )
    })?;
    Ok(Json(EffectiveAgentConfigView {
        client_id: query.client_id,
        sections,
        toml,
        sources,
        generated_at: crate::unix_now().to_string(),
    }))
}

pub(crate) async fn list_network_adapter_definitions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<NetworkAdapterDefinitionQuery>,
) -> Result<Json<Vec<NetworkAdapterDefinitionView>>, ApiError> {
    state
        .require_operator_scope(&headers, SCOPE_NETWORK_READ)
        .await?;
    validate_adapter_kind(query.adapter_kind.as_deref())?;
    Ok(Json(
        state
            .repo
            .list_network_adapter_definitions(query.adapter_kind.as_deref())
            .await
            .map_err(ApiError::internal_mapper(
                "network_adapter_definitions_unavailable",
                "The network adapter definitions could not be loaded.",
            ))?,
    ))
}

pub(crate) async fn create_network_adapter_definition(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpsertNetworkAdapterDefinitionRequest>,
) -> Result<Json<NetworkAdapterDefinitionView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    validate_network_adapter_definition(&request).map_err(network_adapter_error)?;
    Ok(Json(
        state
            .repo
            .create_network_adapter_definition(&request, &operator)
            .await
            .map_err(network_adapter_error)?,
    ))
}

pub(crate) async fn update_network_adapter_definition(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(definition_id): Path<Uuid>,
    Json(request): Json<UpsertNetworkAdapterDefinitionRequest>,
) -> Result<Json<NetworkAdapterDefinitionView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    validate_network_adapter_definition(&request).map_err(network_adapter_error)?;
    let existing = state
        .repo
        .network_adapter_definition_by_id(definition_id, None)
        .await
        .map_err(ApiError::internal_mapper(
            "network_adapter_definition_unavailable",
            "The network adapter definition could not be loaded.",
        ))?
        .ok_or_else(|| ApiError::not_found("network_adapter_definition_not_found"))?;
    if existing.adapter_kind != request.adapter_kind {
        return Err(ApiError::conflict(
            "network_adapter_definition_kind_immutable",
        ));
    }
    let saved = state
        .repo
        .update_network_adapter_definition(definition_id, &request, &operator)
        .await
        .map_err(network_adapter_error)?;
    Ok(Json(saved))
}

pub(crate) async fn delete_network_adapter_definition(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(definition_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    state
        .repo
        .delete_network_adapter_definition(definition_id, &operator)
        .await
        .map_err(network_adapter_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn resolve_override_targets(
    state: &AppState,
    fixed_ids: &[String],
    selector_expression: &str,
) -> Result<Vec<String>, ApiError> {
    let mut selected = BTreeSet::new();
    for value in fixed_ids {
        let value = value.trim();
        if value.is_empty()
            || value.len() > 200
            || value.chars().any(|character| character.is_control())
        {
            return Err(ApiError::bad_request("target_client_id_invalid"));
        }
        selected.insert(value.to_string());
    }
    if !selector_expression.trim().is_empty() {
        parse_selector_expression(selector_expression)
            .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?;
        let resolved = state
            .repo
            .resolve_bulk_targets(&BulkResolveRequest {
                selector_expression: selector_expression.trim().to_string(),
            })
            .await
            .map_err(ApiError::internal_mapper(
                "configuration_override_targets_unavailable",
                "The configuration override targets could not be resolved.",
            ))?;
        selected.extend(resolved.targets.into_iter().map(|agent| agent.id));
    }
    if selected.is_empty() {
        return Err(ApiError::bad_request(
            "configuration_source_override_targets_required",
        ));
    }
    if selected.len() > 500 {
        return Err(ApiError::bad_request(
            "configuration_source_override_targets_too_many",
        ));
    }
    let known = state
        .repo
        .list_agents()
        .await
        .map_err(ApiError::internal_mapper(
            "agent_inventory_unavailable",
            "The VPS inventory could not be loaded.",
        ))?
        .into_iter()
        .map(|agent| agent.id)
        .collect::<BTreeSet<_>>();
    if selected.iter().any(|client_id| !known.contains(client_id)) {
        return Err(ApiError::bad_request(
            "configuration_source_override_targets_not_found",
        ));
    }
    Ok(selected.into_iter().collect())
}

async fn enrich_runtime_sync(
    state: &AppState,
    rows: &mut [ConfigurationSourceView],
) -> Result<HashMap<String, AgentRuntimeConfig>, ApiError> {
    let client_ids = rows
        .iter()
        .map(|row| row.client_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if client_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let agents = state
        .repo
        .list_agents_for_client_ids(&client_ids)
        .await
        .map_err(ApiError::internal_mapper(
            "agent_inventory_unavailable",
            "The VPS inventory could not be loaded.",
        ))?;
    let agents_by_id = agents
        .into_iter()
        .map(|agent| (agent.id.clone(), agent))
        .collect::<HashMap<_, _>>();
    let apply_client_id = (client_ids.len() == 1).then_some(client_ids[0].as_str());
    let applies = state
        .repo
        .list_runtime_config_apply_states(apply_client_id)
        .await
        .map_err(ApiError::internal_mapper(
            "runtime_config_apply_states_unavailable",
            "The runtime configuration apply states could not be loaded.",
        ))?
        .into_iter()
        .map(|apply| (apply.client_id.clone(), apply))
        .collect::<HashMap<_, _>>();
    let tunnel_plans = state
        .repo
        .list_tunnel_plans()
        .await
        .map_err(ApiError::internal_mapper(
            "tunnel_plans_unavailable",
            "The tunnel plans could not be loaded.",
        ))?;
    let preset_patches = state
        .repo
        .render_configuration_preset_patches_for_clients(&client_ids)
        .await
        .map_err(ApiError::internal_mapper(
            "configuration_preset_patches_unavailable",
            "The configuration preset patches could not be loaded.",
        ))?;
    let mut desired_hash_futures = Vec::with_capacity(client_ids.len());
    for client_id in &client_ids {
        let client_id = client_id.clone();
        let agent = agents_by_id.get(&client_id).cloned();
        let preset_toml = preset_patches.get(&client_id).cloned();
        let tunnel_plans = tunnel_plans.as_slice();
        desired_hash_futures.push(async move {
            let agent =
                agent.ok_or_else(|| ApiError::not_found("runtime_config_client_not_found"))?;
            let preset_toml = preset_toml
                .ok_or_else(|| ApiError::not_found("runtime_config_client_not_found"))?;
            let desired = compose_runtime_config_for_agent_with_read_model(
                state,
                &agent,
                1,
                &preset_toml,
                tunnel_plans,
            )
            .await?;
            let desired_hash = runtime_config_content_hash(&desired).map_err(|error| {
                ApiError::internal(
                    "configuration_source_sync_hash_failed",
                    "The configuration sync preview could not be prepared.",
                    anyhow::Error::from(error),
                )
            })?;
            Ok::<_, ApiError>((client_id, desired, desired_hash))
        });
    }
    let desired_hashes = stream::iter(desired_hash_futures)
        .buffered(CONFIGURATION_SOURCE_SYNC_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
    let mut runtime_sync_by_client = HashMap::with_capacity(desired_hashes.len());
    let mut desired_configs = HashMap::with_capacity(desired_hashes.len());
    for desired_hash in desired_hashes {
        let (client_id, desired, desired_hash) = desired_hash?;
        let apply = applies.get(&client_id);
        let (sync_state, reason) = match apply {
            Some(record)
                if record.pending_status.as_deref() == Some("failed")
                    && record
                        .pending_content_hash
                        .as_deref()
                        .is_some_and(|hash| hash.eq_ignore_ascii_case(&desired_hash)) =>
            {
                (
                    "failed",
                    record
                        .pending_error
                        .clone()
                        .unwrap_or_else(|| "The last runtime apply failed".to_string()),
                )
            }
            Some(record)
                if record.pending_status.as_deref() == Some("queued")
                    && record
                        .pending_content_hash
                        .as_deref()
                        .is_some_and(|hash| hash.eq_ignore_ascii_case(&desired_hash)) =>
            {
                (
                    "queued",
                    record
                        .pending_reason
                        .clone()
                        .unwrap_or_else(|| "Runtime apply is queued".to_string()),
                )
            }
            Some(record)
                if record
                    .applied_content_hash
                    .as_deref()
                    .is_some_and(|hash| hash.eq_ignore_ascii_case(&desired_hash)) =>
            {
                (
                    "applied",
                    "The VPS acknowledged this effective configuration".to_string(),
                )
            }
            Some(record)
                if record.pending_status.is_some() || record.applied_content_hash.is_some() =>
            {
                (
                    "stale",
                    "The acknowledged runtime configuration differs from desired state".to_string(),
                )
            }
            _ => (
                "unknown",
                "The VPS has not acknowledged a runtime configuration yet".to_string(),
            ),
        };
        runtime_sync_by_client.insert(client_id.clone(), (sync_state, reason));
        desired_configs.insert(client_id, desired);
    }
    for row in rows {
        let (sync_state, reason) = runtime_sync_by_client
            .get(&row.client_id)
            .ok_or_else(|| ApiError::not_found("runtime_config_client_not_found"))?;
        row.runtime_sync.state = sync_state.to_string();
        row.runtime_sync.reason = reason.clone();
    }
    Ok(desired_configs)
}

fn require_preview_hash(submitted: &str, expected: &str) -> Result<(), ApiError> {
    if submitted.trim().is_empty() {
        return Err(ApiError::conflict("configuration_preview_hash_required"));
    }
    if submitted.trim() != expected {
        return Err(ApiError::conflict("configuration_preview_hash_mismatch"));
    }
    Ok(())
}

fn require_configuration_preset_changes(changed_keys: &[String]) -> Result<(), ApiError> {
    if changed_keys.is_empty() {
        return Err(ApiError::conflict("configuration_preset_no_changes"));
    }
    Ok(())
}

fn validate_name_and_description(name: &str, description: Option<&str>) -> Result<(), ApiError> {
    if name.trim().is_empty() || name.trim().len() > 256 || name.chars().any(char::is_control) {
        return Err(ApiError::bad_request("configuration_preset_name_invalid"));
    }
    if description.is_some_and(|value| value.len() > 4096) {
        return Err(ApiError::bad_request(
            "configuration_preset_description_invalid",
        ));
    }
    Ok(())
}

fn validate_client_id(client_id: &str) -> Result<(), ApiError> {
    if client_id.is_empty() || client_id.len() > 128 {
        return Err(ApiError::bad_request("invalid_client_id"));
    }
    Ok(())
}

fn validate_adapter_kind(adapter_kind: Option<&str>) -> Result<(), ApiError> {
    if adapter_kind.is_some_and(|value| !matches!(value, "runtime_tunnel" | "routing_cost")) {
        return Err(ApiError::bad_request("network_adapter_kind_invalid"));
    }
    Ok(())
}

fn configuration_preset_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("configuration_preset_system_immutable") {
        ApiError::conflict("configuration_preset_system_immutable")
    } else if message.contains("configuration_preset_in_use") {
        ApiError::conflict("configuration_preset_in_use")
    } else if message.contains("configuration_preset_duplicate") {
        ApiError::conflict("configuration_preset_duplicate")
    } else if message.contains("configuration_preset_preview_stale")
        || message.contains("configuration_source_override_preview_stale")
    {
        ApiError::conflict("configuration_preview_stale")
    } else if message.contains("configuration_source_override_behavior_mismatch") {
        ApiError::conflict("configuration_source_override_behavior_mismatch")
    } else if message.contains("configuration_source_override_default_requires_reset") {
        ApiError::conflict("configuration_source_override_default_requires_reset")
    } else if message.contains("configuration_source_override_targets_not_found") {
        ApiError::bad_request("configuration_source_override_targets_not_found")
    } else if message.contains("not_found") {
        ApiError::not_found("configuration_preset_not_found")
    } else {
        ApiError::bad_request("configuration_preset_invalid")
    }
}

fn network_adapter_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("network_adapter_definition_in_use") {
        ApiError::conflict("network_adapter_definition_in_use")
    } else if message.contains("network_adapter_definition_kind_immutable") {
        ApiError::conflict("network_adapter_definition_kind_immutable")
    } else if message.contains("network_adapter_definition_duplicate") {
        ApiError::conflict("network_adapter_definition_duplicate")
    } else if message.contains("not_found") {
        ApiError::not_found("network_adapter_definition_not_found")
    } else {
        ApiError::bad_request("network_adapter_definition_invalid")
    }
}

#[cfg(test)]
#[path = "tests_routes_configuration_presets.rs"]
mod tests;
