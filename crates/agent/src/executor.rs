use anyhow::{Context, Result};
use ed25519_dalek::VerifyingKey;
use tokio::{
    sync::mpsc,
    time::{self, Duration},
};
use vpsman_common::{
    decode_noise_key_hex, encode_json, verify_command_envelope, AgentConfig,
    AgentExecutionEnvironmentPolicy, AgentExecutionProcessCleanupPolicy, AgentExecutionPtyPolicy,
    CommandOutput, JobCommand, JobRequest, OutputStream, PrivilegeReplayCache,
    MAX_SHELL_SCRIPT_BYTES,
};

use crate::{
    child_process::{
        run_child_with_bounded_output, run_child_with_streaming_output,
        run_pty_with_bounded_output, run_pty_with_streaming_output, ChildCleanupPolicy,
        ChildOutputSink, ChildRunResult,
    },
    file_download::{execute_file_transfer_download_chunk, execute_file_transfer_download_start},
    file_pull::execute_file_pull_with_timeout,
    file_push::{
        execute_file_push, execute_file_push_chunked, execute_file_transfer_abort,
        execute_file_transfer_chunk, execute_file_transfer_commit, execute_file_transfer_start,
    },
    process::execute_process_list,
    supervisor::execute_process_supervisor_command,
    telemetry::unix_now,
    terminal::execute_terminal_command,
    update::{execute_update_agent, execute_update_check, AgentUpdateCheckInput, AgentUpdateInput},
    update_activation::{
        execute_update_activate, execute_update_rollback, AgentUpdateActivateInput,
        AgentUpdateRollbackInput,
    },
};

const MAX_COMMAND_OUTPUT_BYTES: usize = 64 * 1024;
const PRESET_USER_SESSIONS_W: &str = "/usr/bin/w";
const PRESET_USER_SESSIONS_WHO: &str = "/usr/bin/who";

pub(crate) fn authorize_job(
    config: &AgentConfig,
    request: &JobRequest,
    replay_cache: &mut PrivilegeReplayCache,
) -> std::result::Result<(), String> {
    let proof_key = decode_required_32_hex(
        config.auth.proof_key_hex.as_deref(),
        "missing agent proof key",
    )?;
    let server_public_key = decode_required_32_hex(
        config.auth.server_ed25519_public_key_hex.as_deref(),
        "missing server signing public key",
    )?;
    let proof_key: [u8; 32] = proof_key
        .try_into()
        .map_err(|_| "agent proof key must be 32 bytes".to_string())?;
    let server_public_key: [u8; 32] = server_public_key
        .try_into()
        .map_err(|_| "server signing public key must be 32 bytes".to_string())?;
    let verifying_key = VerifyingKey::from_bytes(&server_public_key)
        .map_err(|_| "invalid server signing public key".to_string())?;
    let payload = encode_json(&request.command).map_err(|error| error.to_string())?;
    verify_command_envelope(
        &proof_key,
        &verifying_key,
        &format!("client:{}", config.client_id),
        &payload,
        &request.envelope,
        unix_now(),
        replay_cache,
    )
    .map_err(|error| error.to_string())
}

fn decode_required_32_hex(
    value: Option<&str>,
    missing_message: &str,
) -> std::result::Result<Vec<u8>, String> {
    let value = value.ok_or_else(|| missing_message.to_string())?;
    decode_noise_key_hex(value).map_err(|error| error.to_string())
}

#[cfg(test)]
pub(crate) async fn execute_job_command(
    job_id: uuid::Uuid,
    command: &JobCommand,
    timeout_secs: u64,
) -> Result<Vec<CommandOutput>> {
    execute_job_command_with_config_and_output_sink(
        &AgentConfig::default(),
        job_id,
        command,
        timeout_secs,
        None,
    )
    .await
}

#[cfg(test)]
pub(crate) async fn execute_job_command_with_output_sink(
    job_id: uuid::Uuid,
    command: &JobCommand,
    timeout_secs: u64,
    output_tx: Option<mpsc::Sender<CommandOutput>>,
) -> Result<Vec<CommandOutput>> {
    execute_job_command_with_config_and_output_sink(
        &AgentConfig::default(),
        job_id,
        command,
        timeout_secs,
        output_tx,
    )
    .await
}

