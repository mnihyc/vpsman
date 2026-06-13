use super::*;
use vpsman_common::{
    job_command_type_label, plan_tunnel, AgentHello, BandwidthTier, GatewayAgentHelloIngest,
    JobCommand, OspfCostPolicy, TunnelEndpointSide, TunnelKind, TunnelPlanInput,
};

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
                internal_build_number: 1,
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
            selector_expression: "provider:provider-a || country:US".to_string(),
        })
        .await
        .unwrap();

    assert_eq!(targets.target_count, 1);
    assert_eq!(targets.targets[0].id, "client-a");

    let explicit_tag_selector = repo
        .resolve_bulk_targets(&BulkResolveRequest {
            selector_expression: "tag:provider:provider-a && provider:provider-a && country:US"
                .to_string(),
        })
        .await
        .unwrap();
    assert_eq!(explicit_tag_selector.target_count, 1);
    assert_eq!(explicit_tag_selector.targets[0].id, "client-a");

    let inner_any = repo
        .resolve_bulk_targets(&BulkResolveRequest {
            selector_expression: "id:client-a".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(inner_any.target_count, 1);
    assert_eq!(inner_any.targets[0].id, "client-a");

    let inner_all = repo
        .resolve_bulk_targets(&BulkResolveRequest {
            selector_expression: "name:client-a && provider:provider-a && country:US".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(inner_all.target_count, 1);
    assert_eq!(inner_all.targets[0].id, "client-a");

    let mismatch = repo
        .resolve_bulk_targets(&BulkResolveRequest {
            selector_expression: "id:client-a && country:DE".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(mismatch.target_count, 0);
}

#[tokio::test]
async fn stale_agent_clears_only_after_changed_internal_build_hello() {
    fn hello(build: u64) -> GatewayAgentHelloIngest {
        GatewayAgentHelloIngest {
            gateway_id: "gateway-a".to_string(),
            noise_public_key_hex: None,
            remote_ip: Some("203.0.113.10".to_string()),
            hello: AgentHello {
                client_id: "client-a".to_string(),
                agent_version: "test".to_string(),
                internal_build_number: build,
                os_release: "test".to_string(),
                arch: "x86_64".to_string(),
                update_heartbeat: None,
                capabilities: Default::default(),
            },
        }
    }

    let repo = Repository::Memory(MemoryState::default());
    repo.upsert_agent_hello(&hello(1)).await.unwrap();
    repo.mark_agent_stale(
        "client-a",
        "agent_rejected_unsupported_shell_argv_command_version",
        serde_json::json!({"job_id": Uuid::nil()}),
    )
    .await
    .unwrap();

    let stale = repo.agent_by_id("client-a").await.unwrap();
    assert_eq!(stale.status, "stale");
    assert_eq!(
        stale.stale_reason.as_deref(),
        Some("agent_rejected_unsupported_shell_argv_command_version")
    );

    repo.upsert_agent_hello(&hello(1)).await.unwrap();
    assert_eq!(repo.agent_by_id("client-a").await.unwrap().status, "stale");

    repo.upsert_agent_hello(&hello(2)).await.unwrap();
    let recovered = repo.agent_by_id("client-a").await.unwrap();
    assert_eq!(recovered.status, "online");
    assert_eq!(recovered.internal_build_number, 2);
    assert!(recovered.stale_since.is_none());
    assert!(recovered.stale_reason.is_none());

    let audit_actions = repo
        .list_audit_logs(10)
        .await
        .unwrap()
        .into_iter()
        .map(|entry| entry.action)
        .collect::<Vec<_>>();
    assert!(audit_actions.contains(&"agent.status_stale".to_string()));
    assert!(audit_actions.contains(&"agent.status_online".to_string()));
}

#[tokio::test]
async fn deleting_memory_agent_removes_inventory_access_and_bulk_targets() {
    let repo = Repository::Memory(MemoryState::default());
    let session_id = Uuid::new_v4();
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "client-delete".to_string(),
                agent_version: "test".to_string(),
                os_release: "test".to_string(),
                arch: "x86_64".to_string(),
                update_heartbeat: None,
                internal_build_number: 1,
                capabilities: Default::default(),
            },
        )
        .await;
        memory
            .client_public_keys
            .write()
            .await
            .insert("client-delete".to_string(), vec![0x55; 32]);
        memory
            .gateway_sessions
            .write()
            .await
            .push(GatewaySessionView {
                id: session_id,
                gateway_id: "gateway-a".to_string(),
                client_id: "client-delete".to_string(),
                noise_public_key_hex: Some("55".repeat(32)),
                status: "active".to_string(),
                started_at: "1700000000".to_string(),
                last_seen_at: "1700000000".to_string(),
                ended_at: None,
                end_reason: None,
            });
    }
    repo.assign_agent_tag("client-delete", "provider:alpha")
        .await
        .unwrap();

    let response = repo
        .delete_agent(
            "client-delete",
            &DeleteAgentRequest {
                confirmed: true,
                reason: Some("retired".to_string()),
            },
            &test_operator(),
        )
        .await
        .unwrap();

    assert!(response.deleted);
    assert_eq!(response.client_id, "client-delete");
    assert!(repo.list_agents().await.unwrap().is_empty());
    assert_eq!(repo.fleet_summary().await.unwrap().total, 0);
    assert!(repo.list_gateway_sessions(10).await.unwrap().is_empty());
    assert!(!repo
        .validate_agent_public_key("client-delete", &"55".repeat(32))
        .await
        .unwrap());
    assert!(repo.agent_by_id("client-delete").await.is_err());
    assert!(repo
        .assign_agent_tag("client-delete", "edge")
        .await
        .is_err());
    assert!(repo
        .upsert_agent_identity(
            &UpsertAgentIdentityRequest {
                client_id: Some("client-delete".to_string()),
                client_public_key_hex: "55".repeat(32),
                display_name: Some("retired edge".to_string()),
                tags: Vec::new(),
                replace_existing_key: false,
                confirmed: true,
            },
            &test_operator(),
        )
        .await
        .is_err());

    let targets = repo
        .resolve_bulk_targets(&BulkResolveRequest {
            selector_expression: "id:client-delete || provider:alpha".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(targets.target_count, 0);

    if let Repository::Memory(memory) = &repo {
        let sessions = memory.gateway_sessions.read().await;
        let session = sessions
            .iter()
            .find(|session| session.id == session_id)
            .unwrap();
        assert_eq!(session.status, "ended");
        assert_eq!(session.end_reason.as_deref(), Some("vps_deleted"));
        assert!(memory
            .audits
            .read()
            .await
            .iter()
            .any(|entry| entry.action == "agent.deleted"));
    }
}

#[tokio::test]
async fn gateway_identity_validation_uses_direct_client_public_key() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let identity = repo
        .upsert_agent_identity(
            &UpsertAgentIdentityRequest {
                client_id: Some("direct-edge-01".to_string()),
                client_public_key_hex: "55".repeat(32),
                display_name: Some("Direct edge 01".to_string()),
                tags: vec!["role:edge".to_string()],
                replace_existing_key: false,
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();

    assert_eq!(identity.client_id, "direct-edge-01");
    assert!(repo
        .validate_agent_public_key("direct-edge-01", &"55".repeat(32))
        .await
        .unwrap());
    assert!(!repo
        .validate_agent_public_key("direct-edge-01", &"66".repeat(32))
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
                internal_build_number: 1,
                capabilities: Default::default(),
            },
        )
        .await;
    }
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "test-operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:client-a || id:client-a || id:missing-client".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: "uptime".to_string(),
        argv: Vec::new(),
        operation: None,
        timeout_secs: None,
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };

    let job_id = repo
        .record_rejected_job(
            Uuid::new_v4(),
            &request,
            &payload_hash(request.command.as_bytes()),
            "test_request_fingerprint",
            &operator,
            "rejected_authorization_required",
            "authorization required",
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
                    internal_build_number: 1,
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
            username: "test-operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "provider:provider-a || tag:bgp".to_string(),
        target_client_ids: vec!["client-a".to_string(), "client-b".to_string()],
        destructive: true,
        confirmed: true,
        command: "uptime".to_string(),
        argv: Vec::new(),
        operation: None,
        timeout_secs: None,
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };

    let job_id = repo
        .record_rejected_job(
            Uuid::new_v4(),
            &request,
            &payload_hash(request.command.as_bytes()),
            "test_request_fingerprint",
            &operator,
            "rejected_authorization_required",
            "authorization required",
        )
        .await
        .unwrap();
    let targets = repo.list_job_targets(job_id).await.unwrap();

    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0].client_id, "client-a");
    assert_eq!(targets[1].client_id, "client-b");
}

