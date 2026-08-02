use super::{
    build_file_transfer_sessions, file_transfer_handoff_download_path,
    file_transfer_handoff_object_key, merge_persisted_file_transfer_session,
    FileTransferStatusOutput,
};
use crate::model_file_transfer::FileTransferSessionView;
use uuid::Uuid;

#[test]
fn builds_latest_upload_session_with_start_metadata() {
    let session_id = Uuid::new_v4();
    let start_job = Uuid::new_v4();
    let chunk_job = Uuid::new_v4();
    let commit_job = Uuid::new_v4();
    let outputs = vec![
        status_output(
            commit_job,
            "edge-a",
            0,
            "300",
            "file_transfer_commit",
            "file_transfer_commit",
            serde_json::json!({
                "type": "file_transfer_commit",
                "session_id": session_id,
                "path": "/opt/app.bin",
                "next_offset": 12,
                "size_bytes": 12,
                "extra": {"sha256_hex": "b".repeat(64), "mode": 420}
            }),
        ),
        status_output(
            chunk_job,
            "edge-a",
            0,
            "200",
            "file_transfer_chunk",
            "file_transfer_chunk_ack",
            serde_json::json!({
                "type": "file_transfer_chunk_ack",
                "session_id": session_id,
                "path": "/opt/app.bin",
                "next_offset": 12,
                "size_bytes": 12,
                "extra": {"ack_offset": 0, "ack_size_bytes": 12}
            }),
        ),
        status_output(
            start_job,
            "edge-a",
            0,
            "100",
            "file_transfer_start",
            "file_transfer_start",
            serde_json::json!({
                "type": "file_transfer_start",
                "session_id": session_id,
                "path": "/opt/app.bin",
                "next_offset": 0,
                "size_bytes": 12,
                "extra": {"resumed": false, "chunk_size_bytes": 65536, "rate_limit_kbps": 1000}
            }),
        ),
    ];

    let sessions = build_file_transfer_sessions(outputs, 20, None);

    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.session_id, session_id);
    assert_eq!(session.client_id, "edge-a");
    assert_eq!(session.direction, "upload");
    assert_eq!(session.status, "completed");
    assert_eq!(session.path, "/opt/app.bin");
    assert_eq!(session.progress_bytes, 12);
    assert_eq!(session.progress_ratio, Some(1.0));
    assert_eq!(session.chunk_size_bytes, Some(65536));
    assert_eq!(session.last_chunk_size_bytes, Some(12));
    assert_eq!(session.rate_limit_kbps, Some(1000));
    assert_eq!(session.last_job_id, commit_job);
    assert_eq!(session.last_event, "file_transfer_commit");
}

#[test]
fn delayed_lower_progress_event_cannot_regress_transfer_lifecycle() {
    let session_id = Uuid::new_v4();
    let delayed_job = Uuid::new_v4();
    let advanced_job = Uuid::new_v4();
    let commit_job = Uuid::new_v4();
    let advanced_hash = "a".repeat(64);
    let outputs = vec![
        status_output(
            delayed_job,
            "edge-a",
            0,
            "400",
            "file_transfer_chunk",
            "file_transfer_chunk_ack",
            serde_json::json!({
                "type": "file_transfer_chunk_ack",
                "session_id": session_id,
                "path": "/opt/app.bin",
                "next_offset": 4,
                "size_bytes": 8,
                "extra": {
                    "ack_size_bytes": 4,
                    "chunk_sha256_hex": "d".repeat(64)
                }
            }),
        ),
        status_output(
            advanced_job,
            "edge-a",
            0,
            "300",
            "file_transfer_chunk",
            "file_transfer_chunk_ack",
            serde_json::json!({
                "type": "file_transfer_chunk_ack",
                "session_id": session_id,
                "path": "/opt/app.bin",
                "next_offset": 8,
                "size_bytes": 8,
                "extra": {
                    "ack_size_bytes": 4,
                    "chunk_sha256_hex": advanced_hash.clone()
                }
            }),
        ),
        status_output(
            commit_job,
            "edge-a",
            0,
            "200",
            "file_transfer_commit",
            "file_transfer_commit",
            serde_json::json!({
                "type": "file_transfer_commit",
                "session_id": session_id,
                "path": "/opt/app.bin",
                "next_offset": 8,
                "size_bytes": 8,
                "extra": {"sha256_hex": "b".repeat(64)}
            }),
        ),
    ];

    let sessions = build_file_transfer_sessions(outputs, 20, None);

    let session = &sessions[0];
    assert_eq!(session.status, "completed");
    assert_eq!(session.progress_bytes, 8);
    assert_eq!(session.progress_ratio, Some(1.0));
    assert_eq!(session.last_chunk_size_bytes, Some(4));
    assert_eq!(
        session.last_chunk_sha256_hex.as_deref(),
        Some(advanced_hash.as_str())
    );
    assert_eq!(session.last_job_id, commit_job);
    assert_eq!(session.last_event, "file_transfer_commit");
}

