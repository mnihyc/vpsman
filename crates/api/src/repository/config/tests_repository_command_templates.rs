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

#[test]
fn output_comparison_ignores_inline_transport_chunking_and_interleaving() {
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
        let split_signature = output_signature(split.clone(), mode);
        let consolidated_signature = output_signature(consolidated.clone(), mode);
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

#[test]
fn text_comparison_normalizes_after_reassembling_split_lines() {
    let split = vec![
        inline_output(0, "stdout", b"first\r"),
        inline_output(1, "stdout", b"\nsecond  \n"),
    ];
    let consolidated = vec![inline_output(0, "stdout", b"first\nsecond\n")];
    assert_eq!(
        output_signature(split, "text").output_digest_hex,
        output_signature(consolidated, "text").output_digest_hex
    );
}

#[test]
fn output_comparison_preserves_stream_byte_and_artifact_identity() {
    let stdout = output_signature(vec![inline_output(0, "stdout", b"same")], "binary");
    let stderr = output_signature(vec![inline_output(0, "stderr", b"same")], "binary");
    let reordered = output_signature(
        vec![
            inline_output(0, "stdout", b"ba"),
            inline_output(1, "stdout", b"dc"),
        ],
        "binary",
    );
    let original = output_signature(vec![inline_output(0, "stdout", b"abcd")], "binary");
    assert_ne!(stdout.output_digest_hex, stderr.output_digest_hex);
    assert_ne!(reordered.output_digest_hex, original.output_digest_hex);

    let first_artifact = output_signature(
        vec![artifact_output(0, "stdout", 4, &"a".repeat(64))],
        "binary",
    );
    let second_artifact = output_signature(
        vec![artifact_output(0, "stdout", 4, &"b".repeat(64))],
        "binary",
    );
    assert_eq!(first_artifact.output_compare_basis, "binary_metadata");
    assert_ne!(
        first_artifact.output_digest_hex,
        second_artifact.output_digest_hex
    );
}

#[test]
fn artifact_metadata_comparison_ignores_cross_stream_packet_interleaving() {
    let stdout_hash = "a".repeat(64);
    let stderr_hash = "b".repeat(64);
    let first = output_signature(
        vec![
            artifact_output(0, "stdout", 4, &stdout_hash),
            artifact_output(1, "stderr", 7, &stderr_hash),
        ],
        "binary",
    );
    let second = output_signature(
        vec![
            artifact_output(0, "stderr", 7, &stderr_hash),
            artifact_output(1, "stdout", 4, &stdout_hash),
        ],
        "binary",
    );

    assert_eq!(first.output_compare_basis, "binary_metadata");
    assert_eq!(first.output_digest_hex, second.output_digest_hex);
    assert_eq!(first.byte_count, second.byte_count);
}
