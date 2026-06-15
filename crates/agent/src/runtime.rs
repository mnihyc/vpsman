use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tokio::{net::TcpStream, sync::mpsc, time};
use tracing::{debug, info, warn};
#[cfg(test)]
use vpsman_common::CURRENT_COMMAND_PROTOCOL_VERSION;
use vpsman_common::{
    decode_json, decode_noise_key_hex, encode_json, job_command_min_supported_protocol_version,
    job_command_protocol_version, job_command_safety, maybe_compress_payload, payload_hash,
    AgentCapabilitySnapshot, AgentConfig, AgentHello, AgentPrivilegeMode, CommandOutput,
    CommandResume, Frame, JobAck, JobCancelAck, JobCancelRequest, JobCommand, JobCommandSafety,
    JobRequest, MessageKind, NoiseFrameStream, OutputStream, SequencedCommandOutput,
    ServerEndpoint, ServerHello, TelemetryEnvelope, TerminalStreamOutput,
};

use crate::{
    backup::{execute_backup_command, BackupCommandInput},
    config_update::{
        apply_data_source_config_patch, apply_hot_config_update, read_redacted_config,
    },
    executor::execute_job_command_with_config_and_output_sink,
    network_apply::{
        execute_network_apply_command, execute_network_ospf_cost_update_command,
        execute_network_rollback_command, NetworkApplyInput, NetworkOspfCostUpdateInput,
        NetworkRollbackInput,
    },
    network_probe::{execute_network_probe_command, NetworkProbeInput},
    network_runtime::{execute_runtime_tunnel_reconcile_report, NetworkRuntimeReconcileInput},
    network_speed::{execute_network_speed_test_command, NetworkSpeedTestInput},
    network_status::{execute_network_status_command, NetworkStatusInput},
    restore::{execute_restore_command, RestoreCommandInput},
    restore_rollback::{execute_restore_rollback_command, RestoreRollbackCommandInput},
    supervisor::reconcile_supervised_processes_on_start,
    telemetry::{collect_metrics_for_config, read_optional, TelemetryRuntimeState},
    terminal::execute_terminal_command_with_stream_sink,
    update::{execute_update_agent, execute_update_check, AgentUpdateCheckInput, AgentUpdateInput},
    update_activation::read_activation_heartbeat,
};

pub(crate) async fn run_agent(
    mut config: AgentConfig,
    config_path: PathBuf,
    endpoint_override: Option<String>,
) -> Result<()> {
    let override_endpoint = endpoint_override.map(|tcp_addr| ServerEndpoint {
        label: "override".to_string(),
        tcp_addr,
        priority: 0,
    });
    let mut command_runtime = AgentCommandRuntime::default();
    match reconcile_supervised_processes_on_start().await {
        Ok(report) => log_supervisor_startup_reconcile(&report),
        Err(error) => warn!(%error, "process supervisor startup reconcile failed"),
    }
    let startup_reconcile = reconcile_configured_runtime_tunnels(&config, "agent_start").await;
    log_configured_runtime_tunnel_reconcile(&startup_reconcile);

    loop {
        let endpoints = override_endpoint
            .as_ref()
            .map(|endpoint| vec![endpoint.clone()])
            .unwrap_or_else(|| endpoint_candidates(&config));
        if endpoints.is_empty() {
            anyhow::bail!("agent has no TCP endpoint configured");
        }

        for endpoint in &endpoints {
            match connect_and_stream(
                &mut config,
                &config_path,
                &endpoint.tcp_addr,
                &mut command_runtime,
            )
            .await
            {
                Ok(()) => warn!(label = %endpoint.label, "gateway session ended"),
                Err(error) => warn!(%error, label = %endpoint.label, "gateway session failed"),
            }
        }

        time::sleep(Duration::from_secs(config.auth.gateway_retry_secs.max(1))).await;
    }
}

fn endpoint_candidates(config: &AgentConfig) -> Vec<ServerEndpoint> {
    let mut endpoints = config.tcp_endpoints.clone();
    endpoints.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left.label.cmp(&right.label))
            .then_with(|| left.tcp_addr.cmp(&right.tcp_addr))
    });
    endpoints.dedup_by(|left, right| left.tcp_addr == right.tcp_addr);
    endpoints
}

async fn connect_and_stream(
    config: &mut AgentConfig,
    config_path: &Path,
    endpoint: &str,
    command_runtime: &mut AgentCommandRuntime,
) -> Result<()> {
    info!(%endpoint, "connecting to gateway");
    let tcp = connect_tcp_endpoint(endpoint, config.auth.gateway_connect_timeout_secs).await?;
    let mut stream = connect_noise_stream(tcp, config).await?;

    let hello = AgentHello {
        client_id: config.client_id.clone(),
        agent_version: env!("CARGO_PKG_VERSION").to_string(),
        internal_build_number: crate::build_info::agent_build_number(),
        os_release: config
            .telemetry
            .os_release_file
            .as_deref()
            .and_then(read_optional)
            .unwrap_or_default(),
        arch: std::env::consts::ARCH.to_string(),
        update_heartbeat: read_activation_heartbeat().unwrap_or_else(|error| {
            warn!(%error, "failed to read update activation heartbeat marker");
            None
        }),
        capabilities: agent_capabilities(config),
    };
    send_json_frame(&mut stream, MessageKind::ClientHello, 0, 1, &hello).await?;

    let server_hello: ServerHello = read_json_frame(&mut stream).await?;
    if !server_hello.accepted {
        anyhow::bail!("server rejected agent: {}", server_hello.message);
    }
    info!(
        server_id = %server_hello.server_id,
        server_version = %server_hello.server_version,
        server_build_number = server_hello.server_build_number,
        "gateway accepted agent"
    );

    let mut seq = 2_u64;
    resume_active_commands(&mut stream, &mut seq, command_runtime).await?;
    let mut telemetry_runtime_state = TelemetryRuntimeState::default();
    let mut ticker = time::interval(Duration::from_secs(
        server_hello.telemetry_light_secs.max(5),
    ));
    let mut unmanaged_update_schedule = UnmanagedUpdateSchedule::new(config);
    let mut unmanaged_update_sleep =
        Box::pin(time::sleep_until(unmanaged_update_schedule.next_due()));
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let metrics = collect_metrics_for_config(config, &mut telemetry_runtime_state).await?;
                let telemetry = TelemetryEnvelope {
                    client_id: config.client_id.clone(),
                    metrics,
                };
                send_json_frame(&mut stream, MessageKind::Telemetry, 0, seq, &telemetry).await?;
                seq += 1;
            }
            frame = stream.read_frame() => {
                let frame = frame?;
                match frame.kind {
                    MessageKind::Command => {
                        if handle_command_frame(
                            frame,
                            CommandFrameContext {
                                config,
                                config_path,
                                stream: &mut stream,
                                seq: &mut seq,
                                command_runtime,
                            },
                        )
                        .await? {
                            ticker = time::interval(Duration::from_secs(config.telemetry_light_secs.max(5)));
                            unmanaged_update_schedule = UnmanagedUpdateSchedule::new(config);
                            unmanaged_update_sleep.as_mut().reset(unmanaged_update_schedule.next_due());
                        }
                    }
                    MessageKind::CommandCancel => {
                        let request: JobCancelRequest = decode_json(&frame.decoded_payload()?)?;
                        handle_command_cancel_frame(
                            &mut stream,
                            &mut seq,
                            &mut command_runtime.active_commands,
                            request,
                        )
                        .await?;
                    }
                    MessageKind::Keepalive => {
                        debug!("gateway keepalive");
                    }
                    other => {
                        debug!(?other, "unhandled agent frame");
                    }
                }
            }
            event = command_runtime.command_event_rx.recv(), if !command_runtime.active_commands.is_empty() => {
                if let Some(event) = event {
                    match event {
                        CommandExecutionEvent::Output(output) => {
                            queue_active_command_output(
                                &mut stream,
                                &mut seq,
                                &mut command_runtime.active_commands,
                                output,
                            )
                            .await?;
                        }
                        CommandExecutionEvent::Finished(result) => {
                            finish_active_command(
                                &mut stream,
                                &mut seq,
                                &mut command_runtime.active_commands,
                                &mut command_runtime.recent_commands,
                                result,
                            )
                            .await?;
                        }
                    }
                }
            }
            output = command_runtime.terminal_stream_rx.recv() => {
                if let Some(output) = output {
                    send_json_frame(
                        &mut stream,
                        MessageKind::TerminalStreamOutput,
                        0,
                        seq,
                        &output,
                    )
                    .await?;
                    seq += 1;
                }
            }
            _ = &mut unmanaged_update_sleep, if command_runtime.active_commands.is_empty() && unmanaged_update_schedule.enabled(config) => {
                if unmanaged_update_schedule.due(config) {
                    unmanaged_update_schedule.mark_attempt(config);
                    unmanaged_update_sleep.as_mut().reset(unmanaged_update_schedule.next_due());
                    run_unmanaged_update_check(config).await;
                }
            }
        }
    }
}

