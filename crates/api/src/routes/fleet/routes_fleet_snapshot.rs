use std::future::Future;

use crate::{
    error::ApiError,
    model::FleetAlertQuery,
    model_alert_policies::PolicyAlertQuery,
    model_fleet_snapshot::{FleetSnapshotQuery, FleetSnapshotResponse, FleetSnapshotSource},
    security::{
        operator_has_scope, SCOPE_BACKUPS_READ, SCOPE_CONFIG_READ, SCOPE_FLEET_READ,
        SCOPE_INTEGRATIONS_READ,
    },
    state::AppState,
    unix_now,
};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};

pub(crate) const FLEET_DETAIL_LIMIT: i64 = 200;

pub(crate) async fn fleet_snapshot(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FleetSnapshotQuery>,
) -> Result<Json<FleetSnapshotResponse>, ApiError> {
    let operator = state.require_operator(&headers).await?;
    let mode = parse_snapshot_mode(query.mode.as_deref())?;
    let scopes = operator.operator.scopes.clone();
    let key = serde_json::json!({
        "endpoint": "fleet_snapshot",
        "auth": crate::state::read_singleflight_auth_key(operator.operator.id, &scopes),
        "mode": mode,
    })
    .to_string();
    let events = state.events.clone();
    let response = events
        .singleflight_fleet_snapshot(key, move || async move {
            build_fleet_snapshot(&state, &scopes, mode).await
        })
        .await?;
    Ok(Json(response))
}

async fn build_fleet_snapshot(
    state: &AppState,
    scopes: &[String],
    mode: &'static str,
) -> Result<FleetSnapshotResponse, ApiError> {
    let context = load_snapshot_read_context(state, scopes, mode == "full").await;
    let (live, full) = tokio::join!(
        load_live_sources(state, scopes, context.agents.clone()),
        load_full_sources(state, scopes, mode == "full", &context),
    );

    let mut response = FleetSnapshotResponse {
        mode: mode.to_string(),
        generated_at: unix_now().to_string(),
        summary: live.summary,
        agents: live.agents,
        telemetry_rollups: live.telemetry_rollups,
        telemetry_network_rates: live.telemetry_network_rates,
        telemetry_tunnels: live.telemetry_tunnels,
        telemetry_uptimes: live.telemetry_uptimes,
        fleet_alerts: None,
        fleet_alerts_truncated: None,
        fleet_alert_history: None,
        fleet_alert_history_truncated: None,
        fleet_alert_policies: None,
        vps_rule_values: None,
        traffic_accounting: None,
        policy_alerts: None,
        policy_alerts_truncated: None,
        current_policy_alerts: None,
        current_policy_alerts_truncated: None,
        fleet_alert_notification_channels: None,
        fleet_alert_notifications: None,
        fleet_alert_notifications_truncated: None,
        webhook_rules: None,
        webhook_rule_deliveries: None,
        webhook_rule_deliveries_truncated: None,
    };
    if let Some(full) = full {
        response.fleet_alerts = Some(full.fleet_alerts);
        response.fleet_alerts_truncated = Some(full.fleet_alerts_truncated);
        response.fleet_alert_history = Some(full.fleet_alert_history);
        response.fleet_alert_history_truncated = Some(full.fleet_alert_history_truncated);
        response.fleet_alert_policies = Some(full.fleet_alert_policies);
        response.vps_rule_values = Some(full.vps_rule_values);
        response.traffic_accounting = Some(full.traffic_accounting);
        response.policy_alerts = Some(full.policy_alerts);
        response.policy_alerts_truncated = Some(full.policy_alerts_truncated);
        response.current_policy_alerts = Some(full.current_policy_alerts);
        response.current_policy_alerts_truncated = Some(full.current_policy_alerts_truncated);
        response.fleet_alert_notification_channels = Some(full.fleet_alert_notification_channels);
        response.fleet_alert_notifications = Some(full.fleet_alert_notifications);
        response.fleet_alert_notifications_truncated =
            Some(full.fleet_alert_notifications_truncated);
        response.webhook_rules = Some(full.webhook_rules);
        response.webhook_rule_deliveries = Some(full.webhook_rule_deliveries);
        response.webhook_rule_deliveries_truncated = Some(full.webhook_rule_deliveries_truncated);
    }
    Ok(response)
}

struct SnapshotReadContext {
    agents: FleetSnapshotSource<Vec<crate::model::AgentView>>,
    vps_rule_values:
        Option<FleetSnapshotSource<Vec<crate::model_alert_policies::VpsRuleValueRecord>>>,
}

