use std::{
    collections::HashSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};

use super::{
    cost::{ospf_cost, MAX_TUNNEL_BANDWIDTH_MBPS, MIN_TUNNEL_BANDWIDTH_MBPS},
    models::{
        RuntimeTunnelControl, RuntimeTunnelFouOptions, RuntimeTunnelManager, RuntimeTunnelRoute,
        RuntimeTunnelTopologyIntent, RuntimeTunnelTrafficLimit, TunnelAddressFamily,
        TunnelAddressPair, TunnelEndpointConfig, TunnelEndpointSide, TunnelKind, TunnelObservation,
        TunnelOspfConfig, TunnelPlan, TunnelPlanInput, MIN_IPV6_TUNNEL_MTU, MIN_TUNNEL_MTU,
    },
};

const MAX_RUNTIME_TOPOLOGY_VERSION_BYTES: usize = 128;
const MAX_RUNTIME_TOPOLOGY_INTERFACES: usize = 128;
const MAX_RUNTIME_TOPOLOGY_ROUTES: usize = 256;

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum NetworkPlanError {
    #[error("invalid tunnel plan identity")]
    InvalidPlanIdentity,
    #[error("tunnel endpoints must be two different clients")]
    InvalidTunnelEndpoints,
    #[error("invalid tunnel underlay address")]
    InvalidUnderlayAddress,
    #[error("invalid tunnel interface name")]
    InvalidInterfaceName,
    #[error("invalid IPv4 CIDR")]
    InvalidCidr,
    #[error("address pool must have prefix length 31 or shorter")]
    AddressPoolTooSmall,
    #[error("address pool is exhausted")]
    AddressPoolExhausted,
    #[error("address pool is required for endpoint allocation")]
    AddressPoolRequired,
    #[error("tunnel plan requires at least one IPv4 or IPv6 endpoint pair")]
    TunnelAddressRequired,
    #[error("tunnel kind is not supported by the selected runtime manager")]
    UnsupportedRuntimeManagerTunnelKind,
    #[error("runtime tunnel command must be bounded and use absolute argv")]
    InvalidRuntimeTunnelCommand,
    #[error("custom adapter requires endpoint adapter-definition bindings")]
    RuntimeTunnelAdapterCommandRequired,
    #[error("external observed tunnels cannot include mutating commands or traffic limits")]
    RuntimeTunnelObservedCannotMutate,
    #[error("runtime topology routes and cleanup require agent iproute2 ownership")]
    RuntimeTunnelTopologyRequiresAgentManagement,
    #[error("runtime tunnel traffic limit is invalid")]
    InvalidRuntimeTunnelTrafficLimit,
    #[error("runtime tunnel topology intent is invalid")]
    InvalidRuntimeTunnelTopology,
    #[error("runtime tunnel route is invalid")]
    InvalidRuntimeTunnelRoute,
    #[error("bandwidth must be between 10 and 10000 Mbps")]
    InvalidBandwidthMbps,
    #[error("tunnel MTU must be between 68 and 65535 bytes, and at least 1280 for SIT or IPv6")]
    InvalidTunnelMtu,
    #[error("agent-managed tunnel requires both endpoint MTUs")]
    TunnelMtuRequired,
    #[error("external tunnel manager owns MTU; endpoint MTUs must be omitted")]
    TunnelMtuExternallyOwned,
    #[error("OSPF configuration is invalid")]
    InvalidOspfConfig,
}

