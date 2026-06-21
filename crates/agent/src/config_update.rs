use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use vpsman_common::{
    validate_hot_config_update, validate_incremental_config_patch_section,
    write_private_file_atomically, AgentConfig, CommandOutput, OutputStream,
    MAX_AGENT_HOT_CONFIG_BYTES,
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
        "full config override TOML exceeds {} bytes",
        MAX_AGENT_HOT_CONFIG_BYTES
    );
    if let Some(base_config_sha256_hex) = base_config_sha256_hex {
        anyhow::ensure!(
            config_sha256_hex(current)? == base_config_sha256_hex,
            "full config override base hash is stale"
        );
    }
    let mut updated: AgentConfig =
        toml::from_str(toml_document).context("failed to parse full config override TOML")?;
    if preserve_redacted {
        preserve_redacted_fields(current, &mut updated);
    }
    validate_hot_config_update(current, &updated)
        .map_err(|message| anyhow::anyhow!("invalid full config override: {message}"))?;
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
}

fn redacted_config_fields() -> Vec<&'static str> {
    vec![
        "client_id",
        "noise.client_private_key_hex",
        "noise.server_public_key_hex",
    ]
}

fn supported_config_autocomplete() -> serde_json::Value {
    serde_json::json!({
        "top_level": [
            "display_name",
            "tcp_endpoints",
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
                "auto_ospf_updater",
                "runtime_status_telemetry_plans"
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
        validate_incremental_config_patch_section(&section)
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
    let rollback = rollback_path(config_path);
    let rollback_document = match fs::read(config_path) {
        Ok(existing) => existing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            toml::to_string_pretty(current)
                .context("failed to serialize current config")?
                .into_bytes()
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read config {}", config_path.display()));
        }
    };
    write_private_file_atomically(&rollback, &rollback_document)
        .with_context(|| format!("failed to write rollback config {}", rollback.display()))?;

    let updated_document =
        toml::to_string_pretty(updated).context("failed to serialize updated config")?;
    write_private_file_atomically(config_path, updated_document.as_bytes()).with_context(|| {
        format!(
            "failed to atomically replace config {}",
            config_path.display()
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

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

    use vpsman_common::{
        plan_tunnel, AgentConfig, AgentRuntimeStatusTelemetryPlan, AgentRuntimeTrafficSource,
        BandwidthTier, ServerEndpoint, TunnelAddressPair, TunnelEndpointSide, TunnelKind,
        TunnelPlanInput,
    };

    use super::{apply_data_source_config_patch, apply_hot_config_update};

    fn temp_config_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("{name}-{}.toml", uuid::Uuid::new_v4()))
    }

    #[test]
    fn applies_valid_hot_config_and_writes_rollback() {
        let mut current = AgentConfig::default();
        let path = temp_config_path("vpsman-hot-config-apply");
        fs::write(&path, toml::to_string_pretty(&current).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

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
        let rollback = path.with_file_name(format!(
            "{}.rollback",
            path.file_name().unwrap().to_string_lossy()
        ));
        assert!(rollback.exists());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&rollback).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let _ = fs::remove_file(rollback);
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
    fn applies_network_runtime_telemetry_patch() {
        let mut current = AgentConfig::default();
        let path = temp_config_path("vpsman-network-telemetry-config-patch");
        fs::write(&path, toml::to_string_pretty(&current).unwrap()).unwrap();

        let plan = plan_tunnel(&TunnelPlanInput {
            name: "edge-a-edge-b-gre".to_string(),
            interface_name: "gre101".to_string(),
            kind: TunnelKind::Gre,
            left_client_id: current.client_id.clone(),
            right_client_id: "edge-b".to_string(),
            left_underlay: "203.0.113.10".to_string(),
            right_underlay: "203.0.113.11".to_string(),
            address_pool_cidr: String::new(),
            reserved_addresses: Vec::new(),
            ipv4_tunnel: Some(TunnelAddressPair {
                left: "10.88.0.0".to_string(),
                right: "10.88.0.1".to_string(),
                prefix_len: 31,
            }),
            ipv6_address_pool_cidr: None,
            ipv6_tunnel: None,
            latency_primary_family: Default::default(),
            bandwidth: BandwidthTier::M100,
            latency_ms: 10.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            runtime_control: Default::default(),
            runtime_topology: Default::default(),
            ospf_policy: Default::default(),
        })
        .unwrap();

        let mut patch_network = current.network.clone();
        patch_network.runtime_status_telemetry_enabled = true;
        patch_network.latency_monitoring_enabled = true;
        patch_network.auto_ospf_enabled = true;
        patch_network.runtime_status_telemetry_plans = vec![AgentRuntimeStatusTelemetryPlan {
            plan_id: Some("plan-edge-a-edge-b".to_string()),
            endpoint_side: TunnelEndpointSide::Left,
            plan,
            traffic_source: AgentRuntimeTrafficSource::InterfaceCounters,
            traffic_command: None,
            latency_monitoring_enabled: true,
            auto_ospf_enabled: true,
            auto_ospf_updater: None,
        }];
        let mut patch_table = toml::map::Map::new();
        patch_table.insert(
            "network".to_string(),
            toml::Value::try_from(&patch_network).unwrap(),
        );

        let outputs = apply_data_source_config_patch(
            uuid::Uuid::new_v4(),
            &mut current,
            &path,
            &toml::to_string_pretty(&toml::Value::Table(patch_table)).unwrap(),
        )
        .unwrap();

        let saved: AgentConfig = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(outputs.len(), 1);
        assert!(current.network.runtime_status_telemetry_enabled);
        assert!(current.network.latency_monitoring_enabled);
        assert!(current.network.auto_ospf_enabled);
        assert_eq!(current.network.runtime_status_telemetry_plans.len(), 1);
        assert_eq!(
            current.network.runtime_status_telemetry_plans[0]
                .plan_id
                .as_deref(),
            Some("plan-edge-a-edge-b")
        );
        assert_eq!(saved.network, current.network);

        let _ = fs::remove_file(path.with_file_name(format!(
            "{}.rollback",
            path.file_name().unwrap().to_string_lossy()
        )));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn applies_frontend_style_inline_network_runtime_telemetry_patch() {
        let mut current = AgentConfig::default();
        let path = temp_config_path("vpsman-inline-network-telemetry-config-patch");
        fs::write(&path, toml::to_string_pretty(&current).unwrap()).unwrap();
        let client_id = current.client_id.clone();
        let patch = format!(
            r#"
[network]
runtime_status_telemetry_enabled = true
latency_monitoring_enabled = true
auto_ospf_enabled = true
runtime_status_telemetry_plans = [{{ plan_id = "plan-inline", endpoint_side = "left", latency_monitoring_enabled = true, auto_ospf_enabled = true, plan = {{ name = "edge-a-edge-b-gre", interface_name = "gre101", kind = "gre", runtime_control = {{ manager = "agent_iproute2_managed" }}, runtime_topology = {{ }}, left_client_id = "{client_id}", right_client_id = "edge-b", left_underlay = "203.0.113.10", right_underlay = "203.0.113.11", left_tunnel_address = "10.88.0.0", right_tunnel_address = "10.88.0.1", tunnel_prefix_len = 31, ipv4_tunnel = {{ left = "10.88.0.0", right = "10.88.0.1", prefix_len = 31 }}, latency_primary_family = "ipv4", bandwidth = "100m", recommended_ospf_cost = 25, ifupdown_file = "/etc/network/interfaces.d/vpsman-tunnels", bird2_file = "/etc/bird/vpsman-ospf.conf", ifupdown_snippet = "", bird2_interface_snippet = "", touched_files = [], validation_steps = [], rollback_notes = [], conflicts = [], mutates_host = false }} }}]
"#
        );

        apply_data_source_config_patch(uuid::Uuid::new_v4(), &mut current, &path, &patch).unwrap();

        assert_eq!(current.network.runtime_status_telemetry_plans.len(), 1);
        let telemetry_plan = &current.network.runtime_status_telemetry_plans[0];
        assert_eq!(telemetry_plan.plan_id.as_deref(), Some("plan-inline"));
        assert_eq!(telemetry_plan.plan.interface_name, "gre101");
        assert!(telemetry_plan.auto_ospf_enabled);

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
