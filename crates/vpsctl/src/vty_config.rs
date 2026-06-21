use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;
use uuid::Uuid;
use vpsman_common::{
    validate_agent_config_shape, validate_incremental_config_patch_section, AgentConfig,
    JobCommand, DATA_SOURCE_CONFIG_APPLY_MODE_INCREMENTAL_PATCH,
    HOT_CONFIG_APPLY_MODE_FULL_OVERRIDE, MAX_AGENT_HOT_CONFIG_BYTES,
    MAX_CONFIGURABLE_COMMAND_TIMEOUT_SECS,
};

use crate::{
    commands_config::validate_update_input,
    commands_schedules::selector_expression_from_targets,
    http::{http_get, http_post_json},
    privilege::build_privilege_for_job_command,
    util::percent_encode_query_value,
    vty_jobs::VtyJobSelection,
};

#[derive(Debug)]
pub(crate) struct VtyHotConfigRequest {
    config_file: PathBuf,
    selection: VtyJobSelection,
    timeout_secs: u64,
    privilege_ttl_secs: u64,
    force_unprivileged: bool,
}

#[derive(Debug)]
pub(crate) struct VtyDataSourceHotConfigApplyRequest {
    client_id: String,
    timeout_secs: u64,
    privilege_ttl_secs: u64,
    confirmed: bool,
    force_unprivileged: bool,
}

#[derive(Debug)]
pub(crate) struct VtyAgentUpdateRequest {
    artifact_url: String,
    sha256_hex: String,
    selection: VtyJobSelection,
    timeout_secs: u64,
    privilege_ttl_secs: u64,
    force_unprivileged: bool,
}

#[derive(Debug)]
pub(crate) struct VtyAgentUpdateCheckRequest {
    version_url: Option<String>,
    activate: bool,
    restart_agent: bool,
    selection: VtyJobSelection,
    timeout_secs: u64,
    privilege_ttl_secs: u64,
    force_unprivileged: bool,
}

#[derive(Debug)]
pub(crate) struct VtyAgentUpdateActivateRequest {
    staged_sha256_hex: String,
    restart_agent: bool,
    selection: VtyJobSelection,
    timeout_secs: u64,
    privilege_ttl_secs: u64,
    force_unprivileged: bool,
}

#[derive(Debug)]
pub(crate) struct VtyAgentUpdateRollbackRequest {
    rollback_sha256_hex: Option<String>,
    selection: VtyJobSelection,
    timeout_secs: u64,
    privilege_ttl_secs: u64,
    force_unprivileged: bool,
}

#[derive(Debug, Deserialize)]
struct VtyBulkResolveResponse {
    targets: Vec<VtyTarget>,
}

#[derive(Debug, Deserialize)]
struct VtyTarget {
    id: String,
}

pub(crate) fn parse_vty_hot_config(tokens: &[&str]) -> Result<VtyHotConfigRequest> {
    let mut config_file = None;
    let mut timeout_secs = 30_u64;
    let mut privilege_ttl_secs = 300_u64;
    let mut force_unprivileged = false;
    let mut target_tokens = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--config-file" => {
                config_file = Some(PathBuf::from(
                    tokens
                        .get(index + 1)
                        .context("--config-file requires a path")?,
                ));
                index += 2;
            }
            "--timeout" => {
                timeout_secs = tokens
                    .get(index + 1)
                    .context("--timeout requires a value")?
                    .parse()
                    .context("--timeout must be an integer")?;
                index += 2;
            }
            "--privilege-ttl" => {
                privilege_ttl_secs = tokens
                    .get(index + 1)
                    .context("--privilege-ttl requires a value")?
                    .parse()
                    .context("--privilege-ttl must be an integer")?;
                index += 2;
            }
            "--force-unprivileged" => {
                force_unprivileged = true;
                index += 1;
            }
            value if value.starts_with("--") && value != "--confirmed" => {
                anyhow::bail!("unknown hot-config flag {value}");
            }
            value => {
                target_tokens.push(value);
                index += 1;
            }
        }
    }
    anyhow::ensure!(
        (1..=MAX_CONFIGURABLE_COMMAND_TIMEOUT_SECS).contains(&timeout_secs),
        "hot-config --timeout must be between 1 and {MAX_CONFIGURABLE_COMMAND_TIMEOUT_SECS}"
    );
    anyhow::ensure!(
        (15..=300).contains(&privilege_ttl_secs),
        "hot-config --privilege-ttl must be between 15 and 300"
    );
    let selection = VtyJobSelection::parse(&target_tokens)?;
    anyhow::ensure!(
        selection.confirmed,
        "hot-config requires --confirmed because it applies a full agent config override"
    );
    Ok(VtyHotConfigRequest {
        config_file: config_file.context("hot-config requires --config-file <path>")?,
        selection,
        timeout_secs,
        privilege_ttl_secs,
        force_unprivileged,
    })
}

