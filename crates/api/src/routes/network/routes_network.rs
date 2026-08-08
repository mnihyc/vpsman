use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use tracing::warn;
use uuid::Uuid;
use vpsman_common::{
    allocate_tunnel_endpoints as allocate_tunnel_endpoint_pairs, payload_hash, plan_tunnel,
    routing_cost_update_privilege_payload, JobCommand, NetworkPlanError,
    RoutingCostAdapterCommands, RuntimeTunnelCommand, RuntimeTunnelManager, TunnelAddressFamily,
    TunnelEndpointSide, TunnelPlan,
};

use crate::{
    error::ApiError,
    model::{
        AllocateTunnelEndpointsRequest, AllocateTunnelEndpointsResponse, CreateJobRequest,
        CreateJobResponse, CreateTunnelPlanRequest, HistoryQuery, NetworkEvidenceQuery,
        NetworkOspfRecommendationView, NetworkOspfUpdatePlanView,
        RefreshTunnelPlanOspfStatusRequest, RuntimeConfigDispatchView,
        TunnelPlanEndpointRuntimeConfigView, TunnelPlanListItem, TunnelPlanMutationResponse,
        TunnelPlanOspfDispatchView, TunnelPlanOspfJobsResponse, TunnelPlanView,
        UpdateTunnelConnectionAssessmentRequest, UpdateTunnelPlanOspfCostRequest,
        UpdateTunnelPlanRequest,
    },
    model_topology::TopologyGraphView,
    privilege::{verify_privilege_intent, DbPrivilegeIntent},
    repository_configuration_presets::validate_network_adapter_definition_view,
    repository_topology_graph::TopologyGraphStageError,
    routes_job_history::network_observation_filter,
    routes_jobs::create_job_from_internal_operator_mutation,
    runtime_config::{dispatch_runtime_config_for_clients, operator_dispatch_error},
    security::{SCOPE_FLEET_READ, SCOPE_NETWORK_READ},
    state::AppState,
    util::limit_or_default,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TunnelPlanMutationRequest {
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) expected_revision: i64,
}

pub(crate) async fn list_tunnel_plans(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<TunnelPlanListItem>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_NETWORK_READ)
        .await?;
    Ok(Json(state.repo.list_tunnel_plan_items().await.map_err(
        ApiError::internal_mapper(
            "tunnel_plans_unavailable",
            "Tunnel plans could not be loaded.",
        ),
    )?))
}

pub(crate) async fn create_tunnel_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateTunnelPlanRequest>,
) -> Result<(StatusCode, Json<TunnelPlanMutationResponse>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    require_tunnel_plan_confirmed(request.confirmed)?;
    let plan = plan_tunnel(&request.input)
        .map_err(|error| ApiError::bad_request(tunnel_plan_error_code(error)))?;
    require_tunnel_endpoint_agents(
        &state,
        &request.input.left_client_id,
        &request.input.right_client_id,
    )
    .await?;
    if state
        .repo
        .list_tunnel_plans()
        .await
        .map_err(ApiError::internal_mapper(
            "tunnel_plans_unavailable",
            "Tunnel plans could not be loaded.",
        ))?
        .iter()
        .any(|plan| plan.name == request.input.name)
    {
        return Err(ApiError::conflict("tunnel_plan_name_conflict"));
    }
    state
        .repo
        .validate_tunnel_plan_resource_conflicts(&plan, None)
        .await
        .map_err(tunnel_plan_repository_error)?;
    validate_tunnel_plan_adapter_bindings(&state, &plan).await?;
    let view = state
        .repo
        .record_tunnel_plan(&request.input, &plan, request.enabled, &operator)
        .await
        .map_err(tunnel_plan_repository_error)?;
    let mut sync_client_ids = Vec::new();
    if view.enabled {
        sync_client_ids.push(view.left_client_id.clone());
        sync_client_ids.push(view.right_client_id.clone());
    }
    let sync = if !sync_client_ids.is_empty() {
        let reason = if view.enabled {
            "tunnel_plan_saved_enabled"
        } else {
            "tunnel_plan_saved_disabled"
        };
        dispatch_runtime_config_for_clients(&state, &operator, sync_client_ids, reason).await
    } else {
        Vec::new()
    };
    let plan = state
        .repo
        .get_tunnel_plan(view.id)
        .await
        .map_err(tunnel_plan_unavailable)?
        .unwrap_or(view);
    Ok((
        StatusCode::CREATED,
        Json(TunnelPlanMutationResponse { plan, sync }),
    ))
}

