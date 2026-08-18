use anyhow::{Context, Result};

use crate::util::percent_encode_query_value;
use crate::{
    commands_inventory,
    commands_schedules::selector_expression_from_targets,
    http::{http_delete, http_get, http_post_json, http_put_json},
    privilege::{
        build_privilege_for_db, load_super_password, load_super_salt_hex, DbPrivilegeRequest,
    },
};

#[derive(Debug, PartialEq)]
enum VtyInventoryCommand {
    TagCreate {
        name: String,
        confirmed: bool,
    },
    AgentTag {
        client_id: String,
        tag: String,
        confirmed: bool,
    },
    ConfigPresets {
        behavior: Option<String>,
    },
    ConfigPresetCreate {
        behavior: String,
        name: String,
        description: Option<String>,
        definition: serde_json::Value,
    },
    ConfigPresetClone {
        preset_id: String,
        name: String,
        description: Option<String>,
    },
    ConfigPresetPreview {
        preset_id: String,
        description: Option<String>,
        clear_description: bool,
        definition: serde_json::Value,
    },
    ConfigPresetUpdate {
        preset_id: String,
        description: Option<String>,
        clear_description: bool,
        definition: serde_json::Value,
        preview_hash: Option<String>,
        confirmed: bool,
    },
    ConfigPresetDelete {
        preset_id: String,
        confirmed: bool,
    },
    ConfigSources {
        client_id: Option<String>,
        behavior: Option<String>,
    },
    FleetAlerts {
        limit: u16,
        client_id: Option<String>,
        severity: Option<String>,
        category: Option<String>,
        operator_state: Option<String>,
        include_muted: bool,
    },
    FleetAlertExport {
        limit: u16,
        client_id: Option<String>,
        severity: Option<String>,
        category: Option<String>,
        operator_state: Option<String>,
        include_muted: bool,
    },
    FleetAlertStates {
        limit: u16,
        state: Option<String>,
    },
    FleetAlertStateUpdate {
        alert_id: String,
        action: String,
        muted_for_secs: Option<i64>,
        reason: Option<String>,
        confirmed: bool,
    },
    VpsRulesList {
        limit: u16,
        selector: Option<String>,
        client_id: Option<String>,
        key: Option<String>,
        state: Option<String>,
    },
    VpsRulesGet {
        client_id: String,
    },
    VpsRulesPreview {
        selector: String,
        set_values: Vec<String>,
    },
    VpsRulesUpsert {
        selector: String,
        set_values: Vec<String>,
        confirmed: bool,
    },
    VpsRulesUnset {
        selector: String,
        keys: Vec<String>,
        confirmed: bool,
    },
    AlertPoliciesList {
        limit: u16,
        enabled: Option<bool>,
        selector: Option<String>,
        client_id: Option<String>,
    },
    AlertPolicyGet {
        name: String,
    },
    AlertPolicyPreview {
        name: String,
        selector: String,
        rule_json: Vec<String>,
        enabled: bool,
        notes: Option<String>,
    },
    AlertPolicyUpsert {
        name: String,
        selector: String,
        rule_json: Vec<String>,
        enabled: bool,
        notes: Option<String>,
        confirmed: bool,
    },
    FleetAlertNotificationChannels {
        limit: u16,
        enabled: Option<bool>,
        scope_kind: Option<String>,
        scope_value: Option<String>,
        delivery_kind: Option<String>,
    },
    FleetAlertNotificationChannelUpsert {
        name: String,
        scope_kind: String,
        scope_value: Option<String>,
        min_severity: Option<String>,
        categories: Vec<String>,
        operator_states: Vec<String>,
        delivery_kind: String,
        target: String,
        cooldown_secs: Option<i64>,
        enabled: bool,
        notes: Option<String>,
        confirmed: bool,
    },
    FleetAlertNotifications {
        limit: u16,
        channel_id: Option<String>,
        alert_id: Option<String>,
        status: Option<String>,
    },
    FleetAlertNotificationDispatch {
        limit: u16,
        client_id: Option<String>,
        severity: Option<String>,
        category: Option<String>,
        operator_state: Option<String>,
        include_muted: bool,
        dry_run: bool,
        preview_hash: Option<String>,
        confirmed: bool,
    },
    FleetAlertNotificationProcess {
        limit: u16,
        status: Option<String>,
        delivery_kind: Option<String>,
        dry_run: bool,
        preview_hash: Option<String>,
        confirmed: bool,
    },
    ConfigRender {
        client_id: String,
        format: String,
    },
    ConfigSourceChange {
        action: String,
        behavior: String,
        preset_id: Option<String>,
        selector: Option<String>,
        clients: Vec<String>,
        tags: Vec<String>,
        preview_hash: Option<String>,
        confirmed: bool,
    },
    BulkResolve {
        tags: Vec<String>,
    },
    TelemetryRollups {
        limit: u16,
        client_id: Option<String>,
        bucket_secs: Option<i32>,
        latest: bool,
    },
    TelemetryNetworkRates {
        limit: u16,
        client_id: Option<String>,
        interface: Option<String>,
        bucket_secs: Option<i32>,
        latest: bool,
    },
    TelemetryTunnels {
        limit: u16,
        client_id: Option<String>,
        interface: Option<String>,
    },
}

#[derive(Debug, Eq, PartialEq)]
struct TelemetryNetworkRateArgs {
    limit: u16,
    client_id: Option<String>,
    interface: Option<String>,
    bucket_secs: Option<i32>,
    latest: bool,
}

