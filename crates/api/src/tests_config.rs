use axum::{extract::State, http::HeaderMap, Json};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use tokio::sync::broadcast;

use crate::{
    gateway_client::GatewayDispatchClient,
    job_request::validate_job_command,
    model::CreateJobRequest,
    repository::{MemoryState, Repository},
    repository_ingest::upsert_memory_agent,
    routes_jobs::create_job,
    state::AppState,
};
use ed25519_dalek::SigningKey;
use vpsman_common::{
    sign_update_artifact_hash, AgentCapabilitySnapshot, AgentConfig, AgentHello,
    AgentPrivilegeMode, JobCommand,
};

async fn wait_for_job_status(
    repo: &crate::repository::Repository,
    job_id: uuid::Uuid,
    expected: &str,
) {
    for _ in 0..50 {
        let jobs = repo.list_jobs(100).await.unwrap();
        if jobs
            .iter()
            .any(|job| job.id == job_id && job.status == expected)
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("job {job_id} did not reach status {expected}");
}

#[test]
fn validates_hot_config_job_document() {
    let config = AgentConfig {
        display_name: "edge-a".to_string(),
        tags: vec!["bgp".to_string()],
        ..AgentConfig::default()
    };
    let command = JobCommand::HotConfig {
        toml: toml::to_string_pretty(&config).unwrap(),
        preserve_redacted: None,
        base_config_sha256_hex: None,
    };

    validate_job_command(&command).unwrap();
}

#[test]
fn rejects_invalid_hot_config_job_document() {
    let command = JobCommand::HotConfig {
        toml: "client_id = ''".to_string(),
        preserve_redacted: None,
        base_config_sha256_hex: None,
    };

    assert!(validate_job_command(&command).is_err());
}

#[test]
fn validates_data_source_config_patch_job_document() {
    let command = JobCommand::DataSourceConfigPatch {
        toml: "[telemetry]\nproc_root = \"/tmp/vpsman-proc\"\n".to_string(),
    };

    validate_job_command(&command).unwrap();
}

#[test]
fn rejects_invalid_data_source_config_patch_job_document() {
    assert!(validate_job_command(&JobCommand::DataSourceConfigPatch {
        toml: String::new(),
    })
    .is_err());
    assert!(validate_job_command(&JobCommand::DataSourceConfigPatch {
        toml: "client_id = \"other\"".to_string(),
    })
    .is_err());
    assert!(validate_job_command(&JobCommand::DataSourceConfigPatch {
        toml: "[auth]\ncommand_timeout_secs = 10".to_string(),
    })
    .is_err());
}

#[test]
fn validates_agent_update_job_document() {
    let command = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "ab".repeat(32),
        artifact_signature_hex: None,
        artifact_signing_key_hex: None,
    };

    validate_job_command(&command).unwrap();

    let signing_key = SigningKey::from_bytes(&[31_u8; 32]);
    let sha256_hex = "cd".repeat(32);
    let command = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: sha256_hex.clone(),
        artifact_signature_hex: Some(hex::encode(sign_update_artifact_hash(
            &signing_key,
            &sha256_hex,
        ))),
        artifact_signing_key_hex: Some(hex::encode(signing_key.verifying_key().to_bytes())),
    };
    validate_job_command(&command).unwrap();

    validate_job_command(&JobCommand::AgentUpdateActivate {
        staged_sha256_hex: "ef".repeat(32),
        restart_agent: false,
    })
    .unwrap();
    validate_job_command(&JobCommand::AgentUpdateRollback {
        rollback_sha256_hex: Some("01".repeat(32)),
    })
    .unwrap();
    validate_job_command(&JobCommand::AgentUpdateRollback {
        rollback_sha256_hex: None,
    })
    .unwrap();
}

#[test]
fn rejects_invalid_agent_update_job_document() {
    assert!(validate_job_command(&JobCommand::UpdateAgent {
        artifact_url: "http://updates.example/vpsman-agent".to_string(),
        sha256_hex: "ab".repeat(32),
        artifact_signature_hex: None,
        artifact_signing_key_hex: None,
    })
    .is_err());
    assert!(validate_job_command(&JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "not-a-hash".to_string(),
        artifact_signature_hex: None,
        artifact_signing_key_hex: None,
    })
    .is_err());
    assert!(validate_job_command(&JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "ab".repeat(32),
        artifact_signature_hex: Some("00".repeat(64)),
        artifact_signing_key_hex: None,
    })
    .is_err());
    assert!(validate_job_command(&JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "ab".repeat(32),
        artifact_signature_hex: Some("00".repeat(64)),
        artifact_signing_key_hex: Some("11".repeat(32)),
    })
    .is_err());
    assert!(validate_job_command(&JobCommand::AgentUpdateActivate {
        staged_sha256_hex: "not-a-hash".to_string(),
        restart_agent: false,
    })
    .is_err());
    assert!(validate_job_command(&JobCommand::AgentUpdateRollback {
        rollback_sha256_hex: Some("not-a-hash".to_string()),
    })
    .is_err());
}

#[tokio::test]
async fn agent_update_degrades_unprivileged_target_after_privilege_verification() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "client-a".to_string(),
                agent_version: "test".to_string(),
                os_release: "test".to_string(),
                arch: "x86_64".to_string(),
                update_heartbeat: None,
                internal_build_number: 1,
                capabilities: AgentCapabilitySnapshot {
                    privilege_mode: AgentPrivilegeMode::Unprivileged,
                    effective_uid: Some(1000),
                    can_attempt_privileged_ops: false,
                    unprivileged_hint: Some("running as normal user".to_string()),
                    ..Default::default()
                },
            },
        )
        .await;
    }
    let operation = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "ab".repeat(32),
        artifact_signature_hex: None,
        artifact_signing_key_hex: None,
    };
    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: true,
        command: "agent_update".to_string(),
        argv: Vec::new(),
        operation: Some(operation),
        timeout_secs: Some(60),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        reconnect_policy: None,
    };

    let (status, Json(response)) = create_job(
        State(test_state_with_privilege_auto_approve(repo.clone())),
        HeaderMap::new(),
        Json(request),
    )
    .await
    .unwrap();
    wait_for_job_status(&repo, response.job_id, "degraded_unprivileged").await;
    let targets = repo.list_job_targets(response.job_id).await.unwrap();
    let outputs = repo.list_job_outputs(response.job_id).await.unwrap();
    let output_bytes = BASE64_STANDARD.decode(&outputs[0].data_base64).unwrap();
    let status_output: serde_json::Value = serde_json::from_slice(&output_bytes).unwrap();

    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(response.accepted_targets, 0);
    assert_eq!(response.status, "dispatching");
    assert_eq!(targets[0].status, "degraded_unprivileged");
    assert_eq!(
        status_output["reason"],
        "target_agent_lacks_agent_update_capability"
    );
}

fn test_state(repo: Repository) -> AppState {
    let (events, _) = broadcast::channel(1);
    AppState {
        repo,
        events,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        backup_object_store: None,
        update_object_store: None,
        update_artifact_public_base_url: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        require_registered_agent_updates: false,
    }
}

fn test_state_with_privilege_auto_approve(repo: Repository) -> AppState {
    AppState {
        gateway: GatewayDispatchClient::test_privilege_auto_approve(),
        ..test_state(repo)
    }
}
