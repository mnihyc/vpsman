use std::{
    collections::{HashMap, HashSet},
    ffi::CString,
    path::Path,
    process::{ExitStatus, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    task::JoinHandle,
    time::{self, Duration, Instant},
};
use vpsman_common::{
    render_tunnel_endpoint_config, AgentConfig, AgentMetrics, AgentRuntimeStatusTelemetryPlan,
    CpuStat, DiskStat, LoadAverage, MemoryStat, NetworkStat, RuntimeTunnelAdapterHealthStat,
    RuntimeTunnelManager, RuntimeTunnelStat, TunnelAddressFamily, TunnelEndpointSide, TunnelKind,
    MAX_TELEMETRY_DISKS, MAX_TELEMETRY_NETWORKS, MAX_TELEMETRY_TUNNELS,
};

use crate::child_process::{run_child_with_bounded_output, ChildCleanupPolicy, ChildRunResult};
use crate::network_runtime::render_runtime_adapter_command;
use crate::port_forwarding::inspect_port_forwarding;
use crate::telemetry_custom::{
    apply_custom_metrics_if_configured, custom_metrics_replaces_linux,
    empty_custom_metrics_snapshot,
};
use crate::telemetry_traffic::traffic_accumulation_for_plan;

const MAX_LATENCY_PROBE_OUTPUT_BYTES: usize = 16 * 1024;

#[derive(Default)]
pub(crate) struct TelemetryRuntimeState {
    last_adapter_check_unix: HashMap<String, u64>,
    cached_adapter_tunnels: HashMap<String, RuntimeTunnelStat>,
    latency_monitors: HashMap<String, LatencyMonitorState>,
}

#[derive(Clone, Debug, Default)]
struct LatencyMonitorState {
    healthy_windows: u8,
    missed_windows: u8,
}

fn collect_linux_metrics(config: &AgentConfig) -> Result<AgentMetrics> {
    let proc_root = Path::new(&config.telemetry.proc_root);
    let networks = network_stats(proc_root)?;
    let cores = std::thread::available_parallelism()
        .context("failed to determine available CPU cores")?
        .get();
    Ok(AgentMetrics {
        observed_unix: unix_now(),
        hostname: hostname(config)?,
        uptime_secs: uptime_secs(proc_root)?,
        cpu: CpuStat {
            load: load_average(proc_root)?,
            cores: u16::try_from(cores)
                .context("available CPU core count exceeds protocol range")?,
        },
        memory: memory_stat(proc_root)?,
        disks: disk_stats(proc_root)?,
        networks,
        tunnels: Vec::new(),
        port_forwarding: None,
    })
}

pub(crate) async fn collect_metrics_for_config(
    config: &AgentConfig,
    runtime_state: &mut TelemetryRuntimeState,
) -> Result<AgentMetrics> {
    let mut metrics = if custom_metrics_replaces_linux(config) {
        empty_custom_metrics_snapshot(unix_now())
    } else {
        collect_linux_metrics(config)?
    };
    apply_custom_metrics_if_configured(config, &mut metrics).await?;
    let reserved_runtime_tunnels = config
        .network
        .runtime_status_telemetry_plans
        .len()
        .min(MAX_TELEMETRY_TUNNELS);
    metrics
        .tunnels
        .truncate(MAX_TELEMETRY_TUNNELS - reserved_runtime_tunnels);
    collect_runtime_status_telemetry(config, &mut metrics, runtime_state).await;
    metrics.port_forwarding = Some(inspect_port_forwarding(&config.network.port_forwarding).await);
    metrics.disks.truncate(MAX_TELEMETRY_DISKS);
    metrics.networks.truncate(MAX_TELEMETRY_NETWORKS);
    metrics.tunnels.truncate(MAX_TELEMETRY_TUNNELS);
    Ok(metrics)
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn read_proc_file(proc_root: &Path, relative_path: &str) -> Result<String> {
    let path = proc_root.join(relative_path);
    std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read telemetry source {}", path.display()))
}

fn hostname(config: &AgentConfig) -> Result<String> {
    resolve_hostname(
        config.telemetry.hostname_file.as_deref(),
        |path| std::fs::read_to_string(path),
        read_system_hostname,
    )
}

fn resolve_hostname<ReadFile, DefaultHostname>(
    configured_path: Option<&str>,
    read_file: ReadFile,
    default_hostname: DefaultHostname,
) -> Result<String>
where
    ReadFile: FnOnce(&str) -> std::io::Result<String>,
    DefaultHostname: FnOnce() -> Result<String>,
{
    let (value, source) = if let Some(path) = configured_path {
        let value = read_file(path)
            .with_context(|| format!("failed to read configured hostname file {path}"))?;
        (value, format!("configured hostname file {path}"))
    } else {
        (
            default_hostname().context("failed to read operating system hostname")?,
            "operating system hostname".to_string(),
        )
    };
    let value = value.trim();
    ensure!(!value.is_empty(), "{source} is empty");
    Ok(value.to_string())
}

fn read_system_hostname() -> Result<String> {
    let mut hostname = [0_u8; 256];
    let result =
        unsafe { libc::gethostname(hostname.as_mut_ptr().cast::<libc::c_char>(), hostname.len()) };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).context("gethostname failed");
    }
    let length = hostname
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(hostname.len());
    String::from_utf8(hostname[..length].to_vec()).context("operating system hostname is not UTF-8")
}