pub fn plan_tunnel(input: &TunnelPlanInput) -> Result<TunnelPlan, NetworkPlanError> {
    validate_plan_identity(input)?;
    validate_interface_name(&input.interface_name)?;
    validate_bandwidth_mbps(input.bandwidth_mbps)?;
    validate_runtime_tunnel_control(&input.runtime_control)?;
    validate_runtime_fou_options(input.kind, &input.runtime_control.fou)?;
    if input.runtime_control.manager != RuntimeTunnelManager::AgentIproute2Managed
        && !input.runtime_topology.is_default()
    {
        return Err(NetworkPlanError::RuntimeTunnelTopologyRequiresAgentManagement);
    }
    validate_runtime_topology_intent(&input.runtime_topology, &input.interface_name)?;
    if let Some(ospf) = &input.ospf {
        validate_ospf_config(ospf)?;
    }
    if input.runtime_control.manager == RuntimeTunnelManager::AgentIproute2Managed
        && input.kind.linux_tunnel_mode().is_none()
    {
        return Err(NetworkPlanError::UnsupportedRuntimeManagerTunnelKind);
    }
    let reserved_ipv4 = input
        .reserved_addresses
        .iter()
        .filter_map(|address| address.parse::<Ipv4Addr>().ok())
        .map(ipv4_to_u32)
        .collect::<HashSet<_>>();
    let reserved_ipv6 = input
        .reserved_addresses
        .iter()
        .filter_map(|address| address.parse::<Ipv6Addr>().ok())
        .map(ipv6_to_u128)
        .collect::<HashSet<_>>();
    let ipv4_tunnel = resolve_ipv4_tunnel(input, &reserved_ipv4)?;
    let ipv6_tunnel = resolve_ipv6_tunnel(input, &reserved_ipv6)?;
    validate_tunnel_mtus(
        input.runtime_control.manager,
        input.kind,
        input.left_mtu,
        input.right_mtu,
        ipv6_tunnel.is_some(),
    )?;
    if ipv4_tunnel.is_none() && ipv6_tunnel.is_none() {
        return Err(NetworkPlanError::TunnelAddressRequired);
    }
    let primary_family = primary_family(
        input.latency_primary_family,
        ipv4_tunnel.as_ref(),
        ipv6_tunnel.as_ref(),
    );
    let primary_tunnel = match primary_family {
        TunnelAddressFamily::Ipv4 => ipv4_tunnel
            .as_ref()
            .or(ipv6_tunnel.as_ref())
            .expect("at least one tunnel address pair exists"),
        TunnelAddressFamily::Ipv6 => ipv6_tunnel
            .as_ref()
            .or(ipv4_tunnel.as_ref())
            .expect("at least one tunnel address pair exists"),
    };
    let recommended_ospf_cost = input.ospf.as_ref().map(|ospf| {
        ospf_cost(
            ospf.policy,
            TunnelObservation {
                latency_ms: ospf.planned_latency_ms,
                packet_loss_ratio: ospf.planned_packet_loss_ratio,
                bandwidth_mbps: input.bandwidth_mbps,
                preference: ospf.preference,
            },
        )
    });
    let left_address = primary_tunnel.left.clone();
    let right_address = primary_tunnel.right.clone();
    let conflicts = plan_conflicts(input, &reserved_ipv4, &reserved_ipv6)?;

    Ok(TunnelPlan {
        name: input.name.clone(),
        interface_name: input.interface_name.clone(),
        kind: input.kind,
        runtime_control: input.runtime_control.clone(),
        runtime_topology: input.runtime_topology.clone(),
        left_client_id: input.left_client_id.clone(),
        right_client_id: input.right_client_id.clone(),
        left_remote_underlay: input.left_remote_underlay.clone(),
        left_local_underlay: input.left_local_underlay.clone(),
        right_remote_underlay: input.right_remote_underlay.clone(),
        right_local_underlay: input.right_local_underlay.clone(),
        left_tunnel_address: left_address.clone(),
        right_tunnel_address: right_address.clone(),
        tunnel_prefix_len: primary_tunnel.prefix_len,
        ipv4_tunnel: ipv4_tunnel.clone(),
        ipv6_tunnel: ipv6_tunnel.clone(),
        latency_primary_family: primary_family,
        bandwidth_mbps: input.bandwidth_mbps,
        left_mtu: input.left_mtu,
        right_mtu: input.right_mtu,
        ospf: input.ospf.clone(),
        recommended_ospf_cost,
        conflicts,
    })
}

