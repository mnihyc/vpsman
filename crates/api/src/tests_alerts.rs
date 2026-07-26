use super::*;
use serde_json::json;
use vpsman_common::{AgentCapabilitySnapshot, AgentPrivilegeMode};

#[tokio::test]
async fn fleet_alerts_derive_actionable_current_status() {
    let repo = Repository::Memory(MemoryState::default());
    let tunnel_input = vpsman_common::TunnelPlanInput {
        name: "edge-a-gre42".to_string(),
        interface_name: "gre42".to_string(),
        kind: vpsman_common::TunnelKind::Gre,
        runtime_control: vpsman_common::RuntimeTunnelControl {
            manager: vpsman_common::RuntimeTunnelManager::ExternalManagedAdapter,
            left_adapter_template_id: Some("11111111-1111-4111-8111-111111111111".to_string()),
            right_adapter_template_id: Some("22222222-2222-4222-8222-222222222222".to_string()),
            ..Default::default()
        },
        runtime_topology: Default::default(),
        left_client_id: "edge-a".to_string(),
        right_client_id: "edge-b".to_string(),
        left_remote_underlay: "198.51.100.10".to_string(),
        right_remote_underlay: "203.0.113.20".to_string(),
        left_local_underlay: None,
        right_local_underlay: None,
        address_pool_cidr: "10.42.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
            left: "10.42.0.0".to_string(),
            right: "10.42.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth_mbps: 100,
        ospf: None,
    };
    let tunnel_plan = vpsman_common::plan_tunnel(&tunnel_input).unwrap();
    let saved_tunnel = repo
        .record_tunnel_plan(&tunnel_input, &tunnel_plan, true, &test_operator())
        .await
        .unwrap();
    let online = AgentView {
        id: "edge-a".to_string(),
        display_name: "Edge A".to_string(),
        status: "online".to_string(),
        tags: vec!["bgp".to_string()],
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        arch: None,
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
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: AgentCapabilitySnapshot {
            privilege_mode: AgentPrivilegeMode::Unprivileged,
            effective_uid: Some(1000),
            max_job_timeout_secs: 3600,
            can_attempt_privileged_ops: true,
            can_manage_runtime_tunnels: false,
            can_apply_process_limits: false,
            port_forwarding: Default::default(),
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
                plan_id: Some(saved_tunnel.id),
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
            });
        memory.jobs.write().await.push(JobHistoryView {
            id: backup_job,
            actor_id: None,
            command_type: "backup".to_string(),
            source_schedule_id: None,
            privileged: true,
            status: "failed".to_string(),
            target_count: 1,
            payload_hash: "aa".repeat(32),
            max_timeout_secs: 30,
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
            deadline_at: None,
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
async fn fleet_alert_policy_groups_issue_resource_alerts() {
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
                arch: None,
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
                arch: None,
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
    let policy = repo
        .upsert_fleet_alert_policy(
            &CreateFleetAlertPolicyRequest {
                id: None,
                name: "edge-cpu".to_string(),
                enabled: true,
                selector_expression: "tag:edge".to_string(),
                rules: vec![PolicyRuleRequest {
                    id: None,
                    name: "cpu over one".to_string(),
                    enabled: true,
                    traffic_selector: None,
                    condition_expression: "cpu.load_1 >= 0.5 + 0.5".to_string(),
                    window_secs: 0,
                    severity: "warning".to_string(),
                }],
                notes: Some("edge hosts".to_string()),
                confirmed: true,
                preview_hash: None,
            },
            &operator,
        )
        .await
        .unwrap();

    let dry_run = repo
        .dry_run_fleet_alert_policy(&PolicyDryRunRequest {
            id: Some(policy.id),
            name: String::new(),
            enabled: true,
            selector_expression: "tag:edge".to_string(),
            rules: vec![PolicyRuleRequest {
                id: None,
                name: String::new(),
                enabled: true,
                traffic_selector: None,
                condition_expression: "cpu.load_1 >= (0.25 + 0.75) * 1".to_string(),
                window_secs: 0,
                severity: "warning".to_string(),
            }],
            notes: Some("edge hosts".to_string()),
        })
        .await
        .unwrap();
    assert_eq!(dry_run.matched_vps, vec!["edge-a".to_string()]);
    assert_eq!(dry_run.rule_previews[0].true_count, 1);
    assert_eq!(dry_run.rule_previews[0].false_count, 0);

    let alerts = repo
        .list_policy_alerts(&PolicyAlertQuery {
            limit: Some(20),
            client_id: Some("edge-a".to_string()),
            severity: Some("warning".to_string()),
            category: Some("resource".to_string()),
            policy_group_id: Some(policy.id),
        })
        .await
        .unwrap();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].category, "resource");
    assert_eq!(alerts[0].actual_value, Some(1.2));
    assert_eq!(alerts[0].threshold_value, Some(1.0));

    let state = alert_test_state(repo);
    let fleet_alerts = state
        .list_fleet_alerts(FleetAlertQuery {
            limit: Some(100),
            client_id: Some("edge-a".to_string()),
            severity: Some("warning".to_string()),
            category: Some("resource".to_string()),
            operator_state: None,
            include_muted: None,
        })
        .await
        .unwrap();
    assert!(fleet_alerts
        .iter()
        .any(|alert| alert.id.starts_with("policy-alert:")));

    let policies = state
        .repo
        .list_fleet_alert_policies(20, Some(true), Some("id:edge-a"), None)
        .await
        .unwrap();
    assert_eq!(policies.len(), 1);
    assert_eq!(policies[0].name, "edge-cpu");
    assert_eq!(policies[0].matched_vps_count, 1);
    assert_eq!(policies[0].active_warning_count, 1);
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
            arch: None,
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
        memory
            .operators
            .write()
            .await
            .push(test_operator_record(&operator));
        memory.agents.write().await.extend([
            AgentView {
                id: "edge-a".to_string(),
                display_name: "Edge A".to_string(),
                status: "stale".to_string(),
                tags: vec!["edge".to_string(), "provider:provider-a".to_string()],
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                arch: None,
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
                arch: None,
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
            name: "edge-webhook".to_string(),
            scope_kind: "tag".to_string(),
            scope_value: Some("edge".to_string()),
            min_severity: Some("warning".to_string()),
            categories: Some(vec!["agent_status".to_string()]),
            operator_states: Some(vec!["open".to_string()]),
            delivery_kind: "webhook".to_string(),
            target: "https://hooks.acme.com/fleet".to_string(),
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
            target: "https://hooks.acme.com/provider".to_string(),
            cooldown_secs: Some(900),
            enabled: Some(true),
            notes: None,
            confirmed: true,
        },
        &operator,
    )
    .await
    .unwrap();
    let unsupported_channel = repo
        .upsert_fleet_alert_notification_channel(
            &CreateFleetAlertNotificationChannelRequest {
                id: None,
                name: "provider-unsupported-adapter".to_string(),
                scope_kind: "provider".to_string(),
                scope_value: Some("provider-a".to_string()),
                min_severity: Some("info".to_string()),
                categories: Some(vec!["agent_status".to_string()]),
                operator_states: Some(Vec::new()),
                delivery_kind: "unsupported_adapter".to_string(),
                target: "adapter:unsupported".to_string(),
                cooldown_secs: Some(900),
                enabled: Some(true),
                notes: None,
                confirmed: true,
            },
            &operator,
        )
        .await;
    assert!(unsupported_channel.is_err());

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
                preview_hash: None,
                confirmed: false,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(dry_run.len(), 2);
    assert!(dry_run
        .iter()
        .all(|delivery| delivery.status == "matched_dry_run"));
    let dispatch_preview_hash = dry_run[0].review_preview_hash.clone();

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
                preview_hash: dispatch_preview_hash.clone(),
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(delivered.len(), 2);
    assert!(delivered.iter().all(|row| row.status == "queued"
        && row.delivery_kind == "webhook"
        && row.payload["schema"] == "vpsman.fleet_alert.notification.v1"));

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
                preview_hash: dispatch_preview_hash,
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

    if let Repository::Memory(memory) = &state.repo {
        for delivery in memory
            .fleet_alert_notification_deliveries
            .write()
            .await
            .iter_mut()
        {
            delivery.target = "https://127.0.0.1:9/blocked".to_string();
        }
    }
    let process_dry_run = state
        .process_fleet_alert_notifications(
            &FleetAlertNotificationProcessRequest {
                limit: Some(20),
                status: Some("queued".to_string()),
                delivery_kind: Some("webhook".to_string()),
                dry_run: Some(true),
                preview_hash: None,
                confirmed: false,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(process_dry_run.len(), 2);
    assert!(process_dry_run
        .iter()
        .all(|delivery| delivery.status == "delivery_dry_run"));
    let after_dry_run = state
        .repo
        .list_fleet_alert_notification_deliveries(20, None, None, Some("queued"))
        .await
        .unwrap();
    assert_eq!(after_dry_run.len(), 2);
    assert!(after_dry_run.iter().all(|row| row.attempt_count == 0));

    let process_preview_hash = process_dry_run[0].review_preview_hash.clone();
    let failed_webhooks = state
        .process_fleet_alert_notifications(
            &FleetAlertNotificationProcessRequest {
                limit: Some(20),
                status: Some("queued".to_string()),
                delivery_kind: Some("webhook".to_string()),
                dry_run: Some(false),
                preview_hash: process_preview_hash,
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(failed_webhooks.len(), 2);
    assert!(failed_webhooks.iter().all(|delivery| {
        delivery.status == "failed"
            && delivery.attempt_count == 1
            && delivery.next_attempt_at.is_some()
            && delivery
                .error
                .as_deref()
                .is_some_and(|error| error.contains("webhook target address is not public"))
    }));
    if let Repository::Memory(memory) = &state.repo {
        let audits = memory.audits.read().await;
        assert!(audits
            .iter()
            .any(|audit| audit.action == "fleet.alert_notification_deliveries_processed"));
    }
}

#[tokio::test]
async fn fleet_alert_notification_dispatch_review_tolerates_observation_timestamp_refresh() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.extend([
            AgentView {
                id: "edge-a".to_string(),
                display_name: "Edge A".to_string(),
                status: "online".to_string(),
                tags: vec!["edge".to_string()],
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                arch: None,
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
                tags: vec!["edge".to_string()],
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                arch: None,
                internal_build_number: 1,
                process_incarnation_id: None,
                stale_since: None,
                stale_reason: None,
                capabilities: AgentCapabilitySnapshot::default(),
            },
        ]);
        let mut edge_b_rollup = alert_test_rollup("edge-b", 2.7, 300, 800);
        edge_b_rollup.latest_observed_at = "150".to_string();
        edge_b_rollup.updated_at = "151".to_string();
        memory
            .telemetry_rollups
            .write()
            .await
            .extend([alert_test_rollup("edge-a", 2.6, 300, 800), edge_b_rollup]);
    }
    repo.upsert_fleet_alert_notification_channel(
        &CreateFleetAlertNotificationChannelRequest {
            id: None,
            name: "resource-webhook".to_string(),
            scope_kind: "global".to_string(),
            scope_value: None,
            min_severity: Some("warning".to_string()),
            categories: Some(vec!["resource".to_string()]),
            operator_states: Some(vec!["open".to_string()]),
            delivery_kind: "webhook".to_string(),
            target: "https://hooks.acme.com/resource-webhook".to_string(),
            cooldown_secs: Some(60),
            enabled: Some(true),
            notes: None,
            confirmed: true,
        },
        &operator,
    )
    .await
    .unwrap();

    let state = alert_test_state(repo.clone());
    let dry_run = state
        .dispatch_fleet_alert_notifications(
            &FleetAlertNotificationDispatchRequest {
                limit: Some(20),
                client_id: None,
                severity: None,
                category: Some("resource".to_string()),
                operator_state: Some("open".to_string()),
                include_muted: None,
                dry_run: Some(true),
                preview_hash: None,
                confirmed: false,
            },
            &operator,
        )
        .await
        .unwrap();
    assert!(!dry_run.is_empty());
    let preview_hash = dry_run[0].review_preview_hash.clone();
    if let Repository::Memory(memory) = &repo {
        let mut rollups = memory.telemetry_rollups.write().await;
        rollups[0].cpu_load_1_avg = 9.0;
        rollups[0].cpu_load_1_max = 9.0;
        rollups[0].latest_observed_at = "180".to_string();
        rollups[0].updated_at = "181".to_string();
    }

    let dispatched = state
        .dispatch_fleet_alert_notifications(
            &FleetAlertNotificationDispatchRequest {
                limit: Some(20),
                client_id: None,
                severity: None,
                category: Some("resource".to_string()),
                operator_state: Some("open".to_string()),
                include_muted: None,
                dry_run: Some(false),
                preview_hash,
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert!(!dispatched.is_empty());
    assert!(dispatched.iter().all(|row| row.status == "queued"));
}

#[tokio::test]
async fn fleet_alert_notification_dispatch_review_rejects_target_set_change() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.push(AgentView {
            id: "edge-a".to_string(),
            display_name: "Edge A".to_string(),
            status: "online".to_string(),
            tags: vec!["edge".to_string()],
            registration_ip: None,
            last_ip: None,
            last_seen_at: None,
            arch: None,
            internal_build_number: 1,
            process_incarnation_id: None,
            stale_since: None,
            stale_reason: None,
            capabilities: AgentCapabilitySnapshot::default(),
        });
        memory
            .telemetry_rollups
            .write()
            .await
            .push(alert_test_rollup("edge-a", 2.6, 300, 800));
    }
    repo.upsert_fleet_alert_notification_channel(
        &CreateFleetAlertNotificationChannelRequest {
            id: None,
            name: "resource-webhook".to_string(),
            scope_kind: "global".to_string(),
            scope_value: None,
            min_severity: Some("warning".to_string()),
            categories: Some(vec!["resource".to_string()]),
            operator_states: Some(vec!["open".to_string()]),
            delivery_kind: "webhook".to_string(),
            target: "https://hooks.acme.com/resource-webhook".to_string(),
            cooldown_secs: Some(60),
            enabled: Some(true),
            notes: None,
            confirmed: true,
        },
        &operator,
    )
    .await
    .unwrap();

    let state = alert_test_state(repo.clone());
    let dry_run = state
        .dispatch_fleet_alert_notifications(
            &FleetAlertNotificationDispatchRequest {
                limit: Some(20),
                client_id: None,
                severity: None,
                category: Some("resource".to_string()),
                operator_state: Some("open".to_string()),
                include_muted: None,
                dry_run: Some(true),
                preview_hash: None,
                confirmed: false,
            },
            &operator,
        )
        .await
        .unwrap();
    let preview_hash = dry_run[0].review_preview_hash.clone();
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.push(AgentView {
            id: "edge-b".to_string(),
            display_name: "Edge B".to_string(),
            status: "online".to_string(),
            tags: vec!["edge".to_string()],
            registration_ip: None,
            last_ip: None,
            last_seen_at: None,
            arch: None,
            internal_build_number: 1,
            process_incarnation_id: None,
            stale_since: None,
            stale_reason: None,
            capabilities: AgentCapabilitySnapshot::default(),
        });
        memory
            .telemetry_rollups
            .write()
            .await
            .push(alert_test_rollup("edge-b", 2.7, 300, 800));
    }

    let dispatch_result = state
        .dispatch_fleet_alert_notifications(
            &FleetAlertNotificationDispatchRequest {
                limit: Some(20),
                client_id: None,
                severity: None,
                category: Some("resource".to_string()),
                operator_state: Some("open".to_string()),
                include_muted: None,
                dry_run: Some(false),
                preview_hash,
                confirmed: true,
            },
            &operator,
        )
        .await;
    assert!(dispatch_result.is_err());
    assert!(dispatch_result
        .unwrap_err()
        .to_string()
        .contains("fleet_alert_notification_dispatch_preview_hash_mismatch"));
}

#[tokio::test]
async fn disabled_alert_notification_channel_cancels_retryable_deliveries() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let channel_id = Uuid::new_v4();
    repo.upsert_fleet_alert_notification_channel(
        &CreateFleetAlertNotificationChannelRequest {
            id: Some(channel_id),
            name: "edge-webhook".to_string(),
            scope_kind: "tag".to_string(),
            scope_value: Some("edge".to_string()),
            min_severity: Some("warning".to_string()),
            categories: Some(vec!["agent_status".to_string()]),
            operator_states: Some(vec!["open".to_string()]),
            delivery_kind: "webhook".to_string(),
            target: "https://hooks.acme.com/fleet".to_string(),
            cooldown_secs: Some(900),
            enabled: Some(true),
            notes: None,
            confirmed: true,
        },
        &operator,
    )
    .await
    .unwrap();
    let deliveries = repo
        .record_fleet_alert_notification_deliveries(
            &[FleetAlertNotificationCandidate {
                channel_id,
                channel_name: "edge-webhook".to_string(),
                alert_id: "agent_status:agent:edge-a".to_string(),
                alert_severity: "critical".to_string(),
                alert_category: "agent_status".to_string(),
                status: "queued".to_string(),
                delivery_kind: "webhook".to_string(),
                target: "https://hooks.acme.com/fleet".to_string(),
                dedupe_key: "fleet-alert-notification:test".to_string(),
                payload: json!({"schema": "test"}),
                cooldown_until_unix: 0,
            }],
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(deliveries.len(), 1);

    repo.upsert_fleet_alert_notification_channel(
        &CreateFleetAlertNotificationChannelRequest {
            id: Some(channel_id),
            name: "edge-webhook".to_string(),
            scope_kind: "tag".to_string(),
            scope_value: Some("edge".to_string()),
            min_severity: Some("warning".to_string()),
            categories: Some(vec!["agent_status".to_string()]),
            operator_states: Some(vec!["open".to_string()]),
            delivery_kind: "webhook".to_string(),
            target: "https://hooks.acme.com/fleet".to_string(),
            cooldown_secs: Some(900),
            enabled: Some(false),
            notes: None,
            confirmed: true,
        },
        &operator,
    )
    .await
    .unwrap();

    let canceled = repo
        .list_fleet_alert_notification_deliveries(20, None, None, Some("canceled_disabled"))
        .await
        .unwrap();
    assert_eq!(canceled.len(), 1);
    assert_eq!(canceled[0].id, deliveries[0].id);
    assert!(canceled[0]
        .error
        .as_deref()
        .is_some_and(|error| error.contains("disabled")));
    let claimed = repo
        .claim_fleet_alert_notification_deliveries_for_process(
            &[deliveries[0].id],
            Uuid::new_v4(),
            60,
        )
        .await
        .unwrap();
    assert!(claimed.is_empty());
}

#[tokio::test]
async fn deleted_alert_notification_channel_preserves_and_cancels_delivery_history() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let channel_id = Uuid::new_v4();
    repo.upsert_fleet_alert_notification_channel(
        &CreateFleetAlertNotificationChannelRequest {
            id: Some(channel_id),
            name: "deleted-edge-webhook".to_string(),
            scope_kind: "global".to_string(),
            scope_value: None,
            min_severity: Some("warning".to_string()),
            categories: Some(vec!["agent_status".to_string()]),
            operator_states: Some(vec!["open".to_string()]),
            delivery_kind: "webhook".to_string(),
            target: "https://hooks.acme.com/fleet".to_string(),
            cooldown_secs: Some(900),
            enabled: Some(true),
            notes: None,
            confirmed: true,
        },
        &operator,
    )
    .await
    .unwrap();
    let created = repo
        .record_fleet_alert_notification_deliveries(
            &[FleetAlertNotificationCandidate {
                channel_id,
                channel_name: "deleted-edge-webhook".to_string(),
                alert_id: "agent_status:agent:edge-a".to_string(),
                alert_severity: "critical".to_string(),
                alert_category: "agent_status".to_string(),
                status: "queued".to_string(),
                delivery_kind: "webhook".to_string(),
                target: "https://hooks.acme.com/fleet".to_string(),
                dedupe_key: "fleet-alert-notification:deleted-test".to_string(),
                payload: json!({"schema": "test"}),
                cooldown_until_unix: 0,
            }],
            &operator,
        )
        .await
        .unwrap();

    repo.delete_fleet_alert_notification_channel(channel_id, &operator)
        .await
        .unwrap();

    let stale_dispatch = repo
        .record_fleet_alert_notification_deliveries(
            &[FleetAlertNotificationCandidate {
                channel_id,
                channel_name: "deleted-edge-webhook".to_string(),
                alert_id: "agent_status:agent:edge-b".to_string(),
                alert_severity: "critical".to_string(),
                alert_category: "agent_status".to_string(),
                status: "queued".to_string(),
                delivery_kind: "webhook".to_string(),
                target: "https://hooks.acme.com/fleet".to_string(),
                dedupe_key: "fleet-alert-notification:stale-deleted-test".to_string(),
                payload: json!({"schema": "test"}),
                cooldown_until_unix: 0,
            }],
            &operator,
        )
        .await
        .unwrap();
    assert!(stale_dispatch.is_empty());

    assert!(repo
        .list_fleet_alert_notification_channels(20, None, None, None, None)
        .await
        .unwrap()
        .is_empty());
    let retained = repo
        .list_fleet_alert_notification_deliveries(20, Some(channel_id), None, None)
        .await
        .unwrap();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].id, created[0].id);
    assert_eq!(retained[0].status, "canceled_disabled");
    assert_eq!(
        retained[0].error.as_deref(),
        Some("fleet alert notification channel deleted")
    );
    assert!(
        repo.claim_fleet_alert_notification_deliveries_for_process(
            &[created[0].id],
            Uuid::new_v4(),
            60,
        )
        .await
        .unwrap()
        .is_empty()
    );
}

