use std::{
    collections::HashSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};

use super::{
    cost::ospf_cost,
    models::{
        RuntimeTunnelCommand, RuntimeTunnelControl, RuntimeTunnelFouOptions, RuntimeTunnelManager,
        RuntimeTunnelRoute, RuntimeTunnelTopologyIntent, RuntimeTunnelTrafficLimit,
        TunnelAddressFamily, TunnelAddressPair, TunnelEndpointConfig, TunnelEndpointSide,
        TunnelKind, TunnelObservation, TunnelPlan, TunnelPlanInput, MANAGED_BIRD2_FILE,
        MANAGED_IFUPDOWN_FILE,
    },
};

const MAX_RUNTIME_TOPOLOGY_VERSION_BYTES: usize = 128;
const MAX_RUNTIME_TOPOLOGY_INTERFACES: usize = 128;
const MAX_RUNTIME_TOPOLOGY_ROUTES: usize = 256;

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum NetworkPlanError {
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
    #[error("tunnel kind is not supported by selected network backend")]
    UnsupportedBackendTunnelKind,
    #[error("runtime tunnel command must be bounded and use absolute argv")]
    InvalidRuntimeTunnelCommand,
    #[error("custom adapter requires at least one lifecycle command")]
    RuntimeTunnelAdapterCommandRequired,
    #[error("external observed tunnels cannot include mutating commands or traffic limits")]
    RuntimeTunnelObservedCannotMutate,
    #[error("runtime tunnel traffic limit is invalid")]
    InvalidRuntimeTunnelTrafficLimit,
    #[error("runtime tunnel topology intent is invalid")]
    InvalidRuntimeTunnelTopology,
    #[error("runtime tunnel route is invalid")]
    InvalidRuntimeTunnelRoute,
}

