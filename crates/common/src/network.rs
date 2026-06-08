mod backends;
mod cost;
mod legacy;
mod models;
mod planner;

pub use backends::{
    backend_config_signature_payload, render_backend_config_for_endpoint,
    render_tunnel_endpoint_backend_config,
};
pub use cost::{effective_bandwidth_tier, observed_bandwidth_tier, observed_ospf_cost, ospf_cost};
pub use legacy::{parse_ifupdown_configs, parse_legacy_bird_config};
pub use models::{
    default_runtime_fou_ipproto, default_runtime_fou_peer_port, default_runtime_fou_port,
    BandwidthTier, IfupdownConfig, IfupdownInterface, LegacyBirdConfig, LegacyBirdPeer,
    OspfCostPolicy, RuntimeTunnelCommand, RuntimeTunnelControl, RuntimeTunnelFouOptions,
    RuntimeTunnelManager, RuntimeTunnelRoute, RuntimeTunnelTopologyIntent,
    RuntimeTunnelTrafficLimit, TunnelBackendConfig, TunnelBackendFile, TunnelConfigBackend,
    TunnelEndpointConfig, TunnelEndpointSide, TunnelKind, TunnelObservation, TunnelPlan,
    TunnelPlanInput, MANAGED_BIRD2_FILE, MANAGED_IFUPDOWN_FILE, MANAGED_NETPLAN_FILE,
    MANAGED_SYSTEMD_NETWORKD_NETDEV_FILE, MANAGED_SYSTEMD_NETWORKD_NETWORK_FILE,
};
pub use planner::{
    plan_tunnel, render_tunnel_endpoint_config, validate_runtime_topology_intent,
    validate_runtime_tunnel_control, NetworkPlanError,
};

#[cfg(test)]
mod tests;
