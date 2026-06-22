use std::path::PathBuf;

use anyhow::{Context, Result};
use uuid::Uuid;
use vpsman_common::{
    validate_agent_config_shape, validate_incremental_config_patch_section, AgentConfig,
    JobCommand, HOT_CONFIG_APPLY_MODE_FULL_OVERRIDE, MAX_AGENT_HOT_CONFIG_BYTES,
    SOURCE_CONFIG_PATCH_APPLY_MODE_INCREMENTAL_PATCH,
};

use crate::commands_schedules::selector_expression_from_targets;
use crate::http::{http_get, http_post_json};
use crate::jobs::{resolve_target_ids, submit_privileged_operation, PrivilegedOperationRequest};
use crate::privilege::{build_privilege_for_job_command, load_super_password, load_super_salt_hex};
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

pub(crate) fn hot_config(
    api_url: &str,
    token: Option<&str>,
    config_file: PathBuf,
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
        "hot-config requires --confirmed because it applies a full agent config override"
    );
    let toml_document = std::fs::read_to_string(&config_file).with_context(|| {
        format!(
            "failed to read full config override {}",
            config_file.display()
        )
    })?;
    anyhow::ensure!(
        toml_document.len() <= MAX_AGENT_HOT_CONFIG_BYTES,
        "full config override exceeds {} bytes",
        MAX_AGENT_HOT_CONFIG_BYTES
    );
    let config: AgentConfig = toml::from_str(&toml_document).with_context(|| {
        format!(
            "failed to parse full config override {}",
            config_file.display()
        )
    })?;
    validate_agent_config_shape(&config)
        .map_err(|message| anyhow::anyhow!("invalid full config override: {message}"))?;
    let operation = JobCommand::HotConfig {
        apply_mode: HOT_CONFIG_APPLY_MODE_FULL_OVERRIDE.to_string(),
        toml: toml_document,
        preserve_redacted: None,
        base_config_sha256_hex: None,
    };
    println!(
        "{}",
        submit_privileged_operation(PrivilegedOperationRequest {
            api_url,
            token,
            operation: &operation,
            command_label: "hot_config",
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

pub(crate) fn config_patch(
    api_url: &str,
    token: Option<&str>,
    config_file: PathBuf,
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
        "config-patch requires --confirmed because it applies an incremental agent config patch"
    );
    let toml_document = std::fs::read_to_string(&config_file)
        .with_context(|| format!("failed to read config patch {}", config_file.display()))?;
    validate_incremental_config_patch(&toml_document)
        .with_context(|| format!("invalid config patch {}", config_file.display()))?;
    let operation = JobCommand::SourceConfigPatch {
        apply_mode: SOURCE_CONFIG_PATCH_APPLY_MODE_INCREMENTAL_PATCH.to_string(),
        toml: toml_document,
    };
    println!(
        "{}",
        submit_privileged_operation(PrivilegedOperationRequest {
            api_url,
            token,
            operation: &operation,
            command_label: "source_config_patch",
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

fn validate_incremental_config_patch(toml_document: &str) -> Result<()> {
    anyhow::ensure!(
        !toml_document.is_empty(),
        "incremental config patch is empty"
    );
    anyhow::ensure!(
        toml_document.len() <= MAX_AGENT_HOT_CONFIG_BYTES,
        "incremental config patch exceeds {} bytes",
        MAX_AGENT_HOT_CONFIG_BYTES
    );
    let value: toml::Value =
        toml::from_str(toml_document).context("incremental config patch is invalid TOML")?;
    let table = value
        .as_table()
        .context("incremental config patch must be a TOML table")?;
    anyhow::ensure!(
        !table.is_empty(),
        "incremental config patch has no sections"
    );
    for section in table.keys() {
        validate_incremental_config_patch_section(section)
            .map_err(|message| anyhow::anyhow!(message))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_update_input;

    #[test]
    fn validates_external_update_input() {
        let sha256_hex = "ab".repeat(32);

        validate_update_input("https://updates.example/vpsman-agent", &sha256_hex).unwrap();
        assert!(validate_update_input("http://updates.example/vpsman-agent", &sha256_hex).is_err());
        assert!(
            validate_update_input("https://updates.example/vpsman-agent", "not-a-hash").is_err()
        );
    }
}
