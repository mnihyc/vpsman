use std::{
    ffi::OsStr,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    sync::mpsc,
    time::sleep,
};
use uuid::Uuid;
use vpsman_common::{
    payload_hash, validate_absolute_file_path, validate_file_transfer_download_chunk_request,
    validate_file_transfer_download_session, CommandOutput, OutputStream,
    FILE_TRANSFER_CHUNK_BYTES, MAX_DIRECT_FILE_DOWNLOAD_BYTES, MAX_RESUMABLE_FILE_DOWNLOAD_BYTES,
};

use crate::command_worker::CommandCancelToken;
use crate::file_pull::{
    chunked_output, stream_buffered_payload_output, COMMAND_OUTPUT_CHUNK_BYTES,
};

const FILE_DOWNLOAD_MANIFEST_ENTRY_LIMIT: usize = 4096;

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

#[derive(Clone, Debug, Serialize)]
struct FileDownloadManifestEntry {
    path: String,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha256_hex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    symlink_target: Option<String>,
}

#[derive(Clone, Debug)]
struct FileDownloadManifestSummary {
    entries: Vec<FileDownloadManifestEntry>,
    hierarchy_sha256_hex: String,
    content_manifest_sha256_hex: String,
    file_count: u64,
    directory_count: u64,
    symlink_count: u64,
    other_count: u64,
    total_file_bytes: u64,
    truncated: bool,
}

struct FileDownloadManifestBuilder {
    entries: Vec<FileDownloadManifestEntry>,
    hierarchy_hasher: Sha256,
    content_hasher: Sha256,
    file_count: u64,
    directory_count: u64,
    symlink_count: u64,
    other_count: u64,
    total_file_bytes: u64,
    truncated: bool,
}

struct DirectoryDownloadArtifact {
    archive: Vec<u8>,
    manifest: FileDownloadManifestSummary,
}

pub(crate) async fn execute_file_download(
    job_id: uuid::Uuid,
    path: &str,
    max_bytes: u64,
    output_tx: Option<mpsc::Sender<CommandOutput>>,
    cancel_token: CommandCancelToken,
) -> Result<Vec<CommandOutput>> {
    cancel_token.check("file_download")?;
    validate_absolute_file_path(path).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let max_bytes = max_bytes.clamp(1, MAX_DIRECT_FILE_DOWNLOAD_BYTES);
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .with_context(|| format!("failed to stat download source {path}"))?;
    if metadata.file_type().is_symlink() {
        anyhow::bail!("file download source is a symlink");
    }
    if metadata.is_file() {
        return execute_regular_file_download(
            job_id,
            path,
            metadata.len(),
            max_bytes,
            output_tx,
            cancel_token,
        )
        .await;
    }
    if metadata.is_dir() {
        return execute_directory_download(job_id, path, max_bytes, output_tx, cancel_token).await;
    }
    anyhow::bail!("file download source is not a regular file or directory");
}

async fn execute_regular_file_download(
    job_id: uuid::Uuid,
    path: &str,
    size_bytes: u64,
    max_bytes: u64,
    output_tx: Option<mpsc::Sender<CommandOutput>>,
    cancel_token: CommandCancelToken,
) -> Result<Vec<CommandOutput>> {
    cancel_token.check("file_download")?;
    if size_bytes > max_bytes {
        anyhow::bail!("file download source exceeds limit: {size_bytes} > {max_bytes} bytes");
    }
    let filename = download_filename(path, false);
    if let Some(sender) = output_tx {
        let summary = stream_file_payload(job_id, path, sender, cancel_token).await?;
        return file_download_status(
            job_id,
            path,
            "file",
            &filename,
            "application/octet-stream",
            summary.size_bytes,
            &summary.sha256_hex,
            false,
            summary.chunk_count,
            None,
        );
    }

    cancel_token.check("file_download")?;
    let data = tokio::fs::read(path)
        .await
        .with_context(|| format!("failed to read download source {path}"))?;
    cancel_token.check("file_download")?;
    let mut outputs = chunked_output(job_id, OutputStream::Stdout, &data);
    outputs.push(file_download_status_output(
        job_id,
        path,
        "file",
        &filename,
        "application/octet-stream",
        data.len() as u64,
        &payload_hash(&data),
        false,
        data.chunks(COMMAND_OUTPUT_CHUNK_BYTES).count() as u64,
        None,
    )?);
    Ok(outputs)
}

