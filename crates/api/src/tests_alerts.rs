use super::*;
use vpsman_common::{AgentCapabilitySnapshot, AgentPrivilegeMode};

#[tokio::test]
async fn fleet_alerts_derive_actionable_current_status() {
    let repo = Repository::Memory(MemoryState::default());
    let online = AgentView {
        id: "edge-a".to_string(),
        display_name: "Edge A".to_string(),
        status: "online".to_string(),
        tags: vec!["bgp".to_string()],
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: AgentCapabilitySnapshot::default(),
    };
    let stale = AgentView {
        id: "edge-b".to_string(),
        display_name: "Edge B".to_string(),
        status: "stale".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: AgentCapabilitySnapshot {
            privilege_mode: AgentPrivilegeMode::Unprivileged,
            effective_uid: Some(1000),
            command_timeout_secs: 3600,
            can_attempt_privileged_ops: true,
            can_manage_runtime_tunnels: false,
            can_apply_process_limits: false,
            unprivileged_hint: Some("agent is running without root".to_string()),
        },
    };
    let backup_job = Uuid::new_v4();
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.extend([online, stale]);
        memory
            .telemetry_tunnels
            .write()
            .await
            .push(TelemetryTunnelView {
                client_id: "edge-a".to_string(),
                observed_at: "200".to_string(),
                interface: "gre42".to_string(),
                kind: "gre".to_string(),
                ownership_mode: "managed".to_string(),
                mutation_policy: "managed".to_string(),
                promotion_required: false,
                plan_correlation: "managed_desired".to_string(),
                plan_id: None,
                plan_name: Some("edge-a-gre42".to_string()),
                plan_runtime_manager: Some("agent_iproute2_managed".to_string()),
                endpoint_side: Some("left".to_string()),
                peer_client_id: Some("edge-b".to_string()),
                source: "telemetry".to_string(),
                operstate: Some("up".to_string()),
                mtu: Some(1476),
                link_type: Some(778),
                address: Some("10.0.0.1".to_string()),
                rx_bytes: 100,
                tx_bytes: 200,
                traffic_source: Some("vnstat".to_string()),
                traffic_status: Some("degraded".to_string()),
                traffic_reason: Some("vnstat missing".to_string()),
                traffic_checked_unix: Some(200),
                adapter_health: Some(TelemetryTunnelAdapterHealthView {
                    status: "failed".to_string(),
                    checked_unix: 200,
                    configured: true,
                    success: false,
                    exit_code: Some(1),
                    reason: Some("adapter exited".to_string()),
                    duration_ms: 12,
                    command_sha256_hex: Some("00".repeat(32)),
                    timed_out: false,
                    output_truncated: false,
                    stdout_sha256_hex: None,
                    stderr_sha256_hex: None,
                }),
                latency_monitoring_enabled: None,
                latency_status: None,
                latency_reason: None,
                latency_primary_family: None,
                latency_target: None,
                latency_checked_unix: None,
                latency_avg_ms: None,
                packet_loss_ratio: None,
                latency_healthy_windows: None,
                latency_missed_windows: None,
                auto_ospf_enabled: None,
                auto_ospf_status: None,
                auto_ospf_reason: None,
                auto_ospf_current_cost: None,
                auto_ospf_recommended_cost: None,
                auto_ospf_updated_unix: None,
            });
        memory.jobs.write().await.push(JobHistoryView {
            id: backup_job,
            actor_id: None,
            command_type: "backup".to_string(),
            privileged: true,
            status: "failed".to_string(),
            target_count: 1,
            payload_hash: "aa".repeat(32),
            created_at: "100".to_string(),
            completed_at: Some("110".to_string()),
        });
        memory.job_targets.write().await.push(JobTargetView {
            job_id: backup_job,
            client_id: "edge-b".to_string(),
            status: "degraded_unprivileged".to_string(),
            message: None,
            exit_code: None,
            started_at: Some("105".to_string()),
            completed_at: Some("110".to_string()),
            process_incarnation_id: None,
        });
    }

    let state = alert_test_state(repo);
    let alerts = state
        .list_fleet_alerts(FleetAlertQuery {
            limit: Some(100),
            client_id: None,
            severity: None,
            category: None,
            operator_state: None,
            include_muted: None,
        })
        .await
        .unwrap();
    assert_alert_category(&alerts, "agent_status");
    assert_alert_category(&alerts, "network");
    assert_alert_category(&alerts, "backup");
    assert_alert_category(&alerts, "unprivileged_blocked");
    assert_alert_category(&alerts, "source_readiness");
    assert!(alerts
        .windows(2)
        .all(|pair| severity_rank_for_test(&pair[0].severity)
            <= severity_rank_for_test(&pair[1].severity)));

    let edge_b = state
        .list_fleet_alerts(FleetAlertQuery {
            limit: Some(100),
            client_id: Some("edge-b".to_string()),
            severity: Some("warning".to_string()),
            category: None,
            operator_state: None,
            include_muted: None,
        })
        .await
        .unwrap();
    assert!(edge_b.iter().all(|alert| {
        alert.client_id.as_deref() == Some("edge-b") && alert.severity == "warning"
    }));
    assert_alert_category(&edge_b, "agent_status");
    assert_alert_category(&edge_b, "unprivileged_blocked");
}

