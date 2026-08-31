use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
    time::Duration,
};

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use uuid::Uuid;

use crate::{
    error::ApiError,
    model::AuthContext,
    model_port_forwarding::{
        normalize_port_forward_hostname, CreatePortForwardRuleRequest, PortForwardBulkAction,
        PortForwardBulkRequest, PortForwardBulkResponse, PortForwardClientSyncView,
        PortForwardMutationRequest, PortForwardMutationResponse, PortForwardRuleListItem,
        PortForwardRuleView, PortForwardSyncView, ResolveHostnameRequest, ResolveHostnameResponse,
        ResolvedAddressView, UpdatePortForwardRuleRequest,
    },
    runtime_config::dispatch_runtime_config_for_clients,
    security::SCOPE_NETWORK_READ,
    state::AppState,
};

pub(crate) async fn list_port_forward_rules(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<PortForwardRuleListItem>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_NETWORK_READ)
        .await?;
    Ok(Json(
        state
            .repo
            .list_port_forward_rule_items()
            .await
            .map_err(ApiError::internal_mapper(
                "port_forward_rules_unavailable",
                "Port-forwarding rules could not be loaded.",
            ))?,
    ))
}

pub(crate) async fn create_port_forward_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreatePortForwardRuleRequest>,
) -> Result<(StatusCode, Json<PortForwardMutationResponse>), ApiError> {
    let operator = network_writer(&state, &headers).await?;
    validate_confirmation(request.enabled, request.confirmed)?;
    require_agent(&state, &request.client_id, request.enabled).await?;
    let client_id = request.client_id.clone();
    let created = state
        .repo
        .create_port_forward_rule(&request, &operator)
        .await
        .map_err(port_forward_repository_error)?;
    let sync = if request.enabled {
        sync_client(&state, &operator, &client_id, "port_forward_rule_created").await
    } else {
        no_sync("saved_disabled")
    };
    let rule = state
        .repo
        .get_port_forward_rule(created.id)
        .await
        .map_err(ApiError::internal_mapper(
            "port_forward_rule_unavailable",
            "The port-forwarding rule could not be loaded.",
        ))?
        .unwrap_or(created);
    Ok((
        StatusCode::CREATED,
        Json(PortForwardMutationResponse {
            rule: rule.into(),
            sync,
        }),
    ))
}

pub(crate) async fn update_port_forward_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(rule_id): Path<Uuid>,
    Json(request): Json<UpdatePortForwardRuleRequest>,
) -> Result<Json<PortForwardMutationResponse>, ApiError> {
    let operator = network_writer(&state, &headers).await?;
    let existing = state
        .repo
        .get_port_forward_rule_identity(rule_id)
        .await
        .map_err(ApiError::internal_mapper(
            "port_forward_rule_unavailable",
            "The port-forwarding rule could not be loaded.",
        ))?
        .filter(|rule| rule.deleted_at.is_none())
        .ok_or_else(|| ApiError::not_found("port_forward_rule_not_found"))?;
    if existing.revision != request.expected_revision {
        return Err(ApiError::conflict("port_forward_rule_snapshot_stale"));
    }
    validate_confirmation(existing.enabled || request.enabled, request.confirmed)?;
    require_agent(&state, &existing.client_id, request.enabled).await?;
    let changed = state
        .repo
        .update_port_forward_rule(rule_id, &request, &operator)
        .await
        .map_err(port_forward_repository_error)?;
    let sync = if existing.enabled || changed.enabled {
        sync_client(
            &state,
            &operator,
            &existing.client_id,
            "port_forward_rule_updated",
        )
        .await
    } else {
        no_sync("saved_disabled")
    };
    let rule = state
        .repo
        .get_port_forward_rule(rule_id)
        .await
        .map_err(ApiError::internal_mapper(
            "port_forward_rule_unavailable",
            "The port-forwarding rule could not be loaded.",
        ))?
        .unwrap_or(changed);
    Ok(Json(PortForwardMutationResponse {
        rule: rule.into(),
        sync,
    }))
}

