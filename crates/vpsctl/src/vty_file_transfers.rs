use anyhow::{Context, Result};

use std::path::PathBuf;

use crate::commands_file_transfers::{file_transfer_sources_output, file_transfers_output};

pub(crate) fn is_vty_file_transfers_command(command: &str) -> bool {
    command == "file-transfers"
        || command.starts_with("file-transfers ")
        || command.starts_with("file-transfer-handoff ")
        || command == "file-transfer-sources"
        || command.starts_with("file-transfer-sources ")
        || command.starts_with("file-transfer-source-upload ")
        || command.starts_with("file-transfer-source-download ")
}

pub(crate) fn submit_vty_file_transfers_command(
    api_url: &str,
    token: Option<&str>,
    command: &str,
) -> Result<String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    match parts.first().copied() {
        Some("file-transfer-handoff") => {
            return submit_vty_file_transfer_handoff(api_url, token, &parts);
        }
        Some("file-transfer-sources") => {
            return submit_vty_file_transfer_sources(api_url, token, &parts);
        }
        Some("file-transfer-source-upload") => {
            return submit_vty_file_transfer_source_upload(api_url, token, &parts);
        }
        Some("file-transfer-source-download") => {
            return submit_vty_file_transfer_source_download(api_url, token, &parts);
        }
        _ => {}
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
                limit = parse_u16(
                    "--limit",
                    Some(value.trim_start_matches("--limit=")),
                    1,
                    200,
                )?;
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
                "usage: file-transfers [--limit <1-200>] [--client-id <id>] [--session-id <uuid>]"
            ),
        }
    }
    file_transfers_output(api_url, token, limit, client_id, session_id)
}

fn submit_vty_file_transfer_sources(
    api_url: &str,
    token: Option<&str>,
    parts: &[&str],
) -> Result<String> {
    let mut limit = 50_u16;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--limit" => {
                limit = parse_u16("--limit", parts.get(index + 1).copied(), 1, 200)?;
                index += 2;
            }
            value if value.starts_with("--limit=") => {
                limit = parse_u16(
                    "--limit",
                    Some(value.trim_start_matches("--limit=")),
                    1,
                    200,
                )?;
                index += 1;
            }
            _ => anyhow::bail!("usage: file-transfer-sources [--limit <1-200>]"),
        }
    }
    file_transfer_sources_output(api_url, token, limit)
}

fn submit_vty_file_transfer_handoff(
    api_url: &str,
    token: Option<&str>,
    parts: &[&str],
) -> Result<String> {
    let mut client_id = None;
    let mut session_id = None;
    let mut output = None;
    let mut confirmed = false;
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
            "--output" | "--output-file" => {
                output = Some(std::path::PathBuf::from(
                    parts
                        .get(index + 1)
                        .context("--output-file requires a path")?,
                ));
                index += 2;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            _ => anyhow::bail!(
                "usage: file-transfer-handoff --client-id <id> --session-id <uuid> [--output-file <file>] --confirmed"
            ),
        }
    }
    let client_id = client_id.context("--client-id is required")?;
    let session_id = session_id.context("--session-id is required")?;
    crate::commands_file_transfers::file_transfer_handoff_output(
        api_url, token, client_id, session_id, output, confirmed,
    )
}

fn submit_vty_file_transfer_source_upload(
    api_url: &str,
    token: Option<&str>,
    parts: &[&str],
) -> Result<String> {
    let mut source = None;
    let mut name = None;
    let mut confirmed = false;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--source" => {
                source = Some(PathBuf::from(
                    parts.get(index + 1).context("--source requires a path")?,
                ));
                index += 2;
            }
            "--name" => {
                name = Some(
                    parts
                        .get(index + 1)
                        .context("--name requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            _ => anyhow::bail!(
                "usage: file-transfer-source-upload --source <file> [--name <name>] --confirmed"
            ),
        }
    }
    crate::commands_file_transfers::file_transfer_source_upload_output(
        api_url,
        token,
        source.context("--source is required")?,
        name,
        confirmed,
    )
}

fn submit_vty_file_transfer_source_download(
    api_url: &str,
    token: Option<&str>,
    parts: &[&str],
) -> Result<String> {
    let mut artifact_id = None;
    let mut output = None;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--artifact-id" => {
                artifact_id = Some(
                    parts
                        .get(index + 1)
                        .context("--artifact-id requires a UUID")?
                        .to_string(),
                );
                index += 2;
            }
            "--output" | "--output-file" => {
                output = Some(PathBuf::from(
                    parts
                        .get(index + 1)
                        .context("--output-file requires a path")?,
                ));
                index += 2;
            }
            _ => anyhow::bail!(
                "usage: file-transfer-source-download --artifact-id <uuid> --output-file <file>"
            ),
        }
    }
    crate::commands_file_transfers::file_transfer_source_download_output(
        api_url,
        token,
        artifact_id.context("--artifact-id is required")?,
        output.context("--output-file is required")?,
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

#[cfg(test)]
mod tests {
    use super::is_vty_file_transfers_command;

    #[test]
    fn recognizes_file_transfers_commands() {
        assert!(is_vty_file_transfers_command("file-transfers"));
        assert!(is_vty_file_transfers_command("file-transfers --limit 10"));
        assert!(is_vty_file_transfers_command(
            "file-transfer-handoff --client-id edge-a --session-id 11111111-2222-4333-8444-555555555555 --confirmed"
        ));
        assert!(is_vty_file_transfers_command("file-transfer-sources"));
        assert!(is_vty_file_transfers_command(
            "file-transfer-source-upload --source ./payload.bin --confirmed"
        ));
        assert!(is_vty_file_transfers_command(
            "file-transfer-source-download --artifact-id 11111111-2222-4333-8444-555555555555 --output-file ./payload.bin"
        ));
        assert!(!is_vty_file_transfers_command(
            "file-transfer-upload --path /tmp/a"
        ));
    }
}
