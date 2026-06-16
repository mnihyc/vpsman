use std::path::{Component, Path};

use anyhow::{Context, Result};
use tokio::time::{self, Duration};
use vpsman_common::{
    payload_hash, validate_absolute_file_path, CommandOutput, OutputStream, RestoreRollbackFile,
};

use crate::command_worker::{run_cancelable, CommandCancelToken};

pub(crate) struct RestoreRollbackCommandInput<'a> {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) source_restore_job_id: uuid::Uuid,
    pub(crate) restored_files: &'a [RestoreRollbackFile],
    pub(crate) timeout_secs: u64,
    pub(crate) cancel_token: CommandCancelToken,
}

pub(crate) async fn execute_restore_rollback_command(
    input: RestoreRollbackCommandInput<'_>,
) -> Result<Vec<CommandOutput>> {
    let RestoreRollbackCommandInput {
        job_id,
        source_restore_job_id,
        restored_files,
        timeout_secs,
        cancel_token,
    } = input;
    let deadline = time::Instant::now() + Duration::from_secs(timeout_secs.max(1));
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
    for file in restored_files.iter().rev() {
        cancel_token.check("restore_rollback")?;
        ensure_restore_rollback_deadline(deadline)?;
        let status = rollback_one_successful_restore(job_id, file, deadline, &cancel_token).await?;
        rolled_back.push(status);
        ensure_restore_rollback_deadline(deadline)?;
    }
    rolled_back.reverse();
    let status = serde_json::json!({
        "type": "restore_rollback",
        "source_restore_job_id": source_restore_job_id,
        "rolled_back_count": rolled_back.len(),
        "rolled_back_files": rolled_back,
    });
    Ok(vec![CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: Some(0),
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
    let destination = Path::new(&file.destination_path);
    let metadata = tokio::fs::metadata(destination).await.with_context(|| {
        format!(
            "restore rollback destination missing: {}",
            destination.display()
        )
    })?;
    cancel_token.check("restore_rollback")?;
    ensure_restore_rollback_deadline(deadline)?;
    if metadata.is_dir() {
        anyhow::bail!(
            "restore rollback destination is a directory: {}",
            destination.display()
        );
    }
    if metadata.len() != file.restored_size_bytes {
        anyhow::bail!(
            "restore rollback destination size changed: {}",
            destination.display()
        );
    }
    let data = tokio::fs::read(destination).await.with_context(|| {
        format!(
            "failed to read restore rollback destination {}",
            destination.display()
        )
    })?;
    cancel_token.check("restore_rollback")?;
    ensure_restore_rollback_deadline(deadline)?;
    if payload_hash(&data) != file.restored_sha256_hex {
        anyhow::bail!(
            "restore rollback destination content changed: {}",
            destination.display()
        );
    }
    if let Some(rollback_path) = &file.rollback_path {
        let rollback = Path::new(rollback_path);
        let rollback_metadata = tokio::fs::metadata(rollback).await.with_context(|| {
            format!("restore rollback snapshot missing: {}", rollback.display())
        })?;
        cancel_token.check("restore_rollback")?;
        if rollback_metadata.is_dir() {
            anyhow::bail!(
                "restore rollback snapshot is a directory: {}",
                rollback.display()
            );
        }
        ensure_restore_rollback_deadline(deadline)?;
    }
    Ok(())
}

async fn rollback_one_successful_restore(
    job_id: uuid::Uuid,
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
            let parent = destination
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .context("restore rollback destination has no parent directory")?;
            let file_name = destination
                .file_name()
                .context("restore rollback destination has no file name")?
                .to_string_lossy();
            let temp_path =
                parent.join(format!(".vpsman-restore-rollback-{file_name}-{job_id}.tmp"));
            tokio::fs::copy(rollback, &temp_path)
                .await
                .with_context(|| {
                    format!("failed to copy rollback snapshot {}", rollback.display())
                })?;
            cancel_token.check("restore_rollback")?;
            if let Err(error) = ensure_restore_rollback_deadline(deadline) {
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err(error);
            }
            tokio::fs::set_permissions(&temp_path, rollback_metadata.permissions())
                .await
                .with_context(|| {
                    format!("failed to set rollback temp mode {}", temp_path.display())
                })?;
            cancel_token.check("restore_rollback")?;
            if let Err(error) = ensure_restore_rollback_deadline(deadline) {
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err(error);
            }
            if let Err(error) = tokio::fs::rename(&temp_path, destination).await {
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err(error).with_context(|| {
                    format!("failed to move rollback into {}", destination.display())
                });
            }
            cancel_token.check("restore_rollback")?;
            ensure_restore_rollback_deadline(deadline)?;
            Ok(serde_json::json!({
                "archive_path": file.archive_path,
                "destination_path": file.destination_path,
                "rollback_path": rollback_path,
                "action": "restored_snapshot",
            }))
        }
        None => {
            tokio::fs::remove_file(destination).await.with_context(|| {
                format!("failed to remove restored file {}", destination.display())
            })?;
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
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::{
        execute_restore_rollback_command, rollback_successful_restore, RestoreRollbackCommandInput,
    };
    use crate::command_worker::CommandCancelToken;
    use tokio::time::{self, Duration};
    use vpsman_common::{payload_hash, RestoreRollbackFile};

    #[tokio::test]
    async fn restore_rollback_restores_snapshots_and_removes_created_files() {
        let job_id = uuid::Uuid::new_v4();
        let restore_job_id = uuid::Uuid::new_v4();
        let root = std::env::temp_dir().join(format!("vpsman-restore-operator-rollback-{job_id}"));
        let restored_existing = root.join("existing.txt");
        let restored_created = root.join("created.txt");
        let snapshot = root.join(".vpsman-restore-existing.bak");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(&snapshot, b"old-existing").await.unwrap();
        tokio::fs::set_permissions(&snapshot, std::fs::Permissions::from_mode(0o640))
            .await
            .unwrap();
        tokio::fs::write(&restored_existing, b"new-existing")
            .await
            .unwrap();
        tokio::fs::write(&restored_created, b"new-created")
            .await
            .unwrap();

        let restored_files = vec![
            RestoreRollbackFile {
                archive_path: "/tmp/existing.txt".to_string(),
                destination_path: restored_existing.display().to_string(),
                rollback_path: Some(snapshot.display().to_string()),
                restored_size_bytes: b"new-existing".len() as u64,
                restored_sha256_hex: payload_hash(b"new-existing"),
            },
            RestoreRollbackFile {
                archive_path: "/tmp/created.txt".to_string(),
                destination_path: restored_created.display().to_string(),
                rollback_path: None,
                restored_size_bytes: b"new-created".len() as u64,
                restored_sha256_hex: payload_hash(b"new-created"),
            },
        ];

        let outputs = execute_restore_rollback_command(RestoreRollbackCommandInput {
            job_id,
            source_restore_job_id: restore_job_id,
            restored_files: &restored_files,
            timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap();

        assert_eq!(
            tokio::fs::read(&restored_existing).await.unwrap(),
            b"old-existing"
        );
        assert_eq!(
            tokio::fs::metadata(&restored_existing)
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
        assert!(!restored_created.exists());
        let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
        assert_eq!(status["type"], "restore_rollback");
        assert_eq!(status["source_restore_job_id"], restore_job_id.to_string());
        assert_eq!(status["rolled_back_count"], 2);

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn restore_rollback_rejects_changed_destination_before_mutating() {
        let job_id = uuid::Uuid::new_v4();
        let root = std::env::temp_dir().join(format!("vpsman-restore-changed-rollback-{job_id}"));
        let destination = root.join("existing.txt");
        let snapshot = root.join(".vpsman-restore-existing.bak");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(&snapshot, b"old-existing").await.unwrap();
        tokio::fs::write(&destination, b"operator-changed")
            .await
            .unwrap();
        let restored_files = vec![RestoreRollbackFile {
            archive_path: "/tmp/existing.txt".to_string(),
            destination_path: destination.display().to_string(),
            rollback_path: Some(snapshot.display().to_string()),
            restored_size_bytes: b"new-existing".len() as u64,
            restored_sha256_hex: payload_hash(b"new-existing"),
        }];

        let error = execute_restore_rollback_command(RestoreRollbackCommandInput {
            job_id,
            source_restore_job_id: uuid::Uuid::new_v4(),
            restored_files: &restored_files,
            timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap_err();

        assert!(error.to_string().contains("destination size changed"));
        assert_eq!(
            tokio::fs::read(&destination).await.unwrap(),
            b"operator-changed"
        );

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn restore_rollback_deadline_expires_without_dropping_mutation_future() {
        let job_id = uuid::Uuid::new_v4();
        let root = std::env::temp_dir().join(format!("vpsman-restore-timeout-rollback-{job_id}"));
        let destination = root.join("created.txt");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(&destination, b"new-created")
            .await
            .unwrap();
        let restored_files = vec![RestoreRollbackFile {
            archive_path: "/tmp/created.txt".to_string(),
            destination_path: destination.display().to_string(),
            rollback_path: None,
            restored_size_bytes: b"new-created".len() as u64,
            restored_sha256_hex: payload_hash(b"new-created"),
        }];

        let error = rollback_successful_restore(
            job_id,
            uuid::Uuid::new_v4(),
            &restored_files,
            time::Instant::now() - Duration::from_millis(1),
            CommandCancelToken::default(),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("restore rollback timed out"));
        assert_eq!(tokio::fs::read(&destination).await.unwrap(), b"new-created");

        let _ = tokio::fs::remove_dir_all(root).await;
    }
}
