use uuid::Uuid;
use vpsman_common::{
    plan_tunnel, AgentCapabilitySnapshot, AgentPrivilegeMode, BandwidthTier, OspfCostPolicy,
    ProcessResourceLimits, ProcessRunPolicy, TunnelEndpointSide, TunnelKind, TunnelPlanInput,
};

use super::*;
use crate::repository_jobs::aggregate_job_status_from_statuses;

#[test]
fn gateway_timeout_output_maps_to_timed_out_target_status() {
    let job_id = Uuid::new_v4();
    let outcome = target_outcome_from_gateway(GatewayCommandDispatchResult {
        client_id: "client-a".to_string(),
        job_id,
        command_version: 1,
        accepted: true,
        message: "accepted".to_string(),
        outputs: vec![CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&serde_json::json!({
                "type": "command_timeout",
                "timeout_secs": 1,
            }))
            .unwrap(),
            exit_code: Some(124),
            done: true,
        }],
    });

    assert_eq!(outcome.status, "agent_timeout");
    assert_eq!(outcome.exit_code, Some(124));
    assert!(outcome.accepted);
    assert_eq!(
        aggregate_job_status_from_statuses(&[outcome.status], 1),
        "agent_timeout"
    );
}

#[test]
fn gateway_unsupported_command_output_maps_to_rejected_target_status() {
    let job_id = Uuid::new_v4();
    let outcome = target_outcome_from_gateway(GatewayCommandDispatchResult {
        client_id: "client-a".to_string(),
        job_id,
        command_version: 1,
        accepted: true,
        message: "accepted".to_string(),
        outputs: vec![CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&serde_json::json!({
                "type": "unsupported_command_version",
                "status": "rejected",
                "command_type": "shell_argv",
            }))
            .unwrap(),
            exit_code: Some(1),
            done: true,
        }],
    });

    assert_eq!(outcome.status, "rejected");
    assert_eq!(outcome.exit_code, Some(1));
    assert!(outcome.accepted);
    assert_eq!(
        aggregate_job_status_from_statuses(&[outcome.status], 1),
        "rejected"
    );
}

#[test]
fn gateway_failed_status_output_sets_target_message_from_agent_reason() {
    let job_id = Uuid::new_v4();
    let outcome = target_outcome_from_gateway(GatewayCommandDispatchResult {
        client_id: "client-a".to_string(),
        job_id,
        command_version: 1,
        accepted: true,
        message: "accepted".to_string(),
        outputs: vec![CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&serde_json::json!({
                "type": "file_write_text",
                "status": "failed",
                "reason": "permission denied",
            }))
            .unwrap(),
            exit_code: Some(13),
            done: true,
        }],
    });

    assert_eq!(outcome.status, "failed");
    assert_eq!(outcome.exit_code, Some(13));
    assert_eq!(outcome.message, "file_write_text: permission denied");
}

#[test]
fn busy_update_skip_uses_shared_reason_code() {
    let job_id = Uuid::new_v4();
    let outcome = busy_update_skip_outcome(
        job_id,
        &BusyUpdateSkip {
            client_id: "client-a".to_string(),
        },
        &JobCommand::AgentUpdateCheck {
            version_url: None,
            activate: false,
            restart_agent: false,
        },
    )
    .unwrap();

    assert_eq!(outcome.status, TARGET_STATUS_SKIPPED);
    assert_eq!(
        outcome.message,
        "busy_agent_active_jobs: target has another active job; update skipped"
    );
    let status: serde_json::Value = serde_json::from_slice(&outcome.outputs[0].data).unwrap();
    assert_eq!(status["type"], "busy_update_skipped");
    assert_eq!(status["reason"], "busy_agent_active_jobs");
}

#[test]
fn gateway_done_output_without_exit_code_maps_to_failed() {
    let job_id = Uuid::new_v4();
    let outcome = target_outcome_from_gateway(GatewayCommandDispatchResult {
        client_id: "client-a".to_string(),
        job_id,
        command_version: 1,
        accepted: true,
        message: "accepted".to_string(),
        outputs: vec![CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: Vec::new(),
            exit_code: None,
            done: true,
        }],
    });

    assert_eq!(outcome.status, "failed");
    assert_eq!(outcome.exit_code, None);
    assert_eq!(outcome.message, COMMAND_COMPLETED_WITHOUT_EXIT_CODE_MESSAGE);
}

