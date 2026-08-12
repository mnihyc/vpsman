use super::*;
use crate::{
    gateway_client::GatewayDispatchClient,
    model::{JobHistoryView, JobOutputView, JobTargetView, TelemetryNetworkRateView},
    model_alert_policies::TrafficCounterSampleRecord,
    repository::{MemoryState, Repository},
    repository_job_outputs::{JobOutputPersistConfig, JobOutputWriteResult},
    state::{AppState, DispatcherRuntimeConfig, DEFAULT_ARTIFACT_MAX_BYTES},
};
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::TimeZone;

#[tokio::test]
async fn actual_import_job_rejects_exit_zero_final_on_wrong_stream() {
    let (state, job_id) = import_test_state().await;
    let output = CommandOutput {
        job_id,
        stream: OutputStream::Stdout,
        data: br#"{"type":"network_traffic_import_vnstat","status":"collected"}"#.to_vec(),
        exit_code: Some(0),
        done: true,
    };

    let applied = apply_network_traffic_import_if_ready(&state, job_id, "edge-a", 0, &output, &[])
        .await
        .unwrap();

    assert_eq!(
        applied,
        NetworkTrafficImportApply::Invalid(
            "network_traffic_import_invalid:final_output_invalid".to_string()
        )
    );
}

#[tokio::test]
async fn actual_import_job_preserves_nonzero_agent_failure() {
    let (state, job_id) = import_test_state().await;
    let output = CommandOutput {
        job_id,
        stream: OutputStream::Stderr,
        data: b"vnstat executable not found".to_vec(),
        exit_code: Some(1),
        done: true,
    };

    let applied = apply_network_traffic_import_if_ready(&state, job_id, "edge-a", 0, &output, &[])
        .await
        .unwrap();

    assert_eq!(applied, NetworkTrafficImportApply::NotApplicable);
}

#[tokio::test]
async fn persisted_import_survives_restart_phase_and_finalizes_exactly_once() {
    let (state, job_id, final_output) = persisted_import_state().await;

    // A startup sweep, without an ingest wake, discovers the persisted final
    // output and performs the server-side phase before terminalizing the job.
    assert_eq!(
        finalize_pending_network_traffic_imports(&state)
            .await
            .unwrap(),
        1
    );
    let target = state.repo.list_job_targets(job_id).await.unwrap().remove(0);
    assert_eq!(target.status, vpsman_server_core::TARGET_STATUS_COMPLETED);
    assert!(target
        .message
        .as_deref()
        .is_some_and(|message| message.contains("vnStat history imported")));
    let rate = match &state.repo {
        Repository::Memory(memory) => memory.telemetry_network_rates.read().await[0].clone(),
        Repository::Postgres(_) => unreachable!(),
    };
    assert_eq!((rate.rx_counter_epoch, rate.tx_counter_epoch), (7, 9));

    let imported_before_retry = match &state.repo {
        Repository::Memory(memory) => {
            let samples = memory.traffic_counter_samples.read().await;
            let mut imported = samples
                .iter()
                .filter(|sample| sample.sample_source == format!("vnstat_import:{job_id}"))
                .collect::<Vec<_>>();
            imported.sort_by_key(|sample| sample.observed_unix);
            let first = imported.first().unwrap();
            let last = imported.last().unwrap();
            assert_eq!(last.rx_bytes - first.rx_bytes, 100);
            assert_eq!(last.tx_bytes - first.tx_bytes, 50);
            imported.len()
        }
        Repository::Postgres(_) => unreachable!(),
    };
    let audit_before_retry = state
        .repo
        .list_audit_logs(500)
        .await
        .unwrap()
        .into_iter()
        .filter(|audit| audit.action == "job.target_result")
        .count();

    assert_eq!(
        state
            .repo
            .classify_existing_job_output_chunk_with_config(
                job_id,
                "edge-a",
                1,
                &final_output,
                JobOutputPersistConfig {
                    object_store: None,
                    artifact_min_bytes: 32_768,
                },
            )
            .await
            .unwrap(),
        Some(JobOutputWriteResult::DuplicateIdentical)
    );
    assert_eq!(
        finalize_pending_network_traffic_imports(&state)
            .await
            .unwrap(),
        0
    );

    let imported_after_retry = match &state.repo {
        Repository::Memory(memory) => {
            let samples = memory.traffic_counter_samples.read().await;
            let mut imported = samples
                .iter()
                .filter(|sample| sample.sample_source == format!("vnstat_import:{job_id}"))
                .collect::<Vec<_>>();
            imported.sort_by_key(|sample| sample.observed_unix);
            let first = imported.first().unwrap();
            let last = imported.last().unwrap();
            assert_eq!(last.rx_bytes - first.rx_bytes, 100);
            assert_eq!(last.tx_bytes - first.tx_bytes, 50);
            imported.len()
        }
        Repository::Postgres(_) => unreachable!(),
    };
    let audit_after_retry = state
        .repo
        .list_audit_logs(500)
        .await
        .unwrap()
        .into_iter()
        .filter(|audit| audit.action == "job.target_result")
        .count();
    assert_eq!(imported_after_retry, imported_before_retry);
    assert_eq!(audit_after_retry, audit_before_retry);
}

