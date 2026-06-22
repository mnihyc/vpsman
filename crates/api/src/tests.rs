use super::*;
use crate::model_terminal::TerminalSessionView;
use base64::Engine as _;
use vpsman_common::{
    job_command_type_label, AgentHello, CommandOutput, GatewayAgentHelloIngest,
    GatewayTerminalOutputIngest, JobCommand,
};
use vpsman_server_core::{TARGET_STATUS_AGENT_LOST, TARGET_STATUS_SKIPPED};

#[tokio::test]
async fn memory_namespaced_tags_participate_in_bulk_resolution() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
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
async fn memory_tag_order_controls_registry_and_agent_tag_reads() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
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

    for tag in ["provider:alpha", "role:edge", "country:US", "app:web"] {
        repo.assign_agent_tag("client-a", tag).await.unwrap();
    }

    assert_eq!(
        repo.list_tags()
            .await
            .unwrap()
            .into_iter()
            .map(|tag| tag.name)
            .collect::<Vec<_>>(),
        vec!["provider:alpha", "role:edge", "country:US", "app:web"]
    );

    repo.update_tag_order(&UpdateTagOrderRequest {
        ordered_tags: vec!["app:web".to_string(), "role:edge".to_string()],
    })
    .await
    .unwrap();

    assert_eq!(
        repo.list_tags()
            .await
            .unwrap()
            .into_iter()
            .map(|tag| tag.name)
            .collect::<Vec<_>>(),
        vec!["app:web", "role:edge", "provider:alpha", "country:US"]
    );
    assert_eq!(
        repo.agent_by_id("client-a").await.unwrap().tags,
        vec!["app:web", "role:edge", "provider:alpha", "country:US"]
    );
}

