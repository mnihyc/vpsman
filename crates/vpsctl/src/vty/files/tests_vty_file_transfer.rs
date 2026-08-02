use super::*;

#[test]
fn parses_vty_resumable_file_upload() {
    let request = parse_vty_file_transfer_upload(&[
        "--source",
        "/tmp/source.bin",
        "--path",
        "/tmp/remote.bin",
        "--mode",
        "0600",
        "--chunk-size-bytes",
        "4096",
        "--rate-limit-kbps",
        "1000",
        "--existing-policy",
        "skip",
        "--multi-target-policy",
        "independent-offsets",
        "--session-id",
        "2e241391-63b4-4deb-b7d2-5df42a55241a",
        "--resume-token",
        "resume-local",
        "id:edge-a",
        "--confirmed",
    ])
    .unwrap();

    assert_eq!(
        request.source,
        FileTransferUploadSource::LocalFile(PathBuf::from("/tmp/source.bin"))
    );
    assert_eq!(request.path, "/tmp/remote.bin");
    assert_eq!(request.mode, 0o600);
    assert_eq!(request.chunk_size_bytes, 4096);
    assert_eq!(request.rate_limit_kbps, 1000);
    assert_eq!(request.existing_policy, FileExistingPolicy::Skip);
    assert_eq!(
        request.multi_target_policy,
        FileTransferMultiTargetPolicy::IndependentOffsets
    );
    assert!(request.clients.is_empty());
    assert_eq!(request.tags, vec!["id:edge-a"]);
    assert!(request.confirmed);
    assert_eq!(request.max_polls, 0);
}

#[test]
fn parses_vty_resumable_file_upload_from_source_artifact() {
    let request = parse_vty_file_transfer_upload(&[
        "--source-artifact-id",
        "11111111-2222-4333-8444-555555555555",
        "--path",
        "/tmp/remote.bin",
        "id:edge-a",
        "--confirmed",
    ])
    .unwrap();

    assert_eq!(
        request.source,
        FileTransferUploadSource::SourceArtifact {
            artifact_id: Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap(),
        }
    );
    assert_eq!(request.path, "/tmp/remote.bin");
    assert_eq!(request.existing_policy, FileExistingPolicy::Replace);
    assert!(request.clients.is_empty());
    assert_eq!(request.tags, vec!["id:edge-a"]);
    assert!(request.confirmed);
    assert_eq!(request.max_polls, 0);
}

#[test]
fn parses_vty_resumable_file_transfer_max_polls_override() {
    let upload = parse_vty_file_transfer_upload(&[
        "--source",
        "/tmp/source.bin",
        "--path",
        "/tmp/remote.bin",
        "--max-polls",
        "7",
        "id:edge-a",
        "--confirmed",
    ])
    .unwrap();
    assert_eq!(upload.max_polls, 7);

    let download = parse_vty_file_transfer_download(&[
        "--path",
        "/tmp/remote.bin",
        "--destination",
        "/tmp/local.bin",
        "--max-polls=0",
        "id:edge-a",
        "--confirmed",
    ])
    .unwrap();
    assert_eq!(download.max_polls, 0);
}

#[test]
fn rejects_vty_resumable_file_upload_without_confirmation() {
    assert!(parse_vty_file_transfer_upload(&[
        "--source",
        "/tmp/source.bin",
        "--path",
        "/tmp/remote.bin",
        "id:edge-a",
    ])
    .is_err());
}

#[test]
fn parses_vty_resumable_file_download() {
    let request = parse_vty_file_transfer_download(&[
        "--path",
        "/tmp/remote.bin",
        "--follow-symlinks",
        "--destination",
        "/tmp/local.bin",
        "--chunk-size-bytes",
        "4096",
        "--rate-limit-kbps",
        "1000",
        "--multi-target-policy",
        "per-target-files",
        "--session-id",
        "2e241391-63b4-4deb-b7d2-5df42a55241a",
        "--resume-token",
        "resume-local",
        "id:edge-a",
        "--confirmed",
    ])
    .unwrap();

    assert_eq!(request.destination, PathBuf::from("/tmp/local.bin"));
    assert_eq!(request.path, "/tmp/remote.bin");
    assert!(request.follow_symlinks);
    assert_eq!(request.chunk_size_bytes, 4096);
    assert_eq!(request.rate_limit_kbps, 1000);
    assert_eq!(
        request.multi_target_policy,
        FileTransferDownloadMultiTargetPolicy::PerTargetFiles
    );
    assert!(request.clients.is_empty());
    assert_eq!(request.tags, vec!["id:edge-a"]);
    assert!(request.confirmed);
}

#[test]
fn parses_vty_resumable_file_download_default_no_follow_symlinks() {
    let request = parse_vty_file_transfer_download(&[
        "--path",
        "/tmp/remote.bin",
        "--destination",
        "/tmp/local.bin",
        "id:edge-a",
        "--confirmed",
    ])
    .unwrap();

    assert!(!request.follow_symlinks);
}

#[test]
fn rejects_vty_resumable_file_download_without_confirmation() {
    assert!(parse_vty_file_transfer_download(&[
        "--path",
        "/tmp/remote.bin",
        "--destination",
        "/tmp/local.bin",
        "id:edge-a",
    ])
    .is_err());
}
