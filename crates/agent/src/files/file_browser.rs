use std::{
    collections::HashSet,
    ffi::{OsStr, OsString},
    io::{self, Read, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use tokio::time::{self, Duration};
use vpsman_common::{
    decode_inline_file_payload, job_command_type_label, payload_hash, validate_absolute_file_path,
    validate_file_mode, CommandOutput, FileActionPolicy, FileOwnershipPolicy, OutputStream,
};

use crate::{
    command_worker::CommandCancelToken,
    file_pull::chunked_output,
    platform_accounts::{
        metadata_gid, metadata_mode, metadata_mtime_unix, metadata_uid, normalize_ownership_tokens,
        PlatformAccounts,
    },
    safe_file, safe_fs,
};

const MAX_FILE_LIST_LIMIT: u32 = 1000;
pub(crate) const MAX_FILE_TREE_SCAN_ENTRIES: usize = 10_000;
const MAX_FILE_READ_BYTES: u64 = 1024 * 1024;
const MAX_FILE_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;

struct LimitedVecWriter<'a> {
    inner: &'a mut Vec<u8>,
    written: u64,
    max_bytes: u64,
}

impl<'a> LimitedVecWriter<'a> {
    fn new(inner: &'a mut Vec<u8>, max_bytes: u64) -> Self {
        Self {
            inner,
            written: 0,
            max_bytes,
        }
    }
}

impl Write for LimitedVecWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let next = self
            .written
            .checked_add(buf.len() as u64)
            .ok_or_else(|| io::Error::other("archive size overflow"))?;
        if next > self.max_bytes {
            return Err(io::Error::other("archive exceeds configured byte limit"));
        }
        self.inner.extend_from_slice(buf);
        self.written = next;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) async fn execute_file_browser_command(
    job_id: uuid::Uuid,
    command: &vpsman_common::JobCommand,
    max_timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<Vec<CommandOutput>> {
    let operation_type = job_command_type_label(command);
    let max_timeout_secs = max_timeout_secs.max(1);
    match time::timeout(
        Duration::from_secs(max_timeout_secs),
        execute_file_browser_command_inner(job_id, command, cancel_token.clone(), operation_type),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            cancel_token.cancel(format!("timeout after {max_timeout_secs}s"));
            Err(anyhow!("file browser command timed out"))
        }
    }
}

async fn execute_file_browser_command_inner(
    job_id: uuid::Uuid,
    command: &vpsman_common::JobCommand,
    cancel_token: CommandCancelToken,
    operation_type: &'static str,
) -> Result<Vec<CommandOutput>> {
    cancel_token.check(operation_type)?;
    match command {
        vpsman_common::JobCommand::FileStat { path } => execute_file_stat(job_id, path).await,
        vpsman_common::JobCommand::FileListDir {
            path,
            offset,
            limit,
            show_hidden,
        } => execute_file_list_dir(job_id, path, *offset, *limit, *show_hidden).await,
        vpsman_common::JobCommand::FileReadText {
            path,
            max_bytes,
            follow_symlinks,
        } => execute_file_read_text(job_id, path, *max_bytes, *follow_symlinks).await,
        vpsman_common::JobCommand::FileWriteText {
            path,
            mode,
            size_bytes,
            sha256_hex,
            content_base64,
            expected_sha256_hex,
            create,
            policy,
        } => {
            execute_file_write_text(
                job_id,
                path,
                *mode,
                *size_bytes,
                sha256_hex,
                content_base64,
                expected_sha256_hex.as_deref(),
                *create,
                *policy,
            )
            .await
        }
        vpsman_common::JobCommand::FileMkdir {
            path,
            mode,
            recursive,
            policy,
        } => execute_file_mkdir(job_id, path, *mode, *recursive, *policy).await,
        vpsman_common::JobCommand::FileRename {
            path,
            new_path,
            overwrite,
            policy,
        } => execute_file_rename(job_id, path, new_path, *overwrite, *policy).await,
        vpsman_common::JobCommand::FileDelete {
            path,
            recursive,
            policy,
        } => {
            execute_file_delete(
                job_id,
                path,
                *recursive,
                *policy,
                cancel_token,
                operation_type,
            )
            .await
        }
        vpsman_common::JobCommand::FileChmod {
            path,
            mode,
            recursive,
            follow_symlinks,
            policy,
        } => {
            execute_file_chmod(
                job_id,
                path,
                *mode,
                *recursive,
                *follow_symlinks,
                *policy,
                cancel_token,
                operation_type,
            )
            .await
        }
        vpsman_common::JobCommand::FileChown {
            path,
            owner,
            group,
            uid,
            gid,
            recursive,
            ownership_policy,
            policy,
        } => {
            execute_file_chown(
                job_id,
                path,
                owner.as_deref(),
                group.as_deref(),
                *uid,
                *gid,
                *recursive,
                *ownership_policy,
                *policy,
                cancel_token,
                operation_type,
            )
            .await
        }
        vpsman_common::JobCommand::FileCopy {
            path,
            new_path,
            overwrite,
            recursive,
            follow_symlinks,
            policy,
        } => {
            execute_file_copy(
                job_id,
                path,
                new_path,
                *overwrite,
                *recursive,
                *follow_symlinks,
                *policy,
                cancel_token,
                operation_type,
            )
            .await
        }
        vpsman_common::JobCommand::FileArchiveTar {
            path,
            max_bytes,
            follow_symlinks,
        } => {
            execute_file_archive_tar(
                job_id,
                path,
                *max_bytes,
                *follow_symlinks,
                cancel_token,
                operation_type,
            )
            .await
        }
        _ => anyhow::bail!("unsupported file browser command"),
    }
}

async fn execute_file_stat(job_id: uuid::Uuid, path: &str) -> Result<Vec<CommandOutput>> {
    validate_browser_path(path)?;
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .with_context(|| format!("failed to stat {path}"))?;
    let status = json!({
        "type": "file_stat",
        "path": path,
        "metadata": metadata_json(Path::new(path), &metadata).await?,
    });
    status_output(job_id, status)
}

async fn execute_file_list_dir(
    job_id: uuid::Uuid,
    path: &str,
    offset: u32,
    limit: u32,
    show_hidden: bool,
) -> Result<Vec<CommandOutput>> {
    validate_browser_path(path)?;
    let limit = limit.clamp(1, MAX_FILE_LIST_LIMIT);
    let metadata = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("failed to stat directory {path}"))?;
    if !metadata.is_dir() {
        anyhow::bail!("file list path is not a directory");
    }

    let mut entries = Vec::new();
    let mut scanned_entries = 0_usize;
    let mut visible_entries_scanned = 0_usize;
    let mut truncated_by_scan_cap = false;
    let mut read_dir = tokio::fs::read_dir(path)
        .await
        .with_context(|| format!("failed to read directory {path}"))?;
    while let Some(entry) = read_dir
        .next_entry()
        .await
        .with_context(|| format!("failed to read directory entry in {path}"))?
    {
        if scanned_entries >= MAX_FILE_TREE_SCAN_ENTRIES {
            truncated_by_scan_cap = true;
            break;
        }
        scanned_entries += 1;
        let name = entry.file_name().to_string_lossy().to_string();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        visible_entries_scanned += 1;
        let entry_path = entry.path();
        let entry_metadata = tokio::fs::symlink_metadata(&entry_path)
            .await
            .with_context(|| format!("failed to stat {}", entry_path.display()))?;
        entries.push(FileListEntry {
            name,
            path: entry_path.to_string_lossy().to_string(),
            is_dir: entry_metadata.is_dir(),
            metadata: entry_metadata,
        });
    }
    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.name.cmp(&right.name))
    });
    let exact_total_entries = entries.len();
    let start = (offset as usize).min(exact_total_entries);
    let end = start
        .saturating_add(limit as usize)
        .min(exact_total_entries);
    let mut rendered = Vec::with_capacity(end.saturating_sub(start));
    for entry in &entries[start..end] {
        rendered.push(metadata_entry_json(entry).await?);
    }
    let total_entries = if truncated_by_scan_cap {
        Value::Null
    } else {
        json!(exact_total_entries)
    };
    let status = json!({
        "type": "file_list_dir",
        "path": path,
        "offset": offset,
        "limit": limit,
        "total_entries": total_entries,
        "scanned_entries": scanned_entries,
        "visible_entries_scanned": visible_entries_scanned,
        "scan_cap_entries": MAX_FILE_TREE_SCAN_ENTRIES,
        "truncated_by_scan_cap": truncated_by_scan_cap,
        "truncated": truncated_by_scan_cap || end < exact_total_entries,
        "entries": rendered,
        "metadata": metadata_json(Path::new(path), &metadata).await?,
    });
    status_output(job_id, status)
}