pub(crate) fn parse_vty_data_source_hot_config_apply(
    tokens: &[&str],
) -> Result<VtyDataSourceHotConfigApplyRequest> {
    let mut client_id = None;
    let mut timeout_secs = 30_u64;
    let mut privilege_ttl_secs = 300_u64;
    let mut confirmed = false;
    let mut force_unprivileged = false;
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--client-id" => {
                client_id = Some(
                    tokens
                        .get(index + 1)
                        .context("--client-id requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--timeout" => {
                timeout_secs = tokens
                    .get(index + 1)
                    .context("--timeout requires a value")?
                    .parse()
                    .context("--timeout must be an integer")?;
                index += 2;
            }
            "--privilege-ttl" => {
                privilege_ttl_secs = tokens
                    .get(index + 1)
                    .context("--privilege-ttl requires a value")?
                    .parse()
                    .context("--privilege-ttl must be an integer")?;
                index += 2;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            "--force-unprivileged" => {
                force_unprivileged = true;
                index += 1;
            }
            value => {
                anyhow::bail!("unknown data-source-hot-config-apply flag {value}");
            }
        }
    }
    validate_config_dispatch_bounds(
        timeout_secs,
        privilege_ttl_secs,
        "data-source-hot-config-apply",
    )?;
    let client_id = client_id.context("data-source-hot-config-apply requires --client-id <id>")?;
    anyhow::ensure!(
        !client_id.is_empty() && client_id.len() <= 128,
        "--client-id must be between 1 and 128 bytes"
    );
    anyhow::ensure!(
        confirmed,
        "data-source-hot-config-apply requires --confirmed because it applies an incremental config patch"
    );
    Ok(VtyDataSourceHotConfigApplyRequest {
        client_id,
        timeout_secs,
        privilege_ttl_secs,
        confirmed,
        force_unprivileged,
    })
}

pub(crate) fn parse_vty_agent_update(tokens: &[&str]) -> Result<VtyAgentUpdateRequest> {
    let mut artifact_url = None;
    let mut sha256_hex = None;
    let mut timeout_secs = 300_u64;
    let mut privilege_ttl_secs = 300_u64;
    let mut force_unprivileged = false;
    let mut target_tokens = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--artifact-url" => {
                artifact_url = Some(
                    tokens
                        .get(index + 1)
                        .context("--artifact-url requires a URL")?
                        .to_string(),
                );
                index += 2;
            }
            "--sha256-hex" => {
                sha256_hex = Some(
                    tokens
                        .get(index + 1)
                        .context("--sha256-hex requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--timeout" => {
                timeout_secs = tokens
                    .get(index + 1)
                    .context("--timeout requires a value")?
                    .parse()
                    .context("--timeout must be an integer")?;
                index += 2;
            }
            "--privilege-ttl" => {
                privilege_ttl_secs = tokens
                    .get(index + 1)
                    .context("--privilege-ttl requires a value")?
                    .parse()
                    .context("--privilege-ttl must be an integer")?;
                index += 2;
            }
            "--force-unprivileged" => {
                force_unprivileged = true;
                index += 1;
            }
            value if value.starts_with("--") && value != "--confirmed" => {
                anyhow::bail!("unknown agent-update flag {value}");
            }
            value => {
                target_tokens.push(value);
                index += 1;
            }
        }
    }
    anyhow::ensure!(
        (1..=MAX_CONFIGURABLE_COMMAND_TIMEOUT_SECS).contains(&timeout_secs),
        "agent-update --timeout must be between 1 and {MAX_CONFIGURABLE_COMMAND_TIMEOUT_SECS}"
    );
    anyhow::ensure!(
        (15..=300).contains(&privilege_ttl_secs),
        "agent-update --privilege-ttl must be between 15 and 300"
    );
    let artifact_url = artifact_url.context("agent-update requires --artifact-url <https-url>")?;
    let sha256_hex = sha256_hex.context("agent-update requires --sha256-hex <sha256>")?;
    validate_update_input(&artifact_url, &sha256_hex)?;
    let selection = VtyJobSelection::parse(&target_tokens)?;
    anyhow::ensure!(
        selection.confirmed,
        "agent-update requires --confirmed because it stages a replacement binary"
    );
    Ok(VtyAgentUpdateRequest {
        artifact_url,
        sha256_hex: sha256_hex.to_ascii_lowercase(),
        selection,
        timeout_secs,
        privilege_ttl_secs,
        force_unprivileged,
    })
}

pub(crate) fn parse_vty_agent_update_check(tokens: &[&str]) -> Result<VtyAgentUpdateCheckRequest> {
    let mut version_url = None;
    let mut activate = true;
    let mut restart_agent = true;
    let mut timeout_secs = 300_u64;
    let mut privilege_ttl_secs = 300_u64;
    let mut force_unprivileged = false;
    let mut target_tokens = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--version-url" => {
                version_url = Some(
                    tokens
                        .get(index + 1)
                        .context("--version-url requires a URL")?
                        .to_string(),
                );
                index += 2;
            }
            "--activate" => {
                activate = true;
                index += 1;
            }
            "--no-activate" => {
                activate = false;
                restart_agent = false;
                index += 1;
            }
            "--restart-agent" => {
                restart_agent = true;
                index += 1;
            }
            "--no-restart-agent" => {
                restart_agent = false;
                index += 1;
            }
            "--timeout" => {
                timeout_secs = tokens
                    .get(index + 1)
                    .context("--timeout requires a value")?
                    .parse()
                    .context("--timeout must be an integer")?;
                index += 2;
            }
            "--privilege-ttl" => {
                privilege_ttl_secs = tokens
                    .get(index + 1)
                    .context("--privilege-ttl requires a value")?
                    .parse()
                    .context("--privilege-ttl must be an integer")?;
                index += 2;
            }
            "--force-unprivileged" => {
                force_unprivileged = true;
                index += 1;
            }
            value if value.starts_with("--") && value != "--confirmed" => {
                anyhow::bail!("unknown agent-update-check flag {value}");
            }
            value => {
                target_tokens.push(value);
                index += 1;
            }
        }
    }
    validate_config_dispatch_bounds(timeout_secs, privilege_ttl_secs, "agent-update-check")?;
    if let Some(version_url) = version_url.as_deref() {
        validate_update_check_version_url(version_url)?;
    }
    let selection = VtyJobSelection::parse(&target_tokens)?;
    anyhow::ensure!(
        selection.confirmed,
        "agent-update-check requires --confirmed because it may stage and activate a replacement binary"
    );
    Ok(VtyAgentUpdateCheckRequest {
        version_url,
        activate,
        restart_agent,
        selection,
        timeout_secs,
        privilege_ttl_secs,
        force_unprivileged,
    })
}

