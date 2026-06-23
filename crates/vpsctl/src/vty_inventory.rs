use anyhow::{Context, Result};

use crate::util::percent_encode_query_value;
use crate::{
    commands_inventory,
    commands_schedules::{resolve_schedule_target_ids, selector_expression_from_targets},
    http::{http_get, http_post_json},
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
    SourceTemplates {
        domain: Option<String>,
    },
    SourceTemplateCreate {
        domain: String,
        name: String,
        scope: String,
        owner_client_id: Option<String>,
        description: Option<String>,
        definition: serde_json::Value,
    },
    SourceTemplateClone {
        source_template_id: String,
        name: String,
        scope: String,
        owner_client_id: Option<String>,
        description: Option<String>,
    },
    SourceTemplateDiff {
        template_id: String,
        description: Option<String>,
        clear_description: bool,
        definition: serde_json::Value,
    },
    SourceTemplateTest {
        template_id: String,
        definition: serde_json::Value,
    },
    SourceTemplateUpdate {
        template_id: String,
        description: Option<String>,
        clear_description: bool,
        definition: serde_json::Value,
        confirmed: bool,
    },
    SourceStatus {
        client_id: Option<String>,
        domain: Option<String>,
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
        rules: Vec<String>,
        window_secs: i64,
        severity: String,
        traffic_selector: Option<String>,
        enabled: bool,
        notes: Option<String>,
    },
    AlertPolicyUpsert {
        name: String,
        selector: String,
        rules: Vec<String>,
        window_secs: i64,
        severity: String,
        traffic_selector: Option<String>,
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
        confirmed: bool,
    },
    FleetAlertNotificationProcess {
        limit: u16,
        status: Option<String>,
        delivery_kind: Option<String>,
        dry_run: bool,
        confirmed: bool,
    },
    SourceTemplateAssignments {
        client_id: Option<String>,
        domain: Option<String>,
    },
    TemplateRuntimeConfig {
        client_id: String,
        format: String,
    },
    SourceTemplateAssign {
        domain: String,
        template_id: String,
        clients: Vec<String>,
        tags: Vec<String>,
        confirmed: bool,
    },
    BulkResolve {
        tags: Vec<String>,
    },
    TelemetryRollups {
        limit: u16,
        client_id: Option<String>,
        bucket_secs: Option<i32>,
    },
    TelemetryNetworkRates {
        limit: u16,
        client_id: Option<String>,
        interface: Option<String>,
        bucket_secs: Option<i32>,
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
            | "source-templates"
            | "source-template-create"
            | "source-template-clone"
            | "source-template-diff"
            | "source-template-test"
            | "source-template-update"
            | "source-status"
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
            | "source-template-assignments"
            | "template-runtime-config"
            | "source-template-assign"
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
        VtyInventoryCommand::SourceTemplates { domain } => {
            http_get(api_url, &source_templates_path(domain.as_deref()), token)
        }
        VtyInventoryCommand::SourceTemplateCreate {
            domain,
            name,
            scope,
            owner_client_id,
            description,
            definition,
        } => http_post_json(
            api_url,
            "/api/v1/source-templates",
            token,
            &serde_json::json!({
                "domain": domain,
                "name": name,
                "scope": scope,
                "owner_client_id": owner_client_id,
                "description": description,
                "definition": definition,
            }),
        ),
        VtyInventoryCommand::SourceTemplateClone {
            source_template_id,
            name,
            scope,
            owner_client_id,
            description,
        } => http_post_json(
            api_url,
            &format!("/api/v1/source-templates/{source_template_id}/clone"),
            token,
            &serde_json::json!({
                "name": name,
                "scope": scope,
                "owner_client_id": owner_client_id,
                "description": description,
            }),
        ),
        VtyInventoryCommand::SourceTemplateDiff {
            template_id,
            description,
            clear_description,
            definition,
        } => {
            let keep_description = description.is_none() && !clear_description;
            http_post_json(
                api_url,
                &format!("/api/v1/source-templates/{template_id}/diff"),
                token,
                &serde_json::json!({
                    "description": description,
                    "definition": definition,
                    "keep_description": keep_description,
                }),
            )
        }
        VtyInventoryCommand::SourceTemplateTest {
            template_id,
            definition,
        } => http_post_json(
            api_url,
            &format!("/api/v1/source-templates/{template_id}/test"),
            token,
            &serde_json::json!({
                "definition": definition,
            }),
        ),
        VtyInventoryCommand::SourceTemplateUpdate {
            template_id,
            description,
            clear_description,
            definition,
            confirmed,
        } => {
            let keep_description = description.is_none() && !clear_description;
            http_post_json(
                api_url,
                &format!("/api/v1/source-templates/{template_id}/update"),
                token,
                &serde_json::json!({
                    "description": description,
                    "definition": definition,
                    "confirmed": confirmed,
                    "keep_description": keep_description,
                }),
            )
        }
        VtyInventoryCommand::SourceStatus { client_id, domain } => http_get(
            api_url,
            &source_status_path(client_id.as_deref(), domain.as_deref()),
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
            rules,
            window_secs,
            severity,
            traffic_selector,
            enabled,
            notes,
        } => {
            let request = commands_inventory::alert_policy_request(
                commands_inventory::AlertPolicyWriteOptions {
                    name,
                    selector: Some(selector),
                    rules,
                    window_secs,
                    severity,
                    traffic_selector,
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
            rules,
            window_secs,
            severity,
            traffic_selector,
            enabled,
            notes,
            confirmed,
        } => {
            let mut request = commands_inventory::alert_policy_request(
                commands_inventory::AlertPolicyWriteOptions {
                    name,
                    selector: Some(selector),
                    rules,
                    window_secs,
                    severity,
                    traffic_selector,
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
                "confirmed": confirmed,
            }),
        ),
        VtyInventoryCommand::FleetAlertNotificationProcess {
            limit,
            status,
            delivery_kind,
            dry_run,
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
                "confirmed": confirmed,
            }),
        ),
        VtyInventoryCommand::SourceTemplateAssignments { client_id, domain } => http_get(
            api_url,
            &source_template_assignments_path(client_id.as_deref(), domain.as_deref()),
            token,
        ),
        VtyInventoryCommand::TemplateRuntimeConfig { client_id, format } => {
            let body = http_get(api_url, &template_runtime_config_path(&client_id), token)?;
            match format.as_str() {
                "json" => Ok(body),
                "toml" => {
                    let value: serde_json::Value = serde_json::from_str(&body)
                        .context("invalid source template config response")?;
                    Ok(value
                        .get("toml")
                        .and_then(serde_json::Value::as_str)
                        .context("source template config response missing toml")?
                        .to_string())
                }
                _ => anyhow::bail!("--format must be toml or json"),
            }
        }
        VtyInventoryCommand::SourceTemplateAssign {
            domain,
            template_id,
            clients,
            tags,
            confirmed,
        } => {
            let selector_expression = selector_expression_from_targets(&clients, &tags);
            let target_client_ids =
                resolve_schedule_target_ids(api_url, token, &selector_expression)?;
            http_post_json(
                api_url,
                "/api/v1/source-template-assignments",
                token,
                &serde_json::json!({
                    "domain": domain,
                    "template_id": template_id,
                    "selector_expression": selector_expression,
                    "target_client_ids": target_client_ids,
                    "confirmed": confirmed,
                }),
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
        } => http_get(
            api_url,
            &telemetry_rollups_path(limit, client_id.as_deref(), bucket_secs),
            token,
        ),
        VtyInventoryCommand::TelemetryNetworkRates {
            limit,
            client_id,
            interface,
            bucket_secs,
        } => http_get(
            api_url,
            &telemetry_network_rates_path(
                limit,
                client_id.as_deref(),
                interface.as_deref(),
                bucket_secs,
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
    let parts = command.split_whitespace().collect::<Vec<_>>();
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
        "source-templates" => {
            let mut domain = None;
            let mut index = 1;
            while index < parts.len() {
                match parts[index] {
                    "--domain" => {
                        domain = Some(
                            (*parts.get(index + 1).context("--domain requires a value")?)
                                .to_string(),
                        );
                        index += 2;
                    }
                    value if value.starts_with("--domain=") => {
                        domain = Some(value.trim_start_matches("--domain=").to_string());
                        index += 1;
                    }
                    value => anyhow::bail!("unexpected argument {value}"),
                }
            }
            Ok(VtyInventoryCommand::SourceTemplates { domain })
        }
        "source-template-create" => parse_source_template_create(&parts),
        "source-template-clone" => parse_source_template_clone(&parts),
        "source-template-diff" => parse_source_template_diff(&parts),
        "source-template-test" => parse_source_template_test(&parts),
        "source-template-update" => parse_source_template_update(&parts),
        "source-status" => parse_source_status(&parts),
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
        "source-template-assignments" => parse_source_template_assignments(&parts),
        "template-runtime-config" => parse_template_runtime_config(&parts),
        "source-template-assign" => parse_source_template_assign(&parts),
        "bulk-resolve" => Ok(VtyInventoryCommand::BulkResolve {
            tags: parts
                .iter()
                .skip(1)
                .map(|value| (*value).to_string())
                .collect(),
        }),
        "telemetry-rollups" => {
            let (limit, client_id, bucket_secs) = parse_telemetry_rollups_args(&parts)?;
            Ok(VtyInventoryCommand::TelemetryRollups {
                limit,
                client_id,
                bucket_secs,
            })
        }
        "telemetry-network-rates" => {
            let args = parse_telemetry_network_rates_args(&parts)?;
            Ok(VtyInventoryCommand::TelemetryNetworkRates {
                limit: args.limit,
                client_id: args.client_id,
                interface: args.interface,
                bucket_secs: args.bucket_secs,
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

fn parse_source_template_create(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let mut domain = None;
    let mut name = None;
    let mut scope = "shared".to_string();
    let mut owner_client_id = None;
    let mut description = None;
    let mut definition = serde_json::json!({});
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--domain" => {
                domain = Some(next_arg(parts, index, "--domain")?.to_string());
                index += 2;
            }
            "--name" => {
                name = Some(next_arg(parts, index, "--name")?.to_string());
                index += 2;
            }
            "--scope" => {
                scope = next_arg(parts, index, "--scope")?.to_string();
                index += 2;
            }
            "--owner-client-id" => {
                owner_client_id = Some(next_arg(parts, index, "--owner-client-id")?.to_string());
                index += 2;
            }
            "--description" => {
                description = Some(next_arg(parts, index, "--description")?.to_string());
                index += 2;
            }
            "--definition-json" => {
                definition = serde_json::from_str(next_arg(parts, index, "--definition-json")?)
                    .context("invalid --definition-json")?;
                index += 2;
            }
            value if value.starts_with("--domain=") => {
                domain = Some(value.trim_start_matches("--domain=").to_string());
                index += 1;
            }
            value if value.starts_with("--name=") => {
                name = Some(value.trim_start_matches("--name=").to_string());
                index += 1;
            }
            value if value.starts_with("--scope=") => {
                scope = value.trim_start_matches("--scope=").to_string();
                index += 1;
            }
            value if value.starts_with("--owner-client-id=") => {
                owner_client_id = Some(value.trim_start_matches("--owner-client-id=").to_string());
                index += 1;
            }
            value if value.starts_with("--description=") => {
                description = Some(value.trim_start_matches("--description=").to_string());
                index += 1;
            }
            value if value.starts_with("--definition-json=") => {
                definition = serde_json::from_str(value.trim_start_matches("--definition-json="))
                    .context("invalid --definition-json")?;
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    Ok(VtyInventoryCommand::SourceTemplateCreate {
        domain: domain.context("source-template-create requires --domain")?,
        name: name.context("source-template-create requires --name")?,
        scope,
        owner_client_id,
        description,
        definition,
    })
}

fn parse_source_template_clone(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let mut source_template_id = None;
    let mut name = None;
    let mut scope = "shared".to_string();
    let mut owner_client_id = None;
    let mut description = None;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--template-id" => {
                source_template_id = Some(next_arg(parts, index, "--template-id")?.to_string());
                index += 2;
            }
            "--name" => {
                name = Some(next_arg(parts, index, "--name")?.to_string());
                index += 2;
            }
            "--scope" => {
                scope = next_arg(parts, index, "--scope")?.to_string();
                index += 2;
            }
            "--owner-client-id" => {
                owner_client_id = Some(next_arg(parts, index, "--owner-client-id")?.to_string());
                index += 2;
            }
            "--description" => {
                description = Some(next_arg(parts, index, "--description")?.to_string());
                index += 2;
            }
            value if value.starts_with("--template-id=") => {
                source_template_id = Some(value.trim_start_matches("--template-id=").to_string());
                index += 1;
            }
            value if value.starts_with("--name=") => {
                name = Some(value.trim_start_matches("--name=").to_string());
                index += 1;
            }
            value if value.starts_with("--scope=") => {
                scope = value.trim_start_matches("--scope=").to_string();
                index += 1;
            }
            value if value.starts_with("--owner-client-id=") => {
                owner_client_id = Some(value.trim_start_matches("--owner-client-id=").to_string());
                index += 1;
            }
            value if value.starts_with("--description=") => {
                description = Some(value.trim_start_matches("--description=").to_string());
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    Ok(VtyInventoryCommand::SourceTemplateClone {
        source_template_id: source_template_id
            .context("source-template-clone requires --template-id")?,
        name: name.context("source-template-clone requires --name")?,
        scope,
        owner_client_id,
        description,
    })
}

fn parse_source_template_diff(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let (template_id, description, clear_description, definition, confirmed) =
        parse_source_template_candidate_args(parts, "source-template-diff")?;
    anyhow::ensure!(
        !confirmed,
        "source-template-diff does not accept --confirmed"
    );
    Ok(VtyInventoryCommand::SourceTemplateDiff {
        template_id,
        description,
        clear_description,
        definition,
    })
}

fn parse_source_template_test(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let mut template_id = None;
    let mut definition = serde_json::json!({});
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--template-id" => {
                template_id = Some(next_arg(parts, index, "--template-id")?.to_string());
                index += 2;
            }
            "--definition-json" => {
                definition = serde_json::from_str(next_arg(parts, index, "--definition-json")?)
                    .context("invalid --definition-json")?;
                index += 2;
            }
            value if value.starts_with("--template-id=") => {
                template_id = Some(value.trim_start_matches("--template-id=").to_string());
                index += 1;
            }
            value if value.starts_with("--definition-json=") => {
                definition = serde_json::from_str(value.trim_start_matches("--definition-json="))
                    .context("invalid --definition-json")?;
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    Ok(VtyInventoryCommand::SourceTemplateTest {
        template_id: template_id.context("source-template-test requires --template-id")?,
        definition,
    })
}

fn parse_source_template_update(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let (template_id, description, clear_description, definition, confirmed) =
        parse_source_template_candidate_args(parts, "source-template-update")?;
    Ok(VtyInventoryCommand::SourceTemplateUpdate {
        template_id,
        description,
        clear_description,
        definition,
        confirmed,
    })
}

fn parse_source_template_candidate_args(
    parts: &[&str],
    command_name: &str,
) -> Result<(String, Option<String>, bool, serde_json::Value, bool)> {
    let mut template_id = None;
    let mut description = None;
    let mut clear_description = false;
    let mut definition = serde_json::json!({});
    let mut confirmed = false;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--template-id" => {
                template_id = Some(next_arg(parts, index, "--template-id")?.to_string());
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
                definition = serde_json::from_str(next_arg(parts, index, "--definition-json")?)
                    .context("invalid --definition-json")?;
                index += 2;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            value if value.starts_with("--template-id=") => {
                template_id = Some(value.trim_start_matches("--template-id=").to_string());
                index += 1;
            }
            value if value.starts_with("--description=") => {
                description = Some(value.trim_start_matches("--description=").to_string());
                index += 1;
            }
            value if value.starts_with("--definition-json=") => {
                definition = serde_json::from_str(value.trim_start_matches("--definition-json="))
                    .context("invalid --definition-json")?;
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    anyhow::ensure!(
        description.is_none() || !clear_description,
        "use only one of --description or --clear-description"
    );
    Ok((
        template_id.with_context(|| format!("{command_name} requires --template-id"))?,
        description,
        clear_description,
        definition,
        confirmed,
    ))
}

fn parse_source_template_assignments(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let (client_id, domain) =
        parse_source_template_filter_args(parts, "source-template-assignments")?;
    Ok(VtyInventoryCommand::SourceTemplateAssignments { client_id, domain })
}

fn parse_source_status(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let (client_id, domain) = parse_source_template_filter_args(parts, "source-status")?;
    Ok(VtyInventoryCommand::SourceStatus { client_id, domain })
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
    let mut rules = Vec::new();
    let mut window_secs = 0_i64;
    let mut severity = "warning".to_string();
    let mut traffic_selector = None;
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
            "--rule" => {
                rules.push(next_arg(parts, index, "--rule")?.to_string());
                index += 2;
            }
            "--window-secs" => {
                window_secs = next_arg(parts, index, "--window-secs")?
                    .parse()
                    .context("--window-secs must be an integer")?;
                index += 2;
            }
            "--severity" => {
                severity = next_arg(parts, index, "--severity")?.to_string();
                index += 2;
            }
            "--traffic-selector" => {
                traffic_selector = Some(next_arg(parts, index, "--traffic-selector")?.to_string());
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
            value if value.starts_with("--rule=") => {
                rules.push(value.trim_start_matches("--rule=").to_string());
                index += 1;
            }
            value if value.starts_with("--window-secs=") => {
                window_secs = value
                    .trim_start_matches("--window-secs=")
                    .parse()
                    .context("--window-secs must be an integer")?;
                index += 1;
            }
            value if value.starts_with("--severity=") => {
                severity = value.trim_start_matches("--severity=").to_string();
                index += 1;
            }
            value if value.starts_with("--traffic-selector=") => {
                traffic_selector =
                    Some(value.trim_start_matches("--traffic-selector=").to_string());
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
        !rules.is_empty(),
        "alert-policy command requires at least one --rule"
    );
    let name = name.context("alert-policy command requires --name")?;
    let selector = selector.context("alert-policy command requires --selector")?;
    if apply {
        Ok(VtyInventoryCommand::AlertPolicyUpsert {
            name,
            selector,
            rules,
            window_secs,
            severity,
            traffic_selector,
            enabled,
            notes,
            confirmed,
        })
    } else {
        Ok(VtyInventoryCommand::AlertPolicyPreview {
            name,
            selector,
            rules,
            window_secs,
            severity,
            traffic_selector,
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
    Ok(VtyInventoryCommand::FleetAlertNotificationDispatch {
        limit,
        client_id,
        severity,
        category,
        operator_state,
        include_muted,
        dry_run,
        confirmed,
    })
}

fn parse_fleet_alert_notification_process(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let mut limit = 50_u16;
    let mut status = None;
    let mut delivery_kind = None;
    let mut dry_run = false;
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
    Ok(VtyInventoryCommand::FleetAlertNotificationProcess {
        limit,
        status,
        delivery_kind,
        dry_run,
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

fn parse_source_template_filter_args(
    parts: &[&str],
    command_name: &str,
) -> Result<(Option<String>, Option<String>)> {
    let mut client_id = None;
    let mut domain = None;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--client-id" => {
                client_id = Some(next_arg(parts, index, "--client-id")?.to_string());
                index += 2;
            }
            "--domain" => {
                domain = Some(next_arg(parts, index, "--domain")?.to_string());
                index += 2;
            }
            value if value.starts_with("--client-id=") => {
                client_id = Some(value.trim_start_matches("--client-id=").to_string());
                index += 1;
            }
            value if value.starts_with("--domain=") => {
                domain = Some(value.trim_start_matches("--domain=").to_string());
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
    if let Some(domain) = domain.as_deref() {
        anyhow::ensure!(
            !domain.is_empty() && domain.len() <= 128,
            "{command_name} --domain must be between 1 and 128 bytes"
        );
    }
    Ok((client_id, domain))
}

fn parse_template_runtime_config(parts: &[&str]) -> Result<VtyInventoryCommand> {
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
    Ok(VtyInventoryCommand::TemplateRuntimeConfig {
        client_id: client_id.context("template-runtime-config requires --client-id")?,
        format,
    })
}

fn parse_source_template_assign(parts: &[&str]) -> Result<VtyInventoryCommand> {
    let mut domain = None;
    let mut template_id = None;
    let mut clients = Vec::new();
    let mut tags = Vec::new();
    let mut confirmed = false;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--domain" => {
                domain = Some(next_arg(parts, index, "--domain")?.to_string());
                index += 2;
            }
            "--template-id" => {
                template_id = Some(next_arg(parts, index, "--template-id")?.to_string());
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
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            value if value.starts_with("--domain=") => {
                domain = Some(value.trim_start_matches("--domain=").to_string());
                index += 1;
            }
            value if value.starts_with("--template-id=") => {
                template_id = Some(value.trim_start_matches("--template-id=").to_string());
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
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    Ok(VtyInventoryCommand::SourceTemplateAssign {
        domain: domain.context("source-template-assign requires --domain")?,
        template_id: template_id.context("source-template-assign requires --template-id")?,
        clients,
        tags,
        confirmed,
    })
}

fn next_arg<'a>(parts: &'a [&str], index: usize, flag: &str) -> Result<&'a str> {
    parts
        .get(index + 1)
        .copied()
        .with_context(|| format!("{flag} requires a value"))
}

fn parse_telemetry_rollups_args(parts: &[&str]) -> Result<(u16, Option<String>, Option<i32>)> {
    let mut limit = 50_u16;
    let mut client_id = None;
    let mut bucket_secs = None;
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
            bucket_secs == 60,
            "telemetry-rollups --bucket-secs must be 60"
        );
    }
    Ok((limit, client_id, bucket_secs))
}

fn parse_telemetry_network_rates_args(parts: &[&str]) -> Result<TelemetryNetworkRateArgs> {
    let mut limit = 50_u16;
    let mut client_id = None;
    let mut interface = None;
    let mut bucket_secs = None;
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
            bucket_secs == 60,
            "telemetry-network-rates --bucket-secs must be 60"
        );
    }
    Ok(TelemetryNetworkRateArgs {
        limit,
        client_id,
        interface,
        bucket_secs,
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

fn telemetry_rollups_path(limit: u16, client_id: Option<&str>, bucket_secs: Option<i32>) -> String {
    let mut path = format!("/api/v1/telemetry/rollups?limit={limit}");
    if let Some(client_id) = client_id {
        path.push_str("&client_id=");
        path.push_str(&percent_encode_query_value(client_id));
    }
    if let Some(bucket_secs) = bucket_secs {
        path.push_str("&bucket_secs=");
        path.push_str(&bucket_secs.to_string());
    }
    path
}

fn telemetry_network_rates_path(
    limit: u16,
    client_id: Option<&str>,
    interface: Option<&str>,
    bucket_secs: Option<i32>,
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

fn source_templates_path(domain: Option<&str>) -> String {
    match domain {
        Some(domain) => format!(
            "/api/v1/source-templates?domain={}",
            percent_encode_query_value(domain)
        ),
        None => "/api/v1/source-templates".to_string(),
    }
}

fn source_template_assignments_path(client_id: Option<&str>, domain: Option<&str>) -> String {
    source_template_filtered_path("/api/v1/source-template-assignments", client_id, domain)
}

fn source_status_path(client_id: Option<&str>, domain: Option<&str>) -> String {
    source_template_filtered_path("/api/v1/source-status", client_id, domain)
}

fn source_template_filtered_path(
    base: &str,
    client_id: Option<&str>,
    domain: Option<&str>,
) -> String {
    let mut query = Vec::new();
    if let Some(client_id) = client_id {
        query.push(format!(
            "client_id={}",
            percent_encode_query_value(client_id)
        ));
    }
    if let Some(domain) = domain {
        query.push(format!("domain={}", percent_encode_query_value(domain)));
    }
    if query.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", query.join("&"))
    }
}

fn template_runtime_config_path(client_id: &str) -> String {
    format!(
        "/api/v1/template-runtime-config?client_id={}",
        percent_encode_query_value(client_id)
    )
}

#[cfg(test)]
#[path = "vty_inventory_tests.rs"]
mod tests;
