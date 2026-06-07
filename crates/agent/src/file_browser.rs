use std::{
    ffi::{CString, OsStr, OsString},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tokio::time::{self, Duration};
use vpsman_common::{
    decode_inline_file_payload, payload_hash, validate_absolute_file_path, validate_file_mode,
    CommandOutput, FileActionPolicy, FileOwnershipPolicy, OutputStream,
};

use crate::file_pull::chunked_output;

const MAX_FILE_LIST_LIMIT: u32 = 1000;
const MAX_FILE_READ_BYTES: u64 = 1024 * 1024;
const MAX_FILE_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;

pub(crate) async fn execute_file_browser_command(
    job_id: uuid::Uuid,
    command: &vpsman_common::JobCommand,
    timeout_secs: u64,
) -> Result<Vec<CommandOutput>> {
    time::timeout(
        Duration::from_secs(timeout_secs.max(1)),
        execute_file_browser_command_inner(job_id, command),
    )
    .await
    .context("file browser command timed out")?
}

async fn execute_file_browser_command_inner(
    job_id: uuid::Uuid,
    command: &vpsman_common::JobCommand,
) -> Result<Vec<CommandOutput>> {
    match command {
        vpsman_common::JobCommand::FileStat { path } => execute_file_stat(job_id, path).await,
        vpsman_common::JobCommand::FileListDir {
            path,
            offset,
            limit,
            show_hidden,
        } => execute_file_list_dir(job_id, path, *offset, *limit, *show_hidden).await,
        vpsman_common::JobCommand::FileReadText { path, max_bytes } => {
            execute_file_read_text(job_id, path, *max_bytes).await
        }
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
        } => execute_file_delete(job_id, path, *recursive, *policy).await,
        vpsman_common::JobCommand::FileChmod {
            path,
            mode,
            recursive,
            policy,
        } => execute_file_chmod(job_id, path, *mode, *recursive, *policy).await,
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
            )
            .await
        }
        vpsman_common::JobCommand::FileCopy {
            path,
            new_path,
            overwrite,
            recursive,
            policy,
        } => execute_file_copy(job_id, path, new_path, *overwrite, *recursive, *policy).await,
        vpsman_common::JobCommand::FileArchiveTar { path, max_bytes } => {
            execute_file_archive_tar(job_id, path, *max_bytes).await
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
    let mut read_dir = tokio::fs::read_dir(path)
        .await
        .with_context(|| format!("failed to read directory {path}"))?;
    while let Some(entry) = read_dir
        .next_entry()
        .await
        .with_context(|| format!("failed to read directory entry in {path}"))?
    {
        let name = entry.file_name().to_string_lossy().to_string();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
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
    let total_entries = entries.len();
    let start = (offset as usize).min(total_entries);
    let end = start.saturating_add(limit as usize).min(total_entries);
    let mut rendered = Vec::with_capacity(end.saturating_sub(start));
    for entry in &entries[start..end] {
        rendered.push(metadata_entry_json(entry).await?);
    }
    let status = json!({
        "type": "file_list_dir",
        "path": path,
        "offset": offset,
        "limit": limit,
        "total_entries": total_entries,
        "truncated": end < total_entries,
        "entries": rendered,
        "metadata": metadata_json(Path::new(path), &metadata).await?,
    });
    status_output(job_id, status)
}

async fn execute_file_read_text(
    job_id: uuid::Uuid,
    path: &str,
    max_bytes: u64,
) -> Result<Vec<CommandOutput>> {
    validate_browser_path(path)?;
    let max_bytes = max_bytes.clamp(1, MAX_FILE_READ_BYTES);
    let metadata = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("failed to stat file {path}"))?;
    if !metadata.is_file() {
        anyhow::bail!("file read path is not a regular file");
    }
    if metadata.len() > max_bytes {
        anyhow::bail!(
            "file exceeds text read limit: {} > {max_bytes}",
            metadata.len()
        );
    }
    let data = tokio::fs::read(path)
        .await
        .with_context(|| format!("failed to read file {path}"))?;
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

#[allow(clippy::too_many_arguments)]
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
            if let Ok(current) = tokio::fs::read(destination).await {
                let current_hash = payload_hash(&current);
                if current_hash == sha256_hex.to_ascii_lowercase() {
                    return status_output(
                        job_id,
                        json!({
                            "type": "file_write_text",
                            "path": path,
                            "status": "unchanged",
                            "sha256_hex": current_hash,
                            "size_bytes": current.len(),
                        }),
                    );
                }
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
        let current = tokio::fs::read(destination)
            .await
            .with_context(|| format!("failed to read current file before writing {path}"))?;
        let current_hash = payload_hash(&current);
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
                        "size_bytes": current.len(),
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
    atomic_write(destination, mode, &data, !create).await?;
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
        }),
    )
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
    let target = Path::new(path);
    if let Ok(metadata) = tokio::fs::metadata(target).await {
        if metadata.is_dir() && policy != FileActionPolicy::Fail {
            return status_output(
                job_id,
                json!({"type": "file_mkdir", "path": path, "status": "unchanged"}),
            );
        }
        anyhow::bail!("mkdir target already exists");
    }
    if recursive {
        tokio::fs::create_dir_all(target)
            .await
            .with_context(|| format!("failed to create directory {path}"))?;
    } else {
        tokio::fs::create_dir(target)
            .await
            .with_context(|| format!("failed to create directory {path}"))?;
    }
    tokio::fs::set_permissions(target, std::fs::Permissions::from_mode(mode))
        .await
        .with_context(|| format!("failed to set directory mode on {path}"))?;
    status_output(
        job_id,
        json!({"type": "file_mkdir", "path": path, "status": "created", "mode": mode}),
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
    let source = Path::new(path);
    let destination = Path::new(new_path);
    if tokio::fs::symlink_metadata(source).await.is_err() {
        if policy == FileActionPolicy::Ensure
            && tokio::fs::symlink_metadata(destination).await.is_ok()
        {
            return status_output(
                job_id,
                json!({"type": "file_rename", "path": path, "new_path": new_path, "status": "unchanged"}),
            );
        }
        if policy_allows_missing(policy) {
            return status_output(
                job_id,
                json!({"type": "file_rename", "path": path, "new_path": new_path, "status": "skipped", "reason": "missing"}),
            );
        }
        anyhow::bail!("rename source does not exist");
    }
    if !overwrite && tokio::fs::symlink_metadata(destination).await.is_ok() {
        if policy == FileActionPolicy::Ignore {
            return status_output(
                job_id,
                json!({"type": "file_rename", "path": path, "new_path": new_path, "status": "skipped", "reason": "destination_exists"}),
            );
        }
        anyhow::bail!("rename destination already exists");
    }
    tokio::fs::rename(source, destination)
        .await
        .with_context(|| format!("failed to rename {path} to {new_path}"))?;
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
) -> Result<Vec<CommandOutput>> {
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
    remove_path(target, recursive).await?;
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
    policy: FileActionPolicy,
) -> Result<Vec<CommandOutput>> {
    validate_mutable_path(path)?;
    validate_file_mode(mode).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let target = PathBuf::from(path);
    if tokio::fs::symlink_metadata(&target).await.is_err() {
        if policy_allows_missing(policy) {
            return status_output(
                job_id,
                json!({"type": "file_chmod", "path": path, "status": "skipped", "reason": "missing"}),
            );
        }
        anyhow::bail!("chmod target does not exist");
    }
    tokio::task::spawn_blocking(move || chmod_path(&target, mode, recursive))
        .await
        .context("chmod worker failed")??;
    status_output(
        job_id,
        json!({"type": "file_chmod", "path": path, "status": "changed", "mode": mode, "recursive": recursive}),
    )
}

