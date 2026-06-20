use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;
use vpsman_common::{
    validate_incremental_config_patch_section, JobCommand,
    DATA_SOURCE_CONFIG_APPLY_MODE_INCREMENTAL_PATCH, MAX_AGENT_HOT_CONFIG_BYTES,
};

use crate::commands_schedules::selector_expression_from_targets;
use crate::http::{http_get, http_post_json};
use crate::jobs::{resolve_target_ids, submit_privileged_operation, PrivilegedOperationRequest};
use crate::privilege::{
    build_privilege_for_db, load_super_password, load_super_salt_hex, DbPrivilegeRequest,
};
use crate::util::percent_encode_query_value;

#[derive(Debug, Deserialize)]
struct DataSourceHotConfigResponse {
    toml: String,
}

pub(crate) fn summary(api_url: &str, token: Option<&str>) -> Result<()> {
    println!("{}", http_get(api_url, "/api/v1/fleet/summary", token)?);
    Ok(())
}

pub(crate) fn agents(api_url: &str, token: Option<&str>) -> Result<()> {
    println!("{}", http_get(api_url, "/api/v1/agents", token)?);
    Ok(())
}

pub(crate) struct FleetAlertFilterOptions {
    pub(crate) limit: u16,
    pub(crate) client_id: Option<String>,
    pub(crate) severity: Option<String>,
    pub(crate) category: Option<String>,
    pub(crate) operator_state: Option<String>,
    pub(crate) include_muted: bool,
}

pub(crate) fn fleet_alerts(
    api_url: &str,
    token: Option<&str>,
    options: FleetAlertFilterOptions,
) -> Result<()> {
    let path = fleet_alerts_path(
        options.limit,
        options.client_id.as_deref(),
        options.severity.as_deref(),
        options.category.as_deref(),
        options.operator_state.as_deref(),
        options.include_muted,
    )?;
    println!("{}", http_get(api_url, &path, token)?);
    Ok(())
}

pub(crate) fn fleet_alert_export(
    api_url: &str,
    token: Option<&str>,
    options: FleetAlertFilterOptions,
) -> Result<()> {
    let mut path = fleet_alerts_path(
        options.limit,
        options.client_id.as_deref(),
        options.severity.as_deref(),
        options.category.as_deref(),
        options.operator_state.as_deref(),
        options.include_muted,
    )?;
    path = path.replacen("/api/v1/fleet-alerts?", "/api/v1/fleet-alerts/export?", 1);
    println!("{}", http_get(api_url, &path, token)?);
    Ok(())
}

pub(crate) fn fleet_alert_states(
    api_url: &str,
    token: Option<&str>,
    limit: u16,
    state: Option<String>,
) -> Result<()> {
    let path = fleet_alert_states_path(limit, state.as_deref())?;
    println!("{}", http_get(api_url, &path, token)?);
    Ok(())
}

pub(crate) struct FleetAlertStateUpdateOptions {
    pub(crate) alert_id: String,
    pub(crate) action: String,
    pub(crate) muted_for_secs: Option<i64>,
    pub(crate) reason: Option<String>,
    pub(crate) confirmed: bool,
}

