use crate::state::UpdateReleasePolicy;
use crate::*;
use axum::{
    extract::State,
    http::{header::AUTHORIZATION, HeaderMap},
    Json,
};
use vpsman_common::{
    agent_update_asset_name_for_arch, encode_json, payload_hash, AgentHello,
    AgentUpdateVerificationRequest, CommandOutput, GatewayAgentUpdateVerificationIngest,
    GatewayCommandOutputIngest, GatewaySessionLifecycleIngest, JobCommand, OutputStream,
};

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
    assert!(repo
        .agent_update_release_exists_for_rollback_artifact(&rollback_sha256_hex)
        .await
        .unwrap());
    assert!(!repo
        .agent_update_release_exists_for_rollback_artifact(&"56".repeat(32))
        .await
        .unwrap());
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
    upsert_test_agent(&repo, "client-a", "x86_64").await;
    let operation = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "12".repeat(32),
    };
    let command_hash = payload_hash(&encode_json(&operation).unwrap());
    let request = CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: true,
        command: "agent_update".to_string(),
        argv: Vec::new(),
        operation: Some(operation),
        max_timeout_secs: Some(30),
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
async fn strict_agent_update_release_policy_rejects_unregistered_activation_and_rollback() {
    let cases = [
        (
            "agent_update_activate",
            JobCommand::AgentUpdateActivate {
                staged_sha256_hex: "34".repeat(32),
                restart_agent: true,
            },
        ),
        (
            "agent_update_rollback",
            JobCommand::AgentUpdateRollback {
                rollback_sha256_hex: Some("56".repeat(32)),
            },
        ),
        (
            "agent_update_rollback",
            JobCommand::AgentUpdateRollback {
                rollback_sha256_hex: None,
            },
        ),
    ];

    for (command, operation) in cases {
        let repo = Repository::Memory(MemoryState::default());
        upsert_test_agent(&repo, "client-a", "x86_64").await;
        let command_hash = payload_hash(&encode_json(&operation).unwrap());
        let request = update_job_request(command, operation);
        let state = test_state(repo.clone(), Default::default(), true);
        let headers = crate::test_auth_headers(&state).await;

        let (status, Json(response)) =
            routes_jobs::create_job(State(state), headers, Json(request))
                .await
                .unwrap();

        assert_eq!(status, axum::http::StatusCode::CONFLICT);
        assert_eq!(response.status, "failed");
        let jobs = repo.list_jobs(10).await.unwrap();
        assert_eq!(jobs[0].payload_hash, command_hash);
        let audits = repo.list_audit_logs(10).await.unwrap();
        assert!(audits.iter().any(|audit| audit.action == "job.failed"));
    }
}

#[tokio::test]
async fn strict_agent_update_release_policy_allows_registered_manifest_check() {
    let repo = Repository::Memory(MemoryState::default());
    upsert_test_agent(&repo, "client-a", "x86_64").await;
    let operator = test_operator();
    repo.record_agent_update_release(
        &external_release_request("vpsman-agent", "9.9.9", "stable"),
        &operator,
    )
    .await
    .unwrap();
    let version_url = local_update_manifest_url(&[("x86_64", "12".repeat(32))]);
    let operation = JobCommand::AgentUpdateCheck {
        version_url: Some(version_url),
        activate: true,
        restart_agent: true,
    };
    let request = update_job_request("agent_update_check", operation);
    let state = test_state(repo.clone(), Default::default(), true);
    let headers = crate::test_auth_headers(&state).await;

    let (status, Json(response)) = routes_jobs::create_job(State(state), headers, Json(request))
        .await
        .unwrap();

    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(response.status, "queued");
}

#[tokio::test]
async fn strict_agent_update_release_policy_defers_unregistered_manifest_check_to_agent_verification(
) {
    let repo = Repository::Memory(MemoryState::default());
    upsert_test_agent(&repo, "client-a", "x86_64").await;
    let version_url = local_update_manifest_url(&[("x86_64", "34".repeat(32))]);
    let operation = JobCommand::AgentUpdateCheck {
        version_url: Some(version_url),
        activate: true,
        restart_agent: true,
    };
    let command_hash = payload_hash(&encode_json(&operation).unwrap());
    let request = update_job_request("agent_update_check", operation);
    let state = test_state(repo.clone(), Default::default(), true);
    let headers = crate::test_auth_headers(&state).await;

    let (status, Json(response)) = routes_jobs::create_job(State(state), headers, Json(request))
        .await
        .unwrap();

    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(response.status, "queued");
    let jobs = repo.list_jobs(10).await.unwrap();
    assert_eq!(jobs[0].payload_hash, command_hash);
}