#[allow(clippy::too_many_arguments)]
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
) -> Result<Vec<CommandOutput>> {
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
    tokio::task::spawn_blocking(move || chown_path_recursive(&target, uid, gid, recursive))
        .await
        .context("chown worker failed")??;
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
    policy: FileActionPolicy,
) -> Result<Vec<CommandOutput>> {
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
        anyhow::bail!("copy source is a symlink");
    }
    let effective_destination = effective_copy_destination(&source, &destination)?;
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
    tokio::task::spawn_blocking(move || {
        copy_path(&source, &effective_destination, recursive, overwrite)
    })
    .await
    .context("copy worker failed")??;
    status_output(
        job_id,
        json!({"type": "file_copy", "path": path, "new_path": new_path, "effective_path": status_path, "status": "copied", "overwrite": overwrite, "recursive": recursive}),
    )
}

async fn execute_file_archive_tar(
    job_id: uuid::Uuid,
    path: &str,
    max_bytes: u64,
) -> Result<Vec<CommandOutput>> {
    validate_browser_path(path)?;
    let max_bytes = max_bytes.clamp(1, MAX_FILE_ARCHIVE_BYTES);
    let source = PathBuf::from(path);
    let archive = tokio::task::spawn_blocking(move || build_tar_archive(&source, max_bytes))
        .await
        .context("archive worker failed")??;
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
        "mode": metadata.mode() & 0o777,
        "uid": metadata.uid(),
        "gid": metadata.gid(),
        "mtime_unix": metadata.mtime(),
        "symlink_target": symlink_target,
    }))
}