async fn load_snapshot_read_context(
    state: &AppState,
    scopes: &[String],
    include_full: bool,
) -> SnapshotReadContext {
    let fleet_read = operator_has_scope(scopes, SCOPE_FLEET_READ);
    let config_read = operator_has_scope(scopes, SCOPE_CONFIG_READ);
    let agents = load_source("agents", fleet_read, state.repo.list_agents()).await;
    let vps_rule_values = if include_full && (fleet_read || config_read) {
        let client_ids = agents.data.as_ref().map(|agents| {
            agents
                .iter()
                .map(|agent| agent.id.clone())
                .collect::<Vec<_>>()
        });
        Some(match client_ids {
            Some(client_ids) => {
                load_source(
                    "vps_rule_values",
                    true,
                    state.repo.list_all_vps_rules_for_clients(&client_ids),
                )
                .await
            }
            None if fleet_read => {
                FleetSnapshotSource::unavailable("fleet_snapshot_vps_rule_values_unavailable")
            }
            // A config-only operator is deliberately not allowed to load the
            // fleet owner. Rules remain an independently authorized source in
            // that permission shape; this is not a retry after a failed read.
            None => load_source("vps_rule_values", true, state.repo.list_all_vps_rules()).await,
        })
    } else {
        None
    };
    SnapshotReadContext {
        agents,
        vps_rule_values,
    }
}

fn parse_snapshot_mode(mode: Option<&str>) -> Result<&'static str, ApiError> {
    match mode {
        Some("live") => Ok("live"),
        Some("full") => Ok("full"),
        Some(_) => Err(ApiError::bad_request("fleet_snapshot_mode_invalid")),
        None => Err(ApiError::bad_request("fleet_snapshot_mode_required")),
    }
}

struct LiveSources {
    summary: FleetSnapshotSource<crate::model::FleetSummary>,
    agents: FleetSnapshotSource<Vec<crate::model::AgentView>>,
    telemetry_rollups: FleetSnapshotSource<Vec<crate::model::TelemetryRollupView>>,
    telemetry_network_rates: FleetSnapshotSource<Vec<crate::model::TelemetryNetworkRateView>>,
    telemetry_tunnels: FleetSnapshotSource<Vec<crate::model::TelemetryTunnelView>>,
    telemetry_uptimes: FleetSnapshotSource<Vec<crate::model::TelemetryUptimeView>>,
}

pub(crate) struct HomeFleetSources {
    pub(crate) summary: FleetSnapshotSource<crate::model::FleetSummary>,
    pub(crate) agents: FleetSnapshotSource<Vec<crate::model::AgentView>>,
    pub(crate) telemetry_rollups: FleetSnapshotSource<Vec<crate::model::TelemetryRollupView>>,
    pub(crate) telemetry_network_rates:
        FleetSnapshotSource<Vec<crate::model::TelemetryNetworkRateView>>,
    pub(crate) fleet_alerts: FleetSnapshotSource<Vec<crate::model::FleetAlertView>>,
    pub(crate) fleet_alerts_truncated: bool,
}

pub(crate) async fn load_home_agents(
    state: &AppState,
    scopes: &[String],
) -> FleetSnapshotSource<Vec<crate::model::AgentView>> {
    load_source(
        "agents",
        operator_has_scope(scopes, SCOPE_FLEET_READ),
        state.repo.list_agents(),
    )
    .await
}

pub(crate) async fn load_home_fleet_sources(
    state: &AppState,
    scopes: &[String],
    agents: FleetSnapshotSource<Vec<crate::model::AgentView>>,
) -> HomeFleetSources {
    let fleet_read = operator_has_scope(scopes, SCOPE_FLEET_READ);
    let backups_read = operator_has_scope(scopes, SCOPE_BACKUPS_READ);
    let telemetry_client_ids = visible_agent_ids(&agents);
    let (summary, telemetry_rollups, telemetry_network_rates, fleet_alerts) = tokio::join!(
        load_source("summary", fleet_read, state.repo.fleet_summary()),
        load_snapshot_telemetry_rollups(state, fleet_read, telemetry_client_ids.as_deref(),),
        load_snapshot_telemetry_network_rates(state, fleet_read, telemetry_client_ids.as_deref(),),
        load_home_fleet_alerts(state, fleet_read && backups_read, agents.data.as_deref(),),
    );
    let (fleet_alerts, fleet_alerts_truncated) = fleet_alerts;
    HomeFleetSources {
        summary,
        agents,
        telemetry_rollups,
        telemetry_network_rates,
        fleet_alerts,
        fleet_alerts_truncated,
    }
}

