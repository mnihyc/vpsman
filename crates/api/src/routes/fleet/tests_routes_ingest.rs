use super::*;

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
