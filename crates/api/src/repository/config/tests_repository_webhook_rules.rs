use super::*;
use crate::model::{OperatorPreferences, OperatorView};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::{path::Path, str::FromStr};

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
async fn postgres_manual_alert_send_revision_rejects_completion_after_suspension() {
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
    let database_options = options.database(&db_name);
    let migrations_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("migrations");
    crate::repository::migrate_postgres_database(&database_options, &migrations_dir)
        .await
        .unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect_with(database_options.options([("search_path", "public")]))
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
    let eligibility = repo
        .begin_webhook_rule_alert_send(delivery_id, lease_id)
        .await
        .unwrap();
    assert!(eligibility.is_deliverable());
    let revision = eligibility.revision().unwrap();

    let mut suspension = pool.begin().await.unwrap();
    sqlx::query(
        r#"
        UPDATE clients
        SET status='suspended', suspended_at=now(),
            suspended_from_status='offline'
        WHERE id=$1
        "#,
    )
    .bind(&client_id)
    .execute(&mut *suspension)
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
    .execute(&mut *suspension)
    .await
    .unwrap();
    suspension.commit().await.unwrap();

    let stale_completion = repo
        .complete_webhook_rule_delivery_attempt(
            delivery_id,
            lease_id,
            WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED,
            None,
            None,
            Some(revision),
        )
        .await;
    assert!(
        stale_completion.is_err(),
        "a completion armed before suspension must not overwrite cancellation"
    );

    let blocked_delivery_id = Uuid::new_v4();
    let blocked_lease_id = Uuid::new_v4();
    insert_claimed(blocked_delivery_id, blocked_lease_id).await;
    let blocked = repo
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
        )
        .await
        .unwrap();
    assert_eq!(
        canceled.status,
        WEBHOOK_RULE_DELIVERY_STATUS_CANCELED_DISABLED
    );

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
    }

    pool.close().await;
    sqlx::query(
        r#"
        SELECT pg_terminate_backend(pid)
        FROM pg_stat_activity
        WHERE datname = $1
          AND pid <> pg_backend_pid()
        "#,
    )
    .bind(&db_name)
    .execute(&admin_pool)
    .await
    .unwrap();
    sqlx::query(&format!("DROP DATABASE {db_name}"))
        .execute(&admin_pool)
        .await
        .unwrap();
    admin_pool.close().await;
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
