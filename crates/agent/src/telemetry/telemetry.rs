use std::{
    collections::{HashMap, HashSet},
    ffi::CString,
    path::Path,
    process::{ExitStatus, Stdio},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    net::TcpStream,
    process::Command,
    sync::Semaphore,
    task::JoinHandle,
    time::{self, Duration, Instant},
};
use vpsman_common::{
    render_tunnel_endpoint_config, AgentConfig, AgentMetrics, AgentPingProbeKind, AgentPingTarget,
    AgentRuntimeStatusTelemetryPlan, ConnectionStat, CpuStat, DiskStat, LoadAverage, MemoryStat,
    NetworkStat, PingTargetResult, RuntimeTunnelAdapterHealthStat, RuntimeTunnelManager,
    RuntimeTunnelStat, TunnelAddressFamily, TunnelEndpointSide, TunnelKind, MAX_TELEMETRY_DISKS,
    MAX_TELEMETRY_NETWORKS, MAX_TELEMETRY_PING_RESULTS, MAX_TELEMETRY_TUNNELS,
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
pub(crate) const GENERAL_PING_INTERVAL_SECS: u64 = 60;
const GENERAL_PING_MAX_ATTEMPT_TIMEOUT_SECS: u64 = 8;

#[derive(Default)]
pub(crate) struct TelemetryRuntimeState {
    cpu_time_counters: Option<CpuTimeCounters>,
    connection_collection_failed: bool,
    last_adapter_check_unix: HashMap<String, u64>,
    cached_adapter_tunnels: HashMap<String, RuntimeTunnelStat>,
    latency_monitors: HashMap<String, LatencyMonitorState>,
    last_ping_check_unix: HashMap<String, u64>,
    cached_ping_results: HashMap<String, PingTargetResult>,
}

#[derive(Clone, Debug, Default)]
struct LatencyMonitorState {
    healthy_windows: u8,
    missed_windows: u8,
}

const PROC_STAT_CPU_COUNTER_COUNT: usize = 8;
const PROC_STAT_IDLE_INDEX: usize = 3;
const PROC_STAT_IOWAIT_INDEX: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CpuTimeCounters {
    // user, nice, system, idle, iowait, irq, softirq, steal. Linux reports
    // guest counters inside user/nice already, so including them would count
    // the same CPU time twice.
    values: [u64; PROC_STAT_CPU_COUNTER_COUNT],
}

impl CpuTimeCounters {
    fn utilization_ratio_since(self, previous: Self) -> Option<f64> {
        let mut total_delta = 0_u64;
        let mut idle_delta = 0_u64;
        for (index, (current, previous)) in self.values.into_iter().zip(previous.values).enumerate()
        {
            let delta = current.checked_sub(previous)?;
            total_delta = total_delta.checked_add(delta)?;
            if matches!(index, PROC_STAT_IDLE_INDEX | PROC_STAT_IOWAIT_INDEX) {
                idle_delta = idle_delta.checked_add(delta)?;
            }
        }
        if total_delta == 0 {
            return None;
        }
        let busy_delta = total_delta.checked_sub(idle_delta)?;
        Some(((busy_delta as f64) / (total_delta as f64)).clamp(0.0, 1.0))
    }
}

fn collect_linux_metrics(
    config: &AgentConfig,
    runtime_state: &mut TelemetryRuntimeState,
) -> Result<AgentMetrics> {
    let proc_root = Path::new(&config.telemetry.proc_root);
    let networks = network_stats(proc_root)?;
    let connections = match connection_stats(proc_root) {
        Ok(connections) => {
            if runtime_state.connection_collection_failed {
                tracing::info!("Linux socket telemetry collection recovered");
            }
            runtime_state.connection_collection_failed = false;
            Some(connections)
        }
        Err(error) => {
            if !runtime_state.connection_collection_failed {
                tracing::warn!(%error, "Linux socket telemetry is unavailable");
            }
            runtime_state.connection_collection_failed = true;
            None
        }
    };
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
            utilization_ratio: cpu_utilization_ratio(proc_root, runtime_state),
        },
        memory: memory_stat(proc_root)?,
        disks: disk_stats(proc_root)?,
        networks,
        connections,
        tunnels: Vec::new(),
        ping_results: Vec::new(),
        port_forwarding: None,
    })
}

