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
