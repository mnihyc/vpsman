use super::*;
use base64::Engine as _;
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
};
use vpsman_common::{
    AgentCapabilitySnapshot, AgentHello, AgentPrivilegeMode, GatewayPrivilegeVerificationResult,
    GatewaySessionDisconnectResult, PrivilegeAssertion,
};
use vpsman_server_core::TARGET_STATUS_AGENT_LOST;

#[tokio::test]
async fn direct_agent_identity_imports_key_and_tags_without_panel_token() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = identity_operator();

    let identity = repo
        .upsert_agent_identity(
            &UpsertAgentIdentityRequest {
                client_id: Some("edge-direct-01".to_string()),
                client_public_key_hex: "11".repeat(32),
                display_name: Some("LAX edge direct 01".to_string()),
                tags: vec!["role:edge".to_string(), "region:us-west".to_string()],
                replace_existing_key: false,
                confirmed: true,
                privilege_assertion: None,
            },
            &operator,
        )
        .await
        .unwrap();

    assert_eq!(identity.client_id, "edge-direct-01");
    assert_eq!(identity.display_name, "LAX edge direct 01");
    assert!(identity.tags.contains(&"role:edge".to_string()));
    assert!(repo
        .validate_agent_public_key("edge-direct-01", &"11".repeat(32))
        .await
        .unwrap());

    let report = repo.key_lifecycle_report().await.unwrap();
    assert_eq!(report.direct_identity_client_count, 1);
    assert_eq!(report.current_key_revoked_count, 0);
}

#[tokio::test]
async fn visible_agent_display_names_are_unique_and_hidden_names_are_reusable() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = identity_operator();

    repo.upsert_agent_identity(
        &UpsertAgentIdentityRequest {
            client_id: Some("edge-direct-unique-01".to_string()),
            client_public_key_hex: "12".repeat(32),
            display_name: Some("Edge Unique".to_string()),
            tags: Vec::new(),
            replace_existing_key: false,
            confirmed: true,
            privilege_assertion: None,
        },
        &operator,
    )
    .await
    .unwrap();

    let duplicate = repo
        .upsert_agent_identity(
            &UpsertAgentIdentityRequest {
                client_id: Some("edge-direct-unique-02".to_string()),
                client_public_key_hex: "13".repeat(32),
                display_name: Some(" edge unique ".to_string()),
                tags: Vec::new(),
                replace_existing_key: false,
                confirmed: true,
                privilege_assertion: None,
            },
            &operator,
        )
        .await
        .unwrap_err();
    assert!(duplicate
        .to_string()
        .contains("display_name_already_exists"));

    repo.upsert_agent_identity(
        &UpsertAgentIdentityRequest {
            client_id: Some("edge-direct-unique-03".to_string()),
            client_public_key_hex: "14".repeat(32),
            display_name: Some("Spare Unique".to_string()),
            tags: Vec::new(),
            replace_existing_key: false,
            confirmed: true,
            privilege_assertion: None,
        },
        &operator,
    )
    .await
    .unwrap();
    let alias_collision = repo
        .update_agent_alias("edge-direct-unique-03", "EDGE UNIQUE", &operator)
        .await
        .unwrap_err();
    assert!(alias_collision
        .to_string()
        .contains("display_name_already_exists"));

    repo.upsert_agent_identity(
        &UpsertAgentIdentityRequest {
            client_id: Some("edge-direct-unique-05".to_string()),
            client_public_key_hex: "16".repeat(32),
            display_name: Some("Ünicode Edge".to_string()),
            tags: Vec::new(),
            replace_existing_key: false,
            confirmed: true,
            privilege_assertion: None,
        },
        &operator,
    )
    .await
    .unwrap();
    let unicode_collision = repo
        .upsert_agent_identity(
            &UpsertAgentIdentityRequest {
                client_id: Some("edge-direct-unique-06".to_string()),
                client_public_key_hex: "17".repeat(32),
                display_name: Some("ünicode edge".to_string()),
                tags: Vec::new(),
                replace_existing_key: false,
                confirmed: true,
                privilege_assertion: None,
            },
            &operator,
        )
        .await
        .unwrap_err();
    assert!(unicode_collision
        .to_string()
        .contains("display_name_already_exists"));

    if let Repository::Memory(memory) = &repo {
        memory
            .hidden_clients
            .write()
            .await
            .insert("edge-direct-unique-01".to_string());
    }
    let reused = repo
        .upsert_agent_identity(
            &UpsertAgentIdentityRequest {
                client_id: Some("edge-direct-unique-04".to_string()),
                client_public_key_hex: "15".repeat(32),
                display_name: Some("edge unique".to_string()),
                tags: Vec::new(),
                replace_existing_key: false,
                confirmed: true,
                privilege_assertion: None,
            },
            &operator,
        )
        .await
        .unwrap();

    assert_eq!(reused.display_name, "edge unique");
}

