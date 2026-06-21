use std::path::PathBuf;

use anyhow::{Context, Result};
use uuid::Uuid;
use vpsman_common::{
    render_tunnel_endpoint_config, JobCommand, TunnelEndpointSide, TunnelPlan,
    DEFAULT_MAX_COMMAND_TIMEOUT_SECS, MAX_CONFIGURABLE_COMMAND_TIMEOUT_SECS,
    NETWORK_SPEED_TEST_MAX_CONNECT_TIMEOUT_MS, NETWORK_SPEED_TEST_MAX_DURATION_SECS,
    NETWORK_SPEED_TEST_MAX_MAX_BYTES, NETWORK_SPEED_TEST_MAX_PORT,
    NETWORK_SPEED_TEST_MAX_RATE_LIMIT_KBPS, NETWORK_SPEED_TEST_MIN_CONNECT_TIMEOUT_MS,
    NETWORK_SPEED_TEST_MIN_DURATION_SECS, NETWORK_SPEED_TEST_MIN_MAX_BYTES,
    NETWORK_SPEED_TEST_MIN_PORT, NETWORK_SPEED_TEST_MIN_RATE_LIMIT_KBPS,
};

use crate::{
    commands_schedules::selector_expression_from_targets, http::http_post_json,
    privilege::build_privilege_for_job_command, vty_jobs::VtyPrivilegeContext,
};

#[derive(Debug, PartialEq)]
pub(crate) struct VtyTunnelSpeedTestRequest {
    pub(crate) plan_file: PathBuf,
    pub(crate) server_side: TunnelEndpointSide,
    pub(crate) duration_secs: u8,
    pub(crate) max_bytes: u64,
    pub(crate) rate_limit_kbps: u32,
    pub(crate) port: u16,
    pub(crate) connect_timeout_ms: u16,
    pub(crate) timeout_secs: u64,
    pub(crate) privilege_ttl_secs: u64,
    pub(crate) confirmed: bool,
}

