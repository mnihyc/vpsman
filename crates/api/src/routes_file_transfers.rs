use std::path::PathBuf;

use axum::{
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, Response},
    Json,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use futures_util::stream;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;
use vpsman_common::{create_private_file_new_async, ensure_private_dir};

use crate::{
    error::ApiError,
    model::{JobOutputView, NewServerArtifact},
    model_file_transfer::{
        FileTransferHandoffRequest, FileTransferHandoffView, FileTransferSessionView,
        FileTransferSourceArtifactView, UploadFileTransferSourceArtifactRequest,
    },
    object_store::{BackupObjectStore, VerifiedObjectFile},
    repository_file_transfer_sources::{
        file_transfer_source_artifact_download_path, file_transfer_source_artifact_object_key,
        file_transfer_source_server_artifact,
    },
    repository_file_transfers::{
        file_transfer_handoff_download_path, file_transfer_handoff_object_key,
    },
    security::SCOPE_JOBS_READ,
    state::AppState,
    util::limit_or_default,
};

pub(crate) const MAX_FILE_TRANSFER_SOURCE_UPLOAD_BODY_BYTES: usize = 24 * 1024 * 1024;
const MAX_FILE_TRANSFER_SOURCE_UPLOAD_BYTES: usize = 16 * 1024 * 1024;
const MAX_FILE_TRANSFER_SOURCE_NAME_BYTES: usize = 160;
const ARTIFACT_STREAM_CHUNK_BYTES: usize = 64 * 1024;

struct ArtifactResponseCodes {
    not_found: &'static str,
    integrity: &'static str,
    filename_invalid: &'static str,
    hash_invalid: &'static str,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FileTransferSessionQuery {
    pub(crate) limit: Option<i64>,
    pub(crate) client_id: Option<String>,
    pub(crate) session_id: Option<Uuid>,
}

pub(crate) async fn list_file_transfer_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FileTransferSessionQuery>,
) -> Result<Json<Vec<FileTransferSessionView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_JOBS_READ)
        .await?;
    let client_id = query
        .client_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(client_id) = client_id {
        if client_id.len() > 128 {
            return Err(ApiError::bad_request("file_transfer_client_id_too_long"));
        }
    }
    let mut sessions = state
        .repo
        .list_file_transfer_sessions(limit_or_default(query.limit), client_id, query.session_id)
        .await?;
    state
        .repo
        .annotate_file_transfer_handoff_evidence(&mut sessions)
        .await?;
    Ok(Json(sessions))
}

pub(crate) async fn list_file_transfer_source_artifacts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<crate::model::HistoryQuery>,
) -> Result<Json<Vec<FileTransferSourceArtifactView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_JOBS_READ)
        .await?;
    Ok(Json(
        state
            .repo
            .list_file_transfer_source_artifacts(limit_or_default(query.limit))
            .await?,
    ))
}

