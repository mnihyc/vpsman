use super::*;
use crate::model::{OperatorPreferences, OperatorView};

fn operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "test".to_string(),
            role: "admin".to_string(),
            scopes: Vec::new(),
            preferences: OperatorPreferences::default(),
            totp_enabled: false,
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
        },
        session_id: None,
    }
}

#[test]
fn webhook_url_policy_requires_public_https_by_default() {
    assert!(validate_webhook_rule_target("https://hooks.acme.com/vpsman").is_ok());
    assert!(validate_webhook_rule_target("http://localhost:9000/hook").is_err());
    assert!(validate_webhook_rule_target("http://127.0.0.1:9000/hook").is_err());
    assert!(validate_webhook_rule_target("http://hooks.acme.com/hook").is_err());
    assert!(validate_webhook_rule_target("https://127.0.0.1/hook").is_err());
    assert!(validate_webhook_rule_target("https://user:secret@example.com/hook").is_err());
}

#[test]
fn webhook_rule_request_validates_expression_and_target() {
    let mut request = CreateWebhookRuleRequest {
        id: None,
        name: "stale edge".to_string(),
        enabled: true,
        expression: "status = stale && tag:edge".to_string(),
        target: "https://hooks.acme.com/vpsman".to_string(),
        body_template: "{vps.name} stale".to_string(),
        signing_secret: None,
        clear_signing_secret: false,
        cooldown_secs: Some(60),
        notes: None,
        confirmed: true,
    };
    assert!(webhook_rule_from_request(&request, &operator()).is_ok());
    request.expression = "status in []".to_string();
    assert!(webhook_rule_from_request(&request, &operator()).is_err());
}

#[test]
fn webhook_rotation_hash_is_stable_across_scan_batch_order() {
    let first = Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap();
    let second = Uuid::parse_str("22222222-2222-4333-8444-555555555555").unwrap();
    let mut forward = vec![first, second];
    let mut reverse = vec![second, first];

    let forward_hash = webhook_rotation_preview_hash(
        Some("2026-07-01T00:00:00+00:00"),
        Some(WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED),
        None,
        &mut forward,
    )
    .unwrap();
    let reverse_hash = webhook_rotation_preview_hash(
        Some("2026-07-01T00:00:00+00:00"),
        Some(WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED),
        None,
        &mut reverse,
    )
    .unwrap();

    assert_eq!(forward_hash, reverse_hash);
}

#[test]
fn webhook_rotation_hash_changes_when_the_reviewed_set_changes() {
    let first = Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap();
    let second = Uuid::parse_str("22222222-2222-4333-8444-555555555555").unwrap();
    let mut one = vec![first];
    let mut two = vec![first, second];

    let one_hash = webhook_rotation_preview_hash(None, None, None, &mut one).unwrap();
    let two_hash = webhook_rotation_preview_hash(None, None, None, &mut two).unwrap();

    assert_ne!(one_hash, two_hash);
}