pub(crate) async fn update_tunnel_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(plan_id): Path<Uuid>,
    Json(request): Json<UpdateTunnelPlanRequest>,
) -> Result<Json<TunnelPlanMutationResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    require_tunnel_plan_confirmed(request.confirmed)?;
    let identity = state
        .repo
        .get_tunnel_plan_identity(plan_id)
        .await
        .map_err(ApiError::internal_mapper(
            "tunnel_plan_unavailable",
            "The tunnel plan could not be loaded.",
        ))?
        .ok_or_else(|| ApiError::not_found("tunnel_plan_not_found"))?;
    if identity.revision != request.expected_revision {
        return Err(ApiError::conflict("tunnel_plan_snapshot_stale"));
    }
    if identity.name != request.input.name {
        return Err(ApiError::bad_request("tunnel_plan_name_is_immutable"));
    }
    let enabled = request.enabled.unwrap_or(identity.enabled);
    let plan = plan_tunnel(&request.input)
        .map_err(|error| ApiError::bad_request(tunnel_plan_error_code(error)))?;
    match state.repo.get_tunnel_plan(plan_id).await {
        Ok(Some(existing)) if existing.enabled == enabled && existing.input == request.input => {
            return Ok(Json(TunnelPlanMutationResponse {
                plan: existing,
                sync: Vec::new(),
            }));
        }
        Ok(_) => {}
        Err(error) if error.to_string().starts_with("invalid persisted tunnel") => {
            warn!(
                event = "tunnel_plan_configuration_replacement",
                %plan_id,
                error = %error,
                "allowing reviewed full replacement of malformed tunnel configuration"
            );
        }
        Err(error) => return Err(tunnel_plan_unavailable(error)),
    }
    require_tunnel_endpoint_agents(
        &state,
        &request.input.left_client_id,
        &request.input.right_client_id,
    )
    .await?;
    state
        .repo
        .validate_tunnel_plan_resource_conflicts(&plan, Some(plan_id))
        .await
        .map_err(tunnel_plan_repository_error)?;
    validate_tunnel_plan_adapter_bindings(&state, &plan).await?;
    let view = state
        .repo
        .update_tunnel_plan(
            plan_id,
            request.expected_revision,
            &request.input,
            &plan,
            enabled,
            &operator,
        )
        .await
        .map_err(tunnel_plan_repository_error)?;
    let mut sync_client_ids = Vec::new();
    if identity.enabled {
        sync_client_ids.push(identity.left_client_id);
        sync_client_ids.push(identity.right_client_id);
    }
    if view.enabled {
        sync_client_ids.push(view.left_client_id.clone());
        sync_client_ids.push(view.right_client_id.clone());
    }
    let sync = if !sync_client_ids.is_empty() {
        dispatch_runtime_config_for_clients(
            &state,
            &operator,
            sync_client_ids,
            "tunnel_plan_updated",
        )
        .await
    } else {
        Vec::new()
    };
    let plan = state
        .repo
        .get_tunnel_plan(view.id)
        .await
        .map_err(tunnel_plan_unavailable)?
        .unwrap_or(view);
    Ok(Json(TunnelPlanMutationResponse { plan, sync }))
}

pub(crate) async fn allocate_tunnel_endpoints(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<AllocateTunnelEndpointsRequest>,
) -> Result<Json<AllocateTunnelEndpointsResponse>, ApiError> {
    let _operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    let mut reserved_addresses = request.reserved_addresses.clone();
    for plan in state
        .repo
        .list_tunnel_plans()
        .await
        .map_err(ApiError::internal_mapper(
            "tunnel_plans_unavailable",
            "Tunnel plans could not be loaded.",
        ))?
    {
        if let Some(pair) = plan.plan.ipv4_tunnel {
            reserved_addresses.push(pair.left);
            reserved_addresses.push(pair.right);
        }
        if let Some(pair) = plan.plan.ipv6_tunnel {
            reserved_addresses.push(pair.left);
            reserved_addresses.push(pair.right);
        }
    }
    let (configured_ipv4_pool, configured_ipv6_pool) = state.tunnel_allocation_pool_cidrs();
    let ipv4_pool = normalize_optional_string(request.ipv4_pool_cidr);
    let ipv6_pool = normalize_optional_string(request.ipv6_pool_cidr);
    let explicit_request = ipv4_pool.is_some()
        || ipv6_pool.is_some()
        || request.include_ipv4.is_some()
        || request.include_ipv6.is_some();
    let resolved_ipv4 = resolve_allocation_family(
        request.include_ipv4,
        ipv4_pool,
        configured_ipv4_pool,
        explicit_request,
        "ipv4_allocation_pool_required",
    )?;
    let resolved_ipv6 = resolve_allocation_family(
        request.include_ipv6,
        ipv6_pool,
        configured_ipv6_pool,
        explicit_request,
        "ipv6_allocation_pool_required",
    )?;
    if resolved_ipv4.is_none() && resolved_ipv6.is_none() {
        return Ok(Json(AllocateTunnelEndpointsResponse {
            ipv4_tunnel: None,
            ipv6_tunnel: None,
            latency_primary_family: TunnelAddressFamily::Ipv4,
            conflicts: Vec::new(),
        }));
    }
    let allocation = allocate_tunnel_endpoint_pairs(
        resolved_ipv4.as_deref(),
        resolved_ipv6.as_deref(),
        &reserved_addresses,
        resolved_ipv4.is_some(),
        resolved_ipv6.is_some(),
    )
    .map_err(|error| ApiError::bad_request(tunnel_plan_error_code(error)))?;
    Ok(Json(AllocateTunnelEndpointsResponse {
        ipv4_tunnel: allocation.ipv4_tunnel,
        ipv6_tunnel: allocation.ipv6_tunnel,
        latency_primary_family: allocation.latency_primary_family,
        conflicts: Vec::new(),
    }))
}

pub(crate) async fn export_tunnel_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(plan_id): Path<Uuid>,
) -> Result<Json<TunnelPlan>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_NETWORK_READ)
        .await?;
    let Some(view) = state
        .repo
        .get_tunnel_plan(plan_id)
        .await
        .map_err(tunnel_plan_unavailable)?
    else {
        return Err(ApiError::not_found("tunnel_plan_not_found"));
    };
    Ok(Json(view.plan))
}

pub(crate) async fn rotate_tunnel_plan_credentials(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(plan_id): Path<Uuid>,
    Json(request): Json<TunnelPlanMutationRequest>,
) -> Result<Json<TunnelPlanMutationResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    require_tunnel_plan_confirmed(request.confirmed)?;
    let rotated = state
        .repo
        .rotate_tunnel_plan_credentials(plan_id, request.expected_revision, &operator)
        .await
        .map_err(tunnel_plan_repository_error)?;
    let sync = if rotated.enabled {
        dispatch_runtime_config_for_clients(
            &state,
            &operator,
            vec![
                rotated.left_client_id.clone(),
                rotated.right_client_id.clone(),
            ],
            "tunnel_plan_credentials_rotated",
        )
        .await
    } else {
        Vec::new()
    };
    let plan = state
        .repo
        .get_tunnel_plan(plan_id)
        .await
        .map_err(ApiError::internal_mapper(
            "tunnel_plan_unavailable",
            "The tunnel plan could not be loaded.",
        ))?
        .unwrap_or(rotated);
    Ok(Json(TunnelPlanMutationResponse { plan, sync }))
}

