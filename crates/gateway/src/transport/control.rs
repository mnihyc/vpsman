use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, UnixListener},
    sync::{mpsc, oneshot, watch},
    task::JoinSet,
    time,
};
use tracing::{info, warn};
use vpsman_common::{
    verify_privilege_assertion, GatewayClientSuspensionFenceClear,
    GatewayClientSuspensionFencePrepare, GatewayClientSuspensionFencePromote,
    GatewayClientSuspensionFenceResult, GatewayCommandCancel, GatewayCommandCancelResult,
    GatewayCommandDispatch, GatewayCommandDispatchResult, GatewayPrivilegeVerification,
    GatewayPrivilegeVerificationResult, GatewaySessionDisconnect, GatewaySessionDisconnectResult,
    GatewayTerminalControl, GatewayTerminalControlResult, PrivilegeAssertionError,
};

use crate::{
    state::{
        GatewayCancelCommand, GatewayClientSuspensionFence, GatewayCommand,
        GatewayCommandEnqueueMarker, GatewaySessionCloseRequest, GatewaySessionMessage,
        GatewayState, GatewayTerminalControlCommand,
    },
    Args,
};

const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) async fn run_control_listener(
    args: Args,
    state: GatewayState,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    if let Some(path) = control_socket_path(&args.control_bind) {
        prepare_control_socket(&path)?;
        let listener = UnixListener::bind(&path)
            .with_context(|| format!("failed to bind gateway control socket {}", path.display()))?;
        info!(path = %path.display(), "gateway control listening on Unix socket");
        let mut connections = JoinSet::new();
        let (connection_shutdown, connection_shutdown_rx) = watch::channel(false);
        let mut listener_error = None;

        loop {
            let accepted = tokio::select! {
                biased;
                _ = wait_for_shutdown(&mut shutdown) => break,
                accepted = listener.accept() => accepted,
                completed = connections.join_next(), if !connections.is_empty() => {
                    if let Some(Err(error)) = completed {
                        warn!(%error, "gateway Unix control connection consumer failed");
                    }
                    continue;
                }
            };
            let (stream, _) = match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    listener_error = Some(error);
                    break;
                }
            };
            let state = state.clone();
            let internal_token = args.internal_token.clone();
            let privilege_verifier_key_hex = args.privilege_verifier_key_hex.clone();
            let mut connection_shutdown = connection_shutdown_rx.clone();
            connections.spawn(async move {
                tokio::select! {
                    result = handle_control_connection(
                        stream,
                        state,
                        internal_token,
                        privilege_verifier_key_hex,
                    ) => {
                        if let Err(error) = result {
                            warn!(%error, "gateway Unix control request failed");
                        }
                    }
                    _ = wait_for_shutdown(&mut connection_shutdown) => {}
                }
            });
        }
        drop(listener);
        let _ = connection_shutdown.send(true);
        drain_control_connections(&mut connections, "Unix").await;
        if let Some(error) = listener_error {
            return Err(error).context("gateway Unix control listener failed");
        }
        return Ok(());
    }
    let listener = TcpListener::bind(&args.control_bind)
        .await
        .with_context(|| format!("failed to bind gateway control on {}", args.control_bind))?;
    info!(bind = %args.control_bind, "gateway control listening");
    let mut connections = JoinSet::new();
    let (connection_shutdown, connection_shutdown_rx) = watch::channel(false);
    let mut listener_error = None;

    loop {
        let accepted = tokio::select! {
            biased;
            _ = wait_for_shutdown(&mut shutdown) => break,
            accepted = listener.accept() => accepted,
            completed = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = completed {
                    warn!(%error, "gateway TCP control connection consumer failed");
                }
                continue;
            }
        };
        let (stream, peer) = match accepted {
            Ok(accepted) => accepted,
            Err(error) => {
                listener_error = Some(error);
                break;
            }
        };
        let state = state.clone();
        let internal_token = args.internal_token.clone();
        let privilege_verifier_key_hex = args.privilege_verifier_key_hex.clone();
        let mut connection_shutdown = connection_shutdown_rx.clone();
        connections.spawn(async move {
            tokio::select! {
                result = handle_control_connection(
                    stream,
                    state,
                    internal_token,
                    privilege_verifier_key_hex,
                ) => {
                    if let Err(error) = result {
                        warn!(%peer, %error, "gateway control request failed");
                    }
                }
                _ = wait_for_shutdown(&mut connection_shutdown) => {}
            }
        });
    }
    drop(listener);
    let _ = connection_shutdown.send(true);
    drain_control_connections(&mut connections, "TCP").await;
    match listener_error {
        Some(error) => Err(error).context("gateway TCP control listener failed"),
        None => Ok(()),
    }
}

