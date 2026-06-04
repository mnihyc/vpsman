use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, Response, StatusCode},
    Json,
};
use serde::Deserialize;
use vpsman_common::verify_update_artifact_signature;

use crate::{
    agent_update_artifact_ingest::{
        cleanup_temp_file, hosted_uploaded_artifact_ref, is_hex_len, sha256_hex,
        store_uploaded_artifact, store_uploaded_artifact_file, stream_and_verify_artifact,
        validate_base64_release_artifact, StreamArtifactMetadata, ValidatedArtifactBytes,
        MAX_RELEASE_ARTIFACT_BYTES,
    },
    error::ApiError,
    model::{
        AgentUpdateReleaseView, CreateAgentUpdateReleaseRequest,
        CreateHostedAgentUpdateReleaseRequest, HistoryQuery, StreamedAgentUpdateArtifactView,
        UploadAgentUpdateArtifactRequest,
    },
    repository_agent_update_releases::{
        UploadedAgentUpdateReleaseArtifacts, UploadedAgentUpdateReleaseMetadata,
    },
    state::AppState,
    util::limit_or_default,
};

const MAX_RELEASE_NAME_BYTES: usize = 80;
const MAX_RELEASE_VERSION_BYTES: usize = 80;
const MAX_RELEASE_CHANNEL_BYTES: usize = 32;
const MAX_RELEASE_NOTES_BYTES: usize = 1024;
const MAX_RELEASE_ARTIFACT_URL_BYTES: usize = 2048;
pub(crate) const MAX_RELEASE_ARTIFACT_UPLOAD_BODY_BYTES: usize = 24 * 1024 * 1024;

pub(crate) async fn list_agent_update_releases(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<AgentUpdateReleaseView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    let releases = state
        .repo
        .list_agent_update_releases(limit_or_default(query.limit))
        .await?
        .into_iter()
        .map(|release| state.enrich_agent_update_release_urls(release))
        .collect();
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
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
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
    Ok(Json(state.enrich_agent_update_release_urls(release)))
}

pub(crate) async fn create_agent_update_release(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateAgentUpdateReleaseRequest>,
) -> Result<(StatusCode, Json<AgentUpdateReleaseView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    validate_agent_update_release_request(&request)?;
    validate_release_policy_for_metadata(
        &state,
        &request.channel,
        &request.artifact_signing_key_hex,
        request.rollback_artifact_signing_key_hex.as_deref(),
    )?;
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
    Ok((
        StatusCode::CREATED,
        Json(state.enrich_agent_update_release_urls(release)),
    ))
}

pub(crate) async fn upload_agent_update_artifact(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UploadAgentUpdateArtifactRequest>,
) -> Result<(StatusCode, Json<AgentUpdateReleaseView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    let artifact = validate_agent_update_artifact_upload_request(&request)?;
    validate_release_policy_for_metadata(
        &state,
        &request.channel,
        &request.artifact_signing_key_hex,
        request.rollback_artifact_signing_key_hex.as_deref(),
    )?;
    let store = state
        .update_object_store
        .as_ref()
        .ok_or_else(|| ApiError::conflict("agent_update_artifact_store_not_configured"))?;
    let primary = store_uploaded_artifact(store, artifact.primary).await?;
    let rollback = if let Some(rollback) = artifact.rollback {
        Some(store_uploaded_artifact(store, rollback).await?)
    } else {
        None
    };
    let artifacts = UploadedAgentUpdateReleaseArtifacts { primary, rollback };
    let metadata = UploadedAgentUpdateReleaseMetadata::from_base64_request(&request);
    let release = state
        .repo
        .record_uploaded_agent_update_release(&metadata, &artifacts, &operator)
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
    Ok((
        StatusCode::CREATED,
        Json(state.enrich_agent_update_release_urls(release)),
    ))
}

