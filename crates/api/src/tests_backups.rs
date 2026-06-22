use axum::{
    body::{to_bytes, Body},
    extract::{Path, State},
    http::{
        header::{AUTHORIZATION, CONTENT_TYPE},
        Request, StatusCode,
    },
    Json,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use std::io::Write;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::broadcast,
};
use tower::ServiceExt;
use uuid::Uuid;
use vpsman_common::{
    encode_json, payload_hash, AgentHello, CommandOutput, GatewayCommandDispatch,
    GatewayCommandDispatchResult, JobCommand, OutputStream,
};

use crate::{
    gateway_client::GatewayDispatchClient,
    job_request::validate_job_command,
    model::{
        AuthContext, BackupArtifactHandoffRequest, BackupArtifactUploadChunkRequest,
        BackupArtifactUploadCommitRequest, BackupArtifactUploadSessionCreateRequest,
        BackupPolicyPruneRequest, BackupRequestStatus, CreateBackupPolicyRequest,
        CreateBackupRequest, CreateJobRequest, JobHistoryView, JobOutputView, JobTargetView,
        OperatorView, RecordBackupArtifactMetadataRequest, UploadBackupArtifactRequest,
    },
    object_store::BackupObjectStore,
    repository::{MemoryState, Repository},
    repository_backups::BackupRequestSourceLink,
    repository_ingest::upsert_memory_agent,
    repository_job_outputs,
    routes_backups::{
        abort_backup_artifact_upload_session, commit_backup_artifact_upload_session,
        create_backup_artifact_handoff, create_backup_artifact_upload_session,
        create_backup_policy, create_backup_request, download_backup_artifact,
        list_backup_policies, prune_backup_policies, record_backup_artifact_metadata,
        upload_backup_artifact, upload_backup_artifact_session_chunk,
        validate_backup_artifact_metadata_request, validate_create_backup_policy_request,
        validate_create_backup_request,
    },
    routes_jobs::create_job,
    state::AppState,
    unix_now, TargetDispatchOutcome,
};

const TEST_INTERNAL_TOKEN: &str = "test-internal-token-value-32-plus-chars";

#[test]
fn backup_request_validation_requires_safe_scope_and_confirmation() {
    let missing_scope = CreateBackupRequest {
        client_id: "client-a".to_string(),
        paths: Vec::new(),
        include_config: false,
        follow_symlinks: false,
        confirmed: true,
        note: None,
        privilege_assertion: None,
    };
    assert_eq!(
        validate_create_backup_request(&missing_scope)
            .unwrap_err()
            .code,
        "backup_scope_required"
    );

    let relative_path = CreateBackupRequest {
        client_id: "client-a".to_string(),
        paths: vec!["relative".to_string()],
        include_config: false,
        follow_symlinks: false,
        confirmed: true,
        note: None,
        privilege_assertion: None,
    };
    assert_eq!(
        validate_create_backup_request(&relative_path)
            .unwrap_err()
            .code,
        "file_path_must_be_absolute"
    );

    let unconfirmed = CreateBackupRequest {
        client_id: "client-a".to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: false,
        follow_symlinks: false,
        confirmed: false,
        note: None,
        privilege_assertion: None,
    };
    assert_eq!(
        validate_create_backup_request(&unconfirmed)
            .unwrap_err()
            .code,
        "backup_confirmation_required"
    );
}

#[test]
fn backup_policy_validation_requires_targets_retention_and_confirmation() {
    let mut request = CreateBackupPolicyRequest {
        name: "nightly".to_string(),
        selector_expression: "tag:edge".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        follow_symlinks: false,
        retention_days: Some(30),
        keep_last: Some(7),
        rotation_generation: Some("keyring/v2".to_string()),
        cron_expr: "0 3 * * *".to_string(),
        timezone: "UTC".to_string(),
        enabled: true,
        catch_up_policy: "skip_missed".to_string(),
        catch_up_limit: 1,
        retry_delay_secs: 300,
        max_failures: 3,
        confirmed: true,
        privilege_assertion: None,
    };

    validate_create_backup_policy_request(&request).unwrap();
    request.confirmed = false;
    assert_eq!(
        validate_create_backup_policy_request(&request)
            .unwrap_err()
            .code,
        "backup_policy_confirmation_required"
    );
    request.confirmed = true;
    request.retention_days = Some(0);
    assert_eq!(
        validate_create_backup_policy_request(&request)
            .unwrap_err()
            .code,
        "backup_policy_retention_days_out_of_range"
    );
    request.retention_days = Some(30);
    request.keep_last = Some(0);
    assert_eq!(
        validate_create_backup_policy_request(&request)
            .unwrap_err()
            .code,
        "backup_policy_keep_last_out_of_range"
    );
}

#[test]
fn backup_artifact_metadata_validation_requires_safe_metadata() {
    let unconfirmed = RecordBackupArtifactMetadataRequest {
        object_key: "backups/client-a/artifact.tar".to_string(),
        sha256_hex: "a".repeat(64),
        size_bytes: 128,
        confirmed: false,
    };
    assert_eq!(
        validate_backup_artifact_metadata_request(&unconfirmed)
            .unwrap_err()
            .code,
        "backup_artifact_confirmation_required"
    );

    let unsafe_key = RecordBackupArtifactMetadataRequest {
        object_key: "../artifact".to_string(),
        sha256_hex: "a".repeat(64),
        size_bytes: 128,
        confirmed: true,
    };
    assert_eq!(
        validate_backup_artifact_metadata_request(&unsafe_key)
            .unwrap_err()
            .code,
        "backup_artifact_object_key_invalid"
    );

    let bad_hash = RecordBackupArtifactMetadataRequest {
        object_key: "backups/client-a/artifact.tar".to_string(),
        sha256_hex: "not-a-hash".to_string(),
        size_bytes: 128,
        confirmed: true,
    };
    assert_eq!(
        validate_backup_artifact_metadata_request(&bad_hash)
            .unwrap_err()
            .code,
        "backup_artifact_invalid_sha256"
    );
}

#[test]
fn backup_job_command_validates_executable_scope() {
    validate_job_command(&JobCommand::Backup {
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        follow_symlinks: false,
    })
    .unwrap();
    assert_eq!(
        validate_job_command(&JobCommand::Backup {
            paths: Vec::new(),
            include_config: false,
            follow_symlinks: false,
        })
        .unwrap_err()
        .code,
        "backup_scope_required"
    );
    assert_eq!(
        validate_job_command(&JobCommand::Backup {
            paths: vec!["relative".to_string()],
            include_config: false,
            follow_symlinks: false,
        })
        .unwrap_err()
        .code,
        "file_path_must_be_absolute"
    );
}

#[tokio::test]
async fn backup_job_dispatch_requires_confirmation() {
    let request = CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: "backup".to_string(),
        argv: Vec::new(),
        operation: Some(JobCommand::Backup {
            paths: vec!["/etc/hostname".to_string()],
            include_config: true,
            follow_symlinks: false,
        }),
        timeout_secs: Some(30),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };

    let state = test_state(Repository::Memory(MemoryState::default()));
    let headers = crate::test_auth_headers(&state).await;
    let error = create_job(State(state), headers, Json(request))
        .await
        .unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(error.code, "backup_confirmation_required");
}