#[tokio::test]
async fn direct_agent_identity_key_change_requires_explicit_replace_and_blocks_revoked_key() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = identity_operator();
    let process_incarnation_id = Uuid::new_v4();
    let rotation_job_id = Uuid::new_v4();
    repo.upsert_agent_identity(
        &UpsertAgentIdentityRequest {
            client_id: Some("edge-direct-02".to_string()),
            client_public_key_hex: "22".repeat(32),
            display_name: Some("SJC edge direct 02".to_string()),
            tags: Vec::new(),
            replace_existing_key: false,
            confirmed: true,
            privilege_assertion: None,
        },
        &operator,
    )
    .await
    .unwrap();
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "edge-direct-02".to_string(),
                process_incarnation_id,
                agent_version: "test".to_string(),
                os_release: "test".to_string(),
                arch: "x86_64".to_string(),
                update_heartbeat: None,
                internal_build_number: 1,
                capabilities: Default::default(),
            },
        )
        .await;
        memory.jobs.write().await.push(JobHistoryView {
            id: rotation_job_id,
            actor_id: Some(operator.operator.id),
            command_type: "shell".to_string(),
            privileged: true,
            status: "running".to_string(),
            target_count: 1,
            payload_hash: "rotation-active-target".to_string(),
            max_timeout_secs: 30,
            created_at: "1700000000".to_string(),
            completed_at: None,
        });
        memory.job_targets.write().await.push(JobTargetView {
            job_id: rotation_job_id,
            client_id: "edge-direct-02".to_string(),
            status: "running".to_string(),
            message: None,
            exit_code: None,
            started_at: Some("1700000001".to_string()),
            deadline_at: None,
            completed_at: None,
            process_incarnation_id: Some(process_incarnation_id),
        });
    }

    assert!(repo
        .upsert_agent_identity(
            &UpsertAgentIdentityRequest {
                client_id: Some("edge-direct-02".to_string()),
                client_public_key_hex: "33".repeat(32),
                display_name: None,
                tags: Vec::new(),
                replace_existing_key: false,
                confirmed: true,
                privilege_assertion: None,
            },
            &operator,
        )
        .await
        .is_err());

    repo.upsert_agent_identity(
        &UpsertAgentIdentityRequest {
            client_id: Some("edge-direct-02".to_string()),
            client_public_key_hex: "33".repeat(32),
            display_name: None,
            tags: Vec::new(),
            replace_existing_key: true,
            confirmed: true,
            privilege_assertion: None,
        },
        &operator,
    )
    .await
    .unwrap();
    assert!(repo
        .validate_agent_public_key("edge-direct-02", &"33".repeat(32))
        .await
        .unwrap());
    if let Repository::Memory(memory) = &repo {
        let targets = memory.job_targets.read().await;
        let target = targets
            .iter()
            .find(|target| target.job_id == rotation_job_id && target.client_id == "edge-direct-02")
            .unwrap();
        assert_eq!(target.status, TARGET_STATUS_AGENT_LOST);
        assert_eq!(
            target.message.as_deref(),
            Some("client public key was replaced before final command output")
        );
        drop(targets);
        let outputs = memory.job_outputs.read().await;
        let output = outputs
            .iter()
            .find(|output| output.job_id == rotation_job_id && output.client_id == "edge-direct-02")
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(
            &base64::engine::general_purpose::STANDARD
                .decode(&output.data_base64)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(payload["code"], "client_key_replaced");
    }
    let job_id = Uuid::new_v4();
    if let Repository::Memory(memory) = &repo {
        memory.jobs.write().await.push(JobHistoryView {
            id: job_id,
            actor_id: Some(operator.operator.id),
            command_type: "shell".to_string(),
            privileged: true,
            status: "queued".to_string(),
            target_count: 1,
            payload_hash: "revocation-test".to_string(),
            max_timeout_secs: 30,
            created_at: "1700000000".to_string(),
            completed_at: None,
        });
        memory.job_targets.write().await.push(JobTargetView {
            job_id,
            client_id: "edge-direct-02".to_string(),
            status: "queued".to_string(),
            message: None,
            exit_code: None,
            started_at: None,
            deadline_at: None,
            completed_at: None,
            process_incarnation_id: None,
        });
    }

    repo.revoke_current_client_key(
        "edge-direct-02",
        &CreateClientKeyRevocationRequest {
            confirmed: true,
            reason: Some("provider rebuild with compromised disk snapshot".to_string()),
            privilege_assertion: None,
        },
        &operator,
    )
    .await
    .unwrap();
    assert!(!repo
        .validate_agent_public_key("edge-direct-02", &"33".repeat(32))
        .await
        .unwrap());
    if let Repository::Memory(memory) = &repo {
        let targets = memory.job_targets.read().await;
        let target = targets
            .iter()
            .find(|target| target.job_id == job_id && target.client_id == "edge-direct-02")
            .unwrap();
        assert_eq!(target.status, "skipped");
        assert_eq!(
            target.message.as_deref(),
            Some("client_key_revoked: target skipped before dispatch")
        );
        drop(targets);
        let outputs = memory.job_outputs.read().await;
        let output = outputs
            .iter()
            .find(|output| output.job_id == job_id && output.client_id == "edge-direct-02")
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(
            &base64::engine::general_purpose::STANDARD
                .decode(&output.data_base64)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(payload["code"], "client_key_revoked");
    }
    assert!(repo
        .upsert_agent_identity(
            &UpsertAgentIdentityRequest {
                client_id: Some("edge-direct-02".to_string()),
                client_public_key_hex: "33".repeat(32),
                display_name: None,
                tags: Vec::new(),
                replace_existing_key: true,
                confirmed: true,
                privilege_assertion: None,
            },
            &operator,
        )
        .await
        .is_err());
}

#[tokio::test]
async fn deleted_direct_identity_cannot_be_reanimated() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = identity_operator();
    repo.upsert_agent_identity(
        &UpsertAgentIdentityRequest {
            client_id: Some("edge-direct-03".to_string()),
            client_public_key_hex: "44".repeat(32),
            display_name: Some("NRT edge direct 03".to_string()),
            tags: vec!["role:edge".to_string()],
            replace_existing_key: false,
            confirmed: true,
            privilege_assertion: None,
        },
        &operator,
    )
    .await
    .unwrap();

    repo.delete_agent(
        "edge-direct-03",
        &DeleteAgentRequest {
            confirmed: true,
            reason: Some("contract terminated".to_string()),
            privilege_assertion: None,
        },
        &operator,
    )
    .await
    .unwrap();

    assert!(!repo
        .validate_agent_public_key("edge-direct-03", &"44".repeat(32))
        .await
        .unwrap());
    assert!(repo
        .upsert_agent_identity(
            &UpsertAgentIdentityRequest {
                client_id: Some("edge-direct-03".to_string()),
                client_public_key_hex: "55".repeat(32),
                display_name: Some("new NRT edge".to_string()),
                tags: Vec::new(),
                replace_existing_key: true,
                confirmed: true,
                privilege_assertion: None,
            },
            &operator,
        )
        .await
        .is_err());
}