#[test]
fn file_pull_job_command_uses_operation_payload_and_type() {
    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::FilePull {
            path: "/etc/hostname".to_string(),
        }),
        timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
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
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: "ignored".to_string(),
        argv: Vec::new(),
        operation: Some(JobCommand::Shell {
            argv: vec!["/usr/bin/tty".to_string()],
            pty: true,
        }),
        timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
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
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::FilePull {
            path: "relative/path".to_string(),
        }),
        timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };

    let error = request.job_command().unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
}

#[test]
fn file_browser_job_commands_use_operation_payload_and_type() {
    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: true,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::FileListDir {
            path: "/var/log".to_string(),
            offset: 0,
            limit: 250,
            show_hidden: false,
        }),
        timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };

    assert_eq!(request.command_type_label(), "file_list_dir");
    match request.job_command().unwrap() {
        JobCommand::FileListDir { path, limit, .. } => {
            assert_eq!(path, "/var/log");
            assert_eq!(limit, 250);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn file_browser_job_commands_validate_paths_and_limits() {
    let mut request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: true,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::FileListDir {
            path: "var/log".to_string(),
            offset: 0,
            limit: 250,
            show_hidden: false,
        }),
        timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };
    assert_eq!(
        request.job_command().unwrap_err().code,
        "file_path_must_be_absolute"
    );

    request.operation = Some(JobCommand::FileListDir {
        path: "/var/log".to_string(),
        offset: 0,
        limit: 0,
        show_hidden: false,
    });
    assert_eq!(
        request.job_command().unwrap_err().code,
        "file_list_limit_out_of_range"
    );
}