pub(crate) async fn enable_tunnel_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(plan_id): Path<Uuid>,
    Json(request): Json<TunnelPlanMutationRequest>,
) -> Result<Json<TunnelPlanMutationResponse>, ApiError> {
    mutate_tunnel_plan_enabled(state, headers, plan_id, request, true).await
}

pub(crate) async fn disable_tunnel_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(plan_id): Path<Uuid>,
    Json(request): Json<TunnelPlanMutationRequest>,
) -> Result<Json<TunnelPlanMutationResponse>, ApiError> {
    mutate_tunnel_plan_enabled(state, headers, plan_id, request, false).await
}

pub(crate) async fn delete_tunnel_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(plan_id): Path<Uuid>,
    Json(request): Json<TunnelPlanMutationRequest>,
) -> Result<Json<TunnelPlanMutationResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    require_tunnel_plan_confirmed(request.confirmed)?;
    let existing = state
        .repo
        .get_tunnel_plan(plan_id)
        .await
        .map_err(ApiError::internal_mapper(
            "tunnel_plan_unavailable",
            "The tunnel plan could not be loaded.",
        ))?
        .ok_or_else(|| ApiError::not_found("tunnel_plan_not_found"))?;
    if existing.revision != request.expected_revision {
        return Err(ApiError::conflict("tunnel_plan_snapshot_stale"));
    }
    let mut deleted = state
        .repo
        .delete_tunnel_plan(plan_id, request.expected_revision, &operator)
        .await
        .map_err(tunnel_plan_repository_error)?;
    let sync = dispatch_runtime_config_for_clients(
        &state,
        &operator,
        vec![
            deleted.left_client_id.clone(),
            deleted.right_client_id.clone(),
        ],
        "tunnel_plan_deleted",
    )
    .await;
    deleted.left_runtime_config =
        retired_endpoint_runtime_config(&deleted.left_client_id, &sync, &deleted.updated_at);
    deleted.right_runtime_config =
        retired_endpoint_runtime_config(&deleted.right_client_id, &sync, &deleted.updated_at);
    Ok(Json(TunnelPlanMutationResponse {
        plan: deleted,
        sync,
    }))
}

fn retired_endpoint_runtime_config(
    client_id: &str,
    sync: &[RuntimeConfigDispatchView],
    updated_at: &str,
) -> TunnelPlanEndpointRuntimeConfigView {
    let outcome = sync.iter().find(|outcome| outcome.client_id == client_id);
    TunnelPlanEndpointRuntimeConfigView {
        client_id: client_id.to_string(),
        desired: "absent".to_string(),
        status: outcome
            .map(|outcome| match outcome.status.as_str() {
                "queue_failed" => "failed",
                "not_queued" => "not_dispatched",
                status => status,
            })
            .unwrap_or("not_dispatched")
            .to_string(),
        job_id: outcome.and_then(|outcome| outcome.job_id),
        error: outcome.and_then(|outcome| outcome.error.clone()),
        updated_at: Some(updated_at.to_string()),
    }
}

pub(crate) async fn update_tunnel_connection_assessment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(plan_id): Path<Uuid>,
    Json(request): Json<UpdateTunnelConnectionAssessmentRequest>,
) -> Result<Json<TunnelPlanView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    let existing = state
        .repo
        .get_tunnel_plan(plan_id)
        .await
        .map_err(ApiError::internal_mapper(
            "tunnel_plan_unavailable",
            "The tunnel plan could not be loaded.",
        ))?
        .ok_or_else(|| ApiError::not_found("tunnel_plan_not_found"))?;
    if existing.revision != request.expected_revision {
        return Err(ApiError::conflict("tunnel_plan_snapshot_stale"));
    }
    if !existing.enabled && request.assessment.trim() != "automatic" {
        return Err(ApiError::conflict(
            "tunnel_connection_assessment_requires_enabled_plan",
        ));
    }
    state
        .repo
        .update_tunnel_connection_assessment(
            plan_id,
            request.expected_revision,
            &request.assessment,
            request.note.as_deref(),
            &operator,
        )
        .await
        .map(Json)
        .map_err(tunnel_plan_repository_error)
}

pub(crate) async fn update_tunnel_plan_ospf_cost(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(plan_id): Path<Uuid>,
    Json(request): Json<UpdateTunnelPlanOspfCostRequest>,
) -> Result<Json<TunnelPlanOspfJobsResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    validate_tunnel_plan_ospf_cost_request(&request)?;
    let existing = state
        .repo
        .get_tunnel_plan(plan_id)
        .await
        .map_err(ApiError::internal_mapper(
            "tunnel_plan_unavailable",
            "The tunnel plan could not be loaded.",
        ))?
        .ok_or_else(|| ApiError::bad_request("tunnel_plan_not_found"))?;
    require_tunnel_ospf_enabled(&existing)?;
    if existing.revision != request.plan_revision {
        return Err(ApiError::conflict("tunnel_plan_ospf_snapshot_stale"));
    }
    if existing.left_ospf_status != "verified" || existing.right_ospf_status != "verified" {
        return Err(ApiError::conflict(
            "tunnel_plan_ospf_status_verification_required",
        ));
    }
    require_tunnel_endpoint_agents(&state, &existing.left_client_id, &existing.right_client_id)
        .await?;
    validate_ospf_recommendation_contract(&state, plan_id, &request).await?;
    if existing.left_current_ospf_cost != request.left_current_ospf_cost.map(i32::from)
        || existing.right_current_ospf_cost != request.right_current_ospf_cost.map(i32::from)
    {
        return Err(ApiError::conflict("tunnel_plan_ospf_snapshot_stale"));
    }
    let (left_adapter, right_adapter) = resolve_plan_routing_adapters(&state, &existing).await?;
    if left_adapter.definition_hash != request.left_adapter_definition_hash
        || right_adapter.definition_hash != request.right_adapter_definition_hash
    {
        return Err(ApiError::conflict(
            "routing_cost_adapter_confirmation_stale",
        ));
    }
    let target_client_ids = vec![
        existing.left_client_id.clone(),
        existing.right_client_id.clone(),
    ];
    let target = tunnel_plan_privilege_target(plan_id);
    let privilege_payload_hash = tunnel_plan_ospf_cost_payload_hash(plan_id, &request);
    let privilege_intent = DbPrivilegeIntent::new(
        "network.ospf_cost.apply",
        &target,
        None,
        &target_client_ids,
        true,
        Some(&privilege_payload_hash),
    );
    verify_privilege_intent(
        &state,
        &privilege_intent,
        request.privilege_assertion.clone(),
    )
    .await?;
    let left_job_id = Uuid::new_v4();
    let right_job_id = Uuid::new_v4();
    let plan = state
        .repo
        .stage_tunnel_plan_ospf_jobs(
            plan_id,
            request.plan_revision,
            request.left_current_ospf_cost,
            request.right_current_ospf_cost,
            Some(request.desired_ospf_cost),
            left_job_id,
            right_job_id,
            &operator,
        )
        .await
        .map_err(tunnel_plan_mutation_error)?;
    let (jobs, dispatch) = dispatch_routing_jobs(
        &state,
        &operator,
        &plan,
        left_job_id,
        right_job_id,
        left_adapter,
        right_adapter,
        Some((
            request.left_current_ospf_cost,
            request.right_current_ospf_cost,
            request.desired_ospf_cost,
        )),
    )
    .await;
    let plan = state
        .repo
        .get_tunnel_plan(plan.id)
        .await
        .map_err(tunnel_plan_unavailable)?
        .unwrap_or(plan);
    Ok(Json(TunnelPlanOspfJobsResponse {
        plan,
        jobs,
        dispatch,
    }))
}