pub(crate) async fn upload_file_transfer_source_artifact(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UploadFileTransferSourceArtifactRequest>,
) -> Result<(axum::http::StatusCode, Json<FileTransferSourceArtifactView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    let store = state
        .backup_object_store
        .as_ref()
        .ok_or_else(|| ApiError::conflict("file_transfer_source_object_store_not_configured"))?;
    validate_file_transfer_source_upload_request(&request)?;
    let bytes = BASE64
        .decode(request.source_base64.trim())
        .map_err(|_| ApiError::bad_request("file_transfer_source_base64_invalid"))?;
    if bytes.len() > MAX_FILE_TRANSFER_SOURCE_UPLOAD_BYTES {
        return Err(ApiError::bad_request("file_transfer_source_too_large"));
    }
    if i64::try_from(bytes.len()).ok() != Some(request.size_bytes) {
        return Err(ApiError::bad_request("file_transfer_source_size_mismatch"));
    }
    let sha256_hex = hex::encode(Sha256::digest(&bytes));
    if sha256_hex != request.sha256_hex.to_ascii_lowercase() {
        return Err(ApiError::bad_request("file_transfer_source_hash_mismatch"));
    }
    let name = safe_source_artifact_name(request.name.as_deref());
    let artifact_id = Uuid::new_v4();
    let object_key = file_transfer_source_artifact_object_key(artifact_id, &sha256_hex);
    let reserved_source = FileTransferSourceArtifactView {
        id: artifact_id,
        name: name.clone(),
        object_key: object_key.clone(),
        sha256_hex: sha256_hex.clone(),
        size_bytes: bytes.len() as i64,
        status: "creating".to_string(),
        created_by: Some(operator.operator.id),
        created_at: crate::unix_now().to_string(),
        download_path: file_transfer_source_artifact_download_path(artifact_id),
    };
    reserve_file_transfer_artifact(
        &state,
        file_transfer_source_server_artifact(&reserved_source),
        "file_transfer_source_object_exists",
    )
    .await?;
    if let Err(error) = store.put_new(&object_key, &bytes).await {
        release_file_transfer_artifact_reservation(&state, &object_key).await;
        return Err(ApiError::from(error));
    }
    let artifact = match state
        .repo
        .record_file_transfer_source_artifact(
            name,
            artifact_id,
            object_key.clone(),
            sha256_hex,
            bytes.len() as i64,
            &operator,
        )
        .await
    {
        Ok(artifact) => artifact,
        Err(error) => {
            cleanup_file_transfer_reserved_object_after_error(
                &state,
                store,
                &object_key,
                &error.to_string(),
                true,
            )
            .await;
            return Err(ApiError::from(error));
        }
    };
    Ok((axum::http::StatusCode::CREATED, Json(artifact)))
}

pub(crate) async fn download_file_transfer_source_artifact(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(artifact_id): Path<Uuid>,
) -> Result<Response<Body>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_JOBS_READ)
        .await?;
    let store = state
        .backup_object_store
        .as_ref()
        .ok_or_else(|| ApiError::not_found("file_transfer_source_object_store_not_configured"))?;
    let artifact = state
        .repo
        .get_file_transfer_source_artifact(artifact_id)
        .await?
        .ok_or_else(|| ApiError::not_found("file_transfer_source_artifact_not_found"))?;
    if artifact.status == "deleting" || artifact.status == "creating" {
        return Err(ApiError::conflict(
            "file_transfer_source_artifact_delete_in_progress",
        ));
    }
    verified_object_artifact_response(
        store,
        &artifact.object_key,
        &artifact.name,
        &artifact.sha256_hex,
        artifact.size_bytes,
        state.artifact_max_bytes(),
        ArtifactResponseCodes {
            not_found: "file_transfer_source_artifact_not_found",
            integrity: "file_transfer_source_artifact_integrity_mismatch",
            filename_invalid: "file_transfer_source_filename_invalid",
            hash_invalid: "file_transfer_source_hash_invalid",
        },
    )
    .await
}

