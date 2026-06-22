use std::{
    collections::{HashMap, HashSet},
    ffi::CString,
    path::Path,
    process::{ExitStatus, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    task::JoinHandle,
    time::{self, Duration, Instant},
};
use tracing::debug;
use vpsman_common::{
    ospf_cost, render_tunnel_endpoint_config, AgentConfig, AgentMetrics,
    AgentRuntimeStatusTelemetryPlan, CpuStat, DiskStat, LoadAverage, MemoryStat, NetworkStat,
    RuntimeTunnelAdapterHealthStat, RuntimeTunnelCommand, RuntimeTunnelManager, RuntimeTunnelStat,
    TunnelAddressFamily, TunnelEndpointSide, TunnelKind, TunnelObservation,
};

use crate::child_process::{run_child_with_bounded_output, ChildCleanupPolicy, ChildRunResult};
use crate::network_runtime::render_runtime_adapter_command;
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
    last_cost: Option<u16>,
    last_update_unix: Option<u64>,
}

fn collect_linux_metrics(config: &AgentConfig) -> Result<AgentMetrics> {
    let proc_root = Path::new(&config.telemetry.proc_root);
    let sys_class_net = Path::new(&config.telemetry.sys_class_net_dir);
    let networks = network_stats(proc_root).unwrap_or_default();
    let network_counters = networks
        .iter()
        .map(|stat| (stat.interface.clone(), stat.clone()))
        .collect::<HashMap<_, _>>();
    Ok(AgentMetrics {
        observed_unix: unix_now(),
        hostname: hostname(config),
        uptime_secs: uptime_secs(proc_root).unwrap_or_default(),
        cpu: CpuStat {
            load: load_average(proc_root).unwrap_or_default(),
            cores: std::thread::available_parallelism()
                .map(|value| value.get() as u16)
                .unwrap_or(1),
        },
        memory: memory_stat(proc_root).unwrap_or_default(),
        disks: disk_stats(proc_root).unwrap_or_default(),
        networks,
        tunnels: tunnel_stats_from_sysfs(sys_class_net, &network_counters).unwrap_or_default(),
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
    apply_custom_metrics_if_configured(config, &mut metrics).await;
    collect_runtime_status_telemetry(config, &mut metrics, runtime_state).await;
    Ok(metrics)
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn read_optional(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn read_optional_path(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn hostname(config: &AgentConfig) -> String {
    config
        .telemetry
        .hostname_file
        .as_deref()
        .and_then(read_optional)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "unknown".to_string())
}

fn uptime_secs(proc_root: &Path) -> Option<u64> {
    let contents = read_optional_path(&proc_root.join("uptime"))?;
    let first = contents.split_whitespace().next()?;
    first.parse::<f64>().ok().map(|value| value as u64)
}

fn load_average(proc_root: &Path) -> Option<LoadAverage> {
    let contents = read_optional_path(&proc_root.join("loadavg"))?;
    let mut fields = contents.split_whitespace();
    Some(LoadAverage {
        one: fields.next()?.parse().ok()?,
        five: fields.next()?.parse().ok()?,
        fifteen: fields.next()?.parse().ok()?,
    })
}

fn memory_stat(proc_root: &Path) -> Option<MemoryStat> {
    let contents = read_optional_path(&proc_root.join("meminfo"))?;
    let mut total = 0_u64;
    let mut available = 0_u64;

    for line in contents.lines() {
        let mut fields = line.split_whitespace();
        match fields.next()? {
            "MemTotal:" => total = fields.next()?.parse::<u64>().ok()? * 1024,
            "MemAvailable:" => available = fields.next()?.parse::<u64>().ok()? * 1024,
            _ => {}
        }
    }

    Some(MemoryStat {
        total_bytes: total,
        available_bytes: available,
    })
}

fn network_stats(proc_root: &Path) -> Option<Vec<NetworkStat>> {
    let contents = read_optional_path(&proc_root.join("net/dev"))?;
    Some(network_stats_from_proc_net_dev(&contents))
}

fn network_stats_from_proc_net_dev(contents: &str) -> Vec<NetworkStat> {
    let mut stats = Vec::new();

    for line in contents.lines().skip(2) {
        let Some((name, counters)) = line.split_once(':') else {
            continue;
        };
        let fields: Vec<&str> = counters.split_whitespace().collect();
        if fields.len() < 16 {
            continue;
        }
        stats.push(NetworkStat {
            interface: name.trim().to_string(),
            rx_bytes: fields[0].parse().unwrap_or_default(),
            tx_bytes: fields[8].parse().unwrap_or_default(),
        });
    }

    stats
}

fn tunnel_stats_from_sysfs(
    sys_class_net: &Path,
    network_counters: &HashMap<String, NetworkStat>,
) -> Result<Vec<RuntimeTunnelStat>> {
    let mut tunnels = Vec::new();
    for entry in std::fs::read_dir(sys_class_net)? {
        let entry = entry?;
        let interface = entry.file_name().to_string_lossy().to_string();
        if interface.is_empty() || interface.len() > 64 {
            continue;
        }
        let interface_path = entry.path();
        let link_type =
            read_small_trimmed(&interface_path.join("type")).and_then(|value| value.parse().ok());
        let Some(kind) = classify_runtime_tunnel(&interface, link_type) else {
            continue;
        };
        let counters = network_counters.get(&interface);
        tunnels.push(RuntimeTunnelStat {
            interface,
            kind,
            ownership_mode: "runtime_observed".to_string(),
            mutation_policy: "observe_only_import_candidate".to_string(),
            promotion_required: true,
            source: "sysfs_proc_net_dev".to_string(),
            operstate: read_small_trimmed(&interface_path.join("operstate")),
            mtu: read_small_trimmed(&interface_path.join("mtu"))
                .and_then(|value| value.parse().ok()),
            link_type,
            address: read_small_trimmed(&interface_path.join("address")),
            rx_bytes: counters.map(|stat| stat.rx_bytes).unwrap_or_default(),
            tx_bytes: counters.map(|stat| stat.tx_bytes).unwrap_or_default(),
            traffic_source: Some("interface_counters".to_string()),
            traffic_status: Some(
                if counters.is_some() {
                    "ok"
                } else {
                    "interface_counters_missing"
                }
                .to_string(),
            ),
            traffic_reason: counters
                .is_none()
                .then(|| "interface_not_found_in_proc_net_dev".to_string()),
            ..RuntimeTunnelStat::default()
        });
    }
    tunnels.sort_by(|left, right| left.interface.cmp(&right.interface));
    tunnels.truncate(64);
    Ok(tunnels)
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
    for telemetry_plan in config
        .network
        .runtime_status_telemetry_plans
        .iter()
        .take(16)
    {
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
        promotion_required: false,
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
    let Some(command) = &plan.runtime_control.status else {
        return RuntimeTunnelAdapterHealthStat {
            status: "unconfigured".to_string(),
            checked_unix: now,
            configured: false,
            reason: Some("adapter_status_unconfigured".to_string()),
            ..RuntimeTunnelAdapterHealthStat::default()
        };
    };
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
    stat.auto_ospf_enabled = Some(
        config.network.auto_ospf_enabled
            && telemetry_plan.auto_ospf_enabled
            && effective_ospf_updater(config, telemetry_plan).is_some(),
    );
    if !monitoring_enabled {
        stat.latency_status = Some("disabled".to_string());
        stat.auto_ospf_status = Some("disabled".to_string());
        stat.auto_ospf_reason = Some("latency_monitoring_disabled".to_string());
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
        stat.auto_ospf_status = Some("monitoring_only".to_string());
        stat.auto_ospf_reason = Some("latency_target_missing".to_string());
        return;
    };
    let state = runtime_state
        .latency_monitors
        .entry(key.to_string())
        .or_insert_with(|| LatencyMonitorState {
            last_cost: Some(plan.recommended_ospf_cost),
            ..LatencyMonitorState::default()
        });
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
    apply_auto_ospf(config, telemetry_plan, now, state, stat, &probe).await;
}

async fn apply_auto_ospf(
    config: &AgentConfig,
    telemetry_plan: &AgentRuntimeStatusTelemetryPlan,
    now: u64,
    state: &mut LatencyMonitorState,
    stat: &mut RuntimeTunnelStat,
    probe: &LatencyProbeResult,
) {
    let plan = &telemetry_plan.plan;
    let Some(updater) = effective_ospf_updater(config, telemetry_plan) else {
        stat.auto_ospf_status = Some("monitoring_only".to_string());
        stat.auto_ospf_reason = Some("external_cost_program_unconfigured".to_string());
        return;
    };
    stat.auto_ospf_updated_unix = state.last_update_unix;
    if !config.network.auto_ospf_enabled || !telemetry_plan.auto_ospf_enabled {
        stat.auto_ospf_status = Some("disabled".to_string());
        stat.auto_ospf_reason = Some("auto_ospf_disabled".to_string());
        return;
    }
    if !probe.healthy {
        stat.auto_ospf_status = Some("report_only".to_string());
        stat.auto_ospf_reason =
            Some("latency_probe_unhealthy_ospf_handles_dead_adjacency".to_string());
        return;
    }
    if state.healthy_windows < config.network.auto_ospf_healthy_windows {
        stat.auto_ospf_status = Some("stabilizing".to_string());
        stat.auto_ospf_reason = Some(format!(
            "healthy_windows:{}/{}",
            state.healthy_windows, config.network.auto_ospf_healthy_windows
        ));
        return;
    }
    let latency_ms = probe
        .latency_avg_ms
        .unwrap_or(plan.recommended_ospf_cost as f64);
    let packet_loss_ratio = probe.packet_loss_ratio.unwrap_or(0.0);
    let recommended = ospf_cost(
        config.network.auto_ospf_policy,
        TunnelObservation {
            latency_ms,
            packet_loss_ratio,
            bandwidth: plan.bandwidth,
            preference: 1.0,
        },
    );
    let current = state.last_cost.unwrap_or(plan.recommended_ospf_cost);
    stat.auto_ospf_current_cost = Some(current);
    stat.auto_ospf_recommended_cost = Some(recommended);
    let delta = current.abs_diff(recommended);
    if delta < config.network.auto_ospf_min_cost_delta {
        stat.auto_ospf_status = Some("stable".to_string());
        stat.auto_ospf_reason = Some(format!(
            "cost_delta:{delta}<{}",
            config.network.auto_ospf_min_cost_delta
        ));
        return;
    }
    match run_auto_ospf_updater(config, telemetry_plan, updater, current, recommended, probe).await
    {
        Ok(()) => {
            state.last_cost = Some(recommended);
            state.last_update_unix = Some(now);
            stat.auto_ospf_status = Some("updated".to_string());
            stat.auto_ospf_reason = Some("external_cost_program_succeeded".to_string());
            stat.auto_ospf_updated_unix = Some(now);
        }
        Err(error) => {
            stat.auto_ospf_status = Some("failed".to_string());
            stat.auto_ospf_reason = Some(format!("external_cost_program_failed:{error}"));
        }
    }
}

fn effective_ospf_updater<'a>(
    config: &'a AgentConfig,
    telemetry_plan: &'a AgentRuntimeStatusTelemetryPlan,
) -> Option<&'a RuntimeTunnelCommand> {
    // Config precedence is most local first: tunnel plan, agent config, then global/default sources.
    telemetry_plan
        .auto_ospf_updater
        .as_ref()
        .or(config.network.auto_ospf_updater.as_ref())
}

async fn run_auto_ospf_updater(
    config: &AgentConfig,
    telemetry_plan: &AgentRuntimeStatusTelemetryPlan,
    updater: &RuntimeTunnelCommand,
    current_cost: u16,
    recommended_cost: u16,
    probe: &LatencyProbeResult,
) -> Result<()> {
    let endpoint =
        render_tunnel_endpoint_config(&telemetry_plan.plan, telemetry_plan.endpoint_side)
            .map_err(|error| anyhow::anyhow!("endpoint_render_failed:{error}"))?;
    let mut argv = render_runtime_adapter_command(updater, &telemetry_plan.plan, &endpoint)?;
    for part in &mut argv {
        *part = part
            .replace("{current_ospf_cost}", &current_cost.to_string())
            .replace("{recommended_ospf_cost}", &recommended_cost.to_string())
            .replace("{latency_avg_ms}", &optional_f64(probe.latency_avg_ms))
            .replace(
                "{packet_loss_ratio}",
                &optional_f64(probe.packet_loss_ratio),
            )
            .replace("{latency_family}", probe.family_name())
            .replace("{latency_target}", &probe.target);
    }
    let payload = serde_json::json!({
        "type": "network_auto_ospf_cost_update",
        "plan": telemetry_plan.plan.name,
        "interface": telemetry_plan.plan.interface_name,
        "side": endpoint_side_label(telemetry_plan.endpoint_side),
        "client_id": &config.client_id,
        "peer_client_id": &endpoint.peer_client_id,
        "local_underlay": if telemetry_plan.endpoint_side == TunnelEndpointSide::Left { &telemetry_plan.plan.left_underlay } else { &telemetry_plan.plan.right_underlay },
        "remote_underlay": if telemetry_plan.endpoint_side == TunnelEndpointSide::Left { &telemetry_plan.plan.right_underlay } else { &telemetry_plan.plan.left_underlay },
        "local_address": &endpoint.local_tunnel_address,
        "remote_address": &endpoint.remote_tunnel_address,
        "prefix_len": endpoint.tunnel_prefix_len,
        "ipv4": &telemetry_plan.plan.ipv4_tunnel,
        "ipv6": &telemetry_plan.plan.ipv6_tunnel,
        "latency": {
            "family": probe.family_name(),
            "target": probe.target,
            "healthy": probe.healthy,
            "latency_avg_ms": probe.latency_avg_ms,
            "packet_loss_ratio": probe.packet_loss_ratio,
        },
        "current_ospf_cost": current_cost,
        "recommended_ospf_cost": recommended_cost,
        "reason": "latency_and_configured_bandwidth_tier",
    });
    run_json_stdin_command(
        &argv,
        updater.max_timeout_secs,
        updater.max_output_bytes as usize,
        payload,
    )
    .await
}

async fn run_json_stdin_command(
    argv: &[String],
    max_timeout_secs: u64,
    max_output_bytes: usize,
    payload: serde_json::Value,
) -> Result<()> {
    if argv.is_empty() || !argv[0].starts_with('/') {
        anyhow::bail!("external cost program executable must be absolute");
    }
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]);
    command.kill_on_drop(true);
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        let body = serde_json::to_vec(&payload)?;
        stdin.write_all(&body).await?;
        stdin.write_all(b"\n").await?;
        stdin.shutdown().await?;
    }
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("external cost stdout pipe missing"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("external cost stderr pipe missing"))?;
    let stdout_task = tokio::spawn(read_limited(stdout, max_output_bytes));
    let stderr_task = tokio::spawn(read_limited(stderr, max_output_bytes));
    let status = time::timeout(
        Duration::from_secs(max_timeout_secs.clamp(1, 120)),
        child.wait(),
    )
    .await
    .context("external cost program timed out")??;
    let stdout = stdout_task.await??;
    let stderr = stderr_task.await??;
    if stdout.truncated || stderr.truncated {
        anyhow::bail!("external cost program output limit exceeded");
    }
    if !status.success() {
        anyhow::bail!("external cost program exited with {:?}", status.code());
    }
    Ok(())
}

