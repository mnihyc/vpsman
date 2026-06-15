use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    Json,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    error::ApiError, model_terminal::TerminalReplayView, model_terminal::TerminalSessionView,
    state::AppState, util::limit_or_default,
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
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
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
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    validate_terminal_replay_client_id(&client_id)?;
    let mut replay = state
        .repo
        .terminal_session_replay(
            &client_id,
            session_id,
            query.from_seq,
            query.limit.unwrap_or(DEFAULT_TERMINAL_REPLAY_LIMIT),
        )
        .await?;
    apply_terminal_replay_bounds_and_hydration(
        &state,
        &mut replay,
        query.include_data.unwrap_or(true),
        query
            .max_bytes
            .unwrap_or(MAX_TERMINAL_REPLAY_BYTES)
            .clamp(1, MAX_TERMINAL_REPLAY_BYTES),
    )
    .await?;
    Ok(Json(replay))
}

async fn apply_terminal_replay_bounds_and_hydration(
    state: &AppState,
    replay: &mut TerminalReplayView,
    include_data: bool,
    max_bytes: i64,
) -> Result<(), ApiError> {
    let mut byte_count = 0_i64;
    let mut kept = Vec::new();
    for mut chunk in std::mem::take(&mut replay.chunks) {
        let size_bytes = chunk.size_bytes.max(0);
        if byte_count.saturating_add(size_bytes) > max_bytes {
            replay.truncated = true;
            break;
        }
        byte_count = byte_count.saturating_add(size_bytes);
        if include_data {
            hydrate_terminal_replay_chunk(state, &mut chunk).await?;
        } else {
            chunk.data_base64 = None;
        }
        kept.push(chunk);
    }
    replay.byte_count = byte_count;
    replay.chunk_count = kept.len();
    replay.available_first_seq = kept.first().map(|chunk| chunk.terminal_seq);
    replay.chunks = kept;
    Ok(())
}

async fn hydrate_terminal_replay_chunk(
    state: &AppState,
    chunk: &mut crate::model_terminal::TerminalReplayChunkView,
) -> Result<(), ApiError> {
    if chunk.data_base64.is_some() {
        return Ok(());
    }
    if chunk.storage != "object_store" {
        return Ok(());
    }
    let store = state
        .backup_object_store
        .as_ref()
        .ok_or_else(|| ApiError::conflict("terminal_replay_object_store_not_configured"))?;
    let object_key = chunk
        .artifact_object_key
        .as_deref()
        .ok_or_else(|| ApiError::conflict("terminal_replay_artifact_missing"))?;
    let data = store
        .get_with_limit(object_key, state.artifact_max_bytes())
        .await?;
    if data.len() as i64 != chunk.size_bytes {
        return Err(ApiError::conflict("terminal_replay_artifact_size_mismatch"));
    }
    if let Some(expected_hash) = chunk.sha256_hex.as_deref() {
        if hex::encode(Sha256::digest(&data)) != expected_hash {
            return Err(ApiError::conflict("terminal_replay_artifact_hash_mismatch"));
        }
    }
    chunk.data_base64 = Some(BASE64.encode(data));
    Ok(())
}

fn validate_terminal_replay_client_id(client_id: &str) -> Result<(), ApiError> {
    if client_id.trim().is_empty() || client_id.len() > 128 || client_id.contains('/') {
        return Err(ApiError::bad_request("terminal_replay_client_id_invalid"));
    }
    Ok(())
}
