use std::{fs, os::unix::fs::PermissionsExt};

use super::{
    agent_restart_plan, agent_stop_plan, lifecycle_output, request_agent_restart_with_plan,
    validate_restart_executable, AgentRestartPlan, AgentStopPlan,
};

#[test]
fn supervised_stop_requires_restart_prevention_contract() {
    assert!(agent_stop_plan("signal_only", "")
        .unwrap_err()
        .to_string()
        .contains("agent_stop_supervisor_contract_unavailable"));
    assert!(agent_stop_plan("signal_only", "unexpected").is_err());
    assert_eq!(
        agent_stop_plan("signal_only", "restart_prevent_exit_64").unwrap(),
        AgentStopPlan::SystemdRestartPrevent
    );
}

#[test]
fn direct_stop_uses_success_exit_without_supervisor_assumptions() {
    assert_eq!(agent_stop_plan("", "").unwrap(), AgentStopPlan::Direct);
    assert_eq!(agent_stop_plan("process_spawn", "").unwrap().exit_code(), 0);
}

#[test]
fn restart_plan_preserves_supervised_signal_and_uses_same_process_exec_otherwise() {
    assert_eq!(
        agent_restart_plan("signal_only"),
        AgentRestartPlan::SupervisorSignal
    );
    assert_eq!(agent_restart_plan(""), AgentRestartPlan::SameProcessExec);
    assert_eq!(
        agent_restart_plan("process_spawn"),
        AgentRestartPlan::SameProcessExec
    );
}

#[test]
fn same_process_restart_preflight_requires_an_executable_file() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-agent-restart-preflight-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&dir).unwrap();
    let executable = dir.join("vpsman-agent");
    fs::write(&executable, b"test executable").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    validate_restart_executable(&executable).unwrap();

    fs::set_permissions(&executable, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(validate_restart_executable(&executable).is_err());
    let missing = dir.join("missing-agent");
    assert!(validate_restart_executable(&missing).is_err());
    assert!(request_agent_restart_with_plan(&missing, AgentRestartPlan::SameProcessExec).is_err());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn lifecycle_status_is_terminal_and_explicit_about_external_start() {
    let job_id = uuid::Uuid::new_v4();
    let output = lifecycle_output(job_id, "agent_stop", "stop_requested", "systemd", true).unwrap();
    let status: serde_json::Value = serde_json::from_slice(&output.data).unwrap();
    assert_eq!(output.job_id, job_id);
    assert!(output.done);
    assert_eq!(output.exit_code, Some(0));
    assert_eq!(status["type"], "agent_stop");
    assert_eq!(status["status"], "stop_requested");
    assert_eq!(status["supervisor"], "systemd");
    assert_eq!(status["external_start_required"], true);

    let output = lifecycle_output(
        job_id,
        "agent_restart",
        "restart_requested",
        AgentRestartPlan::SupervisorSignal.supervisor(),
        false,
    )
    .unwrap();
    let status: serde_json::Value = serde_json::from_slice(&output.data).unwrap();
    assert!(output.done);
    assert_eq!(output.exit_code, Some(0));
    assert_eq!(status["type"], "agent_restart");
    assert_eq!(status["status"], "restart_requested");
    assert_eq!(status["supervisor"], "supervised");
    assert_eq!(status["external_start_required"], false);
}
