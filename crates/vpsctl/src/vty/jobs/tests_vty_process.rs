use super::{
    host_processes_path, parse_vty_host_process_refresh, parse_vty_process_supervisor,
    parse_vty_user_sessions, process_supervisor_inventory_path,
};
use vpsman_common::{JobCommand, ProcessResourceLimits, ProcessRestartPolicy, ProcessRunPolicy};

const TEST_PROCESS_ARGV_SLEEP: &str = "/bin/sleep";

#[test]
fn parses_vty_host_process_refresh_targets_and_limit() {
    let request =
        parse_vty_host_process_refresh(&["id:client-a", "tag:bgp", "--limit", "25"]).unwrap();

    assert_eq!(request.command_label, "process_list");
    assert!(request.selection.clients.is_empty());
    assert_eq!(request.selection.tags, vec!["bgp", "id:client-a"]);
    assert!(!request.selection.destructive);
    match request.operation {
        JobCommand::ProcessList { limit } => assert_eq!(limit, 25),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn rejects_invalid_vty_host_process_refresh_flags() {
    assert!(parse_vty_host_process_refresh(&["tag:bgp", "--limit", "0"]).is_err());
    assert!(parse_vty_host_process_refresh(&["tag:bgp", "--limit=600"]).is_err());
    assert!(parse_vty_host_process_refresh(&["tag:bgp", "--destructive"]).is_err());
    assert!(parse_vty_host_process_refresh(&["tag:bgp", "--confirmed"]).is_err());
}

#[test]
fn builds_host_process_snapshot_path() {
    assert_eq!(
        host_processes_path("host-processes --client-id edge/a --limit 25").unwrap(),
        "/api/v1/host-processes/edge%2Fa?limit=25"
    );
    assert!(host_processes_path("host-processes --limit 25").is_err());
}

#[test]
fn parses_vty_user_sessions_targets_and_timeout() {
    let request =
        parse_vty_user_sessions(&["id:client-a", "tag:bgp", "--confirmed", "--max-timeout=45"])
            .unwrap();

    assert_eq!(request.command_label, "user_sessions");
    assert_eq!(request.max_timeout_secs, 45);
    assert!(request.selection.clients.is_empty());
    assert_eq!(request.selection.tags, vec!["bgp", "id:client-a"]);
    assert!(request.selection.confirmed);
    match request.operation {
        JobCommand::UserSessions => {}
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parses_process_supervisor_inventory_limit() {
    assert_eq!(
        process_supervisor_inventory_path("process-supervisor-inventory --limit 25").unwrap(),
        "/api/v1/process-supervisor/inventory?limit=25"
    );
    assert!(process_supervisor_inventory_path("process-supervisor-inventory --limit=0").is_err());
    assert!(process_supervisor_inventory_path("process-supervisor-inventory tag:edge").is_err());
}

#[test]
fn rejects_invalid_vty_user_sessions_flags() {
    assert!(parse_vty_user_sessions(&["tag:bgp", "--max-timeout=0"]).is_err());
    assert!(parse_vty_user_sessions(&["tag:bgp", "--destructive"]).is_err());
    assert!(parse_vty_user_sessions(&["--confirmed"]).is_err());
}

#[test]
fn parses_vty_process_start_with_targets_and_options() {
    let request = parse_vty_process_supervisor(
        "process-start",
        &[
            "edge-worker",
            "--argv",
            TEST_PROCESS_ARGV_SLEEP,
            "--argv=60",
            "--cwd",
            "/tmp",
            "--env=KEY=value",
            "id:client-a",
            "tag:bgp",
            "--confirmed",
            "--max-timeout=45",
        ],
    )
    .unwrap();

    assert_eq!(request.command_label, "process_start");
    assert_eq!(request.max_timeout_secs, 45);
    assert!(request.selection.clients.is_empty());
    assert_eq!(request.selection.tags, vec!["bgp", "id:client-a"]);
    assert!(request.selection.confirmed);
    match request.operation {
        JobCommand::ProcessStart {
            name,
            argv,
            cwd,
            env,
            policy,
            limits,
        } => {
            assert_eq!(name, "edge-worker");
            assert_eq!(argv, vec!["/bin/sleep", "60"]);
            assert_eq!(cwd.as_deref(), Some("/tmp"));
            assert_eq!(env.get("KEY").map(String::as_str), Some("value"));
            assert_eq!(policy, ProcessRunPolicy::default());
            assert_eq!(limits, ProcessResourceLimits::default());
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parses_vty_process_start_policy_and_limits() {
    let request = parse_vty_process_supervisor(
        "process-start",
        &[
            "limited-worker",
            "--argv=/bin/sleep",
            "--argv=60",
            "--restart-policy=on-failure",
            "--restart-max-retries=3",
            "--restart-backoff-secs=10",
            "--graceful-stop-secs=15",
            "--memory-max-bytes=134217728",
            "--pids-max=32",
            "--open-files-max=256",
            "--cpu-shares=1024",
            "--no-new-privileges",
            "--force-unprivileged",
            "id:client-a",
            "--confirmed",
        ],
    )
    .unwrap();

    assert!(request.force_unprivileged);
    match request.operation {
        JobCommand::ProcessStart { policy, limits, .. } => {
            assert_eq!(policy.restart, ProcessRestartPolicy::OnFailure);
            assert_eq!(policy.restart_max_retries, 3);
            assert_eq!(policy.restart_backoff_secs, 10);
            assert_eq!(policy.graceful_stop_secs, 15);
            assert_eq!(limits.memory_max_bytes, Some(134217728));
            assert_eq!(limits.pids_max, Some(32));
            assert_eq!(limits.open_files_max, Some(256));
            assert_eq!(limits.cpu_shares, Some(1024));
            assert!(limits.no_new_privileges);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parses_vty_process_status_and_logs() {
    let status =
        parse_vty_process_supervisor("process-status", &["--name=demo", "pool:pool-a"]).unwrap();
    assert_eq!(status.command_label, "process_status");
    match status.operation {
        JobCommand::ProcessStatus { name } => assert_eq!(name.as_deref(), Some("demo")),
        other => panic!("unexpected command: {other:?}"),
    }

    let logs =
        parse_vty_process_supervisor("process-logs", &["demo", "--max-bytes", "4096", "tag:edge"])
            .unwrap();
    assert_eq!(logs.command_label, "process_logs");
    match logs.operation {
        JobCommand::ProcessLogs { name, max_bytes } => {
            assert_eq!(name, "demo");
            assert_eq!(max_bytes, 4096);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn rejects_bad_vty_process_supervisor_requests() {
    assert!(parse_vty_process_supervisor(
        "process-start",
        &["demo", "--argv", "sleep", "tag:edge"]
    )
    .is_err());
    assert!(parse_vty_process_supervisor(
        "process-logs",
        &["demo", "--max-bytes", "0", "tag:edge"]
    )
    .is_err());
    assert!(
        parse_vty_process_supervisor("process-stop", &["demo", "--destructive", "tag:edge"])
            .is_err()
    );
}
