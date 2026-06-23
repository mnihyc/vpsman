use std::{collections::BTreeMap, fs, path::PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::commands_schedules::selector_expression_from_targets;
use crate::http::{http_get, http_post_json};
use crate::jobs::resolve_target_ids;
use crate::privilege::{
    build_privilege_for_db, load_super_password, load_super_salt_hex, DbPrivilegeRequest,
};
use crate::util::percent_encode_query_value;

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

pub(crate) struct VpsRulesListOptions {
    pub(crate) limit: u16,
    pub(crate) selector: Option<String>,
    pub(crate) client_id: Option<String>,
    pub(crate) key: Option<String>,
    pub(crate) state: Option<String>,
}

pub(crate) fn vps_rules_list(
    api_url: &str,
    token: Option<&str>,
    options: VpsRulesListOptions,
) -> Result<()> {
    let path = vps_rules_path(
        options.limit,
        options.selector.as_deref(),
        options.client_id.as_deref(),
        options.key.as_deref(),
        options.state.as_deref(),
    )?;
    println!("{}", http_get(api_url, &path, token)?);
    Ok(())
}

pub(crate) fn vps_rules_get(api_url: &str, token: Option<&str>, client_id: String) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!(
                "/api/v1/vps-rules/effective/{}",
                percent_encode_query_value(&client_id)
            ),
            token,
        )?
    );
    Ok(())
}

pub(crate) struct VpsRulesPreviewOptions {
    pub(crate) selector: String,
    pub(crate) set_values: Vec<String>,
}

pub(crate) fn vps_rules_preview(
    api_url: &str,
    token: Option<&str>,
    options: VpsRulesPreviewOptions,
) -> Result<()> {
    let values = parse_key_value_args(&options.set_values)?;
    let preview = vps_rules_dry_run(
        api_url,
        token,
        "upsert",
        &options.selector,
        values,
        Vec::new(),
    )?;
    println!("{}", serde_json::to_string_pretty(&preview)?);
    Ok(())
}

pub(crate) struct VpsRulesUpsertOptions {
    pub(crate) selector: String,
    pub(crate) set_values: Vec<String>,
    pub(crate) confirmed: bool,
}

pub(crate) fn vps_rules_upsert(
    api_url: &str,
    token: Option<&str>,
    options: VpsRulesUpsertOptions,
) -> Result<()> {
    let values = parse_key_value_args(&options.set_values)?;
    let preview = vps_rules_dry_run(
        api_url,
        token,
        "upsert",
        &options.selector,
        values.clone(),
        Vec::new(),
    )?;
    if !options.confirmed {
        println!("{}", serde_json::to_string_pretty(&preview)?);
        return Ok(());
    }
    let preview_hash = preview_hash_from_value(&preview)?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/vps-rules/bulk-upsert",
            token,
            &json!({
                "selector_expression": options.selector,
                "values": values,
                "confirmed": true,
                "preview_hash": preview_hash,
            }),
        )?
    );
    Ok(())
}

pub(crate) struct VpsRulesUnsetOptions {
    pub(crate) selector: String,
    pub(crate) keys: Vec<String>,
    pub(crate) confirmed: bool,
}

pub(crate) fn vps_rules_unset(
    api_url: &str,
    token: Option<&str>,
    options: VpsRulesUnsetOptions,
) -> Result<()> {
    anyhow::ensure!(
        !options.keys.is_empty(),
        "vps-rules unset requires at least one --key"
    );
    let preview = vps_rules_dry_run(
        api_url,
        token,
        "unset",
        &options.selector,
        BTreeMap::new(),
        options.keys.clone(),
    )?;
    if !options.confirmed {
        println!("{}", serde_json::to_string_pretty(&preview)?);
        return Ok(());
    }
    let preview_hash = preview_hash_from_value(&preview)?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/vps-rules/bulk-unset",
            token,
            &json!({
                "selector_expression": options.selector,
                "keys": options.keys,
                "confirmed": true,
                "preview_hash": preview_hash,
            }),
        )?
    );
    Ok(())
}

pub(crate) struct AlertPoliciesListOptions {
    pub(crate) limit: u16,
    pub(crate) enabled: Option<bool>,
    pub(crate) selector: Option<String>,
    pub(crate) client_id: Option<String>,
}

pub(crate) fn alert_policies_list(
    api_url: &str,
    token: Option<&str>,
    options: AlertPoliciesListOptions,
) -> Result<()> {
    let path = alert_policies_path(
        options.limit,
        options.enabled,
        options.selector.as_deref(),
        options.client_id.as_deref(),
    )?;
    println!("{}", http_get(api_url, &path, token)?);
    Ok(())
}

