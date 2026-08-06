use std::net::IpAddr;

use anyhow::{Context, Result};
use vpsman_common::{
    default_ospf_healthy_windows, default_ospf_min_cost_delta, default_tunnel_mtu, plan_tunnel,
    BandwidthMbps, OspfControlMode, OspfCostPolicy, RuntimeTunnelManager,
    RuntimeTunnelOpenvpnTransport, RuntimeTunnelWireguardEndpointMode, TunnelAddressFamily,
    TunnelAddressPair, TunnelEndpointSide, TunnelKind, TunnelOspfConfig, TunnelPlanInput,
    MAX_TUNNEL_BANDWIDTH_MBPS, MIN_TUNNEL_BANDWIDTH_MBPS,
};

use crate::network_runtime_args::{
    build_runtime_control, build_runtime_topology, parse_runtime_manager, RuntimeControlArgs,
    RuntimeTopologyArgs,
};

#[derive(Debug, PartialEq)]
pub(crate) struct VtyTunnelPlanRequest {
    pub(crate) input: TunnelPlanInput,
    pub(crate) save: bool,
    pub(crate) update_plan_id: Option<uuid::Uuid>,
    pub(crate) expected_revision: Option<i64>,
    pub(crate) enabled: bool,
    pub(crate) confirmed: bool,
}