#[tokio::test]
async fn fleet_alerts_apply_scoped_resource_policy_overrides() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.extend([
            AgentView {
                id: "edge-a".to_string(),
                display_name: "Edge A".to_string(),
                status: "online".to_string(),
                tags: vec!["edge".to_string(), "provider:provider-a".to_string()],
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                internal_build_number: 1,
                process_incarnation_id: None,
                stale_since: None,
                stale_reason: None,
                capabilities: AgentCapabilitySnapshot::default(),
            },
            AgentView {
                id: "edge-b".to_string(),
                display_name: "Edge B".to_string(),
                status: "online".to_string(),
                tags: Vec::new(),
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                internal_build_number: 1,
                process_incarnation_id: None,
                stale_since: None,
                stale_reason: None,
                capabilities: AgentCapabilitySnapshot::default(),
            },
        ]);
        memory.telemetry_rollups.write().await.extend([
            alert_test_rollup("edge-a", 1.2, 300, 800),
            alert_test_rollup("edge-b", 1.2, 300, 800),
        ]);
    }
    repo.upsert_fleet_alert_policy(
        &CreateFleetAlertPolicyRequest {
            id: None,
            name: "provider-a-cpu".to_string(),
            scope_kind: "provider".to_string(),
            scope_value: Some("provider-a".to_string()),
            memory_available_warning_ratio: None,
            memory_available_critical_ratio: None,
            disk_available_warning_ratio: None,
            disk_available_critical_ratio: None,
            cpu_load_warning: Some(1.0),
            cpu_load_critical: Some(2.0),
            priority: Some(10),
            enabled: Some(true),
            notes: Some("provider-a hosts are small".to_string()),
            confirmed: true,
        },
        &operator,
    )
    .await
    .unwrap();
    repo.upsert_fleet_alert_policy(
        &CreateFleetAlertPolicyRequest {
            id: None,
            name: "edge-memory".to_string(),
            scope_kind: "tag".to_string(),
            scope_value: Some("edge".to_string()),
            memory_available_warning_ratio: Some(0.50),
            memory_available_critical_ratio: Some(0.20),
            disk_available_warning_ratio: None,
            disk_available_critical_ratio: None,
            cpu_load_warning: None,
            cpu_load_critical: None,
            priority: Some(20),
            enabled: Some(true),
            notes: None,
            confirmed: true,
        },
        &operator,
    )
    .await
    .unwrap();

    let state = alert_test_state(repo);
    let alerts = state
        .list_fleet_alerts(FleetAlertQuery {
            limit: Some(100),
            client_id: None,
            severity: None,
            category: None,
            operator_state: None,
            include_muted: None,
        })
        .await
        .unwrap();
    let edge_a_cpu = alerts
        .iter()
        .find(|alert| {
            alert.client_id.as_deref() == Some("edge-a") && alert.status == "cpu_load_high"
        })
        .expect("missing provider-scoped CPU alert");
    assert_eq!(edge_a_cpu.severity, "warning");
    assert_eq!(edge_a_cpu.evidence["threshold"].as_f64().unwrap(), 1.0);
    assert!(edge_a_cpu.evidence["alert_policy"]["matched_policy_ids"]
        .as_array()
        .is_some_and(|ids| !ids.is_empty()));

    let edge_a_memory = alerts
        .iter()
        .find(|alert| alert.client_id.as_deref() == Some("edge-a") && alert.status == "memory_low")
        .expect("missing tag-scoped memory alert");
    assert_eq!(edge_a_memory.severity, "warning");
    assert_eq!(
        edge_a_memory.evidence["warning_threshold"]
            .as_f64()
            .unwrap(),
        0.50
    );

    assert!(!alerts
        .iter()
        .any(|alert| alert.client_id.as_deref() == Some("edge-b")
            && matches!(alert.status.as_str(), "cpu_load_high" | "memory_low")));

    let policies = state
        .repo
        .list_fleet_alert_policies(20, Some(true), Some("tag"), Some("edge"))
        .await
        .unwrap();
    assert_eq!(policies.len(), 1);
    assert_eq!(policies[0].name, "edge-memory");
}