fn validate_plan_identity(input: &TunnelPlanInput) -> Result<(), NetworkPlanError> {
    let name = input.name.trim();
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err(NetworkPlanError::InvalidPlanIdentity);
    }
    if input.left_client_id.trim().is_empty()
        || input.right_client_id.trim().is_empty()
        || input.left_client_id == input.right_client_id
    {
        return Err(NetworkPlanError::InvalidTunnelEndpoints);
    }
    validate_endpoint_underlay(
        &input.left_remote_underlay,
        input.left_local_underlay.as_deref(),
        input.runtime_control.manager,
    )?;
    validate_endpoint_underlay(
        &input.right_remote_underlay,
        input.right_local_underlay.as_deref(),
        input.runtime_control.manager,
    )?;
    Ok(())
}

fn validate_endpoint_underlay(
    remote: &str,
    local: Option<&str>,
    manager: RuntimeTunnelManager,
) -> Result<(), NetworkPlanError> {
    let remote = remote
        .parse::<IpAddr>()
        .map_err(|_| NetworkPlanError::InvalidUnderlayAddress)?;
    let local = local
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::parse::<IpAddr>)
        .transpose()
        .map_err(|_| NetworkPlanError::InvalidUnderlayAddress)?;
    if local.is_some_and(|local| local.is_ipv4() != remote.is_ipv4())
        || (manager == RuntimeTunnelManager::AgentIproute2Managed && !remote.is_ipv4())
    {
        return Err(NetworkPlanError::InvalidUnderlayAddress);
    }
    Ok(())
}

fn validate_bandwidth_mbps(value: u32) -> Result<(), NetworkPlanError> {
    if (MIN_TUNNEL_BANDWIDTH_MBPS..=MAX_TUNNEL_BANDWIDTH_MBPS).contains(&value) {
        Ok(())
    } else {
        Err(NetworkPlanError::InvalidBandwidthMbps)
    }
}

fn validate_tunnel_mtus(
    manager: RuntimeTunnelManager,
    kind: TunnelKind,
    left_mtu: Option<u16>,
    right_mtu: Option<u16>,
    ipv6_enabled: bool,
) -> Result<(), NetworkPlanError> {
    match manager {
        RuntimeTunnelManager::AgentIproute2Managed => {
            let (Some(left_mtu), Some(right_mtu)) = (left_mtu, right_mtu) else {
                return Err(NetworkPlanError::TunnelMtuRequired);
            };
            let requires_ipv6_mtu = ipv6_enabled || kind == TunnelKind::Sit;
            for value in [left_mtu, right_mtu] {
                if value < MIN_TUNNEL_MTU || (requires_ipv6_mtu && value < MIN_IPV6_TUNNEL_MTU) {
                    return Err(NetworkPlanError::InvalidTunnelMtu);
                }
            }
        }
        RuntimeTunnelManager::ExternalObserved | RuntimeTunnelManager::ExternalManagedAdapter => {
            if left_mtu.is_some() || right_mtu.is_some() {
                return Err(NetworkPlanError::TunnelMtuExternallyOwned);
            }
        }
    }
    Ok(())
}

