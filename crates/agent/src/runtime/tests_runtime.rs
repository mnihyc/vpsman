use super::*;

#[test]
fn ping_targets_bound_telemetry_publish_cadence_to_one_minute() {
    let mut network = vpsman_common::AgentNetworkConfig::default();
    network.ping_targets.push(vpsman_common::AgentPingTarget {
        id: uuid::Uuid::new_v4().to_string(),
        generation: 1,
        name: "gateway".to_string(),
        host: "192.0.2.1".to_string(),
        kind: vpsman_common::AgentPingProbeKind::Icmp,
        port: None,
    });
    assert_eq!(effective_telemetry_interval_secs(3_600, &network), 60);
    assert_eq!(effective_telemetry_interval_secs(30, &network), 30);

    network.ping_targets.clear();
    assert_eq!(effective_telemetry_interval_secs(3_600, &network), 3_600);
    assert_eq!(effective_telemetry_interval_secs(1, &network), 5);
}

#[test]
fn enabled_runtime_status_and_latency_bound_telemetry_publish_cadence() {
    let mut network = vpsman_common::AgentNetworkConfig {
        runtime_status_telemetry_interval_secs: 300,
        latency_monitoring_interval_secs: 45,
        runtime_status_telemetry_plans: vec![runtime_sync_test_telemetry_plan(
            runtime_sync_test_plan("203.0.113.20", "10.255.0.0", "10.255.0.1"),
        )],
        ..Default::default()
    };
    assert_eq!(effective_telemetry_interval_secs(3_600, &network), 45);

    network.runtime_status_telemetry_plans[0].latency_monitoring_enabled = false;
    assert_eq!(effective_telemetry_interval_secs(3_600, &network), 300);

    network.runtime_status_telemetry_enabled = false;
    assert_eq!(effective_telemetry_interval_secs(3_600, &network), 3_600);
}

#[test]
fn configured_os_release_read_failure_is_explicit() {
    let path = std::env::temp_dir().join(format!(
        "vpsman-missing-os-release-{}",
        uuid::Uuid::new_v4()
    ));

    let error = configured_os_release(path.to_str()).unwrap_err();

    assert!(error
        .to_string()
        .contains("failed to read configured OS release file"));
    assert!(format!("{error:#}").contains("No such file or directory"));
}

#[test]
fn configured_os_release_rejects_blank_content() {
    let path = std::env::temp_dir().join(format!("vpsman-os-release-{}", uuid::Uuid::new_v4()));
    std::fs::write(&path, " \n\t").unwrap();

    let error = configured_os_release(path.to_str()).unwrap_err();

    assert!(error.to_string().contains("OS release file"));
    assert!(error.to_string().contains("is empty"));
    std::fs::remove_file(path).ok();
}

#[test]
fn configured_os_release_preserves_valid_content_and_optional_absence() {
    let path = std::env::temp_dir().join(format!("vpsman-os-release-{}", uuid::Uuid::new_v4()));
    let contents = "NAME=Example Linux\nVERSION_ID=1\n";
    std::fs::write(&path, contents).unwrap();

    assert_eq!(
        configured_os_release(path.to_str()).unwrap(),
        contents.to_string()
    );
    assert_eq!(configured_os_release(None).unwrap(), "");
    std::fs::remove_file(path).ok();
}

#[test]
fn recent_command_cache_keeps_payload_hash_and_evicts_oldest() {
    let first = uuid::Uuid::new_v4();
    let second = uuid::Uuid::new_v4();
    let third = uuid::Uuid::new_v4();
    let mut cache = RecentCommandCache {
        max_entries: 2,
        ..RecentCommandCache::default()
    };

    remember_recent_command_outputs(
        &mut cache,
        first,
        "hash-a".to_string(),
        &[test_status_output(first)],
    );
    remember_recent_command_outputs(
        &mut cache,
        second,
        "hash-b".to_string(),
        &[test_status_output(second)],
    );
    assert_eq!(
        cache.get(first).map(|entry| entry.payload_hash.as_str()),
        Some("hash-a")
    );
    assert_eq!(
        cache.get(second).map(|entry| entry.payload_hash.as_str()),
        Some("hash-b")
    );
    assert_eq!(cache.get(first).map(|entry| entry.outputs.len()), Some(1));

    remember_recent_command_outputs(
        &mut cache,
        third,
        "hash-c".to_string(),
        &[test_status_output(third)],
    );
    assert!(cache.get(first).is_none());
    assert_eq!(
        cache.get(second).map(|entry| entry.payload_hash.as_str()),
        Some("hash-b")
    );
    assert_eq!(
        cache.get(third).map(|entry| entry.payload_hash.as_str()),
        Some("hash-c")
    );
}

#[test]
fn recent_command_cache_marks_oversized_replay_unavailable() {
    let job_id = uuid::Uuid::new_v4();
    let mut cache = RecentCommandCache {
        max_entry_output_bytes: 4,
        ..RecentCommandCache::default()
    };

    let outputs = sequenced_outputs_starting_at(
        0,
        &[CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: b"too-large".to_vec(),
            exit_code: Some(0),
            done: true,
        }],
    );
    cache.remember(
        job_id,
        "hash-a".to_string(),
        outputs.clone(),
        terminal_replay_output_from(&outputs),
        false,
    );

    let entry = cache.get(job_id).expect("recent entry retained");
    assert!(entry.truncated);
    assert!(entry.outputs.is_empty());
    assert_eq!(
        entry
            .terminal_output
            .as_ref()
            .and_then(|output| output.output.exit_code),
        Some(75)
    );
    let status: serde_json::Value =
        serde_json::from_slice(&entry.terminal_output.as_ref().unwrap().output.data).unwrap();
    assert_eq!(status["type"], "duplicate_job_replay_unavailable");
    assert_eq!(status["status"], "failed");
}

