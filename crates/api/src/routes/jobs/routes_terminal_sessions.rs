use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use tokio::{
    sync::{broadcast, mpsc},
    time::{self, Duration},
};
use uuid::Uuid;
use vpsman_common::{
    encode_json, payload_hash, TerminalControlAck, TerminalControlAction, TerminalControlRequest,
    MAX_TERMINAL_INPUT_BYTES,
};

use crate::{
    auth_model::AuthContext,
    error::ApiError,
    job_terminal::{validate_terminal_close, validate_terminal_resize},
    model::WsEvent,
    model_terminal::{
        TerminalControlSubmitRequest, TerminalReplayChunkView, TerminalReplayView,
        TerminalSessionView,
    },
    security::{operator_has_scope, role_allows, SCOPE_TERMINAL_READ},
    state::AppState,
    util::limit_or_default,
};

const DEFAULT_TERMINAL_REPLAY_LIMIT: i64 = 100;
const MAX_TERMINAL_REPLAY_BYTES: i64 = 4 * 1024 * 1024;
const TERMINAL_SOCKET_AUTH_TIMEOUT_SECS: u64 = 10;
const TERMINAL_SOCKET_AUTH_REVALIDATE_SECS: u64 = 30;
const TERMINAL_SOCKET_CONTROL_QUEUE: usize = 32;
const TERMINAL_SOCKET_PENDING_INPUT_BYTES: usize = 64 * 1024;
const TERMINAL_SOCKET_REPLAY_LIMIT: i64 = 1000;
const TERMINAL_SOCKET_REPLAY_BYTES: i64 = 4 * 1024 * 1024;
const TERMINAL_SOCKET_RECENT_REQUEST_IDS: usize = 256;

