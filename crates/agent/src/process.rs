use std::{path::Path, process::Stdio, time::Duration};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::{process::Command, time};
use vpsman_common::{
    AgentConfig, AgentProcessInventorySource, CommandOutput, OutputStream, RuntimeTunnelCommand,
};

use crate::{
    child_process::{run_child_with_bounded_output_cancelable, ChildCleanupPolicy, ChildRunResult},
    command_worker::CommandCancelToken,
};

const MAX_PROCESS_OUTPUT_CHUNK_BYTES: usize = 64 * 1024;

pub(crate) async fn execute_process_list(
    config: &AgentConfig,
    job_id: uuid::Uuid,
    limit: u16,
    max_timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<Vec<CommandOutput>> {
    let limit = limit.clamp(1, 512);
    let snapshot =
        collect_process_snapshot_for_config(config, limit, max_timeout_secs, cancel_token).await?;
    let stdout = serde_json::to_vec(&snapshot)?;
    let mut outputs = chunked_output(job_id, OutputStream::Stdout, &stdout);
    outputs.push(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "process_list",
            "count": snapshot.processes.len(),
            "truncated": snapshot.truncated,
            "source": snapshot.source,
        }))?,
        exit_code: Some(0),
        done: true,
    });
    Ok(outputs)
}

#[derive(Debug, Deserialize, Serialize)]
struct ProcessSnapshot {
    #[serde(default = "default_process_list_type")]
    r#type: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    truncated: bool,
    #[serde(default)]
    processes: Vec<ProcessView>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ProcessView {
    pid: u32,
    ppid: u32,
    uid: u32,
    state: String,
    name: String,
    command: String,
    rss_kib: u64,
}

async fn collect_process_snapshot_for_config(
    config: &AgentConfig,
    limit: u16,
    max_timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<ProcessSnapshot> {
    cancel_token.check("process_list")?;
    match config.execution.process_inventory_source {
        AgentProcessInventorySource::LinuxProcfs => {
            let proc_root = config.execution.process_proc_root.clone();
            let snapshot = time::timeout(
                Duration::from_secs(max_timeout_secs.max(1)),
                tokio::task::spawn_blocking(move || {
                    collect_linux_procfs_snapshot(&proc_root, limit)
                }),
            )
            .await
            .context("process list timed out")??
            .context("failed to collect process list")?;
            cancel_token.check("process_list")?;
            Ok(snapshot)
        }
        AgentProcessInventorySource::CustomCommand => {
            let command = config
                .execution
                .process_inventory_command
                .as_ref()
                .context("custom process inventory command is not configured")?;
            collect_custom_process_snapshot(config, command, limit, max_timeout_secs, cancel_token)
                .await
        }
    }
}

fn collect_linux_procfs_snapshot(proc_root: &str, limit: u16) -> Result<ProcessSnapshot> {
    let proc_root = Path::new(proc_root);
    let mut processes = Vec::new();
    for entry in std::fs::read_dir(proc_root)
        .with_context(|| format!("failed to read {}", proc_root.display()))?
    {
        let entry = entry?;
        let file_name = entry.file_name();
        let Some(pid) = file_name
            .to_str()
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        if let Some(process) = read_process_view(proc_root, pid) {
            processes.push(process);
        }
    }

    processes.sort_by(|left, right| {
        right
            .rss_kib
            .cmp(&left.rss_kib)
            .then_with(|| left.pid.cmp(&right.pid))
    });
    let limit = limit as usize;
    let truncated = processes.len() > limit;
    processes.truncate(limit);
    Ok(ProcessSnapshot {
        r#type: default_process_list_type(),
        source: "linux_procfs".to_string(),
        truncated,
        processes,
    })
}

async fn collect_custom_process_snapshot(
    config: &AgentConfig,
    command: &RuntimeTunnelCommand,
    limit: u16,
    max_timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<ProcessSnapshot> {
    let argv = render_process_inventory_argv(config, command, limit)?;
    let output = run_json_command(
        &argv,
        command
            .max_timeout_secs
            .min(max_timeout_secs.max(1))
            .clamp(1, 120),
        command.max_output_bytes.clamp(1024, 64 * 1024) as usize,
        cancel_token,
    )
    .await?;
    let mut snapshot: ProcessSnapshot = serde_json::from_slice(&output)
        .context("custom process inventory returned invalid JSON")?;
    snapshot.r#type = default_process_list_type();
    snapshot.source = "custom_command".to_string();
    snapshot.processes.sort_by(|left, right| {
        right
            .rss_kib
            .cmp(&left.rss_kib)
            .then_with(|| left.pid.cmp(&right.pid))
    });
    let limit = limit as usize;
    snapshot.truncated |= snapshot.processes.len() > limit;
    snapshot.processes.truncate(limit);
    Ok(snapshot)
}

fn read_process_view(proc_root: &Path, pid: u32) -> Option<ProcessView> {
    let status = std::fs::read_to_string(proc_root.join(pid.to_string()).join("status")).ok()?;
    let mut ppid = 0_u32;
    let mut uid = 0_u32;
    let mut rss_kib = 0_u64;
    let mut name = String::new();
    let mut state = String::new();

    for line in status.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key {
            "Name" => name = value.to_string(),
            "State" => state = value.to_string(),
            "PPid" => ppid = value.parse().unwrap_or_default(),
            "Uid" => {
                uid = value
                    .split_whitespace()
                    .next()
                    .and_then(|part| part.parse().ok())
                    .unwrap_or_default();
            }
            "VmRSS" => {
                rss_kib = value
                    .split_whitespace()
                    .next()
                    .and_then(|part| part.parse().ok())
                    .unwrap_or_default();
            }
            _ => {}
        }
    }
    if name.is_empty() {
        name = format!("pid-{pid}");
    }
    Some(ProcessView {
        pid,
        ppid,
        uid,
        state,
        command: process_command(proc_root, pid).unwrap_or_else(|| name.clone()),
        name,
        rss_kib,
    })
}

fn process_command(proc_root: &Path, pid: u32) -> Option<String> {
    let bytes = std::fs::read(proc_root.join(pid.to_string()).join("cmdline")).ok()?;
    if bytes.is_empty() {
        return None;
    }
    let mut command = bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part))
        .collect::<Vec<_>>()
        .join(" ");
    if command.len() > 256 {
        command.truncate(256);
        command.push_str("...");
    }
    Some(command)
}

