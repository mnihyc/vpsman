use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::time::{sleep, Duration};
use uuid::Uuid;
use vpsman_common::{
    decode_chunked_file_payload, decode_file_transfer_chunk, decode_inline_file_payload,
    payload_hash, validate_absolute_file_path, validate_file_mode, validate_file_transfer_session,
    validate_file_transfer_session_token, CommandOutput, FileExistingPolicy, FileOwnershipPolicy,
    FilePushChunk, OutputStream, FILE_TRANSFER_CHUNK_BYTES,
};

use crate::{
    command_worker::CommandCancelToken,
    platform_accounts::{
        current_effective_uid, metadata_gid, metadata_mode, metadata_uid,
        normalize_ownership_tokens, NameIdResolution, PlatformAccounts,
    },
    safe_fs,
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
    temp_identity: safe_fs::NodeIdentity,
}

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
    cancel_token: CommandCancelToken,
) -> Result<Vec<CommandOutput>> {
    cancel_token.check("file_transfer_start")?;
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
        let next_offset =
            current_transfer_offset(&paths.temp, size_bytes, &metadata.temp_identity).await?;
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

    if transfer_temp_exists(&paths.temp).await? {
        anyhow::bail!("file transfer temporary file exists without session metadata");
    }
    if let Ok(metadata) = tokio::fs::symlink_metadata(&paths.destination).await {
        if metadata.is_dir() {
            anyhow::bail!("file transfer destination is a directory");
        }
        if existing_policy == FileExistingPolicy::Skip {
            cancel_token.check("file_transfer_start")?;
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
    cancel_token.check("file_transfer_start")?;
    let temp_identity = create_transfer_temp_file(&paths.temp).await?;
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
        temp_identity,
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
    cancel_token: CommandCancelToken,
) -> Result<Vec<CommandOutput>> {
    cancel_token.check("file_transfer_chunk")?;
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
    let current_offset =
        current_transfer_offset(&paths.temp, metadata.size_bytes, &metadata.temp_identity).await?;
    let next_offset = if offset == current_offset {
        cancel_token.check("file_transfer_chunk")?;
        write_transfer_chunk(&paths.temp, &metadata.temp_identity, offset, &decoded).await?;
        maybe_throttle_complete_on_cancel(metadata.rate_limit_kbps, decoded.len(), &cancel_token)
            .await;
        offset + decoded.len() as u64
    } else if offset < current_offset && offset + decoded.len() as u64 <= current_offset {
        cancel_token.check("file_transfer_chunk")?;
        verify_existing_chunk(&paths.temp, &metadata.temp_identity, offset, &decoded).await?;
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
    cancel_token: CommandCancelToken,
) -> Result<Vec<CommandOutput>> {
    cancel_token.check("file_transfer_commit")?;
    validate_file_transfer_session_token(session_id, resume_token_hash)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let metadata = read_metadata_by_session(session_id).await?;
    ensure_resume_token(&metadata, resume_token_hash)?;
    let paths = transfer_session_paths(&metadata.path, session_id)?;
    let current_offset =
        current_transfer_offset(&paths.temp, metadata.size_bytes, &metadata.temp_identity).await?;
    if current_offset != metadata.size_bytes {
        anyhow::bail!("file transfer commit before all bytes are received");
    }
    let actual_hash = hash_file(
        &paths.temp,
        &metadata.temp_identity,
        &cancel_token,
        "file_transfer_commit",
    )
    .await?;
    if actual_hash != metadata.sha256_hex {
        anyhow::bail!("file transfer final hash mismatch");
    }
    cancel_token.check("file_transfer_commit")?;
    chmod_transfer_temp(&paths.temp, &metadata.temp_identity, metadata.mode).await?;
    cancel_token.check("file_transfer_commit")?;
    commit_transfer_temp(&paths, &metadata).await?;
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
    cancel_token: CommandCancelToken,
) -> Result<Vec<CommandOutput>> {
    cancel_token.check("file_transfer_abort")?;
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
    existing_policy: FileExistingPolicy,
    owner: Option<&str>,
    group: Option<&str>,
    uid: Option<u32>,
    gid: Option<u32>,
    ownership_policy: FileOwnershipPolicy,
) -> Result<Vec<CommandOutput>> {
    let destination = Path::new(path);
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
    let destination = destination.to_path_buf();
    let destination_for_worker = destination.clone();
    let payload = data.to_vec();
    let ownership_for_worker = ownership_plan.clone();
    let commit = tokio::task::spawn_blocking(move || {
        write_file_push_blocking(
            &destination_for_worker,
            mode,
            &payload,
            existing_policy,
            &ownership_for_worker,
        )
    })
    .await
    .context("file push worker failed")??;
    if let FilePushCommit::Skipped(metadata) = commit {
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

    let metadata = tokio::fs::symlink_metadata(&destination)
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

enum FilePushCommit {
    Completed,
    Skipped(std::fs::Metadata),
}

fn write_file_push_blocking(
    destination: &Path,
    mode: u32,
    data: &[u8],
    existing_policy: FileExistingPolicy,
    ownership_plan: &OwnershipPlan,
) -> Result<FilePushCommit> {
    let parent = safe_fs::resolve_parent(destination)?;
    let (mut temp_file, temp_name) =
        safe_fs::create_private_temp_file(parent.dir(), parent.name(), "upload")?;
    let result = (|| -> Result<FilePushCommit> {
        temp_file.write_all(data).with_context(|| {
            format!(
                "failed to write temporary file for {}",
                destination.display()
            )
        })?;
        safe_fs::fchmod_file(&temp_file, mode)?;
        apply_ownership_plan_to_file(&temp_file, ownership_plan)?;
        temp_file.sync_all().with_context(|| {
            format!(
                "failed to sync temporary file for {}",
                destination.display()
            )
        })?;
        let replace = existing_policy == FileExistingPolicy::Replace;
        match safe_fs::rename_child(
            parent.dir(),
            &temp_name,
            parent.dir(),
            parent.name(),
            replace,
        ) {
            Ok(()) => {
                safe_fs::sync_dir_best_effort(parent.dir());
                Ok(FilePushCommit::Completed)
            }
            Err(error)
                if existing_policy == FileExistingPolicy::Skip
                    && error.kind() == std::io::ErrorKind::AlreadyExists =>
            {
                let metadata = std::fs::symlink_metadata(destination).with_context(|| {
                    format!(
                        "failed to stat existing destination {}",
                        destination.display()
                    )
                })?;
                Ok(FilePushCommit::Skipped(metadata))
            }
            Err(error) => Err(error).with_context(|| {
                format!(
                    "failed to move file into place at {}",
                    destination.display()
                )
            }),
        }
    })();
    if !matches!(result, Ok(FilePushCommit::Completed)) {
        let _ = safe_fs::remove_child_file(parent.dir(), &temp_name);
    }
    result
}

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
        "mode": metadata_mode(metadata).map(|mode| mode & 0o777),
        "requested_mode": mode,
        "uid": metadata_uid(metadata),
        "gid": metadata_gid(metadata),
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
    let tokens = normalize_ownership_tokens(owner, group, uid, gid)?;
    let owner = tokens.owner.as_deref();
    let group = tokens.group.as_deref();
    if owner.is_none() && group.is_none() && uid.is_none() && gid.is_none() {
        return Ok(OwnershipPlan::unchanged());
    }

    let accounts = PlatformAccounts::load();
    let mut missing = Vec::new();
    let owner_resolution = match owner.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => resolve_owner_value(value, &accounts, &mut missing),
        None => None,
    };
    let group_resolution = match group.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => resolve_group_value(value, &accounts, &mut missing),
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
            .or_else(|| resolved_uid.and_then(|value| accounts.user_name_for_id(value))),
        group: group_resolution
            .and_then(|value| value.name)
            .or_else(|| resolved_gid.and_then(|value| accounts.group_name_for_id(value))),
        status: OwnershipPlanStatus::Planned,
    })
}

fn resolve_owner_value(
    value: &str,
    accounts: &PlatformAccounts,
    missing: &mut Vec<String>,
) -> Option<NameIdResolution> {
    if let Ok(id) = value.parse::<u32>() {
        return Some(NameIdResolution {
            id,
            name: accounts.user_name_for_id(id),
        });
    }
    if let Some(resolution) = accounts.resolve_user(value) {
        return Some(resolution);
    }
    missing.push(format!("owner:{value}"));
    None
}

fn resolve_group_value(
    value: &str,
    accounts: &PlatformAccounts,
    missing: &mut Vec<String>,
) -> Option<NameIdResolution> {
    if let Ok(id) = value.parse::<u32>() {
        return Some(NameIdResolution {
            id,
            name: accounts.group_name_for_id(id),
        });
    }
    if let Some(resolution) = accounts.resolve_group(value) {
        return Some(resolution);
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

fn apply_ownership_plan_to_file(file: &File, ownership: &OwnershipPlan) -> Result<()> {
    if !matches!(ownership.status, OwnershipPlanStatus::Planned) {
        return Ok(());
    }
    safe_fs::fchown_file(file, ownership.uid, ownership.gid)
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
    plan.owner.clone().or_else(|| {
        metadata_uid(metadata).and_then(|uid| PlatformAccounts::load().user_name_for_id(uid))
    })
}

fn metadata_group_name(metadata: &std::fs::Metadata, plan: &OwnershipPlan) -> Option<String> {
    plan.group.clone().or_else(|| {
        metadata_gid(metadata).and_then(|gid| PlatformAccounts::load().group_name_for_id(gid))
    })
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

async fn transfer_temp_exists(path: &Path) -> Result<bool> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let parent = safe_fs::resolve_parent(&path)?;
        Ok(parent.child_stat_nofollow()?.is_some())
    })
    .await
    .context("file transfer temp stat worker failed")?
}

async fn create_transfer_temp_file(path: &Path) -> Result<safe_fs::NodeIdentity> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let parent = safe_fs::resolve_parent(&path)?;
        let file = safe_fs::create_private_child_file(parent.dir(), parent.name())
            .with_context(|| format!("failed to create transfer temp file {}", path.display()))?;
        let identity = safe_fs::stat_file(&file)?.identity;
        file.sync_all()
            .with_context(|| format!("failed to sync transfer temp file {}", path.display()))?;
        safe_fs::sync_dir_best_effort(parent.dir());
        Ok(identity)
    })
    .await
    .context("file transfer temp create worker failed")?
}

