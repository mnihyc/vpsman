use std::{net::IpAddr, path::PathBuf};

use anyhow::{Context, Result};
use uuid::Uuid;
use vpsman_common::{
    plan_tunnel, render_tunnel_endpoint_config, BandwidthMbps, JobCommand, TunnelAddressFamily,
    TunnelAddressPair, TunnelEndpointSide, TunnelPlan, MAX_CONFIGURABLE_JOB_TIMEOUT_SECS,
    MAX_TUNNEL_BANDWIDTH_MBPS, MIN_TUNNEL_BANDWIDTH_MBPS,
};

use crate::{
    commands_schedules::selector_expression_from_targets,
    http::{http_get, http_post_json},
    privilege::build_privilege_for_job_command,
    vty_jobs::VtyPrivilegeContext,
};

pub(crate) use crate::vty_tunnel_plan::{parse_vty_tunnel_plan, VtyTunnelPlanRequest};

#[derive(Debug, PartialEq)]
pub(crate) struct VtyTunnelPlanSideRequest {
    pub(crate) plan_file: PathBuf,
    pub(crate) side: TunnelEndpointSide,
    pub(crate) max_timeout_secs: u64,
    pub(crate) privilege_ttl_secs: u64,
}

pub(crate) type VtyTunnelStatusRequest = VtyTunnelPlanSideRequest;

#[derive(Debug, PartialEq)]
pub(crate) struct VtyTunnelPromoteExternalObserveRequest {
    pub(crate) client_id: String,
    pub(crate) interface: String,
    pub(crate) peer_client_id: String,
    pub(crate) local_underlay: String,
    pub(crate) peer_underlay: String,
    pub(crate) address_pool_cidr: String,
    pub(crate) ipv4_tunnel: Option<TunnelAddressPair>,
    pub(crate) ipv6_address_pool_cidr: Option<String>,
    pub(crate) ipv6_tunnel: Option<TunnelAddressPair>,
    pub(crate) latency_primary_family: TunnelAddressFamily,
    pub(crate) side: TunnelEndpointSide,
    pub(crate) name: Option<String>,
    pub(crate) bandwidth_mbps: Option<BandwidthMbps>,
    pub(crate) latency_ms: Option<f64>,
    pub(crate) packet_loss_ratio: Option<f64>,
    pub(crate) preference: Option<f64>,
    pub(crate) enabled: bool,
    pub(crate) confirmed: bool,
}

#[derive(Debug, PartialEq)]
pub(crate) struct VtyTunnelAllocateRequest {
    pub(crate) ipv4_pool_cidr: Option<String>,
    pub(crate) ipv6_pool_cidr: Option<String>,
    pub(crate) reserved_addresses: Vec<String>,
    pub(crate) include_ipv4: Option<bool>,
    pub(crate) include_ipv6: Option<bool>,
}

#[derive(Debug, PartialEq)]
pub(crate) struct VtyTunnelPlanExportRequest {
    pub(crate) plan_id: Uuid,
    pub(crate) output_file: Option<PathBuf>,
}

pub(crate) fn submit_or_render_vty_tunnel_plan(
    api_url: &str,
    token: Option<&str>,
    request: VtyTunnelPlanRequest,
) -> Result<String> {
    if request.save {
        anyhow::ensure!(request.confirmed, "tunnel-plan --save requires --confirmed");
        let mut body = serde_json::to_value(&request.input)?;
        if let Some(object) = body.as_object_mut() {
            object.insert("confirmed".to_string(), serde_json::Value::Bool(true));
            object.insert(
                "enabled".to_string(),
                serde_json::Value::Bool(request.enabled),
            );
        }
        http_post_json(api_url, "/api/v1/tunnel-plans", token, &body)
    } else {
        let plan = plan_tunnel(&request.input)?;
        Ok(serde_json::to_string_pretty(&plan)?)
    }
}