pub(crate) fn parse_vty_tunnel_speed_test(tokens: &[&str]) -> Result<VtyTunnelSpeedTestRequest> {
    let mut plan_file = None::<PathBuf>;
    let mut server_side = None::<TunnelEndpointSide>;
    let mut duration_secs = 3_u8;
    let mut max_bytes = 16 * 1024 * 1024_u64;
    let mut rate_limit_kbps = 100_000_u32;
    let mut port = 5201_u16;
    let mut connect_timeout_ms = 5_000_u16;
    let mut timeout_secs = DEFAULT_MAX_COMMAND_TIMEOUT_SECS;
    let mut privilege_ttl_secs = 300_u64;
    let mut confirmed = false;

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
            "--server-side" => {
                server_side = Some(parse_side(next_value(tokens, index, "--server-side")?)?);
                index += 2;
            }
            value if value.starts_with("--server-side=") => {
                server_side = Some(parse_side(flag_value(value, "--server-side="))?);
                index += 1;
            }
            "--duration-secs" => {
                duration_secs = parse_bounded_u8(
                    next_value(tokens, index, "--duration-secs")?,
                    "--duration-secs",
                    NETWORK_SPEED_TEST_MIN_DURATION_SECS,
                    NETWORK_SPEED_TEST_MAX_DURATION_SECS,
                )?;
                index += 2;
            }
            value if value.starts_with("--duration-secs=") => {
                duration_secs = parse_bounded_u8(
                    flag_value(value, "--duration-secs="),
                    "--duration-secs",
                    NETWORK_SPEED_TEST_MIN_DURATION_SECS,
                    NETWORK_SPEED_TEST_MAX_DURATION_SECS,
                )?;
                index += 1;
            }
            "--max-bytes" => {
                max_bytes = parse_bounded_u64(
                    next_value(tokens, index, "--max-bytes")?,
                    "--max-bytes",
                    NETWORK_SPEED_TEST_MIN_MAX_BYTES,
                    NETWORK_SPEED_TEST_MAX_MAX_BYTES,
                )?;
                index += 2;
            }
            value if value.starts_with("--max-bytes=") => {
                max_bytes = parse_bounded_u64(
                    flag_value(value, "--max-bytes="),
                    "--max-bytes",
                    NETWORK_SPEED_TEST_MIN_MAX_BYTES,
                    NETWORK_SPEED_TEST_MAX_MAX_BYTES,
                )?;
                index += 1;
            }
            "--rate-limit-kbps" => {
                rate_limit_kbps = parse_bounded_u32(
                    next_value(tokens, index, "--rate-limit-kbps")?,
                    "--rate-limit-kbps",
                    NETWORK_SPEED_TEST_MIN_RATE_LIMIT_KBPS,
                    NETWORK_SPEED_TEST_MAX_RATE_LIMIT_KBPS,
                )?;
                index += 2;
            }
            value if value.starts_with("--rate-limit-kbps=") => {
                rate_limit_kbps = parse_bounded_u32(
                    flag_value(value, "--rate-limit-kbps="),
                    "--rate-limit-kbps",
                    NETWORK_SPEED_TEST_MIN_RATE_LIMIT_KBPS,
                    NETWORK_SPEED_TEST_MAX_RATE_LIMIT_KBPS,
                )?;
                index += 1;
            }
            "--port" => {
                port = parse_bounded_u16(
                    next_value(tokens, index, "--port")?,
                    "--port",
                    NETWORK_SPEED_TEST_MIN_PORT,
                    NETWORK_SPEED_TEST_MAX_PORT,
                )?;
                index += 2;
            }
            value if value.starts_with("--port=") => {
                port = parse_bounded_u16(
                    flag_value(value, "--port="),
                    "--port",
                    NETWORK_SPEED_TEST_MIN_PORT,
                    NETWORK_SPEED_TEST_MAX_PORT,
                )?;
                index += 1;
            }
            "--connect-timeout-ms" => {
                connect_timeout_ms = parse_bounded_u16(
                    next_value(tokens, index, "--connect-timeout-ms")?,
                    "--connect-timeout-ms",
                    NETWORK_SPEED_TEST_MIN_CONNECT_TIMEOUT_MS,
                    NETWORK_SPEED_TEST_MAX_CONNECT_TIMEOUT_MS,
                )?;
                index += 2;
            }
            value if value.starts_with("--connect-timeout-ms=") => {
                connect_timeout_ms = parse_bounded_u16(
                    flag_value(value, "--connect-timeout-ms="),
                    "--connect-timeout-ms",
                    NETWORK_SPEED_TEST_MIN_CONNECT_TIMEOUT_MS,
                    NETWORK_SPEED_TEST_MAX_CONNECT_TIMEOUT_MS,
                )?;
                index += 1;
            }
            "--timeout" | "--timeout-secs" => {
                timeout_secs = parse_bounded_u64(
                    next_value(tokens, index, tokens[index])?,
                    tokens[index],
                    1,
                    MAX_CONFIGURABLE_COMMAND_TIMEOUT_SECS,
                )?;
                index += 2;
            }
            value if value.starts_with("--timeout=") => {
                timeout_secs = parse_bounded_u64(
                    flag_value(value, "--timeout="),
                    "--timeout",
                    1,
                    MAX_CONFIGURABLE_COMMAND_TIMEOUT_SECS,
                )?;
                index += 1;
            }
            value if value.starts_with("--timeout-secs=") => {
                timeout_secs = parse_bounded_u64(
                    flag_value(value, "--timeout-secs="),
                    "--timeout-secs",
                    1,
                    MAX_CONFIGURABLE_COMMAND_TIMEOUT_SECS,
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
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            other => anyhow::bail!("unknown tunnel-speed-test flag {other}"),
        }
    }
    anyhow::ensure!(
        confirmed,
        "tunnel-speed-test requires --confirmed because it opens a listener and sends traffic"
    );

    Ok(VtyTunnelSpeedTestRequest {
        plan_file: required(plan_file, "--plan-file")?,
        server_side: required(server_side, "--server-side")?,
        duration_secs,
        max_bytes,
        rate_limit_kbps,
        port,
        connect_timeout_ms,
        timeout_secs,
        privilege_ttl_secs,
        confirmed,
    })
}