fn uptime_secs(proc_root: &Path) -> Result<u64> {
    let contents = read_proc_file(proc_root, "uptime")?;
    let first = contents
        .split_whitespace()
        .next()
        .context("telemetry uptime source is empty")?;
    let value = first
        .parse::<f64>()
        .context("telemetry uptime is not numeric")?;
    ensure!(
        value.is_finite() && value >= 0.0 && value <= u64::MAX as f64,
        "telemetry uptime is out of range"
    );
    Ok(value as u64)
}

fn load_average(proc_root: &Path) -> Result<LoadAverage> {
    let contents = read_proc_file(proc_root, "loadavg")?;
    let mut fields = contents.split_whitespace();
    let parse_field = |field: Option<&str>, label: &str| -> Result<f64> {
        let value = field
            .with_context(|| format!("telemetry load average is missing {label}"))?
            .parse::<f64>()
            .with_context(|| format!("telemetry load average {label} is not numeric"))?;
        ensure!(
            value.is_finite() && value >= 0.0,
            "telemetry load average {label} is out of range"
        );
        Ok(value)
    };
    Ok(LoadAverage {
        one: parse_field(fields.next(), "one-minute value")?,
        five: parse_field(fields.next(), "five-minute value")?,
        fifteen: parse_field(fields.next(), "fifteen-minute value")?,
    })
}

fn memory_stat(proc_root: &Path) -> Result<MemoryStat> {
    let contents = read_proc_file(proc_root, "meminfo")?;
    let mut total = None;
    let mut available = None;

    for line in contents.lines() {
        let mut fields = line.split_whitespace();
        let Some(key) = fields.next() else {
            continue;
        };
        match key {
            "MemTotal:" | "MemAvailable:" => {
                let value = fields
                    .next()
                    .with_context(|| format!("telemetry {key} value is missing"))?
                    .parse::<u64>()
                    .with_context(|| format!("telemetry {key} value is not numeric"))?;
                ensure!(
                    fields.next() == Some("kB"),
                    "telemetry {key} unit is not kB"
                );
                let value = value
                    .checked_mul(1024)
                    .with_context(|| format!("telemetry {key} value is out of range"))?;
                if key == "MemTotal:" {
                    total = Some(value);
                } else {
                    available = Some(value);
                }
            }
            _ => {}
        }
    }

    let total_bytes = total.context("telemetry meminfo is missing MemTotal")?;
    let available_bytes = available.context("telemetry meminfo is missing MemAvailable")?;
    ensure!(
        total_bytes > 0 && available_bytes <= total_bytes,
        "telemetry memory values are inconsistent"
    );
    Ok(MemoryStat {
        total_bytes,
        available_bytes,
    })
}

fn network_stats(proc_root: &Path) -> Result<Vec<NetworkStat>> {
    let contents = read_proc_file(proc_root, "net/dev")?;
    network_stats_from_proc_net_dev(&contents)
}

fn network_stats_from_proc_net_dev(contents: &str) -> Result<Vec<NetworkStat>> {
    let mut lines = contents.lines();
    ensure!(
        lines.next().is_some_and(|line| line.contains("Receive")),
        "telemetry network source is missing its receive header"
    );
    ensure!(
        lines.next().is_some_and(|line| line.contains("bytes")),
        "telemetry network source is missing its counter header"
    );
    let mut stats = Vec::new();

    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let (name, counters) = line
            .split_once(':')
            .context("telemetry network row is missing ':'")?;
        let name = name.trim();
        ensure!(
            !name.is_empty(),
            "telemetry network interface name is empty"
        );
        let fields: Vec<&str> = counters.split_whitespace().collect();
        ensure!(
            fields.len() >= 16,
            "telemetry network row for {name} has incomplete counters"
        );
        stats.push(NetworkStat {
            interface: name.to_string(),
            rx_bytes: fields[0]
                .parse()
                .with_context(|| format!("telemetry RX counter for {name} is not numeric"))?,
            tx_bytes: fields[8]
                .parse()
                .with_context(|| format!("telemetry TX counter for {name} is not numeric"))?,
        });
        if stats.len() == MAX_TELEMETRY_NETWORKS {
            break;
        }
    }

    ensure!(
        !stats.is_empty(),
        "telemetry network source contains no interface counters"
    );
    Ok(stats)
}

