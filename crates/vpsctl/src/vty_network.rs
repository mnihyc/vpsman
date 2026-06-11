use std::path::PathBuf;

use anyhow::{Context, Result};
use uuid::Uuid;
use vpsman_common::{
    backend_config_signature_payload, payload_hash, plan_tunnel,
    render_tunnel_endpoint_backend_config, render_tunnel_endpoint_config, BandwidthTier,
    JobCommand, TunnelConfigBackend, TunnelEndpointSide, TunnelPlan,
};

use crate::{
    commands_schedules::selector_expression_from_targets, http::http_post_json,
    privilege::build_privilege_for_job_command, vty_jobs::VtyPrivilegeContext,
};

pub(crate) use crate::vty_tunnel_plan::{parse_vty_tunnel_plan, VtyTunnelPlanRequest};

#[derive(Debug, PartialEq)]
pub(crate) struct VtyTunnelApplyRequest {
    pub(crate) plan_file: PathBuf,
    pub(crate) side: TunnelEndpointSide,
    pub(crate) backend: TunnelConfigBackend,
    pub(crate) timeout_secs: u64,
    pub(crate) privilege_ttl_secs: u64,
    pub(crate) confirmed: bool,
    pub(crate) force_unprivileged: bool,
}

pub(crate) type VtyTunnelRollbackRequest = VtyTunnelApplyRequest;
pub(crate) type VtyTunnelStatusRequest = VtyTunnelApplyRequest;

#[derive(Debug, PartialEq)]
pub(crate) struct VtyTunnelPromoteTelemetryRequest {
    pub(crate) client_id: String,
    pub(crate) interface: String,
    pub(crate) peer_client_id: String,
    pub(crate) local_underlay: String,
    pub(crate) peer_underlay: String,
    pub(crate) address_pool_cidr: String,
    pub(crate) side: TunnelEndpointSide,
    pub(crate) name: Option<String>,
    pub(crate) topology_version: Option<String>,
    pub(crate) bandwidth: Option<BandwidthTier>,
    pub(crate) latency_ms: Option<f64>,
    pub(crate) packet_loss_ratio: Option<f64>,
    pub(crate) preference: Option<f64>,
}

pub(crate) fn submit_or_render_vty_tunnel_plan(
    api_url: &str,
    token: Option<&str>,
    request: VtyTunnelPlanRequest,
) -> Result<String> {
    if request.save {
        http_post_json(
            api_url,
            "/api/v1/tunnel-plans",
            token,
            &serde_json::to_value(&request.input)?,
        )
    } else {
        let plan = plan_tunnel(&request.input)?;
        Ok(serde_json::to_string_pretty(&plan)?)
    }
}

pub(crate) fn submit_vty_tunnel_promote_telemetry(
    api_url: &str,
    token: Option<&str>,
    request: VtyTunnelPromoteTelemetryRequest,
) -> Result<String> {
    http_post_json(
        api_url,
        "/api/v1/tunnel-plans/promote-telemetry",
        token,
        &serde_json::json!({
            "client_id": request.client_id,
            "interface": request.interface,
            "peer_client_id": request.peer_client_id,
            "local_underlay": request.local_underlay,
            "peer_underlay": request.peer_underlay,
            "address_pool_cidr": request.address_pool_cidr,
            "side": request.side,
            "name": request.name,
            "topology_version": request.topology_version,
            "bandwidth": request.bandwidth,
            "latency_ms": request.latency_ms,
            "packet_loss_ratio": request.packet_loss_ratio,
            "preference": request.preference,
        }),
    )
}

