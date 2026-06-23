use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;
use vpsman_common::{
    allocate_tunnel_endpoints as allocate_tunnel_endpoint_pairs, plan_tunnel, BandwidthTier,
    NetworkPlanError, OspfCostPolicy, RuntimeTunnelControl, RuntimeTunnelManager,
    RuntimeTunnelTopologyIntent, TunnelAddressFamily, TunnelEndpointSide, TunnelKind, TunnelPlan,
    TunnelPlanInput,
};

use crate::{
    error::ApiError,
    model::{
        AllocateTunnelEndpointsRequest, AllocateTunnelEndpointsResponse, CreateTunnelPlanRequest,
        HistoryQuery, NetworkOspfRecommendationView, NetworkOspfUpdatePlanView,
        PromoteTelemetryTunnelRequest, PromoteTunnelPlanToCustomAdapterRequest,
        TelemetryTunnelView, TunnelPlanView, UpdateTunnelPlanOspfCostRequest,
    },
    model_topology::TopologyGraphView,
    runtime_config::push_runtime_config_for_clients,
    security::{SCOPE_FLEET_READ, SCOPE_NETWORK_READ},
    state::AppState,
    util::limit_or_default,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TunnelPlanMutationRequest {
    #[serde(default)]
    pub(crate) confirmed: bool,
}

pub(crate) async fn list_tunnel_plans(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<TunnelPlanView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_NETWORK_READ)
        .await?;
    Ok(Json(state.repo.list_tunnel_plans().await?))
}

pub(crate) async fn create_tunnel_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateTunnelPlanRequest>,
) -> Result<(StatusCode, Json<TunnelPlanView>), ApiError> {
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
    let previous_plan = state
        .repo
        .list_tunnel_plans()
        .await?
        .into_iter()
        .find(|plan| plan.name == request.input.name);
    let view = state
        .repo
        .record_tunnel_plan(&request.input, &plan, request.enabled, &operator)
        .await?;
    let mut sync_client_ids = Vec::new();
    if let Some(previous) = previous_plan.as_ref().filter(|plan| plan.enabled) {
        sync_client_ids.push(previous.left_client_id.clone());
        sync_client_ids.push(previous.right_client_id.clone());
    }
    if view.enabled {
        sync_client_ids.push(view.left_client_id.clone());
        sync_client_ids.push(view.right_client_id.clone());
    }
    if !sync_client_ids.is_empty() {
        let reason = if view.enabled {
            "tunnel_plan_saved_enabled"
        } else {
            "tunnel_plan_saved_disabled"
        };
        let _sync_jobs =
            push_runtime_config_for_clients(&state, &operator, sync_client_ids, reason).await?;
    }
    Ok((StatusCode::CREATED, Json(view)))
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
    for plan in state.repo.list_tunnel_plans().await? {
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
    let Some(view) = state.repo.get_tunnel_plan(plan_id).await? else {
        return Err(ApiError::not_found("tunnel_plan_not_found"));
    };
    Ok(Json(view.plan))
}

pub(crate) async fn enable_tunnel_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(plan_id): Path<Uuid>,
    Json(request): Json<TunnelPlanMutationRequest>,
) -> Result<Json<TunnelPlanView>, ApiError> {
    mutate_tunnel_plan_enabled(state, headers, plan_id, request, true).await
}

pub(crate) async fn disable_tunnel_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(plan_id): Path<Uuid>,
    Json(request): Json<TunnelPlanMutationRequest>,
) -> Result<Json<TunnelPlanView>, ApiError> {
    mutate_tunnel_plan_enabled(state, headers, plan_id, request, false).await
}

