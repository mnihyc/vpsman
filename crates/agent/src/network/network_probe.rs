use std::{
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use tokio::{process::Command, time};
use vpsman_common::{
    payload_hash, render_tunnel_endpoint_config, AgentConfig, CommandOutput, OutputStream,
    TunnelEndpointSide, TunnelPlan,
};

use crate::{
    child_process::{run_child_with_bounded_output_cancelable, ChildCleanupPolicy, ChildRunResult},
    command_worker::{run_cancelable, CommandCancelToken, CommandCanceled},
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
    pub(crate) max_timeout_secs: u64,
    pub(crate) cancel_token: CommandCancelToken,
}

pub(crate) async fn execute_network_probe_command(
    input: NetworkProbeInput<'_>,
) -> Result<Vec<CommandOutput>> {
    let cancel_token = input.cancel_token.clone();
    run_cancelable("network_probe", cancel_token, async move {
        time::timeout(
            Duration::from_secs(input.max_timeout_secs.max(1)),
            probe_network_plan(input),
        )
        .await
        .context("network probe timed out")?
    })
    .await
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
    command.args(&ping_argv[1..]);
    let output = match run_child_with_bounded_output_cancelable(
        command,
        input.max_timeout_secs,
        MAX_PING_OUTPUT_BYTES,
        ChildCleanupPolicy::ProcessGroup,
        input.cancel_token,
    )
    .await
    .with_context(|| format!("failed to run latency probe to {target}"))?
    {
        ChildRunResult::Completed(output) => output,
        ChildRunResult::TimedOut(_) => anyhow::bail!("network probe timed out"),
        ChildRunResult::Canceled { reason, .. } => {
            return Err(CommandCanceled::new("network_probe", reason).into());
        }
    };
    let stdout = limit_bytes(output.stdout);
    let stderr = limit_bytes(output.stderr);
    let parsed = parse_ping_measurement(std::str::from_utf8(&stdout).unwrap_or_default());
    let healthy = output.exit_code == Some(0) && parsed.healthy;
    let reason = if healthy {
        None
    } else if output.exit_code != Some(0) {
        Some(format!("ping_exit:{:?}", output.exit_code))
    } else if parsed.transmitted == 0 {
        Some("ping_output_unparseable".to_string())
    } else if parsed.received == 0 {
        Some("no_reply".to_string())
    } else {
        Some("ping_measurement_incomplete".to_string())
    };
    let measured_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let status = serde_json::json!({
        "type": "tunnel_reachability",
        "source": "manual",
        "probe": "icmp_ping",
        "plan": input.plan.name,
        "interface": input.plan.interface_name,
        "side": side_label(input.side),
        "client_id": input.config.client_id,
        "peer_client_id": endpoint.peer_client_id,
        "target": target,
        "address_family": if target.contains(':') { "ipv6" } else { "ipv4" },
        "measured_unix": measured_unix,
        "stale_after_secs": (input.config.network.latency_monitoring_interval_secs.clamp(15, 3600) * 3).max(180),
        "count": count,
        "interval_ms": interval_ms,
        "command_source": command_source,
        "command_sha256_hex": command_sha256_hex,
        "exit_code": output.exit_code,
        "success": healthy,
        "healthy": healthy,
        "reason": reason,
        "transmitted": parsed.transmitted,
        "received": parsed.received,
        "packet_loss_ratio": parsed.packet_loss_ratio,
        "latency_min_ms": parsed.latency_min_ms,
        "latency_avg_ms": parsed.latency_avg_ms,
        "latency_max_ms": parsed.latency_max_ms,
        "latency_mdev_ms": parsed.latency_mdev_ms,
        "stdout_sha256_hex": payload_hash(&stdout),
        "stderr_sha256_hex": payload_hash(&stderr),
        "stdout_bytes": stdout.len(),
        "stderr_bytes": stderr.len(),
        "parsed": parsed.as_json(),
    });
    Ok(vec![CommandOutput {
        job_id: input.job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: output.exit_code,
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

#[derive(Clone, Debug)]
pub(crate) struct ParsedPingMeasurement {
    pub(crate) transmitted: u32,
    pub(crate) received: u32,
    pub(crate) packet_loss_ratio: f64,
    pub(crate) latency_min_ms: Option<f64>,
    pub(crate) latency_avg_ms: Option<f64>,
    pub(crate) latency_max_ms: Option<f64>,
    pub(crate) latency_mdev_ms: Option<f64>,
    pub(crate) healthy: bool,
}

impl Default for ParsedPingMeasurement {
    fn default() -> Self {
        Self {
            transmitted: 0,
            received: 0,
            packet_loss_ratio: 1.0,
            latency_min_ms: None,
            latency_avg_ms: None,
            latency_max_ms: None,
            latency_mdev_ms: None,
            healthy: false,
        }
    }
}

impl ParsedPingMeasurement {
    pub(crate) fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "transmitted": self.transmitted,
            "received": self.received,
            "packet_loss_ratio": self.packet_loss_ratio,
            "latency_min_ms": self.latency_min_ms,
            "latency_avg_ms": self.latency_avg_ms,
            "latency_max_ms": self.latency_max_ms,
            "latency_mdev_ms": self.latency_mdev_ms,
            "healthy": self.healthy,
        })
    }
}

pub(crate) fn parse_ping_measurement(stdout: &str) -> ParsedPingMeasurement {
    let mut parsed = ParsedPingMeasurement::default();
    for line in stdout.lines() {
        if line.contains("packets transmitted") && line.contains("packet loss") {
            let parts = line.split(',').map(str::trim).collect::<Vec<_>>();
            parsed.transmitted = parts
                .first()
                .and_then(|part| part.split_whitespace().next())
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            parsed.received = parts
                .get(1)
                .and_then(|part| part.split_whitespace().next())
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            parsed.packet_loss_ratio = parts
                .iter()
                .find_map(|part| part.strip_suffix("% packet loss"))
                .and_then(|value| value.trim().parse::<f64>().ok())
                .map(|percent| percent / 100.0)
                .unwrap_or_else(|| {
                    if parsed.transmitted == 0 {
                        1.0
                    } else {
                        1.0 - f64::from(parsed.received) / f64::from(parsed.transmitted)
                    }
                });
        }
        if let Some((_prefix, values)) = line.split_once(" = ") {
            let values = values.trim_end_matches(" ms");
            let samples = values
                .split('/')
                .filter_map(|value| value.parse::<f64>().ok())
                .collect::<Vec<_>>();
            if samples.len() >= 2 {
                parsed.latency_min_ms = Some(samples[0]);
                parsed.latency_avg_ms = Some(samples[1]);
                parsed.latency_max_ms = samples.get(2).copied();
                parsed.latency_mdev_ms = samples.get(3).copied();
            }
        }
    }
    parsed.healthy = parsed.received > 0 && parsed.latency_avg_ms.is_some();
    parsed
}

#[cfg(test)]
#[path = "tests_network_probe.rs"]
mod tests;