#[test]
fn gateway_output_without_done_marker_stays_running() {
    let job_id = Uuid::new_v4();
    let outcome = target_outcome_from_gateway(GatewayCommandDispatchResult {
        client_id: "client-a".to_string(),
        job_id,
        command_version: 1,
        accepted: true,
        message: "accepted".to_string(),
        outputs: vec![CommandOutput {
            job_id,
            stream: OutputStream::Stdout,
            data: b"still running".to_vec(),
            exit_code: None,
            done: false,
        }],
    });

    assert_eq!(outcome.status, "running");
    assert_eq!(outcome.exit_code, None);
    assert_eq!(outcome.message, "accepted");
}

#[test]
fn protocol_mismatch_detects_unsupported_command_status_output() {
    let job_id = Uuid::new_v4();
    let outcome = TargetDispatchOutcome {
        status: "failed".to_string(),
        exit_code: Some(1),
        command_version: Some(2),
        accepted: true,
        message: "failed".to_string(),
        received_at: None,
        outputs: vec![CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&serde_json::json!({
                "type": "unsupported_command_version",
                "command_type": "shell_argv",
                "requested_version": 2,
                "supported_min_version": 1,
                "supported_max_version": 1,
            }))
            .unwrap(),
            exit_code: Some(1),
            done: true,
        }],
    };

    assert_eq!(
        protocol_mismatch_reason(
            &outcome,
            2,
            &JobCommand::Shell {
                argv: vec!["/bin/true".to_string()],
                pty: false,
            },
        )
        .as_deref(),
        Some("agent_rejected_unsupported_shell_argv_command_version")
    );
}

#[test]
fn protocol_mismatch_detects_lower_response_command_version() {
    let job_id = Uuid::new_v4();
    let outcome = TargetDispatchOutcome {
        status: "completed".to_string(),
        exit_code: Some(0),
        command_version: Some(1),
        accepted: true,
        message: "ok".to_string(),
        received_at: None,
        outputs: vec![CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&serde_json::json!({
                "type": "command_status",
                "command_version": 1,
            }))
            .unwrap(),
            exit_code: Some(0),
            done: true,
        }],
    };

    assert_eq!(
        protocol_mismatch_reason(
            &outcome,
            2,
            &JobCommand::Shell {
                argv: vec!["/bin/true".to_string()],
                pty: false,
            },
        )
        .as_deref(),
        Some("agent_returned_lower_command_version")
    );
}

#[test]
fn stale_target_message_keeps_failure_reason_explicit() {
    assert_eq!(
        stale_target_message(
            "unsupported_command_version",
            "agent_rejected_unsupported_shell_argv_command_version",
        ),
        "stale: agent_rejected_unsupported_shell_argv_command_version; unsupported_command_version",
    );
    assert_eq!(
        stale_target_message(
            "stale: prior detail",
            "agent_returned_lower_command_version"
        ),
        "stale: prior detail",
    );
}

#[test]
fn aggregate_job_status_uses_terminal_target_states() {
    assert_eq!(
        aggregate_job_status_from_statuses(&["completed".to_string(), "completed".to_string()], 2),
        "completed"
    );
    assert_eq!(
        aggregate_job_status_from_statuses(&["completed".to_string(), "failed".to_string()], 2),
        "partial_success"
    );
    assert_eq!(
        aggregate_job_status_from_statuses(&["failed".to_string(), "failed".to_string()], 2),
        "failed"
    );
    assert_eq!(
        aggregate_job_status_from_statuses(&["control_timeout".to_string()], 1),
        "control_timeout"
    );
    assert_eq!(
        aggregate_job_status_from_statuses(&["skipped".to_string()], 1),
        "skipped"
    );
    assert_eq!(
        aggregate_job_status_from_statuses(&["completed".to_string(), "skipped".to_string()], 2,),
        "partial_success"
    );
}

#[test]
fn capability_split_skips_only_explicit_unprivileged_root_network_targets() {
    let command = network_rollback_command();
    let targets = vec![
        "root-a".to_string(),
        "user-b".to_string(),
        "legacy-c".to_string(),
    ];
    let agents = vec![
        test_agent(
            "root-a",
            AgentCapabilitySnapshot {
                privilege_mode: AgentPrivilegeMode::Root,
                can_manage_runtime_tunnels: true,
                ..Default::default()
            },
        ),
        test_agent(
            "user-b",
            AgentCapabilitySnapshot {
                privilege_mode: AgentPrivilegeMode::Unprivileged,
                effective_uid: Some(1000),
                can_attempt_privileged_ops: true,
                ..Default::default()
            },
        ),
        test_agent("legacy-c", AgentCapabilitySnapshot::default()),
    ];

    let (dispatch, skipped) = split_targets_by_capability(&command, &targets, &agents, false);

    assert_eq!(dispatch, vec!["root-a".to_string(), "legacy-c".to_string()]);
    assert_eq!(skipped_client_ids(&skipped), vec!["user-b"]);
    assert_eq!(
        skipped[0].failure.reason,
        "target_agent_lacks_root_runtime_network_capability"
    );
}

