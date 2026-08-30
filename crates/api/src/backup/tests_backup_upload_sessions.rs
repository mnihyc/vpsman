use super::*;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

#[tokio::test]
async fn create_session_uses_private_staging_files() {
    let root =
        std::env::temp_dir().join(format!("vpsman-backup-upload-private-{}", Uuid::new_v4()));
    let sessions = BackupArtifactUploadSessions::new(root.clone());
    let view = sessions
        .create(
            Uuid::new_v4(),
            "edge-a".to_string(),
            BackupArtifactUploadSessionCreateRequest {
                object_key: "backups/edge-a/private.tar".to_string(),
                expected_sha256_hex: "a".repeat(64),
                expected_size_bytes: 8,
                confirmed: true,
            },
        )
        .await
        .unwrap();

    assert_eq!(mode(&root), 0o700);
    assert_eq!(mode(&sessions.staging_path(view.upload_id)), 0o600);
    assert_eq!(mode(&sessions.manifest_path(view.upload_id)), 0o600);
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn cleanup_removes_expired_orphaned_and_temp_upload_files() {
    let root =
        std::env::temp_dir().join(format!("vpsman-backup-upload-cleanup-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&root).await.unwrap();
    let sessions = BackupArtifactUploadSessions::new(root.clone());
    let now = unix_now().saturating_add(BACKUP_ARTIFACT_UPLOAD_SESSION_TTL_SECS + 1);

    let live_id = Uuid::new_v4();
    let live_part = sessions.staging_path(live_id);
    let live_manifest = sessions.manifest_path(live_id);
    tokio::fs::write(&live_part, b"live").await.unwrap();
    let live = BackupArtifactUploadSession {
        upload_id: live_id,
        backup_request_id: Uuid::new_v4(),
        client_id: "edge-a".to_string(),
        object_key: "backups/edge-a/live.tar".to_string(),
        expected_sha256_hex: "a".repeat(64),
        expected_size_bytes: 4,
        received_bytes: 0,
        chunk_count: 0,
        created_unix: 1,
        updated_unix: 1,
        expires_unix: now.saturating_add(BACKUP_ARTIFACT_UPLOAD_SESSION_TTL_SECS),
        staging_path: live_part.clone(),
    };
    tokio::fs::write(&live_manifest, serde_json::to_vec(&live).unwrap())
        .await
        .unwrap();

    let expired_id = Uuid::new_v4();
    let expired_part = sessions.staging_path(expired_id);
    let expired_manifest = sessions.manifest_path(expired_id);
    tokio::fs::write(&expired_part, b"expired").await.unwrap();
    let expired = BackupArtifactUploadSession {
        upload_id: expired_id,
        backup_request_id: Uuid::new_v4(),
        client_id: "edge-a".to_string(),
        object_key: "backups/edge-a/expired.tar".to_string(),
        expected_sha256_hex: "b".repeat(64),
        expected_size_bytes: 7,
        received_bytes: 0,
        chunk_count: 0,
        created_unix: 1,
        updated_unix: 1,
        expires_unix: 1,
        staging_path: expired_part.clone(),
    };
    tokio::fs::write(&expired_manifest, serde_json::to_vec(&expired).unwrap())
        .await
        .unwrap();

    let orphan_part = sessions.staging_path(Uuid::new_v4());
    let temp_manifest = root.join(format!("{}.json.tmp-{}", Uuid::new_v4(), Uuid::new_v4()));
    tokio::fs::write(&orphan_part, b"orphan").await.unwrap();
    tokio::fs::write(&temp_manifest, b"partial").await.unwrap();

    let busy_id = Uuid::new_v4();
    let busy_part = sessions.staging_path(busy_id);
    let busy_manifest = sessions.manifest_path(busy_id);
    tokio::fs::write(&busy_part, b"busy").await.unwrap();
    let busy = BackupArtifactUploadSession {
        upload_id: busy_id,
        backup_request_id: Uuid::new_v4(),
        client_id: "edge-a".to_string(),
        object_key: "backups/edge-a/busy.tar".to_string(),
        expected_sha256_hex: "c".repeat(64),
        expected_size_bytes: 4,
        received_bytes: 0,
        chunk_count: 0,
        created_unix: 1,
        updated_unix: 1,
        expires_unix: 1,
        staging_path: busy_part.clone(),
    };
    tokio::fs::write(&busy_manifest, serde_json::to_vec(&busy).unwrap())
        .await
        .unwrap();
    let busy_owner = sessions.claim_staging_file(busy_id).await.unwrap();

    sessions.cleanup_expired_at(now).await;

    assert!(live_part.exists());
    assert!(live_manifest.exists());
    assert!(!expired_part.exists());
    assert!(!expired_manifest.exists());
    assert!(!orphan_part.exists());
    assert!(!temp_manifest.exists());
    assert!(busy_part.exists());
    assert!(busy_manifest.exists());

    drop(busy_owner);
    sessions.cleanup_expired_at(now).await;
    assert!(!busy_part.exists());
    assert!(!busy_manifest.exists());

    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_mutations_wait_for_the_exact_staging_owner() {
    let root = std::env::temp_dir().join(format!(
        "vpsman-backup-upload-exact-owner-{}",
        Uuid::new_v4()
    ));
    let sessions = BackupArtifactUploadSessions::new(root.clone());
    let backup_request_id = Uuid::new_v4();
    let view = sessions
        .create(
            backup_request_id,
            "edge-a".to_string(),
            BackupArtifactUploadSessionCreateRequest {
                object_key: "backups/edge-a/owned.tar".to_string(),
                expected_sha256_hex: "a".repeat(64),
                expected_size_bytes: 4,
                confirmed: true,
            },
        )
        .await
        .unwrap();

    let owner = sessions.claim_staging_file(view.upload_id).await.unwrap();
    let writer_sessions = sessions.clone();
    let mut writer = tokio::spawn(async move {
        writer_sessions
            .write_chunk(
                backup_request_id,
                view.upload_id,
                BackupArtifactUploadChunkRequest {
                    offset_bytes: 0,
                    data_base64: BASE64.encode(b"data"),
                },
            )
            .await
    });
    assert!(tokio::time::timeout(Duration::from_millis(50), &mut writer)
        .await
        .is_err());
    drop(owner);
    let written = tokio::time::timeout(Duration::from_secs(2), writer)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(written.received_bytes, 4);
    assert_eq!(written.chunk_count, 1);

    let owner = sessions
        .claim_staging_file(written.upload_id)
        .await
        .unwrap();
    let abort_sessions = sessions.clone();
    let mut abort = tokio::spawn(async move {
        abort_sessions
            .abort(backup_request_id, written.upload_id, true)
            .await
    });
    assert!(tokio::time::timeout(Duration::from_millis(50), &mut abort)
        .await
        .is_err());
    drop(owner);
    let aborted = tokio::time::timeout(Duration::from_secs(2), abort)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(aborted.status, "aborted");
    assert!(!sessions.staging_path(aborted.upload_id).exists());
    assert!(!sessions.manifest_path(aborted.upload_id).exists());

    let _ = tokio::fs::remove_dir_all(root).await;
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}