pub(crate) async fn update_tunnel_plan_ospf_cost(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(plan_id): Path<Uuid>,
    Json(request): Json<UpdateTunnelPlanOspfCostRequest>,
) -> Result<Json<TunnelPlanView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    validate_tunnel_plan_ospf_cost_request(&request)?;
    let existing = state
        .repo
        .get_tunnel_plan(plan_id)
        .await?
        .ok_or_else(|| ApiError::bad_request("tunnel_plan_not_found"))?;
    if existing.enabled {
        require_tunnel_endpoint_agents(&state, &existing.left_client_id, &existing.right_client_id)
            .await?;
    }
    let view = state
        .repo
        .update_tunnel_plan_ospf_cost(
            plan_id,
            request.current_ospf_cost,
            request.recommended_ospf_cost,
            &operator,
        )
        .await
        .map_err(tunnel_plan_mutation_error)?;
    if view.enabled {
        let _sync_jobs = push_runtime_config_for_clients(
            &state,
            &operator,
            vec![view.left_client_id.clone(), view.right_client_id.clone()],
            "tunnel_plan_ospf_cost_updated",
        )
        .await?;
    }
    Ok(Json(view))
}

async fn mutate_tunnel_plan_enabled(
    state: AppState,
    headers: HeaderMap,
    plan_id: Uuid,
    request: TunnelPlanMutationRequest,
    enabled: bool,
) -> Result<Json<TunnelPlanView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    require_tunnel_plan_confirmed(request.confirmed)?;
    let existing = state
        .repo
        .get_tunnel_plan(plan_id)
        .await?
        .ok_or_else(|| ApiError::bad_request("tunnel_plan_not_found"))?;
    if enabled {
        require_tunnel_endpoint_agents(&state, &existing.left_client_id, &existing.right_client_id)
            .await?;
    }
    let view = state
        .repo
        .set_tunnel_plan_enabled(plan_id, enabled, &operator)
        .await?;
    let reason = if enabled {
        "tunnel_plan_enabled"
    } else {
        "tunnel_plan_disabled"
    };
    let _sync_jobs = push_runtime_config_for_clients(
        &state,
        &operator,
        vec![view.left_client_id.clone(), view.right_client_id.clone()],
        reason,
    )
    .await?;
    Ok(Json(view))
}

pub(crate) async fn promote_telemetry_tunnel_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PromoteTelemetryTunnelRequest>,
) -> Result<(StatusCode, Json<TunnelPlanView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    require_tunnel_plan_confirmed(request.confirmed)?;
    validate_telemetry_promotion_request(&request)?;
    let mut reports = state
        .repo
        .list_telemetry_tunnels(1, Some(&request.client_id), Some(&request.interface))
        .await?;
    let Some(report) = reports.pop() else {
        return Err(ApiError::bad_request("telemetry_tunnel_not_found"));
    };
    if !report.promotion_required
        || report.mutation_policy.as_str() != "observe_only_import_candidate"
    {
        return Err(ApiError::bad_request(
            "telemetry_tunnel_not_import_candidate",
        ));
    }
    let input = telemetry_promotion_input(&request, &report)?;
    let plan = plan_tunnel(&input)
        .map_err(|error| ApiError::bad_request(tunnel_plan_error_code(error)))?;
    require_tunnel_endpoint_agents(&state, &input.left_client_id, &input.right_client_id).await?;
    let view = state
        .repo
        .record_tunnel_plan(&input, &plan, request.enabled, &operator)
        .await?;
    state
        .repo
        .record_tunnel_plan_promotion_audit(&view, &operator, &report)
        .await?;
    if view.enabled {
        let _sync_jobs = push_runtime_config_for_clients(
            &state,
            &operator,
            vec![view.left_client_id.clone(), view.right_client_id.clone()],
            "tunnel_plan_promoted_from_telemetry",
        )
        .await?;
    }
    Ok((StatusCode::CREATED, Json(view)))
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
    let agents = state.repo.list_agents().await?;
    let has_left = agents.iter().any(|agent| agent.id == left_client_id);
    let has_right = agents.iter().any(|agent| agent.id == right_client_id);
    if !has_left || !has_right {
        return Err(ApiError::bad_request(
            "tunnel_plan_endpoint_agent_not_found",
        ));
    }
    Ok(())
}

fn validate_tunnel_plan_ospf_cost_request(
    request: &UpdateTunnelPlanOspfCostRequest,
) -> Result<(), ApiError> {
    require_tunnel_plan_confirmed(request.confirmed)?;
    if request.current_ospf_cost == 0 || request.recommended_ospf_cost == 0 {
        return Err(ApiError::bad_request("tunnel_plan_ospf_cost_invalid"));
    }
    if request.current_ospf_cost == request.recommended_ospf_cost {
        return Err(ApiError::bad_request("tunnel_plan_ospf_cost_noop"));
    }
    Ok(())
}