pub(crate) fn alert_policy_get(api_url: &str, token: Option<&str>, name: String) -> Result<()> {
    let path = alert_policies_path(1000, None, None, None)?;
    let policies: Value = serde_json::from_str(&http_get(api_url, &path, token)?)?;
    let policy = policies
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("name").and_then(Value::as_str) == Some(name.as_str()))
        })
        .cloned()
        .with_context(|| format!("alert policy not found: {name}"))?;
    println!("{}", serde_json::to_string_pretty(&policy)?);
    Ok(())
}

pub(crate) struct AlertPolicyWriteOptions {
    pub(crate) name: String,
    pub(crate) selector: Option<String>,
    pub(crate) rules: Vec<String>,
    pub(crate) window_secs: i64,
    pub(crate) severity: String,
    pub(crate) traffic_selector: Option<String>,
    pub(crate) enabled: bool,
    pub(crate) notes: Option<String>,
    pub(crate) file: Option<PathBuf>,
    pub(crate) confirmed: bool,
}

pub(crate) fn alert_policy_preview(
    api_url: &str,
    token: Option<&str>,
    options: AlertPolicyWriteOptions,
) -> Result<()> {
    let request = alert_policy_request(options, None)?;
    let preview = alert_policy_dry_run(api_url, token, &request)?;
    println!("{}", serde_json::to_string_pretty(&preview)?);
    Ok(())
}

