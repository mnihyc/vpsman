use anyhow::{bail, Context, Result};
use serde_json::{Map, Value};

use crate::{
    model::{DataSourceHotConfigView, DataSourcePresetView},
    repository::Repository,
    unix_now,
};

const MAX_PRESET_ARGV_ITEMS: usize = 32;
const MAX_PRESET_ARG_BYTES: usize = 4096;

impl Repository {
    pub(crate) async fn render_data_source_hot_config(
        &self,
        client_id: &str,
    ) -> Result<DataSourceHotConfigView> {
        let agents = self.list_agents().await?;
        anyhow::ensure!(
            agents.iter().any(|agent| agent.id == client_id),
            "data_source_hot_config_client_not_found:{client_id}"
        );

        let assignments = self
            .list_data_source_assignments(Some(client_id), None)
            .await?;
        let presets = self.list_data_source_presets(None).await?;
        let mut renderer = HotConfigRenderer::default();

        for assignment in &assignments {
            let preset = presets
                .iter()
                .find(|candidate| candidate.id == assignment.preset_id)
                .with_context(|| {
                    format!(
                        "data_source_hot_config_preset_not_found:{}",
                        assignment.preset_id
                    )
                })?;
            renderer.apply_preset(&assignment.domain, preset)?;
        }

        let sections = Value::Object(renderer.sections);
        let toml = toml::to_string_pretty(&sections)
            .context("failed to serialize data-source config patch TOML")?;
        Ok(DataSourceHotConfigView {
            client_id: client_id.to_string(),
            sections,
            toml,
            assignments,
            unsupported_domains: renderer.unsupported_domains,
            render_notes: renderer.render_notes,
            generated_at: unix_now().to_string(),
        })
    }
}

pub(crate) struct DataSourcePresetRenderCheck {
    pub(crate) sections: Value,
    pub(crate) toml: String,
    pub(crate) unsupported_domains: Vec<String>,
    pub(crate) render_notes: Vec<String>,
}

pub(crate) fn render_data_source_preset_candidate(
    preset: &DataSourcePresetView,
) -> Result<DataSourcePresetRenderCheck> {
    let mut renderer = HotConfigRenderer::default();
    renderer.apply_preset(&preset.domain, preset)?;
    let sections = Value::Object(renderer.sections);
    let toml = toml::to_string_pretty(&sections)
        .context("failed to serialize data-source preset test TOML")?;
    Ok(DataSourcePresetRenderCheck {
        sections,
        toml,
        unsupported_domains: renderer.unsupported_domains,
        render_notes: renderer.render_notes,
    })
}

#[derive(Default)]
struct HotConfigRenderer {
    sections: Map<String, Value>,
    unsupported_domains: Vec<String>,
    render_notes: Vec<String>,
}

impl HotConfigRenderer {
    fn apply_preset(&mut self, domain: &str, preset: &DataSourcePresetView) -> Result<()> {
        match domain {
            "telemetry_metrics_source" => self.apply_telemetry_source(preset),
            "process_inventory_source" => self.apply_process_source(preset),
            "user_session_inventory_source" => self.apply_user_session_source(preset),
            "command_execution_policy" => self.apply_command_execution_policy(preset),
            "latency_probe_source" => self.apply_latency_probe_source(preset),
            "runtime_traffic_accounting_source" => self.apply_runtime_traffic_source(preset),
            "runtime_tunnel_adapter" => self.apply_runtime_tunnel_adapter(preset),
            "routing_daemon_adapter" => self.apply_routing_daemon_adapter(preset),
            "speed_test_provider"
            | "process_supervisor_policy"
            | "traffic_limit_status_source"
            | "backup_object_store"
            | "restore_path_mapping"
            | "update_artifact_source"
            | "update_restart_policy"
            | "update_rollback_heartbeat_source" => {
                self.unsupported_domains.push(format!(
                    "{domain}:{} requires a job, object-store, or release workflow rather than agent hot-config",
                    preset.name
                ));
                Ok(())
            }
            _ => {
                self.unsupported_domains
                    .push(format!("{domain}:{} is not renderable", preset.name));
                Ok(())
            }
        }
    }

