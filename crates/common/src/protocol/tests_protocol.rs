use std::collections::BTreeSet;

use super::{
    agent_update_release_statuses, backup_request_statuses, canonical_db_privilege_intent,
    file_transfer_command_types, file_transfer_session_events, file_transfer_session_status,
    file_transfer_session_statuses, fleet_alert_notification_delivery_process_statuses,
    fleet_alert_notification_delivery_statuses, is_file_transfer_command_type,
    is_file_transfer_session_event, is_fleet_alert_notification_delivery_process_status,
    is_fleet_alert_notification_delivery_status, is_server_job_status, is_server_job_type,
    is_terminal_command_type, is_terminal_session_event, is_topology_edge_health_status,
    is_topology_neighbor_state, is_topology_node_status, is_topology_observation_state,
    is_topology_probe_state, is_topology_runtime_state, is_webhook_rule_delivery_history_status,
    is_webhook_rule_delivery_process_status, is_webhook_rule_delivery_status,
    job_command_confirmation_required_by_operation_type,
    job_command_requires_confirmation_by_operation_type, job_command_safety_by_operation_type,
    job_command_type_by_operation_type, job_command_type_label_from_operation_type,
    job_command_type_labels, job_command_variant_names, job_status_class_by_status,
    job_status_classes, job_statuses, job_target_status_class_by_status, job_target_status_classes,
    job_target_statuses, migration_link_statuses, parse_build_number, restore_plan_statuses,
    server_job_statuses, server_job_types, terminal_command_types, terminal_session_events,
    terminal_session_state, terminal_session_states, terminal_session_statuses,
    topology_edge_health_statuses, topology_neighbor_states, topology_node_statuses,
    topology_observation_states, topology_probe_states, topology_runtime_states,
    webhook_rule_delivery_history_statuses, webhook_rule_delivery_process_statuses,
    webhook_rule_delivery_statuses, AgentHello, AgentRuntimeConfigReloadRequest,
    AgentUpdateReleaseStatus, BackupRequestStatus, JobCommand, JobStatus, JobStatusClass,
    JobTargetStatus, JobTargetStatusClass, MigrationLinkStatus, RestorePlanStatus, ServerHello,
    JOB_COMMAND_SAFETY_EXCLUSIVE, JOB_COMMAND_SAFETY_EXEC, JOB_COMMAND_SAFETY_READ,
    JOB_COMMAND_SAFETY_WRITE, JOB_STATUS_CLASSES, JOB_STATUS_PARTIAL_SUCCESS, JOB_STATUS_SKIPPED,
    JOB_TARGET_STATUS_CLASSES, TARGET_STATUS_SKIPPED,
};

#[test]
fn parses_positive_build_numbers_with_default_fallback() {
    assert_eq!(parse_build_number(Some("42")), 42);
    assert_eq!(parse_build_number(Some(" 7 ")), 7);
    assert_eq!(parse_build_number(Some("0")), 1);
    assert_eq!(parse_build_number(Some("not-a-number")), 1);
    assert_eq!(parse_build_number(None), 1);
}

#[test]
fn server_hello_carries_server_version_and_build_number() {
    let hello = ServerHello {
        server_id: "gateway-a".to_string(),
        server_version: "0.1.0".to_string(),
        server_build_number: 1001,
        accepted: true,
        message: "accepted".to_string(),
        telemetry_interval_secs: 15,
    };

    let encoded = serde_json::to_value(&hello).unwrap();
    assert_eq!(encoded["server_version"], "0.1.0");
    assert_eq!(encoded["server_build_number"], 1001);
}

#[test]
fn legacy_runtime_reload_request_does_not_force_a_sync() {
    let request: AgentRuntimeConfigReloadRequest = serde_json::from_value(serde_json::json!({
        "client_id": "edge-a",
        "current_content_hash": "ab",
        "reason": "legacy-agent"
    }))
    .unwrap();

    assert!(!request.requires_authoritative_sync);
    assert!(request.reconcile_resources.is_empty());
    assert!(!request.requires_port_forwarding_sync);
}

