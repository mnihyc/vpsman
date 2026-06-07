use std::{
    ffi::CString,
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, PermissionsExt},
    },
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
    validate_file_transfer_session_token, CommandOutput, FileExistingPolicy, FileOwnershipPolicy,
    FilePushChunk, OutputStream, FILE_TRANSFER_CHUNK_BYTES,
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
    #[serde(default)]
    existing_policy: FileExistingPolicy,
    resume_token_hash: String,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_file_push(
    job_id: uuid::Uuid,
    path: &str,
    mode: u32,
    size_bytes: u64,
    sha256_hex: &str,
    data_base64: &str,
    existing_policy: FileExistingPolicy,
    owner: Option<&str>,
    group: Option<&str>,
    uid: Option<u32>,
    gid: Option<u32>,
    ownership_policy: FileOwnershipPolicy,
) -> Result<Vec<CommandOutput>> {
    validate_file_push_request(path, mode)?;
    let data = decode_inline_file_payload(data_base64, size_bytes, sha256_hex)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    write_file_push(
        job_id,
        path,
        mode,
        &data,
        "file_push",
        None,
        existing_policy,
        owner,
        group,
        uid,
        gid,
        ownership_policy,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_file_push_chunked(
    job_id: uuid::Uuid,
    path: &str,
    mode: u32,
    size_bytes: u64,
    sha256_hex: &str,
    chunks: &[FilePushChunk],
    existing_policy: FileExistingPolicy,
    owner: Option<&str>,
    group: Option<&str>,
    uid: Option<u32>,
    gid: Option<u32>,
    ownership_policy: FileOwnershipPolicy,
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
        existing_policy,
        owner,
        group,
        uid,
        gid,
        ownership_policy,
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
    existing_policy: FileExistingPolicy,
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
            existing_policy,
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
                "existing_policy": file_existing_policy_label(metadata.existing_policy),
            }),
        );
    }

    if paths.temp.exists() {
        anyhow::bail!("file transfer temporary file exists without session metadata");
    }
    if let Ok(metadata) = tokio::fs::symlink_metadata(&paths.destination).await {
        if metadata.is_dir() {
            anyhow::bail!("file transfer destination is a directory");
        }
        if existing_policy == FileExistingPolicy::Skip {
            return transfer_status(
                job_id,
                "file_transfer_start",
                session_id,
                path,
                size_bytes,
                Some(size_bytes),
                serde_json::json!({
                    "resumed": false,
                    "skipped": true,
                    "reason": "destination_exists",
                    "existing_policy": file_existing_policy_label(existing_policy),
                }),
            );
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
        existing_policy,
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
            "existing_policy": file_existing_policy_label(existing_policy),
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
    let move_result = match metadata.existing_policy {
        FileExistingPolicy::Replace => tokio::fs::rename(&paths.temp, &paths.destination).await,
        FileExistingPolicy::Skip => rename_no_replace(&paths.temp, &paths.destination).await,
    };
    if let Err(error) = move_result {
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
            "existing_policy": file_existing_policy_label(metadata.existing_policy),
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

#[allow(clippy::too_many_arguments)]
async fn write_file_push(
    job_id: uuid::Uuid,
    path: &str,
    mode: u32,
    data: &[u8],
    status_type: &'static str,
    chunk_count: Option<usize>,
    existing_policy: FileExistingPolicy,
    owner: Option<&str>,
    group: Option<&str>,
    uid: Option<u32>,
    gid: Option<u32>,
    ownership_policy: FileOwnershipPolicy,
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
    let ownership_plan = resolve_ownership(owner, group, uid, gid, ownership_policy)?;

    if let Ok(metadata) = tokio::fs::symlink_metadata(destination).await {
        if metadata.is_dir() {
            anyhow::bail!("file push destination is a directory");
        }
        if existing_policy == FileExistingPolicy::Skip {
            return file_push_status(
                job_id,
                status_type,
                path,
                mode,
                data,
                chunk_count,
                existing_policy,
                "skipped",
                Some("destination_exists"),
                &metadata,
                &ownership_plan,
            );
        }
    }
    tokio::fs::write(&temp_path, data)
        .await
        .with_context(|| format!("failed to write temporary file {}", temp_path.display()))?;
    tokio::fs::set_permissions(&temp_path, std::fs::Permissions::from_mode(mode))
        .await
        .with_context(|| format!("failed to set file mode on {}", temp_path.display()))?;
    if let Err(error) = apply_ownership_plan(&temp_path, &ownership_plan).await {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(error);
    }
    match existing_policy {
        FileExistingPolicy::Replace => {
            if let Err(error) = tokio::fs::rename(&temp_path, destination).await {
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err(error).with_context(|| {
                    format!(
                        "failed to move file into place at {}",
                        destination.display()
                    )
                });
            }
        }
        FileExistingPolicy::Skip => {
            if let Err(error) = rename_no_replace(&temp_path, destination).await {
                let _ = tokio::fs::remove_file(&temp_path).await;
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    let metadata = tokio::fs::symlink_metadata(destination)
                        .await
                        .with_context(|| {
                            format!(
                                "failed to stat existing destination {}",
                                destination.display()
                            )
                        })?;
                    return file_push_status(
                        job_id,
                        status_type,
                        path,
                        mode,
                        data,
                        chunk_count,
                        existing_policy,
                        "skipped",
                        Some("destination_exists"),
                        &metadata,
                        &ownership_plan,
                    );
                }
                return Err(error).with_context(|| {
                    format!(
                        "failed to move file into place at {}",
                        destination.display()
                    )
                });
            }
        }
    }

    let metadata = tokio::fs::symlink_metadata(destination)
        .await
        .with_context(|| format!("failed to stat uploaded file {}", destination.display()))?;
    file_push_status(
        job_id,
        status_type,
        path,
        mode,
        data,
        chunk_count,
        existing_policy,
        "completed",
        None,
        &metadata,
        &ownership_plan,
    )
}

#[allow(clippy::too_many_arguments)]
fn file_push_status(
    job_id: uuid::Uuid,
    status_type: &'static str,
    path: &str,
    mode: u32,
    data: &[u8],
    chunk_count: Option<usize>,
    existing_policy: FileExistingPolicy,
    status_text: &'static str,
    reason: Option<&'static str>,
    metadata: &std::fs::Metadata,
    ownership_plan: &OwnershipPlan,
) -> Result<Vec<CommandOutput>> {
    let (ownership_status, ownership_reason) = if status_text == "skipped" {
        ("unchanged", None)
    } else {
        ownership_status_label(&ownership_plan.status)
    };
    let status = serde_json::json!({
        "type": status_type,
        "status": status_text,
        "reason": reason,
        "path": path,
        "size_bytes": data.len(),
        "sha256_hex": payload_hash(data),
        "mode": metadata.mode() & 0o777,
        "requested_mode": mode,
        "uid": metadata.uid(),
        "gid": metadata.gid(),
        "owner": metadata_owner_name(metadata, ownership_plan),
        "group": metadata_group_name(metadata, ownership_plan),
        "overwrite_policy": file_existing_policy_label(existing_policy),
        "ownership_status": ownership_status,
        "ownership_reason": ownership_reason,
        "atomic": status_text == "completed",
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

#[derive(Clone, Debug)]
struct OwnershipPlan {
    uid: Option<u32>,
    gid: Option<u32>,
    owner: Option<String>,
    group: Option<String>,
    status: OwnershipPlanStatus,
}

#[derive(Clone, Debug)]
enum OwnershipPlanStatus {
    Unchanged,
    Planned,
    Skipped(&'static str),
}

impl OwnershipPlan {
    fn unchanged() -> Self {
        Self {
            uid: None,
            gid: None,
            owner: None,
            group: None,
            status: OwnershipPlanStatus::Unchanged,
        }
    }

    fn skipped(reason: &'static str) -> Self {
        Self {
            uid: None,
            gid: None,
            owner: None,
            group: None,
            status: OwnershipPlanStatus::Skipped(reason),
        }
    }
}

fn resolve_ownership(
    owner: Option<&str>,
    group: Option<&str>,
    uid: Option<u32>,
    gid: Option<u32>,
    ownership_policy: FileOwnershipPolicy,
) -> Result<OwnershipPlan> {
    if owner.is_none() && group.is_none() && uid.is_none() && gid.is_none() {
        return Ok(OwnershipPlan::unchanged());
    }

    let users = PasswdEntries::load();
    let groups = GroupEntries::load();
    let mut missing = Vec::new();
    let owner_resolution = match owner.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => resolve_owner_value(value, &users, &mut missing),
        None => None,
    };
    let group_resolution = match group.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => resolve_group_value(value, &groups, &mut missing),
        None => None,
    };

    if !missing.is_empty() {
        if ownership_policy == FileOwnershipPolicy::Ignore {
            return Ok(OwnershipPlan::skipped("missing_owner_or_group"));
        }
        anyhow::bail!("missing owner/group: {}", missing.join(", "));
    }

    let resolved_uid = merge_optional_id(
        "owner",
        uid,
        owner_resolution.as_ref().map(|value| value.id),
    )?;
    let resolved_gid = merge_optional_id(
        "group",
        gid,
        group_resolution.as_ref().map(|value| value.id),
    )?;
    if resolved_uid.is_none() && resolved_gid.is_none() {
        return Ok(OwnershipPlan::unchanged());
    }

    Ok(OwnershipPlan {
        uid: resolved_uid,
        gid: resolved_gid,
        owner: owner_resolution
            .and_then(|value| value.name)
            .or_else(|| resolved_uid.and_then(|value| users.name_for_id(value))),
        group: group_resolution
            .and_then(|value| value.name)
            .or_else(|| resolved_gid.and_then(|value| groups.name_for_id(value))),
        status: OwnershipPlanStatus::Planned,
    })
}

#[derive(Clone, Debug)]
struct IdResolution {
    id: u32,
    name: Option<String>,
}

fn resolve_owner_value(
    value: &str,
    users: &PasswdEntries,
    missing: &mut Vec<String>,
) -> Option<IdResolution> {
    if let Ok(id) = value.parse::<u32>() {
        return Some(IdResolution {
            id,
            name: users.name_for_id(id),
        });
    }
    if let Some(id) = users.id_for_name(value) {
        return Some(IdResolution {
            id,
            name: Some(value.to_string()),
        });
    }
    missing.push(format!("owner:{value}"));
    None
}

fn resolve_group_value(
    value: &str,
    groups: &GroupEntries,
    missing: &mut Vec<String>,
) -> Option<IdResolution> {
    if let Ok(id) = value.parse::<u32>() {
        return Some(IdResolution {
            id,
            name: groups.name_for_id(id),
        });
    }
    if let Some(id) = groups.id_for_name(value) {
        return Some(IdResolution {
            id,
            name: Some(value.to_string()),
        });
    }
    missing.push(format!("group:{value}"));
    None
}

fn merge_optional_id(
    kind: &str,
    explicit: Option<u32>,
    resolved: Option<u32>,
) -> Result<Option<u32>> {
    match (explicit, resolved) {
        (Some(left), Some(right)) if left != right => {
            anyhow::bail!("{kind} id conflicts with resolved {kind} name")
        }
        (Some(value), _) | (_, Some(value)) => Ok(Some(value)),
        (None, None) => Ok(None),
    }
}

#[derive(Clone, Debug, Default)]
struct PasswdEntries {
    entries: Vec<NameIdEntry>,
}

#[derive(Clone, Debug, Default)]
struct GroupEntries {
    entries: Vec<NameIdEntry>,
}

#[derive(Clone, Debug)]
struct NameIdEntry {
    name: String,
    id: u32,
}

impl PasswdEntries {
    fn load() -> Self {
        let data = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
        let entries = data
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() < 3 {
                    return None;
                }
                Some(NameIdEntry {
                    name: parts[0].to_string(),
                    id: parts[2].parse().ok()?,
                })
            })
            .collect();
        Self { entries }
    }

    fn id_for_name(&self, name: &str) -> Option<u32> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.id)
    }

    fn name_for_id(&self, id: u32) -> Option<String> {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.name.clone())
    }
}