async fn chmod_transfer_temp(
    path: &Path,
    expected_identity: &safe_fs::NodeIdentity,
    mode: u32,
) -> Result<()> {
    let path = path.to_path_buf();
    let expected_identity = expected_identity.clone();
    tokio::task::spawn_blocking(move || {
        let parent = safe_fs::resolve_parent(&path)?;
        let file = parent.open_child_readwrite_nofollow()?;
        safe_fs::ensure_identity(
            &file,
            &expected_identity,
            "file transfer temporary file changed",
        )?;
        safe_fs::fchmod_file(&file, mode)
            .with_context(|| format!("failed to set file mode on {}", path.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to sync transfer temp file {}", path.display()))?;
        Ok(())
    })
    .await
    .context("file transfer chmod worker failed")?
}

async fn commit_transfer_temp(
    paths: &TransferSessionPaths,
    metadata: &FileTransferSessionMetadata,
) -> Result<()> {
    let temp = paths.temp.clone();
    let destination = paths.destination.clone();
    let expected_identity = metadata.temp_identity.clone();
    let replace = metadata.existing_policy == FileExistingPolicy::Replace;
    tokio::task::spawn_blocking(move || {
        let temp_parent = safe_fs::resolve_parent(&temp)?;
        let destination_parent = safe_fs::resolve_parent(&destination)?;
        let temp_file = temp_parent.open_child_readwrite_nofollow()?;
        safe_fs::ensure_identity(
            &temp_file,
            &expected_identity,
            "file transfer temporary file changed",
        )?;
        temp_file
            .sync_all()
            .with_context(|| format!("failed to sync transfer temp file {}", temp.display()))?;
        safe_fs::rename_child(
            temp_parent.dir(),
            temp_parent.name(),
            destination_parent.dir(),
            destination_parent.name(),
            replace,
        )
        .with_context(|| {
            format!(
                "failed to move file into place at {}",
                destination.display()
            )
        })?;
        safe_fs::sync_dir_best_effort(temp_parent.dir());
        safe_fs::sync_dir_best_effort(destination_parent.dir());
        Ok(())
    })
    .await
    .context("file transfer commit worker failed")?
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
    let metadata = transfer_metadata_path(session_id)?;
    Ok(TransferSessionPaths {
        destination,
        temp,
        metadata,
    })
}

fn transfer_metadata_path(session_id: Uuid) -> Result<PathBuf> {
    let root = std::env::temp_dir().join("vpsman-agent-transfer-sessions");
    let (dir, _) = safe_fs::ensure_dir_all_no_symlinks_with_mode(&root, 0o700)?;
    let metadata = safe_fs::stat_file(&dir)?;
    if metadata.uid != current_effective_uid() {
        anyhow::bail!("file transfer metadata directory is not owned by the agent user");
    }
    safe_fs::fchmod_file(&dir, 0o700)?;
    safe_fs::sync_dir_best_effort(&dir);
    Ok(root.join(format!("{session_id}.json")))
}

async fn read_metadata_by_session(session_id: Uuid) -> Result<FileTransferSessionMetadata> {
    if session_id.is_nil() {
        anyhow::bail!("file transfer session id is invalid");
    }
    let path = transfer_metadata_path(session_id)?;
    read_transfer_metadata(&path).await
}

fn read_private_file_no_symlink(path: &Path) -> Result<Vec<u8>> {
    let parent = safe_fs::resolve_parent(path)?;
    let mut file = parent.open_child_file_read(false)?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)
        .with_context(|| format!("failed to read {}", path.display()))?;
    Ok(data)
}

fn write_private_file_no_replace(path: &Path, data: &[u8]) -> Result<()> {
    let parent = safe_fs::resolve_parent(path)?;
    let (mut temp_file, temp_name) =
        safe_fs::create_private_temp_file(parent.dir(), parent.name(), "metadata")?;
    let result = (|| -> Result<()> {
        temp_file
            .write_all(data)
            .with_context(|| format!("failed to write {}", path.display()))?;
        safe_fs::fchmod_file(&temp_file, 0o600)?;
        temp_file
            .sync_all()
            .with_context(|| format!("failed to sync {}", path.display()))?;
        safe_fs::rename_child(parent.dir(), &temp_name, parent.dir(), parent.name(), false)
            .with_context(|| format!("failed to publish {}", path.display()))?;
        safe_fs::sync_dir_best_effort(parent.dir());
        Ok(())
    })();
    if result.is_err() {
        let _ = safe_fs::remove_child_file(parent.dir(), &temp_name);
    }
    result
}

async fn read_transfer_metadata(path: &Path) -> Result<FileTransferSessionMetadata> {
    let path = path.to_path_buf();
    let data = tokio::task::spawn_blocking(move || read_private_file_no_symlink(&path))
        .await
        .context("file transfer metadata read worker failed")??;
    serde_json::from_slice(&data).context("file transfer metadata is invalid")
}

async fn write_transfer_metadata(
    path: &Path,
    metadata: &FileTransferSessionMetadata,
) -> Result<()> {
    let data = serde_json::to_vec(metadata)?;
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || write_private_file_no_replace(&path, &data))
        .await
        .context("file transfer metadata write worker failed")?
}

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