pub(crate) async fn stream_agent_update_artifact(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Result<(StatusCode, Json<StreamedAgentUpdateArtifactView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    let metadata = StreamArtifactMetadata::from_headers(&headers)?;
    if !metadata.confirmed {
        return Err(ApiError::conflict(
            "agent_update_artifact_stream_confirmation_required",
        ));
    }
    validate_artifact_signature_metadata(
        &metadata.artifact_signature_hex,
        &metadata.artifact_signing_key_hex,
        "agent_update_release_signature_invalid",
        "agent_update_release_signing_key_invalid",
    )?;
    state.update_release_policy.validate_signing_key(
        &metadata.artifact_signing_key_hex,
        "agent_update_release_signing_key_untrusted",
    )?;
    let store = state
        .update_object_store
        .as_ref()
        .ok_or_else(|| ApiError::conflict("agent_update_artifact_store_not_configured"))?;
    let streamed = stream_and_verify_artifact(
        body,
        &metadata.artifact_signature_hex,
        &metadata.artifact_signing_key_hex,
    )
    .await?;
    let uploaded = match store_uploaded_artifact_file(store, &streamed).await {
        Ok(uploaded) => uploaded,
        Err(error) => {
            cleanup_temp_file(&streamed.temp_path).await;
            return Err(error);
        }
    };
    cleanup_temp_file(&streamed.temp_path).await;
    let artifact_signature_sha256_hex = sha256_hex(
        metadata
            .artifact_signature_hex
            .to_ascii_lowercase()
            .as_bytes(),
    );
    let artifact_signing_key_sha256_hex = sha256_hex(
        metadata
            .artifact_signing_key_hex
            .to_ascii_lowercase()
            .as_bytes(),
    );
    state
        .repo
        .record_streamed_agent_update_artifact_audit(
            &uploaded,
            &artifact_signature_sha256_hex,
            &artifact_signing_key_sha256_hex,
            &operator,
        )
        .await?;
    let artifact_download_url = state.public_update_artifact_url(&uploaded.artifact_download_path);
    Ok((
        StatusCode::CREATED,
        Json(StreamedAgentUpdateArtifactView {
            artifact_sha256_hex: uploaded.artifact_sha256_hex,
            artifact_signature_provided: true,
            artifact_signature_sha256_hex,
            artifact_signing_key_sha256_hex,
            artifact_object_key: uploaded.artifact_object_key,
            artifact_download_path: uploaded.artifact_download_path,
            artifact_download_url,
            size_bytes: uploaded.size_bytes,
        }),
    ))
}

pub(crate) async fn create_hosted_agent_update_release(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateHostedAgentUpdateReleaseRequest>,
) -> Result<(StatusCode, Json<AgentUpdateReleaseView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    let metadata = validate_hosted_agent_update_release_request(&state, &request)?;
    let store = state
        .update_object_store
        .as_ref()
        .ok_or_else(|| ApiError::conflict("agent_update_artifact_store_not_configured"))?;
    let primary = hosted_uploaded_artifact_ref(store, &request.artifact_sha256_hex).await?;
    let rollback = if request.rollback_artifact_sha256_hex.is_some() {
        Some(
            hosted_uploaded_artifact_ref(
                store,
                request
                    .rollback_artifact_sha256_hex
                    .as_deref()
                    .unwrap_or_default(),
            )
            .await?,
        )
    } else {
        None
    };
    let artifacts = UploadedAgentUpdateReleaseArtifacts { primary, rollback };
    let release = state
        .repo
        .record_uploaded_agent_update_release(&metadata, &artifacts, &operator)
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
    Ok((
        StatusCode::CREATED,
        Json(state.enrich_agent_update_release_urls(release)),
    ))
}

pub(crate) async fn download_agent_update_artifact(
    State(state): State<AppState>,
    Path(artifact_sha256_hex): Path<String>,
) -> Result<Response<Body>, ApiError> {
    if !is_hex_len(&artifact_sha256_hex, 64) {
        return Err(ApiError::bad_request(
            "agent_update_artifact_sha256_invalid",
        ));
    }
    let artifact_ref = state
        .repo
        .get_hosted_agent_update_artifact_ref(&artifact_sha256_hex)
        .await?
        .ok_or_else(|| ApiError::not_found("agent_update_artifact_not_found"))?;
    let store = state
        .update_object_store
        .as_ref()
        .ok_or_else(|| ApiError::not_found("agent_update_artifact_store_not_configured"))?;
    let bytes = store.get(&artifact_ref.artifact_object_key).await?;
    let actual_hash = sha256_hex(&bytes);
    if actual_hash != artifact_ref.artifact_sha256_hex
        || artifact_ref
            .size_bytes
            .is_some_and(|expected| expected != bytes.len() as i64)
    {
        return Err(ApiError::conflict(
            "agent_update_artifact_integrity_mismatch",
        ));
    }
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"vpsman-agent-{}.bin\"",
            &artifact_ref.artifact_sha256_hex[..12]
        ))
        .map_err(|_| ApiError::conflict("agent_update_artifact_filename_invalid"))?,
    );
    response.headers_mut().insert(
        "x-vpsman-artifact-sha256",
        HeaderValue::from_str(&artifact_ref.artifact_sha256_hex)
            .map_err(|_| ApiError::conflict("agent_update_artifact_hash_invalid"))?,
    );
    Ok(response)
}