pub(crate) async fn create_file_transfer_handoff(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((client_id, session_id)): Path<(String, Uuid)>,
    Json(request): Json<FileTransferHandoffRequest>,
) -> Result<Json<FileTransferHandoffView>, ApiError> {
    let _operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    if !request.confirmed {
        return Err(ApiError::conflict(
            "file_transfer_handoff_confirmation_required",
        ));
    }
    validate_handoff_client_id(&client_id)?;
    let store = state
        .backup_object_store
        .as_ref()
        .ok_or_else(|| ApiError::conflict("file_transfer_handoff_object_store_not_configured"))?;
    let session = completed_download_session(&state, &client_id, session_id).await?;
    let sha256_hex = session
        .sha256_hex
        .as_deref()
        .ok_or_else(|| ApiError::conflict("file_transfer_handoff_hash_missing"))?;
    let size_bytes = session
        .size_bytes
        .ok_or_else(|| ApiError::conflict("file_transfer_handoff_size_missing"))?;
    let object_key = file_transfer_handoff_object_key(&client_id, session_id, sha256_hex);
    if state
        .repo
        .active_server_artifact_matches(
            "file_transfer_handoff",
            &object_key,
            sha256_hex,
            size_bytes,
        )
        .await?
    {
        return Ok(Json(FileTransferHandoffView {
            client_id,
            session_id,
            object_key,
            sha256_hex: sha256_hex.to_string(),
            size_bytes,
            chunk_count: 0,
            source: "existing_handoff_artifact".to_string(),
            download_path: file_transfer_handoff_download_path(
                &session.client_id,
                session.session_id,
            ),
        }));
    }
    if !session.handoff_available {
        return Err(ApiError::conflict(file_transfer_handoff_unavailable_code(
            &session,
        )));
    }
    let handoff_artifact = NewServerArtifact {
        domain: "file_transfer_handoff".to_string(),
        object_key: object_key.clone(),
        sha256_hex: sha256_hex.to_string(),
        size_bytes,
        job_id: Some(session.last_job_id),
        client_id: Some(client_id.clone()),
        stream: None,
        seq: None,
        backup_request_id: None,
        backup_artifact_id: None,
        release_id: None,
        metadata: serde_json::json!({
            "session_id": session_id,
            "path": &session.path,
        }),
    };
    reserve_file_transfer_artifact(
        &state,
        handoff_artifact.clone(),
        "file_transfer_handoff_object_exists",
    )
    .await?;
    let temp_path = file_transfer_handoff_temp_path(session_id)?;
    let chunk_count = match write_handoff_temp_file(
        &state, &client_id, session_id, &temp_path, sha256_hex, size_bytes,
    )
    .await
    {
        Ok(chunk_count) => chunk_count,
        Err(error) => {
            release_file_transfer_artifact_reservation(&state, &object_key).await;
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(error);
        }
    };
    let created_object = match store
        .put_file_idempotent(&object_key, &temp_path, sha256_hex, size_bytes as u64)
        .await
    {
        Ok(created_object) => {
            let _ = tokio::fs::remove_file(&temp_path).await;
            created_object
        }
        Err(error) => {
            release_file_transfer_artifact_reservation(&state, &object_key).await;
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(ApiError::from(error));
        }
    };
    let handoff_artifact = NewServerArtifact {
        metadata: serde_json::json!({
            "session_id": session_id,
            "path": &session.path,
            "chunk_count": chunk_count,
        }),
        ..handoff_artifact
    };
    if let Err(error) = state.repo.register_server_artifact(handoff_artifact).await {
        cleanup_file_transfer_reserved_object_after_error(
            &state,
            store,
            &object_key,
            &error.to_string(),
            created_object,
        )
        .await;
        return Err(ApiError::from(error));
    }
    Ok(Json(FileTransferHandoffView {
        client_id,
        session_id,
        object_key,
        sha256_hex: sha256_hex.to_string(),
        size_bytes,
        chunk_count,
        source: "job_outputs".to_string(),
        download_path: file_transfer_handoff_download_path(&session.client_id, session.session_id),
    }))
}

pub(crate) async fn download_file_transfer_handoff(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((client_id, session_id)): Path<(String, Uuid)>,
) -> Result<Response<Body>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_JOBS_READ)
        .await?;
    validate_handoff_client_id(&client_id)?;
    let store = state
        .backup_object_store
        .as_ref()
        .ok_or_else(|| ApiError::not_found("file_transfer_handoff_not_available"))?;
    let session = completed_download_session(&state, &client_id, session_id).await?;
    let sha256_hex = session
        .sha256_hex
        .as_deref()
        .ok_or_else(|| ApiError::conflict("file_transfer_handoff_hash_missing"))?;
    let size_bytes = session
        .size_bytes
        .ok_or_else(|| ApiError::conflict("file_transfer_handoff_size_missing"))?;
    let object_key = file_transfer_handoff_object_key(&client_id, session_id, sha256_hex);
    verified_object_artifact_response(
        store,
        &object_key,
        &session.path,
        sha256_hex,
        size_bytes,
        state.artifact_max_bytes(),
        ArtifactResponseCodes {
            not_found: "file_transfer_handoff_artifact_not_found",
            integrity: "file_transfer_handoff_integrity_mismatch",
            filename_invalid: "file_transfer_handoff_filename_invalid",
            hash_invalid: "file_transfer_handoff_hash_invalid",
        },
    )
    .await
}

