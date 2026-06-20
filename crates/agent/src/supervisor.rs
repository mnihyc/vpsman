use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use vpsman_common::{
    create_private_file_new, ensure_private_dir, ensure_private_dir_tree, open_private_file_append,
    open_private_file_read, open_private_file_read_write, CommandOutput, JobCommand, OutputStream,
    ProcessResourceLimits, ProcessRestartPolicy, ProcessRunPolicy,
};

use crate::process_cleanup::{
    process_is_running, process_start_time_ticks, process_state, set_current_process_group,
    terminate_process_blocking_before, terminate_process_group_blocking_before,
    ProcessCleanupReport,
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
const SUPERVISOR_LOG_ROTATE_BYTES: u64 = 8 * 1024 * 1024;
const SUPERVISOR_LOG_ROTATE_FILES: usize = 3;
const MONITOR_IDLE_SLEEP_SECS: u64 = 1;

static PROCESS_MONITORS: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ProcessIdentity {
    start_time_ticks: u64,
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    process_identity: Option<ProcessIdentity>,
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
    let timeout = Duration::from_secs(timeout_secs.clamp(1, 60));
    let deadline = Instant::now() + timeout;
    execute_at_root(job_id, command, &root, deadline).await
}

pub(crate) async fn reconcile_supervised_processes_on_start() -> Result<serde_json::Value> {
    let root = supervisor_root();
    tokio::task::spawn_blocking(move || reconcile_supervisor_records_at_root(&root)).await?
}

pub(crate) fn reconcile_supervisor_records_at_root(root: &Path) -> Result<serde_json::Value> {
    ensure_supervisor_dirs(root)?;
    rotate_supervisor_logs(root)?;
    let mut records = Vec::new();
    let mut running = 0_u64;
    let mut restarted = 0_u64;
    let mut restart_pending = 0_u64;
    let mut stopped = 0_u64;
    let mut failed = 0_u64;
    let mut no_retries_remaining = 0_u64;
    for record in load_all_records(root)? {
        let before_pid = record.pid;
        let before_restart_attempts = record.restart_attempts;
        let record = reconcile_and_save_record(root, record)?;
        match record.status.as_str() {
            "running" => running += 1,
            "restart_pending" => restart_pending += 1,
            "stopped" | "stop_requested" => stopped += 1,
            "failed" | "exited" => failed += 1,
            "failed_no_retries_remaining" | "exited_no_retries_remaining" => {
                no_retries_remaining += 1
            }
            _ => failed += 1,
        }
        let was_restarted =
            record.pid != before_pid || record.restart_attempts > before_restart_attempts;
        if was_restarted {
            restarted += 1;
        }
        records.push(serde_json::json!({
            "name": record.name,
            "status": record.status,
            "pid": record.pid,
            "restart_policy": record.policy.restart,
            "restart_attempts": record.restart_attempts,
            "restarted": was_restarted,
        }));
    }
    Ok(serde_json::json!({
        "type": "process_supervisor_startup_reconcile",
        "status": "completed",
        "root": root.display().to_string(),
        "total": records.len(),
        "running": running,
        "restarted": restarted,
        "restart_pending": restart_pending,
        "stopped": stopped,
        "failed": failed,
        "no_retries_remaining": no_retries_remaining,
        "processes": records,
    }))
}

async fn execute_at_root(
    job_id: uuid::Uuid,
    command: &JobCommand,
    root: &Path,
    deadline: Instant,
) -> Result<Vec<CommandOutput>> {
    tokio::task::spawn_blocking({
        let command = command.clone();
        let root = root.to_path_buf();
        move || execute_blocking_with_deadline(job_id, &command, &root, deadline)
    })
    .await?
}

#[cfg(test)]
pub(crate) fn execute_blocking(
    job_id: uuid::Uuid,
    command: &JobCommand,
    root: &Path,
) -> Result<Vec<CommandOutput>> {
    execute_blocking_with_deadline(
        job_id,
        command,
        root,
        Instant::now() + Duration::from_secs(60),
    )
}

pub(crate) fn execute_blocking_with_deadline(
    job_id: uuid::Uuid,
    command: &JobCommand,
    root: &Path,
    deadline: Instant,
) -> Result<Vec<CommandOutput>> {
    ensure_supervisor_dirs(root)?;
    rotate_supervisor_logs(root)?;
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
                if process_identity_status(&record) == RecordIdentityStatus::Verified
                    && process_is_running(record.pid)
                {
                    anyhow::bail!("process {name} is already running with pid {}", record.pid);
                }
            }
            ensure_supervisor_deadline(deadline)?;
            let record = start_process(root, name, argv, cwd, env, policy, limits)?;
            save_newly_started_record(root, &record, deadline)?;
            ensure_restart_monitor(root, name);
            Ok(status_outputs(job_id, "process_start", &record))
        }
        JobCommand::ProcessStop { name } => {
            validate_process_name(name)?;
            let mut record =
                load_record(root, name)?.context("process is not managed by vpsman")?;
            ensure_supervisor_deadline(deadline)?;
            let cleanup = stop_record(&record, deadline);
            apply_stop_observation(&mut record);
            ensure_supervisor_deadline(deadline)?;
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
            ensure_supervisor_deadline(deadline)?;
            let cleanup = stop_record(&record, deadline);
            apply_stop_observation(&mut record);
            if record.status == "stale" {
                anyhow::bail!("process record identity is stale; refusing restart");
            }
            if record.status == "stop_requested" {
                anyhow::bail!("process {name} did not stop before restart deadline");
            }
            ensure_supervisor_deadline(deadline)?;
            record = start_process(
                root,
                &record.name,
                &record.argv,
                &record.cwd,
                &record.env,
                &record.policy,
                &record.limits,
            )?;
            ensure_supervisor_deadline(deadline)?;
            save_newly_started_record(root, &record, deadline)?;
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
                    .map(observe_record)
                    .collect::<Vec<_>>()
            } else {
                load_all_records(root)?
                    .into_iter()
                    .map(observe_record)
                    .collect::<Vec<_>>()
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
            let refreshed = observe_record(record);
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
    rotate_log_file(&stdout_log)?;
    rotate_log_file(&stderr_log)?;
    ensure_supervisor_dirs(root)?;
    let stdout = open_private_file_append(&stdout_log)
        .with_context(|| format!("failed to open {}", stdout_log.display()))?;
    let stderr = open_private_file_append(&stderr_log)
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
    let process_identity = Some(capture_process_identity(child.id())?);
    let mut record = ProcessRecord {
        name: name.to_string(),
        argv: argv.to_vec(),
        cwd: cwd.clone(),
        env: env.clone(),
        policy: policy.clone(),
        limits: limits.clone(),
        pid: child.id(),
        process_group_id: Some(child.id()),
        process_identity,
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

fn stop_record(record: &ProcessRecord, deadline: Instant) -> ProcessCleanupReport {
    match process_identity_status(record) {
        RecordIdentityStatus::Verified => {}
        RecordIdentityStatus::NotRunning => return completed_cleanup_report(record),
        RecordIdentityStatus::Missing | RecordIdentityStatus::Mismatched => {
            return refused_cleanup_report(record)
        }
    }
    if collect_child_exit_code(record.pid).is_some() || !process_is_running(record.pid) {
        return completed_cleanup_report(record);
    }
    let graceful_wait = Duration::from_secs(record.policy.graceful_stop_secs.clamp(1, 300));
    let process_group_id = record.process_group_id.unwrap_or(record.pid) as libc::pid_t;
    let mut group_report =
        terminate_process_group_blocking_before(process_group_id, graceful_wait, deadline);
    if collect_child_exit_code(record.pid).is_some()
        || process_identity_status(record) == RecordIdentityStatus::NotRunning
        || !process_is_running(record.pid)
    {
        group_report.final_running = false;
        return group_report;
    }
    if Instant::now() >= deadline {
        return group_report;
    }
    let mut fallback =
        terminate_process_blocking_before(record.pid as libc::pid_t, graceful_wait, deadline);
    fallback.fallback_used = true;
    fallback
}

fn completed_cleanup_report(record: &ProcessRecord) -> ProcessCleanupReport {
    ProcessCleanupReport {
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
    }
}

fn refused_cleanup_report(record: &ProcessRecord) -> ProcessCleanupReport {
    let mut report = ProcessCleanupReport {
        target_kind: "process",
        target_id: record.pid as libc::pid_t,
        graceful_signal: "SIGTERM",
        graceful_wait_ms: 0,
        graceful_signal_sent: false,
        forced_signal: None,
        forced_signal_sent: false,
        exited_after_grace: false,
        final_running: process_is_running(record.pid),
        fallback_used: false,
        errors: Vec::new(),
    };
    report
        .errors
        .push("process identity is not verified; refusing to signal".to_string());
    report
}

fn capture_process_identity(pid: u32) -> Result<ProcessIdentity> {
    Ok(ProcessIdentity {
        start_time_ticks: process_start_time_ticks(pid)
            .with_context(|| format!("failed to capture process identity for pid {pid}"))?,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecordIdentityStatus {
    Verified,
    NotRunning,
    Missing,
    Mismatched,
}

fn process_identity_status(record: &ProcessRecord) -> RecordIdentityStatus {
    let Some(identity) = &record.process_identity else {
        return RecordIdentityStatus::Missing;
    };
    match process_start_time_ticks(record.pid) {
        Ok(start_time_ticks) if start_time_ticks == identity.start_time_ticks => {
            RecordIdentityStatus::Verified
        }
        Ok(_) => RecordIdentityStatus::Mismatched,
        Err(error) if error.kind() == io::ErrorKind::NotFound => RecordIdentityStatus::NotRunning,
        Err(_) => RecordIdentityStatus::Mismatched,
    }
}

fn process_identity_status_label(status: RecordIdentityStatus) -> &'static str {
    match status {
        RecordIdentityStatus::Verified => "verified",
        RecordIdentityStatus::NotRunning => "not_running",
        RecordIdentityStatus::Missing => "missing",
        RecordIdentityStatus::Mismatched => "mismatched",
    }
}

fn observe_record(mut record: ProcessRecord) -> ProcessRecord {
    if matches!(record.status.as_str(), "stopped" | "stop_requested") {
        return record;
    }
    match process_identity_status(&record) {
        RecordIdentityStatus::Verified => {
            if process_state(record.pid).is_ok_and(|state| state == 'Z') {
                mark_record_exited(&mut record, None);
            } else if process_is_running(record.pid) {
                record.status = "running".to_string();
                record.exit_code = None;
            } else if matches!(record.status.as_str(), "running" | "restart_pending")
                && record.last_exit_unix.is_none()
            {
                mark_record_exited(&mut record, None);
            }
        }
        RecordIdentityStatus::NotRunning => {
            if matches!(record.status.as_str(), "running" | "restart_pending")
                && record.last_exit_unix.is_none()
            {
                mark_record_exited(&mut record, None);
            }
        }
        RecordIdentityStatus::Missing | RecordIdentityStatus::Mismatched => {
            mark_record_stale(&mut record);
        }
    }
    record
}

fn apply_stop_observation(record: &mut ProcessRecord) {
    match process_identity_status(record) {
        RecordIdentityStatus::Verified => {
            if process_is_running(record.pid) {
                record.status = "stop_requested".to_string();
                record.exit_code = None;
            } else {
                record.status = "stopped".to_string();
                record.exit_code = Some(0);
            }
        }
        RecordIdentityStatus::NotRunning => {
            record.status = "stopped".to_string();
            record.exit_code = Some(0);
        }
        RecordIdentityStatus::Missing | RecordIdentityStatus::Mismatched => {
            mark_record_stale(record);
        }
    }
}

fn mark_record_stale(record: &mut ProcessRecord) {
    record.status = "stale".to_string();
    record.exit_code = None;
}

fn ensure_supervisor_deadline(deadline: Instant) -> Result<()> {
    if Instant::now() >= deadline {
        anyhow::bail!("process supervisor command timed out");
    }
    Ok(())
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
    match process_identity_status(&record) {
        RecordIdentityStatus::Verified => {
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
        }
        RecordIdentityStatus::NotRunning => {
            if matches!(record.status.as_str(), "running" | "restart_pending")
                && record.last_exit_unix.is_none()
            {
                mark_record_exited(&mut record, None);
            }
        }
        RecordIdentityStatus::Missing | RecordIdentityStatus::Mismatched => {
            mark_record_stale(&mut record);
            return Ok(record);
        }
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
    save_newly_started_record(
        root,
        &restarted,
        Instant::now() + Duration::from_secs(record.policy.graceful_stop_secs.clamp(1, 300)),
    )?;
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
    if !matches!(record.status.as_str(), "running" | "restart_pending") {
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
        if rotate_supervisor_logs(root).is_err() {
            break;
        }
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
    let identity_status = process_identity_status(record);
    serde_json::json!({
        "name": record.name,
        "pid": record.pid,
        "process_group_id": record.process_group_id.unwrap_or(record.pid),
        "process_identity_status": process_identity_status_label(identity_status),
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
    let mut file = match open_private_file_read(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to open {}", path.display()))
        }
    };
    let file_len = file
        .metadata()
        .with_context(|| format!("failed to stat {}", path.display()))?
        .len();
    let read_len = file_len.min(max_bytes as u64);
    file.seek(SeekFrom::Start(file_len.saturating_sub(read_len)))
        .with_context(|| format!("failed to seek {}", path.display()))?;
    let mut data = Vec::with_capacity(read_len as usize);
    file.take(read_len)
        .read_to_end(&mut data)
        .with_context(|| format!("failed to read log tail {}", path.display()))?;
    Ok(data)
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
            records.push(read_process_record_file(&entry.path())?);
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
    Ok(Some(read_process_record_file(&path)?))
}

fn save_record(root: &Path, record: &ProcessRecord) -> Result<()> {
    let path = record_path(root, &record.name);
    let parent = records_dir(root);
    ensure_private_dir_tree(root, &parent)?;
    let tmp_path = path.with_file_name(format!(
        "{}.json.tmp.{}.{}",
        record.name,
        std::process::id(),
        unique_time_suffix()
    ));
    let bytes = serde_json::to_vec_pretty(record)?;
    {
        let mut file = create_private_file_new(&tmp_path)
            .with_context(|| format!("failed to create {}", tmp_path.display()))?;
        file.write_all(&bytes)
            .with_context(|| format!("failed to write {}", tmp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to fsync {}", tmp_path.display()))?;
    }
    if let Err(error) = fs::rename(&tmp_path, &path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(error)
            .with_context(|| format!("failed to replace process record {}", path.display()));
    }
    sync_parent_dir(&parent)?;
    Ok(())
}

fn save_newly_started_record(root: &Path, record: &ProcessRecord, deadline: Instant) -> Result<()> {
    if let Err(error) = save_record(root, record) {
        let _ = stop_record(record, deadline);
        return Err(error);
    }
    Ok(())
}

fn rotate_supervisor_logs(root: &Path) -> Result<()> {
    for record in load_all_records(root)? {
        rotate_log_file(Path::new(&record.stdout_log))?;
        rotate_log_file(Path::new(&record.stderr_log))?;
    }
    Ok(())
}

fn rotate_log_file(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to stat {}", path.display()))
        }
    };
    anyhow::ensure!(
        !metadata.file_type().is_symlink() && metadata.is_file(),
        "supervisor log {} is not a regular file",
        path.display()
    );
    let _ = open_private_file_read(path)
        .with_context(|| format!("failed to secure {}", path.display()))?;
    let metadata =
        fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
    if metadata.len() <= SUPERVISOR_LOG_ROTATE_BYTES {
        return Ok(());
    }

    for index in (1..=SUPERVISOR_LOG_ROTATE_FILES).rev() {
        let source = rotated_log_path(path, index);
        let target = rotated_log_path(path, index + 1);
        if index == SUPERVISOR_LOG_ROTATE_FILES {
            let _ = fs::remove_file(&source);
        } else if source.exists() {
            let _ = open_private_file_read(&source)
                .with_context(|| format!("failed to secure {}", source.display()))?;
            if target.exists() {
                fs::remove_file(&target).with_context(|| {
                    format!("failed to remove old rotated log {}", target.display())
                })?;
            }
            fs::rename(&source, &target).with_context(|| {
                format!(
                    "failed to rotate supervisor log {} to {}",
                    source.display(),
                    target.display()
                )
            })?;
        }
    }

    let mut file = open_private_file_read_write(path, false)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let file_len = file.metadata()?.len();
    let keep_bytes = SUPERVISOR_LOG_ROTATE_BYTES.min(file_len);
    file.seek(SeekFrom::Start(file_len.saturating_sub(keep_bytes)))?;
    let mut tail = Vec::with_capacity(usize::try_from(keep_bytes).unwrap_or(0));
    file.read_to_end(&mut tail)
        .with_context(|| format!("failed to read {}", path.display()))?;

    let first_rotation = rotated_log_path(path, 1);
    let tmp_rotation = first_rotation.with_extension(format!(
        "{}.tmp.{}.{}",
        first_rotation
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("log"),
        std::process::id(),
        unique_time_suffix()
    ));
    {
        let mut rotated = create_private_file_new(&tmp_rotation)
            .with_context(|| format!("failed to create {}", tmp_rotation.display()))?;
        rotated
            .write_all(&tail)
            .with_context(|| format!("failed to write {}", tmp_rotation.display()))?;
        rotated
            .sync_all()
            .with_context(|| format!("failed to fsync {}", tmp_rotation.display()))?;
    }
    fs::rename(&tmp_rotation, &first_rotation).with_context(|| {
        format!(
            "failed to install rotated supervisor log {}",
            first_rotation.display()
        )
    })?;

    file.set_len(0)
        .with_context(|| format!("failed to truncate {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to fsync {}", path.display()))?;
    if let Some(parent) = path.parent() {
        sync_parent_dir(parent)?;
    }
    Ok(())
}

fn rotated_log_path(path: &Path, index: usize) -> PathBuf {
    PathBuf::from(format!("{}.{}", path.to_string_lossy(), index))
}

fn ensure_supervisor_dirs(root: &Path) -> Result<()> {
    ensure_private_dir(root)
        .with_context(|| format!("failed to create supervisor root {}", root.display()))?;
    ensure_private_dir_tree(root, &records_dir(root)).with_context(|| {
        format!(
            "failed to create supervisor records dir {}",
            records_dir(root).display()
        )
    })?;
    ensure_private_dir_tree(root, &logs_dir(root)).with_context(|| {
        format!(
            "failed to create supervisor logs dir {}",
            logs_dir(root).display()
        )
    })?;
    Ok(())
}

fn read_process_record_file(path: &Path) -> Result<ProcessRecord> {
    let mut file = open_private_file_read(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to decode process record {}", path.display()))
}

fn sync_parent_dir(path: &Path) -> Result<()> {
    let dir = OpenOptions::new()
        .read(true)
        .open(path)
        .with_context(|| format!("failed to open directory {}", path.display()))?;
    dir.sync_all()
        .with_context(|| format!("failed to fsync directory {}", path.display()))
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

fn unique_time_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn tail_file_reads_only_requested_suffix() {
        let path = std::env::temp_dir().join(format!(
            "vpsman-supervisor-tail-{}.log",
            uuid::Uuid::new_v4()
        ));
        let data = (0..4096)
            .map(|value| b'a' + (value % 26) as u8)
            .collect::<Vec<_>>();
        fs::write(&path, &data).unwrap();

        let tail = tail_file(&path, 37).unwrap();

        assert_eq!(tail, data[data.len() - 37..]);
        assert_eq!(mode(&path), 0o600);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn process_records_are_written_private() {
        let root = temp_supervisor_root("record");
        let record = test_record(&root, "private-record", 12345);

        save_record(&root, &record).unwrap();

        assert_eq!(mode(&root), 0o700);
        assert_eq!(mode(&records_dir(&root)), 0o700);
        assert_eq!(mode(&record_path(&root, "private-record")), 0o600);
        let loaded = load_record(&root, "private-record").unwrap().unwrap();
        assert_eq!(loaded.name, "private-record");
        assert_eq!(mode(&record_path(&root, "private-record")), 0o600);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn process_logs_are_created_private() {
        let root = temp_supervisor_root("logs");
        let argv = vec!["/bin/true".to_string()];
        let record = start_process(
            &root,
            "private-logs",
            &argv,
            &None,
            &BTreeMap::new(),
            &ProcessRunPolicy::default(),
            &ProcessResourceLimits::default(),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let _ = collect_child_exit_code(record.pid);

        assert_eq!(mode(&root), 0o700);
        assert_eq!(mode(&logs_dir(&root)), 0o700);
        assert_eq!(mode(Path::new(&record.stdout_log)), 0o600);
        assert_eq!(mode(Path::new(&record.stderr_log)), 0o600);
        let _ = fs::remove_dir_all(root);
    }

    fn test_record(root: &Path, name: &str, pid: u32) -> ProcessRecord {
        ProcessRecord {
            name: name.to_string(),
            argv: vec!["/bin/true".to_string()],
            cwd: None,
            env: BTreeMap::new(),
            policy: ProcessRunPolicy::default(),
            limits: ProcessResourceLimits::default(),
            pid,
            process_group_id: Some(pid),
            process_identity: None,
            started_unix: 1,
            stdout_log: logs_dir(root)
                .join(format!("{name}.stdout.log"))
                .to_string_lossy()
                .to_string(),
            stderr_log: logs_dir(root)
                .join(format!("{name}.stderr.log"))
                .to_string_lossy()
                .to_string(),
            status: "running".to_string(),
            exit_code: None,
            restart_attempts: 0,
            last_exit_code: None,
            last_exit_unix: None,
            last_restart_unix: None,
            cgroup_path: None,
            limit_evidence: ProcessLimitEvidence::default(),
        }
    }

    fn temp_supervisor_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "vpsman-supervisor-{label}-{}",
            uuid::Uuid::new_v4()
        ))
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }
}
