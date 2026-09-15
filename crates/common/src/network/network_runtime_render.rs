//! Pure native rendering shared by the agent and the draft-plan preview.
//! This module never inspects a host, accesses credentials, or executes commands.

use std::{
    collections::HashSet,
    net::{IpAddr, Ipv6Addr},
    path::Path,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    render_tunnel_endpoint_config, NetworkPlanError, RuntimeTunnelCommand, RuntimeTunnelManager,
    RuntimeTunnelOpenvpnTransport, RuntimeTunnelRoute, RuntimeTunnelTrafficLimit,
    TunnelEndpointConfig, TunnelEndpointSide, TunnelKind, TunnelPlan,
};

/// The existing runtime-adapter command contract, also used by lifecycle hooks.
pub fn validate_runtime_tunnel_hook(
    command: &RuntimeTunnelCommand,
) -> Result<(), NetworkPlanError> {
    if command.argv.is_empty()
        || command.argv.len() > 32
        || !command.argv[0].starts_with('/')
        || command
            .argv
            .iter()
            .any(|arg| arg.is_empty() || arg.len() > 4096 || arg.contains('\0'))
        || !(1..=120).contains(&command.max_timeout_secs)
        || !(1024..=64 * 1024).contains(&command.max_output_bytes)
    {
        return Err(NetworkPlanError::InvalidRuntimeTunnelCommand);
    }
    Ok(())
}

