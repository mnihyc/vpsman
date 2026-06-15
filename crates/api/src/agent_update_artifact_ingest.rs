use std::path::{Path as FsPath, PathBuf};

use axum::{body::Body, http::HeaderMap};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;
use vpsman_common::verify_update_artifact_signature;

use crate::{
    error::ApiError, object_store::BackupObjectStore,
    repository_agent_update_releases::UploadedAgentUpdateArtifactRef,
};

pub(crate) const MAX_RELEASE_ARTIFACT_BYTES: i64 = 16 * 1024 * 1024;

pub(crate) struct ValidatedArtifactBytes {
    pub(crate) sha256_hex: String,
    pub(crate) bytes: Vec<u8>,
}

pub(crate) struct StreamArtifactMetadata {
    pub(crate) artifact_signature_hex: String,
    pub(crate) artifact_signing_key_hex: String,
    pub(crate) confirmed: bool,
}

pub(crate) struct StreamedArtifactFile {
    pub(crate) sha256_hex: String,
    pub(crate) temp_path: PathBuf,
    pub(crate) size_bytes: i64,
}

impl StreamArtifactMetadata {
    pub(crate) fn from_headers(headers: &HeaderMap) -> Result<Self, ApiError> {
        Ok(Self {
            artifact_signature_hex: required_header(
                headers,
                "x-vpsman-artifact-signature-hex",
                "agent_update_artifact_stream_signature_required",
            )?,
            artifact_signing_key_hex: required_header(
                headers,
                "x-vpsman-artifact-signing-key-hex",
                "agent_update_artifact_stream_signing_key_required",
            )?,
            confirmed: optional_header(headers, "x-vpsman-confirmed")?
                .as_deref()
                .map(|value| matches!(value, "true" | "1" | "yes"))
                .unwrap_or(false),
        })
    }
}

pub(crate) async fn store_uploaded_artifact(
    store: &BackupObjectStore,
    artifact: ValidatedArtifactBytes,
) -> Result<UploadedAgentUpdateArtifactRef, ApiError> {
    let object_key = hosted_artifact_object_key(&artifact.sha256_hex);
    let download_path = hosted_artifact_download_path(&artifact.sha256_hex);
    match store.put_new(&object_key, &artifact.bytes).await {
        Ok(()) => {}
        Err(error) if error.to_string().contains("object already exists") => {
            let stored = store
                .get_with_limit(&object_key, artifact.bytes.len())
                .await?;
            let stored_hash = sha256_hex(&stored);
            if stored_hash != artifact.sha256_hex || stored.len() != artifact.bytes.len() {
                return Err(ApiError::conflict(
                    "agent_update_artifact_existing_object_mismatch",
                ));
            }
        }
        Err(error) => return Err(ApiError::from(error)),
    }
    Ok(UploadedAgentUpdateArtifactRef {
        artifact_sha256_hex: artifact.sha256_hex,
        artifact_object_key: object_key,
        artifact_download_path: download_path,
        size_bytes: artifact.bytes.len() as i64,
    })
}

pub(crate) async fn store_uploaded_artifact_file(
    store: &BackupObjectStore,
    artifact: &StreamedArtifactFile,
) -> Result<UploadedAgentUpdateArtifactRef, ApiError> {
    let object_key = hosted_artifact_object_key(&artifact.sha256_hex);
    let download_path = hosted_artifact_download_path(&artifact.sha256_hex);
    store
        .put_file_idempotent(
            &object_key,
            &artifact.temp_path,
            &artifact.sha256_hex,
            artifact.size_bytes as u64,
        )
        .await
        .map_err(ApiError::from)?;
    Ok(UploadedAgentUpdateArtifactRef {
        artifact_sha256_hex: artifact.sha256_hex.clone(),
        artifact_object_key: object_key,
        artifact_download_path: download_path,
        size_bytes: artifact.size_bytes,
    })
}

pub(crate) async fn hosted_uploaded_artifact_ref(
    store: &BackupObjectStore,
    artifact_sha256_hex: &str,
) -> Result<UploadedAgentUpdateArtifactRef, ApiError> {
    let artifact_sha256_hex = artifact_sha256_hex.trim().to_ascii_lowercase();
    if !is_hex_len(&artifact_sha256_hex, 64) {
        return Err(ApiError::bad_request("agent_update_release_sha256_invalid"));
    }
    let object_key = hosted_artifact_object_key(&artifact_sha256_hex);
    let bytes = store
        .get(&object_key)
        .await
        .map_err(|_| ApiError::not_found("agent_update_hosted_artifact_not_found"))?;
    if bytes.is_empty() || bytes.len() > MAX_RELEASE_ARTIFACT_BYTES as usize {
        return Err(ApiError::conflict(
            "agent_update_hosted_artifact_size_invalid",
        ));
    }
    if sha256_hex(&bytes) != artifact_sha256_hex {
        return Err(ApiError::conflict(
            "agent_update_hosted_artifact_integrity_mismatch",
        ));
    }
    Ok(UploadedAgentUpdateArtifactRef {
        artifact_sha256_hex: artifact_sha256_hex.clone(),
        artifact_object_key: object_key,
        artifact_download_path: hosted_artifact_download_path(&artifact_sha256_hex),
        size_bytes: bytes.len() as i64,
    })
}

