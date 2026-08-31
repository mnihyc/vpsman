use super::*;

fn shell_template_request(name: &str, scope_kind: &str) -> UpsertCommandTemplateRequest {
    UpsertCommandTemplateRequest {
        name: name.to_string(),
        scope_kind: scope_kind.to_string(),
        scope_value: None,
        display_group: None,
        operation: serde_json::json!({
            "type": "shell",
            "argv": ["/usr/bin/uptime"],
            "pty": false
        }),
        defaults: serde_json::json!({
            "max_timeout_secs": 30,
            "confirmed": false
        }),
        confirmed: true,
    }
}

#[test]
fn runtime_config_sync_command_templates_are_forbidden() {
    let mut request = shell_template_request("forbidden runtime sync", "global");
    request.operation = serde_json::to_value(JobCommand::RuntimeConfigSync {
        desired_version: 1,
        reason: "template must not own desired state".to_string(),
        config: Box::new(vpsman_common::AgentRuntimeConfig::default()),
    })
    .unwrap();
    let error = validate_command_template_request(&request).unwrap_err();
    assert!(error.to_string().contains("server-issued"));
}

fn inline_output(seq: i32, stream: &str, data: &[u8]) -> JobOutputView {
    JobOutputView {
        job_id: Uuid::nil(),
        client_id: "comparison-client".to_string(),
        seq,
        stream: stream.to_string(),
        data_base64: base64::engine::general_purpose::STANDARD.encode(data),
        storage: "inline".to_string(),
        artifact_object_key: None,
        artifact_sha256_hex: None,
        artifact_size_bytes: None,
        exit_code: None,
        done: stream == "status",
        received_at: None,
        created_at: "test".to_string(),
    }
}

fn artifact_output(seq: i32, stream: &str, size: i64, digest: &str) -> JobOutputView {
    JobOutputView {
        storage: "object_store".to_string(),
        artifact_object_key: Some(format!("test/{digest}")),
        artifact_sha256_hex: Some(digest.to_string()),
        artifact_size_bytes: Some(size),
        ..inline_output(seq, stream, &[])
    }
}

#[tokio::test]
async fn output_comparison_ignores_inline_transport_chunking_and_interleaving() {
    let split = vec![
        inline_output(0, "stdout", b"hello "),
        inline_output(1, "stderr", b"warning\n"),
        inline_output(2, "stdout", b"world\n"),
        inline_output(3, "status", b""),
    ];
    let consolidated = vec![
        inline_output(0, "stdout", b"hello world\n"),
        inline_output(1, "status", b""),
        inline_output(2, "stderr", b"warning\n"),
    ];

    for mode in ["binary", "text"] {
        let split_signature = output_signature(split.clone(), mode, None, usize::MAX)
            .await
            .unwrap();
        let consolidated_signature = output_signature(consolidated.clone(), mode, None, usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            split_signature.output_digest_hex,
            consolidated_signature.output_digest_hex
        );
        assert_eq!(split_signature.preview, consolidated_signature.preview);
        assert_eq!(
            split_signature.byte_count,
            consolidated_signature.byte_count
        );
    }
}

#[tokio::test]
async fn text_comparison_normalizes_after_reassembling_split_lines() {
    let split = vec![
        inline_output(0, "stdout", b"first\r"),
        inline_output(1, "stdout", b"\nsecond  \n"),
    ];
    let consolidated = vec![inline_output(0, "stdout", b"first\nsecond\n")];
    assert_eq!(
        output_signature(split, "text", None, usize::MAX)
            .await
            .unwrap()
            .output_digest_hex,
        output_signature(consolidated, "text", None, usize::MAX)
            .await
            .unwrap()
            .output_digest_hex
    );
}

#[tokio::test]
async fn output_comparison_preserves_stream_and_byte_identity() {
    let stdout = output_signature(
        vec![inline_output(0, "stdout", b"same")],
        "binary",
        None,
        usize::MAX,
    )
    .await
    .unwrap();
    let stderr = output_signature(
        vec![inline_output(0, "stderr", b"same")],
        "binary",
        None,
        usize::MAX,
    )
    .await
    .unwrap();
    let reordered = output_signature(
        vec![
            inline_output(0, "stdout", b"ba"),
            inline_output(1, "stdout", b"dc"),
        ],
        "binary",
        None,
        usize::MAX,
    )
    .await
    .unwrap();
    let original = output_signature(
        vec![inline_output(0, "stdout", b"abcd")],
        "binary",
        None,
        usize::MAX,
    )
    .await
    .unwrap();
    assert_ne!(stdout.output_digest_hex, stderr.output_digest_hex);
    assert_ne!(reordered.output_digest_hex, original.output_digest_hex);
}

