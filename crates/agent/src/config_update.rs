use std::path::Path;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use vpsman_common::{AgentConfig, CommandOutput, OutputStream};

pub(crate) const REDACTED_PRESERVE: &str = "<redacted>";

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
            "scope": "effective_runtime_config",
            "bootstrap_config_path": config_path.display().to_string(),
            "toml": redacted_toml,
            "config_sha256_hex": config_sha256_hex(current)?,
            "redacted_fields": redacted_config_fields(),
            "supported_sections": [
                "display_name",
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

pub(crate) fn config_sha256_hex(config: &AgentConfig) -> Result<String> {
    let document = toml::to_string_pretty(config).context("failed to serialize config for hash")?;
    Ok(hex::encode(Sha256::digest(document.as_bytes())))
}

fn redact_preserved_fields(config: &mut AgentConfig) {
    if config.noise.client_private_key_hex.is_some() {
        config.noise.client_private_key_hex = Some(REDACTED_PRESERVE.to_string());
    }
    if config.noise.server_public_key_hex.is_some() {
        config.noise.server_public_key_hex = Some(REDACTED_PRESERVE.to_string());
    }
}

fn redacted_config_fields() -> Vec<&'static str> {
    vec![
        "noise.client_private_key_hex",
        "noise.server_public_key_hex",
    ]
}

fn supported_config_autocomplete() -> serde_json::Value {
    serde_json::json!({
        "top_level": [
            "display_name",
            "telemetry_light_secs",
            "telemetry_full_secs",
            "tags"
        ],
        "sections": {
            "backup": [
                "max_uncompressed_bytes",
                "max_archive_bytes",
            ],
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
                "runtime_reconcile_enabled",
                "runtime_status_telemetry_enabled",
                "runtime_status_telemetry_interval_secs",
                "latency_monitoring_enabled",
                "latency_monitoring_interval_secs",
                "latency_down_windows",
                "auto_ospf_enabled",
                "auto_ospf_min_cost_delta",
                "auto_ospf_healthy_windows",
                "auto_ospf_policy",
                "auto_ospf_updater"
            ]
        }
    })
}
