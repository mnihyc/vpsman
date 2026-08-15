use super::*;
use crate::test_support::PgWorkerTestDb;

#[test]
fn webhook_rule_worker_config_clamps_operational_bounds_and_validates_retention() {
    assert_eq!(
        WebhookRuleWorkerConfig::new(0, 0, 1, 1, 0, 0).unwrap(),
        WebhookRuleWorkerConfig {
            delivery_limit: 1,
            materialize_limit: 1,
            retention_days: 1,
            telemetry_event_retention_days: 1,
            retention_prune_limit: 1,
            webhook_timeout_secs: 1,
        }
    );
    assert_eq!(
        WebhookRuleWorkerConfig::new(10_000, 10_000, 3_650, 3_650, 20_000, 120).unwrap(),
        WebhookRuleWorkerConfig {
            delivery_limit: 200,
            materialize_limit: 1000,
            retention_days: 3_650,
            telemetry_event_retention_days: 3_650,
            retention_prune_limit: 10_000,
            webhook_timeout_secs: 60,
        }
    );
    assert!(WebhookRuleWorkerConfig::new(25, 100, 0, 1, 1_000, 5).is_err());
    assert!(WebhookRuleWorkerConfig::new(25, 100, 3_651, 7, 1_000, 5).is_err());
    assert_eq!(
        WebhookRuleWorkerConfig::new(25, 100, 3, 7, 1_000, 5)
            .unwrap()
            .telemetry_event_retention_days,
        3
    );
}

#[tokio::test]
async fn postgres_prunes_only_processed_telemetry_events_past_the_short_retention() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let old_processed = Uuid::from_u128(1);
    let old_unprocessed = Uuid::from_u128(2);
    let recent_processed = Uuid::from_u128(3);
    let old_non_telemetry = Uuid::from_u128(4);
    sqlx::query(
        r#"
        INSERT INTO webhook_events (
            id,
            kind,
            event_id,
            payload,
            occurred_at,
            processed_at
        ) VALUES
            ($1, 'telemetry.rollup', 'old-processed', '{}'::jsonb,
                now() - interval '8 days', now() - interval '8 days'),
            ($2, 'telemetry.rollup', 'old-unprocessed', '{}'::jsonb,
                now() - interval '8 days', NULL),
            ($3, 'telemetry.rollup', 'recent-processed', '{}'::jsonb,
                now() - interval '6 days', now() - interval '6 days'),
            ($4, 'alert.open', 'old-non-telemetry', '{}'::jsonb,
                now() - interval '8 days', now() - interval '8 days')
        "#,
    )
    .bind(old_processed)
    .bind(old_unprocessed)
    .bind(recent_processed)
    .bind(old_non_telemetry)
    .execute(&db.pool)
    .await
    .unwrap();

    let config = WebhookRuleWorkerConfig::new(25, 100, 90, 7, 1_000, 5).unwrap();
    assert_eq!(
        prune_processed_telemetry_events(&db.pool, config)
            .await
            .unwrap(),
        1
    );
    let remaining = sqlx::query_scalar::<_, String>(
        "SELECT event_id FROM webhook_events ORDER BY event_id ASC",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        remaining,
        vec![
            "old-non-telemetry".to_string(),
            "old-unprocessed".to_string(),
            "recent-processed".to_string(),
        ]
    );

    db.cleanup().await;
}

#[test]
fn delivery_error_is_bounded() {
    let error = "x".repeat(MAX_ERROR_BYTES + 100);
    assert_eq!(truncate_error(&error).len(), MAX_ERROR_BYTES);
}

#[test]
fn delivery_error_keeps_nested_transport_cause() {
    let error = anyhow::anyhow!("connection refused").context("webhook request failed");
    assert_eq!(
        format_delivery_error(&error),
        "webhook request failed: connection refused"
    );
}

#[test]
fn automatic_delivery_cooldown_blocks_new_events_but_not_boundary_event() {
    assert!(delivery_candidate_is_suppressed(false, 1_300, 1_299));
    assert!(!delivery_candidate_is_suppressed(false, 1_300, 1_300));
    assert!(delivery_candidate_is_suppressed(true, 0, 1_300));
}

