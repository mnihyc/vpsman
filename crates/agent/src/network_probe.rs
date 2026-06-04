use std::{path::Path, process::Stdio, time::Duration};

use anyhow::{Context, Result};
use tokio::{process::Command, time};
use vpsman_common::{
    payload_hash, render_tunnel_endpoint_config, AgentConfig, CommandOutput, OutputStream,
    TunnelEndpointSide, TunnelPlan,
};

const MAX_PING_OUTPUT_BYTES: usize = 16 * 1024;
const PRESET_PING_CANDIDATES: &[&str] = &["/bin/ping", "/usr/bin/ping"];

pub(crate) struct NetworkProbeInput<'a> {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) config: &'a AgentConfig,
    pub(crate) plan: &'a TunnelPlan,
    pub(crate) side: TunnelEndpointSide,
    pub(crate) count: u8,
    pub(crate) interval_ms: u16,
    pub(crate) timeout_secs: u64,
}

pub(crate) async fn execute_network_probe_command(
    input: NetworkProbeInput<'_>,
) -> Result<Vec<CommandOutput>> {
    time::timeout(
        Duration::from_secs(input.timeout_secs.max(1)),
        probe_network_plan(input),
    )
    .await
    .context("network probe timed out")?
}

async fn probe_network_plan(input: NetworkProbeInput<'_>) -> Result<Vec<CommandOutput>> {
    let endpoint = render_tunnel_endpoint_config(input.plan, input.side)
        .map_err(|error| anyhow::anyhow!("invalid tunnel endpoint config: {error}"))?;
    if endpoint.local_client_id != input.config.client_id {
        anyhow::bail!(
            "network probe side targets {}, but this agent is {}",
            endpoint.local_client_id,
            input.config.client_id
        );
    }
    let target = peer_tunnel_address(input.plan, input.side);
    let count = input.count.clamp(1, 20);
    let interval_ms = input.interval_ms.clamp(200, 10_000);
    let (mut ping_argv, command_source) = ping_base_argv(input.config)?;
    let count_arg = count.to_string();
    let interval_secs = format!("{:.3}", f64::from(interval_ms) / 1000.0);
    ping_argv.extend([
        "-n".to_string(),
        "-c".to_string(),
        count_arg,
        "-i".to_string(),
        interval_secs,
        "-W".to_string(),
        "2".to_string(),
        target.to_string(),
    ]);
    let command_sha256_hex = payload_hash(&serde_json::to_vec(&ping_argv).unwrap_or_default());
    let mut command = Command::new(&ping_argv[0]);
    command
        .args(&ping_argv[1..])
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command
        .output()
        .await
        .with_context(|| format!("failed to run latency probe to {target}"))?;
    let stdout = limit_bytes(output.stdout);
    let stderr = limit_bytes(output.stderr);
    let parsed = parse_ping_output(std::str::from_utf8(&stdout).unwrap_or_default());
    let status = serde_json::json!({
        "type": "network_probe",
        "probe": "icmp_ping",
        "plan": input.plan.name,
        "interface": input.plan.interface_name,
        "side": side_label(input.side),
        "client_id": input.config.client_id,
        "peer_client_id": endpoint.peer_client_id,
        "target": target,
        "count": count,
        "interval_ms": interval_ms,
        "command_source": command_source,
        "command_sha256_hex": command_sha256_hex,
        "exit_code": output.status.code(),
        "success": output.status.success(),
        "stdout_sha256_hex": payload_hash(&stdout),
        "stderr_sha256_hex": payload_hash(&stderr),
        "stdout_bytes": stdout.len(),
        "stderr_bytes": stderr.len(),
        "parsed": parsed,
    });
    Ok(vec![CommandOutput {
        job_id: input.job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: output.status.code(),
        done: true,
    }])
}

fn ping_base_argv(config: &AgentConfig) -> Result<(Vec<String>, &'static str)> {
    if !config.network.probe_ping_argv.is_empty() {
        return Ok((config.network.probe_ping_argv.clone(), "configured"));
    }
    for path in PRESET_PING_CANDIDATES {
        if Path::new(path).exists() {
            return Ok((vec![path.to_string()], "linux_ping_preset"));
        }
    }
    anyhow::bail!(
        "latency probe binary not found in configured argv or Linux preset candidates: {}",
        PRESET_PING_CANDIDATES.join(", ")
    )
}

fn peer_tunnel_address(plan: &TunnelPlan, side: TunnelEndpointSide) -> &str {
    match side {
        TunnelEndpointSide::Left => &plan.right_tunnel_address,
        TunnelEndpointSide::Right => &plan.left_tunnel_address,
    }
}

fn side_label(side: TunnelEndpointSide) -> &'static str {
    match side {
        TunnelEndpointSide::Left => "left",
        TunnelEndpointSide::Right => "right",
    }
}