async fn load_home_fleet_alerts(
    state: &AppState,
    permitted: bool,
    agents: Option<&[crate::model::AgentView]>,
) -> (FleetSnapshotSource<Vec<crate::model::FleetAlertView>>, bool) {
    if !permitted {
        return (FleetSnapshotSource::unavailable("forbidden"), false);
    }
    let Some(agents) = agents else {
        return (FleetSnapshotSource::unavailable("unavailable"), false);
    };
    let query = FleetAlertQuery {
        limit: Some(FLEET_DETAIL_LIMIT),
        client_id: None,
        severity: None,
        category: None,
        operator_state: None,
        include_muted: Some(true),
    };
    let selection = state
        .list_fleet_alerts_selected_with_visible_agents(query, None, agents)
        .await;
    match selection {
        Ok(selection) => (
            FleetSnapshotSource::available(selection.alerts),
            selection.truncated,
        ),
        Err(error) => {
            tracing::warn!(%error, source = "fleet_alerts", "home snapshot source unavailable");
            (FleetSnapshotSource::unavailable("unavailable"), false)
        }
    }
}

async fn load_live_sources(
    state: &AppState,
    scopes: &[String],
    agents: FleetSnapshotSource<Vec<crate::model::AgentView>>,
) -> LiveSources {
    let fleet_read = operator_has_scope(scopes, SCOPE_FLEET_READ);
    let telemetry_client_ids = visible_agent_ids(&agents);
    let (
        summary,
        agents,
        telemetry_rollups,
        telemetry_network_rates,
        telemetry_tunnels,
        telemetry_uptimes,
    ) = tokio::join!(
        load_source("summary", fleet_read, state.repo.fleet_summary()),
        async { agents },
        load_snapshot_telemetry_rollups(state, fleet_read, telemetry_client_ids.as_deref(),),
        load_snapshot_telemetry_network_rates(state, fleet_read, telemetry_client_ids.as_deref(),),
        load_snapshot_telemetry_tunnels(state, fleet_read, telemetry_client_ids.as_deref()),
        load_snapshot_telemetry_uptimes(state, fleet_read, telemetry_client_ids.as_deref(),),
    );
    LiveSources {
        summary,
        agents,
        telemetry_rollups,
        telemetry_network_rates,
        telemetry_tunnels,
        telemetry_uptimes,
    }
}

fn visible_agent_ids(
    agents: &FleetSnapshotSource<Vec<crate::model::AgentView>>,
) -> Option<Vec<String>> {
    agents.data.as_ref().map(|agents| {
        agents
            .iter()
            .filter(|agent| agent.is_monitoring_visible())
            .map(|agent| agent.id.clone())
            .collect()
    })
}

async fn load_snapshot_telemetry_rollups(
    state: &AppState,
    permitted: bool,
    client_ids: Option<&[String]>,
) -> FleetSnapshotSource<Vec<crate::model::TelemetryRollupView>> {
    if !permitted {
        return FleetSnapshotSource::unavailable("operator_scope_insufficient");
    }
    let Some(client_ids) = client_ids else {
        return FleetSnapshotSource::unavailable("fleet_snapshot_telemetry_rollups_unavailable");
    };
    load_source(
        "telemetry_rollups",
        true,
        state
            .repo
            .list_latest_telemetry_rollups_for_clients(client_ids, None),
    )
    .await
}

async fn load_snapshot_telemetry_network_rates(
    state: &AppState,
    permitted: bool,
    client_ids: Option<&[String]>,
) -> FleetSnapshotSource<Vec<crate::model::TelemetryNetworkRateView>> {
    if !permitted {
        return FleetSnapshotSource::unavailable("operator_scope_insufficient");
    }
    let Some(client_ids) = client_ids else {
        return FleetSnapshotSource::unavailable(
            "fleet_snapshot_telemetry_network_rates_unavailable",
        );
    };
    load_source(
        "telemetry_network_rates",
        true,
        state
            .repo
            .list_latest_telemetry_network_rates_for_clients(client_ids),
    )
    .await
}