#[test]
fn enabled_rule_pagination_advances_and_interval_checks_include_later_pages() {
    let rule = |id, expression: &str| RuleRow {
        id: Uuid::from_u128(id),
        actor_id: None,
        name: format!("rule-{id}"),
        expression: expression.to_string(),
        target: "https://hooks.acme.com/vpsman".to_string(),
        body_template: String::new(),
        cooldown_secs: 30,
    };
    let first_page = vec![rule(1, "(tag:edge"), rule(2, "status = online")];
    let final_page = vec![rule(3, "interval.30sec && tag:edge")];

    assert_eq!(
        next_enabled_rule_cursor(&first_page, 2),
        Some(Uuid::from_u128(2))
    );
    assert_eq!(next_enabled_rule_cursor(&final_page, 2), None);

    let all_rules = first_page.into_iter().chain(final_page).collect::<Vec<_>>();
    assert!(all_rules.iter().any(|rule| {
        validated_rule_expression(rule).is_ok_and(|expression| {
            expression_referenced_events(&expression).contains("interval.30sec")
        })
    }));
}

#[test]
fn persisted_rule_validation_reports_expression_and_template_errors() {
    let rule = |expression: &str, body_template: &str| RuleRow {
        id: Uuid::from_u128(1),
        actor_id: None,
        name: "rule".to_string(),
        expression: expression.to_string(),
        target: "https://hooks.acme.com/vpsman".to_string(),
        body_template: body_template.to_string(),
        cooldown_secs: 30,
    };

    let expression_error = validated_rule_expression(&rule("(tag:edge", ""))
        .unwrap_err()
        .to_string();
    assert!(expression_error.starts_with("invalid webhook rule expression:"));

    let template_error = validated_rule_expression(&rule("tag:edge", "[if alert.open]missing end"))
        .unwrap_err()
        .to_string();
    assert!(template_error.starts_with("invalid webhook rule template:"));

    assert!(validated_rule_expression(&rule(
        "interval.30sec && tag:edge",
        "{rule.name} {event.kind}"
    ))
    .is_ok());
}

#[test]
fn configuration_failure_identity_changes_only_with_material_configuration() {
    let mut rule = RuleRow {
        id: Uuid::from_u128(1),
        actor_id: None,
        name: "rule".to_string(),
        expression: "(tag:edge".to_string(),
        target: "https://hooks.acme.com/vpsman".to_string(),
        body_template: String::new(),
        cooldown_secs: 30,
    };
    let original = rule_configuration_failure_event_id(&rule);
    assert_eq!(original, rule_configuration_failure_event_id(&rule));

    rule.name = "renamed rule".to_string();
    assert_eq!(original, rule_configuration_failure_event_id(&rule));

    rule.expression = "(tag:core".to_string();
    assert_ne!(original, rule_configuration_failure_event_id(&rule));
}