async fn connect_tcp_endpoint(endpoint: &str, timeout_secs: u64) -> Result<TcpStream> {
    let mut addrs = tokio::net::lookup_host(endpoint)
        .await
        .with_context(|| format!("failed to resolve gateway endpoint {endpoint}"))?
        .collect::<Vec<_>>();
    if addrs.is_empty() {
        anyhow::bail!("gateway endpoint {endpoint} resolved to no addresses");
    }
    addrs.sort_by_key(address_family_order);

    let timeout = Duration::from_secs(timeout_secs.clamp(1, 300));
    let mut last_error = None;
    for addr in addrs {
        match time::timeout(timeout, TcpStream::connect(addr)).await {
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(error)) => {
                debug!(%endpoint, %addr, %error, "gateway address connect failed");
                last_error = Some(anyhow::Error::new(error));
            }
            Err(error) => {
                debug!(%endpoint, %addr, "gateway address connect timed out");
                last_error = Some(anyhow::Error::new(error));
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("gateway endpoint {endpoint} failed")))
}

fn address_family_order(addr: &SocketAddr) -> u8 {
    if addr.is_ipv4() {
        0
    } else {
        1
    }
}

async fn run_unmanaged_update_check(config: &AgentConfig) {
    let version_url = config.update.unmanaged_version_url.trim();
    if !config.update.unmanaged_enabled || version_url.is_empty() {
        return;
    }
    let job_id = uuid::Uuid::new_v4();
    info!(%job_id, %version_url, "running unmanaged agent update check");
    match execute_update_check(AgentUpdateCheckInput {
        job_id,
        version_url,
        activate: config.update.unmanaged_activate,
        restart_agent: config.update.unmanaged_restart_agent,
        trusted_artifact_signing_key_hex: config.update.trusted_artifact_signing_key_hex.as_deref(),
        timeout_secs: config.auth.command_timeout_secs.max(300),
    })
    .await
    {
        Ok(outputs) => {
            for output in outputs {
                debug!(
                    %job_id,
                    done = output.done,
                    exit_code = output.exit_code,
                    bytes = output.data.len(),
                    "unmanaged agent update check output"
                );
            }
        }
        Err(error) => warn!(%job_id, %error, "unmanaged agent update check failed"),
    }
}

async fn reconcile_configured_runtime_tunnels(
    config: &AgentConfig,
    trigger: &'static str,
) -> serde_json::Value {
    let total = config.network.runtime_status_telemetry_plans.len();
    let mut summaries = Vec::with_capacity(total);
    let mut converged = 0_u64;
    let mut observed = 0_u64;
    let mut skipped = 0_u64;
    let mut degraded = 0_u64;
    let mut failed = 0_u64;

    for telemetry_plan in &config.network.runtime_status_telemetry_plans {
        let plan = &telemetry_plan.plan;
        match execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
            config,
            plan,
            side: telemetry_plan.endpoint_side,
            timeout_secs: config.network.runtime_command_timeout_secs.max(1),
            #[cfg(test)]
            effective_uid_override: None,
        })
        .await
        {
            Ok(report) => {
                match report["status"].as_str().unwrap_or("unknown") {
                    "converged" => converged += 1,
                    "observed_only" => observed += 1,
                    "skipped" => skipped += 1,
                    "degraded_unprivileged" => degraded += 1,
                    "failed" => failed += 1,
                    _ => degraded += 1,
                }
                summaries.push(runtime_reconcile_summary(
                    trigger,
                    telemetry_plan.plan_id.as_deref(),
                    report,
                    None,
                ));
            }
            Err(error) => {
                failed += 1;
                warn!(
                    %trigger,
                    plan = %plan.name,
                    interface = %plan.interface_name,
                    side = endpoint_side_name(telemetry_plan.endpoint_side),
                    %error,
                    "configured runtime tunnel reconcile failed"
                );
                summaries.push(runtime_reconcile_summary(
                    trigger,
                    telemetry_plan.plan_id.as_deref(),
                    serde_json::json!({
                        "type": "runtime_tunnel_reconcile",
                        "status": "failed",
                        "plan": plan.name,
                        "interface": plan.interface_name,
                        "side": endpoint_side_name(telemetry_plan.endpoint_side),
                        "manager": plan.runtime_control.manager,
                    }),
                    Some(error.to_string()),
                ));
            }
        }
    }

    let status = if total == 0 {
        "skipped"
    } else if failed > 0 {
        "failed"
    } else if degraded > 0 {
        "degraded"
    } else {
        "completed"
    };
    serde_json::json!({
        "type": "configured_runtime_tunnel_reconcile",
        "trigger": trigger,
        "status": status,
        "total": total,
        "converged": converged,
        "observed": observed,
        "skipped": skipped,
        "degraded": degraded,
        "failed": failed,
        "tunnels": summaries,
    })
}

fn runtime_reconcile_summary(
    trigger: &'static str,
    plan_id: Option<&str>,
    report: serde_json::Value,
    error: Option<String>,
) -> serde_json::Value {
    serde_json::json!({
        "trigger": trigger,
        "plan_id": plan_id,
        "plan": report.get("plan").cloned().unwrap_or(serde_json::Value::Null),
        "interface": report.get("interface").cloned().unwrap_or(serde_json::Value::Null),
        "side": report.get("side").cloned().unwrap_or(serde_json::Value::Null),
        "manager": report.get("manager").cloned().unwrap_or(serde_json::Value::Null),
        "status": report.get("status").cloned().unwrap_or(serde_json::Value::Null),
        "reason": report.get("reason").cloned().unwrap_or(serde_json::Value::Null),
        "link_existed_before": report
            .get("link_existed_before")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "existing_link_validation": report
            .get("existing_link_validation")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "error": error,
    })
}

fn log_configured_runtime_tunnel_reconcile(report: &serde_json::Value) {
    let trigger = report["trigger"].as_str().unwrap_or("unknown");
    let status = report["status"].as_str().unwrap_or("unknown");
    let total = report["total"].as_u64().unwrap_or_default();
    let converged = report["converged"].as_u64().unwrap_or_default();
    let observed = report["observed"].as_u64().unwrap_or_default();
    let skipped = report["skipped"].as_u64().unwrap_or_default();
    let degraded = report["degraded"].as_u64().unwrap_or_default();
    let failed = report["failed"].as_u64().unwrap_or_default();
    if failed > 0 || degraded > 0 {
        warn!(
            %trigger,
            %status,
            total,
            converged,
            observed,
            skipped,
            degraded,
            failed,
            "configured runtime tunnel reconcile completed with issues"
        );
    } else if total > 0 {
        info!(
            %trigger,
            %status,
            total,
            converged,
            observed,
            skipped,
            "configured runtime tunnel reconcile completed"
        );
    } else {
        debug!(%trigger, %status, "no configured runtime tunnels to reconcile");
    }
}