fn shutdown_requested(shutdown: &watch::Receiver<bool>) -> bool {
    *shutdown.borrow() || shutdown.has_changed().is_err()
}

async fn wait_for_shutdown(shutdown: &mut watch::Receiver<bool>) {
    while !shutdown_requested(shutdown) {
        if shutdown.changed().await.is_err() {
            return;
        }
    }
}

async fn drain_control_connections(connections: &mut JoinSet<()>, transport: &str) {
    while let Some(completed) = connections.join_next().await {
        if let Err(error) = completed {
            warn!(%error, transport, "gateway control connection consumer failed during drain");
        }
    }
}

fn control_socket_path(value: &str) -> Option<PathBuf> {
    let value = value.trim();
    value
        .strip_prefix("unix://")
        .or_else(|| value.strip_prefix("unix:"))
        .map(PathBuf::from)
}

fn prepare_control_socket(path: &Path) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create control socket directory {}",
                parent.display()
            )
        })?;
    }
    if path.exists() {
        fs::remove_file(path)
            .with_context(|| format!("failed to remove stale control socket {}", path.display()))?;
    }
    Ok(())
}

async fn handle_control_connection<S>(
    mut stream: S,
    state: GatewayState,
    internal_token: Option<String>,
    privilege_verifier_key_hex: Option<String>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let request = read_http_request(&mut stream).await?;
    if request.method != "POST"
        || !matches!(
            request.path.as_str(),
            "/internal/v1/gateway/command"
                | "/internal/v1/gateway/command/cancel"
                | "/internal/v1/gateway/session/disconnect"
                | "/internal/v1/gateway/client/suspension-fence/prepare"
                | "/internal/v1/gateway/client/suspension-fence/promote"
                | "/internal/v1/gateway/client/suspension-fence/clear"
                | "/internal/v1/gateway/terminal/control"
                | "/internal/v1/gateway/metrics"
                | "/internal/v1/gateway/privilege/verify"
        )
    {
        write_http_json(
            &mut stream,
            "404 Not Found",
            &serde_json::json!({"error": "not_found"}),
        )
        .await?;
        return Ok(());
    }
    if !authorized_internal_request(&request.headers, internal_token.as_deref()) {
        write_http_json(
            &mut stream,
            "401 Unauthorized",
            &serde_json::json!({"error": "invalid_internal_token"}),
        )
        .await?;
        return Ok(());
    }

    if request.path == "/internal/v1/gateway/command" {
        let dispatch: GatewayCommandDispatch = match serde_json::from_slice(&request.body) {
            Ok(dispatch) => dispatch,
            Err(error) => {
                write_http_json(
                    &mut stream,
                    "400 Bad Request",
                    &serde_json::json!({"error": format!("invalid_command_dispatch:{error}")}),
                )
                .await?;
                return Ok(());
            }
        };
        match dispatch_gateway_command(&state, dispatch).await {
            Ok(result) => write_http_json(&mut stream, "200 OK", &result).await?,
            Err(error) => write_gateway_error(&mut stream, error).await?,
        }
    } else if request.path == "/internal/v1/gateway/command/cancel" {
        let cancel: GatewayCommandCancel = match serde_json::from_slice(&request.body) {
            Ok(cancel) => cancel,
            Err(error) => {
                write_http_json(
                    &mut stream,
                    "400 Bad Request",
                    &serde_json::json!({"error": format!("invalid_command_cancel:{error}")}),
                )
                .await?;
                return Ok(());
            }
        };
        match cancel_gateway_command(&state, cancel).await {
            Ok(result) => write_http_json(&mut stream, "200 OK", &result).await?,
            Err(error) => write_gateway_error(&mut stream, error).await?,
        }
    } else if request.path == "/internal/v1/gateway/session/disconnect" {
        let disconnect: GatewaySessionDisconnect = match serde_json::from_slice(&request.body) {
            Ok(disconnect) => disconnect,
            Err(error) => {
                write_http_json(
                    &mut stream,
                    "400 Bad Request",
                    &serde_json::json!({"error": format!("invalid_session_disconnect:{error}")}),
                )
                .await?;
                return Ok(());
            }
        };
        match disconnect_gateway_session(&state, disconnect).await {
            Ok(result) => write_http_json(&mut stream, "200 OK", &result).await?,
            Err(error) => write_gateway_error(&mut stream, error).await?,
        }
    } else if request.path == "/internal/v1/gateway/client/suspension-fence/prepare" {
        let prepare: GatewayClientSuspensionFencePrepare = match serde_json::from_slice(
            &request.body,
        ) {
            Ok(prepare) => prepare,
            Err(error) => {
                write_http_json(
                        &mut stream,
                        "400 Bad Request",
                        &serde_json::json!({"error": format!("invalid_suspension_fence_prepare:{error}")}),
                    )
                    .await?;
                return Ok(());
            }
        };
        let result = prepare_gateway_client_suspension_fence(&state, prepare).await;
        write_http_json(&mut stream, "200 OK", &result).await?;
    } else if request.path == "/internal/v1/gateway/client/suspension-fence/promote" {
        let promote: GatewayClientSuspensionFencePromote = match serde_json::from_slice(
            &request.body,
        ) {
            Ok(promote) => promote,
            Err(error) => {
                write_http_json(
                        &mut stream,
                        "400 Bad Request",
                        &serde_json::json!({"error": format!("invalid_suspension_fence_promote:{error}")}),
                    )
                    .await?;
                return Ok(());
            }
        };
        let result = promote_gateway_client_suspension_fence(&state, promote).await;
        write_http_json(&mut stream, "200 OK", &result).await?;
    } else if request.path == "/internal/v1/gateway/client/suspension-fence/clear" {
        let clear: GatewayClientSuspensionFenceClear = match serde_json::from_slice(&request.body) {
            Ok(clear) => clear,
            Err(error) => {
                write_http_json(
                    &mut stream,
                    "400 Bad Request",
                    &serde_json::json!({"error": format!("invalid_suspension_fence_clear:{error}")}),
                )
                .await?;
                return Ok(());
            }
        };
        let result = clear_gateway_client_suspension_fence(&state, clear).await;
        write_http_json(&mut stream, "200 OK", &result).await?;
    } else if request.path == "/internal/v1/gateway/terminal/control" {
        let control: GatewayTerminalControl = match serde_json::from_slice(&request.body) {
            Ok(control) => control,
            Err(error) => {
                write_http_json(
                    &mut stream,
                    "400 Bad Request",
                    &serde_json::json!({"error": format!("invalid_terminal_control:{error}")}),
                )
                .await?;
                return Ok(());
            }
        };
        match dispatch_terminal_control(&state, control).await {
            Ok(result) => write_http_json(&mut stream, "200 OK", &result).await?,
            Err(error) => write_gateway_error(&mut stream, error).await?,
        }
    } else if request.path == "/internal/v1/gateway/metrics" {
        write_http_json(&mut stream, "200 OK", &state.forward_metrics.snapshot()).await?;
    } else {
        let verification: GatewayPrivilegeVerification = match serde_json::from_slice(&request.body)
        {
            Ok(verification) => verification,
            Err(error) => {
                write_http_json(
                        &mut stream,
                        "400 Bad Request",
                        &serde_json::json!({"error": format!("invalid_privilege_verification:{error}")}),
                    )
                    .await?;
                return Ok(());
            }
        };
        match verify_gateway_privilege(&state, privilege_verifier_key_hex.as_deref(), verification)
            .await
        {
            Ok(result) => write_http_json(&mut stream, "200 OK", &result).await?,
            Err(error) => write_privilege_error(&mut stream, error).await?,
        }
    }
    Ok(())
}