    fn apply_telemetry_source(&mut self, preset: &DataSourcePresetView) -> Result<()> {
        let source = string_field(&preset.definition, "source").unwrap_or("linux_procfs");
        let section = self.section_mut("telemetry")?;
        match source {
            "linux_procfs" | "custom_command" | "linux_procfs_and_custom_command" => {
                insert_string(section, "source", source);
            }
            _ => bail!("unsupported_telemetry_metrics_source:{source}"),
        }
        for (definition_key, config_key) in [
            ("proc_root", "proc_root"),
            ("sys_class_net_dir", "sys_class_net_dir"),
            ("hostname_file", "hostname_file"),
            ("os_release_file", "os_release_file"),
        ] {
            if let Some(path) = string_field(&preset.definition, definition_key) {
                validate_absolute_path(path, definition_key)?;
                insert_string(section, config_key, path);
            }
        }
        if matches!(source, "custom_command" | "linux_procfs_and_custom_command") {
            let command = command_field(
                &preset.definition,
                &["custom_metrics_command", "metrics_command", "command"],
            )?
            .with_context(|| {
                format!(
                    "telemetry_metrics_source:{} requires custom_metrics_command",
                    preset.name
                )
            })?;
            section.insert("custom_metrics_command".to_string(), command);
        }
        Ok(())
    }

    fn apply_process_source(&mut self, preset: &DataSourcePresetView) -> Result<()> {
        let source = string_field(&preset.definition, "source").unwrap_or("linux_procfs");
        let section = self.section_mut("execution")?;
        match source {
            "linux_procfs" => {
                insert_string(section, "process_inventory_source", source);
                if let Some(proc_root) = string_field(&preset.definition, "proc_root") {
                    validate_absolute_path(proc_root, "proc_root")?;
                    insert_string(section, "process_proc_root", proc_root);
                }
            }
            "custom_command" => {
                insert_string(section, "process_inventory_source", source);
                let command = command_field(
                    &preset.definition,
                    &["process_inventory_command", "process_command", "command"],
                )?
                .with_context(|| {
                    format!(
                        "process_inventory_source:{} requires process_inventory_command",
                        preset.name
                    )
                })?;
                section.insert("process_inventory_command".to_string(), command);
            }
            _ => bail!("unsupported_process_inventory_source:{source}"),
        }
        Ok(())
    }

    fn apply_user_session_source(&mut self, preset: &DataSourcePresetView) -> Result<()> {
        let source = string_field(&preset.definition, "source").unwrap_or("linux_w_who_preset");
        let section = self.section_mut("execution")?;
        match source {
            "linux_w_who_preset" => {
                insert_string(section, "user_sessions_source", source);
                if let Some(command) =
                    command_field(&preset.definition, &["user_sessions_command", "command"])?
                {
                    section.insert("user_sessions_command".to_string(), command);
                }
            }
            "custom_command" => {
                insert_string(section, "user_sessions_source", source);
                let command =
                    command_field(&preset.definition, &["user_sessions_command", "command"])?
                        .with_context(|| {
                            format!(
                                "user_session_inventory_source:{} requires user_sessions_command",
                                preset.name
                            )
                        })?;
                section.insert("user_sessions_command".to_string(), command);
            }
            _ => bail!("unsupported_user_session_inventory_source:{source}"),
        }
        Ok(())
    }

