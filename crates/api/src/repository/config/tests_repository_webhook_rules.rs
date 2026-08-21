use super::*;
use crate::model::{OperatorPreferences, OperatorView};
use crate::repository::MemoryState;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::{path::Path, str::FromStr};
use tokio::sync::oneshot;

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

#[tokio::test]
async fn postgres_manual_alert_send_fence_linearizes_suspension() {
    let base_url = match std::env::var("VPSMAN_TEST_POSTGRES_URL") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => return,
    };
    let options = PgConnectOptions::from_str(&base_url).unwrap();
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone().database("postgres"))
        .await
        .unwrap();
    let db_name = format!("vpsman_webhook_manual_fence_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {db_name}"))
        .execute(&admin_pool)
        .await
        .unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect_with(options.database(&db_name))
        .await
        .unwrap();
    sqlx::migrate::Migrator::new(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("migrations"),
    )
    .await
    .unwrap()
    .run(&pool)
    .await
    .unwrap();

    let client_id = format!("manual-alert-fence-{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO clients (id,display_name,public_key,status) VALUES ($1,$1,decode('', 'hex'),'offline')",
    )
    .bind(&client_id)
    .execute(&pool)
    .await
    .unwrap();
    let rule_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO webhook_rules (
            id,name,enabled,expression,target,body_template,cooldown_secs
        ) VALUES (
            $1,'manual-alert-fence',TRUE,'alert.triggered',
            'https://hooks.example.invalid/vpsman','',0
        )
        "#,
    )
    .bind(rule_id)
    .execute(&pool)
    .await
    .unwrap();
    let insert_claimed = |delivery_id: Uuid, lease_id: Uuid| {
        let pool = pool.clone();
        let client_id = client_id.clone();
        async move {
            sqlx::query(
                r#"
                INSERT INTO webhook_rule_deliveries (
                    id,rule_id,rule_name,event_kind,event_id,status,target,
                    dedupe_key,payload,matched_vps,message,cooldown_until_unix,
                    delivery_lease_id,delivery_lease_until
                ) VALUES (
                    $1,$2,'manual-alert-fence','alert.triggered',$3,'in_progress',
                    'https://hooks.example.invalid/vpsman',$4,'{}'::jsonb,
                    jsonb_build_array(jsonb_build_object(
                        'id',$5::text,
                        'display_name',$5::text,
                        'status','offline',
                        'tags','[]'::jsonb,
                        'registration_ip',NULL,
                        'last_ip',NULL,
                        'last_seen_at',NULL,
                        'arch',NULL,
                        'internal_build_number',0,
                        'process_incarnation_id',NULL,
                        'stale_since',NULL,
                        'stale_reason',NULL,
                        'capabilities','{}'::jsonb
                    )),
                    'alert',0,$6,now()+interval '60 seconds'
                )
                "#,
            )
            .bind(delivery_id)
            .bind(rule_id)
            .bind(format!("fleet-alert:{client_id}:{delivery_id}:triggered"))
            .bind(format!("manual-alert-fence:{delivery_id}"))
            .bind(&client_id)
            .bind(lease_id)
            .execute(&pool)
            .await
            .unwrap();
        }
    };
    let repo = Repository::Postgres(pool.clone());
    let delivery_id = Uuid::new_v4();
    let lease_id = Uuid::new_v4();
    insert_claimed(delivery_id, lease_id).await;
    let mut guard = repo
        .begin_webhook_rule_alert_send(delivery_id, lease_id)
        .await
        .unwrap();
    assert!(guard.is_deliverable());

    let suspension_pool = pool.clone();
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
                suspended_from_status='offline'
            WHERE id=$1
            "#,
        )
        .bind(&suspension_client_id)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            r#"
            UPDATE webhook_rule_deliveries
            SET status='canceled_disabled',error='client_suspended',
                delivery_lease_id=NULL,delivery_lease_until=NULL
            WHERE id=$1 AND status IN ('queued','failed','in_progress')
            "#,
        )
        .bind(delivery_id)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    });
    attempted_rx.await.unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut suspension)
            .await
            .is_err(),
        "suspension must wait for a manual alert webhook send that won the fence"
    );
    let completed = repo
        .complete_webhook_rule_delivery_attempt(
            delivery_id,
            lease_id,
            WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED,
            None,
            None,
            Some(&mut guard),
        )
        .await
        .unwrap();
    assert_eq!(completed.status, WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED);
    guard.release().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), suspension)
        .await
        .expect("suspension did not resume after the durable send outcome")
        .unwrap();

    let blocked_delivery_id = Uuid::new_v4();
    let blocked_lease_id = Uuid::new_v4();
    insert_claimed(blocked_delivery_id, blocked_lease_id).await;
    let mut blocked = repo
        .begin_webhook_rule_alert_send(blocked_delivery_id, blocked_lease_id)
        .await
        .unwrap();
    assert!(!blocked.is_deliverable());
    assert_eq!(blocked.cancellation_reason(), Some("client_suspended"));
    let canceled = repo
        .cancel_claimed_webhook_rule_delivery(
            blocked_delivery_id,
            blocked_lease_id,
            "client_suspended",
            Some(&mut blocked),
        )
        .await
        .unwrap();
    assert_eq!(
        canceled.status,
        WEBHOOK_RULE_DELIVERY_STATUS_CANCELED_DISABLED
    );
    blocked.release().await.unwrap();

    for mutation in ["matched_vps || matched_vps", "matched_vps || '[{}]'::jsonb"] {
        let invalid_delivery_id = Uuid::new_v4();
        let invalid_lease_id = Uuid::new_v4();
        insert_claimed(invalid_delivery_id, invalid_lease_id).await;
        sqlx::query(&format!(
            "UPDATE webhook_rule_deliveries SET matched_vps={mutation} WHERE id=$1"
        ))
        .bind(invalid_delivery_id)
        .execute(&pool)
        .await
        .unwrap();
        let invalid = repo
            .begin_webhook_rule_alert_send(invalid_delivery_id, invalid_lease_id)
            .await
            .unwrap();
        assert!(!invalid.is_deliverable());
        assert_eq!(
            invalid.cancellation_reason(),
            Some("client_alert_scope_invalid"),
            "duplicate and malformed client snapshots must fail closed"
        );
        invalid.release().await.unwrap();
    }

    pool.close().await;
    sqlx::query(&format!("DROP DATABASE {db_name}"))
        .execute(&admin_pool)
        .await
        .unwrap();
    admin_pool.close().await;
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
