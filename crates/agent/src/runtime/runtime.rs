use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{ensure, Context, Result};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::{
    net::TcpStream,
    sync::{mpsc, oneshot, Semaphore},
    task::JoinSet,
    time,
};
use tracing::{debug, info, warn};
use vpsman_common::{
    decode_json, decode_noise_key_hex, encode_json, job_command_min_supported_protocol_version,
    job_command_protocol_version, job_command_safety, job_command_type_label,
    maybe_compress_payload, payload_hash, runtime_config_content_hash,
    runtime_config_reconcile_scope_from_reason, validate_agent_config_shape,
    AgentBuiltinTunnelDriverCapabilities, AgentBuiltinTunnelDriverCapability,
    AgentCapabilitySnapshot, AgentConfig, AgentHello, AgentNetworkConfig, AgentPrivilegeMode,
    AgentRuntimeConfig, AgentRuntimeConfigReloadRequest, AgentSessionDisconnect,
    AgentUpdateVerificationResult, CommandOutput, CommandResume, Frame, JobAck, JobCancelAck,
    JobCancelRequest, JobCommand, JobCommandSafety, JobRequest, MessageKind, NoiseFrameStream,
    OutputStream, PortForwardRuntimeSnapshot, PortForwardRuntimeStatus,
    RuntimeConfigReconcileResource, RuntimeConfigReconcileScope, SequencedCommandOutput,
    ServerEndpoint, ServerHello, TelemetryEnvelope, TerminalStreamOutput,
    MAX_CONFIGURABLE_JOB_TIMEOUT_SECS,
};

