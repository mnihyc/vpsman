use std::{
    process::{ExitStatus, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
    task::JoinHandle,
    time,
};

use crate::{
    command_worker::{CommandCancelToken, CommandCanceled},
    process_cleanup::signal_process_group,
};

#[derive(Debug)]
struct LimitedOutput {
    data: Vec<u8>,
    truncated: bool,
}

#[allow(dead_code)]
pub(super) async fn run_runtime_command(
    label: &'static str,
    argv: &[String],
    mutates: bool,
    required: bool,
    max_timeout_secs: u64,
    max_output_bytes: usize,
) -> Result<serde_json::Value> {
    run_runtime_command_cancelable(
        label,
        argv,
        mutates,
        required,
        max_timeout_secs,
        max_output_bytes,
        CommandCancelToken::default(),
    )
    .await
}

pub(super) async fn run_runtime_command_cancelable(
    label: &'static str,
    argv: &[String],
    mutates: bool,
    required: bool,
    max_timeout_secs: u64,
    max_output_bytes: usize,
    cancel_token: CommandCancelToken,
) -> Result<serde_json::Value> {
    cancel_token.check("network_runtime")?;
    ensure_command_base(argv, label)?;
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.kill_on_drop(true);
    command.process_group(0);
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to run runtime tunnel command {label}"))?;
    let process_group_id = child.id().map(|pid| pid as libc::pid_t);
    let stdout = child
        .stdout
        .take()
        .context("runtime tunnel command stdout pipe missing")?;
    let stderr = child
        .stderr
        .take()
        .context("runtime tunnel command stderr pipe missing")?;
    let mut stdout_task = Some(tokio::spawn(read_limited(stdout, max_output_bytes)));
    let mut stderr_task = Some(tokio::spawn(read_limited(stderr, max_output_bytes)));
    let deadline = Instant::now() + Duration::from_secs(max_timeout_secs.clamp(1, 120));
    let mut timed_out = false;
    let mut killed_for_output_limit = false;
    let mut stdout_output = None;
    let mut stderr_output = None;

    let status = loop {
        if let Some(exit_status) = child.try_wait()? {
            break Some(exit_status);
        }
        if stdout_output.is_none() && task_is_finished(&stdout_task) {
            let output = join_limited(stdout_task.take()).await?;
            killed_for_output_limit |= output.truncated;
            stdout_output = Some(output);
        }
        if stderr_output.is_none() && task_is_finished(&stderr_task) {
            let output = join_limited(stderr_task.take()).await?;
            killed_for_output_limit |= output.truncated;
            stderr_output = Some(output);
        }
        if killed_for_output_limit {
            break terminate_runtime_child(&mut child, process_group_id).await;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            break terminate_runtime_child(&mut child, process_group_id).await;
        }
        tokio::select! {
            biased;
            reason = cancel_token.cancelled() => {
                let _ = terminate_runtime_child(&mut child, process_group_id).await;
                if let Some(task) = stdout_task.take() {
                    task.abort();
                }
                if let Some(task) = stderr_task.take() {
                    task.abort();
                }
                return Err(CommandCanceled::new("network_runtime", reason).into());
            }
            _ = time::sleep(Duration::from_millis(10)) => {}
        }
    };

    let stdout = match stdout_output {
        Some(output) => output,
        None => join_limited(stdout_task.take()).await?,
    };
    let stderr = match stderr_output {
        Some(output) => output,
        None => join_limited(stderr_task.take()).await?,
    };
    let exit_code = status.and_then(|status| status.code());
    let success = exit_code == Some(0) && !timed_out && !killed_for_output_limit;
    Ok(serde_json::json!({
        "label": label,
        "argv": argv,
        "mutates": mutates,
        "required": required,
        "skipped": false,
        "success": success,
        "exit_code": exit_code,
        "timed_out": timed_out,
        "killed_for_output_limit": killed_for_output_limit,
        "stdout": output_json(stdout),
        "stderr": output_json(stderr),
    }))
}

async fn terminate_runtime_child(
    child: &mut Child,
    process_group_id: Option<libc::pid_t>,
) -> Option<ExitStatus> {
    let Some(process_group_id) = process_group_id else {
        let _ = child.start_kill();
        return child.wait().await.ok();
    };
    let _ = signal_process_group(process_group_id, libc::SIGTERM);
    if let Ok(status) = time::timeout(Duration::from_millis(500), child.wait()).await {
        return status.ok();
    }
    let _ = signal_process_group(process_group_id, libc::SIGKILL);
    match time::timeout(Duration::from_secs(2), child.wait()).await {
        Ok(status) => status.ok(),
        Err(_) => None,
    }
}

async fn read_limited<R>(mut reader: R, max_bytes: usize) -> Result<LimitedOutput>
where
    R: AsyncRead + Unpin,
{
    let mut data = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(LimitedOutput {
                data,
                truncated: false,
            });
        }
        let remaining = max_bytes.saturating_sub(data.len());
        if remaining == 0 {
            return Ok(LimitedOutput {
                data,
                truncated: true,
            });
        }
        let take = read.min(remaining);
        data.extend_from_slice(&buffer[..take]);
        if take < read {
            return Ok(LimitedOutput {
                data,
                truncated: true,
            });
        }
    }
}

