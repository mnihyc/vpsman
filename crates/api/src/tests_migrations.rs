use axum::{extract::State, Json};
use tokio::sync::broadcast;
use uuid::Uuid;
use vpsman_common::{AgentHello, JobCommand};

use crate::{
    gateway_client::GatewayDispatchClient,
    model::{
        CreateBackupRequest, CreateJobRequest, CreateMigrationLinkRequest,
        CreateMigrationRunRequest, CreateRestorePlanRequest,
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
            timeout_secs: Some(60),
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
