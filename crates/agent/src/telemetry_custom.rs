use std::process::Stdio;

use anyhow::{Context, Result};
use serde::Deserialize;
use tokio::process::Command;
use tracing::warn;
use vpsman_common::{
    AgentConfig, AgentMetrics, AgentTelemetrySource, CpuStat, DiskStat, LoadAverage, MemoryStat,
    NetworkStat, RuntimeTunnelCommand, RuntimeTunnelStat,
};

use crate::child_process::{run_child_with_bounded_output, ChildCleanupPolicy, ChildRunResult};

#[derive(Debug, Default, Deserialize)]
struct CustomMetricsPatch {
    hostname: Option<String>,
    uptime_secs: Option<u64>,
    cpu: Option<CpuPatch>,
    memory: Option<MemoryStat>,
    disks: Option<Vec<DiskStat>>,
    networks: Option<Vec<NetworkStat>>,
    tunnels: Option<Vec<RuntimeTunnelStat>>,
}

#[derive(Debug, Default, Deserialize)]
struct CpuPatch {
    load: Option<LoadAverage>,
    cores: Option<u16>,
}

pub(crate) fn custom_metrics_replaces_linux(config: &AgentConfig) -> bool {
    config.telemetry.source == AgentTelemetrySource::CustomCommand
}

pub(crate) async fn apply_custom_metrics_if_configured(
    config: &AgentConfig,
    metrics: &mut AgentMetrics,
) {
    if !matches!(
        config.telemetry.source,
        AgentTelemetrySource::CustomCommand | AgentTelemetrySource::LinuxProcfsAndCustomCommand
    ) {
        return;
    }
    let Some(command) = &config.telemetry.custom_metrics_command else {
        return;
    };
    match run_custom_metrics_command(config, command).await {
        Ok(patch) => apply_patch(metrics, patch),
        Err(error) => warn!(
            error = %error,
            "custom telemetry source failed; keeping available metrics"
        ),
    }
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
    if let Some(hostname) = patch
        .hostname
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        metrics.hostname = hostname;
    }
    if let Some(uptime_secs) = patch.uptime_secs {
        metrics.uptime_secs = uptime_secs;
    }
    if let Some(cpu) = patch.cpu {
        if let Some(load) = cpu.load {
            metrics.cpu.load = load;
        }
        if let Some(cores) = cpu.cores.filter(|cores| *cores > 0) {
            metrics.cpu.cores = cores;
        }
    }
    if let Some(memory) = patch.memory {
        metrics.memory = memory;
    }
    if let Some(disks) = patch.disks {
        metrics.disks = disks.into_iter().take(128).collect();
    }
    if let Some(networks) = patch.networks {
        metrics.networks = networks.into_iter().take(512).collect();
    }
    if let Some(tunnels) = patch.tunnels {
        metrics.tunnels = tunnels.into_iter().take(128).collect();
    }
}

pub(crate) fn empty_custom_metrics_snapshot(observed_unix: u64) -> AgentMetrics {
    AgentMetrics {
        observed_unix,
        hostname: "unknown".to_string(),
        uptime_secs: 0,
        cpu: CpuStat {
            load: LoadAverage::default(),
            cores: 1,
        },
        memory: MemoryStat::default(),
        disks: Vec::new(),
        networks: Vec::new(),
        tunnels: Vec::new(),
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
}