async fn current_transfer_offset(
    path: &Path,
    declared_size: u64,
    expected_identity: &safe_fs::NodeIdentity,
) -> Result<u64> {
    let path = path.to_path_buf();
    let expected_identity = expected_identity.clone();
    tokio::task::spawn_blocking(move || {
        let parent = safe_fs::resolve_parent(&path)?;
        let file = parent.open_child_file_read(false)?;
        safe_fs::ensure_identity(
            &file,
            &expected_identity,
            "file transfer temporary file changed",
        )?;
        let len = file
            .metadata()
            .with_context(|| format!("failed to stat transfer temp file {}", path.display()))?
            .len();
        if len > declared_size {
            anyhow::bail!("file transfer temporary file is larger than declared size");
        }
        Ok(len)
    })
    .await
    .context("file transfer offset worker failed")?
}

async fn write_transfer_chunk(
    path: &Path,
    expected_identity: &safe_fs::NodeIdentity,
    offset: u64,
    data: &[u8],
) -> Result<()> {
    let path = path.to_path_buf();
    let expected_identity = expected_identity.clone();
    let data = data.to_vec();
    tokio::task::spawn_blocking(move || {
        let parent = safe_fs::resolve_parent(&path)?;
        let mut file = parent.open_child_readwrite_nofollow()?;
        safe_fs::ensure_identity(
            &file,
            &expected_identity,
            "file transfer temporary file changed",
        )?;
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(&data)?;
        file.flush()?;
        Ok(())
    })
    .await
    .context("file transfer chunk worker failed")?
}

