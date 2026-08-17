use super::*;
use std::collections::BTreeMap;

use axum::{
    extract::{Path, Query, State},
    Json,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use vpsman_common::{
    AgentCapabilitySnapshot, AgentHello, AgentPrivilegeMode, HostPackageCapability,
    HostPackageCapabilityStatus, HostPackageProvider, HostPackageUpdatePlanSnapshot,
    HostPackageUpdateRecord, HostProcessSnapshot, HostProcessView, HostServiceAction,
    HostServiceCapability, HostServiceCapabilityStatus, HostServiceProvider, HostServiceRecord,
    HostServiceSnapshot, HostStorageCapability, HostStorageCapabilityStatus, HostStorageProvider,
    HostStorageSnapshot, JobCommand, ProcessResourceLimits, ProcessRestartPolicy, ProcessRunPolicy,
};

use crate::{
    gateway_client::GatewayDispatchClient,
    model::{JobHistoryView, JobOutputView, JobTargetView},
    routes_host_management::{
        get_host_package_update_plan, get_host_process_inventory, get_host_service_inventory,
        get_host_storage_inventory, list_host_package_update_plans, HostProcessInventoryQuery,
    },
    routes_jobs::create_job,
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
                cpu_model: None,
                kernel_release: None,
                virtualization: None,
                update_heartbeat: None,
                internal_build_number: 1,
                capabilities: AgentCapabilitySnapshot {
                    privilege_mode: AgentPrivilegeMode::Unprivileged,
                    effective_uid: Some(1000),
                    max_job_timeout_secs: 3600,
                    can_attempt_privileged_ops: true,
                    can_manage_runtime_tunnels: false,
                    builtin_tunnel_drivers: Default::default(),
                    can_apply_process_limits: false,
                    port_forwarding: Default::default(),
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
        max_timeout_secs: Some(30),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
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

#[tokio::test]
async fn host_process_inventory_keeps_last_success_and_exposes_newer_failure() {
    let memory = MemoryState::default();
    let repo = Repository::Memory(memory.clone());
    upsert_memory_agent(
        &memory.agents,
        &AgentHello {
            client_id: "client-a".to_string(),
            process_incarnation_id: Uuid::new_v4(),
            agent_version: "test".to_string(),
            os_release: "test".to_string(),
            arch: "x86_64".to_string(),
            cpu_model: None,
            kernel_release: None,
            virtualization: None,
            update_heartbeat: None,
            internal_build_number: 1,
            capabilities: AgentCapabilitySnapshot::default(),
        },
    )
    .await;

    let successful_job_id = Uuid::new_v4();
    let failed_job_id = Uuid::new_v4();
    memory.jobs.write().await.extend([
        host_job(successful_job_id, "process_list", "100", "completed"),
        host_job(failed_job_id, "process_list", "200", "failed"),
    ]);
    memory.job_targets.write().await.extend([
        host_job_target(successful_job_id, "completed", None, "150"),
        host_job_target(failed_job_id, "failed", Some("permission denied"), "250"),
    ]);
    let payload = serde_json::to_vec(&HostProcessSnapshot {
        r#type: "process_list".to_string(),
        source: "/proc".to_string(),
        truncated: false,
        processes: vec![
            HostProcessView {
                pid: 1,
                ppid: 0,
                uid: 0,
                state: "S".to_string(),
                name: "init".to_string(),
                command: "/sbin/init".to_string(),
                rss_kib: 4096,
            },
            HostProcessView {
                pid: 22,
                ppid: 1,
                uid: 0,
                state: "S".to_string(),
                name: "sshd".to_string(),
                command: "/usr/sbin/sshd -D".to_string(),
                rss_kib: 8192,
            },
        ],
    })
    .unwrap();
    let split = payload.len() / 2;
    memory.job_outputs.write().await.extend([
        host_job_output(successful_job_id, 1, &payload[split..], "151"),
        host_job_output(successful_job_id, 0, &payload[..split], "150"),
    ]);

    let state = test_state(repo);
    let headers = crate::test_auth_headers(&state).await;
    let Json(view) = get_host_process_inventory(
        State(state),
        headers,
        Path("client-a".to_string()),
        Query(HostProcessInventoryQuery { limit: Some(1) }),
    )
    .await
    .unwrap();

    assert_eq!(view.source_job_id, Some(successful_job_id));
    assert_eq!(view.source.as_deref(), Some("/proc"));
    assert_eq!(view.observed_at.as_deref(), Some("151"));
    assert_eq!(view.processes.len(), 1);
    assert_eq!(view.processes[0].name, "init");
    let attempt = view.last_attempt.expect("latest attempt");
    assert_eq!(attempt.job_id, failed_job_id);
    assert_eq!(attempt.status, "failed");
    assert_eq!(attempt.message.as_deref(), Some("permission denied"));
}

#[tokio::test]
async fn host_service_inventory_keeps_last_success_and_exposes_newer_failure() {
    let memory = MemoryState::default();
    let repo = Repository::Memory(memory.clone());
    upsert_memory_agent(
        &memory.agents,
        &AgentHello {
            client_id: "client-a".to_string(),
            process_incarnation_id: Uuid::new_v4(),
            agent_version: "test".to_string(),
            os_release: "Debian GNU/Linux 12".to_string(),
            arch: "x86_64".to_string(),
            cpu_model: None,
            kernel_release: None,
            virtualization: None,
            update_heartbeat: None,
            internal_build_number: 1,
            capabilities: AgentCapabilitySnapshot::default(),
        },
    )
    .await;

    let successful_job_id = Uuid::new_v4();
    let failed_job_id = Uuid::new_v4();
    memory.jobs.write().await.extend([
        host_job(successful_job_id, "service_inventory", "100", "completed"),
        host_job(failed_job_id, "service_inventory", "200", "failed"),
    ]);
    memory.job_targets.write().await.extend([
        host_job_target(successful_job_id, "completed", None, "150"),
        host_job_target(
            failed_job_id,
            "failed",
            Some("host_service_provider_changed: expected systemd, observed unsupported"),
            "250",
        ),
    ]);
    let payload = serde_json::to_vec(&HostServiceSnapshot {
        r#type: "service_inventory".to_string(),
        capability: HostServiceCapability {
            status: HostServiceCapabilityStatus::Supported,
            provider: Some(HostServiceProvider::Systemd),
            can_inventory: true,
            can_start_stop_restart: true,
            can_enable_disable: true,
            can_read_logs: true,
            enable_backend: Some("systemctl".to_string()),
            ..HostServiceCapability::default()
        },
        truncated: false,
        services: vec![
            HostServiceRecord {
                name: "cron.service".to_string(),
                description: "Regular background program processing daemon".to_string(),
                load_state: "loaded".to_string(),
                active_state: "active".to_string(),
                sub_state: "running".to_string(),
                enabled_state: "enabled".to_string(),
                state_reason: None,
            },
            HostServiceRecord {
                name: "sshd.service".to_string(),
                description: "OpenSSH server".to_string(),
                load_state: "loaded".to_string(),
                active_state: "active".to_string(),
                sub_state: "running".to_string(),
                enabled_state: "enabled".to_string(),
                state_reason: None,
            },
        ],
    })
    .unwrap();
    let split = payload.len() / 2;
    memory.job_outputs.write().await.extend([
        host_job_output(successful_job_id, 1, &payload[split..], "151"),
        host_job_output(successful_job_id, 0, &payload[..split], "150"),
    ]);

    let state = test_state(repo);
    let headers = crate::test_auth_headers(&state).await;
    let Json(view) = get_host_service_inventory(
        State(state),
        headers,
        Path("client-a".to_string()),
        Query(HostProcessInventoryQuery { limit: Some(1) }),
    )
    .await
    .unwrap();

    assert_eq!(view.source_job_id, Some(successful_job_id));
    assert_eq!(view.observed_at.as_deref(), Some("151"));
    assert_eq!(view.services.len(), 1);
    assert_eq!(view.services[0].name, "cron.service");
    assert_eq!(
        view.capability.as_ref().and_then(|value| value.provider),
        Some(HostServiceProvider::Systemd)
    );
    let attempt = view.last_attempt.expect("latest attempt");
    assert_eq!(attempt.job_id, failed_job_id);
    assert_eq!(attempt.status, "failed");
    assert!(attempt
        .message
        .as_deref()
        .is_some_and(|value| value.contains("provider_changed")));
}

#[tokio::test]
async fn host_storage_inventory_keeps_last_success_and_exposes_newer_failure() {
    let memory = MemoryState::default();
    let repo = Repository::Memory(memory.clone());
    upsert_memory_agent(
        &memory.agents,
        &AgentHello {
            client_id: "client-a".to_string(),
            process_incarnation_id: Uuid::new_v4(),
            agent_version: "test".to_string(),
            os_release: "CentOS Linux 7".to_string(),
            arch: "x86_64".to_string(),
            cpu_model: None,
            kernel_release: None,
            virtualization: None,
            update_heartbeat: None,
            internal_build_number: 1,
            capabilities: AgentCapabilitySnapshot::default(),
        },
    )
    .await;

    let successful_job_id = Uuid::new_v4();
    let failed_job_id = Uuid::new_v4();
    memory.jobs.write().await.extend([
        host_job(successful_job_id, "storage_inventory", "100", "completed"),
        host_job(failed_job_id, "storage_inventory", "200", "failed"),
    ]);
    memory.job_targets.write().await.extend([
        host_job_target(successful_job_id, "completed", None, "150"),
        host_job_target(
            failed_job_id,
            "failed",
            Some("host_storage_command_timed_out: /bin/lsblk"),
            "250",
        ),
    ]);
    let snapshot = HostStorageSnapshot {
        r#type: "storage_inventory".to_string(),
        capability: HostStorageCapability {
            status: HostStorageCapabilityStatus::Supported,
            provider: Some(HostStorageProvider::LsblkPairs),
            provider_version: Some("lsblk from util-linux 2.23.2".to_string()),
            available_columns: vec![
                "NAME".to_string(),
                "TYPE".to_string(),
                "SIZE".to_string(),
                "RO".to_string(),
            ],
            can_report_filesystem_usage: false,
            reason: Some(
                "device inventory is supported; this lsblk version does not report FSAVAIL and FSUSE%"
                    .to_string(),
            ),
        },
        include_pseudo_mounts: false,
        devices_truncated: false,
        mounts_truncated: false,
        devices: Vec::new(),
        mounts: Vec::new(),
    };
    memory.job_outputs.write().await.push(host_job_output(
        successful_job_id,
        0,
        &serde_json::to_vec(&snapshot).unwrap(),
        "151",
    ));

    let state = test_state(repo);
    let headers = crate::test_auth_headers(&state).await;
    let Json(view) = get_host_storage_inventory(
        State(state),
        headers,
        Path("client-a".to_string()),
        Query(HostProcessInventoryQuery { limit: Some(100) }),
    )
    .await
    .unwrap();

    assert_eq!(view.source_job_id, Some(successful_job_id));
    assert_eq!(view.observed_at.as_deref(), Some("151"));
    assert_eq!(
        view.capability.as_ref().and_then(|value| value.provider),
        Some(HostStorageProvider::LsblkPairs)
    );
    assert!(
        !view
            .capability
            .as_ref()
            .expect("capability")
            .can_report_filesystem_usage
    );
    let attempt = view.last_attempt.expect("latest attempt");
    assert_eq!(attempt.job_id, failed_job_id);
    assert_eq!(attempt.status, "failed");
    assert!(attempt
        .message
        .as_deref()
        .is_some_and(|value| value.contains("timed_out")));
}

#[tokio::test]
async fn package_update_posture_keeps_success_and_isolates_corrupt_fleet_evidence() {
    let memory = MemoryState::default();
    let repo = Repository::Memory(memory.clone());
    for client_id in ["client-a", "client-b"] {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: client_id.to_string(),
                process_incarnation_id: Uuid::new_v4(),
                agent_version: "test".to_string(),
                os_release: "Ubuntu 22.04".to_string(),
                arch: "x86_64".to_string(),
                cpu_model: None,
                kernel_release: None,
                virtualization: None,
                update_heartbeat: None,
                internal_build_number: 1,
                capabilities: AgentCapabilitySnapshot::default(),
            },
        )
        .await;
    }

    let successful_job_id = Uuid::new_v4();
    let failed_job_id = Uuid::new_v4();
    let corrupt_job_id = Uuid::new_v4();
    memory.jobs.write().await.extend([
        host_job(successful_job_id, "package_update_plan", "100", "completed"),
        host_job(failed_job_id, "package_update_plan", "200", "failed"),
        host_job(corrupt_job_id, "package_update_plan", "300", "completed"),
    ]);
    memory.job_targets.write().await.extend([
        host_job_target_for(successful_job_id, "client-a", "completed", None, "150"),
        host_job_target_for(
            failed_job_id,
            "client-a",
            "failed",
            Some("host_package_command_failed: repository metadata unavailable"),
            "250",
        ),
        host_job_target_for(corrupt_job_id, "client-b", "completed", None, "350"),
    ]);
    let snapshot = HostPackageUpdatePlanSnapshot {
        r#type: "package_update_plan".to_string(),
        capability: HostPackageCapability {
            status: HostPackageCapabilityStatus::Supported,
            provider: Some(HostPackageProvider::Apt),
            distro_id: "ubuntu".to_string(),
            distro_version: Some("22.04".to_string()),
            can_plan_cached: true,
            can_refresh_metadata: true,
            can_apply: true,
            reason: None,
        },
        metadata_refresh_requested: true,
        metadata_refreshed: true,
        plan_hash: Some("a".repeat(64)),
        truncated: false,
        packages: vec![HostPackageUpdateRecord {
            name: "openssl".to_string(),
            architecture: Some("amd64".to_string()),
            current_version: Some("1.1.1-1".to_string()),
            candidate_version: "1.1.1-2".to_string(),
            repository: Some("updates".to_string()),
        }],
        reboot_required_before: Some(false),
    };
    memory.job_outputs.write().await.extend([
        host_job_output(
            successful_job_id,
            0,
            &serde_json::to_vec(&snapshot).unwrap(),
            "151",
        ),
        host_job_output_for(corrupt_job_id, "client-b", 0, b"not-json", "351"),
    ]);

    let state = test_state(repo);
    let headers = crate::test_auth_headers(&state).await;
    let Json(view) = get_host_package_update_plan(
        State(state.clone()),
        headers.clone(),
        Path("client-a".to_string()),
    )
    .await
    .unwrap();
    assert_eq!(view.source_job_id, Some(successful_job_id));
    assert_eq!(view.packages.len(), 1);
    assert_eq!(view.packages[0].name, "openssl");
    assert!(view.metadata_refreshed);
    let latest = view.last_attempt.expect("latest package attempt");
    assert_eq!(latest.job_id, failed_job_id);
    assert_eq!(latest.status, "failed");
    assert!(latest
        .message
        .as_deref()
        .is_some_and(|message| message.contains("metadata unavailable")));

    let Json(fleet) = list_host_package_update_plans(State(state), headers)
        .await
        .unwrap();
    assert_eq!(fleet.len(), 2);
    let client_a = fleet
        .iter()
        .find(|item| item.client_id == "client-a")
        .unwrap();
    assert_eq!(client_a.packages.len(), 1);
    assert!(client_a.evidence_error.is_none());
    let client_b = fleet
        .iter()
        .find(|item| item.client_id == "client-b")
        .unwrap();
    assert!(client_b.packages.is_empty());
    assert_eq!(
        client_b.evidence_error.as_deref(),
        Some("host_package_plan_snapshot_invalid")
    );
}

fn host_job(id: Uuid, command_type: &str, created_at: &str, status: &str) -> JobHistoryView {
    JobHistoryView {
        id,
        actor_id: None,
        command_type: command_type.to_string(),
        source_schedule_id: None,
        privileged: false,
        status: status.to_string(),
        target_count: 1,
        payload_hash: "a".repeat(64),
        max_timeout_secs: 30,
        created_at: created_at.to_string(),
        completed_at: Some(created_at.to_string()),
    }
}

fn host_job_target(
    job_id: Uuid,
    status: &str,
    message: Option<&str>,
    completed_at: &str,
) -> JobTargetView {
    JobTargetView {
        job_id,
        client_id: "client-a".to_string(),
        status: status.to_string(),
        message: message.map(str::to_string),
        exit_code: (status == "completed").then_some(0),
        started_at: Some("100".to_string()),
        deadline_at: Some("300".to_string()),
        completed_at: Some(completed_at.to_string()),
        process_incarnation_id: None,
    }
}

fn host_job_target_for(
    job_id: Uuid,
    client_id: &str,
    status: &str,
    message: Option<&str>,
    completed_at: &str,
) -> JobTargetView {
    JobTargetView {
        client_id: client_id.to_string(),
        ..host_job_target(job_id, status, message, completed_at)
    }
}

fn host_job_output(job_id: Uuid, seq: i32, bytes: &[u8], created_at: &str) -> JobOutputView {
    JobOutputView {
        job_id,
        client_id: "client-a".to_string(),
        seq,
        stream: "stdout".to_string(),
        data_base64: BASE64_STANDARD.encode(bytes),
        storage: "inline".to_string(),
        artifact_object_key: None,
        artifact_sha256_hex: None,
        artifact_size_bytes: Some(bytes.len() as i64),
        exit_code: None,
        done: false,
        received_at: None,
        created_at: created_at.to_string(),
    }
}

fn host_job_output_for(
    job_id: Uuid,
    client_id: &str,
    seq: i32,
    bytes: &[u8],
    created_at: &str,
) -> JobOutputView {
    JobOutputView {
        client_id: client_id.to_string(),
        ..host_job_output(job_id, seq, bytes, created_at)
    }
}

fn test_state(repo: Repository) -> AppState {
    let (events, _) = crate::state::WsEventBus::new(1);
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
