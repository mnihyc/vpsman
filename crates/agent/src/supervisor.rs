use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::time;
use vpsman_common::{
    CommandOutput, JobCommand, OutputStream, ProcessResourceLimits, ProcessRestartPolicy,
    ProcessRunPolicy,
};

use crate::process_cleanup::{
    process_is_running, set_current_process_group, terminate_process_blocking,
    terminate_process_group_blocking, ProcessCleanupReport,
};
use crate::supervisor_cgroup::{
    apply_cpu_shares_cgroup_v2, cgroup_status, cleanup_process_cgroup, limit_effectiveness,
    ProcessLimitEvidence,
};
use crate::supervisor_validation::{
    validate_process_argv, validate_process_cwd, validate_process_env, validate_process_limits,
    validate_process_name, validate_process_policy,
};

const DEFAULT_STATE_ROOT: &str = "/var/lib/vpsman/supervisor";
const MAX_LOG_TAIL_BYTES: u32 = 512 * 1024;
const MONITOR_IDLE_SLEEP_SECS: u64 = 1;

static PROCESS_MONITORS: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();

#[derive(Debug, Deserialize, Serialize)]
struct ProcessRecord {
    name: String,
    argv: Vec<String>,
    cwd: Option<String>,
    env: BTreeMap<String, String>,
    #[serde(default)]
    policy: ProcessRunPolicy,
    #[serde(default)]
    limits: ProcessResourceLimits,
    pid: u32,
    #[serde(default)]
    process_group_id: Option<u32>,
    started_unix: u64,
    stdout_log: String,
    stderr_log: String,
    status: String,
    exit_code: Option<i32>,
    #[serde(default)]
    restart_attempts: u16,
    #[serde(default)]
    last_exit_code: Option<i32>,
    #[serde(default)]
    last_exit_unix: Option<u64>,
    #[serde(default)]
    last_restart_unix: Option<u64>,
    #[serde(default)]
    cgroup_path: Option<String>,
    #[serde(default)]
    limit_evidence: ProcessLimitEvidence,
}

pub(crate) async fn execute_process_supervisor_command(
    job_id: uuid::Uuid,
    command: &JobCommand,
    timeout_secs: u64,
) -> Result<Vec<CommandOutput>> {
    let root = supervisor_root();
    time::timeout(
        Duration::from_secs(timeout_secs.clamp(1, 60)),
        execute_at_root(job_id, command, &root),
    )
    .await
    .context("process supervisor command timed out")?
}

async fn execute_at_root(
    job_id: uuid::Uuid,
    command: &JobCommand,
    root: &Path,
) -> Result<Vec<CommandOutput>> {
    tokio::task::spawn_blocking({
        let command = command.clone();
        let root = root.to_path_buf();
        move || execute_blocking(job_id, &command, &root)
    })
    .await?
}