#[tokio::test]
async fn active_command_keeps_pending_outputs_until_flushed() {
    let job_id = uuid::Uuid::new_v4();
    let mut active = test_active_command(job_id);

    enqueue_active_command_output(&mut active, test_status_output(job_id));
    enqueue_active_command_output(&mut active, test_status_output(job_id));

    assert_eq!(active.next_output_seq, 2);
    assert_eq!(
        active
            .pending_outputs
            .iter()
            .map(|output| output.seq)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(active.replay_outputs.len(), 2);
    active.finished = true;

    let mut active_commands = HashMap::from([(job_id, active)]);
    remove_finished_flushed_commands(&mut active_commands);
    assert!(active_commands.contains_key(&job_id));

    active_commands
        .get_mut(&job_id)
        .unwrap()
        .pending_outputs
        .clear();
    remove_finished_flushed_commands(&mut active_commands);
    assert!(!active_commands.contains_key(&job_id));
}

fn test_active_command(job_id: uuid::Uuid) -> ActiveCommand {
    ActiveCommand {
        payload_hash: "payload-hash".to_string(),
        cancel_token: CommandCancelToken::default(),
        command_version: vpsman_common::MIN_COMMAND_PROTOCOL_VERSION,
        safety: JobCommandSafety::Read,
        stream_id: 1,
        replay_outputs: Vec::new(),
        terminal_output: None,
        replay_output_bytes: 0,
        replay_truncated: false,
        pending_outputs: VecDeque::new(),
        next_output_seq: 0,
        finished: false,
        _task: tokio::spawn(async move {
            let _ = job_id;
        }),
    }
}

fn test_status_output(job_id: uuid::Uuid) -> CommandOutput {
    CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: b"ok".to_vec(),
        exit_code: Some(0),
        done: true,
    }
}

#[test]
fn command_payload_hash_changes_with_command_shape() {
    let left = JobCommand::Shell {
        argv: vec!["/bin/true".to_string()],
        pty: false,
    };
    let right = JobCommand::Shell {
        argv: vec!["/bin/false".to_string()],
        pty: false,
    };

    assert_ne!(
        command_payload_hash(&left).unwrap(),
        command_payload_hash(&right).unwrap()
    );
}

#[test]
fn command_protocol_rejects_future_non_update_commands() {
    let command = JobCommand::Shell {
        argv: vec!["/bin/true".to_string()],
        pty: false,
    };
    let command_version = job_command_protocol_version(&command);

    assert!(command_supports_requested_protocol(
        &command,
        command_version
    ));
    assert!(!command_supports_requested_protocol(
        &command,
        command_version + 1
    ));
    assert!(!command_supports_requested_protocol(&command, 0));
}

#[test]
fn command_protocol_rejects_future_update_commands() {
    let command = JobCommand::AgentUpdateCheck {
        version_url: None,
        activate: true,
        restart_agent: true,
    };
    let command_version = job_command_protocol_version(&command);

    assert!(!command_supports_requested_protocol(
        &command,
        command_version + 10
    ));
    assert!(command_supports_requested_protocol(
        &command,
        command_version
    ));
    assert!(!command_supports_requested_protocol(&command, 0));
}

#[test]
fn new_host_commands_require_their_exact_protocol_generation() {
    let command = JobCommand::StorageInventory {
        include_pseudo_mounts: false,
        limit: 512,
    };
    let command_version = job_command_protocol_version(&command);

    assert_eq!(
        command_version,
        job_command_min_supported_protocol_version(&command)
    );
    assert!(command_version > vpsman_common::MIN_COMMAND_PROTOCOL_VERSION);
    assert!(command_supports_requested_protocol(
        &command,
        command_version
    ));
    assert!(!command_supports_requested_protocol(
        &command,
        vpsman_common::MIN_COMMAND_PROTOCOL_VERSION
    ));
    assert!(!command_supports_requested_protocol(
        &command,
        command_version + 1
    ));
}

#[test]
fn command_field_drift_becomes_a_terminal_rejection() {
    let job_id = uuid::Uuid::new_v4();
    let payload = serde_json::to_vec(&serde_json::json!({
        "job_id": job_id,
        "command_version": 1,
        "command": {
            "type": "agent_update",
            "artifact_url": "https://updates.example/vpsman-agent",
            "sha256_hex": "ab".repeat(32),
            "future_optional_policy": "must-use-a-new-command-variant"
        },
        "max_timeout_secs": 30,
        "future_request_metadata": { "trace": "ignored" }
    }))
    .unwrap();

    let decoded = decode_job_request_payload(&payload).unwrap();
    let DecodedJobRequest::Unsupported(request) = decoded else {
        panic!("unknown command fields must be rejected without dropping the session");
    };
    assert_eq!(request.job_id, job_id);
    assert_eq!(request.command_type, "agent_update");

    let output = unsupported_command_shape_output(&request).unwrap();
    assert_eq!(output.exit_code, Some(78));
    assert!(output.done);
}

