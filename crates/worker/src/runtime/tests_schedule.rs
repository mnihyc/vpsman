use super::*;
use crate::test_support::PgWorkerTestDb;

#[test]
fn artifact_cleanup_worker_rejects_targets_beyond_the_reviewed_limit() {
    assert!(ensure_artifact_cleanup_target_count(MAX_ARTIFACT_CLEANUP_REVIEWED_TARGETS).is_ok());
    assert!(
        ensure_artifact_cleanup_target_count(MAX_ARTIFACT_CLEANUP_REVIEWED_TARGETS + 1).is_err()
    );
}

#[test]
fn artifact_cleanup_worker_validates_all_sizes_before_deletion() {
    let candidate = |id, size_bytes| ArtifactCleanupCandidate {
        id,
        domain: "job_output".to_string(),
        object_key: format!("job-outputs/{id}"),
        sha256_hex: "a".repeat(64),
        size_bytes,
        status: "active".to_string(),
        backup_artifact_id: None,
        identity_matches_review: true,
    };
    assert!(validate_artifact_cleanup_candidate_sizes(&[
        candidate(Uuid::new_v4(), 1),
        candidate(Uuid::new_v4(), 2),
    ])
    .is_ok());
    assert!(validate_artifact_cleanup_candidate_sizes(&[candidate(Uuid::new_v4(), -1,)]).is_err());
    assert!(validate_artifact_cleanup_candidate_sizes(&[
        candidate(Uuid::new_v4(), i64::MAX),
        candidate(Uuid::new_v4(), 1),
    ])
    .is_err());
}

fn schedule_with_policy(policy: &str, limit: i32) -> DueSchedule {
    DueSchedule {
        id: Uuid::nil(),
        actor_id: None,
        actor_username: None,
        actor_role: None,
        name: "test".to_string(),
        definition_revision: 1,
        trigger_kind: "cron".to_string(),
        operation: JobCommand::Shell {
            argv: vec!["/bin/true".to_string()],
            pty: false,
        },
        selector_expression: "tag:edge".to_string(),
        target_client_ids: vec!["edge-a".to_string()],
        cron_expr: "* * * * *".to_string(),
        next_run_at_unix: 1_800_000_000,
        catch_up_policy: policy.to_string(),
        catch_up_limit: limit,
        retry_delay_secs: 300,
        max_failures: 3,
        failure_count: 0,
        last_error: None,
        materialization: ScheduleMaterializationContext::default(),
    }
}

fn scheduled_speed_test_operation() -> serde_json::Value {
    let plan = plan_tunnel(&TunnelPlanInput {
        name: "left-a-right-b".to_string(),
        interface_name: "tunab".to_string(),
        kind: TunnelKind::Gre,
        runtime_control: Default::default(),
        runtime_topology: Default::default(),
        left_client_id: "left-a".to_string(),
        right_client_id: "right-b".to_string(),
        left_remote_underlay: "198.51.100.10".to_string(),
        right_remote_underlay: "203.0.113.20".to_string(),
        left_local_underlay: None,
        right_local_underlay: None,
        address_pool_cidr: "10.255.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(TunnelAddressPair {
            left: "10.255.0.0".to_string(),
            right: "10.255.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth_mbps: 100,
        left_mtu: vpsman_common::default_tunnel_mtu(TunnelKind::Gre),
        right_mtu: vpsman_common::default_tunnel_mtu(TunnelKind::Gre),
        ospf: None,
    })
    .unwrap();
    serde_json::to_value(JobCommand::NetworkSpeedTest {
        plan_id: "11111111-2222-4333-8444-555555555555".to_string(),
        plan: Box::new(plan),
        server_side: TunnelEndpointSide::Left,
        duration_secs: 3,
        max_bytes: 16 * 1024 * 1024,
        rate_limit_kbps: 100_000,
        port: 5201,
        connect_timeout_ms: 5000,
    })
    .unwrap()
}

#[test]
fn schedule_catch_up_run_count_is_bounded() {
    assert_eq!(
        catch_up_run_count(&schedule_with_policy("skip_missed", 1), 50),
        1
    );
    assert_eq!(
        catch_up_run_count(&schedule_with_policy("run_once", 1), 50),
        1
    );
    assert_eq!(
        catch_up_run_count(&schedule_with_policy("run_all_limited", 4), 50),
        4
    );
    assert_eq!(
        catch_up_run_count(&schedule_with_policy("run_all_limited", 30), 50),
        25
    );
    assert_eq!(
        catch_up_run_count(&schedule_with_policy("run_all_limited", 4), 0),
        1
    );
}

#[test]
fn schedule_cron_catch_up_counts_missed_runs() {
    let schedule = schedule_with_policy("run_all_limited", 4);
    let now = date_time_from_unix(schedule.next_run_at_unix + 180).unwrap();
    assert_eq!(calculate_due_occurrences(&schedule, now).unwrap(), 4);
}

#[test]
fn schedule_cron_next_run_advances_from_policy_cursor() {
    let mut schedule = schedule_with_policy("run_once", 1);
    let now = date_time_from_unix(schedule.next_run_at_unix + 3600).unwrap();
    assert_eq!(
        next_run_after_success(&schedule, 1, now)
            .unwrap()
            .timestamp(),
        schedule.next_run_at_unix + 60
    );

    schedule.catch_up_policy = "skip_missed".to_string();
    assert!(next_run_after_success(&schedule, 1, now).unwrap() > now);
}

#[test]
fn schedule_error_is_bounded() {
    let error = "x".repeat(1200);
    assert_eq!(truncate_schedule_error(&error).len(), 1024);
}

#[test]
fn schedule_max_timeout_uses_configured_value_without_agent_cap_clamp() {
    let targets = vec!["edge-a".to_string(), "edge-b".to_string()];
    let capabilities = vec![
        TargetCapability {
            client_id: "edge-a".to_string(),
            arch: Some("x86_64".to_string()),
            capabilities: AgentCapabilitySnapshot {
                max_job_timeout_secs: 20,
                ..AgentCapabilitySnapshot::default()
            },
        },
        TargetCapability {
            client_id: "edge-b".to_string(),
            arch: Some("aarch64".to_string()),
            capabilities: AgentCapabilitySnapshot {
                max_job_timeout_secs: 120,
                ..AgentCapabilitySnapshot::default()
            },
        },
    ];

    assert_eq!(
        effective_schedule_max_timeout_secs(
            90,
            DEFAULT_MAX_JOB_TIMEOUT_SECS,
            &targets,
            &capabilities
        ),
        90
    );
    assert_eq!(
        effective_schedule_max_timeout_secs(
            10,
            DEFAULT_MAX_JOB_TIMEOUT_SECS,
            &targets,
            &capabilities
        ),
        10
    );
    assert_eq!(
        effective_schedule_max_timeout_secs(90, DEFAULT_MAX_JOB_TIMEOUT_SECS, &[], &[]),
        90
    );
    assert_eq!(
        effective_schedule_max_timeout_secs(7_200, 7_200, &[], &[]),
        7_200
    );
}

#[test]
fn schedule_selector_expression_matches_clients() {
    let expression = parse_expression("provider:alpha && (country:US || id:edge-b)")
        .unwrap()
        .unwrap();
    let context = ExpressionContext::for_vps(VpsMetadata {
        id: "edge-a".to_string(),
        display_name: "edge-a".to_string(),
        status: "online".to_string(),
        tags: vec!["provider:alpha".to_string(), "country:US".to_string()],
        ..VpsMetadata::default()
    });
    assert!(expression_matches(&context, &expression));
}

#[test]
fn schedule_cadence_validation_distinguishes_legacy_failures() {
    let now = Utc::now();
    assert_eq!(schedule_cadence_error("* * * * *", now), None);
    assert_eq!(
        schedule_cadence_error("0 0 31 2 *", now),
        Some(SCHEDULE_CRON_NO_FUTURE_OCCURRENCE)
    );
    assert_eq!(
        schedule_cadence_error("not a cron", now),
        Some(SCHEDULE_CRON_INVALID)
    );
}