#[tokio::test]
async fn strict_agent_update_release_policy_requires_every_target_arch_hash() {
    let repo = Repository::Memory(MemoryState::default());
    upsert_test_agent(&repo, "client-a", "x86_64").await;
    upsert_test_agent(&repo, "client-b", "aarch64").await;
    let operator = test_operator();
    let x86_hash = "12".repeat(32);
    let arm_hash = "56".repeat(32);
    repo.record_agent_update_release(
        &external_release_request_with_hash("vpsman-agent", "9.9.9", "stable", &x86_hash),
        &operator,
    )
    .await
    .unwrap();
    repo.record_agent_update_release(
        &external_release_request_with_hash("vpsman-agent", "9.9.10", "stable", &arm_hash),
        &operator,
    )
    .await
    .unwrap();
    let operation = JobCommand::AgentUpdateCheck {
        version_url: Some(local_update_manifest_url(&[
            ("x86_64", x86_hash.clone()),
            ("aarch64", arm_hash.clone()),
        ])),
        activate: true,
        restart_agent: true,
    };
    let request =
        update_job_request_for_targets("agent_update_check", operation, &["client-a", "client-b"]);
    let state = test_state(repo.clone(), Default::default(), true);
    let headers = crate::test_auth_headers(&state).await;

    let (status, Json(response)) = routes_jobs::create_job(State(state), headers, Json(request))
        .await
        .unwrap();

    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(response.status, "queued");
}

#[tokio::test]
async fn strict_agent_update_release_policy_defers_target_arch_hashes_to_agent_verification() {
    let repo = Repository::Memory(MemoryState::default());
    upsert_test_agent(&repo, "client-a", "x86_64").await;
    upsert_test_agent(&repo, "client-b", "aarch64").await;
    let operator = test_operator();
    let x86_hash = "12".repeat(32);
    let arm_hash = "56".repeat(32);
    repo.record_agent_update_release(
        &external_release_request_with_hash("vpsman-agent", "9.9.9", "stable", &x86_hash),
        &operator,
    )
    .await
    .unwrap();
    let operation = JobCommand::AgentUpdateCheck {
        version_url: Some(local_update_manifest_url(&[
            ("x86_64", x86_hash),
            ("aarch64", arm_hash),
        ])),
        activate: true,
        restart_agent: true,
    };
    let request =
        update_job_request_for_targets("agent_update_check", operation, &["client-a", "client-b"]);
    let state = test_state(repo.clone(), Default::default(), true);
    let headers = crate::test_auth_headers(&state).await;

    let (status, Json(response)) = routes_jobs::create_job(State(state), headers, Json(request))
        .await
        .unwrap();

    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(response.status, "queued");
}