#[tokio::test]
async fn text_artifact_fallback_preserves_metadata_comparison() {
    let stdout_hash = "a".repeat(64);
    let stderr_hash = "b".repeat(64);
    let first = output_signature(
        vec![
            artifact_output(0, "stdout", 4, &stdout_hash),
            artifact_output(1, "stderr", 7, &stderr_hash),
        ],
        "text",
        None,
        usize::MAX,
    )
    .await
    .unwrap();
    let second = output_signature(
        vec![
            artifact_output(0, "stderr", 7, &stderr_hash),
            artifact_output(1, "stdout", 4, &stdout_hash),
        ],
        "text",
        None,
        usize::MAX,
    )
    .await
    .unwrap();

    assert_eq!(first.output_compare_basis, "binary_metadata");
    assert_eq!(first.output_digest_hex, second.output_digest_hex);
    assert_eq!(first.byte_count, second.byte_count);
}

#[tokio::test]
async fn binary_comparison_ignores_mixed_inline_artifact_rechunking() {
    let root =
        std::env::temp_dir().join(format!("vpsman-output-comparison-mixed-{}", Uuid::new_v4()));
    let store = BackupObjectStore::filesystem(root.clone()).unwrap();
    let first_artifact_bytes = b"hello ";
    let second_artifact_bytes = b" world";
    let first_hash = vpsman_common::payload_hash(first_artifact_bytes);
    let second_hash = vpsman_common::payload_hash(second_artifact_bytes);
    store
        .put_new(&format!("test/{first_hash}"), first_artifact_bytes)
        .await
        .unwrap();
    store
        .put_new(&format!("test/{second_hash}"), second_artifact_bytes)
        .await
        .unwrap();

    let first = output_signature(
        vec![
            artifact_output(0, "stdout", first_artifact_bytes.len() as i64, &first_hash),
            inline_output(1, "stdout", b"world"),
        ],
        "binary",
        Some(&store),
        1024,
    )
    .await
    .unwrap();
    let second = output_signature(
        vec![
            inline_output(0, "stdout", b"hello"),
            artifact_output(
                1,
                "stdout",
                second_artifact_bytes.len() as i64,
                &second_hash,
            ),
        ],
        "binary",
        Some(&store),
        1024,
    )
    .await
    .unwrap();

    assert_eq!(first.output_compare_basis, "binary");
    assert_eq!(first.output_digest_hex, second.output_digest_hex);
    assert_eq!(first.byte_count, second.byte_count);
    assert_eq!(first.preview, "hello world");
    assert_eq!(first.preview, second.preview);
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn binary_comparison_rejects_corrupt_artifact_bytes() {
    let root = std::env::temp_dir().join(format!(
        "vpsman-output-comparison-corrupt-{}",
        Uuid::new_v4()
    ));
    let store = BackupObjectStore::filesystem(root.clone()).unwrap();
    let expected_hash = vpsman_common::payload_hash(b"expected");
    store
        .put_new(&format!("test/{expected_hash}"), b"corrupt!")
        .await
        .unwrap();

    let error = output_signature(
        vec![artifact_output(0, "stdout", 8, &expected_hash)],
        "binary",
        Some(&store),
        1024,
    )
    .await
    .unwrap_err();
    assert!(format!("{error:#}").contains("object hash mismatch"));
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn binary_comparison_rejects_missing_artifact_bytes() {
    let root = std::env::temp_dir().join(format!(
        "vpsman-output-comparison-missing-{}",
        Uuid::new_v4()
    ));
    let store = BackupObjectStore::filesystem(root.clone()).unwrap();
    let missing_hash = vpsman_common::payload_hash(b"missing");

    let error = output_signature(
        vec![artifact_output(0, "stdout", 7, &missing_hash)],
        "binary",
        Some(&store),
        1024,
    )
    .await
    .unwrap_err();
    assert!(format!("{error:#}").contains("failed to stat object"));
    tokio::fs::remove_dir_all(root).await.unwrap();
}
