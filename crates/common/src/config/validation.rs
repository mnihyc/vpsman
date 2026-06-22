use super::models::{
    AgentAuthConfig, AgentBackupConfig, AgentConfig, AgentExecutionConfig, AgentNetworkConfig,
    AgentNetworkPreset, AgentNoiseConfig, AgentProcessInventorySource,
    AgentRuntimeStatusTelemetryPlan, AgentRuntimeTrafficSource, AgentTelemetryConfig,
    AgentTelemetrySource, AgentUpdateConfig, AgentUserSessionsSource, ServerEndpoint,
};
use crate::{
    validate_runtime_topology_intent, validate_runtime_tunnel_control, RuntimeTunnelManager,
    TunnelConfigBackend, TunnelEndpointSide, MAX_CONFIGURABLE_JOB_TIMEOUT_SECS,
};

pub const INCREMENTAL_CONFIG_PATCH_SECTIONS: &[&str] =
    &["update", "telemetry", "execution", "network"];

pub fn validate_agent_config_shape(config: &AgentConfig) -> Result<(), String> {
    validate_identifier(&config.client_id, "client_id", 128)?;
    validate_display_name(&config.display_name)?;
    validate_endpoints(&config.tcp_endpoints)?;
    validate_noise_config(&config.noise)?;
    validate_auth_config(&config.auth)?;
    validate_backup_config(&config.backup)?;
    validate_update_config(&config.update)?;
    validate_execution_config(&config.execution)?;
    validate_telemetry_config(&config.telemetry)?;
    validate_network_config(&config.network)?;
    validate_telemetry_interval(config.telemetry_light_secs, "telemetry_light_secs")?;
    validate_telemetry_interval(config.telemetry_full_secs, "telemetry_full_secs")?;
    if config.telemetry_full_secs < config.telemetry_light_secs {
        return Err("telemetry_full_secs_must_be_greater_than_or_equal_to_light".to_string());
    }
    validate_tags(&config.tags)?;
    Ok(())
}

pub fn validate_hot_config_update(
    current: &AgentConfig,
    updated: &AgentConfig,
) -> Result<(), String> {
    validate_agent_config_shape(updated)?;
    if updated.client_id != current.client_id {
        return Err("hot_config_cannot_change_client_id".to_string());
    }
    if updated.noise != current.noise {
        return Err("hot_config_cannot_change_noise_identity".to_string());
    }
    Ok(())
}

pub fn validate_incremental_config_patch_section(section: &str) -> Result<(), String> {
    if INCREMENTAL_CONFIG_PATCH_SECTIONS.contains(&section) {
        Ok(())
    } else {
        Err(format!("config_patch_section_not_allowed:{section}"))
    }
}

fn validate_identifier(value: &str, field: &str, max_len: usize) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{field}_required"));
    }
    if value.len() > max_len {
        return Err(format!("{field}_too_long"));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
    {
        return Err(format!("{field}_contains_invalid_characters"));
    }
    Ok(())
}

fn validate_display_name(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err("display_name_required".to_string());
    }
    if value.len() > 256 || value.as_bytes().contains(&0) {
        return Err("display_name_invalid".to_string());
    }
    Ok(())
}

fn validate_endpoints(endpoints: &[ServerEndpoint]) -> Result<(), String> {
    if endpoints.is_empty() {
        return Err("tcp_endpoints_required".to_string());
    }
    if endpoints.len() > 16 {
        return Err("tcp_endpoints_too_many".to_string());
    }
    for endpoint in endpoints {
        validate_identifier(&endpoint.label, "tcp_endpoint_label", 64)?;
        if endpoint.tcp_addr.is_empty()
            || endpoint.tcp_addr.len() > 256
            || endpoint.tcp_addr.as_bytes().contains(&0)
            || endpoint.tcp_addr.chars().any(char::is_whitespace)
            || !endpoint.tcp_addr.contains(':')
        {
            return Err("tcp_endpoint_addr_invalid".to_string());
        }
    }
    Ok(())
}

fn validate_noise_config(config: &AgentNoiseConfig) -> Result<(), String> {
    validate_required_hex32(
        config.client_private_key_hex.as_deref(),
        "client_private_key_hex",
    )?;
    validate_required_hex32(
        config.server_public_key_hex.as_deref(),
        "server_public_key_hex",
    )?;
    Ok(())
}

