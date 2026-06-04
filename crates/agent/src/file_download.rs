use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    time::sleep,
};
use uuid::Uuid;
use vpsman_common::{
    payload_hash, validate_file_transfer_download_chunk_request,
    validate_file_transfer_download_session, CommandOutput, OutputStream,
    FILE_TRANSFER_CHUNK_BYTES, MAX_RESUMABLE_FILE_DOWNLOAD_BYTES,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
struct FileDownloadSessionMetadata {
    session_id: Uuid,
    path: String,
    size_bytes: u64,
    sha256_hex: String,
    chunk_size_bytes: u32,
    rate_limit_kbps: u32,
    resume_token_hash: String,
}

pub(crate) async fn execute_file_transfer_download_start(
    job_id: uuid::Uuid,
    session_id: Uuid,
    path: &str,
    chunk_size_bytes: u32,
    rate_limit_kbps: u32,
    resume_token_hash: &str,
) -> Result<Vec<CommandOutput>> {
    validate_file_transfer_download_session(
        session_id,
        path,
        chunk_size_bytes,
        rate_limit_kbps,
        resume_token_hash,
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let metadata_path = download_metadata_path(session_id);
    let file_metadata = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("failed to stat download source {path}"))?;
    if !file_metadata.is_file() {
        anyhow::bail!("file transfer download source is not a regular file");
    }
    let size_bytes = file_metadata.len();
    if size_bytes > MAX_RESUMABLE_FILE_DOWNLOAD_BYTES {
        anyhow::bail!(
            "file transfer download source exceeds limit: {} > {} bytes",
            size_bytes,
            MAX_RESUMABLE_FILE_DOWNLOAD_BYTES
        );
    }
    let sha256_hex = hash_file(Path::new(path)).await?;
    let resumed = if let Ok(existing) = read_download_metadata(&metadata_path).await {
        ensure_download_metadata_matches(
            &existing,
            path,
            size_bytes,
            &sha256_hex,
            chunk_size_bytes,
            rate_limit_kbps,
            resume_token_hash,
        )?;
        true
    } else {
        false
    };
    let metadata = FileDownloadSessionMetadata {
        session_id,
        path: path.to_string(),
        size_bytes,
        sha256_hex: sha256_hex.clone(),
        chunk_size_bytes,
        rate_limit_kbps,
        resume_token_hash: resume_token_hash.to_ascii_lowercase(),
    };
    write_download_metadata(&metadata_path, &metadata).await?;
    download_status(
        job_id,
        "file_transfer_download_start",
        session_id,
        path,
        0,
        Some(size_bytes),
        serde_json::json!({
            "resumed": resumed,
            "sha256_hex": sha256_hex,
            "chunk_size_bytes": chunk_size_bytes,
            "rate_limit_kbps": rate_limit_kbps,
        }),
    )
}

pub(crate) async fn execute_file_transfer_download_chunk(
    job_id: uuid::Uuid,
    session_id: Uuid,
    offset: u64,
    max_bytes: u32,
    resume_token_hash: &str,
) -> Result<Vec<CommandOutput>> {
    validate_file_transfer_download_chunk_request(session_id, offset, max_bytes, resume_token_hash)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let metadata = read_download_metadata(&download_metadata_path(session_id)).await?;
    ensure_resume_token(&metadata, resume_token_hash)?;
    if offset > metadata.size_bytes {
        anyhow::bail!("file transfer download offset is beyond file size");
    }
    let read_size = (metadata.size_bytes - offset)
        .min(u64::from(max_bytes))
        .min(u64::from(metadata.chunk_size_bytes)) as usize;
    let chunk = read_file_chunk(Path::new(&metadata.path), offset, read_size).await?;
    maybe_throttle(metadata.rate_limit_kbps, chunk.len()).await;
    let next_offset = offset + chunk.len() as u64;
    let mut outputs = Vec::new();
    if !chunk.is_empty() {
        outputs.push(CommandOutput {
            job_id,
            stream: OutputStream::Stdout,
            data: chunk.clone(),
            exit_code: None,
            done: false,
        });
    }
    outputs.push(download_status_output(
        job_id,
        "file_transfer_download_chunk",
        session_id,
        &metadata.path,
        next_offset,
        Some(metadata.size_bytes),
        serde_json::json!({
            "offset": offset,
            "chunk_size_bytes": chunk.len(),
            "chunk_sha256_hex": payload_hash(&chunk),
            "complete": next_offset == metadata.size_bytes,
            "file_sha256_hex": metadata.sha256_hex,
        }),
    )?);
    if next_offset == metadata.size_bytes {
        let _ = tokio::fs::remove_file(download_metadata_path(session_id)).await;
    }
    Ok(outputs)
}

fn ensure_download_metadata_matches(
    metadata: &FileDownloadSessionMetadata,
    path: &str,
    size_bytes: u64,
    sha256_hex: &str,
    chunk_size_bytes: u32,
    rate_limit_kbps: u32,
    resume_token_hash: &str,
) -> Result<()> {
    if metadata.path != path
        || metadata.size_bytes != size_bytes
        || metadata.sha256_hex != sha256_hex.to_ascii_lowercase()
        || metadata.chunk_size_bytes != chunk_size_bytes
        || metadata.rate_limit_kbps != rate_limit_kbps
        || metadata.resume_token_hash != resume_token_hash.to_ascii_lowercase()
    {
        anyhow::bail!("file transfer download session metadata does not match start request");
    }
    Ok(())
}

fn ensure_resume_token(
    metadata: &FileDownloadSessionMetadata,
    resume_token_hash: &str,
) -> Result<()> {
    if metadata.resume_token_hash != resume_token_hash.to_ascii_lowercase() {
        anyhow::bail!("file transfer download resume token hash mismatch");
    }
    Ok(())
}

async fn read_file_chunk(path: &Path, offset: u64, size: usize) -> Result<Vec<u8>> {
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("failed to open download source {}", path.display()))?;
    file.seek(std::io::SeekFrom::Start(offset)).await?;
    let mut chunk = vec![0_u8; size];
    let read = file.read(&mut chunk).await?;
    chunk.truncate(read);
    Ok(chunk)
}

