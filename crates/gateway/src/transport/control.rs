use std::{
    collections::{HashMap, HashSet},
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
    verify_privilege_assertion, GatewayClientDispatchFenceAcquire,
    GatewayClientDispatchFenceAcquireResult, GatewayClientDispatchFenceBatchResult,
    GatewayClientDispatchFenceClear, GatewayClientDispatchFenceClearBatchRequest,
    GatewayClientDispatchFencePrepare, GatewayClientDispatchFencePrepareBatchRequest,
    GatewayClientDispatchFencePromote, GatewayClientDispatchFencePromoteBatchRequest,
    GatewayClientDispatchFencePurpose, GatewayClientDispatchFenceResult, GatewayCommandCancel,
    GatewayCommandCancelResult, GatewayCommandDispatch, GatewayCommandDispatchResult,
    GatewayPrivilegeVerification, GatewayPrivilegeVerificationBatchItemResult,
    GatewayPrivilegeVerificationBatchRequest, GatewayPrivilegeVerificationBatchResult,
    GatewayPrivilegeVerificationResult, GatewaySessionDisconnect,
    GatewaySessionDisconnectBatchRequest, GatewaySessionDisconnectBatchResult,
    GatewaySessionDisconnectResult, GatewayTerminalControl, GatewayTerminalControlResult,
    PrivilegeAssertionError, TerminalControlAction, GATEWAY_CLIENT_DISPATCH_FENCE_BATCH_MAX_ITEMS,
    GATEWAY_CONTROL_BATCH_MAX_ITEMS,
};