pub(crate) fn execute_blocking(
    job_id: uuid::Uuid,
    command: &JobCommand,
    root: &Path,
) -> Result<Vec<CommandOutput>> {
    fs::create_dir_all(records_dir(root))?;
    fs::create_dir_all(logs_dir(root))?;
    match command {
        JobCommand::ProcessStart {
            name,
            argv,
            cwd,
            env,
            policy,
            limits,
        } => {
            validate_process_name(name)?;
            validate_process_argv(argv)?;
            validate_process_cwd(cwd.as_deref())?;
            validate_process_env(env)?;
            validate_process_policy(policy)?;
            validate_process_limits(limits)?;
            if let Some(record) = load_record(root, name)? {
                if process_is_running(record.pid) {
                    anyhow::bail!("process {name} is already running with pid {}", record.pid);
                }
            }
            let record = start_process(root, name, argv, cwd, env, policy, limits)?;
            save_record(root, &record)?;
            ensure_restart_monitor(root, name);
            Ok(status_outputs(job_id, "process_start", &record))
        }
        JobCommand::ProcessStop { name } => {
            validate_process_name(name)?;
            let mut record =
                load_record(root, name)?.context("process is not managed by vpsman")?;
            let cleanup = stop_record(&record);
            let stopped = !process_is_running(record.pid);
            record.status = if stopped { "stopped" } else { "stop_requested" }.to_string();
            record.exit_code = if stopped { Some(0) } else { None };
            save_record(root, &record)?;
            Ok(status_outputs_with_cleanup(
                job_id,
                "process_stop",
                &record,
                &cleanup,
            ))
        }
        JobCommand::ProcessRestart { name } => {
            validate_process_name(name)?;
            let mut record =
                load_record(root, name)?.context("process is not managed by vpsman")?;
            let cleanup = stop_record(&record);
            record = start_process(
                root,
                &record.name,
                &record.argv,
                &record.cwd,
                &record.env,
                &record.policy,
                &record.limits,
            )?;
            save_record(root, &record)?;
            ensure_restart_monitor(root, name);
            Ok(status_outputs_with_cleanup(
                job_id,
                "process_restart",
                &record,
                &cleanup,
            ))
        }
        JobCommand::ProcessStatus { name } => {
            if let Some(name) = name {
                validate_process_name(name)?;
            }
            let records = if let Some(name) = name {
                load_record(root, name)?
                    .into_iter()
                    .map(|record| reconcile_and_save_record(root, record))
                    .collect::<Result<Vec<_>>>()?
            } else {
                load_all_records(root)?
                    .into_iter()
                    .map(|record| reconcile_and_save_record(root, record))
                    .collect::<Result<Vec<_>>>()?
            };
            Ok(json_stdout_outputs(
                job_id,
                "process_status",
                &serde_json::json!({
                    "type": "process_status",
                    "processes": records.iter().map(process_record_status_value).collect::<Vec<_>>(),
                }),
            )?)
        }
        JobCommand::ProcessLogs { name, max_bytes } => {
            validate_process_name(name)?;
            let record = load_record(root, name)?.context("process is not managed by vpsman")?;
            let max_bytes = (*max_bytes).clamp(1, MAX_LOG_TAIL_BYTES);
            let mut outputs = Vec::new();
            push_tail_output(
                job_id,
                OutputStream::Stdout,
                Path::new(&record.stdout_log),
                max_bytes,
                &mut outputs,
            )?;
            push_tail_output(
                job_id,
                OutputStream::Stderr,
                Path::new(&record.stderr_log),
                max_bytes,
                &mut outputs,
            )?;
            let refreshed = reconcile_and_save_record(root, record)?;
            outputs.push(CommandOutput {
                job_id,
                stream: OutputStream::Status,
                data: serde_json::to_vec(&status_value(
                    "process_logs",
                    &refreshed,
                    Some(max_bytes),
                ))?,
                exit_code: Some(0),
                done: true,
            });
            Ok(outputs)
        }
        _ => anyhow::bail!("not a process supervisor command"),
    }
}

fn start_process(
    root: &Path,
    name: &str,
    argv: &[String],
    cwd: &Option<String>,
    env: &BTreeMap<String, String>,
    policy: &ProcessRunPolicy,
    limits: &ProcessResourceLimits,
) -> Result<ProcessRecord> {
    let stdout_log = logs_dir(root).join(format!("{name}.stdout.log"));
    let stderr_log = logs_dir(root).join(format!("{name}.stderr.log"));
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stdout_log)
        .with_context(|| format!("failed to open {}", stdout_log.display()))?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stderr_log)
        .with_context(|| format!("failed to open {}", stderr_log.display()))?;
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command.envs(env);
    command.stdin(Stdio::null());
    command.stdout(Stdio::from(stdout));
    command.stderr(Stdio::from(stderr));
    let limits_for_child = limits.clone();
    unsafe {
        command.pre_exec(move || {
            set_current_process_group()?;
            apply_process_limits_in_child(&limits_for_child)
        });
    }
    let child = command
        .spawn()
        .with_context(|| format!("failed to start supervised process {name}"))?;
    let mut record = ProcessRecord {
        name: name.to_string(),
        argv: argv.to_vec(),
        cwd: cwd.clone(),
        env: env.clone(),
        policy: policy.clone(),
        limits: limits.clone(),
        pid: child.id(),
        process_group_id: Some(child.id()),
        started_unix: unix_now(),
        stdout_log: stdout_log.to_string_lossy().to_string(),
        stderr_log: stderr_log.to_string_lossy().to_string(),
        status: "running".to_string(),
        exit_code: None,
        restart_attempts: 0,
        last_exit_code: None,
        last_exit_unix: None,
        last_restart_unix: None,
        cgroup_path: None,
        limit_evidence: ProcessLimitEvidence::default(),
    };
    if let Some(cpu_shares) = limits.cpu_shares {
        let evidence = apply_cpu_shares_cgroup_v2(name, child.id(), cpu_shares);
        record.cgroup_path.clone_from(&evidence.path);
        record.limit_evidence.cpu_shares = Some(evidence);
    }
    Ok(record)
}

