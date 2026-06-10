use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use vpsman_common::{
    payload_hash, validate_file_transfer_session, FileExistingPolicy, FilePushChunk, JobCommand,
    FILE_TRANSFER_CHUNK_BYTES, MAX_FILE_TRANSFER_RESUME_TOKEN_BYTES, MAX_RESUMABLE_FILE_PUSH_BYTES,
};

use crate::{
    commands_file_transfers::file_transfer_source_download_path,
    commands_schedules::selector_expression_from_targets,
    http::{http_get, http_get_bytes, http_post_json},
    jobs::resolve_target_ids,
    privilege::{build_privilege_for_job_command, load_super_password, load_super_salt_hex},
};

#[derive(Debug)]
pub(crate) struct FileTransferUploadPlan {
    pub(crate) source: FileTransferUploadSource,
    pub(crate) path: String,
    pub(crate) mode: u32,
    pub(crate) clients: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) privilege_ttl_secs: u64,
    pub(crate) timeout_secs: u64,
    pub(crate) confirmed: bool,
    pub(crate) session_id: Option<Uuid>,
    pub(crate) resume_token: Option<String>,
    pub(crate) chunk_size_bytes: u32,
    pub(crate) rate_limit_kbps: u32,
    pub(crate) existing_policy: FileExistingPolicy,
    pub(crate) poll_interval_ms: u64,
    pub(crate) max_polls: u32,
    pub(crate) multi_target_policy: FileTransferMultiTargetPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum FileTransferUploadSource {
    LocalFile(PathBuf),
    SourceArtifact { artifact_id: Uuid },
}

impl FileTransferUploadSource {
    fn label(&self) -> String {
        match self {
            Self::LocalFile(path) => path.display().to_string(),
            Self::SourceArtifact { artifact_id } => format!("source-artifact:{artifact_id}"),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::LocalFile(_) => "local_file",
            Self::SourceArtifact { .. } => "source_artifact",
        }
    }
}

#[derive(Debug)]
struct PreparedUploadSource {
    source: FileTransferUploadSource,
    artifact_bytes: Option<Vec<u8>>,
    size_bytes: u64,
    sha256_hex: String,
}

