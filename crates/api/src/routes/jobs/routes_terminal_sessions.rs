use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::Deserialize;
use uuid::Uuid;
use vpsman_common::{
    encode_json, payload_hash, TerminalControlAck, TerminalControlAction, TerminalControlRequest,
    MAX_TERMINAL_INPUT_BYTES,
};

use crate::{
    error::ApiError,
    job_terminal::{validate_terminal_close, validate_terminal_resize},
    model_terminal::{TerminalControlSubmitRequest, TerminalReplayView, TerminalSessionView},
    security::{operator_has_scope, SCOPE_TERMINAL_READ},
    state::AppState,
    util::limit_or_default,
};

const DEFAULT_TERMINAL_REPLAY_LIMIT: i64 = 100;
const MAX_TERMINAL_REPLAY_BYTES: i64 = 4 * 1024 * 1024;

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
    let sessions = state
        .repo
        .list_terminal_sessions(1, Some(&client_id), Some(session_id))
        .await?;
    let session = sessions
        .first()
        .ok_or_else(|| ApiError::not_found("terminal_session_not_found"))?;
    if matches!(session.state.as_str(), "opening" | "open") {
        state
            .repo
            .reconcile_terminal_job_by_id(session.job_id)
            .await?;
    }
    let job_id = state
        .repo
        .authorize_terminal_control(&client_id, session_id, &operator)
        .await?;
    state
        .repo
        .reconcile_terminal_job(job_id, &client_id, "open")
        .await?;
    let agent = state
        .repo
        .agent_by_id(&client_id)
        .await
        .map_err(|_| ApiError::not_found("terminal_agent_not_found"))?;
    let process_incarnation_id = agent
        .process_incarnation_id
        .filter(|_| agent.status == "online")
        .ok_or_else(|| ApiError::conflict("terminal_agent_not_online"))?;
    let action_hash = payload_hash(
        &encode_json(&request.action)
            .map_err(|_| ApiError::bad_request("terminal_control_action_invalid"))?,
    );
    let action = request.action.clone();
    let result = state
        .gateway
        .terminal_control(
            &client_id,
            process_incarnation_id,
            TerminalControlRequest {
                request_id: request.request_id,
                session_id,
                action: request.action,
            },
        )
        .await
        .map_err(map_terminal_gateway_error)?;
    if result.client_id != client_id
        || result.ack.request_id != request.request_id
        || result.ack.session_id != session_id
        || result.ack.action != action.kind()
    {
        return Err(ApiError::conflict("terminal_control_ack_mismatch"));
    }
    validate_terminal_control_ack(&action, &result.ack)?;
    state
        .repo
        .record_terminal_control_ack(
            &operator,
            &client_id,
            job_id,
            &action,
            &action_hash,
            &result.ack,
        )
        .await?;
    if !result.ack.accepted {
        return Err(ApiError::conflict_with_message(
            "terminal_control_rejected",
            result.ack.message,
        ));
    }
    Ok(Json(result.ack))
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