#[test]
fn legacy_broad_manual_event_is_classified_for_fail_closed_skip() {
    let event = EventRow {
        id: Uuid::from_u128(7),
        actor_id: None,
        kind: "interval.30sec".to_string(),
        event_id: "legacy-reviewed-event".to_string(),
        event_predicates: vec!["interval.30sec".to_string()],
        subject_client_ids: Vec::new(),
        payload: json!({
            "event": {
                "kind": "interval.30sec",
                "id": "legacy-reviewed-event",
                "source": "manual_dispatch",
            }
        }),
        occurred_at_unix: 1,
    };
    assert!(is_legacy_broad_manual_dispatch(&event));

    let mut automatic = event;
    automatic.payload["event"]["source"] = Value::String("automatic".to_string());
    assert!(!is_legacy_broad_manual_dispatch(&automatic));
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
fn candidate_uses_interval_predicate_and_aggregates_matches() {
    let rule = RuleRow {
        id: Uuid::nil(),
        actor_id: None,
        name: "edge interval".to_string(),
        expression: "interval.30sec && tag:edge".to_string(),
        target: "https://hooks.acme.com/vpsman".to_string(),
        body_template: "{event.kind} {vps.id}".to_string(),
        cooldown_secs: 30,
    };
    let vps_rows = vec![
        VpsRow {
            id: "edge-a".to_string(),
            display_name: "edge-a".to_string(),
            status: "online".to_string(),
            tags: vec!["edge".to_string()],
            registration_ip: None,
            last_ip: None,
            last_seen_at: None,
            internal_build_number: 1,
            stale_since: None,
            stale_reason: None,
            capabilities: json!({}),
            vps_rules: VpsRuleContext::default(),
        },
        VpsRow {
            id: "core-a".to_string(),
            display_name: "core-a".to_string(),
            status: "online".to_string(),
            tags: vec!["core".to_string()],
            registration_ip: None,
            last_ip: None,
            last_seen_at: None,
            internal_build_number: 1,
            stale_since: None,
            stale_reason: None,
            capabilities: json!({}),
            vps_rules: VpsRuleContext::default(),
        },
    ];
    let candidate =
        delivery_candidate_for_rule(&rule, "interval.30sec", "interval.30sec:1", &vps_rows, 1)
            .unwrap()
            .unwrap();
    assert_eq!(candidate.matched_vps.len(), 1);
    assert_eq!(candidate.message, "interval.30sec edge-a");
}

#[test]
fn candidate_can_match_vps_rules_without_exposing_them_in_payload() {
    let rule = RuleRow {
        id: Uuid::nil(),
        actor_id: None,
        name: "rule scoped interval".to_string(),
        expression: "interval.30sec && vps.rules:traffic.reset_day >= 15".to_string(),
        target: "https://hooks.acme.com/vpsman".to_string(),
        body_template: "{event.kind} {vps.id}".to_string(),
        cooldown_secs: 30,
    };
    let mut vps_rules = VpsRuleContext::default();
    insert_persisted_vps_rule(
        &mut vps_rules,
        "traffic.reset_day".to_string(),
        "015".to_string(),
        json!({"day": 15}),
    )
    .unwrap();
    let vps_rows = vec![VpsRow {
        id: "edge-a".to_string(),
        display_name: "edge-a".to_string(),
        status: "online".to_string(),
        tags: vec!["edge".to_string()],
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        internal_build_number: 1,
        stale_since: None,
        stale_reason: None,
        capabilities: json!({}),
        vps_rules,
    }];

    let candidate =
        delivery_candidate_for_rule(&rule, "interval.30sec", "interval.30sec:1", &vps_rows, 1)
            .unwrap()
            .unwrap();
    assert_eq!(candidate.matched_vps.len(), 1);
    assert_eq!(candidate.payload["matched_vps"][0].get("vps_rules"), None);
    assert!(insert_persisted_vps_rule(
        &mut VpsRuleContext::default(),
        "network.port_speed".to_string(),
        "not-a-speed".to_string(),
        json!({}),
    )
    .is_err());
}

#[test]
fn policy_alert_event_uses_event_roots_without_webhook_rule_collision() {
    let rule = RuleRow {
        id: Uuid::nil(),
        actor_id: None,
        name: "delivery-rule".to_string(),
        expression: "alert.policy_reached && alert.category:traffic && traffic.cycle_percent >= 80 && policy.name = monthly && rule.name = quota-80 && policy_rule.window_secs = 0".to_string(),
        target: "https://hooks.acme.com/vpsman".to_string(),
        body_template:
            "{rule.name} {policy.name} {policy_rule.name} {traffic.cycle_percent}".to_string(),
        cooldown_secs: 30,
    };
    let event = EventRow {
        id: Uuid::from_u128(7),
        actor_id: None,
        kind: "alert.policy_reached".to_string(),
        event_id: "policy-alert:test".to_string(),
        event_predicates: vec![
            "alert.policy_reached".to_string(),
            "alert.category:traffic".to_string(),
        ],
        subject_client_ids: vec!["edge-a".to_string()],
        payload: json!({
            "event": {"kind": "alert.policy_reached"},
            "alert": {"category": "traffic"},
            "policy": {"name": "monthly"},
            "rule": {"name": "quota-80", "window_secs": 0},
            "traffic": {"cycle_percent": 82.0},
        }),
        occurred_at_unix: 1,
    };
    let vps_rows = vec![VpsRow {
        id: "edge-a".to_string(),
        display_name: "edge-a".to_string(),
        status: "online".to_string(),
        tags: vec!["edge".to_string()],
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        internal_build_number: 1,
        stale_since: None,
        stale_reason: None,
        capabilities: json!({}),
        vps_rules: VpsRuleContext::default(),
    }];

    let candidate = event_candidate_for_rule(&rule, &event, &vps_rows)
        .unwrap()
        .unwrap();
    assert_eq!(candidate.message, "delivery-rule monthly quota-80 82.0");
    assert_eq!(candidate.payload["rule"]["name"], "delivery-rule");
    assert_eq!(candidate.payload["policy_rule"]["name"], "quota-80");
    assert_eq!(candidate.payload["policy"]["name"], "monthly");
    assert_eq!(candidate.payload["traffic"]["cycle_percent"], 82.0);
}