async fn dispatch_terminal_control(
    state: &GatewayState,
    control: GatewayTerminalControl,
) -> Result<GatewayTerminalControlResult> {
    let Some(session) = state.sessions.read().await.get(&control.client_id).cloned() else {
        return Err(anyhow!("agent_not_online:{}", control.client_id));
    };
    if session.process_incarnation_id != control.expected_process_incarnation_id {
        return Err(anyhow!(
            "agent_incarnation_mismatch:{}:expected={}:actual={}",
            control.client_id,
            control.expected_process_incarnation_id,
            session.process_incarnation_id
        ));
    }
    let (response_tx, response_rx) = oneshot::channel();
    session
        .sender
        .try_send(GatewaySessionMessage::TerminalControl(
            GatewayTerminalControlCommand {
                request: control.request,
                response: response_tx,
            },
        ))
        .map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => {
                anyhow!("agent_session_command_queue_full:{}", control.client_id)
            }
            mpsc::error::TrySendError::Closed(_) => {
                anyhow!("agent_session_closed:{}", control.client_id)
            }
        })?;
    let ack = time::timeout(Duration::from_secs(state.dispatch_ack_secs()), response_rx)
        .await
        .context("gateway terminal control timed out")?
        .context("gateway terminal control response dropped")?;
    Ok(GatewayTerminalControlResult {
        client_id: control.client_id,
        ack,
    })
}