#[test]
fn unknown_command_shape_becomes_a_terminal_rejection() {
    let job_id = uuid::Uuid::new_v4();
    let payload = serde_json::to_vec(&serde_json::json!({
        "job_id": job_id,
        "command_version": 2,
        "command": {
            "type": "future_atomic_operation",
            "required_semantics": true
        },
        "max_timeout_secs": 30
    }))
    .unwrap();

    let decoded = decode_job_request_payload(&payload).unwrap();
    let DecodedJobRequest::Unsupported(request) = decoded else {
        panic!("unknown commands must be rejected without decoding the session frame");
    };
    assert_eq!(request.job_id, job_id);
    assert_eq!(request.command_type, "future_atomic_operation");

    let output = unsupported_command_shape_output(&request).unwrap();
    assert_eq!(output.exit_code, Some(78));
    assert!(output.done);
    let status: serde_json::Value = serde_json::from_slice(&output.data).unwrap();
    assert_eq!(status["type"], "unsupported_command_version");
    assert_eq!(status["status"], "rejected");
    assert_eq!(
        status["reason"],
        "agent_binary_does_not_support_command_shape"
    );
}

#[tokio::test]
async fn configured_runtime_reconcile_runs_saved_telemetry_plans() {
    let root = std::env::temp_dir().join(format!(
        "vpsman-configured-runtime-reconcile-{}",
        uuid::Uuid::new_v4()
    ));
    tokio::fs::create_dir_all(&root).await.unwrap();
    let plan = vpsman_common::plan_tunnel(&vpsman_common::TunnelPlanInput {
        name: "left-right".to_string(),
        interface_name: "tunlr".to_string(),
        kind: vpsman_common::TunnelKind::Gre,
        runtime_control: Default::default(),
        runtime_topology: Default::default(),
        left_client_id: "left-a".to_string(),
        right_client_id: "right-b".to_string(),
        left_remote_underlay: "198.51.100.10".to_string(),
        right_remote_underlay: "203.0.113.20".to_string(),
        left_local_underlay: None,
        right_local_underlay: None,
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
        bandwidth_mbps: 100,
        left_mtu: Some(1476),
        right_mtu: Some(1476),
        ospf: None,
    })
    .unwrap();
    let config = AgentConfig {
        client_id: "left-a".to_string(),
        network: vpsman_common::AgentNetworkConfig {
            apply_enabled: true,
            runtime_reconcile_enabled: true,
            root_dir: root.to_string_lossy().to_string(),
            runtime_ip_argv: vec!["/bin/echo".to_string()],
            runtime_tc_argv: vec!["/bin/echo".to_string()],
            runtime_unprivileged_mutation_policy:
                vpsman_common::AgentRuntimeUnprivilegedMutationPolicy::TryAll,
            runtime_status_telemetry_plans: vec![vpsman_common::AgentRuntimeStatusTelemetryPlan {
                plan_id: Some("plan-a".to_string()),
                topology_identity_hash: "0".repeat(64),
                endpoint_side: vpsman_common::TunnelEndpointSide::Left,
                plan,
                builtin_credentials: None,
                runtime_adapter: None,
                latency_monitoring_enabled: true,
            }],
            ..Default::default()
        },
        ..AgentConfig::default()
    };

    let report = reconcile_configured_runtime_tunnels(&config, "test").await;

    assert_eq!(report["status"], "completed");
    assert_eq!(report["total"], 1);
    assert_eq!(report["converged"], 1);
    assert_eq!(report["tunnels"][0]["plan_id"], "plan-a");
    assert_eq!(report["tunnels"][0]["interface"], "tunlr");
}

#[tokio::test]
async fn runtime_config_sync_returns_applied_candidate_without_mutating_source() {
    let base = AgentConfig {
        client_id: "client-a".to_string(),
        display_name: "old-name".to_string(),
        telemetry_interval_secs: 15,
        ..AgentConfig::default()
    };
    let desired = AgentRuntimeConfig {
        version: 9,
        display_name: "new-name".to_string(),
        telemetry_interval_secs: 30,
        ..AgentRuntimeConfig::default()
    };

    let result = apply_runtime_config_sync(
        uuid::Uuid::new_v4(),
        &base,
        &desired,
        9,
        "test-success",
        CommandCancelToken::default(),
    )
    .await
    .unwrap();

    assert_eq!(base.display_name, "old-name");
    assert_eq!(result.outputs[0].exit_code, Some(0));
    let applied = result.applied_config.expect("sync should apply");
    assert_eq!(applied.display_name, "new-name");
    assert_eq!(applied.telemetry_interval_secs, 30);
    assert_eq!(
        result
            .accepted_runtime_config
            .expect("accepted snapshot should be persisted")
            .version,
        9
    );
}

#[test]
fn runtime_config_generation_is_monotonic_and_exact_replays_are_content_bound() {
    let current = AgentConfig {
        display_name: "current".to_string(),
        ..AgentConfig::default()
    };
    let same = AgentRuntimeConfig::from_agent_config(9, &current);
    let different = AgentRuntimeConfig {
        version: 9,
        display_name: "obsolete".to_string(),
        ..AgentRuntimeConfig::from_agent_config(9, &current)
    };

    assert!(runtime_config_snapshot_is_stale(Some(10), &current, 9, &same).unwrap());
    assert!(runtime_config_snapshot_is_stale(Some(10), &current, 9, &different).unwrap());
    assert!(!runtime_config_snapshot_is_stale(Some(10), &current, 10, &same).unwrap());
    assert!(runtime_config_snapshot_is_stale(Some(10), &current, 10, &different).unwrap());
    assert!(!runtime_config_snapshot_is_stale(Some(10), &current, 11, &different).unwrap());
    assert!(!runtime_config_snapshot_is_stale(None, &current, 1, &different).unwrap());
}

