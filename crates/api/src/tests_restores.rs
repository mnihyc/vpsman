use std::sync::Arc;

use axum::{extract::State, http::HeaderMap, Json};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use ed25519_dalek::SigningKey;
use tokio::sync::broadcast;
use uuid::Uuid;
use vpsman_common::{
    derive_super_key, encode_inline_file_payload, encode_json, payload_hash, random_nonce,
    sign_privilege_proof, AgentCapabilitySnapshot, AgentHello, AgentPrivilegeMode, CommandEnvelope,
    JobCommand, RestoreRollbackFile,
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
    state::{AppState, EnrollmentSettings},
    unix_now,
};

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
        envelope: None,
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
        envelope: None,
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
        envelope: None,
    };
    assert_eq!(
        validate_create_restore_plan(&unconfirmed).unwrap_err().code,
        "restore_confirmation_required"
    );
}

#[tokio::test]
async fn restore_plan_records_metadata_and_audit_after_proof_envelope() {
    let repo = seeded_restore_repo().await;
    let source_backup_id = create_source_backup(&repo).await;
    let state = test_state(repo.clone());
    let request = CreateRestorePlanRequest {
        source_backup_request_id: source_backup_id,
        target_client_id: "client-b".to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        destination_root: Some("/restore".to_string()),
        confirmed: true,
        note: Some("restore rehearsal".to_string()),
        envelope: Some(restore_envelope(
            "client-b",
            source_backup_id,
            &["/etc/hostname"],
            true,
            Some("/restore"),
        )),
    };

    let (status, Json(view)) = create_restore_plan(State(state), HeaderMap::new(), Json(request))
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
    assert_eq!(view.proof_scope, "client:client-b");
    assert!(view.proof_command_id.is_some());
    assert!(view.proof_expires_unix.is_some());
    assert_eq!(restore_plans.len(), 1);
    assert_eq!(restore_plans[0].id, view.id);
    assert!(audits
        .iter()
        .any(|audit| audit.action == "restore.planned_metadata_only"));
}

#[tokio::test]
async fn restore_plan_rejects_missing_mismatched_or_expired_proof_envelope() {
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
        envelope: None,
    };
    let missing_error = create_restore_plan(
        State(test_state(repo.clone())),
        HeaderMap::new(),
        Json(missing),
    )
    .await
    .unwrap_err();
    assert_eq!(missing_error.status, axum::http::StatusCode::FORBIDDEN);
    assert_eq!(missing_error.code, "restore_proof_required");

    let mismatched = CreateRestorePlanRequest {
        source_backup_request_id: source_backup_id,
        target_client_id: "client-b".to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: false,
        destination_root: None,
        confirmed: true,
        note: None,
        envelope: Some(restore_envelope(
            "client-b",
            source_backup_id,
            &["/etc/issue"],
            false,
            None,
        )),
    };
    let mismatched_error = create_restore_plan(
        State(test_state(repo.clone())),
        HeaderMap::new(),
        Json(mismatched),
    )
    .await
    .unwrap_err();
    assert_eq!(mismatched_error.status, axum::http::StatusCode::FORBIDDEN);
    assert_eq!(mismatched_error.code, "invalid_restore_proof_envelope");
    assert!(
        repo.list_audit_logs(10)
            .await
            .unwrap()
            .iter()
            .filter(|audit| audit.action == "restore.rejected_authorization_required")
            .count()
            >= 2
    );

    let mut expired_envelope = restore_envelope(
        "client-b",
        source_backup_id,
        &["/etc/hostname"],
        false,
        None,
    );
    expired_envelope.proof.as_mut().unwrap().expires_unix = unix_now().saturating_sub(1);
    let expired = CreateRestorePlanRequest {
        source_backup_request_id: source_backup_id,
        target_client_id: "client-b".to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: false,
        destination_root: None,
        confirmed: true,
        note: None,
        envelope: Some(expired_envelope),
    };
    let expired_error =
        create_restore_plan(State(test_state(repo)), HeaderMap::new(), Json(expired))
            .await
            .unwrap_err();
    assert_eq!(expired_error.status, axum::http::StatusCode::FORBIDDEN);
    assert_eq!(expired_error.code, "invalid_restore_proof_envelope");
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
    let command_hash = payload_hash(&encode_json(&operation).unwrap());
    let request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-b".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: true,
        confirmed: true,
        command: "restore_rollback".to_string(),
        argv: Vec::new(),
        operation: Some(operation),
        timeout_secs: Some(60),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: Some(test_command_envelope("client-b", &command_hash)),
        envelopes: Default::default(),
    };

    let (status, Json(response)) = create_job(
        State(test_state_with_signing_key(repo.clone())),
        HeaderMap::new(),
        Json(request),
    )
    .await
    .unwrap();
    let targets = repo.list_job_targets(response.job_id).await.unwrap();
    let outputs = repo.list_job_outputs(response.job_id).await.unwrap();
    let output_bytes = BASE64_STANDARD.decode(&outputs[0].data_base64).unwrap();
    let status_output: serde_json::Value = serde_json::from_slice(&output_bytes).unwrap();

    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(response.accepted_targets, 0);
    assert_eq!(response.status, "degraded_unprivileged");
    assert_eq!(targets[0].status, "degraded_unprivileged");
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
                    capabilities: Default::default(),
                },
            )
            .await;
        }
    }
    repo
}

