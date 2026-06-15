use axum::{extract::State, Json};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use tokio::sync::broadcast;
use uuid::Uuid;
use vpsman_common::{
    encode_inline_file_payload, payload_hash, AgentCapabilitySnapshot, AgentHello,
    AgentPrivilegeMode, JobCommand, RestoreRollbackFile,
};

use crate::{
    gateway_client::GatewayDispatchClient,
    job_request::validate_job_command,
    model::{CreateBackupRequest, CreateJobRequest, CreateRestorePlanRequest},
    repository::{MemoryState, Repository},
    repository_ingest::upsert_memory_agent,
    routes_backups::create_backup_request,
    routes_jobs::create_job,
    routes_restores::{create_restore_plan, validate_create_restore_plan},
    state::AppState,
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
fn restore_job_validation_requires_safe_inline_archive() {
    let source_backup_request_id = Uuid::new_v4();
    let missing_archive = JobCommand::Restore {
        source_backup_request_id,
        paths: vec!["/tmp/source.txt".to_string()],
        include_config: false,
        destination_root: Some("/restore".to_string()),
        archive_path: None,
        archive_base64: None,
        archive_size_bytes: None,
        archive_sha256_hex: None,
        dry_run: false,
        post_restore_argv: Vec::new(),
    };
    assert_eq!(
        validate_job_command(&missing_archive).unwrap_err().code,
        "restore_archive_required"
    );

    let archive_bytes = br#"{"format":"vpsman.backup_archive.v1","files":[]}"#;
    let valid = JobCommand::Restore {
        source_backup_request_id,
        paths: vec!["/tmp/source.txt".to_string()],
        include_config: false,
        destination_root: Some("/restore".to_string()),
        archive_path: None,
        archive_base64: Some(encode_inline_file_payload(archive_bytes).unwrap()),
        archive_size_bytes: Some(archive_bytes.len() as u64),
        archive_sha256_hex: Some(payload_hash(archive_bytes)),
        dry_run: false,
        post_restore_argv: Vec::new(),
    };
    validate_job_command(&valid).unwrap();

    let unsafe_path = JobCommand::Restore {
        source_backup_request_id,
        paths: vec!["/tmp/../source.txt".to_string()],
        include_config: false,
        destination_root: Some("/restore".to_string()),
        archive_path: None,
        archive_base64: Some(encode_inline_file_payload(archive_bytes).unwrap()),
        archive_size_bytes: Some(archive_bytes.len() as u64),
        archive_sha256_hex: Some(payload_hash(archive_bytes)),
        dry_run: false,
        post_restore_argv: Vec::new(),
    };
    assert_eq!(
        validate_job_command(&unsafe_path).unwrap_err().code,
        "restore_path_invalid"
    );
}

#[test]
fn restore_rollback_job_validation_requires_safe_manifest() {
    let empty = JobCommand::RestoreRollback {
        source_restore_job_id: Uuid::new_v4(),
        restored_files: Vec::new(),
    };
    assert_eq!(
        validate_job_command(&empty).unwrap_err().code,
        "restore_rollback_files_required"
    );

    let unsafe_destination = JobCommand::RestoreRollback {
        source_restore_job_id: Uuid::new_v4(),
        restored_files: vec![RestoreRollbackFile {
            archive_path: "/tmp/source.txt".to_string(),
            destination_path: "/restore/../source.txt".to_string(),
            rollback_path: None,
            restored_size_bytes: 4,
            restored_sha256_hex: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_string(),
        }],
    };
    assert_eq!(
        validate_job_command(&unsafe_destination).unwrap_err().code,
        "restore_rollback_destination_path_invalid"
    );

    let valid = JobCommand::RestoreRollback {
        source_restore_job_id: Uuid::new_v4(),
        restored_files: vec![RestoreRollbackFile {
            archive_path: "/tmp/source.txt".to_string(),
            destination_path: "/restore/tmp/source.txt".to_string(),
            rollback_path: Some("/restore/tmp/.vpsman-restore-source.txt-job.bak".to_string()),
            restored_size_bytes: 4,
            restored_sha256_hex: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_string(),
        }],
    };
    validate_job_command(&valid).unwrap();
}

#[test]
fn restore_plan_validation_requires_scope_and_confirmation() {
    let backup_id = Uuid::new_v4();
    let missing_scope = CreateRestorePlanRequest {
        source_backup_request_id: backup_id,
        target_client_id: "client-b".to_string(),
        paths: Vec::new(),
        include_config: false,
        destination_root: None,
        confirmed: true,
        note: None,
        privilege_assertion: None,
    };
    assert_eq!(
        validate_create_restore_plan(&missing_scope)
            .unwrap_err()
            .code,
        "restore_scope_required"
    );

    let relative_path = CreateRestorePlanRequest {
        source_backup_request_id: backup_id,
        target_client_id: "client-b".to_string(),
        paths: vec!["relative".to_string()],
        include_config: false,
        destination_root: None,
        confirmed: true,
        note: None,
        privilege_assertion: None,
    };
    assert_eq!(
        validate_create_restore_plan(&relative_path)
            .unwrap_err()
            .code,
        "file_path_must_be_absolute"
    );

    let unconfirmed = CreateRestorePlanRequest {
        source_backup_request_id: backup_id,
        target_client_id: "client-b".to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: false,
        destination_root: None,
        confirmed: false,
        note: None,
        privilege_assertion: None,
    };
    assert_eq!(
        validate_create_restore_plan(&unconfirmed).unwrap_err().code,
        "restore_confirmation_required"
    );
}

#[tokio::test]
async fn restore_plan_records_metadata_and_audit_after_privilege_unlock() {
    let repo = seeded_restore_repo().await;
    let source_backup_id = create_source_backup(&repo).await;
    let state = test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;
    let request = CreateRestorePlanRequest {
        source_backup_request_id: source_backup_id,
        target_client_id: "client-b".to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        destination_root: Some("/restore".to_string()),
        confirmed: true,
        note: Some("restore rehearsal".to_string()),
        privilege_assertion: None,
    };

    let (status, Json(view)) = create_restore_plan(State(state), headers, Json(request))
        .await
        .unwrap();
    let restore_plans = repo.list_restore_plans(10).await.unwrap();
    let audits = repo.list_audit_logs(10).await.unwrap();

    assert_eq!(status, axum::http::StatusCode::CREATED);
    assert_eq!(view.source_backup_request_id, source_backup_id);
    assert_eq!(view.source_client_id, "client-a");
    assert_eq!(view.target_client_id, "client-b");
    assert_eq!(view.paths, vec!["/etc/hostname"]);
    assert!(view.include_config);
    assert_eq!(view.destination_root.as_deref(), Some("/restore"));
    assert_eq!(view.status, "planned_metadata_only");
    assert_eq!(view.command_scope, "client:client-b");
    assert_eq!(restore_plans.len(), 1);
    assert_eq!(restore_plans[0].id, view.id);
    assert!(audits
        .iter()
        .any(|audit| audit.action == "restore.planned_metadata_only"));
}

#[tokio::test]
async fn restore_plan_requires_privilege_gateway_verification() {
    let repo = seeded_restore_repo().await;
    let source_backup_id = create_source_backup(&repo).await;

    let missing = CreateRestorePlanRequest {
        source_backup_request_id: source_backup_id,
        target_client_id: "client-b".to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: false,
        destination_root: None,
        confirmed: true,
        note: None,
        privilege_assertion: None,
    };
    let state = test_state_without_privilege(repo);
    let headers = crate::test_auth_headers(&state).await;
    let missing_error = create_restore_plan(State(state), headers, Json(missing))
        .await
        .unwrap_err();
    assert_eq!(missing_error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(missing_error.code, "gateway_control_url_missing");
}

#[tokio::test]
async fn restore_rollback_degrades_unprivileged_target_without_gateway() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "client-b".to_string(),
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
    let operation = JobCommand::RestoreRollback {
        source_restore_job_id: Uuid::new_v4(),
        restored_files: vec![RestoreRollbackFile {
            archive_path: "/etc/hostname".to_string(),
            destination_path: "/restore/etc/hostname".to_string(),
            rollback_path: Some("/restore/etc/.vpsman-restore-hostname.bak".to_string()),
            restored_size_bytes: 8,
            restored_sha256_hex: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_string(),
        }],
    };
    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:client-b".to_string(),
        target_client_ids: vec!["client-b".to_string()],
        destructive: true,
        confirmed: true,
        command: "restore_rollback".to_string(),
        argv: Vec::new(),
        operation: Some(operation),
        timeout_secs: Some(60),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };

    let state = test_state_with_privilege_auto_approve(repo.clone());
    let headers = crate::test_auth_headers(&state).await;
    let (status, Json(response)) = create_job(State(state), headers, Json(request))
        .await
        .unwrap();
    wait_for_job_status(&repo, response.job_id, "partial_success").await;
    let targets = repo.list_job_targets(response.job_id).await.unwrap();
    let outputs = repo.list_job_outputs(response.job_id).await.unwrap();
    let output_bytes = BASE64_STANDARD.decode(&outputs[0].data_base64).unwrap();
    let status_output: serde_json::Value = serde_json::from_slice(&output_bytes).unwrap();

    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(response.status, "partial_success");
    assert_eq!(targets[0].status, "skipped");
    assert_eq!(
        status_output["reason"],
        "target_agent_lacks_restore_capability"
    );
}

