use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use uuid::Uuid;
use vpsman_common::{
    default_terminal_flow_window_bytes, default_terminal_idle_timeout_secs, JobCommand,
    TerminalControlAction, DEFAULT_MAX_JOB_TIMEOUT_SECS, MAX_CONFIGURABLE_JOB_TIMEOUT_SECS,
};

use crate::{
    commands_terminal::{terminal_control_output, validate_terminal_client_id},
    commands_terminal_sessions::{terminal_replay_output, TerminalReplayRequest},
    vty_jobs::{vty_submit_operation, VtyJobSelection, VtyPrivilegeContext},
};

pub(crate) fn is_vty_terminal_command(command: &str) -> bool {
    command.starts_with("terminal-open ")
        || command.starts_with("terminal-input ")
        || command.starts_with("terminal-poll ")
        || command.starts_with("terminal-resize ")
        || command.starts_with("terminal-close ")
}

pub(crate) fn submit_vty_terminal_command(
    api_url: &str,
    token: Option<&str>,
    privilege_context: &VtyPrivilegeContext,
    command: &str,
) -> Result<String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let verb = parts
        .first()
        .copied()
        .context("terminal command is empty")?;
    let parsed = parse_vty_terminal(verb, &parts[1..])?;
    match parsed {
        VtyTerminalRequest::Job {
            command_label,
            operation,
            selection,
            max_timeout_secs,
        } => vty_submit_operation(
            api_url,
            token,
            privilege_context,
            command_label,
            &operation,
            selection,
            max_timeout_secs,
        ),
        VtyTerminalRequest::Control {
            client_id,
            session_id,
            action,
        } => terminal_control_output(api_url, token, &client_id, session_id, action),
        VtyTerminalRequest::Replay {
            client_id,
            session_id,
            from_seq,
        } => terminal_replay_output(
            api_url,
            token,
            TerminalReplayRequest {
                client_id,
                session_id: session_id.to_string(),
                from_seq,
                limit: 100,
                max_bytes: 4 * 1024 * 1024,
                output_file: None,
                metadata_only: false,
            },
        ),
    }
}

#[derive(Debug)]
enum VtyTerminalRequest {
    Job {
        command_label: &'static str,
        operation: Box<JobCommand>,
        selection: VtyJobSelection,
        max_timeout_secs: u64,
    },
    Control {
        client_id: String,
        session_id: Uuid,
        action: TerminalControlAction,
    },
    Replay {
        client_id: String,
        session_id: Uuid,
        from_seq: Option<u64>,
    },
}

fn parse_vty_terminal(verb: &str, args: &[&str]) -> Result<VtyTerminalRequest> {
    match verb {
        "terminal-open" => parse_terminal_open(args),
        "terminal-input" => parse_terminal_input(args),
        "terminal-poll" => parse_terminal_poll(args),
        "terminal-resize" => parse_terminal_resize(args),
        "terminal-close" => parse_terminal_close(args),
        _ => anyhow::bail!("unknown terminal command {verb}"),
    }
}

fn parse_terminal_open(args: &[&str]) -> Result<VtyTerminalRequest> {
    let mut session_id = None;
    let mut argv = Vec::new();
    let mut cwd = None;
    let mut cols = 120;
    let mut rows = 40;
    let mut replay_from_seq = None;
    let mut idle_timeout_secs = default_terminal_idle_timeout_secs();
    let mut flow_window_bytes = default_terminal_flow_window_bytes();
    let mut max_timeout_secs = DEFAULT_MAX_JOB_TIMEOUT_SECS;
    let mut targets = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "--session-id" => {
                index += 1;
                session_id = Some(parse_uuid(args.get(index), "--session-id")?);
            }
            "--argv" => {
                index += 1;
                argv = split_csv(args.get(index).context("--argv requires a value")?);
            }
            "--cwd" => {
                index += 1;
                cwd = Some(
                    args.get(index)
                        .context("--cwd requires a value")?
                        .to_string(),
                );
            }
            "--cols" => {
                index += 1;
                cols = parse_value(args.get(index), "--cols")?;
            }
            "--rows" => {
                index += 1;
                rows = parse_value(args.get(index), "--rows")?;
            }
            "--replay-from-seq" => {
                index += 1;
                replay_from_seq = Some(parse_value(args.get(index), "--replay-from-seq")?);
            }
            "--idle-timeout-secs" => {
                index += 1;
                idle_timeout_secs = parse_value(args.get(index), "--idle-timeout-secs")?;
            }
            "--flow-window-bytes" => {
                index += 1;
                flow_window_bytes = parse_value(args.get(index), "--flow-window-bytes")?;
            }
            "--max-timeout" => {
                index += 1;
                max_timeout_secs = parse_timeout(args.get(index))?;
            }
            value => targets.push(value),
        }
        index += 1;
    }
    anyhow::ensure!(!argv.is_empty(), "terminal-open requires --argv <abs,argv>");
    let selection = VtyJobSelection::parse(&targets)?;
    Ok(VtyTerminalRequest::Job {
        command_label: "terminal_open",
        operation: Box::new(JobCommand::TerminalOpen {
            session_id: session_id.unwrap_or_else(Uuid::new_v4),
            argv,
            cwd,
            user: None,
            user_policy: vpsman_common::TerminalUserPolicy::Fail,
            cols,
            rows,
            replay_from_seq,
            idle_timeout_secs,
            flow_window_bytes,
        }),
        selection,
        max_timeout_secs,
    })
}