async fn atomic_write(destination: &Path, mode: u32, data: &[u8], replace: bool) -> Result<()> {
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .context("file write destination has no parent directory")?;
    let file_name = destination
        .file_name()
        .and_then(OsStr::to_str)
        .context("file write destination has no file name")?;
    let temp_path = parent.join(format!(".vpsman-edit-{file_name}-{}", uuid::Uuid::new_v4()));
    tokio::fs::write(&temp_path, data)
        .await
        .with_context(|| format!("failed to write temporary file {}", temp_path.display()))?;
    tokio::fs::set_permissions(&temp_path, std::fs::Permissions::from_mode(mode))
        .await
        .with_context(|| format!("failed to set file mode on {}", temp_path.display()))?;
    let move_result = if replace {
        tokio::fs::rename(&temp_path, destination).await
    } else {
        rename_no_replace(&temp_path, destination).await
    };
    if let Err(error) = move_result {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(error).with_context(|| {
            format!(
                "failed to move file into place at {}",
                destination.display()
            )
        });
    }
    Ok(())
}

async fn rename_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    let source = source.to_path_buf();
    let destination = destination.to_path_buf();
    tokio::task::spawn_blocking(move || rename_no_replace_blocking(&source, &destination))
        .await
        .map_err(std::io::Error::other)?
}

fn rename_no_replace_blocking(source: &Path, destination: &Path) -> std::io::Result<()> {
    const RENAME_NOREPLACE: libc::c_uint = 1;
    let source = CString::new(source.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "source path contains nul")
    })?;
    let destination = CString::new(destination.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "destination path contains nul",
        )
    })?;
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            RENAME_NOREPLACE,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

async fn remove_path(path: &Path, recursive: bool) -> Result<()> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .with_context(|| format!("failed to stat {}", path.display()))?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        if recursive {
            tokio::fs::remove_dir_all(path)
                .await
                .with_context(|| format!("failed to remove directory {}", path.display()))?;
        } else {
            tokio::fs::remove_dir(path)
                .await
                .with_context(|| format!("failed to remove empty directory {}", path.display()))?;
        }
    } else {
        tokio::fs::remove_file(path)
            .await
            .with_context(|| format!("failed to remove file {}", path.display()))?;
    }
    Ok(())
}

