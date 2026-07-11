use std::{
    collections::HashSet,
    fs,
    io::{self, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::{
    sync::mpsc,
    time::{self, Duration},
};
use vpsman_common::{
    payload_hash, validate_absolute_file_path, AgentConfig, BackupMissingPathPolicy, CommandOutput,
    OutputStream,
};

use crate::command_worker::{run_cancelable, CommandCancelToken};
use crate::telemetry::unix_now;

pub(crate) const BACKUP_ARCHIVE_FORMAT: &str = "vpsman.backup_tar.v1";
pub(crate) const BACKUP_ARCHIVE_MANIFEST_PATH: &str = "vpsman-backup/manifest.json";
const MAX_BACKUP_PATHS: usize = 64;
const MAX_BACKUP_FILES: usize = 16_384;
const MAX_BACKUP_SCANNED_PATHS: usize = 65_536;
const MAX_REPORTED_SKIPPED_PATHS: usize = 256;

struct LimitedVecWriter {
    inner: Vec<u8>,
    written: u64,
    max_bytes: u64,
}

impl LimitedVecWriter {
    fn new(max_bytes: u64) -> Self {
        Self {
            inner: Vec::new(),
            written: 0,
            max_bytes,
        }
    }

    fn into_inner(self) -> Vec<u8> {
        self.inner
    }
}

impl Write for LimitedVecWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let next = self
            .written
            .checked_add(buf.len() as u64)
            .ok_or_else(|| io::Error::other("backup archive size overflow"))?;
        if next > self.max_bytes {
            return Err(io::Error::other(
                "backup archive exceeds configured archive byte limit",
            ));
        }
        self.inner.extend_from_slice(buf);
        self.written = next;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct BackupArchive {
    pub(crate) format: String,
    pub(crate) client_id: String,
    pub(crate) created_unix: u64,
    pub(crate) files: Vec<BackupFileEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct BackupFileEntry {
    pub(crate) path: String,
    pub(crate) source: BackupFileSource,
    pub(crate) tar_path: String,
    pub(crate) mode: u32,
    pub(crate) size_bytes: u64,
    pub(crate) sha256_hex: String,
    pub(crate) mtime_unix: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BackupFileSource {
    SelectedPath,
    AgentConfig,
}

struct BackupFilePayload {
    entry: BackupFileEntry,
    data: Vec<u8>,
}

#[derive(Debug, Serialize)]
struct BackupSkippedPath {
    path: String,
    reason: &'static str,
}

struct BackupCollection {
    files: Vec<BackupFilePayload>,
    directory_count: usize,
    scanned_path_count: usize,
    skipped_path_count: usize,
    skipped_paths: Vec<BackupSkippedPath>,
}

impl BackupCollection {
    fn record_skip(&mut self, path: &Path, reason: &'static str) {
        self.skipped_path_count = self.skipped_path_count.saturating_add(1);
        if self.skipped_paths.len() < MAX_REPORTED_SKIPPED_PATHS {
            self.skipped_paths.push(BackupSkippedPath {
                path: path.display().to_string(),
                reason,
            });
        }
    }
}

pub(crate) struct BackupCommandInput<'a> {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) config: &'a AgentConfig,
    pub(crate) config_path: &'a Path,
    pub(crate) paths: &'a [String],
    pub(crate) include_config: bool,
    pub(crate) follow_symlinks: bool,
    pub(crate) missing_path_policy: BackupMissingPathPolicy,
    pub(crate) output_tx: Option<mpsc::Sender<CommandOutput>>,
    pub(crate) max_timeout_secs: u64,
    pub(crate) cancel_token: CommandCancelToken,
}

pub(crate) async fn execute_backup_command(
    input: BackupCommandInput<'_>,
) -> Result<Vec<CommandOutput>> {
    let BackupCommandInput {
        job_id,
        config,
        config_path,
        paths,
        include_config,
        follow_symlinks,
        missing_path_policy,
        output_tx,
        max_timeout_secs,
        cancel_token,
    } = input;
    run_cancelable("backup", cancel_token.clone(), async move {
        time::timeout(
            Duration::from_secs(max_timeout_secs.max(1)),
            create_backup_archive_artifact(
                job_id,
                config,
                config_path,
                paths,
                include_config,
                follow_symlinks,
                missing_path_policy,
                output_tx,
                cancel_token,
            ),
        )
        .await
        .context("backup timed out")?
    })
    .await
}

async fn create_backup_archive_artifact(
    job_id: uuid::Uuid,
    config: &AgentConfig,
    config_path: &Path,
    paths: &[String],
    include_config: bool,
    follow_symlinks: bool,
    missing_path_policy: BackupMissingPathPolicy,
    output_tx: Option<mpsc::Sender<CommandOutput>>,
    cancel_token: CommandCancelToken,
) -> Result<Vec<CommandOutput>> {
    cancel_token.check("backup")?;
    validate_backup_scope(paths, include_config)?;
    let collection = collect_backup_files(
        config_path,
        paths,
        include_config,
        follow_symlinks,
        missing_path_policy,
        config.backup.max_uncompressed_bytes,
        &cancel_token,
    )
    .await?;
    cancel_token.check("backup")?;
    let BackupCollection {
        files,
        directory_count,
        scanned_path_count,
        skipped_path_count,
        skipped_paths,
    } = collection;
    anyhow::ensure!(!files.is_empty(), "backup scope captured no regular files");
    let file_count = files.len();
    let created_unix = unix_now();
    let archive = BackupArchive {
        format: BACKUP_ARCHIVE_FORMAT.to_string(),
        client_id: config.client_id.clone(),
        created_unix,
        files: files.iter().map(|file| file.entry.clone()).collect(),
    };
    let plaintext = encode_backup_tar_archive(&archive, &files, config.backup.max_archive_bytes)
        .context("failed to encode backup tar archive")?;
    cancel_token.check("backup")?;
    if plaintext.len() as u64 > config.backup.max_archive_bytes {
        anyhow::bail!(
            "backup archive exceeds archive limit: {} > {} bytes",
            plaintext.len(),
            config.backup.max_archive_bytes
        );
    }
    let artifact_bytes = plaintext;
    let (mut outputs, streamed, chunk_count, chunk_bytes, artifact_sha256_hex) =
        if let Some(output_tx) = output_tx {
            let summary = super::file_pull::stream_buffered_payload_output(
                job_id,
                OutputStream::Stdout,
                &artifact_bytes,
                output_tx,
                "backup artifact output receiver dropped",
            )
            .await?;
            cancel_token.check("backup")?;
            (
                Vec::new(),
                true,
                summary.chunk_count,
                summary.chunk_bytes,
                summary.sha256_hex,
            )
        } else {
            (
                super::file_pull::chunked_output(job_id, OutputStream::Stdout, &artifact_bytes),
                false,
                artifact_bytes
                    .chunks(super::file_pull::COMMAND_OUTPUT_CHUNK_BYTES)
                    .count() as u64,
                super::file_pull::COMMAND_OUTPUT_CHUNK_BYTES,
                payload_hash(&artifact_bytes),
            )
        };
    let status = serde_json::json!({
        "type": "backup",
        "format": BACKUP_ARCHIVE_FORMAT,
        "archive_format": BACKUP_ARCHIVE_FORMAT,
        "paths": paths,
        "include_config": include_config,
        "follow_symlinks": follow_symlinks,
        "missing_path_policy": missing_path_policy.as_str(),
        "file_count": file_count,
        "directory_count": directory_count,
        "scanned_path_count": scanned_path_count,
        "skipped_path_count": skipped_path_count,
        "skipped_paths": skipped_paths,
        "skipped_paths_truncated": skipped_path_count > MAX_REPORTED_SKIPPED_PATHS,
        "artifact_size_bytes": artifact_bytes.len(),
        "artifact_sha256_hex": artifact_sha256_hex,
        "chunk_bytes": chunk_bytes,
        "chunk_count": chunk_count,
        "streamed": streamed,
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

async fn collect_backup_files(
    config_path: &Path,
    paths: &[String],
    include_config: bool,
    follow_symlinks: bool,
    missing_path_policy: BackupMissingPathPolicy,
    max_uncompressed_bytes: u64,
    cancel_token: &CommandCancelToken,
) -> Result<BackupCollection> {
    let config_path = config_path.to_path_buf();
    let paths = paths.to_vec();
    let cancel_token = cancel_token.clone();
    tokio::task::spawn_blocking(move || {
        collect_backup_files_blocking(
            &config_path,
            &paths,
            include_config,
            follow_symlinks,
            missing_path_policy,
            max_uncompressed_bytes,
            &cancel_token,
        )
    })
    .await
    .context("backup collection worker failed")?
}

fn collect_backup_files_blocking(
    config_path: &Path,
    paths: &[String],
    include_config: bool,
    follow_symlinks: bool,
    missing_path_policy: BackupMissingPathPolicy,
    max_uncompressed_bytes: u64,
    cancel_token: &CommandCancelToken,
) -> Result<BackupCollection> {
    let mut collection = BackupCollection {
        files: Vec::new(),
        directory_count: 0,
        scanned_path_count: 0,
        skipped_path_count: 0,
        skipped_paths: Vec::new(),
    };
    let mut total_bytes = 0_u64;
    let mut visited_directories = HashSet::<(u64, u64)>::new();
    let mut seen_file_paths = HashSet::<PathBuf>::new();
    for path in paths {
        cancel_token.check("backup")?;
        collect_selected_path(
            Path::new(path),
            true,
            false,
            follow_symlinks,
            missing_path_policy,
            &mut collection,
            &mut total_bytes,
            max_uncompressed_bytes,
            &mut visited_directories,
            &mut seen_file_paths,
            cancel_token,
        )?;
    }
    if include_config {
        cancel_token.check("backup")?;
        anyhow::ensure!(
            collection.files.len() < MAX_BACKUP_FILES,
            "backup file count limit exceeded"
        );
        collection.files.push(read_backup_file(
            config_path,
            "vpsman:agent_config",
            BackupFileSource::AgentConfig,
            collection.files.len(),
            true,
            &mut total_bytes,
            max_uncompressed_bytes,
            cancel_token,
        )?);
    }
    Ok(collection)
}

#[allow(clippy::too_many_arguments)]
fn collect_selected_path(
    path: &Path,
    selected_root: bool,
    path_counted: bool,
    follow_symlinks: bool,
    missing_path_policy: BackupMissingPathPolicy,
    collection: &mut BackupCollection,
    total_bytes: &mut u64,
    max_uncompressed_bytes: u64,
    visited_directories: &mut HashSet<(u64, u64)>,
    seen_file_paths: &mut HashSet<PathBuf>,
    cancel_token: &CommandCancelToken,
) -> Result<()> {
    cancel_token.check("backup")?;
    if !path_counted {
        anyhow::ensure!(
            collection.scanned_path_count < MAX_BACKUP_SCANNED_PATHS,
            "backup scanned path limit exceeded"
        );
        collection.scanned_path_count = collection.scanned_path_count.saturating_add(1);
    }
    let link_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error)
            if selected_root
                && error.kind() == io::ErrorKind::NotFound
                && missing_path_policy == BackupMissingPathPolicy::Skip =>
        {
            collection.record_skip(path, "missing");
            return Ok(());
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to stat backup path {}", path.display()));
        }
    };
    let is_symlink = link_metadata.file_type().is_symlink();
    if is_symlink && !follow_symlinks {
        if selected_root {
            anyhow::bail!(
                "backup path is a symlink; set follow_symlinks to use the target: {}",
                path.display()
            );
        }
        collection.record_skip(path, "symlink_not_followed");
        return Ok(());
    }
    let metadata = if is_symlink {
        fs::metadata(path)
            .with_context(|| format!("failed to follow backup path {}", path.display()))?
    } else {
        link_metadata
    };
    cancel_token.check("backup")?;
    if metadata.is_file() {
        if !seen_file_paths.insert(path.to_path_buf()) {
            return Ok(());
        }
        anyhow::ensure!(
            collection.files.len() < MAX_BACKUP_FILES,
            "backup file count limit exceeded"
        );
        let archive_path = path.display().to_string();
        collection.files.push(read_backup_file(
            path,
            &archive_path,
            BackupFileSource::SelectedPath,
            collection.files.len(),
            follow_symlinks,
            total_bytes,
            max_uncompressed_bytes,
            cancel_token,
        )?);
        return Ok(());
    }
    if metadata.is_dir() {
        let identity = (metadata.dev(), metadata.ino());
        if !visited_directories.insert(identity) {
            collection.record_skip(path, "directory_already_visited");
            return Ok(());
        }
        collection.directory_count = collection.directory_count.saturating_add(1);
        let mut entries = Vec::new();
        for entry in fs::read_dir(path)
            .with_context(|| format!("failed to read backup directory {}", path.display()))?
        {
            anyhow::ensure!(
                collection.scanned_path_count < MAX_BACKUP_SCANNED_PATHS,
                "backup scanned path limit exceeded"
            );
            collection.scanned_path_count = collection.scanned_path_count.saturating_add(1);
            entries.push(entry?);
        }
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            collect_selected_path(
                &entry.path(),
                false,
                true,
                follow_symlinks,
                missing_path_policy,
                collection,
                total_bytes,
                max_uncompressed_bytes,
                visited_directories,
                seen_file_paths,
                cancel_token,
            )?;
        }
        return Ok(());
    }
    if selected_root {
        anyhow::bail!(
            "backup path is not a regular file or directory: {}",
            path.display()
        );
    }
    collection.record_skip(path, "special_file");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn read_backup_file(
    path: &Path,
    archive_path: &str,
    source: BackupFileSource,
    tar_index: usize,
    follow_symlinks: bool,
    total_bytes: &mut u64,
    max_uncompressed_bytes: u64,
    cancel_token: &CommandCancelToken,
) -> Result<BackupFilePayload> {
    cancel_token.check("backup")?;
    if matches!(source, BackupFileSource::SelectedPath) {
        validate_absolute_file_path(archive_path)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    }
    let opened = crate::safe_file::open_regular_file_for_read(
        path,
        follow_symlinks,
        "backup path is a symlink; set follow_symlinks to use the target",
    )
    .with_context(|| format!("failed to open backup path {}", path.display()))?;
    let metadata = opened.metadata;
    cancel_token.check("backup")?;
    let remaining = max_uncompressed_bytes.saturating_sub(*total_bytes);
    if metadata.len() > remaining {
        anyhow::bail!(
            "backup scope exceeds uncompressed payload limit: {} > {} bytes",
            (*total_bytes).saturating_add(metadata.len()),
            max_uncompressed_bytes
        );
    }
    let data = crate::safe_file::read_opened_file_bounded(
        opened.file,
        remaining,
        "backup file exceeds remaining uncompressed payload limit while reading",
    )?;
    *total_bytes = total_bytes
        .checked_add(data.len() as u64)
        .context("backup size overflow")?;
    cancel_token.check("backup")?;
    let mtime_unix = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs());
    Ok(BackupFilePayload {
        entry: BackupFileEntry {
            path: archive_path.to_string(),
            source,
            tar_path: format!("vpsman-backup/files/{tar_index:04}.bin"),
            mode: metadata.permissions().mode() & 0o777,
            size_bytes: data.len() as u64,
            sha256_hex: payload_hash(&data),
            mtime_unix,
        },
        data,
    })
}

fn encode_backup_tar_archive(
    archive: &BackupArchive,
    files: &[BackupFilePayload],
    max_archive_bytes: u64,
) -> Result<Vec<u8>> {
    let mut builder = tar::Builder::new(LimitedVecWriter::new(max_archive_bytes));
    let manifest =
        serde_json::to_vec(archive).context("failed to encode backup archive manifest")?;
    append_tar_bytes(
        &mut builder,
        BACKUP_ARCHIVE_MANIFEST_PATH,
        0o600,
        archive.created_unix,
        &manifest,
    )?;
    for file in files {
        append_tar_bytes(
            &mut builder,
            &file.entry.tar_path,
            file.entry.mode,
            file.entry.mtime_unix.unwrap_or(archive.created_unix),
            &file.data,
        )?;
    }
    builder
        .finish()
        .context("failed to finish backup tar archive")?;
    Ok(builder
        .into_inner()
        .context("failed to collect backup tar archive")?
        .into_inner())
}

fn append_tar_bytes<W: Write>(
    builder: &mut tar::Builder<W>,
    path: &str,
    mode: u32,
    mtime_unix: u64,
    bytes: &[u8],
) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(mode);
    header.set_mtime(mtime_unix);
    header.set_cksum();
    builder
        .append_data(&mut header, path, bytes)
        .with_context(|| format!("failed to append backup tar entry {path}"))
}

fn validate_backup_scope(paths: &[String], include_config: bool) -> Result<()> {
    if paths.len() > MAX_BACKUP_PATHS {
        anyhow::bail!("backup path limit exceeded");
    }
    if !include_config && paths.is_empty() {
        anyhow::bail!("backup scope is empty");
    }
    for path in paths {
        validate_absolute_file_path(path).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;
    use vpsman_common::AgentBackupConfig;

    #[tokio::test]
    async fn creates_plain_backup_tar_artifact() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-backup-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let selected_path = dir.join("selected.txt");
        let config_path = dir.join("agent.toml");
        tokio::fs::write(&selected_path, b"selected secret contents")
            .await
            .unwrap();
        tokio::fs::write(&config_path, b"noise_client_private_key_hex = \"secret\"")
            .await
            .unwrap();
        let mut config = AgentConfig {
            client_id: "client-a".to_string(),
            backup: AgentBackupConfig {
                max_uncompressed_bytes: 8192,
                max_archive_bytes: 16 * 1024,
            },
            ..AgentConfig::default()
        };
        config.display_name = "client-a".to_string();

        let paths = vec![selected_path.to_string_lossy().to_string()];
        let outputs = execute_backup_command(BackupCommandInput {
            job_id,
            config: &config,
            config_path: &config_path,
            paths: &paths,
            include_config: true,
            follow_symlinks: false,
            missing_path_policy: BackupMissingPathPolicy::Fail,
            output_tx: None,
            max_timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap();
        let artifact_bytes = outputs
            .iter()
            .filter(|output| output.stream == OutputStream::Stdout)
            .flat_map(|output| output.data.clone())
            .collect::<Vec<_>>();
        let archive = manifest_from_tar(&artifact_bytes);
        assert_eq!(archive.format, BACKUP_ARCHIVE_FORMAT);
        assert_eq!(archive.client_id, "client-a");
        assert_eq!(archive.files.len(), 2);
        assert!(archive
            .files
            .iter()
            .any(|file| file.path == selected_path.to_string_lossy().as_ref()
                && file.sha256_hex == payload_hash(b"selected secret contents")));
        let status = outputs.iter().find(|output| output.done).unwrap();
        let status: serde_json::Value = serde_json::from_slice(&status.data).unwrap();
        assert_eq!(status["type"], "backup");
        assert_eq!(status["file_count"], 2);
        assert_eq!(status["artifact_sha256_hex"], payload_hash(&artifact_bytes));

        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn backup_rejects_symlink_paths_by_default() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-backup-symlink-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let target_path = dir.join("target.txt");
        let symlink_path = dir.join("linked.txt");
        let config_path = dir.join("agent.toml");
        tokio::fs::write(&target_path, b"target contents")
            .await
            .unwrap();
        tokio::fs::write(&config_path, b"client_id = \"client-a\"")
            .await
            .unwrap();
        symlink(&target_path, &symlink_path).unwrap();
        let config = AgentConfig {
            client_id: "client-a".to_string(),
            backup: AgentBackupConfig {
                max_uncompressed_bytes: 8192,
                max_archive_bytes: 16 * 1024,
            },
            ..AgentConfig::default()
        };
        let paths = vec![symlink_path.to_string_lossy().to_string()];

        let error = execute_backup_command(BackupCommandInput {
            job_id,
            config: &config,
            config_path: &config_path,
            paths: &paths,
            include_config: false,
            follow_symlinks: false,
            missing_path_policy: BackupMissingPathPolicy::Fail,
            output_tx: None,
            max_timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap_err();
        assert!(error
            .chain()
            .any(|cause| cause.to_string().contains("backup path is a symlink")));

        let outputs = execute_backup_command(BackupCommandInput {
            job_id,
            config: &config,
            config_path: &config_path,
            paths: &paths,
            include_config: false,
            follow_symlinks: true,
            missing_path_policy: BackupMissingPathPolicy::Fail,
            output_tx: None,
            max_timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap();
        let artifact_bytes = outputs
            .iter()
            .filter(|output| output.stream == OutputStream::Stdout)
            .flat_map(|output| output.data.clone())
            .collect::<Vec<_>>();
        let archive = manifest_from_tar(&artifact_bytes);
        assert_eq!(archive.files.len(), 1);
        assert_eq!(archive.files[0].path, symlink_path.to_string_lossy());
        assert_eq!(
            archive.files[0].sha256_hex,
            payload_hash(b"target contents")
        );

        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn backup_recurses_directories_and_reports_tolerated_omissions() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-backup-tree-{job_id}"));
        let root = dir.join("config");
        let nested = root.join("service");
        tokio::fs::create_dir_all(&nested).await.unwrap();
        let root_file = root.join("root.conf");
        let nested_file = nested.join("service.conf");
        let nested_link = nested.join("linked.conf");
        let missing = dir.join("optional-missing");
        tokio::fs::write(&root_file, b"root=true\n").await.unwrap();
        tokio::fs::write(&nested_file, b"enabled=true\n")
            .await
            .unwrap();
        symlink(&root_file, &nested_link).unwrap();

        let config = AgentConfig {
            client_id: "client-tree".to_string(),
            backup: AgentBackupConfig {
                max_uncompressed_bytes: 8192,
                max_archive_bytes: 32 * 1024,
            },
            ..AgentConfig::default()
        };
        let paths = vec![
            root.to_string_lossy().to_string(),
            missing.to_string_lossy().to_string(),
        ];
        let outputs = execute_backup_command(BackupCommandInput {
            job_id,
            config: &config,
            config_path: &root_file,
            paths: &paths,
            include_config: false,
            follow_symlinks: false,
            missing_path_policy: BackupMissingPathPolicy::Skip,
            output_tx: None,
            max_timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap();
        let artifact_bytes = outputs
            .iter()
            .filter(|output| output.stream == OutputStream::Stdout)
            .flat_map(|output| output.data.clone())
            .collect::<Vec<_>>();
        let archive = manifest_from_tar(&artifact_bytes);
        let archived_paths = archive
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(archive.files.len(), 2);
        assert!(archived_paths.contains(&root_file.to_string_lossy().as_ref()));
        assert!(archived_paths.contains(&nested_file.to_string_lossy().as_ref()));
        assert!(!archived_paths.contains(&nested_link.to_string_lossy().as_ref()));

        let status = outputs.iter().find(|output| output.done).unwrap();
        let status: serde_json::Value = serde_json::from_slice(&status.data).unwrap();
        assert_eq!(status["missing_path_policy"], "skip");
        assert_eq!(status["directory_count"], 2);
        assert_eq!(status["skipped_path_count"], 2);
        assert_eq!(status["skipped_paths"][0]["reason"], "symlink_not_followed");
        assert_eq!(status["skipped_paths"][1]["reason"], "missing");

        let error = execute_backup_command(BackupCommandInput {
            job_id: uuid::Uuid::new_v4(),
            config: &config,
            config_path: &root_file,
            paths: &paths,
            include_config: false,
            follow_symlinks: false,
            missing_path_policy: BackupMissingPathPolicy::Fail,
            output_tx: None,
            max_timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap_err();
        assert!(error
            .chain()
            .any(|cause| cause.to_string().contains("failed to stat backup path")));

        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn streams_backup_artifact_through_payload_sink() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-backup-stream-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let selected_path = dir.join("selected.bin");
        let selected_data = (0..8192)
            .map(|value| (value % 251) as u8)
            .collect::<Vec<_>>();
        let config_path = dir.join("agent.toml");
        tokio::fs::write(&selected_path, &selected_data)
            .await
            .unwrap();
        tokio::fs::write(&config_path, b"noise_client_private_key_hex = \"secret\"")
            .await
            .unwrap();
        let mut config = AgentConfig {
            client_id: "client-stream".to_string(),
            backup: AgentBackupConfig {
                max_uncompressed_bytes: 64 * 1024,
                max_archive_bytes: 128 * 1024,
            },
            ..AgentConfig::default()
        };
        config.display_name = "client-stream".to_string();
        let (tx, mut rx) = mpsc::channel(64);

        let paths = vec![selected_path.to_string_lossy().to_string()];
        let outputs = execute_backup_command(BackupCommandInput {
            job_id,
            config: &config,
            config_path: &config_path,
            paths: &paths,
            include_config: true,
            follow_symlinks: false,
            missing_path_policy: BackupMissingPathPolicy::Fail,
            output_tx: Some(tx),
            max_timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap();

        let mut artifact_bytes = Vec::new();
        while let Some(output) = rx.recv().await {
            assert_eq!(output.stream, OutputStream::Stdout);
            assert!(!output.done);
            artifact_bytes.extend_from_slice(&output.data);
        }
        assert!(!artifact_bytes.is_empty());
        assert!(outputs
            .iter()
            .all(|output| output.stream == OutputStream::Status));
        let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
        assert_eq!(status["type"], "backup");
        assert_eq!(status["streamed"], true);
        assert_eq!(status["artifact_sha256_hex"], payload_hash(&artifact_bytes));
        assert!(status["chunk_count"].as_u64().unwrap() >= 1);

        let archive = manifest_from_tar(&artifact_bytes);
        assert_eq!(archive.client_id, "client-stream");
        assert_eq!(archive.files.len(), 2);

        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn backup_rejects_unsafe_scope_and_size_limits() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-backup-reject-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let file_path = dir.join("selected.txt");
        tokio::fs::write(&file_path, b"contents").await.unwrap();

        let paths = vec![file_path.to_string_lossy().to_string()];
        let config = AgentConfig {
            backup: AgentBackupConfig {
                max_uncompressed_bytes: 4,
                max_archive_bytes: 1024,
            },
            ..AgentConfig::default()
        };
        let relative_paths = vec!["relative".to_string()];
        let relative = execute_backup_command(BackupCommandInput {
            job_id,
            config: &config,
            config_path: &file_path,
            paths: &relative_paths,
            include_config: false,
            follow_symlinks: false,
            missing_path_policy: BackupMissingPathPolicy::Fail,
            output_tx: None,
            max_timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap_err();
        assert!(relative.to_string().contains("file path must be absolute"));

        let too_large = execute_backup_command(BackupCommandInput {
            job_id,
            config: &config,
            config_path: &file_path,
            paths: &paths,
            include_config: false,
            follow_symlinks: false,
            missing_path_policy: BackupMissingPathPolicy::Fail,
            output_tx: None,
            max_timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap_err();
        assert!(too_large.to_string().contains("uncompressed payload limit"));

        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn backup_rejects_archive_overhead_above_archive_limit() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-backup-archive-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let file_path = dir.join("selected.txt");
        tokio::fs::write(&file_path, b"small").await.unwrap();

        let paths = vec![file_path.to_string_lossy().to_string()];
        let config = AgentConfig {
            backup: AgentBackupConfig {
                max_uncompressed_bytes: 512,
                max_archive_bytes: 512,
            },
            ..AgentConfig::default()
        };

        let archive_too_large = execute_backup_command(BackupCommandInput {
            job_id,
            config: &config,
            config_path: &file_path,
            paths: &paths,
            include_config: false,
            follow_symlinks: false,
            missing_path_policy: BackupMissingPathPolicy::Fail,
            output_tx: None,
            max_timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap_err();
        assert!(archive_too_large
            .chain()
            .any(|cause| cause.to_string().contains("archive byte limit")));

        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    fn manifest_from_tar(bytes: &[u8]) -> BackupArchive {
        let mut tar_archive = tar::Archive::new(std::io::Cursor::new(bytes));
        for entry in tar_archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            if entry.path().unwrap().to_string_lossy() == BACKUP_ARCHIVE_MANIFEST_PATH {
                let mut data = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut data).unwrap();
                return serde_json::from_slice(&data).unwrap();
            }
        }
        panic!("backup manifest missing")
    }
}
