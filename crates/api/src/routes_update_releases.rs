use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;

use crate::{
    error::ApiError,
    model::{AgentUpdateReleaseView, CreateAgentUpdateReleaseRequest, HistoryQuery},
    security::SCOPE_CONFIG_READ,
    state::AppState,
    util::limit_or_default,
};

const MAX_RELEASE_NAME_BYTES: usize = 80;
const MAX_RELEASE_VERSION_BYTES: usize = 80;
const MAX_RELEASE_CHANNEL_BYTES: usize = 32;
const MAX_RELEASE_NOTES_BYTES: usize = 1024;
const MAX_RELEASE_ARTIFACT_URL_BYTES: usize = 2048;
const MAX_RELEASE_ARTIFACT_BYTES: i64 = 16 * 1024 * 1024;

pub(crate) async fn list_agent_update_releases(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<AgentUpdateReleaseView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_CONFIG_READ)
        .await?;
    let releases = state
        .repo
        .list_agent_update_releases(limit_or_default(query.limit))
        .await?;
    Ok(Json(releases))
}

#[derive(Debug, Deserialize)]
pub(crate) struct LatestReleaseQuery {
    pub(crate) name: String,
    #[serde(default = "default_latest_channel")]
    pub(crate) channel: String,
}

pub(crate) async fn latest_agent_update_release(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<LatestReleaseQuery>,
) -> Result<Json<AgentUpdateReleaseView>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_CONFIG_READ)
        .await?;
    validate_release_token(
        &query.name,
        MAX_RELEASE_NAME_BYTES,
        "agent_update_release_name_invalid",
    )?;
    validate_release_token(
        &query.channel,
        MAX_RELEASE_CHANNEL_BYTES,
        "agent_update_release_channel_invalid",
    )?;
    let channel = query.channel.trim().to_ascii_lowercase();
    let release = state
        .repo
        .list_agent_update_releases(200)
        .await?
        .into_iter()
        .find(|release| release.name == query.name.trim() && release.channel == channel)
        .ok_or_else(|| ApiError::not_found("agent_update_release_not_found"))?;
    Ok(Json(release))
}

pub(crate) async fn create_agent_update_release(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateAgentUpdateReleaseRequest>,
) -> Result<(StatusCode, Json<AgentUpdateReleaseView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "config:write")
        .await?;
    validate_agent_update_release_request(&state, &request)?;
    let release = state
        .repo
        .record_agent_update_release(&request, &operator)
        .await
        .map_err(|error| {
            if error
                .to_string()
                .contains("agent_update_release_already_exists")
                || error.to_string().contains("duplicate key")
            {
                ApiError::conflict("agent_update_release_already_exists")
            } else {
                ApiError::from(error)
            }
        })?;
    Ok((StatusCode::CREATED, Json(release)))
}

pub(crate) fn validate_agent_update_release_request(
    state: &AppState,
    request: &CreateAgentUpdateReleaseRequest,
) -> Result<(), ApiError> {
    validate_release_token(
        &request.name,
        MAX_RELEASE_NAME_BYTES,
        "agent_update_release_name_invalid",
    )?;
    validate_release_token(
        &request.version,
        MAX_RELEASE_VERSION_BYTES,
        "agent_update_release_version_invalid",
    )?;
    validate_release_token(
        &request.channel,
        MAX_RELEASE_CHANNEL_BYTES,
        "agent_update_release_channel_invalid",
    )?;
    if !request.confirmed {
        return Err(ApiError::conflict(
            "agent_update_release_confirmation_required",
        ));
    }
    state
        .update_release_policy
        .validate_channel(&request.channel)?;
    if !is_hex_len(&request.artifact_sha256_hex, 64) {
        return Err(ApiError::bad_request("agent_update_release_sha256_invalid"));
    }
    validate_release_url(
        &request.artifact_url,
        "agent_update_release_artifact_url_invalid",
    )?;
    if let Some(size_bytes) = request.size_bytes {
        validate_release_size(size_bytes, "agent_update_release_size_bytes_invalid")?;
    }
    validate_rollback_release_metadata(
        request.rollback_artifact_sha256_hex.as_deref(),
        request.rollback_artifact_url.as_deref(),
        request.rollback_size_bytes,
    )?;
    if let Some(notes) = request.notes.as_deref() {
        if notes.len() > MAX_RELEASE_NOTES_BYTES || notes.as_bytes().contains(&0) {
            return Err(ApiError::bad_request("agent_update_release_notes_invalid"));
        }
    }
    Ok(())
}

fn validate_rollback_release_metadata(
    sha256_hex: Option<&str>,
    artifact_url: Option<&str>,
    size_bytes: Option<i64>,
) -> Result<(), ApiError> {
    let has_any = sha256_hex.is_some() || artifact_url.is_some() || size_bytes.is_some();
    if !has_any {
        return Ok(());
    }
    let sha256_hex = sha256_hex
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad_request("agent_update_rollback_release_sha256_required"))?;
    let artifact_url = artifact_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request("agent_update_rollback_release_artifact_url_required")
        })?;
    if !is_hex_len(sha256_hex, 64) {
        return Err(ApiError::bad_request(
            "agent_update_rollback_release_sha256_invalid",
        ));
    }
    validate_release_url(
        artifact_url,
        "agent_update_rollback_release_artifact_url_invalid",
    )?;
    if let Some(size_bytes) = size_bytes {
        validate_release_size(
            size_bytes,
            "agent_update_rollback_release_size_bytes_invalid",
        )?;
    }
    Ok(())
}

pub(crate) fn validate_release_token(
    value: &str,
    max_bytes: usize,
    code: &'static str,
) -> Result<(), ApiError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > max_bytes
        || value
            .chars()
            .any(|ch| ch.is_control() || ch == '/' || ch == '\\')
    {
        return Err(ApiError::bad_request(code));
    }
    Ok(())
}

fn validate_release_url(value: &str, code: &'static str) -> Result<(), ApiError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_RELEASE_ARTIFACT_URL_BYTES
        || value.as_bytes().contains(&0)
        || !value.starts_with("https://")
    {
        return Err(ApiError::bad_request(code));
    }
    Ok(())
}

fn validate_release_size(size_bytes: i64, code: &'static str) -> Result<(), ApiError> {
    if size_bytes <= 0 || size_bytes > MAX_RELEASE_ARTIFACT_BYTES {
        return Err(ApiError::bad_request(code));
    }
    Ok(())
}

fn is_hex_len(value: &str, expected_len: usize) -> bool {
    value.len() == expected_len && value.as_bytes().iter().all(u8::is_ascii_hexdigit)
}

fn default_latest_channel() -> String {
    "stable".to_string()
}
