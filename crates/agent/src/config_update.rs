use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use vpsman_common::{
    validate_data_source_config_patch_section, validate_hot_config_update, AgentConfig,
    CommandOutput, OutputStream, MAX_AGENT_HOT_CONFIG_BYTES,
};

pub(crate) const REDACTED_PRESERVE: &str = "<redacted:preserve>";

pub(crate) fn read_redacted_config(
    job_id: uuid::Uuid,
    current: &AgentConfig,
    config_path: &Path,
) -> Result<Vec<CommandOutput>> {
    let mut redacted = current.clone();
    redact_preserved_fields(&mut redacted);
    let redacted_toml =
        toml::to_string_pretty(&redacted).context("failed to serialize redacted config")?;
    Ok(vec![CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "config_read",
            "status": "read",
            "config_path": config_path.display().to_string(),
            "toml": redacted_toml,
            "base_config_sha256_hex": config_sha256_hex(current)?,
            "redaction_token": REDACTED_PRESERVE,
            "redacted_fields": redacted_config_fields(),
            "supported_sections": [
                "display_name",
                "tcp_endpoints",
                "discovery_url",
                "backup",
                "update",
                "execution",
                "telemetry",
                "network",
                "telemetry_light_secs",
                "telemetry_full_secs",
                "tags"
            ],
            "autocomplete": supported_config_autocomplete(),
        }))?,
        exit_code: Some(0),
        done: true,
    }])
}

pub(crate) fn apply_hot_config_update(
    job_id: uuid::Uuid,
    current: &mut AgentConfig,
    config_path: &Path,
    toml_document: &str,
    preserve_redacted: bool,
    base_config_sha256_hex: Option<&str>,
) -> Result<Vec<CommandOutput>> {
    anyhow::ensure!(
        toml_document.len() <= MAX_AGENT_HOT_CONFIG_BYTES,
        "hot config TOML exceeds {} bytes",
        MAX_AGENT_HOT_CONFIG_BYTES
    );
    if let Some(base_config_sha256_hex) = base_config_sha256_hex {
        anyhow::ensure!(
            config_sha256_hex(current)? == base_config_sha256_hex,
            "hot config base hash is stale"
        );
    }
    let mut updated: AgentConfig =
        toml::from_str(toml_document).context("failed to parse hot config TOML")?;
    if preserve_redacted {
        preserve_redacted_fields(current, &mut updated);
    }
    validate_hot_config_update(current, &updated)
        .map_err(|message| anyhow::anyhow!("invalid hot config: {message}"))?;
    persist_config_update(current, &updated, config_path)?;
    *current = updated;

    Ok(vec![CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "hot_config",
            "status": "applied",
            "config_path": config_path.display().to_string(),
            "rollback_path": rollback_path(config_path).display().to_string(),
            "base_config_sha256_hex": base_config_sha256_hex,
            "new_config_sha256_hex": config_sha256_hex(current)?,
        }))?,
        exit_code: Some(0),
        done: true,
    }])
}

pub(crate) fn config_sha256_hex(config: &AgentConfig) -> Result<String> {
    let document = toml::to_string_pretty(config).context("failed to serialize config for hash")?;
    Ok(hex::encode(Sha256::digest(document.as_bytes())))
}

fn redact_preserved_fields(config: &mut AgentConfig) {
    config.client_id = REDACTED_PRESERVE.to_string();
    if config.noise.client_private_key_hex.is_some() {
        config.noise.client_private_key_hex = Some(REDACTED_PRESERVE.to_string());
    }
    if config.noise.server_public_key_hex.is_some() {
        config.noise.server_public_key_hex = Some(REDACTED_PRESERVE.to_string());
    }
    if config.auth.server_ed25519_public_key_hex.is_some() {
        config.auth.server_ed25519_public_key_hex = Some(REDACTED_PRESERVE.to_string());
    }
    if config.update.trusted_artifact_signing_key_hex.is_some() {
        config.update.trusted_artifact_signing_key_hex = Some(REDACTED_PRESERVE.to_string());
    }
}

fn preserve_redacted_fields(current: &AgentConfig, updated: &mut AgentConfig) {
    if updated.client_id == REDACTED_PRESERVE {
        updated.client_id = current.client_id.clone();
    }
    if updated.noise.client_private_key_hex.as_deref() == Some(REDACTED_PRESERVE) {
        updated.noise.client_private_key_hex = current.noise.client_private_key_hex.clone();
    }
    if updated.noise.server_public_key_hex.as_deref() == Some(REDACTED_PRESERVE) {
        updated.noise.server_public_key_hex = current.noise.server_public_key_hex.clone();
    }
    if updated.auth.server_ed25519_public_key_hex.as_deref() == Some(REDACTED_PRESERVE) {
        updated.auth.server_ed25519_public_key_hex =
            current.auth.server_ed25519_public_key_hex.clone();
    }
    if updated.update.trusted_artifact_signing_key_hex.as_deref() == Some(REDACTED_PRESERVE) {
        updated.update.trusted_artifact_signing_key_hex =
            current.update.trusted_artifact_signing_key_hex.clone();
    }
}