async fn seeded_restore_repo() -> Repository {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        for client_id in ["client-a", "client-b"] {
            upsert_memory_agent(
                &memory.agents,
                &AgentHello {
                    client_id: client_id.to_string(),
                    agent_version: "test".to_string(),
                    os_release: "test".to_string(),
                    arch: "x86_64".to_string(),
                    update_heartbeat: None,
                    internal_build_number: 1,
                    capabilities: Default::default(),
                },
            )
            .await;
        }
    }
    repo
}

async fn create_source_backup(repo: &crate::repository::Repository) -> Uuid {
    let request = CreateBackupRequest {
        client_id: "client-a".to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        recipient_public_key_hex: None,
        confirmed: true,
        note: Some("source".to_string()),
        privilege_assertion: None,
    };
    let state = test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;
    let (_, Json(view)) = create_backup_request(State(state), headers, Json(request))
        .await
        .unwrap();
    view.id
}

fn test_state(repo: Repository) -> AppState {
    let (events, _) = broadcast::channel(1);
    AppState {
        repo,
        events,
        internal_token: None,
        gateway: GatewayDispatchClient::test_privilege_auto_approve(),
        backup_object_store: None,
        update_object_store: None,
        update_artifact_public_base_url: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: std::path::PathBuf::from("config/vpsman.toml"),
        dispatcher_config: crate::state::DispatcherRuntimeConfig::default(),
    }
}

fn test_state_without_privilege(repo: Repository) -> AppState {
    AppState {
        gateway: GatewayDispatchClient::default(),
        ..test_state(repo)
    }
}

fn test_state_with_privilege_auto_approve(repo: Repository) -> AppState {
    let (events, _) = broadcast::channel(1);
    AppState {
        repo,
        events,
        internal_token: None,
        gateway: GatewayDispatchClient::test_privilege_auto_approve(),
        backup_object_store: None,
        update_object_store: None,
        update_artifact_public_base_url: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: std::path::PathBuf::from("config/vpsman.toml"),
        dispatcher_config: crate::state::DispatcherRuntimeConfig::default(),
    }
}