pub(crate) async fn execute_job_command_with_config_and_output_sink(
    config: &AgentConfig,
    job_id: uuid::Uuid,
    command: &JobCommand,
    timeout_secs: u64,
    output_tx: Option<mpsc::Sender<CommandOutput>>,
) -> Result<Vec<CommandOutput>> {
    match command {
        JobCommand::Shell { argv, pty } => {
            execute_shell_command(config, job_id, argv, *pty, timeout_secs, output_tx).await
        }
        JobCommand::ShellScript { script } => {
            execute_shell_script(config, job_id, script, timeout_secs, output_tx).await
        }
        JobCommand::TerminalOpen { .. }
        | JobCommand::TerminalInput { .. }
        | JobCommand::TerminalPoll { .. }
        | JobCommand::TerminalResize { .. }
        | JobCommand::TerminalClose { .. } => {
            execute_terminal_command(config, job_id, command, timeout_secs).await
        }
        JobCommand::FilePull { path } => {
            execute_file_pull_with_timeout(job_id, path, timeout_secs, output_tx).await
        }
        JobCommand::FilePush {
            path,
            mode,
            size_bytes,
            sha256_hex,
            data_base64,
        } => time::timeout(
            Duration::from_secs(timeout_secs.max(1)),
            execute_file_push(job_id, path, *mode, *size_bytes, sha256_hex, data_base64),
        )
        .await
        .context("file push timed out")?,
        JobCommand::FilePushChunked {
            path,
            mode,
            size_bytes,
            sha256_hex,
            chunks,
        } => time::timeout(
            Duration::from_secs(timeout_secs.max(1)),
            execute_file_push_chunked(job_id, path, *mode, *size_bytes, sha256_hex, chunks),
        )
        .await
        .context("chunked file push timed out")?,
        JobCommand::FileTransferStart {
            session_id,
            path,
            mode,
            size_bytes,
            sha256_hex,
            chunk_size_bytes,
            rate_limit_kbps,
            resume_token_hash,
        } => time::timeout(
            Duration::from_secs(timeout_secs.max(1)),
            execute_file_transfer_start(
                job_id,
                *session_id,
                path,
                *mode,
                *size_bytes,
                sha256_hex,
                *chunk_size_bytes,
                *rate_limit_kbps,
                resume_token_hash,
            ),
        )
        .await
        .context("file transfer start timed out")?,
        JobCommand::FileTransferChunk {
            session_id,
            offset,
            chunk,
            resume_token_hash,
        } => time::timeout(
            Duration::from_secs(timeout_secs.max(1)),
            execute_file_transfer_chunk(job_id, *session_id, *offset, chunk, resume_token_hash),
        )
        .await
        .context("file transfer chunk timed out")?,
        JobCommand::FileTransferCommit {
            session_id,
            resume_token_hash,
        } => time::timeout(
            Duration::from_secs(timeout_secs.max(1)),
            execute_file_transfer_commit(job_id, *session_id, resume_token_hash),
        )
        .await
        .context("file transfer commit timed out")?,
        JobCommand::FileTransferAbort {
            session_id,
            resume_token_hash,
        } => time::timeout(
            Duration::from_secs(timeout_secs.max(1)),
            execute_file_transfer_abort(job_id, *session_id, resume_token_hash),
        )
        .await
        .context("file transfer abort timed out")?,
        JobCommand::FileTransferDownloadStart {
            session_id,
            path,
            chunk_size_bytes,
            rate_limit_kbps,
            resume_token_hash,
        } => time::timeout(
            Duration::from_secs(timeout_secs.max(1)),
            execute_file_transfer_download_start(
                job_id,
                *session_id,
                path,
                *chunk_size_bytes,
                *rate_limit_kbps,
                resume_token_hash,
            ),
        )
        .await
        .context("file transfer download start timed out")?,
        JobCommand::FileTransferDownloadChunk {
            session_id,
            offset,
            max_bytes,
            resume_token_hash,
        } => time::timeout(
            Duration::from_secs(timeout_secs.max(1)),
            execute_file_transfer_download_chunk(
                job_id,
                *session_id,
                *offset,
                *max_bytes,
                resume_token_hash,
            ),
        )
        .await
        .context("file transfer download chunk timed out")?,
        JobCommand::UserSessions => execute_user_sessions(config, job_id, timeout_secs).await,
        JobCommand::ProcessList { limit } => {
            execute_process_list(config, job_id, *limit, timeout_secs).await
        }
        JobCommand::ProcessStart { .. }
        | JobCommand::ProcessStop { .. }
        | JobCommand::ProcessRestart { .. }
        | JobCommand::ProcessStatus { .. }
        | JobCommand::ProcessLogs { .. } => {
            execute_process_supervisor_command(job_id, command, timeout_secs).await
        }
        JobCommand::UpdateAgent {
            artifact_url,
            sha256_hex,
            artifact_signature_hex,
            artifact_signing_key_hex,
        } => {
            execute_update_agent(AgentUpdateInput {
                job_id,
                artifact_url,
                sha256_hex,
                artifact_signature_hex: artifact_signature_hex.as_deref(),
                artifact_signing_key_hex: artifact_signing_key_hex.as_deref(),
                trusted_artifact_signing_key_hex: None,
                timeout_secs,
            })
            .await
        }
        JobCommand::AgentUpdateActivate {
            staged_sha256_hex,
            restart_agent,
        } => {
            execute_update_activate(AgentUpdateActivateInput {
                job_id,
                staged_sha256_hex: staged_sha256_hex.clone(),
                restart_agent: *restart_agent,
                timeout_secs,
            })
            .await
        }
        JobCommand::AgentUpdateRollback {
            rollback_sha256_hex,
        } => {
            execute_update_rollback(AgentUpdateRollbackInput {
                job_id,
                rollback_sha256_hex: rollback_sha256_hex.clone(),
                timeout_secs,
            })
            .await
        }
        JobCommand::AgentUpdateCheck {
            version_url,
            activate,
            restart_agent,
        } => {
            let version_url = version_url
                .as_deref()
                .unwrap_or(config.update.unmanaged_version_url.as_str());
            execute_update_check(AgentUpdateCheckInput {
                job_id,
                version_url,
                activate: *activate,
                restart_agent: *restart_agent,
                trusted_artifact_signing_key_hex: config
                    .update
                    .trusted_artifact_signing_key_hex
                    .as_deref(),
                timeout_secs,
            })
            .await
        }
        JobCommand::HotConfig { .. }
        | JobCommand::DataSourceConfigPatch { .. }
        | JobCommand::AuthProofKeyRotate { .. }
        | JobCommand::Backup { .. }
        | JobCommand::Restore { .. }
        | JobCommand::RestoreRollback { .. }
        | JobCommand::NetworkApply { .. }
        | JobCommand::NetworkOspfCostUpdate { .. }
        | JobCommand::NetworkRollback { .. }
        | JobCommand::NetworkStatus { .. }
        | JobCommand::NetworkProbe { .. }
        | JobCommand::NetworkSpeedTest { .. } => {
            anyhow::bail!("unsupported command type in direct executor")
        }
    }
}

