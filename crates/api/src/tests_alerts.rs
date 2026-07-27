use super::*;
use base64::Engine as _;
use serde_json::json;
use vpsman_common::{AgentCapabilitySnapshot, AgentPrivilegeMode};

#[tokio::test]
async fn fleet_alerts_derive_actionable_current_status() {
    let repo = Repository::Memory(MemoryState::default());
    let tunnel_input = alert_test_tunnel_input();
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
            status: "skipped".to_string(),
            message: Some("target lacks the required capability".to_string()),
            exit_code: Some(0),
            started_at: Some("105".to_string()),
            deadline_at: None,
            completed_at: Some("110".to_string()),
            process_incarnation_id: None,
        });
        let status = json!({
            "type": "capability_degraded",
            "status": "skipped",
            "client_id": "edge-b",
            "command_type": "backup",
            "reason": "target_agent_lacks_root_runtime_network_capability",
            "hint": "Run this agent with root privileges before retrying the backup.",
        });
        memory.job_outputs.write().await.push(JobOutputView {
            job_id: backup_job,
            client_id: "edge-b".to_string(),
            seq: 0,
            stream: "status".to_string(),
            data_base64: base64::engine::general_purpose::STANDARD
                .encode(serde_json::to_vec(&status).unwrap()),
            storage: "inline".to_string(),
            artifact_object_key: None,
            artifact_sha256_hex: None,
            artifact_size_bytes: None,
            exit_code: Some(0),
            done: true,
            received_at: None,
            created_at: "110".to_string(),
        });
        memory.capability_degraded_job_targets.write().await.insert(
            (backup_job, "edge-b".to_string()),
            (
                "target_agent_lacks_root_runtime_network_capability".to_string(),
                "Run this agent with root privileges before retrying the backup.".to_string(),
            ),
        );
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
    assert_alert_category(&alerts, "capability_degraded");
    assert_alert_category(&alerts, "source_readiness");
    let capability_alert = alerts
        .iter()
        .find(|alert| alert.category == "capability_degraded")
        .unwrap();
    assert_eq!(
        capability_alert.status,
        "target_agent_lacks_root_runtime_network_capability"
    );
    assert_eq!(
        capability_alert.detail,
        "Run this agent with root privileges before retrying the backup."
    );
    assert_eq!(capability_alert.evidence["target_status"], "skipped");
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
    assert_alert_category(&edge_b, "capability_degraded");
}