fn log_supervisor_startup_reconcile(report: &serde_json::Value) {
    let total = report["total"].as_u64().unwrap_or_default();
    if total == 0 {
        debug!("no supervised processes to reconcile at startup");
        return;
    }
    info!(
        total,
        running = report["running"].as_u64().unwrap_or_default(),
        restarted = report["restarted"].as_u64().unwrap_or_default(),
        restart_pending = report["restart_pending"].as_u64().unwrap_or_default(),
        stopped = report["stopped"].as_u64().unwrap_or_default(),
        failed = report["failed"].as_u64().unwrap_or_default(),
        no_retries_remaining = report["no_retries_remaining"].as_u64().unwrap_or_default(),
        "process supervisor startup reconcile completed"
    );
}

fn attach_runtime_reconcile_report(
    outputs: &mut [CommandOutput],
    report: serde_json::Value,
) -> Result<()> {
    let Some(output) = outputs
        .iter_mut()
        .rev()
        .find(|output| output.stream == OutputStream::Status && output.done)
    else {
        return Ok(());
    };
    let mut status: serde_json::Value =
        serde_json::from_slice(&output.data).context("config update status output was not JSON")?;
    if let Some(object) = status.as_object_mut() {
        object.insert("runtime_reconcile".to_string(), report);
        output.data = serde_json::to_vec(&status)?;
    }
    Ok(())
}

fn endpoint_side_name(side: vpsman_common::TunnelEndpointSide) -> &'static str {
    match side {
        vpsman_common::TunnelEndpointSide::Left => "left",
        vpsman_common::TunnelEndpointSide::Right => "right",
    }
}

struct UnmanagedUpdateSchedule {
    next_due: time::Instant,
}

impl UnmanagedUpdateSchedule {
    fn new(config: &AgentConfig) -> Self {
        Self {
            next_due: next_unmanaged_update_due(config, SystemTime::now(), time::Instant::now()),
        }
    }

    fn next_due(&self) -> time::Instant {
        self.next_due
    }

    fn enabled(&self, config: &AgentConfig) -> bool {
        config.update.unmanaged_enabled
    }

    fn due(&self, config: &AgentConfig) -> bool {
        config.update.unmanaged_enabled && time::Instant::now() >= self.next_due
    }

    fn mark_attempt(&mut self, config: &AgentConfig) {
        self.next_due = next_unmanaged_update_due(config, SystemTime::now(), time::Instant::now());
    }
}

fn next_unmanaged_update_due(
    config: &AgentConfig,
    base_system: SystemTime,
    base_instant: time::Instant,
) -> time::Instant {
    let jitter = unmanaged_update_jitter(config);
    let interval = config.update.unmanaged_interval_secs.max(300);
    let base_unix = base_system
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let interval_start = (base_unix / interval) * interval;
    let slot_unix = interval_start.saturating_add(jitter.as_secs().min(interval - 1));
    let target_unix = if slot_unix <= base_unix {
        slot_unix.saturating_add(interval)
    } else {
        slot_unix
    };
    base_instant + Duration::from_secs(target_unix.saturating_sub(base_unix))
}

fn unmanaged_update_jitter(config: &AgentConfig) -> Duration {
    let jitter_secs = config.update.unmanaged_jitter_secs;
    if jitter_secs == 0 {
        return Duration::ZERO;
    }
    let unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let day = unix_secs / 86_400;
    let mut hasher = Sha256::new();
    hasher.update(config.client_id.as_bytes());
    hasher.update(config.update.unmanaged_version_url.as_bytes());
    hasher.update(day.to_le_bytes());
    let digest = hasher.finalize();
    let mut first = [0_u8; 8];
    first.copy_from_slice(&digest[..8]);
    Duration::from_secs(u64::from_le_bytes(first) % jitter_secs)
}

fn agent_capabilities(config: &AgentConfig) -> AgentCapabilitySnapshot {
    let effective_uid = unsafe { libc::geteuid() } as u32;
    let root = effective_uid == 0;
    AgentCapabilitySnapshot {
        privilege_mode: if root {
            AgentPrivilegeMode::Root
        } else {
            AgentPrivilegeMode::Unprivileged
        },
        effective_uid: Some(effective_uid),
        command_timeout_secs: config.auth.command_timeout_secs.clamp(1, 3600),
        can_attempt_privileged_ops: true,
        can_manage_runtime_tunnels: root,
        can_apply_process_limits: root,
        unprivileged_hint: (!root).then(|| {
            "agent is not running as root; root-only network, update, restore, and limit operations may report ineffective or require forced best-effort mode".to_string()
        }),
    }
}

struct ActiveCommand {
    payload_hash: String,
    command_version: u16,
    safety: JobCommandSafety,
    stream_id: u32,
    replay_outputs: Vec<SequencedCommandOutput>,
    terminal_output: Option<SequencedCommandOutput>,
    replay_output_bytes: usize,
    replay_truncated: bool,
    pending_outputs: VecDeque<SequencedCommandOutput>,
    next_output_seq: i32,
    finished: bool,
    task: tokio::task::JoinHandle<()>,
}

struct CommandExecutionResult {
    job_id: uuid::Uuid,
    result: Result<Vec<CommandOutput>>,
}

enum CommandExecutionEvent {
    Output(CommandOutput),
    Finished(CommandExecutionResult),
}

struct CommandFrameContext<'a> {
    config: &'a mut AgentConfig,
    config_path: &'a Path,
    stream: &'a mut NoiseFrameStream<TcpStream>,
    seq: &'a mut u64,
    command_runtime: &'a mut AgentCommandRuntime,
}

struct AgentCommandRuntime {
    active_commands: HashMap<uuid::Uuid, ActiveCommand>,
    recent_commands: RecentCommandCache,
    command_event_tx: mpsc::Sender<CommandExecutionEvent>,
    command_event_rx: mpsc::Receiver<CommandExecutionEvent>,
    terminal_stream_tx: mpsc::Sender<TerminalStreamOutput>,
    terminal_stream_rx: mpsc::Receiver<TerminalStreamOutput>,
}

impl Default for AgentCommandRuntime {
    fn default() -> Self {
        let (command_event_tx, command_event_rx) = mpsc::channel::<CommandExecutionEvent>(32);
        let (terminal_stream_tx, terminal_stream_rx) = mpsc::channel::<TerminalStreamOutput>(64);
        Self {
            active_commands: HashMap::new(),
            recent_commands: RecentCommandCache::default(),
            command_event_tx,
            command_event_rx,
            terminal_stream_tx,
            terminal_stream_rx,
        }
    }
}

struct RecentCommandCache {
    max_entries: usize,
    max_total_output_bytes: usize,
    max_entry_output_bytes: usize,
    current_output_bytes: usize,
    entries: HashMap<uuid::Uuid, RecentCommandEntry>,
    order: VecDeque<uuid::Uuid>,
}

#[derive(Clone)]
struct RecentCommandEntry {
    payload_hash: String,
    outputs: Vec<SequencedCommandOutput>,
    terminal_output: Option<SequencedCommandOutput>,
    output_bytes: usize,
    truncated: bool,
}

