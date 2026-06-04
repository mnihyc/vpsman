use anyhow::{Context, Result};

use crate::commands_terminal_sessions::{
    terminal_follow, terminal_replay_output, terminal_sessions_output, TerminalFollowRequest,
    TerminalReplayRequest,
};

pub(crate) fn is_vty_terminal_sessions_command(command: &str) -> bool {
    command == "terminal-sessions"
        || command.starts_with("terminal-sessions ")
        || command.starts_with("terminal-replay ")
        || command.starts_with("terminal-follow ")
}

pub(crate) fn submit_vty_terminal_sessions_command(
    api_url: &str,
    token: Option<&str>,
    command: &str,
) -> Result<String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    if parts.first().copied() == Some("terminal-replay") {
        return submit_vty_terminal_replay(api_url, token, &parts);
    }
    if parts.first().copied() == Some("terminal-follow") {
        submit_vty_terminal_follow(api_url, token, &parts)?;
        return Ok("{\"follow\":\"completed\"}".to_string());
    }
    let mut limit = 50_u16;
    let mut client_id = None;
    let mut session_id = None;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--limit" => {
                limit = parse_u16("--limit", parts.get(index + 1).copied(), 1, 200)?;
                index += 2;
            }
            value if value.starts_with("--limit=") => {
                limit = parse_u16("--limit", Some(value.trim_start_matches("--limit=")), 1, 200)?;
                index += 1;
            }
            "--client-id" => {
                client_id = Some(
                    parts
                        .get(index + 1)
                        .context("--client-id requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            value if value.starts_with("--client-id=") => {
                client_id = Some(value.trim_start_matches("--client-id=").to_string());
                index += 1;
            }
            "--session-id" => {
                session_id = Some(
                    parts
                        .get(index + 1)
                        .context("--session-id requires a UUID")?
                        .to_string(),
                );
                index += 2;
            }
            value if value.starts_with("--session-id=") => {
                session_id = Some(value.trim_start_matches("--session-id=").to_string());
                index += 1;
            }
            _ => anyhow::bail!(
                "usage: terminal-sessions [--limit <1-200>] [--client-id <id>] [--session-id <uuid>]"
            ),
        }
    }
    terminal_sessions_output(api_url, token, limit, client_id, session_id)
}

fn submit_vty_terminal_follow(api_url: &str, token: Option<&str>, parts: &[&str]) -> Result<()> {
    let mut client_id = None;
    let mut session_id = None;
    let mut from_seq = None;
    let mut interval_ms = 500_u64;
    let mut max_polls = 1_u32;
    let mut json = false;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--client-id" => {
                client_id = Some(
                    parts
                        .get(index + 1)
                        .context("--client-id requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--session-id" => {
                session_id = Some(
                    parts
                        .get(index + 1)
                        .context("--session-id requires a UUID")?
                        .to_string(),
                );
                index += 2;
            }
            "--from-seq" => {
                from_seq = Some(parse_u64("--from-seq", parts.get(index + 1).copied(), 1, u64::MAX)?);
                index += 2;
            }
            "--interval-ms" => {
                interval_ms = parse_u64("--interval-ms", parts.get(index + 1).copied(), 250, 10_000)?;
                index += 2;
            }
            "--max-polls" => {
                max_polls = u32::from(parse_u16("--max-polls", parts.get(index + 1).copied(), 1, 1000)?);
                index += 2;
            }
            "--json" => {
                json = true;
                index += 1;
            }
            _ => anyhow::bail!(
                "usage: terminal-follow --client-id <id> --session-id <uuid> [--from-seq <n>] [--interval-ms <250-10000>] [--max-polls <1-1000>] [--json]"
            ),
        }
    }
    terminal_follow(
        api_url,
        token,
        TerminalFollowRequest {
            client_id: client_id.context("--client-id is required")?,
            session_id: session_id.context("--session-id is required")?,
            from_seq,
            interval_ms,
            max_polls,
            json,
        },
    )
}