async fn execute_file_read_text(
    job_id: uuid::Uuid,
    path: &str,
    max_bytes: u64,
    follow_symlinks: bool,
) -> Result<Vec<CommandOutput>> {
    validate_browser_path(path)?;
    let max_bytes = max_bytes.clamp(1, MAX_FILE_READ_BYTES);
    let read = tokio::task::spawn_blocking({
        let path = PathBuf::from(path);
        move || {
            safe_file::read_regular_file_bounded(
                &path,
                max_bytes,
                follow_symlinks,
                "file exceeds text read limit while reading",
                "path is a symlink; set follow_symlinks to use the target",
            )
        }
    })
    .await
    .context("file read worker failed")?
    .with_context(|| format!("failed to read file {path}"))?;
    let metadata = read.metadata;
    let data = read.data;
    if std::str::from_utf8(&data).is_err() {
        anyhow::bail!("file is not valid UTF-8 text");
    }
    let status = json!({
        "type": "file_read_text",
        "path": path,
        "content_base64": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data),
        "size_bytes": data.len(),
        "sha256_hex": payload_hash(&data),
        "truncated": false,
        "metadata": metadata_json(Path::new(path), &metadata).await?,
    });
    status_output(job_id, status)
}

fn path_metadata_for_follow(path: &Path, follow_symlinks: bool) -> Result<std::fs::Metadata> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        if follow_symlinks {
            return std::fs::metadata(path).with_context(|| {
                format!(
                    "symlink does not resolve to a readable target {}",
                    path.display()
                )
            });
        }
        anyhow::bail!("path is a symlink; set follow_symlinks to use the target");
    }
    Ok(metadata)
}

async fn execute_file_write_text(
    job_id: uuid::Uuid,
    path: &str,
    mode: u32,
    size_bytes: u64,
    sha256_hex: &str,
    content_base64: &str,
    expected_sha256_hex: Option<&str>,
    create: bool,
    policy: FileActionPolicy,
) -> Result<Vec<CommandOutput>> {
    anyhow::ensure!(
        create || expected_sha256_hex.is_some(),
        "file write expected hash is required when replacing an existing file"
    );
    validate_mutable_path(path)?;
    validate_file_mode(mode).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let data = decode_inline_file_payload(content_base64, size_bytes, sha256_hex)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    if std::str::from_utf8(&data).is_err() {
        anyhow::bail!("file write text content is not UTF-8");
    }
    let destination = Path::new(path);
    let existing = tokio::fs::symlink_metadata(destination).await.ok();
    if create && existing.is_some() {
        if policy == FileActionPolicy::Ignore {
            return status_output(
                job_id,
                json!({"type": "file_write_text", "path": path, "status": "skipped", "reason": "destination_exists"}),
            );
        }
        if policy == FileActionPolicy::Ensure {
            let (current_hash, current_size) = hash_text_destination(destination)
                .await
                .with_context(|| format!("failed to hash current file before writing {path}"))?;
            if current_hash == sha256_hex.to_ascii_lowercase() {
                return status_output(
                    job_id,
                    json!({
                        "type": "file_write_text",
                        "path": path,
                        "status": "unchanged",
                        "sha256_hex": current_hash,
                        "size_bytes": current_size,
                    }),
                );
            }
        }
        anyhow::bail!("file write create target already exists");
    }
    if existing.is_none() && !create {
        if policy_allows_missing(policy) {
            return status_output(
                job_id,
                json!({"type": "file_write_text", "path": path, "status": "skipped", "reason": "missing"}),
            );
        }
        anyhow::bail!("file write target does not exist");
    }
    if existing.as_ref().is_some_and(|metadata| metadata.is_dir()) {
        anyhow::bail!("file write target is a directory");
    }
    if existing
        .as_ref()
        .is_some_and(|metadata| metadata.file_type().is_symlink())
    {
        anyhow::bail!("file write target is a symlink");
    }
    if let Some(expected) = expected_sha256_hex {
        let (current_hash, current_size) = match hash_text_destination(destination).await {
            Ok(hash) => hash,
            Err(error) if policy == FileActionPolicy::Ignore => {
                return status_output(
                    job_id,
                    json!({
                        "type": "file_write_text",
                        "path": path,
                        "status": "skipped",
                        "reason": "verification_failed",
                        "error": error.to_string(),
                        "expected_sha256_hex": expected,
                    }),
                );
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to hash current file before writing {path}"));
            }
        };
        if current_hash != expected.to_ascii_lowercase() {
            if policy == FileActionPolicy::Ensure && current_hash == sha256_hex.to_ascii_lowercase()
            {
                return status_output(
                    job_id,
                    json!({
                        "type": "file_write_text",
                        "path": path,
                        "status": "unchanged",
                        "sha256_hex": current_hash,
                        "size_bytes": current_size,
                    }),
                );
            }
            if policy == FileActionPolicy::Ignore {
                return status_output(
                    job_id,
                    json!({
                        "type": "file_write_text",
                        "path": path,
                        "status": "skipped",
                        "reason": "stale",
                        "current_sha256_hex": current_hash,
                        "expected_sha256_hex": expected,
                    }),
                );
            }
            anyhow::bail!("file changed since it was opened");
        }
    }
    let commit = atomic_write(
        destination,
        mode,
        &data,
        !create,
        expected_sha256_hex,
        sha256_hex,
        policy,
    )
    .await?;
    match commit {
        AtomicWriteOutcome::Written => {}
        AtomicWriteOutcome::Unchanged {
            sha256_hex,
            size_bytes,
        } => {
            return status_output(
                job_id,
                json!({
                    "type": "file_write_text",
                    "path": path,
                    "status": "unchanged",
                    "sha256_hex": sha256_hex,
                    "size_bytes": size_bytes,
                }),
            );
        }
        AtomicWriteOutcome::SkippedMissing => {
            return status_output(
                job_id,
                json!({"type": "file_write_text", "path": path, "status": "skipped", "reason": "missing"}),
            );
        }
        AtomicWriteOutcome::SkippedDestinationExists => {
            return status_output(
                job_id,
                json!({"type": "file_write_text", "path": path, "status": "skipped", "reason": "destination_exists"}),
            );
        }
        AtomicWriteOutcome::SkippedStale { current_sha256_hex } => {
            return status_output(
                job_id,
                json!({
                    "type": "file_write_text",
                    "path": path,
                    "status": "skipped",
                    "reason": "stale",
                    "current_sha256_hex": current_sha256_hex,
                    "expected_sha256_hex": expected_sha256_hex,
                }),
            );
        }
        AtomicWriteOutcome::SkippedVerification { error } => {
            return status_output(
                job_id,
                json!({
                    "type": "file_write_text",
                    "path": path,
                    "status": "skipped",
                    "reason": "verification_failed",
                    "error": error,
                    "expected_sha256_hex": expected_sha256_hex,
                }),
            );
        }
    }
    let metadata = metadata_entry_for_path(destination).await?;
    status_output(
        job_id,
        json!({
            "type": "file_write_text",
            "path": path,
            "status": if existing.is_some() { "updated" } else { "created" },
            "size_bytes": data.len(),
            "sha256_hex": payload_hash(&data),
            "mode": mode,
            "atomic": true,
            "metadata": metadata,
        }),
    )
}

async fn hash_text_destination(destination: &Path) -> Result<(String, u64)> {
    let destination = destination.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let (hash, metadata, _) = safe_file::hash_regular_file_bounded(
            &destination,
            MAX_FILE_READ_BYTES,
            false,
            "current file exceeds text verification limit",
            "current file is a symlink",
        )?;
        Ok((hash, metadata.len()))
    })
    .await
    .context("file text hash worker failed")?
}

async fn execute_file_mkdir(
    job_id: uuid::Uuid,
    path: &str,
    mode: u32,
    recursive: bool,
    policy: FileActionPolicy,
) -> Result<Vec<CommandOutput>> {
    validate_browser_path(path)?;
    validate_file_mode(mode).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let target = PathBuf::from(path);
    let target_for_create = target.clone();
    let created = tokio::task::spawn_blocking(move || {
        mkdir_path_blocking(&target_for_create, mode, recursive, policy)
    })
    .await
    .context("mkdir worker failed")??;
    if !created {
        let metadata = metadata_entry_for_path(&target).await?;
        return status_output(
            job_id,
            json!({"type": "file_mkdir", "path": path, "status": "unchanged", "metadata": metadata}),
        );
    }
    let metadata = metadata_entry_for_path(&target).await?;
    status_output(
        job_id,
        json!({"type": "file_mkdir", "path": path, "status": "created", "mode": mode, "metadata": metadata}),
    )
}