async fn load_snapshot_telemetry_uptimes(
    state: &AppState,
    permitted: bool,
    client_ids: Option<&[String]>,
) -> FleetSnapshotSource<Vec<crate::model::TelemetryUptimeView>> {
    if !permitted {
        return FleetSnapshotSource::unavailable("operator_scope_insufficient");
    }
    let Some(client_ids) = client_ids else {
        return FleetSnapshotSource::unavailable("fleet_snapshot_telemetry_uptimes_unavailable");
    };
    load_source(
        "telemetry_uptimes",
        true,
        state
            .repo
            .list_latest_telemetry_uptimes_for_clients(client_ids),
    )
    .await
}

async fn load_snapshot_telemetry_tunnels(
    state: &AppState,
    permitted: bool,
    client_ids: Option<&[String]>,
) -> FleetSnapshotSource<Vec<crate::model::TelemetryTunnelView>> {
    if !permitted {
        return FleetSnapshotSource::unavailable("operator_scope_insufficient");
    }
    let Some(client_ids) = client_ids else {
        // The declared-tunnel query is deliberately bounded by the already-loaded
        // visible fleet. If that authoritative universe is unavailable, exposing a
        // global capped fallback would make a successful source silently incomplete.
        return FleetSnapshotSource::unavailable("fleet_snapshot_telemetry_tunnels_unavailable");
    };
    load_source(
        "telemetry_tunnels",
        true,
        state
            .repo
            .list_declared_telemetry_tunnels_for_clients(client_ids),
    )
    .await
}

struct FullSources {
    fleet_alerts: FleetSnapshotSource<Vec<crate::model::FleetAlertView>>,
    fleet_alerts_truncated: bool,
    fleet_alert_history: FleetSnapshotSource<Vec<crate::model::FleetAlertView>>,
    fleet_alert_history_truncated: bool,
    fleet_alert_policies: FleetSnapshotSource<Vec<crate::model_alert_policies::PolicyGroupRecord>>,
    vps_rule_values: FleetSnapshotSource<Vec<crate::model_alert_policies::VpsRuleValueRecord>>,
    traffic_accounting:
        FleetSnapshotSource<Vec<crate::model_alert_policies::TrafficAccountingRecord>>,
    policy_alerts: FleetSnapshotSource<Vec<crate::model_alert_policies::PolicyAlertRecord>>,
    policy_alerts_truncated: bool,
    current_policy_alerts: FleetSnapshotSource<Vec<crate::model_alert_policies::PolicyAlertRecord>>,
    current_policy_alerts_truncated: bool,
    fleet_alert_notification_channels: FleetSnapshotSource<
        Vec<crate::model_alert_notifications::FleetAlertNotificationChannelView>,
    >,
    fleet_alert_notifications: FleetSnapshotSource<
        Vec<crate::model_alert_notifications::FleetAlertNotificationDeliveryView>,
    >,
    fleet_alert_notifications_truncated: bool,
    webhook_rules: FleetSnapshotSource<Vec<crate::model_webhook_rules::WebhookRuleView>>,
    webhook_rule_deliveries:
        FleetSnapshotSource<Vec<crate::model_webhook_rules::WebhookRuleDeliveryView>>,
    webhook_rule_deliveries_truncated: bool,
}

