use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use vpsman_common::{
    encode_chunked_file_payload, encode_inline_file_payload, payload_hash,
    validate_absolute_file_path, validate_file_mode, JobCommand, MAX_CHUNKED_FILE_PUSH_BYTES,
    MAX_INLINE_FILE_PUSH_BYTES,
};

use crate::vty_jobs::VtyJobSelection;

#[derive(Debug)]
pub(crate) struct VtyFileOperation {
    pub(crate) command_label: &'static str,
    pub(crate) operation: JobCommand,
    pub(crate) selection: VtyJobSelection,
    pub(crate) timeout_secs: u64,
}

pub(crate) fn parse_vty_file_pull(tokens: &[&str]) -> Result<VtyFileOperation> {
    let mut path = None;
    let mut timeout_secs = 30_u64;
    let mut target_tokens = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--path" => {
                path = Some(
                    tokens
                        .get(index + 1)
                        .context("--path requires an absolute path")?
                        .to_string(),
                );
                index += 2;
            }
            value if value.starts_with("--path=") => {
                path = Some(value.trim_start_matches("--path=").to_string());
                index += 1;
            }
            "--timeout" => {
                timeout_secs = parse_timeout(tokens.get(index + 1).copied())?;
                index += 2;
            }
            value if value.starts_with("--timeout=") => {
                timeout_secs = parse_timeout(Some(value.trim_start_matches("--timeout=")))?;
                index += 1;
            }
            value => {
                target_tokens.push(value);
                index += 1;
            }
        }
    }
    let path = path.context("--path is required")?;
    validate_absolute_file_path(&path).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(VtyFileOperation {
        command_label: "file_pull",
        operation: JobCommand::FilePull { path },
        selection: VtyJobSelection::parse(&target_tokens)?,
        timeout_secs,
    })
}

pub(crate) fn parse_vty_file_push(tokens: &[&str]) -> Result<VtyFileOperation> {
    let mut source = None;
    let mut path = None;
    let mut mode = 0o644_u32;
    let mut timeout_secs = 30_u64;
    let mut target_tokens = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--source" => {
                source = Some(PathBuf::from(
                    tokens
                        .get(index + 1)
                        .context("--source requires a local file path")?,
                ));
                index += 2;
            }
            value if value.starts_with("--source=") => {
                source = Some(PathBuf::from(value.trim_start_matches("--source=")));
                index += 1;
            }
            "--path" => {
                path = Some(
                    tokens
                        .get(index + 1)
                        .context("--path requires a remote absolute path")?
                        .to_string(),
                );
                index += 2;
            }
            value if value.starts_with("--path=") => {
                path = Some(value.trim_start_matches("--path=").to_string());
                index += 1;
            }
            "--mode" => {
                mode = parse_mode(tokens.get(index + 1).copied())?;
                index += 2;
            }
            value if value.starts_with("--mode=") => {
                mode = parse_mode(Some(value.trim_start_matches("--mode=")))?;
                index += 1;
            }
            "--timeout" => {
                timeout_secs = parse_timeout(tokens.get(index + 1).copied())?;
                index += 2;
            }
            value if value.starts_with("--timeout=") => {
                timeout_secs = parse_timeout(Some(value.trim_start_matches("--timeout=")))?;
                index += 1;
            }
            value => {
                target_tokens.push(value);
                index += 1;
            }
        }
    }
    let source = source.context("--source is required")?;
    let path = path.context("--path is required")?;
    validate_absolute_file_path(&path).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    validate_file_mode(mode).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let selection = VtyJobSelection::parse(&target_tokens)?;
    anyhow::ensure!(
        selection.confirmed,
        "file-push requires --confirmed because it writes a remote file"
    );
    let data = read_source(&source)?;
    let (command_label, operation) = build_file_push_operation(path, mode, &data)?;
    Ok(VtyFileOperation {
        command_label,
        operation,
        selection,
        timeout_secs,
    })
}

fn read_source(source: &PathBuf) -> Result<Vec<u8>> {
    let metadata = fs::metadata(source)
        .with_context(|| format!("failed to stat source {}", source.display()))?;
    anyhow::ensure!(
        metadata.is_file(),
        "file-push source must be a regular file"
    );
    anyhow::ensure!(
        metadata.len() <= MAX_CHUNKED_FILE_PUSH_BYTES as u64,
        "file-push source exceeds chunked transfer limit: {} > {} bytes",
        metadata.len(),
        MAX_CHUNKED_FILE_PUSH_BYTES
    );
    fs::read(source).with_context(|| format!("failed to read source {}", source.display()))
}