pub(crate) fn parse_vty_agent_update_activate(
    tokens: &[&str],
) -> Result<VtyAgentUpdateActivateRequest> {
    let mut staged_sha256_hex = None;
    let mut timeout_secs = 60_u64;
    let mut privilege_ttl_secs = 300_u64;
    let mut restart_agent = false;
    let mut force_unprivileged = false;
    let mut target_tokens = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--staged-sha256-hex" => {
                staged_sha256_hex = Some(
                    tokens
                        .get(index + 1)
                        .context("--staged-sha256-hex requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--timeout" => {
                timeout_secs = tokens
                    .get(index + 1)
                    .context("--timeout requires a value")?
                    .parse()
                    .context("--timeout must be an integer")?;
                index += 2;
            }
            "--privilege-ttl" => {
                privilege_ttl_secs = tokens
                    .get(index + 1)
                    .context("--privilege-ttl requires a value")?
                    .parse()
                    .context("--privilege-ttl must be an integer")?;
                index += 2;
            }
            "--restart-agent" => {
                restart_agent = true;
                index += 1;
            }
            "--force-unprivileged" => {
                force_unprivileged = true;
                index += 1;
            }
            value if value.starts_with("--") && value != "--confirmed" => {
                anyhow::bail!("unknown agent-update-activate flag {value}");
            }
            value => {
                target_tokens.push(value);
                index += 1;
            }
        }
    }
    validate_config_dispatch_bounds(timeout_secs, privilege_ttl_secs, "agent-update-activate")?;
    let staged_sha256_hex =
        staged_sha256_hex.context("agent-update-activate requires --staged-sha256-hex <sha256>")?;
    let staged_sha256_hex = validate_sha256(&staged_sha256_hex, "--staged-sha256-hex")?;
    let selection = VtyJobSelection::parse(&target_tokens)?;
    anyhow::ensure!(
        selection.confirmed,
        "agent-update-activate requires --confirmed because it replaces the active agent binary"
    );
    Ok(VtyAgentUpdateActivateRequest {
        staged_sha256_hex,
        restart_agent,
        selection,
        timeout_secs,
        privilege_ttl_secs,
        force_unprivileged,
    })
}

