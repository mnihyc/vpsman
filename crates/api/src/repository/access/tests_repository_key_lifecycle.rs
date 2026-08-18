use super::*;

#[test]
fn replacement_transition_requires_an_existing_non_offline_status() {
    assert_eq!(replacement_transition_prior_status(true, None), None);
    assert_eq!(
        replacement_transition_prior_status(true, Some("offline")),
        None
    );
    assert_eq!(
        replacement_transition_prior_status(true, Some("never")),
        Some("never")
    );
    assert_eq!(
        replacement_transition_prior_status(false, Some("online")),
        None
    );
}

#[test]
fn current_key_revocation_is_found_beyond_the_legacy_report_horizon() {
    let client_id = "edge-current";
    let fingerprint = "a".repeat(64);
    let mut revocations = (0..5_000)
        .map(|index| ClientKeyRevocationView {
            id: Uuid::new_v4(),
            client_id: format!("other-{index}"),
            public_key_sha256_hex: format!("{index:064x}"),
            reason: None,
            revoked_by: None,
            created_at: index.to_string(),
        })
        .collect::<Vec<_>>();
    revocations.push(ClientKeyRevocationView {
        id: Uuid::new_v4(),
        client_id: client_id.to_string(),
        public_key_sha256_hex: fingerprint.clone(),
        reason: Some("retired".to_string()),
        revoked_by: None,
        created_at: "0".to_string(),
    });

    let latest = latest_current_revocation(&revocations, client_id, Some(fingerprint.as_str()));

    assert_eq!(
        latest.and_then(|record| record.reason.as_deref()),
        Some("retired")
    );
}