pub(crate) async fn refresh_tunnel_plan_ospf_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(plan_id): Path<Uuid>,
    Json(_request): Json<RefreshTunnelPlanOspfStatusRequest>,
) -> Result<Json<TunnelPlanOspfJobsResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    let existing = state
        .repo
        .get_tunnel_plan(plan_id)
        .await
        .map_err(ApiError::internal_mapper(
            "tunnel_plan_unavailable",
            "The tunnel plan could not be loaded.",
        ))?
        .ok_or_else(|| ApiError::not_found("tunnel_plan_not_found"))?;
    require_tunnel_ospf_enabled(&existing)?;
    require_tunnel_endpoint_agents(&state, &existing.left_client_id, &existing.right_client_id)
        .await?;
    let (left_adapter, right_adapter) = resolve_plan_routing_adapters(&state, &existing).await?;
    let left_job_id = Uuid::new_v4();
    let right_job_id = Uuid::new_v4();
    let plan = state
        .repo
        .stage_tunnel_plan_ospf_jobs(
            plan_id,
            existing.revision,
            existing.left_current_ospf_cost.map(|value| value as u16),
            existing.right_current_ospf_cost.map(|value| value as u16),
            None,
            left_job_id,
            right_job_id,
            &operator,
        )
        .await
        .map_err(tunnel_plan_mutation_error)?;
    let (jobs, dispatch) = dispatch_routing_jobs(
        &state,
        &operator,
        &plan,
        left_job_id,
        right_job_id,
        left_adapter,
        right_adapter,
        None,
    )
    .await;
    let plan = state
        .repo
        .get_tunnel_plan(plan.id)
        .await
        .map_err(tunnel_plan_unavailable)?
        .unwrap_or(plan);
    Ok(Json(TunnelPlanOspfJobsResponse {
        plan,
        jobs,
        dispatch,
    }))
}

async fn mutate_tunnel_plan_enabled(
    state: AppState,
    headers: HeaderMap,
    plan_id: Uuid,
    request: TunnelPlanMutationRequest,
    enabled: bool,
) -> Result<Json<TunnelPlanMutationResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    require_tunnel_plan_confirmed(request.confirmed)?;
    let existing = state
        .repo
        .get_tunnel_plan(plan_id)
        .await
        .map_err(ApiError::internal_mapper(
            "tunnel_plan_unavailable",
            "The tunnel plan could not be loaded.",
        ))?
        .ok_or_else(|| ApiError::bad_request("tunnel_plan_not_found"))?;
    if existing.revision != request.expected_revision {
        return Err(ApiError::conflict("tunnel_plan_snapshot_stale"));
    }
    if enabled {
        require_tunnel_endpoint_agents(&state, &existing.left_client_id, &existing.right_client_id)
            .await?;
        validate_tunnel_plan_adapter_bindings(&state, &existing.plan).await?;
    }
    let view = if existing.enabled == enabled {
        existing
    } else {
        state
            .repo
            .set_tunnel_plan_enabled(plan_id, request.expected_revision, enabled, &operator)
            .await
            .map_err(tunnel_plan_repository_error)?
    };
    let reason = if enabled {
        "tunnel_plan_enabled"
    } else {
        "tunnel_plan_disabled"
    };
    let sync = dispatch_runtime_config_for_clients(
        &state,
        &operator,
        vec![view.left_client_id.clone(), view.right_client_id.clone()],
        reason,
    )
    .await;
    let plan = state
        .repo
        .get_tunnel_plan(view.id)
        .await
        .map_err(tunnel_plan_unavailable)?
        .unwrap_or(view);
    Ok(Json(TunnelPlanMutationResponse { plan, sync }))
}

fn require_tunnel_plan_confirmed(confirmed: bool) -> Result<(), ApiError> {
    if confirmed {
        Ok(())
    } else {
        Err(ApiError::conflict(
            "tunnel_plan_mutation_requires_confirmation",
        ))
    }
}

async fn require_tunnel_endpoint_agents(
    state: &AppState,
    left_client_id: &str,
    right_client_id: &str,
) -> Result<(), ApiError> {
    let agents = state
        .repo
        .list_agents()
        .await
        .map_err(ApiError::internal_mapper(
            "vps_inventory_unavailable",
            "The VPS inventory could not be loaded.",
        ))?;
    let has_left = agents.iter().any(|agent| agent.id == left_client_id);
    let has_right = agents.iter().any(|agent| agent.id == right_client_id);
    if !has_left || !has_right {
        return Err(ApiError::bad_request(
            "tunnel_plan_endpoint_agent_not_found",
        ));
    }
    Ok(())
}

