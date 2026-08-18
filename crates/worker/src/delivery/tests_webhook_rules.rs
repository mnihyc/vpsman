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
            ($4, 'job.created', 'old-non-telemetry', '{}'::jsonb,
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

#[tokio::test]
async fn postgres_partition_rotation_never_drops_unprocessed_rows() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let partition_date = Utc::now().date_naive() - ChronoDuration::days(3);
    create_event_partition(&db.pool, partition_date)
        .await
        .unwrap();
    let event_id = Uuid::new_v4();
    let occurred_at = partition_date.and_hms_opt(12, 0, 0).unwrap().and_utc();
    sqlx::query(
        r#"
        INSERT INTO webhook_events (id, kind, event_id, payload, occurred_at)
        VALUES ($1, 'retention.partition_test', $2, '{}'::jsonb, $3)
        "#,
    )
    .bind(event_id)
    .bind(format!("retention-partition:{event_id}"))
    .bind(occurred_at)
    .execute(&db.pool)
    .await
    .unwrap();
    let config = WebhookRuleWorkerConfig::new(25, 100, 1, 1, 1_000, 5).unwrap();
    assert_eq!(
        drop_old_event_partitions(&db.pool, config).await.unwrap(),
        0
    );
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM webhook_events WHERE id=$1)",
    )
    .bind(event_id)
    .fetch_one(&db.pool)
    .await
    .unwrap());

    sqlx::query("UPDATE webhook_events SET processed_at=now() WHERE id=$1")
        .bind(event_id)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        drop_old_event_partitions(&db.pool, config).await.unwrap(),
        1
    );
    let partition_name = format!("webhook_events_{}", partition_date.format("%Y%m%d"));
    assert!(!sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM pg_tables
            WHERE schemaname=current_schema() AND tablename=$1
        )
        "#,
    )
    .bind(partition_name)
    .fetch_one(&db.pool)
    .await
    .unwrap());

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_generic_alert_resolution_materializes_once_for_an_explicit_retained_subject() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    insert_webhook_test_client(&db.pool, "retained-generic-edge", "deleted", true).await;
    let stable_rule = insert_webhook_test_rule(
        &db.pool,
        "retained-generic-resolution",
        "alert.resolved && alert.category:job && alert.record_kind = event",
    )
    .await;
    let mutable_vps_rule = insert_webhook_test_rule(
        &db.pool,
        "retained-generic-resolution-live-state",
        "alert.resolved && status = deleted",
    )
    .await;
    let episode_id = Uuid::new_v4();
    let event_id = format!("fleet-alert:{episode_id}:resolved");
    let subject_client_ids = vec!["retained-generic-edge".to_string()];
    let payload = json!({
        "event": {
            "kind": "alert.resolved",
            "id": &event_id,
            "occurred_at": "2026-08-18T12:00:00Z",
        },
        "alert": {
            "id": "job:failed:job-1",
            "episode_id": episode_id,
            "record_kind": "event",
            "producer_kind": "job",
            "trigger_generation": 1,
            "lifecycle_state": "resolved",
            "severity": "critical",
            "category": "job",
            "client_id": "retained-generic-edge",
            "resolved_at": "2026-08-18T12:00:00Z",
            "resolution_reason": "operator_resolved",
        },
    });
    assert!(insert_webhook_event(
        &db.pool,
        "alert.resolved",
        &event_id,
        &[
            "alert.resolved",
            "alert.category:job",
            "alert.severity:critical",
        ],
        &subject_client_ids,
        payload.clone(),
    )
    .await
    .unwrap());
    assert!(!insert_webhook_event(
        &db.pool,
        "alert.resolved",
        &event_id,
        &[
            "alert.resolved",
            "alert.category:job",
            "alert.severity:critical",
        ],
        &subject_client_ids,
        payload,
    )
    .await
    .unwrap());

    assert_eq!(
        process_webhook_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        (1, 0)
    );
    assert_eq!(
        process_webhook_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        (0, 0)
    );

    let deliveries = sqlx::query_as::<_, (Uuid, String, String, SqlJson<Value>, SqlJson<Value>)>(
        r#"
        SELECT rule_id, event_kind, event_id, payload, matched_vps
        FROM webhook_rule_deliveries
        ORDER BY rule_id
        "#,
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].0, stable_rule);
    assert_ne!(deliveries[0].0, mutable_vps_rule);
    assert_eq!(deliveries[0].1, "alert.resolved");
    assert_eq!(deliveries[0].2, event_id);
    assert_eq!(
        deliveries[0].3 .0["alert"]["episode_id"],
        episode_id.to_string()
    );
    assert_eq!(
        deliveries[0].3 .0["alert"]["resolution_reason"],
        "operator_resolved"
    );
    assert_eq!(deliveries[0].4 .0[0]["id"], "retained-generic-edge");
    assert_eq!(deliveries[0].4 .0[0]["status"], "deleted");

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_subjectless_interval_excludes_retained_subjects_loaded_for_other_events() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    insert_webhook_test_client(&db.pool, "visible-edge", "online", false).await;
    insert_webhook_test_client(&db.pool, "retained-edge", "deleted", true).await;
    let interval_rule =
        insert_webhook_test_rule(&db.pool, "visible-interval", "interval.30sec").await;
    assert!(insert_webhook_event(
        &db.pool,
        "agent.test",
        "retained-subject-batch-loader",
        &["agent.test"],
        &["retained-edge".to_string()],
        json!({"event": {"kind": "agent.test"}}),
    )
    .await
    .unwrap());
    assert!(insert_webhook_event(
        &db.pool,
        "interval.30sec",
        "interval.30sec:retained-regression",
        &["interval.30sec"],
        &[],
        json!({"event": {"kind": "interval.30sec"}}),
    )
    .await
    .unwrap());

    assert_eq!(
        process_webhook_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        (1, 0)
    );
    let (rule_id, matched_vps) = sqlx::query_as::<_, (Uuid, SqlJson<Value>)>(
        "SELECT rule_id, matched_vps FROM webhook_rule_deliveries",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(rule_id, interval_rule);
    assert_eq!(matched_vps.0.as_array().map(Vec::len), Some(1));
    assert_eq!(matched_vps.0[0]["id"], "visible-edge");

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_subjectless_generic_alert_edges_do_not_borrow_fleet_subjects() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let event_only_rule = insert_webhook_test_rule(
        &db.pool,
        "global-alert-event-only",
        "(alert.triggered || alert.resolved) && alert.category:job",
    )
    .await;
    let vps_dependent_rule = insert_webhook_test_rule(
        &db.pool,
        "global-alert-vps-dependent",
        "(alert.triggered || alert.resolved) && status = online",
    )
    .await;
    let edge_payload = |kind: &str, event_id: &str, state: &str| {
        json!({
            "event": {"kind": kind, "id": event_id},
            "alert": {
                "id": "job:failed:global-job",
                "episode_id": Uuid::nil(),
                "record_kind": "event",
                "producer_kind": "job",
                "lifecycle_state": state,
                "severity": "critical",
                "category": "job",
                "target_kind": "job",
                "target_id": "global-job",
                "client_id": null,
            },
        })
    };

    assert!(insert_webhook_event(
        &db.pool,
        "alert.triggered",
        "fleet-alert:global-empty:triggered",
        &[
            "alert.triggered",
            "alert.category:job",
            "alert.severity:critical",
        ],
        &[],
        edge_payload(
            "alert.triggered",
            "fleet-alert:global-empty:triggered",
            "triggered",
        ),
    )
    .await
    .unwrap());
    assert_eq!(
        process_webhook_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        (1, 0)
    );

    insert_webhook_test_client(&db.pool, "unrelated-visible-edge", "online", false).await;
    assert!(insert_webhook_event(
        &db.pool,
        "alert.resolved",
        "fleet-alert:global-visible:resolved",
        &[
            "alert.resolved",
            "alert.category:job",
            "alert.severity:critical",
        ],
        &[],
        edge_payload(
            "alert.resolved",
            "fleet-alert:global-visible:resolved",
            "resolved",
        ),
    )
    .await
    .unwrap());
    assert!(!insert_webhook_event(
        &db.pool,
        "alert.resolved",
        "fleet-alert:global-visible:resolved",
        &[
            "alert.resolved",
            "alert.category:job",
            "alert.severity:critical",
        ],
        &[],
        edge_payload(
            "alert.resolved",
            "fleet-alert:global-visible:resolved",
            "resolved",
        ),
    )
    .await
    .unwrap());
    assert_eq!(
        process_webhook_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        (1, 0)
    );

    let deliveries = sqlx::query_as::<_, (Uuid, String, SqlJson<Value>)>(
        r#"
        SELECT rule_id, event_kind, matched_vps
        FROM webhook_rule_deliveries
        ORDER BY event_kind DESC
        "#,
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(deliveries.len(), 2);
    assert!(deliveries
        .iter()
        .all(|delivery| delivery.0 == event_only_rule));
    assert!(deliveries
        .iter()
        .all(|delivery| delivery.0 != vps_dependent_rule));
    assert_eq!(deliveries[0].1, "alert.triggered");
    assert_eq!(deliveries[1].1, "alert.resolved");
    assert!(deliveries
        .iter()
        .all(|delivery| delivery.2 .0.as_array().is_some_and(Vec::is_empty)));

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_permanent_failures_remain_delivery_and_audit_evidence_only() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    insert_webhook_test_rule(&db.pool, "invalid-no-recursive-alert", "(alert.triggered").await;
    assert!(insert_webhook_event(
        &db.pool,
        "alert.triggered",
        "fleet-alert:no-recursion:triggered",
        &["alert.triggered", "alert.category:job"],
        &[],
        json!({
            "event": {
                "kind": "alert.triggered",
                "id": "fleet-alert:no-recursion:triggered",
            },
            "alert": {
                "id": "job:failed:no-recursion",
                "record_kind": "event",
                "lifecycle_state": "triggered",
                "category": "job",
            },
        }),
    )
    .await
    .unwrap());

    assert_eq!(
        process_webhook_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        (1, 0)
    );
    assert_eq!(
        process_webhook_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        (0, 0)
    );
    let permanent_deliveries: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM webhook_rule_deliveries WHERE status = 'permanently_failed'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let permanent_failure_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs WHERE action = 'webhook.rule_delivery_permanently_failed'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let fabricated_states: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fleet_alert_states WHERE alert_id LIKE 'webhook_delivery:%'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let recursive_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM webhook_events WHERE event_id <> 'fleet-alert:no-recursion:triggered'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(permanent_deliveries, 1);
    assert_eq!(permanent_failure_audits, 1);
    assert_eq!(fabricated_states, 0);
    assert_eq!(recursive_events, 0);

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_delivery_retention_never_changes_operator_triage() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let rule_id =
        insert_webhook_test_rule(&db.pool, "retention-does-not-triage", "alert.triggered").await;
    let delivery_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO webhook_rule_deliveries (
            id, rule_id, rule_name, event_kind, event_id, status, target,
            dedupe_key, payload, matched_vps, message, cooldown_until_unix,
            created_at
        )
        VALUES (
            $1, $2, 'retention-does-not-triage', 'alert.triggered',
            'fleet-alert:retention:triggered', 'permanently_failed',
            'https://hooks.example.invalid/vpsman', 'retention-no-triage',
            '{}'::jsonb, '[]'::jsonb, 'failed', 0,
            now() - interval '2 days'
        )
        "#,
    )
    .bind(delivery_id)
    .bind(rule_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO fleet_alert_states (alert_id, state, reason)
        VALUES ($1, 'open', 'operator-owned legacy triage')
        "#,
    )
    .bind(format!("webhook_delivery:{delivery_id}"))
    .execute(&db.pool)
    .await
    .unwrap();

    let config = WebhookRuleWorkerConfig::new(25, 100, 1, 1, 1_000, 5).unwrap();
    assert_eq!(prune_deliveries(&db.pool, config).await.unwrap(), 1);
    let triage = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT state, reason FROM fleet_alert_states WHERE alert_id = $1",
    )
    .bind(format!("webhook_delivery:{delivery_id}"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(triage.0, "open");
    assert_eq!(triage.1.as_deref(), Some("operator-owned legacy triage"));

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
    assert!(delivery_candidate_is_suppressed(
        false,
        1_300,
        1_299,
        "job.created"
    ));
    assert!(!delivery_candidate_is_suppressed(
        false,
        1_300,
        1_300,
        "job.created"
    ));
    assert!(delivery_candidate_is_suppressed(
        true,
        0,
        1_300,
        "job.created"
    ));
}

#[test]
fn alert_lifecycle_edges_bypass_rule_cooldown_but_keep_exact_dedupe() {
    for event_kind in ["alert.triggered", "alert.resolved"] {
        assert!(!delivery_candidate_is_suppressed(
            false, 1_300, 1_100, event_kind
        ));
        assert!(delivery_candidate_is_suppressed(
            true, 1_300, 1_100, event_kind
        ));
    }
    assert!(delivery_candidate_is_suppressed(
        false,
        1_300,
        1_100,
        "job.created"
    ));
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

    let template_error =
        validated_rule_expression(&rule("tag:edge", "[if alert.triggered]missing end"))
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
            retained_tombstone: false,
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
            retained_tombstone: false,
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
        retained_tombstone: false,
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
        expression: "alert.triggered && alert.category:traffic && traffic.cycle_percent >= 80 && policy.name = monthly && policy_rule.name = quota-80 && policy_rule.trigger_meta_condition.window_seconds = 0".to_string(),
        target: "https://hooks.acme.com/vpsman".to_string(),
        body_template:
            "{rule.name} {policy.name} {policy_rule.name} {traffic.cycle_percent}".to_string(),
        cooldown_secs: 30,
    };
    let event = EventRow {
        id: Uuid::from_u128(7),
        actor_id: None,
        kind: "alert.triggered".to_string(),
        event_id: "policy-alert:test".to_string(),
        event_predicates: vec![
            "alert.triggered".to_string(),
            "alert.category:traffic".to_string(),
        ],
        subject_client_ids: vec!["edge-a".to_string()],
        payload: json!({
            "event": {"kind": "alert.triggered"},
            "alert": {"category": "traffic"},
            "policy": {"name": "monthly"},
            "policy_rule": {
                "name": "quota-80",
                "trigger_meta_condition": {"kind": "immediate", "window_seconds": 0}
            },
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
        retained_tombstone: false,
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

#[test]
fn generic_alert_lifecycle_edges_preserve_payload_and_subject_identity() {
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
        retained_tombstone: false,
        vps_rules: VpsRuleContext::default(),
    }];
    let episode_id = Uuid::from_u128(17);
    let lifecycle_event = |kind: &str, state: &str, suffix: &str| EventRow {
        id: Uuid::from_u128(if state == "triggered" { 18 } else { 19 }),
        actor_id: None,
        kind: kind.to_string(),
        event_id: format!("fleet-alert:{episode_id}:{suffix}"),
        event_predicates: vec![
            kind.to_string(),
            "alert.category:job".to_string(),
            "alert.severity:critical".to_string(),
        ],
        subject_client_ids: vec!["edge-a".to_string()],
        payload: json!({
            "event": {
                "kind": kind,
                "id": format!("fleet-alert:{episode_id}:{suffix}"),
                "occurred_at": "2026-08-18T12:00:00Z",
            },
            "alert": {
                "id": "job:failed:job-1",
                "episode_id": episode_id,
                "record_kind": "event",
                "producer_kind": "job",
                "trigger_generation": 1,
                "lifecycle_state": state,
                "severity": "critical",
                "category": "job",
                "target_kind": "job",
                "target_id": "job-1",
                "client_id": "edge-a",
                "title": "Job failed",
                "detail": "exit status 1",
                "status": "failed",
                "triggered_at": "2026-08-18T11:59:00Z",
                "last_confirmed_at": "2026-08-18T11:59:00Z",
                "resolved_at": if state == "resolved" {
                    Some("2026-08-18T12:00:00Z")
                } else {
                    None
                },
                "resolution_reason": if state == "resolved" {
                    Some("operator_resolved")
                } else {
                    None
                },
                "evidence": {"job_id": "job-1"},
            },
        }),
        occurred_at_unix: 1,
    };
    let candidate = |expression: &str, event: &EventRow| {
        event_candidate_for_rule(
            &RuleRow {
                id: Uuid::nil(),
                actor_id: None,
                name: "generic-alert-delivery".to_string(),
                expression: expression.to_string(),
                target: "https://hooks.acme.com/vpsman".to_string(),
                body_template: "{event.kind} {alert.id} {alert.lifecycle_state}".to_string(),
                cooldown_secs: 300,
            },
            event,
            &vps_rows,
        )
        .unwrap()
        .unwrap()
    };

    let triggered_event = lifecycle_event("alert.triggered", "triggered", "triggered");
    let triggered = candidate(
        "alert.triggered && alert.category:job && alert.record_kind = event",
        &triggered_event,
    );
    assert_eq!(triggered.event_kind, "alert.triggered");
    assert_eq!(triggered.event_id, triggered_event.event_id);
    assert_eq!(
        triggered.payload["alert"]["episode_id"],
        episode_id.to_string()
    );
    assert_eq!(triggered.payload["alert"]["lifecycle_state"], "triggered");
    assert_eq!(triggered.payload["matched_vps"][0]["id"], "edge-a");

    let resolved_event = lifecycle_event("alert.resolved", "resolved", "resolved");
    let resolved = candidate(
        "alert.resolved && alert.category:job && alert.record_kind = event",
        &resolved_event,
    );
    assert_eq!(resolved.event_kind, "alert.resolved");
    assert_eq!(resolved.event_id, resolved_event.event_id);
    assert_eq!(resolved.payload["alert"]["lifecycle_state"], "resolved");
    assert_eq!(
        resolved.payload["alert"]["resolution_reason"],
        "operator_resolved"
    );
}

#[test]
fn subjectless_generic_alert_edges_match_only_event_and_alert_context() {
    let mut vps_rules = VpsRuleContext::default();
    insert_persisted_vps_rule(
        &mut vps_rules,
        "traffic.reset_day".to_string(),
        "15".to_string(),
        json!({"day": 15}),
    )
    .unwrap();
    let visible_vps = vec![
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
            retained_tombstone: false,
            vps_rules,
        },
        VpsRow {
            id: "untagged-a".to_string(),
            display_name: "untagged-a".to_string(),
            status: "online".to_string(),
            tags: Vec::new(),
            registration_ip: None,
            last_ip: None,
            last_seen_at: None,
            internal_build_number: 1,
            stale_since: None,
            stale_reason: None,
            capabilities: json!({}),
            retained_tombstone: false,
            vps_rules: VpsRuleContext::default(),
        },
    ];
    let event = EventRow {
        id: Uuid::from_u128(20),
        actor_id: None,
        kind: "alert.triggered".to_string(),
        event_id: "fleet-alert:global-job:triggered".to_string(),
        event_predicates: vec![
            "alert.triggered".to_string(),
            "alert.category:job".to_string(),
            "alert.severity:critical".to_string(),
        ],
        subject_client_ids: Vec::new(),
        payload: json!({
            "event": {
                "kind": "alert.triggered",
                "id": "fleet-alert:global-job:triggered",
            },
            "alert": {
                "id": "job:failed:global-job",
                "record_kind": "event",
                "producer_kind": "job",
                "lifecycle_state": "triggered",
                "severity": "critical",
                "category": "job",
                "target_kind": "job",
                "target_id": "global-job",
                "client_id": null,
            },
        }),
        occurred_at_unix: 1,
    };
    let candidate = |expression: &str, vps_rows: &[VpsRow]| {
        event_candidate_for_rule(
            &RuleRow {
                id: Uuid::nil(),
                actor_id: None,
                name: "global-alert".to_string(),
                expression: expression.to_string(),
                target: "https://hooks.acme.com/vpsman".to_string(),
                body_template: "{event.kind} {alert.id}".to_string(),
                cooldown_secs: 300,
            },
            &event,
            vps_rows,
        )
        .unwrap()
    };

    let empty_fleet = candidate(
        "alert.triggered && alert.category:job && alert.record_kind = event",
        &[],
    )
    .unwrap();
    assert!(empty_fleet.matched_vps.is_empty());
    assert_eq!(empty_fleet.payload["alert"]["client_id"], Value::Null);

    let visible_fleet = candidate(
        "alert.triggered && alert.category:job && alert.record_kind = event",
        &visible_vps,
    )
    .unwrap();
    assert!(visible_fleet.matched_vps.is_empty());
    assert_eq!(visible_fleet.payload["matched_vps"], json!([]));

    for expression in [
        "alert.triggered && vps.id = edge-a",
        "alert.triggered && status = online",
        "alert.triggered && tag:edge",
        "alert.triggered && vps.rules:traffic.reset_day >= 1",
        "alert.triggered && edge",
        "alert.triggered && untagged",
        "alert.triggered && !(status = offline)",
    ] {
        assert!(
            candidate(expression, &visible_vps).is_none(),
            "VPS-dependent expression must fail closed: {expression}"
        );
    }
}

async fn insert_webhook_test_client(pool: &PgPool, client_id: &str, status: &str, hidden: bool) {
    sqlx::query(
        r#"
        INSERT INTO clients (
            id, display_name, public_key, status, internal_build_number,
            capabilities, hidden_at
        )
        VALUES ($1, $1, decode('', 'hex'), $2, 1, '{}'::jsonb,
            CASE WHEN $3 THEN now() ELSE NULL END)
        "#,
    )
    .bind(client_id)
    .bind(status)
    .bind(hidden)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_webhook_test_rule(pool: &PgPool, name: &str, expression: &str) -> Uuid {
    let rule_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO webhook_rules (
            id, name, enabled, expression, target, body_template, cooldown_secs
        )
        VALUES ($1, $2, TRUE, $3, 'https://hooks.example.invalid/vpsman', '', 0)
        "#,
    )
    .bind(rule_id)
    .bind(name)
    .bind(expression)
    .execute(pool)
    .await
    .unwrap();
    rule_id
}