async fn execute_file_rename(
    job_id: uuid::Uuid,
    path: &str,
    new_path: &str,
    overwrite: bool,
    policy: FileActionPolicy,
) -> Result<Vec<CommandOutput>> {
    validate_mutable_path(path)?;
    validate_mutable_path(new_path)?;
    let source = PathBuf::from(path);
    let destination = PathBuf::from(new_path);
    let outcome = tokio::task::spawn_blocking(move || {
        rename_path_blocking(&source, &destination, overwrite, policy)
    })
    .await
    .context("rename worker failed")??;
    match outcome {
        RenameOutcome::Unchanged => {
            return status_output(
                job_id,
                json!({"type": "file_rename", "path": path, "new_path": new_path, "status": "unchanged"}),
            );
        }
        RenameOutcome::SkippedMissing => {
            return status_output(
                job_id,
                json!({"type": "file_rename", "path": path, "new_path": new_path, "status": "skipped", "reason": "missing"}),
            );
        }
        RenameOutcome::SkippedDestinationExists => {
            return status_output(
                job_id,
                json!({"type": "file_rename", "path": path, "new_path": new_path, "status": "skipped", "reason": "destination_exists"}),
            );
        }
        RenameOutcome::Renamed => {}
    }
    status_output(
        job_id,
        json!({"type": "file_rename", "path": path, "new_path": new_path, "status": "renamed", "overwrite": overwrite}),
    )
}

async fn execute_file_delete(
    job_id: uuid::Uuid,
    path: &str,
    recursive: bool,
    policy: FileActionPolicy,
    cancel_token: CommandCancelToken,
    operation_type: &'static str,
) -> Result<Vec<CommandOutput>> {
    cancel_token.check(operation_type)?;
    validate_mutable_path(path)?;
    let target = Path::new(path);
    if tokio::fs::symlink_metadata(target).await.is_err() {
        if policy_allows_missing(policy) {
            return status_output(
                job_id,
                json!({"type": "file_delete", "path": path, "status": "unchanged"}),
            );
        }
        anyhow::bail!("delete target does not exist");
    }
    cancel_token.check(operation_type)?;
    remove_path(target, recursive, cancel_token.clone(), operation_type).await?;
    cancel_token.check(operation_type)?;
    status_output(
        job_id,
        json!({"type": "file_delete", "path": path, "status": "deleted", "recursive": recursive}),
    )
}

async fn execute_file_chmod(
    job_id: uuid::Uuid,
    path: &str,
    mode: u32,
    recursive: bool,
    follow_symlinks: bool,
    policy: FileActionPolicy,
    cancel_token: CommandCancelToken,
    operation_type: &'static str,
) -> Result<Vec<CommandOutput>> {
    cancel_token.check(operation_type)?;
    validate_mutable_path(path)?;
    validate_file_mode(mode).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let target = PathBuf::from(path);
    let target_metadata = match tokio::fs::symlink_metadata(&target).await {
        Ok(metadata) => metadata,
        Err(_) if policy_allows_missing(policy) => {
            return status_output(
                job_id,
                json!({"type": "file_chmod", "path": path, "status": "skipped", "reason": "missing"}),
            );
        }
        Err(_) => anyhow::bail!("chmod target does not exist"),
    };
    if target_metadata.file_type().is_symlink() && !follow_symlinks {
        anyhow::bail!("chmod target is a symlink; set follow_symlinks to mutate the target");
    }
    if follow_symlinks && std::fs::metadata(&target).is_err() {
        if policy_allows_missing(policy) {
            return status_output(
                job_id,
                json!({"type": "file_chmod", "path": path, "status": "skipped", "reason": "missing"}),
            );
        }
        anyhow::bail!("chmod target symlink does not resolve");
    }
    let worker_token = cancel_token.clone();
    tokio::task::spawn_blocking(move || {
        chmod_path(
            &target,
            mode,
            recursive,
            follow_symlinks,
            &worker_token,
            operation_type,
        )
    })
    .await
    .context("chmod worker failed")??;
    cancel_token.check(operation_type)?;
    status_output(
        job_id,
        json!({"type": "file_chmod", "path": path, "status": "changed", "mode": mode, "recursive": recursive, "follow_symlinks": follow_symlinks}),
    )
}

async fn execute_file_chown(
    job_id: uuid::Uuid,
    path: &str,
    owner: Option<&str>,
    group: Option<&str>,
    uid: Option<u32>,
    gid: Option<u32>,
    recursive: bool,
    ownership_policy: FileOwnershipPolicy,
    policy: FileActionPolicy,
    cancel_token: CommandCancelToken,
    operation_type: &'static str,
) -> Result<Vec<CommandOutput>> {
    cancel_token.check(operation_type)?;
    validate_mutable_path(path)?;
    let target = PathBuf::from(path);
    if tokio::fs::symlink_metadata(&target).await.is_err() {
        if policy_allows_missing(policy) {
            return status_output(
                job_id,
                json!({"type": "file_chown", "path": path, "status": "skipped", "reason": "missing"}),
            );
        }
        anyhow::bail!("chown target does not exist");
    }
    let ownership = resolve_owner_group(owner, group, uid, gid, ownership_policy)?;
    if ownership.status == OwnershipResolutionStatus::Skipped {
        return status_output(
            job_id,
            json!({
                "type": "file_chown",
                "path": path,
                "status": "skipped",
                "reason": "missing_owner_or_group",
                "ownership_status": "skipped",
                "recursive": recursive,
            }),
        );
    }
    if ownership.uid.is_none() && ownership.gid.is_none() {
        return status_output(
            job_id,
            json!({"type": "file_chown", "path": path, "status": "unchanged", "recursive": recursive}),
        );
    }
    let uid = ownership.uid;
    let gid = ownership.gid;
    let worker_token = cancel_token.clone();
    let changed = tokio::task::spawn_blocking(move || {
        chown_path_recursive(&target, uid, gid, recursive, &worker_token, operation_type)
    })
    .await
    .context("chown worker failed")??;
    cancel_token.check(operation_type)?;
    if !changed {
        return status_output(
            job_id,
            json!({"type": "file_chown", "path": path, "status": "unchanged", "recursive": recursive}),
        );
    }
    status_output(
        job_id,
        json!({
            "type": "file_chown",
            "path": path,
            "status": "changed",
            "uid": uid,
            "gid": gid,
            "owner": ownership.owner,
            "group": ownership.group,
            "ownership_status": "applied",
            "recursive": recursive,
        }),
    )
}

async fn execute_file_copy(
    job_id: uuid::Uuid,
    path: &str,
    new_path: &str,
    overwrite: bool,
    recursive: bool,
    follow_symlinks: bool,
    policy: FileActionPolicy,
    cancel_token: CommandCancelToken,
    operation_type: &'static str,
) -> Result<Vec<CommandOutput>> {
    cancel_token.check(operation_type)?;
    validate_mutable_path(path)?;
    validate_mutable_path(new_path)?;
    let source = PathBuf::from(path);
    let destination = PathBuf::from(new_path);
    let metadata = match tokio::fs::symlink_metadata(&source).await {
        Ok(metadata) => metadata,
        Err(_) if policy_allows_missing(policy) => {
            return status_output(
                job_id,
                json!({"type": "file_copy", "path": path, "new_path": new_path, "status": "skipped", "reason": "missing"}),
            );
        }
        Err(error) => {
            return Err(error).with_context(|| format!("copy source does not exist: {path}"))
        }
    };
    if metadata.file_type().is_symlink() {
        if !follow_symlinks {
            anyhow::bail!("copy source is a symlink; set follow_symlinks to copy the target");
        }
        if tokio::fs::metadata(&source).await.is_err() {
            anyhow::bail!("copy source symlink does not resolve");
        }
    }
    let effective_destination = effective_copy_destination(&source, &destination)?;
    if let Ok(destination_metadata) = tokio::fs::symlink_metadata(&effective_destination).await {
        if destination_metadata.file_type().is_symlink() {
            if !follow_symlinks {
                anyhow::bail!(
                    "copy destination is a symlink; set follow_symlinks to overwrite the target"
                );
            }
            if !overwrite {
                anyhow::bail!("copy destination symlink following requires overwrite");
            }
        }
    }
    if path_is_within(&effective_destination, &source) {
        anyhow::bail!("refusing to copy a directory into itself");
    }
    if tokio::fs::symlink_metadata(&effective_destination)
        .await
        .is_ok()
        && !overwrite
    {
        if policy == FileActionPolicy::Ignore {
            return status_output(
                job_id,
                json!({"type": "file_copy", "path": path, "new_path": new_path, "effective_path": effective_destination.to_string_lossy(), "status": "skipped", "reason": "destination_exists"}),
            );
        }
        if policy == FileActionPolicy::Ensure
            && paths_have_same_content(&source, &effective_destination)?
        {
            return status_output(
                job_id,
                json!({"type": "file_copy", "path": path, "new_path": new_path, "effective_path": effective_destination.to_string_lossy(), "status": "unchanged"}),
            );
        }
        anyhow::bail!("copy destination already exists");
    }
    let status_path = effective_destination.to_string_lossy().to_string();
    let worker_token = cancel_token.clone();
    tokio::task::spawn_blocking(move || {
        copy_path(
            &source,
            &effective_destination,
            recursive,
            overwrite,
            &worker_token,
            operation_type,
            follow_symlinks,
        )
    })
    .await
    .context("copy worker failed")??;
    cancel_token.check(operation_type)?;
    status_output(
        job_id,
        json!({"type": "file_copy", "path": path, "new_path": new_path, "effective_path": status_path, "status": "copied", "overwrite": overwrite, "recursive": recursive, "follow_symlinks": follow_symlinks}),
    )
}

