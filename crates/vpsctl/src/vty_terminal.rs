use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use uuid::Uuid;
use vpsman_common::{
    default_terminal_flow_window_bytes, default_terminal_idle_timeout_secs, JobCommand,
};

use crate::vty_jobs::{vty_submit_operation, VtyJobSelection, VtyPrivilegeContext};

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
    vty_submit_operation(
        api_url,
        token,
        privilege_context,
        parsed.command_label,
        &parsed.operation,
        parsed.selection,
        parsed.timeout_secs,
    )
}

struct VtyTerminalRequest {
    command_label: &'static str,
    operation: JobCommand,
    selection: VtyJobSelection,
    timeout_secs: u64,
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
    let mut timeout_secs = 30;
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
            "--timeout" => {
                index += 1;
                timeout_secs = parse_timeout(args.get(index))?;
            }
            value => targets.push(value),
        }
        index += 1;
    }
    anyhow::ensure!(!argv.is_empty(), "terminal-open requires --argv <abs,argv>");
    let selection = VtyJobSelection::parse(&targets)?;
    Ok(VtyTerminalRequest {
        command_label: "terminal_open",
        operation: JobCommand::TerminalOpen {
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
        },
        selection,
        timeout_secs,
    })
}

fn parse_terminal_input(args: &[&str]) -> Result<VtyTerminalRequest> {
    let mut session_id = None;
    let mut input_seq = None;
    let mut data_base64 = None;
    let mut timeout_secs = 30;
    let mut targets = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "--session-id" => {
                index += 1;
                session_id = Some(parse_uuid(args.get(index), "--session-id")?);
            }
            "--input-seq" => {
                index += 1;
                input_seq = Some(parse_value(args.get(index), "--input-seq")?);
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
            "--timeout" => {
                index += 1;
                timeout_secs = parse_timeout(args.get(index))?;
            }
            value => targets.push(value),
        }
        index += 1;
    }
    let selection = VtyJobSelection::parse(&targets)?;
    let input_seq = input_seq.context("terminal-input requires --input-seq")?;
    anyhow::ensure!(input_seq >= 1, "--input-seq must be at least 1");
    Ok(VtyTerminalRequest {
        command_label: "terminal_input",
        operation: JobCommand::TerminalInput {
            session_id: session_id.context("terminal-input requires --session-id")?,
            input_seq,
            data_base64: data_base64.context("terminal-input requires --text or --data-base64")?,
        },
        selection,
        timeout_secs,
    })
}

fn parse_terminal_poll(args: &[&str]) -> Result<VtyTerminalRequest> {
    let mut session_id = None;
    let mut replay_from_seq = None;
    let mut timeout_secs = 30;
    let mut targets = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "--session-id" => {
                index += 1;
                session_id = Some(parse_uuid(args.get(index), "--session-id")?);
            }
            "--replay-from-seq" => {
                index += 1;
                replay_from_seq = Some(parse_value(args.get(index), "--replay-from-seq")?);
            }
            "--timeout" => {
                index += 1;
                timeout_secs = parse_timeout(args.get(index))?;
            }
            value => targets.push(value),
        }
        index += 1;
    }
    let selection = VtyJobSelection::parse(&targets)?;
    Ok(VtyTerminalRequest {
        command_label: "terminal_poll",
        operation: JobCommand::TerminalPoll {
            session_id: session_id.context("terminal-poll requires --session-id")?,
            replay_from_seq,
        },
        selection,
        timeout_secs,
    })
}

fn parse_terminal_resize(args: &[&str]) -> Result<VtyTerminalRequest> {
    let mut session_id = None;
    let mut cols = None;
    let mut rows = None;
    let mut timeout_secs = 30;
    let mut targets = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index] {
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
            "--timeout" => {
                index += 1;
                timeout_secs = parse_timeout(args.get(index))?;
            }
            value => targets.push(value),
        }
        index += 1;
    }
    let selection = VtyJobSelection::parse(&targets)?;
    Ok(VtyTerminalRequest {
        command_label: "terminal_resize",
        operation: JobCommand::TerminalResize {
            session_id: session_id.context("terminal-resize requires --session-id")?,
            cols: cols.context("terminal-resize requires --cols")?,
            rows: rows.context("terminal-resize requires --rows")?,
        },
        selection,
        timeout_secs,
    })
}

fn parse_terminal_close(args: &[&str]) -> Result<VtyTerminalRequest> {
    let mut session_id = None;
    let mut reason = None;
    let mut timeout_secs = 30;
    let mut targets = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index] {
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
            "--timeout" => {
                index += 1;
                timeout_secs = parse_timeout(args.get(index))?;
            }
            value => targets.push(value),
        }
        index += 1;
    }
    let selection = VtyJobSelection::parse(&targets)?;
    Ok(VtyTerminalRequest {
        command_label: "terminal_close",
        operation: JobCommand::TerminalClose {
            session_id: session_id.context("terminal-close requires --session-id")?,
            reason,
        },
        selection,
        timeout_secs,
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
    Ok(parse_value::<u64>(value, "--timeout")?.clamp(1, 3600))
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
mod tests {
    use super::parse_vty_terminal;
    use vpsman_common::JobCommand;

    const TEST_TERMINAL_ARGV: &str = "/bin/sh,-l";

    #[test]
    fn parses_terminal_open_contract() {
        let request = parse_vty_terminal(
            "terminal-open",
            &[
                "--argv",
                TEST_TERMINAL_ARGV,
                "--cols",
                "100",
                "--rows",
                "30",
                "id:edge-a",
                "--confirmed",
            ],
        )
        .unwrap();
        assert_eq!(request.command_label, "terminal_open");
        assert!(request.selection.clients.is_empty());
        assert_eq!(request.selection.tags, vec!["id:edge-a".to_string()]);
        assert!(request.selection.confirmed);
        match request.operation {
            JobCommand::TerminalOpen {
                argv, cols, rows, ..
            } => {
                assert_eq!(argv, vec!["/bin/sh".to_string(), "-l".to_string()]);
                assert_eq!(cols, 100);
                assert_eq!(rows, 30);
            }
            other => panic!("unexpected operation {other:?}"),
        }
    }

    #[test]
    fn parses_terminal_input_text() {
        let request = parse_vty_terminal(
            "terminal-input",
            &[
                "--session-id",
                "11111111-1111-4111-8111-111111111111",
                "--input-seq",
                "7",
                "--text",
                "id",
                "id:edge-a",
            ],
        )
        .unwrap();
        match request.operation {
            JobCommand::TerminalInput {
                input_seq,
                data_base64,
                ..
            } => {
                assert_eq!(input_seq, 7);
                assert_eq!(data_base64, "aWQ=");
            }
            other => panic!("unexpected operation {other:?}"),
        }
    }

    #[test]
    fn parses_terminal_poll() {
        let request = parse_vty_terminal(
            "terminal-poll",
            &[
                "--session-id",
                "11111111-1111-4111-8111-111111111111",
                "--replay-from-seq",
                "4",
                "id:edge-a",
            ],
        )
        .unwrap();
        assert_eq!(request.command_label, "terminal_poll");
        match request.operation {
            JobCommand::TerminalPoll {
                replay_from_seq, ..
            } => assert_eq!(replay_from_seq, Some(4)),
            other => panic!("unexpected operation {other:?}"),
        }
    }
}
