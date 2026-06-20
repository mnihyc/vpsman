use std::{
    collections::HashMap,
    io::{Cursor, Read, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::Serialize;
use sha2::Digest;
use tokio::{
    io::AsyncReadExt,
    time::{self, Duration},
};
use vpsman_common::{
    payload_hash, validate_absolute_file_path, validate_file_mode, CommandOutput, OutputStream,
};

use crate::backup::{
    BackupArchive, BackupFileEntry, BackupFileSource, BACKUP_ARCHIVE_FORMAT,
    BACKUP_ARCHIVE_MANIFEST_PATH,
};
use crate::{
    child_process::{run_child_with_bounded_output_cancelable, ChildCleanupPolicy, ChildRunResult},
    command_worker::{run_cancelable, CommandCancelToken},
    safe_fs,
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

#[derive(Clone, Debug)]
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
    pub(crate) archive_size_bytes: Option<u64>,
    pub(crate) archive_sha256_hex: Option<&'a str>,
    pub(crate) max_archive_bytes: u64,
    pub(crate) dry_run: bool,
    pub(crate) post_restore_argv: &'a [String],
    pub(crate) timeout_secs: u64,
    pub(crate) cancel_token: CommandCancelToken,
}

pub(crate) async fn execute_restore_command(
    input: RestoreCommandInput<'_>,
) -> Result<Vec<CommandOutput>> {
    let deadline = time::Instant::now() + Duration::from_secs(input.timeout_secs.max(1));
    let cancel_token = input.cancel_token.clone();
    run_cancelable("restore", cancel_token, restore_archive(input, deadline)).await
}

async fn restore_archive(
    input: RestoreCommandInput<'_>,
    deadline: time::Instant,
) -> Result<Vec<CommandOutput>> {
    let RestoreCommandInput {
        job_id,
        source_backup_request_id,
        paths,
        include_config,
        destination_root,
        archive_path,
        archive_size_bytes,
        archive_sha256_hex,
        max_archive_bytes,
        dry_run,
        post_restore_argv,
        cancel_token,
        timeout_secs: _,
    } = input;
    cancel_token.check("restore")?;
    validate_restore_scope(paths, include_config, destination_root)?;
    validate_post_restore_argv(post_restore_argv)?;
    ensure_restore_deadline(deadline)?;
    cancel_token.check("restore")?;
    let archive_bytes = archive_bytes_from_source(
        archive_path,
        archive_size_bytes,
        archive_sha256_hex,
        max_archive_bytes,
    )
    .await?;
    cancel_token.check("restore")?;
    let archive = decode_backup_archive(&archive_bytes)?;
    cancel_token.check("restore")?;

    let mut restored = Vec::new();
    let mut matched_count = 0_usize;
    for entry in archive.files {
        cancel_token.check("restore")?;
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
    let post_restore =
        run_post_restore_argv(post_restore_argv, post_restore_timeout_secs, cancel_token).await?;
    let post_restore_passed = post_restore_success(&post_restore);

    let status = serde_json::json!({
        "type": "restore",
        "status": if post_restore_passed { "restored" } else { "post_restore_failed" },
        "message": if post_restore_passed { "restore completed" } else { "restore post-hook failed after files were restored" },
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
        exit_code: Some(if post_restore_passed { 0 } else { 1 }),
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
    archive_size_bytes: Option<u64>,
    archive_sha256_hex: Option<&str>,
    max_archive_bytes: u64,
) -> Result<Vec<u8>> {
    let path = archive_path.context("restore archive path is required")?;
    validate_safe_absolute_path(path)?;
    let expected_size = archive_size_bytes.context("restore archive size is required")?;
    let expected_sha256_hex = archive_sha256_hex.context("restore archive sha256 is required")?;
    anyhow::ensure!(expected_size > 0, "restore archive size must be positive");
    anyhow::ensure!(
        expected_size <= max_archive_bytes,
        "restore archive exceeds configured limit: {expected_size} > {max_archive_bytes} bytes"
    );
    let metadata = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("failed to stat restore archive {path}"))?;
    anyhow::ensure!(
        metadata.len() == expected_size,
        "restore archive size mismatch"
    );
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("failed to open restore archive {path}"))?;
    let mut bytes = Vec::with_capacity(expected_size.min(64 * 1024) as usize);
    let mut hasher = sha2::Sha256::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .with_context(|| format!("failed to read restore archive {path}"))?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .context("restore archive size overflow")?;
        anyhow::ensure!(total <= expected_size, "restore archive size mismatch");
        hasher.update(&buffer[..read]);
        bytes.extend_from_slice(&buffer[..read]);
    }
    anyhow::ensure!(total == expected_size, "restore archive size mismatch");
    let actual_sha256_hex = hex::encode(hasher.finalize());
    anyhow::ensure!(
        actual_sha256_hex == expected_sha256_hex,
        "restore archive sha256 mismatch"
    );
    Ok(bytes)
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

async fn run_post_restore_argv(
    argv: &[String],
    timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<serde_json::Value> {
    if argv.is_empty() {
        return Ok(post_restore_status(argv, None, false));
    }
    let mut command = tokio::process::Command::new(&argv[0]);
    command.args(&argv[1..]);
    match run_child_with_bounded_output_cancelable(
        command,
        timeout_secs.max(1),
        8192,
        ChildCleanupPolicy::ProcessGroup,
        cancel_token,
    )
    .await
    .context("failed to run post-restore command")?
    {
        ChildRunResult::Completed(output) => Ok(post_restore_output_status(
            argv,
            output.exit_code,
            &output.stdout,
            &output.stderr,
        )),
        ChildRunResult::TimedOut(_) => Ok(post_restore_timed_out_status(argv)),
        ChildRunResult::Canceled { reason, .. } => Ok(post_restore_canceled_status(argv, &reason)),
    }
}

fn post_restore_success(value: &serde_json::Value) -> bool {
    matches!(
        value.get("status").and_then(serde_json::Value::as_str),
        Some("not_configured" | "skipped_dry_run" | "passed")
    )
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

fn post_restore_output_status(
    argv: &[String],
    exit_code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
) -> serde_json::Value {
    serde_json::json!({
        "configured": true,
        "status": if exit_code == Some(0) { "passed" } else { "failed" },
        "argv": argv,
        "exit_code": exit_code,
        "stdout_preview": String::from_utf8_lossy(stdout).chars().take(4096).collect::<String>(),
        "stderr_preview": String::from_utf8_lossy(stderr).chars().take(4096).collect::<String>(),
    })
}

fn post_restore_timed_out_status(argv: &[String]) -> serde_json::Value {
    serde_json::json!({
        "configured": true,
        "status": "timed_out",
        "argv": argv,
        "exit_code": 124,
        "stdout_preview": "",
        "stderr_preview": "",
    })
}

fn post_restore_canceled_status(argv: &[String], reason: &str) -> serde_json::Value {
    serde_json::json!({
        "configured": true,
        "status": "canceled",
        "argv": argv,
        "reason": reason,
        "exit_code": null,
        "stdout_preview": "",
        "stderr_preview": "",
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
    let destination = destination.to_path_buf();
    let data = data.to_vec();
    tokio::task::spawn_blocking(move || {
        write_restored_file_blocking(job_id, &destination, &data, mode)
    })
    .await
    .context("restore file write worker failed")?
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
    let destination = item.destination_path.clone();
    let rollback = item.rollback.clone();
    tokio::task::spawn_blocking(move || rollback_one_restore_blocking(&destination, rollback))
        .await
        .context("restore rollback worker failed")?
}

fn write_restored_file_blocking(
    job_id: uuid::Uuid,
    destination: &Path,
    data: &[u8],
    mode: u32,
) -> Result<Option<RollbackSnapshot>> {
    let parent_path = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .context("restore destination has no parent directory")?;
    let parent_dir = safe_fs::ensure_dir_all_no_symlinks(parent_path).with_context(|| {
        format!(
            "failed to create restore directory {}",
            parent_path.display()
        )
    })?;
    let file_name = destination
        .file_name()
        .context("restore destination has no file name")?;
    let rollback = if let Some(metadata) = safe_fs::stat_child(&parent_dir, file_name, false)? {
        if metadata.is_dir() {
            anyhow::bail!(
                "restore destination is a directory: {}",
                destination.display()
            );
        }
        if metadata.is_symlink() {
            anyhow::bail!(
                "restore destination is a symlink: {}",
                destination.display()
            );
        }
        let rollback_name = std::ffi::OsString::from(format!(
            ".vpsman-restore-{}-{job_id}.bak",
            file_name.to_string_lossy()
        ));
        let mut source = safe_fs::open_child_file_read(&parent_dir, file_name, false)?;
        safe_fs::ensure_identity(
            &source,
            &metadata.identity,
            "restore destination changed before rollback copy",
        )?;
        let mut rollback_file = safe_fs::create_private_child_file(&parent_dir, &rollback_name)
            .with_context(|| {
                format!(
                    "failed to create rollback {}",
                    parent_path.join(&rollback_name).display()
                )
            })?;
        copy_open_file(&mut source, &mut rollback_file)?;
        safe_fs::fchmod_file(&rollback_file, metadata.permission_bits())?;
        rollback_file.sync_all().with_context(|| {
            format!(
                "failed to sync rollback {}",
                parent_path.join(&rollback_name).display()
            )
        })?;
        Some(RollbackSnapshot {
            path: parent_path.join(rollback_name),
            mode: metadata.permission_bits(),
        })
    } else {
        None
    };

    let temp_name = std::ffi::OsString::from(format!(
        ".vpsman-restore-{}-{job_id}.tmp",
        file_name.to_string_lossy()
    ));
    let mut temp_file =
        safe_fs::create_private_child_file(&parent_dir, &temp_name).with_context(|| {
            format!(
                "failed to create restore temp {}",
                parent_path.join(&temp_name).display()
            )
        })?;
    let result = (|| -> Result<()> {
        temp_file.write_all(data).with_context(|| {
            format!(
                "failed to write restore temp {}",
                parent_path.join(&temp_name).display()
            )
        })?;
        safe_fs::fchmod_file(&temp_file, mode)?;
        temp_file.sync_all().with_context(|| {
            format!(
                "failed to sync restore temp {}",
                parent_path.join(&temp_name).display()
            )
        })?;
        safe_fs::rename_child(&parent_dir, &temp_name, &parent_dir, file_name, true)
            .with_context(|| format!("failed to move restore into {}", destination.display()))?;
        safe_fs::sync_dir_best_effort(&parent_dir);
        Ok(())
    })();
    if result.is_err() {
        let _ = safe_fs::remove_child_file(&parent_dir, &temp_name);
    }
    result?;
    Ok(rollback)
}

fn rollback_one_restore_blocking(
    destination: &Path,
    rollback: Option<RollbackSnapshot>,
) -> Result<()> {
    match rollback {
        Some(snapshot) => {
            copy_snapshot_into_destination(&snapshot.path, destination, snapshot.mode)?;
        }
        None => {
            let parent = safe_fs::resolve_parent(destination)?;
            match safe_fs::remove_child_file(parent.dir(), parent.name()) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "failed to remove newly restored file {}",
                            destination.display()
                        )
                    });
                }
            }
            safe_fs::sync_dir_best_effort(parent.dir());
        }
    }
    Ok(())
}

fn copy_snapshot_into_destination(snapshot: &Path, destination: &Path, mode: u32) -> Result<()> {
    let snapshot_parent = safe_fs::resolve_parent(snapshot)?;
    let mut source = snapshot_parent.open_child_file_read(false)?;
    let destination_parent = safe_fs::resolve_parent(destination)?;
    let (mut temp_file, temp_name) = safe_fs::create_private_temp_file(
        destination_parent.dir(),
        destination_parent.name(),
        "restore-rollback",
    )?;
    let result = (|| -> Result<()> {
        copy_open_file(&mut source, &mut temp_file)?;
        safe_fs::fchmod_file(&temp_file, mode)?;
        temp_file.sync_all().with_context(|| {
            format!("failed to sync rollback temp for {}", destination.display())
        })?;
        safe_fs::rename_child(
            destination_parent.dir(),
            &temp_name,
            destination_parent.dir(),
            destination_parent.name(),
            true,
        )
        .with_context(|| format!("failed to move rollback into {}", destination.display()))?;
        safe_fs::sync_dir_best_effort(destination_parent.dir());
        Ok(())
    })();
    if result.is_err() {
        let _ = safe_fs::remove_child_file(destination_parent.dir(), &temp_name);
    }
    result
}

fn copy_open_file(source: &mut std::fs::File, destination: &mut std::fs::File) -> Result<()> {
    let mut buffer = vec![0_u8; 16 * 1024];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        destination.write_all(&buffer[..read])?;
    }
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
    use std::os::unix::fs::{symlink, PermissionsExt};

    use super::*;

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
        let archive_path = root.join("archive.tar");
        tokio::fs::write(&archive_path, &archive_bytes)
            .await
            .unwrap();
        let archive_sha256_hex = payload_hash(&archive_bytes);
        let outputs = execute_restore_command(RestoreCommandInput {
            job_id,
            source_backup_request_id: uuid::Uuid::new_v4(),
            paths: &paths,
            include_config: true,
            destination_root: Some(destination_root.to_str().unwrap()),
            archive_path: Some(archive_path.to_str().unwrap()),
            archive_size_bytes: Some(archive_bytes.len() as u64),
            archive_sha256_hex: Some(&archive_sha256_hex),
            max_archive_bytes: archive_bytes.len() as u64,
            dry_run: false,
            post_restore_argv: &[],
            timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
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
            archive_size_bytes: None,
            archive_sha256_hex: None,
            max_archive_bytes: 1024,
            dry_run: false,
            post_restore_argv: &[],
            timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap_err();
        assert!(missing
            .to_string()
            .contains("restore archive path is required"));

        let unsafe_paths = vec!["/tmp/../source.txt".to_string()];
        let bad_hash = "0".repeat(64);
        let unsafe_path = execute_restore_command(RestoreCommandInput {
            job_id: uuid::Uuid::new_v4(),
            source_backup_request_id: uuid::Uuid::new_v4(),
            paths: &unsafe_paths,
            include_config: false,
            destination_root: Some("/tmp/restore"),
            archive_path: None,
            archive_size_bytes: Some(0),
            archive_sha256_hex: Some(&bad_hash),
            max_archive_bytes: 1024,
            dry_run: false,
            post_restore_argv: &[],
            timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap_err();
        assert!(unsafe_path.to_string().contains("unsafe path segment"));
    }

    #[tokio::test]
    async fn restore_rejects_archive_above_configured_size_limit() {
        let job_id = uuid::Uuid::new_v4();
        let root = std::env::temp_dir().join(format!("vpsman-restore-archive-cap-{job_id}"));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let archive_bytes = backup_archive_bytes(vec![backup_entry(
            0,
            "/tmp/source.txt",
            BackupFileSource::SelectedPath,
            b"new-data",
        )]);
        let archive_path = root.join("archive.tar");
        tokio::fs::write(&archive_path, &archive_bytes)
            .await
            .unwrap();
        let archive_sha256_hex = payload_hash(&archive_bytes);
        let paths = vec!["/tmp/source.txt".to_string()];

        let error = execute_restore_command(RestoreCommandInput {
            job_id,
            source_backup_request_id: uuid::Uuid::new_v4(),
            paths: &paths,
            include_config: false,
            destination_root: Some(root.join("restore").to_str().unwrap()),
            archive_path: Some(archive_path.to_str().unwrap()),
            archive_size_bytes: Some(archive_bytes.len() as u64),
            archive_sha256_hex: Some(&archive_sha256_hex),
            max_archive_bytes: archive_bytes.len() as u64 - 1,
            dry_run: false,
            post_restore_argv: &[],
            timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("restore archive exceeds configured limit"));
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn restore_rejects_symlink_component_below_destination_root() {
        let job_id = uuid::Uuid::new_v4();
        let root = std::env::temp_dir().join(format!("vpsman-restore-symlink-root-{job_id}"));
        let destination_root = root.join("restore-root");
        let outside = root.join("outside");
        tokio::fs::create_dir_all(&destination_root).await.unwrap();
        tokio::fs::create_dir_all(&outside).await.unwrap();
        symlink(&outside, destination_root.join("tmp")).unwrap();

        let archive_bytes = backup_archive_bytes(vec![backup_entry(
            0,
            "/tmp/source.txt",
            BackupFileSource::SelectedPath,
            b"new-data",
        )]);
        let archive_path = root.join("archive.tar");
        tokio::fs::write(&archive_path, &archive_bytes)
            .await
            .unwrap();
        let archive_sha256_hex = payload_hash(&archive_bytes);
        let paths = vec!["/tmp/source.txt".to_string()];

        let error = execute_restore_command(RestoreCommandInput {
            job_id,
            source_backup_request_id: uuid::Uuid::new_v4(),
            paths: &paths,
            include_config: false,
            destination_root: Some(destination_root.to_str().unwrap()),
            archive_path: Some(archive_path.to_str().unwrap()),
            archive_size_bytes: Some(archive_bytes.len() as u64),
            archive_sha256_hex: Some(&archive_sha256_hex),
            max_archive_bytes: archive_bytes.len() as u64,
            dry_run: false,
            post_restore_argv: &[],
            timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap_err();

        assert!(error
            .chain()
            .any(|cause| cause.to_string().contains("real directory")));
        assert!(!outside.join("source.txt").exists());
        let _ = tokio::fs::remove_dir_all(root).await;
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
        let archive_path = root.join("archive.tar");
        tokio::fs::write(&archive_path, &archive_bytes)
            .await
            .unwrap();
        let archive_sha256_hex = payload_hash(&archive_bytes);
        let error = execute_restore_command(RestoreCommandInput {
            job_id,
            source_backup_request_id: uuid::Uuid::new_v4(),
            paths: &paths,
            include_config: false,
            destination_root: Some(destination_root.to_str().unwrap()),
            archive_path: Some(archive_path.to_str().unwrap()),
            archive_size_bytes: Some(archive_bytes.len() as u64),
            archive_sha256_hex: Some(&archive_sha256_hex),
            max_archive_bytes: archive_bytes.len() as u64,
            dry_run: false,
            post_restore_argv: &[],
            timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
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

    #[tokio::test]
    async fn restore_reports_post_restore_failure_as_terminal_failure() {
        let job_id = uuid::Uuid::new_v4();
        let root = std::env::temp_dir().join(format!("vpsman-restore-post-hook-{job_id}"));
        let destination_root = root.join("restore-root");
        tokio::fs::create_dir_all(&destination_root).await.unwrap();
        let archive_bytes = backup_archive_bytes(vec![backup_entry(
            0,
            "/tmp/source.txt",
            BackupFileSource::SelectedPath,
            b"new-data",
        )]);
        let archive_path = root.join("archive.tar");
        tokio::fs::write(&archive_path, &archive_bytes)
            .await
            .unwrap();
        let archive_sha256_hex = payload_hash(&archive_bytes);
        let paths = vec!["/tmp/source.txt".to_string()];
        let post_restore_argv = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "printf post-hook-failed >&2; exit 7".to_string(),
        ];

        let outputs = execute_restore_command(RestoreCommandInput {
            job_id,
            source_backup_request_id: uuid::Uuid::new_v4(),
            paths: &paths,
            include_config: false,
            destination_root: Some(destination_root.to_str().unwrap()),
            archive_path: Some(archive_path.to_str().unwrap()),
            archive_size_bytes: Some(archive_bytes.len() as u64),
            archive_sha256_hex: Some(&archive_sha256_hex),
            max_archive_bytes: archive_bytes.len() as u64,
            dry_run: false,
            post_restore_argv: &post_restore_argv,
            timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap();

        assert_eq!(
            tokio::fs::read(destination_root.join("tmp/source.txt"))
                .await
                .unwrap(),
            b"new-data"
        );
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].exit_code, Some(1));
        let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
        assert_eq!(status["status"], "post_restore_failed");
        assert_eq!(status["post_restore"]["status"], "failed");
        assert_eq!(status["post_restore"]["exit_code"], 7);
        assert!(status["post_restore"]["stderr_preview"]
            .as_str()
            .unwrap()
            .contains("post-hook-failed"));
        assert_eq!(status["rollback_available"], true);

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
