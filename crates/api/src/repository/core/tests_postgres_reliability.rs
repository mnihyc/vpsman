use std::{collections::BTreeMap, fs, path::Path, str::FromStr, time::Duration};

use axum::http::{header::AUTHORIZATION, HeaderMap};
use chrono::{Datelike, Utc};
use serde_json::{json, Value};
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    types::Json as SqlJson,
    PgPool, Row,
};
use tokio::sync::{broadcast, oneshot};
use uuid::Uuid;
use vpsman_common::{
    pair_port_expressions, parse_expression, payload_hash, plan_tunnel, validate_template,
    AgentCapabilitySnapshot, AgentHello, AgentMetrics, AgentRuntimeConfigReloadRequest,
    AgentUpdateHeartbeat, CommandOutput, CpuStat, DiskStat, GatewayAgentHelloIngest,
    GatewayRuntimeConfigReloadRequest, GatewayTelemetryIngest, JobCommand, LoadAverage, MemoryStat,
    NetworkStat, NetworkTrafficImportBucket, NetworkTrafficImportResult,
    NetworkTrafficImportSource, OspfControlMode, OspfCostPolicy, OutputStream, PingTargetResult,
    PortForwardProtocol, PortForwardRuntimeSnapshot, PortForwardRuntimeStatus,
    RuntimeTunnelControl, RuntimeTunnelManager, RuntimeTunnelStat, TelemetryEnvelope,
    TunnelAddressFamily, TunnelAddressPair, TunnelEndpointSide, TunnelKind, TunnelOspfConfig,
    TunnelPlanInput, TunnelReachabilityObservation, TunnelReachabilitySource,
};

#[tokio::test]
async fn postgres_alert_expression_canonicalization_is_atomic_audited_and_idempotent() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let rule_id = Uuid::new_v4();
    let event_row_id = Uuid::new_v4();
    let prior_expression = "alert.policy_reached && policy_rule.window_secs = 0";
    let prior_template = "[if alert.policy_reached]{policy_rule.condition_expression}[endif]";
    sqlx::query(
        r#"
        INSERT INTO webhook_rules (
            id, name, expression, target, body_template
        ) VALUES ($1, 'legacy policy lifecycle rule', $2,
                  'https://hooks.example.invalid/policy', $3)
        "#,
    )
    .bind(rule_id)
    .bind(prior_expression)
    .bind(prior_template)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO webhook_events (
            id, kind, event_id, event_predicates, subject_client_ids, payload
        ) VALUES (
            $1, 'alert.policy_reached', 'policy-alert:legacy:triggered',
            ARRAY['alert.policy_reached','alert.open']::text[], ARRAY[]::text[],
            jsonb_build_object(
                'event', jsonb_build_object(
                    'kind', 'alert.policy_reached',
                    'predicates', jsonb_build_array('alert.policy_reached','alert.open')
                ),
                'rule', jsonb_build_object(
                    'id', '11111111-2222-4333-8444-555555555555',
                    'name', 'legacy resource threshold',
                    'rule_version', 4,
                    'condition_expression', 'cpu.load_1 >= 3',
                    'window_secs', 300
                )
            )
        )
        "#,
    )
    .bind(event_row_id)
    .execute(&db.pool)
    .await
    .unwrap();

    db.repo
        .canonicalize_alert_event_expressions()
        .await
        .unwrap();
    let canonical_rule = sqlx::query_as::<_, (String, String)>(
        "SELECT expression, body_template FROM webhook_rules WHERE id=$1",
    )
    .bind(rule_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    parse_expression(&canonical_rule.0).unwrap().unwrap();
    validate_template(&canonical_rule.1).unwrap();
    assert!(!canonical_rule.0.contains("alert.policy_"));
    assert!(!canonical_rule.0.contains("window_secs"));
    assert!(!canonical_rule.1.contains("alert.policy_"));
    assert!(!canonical_rule.1.contains("condition_expression"));

    let audit = sqlx::query(
        r#"
        SELECT prior_expression, rewritten_expression,
               prior_body_template, rewritten_body_template,
               prior_expression_sha256, rewritten_expression_sha256,
               prior_body_template_sha256, rewritten_body_template_sha256
        FROM alert_expression_migration_audit
        WHERE rule_id=$1
        "#,
    )
    .bind(rule_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        audit.try_get::<String, _>("prior_expression").unwrap(),
        prior_expression
    );
    assert_eq!(
        audit.try_get::<String, _>("rewritten_expression").unwrap(),
        canonical_rule.0
    );
    assert_eq!(
        audit.try_get::<String, _>("prior_body_template").unwrap(),
        prior_template
    );
    assert_eq!(
        audit
            .try_get::<String, _>("rewritten_body_template")
            .unwrap(),
        canonical_rule.1
    );
    for (column, value) in [
        ("prior_expression_sha256", prior_expression),
        ("rewritten_expression_sha256", canonical_rule.0.as_str()),
        ("prior_body_template_sha256", prior_template),
        ("rewritten_body_template_sha256", canonical_rule.1.as_str()),
    ] {
        assert_eq!(
            audit.try_get::<String, _>(column).unwrap(),
            payload_hash(value.as_bytes())
        );
    }

    let event =
        sqlx::query("SELECT kind, event_predicates, payload FROM webhook_events WHERE id=$1")
            .bind(event_row_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        event.try_get::<String, _>("kind").unwrap(),
        "alert.triggered"
    );
    assert_eq!(
        event.try_get::<Vec<String>, _>("event_predicates").unwrap(),
        vec!["alert.triggered".to_string()]
    );
    let payload = event.try_get::<SqlJson<Value>, _>("payload").unwrap().0;
    assert_eq!(payload["event"]["kind"], "alert.triggered");
    assert_eq!(payload["event"]["predicates"], json!(["alert.triggered"]));
    assert!(payload.get("rule").is_none());
    assert_eq!(
        payload["policy_rule"]["trigger_condition_expression"],
        "cpu.load_1 >= 3"
    );
    assert_eq!(
        payload["policy_rule"]["trigger_meta_condition"],
        json!({"kind":"sustained","window_seconds":300})
    );
    assert_eq!(payload["policy_rule"]["rule_kind"], "metric");
    assert_eq!(
        payload["policy_rule"]["evidence_source"],
        "telemetry.combined"
    );
    let completed_before: chrono::DateTime<Utc> = sqlx::query_scalar(
        "SELECT completed_at FROM alert_expression_migration_meta WHERE singleton",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();

    db.repo
        .canonicalize_alert_event_expressions()
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM alert_expression_migration_audit")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, chrono::DateTime<Utc>>(
            "SELECT completed_at FROM alert_expression_migration_meta WHERE singleton",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        completed_before
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_alert_expression_canonicalization_refuses_nonterminal_deliveries_atomically() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let rule_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO webhook_rules (id, name, expression, target)
        VALUES ($1, 'blocked legacy rule', 'alert.policy_resolved',
                'https://hooks.example.invalid/policy')
        "#,
    )
    .bind(rule_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO webhook_rule_deliveries (
            id, rule_id, rule_name, event_kind, event_id, status, target,
            dedupe_key, payload, matched_vps, message, cooldown_until_unix
        ) VALUES (
            $1, $2, 'blocked legacy rule', 'alert.policy_resolved',
            'policy-alert:blocked:resolved', 'queued',
            'https://hooks.example.invalid/policy', 'blocked-legacy-delivery',
            '{}'::jsonb, '[]'::jsonb, 'legacy body', 0
        )
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(rule_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let error = db
        .repo
        .canonicalize_alert_event_expressions()
        .await
        .unwrap_err()
        .to_string();
    assert!(error
        .contains("requires queued, in-progress, and retryable webhook deliveries to be drained"));
    assert!(sqlx::query_scalar::<_, Option<chrono::DateTime<Utc>>>(
        "SELECT completed_at FROM alert_expression_migration_meta WHERE singleton",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap()
    .is_none());
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT expression FROM webhook_rules WHERE id=$1")
            .bind(rule_id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        "alert.policy_resolved"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM alert_expression_migration_audit")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        0
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_policy_startup_drains_every_event_source_past_each_batch_limit() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "startup-event-source-drain";
    insert_client(&db.pool, client_id, Some(Uuid::new_v4())).await;

    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, command_type, privileged, status, target_count, payload_hash,
            request_fingerprint, max_timeout_secs, created_at, completed_at
        )
        SELECT md5('startup-terminal-' || series::text)::uuid,
               'shell', FALSE, 'failed', 0, repeat('a',64),
               'startup-terminal-' || series::text, 30,
               clock_timestamp(), clock_timestamp()
        FROM generate_series(1,201) series
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, command_type, privileged, status, target_count, payload_hash,
            request_fingerprint, max_timeout_secs, created_at, completed_at
        )
        SELECT md5('startup-capability-' || series::text)::uuid,
               'agent_update', TRUE, 'skipped', 1, repeat('b',64),
               'startup-capability-' || series::text, 30,
               clock_timestamp(), clock_timestamp()
        FROM generate_series(1,201) series
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO job_targets (
            job_id, client_id, status, message, completed_at,
            capability_degraded_reason, capability_degraded_hint
        )
        SELECT md5('startup-capability-' || series::text)::uuid,
               $1, 'skipped', 'capability unavailable', clock_timestamp(),
               'target_agent_lacks_capability',
               'Upgrade the agent before retrying.'
        FROM generate_series(1,201) series
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO backup_requests (
            id, client_id, paths, include_config, status, payload_hash,
            command_scope, created_at
        )
        SELECT md5('startup-backup-' || series::text)::uuid,
               $1, ARRAY['/srv/data'], TRUE, 'execution_failed', repeat('c',64),
               'client:' || $1, clock_timestamp()
        FROM generate_series(1,201) series
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();

    db.repo
        .reconcile_operational_alerts_startup()
        .await
        .unwrap();

    for (source, expected) in [
        ("job.terminal", 201_i64),
        ("backup.failure", 201_i64),
        ("job.capability", 201_i64),
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM alert_policy_evidence WHERE source_kind=$1",
            )
            .bind(source)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
            expected,
            "startup stopped before draining {source}"
        );
    }
    assert!(sqlx::query_scalar::<_, Option<chrono::DateTime<Utc>>>(
        "SELECT startup_reconciled_at FROM alert_policy_lifecycle_meta WHERE singleton",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap()
    .is_some());

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_scope_revision_repair_is_idempotent_and_startup_converges() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "scope-revision-repair";
    insert_client(&db.pool, client_id, Some(Uuid::new_v4())).await;

    let mut source = db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM clients WHERE id=$1 FOR UPDATE")
        .bind(client_id)
        .fetch_one(&mut *source)
        .await
        .unwrap();
    sqlx::query("UPDATE clients SET status='offline' WHERE id=$1")
        .bind(client_id)
        .execute(&mut *source)
        .await
        .unwrap();
    crate::repository_operational_alerts::reconcile_postgres_agent_alert_transition_in_tx(
        &mut source,
        client_id,
        "offline",
    )
    .await
    .unwrap();
    source.commit().await.unwrap();

    // Bump only selector/presentation metadata. The raw source boundary stays
    // immutable and one deterministic scope fact represents this revision.
    sqlx::query("UPDATE clients SET display_name='renamed scope client' WHERE id=$1")
        .bind(client_id)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(
        crate::repository_policy_lifecycle::repair_policy_scope_revision_evidence(&db.pool, 200)
            .await
            .unwrap()
            > 0
    );
    let scope_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM alert_policy_evidence WHERE source_event_id LIKE 'scope:%' AND subject_client_id=$1",
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        crate::repository_policy_lifecycle::repair_policy_scope_revision_evidence(&db.pool, 200)
            .await
            .unwrap(),
        0,
        "an already-emitted scope revision must not keep startup repair live"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM alert_policy_evidence WHERE source_event_id LIKE 'scope:%' AND subject_client_id=$1",
        )
        .bind(client_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        scope_count
    );

    db.repo
        .reconcile_operational_alerts_startup()
        .await
        .unwrap();
    assert!(sqlx::query_scalar::<_, Option<chrono::DateTime<Utc>>>(
        "SELECT startup_reconciled_at FROM alert_policy_lifecycle_meta WHERE singleton",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap()
    .is_some());

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_occurrence_count_windows_use_db_acceptance_not_source_clock() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "count-acceptance-clock";
    insert_client(&db.pool, client_id, Some(Uuid::new_v4())).await;
    let rule_id = insert_typed_policy_rule_fixture(
        &db.pool,
        client_id,
        "occurrence",
        "backup.failure",
        "subject",
        "evidence.status = execution_failed",
        Some(json!({"kind":"count","confirmations":3,"within_seconds":60})),
        None,
        Some(json!({"kind":"elapsed_since_trigger","seconds":3600})),
        "backup",
    )
    .await;

    for (index, observed_at) in [
        Utc::now() - chrono::Duration::days(365),
        Utc::now() + chrono::Duration::days(365),
    ]
    .into_iter()
    .enumerate()
    {
        record_test_policy_fact(
            &db.pool,
            crate::repository_policy_lifecycle::PolicyEvidenceFact {
                source_kind: "backup.failure".to_string(),
                source_event_id: format!("acceptance-clock-{index}"),
                fact_kind: AlertPolicyRuleKind::Occurrence,
                natural_key: format!("backup-{index}"),
                confirmation_bucket_key: format!("backup-{index}"),
                subject_client_id: Some(client_id.to_string()),
                target_kind: "backup_request".to_string(),
                target_id: format!("backup-{index}"),
                source_status: "execution_failed".to_string(),
                complete: true,
                subject_snapshot: json!({}),
                payload: json!({
                    "status":"execution_failed",
                    "backup_request_id":format!("backup-{index}"),
                    "client_id":client_id,
                }),
                observed_at,
                state_started_at: None,
                causation_id: None,
                schedule_lineage: Vec::new(),
            },
        )
        .await;
    }

    assert_eq!(
        crate::repository_policy_lifecycle::repair_missing_policy_evidence_receipts(&db.pool, 100)
            .await
            .unwrap(),
        0,
        "source-transaction evaluation must not leave repair work"
    );

    let confirmations: (i64, bool) = sqlx::query_as(
        r#"
        SELECT count(*), bool_and(abs(extract(epoch FROM (confirmation.accepted_at - clock_timestamp()))) < 10)
        FROM alert_policy_confirmations confirmation
        WHERE confirmation.policy_rule_id=$1 AND confirmation.phase='trigger'
        "#,
    )
    .bind(rule_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(confirmations, (2, true));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM alert_episodes WHERE policy_rule_id=$1",
        )
        .bind(rule_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0,
        "two accepted facts must remain below the three-confirmation gate"
    );

    let trigger_causation_id = Uuid::new_v4();
    let mut trigger_lineage = (0..16).map(|_| Uuid::new_v4()).collect::<Vec<_>>();
    trigger_lineage.sort_unstable();
    record_test_policy_fact(
        &db.pool,
        crate::repository_policy_lifecycle::PolicyEvidenceFact {
            source_kind: "backup.failure".to_string(),
            source_event_id: "acceptance-clock-2".to_string(),
            fact_kind: AlertPolicyRuleKind::Occurrence,
            natural_key: "backup-2".to_string(),
            confirmation_bucket_key: "backup-2".to_string(),
            subject_client_id: Some(client_id.to_string()),
            target_kind: "backup_request".to_string(),
            target_id: "backup-2".to_string(),
            source_status: "execution_failed".to_string(),
            complete: true,
            subject_snapshot: json!({}),
            payload: json!({
                "status":"execution_failed",
                "backup_request_id":"backup-2",
                "client_id":client_id,
            }),
            observed_at: Utc::now(),
            state_started_at: None,
            causation_id: Some(trigger_causation_id),
            schedule_lineage: trigger_lineage.clone(),
        },
    )
    .await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM alert_episodes WHERE policy_rule_id=$1 AND last_confirmed_at IS NOT NULL",
        )
        .bind(rule_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            r#"
            SELECT count(*)
            FROM alert_lifecycle_events event
            JOIN alert_episodes episode ON episode.id = event.episode_id
            WHERE episode.policy_rule_id=$1 AND event.edge_kind='alert.triggered'
            "#,
        )
        .bind(rule_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        1
    );

    // A later matching fact may refresh presentation/provenance, but must not
    // erase the immutable N-fact confirmation snapshot which opened the
    // Count episode.
    record_test_policy_fact(
        &db.pool,
        crate::repository_policy_lifecycle::PolicyEvidenceFact {
            source_kind: "backup.failure".to_string(),
            source_event_id: "acceptance-clock-3".to_string(),
            fact_kind: AlertPolicyRuleKind::Occurrence,
            natural_key: "backup-3".to_string(),
            confirmation_bucket_key: "backup-3".to_string(),
            subject_client_id: Some(client_id.to_string()),
            target_kind: "backup_request".to_string(),
            target_id: "backup-3".to_string(),
            source_status: "execution_failed".to_string(),
            complete: true,
            subject_snapshot: json!({}),
            payload: json!({
                "status":"execution_failed",
                "backup_request_id":"backup-3",
                "client_id":client_id,
            }),
            observed_at: Utc::now(),
            state_started_at: None,
            causation_id: Some(trigger_causation_id),
            schedule_lineage: trigger_lineage.clone(),
        },
    )
    .await;
    let confirmation_snapshot_lengths: (i32, i32) = sqlx::query_as(
        r#"
        SELECT jsonb_array_length(evidence->'confirmation_evidence'),
               jsonb_array_length(
                   evidence->'trigger_evidence_snapshot'->'confirmation_evidence'
               )
        FROM alert_episodes
        WHERE policy_rule_id=$1 AND resolved_at IS NULL
        "#,
    )
    .bind(rule_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(confirmation_snapshot_lengths, (3, 3));

    // A nonmatching fact shares the global/subject state bucket but is not
    // episode provenance. Elapsed resolution must retain the episode's exact
    // 16-entry lineage and causation rather than overflowing with this fact.
    let unrelated_lineage = Uuid::new_v4();
    record_test_policy_fact(
        &db.pool,
        crate::repository_policy_lifecycle::PolicyEvidenceFact {
            source_kind: "backup.failure".to_string(),
            source_event_id: "acceptance-clock-nonmatch".to_string(),
            fact_kind: AlertPolicyRuleKind::Occurrence,
            natural_key: "backup-nonmatch".to_string(),
            confirmation_bucket_key: "backup-nonmatch".to_string(),
            subject_client_id: Some(client_id.to_string()),
            target_kind: "backup_request".to_string(),
            target_id: "backup-nonmatch".to_string(),
            source_status: "completed".to_string(),
            complete: true,
            subject_snapshot: json!({}),
            payload: json!({
                "status":"completed",
                "backup_request_id":"backup-nonmatch",
                "client_id":client_id,
            }),
            observed_at: Utc::now(),
            state_started_at: None,
            causation_id: Some(unrelated_lineage),
            schedule_lineage: vec![unrelated_lineage],
        },
    )
    .await;
    sqlx::query(
        "UPDATE alert_episodes SET triggered_at=clock_timestamp()-interval '3601 seconds' WHERE policy_rule_id=$1 AND resolved_at IS NULL",
    )
    .bind(rule_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE alert_policy_evaluation_states SET next_transition_at=clock_timestamp()-interval '1 second' WHERE policy_rule_id=$1",
    )
    .bind(rule_id)
    .execute(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        crate::repository_policy_lifecycle::evaluate_due_policy_transitions(&db.pool, 10)
            .await
            .unwrap(),
        1
    );
    let resolved: (Vec<Uuid>, Option<Uuid>, Uuid) = sqlx::query_as(
        r#"
        SELECT event.schedule_lineage, event.causation_id, state.last_evidence_id
        FROM alert_lifecycle_events event
        JOIN alert_episodes episode ON episode.id=event.episode_id
        JOIN alert_policy_evaluation_states state
          ON state.policy_rule_id=episode.policy_rule_id
         AND state.rule_version=episode.policy_rule_version
        WHERE episode.policy_rule_id=$1 AND event.edge_kind='alert.resolved'
        "#,
    )
    .bind(rule_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let nonmatching_evidence_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM alert_policy_evidence WHERE source_event_id='acceptance-clock-nonmatch'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(resolved.0, trigger_lineage);
    assert_eq!(resolved.1, Some(trigger_causation_id));
    assert_eq!(resolved.2, nonmatching_evidence_id);
    assert!(!resolved.0.contains(&unrelated_lineage));

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_metric_sustained_requires_fresh_revisions_at_both_boundaries() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "metric-fresh-sustained";
    insert_client(&db.pool, client_id, Some(Uuid::new_v4())).await;
    let rule_id = insert_typed_policy_rule_fixture(
        &db.pool,
        client_id,
        "metric",
        "telemetry.combined",
        "natural_key",
        "cpu.utilization_ratio >= 0.75",
        Some(json!({"kind":"sustained","seconds":60})),
        Some("cpu.utilization_ratio < 0.50"),
        Some(json!({"kind":"sustained","seconds":60})),
        "resource",
    )
    .await;

    record_test_metric_fact(&db.pool, client_id, "metric-high-1", 0.9).await;
    sqlx::query(
        r#"
        UPDATE alert_policy_evaluation_states
        SET trigger_segment_started_at=clock_timestamp()-interval '61 seconds',
            next_transition_at=clock_timestamp()-interval '1 second'
        WHERE policy_rule_id=$1
        "#,
    )
    .bind(rule_id)
    .execute(&db.pool)
    .await
    .unwrap();
    crate::repository_policy_lifecycle::evaluate_due_policy_transitions(&db.pool, 10)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM alert_episodes WHERE policy_rule_id=$1",
        )
        .bind(rule_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0,
        "one old high metric sample must not mature on a timer tick"
    );
    record_test_metric_fact(&db.pool, client_id, "metric-high-2", 0.9).await;
    let episode_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM alert_episodes WHERE policy_rule_id=$1 AND resolved_at IS NULL",
    )
    .bind(rule_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    record_test_metric_fact(&db.pool, client_id, "metric-low-1", 0.2).await;
    sqlx::query(
        r#"
        UPDATE alert_policy_evaluation_states
        SET resolve_segment_started_at=clock_timestamp()-interval '61 seconds',
            next_transition_at=clock_timestamp()-interval '1 second'
        WHERE policy_rule_id=$1
        "#,
    )
    .bind(rule_id)
    .execute(&db.pool)
    .await
    .unwrap();
    crate::repository_policy_lifecycle::evaluate_due_policy_transitions(&db.pool, 10)
        .await
        .unwrap();
    assert!(sqlx::query_scalar::<_, Option<chrono::DateTime<Utc>>>(
        "SELECT resolved_at FROM alert_episodes WHERE id=$1",
    )
    .bind(episode_id)
    .fetch_one(&db.pool)
    .await
    .unwrap()
    .is_none());

    record_test_metric_fact(&db.pool, client_id, "metric-low-2", 0.2).await;
    assert!(sqlx::query_scalar::<_, Option<chrono::DateTime<Utc>>>(
        "SELECT resolved_at FROM alert_episodes WHERE id=$1",
    )
    .bind(episode_id)
    .fetch_one(&db.pool)
    .await
    .unwrap()
    .is_some());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM alert_lifecycle_events WHERE episode_id=$1 AND edge_kind='alert.resolved'",
        )
        .bind(episode_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        1
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_policy_edit_and_delete_drain_old_version_occurrences_at_the_arm_fence() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "definition-fence-occurrence";
    insert_client(&db.pool, client_id, Some(Uuid::new_v4())).await;
    let operator = postgres_network_operator(&db.repo).await;
    let created = db
        .repo
        .upsert_fleet_alert_policy(
            &CreateFleetAlertPolicyRequest {
                id: None,
                name: "definition fence occurrence".to_string(),
                enabled: true,
                selector_expression: format!("id:{client_id}"),
                rules: vec![postgres_backup_failure_rule_request(None, "warning")],
                notes: None,
                confirmed: true,
                preview_hash: None,
            },
            &operator,
        )
        .await
        .unwrap();
    let rule_id = created.rules[0].id;
    let first_seq = insert_unprocessed_backup_failure_evidence(
        &db.pool,
        client_id,
        "definition-fence-before-edit",
    )
    .await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM alert_policy_evidence_receipts WHERE policy_rule_id=$1 AND evidence_seq=$2",
        )
        .bind(rule_id)
        .bind(first_seq)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0
    );

    let edited = db
        .repo
        .upsert_fleet_alert_policy(
            &CreateFleetAlertPolicyRequest {
                id: Some(created.id),
                name: created.name.clone(),
                enabled: true,
                selector_expression: created.selector_expression.clone(),
                rules: vec![postgres_backup_failure_rule_request(
                    Some(rule_id),
                    "critical",
                )],
                notes: None,
                confirmed: true,
                preview_hash: None,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(edited.rules[0].rule_version, 2);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT result FROM alert_policy_evidence_receipts WHERE policy_rule_id=$1 AND rule_version=1 AND evidence_seq=$2",
        )
        .bind(rule_id)
        .bind(first_seq)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        "matched"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM alert_policy_evidence_receipts WHERE policy_rule_id=$1 AND rule_version=2 AND evidence_seq=$2 AND result='pre_armed'",
        )
        .bind(rule_id)
        .bind(first_seq)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0
    );

    insert_unprocessed_backup_failure_evidence(
        &db.pool,
        client_id,
        "definition-fence-before-delete",
    )
    .await;
    db.repo
        .delete_fleet_alert_policy(edited.id, &edited.name, &operator)
        .await
        .unwrap();
    let reasons: Vec<String> = sqlx::query_scalar(
        "SELECT resolution_reason FROM alert_episodes WHERE policy_rule_id=$1 ORDER BY triggered_at",
    )
    .bind(rule_id)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        reasons,
        vec!["policy_changed".to_string(), "policy_deleted".to_string()]
    );
    let edge_counts: (i64, i64) = sqlx::query_as(
        r#"
        SELECT count(*) FILTER (WHERE event.edge_kind='alert.triggered'),
               count(*) FILTER (WHERE event.edge_kind='alert.resolved')
        FROM alert_lifecycle_events event
        JOIN alert_episodes episode ON episode.id=event.episode_id
        WHERE episode.policy_rule_id=$1
        "#,
    )
    .bind(rule_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(edge_counts, (2, 2));

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_source_exit_is_evidence_owned_before_a_later_rule_baseline() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "source-exit-before-rule";
    let natural_key = "tunnel-plan:removed-before-rule";
    insert_client(&db.pool, client_id, Some(Uuid::new_v4())).await;
    let observed_at = Utc::now();
    record_test_policy_fact(
        &db.pool,
        crate::repository_policy_lifecycle::PolicyEvidenceFact {
            source_kind: "tunnel.adapter".to_string(),
            source_event_id: "source-exit-present-fact".to_string(),
            fact_kind: AlertPolicyRuleKind::State,
            natural_key: natural_key.to_string(),
            confirmation_bucket_key: natural_key.to_string(),
            subject_client_id: Some(client_id.to_string()),
            target_kind: "tunnel_plan".to_string(),
            target_id: "removed-before-rule".to_string(),
            source_status: "ok".to_string(),
            complete: true,
            subject_snapshot: json!({}),
            payload: json!({
                "status":"ok",
                "client_id":client_id,
                "tunnel_plan_id":"removed-before-rule",
                "adapter":{"success":true,"interface":"wg0","reason":null},
            }),
            observed_at,
            state_started_at: Some(observed_at),
            causation_id: None,
            schedule_lineage: Vec::new(),
        },
    )
    .await;
    let mut tx = db.pool.begin().await.unwrap();
    assert_eq!(
        crate::repository_policy_lifecycle::record_policy_source_scope_exits_in_tx(
            &mut tx,
            &["tunnel.adapter"],
            &[client_id.to_string()],
            &std::collections::BTreeSet::new(),
        )
        .await
        .unwrap(),
        1,
        "a removed source identity must append an exit fact without an active gate"
    );
    tx.commit().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, Option<bool>>(
            r#"
            SELECT (payload->>'source_present')::boolean
            FROM alert_policy_evidence
            WHERE source_kind='tunnel.adapter' AND natural_key=$1
            ORDER BY observed_at DESC,evidence_seq DESC
            LIMIT 1
            "#,
        )
        .bind(natural_key)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        Some(false)
    );

    let rule_id = insert_typed_policy_rule_fixture(
        &db.pool,
        client_id,
        "state",
        "tunnel.adapter",
        "natural_key",
        "evidence.adapter.success = true",
        None,
        None,
        None,
        "network",
    )
    .await;
    sqlx::query(
        r#"
        UPDATE policy_rules
        SET armed_after_evidence_seq=(SELECT max(evidence_seq) FROM alert_policy_evidence),
            armed_at=clock_timestamp()
        WHERE id=$1
        "#,
    )
    .bind(rule_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let mut tx = db.pool.begin().await.unwrap();
    crate::repository_policy_lifecycle::evaluate_policy_rule_baselines_in_tx(&mut tx, &[rule_id])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM alert_episodes WHERE policy_rule_id=$1",
        )
        .bind(rule_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0,
        "a later baseline must not resurrect the removed present fact"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_due_state_defers_a_newer_unreceipted_recovery_fact() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "due-newer-recovery";
    insert_client(&db.pool, client_id, Some(Uuid::new_v4())).await;
    let rule_id = insert_typed_policy_rule_fixture(
        &db.pool,
        client_id,
        "state",
        "agent.status",
        "natural_key",
        "evidence.status = offline",
        Some(json!({"kind":"sustained","seconds":60})),
        None,
        None,
        "agent_status",
    )
    .await;
    let offline_at = Utc::now();
    record_test_policy_fact(
        &db.pool,
        crate::repository_policy_lifecycle::PolicyEvidenceFact {
            source_kind: "agent.status".to_string(),
            source_event_id: "due-offline".to_string(),
            fact_kind: AlertPolicyRuleKind::State,
            natural_key: client_id.to_string(),
            confirmation_bucket_key: client_id.to_string(),
            subject_client_id: Some(client_id.to_string()),
            target_kind: "client".to_string(),
            target_id: client_id.to_string(),
            source_status: "offline".to_string(),
            complete: true,
            subject_snapshot: json!({}),
            payload: json!({"status":"offline","client_id":client_id}),
            observed_at: offline_at,
            state_started_at: Some(offline_at),
            causation_id: None,
            schedule_lineage: Vec::new(),
        },
    )
    .await;
    sqlx::query(
        r#"
        UPDATE alert_policy_evaluation_states
        SET trigger_segment_started_at=clock_timestamp()-interval '61 seconds',
            next_transition_at=clock_timestamp()-interval '1 second'
        WHERE policy_rule_id=$1
        "#,
    )
    .bind(rule_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let online_evidence_seq: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO alert_policy_evidence (
            id,source_kind,source_event_id,fact_kind,natural_key,
            confirmation_bucket_key,subject_client_id,target_kind,target_id,
            source_status,completeness,subject_snapshot,payload,observed_at,
            state_started_at,causation_id,schedule_lineage
        ) VALUES (
            $1,'agent.status','due-online-unreceipted','state',$2,$2,$2,
            'client',$2,'online','complete',$3,$4,clock_timestamp(),
            clock_timestamp(),NULL,ARRAY[]::uuid[]
        )
        RETURNING evidence_seq
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(client_id)
    .bind(SqlJson(json!({
        "scope_complete":true,
        "scope_revision":1,
        "client_id":client_id,
        "display_name":client_id,
        "status":"online",
        "tags":[],
        "vps_rules":{},
    })))
    .bind(SqlJson(json!({"status":"online","client_id":client_id})))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        crate::repository_policy_lifecycle::evaluate_due_policy_transitions(&db.pool, 10)
            .await
            .unwrap(),
        0,
        "due evaluation must linearize after and defer to the committed recovery fact"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM alert_episodes WHERE policy_rule_id=$1",
        )
        .bind(rule_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0
    );
    assert!(
        crate::repository_policy_lifecycle::repair_missing_policy_evidence_receipts(&db.pool, 100)
            .await
            .unwrap()
            > 0
    );
    let state: (String, Option<chrono::DateTime<Utc>>, i64) = sqlx::query_as(
        r#"
        SELECT truth_state,next_transition_at,last_evidence_seq
        FROM alert_policy_evaluation_states
        WHERE policy_rule_id=$1
        "#,
    )
    .bind(rule_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        state,
        ("not_matched".to_string(), None, online_evidence_seq)
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_due_state_defers_a_newer_subject_scope_revision() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "due-newer-scope";
    insert_client(&db.pool, client_id, Some(Uuid::new_v4())).await;
    let tag_id = Uuid::new_v4();
    sqlx::query("INSERT INTO tags (id,name,display_order) VALUES ($1,'armed',0)")
        .bind(tag_id)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO client_tags (client_id,tag_id) VALUES ($1,$2)")
        .bind(client_id)
        .bind(tag_id)
        .execute(&db.pool)
        .await
        .unwrap();
    let rule_id = insert_typed_policy_rule_fixture(
        &db.pool,
        client_id,
        "state",
        "agent.status",
        "natural_key",
        "evidence.status = offline",
        Some(json!({"kind":"sustained","seconds":60})),
        None,
        None,
        "agent_status",
    )
    .await;
    sqlx::query(
        "UPDATE policy_groups SET selector_expression='tag:armed' WHERE id=(SELECT group_id FROM policy_rules WHERE id=$1)",
    )
    .bind(rule_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let observed_at = Utc::now();
    record_test_policy_fact(
        &db.pool,
        crate::repository_policy_lifecycle::PolicyEvidenceFact {
            source_kind: "agent.status".to_string(),
            source_event_id: "due-scope-offline".to_string(),
            fact_kind: AlertPolicyRuleKind::State,
            natural_key: client_id.to_string(),
            confirmation_bucket_key: client_id.to_string(),
            subject_client_id: Some(client_id.to_string()),
            target_kind: "client".to_string(),
            target_id: client_id.to_string(),
            source_status: "offline".to_string(),
            complete: true,
            subject_snapshot: json!({}),
            payload: json!({"status":"offline","client_id":client_id}),
            observed_at,
            state_started_at: Some(observed_at),
            causation_id: None,
            schedule_lineage: Vec::new(),
        },
    )
    .await;
    sqlx::query(
        r#"
        UPDATE alert_policy_evaluation_states
        SET trigger_segment_started_at=clock_timestamp()-interval '61 seconds',
            next_transition_at=clock_timestamp()-interval '1 second'
        WHERE policy_rule_id=$1
        "#,
    )
    .bind(rule_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM client_tags WHERE client_id=$1 AND tag_id=$2")
        .bind(client_id)
        .bind(tag_id)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        crate::repository_policy_lifecycle::evaluate_due_policy_transitions(&db.pool, 10)
            .await
            .unwrap(),
        0,
        "a due timer must not cross a newer selector revision"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM alert_episodes WHERE policy_rule_id=$1",
        )
        .bind(rule_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0
    );
    let mut tx = db.pool.begin().await.unwrap();
    crate::repository_policy_lifecycle::record_policy_scope_revision_evidence_for_clients_in_tx(
        &mut tx,
        &[client_id.to_string()],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT truth_state FROM alert_policy_evaluation_states WHERE policy_rule_id=$1",
        )
        .bind(rule_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        "not_matched"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_resolve_count_preserves_its_confirmation_evidence_snapshot() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "resolve-count-snapshot";
    insert_client(&db.pool, client_id, Some(Uuid::new_v4())).await;
    let rule_id = insert_typed_policy_rule_fixture(
        &db.pool,
        client_id,
        "state",
        "agent.status",
        "natural_key",
        "evidence.status = offline",
        None,
        Some("evidence.status = online"),
        Some(json!({"kind":"count","confirmations":2,"within_seconds":60})),
        "agent_status",
    )
    .await;
    for (index, status) in ["offline", "online", "online"].into_iter().enumerate() {
        let observed_at = Utc::now() + chrono::Duration::milliseconds(index as i64);
        record_test_policy_fact(
            &db.pool,
            crate::repository_policy_lifecycle::PolicyEvidenceFact {
                source_kind: "agent.status".to_string(),
                source_event_id: format!("resolve-count-{index}"),
                fact_kind: AlertPolicyRuleKind::State,
                natural_key: client_id.to_string(),
                confirmation_bucket_key: client_id.to_string(),
                subject_client_id: Some(client_id.to_string()),
                target_kind: "client".to_string(),
                target_id: client_id.to_string(),
                source_status: status.to_string(),
                complete: true,
                subject_snapshot: json!({}),
                payload: json!({"status":status,"client_id":client_id}),
                observed_at,
                state_started_at: Some(observed_at),
                causation_id: None,
                schedule_lineage: Vec::new(),
            },
        )
        .await;
    }
    let snapshot_lengths: (i32, i32) = sqlx::query_as(
        r#"
        SELECT jsonb_array_length(evidence->'resolution_confirmation_evidence'),
               jsonb_array_length(
                   evidence->'resolution_evidence_snapshot'->'confirmation_evidence'
               )
        FROM alert_episodes
        WHERE policy_rule_id=$1 AND resolved_at IS NOT NULL
        "#,
    )
    .bind(rule_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(snapshot_lengths, (2, 2));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM alert_policy_confirmations WHERE policy_rule_id=$1",
        )
        .bind(rule_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0,
        "resolved gate rows may be pruned only after their immutable snapshot is stored"
    );

    db.cleanup().await;
}

async fn insert_typed_policy_rule_fixture(
    pool: &PgPool,
    client_id: &str,
    rule_kind: &str,
    evidence_source: &str,
    correlation_mode: &str,
    trigger_expression: &str,
    trigger_meta: Option<Value>,
    resolve_expression: Option<&str>,
    resolve_meta: Option<Value>,
    category: &str,
) -> Uuid {
    let group_id = Uuid::new_v4();
    let rule_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO policy_groups (id,name,enabled,selector_expression) VALUES ($1,$2,TRUE,$3)",
    )
    .bind(group_id)
    .bind(format!("typed lifecycle fixture {rule_id}"))
    .bind(format!("id:{client_id}"))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO policy_rules (
            id,group_id,name,enabled,trigger_condition_expression,severity,
            rule_kind,evidence_source,correlation_mode,category,
            title_template,detail_template,trigger_meta_condition,
            resolve_condition_expression,resolve_meta_condition,
            armed_after_evidence_seq,armed_at
        ) VALUES (
            $1,$2,'typed lifecycle rule',TRUE,$3,'warning',
            $4,$5,$6,$7,'Typed lifecycle alert','Typed lifecycle detail',$8,$9,$10,
            0,clock_timestamp()
        )
        "#,
    )
    .bind(rule_id)
    .bind(group_id)
    .bind(trigger_expression)
    .bind(rule_kind)
    .bind(evidence_source)
    .bind(correlation_mode)
    .bind(category)
    .bind(trigger_meta.map(SqlJson))
    .bind(resolve_expression)
    .bind(resolve_meta.map(SqlJson))
    .execute(pool)
    .await
    .unwrap();
    rule_id
}

async fn record_test_policy_fact(
    pool: &PgPool,
    fact: crate::repository_policy_lifecycle::PolicyEvidenceFact,
) {
    let mut tx = pool.begin().await.unwrap();
    assert!(
        crate::repository_policy_lifecycle::record_policy_evidence_in_tx(&mut tx, fact)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
}

async fn record_test_metric_fact(pool: &PgPool, client_id: &str, event_id: &str, value: f64) {
    let observed_at = Utc::now();
    record_test_policy_fact(
        pool,
        crate::repository_policy_lifecycle::PolicyEvidenceFact {
            source_kind: "telemetry.combined".to_string(),
            source_event_id: event_id.to_string(),
            fact_kind: AlertPolicyRuleKind::Metric,
            natural_key: client_id.to_string(),
            confirmation_bucket_key: client_id.to_string(),
            subject_client_id: Some(client_id.to_string()),
            target_kind: "client".to_string(),
            target_id: client_id.to_string(),
            source_status: "complete".to_string(),
            complete: true,
            subject_snapshot: json!({}),
            payload: json!({"cpu":{"utilization_ratio":value}}),
            observed_at,
            state_started_at: Some(observed_at),
            causation_id: None,
            schedule_lineage: Vec::new(),
        },
    )
    .await;
}

async fn insert_unprocessed_backup_failure_evidence(
    pool: &PgPool,
    client_id: &str,
    source_event_id: &str,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO alert_policy_evidence (
            id,source_kind,source_event_id,fact_kind,natural_key,
            confirmation_bucket_key,subject_client_id,target_kind,target_id,
            source_status,completeness,subject_snapshot,payload,observed_at,
            causation_id,schedule_lineage
        ) VALUES (
            $1,'backup.failure',$2,'occurrence',$2,$2,$3,
            'backup_request',$2,'execution_failed','complete',$4,$5,
            clock_timestamp(),$6,ARRAY[$6]::uuid[]
        )
        RETURNING evidence_seq
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(source_event_id)
    .bind(client_id)
    .bind(SqlJson(json!({
        "scope_complete":true,
        "scope_revision":1,
        "client_id":client_id,
        "display_name":client_id,
        "status":"online",
        "tags":[],
        "vps_rules":{},
    })))
    .bind(SqlJson(json!({
        "status":"execution_failed",
        "backup_request_id":source_event_id,
        "client_id":client_id,
    })))
    .bind(Uuid::new_v4())
    .fetch_one(pool)
    .await
    .unwrap()
}
use vpsman_server_core::{
    JOB_STATUS_CANCELED, JOB_STATUS_COMPLETED, JOB_STATUS_CONTROL_TIMEOUT, JOB_STATUS_FAILED,
    JOB_STATUS_SKIPPED, TARGET_STATUS_AGENT_LOST, TARGET_STATUS_CANCELED, TARGET_STATUS_COMPLETED,
    TARGET_STATUS_CONTROL_TIMEOUT, TARGET_STATUS_FAILED, TARGET_STATUS_SKIPPED,
};

use crate::{
    gateway_client::GatewayDispatchClient,
    model::{
        AuthContext, BackupRequestStatus, BootstrapOperatorRequest, BulkTagMutationAction,
        BulkTagMutationRequest, ConfigurationOverrideAction, CreateBackupPolicyRequest,
        CreateBackupRequest, CreateConfigurationPresetRequest, CreateScheduleRequest,
        JobOutputView, JobRolloutPolicy, ListQuery, LoginRequest, NewServerArtifact,
        PingTargetRecord, PreviewConfigurationPresetRequest,
        PreviewConfigurationSourceOverrideRequest, RuntimeConfigOverrideCandidate,
        RuntimeConfigOverrideReplacement, SchedulePrivilegeMutationRequest, ScheduleTriggerKind,
        UpdateTagOrderRequest, UpsertAgentIdentityRequest,
        UpsertRuntimeConfigPatchGeneratorRequest, WsEvent,
    },
    model_alert_notifications::{
        CreateFleetAlertNotificationChannelRequest, FleetAlertNotificationCandidate,
    },
    model_alert_policies::{
        AlertPolicyCorrelationMode, AlertPolicyMetaCondition, AlertPolicyRuleKind,
        CreateFleetAlertPolicyRequest, NetworkRateInterfaceSelection, PolicyDryRunRequest,
        PolicyRuleRequest, VpsRuleQuery, VpsRulesBulkUnsetRequest, VpsRulesBulkUpsertRequest,
        VpsRulesDryRunRequest, VPS_RULE_KEY_NETWORK_PORT_SPEED, VPS_RULE_KEY_PRODUCT_NAME,
        VPS_RULE_KEY_TRAFFIC_RESET_DAY,
    },
    model_command_templates::UpsertCommandTemplateRequest,
    model_history::UpsertHistoryRetentionPolicyRequest,
    model_history::{HistoryDomain, HistoryRetentionPrunePlan},
    model_port_forwarding::{
        CreatePortForwardRuleRequest, UpdatePortForwardRuleRequest, UpdateTargetHostname,
    },
    model_terminal::TerminalSessionView,
    model_webhook_rules::{CreateWebhookRuleRequest, WebhookRuleDeliveryCandidate},
    repository::Repository,
    repository_alert_policies::NO_RESET_TRAFFIC_COUNTER_USAGE_SQL,
    repository_backups::BackupRequestSourceLink,
    repository_job_outputs::{JobOutputPersistConfig, JobOutputWriteResult},
    repository_network_observations::NetworkObservationFilter,
    repository_network_traffic_import::load_postgres_import_boundary_samples,
    repository_terminal_sessions::upsert_postgres_terminal_session,
    runtime_config_workspace::{preview_runtime_config_override, runtime_config_override_revision},
    state::{AppState, DispatcherRuntimeConfig, DEFAULT_ARTIFACT_MAX_BYTES},
};

#[tokio::test]
async fn postgres_single_host_ip_views_never_expose_prefix_lengths() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "plain-ip-projection";
    insert_client(&db.pool, client_id, None).await;
    sqlx::query(
        r#"
        UPDATE clients
        SET agent_version = 'postgres-test',
            registration_ip = '198.51.100.10/24'::inet,
            last_ip = '2001:db8::20/64'::inet
        WHERE id = $1
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO gateway_sessions (
            id, gateway_id, client_id, remote_ip, status
        )
        VALUES ($1, 'plain-ip-gateway', $2, '2001:db8::30/64'::inet, 'active')
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let agent = db
        .repo
        .list_agents()
        .await
        .unwrap()
        .into_iter()
        .find(|agent| agent.id == client_id)
        .unwrap();
    assert_eq!(agent.registration_ip.as_deref(), Some("198.51.100.10"));
    assert_eq!(agent.last_ip.as_deref(), Some("2001:db8::20"));

    let session = db
        .repo
        .list_gateway_sessions(10)
        .await
        .unwrap()
        .into_iter()
        .find(|session| session.client_id == client_id)
        .unwrap();
    assert_eq!(session.remote_ip.as_deref(), Some("2001:db8::30"));

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_deleted_delivery_owners_reject_stale_dispatch_snapshots() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let operator = postgres_network_operator(&db.repo).await;

    let channel_id = Uuid::new_v4();
    db.repo
        .upsert_fleet_alert_notification_channel(
            &CreateFleetAlertNotificationChannelRequest {
                id: Some(channel_id),
                name: "deleted-channel".to_string(),
                scope_kind: "global".to_string(),
                scope_value: None,
                min_severity: Some("warning".to_string()),
                categories: Some(vec!["agent_status".to_string()]),
                operator_states: Some(vec!["open".to_string()]),
                delivery_kind: "webhook".to_string(),
                target: "https://www.cloudflare.com/vpsman-test-fleet-webhook".to_string(),
                cooldown_secs: Some(60),
                enabled: Some(true),
                notes: None,
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    db.repo
        .delete_fleet_alert_notification_channel(channel_id, "deleted-channel", &operator)
        .await
        .unwrap();
    let notification_deliveries = db
        .repo
        .record_fleet_alert_notification_deliveries(
            &[FleetAlertNotificationCandidate {
                channel_id,
                channel_name: "deleted-channel".to_string(),
                alert_id: "agent_status:stale".to_string(),
                alert_severity: "critical".to_string(),
                alert_category: "agent_status".to_string(),
                status: "queued".to_string(),
                delivery_kind: "webhook".to_string(),
                target: "https://www.cloudflare.com/vpsman-test-fleet-webhook".to_string(),
                dedupe_key: "deleted-channel-stale-dispatch".to_string(),
                payload: serde_json::json!({"schema": "test"}),
                cooldown_until_unix: 0,
            }],
            &operator,
        )
        .await
        .unwrap();
    assert!(notification_deliveries.is_empty());

    let rule_id = Uuid::new_v4();
    db.repo
        .upsert_webhook_rule(
            &CreateWebhookRuleRequest {
                id: Some(rule_id),
                name: "deleted-rule".to_string(),
                enabled: true,
                expression: "interval.1min".to_string(),
                target: "https://www.cloudflare.com/vpsman-test-rule-webhook".to_string(),
                body_template: String::new(),
                signing_secret: None,
                clear_signing_secret: false,
                cooldown_secs: Some(60),
                notes: None,
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    db.repo
        .delete_webhook_rule(rule_id, "deleted-rule", &operator)
        .await
        .unwrap();
    let webhook_deliveries = db
        .repo
        .record_webhook_rule_deliveries(&[WebhookRuleDeliveryCandidate {
            rule_id,
            rule_name: "deleted-rule".to_string(),
            event_kind: "manual.test".to_string(),
            event_id: "deleted-rule-stale-event".to_string(),
            target: "https://www.cloudflare.com/vpsman-test-rule-webhook".to_string(),
            dedupe_key: "deleted-rule-stale-dispatch".to_string(),
            payload: serde_json::json!({"schema": "test"}),
            matched_vps: Vec::new(),
            message: "test".to_string(),
            rule_revision_hash: "deleted-rule-revision".to_string(),
            signing_secret: None,
            cooldown_until_unix: 0,
            actor_id: Some(operator.operator.id),
        }])
        .await
        .unwrap();
    assert!(webhook_deliveries.is_empty());

    db.cleanup().await;
}

struct PgReliabilityTestDb {
    repo: Repository,
    pool: PgPool,
    admin_pool: PgPool,
    db_name: String,
}

#[tokio::test]
async fn postgres_runtime_config_override_cas_is_atomic_and_supports_reset() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    for client_id in ["runtime-cas-a", "runtime-cas-b"] {
        insert_client(&db.pool, client_id, None).await;
    }
    let operator = postgres_network_operator(&db.repo).await;
    let absent_revision = runtime_config_override_revision(None);
    db.repo
        .replace_runtime_config_overrides_cas(
            &[
                RuntimeConfigOverrideReplacement {
                    client_id: "runtime-cas-a".to_string(),
                    expected_revision: absent_revision.clone(),
                    toml: Some("telemetry_interval_secs = 41\n".to_string()),
                },
                RuntimeConfigOverrideReplacement {
                    client_id: "runtime-cas-b".to_string(),
                    expected_revision: absent_revision.clone(),
                    toml: Some("telemetry_interval_secs = 42\n".to_string()),
                },
            ],
            "postgres-cas-seed",
            &operator,
        )
        .await
        .unwrap();
    let current = db.repo.list_runtime_config_overrides(None).await.unwrap();
    let current_a = current
        .iter()
        .find(|record| record.client_id == "runtime-cas-a")
        .unwrap();
    let current_b = current
        .iter()
        .find(|record| record.client_id == "runtime-cas-b")
        .unwrap();
    let revision_a = runtime_config_override_revision(Some(current_a));
    let revision_b = runtime_config_override_revision(Some(current_b));
    let retry_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with((*db.pool.connect_options()).clone())
        .await
        .unwrap();
    let retry_repo = Repository::Postgres(retry_pool.clone());

    let stale = db
        .repo
        .replace_runtime_config_overrides_cas(
            &[
                RuntimeConfigOverrideReplacement {
                    client_id: "runtime-cas-a".to_string(),
                    expected_revision: revision_a.clone(),
                    toml: Some("telemetry_interval_secs = 51\n".to_string()),
                },
                RuntimeConfigOverrideReplacement {
                    client_id: "runtime-cas-b".to_string(),
                    expected_revision: absent_revision,
                    toml: Some("telemetry_interval_secs = 52\n".to_string()),
                },
            ],
            "postgres-cas-stale",
            &operator,
        )
        .await
        .unwrap_err();
    assert!(stale.to_string().contains("review_stale"));
    let unchanged = retry_repo
        .list_runtime_config_overrides(None)
        .await
        .unwrap();
    assert_eq!(
        unchanged
            .iter()
            .find(|record| record.client_id == "runtime-cas-a")
            .unwrap()
            .toml,
        "telemetry_interval_secs = 41\n"
    );

    retry_repo
        .replace_runtime_config_overrides_cas(
            &[
                RuntimeConfigOverrideReplacement {
                    client_id: "runtime-cas-a".to_string(),
                    expected_revision: revision_a,
                    toml: None,
                },
                RuntimeConfigOverrideReplacement {
                    client_id: "runtime-cas-b".to_string(),
                    expected_revision: revision_b,
                    toml: None,
                },
            ],
            "postgres-cas-reset",
            &operator,
        )
        .await
        .unwrap();
    assert!(db
        .repo
        .list_runtime_config_overrides(None)
        .await
        .unwrap()
        .is_empty());
    drop(retry_repo);
    retry_pool.close().await;
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_runtime_config_guard_keeps_tunnel_source_stable_through_repreview_and_commit() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    for client_id in ["runtime-guard-pg-left", "runtime-guard-pg-right"] {
        insert_client(&db.pool, client_id, None).await;
    }
    db.repo
        .initialize_system_configuration_presets()
        .await
        .unwrap();
    let operator = postgres_network_operator(&db.repo).await;
    let mut input = postgres_alert_test_tunnel_input();
    input.name = "runtime-guard-pg".to_string();
    input.interface_name = "guardpg0".to_string();
    input.runtime_control = Default::default();
    input.left_mtu = vpsman_common::default_tunnel_mtu(TunnelKind::Gre);
    input.right_mtu = vpsman_common::default_tunnel_mtu(TunnelKind::Gre);
    input.left_client_id = "runtime-guard-pg-left".to_string();
    input.right_client_id = "runtime-guard-pg-right".to_string();
    input.address_pool_cidr = "10.92.0.0/30".to_string();
    input.ipv4_tunnel = Some(TunnelAddressPair {
        left: "10.92.0.0".to_string(),
        right: "10.92.0.1".to_string(),
        prefix_len: 31,
    });
    let planned = plan_tunnel(&input).unwrap();
    let persisted = db
        .repo
        .record_tunnel_plan(&input, &planned, true, &operator)
        .await
        .unwrap();
    let state = postgres_app_state(&db);
    let candidate = RuntimeConfigOverrideCandidate::Structured {
        value: serde_json::json!({"telemetry_interval_secs": 49}),
    };
    let reviewed = preview_runtime_config_override(&state, "runtime-guard-pg-left", &candidate)
        .await
        .unwrap();

    let guard = db.repo.lock_runtime_config_desired_state().await.unwrap();
    let writer_repo = db.repo.clone();
    let writer_input = input.clone();
    let writer_plan = planned.clone();
    let writer_operator = operator.clone();
    let (started_tx, started_rx) = oneshot::channel();
    let (finished_tx, mut finished_rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = started_tx.send(());
        let result = writer_repo
            .update_tunnel_plan(
                persisted.id,
                persisted.revision,
                &writer_input,
                &writer_plan,
                false,
                &writer_operator,
            )
            .await;
        let _ = finished_tx.send(result);
    });
    started_rx.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(150), &mut finished_rx)
            .await
            .is_err()
    );

    let locked_preview =
        preview_runtime_config_override(&state, "runtime-guard-pg-left", &candidate)
            .await
            .unwrap();
    assert_eq!(locked_preview.preview_hash, reviewed.preview_hash);
    db.repo
        .replace_runtime_config_overrides_cas_locked(
            guard,
            &[RuntimeConfigOverrideReplacement {
                client_id: "runtime-guard-pg-left".to_string(),
                expected_revision: locked_preview.override_revision,
                toml: locked_preview.canonical_toml,
            }],
            "postgres-guard-commit",
            &operator,
        )
        .await
        .unwrap();

    let updated = tokio::time::timeout(Duration::from_secs(2), finished_rx)
        .await
        .expect("tunnel writer should resume after override commit")
        .expect("tunnel writer result channel should stay open")
        .expect("tunnel writer should succeed");
    assert!(!updated.enabled);
    assert_eq!(
        db.repo
            .list_runtime_config_overrides(Some("runtime-guard-pg-left"))
            .await
            .unwrap()
            .len(),
        1
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_runtime_config_guard_rejects_single_connection_pool_without_waiting() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let single_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*db.pool.connect_options()).clone())
        .await
        .unwrap();
    let single_repo = Repository::Postgres(single_pool.clone());
    let result = tokio::time::timeout(
        Duration::from_millis(250),
        single_repo.lock_runtime_config_desired_state(),
    )
    .await
    .expect("single-connection capacity rejection must not wait");
    let error = match result {
        Ok(_) => panic!("single-connection guard unexpectedly succeeded"),
        Err(error) => error,
    };
    assert!(error
        .to_string()
        .contains("runtime_config_desired_state_pool_capacity_too_small"));

    drop(single_repo);
    single_pool.close().await;
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_runtime_config_guard_rejects_concurrent_waiter_without_hoarding_pool() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let first = db.repo.lock_runtime_config_desired_state().await.unwrap();
    let rejected = tokio::time::timeout(
        Duration::from_millis(250),
        db.repo.lock_runtime_config_desired_state(),
    )
    .await
    .expect("concurrent desired-state guard must fail without waiting");
    let rejected = match rejected {
        Ok(_) => panic!("concurrent desired-state guard unexpectedly queued"),
        Err(error) => error,
    };
    assert!(rejected
        .to_string()
        .contains("runtime_config_desired_state_busy"));

    drop(first);
    db.repo
        .lock_runtime_config_desired_state()
        .await
        .expect("guard must be immediately reusable after the holder exits");
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_audit_schema_rejects_non_object_metadata() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };

    let error = sqlx::query(
        r#"
        INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
        VALUES ($1, NULL, 'test.invalid_metadata', 'test:audit', NULL, $2)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(serde_json::json!(["result", "origin_kind", "component"]))
    .execute(&db.pool)
    .await
    .unwrap_err();

    assert!(error.to_string().contains("audit_logs_canonical_metadata"));
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_tunnel_evidence_clear_is_scoped_counted_and_audited_atomically() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    for client_id in ["evidence-clear-left", "evidence-clear-right"] {
        insert_client(&db.pool, client_id, None).await;
    }
    let operator = postgres_network_operator(&db.repo).await;
    let mut selected_input = postgres_alert_test_tunnel_input();
    selected_input.name = "evidence-clear-selected".to_string();
    selected_input.interface_name = "tunclr0".to_string();
    selected_input.runtime_control = Default::default();
    selected_input.left_mtu = vpsman_common::default_tunnel_mtu(TunnelKind::Gre);
    selected_input.right_mtu = vpsman_common::default_tunnel_mtu(TunnelKind::Gre);
    selected_input.left_client_id = "evidence-clear-left".to_string();
    selected_input.right_client_id = "evidence-clear-right".to_string();
    selected_input.address_pool_cidr = "10.73.0.0/30".to_string();
    selected_input.ipv4_tunnel = Some(TunnelAddressPair {
        left: "10.73.0.0".to_string(),
        right: "10.73.0.1".to_string(),
        prefix_len: 31,
    });
    let selected = db
        .repo
        .record_tunnel_plan(
            &selected_input,
            &plan_tunnel(&selected_input).unwrap(),
            true,
            &operator,
        )
        .await
        .unwrap();
    let mut retained_input = selected_input.clone();
    retained_input.name = "evidence-clear-retained".to_string();
    retained_input.interface_name = "tunclr1".to_string();
    retained_input.address_pool_cidr = "10.73.0.4/30".to_string();
    retained_input.ipv4_tunnel = Some(TunnelAddressPair {
        left: "10.73.0.4".to_string(),
        right: "10.73.0.5".to_string(),
        prefix_len: 31,
    });
    let retained = db
        .repo
        .record_tunnel_plan(
            &retained_input,
            &plan_tunnel(&retained_input).unwrap(),
            true,
            &operator,
        )
        .await
        .unwrap();
    for (plan, kind, source) in [
        (&selected, "network_status", "manual"),
        (&selected, "tunnel_reachability", "automatic"),
        (&selected, "tunnel_reachability", "manual"),
        (&selected, "network_speed_test", "manual"),
        (&retained, "network_status", "automatic"),
    ] {
        sqlx::query(
            r#"
            INSERT INTO network_observations (
                id, client_id, kind, source, plan_id, topology_identity_hash,
                plan_name, interface_name, peer_client_id
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&plan.left_client_id)
        .bind(kind)
        .bind(source)
        .bind(plan.id)
        .bind(crate::repository_network_observations::topology_identity_hash_for_plan(plan))
        .bind(&plan.name)
        .bind(&plan.plan.interface_name)
        .bind(&plan.right_client_id)
        .execute(&db.pool)
        .await
        .unwrap();
    }

    let results = db
        .repo
        .clear_tunnel_plan_evidence(&[(selected.id, selected.revision)], &operator)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].plan_id, selected.id);
    assert_eq!(results[0].name, selected.name);
    assert_eq!(results[0].reviewed_revision, selected.revision);
    assert_eq!(results[0].cleared_observation_count, 4);
    let counts = sqlx::query_as::<_, (Uuid, i64)>(
        r#"
        SELECT plan_id, count(*)::bigint
        FROM network_observations
        GROUP BY plan_id
        "#,
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(counts, vec![(retained.id, 1)]);
    let audit = sqlx::query(
        r#"
        SELECT target, metadata
        FROM audit_logs
        WHERE action = 'network.tunnel_plan_evidence_cleared'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        audit.try_get::<String, _>("target").unwrap(),
        format!("tunnel_plan:{}", selected.id)
    );
    let metadata = audit.try_get::<serde_json::Value, _>("metadata").unwrap();
    assert_eq!(metadata["cleared_observation_count"], 4);
    assert_eq!(metadata["plans"][0]["reviewed_revision"], selected.revision);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT revision FROM tunnel_plans WHERE id = $1")
            .bind(selected.id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        selected.revision
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_rollout_reconciler_isolates_missing_current_batch_assignment() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let operator = postgres_network_operator(&db.repo).await;
    let rollout_policy = |canary: &str| JobRolloutPolicy {
        canary_client_ids: vec![canary.to_string()],
        batch_size: 1,
        max_failures: 0,
        pause_after_canary: false,
        batch_delay_secs: 0,
    };
    for client_id in [
        "broken-a",
        "broken-b",
        "broken-c",
        "healthy-a",
        "healthy-b",
        "healthy-c",
    ] {
        insert_client(&db.pool, client_id, Some(Uuid::new_v4())).await;
    }

    let malformed_job_id = Uuid::new_v4();
    let mut malformed_request = crate::tests::operation_job_request(
        JobCommand::AgentUpdateCheck {
            version_url: None,
            activate: false,
            restart_agent: false,
        },
        &["broken-a", "broken-b", "broken-c"],
    );
    malformed_request.rollout = Some(rollout_policy("broken-a"));
    db.repo
        .record_dispatching_job(
            malformed_job_id,
            &malformed_request,
            "malformed-command-hash",
            "malformed-request-fingerprint",
            &operator,
            &malformed_request.target_client_ids,
        )
        .await
        .unwrap();

    let healthy_job_id = Uuid::new_v4();
    let mut healthy_request = crate::tests::operation_job_request(
        JobCommand::AgentUpdateCheck {
            version_url: None,
            activate: false,
            restart_agent: false,
        },
        &["healthy-a", "healthy-b", "healthy-c"],
    );
    healthy_request.rollout = Some(rollout_policy("healthy-a"));
    db.repo
        .record_dispatching_job(
            healthy_job_id,
            &healthy_request,
            "healthy-command-hash",
            "healthy-request-fingerprint",
            &operator,
            &healthy_request.target_client_ids,
        )
        .await
        .unwrap();

    sqlx::query(
        "UPDATE job_rollouts SET current_batch = 1, updated_at = to_timestamp(1) WHERE job_id = $1",
    )
    .bind(malformed_job_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM job_rollout_targets WHERE job_id = $1 AND batch_index = 1")
        .bind(malformed_job_id)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE job_rollouts SET updated_at = to_timestamp(2) WHERE job_id = $1")
        .bind(healthy_job_id)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(
        r#"
        UPDATE job_targets
        SET status = 'completed', exit_code = 0, completed_at = now()
        WHERE job_id = $1 AND client_id = 'healthy-a'
        "#,
    )
    .bind(healthy_job_id)
    .execute(&db.pool)
    .await
    .unwrap();

    assert_eq!(db.repo.reconcile_job_rollouts(1).await.unwrap(), 1);
    let malformed = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT status, pause_reason FROM job_rollouts WHERE job_id = $1",
    )
    .bind(malformed_job_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        malformed,
        (
            "paused".to_string(),
            Some("current_batch_assignment_missing".to_string())
        )
    );
    assert_eq!(
        sqlx::query_scalar::<_, i32>("SELECT current_batch FROM job_rollouts WHERE job_id = $1",)
            .bind(healthy_job_id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        0
    );

    assert_eq!(db.repo.reconcile_job_rollouts(1).await.unwrap(), 1);
    let healthy = sqlx::query_as::<_, (String, i32, Option<String>)>(
        "SELECT status, current_batch, pause_reason FROM job_rollouts WHERE job_id = $1",
    )
    .bind(healthy_job_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(healthy, ("running".to_string(), 1, None));

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_schedule_query_without_limit_returns_all_rows() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        r#"
        INSERT INTO schedules (
            id,
            name,
            operation,
            selector_expression,
            target_client_ids,
            cron_expr,
            next_run_at
        )
        SELECT
            md5('schedule-no-limit-' || series::text)::uuid,
            'schedule-no-limit-' || series::text,
            '{"type":"shell","argv":["/bin/true"],"pty":false}'::jsonb,
            'tag:edge',
            ARRAY['client-a']::text[],
            '0 * * * *',
            now() + interval '1 hour'
        FROM generate_series(1, 1001) AS series
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    assert_eq!(
        db.repo
            .query_schedules(&ListQuery::default())
            .await
            .unwrap()
            .len(),
        1_001
    );
    assert_eq!(
        db.repo
            .query_schedules(&ListQuery {
                limit: Some(1_000),
                ..ListQuery::default()
            })
            .await
            .unwrap()
            .len(),
        1_000
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_schedule_edits_preserve_deleted_and_empty_frozen_targets() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "frozen-a", None).await;
    let operator = postgres_network_operator(&db.repo).await;
    let mut create = postgres_shell_schedule_request("frozen-targets", "frozen-a");
    create.selector_expression = "id:frozen-a".to_string();
    let schedule = db.repo.create_schedule(create, &operator).await.unwrap();
    sqlx::query("UPDATE clients SET status = 'deleted', hidden_at = now() WHERE id = 'frozen-a'")
        .execute(&db.pool)
        .await
        .unwrap();

    let original_snapshot = crate::repository_schedules::ScheduleSnapshotExpectation {
        selector_expression: schedule.selector_expression.clone(),
        target_client_ids: schedule.target_client_ids.clone(),
        definition_revision: schedule.definition_revision,
    };
    let preserved = db
        .repo
        .update_schedule_record(
            schedule.id,
            crate::repository_schedules::ScheduleCreateInput {
                name: "frozen-targets-renamed".to_string(),
                operation: schedule.operation.clone(),
                event_argv_template: schedule.event_argv_template.clone(),
                selector_expression: schedule.selector_expression.clone(),
                target_client_ids: schedule.target_client_ids.clone(),
                trigger_kind: schedule.trigger_kind,
                cron_expr: schedule.cron_expr.clone(),
                timezone: schedule.timezone.clone(),
                event_expression: schedule.event_expression.clone(),
                enabled: schedule.enabled,
                catch_up_policy: schedule.catch_up_policy.clone(),
                catch_up_limit: schedule.catch_up_limit,
                retry_delay_secs: schedule.retry_delay_secs,
                max_failures: schedule.max_failures,
                expected_definition_revision: Some(schedule.definition_revision),
            },
            Some(&original_snapshot),
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(preserved.target_client_ids, vec!["frozen-a"]);

    let preserved_snapshot = crate::repository_schedules::ScheduleSnapshotExpectation {
        selector_expression: preserved.selector_expression.clone(),
        target_client_ids: preserved.target_client_ids.clone(),
        definition_revision: preserved.definition_revision,
    };
    let changed_selector_error = db
        .repo
        .update_schedule_record(
            schedule.id,
            crate::repository_schedules::ScheduleCreateInput {
                name: preserved.name.clone(),
                operation: preserved.operation.clone(),
                event_argv_template: preserved.event_argv_template.clone(),
                selector_expression: "id:replacement".to_string(),
                target_client_ids: preserved.target_client_ids.clone(),
                trigger_kind: preserved.trigger_kind,
                cron_expr: preserved.cron_expr.clone(),
                timezone: preserved.timezone.clone(),
                event_expression: preserved.event_expression.clone(),
                enabled: preserved.enabled,
                catch_up_policy: preserved.catch_up_policy.clone(),
                catch_up_limit: preserved.catch_up_limit,
                retry_delay_secs: preserved.retry_delay_secs,
                max_failures: preserved.max_failures,
                expected_definition_revision: Some(preserved.definition_revision),
            },
            Some(&preserved_snapshot),
            &operator,
        )
        .await
        .unwrap_err();
    assert!(changed_selector_error
        .to_string()
        .contains("schedule_fixed_targets_not_found"));

    let empty = db
        .repo
        .update_schedule_targets(
            schedule.id,
            Vec::new(),
            Some(&preserved_snapshot),
            &operator,
        )
        .await
        .unwrap();
    assert!(empty.target_client_ids.is_empty());
    let empty_snapshot = crate::repository_schedules::ScheduleSnapshotExpectation {
        selector_expression: empty.selector_expression.clone(),
        target_client_ids: Vec::new(),
        definition_revision: empty.definition_revision,
    };
    let edited_empty = db
        .repo
        .update_schedule_record(
            schedule.id,
            crate::repository_schedules::ScheduleCreateInput {
                name: "frozen-targets-empty".to_string(),
                operation: empty.operation.clone(),
                event_argv_template: empty.event_argv_template.clone(),
                selector_expression: empty.selector_expression.clone(),
                target_client_ids: Vec::new(),
                trigger_kind: empty.trigger_kind,
                cron_expr: empty.cron_expr.clone(),
                timezone: empty.timezone.clone(),
                event_expression: empty.event_expression.clone(),
                enabled: empty.enabled,
                catch_up_policy: empty.catch_up_policy.clone(),
                catch_up_limit: empty.catch_up_limit,
                retry_delay_secs: empty.retry_delay_secs,
                max_failures: empty.max_failures,
                expected_definition_revision: Some(empty.definition_revision),
            },
            Some(&empty_snapshot),
            &operator,
        )
        .await
        .unwrap();
    assert!(edited_empty.target_client_ids.is_empty());

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_internal_dispatch_queries_do_not_silently_omit_after_one_thousand() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        r#"
        INSERT INTO fleet_alert_notification_channels (
            id,
            name,
            scope_kind,
            min_severity,
            delivery_kind,
            target
        )
        SELECT
            md5('notification-overflow-' || series::text)::uuid,
            'notification-overflow-' || series::text,
            'global',
            'warning',
            'webhook',
            'https://hooks.acme.com/fleet'
        FROM generate_series(1, 1001) AS series
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let notification_error = db
        .repo
        .list_enabled_fleet_alert_notification_channels_for_dispatch()
        .await
        .unwrap_err()
        .to_string();
    assert!(notification_error.contains("fleet_alert_notification_dispatch_channel_limit_exceeded"));

    let targeted_rule_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO webhook_rules (id, name, expression, target)
        SELECT
            md5('webhook-filler-' || series::text)::uuid,
            'webhook-filler-' || lpad(series::text, 4, '0'),
            'interval.30sec',
            'https://hooks.acme.com/filler'
        FROM generate_series(1, 1000) AS series
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO webhook_rules (id, name, expression, target)
        VALUES ($1, 'zzzz-targeted-webhook', 'interval.30sec', 'https://hooks.acme.com/targeted')
        "#,
    )
    .bind(targeted_rule_id)
    .execute(&db.pool)
    .await
    .unwrap();
    assert!(db
        .repo
        .list_webhook_rules(1_000, None)
        .await
        .unwrap()
        .iter()
        .all(|rule| rule.id != targeted_rule_id));
    assert_eq!(
        db.repo
            .webhook_rule_by_id(targeted_rule_id)
            .await
            .unwrap()
            .unwrap()
            .id,
        targeted_rule_id
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_invalid_notification_channel_filters_are_visible_but_never_dispatched() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let operator = postgres_network_operator(&db.repo).await;
    let healthy_id = Uuid::new_v4();
    let invalid_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO fleet_alert_notification_channels (
            id,
            name,
            scope_kind,
            min_severity,
            categories,
            operator_states,
            delivery_kind,
            target
        )
        VALUES
            (
                $1,
                'healthy-channel',
                'global',
                'warning',
                '["agent_status"]'::jsonb,
                '["open"]'::jsonb,
                'webhook',
                'https://hooks.acme.com/healthy'
            ),
            (
                $2,
                'invalid-channel',
                'global',
                'warning',
                '[42]'::jsonb,
                '["open"]'::jsonb,
                'webhook',
                'https://hooks.acme.com/invalid'
            )
        "#,
    )
    .bind(healthy_id)
    .bind(invalid_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let listed = db
        .repo
        .list_fleet_alert_notification_channels(10, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(listed.len(), 2);
    let invalid = listed
        .iter()
        .find(|channel| channel.id == invalid_id)
        .unwrap();
    assert_eq!(
        invalid.configuration_error.as_deref(),
        Some("fleet_alert_notification_channel_filters_invalid")
    );

    let dispatchable = db
        .repo
        .list_enabled_fleet_alert_notification_channels_for_dispatch()
        .await
        .unwrap();
    assert_eq!(
        dispatchable
            .iter()
            .map(|channel| channel.id)
            .collect::<Vec<_>>(),
        vec![healthy_id]
    );

    db.repo
        .delete_fleet_alert_notification_channel(invalid_id, "invalid-channel", &operator)
        .await
        .unwrap();
    assert_eq!(
        db.repo
            .list_fleet_alert_notification_channels(10, None, None, None, None)
            .await
            .unwrap()
            .len(),
        1
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_topology_evidence_is_bounded_per_plan_beyond_global_caps() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    for client_id in [
        "topology-noisy-left",
        "topology-noisy-right",
        "topology-quiet-left",
        "topology-quiet-right",
    ] {
        insert_client(&db.pool, client_id, None).await;
    }
    let operator = postgres_network_operator(&db.repo).await;
    let mut noisy_input = postgres_alert_test_tunnel_input();
    noisy_input.name = "topology-noisy".to_string();
    noisy_input.interface_name = "tun-noisy".to_string();
    noisy_input.runtime_control = Default::default();
    noisy_input.left_mtu = vpsman_common::default_tunnel_mtu(TunnelKind::Gre);
    noisy_input.right_mtu = vpsman_common::default_tunnel_mtu(TunnelKind::Gre);
    noisy_input.left_client_id = "topology-noisy-left".to_string();
    noisy_input.right_client_id = "topology-noisy-right".to_string();
    noisy_input.address_pool_cidr = "10.70.0.0/30".to_string();
    noisy_input.ipv4_tunnel = Some(TunnelAddressPair {
        left: "10.70.0.0".to_string(),
        right: "10.70.0.1".to_string(),
        prefix_len: 31,
    });
    let noisy_plan = db
        .repo
        .record_tunnel_plan(
            &noisy_input,
            &plan_tunnel(&noisy_input).unwrap(),
            true,
            &operator,
        )
        .await
        .unwrap();
    let mut quiet_input = noisy_input.clone();
    quiet_input.name = "topology-quiet".to_string();
    quiet_input.interface_name = "tun-quiet".to_string();
    quiet_input.left_client_id = "topology-quiet-left".to_string();
    quiet_input.right_client_id = "topology-quiet-right".to_string();
    quiet_input.address_pool_cidr = "10.70.0.4/30".to_string();
    quiet_input.ipv4_tunnel = Some(TunnelAddressPair {
        left: "10.70.0.4".to_string(),
        right: "10.70.0.5".to_string(),
        prefix_len: 31,
    });
    let quiet_plan = db
        .repo
        .record_tunnel_plan(
            &quiet_input,
            &plan_tunnel(&quiet_input).unwrap(),
            true,
            &operator,
        )
        .await
        .unwrap();
    let noisy_identity =
        crate::repository_network_observations::topology_identity_hash_for_plan(&noisy_plan);
    let quiet_identity =
        crate::repository_network_observations::topology_identity_hash_for_plan(&quiet_plan);
    let noisy_job_id = Uuid::new_v4();
    let quiet_job_id = Uuid::new_v4();
    insert_job_target(
        &db.pool,
        noisy_job_id,
        "topology-noisy-left",
        "completed",
        true,
        None,
    )
    .await;
    insert_job_target(
        &db.pool,
        quiet_job_id,
        "topology-quiet-left",
        "completed",
        true,
        None,
    )
    .await;
    sqlx::query(
        r#"
        INSERT INTO network_observations (
            id,
            job_id,
            client_id,
            seq,
            kind,
            source,
            endpoint_side,
            stale_after_secs,
            plan_id,
            topology_identity_hash,
            plan_name,
            interface_name,
            peer_client_id,
            healthy,
            latency_avg_ms,
            packet_loss_ratio,
            observed_at
        )
        SELECT
            md5('topology-noisy-observation-' || series::text)::uuid,
            $1,
            'topology-noisy-left',
            series::integer,
            'tunnel_reachability',
            'manual',
            'left',
            180,
            $2,
            $3,
            'topology-noisy',
            'tun-noisy',
            'topology-noisy-right',
            TRUE,
            5.0,
            0.0,
            to_timestamp(2000 + series)
        FROM generate_series(1, 1001) AS series
        "#,
    )
    .bind(noisy_job_id)
    .bind(noisy_plan.id)
    .bind(&noisy_identity)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO network_observations (
            id,
            job_id,
            client_id,
            seq,
            kind,
            source,
            endpoint_side,
            stale_after_secs,
            plan_id,
            topology_identity_hash,
            plan_name,
            interface_name,
            peer_client_id,
            healthy,
            latency_avg_ms,
            packet_loss_ratio,
            observed_at
        )
        VALUES (
            $1,
            $2,
            'topology-quiet-left',
            1,
            'tunnel_reachability',
            'manual',
            'left',
            180,
            $3,
            $4,
            'topology-quiet',
            'tun-quiet',
            'topology-quiet-right',
            FALSE,
            42.0,
            0.1,
            to_timestamp(1000)
        )
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(quiet_job_id)
    .bind(quiet_plan.id)
    .bind(&quiet_identity)
    .execute(&db.pool)
    .await
    .unwrap();

    let observations = db
        .repo
        .list_network_observations_for_topology(
            &[
                (
                    noisy_plan.id,
                    noisy_identity.clone(),
                    noisy_plan.left_client_id.clone(),
                    noisy_plan.right_client_id.clone(),
                ),
                (
                    quiet_plan.id,
                    quiet_identity.clone(),
                    quiet_plan.left_client_id.clone(),
                    quiet_plan.right_client_id.clone(),
                ),
            ],
            0,
            crate::unix_now() as i64,
            24,
        )
        .await
        .unwrap();
    assert_eq!(
        observations
            .iter()
            .filter(|observation| observation.plan_id == Some(noisy_plan.id))
            .count(),
        24
    );
    assert!(observations
        .iter()
        .any(|observation| observation.plan_id == Some(quiet_plan.id)));
    let graph = db
        .repo
        .topology_graph(24, 0, crate::unix_now() as i64, &[])
        .await
        .unwrap();
    let quiet_edge = graph
        .edges
        .iter()
        .find(|edge| edge.plan_id == quiet_plan.id)
        .unwrap();
    assert_eq!(quiet_edge.sample_count, 1);
    assert_eq!(quiet_edge.left_reachability_state, "stale");
    assert_eq!(quiet_edge.right_reachability_state, "unknown");
    assert_eq!(quiet_edge.reachability_state, "recorded");
    assert_eq!(quiet_edge.latency_series_ms, vec![42.0]);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_network_trend_budget_preserves_full_day_for_120_series() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "trend-budget-left", None).await;
    insert_client(&db.pool, "trend-budget-right", None).await;
    let plan_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO tunnel_plans (
            id, name, kind, left_client_id, right_client_id, input, plan
        ) VALUES ($1, 'trend-budget-plan', 'wireguard',
            'trend-budget-left', 'trend-budget-right', '{}'::jsonb, '{}'::jsonb)
        "#,
    )
    .bind(plan_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO network_observation_series (
            plan_id, topology_identity_hash, plan_name, interface_name,
            client_id, peer_client_id, endpoint_side, address_family, target
        )
        SELECT $1,
               'trend-budget-identity-' || series_no,
               'trend-budget-plan',
               'trend-budget-interface-' || series_no,
               'trend-budget-left',
               'trend-budget-right',
               'left',
               'ipv4',
               'trend-budget-target-' || series_no
        FROM generate_series(1, 120) series_no
        "#,
    )
    .bind(plan_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let start_unix: i64 = sqlx::query_scalar(
        "SELECT floor(extract(epoch FROM now()) / 86400)::bigint * 86400 - 86400",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO network_observations (
            id, client_id, kind, source, role, plan_id,
            topology_identity_hash, plan_name, interface_name,
            peer_client_id, target, endpoint_side, address_family,
            stale_after_secs, healthy, transmitted, received,
            latency_min_ms, latency_avg_ms, latency_max_ms,
            latency_mdev_ms, packet_loss_ratio, automatic_series_id,
            observed_at, received_at
        )
        SELECT md5('trend-budget-observation-' || series.id || '-' || point_no)::uuid,
               series.client_id,
               'tunnel_reachability',
               'automatic',
               'endpoint',
               series.plan_id,
               series.topology_identity_hash,
               series.plan_name,
               series.interface_name,
               series.peer_client_id,
               series.target,
               series.endpoint_side,
               series.address_family,
               180,
               TRUE,
               3,
               3,
               5.0,
               5.0,
               5.0,
               0.1,
               0.0,
               series.id,
               to_timestamp($2) + make_interval(mins => point_no * 5),
               to_timestamp($2) + make_interval(mins => point_no * 5)
        FROM network_observation_series series
        CROSS JOIN generate_series(0, 287) point_no
        WHERE series.plan_id = $1
          AND series.topology_identity_hash LIKE 'trend-budget-identity-%'
        "#,
    )
    .bind(plan_id)
    .bind(start_unix)
    .execute(&db.pool)
    .await
    .unwrap();

    let trends = db
        .repo
        .list_network_observation_trends_filtered(&NetworkObservationFilter {
            start_unix,
            end_unix: start_unix + 86_400,
            plan_ids: vec![plan_id],
            client_id: None,
            source: Some("automatic".to_string()),
            kind: Some("tunnel_reachability".to_string()),
            health: None,
            search: None,
            limit: 10_000,
            visible_only: false,
        })
        .await
        .unwrap();

    assert!(trends.len() <= 10_000);
    let mut timestamps_by_series = std::collections::HashMap::<String, Vec<u64>>::new();
    for trend in &trends {
        assert_eq!(trend.bucket_secs, None);
        assert!(!trend.retained);
        let interface_name = trend.interface_name.clone().unwrap();
        let bucket_start = crate::util::parse_timestamp_unix(
            trend
                .bucket_start
                .as_deref()
                .expect("exact trend timestamp"),
        )
        .expect("valid exact trend timestamp");
        timestamps_by_series
            .entry(interface_name)
            .or_default()
            .push(bucket_start);
    }
    assert_eq!(timestamps_by_series.len(), 120);
    for timestamps in timestamps_by_series.values_mut() {
        timestamps.sort_unstable();
        assert_eq!(timestamps.first().copied(), Some(start_unix as u64));
        assert_eq!(
            timestamps.last().copied(),
            Some((start_unix + 287 * 300) as u64)
        );
        assert!(timestamps.len() >= 82);
        assert!(timestamps.len() <= 83);
    }
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_network_observation_prune_includes_tiers_but_preserves_latest() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "network-prune-left", None).await;
    insert_client(&db.pool, "network-prune-right", None).await;
    let plan_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO tunnel_plans (
            id, name, kind, left_client_id, right_client_id, input, plan
        ) VALUES ($1, 'network-prune-plan', 'wireguard',
            'network-prune-left', 'network-prune-right', '{}'::jsonb, '{}'::jsonb)
        "#,
    )
    .bind(plan_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let series_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO network_observation_series (
            plan_id, topology_identity_hash, plan_name, interface_name,
            client_id, peer_client_id, endpoint_side, address_family, target
        ) VALUES ($1, 'network-prune-identity', 'network-prune-plan', 'tun-prune',
            'network-prune-left', 'network-prune-right', 'left', 'ipv4', '10.0.0.2')
        RETURNING id
        "#,
    )
    .bind(plan_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let latest_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO network_observation_latest (
            series_id, observation_id, stale_after_secs, healthy,
            transmitted, received, packet_loss_ratio, observed_at, received_at
        ) VALUES ($1, $2, 180, TRUE, 3, 3, 0.0,
            date_trunc('day', now() - interval '400 days'),
            date_trunc('day', now() - interval '400 days'))
        "#,
    )
    .bind(series_id)
    .bind(latest_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO network_observation_rollups (
            series_id, bucket_secs, bucket_start, health_state, reason_key,
            sample_count, transmitted_total, transmitted_sample_count,
            received_total, received_sample_count, latency_sum_ms,
            latency_sample_count, latency_min_ms, latency_max_ms,
            latency_mdev_sum_ms, latency_mdev_sample_count,
            packet_loss_sum_ratio, packet_loss_sample_count,
            packet_loss_min_ratio, packet_loss_max_ratio,
            latest_observation_id, latest_stale_after_secs, latest_healthy,
            latest_transmitted, latest_received, latest_packet_loss_ratio,
            latest_observed_at, latest_received_at
        ) VALUES (
            $1, 86400, date_trunc('day', now() - interval '400 days'), 1, '',
            2, 6, 2, 6, 2, 0.0, 0, NULL, NULL, 0.0, 0,
            0.0, 2, 0.0, 0.0, $2, 180, TRUE, 3, 3, 0.0,
            date_trunc('day', now() - interval '400 days'),
            date_trunc('day', now() - interval '400 days'))
        "#,
    )
    .bind(series_id)
    .bind(latest_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let inactive_series_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO network_observation_series (
            plan_id, topology_identity_hash, plan_name, interface_name,
            client_id, peer_client_id, endpoint_side, address_family, target,
            active, last_seen_at
        ) VALUES ($1, 'network-prune-inactive', 'network-prune-plan', 'tun-prune-old',
            'network-prune-left', 'network-prune-right', 'left', 'ipv6', 'fd00::2',
            FALSE, now() - interval '400 days')
        RETURNING id
        "#,
    )
    .bind(plan_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO network_observation_latest (
            series_id, observation_id, stale_after_secs, healthy,
            transmitted, received, packet_loss_ratio, observed_at, received_at
        ) VALUES ($1, $2, 180, FALSE, 3, 0, 1.0,
            date_trunc('day', now() - interval '400 days'),
            date_trunc('day', now() - interval '400 days'))
        "#,
    )
    .bind(inactive_series_id)
    .bind(Uuid::new_v4())
    .execute(&db.pool)
    .await
    .unwrap();
    let exported_tiers = db
        .repo
        .export_network_observation_rollups(10)
        .await
        .unwrap();
    assert_eq!(exported_tiers.len(), 1);
    assert_eq!(exported_tiers[0]["retained"], true);
    assert_eq!(exported_tiers[0]["bucket_secs"], 86_400);
    assert_eq!(exported_tiers[0]["effective_resolution_secs"], 86_400);
    assert_eq!(exported_tiers[0]["sample_count"], 2);
    assert_eq!(exported_tiers[0]["latency_sum_ms"], 0.0);
    assert_eq!(exported_tiers[0]["packet_loss_sample_count"], 2);
    let prune_plan = HistoryRetentionPrunePlan {
        domain: HistoryDomain::NetworkObservations,
        prune_limit: 10,
        enabled: true,
    };
    let cutoff = crate::unix_now().saturating_sub(300 * 86_400);
    let preview = db
        .repo
        .prune_history_domain(&prune_plan, cutoff, true)
        .await
        .unwrap();
    assert_eq!(preview.matched_rows, 2);
    assert_eq!(preview.pruned_rows, 0);
    let applied = db
        .repo
        .prune_history_domain(&prune_plan, cutoff, false)
        .await
        .unwrap();
    assert_eq!(applied.matched_rows, 2);
    assert_eq!(applied.pruned_rows, 2);
    let tiered_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM network_observation_rollups WHERE series_id = $1")
            .bind(series_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let latest_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM network_observation_latest WHERE series_id = $1")
            .bind(series_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let inactive_series_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM network_observation_series WHERE id = $1")
            .bind(inactive_series_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(tiered_count, 0);
    assert_eq!(latest_count, 1);
    assert_eq!(inactive_series_count, 0);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_operational_evidence_hides_deleted_plans_but_history_retains_it() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let left_client_id = "deleted-evidence-left";
    let right_client_id = "deleted-evidence-right";
    insert_client(&db.pool, left_client_id, None).await;
    insert_client(&db.pool, right_client_id, None).await;
    let operator = postgres_network_operator(&db.repo).await;
    let mut input = postgres_alert_test_tunnel_input();
    input.name = "deleted-evidence-plan".to_string();
    input.interface_name = "tun-del-ev".to_string();
    input.runtime_control = Default::default();
    input.left_mtu = vpsman_common::default_tunnel_mtu(TunnelKind::Gre);
    input.right_mtu = vpsman_common::default_tunnel_mtu(TunnelKind::Gre);
    input.left_client_id = left_client_id.to_string();
    input.right_client_id = right_client_id.to_string();
    input.address_pool_cidr = "10.71.0.0/30".to_string();
    input.ipv4_tunnel = Some(TunnelAddressPair {
        left: "10.71.0.0".to_string(),
        right: "10.71.0.1".to_string(),
        prefix_len: 31,
    });
    let saved = db
        .repo
        .record_tunnel_plan(&input, &plan_tunnel(&input).unwrap(), true, &operator)
        .await
        .unwrap();
    let observation_id = Uuid::new_v4();
    db.repo
        .record_automatic_tunnel_reachability(
            left_client_id,
            &[TunnelReachabilityObservation {
                id: observation_id,
                source: TunnelReachabilitySource::Automatic,
                plan_id: saved.id,
                topology_identity_hash:
                    crate::repository_network_observations::topology_identity_hash_for_plan(&saved),
                endpoint_side: TunnelEndpointSide::Left,
                peer_client_id: right_client_id.to_string(),
                interface_name: input.interface_name.clone(),
                address_family: TunnelAddressFamily::Ipv4,
                target: "10.71.0.1".to_string(),
                measured_unix: crate::unix_now(),
                stale_after_secs: 180,
                transmitted: 3,
                received: 3,
                latency_min_ms: Some(1.0),
                latency_avg_ms: Some(2.0),
                latency_max_ms: Some(3.0),
                latency_mdev_ms: Some(0.1),
                packet_loss_ratio: 0.0,
                healthy: true,
                reason: None,
            }],
        )
        .await
        .unwrap();

    assert_eq!(
        db.repo
            .list_network_observations(10, true)
            .await
            .unwrap()
            .len(),
        1
    );
    db.repo
        .delete_tunnel_plan(saved.id, saved.revision, &operator)
        .await
        .unwrap();
    assert!(db
        .repo
        .list_network_observations(10, true)
        .await
        .unwrap()
        .is_empty());
    assert!(db
        .repo
        .list_network_observation_trends(10, true)
        .await
        .unwrap()
        .is_empty());
    let history = db.repo.list_network_observations(10, false).await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].id, observation_id);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_process_inventory_bounds_only_relevant_history_and_fails_explicitly() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "process-bound-client", None).await;
    insert_client(&db.pool, "process-hidden-client", None).await;
    let shell_job_id = Uuid::new_v4();
    let process_job_id = Uuid::new_v4();
    let hidden_process_job_id = Uuid::new_v4();
    insert_job_target(
        &db.pool,
        shell_job_id,
        "process-bound-client",
        "completed",
        true,
        None,
    )
    .await;
    insert_job_target(
        &db.pool,
        process_job_id,
        "process-bound-client",
        "completed",
        true,
        None,
    )
    .await;
    insert_job_target(
        &db.pool,
        hidden_process_job_id,
        "process-hidden-client",
        "completed",
        true,
        None,
    )
    .await;
    sqlx::query("UPDATE jobs SET command_type = 'process_status' WHERE id = $1")
        .bind(process_job_id)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET command_type = 'process_status' WHERE id = $1")
        .bind(hidden_process_job_id)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(
        r#"
        INSERT INTO job_outputs (
            job_id, client_id, seq, stream, data, done, created_at
        ) VALUES (
            $1,
            'process-hidden-client',
            0,
            'stdout',
            convert_to(
                '{"type":"process_status","processes":[{"name":"hidden-worker","status":"running"}]}',
                'UTF8'
            ),
            FALSE,
            to_timestamp(30000)
        )
        "#,
    )
    .bind(hidden_process_job_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE clients SET status = 'deleted', hidden_at = now() WHERE id = 'process-hidden-client'",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO job_outputs (
            job_id, client_id, seq, stream, data, done, created_at
        )
        SELECT
            $1,
            'process-bound-client',
            series,
            'stdout',
            convert_to('unrelated shell output', 'UTF8'),
            FALSE,
            to_timestamp(20000 + series)
        FROM generate_series(0, 10000) AS series
        "#,
    )
    .bind(shell_job_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO job_outputs (
            job_id, client_id, seq, stream, data, done, created_at
        )
        VALUES (
            $1,
            'process-bound-client',
            0,
            'stdout',
            convert_to(
                '{"type":"process_status","processes":[{"name":"worker","status":"running"}]}',
                'UTF8'
            ),
            FALSE,
            to_timestamp(10000)
        )
        "#,
    )
    .bind(process_job_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let exact = db.repo.list_process_supervisor_inventory(2).await.unwrap();
    assert_eq!(exact.len(), 1);
    assert_eq!(exact[0].name, "worker");
    assert_eq!(exact[0].client_id, "process-bound-client");

    sqlx::query(
        r#"
        INSERT INTO job_outputs (
            job_id, client_id, seq, stream, data, done, created_at
        )
        SELECT
            $1,
            'process-bound-client',
            series,
            'stdout',
            convert_to(
                '{"type":"process_status","processes":[{"name":"worker","status":"running"}]}',
                'UTF8'
            ),
            FALSE,
            to_timestamp(10000 + series)
        FROM generate_series(1, 10000) AS series
        "#,
    )
    .bind(process_job_id)
    .execute(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        db.repo
            .list_process_supervisor_inventory(2)
            .await
            .unwrap_err()
            .to_string(),
        crate::repository_job_outputs::PROCESS_SUPERVISOR_INVENTORY_SCAN_LIMIT_ERROR
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_ospf_controller_batches_persist_fair_rotation() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "ospf-fair-left", None).await;
    insert_client(&db.pool, "ospf-fair-right", None).await;
    let operator = postgres_network_operator(&db.repo).await;
    let mut created_plans = Vec::new();
    for index in 0..7_u8 {
        let network = 80_u8 + index;
        let mut input = postgres_alert_test_tunnel_input();
        input.name = format!("ospf-fair-{index}");
        input.interface_name = format!("of{index}");
        input.left_client_id = "ospf-fair-left".to_string();
        input.right_client_id = "ospf-fair-right".to_string();
        input.address_pool_cidr = format!("10.{network}.0.0/30");
        input.ipv4_tunnel = Some(TunnelAddressPair {
            left: format!("10.{network}.0.0"),
            right: format!("10.{network}.0.1"),
            prefix_len: 31,
        });
        input.ospf = Some(TunnelOspfConfig {
            mode: OspfControlMode::Automatic,
            planned_latency_ms: 20.0,
            planned_packet_loss_ratio: 0.0,
            preference: 1.0,
            policy: OspfCostPolicy::default(),
            min_cost_delta: 5,
            healthy_windows: 1,
            left_adapter_definition_id: Some(Uuid::new_v4().to_string()),
            right_adapter_definition_id: Some(Uuid::new_v4().to_string()),
        });
        crate::tests_network::seed_test_plan_adapter_definitions(&db.repo, &input).await;
        created_plans.push(
            db.repo
                .record_tunnel_plan(&input, &plan_tunnel(&input).unwrap(), true, &operator)
                .await
                .unwrap(),
        );
    }
    let staged_plan = &created_plans[0];
    db.repo
        .mark_pending_tunnel_plans_reconciled(&[staged_plan.id])
        .await
        .unwrap();
    db.repo
        .stage_tunnel_plan_ospf_jobs(
            staged_plan.id,
            staged_plan.revision,
            None,
            None,
            None,
            Uuid::new_v4(),
            Uuid::new_v4(),
            &operator,
        )
        .await
        .unwrap();
    assert!(sqlx::query_scalar::<_, Option<String>>(
        "SELECT pending_ospf_reconciled_at::text FROM tunnel_plans WHERE id = $1",
    )
    .bind(staged_plan.id)
    .fetch_one(&db.pool)
    .await
    .unwrap()
    .is_none());
    sqlx::query(
        "UPDATE tunnel_plans SET ospf_status = 'pending', left_ospf_status = 'pending', right_ospf_status = 'pending'",
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let first = db
        .repo
        .list_automatic_tunnel_plan_ids_for_controller(3)
        .await
        .unwrap();
    db.repo
        .mark_automatic_tunnel_plans_scanned(&first)
        .await
        .unwrap();
    let second = db
        .repo
        .list_automatic_tunnel_plan_ids_for_controller(3)
        .await
        .unwrap();
    assert!(first.iter().all(|plan_id| !second.contains(plan_id)));

    let pending_first = db
        .repo
        .list_pending_tunnel_plan_ids_for_reconciliation(3)
        .await
        .unwrap();
    db.repo
        .mark_pending_tunnel_plans_reconciled(&pending_first)
        .await
        .unwrap();
    let pending_second = db
        .repo
        .list_pending_tunnel_plan_ids_for_reconciliation(3)
        .await
        .unwrap();
    assert!(pending_first
        .iter()
        .all(|plan_id| !pending_second.contains(plan_id)));
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_ospf_controller_advances_past_malformed_selected_plans() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "ospf-poison-left", None).await;
    insert_client(&db.pool, "ospf-poison-right", None).await;
    let operator = postgres_network_operator(&db.repo).await;

    let mut input = postgres_alert_test_tunnel_input();
    input.name = "ospf-poison".to_string();
    input.interface_name = "op0".to_string();
    input.left_client_id = "ospf-poison-left".to_string();
    input.right_client_id = "ospf-poison-right".to_string();
    input.address_pool_cidr = "10.90.0.0/30".to_string();
    input.ipv4_tunnel = Some(TunnelAddressPair {
        left: "10.90.0.0".to_string(),
        right: "10.90.0.1".to_string(),
        prefix_len: 31,
    });
    input.ospf = Some(TunnelOspfConfig {
        mode: OspfControlMode::Automatic,
        planned_latency_ms: 20.0,
        planned_packet_loss_ratio: 0.0,
        preference: 1.0,
        policy: OspfCostPolicy::default(),
        min_cost_delta: 5,
        healthy_windows: 1,
        left_adapter_definition_id: Some(Uuid::new_v4().to_string()),
        right_adapter_definition_id: Some(Uuid::new_v4().to_string()),
    });
    crate::tests_network::seed_test_plan_adapter_definitions(&db.repo, &input).await;
    let malformed = db
        .repo
        .record_tunnel_plan(&input, &plan_tunnel(&input).unwrap(), true, &operator)
        .await
        .unwrap();

    input.name = "ospf-healthy-after-poison".to_string();
    input.interface_name = "op1".to_string();
    input.address_pool_cidr = "10.90.0.4/30".to_string();
    input.ipv4_tunnel = Some(TunnelAddressPair {
        left: "10.90.0.4".to_string(),
        right: "10.90.0.5".to_string(),
        prefix_len: 31,
    });
    let healthy = db
        .repo
        .record_tunnel_plan(&input, &plan_tunnel(&input).unwrap(), true, &operator)
        .await
        .unwrap();

    sqlx::query(
        r#"
        UPDATE tunnel_plans
        SET input = '{}'::jsonb,
            plan = '{"ospf":{"mode":"automatic"}}'::jsonb,
            ospf_status = 'pending',
            left_ospf_status = 'pending',
            right_ospf_status = 'pending',
            updated_at = now() - interval '10 minutes'
        WHERE id = $1
        "#,
    )
    .bind(malformed.id)
    .execute(&db.pool)
    .await
    .unwrap();

    crate::network_ospf_controller::run_controller_sweep(&postgres_app_state(&db))
        .await
        .unwrap();

    let malformed_markers = sqlx::query_as::<_, (bool, bool)>(
        r#"
        SELECT
            automatic_ospf_scanned_at IS NOT NULL,
            pending_ospf_reconciled_at IS NOT NULL
        FROM tunnel_plans
        WHERE id = $1
        "#,
    )
    .bind(malformed.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(malformed_markers, (true, true));
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT automatic_ospf_scanned_at IS NOT NULL FROM tunnel_plans WHERE id = $1",
    )
    .bind(healthy.id)
    .fetch_one(&db.pool)
    .await
    .unwrap());
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_ospf_results_are_atomic_and_concurrency_safe() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "ospf-result-left", None).await;
    insert_client(&db.pool, "ospf-result-right", None).await;
    let operator = postgres_network_operator(&db.repo).await;
    let mut input = postgres_alert_test_tunnel_input();
    input.name = "ospf-result-atomic".to_string();
    input.interface_name = "or0".to_string();
    input.left_client_id = "ospf-result-left".to_string();
    input.right_client_id = "ospf-result-right".to_string();
    input.address_pool_cidr = "10.91.0.0/30".to_string();
    input.ipv4_tunnel = Some(TunnelAddressPair {
        left: "10.91.0.0".to_string(),
        right: "10.91.0.1".to_string(),
        prefix_len: 31,
    });
    input.ospf = Some(TunnelOspfConfig {
        mode: OspfControlMode::Automatic,
        planned_latency_ms: 20.0,
        planned_packet_loss_ratio: 0.0,
        preference: 1.0,
        policy: OspfCostPolicy::default(),
        min_cost_delta: 5,
        healthy_windows: 1,
        left_adapter_definition_id: Some(Uuid::new_v4().to_string()),
        right_adapter_definition_id: Some(Uuid::new_v4().to_string()),
    });
    crate::tests_network::seed_test_plan_adapter_definitions(&db.repo, &input).await;
    let plan = db
        .repo
        .record_tunnel_plan(&input, &plan_tunnel(&input).unwrap(), true, &operator)
        .await
        .unwrap();
    let left_job_id = Uuid::new_v4();
    let right_job_id = Uuid::new_v4();
    db.repo
        .stage_tunnel_plan_ospf_jobs(
            plan.id,
            plan.revision,
            None,
            None,
            None,
            left_job_id,
            right_job_id,
            &operator,
        )
        .await
        .unwrap();

    sqlx::query(
        r#"
        CREATE FUNCTION reject_test_ospf_aggregate_update() RETURNS trigger
        LANGUAGE plpgsql AS $$
        BEGIN
            RAISE EXCEPTION 'forced OSPF aggregate failure';
        END
        $$
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        CREATE TRIGGER reject_test_ospf_aggregate_update
        BEFORE UPDATE OF ospf_status ON tunnel_plans
        FOR EACH ROW EXECUTE FUNCTION reject_test_ospf_aggregate_update()
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let error = db
        .repo
        .record_tunnel_plan_ospf_job_result(
            plan.id,
            vpsman_common::TunnelEndpointSide::Left,
            left_job_id,
            Some(100),
            true,
        )
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("forced OSPF aggregate failure"));
    assert_eq!(
        sqlx::query_as::<_, (String, String, Option<i32>)>(
            r#"
            SELECT left_ospf_status, ospf_status, left_current_ospf_cost
            FROM tunnel_plans
            WHERE id = $1
            "#,
        )
        .bind(plan.id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        ("pending".to_string(), "pending".to_string(), None)
    );

    sqlx::query("DROP TRIGGER reject_test_ospf_aggregate_update ON tunnel_plans")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION reject_test_ospf_aggregate_update()")
        .execute(&db.pool)
        .await
        .unwrap();

    let left_result = db.repo.record_tunnel_plan_ospf_job_result(
        plan.id,
        vpsman_common::TunnelEndpointSide::Left,
        left_job_id,
        Some(100),
        true,
    );
    let right_result = db.repo.record_tunnel_plan_ospf_job_result(
        plan.id,
        vpsman_common::TunnelEndpointSide::Right,
        right_job_id,
        Some(100),
        true,
    );
    let (left_result, right_result) = tokio::join!(left_result, right_result);
    assert!(left_result.unwrap().is_some());
    assert!(right_result.unwrap().is_some());
    assert_eq!(
        sqlx::query_as::<_, (String, String, String, Option<i32>, Option<i32>)>(
            r#"
            SELECT
                ospf_status,
                left_ospf_status,
                right_ospf_status,
                left_current_ospf_cost,
                right_current_ospf_cost
            FROM tunnel_plans
            WHERE id = $1
            "#,
        )
        .bind(plan.id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (
            "verified".to_string(),
            "verified".to_string(),
            "verified".to_string(),
            Some(100),
            Some(100),
        )
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_legacy_invalid_schedule_cadences_remain_visible_and_repairable() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "legacy-cadence-client", None).await;
    let operator = postgres_network_operator(&db.repo).await;

    let valid = db
        .repo
        .create_schedule(
            postgres_shell_schedule_request("cadence-valid", "legacy-cadence-client"),
            &operator,
        )
        .await
        .unwrap();
    let impossible = db
        .repo
        .create_schedule(
            postgres_shell_schedule_request("cadence-impossible", "legacy-cadence-client"),
            &operator,
        )
        .await
        .unwrap();
    let malformed = db
        .repo
        .create_schedule(
            postgres_shell_schedule_request("cadence-malformed", "legacy-cadence-client"),
            &operator,
        )
        .await
        .unwrap();
    sqlx::query(
        r#"
        UPDATE schedules
        SET cron_expr = CASE
            WHEN id = $1 THEN '0 0 31 2 *'
            WHEN id = $2 THEN 'not a cron'
            ELSE cron_expr
        END
        WHERE id IN ($1, $2)
        "#,
    )
    .bind(impossible.id)
    .bind(malformed.id)
    .execute(&db.pool)
    .await
    .unwrap();

    let schedules = db
        .repo
        .query_schedules(&ListQuery {
            limit: Some(10),
            q: Some("cadence-".to_string()),
            sort: Some("name".to_string()),
            dir: Some("asc".to_string()),
            ..ListQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(schedules.len(), 3);
    assert_eq!(
        schedules
            .iter()
            .find(|schedule| schedule.id == valid.id)
            .unwrap()
            .cadence_error,
        None
    );
    let impossible_view = schedules
        .iter()
        .find(|schedule| schedule.id == impossible.id)
        .unwrap();
    assert!(impossible_view.next_runs.is_empty());
    assert_eq!(
        impossible_view.cadence_error.as_deref(),
        Some("schedule_cron_no_future_occurrence")
    );
    let malformed_view = db.repo.schedule_by_id(malformed.id).await.unwrap();
    assert!(malformed_view.next_runs.is_empty());
    assert_eq!(
        malformed_view.cadence_error.as_deref(),
        Some("schedule_cron_invalid")
    );

    let backup = db
        .repo
        .create_backup_policy(
            CreateBackupPolicyRequest {
                name: "cadence-backup".to_string(),
                selector_expression: String::new(),
                target_client_ids: vec!["legacy-cadence-client".to_string()],
                paths: vec!["/etc/hostname".to_string()],
                include_config: false,
                follow_symlinks: false,
                missing_path_policy: vpsman_common::BackupMissingPathPolicy::Fail,
                retention_days: Some(7),
                keep_last: Some(2),
                rotation_generation: None,
                cron_expr: "0 3 * * *".to_string(),
                timezone: "UTC".to_string(),
                enabled: false,
                catch_up_policy: "skip_missed".to_string(),
                catch_up_limit: 1,
                retry_delay_secs: 120,
                max_failures: 3,
                confirmed: true,
                privilege_assertion: None,
            },
            &operator,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE schedules SET cron_expr = '0 0 31 2 *' WHERE id = $1")
        .bind(backup.schedule_id)
        .execute(&db.pool)
        .await
        .unwrap();
    let backup_view = db
        .repo
        .list_backup_policies(&ListQuery::default())
        .await
        .unwrap()
        .into_iter()
        .find(|policy| policy.schedule_id == backup.schedule_id)
        .unwrap();
    assert!(backup_view.next_runs.is_empty());
    assert_eq!(
        backup_view.cadence_error.as_deref(),
        Some("schedule_cron_no_future_occurrence")
    );
    let repaired_backup = db
        .repo
        .update_backup_policy(
            backup.schedule_id,
            CreateBackupPolicyRequest {
                name: "cadence-backup-repaired".to_string(),
                selector_expression: String::new(),
                target_client_ids: vec!["legacy-cadence-client".to_string()],
                paths: vec!["/etc/hostname".to_string()],
                include_config: true,
                follow_symlinks: false,
                missing_path_policy: vpsman_common::BackupMissingPathPolicy::Skip,
                retention_days: Some(14),
                keep_last: Some(4),
                rotation_generation: None,
                cron_expr: "30 3 * * *".to_string(),
                timezone: "UTC".to_string(),
                enabled: true,
                catch_up_policy: "skip_missed".to_string(),
                catch_up_limit: 1,
                retry_delay_secs: 120,
                max_failures: 3,
                confirmed: true,
                privilege_assertion: None,
            },
            &crate::repository_schedules::ScheduleSnapshotExpectation {
                selector_expression: backup.selector_expression.clone(),
                target_client_ids: backup.target_client_ids.clone(),
                definition_revision: backup.definition_revision,
            },
            &operator,
        )
        .await
        .unwrap()
        .expect("existing backup policy should remain updateable");
    assert_eq!(repaired_backup.schedule_id, backup.schedule_id);
    assert_eq!(repaired_backup.cron_expr, "30 3 * * *");
    assert!(repaired_backup.cadence_error.is_none());
    assert!(repaired_backup.enabled);
    assert_eq!(repaired_backup.retention_days, 14);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM schedules WHERE id = $1")
            .bind(backup.schedule_id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1
    );

    let state = postgres_app_state(&db);
    let session = db
        .repo
        .issue_session(operator.operator.clone())
        .await
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        format!("Bearer {}", session.access_token).parse().unwrap(),
    );
    let error = crate::routes_schedules::enable_schedule(
        axum::extract::State(state),
        headers,
        axum::extract::Path(impossible.id),
        axum::Json(SchedulePrivilegeMutationRequest {
            expected_definition_revision: impossible_view.definition_revision,
            confirmed: true,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "schedule_cron_invalid");

    let repair = postgres_shell_schedule_request("cadence-impossible", "legacy-cadence-client");
    let repaired = db
        .repo
        .update_schedule_record(
            impossible.id,
            crate::repository_schedules::ScheduleCreateInput {
                name: repair.name,
                operation: repair.operation,
                event_argv_template: repair.event_argv_template,
                selector_expression: repair.selector_expression,
                target_client_ids: repair.target_client_ids,
                trigger_kind: repair.trigger_kind,
                cron_expr: repair.cron_expr,
                timezone: repair.timezone,
                event_expression: repair.event_expression,
                enabled: repair.enabled,
                catch_up_policy: repair.catch_up_policy,
                catch_up_limit: repair.catch_up_limit,
                retry_delay_secs: repair.retry_delay_secs,
                max_failures: repair.max_failures,
                expected_definition_revision: Some(impossible_view.definition_revision),
            },
            None,
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(repaired.cadence_error, None);
    assert!(!repaired.next_runs.is_empty());

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_malformed_schedule_operation_is_listable_isolated_and_repairable() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let operator = postgres_network_operator(&db.repo).await;
    insert_client(&db.pool, "malformed-schedule-client", None).await;
    let malformed = db
        .repo
        .create_schedule(
            postgres_shell_schedule_request(
                "malformed-schedule-operation",
                "malformed-schedule-client",
            ),
            &operator,
        )
        .await
        .unwrap();
    let healthy = db
        .repo
        .create_schedule(
            postgres_shell_schedule_request(
                "healthy-schedule-operation",
                "malformed-schedule-client",
            ),
            &operator,
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE schedules SET operation = '{\"type\":\"removed_legacy_operation\"}'::jsonb WHERE id = $1",
    )
    .bind(malformed.id)
    .execute(&db.pool)
    .await
    .unwrap();

    let page = db
        .repo
        .query_schedules(&ListQuery {
            limit: Some(10),
            q: Some("schedule-operation".to_string()),
            ..ListQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(page.len(), 2);
    assert!(page.iter().any(|schedule| schedule.id == healthy.id));
    let visible = page
        .iter()
        .find(|schedule| schedule.id == malformed.id)
        .unwrap();
    assert!(visible.operation.is_none());
    assert_eq!(
        visible.operation_error.as_deref(),
        Some("schedule_operation_invalid")
    );
    assert_eq!(visible.operation_payload_hash.len(), 64);
    assert!(db
        .repo
        .update_schedule_targets(
            malformed.id,
            vec!["malformed-schedule-client".to_string()],
            None,
            &operator,
        )
        .await
        .is_err());
    assert!(db
        .repo
        .set_schedule_enabled(malformed.id, true, malformed.definition_revision, &operator)
        .await
        .is_err());
    let disabled = db
        .repo
        .set_schedule_enabled(
            malformed.id,
            false,
            malformed.definition_revision,
            &operator,
        )
        .await
        .unwrap();
    assert!(!disabled.enabled);

    let repair =
        postgres_shell_schedule_request("repaired-schedule-operation", "malformed-schedule-client");
    let repaired = db
        .repo
        .update_schedule_record(
            malformed.id,
            crate::repository_schedules::ScheduleCreateInput {
                name: repair.name,
                operation: repair.operation,
                event_argv_template: repair.event_argv_template,
                selector_expression: repair.selector_expression,
                target_client_ids: repair.target_client_ids,
                trigger_kind: repair.trigger_kind,
                cron_expr: repair.cron_expr,
                timezone: repair.timezone,
                event_expression: repair.event_expression,
                enabled: false,
                catch_up_policy: repair.catch_up_policy,
                catch_up_limit: repair.catch_up_limit,
                retry_delay_secs: repair.retry_delay_secs,
                max_failures: repair.max_failures,
                expected_definition_revision: Some(disabled.definition_revision),
            },
            None,
            &operator,
        )
        .await
        .unwrap();
    assert!(repaired.operation.is_some());
    assert!(repaired.operation_error.is_none());
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_audited_mutations_roll_back_when_audit_insert_fails() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "atomic-a", None).await;
    insert_client(&db.pool, "atomic-b", None).await;
    let operator = postgres_network_operator(&db.repo).await;

    let schedule = db
        .repo
        .create_schedule(
            postgres_shell_schedule_request("atomic-existing-schedule", "atomic-a"),
            &operator,
        )
        .await
        .unwrap();
    let command_template = db
        .repo
        .upsert_command_template(
            &UpsertCommandTemplateRequest {
                name: "atomic-command".to_string(),
                scope_kind: "global".to_string(),
                scope_value: None,
                display_group: None,
                operation: serde_json::json!({
                    "type": "shell",
                    "argv": ["/usr/bin/uptime"],
                    "pty": false
                }),
                defaults: serde_json::json!({}),
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    let builtin_generator = db
        .repo
        .list_runtime_config_patch_generators()
        .await
        .unwrap()
        .into_iter()
        .find(|generator| generator.built_in)
        .unwrap();
    let patch_generator = db
        .repo
        .upsert_runtime_config_patch_generator(
            &UpsertRuntimeConfigPatchGeneratorRequest {
                id: None,
                name: "Atomic generator".to_string(),
                category: builtin_generator.category.clone(),
                domain: builtin_generator.domain.clone(),
                description: "Atomic rollback fixture".to_string(),
                field_schema: builtin_generator.field_schema.clone(),
                raw_generator_body: builtin_generator.raw_generator_body.clone(),
                docs_metadata: builtin_generator.docs_metadata.clone(),
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    let mut tunnel_input = postgres_alert_test_tunnel_input();
    tunnel_input.name = "atomic-ospf-plan".to_string();
    tunnel_input.interface_name = "gre77".to_string();
    tunnel_input.kind = TunnelKind::Gre;
    tunnel_input.left_mtu = vpsman_common::default_tunnel_mtu(TunnelKind::Gre);
    tunnel_input.right_mtu = vpsman_common::default_tunnel_mtu(TunnelKind::Gre);
    tunnel_input.left_client_id = "atomic-a".to_string();
    tunnel_input.right_client_id = "atomic-b".to_string();
    tunnel_input.runtime_control = RuntimeTunnelControl {
        manager: RuntimeTunnelManager::AgentBuiltin,
        ..Default::default()
    };
    tunnel_input.ospf = Some(vpsman_common::TunnelOspfConfig {
        mode: vpsman_common::OspfControlMode::Reviewed,
        planned_latency_ms: 10.0,
        planned_packet_loss_ratio: 0.0,
        preference: 1.0,
        policy: vpsman_common::OspfCostPolicy::default(),
        min_cost_delta: 5,
        healthy_windows: 2,
        left_adapter_definition_id: Some(Uuid::new_v4().to_string()),
        right_adapter_definition_id: Some(Uuid::new_v4().to_string()),
    });
    crate::tests_network::seed_test_plan_adapter_definitions(&db.repo, &tunnel_input).await;
    let tunnel_plan = plan_tunnel(&tunnel_input).unwrap();
    let tunnel = db
        .repo
        .record_tunnel_plan(&tunnel_input, &tunnel_plan, true, &operator)
        .await
        .unwrap();

    install_rejected_audit_action_trigger(&db.pool).await;

    set_rejected_audit_action(&db.pool, "schedule.created").await;
    assert_forced_audit_failure(
        db.repo
            .create_schedule(
                postgres_shell_schedule_request("atomic-new-schedule", "atomic-a"),
                &operator,
            )
            .await,
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM schedules WHERE name = $1")
            .bind("atomic-new-schedule")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        0
    );

    set_rejected_audit_action(&db.pool, "schedule.updated").await;
    assert_forced_audit_failure(
        db.repo
            .update_schedule_record(
                schedule.id,
                crate::repository_schedules::ScheduleCreateInput {
                    name: "atomic-updated-schedule".to_string(),
                    operation: Some(JobCommand::Shell {
                        argv: vec!["/usr/bin/uptime".to_string()],
                        pty: false,
                    }),
                    event_argv_template: None,
                    selector_expression: "id:atomic-a".to_string(),
                    target_client_ids: vec!["atomic-a".to_string()],
                    trigger_kind: ScheduleTriggerKind::Cron,
                    cron_expr: Some("30 * * * *".to_string()),
                    timezone: Some("UTC".to_string()),
                    event_expression: None,
                    enabled: true,
                    catch_up_policy: Some("skip_missed".to_string()),
                    catch_up_limit: Some(1),
                    retry_delay_secs: Some(120),
                    max_failures: 2,
                    expected_definition_revision: Some(schedule.definition_revision),
                },
                None,
                &operator,
            )
            .await,
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT name FROM schedules WHERE id = $1")
            .bind(schedule.id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        "atomic-existing-schedule"
    );

    set_rejected_audit_action(&db.pool, "schedule.targets_updated").await;
    let schedule_snapshot = crate::repository_schedules::ScheduleSnapshotExpectation {
        selector_expression: schedule.selector_expression.clone(),
        target_client_ids: schedule.target_client_ids.clone(),
        definition_revision: schedule.definition_revision,
    };
    assert_forced_audit_failure(
        db.repo
            .update_schedule_targets(
                schedule.id,
                vec!["atomic-b".to_string()],
                Some(&schedule_snapshot),
                &operator,
            )
            .await,
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT selector_expression FROM schedules WHERE id = $1",)
            .bind(schedule.id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        ""
    );

    set_rejected_audit_action(&db.pool, "schedule.disabled").await;
    assert_forced_audit_failure(
        db.repo
            .set_schedule_enabled(schedule.id, false, schedule.definition_revision, &operator)
            .await,
    );
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT enabled FROM schedules WHERE id = $1")
            .bind(schedule.id)
            .fetch_one(&db.pool)
            .await
            .unwrap()
    );

    set_rejected_audit_action(&db.pool, "schedule.deferred").await;
    assert_forced_audit_failure(
        db.repo
            .defer_schedule(
                schedule.id,
                "2030-01-01T00:00:00Z",
                Some("atomic rollback"),
                schedule.definition_revision,
                &operator,
            )
            .await,
    );
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT deferred_until IS NULL FROM schedules WHERE id = $1",
    )
    .bind(schedule.id)
    .fetch_one(&db.pool)
    .await
    .unwrap());

    set_rejected_audit_action(&db.pool, "schedule.deleted").await;
    assert_forced_audit_failure(
        db.repo
            .soft_delete_schedule(schedule.id, schedule.definition_revision, &operator)
            .await,
    );
    assert_eq!(
        sqlx::query_as::<_, (bool, bool)>(
            "SELECT enabled, deleted_at IS NULL FROM schedules WHERE id = $1",
        )
        .bind(schedule.id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (true, true)
    );

    set_rejected_audit_action(&db.pool, "command_template.upserted").await;
    assert_forced_audit_failure(
        db.repo
            .upsert_command_template(
                &UpsertCommandTemplateRequest {
                    name: "atomic-command".to_string(),
                    scope_kind: "global".to_string(),
                    scope_value: None,
                    display_group: Some("changed".to_string()),
                    operation: serde_json::json!({
                        "type": "shell",
                        "argv": ["/usr/bin/uptime"],
                        "pty": false
                    }),
                    defaults: serde_json::json!({}),
                    confirmed: true,
                },
                &operator,
            )
            .await,
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT display_group FROM command_templates WHERE id = $1",
        )
        .bind(command_template.id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        command_template.display_group
    );

    set_rejected_audit_action(&db.pool, "command_template.deleted").await;
    assert_forced_audit_failure(
        db.repo
            .delete_command_template(command_template.id, &command_template.name, &operator)
            .await,
    );
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM command_templates WHERE id = $1)",
    )
    .bind(command_template.id)
    .fetch_one(&db.pool)
    .await
    .unwrap());

    let history_policy_before =
        sqlx::query_as::<_, (i32, i32, bool, bool, bool, Option<String>, Option<Uuid>)>(
            r#"
        SELECT
            retention_days,
            prune_limit,
            enabled,
            metadata_only,
            export_enabled,
            notes,
            updated_by
        FROM history_retention_policies
        WHERE domain = 'audit_logs'
        "#,
        )
        .fetch_optional(&db.pool)
        .await
        .unwrap();
    set_rejected_audit_action(&db.pool, "history_retention.policy_updated").await;
    assert_forced_audit_failure(
        db.repo
            .upsert_history_retention_policy(
                UpsertHistoryRetentionPolicyRequest {
                    domain: "audit_logs".to_string(),
                    retention_days: Some(30),
                    prune_limit: Some(100),
                    enabled: Some(true),
                    metadata_only: Some(false),
                    export_enabled: Some(false),
                    notes: Some("atomic rollback".to_string()),
                    clear_notes: false,
                    confirmed: true,
                },
                &operator,
            )
            .await,
    );
    let history_policy_after =
        sqlx::query_as::<_, (i32, i32, bool, bool, bool, Option<String>, Option<Uuid>)>(
            r#"
        SELECT
            retention_days,
            prune_limit,
            enabled,
            metadata_only,
            export_enabled,
            notes,
            updated_by
        FROM history_retention_policies
        WHERE domain = 'audit_logs'
        "#,
        )
        .fetch_optional(&db.pool)
        .await
        .unwrap();
    assert_eq!(history_policy_after, history_policy_before);

    set_rejected_audit_action(&db.pool, "runtime_config_patch_generator.saved").await;
    assert_forced_audit_failure(
        db.repo
            .upsert_runtime_config_patch_generator(
                &UpsertRuntimeConfigPatchGeneratorRequest {
                    id: Some(patch_generator.id),
                    name: "Atomic generator changed".to_string(),
                    category: patch_generator.category.clone(),
                    domain: patch_generator.domain.clone(),
                    description: patch_generator.description.clone(),
                    field_schema: patch_generator.field_schema.clone(),
                    raw_generator_body: patch_generator.raw_generator_body.clone(),
                    docs_metadata: patch_generator.docs_metadata.clone(),
                    confirmed: true,
                },
                &operator,
            )
            .await,
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT name FROM runtime_config_patch_generators WHERE id = $1",
        )
        .bind(patch_generator.id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        "Atomic generator"
    );

    set_rejected_audit_action(&db.pool, "runtime_config_patch_generator.deleted").await;
    assert_forced_audit_failure(
        db.repo
            .delete_runtime_config_patch_generator(
                patch_generator.id,
                &patch_generator.name,
                &operator,
            )
            .await,
    );
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM runtime_config_patch_generators WHERE id = $1)",
    )
    .bind(patch_generator.id)
    .fetch_one(&db.pool)
    .await
    .unwrap());

    set_rejected_audit_action(&db.pool, "backup_policy.upserted").await;
    assert_forced_audit_failure(
        db.repo
            .create_backup_policy(
                CreateBackupPolicyRequest {
                    name: "atomic-backup-policy".to_string(),
                    selector_expression: "id:atomic-a".to_string(),
                    target_client_ids: vec!["atomic-a".to_string()],
                    paths: vec!["/etc/hostname".to_string()],
                    include_config: true,
                    follow_symlinks: false,
                    missing_path_policy: vpsman_common::BackupMissingPathPolicy::Fail,
                    retention_days: Some(30),
                    keep_last: Some(7),
                    rotation_generation: None,
                    cron_expr: "0 3 * * *".to_string(),
                    timezone: "UTC".to_string(),
                    enabled: true,
                    catch_up_policy: "skip_missed".to_string(),
                    catch_up_limit: 1,
                    retry_delay_secs: 300,
                    max_failures: 3,
                    confirmed: true,
                    privilege_assertion: None,
                },
                &operator,
            )
            .await,
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM schedules WHERE name = $1")
            .bind("atomic-backup-policy")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_logs WHERE action = 'schedule.created' AND metadata->>'name' = $1",
        )
        .bind("atomic-backup-policy")
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0
    );

    set_rejected_audit_action(&db.pool, "network.tunnel_plan_disabled").await;
    assert_forced_audit_failure(
        db.repo
            .set_tunnel_plan_enabled(tunnel.id, tunnel.revision, false, &operator)
            .await,
    );
    let enabled_state = sqlx::query_as::<_, (bool, i64)>(
        "SELECT enabled, revision FROM tunnel_plans WHERE id = $1",
    )
    .bind(tunnel.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(enabled_state, (true, tunnel.revision));

    set_rejected_audit_action(&db.pool, "network.ospf_jobs_staged").await;
    assert_forced_audit_failure(
        db.repo
            .stage_tunnel_plan_ospf_jobs(
                tunnel.id,
                tunnel.revision,
                None,
                None,
                None,
                Uuid::new_v4(),
                Uuid::new_v4(),
                &operator,
            )
            .await,
    );
    let ospf_state = sqlx::query_as::<_, (String, Option<Uuid>, Option<Uuid>)>(
        "SELECT ospf_status, left_ospf_job_id, right_ospf_job_id FROM tunnel_plans WHERE id = $1",
    )
    .bind(tunnel.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(ospf_state, ("unverified".to_string(), None, None));

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_fleet_summary_accounts_for_disconnected_and_missing_contact_states() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    for (index, status, last_seen_at) in [
        (1_u8, "online", Some("2026-07-12T12:00:00Z")),
        (2, "online", None),
        (3, "disconnected", Some("2026-07-12T11:59:00Z")),
        (4, "never", None),
        (5, "stale", Some("2026-07-11T12:00:00Z")),
    ] {
        sqlx::query(
            r#"
            INSERT INTO clients (id, display_name, public_key, status, last_seen_at)
            VALUES ($1, $1, $2, $3, $4::timestamptz)
            "#,
        )
        .bind(format!("fleet-summary-{index}"))
        .bind(vec![index; 32])
        .bind(status)
        .bind(last_seen_at)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, command_type, privileged, status, target_count, payload_hash,
            request_fingerprint, max_timeout_secs
        )
        SELECT
            md5('fleet-summary-job-' || status)::uuid, 'shell', false, status, 1,
            repeat('a', 64), 'fleet-summary-' || status, 30
        FROM unnest(ARRAY['queued', 'running', 'completed', 'skipped']) AS status
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let summary = db.repo.fleet_summary().await.unwrap();
    assert_eq!(summary.total, 5);
    assert_eq!(summary.online, 1);
    assert_eq!(summary.offline, 1);
    assert_eq!(summary.never, 1);
    assert_eq!(summary.stale, 1);
    assert_eq!(summary.unknown, 1);
    assert_eq!(summary.warnings, 4);
    assert_eq!(summary.running_jobs, 2);
    assert_eq!(
        summary.online + summary.offline + summary.never + summary.stale + summary.unknown,
        summary.total
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_inactive_session_rejects_async_ingest_without_mutation() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "inactive-async-ingest-client";
    let gateway_id = "gateway-a";
    let process_incarnation_id = Uuid::new_v4();
    let gateway_session_id = Uuid::new_v4();
    insert_client(&db.pool, client_id, Some(process_incarnation_id)).await;
    start_test_gateway_session(&db.repo, gateway_id, client_id, gateway_session_id).await;
    let telemetry_event = GatewayTelemetryIngest {
        gateway_id: gateway_id.to_string(),
        gateway_session_id,
        process_incarnation_id,
        telemetry_seq: 1,
        remote_ip: None,
        telemetry: TelemetryEnvelope {
            client_id: client_id.to_string(),
            metrics: AgentMetrics {
                observed_unix: crate::unix_now().max(1),
                hostname: client_id.to_string(),
                ..Default::default()
            },
        },
    };
    let mut expiry_tx = db.pool.begin().await.unwrap();
    sqlx::query(
        r#"
        UPDATE gateway_sessions
        SET status = 'expired', ended_at = now(), end_reason = 'agent_offline_timeout'
        WHERE id = $1
        "#,
    )
    .bind(gateway_session_id)
    .execute(&mut *expiry_tx)
    .await
    .unwrap();
    let record_repo = db.repo.clone();
    let record_event = telemetry_event.clone();
    let mut record_task =
        tokio::spawn(async move { record_repo.record_telemetry_outcome(&record_event).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut record_task)
            .await
            .is_err()
    );
    expiry_tx.commit().await.unwrap();
    assert_eq!(
        record_task.await.unwrap().unwrap(),
        crate::repository_ingest::TelemetryRecordOutcome::GatewaySessionNotActive
    );

    let state = postgres_app_state(&db);
    let headers = internal_gateway_headers();
    let telemetry_error = crate::routes_ingest::ingest_telemetry(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::Json(telemetry_event),
    )
    .await
    .unwrap_err();
    assert_eq!(telemetry_error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(telemetry_error.code, "gateway_session_not_active");

    let reload_error = crate::routes_ingest::request_runtime_config_reload(
        axum::extract::State(state),
        headers,
        axum::Json(GatewayRuntimeConfigReloadRequest {
            gateway_id: gateway_id.to_string(),
            gateway_session_id,
            process_incarnation_id,
            remote_ip: None,
            request: AgentRuntimeConfigReloadRequest {
                client_id: client_id.to_string(),
                current_content_hash: "a".repeat(64),
                reason: "agent_reconnect_authoritative_sync".to_string(),
                requires_authoritative_sync: true,
                reconcile_resources: Vec::new(),
                requires_port_forwarding_sync: false,
            },
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(reload_error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(reload_error.code, "gateway_session_not_active");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*)::bigint FROM telemetry_samples WHERE client_id = $1",
        )
        .bind(client_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*)::bigint FROM jobs")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        0
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_telemetry_ingest_is_sequence_bound_and_idempotent() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "telemetry-sequence-client";
    let process_incarnation_id = Uuid::new_v4();
    let gateway_session_id = Uuid::new_v4();
    insert_client(&db.pool, client_id, Some(process_incarnation_id)).await;
    start_test_gateway_session(&db.repo, "gateway-a", client_id, gateway_session_id).await;
    let ping_target_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO ping_targets (id, name, host, probe_kind, selector_expression)
        VALUES ($1, 'Telemetry sequence Ping', '192.0.2.90', 'icmp', '*')
        "#,
    )
    .bind(ping_target_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO ping_target_assignments (target_id, client_id, is_primary)
        VALUES ($1, $2, TRUE)
        "#,
    )
    .bind(ping_target_id)
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let mut event = GatewayTelemetryIngest {
        gateway_id: "gateway-a".to_string(),
        gateway_session_id,
        process_incarnation_id,
        telemetry_seq: 2,
        remote_ip: None,
        telemetry: TelemetryEnvelope {
            client_id: client_id.to_string(),
            metrics: AgentMetrics {
                observed_unix: 1,
                hostname: client_id.to_string(),
                cpu: CpuStat {
                    load: LoadAverage {
                        one: 1.0,
                        five: 0.8,
                        fifteen: 0.5,
                    },
                    cores: 2,
                    utilization_ratio: None,
                },
                memory: MemoryStat {
                    total_bytes: 200,
                    available_bytes: 50,
                    swap_total_bytes: Some(200),
                    swap_available_bytes: Some(50),
                },
                disks: vec![DiskStat {
                    mountpoint: "/".to_string(),
                    total_bytes: 200,
                    available_bytes: 50,
                }],
                networks: vec![NetworkStat {
                    interface: "eth0".to_string(),
                    rx_bytes: 100,
                    tx_bytes: 200,
                }],
                tunnels: vec![RuntimeTunnelStat {
                    interface: "wg0".to_string(),
                    rx_bytes: 300,
                    tx_bytes: 400,
                    ..RuntimeTunnelStat::default()
                }],
                ping_results: vec![
                    PingTargetResult {
                        target_id: ping_target_id.to_string(),
                        generation: 1,
                        checked_unix: 1,
                        status: "ok".to_string(),
                        latency_avg_ms: Some(12.5),
                        loss_ratio: 0.0,
                        reason: None,
                    },
                    PingTargetResult {
                        target_id: ping_target_id.to_string(),
                        generation: 1,
                        checked_unix: 1,
                        status: "degraded".to_string(),
                        latency_avg_ms: Some(25.0),
                        loss_ratio: 0.5,
                        reason: Some("duplicate winner".to_string()),
                    },
                ],
                ..AgentMetrics::default()
            },
        },
    };

    assert!(db.repo.record_telemetry(&event).await.unwrap());
    assert!(!db.repo.record_telemetry(&event).await.unwrap());
    event.telemetry_seq = 1;
    event.telemetry.metrics.cpu.load.one = 99.0;
    event.telemetry.metrics.cpu.cores = 64;
    event.telemetry.metrics.memory = MemoryStat {
        total_bytes: 10_000,
        available_bytes: 0,
        swap_total_bytes: Some(10_000),
        swap_available_bytes: Some(0),
    };
    event.telemetry.metrics.disks[0].total_bytes = 10_000;
    event.telemetry.metrics.disks[0].available_bytes = 0;
    assert!(!db.repo.record_telemetry(&event).await.unwrap());
    // Cross an API receive-time second while resending the same cached source
    // Ping. Logical deduplication must not depend on the rebased chart second.
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    event.telemetry_seq = 3;
    event.telemetry.metrics.cpu.load.one = 3.0;
    event.telemetry.metrics.cpu.cores = 4;
    event.telemetry.metrics.memory = MemoryStat {
        total_bytes: 100,
        available_bytes: 75,
        swap_total_bytes: Some(100),
        swap_available_bytes: Some(75),
    };
    event.telemetry.metrics.disks[0].total_bytes = 100;
    event.telemetry.metrics.disks[0].available_bytes = 75;
    assert!(db.repo.record_telemetry(&event).await.unwrap());
    let reconnect_session_id = Uuid::new_v4();
    start_test_gateway_session(&db.repo, "gateway-a", client_id, reconnect_session_id).await;
    event.gateway_session_id = reconnect_session_id;
    event.telemetry_seq = 1;
    event.telemetry.metrics.cpu.load.one = 4.0;
    event.telemetry.metrics.cpu.cores = 8;
    event.telemetry.metrics.memory = MemoryStat {
        total_bytes: 400,
        available_bytes: 200,
        swap_total_bytes: Some(400),
        swap_available_bytes: Some(200),
    };
    event.telemetry.metrics.disks[0].total_bytes = 400;
    event.telemetry.metrics.disks[0].available_bytes = 200;
    assert!(db.repo.record_telemetry(&event).await.unwrap());
    event.telemetry_seq = 2;
    event.telemetry.metrics.memory.swap_total_bytes = Some(0);
    event.telemetry.metrics.memory.swap_available_bytes = Some(0);
    assert!(db.repo.record_telemetry(&event).await.unwrap());

    let sample_count: i64 = sqlx::query_scalar(
        "SELECT COALESCE(sum(sample_count), 0)::bigint FROM telemetry_rollups WHERE client_id = $1",
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(sample_count, 4);
    let (first_observed, last_observed): (f64, f64) = sqlx::query_as(
        r#"
        SELECT
            extract(epoch FROM min(observed_at))::double precision,
            extract(epoch FROM max(observed_at))::double precision
        FROM telemetry_samples
        WHERE client_id = $1
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(last_observed > first_observed);
    let raw_resources = db
        .repo
        .list_dashboard_raw_telemetry_rollups(
            10,
            first_observed as u64,
            last_observed as u64,
            60,
            &[client_id.to_string()],
        )
        .await
        .unwrap();
    assert_eq!(
        raw_resources
            .iter()
            .map(|point| point.sample_count)
            .sum::<i32>(),
        4
    );
    assert!(raw_resources.iter().all(|point| {
        point.connections_sample_count == point.sample_count
            && point.tcp_sockets_latest == Some(i64::MAX)
            && point.udp_sockets_latest == Some(i64::MAX)
    }));
    let (ping_fact_count, distinct_source_count, source_checked_unix, all_last_input_winners): (
        i64,
        i64,
        i64,
        bool,
    ) = sqlx::query_as(
        r#"
        SELECT
            count(*)::bigint,
            count(DISTINCT fact.source_checked_unix)::bigint,
            min(fact.source_checked_unix)::bigint,
            bool_and(
                fact.status = 'degraded'
                AND fact.latency_avg_ms = 25.0
                AND fact.loss_ratio = 0.5
                AND fact.reason = 'duplicate winner'
            )
        FROM telemetry_ping_facts fact
        JOIN telemetry_ping_series series ON series.id = fact.series_id
        WHERE series.client_id = $1
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(ping_fact_count, 1);
    assert_eq!(distinct_source_count, 1);
    assert_eq!(source_checked_unix, 1);
    assert!(all_last_input_winners);
    let counter_fact_counts = sqlx::query_as::<_, (String, i64)>(
        r#"
        SELECT source_kind, count(*)::bigint
        FROM telemetry_counter_facts
        WHERE client_id = $1
        GROUP BY source_kind
        ORDER BY source_kind
        "#,
    )
    .bind(client_id)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        counter_fact_counts,
        vec![("host".to_string(), 4), ("tunnel".to_string(), 4)]
    );
    let ping_history = db
        .repo
        .list_raw_ping_results(client_id, None, None, 10, 60)
        .await
        .unwrap();
    assert!(!ping_history.is_empty());
    assert!(ping_history.iter().all(|point| {
        point.target_id == ping_target_id
            && point.latest_status == "degraded"
            && point.latency_avg_ms == Some(25.0)
            && point.loss_ratio_avg == 0.5
            && point.latest_reason.as_deref() == Some("duplicate winner")
    }));
    let current_ping: (String, Option<f64>, f64, Option<String>) = sqlx::query_as(
        r#"
        SELECT latest_status, latency_avg_ms, rolling_loss_ratio, latest_reason
        FROM telemetry_ping_current
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        current_ping,
        (
            "degraded".to_string(),
            Some(25.0),
            0.5,
            Some("duplicate winner".to_string()),
        )
    );
    assert!(db
        .repo
        .raw_telemetry_covers_range_start(&[client_id.to_string()], first_observed as u64)
        .await
        .unwrap());
    let resource_rollup = sqlx::query(
        r#"
        SELECT
            max(cpu_cores_max) AS cpu_cores_max,
            max(memory_total_bytes_max) AS memory_total_bytes_max,
            round(
                sum(memory_available_bytes_sum) / sum(sample_count)::numeric
            )::bigint AS memory_available_bytes_avg,
            min(memory_available_bytes_min) AS memory_available_bytes_min,
            sum(memory_used_ratio_sum)
                / sum(sample_count)::double precision AS memory_used_ratio_avg,
            max(memory_used_ratio_max) AS memory_used_ratio_max,
            sum(swap_sample_count)::integer AS swap_sample_count,
            max(swap_total_bytes_max) AS swap_total_bytes_max,
            round(
                sum(swap_available_bytes_sum)
                    / nullif(sum(swap_sample_count), 0)::numeric
            )::bigint AS swap_available_bytes_avg,
            min(swap_available_bytes_min)
                FILTER (WHERE swap_sample_count > 0) AS swap_available_bytes_min,
            sum(swap_used_ratio_sum)
                / nullif(sum(swap_sample_count), 0)::double precision
                AS swap_used_ratio_avg,
            max(swap_used_ratio_max) AS swap_used_ratio_max,
            max(disk_total_bytes_max) AS disk_total_bytes_max,
            round(
                sum(disk_available_bytes_sum) / sum(sample_count)::numeric
            )::bigint AS disk_available_bytes_avg,
            min(disk_available_bytes_min) AS disk_available_bytes_min,
            sum(disk_used_ratio_sum)
                / sum(sample_count)::double precision AS disk_used_ratio_avg,
            max(disk_used_ratio_max) AS disk_used_ratio_max
        FROM telemetry_rollups
        WHERE client_id = $1
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(resource_rollup.get::<i32, _>("cpu_cores_max"), 8);
    assert_eq!(resource_rollup.get::<i64, _>("memory_total_bytes_max"), 400);
    assert_eq!(
        resource_rollup.get::<i64, _>("memory_available_bytes_avg"),
        131
    );
    assert_eq!(
        resource_rollup.get::<i64, _>("memory_available_bytes_min"),
        50
    );
    assert!((resource_rollup.get::<f64, _>("memory_used_ratio_avg") - 0.5).abs() < f64::EPSILON);
    assert!((resource_rollup.get::<f64, _>("memory_used_ratio_max") - 0.75).abs() < f64::EPSILON);
    assert_eq!(resource_rollup.get::<i32, _>("swap_sample_count"), 3);
    assert_eq!(
        resource_rollup.get::<Option<i64>, _>("swap_total_bytes_max"),
        Some(400)
    );
    assert_eq!(
        resource_rollup.get::<Option<i64>, _>("swap_available_bytes_avg"),
        Some(108)
    );
    assert_eq!(
        resource_rollup.get::<Option<i64>, _>("swap_available_bytes_min"),
        Some(50)
    );
    assert!(
        (resource_rollup
            .get::<Option<f64>, _>("swap_used_ratio_avg")
            .unwrap()
            - 0.5)
            .abs()
            < f64::EPSILON
    );
    assert!(
        (resource_rollup
            .get::<Option<f64>, _>("swap_used_ratio_max")
            .unwrap()
            - 0.75)
            .abs()
            < f64::EPSILON
    );
    assert_eq!(resource_rollup.get::<i64, _>("disk_total_bytes_max"), 400);
    assert_eq!(
        resource_rollup.get::<i64, _>("disk_available_bytes_avg"),
        131
    );
    assert_eq!(
        resource_rollup.get::<i64, _>("disk_available_bytes_min"),
        50
    );
    assert!((resource_rollup.get::<f64, _>("disk_used_ratio_avg") - 0.5).abs() < f64::EPSILON);
    assert!((resource_rollup.get::<f64, _>("disk_used_ratio_max") - 0.75).abs() < f64::EPSILON);
    for invalid_swap_state in [
        "swap_sample_count = 0, swap_total_bytes_max = NULL, swap_available_bytes_avg = 0, swap_available_bytes_min = 0, swap_used_ratio_avg = NULL, swap_used_ratio_max = NULL",
        "swap_sample_count = 0, swap_total_bytes_max = 0, swap_available_bytes_avg = NULL, swap_available_bytes_min = NULL, swap_used_ratio_avg = NULL, swap_used_ratio_max = NULL",
        "swap_sample_count = 1, swap_total_bytes_max = NULL, swap_available_bytes_avg = 0, swap_available_bytes_min = 0, swap_used_ratio_avg = 0, swap_used_ratio_max = 0",
    ] {
        let result = sqlx::query(&format!(
            "UPDATE telemetry_rollups SET {invalid_swap_state} WHERE client_id = $1"
        ))
        .bind(client_id)
        .execute(&db.pool)
        .await;
        assert!(result.is_err(), "invalid swap state was accepted");
    }
    let (gateway_session_id, telemetry_seq): (Uuid, i64) = sqlx::query_as(
        "SELECT gateway_session_id, telemetry_seq FROM telemetry_ingest_watermarks WHERE client_id = $1",
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(gateway_session_id, reconnect_session_id);
    assert_eq!(telemetry_seq, 2);
    let webhook_event_count: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM webhook_events WHERE kind = 'telemetry.rollup' AND event_id LIKE $1",
    )
    .bind(format!("telemetry:{client_id}:%"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(webhook_event_count, 4);

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_combined_telemetry_evidence_is_atomic_exact_and_repairs_only_receipts() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "telemetry-policy-atomic-client";
    let gateway_id = "telemetry-policy-atomic-gateway";
    let process_incarnation_id = Uuid::new_v4();
    let gateway_session_id = Uuid::new_v4();
    let group_id = Uuid::new_v4();
    let rule_id = Uuid::new_v4();
    insert_client(&db.pool, client_id, Some(process_incarnation_id)).await;
    start_test_gateway_session(&db.repo, gateway_id, client_id, gateway_session_id).await;
    sqlx::query(
        r#"
        INSERT INTO vps_rule_values (client_id, key, value_raw, value_json)
        VALUES
            ($1, 'traffic.reset_day', '-1', '{"day":-1}'::jsonb),
            (
                $1, 'traffic.selectors', 'eth0',
                '{"selectors":[{"source":"host","interface":"eth0","direction":"total","canonical":"eth0"}]}'::jsonb
            ),
            (
                $1, 'traffic.quota.total', '1KB',
                '{"bytes":1000,"display":"1 KB"}'::jsonb
            )
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    // Keep the accounting baseline before both deliberately skewed source
    // timestamps. The assertion is about the sample accepted in this
    // transaction, not wall-clock freshness, so using the current clock here
    // would exclude the baseline from the as-of traffic snapshot.
    let baseline_unix = 7_940_u64;
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source
        ) VALUES (
            $1, 'host', 'eth0', date_trunc('minute', to_timestamp($2::double precision)),
            10, 20, 0, 0, 'agent_networks'
        )
        "#,
    )
    .bind(client_id)
    .bind(baseline_unix as f64)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO policy_groups (id, name, enabled, selector_expression)
        VALUES ($1, 'Atomic telemetry metric test', TRUE, '*')
        "#,
    )
    .bind(group_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO policy_rules (
            id, group_id, name, enabled, trigger_condition_expression,
            severity, rule_kind, evidence_source, correlation_mode, category,
            title_template, detail_template
        ) VALUES (
            $1, $2, 'Atomic CPU high', TRUE,
            'cpu.utilization_ratio >= 0.9', 'critical', 'metric',
            'telemetry.combined', 'natural_key', 'resource',
            'CPU high', 'CPU is high for {subject.display_name}'
        )
        "#,
    )
    .bind(rule_id)
    .bind(group_id)
    .execute(&db.pool)
    .await
    .unwrap();

    sqlx::query(
        r#"
        CREATE FUNCTION reject_atomic_metric_state() RETURNS trigger
        LANGUAGE plpgsql AS $$
        BEGIN
            IF NEW.policy_rule_id = TG_ARGV[0]::uuid THEN
                RAISE EXCEPTION 'forced transient metric evaluation failure';
            END IF;
            RETURN NEW;
        END
        $$
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(&format!(
        r#"
        CREATE TRIGGER reject_atomic_metric_state
        BEFORE INSERT OR UPDATE ON alert_policy_evaluation_states
        FOR EACH ROW EXECUTE FUNCTION reject_atomic_metric_state('{}')
        "#,
        rule_id
    ))
    .execute(&db.pool)
    .await
    .unwrap();

    let telemetry_event = |telemetry_seq, reported_observed_unix, cpu_ratio, rx_bytes, tx_bytes| {
        GatewayTelemetryIngest {
            gateway_id: gateway_id.to_string(),
            gateway_session_id,
            process_incarnation_id,
            telemetry_seq,
            remote_ip: None,
            telemetry: TelemetryEnvelope {
                client_id: client_id.to_string(),
                metrics: AgentMetrics {
                    observed_unix: reported_observed_unix,
                    hostname: client_id.to_string(),
                    cpu: CpuStat {
                        load: LoadAverage {
                            one: cpu_ratio * 2.0,
                            five: cpu_ratio * 2.0,
                            fifteen: cpu_ratio * 2.0,
                        },
                        cores: 2,
                        utilization_ratio: Some(cpu_ratio),
                    },
                    networks: vec![NetworkStat {
                        interface: "eth0".to_string(),
                        rx_bytes,
                        tx_bytes,
                    }],
                    ..AgentMetrics::default()
                },
            },
        }
    };

    // The evaluator fails inside its savepoint. Accepted telemetry, its exact
    // raw sample, traffic snapshot, and immutable evidence must still commit.
    let high = telemetry_event(1, 9_000, 0.95, 110, 220);
    assert!(db.repo.record_telemetry(&high).await.unwrap());
    assert_eq!(
        db.repo
            .repair_combined_telemetry_policy_evidence(100)
            .await
            .unwrap(),
        0
    );
    let high_evidence = sqlx::query(
        r#"
        SELECT evidence.evidence_seq, evidence.source_event_id,
               evidence.observed_at, evidence.payload
        FROM alert_policy_evidence evidence
        WHERE evidence.source_kind='telemetry.combined'
          AND evidence.subject_client_id=$1
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let high_evidence_seq: i64 = high_evidence.try_get("evidence_seq").unwrap();
    let high_payload = high_evidence
        .try_get::<SqlJson<Value>, _>("payload")
        .unwrap()
        .0;
    assert_eq!(
        high_evidence
            .try_get::<String, _>("source_event_id")
            .unwrap(),
        format!("telemetry.combined:{gateway_session_id}:{process_incarnation_id}:1")
    );
    assert_eq!(high_payload["telemetry"]["seq"], 1);
    assert_eq!(high_payload["telemetry"]["reported_observed_unix"], 9_000);
    assert_eq!(high_payload["cpu"]["utilization_ratio"], 0.95);
    assert_eq!(high_payload["traffic"]["cycle"]["total"], 300);
    assert_eq!(high_payload["traffic"]["state"], "ok");
    assert!(high_payload["traffic"]["snapshot"]["selector_hash"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));
    let high_sample_id =
        Uuid::parse_str(high_payload["telemetry"]["sample_id"].as_str().unwrap()).unwrap();
    let stored_high_cpu: f64 =
        sqlx::query_scalar("SELECT cpu_utilization_ratio FROM telemetry_samples WHERE id=$1")
            .bind(high_sample_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(stored_high_cpu, 0.95);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM alert_policy_evidence_receipts WHERE policy_rule_id=$1 AND evidence_seq=$2",
        )
        .bind(rule_id)
        .bind(high_evidence_seq)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0
    );

    sqlx::query("DROP TRIGGER reject_atomic_metric_state ON alert_policy_evaluation_states")
        .execute(&db.pool)
        .await
        .unwrap();
    // Equal then regressing source timestamps are both accepted by increasing
    // sequence. Their receive-time facts remain distinct through evidence_seq.
    let low_equal_source_time = telemetry_event(2, 9_000, 0.10, 210, 420);
    let low_out_of_order_source_time = telemetry_event(3, 8_000, 0.10, 310, 620);
    assert!(db
        .repo
        .record_telemetry(&low_equal_source_time)
        .await
        .unwrap());
    assert!(db
        .repo
        .record_telemetry(&low_out_of_order_source_time)
        .await
        .unwrap());

    let evidence_rows = sqlx::query(
        r#"
        SELECT evidence_seq, observed_at, payload
        FROM alert_policy_evidence
        WHERE source_kind='telemetry.combined' AND subject_client_id=$1
        ORDER BY evidence_seq
        "#,
    )
    .bind(client_id)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(evidence_rows.len(), 3);
    let reported_times = evidence_rows
        .iter()
        .map(|row| {
            row.try_get::<SqlJson<Value>, _>("payload").unwrap().0["telemetry"]
                ["reported_observed_unix"]
                .as_u64()
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(reported_times, vec![9_000, 9_000, 8_000]);
    let evidence_observed = evidence_rows
        .iter()
        .map(|row| {
            row.try_get::<chrono::DateTime<Utc>, _>("observed_at")
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(evidence_observed[0], evidence_observed[1]);
    assert!(evidence_observed[2] < evidence_observed[1]);
    let first_payload = evidence_rows[0]
        .try_get::<SqlJson<Value>, _>("payload")
        .unwrap()
        .0;
    let last_payload = evidence_rows[2]
        .try_get::<SqlJson<Value>, _>("payload")
        .unwrap()
        .0;
    assert_eq!(first_payload["traffic"]["cycle"]["total"], 300);
    assert_eq!(last_payload["traffic"]["cycle"]["total"], 900);
    assert_ne!(
        first_payload["telemetry"]["sample_id"],
        last_payload["telemetry"]["sample_id"]
    );

    assert_eq!(
        crate::repository_policy_lifecycle::repair_missing_policy_evidence_receipts(&db.pool, 100)
            .await
            .unwrap(),
        1
    );
    let receipts = sqlx::query_as::<_, (i64, String)>(
        r#"
        SELECT evidence_seq, result
        FROM alert_policy_evidence_receipts
        WHERE policy_rule_id=$1
        ORDER BY evidence_seq
        "#,
    )
    .bind(rule_id)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        receipts,
        vec![
            (
                evidence_rows[0].try_get("evidence_seq").unwrap(),
                "stale".to_string()
            ),
            (
                evidence_rows[1].try_get("evidence_seq").unwrap(),
                "not_matched".to_string()
            ),
            (
                evidence_rows[2].try_get("evidence_seq").unwrap(),
                "stale".to_string()
            ),
        ]
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM alert_episodes WHERE policy_rule_id=$1")
            .bind(rule_id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        0
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_raw_ping_retention_keeps_the_logical_winner_independent_of_raw_samples() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "raw-ping-retention-client";
    let target_id = Uuid::new_v4();
    insert_client(&db.pool, client_id, None).await;
    sqlx::query(
        r#"
        INSERT INTO ping_targets (id, name, host, probe_kind, selector_expression)
        VALUES ($1, 'Raw Ping retention', '192.0.2.91', 'icmp', '*')
        "#,
    )
    .bind(target_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO ping_target_assignments (target_id, client_id, is_primary)
        VALUES ($1, $2, TRUE)
        "#,
    )
    .bind(target_id)
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let checked_unix = 1_800_000_000;
    let older_metrics = AgentMetrics {
        observed_unix: checked_unix + 10,
        hostname: client_id.to_string(),
        ping_results: vec![PingTargetResult {
            target_id: target_id.to_string(),
            generation: 1,
            checked_unix,
            status: "ok".to_string(),
            latency_avg_ms: Some(10.0),
            loss_ratio: 0.0,
            reason: None,
        }],
        ..AgentMetrics::default()
    };
    let newer_metrics = AgentMetrics {
        observed_unix: checked_unix + 20,
        ping_results: vec![PingTargetResult {
            status: "degraded".to_string(),
            latency_avg_ms: Some(20.0),
            loss_ratio: 0.5,
            reason: Some("packet_loss".to_string()),
            ..older_metrics.ping_results[0].clone()
        }],
        ..older_metrics.clone()
    };
    let _older_sample =
        insert_raw_telemetry_fixture(&db.pool, client_id, checked_unix + 10, &older_metrics).await;
    let newer_sample =
        insert_raw_telemetry_fixture(&db.pool, client_id, checked_unix + 20, &newer_metrics).await;
    let _invalid_newest_sample =
        insert_raw_telemetry_fixture(&db.pool, client_id, checked_unix + 4_000, &newer_metrics)
            .await;

    let newest = db
        .repo
        .list_raw_ping_results(client_id, None, None, 10, 60)
        .await
        .unwrap();
    assert_eq!(newest.len(), 1);
    assert_eq!(newest[0].sample_count, 1);
    assert_eq!(newest[0].latest_status, "degraded");
    assert_eq!(newest[0].latency_avg_ms, Some(20.0));
    assert_eq!(newest[0].loss_ratio_avg, 0.5);
    assert_eq!(newest[0].latest_reason.as_deref(), Some("packet_loss"));

    sqlx::query("DELETE FROM telemetry_samples WHERE id = $1")
        .bind(newer_sample)
        .execute(&db.pool)
        .await
        .unwrap();
    let retained = db
        .repo
        .list_raw_ping_results(client_id, None, None, 10, 60)
        .await
        .unwrap();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].sample_count, 1);
    assert_eq!(retained[0].latest_status, "degraded");
    assert_eq!(retained[0].latency_avg_ms, Some(20.0));
    assert_eq!(retained[0].loss_ratio_avg, 0.5);
    assert_eq!(retained[0].latest_reason.as_deref(), Some("packet_loss"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*)::bigint FROM telemetry_ping_facts",)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_raw_ping_filters_occurrences_before_dedup_and_metadata_after() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "raw-ping-dedup-client";
    let active_target_id = Uuid::new_v4();
    let secondary_target_id = Uuid::new_v4();
    let unassigned_target_id = Uuid::new_v4();
    insert_client(&db.pool, client_id, None).await;
    sqlx::query(
        r#"
        INSERT INTO ping_targets (
            id, name, host, probe_kind, selector_expression, generation
        ) VALUES
            ($1, 'Raw Ping active', '192.0.2.101', 'icmp', '*', 2),
            ($2, 'Raw Ping secondary', '192.0.2.102', 'icmp', '*', 1),
            ($3, 'Raw Ping unassigned', '192.0.2.103', 'icmp', '*', 1)
        "#,
    )
    .bind(active_target_id)
    .bind(secondary_target_id)
    .bind(unassigned_target_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO ping_target_assignments (target_id, client_id, is_primary)
        VALUES ($1, $3, TRUE), ($2, $3, FALSE)
        "#,
    )
    .bind(active_target_id)
    .bind(secondary_target_id)
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let checked_unix = 1_800_000_000;
    let older_metrics = AgentMetrics {
        observed_unix: checked_unix + 10,
        hostname: client_id.to_string(),
        ping_results: vec![
            PingTargetResult {
                target_id: active_target_id.to_string(),
                generation: 2,
                checked_unix,
                status: "ok".to_string(),
                latency_avg_ms: Some(10.0),
                loss_ratio: 0.0,
                reason: None,
            },
            PingTargetResult {
                target_id: active_target_id.to_string(),
                generation: 1,
                checked_unix: checked_unix + 60,
                status: "down".to_string(),
                latency_avg_ms: None,
                loss_ratio: 1.0,
                reason: Some("stale_generation".to_string()),
            },
            PingTargetResult {
                target_id: secondary_target_id.to_string(),
                generation: 1,
                checked_unix: checked_unix + 60,
                status: "degraded".to_string(),
                latency_avg_ms: Some(30.0),
                loss_ratio: 0.25,
                reason: Some("packet_loss".to_string()),
            },
            PingTargetResult {
                target_id: unassigned_target_id.to_string(),
                generation: 1,
                checked_unix: checked_unix + 120,
                status: "down".to_string(),
                latency_avg_ms: None,
                loss_ratio: 1.0,
                reason: Some("unassigned".to_string()),
            },
        ],
        ..AgentMetrics::default()
    };
    let newer_metrics = AgentMetrics {
        observed_unix: checked_unix + 20,
        ping_results: vec![PingTargetResult {
            status: "degraded".to_string(),
            latency_avg_ms: Some(20.0),
            loss_ratio: 0.5,
            reason: Some("newest_valid".to_string()),
            ..older_metrics.ping_results[0].clone()
        }],
        ..older_metrics.clone()
    };
    let invalid_newest_metrics = AgentMetrics {
        observed_unix: checked_unix + 4_000,
        ping_results: vec![PingTargetResult {
            status: "down".to_string(),
            latency_avg_ms: None,
            loss_ratio: 1.0,
            reason: Some("invalid_occurrence".to_string()),
            ..older_metrics.ping_results[0].clone()
        }],
        ..older_metrics.clone()
    };
    insert_raw_telemetry_fixture(
        &db.pool,
        client_id,
        older_metrics.observed_unix,
        &older_metrics,
    )
    .await;
    insert_raw_telemetry_fixture(
        &db.pool,
        client_id,
        newer_metrics.observed_unix,
        &newer_metrics,
    )
    .await;
    insert_raw_telemetry_fixture(
        &db.pool,
        client_id,
        invalid_newest_metrics.observed_unix,
        &invalid_newest_metrics,
    )
    .await;
    sqlx::query("UPDATE ping_targets SET name = 'Raw Ping current' WHERE id = $1")
        .bind(active_target_id)
        .execute(&db.pool)
        .await
        .unwrap();

    let rows = db
        .repo
        .list_raw_ping_results(
            client_id,
            Some(checked_unix),
            Some(checked_unix + 120),
            10,
            60,
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    let active = rows
        .iter()
        .find(|row| row.target_id == active_target_id)
        .unwrap();
    assert_eq!(active.target_name, "Raw Ping current");
    assert!(active.is_primary);
    assert_eq!(active.generation, 2);
    assert_eq!(active.sample_count, 1);
    assert_eq!(active.latest_status, "degraded");
    assert_eq!(active.latency_avg_ms, Some(20.0));
    assert_eq!(active.loss_ratio_avg, 0.5);
    assert_eq!(active.latest_reason.as_deref(), Some("newest_valid"));
    let secondary = rows
        .iter()
        .find(|row| row.target_id == secondary_target_id)
        .unwrap();
    assert!(!secondary.is_primary);
    assert_eq!(secondary.generation, 1);
    assert_eq!(secondary.latest_status, "degraded");
    assert_eq!(secondary.latest_reason.as_deref(), Some("packet_loss"));
    assert!(rows.iter().all(|row| row.target_id != unassigned_target_id));

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_authoritative_traffic_history_tracks_counter_epochs_and_raw_ranges() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "traffic-history-client";
    let session_id = Uuid::new_v4();
    let process_id = Uuid::new_v4();
    insert_client(&db.pool, client_id, Some(process_id)).await;
    start_test_gateway_session(&db.repo, "gateway-traffic", client_id, session_id).await;
    sqlx::query(
        r#"
        INSERT INTO vps_rule_values (
            client_id, key, value_raw, value_json, source_kind
        ) VALUES ($1, 'traffic.selectors', 'eth0', $2, 'test')
        "#,
    )
    .bind(client_id)
    .bind(serde_json::json!({
        "selectors": [{
            "source": "host",
            "interface": "eth0",
            "direction": "total",
            "canonical": "eth0"
        }]
    }))
    .execute(&db.pool)
    .await
    .unwrap();

    let base = (crate::unix_now() / 60) * 60 - 180;
    for (index, (offset, rx_bytes, tx_bytes)) in
        [(0, 100, 200), (10, 150, 240), (60, 10, 5), (120, 30, 25)]
            .into_iter()
            .enumerate()
    {
        let event = GatewayTelemetryIngest {
            gateway_id: "gateway-traffic".to_string(),
            gateway_session_id: session_id,
            process_incarnation_id: process_id,
            telemetry_seq: (index + 1) as u64,
            remote_ip: None,
            telemetry: TelemetryEnvelope {
                client_id: client_id.to_string(),
                metrics: AgentMetrics {
                    observed_unix: base + offset,
                    hostname: client_id.to_string(),
                    networks: vec![NetworkStat {
                        interface: "eth0".to_string(),
                        rx_bytes,
                        tx_bytes,
                    }],
                    ..AgentMetrics::default()
                },
            },
        };
        assert!(db.repo.record_telemetry(&event).await.unwrap());
    }

    let persisted_counters = sqlx::query_as::<_, (i64, i64, i64, i64)>(
        r#"
        SELECT rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch
        FROM traffic_counter_samples
        WHERE client_id = $1 AND interface = 'eth0'
        ORDER BY observed_at
        "#,
    )
    .bind(client_id)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    // Durable counter minutes use API receive time, so rapid test events normally
    // replace one minute row; a wall-clock minute boundary may legitimately leave
    // two. The final counter and its independently retained epochs are invariant.
    assert_eq!(persisted_counters.last(), Some(&(30, 25, 1, 1)));
    assert!(persisted_counters
        .windows(2)
        .all(|rows| { rows[0].2 <= rows[1].2 && rows[0].3 <= rows[1].3 }));

    sqlx::query("DELETE FROM traffic_counter_samples WHERE client_id = $1")
        .bind(client_id)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM telemetry_samples WHERE client_id = $1")
        .bind(client_id)
        .execute(&db.pool)
        .await
        .unwrap();
    for (offset, rx_bytes, tx_bytes, counter_epoch) in [
        (0, 150_i64, 240_i64, 0_i64),
        (60, 10, 5, 1),
        (120, 30, 25, 1),
    ] {
        sqlx::query(
            r#"
            INSERT INTO traffic_counter_samples (
                client_id, source_kind, interface, observed_at,
                rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source
            ) VALUES (
                $1, 'host', 'eth0', to_timestamp($2::double precision),
                $3, $4, $5, $5, 'test'
            )
            "#,
        )
        .bind(client_id)
        .bind((base + offset) as f64)
        .bind(rx_bytes)
        .bind(tx_bytes)
        .bind(counter_epoch)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    for (offset, rx_bytes, tx_bytes, tunnel_rx_bytes, tunnel_tx_bytes) in [
        (0, 100_u64, 200_u64, 1_000_u64, 2_000_u64),
        (10, 150, 240, 1_050, 2_040),
        (60, 10, 5, 1_060, 2_050),
        (120, 30, 25, 1_080, 2_070),
    ] {
        let metrics = AgentMetrics {
            observed_unix: base + offset,
            hostname: client_id.to_string(),
            networks: vec![NetworkStat {
                interface: "eth0".to_string(),
                rx_bytes,
                tx_bytes,
            }],
            tunnels: vec![RuntimeTunnelStat {
                interface: "wg0".to_string(),
                rx_bytes: tunnel_rx_bytes,
                tx_bytes: tunnel_tx_bytes,
                ..RuntimeTunnelStat::default()
            }],
            ..AgentMetrics::default()
        };
        insert_raw_telemetry_fixture(&db.pool, client_id, base + offset, &metrics).await;
    }

    let minute = db
        .repo
        .list_traffic_history(client_id, base, base + 120, 60, false)
        .await
        .unwrap();
    assert_eq!(minute.len(), 2);
    assert_eq!(minute[0].sample_count, 0);
    assert_eq!(minute[0].reset_count, 1);
    assert_eq!(minute[0].rx_bytes, None);
    assert_eq!(minute[0].tx_bytes, None);
    assert_eq!(minute[1].rx_bytes, Some(20));
    assert_eq!(minute[1].tx_bytes, Some(20));

    sqlx::query(
        r#"
        UPDATE vps_rule_values
        SET value_raw = 'eth0,tunnel:wg0', value_json = $2
        WHERE client_id = $1 AND key = 'traffic.selectors'
        "#,
    )
    .bind(client_id)
    .bind(serde_json::json!({
        "selectors": [
            {
                "source": "host",
                "interface": "eth0",
                "direction": "total",
                "canonical": "eth0"
            },
            {
                "source": "tunnel",
                "interface": "wg0",
                "direction": "total",
                "canonical": "tunnel:wg0"
            }
        ]
    }))
    .execute(&db.pool)
    .await
    .unwrap();

    let raw = db
        .repo
        .list_traffic_history(client_id, base + 10, base + 120, 60, true)
        .await
        .unwrap();
    assert_eq!(
        raw.iter().filter_map(|point| point.rx_bytes).sum::<i64>(),
        150
    );
    assert_eq!(
        raw.iter().filter_map(|point| point.tx_bytes).sum::<i64>(),
        130
    );
    assert!(raw.iter().any(|point| point.reset_count == 1));

    let operator = postgres_network_operator(&db.repo).await;
    let share = crate::model_monitoring::MonitoringShareRecord {
        id: Uuid::new_v4(),
        name: "Traffic evidence".to_string(),
        token_secret: payload_hash(b"traffic-share"),
        selector_expression: "*".to_string(),
        targets: vec![crate::model_monitoring::MonitoringShareTargetRecord {
            client_id: client_id.to_string(),
            public_client_key: "3".repeat(64),
        }],
        visibility: crate::model_monitoring::MonitoringShareVisibilityView {
            identity_context: false,
            billing: true,
            system_information: true,
            resources: true,
            network: true,
            traffic: true,
            ping: true,
            detail_history: true,
        },
        expires_at: crate::unix_now().saturating_add(3_600).to_string(),
        revoked_at: None,
        revoked_by: None,
        created_by: Some(operator.operator.id),
        created_at: crate::unix_now().to_string(),
        updated_at: crate::unix_now().to_string(),
    };
    db.repo
        .create_monitoring_share(share.clone(), &operator)
        .await
        .unwrap();
    let persisted_share = db
        .repo
        .monitoring_share_record(share.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted_share.targets, share.targets);
    assert_eq!(persisted_share.visibility, share.visibility);
    for source_ip in ["198.51.100.10", "198.51.100.11"] {
        db.repo
            .record_monitoring_share_visitor(
                &share,
                Some(Uuid::new_v4()),
                source_ip,
                Some("browser"),
            )
            .await
            .unwrap();
    }
    let listed = db
        .repo
        .list_monitoring_shares(Some("active"), 10, 0)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].target_count, 1);
    assert_eq!(listed[0].visitor_count, 2);
    assert!(listed[0].visibility.billing);
    assert!(listed[0].visibility.system_information);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_raw_traffic_adds_exact_imports_without_replacing_live_or_rollups() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "raw-traffic-import-history";
    insert_client(&db.pool, client_id, None).await;
    sqlx::query(
        r#"
        INSERT INTO vps_rule_values (client_id, key, value_raw, value_json)
        VALUES (
            $1,
            'traffic.selectors',
            'eth0,eth1',
            '{"selectors":[
                {"source":"host","interface":"eth0","direction":"total","canonical":"eth0"},
                {"source":"host","interface":"eth1","direction":"total","canonical":"eth1"}
            ]}'::jsonb
        )
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let hour_start = ((crate::unix_now() / 3_600) * 3_600).saturating_sub(7_200);
    let base = hour_start + 600;
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch,
            sample_source, inbound_promoted
        ) VALUES
            ($1, 'host', 'eth0', to_timestamp(($2 - 60)::double precision),
                0, 0, 0, 0, 'vnstat_import:test', FALSE),
            ($1, 'host', 'eth0', to_timestamp($2::double precision),
                10, 20, 0, 0, 'vnstat_import:test', FALSE),
            ($1, 'host', 'eth0', to_timestamp(($2 + 60)::double precision),
                25, 50, 0, 0, 'vnstat_import:test', FALSE),
            ($1, 'host', 'eth0', to_timestamp(($2 + 180)::double precision),
                500, 700, 1, 1, 'agent_networks', FALSE),
            ($1, 'host', 'eth0', to_timestamp(($2 + 240)::double precision),
                550, 760, 1, 1, 'agent_networks', FALSE),
            ($1, 'host', 'eth1', to_timestamp($2::double precision),
                0, 0, 0, 0, 'agent_networks', FALSE),
            ($1, 'host', 'eth1', to_timestamp(($2 + 60)::double precision),
                1000, 2000, 0, 0, 'vnstat_import:test', TRUE)
        "#,
    )
    .bind(client_id)
    .bind(base as i64)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        ) VALUES (
            $1, 'host', 'eth1', 'vnstat_import', 3600,
            to_timestamp($2::double precision), 1000, 2000,
            1, 1, 1, 0, 0, 0,
            to_timestamp(($3 + 60)::double precision),
            to_timestamp(($3 + 60)::double precision)
        )
        "#,
    )
    .bind(client_id)
    .bind(hour_start as i64)
    .bind(base as i64)
    .execute(&db.pool)
    .await
    .unwrap();

    for (offset, rx_bytes, tx_bytes) in [(130, 100, 200), (145, 130, 240)] {
        let metrics = AgentMetrics {
            observed_unix: base + offset,
            hostname: client_id.to_string(),
            networks: vec![NetworkStat {
                interface: "eth0".to_string(),
                rx_bytes,
                tx_bytes,
            }],
            ..AgentMetrics::default()
        };
        insert_raw_telemetry_fixture(&db.pool, client_id, base + offset, &metrics).await;
    }

    let raw = db
        .repo
        .list_traffic_history(client_id, base, base + 300, 300, true)
        .await
        .unwrap();
    assert_eq!(raw.len(), 1);
    assert_eq!(raw[0].bucket_secs, 300);
    assert_eq!(raw[0].sample_count, 3);
    assert_eq!(raw[0].reset_count, 0);
    assert_eq!(raw[0].rx_bytes, Some(55));
    assert_eq!(raw[0].tx_bytes, Some(90));
    assert_eq!(raw[0].total_bytes, Some(145));

    let retained = db
        .repo
        .list_traffic_history(client_id, base, base + 300, 300, false)
        .await
        .unwrap();
    assert_eq!(
        retained.iter().map(|point| point.sample_count).sum::<i32>(),
        4
    );
    assert_eq!(
        retained
            .iter()
            .filter_map(|point| point.rx_bytes)
            .sum::<i64>(),
        1_075
    );
    assert_eq!(
        retained
            .iter()
            .filter_map(|point| point.tx_bytes)
            .sum::<i64>(),
        2_110
    );
    assert!(retained.iter().any(|point| point.bucket_secs == 300));
    assert!(retained.iter().any(|point| point.bucket_secs == 3_600));

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_retained_system_history_returns_whole_overlapping_coarse_bucket() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let bucket_start: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO system_metric_rollups (
            metric, bucket_start, bucket_secs, sample_count, value_sum,
            avg_value, max_value, latest_value, latest_observed_at
        ) VALUES (
            'test.retained.system',
            date_trunc('day', now()) - interval '40 days',
            86400, 1440, 4320, 3, 5, 4,
            date_trunc('day', now()) - interval '39 days 1 minute'
        )
        RETURNING extract(epoch FROM bucket_start)::bigint
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO system_metric_rollups (
            metric, bucket_start, bucket_secs, sample_count, value_sum,
            avg_value, max_value, latest_value, latest_observed_at
        ) VALUES (
            'test.retained.system', to_timestamp($1::double precision) + interval '23 hours 59 minutes',
            60, 1, 99, 99, 99, 99,
            to_timestamp($1::double precision) + interval '23 hours 59 minutes'
        )
        "#,
    )
    .bind(bucket_start as f64)
    .execute(&db.pool)
    .await
    .unwrap();

    let rows = db
        .repo
        .list_system_metric_rollups_at_step(
            (bucket_start + 86_367) as u64,
            (bucket_start + 86_400) as u64,
            10,
            86_400,
        )
        .await
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].bucket_secs, 86_400);
    assert_eq!(rows[0].sample_count, 1_440);
    assert_eq!(rows[0].avg_value, 3.0);
    assert_eq!(rows[0].max_value, 5.0);
    assert_eq!(rows[0].latest_value, 4.0);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_system_rollup_fast_path_preserves_mixed_tier_selection() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let performance_start = ((crate::unix_now() / 60) * 60).saturating_sub(150_000);
    sqlx::query(
        r#"
        INSERT INTO system_metric_rollups (
            metric, bucket_start, bucket_secs, sample_count, value_sum,
            avg_value, max_value, latest_value, latest_observed_at
        )
        SELECT
            metric,
            to_timestamp($1::double precision) + point * interval '1 minute',
            60,
            1,
            (point % 100 + 1)::double precision,
            (point % 100 + 1)::double precision,
            (point % 100 + 1)::double precision,
            (point % 100 + 1)::double precision,
            to_timestamp($1::double precision) + point * interval '1 minute'
        FROM unnest(ARRAY[
            'test.rollup.fast.cpu',
            'test.rollup.fast.memory',
            'test.rollup.fast.disk',
            'test.rollup.fast.network'
        ]::text[]) AS metrics(metric)
        CROSS JOIN generate_series(0, 2399) AS points(point)
        "#,
    )
    .bind(performance_start as f64)
    .execute(&db.pool)
    .await
    .unwrap();

    sqlx::query("ANALYZE system_metric_rollups")
        .execute(&db.pool)
        .await
        .unwrap();
    let explain_sql = format!(
        "EXPLAIN (ANALYZE, FORMAT JSON) {}",
        crate::repository_system_dashboard::SYSTEM_METRIC_ROLLUP_AT_STEP_SQL
    );
    let plan: Value = sqlx::query_scalar(&explain_sql)
        .bind(performance_start as f64)
        .bind((performance_start + 2_400 * 60 - 1) as f64)
        .bind(60_i32)
        .bind(20_000_i64)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let plan = plan.to_string();
    assert!(plan.contains("\"Node Type\":\"WindowAgg\""), "{plan}");
    assert!(plan.contains("\"Join Type\":\"Left\""), "{plan}");
    assert!(plan.contains("\"Actual Loops\":0"), "{plan}");
    assert!(!plan.contains("\"Join Type\":\"Anti\""), "{plan}");

    let mixed_start = performance_start - 600;
    sqlx::query(
        r#"
        INSERT INTO system_metric_rollups (
            metric, bucket_start, bucket_secs, sample_count, value_sum,
            avg_value, max_value, latest_value, latest_observed_at
        ) VALUES
            ('test.rollup.mixed', to_timestamp($1::double precision),
                300, 5, 50, 10, 10, 10, to_timestamp(($1 + 299)::double precision)),
            ('test.rollup.mixed', to_timestamp(($1 + 60)::double precision),
                60, 1, 99, 99, 99, 99, to_timestamp(($1 + 60)::double precision)),
            ('test.rollup.mixed', to_timestamp(($1 + 300)::double precision),
                60, 1, 7, 7, 7, 7, to_timestamp(($1 + 300)::double precision))
        "#,
    )
    .bind(mixed_start as f64)
    .execute(&db.pool)
    .await
    .unwrap();

    let selection_delta: i64 = sqlx::query_scalar(
        r#"
        WITH legacy AS (
            SELECT row.metric, row.bucket_start, row.bucket_secs
            FROM system_metric_rollups row
            WHERE row.metric = 'test.rollup.mixed'
              AND row.bucket_start <= to_timestamp($2::double precision)
              AND row.bucket_start + make_interval(secs => row.bucket_secs)
                    > to_timestamp($1::double precision)
              AND NOT EXISTS (
                    SELECT 1
                    FROM system_metric_rollups coarser
                    WHERE coarser.metric = row.metric
                      AND coarser.bucket_secs > row.bucket_secs
                      AND coarser.bucket_start <= to_timestamp($2::double precision)
                      AND coarser.bucket_start
                            + make_interval(secs => coarser.bucket_secs)
                            > to_timestamp($1::double precision)
                      AND coarser.bucket_start
                            < row.bucket_start + make_interval(secs => row.bucket_secs)
                      AND coarser.bucket_start
                            + make_interval(secs => coarser.bucket_secs)
                            > row.bucket_start
                )
        ),
        candidates AS (
            SELECT
                row.*,
                max(row.bucket_secs) OVER (PARTITION BY row.metric) AS max_bucket_secs
            FROM system_metric_rollups row
            WHERE row.metric = 'test.rollup.mixed'
              AND row.bucket_start <= to_timestamp($2::double precision)
              AND row.bucket_start + make_interval(secs => row.bucket_secs)
                    > to_timestamp($1::double precision)
        ),
        optimized AS (
            SELECT row.metric, row.bucket_start, row.bucket_secs
            FROM candidates row
            LEFT JOIN LATERAL (
                SELECT TRUE AS overlaps
                FROM system_metric_rollups coarser
                WHERE row.bucket_secs < row.max_bucket_secs
                  AND coarser.metric = row.metric
                  AND coarser.bucket_secs > row.bucket_secs
                  AND coarser.bucket_start <= to_timestamp($2::double precision)
                  AND coarser.bucket_start
                        + make_interval(secs => coarser.bucket_secs)
                        > to_timestamp($1::double precision)
                  AND coarser.bucket_start
                        < row.bucket_start + make_interval(secs => row.bucket_secs)
                  AND coarser.bucket_start
                        + make_interval(secs => coarser.bucket_secs)
                        > row.bucket_start
                LIMIT 1
            ) coarser_overlap ON TRUE
            WHERE coarser_overlap.overlaps IS NULL
        ),
        delta AS (
            (SELECT * FROM legacy EXCEPT ALL SELECT * FROM optimized)
            UNION ALL
            (SELECT * FROM optimized EXCEPT ALL SELECT * FROM legacy)
        )
        SELECT count(*)::bigint FROM delta
        "#,
    )
    .bind(mixed_start as f64)
    .bind((mixed_start + 359) as f64)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(selection_delta, 0);

    let mixed_rows = db
        .repo
        .list_system_metric_rollups_at_step(mixed_start, mixed_start + 359, 10, 60)
        .await
        .unwrap();
    assert_eq!(mixed_rows.len(), 2);
    assert_eq!(mixed_rows[0].bucket_secs, 300);
    assert_eq!(mixed_rows[0].sample_count, 5);
    assert_eq!(mixed_rows[1].bucket_secs, 60);
    assert_eq!(mixed_rows[1].sample_count, 1);
    assert_eq!(mixed_rows[1].latest_value, 7.0);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_telemetry_queries_preserve_scope_baseline_and_multi_day_endpoints() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    for (client_id, key_byte) in [
        ("selected-telemetry", 51_u8),
        ("unrelated-telemetry", 52_u8),
        ("multi-day-telemetry", 53_u8),
        ("adaptive-telemetry", 54_u8),
        ("reset-telemetry", 55_u8),
        ("raw-reset-telemetry", 56_u8),
        ("intra-reset-telemetry", 57_u8),
        ("rate-selection-telemetry", 58_u8),
    ] {
        sqlx::query(
            r#"
            INSERT INTO clients (id, display_name, public_key, status)
            VALUES ($1, $1, $2, 'online')
            "#,
        )
        .bind(client_id)
        .bind(vec![key_byte; 32])
        .execute(&db.pool)
        .await
        .unwrap();
    }
    let current = crate::unix_now() / 60 * 60;
    let previous = current.saturating_sub(60);
    for (client_id, load) in [
        ("unrelated-telemetry", 9.0_f64),
        ("selected-telemetry", 0.5_f64),
    ] {
        sqlx::query(
            r#"
            INSERT INTO telemetry_rollups (
                client_id, bucket_start, bucket_secs, sample_count,
                cpu_load_1_avg, cpu_load_1_sum, cpu_load_1_max,
                memory_total_bytes_max, memory_available_bytes_avg,
                memory_available_bytes_sum, memory_available_bytes_min,
                memory_used_ratio_avg, memory_used_ratio_sum, memory_used_ratio_max,
                disk_total_bytes_max, disk_available_bytes_avg,
                disk_available_bytes_sum, disk_available_bytes_min,
                disk_used_ratio_avg, disk_used_ratio_sum, disk_used_ratio_max,
                network_rx_bytes_max, network_tx_bytes_max, latest_observed_at
            )
            VALUES (
                $1, to_timestamp($2::double precision), 60, 1,
                $3, $3, $3, 1000, 500, 500, 500, 0.5, 0.5, 0.5,
                2000, 1500, 1500, 1500, 0.25, 0.25, 0.25, 0, 0,
                to_timestamp($2::double precision)
            )
            "#,
        )
        .bind(client_id)
        .bind(current as f64)
        .bind(load)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    for (client_id, observed, rx, tx) in [
        ("unrelated-telemetry", current, 99_000_i64, 99_000_i64),
        ("selected-telemetry", previous, 1_000_i64, 2_000_i64),
        ("selected-telemetry", current, 4_000_i64, 8_000_i64),
    ] {
        sqlx::query(
            r#"
            INSERT INTO telemetry_network_rates (
                client_id, interface, bucket_start, bucket_secs,
                sample_count, rx_bytes_sum, tx_bytes_sum,
                rx_bytes_avg, tx_bytes_avg,
                rx_bytes_last, tx_bytes_last, rx_counter_epoch, tx_counter_epoch,
                latest_observed_at
            )
            VALUES ($1, 'eth0', to_timestamp($2::double precision), 60, 1,
                $3, $4, $3, $4, $3, $4, 0, 0,
                to_timestamp($2::double precision))
            "#,
        )
        .bind(client_id)
        .bind(observed as f64)
        .bind(rx)
        .bind(tx)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    for (observed, rx, tx) in [
        (current.saturating_sub(360), 10_000_i64, 20_000_i64),
        (current + 60, 25_000_i64, 50_000_i64),
    ] {
        sqlx::query(
            r#"
            INSERT INTO telemetry_network_rates (
                client_id, interface, bucket_start, bucket_secs,
                sample_count, rx_bytes_sum, tx_bytes_sum,
                rx_bytes_avg, tx_bytes_avg,
                rx_bytes_last, tx_bytes_last, rx_counter_epoch, tx_counter_epoch,
                latest_observed_at
            )
            VALUES ('selected-telemetry', 'eth0', to_timestamp($1::double precision), 300, 1,
                $2, $3, $2, $3, $2, $3, 0, 0,
                to_timestamp(($1::bigint + 240)::double precision))
            "#,
        )
        .bind(observed as f64)
        .bind(rx)
        .bind(tx)
        .execute(&db.pool)
        .await
        .unwrap();
    }

    let scope = vec!["selected-telemetry".to_string()];
    let rollups = db
        .repo
        .list_dashboard_telemetry_rollups(
            1,
            Some(current),
            Some(current + 59),
            Some(60),
            60,
            &scope,
        )
        .await
        .unwrap();
    assert_eq!(rollups.len(), 1);
    assert_eq!(rollups[0].client_id, "selected-telemetry");
    let listed_rollups = db
        .repo
        .list_telemetry_rollups(10, Some("selected-telemetry"), Some(60), false)
        .await
        .unwrap();
    assert_eq!(listed_rollups.len(), 1);
    assert_eq!(
        crate::util::parse_timestamp_unix(&listed_rollups[0].latest_observed_at),
        Some(current)
    );

    let coarse_start = current / 300 * 300;
    let mut coarse_test_minutes = [0_u64, 60, 120, 180, 240]
        .into_iter()
        .filter(|offset| coarse_start + offset != current);
    let coarse_first = coarse_start + coarse_test_minutes.next().unwrap();
    let coarse_second = coarse_start + coarse_test_minutes.next().unwrap();
    sqlx::query(
        r#"
        INSERT INTO telemetry_rollups (
            client_id, bucket_start, bucket_secs, sample_count,
            cpu_load_1_avg, cpu_load_1_sum, cpu_load_1_max,
            memory_total_bytes_max, memory_available_bytes_avg,
            memory_available_bytes_sum, memory_available_bytes_min,
            memory_used_ratio_avg, memory_used_ratio_sum, memory_used_ratio_max,
            disk_total_bytes_max, disk_available_bytes_avg,
            disk_available_bytes_sum, disk_available_bytes_min,
            disk_used_ratio_avg, disk_used_ratio_sum, disk_used_ratio_max,
            network_rx_bytes_max, network_tx_bytes_max, latest_observed_at
        )
        VALUES
            (
                'selected-telemetry', to_timestamp($1::double precision), 60, 1,
                1.0, 1.0, 1.2, 1000, 900, 900, 900, 0.1, 0.1, 0.1,
                2000, 1900, 1900, 1900, 0.05, 0.05, 0.05, 0, 0,
                to_timestamp($1::double precision)
            ),
            (
                'selected-telemetry', to_timestamp($2::double precision), 60, 3,
                3.0, 9.0, 3.4, 1000, 500, 1500, 500, 0.5, 1.5, 0.5,
                2000, 1100, 3300, 1100, 0.45, 1.35, 0.45, 0, 0,
                to_timestamp($2::double precision)
            )
        "#,
    )
    .bind(coarse_first as f64)
    .bind(coarse_second as f64)
    .execute(&db.pool)
    .await
    .unwrap();
    let aggregated = db
        .repo
        .list_dashboard_telemetry_rollups(
            2,
            Some(coarse_start),
            Some(coarse_start + 299),
            Some(60),
            300,
            &scope,
        )
        .await
        .unwrap();
    assert_eq!(aggregated.len(), 1);
    assert_eq!(
        crate::util::parse_timestamp_unix(&aggregated[0].bucket_start),
        Some(coarse_start)
    );
    assert_eq!(aggregated[0].bucket_secs, 300);
    assert_eq!(aggregated[0].sample_count, 5);
    assert!((aggregated[0].cpu_load_1_avg - 2.1).abs() < 0.000_001);
    assert_eq!(aggregated[0].cpu_load_1_max, 3.4);
    assert_eq!(aggregated[0].memory_available_bytes_avg, 580);
    assert_eq!(aggregated[0].memory_available_bytes_min, 500);
    assert!((aggregated[0].memory_used_ratio_avg - 0.42).abs() < 0.000_001);
    assert!((aggregated[0].memory_used_ratio_max - 0.5).abs() < 0.000_001);
    assert_eq!(aggregated[0].disk_available_bytes_avg, 1340);
    assert_eq!(aggregated[0].disk_available_bytes_min, 1100);
    assert!((aggregated[0].disk_used_ratio_avg - 0.33).abs() < 0.000_001);
    assert!((aggregated[0].disk_used_ratio_max - 0.45).abs() < 0.000_001);

    sqlx::query(
        r#"
        INSERT INTO telemetry_rollups (
            client_id, bucket_start, bucket_secs, sample_count,
            cpu_load_1_avg, cpu_load_1_sum, cpu_load_1_max,
            memory_total_bytes_max, memory_available_bytes_avg,
            memory_available_bytes_sum, memory_available_bytes_min,
            memory_used_ratio_avg, memory_used_ratio_sum, memory_used_ratio_max,
            disk_total_bytes_max, disk_available_bytes_avg,
            disk_available_bytes_sum, disk_available_bytes_min,
            disk_used_ratio_avg, disk_used_ratio_sum, disk_used_ratio_max,
            network_rx_bytes_max, network_tx_bytes_max, latest_observed_at
        )
        VALUES
            (
                'multi-day-telemetry', to_timestamp(0), 60, 1,
                1.0, 1.0, 1.0, 1000, 900, 900, 900, 0.1, 0.1, 0.1,
                2000, 1900, 1900, 1900, 0.05, 0.05, 0.05, 0, 0,
                to_timestamp(0)
            ),
            (
                'multi-day-telemetry', to_timestamp(172800), 60, 1,
                2.0, 2.0, 2.0, 1000, 800, 800, 800, 0.2, 0.2, 0.2,
                2000, 1800, 1800, 1800, 0.1, 0.1, 0.1, 0, 0,
                to_timestamp(172800)
            ),
            (
                'multi-day-telemetry', to_timestamp(345600), 60, 1,
                3.0, 3.0, 3.0, 1000, 700, 700, 700, 0.3, 0.3, 0.3,
                2000, 1700, 1700, 1700, 0.15, 0.15, 0.15, 0, 0,
                to_timestamp(345600)
            )
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let multi_day_rollups = db
        .repo
        .list_dashboard_telemetry_rollups(
            2,
            Some(0),
            Some(345_600),
            Some(60),
            345_600,
            &["multi-day-telemetry".to_string()],
        )
        .await
        .unwrap();
    let multi_day_bucket_starts = multi_day_rollups
        .iter()
        .map(|row| crate::util::parse_timestamp_unix(&row.bucket_start).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(multi_day_bucket_starts, vec![0, 345_600]);

    let rates = db
        .repo
        .list_dashboard_telemetry_network_rates(
            10,
            Some(current),
            Some(current + 59),
            Some(60),
            60,
            &scope,
        )
        .await
        .unwrap();
    assert_eq!(rates.len(), 1);
    assert_eq!(rates[0].client_id, "selected-telemetry");
    assert_eq!(rates[0].rx_bytes_delta, 3_000);
    assert_eq!(rates[0].tx_bytes_delta, 6_000);
    let latest = db
        .repo
        .list_latest_telemetry_network_rates(10, Some("selected-telemetry"), Some("eth0"), Some(60))
        .await
        .unwrap();
    assert_eq!(latest.len(), 1);
    assert_eq!(latest[0].rx_bytes_delta, 3_000);
    let latest_mixed = db
        .repo
        .list_latest_telemetry_network_rates(10, Some("selected-telemetry"), Some("eth0"), None)
        .await
        .unwrap();
    assert_eq!(latest_mixed.len(), 1);
    assert_eq!(latest_mixed[0].bucket_secs, 300);
    assert_eq!(latest_mixed[0].rx_bytes_delta, 21_000);
    let latest_scoped = db
        .repo
        .list_latest_telemetry_network_rates_for_clients(&scope)
        .await
        .unwrap();
    assert!(latest_scoped
        .iter()
        .all(|rate| rate.client_id == "selected-telemetry"));
    assert!(!latest_scoped.is_empty());

    for (interface, observed, rx, tx) in [
        ("eth0", previous, 100_i64, 200_i64),
        ("eth0", current, 160_i64, 320_i64),
        ("lo", previous, 1_000_i64, 2_000_i64),
        ("lo", current, 1_600_i64, 3_200_i64),
    ] {
        sqlx::query(
            r#"
            INSERT INTO telemetry_network_rates (
                client_id, interface, bucket_start, bucket_secs,
                sample_count, rx_bytes_sum, tx_bytes_sum,
                rx_bytes_avg, tx_bytes_avg,
                rx_bytes_last, tx_bytes_last, rx_counter_epoch, tx_counter_epoch,
                latest_observed_at
            )
            VALUES (
                'rate-selection-telemetry', $1, to_timestamp($2::double precision),
                60, 1, $3, $4, $3, $4, $3, $4, 0, 0,
                to_timestamp($2::double precision)
            )
            "#,
        )
        .bind(interface)
        .bind(observed as f64)
        .bind(rx)
        .bind(tx)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    for (observed, eth0_rx, eth0_tx, lo_rx, lo_tx) in [
        (previous, 100_u64, 200_u64, 1_000_u64, 2_000_u64),
        (current, 160_u64, 320_u64, 1_600_u64, 3_200_u64),
    ] {
        let metrics = AgentMetrics {
            observed_unix: observed,
            hostname: "rate-selection-telemetry".to_string(),
            networks: vec![
                NetworkStat {
                    interface: "eth0".to_string(),
                    rx_bytes: eth0_rx,
                    tx_bytes: eth0_tx,
                },
                NetworkStat {
                    interface: "lo".to_string(),
                    rx_bytes: lo_rx,
                    tx_bytes: lo_tx,
                },
            ],
            ..AgentMetrics::default()
        };
        insert_raw_telemetry_fixture(&db.pool, "rate-selection-telemetry", observed, &metrics)
            .await;
    }
    let mut rate_selection = NetworkRateInterfaceSelection::default();
    rate_selection.select_exact(
        "rate-selection-telemetry".to_string(),
        std::collections::BTreeSet::from(["eth0".to_string()]),
    );
    for selected in [
        db.repo
            .list_dashboard_telemetry_network_rates_selected(
                10,
                Some(current),
                Some(current),
                Some(60),
                60,
                &rate_selection,
            )
            .await
            .unwrap(),
        db.repo
            .list_dashboard_raw_telemetry_network_rates_selected(
                10,
                current,
                current,
                60,
                &rate_selection,
            )
            .await
            .unwrap(),
        db.repo
            .list_latest_telemetry_network_rates_for_selection(&rate_selection)
            .await
            .unwrap(),
    ] {
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].interface, "eth0");
        assert_eq!(selected[0].rx_bytes_delta, 60);
        assert!(selected[0].rx_bps_avg > 0.0);
        assert_eq!(selected[0].tx_bytes_delta, 120);
        assert!(selected[0].tx_bps_avg > 0.0);
    }
    let raw_all = db
        .repo
        .list_dashboard_raw_telemetry_network_rates(
            10,
            current,
            current,
            60,
            &["rate-selection-telemetry".to_string()],
        )
        .await
        .unwrap();
    assert_eq!(
        raw_all
            .iter()
            .map(|row| row.interface.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from(["eth0", "lo"])
    );

    let reset_previous = current.saturating_sub(120);
    let reset_at = current.saturating_sub(60);
    sqlx::query(
        r#"
        INSERT INTO telemetry_network_rates (
            client_id, interface, bucket_start, bucket_secs,
            sample_count, rx_bytes_sum, tx_bytes_sum,
            rx_bytes_avg, tx_bytes_avg,
            rx_bytes_last, tx_bytes_last, rx_counter_epoch, tx_counter_epoch,
            latest_observed_at
        ) VALUES
            ('reset-telemetry', 'eth0', to_timestamp($1::double precision), 60,
                1, 1000, 2000, 1000, 2000, 1000, 2000, 0, 0,
                to_timestamp($1::double precision)),
            ('reset-telemetry', 'eth0', to_timestamp($2::double precision), 60,
                1, 100, 2100, 100, 2100, 100, 2100, 1, 0,
                to_timestamp($2::double precision))
        "#,
    )
    .bind(reset_previous as f64)
    .bind(reset_at as f64)
    .execute(&db.pool)
    .await
    .unwrap();
    assert!(db
        .repo
        .list_dashboard_telemetry_network_rates(
            10,
            Some(reset_at),
            Some(reset_at),
            Some(60),
            60,
            &["reset-telemetry".to_string()],
        )
        .await
        .unwrap()
        .is_empty());
    let reset_list = db
        .repo
        .list_telemetry_network_rates(10, Some("reset-telemetry"), Some("eth0"), Some(60), false)
        .await
        .unwrap();
    assert!(reset_list.is_empty());
    assert!(db
        .repo
        .list_latest_telemetry_network_rates(10, Some("reset-telemetry"), Some("eth0"), Some(60),)
        .await
        .unwrap()
        .is_empty());

    sqlx::query(
        r#"
        INSERT INTO telemetry_network_rates (
            client_id, interface, bucket_start, bucket_secs,
            sample_count, rx_bytes_sum, tx_bytes_sum,
            rx_bytes_avg, tx_bytes_avg,
            rx_bytes_last, tx_bytes_last, rx_counter_epoch, tx_counter_epoch,
            latest_observed_at
        ) VALUES (
            'reset-telemetry', 'eth0', to_timestamp($1::double precision), 60,
            1, 160, 2200, 160, 2200, 160, 2200, 1, 0,
            to_timestamp($1::double precision)
        )
        "#,
    )
    .bind(current as f64)
    .execute(&db.pool)
    .await
    .unwrap();
    let recovered = db
        .repo
        .list_latest_telemetry_network_rates(10, Some("reset-telemetry"), Some("eth0"), Some(60))
        .await
        .unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].rx_bytes_delta, 60);
    assert_eq!(recovered[0].tx_bytes_delta, 100);

    for (observed, rx, tx) in [
        (reset_previous, 1_000_u64, 2_000_u64),
        (reset_at, 100, 2_100),
        (current, 160, 2_200),
    ] {
        let metrics = AgentMetrics {
            observed_unix: observed,
            hostname: "raw-reset-telemetry".to_string(),
            networks: vec![NetworkStat {
                interface: "eth0".to_string(),
                rx_bytes: rx,
                tx_bytes: tx,
            }],
            ..AgentMetrics::default()
        };
        insert_raw_telemetry_fixture(&db.pool, "raw-reset-telemetry", observed, &metrics).await;
    }
    assert!(db
        .repo
        .list_dashboard_raw_telemetry_network_rates(
            10,
            reset_at,
            reset_at,
            60,
            &["raw-reset-telemetry".to_string()],
        )
        .await
        .unwrap()
        .is_empty());
    let raw_recovered = db
        .repo
        .list_dashboard_raw_telemetry_network_rates(
            10,
            current,
            current,
            60,
            &["raw-reset-telemetry".to_string()],
        )
        .await
        .unwrap();
    assert_eq!(raw_recovered.len(), 1);
    assert_eq!(raw_recovered[0].rx_bytes_delta, 60);
    assert_eq!(raw_recovered[0].tx_bytes_delta, 100);

    let intra_minute = current.saturating_sub(120);
    sqlx::query(
        r#"
        INSERT INTO telemetry_network_rates (
            client_id, interface, bucket_start, bucket_secs, sample_count,
            rx_bytes_sum, tx_bytes_sum, rx_bytes_avg, tx_bytes_avg,
            rx_bytes_last, tx_bytes_last,
            rx_counter_epoch, tx_counter_epoch, latest_observed_at
        ) VALUES
            ('intra-reset-telemetry', 'eth0', to_timestamp($1::double precision), 60, 1,
                1000, 2000, 1000, 2000, 1000, 2000, 0, 0,
                to_timestamp($1::double precision)),
            ('intra-reset-telemetry', 'eth0', to_timestamp($2::double precision), 60, 2,
                1300, 4300, 650, 2150, 1200, 2200, 1, 0,
                to_timestamp(($2::bigint + 20)::double precision)),
            ('intra-reset-telemetry', 'eth0', to_timestamp($3::double precision), 60, 1,
                1300, 2300, 1300, 2300, 1300, 2300, 1, 0,
                to_timestamp($3::double precision))
        "#,
    )
    .bind(intra_minute.saturating_sub(60) as f64)
    .bind(intra_minute as f64)
    .bind(intra_minute.saturating_add(60) as f64)
    .execute(&db.pool)
    .await
    .unwrap();
    for (observed, rx, tx) in [
        (intra_minute.saturating_sub(15), 1_000_u64, 2_000_u64),
        (intra_minute.saturating_add(5), 100, 2_100),
        (intra_minute.saturating_add(20), 1_200, 2_200),
        (intra_minute.saturating_add(65), 1_300, 2_300),
    ] {
        let metrics = AgentMetrics {
            observed_unix: observed,
            hostname: "intra-reset-telemetry".to_string(),
            networks: vec![NetworkStat {
                interface: "eth0".to_string(),
                rx_bytes: rx,
                tx_bytes: tx,
            }],
            ..AgentMetrics::default()
        };
        insert_raw_telemetry_fixture(&db.pool, "intra-reset-telemetry", observed, &metrics).await;
    }
    assert!(db
        .repo
        .list_dashboard_telemetry_network_rates(
            10,
            Some(intra_minute),
            Some(intra_minute + 59),
            Some(60),
            60,
            &["intra-reset-telemetry".to_string()],
        )
        .await
        .unwrap()
        .is_empty());
    assert!(db
        .repo
        .list_dashboard_raw_telemetry_network_rates(
            10,
            intra_minute,
            intra_minute + 59,
            60,
            &["intra-reset-telemetry".to_string()],
        )
        .await
        .unwrap()
        .is_empty());
    for recovered in [
        db.repo
            .list_dashboard_telemetry_network_rates(
                10,
                Some(intra_minute + 60),
                Some(intra_minute + 119),
                Some(60),
                60,
                &["intra-reset-telemetry".to_string()],
            )
            .await
            .unwrap(),
        db.repo
            .list_dashboard_raw_telemetry_network_rates(
                10,
                intra_minute + 60,
                intra_minute + 119,
                60,
                &["intra-reset-telemetry".to_string()],
            )
            .await
            .unwrap(),
    ] {
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].rx_bytes_delta, 100);
        assert_eq!(recovered[0].tx_bytes_delta, 100);
    }

    let adaptive_start = current.saturating_sub(7_200);
    sqlx::query(
        r#"
        INSERT INTO telemetry_rollups (
            client_id, bucket_start, bucket_secs, sample_count,
            cpu_usage_sample_count, cpu_usage_sum, cpu_usage_avg,
            cpu_usage_max, cpu_cores_max,
            cpu_load_1_avg, cpu_load_1_sum, cpu_load_1_max,
            cpu_load_5_avg, cpu_load_5_sum, cpu_load_5_max,
            cpu_load_15_avg, cpu_load_15_sum, cpu_load_15_max,
            memory_total_bytes_max, memory_available_bytes_avg,
            memory_available_bytes_sum, memory_available_bytes_min,
            memory_used_ratio_avg, memory_used_ratio_sum, memory_used_ratio_max,
            swap_sample_count, swap_total_bytes_max,
            swap_available_bytes_avg, swap_available_bytes_sum,
            swap_available_bytes_min, swap_used_ratio_avg,
            swap_used_ratio_sum, swap_used_ratio_max,
            disk_total_bytes_max, disk_available_bytes_avg,
            disk_available_bytes_sum, disk_available_bytes_min,
            disk_used_ratio_avg, disk_used_ratio_sum, disk_used_ratio_max,
            network_rx_bytes_max, network_tx_bytes_max, latest_observed_at
        ) VALUES (
            'adaptive-telemetry', to_timestamp($1::double precision), 300, 5,
            5, 1.25, 0.25, 0.25, 2,
            0.5, 2.5, 0.5, 0.4, 2.0, 0.4, 0.3, 1.5, 0.3,
            1000, 500, 2500, 500, 0.5, 2.5, 0.5,
            1, 1000, 400, 400, 400, 0.6, 0.6, 0.6,
            2000, 1000, 5000, 1000, 0.5, 2.5, 0.5, 0, 0,
            to_timestamp(($1::bigint + 299)::double precision)
        )
        "#,
    )
    .bind(adaptive_start as f64)
    .execute(&db.pool)
    .await
    .unwrap();
    let adaptive_resources = db
        .repo
        .list_dashboard_telemetry_rollups(
            10,
            Some(adaptive_start + 60),
            Some(adaptive_start + 240),
            None,
            60,
            &["adaptive-telemetry".to_string()],
        )
        .await
        .unwrap();
    assert_eq!(adaptive_resources.len(), 1);
    assert_eq!(adaptive_resources[0].bucket_secs, 300);
    assert_eq!(adaptive_resources[0].sample_count, 5);
    assert_eq!(adaptive_resources[0].cpu_usage_sample_count, 5);
    assert_eq!(adaptive_resources[0].swap_sample_count, 1);
    assert_eq!(adaptive_resources[0].swap_total_bytes_max, Some(1_000));
    assert_eq!(adaptive_resources[0].swap_available_bytes_avg, Some(400));
    assert_eq!(adaptive_resources[0].swap_available_bytes_min, Some(400));
    assert_eq!(adaptive_resources[0].swap_used_ratio_avg, Some(0.6));
    assert_eq!(adaptive_resources[0].swap_used_ratio_max, Some(0.6));

    sqlx::query(
        r#"
        INSERT INTO telemetry_network_rates (
            client_id, interface, bucket_start, bucket_secs,
                sample_count, rx_bytes_sum, tx_bytes_sum,
                rx_bytes_avg, tx_bytes_avg,
                rx_bytes_last, tx_bytes_last, rx_counter_epoch, tx_counter_epoch,
                latest_observed_at
        ) VALUES
            ('adaptive-telemetry', 'eth0', to_timestamp($1::double precision), 60,
                1, 1000, 2000, 1000, 2000, 1000, 2000, 0, 0,
                to_timestamp($1::double precision)),
            ('adaptive-telemetry', 'eth0', to_timestamp($2::double precision), 300,
                5, 8000, 14500, 1600, 2900, 1600, 2900, 0, 0,
                to_timestamp(($2::bigint + 240)::double precision))
        "#,
    )
    .bind(adaptive_start.saturating_sub(60) as f64)
    .bind(adaptive_start as f64)
    .execute(&db.pool)
    .await
    .unwrap();
    let adaptive_network = db
        .repo
        .list_dashboard_telemetry_network_rates(
            10,
            Some(adaptive_start),
            Some(adaptive_start + 240),
            None,
            60,
            &["adaptive-telemetry".to_string()],
        )
        .await
        .unwrap();
    assert_eq!(adaptive_network.len(), 1);
    assert_eq!(adaptive_network[0].bucket_secs, 300);
    assert_eq!(adaptive_network[0].sample_count, 5);
    assert_eq!(adaptive_network[0].rx_bytes_delta, 600);

    let ping_target_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO ping_targets (id, name, host, probe_kind, selector_expression)
        VALUES ($1, 'Adaptive Ping', '1.1.1.1', 'icmp', '*')
        "#,
    )
    .bind(ping_target_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO ping_target_assignments (target_id, client_id, is_primary)
        VALUES ($1, 'adaptive-telemetry', TRUE)
        "#,
    )
    .bind(ping_target_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        WITH series AS (
            INSERT INTO telemetry_ping_series (client_id, target_id, generation)
            VALUES ('adaptive-telemetry', $1, 1)
            RETURNING id
        )
        INSERT INTO telemetry_ping_rollups (
            series_id, bucket_start, bucket_secs, sample_count, success_count,
            latency_sum_ms, latency_avg_ms, latency_min_ms, latency_max_ms,
            loss_ratio_avg, loss_ratio_sum, loss_ratio_max,
            latest_status, latest_checked_at
        ) SELECT
            id, to_timestamp($2::double precision), 300, 5, 5,
            60, 12, 12, 12, 0, 0, 0, 'ok',
            to_timestamp(($2::bigint + 299)::double precision)
        FROM series
        "#,
    )
    .bind(ping_target_id)
    .bind(adaptive_start as f64)
    .execute(&db.pool)
    .await
    .unwrap();
    let adaptive_ping = db
        .repo
        .list_ping_rollups(
            "adaptive-telemetry",
            Some(adaptive_start),
            Some(adaptive_start + 240),
            10,
            60,
        )
        .await
        .unwrap();
    assert_eq!(adaptive_ping.len(), 1);
    assert_eq!(adaptive_ping[0].bucket_secs, 300);
    assert_eq!(adaptive_ping[0].sample_count, 5);
    assert_eq!(adaptive_ping[0].success_count, 5);

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_retained_queries_preserve_authority_baselines_and_reset_gaps() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "retained-network-parity";
    insert_client(&db.pool, client_id, None).await;
    let base = crate::unix_now() / 300 * 300 - 3_600;

    sqlx::query(
        r#"
        INSERT INTO telemetry_network_rates (
            client_id, interface, bucket_start, bucket_secs, sample_count,
            rx_bytes_sum, tx_bytes_sum, rx_bytes_avg, tx_bytes_avg,
            rx_bytes_last, tx_bytes_last, rx_counter_epoch, tx_counter_epoch,
            latest_observed_at
        ) VALUES
            ($1, 'eth-authority', to_timestamp(($2::bigint - 600)::double precision), 60, 1,
                900, 1900, 900, 1900, 900, 1900, 0, 0,
                to_timestamp(($2::bigint - 600)::double precision)),
            ($1, 'eth-authority', to_timestamp(($2::bigint - 360)::double precision), 60, 1,
                990, 1990, 990, 1990, 990, 1990, 0, 0,
                to_timestamp(($2::bigint - 360)::double precision)),
            ($1, 'eth-authority', to_timestamp(($2::bigint - 600)::double precision), 300, 5,
                5000, 10000, 1000, 2000, 1000, 2000, 0, 0,
                to_timestamp(($2::bigint - 360)::double precision)),
            ($1, 'eth-authority', to_timestamp($2::double precision), 300, 5,
                7500, 12500, 1500, 2500, 1500, 2500, 0, 0,
                to_timestamp(($2::bigint + 240)::double precision)),
            ($1, 'eth-reset', to_timestamp(($2::bigint + 240)::double precision), 60, 1,
                1000, 2000, 1000, 2000, 1000, 2000, 0, 0,
                to_timestamp(($2::bigint + 240)::double precision)),
            ($1, 'eth-reset', to_timestamp(($2::bigint + 300)::double precision), 60, 1,
                100, 2100, 100, 2100, 100, 2100, 1, 0,
                to_timestamp(($2::bigint + 300)::double precision)),
            ($1, 'eth-reset', to_timestamp(($2::bigint + 360)::double precision), 60, 1,
                160, 2220, 160, 2220, 160, 2220, 1, 0,
                to_timestamp(($2::bigint + 360)::double precision))
        "#,
    )
    .bind(client_id)
    .bind(base as i64)
    .execute(&db.pool)
    .await
    .unwrap();

    let authority = db
        .repo
        .list_dashboard_telemetry_network_rates(
            10,
            Some(base),
            Some(base + 239),
            None,
            60,
            &[client_id.to_string()],
        )
        .await
        .unwrap();
    assert_eq!(authority.len(), 1);
    assert_eq!(authority[0].interface, "eth-authority");
    assert_eq!(authority[0].bucket_secs, 300);
    assert_eq!(authority[0].sample_count, 5);
    assert_eq!(authority[0].rx_bytes_delta, 500);
    assert_eq!(authority[0].tx_bytes_delta, 500);

    let reset = db
        .repo
        .list_dashboard_telemetry_network_rates(
            10,
            Some(base + 300),
            Some(base + 300),
            None,
            60,
            &[client_id.to_string()],
        )
        .await
        .unwrap();
    assert!(reset.iter().all(|row| row.interface != "eth-reset"));

    let recovered = db
        .repo
        .list_dashboard_telemetry_network_rates(
            10,
            Some(base + 360),
            Some(base + 360),
            None,
            60,
            &[client_id.to_string()],
        )
        .await
        .unwrap();
    let recovered = recovered
        .iter()
        .find(|row| row.interface == "eth-reset")
        .expect("the post-reset sample uses the reset row as its baseline");
    assert_eq!(recovered.rx_bytes_delta, 60);
    assert_eq!(recovered.tx_bytes_delta, 120);

    sqlx::query(
        r#"
        INSERT INTO telemetry_rollups (
            client_id, bucket_start, bucket_secs, sample_count,
            cpu_usage_sample_count, cpu_usage_sum, cpu_usage_avg,
            cpu_usage_max, cpu_cores_max,
            cpu_load_1_avg, cpu_load_1_sum, cpu_load_1_max,
            cpu_load_5_avg, cpu_load_5_sum, cpu_load_5_max,
            cpu_load_15_avg, cpu_load_15_sum, cpu_load_15_max,
            memory_total_bytes_max, memory_available_bytes_avg,
            memory_available_bytes_sum, memory_available_bytes_min,
            memory_used_ratio_avg, memory_used_ratio_sum, memory_used_ratio_max,
            disk_total_bytes_max, disk_available_bytes_avg,
            disk_available_bytes_sum, disk_available_bytes_min,
            disk_used_ratio_avg, disk_used_ratio_sum, disk_used_ratio_max,
            network_rx_bytes_max, network_tx_bytes_max, latest_observed_at
        ) VALUES
            ($1, to_timestamp(($2::bigint + 60)::double precision), 60, 1,
                1, 0.9, 0.9, 0.9, 2, 9, 9, 9, 9, 9, 9, 9, 9, 9,
                1000, 100, 100, 100, 0.9, 0.9, 0.9,
                2000, 200, 200, 200, 0.9, 0.9, 0.9, 0, 0,
                to_timestamp(($2::bigint + 60)::double precision)),
            ($1, to_timestamp($2::double precision), 300, 5,
                5, 1, 0.2, 0.3, 2, 2, 10, 3, 2, 10, 3, 2, 10, 3,
                1000, 800, 4000, 700, 0.2, 1.0, 0.3,
                2000, 1600, 8000, 1400, 0.2, 1.0, 0.3, 0, 0,
                to_timestamp(($2::bigint + 240)::double precision))
        "#,
    )
    .bind(client_id)
    .bind(base as i64)
    .execute(&db.pool)
    .await
    .unwrap();
    let resources = db
        .repo
        .list_dashboard_telemetry_rollups(
            10,
            Some(base + 60),
            Some(base + 60),
            None,
            60,
            &[client_id.to_string()],
        )
        .await
        .unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].bucket_secs, 300);
    assert_eq!(resources[0].sample_count, 5);
    assert_eq!(resources[0].cpu_load_1_avg, 2.0);

    let target_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO ping_targets (id, name, host, probe_kind, selector_expression)
        VALUES ($1, 'Retained parity Ping', '192.0.2.80', 'icmp', '*')
        "#,
    )
    .bind(target_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO ping_target_assignments (target_id, client_id, is_primary)
        VALUES ($1, $2, TRUE)
        "#,
    )
    .bind(target_id)
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        WITH series AS (
            INSERT INTO telemetry_ping_series (client_id, target_id, generation)
            VALUES ($1, $2, 1)
            RETURNING id
        )
        INSERT INTO telemetry_ping_rollups (
            series_id, bucket_start, bucket_secs, sample_count, success_count,
            latency_sum_ms, latency_avg_ms, latency_min_ms, latency_max_ms,
            loss_ratio_avg, loss_ratio_sum, loss_ratio_max,
            latest_status, latest_reason, latest_checked_at
        )
        SELECT id, to_timestamp(($3::bigint + 60)::double precision), 60, 1, 0,
            0, NULL, NULL, NULL, 1, 1, 1, 'down', 'timeout',
            to_timestamp(($3::bigint + 60)::double precision)
        FROM series
        UNION ALL
        SELECT id, to_timestamp($3::double precision), 300, 5, 4,
            40, 10, 8, 12, 0.2, 1, 1, 'degraded', 'packet_loss',
            to_timestamp(($3::bigint + 240)::double precision)
        FROM series
        "#,
    )
    .bind(client_id)
    .bind(target_id)
    .bind(base as i64)
    .execute(&db.pool)
    .await
    .unwrap();
    let ping = db
        .repo
        .list_ping_rollups(client_id, Some(base + 60), Some(base + 60), 10, 60)
        .await
        .unwrap();
    assert_eq!(ping.len(), 1);
    assert_eq!(ping[0].bucket_secs, 300);
    assert_eq!(ping[0].sample_count, 5);
    assert_eq!(ping[0].success_count, 4);
    assert_eq!(ping[0].latency_avg_ms, Some(10.0));
    assert!((ping[0].loss_ratio_avg - 0.2).abs() < 0.000_001);
    assert_eq!(ping[0].latest_status, "degraded");

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_current_ping_smooths_loss_and_keeps_latest_hard_failure_immediate() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let smoothed_client_id = "current-ping-smoothed";
    let threshold_client_id = "current-ping-threshold";
    insert_client(&db.pool, smoothed_client_id, None).await;
    insert_client(&db.pool, threshold_client_id, None).await;
    let smoothed_target_id = Uuid::new_v4();
    let threshold_target_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO ping_targets (
            id, name, host, probe_kind, selector_expression, generation
        ) VALUES
            ($1, 'Current Ping Smoothed', '192.0.2.10', 'icmp', '*', 2),
            ($2, 'Current Ping Threshold', '192.0.2.11', 'icmp', '*', 1)
        "#,
    )
    .bind(smoothed_target_id)
    .bind(threshold_target_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO ping_target_assignments (target_id, client_id, is_primary)
        VALUES ($1, $2, TRUE), ($3, $4, TRUE)
        "#,
    )
    .bind(smoothed_target_id)
    .bind(smoothed_client_id)
    .bind(threshold_target_id)
    .bind(threshold_client_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let latest_minute = crate::unix_now() / 60 * 60;
    sqlx::query(
        r#"
        WITH series AS (
            INSERT INTO telemetry_ping_series (client_id, target_id, generation)
            VALUES ($1, $2, 2)
            RETURNING id
        )
        INSERT INTO telemetry_ping_facts (
            series_id, observed_at, evidence_id, source_checked_unix, checked_unix,
            status, latency_avg_ms, loss_ratio, reason
        )
        SELECT
            series.id,
            to_timestamp(($3::bigint - minute_offset * 60)::double precision),
            $4,
            $3::bigint - minute_offset * 60,
            $3::bigint - minute_offset * 60,
            CASE WHEN minute_offset = 0 THEN 'degraded' ELSE 'ok' END,
            37,
            CASE WHEN minute_offset = 0 THEN 1.0 / 3.0 ELSE 0 END,
            CASE WHEN minute_offset = 0 THEN 'packet_loss' ELSE NULL END
        FROM series, generate_series(0, 14) AS offsets(minute_offset)
        "#,
    )
    .bind(smoothed_client_id)
    .bind(smoothed_target_id)
    .bind(latest_minute as i64)
    .bind(Uuid::new_v4())
    .execute(&db.pool)
    .await
    .unwrap();

    sqlx::query(
        r#"
        WITH series AS (
            INSERT INTO telemetry_ping_series (client_id, target_id, generation)
            VALUES ($1, $2, 1)
            RETURNING id
        )
        INSERT INTO telemetry_ping_facts (
            series_id, observed_at, evidence_id, source_checked_unix, checked_unix,
            status, latency_avg_ms, loss_ratio, reason
        )
        SELECT
            series.id,
            to_timestamp(($3::bigint - minute_offset * 60)::double precision),
            $4,
            $3::bigint - minute_offset * 60,
            $3::bigint - minute_offset * 60,
            CASE WHEN minute_offset BETWEEN 1 AND 3 THEN 'degraded' ELSE 'ok' END,
            41,
            CASE WHEN minute_offset BETWEEN 1 AND 3 THEN 1.0 / 3.0 ELSE 0 END,
            CASE WHEN minute_offset BETWEEN 1 AND 3 THEN 'packet_loss' ELSE NULL END
        FROM series, generate_series(0, 9) AS offsets(minute_offset)
        "#,
    )
    .bind(threshold_client_id)
    .bind(threshold_target_id)
    .bind(latest_minute as i64)
    .bind(Uuid::new_v4())
    .execute(&db.pool)
    .await
    .unwrap();

    // A hard failure for an obsolete generation must never leak into the
    // target's current generation.
    sqlx::query(
        r#"
        WITH series AS (
            INSERT INTO telemetry_ping_series (client_id, target_id, generation)
            VALUES ($1, $2, 1)
            RETURNING id
        )
        INSERT INTO telemetry_ping_current (
            series_id, latest_status, latency_avg_ms, rolling_loss_ratio,
            latest_reason, latest_checked_at
        )
        SELECT id, 'down', NULL, 1, 'obsolete_generation',
               to_timestamp($3::double precision)
        FROM series
        "#,
    )
    .bind(smoothed_client_id)
    .bind(smoothed_target_id)
    .bind(latest_minute as f64)
    .execute(&db.pool)
    .await
    .unwrap();

    let mut tx = db.pool.begin().await.unwrap();
    crate::repository_monitoring::upsert_postgres_ping_results(
        &mut tx,
        smoothed_client_id,
        latest_minute,
        &[PingTargetResult {
            target_id: smoothed_target_id.to_string(),
            generation: 2,
            checked_unix: latest_minute,
            status: "degraded".to_string(),
            latency_avg_ms: Some(37.0),
            loss_ratio: 1.0 / 3.0,
            reason: Some("packet_loss".to_string()),
        }],
        &[latest_minute],
    )
    .await
    .unwrap();
    crate::repository_monitoring::upsert_postgres_ping_results(
        &mut tx,
        threshold_client_id,
        latest_minute,
        &[PingTargetResult {
            target_id: threshold_target_id.to_string(),
            generation: 1,
            checked_unix: latest_minute,
            status: "ok".to_string(),
            latency_avg_ms: Some(41.0),
            loss_ratio: 0.0,
            reason: None,
        }],
        &[latest_minute],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let current = db
        .repo
        .current_primary_ping_for_clients(&[
            smoothed_client_id.to_string(),
            threshold_client_id.to_string(),
        ])
        .await
        .unwrap();
    let smoothed = current
        .iter()
        .find_map(|(client_id, ping)| (client_id == smoothed_client_id).then_some(ping))
        .unwrap();
    assert_eq!(smoothed.state, "ok");
    assert_eq!(smoothed.status.as_deref(), Some("ok"));
    assert!((smoothed.loss_ratio.unwrap() - 1.0 / 45.0).abs() < 1e-12);
    assert_eq!(smoothed.latency_avg_ms, Some(37.0));
    assert_eq!(smoothed.reason.as_deref(), Some("packet_loss"));

    let threshold = current
        .iter()
        .find_map(|(client_id, ping)| (client_id == threshold_client_id).then_some(ping))
        .unwrap();
    assert_eq!(threshold.state, "degraded");
    assert_eq!(threshold.status.as_deref(), Some("degraded"));
    assert!((threshold.loss_ratio.unwrap() - 0.1).abs() < 1e-12);
    assert_eq!(threshold.latency_avg_ms, Some(41.0));

    for latest_status in ["down", "error"] {
        sqlx::query(
            r#"
            UPDATE telemetry_ping_facts fact
            SET status = $1,
                latency_avg_ms = NULL,
                loss_ratio = 1,
                reason = $2
            FROM telemetry_ping_series series
            WHERE series.id = fact.series_id
              AND series.client_id = $3
              AND series.target_id = $4
              AND series.generation = 2
              AND fact.checked_unix = $5
            "#,
        )
        .bind(latest_status)
        .bind(format!("latest_{latest_status}"))
        .bind(smoothed_client_id)
        .bind(smoothed_target_id)
        .bind(latest_minute as i64)
        .execute(&db.pool)
        .await
        .unwrap();

        let mut tx = db.pool.begin().await.unwrap();
        crate::repository_monitoring::upsert_postgres_ping_results(
            &mut tx,
            smoothed_client_id,
            latest_minute,
            &[PingTargetResult {
                target_id: smoothed_target_id.to_string(),
                generation: 2,
                checked_unix: latest_minute,
                status: latest_status.to_string(),
                latency_avg_ms: None,
                loss_ratio: 1.0,
                reason: Some(format!("latest_{latest_status}")),
            }],
            &[latest_minute],
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let current = db
            .repo
            .current_primary_ping_for_clients(&[smoothed_client_id.to_string()])
            .await
            .unwrap();
        let ping = &current[0].1;
        assert_eq!(ping.state, latest_status);
        assert_eq!(ping.status.as_deref(), Some(latest_status));
        assert!((ping.loss_ratio.unwrap() - 1.0 / 15.0).abs() < 1e-12);
        assert_eq!(ping.latency_avg_ms, None);
        let expected_reason = format!("latest_{latest_status}");
        assert_eq!(ping.reason.as_deref(), Some(expected_reason.as_str()));
    }

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_ping_source_identity_counts_equal_chart_times_deterministically() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "ping-source-equal-chart-time";
    insert_client(&db.pool, client_id, None).await;
    let target_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO ping_targets (id, name, host, probe_kind, selector_expression)
        VALUES ($1, 'Equal chart time Ping', '192.0.2.40', 'icmp', '*')
        "#,
    )
    .bind(target_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO ping_target_assignments (target_id, client_id, is_primary)
        VALUES ($1, $2, TRUE)
        "#,
    )
    .bind(target_id)
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let checked_unix = crate::unix_now() / 60 * 60;
    let evidence_id = Uuid::new_v4();
    sqlx::query(
        r#"
        WITH series AS (
            INSERT INTO telemetry_ping_series (client_id, target_id, generation)
            VALUES ($1, $2, 1)
            RETURNING id
        )
        INSERT INTO telemetry_ping_facts (
            series_id, observed_at, evidence_id, source_checked_unix, checked_unix,
            status, latency_avg_ms, loss_ratio, reason
        )
        SELECT id, to_timestamp($3::double precision), $4, $5, $3, 'ok', 10, 0, NULL
        FROM series
        UNION ALL
        SELECT id, to_timestamp($3::double precision), $4, $6, $3, 'degraded', 20, 0.5,
               'higher source identity' FROM series
        "#,
    )
    .bind(client_id)
    .bind(target_id)
    .bind(checked_unix as i64)
    .bind(evidence_id)
    .bind(checked_unix as i64 + 10)
    .bind(checked_unix as i64 + 20)
    .execute(&db.pool)
    .await
    .unwrap();

    let rebuild = PingTargetResult {
        target_id: target_id.to_string(),
        generation: 1,
        checked_unix,
        status: "ok".to_string(),
        latency_avg_ms: Some(10.0),
        loss_ratio: 0.0,
        reason: None,
    };
    for source_checked_unix in [checked_unix + 20, checked_unix + 10] {
        let mut tx = db.pool.begin().await.unwrap();
        crate::repository_monitoring::upsert_postgres_ping_results(
            &mut tx,
            client_id,
            checked_unix,
            std::slice::from_ref(&rebuild),
            &[source_checked_unix],
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    let raw = db
        .repo
        .list_raw_ping_results(client_id, Some(checked_unix), Some(checked_unix), 10, 60)
        .await
        .unwrap();
    assert_eq!(raw.len(), 1);
    assert_eq!(raw[0].sample_count, 2);
    assert_eq!(raw[0].success_count, 2);
    assert_eq!(raw[0].latency_avg_ms, Some(15.0));
    assert_eq!(raw[0].latest_status, "degraded");
    assert_eq!(
        raw[0].latest_reason.as_deref(),
        Some("higher source identity")
    );
    assert_eq!(
        crate::util::parse_timestamp_unix(&raw[0].latest_checked_at),
        Some(checked_unix),
    );
    let primary = db
        .repo
        .list_raw_primary_ping_results_for_clients(
            &[client_id.to_string()],
            checked_unix,
            checked_unix,
            10,
            60,
        )
        .await
        .unwrap();
    assert_eq!(primary.len(), 1);
    assert_eq!(primary[0].sample_count, 2);
    assert_eq!(primary[0].latest_status, "degraded");
    assert_eq!(
        primary[0].latest_reason.as_deref(),
        Some("higher source identity")
    );
    let retained: (i32, String, Option<String>) = sqlx::query_as(
        r#"
        SELECT sample_count, latest_status, latest_reason
        FROM telemetry_ping_rollups
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        retained,
        (
            2,
            "degraded".to_string(),
            Some("higher source identity".to_string())
        )
    );
    let current: (String, Option<String>) =
        sqlx::query_as("SELECT latest_status, latest_reason FROM telemetry_ping_current")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        current,
        (
            "degraded".to_string(),
            Some("higher source identity".to_string())
        )
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_policy_rollup_lookup_is_selected_and_not_public_page_bounded() {
    const CLIENT_COUNT: i32 = 5_001;
    const PUBLIC_PAGE_SIZE: i64 = 5_000;

    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        r#"
        INSERT INTO clients (id, display_name, public_key, status)
        SELECT
            format('policy-rollup-scale-%s', value),
            format('Policy Rollup Scale %s', value),
            decode(lpad(to_hex(value), 64, '0'), 'hex'),
            'online'
        FROM generate_series(1, $1) AS generated(value)
        "#,
    )
    .bind(CLIENT_COUNT)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO telemetry_resource_latest (
            client_id, bucket_start, bucket_secs, sample_count,
            cpu_load_1_avg, cpu_load_1_sum, cpu_load_1_max,
            memory_total_bytes_max, memory_available_bytes_avg,
            memory_available_bytes_sum, memory_available_bytes_min,
            memory_used_ratio_avg, memory_used_ratio_sum, memory_used_ratio_max,
            disk_total_bytes_max, disk_available_bytes_avg,
            disk_available_bytes_sum, disk_available_bytes_min,
            disk_used_ratio_avg, disk_used_ratio_sum, disk_used_ratio_max,
            network_rx_bytes_max, network_tx_bytes_max, latest_observed_at
        )
        SELECT
            id, date_trunc('minute', now()), 60, 1,
            2.0, 2.0, 2.0, 1000, 500, 500, 500, 0.5, 0.5, 0.5,
            2000, 1500, 1500, 1500, 0.25, 0.25, 0.25, 0, 0, now()
        FROM visible_clients
        WHERE id LIKE 'policy-rollup-scale-%'
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let client_ids = (1..=CLIENT_COUNT)
        .map(|value| format!("policy-rollup-scale-{value}"))
        .collect::<Vec<_>>();

    let public_page = db
        .repo
        .list_latest_telemetry_rollups(PUBLIC_PAGE_SIZE, None, None)
        .await
        .unwrap();
    assert_eq!(public_page.len(), PUBLIC_PAGE_SIZE as usize);
    let selected = db
        .repo
        .list_latest_telemetry_rollups_for_clients(&client_ids, None)
        .await
        .unwrap();
    assert_eq!(selected.len(), CLIENT_COUNT as usize);
    let preview = db
        .repo
        .dry_run_fleet_alert_policy(&PolicyDryRunRequest {
            id: None,
            name: "postgres-large-fleet-policy".to_string(),
            enabled: true,
            selector_expression: "*".to_string(),
            rules: vec![postgres_metric_policy_rule_request(
                None,
                "all-client threshold",
                "warning",
            )],
            notes: None,
        })
        .await
        .unwrap();
    assert_eq!(preview.matched_vps_count, CLIENT_COUNT as usize);
    assert_eq!(preview.rule_previews[0].true_count, i64::from(CLIENT_COUNT));
    assert_eq!(preview.rule_previews[0].incomplete_count, 0);

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_fleet_alert_policy_regression_concurrent_name_upserts_share_identity() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let operator = postgres_network_operator(&db.repo).await;
    let request = || CreateFleetAlertPolicyRequest {
        id: None,
        name: "concurrent-name-policy".to_string(),
        enabled: true,
        selector_expression: "*".to_string(),
        rules: vec![postgres_metric_policy_rule_request(
            None,
            "cpu threshold",
            "warning",
        )],
        notes: None,
        confirmed: true,
        preview_hash: None,
    };
    let first_request = request();
    let second_request = request();

    let (first, second) = tokio::join!(
        db.repo.upsert_fleet_alert_policy(&first_request, &operator),
        db.repo
            .upsert_fleet_alert_policy(&second_request, &operator),
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(second.id, first.id);
    assert_eq!(second.rules[0].id, first.rules[0].id);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM policy_groups WHERE name = $1")
            .bind("concurrent-name-policy")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_fleet_alert_policy_regression_reads_legacy_overlapping_traffic_selectors() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let operator = postgres_network_operator(&db.repo).await;
    let client_id = "legacy-overlap-client";
    insert_client(&db.pool, client_id, None).await;
    sqlx::query(
        r#"
        INSERT INTO vps_rule_values (client_id, key, value_raw, value_json)
        VALUES (
            $1,
            'traffic.selectors',
            'eth0,eth0+rx',
            '{"selectors":[
                {"source":"host","interface":"eth0","direction":"total","canonical":"eth0"},
                {"source":"host","interface":"eth0","direction":"rx","canonical":"eth0+rx"}
            ]}'::jsonb
        )
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let rules = db
        .repo
        .list_vps_rules(&VpsRuleQuery {
            limit: Some(10),
            client_id: Some(client_id.to_string()),
            selector_expression: None,
            key: Some("traffic.selectors".to_string()),
            state: None,
        })
        .await
        .unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].value_raw, "eth0,eth0+rx");

    db.repo
        .upsert_fleet_alert_policy(
            &CreateFleetAlertPolicyRequest {
                id: None,
                name: "legacy-overlap-policy".to_string(),
                enabled: true,
                selector_expression: format!("id:{client_id}"),
                rules: vec![postgres_metric_policy_rule_request(
                    None,
                    "cpu threshold",
                    "warning",
                )],
                notes: None,
                confirmed: true,
                preview_hash: None,
            },
            &operator,
        )
        .await
        .unwrap();
    db.repo.evaluate_policy_rules().await.unwrap();

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_product_name_uses_the_fresh_canonical_rule_schema() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "product-name-client";
    insert_client(&db.pool, client_id, None).await;
    let operator = postgres_network_operator(&db.repo).await;
    let values = BTreeMap::from([(
        VPS_RULE_KEY_PRODUCT_NAME.to_string(),
        "  Storage-Box\t 4  ".to_string(),
    )]);
    let preview = db
        .repo
        .dry_run_vps_rules(&VpsRulesDryRunRequest {
            operation: "upsert".to_string(),
            selector_expression: format!("id:{client_id}"),
            values: values.clone(),
            keys: Vec::new(),
        })
        .await
        .unwrap();
    assert_eq!(preview.changed_row_count, 1);
    assert_eq!(preview.changes[0].after.as_deref(), Some("Storage-Box 4"));
    db.repo
        .bulk_upsert_vps_rules(
            &VpsRulesBulkUpsertRequest {
                selector_expression: format!("id:{client_id}"),
                values,
                confirmed: true,
                preview_hash: preview.preview_hash,
            },
            &operator,
        )
        .await
        .unwrap();

    let stored = sqlx::query_as::<_, (String, Value, chrono::DateTime<chrono::Utc>)>(
        "SELECT value_raw, value_json, updated_at FROM vps_rule_values WHERE client_id = $1 AND key = $2",
    )
    .bind(client_id)
    .bind(VPS_RULE_KEY_PRODUCT_NAME)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(stored.0, "Storage-Box 4");
    assert_eq!(stored.1["name"], "Storage-Box 4");
    assert_eq!(stored.1["display"], "Storage-Box 4");
    let audit_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM audit_logs WHERE action = 'fleet.vps_rules_updated'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let equivalent_values = BTreeMap::from([(
        VPS_RULE_KEY_PRODUCT_NAME.to_string(),
        "\n Storage-Box     4 \t".to_string(),
    )]);
    let equivalent_preview = db
        .repo
        .dry_run_vps_rules(&VpsRulesDryRunRequest {
            operation: "upsert".to_string(),
            selector_expression: format!("id:{client_id}"),
            values: equivalent_values.clone(),
            keys: Vec::new(),
        })
        .await
        .unwrap();
    assert_eq!(equivalent_preview.changed_row_count, 0);
    assert_eq!(equivalent_preview.changes[0].action, "unchanged");
    db.repo
        .bulk_upsert_vps_rules(
            &VpsRulesBulkUpsertRequest {
                selector_expression: format!("id:{client_id}"),
                values: equivalent_values,
                confirmed: true,
                preview_hash: equivalent_preview.preview_hash,
            },
            &operator,
        )
        .await
        .unwrap();

    let after = sqlx::query_as::<_, (String, Value, chrono::DateTime<chrono::Utc>)>(
        "SELECT value_raw, value_json, updated_at FROM vps_rule_values WHERE client_id = $1 AND key = $2",
    )
    .bind(client_id)
    .bind(VPS_RULE_KEY_PRODUCT_NAME)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(after, stored);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_logs WHERE action = 'fleet.vps_rules_updated'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        audit_count
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_vps_rule_spacing_edit_is_canonical_and_does_not_update_the_row() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "canonical-rule-client";
    insert_client(&db.pool, client_id, None).await;
    let operator = postgres_network_operator(&db.repo).await;
    let first_values = BTreeMap::from([(
        format!(" {VPS_RULE_KEY_NETWORK_PORT_SPEED} "),
        " 001.000   gbps ".to_string(),
    )]);
    let first_preview = db
        .repo
        .dry_run_vps_rules(&VpsRulesDryRunRequest {
            operation: "upsert".to_string(),
            selector_expression: format!("id:{client_id}"),
            values: first_values.clone(),
            keys: Vec::new(),
        })
        .await
        .unwrap();
    db.repo
        .bulk_upsert_vps_rules(
            &VpsRulesBulkUpsertRequest {
                selector_expression: format!("id:{client_id}"),
                values: first_values,
                confirmed: true,
                preview_hash: first_preview.preview_hash,
            },
            &operator,
        )
        .await
        .unwrap();
    let audit_count_after_create = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM audit_logs WHERE action = 'fleet.vps_rules_updated'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let changed_values = BTreeMap::from([(
        VPS_RULE_KEY_NETWORK_PORT_SPEED.to_string(),
        " 001.500   gbps ".to_string(),
    )]);
    let changed_preview = db
        .repo
        .dry_run_vps_rules(&VpsRulesDryRunRequest {
            operation: "upsert".to_string(),
            selector_expression: format!("id:{client_id}"),
            values: changed_values.clone(),
            keys: Vec::new(),
        })
        .await
        .unwrap();
    assert_eq!(changed_preview.changed_row_count, 1);
    assert_eq!(changed_preview.changes[0].action, "set");
    db.repo
        .bulk_upsert_vps_rules(
            &VpsRulesBulkUpsertRequest {
                selector_expression: format!("id:{client_id}"),
                values: changed_values,
                confirmed: true,
                preview_hash: changed_preview.preview_hash,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT value_raw FROM vps_rule_values WHERE client_id = $1 AND key = $2",
        )
        .bind(client_id)
        .bind(VPS_RULE_KEY_NETWORK_PORT_SPEED)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        "1.5 Gbps"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_logs WHERE action = 'fleet.vps_rules_updated'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        audit_count_after_create + 1
    );

    sqlx::query("UPDATE vps_rule_values SET value_raw = $3 WHERE client_id = $1 AND key = $2")
        .bind(client_id)
        .bind(VPS_RULE_KEY_NETWORK_PORT_SPEED)
        .bind(" 001.500   gbps ")
        .execute(&db.pool)
        .await
        .unwrap();
    let normalization_audit_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM audit_logs WHERE action = 'fleet.vps_rules_updated'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let normalization_values = BTreeMap::from([(
        VPS_RULE_KEY_NETWORK_PORT_SPEED.to_string(),
        "1.5Gbps".to_string(),
    )]);
    let normalization_preview = db
        .repo
        .dry_run_vps_rules(&VpsRulesDryRunRequest {
            operation: "upsert".to_string(),
            selector_expression: format!("id:{client_id}"),
            values: normalization_values.clone(),
            keys: Vec::new(),
        })
        .await
        .unwrap();
    assert_eq!(normalization_preview.changed_row_count, 1);
    assert_eq!(normalization_preview.changes[0].action, "set");
    assert_eq!(
        normalization_preview.changes[0].before.as_deref(),
        Some(" 001.500   gbps ")
    );
    assert_eq!(
        normalization_preview.changes[0].after.as_deref(),
        Some("1.5 Gbps")
    );
    db.repo
        .bulk_upsert_vps_rules(
            &VpsRulesBulkUpsertRequest {
                selector_expression: format!("id:{client_id}"),
                values: normalization_values,
                confirmed: true,
                preview_hash: normalization_preview.preview_hash,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT value_raw FROM vps_rule_values WHERE client_id = $1 AND key = $2",
        )
        .bind(client_id)
        .bind(VPS_RULE_KEY_NETWORK_PORT_SPEED)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        "1.5 Gbps"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_logs WHERE action = 'fleet.vps_rules_updated'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        normalization_audit_count + 1
    );

    let before_updated_at = sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
        "SELECT updated_at FROM vps_rule_values WHERE client_id = $1 AND key = $2",
    )
    .bind(client_id)
    .bind(VPS_RULE_KEY_NETWORK_PORT_SPEED)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let before_audit_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM audit_logs WHERE action = 'fleet.vps_rules_updated'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let edit_values = BTreeMap::from([(
        VPS_RULE_KEY_NETWORK_PORT_SPEED.to_string(),
        "1.5Gbps".to_string(),
    )]);
    let edit_preview = db
        .repo
        .dry_run_vps_rules(&VpsRulesDryRunRequest {
            operation: "upsert".to_string(),
            selector_expression: format!("id:{client_id}"),
            values: edit_values.clone(),
            keys: Vec::new(),
        })
        .await
        .unwrap();
    assert_eq!(edit_preview.changed_row_count, 0);
    assert_eq!(edit_preview.changes[0].action, "unchanged");
    db.repo
        .bulk_upsert_vps_rules(
            &VpsRulesBulkUpsertRequest {
                selector_expression: format!("id:{client_id}"),
                values: edit_values,
                confirmed: true,
                preview_hash: edit_preview.preview_hash,
            },
            &operator,
        )
        .await
        .unwrap();

    let row = sqlx::query(
        "SELECT value_raw, value_json, updated_at FROM vps_rule_values WHERE client_id = $1 AND key = $2",
    )
    .bind(client_id)
    .bind(VPS_RULE_KEY_NETWORK_PORT_SPEED)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("value_raw"), "1.5 Gbps");
    assert_eq!(
        row.get::<sqlx::types::Json<Value>, _>("value_json").0,
        json!({"bps": 1_500_000_000_i64, "display": "1.5 Gbps"})
    );
    assert_eq!(
        row.get::<chrono::DateTime<chrono::Utc>, _>("updated_at"),
        before_updated_at
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_logs WHERE action = 'fleet.vps_rules_updated'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        before_audit_count
    );

    let duplicate_error = db
        .repo
        .dry_run_vps_rules(&VpsRulesDryRunRequest {
            operation: "upsert".to_string(),
            selector_expression: format!("id:{client_id}"),
            values: BTreeMap::from([
                (
                    VPS_RULE_KEY_NETWORK_PORT_SPEED.to_string(),
                    "1 Gbps".to_string(),
                ),
                (
                    format!(" {VPS_RULE_KEY_NETWORK_PORT_SPEED} "),
                    "2 Gbps".to_string(),
                ),
            ]),
            keys: Vec::new(),
        })
        .await
        .unwrap_err();
    assert!(duplicate_error
        .to_string()
        .contains("vps_rules_duplicate_key"));

    let unset_keys = vec![format!(" {VPS_RULE_KEY_NETWORK_PORT_SPEED} ")];
    let unset_preview = db
        .repo
        .dry_run_vps_rules(&VpsRulesDryRunRequest {
            operation: "unset".to_string(),
            selector_expression: format!("id:{client_id}"),
            values: BTreeMap::new(),
            keys: unset_keys.clone(),
        })
        .await
        .unwrap();
    assert_eq!(unset_preview.changed_row_count, 1);
    db.repo
        .bulk_unset_vps_rules(
            &VpsRulesBulkUnsetRequest {
                selector_expression: format!("id:{client_id}"),
                keys: unset_keys,
                confirmed: true,
                preview_hash: unset_preview.preview_hash,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM vps_rule_values WHERE client_id = $1 AND key = $2",
        )
        .bind(client_id)
        .bind(VPS_RULE_KEY_NETWORK_PORT_SPEED)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_single_connection_serializes_stale_self_referential_rule_confirmations() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "concurrent-rule-client";
    insert_client(&db.pool, client_id, None).await;
    sqlx::query(
        r#"
        INSERT INTO vps_rule_values (client_id, key, value_raw, value_json)
        VALUES ($1, $2, '1', '{"day":1}'::jsonb)
        "#,
    )
    .bind(client_id)
    .bind(VPS_RULE_KEY_TRAFFIC_RESET_DAY)
    .execute(&db.pool)
    .await
    .unwrap();
    let single_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*db.pool.connect_options()).clone())
        .await
        .unwrap();
    let single_repo = Repository::Postgres(single_pool.clone());
    let operator = postgres_network_operator(&db.repo).await;
    let selector_expression = "vps.rules:traffic.reset_day <= 1".to_string();
    let values = BTreeMap::from([(VPS_RULE_KEY_TRAFFIC_RESET_DAY.to_string(), "2".to_string())]);
    let preview = single_repo
        .dry_run_vps_rules(&VpsRulesDryRunRequest {
            operation: "upsert".to_string(),
            selector_expression: selector_expression.clone(),
            values: values.clone(),
            keys: Vec::new(),
        })
        .await
        .unwrap();
    assert_eq!(preview.matched_vps_count, 1);
    let left_request = VpsRulesBulkUpsertRequest {
        selector_expression: selector_expression.clone(),
        values: values.clone(),
        confirmed: true,
        preview_hash: preview.preview_hash.clone(),
    };
    let right_request = VpsRulesBulkUpsertRequest {
        selector_expression,
        values,
        confirmed: true,
        preview_hash: preview.preview_hash,
    };
    let (left, right) = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(
            single_repo.bulk_upsert_vps_rules(&left_request, &operator),
            single_repo.bulk_upsert_vps_rules(&right_request, &operator),
        )
    })
    .await
    .expect("single-connection rule mutations must not deadlock");
    let results = [left, right];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let error = results
        .into_iter()
        .find_map(Result::err)
        .expect("one stale confirmation");
    assert!(error
        .to_string()
        .contains("vps_rules_preview_hash_mismatch"));
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT value_raw FROM vps_rule_values WHERE client_id = $1 AND key = $2",
        )
        .bind(client_id)
        .bind(VPS_RULE_KEY_TRAFFIC_RESET_DAY)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        "2"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_logs WHERE action = 'fleet.vps_rules_updated'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        1
    );
    drop(single_repo);
    single_pool.close().await;
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_confirmed_bulk_tag_mutation_waits_for_agent_lifecycle_lock() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "tag-lifecycle-client";
    insert_client(&db.pool, client_id, None).await;

    let mut lifecycle_blocker = db.pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('vpsman.agent_key_lifecycle'))")
        .execute(&mut *lifecycle_blocker)
        .await
        .unwrap();

    let mutation_repo = db.repo.clone();
    let request = BulkTagMutationRequest {
        action: BulkTagMutationAction::Add,
        tag: "serialized-tag".to_string(),
        selector_expression: format!("id:{client_id}"),
        target_client_ids: vec![client_id.to_string()],
        confirmed: true,
        preview_hash: None,
        privilege_assertion: None,
    };
    let mutation_task =
        tokio::spawn(async move { mutation_repo.bulk_mutate_tags(&request, false).await });

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting_for_lifecycle_lock: bool = sqlx::query_scalar(
                r#"
                SELECT EXISTS (
                    SELECT 1
                    FROM pg_stat_activity
                    WHERE datname = current_database()
                      AND pid <> pg_backend_pid()
                      AND state = 'active'
                      AND wait_event_type = 'Lock'
                      AND query LIKE '%vpsman.agent_key_lifecycle%'
                )
                "#,
            )
            .fetch_one(&db.pool)
            .await
            .unwrap();
            if waiting_for_lifecycle_lock {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("confirmed tag mutation should wait for the agent lifecycle lock");

    lifecycle_blocker.rollback().await.unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), mutation_task)
        .await
        .expect("tag mutation should finish after the lifecycle lock is released")
        .expect("tag mutation task should not panic")
        .unwrap();
    assert_eq!(response.changed_count, 1);
    assert!(db
        .repo
        .list_agents()
        .await
        .unwrap()
        .into_iter()
        .find(|agent| agent.id == client_id)
        .unwrap()
        .tags
        .iter()
        .any(|tag| tag == "serialized-tag"));

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_tag_order_setting_governs_every_tag_creation_path_atomically() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let operator = postgres_network_operator(&db.repo).await;
    insert_client(&db.pool, "tag-order-client-a", None).await;
    for tag in [
        "provider:A10",
        "provider:A2",
        "country:US",
        "provider:B10",
        "provider:B2",
        "plain",
    ] {
        db.repo.create_tag_name(tag.to_string()).await.unwrap();
    }

    let enabled = db
        .repo
        .update_tag_order(
            &UpdateTagOrderRequest {
                ordered_tags: [
                    "provider:A10",
                    "provider:A2",
                    "country:US",
                    "provider:B10",
                    "provider:B2",
                    "plain",
                ]
                .into_iter()
                .map(str::to_string)
                .collect(),
                namespace_natural_sort_enabled: true,
            },
            operator.operator.id,
        )
        .await
        .unwrap();
    assert!(enabled.namespace_natural_sort_enabled);

    db.repo
        .create_tag_name("provider:A02".to_string())
        .await
        .unwrap();
    db.repo
        .assign_agent_tag("tag-order-client-a", "provider:B1")
        .await
        .unwrap();
    db.repo
        .bulk_mutate_tags(
            &BulkTagMutationRequest {
                action: BulkTagMutationAction::Add,
                tag: "provider:B02".to_string(),
                selector_expression: "id:tag-order-client-a".to_string(),
                target_client_ids: vec!["tag-order-client-a".to_string()],
                confirmed: true,
                preview_hash: None,
                privilege_assertion: None,
            },
            false,
        )
        .await
        .unwrap();
    db.repo
        .upsert_agent_identity(
            &UpsertAgentIdentityRequest {
                client_id: Some("tag-order-client-b".to_string()),
                client_public_key_hex: "42".repeat(32),
                display_name: Some("Tag order client B".to_string()),
                tags: vec!["provider:A1".to_string()],
                replace_existing_key: false,
                confirmed: true,
                privilege_assertion: None,
            },
            &operator,
        )
        .await
        .unwrap();

    let state = db.repo.tag_order_state().await.unwrap();
    assert!(state.namespace_natural_sort_enabled);
    assert_eq!(
        state
            .tags
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        [
            "provider:A2",
            "provider:A10",
            "country:US",
            "provider:A1",
            "provider:A02",
            "provider:B1",
            "provider:B2",
            "provider:B02",
            "provider:B10",
            "plain",
        ]
    );
    let setting: (serde_json::Value, Option<Uuid>) = sqlx::query_as(
        r#"
        SELECT value_json, updated_by
        FROM fleet_tag_settings
        WHERE setting_key = 'order.namespace_natural_sort_enabled'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(setting, (json!(true), Some(operator.operator.id)));

    let stale = db
        .repo
        .update_tag_order(
            &UpdateTagOrderRequest {
                ordered_tags: vec!["unknown:tag".to_string()],
                namespace_natural_sort_enabled: false,
            },
            operator.operator.id,
        )
        .await
        .unwrap_err();
    assert!(stale.to_string().contains("unknown_tag"));
    let after_rejection = db.repo.tag_order_state().await.unwrap();
    assert!(after_rejection.namespace_natural_sort_enabled);
    assert_eq!(
        after_rejection
            .tags
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        state
            .tags
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>()
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_concurrent_tag_create_and_order_save_share_one_serialization_lock() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let operator = postgres_network_operator(&db.repo).await;
    db.repo
        .create_tag_name("provider:A10".to_string())
        .await
        .unwrap();
    db.repo
        .create_tag_name("provider:A2".to_string())
        .await
        .unwrap();
    let create_repo = db.repo.clone();
    let update_repo = db.repo.clone();
    let update_operator_id = operator.operator.id;
    let (created, updated) = tokio::time::timeout(Duration::from_secs(10), async move {
        let update_request = UpdateTagOrderRequest {
            ordered_tags: vec!["provider:A10".to_string(), "provider:A2".to_string()],
            namespace_natural_sort_enabled: true,
        };
        tokio::join!(
            create_repo.create_tag_name("provider:A02".to_string()),
            update_repo.update_tag_order(&update_request, update_operator_id,)
        )
    })
    .await
    .expect("tag create and order save must not deadlock");
    created.unwrap();
    updated.unwrap();

    let state = db.repo.tag_order_state().await.unwrap();
    assert!(state.namespace_natural_sort_enabled);
    assert_eq!(
        state
            .tags
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        ["provider:A2", "provider:A02", "provider:A10"]
    );
    let orders =
        sqlx::query_scalar::<_, i64>("SELECT display_order FROM tags ORDER BY display_order")
            .fetch_all(&db.pool)
            .await
            .unwrap();
    assert_eq!(orders, vec![1024, 2048, 3072]);

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_concurrent_tag_delete_save_and_create_share_one_serialization_lock() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let operator = postgres_network_operator(&db.repo).await;
    for tag in ["provider:A10", "provider:A2", "country:SG"] {
        db.repo.create_tag_name(tag.to_string()).await.unwrap();
    }
    let delete_repo = db.repo.clone();
    let create_repo = db.repo.clone();
    let update_repo = db.repo.clone();
    let update_operator_id = operator.operator.id;
    let (deleted, created, updated) = tokio::time::timeout(Duration::from_secs(10), async move {
        let update_request = UpdateTagOrderRequest {
            ordered_tags: vec!["country:SG".to_string()],
            namespace_natural_sort_enabled: true,
        };
        tokio::join!(
            delete_repo.delete_tag("provider:A10", true, false),
            create_repo.create_tag_name("provider:A02".to_string()),
            update_repo.update_tag_order(&update_request, update_operator_id),
        )
    })
    .await
    .expect("tag delete, create, and order save must not deadlock");
    deleted.unwrap();
    created.unwrap();
    updated.unwrap();

    let state = db.repo.tag_order_state().await.unwrap();
    assert!(state.namespace_natural_sort_enabled);
    assert_eq!(
        state
            .tags
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        ["country:SG", "provider:A2", "provider:A02"]
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_stale_tag_order_save_keeps_new_tag_in_last_namespace_block() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let operator = postgres_network_operator(&db.repo).await;
    for tag in ["provider:A", "country:US", "provider:B", "plain"] {
        db.repo.create_tag_name(tag.to_string()).await.unwrap();
    }
    let stale_order = ["provider:A", "country:US", "provider:B", "plain"]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    db.repo
        .create_tag_name("provider:C".to_string())
        .await
        .unwrap();

    let saved = db
        .repo
        .update_tag_order(
            &UpdateTagOrderRequest {
                ordered_tags: stale_order,
                namespace_natural_sort_enabled: false,
            },
            operator.operator.id,
        )
        .await
        .unwrap();
    assert_eq!(
        saved
            .tags
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        [
            "provider:A",
            "country:US",
            "provider:B",
            "provider:C",
            "plain",
        ]
    );

    db.cleanup().await;
}

#[tokio::test]
async fn filter_limit_regression_postgres_rules_and_policies() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let target_client_id = "zzz-filter-target";
    insert_client(&db.pool, target_client_id, None).await;
    sqlx::query(
        r#"
        INSERT INTO clients (
            id,
            display_name,
            public_key,
            status,
            internal_build_number,
            capabilities
        )
        SELECT
            'aaa-filter-' || lpad(value::text, 3, '0'),
            'AAA Filter ' || lpad(value::text, 3, '0'),
            decode(lpad(to_hex(value), 64, '0'), 'hex'),
            'online',
            1,
            '{}'::jsonb
        FROM generate_series(1, 21) AS series(value)
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO vps_rule_values (client_id, key, value_raw, value_json)
        SELECT client.id, rule.key, rule.value_raw, rule.value_json
        FROM visible_clients client
        CROSS JOIN (
            VALUES
                ('traffic.reset_day', '1', '{"day":1}'::jsonb),
                ('traffic.quota.total', '1GB', '{"bytes":1000000000}'::jsonb),
                ('traffic.quota.rx', '1GB', '{"bytes":1000000000}'::jsonb),
                ('traffic.quota.tx', '1GB', '{"bytes":1000000000}'::jsonb),
                (
                    'traffic.selectors',
                    'eth0',
                    '{"selectors":[{"source":"host","interface":"eth0","direction":"total","canonical":"eth0"}]}'::jsonb
                )
        ) AS rule(key, value_raw, value_json)
        WHERE client.id LIKE 'aaa-filter-%'
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO vps_rule_values (client_id, key, value_raw, value_json)
        VALUES
            ($1, 'traffic.reset_day', '1', '{"day":1}'::jsonb),
            ($1, 'traffic.quota.total', '1GB', '{"bytes":1000000000}'::jsonb),
            ($1, 'traffic.quota.rx', '1GB', '{"bytes":1000000000}'::jsonb),
            ($1, 'traffic.quota.tx', '1GB', '{"bytes":1000000000}'::jsonb),
            (
                $1,
                'traffic.selectors',
                'eth0',
                '{"selectors":[{"source":"host","interface":"eth0","direction":"total","canonical":"eth0"}]}'::jsonb
            )
        "#,
    )
    .bind(target_client_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let effective = db.repo.effective_vps_rules(target_client_id).await.unwrap();
    assert_eq!(effective.len(), 5);
    assert!(effective
        .iter()
        .all(|rule| rule.client_id == target_client_id));

    let client_filtered = db
        .repo
        .list_vps_rules(&VpsRuleQuery {
            limit: Some(2),
            client_id: Some(target_client_id.to_string()),
            selector_expression: None,
            key: None,
            state: None,
        })
        .await
        .unwrap();
    assert_eq!(client_filtered.len(), 2);
    assert!(client_filtered
        .iter()
        .all(|rule| rule.client_id == target_client_id));

    let selector_filtered = db
        .repo
        .list_vps_rules(&VpsRuleQuery {
            limit: Some(2),
            client_id: None,
            selector_expression: Some(format!("id:{target_client_id}")),
            key: None,
            state: Some("ok".to_string()),
        })
        .await
        .unwrap();
    assert_eq!(selector_filtered.len(), 2);
    assert!(selector_filtered
        .iter()
        .all(|rule| rule.client_id == target_client_id));

    let matching_policy_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO policy_groups (id, name, enabled, selector_expression)
        VALUES
            ($1, 'aaa-filter-policy-1', TRUE, 'id:not-present'),
            ($2, 'aaa-filter-policy-2', TRUE, 'id:not-present'),
            ($3, 'zzz-filter-policy', TRUE, $4)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .bind(matching_policy_id)
    .bind(format!("id:{target_client_id}"))
    .execute(&db.pool)
    .await
    .unwrap();

    let client_policies = db
        .repo
        .list_fleet_alert_policies(1, Some(true), None, Some(target_client_id), true)
        .await
        .unwrap();
    assert_eq!(client_policies.len(), 1);
    assert_eq!(client_policies[0].id, matching_policy_id);

    let selector_policies = db
        .repo
        .list_fleet_alert_policies(
            1,
            Some(true),
            Some(&format!("id:{target_client_id}")),
            None,
            true,
        )
        .await
        .unwrap();
    assert_eq!(selector_policies.len(), 1);
    assert_eq!(selector_policies[0].id, matching_policy_id);

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_traffic_accounting_ignores_more_than_200k_unrelated_old_rows() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let old_client_id = "traffic-old-history";
    let target_client_id = "traffic-current-cycle";
    insert_client(&db.pool, old_client_id, None).await;
    insert_client(&db.pool, target_client_id, None).await;

    // Keep the configured boundary well behind "now", including across a
    // midnight rollover while this test is running.
    let today = Utc::now().day();
    let reset_day = if today > 14 { today - 14 } else { today + 14 };
    sqlx::query(
        r#"
        INSERT INTO vps_rule_values (client_id, key, value_raw, value_json)
        VALUES
            (
                $1,
                'traffic.reset_day',
                $2,
                jsonb_build_object('day', $3::integer)
            ),
            (
                $1,
                'traffic.selectors',
                'eth0',
                '{"selectors":[{"source":"host","interface":"eth0","direction":"total","canonical":"eth0"}]}'::jsonb
            ),
            (
                $1,
                'traffic.quota.total',
                '1TB',
                '{"bytes":1000000000000,"display":"1 TB"}'::jsonb
            )
        "#,
    )
    .bind(target_client_id)
    .bind(reset_day.to_string())
    .bind(reset_day as i32)
    .execute(&db.pool)
    .await
    .unwrap();
    let cycle_start = chrono::DateTime::parse_from_rfc3339(
        db.repo
            .get_traffic_accounting(target_client_id)
            .await
            .unwrap()
            .cycle_start
            .as_deref()
            .expect("monthly traffic has a cycle start"),
    )
    .unwrap()
    .timestamp();

    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id,
            source_kind,
            interface,
            observed_at,
            rx_bytes,
            tx_bytes,
            rx_counter_epoch,
            tx_counter_epoch,
            sample_source
        )
        SELECT
            $1,
            'host',
            'eth0',
            to_timestamp(
                ($2::bigint + generated.sample::bigint * 60)::double precision
            ),
            generated.sample::bigint,
            generated.sample::bigint,
            0,
            0,
            'test'
        FROM generate_series(1, 200001) AS generated(sample)
        "#,
    )
    .bind(old_client_id)
    .bind(cycle_start - 200_002_i64 * 60)
    .execute(&db.pool)
    .await
    .unwrap();

    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id,
            source_kind,
            interface,
            observed_at,
            rx_bytes,
            tx_bytes,
            rx_counter_epoch,
            tx_counter_epoch,
            sample_source
        )
        VALUES
            ($1, 'host', 'eth0', to_timestamp($2::double precision), 100, 200, 0, 0, 'test'),
            ($1, 'host', 'eth0', to_timestamp($3::double precision), 130, 260, 0, 0, 'test'),
            ($1, 'host', 'eth0', to_timestamp($4::double precision), 10, 300, 1, 0, 'test'),
            ($1, 'host', 'eth0', to_timestamp($5::double precision), 20, 320, 1, 0, 'test'),
            ($1, 'host', 'eth0', to_timestamp($6::double precision), 30, 340, 1, 0, 'test')
        "#,
    )
    .bind(target_client_id)
    .bind((cycle_start - 60) as f64)
    .bind(cycle_start as f64)
    .bind((cycle_start + 60) as f64)
    .bind((cycle_start + 120) as f64)
    .bind((cycle_start + 180) as f64)
    .execute(&db.pool)
    .await
    .unwrap();

    let accounting = db
        .repo
        .get_traffic_accounting(target_client_id)
        .await
        .unwrap();
    assert_eq!(accounting.client_id, target_client_id);
    assert_eq!(accounting.rx_bytes, 50);
    assert_eq!(accounting.tx_bytes, 140);
    assert_eq!(accounting.total_bytes, 190);
    assert_eq!(accounting.latest_rx_bytes, 30);
    assert_eq!(accounting.latest_tx_bytes, 340);
    assert_eq!(accounting.latest_total_bytes, 370);
    assert_eq!(accounting.counter_epochs_seen, 2);
    assert_eq!(
        chrono::DateTime::parse_from_rfc3339(
            accounting
                .last_sample_at
                .as_deref()
                .expect("current-cycle traffic sample is present")
        )
        .unwrap()
        .timestamp(),
        cycle_start + 180
    );

    let retention_client_id = "traffic-retention-baseline";
    insert_client(&db.pool, retention_client_id, None).await;
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source
        )
        VALUES
            ($1, 'host', 'eth0', to_timestamp(60), 10, 10, 0, 0, 'test'),
            ($1, 'host', 'eth0', to_timestamp(120), 20, 20, 0, 0, 'test'),
            ($1, 'host', 'eth0', to_timestamp(600), 100, 100, 0, 0, 'test')
        "#,
    )
    .bind(retention_client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let pruned = db
        .repo
        .prune_history_domain(
            &HistoryRetentionPrunePlan {
                domain: HistoryDomain::TrafficCounterSamples,
                prune_limit: 100,
                enabled: true,
            },
            300,
            false,
        )
        .await
        .unwrap();
    assert_eq!(pruned.pruned_rows, 1);
    let retained: Vec<i64> = sqlx::query_scalar(
        r#"
        SELECT EXTRACT(EPOCH FROM observed_at)::bigint
        FROM traffic_counter_samples
        WHERE client_id = $1
        ORDER BY observed_at ASC
        "#,
    )
    .bind(retention_client_id)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(retained, vec![120, 600]);

    let rollup_retention_client_id = "traffic-retention-tiered";
    insert_client(&db.pool, rollup_retention_client_id, None).await;
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source
        ) VALUES ($1, 'host', 'eth0', to_timestamp(7200), 1, 1, 0, 0, 'test')
        "#,
    )
    .bind(rollup_retention_client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        ) VALUES
            ($1, 'host', 'eth0', 'live', 3600, to_timestamp(0),
                1, 1, 1, 1, 1, 0, 0, 0, to_timestamp(0), to_timestamp(0)),
            ($1, 'host', 'eth0', 'live', 3600, to_timestamp(3600),
                1, 1, 1, 1, 1, 0, 0, 0, to_timestamp(3600), to_timestamp(3600))
        "#,
    )
    .bind(rollup_retention_client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let tiered_prune_plan = HistoryRetentionPrunePlan {
        domain: HistoryDomain::TrafficCounterSamples,
        prune_limit: 1,
        enabled: true,
    };
    let preview = db
        .repo
        .prune_history_domain(&tiered_prune_plan, 7201, true)
        .await
        .unwrap();
    assert_eq!((preview.matched_rows, preview.pruned_rows), (1, 0));
    let applied = db
        .repo
        .prune_history_domain(&tiered_prune_plan, 7201, false)
        .await
        .unwrap();
    assert_eq!((applied.matched_rows, applied.pruned_rows), (1, 1));
    let retained_rollups: i64 =
        sqlx::query_scalar("SELECT count(*) FROM traffic_counter_rollups WHERE client_id = $1")
            .bind(rollup_retention_client_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(retained_rollups, 1);

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_no_reset_traffic_combines_exact_transitions_with_the_rollup_ledger() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "traffic-no-reset-tier-ledger";
    let now_unix = Utc::now().timestamp().div_euclid(60) * 60;
    insert_client(&db.pool, client_id, None).await;

    sqlx::query(
        r#"
        INSERT INTO vps_rule_values (client_id, key, value_raw, value_json)
        VALUES
            ($1, 'traffic.reset_day', '-1', '{"day":-1}'::jsonb),
            (
                $1,
                'traffic.selectors',
                'eth0',
                '{"selectors":[{"source":"host","interface":"eth0","direction":"total","canonical":"eth0"}]}'::jsonb
            ),
            (
                $1,
                'traffic.quota.total',
                '1TB',
                '{"bytes":1000000000000,"display":"1 TB"}'::jsonb
            )
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        WITH bucket AS (
            SELECT date_bin(
                '1 day', now() - interval '100 days',
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
            ) AS bucket_start
        )
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        )
        SELECT
            $1, 'host', 'eth0', origin_kind, 86400, bucket_start,
            rx_bytes, tx_bytes, 1, 1, 1, 0, 0, 0,
            bucket_start + offset_secs * interval '1 second',
            bucket_start + offset_secs * interval '1 second'
        FROM bucket
        CROSS JOIN (
            VALUES
                ('vnstat_import'::text, 30::bigint, 50::bigint, 60),
                ('live'::text, 15::bigint, 25::bigint, 120)
        ) contribution(origin_kind, rx_bytes, tx_bytes, offset_secs)
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch,
            sample_source, inbound_promoted
        ) VALUES
            ($1, 'host', 'eth0', to_timestamp($2 - 33 * 86400),
                10, 20, 1, 1, 'agent_networks', TRUE),
            ($1, 'host', 'eth0', to_timestamp($2 - 60),
                35, 55, 1, 1, 'agent_networks', FALSE)
        "#,
    )
    .bind(client_id)
    .bind(now_unix)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("ANALYZE traffic_counter_samples, traffic_counter_rollups")
        .execute(&db.pool)
        .await
        .unwrap();

    let accounting = db.repo.get_traffic_accounting(client_id).await.unwrap();
    assert_eq!(accounting.cycle_start, None);
    assert_eq!(accounting.cycle_end, None);
    assert_eq!(accounting.rx_bytes, 70);
    assert_eq!(accounting.tx_bytes, 110);
    assert_eq!(accounting.total_bytes, 180);
    assert_eq!(accounting.latest_rx_bytes, 35);
    assert_eq!(accounting.latest_tx_bytes, 55);
    assert_eq!(accounting.counter_epochs_seen, 1);

    let explain_sql =
        format!("EXPLAIN (ANALYZE, FORMAT JSON) {NO_RESET_TRAFFIC_COUNTER_USAGE_SQL}");
    let plan: serde_json::Value = sqlx::query_scalar(&explain_sql)
        .bind(vec![client_id.to_string()])
        .bind(vec!["host".to_string()])
        .bind(vec!["eth0".to_string()])
        .bind(now_unix)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let plan_text = plan.to_string();
    assert!(plan_text.contains("WindowAgg"), "{plan_text}");
    assert!(plan_text.contains("traffic_counter_rollups"), "{plan_text}");

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_retained_traffic_uses_promoted_boundary_only_as_exact_baseline() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "traffic-promoted-boundary-history";
    insert_client(&db.pool, client_id, None).await;
    sqlx::query(
        r#"
        INSERT INTO vps_rule_values (client_id, key, value_raw, value_json)
        VALUES (
            $1,
            'traffic.selectors',
            'eth0',
            '{"selectors":[{"source":"host","interface":"eth0","direction":"total","canonical":"eth0"}]}'::jsonb
        )
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let bucket_start: i64 = sqlx::query_scalar(
        r#"
        SELECT extract(epoch FROM date_bin(
            '1 hour', now() - interval '40 days',
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        ))::bigint
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch,
            sample_source, inbound_promoted
        ) VALUES
            ($1, 'host', 'eth0', to_timestamp($2 - 60),
                60, 120, 0, 0, 'agent_networks', FALSE),
            ($1, 'host', 'eth0', to_timestamp($2 + 3540),
                100, 20, 0, 1, 'agent_networks', TRUE),
            ($1, 'host', 'eth0', to_timestamp($2 + 3600),
                110, 40, 0, 1, 'agent_networks', FALSE)
        "#,
    )
    .bind(client_id)
    .bind(bucket_start)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        ) VALUES (
            $1, 'host', 'eth0', 'live', 3600, to_timestamp($2),
            40, 0, 1, 0, 1, 0, 1, 1,
            to_timestamp($2 + 3540), to_timestamp($2 + 3540)
        )
        "#,
    )
    .bind(client_id)
    .bind(bucket_start)
    .execute(&db.pool)
    .await
    .unwrap();

    let history = db
        .repo
        .list_traffic_history(
            client_id,
            bucket_start as u64,
            (bucket_start + 3660) as u64,
            60,
            false,
        )
        .await
        .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].bucket_secs, 3600);
    assert_eq!(history[0].sample_count, 1);
    assert_eq!(history[0].reset_count, 1);
    assert_eq!(history[0].rx_bytes, Some(40));
    assert_eq!(history[0].tx_bytes, None);
    assert_eq!(history[1].bucket_secs, 60);
    assert_eq!(history[1].sample_count, 1);
    assert_eq!(history[1].reset_count, 0);
    assert_eq!(history[1].rx_bytes, Some(10));
    assert_eq!(history[1].tx_bytes, Some(20));

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_retained_traffic_overlap_probes_preserve_authority() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "traffic-overlap-probe-authority";
    insert_client(&db.pool, client_id, None).await;
    sqlx::query(
        r#"
        INSERT INTO vps_rule_values (client_id, key, value_raw, value_json)
        VALUES (
            $1,
            'traffic.selectors',
            'eth0',
            '{"selectors":[{"source":"host","interface":"eth0","direction":"total","canonical":"eth0"}]}'::jsonb
        )
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let anchor: i64 = sqlx::query_scalar(
        r#"
        SELECT extract(epoch FROM date_bin(
            '3 hours', now() - interval '40 days',
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        ))::bigint
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        ) VALUES
            ($1, 'host', 'eth0', 'live', 10800, to_timestamp($2),
                300, 600, 3, 3, 3, 0, 0, 0,
                to_timestamp($2), to_timestamp($2 + 10740)),
            ($1, 'host', 'eth0', 'live', 3600, to_timestamp($2 + 3600),
                100, 200, 1, 1, 1, 0, 0, 0,
                to_timestamp($2 + 3600), to_timestamp($2 + 7140)),
            ($1, 'host', 'eth0', 'live', 3600, to_timestamp($2 + 7200),
                100, 200, 1, 1, 1, 0, 0, 0,
                to_timestamp($2 + 7200), to_timestamp($2 + 10740))
        "#,
    )
    .bind(client_id)
    .bind(anchor)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch,
            sample_source, inbound_promoted
        ) VALUES
            ($1, 'host', 'eth0', to_timestamp($2 + 4200),
                10, 20, 0, 0, 'agent_networks', FALSE),
            ($1, 'host', 'eth0', to_timestamp($2 + 4260),
                20, 40, 0, 0, 'agent_networks', FALSE)
        "#,
    )
    .bind(client_id)
    .bind(anchor)
    .execute(&db.pool)
    .await
    .unwrap();

    let history = db
        .repo
        .list_traffic_history(
            client_id,
            anchor as u64,
            (anchor + 10_799) as u64,
            3_600,
            false,
        )
        .await
        .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(
        history.iter().map(|point| point.sample_count).sum::<i32>(),
        2
    );
    assert_eq!(
        history
            .iter()
            .filter_map(|point| point.rx_bytes)
            .sum::<i64>(),
        110
    );
    assert_eq!(
        history
            .iter()
            .filter_map(|point| point.tx_bytes)
            .sum::<i64>(),
        220
    );
    assert!(history.iter().all(|point| point.bucket_secs == 3_600));
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_retained_traffic_realistic_exact_tail_stays_bounded() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "traffic-realistic-exact-tail";
    insert_client(&db.pool, client_id, None).await;
    sqlx::query(
        r#"
        INSERT INTO vps_rule_values (client_id, key, value_raw, value_json)
        VALUES (
            $1,
            'traffic.selectors',
            'eth0',
            '{"selectors":[{"source":"host","interface":"eth0","direction":"total","canonical":"eth0"}]}'::jsonb
        )
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        WITH anchor AS (
            SELECT date_trunc('minute', now()) - interval '32 days' AS value
        )
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch,
            sample_source, inbound_promoted
        )
        SELECT
            $1, 'host', 'eth0', anchor.value + sample_number * interval '1 minute',
            sample_number, sample_number * 2, 0, 0,
            'agent_networks', FALSE
        FROM anchor
        CROSS JOIN generate_series(0, 46080) sample_number
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        WITH anchor AS (
            SELECT date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                AS value
        ), buckets(bucket_secs, bucket_start) AS (
            SELECT 3600, bucket_start
            FROM anchor
            CROSS JOIN LATERAL generate_series(
                anchor.value - interval '91 days',
                anchor.value - interval '32 days 1 hour',
                interval '1 hour'
            ) bucket_start
            UNION ALL
            SELECT 10800, bucket_start
            FROM anchor
            CROSS JOIN LATERAL generate_series(
                anchor.value - interval '181 days',
                anchor.value - interval '91 days 3 hours',
                interval '3 hours'
            ) bucket_start
            UNION ALL
            SELECT 21600, bucket_start
            FROM anchor
            CROSS JOIN LATERAL generate_series(
                anchor.value - interval '366 days',
                anchor.value - interval '181 days 6 hours',
                interval '6 hours'
            ) bucket_start
            UNION ALL
            SELECT 86400, bucket_start
            FROM anchor
            CROSS JOIN LATERAL generate_series(
                anchor.value - interval '1095 days',
                anchor.value - interval '367 days',
                interval '1 day'
            ) bucket_start
        )
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        )
        SELECT
            $1, 'host', 'eth0', 'live', bucket_secs, bucket_start,
            1, 2, 1, 1, 1, 0, 0, 0,
            bucket_start,
            bucket_start + make_interval(secs => bucket_secs - 60)
        FROM buckets
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let seeded_counts: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM traffic_counter_samples WHERE client_id = $1),
            (SELECT count(*) FROM traffic_counter_rollups WHERE client_id = $1)
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(seeded_counts, (46_081, 3_605));
    sqlx::query("ANALYZE traffic_counter_samples, traffic_counter_rollups")
        .execute(&db.pool)
        .await
        .unwrap();
    let (start_unix, end_unix): (i64, i64) = sqlx::query_as(
        "SELECT extract(epoch FROM now() - interval '90 days')::bigint, extract(epoch FROM now())::bigint",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let expected_rollups: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*)
        FROM traffic_counter_rollups
        WHERE client_id = $1
          AND bucket_start < to_timestamp($3) + interval '1 second'
          AND bucket_start + make_interval(secs => bucket_secs) > to_timestamp($2)
        "#,
    )
    .bind(client_id)
    .bind(start_unix)
    .bind(end_unix)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let history = tokio::time::timeout(
        Duration::from_secs(10),
        db.repo
            .list_traffic_history(client_id, start_unix as u64, end_unix as u64, 10_800, false),
    )
    .await
    .expect("realistic retained traffic query exceeded ten seconds")
    .unwrap();
    let expected_samples = 46_080_i64 + expected_rollups;
    assert_eq!(
        history
            .iter()
            .map(|point| i64::from(point.sample_count))
            .sum::<i64>(),
        expected_samples
    );
    assert_eq!(
        history
            .iter()
            .filter_map(|point| point.rx_bytes)
            .sum::<i64>(),
        expected_samples
    );
    assert_eq!(
        history
            .iter()
            .filter_map(|point| point.tx_bytes)
            .sum::<i64>(),
        expected_samples * 2
    );
    assert!(history.len() <= 721);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_retained_traffic_keeps_bidirectional_diagnostics() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "traffic-tier-direction-mask";
    insert_client(&db.pool, client_id, None).await;
    let bucket_start: i64 = sqlx::query_scalar(
        r#"
        SELECT extract(epoch FROM date_bin(
            '1 hour', now() - interval '40 days',
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        ))::bigint
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        ) VALUES (
            $1, 'host', 'eth0', 'live', 3600, to_timestamp($2),
            300, 600, 4, 0, 4, 0, 1, 1,
            to_timestamp($2), to_timestamp($2 + 3540)
        )
        "#,
    )
    .bind(client_id)
    .bind(bucket_start)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO vps_rule_values (client_id, key, value_raw, value_json)
        VALUES (
            $1, 'traffic.selectors', 'eth0+rx',
            '{"selectors":[{"source":"host","interface":"eth0","direction":"rx","canonical":"eth0+rx"}]}'::jsonb
        )
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let rx = db
        .repo
        .list_traffic_history(
            client_id,
            bucket_start as u64,
            (bucket_start + 3599) as u64,
            60,
            false,
        )
        .await
        .unwrap();
    assert_eq!(rx.len(), 1);
    assert_eq!(rx[0].sample_count, 4);
    assert_eq!(rx[0].reset_count, 1);
    assert_eq!(rx[0].rx_bytes, Some(300));
    assert_eq!(rx[0].tx_bytes, None);

    sqlx::query(
        r#"
        UPDATE vps_rule_values
        SET value_raw = 'eth0+tx',
            value_json = '{"selectors":[{"source":"host","interface":"eth0","direction":"tx","canonical":"eth0+tx"}]}'::jsonb
        WHERE client_id = $1 AND key = 'traffic.selectors'
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let tx = db
        .repo
        .list_traffic_history(
            client_id,
            bucket_start as u64,
            (bucket_start + 3599) as u64,
            60,
            false,
        )
        .await
        .unwrap();
    assert_eq!(tx.len(), 1);
    assert_eq!(tx[0].sample_count, 4);
    assert_eq!(tx[0].reset_count, 1);
    assert_eq!(tx[0].rx_bytes, Some(300));
    assert_eq!(tx[0].tx_bytes, None);

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_vnstat_rerun_hydrates_only_non_import_boundary_rows() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "traffic-import-bounded-rerun";
    let start_unix = 1_722_470_400_i64;
    insert_client(&db.pool, client_id, None).await;

    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source
        )
        SELECT
            $1,
            'host',
            'eth0',
            to_timestamp(($2::bigint - (generated.sample::bigint + 1) * 60)::double precision),
            generated.sample::bigint,
            generated.sample::bigint * 2,
            0,
            0,
            'vnstat_import:11111111-1111-4111-8111-111111111111'
        FROM generate_series(1, 50000) AS generated(sample)
        "#,
    )
    .bind(client_id)
    .bind(start_unix)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source
        )
        SELECT
            $1,
            'host',
            'eth0',
            to_timestamp(($2::bigint + generated.sample::bigint * 60)::double precision),
            50000 + generated.sample::bigint,
            100000 + generated.sample::bigint,
            0,
            0,
            'vnstat_import:11111111-1111-4111-8111-111111111111'
        FROM generate_series(0, 9) AS generated(sample)
        "#,
    )
    .bind(client_id)
    .bind(start_unix)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source
        )
        VALUES
            ($1, 'host', 'eth0', to_timestamp(($2::bigint - 60)::double precision), 700, 900, 3, 4, 'interface_counters'),
            ($1, 'host', 'eth0', to_timestamp(($2::bigint + 600)::double precision), 20, 30, 5, 6, 'interface_counters')
        "#,
    )
    .bind(client_id)
    .bind(start_unix)
    .execute(&db.pool)
    .await
    .unwrap();

    let imported_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM traffic_counter_samples WHERE client_id = $1 AND sample_source LIKE 'vnstat_import:%'",
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(imported_rows, 50_010);

    let mut tx = db.pool.begin().await.unwrap();
    let boundaries = load_postgres_import_boundary_samples(
        &mut tx,
        client_id,
        &["eth0".to_string()],
        start_unix as u64,
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();

    assert_eq!(boundaries.len(), 2);
    assert!(boundaries
        .iter()
        .all(|sample| sample.sample_source == "interface_counters"));
    assert_eq!(boundaries[0].observed_unix, start_unix - 60);
    assert_eq!(boundaries[0].rx_bytes, 700);
    assert_eq!(boundaries[1].observed_unix, start_unix + 600);
    assert_eq!(boundaries[1].tx_bytes, 30);

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_vnstat_reimport_replaces_only_imported_traffic_ledger() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "traffic-import-tier-replace";
    let now_unix = Utc::now().timestamp().div_euclid(60) * 60;
    let start_unix = now_unix - 40 * 86_400;
    let live_unix = start_unix + 600;
    insert_client(&db.pool, client_id, None).await;
    sqlx::query(
        r#"
        INSERT INTO vps_rule_values (client_id, key, value_raw, value_json)
        VALUES
            ($1, 'traffic.reset_day', '-1', '{"day":-1}'::jsonb),
            (
                $1,
                'traffic.selectors',
                'eth0',
                '{"selectors":[{"source":"host","interface":"eth0","direction":"total","canonical":"eth0"}]}'::jsonb
            ),
            (
                $1,
                'traffic.quota.total',
                '1TB',
                '{"bytes":1000000000000,"display":"1 TB"}'::jsonb
            )
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source
        ) VALUES (
            $1, 'host', 'eth0', to_timestamp($2::double precision),
            10, 20, 0, 0, 'interface_counters'
        )
        "#,
    )
    .bind(client_id)
    .bind((now_unix - 60) as f64)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO telemetry_network_rates (
            client_id, interface, bucket_start, bucket_secs,
            sample_count, rx_bytes_sum, tx_bytes_sum,
            rx_bytes_avg, tx_bytes_avg,
            rx_bytes_last, tx_bytes_last, rx_counter_epoch, tx_counter_epoch,
            latest_observed_at
        ) VALUES (
            $1, 'eth0', to_timestamp($2::double precision), 60,
            1, 10, 20, 10, 20, 10, 20, 7, 9,
            to_timestamp($2::double precision)
        )
        "#,
    )
    .bind(client_id)
    .bind(start_unix as f64)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        WITH bucket AS (
            SELECT date_bin(
                '1 hour', to_timestamp($2::double precision),
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
            ) AS bucket_start
        )
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        )
        SELECT
            $1, 'host', 'eth0', 'live', 3600, bucket_start,
            5, 7, 1, 1, 1, 0, 0, 0,
            to_timestamp($2::double precision),
            to_timestamp($2::double precision)
        FROM bucket
        "#,
    )
    .bind(client_id)
    .bind(live_unix as f64)
    .execute(&db.pool)
    .await
    .unwrap();

    let result = NetworkTrafficImportResult {
        r#type: "network_traffic_import_vnstat".to_string(),
        status: "collected".to_string(),
        requested_start_unix: start_unix as u64,
        collected_until_unix: live_unix as u64,
        interfaces: vec!["eth0".to_string()],
        sources: vec![NetworkTrafficImportSource {
            interface: "eth0".to_string(),
            database_created_unix: Some(start_unix as u64),
            retained_start_unix: start_unix as u64,
            source_updated_unix: Some(live_unix as u64),
        }],
        batch_count: 1,
        bucket_count: 1,
        message: String::new(),
    };
    let import = |rx_bytes, tx_bytes| NetworkTrafficImportBucket {
        interface: "eth0".to_string(),
        start_unix: start_unix as u64,
        duration_secs: 600,
        rx_bytes,
        tx_bytes,
    };
    db.repo
        .import_vnstat_traffic_history(
            Uuid::new_v4(),
            client_id,
            &["eth0".to_string()],
            start_unix as u64,
            &result,
            &[import(100, 50)],
            now_unix as u64,
        )
        .await
        .unwrap();

    let contributions: Vec<(String, i64, i64)> = sqlx::query_as(
        r#"
        SELECT origin_kind, sum(rx_bytes)::bigint, sum(tx_bytes)::bigint
        FROM traffic_counter_rollups
        WHERE client_id = $1
        GROUP BY origin_kind
        ORDER BY origin_kind
        "#,
    )
    .bind(client_id)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        contributions,
        vec![
            ("live".to_string(), 5, 7),
            ("vnstat_import".to_string(), 100, 50),
        ]
    );
    let accounting = db.repo.get_traffic_accounting(client_id).await.unwrap();
    assert_eq!((accounting.rx_bytes, accounting.tx_bytes), (105, 57));

    db.repo
        .import_vnstat_traffic_history(
            Uuid::new_v4(),
            client_id,
            &["eth0".to_string()],
            start_unix as u64,
            &result,
            &[import(120, 60)],
            now_unix as u64,
        )
        .await
        .unwrap();
    let contributions: Vec<(String, i64, i64)> = sqlx::query_as(
        r#"
        SELECT origin_kind, sum(rx_bytes)::bigint, sum(tx_bytes)::bigint
        FROM traffic_counter_rollups
        WHERE client_id = $1
        GROUP BY origin_kind
        ORDER BY origin_kind
        "#,
    )
    .bind(client_id)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        contributions,
        vec![
            ("live".to_string(), 5, 7),
            ("vnstat_import".to_string(), 120, 60),
        ]
    );
    let accounting = db.repo.get_traffic_accounting(client_id).await.unwrap();
    assert_eq!((accounting.rx_bytes, accounting.tx_bytes), (125, 67));
    let network_epochs: (i64, i64) = sqlx::query_as(
        r#"
        SELECT rx_counter_epoch, tx_counter_epoch
        FROM telemetry_network_rates
        WHERE client_id = $1 AND interface = 'eth0'
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(network_epochs, (7, 9));
    let imported_exact: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*)
        FROM traffic_counter_samples
        WHERE client_id = $1 AND sample_source LIKE 'vnstat_import:%'
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(imported_exact, 1);
    let import_predecessor_promoted: bool = sqlx::query_scalar(
        r#"
        SELECT inbound_promoted
        FROM traffic_counter_samples
        WHERE client_id = $1 AND sample_source LIKE 'vnstat_import:%'
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(import_predecessor_promoted);

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_telemetry_sample_prune_shares_limit_with_ping_facts_atomically() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "telemetry-raw-shared-prune-limit";
    insert_client(&db.pool, client_id, None).await;
    let target_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO ping_targets (id, name, host, probe_kind, selector_expression)
        VALUES ($1, 'Shared prune limit Ping', '192.0.2.200', 'icmp', '*')
        "#,
    )
    .bind(target_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let series_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO telemetry_ping_series (client_id, target_id, generation)
        VALUES ($1, $2, 1)
        RETURNING id
        "#,
    )
    .bind(client_id)
    .bind(target_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let cutoff_unix: i64 =
        sqlx::query_scalar("SELECT floor(extract(epoch FROM now()) / 60)::bigint * 60 - 3600")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let oldest_unix = cutoff_unix - 120;
    let newer_fact_unix = cutoff_unix - 90;
    let newer_sample_unix = cutoff_unix - 60;
    let fresh_unix = cutoff_unix + 60;
    let oldest_sample_id = Uuid::new_v4();
    let newer_sample_id = Uuid::new_v4();
    let fresh_sample_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO telemetry_samples (
            id, client_id, observed_at,
            cpu_utilization_ratio, cpu_cores, cpu_load_1, cpu_load_5, cpu_load_15,
            memory_total_bytes, memory_available_bytes,
            swap_total_bytes, swap_available_bytes,
            disk_total_bytes, disk_available_bytes,
            network_rx_bytes, network_tx_bytes,
            tcp_sockets, udp_sockets, payload
        ) VALUES
            ($1, $4, to_timestamp($5), NULL, 1, 0, 0, 0,
                1, 1, NULL, NULL, 1, 1, 0, 0, 0, 0, '{}'::jsonb),
            ($2, $4, to_timestamp($6), NULL, 1, 0, 0, 0,
                1, 1, NULL, NULL, 1, 1, 0, 0, 0, 0, '{}'::jsonb),
            ($3, $4, to_timestamp($7), NULL, 1, 0, 0, 0,
                1, 1, NULL, NULL, 1, 1, 0, 0, 0, 0, '{}'::jsonb)
        "#,
    )
    .bind(oldest_sample_id)
    .bind(newer_sample_id)
    .bind(fresh_sample_id)
    .bind(client_id)
    .bind(oldest_unix)
    .bind(newer_sample_unix)
    .bind(fresh_unix)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO telemetry_ping_facts (
            series_id, observed_at, evidence_id, source_checked_unix, checked_unix,
            status, latency_avg_ms, loss_ratio, reason
        ) VALUES
            ($1, to_timestamp($2), $3, $2, $2, 'ok', 10, 0, NULL),
            -- Distinct source checks may legitimately share a rebased chart
            -- second; the shared prune limit must still delete one fact only.
            ($1, to_timestamp($4), $5, $4, $2, 'ok', 11, 0, NULL),
            ($1, to_timestamp($6), $7, $6, $6, 'ok', 12, 0, NULL)
        "#,
    )
    .bind(series_id)
    .bind(oldest_unix)
    .bind(oldest_sample_id)
    .bind(newer_fact_unix)
    .bind(newer_sample_id)
    .bind(fresh_unix)
    .bind(fresh_sample_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO telemetry_ping_rollups (
            series_id, bucket_start, bucket_secs, sample_count, success_count,
            latency_sum_ms, latency_avg_ms, latency_min_ms, latency_max_ms,
            loss_ratio_avg, loss_ratio_sum, loss_ratio_max,
            latest_status, latest_reason, latest_checked_at
        ) VALUES (
            $1, to_timestamp($2), 60, 1, 1,
            10, 10, 10, 10, 0, 0, 0, 'ok', NULL, to_timestamp($2)
        )
        "#,
    )
    .bind(series_id)
    .bind(oldest_unix)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO telemetry_ping_current (
            series_id, latest_status, latency_avg_ms, rolling_loss_ratio,
            latest_reason, latest_checked_at
        ) VALUES ($1, 'ok', 12, 0, NULL, to_timestamp($2))
        "#,
    )
    .bind(series_id)
    .bind(fresh_unix)
    .execute(&db.pool)
    .await
    .unwrap();

    let plan = HistoryRetentionPrunePlan {
        domain: HistoryDomain::TelemetrySamples,
        prune_limit: 2,
        enabled: true,
    };
    let preview = db
        .repo
        .prune_history_domain(&plan, cutoff_unix as u64, true)
        .await
        .unwrap();
    assert_eq!((preview.matched_rows, preview.pruned_rows), (2, 0));
    let untouched_after_preview: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM telemetry_samples
             WHERE client_id = $1 AND observed_at < to_timestamp($2))::bigint,
            (SELECT count(*) FROM telemetry_ping_facts
             WHERE series_id = $3 AND observed_at < to_timestamp($2))::bigint
        "#,
    )
    .bind(client_id)
    .bind(cutoff_unix)
    .bind(series_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(untouched_after_preview, (2, 2));

    sqlx::query(
        r#"
        CREATE FUNCTION reject_ping_fact_prune_for_atomicity_test()
        RETURNS trigger
        LANGUAGE plpgsql
        AS $function$
        BEGIN
            RAISE EXCEPTION 'intentional Ping fact prune failure';
        END;
        $function$
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        CREATE TRIGGER reject_ping_fact_prune_for_atomicity_test
        BEFORE DELETE ON telemetry_ping_facts
        FOR EACH ROW
        EXECUTE FUNCTION reject_ping_fact_prune_for_atomicity_test()
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let error = db
        .repo
        .prune_history_domain(&plan, cutoff_unix as u64, false)
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("intentional Ping fact prune failure"));
    let untouched_after_failure: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM telemetry_samples
             WHERE client_id = $1 AND observed_at < to_timestamp($2))::bigint,
            (SELECT count(*) FROM telemetry_ping_facts
             WHERE series_id = $3 AND observed_at < to_timestamp($2))::bigint
        "#,
    )
    .bind(client_id)
    .bind(cutoff_unix)
    .bind(series_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(untouched_after_failure, (2, 2));
    sqlx::query("DROP TRIGGER reject_ping_fact_prune_for_atomicity_test ON telemetry_ping_facts")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION reject_ping_fact_prune_for_atomicity_test()")
        .execute(&db.pool)
        .await
        .unwrap();

    let first_apply = db
        .repo
        .prune_history_domain(&plan, cutoff_unix as u64, false)
        .await
        .unwrap();
    assert_eq!((first_apply.matched_rows, first_apply.pruned_rows), (2, 2));
    let deterministic_first_batch: (bool, bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT
            EXISTS (SELECT 1 FROM telemetry_samples WHERE id = $1),
            EXISTS (SELECT 1 FROM telemetry_ping_facts
                    WHERE series_id = $2 AND source_checked_unix = $3),
            EXISTS (SELECT 1 FROM telemetry_samples WHERE id = $4),
            EXISTS (SELECT 1 FROM telemetry_ping_facts
                    WHERE series_id = $2 AND source_checked_unix = $5)
        "#,
    )
    .bind(oldest_sample_id)
    .bind(series_id)
    .bind(oldest_unix)
    .bind(newer_sample_id)
    .bind(newer_fact_unix)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(deterministic_first_batch, (false, false, true, true));

    let second_preview = db
        .repo
        .prune_history_domain(&plan, cutoff_unix as u64, true)
        .await
        .unwrap();
    assert_eq!(
        (second_preview.matched_rows, second_preview.pruned_rows),
        (2, 0)
    );
    let second_apply = db
        .repo
        .prune_history_domain(&plan, cutoff_unix as u64, false)
        .await
        .unwrap();
    assert_eq!(
        (second_apply.matched_rows, second_apply.pruned_rows),
        (2, 2)
    );
    let drained = db
        .repo
        .prune_history_domain(&plan, cutoff_unix as u64, true)
        .await
        .unwrap();
    assert_eq!((drained.matched_rows, drained.pruned_rows), (0, 0));
    let retained_state: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM telemetry_samples
             WHERE client_id = $1 AND observed_at >= to_timestamp($2))::bigint,
            (SELECT count(*) FROM telemetry_ping_facts
             WHERE series_id = $3 AND observed_at >= to_timestamp($2))::bigint,
            (SELECT count(*) FROM telemetry_ping_current
             WHERE series_id = $3)::bigint,
            (SELECT count(*) FROM telemetry_ping_rollups
             WHERE series_id = $3)::bigint
        "#,
    )
    .bind(client_id)
    .bind(cutoff_unix)
    .bind(series_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(retained_state, (1, 1, 1, 1));

    db.cleanup().await;
}

async fn exact_v044_migration_test_db() -> Option<(PgReliabilityTestDb, std::path::PathBuf)> {
    let base_url = match std::env::var("VPSMAN_TEST_POSTGRES_URL") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skipping Postgres migration test: VPSMAN_TEST_POSTGRES_URL is unset");
            return None;
        }
    };
    let baseline_dir = std::env::temp_dir().join(format!(
        "vpsman-v044-migrations-{}",
        Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&baseline_dir).unwrap();
    let migrations_dir = workspace_migrations_dir();
    for (name, expected_sha256) in [
        (
            "0001_identity_access.sql",
            "884f950940275597749a575ab30780369ea2a5adb17889e1c29c5dd1b2fd1167",
        ),
        (
            "0002_jobs_schedules_commands.sql",
            "031f27cfbdce3b593dcfc144b122c7d2bff4d56e7a4c074fea373da9e0f04883",
        ),
        (
            "0003_telemetry_alerts_history.sql",
            "f1408e33815cb10b98b1061d1d0275874357ed7803218252796846572e7c7e3b",
        ),
        (
            "0004_backups_restores.sql",
            "120ebf2284a7035f7ad51f9989e79e871c93f9b5690a0681abcc0248165833e9",
        ),
        (
            "0005_network_tunnels.sql",
            "dc77d215b22080f9036b43afcc8a15f0bc6295bfab0a0884dd116271b0f35131",
        ),
        (
            "0006_agent_updates.sql",
            "150ec74e23db6fe98c6ba6de723c369ea219c8c12ab2295dda5e4ceea78a2158",
        ),
        (
            "0007_configuration_presets_file_transfer.sql",
            "6ff29337a5408b8a9f34536a53be4fa189c0d326251abb53bdbd4b489f99fe8c",
        ),
        (
            "0008_system_metrics.sql",
            "83fb85dd37b217e2f94995074c851fddbf852c3327b062ecc534d1058763e8a8",
        ),
        (
            "0009_fleet_tag_settings.sql",
            "b0c0deaa0ad9bcf98dc0b6e2af1e8295155568b01c5d7cb2d5491cdd345caa9d",
        ),
    ] {
        let bytes = fs::read(migrations_dir.join(name)).unwrap();
        assert_eq!(payload_hash(&bytes), expected_sha256, "migration: {name}");
        fs::copy(migrations_dir.join(name), baseline_dir.join(name)).unwrap();
    }
    let db = PgReliabilityTestDb::new_with_migrations(&base_url, &baseline_dir)
        .await
        .expect("failed to create exact v0.4.4 baseline database");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM _sqlx_migrations WHERE success")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        9
    );
    Some((db, baseline_dir))
}

async fn insert_policy_migration_state(
    pool: &PgPool,
    policy_rule_id: Uuid,
    client_id: &str,
    rule_version: i32,
) {
    sqlx::query(
        r#"
        INSERT INTO policy_rule_states (
            policy_rule_id, client_id, rule_version, condition_true,
            previous_condition_true, window_satisfied, first_true_at,
            last_true_at, last_evaluated_at, incomplete, last_actual_value,
            last_threshold_value, last_fired_at, trigger_generation
        ) VALUES (
            $1, $2, $3, TRUE, FALSE, TRUE, now(), now(), now(), FALSE,
            1.0, 0.75, now(), 1
        )
        "#,
    )
    .bind(policy_rule_id)
    .bind(client_id)
    .bind(rule_version)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_policy_migration_alert(
    pool: &PgPool,
    alert_id: Uuid,
    policy_group_id: Uuid,
    policy_rule_id: Uuid,
    client_id: &str,
) {
    sqlx::query(
        r#"
        INSERT INTO policy_alerts (
            id, policy_group_id, policy_rule_id, client_id,
            trigger_generation, severity, category, title, detail,
            actual_value, threshold_value, payload, observed_at
        ) VALUES (
            $1, $2, $3, $4, 1, 'warning', 'resource',
            'Retained migration alert', 'historical policy evidence',
            1.0, 0.75, '{}'::jsonb, now()
        )
        "#,
    )
    .bind(alert_id)
    .bind(policy_group_id)
    .bind(policy_rule_id)
    .bind(client_id)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn postgres_fresh_schema_has_operational_alert_lifecycle_invariants() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };

    assert_eq!(
        sqlx::query_as::<_, (bool, bool)>(
            r#"
            SELECT
                to_regclass('public.operational_alert_episodes') IS NULL,
                to_regclass('public.alert_episodes') IS NOT NULL
            "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (true, true)
    );
    let meta: (bool, Option<String>, bool) = sqlx::query_as(
        r#"
        SELECT backfill_completed, completed_at::text,
               event_source_cutoff_at IS NOT NULL
        FROM operational_alert_lifecycle_meta
        WHERE singleton
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(meta, (false, None, true));

    let columns = sqlx::query_scalar::<_, String>(
        r#"
        SELECT table_name || '.' || column_name
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND (table_name, column_name) IN (
            ('clients', 'operational_alert_status_at'),
            ('clients', 'operational_alert_legacy_status'),
            ('clients', 'operational_alert_tunnel_boundary_at'),
            ('telemetry_tunnels', 'telemetry_topology_identity_hash'),
            ('telemetry_tunnels', 'telemetry_runtime_evidence_identity_hash'),
            ('telemetry_tunnels', 'operational_alert_legacy_identity'),
            ('tunnel_plans', 'operational_alert_legacy_runtime_identity'),
            ('tunnel_plans', 'operational_alert_runtime_boundary_at'),
            ('jobs', 'alert_terminal_at'),
            ('job_targets', 'capability_alert_at'),
            ('backup_requests', 'terminal_at')
          )
        ORDER BY table_name, column_name
        "#,
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(columns.len(), 11, "all cutover evidence columns must exist");

    for trigger in [
        "clients_operational_alert_boundaries_insert_trigger",
        "clients_operational_alert_boundaries_update_trigger",
        "gateway_sessions_operational_alert_boundary_trigger",
        "tunnel_plans_legacy_runtime_identity_trigger",
    ] {
        assert!(
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM pg_trigger WHERE tgname = $1 AND NOT tgisinternal)",
            )
            .bind(trigger)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
            "missing lifecycle provenance trigger {trigger}"
        );
    }

    for index in [
        "alert_episodes_identity_idx",
        "alert_episodes_one_current_idx",
        "alert_episodes_trigger_evidence_idx",
        "alert_episodes_last_evidence_idx",
        "alert_policy_evaluation_states_due_idx",
        "alert_lifecycle_events_consumer_idx",
        "jobs_alert_terminal_at_idx",
        "job_targets_capability_alert_at_idx",
        "backup_requests_failed_terminal_idx",
    ] {
        assert!(
            sqlx::query_scalar::<_, bool>("SELECT to_regclass($1) IS NOT NULL")
                .bind(index)
                .fetch_one(&db.pool)
                .await
                .unwrap(),
            "missing lifecycle index {index}"
        );
    }
    assert_eq!(
        sqlx::query_as::<_, (bool, bool)>(
            r#"
            SELECT
                to_regclass('public.operational_alert_episodes_one_current_idx') IS NULL,
                to_regclass('public.operational_alert_episodes_event_source_once_idx') IS NULL
            "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (true, true)
    );
    for constraint in [
        "alert_episodes_policy_provenance_check",
        "alert_episodes_rule_record_kind_check",
        "alert_episodes_lifecycle_check",
    ] {
        assert!(
            sqlx::query_scalar::<_, bool>(
                r#"
                SELECT EXISTS (
                    SELECT 1
                    FROM pg_constraint
                    WHERE conrelid = 'alert_episodes'::regclass
                      AND conname = $1
                )
                "#,
            )
            .bind(constraint)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
            "missing unified lifecycle constraint {constraint}"
        );
    }

    let operator = postgres_network_operator(&db.repo).await;
    let invalid = sqlx::query(
        r#"
        INSERT INTO alert_episodes (
            id, public_id, producer_kind, natural_key, record_kind,
            trigger_generation, trigger_severity, trigger_category,
            severity, category, target_kind, target_id, title, detail,
            source_status, evidence, lifecycle_state, triggered_at,
            last_confirmed_at, resolved_at, resolution_reason,
            resolution_note, resolution_actor_id,
            policy_group_id, policy_rule_id, policy_rule_version,
            policy_rule_kind, policy_group_name, policy_rule_name,
            policy_rule_system_seed_key
        ) VALUES (
            $1, 'invalid-condition-resolution', 'agent.access', 'invalid', 'condition',
            1, 'critical', 'agent_status', 'critical', 'agent_status',
            'agent', 'invalid', 'invalid', 'invalid', 'offline', '{}'::jsonb,
            'resolved', now(), now(), now(), 'operator_resolved', 'invalid', $2,
            'c1000000-0000-4000-8000-000000000001',
            'd1000000-0000-4000-8000-000000000004', 1,
            'state', 'System operational evidence policies',
            'Agent access revoked', 'agent.access_revoked'
        )
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(operator.operator.id)
    .execute(&db.pool)
    .await
    .unwrap_err();
    assert_eq!(
        invalid
            .as_database_error()
            .and_then(|database_error| database_error.constraint()),
        Some("alert_episodes_lifecycle_check"),
        "condition episodes cannot be operator-resolved"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_gateway_session_boundary_advances_only_for_real_session_transitions() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "gateway-alert-boundary";
    insert_client(&db.pool, client_id, None).await;
    sqlx::query(
        "UPDATE clients SET operational_alert_tunnel_boundary_at = '2020-01-01T00:00:00Z' WHERE id = $1",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let session_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO gateway_sessions (id, gateway_id, client_id, status)
        VALUES ($1, 'gateway-a', $2, 'active')
        "#,
    )
    .bind(session_id)
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let first_boundary: String = sqlx::query_scalar(
        "SELECT operational_alert_tunnel_boundary_at::text FROM clients WHERE id = $1",
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_ne!(first_boundary, "2020-01-01 00:00:00+00");

    sqlx::query("UPDATE gateway_sessions SET status = 'active' WHERE id = $1")
        .bind(session_id)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT operational_alert_tunnel_boundary_at::text FROM clients WHERE id = $1",
        )
        .bind(client_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        first_boundary,
        "an idempotent replay of the same active session must preserve the boundary"
    );

    sqlx::query("UPDATE gateway_sessions SET status = 'ended' WHERE id = $1")
        .bind(session_id)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE gateway_sessions SET status = 'active' WHERE id = $1")
        .bind(session_id)
        .execute(&db.pool)
        .await
        .unwrap();
    let reactivated_boundary: String = sqlx::query_scalar(
        "SELECT operational_alert_tunnel_boundary_at::text FROM clients WHERE id = $1",
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_ne!(reactivated_boundary, first_boundary);

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_exact_v044_operational_lifecycle_backfill_is_quiet_and_idempotent() {
    let Some((db, baseline_dir)) = exact_v044_migration_test_db().await else {
        return;
    };
    let client_id = "legacy-operational-alert";
    let public_key = hex::decode(payload_hash(client_id.as_bytes())).unwrap();
    sqlx::query(
        r#"
        INSERT INTO clients (id, display_name, public_key, status, capabilities)
        VALUES ($1, 'Legacy operational alert', $2, 'never', '{}'::jsonb)
        "#,
    )
    .bind(client_id)
    .bind(public_key)
    .execute(&db.pool)
    .await
    .unwrap();
    for (client_id, key_byte) in [("legacy-tunnel-a", 151_u8), ("legacy-tunnel-b", 152_u8)] {
        sqlx::query(
            r#"
            INSERT INTO clients (
                id, display_name, public_key, status, created_at, last_seen_at
            ) VALUES (
                $1, $1, $2, 'online', '2020-01-01T00:00:00Z',
                '2020-01-01T00:00:00Z'
            )
            "#,
        )
        .bind(client_id)
        .bind(vec![key_byte; 32])
        .execute(&db.pool)
        .await
        .unwrap();
    }
    sqlx::query(
        r#"
        INSERT INTO client_status_history (
            id, client_id, from_status, to_status, reason, created_at
        ) VALUES (
            $1, 'legacy-tunnel-a', 'online', 'online',
            'legacy session confirmation', '2020-01-04T00:00:00Z'
        )
        "#,
    )
    .bind(Uuid::new_v4())
    .execute(&db.pool)
    .await
    .unwrap();
    let legacy_job_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, command_type, privileged, status, target_count, payload_hash,
            request_fingerprint, max_timeout_secs, created_at, completed_at
        ) VALUES (
            $1, 'shell', false, 'failed', 1, repeat('a', 64),
            $2, 30, '2020-01-02T00:00:00Z', '2020-01-02T00:01:00Z'
        )
        "#,
    )
    .bind(legacy_job_id)
    .bind(format!("legacy-job-{legacy_job_id}"))
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, command_type, privileged, status, target_count, payload_hash,
            request_fingerprint, max_timeout_secs, created_at, completed_at
        )
        SELECT
            md5('legacy-operational-job-' || value::text)::uuid,
            'shell', false, 'failed', 1, repeat('d', 64),
            'legacy-operational-job-' || value::text, 30,
            '2019-01-01T00:00:00Z'::timestamptz + value * interval '1 second',
            '2019-01-01T00:00:00Z'::timestamptz + value * interval '1 second'
        FROM generate_series(1, 201) AS generated(value)
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let legacy_capability_job_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, command_type, privileged, status, target_count, payload_hash,
            request_fingerprint, max_timeout_secs, created_at, completed_at
        ) VALUES (
            $1, 'agent_update', true, 'skipped', 1, repeat('b', 64),
            $2, 30, '2020-01-03T00:00:00Z', '2020-01-03T00:01:00Z'
        )
        "#,
    )
    .bind(legacy_capability_job_id)
    .bind(format!("legacy-capability-{legacy_capability_job_id}"))
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO job_targets (
            job_id, client_id, status, message, started_at, completed_at,
            capability_degraded_reason, capability_degraded_hint
        ) VALUES (
            $1, 'legacy-tunnel-a', 'skipped', 'legacy capability skip',
            '2020-01-03T00:00:30Z', '2020-01-03T00:01:00Z',
            'target_agent_lacks_agent_update_capability',
            'Upgrade the target agent before retrying.'
        )
        "#,
    )
    .bind(legacy_capability_job_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let legacy_backup_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO backup_requests (
            id, client_id, paths, include_config, status, payload_hash,
            command_scope, created_at
        ) VALUES (
            $1, 'legacy-tunnel-a', ARRAY['/srv/legacy'], true,
            'execution_failed', repeat('c', 64), 'client:legacy-tunnel-a',
            '2020-01-04T00:00:00Z'
        )
        "#,
    )
    .bind(legacy_backup_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let legacy_tunnel_id = Uuid::new_v4();
    let mut legacy_tunnel_input = postgres_alert_test_tunnel_input();
    legacy_tunnel_input.name = "legacy-alert-tunnel".to_string();
    legacy_tunnel_input.left_client_id = "legacy-tunnel-a".to_string();
    legacy_tunnel_input.right_client_id = "legacy-tunnel-b".to_string();
    let legacy_tunnel_plan = plan_tunnel(&legacy_tunnel_input).unwrap();
    sqlx::query(
        r#"
        INSERT INTO tunnel_plans (
            id, name, kind, enabled, left_client_id, right_client_id,
            input, plan, ospf_status, left_ospf_status, right_ospf_status,
            created_at, updated_at
        ) VALUES (
            $1, $2, 'gre', true, $3, $4, $5, $6,
            'disabled', 'disabled', 'disabled',
            '2020-01-01T00:00:00Z', '2020-01-01T00:00:00Z'
        )
        "#,
    )
    .bind(legacy_tunnel_id)
    .bind(&legacy_tunnel_input.name)
    .bind(&legacy_tunnel_input.left_client_id)
    .bind(&legacy_tunnel_input.right_client_id)
    .bind(SqlJson(&legacy_tunnel_input))
    .bind(SqlJson(&legacy_tunnel_plan))
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO telemetry_tunnels (
            client_id, observed_at, interface, kind, ownership_mode,
            mutation_policy, source, telemetry_plan_id, telemetry_plan_name,
            telemetry_plan_runtime_manager, telemetry_endpoint_side,
            telemetry_peer_client_id, traffic_status, traffic_reason,
            adapter_health, updated_at
        ) VALUES (
            'legacy-tunnel-a', '2020-01-05T00:00:00Z', $1, 'gre',
            'custom_adapter', 'managed_desired', 'telemetry', $2, $3,
            'custom_adapter', 'left', 'legacy-tunnel-b', 'degraded',
            'legacy counters unavailable',
            '{"status":"failed","configured":true,"success":false,"reason":"legacy adapter failure"}'::jsonb,
            '2020-01-05T00:00:00Z'
        )
        "#,
    )
    .bind(&legacy_tunnel_input.interface_name)
    .bind(legacy_tunnel_id.to_string())
    .bind(&legacy_tunnel_input.name)
    .execute(&db.pool)
    .await
    .unwrap();
    let legacy_unattributed_tunnel_id = Uuid::new_v4();
    let mut legacy_unattributed_input = postgres_alert_test_tunnel_input();
    legacy_unattributed_input.name = "legacy-unattributed-alert-tunnel".to_string();
    legacy_unattributed_input.interface_name = "gre43".to_string();
    legacy_unattributed_input.left_client_id = "legacy-tunnel-a".to_string();
    legacy_unattributed_input.right_client_id = "legacy-tunnel-b".to_string();
    let legacy_unattributed_plan = plan_tunnel(&legacy_unattributed_input).unwrap();
    let legacy_unattributed_healthy_tunnel_id = Uuid::new_v4();
    let mut legacy_unattributed_healthy_input = postgres_alert_test_tunnel_input();
    legacy_unattributed_healthy_input.name = "legacy-unattributed-healthy-alert-tunnel".to_string();
    legacy_unattributed_healthy_input.interface_name = "gre44".to_string();
    legacy_unattributed_healthy_input.left_client_id = "legacy-tunnel-a".to_string();
    legacy_unattributed_healthy_input.right_client_id = "legacy-tunnel-b".to_string();
    let legacy_unattributed_healthy_plan = plan_tunnel(&legacy_unattributed_healthy_input).unwrap();
    let legacy_pre_boundary_tunnel_id = Uuid::new_v4();
    let mut legacy_pre_boundary_input = postgres_alert_test_tunnel_input();
    legacy_pre_boundary_input.name = "legacy-pre-boundary-alert-tunnel".to_string();
    legacy_pre_boundary_input.interface_name = "gre46".to_string();
    legacy_pre_boundary_input.left_client_id = "legacy-tunnel-a".to_string();
    legacy_pre_boundary_input.right_client_id = "legacy-tunnel-b".to_string();
    let legacy_pre_boundary_plan = plan_tunnel(&legacy_pre_boundary_input).unwrap();
    let unmarked_unattributed_tunnel_id = Uuid::new_v4();
    let mut unmarked_unattributed_input = postgres_alert_test_tunnel_input();
    unmarked_unattributed_input.name = "unmarked-unattributed-alert-tunnel".to_string();
    unmarked_unattributed_input.interface_name = "gre45".to_string();
    unmarked_unattributed_input.left_client_id = "legacy-tunnel-a".to_string();
    unmarked_unattributed_input.right_client_id = "legacy-tunnel-b".to_string();
    let unmarked_unattributed_plan = plan_tunnel(&unmarked_unattributed_input).unwrap();
    for (plan_id, input, plan, updated_at) in [
        (
            legacy_unattributed_tunnel_id,
            &legacy_unattributed_input,
            &legacy_unattributed_plan,
            "2020-01-06T00:00:00Z",
        ),
        (
            legacy_unattributed_healthy_tunnel_id,
            &legacy_unattributed_healthy_input,
            &legacy_unattributed_healthy_plan,
            "2020-01-06T00:00:00Z",
        ),
        (
            unmarked_unattributed_tunnel_id,
            &unmarked_unattributed_input,
            &unmarked_unattributed_plan,
            "2020-01-01T00:00:00Z",
        ),
        (
            legacy_pre_boundary_tunnel_id,
            &legacy_pre_boundary_input,
            &legacy_pre_boundary_plan,
            "2020-01-01T00:00:00Z",
        ),
    ] {
        sqlx::query(
            r#"
            INSERT INTO tunnel_plans (
                id, name, kind, enabled, left_client_id, right_client_id,
                input, plan, ospf_status, left_ospf_status, right_ospf_status,
                created_at, updated_at
            ) VALUES (
                $1, $2, 'gre', true, $3, $4, $5, $6,
                'disabled', 'disabled', 'disabled',
                '2020-01-01T00:00:00Z', $7::timestamptz
            )
            "#,
        )
        .bind(plan_id)
        .bind(&input.name)
        .bind(&input.left_client_id)
        .bind(&input.right_client_id)
        .bind(SqlJson(input))
        .bind(SqlJson(plan))
        .bind(updated_at)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    for (input, plan_id, traffic_status, traffic_reason, adapter_health, observed_at) in [
        (
            &legacy_unattributed_input,
            legacy_unattributed_tunnel_id,
            "degraded",
            "retained legacy counters unavailable",
            json!({
                "status": "failed",
                "configured": true,
                "success": false,
                "reason": "retained legacy adapter failure",
            }),
            "2020-01-05T00:00:00Z",
        ),
        (
            &legacy_unattributed_healthy_input,
            legacy_unattributed_healthy_tunnel_id,
            "ok",
            "",
            json!({
                "status": "healthy",
                "configured": true,
                "success": true,
            }),
            "2020-01-05T00:00:00Z",
        ),
        (
            &legacy_pre_boundary_input,
            legacy_pre_boundary_tunnel_id,
            "degraded",
            "pre-boundary legacy counters unavailable",
            json!({
                "status": "failed",
                "configured": true,
                "success": false,
                "reason": "pre-boundary legacy adapter failure",
            }),
            "2020-01-03T00:00:00Z",
        ),
    ] {
        sqlx::query(
            r#"
            INSERT INTO telemetry_tunnels (
                client_id, observed_at, interface, kind, ownership_mode,
                mutation_policy, source, telemetry_plan_id, telemetry_plan_name,
                telemetry_plan_runtime_manager, telemetry_endpoint_side,
                telemetry_peer_client_id, traffic_status, traffic_reason,
                adapter_health, updated_at
            ) VALUES (
                'legacy-tunnel-a', $7::timestamptz, $1, 'gre',
                'custom_adapter', 'managed_desired', 'telemetry', $2, $3,
                'custom_adapter', 'left', 'legacy-tunnel-b', $4, NULLIF($5, ''),
                $6, $7::timestamptz
            )
            "#,
        )
        .bind(&input.interface_name)
        .bind(plan_id.to_string())
        .bind(&input.name)
        .bind(traffic_status)
        .bind(traffic_reason)
        .bind(SqlJson(adapter_health))
        .bind(observed_at)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    let legacy_fingerprint = json!({
        "severity": "warning",
        "category": "agent_status",
        "target_kind": "agent",
        "target_id": client_id,
        "title": "Agent is not online",
        "status": "never",
    });
    let legacy_hash = payload_hash(legacy_fingerprint.to_string().as_bytes());
    let legacy_public_id = format!("agent_status:agent:{}", &legacy_hash[..16]);
    let retained_adapter_fingerprint = json!({
        "severity": "critical",
        "category": "network",
        "target_kind": "tunnel",
        "target_id": format!(
            "legacy-tunnel-a:{}",
            legacy_unattributed_input.interface_name
        ),
        "title": "Tunnel adapter status failed",
        "status": "tunnel_adapter_degraded",
    });
    let retained_adapter_hash = payload_hash(retained_adapter_fingerprint.to_string().as_bytes());
    let retained_adapter_public_id = format!("network:tunnel:{}", &retained_adapter_hash[..16]);
    let pre_boundary_adapter_fingerprint = json!({
        "severity": "critical",
        "category": "network",
        "target_kind": "tunnel",
        "target_id": format!(
            "legacy-tunnel-a:{}",
            legacy_pre_boundary_input.interface_name
        ),
        "title": "Tunnel adapter status failed",
        "status": "tunnel_adapter_degraded",
    });
    let pre_boundary_adapter_hash =
        payload_hash(pre_boundary_adapter_fingerprint.to_string().as_bytes());
    let pre_boundary_adapter_public_id =
        format!("network:tunnel:{}", &pre_boundary_adapter_hash[..16]);
    sqlx::query(
        r#"
        INSERT INTO fleet_alert_states (alert_id, state, reason)
        VALUES ($1, 'acknowledged', 'preserve this operator triage'),
               ($2, 'escalated', 'preserve retained tunnel triage'),
               ($3, 'acknowledged', 'preserve pre-boundary tunnel triage'),
               ('webhook_delivery:00000000-0000-4000-8000-000000000001',
                'acknowledged', 'remove machine-owned orphan')
        "#,
    )
    .bind(&legacy_public_id)
    .bind(&retained_adapter_public_id)
    .bind(&pre_boundary_adapter_public_id)
    .execute(&db.pool)
    .await
    .unwrap();

    sqlx::migrate::Migrator::new(workspace_migrations_dir().as_path())
        .await
        .unwrap()
        .run(&db.pool)
        .await
        .expect("0010, 0011, and 0012 must apply to exact v0.4.4");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM _sqlx_migrations WHERE success")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        12
    );
    assert_eq!(
        sqlx::query_as::<_, (bool, bool)>(
            r#"
            SELECT
                to_regclass('public.operational_alert_episodes') IS NULL,
                to_regclass('public.alert_episodes') IS NOT NULL
            "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (true, true)
    );
    sqlx::query(
        r#"
        INSERT INTO telemetry_tunnels (
            client_id, observed_at, interface, kind, ownership_mode,
            mutation_policy, source, telemetry_plan_id, telemetry_plan_name,
            telemetry_plan_runtime_manager, telemetry_endpoint_side,
            telemetry_peer_client_id, traffic_status, traffic_reason,
            adapter_health, updated_at
        ) VALUES (
            'legacy-tunnel-a', clock_timestamp(), $1, 'gre',
            'custom_adapter', 'managed_desired', 'telemetry', $2, $3,
            'custom_adapter', 'left', 'legacy-tunnel-b', 'degraded',
            'post-cutover counters unavailable',
            '{"status":"failed","configured":true,"success":false,
              "reason":"post-cutover adapter failure"}'::jsonb,
            clock_timestamp()
        )
        "#,
    )
    .bind(&unmarked_unattributed_input.interface_name)
    .bind(unmarked_unattributed_tunnel_id.to_string())
    .bind(&unmarked_unattributed_input.name)
    .execute(&db.pool)
    .await
    .unwrap();
    assert!(!sqlx::query_scalar::<_, bool>(
        r#"
        SELECT operational_alert_legacy_identity
        FROM telemetry_tunnels
        WHERE client_id = 'legacy-tunnel-a' AND interface = $1
        "#,
    )
    .bind(&unmarked_unattributed_input.interface_name)
    .fetch_one(&db.pool)
    .await
    .unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM fleet_alert_states WHERE alert_id LIKE 'webhook_delivery:%'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM fleet_alert_states WHERE alert_id = $1",
        )
        .bind(&legacy_public_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        "acknowledged"
    );

    let first_repo = db.repo.clone();
    let concurrent_repo = db.repo.clone();
    let (first_reconcile, concurrent_reconcile) = tokio::join!(
        first_repo.reconcile_operational_alerts(),
        concurrent_repo.reconcile_operational_alerts(),
    );
    first_reconcile.unwrap();
    concurrent_reconcile.unwrap();

    let episode: (String, String, bool, i64, String, String, String) = sqlx::query_as(
        r#"
        SELECT public_id, lifecycle_state, backfilled, trigger_generation,
               producer_kind, policy_rule_kind, policy_rule_system_seed_key
        FROM alert_episodes
        WHERE producer_kind = 'agent.status' AND client_id = $1
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        episode,
        (
            legacy_public_id.clone(),
            "persisting".to_string(),
            true,
            1,
            "agent.status".to_string(),
            "state".to_string(),
            "agent.never_connected".to_string(),
        )
    );

    let policy_shapes = sqlx::query_as::<_, (String, String, String, String, Option<String>, i64)>(
        r#"
        SELECT producer_kind, policy_rule_kind, record_kind, policy_group_name,
               policy_rule_system_seed_key, count(*)
        FROM alert_episodes
        GROUP BY producer_kind, policy_rule_kind, record_kind, policy_group_name,
                 policy_rule_system_seed_key
        ORDER BY producer_kind
        "#,
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        policy_shapes,
        vec![
            (
                "agent.status".to_string(),
                "state".to_string(),
                "condition".to_string(),
                "System operational evidence policies".to_string(),
                Some("agent.never_connected".to_string()),
                1,
            ),
            (
                "backup.failure".to_string(),
                "occurrence".to_string(),
                "event".to_string(),
                "System operational evidence policies".to_string(),
                Some("backup.request_failure".to_string()),
                1,
            ),
            (
                "job.capability".to_string(),
                "occurrence".to_string(),
                "event".to_string(),
                "System operational evidence policies".to_string(),
                Some("job.capability_degraded".to_string()),
                1,
            ),
            (
                "job.terminal".to_string(),
                "occurrence".to_string(),
                "event".to_string(),
                "System operational evidence policies".to_string(),
                Some("job.general_hard_failure".to_string()),
                200,
            ),
            (
                "tunnel.adapter".to_string(),
                "state".to_string(),
                "condition".to_string(),
                "System operational evidence policies".to_string(),
                Some("tunnel.adapter_failure".to_string()),
                3,
            ),
            (
                "tunnel.traffic".to_string(),
                "state".to_string(),
                "condition".to_string(),
                "System operational evidence policies".to_string(),
                Some("tunnel.traffic_degraded".to_string()),
                3,
            ),
        ]
    );

    let retained_adapter = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            String,
            bool,
            i64,
            bool,
            String,
        ),
    >(
        r#"
        SELECT
            public_id,
            lifecycle_state,
            source_status,
            title,
            detail,
            backfilled,
            EXTRACT(EPOCH FROM last_confirmed_at)::bigint,
            evidence#>>'{source,retain_unknown_backfill}' = 'true',
            evidence#>>'{source,evidence_status}'
        FROM alert_episodes
        WHERE producer_kind = 'tunnel.adapter'
          AND target_id = $1
        "#,
    )
    .bind(format!(
        "legacy-tunnel-a:{}",
        legacy_unattributed_input.interface_name
    ))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        retained_adapter,
        (
            retained_adapter_public_id.clone(),
            "unknown".to_string(),
            "tunnel_adapter_degraded".to_string(),
            "Tunnel adapter status failed".to_string(),
            "retained legacy adapter failure".to_string(),
            true,
            1_578_182_400,
            true,
            "retained_degradation_current_attribution_unavailable".to_string(),
        )
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM fleet_alert_states WHERE alert_id = $1",
        )
        .bind(&retained_adapter_public_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        "escalated",
        "the legacy public ID must keep operator triage attached"
    );

    let pre_boundary_adapter = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            String,
            bool,
            i64,
            bool,
            String,
        ),
    >(
        r#"
        SELECT
            public_id,
            lifecycle_state,
            source_status,
            title,
            detail,
            backfilled,
            EXTRACT(EPOCH FROM last_confirmed_at)::bigint,
            evidence#>>'{source,retain_unknown_backfill}' = 'true',
            evidence#>>'{source,evidence_status}'
        FROM alert_episodes
        WHERE producer_kind = 'tunnel.adapter'
          AND target_id = $1
        "#,
    )
    .bind(format!(
        "legacy-tunnel-a:{}",
        legacy_pre_boundary_input.interface_name
    ))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        pre_boundary_adapter,
        (
            pre_boundary_adapter_public_id.clone(),
            "unknown".to_string(),
            "tunnel_adapter_evidence_missing".to_string(),
            "Tunnel adapter status failed".to_string(),
            "pre-boundary legacy adapter failure".to_string(),
            true,
            1_578_009_600,
            true,
            "retained_degradation_current_attribution_unavailable".to_string(),
        )
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM fleet_alert_states WHERE alert_id = $1",
        )
        .bind(&pre_boundary_adapter_public_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        "acknowledged",
        "the status-boundary fallback must keep legacy triage attached"
    );

    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            r#"
            SELECT count(*)
            FROM alert_episodes
            WHERE target_id = $1
            "#,
        )
        .bind(format!(
            "legacy-tunnel-a:{}",
            legacy_unattributed_healthy_input.interface_name
        ))
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0,
        "unattributed healthy legacy evidence must not invent an incident"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            r#"
            SELECT count(*)
            FROM alert_episodes
            WHERE target_id = $1
              AND producer_kind IN ('tunnel.adapter', 'tunnel.traffic')
              AND backfilled
            "#,
        )
        .bind(format!(
            "legacy-tunnel-a:{}",
            unmarked_unattributed_input.interface_name
        ))
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0,
        "post-migration unmarked evidence is outside the maintenance-gated backfill"
    );
    assert_eq!(
        sqlx::query_as::<_, (bool, bool)>(
            r#"
            SELECT backfill_completed, completed_at IS NOT NULL
            FROM operational_alert_lifecycle_meta
            WHERE singleton
            "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (true, true)
    );

    let lifecycle_shapes = sqlx::query_as::<_, (String, String, Option<String>, i64)>(
        r#"
            SELECT producer_kind, lifecycle_state, resolution_reason, count(*)
            FROM alert_episodes
            GROUP BY producer_kind, lifecycle_state, resolution_reason
            ORDER BY producer_kind, lifecycle_state
            "#,
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        lifecycle_shapes,
        vec![
            (
                "agent.status".to_string(),
                "persisting".to_string(),
                None,
                1,
            ),
            (
                "backup.failure".to_string(),
                "resolved".to_string(),
                Some("policy_time_elapsed".to_string()),
                1,
            ),
            (
                "job.capability".to_string(),
                "resolved".to_string(),
                Some("policy_time_elapsed".to_string()),
                1,
            ),
            (
                "job.terminal".to_string(),
                "resolved".to_string(),
                Some("policy_time_elapsed".to_string()),
                200,
            ),
            (
                "tunnel.adapter".to_string(),
                "persisting".to_string(),
                None,
                1,
            ),
            ("tunnel.adapter".to_string(), "unknown".to_string(), None, 2,),
            (
                "tunnel.traffic".to_string(),
                "persisting".to_string(),
                None,
                1,
            ),
            ("tunnel.traffic".to_string(), "unknown".to_string(), None, 2,),
        ]
    );

    let ownership: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            count(*)::bigint,
            count(*) FILTER (
                WHERE episode.policy_group_id IS NOT NULL
                  AND episode.policy_rule_id IS NOT NULL
                  AND episode.policy_rule_version = 1
                  AND episode.policy_rule_kind IS NOT NULL
                  AND episode.trigger_evidence_id IS NOT NULL
                  AND episode.last_evidence_id IS NOT NULL
            )::bigint,
            count(*) FILTER (
                WHERE evidence.source_kind = episode.producer_kind
                  AND evidence.fact_kind = episode.policy_rule_kind
                  AND evidence.natural_key = episode.natural_key
            )::bigint,
            count(*) FILTER (
                WHERE EXISTS (
                    SELECT 1
                    FROM alert_policy_evidence_receipts receipt
                    WHERE receipt.policy_rule_id = episode.policy_rule_id
                      AND receipt.rule_version = episode.policy_rule_version
                      AND receipt.evidence_id = episode.trigger_evidence_id
                )
            )::bigint
        FROM alert_episodes episode
        LEFT JOIN alert_policy_evidence evidence
          ON evidence.id = episode.trigger_evidence_id
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        ownership,
        (209, 209, 209, 209),
        "every retained episode must have typed policy provenance and consumed trigger evidence"
    );
    assert_eq!(
        sqlx::query_as::<_, (String, i64)>(
            r#"
            SELECT receipt.result, count(*)
            FROM alert_episodes episode
            JOIN alert_policy_evidence_receipts receipt
              ON receipt.policy_rule_id = episode.policy_rule_id
             AND receipt.rule_version = episode.policy_rule_version
             AND receipt.evidence_id = episode.trigger_evidence_id
            GROUP BY receipt.result
            ORDER BY receipt.result
            "#,
        )
        .fetch_all(&db.pool)
        .await
        .unwrap(),
        vec![("matched".to_string(), 205), ("unknown".to_string(), 4),]
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM alert_lifecycle_events")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        0,
        "migration and retained-source reconciliation must not synthesize lifecycle edges"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM webhook_events WHERE kind IN ('alert.triggered', 'alert.resolved')",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0,
        "a quiet lifecycle migration must not bypass the dedicated outbox"
    );

    let stable_counts: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM alert_episodes)::bigint,
            (SELECT count(*) FROM alert_policy_evidence)::bigint,
            (SELECT count(*) FROM alert_policy_evidence_receipts)::bigint,
            (SELECT count(*) FROM alert_lifecycle_events)::bigint
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    db.repo.reconcile_operational_alerts().await.unwrap();
    db.repo.reconcile_operational_alerts().await.unwrap();
    assert_eq!(
        sqlx::query_as::<_, (i64, i64, i64, i64)>(
            r#"
            SELECT
                (SELECT count(*) FROM alert_episodes)::bigint,
                (SELECT count(*) FROM alert_policy_evidence)::bigint,
                (SELECT count(*) FROM alert_policy_evidence_receipts)::bigint,
                (SELECT count(*) FROM alert_lifecycle_events)::bigint
            "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        stable_counts,
        "repeated reconciliation must not duplicate evidence, receipts, episodes, or outbox edges"
    );
    let retained_episode: (Uuid, String, i64, Uuid, Uuid) = sqlx::query_as(
        r#"
        SELECT id, public_id, trigger_generation, policy_rule_id, trigger_evidence_id
        FROM alert_episodes
        WHERE producer_kind = 'tunnel.adapter' AND target_id = $1
        "#,
    )
    .bind(format!(
        "legacy-tunnel-a:{}",
        legacy_pre_boundary_input.interface_name
    ))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(retained_episode.1, pre_boundary_adapter_public_id);
    assert_eq!(retained_episode.2, 1);

    let exact_runtime_identity = vpsman_common::tunnel_runtime_evidence_identity_hash(
        legacy_pre_boundary_tunnel_id,
        &legacy_pre_boundary_plan,
        None,
    );
    let exact_topology_identity = vpsman_common::tunnel_topology_identity_hash(
        legacy_pre_boundary_tunnel_id,
        &legacy_pre_boundary_plan,
    );
    sqlx::query(
        r#"
        UPDATE telemetry_tunnels
        SET observed_at = clock_timestamp(),
            updated_at = clock_timestamp(),
            telemetry_topology_identity_hash = $1,
            telemetry_runtime_evidence_identity_hash = $2,
            traffic_status = 'degraded',
            traffic_reason = 'fresh exact counters unavailable',
            adapter_health = '{"status":"failed","configured":true,"success":false,
                "reason":"fresh exact adapter failure"}'::jsonb
        WHERE client_id = 'legacy-tunnel-a' AND interface = $3
        "#,
    )
    .bind(&exact_topology_identity)
    .bind(&exact_runtime_identity)
    .bind(&legacy_pre_boundary_input.interface_name)
    .execute(&db.pool)
    .await
    .unwrap();
    db.repo.reconcile_operational_alerts().await.unwrap();

    let persisted_exact: (Uuid, String, String, i64, Uuid, Uuid, String) = sqlx::query_as(
        r#"
        SELECT id, public_id, lifecycle_state, trigger_generation,
               trigger_evidence_id, last_evidence_id,
               evidence#>>'{source,status}'
        FROM alert_episodes
        WHERE policy_rule_id = $1 AND target_id = $2
        "#,
    )
    .bind(retained_episode.3)
    .bind(format!(
        "legacy-tunnel-a:{}",
        legacy_pre_boundary_input.interface_name
    ))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(persisted_exact.0, retained_episode.0);
    assert_eq!(persisted_exact.1, pre_boundary_adapter_public_id);
    assert_eq!(persisted_exact.2, "persisting");
    assert_eq!(persisted_exact.3, 1);
    assert_eq!(persisted_exact.4, retained_episode.4);
    assert_ne!(persisted_exact.5, retained_episode.4);
    assert_eq!(persisted_exact.6, "tunnel_adapter_degraded");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM alert_lifecycle_events WHERE episode_id=$1 AND edge_kind='alert.triggered'",
        )
        .bind(retained_episode.0)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0,
        "fresh confirmation of a retained Unknown episode must not forge a Trigger edge"
    );

    sqlx::query(
        r#"
        UPDATE telemetry_tunnels
        SET observed_at = clock_timestamp(),
            updated_at = clock_timestamp(),
            traffic_status = 'degraded',
            traffic_reason = 'fresh exact counters unavailable',
            adapter_health = '{"status":"healthy","configured":true,"success":true}'::jsonb
        WHERE client_id = 'legacy-tunnel-a' AND interface = $1
        "#,
    )
    .bind(&legacy_pre_boundary_input.interface_name)
    .execute(&db.pool)
    .await
    .unwrap();
    db.repo.reconcile_operational_alerts().await.unwrap();
    let forced_due = sqlx::query(
        r#"
        UPDATE alert_policy_evaluation_states
        SET resolve_segment_started_at = clock_timestamp() - interval '61 seconds',
            next_transition_at = clock_timestamp() - interval '1 second'
        WHERE policy_rule_id = $1 AND active_episode_id = $2
        "#,
    )
    .bind(retained_episode.3)
    .bind(retained_episode.0)
    .execute(&db.pool)
    .await
    .unwrap();
    assert_eq!(forced_due.rows_affected(), 1);
    assert_eq!(
        crate::repository_policy_lifecycle::evaluate_due_policy_transitions(&db.pool, 10)
            .await
            .unwrap(),
        1
    );

    type ResolvedExactRow = (
        Uuid,
        String,
        String,
        i64,
        Option<String>,
        Uuid,
        Uuid,
        String,
        String,
        String,
        i32,
    );
    let resolved_exact: ResolvedExactRow = sqlx::query_as(
        r#"
        SELECT id, public_id, lifecycle_state, trigger_generation,
               resolution_reason, trigger_evidence_id, last_evidence_id,
               evidence#>>'{resolution_evidence_snapshot,source,status}',
               evidence#>>'{resolution_evidence_snapshot,source,adapter,success}',
               evidence#>>'{resolution_evidence_snapshot,source_completeness}',
               jsonb_array_length(evidence->'resolution_confirmation_evidence')
        FROM alert_episodes
        WHERE policy_rule_id = $1 AND target_id = $2
        "#,
    )
    .bind(retained_episode.3)
    .bind(format!(
        "legacy-tunnel-a:{}",
        legacy_pre_boundary_input.interface_name
    ))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(resolved_exact.0, retained_episode.0);
    assert_eq!(resolved_exact.1, pre_boundary_adapter_public_id);
    assert_eq!(resolved_exact.2, "resolved");
    assert_eq!(resolved_exact.3, 1);
    assert_eq!(resolved_exact.4.as_deref(), Some("condition_recovered"));
    assert_eq!(resolved_exact.5, retained_episode.4);
    assert_ne!(resolved_exact.6, retained_episode.4);
    assert_eq!(resolved_exact.7, "tunnel_adapter_healthy");
    assert_eq!(resolved_exact.8, "true");
    assert_eq!(resolved_exact.9, "complete");
    assert_eq!(resolved_exact.10, 1);
    assert_eq!(
        sqlx::query_as::<_, (i64, i64)>(
            r#"
            SELECT
                count(*) FILTER (WHERE edge_kind = 'alert.triggered')::bigint,
                count(*) FILTER (WHERE edge_kind = 'alert.resolved')::bigint
            FROM alert_lifecycle_events
            WHERE episode_id = $1
            "#,
        )
        .bind(retained_episode.0)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (0, 0),
        "a quiet backfill must not emit an orphan Resolve edge without a Trigger edge"
    );
    assert_eq!(
        crate::repository_policy_lifecycle::evaluate_due_policy_transitions(&db.pool, 10)
            .await
            .unwrap(),
        0,
        "the resolved transition must remain idempotent"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM alert_episodes WHERE id=$1 AND trigger_generation=1",
        )
        .bind(retained_episode.0)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        1,
        "recovery must resolve the retained generation in place"
    );

    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM alert_episodes WHERE client_id = $1",)
            .bind(client_id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM alert_episodes")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        209,
        "repair must neither duplicate nor ingest legacy sources beyond the bounded horizon"
    );

    db.cleanup().await;
    fs::remove_dir_all(&baseline_dir).unwrap();
}

#[tokio::test]
async fn postgres_fresh_schema_has_disabled_resource_policy_starters() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let groups = sqlx::query_as::<_, (Uuid, String, bool, String)>(
        r#"
        SELECT id, name, enabled, selector_expression
        FROM policy_groups
        WHERE id = ANY($1::uuid[])
        ORDER BY id
        "#,
    )
    .bind(vec![
        Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa4").unwrap(),
        Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa5").unwrap(),
        Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa6").unwrap(),
    ])
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        groups,
        vec![
            (
                Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa4").unwrap(),
                "Predefined CPU utilization".to_string(),
                false,
                "status:online".to_string(),
            ),
            (
                Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa5").unwrap(),
                "Predefined memory availability".to_string(),
                false,
                "status:online".to_string(),
            ),
            (
                Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa6").unwrap(),
                "Predefined disk availability".to_string(),
                false,
                "status:online".to_string(),
            ),
        ]
    );

    let rules = sqlx::query_scalar::<_, Value>(
        r#"
        SELECT jsonb_build_object(
            'group_name', groups.name,
            'sort_order', rules.sort_order,
            'trigger_condition_expression', rules.trigger_condition_expression,
            'trigger_meta_condition', rules.trigger_meta_condition,
            'resolve_condition_expression', rules.resolve_condition_expression,
            'resolve_meta_condition', rules.resolve_meta_condition,
            'severity', rules.severity,
            'rule_kind', rules.rule_kind,
            'evidence_source', rules.evidence_source,
            'correlation_mode', rules.correlation_mode,
            'category', rules.category,
            'system_seed_key', rules.system_seed_key
        )
        FROM policy_rules AS rules
        JOIN policy_groups AS groups ON groups.id = rules.group_id
        WHERE groups.id = ANY($1::uuid[])
        ORDER BY groups.id, rules.sort_order
        "#,
    )
    .bind(vec![
        Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa4").unwrap(),
        Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa5").unwrap(),
        Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa6").unwrap(),
    ])
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        rules,
        vec![
            json!({
                "group_name": "Predefined CPU utilization",
                "sort_order": 0,
                "trigger_condition_expression":
                    "cpu.utilization_ratio >= 0.75 && cpu.utilization_ratio < 0.90",
                "trigger_meta_condition": {"kind": "sustained", "seconds": 300},
                "resolve_condition_expression": null,
                "resolve_meta_condition": null,
                "severity": "warning",
                "rule_kind": "metric",
                "evidence_source": "telemetry.combined",
                "correlation_mode": "natural_key",
                "category": "resource",
                "system_seed_key": null,
            }),
            json!({
                "group_name": "Predefined CPU utilization",
                "sort_order": 1,
                "trigger_condition_expression": "cpu.utilization_ratio >= 0.90",
                "trigger_meta_condition": {"kind": "sustained", "seconds": 300},
                "resolve_condition_expression": null,
                "resolve_meta_condition": null,
                "severity": "critical",
                "rule_kind": "metric",
                "evidence_source": "telemetry.combined",
                "correlation_mode": "natural_key",
                "category": "resource",
                "system_seed_key": null,
            }),
            json!({
                "group_name": "Predefined memory availability",
                "sort_order": 0,
                "trigger_condition_expression":
                    "memory.available_ratio <= 0.20 && memory.available_ratio > 0.10",
                "trigger_meta_condition": {"kind": "sustained", "seconds": 300},
                "resolve_condition_expression": null,
                "resolve_meta_condition": null,
                "severity": "warning",
                "rule_kind": "metric",
                "evidence_source": "telemetry.combined",
                "correlation_mode": "natural_key",
                "category": "resource",
                "system_seed_key": null,
            }),
            json!({
                "group_name": "Predefined memory availability",
                "sort_order": 1,
                "trigger_condition_expression": "memory.available_ratio <= 0.10",
                "trigger_meta_condition": {"kind": "sustained", "seconds": 300},
                "resolve_condition_expression": null,
                "resolve_meta_condition": null,
                "severity": "critical",
                "rule_kind": "metric",
                "evidence_source": "telemetry.combined",
                "correlation_mode": "natural_key",
                "category": "resource",
                "system_seed_key": null,
            }),
            json!({
                "group_name": "Predefined disk availability",
                "sort_order": 0,
                "trigger_condition_expression":
                    "disk.available_ratio <= 0.20 && disk.available_ratio > 0.10",
                "trigger_meta_condition": {"kind": "sustained", "seconds": 300},
                "resolve_condition_expression": null,
                "resolve_meta_condition": null,
                "severity": "warning",
                "rule_kind": "metric",
                "evidence_source": "telemetry.combined",
                "correlation_mode": "natural_key",
                "category": "resource",
                "system_seed_key": null,
            }),
            json!({
                "group_name": "Predefined disk availability",
                "sort_order": 1,
                "trigger_condition_expression": "disk.available_ratio <= 0.10",
                "trigger_meta_condition": {"kind": "sustained", "seconds": 300},
                "resolve_condition_expression": null,
                "resolve_meta_condition": null,
                "severity": "critical",
                "rule_kind": "metric",
                "evidence_source": "telemetry.combined",
                "correlation_mode": "natural_key",
                "category": "resource",
                "system_seed_key": null,
            }),
        ]
    );
    let legacy_resource_starters = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM policy_groups WHERE id = ANY($1::uuid[])",
    )
    .bind(vec![
        Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1").unwrap(),
        Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa2").unwrap(),
    ])
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(legacy_resource_starters, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM policy_groups WHERE id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa3'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        1
    );

    let lifecycle_owners = sqlx::query_as::<_, (bool, bool, bool, bool, bool)>(
        r#"
        SELECT
            to_regclass('public.policy_alerts') IS NULL,
            to_regclass('public.policy_rule_states') IS NULL,
            to_regclass('public.alert_episodes') IS NOT NULL,
            to_regclass('public.alert_policy_evaluation_states') IS NOT NULL,
            to_regclass('public.alert_lifecycle_events') IS NOT NULL
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(lifecycle_owners, (true, true, true, true, true));

    let lifecycle_columns = sqlx::query_as::<_, (String, String, String)>(
        r#"
        SELECT column_name, data_type, is_nullable
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'alert_episodes'
          AND column_name IN (
              'first_post_upgrade_evaluated_at', 'last_evidence_id',
              'policy_group_id', 'policy_rule_id', 'policy_rule_kind',
              'policy_rule_version', 'trigger_evidence_id'
          )
        ORDER BY column_name
        "#,
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        lifecycle_columns,
        vec![
            (
                "first_post_upgrade_evaluated_at".to_string(),
                "timestamp with time zone".to_string(),
                "YES".to_string(),
            ),
            (
                "last_evidence_id".to_string(),
                "uuid".to_string(),
                "YES".to_string(),
            ),
            (
                "policy_group_id".to_string(),
                "uuid".to_string(),
                "NO".to_string(),
            ),
            (
                "policy_rule_id".to_string(),
                "uuid".to_string(),
                "NO".to_string(),
            ),
            (
                "policy_rule_kind".to_string(),
                "text".to_string(),
                "NO".to_string(),
            ),
            (
                "policy_rule_version".to_string(),
                "integer".to_string(),
                "NO".to_string(),
            ),
            (
                "trigger_evidence_id".to_string(),
                "uuid".to_string(),
                "YES".to_string(),
            ),
        ]
    );
    let lifecycle_constraint = sqlx::query_scalar::<_, String>(
        r#"
            SELECT pg_get_constraintdef(oid)
            FROM pg_constraint
            WHERE conrelid = 'alert_episodes'::regclass
              AND conname = 'alert_episodes_lifecycle_check'
            "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(
        lifecycle_constraint.contains("resolved_at >= last_confirmed_at"),
        "resolved episodes must preserve causal timestamp ordering: {lifecycle_constraint}"
    );
    assert_eq!(
        sqlx::query_as::<_, (String, String)>(
            r#"
            SELECT pg_class.relname,
                   pg_get_expr(pg_index.indpred, pg_index.indrelid)
            FROM pg_index
            JOIN pg_class ON pg_class.oid = pg_index.indexrelid
            WHERE pg_class.relname IN (
                'alert_episodes_one_current_idx',
                'alert_policy_evaluation_states_due_idx'
            )
              AND pg_index.indisvalid
            ORDER BY pg_class.relname
            "#,
        )
        .fetch_all(&db.pool)
        .await
        .unwrap(),
        vec![
            (
                "alert_episodes_one_current_idx".to_string(),
                "((resolved_at IS NULL) AND (last_confirmed_at IS NOT NULL))".to_string(),
            ),
            (
                "alert_policy_evaluation_states_due_idx".to_string(),
                "(next_transition_at IS NOT NULL)".to_string(),
            ),
        ]
    );
    assert_eq!(
        sqlx::query_as::<_, (String, i64)>(
            r#"
            SELECT consumer_kind, last_event_seq
            FROM alert_lifecycle_consumer_cursors
            ORDER BY consumer_kind
            "#,
        )
        .fetch_all(&db.pool)
        .await
        .unwrap(),
        vec![("schedule".to_string(), 0), ("webhook".to_string(), 0)]
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_exact_v044_resource_policy_upgrade_preserves_operator_intent() {
    let Some((db, baseline_dir)) = exact_v044_migration_test_db().await else {
        return;
    };
    let cpu_group_id = Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1").unwrap();
    let cpu_rule_id = Uuid::parse_str("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb1").unwrap();
    let memory_group_id = Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa2").unwrap();
    sqlx::query(
        r#"
        UPDATE policy_groups
        SET name = 'Operator CPU policy',
            enabled = TRUE,
            notes = 'operator-owned threshold'
        WHERE id = $1
        "#,
    )
    .bind(cpu_group_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE policy_rules
        SET name = 'Operator CPU load threshold',
            condition_expression = 'cpu.load_1 >= 3'
        WHERE id = $1
        "#,
    )
    .bind(cpu_rule_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM policy_groups WHERE id = $1")
        .bind(memory_group_id)
        .execute(&db.pool)
        .await
        .unwrap();

    let cpu_group_before = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(policy_group) FROM policy_groups AS policy_group WHERE id = $1",
    )
    .bind(cpu_group_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let cpu_rule_before = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(policy_rule) FROM policy_rules AS policy_rule WHERE id = $1",
    )
    .bind(cpu_rule_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let traffic_group_before = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(policy_group) FROM policy_groups AS policy_group WHERE id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa3'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let traffic_rule_before = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(policy_rule) FROM policy_rules AS policy_rule WHERE id = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb3'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();

    sqlx::migrate::Migrator::new(workspace_migrations_dir().as_path())
        .await
        .unwrap()
        .run(&db.pool)
        .await
        .expect("0010, 0011, and 0012 must apply after exact v0.4.4 migration checksums");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM _sqlx_migrations WHERE success")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        12
    );
    assert_eq!(
        sqlx::query_scalar::<_, Value>(
            "SELECT to_jsonb(policy_group) FROM policy_groups AS policy_group WHERE id = $1",
        )
        .bind(cpu_group_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        cpu_group_before
    );
    let cpu_rule_after = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(policy_rule) FROM policy_rules AS policy_rule WHERE id = $1",
    )
    .bind(cpu_rule_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, Value>(
            "SELECT to_jsonb(policy_group) FROM policy_groups AS policy_group WHERE id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa3'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        traffic_group_before
    );
    let traffic_rule_after = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(policy_rule) FROM policy_rules AS policy_rule WHERE id = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb3'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    for (before, after, expected_category) in [
        (&cpu_rule_before, &cpu_rule_after, "resource"),
        (&traffic_rule_before, &traffic_rule_after, "traffic"),
    ] {
        for key in [
            "id",
            "group_id",
            "rule_version",
            "sort_order",
            "name",
            "enabled",
            "traffic_selector",
            "severity",
            "created_at",
            "updated_at",
        ] {
            assert_eq!(
                after.get(key),
                before.get(key),
                "migrated rule field: {key}"
            );
        }
        assert_eq!(
            after.get("trigger_condition_expression"),
            before.get("condition_expression")
        );
        assert_eq!(
            after["trigger_meta_condition"],
            json!({"kind": "sustained", "seconds": 300})
        );
        assert_eq!(after["rule_kind"], "metric");
        assert_eq!(after["evidence_source"], "telemetry.combined");
        assert_eq!(after["correlation_mode"], "natural_key");
        assert_eq!(after["category"], expected_category);
        assert_eq!(after["resolve_condition_expression"], Value::Null);
        assert_eq!(after["resolve_meta_condition"], Value::Null);
        assert_eq!(after["system_seed_key"], Value::Null);
        assert_eq!(after["armed_after_evidence_seq"], 0);
        assert!(after.get("condition_expression").is_none());
        assert!(after.get("window_secs").is_none());
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM policy_groups WHERE id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa4'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM policy_groups WHERE id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa5'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0,
        "an operator-deleted memory starter must stay deleted"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM policy_groups WHERE id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa6'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        1
    );

    db.cleanup().await;
    fs::remove_dir_all(&baseline_dir).unwrap();
}

#[tokio::test]
async fn postgres_exact_v044_policy_upgrade_resets_only_saturation_state_and_keeps_history() {
    let Some((db, baseline_dir)) = exact_v044_migration_test_db().await else {
        return;
    };
    let client_id = "migration-policy-state-client";
    insert_client(&db.pool, client_id, None).await;

    let legacy_cpu_group_id = Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1").unwrap();
    let legacy_cpu_rule_id = Uuid::parse_str("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb1").unwrap();
    let legacy_alert_id = Uuid::parse_str("eeeeeeee-eeee-4eee-8eee-eeeeeeeeeee1").unwrap();
    insert_policy_migration_state(&db.pool, legacy_cpu_rule_id, client_id, 1).await;
    insert_policy_migration_alert(
        &db.pool,
        legacy_alert_id,
        legacy_cpu_group_id,
        legacy_cpu_rule_id,
        client_id,
    )
    .await;
    sqlx::query("INSERT INTO fleet_alert_states (alert_id, state) VALUES ($1, 'acknowledged')")
        .bind(format!("policy-alert:{legacy_alert_id}"))
        .execute(&db.pool)
        .await
        .unwrap();

    let restored_memory_group_id = Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa2").unwrap();
    let restored_memory_rule_id = Uuid::parse_str("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb2").unwrap();
    sqlx::query(
        "UPDATE policy_groups SET updated_at = created_at + interval '1 second' WHERE id = $1",
    )
    .bind(restored_memory_group_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE policy_rules SET updated_at = created_at + interval '1 second' WHERE id = $1",
    )
    .bind(restored_memory_rule_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let custom_group_id = Uuid::parse_str("cccccccc-cccc-4ccc-8ccc-ccccccccccc1").unwrap();
    let affected_rule_id = Uuid::parse_str("dddddddd-dddd-4ddd-8ddd-ddddddddddd1").unwrap();
    let unaffected_rule_id = Uuid::parse_str("dddddddd-dddd-4ddd-8ddd-ddddddddddd2").unwrap();
    let affected_alert_id = Uuid::parse_str("eeeeeeee-eeee-4eee-8eee-eeeeeeeeeee2").unwrap();
    sqlx::query(
        r#"
        INSERT INTO policy_groups (id, name, enabled, selector_expression, notes)
        VALUES ($1, 'Operator saturation policy', TRUE, 'status:online', 'retained')
        "#,
    )
    .bind(custom_group_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO policy_rules (
            id, group_id, rule_version, sort_order, name, enabled,
            condition_expression, window_secs, severity
        ) VALUES
            ($1, $3, 7, 0, 'Saturation', TRUE,
             'cpu.load_saturation >= 0.75', 0, 'warning'),
            ($2, $3, 4, 1, 'Raw load', TRUE,
             'cpu.load_1 >= 2', 300, 'warning')
        "#,
    )
    .bind(affected_rule_id)
    .bind(unaffected_rule_id)
    .bind(custom_group_id)
    .execute(&db.pool)
    .await
    .unwrap();
    insert_policy_migration_state(&db.pool, affected_rule_id, client_id, 7).await;
    insert_policy_migration_state(&db.pool, unaffected_rule_id, client_id, 4).await;
    insert_policy_migration_alert(
        &db.pool,
        affected_alert_id,
        custom_group_id,
        affected_rule_id,
        client_id,
    )
    .await;
    let unaffected_rule_before = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(policy_rule) FROM policy_rules AS policy_rule WHERE id = $1",
    )
    .bind(unaffected_rule_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let unaffected_state_before = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(policy_state) FROM policy_rule_states AS policy_state WHERE policy_rule_id = $1",
    )
    .bind(unaffected_rule_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    sqlx::migrate::Migrator::new(workspace_migrations_dir().as_path())
        .await
        .unwrap()
        .run(&db.pool)
        .await
        .expect("0010, 0011, and 0012 lifecycle migrations must apply to exact v0.4.4");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM _sqlx_migrations WHERE success")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        12
    );

    let affected_after = sqlx::query_as::<_, (i32, String, String, String, String, Option<Value>)>(
        r#"
            SELECT rule_version, trigger_condition_expression, rule_kind,
                   evidence_source, category, trigger_meta_condition
            FROM policy_rules
            WHERE id = $1
            "#,
    )
    .bind(affected_rule_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        affected_after,
        (
            8,
            "cpu.load_saturation >= 0.75".to_string(),
            "metric".to_string(),
            "telemetry.combined".to_string(),
            "resource".to_string(),
            None,
        )
    );
    let affected_state = sqlx::query_scalar::<_, Value>(
        r#"
        SELECT jsonb_build_object(
            'rule_version', rule_version,
            'confirmation_bucket_key', confirmation_bucket_key,
            'subject_client_id', subject_client_id,
            'truth_state', truth_state,
            'last_evidence_id', last_evidence_id,
            'trigger_confirmed_duration_secs', trigger_confirmed_duration_secs,
            'trigger_segment_started_at', trigger_segment_started_at,
            'trigger_generation', trigger_generation,
            'active_episode_id', active_episode_id,
            'first_post_upgrade_evaluated_at', first_post_upgrade_evaluated_at
        )
        FROM alert_policy_evaluation_states
        WHERE policy_rule_id = $1
        "#,
    )
    .bind(affected_rule_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        affected_state,
        json!({
            "rule_version": 8,
            "confirmation_bucket_key": format!("natural:{client_id}"),
            "subject_client_id": client_id,
            "truth_state": "unknown",
            "last_evidence_id": affected_alert_id,
            "trigger_confirmed_duration_secs": 0,
            "trigger_segment_started_at": null,
            "trigger_generation": 1,
            "active_episode_id": null,
            "first_post_upgrade_evaluated_at": null,
        }),
        "the invalidated saturation dwell must re-enter as quiet Unknown history"
    );
    let affected_episode = sqlx::query_scalar::<_, Value>(
        r#"
        SELECT jsonb_build_object(
            'public_id', public_id,
            'producer_kind', producer_kind,
            'record_kind', record_kind,
            'trigger_generation', trigger_generation,
            'lifecycle_state', lifecycle_state,
            'last_confirmed_at', last_confirmed_at,
            'resolved_at', resolved_at,
            'resolution_reason', resolution_reason,
            'policy_group_id', policy_group_id,
            'policy_rule_id', policy_rule_id,
            'policy_rule_version', policy_rule_version,
            'policy_rule_kind', policy_rule_kind,
            'trigger_evidence_id', trigger_evidence_id,
            'last_evidence_id', last_evidence_id,
            'first_post_upgrade_evaluated_at', first_post_upgrade_evaluated_at,
            'backfilled', backfilled
        )
        FROM alert_episodes
        WHERE id = $1
        "#,
    )
    .bind(affected_alert_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        affected_episode,
        json!({
            "public_id": format!("policy-alert:{affected_alert_id}"),
            "producer_kind": "telemetry.combined",
            "record_kind": "condition",
            "trigger_generation": 1,
            "lifecycle_state": "unknown",
            "last_confirmed_at": null,
            "resolved_at": null,
            "resolution_reason": null,
            "policy_group_id": custom_group_id,
            "policy_rule_id": affected_rule_id,
            "policy_rule_version": 8,
            "policy_rule_kind": "metric",
            "trigger_evidence_id": affected_alert_id,
            "last_evidence_id": affected_alert_id,
            "first_post_upgrade_evaluated_at": null,
            "backfilled": true,
        }),
        "pre-0010 policy history must enter the unified owner conservatively"
    );

    let unaffected_rule_after = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(policy_rule) FROM policy_rules AS policy_rule WHERE id = $1",
    )
    .bind(unaffected_rule_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    for key in [
        "id",
        "group_id",
        "rule_version",
        "sort_order",
        "name",
        "enabled",
        "traffic_selector",
        "severity",
        "created_at",
        "updated_at",
    ] {
        assert_eq!(
            unaffected_rule_after.get(key),
            unaffected_rule_before.get(key),
            "unaffected migrated rule field: {key}"
        );
    }
    assert_eq!(
        unaffected_rule_after.get("trigger_condition_expression"),
        unaffected_rule_before.get("condition_expression")
    );
    assert_eq!(
        unaffected_rule_after["trigger_meta_condition"],
        json!({"kind": "sustained", "seconds": 300})
    );
    assert_eq!(unaffected_rule_after["rule_kind"], "metric");
    assert_eq!(
        unaffected_rule_after["evidence_source"],
        "telemetry.combined"
    );
    assert_eq!(unaffected_rule_after["correlation_mode"], "natural_key");
    assert_eq!(unaffected_rule_after["category"], "resource");
    assert!(unaffected_rule_after.get("condition_expression").is_none());
    assert!(unaffected_rule_after.get("window_secs").is_none());

    let unaffected_state_after = sqlx::query_scalar::<_, Value>(
        r#"
        SELECT jsonb_build_object(
            'rule_version', rule_version,
            'confirmation_bucket_key', confirmation_bucket_key,
            'subject_client_id', subject_client_id,
            'truth_state', truth_state,
            'last_evidence_id', last_evidence_id,
            'trigger_confirmed_duration_secs', trigger_confirmed_duration_secs,
            'trigger_segment_started_at', trigger_segment_started_at,
            'trigger_generation', trigger_generation,
            'active_episode_id', active_episode_id,
            'first_post_upgrade_evaluated_at', first_post_upgrade_evaluated_at,
            'last_evaluated_at', last_evaluated_at,
            'updated_at', updated_at
        )
        FROM alert_policy_evaluation_states
        WHERE policy_rule_id = $1
        "#,
    )
    .bind(unaffected_rule_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(unaffected_state_after["rule_version"], 4);
    assert_eq!(
        unaffected_state_after["confirmation_bucket_key"],
        format!("natural:{client_id}")
    );
    assert_eq!(unaffected_state_after["subject_client_id"], client_id);
    assert_eq!(unaffected_state_after["truth_state"], "matched");
    assert_eq!(unaffected_state_after["last_evidence_id"], Value::Null);
    assert_eq!(unaffected_state_after["trigger_confirmed_duration_secs"], 0);
    assert_eq!(
        unaffected_state_after["trigger_segment_started_at"],
        Value::Null
    );
    assert_eq!(unaffected_state_after["trigger_generation"], 1);
    assert_eq!(unaffected_state_after["active_episode_id"], Value::Null);
    assert_eq!(
        unaffected_state_after["first_post_upgrade_evaluated_at"],
        Value::Null
    );
    assert_eq!(
        unaffected_state_after["last_evaluated_at"],
        unaffected_state_before["last_evaluated_at"]
    );
    assert_eq!(
        unaffected_state_after["updated_at"],
        unaffected_state_before["updated_at"]
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM policy_groups WHERE id = $1")
            .bind(legacy_cpu_group_id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1,
        "a pristine starter with history must not be removed"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM alert_policy_evaluation_states WHERE policy_rule_id = $1",
        )
        .bind(legacy_cpu_rule_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_as::<
            _,
            (
                String,
                Option<String>,
                Option<String>,
                Option<String>,
                Uuid,
                Uuid
            ),
        >(
            r#"
            SELECT lifecycle_state, last_confirmed_at::text,
                   resolved_at::text, resolution_reason,
                   trigger_evidence_id, last_evidence_id
            FROM alert_episodes
            WHERE id = $1
            "#,
        )
        .bind(legacy_alert_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (
            "unknown".to_string(),
            None,
            None,
            None,
            legacy_alert_id,
            legacy_alert_id,
        ),
        "legacy starter history must not acquire invented lifecycle evidence"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM fleet_alert_states WHERE alert_id = $1")
            .bind(format!("policy-alert:{legacy_alert_id}"))
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM policy_groups WHERE id = $1")
            .bind(restored_memory_group_id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1,
        "a restored canonical group with operator update history must not be removed"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM policy_rules WHERE id = $1")
            .bind(restored_memory_rule_id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1,
        "a restored canonical rule with operator update history must not be removed"
    );
    assert_eq!(
        sqlx::query_as::<_, (Uuid, String)>(
            r#"
            SELECT evidence_id, result
            FROM alert_policy_evidence_receipts
            WHERE evidence_id = ANY($1::uuid[])
            ORDER BY evidence_id
            "#,
        )
        .bind(vec![legacy_alert_id, affected_alert_id])
        .fetch_all(&db.pool)
        .await
        .unwrap(),
        vec![
            (legacy_alert_id, "unknown".to_string()),
            (affected_alert_id, "unknown".to_string()),
        ],
        "migrated Unknown history must have durable consumed evidence"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM alert_lifecycle_events")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        0,
        "the unified lifecycle migration must not synthesize outbox edges"
    );
    assert_eq!(
        sqlx::query_as::<_, (bool, bool)>(
            r#"
        SELECT
            to_regclass('public.policy_alerts') IS NULL,
            to_regclass('public.policy_rule_states') IS NULL
        "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (true, true)
    );

    db.cleanup().await;
    fs::remove_dir_all(&baseline_dir).unwrap();
}

#[tokio::test]
async fn postgres_exact_v035_baseline_applies_supported_migrations_in_place() {
    let base_url = match std::env::var("VPSMAN_TEST_POSTGRES_URL") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skipping Postgres migration test: VPSMAN_TEST_POSTGRES_URL is unset");
            return;
        }
    };
    let baseline_dir = std::env::temp_dir().join(format!(
        "vpsman-v035-migrations-{}",
        Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&baseline_dir).unwrap();
    let migrations_dir = workspace_migrations_dir();
    for (name, expected_sha256) in [
        (
            "0001_identity_access.sql",
            "884f950940275597749a575ab30780369ea2a5adb17889e1c29c5dd1b2fd1167",
        ),
        (
            "0002_jobs_schedules_commands.sql",
            "031f27cfbdce3b593dcfc144b122c7d2bff4d56e7a4c074fea373da9e0f04883",
        ),
        (
            "0003_telemetry_alerts_history.sql",
            "f1408e33815cb10b98b1061d1d0275874357ed7803218252796846572e7c7e3b",
        ),
        (
            "0004_backups_restores.sql",
            "120ebf2284a7035f7ad51f9989e79e871c93f9b5690a0681abcc0248165833e9",
        ),
        (
            "0005_network_tunnels.sql",
            "dc77d215b22080f9036b43afcc8a15f0bc6295bfab0a0884dd116271b0f35131",
        ),
        (
            "0006_agent_updates.sql",
            "150ec74e23db6fe98c6ba6de723c369ea219c8c12ab2295dda5e4ceea78a2158",
        ),
        (
            "0007_configuration_presets_file_transfer.sql",
            "6ff29337a5408b8a9f34536a53be4fa189c0d326251abb53bdbd4b489f99fe8c",
        ),
        (
            "0008_system_metrics.sql",
            "83fb85dd37b217e2f94995074c851fddbf852c3327b062ecc534d1058763e8a8",
        ),
    ] {
        let bytes = fs::read(migrations_dir.join(name)).unwrap();
        assert_eq!(payload_hash(&bytes), expected_sha256, "migration: {name}");
        fs::copy(migrations_dir.join(name), baseline_dir.join(name)).unwrap();
    }
    let db = PgReliabilityTestDb::new_with_migrations(&base_url, &baseline_dir)
        .await
        .expect("failed to create exact v0.3.5 baseline database");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM _sqlx_migrations WHERE success")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        8
    );
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT to_regclass('public.fleet_tag_settings') IS NULL",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap());

    let retained_tags = [
        (Uuid::new_v4(), "provider:Z", -2048_i64),
        (Uuid::new_v4(), "country:SG", 7168_i64),
    ];
    for (id, name, display_order) in &retained_tags {
        sqlx::query("INSERT INTO tags (id, name, display_order) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(name)
            .bind(display_order)
            .execute(&db.pool)
            .await
            .unwrap();
    }

    sqlx::migrate::Migrator::new(migrations_dir.as_path())
        .await
        .unwrap()
        .run(&db.pool)
        .await
        .expect("0009 through 0012 must apply after exact v0.3.5 migration checksums");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM _sqlx_migrations WHERE success")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        12
    );
    assert_eq!(
        sqlx::query_as::<_, (bool, bool, bool, bool, bool, bool)>(
            r#"
            SELECT
                to_regclass('public.policy_alerts') IS NULL,
                to_regclass('public.policy_rule_states') IS NULL,
                to_regclass('public.alert_episodes') IS NOT NULL,
                to_regclass('public.alert_policy_evaluation_states') IS NOT NULL,
                to_regclass('public.alert_policy_evidence') IS NOT NULL,
                to_regclass('public.alert_lifecycle_events') IS NOT NULL
            "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (true, true, true, true, true, true)
    );
    assert_eq!(
        sqlx::query_as::<_, (String, String, String, String)>(
            r#"
            SELECT trigger_condition_expression, rule_kind,
                   evidence_source, correlation_mode
            FROM policy_rules
            WHERE id = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb3'
            "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (
            "traffic.cycle_percent >= 80".to_string(),
            "metric".to_string(),
            "telemetry.combined".to_string(),
            "natural_key".to_string(),
        )
    );
    let setting: (serde_json::Value, Option<Uuid>) = sqlx::query_as(
        r#"
        SELECT value_json, updated_by
        FROM fleet_tag_settings
        WHERE setting_key = 'order.namespace_natural_sort_enabled'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(setting, (json!(false), None));
    let tags_after: Vec<(Uuid, String, i64)> =
        sqlx::query_as("SELECT id, name, display_order FROM tags ORDER BY name")
            .fetch_all(&db.pool)
            .await
            .unwrap();
    let mut expected_tags = retained_tags
        .into_iter()
        .map(|(id, name, display_order)| (id, name.to_string(), display_order))
        .collect::<Vec<_>>();
    expected_tags.sort_by(|left, right| left.1.cmp(&right.1));
    assert_eq!(tags_after, expected_tags);
    let order_state = db.repo.tag_order_state().await.unwrap();
    assert!(!order_state.namespace_natural_sort_enabled);
    assert_eq!(
        order_state
            .tags
            .iter()
            .map(|tag| (tag.name.as_str(), tag.display_order))
            .collect::<Vec<_>>(),
        [("provider:Z", -2048), ("country:SG", 7168)]
    );

    let future_value = json!({"shape": ["opaque", 1]});
    sqlx::query("INSERT INTO fleet_tag_settings (setting_key, value_json) VALUES ($1, $2)")
        .bind("future.order.mode")
        .bind(&future_value)
        .execute(&db.pool)
        .await
        .unwrap();
    let stored_future_value: serde_json::Value =
        sqlx::query_scalar("SELECT value_json FROM fleet_tag_settings WHERE setting_key = $1")
            .bind("future.order.mode")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(stored_future_value, future_value);

    db.cleanup().await;
    fs::remove_dir_all(&baseline_dir).unwrap();
}

#[tokio::test]
async fn postgres_fresh_schema_omits_obsolete_no_reset_epoch_indexes() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let indexes_absent: bool = sqlx::query_scalar(
        r#"
        SELECT
            to_regclass('public.traffic_counter_samples_rx_epoch_lookup_idx') IS NULL
            AND to_regclass('public.traffic_counter_samples_tx_epoch_lookup_idx') IS NULL
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(indexes_absent);

    db.cleanup().await;
}

impl PgReliabilityTestDb {
    async fn maybe_new() -> Option<Self> {
        let base_url = match std::env::var("VPSMAN_TEST_POSTGRES_URL") {
            Ok(value) if !value.trim().is_empty() => value,
            _ => {
                eprintln!("skipping Postgres reliability test: VPSMAN_TEST_POSTGRES_URL is unset");
                return None;
            }
        };
        Some(
            Self::new(&base_url)
                .await
                .expect("failed to create Postgres reliability test database"),
        )
    }

    async fn new(base_url: &str) -> anyhow::Result<Self> {
        Self::new_with_migrations(base_url, &workspace_migrations_dir()).await
    }

    async fn new_with_migrations(base_url: &str, migrations_dir: &Path) -> anyhow::Result<Self> {
        let base_options = PgConnectOptions::from_str(base_url)?;
        let admin_pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(base_options.clone().database("postgres"))
            .await?;
        let db_name = format!("vpsman_reliability_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE {}", quote_ident(&db_name)))
            .execute(&admin_pool)
            .await?;
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect_with(base_options.database(&db_name))
            .await?;
        let migrator = sqlx::migrate::Migrator::new(migrations_dir).await?;
        migrator.run(&pool).await?;
        let repo = Repository::Postgres(pool.clone());
        Ok(Self {
            repo,
            pool,
            admin_pool,
            db_name,
        })
    }

    async fn cleanup(self) {
        let Self {
            repo,
            pool,
            admin_pool,
            db_name,
        } = self;
        drop(repo);
        pool.close().await;
        let _ = sqlx::query(
            r#"
            SELECT pg_terminate_backend(pid)
            FROM pg_stat_activity
            WHERE datname = $1
              AND pid <> pg_backend_pid()
            "#,
        )
        .bind(&db_name)
        .execute(&admin_pool)
        .await;
        let _ = sqlx::query(&format!(
            "DROP DATABASE IF EXISTS {}",
            quote_ident(&db_name)
        ))
        .execute(&admin_pool)
        .await;
        admin_pool.close().await;
    }
}

fn quote_ident(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn workspace_migrations_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("migrations")
}

fn postgres_alert_test_tunnel_input() -> TunnelPlanInput {
    TunnelPlanInput {
        name: "postgres-alert-gre42".to_string(),
        interface_name: "gre42".to_string(),
        kind: TunnelKind::Gre,
        runtime_control: RuntimeTunnelControl {
            manager: RuntimeTunnelManager::CustomAdapter,
            left_adapter_definition_id: Some("11111111-1111-4111-8111-111111111111".to_string()),
            right_adapter_definition_id: Some("22222222-2222-4222-8222-222222222222".to_string()),
            ..Default::default()
        },
        runtime_topology: Default::default(),
        left_client_id: "alert-target-a".to_string(),
        right_client_id: "alert-target-b".to_string(),
        left_remote_underlay: "198.51.100.10".to_string(),
        right_remote_underlay: "203.0.113.20".to_string(),
        left_local_underlay: None,
        right_local_underlay: None,
        address_pool_cidr: "10.42.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(TunnelAddressPair {
            left: "10.42.0.0".to_string(),
            right: "10.42.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth_mbps: 100,
        left_mtu: None,
        right_mtu: None,
        ospf: None,
    }
}

fn postgres_app_state(db: &PgReliabilityTestDb) -> AppState {
    let (events, _) = crate::state::WsEventBus::new(16);
    AppState {
        repo: db.repo.clone(),
        events,
        internal_token: Some("gateway-secret-at-least-32-characters".to_string()),
        gateway: GatewayDispatchClient::default(),
        backup_object_store: None,
        update_release_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        artifact_max_bytes: DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: std::path::PathBuf::from("config/vpsman.toml"),
        dispatcher_config: DispatcherRuntimeConfig::default(),
    }
}

fn internal_gateway_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        "Bearer gateway-secret-at-least-32-characters"
            .parse()
            .unwrap(),
    );
    headers
}

async fn insert_client(pool: &PgPool, client_id: &str, incarnation: Option<Uuid>) {
    let public_key = hex::decode(payload_hash(client_id.as_bytes())).unwrap();
    sqlx::query(
        r#"
        INSERT INTO clients (
            id, display_name, public_key, status, internal_build_number,
            process_incarnation_id, capabilities
        )
        VALUES ($1, $1, $3, 'online', 1, $2, '{}'::jsonb)
        "#,
    )
    .bind(client_id)
    .bind(incarnation)
    .bind(public_key)
    .execute(pool)
    .await
    .unwrap();
}

fn postgres_metric_policy_rule_request(
    id: Option<Uuid>,
    name: &str,
    severity: &str,
) -> PolicyRuleRequest {
    PolicyRuleRequest {
        id,
        name: name.to_string(),
        enabled: true,
        rule_kind: AlertPolicyRuleKind::Metric,
        evidence_source: "telemetry.combined".to_string(),
        correlation_mode: AlertPolicyCorrelationMode::NaturalKey,
        traffic_selector: None,
        trigger_condition_expression: "cpu.load_1 >= 1".to_string(),
        trigger_meta_condition: None,
        resolve_condition_expression: None,
        resolve_meta_condition: None,
        severity: severity.to_string(),
        category: "resource".to_string(),
        title_template: "Resource threshold reached".to_string(),
        detail_template: "CPU load is above the configured threshold".to_string(),
    }
}

fn postgres_backup_failure_rule_request(id: Option<Uuid>, severity: &str) -> PolicyRuleRequest {
    PolicyRuleRequest {
        id,
        name: "backup execution failure".to_string(),
        enabled: true,
        rule_kind: AlertPolicyRuleKind::Occurrence,
        evidence_source: "backup.failure".to_string(),
        correlation_mode: AlertPolicyCorrelationMode::NaturalKey,
        traffic_selector: None,
        trigger_condition_expression: "evidence.status = execution_failed".to_string(),
        trigger_meta_condition: None,
        resolve_condition_expression: None,
        resolve_meta_condition: Some(AlertPolicyMetaCondition::ElapsedSinceTrigger {
            seconds: 3600,
        }),
        severity: severity.to_string(),
        category: "backup".to_string(),
        title_template: "Backup execution failed".to_string(),
        detail_template: "The backup request failed during execution".to_string(),
    }
}

async fn insert_raw_telemetry_fixture(
    pool: &PgPool,
    client_id: &str,
    observed_unix: u64,
    metrics: &AgentMetrics,
) -> Uuid {
    let sample_id = Uuid::new_v4();
    let disk_total = metrics
        .disks
        .iter()
        .fold(0_u128, |total, disk| {
            total.saturating_add(disk.total_bytes as u128)
        })
        .min(i64::MAX as u128) as i64;
    let disk_available = metrics
        .disks
        .iter()
        .fold(0_u128, |total, disk| {
            total.saturating_add(disk.available_bytes as u128)
        })
        .min(i64::MAX as u128) as i64;
    let network_rx = metrics
        .networks
        .iter()
        .fold(0_u128, |total, network| {
            total.saturating_add(network.rx_bytes as u128)
        })
        .min(i64::MAX as u128) as i64;
    let network_tx = metrics
        .networks
        .iter()
        .fold(0_u128, |total, network| {
            total.saturating_add(network.tx_bytes as u128)
        })
        .min(i64::MAX as u128) as i64;
    let to_i64 = |value: u64| value.min(i64::MAX as u64) as i64;
    sqlx::query(
        r#"
        INSERT INTO telemetry_samples (
            id, client_id, observed_at,
            cpu_utilization_ratio, cpu_cores, cpu_load_1, cpu_load_5, cpu_load_15,
            memory_total_bytes, memory_available_bytes,
            swap_total_bytes, swap_available_bytes,
            disk_total_bytes, disk_available_bytes,
            network_rx_bytes, network_tx_bytes,
            tcp_sockets, udp_sockets, payload
        ) VALUES (
            $1, $2, to_timestamp($3::double precision),
            $4, $5, $6, $7, $8,
            $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19
        )
        "#,
    )
    .bind(sample_id)
    .bind(client_id)
    .bind(observed_unix as f64)
    .bind(metrics.cpu.utilization_ratio)
    .bind(i32::from(metrics.cpu.cores))
    .bind(metrics.cpu.load.one)
    .bind(metrics.cpu.load.five)
    .bind(metrics.cpu.load.fifteen)
    .bind(to_i64(metrics.memory.total_bytes))
    .bind(to_i64(metrics.memory.available_bytes))
    .bind(metrics.memory.swap_total_bytes.map(to_i64))
    .bind(metrics.memory.swap_available_bytes.map(to_i64))
    .bind(disk_total)
    .bind(disk_available)
    .bind(network_rx)
    .bind(network_tx)
    .bind(
        metrics
            .connections
            .as_ref()
            .map(|connections| to_i64(connections.tcp))
            .unwrap_or(i64::MAX),
    )
    .bind(
        metrics
            .connections
            .as_ref()
            .map(|connections| to_i64(connections.udp))
            .unwrap_or(i64::MAX),
    )
    .bind(serde_json::json!(metrics))
    .execute(pool)
    .await
    .unwrap();

    for (ordinal, network) in metrics.networks.iter().enumerate() {
        sqlx::query(
            r#"
            INSERT INTO telemetry_counter_facts (
                sample_id, client_id, observed_at, source_kind,
                ordinal, interface, rx_bytes, tx_bytes
            ) VALUES (
                $1, $2, to_timestamp($3::double precision), 'host', $4, $5, $6, $7
            )
            "#,
        )
        .bind(sample_id)
        .bind(client_id)
        .bind(observed_unix as f64)
        .bind(ordinal as i32)
        .bind(&network.interface)
        .bind(to_i64(network.rx_bytes))
        .bind(to_i64(network.tx_bytes))
        .execute(pool)
        .await
        .unwrap();
    }
    for (ordinal, tunnel) in metrics.tunnels.iter().enumerate() {
        sqlx::query(
            r#"
            INSERT INTO telemetry_counter_facts (
                sample_id, client_id, observed_at, source_kind,
                ordinal, interface, rx_bytes, tx_bytes
            ) VALUES (
                $1, $2, to_timestamp($3::double precision), 'tunnel', $4, $5, $6, $7
            )
            "#,
        )
        .bind(sample_id)
        .bind(client_id)
        .bind(observed_unix as f64)
        .bind(ordinal as i32)
        .bind(&tunnel.interface)
        .bind(to_i64(tunnel.rx_bytes))
        .bind(to_i64(tunnel.tx_bytes))
        .execute(pool)
        .await
        .unwrap();
    }
    for ping in &metrics.ping_results {
        sqlx::query(
            r#"
            WITH series AS (
                INSERT INTO telemetry_ping_series (client_id, target_id, generation)
                VALUES ($1, $2, $3)
                ON CONFLICT (client_id, target_id, generation) DO UPDATE
                    SET generation = EXCLUDED.generation
                RETURNING id
            )
            INSERT INTO telemetry_ping_facts (
                series_id, observed_at, evidence_id, source_checked_unix, checked_unix,
                status, latency_avg_ms, loss_ratio, reason
            )
            SELECT id, to_timestamp($4::double precision), $5, $6, $6, $7, $8, $9, $10
            FROM series
            WHERE $6 <= floor($4::double precision)::bigint + 300
              AND floor($4::double precision)::bigint - $6 <= 3900
            ON CONFLICT (series_id, source_checked_unix) DO UPDATE SET
                observed_at = EXCLUDED.observed_at,
                evidence_id = EXCLUDED.evidence_id,
                status = EXCLUDED.status,
                latency_avg_ms = EXCLUDED.latency_avg_ms,
                loss_ratio = EXCLUDED.loss_ratio,
                reason = EXCLUDED.reason
            "#,
        )
        .bind(client_id)
        .bind(Uuid::parse_str(&ping.target_id).unwrap())
        .bind(ping.generation as i64)
        .bind(observed_unix as f64)
        .bind(sample_id)
        .bind(ping.checked_unix as i64)
        .bind(&ping.status)
        .bind(ping.latency_avg_ms)
        .bind(ping.loss_ratio)
        .bind(&ping.reason)
        .execute(pool)
        .await
        .unwrap();
    }
    sample_id
}

async fn start_test_gateway_session(
    repo: &Repository,
    gateway_id: &str,
    client_id: &str,
    session_id: Uuid,
) {
    repo.record_gateway_session_started(&vpsman_common::GatewaySessionLifecycleIngest {
        gateway_id: gateway_id.to_string(),
        client_id: client_id.to_string(),
        session_id,
        noise_public_key_hex: None,
        remote_ip: None,
        agent_version: Some("postgres-test".to_string()),
        reason: None,
    })
    .await
    .unwrap();
}

async fn install_rejected_audit_action_trigger(pool: &PgPool) {
    sqlx::query("CREATE TABLE rejected_test_audit_actions (action TEXT PRIMARY KEY)")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        r#"
        CREATE FUNCTION reject_test_audit_action() RETURNS trigger
        LANGUAGE plpgsql AS $$
        BEGIN
            IF EXISTS (
                SELECT 1
                FROM rejected_test_audit_actions rejected
                WHERE rejected.action = NEW.action
            ) THEN
                RAISE EXCEPTION 'forced audit failure for %', NEW.action;
            END IF;
            RETURN NEW;
        END
        $$
        "#,
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        CREATE TRIGGER reject_test_audit_action
        BEFORE INSERT ON audit_logs
        FOR EACH ROW EXECUTE FUNCTION reject_test_audit_action()
        "#,
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn set_rejected_audit_action(pool: &PgPool, action: &str) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("DELETE FROM rejected_test_audit_actions")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO rejected_test_audit_actions (action) VALUES ($1)")
        .bind(action)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn install_invalid_job_operation_audit_rejection_trigger(pool: &PgPool) {
    sqlx::query(
        r#"
        CREATE FUNCTION reject_invalid_job_operation_audit() RETURNS trigger
        LANGUAGE plpgsql AS $$
        BEGIN
            IF NEW.action = 'job.target_result'
               AND NEW.metadata->>'reason' = 'invalid_job_operation'
            THEN
                RAISE EXCEPTION 'forced invalid job operation audit failure';
            END IF;
            RETURN NEW;
        END
        $$
        "#,
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        CREATE TRIGGER reject_invalid_job_operation_audit
        BEFORE INSERT ON audit_logs
        FOR EACH ROW EXECUTE FUNCTION reject_invalid_job_operation_audit()
        "#,
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn remove_invalid_job_operation_audit_rejection_trigger(pool: &PgPool) {
    sqlx::query("DROP TRIGGER reject_invalid_job_operation_audit ON audit_logs")
        .execute(pool)
        .await
        .unwrap();
}

fn assert_forced_audit_failure<T>(result: anyhow::Result<T>) {
    let error = match result {
        Ok(_) => panic!("audit rejection must fail the mutation"),
        Err(error) => error,
    };
    assert!(
        format!("{error:#}").contains("forced audit failure"),
        "unexpected error: {error:#}"
    );
}

#[tokio::test]
async fn postgres_terminal_merge_preserves_terminal_state_nulls_and_true_open_time() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "terminal-merge-client";
    let session_id = Uuid::new_v4();
    let open_job = Uuid::new_v4();
    insert_client(&db.pool, client_id, None).await;
    insert_job_target(&db.pool, open_job, client_id, "running", true, None).await;

    let terminal_view =
        |state: &str, last_status: &str, last_event: &str, observed_at: &str, opened_at: &str| {
            TerminalSessionView {
                session_id,
                client_id: client_id.to_string(),
                job_id: open_job,
                state: state.to_string(),
                last_status: last_status.to_string(),
                argv: vec!["/bin/sh".to_string()],
                cwd: None,
                cols: None,
                rows: None,
                idle_timeout_secs: None,
                flow_window_bytes: None,
                output_first_seq: None,
                output_next_seq: None,
                output_retained_first_seq: None,
                output_retained_bytes: None,
                output_dropped_bytes: None,
                output_dropped_chunks: None,
                output_replay_truncated: false,
                last_input_seq: 0,
                close_reason: (state == "closed").then(|| "operator".to_string()),
                last_event: last_event.to_string(),
                opened_at: Some(opened_at.to_string()),
                observed_at: observed_at.to_string(),
            }
        };

    upsert_postgres_terminal_session(
        &db.pool,
        &terminal_view(
            "open",
            "opened",
            "terminal_open",
            "1970-01-01T00:03:20Z",
            "1970-01-01T00:03:20Z",
        ),
    )
    .await
    .unwrap();
    upsert_postgres_terminal_session(
        &db.pool,
        &terminal_view(
            "closed",
            "closed",
            "terminal_close",
            "1970-01-01T00:01:40Z",
            "1970-01-01T00:00:50Z",
        ),
    )
    .await
    .unwrap();
    upsert_postgres_terminal_session(
        &db.pool,
        &terminal_view(
            "open",
            "streaming",
            "terminal_stream",
            "1970-01-01T00:05:00Z",
            "1970-01-01T00:03:20Z",
        ),
    )
    .await
    .unwrap();
    upsert_postgres_terminal_session(
        &db.pool,
        &terminal_view(
            "open",
            "opened",
            "terminal_open",
            "1970-01-01T00:00:25Z",
            "1970-01-01T00:00:25Z",
        ),
    )
    .await
    .unwrap();

    let conflicting_job = Uuid::new_v4();
    insert_job_target(&db.pool, conflicting_job, client_id, "running", true, None).await;
    let mut conflicting = terminal_view(
        "open",
        "opened",
        "terminal_open",
        "1970-01-01T00:06:40Z",
        "1970-01-01T00:06:40Z",
    );
    conflicting.job_id = conflicting_job;
    let conflict = upsert_postgres_terminal_session(&db.pool, &conflicting)
        .await
        .unwrap_err();
    assert_eq!(conflict.to_string(), "terminal_session_job_conflict");

    let row = sqlx::query(
        r#"
        SELECT
            state,
            output_next_seq,
            EXTRACT(EPOCH FROM opened_at)::bigint AS opened_at_unix,
            EXTRACT(EPOCH FROM observed_at)::bigint AS observed_at_unix,
            last_event,
            job_id
        FROM terminal_sessions
        WHERE client_id = $1 AND session_id = $2
        "#,
    )
    .bind(client_id)
    .bind(session_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(row.try_get::<String, _>("state").unwrap(), "closed");
    assert_eq!(
        row.try_get::<Option<i64>, _>("output_next_seq").unwrap(),
        None
    );
    assert_eq!(row.try_get::<i64, _>("opened_at_unix").unwrap(), 50);
    assert_eq!(row.try_get::<i64, _>("observed_at_unix").unwrap(), 100);
    assert_eq!(
        row.try_get::<String, _>("last_event").unwrap(),
        "terminal_close"
    );
    assert_eq!(row.try_get::<Uuid, _>("job_id").unwrap(), open_job);

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_tunnel_plan_conflict_checks_are_concurrency_safe() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "client-a", None).await;
    insert_client(&db.pool, "client-b", None).await;
    let operator = postgres_network_operator(&db.repo).await;

    let mut first_input =
        crate::tests_network::test_plan_input(RuntimeTunnelManager::AgentBuiltin, false);
    first_input.name = "concurrent-interface-a".to_string();
    first_input.address_pool_cidr = "10.96.0.0/29".to_string();
    first_input.ipv4_tunnel = Some(TunnelAddressPair {
        left: "10.96.0.0".to_string(),
        right: "10.96.0.1".to_string(),
        prefix_len: 31,
    });
    let mut second_input = first_input.clone();
    second_input.name = "concurrent-interface-b".to_string();
    second_input.address_pool_cidr = "10.96.0.0/29".to_string();
    second_input.ipv4_tunnel = Some(TunnelAddressPair {
        left: "10.96.0.2".to_string(),
        right: "10.96.0.3".to_string(),
        prefix_len: 31,
    });
    let first_plan = plan_tunnel(&first_input).unwrap();
    let second_plan = plan_tunnel(&second_input).unwrap();
    let (first, second) = tokio::join!(
        db.repo
            .record_tunnel_plan(&first_input, &first_plan, false, &operator),
        db.repo
            .record_tunnel_plan(&second_input, &second_plan, false, &operator),
    );
    match (first, second) {
        (Ok(_), Err(error)) | (Err(error), Ok(_)) => {
            assert_eq!(error.to_string(), "tunnel_plan_interface_conflict");
        }
        (first, second) => panic!("expected one interface conflict, got {first:?} and {second:?}"),
    }

    let mut third_input = first_input.clone();
    third_input.name = "concurrent-address-a".to_string();
    third_input.interface_name = "addr-a".to_string();
    third_input.address_pool_cidr = "10.97.0.0/29".to_string();
    third_input.ipv4_tunnel = Some(TunnelAddressPair {
        left: "10.97.0.0".to_string(),
        right: "10.97.0.1".to_string(),
        prefix_len: 31,
    });
    let mut fourth_input = third_input.clone();
    fourth_input.name = "concurrent-address-b".to_string();
    fourth_input.interface_name = "addr-b".to_string();
    let third_plan = plan_tunnel(&third_input).unwrap();
    let fourth_plan = plan_tunnel(&fourth_input).unwrap();
    let (third, fourth) = tokio::join!(
        db.repo
            .record_tunnel_plan(&third_input, &third_plan, false, &operator),
        db.repo
            .record_tunnel_plan(&fourth_input, &fourth_plan, false, &operator),
    );
    match (third, fourth) {
        (Ok(_), Err(error)) | (Err(error), Ok(_)) => {
            assert_eq!(error.to_string(), "tunnel_plan_address_conflict");
        }
        (third, fourth) => panic!("expected one address conflict, got {third:?} and {fourth:?}"),
    }

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_tunnel_plan_update_locks_endpoints_before_plan_row() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "client-a", None).await;
    insert_client(&db.pool, "client-b", None).await;
    let operator = postgres_network_operator(&db.repo).await;

    let input = crate::tests_network::test_plan_input(RuntimeTunnelManager::AgentBuiltin, false);
    let plan = plan_tunnel(&input).unwrap();
    let saved = db
        .repo
        .record_tunnel_plan(&input, &plan, false, &operator)
        .await
        .unwrap();
    let mut updated_input = input;
    updated_input.bandwidth_mbps += 1;
    let updated_plan = plan_tunnel(&updated_input).unwrap();

    let mut lifecycle_blocker = db.pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('vpsman.agent_key_lifecycle'))")
        .execute(&mut *lifecycle_blocker)
        .await
        .unwrap();

    let update_repo = db.repo.clone();
    let update_operator = operator.clone();
    let update_task = tokio::spawn(async move {
        update_repo
            .update_tunnel_plan(
                saved.id,
                saved.revision,
                &updated_input,
                &updated_plan,
                false,
                &update_operator,
            )
            .await
    });

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting_for_lifecycle_lock: bool = sqlx::query_scalar(
                r#"
                SELECT EXISTS (
                    SELECT 1
                    FROM pg_stat_activity
                    WHERE datname = current_database()
                      AND pid <> pg_backend_pid()
                      AND state = 'active'
                      AND wait_event_type = 'Lock'
                      AND query LIKE '%vpsman.agent_key_lifecycle%'
                )
                "#,
            )
            .fetch_one(&db.pool)
            .await
            .unwrap();
            if waiting_for_lifecycle_lock {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("tunnel plan update should wait for the endpoint lifecycle lock");

    let mut row_probe = db.pool.begin().await.unwrap();
    let locked_plan_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM tunnel_plans WHERE id = $1 FOR UPDATE NOWAIT",
    )
    .bind(saved.id)
    .fetch_one(&mut *row_probe)
    .await
    .expect("waiting update must not hold the tunnel plan row lock");
    assert_eq!(locked_plan_id, saved.id);
    row_probe.rollback().await.unwrap();

    lifecycle_blocker.rollback().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), update_task)
        .await
        .expect("tunnel plan update should finish after the lifecycle lock is released")
        .expect("tunnel plan update task should not panic")
        .expect("tunnel plan update should succeed");

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_agent_delete_returns_retired_peers_and_rejects_hidden_endpoint_reuse() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "client-a", None).await;
    insert_client(&db.pool, "client-b", None).await;
    let operator = postgres_network_operator(&db.repo).await;
    db.repo
        .initialize_system_configuration_presets()
        .await
        .unwrap();
    let preset = db
        .repo
        .create_configuration_preset(
            &CreateConfigurationPresetRequest {
                behavior: "process_inventory".to_string(),
                name: "Retired endpoint processes".to_string(),
                description: None,
                definition: serde_json::json!({
                    "source": "linux_procfs",
                    "proc_root": "/host/proc"
                }),
            },
            &operator,
        )
        .await
        .unwrap();
    let override_preview = db
        .repo
        .preview_configuration_source_override(&PreviewConfigurationSourceOverrideRequest {
            action: ConfigurationOverrideAction::Set,
            behavior: "process_inventory".to_string(),
            preset_id: Some(preset.id),
            selector_expression: String::new(),
            target_client_ids: vec!["client-a".to_string()],
        })
        .await
        .unwrap();
    let ping_target_id = Uuid::new_v4();
    let now = crate::unix_now().to_string();
    db.repo
        .upsert_ping_target(
            PingTargetRecord {
                id: ping_target_id,
                name: "Retired endpoint Ping".to_string(),
                host: "1.1.1.1".to_string(),
                probe_kind: "icmp".to_string(),
                port: None,
                enabled: true,
                selector_expression: "id:client-a".to_string(),
                generation: 1,
                created_by: Some(operator.operator.id),
                created_at: now.clone(),
                updated_at: now,
            },
            &["client-a".to_string()],
            None,
            &operator,
            "ping_target.created",
        )
        .await
        .unwrap();
    db.repo
        .apply_configuration_source_override(&override_preview, &operator)
        .await
        .unwrap();
    let input = crate::tests_network::test_plan_input(RuntimeTunnelManager::AgentBuiltin, false);
    let plan = plan_tunnel(&input).unwrap();
    db.repo
        .record_tunnel_plan(&input, &plan, true, &operator)
        .await
        .unwrap();

    let deleted = db
        .repo
        .delete_agent("client-a", Some("retire endpoint"), &operator)
        .await
        .unwrap();

    let visible_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM visible_clients WHERE id = 'client-a'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let tombstone_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM clients WHERE id = 'client-a'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(visible_count, 0);
    assert_eq!(tombstone_count, 1);

    assert_eq!(
        deleted.retired_tunnel_endpoint_pairs,
        vec![("client-a".to_string(), "client-b".to_string())]
    );
    assert!(db.repo.list_tunnel_plans().await.unwrap().is_empty());
    let released_preset = db
        .repo
        .list_configuration_presets(Some("process_inventory"))
        .await
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.id == preset.id)
        .unwrap();
    assert_eq!(released_preset.override_vps_count, 0);
    assert_eq!(released_preset.effective_vps_count, 0);
    assert!(db
        .repo
        .apply_configuration_source_override(&override_preview, &operator)
        .await
        .unwrap_err()
        .to_string()
        .contains("configuration_source_override_preview_stale"));
    let remaining_override_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM client_configuration_preset_overrides WHERE client_id = 'client-a'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(remaining_override_count, 1);
    let preset_update_preview = db
        .repo
        .preview_configuration_preset_update(
            preset.id,
            &PreviewConfigurationPresetRequest {
                description: Some("Updated after endpoint retirement".to_string()),
                definition: serde_json::json!({
                    "source": "linux_procfs",
                    "proc_root": "/srv/proc"
                }),
            },
        )
        .await
        .unwrap();
    assert!(preset_update_preview.affected_client_ids.is_empty());
    let updated_preset = db
        .repo
        .update_configuration_preset(preset.id, &preset_update_preview, &operator)
        .await
        .unwrap();
    assert_eq!(updated_preset.override_vps_count, 0);
    assert!(db
        .repo
        .mutate_ping_targets_bulk(&[ping_target_id], "disable", &operator)
        .await
        .unwrap()
        .is_empty());
    assert!(db
        .repo
        .mutate_ping_targets_bulk(&[ping_target_id], "enable", &operator)
        .await
        .unwrap()
        .is_empty());
    assert!(db
        .repo
        .delete_ping_target(ping_target_id, &operator)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        db.repo
            .record_tunnel_plan(&input, &plan, true, &operator)
            .await
            .unwrap_err()
            .to_string(),
        "tunnel_plan_endpoint_agent_not_found"
    );
    db.repo
        .delete_configuration_preset(preset.id, &operator)
        .await
        .unwrap();
    let archived_override_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM client_configuration_preset_overrides WHERE client_id = 'client-a'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(archived_override_count, 0);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_key_revocation_remains_visible_and_preserves_configuration_override() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "client-revoke", None).await;
    insert_client(&db.pool, "client-revoke-recovery", None).await;
    sqlx::query("UPDATE clients SET public_key = decode($2, 'hex') WHERE id = $1")
        .bind("client-revoke")
        .bind("42".repeat(32))
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE clients SET public_key = decode($2, 'hex') WHERE id = $1")
        .bind("client-revoke-recovery")
        .bind("43".repeat(32))
        .execute(&db.pool)
        .await
        .unwrap();
    let operator = postgres_network_operator(&db.repo).await;
    db.repo
        .initialize_system_configuration_presets()
        .await
        .unwrap();
    let preset = db
        .repo
        .create_configuration_preset(
            &CreateConfigurationPresetRequest {
                behavior: "process_inventory".to_string(),
                name: "Revoked endpoint processes".to_string(),
                description: None,
                definition: serde_json::json!({
                    "source": "linux_procfs",
                    "proc_root": "/host/proc"
                }),
            },
            &operator,
        )
        .await
        .unwrap();
    let preview = db
        .repo
        .preview_configuration_source_override(&PreviewConfigurationSourceOverrideRequest {
            action: ConfigurationOverrideAction::Set,
            behavior: "process_inventory".to_string(),
            preset_id: Some(preset.id),
            selector_expression: String::new(),
            target_client_ids: vec!["client-revoke".to_string()],
        })
        .await
        .unwrap();
    db.repo
        .apply_configuration_source_override(&preview, &operator)
        .await
        .unwrap();

    db.repo
        .revoke_current_client_key("client-revoke", Some("compromised"), &operator)
        .await
        .unwrap();

    let remaining_override_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM client_configuration_preset_overrides WHERE client_id = 'client-revoke'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(remaining_override_count, 1);
    let status: String =
        sqlx::query_scalar("SELECT status FROM visible_clients WHERE id = 'client-revoke'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(status, "revoked");

    let recovery_preview = db
        .repo
        .preview_configuration_source_override(&PreviewConfigurationSourceOverrideRequest {
            action: ConfigurationOverrideAction::Set,
            behavior: "process_inventory".to_string(),
            preset_id: Some(preset.id),
            selector_expression: String::new(),
            target_client_ids: vec!["client-revoke-recovery".to_string()],
        })
        .await
        .unwrap();
    db.repo
        .apply_configuration_source_override(&recovery_preview, &operator)
        .await
        .unwrap();
    let recovery_public_key = vec![0x43; 32];
    sqlx::query(
        r#"
        INSERT INTO client_key_revocations (
            id, client_id, public_key_sha256_hex, reason, revoked_by
        )
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind("client-revoke-recovery")
    .bind(crate::repository_key_lifecycle::public_key_sha256_hex(
        &recovery_public_key,
    ))
    .bind("existing record")
    .bind(operator.operator.id)
    .execute(&db.pool)
    .await
    .unwrap();
    db.repo
        .revoke_current_client_key("client-revoke-recovery", Some("retry"), &operator)
        .await
        .unwrap();
    let recovery_override_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM client_configuration_preset_overrides WHERE client_id = 'client-revoke-recovery'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(recovery_override_count, 1);
    let recovery_status: String = sqlx::query_scalar(
        "SELECT status FROM visible_clients WHERE id = 'client-revoke-recovery'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(recovery_status, "revoked");
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_tunnel_underlay_and_operator_assessment_round_trip_without_conflation() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "client-a", None).await;
    insert_client(&db.pool, "client-b", None).await;
    let operator = postgres_network_operator(&db.repo).await;
    let mut input =
        crate::tests_network::test_plan_input(RuntimeTunnelManager::AgentBuiltin, false);
    input.left_remote_underlay = "203.0.113.20".to_string();
    input.left_local_underlay = Some("10.0.0.10".to_string());
    input.right_remote_underlay = "198.51.100.10".to_string();
    input.right_local_underlay = Some("10.0.1.20".to_string());
    let plan = plan_tunnel(&input).unwrap();
    let saved = db
        .repo
        .record_tunnel_plan(&input, &plan, true, &operator)
        .await
        .unwrap();

    let assessed = db
        .repo
        .update_tunnel_connection_assessment(
            saved.id,
            saved.revision,
            "connected",
            Some("Application traffic verified across NAT"),
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(assessed.plan.left_remote_underlay, "203.0.113.20");
    assert_eq!(
        assessed.plan.left_local_underlay.as_deref(),
        Some("10.0.0.10")
    );
    assert_eq!(assessed.plan.right_remote_underlay, "198.51.100.10");
    assert_eq!(
        assessed.plan.right_local_underlay.as_deref(),
        Some("10.0.1.20")
    );
    assert_eq!(assessed.connection_assessment, "connected");
    assert_eq!(
        assessed.connection_assessment_note.as_deref(),
        Some("Application traffic verified across NAT")
    );
    assert_eq!(assessed.connection_assessed_by, Some(operator.operator.id));

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_port_forward_hostname_context_round_trips_with_literal_target() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "edge-domain", None).await;
    let operator = postgres_network_operator(&db.repo).await;
    let created = db
        .repo
        .create_port_forward_rule(
            &CreatePortForwardRuleRequest {
                client_id: "edge-domain".to_string(),
                name: "resolved-web".to_string(),
                protocol: PortForwardProtocol::Tcp,
                target_ip: "192.0.2.40".parse().unwrap(),
                target_hostname: Some(" App.Internal. ".to_string()),
                mappings: pair_port_expressions("18443", "8443").unwrap(),
                masquerade: true,
                enabled: true,
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(created.target_hostname.as_deref(), Some("app.internal"));

    let row = sqlx::query(
        "SELECT host(target_ip) AS target_ip, target_hostname FROM port_forward_rules WHERE id = $1",
    )
    .bind(created.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(row.try_get::<String, _>("target_ip").unwrap(), "192.0.2.40");
    assert_eq!(
        row.try_get::<Option<String>, _>("target_hostname")
            .unwrap()
            .as_deref(),
        Some("app.internal")
    );

    let disabled = db
        .repo
        .set_port_forward_rule_enabled(created.id, created.revision, false, &operator)
        .await
        .unwrap();
    assert_eq!(disabled.target_hostname.as_deref(), Some("app.internal"));
    let listed = db
        .repo
        .get_port_forward_rule(created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(listed.target_hostname.as_deref(), Some("app.internal"));
    let audit_hostname: Option<String> = sqlx::query_scalar(
        r#"
        SELECT metadata ->> 'target_hostname'
        FROM audit_logs
        WHERE target = $1
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(format!("port_forward_rule:{}", created.id))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(audit_hostname.as_deref(), Some("app.internal"));

    let cleared = db
        .repo
        .update_port_forward_rule(
            created.id,
            &UpdatePortForwardRuleRequest {
                expected_revision: disabled.revision,
                name: disabled.name.clone(),
                protocol: disabled.protocol,
                target_ip: disabled.target_ip,
                target_hostname: UpdateTargetHostname::Clear,
                mappings: disabled.mappings.clone(),
                masquerade: disabled.masquerade,
                enabled: false,
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(cleared.target_hostname, None);
    let persisted_hostname: Option<String> =
        sqlx::query_scalar("SELECT target_hostname FROM port_forward_rules WHERE id = $1")
            .bind(created.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(persisted_hostname, None);

    let invalid_literal =
        sqlx::query("UPDATE port_forward_rules SET target_hostname = '192.0.2.40' WHERE id = $1")
            .bind(created.id)
            .execute(&db.pool)
            .await
            .unwrap_err();
    assert!(invalid_literal
        .to_string()
        .contains("port_forward_rules_target_hostname_check"));

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_network_json_corruption_is_visible_isolated_and_replaceable() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "client-a", None).await;
    insert_client(&db.pool, "client-b", None).await;
    insert_client(&db.pool, "edge-a", None).await;
    let operator = postgres_network_operator(&db.repo).await;

    let healthy_input =
        crate::tests_network::test_plan_input(RuntimeTunnelManager::AgentBuiltin, false);
    let healthy_plan = plan_tunnel(&healthy_input).unwrap();
    db.repo
        .record_tunnel_plan(&healthy_input, &healthy_plan, true, &operator)
        .await
        .unwrap();
    let mut repair_input = healthy_input.clone();
    repair_input.name = "repair-corrupt-tunnel".to_string();
    repair_input.interface_name = "vpsman-repair".to_string();
    repair_input.address_pool_cidr = "10.11.0.0/29".to_string();
    repair_input.ipv4_tunnel = Some(TunnelAddressPair {
        left: "10.11.0.0".to_string(),
        right: "10.11.0.1".to_string(),
        prefix_len: 31,
    });
    let repair_plan = plan_tunnel(&repair_input).unwrap();
    let corrupt_tunnel = db
        .repo
        .record_tunnel_plan(&repair_input, &repair_plan, true, &operator)
        .await
        .unwrap();
    sqlx::query("UPDATE tunnel_plans SET plan = $2 WHERE id = $1")
        .bind(corrupt_tunnel.id)
        .bind(sqlx::types::Json(serde_json::json!({"name": 42})))
        .execute(&db.pool)
        .await
        .unwrap();

    let tunnel_items = db.repo.list_tunnel_plan_items().await.unwrap();
    assert_eq!(tunnel_items.len(), 2);
    assert!(tunnel_items.iter().any(|item| matches!(
        item,
        crate::model::TunnelPlanListItem::Corrupt(corrupt)
            if corrupt.id == corrupt_tunnel.id
                && corrupt.configuration_error.contains("invalid")
    )));
    assert_eq!(db.repo.list_tunnel_plans().await.unwrap().len(), 1);
    db.repo
        .update_tunnel_plan(
            corrupt_tunnel.id,
            corrupt_tunnel.revision,
            &repair_input,
            &repair_plan,
            true,
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(db.repo.list_tunnel_plans().await.unwrap().len(), 2);

    let mappings_a = pair_port_expressions("8080", "80").unwrap();
    let mappings_b = pair_port_expressions("8081", "81").unwrap();
    let healthy_rule = db
        .repo
        .create_port_forward_rule(
            &CreatePortForwardRuleRequest {
                client_id: "edge-a".to_string(),
                name: "healthy-web".to_string(),
                protocol: PortForwardProtocol::Tcp,
                target_ip: "192.0.2.10".parse().unwrap(),
                target_hostname: None,
                mappings: mappings_a,
                masquerade: true,
                enabled: true,
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    let corrupt_rule = db
        .repo
        .create_port_forward_rule(
            &CreatePortForwardRuleRequest {
                client_id: "edge-a".to_string(),
                name: "repair-web".to_string(),
                protocol: PortForwardProtocol::Tcp,
                target_ip: "192.0.2.11".parse().unwrap(),
                target_hostname: None,
                mappings: mappings_b.clone(),
                masquerade: true,
                enabled: true,
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE port_forward_rules SET mappings = $2 WHERE id = $1")
        .bind(corrupt_rule.id)
        .bind(sqlx::types::Json(serde_json::json!([{"broken": true}])))
        .execute(&db.pool)
        .await
        .unwrap();

    let rule_items = db.repo.list_port_forward_rule_items().await.unwrap();
    assert_eq!(rule_items.len(), 2);
    assert!(rule_items.iter().any(|item| matches!(
        item,
        crate::model_port_forwarding::PortForwardRuleListItem::Corrupt(corrupt)
            if corrupt.id == corrupt_rule.id
                && corrupt.configuration_error.contains("invalid")
    )));
    assert_eq!(db.repo.list_port_forward_rules().await.unwrap().len(), 1);
    assert!(db
        .repo
        .port_forwarding_config_for_client("edge-a")
        .await
        .unwrap_err()
        .to_string()
        .contains("port_forward_rule_configuration_corrupt"));

    db.repo
        .update_port_forward_rule(
            corrupt_rule.id,
            &UpdatePortForwardRuleRequest {
                expected_revision: corrupt_rule.revision,
                name: "repair-web".to_string(),
                protocol: PortForwardProtocol::Tcp,
                target_ip: "192.0.2.11".parse().unwrap(),
                target_hostname: UpdateTargetHostname::Preserve,
                mappings: mappings_b,
                masquerade: true,
                enabled: true,
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(
        db.repo
            .port_forwarding_config_for_client("edge-a")
            .await
            .unwrap()
            .rules
            .len(),
        2
    );

    sqlx::query(
        r#"
        INSERT INTO port_forward_runtime_state (client_id, snapshot, observed_at)
        VALUES ('edge-a', $1, now())
        "#,
    )
    .bind(sqlx::types::Json(
        serde_json::json!({"status": "not-a-runtime-status"}),
    ))
    .execute(&db.pool)
    .await
    .unwrap();
    let rules = db.repo.list_port_forward_rules().await.unwrap();
    assert_eq!(rules.len(), 2);
    assert!(rules.iter().all(|rule| {
        rule.runtime_status == "failed"
            && rule.runtime_error_code.as_deref() == Some("port_forward_runtime_snapshot_corrupt")
    }));
    db.repo
        .record_port_forward_runtime_snapshot(
            "edge-a",
            &PortForwardRuntimeSnapshot {
                status: PortForwardRuntimeStatus::Unknown,
                ..PortForwardRuntimeSnapshot::default()
            },
        )
        .await
        .unwrap();
    assert!(db
        .repo
        .list_port_forward_rules()
        .await
        .unwrap()
        .iter()
        .all(|rule| rule.runtime_error_code.as_deref()
            != Some("port_forward_runtime_snapshot_corrupt")));
    assert!(db
        .repo
        .get_port_forward_rule(healthy_rule.id)
        .await
        .unwrap()
        .is_some());

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_schema_enforces_global_agent_key_ownership() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let key = vec![0x42_u8; 32];
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1, $1, $2, 'never')",
    )
    .bind("key-owner-a")
    .bind(&key)
    .execute(&db.pool)
    .await
    .unwrap();

    let duplicate = sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1, $1, $2, 'never')",
    )
    .bind("key-owner-b")
    .bind(&key)
    .execute(&db.pool)
    .await
    .unwrap_err();
    assert!(duplicate
        .to_string()
        .contains("clients_public_key_unique_idx"));

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_agent_hello_cannot_restore_a_rotated_key() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "hello-rotation-race";
    let old_key = vec![0x51_u8; 32];
    let new_key = vec![0x52_u8; 32];
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1, $1, $2, 'never')",
    )
    .bind(client_id)
    .bind(&old_key)
    .execute(&db.pool)
    .await
    .unwrap();

    let mut stale_hello = hello_event(client_id, Uuid::new_v4(), None);
    stale_hello.noise_public_key_hex = hex::encode(&old_key);
    stale_hello.hello.cpu_model = Some("AMD EPYC".to_string());
    stale_hello.hello.kernel_release = Some("6.12.1".to_string());
    stale_hello.hello.virtualization = Some("kvm".to_string());
    assert!(db.repo.upsert_agent_hello(&stale_hello).await.unwrap());
    let system = sqlx::query(
        r#"
        SELECT cpu_model, kernel_release, virtualization,
               system_reported_at IS NOT NULL AS reported
        FROM clients
        WHERE id = $1
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        system.try_get::<String, _>("cpu_model").unwrap(),
        "AMD EPYC"
    );
    assert_eq!(
        system.try_get::<String, _>("kernel_release").unwrap(),
        "6.12.1"
    );
    assert_eq!(
        system.try_get::<String, _>("virtualization").unwrap(),
        "kvm"
    );
    assert!(system.try_get::<bool, _>("reported").unwrap());

    let mut tx = db.pool.begin().await.unwrap();
    sqlx::query(
        r#"
        INSERT INTO client_key_revocations (
            id, client_id, public_key_sha256_hex, reason
        )
        VALUES ($1, $2, $3, 'client_key_replaced')
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(client_id)
    .bind(crate::repository_key_lifecycle::public_key_sha256_hex(
        &old_key,
    ))
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE clients SET public_key = $2, status = 'offline', process_incarnation_id = NULL WHERE id = $1",
    )
    .bind(client_id)
    .bind(&new_key)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    assert!(!db.repo.upsert_agent_hello(&stale_hello).await.unwrap());
    let row = sqlx::query("SELECT public_key, status FROM clients WHERE id = $1")
        .bind(client_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(row.try_get::<Vec<u8>, _>("public_key").unwrap(), new_key);
    assert_eq!(row.try_get::<String, _>("status").unwrap(), "offline");

    db.cleanup().await;
}

async fn insert_job_target(
    pool: &PgPool,
    job_id: Uuid,
    client_id: &str,
    status: &str,
    started: bool,
    target_incarnation: Option<Uuid>,
) {
    let operation = JobCommand::Shell {
        argv: vec!["true".to_string()],
        pty: false,
    };
    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, command_type, status, target_count, payload_hash, operation,
            request_fingerprint, max_timeout_secs
        )
        VALUES ($1, 'shell', 'queued', 1, $2, $3, $4, 30)
        "#,
    )
    .bind(job_id)
    .bind(payload_hash(format!("payload-{job_id}").as_bytes()))
    .bind(sqlx::types::Json(operation))
    .bind(format!("fingerprint-{job_id}"))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO job_targets (
            job_id, client_id, status, started_at, process_incarnation_id,
            dispatch_lease_until, deadline_at
        )
        VALUES (
            $1,
            $2,
            $3,
            CASE WHEN $4 THEN now() - interval '5 seconds' ELSE NULL END,
            $5,
            now() - interval '1 second',
            CASE WHEN $4 THEN now() + interval '5 minutes' ELSE NULL END
        )
        "#,
    )
    .bind(job_id)
    .bind(client_id)
    .bind(status)
    .bind(started)
    .bind(target_incarnation)
    .execute(pool)
    .await
    .unwrap();
}

#[allow(clippy::too_many_arguments)]
async fn insert_scheduled_backup_job_with_provenance(
    pool: &PgPool,
    actor_id: Uuid,
    schedule_id: Uuid,
    job_id: Uuid,
    client_id: &str,
    payload_hash: &str,
    causation_id: Uuid,
    schedule_lineage: &[Uuid],
    operation: &JobCommand,
) {
    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, actor_id, command_type, privileged, status, target_count,
            payload_hash, operation, source_schedule_id, request_fingerprint,
            max_timeout_secs, causation_id, schedule_lineage
        )
        VALUES ($1, $2, 'scheduled_backup', TRUE, 'queued', 1,
                $3, $4, $5, $6, 30, $7, $8)
        "#,
    )
    .bind(job_id)
    .bind(actor_id)
    .bind(payload_hash)
    .bind(SqlJson(operation))
    .bind(schedule_id)
    .bind(format!("provenance-{job_id}"))
    .bind(causation_id)
    .bind(schedule_lineage)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO job_targets (job_id, client_id, status) VALUES ($1, $2, 'queued')")
        .bind(job_id)
        .bind(client_id)
        .execute(pool)
        .await
        .unwrap();
}

#[allow(clippy::too_many_arguments)]
async fn insert_job_target_with_operation(
    pool: &PgPool,
    job_id: Uuid,
    client_id: &str,
    operation: JobCommand,
    command_type: &str,
    source_schedule_id: Option<Uuid>,
    status: &str,
    started: bool,
    target_incarnation: Option<Uuid>,
    max_timeout_secs: i64,
    deadline_elapsed: bool,
) {
    let job_status = if status == "queued" {
        "queued"
    } else {
        "running"
    };
    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, command_type, status, target_count, payload_hash, operation,
            source_schedule_id, request_fingerprint, max_timeout_secs
        )
        VALUES ($1, $2, $3, 1, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(job_id)
    .bind(command_type)
    .bind(job_status)
    .bind(payload_hash(format!("payload-{job_id}").as_bytes()))
    .bind(sqlx::types::Json(operation))
    .bind(source_schedule_id)
    .bind(format!("fingerprint-{job_id}"))
    .bind(max_timeout_secs)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO job_targets (
            job_id, client_id, status, started_at, process_incarnation_id,
            dispatch_lease_until, deadline_at
        )
        VALUES (
            $1,
            $2,
            $3,
            CASE WHEN $4 THEN now() - interval '10 seconds' ELSE NULL END,
            $5,
            now() - interval '1 second',
            CASE
                WHEN $4 AND $6 THEN now() - interval '1 second'
                WHEN $4 THEN now() + interval '5 minutes'
                ELSE NULL
            END
        )
        "#,
    )
    .bind(job_id)
    .bind(client_id)
    .bind(status)
    .bind(started)
    .bind(target_incarnation)
    .bind(deadline_elapsed)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_update_activation_target(
    pool: &PgPool,
    job_id: Uuid,
    client_id: &str,
    client_incarnation: Uuid,
    staged_sha256_hex: &str,
    deadline_elapsed: bool,
) {
    let operation = JobCommand::AgentUpdateActivate {
        staged_sha256_hex: staged_sha256_hex.to_string(),
        restart_agent: true,
    };
    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, command_type, status, target_count, payload_hash, operation,
            request_fingerprint, max_timeout_secs
        )
        VALUES ($1, 'agent_update_activate', 'running', 1, $2, $3, $4, 1)
        "#,
    )
    .bind(job_id)
    .bind(payload_hash(format!("payload-{job_id}").as_bytes()))
    .bind(sqlx::types::Json(operation))
    .bind(format!("fingerprint-{job_id}"))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO job_targets (
            job_id, client_id, status, started_at, process_incarnation_id,
            dispatch_lease_until, deadline_at
        )
        VALUES (
            $1,
            $2,
            'running',
            now() - interval '10 seconds',
            $3,
            now() - interval '1 second',
            CASE WHEN $4 THEN now() - interval '1 second' ELSE now() + interval '5 minutes' END
        )
        "#,
    )
    .bind(job_id)
    .bind(client_id)
    .bind(client_incarnation)
    .bind(deadline_elapsed)
    .execute(pool)
    .await
    .unwrap();
}

fn hello_event(
    client_id: &str,
    process_incarnation_id: Uuid,
    update_heartbeat: Option<AgentUpdateHeartbeat>,
) -> GatewayAgentHelloIngest {
    GatewayAgentHelloIngest {
        gateway_id: "pg-test-gateway".to_string(),
        gateway_session_id: Uuid::new_v4(),
        remote_ip: None,
        noise_public_key_hex: payload_hash(client_id.as_bytes()),
        hello: AgentHello {
            client_id: client_id.to_string(),
            process_incarnation_id,
            agent_version: "pg-test-agent".to_string(),
            internal_build_number: 1,
            os_release: "test".to_string(),
            arch: "x86_64".to_string(),
            cpu_model: None,
            kernel_release: None,
            virtualization: None,
            update_heartbeat,
            capabilities: AgentCapabilitySnapshot::default(),
        },
    }
}

async fn output_rows(pool: &PgPool, job_id: Uuid, client_id: &str) -> Vec<JobOutputView> {
    sqlx::query(
        r#"
        SELECT
            job_id,
            client_id,
            seq,
            stream,
            encode(data, 'base64') AS data_base64,
            storage,
            object_key AS artifact_object_key,
            data_sha256_hex AS artifact_sha256_hex,
            data_size_bytes AS artifact_size_bytes,
            exit_code,
            done,
            received_at::text AS received_at,
            created_at::text AS created_at
        FROM job_outputs
        WHERE job_id = $1 AND client_id = $2
        ORDER BY seq
        "#,
    )
    .bind(job_id)
    .bind(client_id)
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .map(|row| JobOutputView {
        job_id: row.try_get("job_id").unwrap(),
        client_id: row.try_get("client_id").unwrap(),
        seq: row.try_get("seq").unwrap(),
        stream: row.try_get("stream").unwrap(),
        data_base64: row.try_get("data_base64").unwrap(),
        storage: row.try_get("storage").unwrap(),
        artifact_object_key: row.try_get("artifact_object_key").unwrap(),
        artifact_sha256_hex: row.try_get("artifact_sha256_hex").unwrap(),
        artifact_size_bytes: row.try_get("artifact_size_bytes").unwrap(),
        exit_code: row.try_get("exit_code").unwrap(),
        done: row.try_get("done").unwrap(),
        received_at: row.try_get("received_at").unwrap(),
        created_at: row.try_get("created_at").unwrap(),
    })
    .collect()
}

async fn target_status(pool: &PgPool, job_id: Uuid, client_id: &str) -> String {
    sqlx::query_scalar("SELECT status FROM job_targets WHERE job_id = $1 AND client_id = $2")
        .bind(job_id)
        .bind(client_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn job_status(pool: &PgPool, job_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM jobs WHERE id = $1")
        .bind(job_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn job_payload_hash(pool: &PgPool, job_id: Uuid) -> String {
    sqlx::query_scalar("SELECT payload_hash FROM jobs WHERE id = $1")
        .bind(job_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn webhook_event_exists(pool: &PgPool, kind: &str, event_id: &str) -> bool {
    sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM webhook_events
            WHERE kind = $1 AND event_id = $2
        )
        "#,
    )
    .bind(kind)
    .bind(event_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn webhook_event_count(pool: &PgPool, kind: &str, event_id: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM webhook_events
        WHERE kind = $1 AND event_id = $2
        "#,
    )
    .bind(kind)
    .bind(event_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn processed_terminal_event_count(pool: &PgPool, job_id: Uuid) -> i64 {
    sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM job_terminal_events
        WHERE job_id = $1 AND processing_status = 'processed'
        "#,
    )
    .bind(job_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn backup_request_status(pool: &PgPool, backup_request_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM backup_requests WHERE id = $1")
        .bind(backup_request_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn schedule_outcome_row(pool: &PgPool, schedule_id: Uuid) -> (i32, String, Option<Uuid>) {
    let row = sqlx::query(
        r#"
        SELECT failure_count, COALESCE(last_job_status, '') AS last_job_status, last_job_id
        FROM schedules
        WHERE id = $1
        "#,
    )
    .bind(schedule_id)
    .fetch_one(pool)
    .await
    .unwrap();
    (
        row.try_get("failure_count").unwrap(),
        row.try_get("last_job_status").unwrap(),
        row.try_get("last_job_id").unwrap(),
    )
}

async fn receive_job_finished(
    rx: &mut broadcast::Receiver<WsEvent>,
    job_id: Uuid,
) -> Option<String> {
    for _ in 0..6 {
        let Ok(Ok(event)) = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await else {
            continue;
        };
        if let WsEvent::JobFinished {
            job_id: event_job_id,
            status,
        } = event
        {
            if event_job_id == job_id {
                return Some(status);
            }
        }
    }
    None
}

fn postgres_shell_schedule_request(name: &str, client_id: &str) -> CreateScheduleRequest {
    CreateScheduleRequest {
        name: name.to_string(),
        operation: Some(JobCommand::Shell {
            argv: vec![
                "/bin/sh".to_string(),
                "-lc".to_string(),
                "uptime".to_string(),
            ],
            pty: false,
        }),
        event_argv_template: None,
        selector_expression: String::new(),
        target_client_ids: vec![client_id.to_string()],
        trigger_kind: ScheduleTriggerKind::Cron,
        cron_expr: Some("0 * * * *".to_string()),
        timezone: Some("UTC".to_string()),
        event_expression: None,
        enabled: true,
        catch_up_policy: Some("skip_missed".to_string()),
        catch_up_limit: Some(1),
        retry_delay_secs: Some(120),
        max_failures: 2,
        privilege_assertion: None,
        confirmed: true,
    }
}

async fn latest_status_output_json(
    pool: &PgPool,
    job_id: Uuid,
    client_id: &str,
) -> serde_json::Value {
    let value: String = sqlx::query_scalar(
        r#"
        SELECT convert_from(data, 'UTF8')
        FROM job_outputs
        WHERE job_id = $1 AND client_id = $2 AND stream = 'status'
        ORDER BY seq DESC
        LIMIT 1
        "#,
    )
    .bind(job_id)
    .bind(client_id)
    .fetch_one(pool)
    .await
    .unwrap();
    serde_json::from_str(&value).unwrap()
}

async fn postgres_network_operator(repo: &Repository) -> AuthContext {
    let auth = repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "network-operator".to_string(),
            password: "network-password-123".to_string(),
        })
        .await
        .unwrap();
    AuthContext {
        operator: auth.operator,
        session_id: None,
    }
}

#[tokio::test]
async fn postgres_bootstrap_rolls_back_when_success_evidence_cannot_be_recorded() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    install_rejected_audit_action_trigger(&db.pool).await;
    set_rejected_audit_action(&db.pool, "operator_auth.login_success").await;
    let request = BootstrapOperatorRequest {
        username: "admin".to_string(),
        password: "admin-password-123".to_string(),
    };
    assert!(db
        .repo
        .bootstrap_operator_with_auth_event(
            &request,
            "203.0.113.40",
            Some("bootstrap-atomicity-test"),
        )
        .await
        .is_err());
    let rolled_back = sqlx::query_as::<_, (i64, i64, i64)>(
        r#"
        SELECT (SELECT count(*) FROM operators),
               (SELECT count(*) FROM operator_sessions),
               (SELECT count(*) FROM audit_logs)
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(rolled_back, (0, 0, 0));

    sqlx::query("DELETE FROM rejected_test_audit_actions")
        .execute(&db.pool)
        .await
        .unwrap();
    let auth = db
        .repo
        .bootstrap_operator_with_auth_event(
            &request,
            "203.0.113.40",
            Some("bootstrap-atomicity-test"),
        )
        .await
        .unwrap();
    let evidence = sqlx::query_as::<_, (Uuid, String, String, String)>(
        r#"
        SELECT actor_id,
               metadata->>'operator_session_id',
               metadata->>'remote_ip',
               metadata->>'user_agent'
        FROM audit_logs
        WHERE action = 'operator_auth.login_success'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(evidence.0, auth.operator.id);
    assert_eq!(evidence.1, auth.session_id.to_string());
    assert_eq!(evidence.2, "203.0.113.40");
    assert_eq!(evidence.3, "bootstrap-atomicity-test");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM operator_sessions")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_login_rolls_back_session_and_totp_step_with_success_evidence() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let password = "admin-password-123";
    let auth = db
        .repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: password.to_string(),
        })
        .await
        .unwrap();
    let throttle = crate::state::OperatorAuthThrottleConfig {
        username_failed_attempt_limit: 100,
        ip_failed_attempt_limit: 100,
        failed_attempt_window_secs: 300,
        lockout_secs: 60,
    };
    install_rejected_audit_action_trigger(&db.pool).await;
    set_rejected_audit_action(&db.pool, "operator_auth.login_success").await;
    let password_login = LoginRequest {
        username: "admin".to_string(),
        password: password.to_string(),
        totp_code: None,
    };
    assert!(db
        .repo
        .login_operator_with_throttle(
            &password_login,
            "203.0.113.41",
            Some("password-atomicity-test"),
            &throttle,
        )
        .await
        .is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM operator_sessions")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1
    );

    sqlx::query("DELETE FROM rejected_test_audit_actions")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(matches!(
        db.repo
            .login_operator_with_throttle(
                &password_login,
                "203.0.113.41",
                Some("password-atomicity-test"),
                &throttle,
            )
            .await
            .unwrap(),
        crate::repository_auth::OperatorLoginAttempt::Authenticated(_)
    ));

    let actor = AuthContext {
        operator: auth.operator,
        session_id: Some(auth.session_id),
    };
    let crate::model::TotpSetupOutcome::Created(_) =
        db.repo.setup_operator_totp(&actor, password).await.unwrap()
    else {
        panic!("expected TOTP setup");
    };
    let encrypted = db
        .repo
        .operator_by_username("admin")
        .await
        .unwrap()
        .unwrap()
        .encrypted_totp_secret()
        .unwrap();
    let secret = crate::auth_totp::decrypt_totp_secret(password, &encrypted).unwrap();
    let current_step = crate::unix_now() / crate::auth_totp::TOTP_PERIOD_SECS;
    let confirm_code = crate::auth_totp::totp_code_for_step(&secret, current_step);
    assert!(matches!(
        db.repo
            .confirm_operator_totp(&actor, password, &confirm_code)
            .await
            .unwrap(),
        crate::model::TotpUpdateOutcome::Updated(_)
    ));
    let login_step = current_step.saturating_add(1);
    let login_code = crate::auth_totp::totp_code_for_step(&secret, login_step);
    let totp_login = LoginRequest {
        username: "admin".to_string(),
        password: password.to_string(),
        totp_code: Some(login_code),
    };

    set_rejected_audit_action(&db.pool, "operator_auth.login_success").await;
    assert!(db
        .repo
        .login_operator_with_throttle(
            &totp_login,
            "203.0.113.42",
            Some("totp-atomicity-test"),
            &throttle,
        )
        .await
        .is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM operator_sessions")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT totp_last_accepted_step FROM operators WHERE id = $1",
        )
        .bind(actor.operator.id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        Some(current_step as i64)
    );

    sqlx::query("DELETE FROM rejected_test_audit_actions")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(matches!(
        db.repo
            .login_operator_with_throttle(
                &totp_login,
                "203.0.113.42",
                Some("totp-atomicity-test"),
                &throttle,
            )
            .await
            .unwrap(),
        crate::repository_auth::OperatorLoginAttempt::Authenticated(_)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM operator_sessions")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        3
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_logs WHERE action = 'operator_auth.login_success'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        2
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_password_login_rejects_concurrent_operator_credential_change() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let password = "admin-password-123";
    let auth = db
        .repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: password.to_string(),
        })
        .await
        .unwrap();
    let replacement_hash = crate::hash_operator_password("replacement-password-456").unwrap();
    let mut credential_change = db.pool.begin().await.unwrap();
    sqlx::query("UPDATE operators SET password_hash = $2 WHERE id = $1")
        .bind(auth.operator.id)
        .bind(replacement_hash)
        .execute(&mut *credential_change)
        .await
        .unwrap();

    let login_repo = db.repo.clone();
    let login = tokio::spawn(async move {
        login_repo
            .login_operator_with_throttle(
                &LoginRequest {
                    username: "admin".to_string(),
                    password: password.to_string(),
                    totp_code: None,
                },
                "203.0.113.43",
                Some("credential-change-race-test"),
                &crate::state::OperatorAuthThrottleConfig {
                    username_failed_attempt_limit: 100,
                    ip_failed_attempt_limit: 100,
                    failed_attempt_window_secs: 300,
                    lockout_secs: 60,
                },
            )
            .await
    });

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                r#"
                SELECT EXISTS (
                    SELECT 1
                    FROM pg_stat_activity
                    WHERE datname = current_database()
                      AND pid <> pg_backend_pid()
                      AND wait_event_type = 'Lock'
                      AND query LIKE '%NOT totp_enabled%'
                )
                "#,
            )
            .fetch_one(&db.pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("password login did not reach the guarded operator row");

    credential_change.commit().await.unwrap();
    let attempt = tokio::time::timeout(Duration::from_secs(5), login)
        .await
        .expect("password login remained blocked")
        .unwrap()
        .unwrap();
    assert!(matches!(
        attempt,
        crate::repository_auth::OperatorLoginAttempt::InvalidCredentials
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM operator_sessions")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            r#"
            SELECT metadata->>'reason'
            FROM audit_logs
            WHERE action = 'operator_auth.login_failure'
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        "operator_state_changed"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_operator_login_throttle_persists_per_client_identity_bucket() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let throttle = crate::state::OperatorAuthThrottleConfig {
        username_failed_attempt_limit: 2,
        ip_failed_attempt_limit: 100,
        failed_attempt_window_secs: 60,
        lockout_secs: 60,
    };
    db.repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: "admin-password-123".to_string(),
        })
        .await
        .unwrap();

    for _ in 0..2 {
        assert!(matches!(
            db.repo
                .login_operator_with_throttle(
                    &LoginRequest {
                        username: "admin".to_string(),
                        password: "wrong-password-123".to_string(),
                        totp_code: None,
                    },
                    "203.0.113.30",
                    None,
                    &throttle,
                )
                .await
                .unwrap(),
            crate::repository_auth::OperatorLoginAttempt::InvalidCredentials
        ));
    }
    let second_repo = Repository::Postgres(db.pool.clone());
    assert!(matches!(
        second_repo
            .login_operator_with_throttle(
                &LoginRequest {
                    username: "admin".to_string(),
                    password: "admin-password-123".to_string(),
                    totp_code: None,
                },
                "203.0.113.30",
                None,
                &throttle,
            )
            .await
            .unwrap(),
        crate::repository_auth::OperatorLoginAttempt::Throttled
    ));
    assert!(matches!(
        second_repo
            .login_operator_with_throttle(
                &LoginRequest {
                    username: "admin".to_string(),
                    password: "admin-password-123".to_string(),
                    totp_code: None,
                },
                "203.0.113.31",
                None,
                &throttle,
            )
            .await
            .unwrap(),
        crate::repository_auth::OperatorLoginAttempt::Authenticated(_)
    ));

    let row = sqlx::query(
        r#"
        SELECT failed_attempts,
               locked_until IS NOT NULL AND locked_until > now() AS locked,
               scope_key
        FROM operator_auth_throttle
        WHERE scope_kind = 'username_ip'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let failed_attempts: i64 = row.try_get("failed_attempts").unwrap();
    let locked: bool = row.try_get("locked").unwrap();
    let scope_key: String = row.try_get("scope_key").unwrap();
    assert_eq!(failed_attempts, 2);
    assert!(locked);
    assert_eq!(scope_key, "5:admin|203.0.113.30");
    let audit_count: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs WHERE action = $1")
        .bind("operator_auth.lockout_created")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(audit_count, 1);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_repeated_totp_setup_reuses_pending_secret_without_enabling() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let password = "admin-password-123";
    let auth = db
        .repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: password.to_string(),
        })
        .await
        .unwrap();
    let actor = AuthContext {
        operator: auth.operator,
        session_id: Some(Uuid::new_v4()),
    };
    let crate::model::TotpSetupOutcome::Created(first) =
        db.repo.setup_operator_totp(&actor, password).await.unwrap()
    else {
        panic!("expected initial TOTP setup");
    };
    let factor_before = sqlx::query_as::<
        _,
        (
            bool,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i64>,
        ),
    >(
        r#"
        SELECT totp_enabled,
               totp_secret_ciphertext_hex,
               totp_secret_nonce_hex,
               totp_secret_salt_hex,
               totp_last_accepted_step
        FROM operators
        WHERE id = $1
        "#,
    )
    .bind(actor.operator.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let crate::model::TotpSetupOutcome::Created(second) =
        db.repo.setup_operator_totp(&actor, password).await.unwrap()
    else {
        panic!("expected pending TOTP setup");
    };
    let factor_after = sqlx::query_as::<
        _,
        (
            bool,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i64>,
        ),
    >(
        r#"
        SELECT totp_enabled,
               totp_secret_ciphertext_hex,
               totp_secret_nonce_hex,
               totp_secret_salt_hex,
               totp_last_accepted_step
        FROM operators
        WHERE id = $1
        "#,
    )
    .bind(actor.operator.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    assert_eq!(second.secret_base32, first.secret_base32);
    assert_eq!(second.otpauth_uri, first.otpauth_uri);
    assert_eq!(factor_after, factor_before);
    assert!(!factor_after.0);
    assert_eq!(factor_after.4, None);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_logs WHERE action = 'operator_totp.setup'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        1
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_concurrent_totp_login_consumes_one_code_once() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let password = "admin-password-123";
    let auth = db
        .repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: password.to_string(),
        })
        .await
        .unwrap();
    let actor = AuthContext {
        operator: auth.operator,
        session_id: Some(Uuid::new_v4()),
    };
    let crate::model::TotpSetupOutcome::Created(_) =
        db.repo.setup_operator_totp(&actor, password).await.unwrap()
    else {
        panic!("expected TOTP setup");
    };
    let encrypted = db
        .repo
        .operator_by_username("admin")
        .await
        .unwrap()
        .unwrap()
        .encrypted_totp_secret()
        .unwrap();
    let secret = crate::auth_totp::decrypt_totp_secret(password, &encrypted).unwrap();
    let current_step = crate::unix_now() / crate::auth_totp::TOTP_PERIOD_SECS;
    let confirm_code = crate::auth_totp::totp_code_for_step(&secret, current_step);
    let login_step = current_step.saturating_add(1);
    let login_code = crate::auth_totp::totp_code_for_step(&secret, login_step);
    assert!(matches!(
        db.repo
            .confirm_operator_totp(&actor, password, &confirm_code)
            .await
            .unwrap(),
        crate::model::TotpUpdateOutcome::Updated(_)
    ));

    let left_repo = Repository::Postgres(db.pool.clone());
    let right_repo = Repository::Postgres(db.pool.clone());
    let left_request = LoginRequest {
        username: "admin".to_string(),
        password: password.to_string(),
        totp_code: Some(login_code.clone()),
    };
    let right_request = LoginRequest {
        username: "admin".to_string(),
        password: password.to_string(),
        totp_code: Some(login_code),
    };
    let throttle = crate::state::OperatorAuthThrottleConfig {
        username_failed_attempt_limit: 100,
        ip_failed_attempt_limit: 100,
        failed_attempt_window_secs: 300,
        lockout_secs: 60,
    };
    let (left, right) = tokio::join!(
        left_repo.login_operator_with_throttle(
            &left_request,
            "203.0.113.81",
            Some("totp-concurrency-left"),
            &throttle,
        ),
        right_repo.login_operator_with_throttle(
            &right_request,
            "203.0.113.82",
            Some("totp-concurrency-right"),
            &throttle,
        ),
    );
    assert_eq!(
        [left.unwrap(), right.unwrap()]
            .into_iter()
            .filter(|attempt| {
                matches!(
                    attempt,
                    crate::repository_auth::OperatorLoginAttempt::Authenticated(_)
                )
            })
            .count(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM operator_sessions WHERE operator_id = $1",
        )
        .bind(actor.operator.id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT totp_last_accepted_step FROM operators WHERE id = $1",
        )
        .bind(actor.operator.id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        Some(login_step as i64)
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_incorrect_totp_code_preserves_factor_and_creates_no_session() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let password = "admin-password-123";
    let auth = db
        .repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: password.to_string(),
        })
        .await
        .unwrap();
    let actor = AuthContext {
        operator: auth.operator,
        session_id: Some(Uuid::new_v4()),
    };
    let crate::model::TotpSetupOutcome::Created(_) =
        db.repo.setup_operator_totp(&actor, password).await.unwrap()
    else {
        panic!("expected TOTP setup");
    };
    let encrypted = db
        .repo
        .operator_by_username("admin")
        .await
        .unwrap()
        .unwrap()
        .encrypted_totp_secret()
        .unwrap();
    let secret = crate::auth_totp::decrypt_totp_secret(password, &encrypted).unwrap();
    let current_step = crate::unix_now() / crate::auth_totp::TOTP_PERIOD_SECS;
    let confirm_code = crate::auth_totp::totp_code_for_step(&secret, current_step);
    assert!(matches!(
        db.repo
            .confirm_operator_totp(&actor, password, &confirm_code)
            .await
            .unwrap(),
        crate::model::TotpUpdateOutcome::Updated(_)
    ));

    let factor_before = sqlx::query_as::<
        _,
        (
            bool,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i64>,
        ),
    >(
        r#"
        SELECT totp_enabled,
               totp_secret_ciphertext_hex,
               totp_secret_nonce_hex,
               totp_secret_salt_hex,
               totp_last_accepted_step
        FROM operators
        WHERE id = $1
        "#,
    )
    .bind(actor.operator.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(factor_before.0);
    assert_eq!(factor_before.4, Some(current_step as i64));
    let session_count_before = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM operator_sessions WHERE operator_id = $1",
    )
    .bind(actor.operator.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let wrong_code = (0..=999_999)
        .map(|value| format!("{value:06}"))
        .find(|candidate| {
            (current_step.saturating_sub(2)..=current_step.saturating_add(3)).all(|step| {
                crate::auth_totp::totp_code_for_step(&secret, step).as_str() != candidate.as_str()
            })
        })
        .expect("six surrounding TOTP steps cannot exhaust the code space");
    let attempt = db
        .repo
        .login_operator_with_throttle(
            &LoginRequest {
                username: "admin".to_string(),
                password: password.to_string(),
                totp_code: Some(wrong_code),
            },
            "203.0.113.83",
            Some("totp-wrong-code"),
            &crate::state::OperatorAuthThrottleConfig {
                username_failed_attempt_limit: 100,
                ip_failed_attempt_limit: 100,
                failed_attempt_window_secs: 300,
                lockout_secs: 60,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        attempt,
        crate::repository_auth::OperatorLoginAttempt::InvalidCredentials
    ));

    let factor_after = sqlx::query_as::<
        _,
        (
            bool,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i64>,
        ),
    >(
        r#"
        SELECT totp_enabled,
               totp_secret_ciphertext_hex,
               totp_secret_nonce_hex,
               totp_secret_salt_hex,
               totp_last_accepted_step
        FROM operators
        WHERE id = $1
        "#,
    )
    .bind(actor.operator.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(factor_after, factor_before);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM operator_sessions WHERE operator_id = $1",
        )
        .bind(actor.operator.id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        session_count_before
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_operator_totp_constraints_reject_partial_or_inconsistent_state() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let auth = db
        .repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: "admin-password-123".to_string(),
        })
        .await
        .unwrap();

    let partial_secret =
        sqlx::query("UPDATE operators SET totp_secret_ciphertext_hex = 'aa' WHERE id = $1")
            .bind(auth.operator.id)
            .execute(&db.pool)
            .await;
    assert!(partial_secret.is_err());

    let enabled_without_step = sqlx::query(
        r#"
        UPDATE operators
        SET totp_enabled = TRUE,
            totp_secret_ciphertext_hex = 'aa',
            totp_secret_nonce_hex = repeat('b', 24),
            totp_secret_salt_hex = repeat('c', 32),
            totp_last_accepted_step = NULL
        WHERE id = $1
        "#,
    )
    .bind(auth.operator.id)
    .execute(&db.pool)
    .await;
    assert!(enabled_without_step.is_err());

    let disabled_with_step =
        sqlx::query("UPDATE operators SET totp_last_accepted_step = 1 WHERE id = $1")
            .bind(auth.operator.id)
            .execute(&db.pool)
            .await;
    assert!(disabled_with_step.is_err());

    sqlx::query(
        r#"
        UPDATE operators
        SET totp_secret_ciphertext_hex = 'aa',
            totp_secret_nonce_hex = repeat('b', 24),
            totp_secret_salt_hex = repeat('c', 32)
        WHERE id = $1
        "#,
    )
    .bind(auth.operator.id)
    .execute(&db.pool)
    .await
    .expect("pending enrollment is a valid canonical TOTP state");

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_artifact_cleanup_job_persists_reviewed_artifact_identity() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let operator = postgres_network_operator(&db.repo).await;
    db.repo
        .register_server_artifact(NewServerArtifact {
            domain: "job_output".to_string(),
            object_key: "job-output/test-reviewed-artifact".to_string(),
            sha256_hex: "a".repeat(64),
            size_bytes: 12,
            job_id: Some(Uuid::new_v4()),
            client_id: Some("edge-reviewed".to_string()),
            stream: Some("stdout".to_string()),
            seq: Some(0),
            backup_request_id: None,
            backup_artifact_id: None,
            release_id: None,
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap();

    let preview = db
        .repo
        .preview_artifact_cleanup(
            r#"artifact.domain = "job_output""#,
            &["job_output".to_string()],
        )
        .await
        .unwrap();
    assert_eq!(preview.matched_count, 1);
    assert_eq!(preview.retained_count, 1);
    assert_eq!(preview.reference_protected_count, 0);
    assert_eq!(
        preview.representative_objects[0].object_key,
        "job-output/test-reviewed-artifact"
    );
    assert!(preview.oldest_created_at.is_some());
    assert!(preview.newest_created_at.is_some());
    let job = db
        .repo
        .create_artifact_cleanup_job(
            &preview.expression,
            &preview.domains,
            &preview.preview_hash,
            &operator,
        )
        .await
        .unwrap();

    let row = sqlx::query(
        r#"
        SELECT domain, object_key, sha256_hex, size_bytes
        FROM server_job_artifact_cleanup_targets
        WHERE server_job_id = $1
        "#,
    )
    .bind(job.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("domain"), "job_output");
    assert_eq!(
        row.get::<String, _>("object_key"),
        "job-output/test-reviewed-artifact"
    );
    assert_eq!(row.get::<String, _>("sha256_hex"), "a".repeat(64));
    assert_eq!(row.get::<i64, _>("size_bytes"), 12);

    sqlx::query(
        r#"
        UPDATE server_artifacts
        SET sha256_hex = $2, size_bytes = $3
        WHERE object_key = $1
        "#,
    )
    .bind("job-output/test-reviewed-artifact")
    .bind("b".repeat(64))
    .bind(13_i64)
    .execute(&db.pool)
    .await
    .unwrap();
    let identity_matches_review: bool = sqlx::query_scalar(
        r#"
        SELECT (
            artifact.domain = target.domain
            AND artifact.object_key = target.object_key
            AND artifact.sha256_hex = target.sha256_hex
            AND artifact.size_bytes = target.size_bytes
        )
        FROM server_job_artifact_cleanup_targets target
        JOIN server_artifacts artifact ON artifact.id = target.artifact_id
        WHERE target.server_job_id = $1
        "#,
    )
    .bind(job.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(!identity_matches_review);
    sqlx::query("DELETE FROM server_artifacts WHERE object_key = $1")
        .bind("job-output/test-reviewed-artifact")
        .execute(&db.pool)
        .await
        .unwrap();
    let reviewed_target_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM server_job_artifact_cleanup_targets WHERE server_job_id = $1",
    )
    .bind(job.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(reviewed_target_count, 1);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_job_persistence_and_claim_revalidate_revoked_targets() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "pg-revoked-job-target";
    insert_client(&db.pool, client_id, Some(Uuid::new_v4())).await;
    sqlx::query("UPDATE clients SET status = 'revoked' WHERE id = $1")
        .bind(client_id)
        .execute(&db.pool)
        .await
        .unwrap();
    let operator = postgres_network_operator(&db.repo).await;
    let request = crate::model::CreateJobRequest {
        job_id: None,
        selector_expression: format!("id:{client_id}"),
        target_client_ids: vec![client_id.to_string()],
        destructive: false,
        confirmed: false,
        command: "true".to_string(),
        argv: vec!["true".to_string()],
        operation: None,
        max_timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
    };
    let rejected_job_id = Uuid::new_v4();
    let error = db
        .repo
        .record_dispatching_job(
            rejected_job_id,
            &request,
            "revoked-target-command-hash",
            "revoked_before_persistence",
            &operator,
            &[client_id.to_string()],
        )
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "job_target_no_longer_available");
    let rejected_job_count: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE id = $1")
        .bind(rejected_job_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rejected_job_count, 0);

    let queued_job_id = Uuid::new_v4();
    insert_job_target(&db.pool, queued_job_id, client_id, "queued", false, None).await;
    assert!(db
        .repo
        .claim_due_job_targets(10, 30, 0)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        target_status(&db.pool, queued_job_id, client_id).await,
        "queued"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_dispatch_claim_quarantines_null_operation_and_keeps_healthy_progress() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let poison_job_id = Uuid::new_v4();
    let healthy_job_id = Uuid::new_v4();
    let deferred_healthy_job_id = Uuid::new_v4();
    let poison_client_id = "pg-claim-null-operation";
    let healthy_client_id = "pg-claim-healthy-operation";
    let deferred_healthy_client_id = "pg-claim-deferred-healthy-operation";
    insert_client(&db.pool, poison_client_id, Some(Uuid::new_v4())).await;
    insert_client(&db.pool, healthy_client_id, Some(Uuid::new_v4())).await;
    insert_client(&db.pool, deferred_healthy_client_id, Some(Uuid::new_v4())).await;
    insert_job_target(
        &db.pool,
        poison_job_id,
        poison_client_id,
        "queued",
        false,
        None,
    )
    .await;
    insert_job_target(
        &db.pool,
        deferred_healthy_job_id,
        deferred_healthy_client_id,
        "queued",
        false,
        None,
    )
    .await;
    insert_job_target(
        &db.pool,
        healthy_job_id,
        healthy_client_id,
        "queued",
        false,
        None,
    )
    .await;
    sqlx::query(
        r#"
        UPDATE jobs
        SET operation = NULL,
            created_at = now() - interval '10 minutes'
        WHERE id = $1
        "#,
    )
    .bind(poison_job_id)
    .execute(&db.pool)
    .await
    .unwrap();

    install_invalid_job_operation_audit_rejection_trigger(&db.pool).await;
    let claimed = db.repo.claim_due_job_targets(2, 30, 0).await.unwrap();
    assert_eq!(claimed.len(), 1);
    assert!([healthy_job_id, deferred_healthy_job_id].contains(&claimed[0].job_id));
    let initially_claimed_job_id = claimed[0].job_id;
    let deferred_claim = db.repo.claim_due_job_targets(1, 30, 0).await.unwrap();
    assert_eq!(deferred_claim.len(), 1);
    assert!([healthy_job_id, deferred_healthy_job_id].contains(&deferred_claim[0].job_id));
    assert_ne!(deferred_claim[0].job_id, initially_claimed_job_id);
    assert_eq!(
        target_status(&db.pool, poison_job_id, poison_client_id).await,
        "dispatching"
    );
    assert_eq!(job_status(&db.pool, poison_job_id).await, "running");
    assert_eq!(
        target_status(&db.pool, healthy_job_id, healthy_client_id).await,
        "dispatching"
    );
    assert_eq!(
        target_status(
            &db.pool,
            deferred_healthy_job_id,
            deferred_healthy_client_id
        )
        .await,
        "dispatching"
    );
    let poison_completed_at: Option<String> = sqlx::query_scalar(
        "SELECT completed_at::text FROM job_targets WHERE job_id = $1 AND client_id = $2",
    )
    .bind(poison_job_id)
    .bind(poison_client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(poison_completed_at.is_none());
    let poison_lease: Option<String> = sqlx::query_scalar(
        r#"
        SELECT dispatch_lease_until::text
        FROM job_targets
        WHERE job_id = $1 AND client_id = $2
        "#,
    )
    .bind(poison_job_id)
    .bind(poison_client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(poison_lease.is_some());
    let poison_dispatch_error: Option<String> = sqlx::query_scalar(
        "SELECT last_dispatch_error FROM job_targets WHERE job_id = $1 AND client_id = $2",
    )
    .bind(poison_job_id)
    .bind(poison_client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(poison_dispatch_error
        .as_deref()
        .is_some_and(|error| error.starts_with("invalid_job_operation:")));
    let poison_audit_count: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*)
        FROM audit_logs
        WHERE action = 'job.target_result'
          AND metadata->>'job_id' = $1
        "#,
    )
    .bind(poison_job_id.to_string())
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(poison_audit_count, 0);

    remove_invalid_job_operation_audit_rejection_trigger(&db.pool).await;
    sqlx::query(
        r#"
        UPDATE job_targets
        SET dispatch_lease_until = now() - interval '1 second'
        WHERE job_id = $1 AND client_id = $2
        "#,
    )
    .bind(poison_job_id)
    .bind(poison_client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    assert!(db
        .repo
        .claim_due_job_targets(10, 30, 0)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        target_status(&db.pool, poison_job_id, poison_client_id).await,
        TARGET_STATUS_FAILED
    );
    assert_eq!(job_status(&db.pool, poison_job_id).await, JOB_STATUS_FAILED);
    let poison_lease: Option<String> = sqlx::query_scalar(
        r#"
        SELECT dispatch_lease_until::text
        FROM job_targets
        WHERE job_id = $1 AND client_id = $2
        "#,
    )
    .bind(poison_job_id)
    .bind(poison_client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(poison_lease.is_none());
    let audit: sqlx::types::Json<serde_json::Value> = sqlx::query_scalar(
        r#"
        SELECT metadata
        FROM audit_logs
        WHERE action = 'job.target_result'
          AND metadata->>'job_id' = $1
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(poison_job_id.to_string())
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(audit.0["reason"], "invalid_job_operation");
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_dispatch_claim_binds_incarnation_and_keeps_deadline_immutable() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "pg-client-a";
    let incarnation = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    let stale_null_job_id = Uuid::new_v4();
    insert_client(&db.pool, client_id, Some(incarnation)).await;
    insert_job_target(&db.pool, job_id, client_id, "queued", false, None).await;
    insert_job_target(
        &db.pool,
        stale_null_job_id,
        client_id,
        "dispatching",
        true,
        None,
    )
    .await;

    let claimed = db.repo.claim_due_job_targets(10, 1, 0).await.unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].job_id, job_id);
    assert_eq!(claimed[0].process_incarnation_id, incarnation);
    let first_deadline: String = sqlx::query_scalar(
        "SELECT deadline_at::text FROM job_targets WHERE job_id = $1 AND client_id = $2",
    )
    .bind(job_id)
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let bound_incarnation: Uuid = sqlx::query_scalar(
        "SELECT process_incarnation_id FROM job_targets WHERE job_id = $1 AND client_id = $2",
    )
    .bind(job_id)
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(bound_incarnation, incarnation);

    sqlx::query(
        "UPDATE job_targets SET dispatch_lease_until = now() - interval '1 second' WHERE job_id = $1 AND client_id = $2",
    )
    .bind(job_id)
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let reclaimed = db.repo.claim_due_job_targets(10, 1, 0).await.unwrap();
    assert_eq!(reclaimed.len(), 1);
    assert_eq!(reclaimed[0].job_id, job_id);
    let second_deadline: String = sqlx::query_scalar(
        "SELECT deadline_at::text FROM job_targets WHERE job_id = $1 AND client_id = $2",
    )
    .bind(job_id)
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(second_deadline, first_deadline);

    sqlx::query(
        "UPDATE job_targets SET dispatch_lease_until = now() - interval '1 second' WHERE job_id = $1 AND client_id = $2",
    )
    .bind(stale_null_job_id)
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let stale_null_claim = db.repo.claim_due_job_targets(10, 1, 0).await.unwrap();
    assert!(stale_null_claim.is_empty());
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_batch_output_conflict_poison_prevents_later_final_insert() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let job_id = Uuid::new_v4();
    let client_id = "pg-client-output";
    insert_client(&db.pool, client_id, Some(Uuid::new_v4())).await;
    insert_job_target(
        &db.pool,
        job_id,
        client_id,
        "running",
        true,
        Some(Uuid::new_v4()),
    )
    .await;
    let first = CommandOutput {
        job_id,
        stream: OutputStream::Stdout,
        data: b"first".to_vec(),
        exit_code: None,
        done: false,
    };
    db.repo
        .record_job_output_chunk_checked_with_config(
            job_id,
            client_id,
            0,
            &first,
            None,
            JobOutputPersistConfig {
                object_store: None,
                artifact_min_bytes: usize::MAX,
            },
        )
        .await
        .unwrap();

    let conflicting = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: br#"{"type":"different"}"#.to_vec(),
        exit_code: Some(1),
        done: false,
    };
    let later_final = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: br#"{"type":"completed"}"#.to_vec(),
        exit_code: Some(0),
        done: true,
    };
    let results = db
        .repo
        .record_job_outputs_checked_with_config(
            job_id,
            client_id,
            &[conflicting, later_final],
            JobOutputPersistConfig {
                object_store: None,
                artifact_min_bytes: usize::MAX,
            },
        )
        .await
        .unwrap();
    assert!(results.contains(&JobOutputWriteResult::DuplicateConflict));
    let outputs = output_rows(&db.pool, job_id, client_id).await;
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].seq, 0);
    assert!(!outputs[0].done);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_command_output_ingest_rejects_late_new_output_after_terminal_target() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let job_id = Uuid::new_v4();
    let client_id = "pg-client-late-output";
    let incarnation = Uuid::new_v4();
    let gateway_session_id = Uuid::new_v4();
    insert_client(&db.pool, client_id, Some(incarnation)).await;
    sqlx::query(
        r#"
        INSERT INTO gateway_sessions (id, gateway_id, client_id, status)
        VALUES ($1, 'gateway-a', $2, 'active')
        "#,
    )
    .bind(gateway_session_id)
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    insert_job_target(
        &db.pool,
        job_id,
        client_id,
        "running",
        true,
        Some(incarnation),
    )
    .await;
    let state = postgres_app_state(&db);
    let payload_hash = job_payload_hash(&db.pool, job_id).await;
    let final_output = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: br#"{"type":"completed"}"#.to_vec(),
        exit_code: Some(0),
        done: true,
    };
    let final_event = vpsman_common::GatewayCommandOutputIngest {
        gateway_id: "gateway-a".to_string(),
        gateway_session_id,
        process_incarnation_id: incarnation,
        spooled_replay: false,
        client_id: client_id.to_string(),
        job_id,
        payload_hash: payload_hash.clone(),
        seq: 0,
        received_unix: Some(100),
        output: final_output,
    };
    let _ = crate::routes_ingest::ingest_command_output(
        axum::extract::State(state.clone()),
        internal_gateway_headers(),
        axum::Json(final_event.clone()),
    )
    .await
    .unwrap();
    assert_eq!(
        target_status(&db.pool, job_id, client_id).await,
        TARGET_STATUS_COMPLETED
    );
    assert_eq!(job_status(&db.pool, job_id).await, JOB_STATUS_COMPLETED);
    let target_event_id =
        format!("job:{job_id}:target:{client_id}:status:{TARGET_STATUS_COMPLETED}");
    let job_event_id = format!("job:{job_id}:status:{JOB_STATUS_COMPLETED}");
    assert_eq!(
        webhook_event_count(&db.pool, "job.target.status", &target_event_id).await,
        1
    );
    assert_eq!(
        webhook_event_count(&db.pool, "job.status", &job_event_id).await,
        1
    );
    assert_eq!(processed_terminal_event_count(&db.pool, job_id).await, 2);

    let _ = crate::routes_ingest::ingest_command_output(
        axum::extract::State(state.clone()),
        internal_gateway_headers(),
        axum::Json(final_event),
    )
    .await
    .unwrap();
    assert_eq!(
        webhook_event_count(&db.pool, "job.target.status", &target_event_id).await,
        1
    );
    assert_eq!(
        webhook_event_count(&db.pool, "job.status", &job_event_id).await,
        1
    );
    assert_eq!(processed_terminal_event_count(&db.pool, job_id).await, 2);

    let late_output = CommandOutput {
        job_id,
        stream: OutputStream::Stdout,
        data: b"late data".to_vec(),
        exit_code: None,
        done: false,
    };
    let late_event = vpsman_common::GatewayCommandOutputIngest {
        gateway_id: "gateway-a".to_string(),
        gateway_session_id,
        process_incarnation_id: incarnation,
        spooled_replay: false,
        client_id: client_id.to_string(),
        job_id,
        payload_hash,
        seq: 1,
        received_unix: Some(101),
        output: late_output,
    };
    let error = crate::routes_ingest::ingest_command_output(
        axum::extract::State(state),
        internal_gateway_headers(),
        axum::Json(late_event),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "job_target_not_active");
    let outputs = output_rows(&db.pool, job_id, client_id).await;
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].seq, 0);
    assert!(outputs[0].done);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_changed_incarnation_isolates_missing_and_malformed_job_operations() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "pg-client-reconnect-invalid-operation";
    let old_incarnation = Uuid::new_v4();
    let new_incarnation = Uuid::new_v4();
    let missing_job_id = Uuid::new_v4();
    let malformed_job_id = Uuid::new_v4();
    let healthy_job_id = Uuid::new_v4();
    insert_client(&db.pool, client_id, Some(old_incarnation)).await;
    insert_job_target(
        &db.pool,
        missing_job_id,
        client_id,
        "running",
        true,
        Some(old_incarnation),
    )
    .await;
    insert_job_target(
        &db.pool,
        malformed_job_id,
        client_id,
        "running",
        true,
        Some(old_incarnation),
    )
    .await;
    insert_job_target(
        &db.pool,
        healthy_job_id,
        client_id,
        "running",
        true,
        Some(old_incarnation),
    )
    .await;
    sqlx::query("UPDATE jobs SET operation = NULL WHERE id = $1")
        .bind(missing_job_id)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET operation = '{}'::jsonb WHERE id = $1")
        .bind(malformed_job_id)
        .execute(&db.pool)
        .await
        .unwrap();

    assert!(db
        .repo
        .upsert_agent_hello(&hello_event(client_id, new_incarnation, None))
        .await
        .unwrap());

    for job_id in [missing_job_id, malformed_job_id, healthy_job_id] {
        assert_eq!(
            target_status(&db.pool, job_id, client_id).await,
            TARGET_STATUS_AGENT_LOST
        );
    }
    let client_incarnation: Uuid =
        sqlx::query_scalar("SELECT process_incarnation_id FROM clients WHERE id = $1")
            .bind(client_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(client_incarnation, new_incarnation);
    for job_id in [missing_job_id, malformed_job_id] {
        let audit: sqlx::types::Json<serde_json::Value> = sqlx::query_scalar(
            r#"
            SELECT metadata
            FROM audit_logs
            WHERE action = 'job.target_result'
              AND metadata->>'job_id' = $1
            ORDER BY created_at DESC, id DESC
            LIMIT 1
            "#,
        )
        .bind(job_id.to_string())
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(
            audit.0["reason"],
            "agent_process_incarnation_changed_invalid_job_operation"
        );
        assert_eq!(audit.0["operation_decode_failed"], true);
    }
    let healthy_audit: sqlx::types::Json<serde_json::Value> = sqlx::query_scalar(
        r#"
        SELECT metadata
        FROM audit_logs
        WHERE action = 'job.target_result'
          AND metadata->>'job_id' = $1
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(healthy_job_id.to_string())
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        healthy_audit.0["reason"],
        "agent_process_incarnation_changed"
    );
    assert_eq!(healthy_audit.0["operation_decode_failed"], false);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_changed_incarnation_matching_update_heartbeat_completes_activation() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "pg-client-update-heartbeat";
    let old_incarnation = Uuid::new_v4();
    let new_incarnation = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    let staged_sha256_hex = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    insert_client(&db.pool, client_id, Some(old_incarnation)).await;
    insert_update_activation_target(
        &db.pool,
        job_id,
        client_id,
        old_incarnation,
        staged_sha256_hex,
        false,
    )
    .await;

    db.repo
        .upsert_agent_hello(&hello_event(
            client_id,
            new_incarnation,
            Some(AgentUpdateHeartbeat {
                activation_job_id: job_id,
                sha256_hex: staged_sha256_hex.to_string(),
                marker_unix: 100,
                observed_unix: 101,
            }),
        ))
        .await
        .unwrap();

    assert_eq!(
        target_status(&db.pool, job_id, client_id).await,
        TARGET_STATUS_COMPLETED
    );
    assert_eq!(job_status(&db.pool, job_id).await, JOB_STATUS_COMPLETED);
    let client_incarnation: Uuid =
        sqlx::query_scalar("SELECT process_incarnation_id FROM clients WHERE id = $1")
            .bind(client_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(client_incarnation, new_incarnation);
    let output = latest_status_output_json(&db.pool, job_id, client_id).await;
    assert_eq!(output["code"], "agent_update_restart_heartbeat_verified");
    assert_eq!(output["activation_job_id"], job_id.to_string());
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_changed_incarnation_matching_job_but_wrong_hash_fails_activation() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "pg-client-update-heartbeat-mismatch";
    let old_incarnation = Uuid::new_v4();
    let new_incarnation = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    let staged_sha256_hex = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let observed_sha256_hex = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    insert_client(&db.pool, client_id, Some(old_incarnation)).await;
    insert_update_activation_target(
        &db.pool,
        job_id,
        client_id,
        old_incarnation,
        staged_sha256_hex,
        false,
    )
    .await;

    db.repo
        .upsert_agent_hello(&hello_event(
            client_id,
            new_incarnation,
            Some(AgentUpdateHeartbeat {
                activation_job_id: job_id,
                sha256_hex: observed_sha256_hex.to_string(),
                marker_unix: 100,
                observed_unix: 101,
            }),
        ))
        .await
        .unwrap();

    assert_eq!(
        target_status(&db.pool, job_id, client_id).await,
        TARGET_STATUS_FAILED
    );
    assert_eq!(job_status(&db.pool, job_id).await, JOB_STATUS_FAILED);
    let output = latest_status_output_json(&db.pool, job_id, client_id).await;
    assert_eq!(
        output["code"],
        "agent_update_activation_heartbeat_hash_mismatch"
    );
    assert_eq!(output["activation_job_id"], job_id.to_string());
    assert_eq!(output["artifact_sha256_hex"], observed_sha256_hex);
    assert_eq!(output["staged_sha256_hex"], staged_sha256_hex);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_deadline_expiry_quarantines_malformed_operation_and_expires_healthy_row() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let poison_job_id = Uuid::new_v4();
    let healthy_job_id = Uuid::new_v4();
    let deferred_healthy_job_id = Uuid::new_v4();
    let poison_client_id = "pg-deadline-malformed-operation";
    let healthy_client_id = "pg-deadline-healthy-operation";
    let deferred_healthy_client_id = "pg-deadline-deferred-healthy-operation";
    insert_client(&db.pool, poison_client_id, Some(Uuid::new_v4())).await;
    insert_client(&db.pool, healthy_client_id, Some(Uuid::new_v4())).await;
    insert_client(&db.pool, deferred_healthy_client_id, Some(Uuid::new_v4())).await;
    for (job_id, client_id) in [
        (poison_job_id, poison_client_id),
        (healthy_job_id, healthy_client_id),
        (deferred_healthy_job_id, deferred_healthy_client_id),
    ] {
        insert_job_target_with_operation(
            &db.pool,
            job_id,
            client_id,
            JobCommand::Shell {
                argv: vec![
                    "/bin/sh".to_string(),
                    "-lc".to_string(),
                    "sleep 99".to_string(),
                ],
                pty: false,
            },
            "shell",
            None,
            "running",
            true,
            Some(Uuid::new_v4()),
            1,
            true,
        )
        .await;
    }
    sqlx::query(
        r#"
        UPDATE jobs
        SET operation = '{"type":"removed_legacy_operation"}'::jsonb
        WHERE id = $1
        "#,
    )
    .bind(poison_job_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE job_targets
        SET deadline_at = now() - interval '10 minutes',
            started_at = now() - interval '20 minutes'
        WHERE job_id = $1
        "#,
    )
    .bind(poison_job_id)
    .execute(&db.pool)
    .await
    .unwrap();

    install_invalid_job_operation_audit_rejection_trigger(&db.pool).await;
    let expired = db.repo.expire_control_timeout_targets(2, 0).await.unwrap();
    assert_eq!(expired.len(), 1);
    assert!([healthy_job_id, deferred_healthy_job_id].contains(&expired[0].job_id));
    assert_eq!(expired[0].status, TARGET_STATUS_CONTROL_TIMEOUT);
    let initially_expired_job_id = expired[0].job_id;
    let deferred_expiry = db.repo.expire_control_timeout_targets(1, 0).await.unwrap();
    assert_eq!(deferred_expiry.len(), 1);
    assert!([healthy_job_id, deferred_healthy_job_id].contains(&deferred_expiry[0].job_id));
    assert_ne!(deferred_expiry[0].job_id, initially_expired_job_id);
    assert_eq!(deferred_expiry[0].status, TARGET_STATUS_CONTROL_TIMEOUT);
    assert_eq!(
        target_status(&db.pool, poison_job_id, poison_client_id).await,
        "running"
    );
    assert_eq!(
        target_status(&db.pool, healthy_job_id, healthy_client_id).await,
        TARGET_STATUS_CONTROL_TIMEOUT
    );
    assert_eq!(
        target_status(
            &db.pool,
            deferred_healthy_job_id,
            deferred_healthy_client_id
        )
        .await,
        TARGET_STATUS_CONTROL_TIMEOUT
    );
    assert_eq!(job_status(&db.pool, poison_job_id).await, "running");
    assert_eq!(
        job_status(&db.pool, healthy_job_id).await,
        JOB_STATUS_CONTROL_TIMEOUT
    );
    assert_eq!(
        job_status(&db.pool, deferred_healthy_job_id).await,
        JOB_STATUS_CONTROL_TIMEOUT
    );
    let poison_terminal_fields: (Option<String>, Option<String>, Option<String>) = sqlx::query_as(
        r#"
        SELECT completed_at::text, cancel_requested_at::text, last_dispatch_error
        FROM job_targets
        WHERE job_id = $1 AND client_id = $2
        "#,
    )
    .bind(poison_job_id)
    .bind(poison_client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(poison_terminal_fields.0.is_none());
    assert!(poison_terminal_fields.1.is_none());
    assert!(poison_terminal_fields
        .2
        .as_deref()
        .is_some_and(|error| error.starts_with("invalid_job_operation:")));
    let poison_audit_count: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*)
        FROM audit_logs
        WHERE action = 'job.target_result'
          AND metadata->>'job_id' = $1
        "#,
    )
    .bind(poison_job_id.to_string())
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(poison_audit_count, 0);

    remove_invalid_job_operation_audit_rejection_trigger(&db.pool).await;
    sqlx::query(
        r#"
        UPDATE job_targets
        SET dispatch_lease_until = now() - interval '1 second'
        WHERE job_id = $1 AND client_id = $2
        "#,
    )
    .bind(poison_job_id)
    .bind(poison_client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let retry = db.repo.expire_control_timeout_targets(1, 0).await.unwrap();
    assert_eq!(retry.len(), 1);
    assert_eq!(retry[0].job_id, poison_job_id);
    assert_eq!(retry[0].status, TARGET_STATUS_CONTROL_TIMEOUT);
    assert_eq!(
        target_status(&db.pool, poison_job_id, poison_client_id).await,
        TARGET_STATUS_CONTROL_TIMEOUT
    );
    assert_eq!(
        job_status(&db.pool, poison_job_id).await,
        JOB_STATUS_CONTROL_TIMEOUT
    );
    let audit: sqlx::types::Json<serde_json::Value> = sqlx::query_scalar(
        r#"
        SELECT metadata
        FROM audit_logs
        WHERE action = 'job.target_result'
          AND metadata->>'job_id' = $1
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(poison_job_id.to_string())
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(audit.0["reason"], "invalid_job_operation");
    assert!(db
        .repo
        .expire_control_timeout_targets(10, 0)
        .await
        .unwrap()
        .is_empty());
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_missing_update_heartbeat_deadline_becomes_agent_lost() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "pg-client-update-timeout";
    let incarnation = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    insert_client(&db.pool, client_id, Some(incarnation)).await;
    insert_update_activation_target(
        &db.pool,
        job_id,
        client_id,
        incarnation,
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        true,
    )
    .await;

    let expired = db.repo.expire_control_timeout_targets(10, 0).await.unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].job_id, job_id);
    assert_eq!(expired[0].status, TARGET_STATUS_AGENT_LOST);
    assert_eq!(
        target_status(&db.pool, job_id, client_id).await,
        TARGET_STATUS_AGENT_LOST
    );
    assert_eq!(job_status(&db.pool, job_id).await, JOB_STATUS_FAILED);
    let output = latest_status_output_json(&db.pool, job_id, client_id).await;
    assert_eq!(output["code"], "agent_update_restart_missing_heartbeat");
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_vnstat_finalization_phase_requires_contiguous_output_to_defer_deadline() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let protected_client = "pg-vnstat-finalize-protected";
    let gapped_client = "pg-vnstat-finalize-gapped";
    let protected_job = Uuid::new_v4();
    let gapped_job = Uuid::new_v4();
    for client_id in [protected_client, gapped_client] {
        insert_client(&db.pool, client_id, Some(Uuid::new_v4())).await;
    }
    for (job_id, client_id) in [
        (protected_job, protected_client),
        (gapped_job, gapped_client),
    ] {
        insert_job_target_with_operation(
            &db.pool,
            job_id,
            client_id,
            JobCommand::NetworkTrafficImportVnstat {
                interfaces: Vec::new(),
                start_unix: 60,
            },
            "network_traffic_import_vnstat",
            None,
            "running",
            true,
            Some(Uuid::new_v4()),
            1,
            true,
        )
        .await;
    }
    let chunk = CommandOutput {
        job_id: protected_job,
        stream: OutputStream::Status,
        data: br#"{"type":"network_traffic_import_vnstat_batch","batch_index":0,"buckets":[]}"#
            .to_vec(),
        exit_code: None,
        done: false,
    };
    let protected_final = CommandOutput {
        job_id: protected_job,
        stream: OutputStream::Status,
        data: br#"{"type":"network_traffic_import_vnstat","status":"collected"}"#.to_vec(),
        exit_code: Some(0),
        done: true,
    };
    db.repo
        .record_job_outputs(protected_job, protected_client, &[chunk, protected_final])
        .await
        .unwrap();
    let gapped_final = CommandOutput {
        job_id: gapped_job,
        stream: OutputStream::Status,
        data: br#"{"type":"network_traffic_import_vnstat","status":"collected"}"#.to_vec(),
        exit_code: Some(0),
        done: true,
    };
    db.repo
        .record_active_job_output_chunk_checked_with_config(
            gapped_job,
            gapped_client,
            1,
            &gapped_final,
            None,
            JobOutputPersistConfig {
                object_store: None,
                artifact_min_bytes: usize::MAX,
            },
        )
        .await
        .unwrap();

    let expired = db.repo.expire_control_timeout_targets(10, 0).await.unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].job_id, gapped_job);
    assert_eq!(expired[0].status, TARGET_STATUS_CONTROL_TIMEOUT);
    assert_eq!(
        target_status(&db.pool, protected_job, protected_client).await,
        "running"
    );
    assert_eq!(
        target_status(&db.pool, gapped_job, gapped_client).await,
        TARGET_STATUS_CONTROL_TIMEOUT
    );
    assert_eq!(
        db.repo
            .list_pending_network_traffic_import_finalizations(128)
            .await
            .unwrap()
            .into_iter()
            .map(|target| (target.job_id, target.client_id))
            .collect::<Vec<_>>(),
        vec![(protected_job, protected_client.to_string())]
    );
    db.repo
        .defer_network_traffic_import_finalization(
            protected_job,
            protected_client,
            "vnStat server import retry pending",
            30,
        )
        .await
        .unwrap();
    assert!(db
        .repo
        .list_pending_network_traffic_import_finalizations(128)
        .await
        .unwrap()
        .is_empty());
    let retry_is_cooled: bool = sqlx::query_scalar(
        r#"
        SELECT dispatch_lease_until > now()
        FROM job_targets
        WHERE job_id = $1 AND client_id = $2
        "#,
    )
    .bind(protected_job)
    .bind(protected_client)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(retry_is_cooled);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_control_timeout_terminal_event_updates_schedule_and_webhooks() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "pg-client-scheduled-timeout";
    let job_id = Uuid::new_v4();
    insert_client(&db.pool, client_id, Some(Uuid::new_v4())).await;
    let operator = postgres_network_operator(&db.repo).await;
    let schedule = db
        .repo
        .create_schedule(
            postgres_shell_schedule_request("pg-timeout-schedule", client_id),
            &operator,
        )
        .await
        .unwrap();
    insert_job_target_with_operation(
        &db.pool,
        job_id,
        client_id,
        JobCommand::Shell {
            argv: vec![
                "/bin/sh".to_string(),
                "-lc".to_string(),
                "sleep 99".to_string(),
            ],
            pty: false,
        },
        "shell",
        Some(schedule.id),
        "running",
        true,
        Some(Uuid::new_v4()),
        1,
        true,
    )
    .await;

    let expired = db.repo.expire_control_timeout_targets(10, 0).await.unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].status, TARGET_STATUS_CONTROL_TIMEOUT);
    let state = postgres_app_state(&db);
    let batch = state.process_job_terminal_events(500).await.unwrap();
    assert!(batch
        .jobs
        .iter()
        .any(|event| event.job_id == job_id && event.status == JOB_STATUS_CONTROL_TIMEOUT));

    assert_eq!(
        job_status(&db.pool, job_id).await,
        JOB_STATUS_CONTROL_TIMEOUT
    );
    let (failure_count, last_job_status, last_job_id) =
        schedule_outcome_row(&db.pool, schedule.id).await;
    assert_eq!(failure_count, 1);
    assert_eq!(last_job_status, JOB_STATUS_CONTROL_TIMEOUT);
    assert_eq!(last_job_id, Some(job_id));
    assert!(
        webhook_event_exists(
            &db.pool,
            "job.status",
            &format!("job:{job_id}:status:{JOB_STATUS_CONTROL_TIMEOUT}")
        )
        .await
    );
    assert!(
        webhook_event_exists(
            &db.pool,
            "schedule.job_finished",
            &format!("schedule:{}:job:{job_id}:finished", schedule.id)
        )
        .await
    );
    assert!(
        webhook_event_exists(
            &db.pool,
            "schedule.failed",
            &format!("schedule:{}:job:{job_id}:failed", schedule.id)
        )
        .await
    );
    assert_eq!(processed_terminal_event_count(&db.pool, job_id).await, 2);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_terminal_event_retry_keeps_schedule_and_webhooks_idempotent() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "pg-client-terminal-event-retry";
    let job_id = Uuid::new_v4();
    insert_client(&db.pool, client_id, Some(Uuid::new_v4())).await;
    let operator = postgres_network_operator(&db.repo).await;
    let schedule = db
        .repo
        .create_schedule(
            postgres_shell_schedule_request("pg-terminal-event-retry", client_id),
            &operator,
        )
        .await
        .unwrap();
    insert_job_target_with_operation(
        &db.pool,
        job_id,
        client_id,
        JobCommand::Shell {
            argv: vec![
                "/bin/sh".to_string(),
                "-lc".to_string(),
                "sleep 99".to_string(),
            ],
            pty: false,
        },
        "shell",
        Some(schedule.id),
        "running",
        true,
        Some(Uuid::new_v4()),
        1,
        true,
    )
    .await;

    let expired = db.repo.expire_control_timeout_targets(10, 0).await.unwrap();
    assert_eq!(expired.len(), 1);
    let state = postgres_app_state(&db);
    state.process_job_terminal_events(500).await.unwrap();

    let job_event_id = format!("job:{job_id}:status:{JOB_STATUS_CONTROL_TIMEOUT}");
    let schedule_finished_event_id = format!("schedule:{}:job:{job_id}:finished", schedule.id);
    let schedule_failed_event_id = format!("schedule:{}:job:{job_id}:failed", schedule.id);
    let (failure_count, last_job_status, last_job_id) =
        schedule_outcome_row(&db.pool, schedule.id).await;
    assert_eq!(failure_count, 1);
    assert_eq!(last_job_status, JOB_STATUS_CONTROL_TIMEOUT);
    assert_eq!(last_job_id, Some(job_id));
    assert_eq!(
        webhook_event_count(&db.pool, "job.status", &job_event_id).await,
        1
    );
    assert_eq!(
        webhook_event_count(
            &db.pool,
            "schedule.job_finished",
            &schedule_finished_event_id
        )
        .await,
        1
    );
    assert_eq!(
        webhook_event_count(&db.pool, "schedule.failed", &schedule_failed_event_id).await,
        1
    );

    sqlx::query(
        r#"
        UPDATE job_terminal_events
        SET
            processing_status = 'failed',
            processed_at = NULL,
            next_attempt_at = NULL,
            lease_id = NULL,
            lease_until = NULL,
            last_error = NULL
        WHERE job_id = $1
          AND event_kind = 'job_terminalized'
        "#,
    )
    .bind(job_id)
    .execute(&db.pool)
    .await
    .unwrap();
    state.process_job_terminal_events(500).await.unwrap();

    let (failure_count, last_job_status, last_job_id) =
        schedule_outcome_row(&db.pool, schedule.id).await;
    assert_eq!(failure_count, 1);
    assert_eq!(last_job_status, JOB_STATUS_CONTROL_TIMEOUT);
    assert_eq!(last_job_id, Some(job_id));
    assert_eq!(
        webhook_event_count(&db.pool, "job.status", &job_event_id).await,
        1
    );
    assert_eq!(
        webhook_event_count(
            &db.pool,
            "schedule.job_finished",
            &schedule_finished_event_id
        )
        .await,
        1
    );
    assert_eq!(
        webhook_event_count(&db.pool, "schedule.failed", &schedule_failed_event_id).await,
        1
    );
    assert_eq!(processed_terminal_event_count(&db.pool, job_id).await, 2);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_queued_cancel_terminal_event_records_target_and_job_side_effects() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "pg-client-queued-cancel";
    let job_id = Uuid::new_v4();
    insert_client(&db.pool, client_id, Some(Uuid::new_v4())).await;
    insert_job_target(&db.pool, job_id, client_id, "queued", false, None).await;
    let operator = postgres_network_operator(&db.repo).await;

    let plan = db
        .repo
        .request_job_cancel(job_id, &operator, Some("test cancel"))
        .await
        .unwrap();
    assert_eq!(plan.pending_canceled, 1);
    let state = postgres_app_state(&db);
    let batch = state.process_job_terminal_events(500).await.unwrap();
    assert!(batch.targets.iter().any(|event| event.job_id == job_id
        && event.client_id == client_id
        && event.outcome.status == TARGET_STATUS_CANCELED));
    assert!(batch
        .jobs
        .iter()
        .any(|event| event.job_id == job_id && event.status == JOB_STATUS_CANCELED));

    assert_eq!(
        target_status(&db.pool, job_id, client_id).await,
        TARGET_STATUS_CANCELED
    );
    assert_eq!(job_status(&db.pool, job_id).await, JOB_STATUS_CANCELED);
    assert!(
        webhook_event_exists(
            &db.pool,
            "job.target.status",
            &format!("job:{job_id}:target:{client_id}:status:{TARGET_STATUS_CANCELED}")
        )
        .await
    );
    assert!(
        webhook_event_exists(
            &db.pool,
            "job.status",
            &format!("job:{job_id}:status:{JOB_STATUS_CANCELED}")
        )
        .await
    );
    assert_eq!(processed_terminal_event_count(&db.pool, job_id).await, 2);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_agent_hello_cleanup_processes_terminal_events_and_publishes_finish() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "pg-client-hello-terminal-events";
    let old_incarnation = Uuid::new_v4();
    let new_incarnation = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    let staged_sha256_hex = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    insert_client(&db.pool, client_id, Some(old_incarnation)).await;
    insert_update_activation_target(
        &db.pool,
        job_id,
        client_id,
        old_incarnation,
        staged_sha256_hex,
        false,
    )
    .await;
    let state = postgres_app_state(&db);
    let mut rx = state.events.subscribe();

    let _ = crate::routes_ingest::ingest_agent_hello(
        axum::extract::State(state.clone()),
        internal_gateway_headers(),
        axum::Json(hello_event(
            client_id,
            new_incarnation,
            Some(AgentUpdateHeartbeat {
                activation_job_id: job_id,
                sha256_hex: staged_sha256_hex.to_string(),
                marker_unix: 100,
                observed_unix: 101,
            }),
        )),
    )
    .await
    .unwrap();

    assert_eq!(
        receive_job_finished(&mut rx, job_id).await,
        Some(JOB_STATUS_COMPLETED.to_string())
    );
    assert_eq!(job_status(&db.pool, job_id).await, JOB_STATUS_COMPLETED);
    assert!(
        webhook_event_exists(
            &db.pool,
            "job.target.status",
            &format!("job:{job_id}:target:{client_id}:status:{TARGET_STATUS_COMPLETED}")
        )
        .await
    );
    assert!(
        webhook_event_exists(
            &db.pool,
            "job.status",
            &format!("job:{job_id}:status:{JOB_STATUS_COMPLETED}")
        )
        .await
    );
    assert_eq!(processed_terminal_event_count(&db.pool, job_id).await, 2);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_delete_agent_cleanup_terminal_events_cover_backup_and_queued_skip() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "pg-client-delete-cleanup";
    let incarnation = Uuid::new_v4();
    let backup_job_id = Uuid::new_v4();
    let queued_job_id = Uuid::new_v4();
    insert_client(&db.pool, client_id, Some(incarnation)).await;
    let operator = postgres_network_operator(&db.repo).await;
    insert_job_target_with_operation(
        &db.pool,
        backup_job_id,
        client_id,
        JobCommand::Backup {
            paths: vec!["/etc".to_string()],
            include_config: false,
            follow_symlinks: false,
            missing_path_policy: vpsman_common::BackupMissingPathPolicy::Fail,
        },
        "backup",
        None,
        "running",
        true,
        Some(incarnation),
        30,
        false,
    )
    .await;
    insert_job_target(&db.pool, queued_job_id, client_id, "queued", false, None).await;
    let backup_request = db
        .repo
        .record_backup_request_with_source(
            &CreateBackupRequest {
                client_id: client_id.to_string(),
                paths: vec!["/etc".to_string()],
                include_config: false,
                follow_symlinks: false,
                missing_path_policy: vpsman_common::BackupMissingPathPolicy::Fail,
                confirmed: true,
                note: None,
                privilege_assertion: None,
            },
            "backup-request-payload",
            &format!("client:{client_id}"),
            &operator,
            BackupRequestStatus::RequestedMetadataOnly,
            BackupRequestSourceLink {
                job_id: Some(backup_job_id),
                schedule_id: None,
                ..BackupRequestSourceLink::default()
            },
        )
        .await
        .unwrap();

    db.repo
        .delete_agent(client_id, Some("test delete"), &operator)
        .await
        .unwrap();
    let state = postgres_app_state(&db);
    state.process_job_terminal_events(500).await.unwrap();

    assert_eq!(
        backup_request_status(&db.pool, backup_request.id).await,
        BackupRequestStatus::ExecutionFailed.as_str()
    );
    assert_eq!(
        target_status(&db.pool, backup_job_id, client_id).await,
        TARGET_STATUS_AGENT_LOST
    );
    assert_eq!(job_status(&db.pool, backup_job_id).await, JOB_STATUS_FAILED);
    assert_eq!(
        target_status(&db.pool, queued_job_id, client_id).await,
        TARGET_STATUS_SKIPPED
    );
    assert_eq!(
        job_status(&db.pool, queued_job_id).await,
        JOB_STATUS_SKIPPED
    );
    assert!(
        webhook_event_exists(
            &db.pool,
            "job.status",
            &format!("job:{backup_job_id}:status:{JOB_STATUS_FAILED}")
        )
        .await
    );
    assert!(
        webhook_event_exists(
            &db.pool,
            "job.target.status",
            &format!("job:{queued_job_id}:target:{client_id}:status:{TARGET_STATUS_SKIPPED}")
        )
        .await
    );
    assert!(
        webhook_event_exists(
            &db.pool,
            "job.status",
            &format!("job:{queued_job_id}:status:{JOB_STATUS_SKIPPED}")
        )
        .await
    );
    assert_eq!(
        processed_terminal_event_count(&db.pool, backup_job_id).await,
        2
    );
    assert_eq!(
        processed_terminal_event_count(&db.pool, queued_job_id).await,
        2
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_scheduled_backup_claim_and_failure_preserve_exact_provenance() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "pg-backup-provenance-client";
    insert_client(&db.pool, client_id, Some(Uuid::new_v4())).await;
    let operator = postgres_network_operator(&db.repo).await;
    let schedule_a = Uuid::new_v4();
    let causation_a = Uuid::new_v4();
    let causation_b = Uuid::new_v4();
    let job_a = Uuid::new_v4();
    let job_b = Uuid::new_v4();
    let operation = JobCommand::Backup {
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        follow_symlinks: false,
        missing_path_policy: vpsman_common::BackupMissingPathPolicy::Fail,
    };
    let shared_payload = payload_hash(b"same scheduled backup payload");
    sqlx::query(
        r#"
        INSERT INTO schedules (
            id, actor_id, name, operation, selector_expression,
            target_client_ids, cron_expr, next_run_at
        )
        VALUES ($1, $2, 'backup provenance schedule', $3, $4, $5, '0 * * * *', now())
        "#,
    )
    .bind(schedule_a)
    .bind(operator.operator.id)
    .bind(SqlJson(&operation))
    .bind(format!("id:{client_id}"))
    .bind(vec![client_id.to_string()])
    .execute(&db.pool)
    .await
    .unwrap();

    insert_scheduled_backup_job_with_provenance(
        &db.pool,
        operator.operator.id,
        schedule_a,
        job_a,
        client_id,
        &shared_payload,
        causation_a,
        &[schedule_a],
        &operation,
    )
    .await;
    let claimed_a = db
        .repo
        .claim_due_job_targets(1, 30, 0)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(claimed_a.job_id, job_a);
    assert_eq!(claimed_a.causation_id, Some(causation_a));
    assert_eq!(claimed_a.schedule_lineage, vec![schedule_a]);
    let request = CreateBackupRequest {
        client_id: client_id.to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        follow_symlinks: false,
        missing_path_policy: vpsman_common::BackupMissingPathPolicy::Fail,
        confirmed: true,
        note: None,
        privilege_assertion: None,
    };
    let source_a = BackupRequestSourceLink {
        job_id: Some(job_a),
        schedule_id: Some(schedule_a),
        causation_id: claimed_a.causation_id,
        schedule_lineage: claimed_a.schedule_lineage,
    };
    let backup_a = db
        .repo
        .record_backup_request_with_source(
            &request,
            &shared_payload,
            &format!("client:{client_id}"),
            &operator,
            BackupRequestStatus::RequestedMetadataOnly,
            source_a,
        )
        .await
        .unwrap();
    sqlx::query(
        r#"
        UPDATE job_targets
        SET status = 'completed', completed_at = now()
        WHERE job_id = $1 AND client_id = $2
        "#,
    )
    .bind(job_a)
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE jobs SET status = 'completed', completed_at = now() WHERE id = $1")
        .bind(job_a)
        .execute(&db.pool)
        .await
        .unwrap();

    insert_scheduled_backup_job_with_provenance(
        &db.pool,
        operator.operator.id,
        schedule_a,
        job_b,
        client_id,
        &shared_payload,
        causation_b,
        &[schedule_a],
        &operation,
    )
    .await;
    let claimed_b = db
        .repo
        .claim_due_job_targets(1, 30, 0)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(claimed_b.job_id, job_b);
    assert_eq!(claimed_b.causation_id, Some(causation_b));
    assert_eq!(claimed_b.schedule_lineage, vec![schedule_a]);
    let source_b = BackupRequestSourceLink {
        job_id: Some(job_b),
        schedule_id: Some(schedule_a),
        causation_id: claimed_b.causation_id,
        schedule_lineage: claimed_b.schedule_lineage,
    };
    assert!(db
        .repo
        .find_open_backup_request_for_source(client_id, &shared_payload, &source_b)
        .await
        .unwrap()
        .is_none());
    let backup_b = db
        .repo
        .record_backup_request_with_source(
            &request,
            &shared_payload,
            &format!("client:{client_id}"),
            &operator,
            BackupRequestStatus::RequestedMetadataOnly,
            source_b,
        )
        .await
        .unwrap();
    assert_ne!(backup_a.id, backup_b.id);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM backup_requests WHERE payload_hash = $1"
        )
        .bind(&shared_payload)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        2
    );

    db.repo
        .mark_open_backup_request_execution_terminal(
            job_b,
            client_id,
            BackupRequestStatus::ExecutionFailed,
            Some(&operator),
        )
        .await
        .unwrap();
    let evidence = sqlx::query(
        r#"
        SELECT causation_id, schedule_lineage
        FROM alert_policy_evidence
        WHERE source_kind = 'backup.failure' AND source_event_id = $1
        "#,
    )
    .bind(backup_b.id.to_string())
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        evidence.try_get::<Option<Uuid>, _>("causation_id").unwrap(),
        Some(causation_b)
    );
    let failure_lineage = evidence
        .try_get::<Vec<Uuid>, _>("schedule_lineage")
        .unwrap();
    assert_eq!(failure_lineage, vec![schedule_a]);
    assert!(failure_lineage.contains(&schedule_a));
    let lifecycle = sqlx::query(
        r#"
        SELECT event.causation_id, event.schedule_lineage
        FROM alert_lifecycle_events event
        JOIN alert_episodes episode ON episode.id = event.episode_id
        JOIN alert_policy_evidence evidence ON evidence.id = episode.trigger_evidence_id
        WHERE evidence.source_kind = 'backup.failure'
          AND evidence.source_event_id = $1
          AND event.edge_kind = 'alert.triggered'
        "#,
    )
    .bind(backup_b.id.to_string())
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        lifecycle
            .try_get::<Option<Uuid>, _>("causation_id")
            .unwrap(),
        Some(causation_b)
    );
    assert_eq!(
        lifecycle
            .try_get::<Vec<Uuid>, _>("schedule_lineage")
            .unwrap(),
        vec![schedule_a]
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_agent_source_disappearance_closes_policy_episode_and_resets_gate() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "policy-source-exit-agent";
    insert_client(&db.pool, client_id, Some(Uuid::new_v4())).await;

    let mut trigger = db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM clients WHERE id=$1 FOR UPDATE")
        .bind(client_id)
        .fetch_one(&mut *trigger)
        .await
        .unwrap();
    sqlx::query("UPDATE clients SET status='revoked' WHERE id=$1")
        .bind(client_id)
        .execute(&mut *trigger)
        .await
        .unwrap();
    crate::repository_operational_alerts::reconcile_postgres_agent_alert_transition_in_tx(
        &mut trigger,
        client_id,
        "revoked",
    )
    .await
    .unwrap();
    trigger.commit().await.unwrap();

    let episode_id: Uuid = sqlx::query_scalar(
        r#"
        SELECT episode.id
        FROM alert_episodes episode
        JOIN policy_rules rule ON rule.id=episode.policy_rule_id
        WHERE rule.evidence_source='agent.access'
          AND episode.client_id=$1
          AND episode.resolved_at IS NULL
          AND episode.last_confirmed_at IS NOT NULL
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let mut remove = db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM clients WHERE id=$1 FOR UPDATE")
        .bind(client_id)
        .fetch_one(&mut *remove)
        .await
        .unwrap();
    sqlx::query("UPDATE clients SET status='deleted', hidden_at=clock_timestamp() WHERE id=$1")
        .bind(client_id)
        .execute(&mut *remove)
        .await
        .unwrap();
    crate::repository_operational_alerts::reconcile_postgres_agent_alert_transition_in_tx(
        &mut remove,
        client_id,
        "deleted",
    )
    .await
    .unwrap();
    remove.commit().await.unwrap();

    let resolved = sqlx::query(
        r#"
        SELECT episode.lifecycle_state, episode.resolution_reason,
               evidence.payload->>'source_present' AS source_present,
               state.active_episode_id, state.next_transition_at,
               state.trigger_confirmed_duration_secs,
               state.trigger_segment_started_at,
               state.resolve_confirmed_duration_secs,
               state.resolve_segment_started_at
        FROM alert_episodes episode
        JOIN alert_policy_evidence evidence ON evidence.id=episode.last_evidence_id
        JOIN alert_policy_evaluation_states state
          ON state.policy_rule_id=episode.policy_rule_id
         AND state.rule_version=episode.policy_rule_version
         AND state.confirmation_bucket_key='natural:' || episode.natural_key
        WHERE episode.id=$1
        "#,
    )
    .bind(episode_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        resolved.try_get::<String, _>("lifecycle_state").unwrap(),
        "resolved"
    );
    assert_eq!(
        resolved.try_get::<String, _>("resolution_reason").unwrap(),
        "source_scope_exited"
    );
    assert_eq!(
        resolved.try_get::<String, _>("source_present").unwrap(),
        "false"
    );
    assert!(resolved
        .try_get::<Option<Uuid>, _>("active_episode_id")
        .unwrap()
        .is_none());
    assert!(resolved
        .try_get::<Option<chrono::DateTime<Utc>>, _>("next_transition_at")
        .unwrap()
        .is_none());
    assert_eq!(
        resolved
            .try_get::<i64, _>("trigger_confirmed_duration_secs")
            .unwrap(),
        0
    );
    assert_eq!(
        resolved
            .try_get::<i64, _>("resolve_confirmed_duration_secs")
            .unwrap(),
        0
    );
    assert!(resolved
        .try_get::<Option<chrono::DateTime<Utc>>, _>("trigger_segment_started_at")
        .unwrap()
        .is_none());
    assert!(resolved
        .try_get::<Option<chrono::DateTime<Utc>>, _>("resolve_segment_started_at")
        .unwrap()
        .is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM alert_policy_confirmations WHERE policy_rule_id=(SELECT policy_rule_id FROM alert_episodes WHERE id=$1)"
        )
        .bind(episode_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM alert_lifecycle_events WHERE episode_id=$1 AND edge_kind='alert.resolved'"
        )
        .bind(episode_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        1
    );

    db.cleanup().await;
}