fn validate_auth_config(config: &AgentAuthConfig) -> Result<(), String> {
    if !(1..=MAX_CONFIGURABLE_JOB_TIMEOUT_SECS).contains(&config.max_job_timeout_secs) {
        return Err("max_job_timeout_secs_out_of_range".to_string());
    }
    if !(1..=3600).contains(&config.gateway_retry_secs) {
        return Err("gateway_retry_secs_out_of_range".to_string());
    }
    if !(1..=300).contains(&config.gateway_connect_timeout_secs) {
        return Err("gateway_connect_timeout_secs_out_of_range".to_string());
    }
    Ok(())
}

fn validate_backup_config(config: &AgentBackupConfig) -> Result<(), String> {
    if !(1..=16 * 1024 * 1024).contains(&config.max_uncompressed_bytes) {
        return Err("backup_max_uncompressed_bytes_out_of_range".to_string());
    }
    if !(1..=32 * 1024 * 1024).contains(&config.max_archive_bytes) {
        return Err("backup_max_archive_bytes_out_of_range".to_string());
    }
    if config.max_archive_bytes < config.max_uncompressed_bytes {
        return Err("backup_max_archive_bytes_below_uncompressed_limit".to_string());
    }
    Ok(())
}

fn validate_update_config(config: &AgentUpdateConfig) -> Result<(), String> {
    validate_update_version_url(&config.unmanaged_version_url)?;
    if !(300..=604_800).contains(&config.unmanaged_interval_secs) {
        return Err("update_unmanaged_interval_secs_out_of_range".to_string());
    }
    if config.unmanaged_jitter_secs > 604_800 {
        return Err("update_unmanaged_jitter_secs_out_of_range".to_string());
    }
    Ok(())
}

fn validate_update_version_url(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.len() > 2048 || value.as_bytes().contains(&0) {
        return Err("update_unmanaged_version_url_invalid".to_string());
    }
    if value.starts_with("https://") {
        return Ok(());
    }
    if let Some(rest) = value.strip_prefix("http://") {
        if is_local_http_authority(rest) {
            return Ok(());
        }
        return Err("update_unmanaged_version_url_http_must_be_localhost".to_string());
    }
    if let Some(path) = value.strip_prefix("file://") {
        if path.starts_with('/') {
            return Ok(());
        }
        return Err("update_unmanaged_version_url_file_must_be_absolute".to_string());
    }
    Err("update_unmanaged_version_url_must_be_https".to_string())
}

fn is_local_http_authority(rest: &str) -> bool {
    let authority = rest.split('/').next().unwrap_or(rest);
    let host = authority.rsplit('@').next().unwrap_or(authority);
    if host == "localhost"
        || host == "127.0.0.1"
        || host.starts_with("localhost:")
        || host.starts_with("127.0.0.1:")
    {
        return true;
    }
    if host == "[::1]" || host.starts_with("[::1]:") {
        return true;
    }
    false
}