async fn collect_runtime_status_telemetry(
    config: &AgentConfig,
    metrics: &mut AgentMetrics,
    runtime_state: &mut TelemetryRuntimeState,
) {
    if !config.network.runtime_status_telemetry_enabled {
        runtime_state.cached_adapter_tunnels.clear();
        runtime_state.last_adapter_check_unix.clear();
        runtime_state.latency_monitors.clear();
        return;
    }
    let now = metrics.observed_unix;
    let status_interval = config
        .network
        .runtime_status_telemetry_interval_secs
        .clamp(15, 3600);
    let latency_interval = config
        .network
        .latency_monitoring_interval_secs
        .clamp(15, 3600);
    let interval = if config.network.latency_monitoring_enabled {
        status_interval.min(latency_interval)
    } else {
        status_interval
    };
    for telemetry_plan in &config.network.runtime_status_telemetry_plans {
        let key = runtime_status_telemetry_key(telemetry_plan);
        let due = runtime_state
            .last_adapter_check_unix
            .get(&key)
            .is_none_or(|last| now.saturating_sub(*last) >= interval);
        if due {
            let interface_counter = metrics
                .networks
                .iter()
                .find(|stat| stat.interface == telemetry_plan.plan.interface_name)
                .cloned();
            let stat = runtime_status_telemetry_stat(
                config,
                telemetry_plan,
                now,
                interface_counter,
                runtime_state,
                &key,
            )
            .await;
            runtime_state
                .last_adapter_check_unix
                .insert(key.clone(), now);
            runtime_state
                .cached_adapter_tunnels
                .insert(key.clone(), stat.clone());
            merge_runtime_status_tunnel(metrics, stat);
        } else if let Some(stat) = runtime_state.cached_adapter_tunnels.get(&key) {
            merge_runtime_status_tunnel(metrics, stat.clone());
        }
    }
}

async fn runtime_status_telemetry_stat(
    config: &AgentConfig,
    telemetry_plan: &AgentRuntimeStatusTelemetryPlan,
    now: u64,
    interface_counter: Option<NetworkStat>,
    runtime_state: &mut TelemetryRuntimeState,
    key: &str,
) -> RuntimeTunnelStat {
    let plan = &telemetry_plan.plan;
    let manager = runtime_manager_label(plan.runtime_control.manager);
    let mut stat = RuntimeTunnelStat {
        interface: plan.interface_name.clone(),
        kind: tunnel_kind_label(plan.kind).to_string(),
        ownership_mode: manager.to_string(),
        mutation_policy: "managed_desired".to_string(),
        source: "approved_runtime_status_telemetry".to_string(),
        rx_bytes: 0,
        tx_bytes: 0,
        plan_id: telemetry_plan.plan_id.clone(),
        plan_name: Some(plan.name.clone()),
        plan_runtime_manager: Some(manager.to_string()),
        endpoint_side: Some(endpoint_side_label(telemetry_plan.endpoint_side).to_string()),
        peer_client_id: Some(peer_client_id(plan, telemetry_plan.endpoint_side).to_string()),
        ..RuntimeTunnelStat::default()
    };
    let traffic = traffic_accumulation_for_plan(config, telemetry_plan, interface_counter).await;
    stat.rx_bytes = traffic.rx_bytes;
    stat.tx_bytes = traffic.tx_bytes;
    stat.traffic_source = Some(traffic.source);
    stat.traffic_status = Some(traffic.status);
    stat.traffic_reason = traffic.reason;
    stat.traffic_checked_unix = Some(now);
    stat.adapter_health = Some(match plan.runtime_control.manager {
        RuntimeTunnelManager::ExternalManagedAdapter => {
            adapter_health_for_plan(config, telemetry_plan, now).await
        }
        RuntimeTunnelManager::AgentIproute2Managed => {
            skipped_adapter_health("agent_iproute2_managed", now, "agent_iproute2_managed")
        }
        RuntimeTunnelManager::ExternalObserved => {
            stat.mutation_policy = "observe_only_saved_plan".to_string();
            skipped_adapter_health("external_observed", now, "external_observed")
        }
    });
    apply_latency_monitoring(config, telemetry_plan, now, key, runtime_state, &mut stat).await;
    stat
}

