use super::*;
use vpsman_common::{AgentCapabilitySnapshot, AgentHello, AgentPrivilegeMode};

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
async fn direct_agent_identity_key_change_requires_explicit_replace_and_blocks_revoked_key() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = identity_operator();
    repo.upsert_agent_identity(
        &UpsertAgentIdentityRequest {
            client_id: Some("edge-direct-02".to_string()),
            client_public_key_hex: "22".repeat(32),
            display_name: Some("SJC edge direct 02".to_string()),
            tags: Vec::new(),
            replace_existing_key: false,
            confirmed: true,
        },
        &operator,
    )
    .await
    .unwrap();

    assert!(repo
        .upsert_agent_identity(
            &UpsertAgentIdentityRequest {
                client_id: Some("edge-direct-02".to_string()),
                client_public_key_hex: "33".repeat(32),
                display_name: None,
                tags: Vec::new(),
                replace_existing_key: false,
                confirmed: true,
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
        },
        &operator,
    )
    .await
    .unwrap();
    assert!(repo
        .validate_agent_public_key("edge-direct-02", &"33".repeat(32))
        .await
        .unwrap());

    repo.revoke_current_client_key(
        "edge-direct-02",
        &CreateClientKeyRevocationRequest {
            confirmed: true,
            reason: Some("provider rebuild with compromised disk snapshot".to_string()),
        },
        &operator,
    )
    .await
    .unwrap();
    assert!(!repo
        .validate_agent_public_key("edge-direct-02", &"33".repeat(32))
        .await
        .unwrap());
    assert!(repo
        .upsert_agent_identity(
            &UpsertAgentIdentityRequest {
                client_id: Some("edge-direct-02".to_string()),
                client_public_key_hex: "33".repeat(32),
                display_name: None,
                tags: Vec::new(),
                replace_existing_key: true,
                confirmed: true,
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
                    command_timeout_secs: 3600,
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