async fn load_full_sources(
    state: &AppState,
    scopes: &[String],
    include: bool,
    context: &SnapshotReadContext,
) -> Option<FullSources> {
    if !include {
        return None;
    }
    let fleet_read = operator_has_scope(scopes, SCOPE_FLEET_READ);
    let backups_read = operator_has_scope(scopes, SCOPE_BACKUPS_READ);
    let config_read = operator_has_scope(scopes, SCOPE_CONFIG_READ);
    let integrations_read = operator_has_scope(scopes, SCOPE_INTEGRATIONS_READ);
    let vps_rule_values = if config_read {
        context.vps_rule_values.clone().unwrap_or_else(|| {
            FleetSnapshotSource::unavailable("fleet_snapshot_vps_rule_values_unavailable")
        })
    } else {
        FleetSnapshotSource::unavailable("operator_scope_insufficient")
    };
    let (
        fleet_alerts,
        fleet_alert_history,
        fleet_alert_policies,
        traffic_accounting,
        policy_alerts,
        current_policy_alerts,
        fleet_alert_notification_channels,
        fleet_alert_notifications,
        webhook_rules,
        webhook_rule_deliveries,
    ) = tokio::join!(
        load_current_fleet_alerts(
            state,
            fleet_read && backups_read,
            context.agents.data.as_deref(),
        ),
        load_fleet_alert_history(state, fleet_read && backups_read),
        load_fleet_alert_policies_with_context(
            state,
            fleet_read,
            config_read,
            context.agents.data.as_deref(),
            context
                .vps_rule_values
                .as_ref()
                .and_then(|source| source.data.as_deref()),
        ),
        load_snapshot_traffic_accounting(
            state,
            fleet_read,
            context.agents.data.as_deref(),
            context
                .vps_rule_values
                .as_ref()
                .and_then(|source| source.data.as_deref()),
        ),
        load_bounded_source(
            "policy_alerts",
            fleet_read,
            state.repo.list_policy_alerts(&PolicyAlertQuery {
                limit: Some(FLEET_DETAIL_LIMIT + 1),
                client_id: None,
                severity: None,
                category: None,
                policy_group_id: None,
            }),
        ),
        load_current_policy_alerts(state, fleet_read),
        load_source(
            "fleet_alert_notification_channels",
            integrations_read,
            state.repo.list_all_fleet_alert_notification_channels(),
        ),
        load_bounded_source(
            "fleet_alert_notifications",
            integrations_read,
            state.repo.list_fleet_alert_notification_deliveries(
                FLEET_DETAIL_LIMIT + 1,
                None,
                None,
                None,
            ),
        ),
        load_source(
            "webhook_rules",
            integrations_read,
            state.repo.list_all_webhook_rules(),
        ),
        load_bounded_source(
            "webhook_rule_deliveries",
            integrations_read,
            state
                .repo
                .list_webhook_rule_deliveries(FLEET_DETAIL_LIMIT + 1, None, None, None,),
        ),
    );
    let (fleet_alerts, fleet_alerts_truncated) = fleet_alerts;
    let (fleet_alert_history, fleet_alert_history_truncated) = fleet_alert_history;
    let (policy_alerts, policy_alerts_truncated) = policy_alerts;
    let (current_policy_alerts, current_policy_alerts_truncated) = current_policy_alerts;
    let (fleet_alert_notifications, fleet_alert_notifications_truncated) =
        fleet_alert_notifications;
    let (webhook_rule_deliveries, webhook_rule_deliveries_truncated) = webhook_rule_deliveries;
    Some(FullSources {
        fleet_alerts,
        fleet_alerts_truncated,
        fleet_alert_history,
        fleet_alert_history_truncated,
        fleet_alert_policies,
        vps_rule_values,
        traffic_accounting,
        policy_alerts,
        policy_alerts_truncated,
        current_policy_alerts,
        current_policy_alerts_truncated,
        fleet_alert_notification_channels,
        fleet_alert_notifications,
        fleet_alert_notifications_truncated,
        webhook_rules,
        webhook_rule_deliveries,
        webhook_rule_deliveries_truncated,
    })
}

async fn load_current_fleet_alerts(
    state: &AppState,
    permitted: bool,
    agents: Option<&[crate::model::AgentView]>,
) -> (FleetSnapshotSource<Vec<crate::model::FleetAlertView>>, bool) {
    if !permitted {
        return (FleetSnapshotSource::unavailable("forbidden"), false);
    }
    let Some(agents) = agents else {
        return (FleetSnapshotSource::unavailable("unavailable"), false);
    };
    let query = FleetAlertQuery {
        limit: Some(FLEET_DETAIL_LIMIT),
        client_id: None,
        severity: None,
        category: None,
        operator_state: None,
        include_muted: Some(true),
    };
    let selection = state
        .list_fleet_alerts_selected_with_visible_agents(query, None, agents)
        .await;
    match selection {
        Ok(selection) => (
            FleetSnapshotSource::available(selection.alerts),
            selection.truncated,
        ),
        Err(error) => {
            tracing::warn!(%error, source = "fleet_alerts", "fleet snapshot source unavailable");
            (FleetSnapshotSource::unavailable("unavailable"), false)
        }
    }
}

async fn load_fleet_alert_history(
    state: &AppState,
    permitted: bool,
) -> (FleetSnapshotSource<Vec<crate::model::FleetAlertView>>, bool) {
    if !permitted {
        return (FleetSnapshotSource::unavailable("forbidden"), false);
    }
    match state
        .list_fleet_alert_history(FleetAlertQuery {
            limit: Some(FLEET_DETAIL_LIMIT),
            client_id: None,
            severity: None,
            category: None,
            operator_state: None,
            include_muted: Some(true),
        })
        .await
    {
        Ok(selection) => (
            FleetSnapshotSource::available(selection.alerts),
            selection.truncated,
        ),
        Err(error) => {
            tracing::warn!(%error, source = "fleet_alert_history", "fleet snapshot source unavailable");
            (FleetSnapshotSource::unavailable("unavailable"), false)
        }
    }
}