fn validate_ospf_config(config: &TunnelOspfConfig) -> Result<(), NetworkPlanError> {
    let policy = config.policy;
    let policy_values = [
        policy.latency_weight,
        policy.loss_weight,
        policy.bandwidth_weight,
        policy.preference_bias,
    ];
    if !config.planned_latency_ms.is_finite()
        || !(0.0..=60_000.0).contains(&config.planned_latency_ms)
        || !config.planned_packet_loss_ratio.is_finite()
        || !(0.0..=1.0).contains(&config.planned_packet_loss_ratio)
        || !config.preference.is_finite()
        || !(0.1..=100.0).contains(&config.preference)
        || config.min_cost_delta == 0
        || !(1..=10).contains(&config.healthy_windows)
        || policy.min_cost == 0
        || policy.min_cost > policy.max_cost
        || policy_values
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
        || !config
            .left_adapter_definition_id
            .as_deref()
            .is_none_or(|value| valid_definition_id(Some(value)))
        || !config
            .right_adapter_definition_id
            .as_deref()
            .is_none_or(|value| valid_definition_id(Some(value)))
    {
        return Err(NetworkPlanError::InvalidOspfConfig);
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TunnelEndpointAllocation {
    pub ipv4_tunnel: Option<TunnelAddressPair>,
    pub ipv6_tunnel: Option<TunnelAddressPair>,
    pub latency_primary_family: TunnelAddressFamily,
}

pub fn allocate_tunnel_endpoints(
    ipv4_pool_cidr: Option<&str>,
    ipv6_pool_cidr: Option<&str>,
    reserved_addresses: &[String],
    include_ipv4: bool,
    include_ipv6: bool,
) -> Result<TunnelEndpointAllocation, NetworkPlanError> {
    if !include_ipv4 && !include_ipv6 {
        return Err(NetworkPlanError::TunnelAddressRequired);
    }
    let reserved_ipv4 = reserved_addresses
        .iter()
        .filter_map(|address| address.parse::<Ipv4Addr>().ok())
        .map(ipv4_to_u32)
        .collect::<HashSet<_>>();
    let reserved_ipv6 = reserved_addresses
        .iter()
        .filter_map(|address| address.parse::<Ipv6Addr>().ok())
        .map(ipv6_to_u128)
        .collect::<HashSet<_>>();

    let ipv4_tunnel = if include_ipv4 {
        let pool = ipv4_pool_cidr
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or(NetworkPlanError::AddressPoolRequired)?;
        let cidr = Ipv4Cidr::parse(pool)?;
        if cidr.prefix_len > 31 {
            return Err(NetworkPlanError::AddressPoolTooSmall);
        }
        let (left, right) = allocate_tunnel_pair(cidr, &reserved_ipv4)?;
        Some(TunnelAddressPair {
            left: left.to_string(),
            right: right.to_string(),
            prefix_len: 31,
        })
    } else {
        None
    };

    let ipv6_tunnel = if include_ipv6 {
        let pool = ipv6_pool_cidr
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or(NetworkPlanError::AddressPoolRequired)?;
        let cidr = Ipv6Cidr::parse(pool)?;
        if cidr.prefix_len > 127 {
            return Err(NetworkPlanError::AddressPoolTooSmall);
        }
        let (left, right) = allocate_tunnel_pair_v6(cidr, &reserved_ipv6)?;
        Some(TunnelAddressPair {
            left: left.to_string(),
            right: right.to_string(),
            prefix_len: 127,
        })
    } else {
        None
    };

    Ok(TunnelEndpointAllocation {
        latency_primary_family: primary_family(
            TunnelAddressFamily::Ipv4,
            ipv4_tunnel.as_ref(),
            ipv6_tunnel.as_ref(),
        ),
        ipv4_tunnel,
        ipv6_tunnel,
    })
}

pub fn render_tunnel_endpoint_config(
    plan: &TunnelPlan,
    side: TunnelEndpointSide,
) -> Result<TunnelEndpointConfig, NetworkPlanError> {
    validate_interface_name(&plan.interface_name)?;
    let (
        local_client_id,
        peer_client_id,
        remote_underlay,
        local_underlay,
        local_address,
        remote_address,
        local_mtu,
    ) = match side {
        TunnelEndpointSide::Left => (
            &plan.left_client_id,
            &plan.right_client_id,
            &plan.left_remote_underlay,
            &plan.left_local_underlay,
            &plan.left_tunnel_address,
            &plan.right_tunnel_address,
            plan.left_mtu,
        ),
        TunnelEndpointSide::Right => (
            &plan.right_client_id,
            &plan.left_client_id,
            &plan.right_remote_underlay,
            &plan.right_local_underlay,
            &plan.right_tunnel_address,
            &plan.left_tunnel_address,
            plan.right_mtu,
        ),
    };
    Ok(TunnelEndpointConfig {
        side,
        local_client_id: local_client_id.clone(),
        peer_client_id: peer_client_id.clone(),
        local_mtu,
        runtime_control: plan.runtime_control.clone(),
        remote_underlay: remote_underlay.clone(),
        local_underlay: local_underlay.clone(),
        local_tunnel_address: local_address.clone(),
        remote_tunnel_address: remote_address.clone(),
        tunnel_prefix_len: plan.tunnel_prefix_len,
        primary_family: plan.latency_primary_family,
        ipv4_tunnel: plan.ipv4_tunnel.clone(),
        ipv6_tunnel: plan.ipv6_tunnel.clone(),
    })
}

pub fn validate_runtime_tunnel_control(
    control: &RuntimeTunnelControl,
) -> Result<(), NetworkPlanError> {
    match control.manager {
        RuntimeTunnelManager::AgentIproute2Managed => {
            if control.left_adapter_definition_id.is_some()
                || control.right_adapter_definition_id.is_some()
            {
                return Err(NetworkPlanError::InvalidRuntimeTunnelCommand);
            }
        }
        RuntimeTunnelManager::ExternalObserved => {
            if control.left_adapter_definition_id.is_some()
                || control.right_adapter_definition_id.is_some()
                || !control.traffic_limit.is_default()
                || !control.fou.is_default()
            {
                return Err(NetworkPlanError::RuntimeTunnelObservedCannotMutate);
            }
        }
        RuntimeTunnelManager::ExternalManagedAdapter => {
            if !valid_definition_id(control.left_adapter_definition_id.as_deref())
                || !valid_definition_id(control.right_adapter_definition_id.as_deref())
            {
                return Err(NetworkPlanError::RuntimeTunnelAdapterCommandRequired);
            }
        }
    }

    validate_runtime_traffic_limit(&control.traffic_limit)?;
    Ok(())
}

fn valid_definition_id(value: Option<&str>) -> bool {
    value.is_some_and(|value| uuid::Uuid::parse_str(value).is_ok())
}

fn validate_runtime_fou_options(
    kind: TunnelKind,
    options: &RuntimeTunnelFouOptions,
) -> Result<(), NetworkPlanError> {
    if kind != TunnelKind::Fou && !options.is_default() {
        return Err(NetworkPlanError::InvalidRuntimeTunnelCommand);
    }
    if options.port == 0 || options.peer_port == 0 || options.ipproto == 0 {
        return Err(NetworkPlanError::InvalidRuntimeTunnelCommand);
    }
    Ok(())
}

pub fn validate_runtime_topology_intent(
    topology: &RuntimeTunnelTopologyIntent,
    current_interface_name: &str,
) -> Result<(), NetworkPlanError> {
    validate_interface_name(current_interface_name)?;
    if let Some(version) = &topology.version {
        if version.is_empty()
            || version.len() > MAX_RUNTIME_TOPOLOGY_VERSION_BYTES
            || version.as_bytes().contains(&0)
            || !version.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':')
            })
        {
            return Err(NetworkPlanError::InvalidRuntimeTunnelTopology);
        }
    }

    validate_runtime_interface_set(&topology.desired_interfaces)?;
    validate_runtime_interface_set(&topology.stale_interfaces)?;
    let desired = topology
        .desired_interfaces
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let stale = topology
        .stale_interfaces
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if !desired.is_empty() && !desired.contains(current_interface_name) {
        return Err(NetworkPlanError::InvalidRuntimeTunnelTopology);
    }
    if stale.contains(current_interface_name) || desired.iter().any(|name| stale.contains(name)) {
        return Err(NetworkPlanError::InvalidRuntimeTunnelTopology);
    }
    validate_runtime_routes(&topology.routes)?;
    validate_runtime_routes(&topology.stale_routes)?;
    Ok(())
}