async fn create_source_backup(repo: &Repository) -> Uuid {
    let request = CreateBackupRequest {
        client_id: "client-a".to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        recipient_public_key_hex: None,
        confirmed: true,
        note: Some("source".to_string()),
        envelope: Some(backup_envelope("client-a", &["/etc/hostname"], true)),
    };
    let (_, Json(view)) = create_backup_request(
        State(test_state(repo.clone())),
        HeaderMap::new(),
        Json(request),
    )
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
        gateway: GatewayDispatchClient::default(),
        server_signing_key: None,
        enrollment: EnrollmentSettings::default(),
        backup_object_store: None,
        update_object_store: None,
        update_artifact_public_base_url: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        require_registered_agent_updates: false,
    }
}

fn test_state_with_signing_key(repo: Repository) -> AppState {
    let (events, _) = broadcast::channel(1);
    AppState {
        repo,
        events,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        server_signing_key: Some(Arc::new(SigningKey::from_bytes(&[23_u8; 32]))),
        enrollment: EnrollmentSettings::default(),
        backup_object_store: None,
        update_object_store: None,
        update_artifact_public_base_url: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        require_registered_agent_updates: false,
    }
}

fn backup_envelope(client_id: &str, paths: &[&str], include_config: bool) -> CommandEnvelope {
    let command = JobCommand::Backup {
        paths: paths.iter().map(|path| (*path).to_string()).collect(),
        include_config,
        recipient_public_key_hex: None,
    };
    command_envelope(client_id, &command, unix_now() + 60)
}

fn restore_envelope(
    client_id: &str,
    source_backup_request_id: Uuid,
    paths: &[&str],
    include_config: bool,
    destination_root: Option<&str>,
) -> CommandEnvelope {
    let command = JobCommand::Restore {
        source_backup_request_id,
        paths: paths.iter().map(|path| (*path).to_string()).collect(),
        include_config,
        destination_root: destination_root.map(ToOwned::to_owned),
        archive_path: None,
        archive_base64: None,
        archive_size_bytes: None,
        archive_sha256_hex: None,
        dry_run: false,
        post_restore_argv: Vec::new(),
    };
    command_envelope(client_id, &command, unix_now() + 60)
}

fn command_envelope(client_id: &str, command: &JobCommand, expires_unix: u64) -> CommandEnvelope {
    let payload_hash_hex = payload_hash(&encode_json(command).unwrap());
    let command_id = Uuid::new_v4();
    let scope = format!("client:{client_id}");
    let super_key = derive_super_key("correct horse", b"restore-test");
    let proof = sign_privilege_proof(
        &super_key,
        command_id,
        &scope,
        &payload_hash_hex,
        &random_nonce(),
        expires_unix,
    );
    CommandEnvelope {
        command_id,
        scope,
        payload_hash_hex,
        proof: Some(proof),
        server_signature: Vec::new(),
    }
}

fn test_command_envelope(client_id: &str, command_hash: &str) -> CommandEnvelope {
    let command_id = Uuid::new_v4();
    let scope = format!("client:{client_id}");
    let super_key = derive_super_key("correct horse", b"restore-test");
    let proof = sign_privilege_proof(
        &super_key,
        command_id,
        &scope,
        command_hash,
        &random_nonce(),
        unix_now() + 300,
    );
    CommandEnvelope {
        command_id,
        scope,
        payload_hash_hex: command_hash.to_string(),
        proof: Some(proof),
        server_signature: Vec::new(),
    }
}
