use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::Duration,
};

use vpsman_common::{
    CommandOutput, JobCommand, OutputStream, ProcessResourceLimits, ProcessRestartPolicy,
    ProcessRunPolicy,
};

use crate::supervisor::{execute_blocking, reconcile_supervisor_records_at_root};

const TEST_LOG_TAIL_BYTES: u32 = 64 * 1024;
const TEST_SUPERVISOR_SHELL: &str = "/bin/sh";

fn test_root(name: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("vpsman-supervisor-{name}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    root
}

fn cgroup_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[test]
fn rejects_invalid_supervisor_start_request() {
    assert!(execute_blocking(
        uuid::Uuid::new_v4(),
        &JobCommand::ProcessStart {
            name: "../bad".to_string(),
            argv: vec!["sleep".to_string()],
            cwd: None,
            env: BTreeMap::new(),
            policy: ProcessRunPolicy::default(),
            limits: ProcessResourceLimits::default(),
        },
        &test_root("bad")
    )
    .is_err());
}

#[test]
fn starts_statuses_logs_and_stops_process() {
    let root = test_root("lifecycle");
    let job_id = uuid::Uuid::new_v4();
    let outputs = execute_blocking(
        job_id,
        &JobCommand::ProcessStart {
            name: "demo".to_string(),
            argv: vec![
                TEST_SUPERVISOR_SHELL.to_string(),
                "-c".to_string(),
                "echo ready; sleep 30".to_string(),
            ],
            cwd: None,
            env: BTreeMap::new(),
            policy: ProcessRunPolicy::default(),
            limits: ProcessResourceLimits::default(),
        },
        &root,
    )
    .unwrap();
    let status = status_from_outputs(&outputs);
    assert_eq!(status["type"], "process_start");
    assert_eq!(status["name"], "demo");
    assert_eq!(status["process_group_id"], status["pid"]);

    let outputs = execute_blocking(
        uuid::Uuid::new_v4(),
        &JobCommand::ProcessStatus {
            name: Some("demo".to_string()),
        },
        &root,
    )
    .unwrap();
    let stdout = outputs
        .iter()
        .find(|output| output.stream == OutputStream::Stdout)
        .unwrap();
    let status: serde_json::Value = serde_json::from_slice(&stdout.data).unwrap();
    assert_eq!(status["processes"][0]["name"], "demo");

    let mut saw_ready = false;
    for _ in 0..20 {
        let outputs = execute_blocking(
            uuid::Uuid::new_v4(),
            &JobCommand::ProcessLogs {
                name: "demo".to_string(),
                max_bytes: TEST_LOG_TAIL_BYTES,
            },
            &root,
        )
        .unwrap();
        saw_ready = outputs.iter().any(|output| {
            output.stream == OutputStream::Stdout && output.data.starts_with(b"ready")
        });
        if saw_ready {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(saw_ready);

    let outputs = execute_blocking(
        uuid::Uuid::new_v4(),
        &JobCommand::ProcessStop {
            name: "demo".to_string(),
        },
        &root,
    )
    .unwrap();
    let status = status_from_outputs(&outputs);
    assert_eq!(status["type"], "process_stop");
    assert_eq!(status["cleanup"]["target_kind"], "process_group");
    assert_eq!(status["cleanup"]["final_running"], false);
}

#[test]
fn stop_cleans_process_group_children() {
    let root = test_root("tree-cleanup");
    let child_pid_file = root.join("child.pid");
    let script = format!("sleep 30 & echo $! > '{}'; wait", child_pid_file.display());
    let outputs = execute_blocking(
        uuid::Uuid::new_v4(),
        &JobCommand::ProcessStart {
            name: "tree".to_string(),
            argv: vec![TEST_SUPERVISOR_SHELL.to_string(), "-c".to_string(), script],
            cwd: None,
            env: BTreeMap::new(),
            policy: ProcessRunPolicy {
                graceful_stop_secs: 1,
                ..ProcessRunPolicy::default()
            },
            limits: ProcessResourceLimits::default(),
        },
        &root,
    )
    .unwrap();
    let status = status_from_outputs(&outputs);
    assert_eq!(status["process_group_id"], status["pid"]);

    let child_pid = wait_for_pid_file(&child_pid_file);
    assert!(process_running(child_pid));

    let outputs = execute_blocking(
        uuid::Uuid::new_v4(),
        &JobCommand::ProcessStop {
            name: "tree".to_string(),
        },
        &root,
    )
    .unwrap();
    let status = status_from_outputs(&outputs);
    assert_eq!(status["cleanup"]["target_kind"], "process_group");
    assert_eq!(status["cleanup"]["fallback_used"], false);
    assert_eq!(status["cleanup"]["final_running"], false);
    for _ in 0..20 {
        if !process_running(child_pid) {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        !process_running(child_pid),
        "child pid {child_pid} survived stop"
    );
}

#[test]
fn restart_monitor_restarts_failed_process_until_retry_budget() {
    let root = test_root("restart");
    let counter = root.join("counter");
    let script = format!(
        "n=$(cat '{}' 2>/dev/null || echo 0); n=$((n + 1)); echo \"$n\" > '{}'; if [ \"$n\" -eq 1 ]; then exit 7; fi; echo restarted; sleep 30",
        counter.display(),
        counter.display()
    );
    let outputs = execute_blocking(
        uuid::Uuid::new_v4(),
        &JobCommand::ProcessStart {
            name: "flap".to_string(),
            argv: vec![TEST_SUPERVISOR_SHELL.to_string(), "-c".to_string(), script],
            cwd: None,
            env: BTreeMap::new(),
            policy: ProcessRunPolicy {
                restart: ProcessRestartPolicy::OnFailure,
                restart_max_retries: 1,
                restart_backoff_secs: 0,
                graceful_stop_secs: 1,
            },
            limits: ProcessResourceLimits::default(),
        },
        &root,
    )
    .unwrap();
    let status = status_from_outputs(&outputs);
    assert_eq!(status["restart_attempts"], 0);

    let mut restarted = false;
    for _ in 0..30 {
        let count = fs::read_to_string(&counter).unwrap_or_default();
        if count.trim() == "2" {
            restarted = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(restarted, "restart monitor did not run the process twice");

    let outputs = execute_blocking(
        uuid::Uuid::new_v4(),
        &JobCommand::ProcessStatus {
            name: Some("flap".to_string()),
        },
        &root,
    )
    .unwrap();
    let stdout = outputs
        .iter()
        .find(|output| output.stream == OutputStream::Stdout)
        .unwrap();
    let status: serde_json::Value = serde_json::from_slice(&stdout.data).unwrap();
    let process = &status["processes"][0];
    assert_eq!(process["status"], "running");
    assert_eq!(process["restart_attempts"], 1);
    assert_eq!(process["last_exit_code"], 7);
    assert!(process["last_restart_unix"].as_u64().is_some());

    let _ = execute_blocking(
        uuid::Uuid::new_v4(),
        &JobCommand::ProcessStop {
            name: "flap".to_string(),
        },
        &root,
    )
    .unwrap();
}

#[test]
fn startup_reconcile_restarts_persisted_process_with_restart_policy() {
    let root = test_root("startup-reconcile");
    let records = root.join("records");
    let logs = root.join("logs");
    fs::create_dir_all(&records).unwrap();
    fs::create_dir_all(&logs).unwrap();
    let marker = root.join("startup-marker");
    let script = format!("echo restarted > '{}'; sleep 30", marker.display());
    fs::write(
        records.join("daemon.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "name": "daemon",
            "argv": [TEST_SUPERVISOR_SHELL, "-c", script],
            "cwd": null,
            "env": {},
            "policy": {
                "restart": "on_failure",
                "restart_max_retries": 1,
                "restart_backoff_secs": 0,
                "graceful_stop_secs": 1
            },
            "limits": {},
            "pid": 4_000_000_u32,
            "process_group_id": 4_000_000_u32,
            "started_unix": 1_u64,
            "stdout_log": logs.join("daemon.stdout.log").to_string_lossy(),
            "stderr_log": logs.join("daemon.stderr.log").to_string_lossy(),
            "status": "running",
            "exit_code": null,
            "restart_attempts": 0_u16,
            "last_exit_code": null,
            "last_exit_unix": null,
            "last_restart_unix": null,
            "cgroup_path": null,
            "limit_evidence": {}
        }))
        .unwrap(),
    )
    .unwrap();

    let report = reconcile_supervisor_records_at_root(&root).unwrap();

    assert_eq!(report["total"], 1);
    assert_eq!(report["restarted"], 1);
    assert_eq!(report["processes"][0]["name"], "daemon");
    assert_eq!(report["processes"][0]["status"], "running");
    for _ in 0..20 {
        if fs::read_to_string(&marker).unwrap_or_default().trim() == "restarted" {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(
        fs::read_to_string(&marker).unwrap_or_default().trim(),
        "restarted"
    );

    let _ = execute_blocking(
        uuid::Uuid::new_v4(),
        &JobCommand::ProcessStop {
            name: "daemon".to_string(),
        },
        &root,
    )
    .unwrap();
}

#[test]
fn reports_cpu_shares_as_desired_only_limit_evidence() {
    let _guard = cgroup_env_lock().lock().unwrap();
    std::env::set_var("VPSMAN_PROCESS_CGROUP_DISABLED", "1");
    std::env::remove_var("VPSMAN_PROCESS_CGROUP_ROOT");
    let root = test_root("limits");
    let outputs = execute_blocking(
        uuid::Uuid::new_v4(),
        &JobCommand::ProcessStart {
            name: "limited".to_string(),
            argv: vec![
                TEST_SUPERVISOR_SHELL.to_string(),
                "-c".to_string(),
                "sleep 30".to_string(),
            ],
            cwd: None,
            env: BTreeMap::new(),
            policy: ProcessRunPolicy::default(),
            limits: ProcessResourceLimits {
                cpu_shares: Some(1024),
                ..ProcessResourceLimits::default()
            },
        },
        &root,
    )
    .unwrap();
    let status = status_from_outputs(&outputs);
    assert_eq!(
        status["limit_effectiveness"]["overall"]["status"],
        "degraded_desired_only"
    );
    assert_eq!(
        status["limit_effectiveness"]["cpu_shares"]["status"],
        "desired_only"
    );
    assert!(status["limit_effectiveness"]["cpu_shares"]["reason"]
        .as_str()
        .unwrap()
        .contains("disabled"));
    assert_eq!(status["cgroup_status"]["status"], "not_attached");

    let _ = execute_blocking(
        uuid::Uuid::new_v4(),
        &JobCommand::ProcessStop {
            name: "limited".to_string(),
        },
        &root,
    )
    .unwrap();
    std::env::remove_var("VPSMAN_PROCESS_CGROUP_DISABLED");
}

#[test]
fn enforces_cpu_shares_with_configured_cgroup_v2_root() {
    let _guard = cgroup_env_lock().lock().unwrap();
    std::env::remove_var("VPSMAN_PROCESS_CGROUP_DISABLED");
    let root = test_root("cgroup");
    let cgroup_root = root.join("cgroup");
    fs::create_dir_all(&cgroup_root).unwrap();
    fs::write(cgroup_root.join("cgroup.controllers"), "cpu memory\n").unwrap();
    fs::write(cgroup_root.join("cgroup.subtree_control"), "").unwrap();
    std::env::set_var("VPSMAN_PROCESS_CGROUP_ROOT", &cgroup_root);

    let outputs = execute_blocking(
        uuid::Uuid::new_v4(),
        &JobCommand::ProcessStart {
            name: "cgworker".to_string(),
            argv: vec![
                TEST_SUPERVISOR_SHELL.to_string(),
                "-c".to_string(),
                "sleep 30".to_string(),
            ],
            cwd: None,
            env: BTreeMap::new(),
            policy: ProcessRunPolicy::default(),
            limits: ProcessResourceLimits {
                cpu_shares: Some(1024),
                ..ProcessResourceLimits::default()
            },
        },
        &root,
    )
    .unwrap();
    let status = status_from_outputs(&outputs);
    assert_eq!(
        status["limit_effectiveness"]["overall"]["status"],
        "enforced"
    );
    assert_eq!(
        status["limit_effectiveness"]["cpu_shares"]["status"],
        "enforced_cgroup_v2"
    );
    assert_eq!(
        status["limit_effectiveness"]["cpu_shares"]["applied_weight"],
        39
    );
    let cgroup_path = PathBuf::from(status["cgroup_path"].as_str().unwrap());
    assert!(cgroup_path.starts_with(&cgroup_root));
    assert_eq!(status["cgroup_status"]["status"], "available");
    assert_eq!(status["cgroup_status"]["cpu_weight"], 39);
    assert_eq!(status["cgroup_status"]["process_count"], 1);
    assert_eq!(
        fs::read_to_string(cgroup_path.join("cpu.weight")).unwrap(),
        "39"
    );
    let pid = status["pid"].as_u64().unwrap().to_string();
    assert_eq!(
        fs::read_to_string(cgroup_path.join("cgroup.procs")).unwrap(),
        pid
    );
    fs::write(cgroup_path.join("memory.current"), "1048576").unwrap();
    fs::write(cgroup_path.join("pids.current"), "1").unwrap();
    fs::write(cgroup_path.join("cgroup.events"), "populated 1\nfrozen 0\n").unwrap();
    fs::write(
        cgroup_path.join("cpu.stat"),
        "usage_usec 123\nuser_usec 100\nsystem_usec 23\n",
    )
    .unwrap();

    let outputs = execute_blocking(
        uuid::Uuid::new_v4(),
        &JobCommand::ProcessStatus {
            name: Some("cgworker".to_string()),
        },
        &root,
    )
    .unwrap();
    let stdout = outputs
        .iter()
        .find(|output| output.stream == OutputStream::Stdout)
        .unwrap();
    let status: serde_json::Value = serde_json::from_slice(&stdout.data).unwrap();
    let process = &status["processes"][0];
    assert_eq!(process["cgroup_status"]["memory_current_bytes"], 1048576);
    assert_eq!(process["cgroup_status"]["pids_current"], 1);
    assert_eq!(process["cgroup_status"]["events"]["populated"], 1);
    assert_eq!(process["cgroup_status"]["cpu_stat"]["usage_usec"], 123);

    let _ = execute_blocking(
        uuid::Uuid::new_v4(),
        &JobCommand::ProcessStop {
            name: "cgworker".to_string(),
        },
        &root,
    )
    .unwrap();
    std::env::remove_var("VPSMAN_PROCESS_CGROUP_ROOT");
}

fn status_from_outputs(outputs: &[CommandOutput]) -> serde_json::Value {
    let status = outputs.iter().find(|output| output.done).unwrap();
    serde_json::from_slice(&status.data).unwrap()
}

fn wait_for_pid_file(path: &std::path::Path) -> u32 {
    for _ in 0..40 {
        if let Ok(contents) = fs::read_to_string(path) {
            if let Ok(pid) = contents.trim().parse::<u32>() {
                return pid;
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("pid file was not created: {}", path.display());
}

fn process_running(pid: u32) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}