async fn execute_directory_download(
    job_id: uuid::Uuid,
    path: &str,
    max_bytes: u64,
    output_tx: Option<mpsc::Sender<CommandOutput>>,
    cancel_token: CommandCancelToken,
) -> Result<Vec<CommandOutput>> {
    cancel_token.check("file_download")?;
    let source = PathBuf::from(path);
    let worker_token = cancel_token.clone();
    let artifact = tokio::task::spawn_blocking(move || {
        build_directory_download_artifact(&source, max_bytes, &worker_token)
    })
    .await
    .context("file download archive worker failed")??;
    cancel_token.check("file_download")?;
    let DirectoryDownloadArtifact { archive, manifest } = artifact;
    let filename = download_filename(path, true);
    if let Some(sender) = output_tx {
        let summary = stream_buffered_payload_output(
            job_id,
            OutputStream::Stdout,
            &archive,
            sender,
            "file download output receiver dropped",
        )
        .await?;
        return file_download_status(
            job_id,
            path,
            "directory",
            &filename,
            "application/x-tar",
            summary.size_bytes,
            &summary.sha256_hex,
            true,
            summary.chunk_count,
            Some(&manifest),
        );
    }

    let mut outputs = chunked_output(job_id, OutputStream::Stdout, &archive);
    outputs.push(file_download_status_output(
        job_id,
        path,
        "directory",
        &filename,
        "application/x-tar",
        archive.len() as u64,
        &payload_hash(&archive),
        true,
        archive.chunks(COMMAND_OUTPUT_CHUNK_BYTES).count() as u64,
        Some(&manifest),
    )?);
    Ok(outputs)
}

async fn stream_file_payload(
    job_id: uuid::Uuid,
    path: &str,
    output_tx: mpsc::Sender<CommandOutput>,
    cancel_token: CommandCancelToken,
) -> Result<crate::file_pull::StreamedPayloadSummary> {
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("failed to open download source {path}"))?;
    let mut buffer = vec![0_u8; COMMAND_OUTPUT_CHUNK_BYTES];
    let mut hasher = Sha256::new();
    let mut size_bytes = 0_u64;
    let mut chunk_count = 0_u64;
    loop {
        cancel_token.check("file_download")?;
        let read = file
            .read(&mut buffer)
            .await
            .with_context(|| format!("failed to read download source {path}"))?;
        if read == 0 {
            break;
        }
        size_bytes += read as u64;
        chunk_count += 1;
        hasher.update(&buffer[..read]);
        output_tx
            .send(CommandOutput {
                job_id,
                stream: OutputStream::Stdout,
                data: buffer[..read].to_vec(),
                exit_code: None,
                done: false,
            })
            .await
            .context("file download output receiver dropped")?;
    }
    Ok(crate::file_pull::StreamedPayloadSummary {
        size_bytes,
        sha256_hex: hex::encode(hasher.finalize()),
        chunk_bytes: COMMAND_OUTPUT_CHUNK_BYTES,
        chunk_count,
    })
}

fn file_download_status(
    job_id: uuid::Uuid,
    path: &str,
    source_kind: &'static str,
    filename: &str,
    content_type: &'static str,
    size_bytes: u64,
    sha256_hex: &str,
    archive: bool,
    chunk_count: u64,
    manifest: Option<&FileDownloadManifestSummary>,
) -> Result<Vec<CommandOutput>> {
    Ok(vec![file_download_status_output(
        job_id,
        path,
        source_kind,
        filename,
        content_type,
        size_bytes,
        sha256_hex,
        archive,
        chunk_count,
        manifest,
    )?])
}

fn file_download_status_output(
    job_id: uuid::Uuid,
    path: &str,
    source_kind: &'static str,
    filename: &str,
    content_type: &'static str,
    size_bytes: u64,
    sha256_hex: &str,
    archive: bool,
    chunk_count: u64,
    manifest: Option<&FileDownloadManifestSummary>,
) -> Result<CommandOutput> {
    let mut status = serde_json::json!({
        "type": "file_download",
        "path": path,
        "source_kind": source_kind,
        "filename": filename,
        "content_type": content_type,
        "size_bytes": size_bytes,
        "sha256_hex": sha256_hex,
        "archive": archive,
        "chunk_bytes": COMMAND_OUTPUT_CHUNK_BYTES,
        "chunk_count": chunk_count,
    });
    if let Some(manifest) = manifest {
        status["hierarchy_sha256_hex"] = serde_json::json!(manifest.hierarchy_sha256_hex);
        status["content_manifest_sha256_hex"] =
            serde_json::json!(manifest.content_manifest_sha256_hex);
        status["manifest_entries"] = serde_json::json!(manifest.entries);
        status["manifest_entry_count"] = serde_json::json!(
            manifest.file_count
                + manifest.directory_count
                + manifest.symlink_count
                + manifest.other_count
        );
        status["manifest_emitted_entry_count"] = serde_json::json!(manifest.entries.len());
        status["manifest_truncated"] = serde_json::json!(manifest.truncated);
        status["file_count"] = serde_json::json!(manifest.file_count);
        status["directory_count"] = serde_json::json!(manifest.directory_count);
        status["symlink_count"] = serde_json::json!(manifest.symlink_count);
        status["other_count"] = serde_json::json!(manifest.other_count);
        status["total_file_bytes"] = serde_json::json!(manifest.total_file_bytes);
    }
    Ok(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: Some(0),
        done: true,
    })
}