async fn adapter_health_for_plan(
    config: &AgentConfig,
    telemetry_plan: &AgentRuntimeStatusTelemetryPlan,
    now: u64,
) -> RuntimeTunnelAdapterHealthStat {
    let plan = &telemetry_plan.plan;
    let Some(adapter) = &telemetry_plan.runtime_adapter else {
        return RuntimeTunnelAdapterHealthStat {
            status: "unconfigured".to_string(),
            checked_unix: now,
            configured: false,
            reason: Some("adapter_snapshot_unconfigured".to_string()),
            ..RuntimeTunnelAdapterHealthStat::default()
        };
    };
    let command = &adapter.status;
    let endpoint = match render_tunnel_endpoint_config(plan, telemetry_plan.endpoint_side) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            return RuntimeTunnelAdapterHealthStat {
                status: "invalid".to_string(),
                checked_unix: now,
                configured: true,
                reason: Some(format!("endpoint_render_failed:{error}")),
                ..RuntimeTunnelAdapterHealthStat::default()
            };
        }
    };
    let argv = match render_runtime_adapter_command(command, plan, &endpoint) {
        Ok(argv) => argv,
        Err(error) => {
            return RuntimeTunnelAdapterHealthStat {
                status: "invalid".to_string(),
                checked_unix: now,
                configured: true,
                reason: Some(format!("adapter_status_command_invalid:{error}")),
                ..RuntimeTunnelAdapterHealthStat::default()
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
    match run_adapter_status_telemetry(&argv, max_timeout_secs, max_output_bytes, now).await {
        Ok(health) => health,
        Err(error) => RuntimeTunnelAdapterHealthStat {
            status: "failed".to_string(),
            checked_unix: now,
            configured: true,
            command_sha256_hex: Some(sha256_hex(&serde_json::to_vec(&argv).unwrap_or_default())),
            reason: Some(format!("adapter_status_spawn_failed:{error}")),
            ..RuntimeTunnelAdapterHealthStat::default()
        },
    }
}

#[derive(Clone, Debug)]
struct LatencyProbeResult {
    family: TunnelAddressFamily,
    target: String,
    healthy: bool,
    latency_avg_ms: Option<f64>,
    packet_loss_ratio: Option<f64>,
    reason: Option<String>,
}

impl LatencyProbeResult {
    fn family_name(&self) -> &'static str {
        match self.family {
            TunnelAddressFamily::Ipv4 => "ipv4",
            TunnelAddressFamily::Ipv6 => "ipv6",
        }
    }
}

#[derive(Clone, Debug)]
struct LatencyTarget {
    family: TunnelAddressFamily,
    target: String,
    fallback: Option<(TunnelAddressFamily, String)>,
}

fn latency_targets(
    plan: &vpsman_common::TunnelPlan,
    side: TunnelEndpointSide,
) -> Option<LatencyTarget> {
    let primary = plan.latency_primary_family;
    let ipv4 = plan.ipv4_tunnel.as_ref().map(|pair| {
        (
            TunnelAddressFamily::Ipv4,
            remote_for_side(pair, side).to_string(),
        )
    });
    let ipv6 = plan.ipv6_tunnel.as_ref().map(|pair| {
        (
            TunnelAddressFamily::Ipv6,
            remote_for_side(pair, side).to_string(),
        )
    });
    match primary {
        TunnelAddressFamily::Ipv4 => match (ipv4, ipv6) {
            (Some((family, target)), fallback) => Some(LatencyTarget {
                family,
                target,
                fallback,
            }),
            (None, Some((family, target))) => Some(LatencyTarget {
                family,
                target,
                fallback: None,
            }),
            (None, None) => None,
        },
        TunnelAddressFamily::Ipv6 => match (ipv4, ipv6) {
            (fallback, Some((family, target))) => Some(LatencyTarget {
                family,
                target,
                fallback,
            }),
            (Some((family, target)), None) => Some(LatencyTarget {
                family,
                target,
                fallback: None,
            }),
            (None, None) => None,
        },
    }
}

fn remote_for_side(pair: &vpsman_common::TunnelAddressPair, side: TunnelEndpointSide) -> &str {
    match side {
        TunnelEndpointSide::Left => &pair.right,
        TunnelEndpointSide::Right => &pair.left,
    }
}

async fn run_latency_probe(
    config: &AgentConfig,
    family: TunnelAddressFamily,
    target: &str,
) -> Result<LatencyProbeResult> {
    let (mut argv, source) = latency_ping_base_argv(config)?;
    if source == "linux_ping_preset" {
        argv.push(match family {
            TunnelAddressFamily::Ipv4 => "-4".to_string(),
            TunnelAddressFamily::Ipv6 => "-6".to_string(),
        });
    }
    argv.extend([
        "-n".to_string(),
        "-c".to_string(),
        "3".to_string(),
        "-i".to_string(),
        "0.500".to_string(),
        "-W".to_string(),
        "2".to_string(),
        target.to_string(),
    ]);
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]).stdin(Stdio::null());
    let result = run_child_with_bounded_output(
        command,
        10,
        MAX_LATENCY_PROBE_OUTPUT_BYTES,
        ChildCleanupPolicy::ProcessGroup,
    )
    .await
    .context("failed to run latency probe")?;
    match result {
        ChildRunResult::Completed(output) => {
            let output_limited = output.stdout_truncated || output.stderr_truncated;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let parsed = parse_latency_ping_output(&stdout);
            Ok(LatencyProbeResult {
                family,
                target: target.to_string(),
                healthy: parsed.healthy && !output_limited && output.exit_code == Some(0),
                latency_avg_ms: parsed.latency_avg_ms,
                packet_loss_ratio: parsed.packet_loss_ratio,
                reason: if output_limited {
                    Some(format!("latency_probe_output_limit:{source}"))
                } else if output.exit_code != Some(0) {
                    Some(format!(
                        "latency_probe_exit:{:?}:{source}",
                        output.exit_code
                    ))
                } else {
                    None
                },
            })
        }
        ChildRunResult::TimedOut(_) => Ok(LatencyProbeResult {
            family,
            target: target.to_string(),
            healthy: false,
            latency_avg_ms: None,
            packet_loss_ratio: None,
            reason: Some(format!("latency_probe_timeout:{source}")),
        }),
        ChildRunResult::Canceled { reason, .. } => Ok(LatencyProbeResult {
            family,
            target: target.to_string(),
            healthy: false,
            latency_avg_ms: None,
            packet_loss_ratio: None,
            reason: Some(format!("latency_probe_canceled:{source}:{reason}")),
        }),
    }
}