fn validate_execution_config(config: &AgentExecutionConfig) -> Result<(), String> {
    if config.shell_script_argv.is_empty() {
        return Err("execution_shell_script_argv_required".to_string());
    }
    validate_network_hook_argv(&config.shell_script_argv, "execution_shell_script_argv")?;
    if let Some(working_directory) = &config.working_directory {
        validate_absolute_config_path(working_directory, "execution_working_directory")?;
    }
    if config.environment_keep.len() > 64 {
        return Err("execution_environment_keep_too_many_entries".to_string());
    }
    for key in &config.environment_keep {
        validate_environment_key(key, "execution_environment_keep")?;
    }
    if config.environment_set.len() > 64 {
        return Err("execution_environment_set_too_many_entries".to_string());
    }
    for (key, value) in &config.environment_set {
        validate_environment_key(key, "execution_environment_set")?;
        if value.len() > 4096 || value.as_bytes().contains(&0) {
            return Err("execution_environment_set_value_invalid".to_string());
        }
    }
    match config.user_sessions_source {
        AgentUserSessionsSource::LinuxWWhoPreset => {
            if let Some(command) = &config.user_sessions_command {
                validate_network_hook_argv(&command.argv, "execution_user_sessions_argv")?;
                validate_runtime_command_budget(command, "execution_user_sessions_command")?;
            }
        }
        AgentUserSessionsSource::CustomCommand => {
            let Some(command) = &config.user_sessions_command else {
                return Err("execution_user_sessions_command_required".to_string());
            };
            validate_network_hook_argv(&command.argv, "execution_user_sessions_argv")?;
            validate_runtime_command_budget(command, "execution_user_sessions_command")?;
        }
    }
    match config.process_inventory_source {
        AgentProcessInventorySource::LinuxProcfs => {
            validate_absolute_config_path(
                &config.process_proc_root,
                "execution_process_proc_root",
            )?;
            if config.process_inventory_command.is_some() {
                return Err(
                    "execution_process_inventory_command_requires_custom_source".to_string()
                );
            }
        }
        AgentProcessInventorySource::CustomCommand => {
            let Some(command) = &config.process_inventory_command else {
                return Err("execution_process_inventory_command_required".to_string());
            };
            validate_network_hook_argv(&command.argv, "execution_process_inventory_argv")?;
            validate_runtime_command_budget(command, "execution_process_inventory_command")?;
        }
    }
    Ok(())
}

fn validate_environment_key(key: &str, context: &str) -> Result<(), String> {
    if key.is_empty()
        || key.len() > 128
        || !key
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
        || key.as_bytes()[0].is_ascii_digit()
    {
        return Err(format!("{context}_key_invalid"));
    }
    Ok(())
}

fn validate_telemetry_config(config: &AgentTelemetryConfig) -> Result<(), String> {
    if matches!(
        config.source,
        AgentTelemetrySource::LinuxProcfs | AgentTelemetrySource::LinuxProcfsAndCustomCommand
    ) {
        validate_absolute_config_path(&config.proc_root, "telemetry_proc_root")?;
        validate_absolute_config_path(&config.sys_class_net_dir, "telemetry_sys_class_net_dir")?;
        if let Some(path) = &config.hostname_file {
            validate_absolute_config_path(path, "telemetry_hostname_file")?;
        }
        if let Some(path) = &config.os_release_file {
            validate_absolute_config_path(path, "telemetry_os_release_file")?;
        }
    }
    match config.source {
        AgentTelemetrySource::LinuxProcfs => {
            if config.custom_metrics_command.is_some() {
                return Err("telemetry_custom_command_requires_custom_source".to_string());
            }
        }
        AgentTelemetrySource::CustomCommand | AgentTelemetrySource::LinuxProcfsAndCustomCommand => {
            let Some(command) = &config.custom_metrics_command else {
                return Err("telemetry_custom_metrics_command_required".to_string());
            };
            validate_network_hook_argv(&command.argv, "telemetry_custom_metrics_argv")?;
            validate_runtime_command_budget(command, "telemetry_custom_metrics_command")?;
        }
    }
    Ok(())
}