pub(crate) async fn enable_port_forward_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(rule_id): Path<Uuid>,
    Json(request): Json<PortForwardMutationRequest>,
) -> Result<Json<PortForwardMutationResponse>, ApiError> {
    mutate_enabled(state, headers, rule_id, request, true).await
}

pub(crate) async fn disable_port_forward_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(rule_id): Path<Uuid>,
    Json(request): Json<PortForwardMutationRequest>,
) -> Result<Json<PortForwardMutationResponse>, ApiError> {
    mutate_enabled(state, headers, rule_id, request, false).await
}

pub(crate) async fn delete_port_forward_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(rule_id): Path<Uuid>,
    Json(request): Json<PortForwardMutationRequest>,
) -> Result<Json<PortForwardMutationResponse>, ApiError> {
    let operator = network_writer(&state, &headers).await?;
    require_confirmed(request.confirmed)?;
    let identity = state
        .repo
        .get_port_forward_rule_identity(rule_id)
        .await
        .map_err(ApiError::internal_mapper(
            "port_forward_rule_unavailable",
            "The port-forwarding rule could not be loaded.",
        ))?
        .filter(|rule| rule.deleted_at.is_none())
        .ok_or_else(|| ApiError::not_found("port_forward_rule_not_found"))?;
    if identity.revision != request.expected_revision {
        return Err(ApiError::conflict("port_forward_rule_snapshot_stale"));
    }
    let existing =
        state
            .repo
            .get_port_forward_rule(rule_id)
            .await
            .map_err(ApiError::internal_mapper(
                "port_forward_rule_unavailable",
                "The port-forwarding rule could not be loaded.",
            ))?;
    if existing.is_none() {
        let configuration_error = state
            .repo
            .port_forward_rule_configuration_error(rule_id)
            .await
            .map_err(ApiError::internal_mapper(
                "port_forward_rule_configuration_lookup_failed",
                "The port-forwarding rule configuration could not be checked.",
            ))?
            .ok_or_else(|| ApiError::conflict("port_forward_rule_configuration_unavailable"))?;
        let deleted = state
            .repo
            .delete_corrupt_port_forward_rule(
                rule_id,
                request.expected_revision,
                request.reason.as_deref(),
                &configuration_error,
                &operator,
            )
            .await
            .map_err(port_forward_repository_error)?;
        let sync = if !identity.enabled && identity.revision == 1 {
            no_sync("retired_disabled_draft")
        } else {
            sync_client(
                &state,
                &operator,
                &identity.client_id,
                "port_forward_rule_deleted",
            )
            .await
        };
        return Ok(Json(PortForwardMutationResponse {
            rule: PortForwardRuleListItem::Corrupt(Box::new(deleted)),
            sync,
        }));
    }
    let existing = existing.expect("checked above");
    let deleted = state
        .repo
        .delete_port_forward_rule(
            rule_id,
            request.expected_revision,
            request.reason.as_deref(),
            &operator,
        )
        .await
        .map_err(port_forward_repository_error)?;
    let sync = if is_never_applied_disabled_draft(&existing) {
        no_sync("retired_disabled_draft")
    } else {
        sync_client(
            &state,
            &operator,
            &existing.client_id,
            "port_forward_rule_deleted",
        )
        .await
    };
    let rule = state
        .repo
        .get_port_forward_rule(rule_id)
        .await
        .map_err(ApiError::internal_mapper(
            "port_forward_rule_unavailable",
            "The port-forwarding rule could not be loaded.",
        ))?
        .unwrap_or(deleted);
    Ok(Json(PortForwardMutationResponse {
        rule: rule.into(),
        sync,
    }))
}

pub(crate) async fn forget_port_forward_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(rule_id): Path<Uuid>,
    Json(request): Json<PortForwardMutationRequest>,
) -> Result<Json<PortForwardMutationResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "admin", "network:write")
        .await?;
    require_confirmed(request.confirmed)?;
    let rule = state
        .repo
        .forget_port_forward_rule(
            rule_id,
            request.expected_revision,
            request.reason.as_deref(),
            &operator,
        )
        .await
        .map_err(port_forward_repository_error)?;
    Ok(Json(PortForwardMutationResponse {
        rule: rule.into(),
        sync: no_sync("forgotten_without_host_cleanup"),
    }))
}