fn chmod_path(path: &Path, mode: u32, recursive: bool) -> Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .with_context(|| format!("failed to chmod {}", path.display()))?;
    if recursive && path.is_dir() {
        for entry in std::fs::read_dir(path)
            .with_context(|| format!("failed to read directory {}", path.display()))?
        {
            let entry = entry?;
            let metadata = std::fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            chmod_path(&entry.path(), mode, true)?;
        }
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
    if owner.is_none() && group.is_none() && uid.is_none() && gid.is_none() {
        return Ok(OwnershipResolution {
            uid: None,
            gid: None,
            owner: None,
            group: None,
            status: OwnershipResolutionStatus::Unchanged,
        });
    }
    let users = read_name_id_file("/etc/passwd");
    let groups = read_name_id_file("/etc/group");
    let mut missing = Vec::new();
    let owner_id = resolve_name_or_id(owner, &users, "owner", &mut missing);
    let group_id = resolve_name_or_id(group, &groups, "group", &mut missing);
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
            .or_else(|| resolved_uid.and_then(|value| name_for_id(&users, value))),
        group: group_id
            .and_then(|value| value.1)
            .or_else(|| resolved_gid.and_then(|value| name_for_id(&groups, value))),
        status: if resolved_uid.is_some() || resolved_gid.is_some() {
            OwnershipResolutionStatus::Planned
        } else {
            OwnershipResolutionStatus::Unchanged
        },
    })
}

fn read_name_id_file(path: &str) -> Vec<(String, u32)> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() < 3 {
                return None;
            }
            Some((parts[0].to_string(), parts[2].parse().ok()?))
        })
        .collect()
}

fn resolve_name_or_id(
    value: Option<&str>,
    entries: &[(String, u32)],
    kind: &str,
    missing: &mut Vec<String>,
) -> Option<(u32, Option<String>)> {
    let value = value.map(str::trim).filter(|value| !value.is_empty())?;
    if let Ok(id) = value.parse::<u32>() {
        return Some((id, name_for_id(entries, id)));
    }
    if let Some((name, id)) = entries.iter().find(|(name, _)| name == value) {
        return Some((*id, Some(name.clone())));
    }
    missing.push(format!("{kind}:{value}"));
    None
}

fn name_for_id(entries: &[(String, u32)], id: u32) -> Option<String> {
    entries
        .iter()
        .find(|(_, candidate_id)| *candidate_id == id)
        .map(|(name, _)| name.clone())
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
) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("failed to stat {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    chown_path(path, uid, gid)?;
    if recursive && metadata.is_dir() {
        for entry in std::fs::read_dir(path)
            .with_context(|| format!("failed to read directory {}", path.display()))?
        {
            let entry = entry?;
            chown_path_recursive(&entry.path(), uid, gid, true)?;
        }
    }
    Ok(())
}

fn chown_path(path: &Path, uid: Option<u32>, gid: Option<u32>) -> Result<()> {
    let path =
        CString::new(path.as_os_str().as_bytes()).context("path contains an interior nul byte")?;
    let uid = uid
        .map(|value| value as libc::uid_t)
        .unwrap_or(!0 as libc::uid_t);
    let gid = gid
        .map(|value| value as libc::gid_t)
        .unwrap_or(!0 as libc::gid_t);
    let result = unsafe { libc::chown(path.as_ptr(), uid, gid) };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).context("failed to change file ownership");
    }
    Ok(())
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