fn stop_record(record: &ProcessRecord) -> ProcessCleanupReport {
    if collect_child_exit_code(record.pid).is_some() || !process_is_running(record.pid) {
        return ProcessCleanupReport {
            target_kind: "process",
            target_id: record.pid as libc::pid_t,
            graceful_signal: "SIGTERM",
            graceful_wait_ms: 0,
            graceful_signal_sent: false,
            forced_signal: None,
            forced_signal_sent: false,
            exited_after_grace: true,
            final_running: false,
            fallback_used: false,
            errors: Vec::new(),
        };
    }
    let graceful_wait = Duration::from_secs(record.policy.graceful_stop_secs.clamp(1, 300));
    let process_group_id = record.process_group_id.unwrap_or(record.pid) as libc::pid_t;
    let mut group_report = terminate_process_group_blocking(process_group_id, graceful_wait);
    if collect_child_exit_code(record.pid).is_some() || !process_is_running(record.pid) {
        group_report.final_running = false;
        return group_report;
    }
    let mut fallback = terminate_process_blocking(record.pid as libc::pid_t, graceful_wait);
    fallback.fallback_used = true;
    fallback
}

fn collect_child_exit_code(pid: u32) -> Option<i32> {
    unsafe {
        let mut status = 0;
        let waited = libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG);
        if waited == pid as libc::pid_t {
            Some(exit_code_from_wait_status(status))
        } else {
            None
        }
    }
}

fn exit_code_from_wait_status(status: i32) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        1
    }
}

fn reconcile_and_save_record(root: &Path, record: ProcessRecord) -> Result<ProcessRecord> {
    let record = reconcile_record(root, record)?;
    save_record(root, &record)?;
    ensure_restart_monitor(root, &record.name);
    Ok(record)
}

fn reconcile_record(root: &Path, mut record: ProcessRecord) -> Result<ProcessRecord> {
    if matches!(record.status.as_str(), "stopped" | "stop_requested") {
        return Ok(record);
    }
    if let Some(exit_code) = collect_child_exit_code(record.pid) {
        mark_record_exited(&mut record, Some(exit_code));
    } else if process_is_running(record.pid) {
        record.status = "running".to_string();
        record.exit_code = None;
        return Ok(record);
    } else if matches!(record.status.as_str(), "running" | "restart_pending")
        && record.last_exit_unix.is_none()
    {
        mark_record_exited(&mut record, None);
    }
    maybe_restart_record(root, record)
}

fn mark_record_exited(record: &mut ProcessRecord, exit_code: Option<i32>) {
    let status = if exit_code == Some(0) {
        "exited"
    } else {
        "failed"
    };
    record.status = status.to_string();
    record.exit_code = exit_code;
    record.last_exit_code = exit_code;
    record.last_exit_unix = Some(unix_now());
}

fn maybe_restart_record(root: &Path, mut record: ProcessRecord) -> Result<ProcessRecord> {
    if !restart_policy_matches_exit(&record) {
        if record.policy.restart != ProcessRestartPolicy::Never
            && record.restart_attempts >= record.policy.restart_max_retries
            && record.policy.restart_max_retries > 0
        {
            record.status = if record.exit_code == Some(0) {
                "exited_no_retries_remaining"
            } else {
                "failed_no_retries_remaining"
            }
            .to_string();
        }
        return Ok(record);
    }
    cleanup_process_cgroup(record.cgroup_path.as_deref());
    if record.restart_attempts >= record.policy.restart_max_retries {
        record.status = if record.exit_code == Some(0) {
            "exited_no_retries_remaining"
        } else {
            "failed_no_retries_remaining"
        }
        .to_string();
        return Ok(record);
    }
    let now = unix_now();
    let last_exit_unix = record.last_exit_unix.unwrap_or(now);
    if now.saturating_sub(last_exit_unix) < record.policy.restart_backoff_secs {
        record.status = "restart_pending".to_string();
        return Ok(record);
    }
    let restart_attempts = record.restart_attempts.saturating_add(1);
    let last_exit_code = record.last_exit_code;
    let last_exit_unix = record.last_exit_unix;
    let mut restarted = start_process(
        root,
        &record.name,
        &record.argv,
        &record.cwd,
        &record.env,
        &record.policy,
        &record.limits,
    )?;
    restarted.restart_attempts = restart_attempts;
    restarted.last_exit_code = last_exit_code;
    restarted.last_exit_unix = last_exit_unix;
    restarted.last_restart_unix = Some(now);
    Ok(restarted)
}

