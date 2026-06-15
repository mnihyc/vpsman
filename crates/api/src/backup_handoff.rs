use std::{env, path::PathBuf};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use vpsman_common::payload_hash;

use crate::{error::ApiError, model::JobOutputView, state::AppState};

const BACKUP_HANDOFF_STAGING_ENV: &str = "VPSMAN_BACKUP_HANDOFF_STAGING_DIR";
const BACKUP_HANDOFF_MAX_BYTES_ENV: &str = "VPSMAN_BACKUP_HANDOFF_MAX_BYTES";
pub(crate) const MAX_BACKUP_ARTIFACT_CHUNKED_UPLOAD_BYTES: usize = 128 * 1024 * 1024;

pub(crate) struct StagedRetainedBackupArtifact {
    pub(crate) staging_path: PathBuf,
    pub(crate) sha256_hex: String,
    pub(crate) size_bytes: i64,
    pub(crate) source_chunk_count: usize,
}

pub(crate) async fn stage_retained_backup_artifact_stdout(
    state: &AppState,
    outputs: &[JobOutputView],
) -> Result<StagedRetainedBackupArtifact, ApiError> {
    let root = backup_artifact_handoff_staging_dir();
    tokio::fs::create_dir_all(&root)
        .await
        .map_err(|_| ApiError::conflict("backup_artifact_handoff_staging_unavailable"))?;
    let staging_path = root.join(format!("{}.part", uuid::Uuid::new_v4()));
    let mut staging = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging_path)
        .await
        .map_err(|_| ApiError::conflict("backup_artifact_handoff_staging_unavailable"))?;
    let mut hasher = Sha256::new();
    let mut size_bytes = 0_u64;
    let mut source_chunk_count = 0_usize;
    let max_bytes = backup_artifact_streaming_max_bytes() as u64;
    for output in outputs {
        if output.stream != "stdout" {
            continue;
        }
        source_chunk_count = source_chunk_count.saturating_add(1);
        if output.storage == "object_store" {
            let store = state
                .backup_object_store
                .as_ref()
                .ok_or_else(|| ApiError::conflict("backup_object_store_not_configured"))?;
            let object_key = output.artifact_object_key.as_deref().ok_or_else(|| {
                ApiError::conflict("backup_artifact_handoff_output_artifact_missing")
            })?;
            if let (Some(expected_hash), Some(expected_size)) = (
                output.artifact_sha256_hex.as_deref(),
                output.artifact_size_bytes,
            ) {
                let expected_size = expected_size.try_into().map_err(|_| {
                    ApiError::conflict("backup_artifact_handoff_output_size_mismatch")
                })?;
                if let Some(path) = store
                    .verified_filesystem_path(object_key, expected_hash, expected_size)
                    .await
                    .map_err(ApiError::from)?
                {
                    append_file_to_backup_handoff_staging(
                        &path,
                        &mut staging,
                        &mut hasher,
                        &mut size_bytes,
                        max_bytes,
                    )
                    .await?;
                    continue;
                }
            }
            let data = store
                .get_with_limit(object_key, backup_artifact_streaming_max_bytes())
                .await
                .map_err(ApiError::from)?;
            validate_backup_handoff_output_part(output, &data)?;
            append_backup_handoff_bytes(
                &data,
                &mut staging,
                &mut hasher,
                &mut size_bytes,
                max_bytes,
            )
            .await?;
        } else {
            let data = BASE64
                .decode(&output.data_base64)
                .map_err(|_| ApiError::conflict("backup_artifact_handoff_inline_output_invalid"))?;
            validate_backup_handoff_output_part(output, &data)?;
            append_backup_handoff_bytes(
                &data,
                &mut staging,
                &mut hasher,
                &mut size_bytes,
                max_bytes,
            )
            .await?;
        }
    }
    if size_bytes == 0 {
        let _ = tokio::fs::remove_file(&staging_path).await;
        return Err(ApiError::conflict("backup_artifact_handoff_stdout_empty"));
    }
    staging
        .sync_data()
        .await
        .map_err(|_| ApiError::conflict("backup_artifact_handoff_staging_write_failed"))?;
    Ok(StagedRetainedBackupArtifact {
        staging_path,
        sha256_hex: hex::encode(hasher.finalize()),
        size_bytes: i64::try_from(size_bytes)
            .map_err(|_| ApiError::bad_request("backup_artifact_size_invalid"))?,
        source_chunk_count,
    })
}

pub(crate) fn backup_artifact_streaming_max_bytes() -> usize {
    env::var(BACKUP_HANDOFF_MAX_BYTES_ENV)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(MAX_BACKUP_ARTIFACT_CHUNKED_UPLOAD_BYTES)
}

async fn append_file_to_backup_handoff_staging(
    path: &std::path::Path,
    staging: &mut tokio::fs::File,
    hasher: &mut Sha256,
    size_bytes: &mut u64,
    max_bytes: u64,
) -> Result<(), ApiError> {
    let mut source = tokio::fs::File::open(path)
        .await
        .map_err(|_| ApiError::conflict("backup_artifact_handoff_output_read_failed"))?;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = source
            .read(&mut buffer)
            .await
            .map_err(|_| ApiError::conflict("backup_artifact_handoff_output_read_failed"))?;
        if read == 0 {
            break;
        }
        append_backup_handoff_bytes(&buffer[..read], staging, hasher, size_bytes, max_bytes)
            .await?;
    }
    Ok(())
}

async fn append_backup_handoff_bytes(
    bytes: &[u8],
    staging: &mut tokio::fs::File,
    hasher: &mut Sha256,
    size_bytes: &mut u64,
    max_bytes: u64,
) -> Result<(), ApiError> {
    let next_size = size_bytes
        .checked_add(bytes.len() as u64)
        .ok_or_else(|| ApiError::bad_request("backup_artifact_size_invalid"))?;
    if next_size > max_bytes {
        return Err(ApiError::bad_request("backup_artifact_size_invalid"));
    }
    staging
        .write_all(bytes)
        .await
        .map_err(|_| ApiError::conflict("backup_artifact_handoff_staging_write_failed"))?;
    hasher.update(bytes);
    *size_bytes = next_size;
    Ok(())
}

fn validate_backup_handoff_output_part(
    output: &JobOutputView,
    data: &[u8],
) -> Result<(), ApiError> {
    if let Some(expected_hash) = output.artifact_sha256_hex.as_deref() {
        if payload_hash(data) != expected_hash {
            return Err(ApiError::conflict(
                "backup_artifact_handoff_output_hash_mismatch",
            ));
        }
    }
    if let Some(expected_size) = output.artifact_size_bytes {
        if i64::try_from(data.len()).ok() != Some(expected_size) {
            return Err(ApiError::conflict(
                "backup_artifact_handoff_output_size_mismatch",
            ));
        }
    }
    Ok(())
}

fn backup_artifact_handoff_staging_dir() -> PathBuf {
    env::var_os(BACKUP_HANDOFF_STAGING_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir().join("vpsman-backup-handoff"))
}
