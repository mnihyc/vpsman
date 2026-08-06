use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use vpsman_common::{
    default_runtime_fou_ipproto, default_runtime_fou_peer_port, default_runtime_fou_port,
    default_runtime_openvpn_port, default_runtime_wireguard_keepalive_secs,
    default_runtime_wireguard_listen_port, RuntimeTunnelControl, RuntimeTunnelFouOptions,
    RuntimeTunnelManager, RuntimeTunnelOpenvpnOptions, RuntimeTunnelOpenvpnTransport,
    RuntimeTunnelRoute, RuntimeTunnelTopologyIntent, RuntimeTunnelTrafficLimit,
    RuntimeTunnelWireguardEndpointMode, RuntimeTunnelWireguardOptions, TunnelEndpointSide,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
#[value(rename_all = "snake_case")]
pub(crate) enum RuntimeManagerArg {
    #[value(name = "builtin")]
    AgentBuiltin,
    ExternalObserved,
    CustomAdapter,
}

impl From<RuntimeManagerArg> for RuntimeTunnelManager {
    fn from(value: RuntimeManagerArg) -> Self {
        match value {
            RuntimeManagerArg::AgentBuiltin => Self::AgentBuiltin,
            RuntimeManagerArg::ExternalObserved => Self::ExternalObserved,
            RuntimeManagerArg::CustomAdapter => Self::CustomAdapter,
        }
    }
}

pub(crate) struct RuntimeControlArgs<'a> {
    pub(crate) manager: RuntimeTunnelManager,
    pub(crate) left_adapter_definition_id: Option<&'a str>,
    pub(crate) right_adapter_definition_id: Option<&'a str>,
    pub(crate) traffic_ingress_kbps: Option<u32>,
    pub(crate) traffic_egress_kbps: Option<u32>,
    pub(crate) traffic_burst_kb: Option<u32>,
    pub(crate) fou_port: Option<u16>,
    pub(crate) fou_peer_port: Option<u16>,
    pub(crate) fou_ipproto: Option<u8>,
    pub(crate) wireguard_left_listen_port: Option<u16>,
    pub(crate) wireguard_right_listen_port: Option<u16>,
    pub(crate) wireguard_left_keepalive_secs: Option<u16>,
    pub(crate) wireguard_right_keepalive_secs: Option<u16>,
    pub(crate) wireguard_endpoint_mode: Option<RuntimeTunnelWireguardEndpointMode>,
    pub(crate) openvpn_transport: Option<RuntimeTunnelOpenvpnTransport>,
    pub(crate) openvpn_listener_side: Option<TunnelEndpointSide>,
    pub(crate) openvpn_port: Option<u16>,
}

pub(crate) struct RuntimeTopologyArgs<'a> {
    pub(crate) version: Option<&'a str>,
    pub(crate) desired_interfaces: &'a [String],
    pub(crate) stale_interfaces: &'a [String],
    pub(crate) routes: &'a [String],
    pub(crate) stale_routes: &'a [String],
}

pub(crate) fn build_runtime_control(args: RuntimeControlArgs<'_>) -> RuntimeTunnelControl {
    RuntimeTunnelControl {
        manager: args.manager,
        left_adapter_definition_id: args.left_adapter_definition_id.map(str::to_string),
        right_adapter_definition_id: args.right_adapter_definition_id.map(str::to_string),
        traffic_limit: RuntimeTunnelTrafficLimit {
            ingress_kbps: args.traffic_ingress_kbps,
            egress_kbps: args.traffic_egress_kbps,
            burst_kb: args.traffic_burst_kb,
        },
        fou: RuntimeTunnelFouOptions {
            port: args.fou_port.unwrap_or_else(default_runtime_fou_port),
            peer_port: args
                .fou_peer_port
                .unwrap_or_else(default_runtime_fou_peer_port),
            ipproto: args.fou_ipproto.unwrap_or_else(default_runtime_fou_ipproto),
        },
        wireguard: RuntimeTunnelWireguardOptions {
            endpoint_mode: args.wireguard_endpoint_mode.unwrap_or_default(),
            left_listen_port: args
                .wireguard_left_listen_port
                .unwrap_or_else(default_runtime_wireguard_listen_port),
            right_listen_port: args
                .wireguard_right_listen_port
                .unwrap_or_else(default_runtime_wireguard_listen_port),
            left_keepalive_secs: args
                .wireguard_left_keepalive_secs
                .unwrap_or_else(default_runtime_wireguard_keepalive_secs),
            right_keepalive_secs: args
                .wireguard_right_keepalive_secs
                .unwrap_or_else(default_runtime_wireguard_keepalive_secs),
        },
        openvpn: RuntimeTunnelOpenvpnOptions {
            transport: args.openvpn_transport.unwrap_or_default(),
            listener_side: args.openvpn_listener_side.unwrap_or_default(),
            port: args
                .openvpn_port
                .unwrap_or_else(default_runtime_openvpn_port),
        },
    }
}

pub(crate) fn build_runtime_topology(
    args: RuntimeTopologyArgs<'_>,
) -> Result<RuntimeTunnelTopologyIntent> {
    Ok(RuntimeTunnelTopologyIntent {
        version: args.version.map(str::to_string),
        desired_interfaces: args.desired_interfaces.to_vec(),
        stale_interfaces: args.stale_interfaces.to_vec(),
        routes: parse_route_specs(args.routes)?,
        stale_routes: parse_route_specs(args.stale_routes)?,
    })
}

pub(crate) fn parse_runtime_manager(value: &str) -> Result<RuntimeTunnelManager> {
    match value {
        "builtin" => Ok(RuntimeTunnelManager::AgentBuiltin),
        "external_observed" => Ok(RuntimeTunnelManager::ExternalObserved),
        "custom_adapter" => Ok(RuntimeTunnelManager::CustomAdapter),
        _ => bail!("invalid runtime manager {value}"),
    }
}

fn parse_route_specs(values: &[String]) -> Result<Vec<RuntimeTunnelRoute>> {
    values
        .iter()
        .map(|value| parse_route_spec(value))
        .collect::<Result<Vec<_>>>()
}

fn parse_route_spec(value: &str) -> Result<RuntimeTunnelRoute> {
    let mut parts = value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty());
    let destination_cidr = parts
        .next()
        .context("runtime topology route requires destination CIDR")?
        .to_string();
    let mut route = RuntimeTunnelRoute {
        destination_cidr,
        ..RuntimeTunnelRoute::default()
    };

    for part in parts {
        let (key, raw_value) = part
            .split_once('=')
            .with_context(|| format!("route option {part} must be key=value"))?;
        match key {
            "via" => route.via = Some(raw_value.to_string()),
            "dev" | "interface" | "interface_name" => {
                route.interface_name = Some(raw_value.to_string());
            }
            "metric" => {
                route.metric = Some(
                    raw_value
                        .parse()
                        .with_context(|| format!("route metric {raw_value} is invalid"))?,
                );
            }
            _ => bail!("unknown route option {key}"),
        }
    }
    Ok(route)
}
