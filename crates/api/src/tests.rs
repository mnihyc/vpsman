use super::*;
use std::collections::HashMap;
use vpsman_common::{
    sign_privilege_proof, verify_command_envelope, verify_discovery_document_signature, AgentHello,
    AgentNoiseMode, CommandEnvelope, JobCommand, PrivilegeReplayCache,
};

#[test]
fn discovery_document_uses_configured_gateway_endpoints() {
    let signing_key = SigningKey::from_bytes(&[15_u8; 32]);
    let settings = EnrollmentSettings {
        tcp_endpoints: vec![
            vpsman_common::ServerEndpoint {
                label: "primary".to_string(),
                tcp_addr: "198.51.100.10:9443".to_string(),
                priority: 10,
            },
            vpsman_common::ServerEndpoint {
                label: "backup".to_string(),
                tcp_addr: "203.0.113.20:9443".to_string(),
                priority: 20,
            },
        ],
        discovery_url: Some("https://panel.example/.well-known/vpsman/endpoints.json".to_string()),
        ..EnrollmentSettings::default()
    };

    let document =
        routes_discovery::build_discovery_document(&settings, 1_700_000_000, Some(&signing_key));

    assert_eq!(document.version, 1);
    assert_eq!(document.issued_unix, 1_700_000_000);
    assert_eq!(document.expires_unix, 1_700_000_060);
    assert_eq!(document.endpoints, settings.tcp_endpoints);
    assert!(verify_discovery_document_signature(
        &signing_key.verifying_key(),
        &document
    ));
}

#[tokio::test]
async fn memory_namespaced_tags_participate_in_bulk_resolution() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "client-a".to_string(),
                agent_version: "test".to_string(),
                os_release: "test".to_string(),
                arch: "x86_64".to_string(),
                update_heartbeat: None,
                capabilities: Default::default(),
            },
        )
        .await;
    }

    repo.assign_agent_tag("client-a", "provider:provider-a")
        .await
        .unwrap();
    repo.assign_agent_tag("client-a", "country:US")
        .await
        .unwrap();
    let targets = repo
        .resolve_bulk_targets(&BulkResolveRequest {
            clients: Vec::new(),
            tags: vec!["provider:provider-a".to_string(), "country:US".to_string()],
            tag_mode: None,
            destructive: true,
            confirmed: false,
        })
        .await
        .unwrap();

    assert_eq!(targets.target_count, 1);
    assert!(targets.confirmation_required);
    assert_eq!(targets.targets[0].id, "client-a");

    let explicit_tag_selector = repo
        .resolve_bulk_targets(&BulkResolveRequest {
            clients: Vec::new(),
            tags: vec![
                "tag:provider:provider-a".to_string(),
                "provider:provider-a".to_string(),
                "country:US".to_string(),
            ],
            tag_mode: Some("all".to_string()),
            destructive: false,
            confirmed: false,
        })
        .await
        .unwrap();
    assert_eq!(explicit_tag_selector.target_count, 1);
    assert_eq!(explicit_tag_selector.targets[0].id, "client-a");

    let inner_any = repo
        .resolve_bulk_targets(&BulkResolveRequest {
            clients: Vec::new(),
            tags: vec!["id:client-a".to_string()],
            tag_mode: None,
            destructive: false,
            confirmed: false,
        })
        .await
        .unwrap();
    assert_eq!(inner_any.target_count, 1);
    assert_eq!(inner_any.targets[0].id, "client-a");

    let inner_all = repo
        .resolve_bulk_targets(&BulkResolveRequest {
            clients: Vec::new(),
            tags: vec![
                "name:client-a".to_string(),
                "provider:provider-a".to_string(),
                "country:US".to_string(),
            ],
            tag_mode: Some("all".to_string()),
            destructive: false,
            confirmed: false,
        })
        .await
        .unwrap();
    assert_eq!(inner_all.target_count, 1);
    assert_eq!(inner_all.targets[0].id, "client-a");

    let mismatch = repo
        .resolve_bulk_targets(&BulkResolveRequest {
            clients: Vec::new(),
            tags: vec!["id:client-a".to_string(), "country:DE".to_string()],
            tag_mode: Some("all".to_string()),
            destructive: false,
            confirmed: false,
        })
        .await
        .unwrap();
    assert_eq!(mismatch.target_count, 0);
}