async fn verify_existing_chunk(
    path: &Path,
    expected_identity: &safe_fs::NodeIdentity,
    offset: u64,
    data: &[u8],
) -> Result<()> {
    let path = path.to_path_buf();
    let expected_identity = expected_identity.clone();
    let data = data.to_vec();
    tokio::task::spawn_blocking(move || {
        let parent = safe_fs::resolve_parent(&path)?;
        let mut file = parent.open_child_file_read(false)?;
        safe_fs::ensure_identity(
            &file,
            &expected_identity,
            "file transfer temporary file changed",
        )?;
        file.seek(SeekFrom::Start(offset))?;
        let mut existing = vec![0_u8; data.len()];
        file.read_exact(&mut existing)?;
        if existing != data {
            anyhow::bail!("duplicate file transfer chunk does not match existing bytes");
        }
        Ok(())
    })
    .await
    .context("file transfer duplicate chunk worker failed")?
}

async fn hash_file(
    path: &Path,
    expected_identity: &safe_fs::NodeIdentity,
    cancel_token: &CommandCancelToken,
    operation_type: &'static str,
) -> Result<String> {
    let path = path.to_path_buf();
    let expected_identity = expected_identity.clone();
    let cancel_token = cancel_token.clone();
    tokio::task::spawn_blocking(move || {
        let parent = safe_fs::resolve_parent(&path)?;
        let mut file = parent.open_child_file_read(false)?;
        safe_fs::ensure_identity(
            &file,
            &expected_identity,
            "file transfer temporary file changed",
        )?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; FILE_TRANSFER_CHUNK_BYTES];
        loop {
            cancel_token.check(operation_type)?;
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Ok(hex::encode(hasher.finalize()))
    })
    .await
    .context("file transfer hash worker failed")?
}