fn latency_ping_base_argv(config: &AgentConfig) -> Result<(Vec<String>, &'static str)> {
    if !config.network.probe_ping_argv.is_empty() {
        return Ok((config.network.probe_ping_argv.clone(), "configured"));
    }
    for path in ["/bin/ping", "/usr/bin/ping"] {
        if Path::new(path).exists() {
            return Ok((vec![path.to_string()], "linux_ping_preset"));
        }
    }
    anyhow::bail!("latency probe binary not found");
}

#[derive(Default)]
struct ParsedLatencyPing {
    healthy: bool,
    latency_avg_ms: Option<f64>,
    packet_loss_ratio: Option<f64>,
}

fn parse_latency_ping_output(stdout: &str) -> ParsedLatencyPing {
    let mut parsed = ParsedLatencyPing::default();
    let mut received = None::<u64>;
    for line in stdout.lines() {
        if line.contains("packets transmitted") && line.contains("packet loss") {
            let parts = line.split(',').map(str::trim).collect::<Vec<_>>();
            received = parts
                .get(1)
                .and_then(|part| part.split_whitespace().next())
                .and_then(|value| value.parse().ok());
            parsed.packet_loss_ratio = parts
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
            if samples.len() >= 2 {
                parsed.latency_avg_ms = Some(samples[1]);
            }
        }
    }
    parsed.healthy = received.unwrap_or(0) > 0 && parsed.latency_avg_ms.is_some();
    parsed
}

fn failed_probe(family: TunnelAddressFamily, target: String, reason: String) -> LatencyProbeResult {
    LatencyProbeResult {
        family,
        target,
        healthy: false,
        latency_avg_ms: None,
        packet_loss_ratio: Some(1.0),
        reason: Some(reason),
    }
}

fn merge_failed_probe(
    primary: LatencyProbeResult,
    fallback: LatencyProbeResult,
) -> LatencyProbeResult {
    let primary_family = primary.family_name().to_string();
    let fallback_family = fallback.family_name().to_string();
    LatencyProbeResult {
        family: primary.family,
        target: primary.target,
        healthy: false,
        latency_avg_ms: None,
        packet_loss_ratio: Some(1.0),
        reason: Some(format!(
            "primary_{}_and_fallback_{}_unhealthy",
            primary_family, fallback_family
        )),
    }
}

async fn apply_latency_monitoring(
    config: &AgentConfig,
    telemetry_plan: &AgentRuntimeStatusTelemetryPlan,
    now: u64,
    key: &str,
    runtime_state: &mut TelemetryRuntimeState,
    stat: &mut RuntimeTunnelStat,
) {
    let monitoring_enabled =
        config.network.latency_monitoring_enabled && telemetry_plan.latency_monitoring_enabled;
    stat.latency_monitoring_enabled = Some(monitoring_enabled);
    if !monitoring_enabled {
        stat.latency_status = Some("disabled".to_string());
        return;
    }
    let plan = &telemetry_plan.plan;
    let Some(LatencyTarget {
        family,
        target,
        fallback,
    }) = latency_targets(plan, telemetry_plan.endpoint_side)
    else {
        stat.latency_status = Some("unconfigured".to_string());
        stat.latency_reason = Some("no_tunnel_endpoint_for_latency_probe".to_string());
        return;
    };
    let state = runtime_state
        .latency_monitors
        .entry(key.to_string())
        .or_default();
    let probe = match run_latency_probe(config, family, &target).await {
        Ok(probe) if probe.healthy => probe,
        Ok(primary) => {
            if let Some((fallback_family, fallback_target)) = fallback {
                match run_latency_probe(config, fallback_family, &fallback_target).await {
                    Ok(fallback_probe) if fallback_probe.healthy => fallback_probe,
                    Ok(fallback_probe) => merge_failed_probe(primary, fallback_probe),
                    Err(error) => failed_probe(
                        family,
                        target.clone(),
                        format!("fallback_probe_failed:{error}"),
                    ),
                }
            } else {
                primary
            }
        }
        Err(error) => failed_probe(
            family,
            target.clone(),
            format!("latency_probe_failed:{error}"),
        ),
    };
    stat.latency_primary_family = Some(probe.family_name().to_string());
    stat.latency_target = Some(probe.target.clone());
    stat.latency_checked_unix = Some(now);
    stat.latency_avg_ms = probe.latency_avg_ms;
    stat.packet_loss_ratio = probe.packet_loss_ratio;
    if probe.healthy {
        state.healthy_windows = state.healthy_windows.saturating_add(1);
        state.missed_windows = 0;
        stat.latency_status = Some("healthy".to_string());
        stat.latency_reason = probe.reason.clone();
    } else {
        state.healthy_windows = 0;
        state.missed_windows = state.missed_windows.saturating_add(1);
        let down = state.missed_windows >= config.network.latency_down_windows;
        stat.latency_status = Some(if down { "down" } else { "missed" }.to_string());
        stat.latency_reason = probe.reason.clone().or_else(|| {
            Some(format!(
                "latency_probe_missing_healthy_sample:{}/{}",
                state.missed_windows, config.network.latency_down_windows
            ))
        });
    }
    stat.latency_healthy_windows = Some(state.healthy_windows);
    stat.latency_missed_windows = Some(state.missed_windows);
}

