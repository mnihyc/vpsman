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
    pair_port_expressions, payload_hash, plan_tunnel, AgentCapabilitySnapshot, AgentHello,
    AgentMetrics, AgentUpdateHeartbeat, CommandOutput, CpuStat, DiskStat, GatewayAgentHelloIngest,
    GatewayTelemetryIngest, JobCommand, LoadAverage, MemoryStat, NetworkStat, OspfControlMode,
    OspfCostPolicy, OutputStream, PortForwardProtocol, PortForwardRuntimeSnapshot,
    PortForwardRuntimeStatus, RuntimeTunnelControl, RuntimeTunnelManager, TelemetryEnvelope,
    TunnelAddressPair, TunnelKind, TunnelOspfConfig, TunnelPlanInput,
};
use vpsman_server_core::{
    JOB_STATUS_CANCELED, JOB_STATUS_COMPLETED, JOB_STATUS_CONTROL_TIMEOUT, JOB_STATUS_FAILED,
    JOB_STATUS_SKIPPED, TARGET_STATUS_AGENT_LOST, TARGET_STATUS_CANCELED, TARGET_STATUS_COMPLETED,
    TARGET_STATUS_CONTROL_TIMEOUT, TARGET_STATUS_FAILED, TARGET_STATUS_SKIPPED,
};

