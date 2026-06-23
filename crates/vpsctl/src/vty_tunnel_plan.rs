use std::net::IpAddr;

use anyhow::{Context, Result};
use vpsman_common::{
    BandwidthTier, OspfCostPolicy, RuntimeTunnelManager, TunnelAddressFamily, TunnelAddressPair,
    TunnelKind, TunnelPlanInput,
};

use crate::network_runtime_args::{
    build_runtime_control, build_runtime_topology, parse_runtime_manager, split_argv_spec,
    RuntimeControlArgs, RuntimeTopologyArgs,
};

#[derive(Debug, PartialEq)]
pub(crate) struct VtyTunnelPlanRequest {
    pub(crate) input: TunnelPlanInput,
    pub(crate) save: bool,
    pub(crate) enabled: bool,
    pub(crate) confirmed: bool,
}

pub(crate) fn parse_vty_tunnel_plan(tokens: &[&str]) -> Result<VtyTunnelPlanRequest> {
    let mut name = None::<String>;
    let mut interface_name = None::<String>;
    let mut kind = None::<TunnelKind>;
    let mut left_client_id = None::<String>;
    let mut right_client_id = None::<String>;
    let mut left_underlay = None::<String>;
    let mut right_underlay = None::<String>;
    let mut address_pool_cidr = None::<String>;
    let mut reserved_addresses = Vec::<String>::new();
    let mut left_tunnel_ipv4_cidr = None::<String>;
    let mut right_tunnel_ipv4_cidr = None::<String>;
    let mut ipv6_address_pool_cidr = None::<String>;
    let mut left_tunnel_ipv6_cidr = None::<String>;
    let mut right_tunnel_ipv6_cidr = None::<String>;
    let mut latency_primary_family = TunnelAddressFamily::Ipv4;
    let mut bandwidth = None::<BandwidthTier>;
    let mut latency_ms = None::<f64>;
    let mut packet_loss_ratio = 0.0_f64;
    let mut preference = 1.0_f64;
    let mut runtime_manager = RuntimeTunnelManager::AgentIproute2Managed;
    let mut runtime_startup_argv = Vec::<String>::new();
    let mut runtime_stop_argv = Vec::<String>::new();
    let mut runtime_cleanup_argv = Vec::<String>::new();
    let mut runtime_restart_argv = Vec::<String>::new();
    let mut runtime_status_argv = Vec::<String>::new();
    let mut runtime_traffic_limit_argv = Vec::<String>::new();
    let mut traffic_ingress_kbps = None::<u32>;
    let mut traffic_egress_kbps = None::<u32>;
    let mut traffic_burst_kb = None::<u32>;
    let mut fou_port = None::<u16>;
    let mut fou_peer_port = None::<u16>;
    let mut fou_ipproto = None::<u8>;
    let mut topology_desired_interfaces = Vec::<String>::new();
    let mut topology_stale_interfaces = Vec::<String>::new();
    let mut topology_routes = Vec::<String>::new();
    let mut topology_stale_routes = Vec::<String>::new();
    let mut save = false;
    let mut enabled = false;
    let mut confirmed = false;

    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--save" => {
                save = true;
                index += 1;
            }
            "--enabled" => {
                enabled = true;
                index += 1;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            "--name" => {
                name = Some(next_value(tokens, index, "--name")?.to_string());
                index += 2;
            }
            value if value.starts_with("--name=") => {
                name = Some(flag_value(value, "--name=").to_string());
                index += 1;
            }
            "--interface-name" | "--interface" => {
                interface_name = Some(next_value(tokens, index, tokens[index])?.to_string());
                index += 2;
            }
            value if value.starts_with("--interface-name=") => {
                interface_name = Some(flag_value(value, "--interface-name=").to_string());
                index += 1;
            }
            value if value.starts_with("--interface=") => {
                interface_name = Some(flag_value(value, "--interface=").to_string());
                index += 1;
            }
            "--kind" => {
                kind = Some(parse_tunnel_kind(next_value(tokens, index, "--kind")?)?);
                index += 2;
            }
            value if value.starts_with("--kind=") => {
                kind = Some(parse_tunnel_kind(flag_value(value, "--kind="))?);
                index += 1;
            }
            "--left-client-id" | "--left-client" => {
                left_client_id = Some(next_value(tokens, index, tokens[index])?.to_string());
                index += 2;
            }
            value if value.starts_with("--left-client-id=") => {
                left_client_id = Some(flag_value(value, "--left-client-id=").to_string());
                index += 1;
            }
            value if value.starts_with("--left-client=") => {
                left_client_id = Some(flag_value(value, "--left-client=").to_string());
                index += 1;
            }
            "--right-client-id" | "--right-client" => {
                right_client_id = Some(next_value(tokens, index, tokens[index])?.to_string());
                index += 2;
            }
            value if value.starts_with("--right-client-id=") => {
                right_client_id = Some(flag_value(value, "--right-client-id=").to_string());
                index += 1;
            }
            value if value.starts_with("--right-client=") => {
                right_client_id = Some(flag_value(value, "--right-client=").to_string());
                index += 1;
            }
            "--left-underlay" => {
                left_underlay = Some(next_value(tokens, index, "--left-underlay")?.to_string());
                index += 2;
            }
            value if value.starts_with("--left-underlay=") => {
                left_underlay = Some(flag_value(value, "--left-underlay=").to_string());
                index += 1;
            }
            "--right-underlay" => {
                right_underlay = Some(next_value(tokens, index, "--right-underlay")?.to_string());
                index += 2;
            }
            value if value.starts_with("--right-underlay=") => {
                right_underlay = Some(flag_value(value, "--right-underlay=").to_string());
                index += 1;
            }
            "--address-pool-cidr" | "--pool-cidr" => {
                address_pool_cidr = Some(next_value(tokens, index, tokens[index])?.to_string());
                index += 2;
            }
            value if value.starts_with("--address-pool-cidr=") => {
                address_pool_cidr = Some(flag_value(value, "--address-pool-cidr=").to_string());
                index += 1;
            }
            value if value.starts_with("--pool-cidr=") => {
                address_pool_cidr = Some(flag_value(value, "--pool-cidr=").to_string());
                index += 1;
            }
            "--left-tunnel-ipv4-cidr" => {
                left_tunnel_ipv4_cidr =
                    Some(next_value(tokens, index, "--left-tunnel-ipv4-cidr")?.to_string());
                index += 2;
            }
            value if value.starts_with("--left-tunnel-ipv4-cidr=") => {
                left_tunnel_ipv4_cidr =
                    Some(flag_value(value, "--left-tunnel-ipv4-cidr=").to_string());
                index += 1;
            }
            "--right-tunnel-ipv4-cidr" => {
                right_tunnel_ipv4_cidr =
                    Some(next_value(tokens, index, "--right-tunnel-ipv4-cidr")?.to_string());
                index += 2;
            }
            value if value.starts_with("--right-tunnel-ipv4-cidr=") => {
                right_tunnel_ipv4_cidr =
                    Some(flag_value(value, "--right-tunnel-ipv4-cidr=").to_string());
                index += 1;
            }
            "--ipv6-address-pool-cidr" | "--ipv6-pool-cidr" => {
                ipv6_address_pool_cidr =
                    Some(next_value(tokens, index, tokens[index])?.to_string());
                index += 2;
            }
            value if value.starts_with("--ipv6-address-pool-cidr=") => {
                ipv6_address_pool_cidr =
                    Some(flag_value(value, "--ipv6-address-pool-cidr=").to_string());
                index += 1;
            }
            value if value.starts_with("--ipv6-pool-cidr=") => {
                ipv6_address_pool_cidr = Some(flag_value(value, "--ipv6-pool-cidr=").to_string());
                index += 1;
            }
            "--left-tunnel-ipv6-cidr" => {
                left_tunnel_ipv6_cidr =
                    Some(next_value(tokens, index, "--left-tunnel-ipv6-cidr")?.to_string());
                index += 2;
            }
            value if value.starts_with("--left-tunnel-ipv6-cidr=") => {
                left_tunnel_ipv6_cidr =
                    Some(flag_value(value, "--left-tunnel-ipv6-cidr=").to_string());
                index += 1;
            }
            "--right-tunnel-ipv6-cidr" => {
                right_tunnel_ipv6_cidr =
                    Some(next_value(tokens, index, "--right-tunnel-ipv6-cidr")?.to_string());
                index += 2;
            }
            value if value.starts_with("--right-tunnel-ipv6-cidr=") => {
                right_tunnel_ipv6_cidr =
                    Some(flag_value(value, "--right-tunnel-ipv6-cidr=").to_string());
                index += 1;
            }
            "--latency-primary-family" => {
                latency_primary_family = parse_tunnel_address_family(next_value(
                    tokens,
                    index,
                    "--latency-primary-family",
                )?)?;
                index += 2;
            }
            value if value.starts_with("--latency-primary-family=") => {
                latency_primary_family =
                    parse_tunnel_address_family(flag_value(value, "--latency-primary-family="))?;
                index += 1;
            }
            "--reserved-address" | "--reserved" => {
                reserved_addresses.extend(split_csv_values(next_value(
                    tokens,
                    index,
                    tokens[index],
                )?));
                index += 2;
            }
            value if value.starts_with("--reserved-address=") => {
                reserved_addresses
                    .extend(split_csv_values(flag_value(value, "--reserved-address=")));
                index += 1;
            }
            value if value.starts_with("--reserved=") => {
                reserved_addresses.extend(split_csv_values(flag_value(value, "--reserved=")));
                index += 1;
            }
            "--bandwidth" => {
                bandwidth = Some(parse_bandwidth(next_value(tokens, index, "--bandwidth")?)?);
                index += 2;
            }
            value if value.starts_with("--bandwidth=") => {
                bandwidth = Some(parse_bandwidth(flag_value(value, "--bandwidth="))?);
                index += 1;
            }
            "--latency-ms" => {
                latency_ms = Some(parse_f64(
                    next_value(tokens, index, "--latency-ms")?,
                    "--latency-ms",
                )?);
                index += 2;
            }
            value if value.starts_with("--latency-ms=") => {
                latency_ms = Some(parse_f64(
                    flag_value(value, "--latency-ms="),
                    "--latency-ms",
                )?);
                index += 1;
            }
            "--packet-loss-ratio" => {
                packet_loss_ratio = parse_f64(
                    next_value(tokens, index, "--packet-loss-ratio")?,
                    "--packet-loss-ratio",
                )?;
                index += 2;
            }
            value if value.starts_with("--packet-loss-ratio=") => {
                packet_loss_ratio = parse_f64(
                    flag_value(value, "--packet-loss-ratio="),
                    "--packet-loss-ratio",
                )?;
                index += 1;
            }
            "--preference" => {
                preference = parse_f64(next_value(tokens, index, "--preference")?, "--preference")?;
                index += 2;
            }
            value if value.starts_with("--preference=") => {
                preference = parse_f64(flag_value(value, "--preference="), "--preference")?;
                index += 1;
            }
            "--runtime-manager" => {
                runtime_manager =
                    parse_runtime_manager(next_value(tokens, index, "--runtime-manager")?)?;
                index += 2;
            }
            value if value.starts_with("--runtime-manager=") => {
                runtime_manager = parse_runtime_manager(flag_value(value, "--runtime-manager="))?;
                index += 1;
            }
            "--runtime-startup-argv" => {
                runtime_startup_argv =
                    split_argv_spec(next_value(tokens, index, "--runtime-startup-argv")?);
                index += 2;
            }
            value if value.starts_with("--runtime-startup-argv=") => {
                runtime_startup_argv =
                    split_argv_spec(flag_value(value, "--runtime-startup-argv="));
                index += 1;
            }
            "--runtime-stop-argv" => {
                runtime_stop_argv =
                    split_argv_spec(next_value(tokens, index, "--runtime-stop-argv")?);
                index += 2;
            }
            value if value.starts_with("--runtime-stop-argv=") => {
                runtime_stop_argv = split_argv_spec(flag_value(value, "--runtime-stop-argv="));
                index += 1;
            }
            "--runtime-cleanup-argv" => {
                runtime_cleanup_argv =
                    split_argv_spec(next_value(tokens, index, "--runtime-cleanup-argv")?);
                index += 2;
            }
            value if value.starts_with("--runtime-cleanup-argv=") => {
                runtime_cleanup_argv =
                    split_argv_spec(flag_value(value, "--runtime-cleanup-argv="));
                index += 1;
            }
            "--runtime-restart-argv" => {
                runtime_restart_argv =
                    split_argv_spec(next_value(tokens, index, "--runtime-restart-argv")?);
                index += 2;
            }
            value if value.starts_with("--runtime-restart-argv=") => {
                runtime_restart_argv =
                    split_argv_spec(flag_value(value, "--runtime-restart-argv="));
                index += 1;
            }
            "--runtime-status-argv" => {
                runtime_status_argv =
                    split_argv_spec(next_value(tokens, index, "--runtime-status-argv")?);
                index += 2;
            }
            value if value.starts_with("--runtime-status-argv=") => {
                runtime_status_argv = split_argv_spec(flag_value(value, "--runtime-status-argv="));
                index += 1;
            }
            "--runtime-traffic-limit-argv" => {
                runtime_traffic_limit_argv =
                    split_argv_spec(next_value(tokens, index, "--runtime-traffic-limit-argv")?);
                index += 2;
            }
            value if value.starts_with("--runtime-traffic-limit-argv=") => {
                runtime_traffic_limit_argv =
                    split_argv_spec(flag_value(value, "--runtime-traffic-limit-argv="));
                index += 1;
            }
            "--traffic-ingress-kbps" => {
                traffic_ingress_kbps = Some(parse_u32(
                    next_value(tokens, index, "--traffic-ingress-kbps")?,
                    "--traffic-ingress-kbps",
                )?);
                index += 2;
            }
            value if value.starts_with("--traffic-ingress-kbps=") => {
                traffic_ingress_kbps = Some(parse_u32(
                    flag_value(value, "--traffic-ingress-kbps="),
                    "--traffic-ingress-kbps",
                )?);
                index += 1;
            }
            "--traffic-egress-kbps" => {
                traffic_egress_kbps = Some(parse_u32(
                    next_value(tokens, index, "--traffic-egress-kbps")?,
                    "--traffic-egress-kbps",
                )?);
                index += 2;
            }
            value if value.starts_with("--traffic-egress-kbps=") => {
                traffic_egress_kbps = Some(parse_u32(
                    flag_value(value, "--traffic-egress-kbps="),
                    "--traffic-egress-kbps",
                )?);
                index += 1;
            }
            "--traffic-burst-kb" => {
                traffic_burst_kb = Some(parse_u32(
                    next_value(tokens, index, "--traffic-burst-kb")?,
                    "--traffic-burst-kb",
                )?);
                index += 2;
            }
            value if value.starts_with("--traffic-burst-kb=") => {
                traffic_burst_kb = Some(parse_u32(
                    flag_value(value, "--traffic-burst-kb="),
                    "--traffic-burst-kb",
                )?);
                index += 1;
            }
            "--fou-port" => {
                fou_port = Some(parse_u16(
                    next_value(tokens, index, "--fou-port")?,
                    "--fou-port",
                )?);
                index += 2;
            }
            value if value.starts_with("--fou-port=") => {
                fou_port = Some(parse_u16(flag_value(value, "--fou-port="), "--fou-port")?);
                index += 1;
            }
            "--fou-peer-port" => {
                fou_peer_port = Some(parse_u16(
                    next_value(tokens, index, "--fou-peer-port")?,
                    "--fou-peer-port",
                )?);
                index += 2;
            }
            value if value.starts_with("--fou-peer-port=") => {
                fou_peer_port = Some(parse_u16(
                    flag_value(value, "--fou-peer-port="),
                    "--fou-peer-port",
                )?);
                index += 1;
            }
            "--fou-ipproto" => {
                fou_ipproto = Some(parse_u8(
                    next_value(tokens, index, "--fou-ipproto")?,
                    "--fou-ipproto",
                )?);
                index += 2;
            }
            value if value.starts_with("--fou-ipproto=") => {
                fou_ipproto = Some(parse_u8(
                    flag_value(value, "--fou-ipproto="),
                    "--fou-ipproto",
                )?);
                index += 1;
            }
            "--topology-desired-interface" | "--topology-desired" => {
                topology_desired_interfaces
                    .push(next_value(tokens, index, tokens[index])?.to_string());
                index += 2;
            }
            value if value.starts_with("--topology-desired-interface=") => {
                topology_desired_interfaces
                    .push(flag_value(value, "--topology-desired-interface=").to_string());
                index += 1;
            }
            value if value.starts_with("--topology-desired=") => {
                topology_desired_interfaces
                    .push(flag_value(value, "--topology-desired=").to_string());
                index += 1;
            }
            "--topology-stale-interface" | "--topology-stale" => {
                topology_stale_interfaces
                    .push(next_value(tokens, index, tokens[index])?.to_string());
                index += 2;
            }
            value if value.starts_with("--topology-stale-interface=") => {
                topology_stale_interfaces
                    .push(flag_value(value, "--topology-stale-interface=").to_string());
                index += 1;
            }
            value if value.starts_with("--topology-stale=") => {
                topology_stale_interfaces.push(flag_value(value, "--topology-stale=").to_string());
                index += 1;
            }
            "--topology-route" => {
                topology_routes.push(next_value(tokens, index, "--topology-route")?.to_string());
                index += 2;
            }
            value if value.starts_with("--topology-route=") => {
                topology_routes.push(flag_value(value, "--topology-route=").to_string());
                index += 1;
            }
            "--topology-stale-route" => {
                topology_stale_routes
                    .push(next_value(tokens, index, "--topology-stale-route")?.to_string());
                index += 2;
            }
            value if value.starts_with("--topology-stale-route=") => {
                topology_stale_routes
                    .push(flag_value(value, "--topology-stale-route=").to_string());
                index += 1;
            }
            other => anyhow::bail!("unknown tunnel-plan flag {other}"),
        }
    }

    let input = TunnelPlanInput {
        name: required(name, "--name")?,
        interface_name: required(interface_name, "--interface-name")?,
        kind: required(kind, "--kind")?,
        runtime_control: build_runtime_control(RuntimeControlArgs {
            manager: runtime_manager,
            startup_argv: &runtime_startup_argv,
            stop_argv: &runtime_stop_argv,
            cleanup_argv: &runtime_cleanup_argv,
            restart_argv: &runtime_restart_argv,
            status_argv: &runtime_status_argv,
            traffic_limit_argv: &runtime_traffic_limit_argv,
            traffic_ingress_kbps,
            traffic_egress_kbps,
            traffic_burst_kb,
            fou_port,
            fou_peer_port,
            fou_ipproto,
        }),
        runtime_topology: build_runtime_topology(RuntimeTopologyArgs {
            version: None,
            desired_interfaces: &topology_desired_interfaces,
            stale_interfaces: &topology_stale_interfaces,
            routes: &topology_routes,
            stale_routes: &topology_stale_routes,
        })?,
        left_client_id: required(left_client_id, "--left-client-id")?,
        right_client_id: required(right_client_id, "--right-client-id")?,
        left_underlay: required(left_underlay, "--left-underlay")?,
        right_underlay: required(right_underlay, "--right-underlay")?,
        address_pool_cidr: address_pool_cidr.unwrap_or_default(),
        reserved_addresses,
        ipv4_tunnel: build_address_pair_from_cidrs(
            left_tunnel_ipv4_cidr,
            right_tunnel_ipv4_cidr,
            TunnelAddressFamily::Ipv4,
            "IPv4",
        )?,
        ipv6_address_pool_cidr,
        ipv6_tunnel: build_address_pair_from_cidrs(
            left_tunnel_ipv6_cidr,
            right_tunnel_ipv6_cidr,
            TunnelAddressFamily::Ipv6,
            "IPv6",
        )?,
        latency_primary_family,
        bandwidth: required(bandwidth, "--bandwidth")?,
        latency_ms: required(latency_ms, "--latency-ms")?,
        packet_loss_ratio,
        preference,
        ospf_policy: OspfCostPolicy::default(),
    };
    ensure_explicit_tunnel_endpoints(&input.ipv4_tunnel, &input.ipv6_tunnel, "tunnel-plan")?;
    Ok(VtyTunnelPlanRequest {
        input,
        save,
        enabled,
        confirmed,
    })
}