#[tokio::test]
async fn fleet_alerts_merge_operator_state_and_filter_muted_alerts() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.push(AgentView {
            id: "edge-muted".to_string(),
            display_name: "Edge Muted".to_string(),
            status: "stale".to_string(),
            tags: Vec::new(),
            registration_ip: None,
            last_ip: None,
            last_seen_at: None,
            internal_build_number: 1,
            process_incarnation_id: None,
            stale_since: None,
            stale_reason: None,
            capabilities: AgentCapabilitySnapshot::default(),
        });
    }
    let state = alert_test_state(repo);
    let operator = test_operator();
    let alerts = state
        .list_fleet_alerts(FleetAlertQuery {
            limit: Some(100),
            client_id: Some("edge-muted".to_string()),
            severity: None,
            category: Some("agent_status".to_string()),
            operator_state: None,
            include_muted: None,
        })
        .await
        .unwrap();
    let alert_id = alerts[0].id.clone();

    let muted = state
        .repo
        .update_fleet_alert_state(
            &UpdateFleetAlertStateRequest {
                alert_id: alert_id.clone(),
                action: "mute".to_string(),
                muted_for_secs: Some(600),
                reason: Some("maintenance window".to_string()),
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(muted.state, "muted");
    assert!(muted.muted_until_unix.is_some());

    let visible = state
        .list_fleet_alerts(FleetAlertQuery {
            limit: Some(100),
            client_id: Some("edge-muted".to_string()),
            severity: None,
            category: Some("agent_status".to_string()),
            operator_state: None,
            include_muted: None,
        })
        .await
        .unwrap();
    assert!(
        visible.iter().all(|alert| alert.id != alert_id),
        "muted alerts are hidden by default"
    );

    let muted_alerts = state
        .list_fleet_alerts(FleetAlertQuery {
            limit: Some(100),
            client_id: Some("edge-muted".to_string()),
            severity: None,
            category: Some("agent_status".to_string()),
            operator_state: Some("muted".to_string()),
            include_muted: Some(true),
        })
        .await
        .unwrap();
    assert_eq!(muted_alerts.len(), 1);
    assert_eq!(muted_alerts[0].operator_state, "muted");
    assert_eq!(
        muted_alerts[0].state_reason.as_deref(),
        Some("maintenance window")
    );

    state
        .repo
        .update_fleet_alert_state(
            &UpdateFleetAlertStateRequest {
                alert_id: alert_id.clone(),
                action: "acknowledge".to_string(),
                muted_for_secs: None,
                reason: Some("operator reviewing".to_string()),
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    let acknowledged = state
        .list_fleet_alerts(FleetAlertQuery {
            limit: Some(100),
            client_id: Some("edge-muted".to_string()),
            severity: None,
            category: Some("agent_status".to_string()),
            operator_state: Some("acknowledged".to_string()),
            include_muted: None,
        })
        .await
        .unwrap();
    assert_eq!(acknowledged.len(), 1);

    let escalated = state
        .repo
        .update_fleet_alert_state(
            &UpdateFleetAlertStateRequest {
                alert_id,
                action: "escalate".to_string(),
                muted_for_secs: None,
                reason: Some("needs immediate action".to_string()),
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(escalated.state, "escalated");
    assert_eq!(escalated.escalation_level, 1);
}

#[tokio::test]
async fn fleet_alert_notifications_match_scope_and_dedupe_cooldown() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.extend([
            AgentView {
                id: "edge-a".to_string(),
                display_name: "Edge A".to_string(),
                status: "stale".to_string(),
                tags: vec!["edge".to_string(), "provider:provider-a".to_string()],
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                internal_build_number: 1,
                process_incarnation_id: None,
                stale_since: None,
                stale_reason: None,
                capabilities: AgentCapabilitySnapshot::default(),
            },
            AgentView {
                id: "core-a".to_string(),
                display_name: "Core A".to_string(),
                status: "online".to_string(),
                tags: vec!["core".to_string()],
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                internal_build_number: 1,
                process_incarnation_id: None,
                stale_since: None,
                stale_reason: None,
                capabilities: AgentCapabilitySnapshot::default(),
            },
        ]);
    }
    repo.upsert_fleet_alert_notification_channel(
        &CreateFleetAlertNotificationChannelRequest {
            id: None,
            name: "edge-audit".to_string(),
            scope_kind: "tag".to_string(),
            scope_value: Some("edge".to_string()),
            min_severity: Some("warning".to_string()),
            categories: Some(vec!["agent_status".to_string()]),
            operator_states: Some(vec!["open".to_string()]),
            delivery_kind: "audit_log".to_string(),
            target: "audit:fleet".to_string(),
            cooldown_secs: Some(900),
            enabled: Some(true),
            notes: Some("page edge operators".to_string()),
            confirmed: true,
        },
        &operator,
    )
    .await
    .unwrap();
    repo.upsert_fleet_alert_notification_channel(
        &CreateFleetAlertNotificationChannelRequest {
            id: None,
            name: "provider-webhook".to_string(),
            scope_kind: "provider".to_string(),
            scope_value: Some("provider-a".to_string()),
            min_severity: Some("info".to_string()),
            categories: Some(vec!["agent_status".to_string()]),
            operator_states: Some(Vec::new()),
            delivery_kind: "webhook".to_string(),
            target: "https://alerts.example.invalid/vpsman".to_string(),
            cooldown_secs: Some(900),
            enabled: Some(true),
            notes: None,
            confirmed: true,
        },
        &operator,
    )
    .await
    .unwrap();
    repo.upsert_fleet_alert_notification_channel(
        &CreateFleetAlertNotificationChannelRequest {
            id: None,
            name: "provider-custom-adapter".to_string(),
            scope_kind: "provider".to_string(),
            scope_value: Some("provider-a".to_string()),
            min_severity: Some("info".to_string()),
            categories: Some(vec!["agent_status".to_string()]),
            operator_states: Some(Vec::new()),
            delivery_kind: "custom_pager".to_string(),
            target: "adapter:custom-pager".to_string(),
            cooldown_secs: Some(900),
            enabled: Some(true),
            notes: None,
            confirmed: true,
        },
        &operator,
    )
    .await
    .unwrap();

    let state = alert_test_state(repo);
    let dry_run = state
        .dispatch_fleet_alert_notifications(
            &FleetAlertNotificationDispatchRequest {
                limit: Some(100),
                client_id: Some("edge-a".to_string()),
                severity: None,
                category: Some("agent_status".to_string()),
                operator_state: None,
                include_muted: None,
                dry_run: Some(true),
                confirmed: false,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(dry_run.len(), 3);
    assert!(dry_run
        .iter()
        .all(|delivery| delivery.status == "matched_dry_run"));

    let delivered = state
        .dispatch_fleet_alert_notifications(
            &FleetAlertNotificationDispatchRequest {
                limit: Some(100),
                client_id: Some("edge-a".to_string()),
                severity: None,
                category: Some("agent_status".to_string()),
                operator_state: None,
                include_muted: None,
                dry_run: Some(false),
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(delivered.len(), 3);
    assert!(delivered.iter().any(|row| row.status == "delivered"
        && row.delivery_kind == "audit_log"
        && row.payload["schema"] == "vpsman.fleet_alert.notification.v1"));
    assert!(delivered
        .iter()
        .any(|row| row.status == "queued" && row.delivery_kind == "webhook"));
    assert!(delivered
        .iter()
        .any(|row| row.status == "queued" && row.delivery_kind == "custom_pager"));

    let duplicate = state
        .dispatch_fleet_alert_notifications(
            &FleetAlertNotificationDispatchRequest {
                limit: Some(100),
                client_id: Some("edge-a".to_string()),
                severity: None,
                category: Some("agent_status".to_string()),
                operator_state: None,
                include_muted: None,
                dry_run: None,
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert!(
        duplicate.is_empty(),
        "cooldown dedupe should suppress repeated delivery records"
    );

    let stored = state
        .repo
        .list_fleet_alert_notification_deliveries(20, None, None, Some("queued"))
        .await
        .unwrap();
    assert_eq!(stored.len(), 2);
    assert!(stored.iter().all(|row| row.attempt_count == 0));

    let process_dry_run = state
        .process_fleet_alert_notifications(
            &FleetAlertNotificationProcessRequest {
                limit: Some(20),
                status: Some("queued".to_string()),
                delivery_kind: Some("webhook".to_string()),
                dry_run: Some(true),
                confirmed: false,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(process_dry_run.len(), 1);
    assert_eq!(process_dry_run[0].status, "delivery_dry_run");
    let after_dry_run = state
        .repo
        .list_fleet_alert_notification_deliveries(20, None, None, Some("queued"))
        .await
        .unwrap();
    assert_eq!(after_dry_run.len(), 2);
    assert!(after_dry_run.iter().all(|row| row.attempt_count == 0));

    let failed_custom = state
        .process_fleet_alert_notifications(
            &FleetAlertNotificationProcessRequest {
                limit: Some(20),
                status: Some("queued".to_string()),
                delivery_kind: Some("custom_pager".to_string()),
                dry_run: Some(false),
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(failed_custom.len(), 1);
    assert_eq!(failed_custom[0].status, "failed");
    assert_eq!(failed_custom[0].attempt_count, 1);
    assert!(failed_custom[0]
        .error
        .as_deref()
        .is_some_and(|error| error.contains("not configured")));
    if let Repository::Memory(memory) = &state.repo {
        let audits = memory.audits.read().await;
        assert!(audits
            .iter()
            .any(|audit| audit.action == "fleet.alert_notification_deliveries_processed"));
    }
}

fn alert_test_state(repo: Repository) -> AppState {
    AppState {
        repo,
        events: tokio::sync::broadcast::channel(4).0,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        backup_object_store: None,
        update_object_store: None,
        update_artifact_public_base_url: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: std::path::PathBuf::from("config/vpsman.toml"),
        dispatcher_config: crate::state::DispatcherRuntimeConfig::default(),
    }
}

fn alert_test_rollup(
    client_id: &str,
    cpu_load_1_max: f64,
    memory_available: i64,
    disk_available: i64,
) -> TelemetryRollupView {
    TelemetryRollupView {
        client_id: client_id.to_string(),
        bucket_start: "100".to_string(),
        bucket_secs: 60,
        sample_count: 3,
        cpu_load_1_avg: cpu_load_1_max,
        cpu_load_1_max,
        memory_total_bytes_max: 1000,
        memory_available_bytes_avg: memory_available,
        memory_available_bytes_min: memory_available,
        disk_total_bytes_max: 2000,
        disk_available_bytes_avg: disk_available,
        disk_available_bytes_min: disk_available,
        network_rx_bytes_max: 0,
        network_tx_bytes_max: 0,
        latest_observed_at: "120".to_string(),
        updated_at: "121".to_string(),
    }
}

fn test_operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::new_v4(),
            username: "test-admin".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::new_v4(),
    }
}

fn assert_alert_category(alerts: &[FleetAlertView], category: &str) {
    assert!(
        alerts.iter().any(|alert| alert.category == category),
        "missing {category} alert in {alerts:#?}"
    );
}

fn severity_rank_for_test(severity: &str) -> usize {
    match severity {
        "critical" => 0,
        "warning" => 1,
        "info" => 2,
        _ => 3,
    }
}