pub(crate) fn parse_vty_agent_update_rollback(
    tokens: &[&str],
) -> Result<VtyAgentUpdateRollbackRequest> {
    let mut rollback_sha256_hex = None;
    let mut timeout_secs = 60_u64;
    let mut privilege_ttl_secs = 300_u64;
    let mut force_unprivileged = false;
    let mut target_tokens = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--rollback-sha256-hex" => {
                rollback_sha256_hex = Some(
                    tokens
                        .get(index + 1)
                        .context("--rollback-sha256-hex requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--timeout" => {
                timeout_secs = tokens
                    .get(index + 1)
                    .context("--timeout requires a value")?
                    .parse()
                    .context("--timeout must be an integer")?;
                index += 2;
            }
            "--privilege-ttl" => {
                privilege_ttl_secs = tokens
                    .get(index + 1)
                    .context("--privilege-ttl requires a value")?
                    .parse()
                    .context("--privilege-ttl must be an integer")?;
                index += 2;
            }
            "--force-unprivileged" => {
                force_unprivileged = true;
                index += 1;
            }
            value if value.starts_with("--") && value != "--confirmed" => {
                anyhow::bail!("unknown agent-update-rollback flag {value}");
            }
            value => {
                target_tokens.push(value);
                index += 1;
            }
        }
    }
    validate_config_dispatch_bounds(timeout_secs, privilege_ttl_secs, "agent-update-rollback")?;
    let rollback_sha256_hex = rollback_sha256_hex
        .as_deref()
        .map(|value| validate_sha256(value, "--rollback-sha256-hex"))
        .transpose()?;
    let selection = VtyJobSelection::parse(&target_tokens)?;
    anyhow::ensure!(
        selection.confirmed,
        "agent-update-rollback requires --confirmed because it replaces the active agent binary"
    );
    Ok(VtyAgentUpdateRollbackRequest {
        rollback_sha256_hex,
        selection,
        timeout_secs,
        privilege_ttl_secs,
        force_unprivileged,
    })
}