async fn verify_gateway_privilege(
    state: &GatewayState,
    verifier_key_hex: Option<&str>,
    verification: GatewayPrivilegeVerification,
) -> Result<GatewayPrivilegeVerificationResult> {
    let verifier_key = decode_verifier_key(verifier_key_hex)?;
    let now_unix = unix_now();
    let mut replay_cache = state.privilege_assertions.lock().await;
    let intent_hash_hex = verify_privilege_assertion(
        &verifier_key,
        &verification.intent,
        &verification.assertion,
        now_unix,
        &mut replay_cache,
    )
    .map_err(|error| anyhow!("privilege_assertion_{error:?}"))?;
    Ok(GatewayPrivilegeVerificationResult {
        approved: true,
        intent_hash_hex,
        message: "approved".to_string(),
    })
}

fn decode_verifier_key(value: Option<&str>) -> Result<[u8; 32]> {
    let value = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("privilege verifier is not configured")?;
    let bytes = hex::decode(value).context("privilege verifier key must be hex")?;
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("privilege verifier key must be 32 bytes"))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

async fn dispatch_gateway_command(
    state: &GatewayState,
    dispatch: GatewayCommandDispatch,
) -> Result<GatewayCommandDispatchResult> {
    let grace_deadline = state
        .disconnected_at
        .read()
        .await
        .get(&dispatch.client_id)
        .map(|disconnected| *disconnected + Duration::from_secs(state.reconnect_grace_secs()));

    loop {
        let lifecycle_owner = state.client_lifecycle_owner(&dispatch.client_id).await;
        let client_lifecycle = lifecycle_owner.read().await;
        let fenced = state
            .client_suspension_fences
            .read()
            .await
            .get(&dispatch.client_id)
            .copied()
            .is_some_and(|fence| fence.active_at(Instant::now()));
        if fenced {
            return Err(anyhow!("agent_suspended:{}", dispatch.client_id));
        }
        let session = {
            let sessions = state.sessions.read().await;
            sessions.get(&dispatch.client_id).cloned()
        };
        if let Some(session) = session {
            if session.process_incarnation_id != dispatch.expected_process_incarnation_id {
                return Err(anyhow!(
                    "agent_incarnation_mismatch:{}:expected={}:actual={}",
                    dispatch.client_id,
                    dispatch.expected_process_incarnation_id,
                    session.process_incarnation_id
                ));
            }
            let (response_tx, response_rx) = oneshot::channel();
            let enqueue_key = (dispatch.client_id.clone(), dispatch.request.job_id);
            let enqueue_expiry = Instant::now()
                + Duration::from_secs(
                    dispatch
                        .request
                        .max_timeout_secs
                        .saturating_add(state.dispatch_ack_secs())
                        .saturating_add(60)
                        .clamp(60, 10_800),
                );
            let enqueue_marker = GatewayCommandEnqueueMarker {
                generation: uuid::Uuid::new_v4(),
                expires_at: enqueue_expiry,
            };
            let prior_enqueue_marker = state
                .command_enqueues
                .write()
                .await
                .insert(enqueue_key.clone(), enqueue_marker);
            if let Err(error) = session
                .sender
                .try_send(GatewaySessionMessage::Command(Box::new(GatewayCommand {
                    request: dispatch.request.clone(),
                    payload_hash: dispatch.payload_hash.clone(),
                    response: response_tx,
                })))
            {
                rollback_failed_command_enqueue(
                    state,
                    enqueue_key,
                    enqueue_marker,
                    prior_enqueue_marker,
                )
                .await;
                return Err(match error {
                    mpsc::error::TrySendError::Full(_) => {
                        anyhow!("agent_session_command_queue_full:{}", dispatch.client_id)
                    }
                    mpsc::error::TrySendError::Closed(_) => {
                        anyhow!("agent_session_closed:{}", dispatch.client_id)
                    }
                });
            }
            // Fence installation needs only to order against enqueue. Do not
            // retain the lifecycle guard while waiting for an agent ACK.
            drop(client_lifecycle);
            return time::timeout(Duration::from_secs(state.dispatch_ack_secs()), response_rx)
                .await
                .context("gateway command ack timed out")?
                .context("gateway command response dropped");
        }
        drop(client_lifecycle);
        match grace_deadline {
            Some(deadline) if std::time::Instant::now() < deadline => {
                time::sleep(Duration::from_millis(500)).await;
                continue;
            }
            _ => {
                return Err(anyhow!("agent_not_online:{}", dispatch.client_id));
            }
        }
    }
}