fn copy_path(source: &Path, destination: &Path, recursive: bool, overwrite: bool) -> Result<()> {
    let metadata = std::fs::symlink_metadata(source)
        .with_context(|| format!("failed to stat copy source {}", source.display()))?;
    if metadata.file_type().is_symlink() {
        anyhow::bail!("copy source is a symlink");
    }
    if metadata.is_file() {
        if let Ok(destination_metadata) = std::fs::symlink_metadata(destination) {
            if destination_metadata.is_dir() && !destination_metadata.file_type().is_symlink() {
                anyhow::bail!("cannot overwrite a directory with a file");
            }
            if !overwrite {
                anyhow::bail!("copy destination already exists");
            }
        }
        std::fs::copy(source, destination).with_context(|| {
            format!(
                "failed to copy {} to {}",
                source.display(),
                destination.display()
            )
        })?;
        std::fs::set_permissions(
            destination,
            std::fs::Permissions::from_mode(metadata.mode() & 0o777),
        )
        .with_context(|| format!("failed to set mode on {}", destination.display()))?;
        return Ok(());
    }
    if metadata.is_dir() {
        if !recursive {
            anyhow::bail!("copy source is a directory and recursive is false");
        }
        if let Ok(destination_metadata) = std::fs::symlink_metadata(destination) {
            if !destination_metadata.is_dir() || destination_metadata.file_type().is_symlink() {
                anyhow::bail!("cannot overwrite a non-directory with a directory");
            }
            if !overwrite {
                anyhow::bail!("copy destination already exists");
            }
        } else {
            std::fs::create_dir(destination)
                .with_context(|| format!("failed to create directory {}", destination.display()))?;
        }
        std::fs::set_permissions(
            destination,
            std::fs::Permissions::from_mode(metadata.mode() & 0o777),
        )
        .with_context(|| format!("failed to set mode on {}", destination.display()))?;
        for entry in std::fs::read_dir(source)
            .with_context(|| format!("failed to read directory {}", source.display()))?
        {
            let entry = entry?;
            copy_path(
                &entry.path(),
                &destination.join(entry.file_name()),
                true,
                overwrite,
            )?;
        }
        return Ok(());
    }
    anyhow::bail!("copy source is not a regular file or directory");
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
        return Ok(std::fs::read(source)
            .with_context(|| format!("failed to read {}", source.display()))?
            == std::fs::read(destination)
                .with_context(|| format!("failed to read {}", destination.display()))?);
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

fn build_tar_archive(source: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(source)
        .with_context(|| format!("failed to stat archive source {}", source.display()))?;
    let estimated_size = estimate_archive_input_bytes(source, &metadata)?;
    if estimated_size > max_bytes {
        anyhow::bail!("archive source exceeds limit: {estimated_size} > {max_bytes}");
    }
    let mut archive = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut archive);
        let name = source
            .file_name()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| OsStr::new("root"));
        if metadata.is_dir() {
            builder
                .append_dir_all(name, source)
                .with_context(|| format!("failed to archive directory {}", source.display()))?;
        } else {
            builder
                .append_path_with_name(source, name)
                .with_context(|| format!("failed to archive file {}", source.display()))?;
        }
        builder.finish().context("failed to finish tar archive")?;
    }
    if archive.len() as u64 > max_bytes {
        anyhow::bail!("tar archive exceeds limit: {} > {max_bytes}", archive.len());
    }
    Ok(archive)
}

