use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct FleetAlertNotificationChannelView {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) scope_kind: String,
    pub(crate) scope_value: Option<String>,
    pub(crate) min_severity: String,
    pub(crate) categories: Vec<String>,
    pub(crate) operator_states: Vec<String>,
    pub(crate) delivery_kind: String,
    pub(crate) target: String,
    pub(crate) cooldown_secs: i64,
    pub(crate) enabled: bool,
    pub(crate) notes: Option<String>,
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FleetAlertNotificationChannelQuery {
    pub(crate) limit: Option<i64>,
    pub(crate) enabled: Option<bool>,
    pub(crate) scope_kind: Option<String>,
    pub(crate) scope_value: Option<String>,
    pub(crate) delivery_kind: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateFleetAlertNotificationChannelRequest {
    pub(crate) name: String,
    pub(crate) scope_kind: String,
    pub(crate) scope_value: Option<String>,
    pub(crate) min_severity: Option<String>,
    pub(crate) categories: Option<Vec<String>>,
    pub(crate) operator_states: Option<Vec<String>>,
    pub(crate) delivery_kind: String,
    pub(crate) target: String,
    pub(crate) cooldown_secs: Option<i64>,
    pub(crate) enabled: Option<bool>,
    pub(crate) notes: Option<String>,
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct FleetAlertNotificationDeliveryView {
    pub(crate) id: Uuid,
    pub(crate) channel_id: Uuid,
    pub(crate) channel_name: String,
    pub(crate) alert_id: String,
    pub(crate) alert_severity: String,
    pub(crate) alert_category: String,
    pub(crate) status: String,
    pub(crate) delivery_kind: String,
    pub(crate) target: String,
    pub(crate) dedupe_key: String,
    pub(crate) payload: serde_json::Value,
    pub(crate) error: Option<String>,
    pub(crate) attempt_count: i32,
    pub(crate) last_attempt_at: Option<String>,
    pub(crate) cooldown_until_unix: i64,
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) created_at: String,
    pub(crate) delivered_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FleetAlertNotificationDeliveryQuery {
    pub(crate) limit: Option<i64>,
    pub(crate) channel_id: Option<Uuid>,
    pub(crate) alert_id: Option<String>,
    pub(crate) status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FleetAlertNotificationDispatchRequest {
    pub(crate) limit: Option<i64>,
    pub(crate) client_id: Option<String>,
    pub(crate) severity: Option<String>,
    pub(crate) category: Option<String>,
    pub(crate) operator_state: Option<String>,
    pub(crate) include_muted: Option<bool>,
    pub(crate) dry_run: Option<bool>,
    pub(crate) confirmed: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FleetAlertNotificationProcessRequest {
    pub(crate) limit: Option<i64>,
    pub(crate) status: Option<String>,
    pub(crate) delivery_kind: Option<String>,
    pub(crate) dry_run: Option<bool>,
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct FleetAlertNotificationCandidate {
    pub(crate) channel_id: Uuid,
    pub(crate) channel_name: String,
    pub(crate) alert_id: String,
    pub(crate) alert_severity: String,
    pub(crate) alert_category: String,
    pub(crate) status: String,
    pub(crate) delivery_kind: String,
    pub(crate) target: String,
    pub(crate) dedupe_key: String,
    pub(crate) payload: serde_json::Value,
    pub(crate) cooldown_until_unix: i64,
}