#[test]
fn shell_script_job_command_uses_operation_payload_and_type() {
    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::ShellScript {
            script: "echo vpsman".to_string(),
        }),
        timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
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
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::ShellScript {
            script: " ".to_string(),
        }),
        timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
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
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::UserSessions),
        timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
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
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::ProcessList { limit: 25 }),
        timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
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
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::ProcessList { limit: 0 }),
        timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
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
            username: "test-operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: "true".to_string(),
        argv: vec!["true".to_string()],
        operation: None,
        timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };
    let command = request.job_command().unwrap();
    let command_hash = payload_hash(&encode_json(&command).unwrap());
    let job_id = repo
        .record_dispatching_job(
            Uuid::new_v4(),
            &request,
            &command_hash,
            "test_request_fingerprint",
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
            command_version: Some(1),
            accepted: true,
            message: "ok".to_string(),
            received_at: None,
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

#[tokio::test]
async fn memory_dispatch_claims_one_exclusive_target_per_client_per_batch() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let request = test_job_request(&["client-a"]);
    let command = request.job_command().unwrap();
    let command_hash = payload_hash(&encode_json(&command).unwrap());

    let first_job_id = repo
        .record_dispatching_job(
            Uuid::new_v4(),
            &request,
            &command_hash,
            "first_request_fingerprint",
            &operator,
            &["client-a".to_string()],
        )
        .await
        .unwrap();
    let second_job_id = repo
        .record_dispatching_job(
            Uuid::new_v4(),
            &request,
            &command_hash,
            "second_request_fingerprint",
            &operator,
            &["client-a".to_string()],
        )
        .await
        .unwrap();

    let first_claim = repo.claim_due_job_targets(10, 30).await.unwrap();
    assert_eq!(first_claim.len(), 1);
    assert_eq!(first_claim[0].job_id, first_job_id);
    assert_eq!(first_claim[0].client_id, "client-a");
    assert!(repo.claim_due_job_targets(10, 30).await.unwrap().is_empty());

    repo.update_job_target_result(
        first_job_id,
        "client-a",
        &TargetDispatchOutcome {
            status: "completed".to_string(),
            exit_code: Some(0),
            command_version: Some(1),
            accepted: true,
            message: "ok".to_string(),
            received_at: None,
            outputs: Vec::new(),
        },
    )
    .await
    .unwrap();

    let second_claim = repo.claim_due_job_targets(10, 30).await.unwrap();
    assert_eq!(second_claim.len(), 1);
    assert_eq!(second_claim[0].job_id, second_job_id);
    assert_eq!(second_claim[0].client_id, "client-a");
}

