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
    job_command_protocol_version, maybe_compress_payload, payload_hash, AgentCapabilitySnapshot,
    AgentConfig, AgentHello, AgentPrivilegeMode, CommandOutput, Frame, JobAck, JobCancelAck,
    JobCancelRequest, JobCommand, JobRequest, MessageKind, NoiseFrameStream, OutputStream,
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
    network_speed::{execute_network_speed_test_command, NetworkSpeedTestInput},
    network_status::{execute_network_status_command, NetworkStatusInput},
    restore::{execute_restore_command, RestoreCommandInput},
    restore_rollback::{execute_restore_rollback_command, RestoreRollbackCommandInput},
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
    let mut recent_commands = RecentCommandCache::default();

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
                &mut recent_commands,
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
    recent_commands: &mut RecentCommandCache,
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
    let (command_event_tx, mut command_event_rx) = mpsc::channel::<CommandExecutionEvent>(32);
    let (terminal_stream_tx, mut terminal_stream_rx) = mpsc::channel::<TerminalStreamOutput>(64);
    let mut active_commands = HashMap::<uuid::Uuid, ActiveCommand>::new();
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
                                active_commands: &mut active_commands,
                                recent_commands,
                                command_event_tx: &command_event_tx,
                                terminal_stream_tx: &terminal_stream_tx,
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
                            &mut active_commands,
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
            event = command_event_rx.recv(), if !active_commands.is_empty() => {
                if let Some(event) = event {
                    match event {
                        CommandExecutionEvent::Output(output) => {
                            send_active_command_output(
                                &mut stream,
                                &mut seq,
                                &active_commands,
                                output,
                            )
                            .await?;
                        }
                        CommandExecutionEvent::Finished(result) => {
                            finish_active_command(
                                &mut stream,
                                &mut seq,
                                &mut active_commands,
                                recent_commands,
                                result,
                            )
                            .await?;
                        }
                    }
                }
            }
            output = terminal_stream_rx.recv() => {
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
            _ = &mut unmanaged_update_sleep, if active_commands.is_empty() && unmanaged_update_schedule.enabled(config) => {
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
    stream_id: u32,
    task: tokio::task::JoinHandle<()>,
}

struct CommandExecutionResult {
    job_id: uuid::Uuid,
    stream_id: u32,
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
    active_commands: &'a mut HashMap<uuid::Uuid, ActiveCommand>,
    recent_commands: &'a mut RecentCommandCache,
    command_event_tx: &'a mpsc::Sender<CommandExecutionEvent>,
    terminal_stream_tx: &'a mpsc::Sender<TerminalStreamOutput>,
}

struct RecentCommandCache {
    max_entries: usize,
    payload_hashes: HashMap<uuid::Uuid, String>,
    order: VecDeque<uuid::Uuid>,
}

impl Default for RecentCommandCache {
    fn default() -> Self {
        Self {
            max_entries: 512,
            payload_hashes: HashMap::new(),
            order: VecDeque::new(),
        }
    }
}

impl RecentCommandCache {
    fn remember(&mut self, job_id: uuid::Uuid, payload_hash: String) {
        if !self.payload_hashes.contains_key(&job_id) {
            self.order.push_back(job_id);
        }
        self.payload_hashes.insert(job_id, payload_hash);
        while self.order.len() > self.max_entries {
            if let Some(expired) = self.order.pop_front() {
                self.payload_hashes.remove(&expired);
            }
        }
    }

    fn get(&self, job_id: uuid::Uuid) -> Option<&str> {
        self.payload_hashes.get(&job_id).map(String::as_str)
    }
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
        active_commands,
        recent_commands,
        command_event_tx,
        terminal_stream_tx,
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
        recent_commands.remember(request.job_id, request_payload_hash);
        send_unsupported_command_version(
            stream,
            frame.stream_id,
            seq,
            request.job_id,
            &request.command,
            request.command_version,
        )
        .await?;
        return Ok(false);
    }
    if let Some(active) = active_commands.get(&request.job_id) {
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
        send_json_frame(stream, MessageKind::CommandAck, frame.stream_id, *seq, &ack).await?;
        *seq += 1;
        return Ok(false);
    }
    if let Some(completed_payload_hash) = recent_commands.get(request.job_id) {
        if completed_payload_hash == request_payload_hash {
            let ack = JobAck {
                job_id: request.job_id,
                accepted: true,
                message: "duplicate completed job suppressed".to_string(),
            };
            send_json_frame(stream, MessageKind::CommandAck, frame.stream_id, *seq, &ack).await?;
            *seq += 1;
            let status = serde_json::json!({
                "type": "duplicate_job_suppressed",
                "job_id": request.job_id,
                "reason": "already_completed_in_agent_runtime",
                "duplicate_delivery": "ignore_completed",
                "resume_outputs": true,
            });
            let output = CommandOutput {
                job_id: request.job_id,
                stream: OutputStream::Status,
                data: serde_json::to_vec(&status)?,
                exit_code: Some(0),
                done: true,
            };
            send_json_frame(
                stream,
                MessageKind::CommandOutput,
                frame.stream_id,
                *seq,
                &output,
            )
            .await?;
            *seq += 1;
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
        recent_commands.remember(request.job_id, request_payload_hash);
        send_command_result_outputs(stream, frame.stream_id, seq, request.job_id, result).await?;
        return Ok(true);
    }
    if let JobCommand::HotConfig {
        toml,
        preserve_redacted,
        base_config_sha256_hex,
    } = &request.command
    {
        let result = apply_hot_config_update(
            request.job_id,
            config,
            config_path,
            toml,
            preserve_redacted.unwrap_or(false),
            base_config_sha256_hex.as_deref(),
        );
        recent_commands.remember(request.job_id, request_payload_hash);
        send_command_result_outputs(stream, frame.stream_id, seq, request.job_id, result).await?;
        return Ok(true);
    }
    if let JobCommand::DataSourceConfigPatch { toml } = &request.command {
        let result = apply_data_source_config_patch(request.job_id, config, config_path, toml);
        recent_commands.remember(request.job_id, request_payload_hash);
        send_command_result_outputs(stream, frame.stream_id, seq, request.job_id, result).await?;
        return Ok(true);
    }
    let job_id = request.job_id;
    let stream_id = frame.stream_id;
    let task_config = config.clone();
    let task_config_path = config_path.to_path_buf();
    let event_tx = command_event_tx.clone();
    let terminal_stream_tx = terminal_stream_tx.clone();
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
                stream_id,
                result,
            }))
            .await;
    });
    active_commands.insert(
        job_id,
        ActiveCommand {
            payload_hash: request_payload_hash,
            stream_id,
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
    command: &JobCommand,
    command_version: u16,
) -> Result<()> {
    let current_command_protocol_version = job_command_protocol_version(command);
    let min_command_protocol_version = job_command_min_supported_protocol_version(command);
    let ack = JobAck {
        job_id,
        accepted: true,
        message: "unsupported_command_version".to_string(),
    };
    send_json_frame(stream, MessageKind::CommandAck, stream_id, *seq, &ack).await?;
    *seq += 1;
    let status = serde_json::json!({
        "type": "unsupported_command_version",
        "status": "rejected",
        "job_id": job_id,
        "command_version": command_version,
        "current_command_protocol_version": current_command_protocol_version,
        "min_command_protocol_version": min_command_protocol_version,
        "reason": "agent_binary_does_not_support_requested_command_protocol",
    });
    let output = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: Some(78),
        done: true,
    };
    send_json_frame(stream, MessageKind::CommandOutput, stream_id, *seq, &output).await?;
    *seq += 1;
    Ok(())
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

async fn send_command_result_outputs(
    stream: &mut NoiseFrameStream<TcpStream>,
    stream_id: u32,
    seq: &mut u64,
    job_id: uuid::Uuid,
    result: Result<Vec<CommandOutput>>,
) -> Result<()> {
    match result {
        Ok(outputs) => {
            for output in outputs {
                send_json_frame(stream, MessageKind::CommandOutput, stream_id, *seq, &output)
                    .await?;
                *seq += 1;
            }
        }
        Err(error) => {
            let output = CommandOutput {
                job_id,
                stream: OutputStream::Status,
                data: format!("command failed: {error}").into_bytes(),
                exit_code: Some(127),
                done: true,
            };
            send_json_frame(stream, MessageKind::CommandOutput, stream_id, *seq, &output).await?;
            *seq += 1;
        }
    }
    Ok(())
}

async fn send_active_command_output(
    stream: &mut NoiseFrameStream<TcpStream>,
    seq: &mut u64,
    active_commands: &HashMap<uuid::Uuid, ActiveCommand>,
    output: CommandOutput,
) -> Result<()> {
    let Some(active) = active_commands.get(&output.job_id) else {
        return Ok(());
    };
    send_json_frame(
        stream,
        MessageKind::CommandOutput,
        active.stream_id,
        *seq,
        &output,
    )
    .await?;
    *seq += 1;
    Ok(())
}

async fn finish_active_command(
    stream: &mut NoiseFrameStream<TcpStream>,
    seq: &mut u64,
    active_commands: &mut HashMap<uuid::Uuid, ActiveCommand>,
    recent_commands: &mut RecentCommandCache,
    result: CommandExecutionResult,
) -> Result<()> {
    let Some(active) = active_commands.remove(&result.job_id) else {
        return Ok(());
    };
    recent_commands.remember(result.job_id, active.payload_hash);
    send_command_result_outputs(stream, result.stream_id, seq, result.job_id, result.result).await
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

        cache.remember(first, "hash-a".to_string());
        cache.remember(second, "hash-b".to_string());
        assert_eq!(cache.get(first), Some("hash-a"));
        assert_eq!(cache.get(second), Some("hash-b"));

        cache.remember(third, "hash-c".to_string());
        assert_eq!(cache.get(first), None);
        assert_eq!(cache.get(second), Some("hash-b"));
        assert_eq!(cache.get(third), Some("hash-c"));
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