fn tunnel_plan_mutation_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("tunnel_plan_not_found") {
        ApiError::not_found("tunnel_plan_not_found")
    } else if message.contains("tunnel_plan_ospf_cost_stale") {
        ApiError::conflict("tunnel_plan_ospf_cost_stale")
    } else {
        ApiError::from(error)
    }
}

pub(crate) async fn promote_tunnel_plan_to_custom_adapter(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PromoteTunnelPlanToCustomAdapterRequest>,
) -> Result<Json<TunnelPlanView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    validate_custom_adapter_request(&request)?;
    let existing = state
        .repo
        .get_tunnel_plan(request.plan_id)
        .await?
        .ok_or_else(|| ApiError::bad_request("tunnel_plan_not_found"))?;
    if existing.plan.runtime_control.manager != RuntimeTunnelManager::ExternalObserved {
        return Err(ApiError::bad_request("tunnel_plan_not_external_observed"));
    }
    let mut input = existing.input.clone();
    if let Some(name) = &request.name {
        input.name = name.clone();
    }
    input.runtime_control = request.runtime_control.clone();
    let server_topology_version = existing.input.runtime_topology.version.clone();
    input.runtime_topology = request
        .runtime_topology
        .clone()
        .unwrap_or_else(|| existing.input.runtime_topology.clone());
    input.runtime_topology.version = server_topology_version;
    if input.runtime_topology.desired_interfaces.is_empty() {
        input.runtime_topology.desired_interfaces = vec![input.interface_name.clone()];
    }
    let plan = plan_tunnel(&input)
        .map_err(|error| ApiError::bad_request(tunnel_plan_error_code(error)))?;
    require_tunnel_endpoint_agents(&state, &input.left_client_id, &input.right_client_id).await?;
    let view = state
        .repo
        .promote_tunnel_plan_to_custom_adapter(&existing, &input, &plan, &operator)
        .await?;
    if view.enabled {
        let _sync_jobs = push_runtime_config_for_clients(
            &state,
            &operator,
            vec![view.left_client_id.clone(), view.right_client_id.clone()],
            "tunnel_plan_custom_adapter_updated",
        )
        .await?;
    }
    Ok(Json(view))
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
        NetworkPlanError::InvalidRuntimeTunnelCommand
        | NetworkPlanError::RuntimeTunnelAdapterCommandRequired
        | NetworkPlanError::RuntimeTunnelObservedCannotMutate
        | NetworkPlanError::InvalidRuntimeTunnelTrafficLimit => "network_runtime_control_invalid",
        NetworkPlanError::InvalidRuntimeTunnelTopology => "network_runtime_topology_invalid",
        NetworkPlanError::InvalidRuntimeTunnelRoute => "network_runtime_route_invalid",
        NetworkPlanError::UnsupportedBackendTunnelKind => {
            "unsupported_tunnel_kind_for_runtime_manager"
        }
        NetworkPlanError::InvalidInterfaceName
        | NetworkPlanError::InvalidCidr
        | NetworkPlanError::AddressPoolTooSmall
        | NetworkPlanError::AddressPoolExhausted
        | NetworkPlanError::AddressPoolRequired
        | NetworkPlanError::TunnelAddressRequired => "invalid_tunnel_plan_input",
    }
}

fn validate_custom_adapter_request(
    request: &PromoteTunnelPlanToCustomAdapterRequest,
) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "custom_adapter_requires_confirmation",
        ));
    }
    if request.runtime_control.manager != RuntimeTunnelManager::ExternalManagedAdapter {
        return Err(ApiError::bad_request(
            "custom_adapter_requires_external_managed_adapter",
        ));
    }
    if request.runtime_control.status.is_none() {
        return Err(ApiError::bad_request(
            "custom_adapter_status_command_required",
        ));
    }
    if request
        .name
        .as_ref()
        .is_some_and(|name| name.is_empty() || name.len() > 128)
    {
        return Err(ApiError::bad_request("invalid_tunnel_plan_name"));
    }
    Ok(())
}