#[tokio::test]
async fn strict_agent_update_release_policy_defers_unsupported_arch_to_agent_verification() {
    let repo = Repository::Memory(MemoryState::default());
    upsert_test_agent(&repo, "client-a", "s390x").await;
    let operator = test_operator();
    let x86_hash = "12".repeat(32);
    repo.record_agent_update_release(
        &external_release_request_with_hash("vpsman-agent", "9.9.9", "stable", &x86_hash),
        &operator,
    )
    .await
    .unwrap();
    let operation = JobCommand::AgentUpdateCheck {
        version_url: Some(local_update_manifest_url(&[("x86_64", x86_hash)])),
        activate: true,
        restart_agent: true,
    };
    let request = update_job_request("agent_update_check", operation);
    let state = test_state(repo.clone(), Default::default(), true);
    let headers = crate::test_auth_headers(&state).await;

    let (status, Json(response)) = routes_jobs::create_job(State(state), headers, Json(request))
        .await
        .unwrap();

    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(response.status, "queued");
    assert_eq!(repo.list_jobs(10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn strict_agent_update_check_verification_rejects_unregistered_hash() {
    let repo = Repository::Memory(MemoryState::default());
    let sha256_hex = "78".repeat(32);
    let (state, headers, event) =
        active_update_check_verification_fixture(repo, sha256_hex.clone(), true).await;

    let Json(result) =
        routes_ingest::verify_agent_update_artifact(State(state), headers, Json(event))
            .await
            .unwrap();

    assert!(!result.approved);
    assert_eq!(result.message, "registered agent update artifact missing");
}

#[tokio::test]
async fn strict_agent_update_check_verification_accepts_registered_hash() {
    let repo = Repository::Memory(MemoryState::default());
    let sha256_hex = "9a".repeat(32);
    repo.record_agent_update_release(
        &external_release_request_with_hash("vpsman-agent", "9.9.9", "stable", &sha256_hex),
        &test_operator(),
    )
    .await
    .unwrap();
    let (state, headers, event) =
        active_update_check_verification_fixture(repo, sha256_hex, true).await;

    let Json(result) =
        routes_ingest::verify_agent_update_artifact(State(state), headers, Json(event))
            .await
            .unwrap();

    assert!(result.approved);
    assert_eq!(result.message, "registered agent update artifact accepted");
}

#[tokio::test]
async fn async_agent_update_activation_output_records_lifecycle_audit() {
    let repo = Repository::Memory(MemoryState::default());
    let process_incarnation_id = upsert_test_agent(&repo, "client-a", "x86_64").await;
    let staged_sha256_hex = "ab".repeat(32);
    let operation = JobCommand::AgentUpdateActivate {
        staged_sha256_hex: staged_sha256_hex.clone(),
        restart_agent: false,
    };
    let command_hash = payload_hash(&encode_json(&operation).unwrap());
    let request = update_job_request("agent_update_activate", operation);
    let job_id = request.job_id.unwrap();
    let mut state = test_state(repo.clone(), Default::default(), false);
    let operator_headers = crate::test_auth_headers(&state).await;

    let (status, Json(_)) =
        routes_jobs::create_job(State(state.clone()), operator_headers, Json(request))
            .await
            .unwrap();
    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    let claimed = repo.claim_due_job_targets(10, 30, 0).await.unwrap();
    assert_eq!(claimed.len(), 1);

    let gateway_session_id = Uuid::new_v4();
    repo.record_gateway_session_started(&GatewaySessionLifecycleIngest {
        gateway_id: "gateway-a".to_string(),
        client_id: "client-a".to_string(),
        session_id: gateway_session_id,
        noise_public_key_hex: None,
        remote_ip: None,
        reason: None,
    })
    .await
    .unwrap();
    let internal_token = "test-internal-token-32-byte-minimum".to_string();
    state.internal_token = Some(internal_token.clone());
    let mut internal_headers = HeaderMap::new();
    internal_headers.insert(
        AUTHORIZATION,
        format!("Bearer {internal_token}").parse().unwrap(),
    );
    let final_status = serde_json::json!({
        "type": "agent_update_activation",
        "status": "activated_pending_restart",
        "sha256_hex": staged_sha256_hex.clone(),
        "restart": "manual_restart_required",
    });

    let Json(_) = routes_ingest::ingest_command_output(
        State(state),
        internal_headers,
        Json(GatewayCommandOutputIngest {
            gateway_id: "gateway-a".to_string(),
            gateway_session_id,
            process_incarnation_id,
            spooled_replay: false,
            client_id: "client-a".to_string(),
            job_id,
            payload_hash: command_hash,
            seq: 0,
            received_unix: None,
            output: CommandOutput {
                job_id,
                stream: OutputStream::Status,
                data: serde_json::to_vec(&final_status).unwrap(),
                exit_code: Some(0),
                done: true,
            },
        }),
    )
    .await
    .unwrap();

    let audits = repo.list_audit_logs(20).await.unwrap();
    assert!(audits.iter().any(|audit| {
        audit.action == "agent_update.activation_completed"
            && audit.target == "client:client-a"
            && audit.metadata["activation_job_id"] == job_id.to_string()
            && audit.metadata["artifact_sha256_hex"] == staged_sha256_hex
    }));
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
        gateway: GatewayDispatchClient::new(
            Some("http://127.0.0.1:9".to_string()),
            Some("test-token-32-byte-minimum-value".to_string()),
        )
        .with_test_privilege_auto_approve(),
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
    external_release_request_with_hash(name, version, channel, &"12".repeat(32))
}

fn external_release_request_with_hash(
    name: &str,
    version: &str,
    channel: &str,
    artifact_sha256_hex: &str,
) -> CreateAgentUpdateReleaseRequest {
    CreateAgentUpdateReleaseRequest {
        name: name.to_string(),
        version: version.to_string(),
        channel: channel.to_string(),
        artifact_sha256_hex: artifact_sha256_hex.to_string(),
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        rollback_artifact_sha256_hex: None,
        rollback_artifact_url: None,
        rollback_size_bytes: None,
        size_bytes: Some(1024),
        notes: Some("external release metadata".to_string()),
        confirmed: true,
    }
}

fn update_job_request(command: &str, operation: JobCommand) -> CreateJobRequest {
    update_job_request_for_targets(command, operation, &["client-a"])
}

fn update_job_request_for_targets(
    command: &str,
    operation: JobCommand,
    targets: &[&str],
) -> CreateJobRequest {
    CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: targets
            .iter()
            .map(|target| format!("id:{target}"))
            .collect::<Vec<_>>()
            .join(" OR "),
        target_client_ids: targets.iter().map(|target| (*target).to_string()).collect(),
        destructive: false,
        confirmed: true,
        command: command.to_string(),
        argv: Vec::new(),
        operation: Some(operation),
        max_timeout_secs: Some(30),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    }
}

fn local_update_manifest_url(arch_hashes: &[(&str, String)]) -> String {
    let root =
        std::env::temp_dir().join(format!("vpsman-update-manifest-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let sums_path = root.join("SHA256SUMS");
    let mut sums = String::new();
    let assets = arch_hashes
        .iter()
        .map(|(arch, artifact_sha256_hex)| {
            let asset_name = agent_update_asset_name_for_arch(arch).unwrap();
            sums.push_str(&format!("{artifact_sha256_hex}  {asset_name}\n"));
            serde_json::json!({
                "name": asset_name,
                "download_url": format!("https://updates.example/{asset_name}")
            })
        })
        .collect::<Vec<_>>();
    std::fs::write(&sums_path, sums).unwrap();
    let manifest_path = root.join("version.json");
    let manifest = serde_json::json!({
        "schema_version": 2,
        "project": "vpsman",
        "version": "99.0.0",
        "tag": "v99.0.0",
        "assets": assets,
        "checksum_manifest": {
            "name": "SHA256SUMS",
            "download_url": format!("file://{}", sums_path.display())
        }
    });
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    format!("file://{}", manifest_path.display())
}

async fn active_update_check_verification_fixture(
    repo: Repository,
    sha256_hex: String,
    require_registered_agent_updates: bool,
) -> (AppState, HeaderMap, GatewayAgentUpdateVerificationIngest) {
    let process_incarnation_id = upsert_test_agent(&repo, "client-a", "x86_64").await;
    let state = test_state(
        repo.clone(),
        Default::default(),
        require_registered_agent_updates,
    );
    let operator_headers = crate::test_auth_headers(&state).await;
    let operation = JobCommand::AgentUpdateCheck {
        version_url: Some("https://updates.example/version.json".to_string()),
        activate: true,
        restart_agent: true,
    };
    let request = update_job_request("agent_update_check", operation);
    let job_id = request.job_id.unwrap();
    let (status, Json(_)) =
        routes_jobs::create_job(State(state.clone()), operator_headers, Json(request))
            .await
            .unwrap();
    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    let claimed = repo.claim_due_job_targets(10, 30, 0).await.unwrap();
    assert_eq!(claimed.len(), 1);
    let gateway_session_id = Uuid::new_v4();
    repo.record_gateway_session_started(&GatewaySessionLifecycleIngest {
        gateway_id: "gateway-a".to_string(),
        client_id: "client-a".to_string(),
        session_id: gateway_session_id,
        noise_public_key_hex: None,
        remote_ip: None,
        reason: None,
    })
    .await
    .unwrap();
    let internal_token = "test-internal-token-32-byte-minimum".to_string();
    let mut state = state;
    state.internal_token = Some(internal_token.clone());
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        format!("Bearer {internal_token}").parse().unwrap(),
    );
    (
        state,
        headers,
        GatewayAgentUpdateVerificationIngest {
            gateway_id: "gateway-a".to_string(),
            gateway_session_id,
            process_incarnation_id,
            client_id: "client-a".to_string(),
            request: AgentUpdateVerificationRequest {
                job_id,
                version_url: "https://updates.example/version.json".to_string(),
                artifact_url: "https://updates.example/vpsman-agent".to_string(),
                checksum_url: "https://updates.example/SHA256SUMS".to_string(),
                asset_name: "vpsman-agent-linux-x86_64-musl".to_string(),
                sha256_hex,
            },
        },
    )
}

async fn upsert_test_agent(repo: &Repository, client_id: &str, arch: &str) -> Uuid {
    let process_incarnation_id = uuid::Uuid::new_v4();
    if let Repository::Memory(memory) = repo {
        repository_ingest::upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: client_id.to_string(),
                process_incarnation_id,
                agent_version: "test".to_string(),
                os_release: "test".to_string(),
                arch: arch.to_string(),
                update_heartbeat: None,
                internal_build_number: 1,
                capabilities: Default::default(),
            },
        )
        .await;
    }
    process_incarnation_id
}