use crate::{
    backup::{execute_backup_command, BackupCommandInput},
    command_ledger::{compact_ledger_terminal_output, CommandLedger},
    command_worker::{
        command_canceled_output, command_timeout_output, run_cancelable, CommandCancelToken,
        CommandCanceled,
    },
    config_update::read_redacted_config,
    executor::execute_job_command_with_config_cancel_and_output_sink,
    network_probe::{execute_network_probe_command, NetworkProbeInput},
    network_routing_adapter::{
        execute_network_routing_adapter_command, NetworkRoutingAdapterInput,
    },
    network_runtime::{
        execute_runtime_tunnel_reconcile_report_cancelable,
        execute_runtime_tunnel_remove_report_cancelable, probe_runtime_command,
        NetworkRuntimeReconcileInput, NetworkRuntimeRemoveInput,
    },
    network_speed::{execute_network_speed_test_command, NetworkSpeedTestInput},
    network_status::{
        execute_network_status_command, runtime_tunnel_requires_reconnect_sync, NetworkStatusInput,
    },
    port_forwarding::{
        inspect_port_forwarding, probe_port_forwarding_capability, reconcile_port_forwarding,
    },
    restore::{execute_restore_command, RestoreCommandInput},
    restore_rollback::{execute_restore_rollback_command, RestoreRollbackCommandInput},
    runtime_config_cache::RuntimeConfigCache,
    supervisor::reconcile_supervised_processes_on_start,
    telemetry::{
        collect_connection_host_facts, collect_metrics_for_config, TelemetryRuntimeState,
        GENERAL_PING_INTERVAL_SECS,
    },
    terminal::{
        close_all_terminal_sessions_for_lifecycle, control_terminal_session,
        drain_pending_terminal_final_events, execute_terminal_command_with_stream_sink,
        mark_gateway_connected, mark_gateway_disconnected,
    },
    update::{
        execute_update_agent, execute_update_check, AgentUpdateCheckInput, AgentUpdateInput,
        AgentUpdateVerificationWork,
    },
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
    let command_ledger = CommandLedger::open_default().await?;
    let runtime_config_cache = RuntimeConfigCache::open_default().await?;
    let mut loaded_cached_runtime_config_version = None;
    match runtime_config_cache.load().await {
        Ok(Some(runtime_config)) => {
            let mut candidate = config.clone();
            runtime_config.apply_to_agent_config(&mut candidate);
            match validate_agent_config_shape(&candidate) {
                Ok(()) => {
                    info!(
                        runtime_config_version = runtime_config.version,
                        "loaded last accepted runtime config"
                    );
                    config = candidate;
                    loaded_cached_runtime_config_version = Some(runtime_config.version);
                }
                Err(error) => {
                    warn!(%error, "ignored invalid last accepted runtime config");
                }
            }
        }
        Ok(None) => {}
        Err(error) => warn!(%error, "ignored unreadable last accepted runtime config"),
    }
    let mut command_runtime = AgentCommandRuntime::with_persistence(
        command_ledger,
        runtime_config_cache,
        loaded_cached_runtime_config_version,
    );
    let startup_runtime_config_requires_sync = loaded_cached_runtime_config_version.is_none();
    let mut startup_reconcile_resources = BTreeSet::new();
    let process_incarnation_id = uuid::Uuid::new_v4();
    match reconcile_supervised_processes_on_start().await {
        Ok(report) => log_supervisor_startup_reconcile(&report),
        Err(error) => warn!(%error, "process supervisor startup reconcile failed"),
    }
    if loaded_cached_runtime_config_version.is_some() {
        match reconcile_port_forwarding(
            &config.network.port_forwarding,
            !config.network.port_forwarding.rules.is_empty(),
            CommandCancelToken::default(),
        )
        .await
        {
            Ok(snapshot) => info!(?snapshot.status, "startup port-forwarding reconcile completed"),
            Err(error) => {
                startup_reconcile_resources.insert(RuntimeConfigReconcileResource::PortForwarding);
                warn!(%error, "startup port-forwarding reconcile failed; last accepted desired state remains cached")
            }
        }
    } else {
        info!(
            "no accepted runtime config is cached; preserving existing port-forwarding host state until an explicit sync"
        );
    }
    if loaded_cached_runtime_config_version.is_some() {
        let report =
            reconcile_configured_runtime_tunnels(&config, "last_accepted_config_startup").await;
        match report.get("status").and_then(serde_json::Value::as_str) {
            Some("failed") => {
                startup_reconcile_resources.insert(RuntimeConfigReconcileResource::RuntimeTunnels);
                warn!(report = %report, "last accepted tunnel reconcile failed");
            }
            _ => info!(report = %report, "last accepted tunnel reconcile completed"),
        }
    }
    command_runtime.requires_authoritative_runtime_config_sync =
        startup_runtime_config_requires_sync;
    command_runtime.pending_reconcile_resources = startup_reconcile_resources;
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
                process_incarnation_id,
            )
            .await
            {
                Ok(()) => {
                    mark_gateway_disconnected().await;
                    warn!(label = %endpoint.label, "gateway session ended");
                }
                Err(error) => {
                    mark_gateway_disconnected().await;
                    warn!(%error, label = %endpoint.label, "gateway session failed");
                }
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

fn effective_telemetry_interval_secs(configured_secs: u64, network: &AgentNetworkConfig) -> u64 {
    let mut interval_secs = configured_secs.max(5);
    if !network.ping_targets.is_empty() {
        interval_secs = interval_secs.min(GENERAL_PING_INTERVAL_SECS);
    }
    if network.runtime_status_telemetry_enabled
        && !network.runtime_status_telemetry_plans.is_empty()
    {
        interval_secs = interval_secs.min(
            network
                .runtime_status_telemetry_interval_secs
                .clamp(15, 3_600),
        );
        if network.latency_monitoring_enabled
            && network
                .runtime_status_telemetry_plans
                .iter()
                .any(|plan| plan.latency_monitoring_enabled)
        {
            interval_secs =
                interval_secs.min(network.latency_monitoring_interval_secs.clamp(15, 3_600));
        }
    }
    interval_secs
}

async fn connect_and_stream(
    config: &mut AgentConfig,
    config_path: &Path,
    endpoint: &str,
    command_runtime: &mut AgentCommandRuntime,
    process_incarnation_id: uuid::Uuid,
) -> Result<()> {
    let os_release = configured_os_release(config.telemetry.os_release_file.as_deref())?;
    let host_facts = collect_connection_host_facts(config);
    info!(%endpoint, "connecting to gateway");
    let tcp = connect_tcp_endpoint(endpoint, config.auth.gateway_connect_timeout_secs).await?;
    let mut stream = connect_noise_stream(tcp, config).await?;

    let port_forwarding_capability = probe_port_forwarding_capability().await;
    let hello = AgentHello {
        client_id: config.client_id.clone(),
        process_incarnation_id,
        agent_version: crate::build_info::agent_release_version().to_string(),
        internal_build_number: crate::build_info::agent_build_number(),
        os_release,
        arch: std::env::consts::ARCH.to_string(),
        cpu_model: host_facts.cpu_model,
        kernel_release: host_facts.kernel_release,
        virtualization: host_facts.virtualization,
        update_heartbeat: read_activation_heartbeat().unwrap_or_else(|error| {
            warn!(%error, "failed to read update activation heartbeat marker");
            None
        }),
        capabilities: agent_capabilities(config, port_forwarding_capability).await,
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
    mark_gateway_connected().await;

    let mut reconcile_resources = command_runtime.pending_reconcile_resources.clone();
    let port_forwarding_snapshot = inspect_port_forwarding(&config.network.port_forwarding).await;
    if port_forwarding_snapshot_requires_reconnect_sync(&port_forwarding_snapshot) {
        info!(
            status = ?port_forwarding_snapshot.status,
            error_code = port_forwarding_snapshot.error_code.as_deref(),
            "owned port-forwarding state requires reconnect reconciliation"
        );
        reconcile_resources.insert(RuntimeConfigReconcileResource::PortForwarding);
    }
    if configured_runtime_tunnels_require_reconnect_sync(config).await {
        info!("declared managed tunnel state requires reconnect reconciliation");
        reconcile_resources.insert(RuntimeConfigReconcileResource::RuntimeTunnels);
    }
    let mut seq = 2_u64;
    request_runtime_config_reload(
        &mut stream,
        &mut seq,
        config,
        command_runtime.requires_authoritative_runtime_config_sync,
        reconcile_resources,
    )
    .await?;
    for output in drain_pending_terminal_final_events().await {
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
    resume_active_commands(&mut stream, &mut seq, command_runtime).await?;
    let mut telemetry_runtime_state = TelemetryRuntimeState::default();
    let mut ticker = time::interval(Duration::from_secs(effective_telemetry_interval_secs(
        server_hello.telemetry_interval_secs,
        &config.network,
    )));
    let mut unmanaged_update_schedule = UnmanagedUpdateSchedule::new(config);
    let mut unmanaged_update_sleep =
        Box::pin(time::sleep_until(unmanaged_update_schedule.next_due()));
    let mut pending_update_verifications =
        HashMap::<uuid::Uuid, oneshot::Sender<AgentUpdateVerificationResult>>::new();
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let metrics = match collect_metrics_for_config(config, &mut telemetry_runtime_state).await {
                    Ok(metrics) => metrics,
                    Err(error) => {
                        warn!(%error, "telemetry collection failed; no sample published");
                        continue;
                    }
                };
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
                            ticker = time::interval(Duration::from_secs(
                                effective_telemetry_interval_secs(
                                    config.telemetry_interval_secs,
                                    &config.network,
                                ),
                            ));
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
                    MessageKind::TerminalControl => {
                        let request: vpsman_common::TerminalControlRequest =
                            decode_json(&frame.decoded_payload()?)?;
                        let ack = control_terminal_session(request).await;
                        send_json_frame(
                            &mut stream,
                            MessageKind::TerminalControlAck,
                            frame.stream_id,
                            seq,
                            &ack,
                        )
                        .await?;
                        seq += 1;
                    }
                    MessageKind::Keepalive => {
                        debug!("gateway keepalive");
                    }
                    MessageKind::AgentSessionDisconnect => {
                        let request: AgentSessionDisconnect =
                            decode_json(&frame.decoded_payload()?)?;
                        close_all_terminal_sessions_for_lifecycle(&request.reason).await;
                    }
                    MessageKind::AgentUpdateVerificationResult => {
                        let result: AgentUpdateVerificationResult =
                            decode_json(&frame.decoded_payload()?)?;
                        if let Some(response) =
                            pending_update_verifications.remove(&result.job_id)
                        {
                            let _ = response.send(result);
                        } else {
                            warn!(
                                job_id = %result.job_id,
                                "received unknown agent update verification result"
                            );
                        }
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
                        CommandExecutionEvent::Finished(mut result) => {
                            let mut accepted_runtime_config_persisted = false;
                            if let Some(runtime_config) = result.runtime_config_update.take() {
                                let accepted_version = runtime_config.version;
                                if let Some(cache) = command_runtime.runtime_config_cache.as_ref() {
                                    if let Err(error) = cache.store(&runtime_config).await {
                                        result.result = Err(error.context(
                                            "failed to persist last accepted runtime config",
                                        ));
                                        result.config_update = None;
                                    } else {
                                        command_runtime.accepted_runtime_config_version =
                                            Some(accepted_version);
                                        accepted_runtime_config_persisted = true;
                                    }
                                } else {
                                    command_runtime.accepted_runtime_config_version =
                                        Some(accepted_version);
                                    accepted_runtime_config_persisted = true;
                                }
                            }
                            if result.runtime_config_fully_applied
                                && accepted_runtime_config_persisted
                            {
                                if result.runtime_config_reconcile_scope.authoritative {
                                    command_runtime.requires_authoritative_runtime_config_sync = false;
                                    command_runtime.pending_reconcile_resources.clear();
                                } else {
                                    command_runtime.pending_reconcile_resources.retain(|resource| {
                                        !result
                                            .runtime_config_reconcile_scope
                                            .resources
                                            .contains(resource)
                                    });
                                }
                            }
                            let config_update = result.config_update.take();
                            if let Some(next_config) = config_update {
                                *config = next_config;
                                ticker = time::interval(Duration::from_secs(
                                    effective_telemetry_interval_secs(
                                        config.telemetry_interval_secs,
                                        &config.network,
                                    ),
                                ));
                                unmanaged_update_schedule = UnmanagedUpdateSchedule::new(config);
                                unmanaged_update_sleep.as_mut().reset(unmanaged_update_schedule.next_due());
                            }
                            finish_active_command(
                                &mut stream,
                                &mut seq,
                                &mut *command_runtime,
                                *result,
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
            work = command_runtime.update_verification_rx.recv() => {
                if let Some(work) = work {
                    let job_id = work.request.job_id;
                    if pending_update_verifications.contains_key(&job_id) {
                        let _ = work.response.send(AgentUpdateVerificationResult {
                            job_id,
                            approved: false,
                            message: "agent update verification already pending".to_string(),
                        });
                        continue;
                    }
                    if let Err(error) = send_json_frame(
                        &mut stream,
                        MessageKind::AgentUpdateVerificationRequest,
                        2,
                        seq,
                        &work.request,
                    )
                    .await
                    {
                        let message = format!("agent update verification send failed: {error}");
                        let _ = work.response.send(AgentUpdateVerificationResult {
                            job_id,
                            approved: false,
                            message,
                        });
                        return Err(error);
                    }
                    seq += 1;
                    pending_update_verifications.insert(job_id, work.response);
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

fn configured_os_release(path: Option<&str>) -> Result<String> {
    let Some(path) = path else {
        return Ok(String::new());
    };
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read configured OS release file {path}"))?;
    ensure!(
        !contents.trim().is_empty(),
        "configured OS release file {path} is empty"
    );
    Ok(contents)
}

async fn request_runtime_config_reload(
    stream: &mut NoiseFrameStream<TcpStream>,
    seq: &mut u64,
    config: &AgentConfig,
    requires_authoritative_sync: bool,
    reconcile_resources: BTreeSet<RuntimeConfigReconcileResource>,
) -> Result<()> {
    let runtime_config = AgentRuntimeConfig::from_agent_config(0, config);
    let current_content_hash = runtime_config_content_hash(&runtime_config)
        .context("failed to hash current runtime config")?;
    let request = AgentRuntimeConfigReloadRequest {
        client_id: config.client_id.clone(),
        current_content_hash,
        reason: "agent_reconnect_runtime_config_check".to_string(),
        requires_authoritative_sync,
        reconcile_resources: reconcile_resources.iter().copied().collect(),
        // New agents keep this compatibility projection until old APIs no longer
        // need the forwarding-only reconnect signal.
        requires_port_forwarding_sync: reconcile_resources
            .contains(&RuntimeConfigReconcileResource::PortForwarding),
    };
    send_json_frame(stream, MessageKind::ConfigUpdate, 0, *seq, &request).await?;
    *seq += 1;
    Ok(())
}

async fn configured_runtime_tunnels_require_reconnect_sync(config: &AgentConfig) -> bool {
    let managed = config
        .network
        .runtime_status_telemetry_plans
        .iter()
        .filter(|plan| {
            plan.plan.runtime_control.manager
                != vpsman_common::RuntimeTunnelManager::ExternalObserved
        })
        .cloned()
        .collect::<Vec<_>>();
    if managed.is_empty() {
        return false;
    }

    let permits = Arc::new(Semaphore::new(4));
    let mut inspections = JoinSet::new();
    for telemetry_plan in managed {
        let permit = permits.clone();
        let config = config.clone();
        inspections.spawn(async move {
            let _permit = permit.acquire_owned().await;
            let plan_id = telemetry_plan.plan_id.clone();
            let result = runtime_tunnel_requires_reconnect_sync(&config, &telemetry_plan).await;
            (plan_id, result)
        });
    }

    let inspection_budget =
        Duration::from_secs(config.network.status_probe_timeout_secs.clamp(1, 30));
    let outcome = time::timeout(inspection_budget, async {
        while let Some(result) = inspections.join_next().await {
            match result {
                Ok((plan_id, Ok(true))) => {
                    debug!(plan_id, "managed tunnel reconnect inspection found drift");
                    inspections.abort_all();
                    return true;
                }
                Ok((_plan_id, Ok(false))) => {}
                Ok((plan_id, Err(error))) => {
                    warn!(plan_id, %error, "managed tunnel reconnect inspection failed");
                    inspections.abort_all();
                    return true;
                }
                Err(error) => {
                    warn!(%error, "managed tunnel reconnect inspection task failed");
                    inspections.abort_all();
                    return true;
                }
            }
        }
        false
    })
    .await;
    match outcome {
        Ok(requires_sync) => requires_sync,
        Err(_) => {
            inspections.abort_all();
            warn!(
                max_wait_secs = inspection_budget.as_secs(),
                "managed tunnel reconnect inspection timed out; requesting declared tunnel reconciliation"
            );
            true
        }
    }
}

async fn connect_tcp_endpoint(endpoint: &str, max_timeout_secs: u64) -> Result<TcpStream> {
    let mut addrs = tokio::net::lookup_host(endpoint)
        .await
        .with_context(|| format!("failed to resolve gateway endpoint {endpoint}"))?
        .collect::<Vec<_>>();
    if addrs.is_empty() {
        anyhow::bail!("gateway endpoint {endpoint} resolved to no addresses");
    }
    addrs.sort_by_key(address_family_order);

    let timeout = Duration::from_secs(max_timeout_secs.clamp(1, 300));
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
        max_timeout_secs: config.auth.max_job_timeout_secs.max(300),
        cancel_token: CommandCancelToken::default(),
        verification_tx: None,
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

#[cfg_attr(not(test), allow(dead_code))]
async fn reconcile_configured_runtime_tunnels(
    config: &AgentConfig,
    trigger: &'static str,
) -> serde_json::Value {
    reconcile_configured_runtime_tunnels_cancelable(config, trigger, CommandCancelToken::default())
        .await
}

async fn reconcile_configured_runtime_tunnels_cancelable(
    config: &AgentConfig,
    trigger: &'static str,
    cancel_token: CommandCancelToken,
) -> serde_json::Value {
    let total = config.network.runtime_status_telemetry_plans.len();
    let mut summaries = Vec::with_capacity(total);
    let mut converged = 0_u64;
    let mut observed = 0_u64;
    let mut skipped = 0_u64;
    let mut degraded = 0_u64;
    let mut failed = 0_u64;

    for telemetry_plan in &config.network.runtime_status_telemetry_plans {
        if let Err(error) = cancel_token.check("runtime_config_sync") {
            failed += 1;
            summaries.push(runtime_reconcile_summary(
                trigger,
                telemetry_plan.plan_id.as_deref(),
                serde_json::json!({
                    "type": "runtime_tunnel_reconcile",
                    "status": "failed",
                    "plan": telemetry_plan.plan.name,
                    "interface": telemetry_plan.plan.interface_name,
                    "side": endpoint_side_name(telemetry_plan.endpoint_side),
                    "manager": telemetry_plan.plan.runtime_control.manager,
                }),
                Some(error.to_string()),
            ));
            break;
        }
        let plan = &telemetry_plan.plan;
        match execute_runtime_tunnel_reconcile_report_cancelable(
            NetworkRuntimeReconcileInput {
                config,
                plan_id: telemetry_plan.plan_id.as_deref(),
                plan,
                builtin_credentials: telemetry_plan.builtin_credentials.as_ref(),
                runtime_adapter: telemetry_plan.runtime_adapter.as_ref(),
                side: telemetry_plan.endpoint_side,
                max_timeout_secs: config.network.runtime_command_timeout_secs.max(1),
                #[cfg(test)]
                effective_uid_override: None,
            },
            cancel_token.clone(),
        )
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

#[derive(Debug)]
struct RuntimeConfigSyncResult {
    outputs: Vec<CommandOutput>,
    applied_config: Option<AgentConfig>,
    accepted_runtime_config: Option<AgentRuntimeConfig>,
    fully_applied: bool,
}

async fn apply_runtime_config_sync(
    job_id: uuid::Uuid,
    config: &AgentConfig,
    runtime_config: &AgentRuntimeConfig,
    desired_version: u64,
    reason: &str,
    cancel_token: CommandCancelToken,
) -> Result<RuntimeConfigSyncResult> {
    anyhow::ensure!(
        runtime_config.version == desired_version,
        "runtime config version mismatch"
    );
    let mut candidate_config = config.clone();
    runtime_config.apply_to_agent_config(&mut candidate_config);
    let previous_tunnels = config.network.runtime_status_telemetry_plans.clone();
    let previous_port_forwarding = config.network.port_forwarding.clone();
    let desired_tunnels = candidate_config
        .network
        .runtime_status_telemetry_plans
        .clone();
    let tunnels_changed = previous_tunnels != desired_tunnels;
    let port_forwarding_changed =
        previous_port_forwarding != candidate_config.network.port_forwarding;
    let reconcile_scope = runtime_config_reconcile_scope_from_reason(reason);
    let port_forwarding_reapply =
        reconcile_scope.includes(RuntimeConfigReconcileResource::PortForwarding);
    let tunnel_reapply = reconcile_scope.includes(RuntimeConfigReconcileResource::RuntimeTunnels);
    let port_forwarding = if port_forwarding_changed || port_forwarding_reapply {
        let require_table_access = port_forwarding_table_access_required(
            !previous_port_forwarding.rules.is_empty(),
            !candidate_config.network.port_forwarding.rules.is_empty(),
            reason,
        );
        match reconcile_port_forwarding(
            &candidate_config.network.port_forwarding,
            require_table_access,
            cancel_token.clone(),
        )
        .await
        {
            Ok(snapshot) => serde_json::to_value(snapshot).unwrap_or_else(|error| {
                serde_json::json!({
                    "status": "failed",
                    "error": error.to_string(),
                })
            }),
            Err(error) => serde_json::json!({
                "status": "failed",
                "error": error.to_string(),
            }),
        }
    } else {
        serde_json::json!({
            "status": "unchanged",
        })
    };
    let port_forwarding_failed = port_forwarding
        .get("status")
        .and_then(serde_json::Value::as_str)
        == Some("failed");

    let stale_tunnels = previous_tunnels
        .iter()
        .filter(|_| tunnels_changed)
        .filter(|previous| {
            !desired_tunnels
                .iter()
                .any(|desired| runtime_tunnel_identity_matches(previous, desired))
        })
        .cloned()
        .collect::<Vec<_>>();

    let mut removals = Vec::with_capacity(stale_tunnels.len());
    for stale in &stale_tunnels {
        cancel_token.check("runtime_config_sync")?;
        match execute_runtime_tunnel_remove_report_cancelable(
            NetworkRuntimeRemoveInput {
                config,
                plan_id: stale.plan_id.as_deref(),
                plan: &stale.plan,
                builtin_credentials: stale.builtin_credentials.as_ref(),
                runtime_adapter: stale.runtime_adapter.as_ref(),
                side: stale.endpoint_side,
                max_timeout_secs: config.network.runtime_command_timeout_secs.max(1),
                #[cfg(test)]
                effective_uid_override: None,
            },
            cancel_token.clone(),
        )
        .await
        {
            Ok(report) => removals.push(runtime_reconcile_summary(
                "runtime_config_sync_remove",
                stale.plan_id.as_deref(),
                report,
                None,
            )),
            Err(error) => removals.push(runtime_reconcile_summary(
                "runtime_config_sync_remove",
                stale.plan_id.as_deref(),
                serde_json::json!({
                    "type": "runtime_tunnel_remove",
                    "status": "failed",
                    "plan": stale.plan.name,
                    "interface": stale.plan.interface_name,
                    "side": endpoint_side_name(stale.endpoint_side),
                    "manager": stale.plan.runtime_control.manager,
                }),
                Some(error.to_string()),
            )),
        }
    }

    cancel_token.check("runtime_config_sync")?;
    let reconcile = if tunnels_changed || tunnel_reapply {
        reconcile_configured_runtime_tunnels_cancelable(
            &candidate_config,
            "runtime_config_sync",
            cancel_token.clone(),
        )
        .await
    } else {
        serde_json::json!({
            "status": "unchanged",
            "total": desired_tunnels.len(),
        })
    };
    let removal_failed = removals.iter().any(|removal| {
        !matches!(
            removal.get("status").and_then(serde_json::Value::as_str),
            Some("removed" | "observed_only")
        )
    });
    let reconcile_failed =
        reconcile.get("status").and_then(serde_json::Value::as_str) == Some("failed");
    let status = if removal_failed || reconcile_failed || port_forwarding_failed {
        "failed"
    } else {
        "applied"
    };
    let (applied_config, accepted_scope) = accepted_config_after_network_sync(
        config,
        &candidate_config,
        status == "applied",
        tunnels_changed,
        port_forwarding_changed,
        removal_failed || reconcile_failed,
        port_forwarding_failed,
    );
    let accepted_runtime_config = applied_config
        .as_ref()
        .map(|config| AgentRuntimeConfig::from_agent_config(desired_version, config));
    let body = serde_json::json!({
        "type": "runtime_config_sync",
        "status": status,
        "job_id": job_id,
        "desired_version": desired_version,
        "reason": reason,
        "client_id": &candidate_config.client_id,
        "removed_tunnel_count": removals.len(),
        "removals": removals,
        "reconcile": reconcile,
        "port_forwarding": port_forwarding,
        "accepted_scope": accepted_scope,
        "bootstrap_config_persisted": false,
    });
    let output = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&body)?,
        exit_code: Some(if status == "applied" { 0 } else { 1 }),
        done: true,
    };
    Ok(RuntimeConfigSyncResult {
        outputs: vec![output],
        applied_config,
        accepted_runtime_config,
        fully_applied: status == "applied",
    })
}

#[cfg(test)]
fn runtime_config_reason_requires_full_reconcile(reason: &str) -> bool {
    runtime_config_reconcile_scope_from_reason(reason).authoritative
}

fn runtime_config_reason_requires_port_forwarding_table_access(reason: &str) -> bool {
    runtime_config_reconcile_scope_from_reason(reason)
        .resources
        .contains(&RuntimeConfigReconcileResource::PortForwarding)
}

#[cfg(test)]
fn runtime_config_reason_requires_tunnel_reconcile(reason: &str) -> bool {
    runtime_config_reconcile_scope_from_reason(reason)
        .includes(RuntimeConfigReconcileResource::RuntimeTunnels)
}

fn port_forwarding_table_access_required(
    previous_rules_present: bool,
    desired_rules_present: bool,
    reason: &str,
) -> bool {
    previous_rules_present
        || desired_rules_present
        || runtime_config_reason_requires_port_forwarding_table_access(reason)
}

fn port_forwarding_snapshot_requires_reconnect_sync(snapshot: &PortForwardRuntimeSnapshot) -> bool {
    match snapshot.status {
        PortForwardRuntimeStatus::Drifted => true,
        PortForwardRuntimeStatus::Failed => {
            snapshot.error_code.as_deref() != Some("table_ownership_conflict")
        }
        PortForwardRuntimeStatus::Absent
        | PortForwardRuntimeStatus::Applied
        | PortForwardRuntimeStatus::Unsupported
        | PortForwardRuntimeStatus::Unknown => false,
    }
}

fn accepted_config_after_network_sync(
    current: &AgentConfig,
    candidate: &AgentConfig,
    fully_applied: bool,
    tunnels_changed: bool,
    port_forwarding_changed: bool,
    tunnel_failed: bool,
    port_forwarding_failed: bool,
) -> (Option<AgentConfig>, &'static str) {
    if fully_applied {
        return (Some(candidate.clone()), "full");
    }
    let mut partial = current.clone();
    let mut accepted_tunnels = false;
    let mut accepted_port_forwarding = false;
    if tunnels_changed && !tunnel_failed {
        let previous_port_forwarding = partial.network.port_forwarding.clone();
        partial.network = candidate.network.clone();
        if port_forwarding_failed {
            partial.network.port_forwarding = previous_port_forwarding;
        }
        accepted_tunnels = true;
    }
    if port_forwarding_changed && !port_forwarding_failed {
        partial.network.port_forwarding = candidate.network.port_forwarding.clone();
        accepted_port_forwarding = true;
    }
    if partial == *current {
        (None, "none")
    } else if accepted_port_forwarding && !accepted_tunnels {
        (Some(partial), "port_forwarding")
    } else if accepted_tunnels && !accepted_port_forwarding {
        (Some(partial), "tunnels")
    } else {
        (Some(partial), "network")
    }
}

fn runtime_tunnel_identity_matches(
    left: &vpsman_common::AgentRuntimeStatusTelemetryPlan,
    right: &vpsman_common::AgentRuntimeStatusTelemetryPlan,
) -> bool {
    left.endpoint_side == right.endpoint_side
        && left.plan_id == right.plan_id
        && left.plan.interface_name == right.plan.interface_name
        && left.plan.kind == right.plan.kind
        && left.plan.runtime_control.manager == right.plan.runtime_control.manager
        && left.plan.left_client_id == right.plan.left_client_id
        && left.plan.right_client_id == right.plan.right_client_id
        && runtime_tunnel_underlay_identity_matches(&left.plan, &right.plan)
        && left.plan.left_tunnel_address == right.plan.left_tunnel_address
        && left.plan.right_tunnel_address == right.plan.right_tunnel_address
        && left.plan.tunnel_prefix_len == right.plan.tunnel_prefix_len
        && left.plan.ipv4_tunnel == right.plan.ipv4_tunnel
        && left.plan.ipv6_tunnel == right.plan.ipv6_tunnel
        && left.runtime_adapter == right.runtime_adapter
        && runtime_tunnel_control_identity_matches(
            &left.plan.runtime_control,
            &right.plan.runtime_control,
        )
}

fn runtime_tunnel_underlay_identity_matches(
    left: &vpsman_common::TunnelPlan,
    right: &vpsman_common::TunnelPlan,
) -> bool {
    if left.runtime_control.manager == vpsman_common::RuntimeTunnelManager::AgentBuiltin
        && matches!(
            left.kind,
            vpsman_common::TunnelKind::Wireguard | vpsman_common::TunnelKind::Openvpn
        )
    {
        return true;
    }
    left.left_remote_underlay == right.left_remote_underlay
        && left.left_local_underlay == right.left_local_underlay
        && left.right_remote_underlay == right.right_remote_underlay
        && left.right_local_underlay == right.right_local_underlay
}

fn runtime_config_snapshot_is_stale(
    accepted_version: Option<u64>,
    current: &AgentConfig,
    desired_version: u64,
    incoming: &AgentRuntimeConfig,
) -> Result<bool> {
    let Some(accepted_version) = accepted_version else {
        return Ok(false);
    };
    if desired_version < accepted_version {
        return Ok(true);
    }
    if desired_version > accepted_version {
        return Ok(false);
    }
    let current = AgentRuntimeConfig::from_agent_config(0, current);
    Ok(runtime_config_content_hash(&current)? != runtime_config_content_hash(incoming)?)
}

fn runtime_tunnel_control_identity_matches(
    left: &vpsman_common::RuntimeTunnelControl,
    right: &vpsman_common::RuntimeTunnelControl,
) -> bool {
    match left.manager {
        vpsman_common::RuntimeTunnelManager::AgentBuiltin => left.fou == right.fou,
        vpsman_common::RuntimeTunnelManager::ExternalObserved => true,
        vpsman_common::RuntimeTunnelManager::CustomAdapter => {
            left.left_adapter_definition_id == right.left_adapter_definition_id
                && left.right_adapter_definition_id == right.right_adapter_definition_id
                && left.traffic_limit == right.traffic_limit
        }
    }
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

async fn agent_capabilities(
    config: &AgentConfig,
    port_forwarding: vpsman_common::PortForwardCapability,
) -> AgentCapabilitySnapshot {
    let effective_uid = unsafe { libc::geteuid() } as u32;
    let root = effective_uid == 0;
    let builtin_tunnel_drivers = AgentBuiltinTunnelDriverCapabilities {
        iproute2: probe_builtin_driver(
            &config.network.runtime_ip_argv,
            &["-Version"],
            Some("iproute2-"),
            None,
        )
        .await,
        wireguard: probe_builtin_driver(
            &config.network.runtime_wg_argv,
            &["--version"],
            Some("wireguard-tools v"),
            None,
        )
        .await,
        openvpn: probe_builtin_driver(
            &config.network.runtime_openvpn_argv,
            &["--version"],
            Some("OpenVPN "),
            Some(Version::new(2, 4, 0)),
        )
        .await,
    };
    AgentCapabilitySnapshot {
        privilege_mode: if root {
            AgentPrivilegeMode::Root
        } else {
            AgentPrivilegeMode::Unprivileged
        },
        effective_uid: Some(effective_uid),
        max_job_timeout_secs: config.auth.max_job_timeout_secs.max(1),
        can_attempt_privileged_ops: true,
        can_manage_runtime_tunnels: root,
        builtin_tunnel_drivers,
        can_apply_process_limits: root,
        port_forwarding,
        unprivileged_hint: (!root).then(|| {
            "agent is not running as root; root-only network, update, restore, and limit operations may report ineffective or require forced best-effort mode".to_string()
        }),
    }
}

async fn probe_builtin_driver(
    base_argv: &[String],
    version_args: &[&str],
    version_marker: Option<&str>,
    minimum_version: Option<Version>,
) -> AgentBuiltinTunnelDriverCapability {
    let Some(_executable) = base_argv.first().filter(|value| value.starts_with('/')) else {
        return AgentBuiltinTunnelDriverCapability {
            unavailable_reason: Some(
                "configured executable argv is missing or not absolute".to_string(),
            ),
            ..AgentBuiltinTunnelDriverCapability::default()
        };
    };
    let mut argv = base_argv.to_vec();
    argv.extend(version_args.iter().map(|value| (*value).to_string()));
    let report = match probe_runtime_command("builtin_tunnel_driver_version", &argv, 2, 4096).await
    {
        Ok(report) => report,
        Err(error) => {
            return AgentBuiltinTunnelDriverCapability {
                unavailable_reason: Some(format!("executable unavailable: {error}")),
                ..AgentBuiltinTunnelDriverCapability::default()
            }
        }
    };
    let stdout = report["stdout"]["text"].as_str().unwrap_or_default();
    let stderr = report["stderr"]["text"].as_str().unwrap_or_default();
    let text = format!("{stdout}\n{stderr}");
    let version = version_marker.and_then(|marker| parse_marked_version(&text, marker));
    if report["success"].as_bool() != Some(true) {
        let reason = if report["timed_out"].as_bool() == Some(true) {
            "version probe timed out".to_string()
        } else if report["killed_for_output_limit"].as_bool() == Some(true) {
            "version probe exceeded its output limit".to_string()
        } else {
            format!(
                "version probe exited with {}",
                report["exit_code"]
                    .as_i64()
                    .map_or_else(|| "signal".to_string(), |code| code.to_string())
            )
        };
        return AgentBuiltinTunnelDriverCapability {
            unavailable_reason: Some(reason),
            ..AgentBuiltinTunnelDriverCapability::default()
        };
    }
    if version_marker.is_some() && version.is_none() {
        return AgentBuiltinTunnelDriverCapability {
            unavailable_reason: Some(
                "version probe did not identify the configured driver".to_string(),
            ),
            ..AgentBuiltinTunnelDriverCapability::default()
        };
    }
    if minimum_version
        .as_ref()
        .is_some_and(|minimum| version.as_ref().is_none_or(|version| version < minimum))
    {
        return AgentBuiltinTunnelDriverCapability {
            version: version.map(|version| version.to_string()),
            unavailable_reason: Some(format!(
                "version {} or newer is required",
                minimum_version.expect("minimum version exists")
            )),
            ..AgentBuiltinTunnelDriverCapability::default()
        };
    }
    AgentBuiltinTunnelDriverCapability {
        available: true,
        version: version.map(|version| version.to_string()),
        unavailable_reason: None,
    }
}

fn parse_marked_version(output: &str, marker: &str) -> Option<Version> {
    let remainder = output.split_once(marker)?.1;
    let version = remainder
        .trim_start()
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '.')
        .next()?;
    Version::parse(version).ok()
}

struct ActiveCommand {
    payload_hash: String,
    cancel_token: CommandCancelToken,
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
    _task: tokio::task::JoinHandle<()>,
}

struct CommandExecutionResult {
    job_id: uuid::Uuid,
    operation_type: &'static str,
    max_timeout_secs: u64,
    result: Result<Vec<CommandOutput>>,
    config_update: Option<AgentConfig>,
    runtime_config_update: Option<AgentRuntimeConfig>,
    runtime_config_fully_applied: bool,
    runtime_config_reconcile_scope: RuntimeConfigReconcileScope,
}

enum CommandExecutionEvent {
    Output(CommandOutput),
    Finished(Box<CommandExecutionResult>),
}

struct CommandFrameContext<'a> {
    config: &'a mut AgentConfig,
    config_path: &'a Path,
    stream: &'a mut NoiseFrameStream<TcpStream>,
    seq: &'a mut u64,
    command_runtime: &'a mut AgentCommandRuntime,
}

#[derive(Deserialize)]
struct JobRequestWire {
    job_id: uuid::Uuid,
    #[serde(default = "vpsman_common::default_command_protocol_version")]
    command_version: u16,
    command: serde_json::Value,
    max_timeout_secs: u64,
}

struct UnsupportedJobRequest {
    job_id: uuid::Uuid,
    command_version: u16,
    command_type: String,
    payload_hash: String,
    message: String,
}

enum DecodedJobRequest {
    Supported(Box<JobRequest>),
    Unsupported(UnsupportedJobRequest),
}

struct AgentCommandRuntime {
    active_commands: HashMap<uuid::Uuid, ActiveCommand>,
    recent_commands: RecentCommandCache,
    command_ledger: Option<CommandLedger>,
    runtime_config_cache: Option<RuntimeConfigCache>,
    accepted_runtime_config_version: Option<u64>,
    requires_authoritative_runtime_config_sync: bool,
    pending_reconcile_resources: BTreeSet<RuntimeConfigReconcileResource>,
    command_event_tx: mpsc::Sender<CommandExecutionEvent>,
    command_event_rx: mpsc::Receiver<CommandExecutionEvent>,
    update_verification_tx: mpsc::Sender<AgentUpdateVerificationWork>,
    update_verification_rx: mpsc::Receiver<AgentUpdateVerificationWork>,
    terminal_stream_tx: mpsc::Sender<TerminalStreamOutput>,
    terminal_stream_rx: mpsc::Receiver<TerminalStreamOutput>,
}

impl Default for AgentCommandRuntime {
    fn default() -> Self {
        let (command_event_tx, command_event_rx) = mpsc::channel::<CommandExecutionEvent>(32);
        let (update_verification_tx, update_verification_rx) =
            mpsc::channel::<AgentUpdateVerificationWork>(8);
        let (terminal_stream_tx, terminal_stream_rx) = mpsc::channel::<TerminalStreamOutput>(64);
        Self {
            active_commands: HashMap::new(),
            recent_commands: RecentCommandCache::default(),
            command_ledger: None,
            runtime_config_cache: None,
            accepted_runtime_config_version: None,
            requires_authoritative_runtime_config_sync: true,
            pending_reconcile_resources: BTreeSet::new(),
            command_event_tx,
            command_event_rx,
            update_verification_tx,
            update_verification_rx,
            terminal_stream_tx,
            terminal_stream_rx,
        }
    }
}

impl AgentCommandRuntime {
    fn with_persistence(
        command_ledger: CommandLedger,
        runtime_config_cache: RuntimeConfigCache,
        accepted_runtime_config_version: Option<u64>,
    ) -> Self {
        Self {
            command_ledger: Some(command_ledger),
            runtime_config_cache: Some(runtime_config_cache),
            accepted_runtime_config_version,
            ..Self::default()
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
    let data = serde_json::to_vec(&serde_json::json!({
        "type": "duplicate_job_replay_unavailable",
        "status": "failed",
        "job_id": output.output.job_id,
        "reason": "recent_command_replay_truncated",
        "message": "duplicate command replay is lossy; original terminal output requires human review",
        "original_stream": output_stream_name(output.output.stream),
        "original_exit_code": output.output.exit_code,
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
            exit_code: Some(75),
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
    operation_type: &str,
    max_timeout_secs: u64,
    result: Result<Vec<CommandOutput>>,
) -> Vec<CommandOutput> {
    match result {
        Ok(outputs) => outputs,
        Err(error) => {
            if let Some(canceled) = error.downcast_ref::<CommandCanceled>() {
                return command_canceled_output(
                    job_id,
                    canceled.operation_type(),
                    canceled.reason(),
                )
                .map(|output| vec![output])
                .unwrap_or_else(|_| fallback_failed_output(job_id, &error));
            }
            let message = error.to_string();
            if message.contains("timed out") || message.contains("elapsed") {
                return command_timeout_output(job_id, operation_type, max_timeout_secs)
                    .map(|output| vec![output])
                    .unwrap_or_else(|_| fallback_failed_output(job_id, &error));
            }
            fallback_failed_output(job_id, &error)
        }
    }
}

fn fallback_failed_output(job_id: uuid::Uuid, error: &anyhow::Error) -> Vec<CommandOutput> {
    vec![CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: format!("command failed: {error:#}").into_bytes(),
        exit_code: Some(127),
        done: true,
    }]
}

#[cfg(test)]
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

async fn remember_completed_command_outputs(
    command_runtime: &mut AgentCommandRuntime,
    job_id: uuid::Uuid,
    payload_hash: String,
    outputs: &[CommandOutput],
) -> Result<()> {
    let replay_outputs = sequenced_outputs_starting_at(0, outputs);
    let terminal_output = terminal_replay_output_from(&replay_outputs);
    remember_completed_replay_outputs(
        command_runtime,
        job_id,
        payload_hash,
        replay_outputs,
        terminal_output,
        false,
    )
    .await
}

async fn remember_completed_replay_outputs(
    command_runtime: &mut AgentCommandRuntime,
    job_id: uuid::Uuid,
    payload_hash: String,
    replay_outputs: Vec<SequencedCommandOutput>,
    terminal_output: Option<SequencedCommandOutput>,
    replay_truncated: bool,
) -> Result<()> {
    command_runtime.recent_commands.remember(
        job_id,
        payload_hash.clone(),
        replay_outputs,
        terminal_output.clone(),
        replay_truncated,
    );
    if let Some(ledger) = command_runtime.command_ledger.as_ref() {
        ledger
            .record(
                job_id,
                payload_hash,
                compact_ledger_terminal_output(terminal_output),
                true,
            )
            .await?;
    }
    Ok(())
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
    let payload = frame.decoded_payload()?;
    let request = match decode_job_request_payload(&payload)? {
        DecodedJobRequest::Supported(request) => *request,
        DecodedJobRequest::Unsupported(unsupported) => {
            warn!(
                job_id = %unsupported.job_id,
                command_version = unsupported.command_version,
                command_type = %unsupported.command_type,
                error = %unsupported.message,
                "rejected unsupported command shape without dropping agent session"
            );
            let output = unsupported_command_shape_output(&unsupported)?;
            let replay_outputs = sequenced_outputs_starting_at(0, std::slice::from_ref(&output));
            let terminal_output = terminal_replay_output_from(&replay_outputs);
            remember_completed_replay_outputs(
                command_runtime,
                unsupported.job_id,
                unsupported.payload_hash,
                replay_outputs,
                terminal_output,
                false,
            )
            .await?;
            send_unsupported_command_version(
                stream,
                frame.stream_id,
                seq,
                unsupported.job_id,
                output,
            )
            .await?;
            return Ok(false);
        }
    };
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
        remember_completed_replay_outputs(
            command_runtime,
            request.job_id,
            request_payload_hash,
            replay_outputs,
            terminal_output,
            false,
        )
        .await?;
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
    if let Some(ledger) = command_runtime.command_ledger.as_ref() {
        if let Some(completed) = ledger.lookup(request.job_id).await? {
            if completed.payload_hash == request_payload_hash {
                let ack = JobAck {
                    job_id: request.job_id,
                    accepted: true,
                    message: "duplicate completed job replayed from ledger".to_string(),
                };
                send_json_frame(stream, MessageKind::CommandAck, frame.stream_id, *seq, &ack)
                    .await?;
                *seq += 1;
                if let Some(output) = completed.terminal_output.as_ref() {
                    send_sequenced_command_payload(stream, frame.stream_id, seq, output).await?;
                } else {
                    let output = duplicate_replay_unknown_terminal_output(request.job_id)?;
                    send_sequenced_command_output(stream, frame.stream_id, seq, 0, &output).await?;
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
    }
    if let JobCommand::RuntimeConfigSync {
        desired_version,
        config: runtime_config,
        ..
    } = &request.command
    {
        let rejection = if runtime_config.version != *desired_version {
            Some("runtime_config_version_mismatch")
        } else if runtime_config_snapshot_is_stale(
            command_runtime.accepted_runtime_config_version,
            config,
            *desired_version,
            runtime_config,
        )? {
            Some("runtime_config_snapshot_stale")
        } else {
            None
        };
        if let Some(message) = rejection {
            let ack = JobAck {
                job_id: request.job_id,
                accepted: false,
                message: message.to_string(),
            };
            send_json_frame(stream, MessageKind::CommandAck, frame.stream_id, *seq, &ack).await?;
            *seq += 1;
            return Ok(false);
        }
    }
    let safety = job_command_safety(&request.command);
    let active_exclusive = command_runtime
        .active_commands
        .values()
        .any(|active| active.safety == JobCommandSafety::Exclusive);
    let exclusive_conflict = if safety == JobCommandSafety::Exclusive {
        !command_runtime.active_commands.is_empty()
    } else {
        active_exclusive
    };
    if exclusive_conflict {
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

    let max_timeout_secs = request
        .max_timeout_secs
        .clamp(1, MAX_CONFIGURABLE_JOB_TIMEOUT_SECS);

    if let JobCommand::ConfigRead = &request.command {
        let result = read_redacted_config(request.job_id, config, config_path);
        let outputs =
            command_result_outputs(request.job_id, "config_read", max_timeout_secs, result);
        remember_completed_command_outputs(
            command_runtime,
            request.job_id,
            request_payload_hash,
            &outputs,
        )
        .await?;
        send_command_outputs(stream, frame.stream_id, seq, &outputs).await?;
        return Ok(true);
    }
    let runtime_sync = if let JobCommand::RuntimeConfigSync {
        desired_version,
        reason,
        config: runtime_config,
    } = &request.command
    {
        Some((*desired_version, reason.clone(), (**runtime_config).clone()))
    } else {
        None
    };
    let job_id = request.job_id;
    let command_version = request.command_version;
    let operation_type = job_command_type_label(&request.command);
    let cancel_token = CommandCancelToken::default();
    let task_config = config.clone();
    let task_config_path = config_path.to_path_buf();
    let event_tx = command_runtime.command_event_tx.clone();
    let update_verification_tx = command_runtime.update_verification_tx.clone();
    let task_cancel_token = cancel_token.clone();
    let task = if let Some((desired_version, reason, runtime_config)) = runtime_sync {
        let runtime_config_reconcile_scope = runtime_config_reconcile_scope_from_reason(&reason);
        tokio::spawn(async move {
            let result = time::timeout(
                Duration::from_secs(max_timeout_secs.max(1)),
                run_cancelable(
                    operation_type,
                    task_cancel_token.clone(),
                    apply_runtime_config_sync(
                        job_id,
                        &task_config,
                        &runtime_config,
                        desired_version,
                        &reason,
                        task_cancel_token.clone(),
                    ),
                ),
            )
            .await;
            let (result, config_update, runtime_config_update, runtime_config_fully_applied) =
                match result {
                    Ok(Ok(sync)) => (
                        Ok(sync.outputs),
                        sync.applied_config,
                        sync.accepted_runtime_config,
                        sync.fully_applied,
                    ),
                    Ok(Err(error)) => (Err(error), None, None, false),
                    Err(error) => {
                        task_cancel_token.cancel("runtime_config_sync_timeout".to_string());
                        (
                            Err(anyhow::anyhow!("runtime config sync timed out: {error}")),
                            None,
                            None,
                            false,
                        )
                    }
                };
            let _ = event_tx
                .send(CommandExecutionEvent::Finished(Box::new(
                    CommandExecutionResult {
                        job_id,
                        operation_type,
                        max_timeout_secs,
                        result,
                        config_update,
                        runtime_config_update,
                        runtime_config_fully_applied,
                        runtime_config_reconcile_scope,
                    },
                )))
                .await;
        })
    } else {
        let terminal_stream_tx = command_runtime.terminal_stream_tx.clone();
        tokio::spawn(async move {
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
                max_timeout_secs,
                output_tx,
                update_verification_tx,
                terminal_stream_tx,
                task_cancel_token,
            )
            .await;
            let _ = output_forwarder.await;
            let _ = event_tx
                .send(CommandExecutionEvent::Finished(Box::new(
                    CommandExecutionResult {
                        job_id,
                        operation_type,
                        max_timeout_secs,
                        result,
                        config_update: None,
                        runtime_config_update: None,
                        runtime_config_fully_applied: false,
                        runtime_config_reconcile_scope: RuntimeConfigReconcileScope::default(),
                    },
                )))
                .await;
        })
    };
    command_runtime.active_commands.insert(
        job_id,
        ActiveCommand {
            payload_hash: request_payload_hash,
            cancel_token,
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
            _task: task,
        },
    );
    Ok(false)
}

fn decode_job_request_payload(payload: &[u8]) -> Result<DecodedJobRequest> {
    let wire: JobRequestWire = decode_json(payload)?;
    let command_type = wire
        .command
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let raw_command = serde_json::to_vec(&wire.command)?;
    let raw_payload_hash = payload_hash(&raw_command);
    match serde_json::from_value::<JobCommand>(wire.command) {
        Ok(command) => Ok(DecodedJobRequest::Supported(Box::new(JobRequest {
            job_id: wire.job_id,
            command_version: wire.command_version,
            command,
            max_timeout_secs: wire.max_timeout_secs,
        }))),
        Err(error) => Ok(DecodedJobRequest::Unsupported(UnsupportedJobRequest {
            job_id: wire.job_id,
            command_version: wire.command_version,
            command_type,
            payload_hash: raw_payload_hash,
            message: error.to_string(),
        })),
    }
}

async fn handle_command_cancel_frame(
    stream: &mut NoiseFrameStream<TcpStream>,
    seq: &mut u64,
    active_commands: &mut HashMap<uuid::Uuid, ActiveCommand>,
    request: JobCancelRequest,
) -> Result<()> {
    let (accepted, applied, message) = match active_commands.get_mut(&request.job_id) {
        Some(active) => {
            let reason = request.reason.unwrap_or_else(|| "canceled".to_string());
            active.cancel_token.cancel(reason.clone());
            if active.finished && active.pending_outputs.is_empty() {
                (true, true, reason)
            } else {
                (
                    true,
                    false,
                    format!("{reason}; cancel requested, command worker still finalizing"),
                )
            }
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

fn unsupported_command_shape_output(request: &UnsupportedJobRequest) -> Result<CommandOutput> {
    let status = serde_json::json!({
        "type": "unsupported_command_version",
        "status": "rejected",
        "job_id": request.job_id,
        "command_version": request.command_version,
        "command_type": request.command_type,
        "reason": "agent_binary_does_not_support_command_shape",
    });
    Ok(CommandOutput {
        job_id: request.job_id,
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
    max_timeout_secs: u64,
    streamed_output_tx: mpsc::Sender<CommandOutput>,
    update_verification_tx: mpsc::Sender<AgentUpdateVerificationWork>,
    terminal_stream_tx: mpsc::Sender<TerminalStreamOutput>,
    cancel_token: CommandCancelToken,
) -> Result<Vec<CommandOutput>> {
    let operation_type = job_command_type_label(&request.command);
    let request_payload_hash = command_payload_hash(&request.command)?;
    cancel_token.check(operation_type)?;
    match &request.command {
        JobCommand::ConfigRead => {
            anyhow::bail!("config updates must run on the main agent task")
        }
        JobCommand::Backup {
            paths,
            include_config,
            follow_symlinks,
            missing_path_policy,
        } => {
            execute_backup_command(BackupCommandInput {
                job_id: request.job_id,
                config: &config,
                config_path: &config_path,
                paths,
                include_config: *include_config,
                follow_symlinks: *follow_symlinks,
                missing_path_policy: *missing_path_policy,
                output_tx: Some(streamed_output_tx),
                max_timeout_secs,
                cancel_token: cancel_token.clone(),
            })
            .await
        }
        JobCommand::Restore {
            source_backup_request_id,
            archive_transfer_session_id: _,
            paths,
            include_config,
            destination_root,
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
                archive_size_bytes: *archive_size_bytes,
                archive_sha256_hex: archive_sha256_hex.as_deref(),
                max_archive_bytes: config.backup.max_archive_bytes,
                dry_run: *dry_run,
                post_restore_argv,
                max_timeout_secs,
                cancel_token: cancel_token.clone(),
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
                max_timeout_secs,
                cancel_token: cancel_token.clone(),
            })
            .await
        }
        JobCommand::NetworkStatus {
            plan,
            side,
            runtime_adapter,
            ..
        } => {
            execute_network_status_command(NetworkStatusInput {
                job_id: request.job_id,
                config: &config,
                plan,
                runtime_adapter: runtime_adapter.as_ref(),
                side: *side,
                max_timeout_secs,
                cancel_token: cancel_token.clone(),
            })
            .await
        }
        JobCommand::NetworkProbe {
            plan,
            side,
            count,
            interval_ms,
            ..
        } => {
            execute_network_probe_command(NetworkProbeInput {
                job_id: request.job_id,
                config: &config,
                plan,
                side: *side,
                count: *count,
                interval_ms: *interval_ms,
                max_timeout_secs,
                cancel_token: cancel_token.clone(),
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
            ..
        } => {
            execute_network_speed_test_command(NetworkSpeedTestInput {
                job_id: request.job_id,
                command_payload_hash: &request_payload_hash,
                config: &config,
                plan,
                server_side: *server_side,
                duration_secs: *duration_secs,
                max_bytes: *max_bytes,
                rate_limit_kbps: *rate_limit_kbps,
                port: *port,
                connect_timeout_ms: *connect_timeout_ms,
                max_timeout_secs,
                cancel_token: cancel_token.clone(),
            })
            .await
        }
        JobCommand::NetworkRoutingStatus {
            plan_id,
            plan,
            side,
            adapter,
        } => {
            execute_network_routing_adapter_command(NetworkRoutingAdapterInput {
                job_id: request.job_id,
                client_id: &config.client_id,
                plan_id,
                plan,
                side: *side,
                adapter,
                expected_current_cost: None,
                desired_cost: None,
                max_timeout_secs,
                cancel_token: cancel_token.clone(),
            })
            .await
        }
        JobCommand::NetworkRoutingApply {
            plan_id,
            plan,
            side,
            adapter,
            expected_current_cost,
            desired_cost,
        } => {
            execute_network_routing_adapter_command(NetworkRoutingAdapterInput {
                job_id: request.job_id,
                client_id: &config.client_id,
                plan_id,
                plan,
                side: *side,
                adapter,
                expected_current_cost: *expected_current_cost,
                desired_cost: Some(*desired_cost),
                max_timeout_secs,
                cancel_token: cancel_token.clone(),
            })
            .await
        }
        JobCommand::UpdateAgent {
            artifact_url,
            sha256_hex,
        } => {
            execute_update_agent(AgentUpdateInput {
                job_id: request.job_id,
                artifact_url,
                sha256_hex,
                max_timeout_secs,
                cancel_token: cancel_token.clone(),
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
                max_timeout_secs,
                cancel_token: cancel_token.clone(),
                verification_tx: Some(update_verification_tx),
            })
            .await
        }
        JobCommand::TerminalOpen { .. } => {
            run_cancelable(
                "terminal",
                cancel_token,
                execute_terminal_command_with_stream_sink(
                    &config,
                    request.job_id,
                    &request.command,
                    max_timeout_secs,
                    Some(terminal_stream_tx),
                ),
            )
            .await
        }
        command => {
            execute_job_command_with_config_cancel_and_output_sink(
                &config,
                request.job_id,
                command,
                max_timeout_secs,
                cancel_token,
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
    command_runtime: &mut AgentCommandRuntime,
    result: CommandExecutionResult,
) -> Result<()> {
    let (payload_hash, replay_outputs, terminal_output, replay_truncated) = {
        let Some(active) = command_runtime.active_commands.get_mut(&result.job_id) else {
            return Ok(());
        };
        let final_outputs = command_result_outputs(
            result.job_id,
            result.operation_type,
            result.max_timeout_secs,
            result.result,
        );
        for output in final_outputs {
            enqueue_active_command_output(active, output);
        }
        active.finished = true;
        let replay_outputs = active.replay_outputs.clone();
        let replay_truncated = active.replay_truncated
            || sequenced_command_outputs_bytes(&replay_outputs) > 1024 * 1024;
        (
            active.payload_hash.clone(),
            replay_outputs,
            active.terminal_output.clone(),
            replay_truncated,
        )
    };
    command_runtime.recent_commands.remember(
        result.job_id,
        payload_hash.clone(),
        replay_outputs,
        terminal_output.clone(),
        replay_truncated,
    );
    if let Some(ledger) = command_runtime.command_ledger.as_ref() {
        ledger
            .record(
                result.job_id,
                payload_hash,
                compact_ledger_terminal_output(terminal_output),
                true,
            )
            .await?;
    }
    if let Some(active) = command_runtime.active_commands.get_mut(&result.job_id) {
        flush_pending_command_outputs(stream, seq, active).await?;
    }
    remove_finished_flushed_commands(&mut command_runtime.active_commands);
    Ok(())
}

#[cfg(test)]
#[path = "tests_runtime.rs"]
mod tests;