#[tokio::test]
async fn tunnel_adapter_failures_only_degrade_external_managed_plans() {
    for (manager, manager_label, health_status, expected_degraded) in [
        (
            vpsman_common::RuntimeTunnelManager::AgentIproute2Managed,
            "agent_iproute2_managed",
            "skipped",
            false,
        ),
        (
            vpsman_common::RuntimeTunnelManager::ExternalObserved,
            "external_observed",
            "skipped",
            false,
        ),
        (
            vpsman_common::RuntimeTunnelManager::ExternalManagedAdapter,
            "external_managed_adapter",
            "failed",
            true,
        ),
    ] {
        let repo = Repository::Memory(MemoryState::default());
        let input = crate::tests_network::test_plan_input(manager, false);
        let plan = vpsman_common::plan_tunnel(&input).unwrap();
        let saved = repo
            .record_tunnel_plan(&input, &plan, true, &test_operator())
            .await
            .unwrap();
        if let Repository::Memory(memory) = &repo {
            memory.agents.write().await.push(AgentView {
                id: "client-a".to_string(),
                display_name: "Client A".to_string(),
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
            });
            memory
                .telemetry_tunnels
                .write()
                .await
                .push(TelemetryTunnelView {
                    client_id: "client-a".to_string(),
                    observed_at: "200".to_string(),
                    interface: input.interface_name.clone(),
                    kind: "wireguard".to_string(),
                    ownership_mode: manager_label.to_string(),
                    mutation_policy: "managed_desired".to_string(),
                    plan_id: Some(saved.id),
                    plan_name: Some(saved.name.clone()),
                    plan_runtime_manager: Some(manager_label.to_string()),
                    endpoint_side: Some("left".to_string()),
                    peer_client_id: Some("client-b".to_string()),
                    source: "telemetry".to_string(),
                    operstate: Some("up".to_string()),
                    mtu: Some(1476),
                    link_type: None,
                    address: Some("10.10.0.0".to_string()),
                    rx_bytes: 100,
                    tx_bytes: 200,
                    traffic_source: Some("interface_counters".to_string()),
                    traffic_status: Some("ok".to_string()),
                    traffic_reason: None,
                    traffic_checked_unix: Some(200),
                    adapter_health: Some(TelemetryTunnelAdapterHealthView {
                        status: health_status.to_string(),
                        checked_unix: 200,
                        configured: false,
                        success: false,
                        exit_code: None,
                        reason: Some("adapter command was not applicable or failed".to_string()),
                        duration_ms: 0,
                        command_sha256_hex: None,
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
            let template_id = Uuid::new_v4();
            memory
                .source_templates
                .write()
                .await
                .push(SourceTemplateView {
                    id: template_id,
                    domain: "runtime_tunnel_adapter".to_string(),
                    name: format!("shared:{manager_label}"),
                    scope: "shared".to_string(),
                    built_in: false,
                    is_default: false,
                    owner_client_id: None,
                    description: None,
                    definition: json!({"manager": manager_label}),
                    assigned_client_count: 1,
                    created_at: "100".to_string(),
                    updated_at: "100".to_string(),
                });
            memory
                .source_template_assignments
                .write()
                .await
                .push(SourceTemplateAssignmentView {
                    client_id: "client-a".to_string(),
                    domain: "runtime_tunnel_adapter".to_string(),
                    template_id,
                    template_name: format!("shared:{manager_label}"),
                    template_scope: "shared".to_string(),
                    assigned_at: "100".to_string(),
                });
        }

        let source_status = repo
            .list_source_status(Some("client-a"), Some("runtime_tunnel_adapter"))
            .await
            .unwrap();
        assert_eq!(source_status.len(), 1, "{manager_label}");
        assert_eq!(
            source_status[0].status == "degraded",
            expected_degraded,
            "{manager_label}"
        );

        let alerts = alert_test_state(repo)
            .list_fleet_alerts(FleetAlertQuery {
                limit: Some(10),
                client_id: Some("client-a".to_string()),
                severity: Some("critical".to_string()),
                category: Some("network".to_string()),
                operator_state: None,
                include_muted: None,
            })
            .await
            .unwrap();
        assert_eq!(!alerts.is_empty(), expected_degraded, "{manager_label}");
    }
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
async fn fleet_alert_policy_regression_upsert_by_name_preserves_identity_past_list_limit() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let request = CreateFleetAlertPolicyRequest {
        id: None,
        name: "repeatable-cli-policy".to_string(),
        enabled: true,
        selector_expression: "*".to_string(),
        rules: vec![PolicyRuleRequest {
            id: None,
            name: "rule-1".to_string(),
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

    let first = repo
        .upsert_fleet_alert_policy(&request, &operator)
        .await
        .unwrap();
    assert_eq!(
        repo.list_fleet_alert_policies(20, None, None, None)
            .await
            .unwrap()
            .len(),
        1
    );
    if let Repository::Memory(memory) = &repo {
        let mut groups = memory.policy_groups.write().await;
        for index in 0..1000 {
            let mut filler = first.clone();
            filler.id = Uuid::new_v4();
            filler.name = format!("aaa-filler-{index:04}");
            for rule in &mut filler.rules {
                rule.id = Uuid::new_v4();
                rule.group_id = filler.id;
            }
            groups.push(filler);
        }
    }
    let second = repo
        .upsert_fleet_alert_policy(&request, &operator)
        .await
        .unwrap();

    assert_eq!(second.id, first.id);
    assert_eq!(second.rules[0].id, first.rules[0].id);
    assert_eq!(second.rules[0].rule_version, first.rules[0].rule_version);
    if let Repository::Memory(memory) = &repo {
        assert_eq!(
            memory
                .policy_groups
                .read()
                .await
                .iter()
                .filter(|group| group.name == request.name)
                .count(),
            1
        );
    }

    let conflicting = CreateFleetAlertPolicyRequest {
        id: Some(Uuid::new_v4()),
        name: request.name.clone(),
        enabled: request.enabled,
        selector_expression: request.selector_expression.clone(),
        rules: vec![PolicyRuleRequest {
            id: None,
            name: "rule-1".to_string(),
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
    assert!(repo
        .upsert_fleet_alert_policy(&conflicting, &operator)
        .await
        .unwrap_err()
        .to_string()
        .contains("fleet_alert_policy_name_conflict"));
}

#[tokio::test]
async fn fleet_alert_policy_delete_review_regression_checks_name_inside_delete_lock() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let rule_id = Uuid::new_v4();
    let request = |id, name: &str| CreateFleetAlertPolicyRequest {
        id,
        name: name.to_string(),
        enabled: true,
        selector_expression: "*".to_string(),
        rules: vec![PolicyRuleRequest {
            id: Some(rule_id),
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
    let created = repo
        .upsert_fleet_alert_policy(&request(None, "reviewed-before"), &operator)
        .await
        .unwrap();
    let renamed = repo
        .upsert_fleet_alert_policy(&request(Some(created.id), "reviewed-after"), &operator)
        .await
        .unwrap();

    let stale = repo
        .delete_fleet_alert_policy(renamed.id, "reviewed-before", &operator)
        .await
        .unwrap_err();
    assert!(stale
        .to_string()
        .contains("fleet_alert_policy_delete_review_stale"));
    assert_eq!(
        repo.get_fleet_alert_policy(renamed.id).await.unwrap().name,
        "reviewed-after"
    );
    if let Repository::Memory(memory) = &repo {
        assert!(!memory
            .audits
            .read()
            .await
            .iter()
            .any(|audit| audit.action == "fleet.alert_policy_deleted"));
    }

    repo.delete_fleet_alert_policy(renamed.id, "reviewed-after", &operator)
        .await
        .unwrap();
    assert!(repo
        .get_fleet_alert_policy(renamed.id)
        .await
        .unwrap_err()
        .to_string()
        .contains("fleet_alert_policy_not_found"));
    assert!(repo
        .delete_fleet_alert_policy(renamed.id, "reviewed-after", &operator)
        .await
        .unwrap_err()
        .to_string()
        .contains("fleet_alert_policy_not_found"));
    if let Repository::Memory(memory) = &repo {
        assert_eq!(
            memory
                .audits
                .read()
                .await
                .iter()
                .filter(|audit| audit.action == "fleet.alert_policy_deleted")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn fleet_alert_policy_regression_reordering_keeps_rule_state_and_alert_identity() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.push(AgentView {
            id: "reorder-edge".to_string(),
            display_name: "Reorder Edge".to_string(),
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
        });
        memory
            .telemetry_rollups
            .write()
            .await
            .push(alert_test_rollup("reorder-edge", 2.0, 500, 1500));
    }
    let first_rule_id = Uuid::new_v4();
    let retained_rule_id = Uuid::new_v4();
    let initial = repo
        .upsert_fleet_alert_policy(
            &CreateFleetAlertPolicyRequest {
                id: None,
                name: "reorder-policy".to_string(),
                enabled: true,
                selector_expression: "id:reorder-edge".to_string(),
                rules: vec![
                    PolicyRuleRequest {
                        id: Some(first_rule_id),
                        name: "first".to_string(),
                        enabled: true,
                        traffic_selector: None,
                        condition_expression: "cpu.load_1 >= 1".to_string(),
                        window_secs: 0,
                        severity: "warning".to_string(),
                    },
                    PolicyRuleRequest {
                        id: Some(retained_rule_id),
                        name: "retained".to_string(),
                        enabled: true,
                        traffic_selector: None,
                        condition_expression: "cpu.load_1 >= 1".to_string(),
                        window_secs: 0,
                        severity: "critical".to_string(),
                    },
                ],
                notes: None,
                confirmed: true,
                preview_hash: None,
            },
            &operator,
        )
        .await
        .unwrap();
    let retained_version = initial
        .rules
        .iter()
        .find(|rule| rule.id == retained_rule_id)
        .unwrap()
        .rule_version;
    let (alerts_before, events_before) = if let Repository::Memory(memory) = &repo {
        (
            memory.policy_alerts.read().await.len(),
            memory.webhook_events.read().await.len(),
        )
    } else {
        unreachable!()
    };

    let reordered = repo
        .upsert_fleet_alert_policy(
            &CreateFleetAlertPolicyRequest {
                id: Some(initial.id),
                name: initial.name.clone(),
                enabled: true,
                selector_expression: initial.selector_expression.clone(),
                rules: vec![PolicyRuleRequest {
                    id: Some(retained_rule_id),
                    name: "retained".to_string(),
                    enabled: true,
                    traffic_selector: None,
                    condition_expression: "cpu.load_1 >= 1".to_string(),
                    window_secs: 0,
                    severity: "critical".to_string(),
                }],
                notes: None,
                confirmed: true,
                preview_hash: None,
            },
            &operator,
        )
        .await
        .unwrap();

    assert_eq!(reordered.rules[0].id, retained_rule_id);
    assert_eq!(reordered.rules[0].rule_version, retained_version);
    if let Repository::Memory(memory) = &repo {
        assert_eq!(memory.policy_alerts.read().await.len(), alerts_before);
        assert_eq!(memory.webhook_events.read().await.len(), events_before);
        assert!(memory.policy_rule_states.read().await.iter().any(|state| {
            state.policy_rule_id == retained_rule_id && state.rule_version == retained_version
        }));
    }
}

#[tokio::test]
async fn filter_limit_regression_internal_traffic_accounting_is_unbounded() {
    let memory = MemoryState::default();
    memory.agents.write().await.push(AgentView {
        id: "zzz-rule-target".to_string(),
        display_name: "Rule Target".to_string(),
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
    });
    let stored_rule =
        |client_id: String, key: &str, value_raw: &str, value_json: serde_json::Value| {
            crate::model_alert_policies::VpsRuleValueRecord {
                client_id,
                key: key.to_string(),
                value_raw: value_raw.to_string(),
                value_json,
                parsed_display: value_raw.to_string(),
                state: "ok".to_string(),
                validation_errors: Vec::new(),
                source_kind: "operator".to_string(),
                source_id: None,
                updated_by: None,
                updated_at: "1".to_string(),
            }
        };
    let mut rules = (0..5000)
        .map(|index| {
            stored_rule(
                format!("aaa-filler-{index:04}"),
                "traffic.reset_day",
                "1",
                json!({"day": 1}),
            )
        })
        .collect::<Vec<_>>();
    rules.extend([
        stored_rule(
            "zzz-rule-target".to_string(),
            "traffic.selectors",
            "eth0",
            json!({
                "selectors": [{
                    "source": "host",
                    "interface": "eth0",
                    "direction": "total",
                    "canonical": "eth0"
                }]
            }),
        ),
        stored_rule(
            "zzz-rule-target".to_string(),
            "traffic.reset_day",
            "7",
            json!({"day": 7}),
        ),
        stored_rule(
            "zzz-rule-target".to_string(),
            "traffic.quota.total",
            "1GB",
            json!({"bytes": 1_000_000_000_i64}),
        ),
    ]);
    *memory.vps_rule_values.write().await = rules;
    let repo = Repository::Memory(memory);

    let accounting = repo
        .get_traffic_accounting("zzz-rule-target")
        .await
        .unwrap();
    assert_eq!(accounting.reset_day, Some(7));
    assert_eq!(accounting.quota_total_bytes, Some(1_000_000_000));
    assert_eq!(accounting.selectors, vec!["eth0"]);

    let public_rows = repo
        .list_vps_rules(&VpsRuleQuery {
            limit: Some(2),
            client_id: Some("zzz-rule-target".to_string()),
            selector_expression: None,
            key: None,
            state: None,
        })
        .await
        .unwrap();
    assert_eq!(public_rows.len(), 2);
    assert_eq!(public_rows[0].key, "traffic.quota.total");
    assert_eq!(public_rows[1].key, "traffic.reset_day");
}

#[tokio::test]
async fn filter_limit_regression_policy_evaluator_is_unbounded() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let target = repo
        .upsert_fleet_alert_policy(
            &CreateFleetAlertPolicyRequest {
                id: None,
                name: "zzz-evaluated-policy".to_string(),
                enabled: true,
                selector_expression: "id:zzz-policy-target".to_string(),
                rules: vec![PolicyRuleRequest {
                    id: None,
                    name: "target threshold".to_string(),
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
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.push(AgentView {
            id: "zzz-policy-target".to_string(),
            display_name: "Policy Target".to_string(),
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
        });
        memory
            .telemetry_rollups
            .write()
            .await
            .push(alert_test_rollup("zzz-policy-target", 2.0, 500, 1500));
        let mut groups = (0..1000)
            .map(|index| {
                let mut filler = target.clone();
                filler.id = Uuid::new_v4();
                filler.name = format!("aaa-filler-policy-{index:04}");
                filler.selector_expression = "id:not-present".to_string();
                for rule in &mut filler.rules {
                    rule.id = Uuid::new_v4();
                    rule.group_id = filler.id;
                }
                filler
            })
            .collect::<Vec<_>>();
        groups.push(target);
        *memory.policy_groups.write().await = groups;
    }

    assert_eq!(repo.evaluate_policy_rules().await.unwrap(), 1);
    if let Repository::Memory(memory) = &repo {
        assert_eq!(memory.policy_alerts.read().await.len(), 1);
        assert_eq!(
            memory.policy_alerts.read().await[0].client_id,
            "zzz-policy-target"
        );
    }
}

#[tokio::test]
async fn policy_evaluator_isolates_malformed_persisted_group_selector() {
    let repo = Repository::Memory(MemoryState::default());
    let healthy = repo
        .upsert_fleet_alert_policy(
            &CreateFleetAlertPolicyRequest {
                id: None,
                name: "healthy-policy".to_string(),
                enabled: true,
                selector_expression: "id:healthy-policy-target".to_string(),
                rules: vec![PolicyRuleRequest {
                    id: None,
                    name: "healthy threshold".to_string(),
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
            &test_operator(),
        )
        .await
        .unwrap();
    let malformed_id = Uuid::new_v4();
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.push(AgentView {
            id: "healthy-policy-target".to_string(),
            display_name: "Healthy policy target".to_string(),
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
        });
        memory
            .telemetry_rollups
            .write()
            .await
            .push(alert_test_rollup("healthy-policy-target", 2.0, 500, 1500));
        let mut malformed = healthy.clone();
        malformed.id = malformed_id;
        malformed.name = "malformed-persisted-policy".to_string();
        malformed.selector_expression = "(id:broken".to_string();
        for rule in &mut malformed.rules {
            rule.id = Uuid::new_v4();
            rule.group_id = malformed_id;
        }
        memory.policy_groups.write().await.insert(0, malformed);
    }

    let error = repo.evaluate_policy_rules().await.unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("fleet_alert_policy_evaluation_partial_failure"));
    assert!(message.contains(&malformed_id.to_string()));
    assert!(message.contains("malformed-persisted-policy"));
    if let Repository::Memory(memory) = &repo {
        let alerts = memory.policy_alerts.read().await;
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].policy_group_id, healthy.id);
        assert_eq!(alerts[0].client_id, "healthy-policy-target");
    }
}

#[tokio::test]
async fn policy_evaluator_rejects_malformed_persisted_traffic_selector_without_fallback() {
    let repo = Repository::Memory(MemoryState::default());
    let policy = repo
        .upsert_fleet_alert_policy(
            &CreateFleetAlertPolicyRequest {
                id: None,
                name: "malformed-traffic-selector-policy".to_string(),
                enabled: true,
                selector_expression: "id:traffic-policy-target".to_string(),
                rules: vec![PolicyRuleRequest {
                    id: None,
                    name: "traffic threshold".to_string(),
                    enabled: true,
                    traffic_selector: Some("eth1".to_string()),
                    condition_expression: "traffic.cycle.total >= 1".to_string(),
                    window_secs: 0,
                    severity: "warning".to_string(),
                }],
                notes: None,
                confirmed: true,
                preview_hash: None,
            },
            &test_operator(),
        )
        .await
        .unwrap();
    let client_id = "traffic-policy-target";
    let now_unix = i64::try_from(unix_now()).unwrap();
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.push(AgentView {
            id: client_id.to_string(),
            display_name: "Traffic policy target".to_string(),
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
        });
        let stored_rule =
            |key: &str, value_raw: &str, value_json: serde_json::Value| VpsRuleValueRecord {
                client_id: client_id.to_string(),
                key: key.to_string(),
                value_raw: value_raw.to_string(),
                value_json,
                parsed_display: value_raw.to_string(),
                state: "ok".to_string(),
                validation_errors: Vec::new(),
                source_kind: "test".to_string(),
                source_id: None,
                updated_by: None,
                updated_at: now_unix.to_string(),
            };
        memory.vps_rule_values.write().await.extend([
            stored_rule(VPS_RULE_KEY_TRAFFIC_RESET_DAY, "1", json!({"day": 1})),
            stored_rule(
                VPS_RULE_KEY_TRAFFIC_SELECTORS,
                "eth0",
                json!({"selectors": []}),
            ),
            stored_rule(
                VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL,
                "1GB",
                json!({"bytes": 1_000_000_000_i64}),
            ),
        ]);
        memory.traffic_counter_samples.write().await.extend([
            TrafficCounterSampleRecord {
                client_id: client_id.to_string(),
                source_kind: "host".to_string(),
                interface: "eth0".to_string(),
                observed_at: (now_unix - 60).to_string(),
                observed_unix: now_unix - 60,
                rx_bytes: 100,
                tx_bytes: 100,
                counter_epoch: 1,
                sample_source: "test".to_string(),
            },
            TrafficCounterSampleRecord {
                client_id: client_id.to_string(),
                source_kind: "host".to_string(),
                interface: "eth0".to_string(),
                observed_at: (now_unix - 1).to_string(),
                observed_unix: now_unix - 1,
                rx_bytes: 1_000,
                tx_bytes: 1_000,
                counter_epoch: 1,
                sample_source: "test".to_string(),
            },
        ]);
        let mut groups = memory.policy_groups.write().await;
        let stored = groups
            .iter_mut()
            .find(|group| group.id == policy.id)
            .unwrap();
        stored.rules[0].traffic_selector = Some("host:".to_string());
    }

    assert_eq!(repo.evaluate_policy_rules().await.unwrap(), 0);
    if let Repository::Memory(memory) = &repo {
        assert!(memory.policy_alerts.read().await.is_empty());
        let states = memory.policy_rule_states.read().await;
        let state = states
            .iter()
            .find(|state| {
                state.policy_rule_id == policy.rules[0].id && state.client_id == client_id
            })
            .unwrap();
        assert!(state.incomplete);
        assert!(!state.condition_true);
        assert!(state.incomplete_reasons.iter().any(|reason| {
            reason == "traffic.policy_selector invalid: traffic_selector_interface_required"
        }));
    }
}

#[tokio::test]
async fn policy_rollups_exceed_public_page_without_truncating_preview_or_evaluation() {
    const CLIENT_COUNT: usize = 5_001;
    const PUBLIC_PAGE_SIZE: i64 = 5_000;

    let repo = Repository::Memory(MemoryState::default());
    let policy = repo
        .upsert_fleet_alert_policy(
            &CreateFleetAlertPolicyRequest {
                id: None,
                name: "large-fleet-policy".to_string(),
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
                confirmed: true,
                preview_hash: None,
            },
            &test_operator(),
        )
        .await
        .unwrap();
    if let Repository::Memory(memory) = &repo {
        memory
            .agents
            .write()
            .await
            .extend((0..CLIENT_COUNT).map(|index| {
                let client_id = format!("scale-client-{index:05}");
                AgentView {
                    id: client_id.clone(),
                    display_name: client_id,
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
                }
            }));
        memory.telemetry_rollups.write().await.extend(
            (0..CLIENT_COUNT).map(|index| {
                alert_test_rollup(&format!("scale-client-{index:05}"), 2.0, 500, 1500)
            }),
        );
    }

    let public_page = repo
        .list_latest_telemetry_rollups(PUBLIC_PAGE_SIZE, None, None)
        .await
        .unwrap();
    assert_eq!(public_page.len(), PUBLIC_PAGE_SIZE as usize);

    let preview = repo
        .dry_run_fleet_alert_policy(&PolicyDryRunRequest {
            id: Some(policy.id),
            name: policy.name.clone(),
            enabled: true,
            selector_expression: "*".to_string(),
            rules: vec![PolicyRuleRequest {
                id: Some(policy.rules[0].id),
                name: policy.rules[0].name.clone(),
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
    assert_eq!(preview.matched_vps_count, CLIENT_COUNT);
    assert_eq!(
        preview.rule_previews[0].true_count,
        i64::try_from(CLIENT_COUNT).unwrap()
    );
    assert_eq!(preview.rule_previews[0].false_count, 0);
    assert_eq!(preview.rule_previews[0].incomplete_count, 0);

    let last_client_id = format!("scale-client-{:05}", CLIENT_COUNT - 1);
    let base_alerts = alert_test_state(repo.clone())
        .list_fleet_alerts(FleetAlertQuery {
            limit: Some(100),
            client_id: Some(last_client_id.clone()),
            severity: None,
            category: Some("resource".to_string()),
            operator_state: None,
            include_muted: None,
        })
        .await
        .unwrap();
    assert!(
        base_alerts.iter().any(|alert| {
            alert.client_id.as_deref() == Some(last_client_id.as_str())
                && alert.status == "cpu_load_high"
        }),
        "base resource alerts must include clients beyond the public telemetry page"
    );

    assert_eq!(repo.evaluate_policy_rules().await.unwrap(), CLIENT_COUNT);
    if let Repository::Memory(memory) = &repo {
        assert_eq!(
            memory
                .policy_rule_states
                .read()
                .await
                .iter()
                .filter(|state| state.policy_rule_id == policy.rules[0].id)
                .count(),
            CLIENT_COUNT
        );
        assert_eq!(
            memory
                .policy_alerts
                .read()
                .await
                .iter()
                .filter(|alert| alert.policy_rule_id == policy.rules[0].id)
                .count(),
            CLIENT_COUNT
        );
    }
}

#[tokio::test]
async fn fleet_alert_candidates_are_not_hidden_by_public_page_caps() {
    let repo = Repository::Memory(MemoryState::default());
    let tunnel_input = alert_test_tunnel_input();
    let tunnel_plan = vpsman_common::plan_tunnel(&tunnel_input).unwrap();
    let saved_tunnel = repo
        .record_tunnel_plan(&tunnel_input, &tunnel_plan, true, &test_operator())
        .await
        .unwrap();
    let policy_alert_id = Uuid::new_v4();
    let failed_backup_id = Uuid::new_v4();
    let failed_job_id = Uuid::new_v4();

    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.extend([
            AgentView {
                id: "edge-a".to_string(),
                display_name: "Edge A".to_string(),
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

        let policy_group_id = Uuid::new_v4();
        let policy_rule_id = Uuid::new_v4();
        memory.policy_alerts.write().await.push(PolicyAlertRecord {
            id: policy_alert_id,
            policy_group_id,
            policy_rule_id,
            client_id: "edge-a".to_string(),
            trigger_generation: 1,
            severity: "warning".to_string(),
            category: "resource".to_string(),
            title: "Older scoped policy alert".to_string(),
            detail: "must not be hidden by newer unrelated alerts".to_string(),
            actual_value: Some(2.0),
            threshold_value: Some(1.0),
            payload: json!({"regression": "policy_candidate"}),
            observed_at: "00000".to_string(),
            created_at: "00000".to_string(),
        });
        memory
            .policy_alerts
            .write()
            .await
            .extend((1..=200).map(|index| PolicyAlertRecord {
                id: Uuid::new_v4(),
                policy_group_id,
                policy_rule_id,
                client_id: "filler".to_string(),
                trigger_generation: index,
                severity: "critical".to_string(),
                category: "resource".to_string(),
                title: "Newer filler policy alert".to_string(),
                detail: "not in the requested scope".to_string(),
                actual_value: None,
                threshold_value: None,
                payload: json!({}),
                observed_at: format!("{index:05}"),
                created_at: format!("{index:05}"),
            }));

        memory
            .backup_requests
            .write()
            .await
            .push(BackupRequestView {
                id: failed_backup_id,
                actor_id: None,
                client_id: "edge-a".to_string(),
                paths: vec!["/srv/app".to_string()],
                include_config: true,
                follow_symlinks: false,
                missing_path_policy: vpsman_common::BackupMissingPathPolicy::Fail,
                status: "execution_failed".to_string(),
                payload_hash: "a".repeat(64),
                command_scope: "client:edge-a".to_string(),
                artifact_id: None,
                source_job_id: None,
                source_schedule_id: None,
                note: None,
                created_at: "00000".to_string(),
            });
        memory
            .backup_requests
            .write()
            .await
            .extend((1..=200).map(|index| BackupRequestView {
                id: Uuid::new_v4(),
                actor_id: None,
                client_id: "filler".to_string(),
                paths: vec!["/tmp".to_string()],
                include_config: false,
                follow_symlinks: false,
                missing_path_policy: vpsman_common::BackupMissingPathPolicy::Fail,
                status: "artifact_metadata_recorded".to_string(),
                payload_hash: "b".repeat(64),
                command_scope: "client:filler".to_string(),
                artifact_id: None,
                source_job_id: None,
                source_schedule_id: None,
                note: None,
                created_at: format!("{index:05}"),
            }));

        memory.jobs.write().await.push(JobHistoryView {
            id: failed_job_id,
            actor_id: None,
            command_type: "shell".to_string(),
            source_schedule_id: None,
            privileged: false,
            status: "failed".to_string(),
            target_count: 1,
            payload_hash: "c".repeat(64),
            max_timeout_secs: 30,
            created_at: "00000".to_string(),
            completed_at: Some("00000".to_string()),
        });
        memory
            .jobs
            .write()
            .await
            .extend((1..=200).map(|index| JobHistoryView {
                id: Uuid::new_v4(),
                actor_id: None,
                command_type: "shell".to_string(),
                source_schedule_id: None,
                privileged: false,
                status: "completed".to_string(),
                target_count: 1,
                payload_hash: "d".repeat(64),
                max_timeout_secs: 30,
                created_at: format!("{index:05}"),
                completed_at: Some(format!("{index:05}")),
            }));

        let healthy_adapter = TelemetryTunnelAdapterHealthView {
            status: "ok".to_string(),
            checked_unix: 1,
            configured: true,
            success: true,
            exit_code: Some(0),
            reason: None,
            duration_ms: 1,
            command_sha256_hex: Some("0".repeat(64)),
            timed_out: false,
            output_truncated: false,
            stdout_sha256_hex: None,
            stderr_sha256_hex: None,
        };
        let healthy_tunnel = TelemetryTunnelView {
            client_id: "edge-a".to_string(),
            observed_at: "00001".to_string(),
            interface: "gre42".to_string(),
            kind: "gre".to_string(),
            ownership_mode: "managed".to_string(),
            mutation_policy: "managed".to_string(),
            plan_id: Some(saved_tunnel.id),
            plan_name: Some(saved_tunnel.name.clone()),
            plan_runtime_manager: None,
            endpoint_side: Some("left".to_string()),
            peer_client_id: Some("edge-b".to_string()),
            source: "telemetry".to_string(),
            operstate: Some("up".to_string()),
            mtu: Some(1476),
            link_type: Some(778),
            address: Some("10.42.0.0".to_string()),
            rx_bytes: 100,
            tx_bytes: 200,
            traffic_source: Some("vnstat".to_string()),
            traffic_status: Some("ok".to_string()),
            traffic_reason: None,
            traffic_checked_unix: Some(1),
            adapter_health: Some(healthy_adapter),
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
        };
        memory
            .telemetry_tunnels
            .write()
            .await
            .extend((1..=5_000).map(|index| {
                let mut tunnel = healthy_tunnel.clone();
                tunnel.observed_at = format!("{index:05}");
                tunnel
            }));
        let mut failed_tunnel = healthy_tunnel;
        failed_tunnel.observed_at = "00000".to_string();
        failed_tunnel.adapter_health.as_mut().unwrap().status = "failed".to_string();
        failed_tunnel.adapter_health.as_mut().unwrap().success = false;
        failed_tunnel.adapter_health.as_mut().unwrap().exit_code = Some(1);
        failed_tunnel.adapter_health.as_mut().unwrap().reason =
            Some("adapter status command failed".to_string());
        memory.telemetry_tunnels.write().await.push(failed_tunnel);

        let source_template_id = Uuid::new_v4();
        memory
            .source_templates
            .write()
            .await
            .push(SourceTemplateView {
                id: source_template_id,
                domain: "runtime_tunnel_adapter".to_string(),
                name: "shared:runtime-adapter".to_string(),
                scope: "shared".to_string(),
                built_in: false,
                is_default: false,
                owner_client_id: None,
                description: None,
                definition: json!({"manager": "external_managed_adapter"}),
                assigned_client_count: 1,
                created_at: "00000".to_string(),
                updated_at: "00000".to_string(),
            });
        memory
            .source_template_assignments
            .write()
            .await
            .push(SourceTemplateAssignmentView {
                client_id: "edge-a".to_string(),
                domain: "runtime_tunnel_adapter".to_string(),
                template_id: source_template_id,
                template_name: "shared:runtime-adapter".to_string(),
                template_scope: "shared".to_string(),
                assigned_at: "00000".to_string(),
            });
    }

    assert!(!repo
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
    assert!(!repo
        .list_backup_requests(200)
        .await
        .unwrap()
        .iter()
        .any(|request| request.id == failed_backup_id));
    assert!(!repo
        .list_jobs(200)
        .await
        .unwrap()
        .iter()
        .any(|job| job.id == failed_job_id));
    assert!(repo
        .list_telemetry_tunnels(5_000, None, None)
        .await
        .unwrap()
        .iter()
        .all(|tunnel| {
            tunnel
                .adapter_health
                .as_ref()
                .is_none_or(|health| health.success)
        }));

    let policy_fleet_alert_id = format!("policy-alert:{policy_alert_id}");
    if let Repository::Memory(memory) = &repo {
        memory
            .fleet_alert_states
            .write()
            .await
            .push(FleetAlertStateView {
                alert_id: policy_fleet_alert_id.clone(),
                state: "acknowledged".to_string(),
                muted_until_unix: None,
                escalation_level: 0,
                reason: Some("known older alert".to_string()),
                actor_id: None,
                created_at: "00000".to_string(),
                updated_at: "00000".to_string(),
            });
        memory
            .fleet_alert_states
            .write()
            .await
            .extend((1..=1_000).map(|index| FleetAlertStateView {
                alert_id: format!("unrelated:{index:04}"),
                state: "open".to_string(),
                muted_until_unix: None,
                escalation_level: 0,
                reason: None,
                actor_id: None,
                created_at: format!("{index:05}"),
                updated_at: format!("{index:05}"),
            }));
    }
    assert!(repo
        .list_fleet_alert_states(1_000, None)
        .await
        .unwrap()
        .iter()
        .all(|state| state.alert_id != policy_fleet_alert_id));

    let state = alert_test_state(repo);
    let acknowledged_policy = state
        .list_fleet_alerts(FleetAlertQuery {
            limit: Some(1),
            client_id: Some("edge-a".to_string()),
            severity: Some("warning".to_string()),
            category: Some("resource".to_string()),
            operator_state: Some("acknowledged".to_string()),
            include_muted: None,
        })
        .await
        .unwrap();
    assert_eq!(acknowledged_policy.len(), 1);
    assert_eq!(acknowledged_policy[0].id, policy_fleet_alert_id);

    let backup_alerts = state
        .list_fleet_alerts(FleetAlertQuery {
            limit: Some(1),
            client_id: Some("edge-a".to_string()),
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

    let tunnel_alerts = state
        .list_fleet_alerts(FleetAlertQuery {
            limit: Some(1),
            client_id: Some("edge-a".to_string()),
            severity: Some("critical".to_string()),
            category: Some("network".to_string()),
            operator_state: None,
            include_muted: None,
        })
        .await
        .unwrap();
    assert_eq!(tunnel_alerts.len(), 1);
    assert_eq!(tunnel_alerts[0].status, "tunnel_adapter_degraded");

    let source_alerts = state
        .list_fleet_alerts(FleetAlertQuery {
            limit: Some(100),
            client_id: Some("edge-a".to_string()),
            severity: Some("warning".to_string()),
            category: Some("source_readiness".to_string()),
            operator_state: None,
            include_muted: None,
        })
        .await
        .unwrap();
    let source_alert = source_alerts
        .iter()
        .find(|alert| alert.evidence["domain"] == "runtime_tunnel_adapter")
        .unwrap();
    assert_eq!(source_alert.status, "degraded");
    assert_eq!(source_alert.evidence["evidence"]["sample_count"], 5_001);
    assert_eq!(source_alert.evidence["evidence"]["truncated_count"], 4_901);
    assert_eq!(
        source_alert.evidence["evidence"]["samples"]
            .as_array()
            .unwrap()
            .len(),
        100
    );
    assert_eq!(
        source_alert.evidence["evidence"]["samples"][0]["adapter_status"],
        "failed"
    );
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
async fn fleet_alert_notification_dispatch_rejects_channel_overflow_explicitly() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        let mut channels = memory.fleet_alert_notification_channels.write().await;
        for index in 0..=1_000 {
            channels.push(
                crate::model_alert_notifications::FleetAlertNotificationChannelView {
                    id: Uuid::new_v4(),
                    name: format!("channel-{index:04}"),
                    scope_kind: "global".to_string(),
                    scope_value: None,
                    min_severity: "warning".to_string(),
                    categories: Vec::new(),
                    operator_states: Vec::new(),
                    delivery_kind: "webhook".to_string(),
                    target: "https://hooks.acme.com/fleet".to_string(),
                    cooldown_secs: 60,
                    enabled: true,
                    configuration_error: None,
                    notes: None,
                    actor_id: None,
                    created_at: "0".to_string(),
                    updated_at: "0".to_string(),
                },
            );
        }
    }

    let state = alert_test_state(repo);
    let error = state
        .dispatch_fleet_alert_notifications(
            &FleetAlertNotificationDispatchRequest {
                limit: Some(1),
                client_id: None,
                severity: None,
                category: None,
                operator_state: None,
                include_muted: None,
                dry_run: Some(true),
                preview_hash: None,
                confirmed: false,
            },
            &test_operator(),
        )
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("fleet_alert_notification_dispatch_channel_limit_exceeded"));
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

    let error = repo
        .delete_fleet_alert_notification_channel(channel_id, "stale-name", &operator)
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("fleet_alert_notification_channel_delete_review_stale"));
    assert_eq!(
        repo.list_fleet_alert_notification_channels(20, None, None, None, None)
            .await
            .unwrap()
            .len(),
        1
    );

    repo.delete_fleet_alert_notification_channel(channel_id, "deleted-edge-webhook", &operator)
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

    let error = repo
        .delete_webhook_rule(rule_id, "stale-name", &operator)
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("webhook_rule_delete_review_stale"));
    assert_eq!(repo.list_webhook_rules(20, None).await.unwrap().len(), 1);

    repo.delete_webhook_rule(rule_id, "deleted-edge-rule", &operator)
        .await
        .unwrap();

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
    if let Repository::Memory(memory) = &repo {
        let mut rules = memory.webhook_rules.write().await;
        let scoped_rule = rules
            .iter()
            .find(|rule| rule.id == scoped_rule_id)
            .cloned()
            .unwrap();
        rules.extend((0..1_000).map(|index| {
            let mut rule = scoped_rule.clone();
            rule.id = Uuid::new_v4();
            rule.name = format!("filler-webhook-{index:04}");
            rule
        }));
    }
    assert!(
        repo.list_webhook_rules(1_000, None)
            .await
            .unwrap()
            .iter()
            .all(|rule| rule.id != scoped_rule_id),
        "the scoped rule must be outside the broad list cap for this regression"
    );

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

fn alert_test_tunnel_input() -> vpsman_common::TunnelPlanInput {
    vpsman_common::TunnelPlanInput {
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