async fn verified_object_artifact_response(
    store: &BackupObjectStore,
    object_key: &str,
    filename_source: &str,
    expected_sha256_hex: &str,
    expected_size_bytes: i64,
    artifact_max_bytes: usize,
    codes: ArtifactResponseCodes,
) -> Result<Response<Body>, ApiError> {
    let expected_size_u64 =
        u64::try_from(expected_size_bytes).map_err(|_| ApiError::conflict(codes.integrity))?;
    let object_file = store
        .verified_object_file(
            object_key,
            expected_sha256_hex,
            expected_size_u64,
            artifact_max_bytes,
        )
        .await
        .map_err(|error| map_verified_object_error(error, codes.not_found, codes.integrity))?;
    let delivery = verified_object_delivery(&object_file);
    let body = streaming_artifact_file_body(
        object_file.path,
        codes.not_found,
        object_file.cleanup_after_stream,
    )
    .await?;
    artifact_response(
        body,
        filename_source,
        expected_sha256_hex,
        expected_size_u64,
        delivery,
        &codes,
    )
}

pub(crate) async fn streaming_artifact_file_body(
    path: PathBuf,
    not_found_code: &'static str,
    cleanup_after_stream: bool,
) -> Result<Body, ApiError> {
    let file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(_) => {
            if cleanup_after_stream {
                let _ = tokio::fs::remove_file(&path).await;
            }
            return Err(ApiError::not_found(not_found_code));
        }
    };
    let stream = stream::try_unfold(
        StreamingArtifactFile {
            file,
            buffer: vec![0_u8; ARTIFACT_STREAM_CHUNK_BYTES],
            cleanup_path: cleanup_after_stream.then_some(path),
        },
        |mut state| async move {
            let read = state.file.read(&mut state.buffer).await?;
            if read == 0 {
                return Ok::<_, std::io::Error>(None);
            }
            let bytes = Bytes::copy_from_slice(&state.buffer[..read]);
            Ok(Some((bytes, state)))
        },
    );
    Ok(Body::from_stream(stream))
}

pub(crate) fn map_verified_object_error(
    error: anyhow::Error,
    not_found_code: &'static str,
    integrity_code: &'static str,
) -> ApiError {
    let message = error.to_string();
    if message.contains("failed to stat object")
        || message.contains("S3 get object failed with HTTP 404")
    {
        ApiError::not_found(not_found_code)
    } else {
        ApiError::conflict(integrity_code)
    }
}

fn verified_object_delivery(object_file: &VerifiedObjectFile) -> &'static str {
    if object_file.cleanup_after_stream {
        "streamed-s3-spool"
    } else {
        "streamed-filesystem"
    }
}

struct StreamingArtifactFile {
    file: tokio::fs::File,
    buffer: Vec<u8>,
    cleanup_path: Option<PathBuf>,
}

impl Drop for StreamingArtifactFile {
    fn drop(&mut self) {
        if let Some(path) = self.cleanup_path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn artifact_response(
    body: Body,
    filename_source: &str,
    sha256_hex: &str,
    size_bytes: u64,
    delivery: &'static str,
    codes: &ArtifactResponseCodes,
) -> Result<Response<Body>, ApiError> {
    let mut response = Response::new(body);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"{}\"",
            safe_handoff_filename(filename_source)
        ))
        .map_err(|_| ApiError::conflict(codes.filename_invalid))?,
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&size_bytes.to_string())
            .map_err(|_| ApiError::conflict(codes.integrity))?,
    );
    response.headers_mut().insert(
        "x-vpsman-artifact-sha256",
        HeaderValue::from_str(sha256_hex).map_err(|_| ApiError::conflict(codes.hash_invalid))?,
    );
    response.headers_mut().insert(
        "x-vpsman-artifact-delivery",
        HeaderValue::from_static(delivery),
    );
    Ok(response)
}

