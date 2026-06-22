use axum::{extract::State, Json};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use tokio::sync::broadcast;
use uuid::Uuid;
use vpsman_common::{payload_hash, AgentHello, JobCommand};

use crate::{
    gateway_client::GatewayDispatchClient,
    model::{
        AuthContext, CreateBackupRequest, CreateJobRequest, CreateMigrationLinkRequest,
        CreateMigrationRunRequest, CreateRestorePlanRequest, JobHistoryView, JobOutputView,
        OperatorPreferences, OperatorView, RecordBackupArtifactMetadataRequest,
    },
    repository::{MemoryState, Repository},
    repository_ingest::upsert_memory_agent,
    routes_backups::create_backup_request,
    routes_migrations::{
        create_migration_link, create_migration_run, validate_create_migration_link,
    },
    routes_restores::create_restore_plan,
    state::AppState,
};

#[test]
fn migration_link_validation_requires_confirmation() {
    let unconfirmed = CreateMigrationLinkRequest {
        restore_plan_id: Uuid::new_v4(),
        confirmed: false,
        note: None,
        privilege_assertion: None,
    };
    assert_eq!(
        validate_create_migration_link(&unconfirmed)
            .unwrap_err()
            .code,
        "migration_confirmation_required"
    );

    let oversized_note = CreateMigrationLinkRequest {
        restore_plan_id: Uuid::new_v4(),
        confirmed: true,
        note: Some("x".repeat(1025)),
        privilege_assertion: None,
    };
    assert_eq!(
        validate_create_migration_link(&oversized_note)
            .unwrap_err()
            .code,
        "migration_note_too_long"
    );
}

#[tokio::test]
async fn migration_link_records_restore_plan_identity_and_audit() {
    let repo = seeded_migration_repo().await;
    let source_backup_id = create_source_backup(&repo).await;
    let restore_plan_id = create_restore_plan_record(&repo, source_backup_id).await;
    let request = CreateMigrationLinkRequest {
        restore_plan_id,
        confirmed: true,
        note: Some("rebuilt node ready".to_string()),
        privilege_assertion: None,
    };

    let state = test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;
    let (status, Json(view)) = create_migration_link(State(state), headers, Json(request))
        .await
        .unwrap();
    let links = repo.list_migration_links(10).await.unwrap();
    let audits = repo.list_audit_logs(10).await.unwrap();

    assert_eq!(status, axum::http::StatusCode::CREATED);
    assert_eq!(view.restore_plan_id, restore_plan_id);
    assert_eq!(view.source_backup_request_id, source_backup_id);
    assert_eq!(view.source_client_id, "source-client");
    assert_eq!(view.target_client_id, "rebuilt-client");
    assert_eq!(view.paths, vec!["/etc/hostname"]);
    assert!(view.include_config);
    assert_eq!(view.destination_root.as_deref(), Some("/restore"));
    assert_eq!(view.status, "linked_metadata_only");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].id, view.id);
    assert!(audits
        .iter()
        .any(|audit| audit.action == "migration.linked_metadata_only"));
}

