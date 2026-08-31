use super::*;

#[test]
fn accepted_hello_has_no_reverse_gateway_control_dependency() {
    let source = include_str!("routes_ingest.rs");
    let (_, route) = source
        .split_once("pub(crate) async fn ingest_agent_hello(")
        .expect("agent hello ingest owner");
    let (route, _) = route
        .split_once("pub(crate) async fn request_runtime_config_reload(")
        .expect("agent hello ingest boundary");

    let commit = route
        .find("upsert_agent_hello")
        .expect("durable hello commit");
    let publish = route.find("state.publish").expect("accepted hello event");
    assert!(commit < publish);
    assert!(!route.contains("state.gateway"));
    assert!(!route.contains("suspension_fence"));
    assert!(!route.contains("tokio::time::sleep"));
}

#[test]
fn accepted_hello_preserves_established_database_auto_unsuspend_semantics() {
    let source = include_str!("../../repository/fleet/repository_ingest.rs");
    let (_, hello) = source
        .split_once("pub(crate) async fn upsert_agent_hello(")
        .expect("hello repository owner");
    let (hello, _) = hello
        .split_once("pub(crate) async fn record_telemetry_outcome(")
        .expect("hello repository boundary");

    assert!(hello.contains("ELSE 'online'"));
    for cleared_field in [
        "suspended_at = NULL",
        "suspended_by = NULL",
        "suspended_reason = NULL",
        "suspended_from_status = NULL",
    ] {
        assert!(hello.contains(cleared_field), "{cleared_field}");
    }
    assert!(hello.contains("Some(\"suspended\") => \"agent_online_auto_unsuspend\""));
    assert!(hello.contains("record_client_status_transition_in_tx("));
}

#[test]
fn duplicate_vnstat_output_does_not_clear_retry_cooldown_or_wake_finalization() {
    assert!(network_traffic_import_output_advances_finalization(
        JobOutputWriteResult::Inserted
    ));
    assert!(!network_traffic_import_output_advances_finalization(
        JobOutputWriteResult::DuplicateIdentical
    ));
    assert!(!network_traffic_import_output_advances_finalization(
        JobOutputWriteResult::DuplicateConflict
    ));
}

#[test]
fn agent_metric_validation_distinguishes_unknown_zero_and_invalid_swap() {
    let metrics = |swap_total_bytes, swap_available_bytes| vpsman_common::AgentMetrics {
        observed_unix: 1,
        hostname: "vps".to_string(),
        memory: vpsman_common::MemoryStat {
            total_bytes: 1024,
            available_bytes: 512,
            swap_total_bytes,
            swap_available_bytes,
        },
        ..Default::default()
    };

    assert!(valid_agent_metrics(&metrics(None, None)));
    assert!(valid_agent_metrics(&metrics(Some(0), Some(0))));
    assert!(valid_agent_metrics(&metrics(Some(1024), Some(512))));
    assert!(!valid_agent_metrics(&metrics(Some(1024), None)));
    assert!(!valid_agent_metrics(&metrics(None, Some(0))));
    assert!(!valid_agent_metrics(&metrics(Some(1024), Some(2048))));
}

#[test]
fn unavailable_disk_collection_cannot_carry_disk_rows() {
    let mut metrics = vpsman_common::AgentMetrics {
        observed_unix: 1,
        hostname: "vps".to_string(),
        disks: vec![vpsman_common::DiskStat {
            mountpoint: "/".to_string(),
            total_bytes: 1,
            available_bytes: 1,
        }],
        disk_collection_available: Some(false),
        disk_semantics: Some(
            vpsman_common::DISK_SEMANTICS_PERSISTENT_BLOCK_FILESYSTEMS_V1.to_string(),
        ),
        ..Default::default()
    };
    assert!(!valid_agent_metrics(&metrics));

    metrics.disk_collection_available = Some(true);
    metrics.disks.push(metrics.disks[0].clone());
    assert!(!valid_agent_metrics(&metrics));

    metrics.disks.pop();
    metrics.disk_collection_available = None;
    metrics.disk_semantics = None;
    assert!(valid_agent_metrics(&metrics));
    assert!(!metrics.has_persistent_block_filesystem_disk_sample());

    metrics.disk_collection_available = Some(true);
    metrics.disk_semantics = Some("future_disk_semantics".to_string());
    assert!(valid_agent_metrics(&metrics));
    assert!(!metrics.has_persistent_block_filesystem_disk_sample());
}