#[derive(Debug, Eq, PartialEq)]
struct TelemetryTunnelArgs {
    limit: u16,
    client_id: Option<String>,
    interface: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
struct FleetAlertArgs {
    limit: u16,
    client_id: Option<String>,
    severity: Option<String>,
    category: Option<String>,
    operator_state: Option<String>,
    include_muted: bool,
}

#[derive(Debug, Eq, PartialEq)]
struct FleetAlertStateListArgs {
    limit: u16,
    state: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
struct FleetAlertNotificationChannelListArgs {
    limit: u16,
    enabled: Option<bool>,
    scope_kind: Option<String>,
    scope_value: Option<String>,
    delivery_kind: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
struct FleetAlertNotificationListArgs {
    limit: u16,
    channel_id: Option<String>,
    alert_id: Option<String>,
    status: Option<String>,
}

pub(crate) fn is_vty_inventory_command(command: &str) -> bool {
    let name = command.split_whitespace().next().unwrap_or_default();
    matches!(
        name,
        "tag-create"
            | "agent-tag"
            | "config-presets"
            | "config-preset-create"
            | "config-preset-clone"
            | "config-preset-preview"
            | "config-preset-update"
            | "config-preset-delete"
            | "config-sources"
            | "config-source-set"
            | "config-source-reset"
            | "config-render"
            | "fleet-alerts"
            | "fleet-alert-export"
            | "fleet-alert-states"
            | "fleet-alert-state-update"
            | "vps-rules"
            | "vps-rules-get"
            | "vps-rules-preview"
            | "vps-rules-upsert"
            | "vps-rules-unset"
            | "alert-policies"
            | "alert-policy-get"
            | "alert-policy-preview"
            | "alert-policy-upsert"
            | "fleet-alert-notification-channels"
            | "fleet-alert-notification-channel-upsert"
            | "fleet-alert-notifications"
            | "fleet-alert-notification-dispatch"
            | "fleet-alert-notification-process"
            | "bulk-resolve"
            | "telemetry-rollups"
            | "telemetry-network-rates"
            | "telemetry-tunnels"
    )
}

pub(crate) fn is_vty_gateway_sessions_command(command: &str) -> bool {
    command == "gateway-sessions" || command.starts_with("gateway-sessions ")
}

pub(crate) fn gateway_sessions_path(command: &str) -> Result<String> {
    let mut limit = 50_u16;
    let parts = command.split_whitespace().collect::<Vec<_>>();
    anyhow::ensure!(
        parts.first() == Some(&"gateway-sessions"),
        "expected gateway-sessions command"
    );
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--limit" => {
                limit = parts
                    .get(index + 1)
                    .context("--limit requires a value")?
                    .parse()
                    .context("--limit must be an integer")?;
                index += 2;
            }
            value if value.starts_with("--limit=") => {
                limit = value
                    .trim_start_matches("--limit=")
                    .parse()
                    .context("--limit must be an integer")?;
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    anyhow::ensure!(
        (1..=200).contains(&limit),
        "gateway-sessions --limit must be between 1 and 200"
    );
    Ok(format!("/api/v1/gateway-sessions?limit={limit}"))
}

pub(crate) fn submit_vty_inventory_command(
    api_url: &str,
    token: Option<&str>,
    command: &str,
) -> Result<String> {
    match parse_vty_inventory_command(command)? {
        VtyInventoryCommand::TagCreate { name, confirmed } => {
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
            http_post_json(
                api_url,
                "/api/v1/tags",
                token,
                &serde_json::json!({
                    "name": name,
                    "confirmed": confirmed,
                    "privilege_assertion": privilege_assertion,
                }),
            )
        }
        VtyInventoryCommand::AgentTag {
            client_id,
            tag,
            confirmed,
        } => {
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
            http_post_json(
                api_url,
                &format!("/api/v1/agents/{client_id}/tags"),
                token,
                &serde_json::json!({
                    "tag": tag,
                    "confirmed": confirmed,
                    "privilege_assertion": privilege_assertion,
                }),
            )
        }
        VtyInventoryCommand::ConfigPresets { behavior } => {
            http_get(api_url, &config_presets_path(behavior.as_deref()), token)
        }
        VtyInventoryCommand::ConfigPresetCreate {
            behavior,
            name,
            description,
            definition,
        } => http_post_json(
            api_url,
            "/api/v1/configuration-presets",
            token,
            &serde_json::json!({
                "behavior": behavior,
                "name": name,
                "description": description,
                "definition": definition,
            }),
        ),
        VtyInventoryCommand::ConfigPresetClone {
            preset_id,
            name,
            description,
        } => http_post_json(
            api_url,
            &format!("/api/v1/configuration-presets/{preset_id}/clone"),
            token,
            &serde_json::json!({
                "name": name,
                "description": description,
            }),
        ),
        VtyInventoryCommand::ConfigPresetPreview {
            preset_id,
            description,
            clear_description,
            definition,
        } => {
            let preset_id = uuid::Uuid::parse_str(&preset_id).context("invalid preset UUID")?;
            let description = commands_inventory::preset_candidate_description(
                api_url,
                token,
                preset_id,
                description,
                clear_description,
            )?;
            http_post_json(
                api_url,
                &format!("/api/v1/configuration-presets/{preset_id}/preview"),
                token,
                &serde_json::json!({
                    "description": description,
                    "definition": definition,
                }),
            )
        }
        VtyInventoryCommand::ConfigPresetUpdate {
            preset_id,
            description,
            clear_description,
            definition,
            preview_hash,
            confirmed,
        } => {
            let reviewed_preview_hash = commands_inventory::reviewed_preview_hash_arg(
                confirmed,
                preview_hash.as_deref(),
                "config-preset-update",
            )?;
            let preset_id = uuid::Uuid::parse_str(&preset_id).context("invalid preset UUID")?;
            let description = commands_inventory::preset_candidate_description(
                api_url,
                token,
                preset_id,
                description,
                clear_description,
            )?;
            let preview_path = format!("/api/v1/configuration-presets/{preset_id}/preview");
            let preview_body = serde_json::json!({
                "description": description,
                "definition": definition,
            });
            let preview_raw = http_post_json(api_url, &preview_path, token, &preview_body)?;
            if !confirmed {
                return Ok(preview_raw);
            }
            let preview = commands_inventory::parse_preview_response(&preview_raw)?;
            let current_preview_hash =
                commands_inventory::required_preview_hash(&preview, "config-preset-update")?;
            let preview_hash = commands_inventory::require_matching_reviewed_preview_hash(
                reviewed_preview_hash.as_deref(),
                &current_preview_hash,
                "config-preset-update",
            )?;
            let affected_client_ids =
                commands_inventory::string_array_field(&preview, "affected_client_ids")?;
            let mut body = preview_body;
            body["preview_hash"] = serde_json::Value::String(preview_hash.clone());
            if !affected_client_ids.is_empty() {
                let password = load_super_password("VPSMAN_SUPER_PASSWORD")?;
                let salt_hex = load_super_salt_hex(None)?;
                let target = commands_inventory::config_preset_privilege_target(preset_id);
                let privilege_assertion = build_privilege_for_db(
                    DbPrivilegeRequest {
                        action: "configuration_preset.update",
                        target: &target,
                        selector_expression: None,
                        resolved_targets: &affected_client_ids,
                        confirmed: true,
                        payload_hash: Some(&preview_hash),
                    },
                    &password,
                    &salt_hex,
                    300,
                )?;
                body["privilege_assertion"] = serde_json::to_value(privilege_assertion)?;
            }
            http_put_json(
                api_url,
                &format!("/api/v1/configuration-presets/{preset_id}"),
                token,
                &body,
            )
        }
        VtyInventoryCommand::ConfigPresetDelete {
            preset_id,
            confirmed,
        } => {
            anyhow::ensure!(
                confirmed,
                "config-preset-delete requires --confirmed after verifying the preset has no overrides"
            );
            let preset_id = uuid::Uuid::parse_str(&preset_id).context("invalid preset UUID")?;
            http_delete(
                api_url,
                &format!("/api/v1/configuration-presets/{preset_id}"),
                token,
            )
        }
        VtyInventoryCommand::ConfigSources {
            client_id,
            behavior,
        } => http_get(
            api_url,
            &config_sources_path(client_id.as_deref(), behavior.as_deref()),
            token,
        ),
        VtyInventoryCommand::FleetAlerts {
            limit,
            client_id,
            severity,
            category,
            operator_state,
            include_muted,
        } => http_get(
            api_url,
            &fleet_alerts_path(
                limit,
                client_id.as_deref(),
                severity.as_deref(),
                category.as_deref(),
                operator_state.as_deref(),
                include_muted,
            ),
            token,
        ),
        VtyInventoryCommand::FleetAlertExport {
            limit,
            client_id,
            severity,
            category,
            operator_state,
            include_muted,
        } => http_get(
            api_url,
            &fleet_alert_export_path(
                limit,
                client_id.as_deref(),
                severity.as_deref(),
                category.as_deref(),
                operator_state.as_deref(),
                include_muted,
            ),
            token,
        ),
        VtyInventoryCommand::FleetAlertStates { limit, state } => http_get(
            api_url,
            &fleet_alert_states_path(limit, state.as_deref()),
            token,
        ),
        VtyInventoryCommand::FleetAlertStateUpdate {
            alert_id,
            action,
            muted_for_secs,
            reason,
            confirmed,
        } => http_post_json(
            api_url,
            "/api/v1/fleet-alert-states",
            token,
            &serde_json::json!({
                "alert_id": alert_id,
                "action": action,
                "muted_for_secs": muted_for_secs,
                "reason": reason,
                "confirmed": confirmed,
            }),
        ),
        VtyInventoryCommand::VpsRulesList {
            limit,
            selector,
            client_id,
            key,
            state,
        } => http_get(
            api_url,
            &vps_rules_path(
                limit,
                selector.as_deref(),
                client_id.as_deref(),
                key.as_deref(),
                state.as_deref(),
            ),
            token,
        ),
        VtyInventoryCommand::VpsRulesGet { client_id } => http_get(
            api_url,
            &format!(
                "/api/v1/vps-rules/effective/{}",
                percent_encode_query_value(&client_id)
            ),
            token,
        ),
        VtyInventoryCommand::VpsRulesPreview {
            selector,
            set_values,
        } => {
            let values = commands_inventory::parse_key_value_args(&set_values)?;
            let preview = commands_inventory::vps_rules_dry_run(
                api_url,
                token,
                "upsert",
                &selector,
                values,
                Vec::new(),
            )?;
            Ok(serde_json::to_string_pretty(&preview)?)
        }
        VtyInventoryCommand::VpsRulesUpsert {
            selector,
            set_values,
            confirmed,
        } => {
            let values = commands_inventory::parse_key_value_args(&set_values)?;
            let preview = commands_inventory::vps_rules_dry_run(
                api_url,
                token,
                "upsert",
                &selector,
                values.clone(),
                Vec::new(),
            )?;
            if !confirmed {
                Ok(serde_json::to_string_pretty(&preview)?)
            } else {
                let preview_hash = commands_inventory::preview_hash_from_value(&preview)?;
                http_post_json(
                    api_url,
                    "/api/v1/vps-rules/bulk-upsert",
                    token,
                    &serde_json::json!({
                        "selector_expression": selector,
                        "values": values,
                        "confirmed": true,
                        "preview_hash": preview_hash,
                    }),
                )
            }
        }
        VtyInventoryCommand::VpsRulesUnset {
            selector,
            keys,
            confirmed,
        } => {
            let preview = commands_inventory::vps_rules_dry_run(
                api_url,
                token,
                "unset",
                &selector,
                Default::default(),
                keys.clone(),
            )?;
            if !confirmed {
                Ok(serde_json::to_string_pretty(&preview)?)
            } else {
                let preview_hash = commands_inventory::preview_hash_from_value(&preview)?;
                http_post_json(
                    api_url,
                    "/api/v1/vps-rules/bulk-unset",
                    token,
                    &serde_json::json!({
                        "selector_expression": selector,
                        "keys": keys,
                        "confirmed": true,
                        "preview_hash": preview_hash,
                    }),
                )
            }
        }
        VtyInventoryCommand::AlertPoliciesList {
            limit,
            enabled,
            selector,
            client_id,
        } => http_get(
            api_url,
            &alert_policies_path(limit, enabled, selector.as_deref(), client_id.as_deref()),
            token,
        ),
        VtyInventoryCommand::AlertPolicyGet { name } => {
            let body = http_get(api_url, &alert_policies_path(1000, None, None, None), token)?;
            let policies: serde_json::Value = serde_json::from_str(&body)?;
            let policy = policies
                .as_array()
                .and_then(|items| {
                    items.iter().find(|item| {
                        item.get("name").and_then(serde_json::Value::as_str) == Some(name.as_str())
                    })
                })
                .context("alert policy not found")?;
            Ok(serde_json::to_string_pretty(policy)?)
        }
        VtyInventoryCommand::AlertPolicyPreview {
            name,
            selector,
            rule_json,
            enabled,
            notes,
        } => {
            let request = commands_inventory::alert_policy_request(
                commands_inventory::AlertPolicyWriteOptions {
                    name,
                    selector: Some(selector),
                    rule_json,
                    enabled,
                    notes,
                    file: None,
                    confirmed: false,
                },
                None,
            )?;
            let preview = commands_inventory::alert_policy_dry_run(api_url, token, &request)?;
            Ok(serde_json::to_string_pretty(&preview)?)
        }
        VtyInventoryCommand::AlertPolicyUpsert {
            name,
            selector,
            rule_json,
            enabled,
            notes,
            confirmed,
        } => {
            let mut request = commands_inventory::alert_policy_request(
                commands_inventory::AlertPolicyWriteOptions {
                    name,
                    selector: Some(selector),
                    rule_json,
                    enabled,
                    notes,
                    file: None,
                    confirmed,
                },
                None,
            )?;
            let preview = commands_inventory::alert_policy_dry_run(api_url, token, &request)?;
            if !confirmed {
                Ok(serde_json::to_string_pretty(&preview)?)
            } else {
                request["preview_hash"] = serde_json::Value::String(
                    commands_inventory::preview_hash_from_value(&preview)?,
                );
                http_post_json(api_url, "/api/v1/fleet-alert-policies", token, &request)
            }
        }
        VtyInventoryCommand::FleetAlertNotificationChannels {
            limit,
            enabled,
            scope_kind,
            scope_value,
            delivery_kind,
        } => http_get(
            api_url,
            &fleet_alert_notification_channels_path(
                limit,
                enabled,
                scope_kind.as_deref(),
                scope_value.as_deref(),
                delivery_kind.as_deref(),
            ),
            token,
        ),
        VtyInventoryCommand::FleetAlertNotificationChannelUpsert {
            name,
            scope_kind,
            scope_value,
            min_severity,
            categories,
            operator_states,
            delivery_kind,
            target,
            cooldown_secs,
            enabled,
            notes,
            confirmed,
        } => http_post_json(
            api_url,
            "/api/v1/fleet-alert-notification-channels",
            token,
            &serde_json::json!({
                "name": name,
                "scope_kind": scope_kind,
                "scope_value": scope_value,
                "min_severity": min_severity,
                "categories": categories,
                "operator_states": operator_states,
                "delivery_kind": delivery_kind,
                "target": target,
                "cooldown_secs": cooldown_secs,
                "enabled": enabled,
                "notes": notes,
                "confirmed": confirmed,
            }),
        ),
        VtyInventoryCommand::FleetAlertNotifications {
            limit,
            channel_id,
            alert_id,
            status,
        } => http_get(
            api_url,
            &fleet_alert_notifications_path(
                limit,
                channel_id.as_deref(),
                alert_id.as_deref(),
                status.as_deref(),
            ),
            token,
        ),
        VtyInventoryCommand::FleetAlertNotificationDispatch {
            limit,
            client_id,
            severity,
            category,
            operator_state,
            include_muted,
            dry_run,
            preview_hash,
            confirmed,
        } => http_post_json(
            api_url,
            "/api/v1/fleet-alert-notifications/dispatch",
            token,
            &serde_json::json!({
                "limit": limit,
                "client_id": client_id,
                "severity": severity,
                "category": category,
                "operator_state": operator_state,
                "include_muted": include_muted,
                "dry_run": dry_run,
                "preview_hash": preview_hash,
                "confirmed": confirmed,
            }),
        ),
        VtyInventoryCommand::FleetAlertNotificationProcess {
            limit,
            status,
            delivery_kind,
            dry_run,
            preview_hash,
            confirmed,
        } => http_post_json(
            api_url,
            "/api/v1/fleet-alert-notifications/process",
            token,
            &serde_json::json!({
                "limit": limit,
                "status": status,
                "delivery_kind": delivery_kind,
                "dry_run": dry_run,
                "preview_hash": preview_hash,
                "confirmed": confirmed,
            }),
        ),
        VtyInventoryCommand::ConfigRender { client_id, format } => {
            let body = http_get(api_url, &config_render_path(&client_id), token)?;
            match format.as_str() {
                "json" => Ok(body),
                "toml" => {
                    let value: serde_json::Value =
                        serde_json::from_str(&body).context("invalid effective config response")?;
                    Ok(value
                        .get("toml")
                        .and_then(serde_json::Value::as_str)
                        .context("effective config response missing toml")?
                        .to_string())
                }
                _ => anyhow::bail!("--format must be toml or json"),
            }
        }
        VtyInventoryCommand::ConfigSourceChange {
            action,
            behavior,
            preset_id,
            selector,
            clients,
            tags,
            preview_hash,
            confirmed,
        } => {
            let command_name = format!("config-source-{action}");
            let reviewed_preview_hash = commands_inventory::reviewed_preview_hash_arg(
                confirmed,
                preview_hash.as_deref(),
                &command_name,
            )?;
            anyhow::ensure!(
                matches!(action.as_str(), "set" | "reset"),
                "configuration source action must be set or reset"
            );
            let selector_expression =
                commands_inventory::configuration_source_selector(selector.as_deref(), &tags)?;
            let preset_id = match preset_id {
                Some(value) => Some(uuid::Uuid::parse_str(&value).context("invalid preset UUID")?),
                None => None,
            };
            let mut target_client_ids = clients;
            target_client_ids.sort();
            target_client_ids.dedup();
            anyhow::ensure!(
                !target_client_ids.is_empty() || !selector_expression.is_empty(),
                "config-source-{action} requires --client, --tag, or --selector"
            );
            let preview_body = serde_json::json!({
                "action": action,
                "behavior": behavior,
                "preset_id": preset_id,
                "selector_expression": selector_expression.clone(),
                "target_client_ids": target_client_ids.clone(),
            });
            let preview_raw = http_post_json(
                api_url,
                "/api/v1/configuration-source-overrides/preview",
                token,
                &preview_body,
            )?;
            if !confirmed {
                return Ok(preview_raw);
            }
            let preview = commands_inventory::parse_preview_response(&preview_raw)?;
            let current_preview_hash =
                commands_inventory::required_preview_hash(&preview, &command_name)?;
            let preview_hash = commands_inventory::require_matching_reviewed_preview_hash(
                reviewed_preview_hash.as_deref(),
                &current_preview_hash,
                &command_name,
            )?;
            let resolved_target_ids =
                commands_inventory::configuration_source_preview_target_ids(&preview)?;
            let password = load_super_password("VPSMAN_SUPER_PASSWORD")?;
            let salt_hex = load_super_salt_hex(None)?;
            let target = match preset_id {
                Some(preset_id) => format!("configuration_preset:{preset_id}"),
                None => format!("configuration_behavior:{behavior}"),
            };
            let privilege_assertion = build_privilege_for_db(
                DbPrivilegeRequest {
                    action: "configuration_source_override.apply",
                    target: &target,
                    selector_expression: (!selector_expression.is_empty())
                        .then_some(selector_expression.as_str()),
                    resolved_targets: &resolved_target_ids,
                    confirmed: true,
                    payload_hash: Some(&preview_hash),
                },
                &password,
                &salt_hex,
                300,
            )?;
            let mut body = preview_body;
            body["target_client_ids"] = serde_json::to_value(&resolved_target_ids)?;
            body["preview_hash"] = serde_json::Value::String(preview_hash);
            body["privilege_assertion"] = serde_json::to_value(privilege_assertion)?;
            http_post_json(
                api_url,
                "/api/v1/configuration-source-overrides/apply",
                token,
                &body,
            )
        }
        VtyInventoryCommand::BulkResolve { tags } => http_post_json(
            api_url,
            "/api/v1/bulk/resolve",
            token,
            &serde_json::json!({
                "selector_expression": selector_expression_from_targets(&[], &tags),
            }),
        ),
        VtyInventoryCommand::TelemetryRollups {
            limit,
            client_id,
            bucket_secs,
            latest,
        } => http_get(
            api_url,
            &telemetry_rollups_path(limit, client_id.as_deref(), bucket_secs, latest),
            token,
        ),
        VtyInventoryCommand::TelemetryNetworkRates {
            limit,
            client_id,
            interface,
            bucket_secs,
            latest,
        } => http_get(
            api_url,
            &telemetry_network_rates_path(
                limit,
                client_id.as_deref(),
                interface.as_deref(),
                bucket_secs,
                latest,
            ),
            token,
        ),
        VtyInventoryCommand::TelemetryTunnels {
            limit,
            client_id,
            interface,
        } => http_get(
            api_url,
            &telemetry_tunnels_path(limit, client_id.as_deref(), interface.as_deref()),
            token,
        ),
    }
}

fn parse_vty_inventory_command(command: &str) -> Result<VtyInventoryCommand> {
    let owned_parts = split_vty_inventory_command(command)?;
    let parts = owned_parts.iter().map(String::as_str).collect::<Vec<_>>();
    let name = parts.first().copied().context("empty inventory command")?;
    match name {
        "tag-create" => {
            let mut confirmed = false;
            let mut tag_name = None;
            for part in parts.iter().skip(1) {
                match *part {
                    "--confirmed" => confirmed = true,
                    value if tag_name.is_none() => tag_name = Some(value.to_string()),
                    _ => anyhow::bail!("usage: tag-create <name> --confirmed"),
                }
            }
            let tag_name = tag_name.context("usage: tag-create <name> --confirmed")?;
            Ok(VtyInventoryCommand::TagCreate {
                name: tag_name,
                confirmed,
            })
        }
        "agent-tag" => {
            let mut confirmed = false;
            let mut args = Vec::new();
            for part in parts.iter().skip(1) {
                match *part {
                    "--confirmed" => confirmed = true,
                    value => args.push(value.to_string()),
                }
            }
            anyhow::ensure!(
                args.len() == 2,
                "usage: agent-tag <client_id> <tag> --confirmed"
            );
            Ok(VtyInventoryCommand::AgentTag {
                client_id: args[0].clone(),
                tag: args[1].clone(),
                confirmed,
            })
        }
        "config-presets" => {
            let mut behavior = None;
            let mut index = 1;
            while index < parts.len() {
                match parts[index] {
                    "--behavior" => {
                        behavior = Some(
                            (*parts
                                .get(index + 1)
                                .context("--behavior requires a value")?)
                            .to_string(),
                        );
                        index += 2;
                    }
                    value if value.starts_with("--behavior=") => {
                        behavior = Some(value.trim_start_matches("--behavior=").to_string());
                        index += 1;
                    }
                    value => anyhow::bail!("unexpected argument {value}"),
                }
            }
            Ok(VtyInventoryCommand::ConfigPresets { behavior })
        }
        "config-preset-create" => parse_config_preset_create(&parts),
        "config-preset-clone" => parse_config_preset_clone(&parts),
        "config-preset-preview" => parse_config_preset_preview(&parts),
        "config-preset-update" => parse_config_preset_update(&parts),
        "config-preset-delete" => parse_config_preset_delete(&parts),
        "config-sources" => parse_config_sources(&parts),
        "config-source-set" => parse_config_source_change(&parts, "set"),
        "config-source-reset" => parse_config_source_change(&parts, "reset"),
        "config-render" => parse_config_render(&parts),
        "fleet-alerts" => {
            let args = parse_fleet_alert_args(&parts)?;
            Ok(VtyInventoryCommand::FleetAlerts {
                limit: args.limit,
                client_id: args.client_id,
                severity: args.severity,
                category: args.category,
                operator_state: args.operator_state,
                include_muted: args.include_muted,
            })
        }
        "fleet-alert-export" => {
            let args = parse_fleet_alert_args(&parts)?;
            Ok(VtyInventoryCommand::FleetAlertExport {
                limit: args.limit,
                client_id: args.client_id,
                severity: args.severity,
                category: args.category,
                operator_state: args.operator_state,
                include_muted: args.include_muted,
            })
        }
        "fleet-alert-states" => {
            let args = parse_fleet_alert_state_list(&parts)?;
            Ok(VtyInventoryCommand::FleetAlertStates {
                limit: args.limit,
                state: args.state,
            })
        }
        "fleet-alert-state-update" => parse_fleet_alert_state_update(&parts),
        "vps-rules" => parse_vps_rules_list(&parts),
        "vps-rules-get" => parse_vps_rules_get(&parts),
        "vps-rules-preview" => parse_vps_rules_preview(&parts),
        "vps-rules-upsert" => parse_vps_rules_upsert(&parts),
        "vps-rules-unset" => parse_vps_rules_unset(&parts),
        "alert-policies" => parse_alert_policies_list(&parts),
        "alert-policy-get" => parse_alert_policy_get(&parts),
        "alert-policy-preview" => parse_alert_policy_write(&parts, false),
        "alert-policy-upsert" => parse_alert_policy_write(&parts, true),
        "fleet-alert-notification-channels" => {
            let args = parse_fleet_alert_notification_channel_list(&parts)?;
            Ok(VtyInventoryCommand::FleetAlertNotificationChannels {
                limit: args.limit,
                enabled: args.enabled,
                scope_kind: args.scope_kind,
                scope_value: args.scope_value,
                delivery_kind: args.delivery_kind,
            })
        }
        "fleet-alert-notification-channel-upsert" => {
            parse_fleet_alert_notification_channel_upsert(&parts)
        }
        "fleet-alert-notifications" => {
            let args = parse_fleet_alert_notification_list(&parts)?;
            Ok(VtyInventoryCommand::FleetAlertNotifications {
                limit: args.limit,
                channel_id: args.channel_id,
                alert_id: args.alert_id,
                status: args.status,
            })
        }
        "fleet-alert-notification-dispatch" => parse_fleet_alert_notification_dispatch(&parts),
        "fleet-alert-notification-process" => parse_fleet_alert_notification_process(&parts),
        "bulk-resolve" => Ok(VtyInventoryCommand::BulkResolve {
            tags: parts
                .iter()
                .skip(1)
                .map(|value| (*value).to_string())
                .collect(),
        }),
        "telemetry-rollups" => {
            let (limit, client_id, bucket_secs, latest) = parse_telemetry_rollups_args(&parts)?;
            Ok(VtyInventoryCommand::TelemetryRollups {
                limit,
                client_id,
                bucket_secs,
                latest,
            })
        }
        "telemetry-network-rates" => {
            let args = parse_telemetry_network_rates_args(&parts)?;
            Ok(VtyInventoryCommand::TelemetryNetworkRates {
                limit: args.limit,
                client_id: args.client_id,
                interface: args.interface,
                bucket_secs: args.bucket_secs,
                latest: args.latest,
            })
        }
        "telemetry-tunnels" => {
            let args = parse_telemetry_tunnels_args(&parts)?;
            Ok(VtyInventoryCommand::TelemetryTunnels {
                limit: args.limit,
                client_id: args.client_id,
                interface: args.interface,
            })
        }
        other => anyhow::bail!("unknown inventory command: {other}"),
    }
}

fn split_vty_inventory_command(command: &str) -> Result<Vec<String>> {
    let mut parts = Vec::new();
    let mut part = String::new();
    let mut part_started = false;
    let mut quote = None;
    let mut chars = command.chars();

    while let Some(character) = chars.next() {
        match quote {
            Some(active_quote) if character == active_quote => quote = None,
            Some('\'') => part.push(character),
            Some('"') if character == '\\' => {
                part.push(
                    chars
                        .next()
                        .context("inventory command ends with an incomplete escape")?,
                );
            }
            Some(_) => part.push(character),
            None if character.is_whitespace() => {
                if part_started {
                    parts.push(std::mem::take(&mut part));
                    part_started = false;
                }
            }
            None if matches!(character, '\'' | '"') => {
                quote = Some(character);
                part_started = true;
            }
            None if character == '\\' => {
                part.push(
                    chars
                        .next()
                        .context("inventory command ends with an incomplete escape")?,
                );
                part_started = true;
            }
            None => {
                part.push(character);
                part_started = true;
            }
        }
    }

    anyhow::ensure!(
        quote.is_none(),
        "inventory command has an unterminated quote"
    );
    if part_started {
        parts.push(part);
    }
    Ok(parts)
}

fn parse_config_preset_create(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let mut behavior = None;
    let mut name = None;
    let mut description = None;
    let mut definition = None;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--behavior" => {
                behavior = Some(next_arg(parts, index, "--behavior")?.to_string());
                index += 2;
            }
            "--name" => {
                name = Some(next_arg(parts, index, "--name")?.to_string());
                index += 2;
            }
            "--description" => {
                description = Some(next_arg(parts, index, "--description")?.to_string());
                index += 2;
            }
            "--definition-json" => {
                definition = Some(
                    serde_json::from_str(next_arg(parts, index, "--definition-json")?)
                        .context("invalid --definition-json")?,
                );
                index += 2;
            }
            value if value.starts_with("--behavior=") => {
                behavior = Some(value.trim_start_matches("--behavior=").to_string());
                index += 1;
            }
            value if value.starts_with("--name=") => {
                name = Some(value.trim_start_matches("--name=").to_string());
                index += 1;
            }
            value if value.starts_with("--description=") => {
                description = Some(value.trim_start_matches("--description=").to_string());
                index += 1;
            }
            value if value.starts_with("--definition-json=") => {
                definition = Some(
                    serde_json::from_str(value.trim_start_matches("--definition-json="))
                        .context("invalid --definition-json")?,
                );
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    Ok(VtyInventoryCommand::ConfigPresetCreate {
        behavior: behavior.context("config-preset-create requires --behavior")?,
        name: name.context("config-preset-create requires --name")?,
        description,
        definition: definition.context("config-preset-create requires --definition-json")?,
    })
}

fn parse_config_preset_clone(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let mut preset_id = None;
    let mut name = None;
    let mut description = None;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--preset-id" => {
                preset_id = Some(next_arg(parts, index, "--preset-id")?.to_string());
                index += 2;
            }
            "--name" => {
                name = Some(next_arg(parts, index, "--name")?.to_string());
                index += 2;
            }
            "--description" => {
                description = Some(next_arg(parts, index, "--description")?.to_string());
                index += 2;
            }
            value if value.starts_with("--preset-id=") => {
                preset_id = Some(value.trim_start_matches("--preset-id=").to_string());
                index += 1;
            }
            value if value.starts_with("--name=") => {
                name = Some(value.trim_start_matches("--name=").to_string());
                index += 1;
            }
            value if value.starts_with("--description=") => {
                description = Some(value.trim_start_matches("--description=").to_string());
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    Ok(VtyInventoryCommand::ConfigPresetClone {
        preset_id: preset_id.context("config-preset-clone requires --preset-id")?,
        name: name.context("config-preset-clone requires --name")?,
        description,
    })
}

fn parse_config_preset_preview(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let ParsedConfigPresetCandidate {
        preset_id,
        description,
        clear_description,
        definition,
        preview_hash,
        confirmed,
    } = parse_config_preset_candidate_args(parts, "config-preset-preview")?;
    anyhow::ensure!(
        !confirmed && preview_hash.is_none(),
        "config-preset-preview does not accept --confirmed or --preview-hash"
    );
    Ok(VtyInventoryCommand::ConfigPresetPreview {
        preset_id,
        description,
        clear_description,
        definition,
    })
}

fn parse_config_preset_update(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let ParsedConfigPresetCandidate {
        preset_id,
        description,
        clear_description,
        definition,
        preview_hash,
        confirmed,
    } = parse_config_preset_candidate_args(parts, "config-preset-update")?;
    commands_inventory::reviewed_preview_hash_arg(
        confirmed,
        preview_hash.as_deref(),
        "config-preset-update",
    )?;
    Ok(VtyInventoryCommand::ConfigPresetUpdate {
        preset_id,
        description,
        clear_description,
        definition,
        preview_hash,
        confirmed,
    })
}

struct ParsedConfigPresetCandidate {
    preset_id: String,
    description: Option<String>,
    clear_description: bool,
    definition: serde_json::Value,
    preview_hash: Option<String>,
    confirmed: bool,
}

fn parse_config_preset_candidate_args(
    parts: &[&str],
    command_name: &str,
) -> Result<ParsedConfigPresetCandidate> {
    let mut preset_id = None;
    let mut description = None;
    let mut clear_description = false;
    let mut definition = None;
    let mut preview_hash = None;
    let mut confirmed = false;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--preset-id" => {
                preset_id = Some(next_arg(parts, index, "--preset-id")?.to_string());
                index += 2;
            }
            "--description" => {
                description = Some(next_arg(parts, index, "--description")?.to_string());
                index += 2;
            }
            "--clear-description" => {
                clear_description = true;
                index += 1;
            }
            "--definition-json" => {
                definition = Some(
                    serde_json::from_str(next_arg(parts, index, "--definition-json")?)
                        .context("invalid --definition-json")?,
                );
                index += 2;
            }
            "--preview-hash" => {
                preview_hash = Some(next_arg(parts, index, "--preview-hash")?.to_string());
                index += 2;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            value if value.starts_with("--preset-id=") => {
                preset_id = Some(value.trim_start_matches("--preset-id=").to_string());
                index += 1;
            }
            value if value.starts_with("--description=") => {
                description = Some(value.trim_start_matches("--description=").to_string());
                index += 1;
            }
            value if value.starts_with("--definition-json=") => {
                definition = Some(
                    serde_json::from_str(value.trim_start_matches("--definition-json="))
                        .context("invalid --definition-json")?,
                );
                index += 1;
            }
            value if value.starts_with("--preview-hash=") => {
                preview_hash = Some(value.trim_start_matches("--preview-hash=").to_string());
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    anyhow::ensure!(
        description.is_none() || !clear_description,
        "use only one of --description or --clear-description"
    );
    Ok(ParsedConfigPresetCandidate {
        preset_id: preset_id.with_context(|| format!("{command_name} requires --preset-id"))?,
        description,
        clear_description,
        definition: definition
            .with_context(|| format!("{command_name} requires --definition-json"))?,
        preview_hash,
        confirmed,
    })
}

fn parse_config_preset_delete(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let mut preset_id = None;
    let mut confirmed = false;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--preset-id" => {
                preset_id = Some(next_arg(parts, index, "--preset-id")?.to_string());
                index += 2;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            value if value.starts_with("--preset-id=") => {
                preset_id = Some(value.trim_start_matches("--preset-id=").to_string());
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    Ok(VtyInventoryCommand::ConfigPresetDelete {
        preset_id: preset_id.context("config-preset-delete requires --preset-id")?,
        confirmed,
    })
}

fn parse_config_sources(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let (client_id, behavior) = parse_config_source_filter_args(parts, "config-sources")?;
    Ok(VtyInventoryCommand::ConfigSources {
        client_id,
        behavior,
    })
}

fn parse_fleet_alert_args(parts: &[&str]) -> Result<FleetAlertArgs> {
    let mut limit = 50_u16;
    let mut client_id = None;
    let mut severity = None;
    let mut category = None;
    let mut operator_state = None;
    let mut include_muted = false;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--limit" => {
                limit = next_arg(parts, index, "--limit")?
                    .parse()
                    .context("--limit must be an integer")?;
                index += 2;
            }
            "--client-id" => {
                client_id = Some(next_arg(parts, index, "--client-id")?.to_string());
                index += 2;
            }
            "--severity" => {
                severity = Some(next_arg(parts, index, "--severity")?.to_string());
                index += 2;
            }
            "--category" => {
                category = Some(next_arg(parts, index, "--category")?.to_string());
                index += 2;
            }
            "--operator-state" => {
                operator_state = Some(next_arg(parts, index, "--operator-state")?.to_string());
                index += 2;
            }
            "--include-muted" => {
                include_muted = true;
                index += 1;
            }
            value if value.starts_with("--limit=") => {
                limit = value
                    .trim_start_matches("--limit=")
                    .parse()
                    .context("--limit must be an integer")?;
                index += 1;
            }
            value if value.starts_with("--client-id=") => {
                client_id = Some(value.trim_start_matches("--client-id=").to_string());
                index += 1;
            }
            value if value.starts_with("--severity=") => {
                severity = Some(value.trim_start_matches("--severity=").to_string());
                index += 1;
            }
            value if value.starts_with("--category=") => {
                category = Some(value.trim_start_matches("--category=").to_string());
                index += 1;
            }
            value if value.starts_with("--operator-state=") => {
                operator_state = Some(value.trim_start_matches("--operator-state=").to_string());
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    anyhow::ensure!(
        (1..=200).contains(&limit),
        "fleet-alerts --limit must be between 1 and 200"
    );
    if let Some(client_id) = client_id.as_deref() {
        anyhow::ensure!(
            !client_id.is_empty() && client_id.len() <= 128,
            "fleet-alerts --client-id must be between 1 and 128 bytes"
        );
    }
    if let Some(severity) = severity.as_deref() {
        anyhow::ensure!(
            matches!(severity, "critical" | "warning" | "info"),
            "fleet-alerts --severity must be critical, warning, or info"
        );
    }
    if let Some(category) = category.as_deref() {
        validate_alert_token(category, "fleet-alerts --category")?;
    }
    if let Some(operator_state) = operator_state.as_deref() {
        validate_alert_state(operator_state, "fleet-alerts --operator-state")?;
    }
    Ok(FleetAlertArgs {
        limit,
        client_id,
        severity,
        category,
        operator_state,
        include_muted,
    })
}

fn parse_fleet_alert_state_list(parts: &[&str]) -> Result<FleetAlertStateListArgs> {
    let mut limit = 50_u16;
    let mut state = None;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--limit" => {
                limit = next_arg(parts, index, "--limit")?
                    .parse()
                    .context("--limit must be an integer")?;
                index += 2;
            }
            "--state" => {
                state = Some(next_arg(parts, index, "--state")?.to_string());
                index += 2;
            }
            value if value.starts_with("--limit=") => {
                limit = value
                    .trim_start_matches("--limit=")
                    .parse()
                    .context("--limit must be an integer")?;
                index += 1;
            }
            value if value.starts_with("--state=") => {
                state = Some(value.trim_start_matches("--state=").to_string());
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    anyhow::ensure!(
        (1..=1000).contains(&limit),
        "fleet-alert-states --limit must be between 1 and 1000"
    );
    if let Some(state) = state.as_deref() {
        validate_alert_state(state, "fleet-alert-states --state")?;
    }
    Ok(FleetAlertStateListArgs { limit, state })
}

fn parse_fleet_alert_state_update(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let mut alert_id = None;
    let mut action = None;
    let mut muted_for_secs = None;
    let mut reason = None;
    let mut confirmed = false;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--alert-id" => {
                alert_id = Some(next_arg(parts, index, "--alert-id")?.to_string());
                index += 2;
            }
            "--action" => {
                action = Some(next_arg(parts, index, "--action")?.to_string());
                index += 2;
            }
            "--muted-for-secs" => {
                muted_for_secs = Some(
                    next_arg(parts, index, "--muted-for-secs")?
                        .parse()
                        .context("--muted-for-secs must be an integer")?,
                );
                index += 2;
            }
            "--reason" => {
                reason = Some(next_arg(parts, index, "--reason")?.to_string());
                index += 2;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            value if value.starts_with("--alert-id=") => {
                alert_id = Some(value.trim_start_matches("--alert-id=").to_string());
                index += 1;
            }
            value if value.starts_with("--action=") => {
                action = Some(value.trim_start_matches("--action=").to_string());
                index += 1;
            }
            value if value.starts_with("--muted-for-secs=") => {
                muted_for_secs = Some(
                    value
                        .trim_start_matches("--muted-for-secs=")
                        .parse()
                        .context("--muted-for-secs must be an integer")?,
                );
                index += 1;
            }
            value if value.starts_with("--reason=") => {
                reason = Some(value.trim_start_matches("--reason=").to_string());
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    let alert_id = alert_id.context("fleet-alert-state-update requires --alert-id")?;
    let action = action.context("fleet-alert-state-update requires --action")?;
    validate_alert_token(&alert_id, "fleet-alert-state-update --alert-id")?;
    match action.as_str() {
        "acknowledge" | "mute" | "escalate" | "clear" => {}
        _ => anyhow::bail!("fleet-alert-state-update --action is invalid"),
    }
    if let Some(seconds) = muted_for_secs {
        anyhow::ensure!(
            (60..=90 * 24 * 60 * 60).contains(&seconds),
            "fleet-alert-state-update --muted-for-secs must be between 60 and 7776000"
        );
    }
    anyhow::ensure!(confirmed, "fleet-alert-state-update requires --confirmed");
    Ok(VtyInventoryCommand::FleetAlertStateUpdate {
        alert_id,
        action,
        muted_for_secs,
        reason,
        confirmed,
    })
}

fn parse_vps_rules_list(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let mut limit = 50_u16;
    let mut selector = None;
    let mut client_id = None;
    let mut key = None;
    let mut state = None;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--limit" => {
                limit = next_arg(parts, index, "--limit")?
                    .parse()
                    .context("--limit must be an integer")?;
                index += 2;
            }
            "--selector" => {
                selector = Some(next_arg(parts, index, "--selector")?.to_string());
                index += 2;
            }
            "--client-id" => {
                client_id = Some(next_arg(parts, index, "--client-id")?.to_string());
                index += 2;
            }
            "--key" => {
                key = Some(next_arg(parts, index, "--key")?.to_string());
                index += 2;
            }
            "--state" => {
                state = Some(next_arg(parts, index, "--state")?.to_string());
                index += 2;
            }
            value if value.starts_with("--limit=") => {
                limit = value
                    .trim_start_matches("--limit=")
                    .parse()
                    .context("--limit must be an integer")?;
                index += 1;
            }
            value if value.starts_with("--selector=") => {
                selector = Some(value.trim_start_matches("--selector=").to_string());
                index += 1;
            }
            value if value.starts_with("--client-id=") => {
                client_id = Some(value.trim_start_matches("--client-id=").to_string());
                index += 1;
            }
            value if value.starts_with("--key=") => {
                key = Some(value.trim_start_matches("--key=").to_string());
                index += 1;
            }
            value if value.starts_with("--state=") => {
                state = Some(value.trim_start_matches("--state=").to_string());
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    anyhow::ensure!(
        (1..=1000).contains(&limit),
        "vps-rules --limit must be between 1 and 1000"
    );
    Ok(VtyInventoryCommand::VpsRulesList {
        limit,
        selector,
        client_id,
        key,
        state,
    })
}

fn parse_vps_rules_get(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let mut client_id = None;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--client-id" => {
                client_id = Some(next_arg(parts, index, "--client-id")?.to_string());
                index += 2;
            }
            value if value.starts_with("--client-id=") => {
                client_id = Some(value.trim_start_matches("--client-id=").to_string());
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    Ok(VtyInventoryCommand::VpsRulesGet {
        client_id: client_id.context("vps-rules-get requires --client-id")?,
    })
}

fn parse_vps_rules_preview(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let (selector, set_values, _) = parse_vps_rule_set_args(parts, false)?;
    Ok(VtyInventoryCommand::VpsRulesPreview {
        selector,
        set_values,
    })
}

fn parse_vps_rules_upsert(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let (selector, set_values, confirmed) = parse_vps_rule_set_args(parts, true)?;
    Ok(VtyInventoryCommand::VpsRulesUpsert {
        selector,
        set_values,
        confirmed,
    })
}

fn parse_vps_rules_unset(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let mut selector = None;
    let mut keys = Vec::new();
    let mut confirmed = false;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--selector" => {
                selector = Some(next_arg(parts, index, "--selector")?.to_string());
                index += 2;
            }
            "--key" => {
                keys.push(next_arg(parts, index, "--key")?.to_string());
                index += 2;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            value if value.starts_with("--selector=") => {
                selector = Some(value.trim_start_matches("--selector=").to_string());
                index += 1;
            }
            value if value.starts_with("--key=") => {
                keys.push(value.trim_start_matches("--key=").to_string());
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    anyhow::ensure!(
        !keys.is_empty(),
        "vps-rules-unset requires at least one --key"
    );
    Ok(VtyInventoryCommand::VpsRulesUnset {
        selector: selector.context("vps-rules-unset requires --selector")?,
        keys,
        confirmed,
    })
}

fn parse_vps_rule_set_args(
    parts: &[&str],
    allow_confirmed: bool,
) -> Result<(String, Vec<String>, bool)> {
    let mut selector = None;
    let mut set_values = Vec::new();
    let mut confirmed = false;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--selector" => {
                selector = Some(next_arg(parts, index, "--selector")?.to_string());
                index += 2;
            }
            "--set" => {
                set_values.push(next_arg(parts, index, "--set")?.to_string());
                index += 2;
            }
            "--confirmed" if allow_confirmed => {
                confirmed = true;
                index += 1;
            }
            value if value.starts_with("--selector=") => {
                selector = Some(value.trim_start_matches("--selector=").to_string());
                index += 1;
            }
            value if value.starts_with("--set=") => {
                set_values.push(value.trim_start_matches("--set=").to_string());
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    anyhow::ensure!(
        !set_values.is_empty(),
        "at least one --set key=value is required"
    );
    Ok((
        selector.context("vps-rules command requires --selector")?,
        set_values,
        confirmed,
    ))
}

fn parse_alert_policies_list(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let mut limit = 50_u16;
    let mut enabled = None;
    let mut selector = None;
    let mut client_id = None;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--limit" => {
                limit = next_arg(parts, index, "--limit")?
                    .parse()
                    .context("--limit must be an integer")?;
                index += 2;
            }
            "--enabled" => {
                enabled = Some(parse_bool(next_arg(parts, index, "--enabled")?)?);
                index += 2;
            }
            "--selector" => {
                selector = Some(next_arg(parts, index, "--selector")?.to_string());
                index += 2;
            }
            "--client-id" => {
                client_id = Some(next_arg(parts, index, "--client-id")?.to_string());
                index += 2;
            }
            value if value.starts_with("--limit=") => {
                limit = value
                    .trim_start_matches("--limit=")
                    .parse()
                    .context("--limit must be an integer")?;
                index += 1;
            }
            value if value.starts_with("--enabled=") => {
                enabled = Some(parse_bool(value.trim_start_matches("--enabled="))?);
                index += 1;
            }
            value if value.starts_with("--selector=") => {
                selector = Some(value.trim_start_matches("--selector=").to_string());
                index += 1;
            }
            value if value.starts_with("--client-id=") => {
                client_id = Some(value.trim_start_matches("--client-id=").to_string());
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    anyhow::ensure!(
        (1..=1000).contains(&limit),
        "alert-policies --limit must be between 1 and 1000"
    );
    Ok(VtyInventoryCommand::AlertPoliciesList {
        limit,
        enabled,
        selector,
        client_id,
    })
}

fn parse_alert_policy_get(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let mut name = None;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--name" => {
                name = Some(next_arg(parts, index, "--name")?.to_string());
                index += 2;
            }
            value if value.starts_with("--name=") => {
                name = Some(value.trim_start_matches("--name=").to_string());
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    Ok(VtyInventoryCommand::AlertPolicyGet {
        name: name.context("alert-policy-get requires --name")?,
    })
}

fn parse_alert_policy_write(parts: &[&str], apply: bool) -> Result<VtyInventoryCommand> {
    let mut name = None;
    let mut selector = None;
    let mut rule_json = Vec::new();
    let mut enabled = true;
    let mut notes = None;
    let mut confirmed = false;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--name" => {
                name = Some(next_arg(parts, index, "--name")?.to_string());
                index += 2;
            }
            "--selector" => {
                selector = Some(next_arg(parts, index, "--selector")?.to_string());
                index += 2;
            }
            "--rule-json" => {
                rule_json.push(next_arg(parts, index, "--rule-json")?.to_string());
                index += 2;
            }
            "--enabled" => {
                enabled = parse_bool(next_arg(parts, index, "--enabled")?)?;
                index += 2;
            }
            "--notes" => {
                notes = Some(next_arg(parts, index, "--notes")?.to_string());
                index += 2;
            }
            "--confirmed" if apply => {
                confirmed = true;
                index += 1;
            }
            value if value.starts_with("--name=") => {
                name = Some(value.trim_start_matches("--name=").to_string());
                index += 1;
            }
            value if value.starts_with("--selector=") => {
                selector = Some(value.trim_start_matches("--selector=").to_string());
                index += 1;
            }
            value if value.starts_with("--rule-json=") => {
                rule_json.push(value.trim_start_matches("--rule-json=").to_string());
                index += 1;
            }
            value if value.starts_with("--enabled=") => {
                enabled = parse_bool(value.trim_start_matches("--enabled="))?;
                index += 1;
            }
            value if value.starts_with("--notes=") => {
                notes = Some(value.trim_start_matches("--notes=").to_string());
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    anyhow::ensure!(
        !rule_json.is_empty(),
        "alert-policy command requires at least one --rule-json PolicyRuleRequest"
    );
    let name = name.context("alert-policy command requires --name")?;
    let selector = selector.context("alert-policy command requires --selector")?;
    if apply {
        Ok(VtyInventoryCommand::AlertPolicyUpsert {
            name,
            selector,
            rule_json,
            enabled,
            notes,
            confirmed,
        })
    } else {
        Ok(VtyInventoryCommand::AlertPolicyPreview {
            name,
            selector,
            rule_json,
            enabled,
            notes,
        })
    }
}

fn parse_fleet_alert_notification_channel_list(
    parts: &[&str],
) -> Result<FleetAlertNotificationChannelListArgs> {
    let mut limit = 50_u16;
    let mut enabled = None;
    let mut scope_kind = None;
    let mut scope_value = None;
    let mut delivery_kind = None;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--limit" => {
                limit = next_arg(parts, index, "--limit")?
                    .parse()
                    .context("--limit must be an integer")?;
                index += 2;
            }
            "--enabled" => {
                enabled = Some(parse_bool(next_arg(parts, index, "--enabled")?)?);
                index += 2;
            }
            "--scope-kind" => {
                scope_kind = Some(next_arg(parts, index, "--scope-kind")?.to_string());
                index += 2;
            }
            "--scope-value" => {
                scope_value = Some(next_arg(parts, index, "--scope-value")?.to_string());
                index += 2;
            }
            "--delivery-kind" => {
                delivery_kind = Some(next_arg(parts, index, "--delivery-kind")?.to_string());
                index += 2;
            }
            value if value.starts_with("--limit=") => {
                limit = value
                    .trim_start_matches("--limit=")
                    .parse()
                    .context("--limit must be an integer")?;
                index += 1;
            }
            value if value.starts_with("--enabled=") => {
                enabled = Some(parse_bool(value.trim_start_matches("--enabled="))?);
                index += 1;
            }
            value if value.starts_with("--scope-kind=") => {
                scope_kind = Some(value.trim_start_matches("--scope-kind=").to_string());
                index += 1;
            }
            value if value.starts_with("--scope-value=") => {
                scope_value = Some(value.trim_start_matches("--scope-value=").to_string());
                index += 1;
            }
            value if value.starts_with("--delivery-kind=") => {
                delivery_kind = Some(value.trim_start_matches("--delivery-kind=").to_string());
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    anyhow::ensure!(
        (1..=1000).contains(&limit),
        "fleet-alert-notification-channels --limit must be between 1 and 1000"
    );
    if let Some(scope_kind) = scope_kind.as_deref() {
        validate_fleet_alert_policy_scope_kind(scope_kind)?;
    }
    if let Some(delivery_kind) = delivery_kind.as_deref() {
        validate_alert_notification_delivery_kind(
            delivery_kind,
            "fleet-alert-notification-channels --delivery-kind",
        )?;
    }
    Ok(FleetAlertNotificationChannelListArgs {
        limit,
        enabled,
        scope_kind,
        scope_value,
        delivery_kind,
    })
}

fn parse_fleet_alert_notification_channel_upsert(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let mut name = None;
    let mut scope_kind = None;
    let mut scope_value = None;
    let mut min_severity = None;
    let mut categories = Vec::new();
    let mut operator_states = Vec::new();
    let mut delivery_kind = None;
    let mut target = None;
    let mut cooldown_secs = None;
    let mut enabled = true;
    let mut notes = None;
    let mut confirmed = false;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--name" => {
                name = Some(next_arg(parts, index, "--name")?.to_string());
                index += 2;
            }
            "--scope-kind" => {
                scope_kind = Some(next_arg(parts, index, "--scope-kind")?.to_string());
                index += 2;
            }
            "--scope-value" => {
                scope_value = Some(next_arg(parts, index, "--scope-value")?.to_string());
                index += 2;
            }
            "--min-severity" => {
                min_severity = Some(next_arg(parts, index, "--min-severity")?.to_string());
                index += 2;
            }
            "--categories" => {
                categories.extend(parse_csv_tokens(next_arg(parts, index, "--categories")?));
                index += 2;
            }
            "--operator-states" => {
                operator_states.extend(parse_csv_tokens(next_arg(
                    parts,
                    index,
                    "--operator-states",
                )?));
                index += 2;
            }
            "--delivery-kind" => {
                delivery_kind = Some(next_arg(parts, index, "--delivery-kind")?.to_string());
                index += 2;
            }
            "--target" => {
                target = Some(next_arg(parts, index, "--target")?.to_string());
                index += 2;
            }
            "--cooldown-secs" => {
                cooldown_secs = Some(
                    next_arg(parts, index, "--cooldown-secs")?
                        .parse()
                        .context("--cooldown-secs must be an integer")?,
                );
                index += 2;
            }
            "--enabled" => {
                enabled = parse_bool(next_arg(parts, index, "--enabled")?)?;
                index += 2;
            }
            "--notes" => {
                notes = Some(next_arg(parts, index, "--notes")?.to_string());
                index += 2;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    let scope_kind =
        scope_kind.context("fleet-alert-notification-channel-upsert requires --scope-kind")?;
    validate_fleet_alert_policy_scope_kind(&scope_kind)?;
    if let Some(severity) = min_severity.as_deref() {
        validate_alert_severity(
            severity,
            "fleet-alert-notification-channel-upsert --min-severity",
        )?;
    }
    for category in &categories {
        validate_alert_token(
            category,
            "fleet-alert-notification-channel-upsert --categories",
        )?;
    }
    for state in &operator_states {
        validate_alert_state(
            state,
            "fleet-alert-notification-channel-upsert --operator-states",
        )?;
    }
    let delivery_kind = delivery_kind
        .context("fleet-alert-notification-channel-upsert requires --delivery-kind")?;
    validate_alert_notification_delivery_kind(
        &delivery_kind,
        "fleet-alert-notification-channel-upsert --delivery-kind",
    )?;
    let target = target.context("fleet-alert-notification-channel-upsert requires --target")?;
    anyhow::ensure!(
        !target.trim().is_empty() && target.len() <= 512,
        "fleet-alert-notification-channel-upsert --target is invalid"
    );
    if let Some(cooldown_secs) = cooldown_secs {
        anyhow::ensure!(
            (0..=30 * 24 * 60 * 60).contains(&cooldown_secs),
            "fleet-alert-notification-channel-upsert --cooldown-secs must be between 0 and 2592000"
        );
    }
    anyhow::ensure!(
        confirmed,
        "fleet-alert-notification-channel-upsert requires --confirmed"
    );
    Ok(VtyInventoryCommand::FleetAlertNotificationChannelUpsert {
        name: name.context("fleet-alert-notification-channel-upsert requires --name")?,
        scope_kind,
        scope_value,
        min_severity,
        categories,
        operator_states,
        delivery_kind,
        target,
        cooldown_secs,
        enabled,
        notes,
        confirmed,
    })
}

fn parse_fleet_alert_notification_list(parts: &[&str]) -> Result<FleetAlertNotificationListArgs> {
    let mut limit = 50_u16;
    let mut channel_id = None;
    let mut alert_id = None;
    let mut status = None;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--limit" => {
                limit = next_arg(parts, index, "--limit")?
                    .parse()
                    .context("--limit must be an integer")?;
                index += 2;
            }
            "--channel-id" => {
                channel_id = Some(next_arg(parts, index, "--channel-id")?.to_string());
                index += 2;
            }
            "--alert-id" => {
                alert_id = Some(next_arg(parts, index, "--alert-id")?.to_string());
                index += 2;
            }
            "--status" => {
                status = Some(next_arg(parts, index, "--status")?.to_string());
                index += 2;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    anyhow::ensure!(
        (1..=1000).contains(&limit),
        "fleet-alert-notifications --limit must be between 1 and 1000"
    );
    if let Some(alert_id) = alert_id.as_deref() {
        validate_alert_token(alert_id, "fleet-alert-notifications --alert-id")?;
    }
    if let Some(status) = status.as_deref() {
        validate_alert_token(status, "fleet-alert-notifications --status")?;
    }
    Ok(FleetAlertNotificationListArgs {
        limit,
        channel_id,
        alert_id,
        status,
    })
}

fn parse_fleet_alert_notification_dispatch(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let mut limit = 200_u16;
    let mut client_id = None;
    let mut severity = None;
    let mut category = None;
    let mut operator_state = None;
    let mut include_muted = false;
    let mut dry_run = false;
    let mut preview_hash = None;
    let mut confirmed = false;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--limit" => {
                limit = next_arg(parts, index, "--limit")?
                    .parse()
                    .context("--limit must be an integer")?;
                index += 2;
            }
            "--client-id" => {
                client_id = Some(next_arg(parts, index, "--client-id")?.to_string());
                index += 2;
            }
            "--severity" => {
                severity = Some(next_arg(parts, index, "--severity")?.to_string());
                index += 2;
            }
            "--category" => {
                category = Some(next_arg(parts, index, "--category")?.to_string());
                index += 2;
            }
            "--operator-state" => {
                operator_state = Some(next_arg(parts, index, "--operator-state")?.to_string());
                index += 2;
            }
            "--include-muted" => {
                include_muted = true;
                index += 1;
            }
            "--dry-run" => {
                dry_run = true;
                index += 1;
            }
            "--preview-hash" => {
                preview_hash = Some(next_arg(parts, index, "--preview-hash")?.to_string());
                index += 2;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            value if value.starts_with("--limit=") => {
                limit = value
                    .trim_start_matches("--limit=")
                    .parse()
                    .context("--limit must be an integer")?;
                index += 1;
            }
            value if value.starts_with("--client-id=") => {
                client_id = Some(value.trim_start_matches("--client-id=").to_string());
                index += 1;
            }
            value if value.starts_with("--severity=") => {
                severity = Some(value.trim_start_matches("--severity=").to_string());
                index += 1;
            }
            value if value.starts_with("--category=") => {
                category = Some(value.trim_start_matches("--category=").to_string());
                index += 1;
            }
            value if value.starts_with("--operator-state=") => {
                operator_state = Some(value.trim_start_matches("--operator-state=").to_string());
                index += 1;
            }
            value if value.starts_with("--preview-hash=") => {
                preview_hash = Some(value.trim_start_matches("--preview-hash=").to_string());
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    anyhow::ensure!(
        (1..=200).contains(&limit),
        "fleet-alert-notification-dispatch --limit must be between 1 and 200"
    );
    if let Some(client_id) = client_id.as_deref() {
        anyhow::ensure!(
            !client_id.is_empty() && client_id.len() <= 128,
            "fleet-alert-notification-dispatch --client-id must be between 1 and 128 bytes"
        );
    }
    if let Some(severity) = severity.as_deref() {
        validate_alert_severity(severity, "fleet-alert-notification-dispatch --severity")?;
    }
    if let Some(category) = category.as_deref() {
        validate_alert_token(category, "fleet-alert-notification-dispatch --category")?;
    }
    if let Some(operator_state) = operator_state.as_deref() {
        validate_alert_state(
            operator_state,
            "fleet-alert-notification-dispatch --operator-state",
        )?;
    }
    anyhow::ensure!(
        dry_run || confirmed,
        "fleet-alert-notification-dispatch requires --confirmed unless --dry-run is set"
    );
    if !dry_run && confirmed {
        anyhow::ensure!(
            preview_hash
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()),
            "fleet-alert-notification-dispatch requires --preview-hash when --confirmed is set"
        );
    }
    Ok(VtyInventoryCommand::FleetAlertNotificationDispatch {
        limit,
        client_id,
        severity,
        category,
        operator_state,
        include_muted,
        dry_run,
        preview_hash,
        confirmed,
    })
}

fn parse_fleet_alert_notification_process(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let mut limit = 50_u16;
    let mut status = None;
    let mut delivery_kind = None;
    let mut dry_run = false;
    let mut preview_hash = None;
    let mut confirmed = false;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--limit" => {
                limit = next_arg(parts, index, "--limit")?
                    .parse()
                    .context("--limit must be an integer")?;
                index += 2;
            }
            "--status" => {
                status = Some(next_arg(parts, index, "--status")?.to_string());
                index += 2;
            }
            "--delivery-kind" => {
                delivery_kind = Some(next_arg(parts, index, "--delivery-kind")?.to_string());
                index += 2;
            }
            "--dry-run" => {
                dry_run = true;
                index += 1;
            }
            "--preview-hash" => {
                preview_hash = Some(next_arg(parts, index, "--preview-hash")?.to_string());
                index += 2;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            value if value.starts_with("--limit=") => {
                limit = value
                    .trim_start_matches("--limit=")
                    .parse()
                    .context("--limit must be an integer")?;
                index += 1;
            }
            value if value.starts_with("--status=") => {
                status = Some(value.trim_start_matches("--status=").to_string());
                index += 1;
            }
            value if value.starts_with("--delivery-kind=") => {
                delivery_kind = Some(value.trim_start_matches("--delivery-kind=").to_string());
                index += 1;
            }
            value if value.starts_with("--preview-hash=") => {
                preview_hash = Some(value.trim_start_matches("--preview-hash=").to_string());
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    anyhow::ensure!(
        (1..=200).contains(&limit),
        "fleet-alert-notification-process --limit must be between 1 and 200"
    );
    if let Some(status) = status.as_deref() {
        anyhow::ensure!(
            matches!(status, "queued" | "failed"),
            "fleet-alert-notification-process --status must be queued or failed"
        );
    }
    if let Some(delivery_kind) = delivery_kind.as_deref() {
        validate_alert_notification_delivery_kind(
            delivery_kind,
            "fleet-alert-notification-process --delivery-kind",
        )?;
    }
    anyhow::ensure!(
        dry_run || confirmed,
        "fleet-alert-notification-process requires --confirmed unless --dry-run is set"
    );
    if !dry_run && confirmed {
        anyhow::ensure!(
            preview_hash
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()),
            "fleet-alert-notification-process requires --preview-hash when --confirmed is set"
        );
    }
    Ok(VtyInventoryCommand::FleetAlertNotificationProcess {
        limit,
        status,
        delivery_kind,
        dry_run,
        preview_hash,
        confirmed,
    })
}

fn parse_csv_tokens(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_bool(value: &str) -> Result<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => anyhow::bail!("boolean value must be true or false"),
    }
}

fn validate_fleet_alert_policy_scope_kind(scope_kind: &str) -> Result<()> {
    anyhow::ensure!(
        matches!(scope_kind, "global" | "provider" | "tag" | "client"),
        "fleet alert policy scope kind must be global, provider, tag, or client"
    );
    Ok(())
}

fn validate_alert_token(value: &str, context: &str) -> Result<()> {
    anyhow::ensure!(
        !value.trim().is_empty()
            && value.len() <= 192
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_' | b'.')
            }),
        "{context} contains unsupported characters"
    );
    Ok(())
}

fn validate_alert_notification_delivery_kind(value: &str, context: &str) -> Result<()> {
    anyhow::ensure!(value.trim() == "webhook", "{context} must be webhook");
    Ok(())
}

fn validate_alert_state(value: &str, context: &str) -> Result<()> {
    anyhow::ensure!(
        matches!(value, "open" | "acknowledged" | "muted" | "escalated"),
        "{context} must be open, acknowledged, muted, or escalated"
    );
    Ok(())
}

fn validate_alert_severity(value: &str, context: &str) -> Result<()> {
    anyhow::ensure!(
        matches!(value, "critical" | "warning" | "info"),
        "{context} must be critical, warning, or info"
    );
    Ok(())
}

fn parse_config_source_filter_args(
    parts: &[&str],
    command_name: &str,
) -> Result<(Option<String>, Option<String>)> {
    let mut client_id = None;
    let mut behavior = None;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--client-id" => {
                client_id = Some(next_arg(parts, index, "--client-id")?.to_string());
                index += 2;
            }
            "--behavior" => {
                behavior = Some(next_arg(parts, index, "--behavior")?.to_string());
                index += 2;
            }
            value if value.starts_with("--client-id=") => {
                client_id = Some(value.trim_start_matches("--client-id=").to_string());
                index += 1;
            }
            value if value.starts_with("--behavior=") => {
                behavior = Some(value.trim_start_matches("--behavior=").to_string());
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    if let Some(client_id) = client_id.as_deref() {
        anyhow::ensure!(
            !client_id.is_empty() && client_id.len() <= 128,
            "{command_name} --client-id must be between 1 and 128 bytes"
        );
    }
    if let Some(behavior) = behavior.as_deref() {
        anyhow::ensure!(
            !behavior.is_empty() && behavior.len() <= 128,
            "{command_name} --behavior must be between 1 and 128 bytes"
        );
    }
    Ok((client_id, behavior))
}

fn parse_config_render(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let mut client_id = None;
    let mut format = "toml".to_string();
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--client-id" => {
                client_id = Some(next_arg(parts, index, "--client-id")?.to_string());
                index += 2;
            }
            "--format" => {
                format = next_arg(parts, index, "--format")?.to_string();
                index += 2;
            }
            value if value.starts_with("--client-id=") => {
                client_id = Some(value.trim_start_matches("--client-id=").to_string());
                index += 1;
            }
            value if value.starts_with("--format=") => {
                format = value.trim_start_matches("--format=").to_string();
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    anyhow::ensure!(
        matches!(format.as_str(), "toml" | "json"),
        "--format must be toml or json"
    );
    Ok(VtyInventoryCommand::ConfigRender {
        client_id: client_id.context("config-render requires --client-id")?,
        format,
    })
}

fn parse_config_source_change(parts: &[&str], action: &str) -> Result<VtyInventoryCommand> {
    let mut behavior = None;
    let mut preset_id = None;
    let mut selector = None;
    let mut clients = Vec::new();
    let mut tags = Vec::new();
    let mut preview_hash = None;
    let mut confirmed = false;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--behavior" => {
                behavior = Some(next_arg(parts, index, "--behavior")?.to_string());
                index += 2;
            }
            "--preset-id" if action == "set" => {
                preset_id = Some(next_arg(parts, index, "--preset-id")?.to_string());
                index += 2;
            }
            "--selector" => {
                selector = Some(next_arg(parts, index, "--selector")?.to_string());
                index += 2;
            }
            "--client" => {
                clients.push(next_arg(parts, index, "--client")?.to_string());
                index += 2;
            }
            "--tag" => {
                tags.push(next_arg(parts, index, "--tag")?.to_string());
                index += 2;
            }
            "--preview-hash" => {
                preview_hash = Some(next_arg(parts, index, "--preview-hash")?.to_string());
                index += 2;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            value if value.starts_with("--behavior=") => {
                behavior = Some(value.trim_start_matches("--behavior=").to_string());
                index += 1;
            }
            value if value.starts_with("--preset-id=") && action == "set" => {
                preset_id = Some(value.trim_start_matches("--preset-id=").to_string());
                index += 1;
            }
            value if value.starts_with("--selector=") => {
                selector = Some(value.trim_start_matches("--selector=").to_string());
                index += 1;
            }
            value if value.starts_with("--client=") => {
                clients.push(value.trim_start_matches("--client=").to_string());
                index += 1;
            }
            value if value.starts_with("--tag=") => {
                tags.push(value.trim_start_matches("--tag=").to_string());
                index += 1;
            }
            value if value.starts_with("--preview-hash=") => {
                preview_hash = Some(value.trim_start_matches("--preview-hash=").to_string());
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    if action == "set" {
        anyhow::ensure!(
            preset_id.is_some(),
            "config-source-set requires --preset-id"
        );
    }
    commands_inventory::reviewed_preview_hash_arg(
        confirmed,
        preview_hash.as_deref(),
        &format!("config-source-{action}"),
    )?;
    Ok(VtyInventoryCommand::ConfigSourceChange {
        action: action.to_string(),
        behavior: behavior
            .with_context(|| format!("config-source-{action} requires --behavior"))?,
        preset_id,
        selector,
        clients,
        tags,
        preview_hash,
        confirmed,
    })
}

fn next_arg<'a>(parts: &'a [&str], index: usize, flag: &str) -> Result<&'a str> {
    parts
        .get(index + 1)
        .copied()
        .with_context(|| format!("{flag} requires a value"))
}

fn parse_telemetry_rollups_args(
    parts: &[&str],
) -> Result<(u16, Option<String>, Option<i32>, bool)> {
    let mut limit = 50_u16;
    let mut client_id = None;
    let mut bucket_secs = None;
    let mut latest = false;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--limit" => {
                limit = parts
                    .get(index + 1)
                    .context("--limit requires a value")?
                    .parse()
                    .context("--limit must be an integer")?;
                index += 2;
            }
            "--client-id" => {
                let value = parts
                    .get(index + 1)
                    .context("--client-id requires a value")?;
                client_id = Some((*value).to_string());
                index += 2;
            }
            "--bucket-secs" => {
                bucket_secs = Some(
                    parts
                        .get(index + 1)
                        .context("--bucket-secs requires a value")?
                        .parse()
                        .context("--bucket-secs must be an integer")?,
                );
                index += 2;
            }
            "--latest" => {
                latest = true;
                index += 1;
            }
            value if value.starts_with("--limit=") => {
                limit = value
                    .trim_start_matches("--limit=")
                    .parse()
                    .context("--limit must be an integer")?;
                index += 1;
            }
            value if value.starts_with("--client-id=") => {
                client_id = Some(value.trim_start_matches("--client-id=").to_string());
                index += 1;
            }
            value if value.starts_with("--bucket-secs=") => {
                bucket_secs = Some(
                    value
                        .trim_start_matches("--bucket-secs=")
                        .parse()
                        .context("--bucket-secs must be an integer")?,
                );
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    anyhow::ensure!(
        (1..=200).contains(&limit),
        "telemetry-rollups --limit must be between 1 and 200"
    );
    if let Some(client_id) = client_id.as_deref() {
        anyhow::ensure!(
            !client_id.is_empty() && client_id.len() <= 128,
            "telemetry-rollups --client-id must be between 1 and 128 bytes"
        );
    }
    if let Some(bucket_secs) = bucket_secs {
        anyhow::ensure!(
            bucket_secs >= 60 && bucket_secs % 60 == 0,
            "telemetry-rollups --bucket-secs must be at least 60 and divisible by 60"
        );
    }
    Ok((limit, client_id, bucket_secs, latest))
}

fn parse_telemetry_network_rates_args(parts: &[&str]) -> Result<TelemetryNetworkRateArgs> {
    let mut limit = 50_u16;
    let mut client_id = None;
    let mut interface = None;
    let mut bucket_secs = None;
    let mut latest = false;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--limit" => {
                limit = parts
                    .get(index + 1)
                    .context("--limit requires a value")?
                    .parse()
                    .context("--limit must be an integer")?;
                index += 2;
            }
            "--client-id" => {
                client_id = Some(
                    (*parts
                        .get(index + 1)
                        .context("--client-id requires a value")?)
                    .to_string(),
                );
                index += 2;
            }
            "--interface" => {
                interface = Some(
                    (*parts
                        .get(index + 1)
                        .context("--interface requires a value")?)
                    .to_string(),
                );
                index += 2;
            }
            "--bucket-secs" => {
                bucket_secs = Some(
                    parts
                        .get(index + 1)
                        .context("--bucket-secs requires a value")?
                        .parse()
                        .context("--bucket-secs must be an integer")?,
                );
                index += 2;
            }
            "--latest" => {
                latest = true;
                index += 1;
            }
            value if value.starts_with("--limit=") => {
                limit = value
                    .trim_start_matches("--limit=")
                    .parse()
                    .context("--limit must be an integer")?;
                index += 1;
            }
            value if value.starts_with("--client-id=") => {
                client_id = Some(value.trim_start_matches("--client-id=").to_string());
                index += 1;
            }
            value if value.starts_with("--interface=") => {
                interface = Some(value.trim_start_matches("--interface=").to_string());
                index += 1;
            }
            value if value.starts_with("--bucket-secs=") => {
                bucket_secs = Some(
                    value
                        .trim_start_matches("--bucket-secs=")
                        .parse()
                        .context("--bucket-secs must be an integer")?,
                );
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    anyhow::ensure!(
        (1..=5_000).contains(&limit),
        "telemetry-network-rates --limit must be between 1 and 5000"
    );
    if let Some(client_id) = client_id.as_deref() {
        anyhow::ensure!(
            !client_id.is_empty() && client_id.len() <= 128,
            "telemetry-network-rates --client-id must be between 1 and 128 bytes"
        );
    }
    if let Some(interface) = interface.as_deref() {
        anyhow::ensure!(
            !interface.is_empty() && interface.len() <= 64,
            "telemetry-network-rates --interface must be between 1 and 64 bytes"
        );
    }
    if let Some(bucket_secs) = bucket_secs {
        anyhow::ensure!(
            bucket_secs >= 60 && bucket_secs % 60 == 0,
            "telemetry-network-rates --bucket-secs must be at least 60 and divisible by 60"
        );
    }
    Ok(TelemetryNetworkRateArgs {
        limit,
        client_id,
        interface,
        bucket_secs,
        latest,
    })
}

fn parse_telemetry_tunnels_args(parts: &[&str]) -> Result<TelemetryTunnelArgs> {
    let mut limit = 50_u16;
    let mut client_id = None;
    let mut interface = None;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--limit" => {
                limit = parts
                    .get(index + 1)
                    .context("--limit requires a value")?
                    .parse()
                    .context("--limit must be an integer")?;
                index += 2;
            }
            "--client-id" => {
                client_id = Some(
                    (*parts
                        .get(index + 1)
                        .context("--client-id requires a value")?)
                    .to_string(),
                );
                index += 2;
            }
            "--interface" => {
                interface = Some(
                    (*parts
                        .get(index + 1)
                        .context("--interface requires a value")?)
                    .to_string(),
                );
                index += 2;
            }
            value if value.starts_with("--limit=") => {
                limit = value
                    .trim_start_matches("--limit=")
                    .parse()
                    .context("--limit must be an integer")?;
                index += 1;
            }
            value if value.starts_with("--client-id=") => {
                client_id = Some(value.trim_start_matches("--client-id=").to_string());
                index += 1;
            }
            value if value.starts_with("--interface=") => {
                interface = Some(value.trim_start_matches("--interface=").to_string());
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    anyhow::ensure!(
        (1..=200).contains(&limit),
        "telemetry-tunnels --limit must be between 1 and 200"
    );
    if let Some(client_id) = client_id.as_deref() {
        anyhow::ensure!(
            !client_id.is_empty() && client_id.len() <= 128,
            "telemetry-tunnels --client-id must be between 1 and 128 bytes"
        );
    }
    if let Some(interface) = interface.as_deref() {
        anyhow::ensure!(
            !interface.is_empty() && interface.len() <= 64,
            "telemetry-tunnels --interface must be between 1 and 64 bytes"
        );
    }
    Ok(TelemetryTunnelArgs {
        limit,
        client_id,
        interface,
    })
}

fn telemetry_rollups_path(
    limit: u16,
    client_id: Option<&str>,
    bucket_secs: Option<i32>,
    latest: bool,
) -> String {
    let mut path = format!("/api/v1/telemetry/rollups?limit={limit}");
    if let Some(client_id) = client_id {
        path.push_str("&client_id=");
        path.push_str(&percent_encode_query_value(client_id));
    }
    if let Some(bucket_secs) = bucket_secs {
        path.push_str("&bucket_secs=");
        path.push_str(&bucket_secs.to_string());
    }
    if latest {
        path.push_str("&latest=true");
    }
    path
}

fn telemetry_network_rates_path(
    limit: u16,
    client_id: Option<&str>,
    interface: Option<&str>,
    bucket_secs: Option<i32>,
    latest: bool,
) -> String {
    let mut path = format!("/api/v1/telemetry/network-rates?limit={limit}");
    if let Some(client_id) = client_id {
        path.push_str("&client_id=");
        path.push_str(&percent_encode_query_value(client_id));
    }
    if let Some(interface) = interface {
        path.push_str("&interface=");
        path.push_str(&percent_encode_query_value(interface));
    }
    if let Some(bucket_secs) = bucket_secs {
        path.push_str("&bucket_secs=");
        path.push_str(&bucket_secs.to_string());
    }
    if latest {
        path.push_str("&latest=true");
    }
    path
}

fn telemetry_tunnels_path(limit: u16, client_id: Option<&str>, interface: Option<&str>) -> String {
    let mut path = format!("/api/v1/telemetry/tunnels?limit={limit}");
    if let Some(client_id) = client_id {
        path.push_str("&client_id=");
        path.push_str(&percent_encode_query_value(client_id));
    }
    if let Some(interface) = interface {
        path.push_str("&interface=");
        path.push_str(&percent_encode_query_value(interface));
    }
    path
}

fn fleet_alerts_path(
    limit: u16,
    client_id: Option<&str>,
    severity: Option<&str>,
    category: Option<&str>,
    operator_state: Option<&str>,
    include_muted: bool,
) -> String {
    let mut path = format!("/api/v1/fleet-alerts?limit={limit}");
    if let Some(client_id) = client_id {
        path.push_str("&client_id=");
        path.push_str(&percent_encode_query_value(client_id));
    }
    if let Some(severity) = severity {
        path.push_str("&severity=");
        path.push_str(severity);
    }
    if let Some(category) = category {
        path.push_str("&category=");
        path.push_str(&percent_encode_query_value(category));
    }
    if let Some(operator_state) = operator_state {
        path.push_str("&operator_state=");
        path.push_str(operator_state);
    }
    if include_muted {
        path.push_str("&include_muted=true");
    }
    path
}

fn fleet_alert_export_path(
    limit: u16,
    client_id: Option<&str>,
    severity: Option<&str>,
    category: Option<&str>,
    operator_state: Option<&str>,
    include_muted: bool,
) -> String {
    fleet_alerts_path(
        limit,
        client_id,
        severity,
        category,
        operator_state,
        include_muted,
    )
    .replacen("/api/v1/fleet-alerts?", "/api/v1/fleet-alerts/export?", 1)
}

fn fleet_alert_states_path(limit: u16, state: Option<&str>) -> String {
    let mut path = format!("/api/v1/fleet-alert-states?limit={limit}");
    if let Some(state) = state {
        path.push_str("&state=");
        path.push_str(state);
    }
    path
}

fn vps_rules_path(
    limit: u16,
    selector: Option<&str>,
    client_id: Option<&str>,
    key: Option<&str>,
    state: Option<&str>,
) -> String {
    let mut path = format!("/api/v1/vps-rules?limit={limit}");
    if let Some(selector) = selector {
        path.push_str("&selector_expression=");
        path.push_str(&percent_encode_query_value(selector));
    }
    if let Some(client_id) = client_id {
        path.push_str("&client_id=");
        path.push_str(&percent_encode_query_value(client_id));
    }
    if let Some(key) = key {
        path.push_str("&key=");
        path.push_str(&percent_encode_query_value(key));
    }
    if let Some(state) = state {
        path.push_str("&state=");
        path.push_str(&percent_encode_query_value(state));
    }
    path
}

fn alert_policies_path(
    limit: u16,
    enabled: Option<bool>,
    selector: Option<&str>,
    client_id: Option<&str>,
) -> String {
    let mut path = format!("/api/v1/fleet-alert-policies?limit={limit}");
    if let Some(enabled) = enabled {
        path.push_str("&enabled=");
        path.push_str(if enabled { "true" } else { "false" });
    }
    if let Some(selector) = selector {
        path.push_str("&selector_expression=");
        path.push_str(&percent_encode_query_value(selector));
    }
    if let Some(client_id) = client_id {
        path.push_str("&client_id=");
        path.push_str(&percent_encode_query_value(client_id));
    }
    path
}

fn fleet_alert_notification_channels_path(
    limit: u16,
    enabled: Option<bool>,
    scope_kind: Option<&str>,
    scope_value: Option<&str>,
    delivery_kind: Option<&str>,
) -> String {
    let mut path = format!("/api/v1/fleet-alert-notification-channels?limit={limit}");
    if let Some(enabled) = enabled {
        path.push_str("&enabled=");
        path.push_str(if enabled { "true" } else { "false" });
    }
    if let Some(scope_kind) = scope_kind {
        path.push_str("&scope_kind=");
        path.push_str(scope_kind);
    }
    if let Some(scope_value) = scope_value {
        path.push_str("&scope_value=");
        path.push_str(&percent_encode_query_value(scope_value));
    }
    if let Some(delivery_kind) = delivery_kind {
        path.push_str("&delivery_kind=");
        path.push_str(&percent_encode_query_value(delivery_kind));
    }
    path
}

fn fleet_alert_notifications_path(
    limit: u16,
    channel_id: Option<&str>,
    alert_id: Option<&str>,
    status: Option<&str>,
) -> String {
    let mut path = format!("/api/v1/fleet-alert-notifications?limit={limit}");
    if let Some(channel_id) = channel_id {
        path.push_str("&channel_id=");
        path.push_str(channel_id);
    }
    if let Some(alert_id) = alert_id {
        path.push_str("&alert_id=");
        path.push_str(&percent_encode_query_value(alert_id));
    }
    if let Some(status) = status {
        path.push_str("&status=");
        path.push_str(status);
    }
    path
}

fn config_presets_path(behavior: Option<&str>) -> String {
    match behavior {
        Some(behavior) => format!(
            "/api/v1/configuration-presets?behavior={}",
            percent_encode_query_value(behavior)
        ),
        None => "/api/v1/configuration-presets".to_string(),
    }
}

fn config_sources_path(client_id: Option<&str>, behavior: Option<&str>) -> String {
    let mut query = Vec::new();
    if let Some(client_id) = client_id {
        query.push(format!(
            "client_id={}",
            percent_encode_query_value(client_id)
        ));
    }
    if let Some(behavior) = behavior {
        query.push(format!("behavior={}", percent_encode_query_value(behavior)));
    }
    if query.is_empty() {
        "/api/v1/configuration-sources".to_string()
    } else {
        format!("/api/v1/configuration-sources?{}", query.join("&"))
    }
}

fn config_render_path(client_id: &str) -> String {
    format!(
        "/api/v1/effective-agent-config?client_id={}",
        percent_encode_query_value(client_id)
    )
}

#[cfg(test)]
#[path = "tests_vty_inventory.rs"]
mod tests;