async fn completed_download_session(
    state: &AppState,
    client_id: &str,
    session_id: Uuid,
) -> Result<FileTransferSessionView, ApiError> {
    let mut sessions = state
        .repo
        .list_file_transfer_sessions(1, Some(client_id), Some(session_id))
        .await?;
    state
        .repo
        .annotate_file_transfer_handoff_evidence(&mut sessions)
        .await?;
    let session = sessions
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::not_found("file_transfer_session_not_found"))?;
    if session.direction != "download" {
        return Err(ApiError::conflict(
            "file_transfer_handoff_requires_download",
        ));
    }
    if session.status != "completed" {
        return Err(ApiError::conflict(
            "file_transfer_handoff_requires_completed_session",
        ));
    }
    Ok(session)
}

fn file_transfer_handoff_unavailable_code(session: &FileTransferSessionView) -> &'static str {
    match (
        session.handoff_evidence_status.as_str(),
        session.handoff_unavailable_reason.as_deref(),
    ) {
        ("retained_outputs_conflict", _) | (_, Some("duplicate_offset_conflict")) => {
            "file_transfer_handoff_chunk_offset_conflict"
        }
        ("retained_outputs_pruned", _) | (_, Some("retained_chunk_outputs_pruned")) => {
            "file_transfer_handoff_chunks_missing"
        }
        ("retained_outputs_incomplete", _) => "file_transfer_handoff_evidence_incomplete",
        _ => "file_transfer_handoff_evidence_unavailable",
    }
}

async fn write_handoff_temp_file(
    state: &AppState,
    client_id: &str,
    session_id: Uuid,
    temp_path: &std::path::Path,
    expected_sha256_hex: &str,
    expected_size_bytes: i64,
) -> Result<usize, ApiError> {
    let chunks = state
        .repo
        .list_file_transfer_download_handoff_chunks(client_id, session_id)
        .await?;
    if chunks.is_empty() && expected_size_bytes != 0 {
        return Err(ApiError::conflict("file_transfer_handoff_chunks_missing"));
    }
    let chunks = select_valid_handoff_chunks(state, chunks).await?;
    let mut file = create_private_file_new_async(temp_path)
        .await
        .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
    let mut hasher = Sha256::new();
    let mut next_offset = 0_i64;
    for chunk in &chunks {
        if chunk.offset != next_offset {
            return Err(ApiError::conflict("file_transfer_handoff_chunk_gap"));
        }
        file.write_all(&chunk.bytes)
            .await
            .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
        hasher.update(&chunk.bytes);
        next_offset = next_offset.saturating_add(chunk.size_bytes);
    }
    file.sync_data()
        .await
        .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
    drop(file);
    if next_offset != expected_size_bytes {
        return Err(ApiError::conflict("file_transfer_handoff_size_mismatch"));
    }
    if hex::encode(hasher.finalize()) != expected_sha256_hex {
        return Err(ApiError::conflict("file_transfer_handoff_hash_mismatch"));
    }
    Ok(chunks.len())
}

struct SelectedHandoffChunk {
    offset: i64,
    size_bytes: i64,
    bytes: Vec<u8>,
}