async fn validate_tunnel_plan_adapter_bindings(
    state: &AppState,
    plan: &TunnelPlan,
) -> Result<(), ApiError> {
    if plan.runtime_control.manager == RuntimeTunnelManager::CustomAdapter {
        let left_id = plan
            .runtime_control
            .left_adapter_definition_id
            .as_deref()
            .ok_or_else(|| ApiError::bad_request("runtime_tunnel_adapter_definition_required"))?;
        let right_id = plan
            .runtime_control
            .right_adapter_definition_id
            .as_deref()
            .ok_or_else(|| ApiError::bad_request("runtime_tunnel_adapter_definition_required"))?;
        let left = state
            .repo
            .resolve_runtime_tunnel_adapter(left_id)
            .await
            .map_err(|_| ApiError::conflict("runtime_tunnel_left_adapter_unavailable"))?;
        let right = state
            .repo
            .resolve_runtime_tunnel_adapter(right_id)
            .await
            .map_err(|_| ApiError::conflict("runtime_tunnel_right_adapter_unavailable"))?;
        if !plan.runtime_control.traffic_limit.is_default()
            && (left.traffic_limit_apply.is_none() || right.traffic_limit_apply.is_none())
        {
            return Err(ApiError::conflict(
                "runtime_tunnel_adapter_traffic_limit_unsupported",
            ));
        }
    }
    if plan.ospf.is_some() {
        resolve_routing_adapters_for_tunnel_plan(state, plan).await?;
    }
    Ok(())
}

fn validate_tunnel_plan_ospf_cost_request(
    request: &UpdateTunnelPlanOspfCostRequest,
) -> Result<(), ApiError> {
    require_tunnel_plan_confirmed(request.confirmed)?;
    if request.plan_revision < 1 {
        return Err(ApiError::bad_request("tunnel_plan_ospf_revision_invalid"));
    }
    if request.recommendation_id.trim().is_empty() {
        return Err(ApiError::bad_request(
            "tunnel_plan_ospf_recommendation_id_required",
        ));
    }
    for hash in [
        &request.left_adapter_definition_hash,
        &request.right_adapter_definition_hash,
    ] {
        if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ApiError::bad_request(
                "routing_cost_adapter_definition_hash_invalid",
            ));
        }
    }
    if request.desired_ospf_cost == 0 {
        return Err(ApiError::bad_request("tunnel_plan_ospf_cost_invalid"));
    }
    if request.left_current_ospf_cost == Some(request.desired_ospf_cost)
        && request.right_current_ospf_cost == Some(request.desired_ospf_cost)
    {
        return Err(ApiError::bad_request("tunnel_plan_ospf_cost_noop"));
    }
    Ok(())
}

async fn validate_ospf_recommendation_contract(
    state: &AppState,
    plan_id: Uuid,
    request: &UpdateTunnelPlanOspfCostRequest,
) -> Result<(), ApiError> {
    let Some(plan) = state
        .repo
        .network_ospf_update_plan_by_id(plan_id)
        .await
        .map_err(ApiError::internal_mapper(
            "network_ospf_update_plan_unavailable",
            "The OSPF update plan could not be loaded.",
        ))?
    else {
        return Err(ApiError::conflict(
            "tunnel_plan_ospf_recommendation_missing",
        ));
    };
    if !plan.requires_approval || plan.control_mode != "reviewed" {
        return Err(ApiError::conflict(
            "tunnel_plan_ospf_recommendation_not_actionable",
        ));
    }
    if plan.plan_revision != request.plan_revision
        || plan.recommendation_id != request.recommendation_id
        || plan.recommended_ospf_cost != i32::from(request.desired_ospf_cost)
        || plan.left_current_ospf_cost != request.left_current_ospf_cost.map(i32::from)
        || plan.right_current_ospf_cost != request.right_current_ospf_cost.map(i32::from)
        || plan.left_adapter_definition_hash.as_deref()
            != Some(request.left_adapter_definition_hash.as_str())
        || plan.right_adapter_definition_hash.as_deref()
            != Some(request.right_adapter_definition_hash.as_str())
    {
        return Err(ApiError::conflict("tunnel_plan_ospf_recommendation_stale"));
    }
    Ok(())
}

fn tunnel_plan_mutation_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("tunnel_plan_not_found") {
        ApiError::not_found("tunnel_plan_not_found")
    } else if message.contains("tunnel_plan_ospf_snapshot_stale") {
        ApiError::conflict("tunnel_plan_ospf_snapshot_stale")
    } else if message.contains("tunnel_plan_ospf_job_in_progress") {
        ApiError::conflict("tunnel_plan_ospf_job_in_progress")
    } else if message.contains("tunnel_plan_disabled") {
        ApiError::conflict("tunnel_plan_disabled")
    } else if message.contains("tunnel_plan_ospf_disabled") {
        ApiError::conflict("tunnel_plan_ospf_disabled")
    } else {
        ApiError::internal(
            "tunnel_plan_ospf_update_failed",
            "The OSPF cost update could not be completed.",
            error,
        )
    }
}

fn tunnel_plan_unavailable(error: anyhow::Error) -> ApiError {
    ApiError::internal(
        "tunnel_plan_unavailable",
        "The tunnel plan could not be loaded.",
        error,
    )
}