async fn execute_shell_command(
    config: &AgentConfig,
    job_id: uuid::Uuid,
    argv: &[String],
    pty: bool,
    timeout_secs: u64,
    output_tx: Option<mpsc::Sender<CommandOutput>>,
) -> Result<Vec<CommandOutput>> {
    if argv.is_empty() {
        anyhow::bail!("argv command is empty");
    }

    let mut child = tokio::process::Command::new(&argv[0]);
    child.args(&argv[1..]);
    apply_execution_policy(config, &mut child);
    let cleanup_policy = child_cleanup_policy(config);
    if pty {
        ensure_pty_allowed(config)?;
        execute_pty_child_with_output(job_id, child, timeout_secs, cleanup_policy, output_tx).await
    } else {
        execute_child_with_output(
            job_id,
            child,
            timeout_secs,
            cleanup_policy,
            "shell_argv",
            None,
            output_tx,
        )
        .await
    }
}

async fn execute_shell_script(
    config: &AgentConfig,
    job_id: uuid::Uuid,
    script: &str,
    timeout_secs: u64,
    output_tx: Option<mpsc::Sender<CommandOutput>>,
) -> Result<Vec<CommandOutput>> {
    validate_shell_script(script)?;
    let shell_argv = render_shell_script_argv(config, script)?;
    let mut child = tokio::process::Command::new(&shell_argv[0]);
    child.args(&shell_argv[1..]);
    apply_execution_policy(config, &mut child);
    let status = serde_json::json!({
        "type": "shell_script",
        "shell": shell_argv[0],
        "shell_source": "configured",
        "working_directory": config.execution.working_directory,
        "environment_policy": config.execution.environment_policy,
        "pty_policy": config.execution.pty_policy,
        "process_cleanup": config.execution.process_cleanup,
        "shell_argv_prefix_sha256_hex": sha256_hex(
            &serde_json::to_vec(&config.execution.shell_script_argv).unwrap_or_default(),
        ),
    });
    execute_child_with_output(
        job_id,
        child,
        timeout_secs,
        child_cleanup_policy(config),
        "shell_script",
        Some(status),
        output_tx,
    )
    .await
}

