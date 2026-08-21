use super::*;
use crate::test_support::PgWorkerTestDb;
use tokio::sync::oneshot;

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
async fn postgres_client_alert_send_fence_linearizes_suspension() {
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

    let mut send_guard =
        begin_alert_notification_send(&db.pool, delivery_id, channel_id, &alert_id, lease_id)
            .await
            .unwrap();
    assert!(send_guard.deliverable);

    let suspension_pool = db.pool.clone();
    let suspension_client_id = client_id.clone();
    let (attempting_tx, attempted_rx) = oneshot::channel();
    let mut suspension = tokio::spawn(async move {
        let mut tx = suspension_pool.begin().await.unwrap();
        attempting_tx.send(()).unwrap();
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(vpsman_server_core::client_policy_suppression_lock_key(
                &suspension_client_id,
            ))
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query(
            r#"
            UPDATE clients
            SET status='suspended', suspended_at=now(),
                suspended_reason='test', suspended_from_status='offline'
            WHERE id=$1
            "#,
        )
        .bind(&suspension_client_id)
        .execute(&mut *tx)
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
        .bind(&suspension_client_id)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    });
    attempted_rx.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut suspension)
            .await
            .is_err(),
        "suspension must wait while a pre-suspension external send owns the shared fence"
    );

    sqlx::query(
        r#"
        UPDATE fleet_alert_notification_deliveries
        SET status='delivered', attempt_count=attempt_count+1,
            last_attempt_at=now(), delivered_at=now(),
            delivery_lease_id=NULL, delivery_lease_until=NULL
        WHERE id=$1 AND status='in_progress' AND delivery_lease_id=$2
        "#,
    )
    .bind(delivery_id)
    .bind(lease_id)
    .execute(
        send_guard
            .postgres_connection()
            .expect("client-scoped alert send guard must own its fenced connection"),
    )
    .await
    .unwrap();
    send_guard.release().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), suspension)
        .await
        .expect("suspension did not resume after send outcome commit")
        .unwrap();
    let status: String =
        sqlx::query_scalar("SELECT status FROM fleet_alert_notification_deliveries WHERE id=$1")
            .bind(delivery_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(status, FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_DELIVERED);

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
        !blocked_guard.deliverable,
        "a send whose fence starts after suspension commits must be rejected"
    );
    blocked_guard.release().await.unwrap();

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
        !guard.deliverable,
        "a missing immutable episode must never authorize an outbound alert"
    );
    guard.release().await.unwrap();
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_dropped_client_alert_send_fence_discards_locked_session() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = format!("alert-fence-drop-{}", Uuid::new_v4().simple());
    let guard = ClientPolicySuppressionSharedGuard::acquire(&db.pool, &client_id)
        .await
        .unwrap();
    drop(guard);

    let mut tx = db.pool.begin().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(vpsman_server_core::client_policy_suppression_lock_key(
                &client_id,
            ))
            .execute(&mut *tx)
            .await
            .unwrap();
    })
    .await
    .expect("dropping a guarded send must close the locked PostgreSQL session");
    tx.rollback().await.unwrap();
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