pub(crate) async fn reapply_port_forward_rule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(rule_id): Path<Uuid>,
    Json(request): Json<PortForwardMutationRequest>,
) -> Result<Json<PortForwardMutationResponse>, ApiError> {
    let operator = network_writer(&state, &headers).await?;
    require_confirmed(request.confirmed)?;
    let rule = required_active_rule(&state, rule_id).await?;
    if rule.revision != request.expected_revision {
        return Err(ApiError::conflict("port_forward_rule_snapshot_stale"));
    }
    require_agent(&state, &rule.client_id, true).await?;
    let sync = sync_client(
        &state,
        &operator,
        &rule.client_id,
        "port_forward_table_reapply",
    )
    .await;
    Ok(Json(PortForwardMutationResponse {
        rule: rule.into(),
        sync,
    }))
}

pub(crate) async fn bulk_mutate_port_forward_rules(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PortForwardBulkRequest>,
) -> Result<Json<PortForwardBulkResponse>, ApiError> {
    let operator = network_writer(&state, &headers).await?;
    require_confirmed(request.confirmed)?;
    if request.items.is_empty() || request.items.len() > vpsman_common::MAX_PORT_FORWARD_RULES {
        return Err(ApiError::bad_request("port_forward_bulk_items_invalid"));
    }
    let before = state
        .repo
        .list_port_forward_rules()
        .await
        .map_err(ApiError::internal_mapper(
            "port_forward_rules_unavailable",
            "Port-forwarding rules could not be loaded.",
        ))?;
    let selected_ids = request
        .items
        .iter()
        .map(|item| item.id)
        .collect::<BTreeSet<_>>();
    let selected = before
        .iter()
        .filter(|rule| selected_ids.contains(&rule.id))
        .cloned()
        .collect::<Vec<_>>();
    if selected.len() != request.items.len() {
        return Err(ApiError::not_found("port_forward_rule_not_found"));
    }
    if matches!(
        request.action,
        PortForwardBulkAction::Enable | PortForwardBulkAction::Reapply
    ) {
        let client_ids = selected
            .iter()
            .map(|rule| rule.client_id.clone())
            .collect::<BTreeSet<_>>();
        require_agents(&state, &client_ids, true).await?;
    }
    let rules = state
        .repo
        .bulk_mutate_port_forward_rules(
            request.action,
            &request.items,
            request.reason.as_deref(),
            &operator,
        )
        .await
        .map_err(port_forward_repository_error)?;
    let client_ids = selected
        .iter()
        .map(|rule| rule.client_id.clone())
        .collect::<BTreeSet<_>>();
    let sync_client_ids = client_ids
        .iter()
        .filter(|client_id| {
            !matches!(request.action, PortForwardBulkAction::Delete)
                || selected.iter().any(|rule| {
                    rule.client_id == client_id.as_str() && !is_never_applied_disabled_draft(rule)
                })
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut dispatched = dispatch_runtime_config_for_clients(
        &state,
        &operator,
        sync_client_ids.iter().cloned(),
        bulk_sync_reason(request.action),
    )
    .await
    .into_iter()
    .map(|outcome| {
        (
            outcome.client_id.clone(),
            PortForwardSyncView {
                status: outcome.status,
                job_id: outcome.job_id,
                error: outcome.error,
            },
        )
    })
    .collect::<BTreeMap<_, _>>();
    let mut sync = Vec::with_capacity(client_ids.len());
    for client_id in client_ids {
        let result = if sync_client_ids.contains(&client_id) {
            dispatched
                .remove(&client_id)
                .unwrap_or_else(|| PortForwardSyncView {
                    status: "not_queued".to_string(),
                    job_id: None,
                    error: Some("VPS is no longer available".to_string()),
                })
        } else {
            no_sync("retired_disabled_draft")
        };
        sync.push(PortForwardClientSyncView {
            client_id,
            sync: result,
        });
    }
    Ok(Json(PortForwardBulkResponse { rules, sync }))
}

pub(crate) async fn resolve_network_hostname(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ResolveHostnameRequest>,
) -> Result<Json<ResolveHostnameResponse>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_NETWORK_READ)
        .await?;
    let hostname = normalize_port_forward_hostname(&request.hostname)
        .ok_or_else(|| ApiError::bad_request("hostname_invalid"))?;
    let resolved = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::lookup_host((hostname.as_str(), 0)),
    )
    .await
    .map_err(|_| ApiError::bad_request("hostname_resolution_timeout"))?
    .map_err(|_| ApiError::bad_request("hostname_resolution_failed"))?;
    let mut seen = BTreeSet::<IpAddr>::new();
    let candidates = resolved
        .map(|address| address.ip())
        .filter(|address| vpsman_common::validate_target_ip(*address).is_ok())
        .filter(|address| seen.insert(*address))
        .take(32)
        .map(|address| ResolvedAddressView {
            address,
            family: if address.is_ipv4() { "ipv4" } else { "ipv6" },
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err(ApiError::bad_request("hostname_resolution_no_addresses"));
    }
    Ok(Json(ResolveHostnameResponse {
        hostname,
        candidates,
    }))
}