pub(crate) fn parse_vty_tunnel_plan(tokens: &[&str]) -> Result<VtyTunnelPlanRequest> {
    let mut name = None::<String>;
    let mut interface_name = None::<String>;
    let mut kind = None::<TunnelKind>;
    let mut left_client_id = None::<String>;
    let mut right_client_id = None::<String>;
    let mut left_remote_underlay = None::<String>;
    let mut left_local_underlay = None::<String>;
    let mut right_remote_underlay = None::<String>;
    let mut right_local_underlay = None::<String>;
    let mut address_pool_cidr = None::<String>;
    let mut reserved_addresses = Vec::<String>::new();
    let mut left_tunnel_ipv4_cidr = None::<String>;
    let mut right_tunnel_ipv4_cidr = None::<String>;
    let mut ipv6_address_pool_cidr = None::<String>;
    let mut left_tunnel_ipv6_cidr = None::<String>;
    let mut right_tunnel_ipv6_cidr = None::<String>;
    let mut latency_primary_family = TunnelAddressFamily::Ipv4;
    let mut bandwidth = None::<BandwidthMbps>;
    let mut left_mtu = None::<u16>;
    let mut right_mtu = None::<u16>;
    let mut ospf_enabled = false;
    let mut ospf_mode = OspfControlMode::Reviewed;
    let mut ospf_latency_ms = None::<f64>;
    let mut ospf_packet_loss_ratio = 0.0_f64;
    let mut ospf_preference = 1.0_f64;
    let mut ospf_min_cost_delta = default_ospf_min_cost_delta();
    let mut ospf_healthy_windows = default_ospf_healthy_windows();
    let mut ospf_latency_weight = 1.0_f64;
    let mut ospf_loss_weight = 400.0_f64;
    let mut ospf_bandwidth_weight = 10.0_f64;
    let mut ospf_preference_bias = 1.0_f64;
    let mut ospf_min_cost = 5_u16;
    let mut ospf_max_cost = 65_535_u16;
    let mut left_routing_adapter_definition_id = None::<String>;
    let mut right_routing_adapter_definition_id = None::<String>;
    let mut runtime_manager = RuntimeTunnelManager::AgentBuiltin;
    let mut left_runtime_adapter_definition_id = None::<String>;
    let mut right_runtime_adapter_definition_id = None::<String>;
    let mut traffic_ingress_kbps = None::<u32>;
    let mut traffic_egress_kbps = None::<u32>;
    let mut traffic_burst_kb = None::<u32>;
    let mut fou_port = None::<u16>;
    let mut fou_peer_port = None::<u16>;
    let mut fou_ipproto = None::<u8>;
    let mut wireguard_left_listen_port = None::<u16>;
    let mut wireguard_right_listen_port = None::<u16>;
    let mut wireguard_left_keepalive_secs = None::<u16>;
    let mut wireguard_right_keepalive_secs = None::<u16>;
    let mut wireguard_endpoint_mode = None::<RuntimeTunnelWireguardEndpointMode>;
    let mut openvpn_transport = None::<RuntimeTunnelOpenvpnTransport>;
    let mut openvpn_listener_side = None::<TunnelEndpointSide>;
    let mut openvpn_port = None::<u16>;
    let mut topology_desired_interfaces = Vec::<String>::new();
    let mut topology_stale_interfaces = Vec::<String>::new();
    let mut topology_routes = Vec::<String>::new();
    let mut topology_stale_routes = Vec::<String>::new();
    let mut save = false;
    let mut update_plan_id = None::<uuid::Uuid>;
    let mut expected_revision = None::<i64>;
    let mut enabled = false;
    let mut confirmed = false;

    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--save" => {
                save = true;
                index += 1;
            }
            "--update-plan-id" => {
                update_plan_id = Some(
                    next_value(tokens, index, "--update-plan-id")?
                        .parse()
                        .context("--update-plan-id must be a UUID")?,
                );
                index += 2;
            }
            value if value.starts_with("--update-plan-id=") => {
                update_plan_id = Some(
                    flag_value(value, "--update-plan-id=")
                        .parse()
                        .context("--update-plan-id must be a UUID")?,
                );
                index += 1;
            }
            "--expected-revision" => {
                expected_revision = Some(
                    next_value(tokens, index, "--expected-revision")?
                        .parse()
                        .context("--expected-revision must be a positive integer")?,
                );
                index += 2;
            }
            value if value.starts_with("--expected-revision=") => {
                expected_revision = Some(
                    flag_value(value, "--expected-revision=")
                        .parse()
                        .context("--expected-revision must be a positive integer")?,
                );
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
            "--ospf" => {
                ospf_enabled = true;
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
            "--left-remote-underlay" => {
                left_remote_underlay =
                    Some(next_value(tokens, index, "--left-remote-underlay")?.to_string());
                index += 2;
            }
            value if value.starts_with("--left-remote-underlay=") => {
                left_remote_underlay =
                    Some(flag_value(value, "--left-remote-underlay=").to_string());
                index += 1;
            }
            "--left-local-underlay" => {
                left_local_underlay =
                    Some(next_value(tokens, index, "--left-local-underlay")?.to_string());
                index += 2;
            }
            value if value.starts_with("--left-local-underlay=") => {
                left_local_underlay = Some(flag_value(value, "--left-local-underlay=").to_string());
                index += 1;
            }
            "--right-remote-underlay" => {
                right_remote_underlay =
                    Some(next_value(tokens, index, "--right-remote-underlay")?.to_string());
                index += 2;
            }
            value if value.starts_with("--right-remote-underlay=") => {
                right_remote_underlay =
                    Some(flag_value(value, "--right-remote-underlay=").to_string());
                index += 1;
            }
            "--right-local-underlay" => {
                right_local_underlay =
                    Some(next_value(tokens, index, "--right-local-underlay")?.to_string());
                index += 2;
            }
            value if value.starts_with("--right-local-underlay=") => {
                right_local_underlay =
                    Some(flag_value(value, "--right-local-underlay=").to_string());
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
            "--bandwidth-mbps" => {
                bandwidth = Some(parse_bandwidth_mbps(next_value(
                    tokens,
                    index,
                    "--bandwidth-mbps",
                )?)?);
                index += 2;
            }
            value if value.starts_with("--bandwidth-mbps=") => {
                bandwidth = Some(parse_bandwidth_mbps(flag_value(
                    value,
                    "--bandwidth-mbps=",
                ))?);
                index += 1;
            }
            "--left-mtu" => {
                left_mtu = Some(parse_u16(
                    next_value(tokens, index, "--left-mtu")?,
                    "--left-mtu",
                )?);
                index += 2;
            }
            value if value.starts_with("--left-mtu=") => {
                left_mtu = Some(parse_u16(flag_value(value, "--left-mtu="), "--left-mtu")?);
                index += 1;
            }
            "--right-mtu" => {
                right_mtu = Some(parse_u16(
                    next_value(tokens, index, "--right-mtu")?,
                    "--right-mtu",
                )?);
                index += 2;
            }
            value if value.starts_with("--right-mtu=") => {
                right_mtu = Some(parse_u16(flag_value(value, "--right-mtu="), "--right-mtu")?);
                index += 1;
            }
            "--ospf-mode" => {
                ospf_mode = parse_ospf_control_mode(next_value(tokens, index, "--ospf-mode")?)?;
                index += 2;
            }
            value if value.starts_with("--ospf-mode=") => {
                ospf_mode = parse_ospf_control_mode(flag_value(value, "--ospf-mode="))?;
                index += 1;
            }
            "--ospf-latency-ms" => {
                ospf_latency_ms = Some(parse_f64(
                    next_value(tokens, index, "--ospf-latency-ms")?,
                    "--ospf-latency-ms",
                )?);
                index += 2;
            }
            value if value.starts_with("--ospf-latency-ms=") => {
                ospf_latency_ms = Some(parse_f64(
                    flag_value(value, "--ospf-latency-ms="),
                    "--ospf-latency-ms",
                )?);
                index += 1;
            }
            "--ospf-packet-loss-ratio" => {
                ospf_packet_loss_ratio = parse_f64(
                    next_value(tokens, index, "--ospf-packet-loss-ratio")?,
                    "--ospf-packet-loss-ratio",
                )?;
                index += 2;
            }
            value if value.starts_with("--ospf-packet-loss-ratio=") => {
                ospf_packet_loss_ratio = parse_f64(
                    flag_value(value, "--ospf-packet-loss-ratio="),
                    "--ospf-packet-loss-ratio",
                )?;
                index += 1;
            }
            "--ospf-preference" => {
                ospf_preference = parse_f64(
                    next_value(tokens, index, "--ospf-preference")?,
                    "--ospf-preference",
                )?;
                index += 2;
            }
            value if value.starts_with("--ospf-preference=") => {
                ospf_preference =
                    parse_f64(flag_value(value, "--ospf-preference="), "--ospf-preference")?;
                index += 1;
            }
            "--ospf-min-cost-delta" => {
                ospf_min_cost_delta = parse_u16(
                    next_value(tokens, index, "--ospf-min-cost-delta")?,
                    "--ospf-min-cost-delta",
                )?;
                index += 2;
            }
            value if value.starts_with("--ospf-min-cost-delta=") => {
                ospf_min_cost_delta = parse_u16(
                    flag_value(value, "--ospf-min-cost-delta="),
                    "--ospf-min-cost-delta",
                )?;
                index += 1;
            }
            "--ospf-healthy-windows" => {
                ospf_healthy_windows = parse_u8(
                    next_value(tokens, index, "--ospf-healthy-windows")?,
                    "--ospf-healthy-windows",
                )?;
                index += 2;
            }
            value if value.starts_with("--ospf-healthy-windows=") => {
                ospf_healthy_windows = parse_u8(
                    flag_value(value, "--ospf-healthy-windows="),
                    "--ospf-healthy-windows",
                )?;
                index += 1;
            }
            "--ospf-latency-weight" => {
                ospf_latency_weight = parse_f64(
                    next_value(tokens, index, "--ospf-latency-weight")?,
                    "--ospf-latency-weight",
                )?;
                index += 2;
            }
            value if value.starts_with("--ospf-latency-weight=") => {
                ospf_latency_weight = parse_f64(
                    flag_value(value, "--ospf-latency-weight="),
                    "--ospf-latency-weight",
                )?;
                index += 1;
            }
            "--ospf-loss-weight" => {
                ospf_loss_weight = parse_f64(
                    next_value(tokens, index, "--ospf-loss-weight")?,
                    "--ospf-loss-weight",
                )?;
                index += 2;
            }
            value if value.starts_with("--ospf-loss-weight=") => {
                ospf_loss_weight = parse_f64(
                    flag_value(value, "--ospf-loss-weight="),
                    "--ospf-loss-weight",
                )?;
                index += 1;
            }
            "--ospf-bandwidth-weight" => {
                ospf_bandwidth_weight = parse_f64(
                    next_value(tokens, index, "--ospf-bandwidth-weight")?,
                    "--ospf-bandwidth-weight",
                )?;
                index += 2;
            }
            value if value.starts_with("--ospf-bandwidth-weight=") => {
                ospf_bandwidth_weight = parse_f64(
                    flag_value(value, "--ospf-bandwidth-weight="),
                    "--ospf-bandwidth-weight",
                )?;
                index += 1;
            }
            "--ospf-preference-bias" => {
                ospf_preference_bias = parse_f64(
                    next_value(tokens, index, "--ospf-preference-bias")?,
                    "--ospf-preference-bias",
                )?;
                index += 2;
            }
            value if value.starts_with("--ospf-preference-bias=") => {
                ospf_preference_bias = parse_f64(
                    flag_value(value, "--ospf-preference-bias="),
                    "--ospf-preference-bias",
                )?;
                index += 1;
            }
            "--ospf-min-cost" => {
                ospf_min_cost = parse_u16(
                    next_value(tokens, index, "--ospf-min-cost")?,
                    "--ospf-min-cost",
                )?;
                index += 2;
            }
            value if value.starts_with("--ospf-min-cost=") => {
                ospf_min_cost =
                    parse_u16(flag_value(value, "--ospf-min-cost="), "--ospf-min-cost")?;
                index += 1;
            }
            "--ospf-max-cost" => {
                ospf_max_cost = parse_u16(
                    next_value(tokens, index, "--ospf-max-cost")?,
                    "--ospf-max-cost",
                )?;
                index += 2;
            }
            value if value.starts_with("--ospf-max-cost=") => {
                ospf_max_cost =
                    parse_u16(flag_value(value, "--ospf-max-cost="), "--ospf-max-cost")?;
                index += 1;
            }
            "--left-routing-adapter-definition-id" => {
                left_routing_adapter_definition_id = Some(
                    next_value(tokens, index, "--left-routing-adapter-definition-id")?.to_string(),
                );
                index += 2;
            }
            value if value.starts_with("--left-routing-adapter-definition-id=") => {
                left_routing_adapter_definition_id =
                    Some(flag_value(value, "--left-routing-adapter-definition-id=").to_string());
                index += 1;
            }
            "--right-routing-adapter-definition-id" => {
                right_routing_adapter_definition_id = Some(
                    next_value(tokens, index, "--right-routing-adapter-definition-id")?.to_string(),
                );
                index += 2;
            }
            value if value.starts_with("--right-routing-adapter-definition-id=") => {
                right_routing_adapter_definition_id =
                    Some(flag_value(value, "--right-routing-adapter-definition-id=").to_string());
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
            "--left-runtime-adapter-definition-id" => {
                left_runtime_adapter_definition_id = Some(
                    next_value(tokens, index, "--left-runtime-adapter-definition-id")?.to_string(),
                );
                index += 2;
            }
            value if value.starts_with("--left-runtime-adapter-definition-id=") => {
                left_runtime_adapter_definition_id =
                    Some(flag_value(value, "--left-runtime-adapter-definition-id=").to_string());
                index += 1;
            }
            "--right-runtime-adapter-definition-id" => {
                right_runtime_adapter_definition_id = Some(
                    next_value(tokens, index, "--right-runtime-adapter-definition-id")?.to_string(),
                );
                index += 2;
            }
            value if value.starts_with("--right-runtime-adapter-definition-id=") => {
                right_runtime_adapter_definition_id =
                    Some(flag_value(value, "--right-runtime-adapter-definition-id=").to_string());
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
            "--wireguard-left-listen-port" => {
                wireguard_left_listen_port = Some(parse_u16(
                    next_value(tokens, index, "--wireguard-left-listen-port")?,
                    "--wireguard-left-listen-port",
                )?);
                index += 2;
            }
            value if value.starts_with("--wireguard-left-listen-port=") => {
                wireguard_left_listen_port = Some(parse_u16(
                    flag_value(value, "--wireguard-left-listen-port="),
                    "--wireguard-left-listen-port",
                )?);
                index += 1;
            }
            "--wireguard-right-listen-port" => {
                wireguard_right_listen_port = Some(parse_u16(
                    next_value(tokens, index, "--wireguard-right-listen-port")?,
                    "--wireguard-right-listen-port",
                )?);
                index += 2;
            }
            value if value.starts_with("--wireguard-right-listen-port=") => {
                wireguard_right_listen_port = Some(parse_u16(
                    flag_value(value, "--wireguard-right-listen-port="),
                    "--wireguard-right-listen-port",
                )?);
                index += 1;
            }
            "--wireguard-left-keepalive-secs" => {
                wireguard_left_keepalive_secs = Some(parse_u16(
                    next_value(tokens, index, "--wireguard-left-keepalive-secs")?,
                    "--wireguard-left-keepalive-secs",
                )?);
                index += 2;
            }
            value if value.starts_with("--wireguard-left-keepalive-secs=") => {
                wireguard_left_keepalive_secs = Some(parse_u16(
                    flag_value(value, "--wireguard-left-keepalive-secs="),
                    "--wireguard-left-keepalive-secs",
                )?);
                index += 1;
            }
            "--wireguard-right-keepalive-secs" => {
                wireguard_right_keepalive_secs = Some(parse_u16(
                    next_value(tokens, index, "--wireguard-right-keepalive-secs")?,
                    "--wireguard-right-keepalive-secs",
                )?);
                index += 2;
            }
            value if value.starts_with("--wireguard-right-keepalive-secs=") => {
                wireguard_right_keepalive_secs = Some(parse_u16(
                    flag_value(value, "--wireguard-right-keepalive-secs="),
                    "--wireguard-right-keepalive-secs",
                )?);
                index += 1;
            }
            "--wireguard-endpoint-mode" => {
                wireguard_endpoint_mode = Some(parse_wireguard_endpoint_mode(next_value(
                    tokens,
                    index,
                    "--wireguard-endpoint-mode",
                )?)?);
                index += 2;
            }
            value if value.starts_with("--wireguard-endpoint-mode=") => {
                wireguard_endpoint_mode = Some(parse_wireguard_endpoint_mode(flag_value(
                    value,
                    "--wireguard-endpoint-mode=",
                ))?);
                index += 1;
            }
            "--openvpn-transport" => {
                openvpn_transport = Some(parse_openvpn_transport(next_value(
                    tokens,
                    index,
                    "--openvpn-transport",
                )?)?);
                index += 2;
            }
            value if value.starts_with("--openvpn-transport=") => {
                openvpn_transport = Some(parse_openvpn_transport(flag_value(
                    value,
                    "--openvpn-transport=",
                ))?);
                index += 1;
            }
            "--openvpn-listener-side" => {
                openvpn_listener_side = Some(parse_tunnel_endpoint_side(next_value(
                    tokens,
                    index,
                    "--openvpn-listener-side",
                )?)?);
                index += 2;
            }
            value if value.starts_with("--openvpn-listener-side=") => {
                openvpn_listener_side = Some(parse_tunnel_endpoint_side(flag_value(
                    value,
                    "--openvpn-listener-side=",
                ))?);
                index += 1;
            }
            "--openvpn-port" => {
                openvpn_port = Some(parse_u16(
                    next_value(tokens, index, "--openvpn-port")?,
                    "--openvpn-port",
                )?);
                index += 2;
            }
            value if value.starts_with("--openvpn-port=") => {
                openvpn_port = Some(parse_u16(
                    flag_value(value, "--openvpn-port="),
                    "--openvpn-port",
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

    let ospf = if ospf_enabled {
        Some(TunnelOspfConfig {
            mode: ospf_mode,
            planned_latency_ms: required(ospf_latency_ms, "--ospf-latency-ms")?,
            planned_packet_loss_ratio: ospf_packet_loss_ratio,
            preference: ospf_preference,
            policy: OspfCostPolicy {
                latency_weight: ospf_latency_weight,
                loss_weight: ospf_loss_weight,
                bandwidth_weight: ospf_bandwidth_weight,
                preference_bias: ospf_preference_bias,
                min_cost: ospf_min_cost,
                max_cost: ospf_max_cost,
            },
            min_cost_delta: ospf_min_cost_delta,
            healthy_windows: ospf_healthy_windows,
            left_adapter_definition_id: left_routing_adapter_definition_id,
            right_adapter_definition_id: right_routing_adapter_definition_id,
        })
    } else {
        anyhow::ensure!(
            ospf_latency_ms.is_none()
                && left_routing_adapter_definition_id.is_none()
                && right_routing_adapter_definition_id.is_none(),
            "OSPF options require --ospf"
        );
        None
    };
    let kind = required(kind, "--kind")?;
    let default_mtu = (runtime_manager == RuntimeTunnelManager::AgentBuiltin)
        .then(|| default_tunnel_mtu(kind))
        .flatten();
    let input = TunnelPlanInput {
        name: required(name, "--name")?,
        interface_name: required(interface_name, "--interface-name")?,
        kind,
        runtime_control: build_runtime_control(RuntimeControlArgs {
            manager: runtime_manager,
            left_adapter_definition_id: left_runtime_adapter_definition_id.as_deref(),
            right_adapter_definition_id: right_runtime_adapter_definition_id.as_deref(),
            traffic_ingress_kbps,
            traffic_egress_kbps,
            traffic_burst_kb,
            fou_port,
            fou_peer_port,
            fou_ipproto,
            wireguard_left_listen_port,
            wireguard_right_listen_port,
            wireguard_left_keepalive_secs,
            wireguard_right_keepalive_secs,
            wireguard_endpoint_mode,
            openvpn_transport,
            openvpn_listener_side,
            openvpn_port,
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
        left_remote_underlay: required(left_remote_underlay, "--left-remote-underlay")?,
        left_local_underlay,
        right_remote_underlay: required(right_remote_underlay, "--right-remote-underlay")?,
        right_local_underlay,
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
        bandwidth_mbps: required(bandwidth, "--bandwidth-mbps")?,
        left_mtu: left_mtu.or(default_mtu),
        right_mtu: right_mtu.or(default_mtu),
        ospf,
    };
    ensure_explicit_tunnel_endpoints(&input.ipv4_tunnel, &input.ipv6_tunnel, "tunnel-plan")?;
    plan_tunnel(&input)?;
    anyhow::ensure!(
        update_plan_id.is_some() == expected_revision.is_some(),
        "tunnel-plan update requires both --update-plan-id and --expected-revision"
    );
    anyhow::ensure!(
        expected_revision.is_none_or(|revision| revision > 0),
        "tunnel-plan --expected-revision must be positive"
    );
    anyhow::ensure!(
        update_plan_id.is_none() || save,
        "tunnel-plan --update-plan-id requires --save"
    );
    Ok(VtyTunnelPlanRequest {
        input,
        save,
        update_plan_id,
        expected_revision,
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

fn parse_bandwidth_mbps(value: &str) -> Result<BandwidthMbps> {
    let parsed = parse_u32(value, "--bandwidth-mbps")?;
    if (MIN_TUNNEL_BANDWIDTH_MBPS..=MAX_TUNNEL_BANDWIDTH_MBPS).contains(&parsed) {
        Ok(parsed)
    } else {
        anyhow::bail!("--bandwidth-mbps must be between 10 and 10000")
    }
}

fn parse_tunnel_address_family(value: &str) -> Result<TunnelAddressFamily> {
    match value {
        "ipv4" | "IPv4" => Ok(TunnelAddressFamily::Ipv4),
        "ipv6" | "IPv6" => Ok(TunnelAddressFamily::Ipv6),
        _ => anyhow::bail!("--latency-primary-family must be one of ipv4, ipv6"),
    }
}

fn parse_wireguard_endpoint_mode(value: &str) -> Result<RuntimeTunnelWireguardEndpointMode> {
    match value {
        "left" => Ok(RuntimeTunnelWireguardEndpointMode::Left),
        "right" => Ok(RuntimeTunnelWireguardEndpointMode::Right),
        "both" => Ok(RuntimeTunnelWireguardEndpointMode::Both),
        _ => anyhow::bail!("--wireguard-endpoint-mode must be one of left, right, both"),
    }
}

fn parse_openvpn_transport(value: &str) -> Result<RuntimeTunnelOpenvpnTransport> {
    match value {
        "udp" => Ok(RuntimeTunnelOpenvpnTransport::Udp),
        "tcp" => Ok(RuntimeTunnelOpenvpnTransport::Tcp),
        _ => anyhow::bail!("--openvpn-transport must be one of udp, tcp"),
    }
}

fn parse_tunnel_endpoint_side(value: &str) -> Result<TunnelEndpointSide> {
    match value {
        "left" => Ok(TunnelEndpointSide::Left),
        "right" => Ok(TunnelEndpointSide::Right),
        _ => anyhow::bail!("--openvpn-listener-side must be one of left, right"),
    }
}

fn parse_ospf_control_mode(value: &str) -> Result<OspfControlMode> {
    match value {
        "reviewed" => Ok(OspfControlMode::Reviewed),
        "automatic" => Ok(OspfControlMode::Automatic),
        _ => anyhow::bail!("--ospf-mode must be one of reviewed, automatic"),
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
#[path = "tests_vty_tunnel_plan.rs"]
mod tests;