impl GroupEntries {
    fn load() -> Self {
        let data = std::fs::read_to_string("/etc/group").unwrap_or_default();
        let entries = data
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() < 3 {
                    return None;
                }
                Some(NameIdEntry {
                    name: parts[0].to_string(),
                    id: parts[2].parse().ok()?,
                })
            })
            .collect();
        Self { entries }
    }

    fn id_for_name(&self, name: &str) -> Option<u32> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.id)
    }

    fn name_for_id(&self, id: u32) -> Option<String> {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.name.clone())
    }
}

async fn apply_ownership_plan(path: &Path, ownership: &OwnershipPlan) -> Result<()> {
    if !matches!(ownership.status, OwnershipPlanStatus::Planned) {
        return Ok(());
    }
    let path = path.to_path_buf();
    let uid = ownership.uid;
    let gid = ownership.gid;
    tokio::task::spawn_blocking(move || chown_path(&path, uid, gid))
        .await
        .context("chown worker failed")?
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

fn file_existing_policy_label(policy: FileExistingPolicy) -> &'static str {
    match policy {
        FileExistingPolicy::Skip => "skip",
        FileExistingPolicy::Replace => "replace",
    }
}

fn ownership_status_label(status: &OwnershipPlanStatus) -> (&'static str, Option<&'static str>) {
    match status {
        OwnershipPlanStatus::Unchanged => ("unchanged", None),
        OwnershipPlanStatus::Planned => ("applied", None),
        OwnershipPlanStatus::Skipped(reason) => ("skipped", Some(*reason)),
    }
}

fn metadata_owner_name(metadata: &std::fs::Metadata, plan: &OwnershipPlan) -> Option<String> {
    plan.owner
        .clone()
        .or_else(|| PasswdEntries::load().name_for_id(metadata.uid()))
}

fn metadata_group_name(metadata: &std::fs::Metadata, plan: &OwnershipPlan) -> Option<String> {
    plan.group
        .clone()
        .or_else(|| GroupEntries::load().name_for_id(metadata.gid()))
}

fn validate_file_push_request(path: &str, mode: u32) -> Result<()> {
    validate_absolute_file_path(path).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    if path == "/" {
        anyhow::bail!("refusing to write filesystem root");
    }
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
    if path == "/" {
        anyhow::bail!("refusing to write filesystem root");
    }
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
    existing_policy: FileExistingPolicy,
    resume_token_hash: &str,
) -> Result<()> {
    if metadata.path != path
        || metadata.mode != mode
        || metadata.size_bytes != size_bytes
        || metadata.sha256_hex != sha256_hex.to_ascii_lowercase()
        || metadata.chunk_size_bytes != chunk_size_bytes
        || metadata.rate_limit_kbps != rate_limit_kbps
        || metadata.existing_policy != existing_policy
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
