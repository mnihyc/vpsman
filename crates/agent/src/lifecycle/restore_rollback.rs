use std::{
    io::{Read, Write},
    os::unix::fs::PermissionsExt,
    path::{Component, Path},
};

use anyhow::{Context, Result};
use tokio::time::{self, Duration};
use vpsman_common::{
    validate_absolute_file_path, CommandOutput, OutputStream, RestoreRollbackFile,
};

use crate::{
    command_worker::{run_cancelable, CommandCancelToken},
    safe_file, safe_fs,
};

pub(crate) struct RestoreRollbackCommandInput<'a> {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) source_restore_job_id: uuid::Uuid,
    pub(crate) restored_files: &'a [RestoreRollbackFile],
    pub(crate) max_timeout_secs: u64,
    pub(crate) cancel_token: CommandCancelToken,
}

pub(crate) async fn execute_restore_rollback_command(
    input: RestoreRollbackCommandInput<'_>,
) -> Result<Vec<CommandOutput>> {
    let RestoreRollbackCommandInput {
        job_id,
        source_restore_job_id,
        restored_files,
        max_timeout_secs,
        cancel_token,
    } = input;
    let deadline = time::Instant::now() + Duration::from_secs(max_timeout_secs.max(1));
    run_cancelable(
        "restore_rollback",
        cancel_token.clone(),
        rollback_successful_restore(
            job_id,
            source_restore_job_id,
            restored_files,
            deadline,
            cancel_token,
        ),
    )
    .await
}

async fn rollback_successful_restore(
    job_id: uuid::Uuid,
    source_restore_job_id: uuid::Uuid,
    restored_files: &[RestoreRollbackFile],
    deadline: time::Instant,
    cancel_token: CommandCancelToken,
) -> Result<Vec<CommandOutput>> {
    cancel_token.check("restore_rollback")?;
    validate_restore_rollback_files(restored_files, deadline, &cancel_token).await?;
    let mut rolled_back = Vec::with_capacity(restored_files.len());
    let mut failures = Vec::new();
    for file in restored_files.iter().rev() {
        cancel_token.check("restore_rollback")?;
        ensure_restore_rollback_deadline(deadline)?;
        match rollback_one_successful_restore(job_id, file, deadline, &cancel_token).await {
            Ok(status) => rolled_back.push(status),
            Err(error) => failures.push(serde_json::json!({
                "archive_path": file.archive_path,
                "destination_path": file.destination_path,
                "rollback_path": file.rollback_path,
                "error": error.to_string(),
            })),
        }
        ensure_restore_rollback_deadline(deadline)?;
    }
    rolled_back.reverse();
    failures.reverse();
    let exit_code = if failures.is_empty() { 0 } else { 1 };
    let status = serde_json::json!({
        "type": "restore_rollback",
        "source_restore_job_id": source_restore_job_id,
        "status": if failures.is_empty() { "completed" } else { "partial_failure" },
        "rolled_back_count": rolled_back.len(),
        "rolled_back_files": rolled_back,
        "failed_count": failures.len(),
        "failures": failures,
    });
    Ok(vec![CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: Some(exit_code),
        done: true,
    }])
}

async fn validate_restore_rollback_files(
    restored_files: &[RestoreRollbackFile],
    deadline: time::Instant,
    cancel_token: &CommandCancelToken,
) -> Result<()> {
    if restored_files.is_empty() {
        anyhow::bail!("restore rollback files are required");
    }
    for file in restored_files {
        cancel_token.check("restore_rollback")?;
        ensure_restore_rollback_deadline(deadline)?;
        validate_safe_absolute_path(&file.destination_path)?;
        if let Some(rollback_path) = &file.rollback_path {
            validate_safe_absolute_path(rollback_path)?;
        }
        validate_current_restored_file(file, deadline, cancel_token).await?;
        ensure_restore_rollback_deadline(deadline)?;
    }
    Ok(())
}

async fn validate_current_restored_file(
    file: &RestoreRollbackFile,
    deadline: time::Instant,
    cancel_token: &CommandCancelToken,
) -> Result<()> {
    cancel_token.check("restore_rollback")?;
    ensure_restore_rollback_deadline(deadline)?;
    let restored_file = file.clone();
    tokio::task::spawn_blocking(move || {
        let destination = Path::new(&restored_file.destination_path);
        let destination_parent = safe_fs::resolve_parent(destination)?;
        validate_destination_at_commit(&destination_parent, &restored_file)?;
        if let Some(rollback_path) = &restored_file.rollback_path {
            let rollback = Path::new(rollback_path);
            let rollback_parent = safe_fs::resolve_parent(rollback)?;
            let snapshot = rollback_parent
                .open_child_file_read(false)
                .with_context(|| {
                    format!("restore rollback snapshot missing: {}", rollback.display())
                })?;
            anyhow::ensure!(
                snapshot.metadata()?.is_file(),
                "restore rollback snapshot is not a regular file: {}",
                rollback.display()
            );
        }
        Ok::<(), anyhow::Error>(())
    })
    .await
    .context("restore rollback validation worker failed")??;
    cancel_token.check("restore_rollback")?;
    ensure_restore_rollback_deadline(deadline)?;
    Ok(())
}