pub(crate) fn validate_agent_update_release_request(
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
    if !is_hex_len(&request.artifact_sha256_hex, 64) {
        return Err(ApiError::bad_request("agent_update_release_sha256_invalid"));
    }
    if !is_hex_len(&request.artifact_signature_hex, 128) {
        return Err(ApiError::bad_request(
            "agent_update_release_signature_invalid",
        ));
    }
    if !is_hex_len(&request.artifact_signing_key_hex, 64) {
        return Err(ApiError::bad_request(
            "agent_update_release_signing_key_invalid",
        ));
    }
    if !verify_update_artifact_signature(
        &request.artifact_signing_key_hex,
        &request.artifact_signature_hex,
        &request.artifact_sha256_hex.to_ascii_lowercase(),
    ) {
        return Err(ApiError::bad_request(
            "agent_update_release_signature_mismatch",
        ));
    }
    if let Some(url) = request.artifact_url.as_deref() {
        let url = url.trim();
        if url.is_empty()
            || url.len() > MAX_RELEASE_ARTIFACT_URL_BYTES
            || url.as_bytes().contains(&0)
            || !url.starts_with("https://")
        {
            return Err(ApiError::bad_request(
                "agent_update_release_artifact_url_invalid",
            ));
        }
    }
    if let Some(size_bytes) = request.size_bytes {
        if size_bytes <= 0 || size_bytes > MAX_RELEASE_ARTIFACT_BYTES {
            return Err(ApiError::bad_request(
                "agent_update_release_size_bytes_invalid",
            ));
        }
    }
    validate_rollback_release_metadata(
        request.rollback_artifact_sha256_hex.as_deref(),
        request.rollback_artifact_signature_hex.as_deref(),
        request.rollback_artifact_signing_key_hex.as_deref(),
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

pub(crate) struct ValidatedArtifactUpload {
    primary: ValidatedArtifactBytes,
    rollback: Option<ValidatedArtifactBytes>,
}

pub(crate) fn validate_agent_update_artifact_upload_request(
    request: &UploadAgentUpdateArtifactRequest,
) -> Result<ValidatedArtifactUpload, ApiError> {
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
            "agent_update_artifact_confirmation_required",
        ));
    }
    if !is_hex_len(&request.artifact_signature_hex, 128) {
        return Err(ApiError::bad_request(
            "agent_update_release_signature_invalid",
        ));
    }
    if !is_hex_len(&request.artifact_signing_key_hex, 64) {
        return Err(ApiError::bad_request(
            "agent_update_release_signing_key_invalid",
        ));
    }
    let primary = validate_base64_release_artifact(
        &request.artifact_base64,
        &request.artifact_signature_hex,
        &request.artifact_signing_key_hex,
        "agent_update_artifact_base64_size_invalid",
        "agent_update_artifact_base64_invalid",
        "agent_update_release_size_bytes_invalid",
        "agent_update_release_signature_mismatch",
    )?;
    let rollback = validate_optional_rollback_upload(request)?;
    if let Some(notes) = request.notes.as_deref() {
        if notes.len() > MAX_RELEASE_NOTES_BYTES || notes.as_bytes().contains(&0) {
            return Err(ApiError::bad_request("agent_update_release_notes_invalid"));
        }
    }
    Ok(ValidatedArtifactUpload { primary, rollback })
}