fn render_shell_script_argv(config: &AgentConfig, script: &str) -> Result<Vec<String>> {
    let mut argv = config.execution.shell_script_argv.clone();
    if argv.is_empty() {
        anyhow::bail!("shell script argv is empty");
    }
    if !argv[0].starts_with('/') {
        anyhow::bail!("shell script executable must be absolute");
    }
    argv.push(script.to_string());
    Ok(argv)
}

async fn execute_child_with_output(
    job_id: uuid::Uuid,
    child: tokio::process::Command,
    timeout_secs: u64,
    cleanup_policy: ChildCleanupPolicy,
    mode: &'static str,
    success_status: Option<serde_json::Value>,
    output_tx: Option<mpsc::Sender<CommandOutput>>,
) -> Result<Vec<CommandOutput>> {
    let timeout_secs = timeout_secs.max(1);
    let streaming = output_tx.is_some();
    let output = match output_tx {
        Some(sender) => {
            run_child_with_streaming_output(
                child,
                timeout_secs,
                MAX_COMMAND_OUTPUT_BYTES,
                cleanup_policy,
                ChildOutputSink { job_id, sender },
            )
            .await?
        }
        None => {
            run_child_with_bounded_output(
                child,
                timeout_secs,
                MAX_COMMAND_OUTPUT_BYTES,
                cleanup_policy,
            )
            .await?
        }
    };
    let output = match output {
        ChildRunResult::Completed(output) => output,
        ChildRunResult::TimedOut(cleanup) => {
            let status = serde_json::json!({
                "type": "command_timeout",
                "timeout_secs": timeout_secs,
                "mode": mode,
                "cleanup": cleanup,
            });
            return Ok(vec![CommandOutput {
                job_id,
                stream: OutputStream::Status,
                data: serde_json::to_vec(&status)?,
                exit_code: Some(124),
                done: true,
            }]);
        }
    };
    let mut outputs = Vec::new();
    if !streaming && !output.stdout.is_empty() {
        outputs.push(CommandOutput {
            job_id,
            stream: OutputStream::Stdout,
            data: output.stdout,
            exit_code: None,
            done: false,
        });
    }
    if !streaming && !output.stderr.is_empty() {
        outputs.push(CommandOutput {
            job_id,
            stream: OutputStream::Stderr,
            data: output.stderr,
            exit_code: None,
            done: false,
        });
    }
    outputs.push(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: match success_status {
            Some(status) => serde_json::to_vec(&status)?,
            None => Vec::new(),
        },
        exit_code: output.exit_code,
        done: true,
    });
    Ok(outputs)
}

async fn execute_pty_child_with_output(
    job_id: uuid::Uuid,
    child: tokio::process::Command,
    timeout_secs: u64,
    cleanup_policy: ChildCleanupPolicy,
    output_tx: Option<mpsc::Sender<CommandOutput>>,
) -> Result<Vec<CommandOutput>> {
    let timeout_secs = timeout_secs.max(1);
    let streaming = output_tx.is_some();
    let output = match output_tx {
        Some(sender) => {
            run_pty_with_streaming_output(
                child,
                timeout_secs,
                MAX_COMMAND_OUTPUT_BYTES,
                cleanup_policy,
                ChildOutputSink { job_id, sender },
            )
            .await?
        }
        None => {
            run_pty_with_bounded_output(
                child,
                timeout_secs,
                MAX_COMMAND_OUTPUT_BYTES,
                cleanup_policy,
            )
            .await?
        }
    };
    let output = match output {
        ChildRunResult::Completed(output) => output,
        ChildRunResult::TimedOut(cleanup) => {
            let status = serde_json::json!({
                "type": "command_timeout",
                "timeout_secs": timeout_secs,
                "mode": "shell_pty",
                "cleanup": cleanup,
            });
            return Ok(vec![CommandOutput {
                job_id,
                stream: OutputStream::Status,
                data: serde_json::to_vec(&status)?,
                exit_code: Some(124),
                done: true,
            }]);
        }
    };
    let mut outputs = Vec::new();
    if !streaming && !output.stdout.is_empty() {
        outputs.push(CommandOutput {
            job_id,
            stream: OutputStream::Pty,
            data: output.stdout,
            exit_code: None,
            done: false,
        });
    }
    outputs.push(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "shell_pty",
            "pty": true,
        }))?,
        exit_code: output.exit_code,
        done: true,
    });
    Ok(outputs)
}