fn connection_stats(proc_root: &Path) -> Result<ConnectionStat> {
    Ok(ConnectionStat {
        tcp: socket_protocol_count(proc_root, "tcp")?,
        udp: socket_protocol_count(proc_root, "udp")?,
    })
}

fn socket_protocol_count(proc_root: &Path, protocol: &str) -> Result<u64> {
    let mut found = false;
    let mut total = 0_u64;
    for table in [protocol.to_string(), format!("{protocol}6")] {
        let path = proc_root.join("net").join(&table);
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("failed to read {}", path.display()))
            }
        };
        found = true;
        let mut lines = content.lines().filter(|line| !line.trim().is_empty());
        lines
            .next()
            .with_context(|| format!("{} has no socket-table header", path.display()))?;
        let count = u64::try_from(lines.count()).context("socket table row count overflow")?;
        total = total
            .checked_add(count)
            .context("combined socket table row count overflow")?;
    }
    ensure!(found, "Linux {protocol} socket tables are missing");
    Ok(total)
}

pub(crate) async fn collect_metrics_for_config(
    config: &AgentConfig,
    runtime_state: &mut TelemetryRuntimeState,
) -> Result<AgentMetrics> {
    let mut metrics = if custom_metrics_replaces_linux(config) {
        runtime_state.cpu_time_counters = None;
        empty_custom_metrics_snapshot(unix_now())
    } else {
        collect_linux_metrics(config, runtime_state)?
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
    collect_ping_target_telemetry(config, &mut metrics, runtime_state).await;
    metrics.port_forwarding = Some(inspect_port_forwarding(&config.network.port_forwarding).await);
    metrics.disks.truncate(MAX_TELEMETRY_DISKS);
    metrics.networks.truncate(MAX_TELEMETRY_NETWORKS);
    metrics.tunnels.truncate(MAX_TELEMETRY_TUNNELS);
    metrics
        .ping_results
        .truncate(vpsman_common::MAX_TELEMETRY_PING_RESULTS);
    Ok(metrics)
}

#[derive(Clone)]
struct PingProbeSettings {
    ping_argv: Vec<String>,
    timeout_secs: u64,
}

async fn collect_ping_target_telemetry(
    config: &AgentConfig,
    metrics: &mut AgentMetrics,
    runtime_state: &mut TelemetryRuntimeState,
) {
    let desired = config
        .network
        .ping_targets
        .iter()
        .map(ping_target_key)
        .collect::<HashSet<_>>();
    runtime_state
        .last_ping_check_unix
        .retain(|key, _| desired.contains(key));
    runtime_state
        .cached_ping_results
        .retain(|key, _| desired.contains(key));

    let now = metrics.observed_unix;
    let interval = GENERAL_PING_INTERVAL_SECS;
    let due = config
        .network
        .ping_targets
        .iter()
        .filter(|target| {
            let key = ping_target_key(target);
            runtime_state
                .last_ping_check_unix
                .get(&key)
                .is_none_or(|last| now.saturating_sub(*last) >= interval)
        })
        .cloned()
        .collect::<Vec<_>>();

    if !due.is_empty() {
        let settings = PingProbeSettings {
            ping_argv: config.network.probe_ping_argv.clone(),
            // Sixteen targets run in at most two batches of eight. Keeping
            // each three-attempt probe bounded below the 60-second cadence
            // prevents a failed fleet from indefinitely delaying the next run.
            timeout_secs: config
                .network
                .status_probe_timeout_secs
                .clamp(1, GENERAL_PING_MAX_ATTEMPT_TIMEOUT_SECS),
        };
        let semaphore = Arc::new(Semaphore::new(8));
        let mut tasks = tokio::task::JoinSet::new();
        for target in due {
            let settings = settings.clone();
            let semaphore = semaphore.clone();
            tasks.spawn(async move {
                let key = ping_target_key(&target);
                let _permit = semaphore.acquire_owned().await.ok();
                let result = run_general_ping_probe(&settings, &target, now).await;
                (key, result)
            });
        }
        while let Some(joined) = tasks.join_next().await {
            if let Ok((key, result)) = joined {
                runtime_state.last_ping_check_unix.insert(key.clone(), now);
                runtime_state.cached_ping_results.insert(key, result);
            }
        }
    }

    metrics.ping_results = config
        .network
        .ping_targets
        .iter()
        .filter_map(|target| {
            runtime_state
                .cached_ping_results
                .get(&ping_target_key(target))
        })
        .take(MAX_TELEMETRY_PING_RESULTS)
        .cloned()
        .collect();
}

fn ping_target_key(target: &AgentPingTarget) -> String {
    format!("{}:{}", target.id, target.generation)
}

async fn run_general_ping_probe(
    settings: &PingProbeSettings,
    target: &AgentPingTarget,
    now: u64,
) -> PingTargetResult {
    match target.kind {
        AgentPingProbeKind::Icmp => run_general_icmp_probe(settings, target, now).await,
        AgentPingProbeKind::Tcp => run_general_tcp_probe(settings, target, now).await,
    }
}

async fn run_general_icmp_probe(
    settings: &PingProbeSettings,
    target: &AgentPingTarget,
    now: u64,
) -> PingTargetResult {
    let mut argv = if settings.ping_argv.is_empty() {
        ["/bin/ping", "/usr/bin/ping"]
            .into_iter()
            .find(|path| Path::new(path).exists())
            .map(|path| vec![path.to_string()])
            .unwrap_or_default()
    } else {
        settings.ping_argv.clone()
    };
    if argv.is_empty() {
        return failed_ping_result(target, now, "ping_binary_not_found", "error");
    }
    argv.extend([
        "-n".to_string(),
        "-c".to_string(),
        "3".to_string(),
        "-i".to_string(),
        "0.500".to_string(),
        "-W".to_string(),
        settings.timeout_secs.to_string(),
        target.host.clone(),
    ]);
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]).stdin(Stdio::null());
    let timeout_secs = settings.timeout_secs.saturating_mul(3).saturating_add(2);
    match run_child_with_bounded_output(
        command,
        timeout_secs,
        MAX_LATENCY_PROBE_OUTPUT_BYTES,
        ChildCleanupPolicy::ProcessGroup,
    )
    .await
    {
        Ok(ChildRunResult::Completed(output)) => {
            let parsed = parse_latency_ping_output(&String::from_utf8_lossy(&output.stdout));
            let loss_ratio = parsed.packet_loss_ratio.unwrap_or(1.0).clamp(0.0, 1.0);
            let output_limited = output.stdout_truncated || output.stderr_truncated;
            let (status, reason) = if output_limited {
                ("error", Some("ping_output_limit".to_string()))
            } else if parsed.latency_avg_ms.is_some() && loss_ratio == 0.0 {
                ("ok", None)
            } else if parsed.latency_avg_ms.is_some() {
                ("degraded", Some("packet_loss".to_string()))
            } else {
                ("down", Some(format!("ping_exit:{:?}", output.exit_code)))
            };
            let latency_avg_ms = if matches!(status, "ok" | "degraded") {
                parsed.latency_avg_ms
            } else {
                None
            };
            let loss_ratio = if status == "error" { 1.0 } else { loss_ratio };
            PingTargetResult {
                target_id: target.id.clone(),
                generation: target.generation,
                checked_unix: now,
                status: status.to_string(),
                latency_avg_ms,
                loss_ratio,
                reason,
            }
        }
        Ok(ChildRunResult::TimedOut(_)) => failed_ping_result(target, now, "ping_timeout", "down"),
        Ok(ChildRunResult::Canceled { .. }) => {
            failed_ping_result(target, now, "ping_canceled", "error")
        }
        Err(_) => failed_ping_result(target, now, "ping_spawn_failed", "error"),
    }
}