fn build_directory_download_artifact(
    source: &Path,
    max_bytes: u64,
    cancel_token: &CommandCancelToken,
) -> Result<DirectoryDownloadArtifact> {
    cancel_token.check("file_download")?;
    let metadata = std::fs::symlink_metadata(source)
        .with_context(|| format!("failed to stat download source {}", source.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        anyhow::bail!("file download archive source is not a directory");
    }
    let manifest = build_directory_manifest(source, max_bytes, cancel_token)?;
    let mut archive = Vec::new();
    {
        cancel_token.check("file_download")?;
        let mut builder = tar::Builder::new(&mut archive);
        let name = source
            .file_name()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| OsStr::new("root"));
        let archive_name = PathBuf::from(Path::new(name));
        append_tar_path_checked(&mut builder, &archive_name, source, &metadata, cancel_token)?;
        builder.finish().context("failed to finish tar archive")?;
    }
    if archive.len() as u64 > max_bytes {
        anyhow::bail!("tar archive exceeds limit: {} > {max_bytes}", archive.len());
    }
    Ok(DirectoryDownloadArtifact { archive, manifest })
}

fn append_tar_path_checked<W: Write>(
    builder: &mut tar::Builder<W>,
    archive_path: &Path,
    fs_path: &Path,
    metadata: &std::fs::Metadata,
    cancel_token: &CommandCancelToken,
) -> Result<()> {
    cancel_token.check("file_download")?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        builder
            .append_dir(archive_path, fs_path)
            .with_context(|| format!("failed to archive directory {}", fs_path.display()))?;
        let mut entries = std::fs::read_dir(fs_path)
            .with_context(|| format!("failed to read {}", fs_path.display()))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            cancel_token.check("file_download")?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)
                .with_context(|| format!("failed to stat {}", path.display()))?;
            let file_name = entry.file_name();
            let child_archive_path = archive_path.join(Path::new(&file_name));
            append_tar_path_checked(builder, &child_archive_path, &path, &metadata, cancel_token)?;
        }
        return Ok(());
    }
    builder
        .append_path_with_name(fs_path, archive_path)
        .with_context(|| format!("failed to archive file {}", fs_path.display()))?;
    Ok(())
}

fn build_directory_manifest(
    source: &Path,
    max_bytes: u64,
    cancel_token: &CommandCancelToken,
) -> Result<FileDownloadManifestSummary> {
    let mut builder = FileDownloadManifestBuilder::new();
    collect_manifest_entries(source, source, max_bytes, &mut builder, cancel_token)?;
    Ok(builder.finish())
}

impl FileDownloadManifestBuilder {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            hierarchy_hasher: Sha256::new(),
            content_hasher: Sha256::new(),
            file_count: 0,
            directory_count: 0,
            symlink_count: 0,
            other_count: 0,
            total_file_bytes: 0,
            truncated: false,
        }
    }

    fn push_entry(&mut self, entry: FileDownloadManifestEntry) {
        hash_manifest_hierarchy_entry(&mut self.hierarchy_hasher, &entry);
        hash_manifest_content_entry(&mut self.content_hasher, &entry);
        if self.entries.len() < FILE_DOWNLOAD_MANIFEST_ENTRY_LIMIT {
            self.entries.push(entry);
        } else {
            self.truncated = true;
        }
    }

    fn finish(self) -> FileDownloadManifestSummary {
        FileDownloadManifestSummary {
            entries: self.entries,
            hierarchy_sha256_hex: hex::encode(self.hierarchy_hasher.finalize()),
            content_manifest_sha256_hex: hex::encode(self.content_hasher.finalize()),
            file_count: self.file_count,
            directory_count: self.directory_count,
            symlink_count: self.symlink_count,
            other_count: self.other_count,
            total_file_bytes: self.total_file_bytes,
            truncated: self.truncated,
        }
    }
}