async fn hash_file(path: &Path) -> Result<String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("failed to open download source {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; FILE_TRANSFER_CHUNK_BYTES];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

async fn read_download_metadata(path: &Path) -> Result<FileDownloadSessionMetadata> {
    let data = tokio::fs::read(path)
        .await
        .with_context(|| format!("failed to read download metadata {}", path.display()))?;
    serde_json::from_slice(&data).context("file transfer download metadata is invalid")
}

async fn write_download_metadata(
    path: &Path,
    metadata: &FileDownloadSessionMetadata,
) -> Result<()> {
    let data = serde_json::to_vec(metadata)?;
    tokio::fs::write(path, data)
        .await
        .with_context(|| format!("failed to write download metadata {}", path.display()))
}

async fn maybe_throttle(rate_limit_kbps: u32, byte_count: usize) {
    if rate_limit_kbps == 0 || byte_count == 0 {
        return;
    }
    let bits = byte_count as u64 * 8;
    let millis = bits.saturating_mul(1000) / (rate_limit_kbps as u64 * 1000);
    if millis > 0 {
        sleep(Duration::from_millis(millis)).await;
    }
}

fn download_metadata_path(session_id: Uuid) -> PathBuf {
    std::env::temp_dir().join(format!("vpsman-download-{session_id}.json"))
}

fn download_status(
    job_id: uuid::Uuid,
    status_type: &'static str,
    session_id: Uuid,
    path: &str,
    next_offset: u64,
    size_bytes: Option<u64>,
    extra: serde_json::Value,
) -> Result<Vec<CommandOutput>> {
    Ok(vec![download_status_output(
        job_id,
        status_type,
        session_id,
        path,
        next_offset,
        size_bytes,
        extra,
    )?])
}

fn download_status_output(
    job_id: uuid::Uuid,
    status_type: &'static str,
    session_id: Uuid,
    path: &str,
    next_offset: u64,
    size_bytes: Option<u64>,
    extra: serde_json::Value,
) -> Result<CommandOutput> {
    let status = serde_json::json!({
        "type": status_type,
        "session_id": session_id,
        "path": path,
        "next_offset": next_offset,
        "size_bytes": size_bytes,
        "extra": extra,
    });
    Ok(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: Some(0),
        done: true,
    })
}
