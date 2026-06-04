use super::*;
use crate::repository_enrollment::ENROLLMENT_PURPOSE_REBUILD_REENROLLMENT;
use crate::repository_key_lifecycle::KeyLifecycleTrustReport;
use vpsman_common::{AgentCapabilitySnapshot, AgentHello, AgentPrivilegeMode};

#[tokio::test]
async fn rebuilt_client_reenrollment_rotates_key_and_preserves_server_state() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "memory-dev".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let first_token = repo
        .create_enrollment_token(
            &CreateEnrollmentTokenRequest {
                ttl_secs: Some(600),
                purpose: None,
                allowed_client_id: None,
                confirmed_reenrollment: false,
                preserve_existing_assignments: None,
                default_tags: vec!["provider:alpha".to_string()],
                default_pool_name: None,
                default_display_name: None,
            },
            &operator,
        )
        .await
        .unwrap()
        .token;
    repo.claim_enrollment(
        &EnrollmentSettings::default(),
        &ClaimEnrollmentRequest {
            token: first_token,
            client_id: "client-rebuild".to_string(),
            client_public_key_hex: "11".repeat(32),
        },
    )
    .await
    .unwrap();

    let pool = repo
        .create_pool(CreatePoolRequest {
            name: "alpha-sfo".to_string(),
            provider: Some("alpha".to_string()),
            region: Some("sfo".to_string()),
        })
        .await
        .unwrap();
    repo.assign_agent_pool("client-rebuild", pool.id)
        .await
        .unwrap();
    repo.update_agent_alias("client-rebuild", "edge-after-rebuild")
        .await
        .unwrap();
    repo.assign_agent_tag("client-rebuild", "edge")
        .await
        .unwrap();
    repo.assign_agent_tag("client-rebuild", "os:debian")
        .await
        .unwrap();
    repo.assign_agent_tag("client-rebuild", "panel:custom")
        .await
        .unwrap();

    let second_token = repo
        .create_enrollment_token(
            &CreateEnrollmentTokenRequest {
                ttl_secs: Some(600),
                purpose: Some(ENROLLMENT_PURPOSE_REBUILD_REENROLLMENT.to_string()),
                allowed_client_id: Some("client-rebuild".to_string()),
                confirmed_reenrollment: true,
                preserve_existing_assignments: Some(true),
                default_tags: vec!["rebuilt".to_string()],
                default_pool_name: None,
                default_display_name: None,
            },
            &operator,
        )
        .await
        .unwrap()
        .token;
    repo.claim_enrollment(
        &EnrollmentSettings::default(),
        &ClaimEnrollmentRequest {
            token: second_token,
            client_id: "client-rebuild".to_string(),
            client_public_key_hex: "22".repeat(32),
        },
    )
    .await
    .unwrap();

    let agents = repo.list_agents().await.unwrap();
    let pool = repo.pool_by_id(pool.id).await.unwrap();

    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].id, "client-rebuild");
    assert_eq!(agents[0].display_name, "edge-after-rebuild");
    assert_eq!(agents[0].status, "enrolled");
    assert_eq!(
        agents[0].tags,
        vec![
            "country:US".to_string(),
            "edge".to_string(),
            "os:debian".to_string(),
            "panel:custom".to_string(),
            "provider:alpha".to_string(),
            "rebuilt".to_string()
        ]
    );
    assert_eq!(pool.clients.len(), 1);
    assert_eq!(pool.clients[0].id, "client-rebuild");
    assert!(repo
        .validate_agent_public_key("client-rebuild", &"22".repeat(32))
        .await
        .unwrap());
    assert!(!repo
        .validate_agent_public_key("client-rebuild", &"11".repeat(32))
        .await
        .unwrap());
}