fn validate_runtime_interface_set(interfaces: &[String]) -> Result<(), NetworkPlanError> {
    if interfaces.len() > MAX_RUNTIME_TOPOLOGY_INTERFACES {
        return Err(NetworkPlanError::InvalidRuntimeTunnelTopology);
    }
    let mut seen = HashSet::new();
    for interface in interfaces {
        validate_interface_name(interface)
            .map_err(|_| NetworkPlanError::InvalidRuntimeTunnelTopology)?;
        if !seen.insert(interface.as_str()) {
            return Err(NetworkPlanError::InvalidRuntimeTunnelTopology);
        }
    }
    Ok(())
}

fn validate_runtime_routes(routes: &[RuntimeTunnelRoute]) -> Result<(), NetworkPlanError> {
    if routes.len() > MAX_RUNTIME_TOPOLOGY_ROUTES {
        return Err(NetworkPlanError::InvalidRuntimeTunnelTopology);
    }
    let mut seen = HashSet::new();
    for route in routes {
        parse_ip_cidr(&route.destination_cidr)
            .map_err(|_| NetworkPlanError::InvalidRuntimeTunnelRoute)?;
        if let Some(via) = &route.via {
            via.parse::<IpAddr>()
                .map_err(|_| NetworkPlanError::InvalidRuntimeTunnelRoute)?;
        }
        if let Some(interface) = &route.interface_name {
            validate_interface_name(interface)?;
        }
        if route.metric == Some(0) {
            return Err(NetworkPlanError::InvalidRuntimeTunnelRoute);
        }
        let key = (
            route.destination_cidr.as_str(),
            route.via.as_deref().unwrap_or(""),
            route.interface_name.as_deref().unwrap_or(""),
            route.metric.unwrap_or(0),
        );
        if !seen.insert(key) {
            return Err(NetworkPlanError::InvalidRuntimeTunnelRoute);
        }
    }
    Ok(())
}

