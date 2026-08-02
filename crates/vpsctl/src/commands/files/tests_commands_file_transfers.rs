use super::{
    file_transfer_handoff_path, file_transfer_source_download_path, file_transfer_sources_path,
    file_transfers_path,
};
use uuid::Uuid;

#[test]
fn builds_filtered_file_transfers_path() {
    let path = file_transfers_path(
        500,
        Some("edge a"),
        Some("11111111-2222-4333-8444-555555555555"),
    )
    .unwrap();

    assert_eq!(
        path,
        "/api/v1/file-transfers?limit=200&client_id=edge%20a&session_id=11111111-2222-4333-8444-555555555555"
    );
}

#[test]
fn builds_file_transfer_handoff_path() {
    let session_id = Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap();
    assert_eq!(
        file_transfer_handoff_path("edge a", session_id),
        "/api/v1/file-transfers/edge%20a/11111111-2222-4333-8444-555555555555/handoff"
    );
}

#[test]
fn builds_file_transfer_source_paths() {
    let artifact_id = Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap();
    assert_eq!(
        file_transfer_sources_path(500),
        "/api/v1/file-transfer-sources?limit=200"
    );
    assert_eq!(
        file_transfer_source_download_path(artifact_id),
        "/api/v1/file-transfer-sources/11111111-2222-4333-8444-555555555555/artifact"
    );
}