impl PreparedUploadSource {
    fn read_chunk(&self, offset: u64, chunk_size_bytes: u32) -> Result<Vec<u8>> {
        match &self.source {
            FileTransferUploadSource::LocalFile(path) => {
                read_transfer_chunk(path, offset, chunk_size_bytes)
            }
            FileTransferUploadSource::SourceArtifact { artifact_id } => {
                let bytes = self.artifact_bytes.as_ref().with_context(|| {
                    format!("source artifact {artifact_id} bytes are unavailable")
                })?;
                read_transfer_chunk_from_bytes(bytes, offset, chunk_size_bytes)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FileTransferMultiTargetPolicy {
    SameOffset,
    IndependentOffsets,
}

impl FileTransferMultiTargetPolicy {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "same-offset" | "same_offset" => Ok(Self::SameOffset),
            "independent-offsets" | "independent_offsets" => Ok(Self::IndependentOffsets),
            other => anyhow::bail!(
                "unknown file transfer multi-target policy {other}; expected same-offset or independent-offsets"
            ),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::SameOffset => "same-offset",
            Self::IndependentOffsets => "independent-offsets",
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateJobResponse {
    pub(crate) job_id: Uuid,
    pub(crate) target_count: usize,
}

#[derive(Debug, Deserialize)]
struct JobRecord {
    status: String,
}

#[derive(Debug, Deserialize)]
struct JobOutputRecord {
    client_id: String,
    stream: String,
    data_base64: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct TransferStatusPayload {
    #[serde(rename = "type")]
    pub(crate) status_type: String,
    pub(crate) session_id: Uuid,
    pub(crate) next_offset: u64,
    pub(crate) size_bytes: Option<u64>,
    #[serde(default)]
    pub(crate) extra: serde_json::Value,
}

#[derive(Clone, Debug)]
pub(crate) struct TransferClientStatus {
    pub(crate) client_id: String,
    pub(crate) payload: TransferStatusPayload,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn file_transfer_upload(
    api_url: &str,
    token: Option<&str>,
    source: FileTransferUploadSource,
    path: String,
    mode: u32,
    clients: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
    session_id: Option<Uuid>,
    resume_token: Option<String>,
    chunk_size_bytes: u32,
    rate_limit_kbps: u32,
    existing_policy: FileExistingPolicy,
    poll_interval_ms: u64,
    max_polls: u32,
    multi_target_policy: FileTransferMultiTargetPolicy,
) -> Result<()> {
    let password = load_super_password(&password_env)?;
    let salt_hex = load_super_salt_hex(super_salt_hex.as_deref())?;
    let plan = FileTransferUploadPlan {
        source,
        path,
        mode,
        clients,
        tags,
        privilege_ttl_secs,
        timeout_secs,
        confirmed,
        session_id,
        resume_token,
        chunk_size_bytes,
        rate_limit_kbps,
        existing_policy,
        poll_interval_ms,
        max_polls,
        multi_target_policy,
    };
    print!(
        "{}",
        execute_file_transfer_upload(api_url, token, plan, &password, &salt_hex)?
    );
    Ok(())
}

pub(crate) fn execute_file_transfer_upload(
    api_url: &str,
    token: Option<&str>,
    plan: FileTransferUploadPlan,
    password: &str,
    salt_hex: &str,
) -> Result<String> {
    anyhow::ensure!(
        plan.confirmed,
        "file-transfer-upload requires --confirmed because it writes a remote file"
    );
    let prepared_source = prepare_upload_source(api_url, token, plan.source.clone())?;
    let size_bytes = prepared_source.size_bytes;
    anyhow::ensure!(
        size_bytes <= MAX_RESUMABLE_FILE_PUSH_BYTES,
        "file-transfer-upload source exceeds resumable transfer limit: {} > {} bytes",
        size_bytes,
        MAX_RESUMABLE_FILE_PUSH_BYTES
    );
    let sha256_hex = prepared_source.sha256_hex.clone();
    let session_id = plan.session_id.unwrap_or_else(Uuid::new_v4);
    let (resume_token, generated_resume_token) = match plan.resume_token {
        Some(token) => (token, false),
        None => (generate_resume_token(), true),
    };
    anyhow::ensure!(
        !resume_token.is_empty() && resume_token.len() <= MAX_FILE_TRANSFER_RESUME_TOKEN_BYTES,
        "resume token must be 1-{} bytes",
        MAX_FILE_TRANSFER_RESUME_TOKEN_BYTES
    );
    let resume_token_hash = payload_hash(resume_token.as_bytes());
    validate_file_transfer_session(
        session_id,
        &plan.path,
        plan.mode,
        size_bytes,
        &sha256_hex,
        plan.chunk_size_bytes,
        plan.rate_limit_kbps,
        &resume_token_hash,
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;

    let target_ids = resolve_target_ids(api_url, token, &plan.clients, &plan.tags)?;
    let mut events = String::new();
    push_event(
        &mut events,
        serde_json::json!({
            "event": "file_transfer_upload_ready",
            "session_id": session_id,
            "source": prepared_source.source.label(),
            "source_kind": prepared_source.source.kind(),
            "path": &plan.path,
            "size_bytes": size_bytes,
            "sha256_hex": &sha256_hex,
            "chunk_size_bytes": plan.chunk_size_bytes,
            "rate_limit_kbps": plan.rate_limit_kbps,
            "existing_policy": file_existing_policy_label(plan.existing_policy),
            "multi_target_policy": plan.multi_target_policy.as_str(),
            "targets": &target_ids,
            "resume_token_generated": generated_resume_token,
            "resume_token": generated_resume_token.then_some(resume_token.as_str()),
        }),
    )?;

    let submit = TransferSubmitContext {
        api_url,
        token,
        target_ids: &target_ids,
        password,
        salt_hex,
        privilege_ttl_secs: plan.privilege_ttl_secs,
        timeout_secs: plan.timeout_secs,
        confirmed: plan.confirmed,
    };
    let start = submit_transfer_step(
        &submit,
        "file_transfer_start",
        &JobCommand::FileTransferStart {
            session_id,
            path: plan.path.clone(),
            mode: plan.mode,
            size_bytes,
            sha256_hex: sha256_hex.clone(),
            chunk_size_bytes: plan.chunk_size_bytes,
            rate_limit_kbps: plan.rate_limit_kbps,
            existing_policy: plan.existing_policy,
            resume_token_hash: resume_token_hash.clone(),
        },
    )?;
    let start_statuses = wait_for_transfer_status(
        api_url,
        token,
        start.job_id,
        session_id,
        "file_transfer_start",
        start.target_count,
        plan.poll_interval_ms,
        plan.max_polls,
    )?;
    let mut target_offsets = target_offsets_from_statuses(&start_statuses, size_bytes)?;
    let active_target_ids = active_transfer_targets(&start_statuses);
    let next_offset = match plan.multi_target_policy {
        FileTransferMultiTargetPolicy::SameOffset if active_target_ids.is_empty() => {
            Some(size_bytes)
        }
        FileTransferMultiTargetPolicy::SameOffset => Some(uniform_next_offset(
            &active_statuses(&start_statuses),
            size_bytes,
        )?),
        FileTransferMultiTargetPolicy::IndependentOffsets => None,
    };
    push_event(
        &mut events,
        serde_json::json!({
            "event": "file_transfer_upload_started",
            "job_id": start.job_id,
            "session_id": session_id,
            "next_offset": next_offset,
            "target_offsets": &target_offsets,
            "multi_target_policy": plan.multi_target_policy.as_str(),
            "target_count": start.target_count,
        }),
    )?;

    match plan.multi_target_policy {
        FileTransferMultiTargetPolicy::SameOffset => {
            let active_submit = TransferSubmitContext {
                api_url,
                token,
                target_ids: &active_target_ids,
                password,
                salt_hex,
                privilege_ttl_secs: plan.privilege_ttl_secs,
                timeout_secs: plan.timeout_secs,
                confirmed: plan.confirmed,
            };
            let mut offset = next_offset.expect("same-offset start offset");
            while offset < size_bytes {
                let chunk = prepared_source.read_chunk(offset, plan.chunk_size_bytes)?;
                let chunk_len = chunk.len() as u64;
                let chunk_job = submit_upload_chunk(
                    &active_submit,
                    session_id,
                    offset,
                    &chunk,
                    &resume_token_hash,
                )?;
                let chunk_statuses = wait_for_transfer_status(
                    api_url,
                    token,
                    chunk_job.job_id,
                    session_id,
                    "file_transfer_chunk_ack",
                    chunk_job.target_count,
                    plan.poll_interval_ms,
                    plan.max_polls,
                )?;
                let acknowledged_offset = uniform_next_offset(&chunk_statuses, size_bytes)?;
                anyhow::ensure!(
                    acknowledged_offset > offset,
                    "file transfer chunk acknowledged no progress at offset {offset}"
                );
                target_offsets.extend(target_offsets_from_statuses(&chunk_statuses, size_bytes)?);
                push_event(
                    &mut events,
                    serde_json::json!({
                        "event": "file_transfer_upload_chunk",
                        "job_id": chunk_job.job_id,
                        "session_id": session_id,
                        "offset": offset,
                        "chunk_size_bytes": chunk_len,
                        "next_offset": acknowledged_offset,
                        "target_offsets": &target_offsets,
                        "multi_target_policy": plan.multi_target_policy.as_str(),
                        "targets": &active_target_ids,
                        "size_bytes": size_bytes,
                    }),
                )?;
                offset = acknowledged_offset;
            }
        }
        FileTransferMultiTargetPolicy::IndependentOffsets => {
            while target_offsets.values().any(|offset| *offset < size_bytes) {
                for (offset, targets) in targets_grouped_by_offset(&target_offsets, size_bytes) {
                    let chunk = prepared_source.read_chunk(offset, plan.chunk_size_bytes)?;
                    let chunk_len = chunk.len() as u64;
                    let subset_submit = TransferSubmitContext {
                        api_url,
                        token,
                        target_ids: &targets,
                        password,
                        salt_hex,
                        privilege_ttl_secs: plan.privilege_ttl_secs,
                        timeout_secs: plan.timeout_secs,
                        confirmed: plan.confirmed,
                    };
                    let chunk_job = submit_upload_chunk(
                        &subset_submit,
                        session_id,
                        offset,
                        &chunk,
                        &resume_token_hash,
                    )?;
                    let chunk_statuses = wait_for_transfer_status(
                        api_url,
                        token,
                        chunk_job.job_id,
                        session_id,
                        "file_transfer_chunk_ack",
                        chunk_job.target_count,
                        plan.poll_interval_ms,
                        plan.max_polls,
                    )?;
                    let chunk_offsets = target_offsets_from_statuses(&chunk_statuses, size_bytes)?;
                    for (client_id, acknowledged_offset) in &chunk_offsets {
                        anyhow::ensure!(
                            *acknowledged_offset > offset,
                            "file transfer target {client_id} acknowledged no progress at offset {offset}"
                        );
                    }
                    target_offsets.extend(chunk_offsets);
                    push_event(
                        &mut events,
                        serde_json::json!({
                            "event": "file_transfer_upload_chunk",
                            "job_id": chunk_job.job_id,
                            "session_id": session_id,
                            "offset": offset,
                            "chunk_size_bytes": chunk_len,
                            "next_offset": target_offsets.values().copied().min().unwrap_or(size_bytes),
                            "target_offsets": &target_offsets,
                            "multi_target_policy": plan.multi_target_policy.as_str(),
                            "targets": targets,
                            "size_bytes": size_bytes,
                        }),
                    )?;
                }
            }
        }
    }

    let (complete_job_id, complete_target_count, committed_offsets) =
        if active_target_ids.is_empty() {
            (start.job_id, start.target_count, target_offsets.clone())
        } else {
            let commit_submit = TransferSubmitContext {
                api_url,
                token,
                target_ids: &active_target_ids,
                password,
                salt_hex,
                privilege_ttl_secs: plan.privilege_ttl_secs,
                timeout_secs: plan.timeout_secs,
                confirmed: plan.confirmed,
            };
            let commit = submit_transfer_step(
                &commit_submit,
                "file_transfer_commit",
                &JobCommand::FileTransferCommit {
                    session_id,
                    resume_token_hash,
                },
            )?;
            let commit_statuses = wait_for_transfer_status(
                api_url,
                token,
                commit.job_id,
                session_id,
                "file_transfer_commit",
                commit.target_count,
                plan.poll_interval_ms,
                plan.max_polls,
            )?;
            target_offsets.extend(target_offsets_from_statuses(&commit_statuses, size_bytes)?);
            (commit.job_id, commit.target_count, target_offsets.clone())
        };
    ensure_all_targets_at_offset(&committed_offsets, size_bytes, "file transfer commit")?;
    push_event(
        &mut events,
        serde_json::json!({
            "event": "file_transfer_upload_complete",
            "job_id": complete_job_id,
            "session_id": session_id,
            "path": &plan.path,
            "size_bytes": size_bytes,
            "sha256_hex": &sha256_hex,
            "target_offsets": &committed_offsets,
            "active_targets": &active_target_ids,
            "multi_target_policy": plan.multi_target_policy.as_str(),
            "target_count": complete_target_count,
        }),
    )?;
    Ok(events)
}

pub(crate) struct TransferSubmitContext<'a> {
    pub(crate) api_url: &'a str,
    pub(crate) token: Option<&'a str>,
    pub(crate) target_ids: &'a [String],
    pub(crate) password: &'a str,
    pub(crate) salt_hex: &'a str,
    pub(crate) privilege_ttl_secs: u64,
    pub(crate) timeout_secs: u64,
    pub(crate) confirmed: bool,
}

pub(crate) fn submit_transfer_step(
    ctx: &TransferSubmitContext<'_>,
    command_label: &str,
    operation: &JobCommand,
) -> Result<CreateJobResponse> {
    let selector_expression = selector_expression_from_targets(ctx.target_ids, &[]);
    let privilege = build_privilege_for_job_command(
        ctx.target_ids,
        operation,
        command_label,
        &selector_expression,
        ctx.password,
        ctx.salt_hex,
        ctx.privilege_ttl_secs,
        ctx.timeout_secs,
        false,
        true,
    )?;
    let response = http_post_json(
        ctx.api_url,
        "/api/v1/jobs",
        ctx.token,
        &serde_json::json!({
            "job_id": Uuid::new_v4(),
            "command": command_label,
            "argv": [],
            "operation": operation,
            "selector_expression": selector_expression,
            "target_client_ids": ctx.target_ids,
            "privileged": true,
            "destructive": false,
            "confirmed": ctx.confirmed,
            "force_unprivileged": false,
            "timeout_secs": ctx.timeout_secs,
            "privilege_assertion": privilege.privilege_assertion,
        }),
    )?;
    let response: CreateJobResponse =
        serde_json::from_str(&response).context("failed to parse file transfer job response")?;
    anyhow::ensure!(
        response.target_count == ctx.target_ids.len(),
        "{command_label} queued {} of {} targets; resumable multi-target upload requires a fixed target set",
        response.target_count,
        ctx.target_ids.len()
    );
    Ok(response)
}

fn submit_upload_chunk(
    ctx: &TransferSubmitContext<'_>,
    session_id: Uuid,
    offset: u64,
    chunk: &[u8],
    resume_token_hash: &str,
) -> Result<CreateJobResponse> {
    submit_transfer_step(
        ctx,
        "file_transfer_chunk",
        &JobCommand::FileTransferChunk {
            session_id,
            offset,
            chunk: FilePushChunk {
                offset,
                size_bytes: chunk.len() as u32,
                sha256_hex: payload_hash(chunk),
                data_base64: BASE64.encode(chunk),
            },
            resume_token_hash: resume_token_hash.to_string(),
        },
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn wait_for_transfer_status(
    api_url: &str,
    token: Option<&str>,
    job_id: Uuid,
    session_id: Uuid,
    expected_status_type: &str,
    expected_targets: usize,
    poll_interval_ms: u64,
    max_polls: u32,
) -> Result<Vec<TransferClientStatus>> {
    let interval = Duration::from_millis(poll_interval_ms.clamp(100, 10_000));
    let max_polls = max_polls.clamp(1, 100_000);
    let mut statuses = BTreeMap::new();
    for poll in 0..max_polls {
        let outputs_json = http_get(api_url, &format!("/api/v1/jobs/{job_id}/outputs"), token)?;
        let outputs = serde_json::from_str::<Vec<JobOutputRecord>>(&outputs_json)
            .context("failed to parse file transfer job outputs")?;
        for output in &outputs {
            if let Some(payload) = parse_transfer_status(output, session_id, expected_status_type)?
            {
                statuses.insert(
                    output.client_id.clone(),
                    TransferClientStatus {
                        client_id: output.client_id.clone(),
                        payload,
                    },
                );
            }
        }
        let job_json = http_get(api_url, &format!("/api/v1/jobs/{job_id}"), token)?;
        let job =
            serde_json::from_str::<JobRecord>(&job_json).context("failed to parse transfer job")?;
        if is_terminal_job_status(&job.status) {
            anyhow::ensure!(
                job.status == "completed",
                "{expected_status_type} job {job_id} ended with status {}; outputs: {}",
                job.status,
                summarize_outputs(&outputs)
            );
            anyhow::ensure!(
                statuses.len() == expected_targets,
                "{expected_status_type} job {job_id} returned {} of {} target status ACKs",
                statuses.len(),
                expected_targets
            );
            return Ok(statuses.into_values().collect());
        }
        if poll + 1 < max_polls {
            thread::sleep(interval);
        }
    }
    anyhow::bail!("{expected_status_type} job {job_id} exceeded max polls")
}

fn parse_transfer_status(
    output: &JobOutputRecord,
    session_id: Uuid,
    expected_status_type: &str,
) -> Result<Option<TransferStatusPayload>> {
    if output.stream != "status" {
        return Ok(None);
    }
    let data = BASE64
        .decode(&output.data_base64)
        .context("file transfer status output is not valid base64")?;
    let payload = serde_json::from_slice::<TransferStatusPayload>(&data)
        .context("file transfer status output is not valid JSON")?;
    if payload.session_id != session_id || payload.status_type != expected_status_type {
        return Ok(None);
    }
    Ok(Some(payload))
}

pub(crate) fn uniform_next_offset(
    statuses: &[TransferClientStatus],
    size_bytes: u64,
) -> Result<u64> {
    anyhow::ensure!(
        !statuses.is_empty(),
        "file transfer step did not return any status ACKs"
    );
    let offset = statuses[0].payload.next_offset;
    anyhow::ensure!(
        offset <= size_bytes,
        "file transfer ACK offset {offset} exceeds declared size {size_bytes}"
    );
    for status in statuses {
        anyhow::ensure!(
            status.payload.next_offset == offset,
            "file transfer targets diverged: {} acknowledged {}, expected {offset}",
            status.client_id,
            status.payload.next_offset
        );
        if let Some(declared_size) = status.payload.size_bytes {
            anyhow::ensure!(
                declared_size == size_bytes,
                "file transfer target {} reported size {}, expected {}",
                status.client_id,
                declared_size,
                size_bytes
            );
        }
    }
    Ok(offset)
}

fn target_offsets_from_statuses(
    statuses: &[TransferClientStatus],
    size_bytes: u64,
) -> Result<BTreeMap<String, u64>> {
    anyhow::ensure!(
        !statuses.is_empty(),
        "file transfer step did not return any status ACKs"
    );
    let mut offsets = BTreeMap::new();
    for status in statuses {
        let offset = status.payload.next_offset;
        anyhow::ensure!(
            offset <= size_bytes,
            "file transfer ACK offset {offset} exceeds declared size {size_bytes}"
        );
        if let Some(declared_size) = status.payload.size_bytes {
            anyhow::ensure!(
                declared_size == size_bytes,
                "file transfer target {} reported size {}, expected {}",
                status.client_id,
                declared_size,
                size_bytes
            );
        }
        offsets.insert(status.client_id.clone(), offset);
    }
    Ok(offsets)
}

fn active_transfer_targets(statuses: &[TransferClientStatus]) -> Vec<String> {
    statuses
        .iter()
        .filter(|status| !transfer_status_skipped(&status.payload))
        .map(|status| status.client_id.clone())
        .collect()
}

fn active_statuses(statuses: &[TransferClientStatus]) -> Vec<TransferClientStatus> {
    statuses
        .iter()
        .filter(|status| !transfer_status_skipped(&status.payload))
        .cloned()
        .collect()
}

fn transfer_status_skipped(status: &TransferStatusPayload) -> bool {
    status
        .extra
        .get("skipped")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn file_existing_policy_label(policy: FileExistingPolicy) -> &'static str {
    match policy {
        FileExistingPolicy::Skip => "skip",
        FileExistingPolicy::Replace => "replace",
    }
}

fn targets_grouped_by_offset(
    target_offsets: &BTreeMap<String, u64>,
    size_bytes: u64,
) -> Vec<(u64, Vec<String>)> {
    let mut grouped = BTreeMap::<u64, Vec<String>>::new();
    for (target, offset) in target_offsets {
        if *offset < size_bytes {
            grouped.entry(*offset).or_default().push(target.clone());
        }
    }
    grouped.into_iter().collect()
}

fn ensure_all_targets_at_offset(
    target_offsets: &BTreeMap<String, u64>,
    expected_offset: u64,
    label: &str,
) -> Result<()> {
    anyhow::ensure!(
        !target_offsets.is_empty(),
        "{label} returned no target ACKs"
    );
    for (client_id, offset) in target_offsets {
        anyhow::ensure!(
            *offset == expected_offset,
            "{label} for {client_id} ended at {offset}, expected {expected_offset}"
        );
    }
    Ok(())
}

fn summarize_outputs(outputs: &[JobOutputRecord]) -> String {
    outputs
        .iter()
        .filter_map(|output| {
            let bytes = BASE64.decode(&output.data_base64).ok()?;
            let text = String::from_utf8_lossy(&bytes);
            Some(format!(
                "{}:{}:{}",
                output.client_id,
                output.stream,
                text.trim().chars().take(256).collect::<String>()
            ))
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn prepare_upload_source(
    api_url: &str,
    token: Option<&str>,
    source: FileTransferUploadSource,
) -> Result<PreparedUploadSource> {
    match &source {
        FileTransferUploadSource::LocalFile(path) => {
            let source_meta = fs::metadata(path)
                .with_context(|| format!("failed to stat source file {}", path.display()))?;
            anyhow::ensure!(
                source_meta.is_file(),
                "file-transfer-upload source must be a regular file"
            );
            let size_bytes = source_meta.len();
            let sha256_hex = sha256_file(path)?;
            Ok(PreparedUploadSource {
                source,
                artifact_bytes: None,
                size_bytes,
                sha256_hex,
            })
        }
        FileTransferUploadSource::SourceArtifact { artifact_id } => {
            let path = file_transfer_source_download_path(*artifact_id);
            let bytes = http_get_bytes(api_url, &path, token).with_context(|| {
                format!("failed to download source artifact {artifact_id} before upload")
            })?;
            let size_bytes = bytes.len() as u64;
            Ok(PreparedUploadSource {
                source,
                artifact_bytes: Some(bytes.clone()),
                size_bytes,
                sha256_hex: sha256_bytes(&bytes),
            })
        }
    }
}

fn read_transfer_chunk(path: &Path, offset: u64, chunk_size_bytes: u32) -> Result<Vec<u8>> {
    let mut file = File::open(path)
        .with_context(|| format!("failed to open source file {}", path.display()))?;
    anyhow::ensure!(
        chunk_size_bytes > 0 && chunk_size_bytes as usize <= FILE_TRANSFER_CHUNK_BYTES,
        "chunk size must be between 1 and {FILE_TRANSFER_CHUNK_BYTES} bytes"
    );
    file.seek(SeekFrom::Start(offset))?;
    let mut chunk = vec![0_u8; chunk_size_bytes as usize];
    let read = file
        .read(&mut chunk)
        .context("failed to read file transfer chunk")?;
    anyhow::ensure!(read > 0, "file transfer chunk read made no progress");
    chunk.truncate(read);
    Ok(chunk)
}

fn read_transfer_chunk_from_bytes(
    bytes: &[u8],
    offset: u64,
    chunk_size_bytes: u32,
) -> Result<Vec<u8>> {
    anyhow::ensure!(
        chunk_size_bytes > 0 && chunk_size_bytes as usize <= FILE_TRANSFER_CHUNK_BYTES,
        "chunk size must be between 1 and {FILE_TRANSFER_CHUNK_BYTES} bytes"
    );
    let offset = usize::try_from(offset).context("file transfer offset exceeds platform size")?;
    anyhow::ensure!(
        offset < bytes.len(),
        "file transfer chunk read made no progress"
    );
    let end = offset
        .saturating_add(chunk_size_bytes as usize)
        .min(bytes.len());
    Ok(bytes[offset..end].to_vec())
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)
        .with_context(|| format!("failed to open source file {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; FILE_TRANSFER_CHUNK_BYTES];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("failed to read source file {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub(crate) fn generate_resume_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub(crate) fn push_event(events: &mut String, event: serde_json::Value) -> Result<()> {
    events.push_str(&serde_json::to_string(&event)?);
    events.push('\n');
    Ok(())
}

fn is_terminal_job_status(status: &str) -> bool {
    matches!(
        status,
        "completed"
            | "partially_completed"
            | "failed"
            | "timed_out"
            | "dispatch_failed"
            | "degraded_unprivileged"
            | "accepted"
            | "rejected_authorization_required"
            | "schedule_no_targets"
            | "rejected_by_agent"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_file_transfer_chunk_with_hashable_payload() {
        let path = std::env::temp_dir().join(format!("vpsman-transfer-cli-{}", Uuid::new_v4()));
        fs::write(&path, b"abcdef").unwrap();
        let chunk = read_transfer_chunk(&path, 2, 3).unwrap();
        assert_eq!(chunk, b"cde");
        assert_eq!(payload_hash(&chunk).len(), 64);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn reads_file_transfer_chunk_from_retained_bytes() {
        let chunk = read_transfer_chunk_from_bytes(b"abcdef", 1, 4).unwrap();
        assert_eq!(chunk, b"bcde");
        assert_eq!(payload_hash(&chunk).len(), 64);
    }

    #[test]
    fn detects_divergent_transfer_offsets() {
        let session_id = Uuid::new_v4();
        let statuses = vec![
            TransferClientStatus {
                client_id: "edge-a".to_string(),
                payload: TransferStatusPayload {
                    status_type: "file_transfer_chunk_ack".to_string(),
                    session_id,
                    next_offset: 64,
                    size_bytes: Some(128),
                    extra: serde_json::Value::Null,
                },
            },
            TransferClientStatus {
                client_id: "edge-b".to_string(),
                payload: TransferStatusPayload {
                    status_type: "file_transfer_chunk_ack".to_string(),
                    session_id,
                    next_offset: 32,
                    size_bytes: Some(128),
                    extra: serde_json::Value::Null,
                },
            },
        ];
        assert!(uniform_next_offset(&statuses, 128).is_err());
    }

    #[test]
    fn parses_file_transfer_multi_target_policy() {
        assert_eq!(
            FileTransferMultiTargetPolicy::parse("same-offset").unwrap(),
            FileTransferMultiTargetPolicy::SameOffset
        );
        assert_eq!(
            FileTransferMultiTargetPolicy::parse("independent_offsets").unwrap(),
            FileTransferMultiTargetPolicy::IndependentOffsets
        );
        assert!(FileTransferMultiTargetPolicy::parse("unknown").is_err());
    }

    #[test]
    fn groups_independent_targets_by_current_offset() {
        let offsets = BTreeMap::from([
            ("edge-a".to_string(), 0),
            ("edge-b".to_string(), 64),
            ("edge-c".to_string(), 64),
            ("edge-d".to_string(), 128),
        ]);

        let grouped = targets_grouped_by_offset(&offsets, 128);

        assert_eq!(
            grouped,
            vec![
                (0, vec!["edge-a".to_string()]),
                (64, vec!["edge-b".to_string(), "edge-c".to_string()])
            ]
        );
    }

    #[test]
    fn builds_target_offset_map_from_statuses() {
        let session_id = Uuid::new_v4();
        let statuses = vec![
            TransferClientStatus {
                client_id: "edge-a".to_string(),
                payload: TransferStatusPayload {
                    status_type: "file_transfer_start".to_string(),
                    session_id,
                    next_offset: 64,
                    size_bytes: Some(128),
                    extra: serde_json::Value::Null,
                },
            },
            TransferClientStatus {
                client_id: "edge-b".to_string(),
                payload: TransferStatusPayload {
                    status_type: "file_transfer_start".to_string(),
                    session_id,
                    next_offset: 32,
                    size_bytes: Some(128),
                    extra: serde_json::Value::Null,
                },
            },
        ];

        let offsets = target_offsets_from_statuses(&statuses, 128).unwrap();

        assert_eq!(offsets["edge-a"], 64);
        assert_eq!(offsets["edge-b"], 32);
        assert!(ensure_all_targets_at_offset(&offsets, 128, "test commit").is_err());
    }

    #[test]
    fn skipped_existing_upload_targets_are_not_active_commit_targets() {
        let session_id = Uuid::new_v4();
        let statuses = vec![
            TransferClientStatus {
                client_id: "edge-a".to_string(),
                payload: TransferStatusPayload {
                    status_type: "file_transfer_start".to_string(),
                    session_id,
                    next_offset: 128,
                    size_bytes: Some(128),
                    extra: serde_json::json!({
                        "skipped": true,
                        "reason": "destination_exists",
                    }),
                },
            },
            TransferClientStatus {
                client_id: "edge-b".to_string(),
                payload: TransferStatusPayload {
                    status_type: "file_transfer_start".to_string(),
                    session_id,
                    next_offset: 0,
                    size_bytes: Some(128),
                    extra: serde_json::Value::Null,
                },
            },
        ];

        assert_eq!(active_transfer_targets(&statuses), vec!["edge-b"]);
        assert_eq!(active_statuses(&statuses).len(), 1);
        let offsets = target_offsets_from_statuses(&statuses, 128).unwrap();
        assert_eq!(offsets["edge-a"], 128);
        assert_eq!(offsets["edge-b"], 0);
    }

    #[test]
    fn parses_transfer_status_from_job_output() {
        let session_id = Uuid::new_v4();
        let payload = serde_json::json!({
            "type": "file_transfer_start",
            "session_id": session_id,
            "next_offset": 0,
            "size_bytes": 7,
        });
        let output = JobOutputRecord {
            client_id: "edge-a".to_string(),
            stream: "status".to_string(),
            data_base64: BASE64.encode(serde_json::to_vec(&payload).unwrap()),
        };
        let parsed = parse_transfer_status(&output, session_id, "file_transfer_start")
            .unwrap()
            .unwrap();
        assert_eq!(parsed.next_offset, 0);
        assert_eq!(parsed.size_bytes, Some(7));
    }
}
