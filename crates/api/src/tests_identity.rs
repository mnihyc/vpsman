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
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let first_created = repo
        .create_enrollment_token(
            &CreateEnrollmentTokenRequest {
                ttl_secs: Some(600),
                purpose: None,
                allowed_client_id: None,
                confirmed_reenrollment: false,
                preserve_existing_assignments: None,
                default_tags: vec!["provider:alpha".to_string()],
                default_display_name: None,
                unmanaged_update_enabled: None,
                unmanaged_update_version_url: None,
                unmanaged_update_interval_secs: None,
                unmanaged_update_jitter_secs: None,
                unmanaged_update_activate: None,
                unmanaged_update_restart_agent: None,
            },
            &operator,
        )
        .await
        .unwrap();
    let client_id = first_created.assigned_client_id.clone().unwrap();
    repo.claim_enrollment(
        &EnrollmentSettings::default(),
        &ClaimEnrollmentRequest {
            token: first_created.token,
            client_public_key_hex: "11".repeat(32),
        },
    )
    .await
    .unwrap();

    repo.update_agent_alias(&client_id, "edge-after-rebuild")
        .await
        .unwrap();
    repo.assign_agent_tag(&client_id, "region:sfo")
        .await
        .unwrap();
    repo.assign_agent_tag(&client_id, "edge").await.unwrap();
    repo.assign_agent_tag(&client_id, "os:debian")
        .await
        .unwrap();
    repo.assign_agent_tag(&client_id, "panel:custom")
        .await
        .unwrap();

    let second_token = repo
        .create_enrollment_token(
            &CreateEnrollmentTokenRequest {
                ttl_secs: Some(600),
                purpose: Some(ENROLLMENT_PURPOSE_REBUILD_REENROLLMENT.to_string()),
                allowed_client_id: Some(client_id.clone()),
                confirmed_reenrollment: true,
                preserve_existing_assignments: Some(true),
                default_tags: vec!["rebuilt".to_string()],
                default_display_name: None,
                unmanaged_update_enabled: None,
                unmanaged_update_version_url: None,
                unmanaged_update_interval_secs: None,
                unmanaged_update_jitter_secs: None,
                unmanaged_update_activate: None,
                unmanaged_update_restart_agent: None,
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
            client_public_key_hex: "22".repeat(32),
        },
    )
    .await
    .unwrap();

    let agents = repo.list_agents().await.unwrap();

    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].id, client_id);
    assert_eq!(agents[0].display_name, "edge-after-rebuild");
    assert_eq!(agents[0].status, "offline");
    assert_eq!(
        agents[0].tags,
        vec![
            "country:US".to_string(),
            "edge".to_string(),
            "os:debian".to_string(),
            "panel:custom".to_string(),
            "provider:alpha".to_string(),
            "rebuilt".to_string(),
            "region:sfo".to_string()
        ]
    );
    assert!(repo
        .validate_agent_public_key(&client_id, &"22".repeat(32))
        .await
        .unwrap());
    assert!(!repo
        .validate_agent_public_key(&client_id, &"11".repeat(32))
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
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let first_created = repo
        .create_enrollment_token(
            &CreateEnrollmentTokenRequest {
                ttl_secs: Some(600),
                purpose: None,
                allowed_client_id: None,
                confirmed_reenrollment: false,
                preserve_existing_assignments: None,
                default_tags: Vec::new(),
                default_display_name: None,
                unmanaged_update_enabled: None,
                unmanaged_update_version_url: None,
                unmanaged_update_interval_secs: None,
                unmanaged_update_jitter_secs: None,
                unmanaged_update_activate: None,
                unmanaged_update_restart_agent: None,
            },
            &operator,
        )
        .await
        .unwrap();
    let client_id = first_created.assigned_client_id.clone().unwrap();
    repo.claim_enrollment(
        &EnrollmentSettings::default(),
        &ClaimEnrollmentRequest {
            token: first_created.token,
            client_public_key_hex: "11".repeat(32),
        },
    )
    .await
    .unwrap();

    assert!(repo
        .validate_agent_public_key(&client_id, &"11".repeat(32))
        .await
        .unwrap());

    let revoked = repo
        .revoke_current_client_key(
            &client_id,
            &CreateClientKeyRevocationRequest {
                confirmed: true,
                reason: Some("rebuilt".to_string()),
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(revoked.client_id, client_id);
    assert_eq!(revoked.reason.as_deref(), Some("rebuilt"));
    assert!(!repo
        .validate_agent_public_key(&client_id, &"11".repeat(32))
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
    assert!(report.clients.is_empty());
    assert!(repo.list_agents().await.unwrap().is_empty());

    let second_token = repo
        .create_enrollment_token(
            &CreateEnrollmentTokenRequest {
                ttl_secs: Some(600),
                purpose: Some(ENROLLMENT_PURPOSE_REBUILD_REENROLLMENT.to_string()),
                allowed_client_id: Some(client_id.clone()),
                confirmed_reenrollment: true,
                preserve_existing_assignments: Some(true),
                default_tags: vec!["rebuilt".to_string()],
                default_display_name: None,
                unmanaged_update_enabled: None,
                unmanaged_update_version_url: None,
                unmanaged_update_interval_secs: None,
                unmanaged_update_jitter_secs: None,
                unmanaged_update_activate: None,
                unmanaged_update_restart_agent: None,
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
            client_public_key_hex: "22".repeat(32),
        },
    )
    .await
    .unwrap();

    assert!(repo
        .validate_agent_public_key(&client_id, &"22".repeat(32))
        .await
        .unwrap());
    assert!(!repo
        .validate_agent_public_key(&client_id, &"11".repeat(32))
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
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let first_created = repo
        .create_enrollment_token(
            &CreateEnrollmentTokenRequest {
                ttl_secs: Some(600),
                purpose: None,
                allowed_client_id: None,
                confirmed_reenrollment: false,
                preserve_existing_assignments: None,
                default_tags: Vec::new(),
                default_display_name: None,
                unmanaged_update_enabled: None,
                unmanaged_update_version_url: None,
                unmanaged_update_interval_secs: None,
                unmanaged_update_jitter_secs: None,
                unmanaged_update_activate: None,
                unmanaged_update_restart_agent: None,
            },
            &operator,
        )
        .await
        .unwrap();
    let client_id = first_created.assigned_client_id.clone().unwrap();
    repo.claim_enrollment(
        &EnrollmentSettings::default(),
        &ClaimEnrollmentRequest {
            token: first_created.token,
            client_public_key_hex: "11".repeat(32),
        },
    )
    .await
    .unwrap();

    let normal_created = repo
        .create_enrollment_token(
            &CreateEnrollmentTokenRequest {
                ttl_secs: Some(600),
                purpose: None,
                allowed_client_id: None,
                confirmed_reenrollment: false,
                preserve_existing_assignments: None,
                default_tags: Vec::new(),
                default_display_name: None,
                unmanaged_update_enabled: None,
                unmanaged_update_version_url: None,
                unmanaged_update_interval_secs: None,
                unmanaged_update_jitter_secs: None,
                unmanaged_update_activate: None,
                unmanaged_update_restart_agent: None,
            },
            &operator,
        )
        .await
        .unwrap();
    let normal_client_id = normal_created.assigned_client_id.clone().unwrap();

    let normal_response = repo
        .claim_enrollment(
            &EnrollmentSettings::default(),
            &ClaimEnrollmentRequest {
                token: normal_created.token,
                client_public_key_hex: "22".repeat(32),
            },
        )
        .await
        .unwrap();
    let EnrollmentClaimOutcome::Accepted(normal_response) = normal_response else {
        panic!("expected separate provision enrollment");
    };
    assert_eq!(normal_response.client_id, normal_client_id);
    assert!(repo
        .validate_agent_public_key(&client_id, &"11".repeat(32))
        .await
        .unwrap());
    assert!(!repo
        .validate_agent_public_key(&client_id, &"22".repeat(32))
        .await
        .unwrap());

    let reenrollment_token = repo
        .create_enrollment_token(
            &CreateEnrollmentTokenRequest {
                ttl_secs: Some(600),
                purpose: Some(ENROLLMENT_PURPOSE_REBUILD_REENROLLMENT.to_string()),
                allowed_client_id: Some(client_id.clone()),
                confirmed_reenrollment: true,
                preserve_existing_assignments: Some(true),
                default_tags: Vec::new(),
                default_display_name: None,
                unmanaged_update_enabled: None,
                unmanaged_update_version_url: None,
                unmanaged_update_interval_secs: None,
                unmanaged_update_jitter_secs: None,
                unmanaged_update_activate: None,
                unmanaged_update_restart_agent: None,
            },
            &operator,
        )
        .await
        .unwrap()
        .token;

    let reenrollment_response = repo
        .claim_enrollment(
            &EnrollmentSettings::default(),
            &ClaimEnrollmentRequest {
                token: reenrollment_token,
                client_public_key_hex: "33".repeat(32),
            },
        )
        .await
        .unwrap();
    let EnrollmentClaimOutcome::Accepted(reenrollment_response) = reenrollment_response else {
        panic!("expected bound re-enrollment");
    };
    assert_eq!(reenrollment_response.client_id, client_id);
    assert!(repo
        .validate_agent_public_key(&client_id, &"33".repeat(32))
        .await
        .unwrap());
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
                internal_build_number: 1,
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
