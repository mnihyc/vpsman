use std::path::PathBuf;

use anyhow::{Context, Result};
use uuid::Uuid;
use vpsman_common::{
    plan_tunnel, render_tunnel_endpoint_config, JobCommand, TunnelEndpointSide,
    MAX_CONFIGURABLE_JOB_TIMEOUT_SECS,
};

use crate::{
    commands_schedules::selector_expression_from_targets,
    http::{http_get, http_post_json, http_put_json},
};

pub(crate) use crate::vty_tunnel_plan::{parse_vty_tunnel_plan, VtyTunnelPlanRequest};

#[derive(Debug, PartialEq)]
pub(crate) struct VtyTunnelPlanSideRequest {
    pub(crate) plan_id: Uuid,
    pub(crate) side: TunnelEndpointSide,
    pub(crate) max_timeout_secs: u64,
}

pub(crate) type VtyTunnelStatusRequest = VtyTunnelPlanSideRequest;

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

#[derive(Debug, PartialEq)]
pub(crate) struct VtyTunnelPlanMutationRequest {
    pub(crate) plan_id: Uuid,
    pub(crate) expected_revision: i64,
    pub(crate) confirmed: bool,
}

#[derive(Debug, PartialEq)]
pub(crate) struct VtyTunnelOspfStatusRefreshRequest {
    pub(crate) plan_id: Uuid,
}

pub(crate) fn submit_or_render_vty_tunnel_plan(
    api_url: &str,
    token: Option<&str>,
    request: VtyTunnelPlanRequest,
) -> Result<String> {
    if request.save {
        anyhow::ensure!(request.confirmed, "tunnel-plan --save requires --confirmed");
        let body = vty_tunnel_plan_mutation_body(&request)?;
        if let Some(plan_id) = request.update_plan_id {
            http_put_json(
                api_url,
                &format!("/api/v1/tunnel-plans/{plan_id}"),
                token,
                &body,
            )
        } else {
            http_post_json(api_url, "/api/v1/tunnel-plans", token, &body)
        }
    } else {
        let plan = plan_tunnel(&request.input)?;
        Ok(serde_json::to_string_pretty(&plan)?)
    }
}

fn vty_tunnel_plan_mutation_body(request: &VtyTunnelPlanRequest) -> Result<serde_json::Value> {
    let mut body = serde_json::to_value(&request.input)?;
    if let Some(object) = body.as_object_mut() {
        object.insert("confirmed".to_string(), serde_json::Value::Bool(true));
        if request.update_plan_id.is_none() || request.enabled {
            object.insert(
                "enabled".to_string(),
                serde_json::Value::Bool(request.enabled),
            );
        }
        if let Some(expected_revision) = request.expected_revision {
            object.insert(
                "expected_revision".to_string(),
                serde_json::Value::Number(expected_revision.into()),
            );
        }
    }
    Ok(body)
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

pub(crate) fn submit_vty_tunnel_plan_enabled(
    api_url: &str,
    token: Option<&str>,
    request: VtyTunnelPlanMutationRequest,
    enabled: bool,
) -> Result<String> {
    submit_vty_tunnel_plan_mutation(
        api_url,
        token,
        request,
        if enabled { "enable" } else { "disable" },
    )
}

pub(crate) fn submit_vty_tunnel_plan_delete(
    api_url: &str,
    token: Option<&str>,
    request: VtyTunnelPlanMutationRequest,
) -> Result<String> {
    submit_vty_tunnel_plan_mutation(api_url, token, request, "delete")
}

fn submit_vty_tunnel_plan_mutation(
    api_url: &str,
    token: Option<&str>,
    request: VtyTunnelPlanMutationRequest,
    operation: &str,
) -> Result<String> {
    anyhow::ensure!(
        request.confirmed,
        "tunnel plan {operation} requires --confirmed"
    );
    http_post_json(
        api_url,
        &format!("/api/v1/tunnel-plans/{}/{operation}", request.plan_id),
        token,
        &serde_json::json!({
            "confirmed": true,
            "expected_revision": request.expected_revision,
        }),
    )
}

pub(crate) fn submit_vty_tunnel_ospf_status_refresh(
    api_url: &str,
    token: Option<&str>,
    request: VtyTunnelOspfStatusRefreshRequest,
) -> Result<String> {
    http_post_json(
        api_url,
        &format!("/api/v1/tunnel-plans/{}/ospf-status", request.plan_id),
        token,
        &serde_json::json!({}),
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

pub(crate) fn parse_vty_tunnel_plan_mutation(
    tokens: &[&str],
    command_name: &str,
) -> Result<VtyTunnelPlanMutationRequest> {
    let mut plan_id = None::<Uuid>;
    let mut expected_revision = None::<i64>;
    let mut confirmed = false;
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
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            other => anyhow::bail!("unknown {command_name} flag {other}"),
        }
    }
    anyhow::ensure!(confirmed, "{command_name} requires --confirmed");
    let expected_revision = required(expected_revision, "--expected-revision")?;
    anyhow::ensure!(
        expected_revision > 0,
        "--expected-revision must be positive"
    );
    Ok(VtyTunnelPlanMutationRequest {
        plan_id: required(plan_id, "--plan-id")?,
        expected_revision,
        confirmed,
    })
}

pub(crate) fn parse_vty_tunnel_ospf_status_refresh(
    tokens: &[&str],
) -> Result<VtyTunnelOspfStatusRefreshRequest> {
    let mut plan_id = None::<Uuid>;
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
            other => anyhow::bail!("unknown tunnel-ospf-status-refresh flag {other}"),
        }
    }
    Ok(VtyTunnelOspfStatusRefreshRequest {
        plan_id: required(plan_id, "--plan-id")?,
    })
}