#[test]
fn automatic_reachability_timestamp_uses_the_existing_ping_clock_window() {
    let observed_unix = 10_000;
    let mut metrics = vpsman_common::AgentMetrics {
        observed_unix,
        hostname: "vps".to_string(),
        tunnel_reachability: vec![vpsman_common::TunnelReachabilityObservation {
            id: uuid::Uuid::new_v4(),
            source: vpsman_common::TunnelReachabilitySource::Automatic,
            plan_id: uuid::Uuid::new_v4(),
            topology_identity_hash: "a".repeat(64),
            endpoint_side: vpsman_common::TunnelEndpointSide::Left,
            peer_client_id: "v-2".to_string(),
            interface_name: "tun0".to_string(),
            address_family: vpsman_common::TunnelAddressFamily::Ipv4,
            target: "10.0.0.2".to_string(),
            measured_unix: observed_unix,
            stale_after_secs: 180,
            transmitted: 3,
            received: 3,
            latency_min_ms: Some(1.0),
            latency_avg_ms: Some(2.0),
            latency_max_ms: Some(3.0),
            latency_mdev_ms: Some(0.1),
            packet_loss_ratio: 0.0,
            healthy: true,
            reason: None,
        }],
        ..Default::default()
    };

    assert!(valid_agent_metrics(&metrics));
    metrics.tunnel_reachability[0].measured_unix = observed_unix - 3_900;
    assert!(valid_agent_metrics(&metrics));
    metrics.tunnel_reachability[0].measured_unix = observed_unix - 3_901;
    assert!(!valid_agent_metrics(&metrics));
    metrics.tunnel_reachability[0].measured_unix = observed_unix + 300;
    assert!(valid_agent_metrics(&metrics));
    metrics.tunnel_reachability[0].measured_unix = observed_unix + 301;
    assert!(!valid_agent_metrics(&metrics));
    metrics.tunnel_reachability[0].measured_unix = 0;
    assert!(!valid_agent_metrics(&metrics));
    metrics.tunnel_reachability[0].measured_unix = observed_unix;
    metrics.tunnel_reachability[0].stale_after_secs = i64::MAX as u64 + 1;
    assert!(!valid_agent_metrics(&metrics));
    metrics.tunnel_reachability[0].stale_after_secs = 180;
    metrics.tunnel_reachability[0].transmitted = i32::MAX as u32 + 1;
    metrics.tunnel_reachability[0].received = i32::MAX as u32 + 1;
    assert!(!valid_agent_metrics(&metrics));
    metrics.tunnel_reachability[0].transmitted = 3;
    metrics.tunnel_reachability[0].received = 3;
    assert!(valid_agent_metrics(&metrics));
}

#[test]
fn duplicate_projected_tunnel_interfaces_are_rejected_before_acceptance() {
    let tunnel = vpsman_common::RuntimeTunnelStat {
        interface: "wg0".to_string(),
        kind: "wireguard".to_string(),
        plan_id: Some(uuid::Uuid::new_v4().to_string()),
        plan_name: Some("site-link".to_string()),
        endpoint_side: Some("left".to_string()),
        ..Default::default()
    };
    let mut metrics = vpsman_common::AgentMetrics {
        observed_unix: 1,
        hostname: "vps".to_string(),
        tunnels: vec![tunnel.clone(), tunnel.clone()],
        ..Default::default()
    };
    assert!(!valid_agent_metrics(&metrics));

    // A row filtered out by the durable projection cannot collide with the
    // retained interface, so only projected tunnel identities must be unique.
    metrics.tunnels[1].plan_id = None;
    assert!(valid_agent_metrics(&metrics));
}

#[test]
fn ingest_unsupported_command_output_maps_to_rejected_target_status() {
    let job_id = uuid::Uuid::new_v4();
    let output = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "unsupported_command_version",
            "status": "rejected",
            "command_type": "shell_argv",
        }))
        .unwrap(),
        exit_code: Some(78),
        done: true,
    };

    let outcome =
        target_outcome_from_done_output(job_id, &output, "2026-06-13T00:00:00Z".to_string());

    assert_eq!(outcome.status, vpsman_server_core::TARGET_STATUS_REJECTED);
    assert_eq!(outcome.exit_code, Some(78));
    assert_eq!(outcome.message, "unsupported_command_version: rejected");
}

#[test]
fn ingest_done_output_without_exit_code_maps_to_failed() {
    let job_id = uuid::Uuid::new_v4();
    let output = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: Vec::new(),
        exit_code: None,
        done: true,
    };

    let outcome =
        target_outcome_from_done_output(job_id, &output, "2026-06-13T00:00:00Z".to_string());

    assert_eq!(outcome.status, vpsman_server_core::TARGET_STATUS_FAILED);
    assert_eq!(outcome.exit_code, None);
    assert_eq!(
        outcome.message,
        crate::routes_jobs::COMMAND_COMPLETED_WITHOUT_EXIT_CODE_MESSAGE
    );
}

#[test]
fn ingest_timeout_output_reports_operation_and_duration() {
    let output = CommandOutput {
        job_id: uuid::Uuid::new_v4(),
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "command_timeout",
            "operation_type": "network_speed_test",
            "max_timeout_secs": 60,
        }))
        .unwrap(),
        exit_code: Some(124),
        done: true,
    };

    assert_eq!(
        status_output_message(&output).as_deref(),
        Some(
            "network speed test exceeded its agent execution timeout after 60 seconds (command_timeout)"
        )
    );
}