async fn run_adapter_status_telemetry(
    argv: &[String],
    max_timeout_secs: u64,
    max_output_bytes: usize,
    now: u64,
) -> Result<RuntimeTunnelAdapterHealthStat> {
    if argv.is_empty() || !argv[0].starts_with('/') {
        anyhow::bail!("adapter status telemetry executable must be absolute");
    }
    let started = Instant::now();
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]);
    command.kill_on_drop(true);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("adapter status stdout pipe missing"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("adapter status stderr pipe missing"))?;
    let mut stdout_task = Some(tokio::spawn(read_limited(stdout, max_output_bytes)));
    let mut stderr_task = Some(tokio::spawn(read_limited(stderr, max_output_bytes)));
    let deadline = Instant::now() + Duration::from_secs(max_timeout_secs);
    let mut timed_out = false;
    let mut output_truncated = false;
    let mut stdout_output = None;
    let mut stderr_output = None;

    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if stdout_output.is_none() && task_is_finished(&stdout_task) {
            let output = join_limited(stdout_task.take()).await?;
            output_truncated |= output.truncated;
            stdout_output = Some(output);
        }
        if stderr_output.is_none() && task_is_finished(&stderr_task) {
            let output = join_limited(stderr_task.take()).await?;
            output_truncated |= output.truncated;
            stderr_output = Some(output);
        }
        if output_truncated {
            child.start_kill()?;
            break child.wait().await.ok();
        }
        if Instant::now() >= deadline {
            timed_out = true;
            child.start_kill()?;
            break child.wait().await.ok();
        }
        time::sleep(Duration::from_millis(20)).await;
    };

    let stdout = match stdout_output {
        Some(output) => output,
        None => join_limited(stdout_task.take()).await?,
    };
    let stderr = match stderr_output {
        Some(output) => output,
        None => join_limited(stderr_task.take()).await?,
    };
    output_truncated |= stdout.truncated || stderr.truncated;
    Ok(adapter_health_report(AdapterHealthReportInput {
        argv,
        checked_unix: now,
        duration_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        status,
        timed_out,
        output_truncated,
        stdout,
        stderr,
    }))
}

struct AdapterHealthReportInput<'a> {
    argv: &'a [String],
    checked_unix: u64,
    duration_ms: u64,
    status: Option<ExitStatus>,
    timed_out: bool,
    output_truncated: bool,
    stdout: LimitedOutput,
    stderr: LimitedOutput,
}

fn adapter_health_report(input: AdapterHealthReportInput<'_>) -> RuntimeTunnelAdapterHealthStat {
    let exit_code = input.status.as_ref().and_then(ExitStatus::code);
    let success = input.status.as_ref().is_some_and(ExitStatus::success)
        && !input.timed_out
        && !input.output_truncated;
    let status = if success {
        "healthy"
    } else if input.timed_out {
        "timeout"
    } else if input.output_truncated {
        "output_limited"
    } else {
        "failed"
    };
    let reason = if success {
        None
    } else if input.timed_out {
        Some("adapter_status_timeout".to_string())
    } else if input.output_truncated {
        Some("adapter_status_output_limit".to_string())
    } else {
        Some("adapter_status_failed".to_string())
    };
    RuntimeTunnelAdapterHealthStat {
        status: status.to_string(),
        checked_unix: input.checked_unix,
        configured: true,
        success,
        exit_code,
        reason,
        duration_ms: input.duration_ms,
        command_sha256_hex: Some(sha256_hex(
            &serde_json::to_vec(input.argv).unwrap_or_default(),
        )),
        timed_out: input.timed_out,
        output_truncated: input.output_truncated,
        stdout_sha256_hex: Some(input.stdout.sha256_hex),
        stderr_sha256_hex: Some(input.stderr.sha256_hex),
    }
}

fn merge_runtime_status_tunnel(metrics: &mut AgentMetrics, mut stat: RuntimeTunnelStat) {
    if let Some(existing) = metrics
        .tunnels
        .iter_mut()
        .find(|existing| existing.interface == stat.interface)
    {
        existing.ownership_mode = stat.ownership_mode;
        existing.mutation_policy = stat.mutation_policy;
        existing.source = format!("{}+{}", existing.source, stat.source);
        existing.rx_bytes = stat.rx_bytes;
        existing.tx_bytes = stat.tx_bytes;
        existing.traffic_source = stat.traffic_source.take();
        existing.traffic_status = stat.traffic_status.take();
        existing.traffic_reason = stat.traffic_reason.take();
        existing.traffic_checked_unix = stat.traffic_checked_unix.take();
        existing.plan_id = stat.plan_id.take();
        existing.plan_name = stat.plan_name.take();
        existing.plan_runtime_manager = stat.plan_runtime_manager.take();
        existing.endpoint_side = stat.endpoint_side.take();
        existing.peer_client_id = stat.peer_client_id.take();
        existing.adapter_health = stat.adapter_health.take();
        existing.latency_monitoring_enabled = stat.latency_monitoring_enabled.take();
        existing.latency_status = stat.latency_status.take();
        existing.latency_reason = stat.latency_reason.take();
        existing.latency_primary_family = stat.latency_primary_family.take();
        existing.latency_target = stat.latency_target.take();
        existing.latency_checked_unix = stat.latency_checked_unix.take();
        existing.latency_avg_ms = stat.latency_avg_ms.take();
        existing.packet_loss_ratio = stat.packet_loss_ratio.take();
        existing.latency_healthy_windows = stat.latency_healthy_windows.take();
        existing.latency_missed_windows = stat.latency_missed_windows.take();
    } else {
        metrics.tunnels.push(stat);
    }
}