fn parse_terminal_input(args: &[&str]) -> Result<VtyTerminalRequest> {
    let mut client_id = None;
    let mut session_id = None;
    let mut data_base64 = None;
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "--client-id" => {
                index += 1;
                client_id = Some(
                    args.get(index)
                        .context("--client-id requires a value")?
                        .to_string(),
                );
            }
            "--session-id" => {
                index += 1;
                session_id = Some(parse_uuid(args.get(index), "--session-id")?);
            }
            "--text" => {
                index += 1;
                anyhow::ensure!(
                    data_base64.is_none(),
                    "use either --text or --data-base64, not both"
                );
                data_base64 =
                    Some(BASE64.encode(*args.get(index).context("--text requires a value")?));
            }
            "--data-base64" => {
                index += 1;
                anyhow::ensure!(
                    data_base64.is_none(),
                    "use either --text or --data-base64, not both"
                );
                data_base64 = Some(
                    args.get(index)
                        .context("--data-base64 requires a value")?
                        .to_string(),
                );
            }
            "--input-seq" => anyhow::bail!("terminal-input no longer accepts --input-seq"),
            value => anyhow::bail!("unknown terminal-input argument {value}"),
        }
        index += 1;
    }
    let client_id = client_id.context("terminal-input requires --client-id")?;
    validate_terminal_client_id(&client_id)?;
    Ok(VtyTerminalRequest::Control {
        client_id,
        session_id: session_id.context("terminal-input requires --session-id")?,
        action: TerminalControlAction::Input {
            data_base64: data_base64.context("terminal-input requires --text or --data-base64")?,
        },
    })
}

fn parse_terminal_poll(args: &[&str]) -> Result<VtyTerminalRequest> {
    let mut client_id = None;
    let mut session_id = None;
    let mut replay_from_seq = None;
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "--client-id" => {
                index += 1;
                client_id = Some(
                    args.get(index)
                        .context("--client-id requires a value")?
                        .to_string(),
                );
            }
            "--session-id" => {
                index += 1;
                session_id = Some(parse_uuid(args.get(index), "--session-id")?);
            }
            "--replay-from-seq" => {
                index += 1;
                replay_from_seq = Some(parse_value(args.get(index), "--replay-from-seq")?);
            }
            value => anyhow::bail!("unknown terminal-poll argument {value}"),
        }
        index += 1;
    }
    let client_id = client_id.context("terminal-poll requires --client-id")?;
    validate_terminal_client_id(&client_id)?;
    Ok(VtyTerminalRequest::Replay {
        client_id,
        session_id: session_id.context("terminal-poll requires --session-id")?,
        from_seq: replay_from_seq,
    })
}

fn parse_terminal_resize(args: &[&str]) -> Result<VtyTerminalRequest> {
    let mut client_id = None;
    let mut session_id = None;
    let mut cols = None;
    let mut rows = None;
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "--client-id" => {
                index += 1;
                client_id = Some(
                    args.get(index)
                        .context("--client-id requires a value")?
                        .to_string(),
                );
            }
            "--session-id" => {
                index += 1;
                session_id = Some(parse_uuid(args.get(index), "--session-id")?);
            }
            "--cols" => {
                index += 1;
                cols = Some(parse_value(args.get(index), "--cols")?);
            }
            "--rows" => {
                index += 1;
                rows = Some(parse_value(args.get(index), "--rows")?);
            }
            value => anyhow::bail!("unknown terminal-resize argument {value}"),
        }
        index += 1;
    }
    let client_id = client_id.context("terminal-resize requires --client-id")?;
    validate_terminal_client_id(&client_id)?;
    Ok(VtyTerminalRequest::Control {
        client_id,
        session_id: session_id.context("terminal-resize requires --session-id")?,
        action: TerminalControlAction::Resize {
            cols: cols.context("terminal-resize requires --cols")?,
            rows: rows.context("terminal-resize requires --rows")?,
        },
    })
}

fn parse_terminal_close(args: &[&str]) -> Result<VtyTerminalRequest> {
    let mut client_id = None;
    let mut session_id = None;
    let mut reason = None;
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "--client-id" => {
                index += 1;
                client_id = Some(
                    args.get(index)
                        .context("--client-id requires a value")?
                        .to_string(),
                );
            }
            "--session-id" => {
                index += 1;
                session_id = Some(parse_uuid(args.get(index), "--session-id")?);
            }
            "--reason" => {
                index += 1;
                reason = Some(
                    args.get(index)
                        .context("--reason requires a value")?
                        .to_string(),
                );
            }
            value => anyhow::bail!("unknown terminal-close argument {value}"),
        }
        index += 1;
    }
    let client_id = client_id.context("terminal-close requires --client-id")?;
    validate_terminal_client_id(&client_id)?;
    Ok(VtyTerminalRequest::Control {
        client_id,
        session_id: session_id.context("terminal-close requires --session-id")?,
        action: TerminalControlAction::Close { reason },
    })
}

fn parse_uuid(value: Option<&&str>, name: &str) -> Result<Uuid> {
    Uuid::parse_str(value.context(format!("{name} requires a value"))?)
        .with_context(|| format!("{name} must be a UUID"))
}

fn parse_value<T>(value: Option<&&str>, name: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .context(format!("{name} requires a value"))?
        .parse::<T>()
        .map_err(|error| anyhow::anyhow!("{name} has invalid value: {error}"))
}

fn parse_timeout(value: Option<&&str>) -> Result<u64> {
    let timeout = parse_value::<u64>(value, "--max-timeout")?;
    anyhow::ensure!(
        (1..=MAX_CONFIGURABLE_JOB_TIMEOUT_SECS).contains(&timeout),
        "--max-timeout must be between 1 and {MAX_CONFIGURABLE_JOB_TIMEOUT_SECS}"
    );
    Ok(timeout)
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect()
}

#[cfg(test)]
#[path = "tests_vty_terminal.rs"]
mod tests;
