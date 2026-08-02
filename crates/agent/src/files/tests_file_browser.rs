use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
};

use uuid::Uuid;

use super::*;

#[test]
fn resolves_combined_numeric_owner_group_for_chown() {
    let ownership = resolve_owner_group(
        Some("1000:1001"),
        None,
        None,
        None,
        FileOwnershipPolicy::Fail,
    )
    .unwrap();

    assert_eq!(ownership.uid, Some(1000));
    assert_eq!(ownership.gid, Some(1001));
    assert_eq!(ownership.status, OwnershipResolutionStatus::Planned);
}

#[test]
fn rejects_ambiguous_combined_owner_group_for_chown() {
    let error = resolve_owner_group(
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

    let read = execute_file_read_text(Uuid::new_v4(), file.to_str().unwrap(), 1024, false)
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
    assert_eq!(write_status["metadata"]["name"], "hello.txt");
    assert_eq!(write_status["metadata"]["path"], file.to_str().unwrap());
    assert_eq!(write_status["metadata"]["size_bytes"], 7);
    assert_eq!(write_status["metadata"]["mode"], 0o640);
    assert_eq!(fs::read_to_string(&file).unwrap(), "updated");
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn list_dir_reports_scan_cap_without_exact_total() {
    let root = test_root("list-scan-cap");
    fs::create_dir_all(&root).unwrap();
    for index in 0..=MAX_FILE_TREE_SCAN_ENTRIES {
        fs::write(root.join(format!("entry-{index:05}.txt")), "x").unwrap();
    }

    let list = execute_file_list_dir(Uuid::new_v4(), root.to_str().unwrap(), 0, 50, false)
        .await
        .unwrap();
    let status: Value = serde_json::from_slice(&list[0].data).unwrap();

    assert_eq!(status["entries"].as_array().unwrap().len(), 50);
    assert_eq!(status["total_entries"], Value::Null);
    assert_eq!(status["scan_cap_entries"], MAX_FILE_TREE_SCAN_ENTRIES);
    assert_eq!(status["scanned_entries"], MAX_FILE_TREE_SCAN_ENTRIES);
    assert_eq!(
        status["visible_entries_scanned"],
        MAX_FILE_TREE_SCAN_ENTRIES
    );
    assert_eq!(status["truncated_by_scan_cap"], true);
    assert_eq!(status["truncated"], true);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn atomic_text_write_revalidates_destination_at_commit() {
    let root = test_root("commit-bound-stale-write");
    fs::create_dir_all(&root).unwrap();
    let file = root.join("service.conf");
    fs::write(&file, b"service-update").unwrap();
    let desired = b"operator-update";
    let stale_hash = payload_hash(b"opened-content");

    let error = atomic_write_blocking(
        &file,
        0o644,
        desired,
        true,
        Some(&stale_hash),
        &payload_hash(desired),
        FileActionPolicy::Fail,
    )
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("changed before the write committed"));
    assert_eq!(fs::read(&file).unwrap(), b"service-update");

    let outcome = atomic_write_blocking(
        &file,
        0o644,
        desired,
        true,
        Some(&stale_hash),
        &payload_hash(desired),
        FileActionPolicy::Ignore,
    )
    .unwrap();
    assert!(matches!(outcome, AtomicWriteOutcome::SkippedStale { .. }));
    assert_eq!(fs::read(&file).unwrap(), b"service-update");
    assert!(fs::read_dir(&root).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".vpsman-edit-")));

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
async fn text_replacement_requires_the_opened_file_hash() {
    let root = test_root("write-requires-revision");
    fs::create_dir_all(&root).unwrap();
    let file = root.join("service.conf");
    fs::write(&file, "current").unwrap();
    let result = execute_file_write_text(
        Uuid::new_v4(),
        file.to_str().unwrap(),
        0o644,
        7,
        &payload_hash(b"updated"),
        &base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"updated"),
        None,
        false,
        FileActionPolicy::Fail,
    )
    .await;

    assert!(result
        .unwrap_err()
        .to_string()
        .contains("expected hash is required"));
    assert_eq!(fs::read_to_string(&file).unwrap(), "current");
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
async fn expected_hash_oversized_current_file_fails_closed() {
    let root = test_root("write-oversized-current-fail");
    fs::create_dir_all(&root).unwrap();
    let file = root.join("hello.txt");
    fs::write(&file, vec![b'x'; (MAX_FILE_READ_BYTES + 1) as usize]).unwrap();
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

    let error = result.unwrap_err();
    assert!(error_chain_contains(
        &error,
        "failed to hash current file before writing"
    ));
    assert_eq!(fs::metadata(&file).unwrap().len(), MAX_FILE_READ_BYTES + 1);
    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn expected_hash_oversized_current_file_can_skip_with_ignore_policy() {
    let root = test_root("write-oversized-current-ignore");
    fs::create_dir_all(&root).unwrap();
    let file = root.join("hello.txt");
    fs::write(&file, vec![b'x'; (MAX_FILE_READ_BYTES + 1) as usize]).unwrap();
    let output = execute_file_write_text(
        Uuid::new_v4(),
        file.to_str().unwrap(),
        0o644,
        7,
        &payload_hash(b"updated"),
        &base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"updated"),
        Some(payload_hash(b"old").as_str()),
        false,
        FileActionPolicy::Ignore,
    )
    .await
    .unwrap();
    let status: Value = serde_json::from_slice(&output[0].data).unwrap();

    assert_eq!(status["status"], "skipped");
    assert_eq!(status["reason"], "verification_failed");
    assert!(status["error"]
        .as_str()
        .unwrap()
        .contains("current file exceeds text verification limit"));
    assert_eq!(fs::metadata(&file).unwrap().len(), MAX_FILE_READ_BYTES + 1);
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
    let error = result.unwrap_err();
    assert!(error_chain_contains(
        &error,
        "failed to hash current file before writing"
    ));
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
        false,
        FileActionPolicy::Fail,
        CommandCancelToken::default(),
        "file_copy",
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
        false,
        FileActionPolicy::Ensure,
        CommandCancelToken::default(),
        "file_copy",
    )
    .await
    .unwrap();
    let status: Value = serde_json::from_slice(&output[0].data).unwrap();
    assert_eq!(status["status"], "unchanged");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn recursive_copy_rejects_revisited_symlinked_directory() {
    let root = test_root("copy-directory-cycle");
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(&source).unwrap();
    symlink(&source, source.join("loop")).unwrap();

    let error = copy_path(
        &source,
        &destination,
        true,
        false,
        &CommandCancelToken::default(),
        "file_copy",
        true,
    )
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("recursive copy encountered a previously visited directory"));
    assert!(!destination.join("loop").exists());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn recursive_copy_rejects_real_destination_inside_symlinked_source() {
    let root = test_root("copy-symlinked-source-self");
    let source = root.join("source");
    let source_link = root.join("source-link");
    let destination_parent = source.join("nested");
    let destination = destination_parent.join("copy");
    fs::create_dir_all(&destination_parent).unwrap();
    symlink(&source, &source_link).unwrap();

    let error = copy_path(
        &source_link,
        &destination,
        true,
        false,
        &CommandCancelToken::default(),
        "file_copy",
        true,
    )
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("refusing to copy a directory into itself"));
    assert!(!destination.exists());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn recursive_copy_rejects_destination_reached_through_source_symlink() {
    let root = test_root("copy-source-symlink-to-destination");
    let source = root.join("source");
    let destination = root.join("destination");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination).unwrap();
    symlink(&destination, source.join("external")).unwrap();

    let error = copy_path(
        &source,
        &destination,
        true,
        true,
        &CommandCancelToken::default(),
        "file_copy",
        true,
    )
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("refusing to copy a directory into itself"));
    assert!(!destination.join("external").exists());
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

