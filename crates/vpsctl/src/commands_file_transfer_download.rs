use std::{
    fs::{self, File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::Deserialize;
use uuid::Uuid;
use vpsman_common::{
    payload_hash, validate_file_transfer_download_session, JobCommand,
    MAX_FILE_TRANSFER_RESUME_TOKEN_BYTES, MAX_RESUMABLE_FILE_DOWNLOAD_BYTES,
};

use crate::{
    commands_file_transfer::{
        generate_resume_token, push_event, sha256_file, submit_transfer_step, uniform_next_offset,
        wait_for_transfer_status, TransferClientStatus, TransferSubmitContext,
    },
    http::{http_get, http_get_bytes},
    jobs::resolve_target_ids,
    proof::{load_super_password, load_super_salt_hex},
};

#[derive(Debug)]
pub(crate) struct FileTransferDownloadPlan {
    pub(crate) destination: PathBuf,
    pub(crate) path: String,
    pub(crate) clients: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) proof_ttl_secs: u64,
    pub(crate) timeout_secs: u64,
    pub(crate) confirmed: bool,
    pub(crate) session_id: Option<Uuid>,
    pub(crate) resume_token: Option<String>,
    pub(crate) chunk_size_bytes: u32,
    pub(crate) rate_limit_kbps: u32,
    pub(crate) poll_interval_ms: u64,
    pub(crate) max_polls: u32,
    pub(crate) multi_target_policy: FileTransferDownloadMultiTargetPolicy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FileTransferDownloadMultiTargetPolicy {
    SingleTarget,
    PerTargetFiles,
}

impl FileTransferDownloadMultiTargetPolicy {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "single-target" | "single_target" => Ok(Self::SingleTarget),
            "per-target-files" | "per_target_files" => Ok(Self::PerTargetFiles),
            other => anyhow::bail!(
                "unknown file transfer download multi-target policy {other}; expected single-target or per-target-files"
            ),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::SingleTarget => "single-target",
            Self::PerTargetFiles => "per-target-files",
        }
    }
}

#[derive(Debug, Deserialize)]
struct JobOutputRecord {
    client_id: String,
    seq: i32,
    stream: String,
    data_base64: String,
    #[serde(default)]
    storage: String,
    #[serde(default)]
    artifact_sha256_hex: Option<String>,
    #[serde(default)]
    artifact_size_bytes: Option<i64>,
}