fn estimate_archive_input_bytes(path: &Path, metadata: &std::fs::Metadata) -> Result<u64> {
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Ok(0);
    }
    let mut total = 0_u64;
    for entry in
        std::fs::read_dir(path).with_context(|| format!("failed to read {}", path.display()))?
    {
        let entry = entry?;
        let entry_metadata = std::fs::symlink_metadata(entry.path())?;
        total = total.saturating_add(estimate_archive_input_bytes(
            &entry.path(),
            &entry_metadata,
        )?);
    }
    Ok(total)
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
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use uuid::Uuid;

    use super::*;

    #[tokio::test]
    async fn list_read_and_write_text_file() {
        let root = test_root("list-read-write");
        fs::create_dir_all(&root).unwrap();
        let file = root.join("hello.txt");
        fs::write(&file, "hello").unwrap();

        let list = execute_file_list_dir(Uuid::new_v4(), root.to_str().unwrap(), 0, 50, false)
            .await
            .unwrap();
        let list_status: Value = serde_json::from_slice(&list[0].data).unwrap();
        assert_eq!(list_status["entries"][0]["name"], "hello.txt");

        let read = execute_file_read_text(Uuid::new_v4(), file.to_str().unwrap(), 1024)
            .await
            .unwrap();
        let read_status: Value = serde_json::from_slice(&read[0].data).unwrap();
        assert_eq!(read_status["sha256_hex"], payload_hash(b"hello"));

        let next = b"updated";
        let write = execute_file_write_text(
            Uuid::new_v4(),
            file.to_str().unwrap(),
            0o640,
            next.len() as u64,
            &payload_hash(next),
            &base64::Engine::encode(&base64::engine::general_purpose::STANDARD, next),
            Some(payload_hash(b"hello").as_str()),
            false,
            FileActionPolicy::Fail,
        )
        .await
        .unwrap();
        let write_status: Value = serde_json::from_slice(&write[0].data).unwrap();
        assert_eq!(write_status["status"], "updated");
        assert_eq!(fs::read_to_string(&file).unwrap(), "updated");
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o640
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn stale_text_write_fails() {
        let root = test_root("stale-write");
        fs::create_dir_all(&root).unwrap();
        let file = root.join("hello.txt");
        fs::write(&file, "new").unwrap();
        let result = execute_file_write_text(
            Uuid::new_v4(),
            file.to_str().unwrap(),
            0o644,
            7,
            &payload_hash(b"updated"),
            &base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"updated"),
            Some(payload_hash(b"old").as_str()),
            false,
            FileActionPolicy::Fail,
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("changed"));
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn create_text_write_refuses_existing_file() {
        let root = test_root("create-existing-write");
        fs::create_dir_all(&root).unwrap();
        let file = root.join("hello.txt");
        fs::write(&file, "original").unwrap();
        let result = execute_file_write_text(
            Uuid::new_v4(),
            file.to_str().unwrap(),
            0o644,
            7,
            &payload_hash(b"updated"),
            &base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"updated"),
            None,
            true,
            FileActionPolicy::Fail,
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("already exists"));
        assert_eq!(fs::read_to_string(&file).unwrap(), "original");
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn expected_hash_read_error_fails_closed() {
        let root = test_root("write-read-error");
        fs::create_dir_all(&root).unwrap();
        let file = root.join("hello.txt");
        fs::write(&file, "old").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
        let result = execute_file_write_text(
            Uuid::new_v4(),
            file.to_str().unwrap(),
            0o644,
            7,
            &payload_hash(b"updated"),
            &base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"updated"),
            Some(payload_hash(b"old").as_str()),
            false,
            FileActionPolicy::Fail,
        )
        .await;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(result.unwrap_err().to_string().contains("failed to read"));
        assert_eq!(fs::read_to_string(&file).unwrap(), "old");
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn copy_overwrite_merges_existing_directory_without_deleting_it() {
        let root = test_root("copy-merge-directory");
        let source = root.join("src");
        let destination = root.join("dest");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(source.join("new.txt"), "new").unwrap();
        fs::write(destination.join("keep.txt"), "keep").unwrap();
        let output = execute_file_copy(
            Uuid::new_v4(),
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
            true,
            true,
            FileActionPolicy::Fail,
        )
        .await
        .unwrap();
        let status: Value = serde_json::from_slice(&output[0].data).unwrap();
        assert_eq!(status["status"], "copied");
        assert_eq!(
            fs::read_to_string(destination.join("keep.txt")).unwrap(),
            "keep"
        );
        assert_eq!(
            fs::read_to_string(destination.join("src").join("new.txt")).unwrap(),
            "new"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn copy_ensure_existing_matching_destination_is_unchanged() {
        let root = test_root("copy-ensure-matching");
        let source = root.join("src");
        let destination = root.join("dest");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::create_dir_all(destination.join("src").join("nested")).unwrap();
        fs::write(source.join("nested").join("app.conf"), "same").unwrap();
        fs::write(
            destination.join("src").join("nested").join("app.conf"),
            "same",
        )
        .unwrap();
        let output = execute_file_copy(
            Uuid::new_v4(),
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
            false,
            true,
            FileActionPolicy::Ensure,
        )
        .await
        .unwrap();
        let status: Value = serde_json::from_slice(&output[0].data).unwrap();
        assert_eq!(status["status"], "unchanged");
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn rename_overwrite_does_not_delete_incompatible_destination() {
        let root = test_root("rename-incompatible-destination");
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source.txt");
        let destination = root.join("dest");
        fs::write(&source, "source").unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(destination.join("keep.txt"), "keep").unwrap();
        let result = execute_file_rename(
            Uuid::new_v4(),
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
            true,
            FileActionPolicy::Fail,
        )
        .await;
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(destination.join("keep.txt")).unwrap(),
            "keep"
        );
        assert_eq!(fs::read_to_string(&source).unwrap(), "source");
        let _ = fs::remove_dir_all(&root);
    }

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("vpsman-file-browser-{name}-{}", Uuid::new_v4()))
    }
}
