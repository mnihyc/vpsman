use anyhow::{Context, Result};
use vpsman_common::RuntimeTunnelManager;

use crate::{
    http::http_post_json,
    network_runtime_args::{
        build_runtime_control, build_runtime_topology, split_argv_spec, RuntimeControlArgs,
        RuntimeTopologyArgs,
    },
};

#[derive(Debug, PartialEq)]
pub(crate) struct VtyTunnelPromoteAdapterRequest {
    pub(crate) plan_id: String,
    pub(crate) runtime_startup_argv: Vec<String>,
    pub(crate) runtime_stop_argv: Vec<String>,
    pub(crate) runtime_cleanup_argv: Vec<String>,
    pub(crate) runtime_restart_argv: Vec<String>,
    pub(crate) runtime_status_argv: Vec<String>,
    pub(crate) runtime_traffic_limit_argv: Vec<String>,
    pub(crate) traffic_ingress_kbps: Option<u32>,
    pub(crate) traffic_egress_kbps: Option<u32>,
    pub(crate) traffic_burst_kb: Option<u32>,
    pub(crate) fou_port: Option<u16>,
    pub(crate) fou_peer_port: Option<u16>,
    pub(crate) fou_ipproto: Option<u8>,
    pub(crate) topology_version: Option<String>,
    pub(crate) topology_desired_interfaces: Vec<String>,
    pub(crate) topology_stale_interfaces: Vec<String>,
    pub(crate) topology_routes: Vec<String>,
    pub(crate) topology_stale_routes: Vec<String>,
    pub(crate) name: Option<String>,
    pub(crate) confirmed: bool,
}

pub(crate) fn submit_vty_tunnel_promote_adapter(
    api_url: &str,
    token: Option<&str>,
    request: VtyTunnelPromoteAdapterRequest,
) -> Result<String> {
    let runtime_control = build_runtime_control(RuntimeControlArgs {
        manager: RuntimeTunnelManager::ExternalManagedAdapter,
        startup_argv: &request.runtime_startup_argv,
        stop_argv: &request.runtime_stop_argv,
        cleanup_argv: &request.runtime_cleanup_argv,
        restart_argv: &request.runtime_restart_argv,
        status_argv: &request.runtime_status_argv,
        traffic_limit_argv: &request.runtime_traffic_limit_argv,
        traffic_ingress_kbps: request.traffic_ingress_kbps,
        traffic_egress_kbps: request.traffic_egress_kbps,
        traffic_burst_kb: request.traffic_burst_kb,
        fou_port: request.fou_port,
        fou_peer_port: request.fou_peer_port,
        fou_ipproto: request.fou_ipproto,
    });
    let runtime_topology = build_runtime_topology(RuntimeTopologyArgs {
        version: request.topology_version.as_deref(),
        desired_interfaces: &request.topology_desired_interfaces,
        stale_interfaces: &request.topology_stale_interfaces,
        routes: &request.topology_routes,
        stale_routes: &request.topology_stale_routes,
    })?;
    let runtime_topology = if runtime_topology.is_default() {
        None
    } else {
        Some(runtime_topology)
    };
    http_post_json(
        api_url,
        "/api/v1/tunnel-plans/promote-adapter",
        token,
        &serde_json::json!({
            "plan_id": request.plan_id,
            "runtime_control": runtime_control,
            "runtime_topology": runtime_topology,
            "name": request.name,
            "confirmed": request.confirmed,
        }),
    )
}