fn validate_runtime_traffic_limit(
    limit: &RuntimeTunnelTrafficLimit,
) -> Result<(), NetworkPlanError> {
    if let Some(value) = limit.ingress_kbps {
        if !(64..=1_000_000).contains(&value) {
            return Err(NetworkPlanError::InvalidRuntimeTunnelTrafficLimit);
        }
    }
    if let Some(value) = limit.egress_kbps {
        if !(64..=1_000_000).contains(&value) {
            return Err(NetworkPlanError::InvalidRuntimeTunnelTrafficLimit);
        }
    }
    if let Some(value) = limit.burst_kb {
        if !(1..=1_048_576).contains(&value) {
            return Err(NetworkPlanError::InvalidRuntimeTunnelTrafficLimit);
        }
    }
    Ok(())
}

fn validate_interface_name(name: &str) -> Result<(), NetworkPlanError> {
    let valid = !name.is_empty()
        && name.len() <= 15
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'));
    if valid {
        Ok(())
    } else {
        Err(NetworkPlanError::InvalidInterfaceName)
    }
}

#[derive(Clone, Copy)]
struct Ipv4Cidr {
    network: u32,
    broadcast: u32,
    prefix_len: u8,
}

impl Ipv4Cidr {
    fn parse(value: &str) -> Result<Self, NetworkPlanError> {
        let (address, prefix) = value.split_once('/').ok_or(NetworkPlanError::InvalidCidr)?;
        let address = address
            .parse::<Ipv4Addr>()
            .map_err(|_| NetworkPlanError::InvalidCidr)?;
        let prefix_len = prefix
            .parse::<u8>()
            .map_err(|_| NetworkPlanError::InvalidCidr)?;
        if prefix_len > 32 {
            return Err(NetworkPlanError::InvalidCidr);
        }
        let mask = if prefix_len == 0 {
            0
        } else {
            u32::MAX << (32 - prefix_len)
        };
        let network = ipv4_to_u32(address) & mask;
        let broadcast = network | !mask;
        Ok(Self {
            network,
            broadcast,
            prefix_len,
        })
    }
}