#[tokio::test]
async fn memory_dispatch_claim_preserves_source_schedule_id() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let request = test_job_request(&["client-a"]);
    let command = request.job_command().unwrap();
    let command_hash = payload_hash(&encode_json(&command).unwrap());
    let schedule_id = Uuid::new_v4();
    let job_id = repo
        .record_dispatching_job_from_schedule(
            Uuid::new_v4(),
            &request,
            &command_hash,
            "scheduled_request_fingerprint",
            &operator,
            &["client-a".to_string()],
            schedule_id,
        )
        .await
        .unwrap();

    let claim = repo.claim_due_job_targets(10, 30).await.unwrap();
    assert_eq!(claim.len(), 1);
    assert_eq!(claim[0].job_id, job_id);
    assert_eq!(claim[0].source_schedule_id, Some(schedule_id));
}

#[tokio::test]
async fn memory_dispatch_exclusivity_uses_operation_for_scheduled_labels() {
    for (case, operation) in exclusive_dispatch_operation_cases() {
        let scheduled_label = format!("scheduled_{}", job_command_type_label(&operation));
        let repo = Repository::Memory(MemoryState::default());
        let scheduled_job_id = record_memory_dispatch_job(
            &repo,
            operation.clone(),
            Some(Uuid::new_v4()),
            Some(scheduled_label.clone()),
            &format!("{case}_scheduled_first"),
        )
        .await;
        let direct_job_id = record_memory_dispatch_job(
            &repo,
            operation.clone(),
            None,
            None,
            &format!("{case}_direct_second"),
        )
        .await;

        let first_claim = repo.claim_due_job_targets(10, 30).await.unwrap();
        assert_eq!(first_claim.len(), 1, "{case}: scheduled claim");
        assert_eq!(first_claim[0].job_id, scheduled_job_id, "{case}");
        assert_eq!(first_claim[0].command_type, scheduled_label, "{case}");
        assert!(
            repo.claim_due_job_targets(10, 30).await.unwrap().is_empty(),
            "{case}: direct job must wait behind scheduled exclusive operation"
        );

        complete_memory_target(&repo, scheduled_job_id).await;
        let second_claim = repo.claim_due_job_targets(10, 30).await.unwrap();
        assert_eq!(second_claim.len(), 1, "{case}: direct claim");
        assert_eq!(second_claim[0].job_id, direct_job_id, "{case}");
        assert_eq!(
            second_claim[0].command_type,
            job_command_type_label(&operation),
            "{case}"
        );
    }
}