async fn rollback_one_successful_restore(
    _job_id: uuid::Uuid,
    file: &RestoreRollbackFile,
    deadline: time::Instant,
    cancel_token: &CommandCancelToken,
) -> Result<serde_json::Value> {
    cancel_token.check("restore_rollback")?;
    ensure_restore_rollback_deadline(deadline)?;
    let destination = Path::new(&file.destination_path);
    match &file.rollback_path {
        Some(rollback_path) => {
            let rollback = Path::new(rollback_path);
            let rollback_metadata = tokio::fs::metadata(rollback).await.with_context(|| {
                format!("restore rollback snapshot missing: {}", rollback.display())
            })?;
            cancel_token.check("restore_rollback")?;
            ensure_restore_rollback_deadline(deadline)?;
            cancel_token.check("restore_rollback")?;
            ensure_restore_rollback_deadline(deadline)?;
            let rollback_path = rollback.to_path_buf();
            let rollback_path_display = rollback_path.display().to_string();
            let destination_path = destination.to_path_buf();
            let mode = rollback_metadata.permissions().mode() & 0o777;
            let restored_file = file.clone();
            tokio::task::spawn_blocking(move || {
                copy_snapshot_into_destination(
                    &rollback_path,
                    &destination_path,
                    mode,
                    &restored_file,
                )
            })
            .await
            .context("restore rollback file worker failed")??;
            cancel_token.check("restore_rollback")?;
            ensure_restore_rollback_deadline(deadline)?;
            Ok(serde_json::json!({
                "archive_path": file.archive_path,
                "destination_path": file.destination_path,
                "rollback_path": rollback_path_display,
                "action": "restored_snapshot",
            }))
        }
        None => {
            let restored_file = file.clone();
            tokio::task::spawn_blocking(move || remove_restored_destination(&restored_file))
                .await
                .context("restore rollback remove worker failed")??;
            cancel_token.check("restore_rollback")?;
            ensure_restore_rollback_deadline(deadline)?;
            Ok(serde_json::json!({
                "archive_path": file.archive_path,
                "destination_path": file.destination_path,
                "rollback_path": null,
                "action": "removed_restored_file",
            }))
        }
    }
}

fn copy_snapshot_into_destination(
    snapshot: &Path,
    destination: &Path,
    mode: u32,
    restored_file: &RestoreRollbackFile,
) -> Result<()> {
    let snapshot_parent = safe_fs::resolve_parent(snapshot)?;
    let mut source = snapshot_parent.open_child_file_read(false)?;
    let snapshot_identity_before = safe_file::FileIdentity::from_metadata(&source.metadata()?);
    let destination_parent = safe_fs::resolve_parent(destination)?;
    let (mut temp_file, temp_name) = safe_fs::create_private_temp_file(
        destination_parent.dir(),
        destination_parent.name(),
        "restore-rollback",
    )?;
    let result = (|| -> Result<()> {
        copy_open_file(&mut source, &mut temp_file)?;
        let snapshot_identity_after = safe_file::FileIdentity::from_metadata(&source.metadata()?);
        anyhow::ensure!(
            snapshot_identity_before == snapshot_identity_after,
            "restore rollback snapshot changed while it was being copied"
        );
        safe_fs::fchmod_file(&temp_file, mode)?;
        temp_file.sync_all().with_context(|| {
            format!("failed to sync rollback temp for {}", destination.display())
        })?;
        validate_destination_at_commit(&destination_parent, restored_file)?;
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

fn remove_restored_destination(restored_file: &RestoreRollbackFile) -> Result<()> {
    let destination = Path::new(&restored_file.destination_path);
    let parent = safe_fs::resolve_parent(destination)?;
    validate_destination_at_commit(&parent, restored_file)?;
    safe_fs::remove_child_file(parent.dir(), parent.name())
        .with_context(|| format!("failed to remove restored file {}", destination.display()))?;
    safe_fs::sync_dir_best_effort(parent.dir());
    Ok(())
}

fn validate_destination_at_commit(
    parent: &safe_fs::SafeParent,
    restored_file: &RestoreRollbackFile,
) -> Result<()> {
    let stat_before = parent
        .child_stat_nofollow()?
        .context("restore rollback destination disappeared before commit")?;
    anyhow::ensure!(
        stat_before.is_file(),
        "restore rollback destination is no longer a regular file"
    );
    let file = parent.open_child_file_read(false)?;
    let metadata_before = file.metadata()?;
    anyhow::ensure!(
        metadata_before.len() == restored_file.restored_size_bytes,
        "restore rollback destination size changed before commit"
    );
    let file_identity_before = safe_file::FileIdentity::from_metadata(&metadata_before);
    let current_hash = safe_file::hash_opened_file_bounded(
        file.try_clone()?,
        restored_file.restored_size_bytes,
        "restore rollback destination exceeds expected size",
    )?;
    let metadata_after = file.metadata()?;
    let file_identity_after = safe_file::FileIdentity::from_metadata(&metadata_after);
    let stat_after = parent
        .child_stat_nofollow()?
        .context("restore rollback destination disappeared during commit verification")?;
    anyhow::ensure!(
        stat_before.identity == stat_after.identity && file_identity_before == file_identity_after,
        "restore rollback destination changed during commit verification"
    );
    anyhow::ensure!(
        current_hash.eq_ignore_ascii_case(&restored_file.restored_sha256_hex),
        "restore rollback destination content changed before commit"
    );
    Ok(())
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

fn ensure_restore_rollback_deadline(deadline: time::Instant) -> Result<()> {
    if time::Instant::now() >= deadline {
        anyhow::bail!("restore rollback timed out");
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests_restore_rollback.rs"]
mod tests;
