use std::{
    env,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use vpsman_common::{validate_absolute_file_path, JobCommand, RestoreRollbackFile};

use crate::{
    backup_artifact_validation::{
        validate_artifact_metadata, validate_artifact_object_key, MAX_BACKUP_ARTIFACT_UPLOAD_BYTES,
    },
    commands_schedules::{resolve_schedule_target_ids, selector_expression_from_targets},
    http::{http_get, http_post_json},
    jobs::resolve_target_ids,
    privilege::{
        build_privilege_for_job_command, build_privilege_for_schedule, load_super_password,
        load_super_salt_hex, SchedulePrivilegeRequest,
    },
};

const DEFAULT_BACKUP_ARTIFACT_UPLOAD_CHUNK_BYTES: usize = 4 * 1024 * 1024;
const RESTORE_DESTINATION_ROOT_BASE_ENV: &str = "VPSMAN_RESTORE_DESTINATION_ROOT_BASE";
const DEFAULT_RESTORE_DESTINATION_ROOT_BASE: &str = "/var/lib/vpsman/restores";

pub(crate) struct BackupPolicyUpsertOptions {
    pub(crate) name: String,
    pub(crate) paths: Vec<String>,
    pub(crate) include_config: bool,
    pub(crate) follow_symlinks: bool,
    pub(crate) clients: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) cron_expr: String,
    pub(crate) enabled: bool,
    pub(crate) catch_up_policy: String,
    pub(crate) catch_up_limit: i32,
    pub(crate) retry_delay_secs: i64,
    pub(crate) max_failures: i32,
    pub(crate) retention_days: Option<i32>,
    pub(crate) keep_last: Option<i32>,
    pub(crate) rotation_generation: Option<String>,
    pub(crate) confirmed: bool,
}

pub(crate) struct RestoreRunOptions {
    pub(crate) source_backup_request_id: String,
    pub(crate) target_client_id: String,
    pub(crate) archive_transfer_session_id: String,
    pub(crate) password_env: String,
    pub(crate) super_salt_hex: Option<String>,
    pub(crate) privilege_ttl_secs: u64,
    pub(crate) max_timeout_secs: u64,
    pub(crate) confirmed: bool,
    pub(crate) force_unprivileged: bool,
}

pub(crate) struct RestoreRunWithCredentials<'a> {
    pub(crate) source_backup_request_id: Uuid,
    pub(crate) target_client_id: String,
    pub(crate) archive_transfer_session_id: Uuid,
    pub(crate) paths: Vec<String>,
    pub(crate) include_config: bool,
    pub(crate) destination_root: Option<String>,
    pub(crate) password: &'a str,
    pub(crate) salt_hex: &'a str,
    pub(crate) privilege_ttl_secs: u64,
    pub(crate) max_timeout_secs: u64,
    pub(crate) confirmed: bool,
    pub(crate) force_unprivileged: bool,
}

#[derive(Debug, Deserialize)]
struct JobOutputRecord {
    client_id: String,
    stream: String,
    data_base64: String,
    exit_code: Option<i32>,
    done: bool,
}

#[derive(Debug, Deserialize)]
struct JobOutputListPage {
    items: Vec<JobOutputRecord>,
    next_cursor: Option<String>,
    has_more: bool,
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

#[derive(Debug, Deserialize)]
struct BackupRequestRecord {
    id: Uuid,
    artifact_id: Option<Uuid>,
    paths: Vec<String>,
    include_config: bool,
}

#[derive(Debug, Deserialize)]
struct BackupArtifactRecord {
    id: Uuid,
    sha256_hex: String,
    size_bytes: u64,
    status: String,
}

#[derive(Debug, Deserialize)]
struct FileTransferSessionRecord {
    session_id: Uuid,
    client_id: String,
    direction: String,
    status: String,
    path: String,
    size_bytes: Option<u64>,
    sha256_hex: Option<String>,
}

struct RestoreArchiveTransfer {
    path: String,
    size_bytes: u64,
    sha256_hex: String,
}

pub(crate) struct RestoreScope {
    pub(crate) paths: Vec<String>,
    pub(crate) include_config: bool,
    pub(crate) destination_root: Option<String>,
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
    preview_hash: Option<String>,
    confirmed: bool,
) -> Result<()> {
    let payload =
        backup_policy_prune_payload(schedule_id, dry_run, metadata_only, preview_hash, confirmed)?;
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
    preview_hash: Option<String>,
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
        "preview_hash": preview_hash,
        "confirmed": confirmed,
    }))
}