async fn execute_file_archive_tar(
    job_id: uuid::Uuid,
    path: &str,
    max_bytes: u64,
    follow_symlinks: bool,
    cancel_token: CommandCancelToken,
    operation_type: &'static str,
) -> Result<Vec<CommandOutput>> {
    cancel_token.check(operation_type)?;
    validate_browser_path(path)?;
    let max_bytes = max_bytes.clamp(1, MAX_FILE_ARCHIVE_BYTES);
    let source = PathBuf::from(path);
    let worker_token = cancel_token.clone();
    let archive = tokio::task::spawn_blocking(move || {
        build_tar_archive(
            &source,
            max_bytes,
            follow_symlinks,
            &worker_token,
            operation_type,
        )
    })
    .await
    .context("archive worker failed")??;
    cancel_token.check(operation_type)?;
    let size_bytes = archive.len();
    let sha256_hex = payload_hash(&archive);
    let mut outputs = chunked_output(job_id, OutputStream::Stdout, &archive);
    outputs.push(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&json!({
            "type": "file_archive_tar",
            "path": path,
            "size_bytes": size_bytes,
            "sha256_hex": sha256_hex,
            "content_type": "application/x-tar",
            "filename": archive_filename(path),
            "chunk_bytes": crate::file_pull::COMMAND_OUTPUT_CHUNK_BYTES,
        }))?,
        exit_code: Some(0),
        done: true,
    });
    Ok(outputs)
}

struct FileListEntry {
    name: String,
    path: String,
    is_dir: bool,
    metadata: std::fs::Metadata,
}

async fn metadata_entry_json(entry: &FileListEntry) -> Result<Value> {
    let path = Path::new(&entry.path);
    let mut metadata = metadata_json(path, &entry.metadata).await?;
    metadata["name"] = json!(entry.name);
    Ok(metadata)
}

async fn metadata_entry_for_path(path: &Path) -> Result<Value> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .with_context(|| format!("failed to stat resulting path {}", path.display()))?;
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());
    metadata_entry_json(&FileListEntry {
        name,
        path: path.to_string_lossy().to_string(),
        is_dir: metadata.is_dir(),
        metadata,
    })
    .await
}

async fn metadata_json(path: &Path, metadata: &std::fs::Metadata) -> Result<Value> {
    let file_type = if metadata.file_type().is_symlink() {
        "symlink"
    } else if metadata.is_dir() {
        "directory"
    } else if metadata.is_file() {
        "file"
    } else {
        "other"
    };
    let symlink_target = if metadata.file_type().is_symlink() {
        tokio::fs::read_link(path)
            .await
            .ok()
            .map(|target| target.to_string_lossy().to_string())
    } else {
        None
    };
    Ok(json!({
        "path": path.to_string_lossy(),
        "file_type": file_type,
        "is_dir": metadata.is_dir(),
        "is_file": metadata.is_file(),
        "is_symlink": metadata.file_type().is_symlink(),
        "size_bytes": metadata.len(),
        "mode": metadata_mode(metadata).map(|mode| mode & 0o777),
        "uid": metadata_uid(metadata),
        "gid": metadata_gid(metadata),
        "mtime_unix": metadata_mtime_unix(metadata),
        "symlink_target": symlink_target,
    }))
}

enum RenameOutcome {
    Renamed,
    Unchanged,
    SkippedMissing,
    SkippedDestinationExists,
}

fn rename_path_blocking(
    source: &Path,
    destination: &Path,
    overwrite: bool,
    policy: FileActionPolicy,
) -> Result<RenameOutcome> {
    let source_parent = match safe_fs::resolve_parent(source) {
        Ok(parent) => parent,
        Err(error) if policy_allows_missing(policy) && error_chain_has_not_found(&error) => {
            return Ok(RenameOutcome::SkippedMissing);
        }
        Err(error) => return Err(error),
    };
    let destination_parent = safe_fs::resolve_parent(destination)?;
    let source_metadata = match source_parent.child_stat_nofollow()? {
        Some(metadata) => metadata,
        None => {
            if policy == FileActionPolicy::Ensure
                && destination_parent.child_stat_nofollow()?.is_some()
            {
                return Ok(RenameOutcome::Unchanged);
            }
            if policy_allows_missing(policy) {
                return Ok(RenameOutcome::SkippedMissing);
            }
            anyhow::bail!("rename source does not exist");
        }
    };
    if let Some(destination_metadata) = destination_parent.child_stat_nofollow()? {
        if !overwrite {
            if policy == FileActionPolicy::Ignore {
                return Ok(RenameOutcome::SkippedDestinationExists);
            }
            anyhow::bail!("rename destination already exists");
        }
        if source_metadata.is_dir() != destination_metadata.is_dir() {
            anyhow::bail!("rename destination type is incompatible");
        }
    }
    let source_metadata_at_commit = source_parent
        .child_stat_nofollow()?
        .context("rename source does not exist")?;
    if source_metadata_at_commit.identity != source_metadata.identity {
        anyhow::bail!("rename source changed before commit");
    }
    safe_fs::rename_child(
        source_parent.dir(),
        source_parent.name(),
        destination_parent.dir(),
        destination_parent.name(),
        overwrite,
    )
    .with_context(|| {
        format!(
            "failed to rename {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    safe_fs::sync_dir_best_effort(source_parent.dir());
    safe_fs::sync_dir_best_effort(destination_parent.dir());
    Ok(RenameOutcome::Renamed)
}

#[derive(Debug, Eq, PartialEq)]
enum AtomicWriteOutcome {
    Written,
    Unchanged { sha256_hex: String, size_bytes: u64 },
    SkippedMissing,
    SkippedDestinationExists,
    SkippedStale { current_sha256_hex: String },
    SkippedVerification { error: String },
}

async fn atomic_write(
    destination: &Path,
    mode: u32,
    data: &[u8],
    replace: bool,
    expected_sha256_hex: Option<&str>,
    desired_sha256_hex: &str,
    policy: FileActionPolicy,
) -> Result<AtomicWriteOutcome> {
    let destination = destination.to_path_buf();
    let data = data.to_vec();
    let expected_sha256_hex = expected_sha256_hex.map(str::to_ascii_lowercase);
    let desired_sha256_hex = desired_sha256_hex.to_ascii_lowercase();
    tokio::task::spawn_blocking(move || {
        atomic_write_blocking(
            &destination,
            mode,
            &data,
            replace,
            expected_sha256_hex.as_deref(),
            &desired_sha256_hex,
            policy,
        )
    })
    .await
    .context("file write worker failed")?
}

fn atomic_write_blocking(
    destination: &Path,
    mode: u32,
    data: &[u8],
    replace: bool,
    expected_sha256_hex: Option<&str>,
    desired_sha256_hex: &str,
    policy: FileActionPolicy,
) -> Result<AtomicWriteOutcome> {
    let parent = safe_fs::resolve_parent(destination)?;
    let (mut temp_file, temp_name) =
        safe_fs::create_private_temp_file(parent.dir(), parent.name(), "edit")?;
    let result = (|| -> Result<AtomicWriteOutcome> {
        temp_file.write_all(data).with_context(|| {
            format!(
                "failed to write temporary file for {}",
                destination.display()
            )
        })?;
        safe_fs::fchmod_file(&temp_file, mode)?;
        temp_file.sync_all().with_context(|| {
            format!(
                "failed to sync temporary file for {}",
                destination.display()
            )
        })?;
        let destination_at_commit = parent.child_stat_nofollow()?;
        if replace && destination_at_commit.is_none() {
            return match policy {
                FileActionPolicy::Ensure | FileActionPolicy::Ignore => {
                    Ok(AtomicWriteOutcome::SkippedMissing)
                }
                FileActionPolicy::Fail => {
                    anyhow::bail!("file write target disappeared before commit")
                }
            };
        }
        if !replace && destination_at_commit.is_some() {
            return match policy {
                FileActionPolicy::Ignore => Ok(AtomicWriteOutcome::SkippedDestinationExists),
                FileActionPolicy::Ensure => {
                    let (current_sha256_hex, size_bytes) = hash_destination_at_commit(&parent)?;
                    if current_sha256_hex == desired_sha256_hex {
                        Ok(AtomicWriteOutcome::Unchanged {
                            sha256_hex: current_sha256_hex,
                            size_bytes,
                        })
                    } else {
                        anyhow::bail!("file write create target appeared before commit")
                    }
                }
                FileActionPolicy::Fail => {
                    anyhow::bail!("file write create target appeared before commit")
                }
            };
        }
        if destination_at_commit
            .as_ref()
            .is_some_and(|metadata| !metadata.is_file())
        {
            anyhow::bail!("file write target is no longer a regular file")
        }
        if let Some(expected_sha256_hex) = expected_sha256_hex {
            let (current_sha256_hex, size_bytes) = match hash_destination_at_commit(&parent) {
                Ok(current) => current,
                Err(error) if policy == FileActionPolicy::Ignore => {
                    return Ok(AtomicWriteOutcome::SkippedVerification {
                        error: error.to_string(),
                    });
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "failed to revalidate current file before committing {}",
                            destination.display()
                        )
                    });
                }
            };
            if current_sha256_hex != expected_sha256_hex {
                if policy == FileActionPolicy::Ensure && current_sha256_hex == desired_sha256_hex {
                    return Ok(AtomicWriteOutcome::Unchanged {
                        sha256_hex: current_sha256_hex,
                        size_bytes,
                    });
                }
                if policy == FileActionPolicy::Ignore {
                    return Ok(AtomicWriteOutcome::SkippedStale { current_sha256_hex });
                }
                anyhow::bail!("file changed before the write committed");
            }
        }
        safe_fs::rename_child(
            parent.dir(),
            &temp_name,
            parent.dir(),
            parent.name(),
            replace,
        )
        .with_context(|| {
            format!(
                "failed to move file into place at {}",
                destination.display()
            )
        })?;
        safe_fs::sync_dir_best_effort(parent.dir());
        Ok(AtomicWriteOutcome::Written)
    })();
    if !matches!(&result, Ok(AtomicWriteOutcome::Written)) {
        let _ = safe_fs::remove_child_file(parent.dir(), &temp_name);
    }
    result
}