pub(crate) fn submit_vty_tunnel_allocate(
    api_url: &str,
    token: Option<&str>,
    request: VtyTunnelAllocateRequest,
) -> Result<String> {
    http_post_json(
        api_url,
        "/api/v1/tunnel-plans/allocate",
        token,
        &serde_json::json!({
            "ipv4_pool_cidr": request.ipv4_pool_cidr,
            "ipv6_pool_cidr": request.ipv6_pool_cidr,
            "reserved_addresses": request.reserved_addresses,
            "include_ipv4": request.include_ipv4,
            "include_ipv6": request.include_ipv6,
        }),
    )
}

pub(crate) fn submit_vty_tunnel_plan_export(
    api_url: &str,
    token: Option<&str>,
    request: VtyTunnelPlanExportRequest,
) -> Result<String> {
    let plan = http_get(
        api_url,
        &format!("/api/v1/tunnel-plans/{}/plan", request.plan_id),
        token,
    )?;
    if let Some(path) = request.output_file {
        std::fs::write(&path, &plan)
            .with_context(|| format!("failed to write tunnel plan {}", path.display()))?;
        Ok(format!("wrote {}", path.display()))
    } else {
        Ok(plan)
    }
}

pub(crate) fn submit_vty_tunnel_promote_external_observe(
    api_url: &str,
    token: Option<&str>,
    request: VtyTunnelPromoteExternalObserveRequest,
) -> Result<String> {
    anyhow::ensure!(
        request.confirmed,
        "tunnel-promote-external-observe requires --confirmed"
    );
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
            "ipv4_tunnel": request.ipv4_tunnel,
            "ipv6_address_pool_cidr": request.ipv6_address_pool_cidr,
            "ipv6_tunnel": request.ipv6_tunnel,
            "latency_primary_family": request.latency_primary_family,
            "side": request.side,
            "name": request.name,
            "bandwidth_mbps": request.bandwidth_mbps,
            "latency_ms": request.latency_ms,
            "packet_loss_ratio": request.packet_loss_ratio,
            "preference": request.preference,
            "enabled": request.enabled,
            "confirmed": true,
        }),
    )
}

pub(crate) fn parse_vty_tunnel_allocate(tokens: &[&str]) -> Result<VtyTunnelAllocateRequest> {
    let mut ipv4_pool_cidr = None::<String>;
    let mut ipv6_pool_cidr = None::<String>;
    let mut reserved_addresses = Vec::<String>::new();
    let mut include_ipv4 = None::<bool>;
    let mut include_ipv6 = None::<bool>;
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--ipv4-pool-cidr" | "--address-pool-cidr" | "--pool-cidr" => {
                ipv4_pool_cidr = Some(next_value(tokens, index, tokens[index])?.to_string());
                index += 2;
            }
            value if value.starts_with("--ipv4-pool-cidr=") => {
                ipv4_pool_cidr = Some(flag_value(value, "--ipv4-pool-cidr=").to_string());
                index += 1;
            }
            value if value.starts_with("--address-pool-cidr=") => {
                ipv4_pool_cidr = Some(flag_value(value, "--address-pool-cidr=").to_string());
                index += 1;
            }
            value if value.starts_with("--pool-cidr=") => {
                ipv4_pool_cidr = Some(flag_value(value, "--pool-cidr=").to_string());
                index += 1;
            }
            "--ipv6-pool-cidr" | "--ipv6-address-pool-cidr" => {
                ipv6_pool_cidr = Some(next_value(tokens, index, tokens[index])?.to_string());
                index += 2;
            }
            value if value.starts_with("--ipv6-pool-cidr=") => {
                ipv6_pool_cidr = Some(flag_value(value, "--ipv6-pool-cidr=").to_string());
                index += 1;
            }
            value if value.starts_with("--ipv6-address-pool-cidr=") => {
                ipv6_pool_cidr = Some(flag_value(value, "--ipv6-address-pool-cidr=").to_string());
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
            "--include-ipv4" => {
                include_ipv4 = Some(true);
                index += 1;
            }
            "--no-ipv4" | "--disable-ipv4" => {
                include_ipv4 = Some(false);
                index += 1;
            }
            value if value.starts_with("--include-ipv4=") => {
                include_ipv4 = Some(parse_bool(
                    flag_value(value, "--include-ipv4="),
                    "--include-ipv4",
                )?);
                index += 1;
            }
            "--include-ipv6" => {
                include_ipv6 = Some(true);
                index += 1;
            }
            "--no-ipv6" | "--disable-ipv6" => {
                include_ipv6 = Some(false);
                index += 1;
            }
            value if value.starts_with("--include-ipv6=") => {
                include_ipv6 = Some(parse_bool(
                    flag_value(value, "--include-ipv6="),
                    "--include-ipv6",
                )?);
                index += 1;
            }
            other => anyhow::bail!("unknown tunnel-allocate flag {other}"),
        }
    }
    Ok(VtyTunnelAllocateRequest {
        ipv4_pool_cidr,
        ipv6_pool_cidr,
        reserved_addresses,
        include_ipv4,
        include_ipv6,
    })
}

