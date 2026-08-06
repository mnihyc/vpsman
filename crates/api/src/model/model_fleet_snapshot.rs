use serde::{Deserialize, Serialize};

use crate::{
    model::{
        AgentView, FleetAlertView, FleetSummary, TelemetryNetworkRateView, TelemetryRollupView,
        TelemetryTunnelView,
    },
    model_alert_notifications::{
        FleetAlertNotificationChannelView, FleetAlertNotificationDeliveryView,
    },
    model_alert_policies::{
        PolicyAlertRecord, PolicyGroupRecord, TrafficAccountingRecord, VpsRuleValueRecord,
    },
    model_alert_states::FleetAlertStateView,
    model_webhook_rules::{WebhookRuleDeliveryView, WebhookRuleView},
};

#[derive(Debug, Deserialize)]
pub(crate) struct FleetSnapshotQuery {
    pub(crate) mode: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct FleetSnapshotSource<T> {
    pub(crate) data: Option<T>,
    pub(crate) error: Option<String>,
}

impl<T> FleetSnapshotSource<T> {
    pub(crate) fn available(data: T) -> Self {
        Self {
            data: Some(data),
            error: None,
        }
    }

    pub(crate) fn unavailable(error: impl Into<String>) -> Self {
        Self {
            data: None,
            error: Some(error.into()),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct FleetSnapshotResponse {
    pub(crate) mode: String,
    pub(crate) generated_at: String,
    pub(crate) summary: FleetSnapshotSource<FleetSummary>,
    pub(crate) agents: FleetSnapshotSource<Vec<AgentView>>,
    pub(crate) telemetry_rollups: FleetSnapshotSource<Vec<TelemetryRollupView>>,
    pub(crate) telemetry_network_rates: FleetSnapshotSource<Vec<TelemetryNetworkRateView>>,
    pub(crate) telemetry_tunnels: FleetSnapshotSource<Vec<TelemetryTunnelView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) fleet_alerts: Option<FleetSnapshotSource<Vec<FleetAlertView>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) fleet_alert_states: Option<FleetSnapshotSource<Vec<FleetAlertStateView>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) fleet_alert_policies: Option<FleetSnapshotSource<Vec<PolicyGroupRecord>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) vps_rule_values: Option<FleetSnapshotSource<Vec<VpsRuleValueRecord>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) traffic_accounting: Option<FleetSnapshotSource<Vec<TrafficAccountingRecord>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) policy_alerts: Option<FleetSnapshotSource<Vec<PolicyAlertRecord>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) fleet_alert_notification_channels:
        Option<FleetSnapshotSource<Vec<FleetAlertNotificationChannelView>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) fleet_alert_notifications:
        Option<FleetSnapshotSource<Vec<FleetAlertNotificationDeliveryView>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) webhook_rules: Option<FleetSnapshotSource<Vec<WebhookRuleView>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) webhook_rule_deliveries: Option<FleetSnapshotSource<Vec<WebhookRuleDeliveryView>>>,
}