#[derive(Debug, Deserialize)]
pub(crate) struct TerminalSessionQuery {
    pub(crate) limit: Option<i64>,
    pub(crate) client_id: Option<String>,
    pub(crate) session_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TerminalReplayQuery {
    pub(crate) from_seq: Option<i64>,
    pub(crate) limit: Option<i64>,
    pub(crate) max_bytes: Option<i64>,
    pub(crate) include_data: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalSocketAuthFrame {
    r#type: String,
    access_token: String,
    #[serde(default)]
    from_seq: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum TerminalSocketClientFrame {
    Input {
        request_id: Uuid,
        data_base64: String,
    },
    Resize {
        request_id: Uuid,
        cols: u16,
        rows: u16,
    },
    Close {
        request_id: Uuid,
        #[serde(default)]
        reason: Option<String>,
    },
}

impl TerminalSocketClientFrame {
    fn into_control(self) -> (Uuid, TerminalControlAction) {
        match self {
            Self::Input {
                request_id,
                data_base64,
            } => (request_id, TerminalControlAction::Input { data_base64 }),
            Self::Resize {
                request_id,
                cols,
                rows,
            } => (request_id, TerminalControlAction::Resize { cols, rows }),
            Self::Close { request_id, reason } => {
                (request_id, TerminalControlAction::Close { reason })
            }
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TerminalSocketServerFrame {
    Ready {
        session: TerminalSessionView,
        from_seq: i64,
        available_first_seq: Option<i64>,
        next_seq: i64,
        replay_truncated: bool,
        retained_bytes: Option<i64>,
        dropped_bytes: Option<i64>,
        dropped_chunks: Option<i64>,
    },
    Output {
        terminal_seq: i64,
        data_base64: String,
        size_bytes: i64,
        sha256_hex: String,
        created_at: String,
    },
    ControlAck {
        ack: TerminalControlAck,
    },
    SessionState {
        session: TerminalSessionView,
    },
    Error {
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<Uuid>,
        code: String,
        message: String,
        recoverable: bool,
    },
}

struct TerminalSocketControlWork {
    request_id: Uuid,
    action: TerminalControlAction,
    pending_input_bytes: usize,
}

struct TerminalSocketControlResult {
    request_id: Uuid,
    pending_input_bytes: usize,
    close_control: bool,
    ack: Option<TerminalControlAck>,
    session: Option<TerminalSessionView>,
    error: Option<ApiError>,
    terminal: bool,
}

#[derive(Clone, Debug)]
struct TerminalSocketAuthority {
    operator: AuthContext,
    session: TerminalSessionView,
    process_incarnation_id: Uuid,
}

pub(crate) async fn list_terminal_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TerminalSessionQuery>,
) -> Result<Json<Vec<TerminalSessionView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_TERMINAL_READ)
        .await?;
    let client_id = query
        .client_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(client_id) = client_id {
        if client_id.len() > 128 {
            return Err(ApiError::bad_request("terminal_client_id_too_long"));
        }
    }
    let sessions = state
        .repo
        .list_terminal_sessions(limit_or_default(query.limit), client_id, query.session_id)
        .await?;
    for session in &sessions {
        state
            .repo
            .reconcile_terminal_job_by_id(session.job_id)
            .await?;
    }
    Ok(Json(
        state
            .repo
            .list_terminal_sessions(limit_or_default(query.limit), client_id, query.session_id)
            .await?,
    ))
}

pub(crate) async fn terminal_session_replay(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((client_id, session_id)): Path<(String, Uuid)>,
    Query(query): Query<TerminalReplayQuery>,
) -> Result<Json<TerminalReplayView>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_TERMINAL_READ)
        .await?;
    validate_terminal_replay_client_id(&client_id)?;
    let sessions = state
        .repo
        .list_terminal_sessions(1, Some(&client_id), Some(session_id))
        .await?;
    let session = sessions
        .first()
        .ok_or_else(|| ApiError::not_found("terminal_session_not_found"))?;
    state
        .repo
        .reconcile_terminal_job_by_id(session.job_id)
        .await?;
    let replay = state
        .repo
        .terminal_session_replay(
            &client_id,
            session_id,
            query.from_seq,
            query.limit.unwrap_or(DEFAULT_TERMINAL_REPLAY_LIMIT),
            query
                .max_bytes
                .unwrap_or(MAX_TERMINAL_REPLAY_BYTES)
                .clamp(1, MAX_TERMINAL_REPLAY_BYTES),
            query.include_data.unwrap_or(true),
        )
        .await?;
    Ok(Json(replay))
}

pub(crate) async fn terminal_ws_handler(
    State(state): State<AppState>,
    Path((client_id, session_id)): Path<(String, Uuid)>,
    ws: WebSocketUpgrade,
) -> Response {
    if let Err(error) = validate_terminal_replay_client_id(&client_id) {
        return error.into_response();
    }
    if session_id.is_nil() {
        return ApiError::bad_request("terminal_session_id_invalid").into_response();
    }
    ws.on_upgrade(move |socket| handle_terminal_socket(socket, state, client_id, session_id))
        .into_response()
}

pub(crate) async fn control_terminal_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((client_id, session_id)): Path<(String, Uuid)>,
    Json(request): Json<TerminalControlSubmitRequest>,
) -> Result<Json<TerminalControlAck>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    if !operator_has_scope(&operator.operator.scopes, SCOPE_TERMINAL_READ) {
        return Err(ApiError::forbidden("operator_scope_insufficient"));
    }
    validate_terminal_replay_client_id(&client_id)?;
    if session_id.is_nil() {
        return Err(ApiError::bad_request("terminal_session_id_invalid"));
    }
    if request.request_id.is_nil() {
        return Err(ApiError::bad_request("terminal_control_request_id_invalid"));
    }
    validate_terminal_control_action(session_id, &request.action)?;
    let session = load_current_terminal_session(&state, &client_id, session_id).await?;
    if matches!(session.state.as_str(), "opening" | "open") {
        state
            .repo
            .reconcile_terminal_job_by_id(session.job_id)
            .await?;
    }
    let authority =
        authorize_terminal_socket_context(&state, &client_id, session_id, operator).await?;
    let result = dispatch_bound_terminal_control(
        &state,
        &client_id,
        session_id,
        &authority,
        TerminalSocketControlWork {
            request_id: request.request_id,
            pending_input_bytes: terminal_action_input_bytes(&request.action),
            action: request.action,
        },
    )
    .await;
    if let Some(error) = result.error {
        return Err(error);
    }
    let ack = result
        .ack
        .ok_or_else(|| ApiError::conflict("terminal_control_ack_missing"))?;
    if !ack.accepted {
        return Err(ApiError::conflict_with_message(
            "terminal_control_rejected",
            ack.message,
        ));
    }
    Ok(Json(ack))
}