#[test]
fn authoritative_reconnect_is_the_only_full_reconcile_reason() {
    assert!(runtime_config_reason_requires_full_reconcile(
        "agent_reconnect_authoritative_sync"
    ));
    assert!(!runtime_config_reason_requires_full_reconcile(
        "agent_reconnect_runtime_config_check"
    ));
    assert!(!runtime_config_reason_requires_full_reconcile(
        "agent_reconnect_port_forwarding_sync"
    ));
    assert!(runtime_config_reason_requires_full_reconcile(
        "agent_reconnect_authoritative_port_forwarding_sync"
    ));
    assert!(runtime_config_reason_requires_port_forwarding_table_access(
        "agent_reconnect_port_forwarding_sync"
    ));
    assert!(
        !runtime_config_reason_requires_port_forwarding_table_access(
            "agent_reconnect_authoritative_sync"
        )
    );
    assert!(runtime_config_reason_requires_port_forwarding_table_access(
        "agent_reconnect_authoritative_port_forwarding_sync"
    ));
    assert!(runtime_config_reason_requires_tunnel_reconcile(
        "agent_reconnect_runtime_tunnels_sync"
    ));
    assert!(runtime_config_reason_requires_tunnel_reconcile(
        "agent_reconnect_authoritative_sync"
    ));
    assert!(!runtime_config_reason_requires_tunnel_reconcile(
        "agent_reconnect_port_forwarding_sync"
    ));
    assert!(!port_forwarding_table_access_required(
        false,
        false,
        "agent_reconnect_authoritative_sync"
    ));
    assert!(port_forwarding_table_access_required(
        false,
        false,
        "agent_reconnect_authoritative_port_forwarding_sync"
    ));
    assert!(port_forwarding_table_access_required(
        true,
        false,
        "unrelated_config_update"
    ));
}

#[test]
fn reconnect_reconciles_only_repairable_owned_table_states() {
    for status in [
        PortForwardRuntimeStatus::Absent,
        PortForwardRuntimeStatus::Applied,
        PortForwardRuntimeStatus::Unsupported,
        PortForwardRuntimeStatus::Unknown,
    ] {
        assert!(!port_forwarding_snapshot_requires_reconnect_sync(
            &PortForwardRuntimeSnapshot {
                status,
                ..PortForwardRuntimeSnapshot::default()
            }
        ));
    }

    assert!(port_forwarding_snapshot_requires_reconnect_sync(
        &PortForwardRuntimeSnapshot {
            status: PortForwardRuntimeStatus::Drifted,
            ..PortForwardRuntimeSnapshot::default()
        }
    ));
    assert!(port_forwarding_snapshot_requires_reconnect_sync(
        &PortForwardRuntimeSnapshot {
            status: PortForwardRuntimeStatus::Failed,
            error_code: Some("inspection_failed".to_string()),
            ..PortForwardRuntimeSnapshot::default()
        }
    ));
    assert!(!port_forwarding_snapshot_requires_reconnect_sync(
        &PortForwardRuntimeSnapshot {
            status: PortForwardRuntimeStatus::Failed,
            error_code: Some("table_ownership_conflict".to_string()),
            ..PortForwardRuntimeSnapshot::default()
        }
    ));
}

#[test]
fn successful_forwarding_is_retained_when_an_independent_tunnel_change_fails() {
    let current = AgentConfig::default();
    let mut candidate = current.clone();
    candidate.network.port_forwarding.desired_hash = "new-forwarding-state".to_string();
    candidate
        .network
        .runtime_status_telemetry_plans
        .push(runtime_sync_test_telemetry_plan(runtime_sync_test_plan(
            "203.0.113.20",
            "10.255.0.0",
            "10.255.0.1",
        )));

    let (accepted, scope) =
        accepted_config_after_network_sync(&current, &candidate, false, true, true, true, false);

    let accepted = accepted.expect("successful forwarding should be retained");
    assert_eq!(scope, "port_forwarding");
    assert_eq!(
        accepted.network.port_forwarding.desired_hash,
        "new-forwarding-state"
    );
    assert!(accepted.network.runtime_status_telemetry_plans.is_empty());
}

#[tokio::test]
async fn runtime_config_sync_skips_unchanged_tunnel_commands() {
    let plan = runtime_sync_test_telemetry_plan(runtime_sync_test_plan(
        "203.0.113.20",
        "10.255.0.0",
        "10.255.0.1",
    ));
    let base = AgentConfig {
        client_id: "left-a".to_string(),
        display_name: "before".to_string(),
        network: vpsman_common::AgentNetworkConfig {
            apply_enabled: true,
            runtime_reconcile_enabled: true,
            runtime_ip_argv: vec!["/bin/false".to_string()],
            runtime_tc_argv: vec!["/bin/false".to_string()],
            runtime_status_telemetry_plans: vec![plan],
            ..Default::default()
        },
        ..AgentConfig::default()
    };
    let mut desired = AgentRuntimeConfig::from_agent_config(10, &base);
    desired.display_name = "after".to_string();

    let result = apply_runtime_config_sync(
        uuid::Uuid::new_v4(),
        &base,
        &desired,
        10,
        "unrelated_config_update",
        CommandCancelToken::default(),
    )
    .await
    .unwrap();

    let body: serde_json::Value = serde_json::from_slice(&result.outputs[0].data).unwrap();
    assert_eq!(body["status"], "applied");
    assert_eq!(body["reconcile"]["status"], "unchanged");
    assert_eq!(body["port_forwarding"]["status"], "unchanged");
    assert_eq!(
        result
            .applied_config
            .expect("config should apply")
            .display_name,
        "after"
    );
}