#[tokio::test]
async fn stale_agent_clears_only_after_changed_internal_build_hello() {
    fn hello(build: u64) -> GatewayAgentHelloIngest {
        GatewayAgentHelloIngest {
            gateway_id: "gateway-a".to_string(),
            gateway_session_id: uuid::Uuid::new_v4(),
            noise_public_key_hex: None,
            remote_ip: Some("203.0.113.10".to_string()),
            hello: AgentHello {
                client_id: "client-a".to_string(),
                process_incarnation_id: uuid::Uuid::new_v4(),
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
    let process_incarnation_id = uuid::Uuid::new_v4();
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "client-delete".to_string(),
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
        let job_id = Uuid::new_v4();
        memory.jobs.write().await.push(JobHistoryView {
            id: job_id,
            actor_id: Some(test_operator().operator.id),
            command_type: "shell".to_string(),
            privileged: true,
            status: "queued".to_string(),
            target_count: 1,
            payload_hash: "delete-test".to_string(),
            timeout_secs: 30,
            created_at: "1700000000".to_string(),
            completed_at: None,
        });
        memory.job_targets.write().await.push(JobTargetView {
            job_id,
            client_id: "client-delete".to_string(),
            status: "queued".to_string(),
            message: None,
            exit_code: None,
            started_at: None,
            deadline_at: None,
            completed_at: None,
            process_incarnation_id: None,
        });
        let running_job_id = Uuid::new_v4();
        memory.jobs.write().await.push(JobHistoryView {
            id: running_job_id,
            actor_id: Some(test_operator().operator.id),
            command_type: "shell".to_string(),
            privileged: true,
            status: "running".to_string(),
            target_count: 1,
            payload_hash: "delete-running-test".to_string(),
            timeout_secs: 30,
            created_at: "1700000000".to_string(),
            completed_at: None,
        });
        memory.job_targets.write().await.push(JobTargetView {
            job_id: running_job_id,
            client_id: "client-delete".to_string(),
            status: "running".to_string(),
            message: None,
            exit_code: None,
            started_at: Some("1700000001".to_string()),
            deadline_at: None,
            completed_at: None,
            process_incarnation_id: Some(process_incarnation_id),
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
                privilege_assertion: None,
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
                privilege_assertion: None,
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
        let targets = memory.job_targets.read().await;
        let target = targets
            .iter()
            .find(|target| {
                target.client_id == "client-delete" && target.status == TARGET_STATUS_SKIPPED
            })
            .unwrap();
        assert_eq!(target.status, "skipped");
        assert_eq!(
            target.message.as_deref(),
            Some("vps_deleted: target skipped before dispatch")
        );
        assert!(target.completed_at.is_some());
        let agent_lost_target = targets
            .iter()
            .find(|target| {
                target.client_id == "client-delete" && target.status == TARGET_STATUS_AGENT_LOST
            })
            .unwrap();
        assert_eq!(
            agent_lost_target.message.as_deref(),
            Some("client was deleted before final command output")
        );
        assert!(agent_lost_target.completed_at.is_some());
        drop(targets);
        let outputs = memory.job_outputs.read().await;
        let output_payloads = outputs
            .iter()
            .filter(|output| output.client_id == "client-delete")
            .map(|output| {
                serde_json::from_slice::<serde_json::Value>(
                    &base64::engine::general_purpose::STANDARD
                        .decode(&output.data_base64)
                        .unwrap(),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        assert!(output_payloads.iter().any(|payload| {
            payload["code"] == "vps_deleted" && payload["status"] == TARGET_STATUS_SKIPPED
        }));
        assert!(output_payloads.iter().any(|payload| {
            payload["code"] == "vps_deleted" && payload["status"] == TARGET_STATUS_AGENT_LOST
        }));
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
                privilege_assertion: None,
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
async fn create_job_requires_client_supplied_job_id() {
    let repo = Repository::Memory(MemoryState::default());
    seed_never_connected_memory_agent(&repo, "client-a").await;
    let state = test_app_state(repo);
    let operator = test_operator();
    let request = route_job_request(None, "uptime");

    let error = routes_jobs::create_job_with_operator(&state, &operator, request)
        .await
        .unwrap_err();

    assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(error.code, "job_id_required");
    assert!(state.repo.list_jobs(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn create_job_rejects_public_terminal_input_operation() {
    let repo = Repository::Memory(MemoryState::default());
    seed_never_connected_memory_agent(&repo, "client-a").await;
    let state = test_app_state(repo);
    let operator = test_operator();
    let mut request = route_job_request(Some(Uuid::new_v4()), "terminal_input");
    request.destructive = true;
    request.confirmed = true;
    request.operation = Some(JobCommand::TerminalInput {
        session_id: Uuid::new_v4(),
        input_seq: 1,
        data_base64: vpsman_common::encode_inline_file_payload(b"id\n").unwrap(),
    });

    let error = routes_jobs::create_job_with_operator(&state, &operator, request)
        .await
        .unwrap_err();

    assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "terminal_input_route_required");
    assert!(state.repo.list_jobs(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn create_job_retry_with_same_job_id_returns_existing_job() {
    let repo = Repository::Memory(MemoryState::default());
    seed_never_connected_memory_agent(&repo, "client-a").await;
    let state = test_app_state(repo);
    let operator = test_operator();
    let job_id = Uuid::new_v4();

    let (first_status, axum::Json(first)) = routes_jobs::create_job_with_operator(
        &state,
        &operator,
        route_job_request(Some(job_id), "uptime"),
    )
    .await
    .unwrap();
    let (retry_status, axum::Json(retry)) = routes_jobs::create_job_with_operator(
        &state,
        &operator,
        route_job_request(Some(job_id), "uptime"),
    )
    .await
    .unwrap();

    assert_eq!(first_status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(retry_status, axum::http::StatusCode::OK);
    assert_eq!(first.job_id, job_id);
    assert_eq!(retry.job_id, job_id);
    assert_eq!(retry.target_count, first.target_count);
    assert_eq!(retry.target_counts.total, 1);
    assert_eq!(retry.target_counts.skipped, 1);
    assert_eq!(state.repo.list_jobs(10).await.unwrap().len(), 1);
    let targets = state.repo.list_job_targets(job_id).await.unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].status, TARGET_STATUS_SKIPPED);
    assert!(targets[0].completed_at.is_some());
    let outputs = state.repo.list_job_outputs(job_id).await.unwrap();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].client_id, "client-a");
    assert_eq!(outputs[0].seq, 0);
    assert_eq!(outputs[0].stream, "status");
    assert!(outputs[0].done);
    assert!(state
        .repo
        .claim_due_job_targets(10, 30, 0)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn create_job_rejects_same_job_id_for_different_request() {
    let repo = Repository::Memory(MemoryState::default());
    seed_never_connected_memory_agent(&repo, "client-a").await;
    let state = test_app_state(repo);
    let operator = test_operator();
    let job_id = Uuid::new_v4();

    let _ = routes_jobs::create_job_with_operator(
        &state,
        &operator,
        route_job_request(Some(job_id), "uptime"),
    )
    .await
    .unwrap();
    let error = routes_jobs::create_job_with_operator(
        &state,
        &operator,
        route_job_request(Some(job_id), "hostname"),
    )
    .await
    .unwrap_err();

    assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(error.code, "job_id_reused_with_different_request");
    assert_eq!(state.repo.list_jobs(10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn rejected_job_records_frozen_target_results() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
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
    let operator = AuthContext {
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
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
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
            follow_symlinks: false,
        }),
        timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };

    assert_eq!(request.command_type_label(), "file_pull");
    match request.job_command().unwrap() {
        JobCommand::FilePull {
            path,
            follow_symlinks,
        } => {
            assert_eq!(path, "/etc/hostname");
            assert!(!follow_symlinks);
        }
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
            follow_symlinks: false,
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
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
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
async fn memory_final_output_insert_terminalizes_target_atomically() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let request = test_job_request(&["client-a"]);
    let command = request.job_command().unwrap();
    let command_hash = payload_hash(&encode_json(&command).unwrap());
    let job_id = repo
        .record_dispatching_job(
            Uuid::new_v4(),
            &request,
            &command_hash,
            "final_output_atomic",
            &operator,
            &["client-a".to_string()],
        )
        .await
        .unwrap();
    assert_eq!(
        repo.claim_due_job_targets(10, 30, 0).await.unwrap().len(),
        1
    );

    let output = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: br#"{"type":"completed"}"#.to_vec(),
        exit_code: Some(0),
        done: true,
    };
    let outcome = TargetDispatchOutcome {
        status: "completed".to_string(),
        exit_code: Some(0),
        command_version: Some(1),
        accepted: true,
        message: "ok".to_string(),
        received_at: None,
        outputs: vec![output.clone()],
    };
    let result = repo
        .record_active_final_job_output_and_target_result_with_config(
            job_id,
            "client-a",
            0,
            &output,
            Some("1700000000".to_string()),
            repository_job_outputs::JobOutputPersistConfig {
                object_store: None,
                artifact_min_bytes: usize::MAX,
            },
            &outcome,
        )
        .await
        .unwrap();

    assert_eq!(
        result.write_result,
        repository_job_outputs::JobOutputWriteResult::Inserted
    );
    assert!(result.target_terminalized);
    let job = repo.get_job(job_id).await.unwrap().unwrap();
    let targets = repo.list_job_targets(job_id).await.unwrap();
    let outputs = repo.list_job_outputs(job_id).await.unwrap();
    assert_eq!(job.status, "completed");
    assert!(job.completed_at.is_some());
    assert_eq!(targets[0].status, "completed");
    assert_eq!(targets[0].exit_code, Some(0));
    assert!(targets[0].completed_at.is_some());
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].stream, "status");
    assert_eq!(outputs[0].exit_code, Some(0));
    assert!(outputs[0].done);
}

#[tokio::test]
async fn memory_final_output_waits_for_lower_sequences_before_terminalizing() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let request = test_job_request(&["client-a"]);
    let command = request.job_command().unwrap();
    let command_hash = payload_hash(&encode_json(&command).unwrap());
    let job_id = repo
        .record_dispatching_job(
            Uuid::new_v4(),
            &request,
            &command_hash,
            "final_output_contiguous",
            &operator,
            &["client-a".to_string()],
        )
        .await
        .unwrap();
    assert_eq!(
        repo.claim_due_job_targets(10, 30, 0).await.unwrap().len(),
        1
    );
    let persist = repository_job_outputs::JobOutputPersistConfig {
        object_store: None,
        artifact_min_bytes: usize::MAX,
    };
    let final_output = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: br#"{"type":"completed"}"#.to_vec(),
        exit_code: Some(0),
        done: true,
    };
    let outcome = TargetDispatchOutcome {
        status: "completed".to_string(),
        exit_code: Some(0),
        command_version: Some(1),
        accepted: true,
        message: "ok".to_string(),
        received_at: None,
        outputs: vec![final_output.clone()],
    };

    let early_final = repo
        .record_active_final_job_output_and_target_result_with_config(
            job_id,
            "client-a",
            1,
            &final_output,
            Some("1700000000".to_string()),
            persist,
            &outcome,
        )
        .await
        .unwrap();
    assert_eq!(
        early_final.write_result,
        repository_job_outputs::JobOutputWriteResult::Inserted
    );
    assert!(!early_final.target_terminalized);
    assert_eq!(
        repo.list_job_targets(job_id).await.unwrap()[0].status,
        "dispatching"
    );

    let stdout = CommandOutput {
        job_id,
        stream: OutputStream::Stdout,
        data: b"ready\n".to_vec(),
        exit_code: None,
        done: false,
    };
    repo.record_active_job_output_chunk_checked_with_config(
        job_id,
        "client-a",
        0,
        &stdout,
        Some("1700000001".to_string()),
        persist,
    )
    .await
    .unwrap();
    let candidate = repo
        .contiguous_final_job_output_candidate(job_id, "client-a")
        .await
        .unwrap()
        .expect("stored final should become contiguous");
    assert_eq!(candidate.seq, 1);

    let finalized = repo
        .record_active_final_job_output_and_target_result_with_config(
            job_id,
            "client-a",
            candidate.seq,
            &candidate.output,
            candidate.received_at,
            persist,
            &outcome,
        )
        .await
        .unwrap();
    assert_eq!(
        finalized.write_result,
        repository_job_outputs::JobOutputWriteResult::DuplicateIdentical
    );
    assert!(finalized.target_terminalized);
    assert_eq!(
        repo.list_job_targets(job_id).await.unwrap()[0].status,
        "completed"
    );
}

#[tokio::test]
async fn memory_dispatch_claims_one_exclusive_target_per_client_per_batch() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let command = JobCommand::AgentUpdateCheck {
        version_url: None,
        activate: false,
        restart_agent: false,
    };
    let request = operation_job_request(command.clone(), &["client-a"]);
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

    let first_claim = repo.claim_due_job_targets(10, 30, 0).await.unwrap();
    assert_eq!(first_claim.len(), 1);
    assert_eq!(first_claim[0].job_id, first_job_id);
    assert_eq!(first_claim[0].client_id, "client-a");
    assert!(repo
        .claim_due_job_targets(10, 30, 0)
        .await
        .unwrap()
        .is_empty());

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

    let second_claim = repo.claim_due_job_targets(10, 30, 0).await.unwrap();
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

    let claim = repo.claim_due_job_targets(10, 30, 0).await.unwrap();
    assert_eq!(claim.len(), 1);
    assert_eq!(claim[0].job_id, job_id);
    assert_eq!(claim[0].source_schedule_id, Some(schedule_id));
}

#[tokio::test]
async fn memory_dispatch_claim_promotes_parent_job_to_running() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let request = test_job_request(&["client-a"]);
    let command = request.job_command().unwrap();
    let command_hash = payload_hash(&encode_json(&command).unwrap());
    let job_id = repo
        .record_dispatching_job(
            Uuid::new_v4(),
            &request,
            &command_hash,
            "promote_parent_running",
            &operator,
            &["client-a".to_string()],
        )
        .await
        .unwrap();

    assert_eq!(
        repo.get_job(job_id).await.unwrap().unwrap().status,
        "queued"
    );
    let claim = repo.claim_due_job_targets(10, 30, 0).await.unwrap();
    assert_eq!(claim.len(), 1);

    let job = repo.get_job(job_id).await.unwrap().unwrap();
    let targets = repo.list_job_targets(job_id).await.unwrap();
    assert_eq!(job.status, "running");
    assert_eq!(targets[0].status, "dispatching");
}

#[tokio::test]
async fn late_final_output_does_not_rewrite_control_timeout_target() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let request = test_job_request(&["client-a"]);
    let command = request.job_command().unwrap();
    let command_hash = payload_hash(&encode_json(&command).unwrap());
    let job_id = repo
        .record_dispatching_job(
            Uuid::new_v4(),
            &request,
            &command_hash,
            "control_timeout_terminal",
            &operator,
            &["client-a".to_string()],
        )
        .await
        .unwrap();
    assert_eq!(
        repo.claim_due_job_targets(10, 30, 0).await.unwrap().len(),
        1
    );
    let Repository::Memory(memory) = &repo else {
        unreachable!();
    };
    {
        let mut targets = memory.job_targets.write().await;
        let target = targets
            .iter_mut()
            .find(|target| target.job_id == job_id && target.client_id == "client-a")
            .unwrap();
        target.started_at = Some("0".to_string());
    }
    let expired = repo.expire_control_timeout_targets(10, 0).await.unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(
        repo.refresh_job_status_from_targets(job_id).await.unwrap(),
        Some("control_timeout".to_string())
    );

    repo.update_job_target_result(
        job_id,
        "client-a",
        &TargetDispatchOutcome {
            status: "completed".to_string(),
            exit_code: Some(0),
            command_version: Some(1),
            accepted: true,
            message: "late success".to_string(),
            received_at: None,
            outputs: Vec::new(),
        },
    )
    .await
    .unwrap();
    let targets = repo.list_job_targets(job_id).await.unwrap();
    let job = repo.get_job(job_id).await.unwrap().unwrap();
    assert_eq!(targets[0].status, "control_timeout");
    assert_eq!(targets[0].exit_code, None);
    assert_eq!(job.status, "control_timeout");
}

#[tokio::test]
async fn memory_terminal_input_control_timeout_releases_reservation() {
    let repo = Repository::Memory(MemoryState::default());
    let session_id = Uuid::new_v4();
    let job_id =
        record_memory_terminal_input_dispatch_job(&repo, "client-a", session_id, b"a\n").await;
    assert_eq!(
        repo.claim_due_job_targets(10, 30, 0).await.unwrap().len(),
        1
    );
    let Repository::Memory(memory) = &repo else {
        unreachable!();
    };
    {
        let mut targets = memory.job_targets.write().await;
        let target = targets
            .iter_mut()
            .find(|target| target.job_id == job_id && target.client_id == "client-a")
            .unwrap();
        target.started_at = Some("0".to_string());
    }

    let expired = repo.expire_control_timeout_targets(10, 0).await.unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(
        terminal_input_request_status(&repo, job_id).await,
        Some(("control_timeout".to_string(), true))
    );

    let next_payload = b"b\n";
    let next = repo
        .reserve_terminal_input_request(
            "client-a",
            session_id,
            Uuid::new_v4(),
            &payload_hash(next_payload),
            next_payload.len() as i64,
        )
        .await
        .unwrap();
    assert_eq!(next.input_seq, 2);
}

#[tokio::test]
async fn memory_terminal_input_agent_lost_releases_reservation() {
    let repo = Repository::Memory(MemoryState::default());
    let session_id = Uuid::new_v4();
    let job_id =
        record_memory_terminal_input_dispatch_job(&repo, "client-a", session_id, b"a\n").await;
    assert_eq!(
        repo.claim_due_job_targets(10, 30, 0).await.unwrap().len(),
        1
    );

    let status = repo
        .record_agent_lost_target(job_id, "client-a", "agent lost", None, None)
        .await
        .unwrap();
    assert_eq!(status, Some("failed".to_string()));
    assert_eq!(
        terminal_input_request_status(&repo, job_id).await,
        Some(("agent_lost".to_string(), true))
    );
}

#[tokio::test]
async fn memory_terminal_input_queued_cancel_releases_reservation() {
    let repo = Repository::Memory(MemoryState::default());
    let session_id = Uuid::new_v4();
    let job_id =
        record_memory_terminal_input_dispatch_job(&repo, "client-a", session_id, b"a\n").await;

    let plan = repo
        .request_job_cancel(job_id, test_operator().operator.id, Some("operator"))
        .await
        .unwrap();
    assert_eq!(plan.pending_canceled, 1);
    assert!(plan.cancel_targets.is_empty());
    assert_eq!(
        terminal_input_request_status(&repo, job_id).await,
        Some(("canceled".to_string(), true))
    );
    let outputs = repo.list_job_outputs(job_id).await.unwrap();
    let cancel_output = outputs
        .iter()
        .find(|output| output.client_id == "client-a" && output.done)
        .expect("queued cancel writes final output");
    let cancel_payload: serde_json::Value = serde_json::from_slice(
        &base64::engine::general_purpose::STANDARD
            .decode(&cancel_output.data_base64)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(cancel_payload["type"], "command_canceled");

    let next_payload = b"b\n";
    let next = repo
        .reserve_terminal_input_request(
            "client-a",
            session_id,
            Uuid::new_v4(),
            &payload_hash(next_payload),
            next_payload.len() as i64,
        )
        .await
        .unwrap();
    assert_eq!(next.input_seq, 2);
}

#[tokio::test]
async fn spooled_command_output_accepts_seen_inactive_gateway_session() {
    let repo = Repository::Memory(MemoryState::default());
    let mut state = test_app_state(repo.clone());
    state.internal_token = Some("test-token".to_string());
    let operator = test_operator();
    let client_id = "client-a";
    let gateway_id = "gateway-a";
    let gateway_session_id = Uuid::new_v4();
    let process_incarnation_id = Uuid::new_v4();
    let lifecycle = vpsman_common::GatewaySessionLifecycleIngest {
        gateway_id: gateway_id.to_string(),
        client_id: client_id.to_string(),
        session_id: gateway_session_id,
        noise_public_key_hex: None,
        remote_ip: None,
        reason: None,
    };
    repo.record_gateway_session_started(&lifecycle)
        .await
        .unwrap();
    repo.record_gateway_session_ended(&vpsman_common::GatewaySessionLifecycleIngest {
        reason: Some("rotated".to_string()),
        ..lifecycle.clone()
    })
    .await
    .unwrap();

    let request = CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: format!("id:{client_id}"),
        target_client_ids: vec![client_id.to_string()],
        destructive: true,
        confirmed: true,
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
            request.job_id.unwrap(),
            &request,
            &command_hash,
            "spooled_replay_test",
            &operator,
            &[client_id.to_string()],
        )
        .await
        .unwrap();
    if let Repository::Memory(memory) = &repo {
        let mut targets = memory.job_targets.write().await;
        let target = targets
            .iter_mut()
            .find(|target| target.job_id == job_id && target.client_id == client_id)
            .unwrap();
        target.status = "running".to_string();
        target.started_at = Some(unix_now().to_string());
        target.process_incarnation_id = Some(process_incarnation_id);
    }
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        axum::http::HeaderValue::from_static("Bearer test-token"),
    );
    let event = vpsman_common::GatewayCommandOutputIngest {
        gateway_id: gateway_id.to_string(),
        gateway_session_id,
        process_incarnation_id,
        spooled_replay: false,
        client_id: client_id.to_string(),
        job_id,
        payload_hash: command_hash,
        seq: 0,
        received_unix: Some(unix_now()),
        output: CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: br#"{"type":"completed"}"#.to_vec(),
            exit_code: Some(0),
            done: true,
        },
    };
    let live_error = crate::routes_ingest::ingest_command_output(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::Json(event.clone()),
    )
    .await
    .unwrap_err();
    assert_eq!(live_error.code, "gateway_session_not_active");

    let _accepted = crate::routes_ingest::ingest_command_output(
        axum::extract::State(state),
        headers,
        axum::Json(vpsman_common::GatewayCommandOutputIngest {
            spooled_replay: true,
            ..event
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        repo.list_job_targets(job_id).await.unwrap()[0].status,
        "completed"
    );
}

#[tokio::test]
async fn memory_terminal_input_final_output_preserves_precise_status() {
    let repo = Repository::Memory(MemoryState::default());
    let session_id = Uuid::new_v4();
    let job_id =
        record_memory_terminal_input_dispatch_job(&repo, "client-a", session_id, b"a\n").await;
    assert_eq!(
        repo.claim_due_job_targets(10, 30, 0).await.unwrap().len(),
        1
    );
    let output = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "terminal_input",
            "status": "accepted",
            "session_id": session_id,
            "input_seq": 1,
            "written_bytes": 2
        }))
        .unwrap(),
        exit_code: Some(0),
        done: true,
    };
    let outcome = TargetDispatchOutcome {
        status: "completed".to_string(),
        exit_code: Some(0),
        command_version: Some(1),
        accepted: true,
        message: "ok".to_string(),
        received_at: None,
        outputs: vec![output.clone()],
    };

    let result = repo
        .record_active_final_job_output_and_target_result_with_config(
            job_id,
            "client-a",
            0,
            &output,
            Some("1700000000".to_string()),
            repository_job_outputs::JobOutputPersistConfig {
                object_store: None,
                artifact_min_bytes: usize::MAX,
            },
            &outcome,
        )
        .await
        .unwrap();

    assert!(result.target_terminalized);
    assert_eq!(
        terminal_input_request_status(&repo, job_id).await,
        Some(("accepted".to_string(), true))
    );
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

        let first_claim = repo.claim_due_job_targets(10, 30, 0).await.unwrap();
        assert_eq!(first_claim.len(), 1, "{case}: scheduled claim");
        assert_eq!(first_claim[0].job_id, scheduled_job_id, "{case}");
        assert_eq!(first_claim[0].command_type, scheduled_label, "{case}");
        assert!(
            repo.claim_due_job_targets(10, 30, 0)
                .await
                .unwrap()
                .is_empty(),
            "{case}: direct job must wait behind scheduled exclusive operation"
        );

        complete_memory_target(&repo, scheduled_job_id).await;
        let second_claim = repo.claim_due_job_targets(10, 30, 0).await.unwrap();
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

        let first_claim = repo.claim_due_job_targets(10, 30, 0).await.unwrap();
        assert_eq!(first_claim.len(), 1, "{case}: direct claim");
        assert_eq!(first_claim[0].job_id, direct_job_id, "{case}");
        assert_eq!(
            first_claim[0].command_type,
            job_command_type_label(&operation),
            "{case}"
        );
        assert!(
            repo.claim_due_job_targets(10, 30, 0)
                .await
                .unwrap()
                .is_empty(),
            "{case}: scheduled job must wait behind direct exclusive operation"
        );

        complete_memory_target(&repo, direct_job_id).await;
        let second_claim = repo.claim_due_job_targets(10, 30, 0).await.unwrap();
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

#[tokio::test]
async fn terminal_stream_ingest_is_idempotent_and_does_not_append_job_outputs() {
    let repo = Repository::Memory(MemoryState::default());
    let job_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    seed_terminal_memory_job(&repo, job_id, "client-a", "completed").await;
    let mut state = test_app_state(repo.clone());
    state.internal_token = Some("internal-token".to_string());
    let headers = internal_headers("internal-token");

    let _ = routes_ingest::ingest_terminal_output(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::Json(terminal_stream_ingest(
            job_id,
            session_id,
            Some(1),
            vpsman_common::OutputStream::Pty,
            b"one".to_vec(),
            false,
        )),
    )
    .await
    .unwrap();
    let _ = routes_ingest::ingest_terminal_output(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::Json(terminal_stream_ingest(
            job_id,
            session_id,
            Some(1),
            vpsman_common::OutputStream::Pty,
            b"one".to_vec(),
            false,
        )),
    )
    .await
    .unwrap();

    let Repository::Memory(memory) = &repo else {
        unreachable!();
    };
    assert_eq!(memory.terminal_output_chunks.read().await.len(), 1);
    assert!(memory.job_outputs.read().await.is_empty());
    let replay = repo
        .terminal_session_replay("client-a", session_id, None, 10, 1000, true)
        .await
        .unwrap();
    assert_eq!(replay.source, "terminal_output_chunks");
    assert_eq!(replay.chunk_count, 1);
    assert_eq!(replay.chunks[0].data_base64.as_deref(), Some("b25l"));

    let mut conflicting_event = terminal_stream_ingest(
        job_id,
        session_id,
        Some(1),
        vpsman_common::OutputStream::Pty,
        b"two".to_vec(),
        false,
    );
    conflicting_event.output.output_retained_first_seq = Some(2);
    conflicting_event.output.output_retained_bytes = 0;
    let conflict = routes_ingest::ingest_terminal_output(
        axum::extract::State(state),
        headers,
        axum::Json(conflicting_event),
    )
    .await
    .unwrap_err();
    assert_eq!(conflict.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(memory.terminal_output_chunks.read().await.len(), 1);
    let replay = repo
        .terminal_session_replay("client-a", session_id, Some(1), 10, 1000, true)
        .await
        .unwrap();
    assert_eq!(replay.available_first_seq, Some(1));
    assert_eq!(replay.chunk_count, 1);
    assert_eq!(replay.chunks[0].data_base64.as_deref(), Some("b25l"));
}

#[tokio::test]
async fn terminal_final_stream_status_updates_session_without_job_output_append() {
    let repo = Repository::Memory(MemoryState::default());
    let job_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    seed_terminal_memory_job(&repo, job_id, "client-a", "completed").await;
    let mut state = test_app_state(repo.clone());
    state.internal_token = Some("internal-token".to_string());
    let headers = internal_headers("internal-token");

    let _ = routes_ingest::ingest_terminal_output(
        axum::extract::State(state),
        headers,
        axum::Json(terminal_stream_ingest(
            job_id,
            session_id,
            None,
            vpsman_common::OutputStream::Status,
            serde_json::to_vec(&serde_json::json!({
                "type": "terminal_stream",
                "status": "exited",
                "session_id": session_id,
                "output_first_seq": 1,
                "output_next_seq": 2,
                "output_retained_first_seq": 1,
                "output_retained_bytes": 3,
                "output_dropped_bytes": 0,
                "output_dropped_chunks": 0,
                "output_replay_truncated": false,
                "session_exited": true
            }))
            .unwrap(),
            true,
        )),
    )
    .await
    .unwrap();

    let sessions = repo
        .list_terminal_sessions(10, Some("client-a"), Some(session_id))
        .await
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].state, "exited");
    assert_eq!(sessions[0].last_status, "exited");
    let Repository::Memory(memory) = &repo else {
        unreachable!();
    };
    assert!(memory.job_outputs.read().await.is_empty());
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

fn test_app_state(repo: Repository) -> AppState {
    let (events, _) = tokio::sync::broadcast::channel(1);
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

async fn record_memory_terminal_input_dispatch_job(
    repo: &Repository,
    client_id: &str,
    session_id: Uuid,
    input: &[u8],
) -> Uuid {
    seed_open_terminal_session(repo, client_id, session_id).await;
    let job_id = Uuid::new_v4();
    let reservation = repo
        .reserve_terminal_input_request(
            client_id,
            session_id,
            job_id,
            &payload_hash(input),
            input.len() as i64,
        )
        .await
        .unwrap();
    let request = CreateJobRequest {
        job_id: Some(job_id),
        selector_expression: format!("id:{client_id}"),
        target_client_ids: vec![client_id.to_string()],
        destructive: true,
        confirmed: true,
        command: "terminal_input".to_string(),
        argv: Vec::new(),
        operation: Some(JobCommand::TerminalInput {
            session_id,
            input_seq: u64::try_from(reservation.input_seq).unwrap(),
            data_base64: base64::engine::general_purpose::STANDARD.encode(input),
        }),
        timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };
    let command = request.job_command().unwrap();
    let command_hash = payload_hash(&encode_json(&command).unwrap());
    repo.record_dispatching_job(
        job_id,
        &request,
        &command_hash,
        "terminal_input_test",
        &test_operator(),
        &[client_id.to_string()],
    )
    .await
    .unwrap();
    repo.mark_terminal_input_request_status(job_id, "queued")
        .await
        .unwrap();
    job_id
}

async fn seed_open_terminal_session(repo: &Repository, client_id: &str, session_id: Uuid) {
    let Repository::Memory(memory) = repo else {
        panic!("seed_open_terminal_session supports only memory repository tests");
    };
    memory
        .terminal_sessions
        .write()
        .await
        .push(TerminalSessionView {
            session_id,
            client_id: client_id.to_string(),
            state: "open".to_string(),
            last_status: "accepted".to_string(),
            argv: vec!["/bin/sh".to_string(), "-l".to_string()],
            cwd: Some("/root".to_string()),
            cols: Some(120),
            rows: Some(40),
            idle_timeout_secs: Some(3600),
            flow_window_bytes: Some(65_536),
            output_first_seq: Some(1),
            output_next_seq: Some(1),
            output_retained_first_seq: Some(1),
            output_retained_bytes: Some(0),
            output_dropped_bytes: Some(0),
            output_dropped_chunks: Some(0),
            output_replay_truncated: false,
            last_input_seq: None,
            session_exited: false,
            close_reason: None,
            last_event: "open".to_string(),
            last_job_id: Uuid::new_v4(),
            last_command_type: "terminal_open".to_string(),
            last_seq: 0,
            observed_at: "2026-06-21T00:00:00Z".to_string(),
        });
}

async fn terminal_input_request_status(repo: &Repository, job_id: Uuid) -> Option<(String, bool)> {
    let Repository::Memory(memory) = repo else {
        panic!("terminal_input_request_status supports only memory repository tests");
    };
    memory
        .terminal_input_requests
        .read()
        .await
        .iter()
        .find(|request| request.job_id == job_id)
        .map(|request| (request.status.clone(), request.completed_at.is_some()))
}

fn internal_headers(token: &str) -> axum::http::HeaderMap {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    headers
}

async fn seed_terminal_memory_job(repo: &Repository, job_id: Uuid, client_id: &str, status: &str) {
    let Repository::Memory(memory) = repo else {
        panic!("seed_terminal_memory_job supports only memory repository tests");
    };
    upsert_memory_agent(
        &memory.agents,
        &AgentHello {
            client_id: client_id.to_string(),
            process_incarnation_id: terminal_gateway_process_incarnation_id(),
            agent_version: "test".to_string(),
            internal_build_number: 1,
            os_release: "test".to_string(),
            arch: "x86_64".to_string(),
            update_heartbeat: None,
            capabilities: Default::default(),
        },
    )
    .await;
    memory
        .gateway_sessions
        .write()
        .await
        .push(GatewaySessionView {
            id: terminal_gateway_session_id(),
            gateway_id: "gateway-a".to_string(),
            client_id: client_id.to_string(),
            noise_public_key_hex: None,
            status: "active".to_string(),
            started_at: "2026-06-20T00:00:00Z".to_string(),
            last_seen_at: "2026-06-20T00:00:00Z".to_string(),
            ended_at: None,
            end_reason: None,
        });
    memory.jobs.write().await.push(JobHistoryView {
        id: job_id,
        actor_id: Some(test_operator().operator.id),
        command_type: "terminal_open".to_string(),
        privileged: true,
        status: status.to_string(),
        target_count: 1,
        payload_hash: "terminal-test".to_string(),
        timeout_secs: 30,
        created_at: "2026-06-20T00:00:00Z".to_string(),
        completed_at: Some("2026-06-20T00:00:01Z".to_string()),
    });
    memory.job_targets.write().await.push(JobTargetView {
        job_id,
        client_id: client_id.to_string(),
        status: status.to_string(),
        message: None,
        exit_code: Some(0),
        started_at: Some("2026-06-20T00:00:00Z".to_string()),
        deadline_at: None,
        completed_at: Some("2026-06-20T00:00:01Z".to_string()),
        process_incarnation_id: Some(terminal_gateway_process_incarnation_id()),
    });
}

fn terminal_gateway_session_id() -> Uuid {
    Uuid::from_u128(0x11111111111111111111111111111111)
}

fn terminal_gateway_process_incarnation_id() -> Uuid {
    Uuid::from_u128(0x22222222222222222222222222222222)
}

fn terminal_stream_ingest(
    job_id: Uuid,
    session_id: Uuid,
    terminal_seq: Option<u64>,
    stream: vpsman_common::OutputStream,
    data: Vec<u8>,
    done: bool,
) -> GatewayTerminalOutputIngest {
    GatewayTerminalOutputIngest {
        gateway_id: "gateway-a".to_string(),
        gateway_session_id: terminal_gateway_session_id(),
        process_incarnation_id: terminal_gateway_process_incarnation_id(),
        spooled_replay: false,
        client_id: "client-a".to_string(),
        output: vpsman_common::TerminalStreamOutput {
            job_id,
            session_id,
            terminal_seq,
            output_first_seq: Some(1),
            output_next_seq: terminal_seq.unwrap_or(1).saturating_add(1),
            output_retained_first_seq: Some(1),
            output_retained_bytes: data.len() as u64,
            output_dropped_bytes: 0,
            output_dropped_chunks: 0,
            output_replay_truncated: false,
            output: CommandOutput {
                job_id,
                stream,
                data,
                exit_code: done.then_some(0),
                done,
            },
        },
    }
}

async fn seed_never_connected_memory_agent(repo: &Repository, client_id: &str) {
    let Repository::Memory(memory) = repo else {
        panic!("seed_never_connected_memory_agent supports only memory repository tests");
    };
    memory.agents.write().await.push(AgentView {
        id: client_id.to_string(),
        display_name: client_id.to_string(),
        status: "never".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: vpsman_common::AgentCapabilitySnapshot::default(),
    });
}

fn route_job_request(job_id: Option<Uuid>, command: &str) -> CreateJobRequest {
    CreateJobRequest {
        job_id,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: true,
        command: command.to_string(),
        argv: Vec::new(),
        operation: None,
        timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
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
            "agent_update",
            JobCommand::UpdateAgent {
                artifact_url: "https://updates.example/agent".to_string(),
                sha256_hex: "a".repeat(64),
            },
        ),
        (
            "agent_update_activate",
            JobCommand::AgentUpdateActivate {
                staged_sha256_hex: "b".repeat(64),
                restart_agent: false,
            },
        ),
        (
            "agent_update_check",
            JobCommand::AgentUpdateCheck {
                version_url: None,
                activate: false,
                restart_agent: false,
            },
        ),
        (
            "agent_update_rollback",
            JobCommand::AgentUpdateRollback {
                rollback_sha256_hex: None,
            },
        ),
    ]
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
            unreachable!("test uses the unit-test repository fixture");
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