async fn run_general_tcp_probe(
    settings: &PingProbeSettings,
    target: &AgentPingTarget,
    now: u64,
) -> PingTargetResult {
    let Some(port) = target.port else {
        return failed_ping_result(target, now, "tcp_port_missing", "error");
    };
    let timeout = Duration::from_secs(settings.timeout_secs);
    let mut success_count = 0_u8;
    let mut latency_total_ms = 0.0;
    let mut last_reason = "tcp_connect_failed".to_string();
    for _ in 0..3 {
        let started = Instant::now();
        match time::timeout(timeout, TcpStream::connect((target.host.as_str(), port))).await {
            Ok(Ok(stream)) => {
                success_count = success_count.saturating_add(1);
                latency_total_ms += started.elapsed().as_secs_f64() * 1000.0;
                drop(stream);
            }
            Ok(Err(error)) => {
                last_reason = format!("tcp_connect_failed:{}", error.kind());
            }
            Err(_) => last_reason = "tcp_connect_timeout".to_string(),
        }
        time::sleep(Duration::from_millis(100)).await;
    }
    let loss_ratio = f64::from(3_u8.saturating_sub(success_count)) / 3.0;
    PingTargetResult {
        target_id: target.id.clone(),
        generation: target.generation,
        checked_unix: now,
        status: if success_count == 3 {
            "ok"
        } else if success_count > 0 {
            "degraded"
        } else {
            "down"
        }
        .to_string(),
        latency_avg_ms: (success_count > 0).then_some(latency_total_ms / f64::from(success_count)),
        loss_ratio,
        reason: (success_count < 3).then_some(last_reason),
    }
}