pub(crate) fn fleet_alert_state_update(
    api_url: &str,
    token: Option<&str>,
    options: FleetAlertStateUpdateOptions,
) -> Result<()> {
    validate_fleet_alert_state_update(&options)?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/fleet-alert-states",
            token,
            &json!({
                "alert_id": options.alert_id,
                "action": options.action,
                "muted_for_secs": options.muted_for_secs,
                "reason": options.reason,
                "confirmed": options.confirmed,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn fleet_alert_policies(
    api_url: &str,
    token: Option<&str>,
    limit: u16,
    enabled: Option<bool>,
    scope_kind: Option<String>,
    scope_value: Option<String>,
) -> Result<()> {
    let path = fleet_alert_policies_path(
        limit,
        enabled,
        scope_kind.as_deref(),
        scope_value.as_deref(),
    )?;
    println!("{}", http_get(api_url, &path, token)?);
    Ok(())
}

pub(crate) struct FleetAlertPolicyUpsertOptions {
    pub(crate) name: String,
    pub(crate) scope_kind: String,
    pub(crate) scope_value: Option<String>,
    pub(crate) memory_available_warning_ratio: Option<f64>,
    pub(crate) memory_available_critical_ratio: Option<f64>,
    pub(crate) disk_available_warning_ratio: Option<f64>,
    pub(crate) disk_available_critical_ratio: Option<f64>,
    pub(crate) cpu_load_warning: Option<f64>,
    pub(crate) cpu_load_critical: Option<f64>,
    pub(crate) priority: i32,
    pub(crate) enabled: bool,
    pub(crate) notes: Option<String>,
    pub(crate) confirmed: bool,
}

pub(crate) fn fleet_alert_policy_upsert(
    api_url: &str,
    token: Option<&str>,
    options: FleetAlertPolicyUpsertOptions,
) -> Result<()> {
    validate_fleet_alert_policy_upsert(&options)?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/fleet-alert-policies",
            token,
            &json!({
                "name": options.name,
                "scope_kind": options.scope_kind,
                "scope_value": options.scope_value,
                "memory_available_warning_ratio": options.memory_available_warning_ratio,
                "memory_available_critical_ratio": options.memory_available_critical_ratio,
                "disk_available_warning_ratio": options.disk_available_warning_ratio,
                "disk_available_critical_ratio": options.disk_available_critical_ratio,
                "cpu_load_warning": options.cpu_load_warning,
                "cpu_load_critical": options.cpu_load_critical,
                "priority": options.priority,
                "enabled": options.enabled,
                "notes": options.notes,
                "confirmed": options.confirmed,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn fleet_alert_notification_channels(
    api_url: &str,
    token: Option<&str>,
    limit: u16,
    enabled: Option<bool>,
    scope_kind: Option<String>,
    scope_value: Option<String>,
    delivery_kind: Option<String>,
) -> Result<()> {
    let path = fleet_alert_notification_channels_path(
        limit,
        enabled,
        scope_kind.as_deref(),
        scope_value.as_deref(),
        delivery_kind.as_deref(),
    )?;
    println!("{}", http_get(api_url, &path, token)?);
    Ok(())
}

pub(crate) struct FleetAlertNotificationChannelUpsertOptions {
    pub(crate) name: String,
    pub(crate) scope_kind: String,
    pub(crate) scope_value: Option<String>,
    pub(crate) min_severity: Option<String>,
    pub(crate) categories: Vec<String>,
    pub(crate) operator_states: Vec<String>,
    pub(crate) delivery_kind: String,
    pub(crate) target: String,
    pub(crate) cooldown_secs: Option<i64>,
    pub(crate) enabled: bool,
    pub(crate) notes: Option<String>,
    pub(crate) confirmed: bool,
}

pub(crate) fn fleet_alert_notification_channel_upsert(
    api_url: &str,
    token: Option<&str>,
    options: FleetAlertNotificationChannelUpsertOptions,
) -> Result<()> {
    validate_fleet_alert_notification_channel_upsert(&options)?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/fleet-alert-notification-channels",
            token,
            &json!({
                "name": options.name,
                "scope_kind": options.scope_kind,
                "scope_value": options.scope_value,
                "min_severity": options.min_severity,
                "categories": options.categories,
                "operator_states": options.operator_states,
                "delivery_kind": options.delivery_kind,
                "target": options.target,
                "cooldown_secs": options.cooldown_secs,
                "enabled": options.enabled,
                "notes": options.notes,
                "confirmed": options.confirmed,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn fleet_alert_notifications(
    api_url: &str,
    token: Option<&str>,
    limit: u16,
    channel_id: Option<String>,
    alert_id: Option<String>,
    status: Option<String>,
) -> Result<()> {
    let path = fleet_alert_notifications_path(
        limit,
        channel_id.as_deref(),
        alert_id.as_deref(),
        status.as_deref(),
    )?;
    println!("{}", http_get(api_url, &path, token)?);
    Ok(())
}

pub(crate) struct FleetAlertNotificationDispatchOptions {
    pub(crate) limit: u16,
    pub(crate) client_id: Option<String>,
    pub(crate) severity: Option<String>,
    pub(crate) category: Option<String>,
    pub(crate) operator_state: Option<String>,
    pub(crate) include_muted: bool,
    pub(crate) dry_run: bool,
    pub(crate) confirmed: bool,
}

pub(crate) fn fleet_alert_notification_dispatch(
    api_url: &str,
    token: Option<&str>,
    options: FleetAlertNotificationDispatchOptions,
) -> Result<()> {
    validate_fleet_alert_notification_dispatch(&options)?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/fleet-alert-notifications/dispatch",
            token,
            &json!({
                "limit": options.limit,
                "client_id": options.client_id,
                "severity": options.severity,
                "category": options.category,
                "operator_state": options.operator_state,
                "include_muted": options.include_muted,
                "dry_run": options.dry_run,
                "confirmed": options.confirmed,
            }),
        )?
    );
    Ok(())
}

pub(crate) struct FleetAlertNotificationProcessOptions {
    pub(crate) limit: u16,
    pub(crate) status: Option<String>,
    pub(crate) delivery_kind: Option<String>,
    pub(crate) dry_run: bool,
    pub(crate) confirmed: bool,
}

pub(crate) fn fleet_alert_notification_process(
    api_url: &str,
    token: Option<&str>,
    options: FleetAlertNotificationProcessOptions,
) -> Result<()> {
    validate_fleet_alert_notification_process(&options)?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/fleet-alert-notifications/process",
            token,
            &json!({
                "limit": options.limit,
                "status": options.status,
                "delivery_kind": options.delivery_kind,
                "dry_run": options.dry_run,
                "confirmed": options.confirmed,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn gateway_sessions(api_url: &str, token: Option<&str>, limit: u16) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!("/api/v1/gateway-sessions?limit={}", limit.clamp(1, 200)),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn telemetry_rollups(
    api_url: &str,
    token: Option<&str>,
    limit: u16,
    client_id: Option<String>,
    bucket_secs: Option<i32>,
) -> Result<()> {
    if let Some(bucket_secs) = bucket_secs {
        anyhow::ensure!(
            (60..=86_400).contains(&bucket_secs),
            "--bucket-secs must be between 60 and 86400"
        );
    }
    let mut path = format!("/api/v1/telemetry/rollups?limit={}", limit.clamp(1, 200));
    if let Some(client_id) = client_id {
        anyhow::ensure!(
            !client_id.is_empty() && client_id.len() <= 128,
            "--client-id must be between 1 and 128 bytes"
        );
        path.push_str("&client_id=");
        path.push_str(&percent_encode_query_value(&client_id));
    }
    if let Some(bucket_secs) = bucket_secs {
        path.push_str("&bucket_secs=");
        path.push_str(&bucket_secs.to_string());
    }
    println!("{}", http_get(api_url, &path, token)?);
    Ok(())
}

pub(crate) fn telemetry_network_rates(
    api_url: &str,
    token: Option<&str>,
    limit: u16,
    client_id: Option<String>,
    interface: Option<String>,
    bucket_secs: Option<i32>,
) -> Result<()> {
    if let Some(bucket_secs) = bucket_secs {
        anyhow::ensure!(
            (60..=86_400).contains(&bucket_secs),
            "--bucket-secs must be between 60 and 86400"
        );
    }
    let mut path = format!(
        "/api/v1/telemetry/network-rates?limit={}",
        limit.clamp(1, 5_000)
    );
    if let Some(client_id) = client_id {
        anyhow::ensure!(
            !client_id.is_empty() && client_id.len() <= 128,
            "--client-id must be between 1 and 128 bytes"
        );
        path.push_str("&client_id=");
        path.push_str(&percent_encode_query_value(&client_id));
    }
    if let Some(interface) = interface {
        anyhow::ensure!(
            !interface.is_empty() && interface.len() <= 64,
            "--interface must be between 1 and 64 bytes"
        );
        path.push_str("&interface=");
        path.push_str(&percent_encode_query_value(&interface));
    }
    if let Some(bucket_secs) = bucket_secs {
        path.push_str("&bucket_secs=");
        path.push_str(&bucket_secs.to_string());
    }
    println!("{}", http_get(api_url, &path, token)?);
    Ok(())
}

pub(crate) fn telemetry_tunnels(
    api_url: &str,
    token: Option<&str>,
    limit: u16,
    client_id: Option<String>,
    interface: Option<String>,
) -> Result<()> {
    let mut path = format!("/api/v1/telemetry/tunnels?limit={}", limit.clamp(1, 200));
    if let Some(client_id) = client_id {
        anyhow::ensure!(
            !client_id.is_empty() && client_id.len() <= 128,
            "--client-id must be between 1 and 128 bytes"
        );
        path.push_str("&client_id=");
        path.push_str(&percent_encode_query_value(&client_id));
    }
    if let Some(interface) = interface {
        anyhow::ensure!(
            !interface.is_empty() && interface.len() <= 64,
            "--interface must be between 1 and 64 bytes"
        );
        path.push_str("&interface=");
        path.push_str(&percent_encode_query_value(&interface));
    }
    println!("{}", http_get(api_url, &path, token)?);
    Ok(())
}

pub(crate) fn tags(api_url: &str, token: Option<&str>) -> Result<()> {
    println!("{}", http_get(api_url, "/api/v1/tags", token)?);
    Ok(())
}

pub(crate) fn tag_create(
    api_url: &str,
    token: Option<&str>,
    name: String,
    confirmed: bool,
) -> Result<()> {
    anyhow::ensure!(confirmed, "tag-create requires --confirmed");
    let password = load_super_password("VPSMAN_SUPER_PASSWORD")?;
    let salt_hex = load_super_salt_hex(None)?;
    let privilege_assertion = build_privilege_for_db(
        DbPrivilegeRequest {
            action: "tag.create",
            target: &name,
            selector_expression: None,
            resolved_targets: &[],
            confirmed,
            payload_hash: None,
        },
        &password,
        &salt_hex,
        300,
    )?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/tags",
            token,
            &serde_json::json!({
                "name": name,
                "confirmed": confirmed,
                "privilege_assertion": privilege_assertion,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn agent_tag(
    api_url: &str,
    token: Option<&str>,
    client_id: String,
    tag: String,
    confirmed: bool,
) -> Result<()> {
    anyhow::ensure!(confirmed, "agent-tag requires --confirmed");
    let password = load_super_password("VPSMAN_SUPER_PASSWORD")?;
    let salt_hex = load_super_salt_hex(None)?;
    let targets = vec![client_id.clone()];
    let privilege_assertion = build_privilege_for_db(
        DbPrivilegeRequest {
            action: "tag.assign",
            target: &tag,
            selector_expression: None,
            resolved_targets: &targets,
            confirmed,
            payload_hash: None,
        },
        &password,
        &salt_hex,
        300,
    )?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/agents/{client_id}/tags"),
            token,
            &serde_json::json!({
                "tag": tag,
                "confirmed": confirmed,
                "privilege_assertion": privilege_assertion,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn bulk_resolve(
    api_url: &str,
    token: Option<&str>,
    clients: Vec<String>,
    tags: Vec<String>,
) -> Result<()> {
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/bulk/resolve",
            token,
            &serde_json::json!({
                "selector_expression": selector_expression_from_targets(&clients, &tags),
            }),
        )?
    );
    Ok(())
}

pub(crate) fn data_source_presets(
    api_url: &str,
    token: Option<&str>,
    domain: Option<String>,
) -> Result<()> {
    let mut path = "/api/v1/data-source-presets".to_string();
    if let Some(domain) = domain {
        anyhow::ensure!(
            !domain.is_empty() && domain.len() <= 128,
            "--domain must be between 1 and 128 bytes"
        );
        path.push_str("?domain=");
        path.push_str(&percent_encode_query_value(&domain));
    }
    println!("{}", http_get(api_url, &path, token)?);
    Ok(())
}

pub(crate) fn data_source_status(
    api_url: &str,
    token: Option<&str>,
    client_id: Option<String>,
    domain: Option<String>,
) -> Result<()> {
    let path = data_source_status_path(client_id.as_deref(), domain.as_deref())?;
    println!("{}", http_get(api_url, &path, token)?);
    Ok(())
}

pub(crate) struct DataSourcePresetCreateOptions {
    pub(crate) domain: String,
    pub(crate) name: String,
    pub(crate) scope: String,
    pub(crate) owner_client_id: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) definition_json: Option<String>,
    pub(crate) definition_file: Option<PathBuf>,
}

pub(crate) fn data_source_preset_create(
    api_url: &str,
    token: Option<&str>,
    options: DataSourcePresetCreateOptions,
) -> Result<()> {
    let definition = preset_definition(options.definition_json, options.definition_file)?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/data-source-presets",
            token,
            &serde_json::json!({
                "domain": options.domain,
                "name": options.name,
                "scope": options.scope,
                "owner_client_id": options.owner_client_id,
                "description": options.description,
                "definition": definition,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn data_source_preset_clone(
    api_url: &str,
    token: Option<&str>,
    source_preset_id: String,
    name: String,
    scope: String,
    owner_client_id: Option<String>,
    description: Option<String>,
) -> Result<()> {
    let source_preset_id =
        Uuid::parse_str(&source_preset_id).context("invalid --source-preset-id UUID")?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/data-source-presets/{source_preset_id}/clone"),
            token,
            &serde_json::json!({
                "name": name,
                "scope": scope,
                "owner_client_id": owner_client_id,
                "description": description,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn data_source_preset_diff(
    api_url: &str,
    token: Option<&str>,
    preset_id: String,
    description: Option<String>,
    clear_description: bool,
    definition_json: Option<String>,
    definition_file: Option<PathBuf>,
) -> Result<()> {
    let preset_id = Uuid::parse_str(&preset_id).context("invalid --preset-id UUID")?;
    let definition = preset_definition(definition_json, definition_file)?;
    let keep_description = description.is_none() && !clear_description;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/data-source-presets/{preset_id}/diff"),
            token,
            &serde_json::json!({
                "description": description,
                "definition": definition,
                "keep_description": keep_description,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn data_source_preset_test(
    api_url: &str,
    token: Option<&str>,
    preset_id: String,
    definition_json: Option<String>,
    definition_file: Option<PathBuf>,
) -> Result<()> {
    let preset_id = Uuid::parse_str(&preset_id).context("invalid --preset-id UUID")?;
    let definition = preset_definition(definition_json, definition_file)?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/data-source-presets/{preset_id}/test"),
            token,
            &serde_json::json!({
                "definition": definition,
            }),
        )?
    );
    Ok(())
}

pub(crate) struct DataSourcePresetUpdateOptions {
    pub(crate) preset_id: String,
    pub(crate) description: Option<String>,
    pub(crate) clear_description: bool,
    pub(crate) definition_json: Option<String>,
    pub(crate) definition_file: Option<PathBuf>,
    pub(crate) confirmed: bool,
}

pub(crate) fn data_source_preset_update(
    api_url: &str,
    token: Option<&str>,
    options: DataSourcePresetUpdateOptions,
) -> Result<()> {
    let preset_id = Uuid::parse_str(&options.preset_id).context("invalid --preset-id UUID")?;
    let definition = preset_definition(options.definition_json, options.definition_file)?;
    let keep_description = options.description.is_none() && !options.clear_description;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/data-source-presets/{preset_id}/update"),
            token,
            &serde_json::json!({
                "description": options.description,
                "definition": definition,
                "confirmed": options.confirmed,
                "keep_description": keep_description,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn data_source_assignments(
    api_url: &str,
    token: Option<&str>,
    client_id: Option<String>,
    domain: Option<String>,
) -> Result<()> {
    let mut query = Vec::new();
    if let Some(client_id) = client_id {
        anyhow::ensure!(
            !client_id.is_empty() && client_id.len() <= 128,
            "--client-id must be between 1 and 128 bytes"
        );
        query.push(format!(
            "client_id={}",
            percent_encode_query_value(&client_id)
        ));
    }
    if let Some(domain) = domain {
        anyhow::ensure!(
            !domain.is_empty() && domain.len() <= 128,
            "--domain must be between 1 and 128 bytes"
        );
        query.push(format!("domain={}", percent_encode_query_value(&domain)));
    }
    let path = if query.is_empty() {
        "/api/v1/data-source-assignments".to_string()
    } else {
        format!("/api/v1/data-source-assignments?{}", query.join("&"))
    };
    println!("{}", http_get(api_url, &path, token)?);
    Ok(())
}

pub(crate) fn data_source_hot_config(
    api_url: &str,
    token: Option<&str>,
    client_id: String,
    format: String,
) -> Result<()> {
    anyhow::ensure!(
        !client_id.is_empty() && client_id.len() <= 128,
        "--client-id must be between 1 and 128 bytes"
    );
    let path = data_source_hot_config_path(&client_id);
    let body = http_get(api_url, &path, token)?;
    match format.as_str() {
        "json" => println!("{body}"),
        "toml" => {
            let value: serde_json::Value =
                serde_json::from_str(&body).context("invalid data-source config response")?;
            let toml = value
                .get("toml")
                .and_then(serde_json::Value::as_str)
                .context("data-source config response missing toml")?;
            print!("{toml}");
        }
        _ => anyhow::bail!("--format must be toml or json"),
    }
    Ok(())
}

pub(crate) fn data_source_hot_config_apply(
    api_url: &str,
    token: Option<&str>,
    client_id: String,
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
    force_unprivileged: bool,
) -> Result<()> {
    anyhow::ensure!(
        confirmed,
        "data-source-hot-config-apply requires --confirmed because it applies an incremental config patch"
    );
    anyhow::ensure!(
        !client_id.is_empty() && client_id.len() <= 128,
        "--client-id must be between 1 and 128 bytes"
    );
    let body = http_get(api_url, &data_source_hot_config_path(&client_id), token)?;
    let rendered: DataSourceHotConfigResponse =
        serde_json::from_str(&body).context("invalid data-source config response")?;
    validate_data_source_config_patch(&rendered.toml)?;
    let operation = JobCommand::DataSourceConfigPatch {
        apply_mode: DATA_SOURCE_CONFIG_APPLY_MODE_INCREMENTAL_PATCH.to_string(),
        toml: rendered.toml,
    };
    println!(
        "{}",
        submit_privileged_operation(PrivilegedOperationRequest {
            api_url,
            token,
            operation: &operation,
            command_label: "data_source_config_patch",
            clients: &[client_id],
            tags: &[],
            password_env: &password_env,
            super_salt_hex: super_salt_hex.as_deref(),
            privilege_ttl_secs,
            timeout_secs,
            confirmed,
            force_unprivileged,
        })?
    );
    Ok(())
}

pub(crate) struct DataSourcePresetAssignOptions {
    pub(crate) domain: String,
    pub(crate) preset_id: String,
    pub(crate) clients: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) confirmed: bool,
}

fn data_source_hot_config_path(client_id: &str) -> String {
    format!(
        "/api/v1/data-source-hot-config?client_id={}",
        percent_encode_query_value(client_id)
    )
}

fn data_source_status_path(client_id: Option<&str>, domain: Option<&str>) -> Result<String> {
    let mut query = Vec::new();
    if let Some(client_id) = client_id {
        anyhow::ensure!(
            !client_id.is_empty() && client_id.len() <= 128,
            "--client-id must be between 1 and 128 bytes"
        );
        query.push(format!(
            "client_id={}",
            percent_encode_query_value(client_id)
        ));
    }
    if let Some(domain) = domain {
        anyhow::ensure!(
            !domain.is_empty() && domain.len() <= 128,
            "--domain must be between 1 and 128 bytes"
        );
        query.push(format!("domain={}", percent_encode_query_value(domain)));
    }
    if query.is_empty() {
        Ok("/api/v1/data-source-status".to_string())
    } else {
        Ok(format!("/api/v1/data-source-status?{}", query.join("&")))
    }
}

fn fleet_alerts_path(
    limit: u16,
    client_id: Option<&str>,
    severity: Option<&str>,
    category: Option<&str>,
    operator_state: Option<&str>,
    include_muted: bool,
) -> Result<String> {
    anyhow::ensure!(
        (1..=200).contains(&limit),
        "--limit must be between 1 and 200"
    );
    if let Some(client_id) = client_id {
        anyhow::ensure!(
            !client_id.is_empty() && client_id.len() <= 128,
            "--client-id must be between 1 and 128 bytes"
        );
    }
    if let Some(severity) = severity {
        anyhow::ensure!(
            matches!(severity, "critical" | "warning" | "info"),
            "--severity must be critical, warning, or info"
        );
    }
    if let Some(category) = category {
        validate_alert_token(category, "--category")?;
    }
    if let Some(operator_state) = operator_state {
        validate_alert_state(operator_state, "--operator-state")?;
    }

    let mut query = vec![format!("limit={limit}")];
    if let Some(client_id) = client_id {
        query.push(format!(
            "client_id={}",
            percent_encode_query_value(client_id)
        ));
    }
    if let Some(severity) = severity {
        query.push(format!("severity={severity}"));
    }
    if let Some(category) = category {
        query.push(format!("category={}", percent_encode_query_value(category)));
    }
    if let Some(operator_state) = operator_state {
        query.push(format!("operator_state={operator_state}"));
    }
    if include_muted {
        query.push("include_muted=true".to_string());
    }
    Ok(format!("/api/v1/fleet-alerts?{}", query.join("&")))
}

fn fleet_alert_states_path(limit: u16, state: Option<&str>) -> Result<String> {
    anyhow::ensure!(
        (1..=1000).contains(&limit),
        "--limit must be between 1 and 1000"
    );
    if let Some(state) = state {
        validate_alert_state(state, "--state")?;
    }
    let mut query = vec![format!("limit={limit}")];
    if let Some(state) = state {
        query.push(format!("state={state}"));
    }
    Ok(format!("/api/v1/fleet-alert-states?{}", query.join("&")))
}

fn fleet_alert_policies_path(
    limit: u16,
    enabled: Option<bool>,
    scope_kind: Option<&str>,
    scope_value: Option<&str>,
) -> Result<String> {
    anyhow::ensure!(
        (1..=1000).contains(&limit),
        "--limit must be between 1 and 1000"
    );
    if let Some(scope_kind) = scope_kind {
        validate_alert_policy_scope_kind(scope_kind)?;
    }
    if let Some(scope_value) = scope_value {
        anyhow::ensure!(
            !scope_value.is_empty() && scope_value.len() <= 128,
            "--scope-value must be between 1 and 128 bytes"
        );
    }
    let mut query = vec![format!("limit={limit}")];
    if let Some(enabled) = enabled {
        query.push(format!("enabled={enabled}"));
    }
    if let Some(scope_kind) = scope_kind {
        query.push(format!("scope_kind={scope_kind}"));
    }
    if let Some(scope_value) = scope_value {
        query.push(format!(
            "scope_value={}",
            percent_encode_query_value(scope_value)
        ));
    }
    Ok(format!("/api/v1/fleet-alert-policies?{}", query.join("&")))
}

fn fleet_alert_notification_channels_path(
    limit: u16,
    enabled: Option<bool>,
    scope_kind: Option<&str>,
    scope_value: Option<&str>,
    delivery_kind: Option<&str>,
) -> Result<String> {
    anyhow::ensure!(
        (1..=1000).contains(&limit),
        "--limit must be between 1 and 1000"
    );
    if let Some(scope_kind) = scope_kind {
        validate_alert_policy_scope_kind(scope_kind)?;
    }
    if let Some(scope_value) = scope_value {
        validate_scope_value(scope_value)?;
    }
    if let Some(delivery_kind) = delivery_kind {
        validate_alert_notification_delivery_kind(delivery_kind, "--delivery-kind")?;
    }
    let mut query = vec![format!("limit={limit}")];
    if let Some(enabled) = enabled {
        query.push(format!("enabled={enabled}"));
    }
    if let Some(scope_kind) = scope_kind {
        query.push(format!("scope_kind={scope_kind}"));
    }
    if let Some(scope_value) = scope_value {
        query.push(format!(
            "scope_value={}",
            percent_encode_query_value(scope_value)
        ));
    }
    if let Some(delivery_kind) = delivery_kind {
        query.push(format!(
            "delivery_kind={}",
            percent_encode_query_value(delivery_kind)
        ));
    }
    Ok(format!(
        "/api/v1/fleet-alert-notification-channels?{}",
        query.join("&")
    ))
}

fn fleet_alert_notifications_path(
    limit: u16,
    channel_id: Option<&str>,
    alert_id: Option<&str>,
    status: Option<&str>,
) -> Result<String> {
    anyhow::ensure!(
        (1..=1000).contains(&limit),
        "--limit must be between 1 and 1000"
    );
    if let Some(channel_id) = channel_id {
        Uuid::parse_str(channel_id).context("--channel-id must be a UUID")?;
    }
    if let Some(alert_id) = alert_id {
        validate_alert_id(alert_id)?;
    }
    if let Some(status) = status {
        validate_alert_token(status, "--status")?;
    }
    let mut query = vec![format!("limit={limit}")];
    if let Some(channel_id) = channel_id {
        query.push(format!("channel_id={channel_id}"));
    }
    if let Some(alert_id) = alert_id {
        query.push(format!("alert_id={}", percent_encode_query_value(alert_id)));
    }
    if let Some(status) = status {
        query.push(format!("status={status}"));
    }
    Ok(format!(
        "/api/v1/fleet-alert-notifications?{}",
        query.join("&")
    ))
}

fn validate_fleet_alert_policy_upsert(options: &FleetAlertPolicyUpsertOptions) -> Result<()> {
    anyhow::ensure!(
        options.confirmed,
        "fleet-alert-policy-upsert requires --confirmed"
    );
    anyhow::ensure!(
        !options.name.trim().is_empty() && options.name.len() <= 128,
        "--name must be between 1 and 128 bytes"
    );
    validate_alert_policy_scope_kind(&options.scope_kind)?;
    if options.scope_kind == "global" {
        anyhow::ensure!(
            options
                .scope_value
                .as_deref()
                .is_none_or(|value| value.trim().is_empty()),
            "--scope-value must be omitted for global policies"
        );
    } else {
        anyhow::ensure!(
            options
                .scope_value
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()),
            "--scope-value is required for scoped policies"
        );
    }
    anyhow::ensure!(
        options.memory_available_warning_ratio.is_some()
            || options.memory_available_critical_ratio.is_some()
            || options.disk_available_warning_ratio.is_some()
            || options.disk_available_critical_ratio.is_some()
            || options.cpu_load_warning.is_some()
            || options.cpu_load_critical.is_some(),
        "at least one threshold must be configured"
    );
    validate_optional_ratio(
        options.memory_available_warning_ratio,
        "--memory-available-warning-ratio",
    )?;
    validate_optional_ratio(
        options.memory_available_critical_ratio,
        "--memory-available-critical-ratio",
    )?;
    validate_optional_ratio(
        options.disk_available_warning_ratio,
        "--disk-available-warning-ratio",
    )?;
    validate_optional_ratio(
        options.disk_available_critical_ratio,
        "--disk-available-critical-ratio",
    )?;
    validate_optional_positive(options.cpu_load_warning, "--cpu-load-warning")?;
    validate_optional_positive(options.cpu_load_critical, "--cpu-load-critical")?;
    Ok(())
}

fn validate_fleet_alert_notification_channel_upsert(
    options: &FleetAlertNotificationChannelUpsertOptions,
) -> Result<()> {
    anyhow::ensure!(
        options.confirmed,
        "fleet-alert-notification-channel-upsert requires --confirmed"
    );
    anyhow::ensure!(
        !options.name.trim().is_empty() && options.name.len() <= 128,
        "--name must be between 1 and 128 bytes"
    );
    validate_alert_policy_scope_kind(&options.scope_kind)?;
    if options.scope_kind == "global" {
        anyhow::ensure!(
            options
                .scope_value
                .as_deref()
                .is_none_or(|value| value.trim().is_empty()),
            "--scope-value must be omitted for global notification channels"
        );
    } else {
        validate_scope_value(
            options
                .scope_value
                .as_deref()
                .context("--scope-value is required for scoped notification channels")?,
        )?;
    }
    if let Some(min_severity) = options.min_severity.as_deref() {
        validate_alert_severity(min_severity, "--min-severity")?;
    }
    validate_alert_token_list(&options.categories, "--categories")?;
    for state in &options.operator_states {
        validate_alert_state(state, "--operator-states")?;
    }
    validate_alert_notification_delivery_kind(&options.delivery_kind, "--delivery-kind")?;
    anyhow::ensure!(
        !options.target.trim().is_empty()
            && options.target.len() <= 512
            && !options.target.as_bytes().contains(&0),
        "--target must be between 1 and 512 bytes"
    );
    if let Some(cooldown_secs) = options.cooldown_secs {
        anyhow::ensure!(
            (0..=30 * 24 * 60 * 60).contains(&cooldown_secs),
            "--cooldown-secs must be between 0 and 2592000"
        );
    }
    if let Some(notes) = options.notes.as_deref() {
        anyhow::ensure!(notes.len() <= 1024, "--notes must be at most 1024 bytes");
    }
    Ok(())
}

fn validate_fleet_alert_notification_dispatch(
    options: &FleetAlertNotificationDispatchOptions,
) -> Result<()> {
    anyhow::ensure!(
        options.dry_run || options.confirmed,
        "fleet-alert-notification-dispatch requires --confirmed unless --dry-run is set"
    );
    fleet_alerts_path(
        options.limit,
        options.client_id.as_deref(),
        options.severity.as_deref(),
        options.category.as_deref(),
        options.operator_state.as_deref(),
        options.include_muted,
    )?;
    Ok(())
}

fn validate_fleet_alert_notification_process(
    options: &FleetAlertNotificationProcessOptions,
) -> Result<()> {
    anyhow::ensure!(
        options.dry_run || options.confirmed,
        "fleet-alert-notification-process requires --confirmed unless --dry-run is set"
    );
    anyhow::ensure!(
        (1..=200).contains(&options.limit),
        "--limit must be between 1 and 200"
    );
    if let Some(status) = options.status.as_deref() {
        anyhow::ensure!(
            matches!(status, "queued" | "failed"),
            "--status must be queued or failed"
        );
    }
    if let Some(delivery_kind) = options.delivery_kind.as_deref() {
        validate_alert_notification_delivery_kind(delivery_kind, "--delivery-kind")?;
    }
    Ok(())
}

fn validate_fleet_alert_state_update(options: &FleetAlertStateUpdateOptions) -> Result<()> {
    anyhow::ensure!(
        options.confirmed,
        "fleet-alert-state-update requires --confirmed"
    );
    validate_alert_id(&options.alert_id)?;
    match options.action.as_str() {
        "acknowledge" | "escalate" | "clear" => {}
        "mute" => {
            if let Some(seconds) = options.muted_for_secs {
                anyhow::ensure!(
                    (60..=90 * 24 * 60 * 60).contains(&seconds),
                    "--muted-for-secs must be between 60 and 7776000"
                );
            }
        }
        _ => anyhow::bail!("--action must be acknowledge, mute, escalate, or clear"),
    }
    if let Some(reason) = options.reason.as_deref() {
        anyhow::ensure!(reason.len() <= 1024, "--reason must be at most 1024 bytes");
    }
    Ok(())
}

fn validate_alert_id(alert_id: &str) -> Result<()> {
    anyhow::ensure!(
        !alert_id.trim().is_empty() && alert_id.len() <= 192,
        "--alert-id must be between 1 and 192 bytes"
    );
    validate_alert_token(alert_id, "--alert-id")
}

fn validate_alert_token(value: &str, flag: &str) -> Result<()> {
    anyhow::ensure!(
        !value.trim().is_empty()
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_' | b'.')
            }),
        "{flag} contains unsupported characters"
    );
    Ok(())
}

fn validate_alert_notification_delivery_kind(value: &str, flag: &str) -> Result<()> {
    anyhow::ensure!(value.trim() == "webhook", "{flag} must be webhook");
    Ok(())
}

fn validate_alert_token_list(values: &[String], flag: &str) -> Result<()> {
    anyhow::ensure!(values.len() <= 64, "{flag} accepts at most 64 values");
    for value in values {
        validate_alert_token(value, flag)?;
    }
    Ok(())
}

fn validate_alert_severity(value: &str, flag: &str) -> Result<()> {
    anyhow::ensure!(
        matches!(value, "critical" | "warning" | "info"),
        "{flag} must be critical, warning, or info"
    );
    Ok(())
}

fn validate_scope_value(value: &str) -> Result<()> {
    anyhow::ensure!(
        !value.trim().is_empty() && value.len() <= 128,
        "--scope-value must be between 1 and 128 bytes"
    );
    Ok(())
}

fn validate_alert_state(value: &str, flag: &str) -> Result<()> {
    anyhow::ensure!(
        matches!(value, "open" | "acknowledged" | "muted" | "escalated"),
        "{flag} must be open, acknowledged, muted, or escalated"
    );
    Ok(())
}

fn validate_alert_policy_scope_kind(scope_kind: &str) -> Result<()> {
    anyhow::ensure!(
        matches!(scope_kind, "global" | "provider" | "tag" | "client"),
        "--scope-kind must be global, provider, tag, or client"
    );
    Ok(())
}

fn validate_optional_ratio(value: Option<f64>, flag: &str) -> Result<()> {
    if let Some(value) = value {
        anyhow::ensure!(
            value.is_finite() && (0.0..1.0).contains(&value),
            "{flag} must be greater than 0 and below 1"
        );
    }
    Ok(())
}

fn validate_optional_positive(value: Option<f64>, flag: &str) -> Result<()> {
    if let Some(value) = value {
        anyhow::ensure!(
            value.is_finite() && value > 0.0,
            "{flag} must be greater than 0"
        );
    }
    Ok(())
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

pub(crate) fn data_source_preset_assign(
    api_url: &str,
    token: Option<&str>,
    options: DataSourcePresetAssignOptions,
) -> Result<()> {
    let preset_id = Uuid::parse_str(&options.preset_id).context("invalid --preset-id UUID")?;
    let selector_expression = selector_expression_from_targets(&options.clients, &options.tags);
    let target_client_ids = resolve_target_ids(api_url, token, &options.clients, &options.tags)?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/data-source-assignments",
            token,
            &serde_json::json!({
                "domain": options.domain,
                "preset_id": preset_id,
                "selector_expression": selector_expression,
                "target_client_ids": target_client_ids,
                "confirmed": options.confirmed,
            }),
        )?
    );
    Ok(())
}

fn preset_definition(
    definition_json: Option<String>,
    definition_file: Option<PathBuf>,
) -> Result<serde_json::Value> {
    match (definition_json, definition_file) {
        (Some(_), Some(_)) => {
            anyhow::bail!("use only one of --definition-json or --definition-file")
        }
        (Some(value), None) => serde_json::from_str(&value).context("invalid --definition-json"),
        (None, Some(path)) => {
            let payload = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            serde_json::from_str(&payload).context("invalid --definition-file JSON")
        }
        (None, None) => Ok(serde_json::json!({})),
    }
}