#[tokio::test]
async fn backup_job_dispatch_auto_records_request_and_object_artifact() {
    let repo = Repository::Memory(MemoryState::default());
    seed_backup_agent(&repo).await;
    let object_root = std::env::temp_dir().join(format!(
        "vpsman-api-backup-job-auto-record-{}",
        Uuid::new_v4()
    ));
    let artifact_bytes = plain_backup_artifact_bytes_with_payload("client-a", &vec![b'x'; 48_000]);
    let (gateway_url, gateway_task) = spawn_backup_gateway_once(artifact_bytes.clone()).await;
    let mut state = test_state_with_store(
        repo.clone(),
        BackupObjectStore::filesystem(object_root.clone()).unwrap(),
    );
    state.gateway =
        GatewayDispatchClient::new(Some(gateway_url), Some(TEST_INTERNAL_TOKEN.to_string()))
            .with_test_privilege_auto_approve();
    let operation = JobCommand::Backup {
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        follow_symlinks: false,
    };
    let request = CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: true,
        command: "backup".to_string(),
        argv: Vec::new(),
        operation: Some(operation.clone()),
        timeout_secs: Some(30),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };

    let headers = crate::test_auth_headers(&state).await;
    let (status, Json(response)) = create_job(State(state), headers, Json(request))
        .await
        .unwrap();
    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(response.status, "queued");
    let dispatch = gateway_task.await.unwrap();
    wait_for_job_status(&repo, response.job_id, "completed").await;
    let backups = repo.list_backup_requests(10).await.unwrap();
    let artifacts = repo.list_backup_artifacts(10).await.unwrap();
    let outputs = repo.list_job_outputs(response.job_id).await.unwrap();

    assert_eq!(dispatch.client_id, "client-a");
    assert_eq!(
        encode_json(&dispatch.request.command).unwrap(),
        encode_json(&operation).unwrap()
    );
    assert_eq!(backups.len(), 1);
    assert_eq!(backups[0].client_id, "client-a");
    assert_eq!(backups[0].paths, vec!["/etc/hostname"]);
    assert!(backups[0].include_config);
    assert_eq!(backups[0].status, "artifact_metadata_recorded");
    let expected_note = format!("auto-linked from backup job {}", response.job_id);
    assert_eq!(backups[0].note.as_deref(), Some(expected_note.as_str()));
    assert_eq!(artifacts.len(), 1);
    assert_eq!(backups[0].artifact_id, Some(artifacts[0].id));
    assert_eq!(artifacts[0].sha256_hex, payload_hash(&artifact_bytes));
    let stdout_output = outputs
        .iter()
        .find(|output| output.stream == "stdout")
        .expect("backup stdout job output");
    assert_eq!(stdout_output.storage, "object_store");
    assert_eq!(
        stdout_output.artifact_sha256_hex.as_deref(),
        Some(payload_hash(&artifact_bytes).as_str())
    );
    assert_eq!(
        stdout_output.artifact_size_bytes,
        Some(artifact_bytes.len() as i64)
    );
    assert_eq!(
        tokio::fs::read(object_root.join(&artifacts[0].object_key))
            .await
            .unwrap(),
        artifact_bytes
    );

    let _ = tokio::fs::remove_dir_all(object_root).await;
}

#[tokio::test]
async fn backup_job_dispatch_terminal_failure_marks_backup_request_failed() {
    let repo = Repository::Memory(MemoryState::default());
    seed_backup_agent(&repo).await;
    let source_job_id = Uuid::new_v4();
    let operator = backup_test_operator();
    let request = CreateBackupRequest {
        client_id: "client-a".to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        follow_symlinks: false,
        confirmed: true,
        note: Some("source job request".to_string()),
        privilege_assertion: None,
    };
    let operation = JobCommand::Backup {
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        follow_symlinks: false,
    };
    let command_hash = payload_hash(&encode_json(&operation).unwrap());
    let backup = repo
        .record_backup_request_with_source(
            &request,
            &command_hash,
            "client:client-a",
            &operator,
            BackupRequestStatus::RequestedMetadataOnly,
            BackupRequestSourceLink {
                job_id: Some(source_job_id),
                schedule_id: None,
            },
        )
        .await
        .unwrap();
    let terminal = repo
        .mark_open_backup_request_execution_terminal(
            source_job_id,
            "client-a",
            BackupRequestStatus::ExecutionFailed,
            Some(&operator),
        )
        .await
        .unwrap();
    let backups = repo.list_backup_requests(10).await.unwrap();
    let audits = repo.list_audit_logs(10).await.unwrap();

    assert_eq!(terminal.as_ref().map(|view| view.id), Some(backup.id));
    assert_eq!(backups.len(), 1);
    assert_eq!(backups[0].status, "execution_failed");
    assert!(backups[0].artifact_id.is_none());
    assert!(audits
        .iter()
        .any(|audit| audit.action == "backup.execution_failed"));
}

#[tokio::test]
async fn async_backup_final_failure_marks_backup_request_failed() {
    let repo = Repository::Memory(MemoryState::default());
    seed_backup_agent(&repo).await;
    let operator = backup_test_operator();
    let operation = JobCommand::Backup {
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        follow_symlinks: false,
    };
    let command_hash = payload_hash(&encode_json(&operation).unwrap());
    let request = CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: true,
        confirmed: true,
        command: "backup".to_string(),
        argv: Vec::new(),
        operation: Some(operation),
        timeout_secs: Some(30),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };
    let job_id = repo
        .record_dispatching_job(
            request.job_id.unwrap(),
            &request,
            &command_hash,
            "async_backup_failure",
            &operator,
            &["client-a".to_string()],
        )
        .await
        .unwrap();
    repo.claim_due_job_targets(10, 30, 0).await.unwrap();
    let backup_request = CreateBackupRequest {
        client_id: "client-a".to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        follow_symlinks: false,
        confirmed: true,
        note: Some("source job request".to_string()),
        privilege_assertion: None,
    };
    repo.record_backup_request_with_source(
        &backup_request,
        &command_hash,
        "client:client-a",
        &operator,
        BackupRequestStatus::RequestedMetadataOnly,
        BackupRequestSourceLink {
            job_id: Some(job_id),
            schedule_id: None,
        },
    )
    .await
    .unwrap();
    let output = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: br#"{"type":"backup","status":"failed","message":"disk full"}"#.to_vec(),
        exit_code: Some(1),
        done: true,
    };
    let outcome = TargetDispatchOutcome {
        status: "failed".to_string(),
        exit_code: Some(1),
        command_version: Some(1),
        accepted: true,
        message: "disk full".to_string(),
        received_at: Some("1700000000".to_string()),
        outputs: vec![output.clone()],
    };

    let result = repo
        .record_active_final_job_output_and_target_result_with_config(
            job_id,
            "client-a",
            0,
            &output,
            outcome.received_at.clone(),
            repository_job_outputs::JobOutputPersistConfig {
                object_store: None,
                artifact_min_bytes: usize::MAX,
            },
            &outcome,
        )
        .await
        .unwrap();

    assert!(result.target_terminalized);
    let backups = repo.list_backup_requests(10).await.unwrap();
    assert_eq!(backups.len(), 1);
    assert_eq!(backups[0].status, "execution_failed");
}

