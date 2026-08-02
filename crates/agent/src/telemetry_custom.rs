use std::{collections::HashSet, process::Stdio};

use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use tokio::process::Command;
use vpsman_common::{
    AgentConfig, AgentMetrics, AgentTelemetrySource, ConnectionStat, CpuStat, DiskStat,
    LoadAverage, MemoryStat, NetworkStat, RuntimeTunnelCommand, RuntimeTunnelStat,
    MAX_TELEMETRY_DISKS, MAX_TELEMETRY_NETWORKS, MAX_TELEMETRY_TUNNELS,
};

use crate::child_process::{run_child_with_bounded_output, ChildCleanupPolicy, ChildRunResult};

const MAX_CUSTOM_HOSTNAME_BYTES: usize = 255;
const MAX_CUSTOM_MOUNTPOINT_BYTES: usize = 4096;
const MAX_CUSTOM_INTERFACE_NAME_BYTES: usize = 64;
const MAX_CUSTOM_LOAD_AVERAGE: f64 = 1_000_000.0;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CustomMetricsPatch {
    hostname: Option<String>,
    uptime_secs: Option<u64>,
    cpu: Option<CpuPatch>,
    memory: Option<MemoryStat>,
    disks: Option<Vec<DiskStat>>,
    networks: Option<Vec<NetworkStat>>,
    connections: Option<ConnectionStat>,
    tunnels: Option<Vec<RuntimeTunnelStat>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CpuPatch {
    load: Option<LoadAverage>,
    cores: Option<u16>,
    utilization_ratio: Option<f64>,
}

pub(crate) fn custom_metrics_replaces_linux(config: &AgentConfig) -> bool {
    config.telemetry.source == AgentTelemetrySource::CustomCommand
}

pub(crate) async fn apply_custom_metrics_if_configured(
    config: &AgentConfig,
    metrics: &mut AgentMetrics,
) -> Result<()> {
    if !matches!(
        config.telemetry.source,
        AgentTelemetrySource::CustomCommand | AgentTelemetrySource::LinuxProcfsAndCustomCommand
    ) {
        return Ok(());
    }
    let command = config
        .telemetry
        .custom_metrics_command
        .as_ref()
        .context("custom telemetry source has no configured command")?;
    let patch = run_custom_metrics_command(config, command).await?;
    validate_custom_metrics_patch(&patch)?;
    if custom_metrics_replaces_linux(config) {
        validate_complete_custom_metrics_patch(&patch)?;
    }
    apply_patch(metrics, patch);
    Ok(())
}

async fn run_custom_metrics_command(
    config: &AgentConfig,
    command: &RuntimeTunnelCommand,
) -> Result<CustomMetricsPatch> {
    let argv = render_custom_metrics_argv(config, command)?;
    let mut child = Command::new(&argv[0]);
    child.args(&argv[1..]);
    child.stdin(Stdio::null());
    let result = run_child_with_bounded_output(
        child,
        command.max_timeout_secs.clamp(1, 30),
        command.max_output_bytes.clamp(1024, 64 * 1024) as usize,
        ChildCleanupPolicy::ProcessGroup,
    )
    .await
    .context("failed to run custom telemetry source")?;
    let output = match result {
        ChildRunResult::Completed(output) => {
            if output.stdout_truncated || output.stderr_truncated {
                anyhow::bail!("custom telemetry output exceeded limit");
            }
            if output.exit_code != Some(0) {
                anyhow::bail!("custom telemetry source exited with {:?}", output.exit_code);
            }
            output.stdout
        }
        ChildRunResult::TimedOut(_) => anyhow::bail!("custom telemetry source timed out"),
        ChildRunResult::Canceled { reason, .. } => {
            anyhow::bail!("custom telemetry source canceled: {reason}")
        }
    };
    serde_json::from_slice(&output).context("custom telemetry source returned invalid JSON")
}

fn render_custom_metrics_argv(
    config: &AgentConfig,
    command: &RuntimeTunnelCommand,
) -> Result<Vec<String>> {
    if command.argv.is_empty() {
        anyhow::bail!("custom telemetry argv is empty");
    }
    if !command.argv[0].starts_with('/') {
        anyhow::bail!("custom telemetry executable must be absolute");
    }
    Ok(command
        .argv
        .iter()
        .map(|part| {
            part.replace("{client_id}", &config.client_id)
                .replace("{display_name}", &config.display_name)
                .replace("{tags_csv}", &config.tags.join(","))
        })
        .collect())
}

fn apply_patch(metrics: &mut AgentMetrics, patch: CustomMetricsPatch) {
    if let Some(hostname) = patch.hostname {
        metrics.hostname = hostname.trim().to_string();
    }
    if let Some(uptime_secs) = patch.uptime_secs {
        metrics.uptime_secs = uptime_secs;
    }
    if let Some(cpu) = patch.cpu {
        if let Some(load) = cpu.load {
            metrics.cpu.load = load;
        }
        if let Some(cores) = cpu.cores {
            metrics.cpu.cores = cores;
        }
        if let Some(utilization_ratio) = cpu.utilization_ratio {
            metrics.cpu.utilization_ratio = Some(utilization_ratio);
        }
    }
    if let Some(memory) = patch.memory {
        metrics.memory = memory;
    }
    if let Some(disks) = patch.disks {
        metrics.disks = disks;
    }
    if let Some(networks) = patch.networks {
        metrics.networks = networks;
    }
    if let Some(connections) = patch.connections {
        metrics.connections = Some(connections);
    }
    if let Some(tunnels) = patch.tunnels {
        metrics.tunnels = tunnels;
    }
}

fn validate_custom_metrics_patch(patch: &CustomMetricsPatch) -> Result<()> {
    ensure!(
        patch.hostname.is_some()
            || patch.uptime_secs.is_some()
            || patch.cpu.is_some()
            || patch.memory.is_some()
            || patch.disks.is_some()
            || patch.networks.is_some()
            || patch.connections.is_some()
            || patch.tunnels.is_some(),
        "custom telemetry patch is empty"
    );
    if let Some(hostname) = patch.hostname.as_deref() {
        let normalized = hostname.trim();
        ensure!(
            !normalized.is_empty()
                && normalized.len() <= MAX_CUSTOM_HOSTNAME_BYTES
                && !hostname.chars().any(char::is_control),
            "custom telemetry patch has invalid hostname"
        );
    }
    if let Some(cpu) = patch.cpu.as_ref() {
        ensure!(
            cpu.load.is_some() || cpu.cores.is_some() || cpu.utilization_ratio.is_some(),
            "custom telemetry patch has an empty cpu override"
        );
        if let Some(load) = cpu.load.as_ref() {
            ensure!(
                [load.one, load.five, load.fifteen]
                    .into_iter()
                    .all(|value| {
                        value.is_finite() && (0.0..=MAX_CUSTOM_LOAD_AVERAGE).contains(&value)
                    }),
                "custom telemetry patch has invalid cpu.load"
            );
        }
        if let Some(cores) = cpu.cores {
            ensure!(cores > 0, "custom telemetry patch has invalid cpu.cores");
        }
        if let Some(utilization_ratio) = cpu.utilization_ratio {
            ensure!(
                utilization_ratio.is_finite() && (0.0..=1.0).contains(&utilization_ratio),
                "custom telemetry patch has invalid cpu.utilization_ratio"
            );
        }
    }
    if let Some(memory) = patch.memory.as_ref() {
        ensure!(
            memory.total_bytes > 0 && memory.available_bytes <= memory.total_bytes,
            "custom telemetry patch has invalid memory"
        );
    }
    if let Some(disks) = patch.disks.as_ref() {
        ensure!(
            disks.len() <= MAX_TELEMETRY_DISKS,
            "custom telemetry patch has too many disks"
        );
        ensure!(
            disks.iter().all(|disk| {
                !disk.mountpoint.is_empty()
                    && disk.mountpoint.len() <= MAX_CUSTOM_MOUNTPOINT_BYTES
                    && !disk.mountpoint.chars().any(char::is_control)
                    && disk.available_bytes <= disk.total_bytes
            }),
            "custom telemetry patch has an invalid disk"
        );
    }
    if let Some(networks) = patch.networks.as_ref() {
        ensure!(
            networks.len() <= MAX_TELEMETRY_NETWORKS,
            "custom telemetry patch has too many networks"
        );
        let mut interfaces = HashSet::new();
        ensure!(
            networks.iter().all(|network| {
                !network.interface.is_empty()
                    && network.interface.len() <= MAX_CUSTOM_INTERFACE_NAME_BYTES
                    && !network.interface.chars().any(char::is_control)
                    && interfaces.insert(network.interface.as_str())
            }),
            "custom telemetry patch has an invalid or duplicate network"
        );
    }
    if let Some(tunnels) = patch.tunnels.as_ref() {
        ensure!(
            tunnels.len() <= MAX_TELEMETRY_TUNNELS,
            "custom telemetry patch has too many tunnels"
        );
        ensure!(
            tunnels.iter().all(|tunnel| {
                !tunnel.interface.is_empty()
                    && tunnel.interface.len() <= MAX_CUSTOM_INTERFACE_NAME_BYTES
                    && !tunnel.interface.chars().any(char::is_control)
                    && tunnel
                        .latency_avg_ms
                        .is_none_or(|value| value.is_finite() && value >= 0.0)
                    && tunnel
                        .packet_loss_ratio
                        .is_none_or(|value| value.is_finite() && (0.0..=1.0).contains(&value))
            }),
            "custom telemetry patch has an invalid tunnel"
        );
    }
    Ok(())
}

fn validate_complete_custom_metrics_patch(patch: &CustomMetricsPatch) -> Result<()> {
    ensure!(
        patch
            .hostname
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty()),
        "custom telemetry replacement is missing hostname"
    );
    ensure!(
        patch.uptime_secs.is_some(),
        "custom telemetry replacement is missing uptime_secs"
    );
    let cpu = patch
        .cpu
        .as_ref()
        .context("custom telemetry replacement is missing cpu")?;
    cpu.load
        .as_ref()
        .context("custom telemetry replacement is missing cpu.load")?;
    ensure!(
        cpu.cores.is_some(),
        "custom telemetry replacement is missing cpu.cores"
    );
    patch
        .memory
        .as_ref()
        .context("custom telemetry replacement is missing memory")?;
    ensure!(
        patch.disks.is_some(),
        "custom telemetry replacement is missing disks"
    );
    ensure!(
        patch.networks.is_some(),
        "custom telemetry replacement is missing networks"
    );
    ensure!(
        patch.tunnels.is_some(),
        "custom telemetry replacement is missing tunnels"
    );
    Ok(())
}