/// Read just enough native syntax to identify ownership boundaries. Values and
/// unsupported tuning directives are intentionally left to OpenVPN itself.
fn openvpn_directive(line: &str) -> Option<String> {
    let mut tokens = Vec::new();
    let mut chars = line.trim_start_matches('\u{feff}').chars().peekable();
    while tokens.len() < 3 {
        while chars.peek().is_some_and(|ch| ch.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none_or(|ch| matches!(ch, '#' | ';')) {
            break;
        }
        let mut token = String::new();
        let mut quote = None;
        while let Some(ch) = chars.next() {
            match (quote, ch) {
                (None, '"' | '\'') => quote = Some(ch),
                (Some(delimiter), ch) if delimiter == ch => quote = None,
                (_, '\\') => {
                    if let Some(next) = chars.next() {
                        token.push(next);
                    }
                }
                (None, ch) if ch.is_whitespace() => break,
                _ => token.push(ch),
            }
        }
        tokens.push(token);
    }
    let first = tokens.first()?.trim_start_matches("--");
    let directive = if first == "setenv" && tokens.get(1).is_some_and(|token| token == "opt") {
        tokens.get(2)?.trim_start_matches("--")
    } else {
        first
    };
    Some(
        directive
            .trim_start_matches('<')
            .trim_start_matches('/')
            .trim_end_matches('>')
            .to_string(),
    )
}

fn openvpn_directive_is_owned(directive: &str) -> bool {
    matches!(
        directive,
        // Interface, endpoint, address and route ownership.
        "mode" | "dev" | "dev-type" | "dev-node" | "topology" | "proto"
        | "local" | "remote" | "port" | "lport" | "rport" | "bind" | "nobind"
        | "tun-mtu" | "ifconfig" | "ifconfig-ipv6"
        | "ifconfig-noexec" | "ifconfig-pool" | "ifconfig-pool-persist" | "ifconfig-ipv6-pool"
        | "route" | "route-ipv6" | "route-gateway" | "route-ipv6-gateway" | "route-noexec"
        | "redirect-gateway" | "redirect-private" | "client" | "server" | "server-ipv6"
        | "server-bridge" | "pull" | "push" | "client-config-dir" | "client-connect"
        // Identity and credential source ownership.
        | "ca" | "capath" | "cert" | "extra-certs" | "key" | "pkcs12" | "secret"
        | "tls-server" | "tls-client" | "remote-cert-tls" | "remote-cert-ku"
        | "remote-cert-eku" | "ns-cert-type" | "verify-x509-name" | "peer-fingerprint"
        | "tls-auth" | "tls-crypt" | "tls-crypt-v2" | "dh" | "auth-user-pass"
        | "cryptoapicert" | "pkcs11-id" | "pkcs11-providers" | "management-external-key"
        // The agent starts and tracks the native process; four hooks own lifecycle commands.
        | "daemon" | "writepid" | "status"
        | "up" | "down" | "up-delay" | "up-restart" | "down-pre" | "route-up"
        | "route-pre-down" | "ipchange" | "iproute" | "tls-verify" | "tls-crypt-v2-verify"
        | "auth-user-pass-verify" | "client-disconnect" | "learn-address" | "plugin"
        // Includes and connection blocks would escape the same ownership boundary.
        | "config" | "connection"
    )
}

pub fn validate_openvpn_config_override(text: &str) -> Result<(), NetworkPlanError> {
    for directive in text.lines().filter_map(openvpn_directive) {
        if openvpn_directive_is_owned(&directive) {
            return Err(NetworkPlanError::OpenvpnDirectiveOwned(directive));
        }
    }
    Ok(())
}

/// Remove generated values for the user's directive names, then append user
/// lines verbatim and in order. Native repeatable options retain their meaning.
pub fn merge_openvpn_config_override(
    generated: &str,
    overrides: Option<&str>,
) -> Result<String, NetworkPlanError> {
    let Some(overrides) = overrides.filter(|value| !value.trim().is_empty()) else {
        return Ok(generated.to_string());
    };
    validate_openvpn_config_override(overrides)?;
    let overridden = overrides
        .lines()
        .filter_map(openvpn_directive)
        .collect::<HashSet<_>>();
    let mut result = generated
        .lines()
        .filter(|line| {
            openvpn_directive(line).is_none_or(|directive| !overridden.contains(&directive))
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !result.is_empty() {
        result.push('\n');
    }
    result.push_str(overrides);
    if !result.ends_with('\n') {
        result.push('\n');
    }
    Ok(result)
}

pub struct OpenvpnConfigPaths<'a> {
    pub key: &'a Path,
    pub certificate: &'a Path,
    pub peer_ca: &'a Path,
    pub pid: &'a Path,
    pub status: &'a Path,
}

/// `supports_data_ciphers` is the existing OpenVPN >= 2.5 syntax distinction.
pub fn render_openvpn_config(
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    paths: &OpenvpnConfigPaths<'_>,
    supports_data_ciphers: bool,
) -> Result<String, NetworkPlanError> {
    let options = &plan.runtime_control.openvpn;
    let listener = endpoint.side == options.listener_side;
    let family_address = if listener {
        endpoint
            .local_underlay
            .as_deref()
            .unwrap_or(match endpoint.side {
                TunnelEndpointSide::Left => &plan.right_remote_underlay,
                TunnelEndpointSide::Right => &plan.left_remote_underlay,
            })
    } else {
        &endpoint.remote_underlay
    };
    let family = family_address
        .parse::<IpAddr>()
        .map_err(|_| NetworkPlanError::InvalidUnderlayAddress)?;
    let protocol = match (options.transport, listener, family) {
        (RuntimeTunnelOpenvpnTransport::Udp, _, IpAddr::V4(_)) => "udp4",
        (RuntimeTunnelOpenvpnTransport::Udp, _, IpAddr::V6(_)) => "udp6",
        (RuntimeTunnelOpenvpnTransport::Tcp, true, IpAddr::V4(_)) => "tcp4-server",
        (RuntimeTunnelOpenvpnTransport::Tcp, true, IpAddr::V6(_)) => "tcp6-server",
        (RuntimeTunnelOpenvpnTransport::Tcp, false, IpAddr::V4(_)) => "tcp4-client",
        (RuntimeTunnelOpenvpnTransport::Tcp, false, IpAddr::V6(_)) => "tcp6-client",
    };
    let mut lines = vec![
        "mode p2p".to_string(),
        format!("dev {}", plan.interface_name),
        "dev-type tun".to_string(),
        "topology p2p".to_string(),
        format!("proto {protocol}"),
        format!(
            "tun-mtu {}",
            endpoint
                .local_mtu
                .ok_or(NetworkPlanError::TunnelMtuRequired)?
        ),
        format!("cert {}", openvpn_config_path_value(paths.certificate)?),
        format!("key {}", openvpn_config_path_value(paths.key)?),
        format!("ca {}", openvpn_config_path_value(paths.peer_ca)?),
        if listener { "tls-server" } else { "tls-client" }.to_string(),
        if listener {
            "remote-cert-tls client"
        } else {
            "remote-cert-tls server"
        }
        .to_string(),
        "tls-version-min 1.2".to_string(),
        "cipher AES-256-GCM".to_string(),
        "auth SHA256".to_string(),
        "persist-key".to_string(),
        "persist-tun".to_string(),
        "ping 10".to_string(),
        "ping-restart 60".to_string(),
        "daemon".to_string(),
        format!("writepid {}", openvpn_config_path_value(paths.pid)?),
        format!("status {} 10", openvpn_config_path_value(paths.status)?),
        "verb 3".to_string(),
    ];
    lines.push(
        if supports_data_ciphers {
            "data-ciphers AES-256-GCM:AES-128-GCM"
        } else {
            "ncp-ciphers AES-256-GCM:AES-128-GCM"
        }
        .to_string(),
    );
    if listener {
        lines.push("dh none".to_string());
        lines.push(format!("lport {}", options.port));
        if let Some(local) = endpoint.local_underlay.as_deref() {
            lines.push(format!("local {local}"));
        }
    } else {
        lines.push(format!(
            "remote {} {}",
            endpoint.remote_underlay, options.port
        ));
        if let Some(local) = endpoint.local_underlay.as_deref() {
            lines.push(format!("local {local}"));
            lines.push("lport 0".to_string());
        } else {
            lines.push("nobind".to_string());
        }
    }
    for (local, remote, prefix_len) in tunnel_endpoint_address_pairs(plan, endpoint) {
        if local
            .parse::<IpAddr>()
            .map_err(|_| NetworkPlanError::InvalidCidr)?
            .is_ipv4()
        {
            lines.push(format!("ifconfig {local} {remote}"));
        } else {
            lines.push(format!("ifconfig-ipv6 {local}/{prefix_len} {remote}"));
        }
    }
    merge_openvpn_config_override(
        &format!("{}\n", lines.join("\n")),
        options.config_override(endpoint.side),
    )
}

pub fn openvpn_config_path_value(path: &Path) -> Result<String, NetworkPlanError> {
    let value = path
        .to_str()
        .ok_or(NetworkPlanError::InvalidRuntimeTunnelConfigPath)?;
    if value.chars().any(char::is_control) {
        return Err(NetworkPlanError::InvalidRuntimeTunnelConfigPath);
    }
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

pub fn build_ip_tunnel_argv(
    base: &[String],
    action: &str,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
) -> Result<Vec<String>, NetworkPlanError> {
    let mode = match plan.kind {
        TunnelKind::Gre => "gre",
        TunnelKind::Ipip | TunnelKind::Fou => "ipip",
        TunnelKind::Sit => "sit",
        _ => return Err(NetworkPlanError::UnsupportedRuntimeManagerTunnelKind),
    };
    // UDP encapsulation is a link-type option; `ip tunnel` does not accept it.
    let (object, mode_option) = if plan.kind == TunnelKind::Fou {
        ("link", "type")
    } else {
        ("tunnel", "mode")
    };
    let mut argv = extend_argv(
        base,
        [
            object,
            action,
            &plan.interface_name,
            mode_option,
            mode,
            "remote",
            &endpoint.remote_underlay,
        ],
    );
    if let Some(local) = endpoint.local_underlay.as_deref() {
        argv.extend(["local".to_string(), local.to_string()]);
    }
    argv.extend(["ttl".to_string(), "255".to_string()]);
    if plan.kind == TunnelKind::Fou {
        argv.extend([
            "encap".to_string(),
            "fou".to_string(),
            "encap-sport".to_string(),
            "auto".to_string(),
            "encap-dport".to_string(),
            plan.runtime_control.fou.peer_port.to_string(),
        ]);
    }
    Ok(argv)
}

/// `None` is draft-preview context; executable commands require the saved plan
/// UUID whenever generated peer link-local addresses are included.
pub fn build_wireguard_configure_argv(
    base: &[String],
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    private_key_path: &Path,
    peer_public_key: &str,
    plan_id: Option<uuid::Uuid>,
) -> Result<Vec<String>, NetworkPlanError> {
    let options = &plan.runtime_control.wireguard;
    let private_key = private_key_path
        .to_str()
        .ok_or(NetworkPlanError::InvalidRuntimeTunnelConfigPath)?;
    let mut allowed_ips = Vec::new();
    if plan.ipv4_tunnel.is_some() {
        allowed_ips.push("0.0.0.0/0".to_string());
    } else {
        for cidr in &endpoint.peer_additional_addresses.ipv4 {
            let (address, _) = cidr.split_once('/').ok_or(NetworkPlanError::InvalidCidr)?;
            allowed_ips.push(format!("{address}/32"));
        }
    }
    if plan.ipv6_tunnel.is_some() {
        allowed_ips.push("::/0".to_string());
    } else {
        for cidr in &endpoint.peer_additional_addresses.ipv6 {
            let (address, _) = cidr.split_once('/').ok_or(NetworkPlanError::InvalidCidr)?;
            allowed_ips.push(format!("{address}/128"));
        }
        if plan.manage_link_local
            && plan.runtime_control.manager == RuntimeTunnelManager::AgentBuiltin
            && !endpoint.peer_additional_addresses.ipv6.is_empty()
        {
            let generated_peer = plan_id
                .map_or_else(
                    || "{generated_peer_link_local}/64".to_string(),
                    |plan_id| tunnel_generated_link_local(plan_id, &endpoint.peer_client_id),
                )
                .trim_end_matches("/64")
                .to_string();
            let allowance = format!("{generated_peer}/128");
            if !allowed_ips.contains(&allowance) {
                allowed_ips.push(allowance);
            }
        }
    }
    if allowed_ips.is_empty() {
        return Err(NetworkPlanError::TunnelAddressRequired);
    }
    let mut argv = extend_argv(
        base,
        [
            "set",
            &plan.interface_name,
            "private-key",
            private_key,
            "listen-port",
            &options.listen_port(endpoint.side).to_string(),
            "peer",
            peer_public_key,
        ],
    );
    if options.configures_peer_endpoint(endpoint.side) {
        let address = endpoint
            .remote_underlay
            .parse::<IpAddr>()
            .map_err(|_| NetworkPlanError::InvalidUnderlayAddress)?;
        let port = options.peer_listen_port(endpoint.side);
        let peer = match address {
            IpAddr::V4(address) => format!("{address}:{port}"),
            IpAddr::V6(address) => format!("[{address}]:{port}"),
        };
        argv.extend(["endpoint".to_string(), peer]);
    }
    argv.extend([
        "persistent-keepalive".to_string(),
        options.keepalive_secs(endpoint.side).to_string(),
        "allowed-ips".to_string(),
        allowed_ips.join(","),
    ]);
    Ok(argv)
}

pub fn tunnel_endpoint_address_pairs<'a>(
    plan: &'a TunnelPlan,
    endpoint: &TunnelEndpointConfig,
) -> Vec<(&'a str, &'a str, u8)> {
    [plan.ipv4_tunnel.as_ref(), plan.ipv6_tunnel.as_ref()]
        .into_iter()
        .flatten()
        .map(|pair| match endpoint.side {
            TunnelEndpointSide::Left => (pair.left.as_str(), pair.right.as_str(), pair.prefix_len),
            TunnelEndpointSide::Right => (pair.right.as_str(), pair.left.as_str(), pair.prefix_len),
        })
        .collect()
}

/// `None` renders an explicit draft placeholder for a generated address. Runtime
/// callers bind the authoritative UUID before invoking address configuration.
pub fn build_tunnel_address_argv(
    base: &[String],
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    plan_id: Option<uuid::Uuid>,
) -> Vec<Vec<String>> {
    let mut commands = tunnel_endpoint_address_pairs(plan, endpoint)
        .into_iter()
        .map(|(local, remote, prefix)| {
            // With a peer, iproute2 takes the network prefix from the peer
            // address; a bare peer would force a /32 or /128.
            extend_argv(
                base,
                [
                    "addr",
                    "replace",
                    local,
                    "peer",
                    &format!("{remote}/{prefix}"),
                    "dev",
                    &plan.interface_name,
                ],
            )
        })
        .collect::<Vec<_>>();
    commands.extend(
        endpoint
            .additional_addresses
            .ipv4
            .iter()
            .chain(&endpoint.additional_addresses.ipv6)
            .map(|cidr| extend_argv(base, ["addr", "replace", cidr, "dev", &plan.interface_name])),
    );
    if tunnel_endpoint_manages_link_local(plan, endpoint) {
        let generated = plan_id.map_or_else(
            || "{generated_link_local}/64".to_string(),
            |plan_id| tunnel_generated_link_local(plan_id, &endpoint.local_client_id),
        );
        if !tunnel_endpoint_explicit_address_cidrs(plan, endpoint)
            .iter()
            .any(|cidr| cidr.split('/').next() == generated.split('/').next())
        {
            commands.push(extend_argv(
                base,
                ["addr", "replace", &generated, "dev", &plan.interface_name],
            ));
        }
    }
    commands
}

/// Explicit operator intent only; generated link-local addresses are derived
/// separately so disabling their management never removes manual addresses.
pub fn tunnel_endpoint_explicit_address_cidrs(
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
) -> Vec<String> {
    tunnel_endpoint_address_pairs(plan, endpoint)
        .into_iter()
        .map(|(local, _, prefix)| format!("{local}/{prefix}"))
        .chain(endpoint.additional_addresses.ipv4.iter().cloned())
        .chain(endpoint.additional_addresses.ipv6.iter().cloned())
        .collect()
}

/// IPv6 intent is explicit configuration, never a previously generated address.
pub fn tunnel_endpoint_manages_link_local(
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
) -> bool {
    plan.runtime_control.manager == RuntimeTunnelManager::AgentBuiltin
        && plan.manage_link_local
        && (plan.ipv6_tunnel.is_some() || !endpoint.additional_addresses.ipv6.is_empty())
}

/// One stable, locally assigned /64 address per immutable plan and endpoint.
/// Display names, configuration and evidence generations cannot change it.
pub fn tunnel_generated_link_local(plan_id: uuid::Uuid, client_id: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"vpsman-tunnel-link-local-v1\0");
    hash.update(plan_id.as_bytes());
    hash.update([0]);
    hash.update(client_id.as_bytes());
    let hash = hash.finalize();
    let mut iid = [0; 8];
    iid.copy_from_slice(&hash[..8]);
    iid[0] &= !0x02; // Locally assigned, not a globally unique EUI-64 identifier.
    let mut bytes = [0; 16];
    bytes[..2].copy_from_slice(&[0xfe, 0x80]);
    bytes[8..].copy_from_slice(&iid);
    format!("{}/64", Ipv6Addr::from(bytes))
}

pub fn build_ip_route_argv(
    base: &[String],
    action: &str,
    route: &RuntimeTunnelRoute,
    default_interface_name: &str,
) -> Vec<String> {
    let mut argv = extend_argv(base, ["route", action, &route.destination_cidr]);
    if let Some(via) = &route.via {
        argv.extend(["via".to_string(), via.clone()]);
    }
    argv.extend([
        "dev".to_string(),
        route
            .interface_name
            .as_deref()
            .unwrap_or(default_interface_name)
            .to_string(),
    ]);
    if let Some(metric) = route.metric {
        argv.extend(["metric".to_string(), metric.to_string()]);
    }
    argv
}

fn extend_argv<'a>(base: &[String], parts: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    base.iter()
        .cloned()
        .chain(parts.into_iter().map(str::to_string))
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeTunnelNativeCommand {
    pub label: &'static str,
    pub argv: Vec<String>,
    pub required: bool,
}

pub fn build_tunnel_topology_cleanup_commands(
    base: &[String],
    plan: &TunnelPlan,
) -> Vec<RuntimeTunnelNativeCommand> {
    let mut commands = Vec::new();
    for route in &plan.runtime_topology.stale_routes {
        commands.push(RuntimeTunnelNativeCommand {
            label: "runtime_route_delete",
            argv: build_ip_route_argv(base, "del", route, &plan.interface_name),
            required: false,
        });
    }
    for interface in &plan.runtime_topology.stale_interfaces {
        commands.push(RuntimeTunnelNativeCommand {
            label: "runtime_stale_link_delete",
            argv: extend_argv(base, ["link", "delete", "dev", interface]),
            required: false,
        });
    }
    commands
}

/// Existing traffic-control argv, including explicit clearing of absent limits.
/// The defaults here are unchanged from the agent's native driver.
pub fn build_tunnel_traffic_limit_commands(
    base: &[String],
    interface: &str,
    limit: &RuntimeTunnelTrafficLimit,
) -> Vec<RuntimeTunnelNativeCommand> {
    if limit.is_default() && base.is_empty() {
        return Vec::new();
    }
    let burst = limit.burst_kb.unwrap_or(32).to_string();
    let mut commands = Vec::new();
    if let Some(egress) = limit.egress_kbps {
        commands.push(RuntimeTunnelNativeCommand {
            label: "runtime_traffic_egress_limit",
            argv: extend_argv(
                base,
                [
                    "qdisc",
                    "replace",
                    "dev",
                    interface,
                    "root",
                    "tbf",
                    "rate",
                    &format!("{egress}kbit"),
                    "burst",
                    &format!("{burst}kb"),
                    "latency",
                    "50ms",
                ],
            ),
            required: true,
        });
    } else {
        commands.push(RuntimeTunnelNativeCommand {
            label: "runtime_traffic_egress_clear",
            argv: extend_argv(base, ["qdisc", "del", "dev", interface, "root"]),
            required: true,
        });
    }
    if let Some(ingress) = limit.ingress_kbps {
        commands.push(RuntimeTunnelNativeCommand {
            label: "runtime_traffic_ingress_qdisc",
            argv: extend_argv(base, ["qdisc", "replace", "dev", interface, "ingress"]),
            required: true,
        });
        commands.push(RuntimeTunnelNativeCommand {
            label: "runtime_traffic_ingress_filter",
            argv: extend_argv(
                base,
                [
                    "filter",
                    "replace",
                    "dev",
                    interface,
                    "parent",
                    "ffff:",
                    "protocol",
                    "all",
                    "u32",
                    "match",
                    "u32",
                    "0",
                    "0",
                    "police",
                    "rate",
                    &format!("{ingress}kbit"),
                    "burst",
                    &format!("{burst}kb"),
                    "conform-exceed",
                    "drop",
                ],
            ),
            required: true,
        });
    } else {
        commands.push(RuntimeTunnelNativeCommand {
            label: "runtime_traffic_ingress_clear",
            argv: extend_argv(base, ["qdisc", "del", "dev", interface, "ingress"]),
            required: true,
        });
    }
    commands
}

/// Existing adapter substitution semantics: replacements do not recurse, split
/// arguments, or reinterpret unknown tokens. Additional adapter-specific tokens
/// use exactly the same path as builtin lifecycle hooks.
pub fn render_runtime_tunnel_command(
    command: &RuntimeTunnelCommand,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    additional_placeholders: &[(&str, String)],
) -> Result<Vec<String>, NetworkPlanError> {
    if command
        .argv
        .first()
        .is_none_or(|executable| !executable.starts_with('/'))
    {
        return Err(NetworkPlanError::InvalidRuntimeTunnelCommand);
    }
    let kind = match plan.kind {
        TunnelKind::Gre => "gre",
        TunnelKind::Ipip => "ipip",
        TunnelKind::Sit => "sit",
        TunnelKind::Fou => "fou",
        TunnelKind::Wireguard => "wireguard",
        TunnelKind::Openvpn => "openvpn",
        TunnelKind::TunTap => "tun_tap",
        TunnelKind::Custom => "custom",
    };
    let address = |ipv4, local| {
        let pair = if ipv4 {
            &plan.ipv4_tunnel
        } else {
            &plan.ipv6_tunnel
        };
        pair.as_ref()
            .map(|pair| match (endpoint.side, local) {
                (TunnelEndpointSide::Left, true) | (TunnelEndpointSide::Right, false) => {
                    pair.left.clone()
                }
                _ => pair.right.clone(),
            })
            .unwrap_or_default()
    };
    let mut placeholders = vec![
        ("{interface}", plan.interface_name.clone()),
        ("{plan}", plan.name.clone()),
        ("{kind}", kind.to_string()),
        ("{local_client_id}", endpoint.local_client_id.clone()),
        ("{peer_client_id}", endpoint.peer_client_id.clone()),
        (
            "{local_underlay}",
            endpoint.local_underlay.clone().unwrap_or_default(),
        ),
        ("{remote_underlay}", endpoint.remote_underlay.clone()),
        ("{local_address}", endpoint.local_tunnel_address.clone()),
        ("{remote_address}", endpoint.remote_tunnel_address.clone()),
        ("{prefix_len}", endpoint.tunnel_prefix_len.to_string()),
        ("{local_ipv4}", address(true, true)),
        ("{remote_ipv4}", address(true, false)),
        (
            "{prefix_len_ipv4}",
            plan.ipv4_tunnel
                .as_ref()
                .map(|pair| pair.prefix_len.to_string())
                .unwrap_or_default(),
        ),
        ("{local_ipv6}", address(false, true)),
        ("{remote_ipv6}", address(false, false)),
        (
            "{prefix_len_ipv6}",
            plan.ipv6_tunnel
                .as_ref()
                .map(|pair| pair.prefix_len.to_string())
                .unwrap_or_default(),
        ),
        ("{fou_port}", plan.runtime_control.fou.port.to_string()),
        (
            "{fou_peer_port}",
            plan.runtime_control.fou.peer_port.to_string(),
        ),
        (
            "{fou_ipproto}",
            plan.runtime_control.fou.ipproto.to_string(),
        ),
        (
            "{egress_kbps}",
            plan.runtime_control
                .traffic_limit
                .egress_kbps
                .map(|value| value.to_string())
                .unwrap_or_default(),
        ),
        (
            "{ingress_kbps}",
            plan.runtime_control
                .traffic_limit
                .ingress_kbps
                .map(|value| value.to_string())
                .unwrap_or_default(),
        ),
        (
            "{burst_kb}",
            plan.runtime_control
                .traffic_limit
                .burst_kb
                .map(|value| value.to_string())
                .unwrap_or_default(),
        ),
    ];
    placeholders.extend(additional_placeholders.iter().cloned());
    Ok(command
        .argv
        .iter()
        .map(|argument| {
            let mut rendered = String::with_capacity(argument.len());
            let mut cursor = 0;
            while let Some(relative_start) = argument[cursor..].find('{') {
                let start = cursor + relative_start;
                rendered.push_str(&argument[cursor..start]);
                let Some(relative_end) = argument[start..].find('}') else {
                    rendered.push_str(&argument[start..]);
                    return rendered;
                };
                let end = start + relative_end + 1;
                let token = &argument[start..end];
                if let Some((_, value)) = placeholders.iter().find(|(key, _)| *key == token) {
                    rendered.push_str(value);
                } else {
                    rendered.push_str(token);
                }
                cursor = end;
            }
            rendered.push_str(&argument[cursor..]);
            rendered
        })
        .collect())
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TunnelRuntimePreview {
    pub endpoints: Vec<TunnelRuntimeEndpointPreview>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TunnelRuntimeEndpointPreview {
    pub side: TunnelEndpointSide,
    pub client_id: String,
    pub artifacts: Vec<TunnelRuntimePreviewArtifact>,
    pub commands: Vec<TunnelRuntimePreviewCommand>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TunnelRuntimePreviewArtifact {
    pub label: String,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TunnelRuntimePreviewCommand {
    pub phase: String,
    pub label: String,
    pub argv: Vec<String>,
}

pub fn render_tunnel_runtime_preview(
    plan: &TunnelPlan,
    plan_id: Option<uuid::Uuid>,
) -> Result<TunnelRuntimePreview, NetworkPlanError> {
    if plan.runtime_control.manager != RuntimeTunnelManager::AgentBuiltin {
        return Err(NetworkPlanError::UnsupportedRuntimeManagerTunnelKind);
    }
    if let Some(plan_id) = plan_id {
        super::validate_tunnel_link_local_addresses(plan_id, plan)?;
    }
    let mut endpoints = Vec::new();
    for side in [TunnelEndpointSide::Left, TunnelEndpointSide::Right] {
        let endpoint = render_tunnel_endpoint_config(plan, side)?;
        let mut artifacts = Vec::new();
        let mut commands = Vec::new();
        let mut push = |phase: &str, label: &str, argv| {
            commands.push(TunnelRuntimePreviewCommand {
                phase: phase.to_string(),
                label: label.to_string(),
                argv,
            })
        };
        let ip = vec!["{runtime_ip_argv}".to_string()];
        let endpoint_state_dir =
            Path::new("{agent_state}/network-tunnels/{plan_id}").join(match side {
                TunnelEndpointSide::Left => "left",
                TunnelEndpointSide::Right => "right",
            });
        let hooks = plan.runtime_control.hooks.for_side(side);
        if let Some(hook) = &hooks.pre_start {
            push(
                "pre_start",
                "Pre-start hook",
                render_runtime_tunnel_command(hook, plan, &endpoint, &[])?,
            );
        }
        for command in build_tunnel_topology_cleanup_commands(&ip, plan) {
            push("cleanup", command.label, command.argv);
        }
        match plan.kind {
            TunnelKind::Gre | TunnelKind::Ipip | TunnelKind::Sit | TunnelKind::Fou => {
                if plan.kind == TunnelKind::Fou {
                    push(
                        "start",
                        "FOU listener",
                        extend_argv(
                            &ip,
                            [
                                "fou",
                                "add",
                                "port",
                                &plan.runtime_control.fou.port.to_string(),
                                "ipproto",
                                &plan.runtime_control.fou.ipproto.to_string(),
                            ],
                        ),
                    );
                }
                push(
                    "start",
                    "Create interface when absent",
                    build_ip_tunnel_argv(&ip, "add", plan, &endpoint)?,
                );
            }
            TunnelKind::Wireguard => {
                push(
                    "start",
                    "Create interface when absent",
                    extend_argv(
                        &ip,
                        [
                            "link",
                            "add",
                            "dev",
                            &plan.interface_name,
                            "type",
                            "wireguard",
                        ],
                    ),
                );
                push(
                    "configure",
                    "Configure WireGuard",
                    build_wireguard_configure_argv(
                        &["{runtime_wg_argv}".to_string()],
                        plan,
                        &endpoint,
                        &endpoint_state_dir.join("wireguard.key"),
                        "{peer_public_key}",
                        plan_id,
                    )?,
                );
            }
            TunnelKind::Openvpn => {
                let key = endpoint_state_dir.join("openvpn.key");
                let certificate = endpoint_state_dir.join("openvpn.crt");
                let peer_ca = endpoint_state_dir.join("openvpn-peer-ca.crt");
                let pid = endpoint_state_dir.join("openvpn.pid");
                let status = endpoint_state_dir.join("openvpn.status");
                let paths = OpenvpnConfigPaths {
                    key: &key,
                    certificate: &certificate,
                    peer_ca: &peer_ca,
                    pid: &pid,
                    status: &status,
                };
                for (label, supported) in [
                    ("OpenVPN 2.4 configuration", false),
                    ("OpenVPN 2.5+ configuration", true),
                ] {
                    artifacts.push(TunnelRuntimePreviewArtifact {
                        label: label.to_string(),
                        content: render_openvpn_config(plan, &endpoint, &paths, supported)?,
                    });
                }
                push(
                    "start",
                    "Start OpenVPN when absent or configuration changed",
                    vec![
                        "{runtime_openvpn_argv}".to_string(),
                        "--config".to_string(),
                        endpoint_state_dir
                            .join("openvpn.conf")
                            .to_string_lossy()
                            .into_owned(),
                    ],
                );
            }
            _ => return Err(NetworkPlanError::UnsupportedRuntimeManagerTunnelKind),
        }
        push(
            "configure",
            "Set interface MTU",
            extend_argv(
                &ip,
                [
                    "link",
                    "set",
                    "dev",
                    &plan.interface_name,
                    "mtu",
                    &endpoint
                        .local_mtu
                        .ok_or(NetworkPlanError::TunnelMtuRequired)?
                        .to_string(),
                ],
            ),
        );
        if tunnel_endpoint_manages_link_local(plan, &endpoint) {
            artifacts.push(TunnelRuntimePreviewArtifact {
                label: "Managed IPv6 link-local".to_string(),
                content: format!("The agent disables native link-local generation and maintains one stable generated /64 address for this plan UUID and endpoint identity, plus all explicitly configured link-local addresses. Other link-local addresses on this agent-owned interface are removed.{}", if plan_id.is_none() { " The generated address is assigned when this draft is saved; placeholders below are not executable addresses." } else { "" }),
            });
            push(
                "configure",
                "Set managed link-local generation mode",
                extend_argv(
                    &ip,
                    [
                        "link",
                        "set",
                        "dev",
                        &plan.interface_name,
                        "addrgenmode",
                        "none",
                    ],
                ),
            );
        }
        for argv in build_tunnel_address_argv(&ip, plan, &endpoint, plan_id) {
            push("configure", "Set tunnel address", argv);
        }
        push(
            "configure",
            "Bring interface up",
            extend_argv(&ip, ["link", "set", "dev", &plan.interface_name, "up"]),
        );
        for route in &plan.runtime_topology.routes {
            push(
                "configure",
                "Apply declared route",
                build_ip_route_argv(&ip, "replace", route, &plan.interface_name),
            );
        }
        for command in build_tunnel_traffic_limit_commands(
            &["{runtime_tc_argv}".to_string()],
            &plan.interface_name,
            &plan.runtime_control.traffic_limit,
        ) {
            push("configure", command.label, command.argv);
        }
        if let Some(hook) = &hooks.post_start {
            push(
                "post_start",
                "Post-start hook",
                render_runtime_tunnel_command(hook, plan, &endpoint, &[])?,
            );
        }
        if let Some(hook) = &hooks.pre_shutdown {
            push(
                "pre_shutdown",
                "Pre-shutdown hook",
                render_runtime_tunnel_command(hook, plan, &endpoint, &[])?,
            );
        }
        if plan.kind == TunnelKind::Openvpn {
            artifacts.push(TunnelRuntimePreviewArtifact { label: "Native shutdown".to_string(), content: "Signal and wait for the existing agent-owned OpenVPN process; PID is resolved on the agent.".to_string() });
        }
        for route in &plan.runtime_topology.routes {
            push(
                "shutdown",
                "Remove declared route",
                build_ip_route_argv(&ip, "del", route, &plan.interface_name),
            );
        }
        push(
            "shutdown",
            "Remove interface when present",
            extend_argv(&ip, ["link", "delete", "dev", &plan.interface_name]),
        );
        if plan.kind == TunnelKind::Fou {
            push(
                "shutdown",
                "Remove FOU listener",
                extend_argv(
                    &ip,
                    [
                        "fou",
                        "del",
                        "port",
                        &plan.runtime_control.fou.port.to_string(),
                    ],
                ),
            );
        }
        if let Some(hook) = &hooks.post_shutdown {
            push(
                "post_shutdown",
                "Post-shutdown hook",
                render_runtime_tunnel_command(hook, plan, &endpoint, &[])?,
            );
        }
        endpoints.push(TunnelRuntimeEndpointPreview {
            side,
            client_id: endpoint.local_client_id,
            artifacts,
            commands,
        });
    }
    Ok(TunnelRuntimePreview { endpoints })
}