pub(crate) fn parse_vty_tunnel_promote_telemetry(
    tokens: &[&str],
) -> Result<VtyTunnelPromoteTelemetryRequest> {
    let mut client_id = None::<String>;
    let mut interface = None::<String>;
    let mut peer_client_id = None::<String>;
    let mut local_underlay = None::<String>;
    let mut peer_underlay = None::<String>;
    let mut address_pool_cidr = None::<String>;
    let mut side = TunnelEndpointSide::Left;
    let mut name = None::<String>;
    let mut topology_version = None::<String>;
    let mut bandwidth = None::<BandwidthTier>;
    let mut latency_ms = None::<f64>;
    let mut packet_loss_ratio = None::<f64>;
    let mut preference = None::<f64>;
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--client-id" => {
                client_id = Some(next_value(tokens, index, "--client-id")?.to_string());
                index += 2;
            }
            value if value.starts_with("--client-id=") => {
                client_id = Some(flag_value(value, "--client-id=").to_string());
                index += 1;
            }
            "--interface" => {
                interface = Some(next_value(tokens, index, "--interface")?.to_string());
                index += 2;
            }
            value if value.starts_with("--interface=") => {
                interface = Some(flag_value(value, "--interface=").to_string());
                index += 1;
            }
            "--peer-client-id" => {
                peer_client_id = Some(next_value(tokens, index, "--peer-client-id")?.to_string());
                index += 2;
            }
            value if value.starts_with("--peer-client-id=") => {
                peer_client_id = Some(flag_value(value, "--peer-client-id=").to_string());
                index += 1;
            }
            "--local-underlay" => {
                local_underlay = Some(next_value(tokens, index, "--local-underlay")?.to_string());
                index += 2;
            }
            value if value.starts_with("--local-underlay=") => {
                local_underlay = Some(flag_value(value, "--local-underlay=").to_string());
                index += 1;
            }
            "--peer-underlay" => {
                peer_underlay = Some(next_value(tokens, index, "--peer-underlay")?.to_string());
                index += 2;
            }
            value if value.starts_with("--peer-underlay=") => {
                peer_underlay = Some(flag_value(value, "--peer-underlay=").to_string());
                index += 1;
            }
            "--address-pool-cidr" => {
                address_pool_cidr =
                    Some(next_value(tokens, index, "--address-pool-cidr")?.to_string());
                index += 2;
            }
            value if value.starts_with("--address-pool-cidr=") => {
                address_pool_cidr = Some(flag_value(value, "--address-pool-cidr=").to_string());
                index += 1;
            }
            "--side" => {
                side = parse_tunnel_apply_side(next_value(tokens, index, "--side")?)?;
                index += 2;
            }
            value if value.starts_with("--side=") => {
                side = parse_tunnel_apply_side(flag_value(value, "--side="))?;
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
            "--topology-version" => {
                topology_version =
                    Some(next_value(tokens, index, "--topology-version")?.to_string());
                index += 2;
            }
            value if value.starts_with("--topology-version=") => {
                topology_version = Some(flag_value(value, "--topology-version=").to_string());
                index += 1;
            }
            "--bandwidth" => {
                bandwidth = Some(parse_promote_bandwidth(next_value(
                    tokens,
                    index,
                    "--bandwidth",
                )?)?);
                index += 2;
            }
            value if value.starts_with("--bandwidth=") => {
                bandwidth = Some(parse_promote_bandwidth(flag_value(value, "--bandwidth="))?);
                index += 1;
            }
            "--latency-ms" => {
                latency_ms = Some(next_value(tokens, index, "--latency-ms")?.parse()?);
                index += 2;
            }
            value if value.starts_with("--latency-ms=") => {
                latency_ms = Some(flag_value(value, "--latency-ms=").parse()?);
                index += 1;
            }
            "--packet-loss-ratio" => {
                packet_loss_ratio =
                    Some(next_value(tokens, index, "--packet-loss-ratio")?.parse()?);
                index += 2;
            }
            value if value.starts_with("--packet-loss-ratio=") => {
                packet_loss_ratio = Some(flag_value(value, "--packet-loss-ratio=").parse()?);
                index += 1;
            }
            "--preference" => {
                preference = Some(next_value(tokens, index, "--preference")?.parse()?);
                index += 2;
            }
            value if value.starts_with("--preference=") => {
                preference = Some(flag_value(value, "--preference=").parse()?);
                index += 1;
            }
            other => anyhow::bail!("unknown tunnel-promote-telemetry flag {other}"),
        }
    }
    Ok(VtyTunnelPromoteTelemetryRequest {
        client_id: client_id.context("tunnel-promote-telemetry requires --client-id")?,
        interface: interface.context("tunnel-promote-telemetry requires --interface")?,
        peer_client_id: peer_client_id
            .context("tunnel-promote-telemetry requires --peer-client-id")?,
        local_underlay: local_underlay
            .context("tunnel-promote-telemetry requires --local-underlay")?,
        peer_underlay: peer_underlay
            .context("tunnel-promote-telemetry requires --peer-underlay")?,
        address_pool_cidr: address_pool_cidr
            .context("tunnel-promote-telemetry requires --address-pool-cidr")?,
        side,
        name,
        topology_version,
        bandwidth,
        latency_ms,
        packet_loss_ratio,
        preference,
    })
}

