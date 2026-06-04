use std::{
    collections::{HashMap, HashSet},
    ffi::CString,
    path::Path,
    process::{ExitStatus, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    task::JoinHandle,
    time::{self, Duration, Instant},
};
use tracing::debug;
use vpsman_common::{
    render_tunnel_endpoint_config, AgentConfig, AgentMetrics, AgentRuntimeStatusTelemetryPlan,
    CpuStat, DiskStat, LoadAverage, MemoryStat, NetworkStat, RuntimeTunnelAdapterHealthStat,
    RuntimeTunnelManager, RuntimeTunnelStat, TunnelEndpointSide, TunnelKind,
};

use crate::network_runtime::render_runtime_adapter_command;
use crate::telemetry_custom::{
    apply_custom_metrics_if_configured, custom_metrics_replaces_linux,
    empty_custom_metrics_snapshot,
};
use crate::telemetry_traffic::traffic_accumulation_for_plan;

#[derive(Default)]
pub(crate) struct TelemetryRuntimeState {
    last_adapter_check_unix: HashMap<String, u64>,
    cached_adapter_tunnels: HashMap<String, RuntimeTunnelStat>,
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
        return;
    }
    let now = metrics.observed_unix;
    let interval = config
        .network
        .runtime_status_telemetry_interval_secs
        .clamp(15, 3600);
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
            let stat =
                runtime_status_telemetry_stat(config, telemetry_plan, now, interface_counter).await;
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
    let timeout_secs = command
        .timeout_secs
        .min(config.network.runtime_command_timeout_secs)
        .clamp(1, 30);
    let max_output_bytes = usize::try_from(
        command
            .max_output_bytes
            .min(config.network.runtime_command_max_output_bytes)
            .clamp(1024, 64 * 1024),
    )
    .unwrap_or(16 * 1024);
    match run_adapter_status_telemetry(&argv, timeout_secs, max_output_bytes, now).await {
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

async fn run_adapter_status_telemetry(
    argv: &[String],
    timeout_secs: u64,
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
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
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
        RuntimeTunnelCommand, RuntimeTunnelControl, RuntimeTunnelManager, TunnelEndpointSide,
        TunnelKind, TunnelPlanInput,
    };

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
                    timeout_secs: 2,
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
                timeout_secs: 2,
                max_output_bytes: 1024,
            }),
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
                    timeout_secs: 2,
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
}