#[test]
fn runtime_tunnel_identity_allows_in_place_policy_changes() {
    let mut changed_cost = runtime_sync_test_telemetry_plan(runtime_sync_test_plan(
        "203.0.113.20",
        "10.255.0.0",
        "10.255.0.1",
    ));
    changed_cost.plan.bandwidth_mbps = changed_cost.plan.bandwidth_mbps.saturating_add(10);
    let baseline = runtime_sync_test_telemetry_plan(runtime_sync_test_plan(
        "203.0.113.20",
        "10.255.0.0",
        "10.255.0.1",
    ));

    assert!(runtime_tunnel_identity_matches(&baseline, &changed_cost));
}

#[test]
fn runtime_tunnel_identity_detects_immutable_plan_changes() {
    let baseline = runtime_sync_test_telemetry_plan(runtime_sync_test_plan(
        "203.0.113.20",
        "10.255.0.0",
        "10.255.0.1",
    ));
    let changed_underlay = runtime_sync_test_telemetry_plan(runtime_sync_test_plan(
        "203.0.113.99",
        "10.255.0.0",
        "10.255.0.1",
    ));
    let changed_address = runtime_sync_test_telemetry_plan(runtime_sync_test_plan(
        "203.0.113.20",
        "10.255.0.2",
        "10.255.0.3",
    ));

    assert!(!runtime_tunnel_identity_matches(
        &baseline,
        &changed_underlay
    ));
    assert!(!runtime_tunnel_identity_matches(
        &baseline,
        &changed_address
    ));
}

#[test]
fn runtime_tunnel_identity_keeps_builtin_wireguard_underlay_edits_in_place() {
    let mut baseline = runtime_sync_test_telemetry_plan(runtime_sync_test_plan(
        "203.0.113.20",
        "10.255.0.0",
        "10.255.0.1",
    ));
    baseline.plan.kind = vpsman_common::TunnelKind::Wireguard;
    let mut changed_underlay = baseline.clone();
    changed_underlay.plan.right_remote_underlay = "203.0.113.99".to_string();
    let mut renamed = baseline.clone();
    renamed.plan.name = "renamed-plan".to_string();

    assert!(runtime_tunnel_identity_matches(
        &baseline,
        &changed_underlay
    ));
    assert!(runtime_tunnel_identity_matches(&baseline, &renamed));
}

#[test]
fn builtin_driver_versions_are_parsed_from_their_own_markers() {
    assert_eq!(
        parse_marked_version("ip utility, iproute2-5.15.0, libbpf 0.5.0", "iproute2-"),
        Some(semver::Version::new(5, 15, 0))
    );
    assert_eq!(
        parse_marked_version(
            "wireguard-tools v1.0.20210914 - https://www.wireguard.com/",
            "wireguard-tools v"
        ),
        Some(semver::Version::new(1, 0, 20210914))
    );
    assert_eq!(
        parse_marked_version("OpenVPN 2.4.12 x86_64-pc-linux-gnu", "OpenVPN "),
        Some(semver::Version::new(2, 4, 12))
    );
}

#[tokio::test]
async fn builtin_driver_capability_requires_its_own_successful_marker() {
    let unmarked = probe_builtin_driver(
        &["/bin/true".to_string()],
        &[],
        Some("wireguard-tools v"),
        None,
    )
    .await;
    assert!(!unmarked.available);
    assert_eq!(
        unmarked.unavailable_reason.as_deref(),
        Some("version probe did not identify the configured driver")
    );

    let identified = probe_builtin_driver(
        &[
            "/bin/sh".to_string(),
            "-c".to_string(),
            "printf 'wireguard-tools v1.0.20210914\\n'".to_string(),
        ],
        &[],
        Some("wireguard-tools v"),
        None,
    )
    .await;
    assert!(identified.available);
    assert_eq!(identified.version.as_deref(), Some("1.0.20210914"));
}

