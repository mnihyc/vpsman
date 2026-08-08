use super::*;
use crate::{
    gateway_client::GatewayDispatchClient,
    model::JobHistoryView,
    repository::{MemoryState, Repository},
    state::{AppState, DispatcherRuntimeConfig, DEFAULT_ARTIFACT_MAX_BYTES},
};

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