impl Default for RecentCommandCache {
    fn default() -> Self {
        Self {
            max_entries: 512,
            max_total_output_bytes: 8 * 1024 * 1024,
            max_entry_output_bytes: 1024 * 1024,
            current_output_bytes: 0,
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }
}

impl RecentCommandCache {
    fn remember(
        &mut self,
        job_id: uuid::Uuid,
        payload_hash: String,
        outputs: Vec<SequencedCommandOutput>,
        terminal_output: Option<SequencedCommandOutput>,
        truncated: bool,
    ) {
        let output_bytes = sequenced_command_outputs_bytes(&outputs);
        let replay_truncated = truncated || output_bytes > self.max_entry_output_bytes;
        let (outputs, output_bytes) = if replay_truncated {
            (Vec::new(), 0)
        } else {
            (outputs, output_bytes)
        };
        if let Some(existing) = self.entries.remove(&job_id) {
            self.current_output_bytes = self
                .current_output_bytes
                .saturating_sub(existing.output_bytes);
            self.order.retain(|candidate| *candidate != job_id);
        }
        while self.current_output_bytes.saturating_add(output_bytes) > self.max_total_output_bytes {
            if let Some(expired) = self.order.pop_front() {
                if let Some(expired) = self.entries.remove(&expired) {
                    self.current_output_bytes = self
                        .current_output_bytes
                        .saturating_sub(expired.output_bytes);
                }
            } else {
                break;
            }
        }
        if !self.entries.contains_key(&job_id) {
            self.order.push_back(job_id);
        }
        self.current_output_bytes = self.current_output_bytes.saturating_add(output_bytes);
        self.entries.insert(
            job_id,
            RecentCommandEntry {
                payload_hash,
                outputs,
                terminal_output,
                output_bytes,
                truncated: replay_truncated,
            },
        );
        while self.order.len() > self.max_entries {
            if let Some(expired) = self.order.pop_front() {
                if let Some(expired) = self.entries.remove(&expired) {
                    self.current_output_bytes = self
                        .current_output_bytes
                        .saturating_sub(expired.output_bytes);
                }
            }
        }
    }

    fn get(&self, job_id: uuid::Uuid) -> Option<&RecentCommandEntry> {
        self.entries.get(&job_id)
    }
}

fn sequenced_command_outputs_bytes(outputs: &[SequencedCommandOutput]) -> usize {
    outputs.iter().map(|output| output.output.data.len()).sum()
}

fn capture_replay_output(active: &mut ActiveCommand, output: &SequencedCommandOutput) {
    if output.output.done {
        active.terminal_output = Some(compact_terminal_replay_output(output));
    }
    if active.replay_truncated {
        return;
    }
    let output_bytes = output.output.data.len();
    if active.replay_output_bytes.saturating_add(output_bytes) > 1024 * 1024 {
        active.replay_outputs.clear();
        active.replay_output_bytes = 0;
        active.replay_truncated = true;
        return;
    }
    active.replay_output_bytes = active.replay_output_bytes.saturating_add(output_bytes);
    active.replay_outputs.push(output.clone());
}

fn compact_terminal_replay_output(output: &SequencedCommandOutput) -> SequencedCommandOutput {
    let status = match output.output.exit_code {
        Some(0) => "completed",
        Some(_) | None => "failed",
    };
    let data = serde_json::to_vec(&serde_json::json!({
        "type": "duplicate_job_replay_unavailable",
        "status": status,
        "job_id": output.output.job_id,
        "reason": "recent_command_replay_truncated",
        "original_stream": output_stream_name(output.output.stream),
        "original_data_size_bytes": output.output.data.len(),
        "original_data_sha256_hex": payload_hash(&output.output.data),
    }))
    .unwrap_or_else(|_| b"{\"type\":\"duplicate_job_replay_unavailable\"}".to_vec());
    SequencedCommandOutput {
        seq: output.seq,
        output: CommandOutput {
            job_id: output.output.job_id,
            stream: OutputStream::Status,
            data,
            exit_code: output.output.exit_code,
            done: true,
        },
    }
}

fn output_stream_name(stream: OutputStream) -> &'static str {
    match stream {
        OutputStream::Stdout => "stdout",
        OutputStream::Stderr => "stderr",
        OutputStream::Status => "status",
        OutputStream::Pty => "pty",
    }
}

fn command_result_outputs(
    job_id: uuid::Uuid,
    result: Result<Vec<CommandOutput>>,
) -> Vec<CommandOutput> {
    match result {
        Ok(outputs) => outputs,
        Err(error) => vec![CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: format!("command failed: {error}").into_bytes(),
            exit_code: Some(127),
            done: true,
        }],
    }
}

fn remember_recent_command_outputs(
    cache: &mut RecentCommandCache,
    job_id: uuid::Uuid,
    payload_hash: String,
    outputs: &[CommandOutput],
) {
    let replay_outputs = sequenced_outputs_starting_at(0, outputs);
    let terminal_output = terminal_replay_output_from(&replay_outputs);
    cache.remember(job_id, payload_hash, replay_outputs, terminal_output, false);
}

fn sequenced_outputs_starting_at(
    start_output_seq: i32,
    outputs: &[CommandOutput],
) -> Vec<SequencedCommandOutput> {
    outputs
        .iter()
        .enumerate()
        .map(|(offset, output)| SequencedCommandOutput {
            seq: start_output_seq.saturating_add(i32::try_from(offset).unwrap_or(i32::MAX)),
            output: output.clone(),
        })
        .collect()
}

fn terminal_replay_output_from(
    outputs: &[SequencedCommandOutput],
) -> Option<SequencedCommandOutput> {
    outputs
        .iter()
        .rev()
        .find(|output| output.output.done)
        .map(compact_terminal_replay_output)
}

async fn connect_noise_stream(
    tcp: TcpStream,
    config: &AgentConfig,
) -> Result<NoiseFrameStream<TcpStream>> {
    let client_private = config
        .noise
        .client_private_key_hex
        .as_deref()
        .context("noise enrolled_ik requires client_private_key_hex")?;
    let server_public = config
        .noise
        .server_public_key_hex
        .as_deref()
        .context("noise enrolled_ik requires server_public_key_hex")?;
    let client_private = decode_noise_key_hex(client_private)?;
    let server_public = decode_noise_key_hex(server_public)?;
    NoiseFrameStream::client_enrolled(tcp, &client_private, &server_public)
        .await
        .map_err(Into::into)
}

async fn send_json_frame<T: serde::Serialize>(
    stream: &mut NoiseFrameStream<TcpStream>,
    kind: MessageKind,
    stream_id: u32,
    seq: u64,
    value: &T,
) -> Result<()> {
    let payload = encode_json(value)?;
    let (flags, payload) = maybe_compress_payload(&payload, 1024)?;
    let mut frame = Frame::new(kind, stream_id, seq, payload);
    frame.flags = flags;

    stream.write_frame(&frame).await?;
    Ok(())
}

async fn read_json_frame<T: serde::de::DeserializeOwned>(
    stream: &mut NoiseFrameStream<TcpStream>,
) -> Result<T> {
    let frame = stream.read_frame().await?;
    let payload = frame.decoded_payload()?;
    Ok(decode_json(&payload)?)
}

fn command_payload_hash(command: &JobCommand) -> Result<String> {
    Ok(payload_hash(&encode_json(command)?))
}

