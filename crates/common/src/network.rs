mod cost;
mod models;
mod planner;

pub use cost::{
    effective_bandwidth_mbps, observed_ospf_cost, ospf_cost, routing_cost_update_privilege_payload,
    MAX_TUNNEL_BANDWIDTH_MBPS, MIN_TUNNEL_BANDWIDTH_MBPS,
};
pub use models::{
    default_ospf_healthy_windows, default_ospf_min_cost_delta, default_runtime_fou_ipproto,
    default_runtime_fou_peer_port, default_runtime_fou_port, BandwidthMbps, OspfControlMode,
    OspfCostPolicy, RoutingCostAdapterCommands, RoutingCostAdapterJobResult,
    RoutingCostAdapterOperation, RoutingCostAdapterRequest, RoutingCostAdapterResponse,
    RuntimeTunnelAdapterCommands, RuntimeTunnelCommand, RuntimeTunnelControl,
    RuntimeTunnelFouOptions, RuntimeTunnelManager, RuntimeTunnelRoute, RuntimeTunnelTopologyIntent,
    RuntimeTunnelTrafficLimit, TunnelAddressFamily, TunnelAddressPair, TunnelEndpointConfig,
    TunnelEndpointSide, TunnelKind, TunnelObservation, TunnelOspfConfig, TunnelPlan,
    TunnelPlanInput, ROUTING_COST_ADAPTER_CONTRACT_VERSION,
};
pub use planner::{
    allocate_tunnel_endpoints, plan_tunnel, render_tunnel_endpoint_config,
    validate_runtime_topology_intent, validate_runtime_tunnel_control, NetworkPlanError,
    TunnelEndpointAllocation,
};

#[cfg(test)]
mod tests;