fn restart_policy_matches_exit(record: &ProcessRecord) -> bool {
    match record.policy.restart {
        ProcessRestartPolicy::Never => false,
        ProcessRestartPolicy::Always => true,
        ProcessRestartPolicy::OnFailure => record.exit_code.unwrap_or(1) != 0,
    }
}

fn ensure_restart_monitor(root: &Path, name: &str) {
    let Ok(Some(record)) = load_record(root, name) else {
        return;
    };
    if record.policy.restart == ProcessRestartPolicy::Never {
        return;
    }
    let key = monitor_key(root, name);
    {
        let mut active = process_monitors().lock().unwrap();
        if !active.insert(key.clone()) {
            return;
        }
    }
    let root = root.to_path_buf();
    let name = name.to_string();
    let spawn_result = std::thread::Builder::new()
        .name(format!("vpsman-process-monitor-{name}"))
        .spawn({
            let key = key.clone();
            move || {
                run_restart_monitor(&root, &name);
                process_monitors().lock().unwrap().remove(&key);
            }
        });
    if spawn_result.is_err() {
        process_monitors().lock().unwrap().remove(&key);
    }
}

fn run_restart_monitor(root: &Path, name: &str) {
    loop {
        std::thread::sleep(monitor_sleep(root, name));
        let Ok(Some(record)) = load_record(root, name) else {
            break;
        };
        let Ok(record) = reconcile_record(root, record) else {
            break;
        };
        if save_record(root, &record).is_err() || !monitor_should_continue(&record) {
            break;
        }
    }
}

fn monitor_sleep(root: &Path, name: &str) -> Duration {
    let Ok(Some(record)) = load_record(root, name) else {
        return Duration::from_secs(MONITOR_IDLE_SLEEP_SECS);
    };
    if record.status == "restart_pending" {
        let now = unix_now();
        let remaining = record
            .last_exit_unix
            .unwrap_or(now)
            .saturating_add(record.policy.restart_backoff_secs)
            .saturating_sub(now);
        if remaining == 0 {
            Duration::from_millis(100)
        } else {
            Duration::from_secs(remaining.min(MONITOR_IDLE_SLEEP_SECS))
        }
    } else {
        Duration::from_secs(MONITOR_IDLE_SLEEP_SECS)
    }
}

fn monitor_should_continue(record: &ProcessRecord) -> bool {
    record.policy.restart != ProcessRestartPolicy::Never
        && matches!(record.status.as_str(), "running" | "restart_pending")
}

fn process_monitors() -> &'static Mutex<BTreeSet<String>> {
    PROCESS_MONITORS.get_or_init(|| Mutex::new(BTreeSet::new()))
}

fn monitor_key(root: &Path, name: &str) -> String {
    format!("{}::{name}", root.to_string_lossy())
}

fn status_outputs(
    job_id: uuid::Uuid,
    event_type: &str,
    record: &ProcessRecord,
) -> Vec<CommandOutput> {
    vec![CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status_value(event_type, record, None)).unwrap_or_default(),
        exit_code: Some(0),
        done: true,
    }]
}

fn status_outputs_with_cleanup(
    job_id: uuid::Uuid,
    event_type: &str,
    record: &ProcessRecord,
    cleanup: &ProcessCleanupReport,
) -> Vec<CommandOutput> {
    let mut value = status_value(event_type, record, None);
    if let Some(object) = value.as_object_mut() {
        object.insert("cleanup".to_string(), serde_json::json!(cleanup));
    }
    vec![CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&value).unwrap_or_default(),
        exit_code: Some(0),
        done: true,
    }]
}

fn status_value(
    event_type: &str,
    record: &ProcessRecord,
    max_bytes: Option<u32>,
) -> serde_json::Value {
    let mut value = process_record_status_value(record);
    if let Some(object) = value.as_object_mut() {
        object.insert("type".to_string(), serde_json::json!(event_type));
        if let Some(max_bytes) = max_bytes {
            object.insert("max_bytes".to_string(), serde_json::json!(max_bytes));
        }
    }
    value
}

