use std::collections::BTreeMap;

use uuid::Uuid;
use vpsman_common::{
    HostPackageProvider, HostServiceAction, HostServiceProvider, JobCommand, ProcessResourceLimits,
    ProcessRestartPolicy, ProcessRunPolicy,
};

use crate::model::CreateJobRequest;

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
        max_timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
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
        max_timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
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
        max_timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
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
        max_timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
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

#[test]
fn host_service_commands_reject_ambiguous_or_unbounded_payloads() {
    let mut request = CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::ServiceInventory {
            expected_provider: None,
            limit: 0,
        }),
        max_timeout_secs: Some(30),
        force_unprivileged: false,
        privileged: false,
        privilege_assertion: None,
        rollout: None,
    };
    assert_eq!(
        request.job_command().unwrap_err().code,
        "service_inventory_limit_out_of_range"
    );

    request.operation = Some(JobCommand::ServiceLogs {
        provider: HostServiceProvider::Systemd,
        service: "sshd".to_string(),
        max_lines: 200,
    });
    assert_eq!(
        request.job_command().unwrap_err().code,
        "systemd_service_unit_suffix_required"
    );

    request.operation = Some(JobCommand::ServiceLogs {
        provider: HostServiceProvider::Sysv,
        service: "../sshd".to_string(),
        max_lines: 200,
    });
    assert_eq!(
        request.job_command().unwrap_err().code,
        "host_service_name_invalid"
    );

    request.operation = Some(JobCommand::ServiceLogs {
        provider: HostServiceProvider::Systemd,
        service: "sshd.service".to_string(),
        max_lines: 0,
    });
    assert_eq!(
        request.job_command().unwrap_err().code,
        "service_logs_line_limit_out_of_range"
    );

    request.operation = Some(JobCommand::ServiceAction {
        provider: HostServiceProvider::Systemd,
        service: "sshd.service".to_string(),
        action: HostServiceAction::Restart,
        expected_active_state: "active running".to_string(),
        expected_enabled_state: "enabled".to_string(),
    });
    assert_eq!(
        request.job_command().unwrap_err().code,
        "host_service_expected_state_invalid"
    );
}

#[test]
fn package_update_commands_validate_exact_plan_hashes() {
    let mut request = CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::PackageUpdatePlan {
            expected_provider: Some(HostPackageProvider::Apt),
            refresh_metadata: false,
        }),
        max_timeout_secs: Some(30),
        force_unprivileged: false,
        privileged: false,
        privilege_assertion: None,
        rollout: None,
    };
    assert_eq!(request.command_type_label(), "package_update_plan");
    request.job_command().unwrap();

    request.operation = Some(JobCommand::PackageUpdateApply {
        provider: HostPackageProvider::Apt,
        plan_hash: "not-a-hash".to_string(),
    });
    assert_eq!(
        request.job_command().unwrap_err().code,
        "package_update_plan_hash_invalid"
    );

    request.operation = Some(JobCommand::PackageUpdateApply {
        provider: HostPackageProvider::Apt,
        plan_hash: "a".repeat(64),
    });
    request.job_command().unwrap();
}
