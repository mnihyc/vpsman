use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path as FsPath, PathBuf},
};

use axum::{
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, Response},
    Json,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use futures_util::stream;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

use crate::{
    error::ApiError,
    model::{
        AuditLogView, HistoryQuery, JobHistoryView, JobOutputView, JobTargetView, ListQuery,
        NetworkObservationTrendView, NetworkObservationView, ProcessSupervisorInventoryView,
    },
    model_command_templates::{JobOutputComparisonQuery, JobOutputComparisonView},
    routes_file_transfers::{map_verified_object_error, streaming_artifact_file_body},
    state::AppState,
    util::limit_or_default,
};

const FILE_DOWNLOAD_BUNDLE_STREAM_CHUNK_BYTES: usize = 64 * 1024;
const MAX_FILE_DOWNLOAD_BUNDLE_ENTRY_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_JOB_OUTPUT_ARCHIVE_STREAM_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub(crate) struct FileDownloadBundleQuery {
    pub(crate) clients: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct JobOutputDownloadQuery {
    pub(crate) stream: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct FileDownloadStatus {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    size_bytes: Option<u64>,
    #[serde(default)]
    sha256_hex: Option<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

pub(crate) async fn list_jobs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<JobHistoryView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(state.repo.query_jobs(&query).await?))
}

pub(crate) async fn get_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
) -> Result<Json<JobHistoryView>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    let job = state
        .repo
        .get_job(job_id)
        .await?
        .ok_or_else(|| ApiError::not_found("job_not_found"))?;
    Ok(Json(job))
}

pub(crate) async fn list_job_targets(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
) -> Result<Json<Vec<JobTargetView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(state.repo.list_job_targets(job_id).await?))
}

pub(crate) async fn download_job_target_statuses(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
) -> Result<Response<Body>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    let mut targets = state.repo.list_job_targets(job_id).await?;
    targets.sort_by(|left, right| left.client_id.cmp(&right.client_id));

    let archive_temp = TempDownloadFile::new("vpsman-job-target-status", "tar");
    let archive_path = archive_temp.path().to_path_buf();
    let archive_size = tokio::task::spawn_blocking(move || {
        write_job_target_status_archive(&archive_path, targets)
    })
    .await
    .map_err(|error| ApiError::from(anyhow::anyhow!(error)))?
    .map_err(ApiError::from)?;

    let body = streaming_temp_file_body(archive_temp).await?;
    let mut response = Response::new(body);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-tar"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"job-target-status-{job_id}.tar\""
        ))
        .map_err(|_| ApiError::conflict("job_target_status_filename_invalid"))?,
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&archive_size.to_string())
            .map_err(|_| ApiError::conflict("job_target_status_size_invalid"))?,
    );
    response.headers_mut().insert(
        "x-vpsman-artifact-delivery",
        HeaderValue::from_static("spooled-filesystem"),
    );
    Ok(response)
}

pub(crate) async fn list_job_outputs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
) -> Result<Json<Vec<JobOutputView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(state.repo.list_job_outputs(job_id).await?))
}