pub(crate) fn parse_vty_tunnel_apply(tokens: &[&str]) -> Result<VtyTunnelApplyRequest> {
    parse_vty_tunnel_change(tokens, "tunnel-apply", true)
}

pub(crate) fn parse_vty_tunnel_rollback(tokens: &[&str]) -> Result<VtyTunnelRollbackRequest> {
    parse_vty_tunnel_change(tokens, "tunnel-rollback", true)
}

pub(crate) fn parse_vty_tunnel_status(tokens: &[&str]) -> Result<VtyTunnelStatusRequest> {
    parse_vty_tunnel_change(tokens, "tunnel-status", false)
}

fn parse_vty_tunnel_change(
    tokens: &[&str],
    command_name: &str,
    require_confirmation: bool,
) -> Result<VtyTunnelApplyRequest> {
    let mut plan_file = None::<PathBuf>;
    let mut side = None::<TunnelEndpointSide>;
    let mut backend = TunnelConfigBackend::Ifupdown;
    let mut timeout_secs = 60_u64;
    let mut privilege_ttl_secs = 300_u64;
    let mut confirmed = false;
    let mut force_unprivileged = false;

    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            "--force-unprivileged" => {
                force_unprivileged = true;
                index += 1;
            }
            "--plan-file" => {
                plan_file = Some(PathBuf::from(next_value(tokens, index, "--plan-file")?));
                index += 2;
            }
            value if value.starts_with("--plan-file=") => {
                plan_file = Some(PathBuf::from(flag_value(value, "--plan-file=")));
                index += 1;
            }
            "--side" => {
                side = Some(parse_tunnel_apply_side(next_value(
                    tokens, index, "--side",
                )?)?);
                index += 2;
            }
            value if value.starts_with("--side=") => {
                side = Some(parse_tunnel_apply_side(flag_value(value, "--side="))?);
                index += 1;
            }
            "--backend" => {
                backend = parse_tunnel_backend(next_value(tokens, index, "--backend")?)?;
                index += 2;
            }
            value if value.starts_with("--backend=") => {
                backend = parse_tunnel_backend(flag_value(value, "--backend="))?;
                index += 1;
            }
            "--timeout" | "--timeout-secs" => {
                timeout_secs = parse_bounded_u64(
                    next_value(tokens, index, tokens[index])?,
                    tokens[index],
                    1,
                    3600,
                )?;
                index += 2;
            }
            value if value.starts_with("--timeout=") => {
                timeout_secs =
                    parse_bounded_u64(flag_value(value, "--timeout="), "--timeout", 1, 3600)?;
                index += 1;
            }
            value if value.starts_with("--timeout-secs=") => {
                timeout_secs = parse_bounded_u64(
                    flag_value(value, "--timeout-secs="),
                    "--timeout-secs",
                    1,
                    3600,
                )?;
                index += 1;
            }
            "--privilege-ttl" | "--privilege-ttl-secs" => {
                privilege_ttl_secs = parse_bounded_u64(
                    next_value(tokens, index, tokens[index])?,
                    tokens[index],
                    1,
                    3600,
                )?;
                index += 2;
            }
            value if value.starts_with("--privilege-ttl=") => {
                privilege_ttl_secs = parse_bounded_u64(
                    flag_value(value, "--privilege-ttl="),
                    "--privilege-ttl",
                    1,
                    3600,
                )?;
                index += 1;
            }
            value if value.starts_with("--privilege-ttl-secs=") => {
                privilege_ttl_secs = parse_bounded_u64(
                    flag_value(value, "--privilege-ttl-secs="),
                    "--privilege-ttl-secs",
                    1,
                    3600,
                )?;
                index += 1;
            }
            other => anyhow::bail!("unknown {command_name} flag {other}"),
        }
    }

    if require_confirmation {
        anyhow::ensure!(confirmed, "{command_name} requires --confirmed");
    }
    Ok(VtyTunnelApplyRequest {
        plan_file: required(plan_file, "--plan-file")?,
        side: required(side, "--side")?,
        backend,
        timeout_secs,
        privilege_ttl_secs,
        confirmed,
        force_unprivileged,
    })
}