#[tokio::test]
async fn runtime_config_sync_recreates_tunnel_when_plan_identity_changes() {
    let root = std::env::temp_dir().join(format!(
        "vpsman-runtime-sync-recreate-{}",
        uuid::Uuid::new_v4()
    ));
    tokio::fs::create_dir_all(&root).await.unwrap();
    let old_plan = runtime_sync_test_telemetry_plan(runtime_sync_test_plan(
        "203.0.113.20",
        "10.255.0.0",
        "10.255.0.1",
    ));
    let new_plan = runtime_sync_test_telemetry_plan(runtime_sync_test_plan(
        "203.0.113.99",
        "10.255.0.0",
        "10.255.0.1",
    ));
    let base = AgentConfig {
        client_id: "left-a".to_string(),
        network: vpsman_common::AgentNetworkConfig {
            apply_enabled: true,
            runtime_reconcile_enabled: true,
            root_dir: root.to_string_lossy().to_string(),
            runtime_ip_argv: vec!["/bin/echo".to_string()],
            runtime_tc_argv: vec!["/bin/echo".to_string()],
            runtime_unprivileged_mutation_policy:
                vpsman_common::AgentRuntimeUnprivilegedMutationPolicy::TryAll,
            runtime_status_telemetry_plans: vec![old_plan],
            ..Default::default()
        },
        ..AgentConfig::default()
    };
    let mut desired = AgentRuntimeConfig::from_agent_config(12, &base);
    desired.network.runtime_status_telemetry_plans = vec![new_plan];

    let result = apply_runtime_config_sync(
        uuid::Uuid::new_v4(),
        &base,
        &desired,
        12,
        "test-plan-identity-change",
        CommandCancelToken::default(),
    )
    .await
    .unwrap();

    assert_eq!(result.outputs[0].exit_code, Some(0));
    let body: serde_json::Value = serde_json::from_slice(&result.outputs[0].data).unwrap();
    assert_eq!(body["status"], "applied");
    assert_eq!(body["removed_tunnel_count"], 1);
    assert_eq!(body["removals"][0]["plan_id"], "plan-a");
    assert_eq!(body["reconcile"]["total"], 1);
    assert_eq!(
        result
            .applied_config
            .expect("identity change sync should apply")
            .network
            .runtime_status_telemetry_plans[0]
            .plan
            .right_remote_underlay,
        "203.0.113.99"
    );
}

#[tokio::test]
async fn runtime_config_sync_removes_omitted_tunnel_plan() {
    let root = std::env::temp_dir().join(format!(
        "vpsman-runtime-sync-remove-{}",
        uuid::Uuid::new_v4()
    ));
    tokio::fs::create_dir_all(root.join("sys/class/net/tunlr"))
        .await
        .unwrap();
    let plan = runtime_sync_test_telemetry_plan(runtime_sync_test_plan(
        "203.0.113.20",
        "10.255.0.0",
        "10.255.0.1",
    ));
    let base = AgentConfig {
        client_id: "left-a".to_string(),
        network: vpsman_common::AgentNetworkConfig {
            apply_enabled: true,
            runtime_reconcile_enabled: true,
            root_dir: root.to_string_lossy().to_string(),
            runtime_ip_argv: vec!["/bin/echo".to_string()],
            runtime_tc_argv: vec!["/bin/echo".to_string()],
            runtime_unprivileged_mutation_policy:
                vpsman_common::AgentRuntimeUnprivilegedMutationPolicy::TryAll,
            runtime_status_telemetry_plans: vec![plan],
            ..Default::default()
        },
        ..AgentConfig::default()
    };
    let mut desired = AgentRuntimeConfig::from_agent_config(13, &base);
    desired.network.runtime_status_telemetry_plans.clear();

    let result = apply_runtime_config_sync(
        uuid::Uuid::new_v4(),
        &base,
        &desired,
        13,
        "test-plan-disabled",
        CommandCancelToken::default(),
    )
    .await
    .unwrap();

    assert_eq!(result.outputs[0].exit_code, Some(0));
    let body: serde_json::Value = serde_json::from_slice(&result.outputs[0].data).unwrap();
    assert_eq!(body["status"], "applied");
    assert_eq!(body["removed_tunnel_count"], 1);
    assert_eq!(body["removals"][0]["plan_id"], "plan-a");
    assert_eq!(body["removals"][0]["status"], "removed");
    assert_eq!(body["reconcile"]["total"], 0);
    assert!(result
        .applied_config
        .expect("disabled plan sync should apply")
        .network
        .runtime_status_telemetry_plans
        .is_empty());
}

#[tokio::test]
async fn runtime_config_sync_preserves_omitted_plan_when_cleanup_is_blocked() {
    let plan = runtime_sync_test_telemetry_plan(runtime_sync_test_plan(
        "203.0.113.20",
        "10.255.0.0",
        "10.255.0.1",
    ));
    let base = AgentConfig {
        client_id: "left-a".to_string(),
        network: vpsman_common::AgentNetworkConfig {
            apply_enabled: false,
            runtime_reconcile_enabled: false,
            runtime_status_telemetry_plans: vec![plan],
            ..Default::default()
        },
        ..AgentConfig::default()
    };
    let mut desired = AgentRuntimeConfig::from_agent_config(14, &base);
    desired.network.runtime_status_telemetry_plans.clear();

    let result = apply_runtime_config_sync(
        uuid::Uuid::new_v4(),
        &base,
        &desired,
        14,
        "test-cleanup-blocked",
        CommandCancelToken::default(),
    )
    .await
    .unwrap();

    assert_eq!(result.outputs[0].exit_code, Some(1));
    let body: serde_json::Value = serde_json::from_slice(&result.outputs[0].data).unwrap();
    assert_eq!(body["status"], "failed");
    assert_eq!(body["removals"][0]["status"], "skipped");
    assert_eq!(body["removals"][0]["reason"], "runtime_reconcile_disabled");
    assert!(result.applied_config.is_none());
}

