use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn builds_stable_local_download_temp_path() {
    let session_id = Uuid::parse_str("2e241391-63b4-4deb-b7d2-5df42a55241a").unwrap();
    let path = local_download_temp_path(&PathBuf::from("/tmp/result.bin"), session_id).unwrap();
    assert_eq!(
        path,
        PathBuf::from("/tmp/.vpsman-download-result.bin-2e241391-63b4-4deb-b7d2-5df42a55241a.part")
    );
}

#[test]
fn resumable_download_temp_file_is_private() {
    let root = std::env::temp_dir().join(format!("vpsman-download-private-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let destination = root.join("result.bin");
    let session_id = Uuid::new_v4();
    let temp_path = local_download_temp_path(&destination, session_id).unwrap();
    fs::write(&temp_path, b"partial").unwrap();
    fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o644)).unwrap();

    let (_file, opened_path, offset) =
        open_download_temp_file(&destination, session_id, 128, true).unwrap();

    assert_eq!(opened_path, temp_path);
    assert_eq!(offset, b"partial".len() as u64);
    assert_eq!(mode(&opened_path), 0o600);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn percent_encodes_client_id_segments() {
    assert_eq!(percent_encode_path_segment("edge/a b"), "edge%2Fa%20b");
}

#[test]
fn builds_per_target_download_destinations() {
    let destinations = download_destination_specs(
        &PathBuf::from("/tmp/downloads"),
        "/var/log/app.log",
        &["edge/sfo 01".to_string(), "fra-02".to_string()],
        FileTransferDownloadMultiTargetPolicy::PerTargetFiles,
    )
    .unwrap();
    assert_eq!(
        destinations,
        vec![
            (
                "edge/sfo 01".to_string(),
                PathBuf::from("/tmp/downloads/edge_sfo_01-app.log")
            ),
            (
                "fra-02".to_string(),
                PathBuf::from("/tmp/downloads/fra-02-app.log")
            ),
        ]
    );
}

#[test]
fn rejects_multi_target_download_without_explicit_policy() {
    assert!(download_destination_specs(
        &PathBuf::from("/tmp/result.bin"),
        "/tmp/result.bin",
        &["edge-a".to_string(), "edge-b".to_string()],
        FileTransferDownloadMultiTargetPolicy::SingleTarget,
    )
    .is_err());
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}
