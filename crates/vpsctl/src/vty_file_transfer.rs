use std::path::PathBuf;

use anyhow::{Context, Result};
use uuid::Uuid;
use vpsman_common::{
    validate_file_transfer_download_session, validate_file_transfer_session,
    FILE_TRANSFER_CHUNK_BYTES, MAX_FILE_TRANSFER_RATE_LIMIT_KBPS,
};

use crate::{
    commands_file_transfer::{
        execute_file_transfer_upload, generate_resume_token, FileTransferMultiTargetPolicy,
        FileTransferUploadPlan, FileTransferUploadSource,
    },
    commands_file_transfer_download::{
        execute_file_transfer_download, FileTransferDownloadMultiTargetPolicy,
        FileTransferDownloadPlan,
    },
    vty_jobs::{VtyJobSelection, VtyProofContext},
};

pub(crate) fn parse_vty_file_transfer_upload(tokens: &[&str]) -> Result<FileTransferUploadPlan> {
    let mut source = None;
    let mut source_artifact_id = None;
    let mut path = None;
    let mut mode = 0o644_u32;
    let mut timeout_secs = 60_u64;
    let mut proof_ttl_secs = 300_u64;
    let mut session_id = None;
    let mut resume_token = None;
    let mut chunk_size_bytes = FILE_TRANSFER_CHUNK_BYTES as u32;
    let mut rate_limit_kbps = 0_u32;
    let mut poll_interval_ms = 250_u64;
    let mut max_polls = 1200_u32;
    let mut multi_target_policy = FileTransferMultiTargetPolicy::SameOffset;
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
            "--source-artifact-id" => {
                source_artifact_id = Some(parse_uuid(
                    "--source-artifact-id",
                    tokens.get(index + 1).copied(),
                )?);
                index += 2;
            }
            value if value.starts_with("--source-artifact-id=") => {
                source_artifact_id = Some(parse_uuid(
                    "--source-artifact-id",
                    Some(value.trim_start_matches("--source-artifact-id=")),
                )?);
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
                timeout_secs =
                    parse_bounded_u64("--timeout", tokens.get(index + 1).copied(), 1, 3600)?;
                index += 2;
            }
            value if value.starts_with("--timeout=") => {
                timeout_secs = parse_bounded_u64(
                    "--timeout",
                    Some(value.trim_start_matches("--timeout=")),
                    1,
                    3600,
                )?;
                index += 1;
            }
            "--proof-ttl" => {
                proof_ttl_secs =
                    parse_bounded_u64("--proof-ttl", tokens.get(index + 1).copied(), 1, 3600)?;
                index += 2;
            }
            value if value.starts_with("--proof-ttl=") => {
                proof_ttl_secs = parse_bounded_u64(
                    "--proof-ttl",
                    Some(value.trim_start_matches("--proof-ttl=")),
                    1,
                    3600,
                )?;
                index += 1;
            }
            "--session-id" => {
                session_id = Some(parse_uuid("--session-id", tokens.get(index + 1).copied())?);
                index += 2;
            }
            value if value.starts_with("--session-id=") => {
                session_id = Some(parse_uuid(
                    "--session-id",
                    Some(value.trim_start_matches("--session-id=")),
                )?);
                index += 1;
            }
            "--resume-token" => {
                resume_token = Some(
                    tokens
                        .get(index + 1)
                        .context("--resume-token requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            value if value.starts_with("--resume-token=") => {
                resume_token = Some(value.trim_start_matches("--resume-token=").to_string());
                index += 1;
            }
            "--chunk-size-bytes" => {
                chunk_size_bytes = parse_bounded_u64(
                    "--chunk-size-bytes",
                    tokens.get(index + 1).copied(),
                    1,
                    FILE_TRANSFER_CHUNK_BYTES as u64,
                )? as u32;
                index += 2;
            }
            value if value.starts_with("--chunk-size-bytes=") => {
                chunk_size_bytes = parse_bounded_u64(
                    "--chunk-size-bytes",
                    Some(value.trim_start_matches("--chunk-size-bytes=")),
                    1,
                    FILE_TRANSFER_CHUNK_BYTES as u64,
                )? as u32;
                index += 1;
            }
            "--rate-limit-kbps" => {
                rate_limit_kbps = parse_bounded_u64(
                    "--rate-limit-kbps",
                    tokens.get(index + 1).copied(),
                    0,
                    MAX_FILE_TRANSFER_RATE_LIMIT_KBPS as u64,
                )? as u32;
                index += 2;
            }
            value if value.starts_with("--rate-limit-kbps=") => {
                rate_limit_kbps = parse_bounded_u64(
                    "--rate-limit-kbps",
                    Some(value.trim_start_matches("--rate-limit-kbps=")),
                    0,
                    MAX_FILE_TRANSFER_RATE_LIMIT_KBPS as u64,
                )? as u32;
                index += 1;
            }
            "--poll-interval-ms" => {
                poll_interval_ms = parse_bounded_u64(
                    "--poll-interval-ms",
                    tokens.get(index + 1).copied(),
                    100,
                    10_000,
                )?;
                index += 2;
            }
            value if value.starts_with("--poll-interval-ms=") => {
                poll_interval_ms = parse_bounded_u64(
                    "--poll-interval-ms",
                    Some(value.trim_start_matches("--poll-interval-ms=")),
                    100,
                    10_000,
                )?;
                index += 1;
            }
            "--max-polls" => {
                max_polls =
                    parse_bounded_u64("--max-polls", tokens.get(index + 1).copied(), 1, 100_000)?
                        as u32;
                index += 2;
            }
            value if value.starts_with("--max-polls=") => {
                max_polls = parse_bounded_u64(
                    "--max-polls",
                    Some(value.trim_start_matches("--max-polls=")),
                    1,
                    100_000,
                )? as u32;
                index += 1;
            }
            "--multi-target-policy" => {
                multi_target_policy = FileTransferMultiTargetPolicy::parse(
                    tokens
                        .get(index + 1)
                        .context("--multi-target-policy requires a value")?,
                )?;
                index += 2;
            }
            value if value.starts_with("--multi-target-policy=") => {
                multi_target_policy = FileTransferMultiTargetPolicy::parse(
                    value.trim_start_matches("--multi-target-policy="),
                )?;
                index += 1;
            }
            value => {
                target_tokens.push(value);
                index += 1;
            }
        }
    }
    let source = match (source, source_artifact_id) {
        (Some(path), None) => FileTransferUploadSource::LocalFile(path),
        (None, Some(artifact_id)) => FileTransferUploadSource::SourceArtifact { artifact_id },
        (None, None) => anyhow::bail!("--source or --source-artifact-id is required"),
        (Some(_), Some(_)) => anyhow::bail!("use only one of --source or --source-artifact-id"),
    };
    let path = path.context("--path is required")?;
    let selection = VtyJobSelection::parse(&target_tokens)?;
    anyhow::ensure!(
        selection.confirmed,
        "file-transfer-upload requires --confirmed because it writes a remote file"
    );
    let effective_session_id = session_id.unwrap_or_else(Uuid::new_v4);
    let resume_hash = validation_resume_token_hash(resume_token.as_deref());
    validate_file_transfer_session(
        effective_session_id,
        &path,
        mode,
        0,
        &"0".repeat(64),
        chunk_size_bytes,
        rate_limit_kbps,
        &resume_hash,
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(FileTransferUploadPlan {
        source,
        path,
        mode,
        clients: selection.clients,
        tags: selection.tags,
        proof_ttl_secs,
        timeout_secs,
        confirmed: selection.confirmed,
        session_id,
        resume_token,
        chunk_size_bytes,
        rate_limit_kbps,
        poll_interval_ms,
        max_polls,
        multi_target_policy,
    })
}

pub(crate) fn parse_vty_file_transfer_download(
    tokens: &[&str],
) -> Result<FileTransferDownloadPlan> {
    let mut destination = None;
    let mut path = None;
    let mut timeout_secs = 60_u64;
    let mut proof_ttl_secs = 300_u64;
    let mut session_id = None;
    let mut resume_token = None;
    let mut chunk_size_bytes = FILE_TRANSFER_CHUNK_BYTES as u32;
    let mut rate_limit_kbps = 0_u32;
    let mut poll_interval_ms = 250_u64;
    let mut max_polls = 1200_u32;
    let mut multi_target_policy = FileTransferDownloadMultiTargetPolicy::SingleTarget;
    let mut target_tokens = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--destination" => {
                destination = Some(PathBuf::from(
                    tokens
                        .get(index + 1)
                        .context("--destination requires a local file path")?,
                ));
                index += 2;
            }
            value if value.starts_with("--destination=") => {
                destination = Some(PathBuf::from(value.trim_start_matches("--destination=")));
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
            "--timeout" => {
                timeout_secs =
                    parse_bounded_u64("--timeout", tokens.get(index + 1).copied(), 1, 3600)?;
                index += 2;
            }
            value if value.starts_with("--timeout=") => {
                timeout_secs = parse_bounded_u64(
                    "--timeout",
                    Some(value.trim_start_matches("--timeout=")),
                    1,
                    3600,
                )?;
                index += 1;
            }
            "--proof-ttl" => {
                proof_ttl_secs =
                    parse_bounded_u64("--proof-ttl", tokens.get(index + 1).copied(), 1, 3600)?;
                index += 2;
            }
            value if value.starts_with("--proof-ttl=") => {
                proof_ttl_secs = parse_bounded_u64(
                    "--proof-ttl",
                    Some(value.trim_start_matches("--proof-ttl=")),
                    1,
                    3600,
                )?;
                index += 1;
            }
            "--session-id" => {
                session_id = Some(parse_uuid("--session-id", tokens.get(index + 1).copied())?);
                index += 2;
            }
            value if value.starts_with("--session-id=") => {
                session_id = Some(parse_uuid(
                    "--session-id",
                    Some(value.trim_start_matches("--session-id=")),
                )?);
                index += 1;
            }
            "--resume-token" => {
                resume_token = Some(
                    tokens
                        .get(index + 1)
                        .context("--resume-token requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            value if value.starts_with("--resume-token=") => {
                resume_token = Some(value.trim_start_matches("--resume-token=").to_string());
                index += 1;
            }
            "--chunk-size-bytes" => {
                chunk_size_bytes = parse_bounded_u64(
                    "--chunk-size-bytes",
                    tokens.get(index + 1).copied(),
                    1,
                    FILE_TRANSFER_CHUNK_BYTES as u64,
                )? as u32;
                index += 2;
            }
            value if value.starts_with("--chunk-size-bytes=") => {
                chunk_size_bytes = parse_bounded_u64(
                    "--chunk-size-bytes",
                    Some(value.trim_start_matches("--chunk-size-bytes=")),
                    1,
                    FILE_TRANSFER_CHUNK_BYTES as u64,
                )? as u32;
                index += 1;
            }
            "--rate-limit-kbps" => {
                rate_limit_kbps = parse_bounded_u64(
                    "--rate-limit-kbps",
                    tokens.get(index + 1).copied(),
                    0,
                    MAX_FILE_TRANSFER_RATE_LIMIT_KBPS as u64,
                )? as u32;
                index += 2;
            }
            value if value.starts_with("--rate-limit-kbps=") => {
                rate_limit_kbps = parse_bounded_u64(
                    "--rate-limit-kbps",
                    Some(value.trim_start_matches("--rate-limit-kbps=")),
                    0,
                    MAX_FILE_TRANSFER_RATE_LIMIT_KBPS as u64,
                )? as u32;
                index += 1;
            }
            "--poll-interval-ms" => {
                poll_interval_ms = parse_bounded_u64(
                    "--poll-interval-ms",
                    tokens.get(index + 1).copied(),
                    100,
                    10_000,
                )?;
                index += 2;
            }
            value if value.starts_with("--poll-interval-ms=") => {
                poll_interval_ms = parse_bounded_u64(
                    "--poll-interval-ms",
                    Some(value.trim_start_matches("--poll-interval-ms=")),
                    100,
                    10_000,
                )?;
                index += 1;
            }
            "--max-polls" => {
                max_polls =
                    parse_bounded_u64("--max-polls", tokens.get(index + 1).copied(), 1, 100_000)?
                        as u32;
                index += 2;
            }
            value if value.starts_with("--max-polls=") => {
                max_polls = parse_bounded_u64(
                    "--max-polls",
                    Some(value.trim_start_matches("--max-polls=")),
                    1,
                    100_000,
                )? as u32;
                index += 1;
            }
            "--multi-target-policy" => {
                multi_target_policy = FileTransferDownloadMultiTargetPolicy::parse(
                    tokens
                        .get(index + 1)
                        .context("--multi-target-policy requires a value")?,
                )?;
                index += 2;
            }
            value if value.starts_with("--multi-target-policy=") => {
                multi_target_policy = FileTransferDownloadMultiTargetPolicy::parse(
                    value.trim_start_matches("--multi-target-policy="),
                )?;
                index += 1;
            }
            value => {
                target_tokens.push(value);
                index += 1;
            }
        }
    }
    let destination = destination.context("--destination is required")?;
    let path = path.context("--path is required")?;
    let selection = VtyJobSelection::parse(&target_tokens)?;
    anyhow::ensure!(
        selection.confirmed,
        "file-transfer-download requires --confirmed because it writes a local file"
    );
    let effective_session_id = session_id.unwrap_or_else(Uuid::new_v4);
    let resume_hash = validation_resume_token_hash(resume_token.as_deref());
    validate_file_transfer_download_session(
        effective_session_id,
        &path,
        chunk_size_bytes,
        rate_limit_kbps,
        &resume_hash,
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(FileTransferDownloadPlan {
        destination,
        path,
        clients: selection.clients,
        tags: selection.tags,
        proof_ttl_secs,
        timeout_secs,
        confirmed: selection.confirmed,
        session_id,
        resume_token,
        chunk_size_bytes,
        rate_limit_kbps,
        poll_interval_ms,
        max_polls,
        multi_target_policy,
    })
}

pub(crate) fn submit_vty_file_transfer_upload(
    api_url: &str,
    token: Option<&str>,
    proof_context: &VtyProofContext,
    request: FileTransferUploadPlan,
) -> Result<String> {
    execute_file_transfer_upload(
        api_url,
        token,
        request,
        &proof_context.password,
        &proof_context.salt_hex,
    )
}

pub(crate) fn submit_vty_file_transfer_download(
    api_url: &str,
    token: Option<&str>,
    proof_context: &VtyProofContext,
    request: FileTransferDownloadPlan,
) -> Result<String> {
    execute_file_transfer_download(
        api_url,
        token,
        request,
        &proof_context.password,
        &proof_context.salt_hex,
    )
}

fn validation_resume_token_hash(resume_token: Option<&str>) -> String {
    let token = resume_token
        .map(str::to_owned)
        .unwrap_or_else(generate_resume_token);
    vpsman_common::payload_hash(token.as_bytes())
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
    vpsman_common::validate_file_mode(mode).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(mode)
}

fn parse_uuid(label: &str, value: Option<&str>) -> Result<Uuid> {
    Uuid::parse_str(value.context(format!("{label} requires a UUID"))?)
        .with_context(|| format!("{label} must be a UUID"))
}

fn parse_bounded_u64(label: &str, value: Option<&str>, min: u64, max: u64) -> Result<u64> {
    let parsed = value
        .context(format!("{label} requires a value"))?
        .parse::<u64>()
        .with_context(|| format!("{label} must be an integer"))?;
    anyhow::ensure!(
        (min..=max).contains(&parsed),
        "{label} must be between {min} and {max}"
    );
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vty_resumable_file_upload() {
        let request = parse_vty_file_transfer_upload(&[
            "--source",
            "/tmp/source.bin",
            "--path",
            "/tmp/remote.bin",
            "--mode",
            "0600",
            "--chunk-size-bytes",
            "4096",
            "--rate-limit-kbps",
            "1000",
            "--multi-target-policy",
            "independent-offsets",
            "--session-id",
            "2e241391-63b4-4deb-b7d2-5df42a55241a",
            "--resume-token",
            "resume-local",
            "id:edge-a",
            "--confirmed",
        ])
        .unwrap();

        assert_eq!(
            request.source,
            FileTransferUploadSource::LocalFile(PathBuf::from("/tmp/source.bin"))
        );
        assert_eq!(request.path, "/tmp/remote.bin");
        assert_eq!(request.mode, 0o600);
        assert_eq!(request.chunk_size_bytes, 4096);
        assert_eq!(request.rate_limit_kbps, 1000);
        assert_eq!(
            request.multi_target_policy,
            FileTransferMultiTargetPolicy::IndependentOffsets
        );
        assert!(request.clients.is_empty());
        assert_eq!(request.tags, vec!["id:edge-a"]);
        assert!(request.confirmed);
    }

    #[test]
    fn parses_vty_resumable_file_upload_from_source_artifact() {
        let request = parse_vty_file_transfer_upload(&[
            "--source-artifact-id",
            "11111111-2222-4333-8444-555555555555",
            "--path",
            "/tmp/remote.bin",
            "id:edge-a",
            "--confirmed",
        ])
        .unwrap();

        assert_eq!(
            request.source,
            FileTransferUploadSource::SourceArtifact {
                artifact_id: Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap(),
            }
        );
        assert_eq!(request.path, "/tmp/remote.bin");
        assert!(request.clients.is_empty());
        assert_eq!(request.tags, vec!["id:edge-a"]);
        assert!(request.confirmed);
    }

    #[test]
    fn rejects_vty_resumable_file_upload_without_confirmation() {
        assert!(parse_vty_file_transfer_upload(&[
            "--source",
            "/tmp/source.bin",
            "--path",
            "/tmp/remote.bin",
            "id:edge-a",
        ])
        .is_err());
    }

    #[test]
    fn parses_vty_resumable_file_download() {
        let request = parse_vty_file_transfer_download(&[
            "--path",
            "/tmp/remote.bin",
            "--destination",
            "/tmp/local.bin",
            "--chunk-size-bytes",
            "4096",
            "--rate-limit-kbps",
            "1000",
            "--multi-target-policy",
            "per-target-files",
            "--session-id",
            "2e241391-63b4-4deb-b7d2-5df42a55241a",
            "--resume-token",
            "resume-local",
            "id:edge-a",
            "--confirmed",
        ])
        .unwrap();

        assert_eq!(request.destination, PathBuf::from("/tmp/local.bin"));
        assert_eq!(request.path, "/tmp/remote.bin");
        assert_eq!(request.chunk_size_bytes, 4096);
        assert_eq!(request.rate_limit_kbps, 1000);
        assert_eq!(
            request.multi_target_policy,
            FileTransferDownloadMultiTargetPolicy::PerTargetFiles
        );
        assert!(request.clients.is_empty());
        assert_eq!(request.tags, vec!["id:edge-a"]);
        assert!(request.confirmed);
    }

    #[test]
    fn rejects_vty_resumable_file_download_without_confirmation() {
        assert!(parse_vty_file_transfer_download(&[
            "--path",
            "/tmp/remote.bin",
            "--destination",
            "/tmp/local.bin",
            "id:edge-a",
        ])
        .is_err());
    }
}