async fn mutate_enabled(
    state: AppState,
    headers: HeaderMap,
    rule_id: Uuid,
    request: PortForwardMutationRequest,
    enabled: bool,
) -> Result<Json<PortForwardMutationResponse>, ApiError> {
    let operator = network_writer(&state, &headers).await?;
    require_confirmed(request.confirmed)?;
    let existing = required_rule(&state, rule_id).await?;
    if existing.revision != request.expected_revision {
        return Err(ApiError::conflict("port_forward_rule_snapshot_stale"));
    }
    if existing.deleted_at.is_some() {
        return Err(ApiError::not_found("port_forward_rule_not_found"));
    }
    if enabled {
        require_agent(&state, &existing.client_id, true).await?;
    }
    if existing.enabled == enabled {
        return Ok(Json(PortForwardMutationResponse {
            rule: existing.into(),
            sync: no_sync("already_in_requested_state"),
        }));
    }
    let changed = state
        .repo
        .set_port_forward_rule_enabled(rule_id, request.expected_revision, enabled, &operator)
        .await
        .map_err(port_forward_repository_error)?;
    let sync = sync_client(
        &state,
        &operator,
        &changed.client_id,
        if enabled {
            "port_forward_rule_enabled"
        } else {
            "port_forward_rule_disabled"
        },
    )
    .await;
    let rule = state
        .repo
        .get_port_forward_rule(rule_id)
        .await
        .map_err(ApiError::internal_mapper(
            "port_forward_rule_unavailable",
            "The port-forwarding rule could not be loaded.",
        ))?
        .unwrap_or(changed);
    Ok(Json(PortForwardMutationResponse {
        rule: rule.into(),
        sync,
    }))
}

async fn network_writer(state: &AppState, headers: &HeaderMap) -> Result<AuthContext, ApiError> {
    state
        .require_operator_role_and_scope(headers, "operator", "network:write")
        .await
}

async fn required_rule(state: &AppState, id: Uuid) -> Result<PortForwardRuleView, ApiError> {
    state
        .repo
        .get_port_forward_rule(id)
        .await
        .map_err(ApiError::internal_mapper(
            "port_forward_rule_unavailable",
            "The port-forwarding rule could not be loaded.",
        ))?
        .ok_or_else(|| ApiError::not_found("port_forward_rule_not_found"))
}

async fn required_active_rule(state: &AppState, id: Uuid) -> Result<PortForwardRuleView, ApiError> {
    let rule = required_rule(state, id).await?;
    if rule.deleted_at.is_some() {
        Err(ApiError::not_found("port_forward_rule_not_found"))
    } else {
        Ok(rule)
    }
}

async fn require_agent(
    state: &AppState,
    client_id: &str,
    require_capability: bool,
) -> Result<(), ApiError> {
    require_agents(
        state,
        &BTreeSet::from([client_id.to_string()]),
        require_capability,
    )
    .await
}

