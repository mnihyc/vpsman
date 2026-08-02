use std::path::PathBuf;

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use uuid::Uuid;

use crate::{commands_backups, http::http_post_json};

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct VtyBackupArtifactRecord {
    pub(crate) backup_request_id: Uuid,
    pub(crate) object_key: String,
    pub(crate) sha256_hex: String,
    pub(crate) size_bytes: i64,
    pub(crate) confirmed: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct VtyBackupArtifactUpload {
    pub(crate) backup_request_id: Uuid,
    pub(crate) object_key: String,
    pub(crate) artifact_file: PathBuf,
    pub(crate) confirmed: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct VtyBackupArtifactUploadChunked {
    pub(crate) backup_request_id: Uuid,
    pub(crate) object_key: String,
    pub(crate) artifact_file: PathBuf,
    pub(crate) chunk_size_bytes: usize,
    pub(crate) confirmed: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct VtyBackupArtifactHandoff {
    pub(crate) backup_request_id: Uuid,
    pub(crate) job_id: Option<Uuid>,
    pub(crate) confirmed: bool,
}

pub(crate) fn parse_vty_backup_artifact_record(tokens: &[&str]) -> Result<VtyBackupArtifactRecord> {
    let backup_request_id = tokens
        .first()
        .context("usage: backup-artifact-record <backup_uuid> --object-key <key> --sha256-hex <sha256> --size-bytes <n> --confirmed")?;
    let mut request = VtyBackupArtifactRecord {
        backup_request_id: Uuid::parse_str(backup_request_id)
            .context("invalid backup request UUID")?,
        object_key: String::new(),
        sha256_hex: String::new(),
        size_bytes: 0,
        confirmed: false,
    };
    let mut index = 1;
    while index < tokens.len() {
        match tokens[index] {
            "--confirmed" => {
                request.confirmed = true;
                index += 1;
            }
            "--object-key" => {
                request.object_key = tokens
                    .get(index + 1)
                    .context("--object-key requires a value")?
                    .to_string();
                index += 2;
            }
            value if value.starts_with("--object-key=") => {
                request.object_key = value.trim_start_matches("--object-key=").to_string();
                index += 1;
            }
            "--sha256-hex" => {
                request.sha256_hex = tokens
                    .get(index + 1)
                    .context("--sha256-hex requires a value")?
                    .to_string();
                index += 2;
            }
            value if value.starts_with("--sha256-hex=") => {
                request.sha256_hex = value.trim_start_matches("--sha256-hex=").to_string();
                index += 1;
            }
            "--size-bytes" => {
                request.size_bytes = tokens
                    .get(index + 1)
                    .context("--size-bytes requires a value")?
                    .parse()
                    .context("invalid --size-bytes")?;
                index += 2;
            }
            value if value.starts_with("--size-bytes=") => {
                request.size_bytes = value
                    .trim_start_matches("--size-bytes=")
                    .parse()
                    .context("invalid --size-bytes")?;
                index += 1;
            }
            other => anyhow::bail!("unknown backup-artifact-record flag {other}"),
        }
    }
    validate_artifact_record(&request)?;
    Ok(request)
}

pub(crate) fn parse_vty_backup_artifact_upload(tokens: &[&str]) -> Result<VtyBackupArtifactUpload> {
    let backup_request_id = tokens
        .first()
        .context("usage: backup-artifact-upload <backup_uuid> --object-key <key> --artifact-file <path> --confirmed")?;
    let mut request = VtyBackupArtifactUpload {
        backup_request_id: Uuid::parse_str(backup_request_id)
            .context("invalid backup request UUID")?,
        object_key: String::new(),
        artifact_file: PathBuf::new(),
        confirmed: false,
    };
    let mut index = 1;
    while index < tokens.len() {
        match tokens[index] {
            "--confirmed" => {
                request.confirmed = true;
                index += 1;
            }
            "--object-key" => {
                request.object_key = tokens
                    .get(index + 1)
                    .context("--object-key requires a value")?
                    .to_string();
                index += 2;
            }
            value if value.starts_with("--object-key=") => {
                request.object_key = value.trim_start_matches("--object-key=").to_string();
                index += 1;
            }
            "--artifact-file" => {
                request.artifact_file = tokens
                    .get(index + 1)
                    .context("--artifact-file requires a value")?
                    .into();
                index += 2;
            }
            value if value.starts_with("--artifact-file=") => {
                request.artifact_file = value.trim_start_matches("--artifact-file=").into();
                index += 1;
            }
            other => anyhow::bail!("unknown backup-artifact-upload flag {other}"),
        }
    }
    validate_artifact_upload(&request)?;
    Ok(request)
}

pub(crate) fn parse_vty_backup_artifact_upload_chunked(
    tokens: &[&str],
) -> Result<VtyBackupArtifactUploadChunked> {
    let backup_request_id = tokens
        .first()
        .context("usage: backup-artifact-upload-chunked <backup_uuid> --object-key <key> --artifact-file <path> [--chunk-size-bytes <n>] --confirmed")?;
    let mut request = VtyBackupArtifactUploadChunked {
        backup_request_id: Uuid::parse_str(backup_request_id)
            .context("invalid backup request UUID")?,
        object_key: String::new(),
        artifact_file: PathBuf::new(),
        chunk_size_bytes: 4 * 1024 * 1024,
        confirmed: false,
    };
    let mut index = 1;
    while index < tokens.len() {
        match tokens[index] {
            "--confirmed" => {
                request.confirmed = true;
                index += 1;
            }
            "--object-key" => {
                request.object_key = tokens
                    .get(index + 1)
                    .context("--object-key requires a value")?
                    .to_string();
                index += 2;
            }
            value if value.starts_with("--object-key=") => {
                request.object_key = value.trim_start_matches("--object-key=").to_string();
                index += 1;
            }
            "--artifact-file" => {
                request.artifact_file = tokens
                    .get(index + 1)
                    .context("--artifact-file requires a value")?
                    .into();
                index += 2;
            }
            value if value.starts_with("--artifact-file=") => {
                request.artifact_file = value.trim_start_matches("--artifact-file=").into();
                index += 1;
            }
            "--chunk-size-bytes" => {
                request.chunk_size_bytes = tokens
                    .get(index + 1)
                    .context("--chunk-size-bytes requires a value")?
                    .parse()
                    .context("invalid --chunk-size-bytes")?;
                index += 2;
            }
            value if value.starts_with("--chunk-size-bytes=") => {
                request.chunk_size_bytes = value
                    .trim_start_matches("--chunk-size-bytes=")
                    .parse()
                    .context("invalid --chunk-size-bytes")?;
                index += 1;
            }
            other => anyhow::bail!("unknown backup-artifact-upload-chunked flag {other}"),
        }
    }
    validate_artifact_upload_chunked(&request)?;
    Ok(request)
}

pub(crate) fn parse_vty_backup_artifact_handoff(
    tokens: &[&str],
) -> Result<VtyBackupArtifactHandoff> {
    let backup_request_id = tokens.first().context(
        "usage: backup-artifact-handoff <backup_uuid> [--job-id <job_uuid>] --confirmed",
    )?;
    let mut request = VtyBackupArtifactHandoff {
        backup_request_id: Uuid::parse_str(backup_request_id)
            .context("invalid backup request UUID")?,
        job_id: None,
        confirmed: false,
    };
    let mut index = 1;
    while index < tokens.len() {
        match tokens[index] {
            "--confirmed" => {
                request.confirmed = true;
                index += 1;
            }
            "--job-id" => {
                request.job_id = Some(
                    Uuid::parse_str(tokens.get(index + 1).context("--job-id requires a value")?)
                        .context("invalid --job-id")?,
                );
                index += 2;
            }
            value if value.starts_with("--job-id=") => {
                request.job_id = Some(
                    Uuid::parse_str(value.trim_start_matches("--job-id="))
                        .context("invalid --job-id")?,
                );
                index += 1;
            }
            other => anyhow::bail!("unknown backup-artifact-handoff flag {other}"),
        }
    }
    validate_artifact_handoff(&request)?;
    Ok(request)
}

pub(crate) fn submit_vty_backup_artifact_record(
    api_url: &str,
    token: Option<&str>,
    request: VtyBackupArtifactRecord,
) -> Result<String> {
    http_post_json(
        api_url,
        &format!(
            "/api/v1/backups/{}/artifact-metadata",
            request.backup_request_id
        ),
        token,
        &serde_json::json!({
            "object_key": request.object_key,
            "sha256_hex": request.sha256_hex,
            "size_bytes": request.size_bytes,
            "confirmed": request.confirmed,
        }),
    )
}

pub(crate) fn submit_vty_backup_artifact_upload(
    api_url: &str,
    token: Option<&str>,
    request: VtyBackupArtifactUpload,
) -> Result<String> {
    let metadata = std::fs::metadata(&request.artifact_file).with_context(|| {
        format!(
            "failed to stat artifact file {}",
            request.artifact_file.display()
        )
    })?;
    anyhow::ensure!(metadata.is_file(), "artifact file must be a regular file");
    anyhow::ensure!(
        (1..=16 * 1024 * 1024).contains(&metadata.len()),
        "artifact file size must be between 1 and 16777216 bytes"
    );
    let artifact = std::fs::read(&request.artifact_file).with_context(|| {
        format!(
            "failed to read artifact file {}",
            request.artifact_file.display()
        )
    })?;
    http_post_json(
        api_url,
        &format!("/api/v1/backups/{}/artifact", request.backup_request_id),
        token,
        &serde_json::json!({
            "object_key": request.object_key,
            "artifact_base64": BASE64.encode(artifact),
            "confirmed": request.confirmed,
        }),
    )
}

pub(crate) fn submit_vty_backup_artifact_upload_chunked(
    api_url: &str,
    token: Option<&str>,
    request: VtyBackupArtifactUploadChunked,
) -> Result<String> {
    commands_backups::backup_artifact_upload_chunked_response(
        api_url,
        token,
        request.backup_request_id.to_string(),
        request.object_key,
        request.artifact_file,
        request.chunk_size_bytes,
        request.confirmed,
    )
}

pub(crate) fn submit_vty_backup_artifact_handoff(
    api_url: &str,
    token: Option<&str>,
    request: VtyBackupArtifactHandoff,
) -> Result<String> {
    http_post_json(
        api_url,
        &format!(
            "/api/v1/backups/{}/artifact-handoff",
            request.backup_request_id
        ),
        token,
        &serde_json::json!({
            "confirmed": request.confirmed,
            "job_id": request.job_id,
        }),
    )
}

fn validate_artifact_record(request: &VtyBackupArtifactRecord) -> Result<()> {
    validate_artifact_object_key(&request.object_key)?;
    anyhow::ensure!(
        request.sha256_hex.len() == 64
            && request
                .sha256_hex
                .as_bytes()
                .iter()
                .all(u8::is_ascii_hexdigit),
        "artifact sha256 must be 64 hex characters"
    );
    anyhow::ensure!(request.size_bytes > 0, "artifact size must be positive");
    anyhow::ensure!(
        request.confirmed,
        "backup-artifact-record requires --confirmed"
    );
    Ok(())
}

fn validate_artifact_handoff(request: &VtyBackupArtifactHandoff) -> Result<()> {
    anyhow::ensure!(
        request.confirmed,
        "backup-artifact-handoff requires --confirmed"
    );
    Ok(())
}

fn validate_artifact_upload(request: &VtyBackupArtifactUpload) -> Result<()> {
    validate_artifact_object_key(&request.object_key)?;
    anyhow::ensure!(
        !request.artifact_file.as_os_str().is_empty(),
        "artifact file is required"
    );
    anyhow::ensure!(
        request.confirmed,
        "backup-artifact-upload requires --confirmed"
    );
    Ok(())
}

fn validate_artifact_upload_chunked(request: &VtyBackupArtifactUploadChunked) -> Result<()> {
    validate_artifact_object_key(&request.object_key)?;
    anyhow::ensure!(
        !request.artifact_file.as_os_str().is_empty(),
        "artifact file is required"
    );
    anyhow::ensure!(
        (1..=4 * 1024 * 1024).contains(&request.chunk_size_bytes),
        "chunk size must be between 1 and 4194304 bytes"
    );
    anyhow::ensure!(
        request.confirmed,
        "backup-artifact-upload-chunked requires --confirmed"
    );
    Ok(())
}

fn validate_artifact_object_key(object_key: &str) -> Result<()> {
    anyhow::ensure!(
        !object_key.trim().is_empty(),
        "artifact object key is required"
    );
    anyhow::ensure!(
        object_key.len() <= 1024
            && !object_key.as_bytes().contains(&0)
            && !object_key.starts_with('/')
            && !object_key.contains('\\')
            && !object_key
                .split('/')
                .any(|segment| segment.is_empty() || segment == "." || segment == ".."),
        "artifact object key must be a relative object key without . or .. segments"
    );
    Ok(())
}

#[cfg(test)]
#[path = "tests_vty_backup_artifacts.rs"]
mod tests;