fn render_process_inventory_argv(
    config: &AgentConfig,
    command: &RuntimeTunnelCommand,
    limit: u16,
) -> Result<Vec<String>> {
    if command.argv.is_empty() {
        anyhow::bail!("process inventory argv is empty");
    }
    if !command.argv[0].starts_with('/') {
        anyhow::bail!("process inventory executable must be absolute");
    }
    Ok(command
        .argv
        .iter()
        .map(|part| {
            part.replace("{client_id}", &config.client_id)
                .replace("{display_name}", &config.display_name)
                .replace("{tags_csv}", &config.tags.join(","))
                .replace("{limit}", &limit.to_string())
        })
        .collect())
}

async fn run_json_command(
    argv: &[String],
    max_timeout_secs: u64,
    max_output_bytes: usize,
    cancel_token: CommandCancelToken,
) -> Result<Vec<u8>> {
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]);
    command.stdin(Stdio::null());
    let result = run_child_with_bounded_output_cancelable(
        command,
        max_timeout_secs,
        max_output_bytes,
        ChildCleanupPolicy::ProcessGroup,
        cancel_token,
    )
    .await
    .context("failed to run process inventory source")?;
    match result {
        ChildRunResult::Completed(output) => {
            if output.stdout_truncated || output.stderr_truncated {
                anyhow::bail!("process inventory output exceeded limit");
            }
            if output.exit_code != Some(0) {
                anyhow::bail!(
                    "process inventory source exited with {:?}",
                    output.exit_code
                );
            }
            Ok(output.stdout)
        }
        ChildRunResult::TimedOut(_) => anyhow::bail!("process inventory source timed out"),
        ChildRunResult::Canceled { reason, .. } => {
            anyhow::bail!("process inventory source canceled: {reason}")
        }
    }
}

fn default_process_list_type() -> String {
    "process_list".to_string()
}

fn chunked_output(job_id: uuid::Uuid, stream: OutputStream, data: &[u8]) -> Vec<CommandOutput> {
    data.chunks(MAX_PROCESS_OUTPUT_CHUNK_BYTES)
        .map(|chunk| CommandOutput {
            job_id,
            stream,
            data: chunk.to_vec(),
            exit_code: None,
            done: false,
        })
        .collect()
}