pub(crate) fn parse_vty_tunnel_plan_export(tokens: &[&str]) -> Result<VtyTunnelPlanExportRequest> {
    let mut plan_id = None::<Uuid>;
    let mut output_file = None::<PathBuf>;
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--plan-id" => {
                plan_id = Some(next_value(tokens, index, "--plan-id")?.parse()?);
                index += 2;
            }
            value if value.starts_with("--plan-id=") => {
                plan_id = Some(flag_value(value, "--plan-id=").parse()?);
                index += 1;
            }
            "--output-file" | "--output" => {
                output_file = Some(PathBuf::from(next_value(tokens, index, tokens[index])?));
                index += 2;
            }
            value if value.starts_with("--output-file=") => {
                output_file = Some(PathBuf::from(flag_value(value, "--output-file=")));
                index += 1;
            }
            value if value.starts_with("--output=") => {
                output_file = Some(PathBuf::from(flag_value(value, "--output=")));
                index += 1;
            }
            other => anyhow::bail!("unknown tunnel-plan-export flag {other}"),
        }
    }
    Ok(VtyTunnelPlanExportRequest {
        plan_id: required(plan_id, "--plan-id")?,
        output_file,
    })
}

pub(crate) fn parse_vty_tunnel_promote_external_observe(
    tokens: &[&str],
) -> Result<VtyTunnelPromoteExternalObserveRequest> {
    let mut client_id = None::<String>;
    let mut interface = None::<String>;
    let mut peer_client_id = None::<String>;
    let mut local_underlay = None::<String>;
    let mut peer_underlay = None::<String>;
    let mut address_pool_cidr = None::<String>;
    let mut left_tunnel_ipv4_cidr = None::<String>;
    let mut right_tunnel_ipv4_cidr = None::<String>;
    let mut ipv6_address_pool_cidr = None::<String>;
    let mut left_tunnel_ipv6_cidr = None::<String>;
    let mut right_tunnel_ipv6_cidr = None::<String>;
    let mut latency_primary_family = TunnelAddressFamily::Ipv4;
    let mut side = TunnelEndpointSide::Left;
    let mut name = None::<String>;
    let mut bandwidth = None::<BandwidthMbps>;
    let mut latency_ms = None::<f64>;
    let mut packet_loss_ratio = None::<f64>;
    let mut preference = None::<f64>;
    let mut enabled = false;
    let mut confirmed = false;
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--enabled" => {
                enabled = true;
                index += 1;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
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
            "--side" => {
                side = parse_tunnel_endpoint_side(next_value(tokens, index, "--side")?)?;
                index += 2;
            }
            value if value.starts_with("--side=") => {
                side = parse_tunnel_endpoint_side(flag_value(value, "--side="))?;
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
            "--bandwidth-mbps" => {
                bandwidth = Some(parse_promote_bandwidth_mbps(next_value(
                    tokens,
                    index,
                    "--bandwidth-mbps",
                )?)?);
                index += 2;
            }
            value if value.starts_with("--bandwidth-mbps=") => {
                bandwidth = Some(parse_promote_bandwidth_mbps(flag_value(
                    value,
                    "--bandwidth-mbps=",
                ))?);
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
            other => anyhow::bail!("unknown tunnel-promote-external-observe flag {other}"),
        }
    }
    let ipv4_tunnel = build_address_pair_from_cidrs(
        left_tunnel_ipv4_cidr,
        right_tunnel_ipv4_cidr,
        TunnelAddressFamily::Ipv4,
        "IPv4",
    )?;
    let ipv6_tunnel = build_address_pair_from_cidrs(
        left_tunnel_ipv6_cidr,
        right_tunnel_ipv6_cidr,
        TunnelAddressFamily::Ipv6,
        "IPv6",
    )?;
    ensure_explicit_tunnel_endpoints(
        &ipv4_tunnel,
        &ipv6_tunnel,
        "tunnel-promote-external-observe",
    )?;
    Ok(VtyTunnelPromoteExternalObserveRequest {
        client_id: client_id.context("tunnel-promote-external-observe requires --client-id")?,
        interface: interface.context("tunnel-promote-external-observe requires --interface")?,
        peer_client_id: peer_client_id
            .context("tunnel-promote-external-observe requires --peer-client-id")?,
        local_underlay: local_underlay
            .context("tunnel-promote-external-observe requires --local-underlay")?,
        peer_underlay: peer_underlay
            .context("tunnel-promote-external-observe requires --peer-underlay")?,
        address_pool_cidr: address_pool_cidr.unwrap_or_default(),
        ipv4_tunnel,
        ipv6_address_pool_cidr,
        ipv6_tunnel,
        latency_primary_family,
        side,
        name,
        bandwidth_mbps: bandwidth,
        latency_ms,
        packet_loss_ratio,
        preference,
        enabled,
        confirmed,
    })
}

