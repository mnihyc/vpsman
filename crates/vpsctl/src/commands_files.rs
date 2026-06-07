use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use vpsman_common::{
    encode_chunked_file_payload, encode_inline_file_payload, payload_hash,
    validate_absolute_file_path, validate_file_mode, FileExistingPolicy, FileOwnershipPolicy,
    JobCommand, MAX_CHUNKED_FILE_PUSH_BYTES, MAX_INLINE_FILE_PUSH_BYTES,
};

use crate::jobs::{submit_privileged_operation, PrivilegedOperationRequest};

#[allow(clippy::too_many_arguments)]
pub(crate) fn file_pull(
    api_url: &str,
    token: Option<&str>,
    path: String,
    clients: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    proof_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
) -> Result<()> {
    anyhow::ensure!(path.starts_with('/'), "file pull path must be absolute");
    let operation = JobCommand::FilePull { path: path.clone() };
    println!(
        "{}",
        submit_privileged_operation(PrivilegedOperationRequest {
            api_url,
            token,
            operation: &operation,
            command_label: "file_pull",
            clients: &clients,
            tags: &tags,
            password_env: &password_env,
            super_salt_hex: super_salt_hex.as_deref(),
            proof_ttl_secs,
            timeout_secs,
            confirmed,
            force_unprivileged: false,
        })?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn file_push(
    api_url: &str,
    token: Option<&str>,
    source: PathBuf,
    path: String,
    mode: String,
    clients: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    proof_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
) -> Result<()> {
    anyhow::ensure!(
        confirmed,
        "file push requires --confirmed because it writes a remote file"
    );
    validate_absolute_file_path(&path).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let mode = parse_file_mode(&mode)?;
    validate_file_mode(mode).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let metadata = fs::metadata(&source)
        .with_context(|| format!("failed to stat source file {}", source.display()))?;
    anyhow::ensure!(
        metadata.is_file(),
        "file push source must be a regular file"
    );
    anyhow::ensure!(
        metadata.len() <= MAX_CHUNKED_FILE_PUSH_BYTES as u64,
        "file push source exceeds chunked transfer limit: {} > {} bytes",
        metadata.len(),
        MAX_CHUNKED_FILE_PUSH_BYTES
    );
    let data = fs::read(&source)
        .with_context(|| format!("failed to read source file {}", source.display()))?;
    let data_hash = payload_hash(&data);
    let (command_label, operation) = if data.len() <= MAX_INLINE_FILE_PUSH_BYTES {
        (
            "file_push",
            JobCommand::FilePush {
                path,
                mode,
                size_bytes: data.len() as u64,
                sha256_hex: data_hash,
                data_base64: encode_inline_file_payload(&data)?,
                existing_policy: FileExistingPolicy::Replace,
                owner: None,
                group: None,
                uid: None,
                gid: None,
                ownership_policy: FileOwnershipPolicy::Fail,
            },
        )
    } else {
        (
            "file_push_chunked",
            JobCommand::FilePushChunked {
                path,
                mode,
                size_bytes: data.len() as u64,
                sha256_hex: data_hash,
                chunks: encode_chunked_file_payload(&data)?,
                existing_policy: FileExistingPolicy::Replace,
                owner: None,
                group: None,
                uid: None,
                gid: None,
                ownership_policy: FileOwnershipPolicy::Fail,
            },
        )
    };
    println!(
        "{}",
        submit_privileged_operation(PrivilegedOperationRequest {
            api_url,
            token,
            operation: &operation,
            command_label,
            clients: &clients,
            tags: &tags,
            password_env: &password_env,
            super_salt_hex: super_salt_hex.as_deref(),
            proof_ttl_secs,
            timeout_secs,
            confirmed,
            force_unprivileged: false,
        })?
    );
    Ok(())
}

pub(crate) fn parse_file_mode(value: &str) -> Result<u32> {
    let trimmed = value.trim();
    anyhow::ensure!(!trimmed.is_empty(), "file mode is empty");
    let digits = trimmed.strip_prefix("0o").unwrap_or(trimmed);
    anyhow::ensure!(
        !digits.is_empty()
            && digits.len() <= 4
            && digits
                .chars()
                .all(|character| matches!(character, '0'..='7')),
        "file mode must be an octal value between 0000 and 0777"
    );
    let mode = u32::from_str_radix(digits, 8).context("file mode is not a valid octal number")?;
    validate_file_mode(mode).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(mode)
}

#[cfg(test)]
mod tests {
    use super::parse_file_mode;

    #[test]
    fn parses_file_modes_as_octal_when_prefixed() {
        assert_eq!(parse_file_mode("0644").unwrap(), 0o644);
        assert_eq!(parse_file_mode("0o600").unwrap(), 0o600);
        assert_eq!(parse_file_mode("644").unwrap(), 0o644);
        assert_eq!(parse_file_mode("420").unwrap(), 0o420);
        assert!(parse_file_mode("1000").is_err());
        assert!(parse_file_mode("888").is_err());
        assert!(parse_file_mode("not-a-mode").is_err());
    }
}