#[test]
fn runtime_reload_scope_is_additive_and_accepts_legacy_forwarding_requests() {
    let request: AgentRuntimeConfigReloadRequest = serde_json::from_value(serde_json::json!({
        "client_id": "edge-a",
        "current_content_hash": "ab",
        "reason": "reconnect",
        "reconcile_resources": ["runtime_tunnels"],
        "requires_port_forwarding_sync": true
    }))
    .unwrap();
    let scope = super::RuntimeConfigReconcileScope::from_reload_request(&request);

    assert!(scope.includes(super::RuntimeConfigReconcileResource::RuntimeTunnels));
    assert!(scope.includes(super::RuntimeConfigReconcileResource::PortForwarding));
    assert!(!scope.authoritative);
}

#[test]
fn tunnel_deletion_reason_covers_runtime_tunnel_reconciliation() {
    let scope = super::runtime_config_reconcile_scope_from_reason("tunnel_plan_deleted");

    assert!(scope.includes(super::RuntimeConfigReconcileResource::RuntimeTunnels));
    assert!(!scope.authoritative);
}

#[test]
fn hello_payloads_remain_additive_across_rolling_updates() {
    let process_incarnation_id = uuid::Uuid::new_v4();
    let legacy_agent = serde_json::json!({
        "client_id": "edge-a",
        "process_incarnation_id": process_incarnation_id,
        "agent_version": "0.1.0",
        "os_release": "Linux",
        "arch": "x86_64"
    });
    let decoded_agent: AgentHello = serde_json::from_value(legacy_agent).unwrap();
    assert_eq!(decoded_agent.internal_build_number, 1);
    assert!(decoded_agent.update_heartbeat.is_none());

    let future_agent = serde_json::json!({
        "client_id": "edge-a",
        "process_incarnation_id": process_incarnation_id,
        "agent_version": "0.2.0",
        "internal_build_number": 2000,
        "os_release": "Linux",
        "arch": "x86_64",
        "capabilities": {},
        "future_optional_capability": { "enabled": true }
    });
    let decoded_agent: AgentHello = serde_json::from_value(future_agent).unwrap();
    assert_eq!(decoded_agent.internal_build_number, 2000);

    let legacy_server = serde_json::json!({
        "server_id": "gateway-a",
        "server_version": "0.1.0",
        "accepted": true,
        "message": "accepted",
        "telemetry_interval_secs": 15
    });
    let decoded_server: ServerHello = serde_json::from_value(legacy_server).unwrap();
    assert_eq!(decoded_server.server_build_number, 1);

    let mut future_server = serde_json::to_value(decoded_server).unwrap();
    future_server["future_optional_policy"] = serde_json::json!("ignored");
    let decoded_server: ServerHello = serde_json::from_value(future_server).unwrap();
    assert!(decoded_server.accepted);
}

#[test]
fn legacy_job_requests_default_to_the_minimum_protocol() {
    let request: super::JobRequest = serde_json::from_value(serde_json::json!({
        "job_id": uuid::Uuid::new_v4(),
        "command": {
            "type": "shell",
            "argv": ["/bin/true"],
            "pty": false
        },
        "max_timeout_secs": 30
    }))
    .unwrap();

    assert_eq!(request.command_version, super::MIN_COMMAND_PROTOCOL_VERSION);
}

#[test]
fn update_commands_keep_the_legacy_dispatch_protocol() {
    let command = JobCommand::AgentUpdateCheck {
        version_url: Some("https://updates.example/version.json".to_string()),
        activate: false,
        restart_agent: false,
    };

    assert_eq!(
        super::job_command_dispatch_protocol_version(&command),
        super::MIN_COMMAND_PROTOCOL_VERSION
    );
}

#[test]
fn update_command_wire_shape_rejects_unversioned_field_drift() {
    let command = serde_json::from_value::<JobCommand>(serde_json::json!({
        "type": "agent_update",
        "artifact_url": "https://updates.example/vpsman-agent",
        "sha256_hex": "ab".repeat(32),
        "future_optional_policy": "must-use-a-new-command-variant"
    }));

    assert!(command.is_err());
}