#[tokio::test]
async fn migration_run_validation_failure_creates_no_link_or_job() {
    let repo = seeded_migration_repo().await;
    let source_backup_id = create_source_backup(&repo).await;
    let restore_plan_id = create_restore_plan_record(&repo, source_backup_id).await;
    let state = test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;

    let first_error = create_migration_run(
        State(state.clone()),
        headers.clone(),
        Json(migration_run_request(restore_plan_id, source_backup_id)),
    )
    .await
    .unwrap_err();
    assert_eq!(first_error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(first_error.code, "restore_source_backup_artifact_required");
    assert_eq!(repo.list_migration_links(10).await.unwrap().len(), 0);
    assert_eq!(repo.list_jobs(10).await.unwrap().len(), 0);

    let error = create_migration_run(
        State(state),
        headers,
        Json(migration_run_request(restore_plan_id, source_backup_id)),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(error.code, "restore_source_backup_artifact_required");
    assert_eq!(repo.list_migration_links(10).await.unwrap().len(), 0);
    assert_eq!(repo.list_jobs(10).await.unwrap().len(), 0);
}

#[tokio::test]
async fn migration_run_existing_link_returns_conflict_without_restore_job() {
    let repo = seeded_migration_repo().await;
    let source_backup_id = create_source_backup(&repo).await;
    let restore_plan_id = create_restore_plan_record(&repo, source_backup_id).await;
    let state = test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;

    let (_, Json(_)) = create_migration_link(
        State(state.clone()),
        headers.clone(),
        Json(CreateMigrationLinkRequest {
            restore_plan_id,
            confirmed: true,
            note: Some("already linked".to_string()),
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap();

    let error = create_migration_run(
        State(state),
        headers,
        Json(migration_run_request(restore_plan_id, source_backup_id)),
    )
    .await
    .unwrap_err();

    assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(error.code, "migration_link_already_exists");
    assert_eq!(repo.list_migration_links(10).await.unwrap().len(), 1);
    assert_eq!(repo.list_jobs(10).await.unwrap().len(), 0);
}

#[tokio::test]
async fn migration_run_job_conflict_creates_no_migration_link() {
    let repo = seeded_migration_repo().await;
    let source_backup_id = create_source_backup(&repo).await;
    let (archive_path, archive_size_bytes, archive_sha256_hex) =
        attach_source_backup_artifact(&repo, source_backup_id).await;
    let session_id = Uuid::new_v4();
    seed_completed_archive_upload(
        &repo,
        "rebuilt-client",
        session_id,
        &archive_path,
        archive_size_bytes as i64,
        &archive_sha256_hex,
    )
    .await;
    let restore_plan_id = create_restore_plan_record(&repo, source_backup_id).await;
    let mut request = migration_run_request(restore_plan_id, source_backup_id);
    let conflict_job_id = request.job.job_id.unwrap();
    if let Repository::Memory(memory) = &repo {
        memory.jobs.write().await.push(JobHistoryView {
            id: conflict_job_id,
            actor_id: None,
            command_type: "shell".to_string(),
            privileged: true,
            status: "queued".to_string(),
            target_count: 1,
            payload_hash: "b".repeat(64),
            max_timeout_secs: 60,
            created_at: crate::unix_now().to_string(),
            completed_at: None,
        });
    }
    if let Some(JobCommand::Restore {
        archive_transfer_session_id,
        archive_path: path,
        archive_size_bytes: size,
        archive_sha256_hex: hash,
        ..
    }) = request.job.operation.as_mut()
    {
        *archive_transfer_session_id = session_id;
        *path = Some(archive_path);
        *size = Some(archive_size_bytes);
        *hash = Some(archive_sha256_hex);
    }
    let state = test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;

    let error = create_migration_run(State(state), headers, Json(request))
        .await
        .unwrap_err();

    assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(error.code, "job_id_reused_with_different_request");
    assert_eq!(repo.list_migration_links(10).await.unwrap().len(), 0);
    assert!(!repo
        .list_jobs(10)
        .await
        .unwrap()
        .iter()
        .any(|job| job.command_type == "restore"));
}

#[tokio::test]
async fn migration_link_rejects_missing_restore_plan() {
    let repo = seeded_migration_repo().await;
    let request = CreateMigrationLinkRequest {
        restore_plan_id: Uuid::new_v4(),
        confirmed: true,
        note: None,
        privilege_assertion: None,
    };
    let state = test_state(repo);
    let headers = crate::test_auth_headers(&state).await;
    let error = create_migration_link(State(state), headers, Json(request))
        .await
        .unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "migration_restore_plan_not_found");
}

async fn seeded_migration_repo() -> Repository {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        for client_id in ["source-client", "rebuilt-client"] {
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
    repo
}

async fn create_source_backup(repo: &Repository) -> Uuid {
    let request = CreateBackupRequest {
        client_id: "source-client".to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        follow_symlinks: false,
        confirmed: true,
        note: Some("pre-migration".to_string()),
        privilege_assertion: None,
    };
    let state = test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;
    let (_, Json(view)) = create_backup_request(State(state), headers, Json(request))
        .await
        .unwrap();
    view.id
}

async fn attach_source_backup_artifact(
    repo: &Repository,
    source_backup_id: Uuid,
) -> (String, u64, String) {
    let backup = repo
        .list_backup_requests(10)
        .await
        .unwrap()
        .into_iter()
        .find(|backup| backup.id == source_backup_id)
        .unwrap();
    let archive_bytes = b"plain backup artifact for migration restore validation";
    let archive_sha256_hex = payload_hash(archive_bytes);
    let archive_size_bytes = archive_bytes.len() as u64;
    let archive_path = format!("/var/lib/vpsman/restores/{source_backup_id}.tar");
    repo.record_backup_artifact_metadata(
        &backup,
        Uuid::new_v4(),
        &RecordBackupArtifactMetadataRequest {
            object_key: format!("backups/{}/{}.tar", backup.client_id, backup.id),
            sha256_hex: archive_sha256_hex.clone(),
            size_bytes: archive_size_bytes as i64,
            confirmed: true,
        },
        &migration_test_operator(),
    )
    .await
    .unwrap();
    (archive_path, archive_size_bytes, archive_sha256_hex)
}

async fn seed_completed_archive_upload(
    repo: &Repository,
    client_id: &str,
    session_id: Uuid,
    path: &str,
    size_bytes: i64,
    sha256_hex: &str,
) {
    if let Repository::Memory(memory) = repo {
        let job_id = Uuid::new_v4();
        memory.jobs.write().await.push(JobHistoryView {
            id: job_id,
            actor_id: None,
            command_type: "file_transfer_commit".to_string(),
            privileged: true,
            status: "completed".to_string(),
            target_count: 1,
            payload_hash: "1".repeat(64),
            max_timeout_secs: 30,
            created_at: crate::unix_now().to_string(),
            completed_at: Some(crate::unix_now().to_string()),
        });
        let status = serde_json::json!({
            "type": "file_transfer_commit",
            "session_id": session_id,
            "path": path,
            "next_offset": size_bytes,
            "size_bytes": size_bytes,
            "extra": {
                "sha256_hex": sha256_hex,
                "mode": 0o600,
            },
        });
        memory.job_outputs.write().await.push(JobOutputView {
            job_id,
            client_id: client_id.to_string(),
            seq: 0,
            stream: "status".to_string(),
            data_base64: BASE64_STANDARD.encode(serde_json::to_vec(&status).unwrap()),
            storage: "inline".to_string(),
            artifact_object_key: None,
            artifact_sha256_hex: None,
            artifact_size_bytes: None,
            exit_code: Some(0),
            done: true,
            received_at: None,
            created_at: crate::unix_now().to_string(),
        });
    }
}

async fn create_restore_plan_record(repo: &Repository, source_backup_id: Uuid) -> Uuid {
    let request = CreateRestorePlanRequest {
        source_backup_request_id: source_backup_id,
        target_client_id: "rebuilt-client".to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        destination_root: Some("/restore".to_string()),
        confirmed: true,
        note: Some("restore to rebuilt node".to_string()),
        privilege_assertion: None,
    };
    let state = test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;
    let (_, Json(view)) = create_restore_plan(State(state), headers, Json(request))
        .await
        .unwrap();
    view.id
}

fn migration_run_request(
    restore_plan_id: Uuid,
    source_backup_id: Uuid,
) -> CreateMigrationRunRequest {
    let operation = JobCommand::Restore {
        source_backup_request_id: source_backup_id,
        archive_transfer_session_id: Uuid::new_v4(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        destination_root: Some("/restore".to_string()),
        archive_path: Some("/var/lib/vpsman/archive.tar".to_string()),
        archive_size_bytes: Some(1024),
        archive_sha256_hex: Some("a".repeat(64)),
        dry_run: false,
        post_restore_argv: Vec::new(),
    };
    CreateMigrationRunRequest {
        link: CreateMigrationLinkRequest {
            restore_plan_id,
            confirmed: true,
            note: Some("run migration".to_string()),
            privilege_assertion: None,
        },
        job: CreateJobRequest {
            job_id: Some(Uuid::new_v4()),
            selector_expression: "id:rebuilt-client".to_string(),
            target_client_ids: vec!["rebuilt-client".to_string()],
            destructive: true,
            confirmed: true,
            command: "restore".to_string(),
            argv: Vec::new(),
            operation: Some(operation),
            max_timeout_secs: Some(60),
            force_unprivileged: false,
            privileged: true,
            privilege_assertion: None,
        },
    }
}

fn test_state(repo: Repository) -> AppState {
    let (events, _) = broadcast::channel(1);
    AppState {
        repo,
        events,
        internal_token: None,
        gateway: GatewayDispatchClient::new(
            Some("http://127.0.0.1:9".to_string()),
            Some("test-token-32-byte-minimum-value".to_string()),
        )
        .with_test_privilege_auto_approve(),
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

fn migration_test_operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "migration-test-operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: OperatorPreferences::default(),
            totp_enabled: false,
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
        },
        session_id: Uuid::nil(),
    }
}