#[test]
fn capability_split_allows_forced_unprivileged_best_effort() {
    let command = network_rollback_command();
    let targets = vec!["user-b".to_string()];
    let agents = vec![test_agent(
        "user-b",
        AgentCapabilitySnapshot {
            privilege_mode: AgentPrivilegeMode::Unprivileged,
            effective_uid: Some(1000),
            ..Default::default()
        },
    )];

    let (dispatch, skipped) = split_targets_by_capability(&command, &targets, &agents, true);

    assert_eq!(dispatch, targets);
    assert!(skipped.is_empty());
}

#[test]
fn capability_split_does_not_gate_non_network_commands() {
    let command = JobCommand::Shell {
        argv: vec!["uptime".to_string()],
        pty: false,
    };
    let targets = vec!["user-b".to_string()];
    let agents = vec![test_agent(
        "user-b",
        AgentCapabilitySnapshot {
            privilege_mode: AgentPrivilegeMode::Unprivileged,
            ..Default::default()
        },
    )];

    let (dispatch, skipped) = split_targets_by_capability(&command, &targets, &agents, false);

    assert_eq!(dispatch, targets);
    assert!(skipped.is_empty());
}

#[test]
fn capability_split_skips_process_starts_with_limits_when_target_cannot_apply_them() {
    let command = process_start_with_limits();
    let targets = vec![
        "root-a".to_string(),
        "user-b".to_string(),
        "legacy-c".to_string(),
    ];
    let agents = vec![
        test_agent(
            "root-a",
            AgentCapabilitySnapshot {
                privilege_mode: AgentPrivilegeMode::Root,
                can_apply_process_limits: true,
                ..Default::default()
            },
        ),
        test_agent(
            "user-b",
            AgentCapabilitySnapshot {
                privilege_mode: AgentPrivilegeMode::Unprivileged,
                effective_uid: Some(1000),
                can_attempt_privileged_ops: true,
                can_apply_process_limits: false,
                ..Default::default()
            },
        ),
        test_agent("legacy-c", AgentCapabilitySnapshot::default()),
    ];

    let (dispatch, skipped) = split_targets_by_capability(&command, &targets, &agents, false);

    assert_eq!(dispatch, vec!["root-a".to_string(), "legacy-c".to_string()]);
    assert_eq!(skipped_client_ids(&skipped), vec!["user-b"]);
    assert_eq!(
        skipped[0].failure.reason,
        "target_agent_lacks_process_limit_capability"
    );
}

#[test]
fn capability_split_allows_unlimited_process_starts_for_unprivileged_targets() {
    let command = JobCommand::ProcessStart {
        name: "worker".to_string(),
        argv: vec!["/bin/sleep".to_string(), "60".to_string()],
        cwd: None,
        env: Default::default(),
        policy: ProcessRunPolicy::default(),
        limits: ProcessResourceLimits::default(),
    };
    let targets = vec!["user-b".to_string()];
    let agents = vec![test_agent(
        "user-b",
        AgentCapabilitySnapshot {
            privilege_mode: AgentPrivilegeMode::Unprivileged,
            effective_uid: Some(1000),
            can_apply_process_limits: false,
            ..Default::default()
        },
    )];

    let (dispatch, skipped) = split_targets_by_capability(&command, &targets, &agents, false);

    assert_eq!(dispatch, targets);
    assert!(skipped.is_empty());
}

#[test]
fn capability_split_force_sends_process_limit_best_effort() {
    let command = process_start_with_limits();
    let targets = vec!["user-b".to_string()];
    let agents = vec![test_agent(
        "user-b",
        AgentCapabilitySnapshot {
            privilege_mode: AgentPrivilegeMode::Unprivileged,
            effective_uid: Some(1000),
            can_apply_process_limits: false,
            ..Default::default()
        },
    )];

    let (dispatch, skipped) = split_targets_by_capability(&command, &targets, &agents, true);

    assert_eq!(dispatch, targets);
    assert!(skipped.is_empty());
}