fn failed_ping_result(
    target: &AgentPingTarget,
    now: u64,
    reason: &str,
    status: &str,
) -> PingTargetResult {
    PingTargetResult {
        target_id: target.id.clone(),
        generation: target.generation,
        checked_unix: now,
        status: status.to_string(),
        latency_avg_ms: None,
        loss_ratio: 1.0,
        reason: Some(reason.to_string()),
    }
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

fn cpu_utilization_ratio(
    proc_root: &Path,
    runtime_state: &mut TelemetryRuntimeState,
) -> Option<f64> {
    let contents = match read_proc_file(proc_root, "stat") {
        Ok(contents) => contents,
        Err(_) => {
            runtime_state.cpu_time_counters = None;
            return None;
        }
    };
    update_cpu_utilization_ratio(&mut runtime_state.cpu_time_counters, &contents)
}

fn update_cpu_utilization_ratio(
    previous: &mut Option<CpuTimeCounters>,
    proc_stat: &str,
) -> Option<f64> {
    let current = match parse_cpu_time_counters(proc_stat) {
        Ok(current) => current,
        Err(_) => {
            *previous = None;
            return None;
        }
    };
    let ratio = previous.and_then(|value| current.utilization_ratio_since(value));
    *previous = Some(current);
    ratio
}

fn parse_cpu_time_counters(proc_stat: &str) -> Result<CpuTimeCounters> {
    let mut fields = proc_stat
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            (fields.next() == Some("cpu")).then_some(fields)
        })
        .context("telemetry CPU source is missing its aggregate row")?;
    let mut values = [0_u64; PROC_STAT_CPU_COUNTER_COUNT];
    for (index, value) in values.iter_mut().enumerate() {
        *value = fields
            .next()
            .with_context(|| format!("telemetry CPU source is missing counter {index}"))?
            .parse::<u64>()
            .with_context(|| format!("telemetry CPU counter {index} is not numeric"))?;
    }
    Ok(CpuTimeCounters { values })
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
#[path = "tests_telemetry.rs"]
mod tests;
