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

use crate::{
    error::ApiError,
    model::JobOutputView,
    model_file_transfer::{
        FileTransferHandoffRequest, FileTransferHandoffView, FileTransferSessionView,
        FileTransferSourceArtifactView, UploadFileTransferSourceArtifactRequest,
    },
    object_store::BackupObjectStore,
    repository_file_transfer_sources::file_transfer_source_artifact_object_key,
    repository_file_transfers::{
        file_transfer_handoff_download_path, file_transfer_handoff_object_key,
    },
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
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
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
    Ok(Json(
        state
            .repo
            .list_file_transfer_sessions(limit_or_default(query.limit), client_id, query.session_id)
            .await?,
    ))
}

pub(crate) async fn list_file_transfer_source_artifacts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<crate::model::HistoryQuery>,
) -> Result<Json<Vec<FileTransferSourceArtifactView>>, ApiError> {
    let _operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
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
    let object_key = file_transfer_source_artifact_object_key(&sha256_hex);
    match store.put_new(&object_key, &bytes).await {
        Ok(()) => {}
        Err(error) if error.to_string().contains("object already exists") => {
            let existing = store.get(&object_key).await?;
            if existing.len() != bytes.len() || hex::encode(Sha256::digest(&existing)) != sha256_hex
            {
                return Err(ApiError::conflict(
                    "file_transfer_source_existing_object_mismatch",
                ));
            }
        }
        Err(error) => return Err(ApiError::from(error)),
    }
    let artifact = state
        .repo
        .record_file_transfer_source_artifact(
            name,
            object_key,
            sha256_hex,
            bytes.len() as i64,
            &operator,
        )
        .await?;
    Ok((axum::http::StatusCode::CREATED, Json(artifact)))
}

pub(crate) async fn download_file_transfer_source_artifact(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(artifact_id): Path<Uuid>,
) -> Result<Response<Body>, ApiError> {
    let _operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
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
    verified_object_artifact_response(
        store,
        &artifact.object_key,
        &artifact.name,
        &artifact.sha256_hex,
        artifact.size_bytes,
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
    let temp_path = std::env::temp_dir().join(format!(
        "vpsman-transfer-handoff-{session_id}-{}.tmp",
        Uuid::new_v4()
    ));
    let chunk_count = write_handoff_temp_file(
        &state, &client_id, session_id, &temp_path, sha256_hex, size_bytes,
    )
    .await?;
    match store
        .put_file_idempotent(&object_key, &temp_path, sha256_hex, size_bytes as u64)
        .await
    {
        Ok(_) => {
            let _ = tokio::fs::remove_file(&temp_path).await;
        }
        Err(error) => {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(ApiError::from(error));
        }
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
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
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
    codes: ArtifactResponseCodes,
) -> Result<Response<Body>, ApiError> {
    let expected_size_u64 =
        u64::try_from(expected_size_bytes).map_err(|_| ApiError::conflict(codes.integrity))?;
    match store
        .verified_filesystem_path(object_key, expected_sha256_hex, expected_size_u64)
        .await
    {
        Ok(Some(path)) => {
            let body = streaming_file_body(path, codes.not_found).await?;
            return artifact_response(
                body,
                filename_source,
                expected_sha256_hex,
                expected_size_u64,
                "streamed-filesystem",
                &codes,
            );
        }
        Ok(None) => {}
        Err(error) if error.to_string().contains("failed to stat object") => {
            return Err(ApiError::not_found(codes.not_found));
        }
        Err(_) => return Err(ApiError::conflict(codes.integrity)),
    }
    let bytes = store
        .get(object_key)
        .await
        .map_err(|_| ApiError::not_found(codes.not_found))?;
    if bytes.len() as i64 != expected_size_bytes
        || hex::encode(Sha256::digest(&bytes)) != expected_sha256_hex
    {
        return Err(ApiError::conflict(codes.integrity));
    }
    artifact_response(
        Body::from(bytes),
        filename_source,
        expected_sha256_hex,
        expected_size_u64,
        "buffered-object-store",
        &codes,
    )
}

async fn streaming_file_body(
    path: std::path::PathBuf,
    not_found_code: &'static str,
) -> Result<Body, ApiError> {
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| ApiError::not_found(not_found_code))?;
    let stream = stream::try_unfold(
        (file, vec![0_u8; ARTIFACT_STREAM_CHUNK_BYTES]),
        |(mut file, mut buffer)| async move {
            let read = file.read(&mut buffer).await?;
            if read == 0 {
                return Ok::<_, std::io::Error>(None);
            }
            let bytes = Bytes::copy_from_slice(&buffer[..read]);
            Ok(Some((bytes, (file, buffer))))
        },
    );
    Ok(Body::from_stream(stream))
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
    let sessions = state
        .repo
        .list_file_transfer_sessions(1, Some(client_id), Some(session_id))
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
    if chunks.is_empty() {
        return Err(ApiError::conflict("file_transfer_handoff_chunks_missing"));
    }
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp_path)
        .await
        .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
    let mut hasher = Sha256::new();
    let mut next_offset = 0_i64;
    for chunk in &chunks {
        if chunk.offset != next_offset {
            return Err(ApiError::conflict("file_transfer_handoff_chunk_gap"));
        }
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
        file.write_all(&bytes)
            .await
            .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
        hasher.update(&bytes);
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
            let data = store.get(object_key).await?;
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