pub(crate) fn submit_vty_tunnel_apply(
    api_url: &str,
    token: Option<&str>,
    privilege_context: &VtyPrivilegeContext,
    request: VtyTunnelApplyRequest,
) -> Result<String> {
    let plan_text = std::fs::read_to_string(&request.plan_file)
        .with_context(|| format!("failed to read tunnel plan {}", request.plan_file.display()))?;
    let plan: TunnelPlan =
        serde_json::from_str(&plan_text).context("tunnel plan JSON is invalid")?;
    let endpoint = render_tunnel_endpoint_config(&plan, request.side)?;
    let backend_config =
        render_tunnel_endpoint_backend_config(&plan, request.side, request.backend)?;
    let operation = JobCommand::NetworkApply {
        plan: Box::new(plan),
        side: request.side,
        config_backend: request.backend,
        config_sha256_hex: Some(payload_hash(&backend_config_signature_payload(
            &backend_config,
        ))),
        ifupdown_sha256_hex: payload_hash(endpoint.ifupdown_snippet.as_bytes()),
        bird2_sha256_hex: payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
    };
    submit_vty_network_job(
        api_url,
        token,
        privilege_context,
        "network_apply",
        vec![endpoint.local_client_id],
        operation,
        request.privilege_ttl_secs,
        request.timeout_secs,
        true,
        request.confirmed,
        request.force_unprivileged,
    )
}

pub(crate) fn submit_vty_tunnel_status(
    api_url: &str,
    token: Option<&str>,
    privilege_context: &VtyPrivilegeContext,
    request: VtyTunnelStatusRequest,
) -> Result<String> {
    let plan_text = std::fs::read_to_string(&request.plan_file)
        .with_context(|| format!("failed to read tunnel plan {}", request.plan_file.display()))?;
    let plan: TunnelPlan =
        serde_json::from_str(&plan_text).context("tunnel plan JSON is invalid")?;
    let endpoint = render_tunnel_endpoint_config(&plan, request.side)?;
    let operation = JobCommand::NetworkStatus {
        plan: Box::new(plan),
        side: request.side,
    };
    submit_vty_network_job(
        api_url,
        token,
        privilege_context,
        "network_status",
        vec![endpoint.local_client_id],
        operation,
        request.privilege_ttl_secs,
        request.timeout_secs,
        false,
        false,
        false,
    )
}

pub(crate) fn submit_vty_tunnel_rollback(
    api_url: &str,
    token: Option<&str>,
    privilege_context: &VtyPrivilegeContext,
    request: VtyTunnelRollbackRequest,
) -> Result<String> {
    let plan_text = std::fs::read_to_string(&request.plan_file)
        .with_context(|| format!("failed to read tunnel plan {}", request.plan_file.display()))?;
    let plan: TunnelPlan =
        serde_json::from_str(&plan_text).context("tunnel plan JSON is invalid")?;
    let endpoint = render_tunnel_endpoint_config(&plan, request.side)?;
    let operation = JobCommand::NetworkRollback {
        plan: Box::new(plan),
        side: request.side,
    };
    submit_vty_network_job(
        api_url,
        token,
        privilege_context,
        "network_rollback",
        vec![endpoint.local_client_id],
        operation,
        request.privilege_ttl_secs,
        request.timeout_secs,
        true,
        request.confirmed,
        request.force_unprivileged,
    )
}