fn redacted_config_fields() -> Vec<&'static str> {
    vec![
        "client_id",
        "noise.client_private_key_hex",
        "noise.server_public_key_hex",
        "auth.server_ed25519_public_key_hex",
        "update.trusted_artifact_signing_key_hex",
    ]
}

fn supported_config_autocomplete() -> serde_json::Value {
    serde_json::json!({
        "top_level": [
            "display_name",
            "tcp_endpoints",
            "discovery_url",
            "telemetry_light_secs",
            "telemetry_full_secs",
            "tags"
        ],
        "sections": {
            "backup": ["recipient_public_key_hex", "max_plaintext_bytes"],
            "update": [
                "unmanaged_enabled",
                "unmanaged_version_url",
                "unmanaged_interval_secs",
                "unmanaged_jitter_secs",
                "unmanaged_activate",
                "unmanaged_restart_agent"
            ],
            "execution": [
                "shell_script_argv",
                "working_directory",
                "environment_policy",
                "environment_keep",
                "environment_set",
                "pty_policy",
                "process_cleanup"
            ],
            "telemetry": [
                "source",
                "proc_root",
                "sys_class_net_dir",
                "hostname_file",
                "os_release_file",
                "custom_metrics_command"
            ],
            "network": [
                "root_dir",
                "backend",
                "preset",
                "apply_enabled",
                "validate_enabled",
                "reload_enabled",
                "runtime_reconcile_enabled"
            ]
        }
    })
}

pub(crate) fn apply_data_source_config_patch(
    job_id: uuid::Uuid,
    current: &mut AgentConfig,
    config_path: &Path,
    toml_document: &str,
) -> Result<Vec<CommandOutput>> {
    anyhow::ensure!(
        toml_document.len() <= MAX_AGENT_HOT_CONFIG_BYTES,
        "data-source config patch TOML exceeds {} bytes",
        MAX_AGENT_HOT_CONFIG_BYTES
    );
    let patch: toml::Value =
        toml::from_str(toml_document).context("failed to parse data-source config patch TOML")?;
    let mut merged = toml::Value::try_from(&*current)
        .context("failed to serialize current config before data-source patch")?;
    merge_data_source_patch(&mut merged, patch)?;
    let updated: AgentConfig = merged
        .try_into()
        .context("failed to parse merged data-source config")?;
    validate_hot_config_update(current, &updated)
        .map_err(|message| anyhow::anyhow!("invalid data-source config patch: {message}"))?;
    persist_config_update(current, &updated, config_path)?;
    *current = updated;

    Ok(vec![CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "data_source_config_patch",
            "status": "applied",
            "config_path": config_path.display().to_string(),
            "rollback_path": rollback_path(config_path).display().to_string(),
        }))?,
        exit_code: Some(0),
        done: true,
    }])
}

fn merge_data_source_patch(target: &mut toml::Value, patch: toml::Value) -> Result<()> {
    let target_table = target
        .as_table_mut()
        .context("current config is not a TOML table")?;
    let toml::Value::Table(patch_table) = patch else {
        anyhow::bail!("data-source config patch must be a TOML table");
    };
    anyhow::ensure!(
        !patch_table.is_empty(),
        "data-source config patch must contain at least one section"
    );
    for (section, value) in patch_table {
        validate_data_source_config_patch_section(&section)
            .map_err(|message| anyhow::anyhow!(message))?;
        merge_toml_value(target_table, section, value);
    }
    Ok(())
}

fn merge_toml_value(
    target: &mut toml::map::Map<String, toml::Value>,
    key: String,
    value: toml::Value,
) {
    match (target.get_mut(&key), value) {
        (Some(toml::Value::Table(target_table)), toml::Value::Table(patch_table)) => {
            merge_toml_table(target_table, patch_table);
        }
        (_, value) => {
            target.insert(key, value);
        }
    }
}

fn merge_toml_table(
    target: &mut toml::map::Map<String, toml::Value>,
    patch: toml::map::Map<String, toml::Value>,
) {
    for (key, value) in patch {
        match (target.get_mut(&key), value) {
            (Some(toml::Value::Table(target_table)), toml::Value::Table(patch_table)) => {
                merge_toml_table(target_table, patch_table);
            }
            (_, value) => {
                target.insert(key, value);
            }
        }
    }
}

