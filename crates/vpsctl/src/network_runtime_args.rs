use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use vpsman_common::{
    default_runtime_fou_ipproto, default_runtime_fou_peer_port, default_runtime_fou_port,
    RuntimeTunnelControl, RuntimeTunnelFouOptions, RuntimeTunnelManager, RuntimeTunnelRoute,
    RuntimeTunnelTopologyIntent, RuntimeTunnelTrafficLimit,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
#[value(rename_all = "snake_case")]
pub(crate) enum RuntimeManagerArg {
    AgentIproute2Managed,
    ExternalObserved,
    ExternalManagedAdapter,
}

impl From<RuntimeManagerArg> for RuntimeTunnelManager {
    fn from(value: RuntimeManagerArg) -> Self {
        match value {
            RuntimeManagerArg::AgentIproute2Managed => Self::AgentIproute2Managed,
            RuntimeManagerArg::ExternalObserved => Self::ExternalObserved,
            RuntimeManagerArg::ExternalManagedAdapter => Self::ExternalManagedAdapter,
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
        "agent_iproute2_managed" | "iproute2" | "agent" => {
            Ok(RuntimeTunnelManager::AgentIproute2Managed)
        }
        "external_observed" | "observed" => Ok(RuntimeTunnelManager::ExternalObserved),
        "external_managed_adapter" | "adapter" | "external_adapter" => {
            Ok(RuntimeTunnelManager::ExternalManagedAdapter)
        }
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