pub(crate) fn parse_vty_tunnel_status(tokens: &[&str]) -> Result<VtyTunnelStatusRequest> {
    parse_vty_tunnel_plan_side_request(tokens, "tunnel-status")
}

fn parse_vty_tunnel_plan_side_request(
    tokens: &[&str],
    command_name: &str,
) -> Result<VtyTunnelPlanSideRequest> {
    let mut plan_file = None::<PathBuf>;
    let mut side = None::<TunnelEndpointSide>;
    let mut max_timeout_secs = 60_u64;
    let mut privilege_ttl_secs = 300_u64;

    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--plan-file" => {
                plan_file = Some(PathBuf::from(next_value(tokens, index, "--plan-file")?));
                index += 2;
            }
            value if value.starts_with("--plan-file=") => {
                plan_file = Some(PathBuf::from(flag_value(value, "--plan-file=")));
                index += 1;
            }
            "--side" => {
                side = Some(parse_tunnel_endpoint_side(next_value(
                    tokens, index, "--side",
                )?)?);
                index += 2;
            }
            value if value.starts_with("--side=") => {
                side = Some(parse_tunnel_endpoint_side(flag_value(value, "--side="))?);
                index += 1;
            }
            "--max-timeout" | "--max-timeout-secs" => {
                max_timeout_secs = parse_bounded_u64(
                    next_value(tokens, index, tokens[index])?,
                    tokens[index],
                    1,
                    MAX_CONFIGURABLE_JOB_TIMEOUT_SECS,
                )?;
                index += 2;
            }
            value if value.starts_with("--max-timeout=") => {
                max_timeout_secs = parse_bounded_u64(
                    flag_value(value, "--max-timeout="),
                    "--max-timeout",
                    1,
                    MAX_CONFIGURABLE_JOB_TIMEOUT_SECS,
                )?;
                index += 1;
            }
            value if value.starts_with("--max-timeout-secs=") => {
                max_timeout_secs = parse_bounded_u64(
                    flag_value(value, "--max-timeout-secs="),
                    "--max-timeout-secs",
                    1,
                    MAX_CONFIGURABLE_JOB_TIMEOUT_SECS,
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
                    15,
                    300,
                )?;
                index += 1;
            }
            value if value.starts_with("--privilege-ttl-secs=") => {
                privilege_ttl_secs = parse_bounded_u64(
                    flag_value(value, "--privilege-ttl-secs="),
                    "--privilege-ttl-secs",
                    15,
                    300,
                )?;
                index += 1;
            }
            other => anyhow::bail!("unknown {command_name} flag {other}"),
        }
    }

    Ok(VtyTunnelPlanSideRequest {
        plan_file: required(plan_file, "--plan-file")?,
        side: required(side, "--side")?,
        max_timeout_secs,
        privilege_ttl_secs,
    })
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
        request.max_timeout_secs,
        false,
        false,
        false,
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
    max_timeout_secs: u64,
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
        max_timeout_secs,
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
            "max_timeout_secs": max_timeout_secs,
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

