use std::process::Stdio;

use anyhow::Result;
use tokio::process::Command;
use vpsman_common::{
    render_tunnel_endpoint_config, AgentConfig, AgentRuntimeStatusTelemetryPlan,
    AgentRuntimeTrafficSource, NetworkStat, RuntimeTunnelCommand,
};

use crate::{
    child_process::{run_child_with_bounded_output, ChildCleanupPolicy, ChildRunResult},
    network_runtime::render_runtime_adapter_command,
};

pub(crate) struct TrafficAccumulation {
    pub(crate) rx_bytes: u64,
    pub(crate) tx_bytes: u64,
    pub(crate) source: String,
    pub(crate) status: String,
    pub(crate) reason: Option<String>,
}

pub(crate) async fn traffic_accumulation_for_plan(
    config: &AgentConfig,
    telemetry_plan: &AgentRuntimeStatusTelemetryPlan,
    interface_counter: Option<NetworkStat>,
) -> TrafficAccumulation {
    match telemetry_plan.traffic_source {
        AgentRuntimeTrafficSource::InterfaceCounters => traffic_from_optional_counter(
            "interface_counters",
            &telemetry_plan.plan.interface_name,
            interface_counter,
        ),
        AgentRuntimeTrafficSource::Vnstat => {
            traffic_from_vnstat_preset(config, telemetry_plan).await
        }
        AgentRuntimeTrafficSource::CustomCommand => {
            traffic_from_custom_command(config, telemetry_plan).await
        }
    }
}

fn traffic_from_optional_counter(
    source: &str,
    interface: &str,
    counters: Option<NetworkStat>,
) -> TrafficAccumulation {
    if let Some(counters) = counters {
        return TrafficAccumulation {
            rx_bytes: counters.rx_bytes,
            tx_bytes: counters.tx_bytes,
            source: source.to_string(),
            status: "ok".to_string(),
            reason: None,
        };
    }
    TrafficAccumulation {
        rx_bytes: 0,
        tx_bytes: 0,
        source: source.to_string(),
        status: "missing".to_string(),
        reason: Some(format!("{interface}_not_found")),
    }
}

async fn traffic_from_vnstat_preset(
    config: &AgentConfig,
    telemetry_plan: &AgentRuntimeStatusTelemetryPlan,
) -> TrafficAccumulation {
    let command = telemetry_plan
        .traffic_command
        .clone()
        .or_else(|| vnstat_preset_command(config, &telemetry_plan.plan.interface_name));
    let Some(command) = command else {
        return TrafficAccumulation {
            rx_bytes: 0,
            tx_bytes: 0,
            source: "vnstat".to_string(),
            status: "unconfigured".to_string(),
            reason: Some("vnstat_source_not_configured".to_string()),
        };
    };
    traffic_from_command(config, telemetry_plan, &command, "vnstat").await
}

async fn traffic_from_custom_command(
    config: &AgentConfig,
    telemetry_plan: &AgentRuntimeStatusTelemetryPlan,
) -> TrafficAccumulation {
    let Some(command) = &telemetry_plan.traffic_command else {
        return TrafficAccumulation {
            rx_bytes: 0,
            tx_bytes: 0,
            source: "custom_command".to_string(),
            status: "unconfigured".to_string(),
            reason: Some("custom_traffic_command_missing".to_string()),
        };
    };
    traffic_from_command(config, telemetry_plan, command, "custom_command").await
}

fn vnstat_preset_command(config: &AgentConfig, interface: &str) -> Option<RuntimeTunnelCommand> {
    if config.network.runtime_vnstat_argv.is_empty() {
        return None;
    }
    let mut argv = config.network.runtime_vnstat_argv.clone();
    argv.extend([
        "--json".to_string(),
        "-i".to_string(),
        interface.to_string(),
    ]);
    Some(RuntimeTunnelCommand {
        argv,
        max_timeout_secs: 5,
        max_output_bytes: 16 * 1024,
    })
}

async fn traffic_from_command(
    config: &AgentConfig,
    telemetry_plan: &AgentRuntimeStatusTelemetryPlan,
    command: &RuntimeTunnelCommand,
    source: &str,
) -> TrafficAccumulation {
    let endpoint =
        match render_tunnel_endpoint_config(&telemetry_plan.plan, telemetry_plan.endpoint_side) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                return TrafficAccumulation {
                    rx_bytes: 0,
                    tx_bytes: 0,
                    source: source.to_string(),
                    status: "invalid".to_string(),
                    reason: Some(format!("endpoint_render_failed:{error}")),
                };
            }
        };
    let argv = match render_runtime_adapter_command(command, &telemetry_plan.plan, &endpoint) {
        Ok(argv) => argv,
        Err(error) => {
            return TrafficAccumulation {
                rx_bytes: 0,
                tx_bytes: 0,
                source: source.to_string(),
                status: "invalid".to_string(),
                reason: Some(format!("traffic_command_invalid:{error}")),
            };
        }
    };
    let max_timeout_secs = command
        .max_timeout_secs
        .min(config.network.runtime_command_timeout_secs)
        .clamp(1, 30);
    let max_output_bytes = usize::try_from(
        command
            .max_output_bytes
            .min(config.network.runtime_command_max_output_bytes)
            .clamp(1024, 64 * 1024),
    )
    .unwrap_or(16 * 1024);
    match run_traffic_command_json(&argv, max_timeout_secs, max_output_bytes).await {
        Ok(payload) => parse_traffic_json_payload(&payload, source),
        Err(error) => TrafficAccumulation {
            rx_bytes: 0,
            tx_bytes: 0,
            source: source.to_string(),
            status: "failed".to_string(),
            reason: Some(format!("traffic_command_failed:{error}")),
        },
    }
}