use crate::{
    state::{
        GatewayCancelCommand, GatewayClientDispatchFence, GatewayClientDispatchFenceFallback,
        GatewayClientDispatchFenceState, GatewayCommand, GatewayCommandEnqueueMarker,
        GatewaySessionCloseRequest, GatewaySessionMessage, GatewayState,
        GatewayTerminalControlCommand,
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
                | "/internal/v1/gateway/session/disconnect/batch"
                | "/internal/v1/gateway/client/dispatch-fence/acquire"
                | "/internal/v1/gateway/client/dispatch-fence/batch/prepare"
                | "/internal/v1/gateway/client/dispatch-fence/batch/promote"
                | "/internal/v1/gateway/client/dispatch-fence/batch/clear"
                | "/internal/v1/gateway/terminal/control"
                | "/internal/v1/gateway/metrics"
                | "/internal/v1/gateway/privilege/verify"
                | "/internal/v1/gateway/privilege/verify/batch"
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
    } else if request.path == "/internal/v1/gateway/session/disconnect/batch" {
        let batch: GatewaySessionDisconnectBatchRequest = match serde_json::from_slice(
            &request.body,
        ) {
            Ok(batch) => batch,
            Err(error) => {
                write_http_json(
                        &mut stream,
                        "400 Bad Request",
                        &serde_json::json!({"error": format!("invalid_session_disconnect_batch:{error}")}),
                    )
                    .await?;
                return Ok(());
            }
        };
        match disconnect_gateway_sessions(&state, batch).await {
            Ok(result) => write_http_json(&mut stream, "200 OK", &result).await?,
            Err(error) => {
                write_http_json(
                    &mut stream,
                    "400 Bad Request",
                    &serde_json::json!({"error": error}),
                )
                .await?
            }
        }
    } else if request.path == "/internal/v1/gateway/client/dispatch-fence/acquire" {
        let acquire: GatewayClientDispatchFenceAcquire = match serde_json::from_slice(&request.body)
        {
            Ok(acquire) => acquire,
            Err(error) => {
                write_http_json(
                    &mut stream,
                    "400 Bad Request",
                    &serde_json::json!({"error": format!("invalid_dispatch_fence_acquire:{error}")}),
                )
                .await?;
                return Ok(());
            }
        };
        match acquire_gateway_client_dispatch_fence(&state, acquire).await {
            Ok(result) => write_http_json(&mut stream, "200 OK", &result).await?,
            Err(error) => write_gateway_error(&mut stream, error).await?,
        }
    } else if request.path == "/internal/v1/gateway/client/dispatch-fence/batch/prepare" {
        let batch: GatewayClientDispatchFencePrepareBatchRequest = match serde_json::from_slice(
            &request.body,
        ) {
            Ok(batch) => batch,
            Err(error) => {
                write_http_json(
                        &mut stream,
                        "400 Bad Request",
                        &serde_json::json!({"error": format!("invalid_dispatch_fence_prepare_batch:{error}")}),
                    )
                    .await?;
                return Ok(());
            }
        };
        match prepare_gateway_client_dispatch_fence_batch(&state, batch).await {
            Ok(result) => write_http_json(&mut stream, "200 OK", &result).await?,
            Err(error) => {
                write_http_json(
                    &mut stream,
                    "400 Bad Request",
                    &serde_json::json!({"error": error}),
                )
                .await?
            }
        }
    } else if request.path == "/internal/v1/gateway/client/dispatch-fence/batch/promote" {
        let batch: GatewayClientDispatchFencePromoteBatchRequest = match serde_json::from_slice(
            &request.body,
        ) {
            Ok(batch) => batch,
            Err(error) => {
                write_http_json(
                        &mut stream,
                        "400 Bad Request",
                        &serde_json::json!({"error": format!("invalid_dispatch_fence_promote_batch:{error}")}),
                    )
                    .await?;
                return Ok(());
            }
        };
        match promote_gateway_client_dispatch_fence_batch(&state, batch).await {
            Ok(result) => write_http_json(&mut stream, "200 OK", &result).await?,
            Err(error) => {
                write_http_json(
                    &mut stream,
                    "400 Bad Request",
                    &serde_json::json!({"error": error}),
                )
                .await?
            }
        }
    } else if request.path == "/internal/v1/gateway/client/dispatch-fence/batch/clear" {
        let batch: GatewayClientDispatchFenceClearBatchRequest = match serde_json::from_slice(
            &request.body,
        ) {
            Ok(batch) => batch,
            Err(error) => {
                write_http_json(
                        &mut stream,
                        "400 Bad Request",
                        &serde_json::json!({"error": format!("invalid_dispatch_fence_clear_batch:{error}")}),
                    )
                    .await?;
                return Ok(());
            }
        };
        match clear_gateway_client_dispatch_fence_batch(&state, batch).await {
            Ok(result) => write_http_json(&mut stream, "200 OK", &result).await?,
            Err(error) => {
                write_http_json(
                    &mut stream,
                    "400 Bad Request",
                    &serde_json::json!({"error": error}),
                )
                .await?
            }
        }
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
    } else if request.path == "/internal/v1/gateway/privilege/verify/batch" {
        let batch: GatewayPrivilegeVerificationBatchRequest = match serde_json::from_slice(
            &request.body,
        ) {
            Ok(batch) => batch,
            Err(error) => {
                write_http_json(
                        &mut stream,
                        "400 Bad Request",
                        &serde_json::json!({"error": format!("invalid_privilege_verification_batch:{error}")}),
                    )
                    .await?;
                return Ok(());
            }
        };
        match verify_gateway_privileges(&state, privilege_verifier_key_hex.as_deref(), batch).await
        {
            Ok(result) => write_http_json(&mut stream, "200 OK", &result).await?,
            Err(error) => write_privilege_error(&mut stream, error).await?,
        }
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
    let constructive = matches!(
        &control.request.action,
        TerminalControlAction::Input { .. } | TerminalControlAction::Resize { .. }
    );
    let lifecycle_owner = if constructive {
        Some(state.client_lifecycle_owner(&control.client_id).await)
    } else {
        None
    };
    let client_lifecycle = match lifecycle_owner.as_ref() {
        Some(owner) => Some(owner.read().await),
        None => None,
    };
    if constructive {
        let deletion_fenced = {
            let now = Instant::now();
            let mut fences = state.client_dispatch_fences.write().await;
            normalize_expired_dispatch_fence(&mut fences, &control.client_id, now);
            fences.get(&control.client_id).is_some_and(|fence| {
                fence.purpose == GatewayClientDispatchFencePurpose::Deletion && fence.active_at(now)
            })
        };
        if deletion_fenced {
            return Err(anyhow!("agent_lifecycle_fenced:{}", control.client_id));
        }
    }
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
    // Deletion preparation only needs to order against enqueue; the agent ACK
    // may take the normal control timeout without retaining lifecycle owner.
    drop(client_lifecycle);
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

async fn verify_gateway_privileges(
    state: &GatewayState,
    verifier_key_hex: Option<&str>,
    batch: GatewayPrivilegeVerificationBatchRequest,
) -> Result<GatewayPrivilegeVerificationBatchResult> {
    validate_gateway_control_batch(
        batch.items.len(),
        batch.items.iter().map(|item| item.request_id.as_str()),
        "privilege_verification_batch",
    )
    .map_err(anyhow::Error::msg)?;
    let verifier_key = decode_verifier_key(verifier_key_hex)?;
    let now_unix = unix_now();
    let mut replay_cache = state.privilege_assertions.lock().await;
    let mut results = Vec::with_capacity(batch.items.len());
    for item in batch.items {
        match verify_privilege_assertion(
            &verifier_key,
            &item.verification.intent,
            &item.verification.assertion,
            now_unix,
            &mut replay_cache,
        ) {
            Ok(intent_hash_hex) => {
                results.push(GatewayPrivilegeVerificationBatchItemResult {
                    request_id: item.request_id,
                    approved: true,
                    intent_hash_hex: Some(intent_hash_hex),
                    message: "approved".to_string(),
                    error_code: None,
                });
            }
            Err(PrivilegeAssertionError::ReplayProtectionSaturated) => {
                return Err(anyhow!(
                    "privilege_assertion_{:?}",
                    PrivilegeAssertionError::ReplayProtectionSaturated
                ));
            }
            Err(error) => results.push(GatewayPrivilegeVerificationBatchItemResult {
                request_id: item.request_id,
                approved: false,
                intent_hash_hex: None,
                message: "privilege verification failed".to_string(),
                error_code: Some(format!("privilege_assertion_{error:?}")),
            }),
        }
    }
    Ok(GatewayPrivilegeVerificationBatchResult { results })
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
        if dispatch.expected_gateway_epoch != Some(state.client_dispatch_fence_epoch) {
            return Err(anyhow!(
                "agent_gateway_epoch_recheck_required:{}",
                state.client_dispatch_fence_epoch
            ));
        }
        let current_fence = state
            .client_dispatch_fences
            .read()
            .await
            .get(&dispatch.client_id)
            .cloned();
        if current_fence.is_some_and(|fence| !fence.active_at(Instant::now())) {
            let mut fences = state.client_dispatch_fences.write().await;
            normalize_expired_dispatch_fence(&mut fences, &dispatch.client_id, Instant::now());
            drop(fences);
            drop(client_lifecycle);
            continue;
        }
        match current_fence {
            Some(fence) if fence.requires_durable_recheck() => {
                if dispatch.lifecycle_recheck.as_ref() != Some(&fence.owner()) {
                    return Err(durable_lifecycle_recheck_required(fence.owner()));
                }
            }
            Some(_) => return Err(anyhow!("agent_suspended:{}", dispatch.client_id)),
            None if dispatch.lifecycle_recheck.is_some() => {
                return Err(anyhow!(
                    "agent_lifecycle_recheck_stale:{}",
                    dispatch.client_id
                ));
            }
            None => {}
        }
        // A durable-recheck barrier deliberately remains installed. Every
        // dispatch that may have crossed the lease gap independently proves
        // DB eligibility; a second delayed request cannot piggyback on the
        // first request's proof.
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
                .entry(enqueue_key.0.clone())
                .or_default()
                .insert(enqueue_key.1, enqueue_marker);
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

fn durable_lifecycle_recheck_required(
    owner: vpsman_common::GatewayClientDispatchFenceOwner,
) -> anyhow::Error {
    anyhow!(
        "agent_lifecycle_recheck_required:{}:{}:{}",
        owner.gateway_epoch,
        owner.generation,
        owner.token
    )
}

async fn rollback_failed_command_enqueue(
    state: &GatewayState,
    enqueue_key: (String, uuid::Uuid),
    inserted_marker: GatewayCommandEnqueueMarker,
    prior_marker: Option<GatewayCommandEnqueueMarker>,
) {
    let (client_id, job_id) = enqueue_key;
    let mut command_enqueues = state.command_enqueues.write().await;
    if command_enqueues
        .get(&client_id)
        .and_then(|client_enqueues| client_enqueues.get(&job_id))
        != Some(&inserted_marker)
    {
        // A same-key dispatch linearized after this attempt. Its marker owns
        // the registry entry and must survive this failed attempt's rollback.
        return;
    }
    if let Some(prior_marker) = prior_marker {
        command_enqueues
            .entry(client_id)
            .or_default()
            .insert(job_id, prior_marker);
    } else {
        let remove_client = command_enqueues
            .get_mut(&client_id)
            .is_some_and(|client_enqueues| {
                client_enqueues.remove(&job_id);
                client_enqueues.is_empty()
            });
        if remove_client {
            command_enqueues.remove(&client_id);
        }
    }
}

fn validate_dispatch_fence_batch<'a>(
    item_count: usize,
    client_ids: impl IntoIterator<Item = &'a str>,
) -> std::result::Result<(), String> {
    if !(1..=GATEWAY_CLIENT_DISPATCH_FENCE_BATCH_MAX_ITEMS).contains(&item_count) {
        return Err(format!(
            "dispatch_fence_batch_size_out_of_range:expected=1..={}:actual={item_count}",
            GATEWAY_CLIENT_DISPATCH_FENCE_BATCH_MAX_ITEMS
        ));
    }
    let mut unique_client_ids = HashSet::with_capacity(item_count);
    for client_id in client_ids {
        if !unique_client_ids.insert(client_id) {
            return Err(format!(
                "dispatch_fence_batch_duplicate_client_id:{client_id}"
            ));
        }
    }
    Ok(())
}

fn validate_gateway_control_batch<'a>(
    item_count: usize,
    request_ids: impl IntoIterator<Item = &'a str>,
    error_prefix: &str,
) -> std::result::Result<(), String> {
    if !(1..=GATEWAY_CONTROL_BATCH_MAX_ITEMS).contains(&item_count) {
        return Err(format!(
            "{error_prefix}_size_out_of_range:expected=1..={}:actual={item_count}",
            GATEWAY_CONTROL_BATCH_MAX_ITEMS
        ));
    }
    let mut unique_request_ids = HashSet::with_capacity(item_count);
    for request_id in request_ids {
        if request_id.is_empty() {
            return Err(format!("{error_prefix}_empty_request_id"));
        }
        if !unique_request_ids.insert(request_id) {
            return Err(format!("{error_prefix}_duplicate_request_id:{request_id}"));
        }
    }
    Ok(())
}

async fn prepare_gateway_client_dispatch_fence_batch(
    state: &GatewayState,
    batch: GatewayClientDispatchFencePrepareBatchRequest,
) -> std::result::Result<GatewayClientDispatchFenceBatchResult, String> {
    validate_dispatch_fence_batch(
        batch.items.len(),
        batch.items.iter().map(|item| item.client_id.as_str()),
    )?;
    let mut results = Vec::with_capacity(batch.items.len());
    for prepare in batch.items {
        results.push(prepare_gateway_client_dispatch_fence(state, prepare).await);
    }
    Ok(GatewayClientDispatchFenceBatchResult { results })
}

async fn promote_gateway_client_dispatch_fence_batch(
    state: &GatewayState,
    batch: GatewayClientDispatchFencePromoteBatchRequest,
) -> std::result::Result<GatewayClientDispatchFenceBatchResult, String> {
    validate_dispatch_fence_batch(
        batch.items.len(),
        batch.items.iter().map(|item| item.client_id.as_str()),
    )?;
    let mut results = Vec::with_capacity(batch.items.len());
    for promote in batch.items {
        results.push(promote_gateway_client_dispatch_fence(state, promote).await);
    }
    Ok(GatewayClientDispatchFenceBatchResult { results })
}

async fn clear_gateway_client_dispatch_fence_batch(
    state: &GatewayState,
    batch: GatewayClientDispatchFenceClearBatchRequest,
) -> std::result::Result<GatewayClientDispatchFenceBatchResult, String> {
    validate_dispatch_fence_batch(
        batch.items.len(),
        batch.items.iter().map(|item| item.client_id.as_str()),
    )?;
    let mut results = Vec::with_capacity(batch.items.len());
    for clear in batch.items {
        results.push(clear_gateway_client_dispatch_fence(state, clear).await);
    }
    Ok(GatewayClientDispatchFenceBatchResult { results })
}

async fn acquire_gateway_client_dispatch_fence(
    state: &GatewayState,
    acquire: GatewayClientDispatchFenceAcquire,
) -> Result<GatewayClientDispatchFenceAcquireResult> {
    let lifecycle_owner = state.client_lifecycle_owner(&acquire.client_id).await;
    let _client_lifecycle = lifecycle_owner.write().await;
    {
        let mut fences = state.client_dispatch_fences.write().await;
        normalize_expired_dispatch_fence(&mut fences, &acquire.client_id, Instant::now());
    }
    if let Some(existing) = state
        .client_dispatch_fences
        .read()
        .await
        .get(&acquire.client_id)
        .cloned()
    {
        if existing.token == acquire.token {
            anyhow::ensure!(
                existing.purpose == acquire.purpose,
                "dispatch_fence_token_purpose_conflict"
            );
            return Ok(GatewayClientDispatchFenceAcquireResult {
                client_id: acquire.client_id,
                owner: existing.owner(),
            });
        }
        let replaceable = existing.requires_durable_recheck()
            || (matches!(existing.state, GatewayClientDispatchFenceState::Persistent)
                && (existing.purpose == GatewayClientDispatchFencePurpose::Suspension
                    || (existing.purpose == GatewayClientDispatchFencePurpose::Deletion
                        && acquire.purpose == GatewayClientDispatchFencePurpose::Deletion)));
        anyhow::ensure!(replaceable, "dispatch_fence_conflict");
    }
    let mut generations = state.client_dispatch_fence_generations.write().await;
    let generation = generations.entry(acquire.client_id.clone()).or_default();
    if generation.latest_token == Some(acquire.token) {
        anyhow::ensure!(
            generation.latest_purpose == Some(acquire.purpose),
            "dispatch_fence_token_purpose_conflict"
        );
    } else {
        generation.latest_generation = generation
            .latest_generation
            .checked_add(1)
            .context("client dispatch-fence generation exhausted")?;
        generation.latest_token = Some(acquire.token);
        generation.latest_purpose = Some(acquire.purpose);
    }
    Ok(GatewayClientDispatchFenceAcquireResult {
        client_id: acquire.client_id,
        owner: vpsman_common::GatewayClientDispatchFenceOwner {
            token: acquire.token,
            gateway_epoch: state.client_dispatch_fence_epoch,
            generation: generation.latest_generation,
        },
    })
}

pub(crate) fn normalize_expired_dispatch_fence(
    fences: &mut HashMap<String, GatewayClientDispatchFence>,
    client_id: &str,
    now: Instant,
) {
    let Some(fence) = fences.get(client_id).cloned() else {
        return;
    };
    if !fence.lease_expired_at(now) {
        return;
    }
    // Expiry loses continuous gateway ownership; it never decides whether a
    // replaced durable suspension is still authoritative. Keep the complete
    // exact transition (including its fallback) as a cheap durable-recheck
    // barrier until DB state lets its owner finalize or compensate it.
    let fallback = fence.fallback();
    fences.insert(
        client_id.to_string(),
        GatewayClientDispatchFence {
            state: GatewayClientDispatchFenceState::DurableRecheck { fallback },
            ..fence
        },
    );
}

async fn prepare_gateway_client_dispatch_fence(
    state: &GatewayState,
    prepare: GatewayClientDispatchFencePrepare,
) -> GatewayClientDispatchFenceResult {
    let lifecycle_owner = state.client_lifecycle_owner(&prepare.client_id).await;
    let _client_lifecycle = lifecycle_owner.write().await;
    let now = Instant::now();
    if prepare.gateway_epoch != state.client_dispatch_fence_epoch {
        return GatewayClientDispatchFenceResult {
            client_id: prepare.client_id,
            accepted: false,
            fenced: false,
            ownership_continuous: false,
            message: "dispatch_fence_gateway_epoch_stale".to_string(),
            enqueued_job_ids: Vec::new(),
        };
    }
    let generation = state
        .client_dispatch_fence_generations
        .read()
        .await
        .get(&prepare.client_id)
        .copied()
        .unwrap_or_default();
    let mut fences = state.client_dispatch_fences.write().await;
    normalize_expired_dispatch_fence(&mut fences, &prepare.client_id, now);
    if let Some(mut existing) = fences.get(&prepare.client_id).cloned() {
        let same_owner = existing.token == prepare.token
            && existing.gateway_epoch == prepare.gateway_epoch
            && existing.generation == prepare.generation
            && existing.purpose == prepare.purpose;
        let replaces_recoverable_fence = !same_owner
            && (existing.requires_durable_recheck()
                || (matches!(existing.state, GatewayClientDispatchFenceState::Persistent)
                    && (existing.purpose == GatewayClientDispatchFencePurpose::Suspension
                        || (existing.purpose == GatewayClientDispatchFencePurpose::Deletion
                            && prepare.purpose == GatewayClientDispatchFencePurpose::Deletion))));
        if same_owner {
            if prepare.generation <= generation.retired_generation {
                return GatewayClientDispatchFenceResult {
                    client_id: prepare.client_id,
                    accepted: false,
                    fenced: true,
                    ownership_continuous: false,
                    message: "dispatch_fence_generation_retired".to_string(),
                    enqueued_job_ids: Vec::new(),
                };
            }
            if matches!(
                existing.state,
                GatewayClientDispatchFenceState::Prepared { .. }
            ) {
                existing.state = GatewayClientDispatchFenceState::Prepared {
                    expires_at: now + Duration::from_secs(prepare.lease_secs.clamp(1, 300)),
                    fallback: existing.fallback(),
                };
                fences.insert(prepare.client_id.clone(), existing);
            }
            drop(fences);
            let enqueued_job_ids = protected_enqueued_job_ids(state, &prepare.client_id, now).await;
            return GatewayClientDispatchFenceResult {
                client_id: prepare.client_id,
                accepted: true,
                fenced: true,
                ownership_continuous: !existing.requires_durable_recheck(),
                message: if existing.requires_durable_recheck() {
                    "dispatch_fence_requires_durable_recheck".to_string()
                } else {
                    "dispatch_fence_already_owned".to_string()
                },
                enqueued_job_ids,
            };
        }
        if prepare.renewal {
            return GatewayClientDispatchFenceResult {
                client_id: prepare.client_id,
                accepted: false,
                fenced: true,
                ownership_continuous: false,
                message: if existing.fallback().is_some_and(|replaced| {
                    replaced.token == prepare.token
                        && replaced.gateway_epoch == prepare.gateway_epoch
                        && replaced.generation == prepare.generation
                        && replaced.purpose == prepare.purpose
                }) {
                    "dispatch_fence_owner_is_fallback".to_string()
                } else {
                    "dispatch_fence_conflict".to_string()
                },
                enqueued_job_ids: Vec::new(),
            };
        }
        let initial_authorized = generation.latest_generation == prepare.generation
            && generation.latest_token == Some(prepare.token)
            && prepare.generation > generation.retired_generation;
        if !initial_authorized {
            return GatewayClientDispatchFenceResult {
                client_id: prepare.client_id,
                accepted: false,
                fenced: true,
                ownership_continuous: false,
                message: "dispatch_fence_generation_stale".to_string(),
                enqueued_job_ids: Vec::new(),
            };
        }
        if replaces_recoverable_fence {
            let replaced_generation = existing.generation;
            fences.insert(
                prepare.client_id.clone(),
                GatewayClientDispatchFence {
                    token: prepare.token,
                    gateway_epoch: prepare.gateway_epoch,
                    generation: prepare.generation,
                    purpose: prepare.purpose,
                    state: GatewayClientDispatchFenceState::Prepared {
                        expires_at: now + Duration::from_secs(prepare.lease_secs.clamp(1, 300)),
                        fallback: Some(GatewayClientDispatchFenceFallback {
                            token: existing.token,
                            gateway_epoch: existing.gateway_epoch,
                            generation: existing.generation,
                            purpose: existing.purpose,
                            requires_durable_recheck: existing.requires_durable_recheck(),
                        }),
                    },
                },
            );
            drop(fences);
            state
                .retire_client_dispatch_fence_generation(
                    &prepare.client_id,
                    prepare.gateway_epoch,
                    replaced_generation,
                )
                .await;
            let enqueued_job_ids = protected_enqueued_job_ids(state, &prepare.client_id, now).await;
            return GatewayClientDispatchFenceResult {
                client_id: prepare.client_id,
                accepted: true,
                fenced: true,
                ownership_continuous: false,
                message: "dispatch_fence_replaced_recoverable_owner".to_string(),
                enqueued_job_ids,
            };
        }
        return GatewayClientDispatchFenceResult {
            client_id: prepare.client_id,
            accepted: false,
            fenced: true,
            ownership_continuous: false,
            message: "dispatch_fence_conflict".to_string(),
            enqueued_job_ids: Vec::new(),
        };
    }
    let initial_authorized = !prepare.renewal
        && generation.latest_generation == prepare.generation
        && generation.latest_token == Some(prepare.token)
        && prepare.generation > generation.retired_generation;
    if !initial_authorized {
        return GatewayClientDispatchFenceResult {
            client_id: prepare.client_id,
            accepted: false,
            fenced: false,
            ownership_continuous: false,
            message: "dispatch_fence_generation_stale".to_string(),
            enqueued_job_ids: Vec::new(),
        };
    }
    let lease_secs = prepare.lease_secs.clamp(1, 300);
    fences.insert(
        prepare.client_id.clone(),
        GatewayClientDispatchFence {
            token: prepare.token,
            gateway_epoch: prepare.gateway_epoch,
            generation: prepare.generation,
            purpose: prepare.purpose,
            state: GatewayClientDispatchFenceState::Prepared {
                expires_at: now + Duration::from_secs(lease_secs),
                fallback: None,
            },
        },
    );
    drop(fences);

    let enqueued_job_ids =
        protected_enqueued_job_ids(state, &prepare.client_id, Instant::now()).await;
    GatewayClientDispatchFenceResult {
        client_id: prepare.client_id,
        accepted: true,
        fenced: true,
        ownership_continuous: false,
        message: "dispatch_fence_prepared".to_string(),
        enqueued_job_ids,
    }
}

async fn protected_enqueued_job_ids(
    state: &GatewayState,
    client_id: &str,
    now: Instant,
) -> Vec<uuid::Uuid> {
    let mut command_enqueues = state.command_enqueues.write().await;
    let Some(client_enqueues) = command_enqueues.get_mut(client_id) else {
        return Vec::new();
    };
    client_enqueues.retain(|_, marker| marker.expires_at > now);
    let mut job_ids = client_enqueues.keys().copied().collect::<Vec<_>>();
    let remove_client = job_ids.is_empty();
    if remove_client {
        command_enqueues.remove(client_id);
    }
    job_ids.sort();
    job_ids
}

async fn promote_gateway_client_dispatch_fence(
    state: &GatewayState,
    promote: GatewayClientDispatchFencePromote,
) -> GatewayClientDispatchFenceResult {
    let lifecycle_owner = state.client_lifecycle_owner(&promote.client_id).await;
    let _client_lifecycle = lifecycle_owner.write().await;
    let now = Instant::now();
    if promote.gateway_epoch != state.client_dispatch_fence_epoch {
        let fenced = state
            .client_dispatch_fences
            .read()
            .await
            .get(&promote.client_id)
            .cloned()
            .is_some_and(|fence| fence.active_at(now));
        return GatewayClientDispatchFenceResult {
            client_id: promote.client_id,
            accepted: false,
            fenced,
            ownership_continuous: false,
            message: "dispatch_fence_gateway_epoch_stale".to_string(),
            enqueued_job_ids: Vec::new(),
        };
    }
    let retired_generation = state
        .client_dispatch_fence_generations
        .read()
        .await
        .get(&promote.client_id)
        .map(|generation| generation.retired_generation)
        .unwrap_or_default();
    if promote.generation <= retired_generation {
        let fenced = state
            .client_dispatch_fences
            .read()
            .await
            .get(&promote.client_id)
            .is_some_and(|fence| fence.active_at(now));
        return GatewayClientDispatchFenceResult {
            client_id: promote.client_id,
            accepted: false,
            fenced,
            ownership_continuous: false,
            message: "dispatch_fence_generation_retired".to_string(),
            enqueued_job_ids: Vec::new(),
        };
    }
    let mut fences = state.client_dispatch_fences.write().await;
    let exact_before_normalize = fences.get(&promote.client_id).is_some_and(|fence| {
        fence.token == promote.token
            && fence.gateway_epoch == promote.gateway_epoch
            && fence.generation == promote.generation
            && fence.purpose == promote.purpose
    });
    if !exact_before_normalize {
        normalize_expired_dispatch_fence(&mut fences, &promote.client_id, now);
    }
    let mut ownership_continuous = false;
    let directly_owned = fences
        .get_mut(&promote.client_id)
        .filter(|fence| {
            fence.token == promote.token
                && fence.gateway_epoch == promote.gateway_epoch
                && fence.generation == promote.generation
                && fence.purpose == promote.purpose
        })
        .map(|fence| {
            ownership_continuous = fence.active_at(now) && !fence.requires_durable_recheck();
            fence.state = GatewayClientDispatchFenceState::Persistent;
        })
        .is_some();
    let owned_as_fallback = !directly_owned
        && fences.get(&promote.client_id).is_some_and(|fence| {
            fence.fallback().is_some_and(|replaced| {
                replaced.token == promote.token
                    && replaced.gateway_epoch == promote.gateway_epoch
                    && replaced.generation == promote.generation
                    && replaced.purpose == promote.purpose
            })
        });
    let fenced = fences
        .get(&promote.client_id)
        .cloned()
        .is_some_and(|fence| fence.active_at(now));
    drop(fences);
    let enqueued_job_ids = if directly_owned {
        protected_enqueued_job_ids(state, &promote.client_id, now).await
    } else {
        Vec::new()
    };
    GatewayClientDispatchFenceResult {
        client_id: promote.client_id,
        accepted: directly_owned,
        fenced,
        ownership_continuous,
        message: if directly_owned {
            "dispatch_fence_promoted".to_string()
        } else if owned_as_fallback {
            "dispatch_fence_owner_is_fallback".to_string()
        } else {
            "dispatch_fence_owner_superseded_or_missing".to_string()
        },
        enqueued_job_ids,
    }
}

async fn clear_gateway_client_dispatch_fence(
    state: &GatewayState,
    clear: GatewayClientDispatchFenceClear,
) -> GatewayClientDispatchFenceResult {
    let lifecycle_owner = state.client_lifecycle_owner(&clear.client_id).await;
    let _client_lifecycle = lifecycle_owner.write().await;
    let now = Instant::now();
    if clear.gateway_epoch != state.client_dispatch_fence_epoch {
        let fenced = state
            .client_dispatch_fences
            .read()
            .await
            .get(&clear.client_id)
            .cloned()
            .is_some_and(|fence| fence.active_at(now));
        return GatewayClientDispatchFenceResult {
            client_id: clear.client_id,
            accepted: false,
            fenced,
            ownership_continuous: false,
            message: "dispatch_fence_gateway_epoch_stale".to_string(),
            enqueued_job_ids: Vec::new(),
        };
    }
    let retired_before = state
        .client_dispatch_fence_generations
        .read()
        .await
        .get(&clear.client_id)
        .map(|generation| generation.retired_generation)
        .unwrap_or_default();
    let mut fences = state.client_dispatch_fences.write().await;
    let directly_owned = fences.get(&clear.client_id).is_some_and(|fence| {
        fence.token == clear.expected_token
            && fence.gateway_epoch == clear.gateway_epoch
            && fence.generation == clear.expected_generation
    });
    let owned_as_fallback = !directly_owned
        && fences.get(&clear.client_id).is_some_and(|fence| {
            fence.fallback().is_some_and(|replaced| {
                replaced.token == clear.expected_token
                    && replaced.gateway_epoch == clear.gateway_epoch
                    && replaced.generation == clear.expected_generation
            })
        });
    let restored = directly_owned
        && clear.restore_fallback
        && fences
            .get(&clear.client_id)
            .is_some_and(|fence| fence.fallback().is_some());
    let retired_owner_visible =
        clear.expected_generation <= retired_before && (directly_owned || owned_as_fallback);
    if directly_owned && !retired_owner_visible {
        let current = fences[&clear.client_id];
        if clear.restore_fallback {
            if let Some(replaced) = current.fallback() {
                fences.insert(
                    clear.client_id.clone(),
                    GatewayClientDispatchFence {
                        token: replaced.token,
                        gateway_epoch: replaced.gateway_epoch,
                        generation: replaced.generation,
                        purpose: replaced.purpose,
                        state: if replaced.requires_durable_recheck {
                            GatewayClientDispatchFenceState::DurableRecheck { fallback: None }
                        } else {
                            GatewayClientDispatchFenceState::Persistent
                        },
                    },
                );
            } else {
                fences.remove(&clear.client_id);
            }
        } else {
            fences.remove(&clear.client_id);
        }
    }
    let fenced = fences
        .get(&clear.client_id)
        .cloned()
        .is_some_and(|fence| fence.active_at(now));
    let should_retire = !retired_owner_visible && !owned_as_fallback;
    let accepted = !retired_owner_visible && (directly_owned || (!fenced && !owned_as_fallback));
    drop(fences);
    if should_retire {
        state
            .retire_client_dispatch_fence_generation(
                &clear.client_id,
                clear.gateway_epoch,
                clear.expected_generation,
            )
            .await;
    }
    GatewayClientDispatchFenceResult {
        client_id: clear.client_id,
        accepted,
        fenced,
        ownership_continuous: false,
        message: if retired_owner_visible {
            "dispatch_fence_generation_retired".to_string()
        } else if restored {
            format!("dispatch_fence_restored:{}", clear.reason)
        } else if directly_owned {
            format!("dispatch_fence_cleared:{}", clear.reason)
        } else if owned_as_fallback {
            "dispatch_fence_clear_owner_is_fallback".to_string()
        } else if fenced {
            "dispatch_fence_clear_token_mismatch".to_string()
        } else {
            "dispatch_fence_already_clear".to_string()
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
    if let Some(required_owner) = disconnect.required_dispatch_fence_owner {
        let retired = required_owner.gateway_epoch != state.client_dispatch_fence_epoch
            || state
                .client_dispatch_fence_generations
                .read()
                .await
                .get(&disconnect.client_id)
                .is_some_and(|generation| {
                    required_owner.generation <= generation.retired_generation
                });
        let still_owned = state
            .client_dispatch_fences
            .read()
            .await
            .get(&disconnect.client_id)
            .cloned()
            .is_some_and(|fence| {
                !retired
                    && fence.owner() == required_owner
                    && fence.purpose == GatewayClientDispatchFencePurpose::Deletion
                    && matches!(fence.state, GatewayClientDispatchFenceState::Persistent)
            });
        if !still_owned {
            return Ok(GatewaySessionDisconnectResult {
                client_id: disconnect.client_id,
                accepted: true,
                disconnected: false,
                message: "dispatch_fence_owner_superseded".to_string(),
            });
        }
    }
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

async fn disconnect_gateway_sessions(
    state: &GatewayState,
    batch: GatewaySessionDisconnectBatchRequest,
) -> std::result::Result<GatewaySessionDisconnectBatchResult, String> {
    validate_gateway_control_batch(
        batch.items.len(),
        batch.items.iter().map(|item| item.client_id.as_str()),
        "session_disconnect_batch",
    )?;
    let mut results = Vec::with_capacity(batch.items.len());
    for item in batch.items {
        results.push(
            disconnect_gateway_session(state, item)
                .await
                .map_err(|error| format!("session_disconnect_batch_failed:{error}"))?,
        );
    }
    Ok(GatewaySessionDisconnectBatchResult { results })
}

async fn write_gateway_error<S>(stream: &mut S, error: anyhow::Error) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    let message = error.to_string();
    let status = if message.contains("agent_not_online") {
        "404 Not Found"
    } else if message.contains("agent_suspended")
        || message.contains("agent_lifecycle_fenced")
        || message.contains("agent_lifecycle_recheck_required")
        || message.contains("agent_lifecycle_recheck_stale")
        || message.contains("agent_gateway_epoch_recheck_required")
    {
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