pub(crate) fn empty_custom_metrics_snapshot(observed_unix: u64) -> AgentMetrics {
    AgentMetrics {
        observed_unix,
        hostname: "unknown".to_string(),
        uptime_secs: 0,
        cpu: CpuStat {
            load: LoadAverage::default(),
            cores: 1,
            utilization_ratio: None,
        },
        memory: MemoryStat::default(),
        disks: Vec::new(),
        networks: Vec::new(),
        connections: None,
        tunnels: Vec::new(),
        ping_results: Vec::new(),
        port_forwarding: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_custom_metrics_placeholders() {
        let config = AgentConfig {
            client_id: "edge-a".to_string(),
            display_name: "Edge A".to_string(),
            tags: vec!["bgp".to_string(), "lax".to_string()],
            ..AgentConfig::default()
        };
        let argv = render_custom_metrics_argv(
            &config,
            &RuntimeTunnelCommand {
                argv: vec![
                    "/opt/vpsman/metrics".to_string(),
                    "{client_id}".to_string(),
                    "{display_name}".to_string(),
                    "{tags_csv}".to_string(),
                ],
                ..RuntimeTunnelCommand::default()
            },
        )
        .unwrap();

        assert_eq!(
            argv,
            vec![
                "/opt/vpsman/metrics".to_string(),
                "edge-a".to_string(),
                "Edge A".to_string(),
                "bgp,lax".to_string()
            ]
        );
    }

    #[test]
    fn custom_patch_rejects_empty_and_invalid_overlay_values() {
        assert!(
            validate_custom_metrics_patch(&CustomMetricsPatch::default())
                .unwrap_err()
                .to_string()
                .contains("patch is empty")
        );

        let hostname_error = validate_custom_metrics_patch(&CustomMetricsPatch {
            hostname: Some(" \t ".to_string()),
            ..CustomMetricsPatch::default()
        })
        .unwrap_err()
        .to_string();
        assert!(hostname_error.contains("invalid hostname"));

        let cores_error = validate_custom_metrics_patch(&CustomMetricsPatch {
            cpu: Some(CpuPatch {
                cores: Some(0),
                ..CpuPatch::default()
            }),
            ..CustomMetricsPatch::default()
        })
        .unwrap_err()
        .to_string();
        assert!(cores_error.contains("invalid cpu.cores"));

        let load_error = validate_custom_metrics_patch(&CustomMetricsPatch {
            cpu: Some(CpuPatch {
                load: Some(LoadAverage {
                    one: -0.1,
                    five: 0.0,
                    fifteen: 0.0,
                }),
                cores: None,
                utilization_ratio: None,
            }),
            ..CustomMetricsPatch::default()
        })
        .unwrap_err()
        .to_string();
        assert!(load_error.contains("invalid cpu.load"));

        let memory_error = validate_custom_metrics_patch(&CustomMetricsPatch {
            memory: Some(MemoryStat {
                total_bytes: 100,
                available_bytes: 101,
            }),
            ..CustomMetricsPatch::default()
        })
        .unwrap_err()
        .to_string();
        assert!(memory_error.contains("invalid memory"));
    }

    #[test]
    fn custom_patch_rejects_over_cardinality_arrays() {
        let disks_error = validate_custom_metrics_patch(&CustomMetricsPatch {
            disks: Some(vec![DiskStat::default(); MAX_TELEMETRY_DISKS + 1]),
            ..CustomMetricsPatch::default()
        })
        .unwrap_err()
        .to_string();
        assert!(disks_error.contains("too many disks"));

        let networks_error = validate_custom_metrics_patch(&CustomMetricsPatch {
            networks: Some(vec![NetworkStat::default(); MAX_TELEMETRY_NETWORKS + 1]),
            ..CustomMetricsPatch::default()
        })
        .unwrap_err()
        .to_string();
        assert!(networks_error.contains("too many networks"));

        let tunnels_error = validate_custom_metrics_patch(&CustomMetricsPatch {
            tunnels: Some(vec![
                RuntimeTunnelStat::default();
                MAX_TELEMETRY_TUNNELS + 1
            ]),
            ..CustomMetricsPatch::default()
        })
        .unwrap_err()
        .to_string();
        assert!(tunnels_error.contains("too many tunnels"));
    }

    #[test]
    fn custom_patch_rejects_invalid_collection_rows() {
        let disk_error = validate_custom_metrics_patch(&CustomMetricsPatch {
            disks: Some(vec![DiskStat {
                mountpoint: "/".to_string(),
                total_bytes: 100,
                available_bytes: 101,
            }]),
            ..CustomMetricsPatch::default()
        })
        .unwrap_err()
        .to_string();
        assert!(disk_error.contains("invalid disk"));

        let network = NetworkStat {
            interface: "eth0".to_string(),
            rx_bytes: 1,
            tx_bytes: 2,
        };
        let network_error = validate_custom_metrics_patch(&CustomMetricsPatch {
            networks: Some(vec![network.clone(), network]),
            ..CustomMetricsPatch::default()
        })
        .unwrap_err()
        .to_string();
        assert!(network_error.contains("duplicate network"));

        let tunnel_error = validate_custom_metrics_patch(&CustomMetricsPatch {
            tunnels: Some(vec![RuntimeTunnelStat {
                interface: "wg0".to_string(),
                packet_loss_ratio: Some(1.1),
                ..RuntimeTunnelStat::default()
            }]),
            ..CustomMetricsPatch::default()
        })
        .unwrap_err()
        .to_string();
        assert!(tunnel_error.contains("invalid tunnel"));
    }

    #[test]
    fn custom_overlay_accepts_valid_partial_metrics() {
        validate_custom_metrics_patch(&CustomMetricsPatch {
            cpu: Some(CpuPatch {
                load: Some(LoadAverage {
                    one: 0.5,
                    five: 0.4,
                    fifteen: 0.3,
                }),
                cores: None,
                utilization_ratio: Some(0.25),
            }),
            networks: Some(Vec::new()),
            ..CustomMetricsPatch::default()
        })
        .unwrap();
    }

    #[test]
    fn custom_cpu_utilization_is_optional_but_must_be_a_ratio() {
        let snapshot = empty_custom_metrics_snapshot(1);
        assert_eq!(snapshot.cpu.utilization_ratio, None);

        let mut metrics = snapshot;
        apply_patch(
            &mut metrics,
            CustomMetricsPatch {
                cpu: Some(CpuPatch {
                    utilization_ratio: Some(0.75),
                    ..CpuPatch::default()
                }),
                ..CustomMetricsPatch::default()
            },
        );
        assert_eq!(metrics.cpu.utilization_ratio, Some(0.75));

        let error = validate_custom_metrics_patch(&CustomMetricsPatch {
            cpu: Some(CpuPatch {
                utilization_ratio: Some(1.01),
                ..CpuPatch::default()
            }),
            ..CustomMetricsPatch::default()
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("invalid cpu.utilization_ratio"));
    }

    #[test]
    fn custom_patch_rejects_unknown_fields() {
        let error = serde_json::from_str::<CustomMetricsPatch>(
            r#"{"hostname":"edge-a","unexpected_metric":1}"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn custom_replacement_adds_completeness_to_shared_validation() {
        let patch = CustomMetricsPatch {
            hostname: Some("edge-a".to_string()),
            ..CustomMetricsPatch::default()
        };
        validate_custom_metrics_patch(&patch).unwrap();
        let error = validate_complete_custom_metrics_patch(&patch)
            .unwrap_err()
            .to_string();
        assert!(error.contains("uptime_secs"));
    }
}
