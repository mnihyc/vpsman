use super::{
    assess_handoff_chunk_evidence, assess_loaded_handoff_chunk_evidence,
    file_transfer_handoff_download_path, merge_persisted_file_transfer_session,
    FileTransferDownloadHandoffChunk, LoadedHandoffChunkEvidence,
    HANDOFF_EVIDENCE_RETAINED_OUTPUTS_AVAILABLE, HANDOFF_EVIDENCE_RETAINED_OUTPUTS_CONFLICT,
    HANDOFF_EVIDENCE_RETAINED_OUTPUTS_INCOMPLETE, HANDOFF_EVIDENCE_RETAINED_OUTPUTS_PRUNED,
};
use crate::{model::JobOutputView, model_file_transfer::FileTransferSessionView};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

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
fn handoff_download_path_percent_encodes_client_id() {
    let session_id = Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap();

    assert_eq!(
        file_transfer_handoff_download_path("edge a/ignored", session_id),
        "/api/v1/file-transfers/edge%20a%2Fignored/11111111-2222-4333-8444-555555555555/handoff/artifact"
    );
}

#[test]
fn handoff_evidence_query_and_partial_index_use_normalized_session_identity() {
    let repository_source = include_str!("repository_file_transfers.rs");
    assert!(repository_source.contains("job.resource_id = request.session_id"));
    assert!(repository_source.contains("job.resource_kind = 'file_transfer_session'"));
    assert!(repository_source.contains("AND job.command_type = 'file_transfer_download_chunk'"));

    let migration = include_str!("../../../../../migrations/0002_jobs_schedules.sql")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(migration.contains(
        "CREATE INDEX jobs_file_transfer_download_resource_idx ON public.jobs USING btree (resource_id, id) WHERE ((resource_kind = 'file_transfer_session'::text) AND (command_type = 'file_transfer_download_chunk'::text));"
    ));
}

#[test]
fn handoff_chunk_evidence_preserves_retry_and_artifact_semantics() {
    let unavailable_retry_job = Uuid::new_v4();
    let available_retry_job = Uuid::new_v4();
    let second_chunk_job = Uuid::new_v4();
    let object_output = handoff_output(
        unavailable_retry_job,
        1,
        "object_store",
        &[],
        Some(("chunks/missing", "a", 3)),
    );
    let chunks = vec![
        FileTransferDownloadHandoffChunk {
            job_id: unavailable_retry_job,
            offset: 0,
            size_bytes: 3,
            sha256_hex: "chunk-a".to_string(),
            outputs: vec![object_output.clone()],
        },
        FileTransferDownloadHandoffChunk {
            job_id: available_retry_job,
            offset: 0,
            size_bytes: 3,
            sha256_hex: "chunk-a".to_string(),
            outputs: vec![handoff_output(
                available_retry_job,
                1,
                "inline",
                b"abc",
                None,
            )],
        },
        FileTransferDownloadHandoffChunk {
            job_id: second_chunk_job,
            offset: 3,
            size_bytes: 2,
            sha256_hex: "chunk-b".to_string(),
            outputs: vec![handoff_output(second_chunk_job, 1, "inline", b"de", None)],
        },
    ];

    let evidence = assess_handoff_chunk_evidence(&chunks, 5, &BTreeSet::new(), &BTreeMap::new());

    assert!(evidence.available);
    assert_eq!(evidence.status, HANDOFF_EVIDENCE_RETAINED_OUTPUTS_AVAILABLE);
    assert!(evidence.reason.is_none());

    let active_artifact = BTreeSet::from([(unavailable_retry_job, object_output.seq)]);
    let object_only = &chunks[..1];
    let evidence =
        assess_handoff_chunk_evidence(object_only, 3, &active_artifact, &BTreeMap::new());
    assert!(evidence.available);
}

