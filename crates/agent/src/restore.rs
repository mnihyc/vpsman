use std::{
    collections::HashMap,
    io::Cursor,
    os::unix::fs::PermissionsExt,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::Serialize;
use tokio::{
    process::Command,
    time::{self, Duration},
};
use vpsman_common::{
    decode_inline_file_payload, payload_hash, validate_absolute_file_path, validate_file_mode,
    CommandOutput, OutputStream,
};

use crate::backup::{
    BackupArchive, BackupFileEntry, BackupFileSource, BACKUP_ARCHIVE_FORMAT,
    BACKUP_ARCHIVE_MANIFEST_PATH,
};

#[derive(Debug, Serialize)]
struct RestoredFileStatus {
    archive_path: String,
    destination_path: String,
    source: &'static str,
    size_bytes: u64,
    sha256_hex: String,
    mode: u32,
    rollback_path: Option<String>,
}

#[derive(Debug)]
struct AppliedRestore {
    status: RestoredFileStatus,
    destination_path: PathBuf,
    rollback: Option<RollbackSnapshot>,
}

#[derive(Debug)]
struct RollbackSnapshot {
    path: PathBuf,
    mode: u32,
}

struct DecodedBackupArchive {
    client_id: String,
    files: Vec<DecodedBackupFile>,
}

struct DecodedBackupFile {
    entry: BackupFileEntry,
    data: Vec<u8>,
}

#[derive(Debug, serde::Deserialize)]
struct LegacyBackupArchive {
    format: String,
    client_id: String,
    files: Vec<LegacyBackupFileEntry>,
}

#[derive(Debug, serde::Deserialize)]
struct LegacyBackupFileEntry {
    path: String,
    source: BackupFileSource,
    mode: u32,
    size_bytes: u64,
    sha256_hex: String,
    data_base64: String,
}

pub(crate) struct RestoreCommandInput<'a> {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) source_backup_request_id: uuid::Uuid,
    pub(crate) paths: &'a [String],
    pub(crate) include_config: bool,
    pub(crate) destination_root: Option<&'a str>,
    pub(crate) archive_path: Option<&'a str>,
    pub(crate) archive_base64: Option<&'a str>,
    pub(crate) archive_size_bytes: Option<u64>,
    pub(crate) archive_sha256_hex: Option<&'a str>,
    pub(crate) dry_run: bool,
    pub(crate) post_restore_argv: &'a [String],
    pub(crate) timeout_secs: u64,
}

pub(crate) async fn execute_restore_command(
    input: RestoreCommandInput<'_>,
) -> Result<Vec<CommandOutput>> {
    let deadline = time::Instant::now() + Duration::from_secs(input.timeout_secs.max(1));
    restore_archive(
        input.job_id,
        input.source_backup_request_id,
        input.paths,
        input.include_config,
        input.destination_root,
        input.archive_path,
        input.archive_base64,
        input.archive_size_bytes,
        input.archive_sha256_hex,
        input.dry_run,
        input.post_restore_argv,
        deadline,
    )
    .await
}

