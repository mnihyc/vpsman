use axum::{
    extract::{Query, State},
    http::HeaderMap,
    http::StatusCode,
    Json,
};
use vpsman_common::{
    plan_tunnel, BandwidthTier, NetworkPlanError, OspfCostPolicy, RuntimeTunnelControl,
    RuntimeTunnelManager, RuntimeTunnelTopologyIntent, TunnelEndpointSide, TunnelKind,
    TunnelPlanInput,
};

use crate::{
    error::ApiError,
    model::{
        CreateTunnelPlanRequest, HistoryQuery, NetworkOspfRecommendationView,
        NetworkOspfUpdatePlanView, PromoteTelemetryTunnelRequest,
        PromoteTunnelPlanToAdapterRequest, TelemetryTunnelView, TunnelPlanView,
    },
    model_topology::TopologyGraphView,
    state::AppState,
    util::limit_or_default,
};

pub(crate) async fn list_tunnel_plans(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<TunnelPlanView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
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
    let plan = plan_tunnel(&request.input)
        .map_err(|error| ApiError::bad_request(tunnel_plan_error_code(error)))?;
    Ok((
        StatusCode::CREATED,
        Json(
            state
                .repo
                .record_tunnel_plan(&request.input, &plan, &operator)
                .await?,
        ),
    ))
}

pub(crate) async fn promote_telemetry_tunnel_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PromoteTelemetryTunnelRequest>,
) -> Result<(StatusCode, Json<TunnelPlanView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
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
    let view = state
        .repo
        .record_tunnel_plan(&input, &plan, &operator)
        .await?;
    state
        .repo
        .record_tunnel_plan_promotion_audit(&view, &operator, &report)
        .await?;
    Ok((StatusCode::CREATED, Json(view)))
}

pub(crate) async fn promote_tunnel_plan_to_adapter(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PromoteTunnelPlanToAdapterRequest>,
) -> Result<Json<TunnelPlanView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    validate_adapter_promotion_request(&request)?;
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
    input.runtime_topology = request
        .runtime_topology
        .clone()
        .unwrap_or_else(|| existing.input.runtime_topology.clone());
    if input.runtime_topology.desired_interfaces.is_empty() {
        input.runtime_topology.desired_interfaces = vec![input.interface_name.clone()];
    }
    let plan = plan_tunnel(&input)
        .map_err(|error| ApiError::bad_request(tunnel_plan_error_code(error)))?;
    Ok(Json(
        state
            .repo
            .promote_tunnel_plan_to_adapter(&existing, &input, &plan, &operator)
            .await?,
    ))
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
        | NetworkPlanError::AddressPoolExhausted => "invalid_tunnel_plan_input",
    }
}

fn validate_adapter_promotion_request(
    request: &PromoteTunnelPlanToAdapterRequest,
) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "adapter_promotion_requires_confirmation",
        ));
    }
    if request.runtime_control.manager != RuntimeTunnelManager::ExternalManagedAdapter {
        return Err(ApiError::bad_request(
            "adapter_promotion_requires_external_managed_adapter",
        ));
    }
    if request.runtime_control.status.is_none() {
        return Err(ApiError::bad_request(
            "adapter_promotion_status_command_required",
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
    if request
        .topology_version
        .as_ref()
        .is_some_and(|version| version.is_empty() || version.len() > 128)
    {
        return Err(ApiError::bad_request("invalid_runtime_topology_version"));
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
            version: request
                .topology_version
                .clone()
                .or_else(|| Some(format!("telemetry-import:{}", request.interface))),
            desired_interfaces: vec![request.interface.clone()],
            ..RuntimeTunnelTopologyIntent::default()
        },
        left_client_id,
        right_client_id,
        left_underlay,
        right_underlay,
        address_pool_cidr: request.address_pool_cidr.clone(),
        reserved_addresses: Vec::new(),
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
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
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
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
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
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(
        state
            .repo
            .topology_graph(limit_or_default(query.limit))
            .await?,
    ))
}