async fn select_valid_handoff_chunks(
    state: &AppState,
    chunks: Vec<crate::repository_file_transfers::FileTransferDownloadHandoffChunk>,
) -> Result<Vec<SelectedHandoffChunk>, ApiError> {
    let mut by_offset: std::collections::BTreeMap<
        i64,
        Vec<crate::repository_file_transfers::FileTransferDownloadHandoffChunk>,
    > = std::collections::BTreeMap::new();
    for chunk in chunks {
        by_offset.entry(chunk.offset).or_default().push(chunk);
    }
    let mut selected = Vec::new();
    for (_offset, candidates) in by_offset {
        let first = candidates
            .first()
            .ok_or_else(|| ApiError::conflict("file_transfer_handoff_chunk_unavailable"))?;
        if candidates.iter().any(|candidate| {
            candidate.size_bytes != first.size_bytes || candidate.sha256_hex != first.sha256_hex
        }) {
            return Err(ApiError::conflict(
                "file_transfer_handoff_chunk_offset_conflict",
            ));
        }
        let mut last_error = None;
        for candidate in candidates {
            match validate_handoff_chunk_candidate(state, candidate).await {
                Ok(chunk) => {
                    selected.push(chunk);
                    last_error = None;
                    break;
                }
                Err(error) => {
                    last_error = Some(error);
                }
            }
        }
        if let Some(error) = last_error {
            return Err(error);
        }
    }
    Ok(selected)
}

async fn validate_handoff_chunk_candidate(
    state: &AppState,
    chunk: crate::repository_file_transfers::FileTransferDownloadHandoffChunk,
) -> Result<SelectedHandoffChunk, ApiError> {
    let bytes = load_handoff_chunk_bytes(state, &chunk.outputs, chunk.size_bytes).await?;
    if bytes.len() as i64 != chunk.size_bytes {
        return Err(ApiError::conflict(
            "file_transfer_handoff_chunk_size_mismatch",
        ));
    }
    if hex::encode(Sha256::digest(&bytes)) != chunk.sha256_hex {
        return Err(ApiError::conflict(
            "file_transfer_handoff_chunk_hash_mismatch",
        ));
    }
    Ok(SelectedHandoffChunk {
        offset: chunk.offset,
        size_bytes: chunk.size_bytes,
        bytes,
    })
}

async fn load_handoff_chunk_bytes(
    state: &AppState,
    outputs: &[JobOutputView],
    expected_size_bytes: i64,
) -> Result<Vec<u8>, ApiError> {
    let mut bytes = Vec::new();
    for output in outputs {
        let part = if output.storage == "object_store" {
            let store = state.backup_object_store.as_ref().ok_or_else(|| {
                ApiError::conflict("file_transfer_handoff_object_store_not_configured")
            })?;
            let object_key = output.artifact_object_key.as_deref().ok_or_else(|| {
                ApiError::conflict("file_transfer_handoff_output_artifact_missing")
            })?;
            let data = store
                .get_with_limit(object_key, state.artifact_max_bytes())
                .await?;
            if let Some(expected_hash) = output.artifact_sha256_hex.as_deref() {
                if hex::encode(Sha256::digest(&data)) != expected_hash {
                    return Err(ApiError::conflict(
                        "file_transfer_handoff_output_artifact_hash_mismatch",
                    ));
                }
            }
            if let Some(expected_size) = output.artifact_size_bytes {
                if data.len() as i64 != expected_size {
                    return Err(ApiError::conflict(
                        "file_transfer_handoff_output_artifact_size_mismatch",
                    ));
                }
            }
            data
        } else {
            BASE64
                .decode(&output.data_base64)
                .map_err(|_| ApiError::conflict("file_transfer_handoff_inline_output_invalid"))?
        };
        bytes.extend_from_slice(&part);
        if bytes.len() as i64 > expected_size_bytes {
            return Err(ApiError::conflict(
                "file_transfer_handoff_chunk_size_mismatch",
            ));
        }
    }
    Ok(bytes)
}

fn validate_handoff_client_id(client_id: &str) -> Result<(), ApiError> {
    if client_id.trim().is_empty() || client_id.len() > 128 || client_id.contains('/') {
        return Err(ApiError::bad_request("file_transfer_client_id_invalid"));
    }
    Ok(())
}

