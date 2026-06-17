use axum::{extract::State, Json};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use tokio::sync::broadcast;

use crate::{
    gateway_client::GatewayDispatchClient,
    job_request::validate_job_command,
    model::CreateJobRequest,
    repository::{MemoryState, Repository},
    repository_ingest::upsert_memory_agent,
    routes_jobs::create_job,
    state::AppState,
};
use ed25519_dalek::SigningKey;
use vpsman_common::{
    sign_update_artifact_hash, AgentCapabilitySnapshot, AgentConfig, AgentHello,
    AgentPrivilegeMode, JobCommand,
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

#[test]
fn validates_hot_config_job_document() {
    let config = AgentConfig {
        display_name: "edge-a".to_string(),
        tags: vec!["bgp".to_string()],
        ..AgentConfig::default()
    };
    let command = JobCommand::HotConfig {
        toml: toml::to_string_pretty(&config).unwrap(),
        preserve_redacted: None,
        base_config_sha256_hex: None,
    };

    validate_job_command(&command).unwrap();
}

#[test]
fn rejects_invalid_hot_config_job_document() {
    let command = JobCommand::HotConfig {
        toml: "client_id = ''".to_string(),
        preserve_redacted: None,
        base_config_sha256_hex: None,
    };

    assert!(validate_job_command(&command).is_err());
}

#[test]
fn validates_data_source_config_patch_job_document() {
    let command = JobCommand::DataSourceConfigPatch {
        toml: "[telemetry]\nproc_root = \"/tmp/vpsman-proc\"\n".to_string(),
    };

    validate_job_command(&command).unwrap();
}

#[test]
fn rejects_invalid_data_source_config_patch_job_document() {
    assert!(validate_job_command(&JobCommand::DataSourceConfigPatch {
        toml: String::new(),
    })
    .is_err());
    assert!(validate_job_command(&JobCommand::DataSourceConfigPatch {
        toml: "client_id = \"other\"".to_string(),
    })
    .is_err());
    assert!(validate_job_command(&JobCommand::DataSourceConfigPatch {
        toml: "[auth]\ncommand_timeout_secs = 10".to_string(),
    })
    .is_err());
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
    with_cleared_suite_env(&["VPSMAN_WORKER_SCHEDULE_COMMAND_TIMEOUT_SECS"], || {
        let path = temp_suite_config_path("schedule-apply-now-timeout");
        let mut state = test_state(Repository::Memory(MemoryState::default()));
        state.suite_config_path = path.clone();

        std::fs::write(
            &path,
            r#"version = 1

[worker]
schedule_command_timeout_secs = 600

[timeout]
worker_schedule_command_secs = 120
"#,
        )
        .unwrap();
        assert_eq!(state.schedule_apply_now_timeout_secs(), 600);

        std::fs::write(
            &path,
            r#"version = 1

[timeout]
worker_schedule_command_secs = 120
"#,
        )
        .unwrap();
        assert_eq!(state.schedule_apply_now_timeout_secs(), 120);

        std::env::set_var("VPSMAN_WORKER_SCHEDULE_COMMAND_TIMEOUT_SECS", "45");
        assert_eq!(state.schedule_apply_now_timeout_secs(), 45);

        let _ = std::fs::remove_file(path);
    });
}

#[test]
fn validates_agent_update_job_document() {
    let command = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "ab".repeat(32),
        artifact_signature_hex: None,
        artifact_signing_key_hex: None,
    };

    validate_job_command(&command).unwrap();

    let signing_key = SigningKey::from_bytes(&[31_u8; 32]);
    let sha256_hex = "cd".repeat(32);
    let command = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: sha256_hex.clone(),
        artifact_signature_hex: Some(hex::encode(sign_update_artifact_hash(
            &signing_key,
            &sha256_hex,
        ))),
        artifact_signing_key_hex: Some(hex::encode(signing_key.verifying_key().to_bytes())),
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
        artifact_signature_hex: None,
        artifact_signing_key_hex: None,
    })
    .is_err());
    assert!(validate_job_command(&JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "not-a-hash".to_string(),
        artifact_signature_hex: None,
        artifact_signing_key_hex: None,
    })
    .is_err());
    assert!(validate_job_command(&JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "ab".repeat(32),
        artifact_signature_hex: Some("00".repeat(64)),
        artifact_signing_key_hex: None,
    })
    .is_err());
    assert!(validate_job_command(&JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "ab".repeat(32),
        artifact_signature_hex: Some("00".repeat(64)),
        artifact_signing_key_hex: Some("11".repeat(32)),
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
        artifact_signature_hex: None,
        artifact_signing_key_hex: None,
    };
    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: true,
        command: "agent_update".to_string(),
        argv: Vec::new(),
        operation: Some(operation),
        timeout_secs: Some(60),
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
        update_object_store: None,
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