    fn apply_command_execution_policy(&mut self, preset: &DataSourcePresetView) -> Result<()> {
        let section = self.section_mut("execution")?;
        if let Some(argv) = argv_field(&preset.definition, &["shell_script_argv"])? {
            section.insert("shell_script_argv".to_string(), Value::Array(argv));
        }
        if let Some(working_directory) = string_field(&preset.definition, "working_directory") {
            validate_absolute_path(working_directory, "working_directory")?;
            insert_string(section, "working_directory", working_directory);
        }
        if let Some(policy) = string_field(&preset.definition, "environment_policy") {
            validate_one_of(
                policy,
                &["inherit", "clean", "minimal_path"],
                "environment_policy",
            )?;
            insert_string(section, "environment_policy", policy);
        }
        if let Some(keep) = string_array_field(&preset.definition, "environment_keep", 64)? {
            section.insert(
                "environment_keep".to_string(),
                Value::Array(keep.into_iter().map(Value::String).collect()),
            );
        }
        if let Some(environment_set) =
            object_string_field(&preset.definition, "environment_set", 64)?
        {
            section.insert("environment_set".to_string(), environment_set);
        }
        if let Some(policy) = string_field(&preset.definition, "pty_policy") {
            validate_one_of(policy, &["native_pty", "disabled"], "pty_policy")?;
            insert_string(section, "pty_policy", policy);
        }
        if let Some(policy) = string_field(&preset.definition, "process_cleanup") {
            validate_one_of(
                policy,
                &["process_group", "direct_child"],
                "process_cleanup",
            )?;
            insert_string(section, "process_cleanup", policy);
        }
        if section.is_empty() {
            self.render_notes.push(format!(
                "command_execution_policy:{} has no supported fields; agent defaults remain selected",
                preset.name
            ));
        }
        Ok(())
    }

    fn apply_latency_probe_source(&mut self, preset: &DataSourcePresetView) -> Result<()> {
        if let Some(argv) = argv_field(
            &preset.definition,
            &["probe_ping_argv", "ping_argv", "argv"],
        )? {
            let section = self.section_mut("network")?;
            section.insert("probe_ping_argv".to_string(), Value::Array(argv));
        } else {
            self.render_notes.push(format!(
                "latency_probe_source:{} uses the built-in probe preset",
                preset.name
            ));
        }
        Ok(())
    }

    fn apply_runtime_traffic_source(&mut self, preset: &DataSourcePresetView) -> Result<()> {
        let source = string_field(&preset.definition, "source").unwrap_or("interface_counters");
        match source {
            "interface_counters" => {
                self.render_notes.push(format!(
                    "runtime_traffic_accounting_source:{} uses interface counters and needs no global argv",
                    preset.name
                ));
            }
            "vnstat" => {
                if let Some(argv) =
                    argv_field(&preset.definition, &["runtime_vnstat_argv", "vnstat_argv"])?
                {
                    let section = self.section_mut("network")?;
                    section.insert("runtime_vnstat_argv".to_string(), Value::Array(argv));
                } else {
                    self.unsupported_domains.push(format!(
                        "runtime_traffic_accounting_source:{} selected vnstat but has no vnstat_argv; traffic_command belongs to per-tunnel plans",
                        preset.name
                    ));
                }
            }
            "custom_command" => {
                self.unsupported_domains.push(format!(
                    "runtime_traffic_accounting_source:{} custom traffic commands require per-tunnel runtime telemetry plans",
                    preset.name
                ));
            }
            _ => bail!("unsupported_runtime_traffic_accounting_source:{source}"),
        }
        Ok(())
    }

    fn apply_runtime_tunnel_adapter(&mut self, preset: &DataSourcePresetView) -> Result<()> {
        let manager =
            string_field(&preset.definition, "manager").unwrap_or("agent_iproute2_managed");
        match manager {
            "agent_iproute2_managed" => {
                let section = self.section_mut("network")?;
                if bool_field(&preset.definition, "runtime_reconcile_enabled").unwrap_or(false) {
                    section.insert("apply_enabled".to_string(), Value::Bool(true));
                    section.insert("runtime_reconcile_enabled".to_string(), Value::Bool(true));
                }
                if let Some(argv) = argv_field(&preset.definition, &["runtime_ip_argv", "ip_argv"])?
                {
                    section.insert("runtime_ip_argv".to_string(), Value::Array(argv));
                }
                if let Some(argv) = argv_field(&preset.definition, &["runtime_tc_argv", "tc_argv"])?
                {
                    section.insert("runtime_tc_argv".to_string(), Value::Array(argv));
                }
            }
            "external_managed_adapter" | "custom_adapter" => {
                self.unsupported_domains.push(format!(
                    "runtime_tunnel_adapter:{} adapter commands are rendered from tunnel plans, not agent-level fallback config",
                    preset.name
                ));
            }
            _ => bail!("unsupported_runtime_tunnel_adapter:{manager}"),
        }
        Ok(())
    }