fn build_file_push_operation(
    path: String,
    mode: u32,
    data: &[u8],
) -> Result<(&'static str, JobCommand)> {
    let data_hash = payload_hash(data);
    if data.len() <= MAX_INLINE_FILE_PUSH_BYTES {
        Ok((
            "file_push",
            JobCommand::FilePush {
                path,
                mode,
                size_bytes: data.len() as u64,
                sha256_hex: data_hash,
                data_base64: encode_inline_file_payload(data)?,
            },
        ))
    } else {
        Ok((
            "file_push_chunked",
            JobCommand::FilePushChunked {
                path,
                mode,
                size_bytes: data.len() as u64,
                sha256_hex: data_hash,
                chunks: encode_chunked_file_payload(data)?,
            },
        ))
    }
}

fn parse_timeout(value: Option<&str>) -> Result<u64> {
    let value = value.context("--timeout requires a value between 1 and 3600")?;
    let timeout = value
        .parse::<u64>()
        .context("--timeout must be an integer")?;
    anyhow::ensure!(
        (1..=3600).contains(&timeout),
        "--timeout must be between 1 and 3600"
    );
    Ok(timeout)
}

fn parse_mode(value: Option<&str>) -> Result<u32> {
    let value = value.context("--mode requires a value")?.trim();
    let (radix, digits) = if let Some(rest) = value.strip_prefix("0o") {
        (8, rest)
    } else if value.starts_with('0') {
        (8, value)
    } else {
        (10, value)
    };
    let mode = u32::from_str_radix(digits, radix).context("--mode is not a valid number")?;
    validate_file_mode(mode).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_FILE_PULL_PATH: &str = "/etc/hostname";

    #[test]
    fn parses_vty_file_pull() {
        let request =
            parse_vty_file_pull(&["--path", TEST_FILE_PULL_PATH, "client:edge-a"]).unwrap();
        assert_eq!(request.command_label, "file_pull");
        assert_eq!(request.selection.clients, vec!["edge-a"]);
        assert!(matches!(request.operation, JobCommand::FilePull { .. }));
    }

    #[test]
    fn parses_vty_file_push() {
        let source = std::env::temp_dir().join(format!("vpsman-vty-push-{}", uuid::Uuid::new_v4()));
        fs::write(&source, b"payload").unwrap();
        let request = parse_vty_file_push(&[
            "--source",
            source.to_str().unwrap(),
            "--path",
            "/tmp/payload",
            "--mode",
            "0600",
            "client:edge-a",
            "--confirmed",
        ])
        .unwrap();

        assert_eq!(request.command_label, "file_push");
        assert_eq!(request.selection.clients, vec!["edge-a"]);
        match request.operation {
            JobCommand::FilePush {
                mode, size_bytes, ..
            } => {
                assert_eq!(mode, 0o600);
                assert_eq!(size_bytes, 7);
            }
            other => panic!("unexpected command: {other:?}"),
        }
        let _ = fs::remove_file(source);
    }

    #[test]
    fn parses_vty_chunked_file_push_for_large_source() {
        let source = std::env::temp_dir().join(format!("vpsman-vty-push-{}", uuid::Uuid::new_v4()));
        fs::write(&source, vec![9_u8; MAX_INLINE_FILE_PUSH_BYTES + 1]).unwrap();
        let request = parse_vty_file_push(&[
            "--source",
            source.to_str().unwrap(),
            "--path",
            "/tmp/payload",
            "client:edge-a",
            "--confirmed",
        ])
        .unwrap();

        assert_eq!(request.command_label, "file_push_chunked");
        match request.operation {
            JobCommand::FilePushChunked {
                size_bytes, chunks, ..
            } => {
                assert_eq!(size_bytes, (MAX_INLINE_FILE_PUSH_BYTES + 1) as u64);
                assert!(chunks.len() > 1);
            }
            other => panic!("unexpected command: {other:?}"),
        }
        let _ = fs::remove_file(source);
    }

    #[test]
    fn rejects_vty_file_push_without_confirmation_or_absolute_path() {
        let source = std::env::temp_dir().join(format!("vpsman-vty-push-{}", uuid::Uuid::new_v4()));
        fs::write(&source, b"payload").unwrap();
        assert!(parse_vty_file_push(&[
            "--source",
            source.to_str().unwrap(),
            "--path",
            "/tmp/payload",
            "client:edge-a",
        ])
        .is_err());
        assert!(parse_vty_file_push(&[
            "--source",
            source.to_str().unwrap(),
            "--path",
            "relative",
            "client:edge-a",
            "--confirmed",
        ])
        .is_err());
        let _ = fs::remove_file(source);
    }
}
