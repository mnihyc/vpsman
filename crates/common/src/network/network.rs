#[path = "network_cost.rs"]
mod cost;
#[path = "network_models.rs"]
mod models;
#[path = "network_planner.rs"]
mod planner;

pub use cost::{
    effective_bandwidth_mbps, observed_ospf_cost, ospf_cost, routing_cost_update_privilege_payload,
    MAX_TUNNEL_BANDWIDTH_MBPS, MIN_TUNNEL_BANDWIDTH_MBPS,
};
pub use models::{
    default_ospf_healthy_windows, default_ospf_min_cost_delta, default_runtime_fou_ipproto,
    default_runtime_fou_peer_port, default_runtime_fou_port, default_runtime_openvpn_port,
    default_runtime_wireguard_keepalive_secs, default_runtime_wireguard_listen_port,
    default_tunnel_mtu, BandwidthMbps, OspfControlMode, OspfCostPolicy, RoutingCostAdapterCommands,
    RoutingCostAdapterJobResult, RoutingCostAdapterOperation, RoutingCostCommandSource,
    RuntimeTunnelAdapterCommands, RuntimeTunnelCommand, RuntimeTunnelControl,
    RuntimeTunnelFouOptions, RuntimeTunnelManager, RuntimeTunnelOpenvpnOptions,
    RuntimeTunnelOpenvpnTransport, RuntimeTunnelRoute, RuntimeTunnelTopologyIntent,
    RuntimeTunnelTrafficLimit, RuntimeTunnelWireguardEndpointMode, RuntimeTunnelWireguardOptions,
    TunnelAddressFamily, TunnelAddressPair, TunnelBuiltinCredentials,
    TunnelEndpointBuiltinCredentials, TunnelEndpointConfig, TunnelEndpointSide, TunnelKind,
    TunnelObservation, TunnelOpenvpnIdentity, TunnelOspfConfig, TunnelPlan, TunnelPlanInput,
    TunnelWireguardIdentity, MAX_TUNNEL_MTU, MIN_IPV6_TUNNEL_MTU, MIN_TUNNEL_MTU,
    ROUTING_COST_ADAPTER_CONTRACT_VERSION,
};
pub use planner::{
    allocate_tunnel_endpoints, plan_tunnel, render_tunnel_endpoint_config,
    validate_runtime_topology_intent, validate_runtime_tunnel_control,
    validate_runtime_tunnel_driver_options, NetworkPlanError, TunnelEndpointAllocation,
};

#[cfg(test)]
#[path = "tests_network.rs"]
mod tests;