pub(crate) fn parse_vty_tunnel_promote_adapter(
    tokens: &[&str],
) -> Result<VtyTunnelPromoteAdapterRequest> {
    let mut plan_id = None::<String>;
    let mut runtime_startup_argv = Vec::new();
    let mut runtime_stop_argv = Vec::new();
    let mut runtime_cleanup_argv = Vec::new();
    let mut runtime_restart_argv = Vec::new();
    let mut runtime_status_argv = Vec::new();
    let mut runtime_traffic_limit_argv = Vec::new();
    let mut traffic_ingress_kbps = None::<u32>;
    let mut traffic_egress_kbps = None::<u32>;
    let mut traffic_burst_kb = None::<u32>;
    let mut fou_port = None::<u16>;
    let mut fou_peer_port = None::<u16>;
    let mut fou_ipproto = None::<u8>;
    let mut topology_version = None::<String>;
    let mut topology_desired_interfaces = Vec::new();
    let mut topology_stale_interfaces = Vec::new();
    let mut topology_routes = Vec::new();
    let mut topology_stale_routes = Vec::new();
    let mut name = None::<String>;
    let mut confirmed = false;
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--plan-id" => {
                plan_id = Some(next_value(tokens, index, "--plan-id")?.to_string());
                index += 2;
            }
            value if value.starts_with("--plan-id=") => {
                plan_id = Some(flag_value(value, "--plan-id=").to_string());
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
                traffic_ingress_kbps =
                    Some(next_value(tokens, index, "--traffic-ingress-kbps")?.parse()?);
                index += 2;
            }
            value if value.starts_with("--traffic-ingress-kbps=") => {
                traffic_ingress_kbps = Some(flag_value(value, "--traffic-ingress-kbps=").parse()?);
                index += 1;
            }
            "--traffic-egress-kbps" => {
                traffic_egress_kbps =
                    Some(next_value(tokens, index, "--traffic-egress-kbps")?.parse()?);
                index += 2;
            }
            value if value.starts_with("--traffic-egress-kbps=") => {
                traffic_egress_kbps = Some(flag_value(value, "--traffic-egress-kbps=").parse()?);
                index += 1;
            }
            "--traffic-burst-kb" => {
                traffic_burst_kb = Some(next_value(tokens, index, "--traffic-burst-kb")?.parse()?);
                index += 2;
            }
            value if value.starts_with("--traffic-burst-kb=") => {
                traffic_burst_kb = Some(flag_value(value, "--traffic-burst-kb=").parse()?);
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
            "--topology-version" => {
                topology_version =
                    Some(next_value(tokens, index, "--topology-version")?.to_string());
                index += 2;
            }
            value if value.starts_with("--topology-version=") => {
                topology_version = Some(flag_value(value, "--topology-version=").to_string());
                index += 1;
            }
            "--topology-desired-interfaces" => {
                topology_desired_interfaces =
                    split_argv_spec(next_value(tokens, index, "--topology-desired-interfaces")?);
                index += 2;
            }
            value if value.starts_with("--topology-desired-interfaces=") => {
                topology_desired_interfaces =
                    split_argv_spec(flag_value(value, "--topology-desired-interfaces="));
                index += 1;
            }
            "--topology-stale-interfaces" => {
                topology_stale_interfaces =
                    split_argv_spec(next_value(tokens, index, "--topology-stale-interfaces")?);
                index += 2;
            }
            value if value.starts_with("--topology-stale-interfaces=") => {
                topology_stale_interfaces =
                    split_argv_spec(flag_value(value, "--topology-stale-interfaces="));
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
            "--name" => {
                name = Some(next_value(tokens, index, "--name")?.to_string());
                index += 2;
            }
            value if value.starts_with("--name=") => {
                name = Some(flag_value(value, "--name=").to_string());
                index += 1;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            other => anyhow::bail!("unknown tunnel-promote-adapter flag {other}"),
        }
    }
    Ok(VtyTunnelPromoteAdapterRequest {
        plan_id: plan_id.context("tunnel-promote-adapter requires --plan-id")?,
        runtime_startup_argv,
        runtime_stop_argv,
        runtime_cleanup_argv,
        runtime_restart_argv,
        runtime_status_argv,
        runtime_traffic_limit_argv,
        traffic_ingress_kbps,
        traffic_egress_kbps,
        traffic_burst_kb,
        fou_port,
        fou_peer_port,
        fou_ipproto,
        topology_version,
        topology_desired_interfaces,
        topology_stale_interfaces,
        topology_routes,
        topology_stale_routes,
        name,
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

fn parse_u16(value: &str, flag: &str) -> Result<u16> {
    value
        .parse::<u16>()
        .with_context(|| format!("{flag} must be a u16"))
}

fn parse_u8(value: &str, flag: &str) -> Result<u8> {
    value
        .parse::<u8>()
        .with_context(|| format!("{flag} must be a u8"))
}

#[cfg(test)]
mod tests {
    use super::parse_vty_tunnel_promote_adapter;

    #[test]
    fn parses_vty_tunnel_promote_adapter() {
        let request = parse_vty_tunnel_promote_adapter(&[
            "--plan-id=00000000-0000-0000-0000-000000000001",
            "--runtime-startup-argv=/usr/local/libexec/wg-adapter,start,{interface}",
            "--runtime-status-argv",
            "/usr/local/libexec/wg-adapter,status,{interface}",
            "--runtime-stop-argv=/usr/local/libexec/wg-adapter,stop,{interface}",
            "--runtime-cleanup-argv=/usr/local/libexec/wg-adapter,cleanup,{interface}",
            "--traffic-egress-kbps=100000",
            "--traffic-burst-kb",
            "4096",
            "--fou-port=6655",
            "--fou-peer-port",
            "7755",
            "--fou-ipproto=47",
            "--topology-version=adapter-v1",
            "--topology-desired-interfaces=wg42",
            "--topology-route",
            "10.42.0.0/24,dev=wg42,metric=42",
            "--name=managed-wg42",
            "--confirmed",
        ])
        .unwrap();

        assert_eq!(request.plan_id, "00000000-0000-0000-0000-000000000001");
        assert_eq!(
            request.runtime_status_argv,
            vec![
                "/usr/local/libexec/wg-adapter".to_string(),
                "status".to_string(),
                "{interface}".to_string()
            ]
        );
        assert_eq!(
            request.runtime_cleanup_argv,
            vec![
                "/usr/local/libexec/wg-adapter".to_string(),
                "cleanup".to_string(),
                "{interface}".to_string()
            ]
        );
        assert_eq!(request.traffic_egress_kbps, Some(100000));
        assert_eq!(request.traffic_burst_kb, Some(4096));
        assert_eq!(request.fou_port, Some(6655));
        assert_eq!(request.fou_peer_port, Some(7755));
        assert_eq!(request.fou_ipproto, Some(47));
        assert_eq!(request.topology_version.as_deref(), Some("adapter-v1"));
        assert_eq!(
            request.topology_desired_interfaces,
            vec!["wg42".to_string()]
        );
        assert_eq!(request.topology_routes.len(), 1);
        assert_eq!(request.name.as_deref(), Some("managed-wg42"));
        assert!(request.confirmed);
    }
}
