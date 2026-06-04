use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    time::{sleep, Duration},
};
use uuid::Uuid;
use vpsman_common::{
    decode_chunked_file_payload, decode_file_transfer_chunk, decode_inline_file_payload,
    payload_hash, validate_absolute_file_path, validate_file_mode, validate_file_transfer_session,
    validate_file_transfer_session_token, CommandOutput, FilePushChunk, OutputStream,
    FILE_TRANSFER_CHUNK_BYTES,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
struct FileTransferSessionMetadata {
    session_id: Uuid,
    path: String,
    temp_path: String,
    mode: u32,
    size_bytes: u64,
    sha256_hex: String,
    chunk_size_bytes: u32,
    rate_limit_kbps: u32,
    resume_token_hash: String,
}

pub(crate) async fn execute_file_push(
    job_id: uuid::Uuid,
    path: &str,
    mode: u32,
    size_bytes: u64,
    sha256_hex: &str,
    data_base64: &str,
) -> Result<Vec<CommandOutput>> {
    validate_file_push_request(path, mode)?;
    let data = decode_inline_file_payload(data_base64, size_bytes, sha256_hex)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    write_file_push(job_id, path, mode, &data, "file_push", None).await
}

pub(crate) async fn execute_file_push_chunked(
    job_id: uuid::Uuid,
    path: &str,
    mode: u32,
    size_bytes: u64,
    sha256_hex: &str,
    chunks: &[FilePushChunk],
) -> Result<Vec<CommandOutput>> {
    validate_file_push_request(path, mode)?;
    let data = decode_chunked_file_payload(chunks, size_bytes, sha256_hex)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    write_file_push(
        job_id,
        path,
        mode,
        &data,
        "file_push_chunked",
        Some(chunks.len()),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_file_transfer_start(
    job_id: uuid::Uuid,
    session_id: Uuid,
    path: &str,
    mode: u32,
    size_bytes: u64,
    sha256_hex: &str,
    chunk_size_bytes: u32,
    rate_limit_kbps: u32,
    resume_token_hash: &str,
) -> Result<Vec<CommandOutput>> {
    validate_file_transfer_session(
        session_id,
        path,
        mode,
        size_bytes,
        sha256_hex,
        chunk_size_bytes,
        rate_limit_kbps,
        resume_token_hash,
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let paths = transfer_session_paths(path, session_id)?;
    if let Ok(metadata) = read_transfer_metadata(&paths.metadata).await {
        ensure_metadata_matches(
            &metadata,
            path,
            mode,
            size_bytes,
            sha256_hex,
            chunk_size_bytes,
            rate_limit_kbps,
            resume_token_hash,
        )?;
        let next_offset = current_transfer_offset(&paths.temp, size_bytes).await?;
        return transfer_status(
            job_id,
            "file_transfer_start",
            session_id,
            path,
            next_offset,
            Some(size_bytes),
            serde_json::json!({
                "resumed": true,
                "chunk_size_bytes": metadata.chunk_size_bytes,
                "rate_limit_kbps": metadata.rate_limit_kbps,
            }),
        );
    }

    if paths.temp.exists() {
        anyhow::bail!("file transfer temporary file exists without session metadata");
    }
    if let Ok(metadata) = tokio::fs::metadata(&paths.destination).await {
        if metadata.is_dir() {
            anyhow::bail!("file transfer destination is a directory");
        }
    }
    tokio::fs::File::create(&paths.temp)
        .await
        .with_context(|| {
            format!(
                "failed to create transfer temp file {}",
                paths.temp.display()
            )
        })?;
    let metadata = FileTransferSessionMetadata {
        session_id,
        path: path.to_string(),
        temp_path: paths.temp.to_string_lossy().to_string(),
        mode,
        size_bytes,
        sha256_hex: sha256_hex.to_ascii_lowercase(),
        chunk_size_bytes,
        rate_limit_kbps,
        resume_token_hash: resume_token_hash.to_ascii_lowercase(),
    };
    write_transfer_metadata(&paths.metadata, &metadata).await?;
    transfer_status(
        job_id,
        "file_transfer_start",
        session_id,
        path,
        0,
        Some(size_bytes),
        serde_json::json!({
            "resumed": false,
            "chunk_size_bytes": chunk_size_bytes,
            "rate_limit_kbps": rate_limit_kbps,
        }),
    )
}

pub(crate) async fn execute_file_transfer_chunk(
    job_id: uuid::Uuid,
    session_id: Uuid,
    offset: u64,
    chunk: &FilePushChunk,
    resume_token_hash: &str,
) -> Result<Vec<CommandOutput>> {
    validate_file_transfer_session_token(session_id, resume_token_hash)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    if offset != chunk.offset {
        anyhow::bail!("file transfer chunk offset does not match command offset");
    }
    let metadata = read_metadata_by_session(session_id).await?;
    ensure_resume_token(&metadata, resume_token_hash)?;
    let paths = transfer_session_paths(&metadata.path, session_id)?;
    let decoded = decode_file_transfer_chunk(chunk, metadata.chunk_size_bytes)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    if offset.saturating_add(decoded.len() as u64) > metadata.size_bytes {
        anyhow::bail!("file transfer chunk exceeds declared size");
    }
    let current_offset = current_transfer_offset(&paths.temp, metadata.size_bytes).await?;
    let next_offset = if offset == current_offset {
        write_transfer_chunk(&paths.temp, offset, &decoded).await?;
        maybe_throttle(metadata.rate_limit_kbps, decoded.len()).await;
        offset + decoded.len() as u64
    } else if offset < current_offset && offset + decoded.len() as u64 <= current_offset {
        verify_existing_chunk(&paths.temp, offset, &decoded).await?;
        current_offset
    } else {
        anyhow::bail!("file transfer chunk offset is not resumable from current state");
    };
    transfer_status(
        job_id,
        "file_transfer_chunk_ack",
        session_id,
        &metadata.path,
        next_offset,
        Some(metadata.size_bytes),
        serde_json::json!({
            "ack_offset": offset,
            "ack_size_bytes": decoded.len(),
            "duplicate": offset < current_offset,
        }),
    )
}

pub(crate) async fn execute_file_transfer_commit(
    job_id: uuid::Uuid,
    session_id: Uuid,
    resume_token_hash: &str,
) -> Result<Vec<CommandOutput>> {
    validate_file_transfer_session_token(session_id, resume_token_hash)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let metadata = read_metadata_by_session(session_id).await?;
    ensure_resume_token(&metadata, resume_token_hash)?;
    let paths = transfer_session_paths(&metadata.path, session_id)?;
    let current_offset = current_transfer_offset(&paths.temp, metadata.size_bytes).await?;
    if current_offset != metadata.size_bytes {
        anyhow::bail!("file transfer commit before all bytes are received");
    }
    let actual_hash = hash_file(&paths.temp).await?;
    if actual_hash != metadata.sha256_hex {
        anyhow::bail!("file transfer final hash mismatch");
    }
    tokio::fs::set_permissions(&paths.temp, std::fs::Permissions::from_mode(metadata.mode))
        .await
        .with_context(|| format!("failed to set file mode on {}", paths.temp.display()))?;
    if let Err(error) = tokio::fs::rename(&paths.temp, &paths.destination).await {
        let _ = tokio::fs::remove_file(&paths.temp).await;
        return Err(error).with_context(|| {
            format!(
                "failed to move file into place at {}",
                paths.destination.display()
            )
        });
    }
    let _ = tokio::fs::remove_file(&paths.metadata).await;
    transfer_status(
        job_id,
        "file_transfer_commit",
        session_id,
        &metadata.path,
        metadata.size_bytes,
        Some(metadata.size_bytes),
        serde_json::json!({
            "sha256_hex": actual_hash,
            "mode": metadata.mode,
            "atomic": true,
        }),
    )
}

pub(crate) async fn execute_file_transfer_abort(
    job_id: uuid::Uuid,
    session_id: Uuid,
    resume_token_hash: &str,
) -> Result<Vec<CommandOutput>> {
    validate_file_transfer_session_token(session_id, resume_token_hash)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let metadata = read_metadata_by_session(session_id).await?;
    ensure_resume_token(&metadata, resume_token_hash)?;
    let paths = transfer_session_paths(&metadata.path, session_id)?;
    let _ = tokio::fs::remove_file(&paths.temp).await;
    let _ = tokio::fs::remove_file(&paths.metadata).await;
    transfer_status(
        job_id,
        "file_transfer_abort",
        session_id,
        &metadata.path,
        0,
        Some(metadata.size_bytes),
        serde_json::json!({
            "aborted": true,
        }),
    )
}

async fn write_file_push(
    job_id: uuid::Uuid,
    path: &str,
    mode: u32,
    data: &[u8],
    status_type: &'static str,
    chunk_count: Option<usize>,
) -> Result<Vec<CommandOutput>> {
    let destination = Path::new(path);
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .context("file push destination has no parent directory")?;
    let file_name = destination
        .file_name()
        .context("file push destination has no file name")?
        .to_string_lossy();
    let temp_path = parent.join(format!(
        ".vpsman-upload-{file_name}-{}",
        uuid::Uuid::new_v4()
    ));

    if let Ok(metadata) = tokio::fs::metadata(destination).await {
        if metadata.is_dir() {
            anyhow::bail!("file push destination is a directory");
        }
    }
    tokio::fs::write(&temp_path, data)
        .await
        .with_context(|| format!("failed to write temporary file {}", temp_path.display()))?;
    tokio::fs::set_permissions(&temp_path, std::fs::Permissions::from_mode(mode))
        .await
        .with_context(|| format!("failed to set file mode on {}", temp_path.display()))?;
    if let Err(error) = tokio::fs::rename(&temp_path, destination).await {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(error).with_context(|| {
            format!(
                "failed to move file into place at {}",
                destination.display()
            )
        });
    }

    let status = serde_json::json!({
        "type": status_type,
        "path": path,
        "size_bytes": data.len(),
        "sha256_hex": payload_hash(data),
        "mode": mode,
        "atomic": true,
        "chunk_count": chunk_count,
    });
    Ok(vec![CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: Some(0),
        done: true,
    }])
}

fn validate_file_push_request(path: &str, mode: u32) -> Result<()> {
    validate_absolute_file_path(path).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    validate_file_mode(mode).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(())
}

struct TransferSessionPaths {
    destination: PathBuf,
    temp: PathBuf,
    metadata: PathBuf,
}

fn transfer_session_paths(path: &str, session_id: Uuid) -> Result<TransferSessionPaths> {
    validate_absolute_file_path(path).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let destination = PathBuf::from(path);
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .context("file transfer destination has no parent directory")?;
    let file_name = destination
        .file_name()
        .context("file transfer destination has no file name")?
        .to_string_lossy();
    let temp = parent.join(format!(".vpsman-transfer-{file_name}-{session_id}.part"));
    let metadata = std::env::temp_dir().join(format!("vpsman-transfer-{session_id}.json"));
    Ok(TransferSessionPaths {
        destination,
        temp,
        metadata,
    })
}

async fn read_metadata_by_session(session_id: Uuid) -> Result<FileTransferSessionMetadata> {
    if session_id.is_nil() {
        anyhow::bail!("file transfer session id is invalid");
    }
    let path = std::env::temp_dir().join(format!("vpsman-transfer-{session_id}.json"));
    read_transfer_metadata(&path).await
}

async fn read_transfer_metadata(path: &Path) -> Result<FileTransferSessionMetadata> {
    let data = tokio::fs::read(path)
        .await
        .with_context(|| format!("failed to read transfer metadata {}", path.display()))?;
    serde_json::from_slice(&data).context("file transfer metadata is invalid")
}

async fn write_transfer_metadata(
    path: &Path,
    metadata: &FileTransferSessionMetadata,
) -> Result<()> {
    let data = serde_json::to_vec(metadata)?;
    tokio::fs::write(path, data)
        .await
        .with_context(|| format!("failed to write transfer metadata {}", path.display()))
}

#[allow(clippy::too_many_arguments)]
fn ensure_metadata_matches(
    metadata: &FileTransferSessionMetadata,
    path: &str,
    mode: u32,
    size_bytes: u64,
    sha256_hex: &str,
    chunk_size_bytes: u32,
    rate_limit_kbps: u32,
    resume_token_hash: &str,
) -> Result<()> {
    if metadata.path != path
        || metadata.mode != mode
        || metadata.size_bytes != size_bytes
        || metadata.sha256_hex != sha256_hex.to_ascii_lowercase()
        || metadata.chunk_size_bytes != chunk_size_bytes
        || metadata.rate_limit_kbps != rate_limit_kbps
        || metadata.resume_token_hash != resume_token_hash.to_ascii_lowercase()
    {
        anyhow::bail!("file transfer session metadata does not match start request");
    }
    Ok(())
}

fn ensure_resume_token(
    metadata: &FileTransferSessionMetadata,
    resume_token_hash: &str,
) -> Result<()> {
    if metadata.resume_token_hash != resume_token_hash.to_ascii_lowercase() {
        anyhow::bail!("file transfer resume token hash mismatch");
    }
    Ok(())
}

async fn current_transfer_offset(path: &Path, declared_size: u64) -> Result<u64> {
    let len = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("failed to stat transfer temp file {}", path.display()))?
        .len();
    if len > declared_size {
        anyhow::bail!("file transfer temporary file is larger than declared size");
    }
    Ok(len)
}

async fn write_transfer_chunk(path: &Path, offset: u64, data: &[u8]) -> Result<()> {
    let mut file = tokio::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .await
        .with_context(|| format!("failed to open transfer temp file {}", path.display()))?;
    file.seek(std::io::SeekFrom::Start(offset)).await?;
    file.write_all(data).await?;
    file.flush().await?;
    Ok(())
}

async fn verify_existing_chunk(path: &Path, offset: u64, data: &[u8]) -> Result<()> {
    let mut file = tokio::fs::OpenOptions::new()
        .read(true)
        .open(path)
        .await
        .with_context(|| format!("failed to open transfer temp file {}", path.display()))?;
    file.seek(std::io::SeekFrom::Start(offset)).await?;
    let mut existing = vec![0_u8; data.len()];
    file.read_exact(&mut existing).await?;
    if existing != data {
        anyhow::bail!("duplicate file transfer chunk does not match existing bytes");
    }
    Ok(())
}

async fn hash_file(path: &Path) -> Result<String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("failed to open transfer temp file {}", path.display()))?;
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

fn transfer_status(
    job_id: uuid::Uuid,
    status_type: &'static str,
    session_id: Uuid,
    path: &str,
    next_offset: u64,
    size_bytes: Option<u64>,
    extra: serde_json::Value,
) -> Result<Vec<CommandOutput>> {
    let status = serde_json::json!({
        "type": status_type,
        "session_id": session_id,
        "path": path,
        "next_offset": next_offset,
        "size_bytes": size_bytes,
        "extra": extra,
    });
    Ok(vec![CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: Some(0),
        done: true,
    }])
}