pub(crate) fn alert_policy_upsert(
    api_url: &str,
    token: Option<&str>,
    options: AlertPolicyWriteOptions,
) -> Result<()> {
    let mut request = alert_policy_request(options, None)?;
    let preview = alert_policy_dry_run(api_url, token, &request)?;
    if !request
        .get("confirmed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        println!("{}", serde_json::to_string_pretty(&preview)?);
        return Ok(());
    }
    let preview_hash = preview_hash_from_value(&preview)?;
    request["preview_hash"] = Value::String(preview_hash);
    println!(
        "{}",
        http_post_json(api_url, "/api/v1/fleet-alert-policies", token, &request)?
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

pub(crate) fn source_templates(
    api_url: &str,
    token: Option<&str>,
    domain: Option<String>,
) -> Result<()> {
    let mut path = "/api/v1/source-templates".to_string();
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

pub(crate) fn source_status(
    api_url: &str,
    token: Option<&str>,
    client_id: Option<String>,
    domain: Option<String>,
) -> Result<()> {
    let path = source_status_path(client_id.as_deref(), domain.as_deref())?;
    println!("{}", http_get(api_url, &path, token)?);
    Ok(())
}

pub(crate) struct SourceTemplateCreateOptions {
    pub(crate) domain: String,
    pub(crate) name: String,
    pub(crate) scope: String,
    pub(crate) owner_client_id: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) definition_json: Option<String>,
    pub(crate) definition_file: Option<PathBuf>,
}

pub(crate) fn source_template_create(
    api_url: &str,
    token: Option<&str>,
    options: SourceTemplateCreateOptions,
) -> Result<()> {
    let definition = template_definition(options.definition_json, options.definition_file)?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/source-templates",
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

pub(crate) fn source_template_clone(
    api_url: &str,
    token: Option<&str>,
    source_template_id: String,
    name: String,
    scope: String,
    owner_client_id: Option<String>,
    description: Option<String>,
) -> Result<()> {
    let source_template_id =
        Uuid::parse_str(&source_template_id).context("invalid --template-id UUID")?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/source-templates/{source_template_id}/clone"),
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

pub(crate) fn source_template_diff(
    api_url: &str,
    token: Option<&str>,
    template_id: String,
    description: Option<String>,
    clear_description: bool,
    definition_json: Option<String>,
    definition_file: Option<PathBuf>,
) -> Result<()> {
    let template_id = Uuid::parse_str(&template_id).context("invalid --template-id UUID")?;
    let definition = template_definition(definition_json, definition_file)?;
    let keep_description = description.is_none() && !clear_description;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/source-templates/{template_id}/diff"),
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

pub(crate) fn source_template_test(
    api_url: &str,
    token: Option<&str>,
    template_id: String,
    definition_json: Option<String>,
    definition_file: Option<PathBuf>,
) -> Result<()> {
    let template_id = Uuid::parse_str(&template_id).context("invalid --template-id UUID")?;
    let definition = template_definition(definition_json, definition_file)?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/source-templates/{template_id}/test"),
            token,
            &serde_json::json!({
                "definition": definition,
            }),
        )?
    );
    Ok(())
}

pub(crate) struct SourceTemplateUpdateOptions {
    pub(crate) template_id: String,
    pub(crate) description: Option<String>,
    pub(crate) clear_description: bool,
    pub(crate) definition_json: Option<String>,
    pub(crate) definition_file: Option<PathBuf>,
    pub(crate) confirmed: bool,
}

pub(crate) fn source_template_update(
    api_url: &str,
    token: Option<&str>,
    options: SourceTemplateUpdateOptions,
) -> Result<()> {
    let template_id =
        Uuid::parse_str(&options.template_id).context("invalid --template-id UUID")?;
    let definition = template_definition(options.definition_json, options.definition_file)?;
    let keep_description = options.description.is_none() && !options.clear_description;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/source-templates/{template_id}/update"),
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

pub(crate) fn source_template_assignments(
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
        "/api/v1/source-template-assignments".to_string()
    } else {
        format!("/api/v1/source-template-assignments?{}", query.join("&"))
    };
    println!("{}", http_get(api_url, &path, token)?);
    Ok(())
}

pub(crate) fn template_runtime_config(
    api_url: &str,
    token: Option<&str>,
    client_id: String,
    format: String,
) -> Result<()> {
    anyhow::ensure!(
        !client_id.is_empty() && client_id.len() <= 128,
        "--client-id must be between 1 and 128 bytes"
    );
    let path = template_runtime_config_path(&client_id);
    let body = http_get(api_url, &path, token)?;
    match format.as_str() {
        "json" => println!("{body}"),
        "toml" => {
            let value: serde_json::Value =
                serde_json::from_str(&body).context("invalid source template config response")?;
            let toml = value
                .get("toml")
                .and_then(serde_json::Value::as_str)
                .context("source template config response missing toml")?;
            print!("{toml}");
        }
        _ => anyhow::bail!("--format must be toml or json"),
    }
    Ok(())
}

pub(crate) struct SourceTemplateAssignOptions {
    pub(crate) domain: String,
    pub(crate) template_id: String,
    pub(crate) clients: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) confirmed: bool,
}

fn template_runtime_config_path(client_id: &str) -> String {
    format!(
        "/api/v1/template-runtime-config?client_id={}",
        percent_encode_query_value(client_id)
    )
}

fn source_status_path(client_id: Option<&str>, domain: Option<&str>) -> Result<String> {
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
        Ok("/api/v1/source-status".to_string())
    } else {
        Ok(format!("/api/v1/source-status?{}", query.join("&")))
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

fn vps_rules_path(
    limit: u16,
    selector: Option<&str>,
    client_id: Option<&str>,
    key: Option<&str>,
    state: Option<&str>,
) -> Result<String> {
    anyhow::ensure!(
        (1..=1000).contains(&limit),
        "--limit must be between 1 and 1000"
    );
    let mut query = vec![format!("limit={limit}")];
    if let Some(selector) = selector {
        query.push(format!(
            "selector_expression={}",
            percent_encode_query_value(selector)
        ));
    }
    if let Some(client_id) = client_id {
        query.push(format!(
            "client_id={}",
            percent_encode_query_value(client_id)
        ));
    }
    if let Some(key) = key {
        query.push(format!("key={}", percent_encode_query_value(key)));
    }
    if let Some(state) = state {
        query.push(format!("state={}", percent_encode_query_value(state)));
    }
    Ok(format!("/api/v1/vps-rules?{}", query.join("&")))
}

fn alert_policies_path(
    limit: u16,
    enabled: Option<bool>,
    selector: Option<&str>,
    client_id: Option<&str>,
) -> Result<String> {
    anyhow::ensure!(
        (1..=1000).contains(&limit),
        "--limit must be between 1 and 1000"
    );
    let mut query = vec![format!("limit={limit}")];
    if let Some(enabled) = enabled {
        query.push(format!("enabled={enabled}"));
    }
    if let Some(selector) = selector {
        query.push(format!(
            "selector_expression={}",
            percent_encode_query_value(selector)
        ));
    }
    if let Some(client_id) = client_id {
        query.push(format!(
            "client_id={}",
            percent_encode_query_value(client_id)
        ));
    }
    Ok(format!("/api/v1/fleet-alert-policies?{}", query.join("&")))
}

pub(crate) fn vps_rules_dry_run(
    api_url: &str,
    token: Option<&str>,
    operation: &str,
    selector: &str,
    values: BTreeMap<String, String>,
    keys: Vec<String>,
) -> Result<Value> {
    anyhow::ensure!(
        !selector.trim().is_empty(),
        "vps-rules dry-run requires --selector"
    );
    let response = http_post_json(
        api_url,
        "/api/v1/vps-rules/dry-run",
        token,
        &json!({
            "operation": operation,
            "selector_expression": selector,
            "values": values,
            "keys": keys,
        }),
    )?;
    serde_json::from_str(&response).context("invalid VPS rules dry-run response")
}

pub(crate) fn parse_key_value_args(values: &[String]) -> Result<BTreeMap<String, String>> {
    anyhow::ensure!(
        !values.is_empty(),
        "at least one --set key=value is required"
    );
    let mut parsed = BTreeMap::new();
    for value in values {
        let (key, raw_value) = value
            .split_once('=')
            .with_context(|| format!("--set must be key=value, got {value}"))?;
        anyhow::ensure!(!key.trim().is_empty(), "--set key must not be empty");
        anyhow::ensure!(
            !raw_value.trim().is_empty(),
            "--set value must not be empty"
        );
        anyhow::ensure!(
            parsed
                .insert(key.trim().to_string(), raw_value.trim().to_string())
                .is_none(),
            "duplicate --set key: {}",
            key.trim()
        );
    }
    Ok(parsed)
}

pub(crate) fn preview_hash_from_value(value: &Value) -> Result<String> {
    value
        .get("preview_hash")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .context("dry-run response missing preview_hash")
}

pub(crate) fn alert_policy_dry_run(
    api_url: &str,
    token: Option<&str>,
    request: &Value,
) -> Result<Value> {
    let mut dry_run_request = request.clone();
    if let Some(object) = dry_run_request.as_object_mut() {
        object.remove("confirmed");
        object.remove("preview_hash");
    }
    let response = http_post_json(
        api_url,
        "/api/v1/fleet-alert-policies/dry-run",
        token,
        &dry_run_request,
    )?;
    serde_json::from_str(&response).context("invalid alert policy dry-run response")
}

pub(crate) fn alert_policy_request(
    options: AlertPolicyWriteOptions,
    id: Option<Uuid>,
) -> Result<Value> {
    if let Some(file) = options.file {
        anyhow::ensure!(
            options.selector.is_none() && options.rules.is_empty(),
            "--file cannot be combined with --selector or --rule"
        );
        let mut value: Value = serde_json::from_str(
            &fs::read_to_string(&file)
                .with_context(|| format!("failed to read {}", file.display()))?,
        )
        .with_context(|| format!("invalid JSON policy file {}", file.display()))?;
        if let Some(object) = value.as_object_mut() {
            object
                .entry("confirmed".to_string())
                .or_insert(Value::Bool(options.confirmed));
            if let Some(id) = id {
                object.entry("id".to_string()).or_insert(json!(id));
            }
        } else {
            anyhow::bail!("policy file root must be a JSON object");
        }
        return Ok(value);
    }
    let selector = options
        .selector
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("--selector is required without --file")?;
    anyhow::ensure!(
        !options.rules.is_empty(),
        "at least one --rule expression is required without --file"
    );
    let rules = options
        .rules
        .iter()
        .enumerate()
        .map(|(index, expression)| {
            policy_rule_from_expression(
                expression,
                index,
                options.window_secs,
                &options.severity,
                options.traffic_selector.as_deref(),
            )
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({
        "id": id,
        "name": options.name,
        "enabled": options.enabled,
        "selector_expression": selector,
        "rules": rules,
        "notes": options.notes,
        "confirmed": options.confirmed,
    }))
}

fn policy_rule_from_expression(
    expression: &str,
    index: usize,
    window_secs: i64,
    severity: &str,
    traffic_selector: Option<&str>,
) -> Result<Value> {
    let expression = expression.trim();
    anyhow::ensure!(
        !expression.is_empty(),
        "--rule condition expression must not be empty"
    );
    Ok(json!({
        "name": format!("rule-{}", index + 1),
        "enabled": true,
        "traffic_selector": traffic_selector
            .map(str::trim)
            .filter(|value| !value.is_empty()),
        "condition_expression": expression,
        "window_secs": window_secs,
        "severity": severity,
    }))
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

pub(crate) fn source_template_assign(
    api_url: &str,
    token: Option<&str>,
    options: SourceTemplateAssignOptions,
) -> Result<()> {
    let template_id =
        Uuid::parse_str(&options.template_id).context("invalid --template-id UUID")?;
    let selector_expression = selector_expression_from_targets(&options.clients, &options.tags);
    let target_client_ids = resolve_target_ids(api_url, token, &options.clients, &options.tags)?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/source-template-assignments",
            token,
            &serde_json::json!({
                "domain": options.domain,
                "template_id": template_id,
                "selector_expression": selector_expression,
                "target_client_ids": target_client_ids,
                "confirmed": options.confirmed,
            }),
        )?
    );
    Ok(())
}

fn template_definition(
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
