use super::*;
use crate::model::{OperatorPreferences, OperatorView};
use crate::repository::MemoryState;

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

fn idless_webhook_request(target: &str) -> CreateWebhookRuleRequest {
    CreateWebhookRuleRequest {
        id: None,
        name: "retry-safe webhook".to_string(),
        enabled: true,
        expression: "interval.1min".to_string(),
        target: target.to_string(),
        body_template: "{event.kind}".to_string(),
        signing_secret: Some("retry-secret".to_string()),
        clear_signing_secret: false,
        cooldown_secs: Some(60),
        notes: Some("retry fixture".to_string()),
        confirmed: true,
    }
}

#[tokio::test]
async fn idless_webhook_exact_retry_reuses_identity_without_reapplying() {
    let memory = MemoryState::default();
    let repo = Repository::Memory(memory.clone());
    let operator = operator();

    let first = repo
        .upsert_webhook_rule(
            &idless_webhook_request("https://hooks.acme.com/vpsman"),
            &operator,
        )
        .await
        .unwrap();
    let retried = repo
        .upsert_webhook_rule(
            &idless_webhook_request("https://hooks.acme.com/vpsman"),
            &operator,
        )
        .await
        .unwrap();

    assert_eq!(retried.id, first.id);
    assert_eq!(retried.created_at, first.created_at);
    assert_eq!(retried.updated_at, first.updated_at);
    assert_eq!(memory.webhook_rules.read().await.len(), 1);
    assert_eq!(memory.audits.read().await.len(), 1);

    let conflict = repo
        .upsert_webhook_rule(
            &idless_webhook_request("https://hooks.acme.com/changed"),
            &operator,
        )
        .await
        .unwrap_err();
    assert_eq!(conflict.to_string(), "webhook_rule_name_conflict");
    assert_eq!(memory.webhook_rules.read().await.len(), 1);
}

#[tokio::test]
async fn canceled_webhook_upsert_cannot_commit_before_dependent_state_and_audit() {
    let memory = MemoryState::default();
    let repo = Repository::Memory(memory.clone());
    let audit_guard = memory.audits.write().await;
    let task = tokio::spawn({
        let repo = repo.clone();
        let operator = operator();
        async move {
            repo.upsert_webhook_rule(
                &idless_webhook_request("https://hooks.acme.com/cancellation"),
                &operator,
            )
            .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if memory.webhook_rules.try_write().is_err() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("webhook upsert did not reach the blocked audit acquisition");

    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    drop(audit_guard);

    assert!(memory.webhook_rules.read().await.is_empty());
    assert!(memory.webhook_rule_deliveries.read().await.is_empty());
    assert!(memory.audits.read().await.is_empty());
}

#[test]
fn pending_legacy_policy_payload_is_reshaped_to_the_canonical_rule_context() {
    let canonical = canonicalize_alert_event_payload(json!({
        "event": {
            "kind": "alert.policy_reached",
            "predicates": ["alert.policy_reached", "alert.open"]
        },
        "rule": {
            "id": "11111111-2222-4333-8444-555555555555",
            "name": "legacy resource threshold",
            "rule_version": 4,
            "condition_expression": "cpu.load_1 >= 3",
            "traffic_selector": null,
            "window_secs": 300
        }
    }));
    assert!(canonical.get("rule").is_none());
    assert_eq!(canonical["event"]["kind"], "alert.triggered");
    assert_eq!(canonical["event"]["predicates"], json!(["alert.triggered"]));
    assert_eq!(
        canonical["policy_rule"]["trigger_condition_expression"],
        "cpu.load_1 >= 3"
    );
    assert_eq!(
        canonical["policy_rule"]["trigger_meta_condition"],
        json!({"kind":"sustained","window_seconds":300})
    );
    assert_eq!(canonical["policy_rule"]["rule_kind"], "metric");
    assert_eq!(
        canonical["policy_rule"]["evidence_source"],
        "telemetry.combined"
    );
    assert!(canonical["policy_rule"]
        .get("condition_expression")
        .is_none());
    assert!(canonical["policy_rule"].get("window_secs").is_none());
}

#[test]
fn canonical_rule_audit_hashes_exact_bytes() {
    assert_eq!(
        sha256_text("alert.policy_reached"),
        "8455dff07cb9b0663064bb6ddc14fad0f30a7418cb7dc3d38885824f19a17dc9"
    );
    assert_ne!(
        sha256_text("alert.policy_reached"),
        sha256_text("alert.policy_reached ")
    );
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