#[tokio::test]
async fn recursive_delete_observes_cancel_before_removing_tree() {
    let root = test_root("delete-cancel");
    let nested = root.join("nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("keep.txt"), "keep").unwrap();
    let cancel_token = CommandCancelToken::default();
    cancel_token.cancel("operator canceled".to_string());

    let result = remove_path(&root, true, cancel_token, "file_delete").await;

    assert!(result.is_err());
    assert_eq!(fs::read_to_string(nested.join("keep.txt")).unwrap(), "keep");
    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn mkdir_rejects_symlinked_parent_component() {
    let root = test_root("mkdir-parent-symlink");
    let real = root.join("real");
    let link = root.join("link");
    fs::create_dir_all(&real).unwrap();
    symlink(&real, &link).unwrap();

    let result = execute_file_mkdir(
        Uuid::new_v4(),
        link.join("child").to_str().unwrap(),
        0o755,
        false,
        FileActionPolicy::Fail,
    )
    .await;

    assert!(result.unwrap_err().to_string().contains("real directory"));
    assert!(!real.join("child").exists());
    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn mkdir_returns_resulting_entry_metadata() {
    let root = test_root("mkdir-result-metadata");
    fs::create_dir_all(&root).unwrap();
    let directory = root.join("created");

    let output = execute_file_mkdir(
        Uuid::new_v4(),
        directory.to_str().unwrap(),
        0o750,
        false,
        FileActionPolicy::Fail,
    )
    .await
    .unwrap();

    let status: Value = serde_json::from_slice(&output[0].data).unwrap();
    assert_eq!(status["status"], "created");
    assert_eq!(status["metadata"]["name"], "created");
    assert_eq!(status["metadata"]["path"], directory.to_str().unwrap());
    assert_eq!(status["metadata"]["file_type"], "directory");
    assert_eq!(status["metadata"]["mode"], 0o750);
    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn rename_rejects_symlinked_destination_parent() {
    let root = test_root("rename-parent-symlink");
    let real = root.join("real");
    let link = root.join("link");
    let source = root.join("source.txt");
    fs::create_dir_all(&real).unwrap();
    fs::write(&source, "source").unwrap();
    symlink(&real, &link).unwrap();

    let result = execute_file_rename(
        Uuid::new_v4(),
        source.to_str().unwrap(),
        link.join("moved.txt").to_str().unwrap(),
        false,
        FileActionPolicy::Fail,
    )
    .await;

    assert!(result.unwrap_err().to_string().contains("real directory"));
    assert_eq!(fs::read_to_string(&source).unwrap(), "source");
    assert!(!real.join("moved.txt").exists());
    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn chmod_applies_numeric_mode_through_descriptor() {
    let root = test_root("chmod-mode");
    let file = root.join("app.conf");
    fs::create_dir_all(&root).unwrap();
    fs::write(&file, "config").unwrap();

    execute_file_chmod(
        Uuid::new_v4(),
        file.to_str().unwrap(),
        0o755,
        false,
        false,
        FileActionPolicy::Fail,
        CommandCancelToken::default(),
        "file_chmod",
    )
    .await
    .unwrap();

    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o755
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn recursive_chmod_rejects_revisited_symlinked_directory() {
    let root = test_root("chmod-directory-cycle");
    let source = root.join("source");
    fs::create_dir_all(&source).unwrap();
    symlink(&source, source.join("loop")).unwrap();

    let error = chmod_path(
        &source,
        0o755,
        true,
        true,
        &CommandCancelToken::default(),
        "file_chmod",
    )
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("recursive chmod encountered a previously visited directory"));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn tar_archive_observes_cancel_before_walking_tree() {
    let root = test_root("archive-cancel");
    let nested = root.join("nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("keep.txt"), "keep").unwrap();
    let cancel_token = CommandCancelToken::default();
    cancel_token.cancel("operator canceled".to_string());

    let result = build_tar_archive(
        &root,
        MAX_FILE_ARCHIVE_BYTES,
        false,
        &cancel_token,
        "file_archive_tar",
    );

    assert!(result.is_err());
    assert_eq!(fs::read_to_string(nested.join("keep.txt")).unwrap(), "keep");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn tar_archive_estimate_rejects_revisited_symlinked_directory() {
    let root = test_root("archive-estimate-directory-cycle");
    fs::create_dir_all(&root).unwrap();
    symlink(&root, root.join("loop")).unwrap();

    let error = build_tar_archive(
        &root,
        MAX_FILE_ARCHIVE_BYTES,
        true,
        &CommandCancelToken::default(),
        "file_archive_tar",
    )
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("archive size estimation encountered a previously visited directory"));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn tar_archive_walk_rejects_revisited_symlinked_directory() {
    let root = test_root("archive-walk-directory-cycle");
    fs::create_dir_all(&root).unwrap();
    symlink(&root, root.join("loop")).unwrap();
    let metadata = fs::metadata(&root).unwrap();
    let mut archive = Vec::new();
    let mut visited_directories = HashSet::new();
    let mut scanned_entries = 0;
    let error = {
        let mut writer = LimitedVecWriter::new(&mut archive, MAX_FILE_ARCHIVE_BYTES);
        let mut builder = tar::Builder::new(&mut writer);
        builder.follow_symlinks(true);
        append_tar_path_checked(
            &mut builder,
            Path::new("archive"),
            &root,
            &metadata,
            true,
            &CommandCancelToken::default(),
            "file_archive_tar",
            &mut visited_directories,
            &mut scanned_entries,
        )
        .unwrap_err()
    };

    assert!(error
        .to_string()
        .contains("archive traversal encountered a previously visited directory"));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn archive_scan_entry_limit_fails_closed() {
    let mut scanned_entries = MAX_FILE_TREE_SCAN_ENTRIES;

    let error = count_archive_scan_entry(&mut scanned_entries).unwrap_err();

    assert!(error
        .to_string()
        .contains("archive source exceeds entry scan limit"));
}

fn test_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("vpsman-file-browser-{name}-{}", Uuid::new_v4()))
}

fn error_chain_contains(error: &anyhow::Error, needle: &str) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string().contains(needle))
}