#[tokio::test]
async fn runtime_config_sync_stops_observing_without_a_mutation_gate() {
    let mut plan = runtime_sync_test_plan("203.0.113.20", "10.255.0.0", "10.255.0.1");
    plan.runtime_control.manager = vpsman_common::RuntimeTunnelManager::ExternalObserved;
    let base = AgentConfig {
        client_id: "left-a".to_string(),
        network: vpsman_common::AgentNetworkConfig {
            apply_enabled: false,
            runtime_reconcile_enabled: false,
            runtime_status_telemetry_plans: vec![runtime_sync_test_telemetry_plan(plan)],
            ..Default::default()
        },
        ..AgentConfig::default()
    };
    let mut desired = AgentRuntimeConfig::from_agent_config(15, &base);
    desired.network.runtime_status_telemetry_plans.clear();

    let result = apply_runtime_config_sync(
        uuid::Uuid::new_v4(),
        &base,
        &desired,
        15,
        "test-stop-observing",
        CommandCancelToken::default(),
    )
    .await
    .unwrap();

    assert_eq!(result.outputs[0].exit_code, Some(0));
    let body: serde_json::Value = serde_json::from_slice(&result.outputs[0].data).unwrap();
    assert_eq!(body["status"], "applied");
    assert_eq!(body["removals"][0]["status"], "observed_only");
    assert!(result
        .applied_config
        .expect("read-only observation removal should apply")
        .network
        .runtime_status_telemetry_plans
        .is_empty());
}

#[tokio::test]
async fn runtime_config_sync_blocks_status_only_adapter_removal() {
    let root = std::env::temp_dir().join(format!(
        "vpsman-runtime-sync-adapter-remove-unavailable-{}",
        uuid::Uuid::new_v4()
    ));
    tokio::fs::create_dir_all(&root).await.unwrap();
    let mut plan = runtime_sync_test_plan("203.0.113.20", "10.255.0.0", "10.255.0.1");
    plan.runtime_control = vpsman_common::RuntimeTunnelControl {
        manager: vpsman_common::RuntimeTunnelManager::CustomAdapter,
        left_adapter_definition_id: Some("11111111-1111-4111-8111-111111111111".to_string()),
        right_adapter_definition_id: Some("22222222-2222-4222-8222-222222222222".to_string()),
        ..Default::default()
    };
    let mut telemetry_plan = runtime_sync_test_telemetry_plan(plan);
    telemetry_plan.runtime_adapter = Some(vpsman_common::RuntimeTunnelAdapterCommands {
        definition_id: "11111111-1111-4111-8111-111111111111".to_string(),
        definition_name: "status-only-test".to_string(),
        definition_hash: "ab".repeat(32),
        startup: None,
        stop: None,
        cleanup: None,
        restart: None,
        status: vpsman_common::RuntimeTunnelCommand {
            argv: vec!["/bin/echo".to_string(), "status".to_string()],
            max_timeout_secs: 5,
            max_output_bytes: 4096,
        },
        traffic_limit_apply: None,
    });
    let base = AgentConfig {
        client_id: "left-a".to_string(),
        network: vpsman_common::AgentNetworkConfig {
            apply_enabled: true,
            runtime_reconcile_enabled: true,
            root_dir: root.to_string_lossy().to_string(),
            runtime_ip_argv: vec!["/bin/echo".to_string()],
            runtime_tc_argv: vec!["/bin/echo".to_string()],
            runtime_unprivileged_mutation_policy:
                vpsman_common::AgentRuntimeUnprivilegedMutationPolicy::TryAll,
            runtime_status_telemetry_plans: vec![telemetry_plan],
            ..Default::default()
        },
        ..AgentConfig::default()
    };
    let mut desired = AgentRuntimeConfig::from_agent_config(14, &base);
    desired.network.runtime_status_telemetry_plans.clear();

    let result = apply_runtime_config_sync(
        uuid::Uuid::new_v4(),
        &base,
        &desired,
        14,
        "test-status-only-adapter-disabled",
        CommandCancelToken::default(),
    )
    .await
    .unwrap();

    assert_eq!(result.outputs[0].exit_code, Some(1));
    let body: serde_json::Value = serde_json::from_slice(&result.outputs[0].data).unwrap();
    assert_eq!(body["status"], "failed");
    assert_eq!(body["removed_tunnel_count"], 1);
    assert_eq!(body["removals"][0]["plan_id"], "plan-a");
    assert_eq!(body["removals"][0]["status"], "remove_unavailable");
    assert!(result.applied_config.is_none());
}