fn collect_manifest_entries(
    root: &Path,
    current: &Path,
    max_bytes: u64,
    builder: &mut FileDownloadManifestBuilder,
    cancel_token: &CommandCancelToken,
) -> Result<()> {
    cancel_token.check("file_download")?;
    let mut entries = std::fs::read_dir(current)
        .with_context(|| format!("failed to read {}", current.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        cancel_token.check("file_download")?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .with_context(|| format!("failed to stat {}", path.display()))?;
        let relative_path = manifest_relative_path(root, &path);
        if metadata.file_type().is_symlink() {
            builder.symlink_count = builder.symlink_count.saturating_add(1);
            let symlink_target = std::fs::read_link(&path)
                .ok()
                .map(|target| target.to_string_lossy().into_owned());
            builder.push_entry(FileDownloadManifestEntry {
                path: relative_path,
                kind: "symlink",
                size_bytes: None,
                sha256_hex: None,
                symlink_target,
            });
            continue;
        }
        if metadata.is_dir() {
            builder.directory_count = builder.directory_count.saturating_add(1);
            builder.push_entry(FileDownloadManifestEntry {
                path: relative_path,
                kind: "directory",
                size_bytes: None,
                sha256_hex: None,
                symlink_target: None,
            });
            collect_manifest_entries(root, &path, max_bytes, builder, cancel_token)?;
            continue;
        }
        if metadata.is_file() {
            builder.file_count = builder.file_count.saturating_add(1);
            builder.total_file_bytes = builder.total_file_bytes.saturating_add(metadata.len());
            if builder.total_file_bytes > max_bytes {
                anyhow::bail!(
                    "download source exceeds limit: {} > {max_bytes} bytes",
                    builder.total_file_bytes
                );
            }
            builder.push_entry(FileDownloadManifestEntry {
                path: relative_path,
                kind: "file",
                size_bytes: Some(metadata.len()),
                sha256_hex: Some(hash_sync_file(&path)?),
                symlink_target: None,
            });
            continue;
        }
        builder.other_count = builder.other_count.saturating_add(1);
        builder.push_entry(FileDownloadManifestEntry {
            path: relative_path,
            kind: "other",
            size_bytes: None,
            sha256_hex: None,
            symlink_target: None,
        });
    }
    Ok(())
}

fn manifest_relative_path(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let value = relative
        .iter()
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if value.is_empty() {
        ".".to_string()
    } else {
        value
    }
}

fn hash_manifest_hierarchy_entry(hasher: &mut Sha256, entry: &FileDownloadManifestEntry) {
    hash_manifest_field(hasher, &entry.path);
    hash_manifest_field(hasher, entry.kind);
    hash_manifest_field(hasher, entry.symlink_target.as_deref().unwrap_or(""));
    hasher.update([0xff]);
}

fn hash_manifest_content_entry(hasher: &mut Sha256, entry: &FileDownloadManifestEntry) {
    hash_manifest_hierarchy_entry(hasher, entry);
    hash_manifest_field(
        hasher,
        &entry
            .size_bytes
            .map(|value| value.to_string())
            .unwrap_or_default(),
    );
    hash_manifest_field(hasher, entry.sha256_hex.as_deref().unwrap_or(""));
    hasher.update([0xfe]);
}

fn hash_manifest_field(hasher: &mut Sha256, value: &str) {
    hasher.update(value.as_bytes());
    hasher.update([0]);
}

fn hash_sync_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("failed to open download source {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("failed to read download source {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn download_filename(path: &str, archive: bool) -> String {
    let name = Path::new(path)
        .file_name()
        .and_then(OsStr::to_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("root");
    if archive {
        format!("{name}.tar")
    } else {
        name.to_string()
    }
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn directory_download_archive_observes_cancel_before_walking_tree() {
        let root =
            std::env::temp_dir().join(format!("vpsman-file-download-cancel-{}", Uuid::new_v4()));
        let nested = root.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("keep.txt"), "keep").unwrap();
        let cancel_token = CommandCancelToken::default();
        cancel_token.cancel("operator canceled".to_string());

        let result =
            build_directory_download_artifact(&root, MAX_DIRECT_FILE_DOWNLOAD_BYTES, &cancel_token);

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(nested.join("keep.txt")).unwrap(), "keep");
        let _ = fs::remove_dir_all(root);
    }
}
