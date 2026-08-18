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

/// Stable identity for the topology fields that bind reachability evidence.
/// Endpoint or address-family changes detach old observations; policy-only edits do not.
pub fn tunnel_topology_identity_hash(plan_id: uuid::Uuid, plan: &TunnelPlan) -> String {
    let payload = serde_json::to_vec(&serde_json::json!({
        "plan_id": plan_id.to_string(),
        "name": &plan.name,
        "kind": format!("{:?}", plan.kind),
        "left_client_id": &plan.left_client_id,
        "right_client_id": &plan.right_client_id,
        "interface_name": &plan.interface_name,
        "left_remote_underlay": &plan.left_remote_underlay,
        "left_local_underlay": &plan.left_local_underlay,
        "right_remote_underlay": &plan.right_remote_underlay,
        "right_local_underlay": &plan.right_local_underlay,
        "left_tunnel_address": &plan.left_tunnel_address,
        "right_tunnel_address": &plan.right_tunnel_address,
        "ipv4_tunnel": &plan.ipv4_tunnel,
        "ipv6_tunnel": &plan.ipv6_tunnel,
        "latency_primary_family": format!("{:?}", plan.latency_primary_family),
    }))
    .expect("topology identity payload serializes");
    crate::payload_hash(&payload)
}

/// Stable identity for the complete runtime-tunnel configuration that can
/// affect adapter and traffic evidence. Unlike the topology identity above,
/// this intentionally changes for policy/runtime-control edits and builtin
/// credential rotation. A telemetry sample carrying this identity therefore
/// proves that the agent received the exact desired runtime configuration.
pub fn tunnel_runtime_evidence_identity_hash(
    plan_id: uuid::Uuid,
    plan: &TunnelPlan,
    credential_generation: Option<u64>,
) -> String {
    let payload = serde_json::to_vec(&serde_json::json!({
        "schema": 1,
        "plan_id": plan_id.to_string(),
        "plan": plan,
        "builtin_credential_generation": credential_generation,
    }))
    .expect("runtime evidence identity payload serializes");
    crate::payload_hash(&payload)
}