fn submit_vty_network_job(
    api_url: &str,
    token: Option<&str>,
    privilege_context: &VtyPrivilegeContext,
    command_label: &str,
    target_clients: Vec<String>,
    operation: JobCommand,
    ttl_secs: u64,
    timeout_secs: u64,
    destructive: bool,
    confirmed: bool,
    force_unprivileged: bool,
) -> Result<String> {
    let selector_expression = selector_expression_from_targets(&target_clients, &[]);
    let privilege = build_privilege_for_job_command(
        &target_clients,
        &operation,
        command_label,
        &selector_expression,
        &privilege_context.password,
        &privilege_context.salt_hex,
        ttl_secs,
        timeout_secs,
        force_unprivileged,
        true,
    )?;
    http_post_json(
        api_url,
        "/api/v1/jobs",
        token,
        &serde_json::json!({
            "job_id": Uuid::new_v4(),
            "command": command_label,
            "argv": [],
            "selector_expression": selector_expression,
            "target_client_ids": target_clients,
            "privileged": true,
            "destructive": destructive,
            "confirmed": confirmed,
            "force_unprivileged": force_unprivileged,
            "timeout_secs": timeout_secs,
            "operation": operation,
            "privilege_assertion": privilege.privilege_assertion,
        }),
    )
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

fn required<T>(value: Option<T>, flag: &str) -> Result<T> {
    value.with_context(|| format!("missing required {flag}"))
}

fn parse_tunnel_apply_side(value: &str) -> Result<TunnelEndpointSide> {
    match value {
        "left" => Ok(TunnelEndpointSide::Left),
        "right" => Ok(TunnelEndpointSide::Right),
        _ => anyhow::bail!("--side must be one of left, right"),
    }
}

fn parse_promote_bandwidth(value: &str) -> Result<BandwidthTier> {
    match value {
        "10m" => Ok(BandwidthTier::M10),
        "100m" => Ok(BandwidthTier::M100),
        "1000m" => Ok(BandwidthTier::M1000),
        _ => anyhow::bail!("--bandwidth must be one of 10m, 100m, 1000m"),
    }
}

fn parse_tunnel_backend(value: &str) -> Result<TunnelConfigBackend> {
    match value {
        "ifupdown" => Ok(TunnelConfigBackend::Ifupdown),
        "netplan" => Ok(TunnelConfigBackend::Netplan),
        "systemd-networkd" | "systemd_networkd" => Ok(TunnelConfigBackend::SystemdNetworkd),
        _ => anyhow::bail!("--backend must be one of ifupdown, netplan, systemd-networkd"),
    }
}

fn parse_bounded_u64(value: &str, flag: &str, min: u64, max: u64) -> Result<u64> {
    let parsed = value
        .parse::<u64>()
        .with_context(|| format!("{flag} must be an integer"))?;
    anyhow::ensure!(
        (min..=max).contains(&parsed),
        "{flag} must be between {min} and {max}"
    );
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::{
        parse_vty_tunnel_apply, parse_vty_tunnel_promote_telemetry, parse_vty_tunnel_rollback,
        parse_vty_tunnel_status,
    };
    use vpsman_common::{BandwidthTier, TunnelConfigBackend, TunnelEndpointSide};

    #[test]
    fn parses_vty_tunnel_apply() {
        let request = parse_vty_tunnel_apply(&[
            "--plan-file=/tmp/plan.json",
            "--side",
            "right",
            "--backend=netplan",
            "--timeout=120",
            "--privilege-ttl",
            "90",
            "--force-unprivileged",
            "--confirmed",
        ])
        .unwrap();

        assert_eq!(
            request.plan_file,
            std::path::PathBuf::from("/tmp/plan.json")
        );
        assert_eq!(request.side, TunnelEndpointSide::Right);
        assert_eq!(request.backend, TunnelConfigBackend::Netplan);
        assert_eq!(request.timeout_secs, 120);
        assert_eq!(request.privilege_ttl_secs, 90);
        assert!(request.confirmed);
        assert!(request.force_unprivileged);
    }

    #[test]
    fn parses_vty_tunnel_promote_telemetry() {
        let request = parse_vty_tunnel_promote_telemetry(&[
            "--client-id=edge-a",
            "--interface",
            "tun42",
            "--peer-client-id=edge-b",
            "--local-underlay=198.51.100.10",
            "--peer-underlay",
            "198.51.100.11",
            "--address-pool-cidr=10.44.0.0/30",
            "--side=right",
            "--name",
            "imported-tun42",
            "--topology-version=observed-v1",
            "--bandwidth",
            "1000m",
            "--latency-ms=21.5",
            "--packet-loss-ratio",
            "0.02",
            "--preference=1.5",
        ])
        .unwrap();

        assert_eq!(request.client_id, "edge-a");
        assert_eq!(request.interface, "tun42");
        assert_eq!(request.peer_client_id, "edge-b");
        assert_eq!(request.local_underlay, "198.51.100.10");
        assert_eq!(request.peer_underlay, "198.51.100.11");
        assert_eq!(request.address_pool_cidr, "10.44.0.0/30");
        assert_eq!(request.side, TunnelEndpointSide::Right);
        assert_eq!(request.name.as_deref(), Some("imported-tun42"));
        assert_eq!(request.topology_version.as_deref(), Some("observed-v1"));
        assert_eq!(request.bandwidth, Some(BandwidthTier::M1000));
        assert_eq!(request.latency_ms, Some(21.5));
        assert_eq!(request.packet_loss_ratio, Some(0.02));
        assert_eq!(request.preference, Some(1.5));
    }

    #[test]
    fn rejects_vty_tunnel_promote_telemetry_missing_required_fields_or_bad_bandwidth() {
        assert!(parse_vty_tunnel_promote_telemetry(&[
            "--client-id=edge-a",
            "--interface=tun42",
            "--peer-client-id=edge-b",
            "--local-underlay=198.51.100.10",
            "--peer-underlay=198.51.100.11",
            "--address-pool-cidr=10.44.0.0/30",
            "--bandwidth=25m",
        ])
        .is_err());
        assert!(parse_vty_tunnel_promote_telemetry(&[
            "--client-id=edge-a",
            "--interface=tun42",
            "--peer-client-id=edge-b",
            "--local-underlay=198.51.100.10",
            "--peer-underlay=198.51.100.11",
        ])
        .is_err());
    }

    #[test]
    fn rejects_vty_tunnel_apply_without_confirmation_or_side() {
        assert!(parse_vty_tunnel_apply(&[
            "--plan-file=/tmp/plan.json",
            "--side=left",
            "--timeout=120",
        ])
        .is_err());
        assert!(parse_vty_tunnel_apply(&[
            "--plan-file=/tmp/plan.json",
            "--side=middle",
            "--confirmed",
        ])
        .is_err());
        assert!(parse_vty_tunnel_apply(&["--plan-file=/tmp/plan.json", "--confirmed",]).is_err());
    }

    #[test]
    fn parses_vty_tunnel_rollback() {
        let request = parse_vty_tunnel_rollback(&[
            "--plan-file=/tmp/plan.json",
            "--side=left",
            "--timeout-secs=180",
            "--privilege-ttl-secs=120",
            "--force-unprivileged",
            "--confirmed",
        ])
        .unwrap();

        assert_eq!(
            request.plan_file,
            std::path::PathBuf::from("/tmp/plan.json")
        );
        assert_eq!(request.side, TunnelEndpointSide::Left);
        assert_eq!(request.timeout_secs, 180);
        assert_eq!(request.privilege_ttl_secs, 120);
        assert!(request.confirmed);
        assert!(request.force_unprivileged);
    }

    #[test]
    fn parses_vty_tunnel_status_without_confirmation() {
        let request = parse_vty_tunnel_status(&[
            "--plan-file=/tmp/plan.json",
            "--side=right",
            "--timeout=45",
            "--privilege-ttl=75",
        ])
        .unwrap();

        assert_eq!(
            request.plan_file,
            std::path::PathBuf::from("/tmp/plan.json")
        );
        assert_eq!(request.side, TunnelEndpointSide::Right);
        assert_eq!(request.timeout_secs, 45);
        assert_eq!(request.privilege_ttl_secs, 75);
        assert!(!request.confirmed);
        assert!(!request.force_unprivileged);
    }
}
