use std::{fs, os::unix::fs::symlink};

use super::*;

#[test]
fn directory_download_archive_observes_cancel_before_walking_tree() {
    let root = std::env::temp_dir().join(format!("vpsman-file-download-cancel-{}", Uuid::new_v4()));
    let nested = root.join("nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("keep.txt"), "keep").unwrap();
    let cancel_token = CommandCancelToken::default();
    cancel_token.cancel("operator canceled".to_string());

    let result = build_directory_download_artifact(
        &root,
        MAX_DIRECT_FILE_DOWNLOAD_BYTES,
        false,
        &cancel_token,
    );

    assert!(result.is_err());
    assert_eq!(fs::read_to_string(nested.join("keep.txt")).unwrap(), "keep");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn directory_download_manifest_rejects_revisited_symlinked_directory() {
    let root =
        std::env::temp_dir().join(format!("vpsman-file-download-manifest-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    symlink(&root, root.join("loop")).unwrap();

    let error = build_directory_manifest(
        &root,
        MAX_DIRECT_FILE_DOWNLOAD_BYTES,
        true,
        &CommandCancelToken::default(),
    )
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("file download manifest encountered a previously visited directory"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn directory_download_archive_rejects_revisited_symlinked_directory() {
    let root =
        std::env::temp_dir().join(format!("vpsman-file-download-archive-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    symlink(&root, root.join("loop")).unwrap();
    let metadata = fs::metadata(&root).unwrap();
    let mut archive = Vec::new();
    let mut visited_directories = HashSet::new();
    let mut scanned_entries = 0;
    let error = {
        let mut writer = LimitedVecWriter::new(&mut archive, MAX_DIRECT_FILE_DOWNLOAD_BYTES);
        let mut builder = tar::Builder::new(&mut writer);
        builder.follow_symlinks(true);
        append_tar_path_checked(
            &mut builder,
            Path::new("download"),
            &root,
            &metadata,
            true,
            &CommandCancelToken::default(),
            &mut visited_directories,
            &mut scanned_entries,
        )
        .unwrap_err()
    };

    assert!(error
        .to_string()
        .contains("file download archive encountered a previously visited directory"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn directory_download_scan_entry_limit_fails_closed() {
    let mut scanned_entries = MAX_FILE_TREE_SCAN_ENTRIES;

    let error = count_file_download_scan_entry(&mut scanned_entries).unwrap_err();

    assert!(error
        .to_string()
        .contains("file download source exceeds entry scan limit"));
}