pub(crate) fn backup_policy_upsert(
    api_url: &str,
    token: Option<&str>,
    options: BackupPolicyUpsertOptions,
) -> Result<()> {
    validate_backup_scope(&options.paths, options.include_config)?;
    anyhow::ensure!(
        !options.clients.is_empty() || !options.tags.is_empty(),
        "backup-policy-upsert requires at least one target selector"
    );
    anyhow::ensure!(
        options.confirmed,
        "backup-policy-upsert requires --confirmed"
    );
    let selector_expression = selector_expression_from_targets(&options.clients, &options.tags);
    let target_ids = resolve_schedule_target_ids(api_url, token, &selector_expression)?;
    let operation = JobCommand::Backup {
        paths: options.paths.clone(),
        include_config: options.include_config,
        follow_symlinks: options.follow_symlinks,
    };
    let password = load_super_password("VPSMAN_SUPER_PASSWORD")?;
    let salt_hex = load_super_salt_hex(None)?;
    let privilege_assertion = build_privilege_for_schedule(
        SchedulePrivilegeRequest {
            action: "backup_policy.create",
            schedule_id: None,
            name: &options.name,
            command: &operation,
            command_type: "backup",
            selector_expression: &selector_expression,
            resolved_targets: &target_ids,
            cron_expr: &options.cron_expr,
            timezone: "UTC",
            enabled: options.enabled,
            catch_up_policy: &options.catch_up_policy,
            catch_up_limit: options.catch_up_limit,
            retry_delay_secs: options.retry_delay_secs,
            max_failures: options.max_failures,
            deferred_until: None,
            deleted: false,
        },
        &password,
        &salt_hex,
        300,
    )?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/backup-policies",
            token,
            &serde_json::json!({
                "name": options.name,
                "paths": options.paths,
                "include_config": options.include_config,
                "follow_symlinks": options.follow_symlinks,
                "selector_expression": selector_expression,
                "target_client_ids": target_ids,
                "cron_expr": options.cron_expr,
                "timezone": "UTC",
                "enabled": options.enabled,
                "catch_up_policy": options.catch_up_policy,
                "catch_up_limit": options.catch_up_limit,
                "retry_delay_secs": options.retry_delay_secs,
                "max_failures": options.max_failures,
                "retention_days": options.retention_days,
                "keep_last": options.keep_last,
                "rotation_generation": options.rotation_generation,
                "confirmed": options.confirmed,
                "privilege_assertion": privilege_assertion,
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
    anyhow::ensure!(metadata.len() > 0, "artifact file must not be empty");
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

    let upload_result = (|| -> Result<String> {
        let mut file = File::open(&artifact_file)
            .with_context(|| format!("failed to open artifact file {}", artifact_file.display()))?;
        let mut offset = session.next_offset_bytes;
        let mut buffer = vec![0_u8; effective_chunk_size];
        loop {
            let read = file.read(&mut buffer).with_context(|| {
                format!("failed to read artifact file {}", artifact_file.display())
            })?;
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
    })();
    if upload_result.is_err() {
        abort_backup_artifact_upload_session_best_effort(
            api_url,
            token,
            backup_request_id,
            session.upload_id,
        );
    }
    upload_result
}

fn abort_backup_artifact_upload_session_best_effort(
    api_url: &str,
    token: Option<&str>,
    backup_request_id: Uuid,
    upload_id: Uuid,
) {
    let _ = http_post_json(
        api_url,
        &format!("/api/v1/backups/{backup_request_id}/artifact-upload-sessions/{upload_id}/abort"),
        token,
        &serde_json::json!({
            "confirmed": true,
        }),
    );
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

pub(crate) fn backup_run(
    api_url: &str,
    token: Option<&str>,
    paths: Vec<String>,
    include_config: bool,
    follow_symlinks: bool,
    clients: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    max_timeout_secs: u64,
    confirmed: bool,
) -> Result<()> {
    validate_backup_scope(&paths, include_config)?;
    anyhow::ensure!(confirmed, "backup-run requires --confirmed");
    let password = load_super_password(&password_env)?;
    let salt_hex = load_super_salt_hex(super_salt_hex.as_deref())?;
    let selector_expression = selector_expression_from_targets(&clients, &tags);
    let target_ids = resolve_target_ids(api_url, token, &clients, &tags)?;
    let operation = JobCommand::Backup {
        paths: paths.clone(),
        include_config,
        follow_symlinks,
    };
    let privilege = build_privilege_for_job_command(
        &target_ids,
        &operation,
        "backup",
        &selector_expression,
        &password,
        &salt_hex,
        privilege_ttl_secs,
        max_timeout_secs,
        false,
        true,
    )?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/jobs",
            token,
            &serde_json::json!({
                "job_id": Uuid::new_v4(),
                "command": "backup",
                "argv": [],
                "selector_expression": selector_expression,
                "target_client_ids": target_ids,
                "privileged": true,
                "destructive": false,
                "confirmed": confirmed,
                "max_timeout_secs": max_timeout_secs,
                "operation": operation,
                "privilege_assertion": privilege.privilege_assertion,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn backup_request(
    api_url: &str,
    token: Option<&str>,
    client_id: String,
    paths: Vec<String>,
    include_config: bool,
    follow_symlinks: bool,
    note: Option<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    confirmed: bool,
) -> Result<()> {
    validate_backup_scope(&paths, include_config)?;
    let password = load_super_password(&password_env)?;
    let salt_hex = load_super_salt_hex(super_salt_hex.as_deref())?;
    let operation = JobCommand::Backup {
        paths: paths.clone(),
        include_config,
        follow_symlinks,
    };
    let target_ids = vec![client_id.clone()];
    let selector_expression = selector_expression_from_targets(&target_ids, &[]);
    let privilege = build_privilege_for_job_command(
        &target_ids,
        &operation,
        "backup",
        &selector_expression,
        &password,
        &salt_hex,
        privilege_ttl_secs,
        30,
        false,
        true,
    )?;

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
                "follow_symlinks": follow_symlinks,
                "confirmed": confirmed,
                "note": note,
                "privilege_assertion": privilege.privilege_assertion,
            }),
        )?
    );
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

pub(crate) fn restore_scope_from_backup(
    api_url: &str,
    token: Option<&str>,
    source_backup_request_id: Uuid,
    target_client_id: &str,
) -> Result<RestoreScope> {
    let backup = find_backup_request(api_url, token, source_backup_request_id)?;
    anyhow::ensure!(
        backup.include_config || !backup.paths.is_empty(),
        "source backup has no restorable config or paths"
    );
    for path in &backup.paths {
        anyhow::ensure!(
            path.starts_with('/'),
            "source backup paths must be absolute"
        );
        anyhow::ensure!(
            !path
                .split('/')
                .any(|segment| segment == "." || segment == ".."),
            "source backup paths must not contain . or .. segments"
        );
    }
    Ok(RestoreScope {
        paths: backup.paths,
        include_config: backup.include_config,
        destination_root: Some(generated_restore_destination_root(
            source_backup_request_id,
            target_client_id,
        )?),
    })
}

fn find_backup_request(
    api_url: &str,
    token: Option<&str>,
    source_backup_request_id: Uuid,
) -> Result<BackupRequestRecord> {
    let backups_body = http_get(api_url, "/api/v1/backups?limit=200", token)?;
    let backups: Vec<BackupRequestRecord> =
        serde_json::from_str(&backups_body).context("invalid backups JSON")?;
    backups
        .into_iter()
        .find(|backup| backup.id == source_backup_request_id)
        .context("source backup request was not found in latest 200 backups")
}

fn generated_restore_destination_root(
    source_backup_request_id: Uuid,
    target_client_id: &str,
) -> Result<String> {
    let base = env::var(RESTORE_DESTINATION_ROOT_BASE_ENV)
        .unwrap_or_else(|_| DEFAULT_RESTORE_DESTINATION_ROOT_BASE.to_string());
    generated_restore_destination_root_with_base(&base, source_backup_request_id, target_client_id)
}

fn generated_restore_destination_root_with_base(
    base: &str,
    source_backup_request_id: Uuid,
    target_client_id: &str,
) -> Result<String> {
    let base = base.trim_end_matches('/');
    validate_absolute_file_path(base).with_context(|| {
        format!("{RESTORE_DESTINATION_ROOT_BASE_ENV} must be an absolute file path")
    })?;
    Ok(format!(
        "{}/{}/{}",
        base,
        safe_restore_path_segment(&source_backup_request_id.to_string()),
        safe_restore_path_segment(target_client_id),
    ))
}

fn safe_restore_path_segment(value: &str) -> String {
    let segment: String = value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric()
                || *character == '.'
                || *character == '_'
                || *character == '-'
        })
        .take(120)
        .collect();
    if segment.is_empty() || segment == "." || segment == ".." {
        "unknown".to_string()
    } else {
        segment
    }
}

pub(crate) fn restore_plan(
    api_url: &str,
    token: Option<&str>,
    source_backup_request_id: String,
    target_client_id: String,
    note: Option<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    confirmed: bool,
) -> Result<()> {
    let source_backup_request_id =
        Uuid::parse_str(&source_backup_request_id).context("invalid source backup request UUID")?;
    let scope =
        restore_scope_from_backup(api_url, token, source_backup_request_id, &target_client_id)?;
    let password = load_super_password(&password_env)?;
    let salt_hex = load_super_salt_hex(super_salt_hex.as_deref())?;
    let operation = JobCommand::Restore {
        source_backup_request_id,
        archive_transfer_session_id: Uuid::nil(),
        paths: scope.paths.clone(),
        include_config: scope.include_config,
        destination_root: scope.destination_root.clone(),
        archive_path: None,
        archive_size_bytes: None,
        archive_sha256_hex: None,
        dry_run: false,
        post_restore_argv: Vec::new(),
    };
    let target_ids = vec![target_client_id.clone()];
    let selector_expression = selector_expression_from_targets(&target_ids, &[]);
    let privilege = build_privilege_for_job_command(
        &target_ids,
        &operation,
        "restore",
        &selector_expression,
        &password,
        &salt_hex,
        privilege_ttl_secs,
        30,
        false,
        true,
    )?;

    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/restore-plans",
            token,
            &serde_json::json!({
                "source_backup_request_id": source_backup_request_id,
                "target_client_id": target_client_id,
                "paths": scope.paths,
                "include_config": scope.include_config,
                "destination_root": scope.destination_root,
                "confirmed": confirmed,
                "note": note,
                "privilege_assertion": privilege.privilege_assertion,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn restore_run(
    api_url: &str,
    token: Option<&str>,
    options: RestoreRunOptions,
) -> Result<()> {
    anyhow::ensure!(options.confirmed, "restore-run requires --confirmed");
    let source_backup_request_id = Uuid::parse_str(&options.source_backup_request_id)
        .context("invalid source backup request UUID")?;
    let archive_transfer_session_id = Uuid::parse_str(&options.archive_transfer_session_id)
        .context("invalid archive transfer session UUID")?;
    let scope = restore_scope_from_backup(
        api_url,
        token,
        source_backup_request_id,
        &options.target_client_id,
    )?;
    let password = load_super_password(&options.password_env)?;
    let salt_hex = load_super_salt_hex(options.super_salt_hex.as_deref())?;
    println!(
        "{}",
        restore_run_with_credentials(
            api_url,
            token,
            RestoreRunWithCredentials {
                source_backup_request_id,
                target_client_id: options.target_client_id,
                archive_transfer_session_id,
                paths: scope.paths,
                include_config: scope.include_config,
                destination_root: scope.destination_root,
                password: &password,
                salt_hex: &salt_hex,
                privilege_ttl_secs: options.privilege_ttl_secs,
                max_timeout_secs: options.max_timeout_secs,
                confirmed: options.confirmed,
                force_unprivileged: options.force_unprivileged,
            },
        )?
    );
    Ok(())
}

pub(crate) fn restore_run_with_credentials(
    api_url: &str,
    token: Option<&str>,
    request: RestoreRunWithCredentials<'_>,
) -> Result<String> {
    let body = restore_run_request_with_credentials(api_url, token, request)?;
    http_post_json(api_url, "/api/v1/jobs", token, &body)
}

pub(crate) fn restore_run_request_with_credentials(
    api_url: &str,
    token: Option<&str>,
    request: RestoreRunWithCredentials<'_>,
) -> Result<serde_json::Value> {
    anyhow::ensure!(request.confirmed, "restore-run requires --confirmed");
    let archive = resolve_restore_archive_transfer(
        api_url,
        token,
        request.source_backup_request_id,
        &request.target_client_id,
        request.archive_transfer_session_id,
    )?;
    let operation = restore_run_operation(
        request.source_backup_request_id,
        request.archive_transfer_session_id,
        archive.path,
        archive.size_bytes,
        archive.sha256_hex,
        request.paths,
        request.include_config,
        request.destination_root,
    )?;
    let target_ids = vec![request.target_client_id.clone()];
    let selector_expression = selector_expression_from_targets(&target_ids, &[]);
    let privilege = build_privilege_for_job_command(
        &target_ids,
        &operation,
        "restore",
        &selector_expression,
        request.password,
        request.salt_hex,
        request.privilege_ttl_secs,
        request.max_timeout_secs,
        request.force_unprivileged,
        true,
    )?;

    Ok(serde_json::json!({
        "job_id": Uuid::new_v4(),
        "command": "restore",
        "argv": [],
        "selector_expression": selector_expression,
        "target_client_ids": target_ids,
        "privileged": true,
        "destructive": true,
        "confirmed": request.confirmed,
        "force_unprivileged": request.force_unprivileged,
        "max_timeout_secs": request.max_timeout_secs,
        "operation": operation,
        "privilege_assertion": privilege.privilege_assertion,
    }))
}

fn resolve_restore_archive_transfer(
    api_url: &str,
    token: Option<&str>,
    source_backup_request_id: Uuid,
    target_client_id: &str,
    archive_transfer_session_id: Uuid,
) -> Result<RestoreArchiveTransfer> {
    let backup = find_backup_request(api_url, token, source_backup_request_id)?;
    let artifact_id = backup
        .artifact_id
        .context("source backup has no artifact record")?;

    let artifacts_body = http_get(api_url, "/api/v1/backup-artifacts?limit=200", token)?;
    let artifacts: Vec<BackupArtifactRecord> =
        serde_json::from_str(&artifacts_body).context("invalid backup artifacts JSON")?;
    let artifact = artifacts
        .iter()
        .find(|artifact| artifact.id == artifact_id)
        .context("source backup artifact metadata was not found in latest 200 artifacts")?;
    ensure!(
        !matches!(artifact.status.as_str(), "creating" | "deleting"),
        "source backup artifact is not downloadable while status is {}",
        artifact.status
    );

    let transfers_body = http_get(api_url, "/api/v1/file-transfers?limit=200", token)?;
    let transfers: Vec<FileTransferSessionRecord> =
        serde_json::from_str(&transfers_body).context("invalid file transfers JSON")?;
    let transfer = transfers
        .iter()
        .find(|transfer| {
            transfer.session_id == archive_transfer_session_id
                && transfer.client_id == target_client_id
        })
        .context("archive transfer session was not found for target client")?;
    anyhow::ensure!(
        transfer.direction == "upload" && transfer.status == "completed",
        "archive transfer must be a completed upload"
    );
    anyhow::ensure!(
        transfer.path.starts_with('/'),
        "archive transfer path must be absolute"
    );
    let transfer_size = transfer
        .size_bytes
        .context("archive transfer size is missing")?;
    let transfer_sha = transfer
        .sha256_hex
        .as_deref()
        .context("archive transfer SHA-256 is missing")?
        .to_ascii_lowercase();
    anyhow::ensure!(
        transfer_size == artifact.size_bytes,
        "archive transfer size does not match source backup artifact"
    );
    anyhow::ensure!(
        transfer_sha == artifact.sha256_hex.to_ascii_lowercase(),
        "archive transfer SHA-256 does not match source backup artifact"
    );
    Ok(RestoreArchiveTransfer {
        path: transfer.path.clone(),
        size_bytes: transfer_size,
        sha256_hex: transfer_sha,
    })
}

pub(crate) fn restore_rollback(
    api_url: &str,
    token: Option<&str>,
    restore_job_id: String,
    target_client_id: String,
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    max_timeout_secs: u64,
    confirmed: bool,
    force_unprivileged: bool,
) -> Result<()> {
    anyhow::ensure!(confirmed, "restore-rollback requires --confirmed");
    let restore_job_id = Uuid::parse_str(&restore_job_id).context("invalid restore job UUID")?;
    let operation =
        restore_rollback_operation_from_api(api_url, token, restore_job_id, &target_client_id)?;
    let password = load_super_password(&password_env)?;
    let salt_hex = load_super_salt_hex(super_salt_hex.as_deref())?;
    let target_ids = vec![target_client_id.clone()];
    let selector_expression = selector_expression_from_targets(&target_ids, &[]);
    let privilege = build_privilege_for_job_command(
        &target_ids,
        &operation,
        "restore_rollback",
        &selector_expression,
        &password,
        &salt_hex,
        privilege_ttl_secs,
        max_timeout_secs,
        force_unprivileged,
        true,
    )?;

    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/jobs",
            token,
            &serde_json::json!({
                "job_id": Uuid::new_v4(),
                "command": "restore_rollback",
                "argv": [],
                "selector_expression": selector_expression,
                "target_client_ids": target_ids,
                "privileged": true,
                "destructive": true,
                "confirmed": confirmed,
                "force_unprivileged": force_unprivileged,
                "max_timeout_secs": max_timeout_secs,
                "operation": operation,
                "privilege_assertion": privilege.privilege_assertion,
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
    let outputs = fetch_job_outputs(
        api_url,
        token,
        restore_job_id,
        Some(target_client_id),
        Some("status"),
    )
    .context("invalid job outputs JSON")?;
    restore_rollback_operation_from_outputs(restore_job_id, target_client_id, &outputs)
}

fn fetch_job_outputs(
    api_url: &str,
    token: Option<&str>,
    job_id: Uuid,
    client_id: Option<&str>,
    stream: Option<&str>,
) -> Result<Vec<JobOutputRecord>> {
    let mut cursor = None;
    let mut outputs = Vec::new();
    loop {
        let mut params = vec!["limit=1000".to_string(), "include_data=true".to_string()];
        if let Some(cursor) = cursor.as_deref() {
            params.push(format!("cursor={}", percent_encode_query_value(cursor)));
        }
        if let Some(client_id) = client_id {
            params.push(format!(
                "client_id={}",
                percent_encode_query_value(client_id)
            ));
        }
        if let Some(stream) = stream {
            params.push(format!("stream={}", percent_encode_query_value(stream)));
        }
        let page_json = http_get(
            api_url,
            &format!("/api/v1/jobs/{job_id}/outputs?{}", params.join("&")),
            token,
        )?;
        let page = serde_json::from_str::<JobOutputListPage>(&page_json)
            .context("failed to parse job output page")?;
        outputs.extend(page.items);
        if !page.has_more {
            break;
        }
        cursor = page.next_cursor;
        anyhow::ensure!(cursor.is_some(), "job output page omitted next cursor");
    }
    Ok(outputs)
}

fn percent_encode_query_value(value: &str) -> String {
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
    archive_transfer_session_id: Uuid,
    archive_path: String,
    archive_size_bytes: u64,
    archive_sha256_hex: String,
    paths: Vec<String>,
    include_config: bool,
    destination_root: Option<String>,
) -> Result<JobCommand> {
    anyhow::ensure!(
        include_config || !paths.is_empty(),
        "restore-run needs backup config or at least one recorded path"
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
        "config restore requires a generated destination root for safety"
    );
    anyhow::ensure!(
        archive_path.starts_with('/'),
        "restore archive path must be absolute"
    );
    anyhow::ensure!(
        !archive_path
            .split('/')
            .any(|segment| segment == "." || segment == ".."),
        "restore archive path must not contain . or .. segments"
    );
    anyhow::ensure!(
        archive_size_bytes > 0,
        "restore archive size must be positive"
    );
    let archive_sha256_hex = archive_sha256_hex.trim().to_ascii_lowercase();
    anyhow::ensure!(
        archive_sha256_hex.len() == 64
            && archive_sha256_hex
                .as_bytes()
                .iter()
                .all(u8::is_ascii_hexdigit),
        "restore archive SHA-256 must be 64 hex characters"
    );
    Ok(JobCommand::Restore {
        source_backup_request_id,
        archive_transfer_session_id,
        paths,
        include_config,
        destination_root,
        archive_path: Some(archive_path),
        archive_size_bytes: Some(archive_size_bytes),
        archive_sha256_hex: Some(archive_sha256_hex),
        dry_run: false,
        post_restore_argv: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        backup_policy_prune_payload, generated_restore_destination_root_with_base,
        restore_rollback_operation_from_outputs, JobOutputRecord, BASE64,
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
        let payload = backup_policy_prune_payload(
            Some(schedule_id.to_string()),
            true,
            Some(false),
            Some("aa".repeat(32)),
            true,
        )
        .unwrap();

        assert_eq!(payload["schedule_id"], schedule_id.to_string());
        assert_eq!(payload["dry_run"], true);
        assert_eq!(payload["metadata_only"], false);
        assert_eq!(payload["preview_hash"], "aa".repeat(32));
        assert_eq!(payload["confirmed"], true);
        assert!(backup_policy_prune_payload(
            Some("not-a-uuid".to_string()),
            true,
            None,
            None,
            false,
        )
        .is_err());
    }

    #[test]
    fn generated_restore_destination_root_uses_safe_base_and_segments() {
        let backup_id = Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap();
        let root = generated_restore_destination_root_with_base(
            "/tmp/vpsman-restores/",
            backup_id,
            "edge a/../../ignored",
        )
        .unwrap();

        assert_eq!(
            root,
            "/tmp/vpsman-restores/11111111-2222-4333-8444-555555555555/edgea....ignored"
        );
        assert!(
            generated_restore_destination_root_with_base("relative", backup_id, "edge").is_err()
        );
        assert!(
            generated_restore_destination_root_with_base("/tmp/root", backup_id, "..")
                .unwrap()
                .ends_with("/unknown")
        );
    }
}