pub(crate) async fn download_file_download_bundle(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
    Query(query): Query<FileDownloadBundleQuery>,
) -> Result<Response<Body>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    let requested_clients = parse_client_filter(query.clients.as_deref());
    let outputs = state.repo.list_job_outputs(job_id).await?;
    let mut by_client: BTreeMap<String, Vec<JobOutputView>> = BTreeMap::new();
    for output in outputs {
        if requested_clients
            .as_ref()
            .is_some_and(|clients| !clients.contains(&output.client_id))
        {
            continue;
        }
        by_client
            .entry(output.client_id.clone())
            .or_default()
            .push(output);
    }

    let mut entries = Vec::new();
    for (client_id, mut outputs) in by_client {
        outputs.sort_by_key(|output| output.seq);
        let Some(status) = latest_file_download_status(&outputs) else {
            continue;
        };
        let payload = spool_file_download_payload(&state, &outputs).await?;
        validate_spooled_file_download_payload(&status, &payload)?;
        entries.push(SpooledFileDownloadBundleEntry {
            client_id,
            status,
            payload,
        });
    }
    if entries.is_empty() {
        return Err(ApiError::not_found("file_download_outputs_not_found"));
    }

    let archive_temp = TempDownloadFile::new("vpsman-file-download-bundle", "tar");
    let archive_path = archive_temp.path().to_path_buf();
    let archive_size = tokio::task::spawn_blocking(move || {
        write_file_download_bundle_archive(&archive_path, entries)
    })
    .await
    .map_err(|error| ApiError::from(anyhow::anyhow!(error)))?
    .map_err(ApiError::from)?;

    let body = streaming_temp_file_body(archive_temp).await?;
    let mut response = Response::new(body);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-tar"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"file-download-bundle-{job_id}.tar\""
        ))
        .map_err(|_| ApiError::conflict("file_download_bundle_filename_invalid"))?,
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&archive_size.to_string())
            .map_err(|_| ApiError::conflict("file_download_bundle_integrity_mismatch"))?,
    );
    response.headers_mut().insert(
        "x-vpsman-artifact-delivery",
        HeaderValue::from_static("spooled-filesystem"),
    );
    Ok(response)
}

pub(crate) async fn download_job_output_archive(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
    Query(query): Query<FileDownloadBundleQuery>,
) -> Result<Response<Body>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    let requested_clients = parse_client_filter(query.clients.as_deref());
    let outputs = state.repo.list_job_outputs(job_id).await?;
    let mut by_client: BTreeMap<String, Vec<JobOutputView>> = BTreeMap::new();
    for output in outputs {
        if requested_clients
            .as_ref()
            .is_some_and(|clients| !clients.contains(&output.client_id))
        {
            continue;
        }
        by_client
            .entry(output.client_id.clone())
            .or_default()
            .push(output);
    }

    let mut entries = Vec::new();
    for (client_id, mut outputs) in by_client {
        outputs.sort_by_key(|output| output.seq);
        entries.extend(spool_job_output_archive_entries(&state, &client_id, &outputs).await?);
    }
    if entries.is_empty() {
        return Err(ApiError::not_found("job_output_archive_not_found"));
    }

    let archive_temp = TempDownloadFile::new("vpsman-job-output-archive", "tar");
    let archive_path = archive_temp.path().to_path_buf();
    let archive_size =
        tokio::task::spawn_blocking(move || write_job_output_archive(&archive_path, entries))
            .await
            .map_err(|error| ApiError::from(anyhow::anyhow!(error)))?
            .map_err(ApiError::from)?;

    let body = streaming_temp_file_body(archive_temp).await?;
    let mut response = Response::new(body);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-tar"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"job-output-archive-{job_id}.tar\""
        ))
        .map_err(|_| ApiError::conflict("job_output_archive_filename_invalid"))?,
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&archive_size.to_string())
            .map_err(|_| ApiError::conflict("job_output_archive_integrity_mismatch"))?,
    );
    response.headers_mut().insert(
        "x-vpsman-artifact-delivery",
        HeaderValue::from_static("spooled-filesystem"),
    );
    Ok(response)
}

pub(crate) async fn download_file_download_for_client(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((job_id, client_id)): Path<(Uuid, String)>,
) -> Result<Response<Body>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    let mut outputs = state
        .repo
        .list_job_outputs(job_id)
        .await?
        .into_iter()
        .filter(|output| output.client_id == client_id)
        .collect::<Vec<_>>();
    outputs.sort_by_key(|output| output.seq);
    let status = latest_file_download_status(&outputs)
        .ok_or_else(|| ApiError::not_found("file_download_output_not_found"))?;
    let payload = spool_file_download_payload(&state, &outputs).await?;
    validate_spooled_file_download_payload(&status, &payload)?;
    let filename = safe_tar_component(status.filename.as_deref().unwrap_or("download.bin"));
    let content_type = status
        .content_type
        .as_deref()
        .and_then(|value| HeaderValue::from_str(value).ok())
        .unwrap_or_else(|| HeaderValue::from_static("application/octet-stream"));
    streaming_payload_response(
        payload,
        content_type,
        &filename,
        "file_download_output_filename_invalid",
        "file_download_output_hash_invalid",
        "file_download_output_size_invalid",
    )
    .await
}

