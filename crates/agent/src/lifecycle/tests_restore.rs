use std::os::unix::fs::{symlink, PermissionsExt};

use super::*;

#[test]
fn restore_decoder_accepts_only_the_current_tar_archive() {
    let error = decode_backup_archive(br#"{"not":"a tar archive"}"#).unwrap_err();
    assert!(
        error.to_string().contains("restore tar entry is invalid"),
        "{error:#}"
    );
}

#[test]
fn restore_scope_matches_selected_files_and_directory_descendants() {
    let selected = BackupFileEntry {
        path: "/etc/nginx/conf.d/site.conf".to_string(),
        source: BackupFileSource::SelectedPath,
        tar_path: "vpsman-backup/files/0000.bin".to_string(),
        mode: 0o600,
        size_bytes: 0,
        sha256_hex: payload_hash(&[]),
        mtime_unix: None,
    };
    let adjacent = BackupFileEntry {
        path: "/etc/nginx-old/site.conf".to_string(),
        ..selected.clone()
    };
    let config = BackupFileEntry {
        path: "vpsman:agent_config".to_string(),
        source: BackupFileSource::AgentConfig,
        ..selected.clone()
    };

    assert!(entry_requested(
        &selected,
        &["/etc/nginx".to_string()],
        false
    ));
    assert!(entry_requested(
        &selected,
        std::slice::from_ref(&selected.path),
        false
    ));
    assert!(!entry_requested(
        &adjacent,
        &["/etc/nginx".to_string()],
        false
    ));
    assert!(!entry_requested(&config, &[], false));
    assert!(entry_requested(&config, &[], true));
}

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
        max_timeout_secs: 5,
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
        max_timeout_secs: 5,
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
        max_timeout_secs: 5,
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
        max_timeout_secs: 5,
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
        max_timeout_secs: 5,
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
        max_timeout_secs: 5,
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
        max_timeout_secs: 5,
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

fn append_test_tar_entry(builder: &mut tar::Builder<Vec<u8>>, path: &str, mode: u32, data: &[u8]) {
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