use crate::{
    gateway_client::GatewayDispatchClient,
    model::{
        AuthContext, BackupRequestStatus, BootstrapOperatorRequest, ConfigurationOverrideAction,
        CreateBackupPolicyRequest, CreateBackupRequest, CreateConfigurationPresetRequest,
        CreateScheduleRequest, FleetAlertQuery, JobOutputView, JobRolloutPolicy, ListQuery,
        LoginRequest, NewServerArtifact, PingTargetRecord, PreviewConfigurationPresetRequest,
        PreviewConfigurationSourceOverrideRequest, SchedulePrivilegeMutationRequest,
        UpsertRuntimeConfigPatchGeneratorRequest, WsEvent,
    },
    model_alert_notifications::{
        CreateFleetAlertNotificationChannelRequest, FleetAlertNotificationCandidate,
    },
    model_alert_policies::{
        CreateFleetAlertPolicyRequest, NetworkRateInterfaceSelection, PolicyAlertQuery,
        PolicyDryRunRequest, PolicyRuleRequest, VpsRuleQuery,
    },
    model_command_templates::UpsertCommandTemplateRequest,
    model_history::UpsertHistoryRetentionPolicyRequest,
    model_history::{HistoryDomain, HistoryRetentionPrunePlan},
    model_port_forwarding::{CreatePortForwardRuleRequest, UpdatePortForwardRuleRequest},
    model_terminal::TerminalSessionView,
    model_webhook_rules::{CreateWebhookRuleRequest, WebhookRuleDeliveryCandidate},
    repository::Repository,
    repository_backups::BackupRequestSourceLink,
    repository_job_outputs::{JobOutputPersistConfig, JobOutputWriteResult},
    repository_terminal_sessions::upsert_postgres_terminal_session,
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
    };
    let preserved = db
        .repo
        .update_schedule_record(
            schedule.id,
            crate::repository_schedules::ScheduleCreateInput {
                name: "frozen-targets-renamed".to_string(),
                operation: schedule.operation.clone().unwrap(),
                selector_expression: schedule.selector_expression.clone(),
                target_client_ids: schedule.target_client_ids.clone(),
                cron_expr: schedule.cron_expr.clone(),
                timezone: schedule.timezone.clone(),
                enabled: schedule.enabled,
                catch_up_policy: schedule.catch_up_policy.clone(),
                catch_up_limit: schedule.catch_up_limit,
                retry_delay_secs: schedule.retry_delay_secs,
                max_failures: schedule.max_failures,
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
    };
    let changed_selector_error = db
        .repo
        .update_schedule_record(
            schedule.id,
            crate::repository_schedules::ScheduleCreateInput {
                name: preserved.name.clone(),
                operation: preserved.operation.clone().unwrap(),
                selector_expression: "id:replacement".to_string(),
                target_client_ids: preserved.target_client_ids.clone(),
                cron_expr: preserved.cron_expr.clone(),
                timezone: preserved.timezone.clone(),
                enabled: preserved.enabled,
                catch_up_policy: preserved.catch_up_policy.clone(),
                catch_up_limit: preserved.catch_up_limit,
                retry_delay_secs: preserved.retry_delay_secs,
                max_failures: preserved.max_failures,
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
    };
    let edited_empty = db
        .repo
        .update_schedule_record(
            schedule.id,
            crate::repository_schedules::ScheduleCreateInput {
                name: "frozen-targets-empty".to_string(),
                operation: empty.operation.clone().unwrap(),
                selector_expression: empty.selector_expression.clone(),
                target_client_ids: Vec::new(),
                cron_expr: empty.cron_expr.clone(),
                timezone: empty.timezone.clone(),
                enabled: empty.enabled,
                catch_up_policy: empty.catch_up_policy.clone(),
                catch_up_limit: empty.catch_up_limit,
                retry_delay_secs: empty.retry_delay_secs,
                max_failures: empty.max_failures,
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
            'network_probe',
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
            'network_probe',
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
    let graph = db.repo.topology_graph(24).await.unwrap();
    let quiet_edge = graph
        .edges
        .iter()
        .find(|edge| edge.plan_id == quiet_plan.id)
        .unwrap();
    assert_eq!(quiet_edge.sample_count, 1);
    assert_eq!(quiet_edge.probe_state, "degraded");
    assert_eq!(quiet_edge.latency_series_ms, vec![42.0]);
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
                selector_expression: repair.selector_expression,
                target_client_ids: repair.target_client_ids,
                cron_expr: repair.cron_expr,
                timezone: repair.timezone,
                enabled: repair.enabled,
                catch_up_policy: repair.catch_up_policy,
                catch_up_limit: repair.catch_up_limit,
                retry_delay_secs: repair.retry_delay_secs,
                max_failures: repair.max_failures,
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
        .set_schedule_enabled(malformed.id, true, &operator)
        .await
        .is_err());
    assert!(
        !db.repo
            .set_schedule_enabled(malformed.id, false, &operator)
            .await
            .unwrap()
            .enabled
    );

    let repair =
        postgres_shell_schedule_request("repaired-schedule-operation", "malformed-schedule-client");
    let repaired = db
        .repo
        .update_schedule_record(
            malformed.id,
            crate::repository_schedules::ScheduleCreateInput {
                name: repair.name,
                operation: repair.operation,
                selector_expression: repair.selector_expression,
                target_client_ids: repair.target_client_ids,
                cron_expr: repair.cron_expr,
                timezone: repair.timezone,
                enabled: false,
                catch_up_policy: repair.catch_up_policy,
                catch_up_limit: repair.catch_up_limit,
                retry_delay_secs: repair.retry_delay_secs,
                max_failures: repair.max_failures,
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
                    operation: JobCommand::Shell {
                        argv: vec!["/usr/bin/uptime".to_string()],
                        pty: false,
                    },
                    selector_expression: "id:atomic-a".to_string(),
                    target_client_ids: vec!["atomic-a".to_string()],
                    cron_expr: "30 * * * *".to_string(),
                    timezone: "UTC".to_string(),
                    enabled: true,
                    catch_up_policy: "skip_missed".to_string(),
                    catch_up_limit: 1,
                    retry_delay_secs: 120,
                    max_failures: 2,
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
    assert_forced_audit_failure(
        db.repo
            .update_schedule_targets(schedule.id, vec!["atomic-b".to_string()], None, &operator)
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
            .set_schedule_enabled(schedule.id, false, &operator)
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
    assert_forced_audit_failure(db.repo.soft_delete_schedule(schedule.id, &operator).await);
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
async fn postgres_telemetry_ingest_is_sequence_bound_and_idempotent() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "telemetry-sequence-client";
    let process_incarnation_id = Uuid::new_v4();
    let gateway_session_id = Uuid::new_v4();
    insert_client(&db.pool, client_id, Some(process_incarnation_id)).await;
    start_test_gateway_session(&db.repo, "gateway-a", client_id, gateway_session_id).await;
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
    let resource_rollup = sqlx::query(
        r#"
        SELECT
            cpu_cores_max,
            memory_total_bytes_max,
            memory_available_bytes_avg,
            memory_available_bytes_min,
            memory_used_ratio_avg,
            memory_used_ratio_max,
            swap_sample_count,
            swap_total_bytes_max,
            swap_available_bytes_avg,
            swap_available_bytes_min,
            swap_used_ratio_avg,
            swap_used_ratio_max,
            disk_total_bytes_max,
            disk_available_bytes_avg,
            disk_available_bytes_min,
            disk_used_ratio_avg,
            disk_used_ratio_max
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
        132
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
        Some(109)
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
        132
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
    for (offset, rx_bytes, tx_bytes) in [
        (0, 100_u64, 200_u64),
        (10, 150, 240),
        (60, 10, 5),
        (120, 30, 25),
    ] {
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
        sqlx::query(
            r#"
            INSERT INTO telemetry_samples (
                id, client_id, observed_at, cpu_load_1,
                memory_total_bytes, memory_available_bytes, payload
            ) VALUES (
                $1, $2, to_timestamp($3::double precision), 0, 0, 0, $4
            )
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(client_id)
        .bind((base + offset) as f64)
        .bind(serde_json::json!(metrics))
        .execute(&db.pool)
        .await
        .unwrap();
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

    let raw = db
        .repo
        .list_traffic_history(client_id, base + 10, base + 120, 60, true)
        .await
        .unwrap();
    assert_eq!(
        raw.iter().filter_map(|point| point.rx_bytes).sum::<i64>(),
        70
    );
    assert_eq!(
        raw.iter().filter_map(|point| point.tx_bytes).sum::<i64>(),
        60
    );
    assert!(raw
        .iter()
        .any(|point| point.sample_count == 0 && point.reset_count == 1));

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
                cpu_load_1_avg, cpu_load_1_max,
                memory_total_bytes_max, memory_available_bytes_avg,
                memory_available_bytes_min, memory_used_ratio_avg, memory_used_ratio_max,
                disk_total_bytes_max, disk_available_bytes_avg,
                disk_available_bytes_min, disk_used_ratio_avg, disk_used_ratio_max,
                network_rx_bytes_max, network_tx_bytes_max, latest_observed_at
            )
            VALUES (
                $1, to_timestamp($2::double precision), 60, 1,
                $3, $3, 1000, 500, 500, 0.5, 0.5,
                2000, 1500, 1500, 0.25, 0.25, 0, 0,
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
                sample_count, rx_bytes_avg, tx_bytes_avg,
                rx_bytes_last, tx_bytes_last, rx_counter_epoch, tx_counter_epoch
            )
            VALUES ($1, 'eth0', to_timestamp($2::double precision), 60, 1, $3, $4, $3, $4, 0, 0)
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
                sample_count, rx_bytes_avg, tx_bytes_avg,
                rx_bytes_last, tx_bytes_last, rx_counter_epoch, tx_counter_epoch
            )
            VALUES ('selected-telemetry', 'eth0', to_timestamp($1::double precision), 300, 1, $2, $3, $2, $3, 0, 0)
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
            cpu_load_1_avg, cpu_load_1_max,
            memory_total_bytes_max, memory_available_bytes_avg,
            memory_available_bytes_min, memory_used_ratio_avg, memory_used_ratio_max,
            disk_total_bytes_max, disk_available_bytes_avg,
            disk_available_bytes_min, disk_used_ratio_avg, disk_used_ratio_max,
            network_rx_bytes_max, network_tx_bytes_max, latest_observed_at
        )
        VALUES
            (
                'selected-telemetry', to_timestamp($1::double precision), 60, 1,
                1.0, 1.2, 1000, 900, 900, 0.1, 0.1,
                2000, 1900, 1900, 0.05, 0.05, 0, 0,
                to_timestamp($1::double precision)
            ),
            (
                'selected-telemetry', to_timestamp($2::double precision), 60, 3,
                3.0, 3.4, 1000, 500, 500, 0.5, 0.5,
                2000, 1100, 1100, 0.45, 0.45, 0, 0,
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
            cpu_load_1_avg, cpu_load_1_max,
            memory_total_bytes_max, memory_available_bytes_avg,
            memory_available_bytes_min, memory_used_ratio_avg, memory_used_ratio_max,
            disk_total_bytes_max, disk_available_bytes_avg,
            disk_available_bytes_min, disk_used_ratio_avg, disk_used_ratio_max,
            network_rx_bytes_max, network_tx_bytes_max, latest_observed_at
        )
        VALUES
            (
                'multi-day-telemetry', to_timestamp(0), 60, 1,
                1.0, 1.0, 1000, 900, 900, 0.1, 0.1,
                2000, 1900, 1900, 0.05, 0.05, 0, 0,
                to_timestamp(0)
            ),
            (
                'multi-day-telemetry', to_timestamp(172800), 60, 1,
                2.0, 2.0, 1000, 800, 800, 0.2, 0.2,
                2000, 1800, 1800, 0.1, 0.1, 0, 0,
                to_timestamp(172800)
            ),
            (
                'multi-day-telemetry', to_timestamp(345600), 60, 1,
                3.0, 3.0, 1000, 700, 700, 0.3, 0.3,
                2000, 1700, 1700, 0.15, 0.15, 0, 0,
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
                sample_count, rx_bytes_avg, tx_bytes_avg,
                rx_bytes_last, tx_bytes_last, rx_counter_epoch, tx_counter_epoch
            )
            VALUES (
                'rate-selection-telemetry', $1, to_timestamp($2::double precision),
                60, 1, $3, $4, $3, $4, 0, 0
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
        sqlx::query(
            r#"
            INSERT INTO telemetry_samples (
                id, client_id, observed_at, cpu_load_1,
                memory_total_bytes, memory_available_bytes, payload
            ) VALUES (
                $1, 'rate-selection-telemetry', to_timestamp($2::double precision),
                0, 0, 0, $3
            )
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(observed as f64)
        .bind(serde_json::json!({
            "networks": [
                {"interface": "eth0", "rx_bytes": eth0_rx, "tx_bytes": eth0_tx},
                {"interface": "lo", "rx_bytes": lo_rx, "tx_bytes": lo_tx},
            ]
        }))
        .execute(&db.pool)
        .await
        .unwrap();
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
            sample_count, rx_bytes_avg, tx_bytes_avg,
            rx_bytes_last, tx_bytes_last, rx_counter_epoch, tx_counter_epoch
        ) VALUES
            ('reset-telemetry', 'eth0', to_timestamp($1::double precision), 60, 1, 1000, 2000, 1000, 2000, 0, 0),
            ('reset-telemetry', 'eth0', to_timestamp($2::double precision), 60, 1, 100, 2100, 100, 2100, 1, 0)
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
            sample_count, rx_bytes_avg, tx_bytes_avg,
            rx_bytes_last, tx_bytes_last, rx_counter_epoch, tx_counter_epoch
        ) VALUES (
            'reset-telemetry', 'eth0', to_timestamp($1::double precision), 60, 1, 160, 2200, 160, 2200, 1, 0
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
        sqlx::query(
            r#"
            INSERT INTO telemetry_samples (
                id, client_id, observed_at, cpu_load_1,
                memory_total_bytes, memory_available_bytes, payload
            ) VALUES (
                $1, 'raw-reset-telemetry', to_timestamp($2::double precision),
                0, 0, 0, $3
            )
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(observed as f64)
        .bind(serde_json::json!({
            "networks": [{"interface": "eth0", "rx_bytes": rx, "tx_bytes": tx}]
        }))
        .execute(&db.pool)
        .await
        .unwrap();
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
            rx_bytes_avg, tx_bytes_avg, rx_bytes_last, tx_bytes_last,
            rx_counter_epoch, tx_counter_epoch
        ) VALUES
            ('intra-reset-telemetry', 'eth0', to_timestamp($1::double precision), 60, 1,
                1000, 2000, 1000, 2000, 0, 0),
            ('intra-reset-telemetry', 'eth0', to_timestamp($2::double precision), 60, 2,
                650, 2150, 1200, 2200, 1, 0),
            ('intra-reset-telemetry', 'eth0', to_timestamp($3::double precision), 60, 1,
                1300, 2300, 1300, 2300, 1, 0)
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
        sqlx::query(
            r#"
            INSERT INTO telemetry_samples (
                id, client_id, observed_at, cpu_load_1,
                memory_total_bytes, memory_available_bytes, payload
            ) VALUES (
                $1, 'intra-reset-telemetry', to_timestamp($2::double precision),
                0, 0, 0, $3
            )
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(observed as f64)
        .bind(serde_json::json!({
            "networks": [{"interface": "eth0", "rx_bytes": rx, "tx_bytes": tx}]
        }))
        .execute(&db.pool)
        .await
        .unwrap();
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
            cpu_usage_sample_count, cpu_usage_avg, cpu_usage_max, cpu_cores_max,
            cpu_load_1_avg, cpu_load_1_max,
            cpu_load_5_avg, cpu_load_5_max, cpu_load_15_avg, cpu_load_15_max,
            memory_total_bytes_max, memory_available_bytes_avg,
            memory_available_bytes_min, memory_used_ratio_avg, memory_used_ratio_max,
            swap_sample_count, swap_total_bytes_max,
            swap_available_bytes_avg, swap_available_bytes_min,
            swap_used_ratio_avg, swap_used_ratio_max,
            disk_total_bytes_max, disk_available_bytes_avg,
            disk_available_bytes_min, disk_used_ratio_avg, disk_used_ratio_max,
            network_rx_bytes_max, network_tx_bytes_max, latest_observed_at
        ) VALUES (
            'adaptive-telemetry', to_timestamp($1::double precision), 300, 5,
            5, 0.25, 0.25, 2,
            0.5, 0.5, 0.4, 0.4, 0.3, 0.3,
            1000, 500, 500, 0.5, 0.5,
            1, 1000, 400, 400, 0.6, 0.6,
            2000, 1000, 1000, 0.5, 0.5, 0, 0,
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
    assert_eq!(adaptive_resources.len(), 4);
    assert!(adaptive_resources
        .iter()
        .all(|row| row.sample_count == 1 && row.cpu_usage_sample_count == 1));
    assert_eq!(
        adaptive_resources
            .iter()
            .filter(|row| row.swap_sample_count == 1)
            .count(),
        1
    );
    assert!(adaptive_resources.iter().all(|row| {
        if row.swap_sample_count == 0 {
            row.swap_total_bytes_max.is_none()
                && row.swap_available_bytes_avg.is_none()
                && row.swap_available_bytes_min.is_none()
                && row.swap_used_ratio_avg.is_none()
                && row.swap_used_ratio_max.is_none()
        } else {
            row.swap_total_bytes_max == Some(1_000)
                && row.swap_available_bytes_avg == Some(400)
                && row.swap_available_bytes_min == Some(400)
                && row.swap_used_ratio_avg == Some(0.6)
                && row.swap_used_ratio_max == Some(0.6)
        }
    }));

    sqlx::query(
        r#"
        INSERT INTO telemetry_network_rates (
            client_id, interface, bucket_start, bucket_secs,
            sample_count, rx_bytes_avg, tx_bytes_avg,
            rx_bytes_last, tx_bytes_last, rx_counter_epoch, tx_counter_epoch
        ) VALUES
            ('adaptive-telemetry', 'eth0', to_timestamp($1::double precision), 60, 1, 1000, 2000, 1000, 2000, 0, 0),
            ('adaptive-telemetry', 'eth0', to_timestamp($2::double precision), 300, 5, 1600, 2900, 1600, 2900, 0, 0)
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
    assert_eq!(adaptive_network.len(), 5);
    assert_eq!(adaptive_network[0].rx_bytes_delta, 600);
    assert!(adaptive_network[1..]
        .iter()
        .all(|row| row.rx_bytes_delta == 0 && row.rx_bps_avg == 0.0));

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
        INSERT INTO telemetry_ping_rollups (
            client_id, target_id, generation, bucket_start, bucket_secs,
            sample_count, success_count, latency_avg_ms, latency_min_ms, latency_max_ms,
            loss_ratio_avg, loss_ratio_max, latest_status, latest_checked_at
        ) VALUES (
            'adaptive-telemetry', $1, 1, to_timestamp($2::double precision), 300,
            5, 5, 12, 12, 12, 0, 0, 'ok',
            to_timestamp(($2::bigint + 299)::double precision)
        )
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
    assert_eq!(adaptive_ping.len(), 5);
    assert!(adaptive_ping
        .iter()
        .all(|row| row.sample_count == 1 && row.success_count == 1));

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
        INSERT INTO telemetry_rollups (
            client_id, bucket_start, bucket_secs, sample_count,
            cpu_load_1_avg, cpu_load_1_max,
            memory_total_bytes_max, memory_available_bytes_avg,
            memory_available_bytes_min, memory_used_ratio_avg, memory_used_ratio_max,
            disk_total_bytes_max, disk_available_bytes_avg,
            disk_available_bytes_min, disk_used_ratio_avg, disk_used_ratio_max,
            network_rx_bytes_max, network_tx_bytes_max, latest_observed_at
        )
        SELECT
            id, date_trunc('minute', now()), 60, 1,
            2.0, 2.0, 1000, 500, 500, 0.5, 0.5,
            2000, 1500, 1500, 0.25, 0.25, 0, 0, now()
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
            rules: vec![PolicyRuleRequest {
                id: None,
                name: "all-client threshold".to_string(),
                enabled: true,
                traffic_selector: None,
                condition_expression: "cpu.load_1 >= 1".to_string(),
                window_secs: 0,
                severity: "warning".to_string(),
            }],
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
async fn postgres_fleet_alert_candidates_survive_public_caps_and_parse_real_skips() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    for (client_id, display_name, key_byte) in [
        ("alert-target-a", "Alert Target A", 91_u8),
        ("alert-target-b", "Alert Target B", 92_u8),
        ("alert-filler", "Alert Filler", 93_u8),
    ] {
        sqlx::query(
            r#"
            INSERT INTO clients (id, display_name, public_key, status)
            VALUES ($1, $2, $3, 'online')
            "#,
        )
        .bind(client_id)
        .bind(display_name)
        .bind(vec![key_byte; 32])
        .execute(&db.pool)
        .await
        .unwrap();
    }

    let policy_alert_id = Uuid::new_v4();
    let policy_group_id = Uuid::new_v4();
    let policy_rule_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO policy_alerts (
            id, policy_group_id, policy_rule_id, client_id, trigger_generation,
            severity, category, title, detail, payload, observed_at, created_at
        )
        VALUES (
            $1, $2, $3, 'alert-target-a', 0,
            'warning', 'resource', 'Older scoped alert',
            'must survive unrelated newer rows', '{"regression":"policy_candidate"}'::jsonb,
            '2020-01-01T00:00:00Z', '2020-01-01T00:00:00Z'
        )
        "#,
    )
    .bind(policy_alert_id)
    .bind(policy_group_id)
    .bind(policy_rule_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO policy_alerts (
            id, policy_group_id, policy_rule_id, client_id, trigger_generation,
            severity, category, title, detail, payload, observed_at, created_at
        )
        SELECT
            md5('fleet-policy-filler-' || value::text)::uuid,
            $1, $2, 'alert-filler', value,
            'critical', 'resource', 'Newer filler', 'outside requested client',
            '{}'::jsonb, now() + value * interval '1 second',
            now() + value * interval '1 second'
        FROM generate_series(1, 200) AS generated(value)
        "#,
    )
    .bind(policy_group_id)
    .bind(policy_rule_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let failed_backup_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO backup_requests (
            id, client_id, paths, include_config, status, payload_hash,
            command_scope, created_at
        )
        VALUES (
            $1, 'alert-target-a', ARRAY['/srv/app'], true, 'execution_failed',
            repeat('a', 64), 'client:alert-target-a', '2020-01-01T00:00:00Z'
        )
        "#,
    )
    .bind(failed_backup_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO backup_requests (
            id, client_id, paths, include_config, status, payload_hash,
            command_scope, created_at
        )
        SELECT
            md5('fleet-backup-filler-' || value::text)::uuid,
            'alert-filler', ARRAY['/tmp'], false, 'artifact_metadata_recorded',
            repeat('b', 64), 'client:alert-filler',
            now() + value * interval '1 second'
        FROM generate_series(1, 1001) AS generated(value)
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let scoped_failed_backups = db
        .repo
        .list_failed_backup_request_candidates(Some("alert-target-a"), None, None, None, 200)
        .await
        .unwrap();
    assert_eq!(scoped_failed_backups.len(), 1);
    assert_eq!(scoped_failed_backups[0].id, failed_backup_id);

    let failed_job_id = Uuid::new_v4();
    let capability_job_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, command_type, privileged, status, target_count, payload_hash,
            request_fingerprint, max_timeout_secs, created_at, completed_at
        )
        VALUES
            (
                $1, 'shell', false, 'failed', 1, repeat('c', 64),
                'fleet-failed-job', 30,
                '2020-01-01T00:00:00Z', '2020-01-01T00:01:00Z'
            ),
            (
                $2, 'agent_update', true, 'skipped', 1, repeat('d', 64),
                'fleet-capability-job', 30,
                '2020-01-01T00:02:00Z', '2020-01-01T00:03:00Z'
            )
        "#,
    )
    .bind(failed_job_id)
    .bind(capability_job_id)
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
            md5('fleet-job-filler-' || value::text)::uuid,
            'shell', false, 'completed', 1, repeat('e', 64),
            'fleet-job-filler-' || value::text, 30,
            now() + value * interval '1 second',
            now() + value * interval '1 second'
        FROM generate_series(1, 200) AS generated(value)
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO job_targets (
            job_id, client_id, status, message, exit_code,
            started_at, completed_at,
            capability_degraded_reason, capability_degraded_hint
        )
        VALUES (
            $1, 'alert-target-b', 'skipped',
            'target agent lacks agent update capability', 0,
            '2020-01-01T00:02:00Z', '2020-01-01T00:03:00Z',
            'target_agent_lacks_agent_update_capability',
            'Run the agent with host mutation capability before retrying.'
        )
        "#,
    )
    .bind(capability_job_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let capability_reason = "target_agent_lacks_agent_update_capability";
    let capability_hint = "Run the agent with host mutation capability before retrying.";
    let capability_output = serde_json::to_vec(&serde_json::json!({
        "type": "capability_degraded",
        "status": "skipped",
        "client_id": "alert-target-b",
        "command_type": "agent_update",
        "reason": capability_reason,
        "hint": capability_hint,
    }))
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO job_outputs (
            job_id, client_id, seq, stream, data, exit_code, done, storage, created_at
        )
        VALUES (
            $1, 'alert-target-b', 0, 'status', $2, 0, true, 'inline',
            '2020-01-01T00:03:00Z'
        )
        "#,
    )
    .bind(capability_job_id)
    .bind(capability_output)
    .execute(&db.pool)
    .await
    .unwrap();

    let operator = postgres_network_operator(&db.repo).await;
    let tunnel_input = postgres_alert_test_tunnel_input();
    crate::tests_network::seed_test_plan_adapter_definitions(&db.repo, &tunnel_input).await;
    let tunnel_plan = plan_tunnel(&tunnel_input).unwrap();
    let saved_tunnel = db
        .repo
        .record_tunnel_plan(&tunnel_input, &tunnel_plan, true, &operator)
        .await
        .unwrap();
    sqlx::query(
        r#"
        INSERT INTO telemetry_tunnels (
            client_id, observed_at, interface, kind, ownership_mode,
            mutation_policy, source, telemetry_plan_id, telemetry_plan_name,
            telemetry_plan_runtime_manager, telemetry_endpoint_side,
            telemetry_peer_client_id, adapter_health
        )
        VALUES (
            'alert-target-a', '2020-01-01T00:00:00Z', 'gre42', 'gre',
            'custom_adapter', 'adapter_owned', 'telemetry',
            $1, $2, 'custom_adapter', 'left', 'alert-target-b',
            '{"status":"failed"}'::jsonb
        )
        "#,
    )
    .bind(saved_tunnel.id.to_string())
    .bind(&saved_tunnel.name)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO telemetry_tunnels (
            client_id, observed_at, interface, kind, ownership_mode,
            mutation_policy, source, adapter_health
        )
        SELECT
            'alert-filler', now() + value * interval '1 second',
            'filler' || value::text, 'gre', 'custom_adapter',
            'adapter_owned', 'telemetry', '{"status":"ok","success":true}'::jsonb
        FROM generate_series(1, 5000) AS generated(value)
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    assert!(!db
        .repo
        .list_policy_alerts(&PolicyAlertQuery {
            limit: Some(200),
            client_id: None,
            severity: None,
            category: None,
            policy_group_id: None,
        })
        .await
        .unwrap()
        .iter()
        .any(|alert| alert.id == policy_alert_id));
    assert!(!db
        .repo
        .list_backup_requests(200)
        .await
        .unwrap()
        .iter()
        .any(|backup| backup.id == failed_backup_id));
    assert!(db
        .repo
        .list_jobs(200)
        .await
        .unwrap()
        .iter()
        .all(|job| job.id != failed_job_id && job.id != capability_job_id));
    assert!(
        db.repo
            .list_telemetry_tunnels(5_000, None, None)
            .await
            .unwrap()
            .iter()
            .any(|tunnel| tunnel.client_id == "alert-target-a"),
        "undeclared telemetry noise must be discarded before the public tunnel page limit"
    );

    let policy_fleet_alert_id = format!("policy-alert:{policy_alert_id}");
    sqlx::query(
        r#"
        INSERT INTO fleet_alert_states (
            alert_id, state, escalation_level, reason, created_at, updated_at
        )
        VALUES (
            $1, 'acknowledged', 0, 'known old policy alert',
            '2020-01-01T00:00:00Z', '2020-01-01T00:00:00Z'
        )
        "#,
    )
    .bind(&policy_fleet_alert_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO fleet_alert_states (
            alert_id, state, escalation_level, created_at, updated_at
        )
        SELECT
            'unrelated:' || value::text, 'open', 0,
            now() + value * interval '1 second',
            now() + value * interval '1 second'
        FROM generate_series(1, 1000) AS generated(value)
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    assert!(db
        .repo
        .list_fleet_alert_states(1_000, None)
        .await
        .unwrap()
        .iter()
        .all(|state| state.alert_id != policy_fleet_alert_id));

    let state = postgres_app_state(&db);
    let policy_alerts = state
        .list_fleet_alerts(FleetAlertQuery {
            limit: Some(1),
            client_id: Some("alert-target-a".to_string()),
            severity: Some("warning".to_string()),
            category: Some("resource".to_string()),
            operator_state: Some("acknowledged".to_string()),
            include_muted: None,
        })
        .await
        .unwrap();
    assert_eq!(policy_alerts.len(), 1);
    assert_eq!(policy_alerts[0].id, policy_fleet_alert_id);

    let backup_alerts = state
        .list_fleet_alerts(FleetAlertQuery {
            limit: Some(1),
            client_id: Some("alert-target-a".to_string()),
            severity: Some("critical".to_string()),
            category: Some("backup".to_string()),
            operator_state: None,
            include_muted: None,
        })
        .await
        .unwrap();
    assert_eq!(backup_alerts.len(), 1);
    assert_eq!(backup_alerts[0].target_id, failed_backup_id.to_string());

    let job_alerts = state
        .list_fleet_alerts(FleetAlertQuery {
            limit: Some(1),
            client_id: None,
            severity: Some("critical".to_string()),
            category: Some("job".to_string()),
            operator_state: None,
            include_muted: None,
        })
        .await
        .unwrap();
    assert_eq!(job_alerts.len(), 1);
    assert_eq!(job_alerts[0].target_id, failed_job_id.to_string());

    let capability_alerts = state
        .list_fleet_alerts(FleetAlertQuery {
            limit: Some(1),
            client_id: Some("alert-target-b".to_string()),
            severity: Some("warning".to_string()),
            category: Some("capability_degraded".to_string()),
            operator_state: None,
            include_muted: None,
        })
        .await
        .unwrap();
    assert_eq!(capability_alerts.len(), 1);
    assert_eq!(capability_alerts[0].status, capability_reason);
    assert_eq!(capability_alerts[0].detail, capability_hint);
    assert_eq!(capability_alerts[0].evidence["target_status"], "skipped");

    let tunnel_alerts = state
        .list_fleet_alerts(FleetAlertQuery {
            limit: Some(1),
            client_id: Some("alert-target-a".to_string()),
            severity: Some("critical".to_string()),
            category: Some("network".to_string()),
            operator_state: None,
            include_muted: None,
        })
        .await
        .unwrap();
    assert_eq!(tunnel_alerts.len(), 1);
    assert_eq!(tunnel_alerts[0].status, "tunnel_adapter_degraded");
    assert_eq!(
        tunnel_alerts[0].evidence["adapter_health"]["success"],
        false
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_tunnel_adapter_failures_only_degrade_custom_adapter_plans() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let operator = postgres_network_operator(&db.repo).await;
    let cases = [
        (
            RuntimeTunnelManager::AgentBuiltin,
            "agent_builtin",
            "custom_adapter",
            "skipped",
            false,
        ),
        (
            RuntimeTunnelManager::ExternalObserved,
            "external_observed",
            "external_observed",
            "skipped",
            false,
        ),
        (
            RuntimeTunnelManager::CustomAdapter,
            "custom_adapter",
            "agent_builtin",
            "failed",
            true,
        ),
    ];
    for (index, (manager, manager_label, stored_manager, health_status, expected_degraded)) in
        cases.into_iter().enumerate()
    {
        let left_client_id = format!("adapter-semantics-{index}-a");
        let right_client_id = format!("adapter-semantics-{index}-b");
        for (client_id, key_byte) in [
            (left_client_id.as_str(), 110_u8 + index as u8 * 2),
            (right_client_id.as_str(), 111_u8 + index as u8 * 2),
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
        let mut input = crate::tests_network::test_plan_input(manager, false);
        input.name = format!("adapter-semantics-{index}");
        input.interface_name = format!("tas{index}");
        input.left_client_id = left_client_id.clone();
        input.right_client_id = right_client_id.clone();
        input.address_pool_cidr = format!("10.20.{index}.0/29");
        input.ipv4_tunnel = Some(TunnelAddressPair {
            left: format!("10.20.{index}.0"),
            right: format!("10.20.{index}.1"),
            prefix_len: 31,
        });
        crate::tests_network::seed_test_plan_adapter_definitions(&db.repo, &input).await;
        let plan = plan_tunnel(&input).unwrap();
        let saved = db
            .repo
            .record_tunnel_plan(&input, &plan, true, &operator)
            .await
            .unwrap();
        sqlx::query(
            r#"
            INSERT INTO telemetry_tunnels (
                client_id, observed_at, interface, kind, ownership_mode,
                mutation_policy, source, telemetry_plan_id, telemetry_plan_name,
                telemetry_plan_runtime_manager, telemetry_endpoint_side,
                telemetry_peer_client_id, traffic_status, adapter_health
            )
            VALUES (
                $1, '2020-01-01T00:00:00Z', $2, 'wireguard', $3,
                'managed_desired', 'telemetry', $4, $5, $6, 'left', $7, 'ok',
                jsonb_build_object(
                    'status', $8::text,
                    'configured', false,
                    'success', false
                )
            )
            "#,
        )
        .bind(&left_client_id)
        .bind(&input.interface_name)
        .bind(manager_label)
        .bind(saved.id.to_string())
        .bind(&saved.name)
        .bind(stored_manager)
        .bind(&right_client_id)
        .bind(health_status)
        .execute(&db.pool)
        .await
        .unwrap();

        let candidates = db
            .repo
            .list_fleet_alert_tunnel_candidates(
                Some(&left_client_id),
                None,
                Some("critical"),
                None,
                None,
                200,
            )
            .await
            .unwrap();
        assert_eq!(!candidates.is_empty(), expected_degraded, "{manager_label}");
        let network_alerts = postgres_app_state(&db)
            .list_fleet_alerts(FleetAlertQuery {
                limit: Some(10),
                client_id: Some(left_client_id),
                severity: Some("critical".to_string()),
                category: Some("network".to_string()),
                operator_state: None,
                include_muted: None,
            })
            .await
            .unwrap();
        assert_eq!(
            !network_alerts.is_empty(),
            expected_degraded,
            "{manager_label}"
        );
    }

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
            memory_total_bytes_max, memory_available_bytes_avg,
            memory_available_bytes_min, memory_used_ratio_avg, memory_used_ratio_max,
            disk_total_bytes_max, disk_available_bytes_avg,
            disk_available_bytes_min, disk_used_ratio_avg, disk_used_ratio_max,
            network_rx_bytes_max, network_tx_bytes_max, latest_observed_at
        )
        VALUES (
            $1, date_trunc('minute', now()), 60, 1,
            2.0, 2.0, 1000, 500, 500, 0.5, 0.5,
            2000, 1500, 1500, 0.25, 0.25, 0, 0, now()
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