fn validate_telemetry_promotion_request(
    request: &PromoteTelemetryTunnelRequest,
) -> Result<(), ApiError> {
    if request.client_id.is_empty() || request.client_id.len() > 128 {
        return Err(ApiError::bad_request("invalid_client_id"));
    }
    if request.peer_client_id.is_empty() || request.peer_client_id.len() > 128 {
        return Err(ApiError::bad_request("invalid_peer_client_id"));
    }
    if request.client_id == request.peer_client_id {
        return Err(ApiError::bad_request("telemetry_tunnel_peer_required"));
    }
    if request.interface.is_empty() || request.interface.len() > 64 {
        return Err(ApiError::bad_request("invalid_tunnel_interface"));
    }
    if request
        .name
        .as_ref()
        .is_some_and(|name| name.is_empty() || name.len() > 128)
    {
        return Err(ApiError::bad_request("invalid_tunnel_plan_name"));
    }
    Ok(())
}

fn telemetry_promotion_input(
    request: &PromoteTelemetryTunnelRequest,
    report: &TelemetryTunnelView,
) -> Result<TunnelPlanInput, ApiError> {
    let side = request.side.unwrap_or(TunnelEndpointSide::Left);
    let (left_client_id, right_client_id, left_underlay, right_underlay) = match side {
        TunnelEndpointSide::Left => (
            request.client_id.clone(),
            request.peer_client_id.clone(),
            request.local_underlay.clone(),
            request.peer_underlay.clone(),
        ),
        TunnelEndpointSide::Right => (
            request.peer_client_id.clone(),
            request.client_id.clone(),
            request.peer_underlay.clone(),
            request.local_underlay.clone(),
        ),
    };
    Ok(TunnelPlanInput {
        name: request.name.clone().unwrap_or_else(|| {
            format!(
                "{}-{}-{}-import",
                request.client_id, request.peer_client_id, request.interface
            )
        }),
        interface_name: request.interface.clone(),
        kind: telemetry_tunnel_kind(&report.kind)?,
        runtime_control: RuntimeTunnelControl {
            manager: RuntimeTunnelManager::ExternalObserved,
            ..RuntimeTunnelControl::default()
        },
        runtime_topology: RuntimeTunnelTopologyIntent {
            version: Some(format!("telemetry-import:{}", request.interface)),
            desired_interfaces: vec![request.interface.clone()],
            ..RuntimeTunnelTopologyIntent::default()
        },
        left_client_id,
        right_client_id,
        left_underlay,
        right_underlay,
        address_pool_cidr: request.address_pool_cidr.clone(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: request.ipv4_tunnel.clone(),
        ipv6_address_pool_cidr: request.ipv6_address_pool_cidr.clone(),
        ipv6_tunnel: request.ipv6_tunnel.clone(),
        latency_primary_family: request.latency_primary_family,
        bandwidth: request.bandwidth.unwrap_or(BandwidthTier::M100),
        latency_ms: request.latency_ms.unwrap_or(10.0),
        packet_loss_ratio: request.packet_loss_ratio.unwrap_or(0.0),
        preference: request.preference.unwrap_or(1.0),
        ospf_policy: OspfCostPolicy::default(),
    })
}

fn telemetry_tunnel_kind(value: &str) -> Result<TunnelKind, ApiError> {
    match value {
        "gre" => Ok(TunnelKind::Gre),
        "ipip" => Ok(TunnelKind::Ipip),
        "sit" => Ok(TunnelKind::Sit),
        "fou" => Ok(TunnelKind::Fou),
        "openvpn" => Ok(TunnelKind::Openvpn),
        "wireguard" => Ok(TunnelKind::Wireguard),
        "tun_tap" => Ok(TunnelKind::TunTap),
        "custom" => Ok(TunnelKind::Custom),
        _ => Err(ApiError::bad_request("unsupported_telemetry_tunnel_kind")),
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
            .await?,
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
            .await?,
    ))
}

pub(crate) async fn get_topology_graph(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<TopologyGraphView>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    Ok(Json(
        state
            .repo
            .topology_graph(limit_or_default(query.limit))
            .await?,
    ))
}