async fn rollback_failed_command_enqueue(
    state: &GatewayState,
    enqueue_key: (String, uuid::Uuid),
    inserted_marker: GatewayCommandEnqueueMarker,
    prior_marker: Option<GatewayCommandEnqueueMarker>,
) {
    let mut command_enqueues = state.command_enqueues.write().await;
    if command_enqueues.get(&enqueue_key) != Some(&inserted_marker) {
        // A same-key dispatch linearized after this attempt. Its marker owns
        // the registry entry and must survive this failed attempt's rollback.
        return;
    }
    if let Some(prior_marker) = prior_marker {
        command_enqueues.insert(enqueue_key, prior_marker);
    } else {
        command_enqueues.remove(&enqueue_key);
    }
}

async fn prepare_gateway_client_suspension_fence(
    state: &GatewayState,
    prepare: GatewayClientSuspensionFencePrepare,
) -> GatewayClientSuspensionFenceResult {
    let lifecycle_owner = state.client_lifecycle_owner(&prepare.client_id).await;
    let _client_lifecycle = lifecycle_owner.write().await;
    let now = Instant::now();
    let mut fences = state.client_suspension_fences.write().await;
    if let Some(mut existing) = fences.get(&prepare.client_id).copied() {
        if existing.active_at(now) {
            let same_token = existing.token == prepare.token;
            if same_token && existing.expires_at.is_some() {
                existing.expires_at =
                    Some(now + Duration::from_secs(prepare.lease_secs.clamp(1, 300)));
                fences.insert(prepare.client_id.clone(), existing);
            }
            drop(fences);
            let enqueued_job_ids = if same_token {
                protected_enqueued_job_ids(state, &prepare.client_id, now).await
            } else {
                Vec::new()
            };
            return GatewayClientSuspensionFenceResult {
                client_id: prepare.client_id,
                accepted: same_token,
                fenced: true,
                message: if same_token {
                    "suspension_fence_already_prepared".to_string()
                } else {
                    "suspension_fence_conflict".to_string()
                },
                enqueued_job_ids,
            };
        }
        fences.remove(&prepare.client_id);
    }
    let lease_secs = prepare.lease_secs.clamp(1, 300);
    fences.insert(
        prepare.client_id.clone(),
        GatewayClientSuspensionFence {
            token: prepare.token,
            expires_at: Some(now + Duration::from_secs(lease_secs)),
        },
    );
    drop(fences);

    let session = state.sessions.write().await.remove(&prepare.client_id);
    if let Some(session) = session {
        let _ = session
            .close_tx
            .send(Some(GatewaySessionCloseRequest::Graceful(
                "agent_suspended".to_string(),
            )));
    }
    let enqueued_job_ids =
        protected_enqueued_job_ids(state, &prepare.client_id, Instant::now()).await;
    GatewayClientSuspensionFenceResult {
        client_id: prepare.client_id,
        accepted: true,
        fenced: true,
        message: "suspension_fence_prepared".to_string(),
        enqueued_job_ids,
    }
}

