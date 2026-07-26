use std::{path::Path, str::FromStr, time::Duration};

use axum::http::{header::AUTHORIZATION, HeaderMap};
use chrono::{Datelike, Utc};
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    PgPool, Row,
};
use tokio::sync::broadcast;
use uuid::Uuid;
use vpsman_common::{
    payload_hash, plan_tunnel, AgentCapabilitySnapshot, AgentHello, AgentMetrics,
    AgentUpdateHeartbeat, CommandOutput, CpuStat, GatewayAgentHelloIngest, GatewayTelemetryIngest,
    JobCommand, LoadAverage, OutputStream, RuntimeTunnelManager, TelemetryEnvelope,
};
use vpsman_server_core::{
    JOB_STATUS_CANCELED, JOB_STATUS_COMPLETED, JOB_STATUS_CONTROL_TIMEOUT, JOB_STATUS_FAILED,
    JOB_STATUS_SKIPPED, TARGET_STATUS_AGENT_LOST, TARGET_STATUS_CANCELED, TARGET_STATUS_COMPLETED,
    TARGET_STATUS_CONTROL_TIMEOUT, TARGET_STATUS_FAILED, TARGET_STATUS_SKIPPED,
};

use crate::{
    gateway_client::GatewayDispatchClient,
    model::{
        AuthContext, BackupRequestStatus, BootstrapOperatorRequest, CreateBackupRequest,
        CreateScheduleRequest, DeleteAgentRequest, JobOutputView, LoginRequest, NewServerArtifact,
        WsEvent,
    },
    model_alert_notifications::{
        CreateFleetAlertNotificationChannelRequest, FleetAlertNotificationCandidate,
    },
    model_alert_policies::{CreateFleetAlertPolicyRequest, PolicyRuleRequest, VpsRuleQuery},
    model_history::{HistoryDomain, HistoryRetentionPrunePlan},
    model_webhook_rules::{CreateWebhookRuleRequest, WebhookRuleDeliveryCandidate},
    repository::Repository,
    repository_backups::BackupRequestSourceLink,
    repository_job_outputs::{JobOutputPersistConfig, JobOutputWriteResult},
    state::{AppState, DispatcherRuntimeConfig, DEFAULT_ARTIFACT_MAX_BYTES},
};

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
        .delete_fleet_alert_notification_channel(channel_id, &operator)
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
        .delete_webhook_rule(rule_id, &operator)
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

    let summary = db.repo.fleet_summary().await.unwrap();
    assert_eq!(summary.total, 5);
    assert_eq!(summary.online, 1);
    assert_eq!(summary.offline, 1);
    assert_eq!(summary.never, 1);
    assert_eq!(summary.stale, 1);
    assert_eq!(summary.unknown, 1);
    assert_eq!(summary.warnings, 4);
    assert_eq!(
        summary.online + summary.offline + summary.never + summary.stale + summary.unknown,
        summary.total
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_telemetry_ingest_is_sequence_bound_and_idempotent() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "telemetry-sequence-client";
    sqlx::query(
        r#"
        INSERT INTO clients (id, display_name, public_key, status)
        VALUES ($1, $2, $3, 'offline')
        "#,
    )
    .bind(client_id)
    .bind("Telemetry sequence client")
    .bind(vec![42_u8; 32])
    .execute(&db.pool)
    .await
    .unwrap();

    let process_incarnation_id = Uuid::new_v4();
    let mut event = GatewayTelemetryIngest {
        gateway_id: "gateway-a".to_string(),
        gateway_session_id: Uuid::new_v4(),
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
                },
                ..AgentMetrics::default()
            },
        },
    };

    assert!(db.repo.record_telemetry(&event).await.unwrap());
    assert!(!db.repo.record_telemetry(&event).await.unwrap());
    event.telemetry_seq = 1;
    event.telemetry.metrics.cpu.load.one = 99.0;
    assert!(!db.repo.record_telemetry(&event).await.unwrap());
    event.telemetry_seq = 3;
    event.telemetry.metrics.cpu.load.one = 3.0;
    assert!(db.repo.record_telemetry(&event).await.unwrap());
    let reconnect_session_id = Uuid::new_v4();
    event.gateway_session_id = reconnect_session_id;
    event.telemetry_seq = 1;
    event.telemetry.metrics.cpu.load.one = 4.0;
    assert!(db.repo.record_telemetry(&event).await.unwrap());

    let sample_count: i64 = sqlx::query_scalar(
        "SELECT COALESCE(sum(sample_count), 0)::bigint FROM telemetry_rollups WHERE client_id = $1",
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(sample_count, 3);
    let (gateway_session_id, telemetry_seq): (Uuid, i64) = sqlx::query_as(
        "SELECT gateway_session_id, telemetry_seq FROM telemetry_ingest_watermarks WHERE client_id = $1",
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(gateway_session_id, reconnect_session_id);
    assert_eq!(telemetry_seq, 1);
    let webhook_event_count: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM webhook_events WHERE kind = 'telemetry.rollup' AND event_id LIKE $1",
    )
    .bind(format!("telemetry:{client_id}:%"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(webhook_event_count, 3);

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_telemetry_queries_scope_before_limit_and_preserve_rate_baseline() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    for (client_id, key_byte) in [
        ("selected-telemetry", 51_u8),
        ("unrelated-telemetry", 52_u8),
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
                cpu_load_1_avg, cpu_load_1_max,
                memory_total_bytes_max, memory_available_bytes_avg, memory_available_bytes_min,
                disk_total_bytes_max, disk_available_bytes_avg, disk_available_bytes_min,
                network_rx_bytes_max, network_tx_bytes_max, latest_observed_at
            )
            VALUES (
                $1, to_timestamp($2::double precision), 60, 1,
                $3, $3, 1000, 500, 500, 2000, 1500, 1500, 0, 0,
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
                sample_count, rx_bytes_avg, tx_bytes_avg
            )
            VALUES ($1, 'eth0', to_timestamp($2::double precision), 60, 1, $3, $4)
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
        (current.saturating_sub(300), 10_000_i64, 20_000_i64),
        (current + 60, 25_000_i64, 50_000_i64),
    ] {
        sqlx::query(
            r#"
            INSERT INTO telemetry_network_rates (
                client_id, interface, bucket_start, bucket_secs,
                sample_count, rx_bytes_avg, tx_bytes_avg
            )
            VALUES ('selected-telemetry', 'eth0', to_timestamp($1::double precision), 300, 1, $2, $3)
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
    assert_eq!(latest_mixed[0].rx_bytes_delta, 15_000);

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_policy_alert_state_and_webhook_event_commit_atomically_and_repair_idempotently() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let operator = postgres_network_operator(&db.repo).await;
    let client_id = "atomic-policy-client";
    sqlx::query(
        r#"
        INSERT INTO clients (id, display_name, public_key, status)
        VALUES ($1, 'Atomic Policy Client', $2, 'online')
        "#,
    )
    .bind(client_id)
    .bind(vec![81_u8; 32])
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO telemetry_rollups (
            client_id, bucket_start, bucket_secs, sample_count,
            cpu_load_1_avg, cpu_load_1_max,
            memory_total_bytes_max, memory_available_bytes_avg, memory_available_bytes_min,
            disk_total_bytes_max, disk_available_bytes_avg, disk_available_bytes_min,
            network_rx_bytes_max, network_tx_bytes_max, latest_observed_at
        )
        VALUES (
            $1, date_trunc('minute', now()), 60, 1,
            2.0, 2.0, 1000, 500, 500, 2000, 1500, 1500, 0, 0, now()
        )
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let policy_id = Uuid::new_v4();
    let rule_id = Uuid::new_v4();
    for (policy_name, severity, expected_alerts) in [
        ("atomic-policy", "warning", 1_i64),
        ("atomic-policy", "critical", 2_i64),
        ("renamed-atomic-policy", "critical", 2_i64),
    ] {
        db.repo
            .upsert_fleet_alert_policy(
                &CreateFleetAlertPolicyRequest {
                    id: Some(policy_id),
                    name: policy_name.to_string(),
                    enabled: true,
                    selector_expression: format!("id:{client_id}"),
                    rules: vec![PolicyRuleRequest {
                        id: Some(rule_id),
                        name: "cpu threshold".to_string(),
                        enabled: true,
                        traffic_selector: None,
                        condition_expression: "cpu.load_1 >= 1".to_string(),
                        window_secs: 0,
                        severity: severity.to_string(),
                    }],
                    notes: None,
                    confirmed: true,
                    preview_hash: None,
                },
                &operator,
            )
            .await
            .unwrap();
        let alert_count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM policy_alerts WHERE policy_rule_id = $1 AND client_id = $2",
        )
        .bind(rule_id)
        .bind(client_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(alert_count, expected_alerts);
    }

    let generations = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT trigger_generation
        FROM policy_alerts
        WHERE policy_rule_id = $1 AND client_id = $2
        ORDER BY trigger_generation
        "#,
    )
    .bind(rule_id)
    .bind(client_id)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(generations, vec![1, 2]);

    let latest_alert_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id
        FROM policy_alerts
        WHERE policy_rule_id = $1 AND client_id = $2
        ORDER BY trigger_generation DESC
        LIMIT 1
        "#,
    )
    .bind(rule_id)
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM webhook_events WHERE kind = $1 AND event_id = $2")
        .bind("alert.policy_reached")
        .bind(format!("policy-alert:{latest_alert_id}"))
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(db.repo.evaluate_policy_rules().await.unwrap(), 0);
    let repaired = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM webhook_events WHERE kind = $1 AND event_id = $2",
    )
    .bind("alert.policy_reached")
    .bind(format!("policy-alert:{latest_alert_id}"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(repaired, 1);

    sqlx::query(
        "UPDATE telemetry_rollups SET cpu_load_1_avg = 0.0, cpu_load_1_max = 0.0 WHERE client_id = $1",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    assert_eq!(db.repo.evaluate_policy_rules().await.unwrap(), 0);
    sqlx::query(
        r#"
        CREATE FUNCTION reject_policy_webhook_event() RETURNS trigger
        LANGUAGE plpgsql AS $$
        BEGIN
            IF NEW.kind = 'alert.policy_reached' THEN
                RAISE EXCEPTION 'forced policy webhook failure';
            END IF;
            RETURN NEW;
        END
        $$
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        CREATE TRIGGER reject_policy_webhook_event
        BEFORE INSERT ON webhook_events
        FOR EACH ROW EXECUTE FUNCTION reject_policy_webhook_event()
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE telemetry_rollups SET cpu_load_1_avg = 2.0, cpu_load_1_max = 2.0 WHERE client_id = $1",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let error = db.repo.evaluate_policy_rules().await.unwrap_err();
    assert!(format!("{error:#}").contains("forced policy webhook failure"));
    let state = sqlx::query_as::<_, (bool, i64)>(
        r#"
        SELECT condition_true, trigger_generation
        FROM policy_rule_states
        WHERE policy_rule_id = $1 AND client_id = $2
        "#,
    )
    .bind(rule_id)
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(state, (false, 2));
    let alert_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM policy_alerts WHERE policy_rule_id = $1 AND client_id = $2",
    )
    .bind(rule_id)
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(alert_count, 2);

    sqlx::query("DROP TRIGGER reject_policy_webhook_event ON webhook_events")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION reject_policy_webhook_event()")
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(db.repo.evaluate_policy_rules().await.unwrap(), 1);
    let final_counts = sqlx::query_as::<_, (i64, i64)>(
        r#"
        SELECT
            (SELECT count(*) FROM policy_alerts WHERE policy_rule_id = $1 AND client_id = $2),
            (SELECT count(*) FROM webhook_events WHERE kind = 'alert.policy_reached')
        "#,
    )
    .bind(rule_id)
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(final_counts, (3, 3));

    let retained_alert_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id
        FROM policy_alerts
        WHERE policy_rule_id = $1 AND client_id = $2
        ORDER BY trigger_generation DESC
        LIMIT 1
        "#,
    )
    .bind(rule_id)
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE policy_alerts SET observed_at = now() - interval '2 hours' WHERE id = $1")
        .bind(retained_alert_id)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM webhook_events WHERE kind = $1 AND event_id = $2")
        .bind("alert.policy_reached")
        .bind(format!("policy-alert:{retained_alert_id}"))
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(db.repo.evaluate_policy_rules().await.unwrap(), 0);
    let retained_event_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM webhook_events WHERE kind = $1 AND event_id = $2",
    )
    .bind("alert.policy_reached")
    .bind(format!("policy-alert:{retained_alert_id}"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        retained_event_count, 0,
        "normal event retention must not redeliver an old sustained alert"
    );

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
        rules: vec![PolicyRuleRequest {
            id: None,
            name: "cpu threshold".to_string(),
            enabled: true,
            traffic_selector: None,
            condition_expression: "cpu.load_1 >= 1".to_string(),
            window_secs: 0,
            severity: "warning".to_string(),
        }],
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
                rules: vec![PolicyRuleRequest {
                    id: None,
                    name: "cpu threshold".to_string(),
                    enabled: true,
                    traffic_selector: None,
                    condition_expression: "cpu.load_1 >= 1".to_string(),
                    window_secs: 0,
                    severity: "warning".to_string(),
                }],
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
        FROM clients client
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
        .list_fleet_alert_policies(1, Some(true), None, Some(target_client_id))
        .await
        .unwrap();
    assert_eq!(client_policies.len(), 1);
    assert_eq!(client_policies[0].id, matching_policy_id);

    let selector_policies = db
        .repo
        .list_fleet_alert_policies(1, Some(true), Some(&format!("id:{target_client_id}")), None)
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
        &db.repo
            .get_traffic_accounting(target_client_id)
            .await
            .unwrap()
            .cycle_start,
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
            counter_epoch,
            sample_source
        )
        SELECT
            $1,
            'host',
            'eth0',
            to_timestamp(($2::bigint + generated.sample)::double precision),
            generated.sample::bigint,
            generated.sample::bigint,
            0,
            'test'
        FROM generate_series(1, 200001) AS generated(sample)
        "#,
    )
    .bind(old_client_id)
    .bind(cycle_start - 10_000_000)
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
            counter_epoch,
            sample_source
        )
        VALUES
            ($1, 'host', 'eth0', to_timestamp($2::double precision), 100, 200, 0, 'test'),
            ($1, 'host', 'eth0', to_timestamp($3::double precision), 130, 260, 0, 'test'),
            ($1, 'host', 'eth0', to_timestamp($4::double precision), 10, 300, 0, 'test'),
            ($1, 'host', 'eth0', to_timestamp($5::double precision), 20, 320, 0, 'test'),
            ($1, 'host', 'eth0', to_timestamp($6::double precision), 30, 340, 0, 'test')
        "#,
    )
    .bind(target_client_id)
    .bind((cycle_start - 1) as f64)
    .bind((cycle_start + 10) as f64)
    .bind((cycle_start + 20) as f64)
    .bind((cycle_start + 25) as f64)
    .bind((cycle_start + 30) as f64)
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
    assert_eq!(accounting.counter_epochs_seen, 1);
    assert_eq!(
        chrono::DateTime::parse_from_rfc3339(
            accounting
                .last_sample_at
                .as_deref()
                .expect("current-cycle traffic sample is present")
        )
        .unwrap()
        .timestamp(),
        cycle_start + 30
    );

    let retention_client_id = "traffic-retention-baseline";
    insert_client(&db.pool, retention_client_id, None).await;
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, counter_epoch, sample_source
        )
        VALUES
            ($1, 'host', 'eth0', to_timestamp(10), 10, 10, 0, 'test'),
            ($1, 'host', 'eth0', to_timestamp(20), 20, 20, 0, 'test'),
            ($1, 'host', 'eth0', to_timestamp(100), 100, 100, 0, 'test')
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
            50,
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
    assert_eq!(retained, vec![20, 100]);

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
        let migrator = sqlx::migrate::Migrator::new(workspace_migrations_dir()).await?;
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