pub(crate) async fn stream_and_verify_artifact(
    body: Body,
    artifact_signature_hex: &str,
    artifact_signing_key_hex: &str,
) -> Result<StreamedArtifactFile, ApiError> {
    let temp_path =
        std::env::temp_dir().join(format!("vpsman-agent-update-upload-{}.tmp", Uuid::new_v4()));
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .await
        .map_err(anyhow::Error::new)
        .map_err(ApiError::from)?;
    let mut hasher = Sha256::new();
    let mut total_bytes = 0usize;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                cleanup_temp_file(&temp_path).await;
                let _ = error;
                return Err(ApiError::bad_request(
                    "agent_update_artifact_stream_invalid",
                ));
            }
        };
        if chunk.is_empty() {
            continue;
        }
        total_bytes = total_bytes.saturating_add(chunk.len());
        if total_bytes > MAX_RELEASE_ARTIFACT_BYTES as usize {
            cleanup_temp_file(&temp_path).await;
            return Err(ApiError::bad_request(
                "agent_update_artifact_stream_size_invalid",
            ));
        }
        hasher.update(&chunk);
        if let Err(error) = file.write_all(&chunk).await {
            cleanup_temp_file(&temp_path).await;
            return Err(ApiError::from(anyhow::Error::new(error)));
        }
    }
    if total_bytes == 0 {
        cleanup_temp_file(&temp_path).await;
        return Err(ApiError::bad_request(
            "agent_update_artifact_stream_size_invalid",
        ));
    }
    if let Err(error) = file.sync_data().await {
        cleanup_temp_file(&temp_path).await;
        return Err(ApiError::from(anyhow::Error::new(error)));
    }
    drop(file);
    let sha256_hex = hex::encode(hasher.finalize());
    if !verify_update_artifact_signature(
        artifact_signing_key_hex,
        artifact_signature_hex,
        &sha256_hex,
    ) {
        cleanup_temp_file(&temp_path).await;
        return Err(ApiError::bad_request(
            "agent_update_release_signature_mismatch",
        ));
    }
    Ok(StreamedArtifactFile {
        sha256_hex,
        temp_path,
        size_bytes: total_bytes as i64,
    })
}

pub(crate) fn validate_base64_release_artifact(
    artifact_base64: &str,
    signature_hex: &str,
    signing_key_hex: &str,
    base64_size_code: &'static str,
    base64_decode_code: &'static str,
    size_code: &'static str,
    signature_code: &'static str,
) -> Result<ValidatedArtifactBytes, ApiError> {
    let max_base64_len = (MAX_RELEASE_ARTIFACT_BYTES as usize).div_ceil(3) * 4 + 256;
    if artifact_base64.is_empty() || artifact_base64.len() > max_base64_len {
        return Err(ApiError::bad_request(base64_size_code));
    }
    let bytes = BASE64_STANDARD
        .decode(artifact_base64.as_bytes())
        .map_err(|_| ApiError::bad_request(base64_decode_code))?;
    if bytes.is_empty() || bytes.len() > MAX_RELEASE_ARTIFACT_BYTES as usize {
        return Err(ApiError::bad_request(size_code));
    }
    let sha256_hex = sha256_hex(&bytes);
    if !verify_update_artifact_signature(signing_key_hex, signature_hex, &sha256_hex) {
        return Err(ApiError::bad_request(signature_code));
    }
    Ok(ValidatedArtifactBytes { sha256_hex, bytes })
}

pub(crate) async fn cleanup_temp_file(path: &FsPath) {
    let _ = tokio::fs::remove_file(path).await;
}

pub(crate) fn hosted_artifact_object_key(sha256_hex: &str) -> String {
    format!("agent-updates/{sha256_hex}.bin")
}

pub(crate) fn hosted_artifact_download_path(sha256_hex: &str) -> String {
    format!("/api/v1/agent-update-artifacts/{sha256_hex}")
}

pub(crate) fn is_hex_len(value: &str, expected_len: usize) -> bool {
    value.len() == expected_len && value.as_bytes().iter().all(u8::is_ascii_hexdigit)
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn required_header(
    headers: &HeaderMap,
    name: &'static str,
    code: &'static str,
) -> Result<String, ApiError> {
    optional_header(headers, name)?.ok_or_else(|| ApiError::bad_request(code))
}

fn optional_header(headers: &HeaderMap, name: &'static str) -> Result<Option<String>, ApiError> {
    let Some(value) = headers.get(name) else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| ApiError::bad_request("agent_update_artifact_stream_header_invalid"))?
        .trim();
    if value.as_bytes().contains(&0) || value.contains('\r') || value.contains('\n') {
        return Err(ApiError::bad_request(
            "agent_update_artifact_stream_header_invalid",
        ));
    }
    Ok((!value.is_empty()).then(|| value.to_string()))
}
