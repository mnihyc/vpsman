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
#[path = "tests_network_runtime_command_runner.rs"]
mod tests;