#[tokio::test]
async fn memory_agent_inventory_preserves_unprivileged_capability_snapshot() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "client-user-mode".to_string(),
                process_incarnation_id: uuid::Uuid::new_v4(),
                agent_version: "test".to_string(),
                os_release: "test".to_string(),
                arch: "x86_64".to_string(),
                update_heartbeat: None,
                internal_build_number: 1,
                capabilities: AgentCapabilitySnapshot {
                    privilege_mode: AgentPrivilegeMode::Unprivileged,
                    effective_uid: Some(1000),
                    max_job_timeout_secs: 3600,
                    can_attempt_privileged_ops: true,
                    can_manage_runtime_tunnels: false,
                    can_apply_process_limits: false,
                    unprivileged_hint: Some(
                        "root-only operations require forced best-effort or a root agent"
                            .to_string(),
                    ),
                },
            },
        )
        .await;
    }

    let agents = repo.list_agents().await.unwrap();

    assert_eq!(agents.len(), 1);
    assert_eq!(
        agents[0].capabilities.privilege_mode,
        AgentPrivilegeMode::Unprivileged
    );
    assert_eq!(agents[0].capabilities.effective_uid, Some(1000));
    assert!(agents[0].capabilities.can_attempt_privileged_ops);
    assert!(!agents[0].capabilities.can_manage_runtime_tunnels);
    assert!(!agents[0].capabilities.can_apply_process_limits);
    assert!(agents[0].capabilities.unprivileged_hint.is_some());
}