async fn maybe_throttle_complete_on_cancel(
    rate_limit_kbps: u32,
    byte_count: usize,
    cancel_token: &CommandCancelToken,
) {
    if rate_limit_kbps == 0 || byte_count == 0 {
        return;
    }
    let bits = byte_count as u64 * 8;
    let millis = bits.saturating_mul(1000) / (rate_limit_kbps as u64 * 1000);
    if millis > 0 {
        tokio::select! {
            _ = cancel_token.cancelled() => {}
            _ = sleep(Duration::from_millis(millis)) => {}
        }
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::{symlink, MetadataExt, PermissionsExt},
        path::PathBuf,
    };

    use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
    use uuid::Uuid;
    use vpsman_common::{payload_hash, FileExistingPolicy, FileOwnershipPolicy, FilePushChunk};

    use super::*;

    #[test]
    fn resolves_combined_numeric_owner_group_for_file_push() {
        let ownership = resolve_ownership(
            Some("1000:1001"),
            None,
            None,
            None,
            FileOwnershipPolicy::Fail,
        )
        .unwrap();

        assert_eq!(ownership.uid, Some(1000));
        assert_eq!(ownership.gid, Some(1001));
        assert!(matches!(ownership.status, OwnershipPlanStatus::Planned));
    }

    #[test]
    fn rejects_ambiguous_combined_owner_group_for_file_push() {
        let error = resolve_ownership(
            Some("1000:1001"),
            Some("1002"),
            None,
            None,
            FileOwnershipPolicy::Fail,
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("must not be combined with separate owner/group ids"));
    }

    #[tokio::test]
    async fn file_push_commits_requested_mode_and_agent_owner() {
        let root = test_root("push-mode-owner");
        let destination = root.join("app.conf");
        fs::create_dir_all(&root).unwrap();
        let payload = b"config";

        execute_file_push(
            Uuid::new_v4(),
            destination.to_str().unwrap(),
            0o644,
            payload.len() as u64,
            &payload_hash(payload),
            &BASE64_STANDARD.encode(payload),
            FileExistingPolicy::Replace,
            None,
            None,
            None,
            None,
            FileOwnershipPolicy::Fail,
        )
        .await
        .unwrap();

        let metadata = fs::metadata(&destination).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o644);
        assert_eq!(metadata.uid(), current_effective_uid());
        assert_eq!(fs::read(&destination).unwrap(), payload);
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn file_push_rejects_symlinked_parent_component() {
        let root = test_root("push-parent-symlink");
        let real = root.join("real");
        let link = root.join("link");
        fs::create_dir_all(&real).unwrap();
        symlink(&real, &link).unwrap();
        let payload = b"secret";

        let result = execute_file_push(
            Uuid::new_v4(),
            link.join("app.conf").to_str().unwrap(),
            0o644,
            payload.len() as u64,
            &payload_hash(payload),
            &BASE64_STANDARD.encode(payload),
            FileExistingPolicy::Replace,
            None,
            None,
            None,
            None,
            FileOwnershipPolicy::Fail,
        )
        .await;

        assert!(result.unwrap_err().to_string().contains("real directory"));
        assert!(!real.join("app.conf").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn resumable_upload_temp_is_private_and_identity_bound() {
        let root = test_root("transfer-temp-identity");
        fs::create_dir_all(&root).unwrap();
        let destination = root.join("archive.bin");
        let session_id = Uuid::new_v4();
        let payload = b"payload";
        let resume_token_hash = payload_hash(b"resume-token");

        execute_file_transfer_start(
            Uuid::new_v4(),
            session_id,
            destination.to_str().unwrap(),
            0o644,
            payload.len() as u64,
            &payload_hash(payload),
            FILE_TRANSFER_CHUNK_BYTES as u32,
            0,
            FileExistingPolicy::Replace,
            &resume_token_hash,
            CommandCancelToken::default(),
        )
        .await
        .unwrap();

        let paths = transfer_session_paths(destination.to_str().unwrap(), session_id).unwrap();
        assert_eq!(
            fs::metadata(&paths.temp).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(paths.metadata.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        fs::remove_file(&paths.temp).unwrap();
        let symlink_target = root.join("outside.bin");
        fs::write(&symlink_target, b"outside").unwrap();
        symlink(&symlink_target, &paths.temp).unwrap();

        let chunk = FilePushChunk {
            offset: 0,
            size_bytes: payload.len() as u32,
            sha256_hex: payload_hash(payload),
            data_base64: BASE64_STANDARD.encode(payload),
        };
        let error = execute_file_transfer_chunk(
            Uuid::new_v4(),
            session_id,
            0,
            &chunk,
            &resume_token_hash,
            CommandCancelToken::default(),
        )
        .await
        .unwrap_err();

        assert!(error.chain().any(|cause| {
            let message = cause.to_string();
            message.contains("temporary file changed") || message.contains("failed to open file")
        }));
        let _ = fs::remove_file(&paths.metadata);
        let _ = fs::remove_dir_all(&root);
    }

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("vpsman-file-push-{name}-{}", Uuid::new_v4()))
    }
}