fn submit_vty_terminal_replay(
    api_url: &str,
    token: Option<&str>,
    parts: &[&str],
) -> Result<String> {
    let mut client_id = None;
    let mut session_id = None;
    let mut from_seq = None;
    let mut limit = 100_u16;
    let mut max_bytes = 4 * 1024 * 1024_u32;
    let mut output = None;
    let mut metadata_only = false;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--client-id" => {
                client_id = Some(
                    parts
                        .get(index + 1)
                        .context("--client-id requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--session-id" => {
                session_id = Some(
                    parts
                        .get(index + 1)
                        .context("--session-id requires a UUID")?
                        .to_string(),
                );
                index += 2;
            }
            "--from-seq" => {
                from_seq = Some(parse_u64("--from-seq", parts.get(index + 1).copied(), 1, u64::MAX)?);
                index += 2;
            }
            "--limit" => {
                limit = parse_u16("--limit", parts.get(index + 1).copied(), 1, 1000)?;
                index += 2;
            }
            "--max-bytes" => {
                max_bytes = parse_u32("--max-bytes", parts.get(index + 1).copied(), 1, 4 * 1024 * 1024)?;
                index += 2;
            }
            "--output" | "--output-file" => {
                output = Some(std::path::PathBuf::from(
                    parts
                        .get(index + 1)
                        .context("--output-file requires a path")?,
                ));
                index += 2;
            }
            "--metadata-only" => {
                metadata_only = true;
                index += 1;
            }
            _ => anyhow::bail!(
                "usage: terminal-replay --client-id <id> --session-id <uuid> [--from-seq <n>] [--limit <1-1000>] [--max-bytes <1-4194304>] [--output-file <file>] [--metadata-only]"
            ),
        }
    }
    terminal_replay_output(
        api_url,
        token,
        TerminalReplayRequest {
            client_id: client_id.context("--client-id is required")?,
            session_id: session_id.context("--session-id is required")?,
            from_seq,
            limit,
            max_bytes,
            output_file: output,
            metadata_only,
        },
    )
}

fn parse_u16(label: &str, value: Option<&str>, min: u16, max: u16) -> Result<u16> {
    let value = value
        .context(format!("{label} requires a value"))?
        .parse::<u16>()
        .with_context(|| format!("{label} must be an integer"))?;
    anyhow::ensure!(
        (min..=max).contains(&value),
        "{label} must be between {min} and {max}"
    );
    Ok(value)
}

fn parse_u32(label: &str, value: Option<&str>, min: u32, max: u32) -> Result<u32> {
    let value = value
        .context(format!("{label} requires a value"))?
        .parse::<u32>()
        .with_context(|| format!("{label} must be an integer"))?;
    anyhow::ensure!(
        (min..=max).contains(&value),
        "{label} must be between {min} and {max}"
    );
    Ok(value)
}

fn parse_u64(label: &str, value: Option<&str>, min: u64, max: u64) -> Result<u64> {
    let value = value
        .context(format!("{label} requires a value"))?
        .parse::<u64>()
        .with_context(|| format!("{label} must be an integer"))?;
    anyhow::ensure!(
        (min..=max).contains(&value),
        "{label} must be between {min} and {max}"
    );
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::is_vty_terminal_sessions_command;

    #[test]
    fn recognizes_terminal_sessions_commands() {
        assert!(is_vty_terminal_sessions_command("terminal-sessions"));
        assert!(is_vty_terminal_sessions_command(
            "terminal-sessions --limit 10"
        ));
        assert!(is_vty_terminal_sessions_command(
            "terminal-replay --client-id edge-a --session-id 11111111-2222-4333-8444-555555555555"
        ));
        assert!(is_vty_terminal_sessions_command(
            "terminal-follow --client-id edge-a --session-id 11111111-2222-4333-8444-555555555555"
        ));
        assert!(!is_vty_terminal_sessions_command(
            "terminal-open --argv /bin/sh"
        ));
    }
}