fn hash_destination_at_commit(parent: &safe_fs::SafeParent) -> Result<(String, u64)> {
    let identity_before = parent
        .child_stat_nofollow()?
        .context("file write target disappeared during commit verification")?;
    if !identity_before.is_file() {
        anyhow::bail!("file write target is no longer a regular file");
    }
    let file = parent.open_child_file_read(false)?;
    let metadata_before = file.metadata()?;
    let file_identity_before = safe_file::FileIdentity::from_metadata(&metadata_before);
    if metadata_before.len() > MAX_FILE_READ_BYTES {
        anyhow::bail!(
            "current file exceeds text verification limit: {} > {} bytes",
            metadata_before.len(),
            MAX_FILE_READ_BYTES
        );
    }
    let hash = safe_file::hash_opened_file_bounded(
        file.try_clone()?,
        MAX_FILE_READ_BYTES,
        "current file exceeds text verification limit",
    )?;
    let metadata_after = file.metadata()?;
    let file_identity_after = safe_file::FileIdentity::from_metadata(&metadata_after);
    let identity_after = parent
        .child_stat_nofollow()?
        .context("file write target disappeared during commit verification")?;
    anyhow::ensure!(
        identity_before.identity == identity_after.identity
            && file_identity_before == file_identity_after,
        "file changed during commit verification"
    );
    Ok((hash, metadata_after.len()))
}

fn mkdir_path_blocking(
    target: &Path,
    mode: u32,
    recursive: bool,
    policy: FileActionPolicy,
) -> Result<bool> {
    if recursive {
        let (dir, created) = safe_fs::ensure_dir_all_no_symlinks_with_mode(target, mode)?;
        if !created {
            if policy != FileActionPolicy::Fail {
                return Ok(false);
            }
            anyhow::bail!("mkdir target already exists");
        }
        safe_fs::fchmod_file(&dir, mode)?;
        return Ok(true);
    }

    let parent = safe_fs::resolve_parent(target)?;
    if let Some(metadata) = parent.child_stat_nofollow()? {
        if metadata.is_dir() && policy != FileActionPolicy::Fail {
            return Ok(false);
        }
        anyhow::bail!("mkdir target already exists");
    }
    safe_fs::create_child_dir(parent.dir(), parent.name(), mode)?;
    Ok(true)
}

async fn remove_path(
    path: &Path,
    recursive: bool,
    cancel_token: CommandCancelToken,
    operation_type: &'static str,
) -> Result<()> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        remove_path_blocking(&path, recursive, &cancel_token, operation_type)
    })
    .await
    .context("delete worker failed")??;
    Ok(())
}

fn remove_path_blocking(
    path: &Path,
    recursive: bool,
    cancel_token: &CommandCancelToken,
    operation_type: &'static str,
) -> Result<()> {
    cancel_token.check(operation_type)?;
    let parent = safe_fs::resolve_parent(path)?;
    let metadata = parent
        .child_stat_nofollow()?
        .with_context(|| format!("failed to stat {}", path.display()))?;
    if metadata.is_dir() && !metadata.is_symlink() {
        let dir = safe_fs::open_child_dir_no_symlinks(parent.dir(), parent.name())
            .with_context(|| format!("failed to open directory {}", path.display()))?;
        if recursive {
            remove_dir_contents_checked(&dir, cancel_token, operation_type)?;
        }
        safe_fs::remove_child_dir(parent.dir(), parent.name())
            .with_context(|| format!("failed to remove directory {}", path.display()))?;
    } else {
        safe_fs::remove_child_file(parent.dir(), parent.name())
            .with_context(|| format!("failed to remove file {}", path.display()))?;
    }
    safe_fs::sync_dir_best_effort(parent.dir());
    cancel_token.check(operation_type)?;
    Ok(())
}

fn remove_dir_contents_checked(
    dir: &std::fs::File,
    cancel_token: &CommandCancelToken,
    operation_type: &'static str,
) -> Result<()> {
    cancel_token.check(operation_type)?;
    for entry_name in safe_fs::read_dir_names(dir)? {
        cancel_token.check(operation_type)?;
        let metadata = safe_fs::stat_child(dir, &entry_name, false)?
            .with_context(|| format!("failed to stat {}", entry_name.to_string_lossy()))?;
        if metadata.is_dir() && !metadata.is_symlink() {
            let child_dir =
                safe_fs::open_child_dir_no_symlinks(dir, &entry_name).with_context(|| {
                    format!("failed to open directory {}", entry_name.to_string_lossy())
                })?;
            remove_dir_contents_checked(&child_dir, cancel_token, operation_type)?;
            safe_fs::remove_child_dir(dir, &entry_name).with_context(|| {
                format!(
                    "failed to remove directory {}",
                    entry_name.to_string_lossy()
                )
            })?;
        } else {
            safe_fs::remove_child_file(dir, &entry_name).with_context(|| {
                format!("failed to remove file {}", entry_name.to_string_lossy())
            })?;
        }
    }
    safe_fs::sync_dir_best_effort(dir);
    Ok(())
}

fn chmod_path(
    path: &Path,
    mode: u32,
    recursive: bool,
    follow_symlinks: bool,
    cancel_token: &CommandCancelToken,
    operation_type: &'static str,
) -> Result<()> {
    cancel_token.check(operation_type)?;
    let parent = safe_fs::resolve_parent(path)?;
    let metadata = parent
        .child_stat_nofollow()?
        .with_context(|| format!("failed to stat {}", path.display()))?;
    if metadata.is_symlink() && !follow_symlinks {
        anyhow::bail!("chmod target is a symlink");
    }
    let mut visited_directories = HashSet::new();
    chmod_child(
        parent.dir(),
        parent.name(),
        mode,
        recursive,
        follow_symlinks,
        true,
        cancel_token,
        operation_type,
        &mut visited_directories,
    )?;
    Ok(())
}