fn persist_config_update(
    current: &AgentConfig,
    updated: &AgentConfig,
    config_path: &Path,
) -> Result<()> {
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let rollback = rollback_path(config_path);
    if config_path.exists() {
        fs::copy(config_path, &rollback).with_context(|| {
            format!(
                "failed to write hot config rollback copy {}",
                rollback.display()
            )
        })?;
    } else {
        let current_document =
            toml::to_string_pretty(current).context("failed to serialize current config")?;
        fs::write(&rollback, current_document)
            .with_context(|| format!("failed to write rollback config {}", rollback.display()))?;
    }

    let temp = temp_config_path(config_path);
    let updated_document =
        toml::to_string_pretty(updated).context("failed to serialize hot config")?;
    fs::write(&temp, updated_document)
        .with_context(|| format!("failed to write temp config {}", temp.display()))?;
    fs::rename(&temp, config_path).with_context(|| {
        let _ = fs::remove_file(&temp);
        format!(
            "failed to atomically replace config {} with {}",
            config_path.display(),
            temp.display()
        )
    })?;
    Ok(())
}

fn rollback_path(config_path: &Path) -> PathBuf {
    let file_name = config_path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "agent.toml".into());
    config_path.with_file_name(format!("{file_name}.rollback"))
}

fn temp_config_path(config_path: &Path) -> PathBuf {
    let file_name = config_path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "agent.toml".into());
    config_path.with_file_name(format!("{file_name}.tmp-{}", uuid::Uuid::new_v4()))
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use vpsman_common::{AgentConfig, ServerEndpoint};

    use super::{apply_data_source_config_patch, apply_hot_config_update};

    fn temp_config_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("{name}-{}.toml", uuid::Uuid::new_v4()))
    }

    #[test]
    fn applies_valid_hot_config_and_writes_rollback() {
        let mut current = AgentConfig::default();
        let path = temp_config_path("vpsman-hot-config-apply");
        fs::write(&path, toml::to_string_pretty(&current).unwrap()).unwrap();

        let mut updated = current.clone();
        updated.display_name = "edge-a".to_string();
        updated.telemetry_light_secs = 10;
        updated.telemetry_full_secs = 30;
        updated.tags = vec!["bgp".to_string(), "provider-a".to_string()];
        updated.tcp_endpoints = vec![ServerEndpoint {
            label: "primary".to_string(),
            tcp_addr: "gateway.example.test:9443".to_string(),
            priority: 1,
        }];
        let outputs = apply_hot_config_update(
            uuid::Uuid::new_v4(),
            &mut current,
            &path,
            &toml::to_string_pretty(&updated).unwrap(),
            false,
            None,
        )
        .unwrap();

        let saved: AgentConfig = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(current, updated);
        assert_eq!(saved, updated);
        assert_eq!(outputs.len(), 1);
        assert!(path
            .with_file_name(format!(
                "{}.rollback",
                path.file_name().unwrap().to_string_lossy()
            ))
            .exists());

        let _ = fs::remove_file(path.with_file_name(format!(
            "{}.rollback",
            path.file_name().unwrap().to_string_lossy()
        )));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_identity_changes_before_writing_config() {
        let mut current = AgentConfig::default();
        let path = temp_config_path("vpsman-hot-config-reject");
        let mut updated = current.clone();
        updated.client_id = "other".to_string();

        assert!(apply_hot_config_update(
            uuid::Uuid::new_v4(),
            &mut current,
            &path,
            &toml::to_string_pretty(&updated).unwrap(),
            false,
            None,
        )
        .is_err());
        assert!(!path.exists());
    }

    #[test]
    fn applies_data_source_config_patch_without_replacing_identity() {
        let mut current = AgentConfig::default();
        let path = temp_config_path("vpsman-data-source-config-patch");
        fs::write(&path, toml::to_string_pretty(&current).unwrap()).unwrap();

        let outputs = apply_data_source_config_patch(
            uuid::Uuid::new_v4(),
            &mut current,
            &path,
            "[telemetry]\nproc_root = \"/tmp/vpsman-proc\"\n",
        )
        .unwrap();

        let saved: AgentConfig = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(current.client_id, AgentConfig::default().client_id);
        assert_eq!(current.auth, AgentConfig::default().auth);
        assert_eq!(current.telemetry.proc_root, "/tmp/vpsman-proc");
        assert_eq!(saved.telemetry.proc_root, "/tmp/vpsman-proc");
        assert_eq!(outputs.len(), 1);

        let _ = fs::remove_file(path.with_file_name(format!(
            "{}.rollback",
            path.file_name().unwrap().to_string_lossy()
        )));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_data_source_config_patch_outside_allowed_sections() {
        let mut current = AgentConfig::default();
        let path = temp_config_path("vpsman-data-source-config-patch-reject");

        assert!(apply_data_source_config_patch(
            uuid::Uuid::new_v4(),
            &mut current,
            &path,
            "client_id = \"other\"\n",
        )
        .is_err());
        assert!(!path.exists());
    }
}
