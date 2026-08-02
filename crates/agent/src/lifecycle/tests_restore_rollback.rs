use std::os::unix::fs::{symlink, PermissionsExt};

use super::{
    copy_snapshot_into_destination, execute_restore_rollback_command, remove_restored_destination,
    rollback_successful_restore, RestoreRollbackCommandInput,
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
        max_timeout_secs: 5,
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
        max_timeout_secs: 5,
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

#[test]
fn restore_rollback_revalidates_destination_inside_commit() {
    let job_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("vpsman-restore-commit-race-{job_id}"));
    let destination = root.join("existing.txt");
    let created = root.join("created.txt");
    let snapshot = root.join(".vpsman-restore-existing.bak");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(&snapshot, b"previous").unwrap();
    std::fs::write(&destination, b"changed!").unwrap();
    std::fs::write(&created, b"changed!").unwrap();
    let expected_existing = RestoreRollbackFile {
        archive_path: "/tmp/existing.txt".to_string(),
        destination_path: destination.display().to_string(),
        rollback_path: Some(snapshot.display().to_string()),
        restored_size_bytes: b"restored".len() as u64,
        restored_sha256_hex: payload_hash(b"restored"),
    };
    let expected_created = RestoreRollbackFile {
        archive_path: "/tmp/created.txt".to_string(),
        destination_path: created.display().to_string(),
        rollback_path: None,
        restored_size_bytes: b"restored".len() as u64,
        restored_sha256_hex: payload_hash(b"restored"),
    };

    let replace_error =
        copy_snapshot_into_destination(&snapshot, &destination, 0o600, &expected_existing)
            .unwrap_err();
    assert!(replace_error
        .to_string()
        .contains("content changed before commit"));
    let remove_error = remove_restored_destination(&expected_created).unwrap_err();
    assert!(remove_error
        .to_string()
        .contains("content changed before commit"));
    assert_eq!(std::fs::read(&destination).unwrap(), b"changed!");
    assert_eq!(std::fs::read(&created).unwrap(), b"changed!");
    assert!(std::fs::read_dir(&root).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".vpsman-restore-rollback-")
    }));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn restore_rollback_rejects_symlinked_destination_parent() {
    let job_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("vpsman-restore-rollback-symlink-{job_id}"));
    let real = root.join("real");
    let link = root.join("link");
    let snapshot = root.join(".vpsman-restore-existing.bak");
    tokio::fs::create_dir_all(&real).await.unwrap();
    symlink(&real, &link).unwrap();
    tokio::fs::write(&snapshot, b"old-existing").await.unwrap();
    tokio::fs::write(real.join("existing.txt"), b"new-existing")
        .await
        .unwrap();
    let restored_files = vec![RestoreRollbackFile {
        archive_path: "/tmp/existing.txt".to_string(),
        destination_path: link.join("existing.txt").display().to_string(),
        rollback_path: Some(snapshot.display().to_string()),
        restored_size_bytes: b"new-existing".len() as u64,
        restored_sha256_hex: payload_hash(b"new-existing"),
    }];

    let error = execute_restore_rollback_command(RestoreRollbackCommandInput {
        job_id,
        source_restore_job_id: uuid::Uuid::new_v4(),
        restored_files: &restored_files,
        max_timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap_err();

    assert!(error.to_string().contains("real directory"));
    assert_eq!(
        tokio::fs::read(real.join("existing.txt")).await.unwrap(),
        b"new-existing"
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
