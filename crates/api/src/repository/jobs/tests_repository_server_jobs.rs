use super::*;

#[test]
fn cleanup_review_limit_fails_instead_of_returning_a_partial_preview() {
    assert!(
        ensure_artifact_cleanup_match_capacity(MAX_ARTIFACT_CLEANUP_REVIEWED_TARGETS - 1).is_ok()
    );
    let error =
        ensure_artifact_cleanup_match_capacity(MAX_ARTIFACT_CLEANUP_REVIEWED_TARGETS).unwrap_err();
    assert!(error
        .to_string()
        .contains("narrow the domains or expression"));
}

#[test]
fn cleanup_job_persists_reviewed_targets_in_one_ordered_set_insert() {
    let source = include_str!("repository_server_jobs.rs");
    let creation = source
        .split("pub(crate) async fn create_artifact_cleanup_job")
        .nth(1)
        .and_then(|source| source.split("pub(crate) async fn list_server_jobs").next())
        .expect("artifact cleanup job creation body");
    assert_eq!(
        creation
            .matches("INSERT INTO server_job_artifact_cleanup_targets")
            .count(),
        1
    );
    assert!(creation.contains("FROM unnest("));
    assert!(creation.contains("WITH ORDINALITY AS target"));
    assert!(creation.contains("ORDER BY target.input_order"));
}

#[test]
fn cleanup_preview_rejects_invalid_or_overflowing_size_totals() {
    let candidate = |id, size_bytes| ServerArtifactCleanupCandidate {
        id,
        domain: "job_output".to_string(),
        object_key: format!("job-outputs/{id}"),
        sha256_hex: "a".repeat(64),
        size_bytes,
        status: "active".to_string(),
        job_id: None,
        client_id: Some("edge-a".to_string()),
        stream: Some("stdout".to_string()),
        seq: Some(0),
        created_at: "2026-07-01T00:00:00Z".to_string(),
        reference_protected: false,
    };
    let preview = |matched: &[ServerArtifactCleanupCandidate]| {
        cleanup_preview_from_matches(
            "artifact.status = \"active\"".to_string(),
            &["job_output".to_string()],
            matched,
        )
    };

    assert!(preview(&[candidate(Uuid::new_v4(), -1)]).is_err());
    assert!(preview(&[
        candidate(Uuid::new_v4(), i64::MAX),
        candidate(Uuid::new_v4(), 1),
    ])
    .is_err());
}

#[test]
fn cleanup_preview_includes_age_protection_and_representative_objects() {
    let first_id = Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap();
    let second_id = Uuid::parse_str("22222222-2222-4333-8444-555555555555").unwrap();
    let matched = vec![
        ServerArtifactCleanupCandidate {
            id: second_id,
            domain: "backup_artifact".to_string(),
            object_key: "backup-artifacts/protected.tar.zst".to_string(),
            sha256_hex: "b".repeat(64),
            size_bytes: 20,
            status: "active".to_string(),
            job_id: None,
            client_id: Some("agent-fra-02".to_string()),
            stream: None,
            seq: None,
            created_at: "2026-06-02T10:00:00Z".to_string(),
            reference_protected: true,
        },
        ServerArtifactCleanupCandidate {
            id: first_id,
            domain: "file_transfer_source".to_string(),
            object_key: "file-transfer-sources/payload.bin".to_string(),
            sha256_hex: "a".repeat(64),
            size_bytes: 10,
            status: "active".to_string(),
            job_id: None,
            client_id: Some("agent-sfo-01".to_string()),
            stream: None,
            seq: None,
            created_at: "2026-05-31T10:00:00Z".to_string(),
            reference_protected: false,
        },
    ];

    let preview = cleanup_preview_from_matches(
        "artifact.status = \"active\"".to_string(),
        &["file_transfer".to_string(), "backup_artifact".to_string()],
        &matched,
    )
    .unwrap();

    assert_eq!(preview.matched_count, 2);
    assert_eq!(preview.matched_bytes, 30);
    assert_eq!(
        preview.oldest_created_at.as_deref(),
        Some("2026-05-31T10:00:00Z")
    );
    assert_eq!(
        preview.newest_created_at.as_deref(),
        Some("2026-06-02T10:00:00Z")
    );
    assert_eq!(preview.retained_count, 1);
    assert_eq!(preview.reference_protected_count, 1);
    assert_eq!(preview.representative_objects.len(), 2);
    assert_eq!(
        preview.representative_objects[0].object_key,
        "file-transfer-sources/payload.bin"
    );
    assert!(!preview.representative_objects[0].reference_protected);
    assert!(preview.representative_objects[1].reference_protected);
    assert_eq!(
        preview.representative_objects[1].reason.as_deref(),
        Some("Reference protected by backup request")
    );
}