#[test]
fn persisted_merge_keeps_newer_metadata_and_terminal_lifecycle() {
    let session_id = Uuid::new_v4();
    let existing_job = Uuid::new_v4();
    let incoming_job = Uuid::new_v4();
    let mut existing = persisted_session(
        session_id,
        "completed",
        5,
        100,
        "new-hash",
        "2026-07-01T00:00:00Z",
        existing_job,
    );
    existing.handoff_available = true;
    existing.handoff_object_key = Some("old-key".to_string());
    existing.handoff_download_path = Some("old-path".to_string());
    let mut incoming = persisted_session(
        session_id,
        "aborted",
        10,
        80,
        "stale-hash",
        "2026-06-30T23:59:59Z",
        incoming_job,
    );
    incoming.last_chunk_size_bytes = Some(5);
    incoming.last_chunk_sha256_hex = Some("latest-range-hash".to_string());

    let merged = merge_persisted_file_transfer_session(existing, incoming).unwrap();

    assert_eq!(merged.status, "completed");
    assert_eq!(merged.last_job_id, existing_job);
    assert_eq!(merged.progress_bytes, 10);
    assert_eq!(merged.size_bytes, Some(100));
    assert_eq!(merged.sha256_hex.as_deref(), Some("new-hash"));
    assert_eq!(merged.last_chunk_size_bytes, Some(5));
    assert_eq!(
        merged.last_chunk_sha256_hex.as_deref(),
        Some("latest-range-hash")
    );
    assert!(merged.handoff_available);
}

#[test]
fn persisted_merge_accepts_newer_terminal_state_and_clears_handoff() {
    let session_id = Uuid::new_v4();
    let existing = persisted_session(
        session_id,
        "completed",
        10,
        100,
        "completed-hash",
        "2026-07-01T00:00:00Z",
        Uuid::new_v4(),
    );
    let incoming_job = Uuid::new_v4();
    let incoming = persisted_session(
        session_id,
        "aborted",
        10,
        100,
        "aborted-hash",
        "2026-07-01T00:00:00.1Z",
        incoming_job,
    );

    let merged = merge_persisted_file_transfer_session(existing, incoming).unwrap();

    assert_eq!(merged.status, "aborted");
    assert_eq!(merged.last_job_id, incoming_job);
    assert!(!merged.handoff_available);
    assert!(merged.handoff_object_key.is_none());
    assert!(merged.handoff_download_path.is_none());
}

#[test]
fn persisted_merge_rejects_invalid_source_timestamps() {
    let session_id = Uuid::new_v4();
    let existing = persisted_session(
        session_id,
        "started",
        0,
        100,
        "hash",
        "2026-07-01T00:00:00Z",
        Uuid::new_v4(),
    );
    let incoming = persisted_session(
        session_id,
        "transferring",
        1,
        100,
        "hash",
        "not-a-timestamp",
        Uuid::new_v4(),
    );

    assert!(merge_persisted_file_transfer_session(existing, incoming).is_err());
}

