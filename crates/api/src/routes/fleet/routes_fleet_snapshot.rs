use std::future::Future;

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};

use crate::{
    error::ApiError,
    model::FleetAlertQuery,
    model_alert_policies::{PolicyAlertQuery, TrafficAccountingQuery},
    model_fleet_snapshot::{FleetSnapshotQuery, FleetSnapshotResponse, FleetSnapshotSource},
    security::{
        operator_has_scope, SCOPE_BACKUPS_READ, SCOPE_CONFIG_READ, SCOPE_FLEET_READ,
        SCOPE_INTEGRATIONS_READ,
    },
    state::AppState,
    unix_now,
};

const FLEET_DETAIL_LIMIT: i64 = 200;
const FLEET_LATEST_TELEMETRY_LIMIT: i64 = 1_000;
const FLEET_NETWORK_RATE_SNAPSHOT_LIMIT: i64 = 5_000;

pub(crate) async fn fleet_snapshot(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FleetSnapshotQuery>,
) -> Result<Json<FleetSnapshotResponse>, ApiError> {
    let operator = state.require_operator(&headers).await?;
    let mode = parse_snapshot_mode(query.mode.as_deref())?;
    let scopes = &operator.operator.scopes;

    let (live, full) = tokio::join!(
        load_live_sources(&state, scopes),
        load_full_sources(&state, scopes, mode == "full"),
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
        fleet_alert_states: None,
        fleet_alert_policies: None,
        vps_rule_values: None,
        traffic_accounting: None,
        policy_alerts: None,
        fleet_alert_notification_channels: None,
        fleet_alert_notifications: None,
        webhook_rules: None,
        webhook_rule_deliveries: None,
    };
    if let Some(full) = full {
        response.fleet_alerts = Some(full.fleet_alerts);
        response.fleet_alert_states = Some(full.fleet_alert_states);
        response.fleet_alert_policies = Some(full.fleet_alert_policies);
        response.vps_rule_values = Some(full.vps_rule_values);
        response.traffic_accounting = Some(full.traffic_accounting);
        response.policy_alerts = Some(full.policy_alerts);
        response.fleet_alert_notification_channels = Some(full.fleet_alert_notification_channels);
        response.fleet_alert_notifications = Some(full.fleet_alert_notifications);
        response.webhook_rules = Some(full.webhook_rules);
        response.webhook_rule_deliveries = Some(full.webhook_rule_deliveries);
    }
    Ok(Json(response))
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

async fn load_live_sources(state: &AppState, scopes: &[String]) -> LiveSources {
    let fleet_read = operator_has_scope(scopes, SCOPE_FLEET_READ);
    let (
        summary,
        agents,
        telemetry_rollups,
        telemetry_network_rates,
        telemetry_tunnels,
        telemetry_uptimes,
    ) = tokio::join!(
        load_source("summary", fleet_read, state.repo.fleet_summary()),
        load_source("agents", fleet_read, state.repo.list_agents()),
        load_source(
            "telemetry_rollups",
            fleet_read,
            state
                .repo
                .list_latest_telemetry_rollups(FLEET_LATEST_TELEMETRY_LIMIT, None, None,),
        ),
        load_source(
            "telemetry_network_rates",
            fleet_read,
            state.repo.list_latest_telemetry_network_rates(
                FLEET_NETWORK_RATE_SNAPSHOT_LIMIT,
                None,
                None,
                None,
            ),
        ),
        load_source(
            "telemetry_tunnels",
            fleet_read,
            state
                .repo
                .list_telemetry_tunnels(FLEET_LATEST_TELEMETRY_LIMIT, None, None,),
        ),
        load_source(
            "telemetry_uptimes",
            fleet_read,
            state.repo.list_latest_telemetry_uptimes(),
        ),
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

struct FullSources {
    fleet_alerts: FleetSnapshotSource<Vec<crate::model::FleetAlertView>>,
    fleet_alert_states: FleetSnapshotSource<Vec<crate::model_alert_states::FleetAlertStateView>>,
    fleet_alert_policies: FleetSnapshotSource<Vec<crate::model_alert_policies::PolicyGroupRecord>>,
    vps_rule_values: FleetSnapshotSource<Vec<crate::model_alert_policies::VpsRuleValueRecord>>,
    traffic_accounting:
        FleetSnapshotSource<Vec<crate::model_alert_policies::TrafficAccountingRecord>>,
    policy_alerts: FleetSnapshotSource<Vec<crate::model_alert_policies::PolicyAlertRecord>>,
    fleet_alert_notification_channels: FleetSnapshotSource<
        Vec<crate::model_alert_notifications::FleetAlertNotificationChannelView>,
    >,
    fleet_alert_notifications: FleetSnapshotSource<
        Vec<crate::model_alert_notifications::FleetAlertNotificationDeliveryView>,
    >,
    webhook_rules: FleetSnapshotSource<Vec<crate::model_webhook_rules::WebhookRuleView>>,
    webhook_rule_deliveries:
        FleetSnapshotSource<Vec<crate::model_webhook_rules::WebhookRuleDeliveryView>>,
}

async fn load_full_sources(
    state: &AppState,
    scopes: &[String],
    include: bool,
) -> Option<FullSources> {
    if !include {
        return None;
    }
    let fleet_read = operator_has_scope(scopes, SCOPE_FLEET_READ);
    let backups_read = operator_has_scope(scopes, SCOPE_BACKUPS_READ);
    let config_read = operator_has_scope(scopes, SCOPE_CONFIG_READ);
    let integrations_read = operator_has_scope(scopes, SCOPE_INTEGRATIONS_READ);
    let (
        fleet_alerts,
        fleet_alert_states,
        fleet_alert_policies,
        vps_rule_values,
        traffic_accounting,
        policy_alerts,
        fleet_alert_notification_channels,
        fleet_alert_notifications,
        webhook_rules,
        webhook_rule_deliveries,
    ) = tokio::join!(
        load_source(
            "fleet_alerts",
            fleet_read && backups_read,
            state.list_fleet_alerts(FleetAlertQuery {
                limit: Some(FLEET_DETAIL_LIMIT),
                client_id: None,
                severity: None,
                category: None,
                operator_state: None,
                include_muted: Some(true),
            }),
        ),
        load_source(
            "fleet_alert_states",
            fleet_read,
            state.repo.list_fleet_alert_states(FLEET_DETAIL_LIMIT, None),
        ),
        load_source(
            "fleet_alert_policies",
            fleet_read,
            state
                .repo
                .list_fleet_alert_policies(FLEET_DETAIL_LIMIT, None, None, None, config_read,),
        ),
        load_source(
            "vps_rule_values",
            config_read,
            state.repo.list_all_vps_rules(),
        ),
        load_source(
            "traffic_accounting",
            fleet_read,
            state.repo.list_traffic_accounting(&TrafficAccountingQuery {
                selector_expression: None,
                client_id: None,
                state: None,
                limit: Some(FLEET_DETAIL_LIMIT),
            }),
        ),
        load_source(
            "policy_alerts",
            fleet_read,
            state.repo.list_policy_alerts(&PolicyAlertQuery {
                limit: Some(FLEET_DETAIL_LIMIT),
                client_id: None,
                severity: None,
                category: None,
                policy_group_id: None,
            }),
        ),
        load_source(
            "fleet_alert_notification_channels",
            integrations_read,
            state.repo.list_fleet_alert_notification_channels(
                FLEET_DETAIL_LIMIT,
                None,
                None,
                None,
                None,
            ),
        ),
        load_source(
            "fleet_alert_notifications",
            integrations_read,
            state.repo.list_fleet_alert_notification_deliveries(
                FLEET_DETAIL_LIMIT,
                None,
                None,
                None,
            ),
        ),
        load_source(
            "webhook_rules",
            integrations_read,
            state.repo.list_webhook_rules(FLEET_DETAIL_LIMIT, None),
        ),
        load_source(
            "webhook_rule_deliveries",
            integrations_read,
            state
                .repo
                .list_webhook_rule_deliveries(FLEET_DETAIL_LIMIT, None, None, None,),
        ),
    );
    Some(FullSources {
        fleet_alerts,
        fleet_alert_states,
        fleet_alert_policies,
        vps_rule_values,
        traffic_accounting,
        policy_alerts,
        fleet_alert_notification_channels,
        fleet_alert_notifications,
        webhook_rules,
        webhook_rule_deliveries,
    })
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

#[cfg(test)]
#[path = "tests_routes_fleet_snapshot.rs"]
mod tests;