fn validate_network_config(config: &AgentNetworkConfig) -> Result<(), String> {
    validate_absolute_config_path(&config.root_dir, "network_root_dir")?;
    validate_network_backend_preset(config.backend, config.preset)?;
    if config.runtime_reconcile_enabled && !config.apply_enabled {
        return Err("network_runtime_reconcile_requires_apply_enabled".to_string());
    }
    if !(1..=120).contains(&config.hook_timeout_secs) {
        return Err("network_hook_timeout_secs_out_of_range".to_string());
    }
    validate_network_hook_argv(&config.runtime_ip_argv, "network_runtime_ip_argv")?;
    validate_network_hook_argv(&config.runtime_tc_argv, "network_runtime_tc_argv")?;
    if !(1..=120).contains(&config.runtime_command_timeout_secs) {
        return Err("network_runtime_command_timeout_secs_out_of_range".to_string());
    }
    if !(1024..=64 * 1024).contains(&config.runtime_command_max_output_bytes) {
        return Err("network_runtime_command_max_output_bytes_out_of_range".to_string());
    }
    validate_network_hook_argv(
        &config.ifupdown_validate_argv,
        "network_ifupdown_validate_argv",
    )?;
    validate_network_hook_argv(&config.bird2_validate_argv, "network_bird2_validate_argv")?;
    for argv in &config.reload_argv {
        validate_network_hook_argv(argv, "network_reload_argv")?;
    }
    for argv in &config.bird2_reload_argv {
        validate_network_hook_argv(argv, "network_bird2_reload_argv")?;
    }
    validate_network_hook_argv(&config.bird2_status_argv, "network_bird2_status_argv")?;
    validate_network_hook_argv(&config.probe_ping_argv, "network_probe_ping_argv")?;
    if !(1..=30).contains(&config.status_probe_timeout_secs) {
        return Err("network_status_probe_timeout_secs_out_of_range".to_string());
    }
    if !(1024..=64 * 1024).contains(&config.status_probe_max_output_bytes) {
        return Err("network_status_probe_max_output_bytes_out_of_range".to_string());
    }
    if !(15..=3600).contains(&config.runtime_status_telemetry_interval_secs) {
        return Err("network_runtime_status_telemetry_interval_secs_out_of_range".to_string());
    }
    validate_network_hook_argv(&config.runtime_vnstat_argv, "network_runtime_vnstat_argv")?;
    if !(15..=3600).contains(&config.latency_monitoring_interval_secs) {
        return Err("network_latency_monitoring_interval_secs_out_of_range".to_string());
    }
    if !(1..=60).contains(&config.latency_down_windows) {
        return Err("network_latency_down_windows_out_of_range".to_string());
    }
    if config.auto_ospf_min_cost_delta == 0 {
        return Err("network_auto_ospf_min_cost_delta_out_of_range".to_string());
    }
    if !(1..=10).contains(&config.auto_ospf_healthy_windows) {
        return Err("network_auto_ospf_healthy_windows_out_of_range".to_string());
    }
    if let Some(command) = &config.auto_ospf_updater {
        validate_network_hook_argv(&command.argv, "network_auto_ospf_updater_argv")?;
        validate_runtime_command_budget(command, "network_auto_ospf_updater")?;
    }
    validate_runtime_status_telemetry_plans(&config.runtime_status_telemetry_plans)?;
    if config.validate_enabled
        && config.preset.is_none()
        && config.ifupdown_validate_argv.is_empty()
        && config.bird2_validate_argv.is_empty()
    {
        return Err("network_validation_argv_required".to_string());
    }
    if config.reload_enabled && !config.validate_enabled {
        return Err("network_reload_requires_validation".to_string());
    }
    if config.reload_enabled
        && config.preset.is_none()
        && config.reload_argv.is_empty()
        && config.bird2_reload_argv.is_empty()
    {
        return Err("network_reload_argv_required".to_string());
    }
    Ok(())
}

fn validate_runtime_status_telemetry_plans(
    plans: &[AgentRuntimeStatusTelemetryPlan],
) -> Result<(), String> {
    if plans.len() > 16 {
        return Err("network_runtime_status_telemetry_plans_too_many".to_string());
    }
    for plan in plans {
        if let Some(plan_id) = &plan.plan_id {
            validate_identifier(plan_id, "network_runtime_status_telemetry_plan_id", 128)?;
        }
        validate_identifier(
            &plan.plan.name,
            "network_runtime_status_telemetry_plan_name",
            128,
        )?;
        validate_runtime_tunnel_control(&plan.plan.runtime_control)
            .map_err(|_| "network_runtime_status_telemetry_control_invalid".to_string())?;
        validate_runtime_topology_intent(&plan.plan.runtime_topology, &plan.plan.interface_name)
            .map_err(|_| "network_runtime_status_telemetry_topology_invalid".to_string())?;
        if plan.plan.runtime_control.manager == RuntimeTunnelManager::ExternalManagedAdapter
            && plan.plan.runtime_control.status.is_none()
        {
            return Err("network_runtime_status_telemetry_status_command_required".to_string());
        }
        if let Some(command) = &plan.auto_ospf_updater {
            validate_network_hook_argv(&command.argv, "network_runtime_auto_ospf_updater_argv")?;
            validate_runtime_command_budget(command, "network_runtime_auto_ospf_updater")?;
        }
        match plan.traffic_source {
            AgentRuntimeTrafficSource::InterfaceCounters => {
                if plan.traffic_command.is_some() {
                    return Err(
                        "network_runtime_traffic_interface_source_cannot_use_command".to_string(),
                    );
                }
            }
            AgentRuntimeTrafficSource::Vnstat => {
                if let Some(command) = &plan.traffic_command {
                    validate_network_hook_argv(
                        &command.argv,
                        "network_runtime_traffic_vnstat_argv",
                    )?;
                }
            }
            AgentRuntimeTrafficSource::CustomCommand => {
                let Some(command) = &plan.traffic_command else {
                    return Err("network_runtime_traffic_custom_command_required".to_string());
                };
                validate_network_hook_argv(&command.argv, "network_runtime_traffic_custom_argv")?;
            }
        }
        if let Some(command) = &plan.traffic_command {
            validate_runtime_command_budget(command, "network_runtime_traffic_command")?;
        }
        let expected_client = match plan.endpoint_side {
            TunnelEndpointSide::Left => &plan.plan.left_client_id,
            TunnelEndpointSide::Right => &plan.plan.right_client_id,
        };
        validate_identifier(
            expected_client,
            "network_runtime_status_telemetry_local_client_id",
            128,
        )?;
    }
    Ok(())
}