fn next_value<'a>(tokens: &'a [&str], index: usize, flag: &str) -> Result<&'a str> {
    tokens
        .get(index + 1)
        .copied()
        .with_context(|| format!("{flag} requires a value"))
}

fn flag_value<'a>(value: &'a str, prefix: &str) -> &'a str {
    value.trim_start_matches(prefix)
}

fn split_csv_values(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn required<T>(value: Option<T>, flag: &str) -> Result<T> {
    value.with_context(|| format!("missing required {flag}"))
}

fn build_address_pair_from_cidrs(
    left: Option<String>,
    right: Option<String>,
    family: TunnelAddressFamily,
    label: &str,
) -> Result<Option<TunnelAddressPair>> {
    match (left, right) {
        (Some(left), Some(right)) => {
            let (left, left_prefix) = parse_endpoint_cidr(&left, family, label)?;
            let (right, right_prefix) = parse_endpoint_cidr(&right, family, label)?;
            anyhow::ensure!(
                left_prefix == right_prefix,
                "{label} tunnel endpoint CIDRs must use the same prefix length"
            );
            Ok(Some(TunnelAddressPair {
                left,
                right,
                prefix_len: left_prefix,
            }))
        }
        (None, None) => Ok(None),
        _ => anyhow::bail!("{label} tunnel endpoints require both left and right CIDRs"),
    }
}

fn parse_endpoint_cidr(
    value: &str,
    family: TunnelAddressFamily,
    label: &str,
) -> Result<(String, u8)> {
    let (address, prefix) = value
        .split_once('/')
        .with_context(|| format!("{label} tunnel endpoint must be address/prefix CIDR"))?;
    let ip: IpAddr = address
        .parse()
        .with_context(|| format!("{label} tunnel endpoint address {address} is invalid"))?;
    match (family, ip) {
        (TunnelAddressFamily::Ipv4, IpAddr::V4(_)) => {}
        (TunnelAddressFamily::Ipv6, IpAddr::V6(_)) => {}
        (TunnelAddressFamily::Ipv4, IpAddr::V6(_)) => {
            anyhow::bail!("{label} tunnel endpoint must be IPv4")
        }
        (TunnelAddressFamily::Ipv6, IpAddr::V4(_)) => {
            anyhow::bail!("{label} tunnel endpoint must be IPv6")
        }
    }
    let prefix_len = prefix
        .parse::<u8>()
        .with_context(|| format!("{label} tunnel endpoint prefix {prefix} is invalid"))?;
    let max_prefix = match family {
        TunnelAddressFamily::Ipv4 => 32,
        TunnelAddressFamily::Ipv6 => 128,
    };
    anyhow::ensure!(
        prefix_len <= max_prefix,
        "{label} tunnel endpoint prefix must be <= {max_prefix}"
    );
    Ok((address.to_string(), prefix_len))
}

fn ensure_explicit_tunnel_endpoints(
    ipv4_tunnel: &Option<TunnelAddressPair>,
    ipv6_tunnel: &Option<TunnelAddressPair>,
    command: &str,
) -> Result<()> {
    anyhow::ensure!(
        ipv4_tunnel.is_some() || ipv6_tunnel.is_some(),
        "{command} requires explicit IPv4 or IPv6 tunnel endpoint CIDRs; run tunnel-allocate for non-overlapping suggestions first"
    );
    Ok(())
}

fn parse_tunnel_kind(value: &str) -> Result<TunnelKind> {
    match value {
        "gre" => Ok(TunnelKind::Gre),
        "ipip" => Ok(TunnelKind::Ipip),
        "sit" => Ok(TunnelKind::Sit),
        "fou" => Ok(TunnelKind::Fou),
        "openvpn" => Ok(TunnelKind::Openvpn),
        "wireguard" => Ok(TunnelKind::Wireguard),
        "tun_tap" | "tuntap" => Ok(TunnelKind::TunTap),
        "custom" => Ok(TunnelKind::Custom),
        _ => anyhow::bail!(
            "--kind must be one of gre, ipip, sit, fou, openvpn, wireguard, tun_tap, custom"
        ),
    }
}

fn parse_bandwidth(value: &str) -> Result<BandwidthTier> {
    match value {
        "10m" | "m10" => Ok(BandwidthTier::M10),
        "100m" | "m100" => Ok(BandwidthTier::M100),
        "1000m" | "m1000" => Ok(BandwidthTier::M1000),
        _ => anyhow::bail!("--bandwidth must be one of 10m, 100m, 1000m"),
    }
}

fn parse_tunnel_address_family(value: &str) -> Result<TunnelAddressFamily> {
    match value {
        "ipv4" | "IPv4" => Ok(TunnelAddressFamily::Ipv4),
        "ipv6" | "IPv6" => Ok(TunnelAddressFamily::Ipv6),
        _ => anyhow::bail!("--latency-primary-family must be one of ipv4, ipv6"),
    }
}

fn parse_f64(value: &str, flag: &str) -> Result<f64> {
    value
        .parse::<f64>()
        .with_context(|| format!("{flag} must be a number"))
}

fn parse_u32(value: &str, flag: &str) -> Result<u32> {
    value
        .parse::<u32>()
        .with_context(|| format!("{flag} must be an integer"))
}

fn parse_u16(value: &str, flag: &str) -> Result<u16> {
    value
        .parse::<u16>()
        .with_context(|| format!("{flag} must be an integer from 0 to 65535"))
}

fn parse_u8(value: &str, flag: &str) -> Result<u8> {
    value
        .parse::<u8>()
        .with_context(|| format!("{flag} must be an integer from 0 to 255"))
}

#[cfg(test)]
mod tests {
    use super::parse_vty_tunnel_plan;
    use vpsman_common::{BandwidthTier, RuntimeTunnelManager, TunnelAddressFamily, TunnelKind};

    #[test]
    fn parses_vty_tunnel_plan_for_local_render() {
        let request = parse_vty_tunnel_plan(&[
            "--name=lax-hkg",
            "--interface-name",
            "vpnlaxhkg",
            "--kind",
            "gre",
            "--left-client-id",
            "lax",
            "--right-client-id=hkg",
            "--left-underlay",
            "198.51.100.10",
            "--right-underlay=203.0.113.20",
            "--address-pool-cidr",
            "10.255.0.0/30",
            "--left-tunnel-ipv4-cidr=10.255.0.0/31",
            "--right-tunnel-ipv4-cidr=10.255.0.1/31",
            "--reserved-address",
            "10.255.0.2,10.255.0.3",
            "--bandwidth",
            "1000m",
            "--latency-ms",
            "138",
            "--packet-loss-ratio=0.002",
            "--preference",
            "1.2",
        ])
        .unwrap();

        assert!(!request.save);
        assert!(!request.enabled);
        assert_eq!(request.input.name, "lax-hkg");
        assert_eq!(request.input.interface_name, "vpnlaxhkg");
        assert_eq!(request.input.kind, TunnelKind::Gre);
        assert_eq!(request.input.left_client_id, "lax");
        assert_eq!(request.input.right_client_id, "hkg");
        assert_eq!(
            request.input.reserved_addresses,
            vec!["10.255.0.2", "10.255.0.3"]
        );
        assert_eq!(request.input.bandwidth, BandwidthTier::M1000);
        assert_eq!(request.input.latency_ms, 138.0);
        assert_eq!(request.input.packet_loss_ratio, 0.002);
        assert_eq!(request.input.preference, 1.2);
    }

    #[test]
    fn parses_vty_tunnel_plan_save_aliases() {
        let request = parse_vty_tunnel_plan(&[
            "--save",
            "--name",
            "edge",
            "--interface",
            "vpsedge",
            "--kind=fou",
            "--left-client",
            "left",
            "--right-client",
            "right",
            "--left-underlay=198.51.100.10",
            "--right-underlay=203.0.113.20",
            "--pool-cidr=10.255.10.0/29",
            "--left-tunnel-ipv4-cidr=10.255.10.0/31",
            "--right-tunnel-ipv4-cidr=10.255.10.1/31",
            "--reserved=10.255.10.2",
            "--bandwidth=100m",
            "--latency-ms=20",
            "--fou-port=6655",
            "--fou-peer-port=7755",
            "--fou-ipproto=47",
            "--enabled",
            "--confirmed",
        ])
        .unwrap();

        assert!(request.save);
        assert!(request.enabled);
        assert!(request.confirmed);
        assert_eq!(request.input.kind, TunnelKind::Fou);
        assert_eq!(request.input.bandwidth, BandwidthTier::M100);
        assert_eq!(request.input.packet_loss_ratio, 0.0);
        assert_eq!(request.input.preference, 1.0);
        assert_eq!(request.input.runtime_control.fou.port, 6655);
        assert_eq!(request.input.runtime_control.fou.peer_port, 7755);
        assert_eq!(request.input.runtime_control.fou.ipproto, 47);
    }

    #[test]
    fn parses_vty_tunnel_plan_explicit_dual_stack_endpoints() {
        let request = parse_vty_tunnel_plan(&[
            "--name=sea-fra",
            "--interface=vpsseafra",
            "--kind=wireguard",
            "--left-client=sea",
            "--right-client=fra",
            "--left-underlay=198.51.100.10",
            "--right-underlay=203.0.113.20",
            "--left-tunnel-ipv4-cidr=10.255.20.0/31",
            "--right-tunnel-ipv4-cidr=10.255.20.1/31",
            "--left-tunnel-ipv6-cidr=fd7a:115c:a1e0::20/127",
            "--right-tunnel-ipv6-cidr=fd7a:115c:a1e0::21/127",
            "--latency-primary-family=ipv6",
            "--bandwidth=1000m",
            "--latency-ms=87.5",
        ])
        .unwrap();

        assert_eq!(request.input.address_pool_cidr, "");
        assert_eq!(
            request.input.ipv4_tunnel.as_ref().unwrap().left,
            "10.255.20.0"
        );
        assert_eq!(
            request.input.ipv6_tunnel.as_ref().unwrap().right,
            "fd7a:115c:a1e0::21"
        );
        assert_eq!(
            request.input.latency_primary_family,
            TunnelAddressFamily::Ipv6
        );
    }

    #[test]
    fn parses_vty_tunnel_plan_external_adapter_runtime() {
        let request = parse_vty_tunnel_plan(&[
            "--name=external-openvpn",
            "--interface=ovpn42",
            "--kind=openvpn",
            "--left-client=left",
            "--right-client=right",
            "--left-underlay=198.51.100.10",
            "--right-underlay=203.0.113.20",
            "--pool-cidr=10.255.10.0/29",
            "--left-tunnel-ipv4-cidr=10.255.10.0/31",
            "--right-tunnel-ipv4-cidr=10.255.10.1/31",
            "--bandwidth=100m",
            "--latency-ms=20",
            "--runtime-manager=adapter",
            "--runtime-startup-argv=/usr/local/libexec/vpsman-openvpn-adapter,start,{interface}",
            "--runtime-cleanup-argv=/usr/local/libexec/vpsman-openvpn-adapter,cleanup,{interface}",
            "--runtime-status-argv=/usr/local/libexec/vpsman-openvpn-adapter,status,{interface}",
            "--runtime-traffic-limit-argv=/usr/local/libexec/vpsman-openvpn-adapter,shape,{interface}",
            "--traffic-egress-kbps=100000",
            "--traffic-burst-kb=4096",
            "--topology-desired=ovpn42",
            "--topology-route=10.42.0.0/24,dev=ovpn42,metric=42",
        ])
        .unwrap();

        assert_eq!(request.input.kind, TunnelKind::Openvpn);
        assert_eq!(
            request.input.runtime_control.manager,
            RuntimeTunnelManager::ExternalManagedAdapter
        );
        assert_eq!(
            request.input.runtime_control.startup.as_ref().unwrap().argv[1],
            "start"
        );
        assert_eq!(
            request.input.runtime_control.cleanup.as_ref().unwrap().argv[1],
            "cleanup"
        );
        assert_eq!(
            request.input.runtime_control.traffic_limit.egress_kbps,
            Some(100_000)
        );
        assert!(request.input.runtime_topology.version.is_none());
        assert_eq!(request.input.runtime_topology.routes[0].metric, Some(42));
    }

    #[test]
    fn rejects_vty_tunnel_plan_missing_required_or_bad_values() {
        assert!(parse_vty_tunnel_plan(&["--name", "edge"]).is_err());
        assert!(parse_vty_tunnel_plan(&[
            "--name=edge",
            "--interface=vpsedge",
            "--kind=badkind",
            "--left-client=left",
            "--right-client=right",
            "--left-underlay=198.51.100.10",
            "--right-underlay=203.0.113.20",
            "--pool-cidr=10.255.10.0/29",
            "--bandwidth=100m",
            "--latency-ms=20",
        ])
        .is_err());
        assert!(parse_vty_tunnel_plan(&[
            "--name=edge",
            "--interface=vpsedge",
            "--kind=gre",
            "--left-client=left",
            "--right-client=right",
            "--left-underlay=198.51.100.10",
            "--right-underlay=203.0.113.20",
            "--pool-cidr=10.255.10.0/29",
            "--bandwidth=1g",
            "--latency-ms=20",
        ])
        .is_err());
        assert!(parse_vty_tunnel_plan(&[
            "--name=edge",
            "--interface=vpsedge",
            "--kind=gre",
            "--left-client=left",
            "--right-client=right",
            "--left-underlay=198.51.100.10",
            "--right-underlay=203.0.113.20",
            "--left-tunnel-ipv6-cidr=fd7a:115c:a1e0::20/127",
            "--bandwidth=100m",
            "--latency-ms=20",
        ])
        .is_err());
        assert!(parse_vty_tunnel_plan(&[
            "--name=edge",
            "--interface=vpsedge",
            "--kind=gre",
            "--left-client=left",
            "--right-client=right",
            "--left-underlay=198.51.100.10",
            "--right-underlay=203.0.113.20",
            "--pool-cidr=10.255.10.0/29",
            "--bandwidth=100m",
            "--latency-ms=20",
        ])
        .is_err());
    }
}
