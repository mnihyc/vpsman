use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub(crate) use vpsman_common::{
    VPS_RULE_KEY_BILLING_CYCLE, VPS_RULE_KEY_BILLING_PRICE, VPS_RULE_KEY_NETWORK_INTERFACES,
    VPS_RULE_KEY_NETWORK_PORT_SPEED, VPS_RULE_KEY_NETWORK_RATE_INTERFACES,
    VPS_RULE_KEY_PRODUCT_NAME, VPS_RULE_KEY_TRAFFIC_QUOTA_RX, VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL,
    VPS_RULE_KEY_TRAFFIC_QUOTA_TX, VPS_RULE_KEY_TRAFFIC_RESET_DAY, VPS_RULE_KEY_TRAFFIC_SELECTORS,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct NetworkRateInterfaceSelection {
    all_clients: BTreeSet<String>,
    exact_by_client: BTreeMap<String, BTreeSet<String>>,
}

impl NetworkRateInterfaceSelection {
    #[cfg(test)]
    pub(crate) fn all(client_ids: &[String]) -> Self {
        Self {
            all_clients: client_ids.iter().cloned().collect(),
            exact_by_client: BTreeMap::new(),
        }
    }

    pub(crate) fn select_exact(&mut self, client_id: String, interfaces: BTreeSet<String>) {
        self.all_clients.remove(&client_id);
        self.exact_by_client.insert(client_id, interfaces);
    }

    pub(crate) fn allows(&self, client_id: &str, interface: &str) -> bool {
        if self.all_clients.contains(client_id) {
            return true;
        }
        self.exact_by_client
            .get(client_id)
            .is_some_and(|interfaces| interfaces.contains(interface))
    }

    pub(crate) fn expects_rates(&self, client_id: &str) -> bool {
        self.all_clients.contains(client_id)
            || self
                .exact_by_client
                .get(client_id)
                .is_some_and(|interfaces| !interfaces.is_empty())
    }

    pub(crate) fn client_ids(&self) -> Vec<String> {
        self.all_clients
            .iter()
            .chain(self.exact_by_client.keys())
            .cloned()
            .collect()
    }

    pub(crate) fn query_parts(&self) -> (Vec<String>, Vec<String>, Vec<String>) {
        let mut selected_clients = Vec::new();
        let mut selected_interfaces = Vec::new();
        for (client_id, interfaces) in &self.exact_by_client {
            for interface in interfaces {
                selected_clients.push(client_id.clone());
                selected_interfaces.push(interface.clone());
            }
        }
        (
            self.all_clients.iter().cloned().collect(),
            selected_clients,
            selected_interfaces,
        )
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.all_clients.is_empty() && self.exact_by_client.values().all(BTreeSet::is_empty)
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct VpsRuleValueRecord {
    pub(crate) client_id: String,
    pub(crate) key: String,
    pub(crate) value_raw: String,
    #[serde(skip)]
    pub(crate) stored_value_raw: Option<String>,
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
    pub(crate) committed_records: Vec<VpsRuleValueRecord>,
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
    pub(crate) cycle_start: Option<String>,
    pub(crate) cycle_end: Option<String>,
    pub(crate) reset_day: Option<i32>,
    pub(crate) reset_hour: Option<i32>,
    pub(crate) rx_bytes: i64,
    pub(crate) tx_bytes: i64,
    pub(crate) total_bytes: i64,
    pub(crate) diagnostic_rx_bytes: i64,
    pub(crate) diagnostic_tx_bytes: i64,
    pub(crate) diagnostic_total_bytes: i64,
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

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TrafficCounterSampleRecord {
    pub(crate) client_id: String,
    pub(crate) source_kind: String,
    pub(crate) interface: String,
    pub(crate) observed_at: String,
    pub(crate) observed_unix: i64,
    pub(crate) rx_bytes: i64,
    pub(crate) tx_bytes: i64,
    pub(crate) rx_counter_epoch: i64,
    pub(crate) tx_counter_epoch: i64,
    pub(crate) sample_source: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TrafficCounterRollupRecord {
    pub(crate) client_id: String,
    pub(crate) source_kind: String,
    pub(crate) interface: String,
    pub(crate) origin_kind: String,
    pub(crate) bucket_start: String,
    pub(crate) bucket_start_unix: i64,
    pub(crate) bucket_secs: i32,
    pub(crate) rx_bytes: i64,
    pub(crate) tx_bytes: i64,
    pub(crate) rx_valid_count: i32,
    pub(crate) tx_valid_count: i32,
    pub(crate) any_valid_count: i32,
    pub(crate) rx_reset_count: i32,
    pub(crate) tx_reset_count: i32,
    pub(crate) any_reset_count: i32,
    pub(crate) first_observed_unix: i64,
    pub(crate) latest_observed_unix: i64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TrafficAccountingQuery {
    pub(crate) selector_expression: Option<String>,
    pub(crate) client_id: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) limit: Option<i64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AlertPolicyRuleKind {
    Metric,
    State,
    Occurrence,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AlertPolicyCorrelationMode {
    NaturalKey,
    Subject,
    Global,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum AlertPolicyMetaCondition {
    Immediate,
    Sustained {
        seconds: i64,
    },
    Count {
        confirmations: i32,
        within_seconds: i64,
    },
    ElapsedSinceTrigger {
        seconds: i64,
    },
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PolicyRuleRecord {
    pub(crate) id: Uuid,
    pub(crate) group_id: Uuid,
    pub(crate) rule_version: i32,
    pub(crate) sort_order: i32,
    pub(crate) name: String,
    pub(crate) enabled: bool,
    pub(crate) rule_kind: AlertPolicyRuleKind,
    pub(crate) evidence_source: String,
    pub(crate) correlation_mode: AlertPolicyCorrelationMode,
    pub(crate) traffic_selector: Option<String>,
    pub(crate) trigger_condition_expression: String,
    pub(crate) trigger_meta_condition: Option<AlertPolicyMetaCondition>,
    pub(crate) resolve_condition_expression: Option<String>,
    pub(crate) resolve_meta_condition: Option<AlertPolicyMetaCondition>,
    pub(crate) severity: String,
    pub(crate) category: String,
    pub(crate) title_template: String,
    pub(crate) detail_template: String,
    pub(crate) system_seed_key: Option<String>,
    pub(crate) armed_after_evidence_seq: i64,
    pub(crate) armed_at: String,
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
    pub(crate) active_info_count: i64,
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
    pub(crate) lifecycle_state: String,
    pub(crate) last_confirmed_at: String,
    pub(crate) resolved_at: Option<String>,
    pub(crate) resolution_reason: Option<String>,
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
    pub(crate) rule_kind: AlertPolicyRuleKind,
    pub(crate) evidence_source: String,
    pub(crate) correlation_mode: AlertPolicyCorrelationMode,
    pub(crate) traffic_selector: Option<String>,
    pub(crate) trigger_condition_expression: String,
    #[serde(default)]
    pub(crate) trigger_meta_condition: Option<AlertPolicyMetaCondition>,
    pub(crate) resolve_condition_expression: Option<String>,
    #[serde(default)]
    pub(crate) resolve_meta_condition: Option<AlertPolicyMetaCondition>,
    pub(crate) severity: String,
    pub(crate) category: String,
    pub(crate) title_template: String,
    pub(crate) detail_template: String,
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
    /// `current` evaluates the latest authoritative state/metric facts;
    /// `prospective` describes occurrence rules whose pre-arm history cannot
    /// fire and therefore intentionally has no historical true/false counts.
    pub(crate) preview_mode: String,
    pub(crate) trigger_condition_expression: String,
    pub(crate) trigger_meta_condition: Option<AlertPolicyMetaCondition>,
    pub(crate) resolve_condition_expression: Option<String>,
    pub(crate) resolve_meta_condition: Option<AlertPolicyMetaCondition>,
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

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FleetAlertPolicyBulkAction {
    Enable,
    Disable,
    Delete,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FleetAlertPolicyBulkItem {
    pub(crate) id: Uuid,
    pub(crate) reviewed_name: String,
    pub(crate) expected_updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FleetAlertPolicyBulkRequest {
    pub(crate) action: FleetAlertPolicyBulkAction,
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) items: Vec<FleetAlertPolicyBulkItem>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct FleetAlertPolicyBulkOutcome {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) result: String,
    pub(crate) record: Option<PolicyGroupRecord>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct FleetAlertPolicyBulkResponse {
    pub(crate) action: FleetAlertPolicyBulkAction,
    pub(crate) outcomes: Vec<FleetAlertPolicyBulkOutcome>,
}

fn default_true() -> bool {
    true
}