fn task_is_finished(task: &Option<JoinHandle<Result<LimitedOutput>>>) -> bool {
    task.as_ref().is_some_and(JoinHandle::is_finished)
}

async fn join_limited(task: Option<JoinHandle<Result<LimitedOutput>>>) -> Result<LimitedOutput> {
    let Some(task) = task else {
        return Ok(LimitedOutput {
            data: Vec::new(),
            truncated: false,
        });
    };
    task.await?
}

fn output_json(output: LimitedOutput) -> serde_json::Value {
    match String::from_utf8(output.data.clone()) {
        Ok(text) => serde_json::json!({
            "text": text,
            "base64": null,
            "bytes": output.data.len(),
            "truncated": output.truncated,
        }),
        Err(_) => serde_json::json!({
            "text": null,
            "base64": base64_encode(&output.data),
            "bytes": output.data.len(),
            "truncated": output.truncated,
        }),
    }
}

fn ensure_command_base(argv: &[String], label: &str) -> Result<()> {
    if argv.is_empty() {
        anyhow::bail!("{label} argv is empty");
    }
    if !argv[0].starts_with('/') {
        anyhow::bail!("{label} executable must be absolute");
    }
    Ok(())
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SHELL: &str = "/bin/sh";

    #[tokio::test]
    async fn cancellation_kills_runtime_command_process_group_children() {
        let root = std::env::temp_dir().join(format!(
            "vpsman-runtime-command-cancel-{}",
            uuid::Uuid::new_v4()
        ));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let pid_file = root.join("child.pid");
        let argv = vec![
            TEST_SHELL.to_string(),
            "-lc".to_string(),
            format!("sleep 30 & echo $! > '{}'; wait", pid_file.display()),
        ];
        let cancel_token = CommandCancelToken::default();
        let task_cancel_token = cancel_token.clone();
        let task = tokio::spawn(async move {
            run_runtime_command_cancelable(
                "test_runtime_cancel",
                &argv,
                true,
                true,
                60,
                1024,
                task_cancel_token,
            )
            .await
        });
        let child_pid = wait_for_pid_file(&pid_file).await;
        assert!(process_running(child_pid));

        cancel_token.cancel("operator requested cancellation".to_string());
        let error = task.await.unwrap().unwrap_err();
        let canceled = error
            .downcast_ref::<CommandCanceled>()
            .expect("runtime command should return CommandCanceled");
        assert_eq!(canceled.reason(), "operator requested cancellation");

        for _ in 0..40 {
            if !process_running(child_pid) {
                break;
            }
            time::sleep(Duration::from_millis(25)).await;
        }
        assert!(
            !process_running(child_pid),
            "runtime child pid {child_pid} survived cancellation"
        );
        let _ = tokio::fs::remove_dir_all(root).await;
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
        panic!("pid file {} was not written", path.display());
    }

    fn process_running(pid: u32) -> bool {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
}