fn chmod_child(
    parent: &std::fs::File,
    name: &OsStr,
    mode: u32,
    recursive: bool,
    follow_symlinks: bool,
    top_level: bool,
    cancel_token: &CommandCancelToken,
    operation_type: &'static str,
    visited_directories: &mut HashSet<(u64, u64)>,
) -> Result<()> {
    cancel_token.check(operation_type)?;
    let metadata = safe_fs::stat_child(parent, name, false)?
        .with_context(|| format!("failed to stat {}", name.to_string_lossy()))?;
    if metadata.is_symlink() && !follow_symlinks {
        if top_level {
            anyhow::bail!("chmod target is a symlink");
        }
        return Ok(());
    }
    let file = safe_fs::open_child_file_read(parent, name, follow_symlinks)
        .with_context(|| format!("failed to open {}", name.to_string_lossy()))?;
    let opened_metadata = file.metadata()?;
    if recursive
        && opened_metadata.is_dir()
        && !visited_directories.insert((opened_metadata.dev(), opened_metadata.ino()))
    {
        anyhow::bail!("recursive chmod encountered a previously visited directory");
    }
    safe_fs::fchmod_file(&file, mode)
        .with_context(|| format!("failed to chmod {}", name.to_string_lossy()))?;
    if recursive && opened_metadata.is_dir() {
        for entry_name in safe_fs::read_dir_names(&file)? {
            chmod_child(
                &file,
                &entry_name,
                mode,
                true,
                follow_symlinks,
                false,
                cancel_token,
                operation_type,
                visited_directories,
            )?;
        }
        safe_fs::sync_dir_best_effort(&file);
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum OwnershipResolutionStatus {
    Planned,
    Skipped,
    Unchanged,
}

#[derive(Clone, Debug)]
struct OwnershipResolution {
    uid: Option<u32>,
    gid: Option<u32>,
    owner: Option<String>,
    group: Option<String>,
    status: OwnershipResolutionStatus,
}

fn resolve_owner_group(
    owner: Option<&str>,
    group: Option<&str>,
    uid: Option<u32>,
    gid: Option<u32>,
    ownership_policy: FileOwnershipPolicy,
) -> Result<OwnershipResolution> {
    let tokens = normalize_ownership_tokens(owner, group, uid, gid)?;
    let owner = tokens.owner.as_deref();
    let group = tokens.group.as_deref();
    if owner.is_none() && group.is_none() && uid.is_none() && gid.is_none() {
        return Ok(OwnershipResolution {
            uid: None,
            gid: None,
            owner: None,
            group: None,
            status: OwnershipResolutionStatus::Unchanged,
        });
    }
    let accounts = PlatformAccounts::load()?;
    let mut missing = Vec::new();
    let owner_id = resolve_name_or_id(owner, true, &accounts, "owner", &mut missing);
    let group_id = resolve_name_or_id(group, false, &accounts, "group", &mut missing);
    if !missing.is_empty() {
        if ownership_policy == FileOwnershipPolicy::Ignore {
            return Ok(OwnershipResolution {
                uid: None,
                gid: None,
                owner: None,
                group: None,
                status: OwnershipResolutionStatus::Skipped,
            });
        }
        anyhow::bail!("missing owner/group: {}", missing.join(", "));
    }
    let resolved_uid = merge_id("owner", uid, owner_id.as_ref().map(|value| value.0))?;
    let resolved_gid = merge_id("group", gid, group_id.as_ref().map(|value| value.0))?;
    Ok(OwnershipResolution {
        uid: resolved_uid,
        gid: resolved_gid,
        owner: owner_id
            .and_then(|value| value.1)
            .or_else(|| resolved_uid.and_then(|value| accounts.user_name_for_id(value))),
        group: group_id
            .and_then(|value| value.1)
            .or_else(|| resolved_gid.and_then(|value| accounts.group_name_for_id(value))),
        status: if resolved_uid.is_some() || resolved_gid.is_some() {
            OwnershipResolutionStatus::Planned
        } else {
            OwnershipResolutionStatus::Unchanged
        },
    })
}

fn resolve_name_or_id(
    value: Option<&str>,
    user: bool,
    accounts: &PlatformAccounts,
    kind: &str,
    missing: &mut Vec<String>,
) -> Option<(u32, Option<String>)> {
    let value = value.map(str::trim).filter(|value| !value.is_empty())?;
    if let Ok(id) = value.parse::<u32>() {
        let name = if user {
            accounts.user_name_for_id(id)
        } else {
            accounts.group_name_for_id(id)
        };
        return Some((id, name));
    }
    let resolved = if user {
        accounts.resolve_user(value)
    } else {
        accounts.resolve_group(value)
    };
    if let Some(resolved) = resolved {
        return Some((resolved.id, resolved.name));
    }
    missing.push(format!("{kind}:{value}"));
    None
}

fn merge_id(kind: &str, explicit: Option<u32>, resolved: Option<u32>) -> Result<Option<u32>> {
    match (explicit, resolved) {
        (Some(left), Some(right)) if left != right => {
            anyhow::bail!("{kind} id conflicts with resolved {kind} name")
        }
        (Some(value), _) | (_, Some(value)) => Ok(Some(value)),
        (None, None) => Ok(None),
    }
}

fn chown_path_recursive(
    path: &Path,
    uid: Option<u32>,
    gid: Option<u32>,
    recursive: bool,
    cancel_token: &CommandCancelToken,
    operation_type: &'static str,
) -> Result<bool> {
    cancel_token.check(operation_type)?;
    let parent = safe_fs::resolve_parent(path)?;
    chown_child(
        parent.dir(),
        parent.name(),
        uid,
        gid,
        recursive,
        cancel_token,
        operation_type,
    )
}

fn chown_child(
    parent: &std::fs::File,
    name: &OsStr,
    uid: Option<u32>,
    gid: Option<u32>,
    recursive: bool,
    cancel_token: &CommandCancelToken,
    operation_type: &'static str,
) -> Result<bool> {
    cancel_token.check(operation_type)?;
    let metadata = safe_fs::stat_child(parent, name, false)?
        .with_context(|| format!("failed to stat {}", name.to_string_lossy()))?;
    if metadata.is_symlink() {
        return Ok(false);
    }
    let file = safe_fs::open_child_file_read(parent, name, false)
        .with_context(|| format!("failed to open {}", name.to_string_lossy()))?;
    safe_fs::fchown_file(&file, uid, gid)
        .with_context(|| format!("failed to chown {}", name.to_string_lossy()))?;
    let mut changed = true;
    let opened_metadata = safe_fs::stat_file(&file)?;
    if recursive && opened_metadata.is_dir() {
        for entry_name in safe_fs::read_dir_names(&file)? {
            changed |= chown_child(
                &file,
                &entry_name,
                uid,
                gid,
                true,
                cancel_token,
                operation_type,
            )?;
        }
        safe_fs::sync_dir_best_effort(&file);
    }
    Ok(changed)
}

fn effective_copy_destination(source: &Path, destination: &Path) -> Result<PathBuf> {
    if std::fs::symlink_metadata(destination)
        .map(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        let name = source
            .file_name()
            .filter(|name| !name.is_empty())
            .context("copy source has no file name")?;
        return Ok(destination.join(name));
    }
    Ok(destination.to_path_buf())
}

fn path_is_within(path: &Path, ancestor: &Path) -> bool {
    let normalized_path = normalize_path_lexical(path);
    let normalized_ancestor = normalize_path_lexical(ancestor);
    normalized_path != normalized_ancestor && normalized_path.starts_with(normalized_ancestor)
}

fn normalize_path_lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn copy_path(
    source: &Path,
    destination: &Path,
    recursive: bool,
    overwrite: bool,
    cancel_token: &CommandCancelToken,
    operation_type: &'static str,
    follow_symlinks: bool,
) -> Result<()> {
    cancel_token.check(operation_type)?;
    let source_parent = safe_fs::resolve_parent(source)?;
    let mut visited_directories = HashSet::new();
    copy_child_to_path(
        source_parent.dir(),
        source_parent.name(),
        destination,
        recursive,
        overwrite,
        cancel_token,
        operation_type,
        follow_symlinks,
        None,
        &mut visited_directories,
    )
}

fn copy_child_to_path(
    source_parent: &std::fs::File,
    source_name: &OsStr,
    destination: &Path,
    recursive: bool,
    overwrite: bool,
    cancel_token: &CommandCancelToken,
    operation_type: &'static str,
    follow_symlinks: bool,
    source_root_identity: Option<(u64, u64)>,
    visited_directories: &mut HashSet<(u64, u64)>,
) -> Result<()> {
    cancel_token.check(operation_type)?;
    let link_metadata =
        safe_fs::stat_child(source_parent, source_name, false)?.with_context(|| {
            format!(
                "failed to stat copy source {}",
                source_name.to_string_lossy()
            )
        })?;
    if link_metadata.is_symlink() && !follow_symlinks {
        anyhow::bail!("copy source is a symlink");
    }
    let metadata = if link_metadata.is_symlink() && follow_symlinks {
        safe_fs::stat_child(source_parent, source_name, true)?.with_context(|| {
            format!(
                "copy source symlink does not resolve {}",
                source_name.to_string_lossy()
            )
        })?
    } else {
        link_metadata
    };

    if metadata.is_file() {
        let mut source_file =
            safe_fs::open_child_file_read(source_parent, source_name, follow_symlinks)
                .with_context(|| {
                    format!(
                        "failed to open copy source {}",
                        source_name.to_string_lossy()
                    )
                })?;
        safe_fs::ensure_identity(
            &source_file,
            &metadata.identity,
            "copy source changed before open",
        )?;
        copy_open_file_to_path(
            &mut source_file,
            destination,
            overwrite,
            metadata.permission_bits(),
            cancel_token,
            operation_type,
            follow_symlinks,
        )?;
        return Ok(());
    }

    if metadata.is_dir() {
        if !recursive {
            anyhow::bail!("copy source is a directory and recursive is false");
        }
        let source_dir = safe_fs::open_child_dir(source_parent, source_name, follow_symlinks)
            .with_context(|| {
                format!(
                    "failed to open copy source directory {}",
                    source_name.to_string_lossy()
                )
            })?;
        safe_fs::ensure_identity(
            &source_dir,
            &metadata.identity,
            "copy source directory changed before open",
        )?;
        let opened_metadata = source_dir.metadata()?;
        let source_identity = (opened_metadata.dev(), opened_metadata.ino());
        if !visited_directories.insert(source_identity) {
            anyhow::bail!("recursive copy encountered a previously visited directory");
        }
        let source_root_identity = source_root_identity.unwrap_or(source_identity);
        copy_open_dir_to_path(
            &source_dir,
            destination,
            overwrite,
            metadata.permission_bits(),
            cancel_token,
            operation_type,
            follow_symlinks,
            source_root_identity,
            visited_directories,
        )?;
        return Ok(());
    }

    anyhow::bail!("copy source is not a regular file or directory");
}

fn copy_open_file_to_path(
    reader: &mut std::fs::File,
    destination: &Path,
    overwrite: bool,
    mode: u32,
    cancel_token: &CommandCancelToken,
    operation_type: &'static str,
    follow_symlinks: bool,
) -> Result<()> {
    let destination = resolve_copy_destination_for_write(destination, overwrite, follow_symlinks)?;
    let parent = safe_fs::resolve_parent(&destination)?;
    if let Some(destination_metadata) = parent.child_stat_nofollow()? {
        if destination_metadata.is_symlink() {
            anyhow::bail!("copy destination is a symlink");
        }
        if destination_metadata.is_dir() {
            anyhow::bail!("cannot overwrite a directory with a file");
        }
        if !overwrite {
            anyhow::bail!("copy destination already exists");
        }
    }
    let (mut writer, temp_name) =
        safe_fs::create_private_temp_file(parent.dir(), parent.name(), "copy")?;
    let result = (|| -> Result<()> {
        let mut buffer = vec![0_u8; 16 * 1024];
        loop {
            cancel_token.check(operation_type)?;
            let read = reader.read(&mut buffer).with_context(|| {
                format!("failed to read copy source for {}", destination.display())
            })?;
            if read == 0 {
                break;
            }
            writer.write_all(&buffer[..read]).with_context(|| {
                format!(
                    "failed to write temporary copy for {}",
                    destination.display()
                )
            })?;
        }
        writer.flush().with_context(|| {
            format!(
                "failed to flush temporary copy for {}",
                destination.display()
            )
        })?;
        safe_fs::fchmod_file(&writer, mode).with_context(|| {
            format!(
                "failed to set mode on temporary copy for {}",
                destination.display()
            )
        })?;
        writer.sync_all().with_context(|| {
            format!(
                "failed to sync temporary copy for {}",
                destination.display()
            )
        })?;
        safe_fs::rename_child(
            parent.dir(),
            &temp_name,
            parent.dir(),
            parent.name(),
            overwrite,
        )
        .with_context(|| {
            format!(
                "failed to move copied file into place at {}",
                destination.display()
            )
        })?;
        safe_fs::sync_dir_best_effort(parent.dir());
        Ok(())
    })();
    if result.is_err() {
        let _ = safe_fs::remove_child_file(parent.dir(), &temp_name);
    }
    result
}

fn copy_open_dir_to_path(
    source_dir: &std::fs::File,
    destination: &Path,
    overwrite: bool,
    mode: u32,
    cancel_token: &CommandCancelToken,
    operation_type: &'static str,
    follow_symlinks: bool,
    source_root_identity: (u64, u64),
    visited_directories: &mut HashSet<(u64, u64)>,
) -> Result<()> {
    let destination = resolve_copy_destination_for_write(destination, overwrite, follow_symlinks)?;
    let parent = safe_fs::resolve_parent(&destination)?;
    let source_metadata = source_dir.metadata()?;
    let source_identity = (source_metadata.dev(), source_metadata.ino());
    let destination_parent_within_root = opened_directory_is_same_or_within(
        parent.dir(),
        source_root_identity,
        cancel_token,
        operation_type,
    )?;
    let destination_parent_within_current_source = source_identity != source_root_identity
        && opened_directory_is_same_or_within(
            parent.dir(),
            source_identity,
            cancel_token,
            operation_type,
        )?;
    if destination_parent_within_root || destination_parent_within_current_source {
        anyhow::bail!("refusing to copy a directory into itself");
    }
    let destination_dir = match parent.child_stat_nofollow()? {
        Some(metadata) => {
            if metadata.is_symlink() {
                anyhow::bail!("copy destination is a symlink");
            }
            if !metadata.is_dir() {
                anyhow::bail!("cannot overwrite a non-directory with a directory");
            }
            if !overwrite {
                anyhow::bail!("copy destination already exists");
            }
            safe_fs::open_child_dir_no_symlinks(parent.dir(), parent.name())?
        }
        None => safe_fs::create_child_dir(parent.dir(), parent.name(), mode)?,
    };
    let destination_metadata = destination_dir.metadata()?;
    let destination_identity = (destination_metadata.dev(), destination_metadata.ino());
    if destination_identity == source_root_identity || destination_identity == source_identity {
        anyhow::bail!("refusing to copy a directory into itself");
    }
    safe_fs::fchmod_file(&destination_dir, mode)?;
    for entry_name in safe_fs::read_dir_names(source_dir)? {
        cancel_token.check(operation_type)?;
        let child_destination = destination.join(&entry_name);
        copy_child_to_path(
            source_dir,
            &entry_name,
            &child_destination,
            true,
            overwrite,
            cancel_token,
            operation_type,
            follow_symlinks,
            Some(source_root_identity),
            visited_directories,
        )?;
    }
    safe_fs::sync_dir_best_effort(&destination_dir);
    safe_fs::sync_dir_best_effort(parent.dir());
    Ok(())
}

fn opened_directory_is_same_or_within(
    directory: &std::fs::File,
    ancestor_identity: (u64, u64),
    cancel_token: &CommandCancelToken,
    operation_type: &'static str,
) -> Result<bool> {
    let mut current = directory
        .try_clone()
        .context("failed to inspect copy destination parent")?;
    loop {
        cancel_token.check(operation_type)?;
        let metadata = current.metadata()?;
        let current_identity = (metadata.dev(), metadata.ino());
        if current_identity == ancestor_identity {
            return Ok(true);
        }
        let parent = safe_fs::open_child_dir_no_symlinks(&current, OsStr::new(".."))
            .context("failed to inspect copy destination ancestry")?;
        let parent_metadata = parent.metadata()?;
        if (parent_metadata.dev(), parent_metadata.ino()) == current_identity {
            return Ok(false);
        }
        current = parent;
    }
}

fn resolve_copy_destination_for_write(
    destination: &Path,
    overwrite: bool,
    follow_symlinks: bool,
) -> Result<PathBuf> {
    let parent = safe_fs::resolve_parent(destination)?;
    if let Some(metadata) = parent.child_stat_nofollow()? {
        if metadata.is_symlink() {
            if !follow_symlinks {
                anyhow::bail!("copy destination is a symlink");
            }
            if !overwrite {
                anyhow::bail!("copy destination symlink following requires overwrite");
            }
            return std::fs::canonicalize(destination).with_context(|| {
                format!(
                    "copy destination symlink does not resolve {}",
                    destination.display()
                )
            });
        }
    }
    Ok(destination.to_path_buf())
}

fn paths_have_same_content(source: &Path, destination: &Path) -> Result<bool> {
    let source_metadata = std::fs::symlink_metadata(source)
        .with_context(|| format!("failed to stat {}", source.display()))?;
    let destination_metadata = match std::fs::symlink_metadata(destination) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(false),
    };
    if source_metadata.file_type().is_symlink() || destination_metadata.file_type().is_symlink() {
        return Ok(false);
    }
    if source_metadata.is_file() {
        if !destination_metadata.is_file() || source_metadata.len() != destination_metadata.len() {
            return Ok(false);
        }
        return files_have_same_content(source, destination);
    }
    if source_metadata.is_dir() {
        if !destination_metadata.is_dir() {
            return Ok(false);
        }
        let source_entries = sorted_directory_entries(source)?;
        let destination_entries = sorted_directory_entries(destination)?;
        if source_entries.len() != destination_entries.len()
            || source_entries
                .iter()
                .map(|entry| &entry.0)
                .ne(destination_entries.iter().map(|entry| &entry.0))
        {
            return Ok(false);
        }
        for ((_, source_entry), (_, destination_entry)) in
            source_entries.iter().zip(destination_entries.iter())
        {
            if !paths_have_same_content(source_entry, destination_entry)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    Ok(false)
}

fn files_have_same_content(left: &Path, right: &Path) -> Result<bool> {
    let mut left_file =
        std::fs::File::open(left).with_context(|| format!("failed to read {}", left.display()))?;
    let mut right_file = std::fs::File::open(right)
        .with_context(|| format!("failed to read {}", right.display()))?;
    let mut left_buffer = vec![0_u8; 16 * 1024];
    let mut right_buffer = vec![0_u8; 16 * 1024];
    loop {
        let left_read = left_file
            .read(&mut left_buffer)
            .with_context(|| format!("failed to read {}", left.display()))?;
        let right_read = right_file
            .read(&mut right_buffer)
            .with_context(|| format!("failed to read {}", right.display()))?;
        if left_read != right_read {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
        if left_buffer[..left_read] != right_buffer[..right_read] {
            return Ok(false);
        }
    }
}

fn sorted_directory_entries(path: &Path) -> Result<Vec<(OsString, PathBuf)>> {
    let mut entries = Vec::new();
    for entry in
        std::fs::read_dir(path).with_context(|| format!("failed to read {}", path.display()))?
    {
        let entry = entry?;
        entries.push((entry.file_name(), entry.path()));
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(entries)
}

fn build_tar_archive(
    source: &Path,
    max_bytes: u64,
    follow_symlinks: bool,
    cancel_token: &CommandCancelToken,
    operation_type: &'static str,
) -> Result<Vec<u8>> {
    cancel_token.check(operation_type)?;
    let metadata = if follow_symlinks {
        path_metadata_for_follow(source, true)
    } else {
        std::fs::symlink_metadata(source).map_err(Into::into)
    }
    .with_context(|| format!("failed to stat archive source {}", source.display()))?;
    let mut estimate_visited_directories = HashSet::new();
    let mut estimate_scanned_entries = 0;
    let estimated_size = estimate_archive_input_bytes(
        source,
        &metadata,
        follow_symlinks,
        cancel_token,
        operation_type,
        &mut estimate_visited_directories,
        &mut estimate_scanned_entries,
    )?;
    if estimated_size > max_bytes {
        anyhow::bail!("archive source exceeds limit: {estimated_size} > {max_bytes}");
    }
    let mut archive = Vec::new();
    {
        let mut writer = LimitedVecWriter::new(&mut archive, max_bytes);
        let mut builder = tar::Builder::new(&mut writer);
        builder.follow_symlinks(follow_symlinks);
        let name = source
            .file_name()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| OsStr::new("root"));
        let archive_name = PathBuf::from(Path::new(name));
        let mut archive_visited_directories = HashSet::new();
        let mut archive_scanned_entries = 0;
        append_tar_path_checked(
            &mut builder,
            &archive_name,
            source,
            &metadata,
            follow_symlinks,
            cancel_token,
            operation_type,
            &mut archive_visited_directories,
            &mut archive_scanned_entries,
        )?;
        builder.finish().context("failed to finish tar archive")?;
    }
    if archive.len() as u64 > max_bytes {
        anyhow::bail!("tar archive exceeds limit: {} > {max_bytes}", archive.len());
    }
    Ok(archive)
}

fn append_tar_path_checked<W: Write>(
    builder: &mut tar::Builder<W>,
    archive_path: &Path,
    fs_path: &Path,
    metadata: &std::fs::Metadata,
    follow_symlinks: bool,
    cancel_token: &CommandCancelToken,
    operation_type: &'static str,
    visited_directories: &mut HashSet<(u64, u64)>,
    scanned_entries: &mut usize,
) -> Result<()> {
    cancel_token.check(operation_type)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        let directory_identity = (metadata.dev(), metadata.ino());
        if !visited_directories.insert(directory_identity) {
            anyhow::bail!("archive traversal encountered a previously visited directory");
        }
        builder
            .append_dir(archive_path, fs_path)
            .with_context(|| format!("failed to archive directory {}", fs_path.display()))?;
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(fs_path)
            .with_context(|| format!("failed to read {}", fs_path.display()))?
        {
            cancel_token.check(operation_type)?;
            count_archive_scan_entry(scanned_entries)?;
            let entry = entry?;
            entries.push((entry.file_name(), entry.path()));
        }
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        for (file_name, entry_path) in entries {
            cancel_token.check(operation_type)?;
            let entry_metadata = std::fs::symlink_metadata(&entry_path)
                .with_context(|| format!("failed to stat {}", entry_path.display()))?;
            let entry_metadata = if entry_metadata.file_type().is_symlink() && follow_symlinks {
                std::fs::metadata(&entry_path)
                    .with_context(|| format!("failed to follow {}", entry_path.display()))?
            } else {
                entry_metadata
            };
            let child_archive_path = archive_path.join(Path::new(&file_name));
            append_tar_path_checked(
                builder,
                &child_archive_path,
                &entry_path,
                &entry_metadata,
                follow_symlinks,
                cancel_token,
                operation_type,
                visited_directories,
                scanned_entries,
            )?;
        }
        return Ok(());
    }
    builder
        .append_path_with_name(fs_path, archive_path)
        .with_context(|| format!("failed to archive file {}", fs_path.display()))?;
    Ok(())
}

fn estimate_archive_input_bytes(
    path: &Path,
    metadata: &std::fs::Metadata,
    follow_symlinks: bool,
    cancel_token: &CommandCancelToken,
    operation_type: &'static str,
    visited_directories: &mut HashSet<(u64, u64)>,
    scanned_entries: &mut usize,
) -> Result<u64> {
    cancel_token.check(operation_type)?;
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Ok(0);
    }
    let directory_identity = (metadata.dev(), metadata.ino());
    if !visited_directories.insert(directory_identity) {
        anyhow::bail!("archive size estimation encountered a previously visited directory");
    }
    let mut total = 0_u64;
    for entry in
        std::fs::read_dir(path).with_context(|| format!("failed to read {}", path.display()))?
    {
        cancel_token.check(operation_type)?;
        count_archive_scan_entry(scanned_entries)?;
        let entry = entry?;
        let entry_path = entry.path();
        let entry_metadata = std::fs::symlink_metadata(&entry_path)?;
        let entry_metadata = if entry_metadata.file_type().is_symlink() && follow_symlinks {
            std::fs::metadata(&entry_path)
                .with_context(|| format!("failed to follow {}", entry_path.display()))?
        } else {
            entry_metadata
        };
        total = total
            .checked_add(estimate_archive_input_bytes(
                &entry_path,
                &entry_metadata,
                follow_symlinks,
                cancel_token,
                operation_type,
                visited_directories,
                scanned_entries,
            )?)
            .context("archive source size overflow")?;
    }
    Ok(total)
}

fn count_archive_scan_entry(scanned_entries: &mut usize) -> Result<()> {
    *scanned_entries = scanned_entries
        .checked_add(1)
        .context("archive source entry count overflow")?;
    if *scanned_entries > MAX_FILE_TREE_SCAN_ENTRIES {
        anyhow::bail!(
            "archive source exceeds entry scan limit: {} > {}",
            *scanned_entries,
            MAX_FILE_TREE_SCAN_ENTRIES
        );
    }
    Ok(())
}

fn archive_filename(path: &str) -> String {
    let name = Path::new(path)
        .file_name()
        .and_then(OsStr::to_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("root");
    format!("{name}.tar")
}

fn validate_browser_path(path: &str) -> Result<()> {
    validate_absolute_file_path(path).map_err(|error| anyhow::anyhow!(error.to_string()))
}

fn validate_mutable_path(path: &str) -> Result<()> {
    validate_browser_path(path)?;
    if path == "/" {
        anyhow::bail!("refusing to mutate filesystem root");
    }
    Ok(())
}

fn policy_allows_missing(policy: FileActionPolicy) -> bool {
    matches!(policy, FileActionPolicy::Ensure | FileActionPolicy::Ignore)
}

fn error_chain_has_not_found(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    })
}

fn status_output(job_id: uuid::Uuid, status: Value) -> Result<Vec<CommandOutput>> {
    Ok(vec![CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: Some(0),
        done: true,
    }])
}

#[cfg(test)]
#[path = "tests_file_browser.rs"]
mod tests;