#[tokio::test]
async fn enrollment_token_claim_records_client_key_and_consumes_token() {
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
    let created = repo
        .create_enrollment_token(
            &CreateEnrollmentTokenRequest {
                ttl_secs: Some(600),
                purpose: None,
                allowed_client_id: None,
                confirmed_reenrollment: false,
                preserve_existing_assignments: None,
                default_tags: vec!["bgp".to_string(), "edge".to_string()],
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

    assert_eq!(created.token.len(), 64);
    assert_eq!(created.token_prefix.len(), 12);
    assert!(!created.token.contains("bgp"));
    let assigned_client_id = created.assigned_client_id.clone().unwrap();
    assert!(uuid::Uuid::parse_str(&assigned_client_id).is_ok());

    let response = repo
        .claim_enrollment(
            &EnrollmentSettings::default(),
            &ClaimEnrollmentRequest {
                token: created.token.clone(),
                client_id: None,
                client_public_key_hex: "11".repeat(32),
            },
        )
        .await
        .unwrap();
    let EnrollmentClaimOutcome::Accepted(response) = response else {
        panic!("expected accepted enrollment");
    };
    let agents = repo.list_agents().await.unwrap();
    let listed_tokens = repo.list_enrollment_tokens().await.unwrap();

    assert_eq!(response.client_id, assigned_client_id);
    assert_eq!(response.noise_mode, AgentNoiseMode::EnrolledIk);
    assert_eq!(
        response.tags,
        vec![
            "bgp".to_string(),
            "country:US".to_string(),
            "edge".to_string(),
        ]
    );
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].id, response.client_id);
    assert_eq!(agents[0].status, "enrolled");
    assert_eq!(listed_tokens.len(), 1);
    assert_eq!(listed_tokens[0].token_prefix, created.token_prefix);
    assert_eq!(
        listed_tokens[0].used_by_client_id.as_deref(),
        Some(response.client_id.as_str())
    );
    assert!(listed_tokens[0].used_at.is_some());
}

#[tokio::test]
async fn enrollment_token_rejects_reuse_and_never_lists_plaintext_token() {
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
    let created = repo
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
    let request = ClaimEnrollmentRequest {
        token: created.token.clone(),
        client_id: None,
        client_public_key_hex: "22".repeat(32),
    };

    let settings = EnrollmentSettings {
        discovery_trusted_server_ed25519_public_keys_hex: vec!["33".repeat(32), "44".repeat(32)],
        ..EnrollmentSettings::default()
    };
    let accepted = repo.claim_enrollment(&settings, &request).await.unwrap();
    let EnrollmentClaimOutcome::Accepted(accepted) = accepted else {
        panic!("expected accepted enrollment");
    };
    assert_eq!(
        accepted.discovery_trusted_server_ed25519_public_keys_hex,
        vec!["33".repeat(32), "44".repeat(32)]
    );
    assert!(matches!(
        repo.claim_enrollment(&EnrollmentSettings::default(), &request)
            .await
            .unwrap(),
        EnrollmentClaimOutcome::UsedToken
    ));
    assert!(matches!(
        repo.claim_enrollment(
            &EnrollmentSettings::default(),
            &ClaimEnrollmentRequest {
                token: "wrong".to_string(),
                ..request
            },
        )
        .await
        .unwrap(),
        EnrollmentClaimOutcome::InvalidToken
    ));
    let tokens_json = serde_json::to_string(&repo.list_enrollment_tokens().await.unwrap()).unwrap();

    assert!(!tokens_json.contains(&created.token));
    assert!(tokens_json.contains(&created.token_prefix));
}

#[tokio::test]
async fn gateway_identity_validation_uses_enrolled_client_public_key() {
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
    let created = repo
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
    let client_id = created.assigned_client_id.clone().unwrap();
    repo.claim_enrollment(
        &EnrollmentSettings::default(),
        &ClaimEnrollmentRequest {
            token: created.token,
            client_id: None,
            client_public_key_hex: "55".repeat(32),
        },
    )
    .await
    .unwrap();

    assert!(repo
        .validate_agent_public_key(&client_id, &"55".repeat(32))
        .await
        .unwrap());
    assert!(!repo
        .validate_agent_public_key(&client_id, &"66".repeat(32))
        .await
        .unwrap());
    assert!(!repo
        .validate_agent_public_key("missing", &"55".repeat(32))
        .await
        .unwrap());
}