async fn restore_archive(
    job_id: uuid::Uuid,
    source_backup_request_id: uuid::Uuid,
    paths: &[String],
    include_config: bool,
    destination_root: Option<&str>,
    archive_path: Option<&str>,
    archive_base64: Option<&str>,
    archive_size_bytes: Option<u64>,
    archive_sha256_hex: Option<&str>,
    dry_run: bool,
    post_restore_argv: &[String],
    deadline: time::Instant,
) -> Result<Vec<CommandOutput>> {
    validate_restore_scope(paths, include_config, destination_root)?;
    validate_post_restore_argv(post_restore_argv)?;
    ensure_restore_deadline(deadline)?;
    let archive_bytes = archive_bytes_from_source(
        archive_path,
        archive_base64,
        archive_size_bytes,
        archive_sha256_hex,
    )
    .await?;
    let archive = decode_backup_archive(&archive_bytes)?;

    let mut restored = Vec::new();
    let mut matched_count = 0_usize;
    for entry in archive.files {
        ensure_restore_deadline(deadline)?;
        if !entry_requested(&entry.entry, paths, include_config) {
            continue;
        }
        matched_count += 1;
        if dry_run {
            validate_restore_entry(&entry.entry, &entry.data, destination_root)?;
            continue;
        }
        match restore_entry(job_id, &entry, destination_root).await {
            Ok(applied) => {
                restored.push(applied);
                if let Err(error) = ensure_restore_deadline(deadline) {
                    if let Err(rollback_error) = rollback_applied_restores(&restored).await {
                        return Err(error).with_context(|| {
                            format!(
                                "restore timed out after {} files and automatic rollback failed: {rollback_error}",
                                restored.len()
                            )
                        });
                    }
                    return Err(error).with_context(|| {
                        format!(
                            "restore timed out after {} files; applied files were rolled back",
                            restored.len()
                        )
                    });
                }
            }
            Err(error) => {
                if let Err(rollback_error) = rollback_applied_restores(&restored).await {
                    return Err(error).with_context(|| {
                        format!(
                            "restore failed after {} files and automatic rollback failed: {rollback_error}",
                            restored.len()
                        )
                    });
                }
                return Err(error).with_context(|| {
                    format!(
                        "restore failed after {} files; applied files were rolled back",
                        restored.len()
                    )
                });
            }
        }
    }
    if matched_count == 0 {
        anyhow::bail!("restore scope matched no archive entries");
    }
    if dry_run {
        let status = serde_json::json!({
            "type": "restore",
            "status": "rehearsed",
            "source_backup_request_id": source_backup_request_id,
            "source_client_id": archive.client_id,
            "archive_source": archive_source_label(archive_path),
            "archive_sha256_hex": payload_hash(&archive_bytes),
            "requested_paths": paths,
            "include_config": include_config,
            "destination_root": destination_root,
            "matched_count": matched_count,
            "restored_count": 0,
            "dry_run": true,
            "post_restore": post_restore_status(post_restore_argv, None, true),
            "rollback_available": false,
        });
        return Ok(vec![CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&status)?,
            exit_code: Some(0),
            done: true,
        }]);
    }
    let restored_files = restored
        .into_iter()
        .map(|applied| applied.status)
        .collect::<Vec<_>>();
    ensure_restore_deadline(deadline)?;
    let post_restore_timeout_secs = deadline
        .saturating_duration_since(time::Instant::now())
        .as_secs()
        .max(1);
    let post_restore = run_post_restore_argv(post_restore_argv, post_restore_timeout_secs).await?;

    let status = serde_json::json!({
        "type": "restore",
        "status": "restored",
        "source_backup_request_id": source_backup_request_id,
        "source_client_id": archive.client_id,
        "archive_source": archive_source_label(archive_path),
        "archive_sha256_hex": payload_hash(&archive_bytes),
        "requested_paths": paths,
        "include_config": include_config,
        "destination_root": destination_root,
        "matched_count": matched_count,
        "restored_count": restored_files.len(),
        "restored_files": restored_files,
        "dry_run": false,
        "post_restore": post_restore,
        "rollback_available": true,
    });
    Ok(vec![CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: Some(0),
        done: true,
    }])
}

fn ensure_restore_deadline(deadline: time::Instant) -> Result<()> {
    if time::Instant::now() >= deadline {
        anyhow::bail!("restore command timed out");
    }
    Ok(())
}

async fn archive_bytes_from_source(
    archive_path: Option<&str>,
    archive_base64: Option<&str>,
    archive_size_bytes: Option<u64>,
    archive_sha256_hex: Option<&str>,
) -> Result<Vec<u8>> {
    match archive_path {
        Some(path) => {
            validate_safe_absolute_path(path)?;
            let bytes = tokio::fs::read(path)
                .await
                .with_context(|| format!("failed to read restore archive {path}"))?;
            if let Some(expected_size) = archive_size_bytes {
                anyhow::ensure!(
                    bytes.len() as u64 == expected_size,
                    "restore archive size mismatch"
                );
            }
            if let Some(expected_sha256_hex) = archive_sha256_hex {
                anyhow::ensure!(
                    payload_hash(&bytes) == expected_sha256_hex,
                    "restore archive sha256 mismatch"
                );
            }
            Ok(bytes)
        }
        None => {
            let archive_base64 = archive_base64.context("restore archive is required")?;
            let archive_size_bytes =
                archive_size_bytes.context("restore archive size is required")?;
            let archive_sha256_hex =
                archive_sha256_hex.context("restore archive sha256 is required")?;
            decode_inline_file_payload(archive_base64, archive_size_bytes, archive_sha256_hex)
                .map_err(|error| anyhow::anyhow!(error.to_string()))
        }
    }
}