pub(crate) async fn download_job_output_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((job_id, client_id)): Path<(Uuid, String)>,
    Query(query): Query<JobOutputDownloadQuery>,
) -> Result<Response<Body>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    let stream = match query.stream.as_str() {
        "stdout" => JobOutputStreamSelection::Single("stdout"),
        "stderr" => JobOutputStreamSelection::Single("stderr"),
        "combined" => JobOutputStreamSelection::Combined,
        _ => return Err(ApiError::bad_request("job_output_download_stream_invalid")),
    };
    let mut outputs = state
        .repo
        .list_job_outputs(job_id)
        .await?
        .into_iter()
        .filter(|output| output.client_id == client_id)
        .filter(|output| stream.includes(&output.stream))
        .collect::<Vec<_>>();
    outputs.sort_by_key(|output| output.seq);
    if outputs.is_empty() {
        return Err(ApiError::not_found("job_output_download_not_found"));
    }
    let output_refs = outputs.iter().collect::<Vec<_>>();
    let payload =
        spool_selected_job_outputs(&state, &output_refs, "job_output_download_too_large").await?;
    let filename = safe_tar_component(&format!(
        "job-output-{job_id}-{}-{}.bin",
        client_id,
        stream.filename_label()
    ));
    streaming_payload_response(
        payload,
        HeaderValue::from_static("application/octet-stream"),
        &filename,
        "job_output_download_filename_invalid",
        "job_output_download_hash_invalid",
        "job_output_download_size_invalid",
    )
    .await
}

struct TempDownloadFile {
    path: PathBuf,
}

impl TempDownloadFile {
    fn new(prefix: &str, extension: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!("{prefix}-{}.{}", Uuid::new_v4(), extension)),
        }
    }

    fn path(&self) -> &FsPath {
        &self.path
    }
}

impl Drop for TempDownloadFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

struct SpooledFileDownloadPayload {
    temp: TempDownloadFile,
    size_bytes: u64,
    sha256_hex: String,
}

struct SpooledFileDownloadBundleEntry {
    client_id: String,
    status: FileDownloadStatus,
    payload: SpooledFileDownloadPayload,
}

struct SpooledJobOutputArchiveEntry {
    client_id: String,
    stream: String,
    payload: SpooledFileDownloadPayload,
}

enum JobOutputStreamSelection {
    Single(&'static str),
    Combined,
}

impl JobOutputStreamSelection {
    fn includes(&self, stream: &str) -> bool {
        match self {
            Self::Single(expected) => stream == *expected,
            Self::Combined => matches!(stream, "stdout" | "stderr"),
        }
    }

    fn filename_label(&self) -> &'static str {
        match self {
            Self::Single(stream) => stream,
            Self::Combined => "combined",
        }
    }
}

async fn spool_job_output_archive_entries(
    state: &AppState,
    client_id: &str,
    outputs: &[JobOutputView],
) -> Result<Vec<SpooledJobOutputArchiveEntry>, ApiError> {
    let mut by_stream: BTreeMap<String, Vec<&JobOutputView>> = BTreeMap::new();
    for output in outputs {
        if !matches!(output.stream.as_str(), "stdout" | "stderr") {
            continue;
        }
        by_stream
            .entry(output.stream.clone())
            .or_default()
            .push(output);
    }

    let mut entries = Vec::new();
    for (stream, stream_outputs) in by_stream {
        let payload = spool_selected_job_outputs(
            state,
            &stream_outputs,
            "job_output_archive_stream_too_large",
        )
        .await?;
        entries.push(SpooledJobOutputArchiveEntry {
            client_id: client_id.to_string(),
            stream,
            payload,
        });
    }
    Ok(entries)
}