fn process_record_status_value(record: &ProcessRecord) -> serde_json::Value {
    serde_json::json!({
        "name": record.name,
        "pid": record.pid,
        "process_group_id": record.process_group_id.unwrap_or(record.pid),
        "status": record.status,
        "exit_code": record.exit_code,
        "started_unix": record.started_unix,
        "stdout_log": record.stdout_log,
        "stderr_log": record.stderr_log,
        "policy": record.policy,
        "limits": record.limits,
        "restart_attempts": record.restart_attempts,
        "last_exit_code": record.last_exit_code,
        "last_exit_unix": record.last_exit_unix,
        "last_restart_unix": record.last_restart_unix,
        "cgroup_path": record.cgroup_path,
        "cgroup_status": cgroup_status(record.cgroup_path.as_deref()),
        "limit_effectiveness": limit_effectiveness(&record.limits, &record.limit_evidence),
    })
}

fn json_stdout_outputs(
    job_id: uuid::Uuid,
    status_type: &str,
    value: &serde_json::Value,
) -> Result<Vec<CommandOutput>> {
    Ok(vec![
        CommandOutput {
            job_id,
            stream: OutputStream::Stdout,
            data: serde_json::to_vec(value)?,
            exit_code: None,
            done: false,
        },
        CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&serde_json::json!({ "type": status_type }))?,
            exit_code: Some(0),
            done: true,
        },
    ])
}

fn push_tail_output(
    job_id: uuid::Uuid,
    stream: OutputStream,
    path: &Path,
    max_bytes: u32,
    outputs: &mut Vec<CommandOutput>,
) -> Result<()> {
    let data = tail_file(path, max_bytes as usize)?;
    if !data.is_empty() {
        outputs.push(CommandOutput {
            job_id,
            stream,
            data,
            exit_code: None,
            done: false,
        });
    }
    Ok(())
}

fn tail_file(path: &Path, max_bytes: usize) -> Result<Vec<u8>> {
    let data = fs::read(path).unwrap_or_default();
    if data.len() <= max_bytes {
        Ok(data)
    } else {
        Ok(data[data.len() - max_bytes..].to_vec())
    }
}

fn apply_process_limits_in_child(limits: &ProcessResourceLimits) -> io::Result<()> {
    if let Some(value) = limits.memory_max_bytes {
        set_rlimit(libc::RLIMIT_AS as RlimitResource, value)?;
    }
    if let Some(value) = limits.pids_max {
        set_rlimit(libc::RLIMIT_NPROC as RlimitResource, u64::from(value))?;
    }
    if let Some(value) = limits.open_files_max {
        set_rlimit(libc::RLIMIT_NOFILE as RlimitResource, value)?;
    }
    if limits.no_new_privileges {
        let rc = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(target_env = "gnu")]
type RlimitResource = libc::__rlimit_resource_t;
#[cfg(not(target_env = "gnu"))]
type RlimitResource = libc::c_int;

fn set_rlimit(resource: RlimitResource, value: u64) -> io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: value as libc::rlim_t,
        rlim_max: value as libc::rlim_t,
    };
    let rc = unsafe { libc::setrlimit(resource, &limit) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn load_all_records(root: &Path) -> Result<Vec<ProcessRecord>> {
    let mut records = Vec::new();
    let dir = records_dir(root);
    if !dir.exists() {
        return Ok(records);
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.path().extension().and_then(|ext| ext.to_str()) == Some("json") {
            records.push(serde_json::from_slice(&fs::read(entry.path())?)?);
        }
    }
    records.sort_by(|left: &ProcessRecord, right| left.name.cmp(&right.name));
    Ok(records)
}

fn load_record(root: &Path, name: &str) -> Result<Option<ProcessRecord>> {
    let path = record_path(root, name);
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_slice(&fs::read(path)?)?))
}

fn save_record(root: &Path, record: &ProcessRecord) -> Result<()> {
    let path = record_path(root, &record.name);
    fs::write(path, serde_json::to_vec_pretty(record)?)?;
    Ok(())
}

fn record_path(root: &Path, name: &str) -> PathBuf {
    records_dir(root).join(format!("{name}.json"))
}

fn records_dir(root: &Path) -> PathBuf {
    root.join("records")
}

fn logs_dir(root: &Path) -> PathBuf {
    root.join("logs")
}

fn supervisor_root() -> PathBuf {
    std::env::var("VPSMAN_SUPERVISOR_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_STATE_ROOT))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
