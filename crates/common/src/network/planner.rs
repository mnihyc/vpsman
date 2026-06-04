use std::{collections::HashSet, net::Ipv4Addr};

use super::{
    cost::ospf_cost,
    models::{
        RuntimeTunnelCommand, RuntimeTunnelControl, RuntimeTunnelFouOptions, RuntimeTunnelManager,
        RuntimeTunnelRoute, RuntimeTunnelTopologyIntent, RuntimeTunnelTrafficLimit,
        TunnelEndpointConfig, TunnelEndpointSide, TunnelKind, TunnelObservation, TunnelPlan,
        TunnelPlanInput, MANAGED_BIRD2_FILE, MANAGED_IFUPDOWN_FILE,
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
    #[error("tunnel kind is not supported by selected network backend")]
    UnsupportedBackendTunnelKind,
    #[error("runtime tunnel command must be bounded and use absolute argv")]
    InvalidRuntimeTunnelCommand,
    #[error("external managed adapter requires at least one lifecycle command")]
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
    let cidr = Ipv4Cidr::parse(&input.address_pool_cidr)?;
    if cidr.prefix_len > 31 {
        return Err(NetworkPlanError::AddressPoolTooSmall);
    }

    let reserved = input
        .reserved_addresses
        .iter()
        .filter_map(|address| address.parse::<Ipv4Addr>().ok())
        .map(ipv4_to_u32)
        .collect::<HashSet<_>>();
    let (left_address, right_address) = allocate_tunnel_pair(cidr, &reserved)?;
    let observation = TunnelObservation {
        latency_ms: input.latency_ms,
        packet_loss_ratio: input.packet_loss_ratio,
        bandwidth: input.bandwidth,
        preference: input.preference,
    };
    let recommended_ospf_cost = ospf_cost(input.ospf_policy, observation);
    let left_address = left_address.to_string();
    let right_address = right_address.to_string();
    let ifupdown_file = MANAGED_IFUPDOWN_FILE.to_string();
    let bird2_file = MANAGED_BIRD2_FILE.to_string();
    let ifupdown_snippet = render_runtime_snippet(
        TunnelSnippetInput {
            name: &input.name,
            interface_name: &input.interface_name,
            kind: input.kind,
            local_underlay: &input.left_underlay,
            remote_underlay: &input.right_underlay,
            local_address: &left_address,
            remote_address: &right_address,
            fou: &input.runtime_control.fou,
        },
        input.runtime_control.manager,
    );
    let touched_files = touched_files_for_runtime(input.runtime_control.manager);

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
        tunnel_prefix_len: 31,
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
        conflicts: reserved
            .iter()
            .copied()
            .filter(|address| *address >= cidr.network && *address <= cidr.broadcast)
            .map(u32_to_ipv4)
            .map(|address| format!("reserved address {address} is inside requested pool"))
            .collect(),
        mutates_host: false,
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
                local_address,
                remote_address,
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
        Ipv4Cidr::parse(&route.destination_cidr)
            .map_err(|_| NetworkPlanError::InvalidRuntimeTunnelRoute)?;
        if let Some(via) = &route.via {
            via.parse::<Ipv4Addr>()
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
        || !(1..=120).contains(&command.timeout_secs)
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

struct TunnelSnippetInput<'a> {
    name: &'a str,
    interface_name: &'a str,
    kind: TunnelKind,
    local_underlay: &'a str,
    remote_underlay: &'a str,
    local_address: &'a str,
    remote_address: &'a str,
    fou: &'a RuntimeTunnelFouOptions,
}

fn render_ifupdown_snippet(input: TunnelSnippetInput<'_>) -> String {
    let linux_mode = input
        .kind
        .linux_tunnel_mode()
        .expect("iproute2-managed tunnel kind is validated before rendering");
    let mut lines = vec![
        format!("# vpsman tunnel {}: generated plan only", input.name),
        format!("auto {}", input.interface_name),
        format!("iface {} inet static", input.interface_name),
        format!("    address {}", input.local_address),
        "    netmask 255.255.255.254".to_string(),
        format!("    pointopoint {}", input.remote_address),
    ];
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
    lines.join("\n")
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
                "# vpsman tunnel {}: external managed adapter runtime tunnel",
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
    let mut steps = vec!["review generated snippets before apply".to_string()];
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