async fn spool_selected_job_outputs(
    state: &AppState,
    outputs: &[&JobOutputView],
    too_large_code: &'static str,
) -> Result<SpooledFileDownloadPayload, ApiError> {
    let temp = TempDownloadFile::new("vpsman-job-output-stream", "bin");
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp.path())
        .await
        .map_err(|error| ApiError::from(anyhow::anyhow!(error)))?;
    let mut hasher = Sha256::new();
    let mut size_bytes = 0_u64;
    for output in outputs {
        let bytes = materialize_output_bytes(state, output).await?;
        size_bytes = size_bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| ApiError::bad_request(too_large_code))?;
        if size_bytes > MAX_JOB_OUTPUT_ARCHIVE_STREAM_BYTES {
            return Err(ApiError::bad_request(too_large_code));
        }
        hasher.update(&bytes);
        file.write_all(&bytes)
            .await
            .map_err(|error| ApiError::from(anyhow::anyhow!(error)))?;
    }
    file.flush()
        .await
        .map_err(|error| ApiError::from(anyhow::anyhow!(error)))?;
    drop(file);
    Ok(SpooledFileDownloadPayload {
        temp,
        size_bytes,
        sha256_hex: hex::encode(hasher.finalize()),
    })
}

async fn spool_file_download_payload(
    state: &AppState,
    outputs: &[JobOutputView],
) -> Result<SpooledFileDownloadPayload, ApiError> {
    let temp = TempDownloadFile::new("vpsman-file-download-entry", "bin");
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp.path())
        .await
        .map_err(|error| ApiError::from(anyhow::anyhow!(error)))?;
    let mut hasher = Sha256::new();
    let mut size_bytes = 0_u64;
    for output in outputs.iter().filter(|output| output.stream == "stdout") {
        let bytes = materialize_output_bytes(state, output).await?;
        size_bytes = size_bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| ApiError::bad_request("file_download_bundle_entry_too_large"))?;
        if size_bytes > MAX_FILE_DOWNLOAD_BUNDLE_ENTRY_BYTES {
            return Err(ApiError::bad_request(
                "file_download_bundle_entry_too_large",
            ));
        }
        hasher.update(&bytes);
        file.write_all(&bytes)
            .await
            .map_err(|error| ApiError::from(anyhow::anyhow!(error)))?;
    }
    file.flush()
        .await
        .map_err(|error| ApiError::from(anyhow::anyhow!(error)))?;
    drop(file);
    Ok(SpooledFileDownloadPayload {
        temp,
        size_bytes,
        sha256_hex: hex::encode(hasher.finalize()),
    })
}

fn validate_spooled_file_download_payload(
    status: &FileDownloadStatus,
    payload: &SpooledFileDownloadPayload,
) -> Result<(), ApiError> {
    let expected_size = status
        .size_bytes
        .ok_or_else(|| ApiError::conflict("file_download_status_incomplete"))?;
    if expected_size != payload.size_bytes {
        return Err(ApiError::conflict(
            "file_download_output_integrity_mismatch",
        ));
    }
    let expected_sha256 = status
        .sha256_hex
        .as_deref()
        .ok_or_else(|| ApiError::conflict("file_download_status_incomplete"))?
        .to_ascii_lowercase();
    if expected_sha256.len() != 64
        || !expected_sha256
            .chars()
            .all(|value| value.is_ascii_hexdigit())
    {
        return Err(ApiError::conflict("file_download_status_invalid"));
    }
    if expected_sha256 != payload.sha256_hex {
        return Err(ApiError::conflict(
            "file_download_output_integrity_mismatch",
        ));
    }
    Ok(())
}

fn write_file_download_bundle_archive(
    archive_path: &FsPath,
    entries: Vec<SpooledFileDownloadBundleEntry>,
) -> anyhow::Result<u64> {
    let archive_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(archive_path)?;
    let mut builder = tar::Builder::new(archive_file);
    for entry in entries {
        append_file_download_bundle_entry(&mut builder, &entry)?;
    }
    builder.finish()?;
    let archive_file = builder.into_inner()?;
    archive_file.sync_data()?;
    Ok(archive_file.metadata()?.len())
}

