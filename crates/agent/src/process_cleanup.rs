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

pub(crate) fn terminate_process_group_blocking_before(
    process_group_id: libc::pid_t,
    graceful_wait: Duration,
    deadline: std::time::Instant,
) -> ProcessCleanupReport {
    terminate_blocking_before(
        "process_group",
        process_group_id,
        -process_group_id,
        graceful_wait,
        deadline,
    )
}

pub(crate) fn terminate_process_blocking(
    pid: libc::pid_t,
    graceful_wait: Duration,
) -> ProcessCleanupReport {
    terminate_blocking("process", pid, pid, graceful_wait)
}

pub(crate) fn terminate_process_blocking_before(
    pid: libc::pid_t,
    graceful_wait: Duration,
    deadline: std::time::Instant,
) -> ProcessCleanupReport {
    terminate_blocking_before("process", pid, pid, graceful_wait, deadline)
}

pub(crate) fn process_is_running(pid: u32) -> bool {
    signal(pid as libc::pid_t, 0).is_ok()
}

pub(crate) fn process_start_time_ticks(pid: u32) -> io::Result<u64> {
    let stat_path = format!("/proc/{pid}/stat");
    let stat = std::fs::read_to_string(stat_path)?;
    parse_proc_stat_start_time_ticks(&stat)
}

pub(crate) fn process_state(pid: u32) -> io::Result<char> {
    let stat_path = format!("/proc/{pid}/stat");
    let stat = std::fs::read_to_string(stat_path)?;
    parse_proc_stat_state(&stat)
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

fn terminate_blocking_before(
    target_kind: &'static str,
    target_id: libc::pid_t,
    signal_target: libc::pid_t,
    requested_graceful_wait: Duration,
    deadline: std::time::Instant,
) -> ProcessCleanupReport {
    let graceful_wait = graceful_wait_before_deadline(requested_graceful_wait, deadline);
    let mut report =
        ProcessCleanupReport::new(target_kind, target_id, duration_millis_u64(graceful_wait));
    if std::time::Instant::now() >= deadline {
        report.final_running = signal(signal_target, 0).is_ok();
        report
            .errors
            .push("cleanup deadline reached before SIGTERM".to_string());
        return report;
    }
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
    if std::time::Instant::now() >= deadline {
        report.final_running = signal(signal_target, 0).is_ok();
        report
            .errors
            .push("cleanup deadline reached before SIGKILL".to_string());
        return report;
    }
    report.forced_signal = Some("SIGKILL");
    match signal(signal_target, libc::SIGKILL) {
        Ok(()) => report.forced_signal_sent = true,
        Err(error) => report
            .errors
            .push(format!("SIGKILL failed: {}", error.kind())),
    }
    let forced_wait = deadline
        .saturating_duration_since(std::time::Instant::now())
        .min(Duration::from_millis(500));
    report.final_running = !wait_until_not_running(signal_target, forced_wait);
    report
}

fn graceful_wait_before_deadline(requested: Duration, deadline: std::time::Instant) -> Duration {
    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
    let force_reserve = Duration::from_millis(500);
    if remaining > force_reserve {
        requested.min(remaining - force_reserve)
    } else {
        Duration::ZERO
    }
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

fn parse_proc_stat_start_time_ticks(stat: &str) -> io::Result<u64> {
    let after_comm = stat.rfind(") ").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "proc stat missing command field",
        )
    })?;
    stat[after_comm + 2..]
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "proc stat missing starttime"))?
        .parse::<u64>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid proc stat starttime"))
}

fn parse_proc_stat_state(stat: &str) -> io::Result<char> {
    let after_comm = stat.rfind(") ").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "proc stat missing command field",
        )
    })?;
    stat[after_comm + 2..]
        .split_whitespace()
        .next()
        .and_then(|state| state.chars().next())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "proc stat missing state"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_stat_start_time_with_spaces_in_comm() {
        let stat =
            "123 (name with spaces) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 987654 20";

        assert_eq!(parse_proc_stat_start_time_ticks(stat).unwrap(), 987654);
        assert_eq!(parse_proc_stat_state(stat).unwrap(), 'S');
    }
}