fn validate_shell_script(script: &str) -> Result<()> {
    if script.trim().is_empty() {
        anyhow::bail!("shell script is empty");
    }
    if script.len() > MAX_SHELL_SCRIPT_BYTES {
        anyhow::bail!("shell script exceeds {} bytes", MAX_SHELL_SCRIPT_BYTES);
    }
    if script
        .chars()
        .any(|value| value.is_control() && !matches!(value, '\n' | '\r' | '\t'))
    {
        anyhow::bail!("shell script contains unsupported control characters");
    }
    Ok(())
}

async fn execute_user_sessions(
    config: &AgentConfig,
    job_id: uuid::Uuid,
    timeout_secs: u64,
) -> Result<Vec<CommandOutput>> {
    let (args, command_source, command_timeout_secs) =
        user_sessions_argv(config, timeout_secs.max(1))?;
    let mut outputs =
        execute_shell_command(config, job_id, &args, false, command_timeout_secs, None).await?;
    let exit_code = outputs
        .iter()
        .rev()
        .find(|output| output.done)
        .and_then(|output| output.exit_code);
    outputs.retain(|output| !(output.done && output.stream == OutputStream::Status));
    let status = serde_json::json!({
        "type": "user_sessions",
        "source": args[0],
        "command_source": command_source,
        "command_sha256_hex": sha256_hex(&serde_json::to_vec(&args).unwrap_or_default()),
    });
    outputs.push(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code,
        done: true,
    });
    Ok(outputs)
}

fn apply_execution_policy(config: &AgentConfig, command: &mut tokio::process::Command) {
    if let Some(working_directory) = &config.execution.working_directory {
        command.current_dir(working_directory);
    }
    match config.execution.environment_policy {
        AgentExecutionEnvironmentPolicy::Inherit => {}
        AgentExecutionEnvironmentPolicy::Clean => {
            command.env_clear();
            apply_kept_environment(config, command);
        }
        AgentExecutionEnvironmentPolicy::MinimalPath => {
            command.env_clear();
            command.env(
                "PATH",
                "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
            );
            apply_kept_environment(config, command);
        }
    }
    for (key, value) in &config.execution.environment_set {
        command.env(key, value);
    }
}

fn apply_kept_environment(config: &AgentConfig, command: &mut tokio::process::Command) {
    for key in &config.execution.environment_keep {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
}

fn ensure_pty_allowed(config: &AgentConfig) -> Result<()> {
    if config.execution.pty_policy == AgentExecutionPtyPolicy::Disabled {
        anyhow::bail!("execution PTY policy is disabled");
    }
    Ok(())
}

fn child_cleanup_policy(config: &AgentConfig) -> ChildCleanupPolicy {
    match config.execution.process_cleanup {
        AgentExecutionProcessCleanupPolicy::ProcessGroup => ChildCleanupPolicy::ProcessGroup,
        AgentExecutionProcessCleanupPolicy::DirectChild => ChildCleanupPolicy::DirectChild,
    }
}

fn user_sessions_argv(
    config: &AgentConfig,
    timeout_secs: u64,
) -> Result<(Vec<String>, &'static str, u64)> {
    if let Some(command) = &config.execution.user_sessions_command {
        if command.argv.is_empty() {
            anyhow::bail!("user sessions argv is empty");
        }
        if !command.argv[0].starts_with('/') {
            anyhow::bail!("user sessions executable must be absolute");
        }
        return Ok((
            command.argv.clone(),
            if config.execution.user_sessions_source
                == vpsman_common::AgentUserSessionsSource::CustomCommand
            {
                "custom_command"
            } else {
                "configured_linux_command"
            },
            command.timeout_secs.min(timeout_secs).clamp(1, 120),
        ));
    }
    if config.execution.user_sessions_source
        == vpsman_common::AgentUserSessionsSource::CustomCommand
    {
        anyhow::bail!("custom user sessions command is not configured");
    }
    if std::path::Path::new(PRESET_USER_SESSIONS_W).exists() {
        return Ok((
            vec![PRESET_USER_SESSIONS_W.to_string(), "-h".to_string()],
            "linux_w_who_preset",
            timeout_secs,
        ));
    }
    if std::path::Path::new(PRESET_USER_SESSIONS_WHO).exists() {
        return Ok((
            vec![PRESET_USER_SESSIONS_WHO.to_string()],
            "linux_w_who_preset",
            timeout_secs,
        ));
    }
    anyhow::bail!("neither w nor who is available")
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}