fn append_file_download_bundle_entry(
    builder: &mut tar::Builder<std::fs::File>,
    entry: &SpooledFileDownloadBundleEntry,
) -> anyhow::Result<()> {
    let filename = entry.status.filename.as_deref().unwrap_or("download.bin");
    let entry_name = format!(
        "{}/{}",
        safe_tar_component(&entry.client_id),
        safe_tar_component(filename)
    );
    let mut payload_file = std::fs::File::open(entry.payload.temp.path())?;
    let mut header = tar::Header::new_gnu();
    header.set_size(entry.payload.size_bytes);
    header.set_mode(0o644);
    header.set_cksum();
    builder.append_data(&mut header, entry_name, &mut payload_file)?;
    append_json_archive_entry(
        builder,
        &target_status_entry_name(&entry.client_id),
        &entry.status,
    )?;
    Ok(())
}

fn write_job_target_status_archive(
    archive_path: &FsPath,
    targets: Vec<JobTargetView>,
) -> anyhow::Result<u64> {
    let archive_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(archive_path)?;
    let mut builder = tar::Builder::new(archive_file);
    append_json_archive_entry(&mut builder, "targets.json", &targets)?;
    for target in &targets {
        append_json_archive_entry(
            &mut builder,
            &target_status_entry_name(&target.client_id),
            target,
        )?;
    }
    builder.finish()?;
    let archive_file = builder.into_inner()?;
    archive_file.sync_data()?;
    Ok(archive_file.metadata()?.len())
}

fn append_json_archive_entry<T: Serialize>(
    builder: &mut tar::Builder<std::fs::File>,
    entry_name: &str,
    value: &T,
) -> anyhow::Result<()> {
    let payload = serde_json::to_vec_pretty(value)?;
    let mut header = tar::Header::new_gnu();
    header.set_size(payload.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder.append_data(&mut header, entry_name, &mut std::io::Cursor::new(payload))?;
    Ok(())
}

fn write_job_output_archive(
    archive_path: &FsPath,
    entries: Vec<SpooledJobOutputArchiveEntry>,
) -> anyhow::Result<u64> {
    let archive_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(archive_path)?;
    let mut builder = tar::Builder::new(archive_file);
    for entry in entries {
        append_job_output_archive_entry(&mut builder, &entry)?;
    }
    builder.finish()?;
    let archive_file = builder.into_inner()?;
    archive_file.sync_data()?;
    Ok(archive_file.metadata()?.len())
}

fn append_job_output_archive_entry(
    builder: &mut tar::Builder<std::fs::File>,
    entry: &SpooledJobOutputArchiveEntry,
) -> anyhow::Result<()> {
    let entry_name = format!(
        "{}/{}",
        safe_tar_component(&entry.client_id),
        safe_output_stream_filename(&entry.stream)
    );
    let mut payload_file = std::fs::File::open(entry.payload.temp.path())?;
    let mut header = tar::Header::new_gnu();
    header.set_size(entry.payload.size_bytes);
    header.set_mode(0o644);
    header.set_cksum();
    builder.append_data(&mut header, entry_name, &mut payload_file)?;
    Ok(())
}

fn safe_output_stream_filename(stream: &str) -> String {
    format!("{}.bin", safe_tar_component(stream))
}

fn target_status_entry_name(client_id: &str) -> String {
    format!("{}_status.json", safe_tar_component(client_id))
}

async fn streaming_temp_file_body(temp: TempDownloadFile) -> Result<Body, ApiError> {
    let file = tokio::fs::File::open(temp.path())
        .await
        .map_err(|error| ApiError::from(anyhow::anyhow!(error)))?;
    let stream = stream::try_unfold(
        (
            file,
            vec![0_u8; FILE_DOWNLOAD_BUNDLE_STREAM_CHUNK_BYTES],
            temp,
        ),
        |(mut file, mut buffer, temp)| async move {
            let read = file.read(&mut buffer).await?;
            if read == 0 {
                return Ok::<_, std::io::Error>(None);
            }
            let bytes = Bytes::copy_from_slice(&buffer[..read]);
            Ok(Some((bytes, (file, buffer, temp))))
        },
    );
    Ok(Body::from_stream(stream))
}

async fn streaming_payload_response(
    payload: SpooledFileDownloadPayload,
    content_type: HeaderValue,
    filename: &str,
    filename_error_code: &'static str,
    hash_error_code: &'static str,
    size_error_code: &'static str,
) -> Result<Response<Body>, ApiError> {
    let size_bytes = payload.size_bytes;
    let sha256_hex = payload.sha256_hex.clone();
    let body = streaming_temp_file_body(payload.temp).await?;
    let mut response = Response::new(body);
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, content_type);
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .map_err(|_| ApiError::conflict(filename_error_code))?,
    );
    response.headers_mut().insert(
        "x-vpsman-artifact-sha256",
        HeaderValue::from_str(&sha256_hex).map_err(|_| ApiError::conflict(hash_error_code))?,
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&size_bytes.to_string())
            .map_err(|_| ApiError::conflict(size_error_code))?,
    );
    response.headers_mut().insert(
        "x-vpsman-artifact-delivery",
        HeaderValue::from_static("spooled-filesystem"),
    );
    Ok(response)
}

