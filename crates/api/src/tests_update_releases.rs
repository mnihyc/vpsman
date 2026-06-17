use crate::state::UpdateReleasePolicy;
use crate::*;
use axum::{extract::State, Json};
use vpsman_common::{encode_json, payload_hash, AgentHello, JobCommand};

#[tokio::test]
async fn agent_update_release_registry_records_sanitized_external_metadata() {
    let repo = Repository::Memory(MemoryState::default());
    let state = test_state(repo.clone(), Default::default(), false);
    let operator = test_operator();
    let request = external_release_request("vpsman-agent", "1.2.3", "stable");

    routes_update_releases::validate_agent_update_release_request(&state, &request).unwrap();
    let release = repo
        .record_agent_update_release(&request, &operator)
        .await
        .unwrap();

    assert_eq!(release.name, "vpsman-agent");
    assert_eq!(release.version, "1.2.3");
    assert_eq!(release.channel, "stable");
    assert_eq!(release.status, "published_external");
    assert_eq!(release.artifact_sha256_hex, request.artifact_sha256_hex);
    assert!(release.artifact_url_sha256_hex.is_some());
    assert!(release.rollback_artifact_sha256_hex.is_none());
    assert!(repo
        .agent_update_release_exists_for_artifact(&request.artifact_sha256_hex)
        .await
        .unwrap());

    let serialized =
        serde_json::to_string(&repo.list_agent_update_releases(10).await.unwrap()).unwrap();
    assert!(!serialized.contains("https://updates.example"));
    assert!(!serialized.contains(&request.artifact_url));

    let audit = repo
        .list_audit_logs(10)
        .await
        .unwrap()
        .into_iter()
        .find(|audit| audit.action == "agent_update.release_recorded")
        .unwrap();
    let audit_json = serde_json::to_string(&audit.metadata).unwrap();
    assert!(audit_json.contains("artifact_url_sha256_hex"));
    assert!(audit_json.contains("\"artifact_url_stored\":false"));
    assert!(!audit_json.contains("https://updates.example"));
}

#[test]
fn agent_update_release_registry_rejects_bad_or_unconfirmed_metadata() {
    let state = test_state(
        Repository::Memory(MemoryState::default()),
        Default::default(),
        false,
    );
    let mut request = external_release_request("vpsman-agent", "1.2.3", "stable");
    request.confirmed = false;
    assert_eq!(
        routes_update_releases::validate_agent_update_release_request(&state, &request)
            .unwrap_err()
            .code,
        "agent_update_release_confirmation_required"
    );

    let mut request = external_release_request("vpsman-agent", "1.2.3", "stable");
    request.artifact_url = "http://updates.example/vpsman-agent".to_string();
    assert_eq!(
        routes_update_releases::validate_agent_update_release_request(&state, &request)
            .unwrap_err()
            .code,
        "agent_update_release_artifact_url_invalid"
    );

    let mut request = external_release_request("vpsman-agent", "1.2.3", "stable");
    request.artifact_sha256_hex = "not-a-hash".to_string();
    assert_eq!(
        routes_update_releases::validate_agent_update_release_request(&state, &request)
            .unwrap_err()
            .code,
        "agent_update_release_sha256_invalid"
    );

    let mut request = external_release_request("vpsman-agent", "1.2.3", "stable");
    request.rollback_artifact_url = Some("https://updates.example/vpsman-agent.old".to_string());
    assert_eq!(
        routes_update_releases::validate_agent_update_release_request(&state, &request)
            .unwrap_err()
            .code,
        "agent_update_rollback_release_sha256_required"
    );

    let mut request = external_release_request("vpsman-agent", "1.2.3", "stable");
    request.rollback_artifact_sha256_hex = Some("not-a-hash".to_string());
    request.rollback_artifact_url = Some("https://updates.example/vpsman-agent.old".to_string());
    assert_eq!(
        routes_update_releases::validate_agent_update_release_request(&state, &request)
            .unwrap_err()
            .code,
        "agent_update_rollback_release_sha256_invalid"
    );
}