#[test]
fn capability_split_skips_agent_update_for_unprivileged_targets() {
    let command = JobCommand::AgentUpdateActivate {
        staged_sha256_hex: "ab".repeat(32),
        restart_agent: false,
    };
    let targets = vec![
        "root-a".to_string(),
        "user-b".to_string(),
        "legacy-c".to_string(),
    ];
    let agents = vec![
        test_agent(
            "root-a",
            AgentCapabilitySnapshot {
                privilege_mode: AgentPrivilegeMode::Root,
                can_attempt_privileged_ops: true,
                ..Default::default()
            },
        ),
        test_agent(
            "user-b",
            AgentCapabilitySnapshot {
                privilege_mode: AgentPrivilegeMode::Unprivileged,
                effective_uid: Some(1000),
                can_attempt_privileged_ops: false,
                ..Default::default()
            },
        ),
        test_agent("legacy-c", AgentCapabilitySnapshot::default()),
    ];

    let (dispatch, skipped) = split_targets_by_capability(&command, &targets, &agents, false);

    assert_eq!(dispatch, vec!["root-a".to_string(), "legacy-c".to_string()]);
    assert_eq!(skipped_client_ids(&skipped), vec!["user-b"]);
    assert_eq!(
        skipped[0].failure.reason,
        "target_agent_lacks_agent_update_capability"
    );
}

#[test]
fn capability_split_allows_config_read_for_unprivileged_targets() {
    let command = JobCommand::ConfigRead;
    let targets = vec!["user-b".to_string()];
    let agents = vec![test_agent(
        "user-b",
        AgentCapabilitySnapshot {
            privilege_mode: AgentPrivilegeMode::Unprivileged,
            effective_uid: Some(1000),
            can_attempt_privileged_ops: false,
            ..Default::default()
        },
    )];

    let (dispatch, skipped) = split_targets_by_capability(&command, &targets, &agents, false);

    assert_eq!(dispatch, targets);
    assert!(skipped.is_empty());
}

#[test]
fn capability_split_skips_restore_for_unprivileged_targets() {
    let command = restore_rollback_command();
    let targets = vec!["user-b".to_string()];
    let agents = vec![test_agent(
        "user-b",
        AgentCapabilitySnapshot {
            privilege_mode: AgentPrivilegeMode::Unprivileged,
            effective_uid: Some(1000),
            can_attempt_privileged_ops: false,
            ..Default::default()
        },
    )];

    let (dispatch, skipped) = split_targets_by_capability(&command, &targets, &agents, false);

    assert!(dispatch.is_empty());
    assert_eq!(skipped_client_ids(&skipped), vec!["user-b"]);
    assert_eq!(
        skipped[0].failure.reason,
        "target_agent_lacks_restore_capability"
    );
}

#[test]
fn capability_split_force_sends_update_and_restore_best_effort() {
    let targets = vec!["user-b".to_string()];
    let agents = vec![test_agent(
        "user-b",
        AgentCapabilitySnapshot {
            privilege_mode: AgentPrivilegeMode::Unprivileged,
            effective_uid: Some(1000),
            ..Default::default()
        },
    )];

    for command in [
        JobCommand::AgentUpdateRollback {
            rollback_sha256_hex: None,
        },
        restore_rollback_command(),
    ] {
        let (dispatch, skipped) = split_targets_by_capability(&command, &targets, &agents, true);
        assert_eq!(dispatch, targets);
        assert!(skipped.is_empty());
    }
}

#[test]
fn capability_degraded_outcome_records_operator_hint() {
    let (_dispatch, skipped) = split_targets_by_capability(
        &network_rollback_command(),
        &["user-b".to_string()],
        &[test_agent(
            "user-b",
            AgentCapabilitySnapshot {
                privilege_mode: AgentPrivilegeMode::Unprivileged,
                effective_uid: Some(1000),
                ..Default::default()
            },
        )],
        false,
    );
    let outcome =
        capability_degraded_outcome(Uuid::new_v4(), &skipped[0], &network_rollback_command())
            .unwrap();
    let status: serde_json::Value = serde_json::from_slice(&outcome.outputs[0].data).unwrap();

    assert_eq!(outcome.status, "skipped");
    assert_eq!(outcome.exit_code, Some(0));
    assert_eq!(outcome.outputs[0].exit_code, Some(0));
    assert!(!outcome.accepted);
    assert_eq!(
        status["reason"],
        "target_agent_lacks_root_runtime_network_capability"
    );
    assert!(status["hint"]
        .as_str()
        .unwrap()
        .contains("force_unprivileged"));
}

