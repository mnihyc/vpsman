use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tokio::{
    io::AsyncReadExt,
    sync::mpsc,
    time::{self, Duration},
};
use vpsman_common::{payload_hash, validate_absolute_file_path, CommandOutput, OutputStream};

pub(crate) const COMMAND_OUTPUT_CHUNK_BYTES: usize = 64 * 1024;
const MAX_FILE_PULL_BYTES: u64 = 1024 * 1024;
const MAX_STREAMING_FILE_PULL_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StreamedPayloadSummary {
    pub(crate) size_bytes: u64,
    pub(crate) sha256_hex: String,
    pub(crate) chunk_bytes: usize,
    pub(crate) chunk_count: u64,
}

pub(crate) async fn execute_file_pull_with_timeout(
    job_id: uuid::Uuid,
    path: &str,
    timeout_secs: u64,
    output_tx: Option<mpsc::Sender<CommandOutput>>,
) -> Result<Vec<CommandOutput>> {
    time::timeout(
        Duration::from_secs(timeout_secs.max(1)),
        execute_file_pull(job_id, path, output_tx),
    )
    .await
    .context("file pull timed out")?
}

async fn execute_file_pull(
    job_id: uuid::Uuid,
    path: &str,
    output_tx: Option<mpsc::Sender<CommandOutput>>,
) -> Result<Vec<CommandOutput>> {
    validate_file_pull_path(path)?;
    let metadata = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("failed to stat file {path}"))?;
    if !metadata.is_file() {
        anyhow::bail!("file pull path is not a regular file");
    }
    if let Some(sender) = output_tx {
        return execute_streaming_file_pull(job_id, path, metadata.len(), sender).await;
    }
    if metadata.len() > MAX_FILE_PULL_BYTES {
        anyhow::bail!(
            "file exceeds pull limit: {} > {} bytes",
            metadata.len(),
            MAX_FILE_PULL_BYTES
        );
    }

    let data = tokio::fs::read(path)
        .await
        .with_context(|| format!("failed to read file {path}"))?;
    let mut outputs = chunked_output(job_id, OutputStream::Stdout, &data);
    let status = serde_json::json!({
        "type": "file_pull",
        "path": path,
        "size_bytes": data.len(),
        "sha256_hex": payload_hash(&data),
        "truncated": false,
        "chunk_bytes": COMMAND_OUTPUT_CHUNK_BYTES,
    });
    outputs.push(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: Some(0),
        done: true,
    });
    Ok(outputs)
}

async fn execute_streaming_file_pull(
    job_id: uuid::Uuid,
    path: &str,
    size_bytes: u64,
    output_tx: mpsc::Sender<CommandOutput>,
) -> Result<Vec<CommandOutput>> {
    if size_bytes > MAX_STREAMING_FILE_PULL_BYTES {
        anyhow::bail!(
            "file exceeds streaming pull limit: {} > {} bytes",
            size_bytes,
            MAX_STREAMING_FILE_PULL_BYTES
        );
    }
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("failed to open file {path}"))?;
    let mut buffer = vec![0_u8; COMMAND_OUTPUT_CHUNK_BYTES];
    let mut hasher = Sha256::new();
    let mut actual_size = 0_u64;
    let mut chunk_count = 0_u64;

    loop {
        let read = file
            .read(&mut buffer)
            .await
            .with_context(|| format!("failed to read file {path}"))?;
        if read == 0 {
            break;
        }
        actual_size += read as u64;
        hasher.update(&buffer[..read]);
        chunk_count += 1;
        output_tx
            .send(CommandOutput {
                job_id,
                stream: OutputStream::Stdout,
                data: buffer[..read].to_vec(),
                exit_code: None,
                done: false,
            })
            .await
            .context("file pull output receiver dropped")?;
    }

    if actual_size != size_bytes {
        anyhow::bail!("file size changed while pulling: expected {size_bytes}, read {actual_size}");
    }
    let status = serde_json::json!({
        "type": "file_pull",
        "path": path,
        "size_bytes": actual_size,
        "sha256_hex": hex::encode(hasher.finalize()),
        "truncated": false,
        "chunk_bytes": COMMAND_OUTPUT_CHUNK_BYTES,
        "chunk_count": chunk_count,
        "streamed": true,
    });
    Ok(vec![CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: Some(0),
        done: true,
    }])
}

pub(crate) fn chunked_output(
    job_id: uuid::Uuid,
    stream: OutputStream,
    data: &[u8],
) -> Vec<CommandOutput> {
    data.chunks(COMMAND_OUTPUT_CHUNK_BYTES)
        .map(|chunk| CommandOutput {
            job_id,
            stream,
            data: chunk.to_vec(),
            exit_code: None,
            done: false,
        })
        .collect()
}

pub(crate) async fn stream_buffered_payload_output(
    job_id: uuid::Uuid,
    stream: OutputStream,
    data: &[u8],
    output_tx: mpsc::Sender<CommandOutput>,
    receiver_dropped_context: &'static str,
) -> Result<StreamedPayloadSummary> {
    let mut hasher = Sha256::new();
    let mut chunk_count = 0_u64;
    for chunk in data.chunks(COMMAND_OUTPUT_CHUNK_BYTES) {
        hasher.update(chunk);
        chunk_count += 1;
        output_tx
            .send(CommandOutput {
                job_id,
                stream,
                data: chunk.to_vec(),
                exit_code: None,
                done: false,
            })
            .await
            .context(receiver_dropped_context)?;
    }
    Ok(StreamedPayloadSummary {
        size_bytes: data.len() as u64,
        sha256_hex: hex::encode(hasher.finalize()),
        chunk_bytes: COMMAND_OUTPUT_CHUNK_BYTES,
        chunk_count,
    })
}

fn validate_file_pull_path(path: &str) -> Result<()> {
    validate_absolute_file_path(path).map_err(|error| anyhow::anyhow!(error.to_string()))
}