fn validate_runtime_command_budget(
    command: &crate::RuntimeTunnelCommand,
    field: &str,
) -> Result<(), String> {
    if !(1..=120).contains(&command.max_timeout_secs)
        || !(1024..=64 * 1024).contains(&command.max_output_bytes)
    {
        return Err(format!("{field}_invalid"));
    }
    Ok(())
}

fn validate_network_backend_preset(
    backend: TunnelConfigBackend,
    preset: Option<AgentNetworkPreset>,
) -> Result<(), String> {
    let Some(preset) = preset else {
        return Ok(());
    };
    let compatible = matches!(
        (backend, preset),
        (
            TunnelConfigBackend::Ifupdown,
            AgentNetworkPreset::DebianIfupdown2Bird2 | AgentNetworkPreset::DebianIfupdownBird2
        ) | (
            TunnelConfigBackend::Netplan,
            AgentNetworkPreset::DebianNetplanBird2
        ) | (
            TunnelConfigBackend::SystemdNetworkd,
            AgentNetworkPreset::DebianSystemdNetworkdBird2
        )
    );
    if compatible {
        Ok(())
    } else {
        Err("network_backend_preset_mismatch".to_string())
    }
}

fn validate_network_hook_argv(argv: &[String], field: &str) -> Result<(), String> {
    if argv.is_empty() {
        return Ok(());
    }
    if argv.len() > 32 {
        return Err(format!("{field}_too_many_args"));
    }
    if !argv[0].starts_with('/') {
        return Err(format!("{field}_executable_must_be_absolute"));
    }
    for part in argv {
        if part.is_empty() || part.len() > 4096 || part.as_bytes().contains(&0) {
            return Err(format!("{field}_invalid_arg"));
        }
    }
    Ok(())
}

fn validate_telemetry_interval(value: u64, field: &str) -> Result<(), String> {
    if !(5..=3600).contains(&value) {
        return Err(format!("{field}_out_of_range"));
    }
    Ok(())
}

fn validate_tags(tags: &[String]) -> Result<(), String> {
    if tags.len() > 64 {
        return Err("tags_too_many".to_string());
    }
    for tag in tags {
        validate_identifier(tag, "tag", 64)?;
    }
    Ok(())
}

fn validate_absolute_config_path(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty() || !value.starts_with('/') || value.as_bytes().contains(&0) {
        return Err(format!("{field}_must_be_absolute"));
    }
    if value
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return Err(format!("{field}_must_not_contain_dot_segments"));
    }
    Ok(())
}

fn validate_required_hex32(value: Option<&str>, field: &str) -> Result<(), String> {
    let value = value.ok_or_else(|| format!("{field}_required"))?;
    validate_hex32(value, field)
}

fn validate_hex32(value: &str, field: &str) -> Result<(), String> {
    if value.len() != 64 {
        return Err(format!("{field}_must_be_32_byte_hex"));
    }
    let decoded = hex::decode(value).map_err(|_| format!("{field}_invalid_hex"))?;
    if decoded.len() != 32 {
        return Err(format!("{field}_must_be_32_byte_hex"));
    }
    Ok(())
}