fn optional_f64(value: Option<f64>) -> String {
    value.map(|value| format!("{value:.3}")).unwrap_or_default()
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
        existing.promotion_required = false;
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
        existing.auto_ospf_enabled = stat.auto_ospf_enabled.take();
        existing.auto_ospf_status = stat.auto_ospf_status.take();
        existing.auto_ospf_reason = stat.auto_ospf_reason.take();
        existing.auto_ospf_current_cost = stat.auto_ospf_current_cost.take();
        existing.auto_ospf_recommended_cost = stat.auto_ospf_recommended_cost.take();
        existing.auto_ospf_updated_unix = stat.auto_ospf_updated_unix.take();
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

fn classify_runtime_tunnel(interface: &str, link_type: Option<i64>) -> Option<String> {
    let lower = interface.to_ascii_lowercase();
    let kind = match link_type {
        Some(778) => Some("gre"),
        Some(776) => Some("sit"),
        Some(768) => Some("ipip"),
        Some(65534) if lower.starts_with("tun") || lower.starts_with("tap") => Some("tun_tap"),
        _ if lower.starts_with("wg") => Some("wireguard"),
        _ if lower.starts_with("tun") || lower.starts_with("tap") => Some("tun_tap"),
        _ if lower.starts_with("gre") => Some("gre"),
        _ if lower.starts_with("ipip") => Some("ipip"),
        _ if lower.starts_with("sit") => Some("sit"),
        _ if lower.contains("openvpn") || lower.starts_with("ovpn") => Some("openvpn"),
        _ if lower.starts_with("vpn") || lower.starts_with("vps") => Some("custom"),
        _ => None,
    }?;
    Some(kind.to_string())
}

fn read_small_trimmed(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 4096 {
        return None;
    }
    std::fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn disk_stats(proc_root: &Path) -> Option<Vec<DiskStat>> {
    let contents = read_optional_path(&proc_root.join("mounts"))?;
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
    let mut seen = HashSet::new();
    let mut disks = Vec::new();

    for line in contents.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 3 || ignored.contains(fields[2]) || !seen.insert(fields[1].to_string()) {
            continue;
        }
        if let Some(stat) = statvfs(fields[1]) {
            disks.push(DiskStat {
                mountpoint: fields[1].to_string(),
                total_bytes: stat.0,
                available_bytes: stat.1,
            });
        }
    }

    Some(disks)
}

fn statvfs(path: &str) -> Option<(u64, u64)> {
    let c_path = CString::new(path).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        debug!(%path, "statvfs failed");
        return None;
    }
    let stat = unsafe { stat.assume_init() };
    let total = stat.f_blocks.saturating_mul(stat.f_frsize);
    let available = stat.f_bavail.saturating_mul(stat.f_frsize);
    Some((total, available))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_ADAPTER_STATUS_SCRIPT: &str =
        "#!/bin/sh\nprintf 'run\\n' >> \"$3\"\nprintf 'adapter interface=%s peer=%s\\n' \"$1\" \"$2\"\n";
    const TEST_TRAFFIC_SOURCE_SCRIPT: &str =
        "#!/bin/sh\nprintf '{\"rx_bytes\":1234,\"tx_bytes\":5678}\\n'\n";
    const TEST_CUSTOM_METRICS_SOURCE_SCRIPT: &str = "#!/bin/sh\ncat <<'JSON'\n{\"hostname\":\"custom-edge\",\"uptime_secs\":42,\"cpu\":{\"cores\":2,\"load\":{\"one\":0.1,\"five\":0.2,\"fifteen\":0.3}},\"memory\":{\"total_bytes\":1024,\"available_bytes\":512},\"networks\":[{\"interface\":\"edge0\",\"rx_bytes\":10,\"tx_bytes\":20}]}\nJSON\n";
    use std::os::unix::fs::PermissionsExt;
    use vpsman_common::{
        plan_tunnel, AgentRuntimeStatusTelemetryPlan, AgentRuntimeTrafficSource,
        AgentTelemetryConfig, AgentTelemetrySource, BandwidthTier, OspfCostPolicy,
        RuntimeTunnelCommand, RuntimeTunnelControl, RuntimeTunnelManager, TunnelAddressFamily,
        TunnelEndpointSide, TunnelKind, TunnelPlanInput,
    };

    fn write_test_script(path: &std::path::Path, contents: &str) {
        std::fs::write(path, contents).unwrap();
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }

    fn telemetry_dual_stack_plan(
        latency_primary_family: TunnelAddressFamily,
    ) -> vpsman_common::TunnelPlan {
        plan_tunnel(&TunnelPlanInput {
            name: "edge-a-b".to_string(),
            interface_name: "tunab".to_string(),
            kind: TunnelKind::Gre,
            runtime_control: RuntimeTunnelControl::default(),
            runtime_topology: Default::default(),
            left_client_id: "edge-a".to_string(),
            right_client_id: "edge-b".to_string(),
            left_underlay: "198.51.100.10".to_string(),
            right_underlay: "203.0.113.20".to_string(),
            address_pool_cidr: String::new(),
            reserved_addresses: Vec::new(),
            ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
                left: "10.42.0.0".to_string(),
                right: "10.42.0.1".to_string(),
                prefix_len: 31,
            }),
            ipv6_address_pool_cidr: None,
            ipv6_tunnel: Some(vpsman_common::TunnelAddressPair {
                left: "fd00:42::0".to_string(),
                right: "fd00:42::1".to_string(),
                prefix_len: 127,
            }),
            latency_primary_family,
            bandwidth: BandwidthTier::M100,
            latency_ms: 12.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            ospf_policy: OspfCostPolicy::default(),
        })
        .unwrap()
    }

    fn telemetry_plan(
        auto_ospf_enabled: bool,
        auto_ospf_updater: Option<RuntimeTunnelCommand>,
        latency_primary_family: TunnelAddressFamily,
    ) -> AgentRuntimeStatusTelemetryPlan {
        AgentRuntimeStatusTelemetryPlan {
            plan_id: Some("plan-a".to_string()),
            endpoint_side: TunnelEndpointSide::Left,
            plan: telemetry_dual_stack_plan(latency_primary_family),
            traffic_source: AgentRuntimeTrafficSource::InterfaceCounters,
            traffic_command: None,
            latency_monitoring_enabled: true,
            auto_ospf_enabled,
            auto_ospf_updater,
        }
    }

    #[test]
    fn extracts_runtime_tunnel_inventory_from_sysfs_and_proc_counters() {
        let root =
            std::env::temp_dir().join(format!("vpsman-telemetry-tunnels-{}", uuid::Uuid::new_v4()));
        let sys_class_net = root.join("sys/class/net");
        std::fs::create_dir_all(sys_class_net.join("tun0")).unwrap();
        std::fs::create_dir_all(sys_class_net.join("eth0")).unwrap();
        std::fs::write(sys_class_net.join("tun0/type"), "65534\n").unwrap();
        std::fs::write(sys_class_net.join("tun0/operstate"), "up\n").unwrap();
        std::fs::write(sys_class_net.join("tun0/mtu"), "1500\n").unwrap();
        std::fs::write(sys_class_net.join("tun0/address"), "00:00:00:00:00:00\n").unwrap();
        std::fs::write(sys_class_net.join("eth0/type"), "1\n").unwrap();

        let counters = network_stats_from_proc_net_dev(
            r#"
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
  tun0: 123 1 0 0 0 0 0 0 456 2 0 0 0 0 0 0
  eth0: 999 1 0 0 0 0 0 0 888 2 0 0 0 0 0 0
"#,
        )
        .into_iter()
        .map(|stat| (stat.interface.clone(), stat))
        .collect::<HashMap<_, _>>();

        let tunnels = tunnel_stats_from_sysfs(&sys_class_net, &counters).unwrap();
        assert_eq!(tunnels.len(), 1);
        assert_eq!(tunnels[0].interface, "tun0");
        assert_eq!(tunnels[0].kind, "tun_tap");
        assert_eq!(tunnels[0].ownership_mode, "runtime_observed");
        assert_eq!(tunnels[0].mutation_policy, "observe_only_import_candidate");
        assert!(tunnels[0].promotion_required);
        assert_eq!(tunnels[0].rx_bytes, 123);
        assert_eq!(tunnels[0].tx_bytes, 456);
        assert_eq!(
            tunnels[0].traffic_source.as_deref(),
            Some("interface_counters")
        );
        assert_eq!(tunnels[0].traffic_status.as_deref(), Some("ok"));
        assert_eq!(tunnels[0].operstate.as_deref(), Some("up"));

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn latency_targets_use_primary_family_and_keep_fallback() {
        let plan = plan_tunnel(&TunnelPlanInput {
            name: "dual-stack".to_string(),
            interface_name: "tun6".to_string(),
            kind: TunnelKind::Gre,
            runtime_control: RuntimeTunnelControl::default(),
            runtime_topology: Default::default(),
            left_client_id: "edge-a".to_string(),
            right_client_id: "edge-b".to_string(),
            left_underlay: "198.51.100.10".to_string(),
            right_underlay: "203.0.113.20".to_string(),
            address_pool_cidr: String::new(),
            reserved_addresses: Vec::new(),
            ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
                left: "10.255.10.0".to_string(),
                right: "10.255.10.1".to_string(),
                prefix_len: 31,
            }),
            ipv6_address_pool_cidr: None,
            ipv6_tunnel: Some(vpsman_common::TunnelAddressPair {
                left: "fd00:10::0".to_string(),
                right: "fd00:10::1".to_string(),
                prefix_len: 127,
            }),
            latency_primary_family: TunnelAddressFamily::Ipv6,
            bandwidth: BandwidthTier::M100,
            latency_ms: 10.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            ospf_policy: OspfCostPolicy::default(),
        })
        .unwrap();

        let target = latency_targets(&plan, TunnelEndpointSide::Left).expect("target");

        assert_eq!(target.family, TunnelAddressFamily::Ipv6);
        assert_eq!(target.target, "fd00:10::1");
        assert_eq!(
            target.fallback,
            Some((TunnelAddressFamily::Ipv4, "10.255.10.1".to_string()))
        );
    }

    #[test]
    fn effective_ospf_updater_prefers_tunnel_local_over_agent_fallback() {
        let plan = plan_tunnel(&TunnelPlanInput {
            name: "adapter-a-b".to_string(),
            interface_name: "ovpn42".to_string(),
            kind: TunnelKind::Openvpn,
            runtime_control: RuntimeTunnelControl {
                manager: RuntimeTunnelManager::ExternalManagedAdapter,
                status: Some(RuntimeTunnelCommand {
                    argv: vec!["/usr/local/libexec/vpsman-adapter".to_string()],
                    ..RuntimeTunnelCommand::default()
                }),
                ..RuntimeTunnelControl::default()
            },
            runtime_topology: Default::default(),
            left_client_id: "edge-a".to_string(),
            right_client_id: "edge-b".to_string(),
            left_underlay: "198.51.100.10".to_string(),
            right_underlay: "198.51.100.11".to_string(),
            address_pool_cidr: "10.42.0.0/30".to_string(),
            reserved_addresses: Vec::new(),
            ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
                left: "10.42.0.0".to_string(),
                right: "10.42.0.1".to_string(),
                prefix_len: 31,
            }),
            ipv6_address_pool_cidr: None,
            ipv6_tunnel: None,
            latency_primary_family: Default::default(),
            bandwidth: BandwidthTier::M100,
            latency_ms: 12.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            ospf_policy: OspfCostPolicy::default(),
        })
        .unwrap();
        let mut config = AgentConfig::default();
        config.network.auto_ospf_updater = Some(RuntimeTunnelCommand {
            argv: vec!["/usr/local/libexec/global-ospf".to_string()],
            ..RuntimeTunnelCommand::default()
        });
        let telemetry_plan = AgentRuntimeStatusTelemetryPlan {
            plan_id: Some("plan-a".to_string()),
            endpoint_side: TunnelEndpointSide::Left,
            plan,
            traffic_source: AgentRuntimeTrafficSource::InterfaceCounters,
            traffic_command: None,
            latency_monitoring_enabled: true,
            auto_ospf_enabled: true,
            auto_ospf_updater: Some(RuntimeTunnelCommand {
                argv: vec!["/usr/local/libexec/plan-ospf".to_string()],
                ..RuntimeTunnelCommand::default()
            }),
        };

        let updater = effective_ospf_updater(&config, &telemetry_plan).expect("updater");

        assert_eq!(updater.argv[0], "/usr/local/libexec/plan-ospf");
    }

    #[test]
    fn effective_ospf_updater_uses_agent_fallback_when_tunnel_has_none() {
        let plan = plan_tunnel(&TunnelPlanInput {
            name: "adapter-a-b".to_string(),
            interface_name: "ovpn42".to_string(),
            kind: TunnelKind::Openvpn,
            runtime_control: RuntimeTunnelControl {
                manager: RuntimeTunnelManager::ExternalManagedAdapter,
                status: Some(RuntimeTunnelCommand {
                    argv: vec!["/usr/local/libexec/vpsman-adapter".to_string()],
                    ..RuntimeTunnelCommand::default()
                }),
                ..RuntimeTunnelControl::default()
            },
            runtime_topology: Default::default(),
            left_client_id: "edge-a".to_string(),
            right_client_id: "edge-b".to_string(),
            left_underlay: "198.51.100.10".to_string(),
            right_underlay: "198.51.100.11".to_string(),
            address_pool_cidr: "10.42.0.0/30".to_string(),
            reserved_addresses: Vec::new(),
            ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
                left: "10.42.0.0".to_string(),
                right: "10.42.0.1".to_string(),
                prefix_len: 31,
            }),
            ipv6_address_pool_cidr: None,
            ipv6_tunnel: None,
            latency_primary_family: Default::default(),
            bandwidth: BandwidthTier::M100,
            latency_ms: 12.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            ospf_policy: OspfCostPolicy::default(),
        })
        .unwrap();
        let mut config = AgentConfig::default();
        config.network.auto_ospf_updater = Some(RuntimeTunnelCommand {
            argv: vec!["/usr/local/libexec/agent-ospf".to_string()],
            ..RuntimeTunnelCommand::default()
        });
        let telemetry_plan = AgentRuntimeStatusTelemetryPlan {
            plan_id: Some("plan-a".to_string()),
            endpoint_side: TunnelEndpointSide::Left,
            plan,
            traffic_source: AgentRuntimeTrafficSource::InterfaceCounters,
            traffic_command: None,
            latency_monitoring_enabled: true,
            auto_ospf_enabled: true,
            auto_ospf_updater: None,
        };

        let updater = effective_ospf_updater(&config, &telemetry_plan).expect("updater");

        assert_eq!(updater.argv[0], "/usr/local/libexec/agent-ospf");
    }

    #[tokio::test]
    async fn latency_monitoring_uses_fallback_and_marks_down_after_three_missed_windows() {
        let root =
            std::env::temp_dir().join(format!("vpsman-latency-monitor-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let ping = root.join("ping.sh");
        write_test_script(
            &ping,
            r#"#!/bin/sh
target=""
for arg in "$@"; do target="$arg"; done
case "$target" in
  fd00:42::1)
    printf '%s\n' '3 packets transmitted, 0 received, 100% packet loss, time 1000ms'
    exit 1
    ;;
  10.42.0.1)
    printf '%s\n' '3 packets transmitted, 3 received, 0% packet loss, time 1000ms'
    printf '%s\n' 'rtt min/avg/max/mdev = 18.000/18.400/19.000/0.100 ms'
    exit 0
    ;;
  *)
    printf '%s\n' '3 packets transmitted, 0 received, 100% packet loss, time 1000ms'
    exit 1
    ;;
esac
"#,
        );
        let mut config = AgentConfig::default();
        config.network.probe_ping_argv = vec![ping.to_string_lossy().to_string()];
        config.network.latency_down_windows = 3;
        let telemetry_plan = telemetry_plan(false, None, TunnelAddressFamily::Ipv6);
        let mut runtime_state = TelemetryRuntimeState::default();
        let mut stat = RuntimeTunnelStat::default();

        apply_latency_monitoring(
            &config,
            &telemetry_plan,
            100,
            "plan-a:left",
            &mut runtime_state,
            &mut stat,
        )
        .await;

        assert_eq!(stat.latency_status.as_deref(), Some("healthy"));
        assert_eq!(stat.latency_primary_family.as_deref(), Some("ipv4"));
        assert_eq!(stat.latency_target.as_deref(), Some("10.42.0.1"));
        assert_eq!(stat.latency_avg_ms, Some(18.4));
        assert_eq!(stat.latency_healthy_windows, Some(1));
        assert_eq!(stat.latency_missed_windows, Some(0));

        write_test_script(
            &ping,
            r#"#!/bin/sh
printf '%s\n' '3 packets transmitted, 0 received, 100% packet loss, time 1000ms'
exit 1
"#,
        );
        for (now, expected_status, expected_missed) in
            [(101, "missed", 1), (102, "missed", 2), (103, "down", 3)]
        {
            let mut stat = RuntimeTunnelStat::default();
            apply_latency_monitoring(
                &config,
                &telemetry_plan,
                now,
                "plan-a:left",
                &mut runtime_state,
                &mut stat,
            )
            .await;
            assert_eq!(stat.latency_status.as_deref(), Some(expected_status));
            assert_eq!(stat.latency_missed_windows, Some(expected_missed));
            assert_eq!(
                stat.latency_reason.as_deref(),
                Some("primary_ipv6_and_fallback_ipv4_unhealthy")
            );
        }

        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn auto_ospf_gates_external_updater_and_reports_last_update() {
        let root = std::env::temp_dir().join(format!("vpsman-auto-ospf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let updater = root.join("ospf-updater.sh");
        let args_path = root.join("args.log");
        let payload_path = root.join("payload.json");
        write_test_script(
            &updater,
            r#"#!/bin/sh
args_file="$1"
payload_file="$2"
shift 2
printf '%s\n' 'RUN' >> "$args_file"
printf '%s\n' "$@" >> "$args_file"
cat > "$payload_file"
"#,
        );
        let updater_command = RuntimeTunnelCommand {
            argv: vec![
                updater.to_string_lossy().to_string(),
                args_path.to_string_lossy().to_string(),
                payload_path.to_string_lossy().to_string(),
                "{interface}".to_string(),
                "{current_ospf_cost}".to_string(),
                "{recommended_ospf_cost}".to_string(),
                "{latency_avg_ms}".to_string(),
                "{packet_loss_ratio}".to_string(),
                "{latency_family}".to_string(),
                "{latency_target}".to_string(),
            ],
            max_timeout_secs: 2,
            max_output_bytes: 1024,
        };
        let mut config = AgentConfig {
            client_id: "edge-a".to_string(),
            ..AgentConfig::default()
        };
        config.network.auto_ospf_enabled = true;
        config.network.auto_ospf_healthy_windows = 2;
        config.network.auto_ospf_min_cost_delta = 5;
        let telemetry_plan = telemetry_plan(true, Some(updater_command), TunnelAddressFamily::Ipv4);
        let probe = LatencyProbeResult {
            family: TunnelAddressFamily::Ipv4,
            target: "10.42.0.1".to_string(),
            healthy: true,
            latency_avg_ms: Some(12.5),
            packet_loss_ratio: Some(0.0),
            reason: None,
        };
        let mut state = LatencyMonitorState {
            healthy_windows: 1,
            missed_windows: 0,
            last_cost: Some(65_535),
            last_update_unix: None,
        };
        let mut stat = RuntimeTunnelStat::default();

        apply_auto_ospf(&config, &telemetry_plan, 200, &mut state, &mut stat, &probe).await;

        assert_eq!(stat.auto_ospf_status.as_deref(), Some("stabilizing"));
        assert!(!args_path.exists());

        state.healthy_windows = 2;
        let mut stat = RuntimeTunnelStat::default();
        apply_auto_ospf(&config, &telemetry_plan, 240, &mut state, &mut stat, &probe).await;

        assert_eq!(stat.auto_ospf_status.as_deref(), Some("updated"));
        assert_eq!(stat.auto_ospf_current_cost, Some(65_535));
        let recommended = stat.auto_ospf_recommended_cost.expect("recommended cost");
        assert_eq!(state.last_cost, Some(recommended));
        assert_eq!(state.last_update_unix, Some(240));
        assert_eq!(stat.auto_ospf_updated_unix, Some(240));
        let args = std::fs::read_to_string(&args_path).unwrap();
        assert!(args.contains("RUN\n"));
        assert!(args.contains("tunab\n"));
        assert!(args.contains("65535\n"));
        assert!(args.contains(&format!("{recommended}\n")));
        assert!(args.contains("12.500\n"));
        assert!(args.contains("0.000\n"));
        assert!(args.contains("ipv4\n"));
        assert!(args.contains("10.42.0.1\n"));
        let payload: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&payload_path).unwrap()).unwrap();
        assert_eq!(payload["type"], "network_auto_ospf_cost_update");
        assert_eq!(payload["plan"], "edge-a-b");
        assert_eq!(payload["interface"], "tunab");
        assert_eq!(payload["side"], "left");
        assert_eq!(payload["client_id"], "edge-a");
        assert_eq!(payload["peer_client_id"], "edge-b");
        assert_eq!(payload["local_underlay"], "198.51.100.10");
        assert_eq!(payload["remote_underlay"], "203.0.113.20");
        assert_eq!(payload["local_address"], "10.42.0.0");
        assert_eq!(payload["remote_address"], "10.42.0.1");
        assert_eq!(payload["ipv4"]["left"], "10.42.0.0");
        assert_eq!(payload["ipv6"]["right"], "fd00:42::1");
        assert_eq!(payload["latency"]["family"], "ipv4");
        assert_eq!(payload["latency"]["target"], "10.42.0.1");
        assert_eq!(payload["current_ospf_cost"], 65_535);
        assert_eq!(payload["recommended_ospf_cost"], recommended);
        assert_eq!(payload["reason"], "latency_and_configured_bandwidth_tier");

        let mut stat = RuntimeTunnelStat::default();
        apply_auto_ospf(&config, &telemetry_plan, 300, &mut state, &mut stat, &probe).await;

        assert_eq!(stat.auto_ospf_status.as_deref(), Some("stable"));
        assert_eq!(stat.auto_ospf_updated_unix, Some(240));
        assert_eq!(
            std::fs::read_to_string(&args_path)
                .unwrap()
                .lines()
                .filter(|line| *line == "RUN")
                .count(),
            1
        );

        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn unhealthy_latency_reports_only_and_does_not_run_auto_ospf_updater() {
        let root =
            std::env::temp_dir().join(format!("vpsman-auto-ospf-down-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let updater = root.join("ospf-updater.sh");
        let args_path = root.join("args.log");
        write_test_script(
            &updater,
            r#"#!/bin/sh
printf '%s\n' 'RUN' >> "$1"
"#,
        );
        let mut config = AgentConfig::default();
        config.network.auto_ospf_enabled = true;
        let telemetry_plan = telemetry_plan(
            true,
            Some(RuntimeTunnelCommand {
                argv: vec![
                    updater.to_string_lossy().to_string(),
                    args_path.to_string_lossy().to_string(),
                ],
                max_timeout_secs: 2,
                max_output_bytes: 1024,
            }),
            TunnelAddressFamily::Ipv4,
        );
        let mut state = LatencyMonitorState {
            healthy_windows: 2,
            missed_windows: 3,
            last_cost: Some(80),
            last_update_unix: Some(120),
        };
        let probe = LatencyProbeResult {
            family: TunnelAddressFamily::Ipv4,
            target: "10.42.0.1".to_string(),
            healthy: false,
            latency_avg_ms: None,
            packet_loss_ratio: Some(1.0),
            reason: Some("latency_probe_missing_healthy_sample:3/3".to_string()),
        };
        let mut stat = RuntimeTunnelStat::default();

        apply_auto_ospf(&config, &telemetry_plan, 180, &mut state, &mut stat, &probe).await;

        assert_eq!(stat.auto_ospf_status.as_deref(), Some("report_only"));
        assert_eq!(
            stat.auto_ospf_reason.as_deref(),
            Some("latency_probe_unhealthy_ospf_handles_dead_adjacency")
        );
        assert_eq!(stat.auto_ospf_updated_unix, Some(120));
        assert!(!args_path.exists());

        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn approved_adapter_status_telemetry_reports_redacted_health_and_custom_traffic() {
        let root =
            std::env::temp_dir().join(format!("vpsman-adapter-telemetry-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let adapter = root.join("adapter-status.sh");
        let traffic = root.join("adapter-traffic.sh");
        let run_log = root.join("runs.log");
        std::fs::write(&adapter, TEST_ADAPTER_STATUS_SCRIPT).unwrap();
        std::fs::write(&traffic, TEST_TRAFFIC_SOURCE_SCRIPT).unwrap();
        let mut permissions = std::fs::metadata(&adapter).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&adapter, permissions).unwrap();
        let mut permissions = std::fs::metadata(&traffic).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&traffic, permissions).unwrap();

        let plan = plan_tunnel(&TunnelPlanInput {
            name: "adapter-a-b".to_string(),
            interface_name: "ovpn42".to_string(),
            kind: TunnelKind::Openvpn,
            runtime_control: RuntimeTunnelControl {
                manager: RuntimeTunnelManager::ExternalManagedAdapter,
                status: Some(RuntimeTunnelCommand {
                    argv: vec![
                        adapter.to_string_lossy().to_string(),
                        "{interface}".to_string(),
                        "{peer_client_id}".to_string(),
                        run_log.to_string_lossy().to_string(),
                    ],
                    max_timeout_secs: 2,
                    max_output_bytes: 1024,
                }),
                ..Default::default()
            },
            runtime_topology: Default::default(),
            left_client_id: "edge-a".to_string(),
            right_client_id: "edge-b".to_string(),
            left_underlay: "198.51.100.10".to_string(),
            right_underlay: "198.51.100.11".to_string(),
            address_pool_cidr: "10.42.0.0/30".to_string(),
            reserved_addresses: Vec::new(),
            ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
                left: "10.42.0.0".to_string(),
                right: "10.42.0.1".to_string(),
                prefix_len: 31,
            }),
            ipv6_address_pool_cidr: None,
            ipv6_tunnel: None,
            latency_primary_family: Default::default(),
            bandwidth: BandwidthTier::M100,
            latency_ms: 12.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            ospf_policy: OspfCostPolicy::default(),
        })
        .unwrap();
        let mut config = AgentConfig::default();
        config.network.runtime_status_telemetry_plans = vec![AgentRuntimeStatusTelemetryPlan {
            plan_id: Some("plan-a".to_string()),
            endpoint_side: TunnelEndpointSide::Left,
            plan,
            traffic_source: AgentRuntimeTrafficSource::CustomCommand,
            traffic_command: Some(RuntimeTunnelCommand {
                argv: vec![
                    traffic.to_string_lossy().to_string(),
                    "{interface}".to_string(),
                ],
                max_timeout_secs: 2,
                max_output_bytes: 1024,
            }),
            latency_monitoring_enabled: true,
            auto_ospf_enabled: false,
            auto_ospf_updater: None,
        }];
        let mut runtime_state = TelemetryRuntimeState::default();

        let metrics = collect_metrics_for_config(&config, &mut runtime_state)
            .await
            .unwrap();
        let tunnel = metrics
            .tunnels
            .iter()
            .find(|tunnel| tunnel.interface == "ovpn42")
            .unwrap();
        let health = tunnel.adapter_health.as_ref().unwrap();

        assert_eq!(tunnel.ownership_mode, "external_managed_adapter");
        assert_eq!(tunnel.mutation_policy, "managed_desired");
        assert!(!tunnel.promotion_required);
        assert_eq!(tunnel.plan_id.as_deref(), Some("plan-a"));
        assert_eq!(tunnel.peer_client_id.as_deref(), Some("edge-b"));
        assert_eq!(tunnel.rx_bytes, 1234);
        assert_eq!(tunnel.tx_bytes, 5678);
        assert_eq!(tunnel.traffic_source.as_deref(), Some("custom_command"));
        assert_eq!(tunnel.traffic_status.as_deref(), Some("ok"));
        assert_eq!(health.status, "healthy");
        assert!(health.configured);
        assert!(health.success);
        assert_eq!(health.exit_code, Some(0));
        assert!(health.command_sha256_hex.is_some());
        assert!(health.stdout_sha256_hex.is_some());
        assert!(health.stderr_sha256_hex.is_some());

        let metrics = collect_metrics_for_config(&config, &mut runtime_state)
            .await
            .unwrap();
        let tunnel = metrics
            .tunnels
            .iter()
            .find(|tunnel| tunnel.interface == "ovpn42")
            .unwrap();
        assert_eq!(tunnel.adapter_health.as_ref().unwrap().status, "healthy");
        assert_eq!(tunnel.rx_bytes, 1234);
        assert_eq!(
            std::fs::read_to_string(&run_log).unwrap().lines().count(),
            1
        );

        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn custom_traffic_timeout_covers_wait_after_stdout_closes() {
        let root =
            std::env::temp_dir().join(format!("vpsman-traffic-timeout-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let traffic = root.join("traffic-timeout.sh");
        write_test_script(
            &traffic,
            "#!/bin/sh\nprintf '{\"rx_bytes\":1234,\"tx_bytes\":5678}\\n'\nexec 1>&-\nsleep 10\n",
        );
        let mut config = AgentConfig::default();
        config.network.runtime_status_telemetry_plans = vec![AgentRuntimeStatusTelemetryPlan {
            plan_id: Some("plan-a".to_string()),
            endpoint_side: TunnelEndpointSide::Left,
            plan: telemetry_dual_stack_plan(TunnelAddressFamily::Ipv4),
            traffic_source: AgentRuntimeTrafficSource::CustomCommand,
            traffic_command: Some(RuntimeTunnelCommand {
                argv: vec![traffic.to_string_lossy().to_string()],
                max_timeout_secs: 1,
                max_output_bytes: 1024,
            }),
            latency_monitoring_enabled: false,
            auto_ospf_enabled: false,
            auto_ospf_updater: None,
        }];
        let mut runtime_state = TelemetryRuntimeState::default();
        let started = std::time::Instant::now();

        let metrics = collect_metrics_for_config(&config, &mut runtime_state)
            .await
            .unwrap();

        assert!(started.elapsed() < std::time::Duration::from_secs(4));
        let tunnel = metrics
            .tunnels
            .iter()
            .find(|tunnel| tunnel.interface == "tunab")
            .unwrap();
        assert_eq!(tunnel.traffic_status.as_deref(), Some("failed"));
        assert!(tunnel
            .traffic_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("traffic telemetry timed out")));
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn custom_metrics_source_replaces_linux_snapshot() {
        let root =
            std::env::temp_dir().join(format!("vpsman-custom-metrics-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("metrics-source.sh");
        std::fs::write(&source, TEST_CUSTOM_METRICS_SOURCE_SCRIPT).unwrap();
        let mut permissions = std::fs::metadata(&source).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&source, permissions).unwrap();
        let config = AgentConfig {
            telemetry: AgentTelemetryConfig {
                source: AgentTelemetrySource::CustomCommand,
                custom_metrics_command: Some(RuntimeTunnelCommand {
                    argv: vec![source.to_string_lossy().to_string()],
                    max_timeout_secs: 2,
                    max_output_bytes: 4096,
                }),
                ..AgentTelemetryConfig::default()
            },
            ..AgentConfig::default()
        };
        let mut runtime_state = TelemetryRuntimeState::default();

        let metrics = collect_metrics_for_config(&config, &mut runtime_state)
            .await
            .unwrap();

        assert_eq!(metrics.hostname, "custom-edge");
        assert_eq!(metrics.uptime_secs, 42);
        assert_eq!(metrics.cpu.cores, 2);
        assert_eq!(metrics.memory.available_bytes, 512);
        assert_eq!(metrics.networks.len(), 1);
        assert_eq!(metrics.networks[0].interface, "edge0");

        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn custom_metrics_timeout_covers_wait_after_stdout_closes() {
        let root = std::env::temp_dir().join(format!(
            "vpsman-custom-metrics-timeout-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("metrics-timeout.sh");
        write_test_script(
            &source,
            "#!/bin/sh\nprintf '{\"hostname\":\"late\"}\\n'\nexec 1>&-\nsleep 10\n",
        );
        let config = AgentConfig {
            telemetry: AgentTelemetryConfig {
                source: AgentTelemetrySource::CustomCommand,
                custom_metrics_command: Some(RuntimeTunnelCommand {
                    argv: vec![source.to_string_lossy().to_string()],
                    max_timeout_secs: 1,
                    max_output_bytes: 4096,
                }),
                ..AgentTelemetryConfig::default()
            },
            ..AgentConfig::default()
        };
        let mut runtime_state = TelemetryRuntimeState::default();
        let started = std::time::Instant::now();

        let metrics = collect_metrics_for_config(&config, &mut runtime_state)
            .await
            .unwrap();

        assert!(started.elapsed() < std::time::Duration::from_secs(4));
        assert_eq!(metrics.hostname, "unknown");
        std::fs::remove_dir_all(root).ok();
    }
}
