use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct FleetAlertPolicyOverrideView {
    pub(crate) id: Uuid,
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
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FleetAlertPolicyQuery {
    pub(crate) limit: Option<i64>,
    pub(crate) enabled: Option<bool>,
    pub(crate) scope_kind: Option<String>,
    pub(crate) scope_value: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateFleetAlertPolicyRequest {
    pub(crate) id: Option<Uuid>,
    pub(crate) name: String,
    pub(crate) scope_kind: String,
    pub(crate) scope_value: Option<String>,
    pub(crate) memory_available_warning_ratio: Option<f64>,
    pub(crate) memory_available_critical_ratio: Option<f64>,
    pub(crate) disk_available_warning_ratio: Option<f64>,
    pub(crate) disk_available_critical_ratio: Option<f64>,
    pub(crate) cpu_load_warning: Option<f64>,
    pub(crate) cpu_load_critical: Option<f64>,
    pub(crate) priority: Option<i32>,
    pub(crate) enabled: Option<bool>,
    pub(crate) notes: Option<String>,
    pub(crate) confirmed: bool,
}
