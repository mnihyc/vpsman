use axum::{extract::State, Json};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use tokio::sync::broadcast;

use crate::{
    gateway_client::GatewayDispatchClient,
    job_request::validate_job_command,
    model::{
        AuthContext, CreateJobRequest, OperatorPreferences, OperatorView,
        RenderRuntimeConfigPatchGeneratorRequest, UpsertRuntimeConfigPatchGeneratorRequest,
    },
    repository::{MemoryState, Repository},
    repository_ingest::upsert_memory_agent,
    routes_jobs::create_job,
    runtime_config::{
        compose_runtime_config, push_runtime_config_for_clients,
        request_runtime_config_reload_for_agent,
    },
    state::AppState,
};
use uuid::Uuid;
use vpsman_common::{
    runtime_config_content_hash, AgentCapabilitySnapshot, AgentHello, AgentPrivilegeMode,
    AgentRuntimeConfig, AgentUpdateConfig, JobCommand, MAX_RUNTIME_CONFIG_FIELD_BYTES,
    MAX_RUNTIME_CONFIG_REASON_BYTES,
};

async fn wait_for_job_status(
    repo: &crate::repository::Repository,
    job_id: uuid::Uuid,
    expected: &str,
) {
    for _ in 0..50 {
        let jobs = repo.list_jobs(100).await.unwrap();
        if jobs
            .iter()
            .any(|job| job.id == job_id && job.status == expected)
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("job {job_id} did not reach status {expected}");
}

fn memory_admin() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "test-operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: OperatorPreferences::default(),
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

#[test]
fn autonomous_updater_defaults_disabled_with_official_manifest_defaults() {
    let update = AgentUpdateConfig::default();

    assert!(!update.unmanaged_enabled);
    assert_eq!(
        update.unmanaged_version_url,
        "https://github.com/mnihyc/vpsman/releases/latest/download/version.json"
    );
    assert_eq!(update.unmanaged_interval_secs, 86_400);
    assert_eq!(update.unmanaged_jitter_secs, 86_400);
    assert!(update.unmanaged_activate);
    assert!(update.unmanaged_restart_agent);
}

#[tokio::test]
async fn agent_requested_runtime_config_reload_compares_hash_before_queuing() {
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
                capabilities: AgentCapabilitySnapshot::default(),
            },
        )
        .await;
    }
    let state = test_state(repo.clone());
    let desired = compose_runtime_config(&state, "client-a", 7).await.unwrap();
    let current_hash = runtime_config_content_hash(&desired).unwrap();

    let no_op = request_runtime_config_reload_for_agent(
        &state,
        "client-a",
        &current_hash,
        "agent_reconnect_runtime_config_check",
    )
    .await
    .unwrap();
    assert!(no_op.is_empty());
    assert!(repo.list_jobs(10).await.unwrap().is_empty());

    repo.upsert_runtime_config_overrides(
        &["client-a".to_string()],
        "telemetry_light_secs = 30\n",
        "operator runtime config update",
        &memory_admin(),
    )
    .await
    .unwrap();
    let queued = request_runtime_config_reload_for_agent(
        &state,
        "client-a",
        &current_hash,
        "agent_reconnect_runtime_config_check",
    )
    .await
    .unwrap();

    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].target_count, 1);
    let jobs = repo.list_jobs(10).await.unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].actor_id, None);
    assert_eq!(jobs[0].target_count, 1);
}

#[tokio::test]
async fn runtime_config_apply_state_promotes_only_completed_sync_target() {
    let repo = Repository::Memory(MemoryState::default());
    let job_id = Uuid::new_v4();
    let config = AgentRuntimeConfig {
        version: 12,
        display_name: "server-managed".to_string(),
        ..AgentRuntimeConfig::default()
    };
    let hash = runtime_config_content_hash(&config).unwrap();
    repo.queue_runtime_config_apply(
        "client-a",
        config.version,
        &hash,
        &config,
        job_id,
        "test-apply",
    )
    .await
    .unwrap();
    if let Repository::Memory(memory) = &repo {
        memory.job_operations.write().await.insert(
            job_id,
            JobCommand::RuntimeConfigSync {
                desired_version: config.version,
                reason: "test-apply".to_string(),
                config: Box::new(config.clone()),
            },
        );
    }

    repo.record_runtime_config_apply_terminal_for_target_status(
        job_id,
        "client-a",
        vpsman_server_core::TARGET_STATUS_COMPLETED,
        Some("applied"),
    )
    .await
    .unwrap();

    let states = repo
        .list_runtime_config_apply_states(Some("client-a"))
        .await
        .unwrap();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].applied_version, Some(12));
    assert_eq!(
        states[0].applied_content_hash.as_deref(),
        Some(hash.as_str())
    );
    assert_eq!(states[0].applied_job_id, Some(job_id));
    assert_eq!(states[0].pending_status, None);
    assert_eq!(states[0].pending_job_id, None);
    let applied = repo
        .runtime_config_applied_state_for_client("client-a")
        .await
        .unwrap()
        .expect("applied config should be readable");
    assert_eq!(applied.0, 12);
    assert_eq!(applied.1, hash);
    assert_eq!(applied.2.display_name, "server-managed");
}