fn postgres_app_state(db: &PgReliabilityTestDb) -> AppState {
    let (events, _) = broadcast::channel(16);
    AppState {
        repo: db.repo.clone(),
        events,
        internal_token: Some("gateway-secret-at-least-32-characters".to_string()),
        gateway: GatewayDispatchClient::default(),
        backup_object_store: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
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
    sqlx::query(
        r#"
        INSERT INTO clients (
            id, display_name, public_key, status, internal_build_number,
            process_incarnation_id, capabilities
        )
        VALUES ($1, $1, decode('', 'hex'), 'online', 1, $2, '{}'::jsonb)
        "#,
    )
    .bind(client_id)
    .bind(incarnation)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn postgres_agent_delete_returns_retired_peers_and_rejects_hidden_endpoint_reuse() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "client-a", None).await;
    insert_client(&db.pool, "client-b", None).await;
    let operator = postgres_network_operator(&db.repo).await;
    let input =
        crate::tests_network::test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, false);
    let plan = plan_tunnel(&input).unwrap();
    db.repo
        .record_tunnel_plan(&input, &plan, true, &operator)
        .await
        .unwrap();

    let deleted = db
        .repo
        .delete_agent(
            "client-a",
            &DeleteAgentRequest {
                confirmed: true,
                reason: Some("retire endpoint".to_string()),
                privilege_assertion: None,
            },
            &operator,
        )
        .await
        .unwrap();

    assert_eq!(
        deleted.retired_tunnel_endpoint_pairs,
        vec![("client-a".to_string(), "client-b".to_string())]
    );
    assert!(db.repo.list_tunnel_plans().await.unwrap().is_empty());
    assert_eq!(
        db.repo
            .record_tunnel_plan(&input, &plan, true, &operator)
            .await
            .unwrap_err()
            .to_string(),
        "tunnel_plan_endpoint_agent_not_found"
    );
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
        crate::tests_network::test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, false);
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
    stale_hello.noise_public_key_hex = Some(hex::encode(&old_key));
    assert!(db.repo.upsert_agent_hello(&stale_hello).await.unwrap());

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
        noise_public_key_hex: None,
        hello: AgentHello {
            client_id: client_id.to_string(),
            process_incarnation_id,
            agent_version: "pg-test-agent".to_string(),
            internal_build_number: 1,
            os_release: "test".to_string(),
            arch: "x86_64".to_string(),
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
        operation: JobCommand::Shell {
            argv: vec![
                "/bin/sh".to_string(),
                "-lc".to_string(),
                "uptime".to_string(),
            ],
            pty: false,
        },
        selector_expression: String::new(),
        target_client_ids: vec![client_id.to_string()],
        cron_expr: "0 * * * *".to_string(),
        timezone: "UTC".to_string(),
        enabled: true,
        catch_up_policy: "skip_missed".to_string(),
        catch_up_limit: 1,
        retry_delay_secs: 120,
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
        session_id: Uuid::nil(),
    }
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
        .request_job_cancel(job_id, operator.operator.id, Some("test cancel"))
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
            },
        )
        .await
        .unwrap();

    db.repo
        .delete_agent(
            client_id,
            &DeleteAgentRequest {
                confirmed: true,
                reason: Some("test delete".to_string()),
                privilege_assertion: None,
            },
            &operator,
        )
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