pub fn plan_tunnel(input: &TunnelPlanInput) -> Result<TunnelPlan, NetworkPlanError> {
    validate_interface_name(&input.interface_name)?;
    validate_runtime_tunnel_control(&input.runtime_control)?;
    validate_runtime_fou_options(input.kind, &input.runtime_control.fou)?;
    validate_runtime_topology_intent(&input.runtime_topology, &input.interface_name)?;
    if input.runtime_control.manager == RuntimeTunnelManager::AgentIproute2Managed
        && input.kind.linux_tunnel_mode().is_none()
    {
        return Err(NetworkPlanError::UnsupportedBackendTunnelKind);
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
    let observation = TunnelObservation {
        latency_ms: input.latency_ms,
        packet_loss_ratio: input.packet_loss_ratio,
        bandwidth: input.bandwidth,
        preference: input.preference,
    };
    let recommended_ospf_cost = ospf_cost(input.ospf_policy, observation);
    let left_address = primary_tunnel.left.clone();
    let right_address = primary_tunnel.right.clone();
    let ifupdown_file = MANAGED_IFUPDOWN_FILE.to_string();
    let bird2_file = MANAGED_BIRD2_FILE.to_string();
    let ifupdown_snippet = render_runtime_snippet(
        TunnelSnippetInput {
            name: &input.name,
            interface_name: &input.interface_name,
            kind: input.kind,
            local_underlay: &input.left_underlay,
            remote_underlay: &input.right_underlay,
            ipv4: ipv4_tunnel.as_ref().map(|pair| EndpointAddressPair {
                local: pair.left.as_str(),
                remote: pair.right.as_str(),
                prefix_len: pair.prefix_len,
            }),
            ipv6: ipv6_tunnel.as_ref().map(|pair| EndpointAddressPair {
                local: pair.left.as_str(),
                remote: pair.right.as_str(),
                prefix_len: pair.prefix_len,
            }),
            fou: &input.runtime_control.fou,
        },
        input.runtime_control.manager,
    );
    let touched_files = touched_files_for_runtime(input.runtime_control.manager);
    let conflicts = plan_conflicts(input, &reserved_ipv4, &reserved_ipv6)?;

    Ok(TunnelPlan {
        name: input.name.clone(),
        interface_name: input.interface_name.clone(),
        kind: input.kind,
        runtime_control: input.runtime_control.clone(),
        runtime_topology: input.runtime_topology.clone(),
        left_client_id: input.left_client_id.clone(),
        right_client_id: input.right_client_id.clone(),
        left_underlay: input.left_underlay.clone(),
        right_underlay: input.right_underlay.clone(),
        left_tunnel_address: left_address.clone(),
        right_tunnel_address: right_address.clone(),
        tunnel_prefix_len: primary_tunnel.prefix_len,
        ipv4_tunnel: ipv4_tunnel.clone(),
        ipv6_tunnel: ipv6_tunnel.clone(),
        latency_primary_family: primary_family,
        bandwidth: input.bandwidth,
        recommended_ospf_cost,
        ifupdown_file: ifupdown_file.clone(),
        bird2_file: bird2_file.clone(),
        ifupdown_snippet,
        bird2_interface_snippet: render_bird2_interface_snippet(
            input.kind,
            &input.name,
            &input.interface_name,
            &input.left_client_id,
            &input.right_client_id,
            recommended_ospf_cost,
        ),
        touched_files,
        validation_steps: validation_steps_for_runtime(input.runtime_control.manager),
        rollback_notes: rollback_notes_for_runtime(input.runtime_control.manager),
        conflicts,
        mutates_host: false,
    })
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
        local_underlay,
        remote_underlay,
        local_address,
        remote_address,
    ) = match side {
        TunnelEndpointSide::Left => (
            &plan.left_client_id,
            &plan.right_client_id,
            &plan.left_underlay,
            &plan.right_underlay,
            &plan.left_tunnel_address,
            &plan.right_tunnel_address,
        ),
        TunnelEndpointSide::Right => (
            &plan.right_client_id,
            &plan.left_client_id,
            &plan.right_underlay,
            &plan.left_underlay,
            &plan.right_tunnel_address,
            &plan.left_tunnel_address,
        ),
    };
    Ok(TunnelEndpointConfig {
        side,
        local_client_id: local_client_id.clone(),
        peer_client_id: peer_client_id.clone(),
        runtime_control: plan.runtime_control.clone(),
        ifupdown_file: MANAGED_IFUPDOWN_FILE.to_string(),
        bird2_file: MANAGED_BIRD2_FILE.to_string(),
        ifupdown_snippet: render_runtime_snippet(
            TunnelSnippetInput {
                name: &plan.name,
                interface_name: &plan.interface_name,
                kind: plan.kind,
                local_underlay,
                remote_underlay,
                ipv4: plan.ipv4_tunnel.as_ref().map(|pair| EndpointAddressPair {
                    local: address_for_side(pair, side, true),
                    remote: address_for_side(pair, side, false),
                    prefix_len: pair.prefix_len,
                }),
                ipv6: plan.ipv6_tunnel.as_ref().map(|pair| EndpointAddressPair {
                    local: address_for_side(pair, side, true),
                    remote: address_for_side(pair, side, false),
                    prefix_len: pair.prefix_len,
                }),
                fou: &plan.runtime_control.fou,
            },
            plan.runtime_control.manager,
        ),
        bird2_interface_snippet: render_bird2_interface_snippet(
            plan.kind,
            &plan.name,
            &plan.interface_name,
            local_client_id,
            peer_client_id,
            plan.recommended_ospf_cost,
        ),
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
    let has_mutating_command = control.startup.is_some()
        || control.stop.is_some()
        || control.cleanup.is_some()
        || control.restart.is_some()
        || control.traffic_limit_apply.is_some();
    let has_lifecycle_command =
        has_mutating_command || control.status.is_some() || !control.traffic_limit.is_default();

    match control.manager {
        RuntimeTunnelManager::AgentIproute2Managed => {
            if control.startup.is_some()
                || control.stop.is_some()
                || control.cleanup.is_some()
                || control.restart.is_some()
                || control.status.is_some()
                || control.traffic_limit_apply.is_some()
            {
                return Err(NetworkPlanError::InvalidRuntimeTunnelCommand);
            }
        }
        RuntimeTunnelManager::ExternalObserved => {
            if has_lifecycle_command {
                return Err(NetworkPlanError::RuntimeTunnelObservedCannotMutate);
            }
        }
        RuntimeTunnelManager::ExternalManagedAdapter => {
            if !has_lifecycle_command {
                return Err(NetworkPlanError::RuntimeTunnelAdapterCommandRequired);
            }
            if !control.traffic_limit.is_default() && control.traffic_limit_apply.is_none() {
                return Err(NetworkPlanError::InvalidRuntimeTunnelCommand);
            }
        }
    }

    validate_runtime_command(control.startup.as_ref())?;
    validate_runtime_command(control.stop.as_ref())?;
    validate_runtime_command(control.cleanup.as_ref())?;
    validate_runtime_command(control.restart.as_ref())?;
    validate_runtime_command(control.status.as_ref())?;
    validate_runtime_command(control.traffic_limit_apply.as_ref())?;
    validate_runtime_traffic_limit(&control.traffic_limit)?;
    Ok(())
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
        validate_interface_name(interface)?;
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

fn validate_runtime_command(
    command: Option<&RuntimeTunnelCommand>,
) -> Result<(), NetworkPlanError> {
    let Some(command) = command else {
        return Ok(());
    };
    if command.argv.is_empty()
        || command.argv.len() > 32
        || !command.argv[0].starts_with('/')
        || !(1..=120).contains(&command.max_timeout_secs)
        || !(1024..=64 * 1024).contains(&command.max_output_bytes)
    {
        return Err(NetworkPlanError::InvalidRuntimeTunnelCommand);
    }
    for arg in &command.argv {
        if arg.is_empty()
            || arg.len() > 4096
            || arg.as_bytes().contains(&0)
            || arg.chars().any(char::is_control)
        {
            return Err(NetworkPlanError::InvalidRuntimeTunnelCommand);
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
    pair.left
        .parse::<Ipv4Addr>()
        .map_err(|_| NetworkPlanError::InvalidCidr)?;
    pair.right
        .parse::<Ipv4Addr>()
        .map_err(|_| NetworkPlanError::InvalidCidr)?;
    if pair.prefix_len > 32 {
        return Err(NetworkPlanError::InvalidCidr);
    }
    Ok(())
}

fn validate_ipv6_pair(pair: &TunnelAddressPair) -> Result<(), NetworkPlanError> {
    pair.left
        .parse::<Ipv6Addr>()
        .map_err(|_| NetworkPlanError::InvalidCidr)?;
    pair.right
        .parse::<Ipv6Addr>()
        .map_err(|_| NetworkPlanError::InvalidCidr)?;
    if pair.prefix_len > 128 {
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

fn address_for_side(pair: &TunnelAddressPair, side: TunnelEndpointSide, local: bool) -> &str {
    match (side, local) {
        (TunnelEndpointSide::Left, true) | (TunnelEndpointSide::Right, false) => &pair.left,
        (TunnelEndpointSide::Left, false) | (TunnelEndpointSide::Right, true) => &pair.right,
    }
}

#[derive(Clone, Copy)]
struct EndpointAddressPair<'a> {
    local: &'a str,
    remote: &'a str,
    prefix_len: u8,
}

#[derive(Clone, Copy)]
struct TunnelSnippetInput<'a> {
    name: &'a str,
    interface_name: &'a str,
    kind: TunnelKind,
    local_underlay: &'a str,
    remote_underlay: &'a str,
    ipv4: Option<EndpointAddressPair<'a>>,
    ipv6: Option<EndpointAddressPair<'a>>,
    fou: &'a RuntimeTunnelFouOptions,
}

fn render_ifupdown_snippet(input: TunnelSnippetInput<'_>) -> String {
    let linux_mode = input
        .kind
        .linux_tunnel_mode()
        .expect("iproute2-managed tunnel kind is validated before rendering");
    let mut lines = vec![format!(
        "# vpsman tunnel {}: server-managed runtime config",
        input.name
    )];
    if let Some(ipv4) = input.ipv4 {
        lines.extend(render_ifupdown_ipv4_stanza(input, ipv4, linux_mode, true));
    }
    if let Some(ipv6) = input.ipv6 {
        lines.extend(render_ifupdown_ipv6_stanza(
            input,
            ipv6,
            linux_mode,
            input.ipv4.is_none(),
        ));
    }
    lines.join("\n")
}

fn render_ifupdown_ipv4_stanza(
    input: TunnelSnippetInput<'_>,
    address: EndpointAddressPair<'_>,
    linux_mode: &str,
    include_lifecycle: bool,
) -> Vec<String> {
    let mut lines = vec![
        format!("auto {}", input.interface_name),
        format!("iface {} inet static", input.interface_name),
        format!("    address {}", address.local),
        format!("    netmask {}", ipv4_netmask(address.prefix_len)),
        format!("    pointopoint {}", address.remote),
    ];
    if include_lifecycle {
        append_tunnel_lifecycle(&mut lines, input, linux_mode);
    }
    lines
}

fn render_ifupdown_ipv6_stanza(
    input: TunnelSnippetInput<'_>,
    address: EndpointAddressPair<'_>,
    linux_mode: &str,
    include_lifecycle: bool,
) -> Vec<String> {
    let mut lines = vec![
        format!("auto {}", input.interface_name),
        format!("iface {} inet6 static", input.interface_name),
        format!("    address {}", address.local),
        format!("    netmask {}", address.prefix_len),
        format!("    pointopoint {}", address.remote),
    ];
    if include_lifecycle {
        append_tunnel_lifecycle(&mut lines, input, linux_mode);
    }
    lines
}

fn append_tunnel_lifecycle(
    lines: &mut Vec<String>,
    input: TunnelSnippetInput<'_>,
    linux_mode: &str,
) {
    if input.kind == TunnelKind::Fou {
        lines.push(format!(
            "    pre-up ip fou add port {} ipproto {} || true",
            input.fou.port, input.fou.ipproto
        ));
    }
    let mut tunnel_command = format!(
        "    pre-up ip tunnel add $IFACE mode {} remote {} local {} ttl 255",
        linux_mode, input.remote_underlay, input.local_underlay
    );
    if input.kind == TunnelKind::Fou {
        tunnel_command.push_str(&format!(
            " encap fou encap-sport auto encap-dport {}",
            input.fou.peer_port
        ));
    }
    lines.push(tunnel_command);
    lines.push("    up ip link set $IFACE up".to_string());
    lines.push("    post-down ip tunnel del $IFACE || true".to_string());
    if input.kind == TunnelKind::Fou {
        lines.push(format!(
            "    post-down ip fou del port {} || true",
            input.fou.port
        ));
    }
}

fn render_runtime_snippet(input: TunnelSnippetInput<'_>, manager: RuntimeTunnelManager) -> String {
    match manager {
        RuntimeTunnelManager::AgentIproute2Managed => render_ifupdown_snippet(input),
        RuntimeTunnelManager::ExternalObserved => [
            format!(
                "# vpsman tunnel {}: external observed runtime tunnel",
                input.name
            ),
            format!(
                "# interface {} is owned by an external program and is not created by vpsman",
                input.interface_name
            ),
            "# vpsman will observe status, probe/speed evidence, and manage the Bird2 block"
                .to_string(),
        ]
        .join("\n"),
        RuntimeTunnelManager::ExternalManagedAdapter => [
            format!(
                "# vpsman tunnel {}: custom adapter runtime tunnel",
                input.name
            ),
            format!(
                "# interface {} is created, restarted, shaped, or stopped by adapter commands",
                input.interface_name
            ),
            "# vpsman will run bounded adapter argv, observe evidence, and manage the Bird2 block"
                .to_string(),
        ]
        .join("\n"),
    }
}

fn touched_files_for_runtime(manager: RuntimeTunnelManager) -> Vec<String> {
    match manager {
        RuntimeTunnelManager::AgentIproute2Managed => vec![
            MANAGED_IFUPDOWN_FILE.to_string(),
            MANAGED_BIRD2_FILE.to_string(),
        ],
        RuntimeTunnelManager::ExternalObserved | RuntimeTunnelManager::ExternalManagedAdapter => {
            vec![MANAGED_BIRD2_FILE.to_string()]
        }
    }
}

fn validation_steps_for_runtime(manager: RuntimeTunnelManager) -> Vec<String> {
    let mut steps = vec!["review generated runtime snippets before enabling the plan".to_string()];
    match manager {
        RuntimeTunnelManager::AgentIproute2Managed => {
            steps.push(
                "run ifreload --syntax-check for ifupdown2-managed snippets where available"
                    .to_string(),
            );
        }
        RuntimeTunnelManager::ExternalObserved => {
            steps.push("confirm the external interface exists before Bird2 reload".to_string());
            steps.push(
                "capture status, latency, and speed evidence before accepting OSPF cost"
                    .to_string(),
            );
        }
        RuntimeTunnelManager::ExternalManagedAdapter => {
            steps.push(
                "run adapter status/start or restart evidence before Bird2 reload".to_string(),
            );
            steps.push(
                "confirm adapter traffic-limit output when shaping is configured".to_string(),
            );
        }
    }
    steps.push("run bird -p before Bird2 reload where available".to_string());
    steps.push("verify tunnel latency and packet loss before accepting OSPF cost".to_string());
    steps
}

fn rollback_notes_for_runtime(manager: RuntimeTunnelManager) -> Vec<String> {
    match manager {
        RuntimeTunnelManager::AgentIproute2Managed => vec![
            "remove only the vpsman-managed interface block from /etc/network/interfaces.d/vpsman-tunnels".to_string(),
            "remove only the matching vpsman-managed Bird2 interface block".to_string(),
            "reload networking and Bird2 after validation succeeds".to_string(),
        ],
        RuntimeTunnelManager::ExternalObserved => vec![
            "do not delete the external interface from vpsman rollback".to_string(),
            "remove only the matching vpsman-managed Bird2 interface block".to_string(),
            "reload Bird2 after validation succeeds".to_string(),
        ],
        RuntimeTunnelManager::ExternalManagedAdapter => vec![
            "run the adapter stop command only when rollback is intended to stop runtime ownership".to_string(),
            "remove only the matching vpsman-managed Bird2 interface block".to_string(),
            "reload Bird2 after validation succeeds".to_string(),
        ],
    }
}

fn render_bird2_interface_snippet(
    kind: TunnelKind,
    name: &str,
    interface_name: &str,
    local_client_id: &str,
    peer_client_id: &str,
    ospf_cost: u16,
) -> String {
    [
        format!(
            "# vpsman {} tunnel {}: {} -> {}",
            kind.bird2_label(),
            name,
            local_client_id,
            peer_client_id
        ),
        format!("interface \"{}\" {{", interface_name),
        "  type ptp;".to_string(),
        format!("  cost {ospf_cost};"),
        "};".to_string(),
    ]
    .join("\n")
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

fn ipv4_netmask(prefix_len: u8) -> Ipv4Addr {
    let prefix_len = prefix_len.min(32);
    let mask = if prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - prefix_len)
    };
    u32_to_ipv4(mask)
}