fn file_transfer_handoff_temp_path(session_id: Uuid) -> Result<PathBuf, ApiError> {
    let root = std::env::temp_dir().join("vpsman-transfer-handoff");
    ensure_private_dir(&root).map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
    Ok(root.join(format!(
        "vpsman-transfer-handoff-{session_id}-{}.tmp",
        Uuid::new_v4()
    )))
}

fn validate_file_transfer_source_upload_request(
    request: &UploadFileTransferSourceArtifactRequest,
) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::conflict(
            "file_transfer_source_confirmation_required",
        ));
    }
    if let Some(name) = request.name.as_deref() {
        let name = name.trim();
        if name.len() > MAX_FILE_TRANSFER_SOURCE_NAME_BYTES {
            return Err(ApiError::bad_request("file_transfer_source_name_too_long"));
        }
    }
    if request.size_bytes < 0 || request.size_bytes as usize > MAX_FILE_TRANSFER_SOURCE_UPLOAD_BYTES
    {
        return Err(ApiError::bad_request("file_transfer_source_size_invalid"));
    }
    if !is_lower_or_upper_hex_len(&request.sha256_hex, 64) {
        return Err(ApiError::bad_request("file_transfer_source_sha256_invalid"));
    }
    let max_base64_len = MAX_FILE_TRANSFER_SOURCE_UPLOAD_BYTES.div_ceil(3) * 4 + 16;
    if request.source_base64.len() > max_base64_len {
        return Err(ApiError::bad_request(
            "file_transfer_source_base64_too_large",
        ));
    }
    Ok(())
}

async fn reserve_file_transfer_artifact(
    state: &AppState,
    artifact: NewServerArtifact,
    conflict_code: &'static str,
) -> Result<(), ApiError> {
    state
        .repo
        .reserve_server_artifact(artifact)
        .await
        .map_err(|error| {
            if error
                .to_string()
                .contains("server_artifact_object_key_conflict")
            {
                ApiError::conflict(conflict_code)
            } else {
                ApiError::from(error)
            }
        })
}

async fn release_file_transfer_artifact_reservation(state: &AppState, object_key: &str) {
    let _ = state
        .repo
        .discard_server_artifact_reservation(object_key)
        .await;
}

async fn cleanup_file_transfer_reserved_object_after_error(
    state: &AppState,
    store: &BackupObjectStore,
    object_key: &str,
    error: &str,
    created_object: bool,
) {
    if created_object {
        match store.delete_confirmed(object_key).await {
            Ok(()) => {
                let _ = state
                    .repo
                    .discard_server_artifact_reservation(object_key)
                    .await;
            }
            Err(delete_error) => {
                let _ = state
                    .repo
                    .mark_server_artifact_delete_failed(
                        object_key,
                        &format!("{error}; cleanup_delete_failed: {delete_error}"),
                    )
                    .await;
            }
        }
    } else {
        let _ = state
            .repo
            .discard_server_artifact_reservation(object_key)
            .await;
    }
}

fn is_lower_or_upper_hex_len(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn safe_source_artifact_name(name: Option<&str>) -> String {
    let value = name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("vpsman-transfer-source.bin");
    let filename = value
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '/' | '\\' | '"') {
                '_'
            } else {
                ch
            }
        })
        .take(MAX_FILE_TRANSFER_SOURCE_NAME_BYTES)
        .collect::<String>();
    if filename.is_empty() {
        "vpsman-transfer-source.bin".to_string()
    } else {
        filename
    }
}

fn safe_handoff_filename(path: &str) -> String {
    let filename = path
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("vpsman-transfer.bin")
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '/' | '\\' | '"') {
                '_'
            } else {
                ch
            }
        })
        .take(160)
        .collect::<String>();
    if filename.is_empty() {
        "vpsman-transfer.bin".to_string()
    } else {
        filename
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn file_transfer_handoff_temp_path_uses_private_directory() {
        let path = file_transfer_handoff_temp_path(Uuid::new_v4()).unwrap();
        let parent = path.parent().unwrap();

        assert_eq!(
            std::fs::metadata(parent).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}
