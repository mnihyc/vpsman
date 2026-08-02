use super::*;

#[test]
fn reads_file_transfer_chunk_with_hashable_payload() {
    let path = std::env::temp_dir().join(format!("vpsman-transfer-cli-{}", Uuid::new_v4()));
    fs::write(&path, b"abcdef").unwrap();
    let chunk = read_transfer_chunk(&path, 2, 3).unwrap();
    assert_eq!(chunk, b"cde");
    assert_eq!(payload_hash(&chunk).len(), 64);
    let _ = fs::remove_file(path);
}

#[test]
fn reads_file_transfer_chunk_from_retained_bytes() {
    let chunk = read_transfer_chunk_from_bytes(b"abcdef", 1, 4).unwrap();
    assert_eq!(chunk, b"bcde");
    assert_eq!(payload_hash(&chunk).len(), 64);
}

#[test]
fn detects_divergent_transfer_offsets() {
    let session_id = Uuid::new_v4();
    let statuses = vec![
        TransferClientStatus {
            client_id: "edge-a".to_string(),
            payload: TransferStatusPayload {
                status_type: "file_transfer_chunk_ack".to_string(),
                session_id,
                next_offset: 64,
                size_bytes: Some(128),
                extra: serde_json::Value::Null,
            },
        },
        TransferClientStatus {
            client_id: "edge-b".to_string(),
            payload: TransferStatusPayload {
                status_type: "file_transfer_chunk_ack".to_string(),
                session_id,
                next_offset: 32,
                size_bytes: Some(128),
                extra: serde_json::Value::Null,
            },
        },
    ];
    assert!(uniform_next_offset(&statuses, 128).is_err());
}

#[test]
fn parses_file_transfer_multi_target_policy() {
    assert_eq!(
        FileTransferMultiTargetPolicy::parse("same-offset").unwrap(),
        FileTransferMultiTargetPolicy::SameOffset
    );
    assert_eq!(
        FileTransferMultiTargetPolicy::parse("independent_offsets").unwrap(),
        FileTransferMultiTargetPolicy::IndependentOffsets
    );
    assert!(FileTransferMultiTargetPolicy::parse("unknown").is_err());
}

#[test]
fn transfer_poll_limit_zero_is_unlimited() {
    assert_eq!(transfer_poll_limit(0), None);
    assert_eq!(transfer_poll_limit(1), Some(1));
    assert_eq!(transfer_poll_limit(100_001), Some(100_000));
}

#[test]
fn groups_independent_targets_by_current_offset() {
    let offsets = BTreeMap::from([
        ("edge-a".to_string(), 0),
        ("edge-b".to_string(), 64),
        ("edge-c".to_string(), 64),
        ("edge-d".to_string(), 128),
    ]);

    let grouped = targets_grouped_by_offset(&offsets, 128);

    assert_eq!(
        grouped,
        vec![
            (0, vec!["edge-a".to_string()]),
            (64, vec!["edge-b".to_string(), "edge-c".to_string()])
        ]
    );
}

#[test]
fn builds_target_offset_map_from_statuses() {
    let session_id = Uuid::new_v4();
    let statuses = vec![
        TransferClientStatus {
            client_id: "edge-a".to_string(),
            payload: TransferStatusPayload {
                status_type: "file_transfer_start".to_string(),
                session_id,
                next_offset: 64,
                size_bytes: Some(128),
                extra: serde_json::Value::Null,
            },
        },
        TransferClientStatus {
            client_id: "edge-b".to_string(),
            payload: TransferStatusPayload {
                status_type: "file_transfer_start".to_string(),
                session_id,
                next_offset: 32,
                size_bytes: Some(128),
                extra: serde_json::Value::Null,
            },
        },
    ];

    let offsets = target_offsets_from_statuses(&statuses, 128).unwrap();

    assert_eq!(offsets["edge-a"], 64);
    assert_eq!(offsets["edge-b"], 32);
    assert!(ensure_all_targets_at_offset(&offsets, 128, "test commit").is_err());
}

#[test]
fn skipped_existing_upload_targets_are_not_active_commit_targets() {
    let session_id = Uuid::new_v4();
    let statuses = vec![
        TransferClientStatus {
            client_id: "edge-a".to_string(),
            payload: TransferStatusPayload {
                status_type: "file_transfer_start".to_string(),
                session_id,
                next_offset: 128,
                size_bytes: Some(128),
                extra: serde_json::json!({
                    "skipped": true,
                    "reason": "destination_exists",
                }),
            },
        },
        TransferClientStatus {
            client_id: "edge-b".to_string(),
            payload: TransferStatusPayload {
                status_type: "file_transfer_start".to_string(),
                session_id,
                next_offset: 0,
                size_bytes: Some(128),
                extra: serde_json::Value::Null,
            },
        },
    ];

    assert_eq!(active_transfer_targets(&statuses), vec!["edge-b"]);
    assert_eq!(active_statuses(&statuses).len(), 1);
    let offsets = target_offsets_from_statuses(&statuses, 128).unwrap();
    assert_eq!(offsets["edge-a"], 128);
    assert_eq!(offsets["edge-b"], 0);
}

#[test]
fn parses_transfer_status_from_job_output() {
    let session_id = Uuid::new_v4();
    let payload = serde_json::json!({
        "type": "file_transfer_start",
        "session_id": session_id,
        "next_offset": 0,
        "size_bytes": 7,
    });
    let output = JobOutputRecord {
        client_id: "edge-a".to_string(),
        stream: "status".to_string(),
        data_base64: BASE64.encode(serde_json::to_vec(&payload).unwrap()),
    };
    let parsed = parse_transfer_status(&output, session_id, "file_transfer_start")
        .unwrap()
        .unwrap();
    assert_eq!(parsed.next_offset, 0);
    assert_eq!(parsed.size_bytes, Some(7));
}