fn skipped_adapter_health(
    status: &str,
    checked_unix: u64,
    reason: &str,
) -> RuntimeTunnelAdapterHealthStat {
    RuntimeTunnelAdapterHealthStat {
        status: status.to_string(),
        checked_unix,
        configured: false,
        reason: Some(reason.to_string()),
        ..RuntimeTunnelAdapterHealthStat::default()
    }
}

fn runtime_status_telemetry_key(plan: &AgentRuntimeStatusTelemetryPlan) -> String {
    plan.plan_id.clone().unwrap_or_else(|| {
        format!(
            "{}:{}:{}",
            plan.plan.name,
            endpoint_side_label(plan.endpoint_side),
            plan.plan.interface_name
        )
    })
}

fn peer_client_id(plan: &vpsman_common::TunnelPlan, side: TunnelEndpointSide) -> &str {
    match side {
        TunnelEndpointSide::Left => &plan.right_client_id,
        TunnelEndpointSide::Right => &plan.left_client_id,
    }
}

fn endpoint_side_label(side: TunnelEndpointSide) -> &'static str {
    match side {
        TunnelEndpointSide::Left => "left",
        TunnelEndpointSide::Right => "right",
    }
}

fn runtime_manager_label(manager: RuntimeTunnelManager) -> &'static str {
    match manager {
        RuntimeTunnelManager::AgentIproute2Managed => "agent_iproute2_managed",
        RuntimeTunnelManager::ExternalObserved => "external_observed",
        RuntimeTunnelManager::ExternalManagedAdapter => "external_managed_adapter",
    }
}

fn tunnel_kind_label(kind: TunnelKind) -> &'static str {
    match kind {
        TunnelKind::Gre => "gre",
        TunnelKind::Ipip => "ipip",
        TunnelKind::Sit => "sit",
        TunnelKind::Fou => "fou",
        TunnelKind::Openvpn => "openvpn",
        TunnelKind::Wireguard => "wireguard",
        TunnelKind::TunTap => "tun_tap",
        TunnelKind::Custom => "custom",
    }
}

struct LimitedOutput {
    sha256_hex: String,
    truncated: bool,
}

async fn read_limited<R>(mut reader: R, limit: usize) -> std::io::Result<LimitedOutput>
where
    R: AsyncRead + Unpin,
{
    let mut hasher = Sha256::new();
    let mut total = 0_usize;
    let mut truncated = false;
    let mut buffer = [0_u8; 1024];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(total);
        let take = read.min(remaining);
        hasher.update(&buffer[..take]);
        total += take;
        if take < read || total >= limit {
            truncated = true;
            break;
        }
    }
    Ok(LimitedOutput {
        sha256_hex: hex::encode(hasher.finalize()),
        truncated,
    })
}

fn task_is_finished(task: &Option<JoinHandle<std::io::Result<LimitedOutput>>>) -> bool {
    task.as_ref().is_some_and(JoinHandle::is_finished)
}

async fn join_limited(
    task: Option<JoinHandle<std::io::Result<LimitedOutput>>>,
) -> Result<LimitedOutput> {
    let Some(task) = task else {
        return Ok(LimitedOutput {
            sha256_hex: sha256_hex(&[]),
            truncated: false,
        });
    };
    Ok(task.await??)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn disk_stats(proc_root: &Path) -> Result<Vec<DiskStat>> {
    let contents = read_proc_file(proc_root, "mounts")?;
    let ignored = HashSet::from([
        "proc",
        "sysfs",
        "devtmpfs",
        "devpts",
        "tmpfs",
        "securityfs",
        "cgroup",
        "cgroup2",
        "pstore",
        "efivarfs",
        "bpf",
        "tracefs",
        "debugfs",
        "overlay",
    ]);
    let mut seen_sources = HashSet::new();
    let mut disks = Vec::new();

    for (index, line) in contents.lines().enumerate() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        ensure!(
            fields.len() >= 3,
            "telemetry mounts row {} is incomplete",
            index + 1
        );
        if ignored.contains(fields[2])
            || !seen_sources.insert((fields[0].to_string(), fields[2].to_string()))
        {
            continue;
        }
        let mountpoint = decode_proc_mount_field(fields[1]);
        let stat = statvfs(&mountpoint)?;
        disks.push(DiskStat {
            mountpoint,
            total_bytes: stat.0,
            available_bytes: stat.1,
        });
        if disks.len() == MAX_TELEMETRY_DISKS {
            break;
        }
    }

    Ok(disks)
}

