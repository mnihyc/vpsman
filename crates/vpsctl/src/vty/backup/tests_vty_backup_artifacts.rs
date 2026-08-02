use super::{
    parse_vty_backup_artifact_handoff, parse_vty_backup_artifact_record,
    parse_vty_backup_artifact_upload, parse_vty_backup_artifact_upload_chunked,
};
use uuid::Uuid;

#[test]
fn parses_vty_backup_artifact_record() {
    let backup_id = Uuid::new_v4().to_string();
    let request = parse_vty_backup_artifact_record(&[
        &backup_id,
        "--object-key",
        "backups/client-a/artifact.tar",
        "--sha256-hex",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "--size-bytes=4096",
        "--confirmed",
    ])
    .unwrap();

    assert_eq!(request.backup_request_id.to_string(), backup_id);
    assert_eq!(request.object_key, "backups/client-a/artifact.tar");
    assert_eq!(
        request.sha256_hex,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert_eq!(request.size_bytes, 4096);
    assert!(request.confirmed);
}

#[test]
fn rejects_vty_backup_artifact_without_safe_metadata() {
    let backup_id = Uuid::new_v4().to_string();
    assert!(parse_vty_backup_artifact_record(&[
        &backup_id,
        "--object-key",
        "../artifact",
        "--sha256-hex",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "--size-bytes",
        "4096",
        "--confirmed",
    ])
    .is_err());
    assert!(parse_vty_backup_artifact_record(&[
        &backup_id,
        "--object-key",
        "backups/client-a/artifact.tar",
        "--sha256-hex",
        "not-a-hash",
        "--size-bytes",
        "4096",
        "--confirmed",
    ])
    .is_err());
    assert!(parse_vty_backup_artifact_record(&[
        &backup_id,
        "--object-key",
        "backups/client-a/artifact.tar",
        "--sha256-hex",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "--size-bytes",
        "4096",
    ])
    .is_err());
}

#[test]
fn parses_vty_backup_artifact_upload() {
    let backup_id = Uuid::new_v4().to_string();
    let request = parse_vty_backup_artifact_upload(&[
        &backup_id,
        "--object-key",
        "backups/client-a/artifact.json",
        "--artifact-file",
        "/tmp/artifact.json",
        "--confirmed",
    ])
    .unwrap();

    assert_eq!(request.backup_request_id.to_string(), backup_id);
    assert_eq!(request.object_key, "backups/client-a/artifact.json");
    assert_eq!(
        request.artifact_file,
        std::path::PathBuf::from("/tmp/artifact.json")
    );
    assert!(request.confirmed);
}

#[test]
fn rejects_vty_backup_artifact_upload_without_confirmation_or_safe_key() {
    let backup_id = Uuid::new_v4().to_string();
    assert!(parse_vty_backup_artifact_upload(&[
        &backup_id,
        "--object-key",
        "../artifact",
        "--artifact-file",
        "/tmp/artifact.json",
        "--confirmed",
    ])
    .is_err());
    assert!(parse_vty_backup_artifact_upload(&[
        &backup_id,
        "--object-key",
        "backups/client-a/artifact.json",
        "--artifact-file",
        "/tmp/artifact.json",
    ])
    .is_err());
}

#[test]
fn parses_vty_backup_artifact_upload_chunked() {
    let backup_id = Uuid::new_v4().to_string();
    let request = parse_vty_backup_artifact_upload_chunked(&[
        &backup_id,
        "--object-key=backups/client-a/artifact.json",
        "--artifact-file=/tmp/artifact.json",
        "--chunk-size-bytes",
        "65536",
        "--confirmed",
    ])
    .unwrap();

    assert_eq!(request.backup_request_id.to_string(), backup_id);
    assert_eq!(request.object_key, "backups/client-a/artifact.json");
    assert_eq!(
        request.artifact_file,
        std::path::PathBuf::from("/tmp/artifact.json")
    );
    assert_eq!(request.chunk_size_bytes, 65_536);
    assert!(request.confirmed);
}

#[test]
fn rejects_vty_backup_artifact_upload_chunked_without_confirmation_or_safe_key() {
    let backup_id = Uuid::new_v4().to_string();
    assert!(parse_vty_backup_artifact_upload_chunked(&[
        &backup_id,
        "--object-key",
        "../artifact",
        "--artifact-file",
        "/tmp/artifact.json",
        "--confirmed",
    ])
    .is_err());
    assert!(parse_vty_backup_artifact_upload_chunked(&[
        &backup_id,
        "--object-key",
        "backups/client-a/artifact.json",
        "--artifact-file",
        "/tmp/artifact.json",
        "--chunk-size-bytes",
        "0",
        "--confirmed",
    ])
    .is_err());
    assert!(parse_vty_backup_artifact_upload_chunked(&[
        &backup_id,
        "--object-key",
        "backups/client-a/artifact.json",
        "--artifact-file",
        "/tmp/artifact.json",
    ])
    .is_err());
}

#[test]
fn parses_vty_backup_artifact_handoff() {
    let backup_id = Uuid::new_v4().to_string();
    let job_id = Uuid::new_v4().to_string();
    let request =
        parse_vty_backup_artifact_handoff(&[&backup_id, "--job-id", &job_id, "--confirmed"])
            .unwrap();

    assert_eq!(request.backup_request_id.to_string(), backup_id);
    assert_eq!(request.job_id.unwrap().to_string(), job_id);
    assert!(request.confirmed);
}

#[test]
fn rejects_vty_backup_artifact_handoff_without_confirmation() {
    let backup_id = Uuid::new_v4().to_string();
    assert!(parse_vty_backup_artifact_handoff(&[&backup_id]).is_err());
}
