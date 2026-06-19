use super::*;
use std::collections::BTreeMap;

use axum::{extract::State, Json};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use tokio::sync::broadcast;
use vpsman_common::{
    AgentCapabilitySnapshot, AgentHello, AgentPrivilegeMode, JobCommand, ProcessResourceLimits,
    ProcessRestartPolicy, ProcessRunPolicy,
};

use crate::{gateway_client::GatewayDispatchClient, routes_jobs::create_job};

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
fn process_supervisor_job_commands_validate_operation_payloads() {
    let request = CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::ProcessStart {
            name: "demo".to_string(),
            argv: vec!["/bin/sleep".to_string(), "60".to_string()],
            cwd: Some("/tmp".to_string()),
            env: BTreeMap::from([("VPSMAN_TEST".to_string(), "1".to_string())]),
            policy: ProcessRunPolicy::default(),
            limits: ProcessResourceLimits::default(),
        }),
        timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };

    assert_eq!(request.command_type_label(), "process_start");
    match request.job_command().unwrap() {
        JobCommand::ProcessStart {
            name,
            argv,
            cwd,
            env,
            policy,
            limits,
        } => {
            assert_eq!(name, "demo");
            assert_eq!(argv, vec!["/bin/sleep", "60"]);
            assert_eq!(cwd.as_deref(), Some("/tmp"));
            assert_eq!(env.get("VPSMAN_TEST").map(String::as_str), Some("1"));
            assert_eq!(policy, ProcessRunPolicy::default());
            assert_eq!(limits, ProcessResourceLimits::default());
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn process_supervisor_job_commands_accept_policy_and_limits() {
    let request = CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::ProcessStart {
            name: "limited-worker".to_string(),
            argv: vec!["/bin/sleep".to_string(), "60".to_string()],
            cwd: None,
            env: BTreeMap::new(),
            policy: ProcessRunPolicy {
                restart: ProcessRestartPolicy::OnFailure,
                restart_max_retries: 3,
                restart_backoff_secs: 10,
                graceful_stop_secs: 15,
            },
            limits: ProcessResourceLimits {
                memory_max_bytes: Some(128 * 1024 * 1024),
                pids_max: Some(32),
                open_files_max: Some(256),
                cpu_shares: Some(1024),
                no_new_privileges: true,
            },
        }),
        timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };

    request.job_command().unwrap();
}

#[test]
fn process_supervisor_job_commands_reject_unbounded_limits() {
    let request = CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::ProcessStart {
            name: "limited-worker".to_string(),
            argv: vec!["/bin/sleep".to_string(), "60".to_string()],
            cwd: None,
            env: BTreeMap::new(),
            policy: ProcessRunPolicy::default(),
            limits: ProcessResourceLimits {
                memory_max_bytes: Some(1),
                ..ProcessResourceLimits::default()
            },
        }),
        timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };

    let error = request.job_command().unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "process_memory_limit_out_of_range");
}

#[test]
fn process_supervisor_job_commands_reject_bad_payloads() {
    let mut request = CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::ProcessStart {
            name: "../bad".to_string(),
            argv: vec!["sleep".to_string()],
            cwd: None,
            env: BTreeMap::new(),
            policy: ProcessRunPolicy::default(),
            limits: ProcessResourceLimits::default(),
        }),
        timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };

    assert_eq!(
        request.job_command().unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );

    request.operation = Some(JobCommand::ProcessLogs {
        name: "demo".to_string(),
        max_bytes: 0,
    });
    assert_eq!(
        request.job_command().unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn process_start_with_limits_degrades_unprivileged_target_after_privilege_verification() {
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
                    command_timeout_secs: 3600,
                    can_attempt_privileged_ops: true,
                    can_manage_runtime_tunnels: false,
                    can_apply_process_limits: false,
                    unprivileged_hint: Some("running as normal user".to_string()),
                },
            },
        )
        .await;
    }
    let operation = JobCommand::ProcessStart {
        name: "limited-worker".to_string(),
        argv: vec!["/bin/sleep".to_string(), "60".to_string()],
        cwd: None,
        env: BTreeMap::new(),
        policy: ProcessRunPolicy::default(),
        limits: ProcessResourceLimits {
            memory_max_bytes: Some(128 * 1024 * 1024),
            pids_max: Some(32),
            ..ProcessResourceLimits::default()
        },
    };
    let request = CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: true,
        command: "process_start".to_string(),
        argv: Vec::new(),
        operation: Some(operation),
        timeout_secs: Some(30),
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
        "target_agent_lacks_process_limit_capability"
    );
    assert!(status_output["hint"]
        .as_str()
        .unwrap()
        .contains("force_unprivileged"));
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