#[tokio::test]
async fn trust_root_routes_require_request_bound_privilege_assertion() {
    let state = identity_route_test_state(crate::gateway_client::GatewayDispatchClient::new(
        Some("http://127.0.0.1:9".to_string()),
        Some("gateway-secret-at-least-32-characters".to_string()),
    ));
    let headers = crate::test_auth_headers(&state).await;

    let error = routes_key_lifecycle::upsert_agent_identity(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::Json(UpsertAgentIdentityRequest {
            client_id: Some("edge-route-privilege".to_string()),
            client_public_key_hex: "66".repeat(32),
            display_name: Some("route privilege".to_string()),
            tags: Vec::new(),
            replace_existing_key: false,
            confirmed: true,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::FORBIDDEN);
    assert_eq!(error.code, "privilege_assertion_required");

    state
        .repo
        .upsert_agent_identity(
            &UpsertAgentIdentityRequest {
                client_id: Some("edge-route-delete".to_string()),
                client_public_key_hex: "77".repeat(32),
                display_name: Some("route delete".to_string()),
                tags: Vec::new(),
                replace_existing_key: false,
                confirmed: true,
                privilege_assertion: None,
            },
            &identity_operator(),
        )
        .await
        .unwrap();

    let error = routes_inventory::delete_agent(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::extract::Path("edge-route-delete".to_string()),
        axum::Json(DeleteAgentRequest {
            confirmed: true,
            reason: Some("route test".to_string()),
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::FORBIDDEN);
    assert_eq!(error.code, "privilege_assertion_required");

    let error = routes_key_lifecycle::revoke_current_client_key(
        axum::extract::State(state),
        headers,
        axum::extract::Path("edge-route-delete".to_string()),
        axum::Json(CreateClientKeyRevocationRequest {
            confirmed: true,
            reason: Some("route test".to_string()),
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::FORBIDDEN);
    assert_eq!(error.code, "privilege_assertion_required");
}

#[tokio::test]
async fn trust_root_routes_fail_closed_without_gateway_control() {
    let state = identity_route_test_state(crate::gateway_client::GatewayDispatchClient::default());
    let headers = crate::test_auth_headers(&state).await;

    let error = routes_key_lifecycle::upsert_agent_identity(
        axum::extract::State(state),
        headers,
        axum::Json(UpsertAgentIdentityRequest {
            client_id: Some("edge-route-no-gateway".to_string()),
            client_public_key_hex: "88".repeat(32),
            display_name: Some("route no gateway".to_string()),
            tags: Vec::new(),
            replace_existing_key: false,
            confirmed: true,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(error.code, "gateway_control_url_missing");
}

#[tokio::test]
async fn rejected_identity_rotation_does_not_request_gateway_disconnect() {
    let (gateway_url, observed_paths, gateway_task) = spawn_identity_gateway_recorder().await;
    let state = identity_route_test_state(crate::gateway_client::GatewayDispatchClient::new(
        Some(gateway_url),
        Some("gateway-secret-at-least-32-characters".to_string()),
    ));
    let headers = crate::test_auth_headers(&state).await;
    let operator = identity_operator();

    state
        .repo
        .upsert_agent_identity(
            &UpsertAgentIdentityRequest {
                client_id: Some("edge-route-rotation".to_string()),
                client_public_key_hex: "99".repeat(32),
                display_name: Some("route rotation".to_string()),
                tags: Vec::new(),
                replace_existing_key: false,
                confirmed: true,
                privilege_assertion: None,
            },
            &operator,
        )
        .await
        .unwrap();
    if let Repository::Memory(memory) = &state.repo {
        memory
            .client_key_revocations
            .write()
            .await
            .push(ClientKeyRevocationView {
                id: Uuid::new_v4(),
                client_id: "edge-route-rotation".to_string(),
                public_key_sha256_hex: crate::repository_key_lifecycle::public_key_sha256_hex(
                    &[0xaa; 32],
                ),
                reason: Some("previously revoked".to_string()),
                revoked_by: Some(operator.operator.id),
                created_at: crate::unix_now().to_string(),
            });
    }

    let error = routes_key_lifecycle::upsert_agent_identity(
        axum::extract::State(state),
        headers,
        axum::Json(UpsertAgentIdentityRequest {
            client_id: Some("edge-route-rotation".to_string()),
            client_public_key_hex: "aa".repeat(32),
            display_name: None,
            tags: Vec::new(),
            replace_existing_key: true,
            confirmed: true,
            privilege_assertion: Some(dummy_privilege_assertion()),
        }),
    )
    .await
    .unwrap_err();

    assert!(
        error
            .error
            .to_string()
            .contains("agent_identity_key_revoked"),
        "unexpected error: {:?}",
        error
    );
    gateway_task.await.unwrap();
    let paths = observed_paths.lock().await.clone();
    assert_eq!(paths, vec!["/internal/v1/gateway/privilege/verify"]);
}

fn identity_operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "identity-admin".to_string(),
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

fn dummy_privilege_assertion() -> PrivilegeAssertion {
    PrivilegeAssertion {
        nonce_hex: "a".repeat(32),
        issued_unix: 1,
        expires_unix: 2,
        assertion_hex: "b".repeat(64),
    }
}

async fn spawn_identity_gateway_recorder(
) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_url = format!("http://{}", listener.local_addr().unwrap());
    let paths = Arc::new(Mutex::new(Vec::new()));
    let observed_paths = paths.clone();
    let task = tokio::spawn(async move {
        loop {
            let accepted =
                tokio::time::timeout(Duration::from_millis(250), listener.accept()).await;
            let Ok(Ok((mut socket, _))) = accepted else {
                break;
            };
            let path = read_identity_gateway_path(&mut socket).await;
            observed_paths.lock().await.push(path.clone());
            if path == "/internal/v1/gateway/privilege/verify" {
                write_identity_gateway_json(
                    &mut socket,
                    &GatewayPrivilegeVerificationResult {
                        approved: true,
                        intent_hash_hex: "fake-gateway-approved".to_string(),
                        message: "approved".to_string(),
                    },
                )
                .await;
            } else if path == "/internal/v1/gateway/session/disconnect" {
                write_identity_gateway_json(
                    &mut socket,
                    &GatewaySessionDisconnectResult {
                        client_id: "edge-route-rotation".to_string(),
                        accepted: true,
                        disconnected: true,
                        message: "disconnect_requested".to_string(),
                    },
                )
                .await;
            } else {
                socket
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await
                    .unwrap();
            }
        }
    });
    (gateway_url, paths, task)
}

async fn read_identity_gateway_path(socket: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 4096];
        let read = socket.read(&mut chunk).await.unwrap();
        assert_ne!(read, 0, "gateway request ended before headers");
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(header_end) = find_identity_gateway_header_end(&bytes) {
            break header_end;
        }
        assert!(
            bytes.len() <= 64 * 1024,
            "gateway request headers too large"
        );
    };
    let head = std::str::from_utf8(&bytes[..header_end]).unwrap();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap();
    let mut request_parts = request_line.split_whitespace();
    let _method = request_parts.next().unwrap();
    let path = request_parts.next().unwrap().to_string();
    let content_length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let body_start = header_end + 4;
    while bytes.len() < body_start + content_length {
        let mut chunk = [0_u8; 4096];
        let read = socket.read(&mut chunk).await.unwrap();
        assert_ne!(read, 0, "gateway request ended before body");
        bytes.extend_from_slice(&chunk[..read]);
    }
    path
}

async fn write_identity_gateway_json<T: serde::Serialize>(socket: &mut TcpStream, value: &T) {
    let body = serde_json::to_vec(value).unwrap();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    socket.write_all(response.as_bytes()).await.unwrap();
    socket.write_all(&body).await.unwrap();
}

fn find_identity_gateway_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn identity_route_test_state(
    gateway: crate::gateway_client::GatewayDispatchClient,
) -> crate::state::AppState {
    let (events, _) = tokio::sync::broadcast::channel(1);
    crate::state::AppState {
        repo: Repository::Memory(MemoryState::default()),
        events,
        internal_token: Some("gateway-secret-at-least-32-characters".to_string()),
        gateway,
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
