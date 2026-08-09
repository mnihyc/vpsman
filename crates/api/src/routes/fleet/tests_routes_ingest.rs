use super::*;

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