#[tokio::test]
async fn rejected_job_records_frozen_target_results() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "client-a".to_string(),
                agent_version: "test".to_string(),
                os_release: "test".to_string(),
                arch: "x86_64".to_string(),
                update_heartbeat: None,
                capabilities: Default::default(),
            },
        )
        .await;
    }
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
    let request = CreateJobRequest {
        targets: vec![
            "client-a".to_string(),
            "client-a".to_string(),
            "missing-client".to_string(),
        ],
        clients: Vec::new(),
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: false,
        command: "uptime".to_string(),
        argv: Vec::new(),
        operation: None,
        timeout_secs: None,
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };

    let job_id = repo
        .record_rejected_job(
            &request,
            &payload_hash(request.command.as_bytes()),
            &operator,
        )
        .await
        .unwrap();
    let jobs = repo.list_jobs(10).await.unwrap();
    let targets = repo.list_job_targets(job_id).await.unwrap();

    assert_eq!(jobs[0].target_count, 1);
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].client_id, "client-a");
    assert_eq!(targets[0].status, "rejected_authorization_required");
    assert!(targets[0].completed_at.is_some());
}

#[tokio::test]
async fn rejected_job_freezes_tag_targets() {
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
    repo.assign_agent_tag("client-a", "edge").await.unwrap();
    repo.assign_agent_tag("client-a", "provider:provider-a")
        .await
        .unwrap();
    repo.assign_agent_tag("client-b", "bgp").await.unwrap();
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
    let request = CreateJobRequest {
        targets: Vec::new(),
        clients: Vec::new(),
        tags: vec!["provider:provider-a".to_string(), "bgp".to_string()],
        tag_mode: None,
        destructive: true,
        confirmed: true,
        command: "uptime".to_string(),
        argv: Vec::new(),
        operation: None,
        timeout_secs: None,
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };

    let job_id = repo
        .record_rejected_job(
            &request,
            &payload_hash(request.command.as_bytes()),
            &operator,
        )
        .await
        .unwrap();
    let targets = repo.list_job_targets(job_id).await.unwrap();

    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0].client_id, "client-a");
    assert_eq!(targets[1].client_id, "client-b");
}

#[test]
fn server_signs_proof_bearing_envelope_for_resolved_target() {
    let signing_key = SigningKey::from_bytes(&[3_u8; 32]);
    let proof_key = [7_u8; 32];
    let command = JobCommand::Shell {
        argv: vec!["true".to_string()],
        pty: false,
    };
    let command_payload = encode_json(&command).unwrap();
    let command_hash = payload_hash(&command_payload);
    let command_id = Uuid::new_v4();
    let scope = "client:client-a";
    let proof = sign_privilege_proof(
        &proof_key,
        command_id,
        scope,
        &command_hash,
        &[9_u8; 16],
        unix_now() + 300,
    );
    let request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: false,
        command: "true".to_string(),
        argv: vec!["true".to_string()],
        operation: None,
        timeout_secs: Some(5),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: Some(CommandEnvelope {
            command_id,
            scope: scope.to_string(),
            payload_hash_hex: command_hash.clone(),
            proof: Some(proof),
            server_signature: vec![1, 2, 3],
        }),
        envelopes: HashMap::new(),
    };

    let signed = request
        .signed_envelopes_for_targets(&["client-a".to_string()], &command_hash, &signing_key)
        .unwrap();
    let envelope = signed.get("client-a").unwrap();
    assert_ne!(envelope.server_signature, vec![1, 2, 3]);
    assert!(verify_command_envelope(
        &proof_key,
        &signing_key.verifying_key(),
        scope,
        &command_payload,
        envelope,
        unix_now(),
        &mut PrivilegeReplayCache::default(),
    )
    .is_ok());
}

#[test]
fn file_pull_job_command_uses_operation_payload_and_type() {
    let request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::FilePull {
            path: "/etc/hostname".to_string(),
        }),
        timeout_secs: Some(5),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };

    assert_eq!(request.command_type_label(), "file_pull");
    match request.job_command().unwrap() {
        JobCommand::FilePull { path } => assert_eq!(path, "/etc/hostname"),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn shell_pty_job_command_uses_operation_payload_and_type() {
    let request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: false,
        command: "ignored".to_string(),
        argv: Vec::new(),
        operation: Some(JobCommand::Shell {
            argv: vec!["/usr/bin/tty".to_string()],
            pty: true,
        }),
        timeout_secs: Some(5),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };

    let command = request.job_command().unwrap();
    assert_eq!(request.command_type_label(), "shell_pty");
    match command {
        JobCommand::Shell { argv, pty } => {
            assert_eq!(argv, vec!["/usr/bin/tty".to_string()]);
            assert!(pty);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn file_pull_job_command_requires_absolute_path() {
    let request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::FilePull {
            path: "relative/path".to_string(),
        }),
        timeout_secs: Some(5),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };

    let error = request.job_command().unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
}

#[test]
fn shell_script_job_command_uses_operation_payload_and_type() {
    let request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::ShellScript {
            script: "echo vpsman".to_string(),
        }),
        timeout_secs: Some(5),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };

    assert_eq!(request.command_type_label(), "shell_script");
    match request.job_command().unwrap() {
        JobCommand::ShellScript { script } => assert_eq!(script, "echo vpsman"),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn shell_script_job_command_rejects_empty_and_control_payloads() {
    let mut request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::ShellScript {
            script: " ".to_string(),
        }),
        timeout_secs: Some(5),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };

    let error = request.job_command().unwrap_err();
    assert_eq!(error.code, "shell_script_is_empty");

    request.operation = Some(JobCommand::ShellScript {
        script: "echo ok\u{0007}".to_string(),
    });
    let error = request.job_command().unwrap_err();
    assert_eq!(error.code, "shell_script_contains_control_character");
}

