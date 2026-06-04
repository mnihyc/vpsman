use std::{io, time::Duration};

use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ProcessCleanupReport {
    pub(crate) target_kind: &'static str,
    pub(crate) target_id: i32,
    pub(crate) graceful_signal: &'static str,
    pub(crate) graceful_wait_ms: u64,
    pub(crate) graceful_signal_sent: bool,
    pub(crate) forced_signal: Option<&'static str>,
    pub(crate) forced_signal_sent: bool,
    pub(crate) exited_after_grace: bool,
    pub(crate) final_running: bool,
    pub(crate) fallback_used: bool,
    pub(crate) errors: Vec<String>,
}

impl ProcessCleanupReport {
    fn new(target_kind: &'static str, target_id: i32, graceful_wait_ms: u64) -> Self {
        Self {
            target_kind,
            target_id,
            graceful_signal: "SIGTERM",
            graceful_wait_ms,
            graceful_signal_sent: false,
            forced_signal: None,
            forced_signal_sent: false,
            exited_after_grace: false,
            final_running: false,
            fallback_used: false,
            errors: Vec::new(),
        }
    }
}

pub(crate) fn terminate_process_group_blocking(
    process_group_id: libc::pid_t,
    graceful_wait: Duration,
) -> ProcessCleanupReport {
    terminate_blocking(
        "process_group",
        process_group_id,
        -process_group_id,
        graceful_wait,
    )
}

pub(crate) fn terminate_process_blocking(
    pid: libc::pid_t,
    graceful_wait: Duration,
) -> ProcessCleanupReport {
    terminate_blocking("process", pid, pid, graceful_wait)
}

pub(crate) fn process_is_running(pid: u32) -> bool {
    signal(pid as libc::pid_t, 0).is_ok()
}

pub(crate) fn set_current_process_group() -> io::Result<()> {
    let result = unsafe { libc::setpgid(0, 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(crate) fn signal_process_group(
    process_group_id: libc::pid_t,
    signal_number: i32,
) -> io::Result<()> {
    signal(-process_group_id, signal_number)
}

fn terminate_blocking(
    target_kind: &'static str,
    target_id: libc::pid_t,
    signal_target: libc::pid_t,
    graceful_wait: Duration,
) -> ProcessCleanupReport {
    let mut report =
        ProcessCleanupReport::new(target_kind, target_id, duration_millis_u64(graceful_wait));
    match signal(signal_target, libc::SIGTERM) {
        Ok(()) => report.graceful_signal_sent = true,
        Err(error) => report
            .errors
            .push(format!("SIGTERM failed: {}", error.kind())),
    }
    if wait_until_not_running(signal_target, graceful_wait) {
        report.exited_after_grace = true;
        report.final_running = false;
        return report;
    }
    report.forced_signal = Some("SIGKILL");
    match signal(signal_target, libc::SIGKILL) {
        Ok(()) => report.forced_signal_sent = true,
        Err(error) => report
            .errors
            .push(format!("SIGKILL failed: {}", error.kind())),
    }
    report.final_running = !wait_until_not_running(signal_target, Duration::from_secs(2));
    report
}

fn wait_until_not_running(signal_target: libc::pid_t, wait: Duration) -> bool {
    let deadline = std::time::Instant::now() + wait;
    loop {
        if signal(signal_target, 0).is_err() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn signal(target: libc::pid_t, signal_number: i32) -> io::Result<()> {
    let result = unsafe { libc::kill(target, signal_number) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn duration_millis_u64(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}