#[tokio::test]
async fn persisted_import_final_output_prevents_control_deadline_expiry() {
    let (state, job_id, _) = persisted_import_state().await;
    let expired = state
        .repo
        .expire_control_timeout_targets(10, 0)
        .await
        .unwrap();
    assert!(expired.is_empty());
    let target = state.repo.list_job_targets(job_id).await.unwrap().remove(0);
    assert_eq!(target.status, vpsman_server_core::TARGET_STATUS_RUNNING);
    assert!(target.completed_at.is_none());

    let (gapped_state, gapped_job_id, _) = persisted_import_state().await;
    if let Repository::Memory(memory) = &gapped_state.repo {
        memory
            .job_outputs
            .write()
            .await
            .retain(|output| output.seq != 0);
    }
    let expired = gapped_state
        .repo
        .expire_control_timeout_targets(10, 0)
        .await
        .unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].job_id, gapped_job_id);
    assert_eq!(
        expired[0].status,
        vpsman_server_core::TARGET_STATUS_CONTROL_TIMEOUT
    );
}

#[tokio::test]
async fn invalid_persisted_import_terminalizes_as_failed_without_backfill() {
    let (state, job_id, _) = persisted_import_state().await;
    if let Repository::Memory(memory) = &state.repo {
        let mut outputs = memory.job_outputs.write().await;
        outputs
            .iter_mut()
            .find(|output| output.seq == 1)
            .unwrap()
            .stream = "stdout".to_string();
    }

    assert_eq!(
        finalize_pending_network_traffic_imports(&state)
            .await
            .unwrap(),
        1
    );
    let target = state.repo.list_job_targets(job_id).await.unwrap().remove(0);
    assert_eq!(target.status, vpsman_server_core::TARGET_STATUS_FAILED);
    assert_eq!(
        target.message.as_deref(),
        Some("network_traffic_import_invalid:final_output_invalid")
    );
    if let Repository::Memory(memory) = &state.repo {
        assert!(!memory
            .traffic_counter_samples
            .read()
            .await
            .iter()
            .any(|sample| sample.sample_source == format!("vnstat_import:{job_id}")));
    }
}

#[tokio::test]
async fn malformed_contiguous_batch_terminalizes_instead_of_remaining_pending() {
    let (state, job_id, _) = persisted_import_state().await;
    if let Repository::Memory(memory) = &state.repo {
        let mut outputs = memory.job_outputs.write().await;
        outputs
            .iter_mut()
            .find(|output| output.seq == 0)
            .unwrap()
            .stream = "stdout".to_string();
    }

    assert_eq!(
        finalize_pending_network_traffic_imports(&state)
            .await
            .unwrap(),
        1
    );
    let target = state.repo.list_job_targets(job_id).await.unwrap().remove(0);
    assert_eq!(target.status, vpsman_server_core::TARGET_STATUS_FAILED);
    assert_eq!(
        target.message.as_deref(),
        Some("network_traffic_import_invalid:batch_output_invalid")
    );
    assert!(target.completed_at.is_some());
}

