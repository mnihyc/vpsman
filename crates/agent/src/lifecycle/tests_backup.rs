use std::os::unix::fs::symlink;

use super::*;
use vpsman_common::AgentBackupConfig;

#[tokio::test]
async fn creates_plain_backup_tar_artifact() {
    let job_id = uuid::Uuid::new_v4();
    let dir = std::env::temp_dir().join(format!("vpsman-agent-backup-{job_id}"));
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let selected_path = dir.join("selected.txt");
    let config_path = dir.join("agent.toml");
    tokio::fs::write(&selected_path, b"selected secret contents")
        .await
        .unwrap();
    tokio::fs::write(&config_path, b"noise_client_private_key_hex = \"secret\"")
        .await
        .unwrap();
    let config = AgentConfig {
        client_id: "client-a".to_string(),
        backup: AgentBackupConfig {
            max_uncompressed_bytes: 8192,
            max_archive_bytes: 16 * 1024,
        },
        ..AgentConfig::default()
    };

    let paths = vec![selected_path.to_string_lossy().to_string()];
    let outputs = execute_backup_command(BackupCommandInput {
        job_id,
        config: &config,
        config_path: &config_path,
        paths: &paths,
        include_config: true,
        follow_symlinks: false,
        missing_path_policy: BackupMissingPathPolicy::Fail,
        output_tx: None,
        max_timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap();
    let artifact_bytes = outputs
        .iter()
        .filter(|output| output.stream == OutputStream::Stdout)
        .flat_map(|output| output.data.clone())
        .collect::<Vec<_>>();
    let archive = manifest_from_tar(&artifact_bytes);
    assert_eq!(archive.format, BACKUP_ARCHIVE_FORMAT);
    assert_eq!(archive.client_id, "client-a");
    assert_eq!(archive.files.len(), 2);
    assert!(archive
        .files
        .iter()
        .any(|file| file.path == selected_path.to_string_lossy().as_ref()
            && file.sha256_hex == payload_hash(b"selected secret contents")));
    let status = outputs.iter().find(|output| output.done).unwrap();
    let status: serde_json::Value = serde_json::from_slice(&status.data).unwrap();
    assert_eq!(status["type"], "backup");
    assert_eq!(status["file_count"], 2);
    assert_eq!(status["artifact_sha256_hex"], payload_hash(&artifact_bytes));

    let _ = tokio::fs::remove_dir_all(dir).await;
}

#[tokio::test]
async fn backup_rejects_symlink_paths_by_default() {
    let job_id = uuid::Uuid::new_v4();
    let dir = std::env::temp_dir().join(format!("vpsman-agent-backup-symlink-{job_id}"));
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let target_path = dir.join("target.txt");
    let symlink_path = dir.join("linked.txt");
    let config_path = dir.join("agent.toml");
    tokio::fs::write(&target_path, b"target contents")
        .await
        .unwrap();
    tokio::fs::write(&config_path, b"client_id = \"client-a\"")
        .await
        .unwrap();
    symlink(&target_path, &symlink_path).unwrap();
    let config = AgentConfig {
        client_id: "client-a".to_string(),
        backup: AgentBackupConfig {
            max_uncompressed_bytes: 8192,
            max_archive_bytes: 16 * 1024,
        },
        ..AgentConfig::default()
    };
    let paths = vec![symlink_path.to_string_lossy().to_string()];

    let error = execute_backup_command(BackupCommandInput {
        job_id,
        config: &config,
        config_path: &config_path,
        paths: &paths,
        include_config: false,
        follow_symlinks: false,
        missing_path_policy: BackupMissingPathPolicy::Fail,
        output_tx: None,
        max_timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap_err();
    assert!(error
        .chain()
        .any(|cause| cause.to_string().contains("backup path is a symlink")));

    let outputs = execute_backup_command(BackupCommandInput {
        job_id,
        config: &config,
        config_path: &config_path,
        paths: &paths,
        include_config: false,
        follow_symlinks: true,
        missing_path_policy: BackupMissingPathPolicy::Fail,
        output_tx: None,
        max_timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap();
    let artifact_bytes = outputs
        .iter()
        .filter(|output| output.stream == OutputStream::Stdout)
        .flat_map(|output| output.data.clone())
        .collect::<Vec<_>>();
    let archive = manifest_from_tar(&artifact_bytes);
    assert_eq!(archive.files.len(), 1);
    assert_eq!(archive.files[0].path, symlink_path.to_string_lossy());
    assert_eq!(
        archive.files[0].sha256_hex,
        payload_hash(b"target contents")
    );

    let _ = tokio::fs::remove_dir_all(dir).await;
}

#[tokio::test]
async fn backup_recurses_directories_and_reports_tolerated_omissions() {
    let job_id = uuid::Uuid::new_v4();
    let dir = std::env::temp_dir().join(format!("vpsman-agent-backup-tree-{job_id}"));
    let root = dir.join("config");
    let nested = root.join("service");
    tokio::fs::create_dir_all(&nested).await.unwrap();
    let root_file = root.join("root.conf");
    let nested_file = nested.join("service.conf");
    let nested_link = nested.join("linked.conf");
    let missing = dir.join("optional-missing");
    tokio::fs::write(&root_file, b"root=true\n").await.unwrap();
    tokio::fs::write(&nested_file, b"enabled=true\n")
        .await
        .unwrap();
    symlink(&root_file, &nested_link).unwrap();

    let config = AgentConfig {
        client_id: "client-tree".to_string(),
        backup: AgentBackupConfig {
            max_uncompressed_bytes: 8192,
            max_archive_bytes: 32 * 1024,
        },
        ..AgentConfig::default()
    };
    let paths = vec![
        root.to_string_lossy().to_string(),
        missing.to_string_lossy().to_string(),
    ];
    let outputs = execute_backup_command(BackupCommandInput {
        job_id,
        config: &config,
        config_path: &root_file,
        paths: &paths,
        include_config: false,
        follow_symlinks: false,
        missing_path_policy: BackupMissingPathPolicy::Skip,
        output_tx: None,
        max_timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap();
    let artifact_bytes = outputs
        .iter()
        .filter(|output| output.stream == OutputStream::Stdout)
        .flat_map(|output| output.data.clone())
        .collect::<Vec<_>>();
    let archive = manifest_from_tar(&artifact_bytes);
    let archived_paths = archive
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(archive.files.len(), 2);
    assert!(archived_paths.contains(&root_file.to_string_lossy().as_ref()));
    assert!(archived_paths.contains(&nested_file.to_string_lossy().as_ref()));
    assert!(!archived_paths.contains(&nested_link.to_string_lossy().as_ref()));

    let status = outputs.iter().find(|output| output.done).unwrap();
    let status: serde_json::Value = serde_json::from_slice(&status.data).unwrap();
    assert_eq!(status["missing_path_policy"], "skip");
    assert_eq!(status["directory_count"], 2);
    assert_eq!(status["skipped_path_count"], 2);
    assert_eq!(status["skipped_paths"][0]["reason"], "symlink_not_followed");
    assert_eq!(status["skipped_paths"][1]["reason"], "missing");

    let error = execute_backup_command(BackupCommandInput {
        job_id: uuid::Uuid::new_v4(),
        config: &config,
        config_path: &root_file,
        paths: &paths,
        include_config: false,
        follow_symlinks: false,
        missing_path_policy: BackupMissingPathPolicy::Fail,
        output_tx: None,
        max_timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap_err();
    assert!(error
        .chain()
        .any(|cause| cause.to_string().contains("failed to stat backup path")));

    let _ = tokio::fs::remove_dir_all(dir).await;
}

#[tokio::test]
async fn streams_backup_artifact_through_payload_sink() {
    let job_id = uuid::Uuid::new_v4();
    let dir = std::env::temp_dir().join(format!("vpsman-agent-backup-stream-{job_id}"));
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let selected_path = dir.join("selected.bin");
    let selected_data = (0..8192)
        .map(|value| (value % 251) as u8)
        .collect::<Vec<_>>();
    let config_path = dir.join("agent.toml");
    tokio::fs::write(&selected_path, &selected_data)
        .await
        .unwrap();
    tokio::fs::write(&config_path, b"noise_client_private_key_hex = \"secret\"")
        .await
        .unwrap();
    let config = AgentConfig {
        client_id: "client-stream".to_string(),
        backup: AgentBackupConfig {
            max_uncompressed_bytes: 64 * 1024,
            max_archive_bytes: 128 * 1024,
        },
        ..AgentConfig::default()
    };
    let (tx, mut rx) = mpsc::channel(64);

    let paths = vec![selected_path.to_string_lossy().to_string()];
    let outputs = execute_backup_command(BackupCommandInput {
        job_id,
        config: &config,
        config_path: &config_path,
        paths: &paths,
        include_config: true,
        follow_symlinks: false,
        missing_path_policy: BackupMissingPathPolicy::Fail,
        output_tx: Some(tx),
        max_timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap();

    let mut artifact_bytes = Vec::new();
    while let Some(output) = rx.recv().await {
        assert_eq!(output.stream, OutputStream::Stdout);
        assert!(!output.done);
        artifact_bytes.extend_from_slice(&output.data);
    }
    assert!(!artifact_bytes.is_empty());
    assert!(outputs
        .iter()
        .all(|output| output.stream == OutputStream::Status));
    let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
    assert_eq!(status["type"], "backup");
    assert_eq!(status["streamed"], true);
    assert_eq!(status["artifact_sha256_hex"], payload_hash(&artifact_bytes));
    assert!(status["chunk_count"].as_u64().unwrap() >= 1);

    let archive = manifest_from_tar(&artifact_bytes);
    assert_eq!(archive.client_id, "client-stream");
    assert_eq!(archive.files.len(), 2);

    let _ = tokio::fs::remove_dir_all(dir).await;
}

#[tokio::test]
async fn backup_rejects_unsafe_scope_and_size_limits() {
    let job_id = uuid::Uuid::new_v4();
    let dir = std::env::temp_dir().join(format!("vpsman-agent-backup-reject-{job_id}"));
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let file_path = dir.join("selected.txt");
    tokio::fs::write(&file_path, b"contents").await.unwrap();

    let paths = vec![file_path.to_string_lossy().to_string()];
    let config = AgentConfig {
        backup: AgentBackupConfig {
            max_uncompressed_bytes: 4,
            max_archive_bytes: 1024,
        },
        ..AgentConfig::default()
    };
    let relative_paths = vec!["relative".to_string()];
    let relative = execute_backup_command(BackupCommandInput {
        job_id,
        config: &config,
        config_path: &file_path,
        paths: &relative_paths,
        include_config: false,
        follow_symlinks: false,
        missing_path_policy: BackupMissingPathPolicy::Fail,
        output_tx: None,
        max_timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap_err();
    assert!(relative.to_string().contains("file path must be absolute"));

    let too_large = execute_backup_command(BackupCommandInput {
        job_id,
        config: &config,
        config_path: &file_path,
        paths: &paths,
        include_config: false,
        follow_symlinks: false,
        missing_path_policy: BackupMissingPathPolicy::Fail,
        output_tx: None,
        max_timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap_err();
    assert!(too_large.to_string().contains("uncompressed payload limit"));

    let _ = tokio::fs::remove_dir_all(dir).await;
}

#[tokio::test]
async fn backup_rejects_archive_overhead_above_archive_limit() {
    let job_id = uuid::Uuid::new_v4();
    let dir = std::env::temp_dir().join(format!("vpsman-agent-backup-archive-{job_id}"));
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let file_path = dir.join("selected.txt");
    tokio::fs::write(&file_path, b"small").await.unwrap();

    let paths = vec![file_path.to_string_lossy().to_string()];
    let config = AgentConfig {
        backup: AgentBackupConfig {
            max_uncompressed_bytes: 512,
            max_archive_bytes: 512,
        },
        ..AgentConfig::default()
    };

    let archive_too_large = execute_backup_command(BackupCommandInput {
        job_id,
        config: &config,
        config_path: &file_path,
        paths: &paths,
        include_config: false,
        follow_symlinks: false,
        missing_path_policy: BackupMissingPathPolicy::Fail,
        output_tx: None,
        max_timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap_err();
    assert!(archive_too_large
        .chain()
        .any(|cause| cause.to_string().contains("archive byte limit")));

    let _ = tokio::fs::remove_dir_all(dir).await;
}

fn manifest_from_tar(bytes: &[u8]) -> BackupArchive {
    let mut tar_archive = tar::Archive::new(std::io::Cursor::new(bytes));
    for entry in tar_archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry.path().unwrap().to_string_lossy() == BACKUP_ARCHIVE_MANIFEST_PATH {
            let mut data = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut data).unwrap();
            return serde_json::from_slice(&data).unwrap();
        }
    }
    panic!("backup manifest missing")
}