fn tunnel_plan_repository_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("tunnel_plan_name_conflict") {
        ApiError::conflict("tunnel_plan_name_conflict")
    } else if message.contains("tunnel_plan_snapshot_stale") {
        ApiError::conflict("tunnel_plan_snapshot_stale")
    } else if message.contains("tunnel_plan_interface_conflict") {
        ApiError::conflict("tunnel_plan_interface_conflict")
    } else if message.contains("tunnel_plan_address_conflict") {
        ApiError::conflict("tunnel_plan_address_conflict")
    } else if message.contains("tunnel_plan_listener_port_conflict") {
        ApiError::conflict("tunnel_plan_listener_port_conflict")
    } else if message.contains("tunnel_plan_name_is_immutable") {
        ApiError::bad_request("tunnel_plan_name_is_immutable")
    } else if message.contains("tunnel_plan_builtin_credentials_not_supported") {
        ApiError::conflict("tunnel_plan_builtin_credentials_not_supported")
    } else if message.contains("tunnel_plan_builtin_credentials_required") {
        ApiError::conflict("tunnel_plan_builtin_credentials_required")
    } else if message.contains("tunnel_plan_endpoint_agent_not_found") {
        ApiError::conflict("tunnel_plan_endpoint_agent_not_found")
    } else if message.contains("tunnel_plan_adapter_definition_id_invalid") {
        ApiError::bad_request("tunnel_plan_adapter_definition_id_invalid")
    } else if message.contains("tunnel_plan_adapter_definition_unavailable") {
        ApiError::conflict("tunnel_plan_adapter_definition_unavailable")
    } else if message.contains("tunnel_plan_endpoints_must_differ") {
        ApiError::bad_request("tunnel_plan_endpoints_must_differ")
    } else if message.contains("tunnel_connection_assessment_requires_enabled_plan") {
        ApiError::conflict("tunnel_connection_assessment_requires_enabled_plan")
    } else if message.contains("invalid_tunnel_connection_assessment") {
        ApiError::bad_request("invalid_tunnel_connection_assessment")
    } else if message.contains("tunnel_connection_assessment_note_required") {
        ApiError::bad_request("tunnel_connection_assessment_note_required")
    } else if message.contains("tunnel_plan_not_found") {
        ApiError::not_found("tunnel_plan_not_found")
    } else {
        ApiError::internal(
            "tunnel_plan_mutation_failed",
            "The tunnel plan change could not be completed.",
            error,
        )
    }
}

fn tunnel_plan_privilege_target(plan_id: Uuid) -> String {
    format!("tunnel_plan:{plan_id}")
}

fn tunnel_plan_ospf_cost_payload_hash(
    plan_id: Uuid,
    request: &UpdateTunnelPlanOspfCostRequest,
) -> String {
    payload_hash(
        routing_cost_update_privilege_payload(
            plan_id,
            request.plan_revision,
            &request.recommendation_id,
            request.left_current_ospf_cost,
            request.right_current_ospf_cost,
            request.desired_ospf_cost,
            &request.left_adapter_definition_hash,
            &request.right_adapter_definition_hash,
        )
        .as_bytes(),
    )
}

fn require_tunnel_ospf_enabled(plan: &TunnelPlanView) -> Result<(), ApiError> {
    if !plan.enabled {
        return Err(ApiError::conflict("tunnel_plan_disabled"));
    }
    if plan.plan.ospf.is_none() {
        return Err(ApiError::conflict("tunnel_plan_ospf_disabled"));
    }
    if plan.left_ospf_status == "pending" || plan.right_ospf_status == "pending" {
        return Err(ApiError::conflict("tunnel_plan_ospf_job_in_progress"));
    }
    Ok(())
}

pub(crate) async fn resolve_plan_routing_adapters(
    state: &AppState,
    plan: &TunnelPlanView,
) -> Result<(RoutingCostAdapterCommands, RoutingCostAdapterCommands), ApiError> {
    resolve_routing_adapters_for_tunnel_plan(state, &plan.plan).await
}