async fn require_agents(
    state: &AppState,
    client_ids: &BTreeSet<String>,
    require_capability: bool,
) -> Result<(), ApiError> {
    let requested = client_ids.iter().cloned().collect::<Vec<_>>();
    let agents = state
        .repo
        .list_agents_for_client_ids(&requested)
        .await
        .map_err(ApiError::internal_mapper(
            "vps_inventory_unavailable",
            "The VPS inventory could not be loaded.",
        ))?
        .into_iter()
        .map(|agent| (agent.id.clone(), agent))
        .collect::<BTreeMap<_, _>>();
    for client_id in client_ids {
        let agent = agents
            .get(client_id)
            .ok_or_else(|| ApiError::bad_request("port_forward_agent_not_found"))?;
        if !require_capability || agent.capabilities.port_forwarding.supported() {
            continue;
        }
        let capability = &agent.capabilities.port_forwarding;
        let reason = capability.reason.clone().unwrap_or_else(|| {
            format!(
                "VPS {client_id} reports port-forwarding capability as {:?}",
                capability.status
            )
        });
        return Err(ApiError::conflict_with_message(
            "port_forward_agent_capability_required",
            reason,
        ));
    }
    Ok(())
}

async fn sync_client(
    state: &AppState,
    operator: &AuthContext,
    client_id: &str,
    reason: &str,
) -> PortForwardSyncView {
    dispatch_runtime_config_for_clients(state, operator, [client_id.to_string()], reason)
        .await
        .pop()
        .map_or_else(
            || PortForwardSyncView {
                status: "not_queued".to_string(),
                job_id: None,
                error: Some("VPS is no longer available".to_string()),
            },
            |outcome| PortForwardSyncView {
                status: outcome.status,
                job_id: outcome.job_id,
                error: outcome.error,
            },
        )
}

fn no_sync(status: &str) -> PortForwardSyncView {
    PortForwardSyncView {
        status: status.to_string(),
        job_id: None,
        error: None,
    }
}

fn is_never_applied_disabled_draft(rule: &PortForwardRuleView) -> bool {
    !rule.enabled && rule.revision == 1 && rule.deleted_at.is_none()
}

fn require_confirmed(confirmed: bool) -> Result<(), ApiError> {
    if confirmed {
        Ok(())
    } else {
        Err(ApiError::conflict(
            "port_forward_mutation_confirmation_required",
        ))
    }
}

fn validate_confirmation(applies_to_host: bool, confirmed: bool) -> Result<(), ApiError> {
    if applies_to_host {
        require_confirmed(confirmed)
    } else {
        Ok(())
    }
}

fn bulk_sync_reason(action: PortForwardBulkAction) -> &'static str {
    match action {
        PortForwardBulkAction::Enable => "port_forward_bulk_enabled",
        PortForwardBulkAction::Disable => "port_forward_bulk_disabled",
        PortForwardBulkAction::Reapply => "port_forward_bulk_reapply",
        PortForwardBulkAction::Delete => "port_forward_bulk_deleted",
    }
}

fn port_forward_repository_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("port_forward_client_inactive") {
        ApiError::conflict("port_forward_agent_unavailable")
    } else if message.contains("not_found") || message.contains("not_active") {
        ApiError::not_found("port_forward_rule_not_found")
    } else if message.contains("snapshot_stale") {
        ApiError::conflict("port_forward_rule_snapshot_stale")
    } else if message.contains("port_forward_target_hostname_invalid") {
        ApiError::bad_request("hostname_invalid")
    } else if message.contains("name_conflict")
        || message.contains("port_forward_rules_active_name_idx")
    {
        ApiError::conflict("port_forward_rule_name_conflict")
    } else if message.contains("overlap") {
        ApiError::conflict("port_forward_port_claim_conflict")
    } else if message.contains("not_removal_pending") {
        ApiError::conflict("port_forward_rule_not_removal_pending")
    } else if message.contains("reason_required") {
        ApiError::bad_request("port_forward_forget_reason_required")
    } else if message.contains("invalid") || message.contains("limit") || message.contains("empty")
    {
        ApiError::bad_request_with_message("port_forward_rule_invalid", message)
    } else {
        ApiError::internal(
            "port_forward_rule_mutation_failed",
            "The port-forwarding rule change could not be completed.",
            error,
        )
    }
}