pub(crate) fn parse_vty_tunnel_status(tokens: &[&str]) -> Result<VtyTunnelStatusRequest> {
    parse_vty_tunnel_plan_side_request(tokens, "tunnel-status")
}

fn parse_vty_tunnel_plan_side_request(
    tokens: &[&str],
    command_name: &str,
) -> Result<VtyTunnelPlanSideRequest> {
    let mut plan_id = None::<Uuid>;
    let mut side = None::<TunnelEndpointSide>;
    let mut max_timeout_secs = 60_u64;

    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--plan-id" => {
                plan_id = Some(parse_uuid(
                    next_value(tokens, index, "--plan-id")?,
                    "--plan-id",
                )?);
                index += 2;
            }
            value if value.starts_with("--plan-id=") => {
                plan_id = Some(parse_uuid(flag_value(value, "--plan-id="), "--plan-id")?);
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
            other => anyhow::bail!("unknown {command_name} flag {other}"),
        }
    }

    Ok(VtyTunnelPlanSideRequest {
        plan_id: required(plan_id, "--plan-id")?,
        side: required(side, "--side")?,
        max_timeout_secs,
    })
}

pub(crate) fn submit_vty_tunnel_status(
    api_url: &str,
    token: Option<&str>,
    request: VtyTunnelStatusRequest,
) -> Result<String> {
    let plan = crate::commands_network::fetch_tunnel_plan(api_url, token, request.plan_id)?;
    let endpoint = render_tunnel_endpoint_config(&plan, request.side)?;
    let operation = JobCommand::NetworkStatus {
        plan_id: request.plan_id.to_string(),
        plan: Box::new(plan),
        side: request.side,
        runtime_adapter: None,
    };
    let target_clients = vec![endpoint.local_client_id];
    let selector_expression = selector_expression_from_targets(&target_clients, &[]);
    http_post_json(
        api_url,
        "/api/v1/jobs",
        token,
        &serde_json::json!({
            "job_id": Uuid::new_v4(),
            "command": "network_status",
            "argv": [],
            "selector_expression": selector_expression,
            "target_client_ids": target_clients,
            "privileged": false,
            "destructive": false,
            "confirmed": false,
            "force_unprivileged": true,
            "max_timeout_secs": request.max_timeout_secs,
            "operation": operation,
            "privilege_assertion": null,
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

fn parse_tunnel_endpoint_side(value: &str) -> Result<TunnelEndpointSide> {
    match value {
        "left" => Ok(TunnelEndpointSide::Left),
        "right" => Ok(TunnelEndpointSide::Right),
        _ => anyhow::bail!("--side must be one of left, right"),
    }
}

fn parse_uuid(value: &str, flag: &str) -> Result<Uuid> {
    Uuid::parse_str(value).with_context(|| format!("{flag} must be a UUID"))
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
        parse_vty_tunnel_allocate, parse_vty_tunnel_ospf_status_refresh, parse_vty_tunnel_plan,
        parse_vty_tunnel_plan_export, parse_vty_tunnel_plan_mutation, parse_vty_tunnel_status,
        vty_tunnel_plan_mutation_body,
    };
    use uuid::Uuid;
    use vpsman_common::TunnelEndpointSide;

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
    fn parses_explicit_plan_lifecycle_and_ospf_status_refresh() {
        let enabled = parse_vty_tunnel_plan_mutation(
            &[
                "--plan-id=00000000-0000-0000-0000-000000000001",
                "--expected-revision=3",
                "--confirmed",
            ],
            "tunnel-plan-enable",
        )
        .unwrap();
        let deleted = parse_vty_tunnel_plan_mutation(
            &[
                "--plan-id=00000000-0000-0000-0000-000000000001",
                "--expected-revision=4",
                "--confirmed",
            ],
            "tunnel-plan-delete",
        )
        .unwrap();
        let status = parse_vty_tunnel_ospf_status_refresh(&[
            "--plan-id",
            "00000000-0000-0000-0000-000000000001",
        ])
        .unwrap();

        assert!(enabled.confirmed);
        assert_eq!(enabled.expected_revision, 3);
        assert!(deleted.confirmed);
        assert_eq!(deleted.expected_revision, 4);
        assert_eq!(enabled.plan_id, deleted.plan_id);
        assert_eq!(enabled.plan_id, status.plan_id);
        assert!(parse_vty_tunnel_plan_mutation(
            &[
                "--plan-id=00000000-0000-0000-0000-000000000001",
                "--expected-revision=3",
            ],
            "tunnel-plan-disable",
        )
        .is_err());
    }

    #[test]
    fn parses_vty_tunnel_status_without_confirmation() {
        let request = parse_vty_tunnel_status(&[
            "--plan-id=00000000-0000-0000-0000-000000000001",
            "--side=right",
            "--max-timeout=45",
        ])
        .unwrap();

        assert_eq!(
            request.plan_id,
            Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
        );
        assert_eq!(request.side, TunnelEndpointSide::Right);
        assert_eq!(request.max_timeout_secs, 45);
        assert!(parse_vty_tunnel_status(&[
            "--plan-id=00000000-0000-0000-0000-000000000001",
            "--side=right",
            "--privilege-ttl=75",
        ])
        .is_err());
    }

    #[test]
    fn vty_plan_update_preserves_lifecycle_without_explicit_enabled_flag() {
        let request = parse_vty_tunnel_plan(&[
            "--save",
            "--confirmed",
            "--update-plan-id=00000000-0000-4000-8000-000000000001",
            "--expected-revision=7",
            "--name=edge",
            "--interface=tun0",
            "--kind=gre",
            "--left-client=left",
            "--right-client=right",
            "--left-remote-underlay=198.51.100.10",
            "--right-remote-underlay=203.0.113.20",
            "--left-tunnel-ipv4-cidr=10.255.0.0/31",
            "--right-tunnel-ipv4-cidr=10.255.0.1/31",
            "--bandwidth-mbps=100",
        ])
        .unwrap();
        let body = vty_tunnel_plan_mutation_body(&request).unwrap();

        assert_eq!(body["expected_revision"], 7);
        assert_eq!(body["confirmed"], true);
        assert!(body.get("enabled").is_none());
    }
}