#[tokio::test]
async fn client_key_revocation_blocks_current_key_until_confirmed_reenrollment() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "memory-dev".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let first_token = repo
        .create_enrollment_token(
            &CreateEnrollmentTokenRequest {
                ttl_secs: Some(600),
                purpose: None,
                allowed_client_id: None,
                confirmed_reenrollment: false,
                preserve_existing_assignments: None,
                default_tags: Vec::new(),
                default_pool_name: None,
                default_display_name: None,
            },
            &operator,
        )
        .await
        .unwrap()
        .token;
    repo.claim_enrollment(
        &EnrollmentSettings::default(),
        &ClaimEnrollmentRequest {
            token: first_token,
            client_id: "client-revoke".to_string(),
            client_public_key_hex: "11".repeat(32),
        },
    )
    .await
    .unwrap();

    assert!(repo
        .validate_agent_public_key("client-revoke", &"11".repeat(32))
        .await
        .unwrap());

    let revoked = repo
        .revoke_current_client_key(
            "client-revoke",
            &CreateClientKeyRevocationRequest {
                confirmed: true,
                reason: Some("rebuilt".to_string()),
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(revoked.client_id, "client-revoke");
    assert_eq!(revoked.reason.as_deref(), Some("rebuilt"));
    assert!(!repo
        .validate_agent_public_key("client-revoke", &"11".repeat(32))
        .await
        .unwrap());

    let report = repo
        .key_lifecycle_report(KeyLifecycleTrustReport {
            server_ed25519_public_key_configured: true,
            discovery_trusted_server_key_count: 1,
            gateway_server_public_key_configured: true,
        })
        .await
        .unwrap();
    assert_eq!(report.current_key_revoked_count, 1);
    assert_eq!(report.revocation_count, 1);
    assert!(report.clients[0].current_key_revoked);

    let second_token = repo
        .create_enrollment_token(
            &CreateEnrollmentTokenRequest {
                ttl_secs: Some(600),
                purpose: Some(ENROLLMENT_PURPOSE_REBUILD_REENROLLMENT.to_string()),
                allowed_client_id: Some("client-revoke".to_string()),
                confirmed_reenrollment: true,
                preserve_existing_assignments: Some(true),
                default_tags: vec!["rebuilt".to_string()],
                default_pool_name: None,
                default_display_name: None,
            },
            &operator,
        )
        .await
        .unwrap()
        .token;
    repo.claim_enrollment(
        &EnrollmentSettings::default(),
        &ClaimEnrollmentRequest {
            token: second_token,
            client_id: "client-revoke".to_string(),
            client_public_key_hex: "22".repeat(32),
        },
    )
    .await
    .unwrap();

    assert!(repo
        .validate_agent_public_key("client-revoke", &"22".repeat(32))
        .await
        .unwrap());
    assert!(!repo
        .validate_agent_public_key("client-revoke", &"11".repeat(32))
        .await
        .unwrap());
    let report = repo
        .key_lifecycle_report(KeyLifecycleTrustReport {
            server_ed25519_public_key_configured: true,
            discovery_trusted_server_key_count: 1,
            gateway_server_public_key_configured: true,
        })
        .await
        .unwrap();
    assert_eq!(report.current_key_revoked_count, 0);
    assert_eq!(report.revocation_count, 1);
    assert_eq!(report.rebuild_reenrollment_token_count, 1);
    assert!(!report.clients[0].current_key_revoked);
}

#[tokio::test]
async fn existing_client_key_rotation_requires_bound_reenrollment_token() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "memory-dev".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let first_token = repo
        .create_enrollment_token(
            &CreateEnrollmentTokenRequest {
                ttl_secs: Some(600),
                purpose: None,
                allowed_client_id: None,
                confirmed_reenrollment: false,
                preserve_existing_assignments: None,
                default_tags: Vec::new(),
                default_pool_name: None,
                default_display_name: None,
            },
            &operator,
        )
        .await
        .unwrap()
        .token;
    repo.claim_enrollment(
        &EnrollmentSettings::default(),
        &ClaimEnrollmentRequest {
            token: first_token,
            client_id: "client-rebuild".to_string(),
            client_public_key_hex: "11".repeat(32),
        },
    )
    .await
    .unwrap();

    let normal_token = repo
        .create_enrollment_token(
            &CreateEnrollmentTokenRequest {
                ttl_secs: Some(600),
                purpose: None,
                allowed_client_id: None,
                confirmed_reenrollment: false,
                preserve_existing_assignments: None,
                default_tags: Vec::new(),
                default_pool_name: None,
                default_display_name: None,
            },
            &operator,
        )
        .await
        .unwrap()
        .token;

    assert!(matches!(
        repo.claim_enrollment(
            &EnrollmentSettings::default(),
            &ClaimEnrollmentRequest {
                token: normal_token,
                client_id: "client-rebuild".to_string(),
                client_public_key_hex: "22".repeat(32),
            },
        )
        .await
        .unwrap(),
        EnrollmentClaimOutcome::ExistingClientRequiresReenrollmentToken
    ));

    let wrong_client_token = repo
        .create_enrollment_token(
            &CreateEnrollmentTokenRequest {
                ttl_secs: Some(600),
                purpose: Some(ENROLLMENT_PURPOSE_REBUILD_REENROLLMENT.to_string()),
                allowed_client_id: Some("client-rebuild".to_string()),
                confirmed_reenrollment: true,
                preserve_existing_assignments: Some(true),
                default_tags: Vec::new(),
                default_pool_name: None,
                default_display_name: None,
            },
            &operator,
        )
        .await
        .unwrap()
        .token;

    assert!(matches!(
        repo.claim_enrollment(
            &EnrollmentSettings::default(),
            &ClaimEnrollmentRequest {
                token: wrong_client_token,
                client_id: "client-other".to_string(),
                client_public_key_hex: "33".repeat(32),
            },
        )
        .await
        .unwrap(),
        EnrollmentClaimOutcome::TokenClientMismatch
    ));
}

#[tokio::test]
async fn memory_agent_inventory_preserves_unprivileged_capability_snapshot() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "client-user-mode".to_string(),
                agent_version: "test".to_string(),
                os_release: "test".to_string(),
                arch: "x86_64".to_string(),
                update_heartbeat: None,
                capabilities: AgentCapabilitySnapshot {
                    privilege_mode: AgentPrivilegeMode::Unprivileged,
                    effective_uid: Some(1000),
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
