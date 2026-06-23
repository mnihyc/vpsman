use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub(crate) const VPS_RULE_KEY_TRAFFIC_RESET_DAY: &str = "traffic.reset_day";
pub(crate) const VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL: &str = "traffic.quota.total";
pub(crate) const VPS_RULE_KEY_TRAFFIC_QUOTA_RX: &str = "traffic.quota.rx";
pub(crate) const VPS_RULE_KEY_TRAFFIC_QUOTA_TX: &str = "traffic.quota.tx";
pub(crate) const VPS_RULE_KEY_TRAFFIC_SELECTORS: &str = "traffic.selectors";

#[derive(Clone, Debug, Serialize)]
pub(crate) struct VpsRuleValueRecord {
    pub(crate) client_id: String,
    pub(crate) key: String,
    pub(crate) value_raw: String,
    pub(crate) value_json: Value,
    pub(crate) parsed_display: String,
    pub(crate) state: String,
    pub(crate) validation_errors: Vec<String>,
    pub(crate) source_kind: String,
    pub(crate) source_id: Option<Uuid>,
    pub(crate) updated_by: Option<Uuid>,
    pub(crate) updated_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct VpsRuleChangePreview {
    pub(crate) client_id: String,
    pub(crate) display_name: String,
    pub(crate) key: String,
    pub(crate) before: Option<String>,
    pub(crate) after: Option<String>,
    pub(crate) action: String,
    pub(crate) validation: String,
    pub(crate) validation_errors: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct VpsRulesDryRunResponse {
    pub(crate) matched_vps_count: usize,
    pub(crate) changed_row_count: usize,
    pub(crate) invalid_row_count: usize,
    pub(crate) preview_hash: String,
    pub(crate) changes: Vec<VpsRuleChangePreview>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct VpsRuleQuery {
    pub(crate) limit: Option<i64>,
    pub(crate) client_id: Option<String>,
    pub(crate) selector_expression: Option<String>,
    pub(crate) key: Option<String>,
    pub(crate) state: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VpsRulesDryRunRequest {
    pub(crate) operation: String,
    pub(crate) selector_expression: String,
    #[serde(default)]
    pub(crate) values: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) keys: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VpsRulesBulkUpsertRequest {
    pub(crate) selector_expression: String,
    pub(crate) values: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) preview_hash: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VpsRulesBulkUnsetRequest {
    pub(crate) selector_expression: String,
    pub(crate) keys: Vec<String>,
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) preview_hash: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TrafficAccountingSelectorBreakdown {
    pub(crate) source: String,
    pub(crate) interface: String,
    pub(crate) direction: String,
    pub(crate) latest_rx_bytes: i64,
    pub(crate) latest_tx_bytes: i64,
    pub(crate) cycle_rx_bytes: i64,
    pub(crate) cycle_tx_bytes: i64,
    pub(crate) cycle_total_bytes: i64,
    pub(crate) sample_age_secs: Option<i64>,
    pub(crate) state: String,
    pub(crate) incomplete_reasons: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TrafficAccountingRecord {
    pub(crate) client_id: String,
    pub(crate) selectors: Vec<String>,
    pub(crate) selector_hash: String,
    pub(crate) cycle_start: String,
    pub(crate) cycle_end: String,
    pub(crate) reset_day: Option<i32>,
    pub(crate) rx_bytes: i64,
    pub(crate) tx_bytes: i64,
    pub(crate) total_bytes: i64,
    pub(crate) latest_rx_bytes: i64,
    pub(crate) latest_tx_bytes: i64,
    pub(crate) latest_total_bytes: i64,
    pub(crate) quota_rx_bytes: Option<i64>,
    pub(crate) quota_tx_bytes: Option<i64>,
    pub(crate) quota_total_bytes: Option<i64>,
    pub(crate) cycle_percent: Option<f64>,
    pub(crate) state: String,
    pub(crate) incomplete_reasons: Vec<String>,
    pub(crate) last_sample_at: Option<String>,
    pub(crate) counter_epochs_seen: i64,
    pub(crate) updated_at: String,
    pub(crate) selector_breakdown: Vec<TrafficAccountingSelectorBreakdown>,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct TrafficCounterSampleRecord {
    pub(crate) client_id: String,
    pub(crate) source_kind: String,
    pub(crate) interface: String,
    pub(crate) observed_at: String,
    pub(crate) observed_unix: i64,
    pub(crate) rx_bytes: i64,
    pub(crate) tx_bytes: i64,
    pub(crate) counter_epoch: i64,
    pub(crate) sample_source: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TrafficAccountingQuery {
    pub(crate) selector_expression: Option<String>,
    pub(crate) client_id: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) limit: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PolicyRuleRecord {
    pub(crate) id: Uuid,
    pub(crate) group_id: Uuid,
    pub(crate) rule_version: i32,
    pub(crate) sort_order: i32,
    pub(crate) name: String,
    pub(crate) enabled: bool,
    pub(crate) traffic_selector: Option<String>,
    pub(crate) condition_expression: String,
    pub(crate) window_secs: i64,
    pub(crate) severity: String,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PolicyGroupRecord {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) enabled: bool,
    pub(crate) selector_expression: String,
    pub(crate) notes: Option<String>,
    pub(crate) matched_vps_count: i64,
    pub(crate) rule_count: i64,
    pub(crate) enabled_rule_count: i64,
    pub(crate) active_warning_count: i64,
    pub(crate) active_critical_count: i64,
    pub(crate) incomplete_vps_count: i64,
    pub(crate) last_evaluated_at: Option<String>,
    pub(crate) rules: Vec<PolicyRuleRecord>,
    pub(crate) created_by: Option<Uuid>,
    pub(crate) updated_by: Option<Uuid>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PolicyRuleStateRecord {
    pub(crate) policy_rule_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) rule_version: i32,
    pub(crate) condition_true: bool,
    pub(crate) previous_condition_true: bool,
    pub(crate) window_satisfied: bool,
    pub(crate) first_true_at: Option<String>,
    pub(crate) last_true_at: Option<String>,
    pub(crate) last_false_at: Option<String>,
    pub(crate) last_evaluated_at: String,
    pub(crate) incomplete: bool,
    pub(crate) incomplete_reasons: Vec<String>,
    pub(crate) last_actual_value: Option<f64>,
    pub(crate) last_threshold_value: Option<f64>,
    pub(crate) last_fired_at: Option<String>,
    pub(crate) trigger_generation: i64,
    pub(crate) updated_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PolicyAlertRecord {
    pub(crate) id: Uuid,
    pub(crate) policy_group_id: Uuid,
    pub(crate) policy_rule_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) trigger_generation: i64,
    pub(crate) severity: String,
    pub(crate) category: String,
    pub(crate) title: String,
    pub(crate) detail: String,
    pub(crate) actual_value: Option<f64>,
    pub(crate) threshold_value: Option<f64>,
    pub(crate) payload: Value,
    pub(crate) observed_at: String,
    pub(crate) created_at: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FleetAlertPolicyQuery {
    pub(crate) limit: Option<i64>,
    pub(crate) enabled: Option<bool>,
    pub(crate) selector_expression: Option<String>,
    pub(crate) client_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PolicyAlertQuery {
    pub(crate) limit: Option<i64>,
    pub(crate) client_id: Option<String>,
    pub(crate) severity: Option<String>,
    pub(crate) category: Option<String>,
    pub(crate) policy_group_id: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PolicyRuleRequest {
    pub(crate) id: Option<Uuid>,
    pub(crate) name: String,
    #[serde(default = "default_true")]
    pub(crate) enabled: bool,
    pub(crate) traffic_selector: Option<String>,
    pub(crate) condition_expression: String,
    #[serde(default)]
    pub(crate) window_secs: i64,
    pub(crate) severity: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateFleetAlertPolicyRequest {
    pub(crate) id: Option<Uuid>,
    pub(crate) name: String,
    #[serde(default = "default_true")]
    pub(crate) enabled: bool,
    pub(crate) selector_expression: String,
    #[serde(default)]
    pub(crate) rules: Vec<PolicyRuleRequest>,
    pub(crate) notes: Option<String>,
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) preview_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PolicyDryRunRequest {
    pub(crate) id: Option<Uuid>,
    pub(crate) name: String,
    #[serde(default = "default_true")]
    pub(crate) enabled: bool,
    pub(crate) selector_expression: String,
    #[serde(default)]
    pub(crate) rules: Vec<PolicyRuleRequest>,
    pub(crate) notes: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PolicyDryRunRulePreview {
    pub(crate) rule_name: String,
    pub(crate) condition_expression: String,
    pub(crate) category: String,
    pub(crate) severity: String,
    pub(crate) true_count: i64,
    pub(crate) false_count: i64,
    pub(crate) incomplete_count: i64,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PolicyDryRunResponse {
    pub(crate) matched_vps_count: usize,
    pub(crate) invalid_rule_count: usize,
    pub(crate) incomplete_vps_count: usize,
    pub(crate) preview_hash: String,
    pub(crate) matched_vps: Vec<String>,
    pub(crate) rule_previews: Vec<PolicyDryRunRulePreview>,
    pub(crate) validation_errors: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeleteFleetAlertPolicyRequest {
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) reviewed_name: String,
}

fn default_true() -> bool {
    true
}