fn limit_bytes(mut data: Vec<u8>) -> Vec<u8> {
    if data.len() > MAX_PING_OUTPUT_BYTES {
        data.truncate(MAX_PING_OUTPUT_BYTES);
    }
    data
}

fn parse_ping_output(stdout: &str) -> serde_json::Value {
    let mut transmitted = None::<u64>;
    let mut received = None::<u64>;
    let mut packet_loss_ratio = None::<f64>;
    let mut rtt_min_ms = None::<f64>;
    let mut rtt_avg_ms = None::<f64>;
    let mut rtt_max_ms = None::<f64>;
    let mut rtt_mdev_ms = None::<f64>;
    for line in stdout.lines() {
        if line.contains("packets transmitted") && line.contains("packet loss") {
            let parts = line.split(',').map(str::trim).collect::<Vec<_>>();
            transmitted = parts
                .first()
                .and_then(|part| part.split_whitespace().next())
                .and_then(|value| value.parse().ok());
            received = parts
                .get(1)
                .and_then(|part| part.split_whitespace().next())
                .and_then(|value| value.parse().ok());
            packet_loss_ratio = parts
                .iter()
                .find_map(|part| part.strip_suffix("% packet loss"))
                .and_then(|value| value.trim().parse::<f64>().ok())
                .map(|percent| percent / 100.0);
        }
        if let Some((_prefix, values)) = line.split_once(" = ") {
            let values = values.trim_end_matches(" ms");
            let samples = values
                .split('/')
                .filter_map(|value| value.parse::<f64>().ok())
                .collect::<Vec<_>>();
            if samples.len() >= 4 {
                rtt_min_ms = Some(samples[0]);
                rtt_avg_ms = Some(samples[1]);
                rtt_max_ms = Some(samples[2]);
                rtt_mdev_ms = Some(samples[3]);
            }
        }
    }
    serde_json::json!({
        "transmitted": transmitted,
        "received": received,
        "packet_loss_ratio": packet_loss_ratio,
        "latency_min_ms": rtt_min_ms,
        "latency_avg_ms": rtt_avg_ms,
        "latency_max_ms": rtt_max_ms,
        "latency_mdev_ms": rtt_mdev_ms,
        "healthy": received.unwrap_or(0) > 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use vpsman_common::{plan_tunnel, BandwidthTier, OspfCostPolicy, TunnelKind, TunnelPlanInput};

    const TEST_PROBE_WRAPPER: &str = "/opt/vpsman/ping-wrapper";

    #[test]
    fn parses_linux_ping_latency_and_loss() {
        let parsed = parse_ping_output(
            "3 packets transmitted, 2 received, 33.3333% packet loss, time 400ms\n\
             rtt min/avg/max/mdev = 10.100/12.300/14.500/1.200 ms\n",
        );

        assert_eq!(parsed["transmitted"], 3);
        assert_eq!(parsed["received"], 2);
        assert_eq!(parsed["healthy"], true);
        assert_eq!(parsed["packet_loss_ratio"], 0.333333);
        assert_eq!(parsed["latency_avg_ms"], 12.3);
    }

    #[test]
    fn uses_configured_ping_base_argv() {
        let mut config = AgentConfig::default();
        config.network.probe_ping_argv = vec![
            TEST_PROBE_WRAPPER.to_string(),
            "--tenant".to_string(),
            "edge".to_string(),
        ];

        let (argv, source) = ping_base_argv(&config).unwrap();

        assert_eq!(source, "configured");
        assert_eq!(
            argv,
            vec![
                TEST_PROBE_WRAPPER.to_string(),
                "--tenant".to_string(),
                "edge".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn rejects_probe_for_wrong_endpoint_side() {
        let plan = test_plan();
        let config = AgentConfig {
            client_id: "right-b".to_string(),
            display_name: "right-b".to_string(),
            ..AgentConfig::default()
        };

        let error = execute_network_probe_command(NetworkProbeInput {
            job_id: uuid::Uuid::new_v4(),
            config: &config,
            plan: &plan,
            side: TunnelEndpointSide::Left,
            count: 3,
            interval_ms: 500,
            timeout_secs: 1,
        })
        .await
        .unwrap_err();

        assert!(error.to_string().contains("side targets left-a"));
    }

    fn test_plan() -> TunnelPlan {
        plan_tunnel(&TunnelPlanInput {
            name: "left-right".to_string(),
            interface_name: "tunlr".to_string(),
            kind: TunnelKind::Gre,
            runtime_control: Default::default(),
            runtime_topology: Default::default(),
            left_client_id: "left-a".to_string(),
            right_client_id: "right-b".to_string(),
            left_underlay: "198.51.100.10".to_string(),
            right_underlay: "203.0.113.20".to_string(),
            address_pool_cidr: "10.255.0.0/30".to_string(),
            reserved_addresses: Vec::new(),
            bandwidth: BandwidthTier::M100,
            latency_ms: 15.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            ospf_policy: OspfCostPolicy::default(),
        })
        .unwrap()
    }
}