#[test]
fn filters_download_sessions_and_marks_final_chunk_complete() {
    let wanted = Uuid::new_v4();
    let other = Uuid::new_v4();
    let outputs = vec![
        status_output(
            Uuid::new_v4(),
            "edge-b",
            1,
            "300",
            "file_transfer_download_chunk",
            "file_transfer_download_chunk",
            serde_json::json!({
                "type": "file_transfer_download_chunk",
                "session_id": wanted,
                "path": "/var/log/app.log",
                "next_offset": 100,
                "size_bytes": 100,
                "extra": {"offset": 64, "chunk_size_bytes": 36, "chunk_sha256_hex": "a".repeat(64), "complete": true, "file_sha256_hex": "c".repeat(64)}
            }),
        ),
        status_output(
            Uuid::new_v4(),
            "edge-b",
            0,
            "200",
            "file_transfer_download_start",
            "file_transfer_download_start",
            serde_json::json!({
                "type": "file_transfer_download_start",
                "session_id": wanted,
                "path": "/var/log/app.log",
                "next_offset": 0,
                "size_bytes": 100,
                "extra": {"resumed": true, "sha256_hex": "c".repeat(64), "chunk_size_bytes": 64, "rate_limit_kbps": 0}
            }),
        ),
        status_output(
            Uuid::new_v4(),
            "edge-c",
            0,
            "100",
            "file_transfer_download_start",
            "file_transfer_download_start",
            serde_json::json!({
                "type": "file_transfer_download_start",
                "session_id": other,
                "path": "/tmp/other",
                "next_offset": 0,
                "size_bytes": 1,
                "extra": {"chunk_size_bytes": 1}
            }),
        ),
    ];

    let sessions = build_file_transfer_sessions(outputs, 20, Some(wanted));

    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.session_id, wanted);
    assert_eq!(session.direction, "download");
    assert_eq!(session.status, "completed");
    assert_eq!(session.chunk_size_bytes, Some(64));
    assert_eq!(session.last_chunk_size_bytes, Some(36));
    assert_eq!(session.resumed, Some(true));
    let expected_file_hash = "c".repeat(64);
    let expected_chunk_hash = "a".repeat(64);
    assert_eq!(
        session.sha256_hex.as_deref(),
        Some(expected_file_hash.as_str())
    );
    assert_eq!(
        session.last_chunk_sha256_hex.as_deref(),
        Some(expected_chunk_hash.as_str())
    );
    assert!(session.handoff_available);
    assert_eq!(
        session.handoff_object_key.as_deref(),
        Some(file_transfer_handoff_object_key("edge-b", wanted, &expected_file_hash).as_str())
    );
    assert_eq!(
        session.handoff_download_path.as_deref(),
        Some(file_transfer_handoff_download_path("edge-b", wanted).as_str())
    );
}

#[test]
fn handoff_download_path_percent_encodes_client_id() {
    let session_id = Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap();

    assert_eq!(
        file_transfer_handoff_download_path("edge a/ignored", session_id),
        "/api/v1/file-transfers/edge%20a%2Fignored/11111111-2222-4333-8444-555555555555/handoff/artifact"
    );
}

fn status_output(
    job_id: Uuid,
    client_id: &str,
    seq: i32,
    created_at: &str,
    command_type: &str,
    expected_type: &str,
    value: serde_json::Value,
) -> FileTransferStatusOutput {
    assert_eq!(
        value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap(),
        expected_type
    );
    FileTransferStatusOutput {
        job_id,
        client_id: client_id.to_string(),
        seq,
        data: serde_json::to_vec(&value).unwrap(),
        created_at: created_at.to_string(),
        command_type: command_type.to_string(),
    }
}

fn persisted_session(
    session_id: Uuid,
    status: &str,
    progress_bytes: i64,
    size_bytes: i64,
    sha256_hex: &str,
    observed_at: &str,
    last_job_id: Uuid,
) -> FileTransferSessionView {
    FileTransferSessionView {
        session_id,
        client_id: "edge-a".to_string(),
        direction: "download".to_string(),
        status: status.to_string(),
        path: "/var/lib/archive.bin".to_string(),
        size_bytes: Some(size_bytes),
        progress_bytes,
        progress_ratio: Some((progress_bytes as f64 / size_bytes as f64).clamp(0.0, 1.0)),
        sha256_hex: Some(sha256_hex.to_string()),
        chunk_size_bytes: Some(64),
        last_chunk_size_bytes: None,
        last_chunk_sha256_hex: None,
        rate_limit_kbps: Some(1000),
        resumed: Some(false),
        last_event: match status {
            "completed" => "file_transfer_download_chunk",
            "aborted" => "file_transfer_abort",
            "transferring" => "file_transfer_download_chunk",
            _ => "file_transfer_download_start",
        }
        .to_string(),
        last_job_id,
        last_command_type: match status {
            "aborted" => "file_transfer_abort",
            "transferring" | "completed" => "file_transfer_download_chunk",
            _ => "file_transfer_download_start",
        }
        .to_string(),
        last_seq: 0,
        observed_at: observed_at.to_string(),
        handoff_available: status == "completed",
        handoff_evidence_status: "not_completed".to_string(),
        handoff_unavailable_reason: None,
        handoff_object_key: None,
        handoff_download_path: None,
    }
}