fn validate_hosted_agent_update_release_request(
    state: &AppState,
    request: &CreateHostedAgentUpdateReleaseRequest,
) -> Result<UploadedAgentUpdateReleaseMetadata, ApiError> {
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
            "agent_update_hosted_release_confirmation_required",
        ));
    }
    if !is_hex_len(&request.artifact_sha256_hex, 64) {
        return Err(ApiError::bad_request("agent_update_release_sha256_invalid"));
    }
    validate_artifact_signature_metadata(
        &request.artifact_signature_hex,
        &request.artifact_signing_key_hex,
        "agent_update_release_signature_invalid",
        "agent_update_release_signing_key_invalid",
    )?;
    if !verify_update_artifact_signature(
        &request.artifact_signing_key_hex,
        &request.artifact_signature_hex,
        &request.artifact_sha256_hex.to_ascii_lowercase(),
    ) {
        return Err(ApiError::bad_request(
            "agent_update_release_signature_mismatch",
        ));
    }
    validate_optional_hosted_rollback(request)?;
    if let Some(notes) = request.notes.as_deref() {
        if notes.len() > MAX_RELEASE_NOTES_BYTES || notes.as_bytes().contains(&0) {
            return Err(ApiError::bad_request("agent_update_release_notes_invalid"));
        }
    }
    validate_release_policy_for_metadata(
        state,
        &request.channel,
        &request.artifact_signing_key_hex,
        request.rollback_artifact_signing_key_hex.as_deref(),
    )?;
    Ok(UploadedAgentUpdateReleaseMetadata {
        name: request.name.clone(),
        version: request.version.clone(),
        channel: request.channel.clone(),
        artifact_signature_hex: request.artifact_signature_hex.clone(),
        artifact_signing_key_hex: request.artifact_signing_key_hex.clone(),
        rollback_artifact_signature_hex: request.rollback_artifact_signature_hex.clone(),
        rollback_artifact_signing_key_hex: request.rollback_artifact_signing_key_hex.clone(),
        notes: request.notes.clone(),
        confirmed: request.confirmed,
        ingestion_mode: "streamed_hosted_reference",
    })
}

fn validate_optional_hosted_rollback(
    request: &CreateHostedAgentUpdateReleaseRequest,
) -> Result<(), ApiError> {
    let has_any = request.rollback_artifact_sha256_hex.is_some()
        || request.rollback_artifact_signature_hex.is_some()
        || request.rollback_artifact_signing_key_hex.is_some();
    if !has_any {
        return Ok(());
    }
    let sha256_hex = request
        .rollback_artifact_sha256_hex
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad_request("agent_update_rollback_release_sha256_required"))?;
    let signature_hex = request
        .rollback_artifact_signature_hex
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad_request("agent_update_rollback_release_signature_required"))?;
    let signing_key_hex = request
        .rollback_artifact_signing_key_hex
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request("agent_update_rollback_release_signing_key_required")
        })?;
    if !is_hex_len(sha256_hex, 64) {
        return Err(ApiError::bad_request(
            "agent_update_rollback_release_sha256_invalid",
        ));
    }
    validate_artifact_signature_metadata(
        signature_hex,
        signing_key_hex,
        "agent_update_rollback_release_signature_invalid",
        "agent_update_rollback_release_signing_key_invalid",
    )?;
    if !verify_update_artifact_signature(signing_key_hex, signature_hex, sha256_hex) {
        return Err(ApiError::bad_request(
            "agent_update_rollback_release_signature_mismatch",
        ));
    }
    Ok(())
}

fn validate_release_policy_for_metadata(
    state: &AppState,
    channel: &str,
    artifact_signing_key_hex: &str,
    rollback_artifact_signing_key_hex: Option<&str>,
) -> Result<(), ApiError> {
    state.update_release_policy.validate_channel(channel)?;
    state.update_release_policy.validate_signing_key(
        artifact_signing_key_hex,
        "agent_update_release_signing_key_untrusted",
    )?;
    if let Some(rollback_artifact_signing_key_hex) = rollback_artifact_signing_key_hex {
        state.update_release_policy.validate_signing_key(
            rollback_artifact_signing_key_hex,
            "agent_update_rollback_release_signing_key_untrusted",
        )?;
    }
    Ok(())
}