struct DownloadChunkStep {
    offset: u64,
    chunk: Vec<u8>,
    status: TransferClientStatus,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn file_transfer_download(
    api_url: &str,
    token: Option<&str>,
    destination: PathBuf,
    path: String,
    clients: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    proof_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
    session_id: Option<Uuid>,
    resume_token: Option<String>,
    chunk_size_bytes: u32,
    rate_limit_kbps: u32,
    poll_interval_ms: u64,
    max_polls: u32,
    multi_target_policy: FileTransferDownloadMultiTargetPolicy,
) -> Result<()> {
    let password = load_super_password(&password_env)?;
    let salt_hex = load_super_salt_hex(super_salt_hex.as_deref())?;
    let plan = FileTransferDownloadPlan {
        destination,
        path,
        clients,
        tags,
        proof_ttl_secs,
        timeout_secs,
        confirmed,
        session_id,
        resume_token,
        chunk_size_bytes,
        rate_limit_kbps,
        poll_interval_ms,
        max_polls,
        multi_target_policy,
    };
    print!(
        "{}",
        execute_file_transfer_download(api_url, token, plan, &password, &salt_hex)?
    );
    Ok(())
}

pub(crate) fn execute_file_transfer_download(
    api_url: &str,
    token: Option<&str>,
    plan: FileTransferDownloadPlan,
    password: &str,
    salt_hex: &str,
) -> Result<String> {
    anyhow::ensure!(
        plan.confirmed,
        "file-transfer-download requires --confirmed because it writes a local file"
    );
    let session_id = plan.session_id.unwrap_or_else(Uuid::new_v4);
    let (resume_token, generated_resume_token) = match plan.resume_token.as_deref() {
        Some(token) => (token.to_string(), false),
        None => (generate_resume_token(), true),
    };
    anyhow::ensure!(
        !resume_token.is_empty() && resume_token.len() <= MAX_FILE_TRANSFER_RESUME_TOKEN_BYTES,
        "resume token must be 1-{} bytes",
        MAX_FILE_TRANSFER_RESUME_TOKEN_BYTES
    );
    let resume_token_hash = payload_hash(resume_token.as_bytes());
    validate_file_transfer_download_session(
        session_id,
        &plan.path,
        plan.chunk_size_bytes,
        plan.rate_limit_kbps,
        &resume_token_hash,
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let target_ids = resolve_target_ids(
        api_url,
        token,
        &plan.clients,
        &plan.tags,
        false,
        plan.confirmed,
    )?;
    let mut events = String::new();
    let destinations = download_destination_specs(
        &plan.destination,
        &plan.path,
        &target_ids,
        plan.multi_target_policy,
    )?;
    for (target_id, destination) in destinations {
        let target_ids = vec![target_id.clone()];
        let submit = TransferSubmitContext {
            api_url,
            token,
            target_ids: &target_ids,
            password,
            salt_hex,
            proof_ttl_secs: plan.proof_ttl_secs,
            timeout_secs: plan.timeout_secs,
            confirmed: plan.confirmed,
        };
        execute_file_transfer_download_for_target(DownloadTargetInput {
            api_url,
            token,
            plan: &plan,
            submit: &submit,
            session_id,
            resume_token: &resume_token,
            generated_resume_token,
            resume_token_hash: &resume_token_hash,
            target_id: &target_id,
            destination: &destination,
            events: &mut events,
        })?;
    }
    Ok(events)
}

struct DownloadTargetInput<'a> {
    api_url: &'a str,
    token: Option<&'a str>,
    plan: &'a FileTransferDownloadPlan,
    submit: &'a TransferSubmitContext<'a>,
    session_id: Uuid,
    resume_token: &'a str,
    generated_resume_token: bool,
    resume_token_hash: &'a str,
    target_id: &'a str,
    destination: &'a Path,
    events: &'a mut String,
}

fn execute_file_transfer_download_for_target(input: DownloadTargetInput<'_>) -> Result<()> {
    push_event(
        input.events,
        serde_json::json!({
            "event": "file_transfer_download_ready",
            "session_id": input.session_id,
            "path": &input.plan.path,
            "destination": input.destination.display().to_string(),
            "chunk_size_bytes": input.plan.chunk_size_bytes,
            "rate_limit_kbps": input.plan.rate_limit_kbps,
            "target": input.target_id,
            "multi_target_policy": input.plan.multi_target_policy.as_str(),
            "resume_token_generated": input.generated_resume_token,
            "resume_token": input.generated_resume_token.then_some(input.resume_token),
        }),
    )?;
    let start = submit_transfer_step(
        input.submit,
        "file_transfer_download_start",
        &JobCommand::FileTransferDownloadStart {
            session_id: input.session_id,
            path: input.plan.path.clone(),
            chunk_size_bytes: input.plan.chunk_size_bytes,
            rate_limit_kbps: input.plan.rate_limit_kbps,
            resume_token_hash: input.resume_token_hash.to_string(),
        },
    )?;
    let start_statuses = wait_for_transfer_status(
        input.api_url,
        input.token,
        start.job_id,
        input.session_id,
        "file_transfer_download_start",
        start.accepted_targets,
        input.plan.poll_interval_ms,
        input.plan.max_polls,
    )?;
    let size_bytes = start_statuses[0]
        .payload
        .size_bytes
        .context("download start did not report size_bytes")?;
    anyhow::ensure!(
        size_bytes <= MAX_RESUMABLE_FILE_DOWNLOAD_BYTES,
        "download source exceeds resumable limit: {} > {} bytes",
        size_bytes,
        MAX_RESUMABLE_FILE_DOWNLOAD_BYTES
    );
    let file_sha256_hex = start_statuses[0]
        .payload
        .extra
        .get("sha256_hex")
        .and_then(serde_json::Value::as_str)
        .context("download start did not report sha256_hex")?
        .to_string();
    let mut next_offset = uniform_next_offset(&start_statuses, size_bytes)?;
    push_event(
        input.events,
        serde_json::json!({
            "event": "file_transfer_download_started",
            "job_id": start.job_id,
            "session_id": input.session_id,
            "size_bytes": size_bytes,
            "sha256_hex": &file_sha256_hex,
            "next_offset": next_offset,
            "target": input.target_id,
            "multi_target_policy": input.plan.multi_target_policy.as_str(),
        }),
    )?;

    anyhow::ensure!(
        !input.destination.exists(),
        "download destination already exists: {}",
        input.destination.display()
    );
    let can_resume_local = input.plan.session_id.is_some() && !input.generated_resume_token;
    let (mut destination, temp_destination, local_resume_offset) = open_download_temp_file(
        input.destination,
        input.session_id,
        size_bytes,
        can_resume_local,
    )?;
    if local_resume_offset > next_offset {
        next_offset = local_resume_offset;
    }
    while next_offset < size_bytes {
        let chunk_job = submit_transfer_step(
            input.submit,
            "file_transfer_download_chunk",
            &JobCommand::FileTransferDownloadChunk {
                session_id: input.session_id,
                offset: next_offset,
                max_bytes: input.plan.chunk_size_bytes,
                resume_token_hash: input.resume_token_hash.to_string(),
            },
        )?;
        let step = wait_for_download_chunk(
            input.api_url,
            input.token,
            chunk_job.job_id,
            input.target_id,
            input.session_id,
            next_offset,
            input.plan.poll_interval_ms,
            input.plan.max_polls,
        )?;
        anyhow::ensure!(
            !step.chunk.is_empty() && step.status.payload.next_offset > next_offset,
            "download chunk made no progress at offset {next_offset}"
        );
        let expected_len = usize::try_from(step.status.payload.next_offset - next_offset)
            .context("download chunk length overflow")?;
        anyhow::ensure!(
            step.chunk.len() == expected_len,
            "download chunk length mismatch at offset {next_offset}: got {}, expected {expected_len}",
            step.chunk.len()
        );
        destination.write_all(&step.chunk)?;
        next_offset = step.status.payload.next_offset;
        push_event(
            input.events,
            serde_json::json!({
                "event": "file_transfer_download_chunk",
                "job_id": chunk_job.job_id,
                "session_id": input.session_id,
                "offset": step.offset,
                "chunk_size_bytes": step.chunk.len(),
                "next_offset": next_offset,
                "size_bytes": size_bytes,
                "target": input.target_id,
                "multi_target_policy": input.plan.multi_target_policy.as_str(),
            }),
        )?;
    }
    destination.flush()?;
    drop(destination);
    let actual_hash = sha256_file(&temp_destination)?;
    if actual_hash != file_sha256_hex {
        let _ = fs::remove_file(&temp_destination);
        anyhow::bail!("download hash mismatch: {actual_hash} != {file_sha256_hex}");
    }
    fs::rename(&temp_destination, input.destination).with_context(|| {
        format!(
            "failed to move {} into place at {}",
            temp_destination.display(),
            input.destination.display()
        )
    })?;
    push_event(
        input.events,
        serde_json::json!({
            "event": "file_transfer_download_complete",
            "session_id": input.session_id,
            "path": &input.plan.path,
            "destination": input.destination.display().to_string(),
            "size_bytes": size_bytes,
            "sha256_hex": file_sha256_hex,
            "target": input.target_id,
            "multi_target_policy": input.plan.multi_target_policy.as_str(),
        }),
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn wait_for_download_chunk(
    api_url: &str,
    token: Option<&str>,
    job_id: Uuid,
    client_id: &str,
    session_id: Uuid,
    offset: u64,
    poll_interval_ms: u64,
    max_polls: u32,
) -> Result<DownloadChunkStep> {
    let statuses = wait_for_transfer_status(
        api_url,
        token,
        job_id,
        session_id,
        "file_transfer_download_chunk",
        1,
        poll_interval_ms,
        max_polls,
    )?;
    let status = statuses
        .into_iter()
        .next()
        .context("download chunk did not return status")?;
    anyhow::ensure!(
        status.client_id == client_id,
        "download chunk returned status for {}, expected {client_id}",
        status.client_id
    );
    anyhow::ensure!(
        status.payload.next_offset > offset,
        "download chunk acknowledged no progress at offset {offset}"
    );
    let expected_len =
        usize::try_from(status.payload.next_offset - offset).context("chunk length overflow")?;
    let chunk = fetch_download_stdout_chunk(api_url, token, job_id, client_id, expected_len)?;
    if let Some(expected_hash) = status
        .payload
        .extra
        .get("chunk_sha256_hex")
        .and_then(serde_json::Value::as_str)
    {
        anyhow::ensure!(
            payload_hash(&chunk) == expected_hash,
            "download chunk hash mismatch at offset {offset}"
        );
    }
    Ok(DownloadChunkStep {
        offset,
        chunk,
        status,
    })
}

fn fetch_download_stdout_chunk(
    api_url: &str,
    token: Option<&str>,
    job_id: Uuid,
    client_id: &str,
    expected_len: usize,
) -> Result<Vec<u8>> {
    let outputs_json = http_get(api_url, &format!("/api/v1/jobs/{job_id}/outputs"), token)?;
    let mut outputs = serde_json::from_str::<Vec<JobOutputRecord>>(&outputs_json)
        .context("failed to parse download chunk outputs")?
        .into_iter()
        .filter(|output| output.client_id == client_id && output.stream == "stdout")
        .collect::<Vec<_>>();
    outputs.sort_by_key(|output| output.seq);
    anyhow::ensure!(
        !outputs.is_empty(),
        "download chunk job {job_id} did not return stdout for {client_id}"
    );
    let mut chunk = Vec::with_capacity(expected_len);
    for output in outputs {
        let bytes = if output.storage == "object_store" {
            let bytes = http_get_bytes(
                api_url,
                &format!(
                    "/api/v1/jobs/{job_id}/outputs/{}/{}/artifact",
                    percent_encode_path_segment(client_id),
                    output.seq
                ),
                token,
            )?;
            if let Some(expected_hash) = &output.artifact_sha256_hex {
                anyhow::ensure!(
                    payload_hash(&bytes) == *expected_hash,
                    "download chunk artifact hash mismatch for seq {}",
                    output.seq
                );
            }
            if let Some(expected_size) = output.artifact_size_bytes {
                anyhow::ensure!(
                    bytes.len() as i64 == expected_size,
                    "download chunk artifact size mismatch for seq {}",
                    output.seq
                );
            }
            bytes
        } else {
            BASE64
                .decode(&output.data_base64)
                .context("download chunk stdout is not valid base64")?
        };
        chunk.extend_from_slice(&bytes);
    }
    anyhow::ensure!(
        chunk.len() == expected_len,
        "download chunk stdout length mismatch: got {}, expected {expected_len}",
        chunk.len()
    );
    Ok(chunk)
}

fn open_download_temp_file(
    destination: &Path,
    session_id: Uuid,
    size_bytes: u64,
    can_resume: bool,
) -> Result<(File, PathBuf, u64)> {
    if let Some(parent) = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let temp_path = local_download_temp_path(destination, session_id)?;
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&temp_path)
        .with_context(|| format!("failed to open {}", temp_path.display()))?;
    let existing_len = file.metadata()?.len();
    let resume_offset = if can_resume && existing_len <= size_bytes {
        existing_len
    } else {
        file.set_len(0)?;
        0
    };
    file.seek(SeekFrom::Start(resume_offset))?;
    Ok((file, temp_path, resume_offset))
}

fn local_download_temp_path(destination: &Path, session_id: Uuid) -> Result<PathBuf> {
    let file_name = destination
        .file_name()
        .map(|name| name.to_string_lossy())
        .filter(|name| !name.is_empty())
        .context("download destination must include a file name")?;
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    Ok(parent.join(format!(".vpsman-download-{file_name}-{session_id}.part")))
}

fn percent_encode_path_segment(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn download_destination_specs(
    destination: &Path,
    remote_path: &str,
    target_ids: &[String],
    policy: FileTransferDownloadMultiTargetPolicy,
) -> Result<Vec<(String, PathBuf)>> {
    match policy {
        FileTransferDownloadMultiTargetPolicy::SingleTarget => {
            anyhow::ensure!(
                target_ids.len() == 1,
                "file-transfer-download with single-target policy requires exactly one resolved target; got {}",
                target_ids.len()
            );
            Ok(vec![(target_ids[0].clone(), destination.to_path_buf())])
        }
        FileTransferDownloadMultiTargetPolicy::PerTargetFiles => {
            anyhow::ensure!(
                !target_ids.is_empty(),
                "file-transfer-download resolved no targets"
            );
            if destination.exists() {
                anyhow::ensure!(
                    destination.is_dir(),
                    "per-target-files destination must be a directory: {}",
                    destination.display()
                );
            }
            let remote_name = remote_download_file_name(remote_path)?;
            Ok(target_ids
                .iter()
                .map(|target_id| {
                    (
                        target_id.clone(),
                        destination.join(format!(
                            "{}-{remote_name}",
                            sanitize_download_file_component(target_id)
                        )),
                    )
                })
                .collect())
        }
    }
}

fn remote_download_file_name(remote_path: &str) -> Result<String> {
    let name = remote_path
        .rsplit('/')
        .find(|part| !part.is_empty())
        .context("download remote path must include a file name")?;
    let sanitized = sanitize_download_file_component(name);
    anyhow::ensure!(
        !sanitized.is_empty(),
        "download remote path file name is invalid"
    );
    Ok(sanitized)
}

fn sanitize_download_file_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | '~') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let trimmed = sanitized.trim_matches('_');
    if trimmed.is_empty() {
        "unnamed".to_string()
    } else {
        trimmed.chars().take(96).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_stable_local_download_temp_path() {
        let session_id = Uuid::parse_str("2e241391-63b4-4deb-b7d2-5df42a55241a").unwrap();
        let path = local_download_temp_path(&PathBuf::from("/tmp/result.bin"), session_id).unwrap();
        assert_eq!(
            path,
            PathBuf::from(
                "/tmp/.vpsman-download-result.bin-2e241391-63b4-4deb-b7d2-5df42a55241a.part"
            )
        );
    }

    #[test]
    fn percent_encodes_client_id_segments() {
        assert_eq!(percent_encode_path_segment("edge/a b"), "edge%2Fa%20b");
    }

    #[test]
    fn builds_per_target_download_destinations() {
        let destinations = download_destination_specs(
            &PathBuf::from("/tmp/downloads"),
            "/var/log/app.log",
            &["edge/sfo 01".to_string(), "fra-02".to_string()],
            FileTransferDownloadMultiTargetPolicy::PerTargetFiles,
        )
        .unwrap();
        assert_eq!(
            destinations,
            vec![
                (
                    "edge/sfo 01".to_string(),
                    PathBuf::from("/tmp/downloads/edge_sfo_01-app.log")
                ),
                (
                    "fra-02".to_string(),
                    PathBuf::from("/tmp/downloads/fra-02-app.log")
                ),
            ]
        );
    }

    #[test]
    fn rejects_multi_target_download_without_explicit_policy() {
        assert!(download_destination_specs(
            &PathBuf::from("/tmp/result.bin"),
            "/tmp/result.bin",
            &["edge-a".to_string(), "edge-b".to_string()],
            FileTransferDownloadMultiTargetPolicy::SingleTarget,
        )
        .is_err());
    }
}