fn parse_tunnel_endpoint_side(value: &str) -> Result<TunnelEndpointSide> {
    match value {
        "left" => Ok(TunnelEndpointSide::Left),
        "right" => Ok(TunnelEndpointSide::Right),
        _ => anyhow::bail!("--side must be one of left, right"),
    }
}

fn parse_promote_bandwidth_mbps(value: &str) -> Result<BandwidthMbps> {
    let parsed = value
        .parse::<u32>()
        .with_context(|| "--bandwidth-mbps must be an integer")?;
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

fn parse_bool(value: &str, flag: &str) -> Result<bool> {
    match value {
        "true" | "yes" | "1" | "on" => Ok(true),
        "false" | "no" | "0" | "off" => Ok(false),
        _ => anyhow::bail!("{flag} must be true or false"),
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
    use std::path::PathBuf;

    use super::{
        parse_vty_tunnel_allocate, parse_vty_tunnel_plan_export,
        parse_vty_tunnel_promote_external_observe, parse_vty_tunnel_status,
    };
    use uuid::Uuid;
    use vpsman_common::{BandwidthMbps, TunnelAddressFamily, TunnelEndpointSide};

    #[test]
    fn parses_vty_tunnel_promote_external_observe() {
        let request = parse_vty_tunnel_promote_external_observe(&[
            "--client-id=edge-a",
            "--interface",
            "tun42",
            "--peer-client-id=edge-b",
            "--local-underlay=198.51.100.10",
            "--peer-underlay",
            "198.51.100.11",
            "--address-pool-cidr=10.44.0.0/30",
            "--left-tunnel-ipv4-cidr=10.44.0.0/31",
            "--right-tunnel-ipv4-cidr=10.44.0.1/31",
            "--side=right",
            "--name",
            "imported-tun42",
            "--bandwidth-mbps",
            "1000",
            "--latency-ms=21.5",
            "--packet-loss-ratio",
            "0.02",
            "--preference=1.5",
            "--enabled",
            "--confirmed",
        ])
        .unwrap();

        assert_eq!(request.client_id, "edge-a");
        assert_eq!(request.interface, "tun42");
        assert_eq!(request.peer_client_id, "edge-b");
        assert_eq!(request.local_underlay, "198.51.100.10");
        assert_eq!(request.peer_underlay, "198.51.100.11");
        assert_eq!(request.address_pool_cidr, "10.44.0.0/30");
        assert_eq!(request.ipv4_tunnel.as_ref().unwrap().left, "10.44.0.0");
        assert_eq!(request.side, TunnelEndpointSide::Right);
        assert_eq!(request.name.as_deref(), Some("imported-tun42"));
        assert_eq!(request.bandwidth_mbps, Some(1000));
        assert_eq!(request.latency_ms, Some(21.5));
        assert_eq!(request.packet_loss_ratio, Some(0.02));
        assert_eq!(request.preference, Some(1.5));
        assert!(request.enabled);
        assert!(request.confirmed);
    }

    #[test]
    fn parses_vty_tunnel_promote_external_observe_explicit_endpoints() {
        let request = parse_vty_tunnel_promote_external_observe(&[
            "--client-id=edge-a",
            "--interface=tun42",
            "--peer-client-id=edge-b",
            "--local-underlay=198.51.100.10",
            "--peer-underlay=198.51.100.11",
            "--left-tunnel-ipv4-cidr=10.44.0.0/31",
            "--right-tunnel-ipv4-cidr=10.44.0.1/31",
            "--ipv6-pool-cidr=fd7a:115c:a1e0:44::/126",
            "--latency-primary-family=ipv6",
        ])
        .unwrap();

        assert_eq!(request.address_pool_cidr, "");
        assert_eq!(request.ipv4_tunnel.as_ref().unwrap().right, "10.44.0.1");
        assert_eq!(
            request.ipv6_address_pool_cidr.as_deref(),
            Some("fd7a:115c:a1e0:44::/126")
        );
        assert_eq!(request.latency_primary_family, TunnelAddressFamily::Ipv6);
        assert!(!request.enabled);
    }

    #[test]
    fn parses_vty_tunnel_allocate() {
        let request = parse_vty_tunnel_allocate(&[
            "--ipv4-pool-cidr=10.255.40.0/24",
            "--ipv6-pool-cidr",
            "fd7a:115c:a1e0:40::/120",
            "--reserved=10.255.40.0,10.255.40.1",
            "--include-ipv6",
            "--include-ipv4=false",
        ])
        .unwrap();

        assert_eq!(request.ipv4_pool_cidr.as_deref(), Some("10.255.40.0/24"));
        assert_eq!(
            request.ipv6_pool_cidr.as_deref(),
            Some("fd7a:115c:a1e0:40::/120")
        );
        assert_eq!(
            request.reserved_addresses,
            vec!["10.255.40.0", "10.255.40.1"]
        );
        assert_eq!(request.include_ipv4, Some(false));
        assert_eq!(request.include_ipv6, Some(true));
    }

    #[test]
    fn parses_vty_tunnel_plan_export() {
        let request = parse_vty_tunnel_plan_export(&[
            "--plan-id=00000000-0000-0000-0000-000000000001",
            "--output-file",
            "/tmp/plan.json",
        ])
        .unwrap();

        assert_eq!(
            request.plan_id,
            Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
        );
        assert_eq!(request.output_file, Some(PathBuf::from("/tmp/plan.json")));
    }

    #[test]
    fn rejects_vty_tunnel_promote_external_observe_missing_required_fields_or_bad_bandwidth() {
        assert!(parse_vty_tunnel_promote_external_observe(&[
            "--client-id=edge-a",
            "--interface=tun42",
            "--peer-client-id=edge-b",
            "--local-underlay=198.51.100.10",
            "--peer-underlay=198.51.100.11",
            "--address-pool-cidr=10.44.0.0/30",
            "--bandwidth-mbps=25m",
        ])
        .is_err());
        assert!(parse_vty_tunnel_promote_external_observe(&[
            "--client-id=edge-a",
            "--interface=tun42",
            "--peer-client-id=edge-b",
            "--local-underlay=198.51.100.10",
            "--peer-underlay=198.51.100.11",
            "--left-tunnel-ipv4-cidr=10.44.0.0/31",
        ])
        .is_err());
        assert!(parse_vty_tunnel_promote_external_observe(&[
            "--client-id=edge-a",
            "--interface=tun42",
            "--peer-client-id=edge-b",
            "--local-underlay=198.51.100.10",
            "--peer-underlay=198.51.100.11",
            "--address-pool-cidr=10.44.0.0/30",
        ])
        .is_err());
        assert!(parse_vty_tunnel_promote_external_observe(&[
            "--client-id=edge-a",
            "--interface=tun42",
            "--peer-client-id=edge-b",
            "--local-underlay=198.51.100.10",
        ])
        .is_err());
    }

    #[test]
    fn parses_vty_tunnel_status_without_confirmation() {
        let request = parse_vty_tunnel_status(&[
            "--plan-file=/tmp/plan.json",
            "--side=right",
            "--max-timeout=45",
            "--privilege-ttl=75",
        ])
        .unwrap();

        assert_eq!(
            request.plan_file,
            std::path::PathBuf::from("/tmp/plan.json")
        );
        assert_eq!(request.side, TunnelEndpointSide::Right);
        assert_eq!(request.max_timeout_secs, 45);
        assert_eq!(request.privilege_ttl_secs, 75);
    }
}
