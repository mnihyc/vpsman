use super::*;
use crate::test_support::PgWorkerTestDb;
use crate::webhook_rules::{
    process_due_webhook_deliveries, WebhookRuleWorkerConfig, WebhookRuleWorkerRun,
};
use vpsman_common::WEBHOOK_RULE_DELIVERY_STATUS_CANCELED_DISABLED;

#[test]
fn automatic_http_delivery_owners_claim_one_row_and_redrain_until_empty() {
    for (source, due_owner, due_boundary, http_call, completion_call) in [
        (
            include_str!("alert_notifications.rs"),
            "pub(crate) async fn process_due_alert_notifications",
            "pub(crate) async fn drain_alert_notification_retention",
            "deliver_notification(&delivery, config.webhook_timeout_secs).await",
            "complete_claimed_alert_notification(",
        ),
        (
            include_str!("webhook_rules.rs"),
            "pub(crate) async fn process_due_webhook_deliveries",
            "async fn drain_webhook_retention",
            "deliver_webhook(&delivery, config.webhook_timeout_secs).await",
            "complete_webhook_rule_delivery_on_pool(",
        ),
    ] {
        let (_, due) = source
            .split_once(due_owner)
            .expect("automatic delivery owner");
        let (due, _) = due
            .split_once(due_boundary)
            .expect("automatic delivery owner boundary");
        assert!(due.contains("loop {"));
        assert!(due.contains("process_queued_deliveries(pool, config).await?"));
        assert!(due.contains("if claimed == 0"));
        assert!(!due.contains("yield_now"));

        let (_, attempt_page) = source
            .split_once("async fn process_queued_deliveries")
            .expect("automatic delivery attempt page");
        let (attempt_page, _) = attempt_page
            .split_once("fn delivery_lease_secs")
            .expect("automatic delivery attempt page boundary");
        let lease = attempt_page
            .find("let lease_id = Uuid::new_v4();")
            .expect("per-row durable lease");
        let claim = attempt_page.find("LIMIT 1").expect("single-row claim");
        let fetch = attempt_page
            .find(".fetch_optional(pool)")
            .expect("optional single-row claim result");
        let http = attempt_page.find(http_call).expect("bounded HTTP attempt");
        let completion = attempt_page
            .find(completion_call)
            .expect("lease-fenced completion");
        assert!(lease < claim && claim < fetch && fetch < http && http < completion);
        assert!(attempt_page.contains("FOR UPDATE OF delivery SKIP LOCKED"));
        assert!(attempt_page.contains("for _ in 0..config.delivery_limit"));
        assert!(!attempt_page.contains("fetch_all(pool)"));
        assert!(!attempt_page.contains("yield_now"));
    }
}