async fn protected_enqueued_job_ids(
    state: &GatewayState,
    client_id: &str,
    now: Instant,
) -> Vec<uuid::Uuid> {
    let mut command_enqueues = state.command_enqueues.write().await;
    command_enqueues.retain(|_, marker| marker.expires_at > now);
    let mut job_ids = command_enqueues
        .iter()
        .filter_map(|((enqueued_client_id, job_id), _)| {
            (enqueued_client_id == client_id).then_some(*job_id)
        })
        .collect::<Vec<_>>();
    job_ids.sort();
    job_ids.dedup();
    job_ids
}

async fn promote_gateway_client_suspension_fence(
    state: &GatewayState,
    promote: GatewayClientSuspensionFencePromote,
) -> GatewayClientSuspensionFenceResult {
    let lifecycle_owner = state.client_lifecycle_owner(&promote.client_id).await;
    let _client_lifecycle = lifecycle_owner.write().await;
    let now = Instant::now();
    let mut fences = state.client_suspension_fences.write().await;
    let accepted = fences
        .get_mut(&promote.client_id)
        .filter(|fence| fence.token == promote.token && fence.active_at(now))
        .map(|fence| fence.expires_at = None)
        .is_some();
    GatewayClientSuspensionFenceResult {
        client_id: promote.client_id,
        accepted,
        fenced: accepted,
        message: if accepted {
            "suspension_fence_promoted".to_string()
        } else {
            "suspension_fence_prepare_missing_or_expired".to_string()
        },
        enqueued_job_ids: Vec::new(),
    }
}

async fn clear_gateway_client_suspension_fence(
    state: &GatewayState,
    clear: GatewayClientSuspensionFenceClear,
) -> GatewayClientSuspensionFenceResult {
    let lifecycle_owner = state.client_lifecycle_owner(&clear.client_id).await;
    let _client_lifecycle = lifecycle_owner.write().await;
    let mut fences = state.client_suspension_fences.write().await;
    let removable = fences.get(&clear.client_id).is_some_and(|fence| {
        clear
            .expected_token
            .is_none_or(|token| fence.token == token && fence.expires_at.is_some())
    });
    if removable {
        fences.remove(&clear.client_id);
    }
    let fenced = fences
        .get(&clear.client_id)
        .copied()
        .is_some_and(|fence| fence.active_at(Instant::now()));
    GatewayClientSuspensionFenceResult {
        client_id: clear.client_id,
        accepted: removable || !fenced,
        fenced,
        message: if removable {
            format!("suspension_fence_cleared:{}", clear.reason)
        } else if fenced {
            "suspension_fence_clear_token_mismatch".to_string()
        } else {
            "suspension_fence_already_clear".to_string()
        },
        enqueued_job_ids: Vec::new(),
    }
}

async fn cancel_gateway_command(
    state: &GatewayState,
    cancel: GatewayCommandCancel,
) -> Result<GatewayCommandCancelResult> {
    let Some(sender) = state
        .sessions
        .read()
        .await
        .get(&cancel.client_id)
        .map(|session| session.sender.clone())
    else {
        return Ok(GatewayCommandCancelResult {
            client_id: cancel.client_id,
            job_id: cancel.request.job_id,
            acked: false,
            accepted: false,
            applied: false,
            message: "agent_not_online".to_string(),
        });
    };
    let (response_tx, response_rx) = oneshot::channel();
    sender
        .try_send(GatewaySessionMessage::Cancel(GatewayCancelCommand {
            request: cancel.request.clone(),
            response: response_tx,
        }))
        .map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => {
                anyhow!("agent_session_command_queue_full:{}", cancel.client_id)
            }
            mpsc::error::TrySendError::Closed(_) => {
                anyhow!("agent_session_closed:{}", cancel.client_id)
            }
        })?;
    time::timeout(Duration::from_secs(state.dispatch_ack_secs()), response_rx)
        .await
        .context("gateway command cancel timed out")?
        .context("gateway command cancel response dropped")
}

