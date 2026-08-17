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
    if command
        .argv
        .iter()
        .any(|part| part.contains("{display_name}") || part.contains("{tags_csv}"))
    {
        anyhow::bail!("custom telemetry argv contains removed server identity placeholder");
    }
    Ok(command
        .argv
        .iter()
        .map(|part| part.replace("{client_id}", &config.client_id))
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
            memory.total_bytes > 0
                && memory.available_bytes <= memory.total_bytes
                && memory.swap_total_bytes.is_some() == memory.swap_available_bytes.is_some()
                && memory
                    .swap_total_bytes
                    .zip(memory.swap_available_bytes)
                    .is_none_or(|(total, available)| available <= total),
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
        tunnel_reachability: Vec::new(),
        port_forwarding: None,
    }
}

#[cfg(test)]
#[path = "tests_telemetry_custom.rs"]
mod tests;
