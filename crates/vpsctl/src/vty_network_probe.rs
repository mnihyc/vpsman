use std::path::PathBuf;

use anyhow::{Context, Result};
use uuid::Uuid;
use vpsman_common::{
    render_tunnel_endpoint_config, JobCommand, TunnelEndpointSide, TunnelPlan,
    MAX_CONFIGURABLE_JOB_TIMEOUT_SECS,
};

use crate::{
    commands_schedules::selector_expression_from_targets, http::http_post_json,
    privilege::build_privilege_for_job_command, vty_jobs::VtyPrivilegeContext,
};

#[derive(Debug, PartialEq)]
pub(crate) struct VtyTunnelProbeRequest {
    pub(crate) plan_file: PathBuf,
    pub(crate) side: TunnelEndpointSide,
    pub(crate) count: u8,
    pub(crate) interval_ms: u16,
    pub(crate) max_timeout_secs: u64,
    pub(crate) privilege_ttl_secs: u64,
}

pub(crate) fn parse_vty_tunnel_probe(tokens: &[&str]) -> Result<VtyTunnelProbeRequest> {
    let mut plan_file = None::<PathBuf>;
    let mut side = None::<TunnelEndpointSide>;
    let mut count = 3_u8;
    let mut interval_ms = 500_u16;
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
                side = Some(parse_probe_side(next_value(tokens, index, "--side")?)?);
                index += 2;
            }
            value if value.starts_with("--side=") => {
                side = Some(parse_probe_side(flag_value(value, "--side="))?);
                index += 1;
            }
            "--count" => {
                count = parse_bounded_u8(next_value(tokens, index, "--count")?, "--count", 1, 20)?;
                index += 2;
            }
            value if value.starts_with("--count=") => {
                count = parse_bounded_u8(flag_value(value, "--count="), "--count", 1, 20)?;
                index += 1;
            }
            "--interval-ms" => {
                interval_ms = parse_bounded_u16(
                    next_value(tokens, index, "--interval-ms")?,
                    "--interval-ms",
                    200,
                    10_000,
                )?;
                index += 2;
            }
            value if value.starts_with("--interval-ms=") => {
                interval_ms = parse_bounded_u16(
                    flag_value(value, "--interval-ms="),
                    "--interval-ms",
                    200,
                    10_000,
                )?;
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
            other => anyhow::bail!("unknown tunnel-probe flag {other}"),
        }
    }

    Ok(VtyTunnelProbeRequest {
        plan_file: required(plan_file, "--plan-file")?,
        side: required(side, "--side")?,
        count,
        interval_ms,
        max_timeout_secs,
        privilege_ttl_secs,
    })
}

pub(crate) fn submit_vty_tunnel_probe(
    api_url: &str,
    token: Option<&str>,
    privilege_context: &VtyPrivilegeContext,
    request: VtyTunnelProbeRequest,
) -> Result<String> {
    let plan_text = std::fs::read_to_string(&request.plan_file)
        .with_context(|| format!("failed to read tunnel plan {}", request.plan_file.display()))?;
    let plan: TunnelPlan =
        serde_json::from_str(&plan_text).context("tunnel plan JSON is invalid")?;
    let endpoint = render_tunnel_endpoint_config(&plan, request.side)?;
    let operation = JobCommand::NetworkProbe {
        plan: Box::new(plan),
        side: request.side,
        count: request.count,
        interval_ms: request.interval_ms,
    };
    let target_clients = vec![endpoint.local_client_id];
    let selector_expression = selector_expression_from_targets(&target_clients, &[]);
    let privilege = build_privilege_for_job_command(
        &target_clients,
        &operation,
        "network_probe",
        &selector_expression,
        &privilege_context.password,
        &privilege_context.salt_hex,
        request.privilege_ttl_secs,
        request.max_timeout_secs,
        false,
        true,
    )?;

    http_post_json(
        api_url,
        "/api/v1/jobs",
        token,
        &serde_json::json!({
            "job_id": Uuid::new_v4(),
            "command": "network_probe",
            "argv": [],
            "selector_expression": selector_expression,
            "target_client_ids": target_clients,
            "privileged": true,
            "destructive": false,
            "confirmed": false,
            "max_timeout_secs": request.max_timeout_secs,
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

fn parse_probe_side(value: &str) -> Result<TunnelEndpointSide> {
    match value {
        "left" => Ok(TunnelEndpointSide::Left),
        "right" => Ok(TunnelEndpointSide::Right),
        _ => anyhow::bail!("--side must be one of left, right"),
    }
}

fn parse_bounded_u8(value: &str, flag: &str, min: u8, max: u8) -> Result<u8> {
    let parsed = value
        .parse::<u8>()
        .with_context(|| format!("{flag} must be an integer"))?;
    anyhow::ensure!(
        (min..=max).contains(&parsed),
        "{flag} must be between {min} and {max}"
    );
    Ok(parsed)
}

fn parse_bounded_u16(value: &str, flag: &str, min: u16, max: u16) -> Result<u16> {
    let parsed = value
        .parse::<u16>()
        .with_context(|| format!("{flag} must be an integer"))?;
    anyhow::ensure!(
        (min..=max).contains(&parsed),
        "{flag} must be between {min} and {max}"
    );
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
    use super::parse_vty_tunnel_probe;
    use vpsman_common::TunnelEndpointSide;

    #[test]
    fn parses_vty_tunnel_probe_with_bounds() {
        let request = parse_vty_tunnel_probe(&[
            "--plan-file=/tmp/plan.json",
            "--side",
            "right",
            "--count=5",
            "--interval-ms",
            "750",
            "--max-timeout=120",
            "--privilege-ttl",
            "90",
        ])
        .unwrap();

        assert_eq!(
            request.plan_file,
            std::path::PathBuf::from("/tmp/plan.json")
        );
        assert_eq!(request.side, TunnelEndpointSide::Right);
        assert_eq!(request.count, 5);
        assert_eq!(request.interval_ms, 750);
        assert_eq!(request.max_timeout_secs, 120);
        assert_eq!(request.privilege_ttl_secs, 90);
    }

    #[test]
    fn rejects_vty_tunnel_probe_bad_bounds_or_side() {
        assert!(parse_vty_tunnel_probe(&[
            "--plan-file=/tmp/plan.json",
            "--side=left",
            "--count=0",
        ])
        .is_err());
        assert!(parse_vty_tunnel_probe(&[
            "--plan-file=/tmp/plan.json",
            "--side=left",
            "--interval-ms=199",
        ])
        .is_err());
        assert!(parse_vty_tunnel_probe(&["--plan-file=/tmp/plan.json", "--side=middle",]).is_err());
        assert!(parse_vty_tunnel_probe(&["--plan-file=/tmp/plan.json"]).is_err());
    }
}