#[tokio::test]
async fn postgres_event_owner_executes_without_retention() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    assert_eq!(
        process_due_alert_notifications(&db.pool, AlertNotificationWorkerConfig::default())
            .await
            .unwrap(),
        AlertNotificationWorkerRun::default()
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_delivery_consumer_terminalizes_ineligible_durable_work() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = format!("alert-consumer-{}", Uuid::new_v4().simple());
    let alert_id = format!("agent_status:agent:{client_id}");
    let channel_id = Uuid::new_v4();
    let delivery_id = Uuid::new_v4();
    insert_alert_send_fixture(
        &db.pool,
        &client_id,
        &alert_id,
        channel_id,
        delivery_id,
        Uuid::new_v4(),
    )
    .await;
    sqlx::query(
        r#"
        UPDATE fleet_alert_notification_deliveries
        SET status='queued', delivery_lease_id=NULL, delivery_lease_until=NULL
        WHERE id=$1
        "#,
    )
    .bind(delivery_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE alert_episodes SET lifecycle_state='resolved', resolved_at=now(), resolution_reason='condition_recovered' WHERE public_id=$1")
        .bind(&alert_id)
        .execute(&db.pool)
        .await
        .unwrap();

    let run = process_due_alert_notifications(&db.pool, AlertNotificationWorkerConfig::default())
        .await
        .unwrap();
    assert_eq!((run.processed, run.delivered, run.failed), (1, 0, 1));
    assert_eq!(
        sqlx::query_as::<_, (String, Option<String>, i32)>(
            "SELECT status, error, attempt_count FROM fleet_alert_notification_deliveries WHERE id=$1",
        )
        .bind(delivery_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (
            FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_CANCELED_DISABLED.to_string(),
            Some("fleet alert resolved or client suspended".to_string()),
            0,
        )
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_delivery_owners_terminalize_disabled_and_deleted_sources_without_http() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = format!(
        "http://{}/delivery-owner-proof",
        listener.local_addr().unwrap()
    );

    let client_id = format!("delivery-source-{}", Uuid::new_v4().simple());
    let alert_id = format!("agent_status:agent:{client_id}");
    let disabled_channel_id = Uuid::new_v4();
    let disabled_alert_delivery_id = Uuid::new_v4();
    insert_alert_send_fixture(
        &db.pool,
        &client_id,
        &alert_id,
        disabled_channel_id,
        disabled_alert_delivery_id,
        Uuid::new_v4(),
    )
    .await;
    let deleted_channel_id = Uuid::new_v4();
    let deleted_alert_delivery_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO fleet_alert_notification_channels (
            id, name, scope_kind, scope_value, min_severity,
            categories, operator_states, delivery_kind, target,
            cooldown_secs, enabled
        ) VALUES (
            $1, $2, 'client', $3, 'warning', '[]'::jsonb,
            '[]'::jsonb, 'webhook', $4, 0, TRUE
        )
        "#,
    )
    .bind(deleted_channel_id)
    .bind(format!("deleted-alert-source-{deleted_channel_id}"))
    .bind(&client_id)
    .bind(&target)
    .execute(&db.pool)
    .await
    .unwrap();
    insert_claimed_alert_delivery(
        &db.pool,
        &alert_id,
        deleted_channel_id,
        deleted_alert_delivery_id,
        Uuid::new_v4(),
    )
    .await;
    sqlx::query(
        r#"
        UPDATE fleet_alert_notification_channels
        SET enabled=FALSE, target=$2
        WHERE id=$1
        "#,
    )
    .bind(disabled_channel_id)
    .bind(&target)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE fleet_alert_notification_deliveries
        SET status='queued', target=$1,
            delivery_lease_id=NULL, delivery_lease_until=NULL
        WHERE id = ANY($2)
        "#,
    )
    .bind(&target)
    .bind(vec![disabled_alert_delivery_id, deleted_alert_delivery_id])
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM fleet_alert_notification_channels WHERE id=$1")
        .bind(deleted_channel_id)
        .execute(&db.pool)
        .await
        .unwrap();

    let disabled_rule_id = Uuid::new_v4();
    let deleted_rule_id = Uuid::new_v4();
    for (rule_id, name) in [
        (disabled_rule_id, "disabled-webhook-source"),
        (deleted_rule_id, "deleted-webhook-source"),
    ] {
        sqlx::query(
            r#"
            INSERT INTO webhook_rules (
                id, name, enabled, expression, target, body_template, cooldown_secs
            ) VALUES ($1, $2, TRUE, 'job.created', $3, '', 0)
            "#,
        )
        .bind(rule_id)
        .bind(format!("{name}-{rule_id}"))
        .bind(&target)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    let disabled_webhook_delivery_id = Uuid::new_v4();
    let deleted_webhook_delivery_id = Uuid::new_v4();
    for (delivery_id, rule_id, source) in [
        (disabled_webhook_delivery_id, disabled_rule_id, "disabled"),
        (deleted_webhook_delivery_id, deleted_rule_id, "deleted"),
    ] {
        sqlx::query(
            r#"
            INSERT INTO webhook_rule_deliveries (
                id, rule_id, rule_name, event_kind, event_id, status,
                target, dedupe_key, payload, matched_vps, message,
                cooldown_until_unix
            ) VALUES (
                $1, $2, $3, 'job.created', $4, 'queued', $5, $6,
                '{}'::jsonb, '[]'::jsonb, 'delivery owner proof', 0
            )
            "#,
        )
        .bind(delivery_id)
        .bind(rule_id)
        .bind(format!("{source}-webhook-source"))
        .bind(format!("job.created:{delivery_id}"))
        .bind(&target)
        .bind(format!("delivery-owner-proof:{delivery_id}"))
        .execute(&db.pool)
        .await
        .unwrap();
    }
    sqlx::query("UPDATE webhook_rules SET enabled=FALSE WHERE id=$1")
        .bind(disabled_rule_id)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM webhook_rules WHERE id=$1")
        .bind(deleted_rule_id)
        .execute(&db.pool)
        .await
        .unwrap();

    let alert_run = process_due_alert_notifications(
        &db.pool,
        AlertNotificationWorkerConfig::new(1, 90, 1_000, 1),
    )
    .await
    .unwrap();
    assert_eq!(
        (alert_run.processed, alert_run.delivered, alert_run.failed),
        (2, 0, 2)
    );
    let webhook_run = process_due_webhook_deliveries(
        &db.pool,
        WebhookRuleWorkerConfig::new(1, 100, 90, 1_000, 1).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        webhook_run,
        WebhookRuleWorkerRun {
            processed: 2,
            failed: 2,
            ..WebhookRuleWorkerRun::default()
        }
    );

    let alert_rows = sqlx::query_as::<_, (Uuid, String, Option<String>, i32)>(
        r#"
        SELECT id, status, error, attempt_count
        FROM fleet_alert_notification_deliveries
        WHERE id = ANY($1)
        ORDER BY id
        "#,
    )
    .bind(vec![disabled_alert_delivery_id, deleted_alert_delivery_id])
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(alert_rows.len(), 2, "terminal history must remain durable");
    assert!(alert_rows.iter().all(|(_, status, error, attempts)| {
        status == FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_CANCELED_DISABLED
            && error.as_deref() == Some("fleet alert notification channel disabled")
            && *attempts == 0
    }));
    let webhook_rows = sqlx::query_as::<_, (Uuid, String, Option<String>, i32)>(
        r#"
        SELECT id, status, error, attempt_count
        FROM webhook_rule_deliveries
        WHERE id = ANY($1)
        ORDER BY id
        "#,
    )
    .bind(vec![
        disabled_webhook_delivery_id,
        deleted_webhook_delivery_id,
    ])
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        webhook_rows.len(),
        2,
        "terminal webhook history must remain durable"
    );
    assert!(webhook_rows.iter().all(|(_, status, error, attempts)| {
        status == WEBHOOK_RULE_DELIVERY_STATUS_CANCELED_DISABLED
            && error.as_deref() == Some("webhook rule disabled")
            && *attempts == 0
    }));
    let listener = listener.into_std().unwrap();
    assert!(
        matches!(
            listener.accept(),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
        ),
        "disabled or deleted sources must be terminalized before any HTTP connection"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_delivery_drain_does_not_spin_on_another_consumers_claimable_row_lock() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = format!("alert-locked-{}", Uuid::new_v4().simple());
    let alert_id = format!("agent_status:agent:{client_id}");
    let channel_id = Uuid::new_v4();
    let delivery_id = Uuid::new_v4();
    insert_alert_send_fixture(
        &db.pool,
        &client_id,
        &alert_id,
        channel_id,
        delivery_id,
        Uuid::new_v4(),
    )
    .await;
    sqlx::query(
        r#"
        UPDATE fleet_alert_notification_deliveries
        SET status='queued', delivery_lease_id=NULL, delivery_lease_until=NULL
        WHERE id=$1
        "#,
    )
    .bind(delivery_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let mut other_consumer = db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM fleet_alert_notification_deliveries WHERE id=$1 FOR UPDATE")
        .bind(delivery_id)
        .execute(&mut *other_consumer)
        .await
        .unwrap();
    let run = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        process_due_alert_notifications(&db.pool, AlertNotificationWorkerConfig::default()),
    )
    .await
    .expect("a worker must return when all due rows are owned elsewhere")
    .unwrap();
    assert_eq!(run, AlertNotificationWorkerRun::default());
    other_consumer.rollback().await.unwrap();
    db.cleanup().await;
}

#[test]
fn notification_worker_config_clamps_bounds() {
    assert_eq!(
        AlertNotificationWorkerConfig::new(0, 0, 0, 0),
        AlertNotificationWorkerConfig {
            delivery_limit: 1,
            retention_days: 1,
            retention_prune_limit: 1,
            webhook_timeout_secs: 1,
        }
    );
    assert_eq!(
        AlertNotificationWorkerConfig::new(10_000, 10_000, 20_000, 120),
        AlertNotificationWorkerConfig {
            delivery_limit: 200,
            retention_days: 3_650,
            retention_prune_limit: 10_000,
            webhook_timeout_secs: 60,
        }
    );
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

#[tokio::test]
async fn postgres_client_alert_send_revision_rejects_completion_after_suspension() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = format!("alert-fence-{}", Uuid::new_v4().simple());
    let alert_id = format!("agent_status:agent:{client_id}");
    let channel_id = Uuid::new_v4();
    let delivery_id = Uuid::new_v4();
    let lease_id = Uuid::new_v4();
    insert_alert_send_fixture(
        &db.pool,
        &client_id,
        &alert_id,
        channel_id,
        delivery_id,
        lease_id,
    )
    .await;

    let send_eligibility =
        begin_alert_notification_send(&db.pool, delivery_id, channel_id, &alert_id, lease_id)
            .await
            .unwrap();
    let revision = send_eligibility
        .eligibility_revision
        .expect("eligible send must be armed with a durable revision");

    let mut suspension = db.pool.begin().await.unwrap();
    sqlx::query(
        r#"
        UPDATE clients
        SET status='suspended', suspended_at=now(),
            suspended_reason='test', suspended_from_status='offline'
        WHERE id=$1
        "#,
    )
    .bind(&client_id)
    .execute(&mut *suspension)
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE fleet_alert_notification_deliveries delivery
        SET status='canceled_disabled', error='client_suspended',
            delivery_lease_id=NULL, delivery_lease_until=NULL
        FROM alert_episodes episode
        WHERE episode.public_id=delivery.alert_id
          AND episode.client_id=$1
          AND delivery.status IN ('queued','failed','in_progress')
        "#,
    )
    .bind(&client_id)
    .execute(&mut *suspension)
    .await
    .unwrap();
    suspension.commit().await.unwrap();
    assert!(
        complete_claimed_alert_notification(
            &db.pool,
            delivery_id,
            lease_id,
            Some(revision),
            FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_DELIVERED,
            None,
            None,
        )
        .await
        .unwrap()
        .is_none(),
        "the pre-suspension result must not overwrite durable cancellation"
    );
    let status: String =
        sqlx::query_scalar("SELECT status FROM fleet_alert_notification_deliveries WHERE id=$1")
            .bind(delivery_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        status,
        FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_CANCELED_DISABLED
    );

    let blocked_delivery_id = Uuid::new_v4();
    let blocked_lease_id = Uuid::new_v4();
    insert_claimed_alert_delivery(
        &db.pool,
        &alert_id,
        channel_id,
        blocked_delivery_id,
        blocked_lease_id,
    )
    .await;
    let blocked_guard = begin_alert_notification_send(
        &db.pool,
        blocked_delivery_id,
        channel_id,
        &alert_id,
        blocked_lease_id,
    )
    .await
    .unwrap();
    assert!(
        blocked_guard.eligibility_revision.is_none(),
        "a send armed after suspension commits must be rejected"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_alert_send_fails_closed_when_its_episode_is_missing() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = format!("alert-missing-{}", Uuid::new_v4().simple());
    let alert_id = format!("agent_status:agent:{client_id}");
    let channel_id = Uuid::new_v4();
    let delivery_id = Uuid::new_v4();
    let lease_id = Uuid::new_v4();
    insert_alert_send_fixture(
        &db.pool,
        &client_id,
        &alert_id,
        channel_id,
        delivery_id,
        lease_id,
    )
    .await;
    sqlx::query("DELETE FROM alert_episodes WHERE public_id=$1")
        .bind(&alert_id)
        .execute(&db.pool)
        .await
        .unwrap();

    let guard =
        begin_alert_notification_send(&db.pool, delivery_id, channel_id, &alert_id, lease_id)
            .await
            .unwrap();
    assert!(guard.channel_enabled);
    assert!(
        guard.eligibility_revision.is_none(),
        "a missing immutable episode must never authorize an outbound alert"
    );
    db.cleanup().await;
}

async fn insert_alert_send_fixture(
    pool: &PgPool,
    client_id: &str,
    alert_id: &str,
    channel_id: Uuid,
    delivery_id: Uuid,
    lease_id: Uuid,
) {
    sqlx::query(
        r#"
        INSERT INTO clients (id, display_name, public_key, status)
        VALUES ($1, $1, decode('', 'hex'), 'offline')
        "#,
    )
    .bind(client_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO alert_episodes (
            id, public_id, producer_kind, natural_key, record_kind,
            trigger_generation, trigger_severity, trigger_category,
            severity, category, target_kind, target_id, client_id,
            title, detail, source_status, evidence, lifecycle_state,
            triggered_at, last_confirmed_at, policy_group_id, policy_rule_id,
            policy_rule_version, policy_rule_kind, policy_group_name,
            policy_rule_name, policy_rule_system_seed_key
        ) SELECT
            $1, $2, rule.evidence_source, $3, 'condition', 1,
            'warning', 'agent_status', 'warning', 'agent_status',
            'agent', $3, $3, 'Agent offline', 'test', 'offline',
            '{}'::jsonb, 'triggered', now(), now(), rule.group_id, rule.id,
            rule.rule_version, rule.rule_kind, policy.name, rule.name,
            rule.system_seed_key
        FROM policy_rules rule
        JOIN policy_groups policy ON policy.id=rule.group_id
        WHERE rule.id='d1000000-0000-4000-8000-000000000003'
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(alert_id)
    .bind(client_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO fleet_alert_notification_channels (
            id, name, scope_kind, scope_value, min_severity,
            categories, operator_states, delivery_kind, target,
            cooldown_secs, enabled
        ) VALUES (
            $1, $2, 'client', $3, 'warning', '[]'::jsonb,
            '[]'::jsonb, 'webhook', 'https://hooks.example.invalid/vpsman',
            0, TRUE
        )
        "#,
    )
    .bind(channel_id)
    .bind(format!("alert-fence-{channel_id}"))
    .bind(client_id)
    .execute(pool)
    .await
    .unwrap();
    insert_claimed_alert_delivery(pool, alert_id, channel_id, delivery_id, lease_id).await;
}

async fn insert_claimed_alert_delivery(
    pool: &PgPool,
    alert_id: &str,
    channel_id: Uuid,
    delivery_id: Uuid,
    lease_id: Uuid,
) {
    sqlx::query(
        r#"
        INSERT INTO fleet_alert_notification_deliveries (
            id, channel_id, channel_name, alert_id, alert_severity,
            alert_category, status, delivery_kind, target, dedupe_key,
            payload, cooldown_until_unix, delivery_lease_id,
            delivery_lease_until
        ) VALUES (
            $1, $2, 'alert-fence', $3, 'warning', 'agent_status',
            'in_progress', 'webhook',
            'https://hooks.example.invalid/vpsman', $4,
            '{}'::jsonb, 0, $5, now() + interval '60 seconds'
        )
        "#,
    )
    .bind(delivery_id)
    .bind(channel_id)
    .bind(alert_id)
    .bind(format!("alert-fence:{delivery_id}"))
    .bind(lease_id)
    .execute(pool)
    .await
    .unwrap();
}