fn validate_artifact_signature_metadata(
    artifact_signature_hex: &str,
    artifact_signing_key_hex: &str,
    signature_code: &'static str,
    signing_key_code: &'static str,
) -> Result<(), ApiError> {
    if !is_hex_len(artifact_signature_hex, 128) {
        return Err(ApiError::bad_request(signature_code));
    }
    if !is_hex_len(artifact_signing_key_hex, 64) {
        return Err(ApiError::bad_request(signing_key_code));
    }
    Ok(())
}

fn validate_rollback_release_metadata(
    sha256_hex: Option<&str>,
    signature_hex: Option<&str>,
    signing_key_hex: Option<&str>,
    artifact_url: Option<&str>,
    size_bytes: Option<i64>,
) -> Result<(), ApiError> {
    let has_any = sha256_hex.is_some()
        || signature_hex.is_some()
        || signing_key_hex.is_some()
        || artifact_url.is_some()
        || size_bytes.is_some();
    if !has_any {
        return Ok(());
    }
    let sha256_hex = sha256_hex
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad_request("agent_update_rollback_release_sha256_required"))?;
    let signature_hex = signature_hex
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad_request("agent_update_rollback_release_signature_required"))?;
    let signing_key_hex = signing_key_hex
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request("agent_update_rollback_release_signing_key_required")
        })?;
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
    if !is_hex_len(signature_hex, 128) {
        return Err(ApiError::bad_request(
            "agent_update_rollback_release_signature_invalid",
        ));
    }
    if !is_hex_len(signing_key_hex, 64) {
        return Err(ApiError::bad_request(
            "agent_update_rollback_release_signing_key_invalid",
        ));
    }
    if !verify_update_artifact_signature(signing_key_hex, signature_hex, sha256_hex) {
        return Err(ApiError::bad_request(
            "agent_update_rollback_release_signature_mismatch",
        ));
    }
    validate_release_url(
        artifact_url,
        "agent_update_rollback_release_artifact_url_invalid",
    )?;
    if let Some(size_bytes) = size_bytes {
        if size_bytes <= 0 || size_bytes > MAX_RELEASE_ARTIFACT_BYTES {
            return Err(ApiError::bad_request(
                "agent_update_rollback_release_size_bytes_invalid",
            ));
        }
    }
    Ok(())
}

fn validate_optional_rollback_upload(
    request: &UploadAgentUpdateArtifactRequest,
) -> Result<Option<ValidatedArtifactBytes>, ApiError> {
    let has_any = request.rollback_artifact_base64.is_some()
        || request.rollback_artifact_signature_hex.is_some()
        || request.rollback_artifact_signing_key_hex.is_some();
    if !has_any {
        return Ok(None);
    }
    let artifact_base64 = request
        .rollback_artifact_base64
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad_request("agent_update_rollback_artifact_base64_required"))?;
    let signature_hex = request
        .rollback_artifact_signature_hex
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request("agent_update_rollback_artifact_signature_required")
        })?;
    let signing_key_hex = request
        .rollback_artifact_signing_key_hex
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request("agent_update_rollback_artifact_signing_key_required")
        })?;
    if !is_hex_len(signature_hex, 128) {
        return Err(ApiError::bad_request(
            "agent_update_rollback_artifact_signature_invalid",
        ));
    }
    if !is_hex_len(signing_key_hex, 64) {
        return Err(ApiError::bad_request(
            "agent_update_rollback_artifact_signing_key_invalid",
        ));
    }
    validate_base64_release_artifact(
        artifact_base64,
        signature_hex,
        signing_key_hex,
        "agent_update_rollback_artifact_base64_size_invalid",
        "agent_update_rollback_artifact_base64_invalid",
        "agent_update_rollback_artifact_size_bytes_invalid",
        "agent_update_rollback_artifact_signature_mismatch",
    )
    .map(Some)
}

fn validate_release_token(
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

fn default_latest_channel() -> String {
    "stable".to_string()
}