fn allocate_tunnel_pair(
    cidr: Ipv4Cidr,
    reserved: &HashSet<u32>,
) -> Result<(Ipv4Addr, Ipv4Addr), NetworkPlanError> {
    let mut candidate = cidr.network;
    while candidate < cidr.broadcast {
        let peer = candidate.saturating_add(1);
        if peer > cidr.broadcast {
            break;
        }
        if !reserved.contains(&candidate) && !reserved.contains(&peer) {
            return Ok((u32_to_ipv4(candidate), u32_to_ipv4(peer)));
        }
        candidate = candidate.saturating_add(2);
    }
    Err(NetworkPlanError::AddressPoolExhausted)
}

#[derive(Clone, Copy)]
struct Ipv6Cidr {
    network: u128,
    broadcast: u128,
    prefix_len: u8,
}

impl Ipv6Cidr {
    fn parse(value: &str) -> Result<Self, NetworkPlanError> {
        let (address, prefix) = value.split_once('/').ok_or(NetworkPlanError::InvalidCidr)?;
        let address = address
            .parse::<Ipv6Addr>()
            .map_err(|_| NetworkPlanError::InvalidCidr)?;
        let prefix_len = prefix
            .parse::<u8>()
            .map_err(|_| NetworkPlanError::InvalidCidr)?;
        if prefix_len > 128 {
            return Err(NetworkPlanError::InvalidCidr);
        }
        let mask = if prefix_len == 0 {
            0
        } else {
            u128::MAX << (128 - prefix_len)
        };
        let network = ipv6_to_u128(address) & mask;
        let broadcast = network | !mask;
        Ok(Self {
            network,
            broadcast,
            prefix_len,
        })
    }
}

fn allocate_tunnel_pair_v6(
    cidr: Ipv6Cidr,
    reserved: &HashSet<u128>,
) -> Result<(Ipv6Addr, Ipv6Addr), NetworkPlanError> {
    let mut candidate = cidr.network;
    while candidate < cidr.broadcast {
        let peer = candidate.saturating_add(1);
        if peer > cidr.broadcast {
            break;
        }
        if !reserved.contains(&candidate) && !reserved.contains(&peer) {
            return Ok((u128_to_ipv6(candidate), u128_to_ipv6(peer)));
        }
        candidate = candidate.saturating_add(2);
    }
    Err(NetworkPlanError::AddressPoolExhausted)
}

fn resolve_ipv4_tunnel(
    input: &TunnelPlanInput,
    _reserved: &HashSet<u32>,
) -> Result<Option<TunnelAddressPair>, NetworkPlanError> {
    if let Some(pair) = &input.ipv4_tunnel {
        validate_ipv4_pair(pair)?;
        return Ok(Some(pair.clone()));
    }
    Ok(None)
}

fn resolve_ipv6_tunnel(
    input: &TunnelPlanInput,
    _reserved: &HashSet<u128>,
) -> Result<Option<TunnelAddressPair>, NetworkPlanError> {
    if let Some(pair) = &input.ipv6_tunnel {
        validate_ipv6_pair(pair)?;
        return Ok(Some(pair.clone()));
    }
    Ok(None)
}