#[tokio::test]
async fn runtime_config_apply_state_keeps_failed_sync_pending_failed() {
    let repo = Repository::Memory(MemoryState::default());
    let job_id = Uuid::new_v4();
    let config = AgentRuntimeConfig {
        version: 13,
        display_name: "not-applied".to_string(),
        ..AgentRuntimeConfig::default()
    };
    let hash = runtime_config_content_hash(&config).unwrap();
    repo.queue_runtime_config_apply(
        "client-a",
        config.version,
        &hash,
        &config,
        job_id,
        "test-fail",
    )
    .await
    .unwrap();
    if let Repository::Memory(memory) = &repo {
        memory.job_operations.write().await.insert(
            job_id,
            JobCommand::RuntimeConfigSync {
                desired_version: config.version,
                reason: "test-fail".to_string(),
                config: Box::new(config),
            },
        );
    }

    repo.record_runtime_config_apply_terminal_for_target_status(
        job_id,
        "client-a",
        vpsman_server_core::TARGET_STATUS_FAILED,
        Some("runtime tunnel mutation failed"),
    )
    .await
    .unwrap();

    let states = repo
        .list_runtime_config_apply_states(Some("client-a"))
        .await
        .unwrap();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].applied_version, None);
    assert_eq!(states[0].pending_version, Some(13));
    assert_eq!(states[0].pending_status.as_deref(), Some("failed"));
    assert_eq!(
        states[0].pending_error.as_deref(),
        Some("runtime tunnel mutation failed")
    );
    assert!(repo
        .runtime_config_applied_state_for_client("client-a")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn runtime_config_push_creates_pending_apply_state() {
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
                capabilities: AgentCapabilitySnapshot::default(),
            },
        )
        .await;
    }
    let state = test_state(repo.clone());

    let responses = push_runtime_config_for_clients(
        &state,
        &memory_admin(),
        ["client-a".to_string()],
        "operator runtime config update",
    )
    .await
    .unwrap();

    assert_eq!(responses.len(), 1);
    let states = repo
        .list_runtime_config_apply_states(Some("client-a"))
        .await
        .unwrap();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].applied_version, None);
    assert!(states[0].pending_version.is_some());
    assert!(states[0].pending_content_hash.is_some());
    assert_eq!(states[0].pending_job_id, Some(responses[0].job_id));
    assert_eq!(states[0].pending_status.as_deref(), Some("queued"));
    assert_eq!(
        states[0].pending_reason.as_deref(),
        Some("operator runtime config update")
    );
    assert!(repo
        .runtime_config_applied_state_for_client("client-a")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn runtime_config_apply_state_is_per_client_for_partial_and_skipped_syncs() {
    let repo = Repository::Memory(MemoryState::default());
    let previous_job_id = Uuid::new_v4();
    let previous_config = AgentRuntimeConfig {
        version: 19,
        display_name: "previously-applied".to_string(),
        ..AgentRuntimeConfig::default()
    };
    let previous_hash = runtime_config_content_hash(&previous_config).unwrap();
    repo.queue_runtime_config_apply(
        "client-b",
        previous_config.version,
        &previous_hash,
        &previous_config,
        previous_job_id,
        "previous successful rollout",
    )
    .await
    .unwrap();
    if let Repository::Memory(memory) = &repo {
        memory.job_operations.write().await.insert(
            previous_job_id,
            JobCommand::RuntimeConfigSync {
                desired_version: previous_config.version,
                reason: "previous successful rollout".to_string(),
                config: Box::new(previous_config.clone()),
            },
        );
    }
    repo.record_runtime_config_apply_terminal_for_target_status(
        previous_job_id,
        "client-b",
        vpsman_server_core::TARGET_STATUS_COMPLETED,
        Some("applied"),
    )
    .await
    .unwrap();

    let rollout_job_id = Uuid::new_v4();
    let config_a = AgentRuntimeConfig {
        version: 20,
        display_name: "applied-a".to_string(),
        ..AgentRuntimeConfig::default()
    };
    let config_b = AgentRuntimeConfig {
        version: 20,
        display_name: "pending-b".to_string(),
        ..AgentRuntimeConfig::default()
    };
    let hash_a = runtime_config_content_hash(&config_a).unwrap();
    let hash_b = runtime_config_content_hash(&config_b).unwrap();
    repo.queue_runtime_config_apply(
        "client-a",
        config_a.version,
        &hash_a,
        &config_a,
        rollout_job_id,
        "partial fleet rollout",
    )
    .await
    .unwrap();
    repo.queue_runtime_config_apply(
        "client-b",
        config_b.version,
        &hash_b,
        &config_b,
        rollout_job_id,
        "partial fleet rollout",
    )
    .await
    .unwrap();
    if let Repository::Memory(memory) = &repo {
        memory.job_operations.write().await.insert(
            rollout_job_id,
            JobCommand::RuntimeConfigSync {
                desired_version: 20,
                reason: "partial fleet rollout".to_string(),
                config: Box::new(config_a.clone()),
            },
        );
    }

    repo.record_runtime_config_apply_terminal_for_target_status(
        rollout_job_id,
        "client-a",
        vpsman_server_core::TARGET_STATUS_COMPLETED,
        Some("applied"),
    )
    .await
    .unwrap();
    repo.record_runtime_config_apply_terminal_for_target_status(
        rollout_job_id,
        "client-b",
        vpsman_server_core::TARGET_STATUS_SKIPPED,
        Some("target_agent_lacks_root_runtime_network_capability"),
    )
    .await
    .unwrap();

    let state_a = repo
        .list_runtime_config_apply_states(Some("client-a"))
        .await
        .unwrap()
        .pop()
        .expect("client-a state");
    assert_eq!(state_a.applied_version, Some(20));
    assert_eq!(
        state_a.applied_content_hash.as_deref(),
        Some(hash_a.as_str())
    );
    assert_eq!(state_a.pending_status, None);

    let state_b = repo
        .list_runtime_config_apply_states(Some("client-b"))
        .await
        .unwrap()
        .pop()
        .expect("client-b state");
    assert_eq!(state_b.applied_version, Some(19));
    assert_eq!(
        state_b.applied_content_hash.as_deref(),
        Some(previous_hash.as_str())
    );
    assert_eq!(state_b.pending_version, Some(20));
    assert_eq!(
        state_b.pending_content_hash.as_deref(),
        Some(hash_b.as_str())
    );
    assert_eq!(state_b.pending_status.as_deref(), Some("failed"));
    assert_eq!(
        state_b.pending_error.as_deref(),
        Some("target_agent_lacks_root_runtime_network_capability")
    );

    let applied_b = repo
        .runtime_config_applied_state_for_client("client-b")
        .await
        .unwrap()
        .expect("client-b applied state should remain");
    assert_eq!(applied_b.0, 19);
    assert_eq!(applied_b.1, previous_hash);
    assert_eq!(applied_b.2.display_name, "previously-applied");
}

#[test]
fn runtime_config_sync_reason_uses_four_kibibyte_limit() {
    let max_reason = "x".repeat(MAX_RUNTIME_CONFIG_REASON_BYTES);
    validate_job_command(&JobCommand::RuntimeConfigSync {
        desired_version: 1,
        reason: max_reason,
        config: Box::new(AgentRuntimeConfig {
            version: 1,
            ..AgentRuntimeConfig::default()
        }),
    })
    .unwrap();

    let oversized_reason = "x".repeat(MAX_RUNTIME_CONFIG_REASON_BYTES + 1);
    assert!(validate_job_command(&JobCommand::RuntimeConfigSync {
        desired_version: 1,
        reason: oversized_reason,
        config: Box::new(AgentRuntimeConfig {
            version: 1,
            ..AgentRuntimeConfig::default()
        }),
    })
    .is_err());
}

#[tokio::test]
async fn runtime_config_patch_generators_keep_built_ins_immutable_and_custom_generators_editable() {
    let repo = Repository::Memory(MemoryState::default());
    let generators = repo.list_runtime_config_patch_generators().await.unwrap();
    let enable = generators
        .iter()
        .find(|generator| generator.name == "Autonomous updater enabled")
        .expect("missing autonomous updater enable generator");
    let disable = generators
        .iter()
        .find(|generator| generator.name == "Autonomous updater disabled")
        .expect("missing autonomous updater disable generator");
    assert!(enable.built_in);
    assert!(disable.built_in);

    let rendered = repo
        .render_runtime_config_patch_generator(
            enable.id,
            &RenderRuntimeConfigPatchGeneratorRequest {
                values: serde_json::json!({}),
            },
        )
        .await
        .unwrap();
    assert!(rendered.toml.contains("unmanaged_enabled = true"));
    assert!(rendered
        .toml
        .contains("https://github.com/mnihyc/vpsman/releases/latest/download/version.json"));

    let operator = memory_admin();
    let built_in_edit = repo
        .upsert_runtime_config_patch_generator(
            &UpsertRuntimeConfigPatchGeneratorRequest {
                id: Some(enable.id),
                name: "Autonomous updater enabled edited".to_string(),
                category: "update".to_string(),
                domain: "agent_update".to_string(),
                description: "operator-edited predefined updater generator".to_string(),
                field_schema: enable.field_schema.clone(),
                raw_generator_body: enable.raw_generator_body.clone(),
                docs_metadata: enable.docs_metadata.clone(),
                confirmed: true,
            },
            &operator,
        )
        .await;
    assert!(built_in_edit
        .unwrap_err()
        .to_string()
        .contains("runtime_config_patch_generator_builtin_immutable"));

    let custom = repo
        .upsert_runtime_config_patch_generator(
            &UpsertRuntimeConfigPatchGeneratorRequest {
                id: None,
                name: "Custom updater enabled".to_string(),
                category: "update".to_string(),
                domain: "agent_update".to_string(),
                description: "operator-managed updater generator".to_string(),
                field_schema: enable.field_schema.clone(),
                raw_generator_body: enable.raw_generator_body.clone(),
                docs_metadata: enable.docs_metadata.clone(),
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(custom.name, "Custom updater enabled");
    assert!(!custom.built_in);

    let edited = repo
        .upsert_runtime_config_patch_generator(
            &UpsertRuntimeConfigPatchGeneratorRequest {
                id: Some(custom.id),
                name: "Custom updater enabled edited".to_string(),
                category: "update".to_string(),
                domain: "agent_update".to_string(),
                description: "operator-edited custom updater generator".to_string(),
                field_schema: custom.field_schema.clone(),
                raw_generator_body: custom.raw_generator_body.clone(),
                docs_metadata: custom.docs_metadata.clone(),
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(edited.name, "Custom updater enabled edited");
    assert!(!edited.built_in);

    let built_in_delete = repo
        .delete_runtime_config_patch_generator(disable.id, &operator)
        .await;
    assert!(built_in_delete
        .unwrap_err()
        .to_string()
        .contains("runtime_config_patch_generator_builtin_immutable"));

    repo.delete_runtime_config_patch_generator(custom.id, &operator)
        .await
        .unwrap();
    let after_delete = repo.list_runtime_config_patch_generators().await.unwrap();
    assert!(!after_delete
        .iter()
        .any(|generator| generator.id == custom.id));
    assert!(after_delete
        .iter()
        .any(|generator| generator.id == disable.id));
}

#[tokio::test]
async fn runtime_config_patch_generator_fields_use_four_kibibyte_limit() {
    let state = test_state(Repository::Memory(MemoryState::default()));
    let headers = crate::test_auth_headers(&state).await;
    let max_name = "x".repeat(MAX_RUNTIME_CONFIG_FIELD_BYTES);

    let Json(saved) = crate::routes_inventory::upsert_runtime_config_patch_generator(
        State(state.clone()),
        headers.clone(),
        Json(UpsertRuntimeConfigPatchGeneratorRequest {
            id: None,
            name: max_name.clone(),
            category: "network".to_string(),
            domain: "runtime".to_string(),
            description: "operator-managed generator".to_string(),
            field_schema: serde_json::json!({}),
            raw_generator_body: "[telemetry]\nfull_secs = 300\n".to_string(),
            docs_metadata: serde_json::json!({}),
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    assert_eq!(saved.name, max_name);

    let error = crate::routes_inventory::upsert_runtime_config_patch_generator(
        State(state),
        headers,
        Json(UpsertRuntimeConfigPatchGeneratorRequest {
            id: None,
            name: "x".repeat(MAX_RUNTIME_CONFIG_FIELD_BYTES + 1),
            category: "network".to_string(),
            domain: "runtime".to_string(),
            description: "operator-managed generator".to_string(),
            field_schema: serde_json::json!({}),
            raw_generator_body: "[telemetry]\nfull_secs = 300\n".to_string(),
            docs_metadata: serde_json::json!({}),
            confirmed: true,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "runtime_config_patch_generator_invalid");
}

#[test]
fn app_state_reloads_suite_config_hot_fields_from_file() {
    with_cleared_suite_env(API_HOT_RELOAD_ENV, || {
        let path = temp_suite_config_path("api-hot-reload");
        std::fs::write(
            &path,
            suite_runtime_toml(SuiteRuntimeToml {
                batch: 17,
                in_flight: 9,
                dispatch_ack_secs: 11,
                event_post_secs: 12,
                internal_http_read_secs: 13,
                control_deadline_grace_secs: 14,
                artifact_min_bytes: 4096,
                artifact_max_bytes: 96 * 1024 * 1024,
                require_registered_agent_updates: true,
                memory_warning: 0.30,
                memory_critical: 0.20,
                cpu_warning: 3.0,
                cpu_critical: 5.0,
            }),
        )
        .unwrap();
        let mut state = test_state(Repository::Memory(MemoryState::default()));
        state.suite_config_path = path.clone();

        let dispatcher = state.dispatcher_runtime_config();
        assert_eq!(dispatcher.batch_limit, 17);
        assert_eq!(dispatcher.in_flight, 9);
        assert_eq!(dispatcher.dispatch_ack_secs, 11);
        assert_eq!(dispatcher.event_post_secs, 12);
        assert_eq!(dispatcher.internal_http_read_secs, 13);
        assert_eq!(dispatcher.control_deadline_grace_secs, 14);
        assert_eq!(dispatcher.control_deadline_extra_secs(), 39);
        assert_eq!(state.job_output_artifact_min_bytes(), 4096);
        assert_eq!(state.artifact_max_bytes(), 96 * 1024 * 1024);
        assert!(state.require_registered_agent_updates());
        let policy = state.fleet_alert_policy();
        assert_eq!(policy.memory_available_warning_ratio, 0.30);
        assert_eq!(policy.memory_available_critical_ratio, 0.20);
        assert_eq!(policy.cpu_load_warning, 3.0);
        assert_eq!(policy.cpu_load_critical, 5.0);
        state.refresh_gateway_dispatch_timeouts();
        assert_eq!(state.gateway.test_timeouts().read.as_secs(), 13);

        std::fs::write(
            &path,
            suite_runtime_toml(SuiteRuntimeToml {
                batch: 23,
                in_flight: 7,
                dispatch_ack_secs: 29,
                event_post_secs: 8,
                internal_http_read_secs: 19,
                control_deadline_grace_secs: 17,
                artifact_min_bytes: 8192,
                artifact_max_bytes: 160 * 1024 * 1024,
                require_registered_agent_updates: false,
                memory_warning: 0.40,
                memory_critical: 0.15,
                cpu_warning: 4.0,
                cpu_critical: 6.0,
            }),
        )
        .unwrap();

        let dispatcher = state.dispatcher_runtime_config();
        assert_eq!(dispatcher.batch_limit, 23);
        assert_eq!(dispatcher.in_flight, 7);
        assert_eq!(dispatcher.dispatch_ack_secs, 29);
        assert_eq!(dispatcher.event_post_secs, 8);
        assert_eq!(dispatcher.internal_http_read_secs, 19);
        assert_eq!(dispatcher.control_deadline_grace_secs, 17);
        assert_eq!(dispatcher.control_deadline_extra_secs(), 54);
        assert_eq!(state.job_output_artifact_min_bytes(), 8192);
        assert_eq!(state.artifact_max_bytes(), 160 * 1024 * 1024);
        assert!(!state.require_registered_agent_updates());
        let policy = state.fleet_alert_policy();
        assert_eq!(policy.memory_available_warning_ratio, 0.40);
        assert_eq!(policy.memory_available_critical_ratio, 0.15);
        assert_eq!(policy.cpu_load_warning, 4.0);
        assert_eq!(policy.cpu_load_critical, 6.0);
        state.refresh_gateway_dispatch_timeouts();
        assert_eq!(state.gateway.test_timeouts().read.as_secs(), 29);

        let _ = std::fs::remove_file(path);
    });
}

#[test]
fn apply_now_schedule_timeout_matches_worker_suite_precedence() {
    with_cleared_suite_env(&["VPSMAN_WORKER_SCHEDULE_JOB_MAX_TIMEOUT_SECS"], || {
        let path = temp_suite_config_path("schedule-apply-now-timeout");
        let mut state = test_state(Repository::Memory(MemoryState::default()));
        state.suite_config_path = path.clone();

        std::fs::write(
            &path,
            r#"version = 1

[worker]
schedule_job_max_timeout_secs = 600

[timeout]
worker_schedule_job_max_timeout_secs = 120
"#,
        )
        .unwrap();
        assert_eq!(state.schedule_apply_now_max_timeout_secs(), 600);

        std::fs::write(
            &path,
            r#"version = 1

[timeout]
worker_schedule_job_max_timeout_secs = 120
"#,
        )
        .unwrap();
        assert_eq!(state.schedule_apply_now_max_timeout_secs(), 120);

        std::env::set_var("VPSMAN_WORKER_SCHEDULE_JOB_MAX_TIMEOUT_SECS", "45");
        assert_eq!(state.schedule_apply_now_max_timeout_secs(), 45);

        let _ = std::fs::remove_file(path);
    });
}

#[test]
fn validates_agent_update_job_document() {
    let command = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "ab".repeat(32),
    };
    validate_job_command(&command).unwrap();

    validate_job_command(&JobCommand::AgentUpdateActivate {
        staged_sha256_hex: "ef".repeat(32),
        restart_agent: false,
    })
    .unwrap();
    validate_job_command(&JobCommand::AgentUpdateRollback {
        rollback_sha256_hex: Some("01".repeat(32)),
    })
    .unwrap();
    validate_job_command(&JobCommand::AgentUpdateRollback {
        rollback_sha256_hex: None,
    })
    .unwrap();
}

#[test]
fn rejects_invalid_agent_update_job_document() {
    assert!(validate_job_command(&JobCommand::UpdateAgent {
        artifact_url: "http://updates.example/vpsman-agent".to_string(),
        sha256_hex: "ab".repeat(32),
    })
    .is_err());
    assert!(validate_job_command(&JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "not-a-hash".to_string(),
    })
    .is_err());
    assert!(validate_job_command(&JobCommand::AgentUpdateActivate {
        staged_sha256_hex: "not-a-hash".to_string(),
        restart_agent: false,
    })
    .is_err());
    assert!(validate_job_command(&JobCommand::AgentUpdateRollback {
        rollback_sha256_hex: Some("not-a-hash".to_string()),
    })
    .is_err());
}

#[tokio::test]
async fn agent_update_degrades_unprivileged_target_after_privilege_verification() {
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
                capabilities: AgentCapabilitySnapshot {
                    privilege_mode: AgentPrivilegeMode::Unprivileged,
                    effective_uid: Some(1000),
                    can_attempt_privileged_ops: false,
                    unprivileged_hint: Some("running as normal user".to_string()),
                    ..Default::default()
                },
            },
        )
        .await;
    }
    let operation = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "ab".repeat(32),
    };
    let request = CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: true,
        command: "agent_update".to_string(),
        argv: Vec::new(),
        operation: Some(operation),
        max_timeout_secs: Some(60),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };

    let state = test_state_with_privilege_auto_approve(repo.clone());
    let headers = crate::test_auth_headers(&state).await;
    let (status, Json(response)) = create_job(State(state), headers, Json(request))
        .await
        .unwrap();
    wait_for_job_status(&repo, response.job_id, "skipped").await;
    let targets = repo.list_job_targets(response.job_id).await.unwrap();
    let outputs = repo.list_job_outputs(response.job_id).await.unwrap();
    let output_bytes = BASE64_STANDARD.decode(&outputs[0].data_base64).unwrap();
    let status_output: serde_json::Value = serde_json::from_slice(&output_bytes).unwrap();

    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert_eq!(response.status, "skipped");
    assert_eq!(targets[0].status, "skipped");
    assert_eq!(
        status_output["reason"],
        "target_agent_lacks_agent_update_capability"
    );
}

fn test_state(repo: Repository) -> AppState {
    let (events, _) = broadcast::channel(1);
    AppState {
        repo,
        events,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
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

fn test_state_with_privilege_auto_approve(repo: Repository) -> AppState {
    AppState {
        gateway: GatewayDispatchClient::test_privilege_auto_approve(),
        ..test_state(repo)
    }
}

const API_HOT_RELOAD_ENV: &[&str] = &[
    "VPSMAN_DISPATCHER_BATCH",
    "VPSMAN_DISPATCHER_IN_FLIGHT",
    "VPSMAN_DISPATCH_ACK_SECS",
    "VPSMAN_EVENT_POST_SECS",
    "VPSMAN_INTERNAL_HTTP_READ_SECS",
    "VPSMAN_JOB_OUTPUT_ARTIFACT_MIN_BYTES",
    "VPSMAN_ARTIFACT_MAX_BYTES",
    "VPSMAN_REQUIRE_REGISTERED_AGENT_UPDATES",
    "VPSMAN_ALERT_MEMORY_AVAILABLE_WARNING_RATIO",
    "VPSMAN_ALERT_MEMORY_AVAILABLE_CRITICAL_RATIO",
    "VPSMAN_ALERT_DISK_AVAILABLE_WARNING_RATIO",
    "VPSMAN_ALERT_DISK_AVAILABLE_CRITICAL_RATIO",
    "VPSMAN_ALERT_CPU_LOAD_WARNING",
    "VPSMAN_ALERT_CPU_LOAD_CRITICAL",
];

static SUITE_CONFIG_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn with_cleared_suite_env<R>(names: &[&str], run: impl FnOnce() -> R) -> R {
    let _guard = SUITE_CONFIG_ENV_LOCK.lock().unwrap();
    let saved = names
        .iter()
        .map(|name| (*name, std::env::var_os(name)))
        .collect::<Vec<_>>();
    for name in names {
        std::env::remove_var(name);
    }
    let result = run();
    for (name, value) in saved {
        if let Some(value) = value {
            std::env::set_var(name, value);
        } else {
            std::env::remove_var(name);
        }
    }
    result
}

fn temp_suite_config_path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("vpsman-{label}-{}.toml", uuid::Uuid::new_v4()))
}

struct SuiteRuntimeToml {
    batch: i64,
    in_flight: usize,
    dispatch_ack_secs: u64,
    event_post_secs: u64,
    internal_http_read_secs: u64,
    control_deadline_grace_secs: u64,
    artifact_min_bytes: usize,
    artifact_max_bytes: usize,
    require_registered_agent_updates: bool,
    memory_warning: f64,
    memory_critical: f64,
    cpu_warning: f64,
    cpu_critical: f64,
}

fn suite_runtime_toml(input: SuiteRuntimeToml) -> String {
    let SuiteRuntimeToml {
        batch,
        in_flight,
        dispatch_ack_secs,
        event_post_secs,
        internal_http_read_secs,
        control_deadline_grace_secs,
        artifact_min_bytes,
        artifact_max_bytes,
        require_registered_agent_updates,
        memory_warning,
        memory_critical,
        cpu_warning,
        cpu_critical,
    } = input;
    format!(
        r#"version = 1

[capacity]
dispatcher_batch = {batch}
dispatcher_in_flight = {in_flight}

[timeout]
dispatch_ack_secs = {dispatch_ack_secs}
event_post_secs = {event_post_secs}
internal_http_read_secs = {internal_http_read_secs}
control_deadline_grace_secs = {control_deadline_grace_secs}

[api]
job_output_artifact_min_bytes = {artifact_min_bytes}
artifact_max_bytes = {artifact_max_bytes}
require_registered_agent_updates = {require_registered_agent_updates}
alert_memory_available_warning_ratio = {memory_warning}
alert_memory_available_critical_ratio = {memory_critical}
alert_disk_available_warning_ratio = {memory_warning}
alert_disk_available_critical_ratio = {memory_critical}
alert_cpu_load_warning = {cpu_warning}
alert_cpu_load_critical = {cpu_critical}
"#
    )
}
