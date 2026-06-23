use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tokio::{
    net::TcpStream,
    sync::{mpsc, oneshot},
    time,
};
use tracing::{debug, info, warn};
#[cfg(test)]
use vpsman_common::CURRENT_COMMAND_PROTOCOL_VERSION;
use vpsman_common::{
    decode_json, decode_noise_key_hex, encode_json, job_command_min_supported_protocol_version,
    job_command_protocol_version, job_command_safety, job_command_type_label,
    maybe_compress_payload, payload_hash, runtime_config_content_hash, AgentCapabilitySnapshot,
    AgentConfig, AgentHello, AgentPrivilegeMode, AgentRuntimeConfig,
    AgentRuntimeConfigReloadRequest, AgentSessionDisconnect, AgentUpdateVerificationResult,
    CommandOutput, CommandResume, Frame, JobAck, JobCancelAck, JobCancelRequest, JobCommand,
    JobCommandSafety, JobRequest, MessageKind, NoiseFrameStream, OutputStream,
    SequencedCommandOutput, ServerEndpoint, ServerHello, TelemetryEnvelope, TerminalStreamOutput,
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
    network_runtime::{
        execute_runtime_tunnel_reconcile_report_cancelable,
        execute_runtime_tunnel_remove_report_cancelable, NetworkRuntimeReconcileInput,
        NetworkRuntimeRemoveInput,
    },
    network_speed::{execute_network_speed_test_command, NetworkSpeedTestInput},
    network_status::{execute_network_status_command, NetworkStatusInput},
    restore::{execute_restore_command, RestoreCommandInput},
    restore_rollback::{execute_restore_rollback_command, RestoreRollbackCommandInput},
    supervisor::reconcile_supervised_processes_on_start,
    telemetry::{collect_metrics_for_config, read_optional, TelemetryRuntimeState},
    terminal::{
        close_all_terminal_sessions_for_lifecycle, drain_pending_terminal_final_events,
        execute_terminal_command_with_stream_sink, mark_gateway_connected,
        mark_gateway_disconnected,
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
    let mut command_runtime = AgentCommandRuntime::with_command_ledger(command_ledger);
    let process_incarnation_id = uuid::Uuid::new_v4();
    match reconcile_supervised_processes_on_start().await {
        Ok(report) => log_supervisor_startup_reconcile(&report),
        Err(error) => warn!(%error, "process supervisor startup reconcile failed"),
    }
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

async fn connect_and_stream(
    config: &mut AgentConfig,
    config_path: &Path,
    endpoint: &str,
    command_runtime: &mut AgentCommandRuntime,
    process_incarnation_id: uuid::Uuid,
) -> Result<()> {
    info!(%endpoint, "connecting to gateway");
    let tcp = connect_tcp_endpoint(endpoint, config.auth.gateway_connect_timeout_secs).await?;
    let mut stream = connect_noise_stream(tcp, config).await?;

    let hello = AgentHello {
        client_id: config.client_id.clone(),
        process_incarnation_id,
        agent_version: crate::build_info::agent_release_version().to_string(),
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
    mark_gateway_connected().await;

    let mut seq = 2_u64;
    request_runtime_config_reload(&mut stream, &mut seq, config).await?;
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
    let mut ticker = time::interval(Duration::from_secs(
        server_hello.telemetry_light_secs.max(5),
    ));
    let mut unmanaged_update_schedule = UnmanagedUpdateSchedule::new(config);
    let mut unmanaged_update_sleep =
        Box::pin(time::sleep_until(unmanaged_update_schedule.next_due()));
    let mut pending_update_verifications =
        HashMap::<uuid::Uuid, oneshot::Sender<AgentUpdateVerificationResult>>::new();
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
                            let config_update = result.config_update.take();
                            finish_active_command(
                                &mut stream,
                                &mut seq,
                                &mut *command_runtime,
                                *result,
                            )
                            .await?;
                            if let Some(next_config) = config_update {
                                *config = next_config;
                                ticker = time::interval(Duration::from_secs(config.telemetry_light_secs.max(5)));
                                unmanaged_update_schedule = UnmanagedUpdateSchedule::new(config);
                                unmanaged_update_sleep.as_mut().reset(unmanaged_update_schedule.next_due());
                            }
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

async fn request_runtime_config_reload(
    stream: &mut NoiseFrameStream<TcpStream>,
    seq: &mut u64,
    config: &AgentConfig,
) -> Result<()> {
    let runtime_config = AgentRuntimeConfig::from_agent_config(0, config);
    let current_content_hash = runtime_config_content_hash(&runtime_config)
        .context("failed to hash current runtime config")?;
    let request = AgentRuntimeConfigReloadRequest {
        client_id: config.client_id.clone(),
        current_content_hash,
        reason: "agent_reconnect_runtime_config_check".to_string(),
    };
    send_json_frame(stream, MessageKind::ConfigUpdate, 0, *seq, &request).await?;
    *seq += 1;
    Ok(())
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
                plan,
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
    let previous_tunnels = config.network.runtime_status_telemetry_plans.clone();
    let desired_tunnels = runtime_config
        .network
        .runtime_status_telemetry_plans
        .clone();
    let stale_tunnels = previous_tunnels
        .iter()
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
                plan: &stale.plan,
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

    let mut candidate_config = config.clone();
    runtime_config.apply_to_agent_config(&mut candidate_config);
    cancel_token.check("runtime_config_sync")?;
    let reconcile = reconcile_configured_runtime_tunnels_cancelable(
        &candidate_config,
        "runtime_config_sync",
        cancel_token,
    )
    .await;
    let removal_failed = removals
        .iter()
        .any(|removal| removal.get("status").and_then(serde_json::Value::as_str) == Some("failed"));
    let reconcile_failed =
        reconcile.get("status").and_then(serde_json::Value::as_str) == Some("failed");
    let status = if removal_failed || reconcile_failed {
        "failed"
    } else {
        "applied"
    };
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
        applied_config: (status == "applied").then_some(candidate_config),
    })
}

fn runtime_tunnel_identity_matches(
    left: &vpsman_common::AgentRuntimeStatusTelemetryPlan,
    right: &vpsman_common::AgentRuntimeStatusTelemetryPlan,
) -> bool {
    left.endpoint_side == right.endpoint_side
        && left.plan_id == right.plan_id
        && left.plan.name == right.plan.name
        && left.plan.interface_name == right.plan.interface_name
        && left.plan.kind == right.plan.kind
        && left.plan.runtime_control.manager == right.plan.runtime_control.manager
        && left.plan.left_client_id == right.plan.left_client_id
        && left.plan.right_client_id == right.plan.right_client_id
        && left.plan.left_underlay == right.plan.left_underlay
        && left.plan.right_underlay == right.plan.right_underlay
        && left.plan.left_tunnel_address == right.plan.left_tunnel_address
        && left.plan.right_tunnel_address == right.plan.right_tunnel_address
        && left.plan.tunnel_prefix_len == right.plan.tunnel_prefix_len
        && left.plan.ipv4_tunnel == right.plan.ipv4_tunnel
        && left.plan.ipv6_tunnel == right.plan.ipv6_tunnel
        && runtime_tunnel_control_identity_matches(
            &left.plan.runtime_control,
            &right.plan.runtime_control,
        )
}

fn runtime_tunnel_control_identity_matches(
    left: &vpsman_common::RuntimeTunnelControl,
    right: &vpsman_common::RuntimeTunnelControl,
) -> bool {
    match left.manager {
        vpsman_common::RuntimeTunnelManager::AgentIproute2Managed => left.fou == right.fou,
        vpsman_common::RuntimeTunnelManager::ExternalObserved => true,
        vpsman_common::RuntimeTunnelManager::ExternalManagedAdapter => {
            left.startup == right.startup
                && left.stop == right.stop
                && left.cleanup == right.cleanup
                && left.restart == right.restart
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
        max_job_timeout_secs: config.auth.max_job_timeout_secs.max(1),
        network_backend: config.network.backend,
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

struct AgentCommandRuntime {
    active_commands: HashMap<uuid::Uuid, ActiveCommand>,
    recent_commands: RecentCommandCache,
    command_ledger: Option<CommandLedger>,
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
    fn with_command_ledger(command_ledger: CommandLedger) -> Self {
        Self {
            command_ledger: Some(command_ledger),
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
        data: format!("command failed: {error}").into_bytes(),
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
            let (result, config_update) = match result {
                Ok(Ok(sync)) => (Ok(sync.outputs), sync.applied_config),
                Ok(Err(error)) => (Err(error), None),
                Err(error) => {
                    task_cancel_token.cancel("runtime_config_sync_timeout".to_string());
                    (
                        Err(anyhow::anyhow!("runtime config sync timed out: {error}")),
                        None,
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
        } => {
            execute_backup_command(BackupCommandInput {
                job_id: request.job_id,
                config: &config,
                config_path: &config_path,
                paths,
                include_config: *include_config,
                follow_symlinks: *follow_symlinks,
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
        JobCommand::NetworkStatus { plan, side } => {
            execute_network_status_command(NetworkStatusInput {
                job_id: request.job_id,
                config: &config,
                plan,
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
        JobCommand::TerminalOpen { .. }
        | JobCommand::TerminalInput { .. }
        | JobCommand::TerminalPoll { .. }
        | JobCommand::TerminalResize { .. }
        | JobCommand::TerminalClose { .. } => {
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
            Some(75)
        );
        let status: serde_json::Value =
            serde_json::from_slice(&entry.terminal_output.as_ref().unwrap().output.data).unwrap();
        assert_eq!(status["type"], "duplicate_job_replay_unavailable");
        assert_eq!(status["status"], "failed");
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
            cancel_token: CommandCancelToken::default(),
            command_version: CURRENT_COMMAND_PROTOCOL_VERSION,
            safety: JobCommandSafety::Read,
            stream_id: 1,
            replay_outputs: Vec::new(),
            terminal_output: None,
            replay_output_bytes: 0,
            replay_truncated: false,
            pending_outputs: VecDeque::new(),
            next_output_seq: 0,
            finished: false,
            _task: tokio::spawn(async move {
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

    #[tokio::test]
    async fn runtime_config_sync_returns_applied_candidate_without_mutating_source() {
        let base = AgentConfig {
            client_id: "client-a".to_string(),
            display_name: "old-name".to_string(),
            telemetry_light_secs: 15,
            ..AgentConfig::default()
        };
        let desired = AgentRuntimeConfig {
            version: 9,
            display_name: "new-name".to_string(),
            telemetry_light_secs: 30,
            ..AgentRuntimeConfig::default()
        };

        let result = apply_runtime_config_sync(
            uuid::Uuid::new_v4(),
            &base,
            &desired,
            9,
            "test-success",
            CommandCancelToken::default(),
        )
        .await
        .unwrap();

        assert_eq!(base.display_name, "old-name");
        assert_eq!(result.outputs[0].exit_code, Some(0));
        let applied = result.applied_config.expect("sync should apply");
        assert_eq!(applied.display_name, "new-name");
        assert_eq!(applied.telemetry_light_secs, 30);
    }

    #[test]
    fn runtime_tunnel_identity_allows_in_place_policy_changes() {
        let mut changed_cost = runtime_sync_test_telemetry_plan(runtime_sync_test_plan(
            "203.0.113.20",
            "10.255.0.0",
            "10.255.0.1",
        ));
        changed_cost.plan.recommended_ospf_cost =
            changed_cost.plan.recommended_ospf_cost.saturating_add(10);
        let baseline = runtime_sync_test_telemetry_plan(runtime_sync_test_plan(
            "203.0.113.20",
            "10.255.0.0",
            "10.255.0.1",
        ));

        assert!(runtime_tunnel_identity_matches(&baseline, &changed_cost));
    }

    #[test]
    fn runtime_tunnel_identity_detects_immutable_plan_changes() {
        let baseline = runtime_sync_test_telemetry_plan(runtime_sync_test_plan(
            "203.0.113.20",
            "10.255.0.0",
            "10.255.0.1",
        ));
        let changed_underlay = runtime_sync_test_telemetry_plan(runtime_sync_test_plan(
            "203.0.113.99",
            "10.255.0.0",
            "10.255.0.1",
        ));
        let changed_address = runtime_sync_test_telemetry_plan(runtime_sync_test_plan(
            "203.0.113.20",
            "10.255.0.2",
            "10.255.0.3",
        ));

        assert!(!runtime_tunnel_identity_matches(
            &baseline,
            &changed_underlay
        ));
        assert!(!runtime_tunnel_identity_matches(
            &baseline,
            &changed_address
        ));
    }

    #[tokio::test]
    async fn runtime_config_sync_recreates_tunnel_when_plan_identity_changes() {
        let root = std::env::temp_dir().join(format!(
            "vpsman-runtime-sync-recreate-{}",
            uuid::Uuid::new_v4()
        ));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let old_plan = runtime_sync_test_telemetry_plan(runtime_sync_test_plan(
            "203.0.113.20",
            "10.255.0.0",
            "10.255.0.1",
        ));
        let new_plan = runtime_sync_test_telemetry_plan(runtime_sync_test_plan(
            "203.0.113.99",
            "10.255.0.0",
            "10.255.0.1",
        ));
        let base = AgentConfig {
            client_id: "left-a".to_string(),
            network: vpsman_common::AgentNetworkConfig {
                apply_enabled: true,
                runtime_reconcile_enabled: true,
                root_dir: root.to_string_lossy().to_string(),
                runtime_ip_argv: vec!["/bin/echo".to_string()],
                runtime_tc_argv: vec!["/bin/echo".to_string()],
                runtime_unprivileged_mutation_policy:
                    vpsman_common::AgentRuntimeUnprivilegedMutationPolicy::TryAll,
                runtime_status_telemetry_plans: vec![old_plan],
                ..Default::default()
            },
            ..AgentConfig::default()
        };
        let mut desired = AgentRuntimeConfig::from_agent_config(12, &base);
        desired.network.runtime_status_telemetry_plans = vec![new_plan];

        let result = apply_runtime_config_sync(
            uuid::Uuid::new_v4(),
            &base,
            &desired,
            12,
            "test-plan-identity-change",
            CommandCancelToken::default(),
        )
        .await
        .unwrap();

        assert_eq!(result.outputs[0].exit_code, Some(0));
        let body: serde_json::Value = serde_json::from_slice(&result.outputs[0].data).unwrap();
        assert_eq!(body["status"], "applied");
        assert_eq!(body["removed_tunnel_count"], 1);
        assert_eq!(body["removals"][0]["plan_id"], "plan-a");
        assert_eq!(body["reconcile"]["total"], 1);
        assert_eq!(
            result
                .applied_config
                .expect("identity change sync should apply")
                .network
                .runtime_status_telemetry_plans[0]
                .plan
                .right_underlay,
            "203.0.113.99"
        );
    }

    #[tokio::test]
    async fn runtime_config_sync_failure_does_not_return_config_update() {
        let root =
            std::env::temp_dir().join(format!("vpsman-runtime-sync-fail-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let plan = vpsman_common::plan_tunnel(&vpsman_common::TunnelPlanInput {
            name: "left-right".to_string(),
            interface_name: "tunlr".to_string(),
            kind: vpsman_common::TunnelKind::Gre,
            runtime_control: Default::default(),
            runtime_topology: Default::default(),
            left_client_id: "client-a".to_string(),
            right_client_id: "client-b".to_string(),
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
        let base = AgentConfig {
            client_id: "client-a".to_string(),
            display_name: "old-name".to_string(),
            network: vpsman_common::AgentNetworkConfig {
                root_dir: root.to_string_lossy().to_string(),
                runtime_ip_argv: vec!["/bin/false".to_string()],
                runtime_tc_argv: vec!["/bin/false".to_string()],
                runtime_command_timeout_secs: 1,
                runtime_unprivileged_mutation_policy:
                    vpsman_common::AgentRuntimeUnprivilegedMutationPolicy::TryAll,
                ..Default::default()
            },
            ..AgentConfig::default()
        };
        let mut desired = AgentRuntimeConfig {
            version: 10,
            display_name: "new-name".to_string(),
            ..AgentRuntimeConfig::from_agent_config(10, &base)
        };
        desired.network.apply_enabled = true;
        desired.network.runtime_reconcile_enabled = true;
        desired.network.runtime_status_telemetry_plans.push(
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
        );

        let result = apply_runtime_config_sync(
            uuid::Uuid::new_v4(),
            &base,
            &desired,
            10,
            "test-failure",
            CommandCancelToken::default(),
        )
        .await
        .unwrap();

        assert_eq!(base.display_name, "old-name");
        assert_eq!(result.outputs[0].exit_code, Some(1));
        assert!(result.applied_config.is_none());
    }

    #[tokio::test]
    async fn runtime_config_sync_cancel_returns_no_config_update() {
        let token = CommandCancelToken::default();
        token.cancel("operator requested cancellation".to_string());
        let base = AgentConfig {
            client_id: "client-a".to_string(),
            ..AgentConfig::default()
        };
        let desired = AgentRuntimeConfig {
            version: 11,
            ..AgentRuntimeConfig::default()
        };

        let error = apply_runtime_config_sync(
            uuid::Uuid::new_v4(),
            &base,
            &desired,
            11,
            "test-cancel",
            token,
        )
        .await
        .unwrap_err();

        let canceled = error
            .downcast_ref::<CommandCanceled>()
            .expect("runtime sync should surface cancellation");
        assert_eq!(canceled.reason(), "operator requested cancellation");
    }

    #[test]
    fn runtime_config_sync_timeout_maps_to_command_timeout_output() {
        let job_id = uuid::Uuid::new_v4();
        let outputs = command_result_outputs(
            job_id,
            "runtime_config_sync",
            17,
            Err(anyhow::anyhow!(
                "runtime config sync timed out: deadline elapsed"
            )),
        );

        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].exit_code, Some(124));
        assert!(outputs[0].done);
        let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
        assert_eq!(status["type"], "command_timeout");
        assert_eq!(status["operation_type"], "runtime_config_sync");
        assert_eq!(status["max_timeout_secs"], 17);
    }

    fn runtime_sync_test_plan(
        right_underlay: &str,
        left_tunnel: &str,
        right_tunnel: &str,
    ) -> vpsman_common::TunnelPlan {
        vpsman_common::plan_tunnel(&vpsman_common::TunnelPlanInput {
            name: "left-right".to_string(),
            interface_name: "tunlr".to_string(),
            kind: vpsman_common::TunnelKind::Gre,
            runtime_control: Default::default(),
            runtime_topology: Default::default(),
            left_client_id: "left-a".to_string(),
            right_client_id: "right-b".to_string(),
            left_underlay: "198.51.100.10".to_string(),
            right_underlay: right_underlay.to_string(),
            address_pool_cidr: "10.255.0.0/30".to_string(),
            reserved_addresses: Vec::new(),
            ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
                left: left_tunnel.to_string(),
                right: right_tunnel.to_string(),
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
        .unwrap()
    }

    fn runtime_sync_test_telemetry_plan(
        plan: vpsman_common::TunnelPlan,
    ) -> vpsman_common::AgentRuntimeStatusTelemetryPlan {
        vpsman_common::AgentRuntimeStatusTelemetryPlan {
            plan_id: Some("plan-a".to_string()),
            endpoint_side: vpsman_common::TunnelEndpointSide::Left,
            plan,
            traffic_source: Default::default(),
            traffic_command: None,
            latency_monitoring_enabled: true,
            auto_ospf_enabled: false,
            auto_ospf_updater: None,
        }
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
