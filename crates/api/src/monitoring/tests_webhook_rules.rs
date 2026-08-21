use super::*;
use crate::repository::{MemoryState, Repository};
use vpsman_common::AgentCapabilitySnapshot;

fn agent(id: &str, tags: &[&str]) -> AgentView {
    AgentView {
        id: id.to_string(),
        display_name: id.to_string(),
        status: "online".to_string(),
        tags: tags.iter().map(|tag| tag.to_string()).collect(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: AgentCapabilitySnapshot::default(),
    }
}

#[test]
fn webhook_candidate_aggregates_matched_vps_and_renders_template() {
    let rule = WebhookRuleView {
        id: Uuid::nil(),
        name: "edge-online".to_string(),
        enabled: true,
        expression: "interval.30sec && tag:edge".to_string(),
        target: "https://hooks.acme.com/vpsman".to_string(),
        body_template: "{rule.name} {event.kind} {vps.id}".to_string(),
        cooldown_secs: 30,
        signing_secret: Some("secret".to_string()),
        signing_secret_set: true,
        notes: None,
        actor_id: None,
        created_at: "0".to_string(),
        updated_at: "0".to_string(),
    };
    let candidate = webhook_candidate_for_rule(
        &rule,
        "interval.30sec",
        "interval.30sec:1",
        vec![agent("edge-a", &["edge"]), agent("core-a", &["core"])],
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(candidate.matched_vps.len(), 1);
    assert_eq!(candidate.message, "edge-online interval.30sec edge-a");
    assert_eq!(candidate.signing_secret.as_deref(), Some("secret"));
}

#[test]
fn policy_alert_candidate_exposes_event_roots_without_webhook_rule_collision() {
    let rule = WebhookRuleView {
        id: Uuid::nil(),
        name: "delivery-rule".to_string(),
        enabled: true,
        expression: "alert.triggered && alert.category:traffic && traffic.cycle_percent >= 80 && policy.name = monthly && policy_rule.name = quota-80 && policy_rule.trigger_meta_condition.window_seconds = 0".to_string(),
        target: "https://hooks.acme.com/vpsman".to_string(),
        body_template:
            "{rule.name} {policy.name} {policy_rule.name} {traffic.cycle_percent}".to_string(),
        cooldown_secs: 30,
        signing_secret: None,
        signing_secret_set: false,
        notes: None,
        actor_id: None,
        created_at: "0".to_string(),
        updated_at: "0".to_string(),
    };
    let event_payload = json!({
        "event": {"kind": "alert.triggered"},
        "alert": {"category": "traffic"},
        "policy": {"name": "monthly"},
        "policy_rule": {
            "name": "quota-80",
            "trigger_meta_condition": {"kind": "immediate", "window_seconds": 0}
        },
        "traffic": {"cycle_percent": 82.0},
    });

    let candidate = webhook_candidate_for_event(
        &rule,
        "alert.triggered",
        "policy-alert:test",
        &[
            "alert.triggered".to_string(),
            "alert.category:traffic".to_string(),
        ],
        &event_payload,
        vec![agent("edge-a", &["edge"])],
        None,
    )
    .unwrap()
    .unwrap();

    assert_eq!(candidate.message, "delivery-rule monthly quota-80 82.0");
    assert_eq!(candidate.payload["rule"]["name"], "delivery-rule");
    assert_eq!(candidate.payload["policy_rule"]["name"], "quota-80");
    assert_eq!(candidate.payload["policy"]["name"], "monthly");
    assert_eq!(candidate.payload["traffic"]["cycle_percent"], 82.0);
}

#[test]
fn webhook_signature_uses_payload_bytes() {
    let signature = webhook_signature("secret", br#"{"hello":"world"}"#).unwrap();
    assert_eq!(
        signature,
        "sha256=2677ad3e7c090b2fa2c0fb13020d66d5420879b8316eb356a2d60fb9073bc778"
    );
}

#[test]
fn delivery_error_keeps_nested_transport_cause_and_is_bounded() {
    let error = anyhow::anyhow!("connection refused").context("webhook request failed");
    assert_eq!(
        format_delivery_error(&error),
        "webhook request failed: connection refused"
    );
    let long = anyhow::anyhow!("x".repeat(MAX_WEBHOOK_ERROR_BYTES + 100));
    assert_eq!(format_delivery_error(&long).len(), MAX_WEBHOOK_ERROR_BYTES);
}

#[test]
fn process_lease_covers_every_serial_delivery_timeout() {
    assert_eq!(delivery_lease_secs(0), 60);
    assert_eq!(delivery_lease_secs(50), 310);
    assert_eq!(delivery_lease_secs(200), 1_060);
}

#[tokio::test]
async fn manual_alert_delivery_processing_neutralizes_a_suspended_subject() {
    let memory = MemoryState::default();
    let mut subject = agent("edge-suspended", &["edge"]);
    subject.status = "suspended".to_string();
    memory.agents.write().await.push(subject.clone());
    let rule_id = Uuid::new_v4();
    memory.webhook_rules.write().await.push(WebhookRuleView {
        id: rule_id,
        name: "suspended-alert".to_string(),
        enabled: true,
        expression: "alert.triggered".to_string(),
        target: "https://hooks.example.invalid/vpsman".to_string(),
        body_template: "alert".to_string(),
        cooldown_secs: 0,
        signing_secret: None,
        signing_secret_set: false,
        notes: None,
        actor_id: None,
        created_at: "0".to_string(),
        updated_at: "0".to_string(),
    });
    let delivery_id = Uuid::new_v4();
    memory
        .webhook_rule_deliveries
        .write()
        .await
        .push(WebhookRuleDeliveryView {
            id: delivery_id,
            rule_id,
            rule_name: "suspended-alert".to_string(),
            event_kind: "alert.triggered".to_string(),
            event_id: format!("fleet-alert:{}:triggered", Uuid::new_v4()),
            status: vpsman_common::WEBHOOK_RULE_DELIVERY_STATUS_IN_PROGRESS.to_string(),
            target: "https://hooks.example.invalid/vpsman".to_string(),
            dedupe_key: format!("suspended-alert:{delivery_id}"),
            payload: json!({}),
            matched_vps: vec![subject],
            message: "alert".to_string(),
            signing_secret: None,
            error: None,
            cooldown_until_unix: 0,
            attempt_count: 0,
            next_attempt_at: None,
            last_attempt_at: None,
            actor_id: None,
            created_at: "0".to_string(),
            delivered_at: None,
            review_preview_hash: None,
        });
    let repo = Repository::Memory(memory);
    let lease_id = Uuid::new_v4();
    let mut guard = repo
        .begin_webhook_rule_alert_send(delivery_id, lease_id)
        .await
        .unwrap();
    assert!(!guard.is_deliverable());
    assert_eq!(guard.cancellation_reason(), Some("client_suspended"));
    let canceled = repo
        .cancel_claimed_webhook_rule_delivery(
            delivery_id,
            lease_id,
            "client_suspended",
            Some(&mut guard),
        )
        .await
        .unwrap();
    assert_eq!(
        canceled.status,
        vpsman_common::WEBHOOK_RULE_DELIVERY_STATUS_CANCELED_DISABLED
    );
    assert_eq!(canceled.error.as_deref(), Some("client_suspended"));
    guard.release().await.unwrap();
}