pub(crate) fn submit_vty_tunnel_speed_test(
    api_url: &str,
    token: Option<&str>,
    privilege_context: &VtyPrivilegeContext,
    request: VtyTunnelSpeedTestRequest,
) -> Result<String> {
    let plan_text = std::fs::read_to_string(&request.plan_file)
        .with_context(|| format!("failed to read tunnel plan {}", request.plan_file.display()))?;
    let plan: TunnelPlan =
        serde_json::from_str(&plan_text).context("tunnel plan JSON is invalid")?;
    let server_endpoint = render_tunnel_endpoint_config(&plan, request.server_side)?;
    let target_clients = vec![
        server_endpoint.local_client_id.clone(),
        server_endpoint.peer_client_id.clone(),
    ];
    let operation = JobCommand::NetworkSpeedTest {
        plan: Box::new(plan),
        server_side: request.server_side,
        duration_secs: request.duration_secs,
        max_bytes: request.max_bytes,
        rate_limit_kbps: request.rate_limit_kbps,
        port: request.port,
        connect_timeout_ms: request.connect_timeout_ms,
    };
    let selector_expression = selector_expression_from_targets(&target_clients, &[]);
    let privilege = build_privilege_for_job_command(
        &target_clients,
        &operation,
        "network_speed_test",
        &selector_expression,
        &privilege_context.password,
        &privilege_context.salt_hex,
        request.privilege_ttl_secs,
        request.timeout_secs,
        false,
        true,
    )?;

    http_post_json(
        api_url,
        "/api/v1/jobs",
        token,
        &serde_json::json!({
            "job_id": Uuid::new_v4(),
            "command": "network_speed_test",
            "argv": [],
            "selector_expression": selector_expression,
            "target_client_ids": target_clients,
            "privileged": true,
            "destructive": false,
            "confirmed": request.confirmed,
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

fn parse_side(value: &str) -> Result<TunnelEndpointSide> {
    match value {
        "left" => Ok(TunnelEndpointSide::Left),
        "right" => Ok(TunnelEndpointSide::Right),
        _ => anyhow::bail!("--server-side must be one of left, right"),
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

fn parse_bounded_u32(value: &str, flag: &str, min: u32, max: u32) -> Result<u32> {
    let parsed = value
        .parse::<u32>()
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
    use super::parse_vty_tunnel_speed_test;
    use vpsman_common::TunnelEndpointSide;

    #[test]
    fn parses_vty_tunnel_speed_test_with_bounds() {
        let request = parse_vty_tunnel_speed_test(&[
            "--plan-file=/tmp/plan.json",
            "--server-side",
            "right",
            "--duration-secs=5",
            "--max-bytes",
            "1048576",
            "--rate-limit-kbps=10000",
            "--port",
            "55201",
            "--connect-timeout-ms=2500",
            "--timeout=120",
            "--privilege-ttl",
            "90",
            "--confirmed",
        ])
        .unwrap();

        assert_eq!(
            request.plan_file,
            std::path::PathBuf::from("/tmp/plan.json")
        );
        assert_eq!(request.server_side, TunnelEndpointSide::Right);
        assert_eq!(request.duration_secs, 5);
        assert_eq!(request.max_bytes, 1_048_576);
        assert_eq!(request.rate_limit_kbps, 10_000);
        assert_eq!(request.port, 55_201);
        assert_eq!(request.connect_timeout_ms, 2500);
        assert_eq!(request.timeout_secs, 120);
        assert_eq!(request.privilege_ttl_secs, 90);
        assert!(request.confirmed);
    }

    #[test]
    fn rejects_vty_tunnel_speed_test_bad_bounds_or_side() {
        assert!(parse_vty_tunnel_speed_test(&[
            "--plan-file=/tmp/plan.json",
            "--server-side=left",
            "--duration-secs=0",
        ])
        .is_err());
        assert!(parse_vty_tunnel_speed_test(&[
            "--plan-file=/tmp/plan.json",
            "--server-side=left",
            "--max-bytes=1",
        ])
        .is_err());
        assert!(parse_vty_tunnel_speed_test(&[
            "--plan-file=/tmp/plan.json",
            "--server-side=middle",
            "--confirmed",
        ])
        .is_err());
        assert!(parse_vty_tunnel_speed_test(&["--plan-file=/tmp/plan.json"]).is_err());
        assert!(parse_vty_tunnel_speed_test(
            &["--plan-file=/tmp/plan.json", "--server-side=left",]
        )
        .is_err());
    }
}