    fn apply_routing_daemon_adapter(&mut self, preset: &DataSourcePresetView) -> Result<()> {
        let section = self.section_mut("network")?;
        if let Some(enabled) = bool_field(&preset.definition, "latency_monitoring_enabled") {
            section.insert(
                "latency_monitoring_enabled".to_string(),
                Value::Bool(enabled),
            );
        }
        if let Some(value) = u64_field(&preset.definition, "latency_monitoring_interval_secs") {
            section.insert("latency_monitoring_interval_secs".to_string(), value.into());
        }
        if let Some(value) = u64_field(&preset.definition, "latency_down_windows") {
            section.insert("latency_down_windows".to_string(), value.into());
        }
        if let Some(enabled) = bool_field(&preset.definition, "auto_ospf_enabled") {
            section.insert("auto_ospf_enabled".to_string(), Value::Bool(enabled));
        }
        if let Some(value) = u64_field(&preset.definition, "auto_ospf_min_cost_delta") {
            section.insert("auto_ospf_min_cost_delta".to_string(), value.into());
        }
        if let Some(value) = u64_field(&preset.definition, "auto_ospf_healthy_windows") {
            section.insert("auto_ospf_healthy_windows".to_string(), value.into());
        }
        if let Some(command) = command_field(
            &preset.definition,
            &["auto_ospf_updater", "ospf_updater", "command"],
        )? {
            section.insert("auto_ospf_updater".to_string(), command);
        }
        Ok(())
    }

    fn section_mut(&mut self, name: &str) -> Result<&mut Map<String, Value>> {
        let value = self
            .sections
            .entry(name.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        value
            .as_object_mut()
            .with_context(|| format!("hot_config_section_not_object:{name}"))
    }
}

fn insert_string(section: &mut Map<String, Value>, key: &str, value: &str) {
    section.insert(key.to_string(), Value::String(value.to_string()));
}

fn string_field<'a>(definition: &'a Value, key: &str) -> Option<&'a str> {
    definition.get(key).and_then(Value::as_str)
}

fn bool_field(definition: &Value, key: &str) -> Option<bool> {
    definition.get(key).and_then(Value::as_bool)
}

fn u64_field(definition: &Value, key: &str) -> Option<u64> {
    definition.get(key).and_then(Value::as_u64)
}

fn validate_one_of(value: &str, allowed: &[&str], field: &str) -> Result<()> {
    anyhow::ensure!(
        allowed.contains(&value),
        "{field}_unsupported_value:{value}"
    );
    Ok(())
}

fn string_array_field(
    definition: &Value,
    key: &str,
    max_items: usize,
) -> Result<Option<Vec<String>>> {
    let Some(value) = definition.get(key) else {
        return Ok(None);
    };
    let items = value
        .as_array()
        .with_context(|| format!("{key}_must_be_array"))?;
    anyhow::ensure!(items.len() <= max_items, "{key}_too_many_items");
    let mut output = Vec::with_capacity(items.len());
    for item in items {
        let value = item
            .as_str()
            .with_context(|| format!("{key}_items_must_be_strings"))?;
        validate_environment_key(value, key)?;
        output.push(value.to_string());
    }
    Ok(Some(output))
}