fn decode_backup_archive(bytes: &[u8]) -> Result<DecodedBackupArchive> {
    if bytes.first() == Some(&b'{') {
        return decode_legacy_json_archive(bytes);
    }
    decode_tar_archive(bytes)
}

fn decode_legacy_json_archive(bytes: &[u8]) -> Result<DecodedBackupArchive> {
    let archive: LegacyBackupArchive =
        serde_json::from_slice(bytes).context("legacy restore archive JSON is invalid")?;
    if archive.format != "vpsman.backup_archive.v1" {
        anyhow::bail!("restore archive format is invalid");
    }
    let files = archive
        .files
        .into_iter()
        .enumerate()
        .map(|(index, entry)| {
            let data = BASE64_STANDARD
                .decode(&entry.data_base64)
                .with_context(|| {
                    format!("restore archive entry {} has invalid base64", entry.path)
                })?;
            let decoded = BackupFileEntry {
                path: entry.path,
                source: entry.source,
                tar_path: format!("legacy/{index:04}.bin"),
                mode: entry.mode,
                size_bytes: entry.size_bytes,
                sha256_hex: entry.sha256_hex,
                mtime_unix: None,
            };
            validate_restore_entry_payload(&decoded, &data)?;
            Ok(DecodedBackupFile {
                entry: decoded,
                data,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(DecodedBackupArchive {
        client_id: archive.client_id,
        files,
    })
}

fn decode_tar_archive(bytes: &[u8]) -> Result<DecodedBackupArchive> {
    let mut manifest = None::<BackupArchive>;
    let mut payloads = HashMap::<String, Vec<u8>>::new();
    let mut archive = tar::Archive::new(Cursor::new(bytes));
    for entry in archive
        .entries()
        .context("restore tar archive is invalid")?
    {
        let mut entry = entry.context("restore tar entry is invalid")?;
        let path = entry
            .path()
            .context("restore tar entry path is invalid")?
            .to_string_lossy()
            .to_string();
        let mut data = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut data)
            .context("failed to read restore tar entry")?;
        if path == BACKUP_ARCHIVE_MANIFEST_PATH {
            manifest = Some(
                serde_json::from_slice(&data).context("restore tar manifest JSON is invalid")?,
            );
        } else {
            payloads.insert(path, data);
        }
    }
    let manifest = manifest.context("restore tar manifest is missing")?;
    if manifest.format != BACKUP_ARCHIVE_FORMAT {
        anyhow::bail!("restore archive format is invalid");
    }
    let mut files = Vec::with_capacity(manifest.files.len());
    for entry in manifest.files {
        let data = payloads
            .remove(&entry.tar_path)
            .with_context(|| format!("restore tar payload {} is missing", entry.tar_path))?;
        validate_restore_entry_payload(&entry, &data)?;
        files.push(DecodedBackupFile { entry, data });
    }
    Ok(DecodedBackupArchive {
        client_id: manifest.client_id,
        files,
    })
}

fn archive_source_label(archive_path: Option<&str>) -> &'static str {
    if archive_path.is_some() {
        "staged_file"
    } else {
        "inline_payload"
    }
}

fn validate_restore_scope(
    paths: &[String],
    include_config: bool,
    destination_root: Option<&str>,
) -> Result<()> {
    if !include_config && paths.is_empty() {
        anyhow::bail!("restore scope is empty");
    }
    for path in paths {
        validate_safe_absolute_path(path)?;
    }
    if let Some(destination_root) = destination_root {
        validate_safe_absolute_path(destination_root)?;
    }
    if include_config && destination_root.is_none() {
        anyhow::bail!("config restore requires destination root");
    }
    Ok(())
}

fn entry_requested(entry: &BackupFileEntry, paths: &[String], include_config: bool) -> bool {
    match entry.source {
        BackupFileSource::SelectedPath => paths.iter().any(|path| path == &entry.path),
        BackupFileSource::AgentConfig => include_config,
    }
}

async fn restore_entry(
    job_id: uuid::Uuid,
    decoded: &DecodedBackupFile,
    destination_root: Option<&str>,
) -> Result<AppliedRestore> {
    let entry = &decoded.entry;
    validate_restore_entry(entry, &decoded.data, destination_root)?;
    let destination = destination_path_for_entry(entry, destination_root)?;
    let rollback = write_restored_file(job_id, &destination, &decoded.data, entry.mode).await?;
    let status = RestoredFileStatus {
        archive_path: entry.path.clone(),
        destination_path: destination.display().to_string(),
        source: match entry.source {
            BackupFileSource::SelectedPath => "selected_path",
            BackupFileSource::AgentConfig => "agent_config",
        },
        size_bytes: decoded.data.len() as u64,
        sha256_hex: payload_hash(&decoded.data),
        mode: entry.mode,
        rollback_path: rollback
            .as_ref()
            .map(|snapshot| snapshot.path.display().to_string()),
    };
    Ok(AppliedRestore {
        status,
        destination_path: destination,
        rollback,
    })
}

fn validate_restore_entry(
    entry: &BackupFileEntry,
    data: &[u8],
    destination_root: Option<&str>,
) -> Result<()> {
    validate_restore_entry_payload(entry, data)?;
    let _ = destination_path_for_entry(entry, destination_root)?;
    Ok(())
}

fn validate_restore_entry_payload(entry: &BackupFileEntry, data: &[u8]) -> Result<()> {
    validate_file_mode(entry.mode).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    if data.len() as u64 != entry.size_bytes {
        anyhow::bail!("restore archive entry {} size mismatch", entry.path);
    }
    if payload_hash(data) != entry.sha256_hex {
        anyhow::bail!("restore archive entry {} sha256 mismatch", entry.path);
    }
    Ok(())
}

fn validate_post_restore_argv(argv: &[String]) -> Result<()> {
    if argv.is_empty() {
        return Ok(());
    }
    anyhow::ensure!(argv.len() <= 32, "post-restore argv has too many entries");
    anyhow::ensure!(
        argv[0].starts_with('/'),
        "post-restore executable must be absolute"
    );
    for part in argv {
        anyhow::ensure!(
            !part.is_empty() && part.len() <= 4096 && !part.as_bytes().contains(&0),
            "post-restore argv contains invalid part"
        );
    }
    Ok(())
}

async fn run_post_restore_argv(argv: &[String], timeout_secs: u64) -> Result<serde_json::Value> {
    if argv.is_empty() {
        return Ok(post_restore_status(argv, None, false));
    }
    let output = time::timeout(Duration::from_secs(timeout_secs.max(1)), async {
        Command::new(&argv[0]).args(&argv[1..]).output().await
    })
    .await
    .context("post-restore command timed out")?
    .context("failed to run post-restore command")?;
    Ok(post_restore_status(argv, Some(&output), false))
}

fn post_restore_status(
    argv: &[String],
    output: Option<&std::process::Output>,
    dry_run: bool,
) -> serde_json::Value {
    let Some(output) = output else {
        return serde_json::json!({
            "configured": !argv.is_empty(),
            "status": if dry_run && !argv.is_empty() { "skipped_dry_run" } else { "not_configured" },
            "argv": argv,
        });
    };
    serde_json::json!({
        "configured": true,
        "status": if output.status.success() { "passed" } else { "failed" },
        "argv": argv,
        "exit_code": output.status.code(),
        "stdout_preview": String::from_utf8_lossy(&output.stdout).chars().take(4096).collect::<String>(),
        "stderr_preview": String::from_utf8_lossy(&output.stderr).chars().take(4096).collect::<String>(),
    })
}

fn destination_path_for_entry(
    entry: &BackupFileEntry,
    destination_root: Option<&str>,
) -> Result<PathBuf> {
    match entry.source {
        BackupFileSource::SelectedPath => {
            validate_safe_absolute_path(&entry.path)?;
            match destination_root {
                Some(root) => Ok(Path::new(root).join(relative_from_absolute(&entry.path)?)),
                None => Ok(PathBuf::from(&entry.path)),
            }
        }
        BackupFileSource::AgentConfig => {
            let root = destination_root.context("config restore requires destination root")?;
            Ok(Path::new(root).join("vpsman/agent_config.toml"))
        }
    }
}

async fn write_restored_file(
    job_id: uuid::Uuid,
    destination: &Path,
    data: &[u8],
    mode: u32,
) -> Result<Option<RollbackSnapshot>> {
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .context("restore destination has no parent directory")?;
    tokio::fs::create_dir_all(parent)
        .await
        .with_context(|| format!("failed to create restore directory {}", parent.display()))?;
    let rollback = if let Ok(metadata) = tokio::fs::metadata(destination).await {
        if metadata.is_dir() {
            anyhow::bail!(
                "restore destination is a directory: {}",
                destination.display()
            );
        }
        let rollback_mode = metadata.permissions().mode() & 0o777;
        let file_name = destination
            .file_name()
            .context("restore destination has no file name")?
            .to_string_lossy();
        let rollback_path = parent.join(format!(".vpsman-restore-{file_name}-{job_id}.bak"));
        tokio::fs::copy(destination, &rollback_path)
            .await
            .with_context(|| format!("failed to create rollback {}", rollback_path.display()))?;
        Some(RollbackSnapshot {
            path: rollback_path,
            mode: rollback_mode,
        })
    } else {
        None
    };

    let file_name = destination
        .file_name()
        .context("restore destination has no file name")?
        .to_string_lossy();
    let temp_path = parent.join(format!(".vpsman-restore-{file_name}-{job_id}.tmp"));
    tokio::fs::write(&temp_path, data)
        .await
        .with_context(|| format!("failed to write restore temp {}", temp_path.display()))?;
    tokio::fs::set_permissions(&temp_path, std::fs::Permissions::from_mode(mode))
        .await
        .with_context(|| format!("failed to set mode on {}", temp_path.display()))?;
    if let Err(error) = tokio::fs::rename(&temp_path, destination).await {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(error)
            .with_context(|| format!("failed to move restore into {}", destination.display()));
    }
    Ok(rollback)
}

async fn rollback_applied_restores(applied: &[AppliedRestore]) -> Result<()> {
    let mut failures = Vec::new();
    for item in applied.iter().rev() {
        if let Err(error) = rollback_one_restore(item).await {
            failures.push(format!("{}: {error}", item.destination_path.display()));
        }
    }
    if !failures.is_empty() {
        anyhow::bail!("{}", failures.join("; "));
    }
    Ok(())
}

async fn rollback_one_restore(item: &AppliedRestore) -> Result<()> {
    match &item.rollback {
        Some(snapshot) => {
            tokio::fs::copy(&snapshot.path, &item.destination_path)
                .await
                .with_context(|| {
                    format!(
                        "failed to restore rollback copy {}",
                        snapshot.path.display()
                    )
                })?;
            tokio::fs::set_permissions(
                &item.destination_path,
                std::fs::Permissions::from_mode(snapshot.mode),
            )
            .await
            .with_context(|| {
                format!(
                    "failed to restore rollback permissions on {}",
                    item.destination_path.display()
                )
            })?;
        }
        None => match tokio::fs::remove_file(&item.destination_path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to remove newly restored file {}",
                        item.destination_path.display()
                    )
                });
            }
        },
    }
    Ok(())
}