#[tokio::test]
async fn cooled_retry_page_does_not_starve_the_next_pending_target() {
    let memory = MemoryState::default();
    let mut job_ids = Vec::new();
    let mut targets = Vec::new();
    let mut outputs = Vec::new();
    for ordinal in 1_u128..=129 {
        let job_id = uuid::Uuid::from_u128(ordinal);
        job_ids.push(job_id);
        memory.job_operations.write().await.insert(
            job_id,
            JobCommand::NetworkTrafficImportVnstat {
                interfaces: vec!["eth0".to_string()],
                start_unix: 60,
            },
        );
        targets.push(JobTargetView {
            job_id,
            client_id: "edge-a".to_string(),
            status: vpsman_server_core::TARGET_STATUS_RUNNING.to_string(),
            message: None,
            exit_code: None,
            started_at: Some("1".to_string()),
            deadline_at: None,
            completed_at: None,
            process_incarnation_id: None,
        });
        outputs.push(JobOutputView {
            job_id,
            client_id: "edge-a".to_string(),
            seq: 0,
            stream: "status".to_string(),
            data_base64: BASE64.encode(b"{}"),
            storage: "inline".to_string(),
            artifact_object_key: None,
            artifact_sha256_hex: None,
            artifact_size_bytes: Some(2),
            exit_code: Some(0),
            done: true,
            received_at: None,
            created_at: "1".to_string(),
        });
    }
    memory.job_targets.write().await.extend(targets);
    memory.job_outputs.write().await.extend(outputs);
    let memory_view = memory.clone();
    let repo = Repository::Memory(memory);

    for job_id in job_ids.iter().take(128) {
        repo.defer_network_traffic_import_finalization(
            *job_id,
            "edge-a",
            "vnStat server import retry pending",
            30,
        )
        .await
        .unwrap();
    }

    let pending = repo
        .list_pending_network_traffic_import_finalizations(128)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].job_id, job_ids[128]);

    let terminal = target_outcome_from_done_output(
        job_ids[0],
        &CommandOutput {
            job_id: job_ids[0],
            stream: OutputStream::Status,
            data: b"completed".to_vec(),
            exit_code: Some(0),
            done: true,
        },
        "2".to_string(),
    );
    assert!(repo
        .update_job_target_result(job_ids[0], "edge-a", &terminal)
        .await
        .unwrap());
    assert!(!memory_view
        .network_traffic_import_retry_not_before
        .read()
        .await
        .contains_key(&(job_ids[0], "edge-a".to_string())));
}