#[tokio::test]
async fn disabled_webhook_rule_cancels_retryable_deliveries() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let rule_id = Uuid::new_v4();
    repo.upsert_webhook_rule(
        &crate::model_webhook_rules::CreateWebhookRuleRequest {
            id: Some(rule_id),
            name: "edge-rule".to_string(),
            enabled: true,
            expression: "status = stale".to_string(),
            target: "https://hooks.acme.com/webhook".to_string(),
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
    let deliveries = repo
        .record_webhook_rule_deliveries(&[
            crate::model_webhook_rules::WebhookRuleDeliveryCandidate {
                rule_id,
                rule_name: "edge-rule".to_string(),
                event_kind: "manual.test".to_string(),
                event_id: "event-1".to_string(),
                target: "https://hooks.acme.com/webhook".to_string(),
                dedupe_key: "webhook-rule:test".to_string(),
                payload: json!({"schema": "test"}),
                matched_vps: Vec::new(),
                message: "test".to_string(),
                rule_revision_hash: "test-revision".to_string(),
                signing_secret: None,
                cooldown_until_unix: 0,
                actor_id: Some(operator.operator.id),
            },
        ])
        .await
        .unwrap();
    assert_eq!(deliveries.len(), 1);

    repo.upsert_webhook_rule(
        &crate::model_webhook_rules::CreateWebhookRuleRequest {
            id: Some(rule_id),
            name: "edge-rule".to_string(),
            enabled: false,
            expression: "status = stale".to_string(),
            target: "https://hooks.acme.com/webhook".to_string(),
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

    let canceled = repo
        .list_webhook_rule_deliveries(20, Some(rule_id), None, Some("canceled_disabled"))
        .await
        .unwrap();
    assert_eq!(canceled.len(), 1);
    assert_eq!(canceled[0].id, deliveries[0].id);
    assert!(canceled[0]
        .error
        .as_deref()
        .is_some_and(|error| error.contains("disabled")));
    let claimed = repo
        .claim_webhook_rule_deliveries_for_process(&[deliveries[0].id], Uuid::new_v4(), 60)
        .await
        .unwrap();
    assert!(claimed.is_empty());
}

#[tokio::test]
async fn deleted_webhook_rule_preserves_and_cancels_delivery_history() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let rule_id = Uuid::new_v4();
    repo.upsert_webhook_rule(
        &crate::model_webhook_rules::CreateWebhookRuleRequest {
            id: Some(rule_id),
            name: "deleted-edge-rule".to_string(),
            enabled: true,
            expression: "status = stale".to_string(),
            target: "https://hooks.acme.com/webhook".to_string(),
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
    let created = repo
        .record_webhook_rule_deliveries(&[
            crate::model_webhook_rules::WebhookRuleDeliveryCandidate {
                rule_id,
                rule_name: "deleted-edge-rule".to_string(),
                event_kind: "manual.test".to_string(),
                event_id: "deleted-event-1".to_string(),
                target: "https://hooks.acme.com/webhook".to_string(),
                dedupe_key: "webhook-rule:deleted-test".to_string(),
                payload: json!({"schema": "test"}),
                matched_vps: Vec::new(),
                message: "test".to_string(),
                rule_revision_hash: "deleted-test-revision".to_string(),
                signing_secret: None,
                cooldown_until_unix: 0,
                actor_id: Some(operator.operator.id),
            },
        ])
        .await
        .unwrap();

    repo.delete_webhook_rule(rule_id, &operator).await.unwrap();

    let stale_dispatch = repo
        .record_webhook_rule_deliveries(&[
            crate::model_webhook_rules::WebhookRuleDeliveryCandidate {
                rule_id,
                rule_name: "deleted-edge-rule".to_string(),
                event_kind: "manual.test".to_string(),
                event_id: "deleted-event-2".to_string(),
                target: "https://hooks.acme.com/webhook".to_string(),
                dedupe_key: "webhook-rule:stale-deleted-test".to_string(),
                payload: json!({"schema": "test"}),
                matched_vps: Vec::new(),
                message: "test".to_string(),
                rule_revision_hash: "deleted-test-revision".to_string(),
                signing_secret: None,
                cooldown_until_unix: 0,
                actor_id: Some(operator.operator.id),
            },
        ])
        .await
        .unwrap();
    assert!(stale_dispatch.is_empty());

    assert!(repo.list_webhook_rules(20, None).await.unwrap().is_empty());
    let retained = repo
        .list_webhook_rule_deliveries(20, Some(rule_id), None, None)
        .await
        .unwrap();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].id, created[0].id);
    assert_eq!(retained[0].status, "canceled_disabled");
    assert_eq!(retained[0].error.as_deref(), Some("webhook rule deleted"));
    assert!(repo
        .claim_webhook_rule_deliveries_for_process(&[created[0].id], Uuid::new_v4(), 60)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn webhook_rule_signing_secret_is_redacted_preserved_rotated_and_cleared() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let rule_id = Uuid::new_v4();
    let base_request =
        |signing_secret: Option<&str>, clear_signing_secret: bool, target_suffix: &str| {
            crate::model_webhook_rules::CreateWebhookRuleRequest {
                id: Some(rule_id),
                name: "signed-edge-rule".to_string(),
                enabled: true,
                expression: "interval.30sec && tag:edge".to_string(),
                target: format!("https://hooks.acme.com/{target_suffix}"),
                body_template: "{rule.name} {event.kind}".to_string(),
                signing_secret: signing_secret.map(ToOwned::to_owned),
                clear_signing_secret,
                cooldown_secs: Some(60),
                notes: None,
                confirmed: true,
            }
        };

    let created = repo
        .upsert_webhook_rule(
            &base_request(Some("alpha-secret"), false, "create"),
            &operator,
        )
        .await
        .unwrap();
    assert!(created.signing_secret_set);
    assert_eq!(created.signing_secret.as_deref(), Some("alpha-secret"));
    let serialized = serde_json::to_value(&created).unwrap();
    assert_eq!(serialized["signing_secret_set"], true);
    assert!(serialized.get("signing_secret").is_none());

    let preserved = repo
        .upsert_webhook_rule(&base_request(None, false, "preserve"), &operator)
        .await
        .unwrap();
    assert!(preserved.signing_secret_set);
    assert_eq!(preserved.signing_secret.as_deref(), Some("alpha-secret"));
    assert_eq!(preserved.target, "https://hooks.acme.com/preserve");

    let rotated = repo
        .upsert_webhook_rule(
            &base_request(Some("beta-secret"), false, "rotate"),
            &operator,
        )
        .await
        .unwrap();
    assert!(rotated.signing_secret_set);
    assert_eq!(rotated.signing_secret.as_deref(), Some("beta-secret"));

    let cleared = repo
        .upsert_webhook_rule(&base_request(None, true, "clear"), &operator)
        .await
        .unwrap();
    assert!(!cleared.signing_secret_set);
    assert_eq!(cleared.signing_secret, None);
}

#[tokio::test]
async fn webhook_rule_dispatch_can_be_scoped_to_one_rule() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.push(AgentView {
            id: "edge-a".to_string(),
            display_name: "Edge A".to_string(),
            status: "online".to_string(),
            tags: vec!["edge".to_string()],
            registration_ip: None,
            last_ip: None,
            last_seen_at: None,
            arch: None,
            internal_build_number: 1,
            process_incarnation_id: None,
            stale_since: None,
            stale_reason: None,
            capabilities: AgentCapabilitySnapshot::default(),
        });
    }
    let first_rule_id = Uuid::new_v4();
    let second_enabled_rule_id = Uuid::new_v4();
    let scoped_rule_id = Uuid::new_v4();
    for (id, name) in [
        (first_rule_id, "alpha-webhook"),
        (second_enabled_rule_id, "middle-webhook"),
        (scoped_rule_id, "zulu-webhook"),
    ] {
        repo.upsert_webhook_rule(
            &crate::model_webhook_rules::CreateWebhookRuleRequest {
                id: Some(id),
                name: name.to_string(),
                enabled: id != scoped_rule_id,
                expression: "interval.30sec && tag:edge".to_string(),
                target: format!("https://hooks.acme.com/{name}"),
                body_template: "{rule.name} {event.kind}".to_string(),
                signing_secret: (id == scoped_rule_id).then(|| "scoped-secret".to_string()),
                clear_signing_secret: false,
                cooldown_secs: Some(60),
                notes: None,
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    }

    let state = alert_test_state(repo);
    let broad_preview = state
        .dispatch_webhook_rules(
            &crate::model_webhook_rules::WebhookRuleDispatchRequest {
                rule_id: None,
                event_kind: "interval.30sec".to_string(),
                event_id: Some("event-1".to_string()),
                limit: Some(1),
                dry_run: Some(true),
                preview_hash: None,
                confirmed: false,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(broad_preview.len(), 1);
    assert_eq!(broad_preview[0].rule_id, first_rule_id);

    let broad_dispatch = state
        .dispatch_webhook_rules(
            &crate::model_webhook_rules::WebhookRuleDispatchRequest {
                rule_id: None,
                event_kind: "interval.30sec".to_string(),
                event_id: Some("event-1".to_string()),
                limit: Some(1),
                dry_run: Some(false),
                preview_hash: broad_preview[0].review_preview_hash.clone(),
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(broad_dispatch.len(), 1);
    assert_eq!(broad_dispatch[0].rule_id, first_rule_id);
    assert_eq!(broad_dispatch[0].status, "queued");
    if let Repository::Memory(memory) = &state.repo {
        assert!(
            memory.webhook_events.read().await.is_empty(),
            "manual dispatch must not defer broad rule re-evaluation to the worker"
        );
    }

    let scoped_preview = state
        .dispatch_webhook_rules(
            &crate::model_webhook_rules::WebhookRuleDispatchRequest {
                rule_id: Some(scoped_rule_id),
                event_kind: "interval.30sec".to_string(),
                event_id: Some("event-2".to_string()),
                limit: Some(1),
                dry_run: Some(true),
                preview_hash: None,
                confirmed: false,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(scoped_preview.len(), 1);
    assert_eq!(scoped_preview[0].rule_id, scoped_rule_id);
    assert_eq!(scoped_preview[0].status, "matched_dry_run");
    assert_eq!(
        scoped_preview[0].signing_secret.as_deref(),
        Some("scoped-secret")
    );

    let scoped_review_hash = scoped_preview[0].review_preview_hash.clone();
    let scoped_dispatch = state
        .dispatch_webhook_rules(
            &crate::model_webhook_rules::WebhookRuleDispatchRequest {
                rule_id: Some(scoped_rule_id),
                event_kind: "interval.30sec".to_string(),
                event_id: Some("event-2".to_string()),
                limit: Some(1),
                dry_run: Some(false),
                preview_hash: scoped_review_hash,
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(scoped_dispatch.len(), 1);
    assert_eq!(scoped_dispatch[0].rule_id, scoped_rule_id);
    assert_eq!(scoped_dispatch[0].status, "queued");
}

#[tokio::test]
async fn webhook_rule_dispatch_generated_event_id_can_be_confirmed_when_reused() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.push(AgentView {
            id: "edge-a".to_string(),
            display_name: "Edge A".to_string(),
            status: "online".to_string(),
            tags: vec!["edge".to_string()],
            registration_ip: None,
            last_ip: None,
            last_seen_at: None,
            arch: None,
            internal_build_number: 1,
            process_incarnation_id: None,
            stale_since: None,
            stale_reason: None,
            capabilities: AgentCapabilitySnapshot::default(),
        });
    }
    repo.upsert_webhook_rule(
        &crate::model_webhook_rules::CreateWebhookRuleRequest {
            id: None,
            name: "generated-event-webhook".to_string(),
            enabled: true,
            expression: "interval.30sec && tag:edge".to_string(),
            target: "https://hooks.acme.com/generated-event-webhook".to_string(),
            body_template: "{rule.name} {event.id}".to_string(),
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

    let state = alert_test_state(repo);
    let preview = state
        .dispatch_webhook_rules(
            &crate::model_webhook_rules::WebhookRuleDispatchRequest {
                rule_id: None,
                event_kind: "interval.30sec".to_string(),
                event_id: None,
                limit: Some(50),
                dry_run: Some(true),
                preview_hash: None,
                confirmed: false,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(preview.len(), 1);
    let reviewed_event_id = preview[0].event_id.clone();
    assert!(!reviewed_event_id.is_empty());
    let reviewed_hash = preview[0].review_preview_hash.clone();

    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;

    let missing_event_id_error = state
        .dispatch_webhook_rules(
            &crate::model_webhook_rules::WebhookRuleDispatchRequest {
                rule_id: None,
                event_kind: "interval.30sec".to_string(),
                event_id: None,
                limit: Some(50),
                dry_run: Some(false),
                preview_hash: reviewed_hash.clone(),
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(missing_event_id_error.contains("webhook_rule_dispatch_event_id_required"));

    let dispatch = state
        .dispatch_webhook_rules(
            &crate::model_webhook_rules::WebhookRuleDispatchRequest {
                rule_id: None,
                event_kind: "interval.30sec".to_string(),
                event_id: Some(reviewed_event_id.clone()),
                limit: Some(50),
                dry_run: Some(false),
                preview_hash: reviewed_hash,
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(dispatch.len(), 1);
    assert_eq!(dispatch[0].event_id, reviewed_event_id);
    assert_eq!(dispatch[0].status, "queued");
    if let Repository::Memory(memory) = &state.repo {
        assert!(memory.webhook_events.read().await.is_empty());
    }
}

fn alert_test_state(repo: Repository) -> AppState {
    AppState {
        repo,
        events: tokio::sync::broadcast::channel(4).0,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        backup_object_store: None,
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
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
        },
        session_id: Uuid::new_v4(),
    }
}

fn test_operator_record(auth: &AuthContext) -> crate::auth_model::OperatorRecord {
    crate::auth_model::OperatorRecord {
        id: auth.operator.id,
        username: auth.operator.username.clone(),
        password_hash: "test-only-session-issued-directly".to_string(),
        status: auth.operator.status.clone(),
        role: auth.operator.role.clone(),
        scopes: auth.operator.scopes.clone(),
        preferences: auth.operator.preferences.clone(),
        totp_enabled: auth.operator.totp_enabled,
        totp_secret_ciphertext_hex: None,
        totp_secret_nonce_hex: None,
        totp_secret_salt_hex: None,
        session_refresh_ttl_secs: auth.operator.session_refresh_ttl_secs,
        created_at: auth.operator.created_at.clone(),
        disabled_at: auth.operator.disabled_at.clone(),
        deleted_at: auth.operator.deleted_at.clone(),
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