fn validate_safe_absolute_path(path: &str) -> Result<()> {
    if path
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        anyhow::bail!("restore path contains unsafe path segment");
    }
    validate_absolute_file_path(path).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    if Path::new(path).components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::CurDir | Component::Prefix(_)
        )
    }) {
        anyhow::bail!("restore path contains unsafe path segment");
    }
    Ok(())
}

fn relative_from_absolute(path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(segment) => relative.push(segment),
            _ => anyhow::bail!("restore path contains unsafe path segment"),
        }
    }
    Ok(relative)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;

    #[tokio::test]
    async fn restores_selected_path_and_config_under_destination_root_with_rollback() {
        let job_id = uuid::Uuid::new_v4();
        let root = std::env::temp_dir().join(format!("vpsman-restore-{job_id}"));
        let destination_root = root.join("restore-root");
        let selected_destination = destination_root.join("tmp/source.txt");
        tokio::fs::create_dir_all(selected_destination.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&selected_destination, b"old")
            .await
            .unwrap();

        let archive_bytes = backup_archive_bytes(vec![
            backup_entry(
                0,
                "/tmp/source.txt",
                BackupFileSource::SelectedPath,
                b"new-data",
            ),
            backup_entry(
                1,
                "vpsman:agent_config",
                BackupFileSource::AgentConfig,
                b"config",
            ),
        ]);
        let paths = vec!["/tmp/source.txt".to_string()];
        let archive_base64 = BASE64_STANDARD.encode(&archive_bytes);
        let archive_sha256_hex = payload_hash(&archive_bytes);
        let outputs = execute_restore_command(RestoreCommandInput {
            job_id,
            source_backup_request_id: uuid::Uuid::new_v4(),
            paths: &paths,
            include_config: true,
            destination_root: Some(destination_root.to_str().unwrap()),
            archive_path: None,
            archive_base64: Some(&archive_base64),
            archive_size_bytes: Some(archive_bytes.len() as u64),
            archive_sha256_hex: Some(&archive_sha256_hex),
            dry_run: false,
            post_restore_argv: &[],
            timeout_secs: 5,
        })
        .await
        .unwrap();

        assert_eq!(
            tokio::fs::read(&selected_destination).await.unwrap(),
            b"new-data"
        );
        assert_eq!(
            tokio::fs::read(destination_root.join("vpsman/agent_config.toml"))
                .await
                .unwrap(),
            b"config"
        );
        let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
        assert_eq!(status["type"], "restore");
        assert_eq!(status["restored_count"], 2);
        assert!(status["restored_files"][0]["rollback_path"]
            .as_str()
            .unwrap()
            .contains(".vpsman-restore-source.txt-"));

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn restore_rejects_missing_archive_or_unsafe_paths() {
        let paths = vec!["/tmp/source.txt".to_string()];
        let missing = execute_restore_command(RestoreCommandInput {
            job_id: uuid::Uuid::new_v4(),
            source_backup_request_id: uuid::Uuid::new_v4(),
            paths: &paths,
            include_config: false,
            destination_root: Some("/tmp/restore"),
            archive_path: None,
            archive_base64: None,
            archive_size_bytes: None,
            archive_sha256_hex: None,
            dry_run: false,
            post_restore_argv: &[],
            timeout_secs: 5,
        })
        .await
        .unwrap_err();
        assert!(missing.to_string().contains("restore archive is required"));

        let unsafe_paths = vec!["/tmp/../source.txt".to_string()];
        let bad_hash = "0".repeat(64);
        let unsafe_path = execute_restore_command(RestoreCommandInput {
            job_id: uuid::Uuid::new_v4(),
            source_backup_request_id: uuid::Uuid::new_v4(),
            paths: &unsafe_paths,
            include_config: false,
            destination_root: Some("/tmp/restore"),
            archive_path: None,
            archive_base64: Some(""),
            archive_size_bytes: Some(0),
            archive_sha256_hex: Some(&bad_hash),
            dry_run: false,
            post_restore_argv: &[],
            timeout_secs: 5,
        })
        .await
        .unwrap_err();
        assert!(unsafe_path.to_string().contains("unsafe path segment"));
    }

    #[tokio::test]
    async fn restore_rolls_back_applied_files_after_later_entry_failure() {
        let job_id = uuid::Uuid::new_v4();
        let root = std::env::temp_dir().join(format!("vpsman-restore-rollback-{job_id}"));
        let destination_root = root.join("restore-root");
        let first_destination = destination_root.join("tmp/first.txt");
        let created_destination = destination_root.join("tmp/created.txt");
        let broken_destination = destination_root.join("tmp/broken.txt");
        tokio::fs::create_dir_all(first_destination.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&first_destination, b"old-first")
            .await
            .unwrap();
        tokio::fs::set_permissions(&first_destination, std::fs::Permissions::from_mode(0o640))
            .await
            .unwrap();
        tokio::fs::create_dir_all(&broken_destination)
            .await
            .unwrap();

        let archive_bytes = backup_archive_bytes(vec![
            backup_entry(
                0,
                "/tmp/first.txt",
                BackupFileSource::SelectedPath,
                b"new-first",
            ),
            backup_entry(
                1,
                "/tmp/created.txt",
                BackupFileSource::SelectedPath,
                b"new-created",
            ),
            backup_entry(
                2,
                "/tmp/broken.txt",
                BackupFileSource::SelectedPath,
                b"broken-data",
            ),
        ]);
        let paths = vec![
            "/tmp/first.txt".to_string(),
            "/tmp/created.txt".to_string(),
            "/tmp/broken.txt".to_string(),
        ];
        let archive_base64 = BASE64_STANDARD.encode(&archive_bytes);
        let archive_sha256_hex = payload_hash(&archive_bytes);
        let error = execute_restore_command(RestoreCommandInput {
            job_id,
            source_backup_request_id: uuid::Uuid::new_v4(),
            paths: &paths,
            include_config: false,
            destination_root: Some(destination_root.to_str().unwrap()),
            archive_path: None,
            archive_base64: Some(&archive_base64),
            archive_size_bytes: Some(archive_bytes.len() as u64),
            archive_sha256_hex: Some(&archive_sha256_hex),
            dry_run: false,
            post_restore_argv: &[],
            timeout_secs: 5,
        })
        .await
        .unwrap_err();

        assert!(error.to_string().contains("applied files were rolled back"));
        assert_eq!(
            tokio::fs::read(&first_destination).await.unwrap(),
            b"old-first"
        );
        assert_eq!(
            tokio::fs::metadata(&first_destination)
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
        assert!(!created_destination.exists());

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    fn backup_archive_bytes(entries: Vec<(BackupFileEntry, Vec<u8>)>) -> Vec<u8> {
        let manifest = BackupArchive {
            format: BACKUP_ARCHIVE_FORMAT.to_string(),
            client_id: "source-client".to_string(),
            created_unix: 1,
            files: entries.iter().map(|(entry, _)| entry.clone()).collect(),
        };
        let mut builder = tar::Builder::new(Vec::new());
        append_test_tar_entry(
            &mut builder,
            BACKUP_ARCHIVE_MANIFEST_PATH,
            0o600,
            serde_json::to_vec(&manifest).unwrap().as_slice(),
        );
        for (entry, data) in entries {
            append_test_tar_entry(&mut builder, &entry.tar_path, entry.mode, &data);
        }
        builder.finish().unwrap();
        builder.into_inner().unwrap()
    }

    fn append_test_tar_entry(
        builder: &mut tar::Builder<Vec<u8>>,
        path: &str,
        mode: u32,
        data: &[u8],
    ) {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(mode);
        header.set_mtime(1);
        header.set_cksum();
        builder.append_data(&mut header, path, data).unwrap();
    }

    fn backup_entry(
        index: usize,
        path: &str,
        source: BackupFileSource,
        data: &[u8],
    ) -> (BackupFileEntry, Vec<u8>) {
        (
            BackupFileEntry {
                path: path.to_string(),
                source,
                tar_path: format!("vpsman-backup/files/{index:04}.bin"),
                mode: 0o600,
                size_bytes: data.len() as u64,
                sha256_hex: payload_hash(data),
                mtime_unix: Some(1),
            },
            data.to_vec(),
        )
    }
}