fn object_string_field(definition: &Value, key: &str, max_items: usize) -> Result<Option<Value>> {
    let Some(value) = definition.get(key) else {
        return Ok(None);
    };
    let object = value
        .as_object()
        .with_context(|| format!("{key}_must_be_object"))?;
    anyhow::ensure!(object.len() <= max_items, "{key}_too_many_items");
    let mut output = Map::new();
    for (env_key, env_value) in object {
        validate_environment_key(env_key, key)?;
        let env_value = env_value
            .as_str()
            .with_context(|| format!("{key}_values_must_be_strings"))?;
        anyhow::ensure!(
            env_value.len() <= 4096 && !env_value.as_bytes().contains(&0),
            "{key}_value_invalid"
        );
        output.insert(env_key.clone(), Value::String(env_value.to_string()));
    }
    Ok(Some(Value::Object(output)))
}

fn validate_environment_key(key: &str, field: &str) -> Result<()> {
    anyhow::ensure!(
        !key.is_empty()
            && key.len() <= 128
            && !key.as_bytes()[0].is_ascii_digit()
            && key
                .bytes()
                .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric()),
        "{field}_key_invalid"
    );
    Ok(())
}

fn argv_field(definition: &Value, keys: &[&str]) -> Result<Option<Vec<Value>>> {
    for key in keys {
        if let Some(value) = definition.get(*key) {
            let argv = parse_argv(value, key)?;
            return Ok(Some(argv.into_iter().map(Value::String).collect()));
        }
    }
    Ok(None)
}

fn command_field(definition: &Value, keys: &[&str]) -> Result<Option<Value>> {
    for key in keys {
        if let Some(value) = definition.get(*key) {
            return Ok(Some(parse_command(value, key)?));
        }
    }
    Ok(None)
}

fn parse_command(value: &Value, field: &str) -> Result<Value> {
    let object = value
        .as_object()
        .with_context(|| format!("{field}_must_be_object"))?;
    let argv_value = object
        .get("argv")
        .with_context(|| format!("{field}_argv_required"))?;
    let argv = parse_argv(argv_value, field)?;
    let max_timeout_secs = object
        .get("max_timeout_secs")
        .and_then(Value::as_u64)
        .unwrap_or(10);
    let max_output_bytes = object
        .get("max_output_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(16 * 1024);
    anyhow::ensure!(
        (1..=120).contains(&max_timeout_secs) && (1024..=64 * 1024).contains(&max_output_bytes),
        "{field}_budget_invalid"
    );

    Ok(serde_json::json!({
        "argv": argv,
        "max_timeout_secs": max_timeout_secs,
        "max_output_bytes": max_output_bytes as u32,
    }))
}

fn parse_argv(value: &Value, field: &str) -> Result<Vec<String>> {
    let items = value
        .as_array()
        .with_context(|| format!("{field}_must_be_array"))?;
    anyhow::ensure!(
        !items.is_empty() && items.len() <= MAX_PRESET_ARGV_ITEMS,
        "{field}_argv_invalid"
    );
    let mut argv = Vec::with_capacity(items.len());
    for item in items {
        let part = item
            .as_str()
            .with_context(|| format!("{field}_argv_must_be_strings"))?;
        anyhow::ensure!(
            !part.is_empty() && part.len() <= MAX_PRESET_ARG_BYTES && !part.as_bytes().contains(&0),
            "{field}_argv_invalid"
        );
        argv.push(part.to_string());
    }
    anyhow::ensure!(
        argv[0].starts_with('/'),
        "{field}_executable_must_be_absolute"
    );
    Ok(argv)
}

fn validate_absolute_path(value: &str, field: &str) -> Result<()> {
    anyhow::ensure!(
        !value.is_empty() && value.starts_with('/') && !value.as_bytes().contains(&0),
        "{field}_must_be_absolute"
    );
    anyhow::ensure!(
        !value
            .split('/')
            .any(|segment| matches!(segment, "." | "..")),
        "{field}_must_not_contain_dot_segments"
    );
    Ok(())
}