async fn handle_terminal_socket(
    mut socket: WebSocket,
    state: AppState,
    client_id: String,
    session_id: Uuid,
) {
    let auth = match receive_terminal_socket_auth(&mut socket).await {
        Ok(auth) => auth,
        Err(error) => {
            let _ = send_terminal_error(&mut socket, None, error).await;
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    };
    let authority = match authenticate_terminal_socket(
        &state,
        &auth.access_token,
        &client_id,
        session_id,
        true,
    )
    .await
    {
        Ok(authority) => authority,
        Err(error) => {
            let _ = send_terminal_error(&mut socket, None, error).await;
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    };

    let from_seq = auth
        .from_seq
        .unwrap_or_else(|| {
            authority
                .session
                .output_retained_first_seq
                .or(authority.session.output_first_seq)
                .unwrap_or(1)
        })
        .max(1);
    let mut events = state.events.subscribe();
    let initial_replay =
        match load_terminal_socket_replay(&state, &client_id, session_id, from_seq).await {
            Ok(replay) => replay,
            Err(error) => {
                let _ = send_terminal_error(&mut socket, None, error).await;
                let _ = socket.send(Message::Close(None)).await;
                return;
            }
        };
    if !send_terminal_frame(
        &mut socket,
        &TerminalSocketServerFrame::Ready {
            session: authority.session.clone(),
            from_seq,
            available_first_seq: initial_replay.available_first_seq,
            next_seq: initial_replay.next_seq,
            replay_truncated: authority.session.output_replay_truncated,
            retained_bytes: authority.session.output_retained_bytes,
            dropped_bytes: authority.session.output_dropped_bytes,
            dropped_chunks: authority.session.output_dropped_chunks,
        },
    )
    .await
    {
        return;
    }
    let Some(mut replay_cursor) =
        stream_terminal_replay(&mut socket, &state, &client_id, session_id, initial_replay).await
    else {
        return;
    };

    let (control_tx, control_rx) = mpsc::channel(TERMINAL_SOCKET_CONTROL_QUEUE);
    let (result_tx, mut result_rx) = mpsc::channel(TERMINAL_SOCKET_CONTROL_QUEUE);
    tokio::spawn(run_terminal_control_worker(
        state.clone(),
        client_id.clone(),
        session_id,
        authority,
        control_rx,
        result_tx,
    ));

    let mut auth_revalidate =
        time::interval(Duration::from_secs(TERMINAL_SOCKET_AUTH_REVALIDATE_SECS));
    auth_revalidate.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    auth_revalidate.tick().await;
    let mut pending_input_bytes = 0_usize;
    let mut close_queued = false;
    let mut seen_request_ids = HashSet::new();
    let mut recent_request_ids = VecDeque::new();

    loop {
        tokio::select! {
            message = socket.next() => {
                let Some(message) = message else { break; };
                match message {
                    Ok(Message::Text(payload)) => {
                        let frame = match serde_json::from_str::<TerminalSocketClientFrame>(&payload) {
                            Ok(frame) => frame,
                            Err(_) => {
                                if !send_terminal_error(
                                    &mut socket,
                                    None,
                                    ApiError::bad_request("terminal_socket_frame_invalid"),
                                ).await {
                                    break;
                                }
                                continue;
                            }
                        };
                        let (request_id, action) = frame.into_control();
                        if request_id.is_nil() {
                            if !send_terminal_error(
                                &mut socket,
                                Some(request_id),
                                ApiError::bad_request("terminal_control_request_id_invalid"),
                            ).await {
                                break;
                            }
                            continue;
                        }
                        if let Err(error) = validate_terminal_control_action(session_id, &action) {
                            if !send_terminal_error(&mut socket, Some(request_id), error).await {
                                break;
                            }
                            continue;
                        }
                        if close_queued {
                            if !send_terminal_error(
                                &mut socket,
                                Some(request_id),
                                ApiError::conflict("terminal_session_closing"),
                            ).await {
                                break;
                            }
                            continue;
                        }
                        if seen_request_ids.contains(&request_id) {
                            if !send_terminal_error(
                                &mut socket,
                                Some(request_id),
                                ApiError::conflict("terminal_control_request_id_reused"),
                            ).await {
                                break;
                            }
                            continue;
                        }
                        let input_bytes = terminal_action_input_bytes(&action);
                        if pending_input_bytes.saturating_add(input_bytes)
                            > TERMINAL_SOCKET_PENDING_INPUT_BYTES
                        {
                            if !send_terminal_error(
                                &mut socket,
                                Some(request_id),
                                ApiError {
                                    status: StatusCode::SERVICE_UNAVAILABLE,
                                    code: "terminal_input_queue_full",
                                    error: anyhow::anyhow!("terminal_input_queue_full"),
                                    public_message: None,
                                },
                            ).await {
                                break;
                            }
                            continue;
                        }
                        let terminal = matches!(action, TerminalControlAction::Close { .. });
                        let work = TerminalSocketControlWork {
                            request_id,
                            action,
                            pending_input_bytes: input_bytes,
                        };
                        match control_tx.try_send(work) {
                            Ok(()) => {
                                pending_input_bytes = pending_input_bytes.saturating_add(input_bytes);
                                close_queued |= terminal;
                                remember_terminal_request_id(
                                    request_id,
                                    &mut seen_request_ids,
                                    &mut recent_request_ids,
                                );
                            }
                            Err(_) => {
                                if !send_terminal_error(
                                    &mut socket,
                                    Some(request_id),
                                    ApiError {
                                        status: StatusCode::SERVICE_UNAVAILABLE,
                                        code: "terminal_control_busy",
                                        error: anyhow::anyhow!("terminal_control_busy"),
                                        public_message: None,
                                    },
                                ).await {
                                    break;
                                }
                            }
                        }
                    }
                    Ok(Message::Ping(payload)) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Ok(Message::Pong(_)) => {}
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(Message::Binary(_)) => {
                        if !send_terminal_error(
                            &mut socket,
                            None,
                            ApiError::bad_request("terminal_socket_text_required"),
                        ).await {
                            break;
                        }
                    }
                }
            }
            result = result_rx.recv() => {
                let Some(result) = result else { break; };
                pending_input_bytes = pending_input_bytes
                    .saturating_sub(result.pending_input_bytes);
                if should_clear_terminal_close_queue(result.close_control, result.terminal) {
                    close_queued = false;
                }
                if let Some(error) = result.error {
                    if !send_terminal_error(&mut socket, Some(result.request_id), error).await {
                        break;
                    }
                }
                if let Some(ack) = result.ack {
                    if !send_terminal_frame(
                        &mut socket,
                        &TerminalSocketServerFrame::ControlAck { ack },
                    ).await {
                        break;
                    }
                }
                if let Some(session) = result.session {
                    if !send_terminal_frame(
                        &mut socket,
                        &TerminalSocketServerFrame::SessionState { session },
                    ).await {
                        break;
                    }
                }
                if result.terminal {
                    let _ = socket.send(Message::Close(None)).await;
                    break;
                }
            }
            _ = auth_revalidate.tick() => {
                match authenticate_terminal_socket(
                    &state,
                    &auth.access_token,
                    &client_id,
                    session_id,
                    false,
                ).await {
                    Ok(_) => {
                        if socket.send(Message::Ping(Default::default())).await.is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = send_terminal_error(&mut socket, None, error).await;
                        let _ = socket.send(Message::Close(None)).await;
                        break;
                    }
                }
            }
            event = events.recv() => {
                match event {
                    Ok(WsEvent::TerminalOutputRecorded {
                        client_id: event_client_id,
                        session_id: event_session_id,
                        terminal_seq,
                        done,
                        ..
                    }) if event_client_id == client_id && event_session_id == session_id => {
                        if !terminal_event_requires_replay(terminal_seq, done) {
                            continue;
                        }
                        let replay = match load_terminal_socket_replay(
                            &state,
                            &client_id,
                            session_id,
                            replay_cursor,
                        ).await {
                            Ok(replay) => replay,
                            Err(error) => {
                                let _ = send_terminal_error(&mut socket, None, error).await;
                                break;
                            }
                        };
                        let Some(next_cursor) = stream_terminal_replay(
                            &mut socket,
                            &state,
                            &client_id,
                            session_id,
                            replay,
                        ).await else {
                            break;
                        };
                        replay_cursor = next_cursor;
                        if done {
                            if !send_current_terminal_session(
                                &mut socket,
                                &state,
                                &client_id,
                                session_id,
                            ).await {
                                break;
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let replay = match load_terminal_socket_replay(
                            &state,
                            &client_id,
                            session_id,
                            replay_cursor,
                        ).await {
                            Ok(replay) => replay,
                            Err(error) => {
                                let _ = send_terminal_error(&mut socket, None, error).await;
                                break;
                            }
                        };
                        let Some(next_cursor) = stream_terminal_replay(
                            &mut socket,
                            &state,
                            &client_id,
                            session_id,
                            replay,
                        ).await else {
                            break;
                        };
                        replay_cursor = next_cursor;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

async fn receive_terminal_socket_auth(
    socket: &mut WebSocket,
) -> Result<TerminalSocketAuthFrame, ApiError> {
    let message = time::timeout(
        Duration::from_secs(TERMINAL_SOCKET_AUTH_TIMEOUT_SECS),
        socket.next(),
    )
    .await
    .map_err(|_| ApiError::unauthorized("terminal_socket_auth_timeout"))?
    .ok_or_else(|| ApiError::unauthorized("terminal_socket_auth_required"))?
    .map_err(|_| ApiError::unauthorized("terminal_socket_auth_invalid"))?;
    let Message::Text(payload) = message else {
        return Err(ApiError::unauthorized("terminal_socket_auth_required"));
    };
    let auth = serde_json::from_str::<TerminalSocketAuthFrame>(&payload)
        .map_err(|_| ApiError::unauthorized("terminal_socket_auth_invalid"))?;
    if auth.r#type != "auth" || auth.access_token.trim().is_empty() {
        return Err(ApiError::unauthorized("terminal_socket_auth_invalid"));
    }
    if auth.from_seq.is_some_and(|from_seq| from_seq < 1) {
        return Err(ApiError::bad_request("terminal_replay_from_seq_invalid"));
    }
    Ok(auth)
}

async fn authenticate_terminal_socket(
    state: &AppState,
    access_token: &str,
    client_id: &str,
    session_id: Uuid,
    reconcile_on_attach: bool,
) -> Result<TerminalSocketAuthority, ApiError> {
    let operator = state
        .repo
        .authenticate_access_token(access_token)
        .await?
        .ok_or_else(|| ApiError::unauthorized("invalid_bearer_token"))?;
    if !role_allows(&operator.operator.role, "operator") {
        return Err(ApiError::forbidden("operator_role_insufficient"));
    }
    if !operator_has_scope(&operator.operator.scopes, "jobs:write")
        || !operator_has_scope(&operator.operator.scopes, SCOPE_TERMINAL_READ)
    {
        return Err(ApiError::forbidden("operator_scope_insufficient"));
    }
    if reconcile_on_attach {
        let session = load_current_terminal_session(state, client_id, session_id).await?;
        if matches!(session.state.as_str(), "opening" | "open") {
            state
                .repo
                .reconcile_terminal_job_by_id(session.job_id)
                .await?;
        }
    }
    authorize_terminal_socket_context(state, client_id, session_id, operator).await
}

async fn load_current_terminal_session(
    state: &AppState,
    client_id: &str,
    session_id: Uuid,
) -> Result<TerminalSessionView, ApiError> {
    state
        .repo
        .list_terminal_sessions(1, Some(client_id), Some(session_id))
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::not_found("terminal_session_not_found"))
}

async fn run_terminal_control_worker(
    state: AppState,
    client_id: String,
    session_id: Uuid,
    authority: TerminalSocketAuthority,
    mut control_rx: mpsc::Receiver<TerminalSocketControlWork>,
    result_tx: mpsc::Sender<TerminalSocketControlResult>,
) {
    while let Some(work) = control_rx.recv().await {
        let result =
            dispatch_bound_terminal_control(&state, &client_id, session_id, &authority, work).await;
        if result_tx.send(result).await.is_err() {
            break;
        }
    }
}

async fn dispatch_bound_terminal_control(
    state: &AppState,
    client_id: &str,
    session_id: Uuid,
    authority: &TerminalSocketAuthority,
    work: TerminalSocketControlWork,
) -> TerminalSocketControlResult {
    let request_id = work.request_id;
    let pending_input_bytes = work.pending_input_bytes;
    let action = work.action;
    let close_control = matches!(action, TerminalControlAction::Close { .. });
    let result = async {
        let gateway_result = state
            .gateway
            .terminal_control(
                client_id,
                authority.process_incarnation_id,
                TerminalControlRequest {
                    request_id,
                    session_id,
                    action: action.clone(),
                },
            )
            .await
            .map_err(map_terminal_gateway_error)?;
        if gateway_result.client_id != client_id
            || gateway_result.ack.request_id != request_id
            || gateway_result.ack.session_id != session_id
            || gateway_result.ack.action != action.kind()
        {
            return Err(ApiError::conflict("terminal_control_ack_mismatch"));
        }
        validate_terminal_control_ack(&action, &gateway_result.ack)?;
        let lifecycle_event = matches!(action, TerminalControlAction::Close { .. })
            || (!gateway_result.ack.accepted
                && matches!(
                    gateway_result.ack.status.as_str(),
                    "missing" | "failed" | "exited"
                ));
        let action_hash = if lifecycle_event {
            payload_hash(
                &encode_json(&action)
                    .map_err(|_| ApiError::bad_request("terminal_control_action_invalid"))?,
            )
        } else {
            String::new()
        };
        state
            .repo
            .record_terminal_control_ack(
                &authority.operator,
                client_id,
                authority.session.job_id,
                &action,
                &action_hash,
                &gateway_result.ack,
            )
            .await?;
        let terminal = (gateway_result.ack.accepted
            && matches!(action, TerminalControlAction::Close { .. }))
            || (!gateway_result.ack.accepted
                && matches!(
                    gateway_result.ack.status.as_str(),
                    "missing" | "failed" | "exited"
                ));
        let session = if terminal {
            Some(load_current_terminal_session(state, client_id, session_id).await?)
        } else {
            None
        };
        Ok((gateway_result.ack, session, terminal))
    }
    .await;
    match result {
        Ok((ack, session, terminal)) => TerminalSocketControlResult {
            request_id,
            pending_input_bytes,
            close_control,
            ack: Some(ack),
            session,
            error: None,
            terminal,
        },
        Err(error) => {
            let terminal = matches!(
                error.code,
                "terminal_session_not_open"
                    | "terminal_agent_not_online"
                    | "terminal_agent_reconnected"
            );
            TerminalSocketControlResult {
                request_id,
                pending_input_bytes,
                close_control,
                ack: None,
                session: if terminal {
                    load_current_terminal_session(state, client_id, session_id)
                        .await
                        .ok()
                } else {
                    None
                },
                terminal,
                error: Some(error),
            }
        }
    }
}

async fn authorize_terminal_socket_context(
    state: &AppState,
    client_id: &str,
    session_id: Uuid,
    operator: AuthContext,
) -> Result<TerminalSocketAuthority, ApiError> {
    let session = load_current_terminal_session(state, client_id, session_id).await?;
    let job_id = state
        .repo
        .authorize_terminal_control(client_id, session_id, &operator)
        .await?;
    let agent = state
        .repo
        .agent_by_id(client_id)
        .await
        .map_err(|_| ApiError::not_found("terminal_agent_not_found"))?;
    let process_incarnation_id = agent
        .process_incarnation_id
        .filter(|_| agent.status == "online")
        .ok_or_else(|| ApiError::conflict("terminal_agent_not_online"))?;
    let targets = state.repo.list_job_targets(job_id).await?;
    let target = targets
        .iter()
        .find(|target| target.client_id == client_id)
        .ok_or_else(|| ApiError::conflict("terminal_session_job_invalid"))?;
    if target
        .process_incarnation_id
        .is_some_and(|expected| expected != process_incarnation_id)
    {
        return Err(ApiError::conflict("terminal_agent_reconnected"));
    }
    Ok(TerminalSocketAuthority {
        operator,
        session,
        process_incarnation_id,
    })
}

async fn load_terminal_socket_replay(
    state: &AppState,
    client_id: &str,
    session_id: Uuid,
    from_seq: i64,
) -> Result<TerminalReplayView, ApiError> {
    state
        .repo
        .terminal_session_replay(
            client_id,
            session_id,
            Some(from_seq.max(1)),
            TERMINAL_SOCKET_REPLAY_LIMIT,
            TERMINAL_SOCKET_REPLAY_BYTES,
            true,
        )
        .await
        .map_err(ApiError::from)
}

async fn stream_terminal_replay(
    socket: &mut WebSocket,
    state: &AppState,
    client_id: &str,
    session_id: Uuid,
    mut replay: TerminalReplayView,
) -> Option<i64> {
    loop {
        let batch_next_seq = replay
            .chunks
            .last()
            .map(|chunk| chunk.terminal_seq.saturating_add(1));
        for chunk in replay.chunks {
            if !send_terminal_output(socket, chunk).await {
                return None;
            }
        }
        let cursor = batch_next_seq.unwrap_or(replay.next_seq).max(1);
        if !replay.truncated || cursor >= replay.next_seq {
            return Some(cursor.max(replay.next_seq));
        }
        replay = match load_terminal_socket_replay(state, client_id, session_id, cursor).await {
            Ok(replay) => replay,
            Err(error) => {
                let _ = send_terminal_error(socket, None, error).await;
                return None;
            }
        };
    }
}

async fn send_terminal_output(socket: &mut WebSocket, chunk: TerminalReplayChunkView) -> bool {
    let Some(data_base64) = chunk.data_base64 else {
        return false;
    };
    send_terminal_frame(
        socket,
        &TerminalSocketServerFrame::Output {
            terminal_seq: chunk.terminal_seq,
            data_base64,
            size_bytes: chunk.size_bytes,
            sha256_hex: chunk.sha256_hex,
            created_at: chunk.created_at,
        },
    )
    .await
}

async fn send_current_terminal_session(
    socket: &mut WebSocket,
    state: &AppState,
    client_id: &str,
    session_id: Uuid,
) -> bool {
    match load_current_terminal_session(state, client_id, session_id).await {
        Ok(session) => {
            send_terminal_frame(socket, &TerminalSocketServerFrame::SessionState { session }).await
        }
        Err(error) => send_terminal_error(socket, None, error).await,
    }
}

async fn send_terminal_error(
    socket: &mut WebSocket,
    request_id: Option<Uuid>,
    error: ApiError,
) -> bool {
    let recoverable = terminal_socket_error_recoverable(error.status);
    let message = error
        .public_message
        .unwrap_or_else(|| error.code.replace('_', " "));
    send_terminal_frame(
        socket,
        &TerminalSocketServerFrame::Error {
            request_id,
            code: error.code.to_string(),
            message,
            recoverable,
        },
    )
    .await
}

async fn send_terminal_frame(socket: &mut WebSocket, frame: &TerminalSocketServerFrame) -> bool {
    let Ok(payload) = serde_json::to_string(frame) else {
        return false;
    };
    socket.send(Message::Text(payload.into())).await.is_ok()
}

fn terminal_action_input_bytes(action: &TerminalControlAction) -> usize {
    match action {
        TerminalControlAction::Input { data_base64 } => BASE64_STANDARD
            .decode(data_base64.as_bytes())
            .map_or(0, |data| data.len()),
        TerminalControlAction::Resize { .. } | TerminalControlAction::Close { .. } => 0,
    }
}

fn remember_terminal_request_id(
    request_id: Uuid,
    seen: &mut HashSet<Uuid>,
    recent: &mut VecDeque<Uuid>,
) {
    seen.insert(request_id);
    recent.push_back(request_id);
    if recent.len() > TERMINAL_SOCKET_RECENT_REQUEST_IDS {
        if let Some(expired) = recent.pop_front() {
            seen.remove(&expired);
        }
    }
}

fn should_clear_terminal_close_queue(close_control: bool, terminal: bool) -> bool {
    close_control && !terminal
}

fn terminal_socket_error_recoverable(status: StatusCode) -> bool {
    status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS
}

fn terminal_event_requires_replay(terminal_seq: Option<u64>, done: bool) -> bool {
    terminal_seq.is_some() || done
}

fn validate_terminal_control_action(
    session_id: Uuid,
    action: &TerminalControlAction,
) -> Result<(), ApiError> {
    match action {
        TerminalControlAction::Input { data_base64 } => {
            if data_base64.is_empty()
                || data_base64.len() > MAX_TERMINAL_INPUT_BYTES.div_ceil(3) * 4 + 16
            {
                return Err(ApiError::bad_request("terminal_input_size_invalid"));
            }
            let data = BASE64_STANDARD
                .decode(data_base64.as_bytes())
                .map_err(|_| ApiError::bad_request("terminal_input_base64_invalid"))?;
            if data.is_empty() || data.len() > MAX_TERMINAL_INPUT_BYTES {
                return Err(ApiError::bad_request("terminal_input_size_invalid"));
            }
            Ok(())
        }
        TerminalControlAction::Resize { cols, rows } => {
            validate_terminal_resize(session_id, *cols, *rows)
        }
        TerminalControlAction::Close { reason } => {
            validate_terminal_close(session_id, reason.as_deref())
        }
    }
}

fn validate_terminal_control_ack(
    action: &TerminalControlAction,
    ack: &TerminalControlAck,
) -> Result<(), ApiError> {
    if !ack.accepted {
        if matches!(
            ack.status.as_str(),
            "rejected" | "missing" | "failed" | "exited"
        ) {
            return Ok(());
        }
        return Err(ApiError::conflict("terminal_control_ack_mismatch"));
    }
    let valid = match action {
        TerminalControlAction::Input { data_base64 } => {
            let expected_bytes = BASE64_STANDARD
                .decode(data_base64.as_bytes())
                .map_err(|_| ApiError::bad_request("terminal_input_base64_invalid"))?
                .len() as u64;
            ack.status == "accepted"
                && ack.input_seq.is_some_and(|input_seq| input_seq > 0)
                && ack.written_bytes == Some(expected_bytes)
                && ack.cols.is_none()
                && ack.rows.is_none()
        }
        TerminalControlAction::Resize { cols, rows } => {
            ack.status == "resized"
                && ack.cols == Some(*cols)
                && ack.rows == Some(*rows)
                && ack.input_seq.is_none()
                && ack.written_bytes.is_none()
        }
        TerminalControlAction::Close { .. } => {
            ack.status == "closed"
                && ack.input_seq.is_none()
                && ack.written_bytes.is_none()
                && ack.cols.is_none()
                && ack.rows.is_none()
        }
    };
    if valid {
        Ok(())
    } else {
        Err(ApiError::conflict("terminal_control_ack_mismatch"))
    }
}

fn map_terminal_gateway_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    let (status, code) =
        if message.contains("agent_not_online") || message.contains("agent_session_closed") {
            (StatusCode::CONFLICT, "terminal_agent_not_online")
        } else if message.contains("agent_incarnation_mismatch") {
            (StatusCode::CONFLICT, "terminal_agent_reconnected")
        } else if message.contains("queue_full") {
            (StatusCode::SERVICE_UNAVAILABLE, "terminal_control_busy")
        } else if message.contains("timed out") {
            (StatusCode::GATEWAY_TIMEOUT, "terminal_control_timeout")
        } else {
            (StatusCode::BAD_GATEWAY, "terminal_control_delivery_failed")
        };
    ApiError {
        status,
        code,
        error,
        public_message: None,
    }
}

fn validate_terminal_replay_client_id(client_id: &str) -> Result<(), ApiError> {
    if client_id.trim().is_empty() || client_id.len() > 128 || client_id.contains('/') {
        return Err(ApiError::bad_request("terminal_replay_client_id_invalid"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests_routes_terminal_sessions.rs"]
mod tests;