pub(crate) fn submit_vty_hot_config(
    api_url: &str,
    token: Option<&str>,
    password: &str,
    salt_hex: &str,
    request: VtyHotConfigRequest,
) -> Result<String> {
    let toml_document = std::fs::read_to_string(&request.config_file).with_context(|| {
        format!(
            "failed to read full config override {}",
            request.config_file.display()
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
            request.config_file.display()
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

    submit_vty_config_operation(
        api_url,
        token,
        password,
        salt_hex,
        "hot_config",
        operation,
        request.selection,
        request.timeout_secs,
        request.privilege_ttl_secs,
        request.force_unprivileged,
    )
}

pub(crate) fn submit_vty_data_source_hot_config_apply(
    api_url: &str,
    token: Option<&str>,
    password: &str,
    salt_hex: &str,
    request: VtyDataSourceHotConfigApplyRequest,
) -> Result<String> {
    #[derive(Deserialize)]
    struct RenderedPatch {
        toml: String,
    }

    let body = http_get(
        api_url,
        &format!(
            "/api/v1/data-source-hot-config?client_id={}",
            percent_encode_query_value(&request.client_id)
        ),
        token,
    )?;
    let rendered: RenderedPatch =
        serde_json::from_str(&body).context("failed to parse rendered data-source patch")?;
    validate_data_source_config_patch(&rendered.toml)?;
    let operation = JobCommand::DataSourceConfigPatch {
        apply_mode: DATA_SOURCE_CONFIG_APPLY_MODE_INCREMENTAL_PATCH.to_string(),
        toml: rendered.toml,
    };
    let selection = VtyJobSelection {
        clients: vec![request.client_id],
        tags: Vec::new(),
        destructive: false,
        confirmed: request.confirmed,
    };
    submit_vty_config_operation(
        api_url,
        token,
        password,
        salt_hex,
        "data_source_config_patch",
        operation,
        selection,
        request.timeout_secs,
        request.privilege_ttl_secs,
        request.force_unprivileged,
    )
}

pub(crate) fn submit_vty_agent_update(
    api_url: &str,
    token: Option<&str>,
    password: &str,
    salt_hex: &str,
    request: VtyAgentUpdateRequest,
) -> Result<String> {
    let operation = JobCommand::UpdateAgent {
        artifact_url: request.artifact_url,
        sha256_hex: request.sha256_hex,
    };
    submit_vty_config_operation(
        api_url,
        token,
        password,
        salt_hex,
        "agent_update",
        operation,
        request.selection,
        request.timeout_secs,
        request.privilege_ttl_secs,
        request.force_unprivileged,
    )
}

pub(crate) fn submit_vty_agent_update_check(
    api_url: &str,
    token: Option<&str>,
    password: &str,
    salt_hex: &str,
    request: VtyAgentUpdateCheckRequest,
) -> Result<String> {
    submit_vty_config_operation(
        api_url,
        token,
        password,
        salt_hex,
        "agent_update_check",
        JobCommand::AgentUpdateCheck {
            version_url: request.version_url,
            activate: request.activate,
            restart_agent: request.restart_agent,
        },
        request.selection,
        request.timeout_secs,
        request.privilege_ttl_secs,
        request.force_unprivileged,
    )
}

pub(crate) fn submit_vty_agent_update_activate(
    api_url: &str,
    token: Option<&str>,
    password: &str,
    salt_hex: &str,
    request: VtyAgentUpdateActivateRequest,
) -> Result<String> {
    submit_vty_config_operation(
        api_url,
        token,
        password,
        salt_hex,
        "agent_update_activate",
        JobCommand::AgentUpdateActivate {
            staged_sha256_hex: request.staged_sha256_hex,
            restart_agent: request.restart_agent,
        },
        request.selection,
        request.timeout_secs,
        request.privilege_ttl_secs,
        request.force_unprivileged,
    )
}

pub(crate) fn submit_vty_agent_update_rollback(
    api_url: &str,
    token: Option<&str>,
    password: &str,
    salt_hex: &str,
    request: VtyAgentUpdateRollbackRequest,
) -> Result<String> {
    submit_vty_config_operation(
        api_url,
        token,
        password,
        salt_hex,
        "agent_update_rollback",
        JobCommand::AgentUpdateRollback {
            rollback_sha256_hex: request.rollback_sha256_hex,
        },
        request.selection,
        request.timeout_secs,
        request.privilege_ttl_secs,
        request.force_unprivileged,
    )
}

fn validate_config_dispatch_bounds(
    timeout_secs: u64,
    privilege_ttl_secs: u64,
    command: &str,
) -> Result<()> {
    anyhow::ensure!(
        (1..=MAX_CONFIGURABLE_COMMAND_TIMEOUT_SECS).contains(&timeout_secs),
        "{command} --timeout must be between 1 and {MAX_CONFIGURABLE_COMMAND_TIMEOUT_SECS}"
    );
    anyhow::ensure!(
        (15..=300).contains(&privilege_ttl_secs),
        "{command} --privilege-ttl must be between 15 and 300"
    );
    Ok(())
}

fn validate_update_check_version_url(version_url: &str) -> Result<()> {
    anyhow::ensure!(
        version_url.starts_with("https://")
            || version_url.starts_with("http://localhost")
            || version_url.starts_with("http://127.0.0.1")
            || version_url.starts_with("file://"),
        "agent-update-check --version-url must use https://, localhost http://, or file://"
    );
    Ok(())
}

fn validate_sha256(value: &str, label: &str) -> Result<String> {
    let value = value.trim().to_ascii_lowercase();
    anyhow::ensure!(
        value.len() == 64 && value.as_bytes().iter().all(u8::is_ascii_hexdigit),
        "{label} must be 64 hex characters"
    );
    Ok(value)
}

fn validate_data_source_config_patch(toml_document: &str) -> Result<()> {
    anyhow::ensure!(
        !toml_document.is_empty(),
        "rendered data-source config patch is empty"
    );
    anyhow::ensure!(
        toml_document.len() <= MAX_AGENT_HOT_CONFIG_BYTES,
        "rendered data-source config patch exceeds {} bytes",
        MAX_AGENT_HOT_CONFIG_BYTES
    );
    let value: toml::Value = toml::from_str(toml_document)
        .context("rendered data-source config patch is invalid TOML")?;
    let table = value
        .as_table()
        .context("rendered data-source config patch must be a TOML table")?;
    anyhow::ensure!(
        !table.is_empty(),
        "rendered data-source config patch has no sections"
    );
    for section in table.keys() {
        validate_incremental_config_patch_section(section)
            .map_err(|message| anyhow::anyhow!(message))?;
    }
    Ok(())
}

fn submit_vty_config_operation(
    api_url: &str,
    token: Option<&str>,
    password: &str,
    salt_hex: &str,
    command_label: &str,
    operation: JobCommand,
    selection: VtyJobSelection,
    timeout_secs: u64,
    privilege_ttl_secs: u64,
    force_unprivileged: bool,
) -> Result<String> {
    let resolved = http_post_json(
        api_url,
        "/api/v1/bulk/resolve",
        token,
        &serde_json::json!({
            "selector_expression": selector_expression_from_targets(&selection.clients, &selection.tags),
        }),
    )?;
    let resolved: VtyBulkResolveResponse =
        serde_json::from_str(&resolved).context("failed to parse bulk target response")?;
    let client_ids = resolved
        .targets
        .into_iter()
        .map(|target| target.id)
        .collect::<Vec<_>>();
    anyhow::ensure!(
        !client_ids.is_empty(),
        "{command_label} resolved no targets; provide at least one matching target"
    );
    let selector_expression = selector_expression_from_targets(&selection.clients, &selection.tags);
    let privilege = build_privilege_for_job_command(
        &client_ids,
        &operation,
        command_label,
        &selector_expression,
        password,
        salt_hex,
        privilege_ttl_secs,
        timeout_secs,
        force_unprivileged,
        true,
    )?;

    http_post_json(
        api_url,
        "/api/v1/jobs",
        token,
        &serde_json::json!({
            "job_id": Uuid::new_v4(),
            "command": command_label,
            "argv": [],
            "operation": operation,
            "selector_expression": selector_expression,
            "target_client_ids": client_ids,
            "privileged": true,
            "destructive": false,
            "confirmed": selection.confirmed,
            "force_unprivileged": force_unprivileged,
            "timeout_secs": timeout_secs,
            "privilege_assertion": privilege.privilege_assertion,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        parse_vty_agent_update, parse_vty_agent_update_activate, parse_vty_agent_update_check,
        parse_vty_agent_update_rollback, parse_vty_data_source_hot_config_apply,
        parse_vty_hot_config,
    };

    #[test]
    fn parses_hot_config_request() {
        let request = parse_vty_hot_config(&[
            "--config-file",
            "./agent.toml",
            "id:edge-a",
            "tag:bgp",
            "--timeout",
            "45",
            "--privilege-ttl",
            "120",
            "--force-unprivileged",
            "--confirmed",
        ])
        .unwrap();

        assert_eq!(
            request.config_file,
            std::path::PathBuf::from("./agent.toml")
        );
        assert!(request.selection.clients.is_empty());
        assert_eq!(request.selection.tags, vec!["bgp", "id:edge-a"]);
        assert_eq!(request.timeout_secs, 45);
        assert_eq!(request.privilege_ttl_secs, 120);
        assert!(request.force_unprivileged);
        assert!(request.selection.confirmed);
    }

    #[test]
    fn rejects_unconfirmed_hot_config() {
        assert!(parse_vty_hot_config(&["--config-file", "./agent.toml", "tag:bgp"]).is_err());
    }

    #[test]
    fn parses_data_source_hot_config_apply_request() {
        let request = parse_vty_data_source_hot_config_apply(&[
            "--client-id",
            "edge-a",
            "--timeout",
            "45",
            "--privilege-ttl",
            "120",
            "--force-unprivileged",
            "--confirmed",
        ])
        .unwrap();

        assert_eq!(request.client_id, "edge-a");
        assert_eq!(request.timeout_secs, 45);
        assert_eq!(request.privilege_ttl_secs, 120);
        assert!(request.force_unprivileged);
        assert!(request.confirmed);
    }

    #[test]
    fn rejects_unconfirmed_data_source_hot_config_apply() {
        assert!(parse_vty_data_source_hot_config_apply(&["--client-id", "edge-a"]).is_err());
    }

    #[test]
    fn parses_agent_update_request() {
        let sha256_hex = "ab".repeat(32);
        let request = parse_vty_agent_update(&[
            "--artifact-url",
            "https://updates.example/vpsman-agent",
            "--sha256-hex",
            &sha256_hex,
            "id:edge-a",
            "--timeout",
            "300",
            "--privilege-ttl",
            "120",
            "--force-unprivileged",
            "--confirmed",
        ])
        .unwrap();

        assert_eq!(request.artifact_url, "https://updates.example/vpsman-agent");
        assert_eq!(request.sha256_hex, "ab".repeat(32));
        assert!(request.selection.clients.is_empty());
        assert_eq!(request.selection.tags, vec!["id:edge-a"]);
        assert_eq!(request.timeout_secs, 300);
        assert_eq!(request.privilege_ttl_secs, 120);
        assert!(request.force_unprivileged);
    }

    #[test]
    fn parses_agent_update_check_request() {
        let request = parse_vty_agent_update_check(&[
            "--version-url",
            "https://github.com/mnihyc/vpsman/releases/latest/download/version.json",
            "tag:edge",
            "--timeout",
            "300",
            "--privilege-ttl",
            "120",
            "--force-unprivileged",
            "--confirmed",
        ])
        .unwrap();

        assert_eq!(
            request.version_url,
            Some(
                "https://github.com/mnihyc/vpsman/releases/latest/download/version.json"
                    .to_string()
            )
        );
        assert!(request.activate);
        assert!(request.restart_agent);
        assert_eq!(request.selection.tags, vec!["edge"]);
        assert_eq!(request.timeout_secs, 300);
        assert_eq!(request.privilege_ttl_secs, 120);
        assert!(request.force_unprivileged);

        let no_activate = parse_vty_agent_update_check(&[
            "--version-url",
            "file:///tmp/version.json",
            "id:edge-a",
            "--no-activate",
            "--confirmed",
        ])
        .unwrap();
        assert!(!no_activate.activate);
        assert!(!no_activate.restart_agent);
    }

    #[test]
    fn parses_agent_update_activation_and_rollback_requests() {
        let activate = parse_vty_agent_update_activate(&[
            "--staged-sha256-hex",
            &"aa".repeat(32),
            "id:edge-a",
            "--timeout",
            "30",
            "--restart-agent",
            "--force-unprivileged",
            "--confirmed",
        ])
        .unwrap();
        assert_eq!(activate.staged_sha256_hex, "aa".repeat(32));
        assert!(activate.selection.clients.is_empty());
        assert_eq!(activate.selection.tags, vec!["id:edge-a"]);
        assert!(activate.restart_agent);
        assert!(activate.selection.confirmed);
        assert!(activate.force_unprivileged);

        let rollback = parse_vty_agent_update_rollback(&[
            "--rollback-sha256-hex",
            &"bb".repeat(32),
            "tag:bgp",
            "--force-unprivileged",
            "--confirmed",
        ])
        .unwrap();
        assert_eq!(rollback.rollback_sha256_hex, Some("bb".repeat(32)));
        assert_eq!(rollback.selection.tags, vec!["bgp"]);
        assert!(rollback.selection.confirmed);
        assert!(rollback.force_unprivileged);
    }

    #[test]
    fn rejects_unconfirmed_or_bad_agent_update_activation_requests() {
        assert!(parse_vty_agent_update_activate(&[
            "--staged-sha256-hex",
            &"aa".repeat(32),
            "id:edge-a",
        ])
        .is_err());
        assert!(parse_vty_agent_update_activate(&[
            "--staged-sha256-hex",
            "not-a-hash",
            "id:edge-a",
            "--confirmed",
        ])
        .is_err());
        assert!(parse_vty_agent_update_rollback(&[
            "--rollback-sha256-hex",
            "not-a-hash",
            "id:edge-a",
            "--confirmed",
        ])
        .is_err());
    }

    #[test]
    fn rejects_unconfirmed_or_non_https_agent_update() {
        assert!(parse_vty_agent_update(&[
            "--artifact-url",
            "https://updates.example/vpsman-agent",
            "--sha256-hex",
            &"ab".repeat(32),
            "tag:edge",
        ])
        .is_err());
        assert!(parse_vty_agent_update(&[
            "--artifact-url",
            "http://updates.example/vpsman-agent",
            "--sha256-hex",
            &"ab".repeat(32),
            "tag:edge",
            "--confirmed",
        ])
        .is_err());
        assert!(parse_vty_agent_update_check(&[
            "--version-url",
            "http://updates.example/version.json",
            "tag:edge",
            "--confirmed",
        ])
        .is_err());
    }
}