#[tokio::test]
async fn memory_dispatch_direct_exclusive_blocks_scheduled_operation() {
    for (case, operation) in exclusive_dispatch_operation_cases() {
        let scheduled_label = format!("scheduled_{}", job_command_type_label(&operation));
        let repo = Repository::Memory(MemoryState::default());
        let direct_job_id = record_memory_dispatch_job(
            &repo,
            operation.clone(),
            None,
            None,
            &format!("{case}_direct_first"),
        )
        .await;
        let scheduled_job_id = record_memory_dispatch_job(
            &repo,
            operation.clone(),
            Some(Uuid::new_v4()),
            Some(scheduled_label.clone()),
            &format!("{case}_scheduled_second"),
        )
        .await;

        let first_claim = repo.claim_due_job_targets(10, 30).await.unwrap();
        assert_eq!(first_claim.len(), 1, "{case}: direct claim");
        assert_eq!(first_claim[0].job_id, direct_job_id, "{case}");
        assert_eq!(
            first_claim[0].command_type,
            job_command_type_label(&operation),
            "{case}"
        );
        assert!(
            repo.claim_due_job_targets(10, 30).await.unwrap().is_empty(),
            "{case}: scheduled job must wait behind direct exclusive operation"
        );

        complete_memory_target(&repo, direct_job_id).await;
        let second_claim = repo.claim_due_job_targets(10, 30).await.unwrap();
        assert_eq!(second_claim.len(), 1, "{case}: scheduled claim");
        assert_eq!(second_claim[0].job_id, scheduled_job_id, "{case}");
        assert_eq!(second_claim[0].command_type, scheduled_label, "{case}");
        assert!(second_claim[0].source_schedule_id.is_some(), "{case}");
    }
}

#[tokio::test]
async fn job_output_comparison_groups_execution_summaries_by_status_and_output() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let target_clients = vec![
        "client-a".to_string(),
        "client-b".to_string(),
        "client-c".to_string(),
        "client-d".to_string(),
    ];
    let request = test_job_request(&["client-a", "client-b", "client-c", "client-d"]);
    let command = request.job_command().unwrap();
    let command_hash = payload_hash(&encode_json(&command).unwrap());
    let job_id = repo
        .record_dispatching_job(
            Uuid::new_v4(),
            &request,
            &command_hash,
            "test_request_fingerprint",
            &operator,
            &target_clients,
        )
        .await
        .unwrap();

    record_test_output(
        &repo,
        job_id,
        "client-a",
        "completed",
        Some(0),
        OutputStream::Stdout,
        b"ok\n",
    )
    .await;
    record_test_output(
        &repo,
        job_id,
        "client-b",
        "completed",
        Some(0),
        OutputStream::Stdout,
        b"ok\n",
    )
    .await;
    record_test_output(
        &repo,
        job_id,
        "client-c",
        "completed",
        Some(1),
        OutputStream::Stdout,
        b"ok\n",
    )
    .await;
    record_test_output(
        &repo,
        job_id,
        "client-d",
        "completed",
        Some(0),
        OutputStream::Stderr,
        b"ok\n",
    )
    .await;

    let comparison = repo.compare_job_outputs(job_id, "binary").await.unwrap();

    assert_eq!(comparison.mode, "binary");
    assert_eq!(comparison.total_targets, 4);
    assert_eq!(comparison.compared_targets, 4);
    assert_eq!(comparison.group_count, 3);
    assert_eq!(comparison.groups[0].target_count, 2);
    assert_eq!(
        comparison.groups[0].client_ids,
        vec!["client-a".to_string(), "client-b".to_string()]
    );
    assert!(comparison
        .rows
        .iter()
        .filter(|row| row.matches_largest_group)
        .all(|row| row.client_id == "client-a" || row.client_id == "client-b"));
}

#[tokio::test]
async fn job_output_comparison_text_mode_normalizes_line_endings_and_trailing_space() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let target_clients = vec!["client-a".to_string(), "client-b".to_string()];
    let request = test_job_request(&["client-a", "client-b"]);
    let command = request.job_command().unwrap();
    let command_hash = payload_hash(&encode_json(&command).unwrap());
    let job_id = repo
        .record_dispatching_job(
            Uuid::new_v4(),
            &request,
            &command_hash,
            "test_request_fingerprint",
            &operator,
            &target_clients,
        )
        .await
        .unwrap();

    record_test_output(
        &repo,
        job_id,
        "client-a",
        "completed",
        Some(0),
        OutputStream::Stdout,
        b"hello\r\nworld  \r\n",
    )
    .await;
    record_test_output(
        &repo,
        job_id,
        "client-b",
        "completed",
        Some(0),
        OutputStream::Stdout,
        b"hello\nworld\n",
    )
    .await;

    let binary = repo.compare_job_outputs(job_id, "binary").await.unwrap();
    let text = repo.compare_job_outputs(job_id, "text").await.unwrap();

    assert_eq!(binary.group_count, 2);
    assert_eq!(text.group_count, 1);
    assert_eq!(text.groups[0].output_compare_basis, "text");
    assert_eq!(text.groups[0].preview, "hello\nworld");
}