async fn disconnect_gateway_session(
    state: &GatewayState,
    disconnect: GatewaySessionDisconnect,
) -> Result<GatewaySessionDisconnectResult> {
    let lifecycle_owner = state.client_lifecycle_owner(&disconnect.client_id).await;
    let _client_lifecycle = lifecycle_owner.write().await;
    let session = state.sessions.write().await.remove(&disconnect.client_id);
    let Some(session) = session else {
        return Ok(GatewaySessionDisconnectResult {
            client_id: disconnect.client_id,
            accepted: true,
            disconnected: false,
            message: "agent_not_online".to_string(),
        });
    };
    let _ = session
        .close_tx
        .send(Some(GatewaySessionCloseRequest::Graceful(
            disconnect.reason,
        )));
    Ok(GatewaySessionDisconnectResult {
        client_id: disconnect.client_id,
        accepted: true,
        disconnected: true,
        message: "disconnect_requested".to_string(),
    })
}

async fn write_gateway_error<S>(stream: &mut S, error: anyhow::Error) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    let message = error.to_string();
    let status = if message.contains("agent_not_online") {
        "404 Not Found"
    } else if message.contains("agent_suspended") {
        "409 Conflict"
    } else if message.contains("agent_session_command_queue_full") {
        "503 Service Unavailable"
    } else if message.contains("timed out") {
        "504 Gateway Timeout"
    } else {
        "500 Internal Server Error"
    };
    write_http_json(stream, status, &serde_json::json!({"error": message})).await
}

async fn write_privilege_error<S>(stream: &mut S, error: anyhow::Error) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    let message = error.to_string();
    let status = if message.contains("not configured")
        || message.contains(&format!(
            "{:?}",
            PrivilegeAssertionError::ReplayProtectionSaturated
        )) {
        "503 Service Unavailable"
    } else if message.contains(&format!("{:?}", PrivilegeAssertionError::Replay)) {
        "409 Conflict"
    } else if message.contains("privilege_assertion_") {
        "403 Forbidden"
    } else {
        "500 Internal Server Error"
    };
    write_http_json(stream, status, &serde_json::json!({"error": message})).await
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

async fn read_http_request<S>(stream: &mut S) -> Result<HttpRequest>
where
    S: AsyncRead + Unpin,
{
    let mut buffer = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 1024];
        let read = time::timeout(HTTP_READ_TIMEOUT, stream.read(&mut chunk))
            .await
            .context("HTTP header read timed out")??;
        if read == 0 {
            return Err(anyhow!("connection closed before HTTP headers"));
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = find_header_end(&buffer) {
            break position;
        }
        if buffer.len() > 64 * 1024 {
            return Err(anyhow!("HTTP headers too large"));
        }
    };
    let header_bytes = &buffer[..header_end];
    let headers_text = std::str::from_utf8(header_bytes).context("HTTP headers are not UTF-8")?;
    let mut lines = headers_text.split("\r\n");
    let request_line = lines.next().context("missing request line")?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap_or_default().to_string();
    let path = request_parts.next().unwrap_or_default().to_string();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect::<Vec<_>>();
    let content_length = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    if content_length > 16 * 1024 * 1024 {
        return Err(anyhow!("HTTP body too large"));
    }
    let body_start = header_end + 4;
    let mut body = buffer[body_start..].to_vec();
    while body.len() < content_length {
        let mut chunk = vec![0_u8; content_length - body.len()];
        let read = time::timeout(HTTP_READ_TIMEOUT, stream.read(&mut chunk))
            .await
            .context("HTTP body read timed out")??;
        if read == 0 {
            return Err(anyhow!("connection closed before HTTP body"));
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);
    Ok(HttpRequest {
        method,
        path,
        headers,
        body,
    })
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn authorized_internal_request(headers: &[(String, String)], internal_token: Option<&str>) -> bool {
    let Some(expected) = internal_token else {
        return false;
    };
    headers
        .iter()
        .find(|(name, _)| name == "authorization")
        .and_then(|(_, value)| value.strip_prefix("Bearer "))
        .is_some_and(|provided| constant_time_eq(provided.as_bytes(), expected.as_bytes()))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0_u8;
    for (left, right) in left.iter().zip(right.iter()) {
        diff |= left ^ right;
    }
    diff == 0
}

async fn write_http_json<S, T>(stream: &mut S, status: &str, value: &T) -> Result<()>
where
    S: AsyncWrite + Unpin,
    T: serde::Serialize,
{
    let body = serde_json::to_vec(value)?;
    let response = format!(
        "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.write_all(&body).await?;
    Ok(())
}

#[cfg(test)]
#[path = "tests_control.rs"]
mod tests;