#[test]
fn job_timeout_accepts_configured_max_above_default() {
    assert_eq!(
        effective_job_timeout_secs(Some(7_200), 7_200).unwrap(),
        7_200
    );
}

#[test]
fn omitted_job_timeout_uses_default_agent_timeout() {
    assert_eq!(
        effective_job_timeout_secs(None, 7_200).unwrap(),
        DEFAULT_MAX_COMMAND_TIMEOUT_SECS
    );
}

#[test]
fn job_timeout_rejects_above_configured_max() {
    let error = effective_job_timeout_secs(Some(7_201), 7_200).unwrap_err();

    assert_eq!(error.code, "command_timeout_exceeds_configured_max");
}

#[test]
fn explicit_job_timeout_overrides_agent_advertised_default() {
    let agents = vec![test_agent(
        "short-default",
        AgentCapabilitySnapshot {
            command_timeout_secs: 12,
            ..AgentCapabilitySnapshot::default()
        },
    )];

    assert_eq!(effective_job_timeout_secs(Some(90), 600).unwrap(), 90);
    assert_eq!(agents[0].capabilities.command_timeout_secs, 12);
}

#[test]
fn job_selector_expression_is_audit_text_not_target_parser_input() {
    validate_job_audit_selector("operator note: prod wave 1 (ticket OPS-123)").unwrap();
    assert_eq!(
        validate_job_audit_selector("bad\nselector")
            .unwrap_err()
            .code,
        "invalid_selector_expression"
    );
}

fn skipped_client_ids(skipped: &[CapabilitySkip]) -> Vec<&str> {
    skipped.iter().map(|skip| skip.client_id.as_str()).collect()
}

fn test_agent(id: &str, capabilities: AgentCapabilitySnapshot) -> AgentView {
    AgentView {
        id: id.to_string(),
        display_name: id.to_string(),
        status: "online".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities,
    }
}

fn network_rollback_command() -> JobCommand {
    JobCommand::NetworkRollback {
        plan: Box::new(
            plan_tunnel(&TunnelPlanInput {
                name: "edge-a-edge-b".to_string(),
                interface_name: "tunab".to_string(),
                kind: TunnelKind::Gre,
                runtime_control: Default::default(),
                runtime_topology: Default::default(),
                left_client_id: "root-a".to_string(),
                right_client_id: "user-b".to_string(),
                left_underlay: "198.51.100.10".to_string(),
                right_underlay: "203.0.113.20".to_string(),
                address_pool_cidr: "10.255.0.0/30".to_string(),
                reserved_addresses: Vec::new(),
                ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
                    left: "10.255.0.0".to_string(),
                    right: "10.255.0.1".to_string(),
                    prefix_len: 31,
                }),
                ipv6_address_pool_cidr: None,
                ipv6_tunnel: None,
                latency_primary_family: Default::default(),
                bandwidth: BandwidthTier::M100,
                latency_ms: 18.0,
                packet_loss_ratio: 0.0,
                preference: 1.0,
                ospf_policy: OspfCostPolicy::default(),
            })
            .unwrap(),
        ),
        side: TunnelEndpointSide::Left,
    }
}

fn process_start_with_limits() -> JobCommand {
    JobCommand::ProcessStart {
        name: "worker".to_string(),
        argv: vec!["/bin/sleep".to_string(), "60".to_string()],
        cwd: None,
        env: Default::default(),
        policy: ProcessRunPolicy::default(),
        limits: ProcessResourceLimits {
            memory_max_bytes: Some(128 * 1024 * 1024),
            pids_max: Some(32),
            open_files_max: Some(256),
            cpu_shares: Some(1024),
            no_new_privileges: true,
        },
    }
}

fn restore_rollback_command() -> JobCommand {
    JobCommand::RestoreRollback {
        source_restore_job_id: Uuid::new_v4(),
        restored_files: vec![vpsman_common::RestoreRollbackFile {
            archive_path: "/etc/hostname".to_string(),
            destination_path: "/restore/etc/hostname".to_string(),
            rollback_path: Some("/restore/etc/.vpsman-restore-hostname.bak".to_string()),
            restored_size_bytes: 8,
            restored_sha256_hex: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_string(),
        }],
    }
}