#[tokio::test]
async fn job_output_comparison_groups_artifact_backed_output_by_metadata() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let target_clients = vec![
        "client-a".to_string(),
        "client-b".to_string(),
        "client-c".to_string(),
    ];
    let request = test_job_request(&["client-a", "client-b", "client-c"]);
    let command = request.job_command().unwrap();
    let command_hash = payload_hash(&encode_json(&command).unwrap());
    let job_id = repo
        .record_dispatching_job(
            Uuid::new_v4(),
            &request,
            &command_hash,
            "test_request_fingerprint",
            &operator,
            &target_clients,
        )
        .await
        .unwrap();

    for client_id in &target_clients {
        let outputs = vec![CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: Vec::new(),
            exit_code: Some(0),
            done: true,
        }];
        repo.update_job_target_result(
            job_id,
            client_id,
            &TargetDispatchOutcome {
                status: "completed".to_string(),
                exit_code: Some(0),
                command_version: Some(1),
                accepted: true,
                message: "ok".to_string(),
                received_at: None,
                outputs: outputs.clone(),
            },
        )
        .await
        .unwrap();
        repo.record_job_outputs(job_id, client_id, &outputs)
            .await
            .unwrap();
    }
    if let Repository::Memory(memory) = &repo {
        let mut outputs = memory.job_outputs.write().await;
        for (client_id, sha) in [
            ("client-a", "aa".repeat(32)),
            ("client-b", "aa".repeat(32)),
            ("client-c", "bb".repeat(32)),
        ] {
            outputs.push(JobOutputView {
                job_id,
                client_id: client_id.to_string(),
                seq: 1,
                stream: "stdout".to_string(),
                data_base64: String::new(),
                storage: "object_store".to_string(),
                artifact_object_key: Some(format!("job-outputs/{client_id}.bin")),
                artifact_sha256_hex: Some(sha),
                artifact_size_bytes: Some(100),
                exit_code: None,
                done: false,
                received_at: None,
                created_at: "2026-06-05T00:00:00Z".to_string(),
            });
        }
    }

    let comparison = repo.compare_job_outputs(job_id, "text").await.unwrap();

    assert_eq!(comparison.group_count, 2);
    assert_eq!(comparison.groups[0].target_count, 2);
    assert_eq!(comparison.groups[0].output_compare_basis, "binary_metadata");
    assert!(comparison.groups[0]
        .preview
        .contains("Artifact-backed retained output"));
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
        },
        session_id: Uuid::nil(),
    }
}

fn test_job_request(clients: &[&str]) -> CreateJobRequest {
    CreateJobRequest {
        job_id: None,
        selector_expression: test_selector_expression_for_clients(clients),
        target_client_ids: clients.iter().map(|client| (*client).to_string()).collect(),
        destructive: false,
        confirmed: false,
        command: "true".to_string(),
        argv: vec!["true".to_string()],
        operation: None,
        timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    }
}

fn operation_job_request(operation: JobCommand, clients: &[&str]) -> CreateJobRequest {
    CreateJobRequest {
        job_id: None,
        selector_expression: test_selector_expression_for_clients(clients),
        target_client_ids: clients.iter().map(|client| (*client).to_string()).collect(),
        destructive: true,
        confirmed: true,
        command: job_command_type_label(&operation).to_string(),
        argv: Vec::new(),
        operation: Some(operation),
        timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    }
}

