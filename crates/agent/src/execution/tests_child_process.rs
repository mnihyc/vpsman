use super::*;
use std::{io::Write, os::unix::fs::PermissionsExt};

const TEST_SHELL: &str = "/bin/sh";

#[tokio::test]
async fn test_spawn_retries_transient_text_file_busy() {
    let root = std::env::temp_dir().join(format!("vpsman-child-etxtbsy-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("busy.sh");
    let mut script = File::create(&path).unwrap();
    script
        .write_all(b"#!/bin/sh\nprintf fixture-ready")
        .unwrap();
    let mut permissions = script.metadata().unwrap().permissions();
    permissions.set_mode(0o755);
    script.set_permissions(permissions).unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        drop(script);
    });

    let command = tokio::process::Command::new(&path);
    let output =
        match run_child_with_bounded_output(command, 5, 64, ChildCleanupPolicy::ProcessGroup)
            .await
            .unwrap()
        {
            ChildRunResult::Completed(output) => output,
            ChildRunResult::TimedOut(_) => panic!("fixture command timed out"),
            ChildRunResult::Canceled { .. } => panic!("fixture command was canceled"),
        };

    release.join().unwrap();
    assert_eq!(output.stdout, b"fixture-ready");
    assert_eq!(output.exit_code, Some(0));
    std::fs::remove_dir_all(root).unwrap();
}

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

    let output = match run_pty_with_bounded_output(command, 5, 64, ChildCleanupPolicy::ProcessGroup)
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
    command
        .arg("-lc")
        .arg("test -t 0 && test -t 1 && test -t 2 && tty -s && printf tty");
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
    let root = std::env::temp_dir().join(format!("vpsman-child-cancel-{}", uuid::Uuid::new_v4()));
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
    let root = std::env::temp_dir().join(format!("vpsman-child-abort-{}", uuid::Uuid::new_v4()));
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