async fn handle_command_frame(frame: Frame, ctx: CommandFrameContext<'_>) -> Result<bool> {
    let CommandFrameContext {
        config,
        config_path,
        stream,
        seq,
        command_runtime,
    } = ctx;
    let request: JobRequest = decode_json(&frame.decoded_payload()?)?;
    let request_payload_hash = command_payload_hash(&request.command)?;
    if !command_supports_requested_protocol(&request.command, request.command_version) {
        let current_command_protocol_version = job_command_protocol_version(&request.command);
        let min_command_protocol_version =
            job_command_min_supported_protocol_version(&request.command);
        warn!(
            job_id = %request.job_id,
            command_version = request.command_version,
            current_command_protocol_version,
            min_command_protocol_version,
            "rejected command with unsupported protocol version"
        );
        let output = unsupported_command_version_output(
            request.job_id,
            &request.command,
            request.command_version,
        )?;
        let replay_outputs = sequenced_outputs_starting_at(0, std::slice::from_ref(&output));
        let terminal_output = terminal_replay_output_from(&replay_outputs);
        command_runtime.recent_commands.remember(
            request.job_id,
            request_payload_hash,
            replay_outputs,
            terminal_output,
            false,
        );
        send_unsupported_command_version(stream, frame.stream_id, seq, request.job_id, output)
            .await?;
        return Ok(false);
    }
    if let Some(active) = command_runtime.active_commands.get_mut(&request.job_id) {
        let same_payload = active.payload_hash == request_payload_hash;
        let message = if same_payload {
            "duplicate job already active"
        } else {
            "duplicate job id is active with different payload"
        };
        let ack = JobAck {
            job_id: request.job_id,
            accepted: same_payload,
            message: message.to_string(),
        };
        if same_payload {
            active.stream_id = frame.stream_id;
        }
        send_json_frame(stream, MessageKind::CommandAck, frame.stream_id, *seq, &ack).await?;
        *seq += 1;
        let remove_after_flush;
        if same_payload {
            flush_pending_command_outputs(stream, seq, active).await?;
            remove_after_flush = active.finished && active.pending_outputs.is_empty();
        } else {
            remove_after_flush = false;
        }
        if remove_after_flush {
            command_runtime.active_commands.remove(&request.job_id);
        }
        return Ok(false);
    }
    if let Some(completed) = command_runtime.recent_commands.get(request.job_id) {
        if completed.payload_hash == request_payload_hash {
            let ack = JobAck {
                job_id: request.job_id,
                accepted: true,
                message: "duplicate completed job replayed".to_string(),
            };
            send_json_frame(stream, MessageKind::CommandAck, frame.stream_id, *seq, &ack).await?;
            *seq += 1;
            if completed.truncated {
                if let Some(output) = completed.terminal_output.as_ref() {
                    send_sequenced_command_payload(stream, frame.stream_id, seq, output).await?;
                } else {
                    let output = duplicate_replay_unknown_terminal_output(request.job_id)?;
                    send_sequenced_command_output(stream, frame.stream_id, seq, 0, &output).await?;
                }
            } else {
                send_sequenced_command_outputs(stream, frame.stream_id, seq, &completed.outputs)
                    .await?;
            }
            return Ok(false);
        }
        let ack = JobAck {
            job_id: request.job_id,
            accepted: false,
            message: "duplicate completed job id has different payload".to_string(),
        };
        send_json_frame(stream, MessageKind::CommandAck, frame.stream_id, *seq, &ack).await?;
        *seq += 1;
        return Ok(false);
    }
    let safety = job_command_safety(&request.command);
    if safety == JobCommandSafety::Exclusive
        && command_runtime
            .active_commands
            .values()
            .any(|active| active.safety == JobCommandSafety::Exclusive)
    {
        let ack = JobAck {
            job_id: request.job_id,
            accepted: false,
            message: "exclusive_command_already_active".to_string(),
        };
        send_json_frame(stream, MessageKind::CommandAck, frame.stream_id, *seq, &ack).await?;
        *seq += 1;
        return Ok(false);
    }
    let ack = JobAck {
        job_id: request.job_id,
        accepted: true,
        message: "accepted".to_string(),
    };
    send_json_frame(stream, MessageKind::CommandAck, frame.stream_id, *seq, &ack).await?;
    *seq += 1;

    let timeout_secs = request
        .timeout_secs
        .clamp(1, config.auth.command_timeout_secs.max(1));

    if let JobCommand::ConfigRead = &request.command {
        let result = read_redacted_config(request.job_id, config, config_path);
        let outputs = command_result_outputs(request.job_id, result);
        remember_recent_command_outputs(
            &mut command_runtime.recent_commands,
            request.job_id,
            request_payload_hash,
            &outputs,
        );
        send_command_outputs(stream, frame.stream_id, seq, &outputs).await?;
        return Ok(true);
    }
    if let JobCommand::HotConfig {
        toml,
        preserve_redacted,
        base_config_sha256_hex,
    } = &request.command
    {
        let outputs = match apply_hot_config_update(
            request.job_id,
            config,
            config_path,
            toml,
            preserve_redacted.unwrap_or(false),
            base_config_sha256_hex.as_deref(),
        ) {
            Ok(mut outputs) => {
                let reconcile =
                    reconcile_configured_runtime_tunnels(config, "hot_config_update").await;
                log_configured_runtime_tunnel_reconcile(&reconcile);
                if let Err(error) = attach_runtime_reconcile_report(&mut outputs, reconcile) {
                    warn!(%error, "failed to attach runtime tunnel reconcile report to hot config output");
                }
                outputs
            }
            Err(error) => command_result_outputs(request.job_id, Err(error)),
        };
        remember_recent_command_outputs(
            &mut command_runtime.recent_commands,
            request.job_id,
            request_payload_hash,
            &outputs,
        );
        send_command_outputs(stream, frame.stream_id, seq, &outputs).await?;
        return Ok(true);
    }
    if let JobCommand::DataSourceConfigPatch { toml } = &request.command {
        let outputs = match apply_data_source_config_patch(
            request.job_id,
            config,
            config_path,
            toml,
        ) {
            Ok(mut outputs) => {
                let reconcile =
                    reconcile_configured_runtime_tunnels(config, "data_source_config_patch").await;
                log_configured_runtime_tunnel_reconcile(&reconcile);
                if let Err(error) = attach_runtime_reconcile_report(&mut outputs, reconcile) {
                    warn!(%error, "failed to attach runtime tunnel reconcile report to data source config patch output");
                }
                outputs
            }
            Err(error) => command_result_outputs(request.job_id, Err(error)),
        };
        remember_recent_command_outputs(
            &mut command_runtime.recent_commands,
            request.job_id,
            request_payload_hash,
            &outputs,
        );
        send_command_outputs(stream, frame.stream_id, seq, &outputs).await?;
        return Ok(true);
    }
    let job_id = request.job_id;
    let command_version = request.command_version;
    let task_config = config.clone();
    let task_config_path = config_path.to_path_buf();
    let event_tx = command_runtime.command_event_tx.clone();
    let terminal_stream_tx = command_runtime.terminal_stream_tx.clone();
    let task = tokio::spawn(async move {
        let (output_tx, mut output_rx) = mpsc::channel::<CommandOutput>(16);
        let output_event_tx = event_tx.clone();
        let output_forwarder = tokio::spawn(async move {
            while let Some(output) = output_rx.recv().await {
                if output_event_tx
                    .send(CommandExecutionEvent::Output(output))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        let result = execute_authorized_command(
            request,
            task_config,
            task_config_path,
            timeout_secs,
            output_tx,
            terminal_stream_tx,
        )
        .await;
        let _ = output_forwarder.await;
        let _ = event_tx
            .send(CommandExecutionEvent::Finished(CommandExecutionResult {
                job_id,
                result,
            }))
            .await;
    });
    command_runtime.active_commands.insert(
        job_id,
        ActiveCommand {
            payload_hash: request_payload_hash,
            command_version,
            safety,
            stream_id: frame.stream_id,
            replay_outputs: Vec::new(),
            terminal_output: None,
            replay_output_bytes: 0,
            replay_truncated: false,
            pending_outputs: VecDeque::new(),
            next_output_seq: 0,
            finished: false,
            task,
        },
    );
    Ok(false)
}

async fn handle_command_cancel_frame(
    stream: &mut NoiseFrameStream<TcpStream>,
    seq: &mut u64,
    active_commands: &mut HashMap<uuid::Uuid, ActiveCommand>,
    request: JobCancelRequest,
) -> Result<()> {
    let (accepted, applied, message) = match active_commands.remove(&request.job_id) {
        Some(active) => {
            active.task.abort();
            (
                true,
                true,
                request.reason.unwrap_or_else(|| "canceled".to_string()),
            )
        }
        None => (true, false, "command_not_active".to_string()),
    };
    let ack = JobCancelAck {
        job_id: request.job_id,
        accepted,
        applied,
        message,
    };
    send_json_frame(stream, MessageKind::CommandCancelAck, 1, *seq, &ack).await?;
    *seq += 1;
    Ok(())
}

fn command_supports_requested_protocol(command: &JobCommand, command_version: u16) -> bool {
    let min = job_command_min_supported_protocol_version(command);
    let current = job_command_protocol_version(command);
    (min..=current).contains(&command_version)
}

async fn send_unsupported_command_version(
    stream: &mut NoiseFrameStream<TcpStream>,
    stream_id: u32,
    seq: &mut u64,
    job_id: uuid::Uuid,
    output: CommandOutput,
) -> Result<()> {
    let ack = JobAck {
        job_id,
        accepted: true,
        message: "unsupported_command_version".to_string(),
    };
    send_json_frame(stream, MessageKind::CommandAck, stream_id, *seq, &ack).await?;
    *seq += 1;
    send_sequenced_command_output(stream, stream_id, seq, 0, &output).await?;
    Ok(())
}

fn unsupported_command_version_output(
    job_id: uuid::Uuid,
    command: &JobCommand,
    command_version: u16,
) -> Result<CommandOutput> {
    let current_command_protocol_version = job_command_protocol_version(command);
    let min_command_protocol_version = job_command_min_supported_protocol_version(command);
    let status = serde_json::json!({
        "type": "unsupported_command_version",
        "status": "rejected",
        "job_id": job_id,
        "command_version": command_version,
        "current_command_protocol_version": current_command_protocol_version,
        "min_command_protocol_version": min_command_protocol_version,
        "reason": "agent_binary_does_not_support_requested_command_protocol",
    });
    Ok(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: Some(78),
        done: true,
    })
}

async fn execute_authorized_command(
    request: JobRequest,
    config: AgentConfig,
    config_path: PathBuf,
    timeout_secs: u64,
    streamed_output_tx: mpsc::Sender<CommandOutput>,
    terminal_stream_tx: mpsc::Sender<TerminalStreamOutput>,
) -> Result<Vec<CommandOutput>> {
    match &request.command {
        JobCommand::ConfigRead
        | JobCommand::HotConfig { .. }
        | JobCommand::DataSourceConfigPatch { .. } => {
            anyhow::bail!("config updates must run on the main agent task")
        }
        JobCommand::Backup {
            paths,
            include_config,
            recipient_public_key_hex,
        } => {
            execute_backup_command(BackupCommandInput {
                job_id: request.job_id,
                config: &config,
                config_path: &config_path,
                paths,
                include_config: *include_config,
                recipient_public_key_hex: recipient_public_key_hex.as_deref(),
                output_tx: Some(streamed_output_tx),
                timeout_secs,
            })
            .await
        }
        JobCommand::Restore {
            source_backup_request_id,
            paths,
            include_config,
            destination_root,
            archive_base64,
            archive_path,
            archive_size_bytes,
            archive_sha256_hex,
            dry_run,
            post_restore_argv,
        } => {
            execute_restore_command(RestoreCommandInput {
                job_id: request.job_id,
                source_backup_request_id: *source_backup_request_id,
                paths,
                include_config: *include_config,
                destination_root: destination_root.as_deref(),
                archive_path: archive_path.as_deref(),
                archive_base64: archive_base64.as_deref(),
                archive_size_bytes: *archive_size_bytes,
                archive_sha256_hex: archive_sha256_hex.as_deref(),
                dry_run: *dry_run,
                post_restore_argv,
                timeout_secs,
            })
            .await
        }
        JobCommand::RestoreRollback {
            source_restore_job_id,
            restored_files,
        } => {
            execute_restore_rollback_command(RestoreRollbackCommandInput {
                job_id: request.job_id,
                source_restore_job_id: *source_restore_job_id,
                restored_files,
                timeout_secs,
            })
            .await
        }
        JobCommand::NetworkApply {
            plan,
            side,
            config_backend,
            config_sha256_hex,
            ifupdown_sha256_hex,
            bird2_sha256_hex,
        } => {
            execute_network_apply_command(NetworkApplyInput {
                job_id: request.job_id,
                config: &config,
                plan,
                side: *side,
                config_backend: *config_backend,
                config_sha256_hex: config_sha256_hex.as_deref(),
                ifupdown_sha256_hex,
                bird2_sha256_hex,
                timeout_secs,
            })
            .await
        }
        JobCommand::NetworkRollback { plan, side } => {
            execute_network_rollback_command(NetworkRollbackInput {
                job_id: request.job_id,
                config: &config,
                plan,
                side: *side,
                timeout_secs,
            })
            .await
        }
        JobCommand::NetworkOspfCostUpdate {
            plan,
            side,
            current_ospf_cost,
            recommended_ospf_cost,
            bird2_sha256_hex,
        } => {
            execute_network_ospf_cost_update_command(NetworkOspfCostUpdateInput {
                job_id: request.job_id,
                config: &config,
                plan,
                side: *side,
                current_ospf_cost: *current_ospf_cost,
                recommended_ospf_cost: *recommended_ospf_cost,
                bird2_sha256_hex,
                timeout_secs,
            })
            .await
        }
        JobCommand::NetworkStatus { plan, side } => {
            execute_network_status_command(NetworkStatusInput {
                job_id: request.job_id,
                config: &config,
                plan,
                side: *side,
                timeout_secs,
            })
            .await
        }
        JobCommand::NetworkProbe {
            plan,
            side,
            count,
            interval_ms,
        } => {
            execute_network_probe_command(NetworkProbeInput {
                job_id: request.job_id,
                config: &config,
                plan,
                side: *side,
                count: *count,
                interval_ms: *interval_ms,
                timeout_secs,
            })
            .await
        }
        JobCommand::NetworkSpeedTest {
            plan,
            server_side,
            duration_secs,
            max_bytes,
            rate_limit_kbps,
            port,
            connect_timeout_ms,
        } => {
            execute_network_speed_test_command(NetworkSpeedTestInput {
                job_id: request.job_id,
                config: &config,
                plan,
                server_side: *server_side,
                duration_secs: *duration_secs,
                max_bytes: *max_bytes,
                rate_limit_kbps: *rate_limit_kbps,
                port: *port,
                connect_timeout_ms: *connect_timeout_ms,
                timeout_secs,
            })
            .await
        }
        JobCommand::UpdateAgent {
            artifact_url,
            sha256_hex,
            artifact_signature_hex,
            artifact_signing_key_hex,
        } => {
            execute_update_agent(AgentUpdateInput {
                job_id: request.job_id,
                artifact_url,
                sha256_hex,
                artifact_signature_hex: artifact_signature_hex.as_deref(),
                artifact_signing_key_hex: artifact_signing_key_hex.as_deref(),
                trusted_artifact_signing_key_hex: config
                    .update
                    .trusted_artifact_signing_key_hex
                    .as_deref(),
                timeout_secs,
            })
            .await
        }
        JobCommand::AgentUpdateCheck {
            version_url,
            activate,
            restart_agent,
        } => {
            let version_url = version_url
                .as_deref()
                .unwrap_or(config.update.unmanaged_version_url.as_str());
            execute_update_check(AgentUpdateCheckInput {
                job_id: request.job_id,
                version_url,
                activate: *activate,
                restart_agent: *restart_agent,
                trusted_artifact_signing_key_hex: config
                    .update
                    .trusted_artifact_signing_key_hex
                    .as_deref(),
                timeout_secs,
            })
            .await
        }
        JobCommand::TerminalOpen { .. }
        | JobCommand::TerminalInput { .. }
        | JobCommand::TerminalPoll { .. }
        | JobCommand::TerminalResize { .. }
        | JobCommand::TerminalClose { .. } => {
            execute_terminal_command_with_stream_sink(
                &config,
                request.job_id,
                &request.command,
                timeout_secs,
                Some(terminal_stream_tx),
            )
            .await
        }
        command => {
            execute_job_command_with_config_and_output_sink(
                &config,
                request.job_id,
                command,
                timeout_secs,
                Some(streamed_output_tx),
            )
            .await
        }
    }
}

fn duplicate_replay_unknown_terminal_output(job_id: uuid::Uuid) -> Result<CommandOutput> {
    let status = serde_json::json!({
        "type": "duplicate_job_replay_unavailable",
        "status": "failed",
        "job_id": job_id,
        "reason": "recent_command_terminal_result_unavailable",
    });
    Ok(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: Some(75),
        done: true,
    })
}

async fn send_command_outputs(
    stream: &mut NoiseFrameStream<TcpStream>,
    stream_id: u32,
    seq: &mut u64,
    outputs: &[CommandOutput],
) -> Result<()> {
    send_command_outputs_starting_at(stream, stream_id, seq, 0, outputs).await
}

async fn send_command_outputs_starting_at(
    stream: &mut NoiseFrameStream<TcpStream>,
    stream_id: u32,
    seq: &mut u64,
    start_output_seq: i32,
    outputs: &[CommandOutput],
) -> Result<()> {
    for (offset, output) in outputs.iter().enumerate() {
        let output_seq = start_output_seq.saturating_add(i32::try_from(offset).unwrap_or(i32::MAX));
        send_sequenced_command_output(stream, stream_id, seq, output_seq, output).await?;
    }
    Ok(())
}

async fn send_sequenced_command_output(
    stream: &mut NoiseFrameStream<TcpStream>,
    stream_id: u32,
    seq: &mut u64,
    output_seq: i32,
    output: &CommandOutput,
) -> Result<()> {
    let payload = SequencedCommandOutput {
        seq: output_seq,
        output: output.clone(),
    };
    send_json_frame(
        stream,
        MessageKind::CommandOutput,
        stream_id,
        *seq,
        &payload,
    )
    .await?;
    *seq += 1;
    Ok(())
}

async fn send_sequenced_command_outputs(
    stream: &mut NoiseFrameStream<TcpStream>,
    stream_id: u32,
    seq: &mut u64,
    outputs: &[SequencedCommandOutput],
) -> Result<()> {
    for output in outputs {
        send_sequenced_command_payload(stream, stream_id, seq, output).await?;
    }
    Ok(())
}

async fn send_sequenced_command_payload(
    stream: &mut NoiseFrameStream<TcpStream>,
    stream_id: u32,
    seq: &mut u64,
    output: &SequencedCommandOutput,
) -> Result<()> {
    send_json_frame(stream, MessageKind::CommandOutput, stream_id, *seq, output).await?;
    *seq += 1;
    Ok(())
}

async fn resume_active_commands(
    stream: &mut NoiseFrameStream<TcpStream>,
    seq: &mut u64,
    command_runtime: &mut AgentCommandRuntime,
) -> Result<()> {
    let job_ids = command_runtime
        .active_commands
        .keys()
        .copied()
        .collect::<Vec<_>>();
    for job_id in job_ids {
        let Some(active) = command_runtime.active_commands.get(&job_id) else {
            continue;
        };
        let next_output_seq = active
            .pending_outputs
            .front()
            .map(|pending| pending.seq)
            .unwrap_or(active.next_output_seq);
        let resume = CommandResume {
            job_id,
            command_version: active.command_version,
            payload_hash: active.payload_hash.clone(),
            next_output_seq,
        };
        send_json_frame(
            stream,
            MessageKind::CommandResume,
            active.stream_id,
            *seq,
            &resume,
        )
        .await?;
        *seq += 1;
    }
    flush_all_pending_command_outputs(stream, seq, &mut command_runtime.active_commands).await
}

async fn queue_active_command_output(
    stream: &mut NoiseFrameStream<TcpStream>,
    seq: &mut u64,
    active_commands: &mut HashMap<uuid::Uuid, ActiveCommand>,
    output: CommandOutput,
) -> Result<()> {
    let Some(active) = active_commands.get_mut(&output.job_id) else {
        return Ok(());
    };
    enqueue_active_command_output(active, output);
    flush_pending_command_outputs(stream, seq, active).await?;
    remove_finished_flushed_commands(active_commands);
    Ok(())
}

fn enqueue_active_command_output(active: &mut ActiveCommand, output: CommandOutput) {
    let seq = active.next_output_seq;
    active.next_output_seq = active.next_output_seq.saturating_add(1);
    let output = SequencedCommandOutput { seq, output };
    capture_replay_output(active, &output);
    active.pending_outputs.push_back(output);
}

async fn flush_all_pending_command_outputs(
    stream: &mut NoiseFrameStream<TcpStream>,
    seq: &mut u64,
    active_commands: &mut HashMap<uuid::Uuid, ActiveCommand>,
) -> Result<()> {
    let job_ids = active_commands.keys().copied().collect::<Vec<_>>();
    for job_id in job_ids {
        if let Some(active) = active_commands.get_mut(&job_id) {
            flush_pending_command_outputs(stream, seq, active).await?;
        }
    }
    remove_finished_flushed_commands(active_commands);
    Ok(())
}

async fn flush_pending_command_outputs(
    stream: &mut NoiseFrameStream<TcpStream>,
    seq: &mut u64,
    active: &mut ActiveCommand,
) -> Result<()> {
    while let Some(output) = active.pending_outputs.front() {
        send_json_frame(
            stream,
            MessageKind::CommandOutput,
            active.stream_id,
            *seq,
            output,
        )
        .await?;
        *seq += 1;
        active.pending_outputs.pop_front();
    }
    Ok(())
}

fn remove_finished_flushed_commands(active_commands: &mut HashMap<uuid::Uuid, ActiveCommand>) {
    active_commands.retain(|_, active| !(active.finished && active.pending_outputs.is_empty()));
}

async fn finish_active_command(
    stream: &mut NoiseFrameStream<TcpStream>,
    seq: &mut u64,
    active_commands: &mut HashMap<uuid::Uuid, ActiveCommand>,
    recent_commands: &mut RecentCommandCache,
    result: CommandExecutionResult,
) -> Result<()> {
    let Some(active) = active_commands.get_mut(&result.job_id) else {
        return Ok(());
    };
    let final_outputs = command_result_outputs(result.job_id, result.result);
    for output in final_outputs {
        enqueue_active_command_output(active, output);
    }
    active.finished = true;
    let replay_outputs = active.replay_outputs.clone();
    let replay_truncated =
        active.replay_truncated || sequenced_command_outputs_bytes(&replay_outputs) > 1024 * 1024;
    recent_commands.remember(
        result.job_id,
        active.payload_hash.clone(),
        replay_outputs,
        active.terminal_output.clone(),
        replay_truncated,
    );
    flush_pending_command_outputs(stream, seq, active).await?;
    remove_finished_flushed_commands(active_commands);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_command_cache_keeps_payload_hash_and_evicts_oldest() {
        let first = uuid::Uuid::new_v4();
        let second = uuid::Uuid::new_v4();
        let third = uuid::Uuid::new_v4();
        let mut cache = RecentCommandCache {
            max_entries: 2,
            ..RecentCommandCache::default()
        };

        remember_recent_command_outputs(
            &mut cache,
            first,
            "hash-a".to_string(),
            &[test_status_output(first)],
        );
        remember_recent_command_outputs(
            &mut cache,
            second,
            "hash-b".to_string(),
            &[test_status_output(second)],
        );
        assert_eq!(
            cache.get(first).map(|entry| entry.payload_hash.as_str()),
            Some("hash-a")
        );
        assert_eq!(
            cache.get(second).map(|entry| entry.payload_hash.as_str()),
            Some("hash-b")
        );
        assert_eq!(cache.get(first).map(|entry| entry.outputs.len()), Some(1));

        remember_recent_command_outputs(
            &mut cache,
            third,
            "hash-c".to_string(),
            &[test_status_output(third)],
        );
        assert!(cache.get(first).is_none());
        assert_eq!(
            cache.get(second).map(|entry| entry.payload_hash.as_str()),
            Some("hash-b")
        );
        assert_eq!(
            cache.get(third).map(|entry| entry.payload_hash.as_str()),
            Some("hash-c")
        );
    }

    #[test]
    fn recent_command_cache_marks_oversized_replay_unavailable() {
        let job_id = uuid::Uuid::new_v4();
        let mut cache = RecentCommandCache {
            max_entry_output_bytes: 4,
            ..RecentCommandCache::default()
        };

        let outputs = sequenced_outputs_starting_at(
            0,
            &[CommandOutput {
                job_id,
                stream: OutputStream::Status,
                data: b"too-large".to_vec(),
                exit_code: Some(0),
                done: true,
            }],
        );
        cache.remember(
            job_id,
            "hash-a".to_string(),
            outputs.clone(),
            terminal_replay_output_from(&outputs),
            false,
        );

        let entry = cache.get(job_id).expect("recent entry retained");
        assert!(entry.truncated);
        assert!(entry.outputs.is_empty());
        assert_eq!(
            entry
                .terminal_output
                .as_ref()
                .and_then(|output| output.output.exit_code),
            Some(0)
        );
    }

    #[tokio::test]
    async fn active_command_keeps_pending_outputs_until_flushed() {
        let job_id = uuid::Uuid::new_v4();
        let mut active = test_active_command(job_id);

        enqueue_active_command_output(&mut active, test_status_output(job_id));
        enqueue_active_command_output(&mut active, test_status_output(job_id));

        assert_eq!(active.next_output_seq, 2);
        assert_eq!(
            active
                .pending_outputs
                .iter()
                .map(|output| output.seq)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(active.replay_outputs.len(), 2);
        active.finished = true;

        let mut active_commands = HashMap::from([(job_id, active)]);
        remove_finished_flushed_commands(&mut active_commands);
        assert!(active_commands.contains_key(&job_id));

        active_commands
            .get_mut(&job_id)
            .unwrap()
            .pending_outputs
            .clear();
        remove_finished_flushed_commands(&mut active_commands);
        assert!(!active_commands.contains_key(&job_id));
    }

    fn test_active_command(job_id: uuid::Uuid) -> ActiveCommand {
        ActiveCommand {
            payload_hash: "payload-hash".to_string(),
            command_version: CURRENT_COMMAND_PROTOCOL_VERSION,
            safety: JobCommandSafety::ReadOnly,
            stream_id: 1,
            replay_outputs: Vec::new(),
            terminal_output: None,
            replay_output_bytes: 0,
            replay_truncated: false,
            pending_outputs: VecDeque::new(),
            next_output_seq: 0,
            finished: false,
            task: tokio::spawn(async move {
                let _ = job_id;
            }),
        }
    }

    fn test_status_output(job_id: uuid::Uuid) -> CommandOutput {
        CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: b"ok".to_vec(),
            exit_code: Some(0),
            done: true,
        }
    }

    #[test]
    fn command_payload_hash_changes_with_command_shape() {
        let left = JobCommand::Shell {
            argv: vec!["/bin/true".to_string()],
            pty: false,
        };
        let right = JobCommand::Shell {
            argv: vec!["/bin/false".to_string()],
            pty: false,
        };

        assert_ne!(
            command_payload_hash(&left).unwrap(),
            command_payload_hash(&right).unwrap()
        );
    }

    #[test]
    fn command_protocol_rejects_future_non_update_commands() {
        let command = JobCommand::Shell {
            argv: vec!["/bin/true".to_string()],
            pty: false,
        };

        assert!(command_supports_requested_protocol(
            &command,
            CURRENT_COMMAND_PROTOCOL_VERSION
        ));
        assert!(!command_supports_requested_protocol(
            &command,
            CURRENT_COMMAND_PROTOCOL_VERSION + 1
        ));
        assert!(!command_supports_requested_protocol(&command, 0));
    }

    #[test]
    fn command_protocol_rejects_future_update_commands() {
        let command = JobCommand::AgentUpdateCheck {
            version_url: None,
            activate: true,
            restart_agent: true,
        };

        assert!(!command_supports_requested_protocol(
            &command,
            CURRENT_COMMAND_PROTOCOL_VERSION + 10
        ));
        assert!(command_supports_requested_protocol(
            &command,
            CURRENT_COMMAND_PROTOCOL_VERSION
        ));
        assert!(!command_supports_requested_protocol(&command, 0));
    }

    #[tokio::test]
    async fn configured_runtime_reconcile_runs_saved_telemetry_plans() {
        let root = std::env::temp_dir().join(format!(
            "vpsman-configured-runtime-reconcile-{}",
            uuid::Uuid::new_v4()
        ));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let plan = vpsman_common::plan_tunnel(&vpsman_common::TunnelPlanInput {
            name: "left-right".to_string(),
            interface_name: "tunlr".to_string(),
            kind: vpsman_common::TunnelKind::Gre,
            runtime_control: Default::default(),
            runtime_topology: Default::default(),
            left_client_id: "left-a".to_string(),
            right_client_id: "right-b".to_string(),
            left_underlay: "198.51.100.10".to_string(),
            right_underlay: "203.0.113.20".to_string(),
            address_pool_cidr: "10.255.0.0/30".to_string(),
            reserved_addresses: Vec::new(),
            ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
                left: "10.255.0.0".to_string(),
                right: "10.255.0.1".to_string(),
                prefix_len: 31,
            }),
            ipv6_address_pool_cidr: None,
            ipv6_tunnel: None,
            latency_primary_family: Default::default(),
            bandwidth: vpsman_common::BandwidthTier::M100,
            latency_ms: 15.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            ospf_policy: vpsman_common::OspfCostPolicy::default(),
        })
        .unwrap();
        let config = AgentConfig {
            client_id: "left-a".to_string(),
            network: vpsman_common::AgentNetworkConfig {
                apply_enabled: true,
                runtime_reconcile_enabled: true,
                root_dir: root.to_string_lossy().to_string(),
                runtime_ip_argv: vec!["/bin/echo".to_string()],
                runtime_tc_argv: vec!["/bin/echo".to_string()],
                runtime_unprivileged_mutation_policy:
                    vpsman_common::AgentRuntimeUnprivilegedMutationPolicy::TryAll,
                runtime_status_telemetry_plans: vec![
                    vpsman_common::AgentRuntimeStatusTelemetryPlan {
                        plan_id: Some("plan-a".to_string()),
                        endpoint_side: vpsman_common::TunnelEndpointSide::Left,
                        plan,
                        traffic_source: Default::default(),
                        traffic_command: None,
                        latency_monitoring_enabled: true,
                        auto_ospf_enabled: false,
                        auto_ospf_updater: None,
                    },
                ],
                ..Default::default()
            },
            ..AgentConfig::default()
        };

        let report = reconcile_configured_runtime_tunnels(&config, "test").await;

        assert_eq!(report["status"], "completed");
        assert_eq!(report["total"], 1);
        assert_eq!(report["converged"], 1);
        assert_eq!(report["tunnels"][0]["plan_id"], "plan-a");
        assert_eq!(report["tunnels"][0]["interface"], "tunlr");
    }

    #[test]
    fn unmanaged_update_schedule_uses_next_interval_slot() {
        let config = AgentConfig {
            update: vpsman_common::AgentUpdateConfig {
                unmanaged_interval_secs: 300,
                unmanaged_jitter_secs: 0,
                ..vpsman_common::AgentUpdateConfig::default()
            },
            ..AgentConfig::default()
        };
        let base_instant = time::Instant::now();
        let due =
            next_unmanaged_update_due(&config, UNIX_EPOCH + Duration::from_secs(100), base_instant);

        assert_eq!(due.duration_since(base_instant), Duration::from_secs(200));
    }
}
