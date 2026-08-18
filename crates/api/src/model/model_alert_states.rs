use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct FleetAlertStateView {
    pub(crate) alert_id: String,
    pub(crate) state: String,
    pub(crate) muted_until_unix: Option<i64>,
    pub(crate) escalation_level: i32,
    pub(crate) revision: i64,
    pub(crate) reason: Option<String>,
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FleetAlertStateQuery {
    pub(crate) limit: Option<i64>,
    pub(crate) state: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateFleetAlertStateRequest {
    pub(crate) alert_id: String,
    pub(crate) action: String,
    pub(crate) muted_for_secs: Option<i64>,
    pub(crate) reason: Option<String>,
    #[serde(default)]
    pub(crate) expected_revision: Option<i64>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BulkFleetAlertStateItem {
    pub(crate) alert_id: String,
    pub(crate) expected_revision: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BulkUpdateFleetAlertStatesRequest {
    pub(crate) action: String,
    pub(crate) items: Vec<BulkFleetAlertStateItem>,
    pub(crate) muted_for_secs: Option<i64>,
    pub(crate) reason: Option<String>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BulkUpdateFleetAlertStatesResponse {
    pub(crate) batch_id: Uuid,
    pub(crate) states: Vec<FleetAlertStateView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct FleetAlertExportView {
    pub(crate) generated_at: String,
    pub(crate) query: serde_json::Value,
    pub(crate) alerts: Vec<crate::model::FleetAlertView>,
}
