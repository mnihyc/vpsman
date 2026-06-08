use std::path::PathBuf;

use anyhow::{Context, Result};
use vpsman_common::{
    payload_hash, render_tunnel_endpoint_config, JobCommand, TunnelEndpointSide, TunnelPlan,
};

use crate::{
    commands_schedules::selector_expression_from_targets, http::http_post_json,
    privilege::build_privilege_for_job_command, vty_jobs::VtyPrivilegeContext,
};

#[derive(Debug, PartialEq)]
pub(crate) struct VtyTunnelOspfCostUpdateRequest {
    pub(crate) plan_file: PathBuf,
    pub(crate) side: TunnelEndpointSide,
    pub(crate) current_ospf_cost: u16,
    pub(crate) recommended_ospf_cost: u16,
    pub(crate) timeout_secs: u64,
    pub(crate) privilege_ttl_secs: u64,
    pub(crate) confirmed: bool,
    pub(crate) force_unprivileged: bool,
}

pub(crate) fn parse_vty_tunnel_ospf_cost_update(
    tokens: &[&str],
) -> Result<VtyTunnelOspfCostUpdateRequest> {
    let mut plan_file = None::<PathBuf>;
    let mut side = None::<TunnelEndpointSide>;
    let mut current_ospf_cost = None::<u16>;
    let mut recommended_ospf_cost = None::<u16>;
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
            "--current-ospf-cost" => {
                current_ospf_cost = Some(parse_u16(
                    next_value(tokens, index, tokens[index])?,
                    "--current-ospf-cost",
                )?);
                index += 2;
            }
            value if value.starts_with("--current-ospf-cost=") => {
                current_ospf_cost = Some(parse_u16(
                    flag_value(value, "--current-ospf-cost="),
                    "--current-ospf-cost",
                )?);
                index += 1;
            }
            "--recommended-ospf-cost" => {
                recommended_ospf_cost = Some(parse_u16(
                    next_value(tokens, index, tokens[index])?,
                    "--recommended-ospf-cost",
                )?);
                index += 2;
            }
            value if value.starts_with("--recommended-ospf-cost=") => {
                recommended_ospf_cost = Some(parse_u16(
                    flag_value(value, "--recommended-ospf-cost="),
                    "--recommended-ospf-cost",
                )?);
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
            other => anyhow::bail!("unknown tunnel-ospf-cost-update flag {other}"),
        }
    }

    anyhow::ensure!(confirmed, "tunnel-ospf-cost-update requires --confirmed");
    let current_ospf_cost = required(current_ospf_cost, "--current-ospf-cost")?;
    let recommended_ospf_cost = required(recommended_ospf_cost, "--recommended-ospf-cost")?;
    anyhow::ensure!(
        current_ospf_cost != recommended_ospf_cost,
        "tunnel-ospf-cost-update requires a changed OSPF cost"
    );

    Ok(VtyTunnelOspfCostUpdateRequest {
        plan_file: required(plan_file, "--plan-file")?,
        side: required(side, "--side")?,
        current_ospf_cost,
        recommended_ospf_cost,
        timeout_secs,
        privilege_ttl_secs,
        confirmed,
        force_unprivileged,
    })
}

pub(crate) fn submit_vty_tunnel_ospf_cost_update(
    api_url: &str,
    token: Option<&str>,
    privilege_context: &VtyPrivilegeContext,
    request: VtyTunnelOspfCostUpdateRequest,
) -> Result<String> {
    let plan_text = std::fs::read_to_string(&request.plan_file)
        .with_context(|| format!("failed to read tunnel plan {}", request.plan_file.display()))?;
    let mut plan: TunnelPlan =
        serde_json::from_str(&plan_text).context("tunnel plan JSON is invalid")?;
    plan.recommended_ospf_cost = request.recommended_ospf_cost;
    let endpoint = render_tunnel_endpoint_config(&plan, request.side)?;
    let operation = JobCommand::NetworkOspfCostUpdate {
        plan: Box::new(plan),
        side: request.side,
        current_ospf_cost: request.current_ospf_cost,
        recommended_ospf_cost: request.recommended_ospf_cost,
        bird2_sha256_hex: payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
    };
    let target_clients = vec![endpoint.local_client_id];
    let selector_expression = selector_expression_from_targets(&target_clients, &[]);
    let privilege = build_privilege_for_job_command(
        &target_clients,
        &operation,
        "network_ospf_cost_update",
        &selector_expression,
        &privilege_context.password,
        &privilege_context.salt_hex,
        request.privilege_ttl_secs,
        request.timeout_secs,
        None,
        request.force_unprivileged,
        true,
    )?;

    http_post_json(
        api_url,
        "/api/v1/jobs",
        token,
        &serde_json::json!({
            "command": "network_ospf_cost_update",
            "argv": [],
            "selector_expression": selector_expression,
            "privileged": true,
            "destructive": true,
            "confirmed": request.confirmed,
            "force_unprivileged": request.force_unprivileged,
            "timeout_secs": request.timeout_secs,
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

fn parse_u16(value: &str, flag: &str) -> Result<u16> {
    let parsed = value
        .parse::<u16>()
        .with_context(|| format!("{flag} must be an integer"))?;
    anyhow::ensure!(parsed > 0, "{flag} must be between 1 and 65535");
    Ok(parsed)
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
    use super::parse_vty_tunnel_ospf_cost_update;
    use vpsman_common::TunnelEndpointSide;

    #[test]
    fn parses_vty_tunnel_ospf_cost_update() {
        let request = parse_vty_tunnel_ospf_cost_update(&[
            "--plan-file",
            "/tmp/plan.json",
            "--side",
            "left",
            "--current-ospf-cost",
            "100",
            "--recommended-ospf-cost=50",
            "--force-unprivileged",
            "--confirmed",
        ])
        .unwrap();

        assert_eq!(request.side, TunnelEndpointSide::Left);
        assert_eq!(request.current_ospf_cost, 100);
        assert_eq!(request.recommended_ospf_cost, 50);
        assert!(request.force_unprivileged);
        assert!(parse_vty_tunnel_ospf_cost_update(&[
            "--plan-file",
            "/tmp/plan.json",
            "--side",
            "left",
            "--current-ospf-cost",
            "100",
            "--recommended-ospf-cost",
            "100",
            "--confirmed",
        ])
        .is_err());
    }
}