fn parse_client_filter(value: Option<&str>) -> Option<BTreeSet<String>> {
    let clients = value?
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    if clients.is_empty() {
        None
    } else {
        Some(clients)
    }
}

fn latest_file_download_status(outputs: &[JobOutputView]) -> Option<FileDownloadStatus> {
    outputs
        .iter()
        .rev()
        .filter(|output| output.stream == "status")
        .filter_map(|output| BASE64.decode(&output.data_base64).ok())
        .filter_map(|data| serde_json::from_slice::<FileDownloadStatus>(&data).ok())
        .find(|status| {
            status.kind == "file_download"
                && status
                    .status
                    .as_deref()
                    .map(|value| value == "completed")
                    .unwrap_or(true)
        })
}

async fn materialize_output_bytes(
    state: &AppState,
    output: &JobOutputView,
) -> Result<Vec<u8>, ApiError> {
    if output.storage == "object_store" {
        let store = state
            .backup_object_store
            .as_ref()
            .ok_or_else(|| ApiError::not_found("job_output_artifact_not_available"))?;
        let object_key = output
            .artifact_object_key
            .as_deref()
            .ok_or_else(|| ApiError::not_found("job_output_artifact_not_found"))?;
        let bytes = store
            .get_with_limit(object_key, state.artifact_max_bytes())
            .await?;
        if let Some(expected_hash) = output.artifact_sha256_hex.as_deref() {
            if vpsman_common::payload_hash(&bytes) != expected_hash {
                return Err(ApiError::conflict("job_output_artifact_integrity_mismatch"));
            }
        }
        if let Some(expected_size) = output.artifact_size_bytes {
            if bytes.len() as i64 != expected_size {
                return Err(ApiError::conflict("job_output_artifact_integrity_mismatch"));
            }
        }
        return Ok(bytes);
    }
    if output.storage == "artifact_deleted" {
        return Err(ApiError::gone("job_output_artifact_deleted"));
    }
    BASE64
        .decode(&output.data_base64)
        .map_err(|_| ApiError::conflict("job_output_data_invalid"))
}

fn safe_tar_component(value: &str) -> String {
    let sanitized = value
        .trim_matches('/')
        .replace("..", "_")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | '=' | '@')
            {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "download".to_string()
    } else {
        sanitized
    }
}

pub(crate) async fn compare_job_outputs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
    Query(query): Query<JobOutputComparisonQuery>,
) -> Result<Json<JobOutputComparisonView>, ApiError> {
    let operator = state.require_operator_scope(&headers, "fleet:read").await?;
    let mode = query
        .mode
        .as_deref()
        .unwrap_or(&operator.operator.preferences.bulk_output_compare_mode);
    validate_output_compare_mode(mode)?;
    Ok(Json(state.repo.compare_job_outputs(job_id, mode).await?))
}

fn validate_output_compare_mode(mode: &str) -> Result<(), ApiError> {
    if matches!(mode.trim(), "binary" | "text") {
        Ok(())
    } else {
        Err(ApiError::bad_request("invalid_output_compare_mode"))
    }
}