#[tokio::test]
async fn backup_job_dispatch_reuses_existing_open_backup_request() {
    let repo = Repository::Memory(MemoryState::default());
    seed_backup_agent(&repo).await;
    let request_state = test_state(repo.clone());
    let request_headers = crate::test_auth_headers(&request_state).await;
    let manual_request = CreateBackupRequest {
        client_id: "client-a".to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        follow_symlinks: false,
        confirmed: true,
        note: Some("operator-requested".to_string()),
        privilege_assertion: None,
    };
    let (_, Json(manual_backup)) =
        create_backup_request(State(request_state), request_headers, Json(manual_request))
            .await
            .unwrap();
    let operation = JobCommand::Backup {
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        follow_symlinks: false,
    };
    let (gateway_url, gateway_task) =
        spawn_backup_gateway_once(plain_backup_artifact_bytes("client-a")).await;
    let mut state = test_state(repo.clone());
    state.gateway =
        GatewayDispatchClient::new(Some(gateway_url), Some(TEST_INTERNAL_TOKEN.to_string()))
            .with_test_privilege_auto_approve();
    let headers = crate::test_auth_headers(&state).await;
    let job_request = CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: true,
        command: "backup".to_string(),
        argv: Vec::new(),
        operation: Some(operation.clone()),
        timeout_secs: Some(30),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };

    let (_status, Json(response)) = create_job(State(state.clone()), headers, Json(job_request))
        .await
        .unwrap();
    assert_eq!(response.status, "queued");
    crate::job_dispatcher::dispatch_due_job_targets(&state)
        .await
        .unwrap();
    let _dispatch = tokio::time::timeout(std::time::Duration::from_secs(2), gateway_task)
        .await
        .expect("backup gateway dispatch was not attempted")
        .unwrap();
    wait_for_job_status(&repo, response.job_id, "completed").await;
    let backups = repo.list_backup_requests(10).await.unwrap();

    assert_eq!(backups.len(), 1);
    assert_eq!(backups[0].id, manual_backup.id);
    assert_eq!(backups[0].note.as_deref(), Some("operator-requested"));
    assert_eq!(backups[0].status, "requested_metadata_only");
}

#[tokio::test]
async fn backup_request_records_metadata_and_audit_after_privilege_unlock() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "client-a".to_string(),
                process_incarnation_id: uuid::Uuid::new_v4(),
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
    let object_root =
        std::env::temp_dir().join(format!("vpsman-backup-metadata-{}", uuid::Uuid::new_v4()));
    let store = BackupObjectStore::filesystem(object_root.clone()).unwrap();
    let state = test_state_with_store(repo.clone(), store);
    let headers = crate::test_auth_headers(&state).await;
    let request = CreateBackupRequest {
        client_id: "client-a".to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        follow_symlinks: false,
        confirmed: true,
        note: Some("pre-migration".to_string()),
        privilege_assertion: None,
    };

    let (status, Json(view)) = create_backup_request(State(state), headers, Json(request))
        .await
        .unwrap();
    let backups = repo.list_backup_requests(10).await.unwrap();
    let audits = repo.list_audit_logs(10).await.unwrap();

    assert_eq!(status, axum::http::StatusCode::CREATED);
    assert_eq!(view.client_id, "client-a");
    assert_eq!(view.paths, vec!["/etc/hostname"]);
    assert!(view.include_config);
    assert_eq!(view.status, "requested_metadata_only");
    assert_eq!(view.command_scope, "client:client-a");
    assert!(view.artifact_id.is_none());
    assert_eq!(backups.len(), 1);
    assert_eq!(backups[0].id, view.id);
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].action, "backup.requested_metadata_only");
    assert_eq!(
        audits[0].command_hash.as_deref(),
        Some(view.payload_hash.as_str())
    );
}

#[tokio::test]
async fn backup_policy_upsert_records_schedule_metadata_and_audit() {
    let repo = Repository::Memory(MemoryState::default());
    seed_backup_agent_id(&repo, "client-a").await;
    seed_backup_agent_id(&repo, "client-b").await;
    let object_root =
        std::env::temp_dir().join(format!("vpsman-backup-metadata-{}", Uuid::new_v4()));
    let store = BackupObjectStore::filesystem(object_root.clone()).unwrap();
    let state = test_state_with_store(repo.clone(), store);
    let headers = crate::test_auth_headers(&state).await;
    let request = CreateBackupPolicyRequest {
        name: "nightly-edge".to_string(),
        selector_expression: "id:client-a || tag:edge".to_string(),
        target_client_ids: vec!["client-a".to_string(), "client-b".to_string()],
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        follow_symlinks: false,
        retention_days: Some(45),
        keep_last: Some(12),
        rotation_generation: Some("keyring/v2".to_string()),
        cron_expr: "0 3 * * *".to_string(),
        timezone: "UTC".to_string(),
        enabled: true,
        catch_up_policy: "run_once".to_string(),
        catch_up_limit: 1,
        retry_delay_secs: 120,
        max_failures: 5,
        confirmed: true,
        privilege_assertion: None,
    };

    let (status, Json(view)) =
        create_backup_policy(State(state.clone()), headers.clone(), Json(request))
            .await
            .unwrap();
    let Json(policies) = list_backup_policies(State(state), headers).await.unwrap();
    let schedules = repo.list_schedules().await.unwrap();
    let audits = repo.list_audit_logs(10).await.unwrap();
    let audit_json = serde_json::to_string(&audits).unwrap();

    assert_eq!(status, axum::http::StatusCode::CREATED);
    assert_eq!(view.name, "nightly-edge");
    assert_eq!(view.selector_expression, "id:client-a || tag:edge");
    assert_eq!(view.target_client_ids, vec!["client-a", "client-b"]);
    assert_eq!(view.paths, vec!["/etc/hostname"]);
    assert!(view.include_config);
    assert_eq!(view.retention_days, 45);
    assert_eq!(view.keep_last, 12);
    assert_eq!(view.rotation_generation.as_deref(), Some("keyring/v2"));
    assert_eq!(view.cron_expr, "0 3 * * *");
    assert_eq!(view.timezone, "UTC");
    assert_eq!(view.next_runs.len(), 5);
    assert_eq!(policies.len(), 1);
    assert_eq!(policies[0].schedule_id, view.schedule_id);
    assert_eq!(schedules.len(), 1);
    assert_eq!(schedules[0].command_type, "backup");
    assert!(matches!(
        &schedules[0].operation,
        JobCommand::Backup {
            paths,
            include_config,
            ..
        } if paths == &vec!["/etc/hostname".to_string()] && *include_config
    ));
    assert!(audits
        .iter()
        .any(|audit| audit.action == "backup_policy.upserted"));
    assert!(!audit_json.contains("recipient_public_key"));
}