async fn resolve_routing_adapters_for_tunnel_plan(
    state: &AppState,
    plan: &TunnelPlan,
) -> Result<(RoutingCostAdapterCommands, RoutingCostAdapterCommands), ApiError> {
    let ospf = plan
        .ospf
        .as_ref()
        .ok_or_else(|| ApiError::conflict("tunnel_plan_ospf_disabled"))?;
    let fallback_clients = [
        ospf.left_adapter_definition_id
            .is_none()
            .then(|| plan.left_client_id.clone()),
        ospf.right_adapter_definition_id
            .is_none()
            .then(|| plan.right_client_id.clone()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let fallback_sources = state
        .repo
        .effective_ospf_command_sources_for_clients(&fallback_clients)
        .await
        .map_err(|error| {
            ApiError::internal(
                "ospf_command_sources_unavailable",
                "The OSPF command configuration could not be loaded.",
                error,
            )
        })?;
    let left = if let Some(definition_id) = ospf.left_adapter_definition_id.as_deref() {
        resolve_routing_adapter(state, definition_id).await?
    } else {
        routing_commands_from_configuration_preset(
            fallback_sources
                .get(&plan.left_client_id)
                .and_then(Option::as_ref),
        )?
    };
    let right = if let Some(definition_id) = ospf.right_adapter_definition_id.as_deref() {
        resolve_routing_adapter(state, definition_id).await?
    } else {
        routing_commands_from_configuration_preset(
            fallback_sources
                .get(&plan.right_client_id)
                .and_then(Option::as_ref),
        )?
    };
    Ok((left, right))
}

async fn resolve_routing_adapter(
    state: &AppState,
    definition_id: &str,
) -> Result<RoutingCostAdapterCommands, ApiError> {
    let definition_id = Uuid::parse_str(definition_id)
        .map_err(|_| ApiError::bad_request("routing_cost_adapter_definition_id_invalid"))?;
    let definition = state
        .repo
        .network_adapter_definition_by_id(definition_id, Some("routing_cost"))
        .await
        .map_err(ApiError::internal_mapper(
            "routing_cost_adapter_unavailable",
            "The routing-cost adapter could not be loaded.",
        ))?
        .ok_or_else(|| ApiError::conflict("routing_cost_adapter_definition_not_found"))?;
    validate_network_adapter_definition_view(&definition)
        .map_err(|_| ApiError::conflict("routing_cost_adapter_definition_invalid"))?;
    let contract_version = definition
        .definition
        .get("contract_version")
        .and_then(serde_json::Value::as_u64);
    if contract_version
        != Some(u64::from(
            vpsman_common::ROUTING_COST_ADAPTER_CONTRACT_VERSION,
        ))
    {
        return Err(ApiError::conflict(
            "routing_cost_adapter_contract_version_invalid",
        ));
    }
    let status = routing_adapter_command(&definition.definition, "status_command")?;
    let update = routing_adapter_command(&definition.definition, "update_command")?;
    let definition_json = serde_json::to_vec(&definition.definition).map_err(|error| {
        ApiError::internal(
            "routing_cost_adapter_projection_failed",
            "The routing-cost adapter could not be prepared.",
            anyhow::Error::from(error),
        )
    })?;
    Ok(RoutingCostAdapterCommands {
        source: vpsman_common::RoutingCostCommandSource::PlanOverride,
        definition_id: definition.id.to_string(),
        definition_name: definition.name,
        definition_hash: payload_hash(&definition_json),
        status,
        update,
    })
}

fn routing_commands_from_configuration_preset(
    source: Option<&crate::model::ResolvedOspfCommandSource>,
) -> Result<RoutingCostAdapterCommands, ApiError> {
    let source = source.ok_or_else(|| ApiError::conflict("ospf_update_command_unconfigured"))?;
    Ok(RoutingCostAdapterCommands {
        source: vpsman_common::RoutingCostCommandSource::ConfigurationPreset,
        definition_id: source.id.to_string(),
        definition_name: source.name.clone(),
        definition_hash: source.definition_hash.clone(),
        status: source.status.clone(),
        update: source.update.clone(),
    })
}

fn routing_adapter_command(
    definition: &serde_json::Value,
    field: &str,
) -> Result<RuntimeTunnelCommand, ApiError> {
    let command: RuntimeTunnelCommand = serde_json::from_value(
        definition
            .get(field)
            .cloned()
            .ok_or_else(|| ApiError::conflict("routing_cost_adapter_command_missing"))?,
    )
    .map_err(|_| ApiError::conflict("routing_cost_adapter_command_invalid"))?;
    if command.argv.is_empty()
        || command.argv.len() > 32
        || !command.argv[0].starts_with('/')
        || command
            .argv
            .iter()
            .any(|part| part.is_empty() || part.len() > 4096 || part.contains('\0'))
        || !(1..=120).contains(&command.max_timeout_secs)
        || !(1024..=64 * 1024).contains(&command.max_output_bytes)
    {
        return Err(ApiError::conflict("routing_cost_adapter_command_invalid"));
    }
    Ok(command)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn dispatch_routing_jobs(
    state: &AppState,
    operator: &crate::model::AuthContext,
    plan: &TunnelPlanView,
    left_job_id: Uuid,
    right_job_id: Uuid,
    left_adapter: RoutingCostAdapterCommands,
    right_adapter: RoutingCostAdapterCommands,
    apply: Option<(Option<u16>, Option<u16>, u16)>,
) -> (Vec<CreateJobResponse>, Vec<TunnelPlanOspfDispatchView>) {
    // The routing command protocol requires both OSPF adapter identifiers in the
    // plan. Freeze the effective per-endpoint source into this job-only snapshot
    // so published agents can execute preset-backed jobs without re-resolving it.
    let mut resolved_plan = plan.plan.clone();
    if let Some(ospf) = resolved_plan.ospf.as_mut() {
        ospf.left_adapter_definition_id = Some(left_adapter.definition_id.clone());
        ospf.right_adapter_definition_id = Some(right_adapter.definition_id.clone());
    }
    let specs = [
        (
            TunnelEndpointSide::Left,
            plan.left_client_id.clone(),
            left_job_id,
            left_adapter,
            apply.map(|(left, _, desired)| (left, desired)),
        ),
        (
            TunnelEndpointSide::Right,
            plan.right_client_id.clone(),
            right_job_id,
            right_adapter,
            apply.map(|(_, right, desired)| (right, desired)),
        ),
    ];
    let mut jobs = Vec::with_capacity(specs.len());
    let mut dispatch = Vec::with_capacity(specs.len());
    for (side, client_id, job_id, adapter, endpoint_apply) in specs {
        let operation = if let Some((expected_current_cost, desired_cost)) = endpoint_apply {
            JobCommand::NetworkRoutingApply {
                plan_id: plan.id.to_string(),
                plan: Box::new(resolved_plan.clone()),
                side,
                adapter: adapter.clone(),
                expected_current_cost,
                desired_cost,
            }
        } else {
            JobCommand::NetworkRoutingStatus {
                plan_id: plan.id.to_string(),
                plan: Box::new(resolved_plan.clone()),
                side,
                adapter: adapter.clone(),
            }
        };
        let max_timeout_secs = if endpoint_apply.is_some() {
            adapter
                .status
                .max_timeout_secs
                .saturating_mul(2)
                .saturating_add(adapter.update.max_timeout_secs)
                .saturating_add(5)
        } else {
            adapter.status.max_timeout_secs.saturating_add(5)
        };
        let request = CreateJobRequest {
            job_id: Some(job_id),
            selector_expression: vpsman_common::id_selector_expression(&client_id),
            target_client_ids: vec![client_id.clone()],
            destructive: false,
            confirmed: endpoint_apply.is_some(),
            command: vpsman_common::job_command_type_label(&operation).to_string(),
            argv: Vec::new(),
            operation: Some(operation),
            max_timeout_secs: Some(max_timeout_secs),
            force_unprivileged: false,
            privileged: endpoint_apply.is_some(),
            privilege_assertion: None,
            rollout: None,
        };
        match create_job_from_internal_operator_mutation(state, operator, request).await {
            Ok((_status, Json(response))) => {
                let queued = response.target_counts.queued > 0
                    || response.target_counts.dispatching > 0
                    || response.target_counts.running > 0;
                if !queued {
                    if let Err(error) = state
                        .repo
                        .record_tunnel_plan_ospf_job_result(plan.id, side, job_id, None, false)
                        .await
                    {
                        warn!(?error, plan_id = %plan.id, %job_id, ?side, "failed to persist unqueued OSPF endpoint state");
                    }
                }
                dispatch.push(TunnelPlanOspfDispatchView {
                    endpoint_side: side,
                    client_id,
                    job_id,
                    status: if queued { "queued" } else { "not_queued" }.to_string(),
                    error: (!queued)
                        .then(|| format!("job was not queued (status={})", response.status)),
                });
                jobs.push(response);
            }
            Err(error) => {
                if let Err(record_error) = state
                    .repo
                    .record_tunnel_plan_ospf_job_result(plan.id, side, job_id, None, false)
                    .await
                {
                    warn!(?record_error, plan_id = %plan.id, %job_id, ?side, "failed to persist OSPF queue failure state");
                }
                dispatch.push(TunnelPlanOspfDispatchView {
                    endpoint_side: side,
                    client_id,
                    job_id,
                    status: "queue_failed".to_string(),
                    error: Some(operator_dispatch_error(&error, "Routing adapter job")),
                });
            }
        }
    }
    (jobs, dispatch)
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn resolve_allocation_family(
    include: Option<bool>,
    request_pool: Option<String>,
    configured_pool: Option<String>,
    explicit_request: bool,
    missing_code: &'static str,
) -> Result<Option<String>, ApiError> {
    if matches!(include, Some(false)) {
        return Ok(None);
    }
    if let Some(pool) = request_pool {
        return Ok(Some(pool));
    }
    if matches!(include, Some(true)) {
        return configured_pool
            .map(Some)
            .ok_or_else(|| ApiError::bad_request(missing_code));
    }
    if !explicit_request {
        return Ok(configured_pool);
    }
    Ok(None)
}

fn tunnel_plan_error_code(error: NetworkPlanError) -> &'static str {
    match error {
        NetworkPlanError::InvalidPlanIdentity => "invalid_tunnel_plan_identity",
        NetworkPlanError::InvalidTunnelEndpoints => "invalid_tunnel_plan_endpoints",
        NetworkPlanError::InvalidUnderlayAddress => "invalid_tunnel_underlay_address",
        NetworkPlanError::InvalidRuntimeTunnelCommand
        | NetworkPlanError::RuntimeTunnelAdapterCommandRequired
        | NetworkPlanError::RuntimeTunnelObservedCannotMutate
        | NetworkPlanError::InvalidRuntimeTunnelTrafficLimit => "network_runtime_control_invalid",
        NetworkPlanError::RuntimeTunnelTopologyRequiresAgentBuiltin => {
            "network_runtime_topology_requires_agent_builtin"
        }
        NetworkPlanError::InvalidRuntimeTunnelTopology => "network_runtime_topology_invalid",
        NetworkPlanError::InvalidRuntimeTunnelRoute => "network_runtime_route_invalid",
        NetworkPlanError::UnsupportedRuntimeManagerTunnelKind => {
            "unsupported_tunnel_kind_for_runtime_manager"
        }
        NetworkPlanError::InvalidInterfaceName
        | NetworkPlanError::InvalidCidr
        | NetworkPlanError::AddressPoolTooSmall
        | NetworkPlanError::AddressPoolExhausted
        | NetworkPlanError::AddressPoolRequired
        | NetworkPlanError::InvalidBandwidthMbps
        | NetworkPlanError::InvalidOspfConfig
        | NetworkPlanError::TunnelAddressRequired => "invalid_tunnel_plan_input",
        NetworkPlanError::InvalidTunnelMtu => "invalid_tunnel_mtu",
        NetworkPlanError::TunnelMtuRequired => "tunnel_mtu_required",
        NetworkPlanError::TunnelMtuExternallyOwned => "tunnel_mtu_externally_owned",
    }
}

pub(crate) async fn list_network_ospf_recommendations(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<NetworkOspfRecommendationView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    Ok(Json(
        state
            .repo
            .list_network_ospf_recommendations(limit_or_default(query.limit))
            .await
            .map_err(ApiError::internal_mapper(
                "network_ospf_recommendations_unavailable",
                "OSPF recommendations could not be loaded.",
            ))?,
    ))
}

pub(crate) async fn list_network_ospf_update_plans(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<NetworkOspfUpdatePlanView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_NETWORK_READ)
        .await?;
    Ok(Json(
        state
            .repo
            .list_network_ospf_update_plans(limit_or_default(query.limit))
            .await
            .map_err(ApiError::internal_mapper(
                "network_ospf_update_plans_unavailable",
                "OSPF update plans could not be loaded.",
            ))?,
    ))
}

pub(crate) async fn get_topology_graph(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<NetworkEvidenceQuery>,
) -> Result<Json<TopologyGraphView>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    let filter = network_observation_filter(&query, 100, false)?;
    Ok(Json(
        state
            .repo
            .topology_graph(
                filter.limit,
                filter.start_unix,
                filter.end_unix,
                &filter.plan_ids,
            )
            .await
            .map_err(topology_graph_error)?,
    ))
}

pub(crate) fn topology_graph_error(error: anyhow::Error) -> ApiError {
    let (code, message) = match error.downcast_ref::<TopologyGraphStageError>() {
        Some(TopologyGraphStageError::Agents) => (
            "topology_graph_agents_unavailable",
            "Topology could not load the VPS inventory.",
        ),
        Some(TopologyGraphStageError::Plans) => (
            "topology_graph_plans_unavailable",
            "Topology could not load tunnel plans.",
        ),
        Some(TopologyGraphStageError::Observations) => (
            "topology_graph_observations_unavailable",
            "Topology could not load network-test evidence.",
        ),
        Some(TopologyGraphStageError::RuntimeTelemetry) => (
            "topology_graph_runtime_telemetry_unavailable",
            "Topology could not load tunnel runtime evidence.",
        ),
        Some(TopologyGraphStageError::OspfRecommendations) => (
            "topology_graph_ospf_recommendations_unavailable",
            "Topology could not load OSPF recommendations.",
        ),
        Some(TopologyGraphStageError::Contract) => (
            "topology_graph_contract_invalid",
            "Topology evidence did not satisfy the current display contract.",
        ),
        None => (
            "topology_graph_unavailable",
            "Topology could not be loaded.",
        ),
    };
    ApiError::internal(code, message, error)
}