async fn persisted_import_state() -> (AppState, uuid::Uuid, CommandOutput) {
    let memory = MemoryState::default();
    let job_id = uuid::Uuid::new_v4();
    let start = 1_722_470_400_u64;
    let live = start + 600;
    memory.jobs.write().await.push(JobHistoryView {
        id: job_id,
        actor_id: None,
        command_type: "network_traffic_import_vnstat".to_string(),
        source_schedule_id: None,
        privileged: true,
        status: "running".to_string(),
        target_count: 1,
        payload_hash: "00".repeat(32),
        max_timeout_secs: 1,
        created_at: "1".to_string(),
        completed_at: None,
    });
    memory.job_operations.write().await.insert(
        job_id,
        JobCommand::NetworkTrafficImportVnstat {
            interfaces: Vec::new(),
            start_unix: start,
        },
    );
    memory.job_timeouts.write().await.insert(job_id, 1);
    memory.job_targets.write().await.push(JobTargetView {
        job_id,
        client_id: "edge-a".to_string(),
        status: vpsman_server_core::TARGET_STATUS_RUNNING.to_string(),
        message: Some("vnStat history collected; server import pending".to_string()),
        exit_code: None,
        started_at: Some("1".to_string()),
        deadline_at: Some("2".to_string()),
        completed_at: None,
        process_incarnation_id: None,
    });
    memory
        .traffic_counter_samples
        .write()
        .await
        .push(TrafficCounterSampleRecord {
            client_id: "edge-a".to_string(),
            source_kind: "host".to_string(),
            interface: "eth0".to_string(),
            observed_at: Utc
                .timestamp_opt(live as i64, 0)
                .single()
                .unwrap()
                .to_rfc3339(),
            observed_unix: live as i64,
            rx_bytes: 10,
            tx_bytes: 20,
            rx_counter_epoch: 0,
            tx_counter_epoch: 0,
            sample_source: "interface_counters".to_string(),
        });
    memory
        .telemetry_network_rates
        .write()
        .await
        .push(TelemetryNetworkRateView {
            client_id: "edge-a".to_string(),
            interface: "eth0".to_string(),
            bucket_start: Utc
                .timestamp_opt(start as i64, 0)
                .single()
                .unwrap()
                .to_rfc3339(),
            bucket_secs: 60,
            sample_count: 1,
            rx_bytes_avg: 10,
            tx_bytes_avg: 20,
            rx_bytes_last: 10,
            tx_bytes_last: 20,
            rx_counter_epoch: 7,
            tx_counter_epoch: 9,
            latest_observed_at: Utc
                .timestamp_opt(start as i64, 0)
                .single()
                .unwrap()
                .to_rfc3339(),
            rx_bytes_delta: 0,
            tx_bytes_delta: 0,
            rx_bps_avg: 0.0,
            tx_bps_avg: 0.0,
            updated_at: "test".to_string(),
        });
    let batch = NetworkTrafficImportBatch {
        r#type: "network_traffic_import_vnstat_batch".to_string(),
        batch_index: 0,
        buckets: vec![NetworkTrafficImportBucket {
            interface: "eth0".to_string(),
            start_unix: start,
            duration_secs: 600,
            rx_bytes: 100,
            tx_bytes: 50,
        }],
    };
    let result = NetworkTrafficImportResult {
        r#type: "network_traffic_import_vnstat".to_string(),
        status: "collected".to_string(),
        requested_start_unix: start,
        collected_until_unix: live,
        interfaces: vec!["eth0".to_string()],
        sources: vec![vpsman_common::NetworkTrafficImportSource {
            interface: "eth0".to_string(),
            database_created_unix: Some(start),
            retained_start_unix: start,
            source_updated_unix: Some(live),
        }],
        batch_count: 1,
        bucket_count: 1,
        message: "vnStat history collected; API import is pending".to_string(),
    };
    let batch_output = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&batch).unwrap(),
        exit_code: None,
        done: false,
    };
    let final_output = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&result).unwrap(),
        exit_code: Some(0),
        done: true,
    };
    memory.job_outputs.write().await.extend([
        job_output_view(job_id, 0, &batch_output),
        job_output_view(job_id, 1, &final_output),
    ]);
    (test_state(Repository::Memory(memory)), job_id, final_output)
}

fn job_output_view(job_id: uuid::Uuid, seq: i32, output: &CommandOutput) -> JobOutputView {
    JobOutputView {
        job_id,
        client_id: "edge-a".to_string(),
        seq,
        stream: "status".to_string(),
        data_base64: BASE64.encode(&output.data),
        storage: "inline".to_string(),
        artifact_object_key: None,
        artifact_sha256_hex: Some(vpsman_common::payload_hash(&output.data)),
        artifact_size_bytes: Some(i64::try_from(output.data.len()).unwrap()),
        exit_code: output.exit_code,
        done: output.done,
        received_at: Some("2024-08-01T00:10:00Z".to_string()),
        created_at: "2024-08-01T00:10:00Z".to_string(),
    }
}

async fn import_test_state() -> (AppState, uuid::Uuid) {
    let memory = MemoryState::default();
    let job_id = uuid::Uuid::new_v4();
    memory.jobs.write().await.push(JobHistoryView {
        id: job_id,
        actor_id: None,
        command_type: "network_traffic_import_vnstat".to_string(),
        source_schedule_id: None,
        privileged: true,
        status: "running".to_string(),
        target_count: 1,
        payload_hash: "00".repeat(32),
        max_timeout_secs: 300,
        created_at: crate::unix_now().to_string(),
        completed_at: None,
    });
    memory.job_operations.write().await.insert(
        job_id,
        JobCommand::NetworkTrafficImportVnstat {
            interfaces: vec!["eth0".to_string()],
            start_unix: 1_722_470_400,
        },
    );
    (test_state(Repository::Memory(memory)), job_id)
}

fn test_state(repo: Repository) -> AppState {
    AppState {
        repo,
        events: tokio::sync::broadcast::channel(1).0,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        backup_object_store: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32_768,
        artifact_max_bytes: DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: std::path::PathBuf::from("config/vpsman.toml"),
        dispatcher_config: DispatcherRuntimeConfig::default(),
    }
}