fn validate_ipv4_pair(pair: &TunnelAddressPair) -> Result<(), NetworkPlanError> {
    let left = pair
        .left
        .parse::<Ipv4Addr>()
        .map_err(|_| NetworkPlanError::InvalidCidr)?;
    let right = pair
        .right
        .parse::<Ipv4Addr>()
        .map_err(|_| NetworkPlanError::InvalidCidr)?;
    if pair.prefix_len > 32 {
        return Err(NetworkPlanError::InvalidCidr);
    }
    let mask = if pair.prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - pair.prefix_len)
    };
    if left == right || ipv4_to_u32(left) & mask != ipv4_to_u32(right) & mask {
        return Err(NetworkPlanError::InvalidCidr);
    }
    Ok(())
}

fn validate_ipv6_pair(pair: &TunnelAddressPair) -> Result<(), NetworkPlanError> {
    let left = pair
        .left
        .parse::<Ipv6Addr>()
        .map_err(|_| NetworkPlanError::InvalidCidr)?;
    let right = pair
        .right
        .parse::<Ipv6Addr>()
        .map_err(|_| NetworkPlanError::InvalidCidr)?;
    if pair.prefix_len > 128 {
        return Err(NetworkPlanError::InvalidCidr);
    }
    let mask = if pair.prefix_len == 0 {
        0
    } else {
        u128::MAX << (128 - pair.prefix_len)
    };
    if left == right || ipv6_to_u128(left) & mask != ipv6_to_u128(right) & mask {
        return Err(NetworkPlanError::InvalidCidr);
    }
    Ok(())
}

fn primary_family(
    requested: TunnelAddressFamily,
    ipv4: Option<&TunnelAddressPair>,
    ipv6: Option<&TunnelAddressPair>,
) -> TunnelAddressFamily {
    match requested {
        TunnelAddressFamily::Ipv4 if ipv4.is_some() => TunnelAddressFamily::Ipv4,
        TunnelAddressFamily::Ipv6 if ipv6.is_some() => TunnelAddressFamily::Ipv6,
        _ if ipv4.is_some() => TunnelAddressFamily::Ipv4,
        _ => TunnelAddressFamily::Ipv6,
    }
}

fn plan_conflicts(
    input: &TunnelPlanInput,
    reserved_ipv4: &HashSet<u32>,
    reserved_ipv6: &HashSet<u128>,
) -> Result<Vec<String>, NetworkPlanError> {
    let mut conflicts = Vec::new();
    if !input.address_pool_cidr.trim().is_empty() {
        let cidr = Ipv4Cidr::parse(&input.address_pool_cidr)?;
        conflicts.extend(
            reserved_ipv4
                .iter()
                .copied()
                .filter(|address| *address >= cidr.network && *address <= cidr.broadcast)
                .map(u32_to_ipv4)
                .map(|address| format!("reserved address {address} is inside requested IPv4 pool")),
        );
    }
    if let Some(pool) = input
        .ipv6_address_pool_cidr
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let cidr = Ipv6Cidr::parse(pool)?;
        conflicts.extend(
            reserved_ipv6
                .iter()
                .copied()
                .filter(|address| *address >= cidr.network && *address <= cidr.broadcast)
                .map(u128_to_ipv6)
                .map(|address| format!("reserved address {address} is inside requested IPv6 pool")),
        );
    }
    Ok(conflicts)
}

fn parse_ip_cidr(value: &str) -> Result<(), NetworkPlanError> {
    if Ipv4Cidr::parse(value).is_ok() || Ipv6Cidr::parse(value).is_ok() {
        Ok(())
    } else {
        Err(NetworkPlanError::InvalidCidr)
    }
}

fn ipv4_to_u32(address: Ipv4Addr) -> u32 {
    u32::from_be_bytes(address.octets())
}

fn u32_to_ipv4(value: u32) -> Ipv4Addr {
    Ipv4Addr::from(value.to_be_bytes())
}

fn ipv6_to_u128(address: Ipv6Addr) -> u128 {
    u128::from_be_bytes(address.octets())
}

fn u128_to_ipv6(value: u128) -> Ipv6Addr {
    Ipv6Addr::from(value.to_be_bytes())
}