async fn run_traffic_command_json(
    argv: &[String],
    max_timeout_secs: u64,
    max_output_bytes: usize,
) -> Result<serde_json::Value> {
    if argv.is_empty() || !argv[0].starts_with('/') {
        anyhow::bail!("traffic telemetry executable must be absolute");
    }
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]);
    command.stdin(Stdio::null());
    let result = run_child_with_bounded_output(
        command,
        max_timeout_secs,
        max_output_bytes,
        ChildCleanupPolicy::ProcessGroup,
    )
    .await?;
    let output = match result {
        ChildRunResult::Completed(output) => {
            if output.stdout_truncated || output.stderr_truncated {
                anyhow::bail!("traffic telemetry output exceeded limit");
            }
            if output.exit_code != Some(0) {
                anyhow::bail!(
                    "traffic telemetry command exited with {:?}",
                    output.exit_code
                );
            }
            output.stdout
        }
        ChildRunResult::TimedOut(_) => anyhow::bail!("traffic telemetry timed out"),
        ChildRunResult::Canceled { reason, .. } => {
            anyhow::bail!("traffic telemetry canceled: {reason}")
        }
    };
    Ok(serde_json::from_slice(&output)?)
}

fn parse_traffic_json_payload(payload: &serde_json::Value, source: &str) -> TrafficAccumulation {
    if let Some((rx_bytes, tx_bytes)) = parse_flat_traffic_json(payload) {
        return TrafficAccumulation {
            rx_bytes,
            tx_bytes,
            source: source.to_string(),
            status: "ok".to_string(),
            reason: None,
        };
    }
    if let Some((rx_bytes, tx_bytes)) = parse_vnstat_preset_traffic_json(payload) {
        return TrafficAccumulation {
            rx_bytes,
            tx_bytes,
            source: source.to_string(),
            status: "ok".to_string(),
            reason: Some("vnstat_preset_total_rx_tx".to_string()),
        };
    }
    TrafficAccumulation {
        rx_bytes: 0,
        tx_bytes: 0,
        source: source.to_string(),
        status: "invalid_output".to_string(),
        reason: Some("missing_rx_bytes_tx_bytes".to_string()),
    }
}

fn parse_flat_traffic_json(payload: &serde_json::Value) -> Option<(u64, u64)> {
    Some((
        payload.get("rx_bytes")?.as_u64()?,
        payload.get("tx_bytes")?.as_u64()?,
    ))
}

fn parse_vnstat_preset_traffic_json(payload: &serde_json::Value) -> Option<(u64, u64)> {
    let total = payload
        .get("interfaces")?
        .as_array()?
        .first()?
        .get("traffic")?
        .get("total")?
        .as_array()?;
    let mut rx_bytes = 0_u64;
    let mut tx_bytes = 0_u64;
    for row in total {
        rx_bytes = rx_bytes.saturating_add(row.get("rx")?.as_u64()?);
        tx_bytes = tx_bytes.saturating_add(row.get("tx")?.as_u64()?);
    }
    Some((rx_bytes, tx_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_VNSTAT_PRESET_ARGV: &str = "/opt/vpsman/vnstat";

    #[test]
    fn vnstat_preset_uses_configured_base_argv() {
        let mut config = AgentConfig::default();
        config.network.runtime_vnstat_argv = vec![TEST_VNSTAT_PRESET_ARGV.to_string()];

        let command = vnstat_preset_command(&config, "ovpn42").unwrap();

        assert_eq!(
            command.argv,
            vec![
                TEST_VNSTAT_PRESET_ARGV.to_string(),
                "--json".to_string(),
                "-i".to_string(),
                "ovpn42".to_string()
            ]
        );
    }

    #[test]
    fn parses_flat_and_vnstat_preset_traffic_payloads() {
        let flat = serde_json::json!({
            "rx_bytes": 1234,
            "tx_bytes": 5678,
        });
        assert_eq!(parse_flat_traffic_json(&flat), Some((1234, 5678)));

        let vnstat_preset = serde_json::json!({
            "interfaces": [{
                "traffic": {
                    "total": [
                        { "rx": 100, "tx": 200 },
                        { "rx": 7, "tx": 9 }
                    ]
                }
            }]
        });
        assert_eq!(
            parse_vnstat_preset_traffic_json(&vnstat_preset),
            Some((107, 209))
        );
    }
}