#[test]
fn handoff_chunk_evidence_preserves_unavailable_classification() {
    let no_artifacts = BTreeSet::new();
    let pruned = assess_handoff_chunk_evidence(&[], 1, &no_artifacts, &BTreeMap::new());
    assert_eq!(pruned.status, HANDOFF_EVIDENCE_RETAINED_OUTPUTS_PRUNED);
    assert_eq!(
        pruned.reason.as_deref(),
        Some("retained_chunk_outputs_pruned")
    );

    let first_job = Uuid::new_v4();
    let conflicting_job = Uuid::new_v4();
    let conflict = assess_handoff_chunk_evidence(
        &[
            FileTransferDownloadHandoffChunk {
                job_id: first_job,
                offset: 0,
                size_bytes: 3,
                sha256_hex: "first".to_string(),
                outputs: vec![handoff_output(first_job, 1, "inline", b"abc", None)],
            },
            FileTransferDownloadHandoffChunk {
                job_id: conflicting_job,
                offset: 0,
                size_bytes: 3,
                sha256_hex: "different".to_string(),
                outputs: vec![handoff_output(conflicting_job, 1, "inline", b"abc", None)],
            },
        ],
        3,
        &no_artifacts,
        &BTreeMap::new(),
    );
    assert_eq!(conflict.status, HANDOFF_EVIDENCE_RETAINED_OUTPUTS_CONFLICT);
    assert_eq!(
        conflict.reason.as_deref(),
        Some("duplicate_offset_conflict")
    );

    let gap_job = Uuid::new_v4();
    let gap = assess_handoff_chunk_evidence(
        &[FileTransferDownloadHandoffChunk {
            job_id: gap_job,
            offset: 1,
            size_bytes: 2,
            sha256_hex: "gap".to_string(),
            outputs: vec![handoff_output(gap_job, 1, "inline", b"ab", None)],
        }],
        2,
        &no_artifacts,
        &BTreeMap::new(),
    );
    assert_eq!(gap.status, HANDOFF_EVIDENCE_RETAINED_OUTPUTS_INCOMPLETE);
    assert_eq!(gap.reason.as_deref(), Some("chunk_gap"));
}

#[test]
fn summarized_handoff_evidence_preserves_status_and_part_semantics() {
    let session_id = Uuid::new_v4();
    let status = serde_json::to_vec(&serde_json::json!({
        "type": "file_transfer_download_chunk",
        "session_id": session_id,
        "extra": {
            "offset": 0,
            "chunk_size_bytes": 5,
            "chunk_sha256_hex": "a".repeat(64)
        }
    }))
    .unwrap();
    let chunk = LoadedHandoffChunkEvidence {
        status_outputs: vec![b"malformed".to_vec(), status],
        stdout_sizes: vec![2, 3],
        stdout_available: vec![true, true],
    };

    let available =
        assess_loaded_handoff_chunk_evidence(std::slice::from_ref(&chunk), session_id, 5);
    assert!(available.available);
    assert_eq!(
        available.status,
        HANDOFF_EVIDENCE_RETAINED_OUTPUTS_AVAILABLE
    );

    let mut unavailable_part = chunk.clone();
    unavailable_part.stdout_available[1] = false;
    let incomplete = assess_loaded_handoff_chunk_evidence(&[unavailable_part], session_id, 5);
    assert_eq!(
        incomplete.status,
        HANDOFF_EVIDENCE_RETAINED_OUTPUTS_INCOMPLETE
    );
    assert_eq!(
        incomplete.reason.as_deref(),
        Some("chunk_output_unavailable")
    );

    let mut status_only = chunk;
    status_only.stdout_sizes.clear();
    status_only.stdout_available.clear();
    let pruned = assess_loaded_handoff_chunk_evidence(&[status_only], session_id, 5);
    assert_eq!(pruned.status, HANDOFF_EVIDENCE_RETAINED_OUTPUTS_PRUNED);
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

fn handoff_output(
    job_id: Uuid,
    seq: i32,
    storage: &str,
    data: &[u8],
    artifact: Option<(&str, &str, i64)>,
) -> JobOutputView {
    JobOutputView {
        job_id,
        client_id: "edge-a".to_string(),
        seq,
        stream: "stdout".to_string(),
        data_base64: BASE64.encode(data),
        storage: storage.to_string(),
        artifact_object_key: artifact.map(|value| value.0.to_string()),
        artifact_sha256_hex: artifact.map(|value| value.1.to_string()),
        artifact_size_bytes: artifact.map(|value| value.2),
        exit_code: None,
        done: false,
        received_at: None,
        created_at: "2026-08-26T00:00:00Z".to_string(),
    }
}