#[tokio::test]
async fn postgres_invalid_cadence_disables_once_without_blocking_valid_schedules() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    insert_worker_client(&db.pool, "cadence-edge", "online", false).await;
    let invalid_id = insert_worker_schedule(
        &db.pool,
        "invalid-cadence-schedule",
        serde_json::json!({"type": "shell", "argv": ["/bin/true"], "pty": false}),
        &["cadence-edge"],
    )
    .await;
    let valid_id = insert_worker_schedule(
        &db.pool,
        "valid-cadence-schedule",
        serde_json::json!({"type": "shell", "argv": ["/bin/true"], "pty": false}),
        &["cadence-edge"],
    )
    .await;
    sqlx::query("UPDATE schedules SET cron_expr = '0 0 31 2 *' WHERE id = $1")
        .bind(invalid_id)
        .execute(&db.pool)
        .await
        .unwrap();

    let processed = process_due_schedules(
        &db.pool,
        1,
        &ScheduleDispatchConfig::new(60, DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
    )
    .await
    .unwrap();
    assert_eq!(processed, 1);

    let invalid =
        sqlx::query("SELECT enabled, failure_count, last_error FROM schedules WHERE id = $1")
            .bind(invalid_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(!invalid.try_get::<bool, _>("enabled").unwrap());
    assert_eq!(invalid.try_get::<i32, _>("failure_count").unwrap(), 0);
    assert_eq!(
        invalid
            .try_get::<Option<String>, _>("last_error")
            .unwrap()
            .as_deref(),
        Some(SCHEDULE_CRON_NO_FUTURE_OCCURRENCE)
    );
    let invalid_jobs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE source_schedule_id = $1")
            .bind(invalid_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let valid_jobs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE source_schedule_id = $1")
            .bind(valid_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(invalid_jobs, 0);
    assert_eq!(valid_jobs, 1);
    let invalid_audit: SqlJson<Value> = sqlx::query_scalar(
        "SELECT metadata FROM audit_logs WHERE action = 'schedule.due_failed' AND target = $1",
    )
    .bind(format!("schedule:{invalid_id}"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(invalid_audit.0["origin_kind"], "worker");
    assert_eq!(invalid_audit.0["component"], "schedule-dispatch-worker");
    assert_eq!(invalid_audit.0["result"], "failed");
    assert!(invalid_audit.0["operator_id"].is_string());
    assert!(invalid_audit.0["operator_username"].is_string());
    assert_eq!(invalid_audit.0["operator_role"], "operator");
    // The valid minute schedule can legitimately become due again when these
    // two calls straddle a minute boundary. Re-run the shared dispatcher, but
    // assert idempotence only on the invalid schedule this test owns.
    process_due_schedules(
        &db.pool,
        10,
        &ScheduleDispatchConfig::new(60, DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
    )
    .await
    .unwrap();
    let repeated_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs WHERE action = 'schedule.due_failed' AND target = $1",
    )
    .bind(format!("schedule:{invalid_id}"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(repeated_audits, 1);
    let repeated_invalid_jobs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE source_schedule_id = $1")
            .bind(invalid_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(repeated_invalid_jobs, 0);
    let repeated_invalid_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM webhook_events WHERE kind = 'schedule.failed' AND event_id LIKE $1",
    )
    .bind(format!("schedule:{invalid_id}:invalid_cadence:%"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(repeated_invalid_events, 1);

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_cron_round_drains_starting_page_and_excludes_later_schedules() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    insert_worker_client(&db.pool, "cron-round", "online", false).await;
    let starting_id = insert_worker_schedule(
        &db.pool,
        "cron-round-starting",
        serde_json::json!({"type": "shell", "argv": ["/bin/true"], "pty": false}),
        &["cron-round"],
    )
    .await;
    let round_started_at: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let later_id = insert_worker_schedule(
        &db.pool,
        "cron-round-later",
        serde_json::json!({"type": "shell", "argv": ["/bin/true"], "pty": false}),
        &["cron-round"],
    )
    .await;
    sqlx::query("UPDATE schedules SET created_at=$2 + interval '1 second' WHERE id=$1")
        .bind(later_id)
        .bind(round_started_at)
        .execute(&db.pool)
        .await
        .unwrap();

    let processed = process_due_schedules_through(
        &db.pool,
        1,
        &ScheduleDispatchConfig::new(60, DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
        round_started_at,
    )
    .await
    .unwrap();
    assert_eq!(processed, 1);
    let (starting_jobs, later_jobs): (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            count(*) FILTER (WHERE source_schedule_id=$1),
            count(*) FILTER (WHERE source_schedule_id=$2)
        FROM jobs
        "#,
    )
    .bind(starting_id)
    .bind(later_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!((starting_jobs, later_jobs), (1, 0));
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_webhook_rule_failures_do_not_poison_event_batch() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    for index in 0..5 {
        insert_worker_client(&db.pool, &format!("webhook-edge-{index}"), "online", false).await;
    }
    let good_rule =
        insert_worker_webhook_rule(&db.pool, "valid-webhook-rule", "id:webhook-edge-0", "").await;
    let invalid_expression_rule =
        insert_worker_webhook_rule(&db.pool, "invalid-expression-webhook-rule", "(tag:edge", "")
            .await;
    let invalid_template_rule = insert_worker_webhook_rule(
        &db.pool,
        "invalid-template-webhook-rule",
        "status = online",
        "[if alert.triggered]missing end",
    )
    .await;
    let expanding_template = format!("[for v in matched_vps]{}[endfor]", "x".repeat(3_900));
    let render_failure_rule = insert_worker_webhook_rule(
        &db.pool,
        "render-failure-webhook-rule",
        "status = online",
        &expanding_template,
    )
    .await;
    let event_id = "webhook-poison-regression";
    webhook_rules::insert_webhook_event(
        &db.pool,
        "agent.test",
        event_id,
        &["agent.test"],
        &[],
        serde_json::json!({
            "event": {
                "kind": "agent.test",
                "id": event_id,
            }
        }),
    )
    .await
    .unwrap();

    assert_eq!(
        webhook_rules::materialize_interval_events(&db.pool, WebhookRuleWorkerConfig::default(),)
            .await
            .unwrap(),
        2
    );
    let result =
        webhook_rules::process_webhook_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap();
    assert_eq!(result, 2);

    let deliveries = sqlx::query_as::<_, (Uuid, String, String, String, Option<String>)>(
        r#"
        SELECT rule_id, status, event_kind, event_id, error
        FROM webhook_rule_deliveries
        ORDER BY rule_id
        "#,
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(deliveries.len(), 4);
    let delivery_for = |rule_id| {
        deliveries
            .iter()
            .find(|delivery| delivery.0 == rule_id)
            .expect("rule delivery evidence is present")
    };

    let good = delivery_for(good_rule);
    assert_eq!(good.1, "queued");
    assert_eq!(good.2, "agent.test");
    assert_eq!(good.3, event_id);
    assert_eq!(good.4, None);

    let invalid_expression = delivery_for(invalid_expression_rule);
    assert_eq!(invalid_expression.1, "permanently_failed");
    assert_eq!(invalid_expression.2, "webhook.rule_configuration");
    assert!(invalid_expression
        .3
        .starts_with("webhook-rule-configuration:"));
    assert!(invalid_expression
        .4
        .as_deref()
        .is_some_and(|error| error.starts_with("invalid webhook rule expression:")));

    let invalid_template = delivery_for(invalid_template_rule);
    assert_eq!(invalid_template.1, "permanently_failed");
    assert_eq!(invalid_template.2, "webhook.rule_configuration");
    assert!(invalid_template
        .4
        .as_deref()
        .is_some_and(|error| error.starts_with("invalid webhook rule template:")));

    let render_failure = delivery_for(render_failure_rule);
    assert_eq!(render_failure.1, "permanently_failed");
    assert_eq!(render_failure.2, "agent.test");
    assert_eq!(render_failure.3, event_id);
    assert!(render_failure
        .4
        .as_deref()
        .is_some_and(|error| error.contains("rendered message exceeds length limit")));

    let event_processed: bool = sqlx::query_scalar(
        "SELECT processed_at IS NOT NULL FROM webhook_events WHERE event_id = $1",
    )
    .bind(event_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(event_processed);
    let open_failure_alerts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fleet_alert_states WHERE alert_id LIKE 'webhook_delivery:%' AND state = 'open'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(open_failure_alerts, 0);
    let permanent_failure_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs WHERE action = 'webhook.rule_delivery_permanently_failed'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(permanent_failure_audits, 3);

    assert_eq!(
        webhook_rules::process_webhook_events(&db.pool, WebhookRuleWorkerConfig::default(),)
            .await
            .unwrap(),
        0
    );
    let delivery_count: i64 = sqlx::query_scalar("SELECT count(*) FROM webhook_rule_deliveries")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(delivery_count, 4);

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_due_schedule_skips_unavailable_fixed_targets() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    insert_worker_client(&db.pool, "edge-a", "online", false).await;
    insert_worker_client(&db.pool, "edge-b", "deleted", true).await;
    insert_worker_client(&db.pool, "edge-c", "online", false).await;
    let schedule_id = insert_worker_schedule(
        &db.pool,
        "missing-target-schedule",
        serde_json::json!({"type": "shell", "argv": ["/bin/true"], "pty": false}),
        &["edge-a", "edge-b", "edge-c"],
    )
    .await;

    let processed = process_due_schedule(
        &db.pool,
        schedule_id,
        &ScheduleDispatchConfig::new(60, DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
    )
    .await
    .unwrap();

    assert_eq!(processed, 1);
    let (job_id, status, failure_count, last_error) = schedule_result(&db.pool, schedule_id).await;
    assert_eq!(status.as_deref(), Some(JOB_STATUS_QUEUED));
    assert_eq!(failure_count, 0);
    assert_eq!(last_error, None);
    let targets = job_targets(&db.pool, job_id).await;
    assert_eq!(
        targets,
        vec![
            ("edge-a".to_string(), TARGET_STATUS_QUEUED.to_string(), None),
            (
                "edge-b".to_string(),
                TARGET_STATUS_SKIPPED.to_string(),
                Some("fixed_target_unavailable: schedule target skipped".to_string())
            ),
            ("edge-c".to_string(), TARGET_STATUS_QUEUED.to_string(), None),
        ]
    );
    let output = job_status_output(&db.pool, job_id, "edge-b").await;
    assert_eq!(output["type"], "schedule_target_skipped");
    assert_eq!(output["reason"], "fixed_target_unavailable");
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_due_schedule_neutrally_skips_suspended_fixed_targets() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    insert_worker_client(&db.pool, "edge-a", "online", false).await;
    insert_worker_client(&db.pool, "edge-b", "offline", false).await;
    sqlx::query(
        r#"
        UPDATE clients
        SET status='suspended', suspended_at=clock_timestamp(),
            suspended_from_status='offline'
        WHERE id='edge-b'
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let schedule_id = insert_worker_schedule(
        &db.pool,
        "suspended-target-schedule",
        serde_json::json!({"type": "shell", "argv": ["/bin/true"], "pty": false}),
        &["edge-a", "edge-b"],
    )
    .await;

    let processed = process_due_schedule(
        &db.pool,
        schedule_id,
        &ScheduleDispatchConfig::new(60, DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
    )
    .await
    .unwrap();

    assert_eq!(processed, 1);
    let (job_id, status, failure_count, last_error) = schedule_result(&db.pool, schedule_id).await;
    assert_eq!(status.as_deref(), Some(JOB_STATUS_QUEUED));
    assert_eq!(failure_count, 0);
    assert_eq!(last_error, None);
    let targets = job_targets(&db.pool, job_id).await;
    assert_eq!(
        targets,
        vec![
            ("edge-a".to_string(), TARGET_STATUS_QUEUED.to_string(), None),
            (
                "edge-b".to_string(),
                TARGET_STATUS_SKIPPED.to_string(),
                Some("target_suspended: target skipped because VPS is suspended".to_string())
            ),
        ]
    );
    let output = job_status_output(&db.pool, job_id, "edge-b").await;
    assert_eq!(output["type"], "target_suspended");
    assert_eq!(output["reason"], "target_suspended");
    let audit_suspended_targets: serde_json::Value = sqlx::query_scalar(
        r#"
        SELECT metadata->'suspended_fixed_targets'
        FROM audit_logs
        WHERE action='schedule.due_materialized'
          AND target=$1
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(format!("schedule:{schedule_id}"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(audit_suspended_targets, serde_json::json!(["edge-b"]));
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_due_schedule_skips_never_connected_fixed_targets() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    insert_worker_client(&db.pool, "edge-a", "online", false).await;
    insert_worker_client_with_incarnation(&db.pool, "edge-b", "never", false, None).await;
    let schedule_id = insert_worker_schedule(
        &db.pool,
        "never-connected-schedule",
        serde_json::json!({"type": "shell", "argv": ["/bin/true"], "pty": false}),
        &["edge-a", "edge-b"],
    )
    .await;

    let processed = process_due_schedule(
        &db.pool,
        schedule_id,
        &ScheduleDispatchConfig::new(60, DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
    )
    .await
    .unwrap();

    assert_eq!(processed, 1);
    let (job_id, status, failure_count, last_error) = schedule_result(&db.pool, schedule_id).await;
    assert_eq!(status.as_deref(), Some(JOB_STATUS_QUEUED));
    assert_eq!(failure_count, 0);
    assert_eq!(last_error, None);
    let targets = job_targets(&db.pool, job_id).await;
    assert_eq!(
        targets,
        vec![
            ("edge-a".to_string(), TARGET_STATUS_QUEUED.to_string(), None),
            (
                "edge-b".to_string(),
                TARGET_STATUS_SKIPPED.to_string(),
                Some("target_never_connected: schedule target skipped".to_string())
            ),
        ]
    );
    let output = job_status_output(&db.pool, job_id, "edge-b").await;
    assert_eq!(output["type"], "schedule_target_skipped");
    assert_eq!(output["reason"], "target_never_connected");
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_due_schedule_speed_test_skips_both_endpoints_when_peer_is_unavailable() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    insert_worker_client(&db.pool, "left-a", "online", false).await;
    insert_worker_client_with_incarnation(&db.pool, "right-b", "never", false, None).await;
    let schedule_id = insert_worker_schedule(
        &db.pool,
        "speed-test-peer-unavailable-schedule",
        scheduled_speed_test_operation(),
        &["left-a", "right-b"],
    )
    .await;

    let processed = process_due_schedule(
        &db.pool,
        schedule_id,
        &ScheduleDispatchConfig::new(60, DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
    )
    .await
    .unwrap();

    assert_eq!(processed, 1);
    let (job_id, status, failure_count, last_error) = schedule_result(&db.pool, schedule_id).await;
    assert_eq!(status.as_deref(), Some(JOB_STATUS_SKIPPED));
    assert_eq!(failure_count, 0);
    assert_eq!(last_error, None);
    let targets = job_targets(&db.pool, job_id).await;
    assert_eq!(
        targets,
        vec![
            (
                "left-a".to_string(),
                TARGET_STATUS_SKIPPED.to_string(),
                Some("network_speed_test_peer_unavailable: peer target was skipped; speed test requires both endpoints".to_string())
            ),
            (
                "right-b".to_string(),
                TARGET_STATUS_SKIPPED.to_string(),
                Some("target_never_connected: schedule target skipped".to_string())
            ),
        ]
    );
    let left_output = job_status_output(&db.pool, job_id, "left-a").await;
    assert_eq!(left_output["type"], "network_speed_test_peer_unavailable");
    assert_eq!(left_output["reason"], "network_speed_test_peer_unavailable");
    let right_output = job_status_output(&db.pool, job_id, "right-b").await;
    assert_eq!(right_output["type"], "schedule_target_skipped");
    assert_eq!(right_output["reason"], "target_never_connected");
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_due_schedule_records_missing_fixed_targets_as_skipped_rows() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    insert_worker_client(&db.pool, "edge-a", "online", false).await;
    let schedule_id = insert_worker_schedule(
        &db.pool,
        "missing-fixed-target-schedule",
        serde_json::json!({"type": "shell", "argv": ["/bin/true"], "pty": false}),
        &["edge-a", "edge-missing"],
    )
    .await;

    let processed = process_due_schedule(
        &db.pool,
        schedule_id,
        &ScheduleDispatchConfig::new(60, DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
    )
    .await
    .unwrap();

    assert_eq!(processed, 1);
    let (job_id, status, failure_count, last_error) = schedule_result(&db.pool, schedule_id).await;
    assert_eq!(status.as_deref(), Some(JOB_STATUS_QUEUED));
    assert_eq!(failure_count, 0);
    assert_eq!(last_error, None);
    let targets = job_targets(&db.pool, job_id).await;
    assert_eq!(
        targets,
        vec![
            ("edge-a".to_string(), TARGET_STATUS_QUEUED.to_string(), None),
            (
                "edge-missing".to_string(),
                TARGET_STATUS_SKIPPED.to_string(),
                Some("fixed_target_missing: schedule target skipped".to_string())
            ),
        ]
    );
    let output = job_status_output(&db.pool, job_id, "edge-missing").await;
    assert_eq!(output["type"], "schedule_target_skipped");
    assert_eq!(output["reason"], "fixed_target_missing");
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_due_schedule_materializes_canonical_command_hash_and_operation() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    insert_worker_client(&db.pool, "edge-a", "online", false).await;
    let schedule_id = insert_worker_schedule(
        &db.pool,
        "canonical-scheduled-shell",
        serde_json::json!({
            "pty": false,
            "argv": ["/bin/sh", "-c", "printf scheduled"],
            "type": "shell",
        }),
        &["edge-a"],
    )
    .await;

    let processed = process_due_schedule(
        &db.pool,
        schedule_id,
        &ScheduleDispatchConfig::new(60, DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
    )
    .await
    .unwrap();

    assert_eq!(processed, 1);
    let (job_id, status, failure_count, last_error) = schedule_result(&db.pool, schedule_id).await;
    assert_eq!(status.as_deref(), Some(JOB_STATUS_QUEUED));
    assert_eq!(failure_count, 0);
    assert_eq!(last_error, None);
    let expected_operation = JobCommand::Shell {
        argv: vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "printf scheduled".to_string(),
        ],
        pty: false,
    };
    let expected_payload_hash = payload_hash(&encode_json(&expected_operation).unwrap());
    let row = sqlx::query(
        r#"
        SELECT payload_hash, operation
        FROM jobs
        WHERE id = $1
        "#,
    )
    .bind(job_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let stored_payload_hash: String = row.try_get("payload_hash").unwrap();
    let stored_operation: SqlJson<JobCommand> = row.try_get("operation").unwrap();
    assert_eq!(stored_payload_hash, expected_payload_hash);
    assert_eq!(
        encode_json(&stored_operation.0).unwrap(),
        encode_json(&expected_operation).unwrap()
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_due_schedule_with_all_unavailable_targets_is_skipped() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    insert_worker_client(&db.pool, "edge-a", "deleted", true).await;
    insert_worker_client(&db.pool, "edge-b", "revoked", false).await;
    let schedule_id = insert_worker_schedule(
        &db.pool,
        "all-unavailable-schedule",
        serde_json::json!({"type": "shell", "argv": ["/bin/true"], "pty": false}),
        &["edge-a", "edge-b"],
    )
    .await;

    let processed = process_due_schedule(
        &db.pool,
        schedule_id,
        &ScheduleDispatchConfig::new(60, DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
    )
    .await
    .unwrap();

    assert_eq!(processed, 1);
    let (job_id, status, failure_count, last_error) = schedule_result(&db.pool, schedule_id).await;
    assert_eq!(status.as_deref(), Some(JOB_STATUS_SKIPPED));
    assert_eq!(failure_count, 0);
    assert_eq!(last_error, None);
    let targets = job_targets(&db.pool, job_id).await;
    assert!(targets
        .iter()
        .all(|(_, status, _)| status == TARGET_STATUS_SKIPPED));
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_scheduled_update_skips_busy_targets() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    insert_worker_client(&db.pool, "edge-a", "online", false).await;
    insert_worker_client(&db.pool, "edge-b", "online", false).await;
    insert_active_worker_target(&db.pool, "edge-a").await;
    let schedule_id = insert_worker_schedule(
        &db.pool,
        "busy-update-schedule",
        serde_json::json!({
            "type": "agent_update",
            "artifact_url": "https://updates.example.invalid/agent",
            "sha256_hex": "a".repeat(64),
        }),
        &["edge-a", "edge-b"],
    )
    .await;

    let processed = process_due_schedule(
        &db.pool,
        schedule_id,
        &ScheduleDispatchConfig::new(60, DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
    )
    .await
    .unwrap();

    assert_eq!(processed, 1);
    let (job_id, status, failure_count, last_error) = schedule_result(&db.pool, schedule_id).await;
    assert_eq!(status.as_deref(), Some(JOB_STATUS_QUEUED));
    assert_eq!(failure_count, 0);
    assert_eq!(last_error, None);
    let targets = job_targets(&db.pool, job_id).await;
    assert_eq!(
        targets,
        vec![
            (
                "edge-a".to_string(),
                TARGET_STATUS_SKIPPED.to_string(),
                Some(
                    "busy_agent_active_jobs: target has another active job; update skipped"
                        .to_string()
                )
            ),
            ("edge-b".to_string(), TARGET_STATUS_QUEUED.to_string(), None),
        ]
    );
    let output = job_status_output(&db.pool, job_id, "edge-a").await;
    assert_eq!(output["type"], "busy_update_skipped");
    assert_eq!(output["reason"], "busy_agent_active_jobs");
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_scheduled_capability_skip_records_neutral_policy_evidence() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    insert_worker_client(&db.pool, "edge-unprivileged", "online", false).await;
    let capabilities = AgentCapabilitySnapshot {
        privilege_mode: AgentPrivilegeMode::Unprivileged,
        ..AgentCapabilitySnapshot::default()
    };
    sqlx::query("UPDATE clients SET capabilities = $2 WHERE id = $1")
        .bind("edge-unprivileged")
        .bind(SqlJson(capabilities))
        .execute(&db.pool)
        .await
        .unwrap();
    let schedule_id = insert_worker_schedule(
        &db.pool,
        "capability-degraded-schedule",
        serde_json::json!({"type": "agent_update_check"}),
        &["edge-unprivileged"],
    )
    .await;

    let processed = process_due_schedule(
        &db.pool,
        schedule_id,
        &ScheduleDispatchConfig::new(60, DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
    )
    .await
    .unwrap();

    assert_eq!(processed, 1);
    let (job_id, status, _, last_error) = schedule_result(&db.pool, schedule_id).await;
    assert_eq!(status.as_deref(), Some(JOB_STATUS_SKIPPED));
    assert_eq!(last_error, None);
    let row = sqlx::query(
        r#"
        SELECT capability_degraded_reason, capability_degraded_hint
        FROM job_targets
        WHERE job_id = $1 AND client_id = 'edge-unprivileged'
        "#,
    )
    .bind(job_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        row.try_get::<Option<String>, _>("capability_degraded_reason")
            .unwrap()
            .as_deref(),
        Some("target_agent_lacks_agent_update_capability")
    );
    assert!(row
        .try_get::<Option<String>, _>("capability_degraded_hint")
        .unwrap()
        .is_some_and(|hint| hint.contains("agent update was not dispatched")));
    let output = job_status_output(&db.pool, job_id, "edge-unprivileged").await;
    assert_eq!(output["type"], "capability_degraded");
    assert_eq!(output["command_type"], "agent_update_check");

    let job_lineage = sqlx::query("SELECT causation_id, schedule_lineage FROM jobs WHERE id = $1")
        .bind(job_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let expected_causation_id = job_lineage
        .try_get::<Option<Uuid>, _>("causation_id")
        .unwrap();
    let expected_schedule_lineage = job_lineage
        .try_get::<Vec<Uuid>, _>("schedule_lineage")
        .unwrap();
    let evidence = sqlx::query(
        r#"
        SELECT id, source_event_id, fact_kind, natural_key,
               confirmation_bucket_key, subject_client_id, target_kind,
               target_id, source_status, completeness, subject_snapshot,
               payload, state_started_at, causation_id, schedule_lineage,
               evaluation_pending
        FROM alert_policy_evidence
        WHERE source_kind = 'job.capability' AND natural_key = $1
        "#,
    )
    .bind(format!("{job_id}:edge-unprivileged"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let evidence_id: Uuid = evidence.try_get("id").unwrap();
    assert_eq!(
        evidence.try_get::<String, _>("source_event_id").unwrap(),
        format!("{job_id}:edge-unprivileged")
    );
    assert_eq!(
        evidence.try_get::<String, _>("fact_kind").unwrap(),
        "occurrence"
    );
    assert!(evidence.try_get::<bool, _>("evaluation_pending").unwrap());
    assert_eq!(
        evidence
            .try_get::<String, _>("confirmation_bucket_key")
            .unwrap(),
        format!("{job_id}:edge-unprivileged")
    );
    assert_eq!(
        evidence
            .try_get::<Option<String>, _>("subject_client_id")
            .unwrap(),
        Some("edge-unprivileged".to_string())
    );
    assert_eq!(
        evidence.try_get::<String, _>("target_kind").unwrap(),
        "job_target"
    );
    assert_eq!(
        evidence.try_get::<String, _>("target_id").unwrap(),
        format!("{job_id}:edge-unprivileged")
    );
    assert_eq!(
        evidence.try_get::<String, _>("source_status").unwrap(),
        "skipped"
    );
    assert_eq!(
        evidence.try_get::<String, _>("completeness").unwrap(),
        "complete"
    );
    assert!(evidence
        .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("state_started_at")
        .unwrap()
        .is_none());
    assert_eq!(
        evidence.try_get::<Option<Uuid>, _>("causation_id").unwrap(),
        expected_causation_id
    );
    assert_eq!(
        evidence
            .try_get::<Vec<Uuid>, _>("schedule_lineage")
            .unwrap(),
        expected_schedule_lineage
    );
    let subject_snapshot = evidence
        .try_get::<serde_json::Value, _>("subject_snapshot")
        .unwrap();
    assert_eq!(subject_snapshot["client_id"], "edge-unprivileged");
    assert_eq!(subject_snapshot["display_name"], "edge-unprivileged");
    let payload = evidence.try_get::<serde_json::Value, _>("payload").unwrap();
    assert_eq!(payload["status"], "skipped");
    assert_eq!(
        payload["source_status"],
        "target_agent_lacks_agent_update_capability"
    );
    assert_eq!(payload["client_id"], "edge-unprivileged");
    assert_eq!(payload["job_id"], job_id.to_string());
    assert_eq!(payload["command_type"], "scheduled_agent_update_check");
    assert_eq!(
        payload["reason"], payload["hint"],
        "neutral reason mirrors the source capability hint"
    );

    let mut retry = db.pool.begin().await.unwrap();
    reconcile_scheduled_job_event_sources_in_tx(&mut retry, job_id)
        .await
        .unwrap();
    retry.commit().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            r#"
            SELECT count(*)
            FROM alert_policy_evidence
            WHERE source_kind = 'job.capability' AND natural_key = $1
            "#,
        )
        .bind(format!("{job_id}:edge-unprivileged"))
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM alert_policy_evidence WHERE source_kind = 'job.capability' AND natural_key = $1",
        )
            .bind(format!("{job_id}:edge-unprivileged"))
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        evidence_id,
        "schedule retries must retain the original immutable occurrence fact"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            r#"
            SELECT count(*)
            FROM webhook_events
            WHERE kind LIKE 'alert.%'
               OR EXISTS (
                    SELECT 1 FROM unnest(event_predicates) predicate
                    WHERE predicate LIKE 'alert.%'
                  )
            "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0,
        "schedule facts must not publish retired alert lifecycle aliases"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_scheduled_terminal_job_evidence_preserves_lineage_and_dedupes() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let job_id = Uuid::new_v4();
    let causation_id = Uuid::new_v4();
    let lineage = vec![Uuid::new_v4(), Uuid::new_v4()];
    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, command_type, privileged, status, target_count, payload_hash,
            operation, request_fingerprint, max_timeout_secs, completed_at,
            causation_id, schedule_lineage
        ) VALUES (
            $1, 'scheduled_shell', TRUE, 'failed', 0, $2,
            '{}'::jsonb, $3, 60, clock_timestamp(), $4, $5
        )
        "#,
    )
    .bind(job_id)
    .bind(format!("hash-{job_id}"))
    .bind(format!("fingerprint-{job_id}"))
    .bind(causation_id)
    .bind(&lineage)
    .execute(&db.pool)
    .await
    .unwrap();

    let mut first = db.pool.begin().await.unwrap();
    reconcile_scheduled_job_event_sources_in_tx(&mut first, job_id)
        .await
        .unwrap();
    first.commit().await.unwrap();
    let evidence = sqlx::query(
        r#"
        SELECT id, source_event_id, fact_kind, natural_key,
               confirmation_bucket_key, subject_client_id, target_kind,
               target_id, source_status, completeness, subject_snapshot,
               payload, state_started_at, causation_id, schedule_lineage
        FROM alert_policy_evidence
        WHERE source_kind = 'job.terminal' AND natural_key = $1
        "#,
    )
    .bind(job_id.to_string())
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let evidence_id: Uuid = evidence.try_get("id").unwrap();
    assert_eq!(
        evidence.try_get::<String, _>("source_event_id").unwrap(),
        job_id.to_string()
    );
    assert_eq!(
        evidence.try_get::<String, _>("fact_kind").unwrap(),
        "occurrence"
    );
    assert_eq!(
        evidence
            .try_get::<String, _>("confirmation_bucket_key")
            .unwrap(),
        job_id.to_string()
    );
    assert!(evidence
        .try_get::<Option<String>, _>("subject_client_id")
        .unwrap()
        .is_none());
    assert_eq!(evidence.try_get::<String, _>("target_kind").unwrap(), "job");
    assert_eq!(
        evidence.try_get::<String, _>("target_id").unwrap(),
        job_id.to_string()
    );
    assert_eq!(
        evidence.try_get::<String, _>("source_status").unwrap(),
        "failed"
    );
    assert_eq!(
        evidence.try_get::<String, _>("completeness").unwrap(),
        "complete"
    );
    assert_eq!(
        evidence
            .try_get::<serde_json::Value, _>("subject_snapshot")
            .unwrap(),
        serde_json::json!({})
    );
    assert!(evidence
        .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("state_started_at")
        .unwrap()
        .is_none());
    assert_eq!(
        evidence.try_get::<Option<Uuid>, _>("causation_id").unwrap(),
        Some(causation_id)
    );
    let mut canonical_lineage = lineage.clone();
    canonical_lineage.sort_unstable();
    assert_eq!(
        evidence
            .try_get::<Vec<Uuid>, _>("schedule_lineage")
            .unwrap(),
        canonical_lineage
    );
    let payload = evidence.try_get::<serde_json::Value, _>("payload").unwrap();
    assert_eq!(payload["status"], "failed");
    assert_eq!(payload["source_status"], "failed");
    assert_eq!(payload["client_id"], serde_json::Value::Null);
    assert_eq!(payload["job_id"], job_id.to_string());
    assert_eq!(payload["command_type"], "scheduled_shell");
    assert_eq!(payload["target_count"], 0);
    assert_eq!(payload["causation_id"], causation_id.to_string());

    let mut retry = db.pool.begin().await.unwrap();
    reconcile_scheduled_job_event_sources_in_tx(&mut retry, job_id)
        .await
        .unwrap();
    retry.commit().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM alert_policy_evidence WHERE source_kind = 'job.terminal' AND natural_key = $1",
        )
        .bind(job_id.to_string())
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        evidence_id
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM alert_policy_evidence WHERE source_kind = 'job.terminal' AND natural_key = $1",
        )
        .bind(job_id.to_string())
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        1
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_scheduled_update_all_busy_targets_is_skipped() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    insert_worker_client(&db.pool, "edge-a", "online", false).await;
    insert_worker_client(&db.pool, "edge-b", "online", false).await;
    insert_active_worker_target(&db.pool, "edge-a").await;
    insert_active_worker_target(&db.pool, "edge-b").await;
    let schedule_id = insert_worker_schedule(
        &db.pool,
        "all-busy-update-schedule",
        serde_json::json!({
            "type": "agent_update_check",
        }),
        &["edge-a", "edge-b"],
    )
    .await;

    let processed = process_due_schedule(
        &db.pool,
        schedule_id,
        &ScheduleDispatchConfig::new(60, DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
    )
    .await
    .unwrap();

    assert_eq!(processed, 1);
    let (job_id, status, _, last_error) = schedule_result(&db.pool, schedule_id).await;
    assert_eq!(status.as_deref(), Some(JOB_STATUS_SKIPPED));
    assert_eq!(last_error, None);
    let targets = job_targets(&db.pool, job_id).await;
    assert!(targets
        .iter()
        .all(|(_, status, _)| status == TARGET_STATUS_SKIPPED));
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_strict_scheduled_update_policy_is_hash_bound() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let artifact_sha = "12".repeat(32);
    let rollback_sha = "34".repeat(32);
    insert_worker_agent_update_release(&db.pool, &artifact_sha, Some(&rollback_sha)).await;
    let mut tx = db.pool.begin().await.unwrap();
    let policy_targets = vec!["client-a".to_string()];
    let policy_capabilities = vec![TargetCapability {
        client_id: "client-a".to_string(),
        arch: Some("x86_64".to_string()),
        capabilities: AgentCapabilitySnapshot::default(),
    }];

    assert!(scheduled_agent_update_release_policy_allows(
        &mut tx,
        &JobCommand::UpdateAgent {
            artifact_url: "https://updates.example/agent".to_string(),
            sha256_hex: artifact_sha.clone(),
        },
        true,
        &policy_targets,
        &policy_capabilities,
    )
    .await
    .unwrap());
    assert!(scheduled_agent_update_release_policy_allows(
        &mut tx,
        &JobCommand::AgentUpdateActivate {
            staged_sha256_hex: artifact_sha.clone(),
            restart_agent: true,
        },
        true,
        &policy_targets,
        &policy_capabilities,
    )
    .await
    .unwrap());
    assert!(scheduled_agent_update_release_policy_allows(
        &mut tx,
        &JobCommand::AgentUpdateRollback {
            rollback_sha256_hex: Some(rollback_sha.clone()),
        },
        true,
        &policy_targets,
        &policy_capabilities,
    )
    .await
    .unwrap());
    assert!(scheduled_agent_update_release_policy_allows(
        &mut tx,
        &JobCommand::AgentUpdateCheck {
            version_url: Some(local_update_manifest_url(&artifact_sha)),
            activate: true,
            restart_agent: true,
        },
        true,
        &policy_targets,
        &policy_capabilities,
    )
    .await
    .unwrap());
    assert!(scheduled_agent_update_release_policy_allows(
        &mut tx,
        &JobCommand::AgentUpdateCheck {
            version_url: None,
            activate: true,
            restart_agent: true,
        },
        true,
        &policy_targets,
        &policy_capabilities,
    )
    .await
    .unwrap());
    assert!(!scheduled_agent_update_release_policy_allows(
        &mut tx,
        &JobCommand::AgentUpdateRollback {
            rollback_sha256_hex: None,
        },
        true,
        &policy_targets,
        &policy_capabilities,
    )
    .await
    .unwrap());
    assert!(!scheduled_agent_update_release_policy_allows(
        &mut tx,
        &JobCommand::AgentUpdateActivate {
            staged_sha256_hex: "56".repeat(32),
            restart_agent: true,
        },
        true,
        &policy_targets,
        &policy_capabilities,
    )
    .await
    .unwrap());

    tx.rollback().await.unwrap();
    db.cleanup().await;
}

fn local_update_manifest_url(_artifact_sha256_hex: &str) -> String {
    let root =
        std::env::temp_dir().join(format!("vpsman-worker-update-manifest-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let asset_name = vpsman_common::agent_update_asset_name_for_arch("x86_64").unwrap();
    let manifest_path = root.join("version.json");
    let manifest = serde_json::json!({
        "schema_version": 3,
        "project": "vpsman",
        "version": "99.0.0",
        "tag": "v99.0.0",
        "assets": [
            {
                "name": asset_name,
                "download_url": format!("https://updates.example/{asset_name}")
            }
        ]
    });
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    format!("file://{}", manifest_path.display())
}

#[test]
fn worker_runtime_config_reloads_suite_file_from_base_args() {
    with_cleared_worker_env(WORKER_HOT_RELOAD_ENV, || {
        let path = temp_suite_config_path("worker-hot-reload");
        let object_dir = path.with_extension("objects");
        std::fs::write(
            &path,
            worker_runtime_toml(
                7,
                333,
                41,
                true,
                5,
                45,
                500,
                9,
                6,
                11,
                3,
                300,
                13,
                true,
                object_dir.to_string_lossy().as_ref(),
            ),
        )
        .unwrap();
        let args = Args::parse_from(["vpsman-worker", "--suite-config", path.to_str().unwrap()]);

        let runtime = load_worker_runtime_config(&args).unwrap();

        assert_eq!(runtime.tick_secs, 7);
        assert_eq!(runtime.agent_offline_timeout_secs, 333);
        assert_eq!(runtime.schedule_dispatch_config.max_timeout_secs, 41);
        assert!(
            runtime
                .schedule_dispatch_config
                .require_registered_agent_updates
        );
        assert_eq!(runtime.alert_notification_config.delivery_limit, 5);
        assert_eq!(runtime.alert_notification_config.retention_days, 45);
        assert_eq!(runtime.alert_notification_config.retention_prune_limit, 500);
        assert_eq!(runtime.alert_notification_config.webhook_timeout_secs, 9);
        assert_eq!(runtime.webhook_rule_config.delivery_limit, 6);
        assert_eq!(runtime.webhook_rule_config.materialize_limit, 11);
        assert_eq!(runtime.webhook_rule_config.retention_days, 3);
        assert_eq!(runtime.webhook_rule_config.retention_prune_limit, 300);
        assert_eq!(runtime.webhook_rule_config.webhook_timeout_secs, 13);
        assert!(runtime.backup_policy_prune_config.enabled);
        assert_eq!(
            runtime
                .backup_policy_prune_config
                .object_store
                .as_ref()
                .map(BackupObjectStore::kind),
            Some("filesystem")
        );
        assert_eq!(runtime.backup_object_store.kind(), "filesystem");

        std::fs::write(
            &path,
            worker_runtime_toml(
                19,
                444,
                55,
                false,
                8,
                60,
                700,
                12,
                10,
                14,
                4,
                400,
                16,
                false,
                object_dir.to_string_lossy().as_ref(),
            ),
        )
        .unwrap();

        let runtime = load_worker_runtime_config(&args).unwrap();
        assert_eq!(runtime.tick_secs, 19);
        assert_eq!(runtime.agent_offline_timeout_secs, 444);
        assert_eq!(runtime.schedule_dispatch_config.max_timeout_secs, 55);
        assert!(
            !runtime
                .schedule_dispatch_config
                .require_registered_agent_updates
        );
        assert_eq!(runtime.alert_notification_config.delivery_limit, 8);
        assert_eq!(runtime.webhook_rule_config.materialize_limit, 14);
        assert!(!runtime.backup_policy_prune_config.enabled);

        let _ = std::fs::remove_file(path);
    });
}

#[test]
fn backup_policy_prune_store_flag_configures_retention_store() {
    with_cleared_worker_env(WORKER_HOT_RELOAD_ENV, || {
        let object_dir =
            temp_suite_config_path("worker-policy-prune-store").with_extension("objects");
        let args = Args::parse_from([
            "vpsman-worker",
            "--backup-policy-prune-enabled",
            "--backup-policy-prune-delete-objects",
            "--backup-policy-prune-object-store-dir",
            object_dir.to_str().unwrap(),
        ]);

        let runtime = WorkerRuntimeConfig::from_args(&args).unwrap();

        assert_eq!(
            runtime
                .backup_policy_prune_config
                .object_store
                .as_ref()
                .map(BackupObjectStore::kind),
            Some("filesystem")
        );
        assert_eq!(runtime.backup_object_store.kind(), "filesystem");
    });
}

#[test]
fn suite_bool_defaults_do_not_disable_explicit_true_flags() {
    let env_name = "VPSMAN_WORKER_APPLY_BOOL_DEFAULT_TEST_UNSET";

    let mut explicit_true = true;
    apply_bool_default(&mut explicit_true, env_name, Some(false));
    assert!(explicit_true);

    let mut default_false = false;
    apply_bool_default(&mut default_false, env_name, Some(true));
    assert!(default_false);
}

async fn wait_for_ready_retention_events_to_drain(pool: &PgPool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let remains: bool = sqlx::query_scalar(
                r#"
                SELECT EXISTS (
                    SELECT 1
                    FROM telemetry_history_due_events
                    WHERE coalesce_ready_at <= now()
                )
                "#,
            )
            .fetch_one(pool)
            .await
            .unwrap();
            if !remains {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("telemetry retention scheduler did not complete a fresh pass")
}

async fn enqueue_ready_retention_proof(pool: &PgPool, minutes_ago: i32) {
    sqlx::query(
        r#"
        INSERT INTO telemetry_history_due_events (
            domain, source_bucket_secs, destination_bucket_secs,
            owner_identity, destination_start, coalesce_ready_at, due_at
        )
        SELECT
            'telemetry_rollups', 60, 300,
            ARRAY['retention-scheduler-isolation'], boundary.bucket_start,
            boundary.bucket_start + interval '5 minutes',
            boundary.bucket_start + interval '2 days 5 minutes'
        FROM (
            SELECT date_bin(
                interval '5 minutes',
                clock_timestamp() - make_interval(mins => $1),
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
            ) AS bucket_start
        ) boundary
        "#,
    )
    .bind(minutes_ago)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn telemetry_retention_scheduler_runs_while_ordinary_pool_is_saturated_and_closes_cleanly() {
    assert_eq!(
        TELEMETRY_HISTORY_RETENTION_RECOVERY_INTERVAL,
        Duration::from_secs(5)
    );
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    enqueue_ready_retention_proof(&db.pool, 10).await;

    let retention_pool = db.telemetry_retention_pool().await.unwrap();
    let retention_plan_mode: String = sqlx::query_scalar("SHOW plan_cache_mode")
        .fetch_one(&retention_pool)
        .await
        .unwrap();
    assert_eq!(retention_plan_mode, "force_custom_plan");
    let retention_pool_observer = retention_pool.clone();
    let (_retention_wake_tx, retention_wake_rx) = telemetry_retention_wake_channel();
    let scheduler = TelemetryRetentionScheduler::spawn(
        retention_pool,
        Duration::from_millis(100),
        retention_wake_rx,
    );
    wait_for_ready_retention_events_to_drain(&db.pool).await;

    let ordinary_pool = db.additional_pool(2).await.unwrap();
    let ordinary_connection_a = ordinary_pool.acquire().await.unwrap();
    let ordinary_connection_b = ordinary_pool.acquire().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(25), ordinary_pool.acquire())
            .await
            .is_err(),
        "test precondition: both ordinary-workflow connections are occupied"
    );
    enqueue_ready_retention_proof(&db.pool, 15).await;
    wait_for_ready_retention_events_to_drain(&db.pool).await;
    scheduler.shutdown().await.unwrap();
    assert!(retention_pool_observer.is_closed());

    drop(ordinary_connection_a);
    drop(ordinary_connection_b);
    ordinary_pool.close().await;
    db.cleanup().await;
}

#[tokio::test]
async fn telemetry_retention_scheduler_exit_or_panic_is_fatal() {
    let make_scheduler = |task| {
        let pool = PgPoolOptions::new()
            .min_connections(0)
            .max_connections(2)
            .connect_lazy("postgresql://localhost/vpsman-scheduler-supervision-test")
            .unwrap();
        let pool_observer = pool.clone();
        let (control_tx, _control_rx) =
            watch::channel(TelemetryRetentionSchedulerControl { shutdown: false });
        (
            TelemetryRetentionScheduler {
                control_tx,
                task: Some(task),
                pool,
            },
            pool_observer,
        )
    };

    let (mut completed_scheduler, completed_pool) = make_scheduler(tokio::spawn(async {}));
    let completion_error = completed_scheduler
        .wait_for_unexpected_exit()
        .await
        .unwrap_err();
    assert!(completion_error
        .to_string()
        .contains("telemetry retention scheduler exited unexpectedly"));
    completed_scheduler.shutdown().await.unwrap();
    assert!(completed_pool.is_closed());

    let (mut panicked_scheduler, panicked_pool) =
        make_scheduler(tokio::spawn(async { panic!("scheduler-test-panic") }));
    let panic_error = panicked_scheduler
        .wait_for_unexpected_exit()
        .await
        .unwrap_err();
    assert!(panic_error
        .to_string()
        .contains("telemetry retention scheduler task failed"));
    panicked_scheduler.shutdown().await.unwrap();
    assert!(panicked_pool.is_closed());
}

async fn insert_worker_client(pool: &PgPool, client_id: &str, status: &str, hidden: bool) {
    insert_worker_client_with_incarnation(pool, client_id, status, hidden, Some(Uuid::new_v4()))
        .await;
}

async fn insert_worker_client_with_incarnation(
    pool: &PgPool,
    client_id: &str,
    status: &str,
    hidden: bool,
    process_incarnation_id: Option<Uuid>,
) {
    sqlx::query(
        r#"
        INSERT INTO clients (
            id, display_name, public_key, status, internal_build_number,
            process_incarnation_id, capabilities, hidden_at
        )
        VALUES ($1, $1, decode('', 'hex'), $2, 1, $3, $4, CASE WHEN $5 THEN now() ELSE NULL END)
        "#,
    )
    .bind(client_id)
    .bind(status)
    .bind(process_incarnation_id)
    .bind(SqlJson(AgentCapabilitySnapshot::default()))
    .bind(hidden)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_worker_schedule(
    pool: &PgPool,
    name: &str,
    operation: serde_json::Value,
    targets: &[&str],
) -> Uuid {
    let actor_id = insert_worker_operator(
        pool,
        "active",
        "operator",
        &["jobs:write", "schedules:write"],
    )
    .await;
    let schedule_id = Uuid::new_v4();
    let target_client_ids = targets
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    sqlx::query(
        r#"
        INSERT INTO schedules (
            id, actor_id, name, operation, selector_expression, target_client_ids,
            cron_expr, next_run_at, catch_up_policy, catch_up_limit
        )
        VALUES ($1, $2, $3, $4, 'id:*', $5, '* * * * *', now() - interval '60 seconds', 'skip_missed', 1)
        "#,
    )
    .bind(schedule_id)
    .bind(actor_id)
    .bind(name)
    .bind(SqlJson(operation))
    .bind(target_client_ids)
    .execute(pool)
    .await
    .unwrap();
    schedule_id
}

async fn insert_worker_agent_update_release(
    pool: &PgPool,
    artifact_sha256_hex: &str,
    rollback_artifact_sha256_hex: Option<&str>,
) {
    sqlx::query(
        r#"
        INSERT INTO agent_update_releases (
            id, name, version, channel, status, artifact_sha256_hex,
            artifact_url_sha256_hex, rollback_artifact_sha256_hex,
            rollback_artifact_url_sha256_hex
        )
        VALUES ($1, 'vpsman-agent', '9.9.9', 'stable', 'published_external', $2, $3, $4, $5)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(artifact_sha256_hex)
    .bind("aa".repeat(32))
    .bind(rollback_artifact_sha256_hex)
    .bind(rollback_artifact_sha256_hex.map(|_| "bb".repeat(32)))
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_worker_operator(pool: &PgPool, status: &str, role: &str, scopes: &[&str]) -> Uuid {
    let operator_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO operators (id, username, password_hash, status, role, scopes)
        VALUES ($1, $2, 'test-password-hash', $3, $4, $5)
        "#,
    )
    .bind(operator_id)
    .bind(format!("worker-operator-{operator_id}"))
    .bind(status)
    .bind(role)
    .bind(serde_json::json!(scopes))
    .execute(pool)
    .await
    .unwrap();
    operator_id
}

async fn insert_worker_webhook_rule(
    pool: &PgPool,
    name: &str,
    expression: &str,
    body_template: &str,
) -> Uuid {
    let rule_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO webhook_rules (
            id, name, enabled, expression, target, body_template, cooldown_secs
        )
        VALUES ($1, $2, TRUE, $3, 'https://hooks.example.invalid/vpsman', $4, 0)
        "#,
    )
    .bind(rule_id)
    .bind(name)
    .bind(expression)
    .bind(body_template)
    .execute(pool)
    .await
    .unwrap();
    rule_id
}

async fn insert_active_worker_target(pool: &PgPool, client_id: &str) {
    let job_id = Uuid::new_v4();
    let operation = JobCommand::Shell {
        argv: vec!["sleep".to_string(), "60".to_string()],
        pty: false,
    };
    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, command_type, privileged, status, target_count, payload_hash,
            operation, request_fingerprint, max_timeout_secs
        )
        VALUES ($1, 'shell', TRUE, 'running', 1, $2, $3, $4, 60)
        "#,
    )
    .bind(job_id)
    .bind(format!("hash-{job_id}"))
    .bind(SqlJson(operation))
    .bind(format!("fingerprint-{job_id}"))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO job_targets (job_id, client_id, status, started_at)
        VALUES ($1, $2, 'running', now())
        "#,
    )
    .bind(job_id)
    .bind(client_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn schedule_result(
    pool: &PgPool,
    schedule_id: Uuid,
) -> (Uuid, Option<String>, i32, Option<String>) {
    let row = sqlx::query(
        r#"
        SELECT last_job_id, last_job_status, failure_count, last_error
        FROM schedules
        WHERE id = $1
        "#,
    )
    .bind(schedule_id)
    .fetch_one(pool)
    .await
    .unwrap();
    (
        row.try_get::<Option<Uuid>, _>("last_job_id")
            .unwrap()
            .unwrap(),
        row.try_get("last_job_status").unwrap(),
        row.try_get("failure_count").unwrap(),
        row.try_get("last_error").unwrap(),
    )
}

async fn job_targets(pool: &PgPool, job_id: Uuid) -> Vec<(String, String, Option<String>)> {
    sqlx::query(
        r#"
        SELECT client_id, status, message
        FROM job_targets
        WHERE job_id = $1
        ORDER BY client_id ASC
        "#,
    )
    .bind(job_id)
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .map(|row| {
        (
            row.try_get("client_id").unwrap(),
            row.try_get("status").unwrap(),
            row.try_get("message").unwrap(),
        )
    })
    .collect()
}

async fn job_status_output(pool: &PgPool, job_id: Uuid, client_id: &str) -> serde_json::Value {
    let data: Vec<u8> = sqlx::query_scalar(
        r#"
        SELECT data
        FROM job_outputs
        WHERE job_id = $1 AND client_id = $2 AND seq = 0
        "#,
    )
    .bind(job_id)
    .bind(client_id)
    .fetch_one(pool)
    .await
    .unwrap();
    serde_json::from_slice(&data).unwrap()
}

const WORKER_HOT_RELOAD_ENV: &[&str] = &[
    "VPSMAN_WORKER_TICK_SECS",
    "VPSMAN_AGENT_OFFLINE_TIMEOUT_SECS",
    "VPSMAN_WORKER_NOTIFICATION_DELIVERY_LIMIT",
    "VPSMAN_WORKER_NOTIFICATION_RETENTION_DAYS",
    "VPSMAN_WORKER_NOTIFICATION_RETENTION_PRUNE_LIMIT",
    "VPSMAN_WORKER_NOTIFICATION_WEBHOOK_TIMEOUT_SECS",
    "VPSMAN_WORKER_WEBHOOK_RULE_DELIVERY_LIMIT",
    "VPSMAN_WORKER_WEBHOOK_RULE_MATERIALIZE_LIMIT",
    "VPSMAN_WORKER_WEBHOOK_RULE_RETENTION_DAYS",
    "VPSMAN_WORKER_WEBHOOK_RULE_RETENTION_PRUNE_LIMIT",
    "VPSMAN_WORKER_WEBHOOK_RULE_TIMEOUT_SECS",
    "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_ENABLED",
    "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_LIMIT",
    "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_DRY_RUN",
    "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_INCLUDE_DISABLED",
    "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_DELETE_OBJECTS",
    "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_OBJECT_STORE_DIR",
    "VPSMAN_BACKUP_OBJECT_STORE_DIR",
    "VPSMAN_OBJECT_ENDPOINT",
    "VPSMAN_OBJECT_BUCKET",
    "VPSMAN_OBJECT_ACCESS_KEY",
    "VPSMAN_OBJECT_SECRET_KEY",
    "VPSMAN_OBJECT_REGION",
    "VPSMAN_OBJECT_CREATE_BUCKET",
    "VPSMAN_WORKER_SCHEDULE_JOB_MAX_TIMEOUT_SECS",
    "VPSMAN_REQUIRE_REGISTERED_AGENT_UPDATES",
];

static WORKER_SUITE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn with_cleared_worker_env<R>(names: &[&str], run: impl FnOnce() -> R) -> R {
    let _guard = WORKER_SUITE_ENV_LOCK.lock().unwrap();
    let saved = names
        .iter()
        .map(|name| (*name, std::env::var_os(name)))
        .collect::<Vec<_>>();
    for name in names {
        std::env::remove_var(name);
    }
    let result = run();
    for (name, value) in saved {
        if let Some(value) = value {
            std::env::set_var(name, value);
        } else {
            std::env::remove_var(name);
        }
    }
    result
}

fn temp_suite_config_path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("vpsman-{label}-{}.toml", Uuid::new_v4()))
}

#[allow(clippy::too_many_arguments)]
fn worker_runtime_toml(
    tick_secs: u64,
    agent_offline_timeout_secs: i64,
    schedule_job_max_timeout_secs: u64,
    require_registered_agent_updates: bool,
    notification_delivery_limit: i64,
    notification_retention_days: i64,
    notification_retention_prune_limit: i64,
    notification_webhook_timeout_secs: u64,
    webhook_rule_delivery_limit: i64,
    webhook_rule_materialize_limit: i64,
    webhook_rule_retention_days: i64,
    webhook_rule_retention_prune_limit: i64,
    webhook_rule_timeout_secs: u64,
    backup_policy_prune_enabled: bool,
    object_store_dir: &str,
) -> String {
    format!(
        r#"version = 1

[worker]
tick_secs = {tick_secs}
agent_offline_timeout_secs = {agent_offline_timeout_secs}
schedule_job_max_timeout_secs = {schedule_job_max_timeout_secs}
require_registered_agent_updates = {require_registered_agent_updates}
notification_delivery_limit = {notification_delivery_limit}
notification_retention_days = {notification_retention_days}
notification_retention_prune_limit = {notification_retention_prune_limit}
notification_webhook_timeout_secs = {notification_webhook_timeout_secs}
webhook_rule_delivery_limit = {webhook_rule_delivery_limit}
webhook_rule_materialize_limit = {webhook_rule_materialize_limit}
webhook_rule_retention_days = {webhook_rule_retention_days}
webhook_rule_retention_prune_limit = {webhook_rule_retention_prune_limit}
webhook_rule_timeout_secs = {webhook_rule_timeout_secs}
backup_policy_prune_enabled = {backup_policy_prune_enabled}
backup_policy_prune_object_store_dir = "{object_store_dir}"

[storage]
backup_object_store_dir = "{object_store_dir}/backups"
"#
    )
}
