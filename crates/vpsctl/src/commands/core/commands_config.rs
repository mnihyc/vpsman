use std::path::PathBuf;

use anyhow::{Context, Result};
use uuid::Uuid;
use vpsman_common::JobCommand;

use crate::commands_inventory::{
    require_matching_reviewed_preview_hash, required_preview_hash, reviewed_preview_hash_arg,
};
use crate::commands_schedules::selector_expression_from_targets;
use crate::http::{http_get, http_post_json};
use crate::jobs::{resolve_target_ids, submit_privileged_operation, PrivilegedOperationRequest};
use crate::privilege::{
    build_privilege_for_db, build_privilege_for_job_command, load_super_password,
    load_super_salt_hex, DbPrivilegeRequest,
};
use crate::util::percent_encode_path_segment;

pub(crate) struct AgentUpdateOptions {
    pub(crate) artifact_url: String,
    pub(crate) sha256_hex: String,
    pub(crate) clients: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) password_env: String,
    pub(crate) super_salt_hex: Option<String>,
    pub(crate) privilege_ttl_secs: u64,
    pub(crate) max_timeout_secs: u64,
    pub(crate) confirmed: bool,
    pub(crate) force_unprivileged: bool,
}

pub(crate) struct AgentUpdateCheckOptions {
    pub(crate) version_url: Option<String>,
    pub(crate) activate: bool,
    pub(crate) restart_agent: bool,
    pub(crate) clients: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) password_env: String,
    pub(crate) super_salt_hex: Option<String>,
    pub(crate) privilege_ttl_secs: u64,
    pub(crate) max_timeout_secs: u64,
    pub(crate) confirmed: bool,
    pub(crate) force_unprivileged: bool,
}

pub(crate) struct AgentUpdateReleaseRecordOptions {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) channel: String,
    pub(crate) artifact_url: String,
    pub(crate) sha256_hex: String,
    pub(crate) rollback_artifact_url: Option<String>,
    pub(crate) rollback_sha256_hex: Option<String>,
    pub(crate) size_bytes: Option<i64>,
    pub(crate) rollback_size_bytes: Option<i64>,
    pub(crate) notes: Option<String>,
    pub(crate) confirmed: bool,
}