async fn load_current_policy_alerts(
    state: &AppState,
    permitted: bool,
) -> (
    FleetSnapshotSource<Vec<crate::model_alert_policies::PolicyAlertRecord>>,
    bool,
) {
    let mut source = load_source(
        "current_policy_alerts",
        permitted,
        state.repo.list_policy_alert_candidates(
            &PolicyAlertQuery {
                limit: None,
                client_id: None,
                severity: None,
                category: None,
                policy_group_id: None,
            },
            (FLEET_DETAIL_LIMIT + 1) as usize,
            None,
            None,
            None,
        ),
    )
    .await;
    let truncated = source
        .data
        .as_ref()
        .is_some_and(|alerts| alerts.len() > FLEET_DETAIL_LIMIT as usize);
    if let Some(alerts) = source.data.as_mut() {
        alerts.truncate(FLEET_DETAIL_LIMIT as usize);
    }
    (source, truncated)
}

async fn load_fleet_alert_policies_with_context(
    state: &AppState,
    permitted: bool,
    allow_vps_rule_selectors: bool,
    agents: Option<&[crate::model::AgentView]>,
    rules: Option<&[crate::model_alert_policies::VpsRuleValueRecord]>,
) -> FleetSnapshotSource<Vec<crate::model_alert_policies::PolicyGroupRecord>> {
    let future = async {
        let (Some(agents), Some(rules)) = (agents, rules) else {
            return Err(anyhow::anyhow!("fleet_snapshot_policy_context_unavailable"));
        };
        state
            .repo
            .list_all_fleet_alert_policies_with_context(allow_vps_rule_selectors, agents, rules)
            .await
    };
    load_source("fleet_alert_policies", permitted, future).await
}

async fn load_snapshot_traffic_accounting(
    state: &AppState,
    permitted: bool,
    agents: Option<&[crate::model::AgentView]>,
    rules: Option<&[crate::model_alert_policies::VpsRuleValueRecord]>,
) -> FleetSnapshotSource<Vec<crate::model_alert_policies::TrafficAccountingRecord>> {
    let future = async {
        let (Some(agents), Some(rules)) = (agents, rules) else {
            return Err(anyhow::anyhow!(
                "fleet_snapshot_traffic_context_unavailable"
            ));
        };
        let visible_agents = agents
            .iter()
            .filter(|agent| agent.is_monitoring_visible())
            .cloned()
            .collect::<Vec<_>>();
        state
            .repo
            .list_traffic_accounting_for_agents_with_rules(&visible_agents, rules)
            .await
    };
    load_source("traffic_accounting", permitted, future).await
}

async fn load_source<T, F>(
    source: &'static str,
    permitted: bool,
    future: F,
) -> FleetSnapshotSource<T>
where
    F: Future<Output = anyhow::Result<T>>,
{
    if !permitted {
        return FleetSnapshotSource::unavailable("operator_scope_insufficient");
    }
    match future.await {
        Ok(data) => FleetSnapshotSource::available(data),
        Err(error)
            if error
                .to_string()
                .contains("vps_rule_selector_scope_required") =>
        {
            FleetSnapshotSource::unavailable("operator_scope_insufficient")
        }
        Err(error) => {
            tracing::warn!(source, %error, "fleet snapshot source failed");
            FleetSnapshotSource::unavailable(format!("fleet_snapshot_{source}_unavailable"))
        }
    }
}

async fn load_bounded_source<T, F>(
    source: &'static str,
    permitted: bool,
    future: F,
) -> (FleetSnapshotSource<Vec<T>>, bool)
where
    F: Future<Output = anyhow::Result<Vec<T>>>,
{
    let mut source = load_source(source, permitted, future).await;
    let truncated = source
        .data
        .as_ref()
        .is_some_and(|rows| rows.len() > FLEET_DETAIL_LIMIT as usize);
    if let Some(rows) = source.data.as_mut() {
        rows.truncate(FLEET_DETAIL_LIMIT as usize);
    }
    (source, truncated)
}

#[cfg(test)]
#[path = "tests_routes_fleet_snapshot.rs"]
mod tests;