fn exclusive_dispatch_operation_cases() -> Vec<(&'static str, JobCommand)> {
    vec![
        (
            "backup",
            JobCommand::Backup {
                paths: vec!["/etc/hostname".to_string()],
                include_config: true,
                recipient_public_key_hex: None,
            },
        ),
        (
            "shell",
            JobCommand::Shell {
                argv: vec!["/bin/true".to_string()],
                pty: false,
            },
        ),
        (
            "network",
            JobCommand::NetworkRollback {
                plan: Box::new(test_dispatch_tunnel_plan()),
                side: TunnelEndpointSide::Left,
            },
        ),
    ]
}

fn test_dispatch_tunnel_plan() -> vpsman_common::TunnelPlan {
    plan_tunnel(&TunnelPlanInput {
        name: "client-a-client-b".to_string(),
        interface_name: "tunab".to_string(),
        kind: TunnelKind::Gre,
        runtime_control: Default::default(),
        runtime_topology: Default::default(),
        left_client_id: "client-a".to_string(),
        right_client_id: "client-b".to_string(),
        left_underlay: "198.51.100.10".to_string(),
        right_underlay: "203.0.113.20".to_string(),
        address_pool_cidr: "10.255.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        bandwidth: BandwidthTier::M100,
        latency_ms: 18.0,
        packet_loss_ratio: 0.0,
        preference: 1.0,
        ospf_policy: OspfCostPolicy::default(),
    })
    .unwrap()
}

async fn record_memory_dispatch_job(
    repo: &Repository,
    operation: JobCommand,
    source_schedule_id: Option<Uuid>,
    command_type_override: Option<String>,
    fingerprint_suffix: &str,
) -> Uuid {
    let operator = test_operator();
    let request = operation_job_request(operation.clone(), &["client-a"]);
    let command_hash = payload_hash(&encode_json(&operation).unwrap());
    let request_fingerprint = format!("memory_dispatch_exclusive_{fingerprint_suffix}");
    let job_id = match source_schedule_id {
        Some(schedule_id) => repo
            .record_dispatching_job_from_schedule(
                Uuid::new_v4(),
                &request,
                &command_hash,
                &request_fingerprint,
                &operator,
                &["client-a".to_string()],
                schedule_id,
            )
            .await
            .unwrap(),
        None => repo
            .record_dispatching_job(
                Uuid::new_v4(),
                &request,
                &command_hash,
                &request_fingerprint,
                &operator,
                &["client-a".to_string()],
            )
            .await
            .unwrap(),
    };
    if let Some(command_type) = command_type_override {
        let Repository::Memory(memory) = repo else {
            unreachable!("test uses memory repository");
        };
        let mut jobs = memory.jobs.write().await;
        let job = jobs
            .iter_mut()
            .find(|job| job.id == job_id)
            .expect("recorded job must be visible");
        job.command_type = command_type;
    }
    job_id
}

async fn complete_memory_target(repo: &Repository, job_id: Uuid) {
    repo.update_job_target_result(
        job_id,
        "client-a",
        &TargetDispatchOutcome {
            status: "completed".to_string(),
            exit_code: Some(0),
            command_version: Some(1),
            accepted: true,
            message: "ok".to_string(),
            received_at: None,
            outputs: Vec::new(),
        },
    )
    .await
    .unwrap();
}

async fn record_test_output(
    repo: &Repository,
    job_id: Uuid,
    client_id: &str,
    status: &str,
    exit_code: Option<i32>,
    stream: OutputStream,
    data: &[u8],
) {
    let outputs = vec![
        CommandOutput {
            job_id,
            stream,
            data: data.to_vec(),
            exit_code: None,
            done: false,
        },
        CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: Vec::new(),
            exit_code,
            done: true,
        },
    ];
    repo.update_job_target_result(
        job_id,
        client_id,
        &TargetDispatchOutcome {
            status: status.to_string(),
            exit_code,
            command_version: Some(1),
            accepted: true,
            message: status.to_string(),
            received_at: None,
            outputs: outputs.clone(),
        },
    )
    .await
    .unwrap();
    repo.record_job_outputs(job_id, client_id, &outputs)
        .await
        .unwrap();
}