pub(crate) fn config_patch(
    api_url: &str,
    token: Option<&str>,
    config_file: PathBuf,
    clients: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    submitted_preview_hash: Option<String>,
    confirmed: bool,
) -> Result<()> {
    let reviewed_preview_hash =
        reviewed_preview_hash_arg(confirmed, submitted_preview_hash.as_deref(), "config-patch")?;
    let toml_document = std::fs::read_to_string(&config_file)
        .with_context(|| format!("failed to read config patch {}", config_file.display()))?;
    let selector_expression = selector_expression_from_targets(&clients, &tags);
    let preview_raw = http_post_json(
        api_url,
        "/api/v1/runtime-config/overrides/bulk/preview",
        token,
        &serde_json::json!({
            "selector_expression": &selector_expression,
            "target_client_ids": [],
            "patch": &toml_document,
            "reason": "CLI config patch",
        }),
    )?;
    let preview: serde_json::Value = serde_json::from_str(&preview_raw)
        .context("runtime config preview returned malformed JSON")?;
    let current_preview_hash = required_preview_hash(&preview, "config-patch")?;
    if !confirmed {
        println!("{preview_raw}");
        return Ok(());
    }
    let preview_hash = require_matching_reviewed_preview_hash(
        reviewed_preview_hash.as_deref(),
        &current_preview_hash,
        "config-patch",
    )?;
    let target_ids: Vec<String> = serde_json::from_value(
        preview
            .get("target_client_ids")
            .cloned()
            .context("runtime config preview response missing target_client_ids")?,
    )
    .context("runtime config preview returned invalid target_client_ids")?;
    let password = load_super_password(&password_env)?;
    let salt_hex = load_super_salt_hex(super_salt_hex.as_deref())?;
    let privilege_assertion = build_privilege_for_db(
        DbPrivilegeRequest {
            action: "runtime_config.override.bulk_apply",
            target: "runtime_config",
            selector_expression: Some(&selector_expression),
            resolved_targets: &target_ids,
            confirmed: true,
            payload_hash: Some(&preview_hash),
        },
        &password,
        &salt_hex,
        privilege_ttl_secs,
    )?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/runtime-config/overrides/bulk/apply",
            token,
            &serde_json::json!({
                "selector_expression": &selector_expression,
                "target_client_ids": &target_ids,
                "patch": &toml_document,
                "reason": "CLI config patch",
                "preview_hash": preview_hash,
                "confirmed": true,
                "privilege_assertion": privilege_assertion,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn agent_update(
    api_url: &str,
    token: Option<&str>,
    options: AgentUpdateOptions,
) -> Result<()> {
    anyhow::ensure!(
        options.confirmed,
        "agent-update requires --confirmed because it stages a replacement binary"
    );
    validate_update_input(&options.artifact_url, &options.sha256_hex)?;
    let operation = JobCommand::UpdateAgent {
        artifact_url: options.artifact_url,
        sha256_hex: options.sha256_hex.to_ascii_lowercase(),
    };
    let password = load_super_password(&options.password_env)?;
    let salt_hex = load_super_salt_hex(options.super_salt_hex.as_deref())?;
    let selector_expression = selector_expression_from_targets(&options.clients, &options.tags);
    let target_ids = resolve_target_ids(api_url, token, &options.clients, &options.tags)?;
    let privilege = build_privilege_for_job_command(
        &target_ids,
        &operation,
        "agent_update",
        &selector_expression,
        &password,
        &salt_hex,
        options.privilege_ttl_secs,
        options.max_timeout_secs,
        options.force_unprivileged,
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
                "command": "agent_update",
                "argv": [],
                "operation": operation,
                "selector_expression": selector_expression,
                "target_client_ids": target_ids,
                "privileged": true,
                "destructive": false,
                "confirmed": options.confirmed,
                "force_unprivileged": options.force_unprivileged,
                "max_timeout_secs": options.max_timeout_secs,
                "privilege_assertion": privilege.privilege_assertion,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn agent_update_check(
    api_url: &str,
    token: Option<&str>,
    options: AgentUpdateCheckOptions,
) -> Result<()> {
    anyhow::ensure!(
        options.confirmed,
        "agent-update-check requires --confirmed because it may stage and activate a replacement binary"
    );
    anyhow::ensure!(
        !options.restart_agent || options.activate,
        "--restart-agent requires --activate"
    );
    if let Some(version_url) = options.version_url.as_deref() {
        anyhow::ensure!(
            version_url.starts_with("https://")
                || version_url.starts_with("http://localhost")
                || version_url.starts_with("http://127.0.0.1")
                || version_url.starts_with("file://"),
            "version URL must use https://, localhost http://, or file://"
        );
    }
    let operation = JobCommand::AgentUpdateCheck {
        version_url: options.version_url,
        activate: options.activate,
        restart_agent: options.restart_agent,
    };
    let password = load_super_password(&options.password_env)?;
    let salt_hex = load_super_salt_hex(options.super_salt_hex.as_deref())?;
    let selector_expression = selector_expression_from_targets(&options.clients, &options.tags);
    let target_ids = resolve_target_ids(api_url, token, &options.clients, &options.tags)?;
    let privilege = build_privilege_for_job_command(
        &target_ids,
        &operation,
        "agent_update_check",
        &selector_expression,
        &password,
        &salt_hex,
        options.privilege_ttl_secs,
        options.max_timeout_secs,
        options.force_unprivileged,
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
                "command": "agent_update_check",
                "argv": [],
                "operation": operation,
                "selector_expression": selector_expression,
                "target_client_ids": target_ids,
                "privileged": true,
                "destructive": false,
                "confirmed": options.confirmed,
                "force_unprivileged": options.force_unprivileged,
                "max_timeout_secs": options.max_timeout_secs,
                "privilege_assertion": privilege.privilege_assertion,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn agent_update_release_record(
    api_url: &str,
    token: Option<&str>,
    options: AgentUpdateReleaseRecordOptions,
) -> Result<()> {
    anyhow::ensure!(
        options.confirmed,
        "agent-update-release-record requires --confirmed because it records update metadata"
    );
    let sha256_hex = validate_sha256_arg(&options.sha256_hex, "--sha256-hex")?;
    validate_update_input(&options.artifact_url, &sha256_hex)?;
    let rollback = match (
        options.rollback_artifact_url.as_deref(),
        options.rollback_sha256_hex.as_deref(),
    ) {
        (Some(url), Some(sha256)) => {
            let sha256 = validate_sha256_arg(sha256, "--rollback-sha256-hex")?;
            validate_update_input(url, &sha256)?;
            Some((url.to_string(), sha256))
        }
        (None, None) => None,
        _ => anyhow::bail!(
            "--rollback-artifact-url and --rollback-sha256-hex must be provided together"
        ),
    };
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/agent-update-releases",
            token,
            &serde_json::json!({
                "name": options.name,
                "version": options.version,
                "channel": options.channel,
                "artifact_sha256_hex": sha256_hex,
                "artifact_url": options.artifact_url,
                "rollback_artifact_sha256_hex": rollback.as_ref().map(|(_, sha256)| sha256.clone()),
                "rollback_artifact_url": rollback.as_ref().map(|(url, _)| url.clone()),
                "rollback_size_bytes": options.rollback_size_bytes,
                "size_bytes": options.size_bytes,
                "notes": options.notes,
                "confirmed": options.confirmed,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn agent_update_releases(api_url: &str, token: Option<&str>, limit: u16) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!(
                "/api/v1/agent-update-releases?limit={}",
                limit.clamp(1, 200)
            ),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn agent_update_release_latest(
    api_url: &str,
    token: Option<&str>,
    name: String,
    channel: String,
) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!(
                "/api/v1/agent-update-releases/latest?name={}&channel={}",
                percent_encode_path_segment(&name),
                percent_encode_path_segment(&channel)
            ),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn agent_update_activate(
    api_url: &str,
    token: Option<&str>,
    staged_sha256_hex: String,
    clients: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    max_timeout_secs: u64,
    restart_agent: bool,
    confirmed: bool,
    force_unprivileged: bool,
) -> Result<()> {
    anyhow::ensure!(
        confirmed,
        "agent-update-activate requires --confirmed because it replaces the active agent binary"
    );
    let staged_sha256_hex = validate_sha256_arg(&staged_sha256_hex, "--staged-sha256-hex")?;
    let operation = JobCommand::AgentUpdateActivate {
        staged_sha256_hex,
        restart_agent,
    };
    println!(
        "{}",
        submit_privileged_operation(PrivilegedOperationRequest {
            api_url,
            token,
            operation: &operation,
            command_label: "agent_update_activate",
            clients: &clients,
            tags: &tags,
            password_env: &password_env,
            super_salt_hex: super_salt_hex.as_deref(),
            privilege_ttl_secs,
            max_timeout_secs,
            confirmed,
            force_unprivileged,
        })?
    );
    Ok(())
}

pub(crate) fn agent_update_rollback(
    api_url: &str,
    token: Option<&str>,
    rollback_sha256_hex: Option<String>,
    clients: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    max_timeout_secs: u64,
    confirmed: bool,
    force_unprivileged: bool,
) -> Result<()> {
    anyhow::ensure!(
        confirmed,
        "agent-update-rollback requires --confirmed because it replaces the active agent binary"
    );
    let rollback_sha256_hex = rollback_sha256_hex
        .as_deref()
        .map(|value| validate_sha256_arg(value, "--rollback-sha256-hex"))
        .transpose()?;
    let operation = JobCommand::AgentUpdateRollback {
        rollback_sha256_hex,
    };
    println!(
        "{}",
        submit_privileged_operation(PrivilegedOperationRequest {
            api_url,
            token,
            operation: &operation,
            command_label: "agent_update_rollback",
            clients: &clients,
            tags: &tags,
            password_env: &password_env,
            super_salt_hex: super_salt_hex.as_deref(),
            privilege_ttl_secs,
            max_timeout_secs,
            confirmed,
            force_unprivileged,
        })?
    );
    Ok(())
}

fn validate_sha256_arg(value: &str, label: &str) -> Result<String> {
    let value = value.trim().to_ascii_lowercase();
    anyhow::ensure!(
        value.len() == 64 && value.as_bytes().iter().all(u8::is_ascii_hexdigit),
        "agent update {label} must be 64 hex characters"
    );
    Ok(value)
}

pub(crate) fn validate_update_input(artifact_url: &str, sha256_hex: &str) -> Result<()> {
    anyhow::ensure!(
        artifact_url.starts_with("https://"),
        "agent update artifact URL must use https://"
    );
    anyhow::ensure!(
        sha256_hex.len() == 64 && sha256_hex.as_bytes().iter().all(u8::is_ascii_hexdigit),
        "agent update --sha256-hex must be 64 hex characters"
    );
    Ok(())
}

#[cfg(test)]
#[path = "tests_commands_config.rs"]
mod tests;