#[test]
fn job_status_model_is_total_and_strict() {
    let mut mapped_statuses = BTreeSet::new();
    let mut used_classes = BTreeSet::new();
    for (status, status_class) in job_status_class_by_status() {
        mapped_statuses.insert(*status);
        used_classes.insert(*status_class);
        let parsed_status = JobStatus::parse(status).expect("canonical job status parses");
        let parsed_class =
            JobStatusClass::parse(status_class).expect("canonical job status class parses");
        assert_eq!(parsed_status.as_str(), *status);
        assert_eq!(parsed_status.class(), parsed_class);
        assert_eq!(
            parsed_status.is_in_progress(),
            parsed_class.is_in_progress()
        );
        assert_eq!(parsed_status.is_terminal(), parsed_class.is_terminal());
        assert_eq!(
            parsed_status.is_success(),
            parsed_class.is_successful_outcome()
        );
        assert_eq!(
            parsed_status.is_unsuccessful_terminal(),
            parsed_class.is_unsuccessful_outcome()
        );
    }
    assert_eq!(
        mapped_statuses,
        job_statuses().iter().copied().collect::<BTreeSet<_>>()
    );
    assert_eq!(
        used_classes,
        job_status_classes()
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(
        job_status_classes(),
        &JOB_STATUS_CLASSES,
        "generated helper must expose the canonical class array"
    );
    assert!(JobStatus::parse("not_canonical_job_status").is_none());
    assert_eq!(
        JobStatus::parse(JOB_STATUS_PARTIAL_SUCCESS)
            .unwrap()
            .class(),
        JobStatusClass::PartialSuccess
    );
    assert_eq!(
        JobStatus::parse(JOB_STATUS_SKIPPED).unwrap().class(),
        JobStatusClass::Skipped
    );
}

#[test]
fn target_status_model_is_total_and_strict() {
    let mut mapped_statuses = BTreeSet::new();
    let mut used_classes = BTreeSet::new();
    for (status, status_class) in job_target_status_class_by_status() {
        mapped_statuses.insert(*status);
        used_classes.insert(*status_class);
        let parsed_status = JobTargetStatus::parse(status).expect("canonical target status parses");
        let parsed_class = JobTargetStatusClass::parse(status_class)
            .expect("canonical target status class parses");
        assert_eq!(parsed_status.as_str(), *status);
        assert_eq!(parsed_status.class(), parsed_class);
        assert_eq!(parsed_status.is_active(), parsed_class.is_in_progress());
        assert_eq!(parsed_status.is_terminal(), parsed_class.is_terminal());
        assert_eq!(
            parsed_status.is_success(),
            parsed_class.is_successful_outcome()
        );
        assert_eq!(
            parsed_status.is_unsuccessful_terminal(),
            parsed_class.is_unsuccessful_outcome()
        );
    }
    assert_eq!(
        mapped_statuses,
        job_target_statuses()
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(
        used_classes,
        job_target_status_classes()
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(
        job_target_status_classes(),
        &JOB_TARGET_STATUS_CLASSES,
        "generated helper must expose the canonical target class array"
    );
    assert!(JobTargetStatus::parse("not_canonical_target_status").is_none());
    assert_eq!(
        JobTargetStatus::parse(TARGET_STATUS_SKIPPED)
            .unwrap()
            .class(),
        JobTargetStatusClass::Skipped
    );
}

#[test]
fn command_contracts_are_total_and_strict() {
    let operation_types = job_command_variant_names()
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let safety_keys = job_command_safety_by_operation_type()
        .iter()
        .map(|(operation_type, _)| *operation_type)
        .collect::<BTreeSet<_>>();
    let command_type_keys = job_command_type_by_operation_type()
        .iter()
        .map(|(operation_type, _)| *operation_type)
        .collect::<BTreeSet<_>>();
    let confirmation_keys = job_command_confirmation_required_by_operation_type()
        .iter()
        .map(|(operation_type, _)| *operation_type)
        .collect::<BTreeSet<_>>();
    assert_eq!(operation_types, safety_keys);
    assert_eq!(operation_types, command_type_keys);
    assert_eq!(operation_types, confirmation_keys);
    assert!(job_command_type_labels().contains(&"shell_pty"));
    assert_eq!(
        job_command_requires_confirmation_by_operation_type("shell"),
        Some(true)
    );
    assert_eq!(
        job_command_requires_confirmation_by_operation_type("file_transfer_download_start"),
        Some(false)
    );
    assert_eq!(
        job_command_requires_confirmation_by_operation_type("process_logs"),
        Some(false)
    );
    assert_eq!(
        job_command_requires_confirmation_by_operation_type("network_speed_test"),
        Some(true)
    );
    assert_eq!(
        job_command_type_label_from_operation_type("shell"),
        Some("shell_argv")
    );
    assert_eq!(
        job_command_safety_by_operation_type()
            .iter()
            .find(|(operation_type, _)| *operation_type == "backup")
            .map(|(_, safety)| *safety),
        Some(JOB_COMMAND_SAFETY_READ)
    );
    assert_eq!(
        job_command_safety_by_operation_type()
            .iter()
            .find(|(operation_type, _)| *operation_type == "network_status")
            .map(|(_, safety)| *safety),
        Some(JOB_COMMAND_SAFETY_READ)
    );
    assert_eq!(
        job_command_safety_by_operation_type()
            .iter()
            .find(|(operation_type, _)| *operation_type == "network_speed_test")
            .map(|(_, safety)| *safety),
        Some(JOB_COMMAND_SAFETY_EXEC)
    );
    assert_eq!(
        job_command_safety_by_operation_type()
            .iter()
            .find(|(operation_type, _)| *operation_type == "restore")
            .map(|(_, safety)| *safety),
        Some(JOB_COMMAND_SAFETY_WRITE)
    );
    assert_eq!(
        job_command_safety_by_operation_type()
            .iter()
            .find(|(operation_type, _)| *operation_type == "agent_update")
            .map(|(_, safety)| *safety),
        Some(JOB_COMMAND_SAFETY_EXCLUSIVE)
    );
}

#[test]
fn terminal_and_file_transfer_contracts_are_closed() {
    for command_type in terminal_command_types() {
        assert!(is_terminal_command_type(command_type));
    }
    for event in terminal_session_events() {
        assert!(is_terminal_session_event(event));
    }
    for command_type in file_transfer_command_types() {
        assert!(is_file_transfer_command_type(command_type));
    }
    for event in file_transfer_session_events() {
        assert!(is_file_transfer_session_event(event));
    }
    assert!(terminal_session_states().contains(&terminal_session_state(
        "terminal_open",
        "opened",
        false
    )));
    assert!(terminal_session_statuses().contains(&"idle_timeout"));
    assert!(
        file_transfer_session_statuses().contains(&file_transfer_session_status(
            "file_transfer_download_chunk",
            true
        ))
    );
}

#[test]
fn topology_contracts_are_closed_and_separate_observation_from_probe_states() {
    for status in topology_node_statuses() {
        assert!(is_topology_node_status(status));
    }
    for status in topology_edge_health_statuses() {
        assert!(is_topology_edge_health_status(status));
    }
    for status in topology_neighbor_states() {
        assert!(is_topology_neighbor_state(status));
    }
    for status in topology_probe_states() {
        assert!(is_topology_probe_state(status));
    }
    for status in topology_runtime_states() {
        assert!(is_topology_runtime_state(status));
    }
    for status in topology_observation_states() {
        assert!(is_topology_observation_state(status));
    }
    assert!(is_topology_observation_state("recorded"));
    assert!(!is_topology_probe_state("recorded"));
}

#[test]
fn operator_queue_contracts_are_closed() {
    for job_type in server_job_types() {
        assert!(is_server_job_type(job_type));
    }
    for status in server_job_statuses() {
        assert!(is_server_job_status(status));
    }
    for status in fleet_alert_notification_delivery_statuses() {
        assert!(is_fleet_alert_notification_delivery_status(status));
    }
    for status in fleet_alert_notification_delivery_process_statuses() {
        assert!(is_fleet_alert_notification_delivery_process_status(status));
        assert!(is_fleet_alert_notification_delivery_status(status));
    }
    for status in webhook_rule_delivery_statuses() {
        assert!(is_webhook_rule_delivery_status(status));
    }
    for status in webhook_rule_delivery_history_statuses() {
        assert!(is_webhook_rule_delivery_history_status(status));
        assert!(is_webhook_rule_delivery_status(status));
    }
    for status in webhook_rule_delivery_process_statuses() {
        assert!(is_webhook_rule_delivery_process_status(status));
        assert!(is_webhook_rule_delivery_status(status));
    }
    assert!(is_webhook_rule_delivery_status("permanently_failed"));
    assert!(is_webhook_rule_delivery_history_status(
        "permanently_failed"
    ));
    assert!(!is_webhook_rule_delivery_process_status(
        "permanently_failed"
    ));
    assert!(!is_webhook_rule_delivery_history_status("matched_dry_run"));
    assert!(is_fleet_alert_notification_delivery_status(
        "matched_dry_run"
    ));
    assert!(!is_fleet_alert_notification_delivery_process_status(
        "matched_dry_run"
    ));
}

#[test]
fn domain_status_class_maps_are_total() {
    assert_status_class_map_total(
        terminal_session_states(),
        super::terminal_session_state_class_by_state(),
    );
    assert_status_class_map_total(
        terminal_session_statuses(),
        super::terminal_session_status_class_by_status(),
    );
    assert_status_class_map_total(
        file_transfer_session_statuses(),
        super::file_transfer_session_status_class_by_status(),
    );
    assert_status_class_map_total(
        backup_request_statuses(),
        super::backup_request_status_class_by_status(),
    );
    assert_status_class_map_total(
        restore_plan_statuses(),
        super::restore_plan_status_class_by_status(),
    );
    assert_status_class_map_total(
        migration_link_statuses(),
        super::migration_link_status_class_by_status(),
    );
    assert_status_class_map_total(
        super::agent_update_release_statuses(),
        super::agent_update_release_status_class_by_status(),
    );
    assert_status_class_map_total(
        server_job_statuses(),
        super::server_job_status_class_by_status(),
    );
    assert_status_class_map_total(
        fleet_alert_notification_delivery_statuses(),
        super::fleet_alert_notification_delivery_status_class_by_status(),
    );
    assert_status_class_map_total(
        fleet_alert_notification_delivery_process_statuses(),
        super::fleet_alert_notification_delivery_process_status_class_by_status(),
    );
    assert_status_class_map_total(
        webhook_rule_delivery_statuses(),
        super::webhook_rule_delivery_status_class_by_status(),
    );
    assert_status_class_map_total(
        webhook_rule_delivery_history_statuses(),
        super::webhook_rule_delivery_history_status_class_by_status(),
    );
    assert_status_class_map_total(
        webhook_rule_delivery_process_statuses(),
        super::webhook_rule_delivery_process_status_class_by_status(),
    );
    assert_status_class_map_total(
        topology_node_statuses(),
        super::topology_node_status_class_by_status(),
    );
    assert_status_class_map_total(
        topology_edge_health_statuses(),
        super::topology_edge_health_status_class_by_status(),
    );
    assert_status_class_map_total(
        topology_neighbor_states(),
        super::topology_neighbor_state_class_by_state(),
    );
    assert_status_class_map_total(
        topology_probe_states(),
        super::topology_probe_state_class_by_state(),
    );
    assert_status_class_map_total(
        topology_runtime_states(),
        super::topology_runtime_state_class_by_state(),
    );
    assert_status_class_map_total(
        topology_observation_states(),
        super::topology_observation_state_class_by_state(),
    );
}

fn assert_status_class_map_total(statuses: &[&str], status_class_by_status: &[(&str, &str)]) {
    let expected = statuses.iter().copied().collect::<BTreeSet<_>>();
    let actual = status_class_by_status
        .iter()
        .map(|(status, _)| *status)
        .collect::<BTreeSet<_>>();
    assert_eq!(expected, actual);
    for (_, status_class) in status_class_by_status {
        assert!(super::workflow_status_classes().contains(status_class));
    }
}

#[test]
fn finite_storage_status_contracts_parse() {
    for status in backup_request_statuses() {
        assert_eq!(
            BackupRequestStatus::from_storage(status).map(BackupRequestStatus::as_str),
            Some(*status)
        );
    }
    for status in restore_plan_statuses() {
        assert_eq!(
            RestorePlanStatus::from_storage(status).map(RestorePlanStatus::as_str),
            Some(*status)
        );
    }
    for status in migration_link_statuses() {
        assert_eq!(
            MigrationLinkStatus::from_storage(status).map(MigrationLinkStatus::as_str),
            Some(*status)
        );
    }
    for status in agent_update_release_statuses() {
        assert_eq!(
            AgentUpdateReleaseStatus::from_storage(status).map(AgentUpdateReleaseStatus::as_str),
            Some(*status)
        );
    }
    assert!(BackupRequestStatus::from_storage("old_backup_status").is_none());
}

#[test]
fn serializes_agent_update_with_canonical_name_and_rejects_legacy_alias() {
    let command = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "ab".repeat(32),
    };
    let encoded = serde_json::to_value(&command).unwrap();
    assert_eq!(encoded["type"], "agent_update");

    let legacy = serde_json::json!({
        "type": "update_agent",
        "artifact_url": "https://updates.example/vpsman-agent",
        "sha256_hex": "ab".repeat(32),
    });
    assert!(serde_json::from_value::<JobCommand>(legacy).is_err());
}

#[test]
fn omits_false_agent_update_restart_flag_from_payload_hash_shape() {
    let command = JobCommand::AgentUpdateActivate {
        staged_sha256_hex: "ab".repeat(32),
        restart_agent: false,
    };
    let encoded = serde_json::to_value(&command).unwrap();
    assert_eq!(encoded["type"], "agent_update_activate");
    assert!(encoded.get("restart_agent").is_none());

    let restart = JobCommand::AgentUpdateActivate {
        staged_sha256_hex: "ab".repeat(32),
        restart_agent: true,
    };
    let encoded = serde_json::to_value(&restart).unwrap();
    assert_eq!(encoded["restart_agent"], true);
}

#[test]
fn db_privilege_intent_binds_optional_payload_hash() {
    let resolved_targets = vec!["client-b".to_string(), "client-a".to_string()];
    let intent = canonical_db_privilege_intent(
        "suite_config.update",
        "suite_config",
        None,
        &resolved_targets,
        true,
        Some("ab"),
    )
    .unwrap();

    assert_eq!(
        intent,
        r#"{"version":1,"action":"suite_config.update","target":"suite_config","selector_expression":null,"resolved_targets":["client-a","client-b"],"confirmed":true,"payload_hash":"ab"}"#
    );
}

#[test]
fn job_privilege_intent_preserves_long_timeout_values() {
    let resolved_targets = vec!["client-b".to_string(), "client-a".to_string()];
    let job_intent = super::canonical_job_privilege_intent(super::JobPrivilegeIntentInput {
        selector_expression: "tag:prod",
        command_type: "shell",
        operation_payload_hash: "ab",
        rollout_policy_hash: None,
        resolved_targets: &resolved_targets,
        max_timeout_secs: 7_200,
        force_unprivileged: false,
        privileged: true,
    })
    .unwrap();
    assert!(job_intent.contains(r#""max_timeout_secs":7200"#));
}

#[test]
fn job_privilege_intent_binds_rollout_policy_hash() {
    let resolved_targets = vec!["client-b".to_string(), "client-a".to_string()];
    let intent = super::canonical_job_privilege_intent(super::JobPrivilegeIntentInput {
        selector_expression: " tag:prod ",
        command_type: "shell_argv",
        operation_payload_hash: "ab",
        rollout_policy_hash: Some("cd"),
        resolved_targets: &resolved_targets,
        max_timeout_secs: 30,
        force_unprivileged: false,
        privileged: true,
    })
    .unwrap();

    assert_eq!(
        intent,
        r#"{"version":1,"action":"job.dispatch","selector_expression":"tag:prod","command_type":"shell_argv","operation_payload_hash":"ab","rollout_policy_hash":"cd","resolved_targets":["client-a","client-b"],"max_timeout_secs":30,"force_unprivileged":false,"privileged":true}"#
    );
}

#[test]
fn operator_db_payload_hash_uses_stable_non_secret_shape() {
    let scopes = vec!["jobs:write".to_string(), "fleet:read".to_string()];
    let payload_hash = super::operator_db_payload_hash(super::OperatorDbPayloadInput {
        action: "operator.update",
        target: "operator-id",
        username: None,
        role: Some("operator"),
        scopes: &scopes,
        session_refresh_ttl_secs: Some(86_400),
        status: None,
        admin_risk_acknowledged: false,
    })
    .unwrap();

    assert_eq!(payload_hash.len(), 64);
}

#[test]
fn agent_identity_payload_hash_normalizes_operator_input() {
    let tags = vec![" edge ".to_string(), "bgp".to_string(), "edge".to_string()];
    let hash = super::agent_identity_payload_hash(super::AgentIdentityPayloadInput {
        client_id: " v-16 ",
        public_key: &[0x11; 32],
        display_name: Some(" Edge 16 "),
        tags: &tags,
        replace_existing_key: false,
    });
    let normalized_tags = vec!["bgp".to_string(), "edge".to_string()];
    assert_eq!(
        hash,
        super::agent_identity_payload_hash(super::AgentIdentityPayloadInput {
            client_id: "v-16",
            public_key: &[0x11; 32],
            display_name: Some("Edge 16"),
            tags: &normalized_tags,
            replace_existing_key: false,
        })
    );
}
