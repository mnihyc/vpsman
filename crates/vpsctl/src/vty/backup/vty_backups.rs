use anyhow::{Context, Result};
use uuid::Uuid;
use vpsman_common::{BackupMissingPathPolicy, JobCommand, MAX_CONFIGURABLE_JOB_TIMEOUT_SECS};

use crate::{
    commands_backups::{
        backup_policy_upsert_target, build_backup_metadata_privilege, build_restore_plan_privilege,
        restore_rollback_operation_from_api, restore_run_with_credentials,
        restore_scope_from_backup, validate_backup_policy_upsert_mode, RestoreRunWithCredentials,
    },
    commands_schedules::{
        resolve_schedule_target_ids, saved_schedule_target_snapshot,
        selector_expression_from_targets,
    },
    http::{http_post_json, http_put_json},
    privilege::{
        build_privilege_for_schedule, load_super_password, load_super_salt_hex,
        SchedulePrivilegePayload, SchedulePrivilegeRequest,
    },
    vty_jobs::{
        vty_submit_operation, vty_submit_operation_with_force, VtyJobSelection, VtyPrivilegeContext,
    },
};

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct VtyBackupRequest {
    pub(crate) client_id: String,
    pub(crate) paths: Vec<String>,
    pub(crate) include_config: bool,
    pub(crate) follow_symlinks: bool,
    pub(crate) skip_missing_paths: bool,
    pub(crate) confirmed: bool,
    pub(crate) note: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct VtyRestorePlanRequest {
    pub(crate) source_backup_request_id: Uuid,
    pub(crate) target_client_id: String,
    pub(crate) confirmed: bool,
    pub(crate) note: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct VtyRestoreRunRequest {
    pub(crate) source_backup_request_id: Uuid,
    pub(crate) target_client_id: String,
    pub(crate) archive_transfer_session_id: Uuid,
    pub(crate) max_timeout_secs: u64,
    pub(crate) confirmed: bool,
    pub(crate) dry_run: bool,
    pub(crate) force_unprivileged: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct VtyRestoreRollbackRequest {
    pub(crate) restore_job_id: Uuid,
    pub(crate) target_client_id: String,
    pub(crate) max_timeout_secs: u64,
    pub(crate) confirmed: bool,
    pub(crate) force_unprivileged: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct VtyBackupRunRequest {
    pub(crate) paths: Vec<String>,
    pub(crate) include_config: bool,
    pub(crate) follow_symlinks: bool,
    pub(crate) skip_missing_paths: bool,
    pub(crate) selection: VtyJobSelection,
    pub(crate) max_timeout_secs: u64,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct VtyBackupPolicyUpsert {
    pub(crate) schedule_id: Option<Uuid>,
    pub(crate) name: String,
    pub(crate) paths: Vec<String>,
    pub(crate) include_config: bool,
    pub(crate) follow_symlinks: bool,
    pub(crate) skip_missing_paths: bool,
    pub(crate) selection: VtyJobSelection,
    pub(crate) cron_expr: String,
    pub(crate) enabled: bool,
    pub(crate) catch_up_policy: String,
    pub(crate) catch_up_limit: i32,
    pub(crate) retry_delay_secs: i64,
    pub(crate) max_failures: i32,
    pub(crate) retention_days: Option<i32>,
    pub(crate) keep_last: Option<i32>,
    pub(crate) rotation_generation: Option<String>,
    pub(crate) clear_rotation_generation: bool,
}

pub(crate) fn parse_vty_backup_request(tokens: &[&str]) -> Result<VtyBackupRequest> {
    let client_id = tokens
        .first()
        .context("usage: backup-request <client_id> [--path <abs>] [--include-config] [--follow-symlinks] [--skip-missing-paths] [--confirmed] [--note <text>]")?
        .to_string();
    let mut request = VtyBackupRequest {
        client_id,
        paths: Vec::new(),
        include_config: false,
        follow_symlinks: false,
        skip_missing_paths: false,
        confirmed: false,
        note: None,
    };
    let mut index = 1;
    while index < tokens.len() {
        match tokens[index] {
            "--include-config" => {
                request.include_config = true;
                index += 1;
            }
            "--follow-symlinks" => {
                request.follow_symlinks = true;
                index += 1;
            }
            "--skip-missing-paths" => {
                request.skip_missing_paths = true;
                index += 1;
            }
            "--confirmed" => {
                request.confirmed = true;
                index += 1;
            }
            "--path" => {
                request.paths.push(
                    tokens
                        .get(index + 1)
                        .context("--path requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            value if value.starts_with("--path=") => {
                request
                    .paths
                    .push(value.trim_start_matches("--path=").to_string());
                index += 1;
            }
            "--note" => {
                request.note = Some(
                    tokens
                        .get(index + 1)
                        .context("--note requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            value if value.starts_with("--note=") => {
                request.note = Some(value.trim_start_matches("--note=").to_string());
                index += 1;
            }
            other => anyhow::bail!("unknown backup-request flag {other}"),
        }
    }
    ensure_backup_scope(&request.paths, request.include_config, "backup-request")?;
    Ok(request)
}

pub(crate) fn parse_vty_backup_run(tokens: &[&str]) -> Result<VtyBackupRunRequest> {
    let mut paths = Vec::new();
    let mut include_config = false;
    let mut follow_symlinks = false;
    let mut skip_missing_paths = false;
    let mut max_timeout_secs = 60_u64;
    let mut target_tokens = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--include-config" => {
                include_config = true;
                index += 1;
            }
            "--follow-symlinks" => {
                follow_symlinks = true;
                index += 1;
            }
            "--skip-missing-paths" => {
                skip_missing_paths = true;
                index += 1;
            }
            "--path" => {
                paths.push(
                    tokens
                        .get(index + 1)
                        .context("--path requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            value if value.starts_with("--path=") => {
                paths.push(value.trim_start_matches("--path=").to_string());
                index += 1;
            }
            "--max-timeout" => {
                max_timeout_secs = tokens
                    .get(index + 1)
                    .context("--max-timeout requires a value")?
                    .parse()
                    .context("invalid --max-timeout")?;
                index += 2;
            }
            value if value.starts_with("--max-timeout=") => {
                max_timeout_secs = value
                    .trim_start_matches("--max-timeout=")
                    .parse()
                    .context("invalid --max-timeout")?;
                index += 1;
            }
            "--recipient-public-key-hex" | "--recipient-public-key" => {
                anyhow::bail!("backup recipient public keys were removed")
            }
            value
                if value.starts_with("--recipient-public-key-hex=")
                    || value.starts_with("--recipient-public-key=") =>
            {
                anyhow::bail!("backup recipient public keys were removed")
            }
            value => {
                target_tokens.push(value);
                index += 1;
            }
        }
    }
    ensure_backup_scope(&paths, include_config, "backup-run")?;
    anyhow::ensure!(
        (1..=MAX_CONFIGURABLE_JOB_TIMEOUT_SECS).contains(&max_timeout_secs),
        "backup timeout out of range"
    );
    Ok(VtyBackupRunRequest {
        paths,
        include_config,
        follow_symlinks,
        skip_missing_paths,
        selection: VtyJobSelection::parse(&target_tokens)?,
        max_timeout_secs,
    })
}

pub(crate) fn parse_vty_backup_policy_upsert(tokens: &[&str]) -> Result<VtyBackupPolicyUpsert> {
    let name = tokens
        .first()
        .context("usage: backup-policy-upsert <name> [--schedule-id <uuid>] [--path <abs>] [--include-config] [--follow-symlinks] [--skip-missing-paths] [--cron <min> <hour> <dom> <mon> <dow>] [--retention-days <n>] [--keep-last <n>] [--rotation-generation <id> | --clear-rotation-generation] [--disabled] <target>... --confirmed")?
        .to_string();
    let mut schedule_id = None;
    let mut paths = Vec::new();
    let mut include_config = false;
    let mut follow_symlinks = false;
    let mut skip_missing_paths = false;
    let mut cron_expr = "0 3 * * *".to_string();
    let mut enabled = true;
    let mut catch_up_policy = "skip_missed".to_string();
    let mut catch_up_limit = 1_i32;
    let mut retry_delay_secs = 300_i64;
    let mut max_failures = 3_i32;
    let mut retention_days = None;
    let mut keep_last = None;
    let mut rotation_generation = None;
    let mut clear_rotation_generation = false;
    let mut target_tokens = Vec::new();
    let mut index = 1;
    while index < tokens.len() {
        match tokens[index] {
            "--schedule-id" => {
                schedule_id = Some(
                    Uuid::parse_str(
                        tokens
                            .get(index + 1)
                            .context("--schedule-id requires a value")?,
                    )
                    .context("invalid --schedule-id")?,
                );
                index += 2;
            }
            value if value.starts_with("--schedule-id=") => {
                schedule_id = Some(
                    Uuid::parse_str(value.trim_start_matches("--schedule-id="))
                        .context("invalid --schedule-id")?,
                );
                index += 1;
            }
            "--include-config" => {
                include_config = true;
                index += 1;
            }
            "--follow-symlinks" => {
                follow_symlinks = true;
                index += 1;
            }
            "--skip-missing-paths" => {
                skip_missing_paths = true;
                index += 1;
            }
            "--disabled" => {
                enabled = false;
                index += 1;
            }
            "--path" => {
                paths.push(
                    tokens
                        .get(index + 1)
                        .context("--path requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            value if value.starts_with("--path=") => {
                paths.push(value.trim_start_matches("--path=").to_string());
                index += 1;
            }
            "--cron" | "--cron-expr" => {
                let cron_tokens = tokens
                    .get(index + 1..index + 6)
                    .context("--cron requires five UTC cron fields")?;
                cron_expr = cron_tokens.join(" ");
                index += 6;
            }
            value if value.starts_with("--cron=") => {
                cron_expr = normalize_vty_cron_value(value.trim_start_matches("--cron="));
                index += 1;
            }
            value if value.starts_with("--cron-expr=") => {
                cron_expr = normalize_vty_cron_value(value.trim_start_matches("--cron-expr="));
                index += 1;
            }
            "--interval" => {
                anyhow::bail!("--interval was replaced by --cron <min> <hour> <dom> <mon> <dow>");
            }
            value if value.starts_with("--interval=") => {
                anyhow::bail!("--interval was replaced by --cron=<min>,<hour>,<dom>,<mon>,<dow>");
            }
            "--retention-days" => {
                retention_days = Some(
                    tokens
                        .get(index + 1)
                        .context("--retention-days requires a value")?
                        .parse()
                        .context("invalid --retention-days")?,
                );
                index += 2;
            }
            value if value.starts_with("--retention-days=") => {
                retention_days = Some(
                    value
                        .trim_start_matches("--retention-days=")
                        .parse()
                        .context("invalid --retention-days")?,
                );
                index += 1;
            }
            "--keep-last" => {
                keep_last = Some(
                    tokens
                        .get(index + 1)
                        .context("--keep-last requires a value")?
                        .parse()
                        .context("invalid --keep-last")?,
                );
                index += 2;
            }
            value if value.starts_with("--keep-last=") => {
                keep_last = Some(
                    value
                        .trim_start_matches("--keep-last=")
                        .parse()
                        .context("invalid --keep-last")?,
                );
                index += 1;
            }
            "--rotation-generation" => {
                rotation_generation = Some(
                    tokens
                        .get(index + 1)
                        .context("--rotation-generation requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            value if value.starts_with("--rotation-generation=") => {
                rotation_generation = Some(
                    value
                        .trim_start_matches("--rotation-generation=")
                        .to_string(),
                );
                index += 1;
            }
            "--clear-rotation-generation" => {
                clear_rotation_generation = true;
                index += 1;
            }
            "--catch-up-policy" => {
                catch_up_policy = tokens
                    .get(index + 1)
                    .context("--catch-up-policy requires a value")?
                    .to_string();
                index += 2;
            }
            value if value.starts_with("--catch-up-policy=") => {
                catch_up_policy = value.trim_start_matches("--catch-up-policy=").to_string();
                index += 1;
            }
            "--catch-up-limit" => {
                catch_up_limit = tokens
                    .get(index + 1)
                    .context("--catch-up-limit requires a value")?
                    .parse()
                    .context("invalid --catch-up-limit")?;
                index += 2;
            }
            value if value.starts_with("--catch-up-limit=") => {
                catch_up_limit = value
                    .trim_start_matches("--catch-up-limit=")
                    .parse()
                    .context("invalid --catch-up-limit")?;
                index += 1;
            }
            "--retry-delay" => {
                retry_delay_secs = tokens
                    .get(index + 1)
                    .context("--retry-delay requires a value")?
                    .parse()
                    .context("invalid --retry-delay")?;
                index += 2;
            }
            value if value.starts_with("--retry-delay=") => {
                retry_delay_secs = value
                    .trim_start_matches("--retry-delay=")
                    .parse()
                    .context("invalid --retry-delay")?;
                index += 1;
            }
            "--max-failures" => {
                max_failures = tokens
                    .get(index + 1)
                    .context("--max-failures requires a value")?
                    .parse()
                    .context("invalid --max-failures")?;
                index += 2;
            }
            value if value.starts_with("--max-failures=") => {
                max_failures = value
                    .trim_start_matches("--max-failures=")
                    .parse()
                    .context("invalid --max-failures")?;
                index += 1;
            }
            "--recipient-public-key-hex" | "--recipient-public-key" => {
                anyhow::bail!("backup recipient public keys were removed")
            }
            value
                if value.starts_with("--recipient-public-key-hex=")
                    || value.starts_with("--recipient-public-key=") =>
            {
                anyhow::bail!("backup recipient public keys were removed")
            }
            value => {
                target_tokens.push(value);
                index += 1;
            }
        }
    }
    ensure_backup_scope(&paths, include_config, "backup-policy-upsert")?;
    anyhow::ensure!(
        cron_expr.split_whitespace().count() == 5,
        "backup policy cron must have five fields"
    );
    let selection = VtyJobSelection::parse(&target_tokens)?;
    anyhow::ensure!(
        selection.confirmed,
        "backup-policy-upsert requires --confirmed"
    );
    validate_backup_policy_upsert_mode(
        schedule_id,
        retention_days,
        keep_last,
        rotation_generation.as_deref(),
        clear_rotation_generation,
    )?;
    Ok(VtyBackupPolicyUpsert {
        schedule_id,
        name,
        paths,
        include_config,
        follow_symlinks,
        skip_missing_paths,
        selection,
        cron_expr,
        enabled,
        catch_up_policy,
        catch_up_limit,
        retry_delay_secs,
        max_failures,
        retention_days,
        keep_last,
        rotation_generation,
        clear_rotation_generation,
    })
}

fn normalize_vty_cron_value(value: &str) -> String {
    value.replace([',', '_'], " ")
}

pub(crate) fn parse_vty_restore_plan(tokens: &[&str]) -> Result<VtyRestorePlanRequest> {
    let source_backup_request_id = tokens.first().context(
        "usage: restore-plan <source_backup_uuid> <target_client_id> [--confirmed] [--note <text>]",
    )?;
    let target_client_id = tokens.get(1).context(
        "usage: restore-plan <source_backup_uuid> <target_client_id> [--confirmed] [--note <text>]",
    )?;
    let mut request = VtyRestorePlanRequest {
        source_backup_request_id: Uuid::parse_str(source_backup_request_id)
            .context("invalid source backup request UUID")?,
        target_client_id: (*target_client_id).to_string(),
        confirmed: false,
        note: None,
    };
    let mut index = 2;
    while index < tokens.len() {
        match tokens[index] {
            "--confirmed" => {
                request.confirmed = true;
                index += 1;
            }
            "--path" | "--include-config" | "--destination-root" => {
                anyhow::bail!(
                    "{} was removed; restore scope and destination root are derived from records",
                    tokens[index]
                );
            }
            value if value.starts_with("--path=") || value.starts_with("--destination-root=") => {
                anyhow::bail!(
                    "restore scope and destination root flags were removed; select backup and target records"
                );
            }
            "--note" => {
                request.note = Some(
                    tokens
                        .get(index + 1)
                        .context("--note requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            value if value.starts_with("--note=") => {
                request.note = Some(value.trim_start_matches("--note=").to_string());
                index += 1;
            }
            other => anyhow::bail!("unknown restore-plan flag {other}"),
        }
    }
    Ok(request)
}

pub(crate) fn parse_vty_restore_run(tokens: &[&str]) -> Result<VtyRestoreRunRequest> {
    let source_backup_request_id = tokens
        .first()
        .context("usage: restore-run <source_backup_uuid> <target_client_id> --archive-transfer-session-id <uuid> [--max-timeout <secs>] [--dry-run] [--force-unprivileged] --confirmed")?;
    let target_client_id = tokens.get(1).context(
        "usage: restore-run <source_backup_uuid> <target_client_id> --archive-transfer-session-id <uuid> [--max-timeout <secs>] [--dry-run] [--force-unprivileged] --confirmed",
    )?;
    let mut request = VtyRestoreRunRequest {
        source_backup_request_id: Uuid::parse_str(source_backup_request_id)
            .context("invalid source backup request UUID")?,
        target_client_id: (*target_client_id).to_string(),
        archive_transfer_session_id: Uuid::nil(),
        max_timeout_secs: 60,
        confirmed: false,
        dry_run: false,
        force_unprivileged: false,
    };
    let mut index = 2;
    while index < tokens.len() {
        match tokens[index] {
            "--archive-transfer-session-id" => {
                request.archive_transfer_session_id = Uuid::parse_str(
                    tokens
                        .get(index + 1)
                        .context("--archive-transfer-session-id requires a value")?,
                )
                .context("invalid --archive-transfer-session-id")?;
                index += 2;
            }
            value if value.starts_with("--archive-transfer-session-id=") => {
                request.archive_transfer_session_id =
                    Uuid::parse_str(value.trim_start_matches("--archive-transfer-session-id="))
                        .context("invalid --archive-transfer-session-id")?;
                index += 1;
            }
            "--archive-path" | "--archive-size-bytes" | "--archive-sha256-hex" => {
                anyhow::bail!(
                    "{} was removed; use --archive-transfer-session-id",
                    tokens[index]
                );
            }
            value
                if value.starts_with("--archive-path=")
                    || value.starts_with("--archive-size-bytes=")
                    || value.starts_with("--archive-sha256-hex=") =>
            {
                anyhow::bail!(
                    "archive path/size/SHA flags were removed; use --archive-transfer-session-id"
                );
            }
            "--confirmed" => {
                request.confirmed = true;
                index += 1;
            }
            "--dry-run" => {
                request.dry_run = true;
                index += 1;
            }
            "--force-unprivileged" => {
                request.force_unprivileged = true;
                index += 1;
            }
            "--path" | "--include-config" | "--destination-root" => {
                anyhow::bail!(
                    "{} was removed; restore scope and destination root are derived from records",
                    tokens[index]
                );
            }
            value if value.starts_with("--path=") || value.starts_with("--destination-root=") => {
                anyhow::bail!(
                    "restore scope and destination root flags were removed; select backup and target records"
                );
            }
            "--max-timeout" => {
                request.max_timeout_secs = tokens
                    .get(index + 1)
                    .context("--max-timeout requires a value")?
                    .parse()
                    .context("invalid --max-timeout")?;
                index += 2;
            }
            value if value.starts_with("--max-timeout=") => {
                request.max_timeout_secs = value
                    .trim_start_matches("--max-timeout=")
                    .parse()
                    .context("invalid --max-timeout")?;
                index += 1;
            }
            other => anyhow::bail!("unknown restore-run flag {other}"),
        }
    }
    anyhow::ensure!(
        !request.archive_transfer_session_id.is_nil(),
        "restore-run requires --archive-transfer-session-id"
    );
    anyhow::ensure!(
        (1..=MAX_CONFIGURABLE_JOB_TIMEOUT_SECS).contains(&request.max_timeout_secs),
        "restore timeout out of range"
    );
    anyhow::ensure!(
        request.dry_run || request.confirmed,
        "restore-run requires --confirmed unless --dry-run is set"
    );
    Ok(request)
}

pub(crate) fn parse_vty_restore_rollback(tokens: &[&str]) -> Result<VtyRestoreRollbackRequest> {
    let restore_job_id = tokens
        .first()
        .context("usage: restore-rollback <restore_job_uuid> <target_client_id> [--max-timeout <secs>] [--force-unprivileged] --confirmed")?;
    let target_client_id = tokens.get(1).context(
        "usage: restore-rollback <restore_job_uuid> <target_client_id> [--max-timeout <secs>] [--force-unprivileged] --confirmed",
    )?;
    let mut request = VtyRestoreRollbackRequest {
        restore_job_id: Uuid::parse_str(restore_job_id).context("invalid restore job UUID")?,
        target_client_id: (*target_client_id).to_string(),
        max_timeout_secs: 60,
        confirmed: false,
        force_unprivileged: false,
    };
    let mut index = 2;
    while index < tokens.len() {
        match tokens[index] {
            "--confirmed" => {
                request.confirmed = true;
                index += 1;
            }
            "--force-unprivileged" => {
                request.force_unprivileged = true;
                index += 1;
            }
            "--max-timeout" => {
                request.max_timeout_secs = tokens
                    .get(index + 1)
                    .context("--max-timeout requires a value")?
                    .parse()
                    .context("invalid --max-timeout")?;
                index += 2;
            }
            value if value.starts_with("--max-timeout=") => {
                request.max_timeout_secs = value
                    .trim_start_matches("--max-timeout=")
                    .parse()
                    .context("invalid --max-timeout")?;
                index += 1;
            }
            other => anyhow::bail!("unknown restore-rollback flag {other}"),
        }
    }
    anyhow::ensure!(
        (1..=MAX_CONFIGURABLE_JOB_TIMEOUT_SECS).contains(&request.max_timeout_secs),
        "restore rollback timeout out of range"
    );
    anyhow::ensure!(request.confirmed, "restore-rollback requires --confirmed");
    Ok(request)
}

fn backup_missing_path_policy(skip_missing_paths: bool) -> BackupMissingPathPolicy {
    if skip_missing_paths {
        BackupMissingPathPolicy::Skip
    } else {
        BackupMissingPathPolicy::Fail
    }
}

pub(crate) fn submit_vty_backup_request(
    api_url: &str,
    token: Option<&str>,
    password: &str,
    salt_hex: &str,
    request: VtyBackupRequest,
) -> Result<String> {
    let operation = JobCommand::Backup {
        paths: request.paths.clone(),
        include_config: request.include_config,
        follow_symlinks: request.follow_symlinks,
        missing_path_policy: backup_missing_path_policy(request.skip_missing_paths),
    };
    let target_ids = vec![request.client_id.clone()];
    let selector_expression = selector_expression_from_targets(&target_ids, &[]);
    let privilege_assertion = build_backup_metadata_privilege(
        &target_ids,
        &operation,
        &selector_expression,
        password,
        salt_hex,
        300,
    )?;
    http_post_json(
        api_url,
        "/api/v1/backups",
        token,
        &serde_json::json!({
            "client_id": request.client_id,
            "paths": request.paths,
            "include_config": request.include_config,
            "follow_symlinks": request.follow_symlinks,
            "missing_path_policy": backup_missing_path_policy(request.skip_missing_paths),
            "confirmed": request.confirmed,
            "note": request.note,
            "privilege_assertion": privilege_assertion,
        }),
    )
}

pub(crate) fn submit_vty_backup_run(
    api_url: &str,
    token: Option<&str>,
    privilege_context: &VtyPrivilegeContext,
    request: VtyBackupRunRequest,
) -> Result<String> {
    let operation = JobCommand::Backup {
        paths: request.paths,
        include_config: request.include_config,
        follow_symlinks: request.follow_symlinks,
        missing_path_policy: backup_missing_path_policy(request.skip_missing_paths),
    };
    anyhow::ensure!(
        request.selection.confirmed,
        "backup-run requires --confirmed"
    );
    vty_submit_operation(
        api_url,
        token,
        privilege_context,
        "backup",
        &operation,
        request.selection,
        request.max_timeout_secs,
    )
}

pub(crate) fn submit_vty_backup_policy_upsert(
    api_url: &str,
    token: Option<&str>,
    request: VtyBackupPolicyUpsert,
) -> Result<String> {
    validate_backup_policy_upsert_mode(
        request.schedule_id,
        request.retention_days,
        request.keep_last,
        request.rotation_generation.as_deref(),
        request.clear_rotation_generation,
    )?;
    let selector_expression =
        selector_expression_from_targets(&request.selection.clients, &request.selection.tags);
    let target = backup_policy_upsert_target(request.schedule_id);
    let stored_snapshot = target
        .schedule_id
        .as_deref()
        .map(|schedule_id| saved_schedule_target_snapshot(api_url, token, schedule_id))
        .transpose()?;
    let target_client_ids = match stored_snapshot.as_ref() {
        Some(snapshot) if snapshot.selector_expression.trim() == selector_expression.trim() => {
            snapshot.target_client_ids.clone()
        }
        _ => resolve_schedule_target_ids(api_url, token, &selector_expression)?,
    };
    let operation = JobCommand::Backup {
        paths: request.paths.clone(),
        include_config: request.include_config,
        follow_symlinks: request.follow_symlinks,
        missing_path_policy: backup_missing_path_policy(request.skip_missing_paths),
    };
    let password = load_super_password("VPSMAN_SUPER_PASSWORD")?;
    let salt_hex = load_super_salt_hex(None)?;
    let privilege_assertion = build_privilege_for_schedule(
        SchedulePrivilegeRequest {
            action: target.action,
            schedule_id: target.schedule_id.as_deref(),
            definition_revision: stored_snapshot
                .as_ref()
                .map(|snapshot| snapshot.definition_revision),
            name: &request.name,
            payload: SchedulePrivilegePayload::Operation(&operation),
            command_type: "backup",
            selector_expression: &selector_expression,
            resolved_targets: &target_client_ids,
            trigger_kind: "cron",
            cron_expr: Some(&request.cron_expr),
            timezone: Some("UTC"),
            event_expression: None,
            enabled: request.enabled,
            catch_up_policy: Some(&request.catch_up_policy),
            catch_up_limit: Some(request.catch_up_limit),
            retry_delay_secs: Some(request.retry_delay_secs),
            max_failures: request.max_failures,
            deferred_until: None,
            deleted: false,
        },
        &password,
        &salt_hex,
        300,
    )?;
    let mut payload = serde_json::json!({
        "name": request.name,
        "paths": request.paths,
        "include_config": request.include_config,
        "follow_symlinks": request.follow_symlinks,
        "missing_path_policy": backup_missing_path_policy(request.skip_missing_paths),
        "selector_expression": selector_expression,
        "target_client_ids": target_client_ids,
        "cron_expr": request.cron_expr,
        "timezone": "UTC",
        "enabled": request.enabled,
        "catch_up_policy": request.catch_up_policy,
        "catch_up_limit": request.catch_up_limit,
        "retry_delay_secs": request.retry_delay_secs,
        "max_failures": request.max_failures,
        "retention_days": request.retention_days,
        "keep_last": request.keep_last,
        "rotation_generation": if request.clear_rotation_generation {
            None
        } else {
            request.rotation_generation
        },
        "confirmed": request.selection.confirmed,
        "privilege_assertion": privilege_assertion,
    });
    if let Some(snapshot) = stored_snapshot {
        payload["expected_definition_revision"] =
            serde_json::Value::from(snapshot.definition_revision);
        payload["expected_selector_expression"] =
            serde_json::Value::String(snapshot.selector_expression);
        payload["expected_target_client_ids"] = serde_json::to_value(snapshot.target_client_ids)?;
    }
    if target.schedule_id.is_some() {
        http_put_json(api_url, &target.path, token, &payload)
    } else {
        http_post_json(api_url, &target.path, token, &payload)
    }
}

pub(crate) fn submit_vty_restore_plan(
    api_url: &str,
    token: Option<&str>,
    password: &str,
    salt_hex: &str,
    request: VtyRestorePlanRequest,
) -> Result<String> {
    let scope = restore_scope_from_backup(
        api_url,
        token,
        request.source_backup_request_id,
        &request.target_client_id,
    )?;
    let operation = JobCommand::Restore {
        source_backup_request_id: request.source_backup_request_id,
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
    let target_ids = vec![request.target_client_id.clone()];
    let selector_expression = selector_expression_from_targets(&target_ids, &[]);
    let privilege_assertion = build_restore_plan_privilege(
        &target_ids,
        &operation,
        &selector_expression,
        password,
        salt_hex,
        300,
    )?;
    http_post_json(
        api_url,
        "/api/v1/restore-plans",
        token,
        &serde_json::json!({
            "source_backup_request_id": request.source_backup_request_id,
            "target_client_id": request.target_client_id,
            "paths": scope.paths,
            "include_config": scope.include_config,
            "destination_root": scope.destination_root,
            "confirmed": request.confirmed,
            "note": request.note,
            "privilege_assertion": privilege_assertion,
        }),
    )
}

pub(crate) fn submit_vty_restore_run(
    api_url: &str,
    token: Option<&str>,
    privilege_context: &VtyPrivilegeContext,
    request: VtyRestoreRunRequest,
) -> Result<String> {
    let scope = restore_scope_from_backup(
        api_url,
        token,
        request.source_backup_request_id,
        &request.target_client_id,
    )?;
    restore_run_with_credentials(
        api_url,
        token,
        RestoreRunWithCredentials {
            source_backup_request_id: request.source_backup_request_id,
            target_client_id: request.target_client_id,
            archive_transfer_session_id: request.archive_transfer_session_id,
            paths: scope.paths,
            include_config: scope.include_config,
            destination_root: scope.destination_root,
            password: &privilege_context.password,
            salt_hex: &privilege_context.salt_hex,
            privilege_ttl_secs: 300,
            max_timeout_secs: request.max_timeout_secs,
            confirmed: request.confirmed,
            dry_run: request.dry_run,
            force_unprivileged: request.force_unprivileged,
        },
    )
}

pub(crate) fn submit_vty_restore_rollback(
    api_url: &str,
    token: Option<&str>,
    privilege_context: &VtyPrivilegeContext,
    request: VtyRestoreRollbackRequest,
) -> Result<String> {
    let operation = restore_rollback_operation_from_api(
        api_url,
        token,
        request.restore_job_id,
        &request.target_client_id,
    )?;
    vty_submit_operation_with_force(
        api_url,
        token,
        privilege_context,
        "restore_rollback",
        &operation,
        VtyJobSelection {
            clients: vec![request.target_client_id],
            tags: Vec::new(),
            destructive: true,
            confirmed: request.confirmed,
        },
        request.max_timeout_secs,
        request.force_unprivileged,
    )
}

fn ensure_backup_scope(paths: &[String], include_config: bool, command: &str) -> Result<()> {
    anyhow::ensure!(
        include_config || !paths.is_empty(),
        "{command} needs --include-config or at least one --path"
    );
    for path in paths {
        anyhow::ensure!(
            path.starts_with('/'),
            "{} paths must be absolute",
            command.trim_end_matches("-plan")
        );
    }
    Ok(())
}