fn decode_proc_mount_field(value: &str) -> String {
    value
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

fn statvfs(path: &str) -> Result<(u64, u64)> {
    let c_path = CString::new(path).context("telemetry mount path contains a null byte")?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("failed to inspect telemetry mount {path}"));
    }
    let stat = unsafe { stat.assume_init() };
    let total = stat.f_blocks.saturating_mul(stat.f_frsize);
    let available = stat.f_bavail.saturating_mul(stat.f_frsize);
    Ok((total, available))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_hostname_read_failure_is_not_replaced_by_a_default() {
        let default_called = std::cell::Cell::new(false);
        let error = resolve_hostname(
            Some("/configured/hostname"),
            |_| Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing")),
            || {
                default_called.set(true);
                Ok("fallback-hostname".to_string())
            },
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("failed to read configured hostname file /configured/hostname"));
        assert!(!default_called.get());
    }

    #[test]
    fn configured_hostname_must_not_be_empty() {
        let error = resolve_hostname(
            Some("/configured/hostname"),
            |_| Ok(" \n".to_string()),
            || Ok("fallback-hostname".to_string()),
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "configured hostname file /configured/hostname is empty"
        );
    }

    #[test]
    fn missing_default_hostname_is_an_error_instead_of_an_invented_identity() {
        let error = resolve_hostname(
            None,
            |_| unreachable!("no configured hostname file should be read"),
            || anyhow::bail!("system source unavailable"),
        )
        .unwrap_err();

        assert!(format!("{error:#}").contains("system source unavailable"));
        assert!(!format!("{error:#}").contains("unknown"));
    }

    #[test]
    fn default_hostname_uses_the_operating_system_value() {
        let hostname = resolve_hostname(
            None,
            |_| unreachable!("no configured hostname file should be read"),
            || Ok("node-from-os\n".to_string()),
        )
        .unwrap();

        assert_eq!(hostname, "node-from-os");
    }

    #[test]
    fn parses_linux_network_counters_without_classifying_tunnels() {
        let stats = network_stats_from_proc_net_dev(
            "Inter-| Receive | Transmit\n\
             face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n\
             eth0: 10 1 0 0 0 0 0 0 20 2 0 0 0 0 0 0\n\
             wg0: 30 3 0 0 0 0 0 0 40 4 0 0 0 0 0 0\n",
        )
        .unwrap();
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].interface, "eth0");
        assert_eq!(stats[1].interface, "wg0");
        assert_eq!(stats[1].rx_bytes, 30);
        assert_eq!(stats[1].tx_bytes, 40);
    }

    #[test]
    fn linux_network_collection_stays_within_the_ingest_cardinality_limit() {
        let mut contents = String::from(
            "Inter-| Receive | Transmit\n\
             face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n",
        );
        for index in 0..(MAX_TELEMETRY_NETWORKS + 1) {
            contents.push_str(&format!(
                "veth{index}: {index} 1 0 0 0 0 0 0 {index} 1 0 0 0 0 0 0\n"
            ));
        }

        let stats = network_stats_from_proc_net_dev(&contents).unwrap();
        assert_eq!(stats.len(), MAX_TELEMETRY_NETWORKS);
        assert_eq!(stats.last().unwrap().interface, "veth511");
    }

    #[test]
    fn malformed_linux_network_counters_fail_instead_of_becoming_zero() {
        let error = network_stats_from_proc_net_dev(
            "Inter-| Receive | Transmit\n\
             face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n\
             eth0: broken 1 0 0 0 0 0 0 20 2 0 0 0 0 0 0\n",
        )
        .unwrap_err();
        assert!(error.to_string().contains("RX counter"));
    }

    #[test]
    fn incomplete_meminfo_fails_instead_of_becoming_zero() {
        let root = std::env::temp_dir().join(format!("vpsman-meminfo-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("meminfo"), "MemTotal: 1024 kB\n").unwrap();
        let error = memory_stat(&root).unwrap_err();
        assert!(error.to_string().contains("MemAvailable"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn malformed_mount_inventory_fails_instead_of_disappearing() {
        let root = std::env::temp_dir().join(format!("vpsman-mounts-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("mounts"), "incomplete-row\n").unwrap();
        let error = disk_stats(&root).unwrap_err();
        assert!(error.to_string().contains("row 1 is incomplete"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn decodes_proc_mount_escapes_before_inspection() {
        assert_eq!(
            decode_proc_mount_field("/srv/space\\040and\\011tab\\134dir"),
            "/srv/space and\ttab\\dir"
        );
    }

    #[test]
    fn parses_latency_probe_output_as_observation_only() {
        let parsed = parse_latency_ping_output(
            "3 packets transmitted, 2 received, 33.333% packet loss\n\
             rtt min/avg/max/mdev = 10.0/12.5/15.0/1.0 ms\n",
        );
        assert!(parsed.healthy);
        assert_eq!(parsed.latency_avg_ms, Some(12.5));
        assert!(parsed.packet_loss_ratio.unwrap() > 0.33);
    }

    #[test]
    fn runtime_labels_do_not_imply_discovery_or_routing_mutation() {
        assert_eq!(
            runtime_manager_label(RuntimeTunnelManager::AgentIproute2Managed),
            "agent_iproute2_managed"
        );
        assert_eq!(
            runtime_manager_label(RuntimeTunnelManager::ExternalObserved),
            "external_observed"
        );
        assert_eq!(
            runtime_manager_label(RuntimeTunnelManager::ExternalManagedAdapter),
            "external_managed_adapter"
        );
    }
}