#[test]
fn user_sessions_job_command_uses_operation_payload_and_type() {
    let request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::UserSessions),
        timeout_secs: Some(5),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };

    assert_eq!(request.command_type_label(), "user_sessions");
    assert!(matches!(
        request.job_command().unwrap(),
        JobCommand::UserSessions
    ));
}

#[test]
fn process_list_job_command_uses_operation_payload_and_type() {
    let request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::ProcessList { limit: 25 }),
        timeout_secs: Some(5),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };

    assert_eq!(request.command_type_label(), "process_list");
    match request.job_command().unwrap() {
        JobCommand::ProcessList { limit } => assert_eq!(limit, 25),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn process_list_job_command_bounds_limit() {
    let request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::ProcessList { limit: 0 }),
        timeout_secs: Some(5),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };

    let error = request.job_command().unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn dispatching_job_records_and_updates_target_results() {
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
    let request = CreateJobRequest {
        targets: Vec::new(),
        clients: vec!["client-a".to_string()],
        tags: Vec::new(),
        tag_mode: None,
        destructive: false,
        confirmed: false,
        command: "true".to_string(),
        argv: vec!["true".to_string()],
        operation: None,
        timeout_secs: Some(5),
        canary_count: None,
        force_unprivileged: false,
        privileged: true,
        idempotency_key: None,
        reconnect_policy: None,
        envelope: None,
        envelopes: HashMap::new(),
    };
    let command = request.job_command().unwrap();
    let command_hash = payload_hash(&encode_json(&command).unwrap());
    let job_id = repo
        .record_dispatching_job(
            &request,
            &command_hash,
            &operator,
            &["client-a".to_string()],
        )
        .await
        .unwrap();
    repo.update_job_target_result(
        job_id,
        "client-a",
        &TargetDispatchOutcome {
            status: "completed".to_string(),
            exit_code: Some(0),
            accepted: true,
            message: "ok".to_string(),
            outputs: vec![
                CommandOutput {
                    job_id,
                    stream: OutputStream::Stdout,
                    data: b"ok\n".to_vec(),
                    exit_code: None,
                    done: false,
                },
                CommandOutput {
                    job_id,
                    stream: OutputStream::Status,
                    data: Vec::new(),
                    exit_code: Some(0),
                    done: true,
                },
            ],
        },
    )
    .await
    .unwrap();
    repo.record_job_outputs(
        job_id,
        "client-a",
        &[
            CommandOutput {
                job_id,
                stream: OutputStream::Stdout,
                data: b"ok\n".to_vec(),
                exit_code: None,
                done: false,
            },
            CommandOutput {
                job_id,
                stream: OutputStream::Status,
                data: Vec::new(),
                exit_code: Some(0),
                done: true,
            },
        ],
    )
    .await
    .unwrap();
    repo.finish_job(job_id, "completed").await.unwrap();

    let jobs = repo.list_jobs(10).await.unwrap();
    let targets = repo.list_job_targets(job_id).await.unwrap();
    let outputs = repo.list_job_outputs(job_id).await.unwrap();

    assert_eq!(jobs[0].status, "completed");
    assert!(jobs[0].completed_at.is_some());
    assert_eq!(targets[0].status, "completed");
    assert_eq!(targets[0].exit_code, Some(0));
    assert!(targets[0].completed_at.is_some());
    assert_eq!(outputs.len(), 2);
    assert_eq!(outputs[0].client_id, "client-a");
    assert_eq!(outputs[0].stream, "stdout");
    assert_eq!(outputs[0].data_base64, "b2sK");
    assert_eq!(outputs[1].stream, "status");
    assert_eq!(outputs[1].exit_code, Some(0));
    assert!(outputs[1].done);
}