#[tokio::test]
async fn release_registry_records_sanitized_rollback_metadata() {
    let repo = Repository::Memory(MemoryState::default());
    let state = test_state(repo.clone(), Default::default(), false);
    let operator = test_operator();
    let mut request = external_release_request("vpsman-agent", "2.1.0", "stable");
    let rollback_sha256_hex = "34".repeat(32);
    request.rollback_artifact_sha256_hex = Some(rollback_sha256_hex.clone());
    request.rollback_artifact_url = Some("https://updates.example/vpsman-agent.previous".into());
    request.rollback_size_bytes = Some(2048);

    routes_update_releases::validate_agent_update_release_request(&state, &request).unwrap();
    let release = repo
        .record_agent_update_release(&request, &operator)
        .await
        .unwrap();

    assert_eq!(
        release.rollback_artifact_sha256_hex.as_deref(),
        Some(rollback_sha256_hex.as_str())
    );
    assert!(release.rollback_artifact_url_sha256_hex.is_some());
    assert_eq!(release.rollback_size_bytes, Some(2048));
    let serialized = serde_json::to_string(&release).unwrap();
    assert!(!serialized.contains("vpsman-agent.previous"));

    let audit = repo
        .list_audit_logs(10)
        .await
        .unwrap()
        .into_iter()
        .find(|audit| audit.action == "agent_update.release_recorded")
        .unwrap();
    let audit_json = serde_json::to_string(&audit.metadata).unwrap();
    assert!(audit_json.contains("rollback_artifact_url_sha256_hex"));
    assert!(audit_json.contains("\"rollback_artifact_url_stored\":false"));
    assert!(!audit_json.contains("vpsman-agent.previous"));
}

#[tokio::test]
async fn strict_agent_update_release_policy_rejects_unregistered_update_before_gateway() {
    let repo = Repository::Memory(MemoryState::default());
    upsert_test_agent(&repo).await;
    let operation = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "12".repeat(32),
    };
    let command_hash = payload_hash(&encode_json(&operation).unwrap());
    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: true,
        command: "agent_update".to_string(),
        argv: Vec::new(),
        operation: Some(operation),
        timeout_secs: Some(30),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };
    let state = test_state(repo.clone(), Default::default(), true);
    let headers = crate::test_auth_headers(&state).await;

    let (status, Json(response)) = routes_jobs::create_job(State(state), headers, Json(request))
        .await
        .unwrap();

    assert_eq!(status, axum::http::StatusCode::CONFLICT);
    assert_eq!(response.status, "failed");
    let jobs = repo.list_jobs(10).await.unwrap();
    assert_eq!(jobs[0].payload_hash, command_hash);
    let audits = repo.list_audit_logs(10).await.unwrap();
    assert!(audits.iter().any(|audit| audit.action == "job.failed"));
}

#[tokio::test]
async fn release_policy_rejects_disallowed_channels() {
    let repo = Repository::Memory(MemoryState::default());
    let state = test_state(
        repo,
        UpdateReleasePolicy::new(vec!["stable".to_string()]).unwrap(),
        false,
    );
    let headers = crate::test_auth_headers(&state).await;
    let request = external_release_request("vpsman-agent", "2.4.0", "nightly");

    let error =
        routes_update_releases::create_agent_update_release(State(state), headers, Json(request))
            .await
            .unwrap_err();
    assert_eq!(error.code, "agent_update_release_channel_not_allowed");
}

fn test_state(
    repo: Repository,
    update_release_policy: UpdateReleasePolicy,
    require_registered_agent_updates: bool,
) -> AppState {
    AppState {
        repo,
        events: tokio::sync::broadcast::channel(4).0,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        backup_object_store: None,
        update_release_policy,
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates,
        suite_config_path: std::path::PathBuf::from("config/vpsman.toml"),
        dispatcher_config: crate::state::DispatcherRuntimeConfig::default(),
    }
}

fn test_operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "test-operator".to_string(),
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
        session_id: Uuid::nil(),
    }
}

fn external_release_request(
    name: &str,
    version: &str,
    channel: &str,
) -> CreateAgentUpdateReleaseRequest {
    CreateAgentUpdateReleaseRequest {
        name: name.to_string(),
        version: version.to_string(),
        channel: channel.to_string(),
        artifact_sha256_hex: "12".repeat(32),
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        rollback_artifact_sha256_hex: None,
        rollback_artifact_url: None,
        rollback_size_bytes: None,
        size_bytes: Some(1024),
        notes: Some("external release metadata".to_string()),
        confirmed: true,
    }
}

async fn upsert_test_agent(repo: &Repository) {
    if let Repository::Memory(memory) = repo {
        repository_ingest::upsert_memory_agent(
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
}
