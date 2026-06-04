use std::path::PathBuf;

use anyhow::{Context, Result};
use vpsman_common::{render_tunnel_endpoint_config, JobCommand, TunnelEndpointSide, TunnelPlan};

use crate::{
    http::http_post_json, proof::build_envelopes_for_job_command, vty_jobs::VtyProofContext,
};

#[derive(Debug, PartialEq)]
pub(crate) struct VtyTunnelProbeRequest {
    pub(crate) plan_file: PathBuf,
    pub(crate) side: TunnelEndpointSide,
    pub(crate) count: u8,
    pub(crate) interval_ms: u16,
    pub(crate) timeout_secs: u64,
    pub(crate) proof_ttl_secs: u64,
}

pub(crate) fn parse_vty_tunnel_probe(tokens: &[&str]) -> Result<VtyTunnelProbeRequest> {
    let mut plan_file = None::<PathBuf>;
    let mut side = None::<TunnelEndpointSide>;
    let mut count = 3_u8;
    let mut interval_ms = 500_u16;
    let mut timeout_secs = 60_u64;
    let mut proof_ttl_secs = 300_u64;

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
            "--proof-ttl" | "--proof-ttl-secs" => {
                proof_ttl_secs = parse_bounded_u64(
                    next_value(tokens, index, tokens[index])?,
                    tokens[index],
                    1,
                    3600,
                )?;
                index += 2;
            }
            value if value.starts_with("--proof-ttl=") => {
                proof_ttl_secs =
                    parse_bounded_u64(flag_value(value, "--proof-ttl="), "--proof-ttl", 1, 3600)?;
                index += 1;
            }
            value if value.starts_with("--proof-ttl-secs=") => {
                proof_ttl_secs = parse_bounded_u64(
                    flag_value(value, "--proof-ttl-secs="),
                    "--proof-ttl-secs",
                    1,
                    3600,
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
        timeout_secs,
        proof_ttl_secs,
    })
}

pub(crate) fn submit_vty_tunnel_probe(
    api_url: &str,
    token: Option<&str>,
    proof_context: &VtyProofContext,
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
    let (_payload_hash_hex, envelopes) = build_envelopes_for_job_command(
        std::slice::from_ref(&endpoint.local_client_id),
        &operation,
        &proof_context.password,
        &proof_context.salt_hex,
        request.proof_ttl_secs,
    )?;

    http_post_json(
        api_url,
        "/api/v1/jobs",
        token,
        &serde_json::json!({
            "command": "network_probe",
            "argv": [],
            "clients": [endpoint.local_client_id],
            "pools": [],
            "tags": [],
            "privileged": true,
            "destructive": false,
            "confirmed": false,
            "timeout_secs": request.timeout_secs,
            "operation": operation,
            "envelope": null,
            "envelopes": envelopes,
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
            "--timeout=120",
            "--proof-ttl",
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
        assert_eq!(request.timeout_secs, 120);
        assert_eq!(request.proof_ttl_secs, 90);
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