pub(crate) async fn download_job_output_chunk(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((job_id, client_id, seq)): Path<(Uuid, String, i32)>,
) -> Result<Response<Body>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    let output = state
        .repo
        .list_job_outputs(job_id)
        .await?
        .into_iter()
        .find(|output| output.client_id == client_id && output.seq == seq)
        .ok_or_else(|| ApiError::not_found("job_output_download_not_found"))?;
    if !matches!(output.stream.as_str(), "stdout" | "stderr") {
        return Err(ApiError::bad_request("job_output_status_not_downloadable"));
    }
    if output.storage == "artifact_deleted" {
        return Err(ApiError::gone("job_output_artifact_deleted"));
    }
    if output.storage != "object_store" {
        let bytes = BASE64
            .decode(&output.data_base64)
            .map_err(|_| ApiError::conflict("job_output_data_invalid"))?;
        let sha256_hex = vpsman_common::payload_hash(&bytes);
        let mut response = Response::new(Body::from(bytes.clone()));
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        response.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&format!(
                "attachment; filename=\"job-output-{job_id}-{seq}.bin\""
            ))
            .map_err(|_| ApiError::conflict("job_output_download_filename_invalid"))?,
        );
        response.headers_mut().insert(
            "x-vpsman-artifact-sha256",
            HeaderValue::from_str(&sha256_hex)
                .map_err(|_| ApiError::conflict("job_output_download_hash_invalid"))?,
        );
        response.headers_mut().insert(
            header::CONTENT_LENGTH,
            HeaderValue::from_str(&bytes.len().to_string())
                .map_err(|_| ApiError::conflict("job_output_download_size_invalid"))?,
        );
        response.headers_mut().insert(
            "x-vpsman-artifact-delivery",
            HeaderValue::from_static("inline"),
        );
        return Ok(response);
    }
    let store = state
        .backup_object_store
        .as_ref()
        .ok_or_else(|| ApiError::not_found("job_output_artifact_not_available"))?;
    let artifact = state
        .repo
        .get_job_output_artifact_ref(job_id, &client_id, seq)
        .await?
        .ok_or_else(|| ApiError::not_found("job_output_artifact_not_found"))?;
    let expected_size = u64::try_from(artifact.size_bytes)
        .map_err(|_| ApiError::conflict("job_output_artifact_integrity_mismatch"))?;
    let object_file = store
        .verified_object_file(
            &artifact.object_key,
            &artifact.sha256_hex,
            expected_size,
            state.artifact_max_bytes(),
        )
        .await
        .map_err(|error| {
            map_verified_object_error(
                error,
                "job_output_artifact_not_found",
                "job_output_artifact_integrity_mismatch",
            )
        })?;
    let body = streaming_artifact_file_body(
        object_file.path,
        "job_output_artifact_not_found",
        object_file.cleanup_after_stream,
    )
    .await?;
    let mut response = Response::new(body);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"job-output-{job_id}-{seq}.bin\""
        ))
        .map_err(|_| ApiError::conflict("job_output_download_filename_invalid"))?,
    );
    response.headers_mut().insert(
        "x-vpsman-artifact-sha256",
        HeaderValue::from_str(&artifact.sha256_hex)
            .map_err(|_| ApiError::conflict("job_output_download_hash_invalid"))?,
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&expected_size.to_string())
            .map_err(|_| ApiError::conflict("job_output_download_size_invalid"))?,
    );
    Ok(response)
}

pub(crate) async fn list_process_supervisor_inventory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<ProcessSupervisorInventoryView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(
        state
            .repo
            .list_process_supervisor_inventory(limit_or_default(query.limit))
            .await?,
    ))
}

pub(crate) async fn list_audit_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<AuditLogView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(state.repo.query_audit_logs(&query).await?))
}

pub(crate) async fn list_network_observations(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<NetworkObservationView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(
        state
            .repo
            .list_network_observations(limit_or_default(query.limit))
            .await?,
    ))
}

pub(crate) async fn list_network_observation_trends(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<NetworkObservationTrendView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(
        state
            .repo
            .list_network_observation_trends(limit_or_default(query.limit))
            .await?,
    ))
}