#[tokio::test]
async fn backup_policy_prune_applies_retention_and_keep_last_per_client() {
    let repo = Repository::Memory(MemoryState::default());
    seed_backup_agent(&repo).await;
    let object_root =
        std::env::temp_dir().join(format!("vpsman-api-backup-policy-prune-{}", Uuid::new_v4()));
    let state = test_state_with_store(
        repo.clone(),
        BackupObjectStore::filesystem(object_root.clone()).unwrap(),
    );
    let headers = crate::test_auth_headers(&state).await;
    let (_, Json(policy)) = create_backup_policy(
        State(state.clone()),
        headers.clone(),
        Json(CreateBackupPolicyRequest {
            name: "nightly-prune".to_string(),
            selector_expression: "id:client-a".to_string(),
            target_client_ids: vec!["client-a".to_string()],
            paths: vec!["/etc/hostname".to_string()],
            include_config: true,
            follow_symlinks: false,
            retention_days: Some(1),
            keep_last: Some(1),
            rotation_generation: None,
            cron_expr: "0 3 * * *".to_string(),
            timezone: "UTC".to_string(),
            enabled: true,
            catch_up_policy: "skip_missed".to_string(),
            catch_up_limit: 1,
            retry_delay_secs: 120,
            max_failures: 3,
            confirmed: true,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap();
    let old_a = seed_policy_backup_artifact(
        &repo,
        state.backup_object_store.as_ref().unwrap(),
        policy.schedule_id,
        "old-a",
        3,
    )
    .await;
    let old_b = seed_policy_backup_artifact(
        &repo,
        state.backup_object_store.as_ref().unwrap(),
        policy.schedule_id,
        "old-b",
        2,
    )
    .await;
    let retained = seed_policy_backup_artifact(
        &repo,
        state.backup_object_store.as_ref().unwrap(),
        policy.schedule_id,
        "retained",
        0,
    )
    .await;

    let Json(dry_run) = prune_backup_policies(
        State(state.clone()),
        headers.clone(),
        Json(BackupPolicyPruneRequest {
            schedule_id: Some(policy.schedule_id),
            dry_run: true,
            metadata_only: Some(false),
            preview_hash: None,
            confirmed: false,
        }),
    )
    .await
    .unwrap();
    assert_eq!(dry_run.policies.len(), 1);
    assert_eq!(dry_run.policies[0].matched_rows, 2);
    assert_eq!(dry_run.policies[0].pruned_rows, 0);
    assert_eq!(repo.list_backup_artifacts(10).await.unwrap().len(), 3);

    let Json(pruned) = prune_backup_policies(
        State(state.clone()),
        headers,
        Json(BackupPolicyPruneRequest {
            schedule_id: Some(policy.schedule_id),
            dry_run: false,
            metadata_only: Some(false),
            preview_hash: Some(dry_run.preview_hash.clone()),
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    assert_eq!(pruned.policies[0].matched_rows, 2);
    assert_eq!(pruned.policies[0].pruned_rows, 2);
    assert!(pruned.policies[0].object_delete_attempted);
    assert_eq!(repo.list_backup_artifacts(10).await.unwrap().len(), 1);
    let backups = repo.list_backup_requests(10).await.unwrap();
    assert_eq!(
        backups
            .iter()
            .filter(|backup| backup.artifact_id.is_some())
            .count(),
        1
    );
    assert!(!tokio::fs::try_exists(object_root.join(&old_a))
        .await
        .unwrap());
    assert!(!tokio::fs::try_exists(object_root.join(&old_b))
        .await
        .unwrap());
    assert!(tokio::fs::try_exists(object_root.join(&retained))
        .await
        .unwrap());
    assert!(repo
        .list_audit_logs(20)
        .await
        .unwrap()
        .iter()
        .any(|audit| audit.action == "backup_policy.retention_pruned"));

    let _ = tokio::fs::remove_dir_all(object_root).await;
}

#[tokio::test]
async fn backup_policy_prune_partial_error_preserves_metadata_after_delete_failure() {
    let repo = Repository::Memory(MemoryState::default());
    seed_backup_agent(&repo).await;
    let object_root = std::env::temp_dir().join(format!(
        "vpsman-api-backup-policy-prune-partial-{}",
        Uuid::new_v4()
    ));
    let state = test_state_with_store(
        repo.clone(),
        BackupObjectStore::filesystem(object_root.clone()).unwrap(),
    );
    let headers = crate::test_auth_headers(&state).await;
    let (_, Json(policy)) = create_backup_policy(
        State(state.clone()),
        headers.clone(),
        Json(CreateBackupPolicyRequest {
            name: "nightly-prune-partial".to_string(),
            selector_expression: "id:client-a".to_string(),
            target_client_ids: vec!["client-a".to_string()],
            paths: vec!["/etc/hostname".to_string()],
            include_config: true,
            follow_symlinks: false,
            retention_days: Some(1),
            keep_last: Some(1),
            rotation_generation: None,
            cron_expr: "0 3 * * *".to_string(),
            timezone: "UTC".to_string(),
            enabled: true,
            catch_up_policy: "skip_missed".to_string(),
            catch_up_limit: 1,
            retry_delay_secs: 120,
            max_failures: 3,
            confirmed: true,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap();
    let missing_ok = seed_policy_backup_artifact(
        &repo,
        state.backup_object_store.as_ref().unwrap(),
        policy.schedule_id,
        "missing-ok",
        3,
    )
    .await;
    let delete_fails = seed_policy_backup_artifact(
        &repo,
        state.backup_object_store.as_ref().unwrap(),
        policy.schedule_id,
        "delete-fails",
        2,
    )
    .await;
    let retained = seed_policy_backup_artifact(
        &repo,
        state.backup_object_store.as_ref().unwrap(),
        policy.schedule_id,
        "partial-retained",
        0,
    )
    .await;
    tokio::fs::remove_file(object_root.join(&missing_ok))
        .await
        .unwrap();
    tokio::fs::remove_file(object_root.join(&delete_fails))
        .await
        .unwrap();
    tokio::fs::create_dir(object_root.join(&delete_fails))
        .await
        .unwrap();

    let Json(dry_run) = prune_backup_policies(
        State(state.clone()),
        headers.clone(),
        Json(BackupPolicyPruneRequest {
            schedule_id: Some(policy.schedule_id),
            dry_run: true,
            metadata_only: Some(false),
            preview_hash: None,
            confirmed: false,
        }),
    )
    .await
    .unwrap();

    let Json(pruned) = prune_backup_policies(
        State(state.clone()),
        headers,
        Json(BackupPolicyPruneRequest {
            schedule_id: Some(policy.schedule_id),
            dry_run: false,
            metadata_only: Some(false),
            preview_hash: Some(dry_run.preview_hash),
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    let policy_result = &pruned.policies[0];
    assert_eq!(policy_result.status, "partial_error");
    assert_eq!(policy_result.matched_rows, 2);
    assert_eq!(policy_result.pruned_rows, 1);
    assert_eq!(
        policy_result.object_keys,
        vec![missing_ok.clone(), delete_fails.clone()]
    );
    assert_eq!(policy_result.object_delete_errors.len(), 1);
    assert!(policy_result.object_delete_errors[0].contains(&delete_fails));
    let artifacts = repo.list_backup_artifacts(10).await.unwrap();
    assert_eq!(artifacts.len(), 2);
    assert!(artifacts
        .iter()
        .any(|artifact| artifact.object_key == retained));
    assert!(artifacts
        .iter()
        .any(|artifact| artifact.object_key == delete_fails));
    let backups = repo.list_backup_requests(10).await.unwrap();
    assert_eq!(
        backups
            .iter()
            .filter(|backup| backup.artifact_id.is_some())
            .count(),
        2
    );

    let _ = tokio::fs::remove_dir_all(object_root).await;
}

#[tokio::test]
async fn backup_artifact_metadata_links_request_and_audits() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "client-a".to_string(),
                process_incarnation_id: uuid::Uuid::new_v4(),
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
    let object_root =
        std::env::temp_dir().join(format!("vpsman-backup-metadata-{}", Uuid::new_v4()));
    let store = BackupObjectStore::filesystem(object_root.clone()).unwrap();
    let state = test_state_with_store(repo.clone(), store);
    let headers = crate::test_auth_headers(&state).await;
    let request = CreateBackupRequest {
        client_id: "client-a".to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        follow_symlinks: false,
        confirmed: true,
        note: Some("pre-migration".to_string()),
        privilege_assertion: None,
    };
    let (_, Json(backup)) =
        create_backup_request(State(state.clone()), headers.clone(), Json(request))
            .await
            .unwrap();

    let artifact_bytes = plain_backup_artifact_bytes("client-a");
    let object_key = format!("backups/{}/{}.tar", backup.client_id, backup.id);
    state
        .backup_object_store
        .as_ref()
        .unwrap()
        .put_new(&object_key, &artifact_bytes)
        .await
        .unwrap();
    let artifact_request = RecordBackupArtifactMetadataRequest {
        object_key,
        sha256_hex: payload_hash(&artifact_bytes),
        size_bytes: artifact_bytes.len() as i64,
        confirmed: true,
    };
    let (status, Json(artifact)) = record_backup_artifact_metadata(
        State(state.clone()),
        headers.clone(),
        Path(backup.id),
        Json(artifact_request),
    )
    .await
    .unwrap();

    let backups = repo.list_backup_requests(10).await.unwrap();
    let artifacts = repo.list_backup_artifacts(10).await.unwrap();
    let audits = repo.list_audit_logs(10).await.unwrap();

    assert_eq!(status, axum::http::StatusCode::CREATED);
    assert_eq!(artifact.client_id, "client-a");
    assert_eq!(artifact.sha256_hex, payload_hash(&artifact_bytes));
    assert_eq!(artifact.size_bytes, artifact_bytes.len() as i64);
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].id, artifact.id);
    assert_eq!(backups[0].id, backup.id);
    assert_eq!(backups[0].artifact_id, Some(artifact.id));
    assert_eq!(backups[0].status, "artifact_metadata_recorded");
    assert!(audits
        .iter()
        .any(|audit| audit.action == "backup.artifact_metadata_recorded"));

    let duplicate = RecordBackupArtifactMetadataRequest {
        object_key: format!("backups/{}/{}-duplicate.tar", backup.client_id, backup.id),
        sha256_hex: "c".repeat(64),
        size_bytes: 4096,
        confirmed: true,
    };
    let error =
        record_backup_artifact_metadata(State(state), headers, Path(backup.id), Json(duplicate))
            .await
            .unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(error.code, "backup_artifact_already_recorded");
    let _ = tokio::fs::remove_dir_all(object_root).await;
}

#[tokio::test]
async fn backup_artifact_upload_stores_bytes_and_links_metadata() {
    let repo = Repository::Memory(MemoryState::default());
    seed_backup_agent(&repo).await;
    let object_root =
        std::env::temp_dir().join(format!("vpsman-api-backup-upload-{}", Uuid::new_v4()));
    let state = test_state_with_store(
        repo.clone(),
        BackupObjectStore::filesystem(object_root.clone()).unwrap(),
    );
    let backup = create_test_backup_request(&repo, state.clone()).await;
    let artifact_bytes = plain_backup_artifact_bytes("client-a");
    let object_key = format!("backups/{}/{}.tar", backup.client_id, backup.id);
    let headers = crate::test_auth_headers(&state).await;

    let (status, Json(artifact)) = upload_backup_artifact(
        State(state.clone()),
        headers.clone(),
        Path(backup.id),
        Json(UploadBackupArtifactRequest {
            object_key: object_key.clone(),
            artifact_base64: BASE64.encode(&artifact_bytes),
            confirmed: true,
        }),
    )
    .await
    .unwrap();

    let stored_path = object_root.join(&object_key);
    let backups = repo.list_backup_requests(10).await.unwrap();
    let artifacts = repo.list_backup_artifacts(10).await.unwrap();
    let audits = repo.list_audit_logs(10).await.unwrap();

    assert_eq!(status, axum::http::StatusCode::CREATED);
    assert_eq!(artifact.client_id, "client-a");
    assert_eq!(artifact.object_key, object_key);
    assert_eq!(artifact.sha256_hex, payload_hash(&artifact_bytes));
    assert_eq!(artifact.size_bytes, artifact_bytes.len() as i64);
    assert_eq!(tokio::fs::read(stored_path).await.unwrap(), artifact_bytes);
    let download = download_backup_artifact(State(state.clone()), headers.clone(), Path(backup.id))
        .await
        .unwrap();
    assert_eq!(
        download
            .headers()
            .get("x-vpsman-backup-artifact-sha256")
            .unwrap()
            .to_str()
            .unwrap(),
        payload_hash(&artifact_bytes)
    );
    let downloaded = to_bytes(download.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(downloaded.as_ref(), artifact_bytes);
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].id, artifact.id);
    assert_eq!(backups[0].artifact_id, Some(artifact.id));
    assert_eq!(backups[0].status, "artifact_metadata_recorded");
    assert!(audits
        .iter()
        .any(|audit| audit.action == "backup.artifact_metadata_recorded"));

    let duplicate = upload_backup_artifact(
        State(state),
        headers,
        Path(backup.id),
        Json(UploadBackupArtifactRequest {
            object_key: format!("backups/{}/{}-duplicate.tar", backup.client_id, backup.id),
            artifact_base64: BASE64.encode(plain_backup_artifact_bytes("client-a")),
            confirmed: true,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(duplicate.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(duplicate.code, "backup_artifact_already_recorded");

    let _ = tokio::fs::remove_dir_all(object_root).await;
}

#[tokio::test]
async fn backup_artifact_upload_session_stages_chunks_and_commits_artifact() {
    let repo = Repository::Memory(MemoryState::default());
    seed_backup_agent(&repo).await;
    let object_root = std::env::temp_dir().join(format!(
        "vpsman-api-backup-upload-session-{}",
        Uuid::new_v4()
    ));
    let state = test_state_with_store(
        repo.clone(),
        BackupObjectStore::filesystem(object_root.clone()).unwrap(),
    );
    let backup = create_test_backup_request(&repo, state.clone()).await;
    let artifact_bytes = plain_backup_artifact_bytes("client-a");
    let artifact_sha = payload_hash(&artifact_bytes);
    let object_key = format!("backups/{}/{}-chunked.tar", backup.client_id, backup.id);
    let headers = crate::test_auth_headers(&state).await;

    let (status, Json(session)) = create_backup_artifact_upload_session(
        State(state.clone()),
        headers.clone(),
        Path(backup.id),
        Json(BackupArtifactUploadSessionCreateRequest {
            object_key: object_key.clone(),
            expected_sha256_hex: artifact_sha.clone(),
            expected_size_bytes: artifact_bytes.len() as i64,
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    assert_eq!(status, axum::http::StatusCode::CREATED);
    assert_eq!(session.next_offset_bytes, 0);
    assert_eq!(session.status, "receiving");
    assert!(session.max_chunk_bytes >= 1024);

    let first_len = artifact_bytes.len() / 2;
    let first = upload_backup_artifact_session_chunk(
        State(state.clone()),
        headers.clone(),
        Path((backup.id, session.upload_id)),
        Json(BackupArtifactUploadChunkRequest {
            offset_bytes: 0,
            data_base64: BASE64.encode(&artifact_bytes[..first_len]),
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(first.next_offset_bytes, first_len as i64);
    assert_eq!(first.chunk_count, 1);

    let retry = upload_backup_artifact_session_chunk(
        State(state.clone()),
        headers.clone(),
        Path((backup.id, session.upload_id)),
        Json(BackupArtifactUploadChunkRequest {
            offset_bytes: 0,
            data_base64: BASE64.encode(&artifact_bytes[..first_len]),
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(retry.next_offset_bytes, first_len as i64);
    assert_eq!(retry.chunk_count, 1);

    let second = upload_backup_artifact_session_chunk(
        State(state.clone()),
        headers.clone(),
        Path((backup.id, session.upload_id)),
        Json(BackupArtifactUploadChunkRequest {
            offset_bytes: first_len as i64,
            data_base64: BASE64.encode(&artifact_bytes[first_len..]),
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(second.next_offset_bytes, artifact_bytes.len() as i64);
    assert_eq!(second.status, "uploaded");

    let unconfirmed = commit_backup_artifact_upload_session(
        State(state.clone()),
        headers.clone(),
        Path((backup.id, session.upload_id)),
        Json(BackupArtifactUploadCommitRequest { confirmed: false }),
    )
    .await
    .unwrap_err();
    assert_eq!(unconfirmed.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(
        unconfirmed.code,
        "backup_artifact_upload_commit_confirmation_required"
    );

    let (status, Json(artifact)) = commit_backup_artifact_upload_session(
        State(state.clone()),
        headers,
        Path((backup.id, session.upload_id)),
        Json(BackupArtifactUploadCommitRequest { confirmed: true }),
    )
    .await
    .unwrap();
    assert_eq!(status, axum::http::StatusCode::CREATED);
    assert_eq!(artifact.object_key, object_key);
    assert_eq!(artifact.sha256_hex, artifact_sha);
    assert_eq!(artifact.size_bytes, artifact_bytes.len() as i64);
    assert_eq!(
        tokio::fs::read(object_root.join(&object_key))
            .await
            .unwrap(),
        artifact_bytes
    );

    let backups = repo.list_backup_requests(10).await.unwrap();
    let audits = repo.list_audit_logs(10).await.unwrap();
    assert_eq!(backups[0].artifact_id, Some(artifact.id));
    assert_eq!(backups[0].status, "artifact_metadata_recorded");
    assert!(audits
        .iter()
        .any(|audit| audit.action == "backup.artifact_metadata_recorded"));

    let _ = tokio::fs::remove_dir_all(object_root).await;
}

#[tokio::test]
async fn backup_artifact_upload_chunk_route_accepts_advertised_chunk_body() {
    let state = test_state(Repository::Memory(MemoryState::default()));
    let headers = crate::test_auth_headers(&state).await;
    let authorization = headers
        .get(AUTHORIZATION)
        .expect("test authorization header")
        .clone();
    let chunk = vec![0_u8; crate::backup_upload_sessions::MAX_BACKUP_ARTIFACT_UPLOAD_CHUNK_BYTES];
    let body = serde_json::to_vec(&serde_json::json!({
        "offset_bytes": 0,
        "data_base64": BASE64.encode(&chunk),
    }))
    .unwrap();
    assert!(body.len() > 2 * 1024 * 1024);
    assert!(body.len() <= crate::routes::MAX_BACKUP_ARTIFACT_UPLOAD_CHUNK_BODY_BYTES);

    let response = crate::routes::build_router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/backups/{}/artifact-upload-sessions/{}/chunks",
                    Uuid::new_v4(),
                    Uuid::new_v4()
                ))
                .header(AUTHORIZATION, authorization)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn backup_artifact_upload_session_rejects_bad_offsets_and_can_abort() {
    let repo = Repository::Memory(MemoryState::default());
    seed_backup_agent(&repo).await;
    let object_root = std::env::temp_dir().join(format!(
        "vpsman-api-backup-upload-session-abort-{}",
        Uuid::new_v4()
    ));
    let state = test_state_with_store(
        repo.clone(),
        BackupObjectStore::filesystem(object_root.clone()).unwrap(),
    );
    let backup = create_test_backup_request(&repo, state.clone()).await;
    let artifact_bytes = plain_backup_artifact_bytes("client-a");
    let headers = crate::test_auth_headers(&state).await;

    let (_, Json(session)) = create_backup_artifact_upload_session(
        State(state.clone()),
        headers.clone(),
        Path(backup.id),
        Json(BackupArtifactUploadSessionCreateRequest {
            object_key: format!("backups/{}/{}-abort.tar", backup.client_id, backup.id),
            expected_sha256_hex: payload_hash(&artifact_bytes),
            expected_size_bytes: artifact_bytes.len() as i64,
            confirmed: true,
        }),
    )
    .await
    .unwrap();

    let offset_error = upload_backup_artifact_session_chunk(
        State(state.clone()),
        headers.clone(),
        Path((backup.id, session.upload_id)),
        Json(BackupArtifactUploadChunkRequest {
            offset_bytes: 4,
            data_base64: BASE64.encode(&artifact_bytes[..4]),
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(offset_error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(offset_error.code, "backup_artifact_upload_offset_mismatch");

    let abort = abort_backup_artifact_upload_session(
        State(state.clone()),
        headers.clone(),
        Path((backup.id, session.upload_id)),
        Json(BackupArtifactUploadCommitRequest { confirmed: true }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(abort.status, "aborted");

    let missing = upload_backup_artifact_session_chunk(
        State(state),
        headers,
        Path((backup.id, session.upload_id)),
        Json(BackupArtifactUploadChunkRequest {
            offset_bytes: 0,
            data_base64: BASE64.encode(&artifact_bytes[..4]),
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(missing.status, axum::http::StatusCode::NOT_FOUND);
    assert_eq!(missing.code, "backup_artifact_upload_session_not_found");

    let _ = tokio::fs::remove_dir_all(object_root).await;
}

#[tokio::test]
async fn backup_artifact_upload_rejects_invalid_or_wrong_client_payloads() {
    let repo = Repository::Memory(MemoryState::default());
    seed_backup_agent(&repo).await;
    let object_root = std::env::temp_dir().join(format!(
        "vpsman-api-backup-upload-reject-{}",
        Uuid::new_v4()
    ));
    let state = test_state_with_store(
        repo.clone(),
        BackupObjectStore::filesystem(object_root.clone()).unwrap(),
    );
    let backup = create_test_backup_request(&repo, state.clone()).await;
    let headers = crate::test_auth_headers(&state).await;

    let unconfirmed = upload_backup_artifact(
        State(state.clone()),
        headers.clone(),
        Path(backup.id),
        Json(UploadBackupArtifactRequest {
            object_key: format!("backups/{}/unconfirmed.tar", backup.client_id),
            artifact_base64: BASE64.encode(plain_backup_artifact_bytes("client-a")),
            confirmed: false,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(
        unconfirmed.code,
        "backup_artifact_upload_confirmation_required"
    );

    let wrong_client = upload_backup_artifact(
        State(state),
        headers,
        Path(backup.id),
        Json(UploadBackupArtifactRequest {
            object_key: format!("backups/{}/wrong-client.tar", backup.client_id),
            artifact_base64: BASE64.encode(plain_backup_artifact_bytes("client-b")),
            confirmed: true,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(wrong_client.code, "backup_artifact_client_mismatch");
    assert!(
        !tokio::fs::try_exists(object_root.join("backups/client-a/wrong-client.tar"))
            .await
            .unwrap()
    );

    let _ = tokio::fs::remove_dir_all(object_root).await;
}

#[tokio::test]
async fn backup_artifact_handoff_promotes_retained_backup_output() {
    let repo = Repository::Memory(MemoryState::default());
    seed_backup_agent(&repo).await;
    let object_root =
        std::env::temp_dir().join(format!("vpsman-api-backup-handoff-{}", Uuid::new_v4()));
    let state = test_state_with_store(
        repo.clone(),
        BackupObjectStore::filesystem(object_root.clone()).unwrap(),
    );
    let backup = create_test_backup_request(&repo, state.clone()).await;
    let artifact_bytes = plain_backup_artifact_bytes("client-a");
    let source_job_id = Uuid::new_v4();
    if let Repository::Memory(memory) = &repo {
        memory.jobs.write().await.push(JobHistoryView {
            id: source_job_id,
            actor_id: None,
            command_type: "backup".to_string(),
            privileged: true,
            status: "completed".to_string(),
            target_count: 1,
            payload_hash: backup.payload_hash.clone(),
            timeout_secs: 30,
            created_at: unix_now().to_string(),
            completed_at: Some(unix_now().to_string()),
        });
        memory.job_operations.write().await.insert(
            source_job_id,
            JobCommand::Backup {
                paths: backup.paths.clone(),
                include_config: backup.include_config,
                follow_symlinks: backup.follow_symlinks,
            },
        );
        memory.job_targets.write().await.push(JobTargetView {
            job_id: source_job_id,
            client_id: "client-a".to_string(),
            status: "completed".to_string(),
            message: None,
            exit_code: Some(0),
            started_at: Some(unix_now().to_string()),
            deadline_at: None,
            completed_at: Some(unix_now().to_string()),
            process_incarnation_id: None,
        });
        memory.job_outputs.write().await.push(JobOutputView {
            job_id: source_job_id,
            client_id: "client-a".to_string(),
            seq: 0,
            stream: "stdout".to_string(),
            data_base64: BASE64.encode(&artifact_bytes),
            storage: "inline".to_string(),
            artifact_object_key: None,
            artifact_sha256_hex: Some(payload_hash(&artifact_bytes)),
            artifact_size_bytes: Some(artifact_bytes.len() as i64),
            exit_code: Some(0),
            done: true,
            received_at: None,
            created_at: unix_now().to_string(),
        });
    }

    let headers = crate::test_auth_headers(&state).await;
    let (status, Json(handoff)) = create_backup_artifact_handoff(
        State(state.clone()),
        headers,
        Path(backup.id),
        Json(BackupArtifactHandoffRequest {
            confirmed: true,
            job_id: Some(source_job_id),
        }),
    )
    .await
    .unwrap();

    assert_eq!(status, axum::http::StatusCode::CREATED);
    assert_eq!(handoff.source_job_id, source_job_id);
    assert_eq!(handoff.source_chunk_count, 1);
    assert_eq!(handoff.source, "retained_job_outputs_streamed");
    assert_eq!(handoff.artifact.client_id, "client-a");
    assert_eq!(handoff.artifact.sha256_hex, payload_hash(&artifact_bytes));
    assert_eq!(
        tokio::fs::read(object_root.join(&handoff.artifact.object_key))
            .await
            .unwrap(),
        artifact_bytes
    );
    let backups = repo.list_backup_requests(10).await.unwrap();
    assert_eq!(backups[0].artifact_id, Some(handoff.artifact.id));
    assert!(repo
        .list_audit_logs(10)
        .await
        .unwrap()
        .iter()
        .any(|audit| audit.action == "backup.artifact_metadata_recorded"));

    let _ = tokio::fs::remove_dir_all(object_root).await;
}

#[tokio::test]
async fn backup_artifact_handoff_streams_object_store_backed_output() {
    let repo = Repository::Memory(MemoryState::default());
    seed_backup_agent(&repo).await;
    let object_root = std::env::temp_dir().join(format!(
        "vpsman-api-backup-handoff-object-backed-{}",
        Uuid::new_v4()
    ));
    let store = BackupObjectStore::filesystem(object_root.clone()).unwrap();
    let state = test_state_with_store(repo.clone(), store.clone());
    let backup = create_test_backup_request(&repo, state.clone()).await;
    let artifact_bytes = plain_backup_artifact_bytes("client-a");
    let source_object_key = "job-outputs/source-backup-artifact.bin";
    store
        .put_new(source_object_key, &artifact_bytes)
        .await
        .unwrap();
    let source_job_id = Uuid::new_v4();
    if let Repository::Memory(memory) = &repo {
        memory.jobs.write().await.push(JobHistoryView {
            id: source_job_id,
            actor_id: None,
            command_type: "backup".to_string(),
            privileged: true,
            status: "completed".to_string(),
            target_count: 1,
            payload_hash: backup.payload_hash.clone(),
            timeout_secs: 30,
            created_at: unix_now().to_string(),
            completed_at: Some(unix_now().to_string()),
        });
        memory.job_operations.write().await.insert(
            source_job_id,
            JobCommand::Backup {
                paths: backup.paths.clone(),
                include_config: backup.include_config,
                follow_symlinks: backup.follow_symlinks,
            },
        );
        memory.job_targets.write().await.push(JobTargetView {
            job_id: source_job_id,
            client_id: "client-a".to_string(),
            status: "completed".to_string(),
            message: None,
            exit_code: Some(0),
            started_at: Some(unix_now().to_string()),
            deadline_at: None,
            completed_at: Some(unix_now().to_string()),
            process_incarnation_id: None,
        });
        memory.job_outputs.write().await.push(JobOutputView {
            job_id: source_job_id,
            client_id: "client-a".to_string(),
            seq: 0,
            stream: "stdout".to_string(),
            data_base64: String::new(),
            storage: "object_store".to_string(),
            artifact_object_key: Some(source_object_key.to_string()),
            artifact_sha256_hex: Some(payload_hash(&artifact_bytes)),
            artifact_size_bytes: Some(artifact_bytes.len() as i64),
            exit_code: Some(0),
            done: true,
            received_at: None,
            created_at: unix_now().to_string(),
        });
    }

    let headers = crate::test_auth_headers(&state).await;
    let (status, Json(handoff)) = create_backup_artifact_handoff(
        State(state.clone()),
        headers,
        Path(backup.id),
        Json(BackupArtifactHandoffRequest {
            confirmed: true,
            job_id: Some(source_job_id),
        }),
    )
    .await
    .unwrap();

    assert_eq!(status, axum::http::StatusCode::CREATED);
    assert_eq!(handoff.source, "retained_job_outputs_streamed");
    assert_eq!(handoff.source_chunk_count, 1);
    assert_eq!(handoff.artifact.sha256_hex, payload_hash(&artifact_bytes));
    assert_eq!(
        tokio::fs::read(object_root.join(&handoff.artifact.object_key))
            .await
            .unwrap(),
        artifact_bytes
    );

    let _ = tokio::fs::remove_dir_all(object_root).await;
}

#[tokio::test]
async fn backup_artifact_handoff_requires_confirmation_and_source() {
    let repo = Repository::Memory(MemoryState::default());
    seed_backup_agent(&repo).await;
    let object_root = std::env::temp_dir().join(format!(
        "vpsman-api-backup-handoff-reject-{}",
        Uuid::new_v4()
    ));
    let state = test_state_with_store(
        repo.clone(),
        BackupObjectStore::filesystem(object_root.clone()).unwrap(),
    );
    let backup = create_test_backup_request(&repo, state.clone()).await;
    let headers = crate::test_auth_headers(&state).await;

    let unconfirmed = create_backup_artifact_handoff(
        State(state.clone()),
        headers.clone(),
        Path(backup.id),
        Json(BackupArtifactHandoffRequest {
            confirmed: false,
            job_id: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(
        unconfirmed.code,
        "backup_artifact_handoff_confirmation_required"
    );

    let missing_source = create_backup_artifact_handoff(
        State(state),
        headers,
        Path(backup.id),
        Json(BackupArtifactHandoffRequest {
            confirmed: true,
            job_id: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(
        missing_source.code,
        "backup_artifact_handoff_source_missing"
    );

    let _ = tokio::fs::remove_dir_all(object_root).await;
}

#[tokio::test]
async fn backup_request_requires_privilege_gateway_verification() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "client-a".to_string(),
                process_incarnation_id: uuid::Uuid::new_v4(),
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
    let missing = CreateBackupRequest {
        client_id: "client-a".to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: false,
        follow_symlinks: false,
        confirmed: true,
        note: None,
        privilege_assertion: None,
    };
    let state = test_state_without_privilege(repo);
    let headers = crate::test_auth_headers(&state).await;
    let missing_error = create_backup_request(State(state), headers, Json(missing))
        .await
        .unwrap_err();
    assert_eq!(missing_error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(missing_error.code, "gateway_control_url_missing");
}

fn test_state(repo: Repository) -> AppState {
    let (events, _) = broadcast::channel(1);
    AppState {
        repo,
        events,
        internal_token: None,
        gateway: GatewayDispatchClient::test_privilege_auto_approve(),
        backup_object_store: None,
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

fn test_state_with_store(repo: Repository, store: BackupObjectStore) -> AppState {
    AppState {
        backup_object_store: Some(store),
        ..test_state(repo)
    }
}

async fn seed_backup_agent(repo: &crate::repository::Repository) {
    seed_backup_agent_id(repo, "client-a").await;
}

async fn seed_backup_agent_id(repo: &crate::repository::Repository, client_id: &str) {
    if let Repository::Memory(memory) = repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: client_id.to_string(),
                process_incarnation_id: uuid::Uuid::new_v4(),
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

async fn create_test_backup_request(
    repo: &crate::repository::Repository,
    state: AppState,
) -> crate::model::BackupRequestView {
    let request = CreateBackupRequest {
        client_id: "client-a".to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        follow_symlinks: false,
        confirmed: true,
        note: Some("pre-migration".to_string()),
        privilege_assertion: None,
    };
    let headers = crate::test_auth_headers(&state).await;
    let (_, Json(backup)) = create_backup_request(State(state), headers, Json(request))
        .await
        .unwrap();
    assert_eq!(repo.list_backup_requests(10).await.unwrap().len(), 1);
    backup
}

async fn seed_policy_backup_artifact(
    repo: &crate::repository::Repository,
    store: &BackupObjectStore,
    schedule_id: Uuid,
    label: &str,
    age_days: u64,
) -> String {
    let operator = backup_test_operator();
    let request = CreateBackupRequest {
        client_id: "client-a".to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        follow_symlinks: false,
        confirmed: true,
        note: Some(format!("policy artifact {label}")),
        privilege_assertion: None,
    };
    let command = JobCommand::Backup {
        paths: request.paths.clone(),
        include_config: request.include_config,
        follow_symlinks: request.follow_symlinks,
    };
    let command_hash = payload_hash(&encode_json(&command).unwrap());
    let command_scope = format!("client:{}", request.client_id);
    let backup = repo
        .record_backup_request_with_source(
            &request,
            &command_hash,
            &command_scope,
            &operator,
            BackupRequestStatus::RequestedMetadataOnly,
            BackupRequestSourceLink {
                job_id: Some(Uuid::new_v4()),
                schedule_id: Some(schedule_id),
            },
        )
        .await
        .unwrap();
    let artifact_bytes = plain_backup_artifact_bytes("client-a");
    let object_key = format!("backups/client-a/{label}.tar");
    store.put_new(&object_key, &artifact_bytes).await.unwrap();
    let artifact = repo
        .record_backup_artifact_metadata(
            &backup,
            Uuid::new_v4(),
            &RecordBackupArtifactMetadataRequest {
                object_key: object_key.clone(),
                sha256_hex: payload_hash(&artifact_bytes),
                size_bytes: artifact_bytes.len() as i64,
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    if let Repository::Memory(memory) = repo {
        let created_at = unix_now()
            .saturating_sub(age_days.saturating_mul(86_400))
            .to_string();
        if let Some(stored) = memory
            .backup_artifacts
            .write()
            .await
            .iter_mut()
            .find(|stored| stored.id == artifact.id)
        {
            stored.created_at = created_at.clone();
        }
        if let Some(stored) = memory
            .backup_requests
            .write()
            .await
            .iter_mut()
            .find(|stored| stored.id == backup.id)
        {
            stored.created_at = created_at;
        }
    }
    object_key
}

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

fn backup_test_operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::new_v4(),
            username: "operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
        },
        session_id: Uuid::new_v4(),
    }
}

fn plain_backup_artifact_bytes(client_id: &str) -> Vec<u8> {
    let payload = format!("backup bytes for {client_id}").into_bytes();
    plain_backup_artifact_bytes_with_payload(client_id, &payload)
}

fn plain_backup_artifact_bytes_with_payload(client_id: &str, payload: &[u8]) -> Vec<u8> {
    let manifest = serde_json::to_vec(&serde_json::json!({
        "format": "vpsman.backup_tar.v1",
        "client_id": client_id,
        "created_unix": unix_now(),
        "files": [{
            "path": "/etc/hostname",
            "source": "selected_path",
            "tar_path": "vpsman-backup/files/0000.bin",
            "mode": 0o644,
            "size_bytes": payload.len(),
            "sha256_hex": payload_hash(payload),
            "mtime_unix": null,
        }],
    }))
    .unwrap();
    let mut archive = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut archive);
        append_test_tar_entry(
            &mut builder,
            "vpsman-backup/manifest.json",
            0o600,
            &manifest,
        );
        append_test_tar_entry(&mut builder, "vpsman-backup/files/0000.bin", 0o644, payload);
        builder.finish().unwrap();
    }
    archive
}

fn append_test_tar_entry<W: Write>(
    builder: &mut tar::Builder<W>,
    path: &str,
    mode: u32,
    bytes: &[u8],
) {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(mode);
    header.set_mtime(0);
    header.set_cksum();
    builder.append_data(&mut header, path, bytes).unwrap();
}

async fn spawn_backup_gateway_once(
    artifact_bytes: Vec<u8>,
) -> (String, tokio::task::JoinHandle<GatewayCommandDispatch>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buffer = Vec::new();
        let mut chunk = [0_u8; 1024];
        let dispatch = loop {
            let read = stream.read(&mut chunk).await.unwrap();
            assert_ne!(read, 0, "gateway client closed before request body");
            buffer.extend_from_slice(&chunk[..read]);
            if let Some(dispatch) = parse_gateway_dispatch(&buffer) {
                break dispatch;
            }
        };
        let body = serde_json::to_vec(&GatewayCommandDispatchResult {
            client_id: dispatch.client_id.clone(),
            job_id: dispatch.request.job_id,
            command_version: 1,
            accepted: true,
            message: "ok".to_string(),
            outputs: vec![
                CommandOutput {
                    job_id: dispatch.request.job_id,
                    stream: OutputStream::Stdout,
                    data: artifact_bytes,
                    exit_code: None,
                    done: false,
                },
                CommandOutput {
                    job_id: dispatch.request.job_id,
                    stream: OutputStream::Status,
                    data: Vec::new(),
                    exit_code: Some(0),
                    done: true,
                },
            ],
        })
        .unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
        dispatch
    });
    (format!("http://{addr}"), task)
}

fn parse_gateway_dispatch(buffer: &[u8]) -> Option<GatewayCommandDispatch> {
    let header_end = buffer.windows(4).position(|window| window == b"\r\n\r\n")?;
    let headers = std::str::from_utf8(&buffer[..header_end]).ok()?;
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length: "))
        .and_then(|value| value.trim().parse::<usize>().ok())?;
    let body_start = header_end + 4;
    if buffer.len() < body_start + content_length {
        return None;
    }
    serde_json::from_slice(&buffer[body_start..body_start + content_length]).ok()
}
