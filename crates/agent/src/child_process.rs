use std::{
    fs::File,
    io,
    os::fd::{AsRawFd, FromRawFd, RawFd},
    process::{ExitStatus, Stdio},
    time::Duration,
};

use anyhow::{Context, Result};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Child,
    sync::mpsc,
    time::{self, MissedTickBehavior},
};
use vpsman_common::{CommandOutput, OutputStream};

use crate::{
    command_worker::CommandCancelToken,
    process_cleanup::{signal_process_group, terminate_process_blocking, ProcessCleanupReport},
};

const STREAM_OUTPUT_CHUNK_BYTES: usize = 32 * 1024;
const STREAM_OUTPUT_FLUSH_MS: u64 = 200;
const CHILD_TERMINATION_GRACE_MS: u64 = 500;

pub(crate) enum ChildRunResult {
    Completed(ChildRunOutput),
    TimedOut(ProcessCleanupReport),
    Canceled {
        cleanup: ProcessCleanupReport,
        reason: String,
    },
}

pub(crate) struct ChildRunOutput {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) exit_code: Option<i32>,
    pub(crate) stdout_truncated: bool,
    pub(crate) stderr_truncated: bool,
    pub(crate) pty_truncated: bool,
}

struct BoundedReadOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

#[derive(Clone)]
pub(crate) struct ChildOutputSink {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) sender: mpsc::Sender<CommandOutput>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ChildCleanupPolicy {
    ProcessGroup,
    DirectChild,
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn run_child_with_bounded_output(
    command: tokio::process::Command,
    timeout_secs: u64,
    max_output_bytes: usize,
    cleanup_policy: ChildCleanupPolicy,
) -> Result<ChildRunResult> {
    run_child(
        command,
        timeout_secs,
        max_output_bytes,
        cleanup_policy,
        None,
        None,
    )
    .await
}

pub(crate) async fn run_child_with_bounded_output_cancelable(
    command: tokio::process::Command,
    timeout_secs: u64,
    max_output_bytes: usize,
    cleanup_policy: ChildCleanupPolicy,
    cancel_token: CommandCancelToken,
) -> Result<ChildRunResult> {
    run_child(
        command,
        timeout_secs,
        max_output_bytes,
        cleanup_policy,
        Some(cancel_token),
        None,
    )
    .await
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn run_child_with_streaming_output(
    command: tokio::process::Command,
    timeout_secs: u64,
    max_output_bytes: usize,
    cleanup_policy: ChildCleanupPolicy,
    sink: ChildOutputSink,
) -> Result<ChildRunResult> {
    run_child(
        command,
        timeout_secs,
        max_output_bytes,
        cleanup_policy,
        None,
        Some(sink),
    )
    .await
}

pub(crate) async fn run_child_with_streaming_output_cancelable(
    command: tokio::process::Command,
    timeout_secs: u64,
    max_output_bytes: usize,
    cleanup_policy: ChildCleanupPolicy,
    sink: ChildOutputSink,
    cancel_token: CommandCancelToken,
) -> Result<ChildRunResult> {
    run_child(
        command,
        timeout_secs,
        max_output_bytes,
        cleanup_policy,
        Some(cancel_token),
        Some(sink),
    )
    .await
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn run_pty_with_bounded_output(
    command: tokio::process::Command,
    timeout_secs: u64,
    max_output_bytes: usize,
    cleanup_policy: ChildCleanupPolicy,
) -> Result<ChildRunResult> {
    run_pty(
        command,
        timeout_secs,
        max_output_bytes,
        cleanup_policy,
        None,
        None,
    )
    .await
}

pub(crate) async fn run_pty_with_bounded_output_cancelable(
    command: tokio::process::Command,
    timeout_secs: u64,
    max_output_bytes: usize,
    cleanup_policy: ChildCleanupPolicy,
    cancel_token: CommandCancelToken,
) -> Result<ChildRunResult> {
    run_pty(
        command,
        timeout_secs,
        max_output_bytes,
        cleanup_policy,
        Some(cancel_token),
        None,
    )
    .await
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn run_pty_with_streaming_output(
    command: tokio::process::Command,
    timeout_secs: u64,
    max_output_bytes: usize,
    cleanup_policy: ChildCleanupPolicy,
    sink: ChildOutputSink,
) -> Result<ChildRunResult> {
    run_pty(
        command,
        timeout_secs,
        max_output_bytes,
        cleanup_policy,
        None,
        Some(sink),
    )
    .await
}

pub(crate) async fn run_pty_with_streaming_output_cancelable(
    command: tokio::process::Command,
    timeout_secs: u64,
    max_output_bytes: usize,
    cleanup_policy: ChildCleanupPolicy,
    sink: ChildOutputSink,
    cancel_token: CommandCancelToken,
) -> Result<ChildRunResult> {
    run_pty(
        command,
        timeout_secs,
        max_output_bytes,
        cleanup_policy,
        Some(cancel_token),
        Some(sink),
    )
    .await
}

async fn run_child(
    mut command: tokio::process::Command,
    timeout_secs: u64,
    max_output_bytes: usize,
    cleanup_policy: ChildCleanupPolicy,
    cancel_token: Option<CommandCancelToken>,
    sink: Option<ChildOutputSink>,
) -> Result<ChildRunResult> {
    command.kill_on_drop(true);
    if cleanup_policy == ChildCleanupPolicy::ProcessGroup {
        command.process_group(0);
    }
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let timeout_secs = timeout_secs.max(1);
    let mut child = RunningChild::spawn(command, cleanup_policy)?;
    let stdout = child.take_stdout();
    let stderr = child.take_stderr();
    let stdout_task = tokio::spawn(read_bounded_output_with_sink(
        stdout,
        max_output_bytes,
        sink.clone(),
        OutputStream::Stdout,
    ));
    let stderr_task = tokio::spawn(read_bounded_output_with_sink(
        stderr,
        max_output_bytes,
        sink,
        OutputStream::Stderr,
    ));

    let status = match wait_for_child(&mut child, timeout_secs, cancel_token).await? {
        ChildWaitOutcome::Completed(status) => status,
        ChildWaitOutcome::TimedOut => {
            let cleanup = child
                .terminate(Duration::from_millis(CHILD_TERMINATION_GRACE_MS))
                .await;
            stdout_task.abort();
            stderr_task.abort();
            return Ok(ChildRunResult::TimedOut(cleanup));
        }
        ChildWaitOutcome::Canceled(reason) => {
            let cleanup = child
                .terminate(Duration::from_millis(CHILD_TERMINATION_GRACE_MS))
                .await;
            stdout_task.abort();
            stderr_task.abort();
            return Ok(ChildRunResult::Canceled { cleanup, reason });
        }
    };
    child.disarm();
    let stdout = stdout_task
        .await
        .context("stdout reader task failed")?
        .context("failed to read stdout")?;
    let stderr = stderr_task
        .await
        .context("stderr reader task failed")?
        .context("failed to read stderr")?;

    Ok(ChildRunResult::Completed(ChildRunOutput {
        stdout: stdout.bytes,
        stderr: stderr.bytes,
        exit_code: status.code(),
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
        pty_truncated: false,
    }))
}

async fn run_pty(
    mut command: tokio::process::Command,
    timeout_secs: u64,
    max_output_bytes: usize,
    cleanup_policy: ChildCleanupPolicy,
    cancel_token: Option<CommandCancelToken>,
    sink: Option<ChildOutputSink>,
) -> Result<ChildRunResult> {
    command.kill_on_drop(true);
    if cleanup_policy == ChildCleanupPolicy::ProcessGroup {
        command.process_group(0);
    }
    let pty = open_pty_stdio().context("failed to open PTY")?;
    command.stdin(pty.stdin);
    command.stdout(pty.stdout);
    command.stderr(pty.stderr);

    let timeout_secs = timeout_secs.max(1);
    let mut child = RunningChild::spawn(command, cleanup_policy)?;
    let reader_task = tokio::spawn(read_bounded_pty_output_with_sink(
        tokio::fs::File::from_std(pty.master),
        max_output_bytes,
        sink,
    ));

    let status = match wait_for_child(&mut child, timeout_secs, cancel_token).await? {
        ChildWaitOutcome::Completed(status) => status,
        ChildWaitOutcome::TimedOut => {
            let cleanup = child
                .terminate(Duration::from_millis(CHILD_TERMINATION_GRACE_MS))
                .await;
            reader_task.abort();
            return Ok(ChildRunResult::TimedOut(cleanup));
        }
        ChildWaitOutcome::Canceled(reason) => {
            let cleanup = child
                .terminate(Duration::from_millis(CHILD_TERMINATION_GRACE_MS))
                .await;
            reader_task.abort();
            return Ok(ChildRunResult::Canceled { cleanup, reason });
        }
    };
    child.disarm();
    let output = reader_task
        .await
        .context("pty reader task failed")?
        .context("failed to read pty output")?;

    Ok(ChildRunResult::Completed(ChildRunOutput {
        stdout: output.bytes,
        stderr: Vec::new(),
        exit_code: status.code(),
        stdout_truncated: false,
        stderr_truncated: false,
        pty_truncated: output.truncated,
    }))
}

enum ChildWaitOutcome {
    Completed(ExitStatus),
    TimedOut,
    Canceled(String),
}

async fn wait_for_child(
    child: &mut RunningChild,
    timeout_secs: u64,
    cancel_token: Option<CommandCancelToken>,
) -> std::io::Result<ChildWaitOutcome> {
    let timeout = time::sleep(Duration::from_secs(timeout_secs));
    tokio::pin!(timeout);
    if let Some(cancel_token) = cancel_token {
        let canceled = cancel_token.cancelled();
        tokio::pin!(canceled);
        tokio::select! {
            biased;
            reason = &mut canceled => Ok(ChildWaitOutcome::Canceled(reason)),
            status = child.wait() => status.map(ChildWaitOutcome::Completed),
            _ = &mut timeout => Ok(ChildWaitOutcome::TimedOut),
        }
    } else {
        tokio::select! {
            status = child.wait() => status.map(ChildWaitOutcome::Completed),
            _ = &mut timeout => Ok(ChildWaitOutcome::TimedOut),
        }
    }
}

pub(crate) struct PtyStdio {
    pub(crate) master: File,
    pub(crate) stdin: Stdio,
    pub(crate) stdout: Stdio,
    pub(crate) stderr: Stdio,
}

pub(crate) fn open_pty_stdio() -> io::Result<PtyStdio> {
    let mut master_fd: RawFd = -1;
    let mut slave_fd: RawFd = -1;
    let opened = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if opened != 0 {
        return Err(io::Error::last_os_error());
    }

    let stdout_fd = match dup_fd(slave_fd) {
        Ok(fd) => fd,
        Err(error) => {
            close_fd(master_fd);
            close_fd(slave_fd);
            return Err(error);
        }
    };
    let stderr_fd = match dup_fd(slave_fd) {
        Ok(fd) => fd,
        Err(error) => {
            close_fd(master_fd);
            close_fd(slave_fd);
            close_fd(stdout_fd);
            return Err(error);
        }
    };

    unsafe {
        Ok(PtyStdio {
            master: File::from_raw_fd(master_fd),
            stdin: Stdio::from(File::from_raw_fd(slave_fd)),
            stdout: Stdio::from(File::from_raw_fd(stdout_fd)),
            stderr: Stdio::from(File::from_raw_fd(stderr_fd)),
        })
    }
}

pub(crate) fn set_pty_window_size(file: &impl AsRawFd, cols: u16, rows: u16) -> io::Result<()> {
    let winsize = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let resized = unsafe { libc::ioctl(file.as_raw_fd(), libc::TIOCSWINSZ, &winsize) };
    if resized < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn dup_fd(fd: RawFd) -> io::Result<RawFd> {
    let duplicated = unsafe { libc::dup(fd) };
    if duplicated < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(duplicated)
    }
}

fn close_fd(fd: RawFd) {
    if fd >= 0 {
        unsafe {
            libc::close(fd);
        }
    }
}

struct RunningChild {
    child: Child,
    pid: Option<libc::pid_t>,
    process_group_id: Option<libc::pid_t>,
    cleanup_policy: ChildCleanupPolicy,
}

impl RunningChild {
    fn spawn(
        mut command: tokio::process::Command,
        cleanup_policy: ChildCleanupPolicy,
    ) -> Result<Self> {
        let child = command.spawn().context("failed to spawn command")?;
        let pid = child.id().map(|pid| pid as libc::pid_t);
        let process_group_id = if cleanup_policy == ChildCleanupPolicy::ProcessGroup {
            pid
        } else {
            None
        };
        Ok(Self {
            child,
            pid,
            process_group_id,
            cleanup_policy,
        })
    }

    fn take_stdout(&mut self) -> tokio::process::ChildStdout {
        self.child
            .stdout
            .take()
            .expect("stdout is piped before command spawn")
    }

    fn take_stderr(&mut self) -> tokio::process::ChildStderr {
        self.child
            .stderr
            .take()
            .expect("stderr is piped before command spawn")
    }

    async fn wait(&mut self) -> std::io::Result<ExitStatus> {
        self.child.wait().await
    }

    fn disarm(&mut self) {
        self.pid = None;
        self.process_group_id = None;
    }

    async fn terminate(&mut self, graceful_wait: Duration) -> ProcessCleanupReport {
        match self.cleanup_policy {
            ChildCleanupPolicy::ProcessGroup => self.terminate_process_group(graceful_wait).await,
            ChildCleanupPolicy::DirectChild => self.terminate_direct_child(graceful_wait).await,
        }
    }

    async fn terminate_process_group(&mut self, graceful_wait: Duration) -> ProcessCleanupReport {
        self.pid = None;
        let Some(process_group_id) = self.process_group_id.take() else {
            return ProcessCleanupReport {
                target_kind: "process_group",
                target_id: 0,
                graceful_signal: "SIGTERM",
                graceful_wait_ms: graceful_wait.as_millis().try_into().unwrap_or(u64::MAX),
                graceful_signal_sent: false,
                forced_signal: None,
                forced_signal_sent: false,
                exited_after_grace: true,
                final_running: false,
                fallback_used: false,
                errors: vec!["process group was already disarmed".to_string()],
            };
        };
        let mut report = ProcessCleanupReport {
            target_kind: "process_group",
            target_id: process_group_id,
            graceful_signal: "SIGTERM",
            graceful_wait_ms: graceful_wait.as_millis().try_into().unwrap_or(u64::MAX),
            graceful_signal_sent: false,
            forced_signal: None,
            forced_signal_sent: false,
            exited_after_grace: false,
            final_running: false,
            fallback_used: false,
            errors: Vec::new(),
        };
        match signal_process_group(process_group_id, libc::SIGTERM) {
            Ok(()) => report.graceful_signal_sent = true,
            Err(error) => report
                .errors
                .push(format!("SIGTERM failed: {}", error.kind())),
        }
        match time::timeout(graceful_wait, self.child.wait()).await {
            Ok(Ok(_)) => {
                report.exited_after_grace = true;
                return report;
            }
            Ok(Err(error)) => report
                .errors
                .push(format!("wait after SIGTERM failed: {}", error.kind())),
            Err(_) => {}
        }
        report.forced_signal = Some("SIGKILL");
        match signal_process_group(process_group_id, libc::SIGKILL) {
            Ok(()) => report.forced_signal_sent = true,
            Err(error) => report
                .errors
                .push(format!("SIGKILL failed: {}", error.kind())),
        }
        report.final_running = match time::timeout(Duration::from_secs(2), self.child.wait()).await
        {
            Ok(Ok(_)) => false,
            Ok(Err(error)) => {
                report
                    .errors
                    .push(format!("wait after SIGKILL failed: {}", error.kind()));
                true
            }
            Err(_) => true,
        };
        report
    }

    async fn terminate_direct_child(&mut self, graceful_wait: Duration) -> ProcessCleanupReport {
        self.process_group_id = None;
        let Some(pid) = self.pid.take() else {
            return ProcessCleanupReport {
                target_kind: "process",
                target_id: 0,
                graceful_signal: "SIGTERM",
                graceful_wait_ms: graceful_wait.as_millis().try_into().unwrap_or(u64::MAX),
                graceful_signal_sent: false,
                forced_signal: None,
                forced_signal_sent: false,
                exited_after_grace: true,
                final_running: false,
                fallback_used: false,
                errors: vec!["process was already disarmed".to_string()],
            };
        };
        let mut report =
            tokio::task::spawn_blocking(move || terminate_process_blocking(pid, graceful_wait))
                .await
                .unwrap_or_else(|error| ProcessCleanupReport {
                    target_kind: "process",
                    target_id: pid,
                    graceful_signal: "SIGTERM",
                    graceful_wait_ms: graceful_wait.as_millis().try_into().unwrap_or(u64::MAX),
                    graceful_signal_sent: false,
                    forced_signal: None,
                    forced_signal_sent: false,
                    exited_after_grace: false,
                    final_running: true,
                    fallback_used: false,
                    errors: vec![format!("cleanup task failed: {error}")],
                });
        if let Ok(Ok(_)) = time::timeout(Duration::from_secs(2), self.child.wait()).await {
            report.final_running = false;
        }
        report.target_kind = "process";
        report
    }
}

impl Drop for RunningChild {
    fn drop(&mut self) {
        if let Some(process_group_id) = self.process_group_id.take() {
            let _ = signal_process_group(process_group_id, libc::SIGKILL);
        } else if let Some(pid) = self.pid.take() {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
}

async fn read_bounded_pty_output_with_sink<R>(
    mut reader: R,
    max_output_bytes: usize,
    sink: Option<ChildOutputSink>,
) -> std::io::Result<BoundedReadOutput>
where
    R: AsyncRead + Unpin,
{
    let mut captured = Vec::new();
    let mut truncated = false;
    let mut pending_stream = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut flush_tick = time::interval(Duration::from_millis(STREAM_OUTPUT_FLUSH_MS));
    flush_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    flush_tick.tick().await;

    loop {
        tokio::select! {
            read = read_pty_chunk(&mut reader, &mut buffer) => {
                let read = read?;
                if read == 0 {
                    flush_pending_stream(&sink, OutputStream::Pty, &mut pending_stream).await;
                    return Ok(BoundedReadOutput {
                        bytes: captured,
                        truncated,
                    });
                }
                let remaining = max_output_bytes.saturating_sub(captured.len());
                if read > remaining {
                    truncated = true;
                }
                if remaining > 0 {
                    let captured_len = read.min(remaining);
                    let chunk = &buffer[..captured_len];
                    captured.extend_from_slice(chunk);
                    if sink.is_some() {
                        pending_stream.extend_from_slice(chunk);
                        while pending_stream.len() >= STREAM_OUTPUT_CHUNK_BYTES {
                            let chunk: Vec<u8> = pending_stream
                                .drain(..STREAM_OUTPUT_CHUNK_BYTES)
                                .collect();
                            send_stream_chunk(&sink, OutputStream::Pty, chunk).await;
                        }
                    }
                }
            }
            _ = flush_tick.tick(), if sink.is_some() && !pending_stream.is_empty() => {
                flush_pending_stream(&sink, OutputStream::Pty, &mut pending_stream).await;
            }
        }
    }
}

async fn read_pty_chunk<R>(reader: &mut R, buffer: &mut [u8]) -> std::io::Result<usize>
where
    R: AsyncRead + Unpin,
{
    match reader.read(buffer).await {
        Ok(read) => Ok(read),
        Err(error) if error.raw_os_error() == Some(libc::EIO) => Ok(0),
        Err(error) => Err(error),
    }
}

async fn read_bounded_output_with_sink<R>(
    mut reader: R,
    max_output_bytes: usize,
    sink: Option<ChildOutputSink>,
    stream: OutputStream,
) -> std::io::Result<BoundedReadOutput>
where
    R: AsyncRead + Unpin,
{
    let mut captured = Vec::new();
    let mut truncated = false;
    let mut pending_stream = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut flush_tick = time::interval(Duration::from_millis(STREAM_OUTPUT_FLUSH_MS));
    flush_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    flush_tick.tick().await;

    loop {
        tokio::select! {
            read = reader.read(&mut buffer) => {
                let read = read?;
                if read == 0 {
                    flush_pending_stream(&sink, stream, &mut pending_stream).await;
                    return Ok(BoundedReadOutput {
                        bytes: captured,
                        truncated,
                    });
                }
                let remaining = max_output_bytes.saturating_sub(captured.len());
                if read > remaining {
                    truncated = true;
                }
                if remaining > 0 {
                    let captured_len = read.min(remaining);
                    let chunk = &buffer[..captured_len];
                    captured.extend_from_slice(chunk);
                    if sink.is_some() {
                        pending_stream.extend_from_slice(chunk);
                        while pending_stream.len() >= STREAM_OUTPUT_CHUNK_BYTES {
                            let chunk: Vec<u8> = pending_stream
                                .drain(..STREAM_OUTPUT_CHUNK_BYTES)
                                .collect();
                            send_stream_chunk(&sink, stream, chunk).await;
                        }
                    }
                }
            }
            _ = flush_tick.tick(), if sink.is_some() && !pending_stream.is_empty() => {
                flush_pending_stream(&sink, stream, &mut pending_stream).await;
            }
        }
    }
}

async fn flush_pending_stream(
    sink: &Option<ChildOutputSink>,
    stream: OutputStream,
    pending_stream: &mut Vec<u8>,
) {
    if pending_stream.is_empty() {
        return;
    }
    let chunk = std::mem::take(pending_stream);
    send_stream_chunk(sink, stream, chunk).await;
}

async fn send_stream_chunk(sink: &Option<ChildOutputSink>, stream: OutputStream, data: Vec<u8>) {
    if data.is_empty() {
        return;
    }
    if let Some(sink) = sink {
        let _ = sink
            .sender
            .send(CommandOutput {
                job_id: sink.job_id,
                stream,
                data,
                exit_code: None,
                done: false,
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SHELL: &str = "/bin/sh";

    #[tokio::test]
    async fn streams_stdout_before_child_exits() {
        let job_id = uuid::Uuid::new_v4();
        let mut command = tokio::process::Command::new(TEST_SHELL);
        command.arg("-lc").arg("printf start; sleep 1; printf end");
        let (tx, mut rx) = mpsc::channel(4);

        let task = tokio::spawn(run_child_with_streaming_output(
            command,
            5,
            64,
            ChildCleanupPolicy::ProcessGroup,
            ChildOutputSink { job_id, sender: tx },
        ));
        let first = time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("first chunk before command exit")
            .expect("streamed output");

        assert_eq!(first.job_id, job_id);
        assert_eq!(first.stream, OutputStream::Stdout);
        assert_eq!(first.data, b"start");
        assert!(!first.done);

        let output = match task.await.unwrap().unwrap() {
            ChildRunResult::Completed(output) => output,
            ChildRunResult::TimedOut(_) => panic!("child timed out"),
            ChildRunResult::Canceled { .. } => panic!("non-cancelable child was canceled"),
        };
        assert_eq!(output.stdout, b"startend");

        let mut streamed = first.data;
        while let Some(output) = rx.recv().await {
            streamed.extend_from_slice(&output.data);
        }
        assert_eq!(streamed, b"startend");
    }

    #[tokio::test]
    async fn bounded_child_output_reports_stdout_truncation() {
        let mut command = tokio::process::Command::new(TEST_SHELL);
        command.arg("-lc").arg("printf '%080d' 0");

        let output =
            match run_child_with_bounded_output(command, 5, 64, ChildCleanupPolicy::ProcessGroup)
                .await
                .unwrap()
            {
                ChildRunResult::Completed(output) => output,
                ChildRunResult::TimedOut(_) => panic!("child timed out"),
                ChildRunResult::Canceled { .. } => panic!("non-cancelable child was canceled"),
            };

        assert_eq!(output.stdout.len(), 64);
        assert!(output.stdout_truncated);
        assert!(!output.stderr_truncated);
        assert!(!output.pty_truncated);
        assert_eq!(output.exit_code, Some(0));
    }

    #[tokio::test]
    async fn bounded_child_output_reports_stderr_truncation() {
        let mut command = tokio::process::Command::new(TEST_SHELL);
        command.arg("-lc").arg("printf '%080d' 0 >&2");

        let output =
            match run_child_with_bounded_output(command, 5, 64, ChildCleanupPolicy::ProcessGroup)
                .await
                .unwrap()
            {
                ChildRunResult::Completed(output) => output,
                ChildRunResult::TimedOut(_) => panic!("child timed out"),
                ChildRunResult::Canceled { .. } => panic!("non-cancelable child was canceled"),
            };

        assert_eq!(output.stderr.len(), 64);
        assert!(!output.stdout_truncated);
        assert!(output.stderr_truncated);
        assert!(!output.pty_truncated);
        assert_eq!(output.exit_code, Some(0));
    }

    #[tokio::test]
    async fn bounded_pty_output_reports_truncation() {
        let mut command = tokio::process::Command::new(TEST_SHELL);
        command.arg("-lc").arg("printf '%080d' 0");

        let output =
            match run_pty_with_bounded_output(command, 5, 64, ChildCleanupPolicy::ProcessGroup)
                .await
                .unwrap()
            {
                ChildRunResult::Completed(output) => output,
                ChildRunResult::TimedOut(_) => panic!("pty command timed out"),
                ChildRunResult::Canceled { .. } => panic!("non-cancelable pty was canceled"),
            };

        assert_eq!(output.stdout.len(), 64);
        assert!(!output.stdout_truncated);
        assert!(!output.stderr_truncated);
        assert!(output.pty_truncated);
        assert_eq!(output.exit_code, Some(0));
    }

    #[tokio::test]
    async fn pty_command_reports_tty_and_streams_pty_output() {
        let job_id = uuid::Uuid::new_v4();
        let mut command = tokio::process::Command::new(TEST_SHELL);
        command.arg("-lc").arg("test -t 1 && printf tty");
        let (tx, mut rx) = mpsc::channel(4);

        let output = match run_pty_with_streaming_output(
            command,
            5,
            64,
            ChildCleanupPolicy::ProcessGroup,
            ChildOutputSink { job_id, sender: tx },
        )
        .await
        .unwrap()
        {
            ChildRunResult::Completed(output) => output,
            ChildRunResult::TimedOut(_) => panic!("pty command timed out"),
            ChildRunResult::Canceled { .. } => panic!("non-cancelable pty was canceled"),
        };

        assert_eq!(output.stdout, b"tty");
        assert_eq!(output.stderr, b"");
        assert_eq!(output.exit_code, Some(0));
        let streamed = rx.recv().await.expect("streamed pty output");
        assert_eq!(streamed.job_id, job_id);
        assert_eq!(streamed.stream, OutputStream::Pty);
        assert_eq!(streamed.data, b"tty");
    }

    #[tokio::test]
    async fn timeout_reports_process_group_cleanup() {
        let mut command = tokio::process::Command::new(TEST_SHELL);
        command.arg("-lc").arg("sleep 5");

        let cleanup =
            match run_child_with_bounded_output(command, 1, 64, ChildCleanupPolicy::ProcessGroup)
                .await
                .unwrap()
            {
                ChildRunResult::Completed(_) => panic!("sleep command should time out"),
                ChildRunResult::TimedOut(cleanup) => cleanup,
                ChildRunResult::Canceled { .. } => {
                    panic!("non-cancelable child was canceled")
                }
            };

        assert_eq!(cleanup.target_kind, "process_group");
        assert!(cleanup.target_id > 0);
        assert!(cleanup.graceful_signal_sent);
        assert!(!cleanup.final_running);
    }

    #[tokio::test]
    async fn cancellation_reports_process_group_cleanup() {
        let root =
            std::env::temp_dir().join(format!("vpsman-child-cancel-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let pid_file = root.join("child.pid");
        let mut command = tokio::process::Command::new(TEST_SHELL);
        command.arg("-lc").arg(format!(
            "sleep 30 & echo $! > '{}'; wait",
            pid_file.display()
        ));
        let cancel_token = CommandCancelToken::default();
        let task = tokio::spawn(run_child_with_bounded_output_cancelable(
            command,
            60,
            64,
            ChildCleanupPolicy::ProcessGroup,
            cancel_token.clone(),
        ));
        let child_pid = wait_for_pid_file(&pid_file).await;
        assert!(process_running(child_pid));

        cancel_token.cancel("operator requested cancellation".to_string());
        let (cleanup, reason) = match task.await.unwrap().unwrap() {
            ChildRunResult::Completed(_) => panic!("canceled command should not complete"),
            ChildRunResult::TimedOut(_) => panic!("canceled command should not time out"),
            ChildRunResult::Canceled { cleanup, reason } => (cleanup, reason),
        };

        assert_eq!(reason, "operator requested cancellation");
        assert_eq!(cleanup.target_kind, "process_group");
        assert!(cleanup.target_id > 0);
        assert!(cleanup.graceful_signal_sent);
        assert!(!cleanup.final_running);
        for _ in 0..40 {
            if !process_running(child_pid) {
                break;
            }
            time::sleep(Duration::from_millis(25)).await;
        }
        assert!(
            !process_running(child_pid),
            "child pid {child_pid} survived cancellation"
        );
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn aborting_running_child_cleans_process_group_children() {
        let root =
            std::env::temp_dir().join(format!("vpsman-child-abort-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let pid_file = root.join("child.pid");
        let mut command = tokio::process::Command::new(TEST_SHELL);
        command.arg("-lc").arg(format!(
            "sleep 30 & echo $! > '{}'; wait",
            pid_file.display()
        ));

        let task = tokio::spawn(run_child_with_bounded_output(
            command,
            60,
            64,
            ChildCleanupPolicy::ProcessGroup,
        ));
        let child_pid = wait_for_pid_file(&pid_file).await;
        assert!(process_running(child_pid));

        task.abort();
        let _ = task.await;
        for _ in 0..40 {
            if !process_running(child_pid) {
                break;
            }
            time::sleep(Duration::from_millis(25)).await;
        }
        assert!(
            !process_running(child_pid),
            "child pid {child_pid} survived task abort"
        );
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn timeout_can_report_direct_child_cleanup() {
        let mut command = tokio::process::Command::new(TEST_SHELL);
        command.arg("-lc").arg("sleep 5");

        let cleanup =
            match run_child_with_bounded_output(command, 1, 64, ChildCleanupPolicy::DirectChild)
                .await
                .unwrap()
            {
                ChildRunResult::Completed(_) => panic!("sleep command should time out"),
                ChildRunResult::TimedOut(cleanup) => cleanup,
                ChildRunResult::Canceled { .. } => {
                    panic!("non-cancelable child was canceled")
                }
            };

        assert_eq!(cleanup.target_kind, "process");
        assert!(cleanup.target_id > 0);
        assert!(cleanup.graceful_signal_sent);
        assert!(!cleanup.final_running);
    }

    async fn wait_for_pid_file(path: &std::path::Path) -> u32 {
        for _ in 0..40 {
            if let Ok(contents) = tokio::fs::read_to_string(path).await {
                if let Ok(pid) = contents.trim().parse::<u32>() {
                    return pid;
                }
            }
            time::sleep(Duration::from_millis(25)).await;
        }
        panic!("pid file was not created: {}", path.display());
    }

    fn process_running(pid: u32) -> bool {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
}