#[tokio::test]
async fn runtime_config_sync_failure_does_not_return_config_update() {
    let root =
        std::env::temp_dir().join(format!("vpsman-runtime-sync-fail-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&root).await.unwrap();
    let plan = vpsman_common::plan_tunnel(&vpsman_common::TunnelPlanInput {
        name: "left-right".to_string(),
        interface_name: "tunlr".to_string(),
        kind: vpsman_common::TunnelKind::Gre,
        runtime_control: Default::default(),
        runtime_topology: Default::default(),
        left_client_id: "client-a".to_string(),
        right_client_id: "client-b".to_string(),
        left_remote_underlay: "198.51.100.10".to_string(),
        right_remote_underlay: "203.0.113.20".to_string(),
        left_local_underlay: None,
        right_local_underlay: None,
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
        bandwidth_mbps: 100,
        left_mtu: Some(1476),
        right_mtu: Some(1476),
        ospf: None,
    })
    .unwrap();
    let base = AgentConfig {
        client_id: "client-a".to_string(),
        display_name: "old-name".to_string(),
        network: vpsman_common::AgentNetworkConfig {
            root_dir: root.to_string_lossy().to_string(),
            runtime_ip_argv: vec!["/bin/false".to_string()],
            runtime_tc_argv: vec!["/bin/false".to_string()],
            runtime_command_timeout_secs: 1,
            runtime_unprivileged_mutation_policy:
                vpsman_common::AgentRuntimeUnprivilegedMutationPolicy::TryAll,
            ..Default::default()
        },
        ..AgentConfig::default()
    };
    let mut desired = AgentRuntimeConfig {
        version: 10,
        display_name: "new-name".to_string(),
        ..AgentRuntimeConfig::from_agent_config(10, &base)
    };
    desired.network.apply_enabled = true;
    desired.network.runtime_reconcile_enabled = true;
    desired.network.runtime_status_telemetry_plans.push(
        vpsman_common::AgentRuntimeStatusTelemetryPlan {
            plan_id: Some("plan-a".to_string()),
            topology_identity_hash: "0".repeat(64),
            endpoint_side: vpsman_common::TunnelEndpointSide::Left,
            plan,
            builtin_credentials: None,
            runtime_adapter: None,
            latency_monitoring_enabled: true,
        },
    );

    let result = apply_runtime_config_sync(
        uuid::Uuid::new_v4(),
        &base,
        &desired,
        10,
        "test-failure",
        CommandCancelToken::default(),
    )
    .await
    .unwrap();

    assert_eq!(base.display_name, "old-name");
    assert_eq!(result.outputs[0].exit_code, Some(1));
    assert!(result.applied_config.is_none());
}

#[tokio::test]
async fn runtime_config_sync_cancel_returns_no_config_update() {
    let token = CommandCancelToken::default();
    token.cancel("operator requested cancellation".to_string());
    let base = AgentConfig {
        client_id: "client-a".to_string(),
        ..AgentConfig::default()
    };
    let desired = AgentRuntimeConfig {
        version: 11,
        ..AgentRuntimeConfig::default()
    };

    let error = apply_runtime_config_sync(
        uuid::Uuid::new_v4(),
        &base,
        &desired,
        11,
        "test-cancel",
        token,
    )
    .await
    .unwrap_err();

    let canceled = error
        .downcast_ref::<CommandCanceled>()
        .expect("runtime sync should surface cancellation");
    assert_eq!(canceled.reason(), "operator requested cancellation");
}

#[test]
fn runtime_config_sync_timeout_maps_to_command_timeout_output() {
    let job_id = uuid::Uuid::new_v4();
    let outputs = command_result_outputs(
        job_id,
        "runtime_config_sync",
        17,
        Err(anyhow::anyhow!(
            "runtime config sync timed out: deadline elapsed"
        )),
    );

    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].exit_code, Some(124));
    assert!(outputs[0].done);
    let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
    assert_eq!(status["type"], "command_timeout");
    assert_eq!(status["operation_type"], "runtime_config_sync");
    assert_eq!(status["max_timeout_secs"], 17);
}

#[test]
fn command_failure_output_preserves_actionable_error_causes() {
    let job_id = uuid::Uuid::new_v4();
    let error = anyhow::Error::new(std::io::Error::from_raw_os_error(libc::ENOENT))
        .context("failed to spawn command");

    let outputs = command_result_outputs(job_id, "shell_argv", 30, Err(error));

    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].exit_code, Some(127));
    let message = String::from_utf8(outputs[0].data.clone()).unwrap();
    assert!(message.contains("failed to spawn command"));
    assert!(message.contains("No such file or directory"));
}

fn runtime_sync_test_plan(
    right_remote_underlay: &str,
    left_tunnel: &str,
    right_tunnel: &str,
) -> vpsman_common::TunnelPlan {
    vpsman_common::plan_tunnel(&vpsman_common::TunnelPlanInput {
        name: "left-right".to_string(),
        interface_name: "tunlr".to_string(),
        kind: vpsman_common::TunnelKind::Gre,
        runtime_control: Default::default(),
        runtime_topology: Default::default(),
        left_client_id: "left-a".to_string(),
        right_client_id: "right-b".to_string(),
        left_remote_underlay: "198.51.100.10".to_string(),
        right_remote_underlay: right_remote_underlay.to_string(),
        left_local_underlay: None,
        right_local_underlay: None,
        address_pool_cidr: "10.255.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
            left: left_tunnel.to_string(),
            right: right_tunnel.to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth_mbps: 100,
        left_mtu: Some(1476),
        right_mtu: Some(1476),
        ospf: None,
    })
    .unwrap()
}

fn runtime_sync_test_telemetry_plan(
    plan: vpsman_common::TunnelPlan,
) -> vpsman_common::AgentRuntimeStatusTelemetryPlan {
    vpsman_common::AgentRuntimeStatusTelemetryPlan {
        plan_id: Some("plan-a".to_string()),
        topology_identity_hash: "0".repeat(64),
        endpoint_side: vpsman_common::TunnelEndpointSide::Left,
        plan,
        builtin_credentials: None,
        runtime_adapter: None,
        latency_monitoring_enabled: true,
    }
}

#[test]
fn unmanaged_update_schedule_uses_next_interval_slot() {
    let config = AgentConfig {
        update: vpsman_common::AgentUpdateConfig {
            unmanaged_interval_secs: 300,
            unmanaged_jitter_secs: 0,
            ..vpsman_common::AgentUpdateConfig::default()
        },
        ..AgentConfig::default()
    };
    let base_instant = time::Instant::now();
    let due =
        next_unmanaged_update_due(&config, UNIX_EPOCH + Duration::from_secs(100), base_instant);

    assert_eq!(due.duration_since(base_instant), Duration::from_secs(200));
}
