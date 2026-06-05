use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use vpsman_common::{encode_inline_file_payload, payload_hash, JobCommand, RestoreRollbackFile};

use crate::{
    backup_artifact_crypto::{
        decrypt_backup_artifact, validate_artifact_metadata, validate_artifact_object_key,
        MAX_BACKUP_ARTIFACT_UPLOAD_BYTES,
    },
    http::{http_get, http_post_json},
    jobs::resolve_target_ids,
    proof::{build_envelopes_for_job_command, load_super_password, load_super_salt_hex},
};

pub(crate) use crate::backup_artifact_crypto::restore_artifact_bytes;

const MAX_BACKUP_ARTIFACT_CHUNKED_UPLOAD_BYTES: u64 = 128 * 1024 * 1024;
const DEFAULT_BACKUP_ARTIFACT_UPLOAD_CHUNK_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct JobOutputRecord {
    client_id: String,
    stream: String,
    data_base64: String,
    exit_code: Option<i32>,
    done: bool,
}

#[derive(Debug, Deserialize)]
struct RestoreStatusRecord {
    #[serde(rename = "type")]
    record_type: String,
    rollback_available: bool,
    restored_files: Vec<RestoredFileRecord>,
}

#[derive(Debug, Deserialize)]
struct RestoredFileRecord {
    archive_path: String,
    destination_path: String,
    size_bytes: u64,
    sha256_hex: String,
    rollback_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BackupArtifactUploadSessionRecord {
    upload_id: Uuid,
    next_offset_bytes: i64,
    max_chunk_bytes: usize,
}

pub(crate) fn backups(api_url: &str, token: Option<&str>, limit: u16) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!("/api/v1/backups?limit={}", limit.clamp(1, 200)),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn backup_artifacts(api_url: &str, token: Option<&str>, limit: u16) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!("/api/v1/backup-artifacts?limit={}", limit.clamp(1, 200)),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn backup_policies(api_url: &str, token: Option<&str>) -> Result<()> {
    println!("{}", http_get(api_url, "/api/v1/backup-policies", token)?);
    Ok(())
}

pub(crate) fn backup_policy_prune(
    api_url: &str,
    token: Option<&str>,
    schedule_id: Option<String>,
    dry_run: bool,
    metadata_only: Option<bool>,
    confirmed: bool,
) -> Result<()> {
    let payload = backup_policy_prune_payload(schedule_id, dry_run, metadata_only, confirmed)?;
    println!(
        "{}",
        http_post_json(api_url, "/api/v1/backup-policies/prune", token, &payload,)?
    );
    Ok(())
}

fn backup_policy_prune_payload(
    schedule_id: Option<String>,
    dry_run: bool,
    metadata_only: Option<bool>,
    confirmed: bool,
) -> Result<serde_json::Value> {
    let schedule_id = schedule_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .context("invalid backup policy schedule UUID")?;
    Ok(serde_json::json!({
        "schedule_id": schedule_id,
        "dry_run": dry_run,
        "metadata_only": metadata_only,
        "confirmed": confirmed,
    }))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn backup_policy_upsert(
    api_url: &str,
    token: Option<&str>,
    name: String,
    paths: Vec<String>,
    include_config: bool,
    recipient_public_key_hex: Option<String>,
    clients: Vec<String>,
    tags: Vec<String>,
    interval_secs: u64,
    start_at_unix: Option<u64>,
    enabled: bool,
    catch_up_policy: String,
    catch_up_limit: i32,
    retry_delay_secs: i64,
    max_failures: i32,
    retention_days: Option<i32>,
    keep_last: Option<i32>,
    rotation_generation: Option<String>,
    confirmed: bool,
) -> Result<()> {
    validate_backup_scope(&paths, include_config)?;
    validate_backup_recipient_public_key(recipient_public_key_hex.as_deref())?;
    anyhow::ensure!(
        !clients.is_empty() || !tags.is_empty(),
        "backup-policy-upsert requires at least one target selector"
    );
    anyhow::ensure!(confirmed, "backup-policy-upsert requires --confirmed");
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/backup-policies",
            token,
            &serde_json::json!({
                "name": name,
                "paths": paths,
                "include_config": include_config,
                "recipient_public_key_hex": recipient_public_key_hex,
                "clients": clients,
                "tags": tags,
                "interval_secs": interval_secs,
                "start_at_unix": start_at_unix,
                "enabled": enabled,
                "catch_up_policy": catch_up_policy,
                "catch_up_limit": catch_up_limit,
                "retry_delay_secs": retry_delay_secs,
                "max_failures": max_failures,
                "retention_days": retention_days,
                "keep_last": keep_last,
                "rotation_generation": rotation_generation,
                "confirmed": confirmed,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn restore_plans(api_url: &str, token: Option<&str>, limit: u16) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!("/api/v1/restore-plans?limit={}", limit.clamp(1, 200)),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn backup_artifact_record(
    api_url: &str,
    token: Option<&str>,
    backup_request_id: String,
    object_key: String,
    sha256_hex: String,
    size_bytes: i64,
    confirmed: bool,
) -> Result<()> {
    let backup_request_id =
        Uuid::parse_str(&backup_request_id).context("invalid backup request UUID")?;
    validate_artifact_metadata(&object_key, &sha256_hex, size_bytes)?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/backups/{backup_request_id}/artifact-metadata"),
            token,
            &serde_json::json!({
                "object_key": object_key,
                "sha256_hex": sha256_hex,
                "encrypted": true,
                "size_bytes": size_bytes,
                "confirmed": confirmed,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn backup_artifact_upload(
    api_url: &str,
    token: Option<&str>,
    backup_request_id: String,
    object_key: String,
    artifact_file: PathBuf,
    confirmed: bool,
) -> Result<()> {
    let backup_request_id =
        Uuid::parse_str(&backup_request_id).context("invalid backup request UUID")?;
    validate_artifact_object_key(&object_key)?;
    anyhow::ensure!(confirmed, "backup-artifact-upload requires --confirmed");
    let metadata = std::fs::metadata(&artifact_file)
        .with_context(|| format!("failed to stat artifact file {}", artifact_file.display()))?;
    anyhow::ensure!(metadata.is_file(), "artifact file must be a regular file");
    anyhow::ensure!(
        (1..=MAX_BACKUP_ARTIFACT_UPLOAD_BYTES).contains(&metadata.len()),
        "artifact file size must be between 1 and {MAX_BACKUP_ARTIFACT_UPLOAD_BYTES} bytes"
    );
    let artifact = std::fs::read(&artifact_file)
        .with_context(|| format!("failed to read artifact file {}", artifact_file.display()))?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/backups/{backup_request_id}/artifact"),
            token,
            &serde_json::json!({
                "object_key": object_key,
                "artifact_base64": BASE64.encode(artifact),
                "confirmed": confirmed,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn backup_artifact_upload_chunked(
    api_url: &str,
    token: Option<&str>,
    backup_request_id: String,
    object_key: String,
    artifact_file: PathBuf,
    chunk_size_bytes: usize,
    confirmed: bool,
) -> Result<()> {
    println!(
        "{}",
        backup_artifact_upload_chunked_response(
            api_url,
            token,
            backup_request_id,
            object_key,
            artifact_file,
            chunk_size_bytes,
            confirmed,
        )?
    );
    Ok(())
}

pub(crate) fn backup_artifact_upload_chunked_response(
    api_url: &str,
    token: Option<&str>,
    backup_request_id: String,
    object_key: String,
    artifact_file: PathBuf,
    chunk_size_bytes: usize,
    confirmed: bool,
) -> Result<String> {
    let backup_request_id =
        Uuid::parse_str(&backup_request_id).context("invalid backup request UUID")?;
    validate_artifact_object_key(&object_key)?;
    anyhow::ensure!(
        confirmed,
        "backup-artifact-upload-chunked requires --confirmed"
    );
    anyhow::ensure!(
        (1..=DEFAULT_BACKUP_ARTIFACT_UPLOAD_CHUNK_BYTES).contains(&chunk_size_bytes),
        "chunk size must be between 1 and {DEFAULT_BACKUP_ARTIFACT_UPLOAD_CHUNK_BYTES} bytes"
    );
    let metadata = std::fs::metadata(&artifact_file)
        .with_context(|| format!("failed to stat artifact file {}", artifact_file.display()))?;
    anyhow::ensure!(metadata.is_file(), "artifact file must be a regular file");
    anyhow::ensure!(
        (1..=MAX_BACKUP_ARTIFACT_CHUNKED_UPLOAD_BYTES).contains(&metadata.len()),
        "artifact file size must be between 1 and {MAX_BACKUP_ARTIFACT_CHUNKED_UPLOAD_BYTES} bytes"
    );
    let sha256_hex = sha256_file_hex(&artifact_file)?;
    let session_json = http_post_json(
        api_url,
        &format!("/api/v1/backups/{backup_request_id}/artifact-upload-sessions"),
        token,
        &serde_json::json!({
            "object_key": object_key,
            "expected_sha256_hex": sha256_hex,
            "expected_size_bytes": metadata.len(),
            "confirmed": confirmed,
        }),
    )?;
    let session = serde_json::from_str::<BackupArtifactUploadSessionRecord>(&session_json)
        .context("invalid backup artifact upload session JSON")?;
    let effective_chunk_size = chunk_size_bytes.min(session.max_chunk_bytes);
    anyhow::ensure!(
        effective_chunk_size > 0,
        "server returned invalid chunk size"
    );

    let mut file = File::open(&artifact_file)
        .with_context(|| format!("failed to open artifact file {}", artifact_file.display()))?;
    let mut offset = session.next_offset_bytes;
    let mut buffer = vec![0_u8; effective_chunk_size];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("failed to read artifact file {}", artifact_file.display()))?;
        if read == 0 {
            break;
        }
        let view_json = http_post_json(
            api_url,
            &format!(
                "/api/v1/backups/{backup_request_id}/artifact-upload-sessions/{}/chunks",
                session.upload_id
            ),
            token,
            &serde_json::json!({
                "offset_bytes": offset,
                "data_base64": BASE64.encode(&buffer[..read]),
            }),
        )?;
        let view = serde_json::from_str::<BackupArtifactUploadSessionRecord>(&view_json)
            .context("invalid backup artifact upload chunk JSON")?;
        offset = view.next_offset_bytes;
    }

    http_post_json(
        api_url,
        &format!(
            "/api/v1/backups/{backup_request_id}/artifact-upload-sessions/{}/commit",
            session.upload_id
        ),
        token,
        &serde_json::json!({
            "confirmed": confirmed,
        }),
    )
}

pub(crate) fn backup_artifact_handoff(
    api_url: &str,
    token: Option<&str>,
    backup_request_id: String,
    job_id: Option<String>,
    confirmed: bool,
) -> Result<()> {
    let backup_request_id =
        Uuid::parse_str(&backup_request_id).context("invalid backup request UUID")?;
    let job_id = job_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .context("invalid backup source job UUID")?;
    anyhow::ensure!(confirmed, "backup-artifact-handoff requires --confirmed");
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/backups/{backup_request_id}/artifact-handoff"),
            token,
            &serde_json::json!({
                "confirmed": confirmed,
                "job_id": job_id,
            }),
        )?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn backup_run(
    api_url: &str,
    token: Option<&str>,
    paths: Vec<String>,
    include_config: bool,
    recipient_public_key_hex: Option<String>,
    clients: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    proof_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
) -> Result<()> {
    validate_backup_scope(&paths, include_config)?;
    validate_backup_recipient_public_key(recipient_public_key_hex.as_deref())?;
    anyhow::ensure!(confirmed, "backup-run requires --confirmed");
    let password = load_super_password(&password_env)?;
    let salt_hex = load_super_salt_hex(super_salt_hex.as_deref())?;
    let target_ids = resolve_target_ids(api_url, token, &clients, &tags, false, confirmed)?;
    let operation = JobCommand::Backup {
        paths: paths.clone(),
        include_config,
        recipient_public_key_hex: recipient_public_key_hex.map(|value| value.to_ascii_lowercase()),
    };
    let envelopes = build_envelopes_for_job_command(
        &target_ids,
        &operation,
        &password,
        &salt_hex,
        proof_ttl_secs,
    )?
    .1;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/jobs",
            token,
            &serde_json::json!({
                "command": "backup",
                "argv": [],
                "clients": clients,
                "tags": tags,
                "privileged": true,
                "destructive": false,
                "confirmed": confirmed,
                "timeout_secs": timeout_secs,
                "operation": operation,
                "envelope": null,
                "envelopes": envelopes,
            }),
        )?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn backup_request(
    api_url: &str,
    token: Option<&str>,
    client_id: String,
    paths: Vec<String>,
    include_config: bool,
    recipient_public_key_hex: Option<String>,
    note: Option<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    proof_ttl_secs: u64,
    confirmed: bool,
) -> Result<()> {
    validate_backup_scope(&paths, include_config)?;
    validate_backup_recipient_public_key(recipient_public_key_hex.as_deref())?;
    let password = load_super_password(&password_env)?;
    let salt_hex = load_super_salt_hex(super_salt_hex.as_deref())?;
    let operation = JobCommand::Backup {
        paths: paths.clone(),
        include_config,
        recipient_public_key_hex: recipient_public_key_hex
            .clone()
            .map(|value| value.to_ascii_lowercase()),
    };
    let mut envelopes = build_envelopes_for_job_command(
        std::slice::from_ref(&client_id),
        &operation,
        &password,
        &salt_hex,
        proof_ttl_secs,
    )?
    .1;
    let envelope = envelopes
        .remove(&client_id)
        .context("failed to build backup proof envelope")?;

    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/backups",
            token,
            &serde_json::json!({
                "client_id": client_id,
                "paths": paths,
                "include_config": include_config,
                "recipient_public_key_hex": recipient_public_key_hex,
                "confirmed": confirmed,
                "note": note,
                "envelope": envelope,
            }),
        )?
    );
    Ok(())
}

fn validate_backup_recipient_public_key(value: Option<&str>) -> Result<()> {
    if let Some(value) = value {
        anyhow::ensure!(
            value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "backup recipient public key must be 32-byte hex"
        );
    }
    Ok(())
}

fn validate_backup_scope(paths: &[String], include_config: bool) -> Result<()> {
    anyhow::ensure!(
        include_config || !paths.is_empty(),
        "backup needs --include-config or at least one --paths entry"
    );
    for path in paths {
        anyhow::ensure!(path.starts_with('/'), "backup paths must be absolute");
    }
    Ok(())
}

fn sha256_file_hex(path: &Path) -> Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open file {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("failed to read file {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn restore_plan(
    api_url: &str,
    token: Option<&str>,
    source_backup_request_id: String,
    target_client_id: String,
    paths: Vec<String>,
    include_config: bool,
    destination_root: Option<String>,
    note: Option<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    proof_ttl_secs: u64,
    confirmed: bool,
) -> Result<()> {
    anyhow::ensure!(
        include_config || !paths.is_empty(),
        "restore-plan needs --include-config or at least one --paths entry"
    );
    for path in &paths {
        anyhow::ensure!(path.starts_with('/'), "restore paths must be absolute");
    }
    if let Some(destination_root) = &destination_root {
        anyhow::ensure!(
            destination_root.starts_with('/'),
            "restore destination root must be absolute"
        );
    }
    let source_backup_request_id =
        Uuid::parse_str(&source_backup_request_id).context("invalid source backup request UUID")?;
    let password = load_super_password(&password_env)?;
    let salt_hex = load_super_salt_hex(super_salt_hex.as_deref())?;
    let operation = JobCommand::Restore {
        source_backup_request_id,
        paths: paths.clone(),
        include_config,
        destination_root: destination_root.clone(),
        archive_path: None,
        archive_base64: None,
        archive_size_bytes: None,
        archive_sha256_hex: None,
        dry_run: false,
        post_restore_argv: Vec::new(),
    };
    let mut envelopes = build_envelopes_for_job_command(
        std::slice::from_ref(&target_client_id),
        &operation,
        &password,
        &salt_hex,
        proof_ttl_secs,
    )?
    .1;
    let envelope = envelopes
        .remove(&target_client_id)
        .context("failed to build restore proof envelope")?;

    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/restore-plans",
            token,
            &serde_json::json!({
                "source_backup_request_id": source_backup_request_id,
                "target_client_id": target_client_id,
                "paths": paths,
                "include_config": include_config,
                "destination_root": destination_root,
                "confirmed": confirmed,
                "note": note,
                "envelope": envelope,
            }),
        )?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn restore_run(
    api_url: &str,
    token: Option<&str>,
    source_backup_request_id: String,
    target_client_id: String,
    artifact_file: Option<PathBuf>,
    private_key_env: String,
    paths: Vec<String>,
    include_config: bool,
    destination_root: Option<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    proof_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
    force_unprivileged: bool,
) -> Result<()> {
    anyhow::ensure!(confirmed, "restore-run requires --confirmed");
    let source_backup_request_id =
        Uuid::parse_str(&source_backup_request_id).context("invalid source backup request UUID")?;
    let password = load_super_password(&password_env)?;
    let salt_hex = load_super_salt_hex(super_salt_hex.as_deref())?;
    println!(
        "{}",
        restore_run_with_credentials(
            api_url,
            token,
            source_backup_request_id,
            target_client_id,
            artifact_file,
            private_key_env,
            paths,
            include_config,
            destination_root,
            &password,
            &salt_hex,
            proof_ttl_secs,
            timeout_secs,
            confirmed,
            force_unprivileged,
        )?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn restore_run_with_credentials(
    api_url: &str,
    token: Option<&str>,
    source_backup_request_id: Uuid,
    target_client_id: String,
    artifact_file: Option<PathBuf>,
    private_key_env: String,
    paths: Vec<String>,
    include_config: bool,
    destination_root: Option<String>,
    password: &str,
    salt_hex: &str,
    proof_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
    force_unprivileged: bool,
) -> Result<String> {
    anyhow::ensure!(confirmed, "restore-run requires --confirmed");
    let artifact_bytes = restore_artifact_bytes(
        api_url,
        token,
        source_backup_request_id,
        artifact_file.as_ref(),
    )?;
    let operation = restore_run_operation(
        source_backup_request_id,
        &artifact_bytes,
        &private_key_env,
        paths,
        include_config,
        destination_root,
    )?;
    let mut envelopes = build_envelopes_for_job_command(
        std::slice::from_ref(&target_client_id),
        &operation,
        password,
        salt_hex,
        proof_ttl_secs,
    )?
    .1;
    let envelope = envelopes
        .remove(&target_client_id)
        .context("failed to build restore proof envelope")?;

    http_post_json(
        api_url,
        "/api/v1/jobs",
        token,
        &serde_json::json!({
            "command": "restore",
            "argv": [],
            "clients": [target_client_id],
            "tags": [],
            "privileged": true,
            "destructive": true,
            "confirmed": confirmed,
            "force_unprivileged": force_unprivileged,
            "timeout_secs": timeout_secs,
            "operation": operation,
            "envelope": envelope,
            "envelopes": {},
        }),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn restore_rollback(
    api_url: &str,
    token: Option<&str>,
    restore_job_id: String,
    target_client_id: String,
    password_env: String,
    super_salt_hex: Option<String>,
    proof_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
    force_unprivileged: bool,
) -> Result<()> {
    anyhow::ensure!(confirmed, "restore-rollback requires --confirmed");
    let restore_job_id = Uuid::parse_str(&restore_job_id).context("invalid restore job UUID")?;
    let operation =
        restore_rollback_operation_from_api(api_url, token, restore_job_id, &target_client_id)?;
    let password = load_super_password(&password_env)?;
    let salt_hex = load_super_salt_hex(super_salt_hex.as_deref())?;
    let mut envelopes = build_envelopes_for_job_command(
        std::slice::from_ref(&target_client_id),
        &operation,
        &password,
        &salt_hex,
        proof_ttl_secs,
    )?
    .1;
    let envelope = envelopes
        .remove(&target_client_id)
        .context("failed to build restore rollback proof envelope")?;

    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/jobs",
            token,
            &serde_json::json!({
                "command": "restore_rollback",
                "argv": [],
                "clients": [target_client_id],
                "tags": [],
                "privileged": true,
                "destructive": true,
                "confirmed": confirmed,
                "force_unprivileged": force_unprivileged,
                "timeout_secs": timeout_secs,
                "operation": operation,
                "envelope": envelope,
                "envelopes": {},
            }),
        )?
    );
    Ok(())
}

pub(crate) fn restore_rollback_operation_from_api(
    api_url: &str,
    token: Option<&str>,
    restore_job_id: Uuid,
    target_client_id: &str,
) -> Result<JobCommand> {
    let outputs = http_get(
        api_url,
        &format!("/api/v1/jobs/{restore_job_id}/outputs"),
        token,
    )?;
    let outputs = serde_json::from_str::<Vec<JobOutputRecord>>(&outputs)
        .context("invalid job outputs JSON")?;
    restore_rollback_operation_from_outputs(restore_job_id, target_client_id, &outputs)
}

fn restore_rollback_operation_from_outputs(
    restore_job_id: Uuid,
    target_client_id: &str,
    outputs: &[JobOutputRecord],
) -> Result<JobCommand> {
    let output = outputs
        .iter()
        .find(|output| {
            output.client_id == target_client_id
                && output.stream == "status"
                && output.done
                && output.exit_code == Some(0)
        })
        .context("restore status output was not found for target client")?;
    let status_bytes = BASE64
        .decode(&output.data_base64)
        .context("restore status output is not base64")?;
    let status = serde_json::from_slice::<RestoreStatusRecord>(&status_bytes)
        .context("restore status output is not valid restore JSON")?;
    anyhow::ensure!(
        status.record_type == "restore",
        "job output is not a restore status"
    );
    anyhow::ensure!(
        status.rollback_available,
        "restore status does not allow rollback"
    );
    anyhow::ensure!(
        !status.restored_files.is_empty(),
        "restore status has no restored files"
    );
    let restored_files = status
        .restored_files
        .into_iter()
        .map(|file| RestoreRollbackFile {
            archive_path: file.archive_path,
            destination_path: file.destination_path,
            rollback_path: file.rollback_path,
            restored_size_bytes: file.size_bytes,
            restored_sha256_hex: file.sha256_hex,
        })
        .collect();
    Ok(JobCommand::RestoreRollback {
        source_restore_job_id: restore_job_id,
        restored_files,
    })
}

pub(crate) fn restore_run_operation(
    source_backup_request_id: Uuid,
    artifact_bytes: &[u8],
    private_key_env: &str,
    paths: Vec<String>,
    include_config: bool,
    destination_root: Option<String>,
) -> Result<JobCommand> {
    anyhow::ensure!(
        include_config || !paths.is_empty(),
        "restore-run needs --include-config or at least one --paths entry"
    );
    for path in &paths {
        anyhow::ensure!(path.starts_with('/'), "restore paths must be absolute");
        anyhow::ensure!(
            !path
                .split('/')
                .any(|segment| segment == "." || segment == ".."),
            "restore paths must not contain . or .. segments"
        );
    }
    if let Some(destination_root) = &destination_root {
        anyhow::ensure!(
            destination_root.starts_with('/'),
            "restore destination root must be absolute"
        );
        anyhow::ensure!(
            !destination_root
                .split('/')
                .any(|segment| segment == "." || segment == ".."),
            "restore destination root must not contain . or .. segments"
        );
    }
    anyhow::ensure!(
        !include_config || destination_root.is_some(),
        "restore-run --include-config requires --destination-root for safety"
    );
    let private_key_hex = std::env::var(private_key_env)
        .with_context(|| format!("environment variable {private_key_env} is not set"))?;
    let archive_bytes = decrypt_backup_artifact(artifact_bytes, &private_key_hex)?;
    let archive_base64 = encode_inline_file_payload(&archive_bytes)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let archive_sha256_hex = payload_hash(&archive_bytes);
    Ok(JobCommand::Restore {
        source_backup_request_id,
        paths,
        include_config,
        destination_root,
        archive_path: None,
        archive_base64: Some(archive_base64),
        archive_size_bytes: Some(archive_bytes.len() as u64),
        archive_sha256_hex: Some(archive_sha256_hex),
        dry_run: false,
        post_restore_argv: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        backup_policy_prune_payload, restore_rollback_operation_from_outputs, JobOutputRecord,
        BASE64,
    };
    use base64::Engine as _;
    use uuid::Uuid;
    use vpsman_common::JobCommand;

    const TEST_RESTORE_ARCHIVE_PATH: &str = "/etc/hostname";
    const TEST_RESTORE_DESTINATION_PATH: &str = "/restore/etc/hostname";
    const TEST_RESTORE_ROLLBACK_PATH: &str = "/restore/etc/.vpsman-restore-hostname.bak";

    #[test]
    fn builds_restore_rollback_operation_from_restore_status_output() {
        let restore_job_id = Uuid::new_v4();
        let status = serde_json::json!({
            "type": "restore",
            "rollback_available": true,
            "restored_files": [
                {
                    "archive_path": TEST_RESTORE_ARCHIVE_PATH,
                    "destination_path": TEST_RESTORE_DESTINATION_PATH,
                    "size_bytes": 12,
                    "sha256_hex": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "rollback_path": TEST_RESTORE_ROLLBACK_PATH
                },
                {
                    "archive_path": "agent_config",
                    "destination_path": "/restore/vpsman/agent_config.toml",
                    "size_bytes": 21,
                    "sha256_hex": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "rollback_path": null
                }
            ]
        });
        let outputs = vec![JobOutputRecord {
            client_id: "client-a".to_string(),
            stream: "status".to_string(),
            data_base64: BASE64.encode(serde_json::to_vec(&status).unwrap()),
            exit_code: Some(0),
            done: true,
        }];

        let operation =
            restore_rollback_operation_from_outputs(restore_job_id, "client-a", &outputs).unwrap();

        let JobCommand::RestoreRollback {
            source_restore_job_id,
            restored_files,
        } = operation
        else {
            panic!("expected restore rollback operation");
        };
        assert_eq!(source_restore_job_id, restore_job_id);
        assert_eq!(restored_files.len(), 2);
        assert_eq!(restored_files[0].archive_path, TEST_RESTORE_ARCHIVE_PATH);
        assert_eq!(
            restored_files[0].rollback_path.as_deref(),
            Some(TEST_RESTORE_ROLLBACK_PATH)
        );
        assert_eq!(restored_files[1].rollback_path, None);
    }

    #[test]
    fn builds_backup_policy_prune_payload() {
        let schedule_id = Uuid::new_v4();
        let payload =
            backup_policy_prune_payload(Some(schedule_id.to_string()), true, Some(false), true)
                .unwrap();

        assert_eq!(payload["schedule_id"], schedule_id.to_string());
        assert_eq!(payload["dry_run"], true);
        assert_eq!(payload["metadata_only"], false);
        assert_eq!(payload["confirmed"], true);
        assert!(
            backup_policy_prune_payload(Some("not-a-uuid".to_string()), true, None, false,)
                .is_err()
        );
    }
}
