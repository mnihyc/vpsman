use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    model::{AgentView, RuntimeConfigDispatchView, TelemetryNetworkRateView, TelemetryRollupView},
    model_alert_policies::TrafficAccountingRecord,
};

#[derive(Clone, Debug)]
pub(crate) struct PingTargetRecord {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) host: String,
    pub(crate) probe_kind: String,
    pub(crate) port: Option<i32>,
    pub(crate) enabled: bool,
    pub(crate) selector_expression: String,
    pub(crate) generation: i64,
    pub(crate) created_by: Option<Uuid>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Clone, Debug)]
pub(crate) struct PingTargetAssignmentRecord {
    pub(crate) target_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) is_primary: bool,
    pub(crate) assigned_at: String,
}

#[derive(Clone, Debug)]
pub(crate) struct PingTargetAssignmentReplacement {
    pub(crate) expected_target: PingTargetRecord,
    pub(crate) expected_client_ids: Vec<String>,
    pub(crate) next_client_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MonitoringShareTargetRecord {
    pub(crate) client_id: String,
    pub(crate) public_client_key: String,
}

#[derive(Clone)]
pub(crate) struct MonitoringShareRecord {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) token_secret: String,
    pub(crate) selector_expression: String,
    pub(crate) targets: Vec<MonitoringShareTargetRecord>,
    pub(crate) visibility: MonitoringShareVisibilityView,
    pub(crate) expires_at: String,
    pub(crate) revoked_at: Option<String>,
    pub(crate) revoked_by: Option<Uuid>,
    pub(crate) created_by: Option<Uuid>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Clone)]
pub(crate) struct MonitoringShareTargetReplacement {
    pub(crate) expected_share: MonitoringShareRecord,
    pub(crate) next_client_ids: Vec<String>,
}

impl MonitoringShareRecord {
    pub(crate) fn target_client_ids(&self) -> Vec<String> {
        self.targets
            .iter()
            .map(|target| target.client_id.clone())
            .collect()
    }

    pub(crate) fn public_client_key(&self, client_id: &str) -> Option<&str> {
        self.targets
            .iter()
            .find(|target| target.client_id == client_id)
            .map(|target| target.public_client_key.as_str())
    }

    pub(crate) fn client_id_for_public_key(&self, public_client_key: &str) -> Option<&str> {
        self.targets
            .iter()
            .find(|target| target.public_client_key == public_client_key)
            .map(|target| target.client_id.as_str())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct MonitoringShareVisitorRecord {
    pub(crate) share_id: Uuid,
    pub(crate) visitor_id: Uuid,
    pub(crate) source_ip: Option<String>,
    pub(crate) user_agent: Option<String>,
    pub(crate) first_seen_at: String,
    pub(crate) last_seen_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PingTargetRuntimeSyncView {
    pub(crate) state: String,
    pub(crate) reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PingTargetView {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) host: String,
    pub(crate) probe_kind: String,
    pub(crate) port: Option<i32>,
    pub(crate) enabled: bool,
    pub(crate) selector_expression: String,
    pub(crate) generation: i64,
    pub(crate) assigned_count: usize,
    pub(crate) target_client_ids: Vec<String>,
    pub(crate) primary_count: usize,
    pub(crate) runtime_sync: PingTargetRuntimeSyncView,
    pub(crate) target_update_available: bool,
    pub(crate) target_update_evidence_available: bool,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PingTargetAssignmentView {
    pub(crate) target_id: Uuid,
    pub(crate) client: AgentView,
    pub(crate) is_primary: bool,
    pub(crate) assigned_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PingTargetDetailView {
    pub(crate) target: PingTargetView,
    pub(crate) assignments: Vec<PingTargetAssignmentView>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PingTargetMutationRequest {
    pub(crate) name: String,
    pub(crate) host: String,
    pub(crate) probe_kind: String,
    pub(crate) port: Option<i32>,
    #[serde(default = "default_true")]
    pub(crate) enabled: bool,
    #[serde(default = "default_all_selector")]
    pub(crate) selector_expression: String,
    #[serde(default)]
    pub(crate) target_client_ids: Vec<String>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PingTargetMutationResponse {
    pub(crate) target: PingTargetDetailView,
    pub(crate) runtime_sync: Vec<RuntimeConfigDispatchView>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BulkUpdatePingTargetsRequest {
    pub(crate) target_ids: Vec<Uuid>,
    #[serde(default)]
    pub(crate) preview_hash: Option<String>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PingTargetAssignmentChangeView {
    pub(crate) target_id: Uuid,
    pub(crate) target_name: String,
    pub(crate) selector_expression: String,
    pub(crate) added_client_ids: Vec<String>,
    pub(crate) removed_client_ids: Vec<String>,
    pub(crate) unchanged_count: usize,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BulkUpdatePingTargetsResponse {
    pub(crate) preview_hash: String,
    pub(crate) applied: bool,
    pub(crate) changes: Vec<PingTargetAssignmentChangeView>,
    pub(crate) runtime_sync: Vec<RuntimeConfigDispatchView>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MakePrimaryPingTargetRequest {
    pub(crate) client_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeletePingTargetRequest {
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DeletePingTargetResponse {
    pub(crate) runtime_sync: Vec<RuntimeConfigDispatchView>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BulkPingTargetLifecycleRequest {
    pub(crate) target_ids: Vec<Uuid>,
    pub(crate) action: String,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BulkPingTargetLifecycleResponse {
    pub(crate) action: String,
    pub(crate) affected_target_ids: Vec<Uuid>,
    pub(crate) runtime_sync: Vec<RuntimeConfigDispatchView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PingRollupView {
    pub(crate) client_id: String,
    pub(crate) target_id: Uuid,
    pub(crate) target_name: String,
    pub(crate) is_primary: bool,
    pub(crate) generation: i64,
    pub(crate) bucket_start: String,
    pub(crate) bucket_secs: i32,
    pub(crate) sample_count: i32,
    pub(crate) success_count: i32,
    pub(crate) latency_avg_ms: Option<f64>,
    pub(crate) latency_min_ms: Option<f64>,
    pub(crate) latency_max_ms: Option<f64>,
    pub(crate) loss_ratio_avg: f64,
    pub(crate) loss_ratio_max: f64,
    pub(crate) latest_status: String,
    pub(crate) latest_reason: Option<String>,
    pub(crate) latest_checked_at: String,
    #[serde(skip)]
    pub(crate) latest_source_checked_unix: u64,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CurrentPingView {
    pub(crate) target_id: Uuid,
    pub(crate) target_name: String,
    pub(crate) enabled: bool,
    pub(crate) generation: i64,
    pub(crate) state: String,
    pub(crate) status: Option<String>,
    pub(crate) latency_avg_ms: Option<f64>,
    pub(crate) loss_ratio: Option<f64>,
    pub(crate) reason: Option<String>,
    pub(crate) checked_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct MonitoringCardView {
    pub(crate) client: AgentView,
    pub(crate) product_name: Option<String>,
    pub(crate) billing: Option<BillingPlanView>,
    pub(crate) system_information: Option<SystemInformationView>,
    pub(crate) port_speed: Option<PortSpeedView>,
    pub(crate) resources: Option<TelemetryRollupView>,
    pub(crate) resource_history: Vec<TelemetryRollupView>,
    pub(crate) network: Vec<TelemetryNetworkRateView>,
    pub(crate) network_history: Vec<TelemetryNetworkRateView>,
    pub(crate) network_rate_expected: bool,
    pub(crate) traffic: TrafficAccountingRecord,
    pub(crate) primary_ping: Option<CurrentPingView>,
    pub(crate) primary_ping_history: Vec<PingRollupView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PortSpeedView {
    pub(crate) bps: i64,
    pub(crate) display: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BillingPlanView {
    pub(crate) disabled: bool,
    pub(crate) price: Option<String>,
    pub(crate) currency: Option<String>,
    pub(crate) currency_display: Option<String>,
    pub(crate) period: Option<String>,
    pub(crate) period_code: Option<String>,
    pub(crate) cycle: Option<String>,
    pub(crate) display: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SystemInformationView {
    pub(crate) os_name: Option<String>,
    pub(crate) architecture: Option<String>,
    pub(crate) cpu_model: Option<String>,
    pub(crate) kernel_release: Option<String>,
    pub(crate) virtualization: Option<String>,
    pub(crate) reported_at: Option<String>,
    pub(crate) uptime_secs: Option<u64>,
    pub(crate) uptime_observed_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct MonitoringCardsPageView {
    pub(crate) items: Vec<MonitoringCardView>,
    pub(crate) offset: usize,
    pub(crate) limit: usize,
    pub(crate) total: usize,
    pub(crate) next_offset: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct MonitoringResolutionView {
    pub(crate) resources: i32,
    pub(crate) network: i32,
    pub(crate) ping: i32,
    pub(crate) traffic: i32,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct MonitoringRangeView {
    pub(crate) window: String,
    pub(crate) source: String,
    pub(crate) start_unix: u64,
    pub(crate) end_unix: u64,
    pub(crate) requested_step_secs: i32,
    pub(crate) effective_resolution_secs: i32,
    pub(crate) step_secs: i32,
    pub(crate) points: i64,
    pub(crate) effective_points: i64,
    pub(crate) resolutions: MonitoringResolutionView,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ClientMonitoringView {
    pub(crate) client: AgentView,
    pub(crate) product_name: Option<String>,
    pub(crate) system_information: Option<SystemInformationView>,
    pub(crate) range: MonitoringRangeView,
    pub(crate) resources: Vec<TelemetryRollupView>,
    pub(crate) network: Vec<TelemetryNetworkRateView>,
    pub(crate) traffic: TrafficAccountingRecord,
    pub(crate) traffic_history: Vec<TrafficHistoryPointView>,
    pub(crate) ping_targets: Vec<CurrentPingView>,
    pub(crate) ping: Vec<PingRollupView>,
    pub(crate) primary_ping: Option<CurrentPingView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TrafficHistoryPointView {
    pub(crate) bucket_start: String,
    pub(crate) bucket_secs: i32,
    pub(crate) sample_count: i32,
    pub(crate) reset_count: i32,
    pub(crate) rx_bytes: Option<i64>,
    pub(crate) tx_bytes: Option<i64>,
    pub(crate) total_bytes: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PublicMonitoringCardView {
    pub(crate) client_key: String,
    pub(crate) display_name: String,
    pub(crate) status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) product_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) billing: Option<PublicBillingPlanView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) system_information: Option<PublicSystemInformationView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) resources: Option<PublicResourceMetricView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) resource_history: Option<Vec<PublicResourceMetricView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) network: Option<PublicNetworkMetricView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) network_history: Option<Vec<PublicNetworkPointView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) traffic: Option<PublicTrafficMetricView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) primary_ping: Option<PublicPingMetricView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) primary_ping_history: Option<Vec<PublicPingPointView>>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PublicBillingPlanView {
    pub(crate) disabled: bool,
    pub(crate) display: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) period_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cycle: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PublicSystemInformationView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) os_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) architecture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cpu_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) kernel_release: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) virtualization: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reported_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) uptime_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) uptime_observed_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PublicMonitoringDataView {
    pub(crate) share: PublicMonitoringShareView,
    pub(crate) cards: Vec<PublicMonitoringCardView>,
    pub(crate) offset: usize,
    pub(crate) total: usize,
    pub(crate) next_offset: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<PublicMonitoringDetailView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PublicMonitoringDetailView {
    pub(crate) client_key: String,
    pub(crate) range: PublicMonitoringRangeView,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) resources: Option<Vec<PublicResourceMetricView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) network: Option<Vec<PublicNetworkPointView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) traffic: Option<Vec<PublicTrafficHistoryPointView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) ping_targets: Option<Vec<PublicPingMetricView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) ping: Option<Vec<PublicPingPointView>>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PublicMonitoringRangeView {
    pub(crate) window: String,
    pub(crate) source: String,
    pub(crate) start_unix: u64,
    pub(crate) end_unix: u64,
    pub(crate) requested_step_secs: i32,
    pub(crate) effective_resolution_secs: i32,
    pub(crate) step_secs: i32,
    pub(crate) points: i64,
    pub(crate) effective_points: i64,
    pub(crate) resolutions: MonitoringResolutionView,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PublicTrafficHistoryPointView {
    pub(crate) bucket_start: String,
    pub(crate) bucket_secs: i32,
    pub(crate) sample_count: i32,
    pub(crate) reset_count: i32,
    pub(crate) rx_bytes: Option<i64>,
    pub(crate) tx_bytes: Option<i64>,
    pub(crate) total_bytes: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PublicResourceMetricView {
    pub(crate) bucket_start: String,
    pub(crate) bucket_secs: i32,
    pub(crate) sample_count: i32,
    pub(crate) cpu_usage_avg: Option<f64>,
    pub(crate) cpu_cores: i32,
    pub(crate) load_1: f64,
    pub(crate) load_5: f64,
    pub(crate) load_15: f64,
    pub(crate) memory_total_bytes: i64,
    pub(crate) memory_available_bytes: i64,
    pub(crate) memory_used_ratio_avg: f64,
    pub(crate) swap_sample_count: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) swap_total_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) swap_available_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) swap_used_ratio_avg: Option<f64>,
    pub(crate) disk_sample_count: i32,
    pub(crate) disk_total_bytes: i64,
    pub(crate) disk_available_bytes: i64,
    pub(crate) disk_used_ratio_avg: f64,
    pub(crate) tcp_sockets: Option<i64>,
    pub(crate) udp_sockets: Option<i64>,
    pub(crate) connections_observed_at: Option<String>,
    pub(crate) observed_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PublicNetworkMetricView {
    pub(crate) rate_expected: bool,
    pub(crate) rx_bps: Option<f64>,
    pub(crate) tx_bps: Option<f64>,
    pub(crate) observed_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PublicNetworkPointView {
    pub(crate) bucket_start: String,
    pub(crate) bucket_secs: i32,
    pub(crate) rx_bps: f64,
    pub(crate) tx_bps: f64,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PublicTrafficMetricView {
    pub(crate) configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reset_day: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cycle_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cycle_end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rx_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tx_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) total_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) diagnostic_rx_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) diagnostic_tx_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) diagnostic_total_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) quota_rx_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) quota_tx_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) quota_total_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cycle_percent: Option<f64>,
    pub(crate) state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) observed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) port_speed: Option<PublicPortSpeedView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PublicPortSpeedView {
    pub(crate) bps: i64,
    pub(crate) display: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PublicPingMetricView {
    pub(crate) target_name: String,
    pub(crate) state: String,
    pub(crate) status: Option<String>,
    pub(crate) latency_avg_ms: Option<f64>,
    pub(crate) loss_ratio: Option<f64>,
    pub(crate) checked_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PublicPingPointView {
    pub(crate) target_name: String,
    pub(crate) bucket_start: String,
    pub(crate) bucket_secs: i32,
    pub(crate) sample_count: i32,
    pub(crate) latency_avg_ms: Option<f64>,
    pub(crate) loss_ratio: f64,
    pub(crate) status: String,
    pub(crate) checked_at: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct MonitoringShareListQuery {
    pub(crate) status: Option<String>,
    pub(crate) limit: Option<i64>,
    pub(crate) offset: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct MonitoringShareVisibilityView {
    pub(crate) identity_context: bool,
    pub(crate) billing: bool,
    pub(crate) system_information: bool,
    pub(crate) resources: bool,
    pub(crate) network: bool,
    pub(crate) traffic: bool,
    pub(crate) ping: bool,
    pub(crate) detail_history: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MonitoringShareVisibilityRequest {
    #[serde(default)]
    pub(crate) identity_context: bool,
    #[serde(default)]
    pub(crate) billing: bool,
    #[serde(default)]
    pub(crate) system_information: bool,
    #[serde(default = "default_true")]
    pub(crate) resources: bool,
    #[serde(default = "default_true")]
    pub(crate) network: bool,
    #[serde(default = "default_true")]
    pub(crate) traffic: bool,
    #[serde(default = "default_true")]
    pub(crate) ping: bool,
    #[serde(default = "default_true")]
    pub(crate) detail_history: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct MonitoringShareView {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) selector_expression: String,
    pub(crate) target_count: usize,
    pub(crate) target_client_ids: Vec<String>,
    pub(crate) target_update_available: bool,
    pub(crate) target_update_evidence_available: bool,
    pub(crate) visibility: MonitoringShareVisibilityView,
    pub(crate) status: String,
    pub(crate) expires_at: String,
    pub(crate) revoked_at: Option<String>,
    pub(crate) created_by: Option<String>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) visitor_count: usize,
    pub(crate) first_visited_at: Option<String>,
    pub(crate) last_visited_at: Option<String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct MonitoringShareUrlResponse {
    pub(crate) fragment_path: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateMonitoringShareRequest {
    pub(crate) name: String,
    #[serde(default = "default_all_selector")]
    pub(crate) selector_expression: String,
    #[serde(default)]
    pub(crate) target_client_ids: Vec<String>,
    pub(crate) visibility: MonitoringShareVisibilityRequest,
    pub(crate) expires_in_secs: u64,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Clone, Serialize)]
pub(crate) struct CreateMonitoringShareResponse {
    pub(crate) share: MonitoringShareView,
    pub(crate) fragment_path: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExtendMonitoringSharesRequest {
    pub(crate) share_ids: Vec<Uuid>,
    pub(crate) extend_by_secs: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevokeMonitoringSharesRequest {
    pub(crate) share_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BulkUpdateMonitoringShareTargetsRequest {
    pub(crate) share_ids: Vec<Uuid>,
    #[serde(default)]
    pub(crate) preview_hash: Option<String>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct MonitoringShareTargetChangeView {
    pub(crate) share_id: Uuid,
    pub(crate) share_name: String,
    pub(crate) selector_expression: String,
    pub(crate) added_client_ids: Vec<String>,
    pub(crate) removed_client_ids: Vec<String>,
    pub(crate) unchanged_count: usize,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BulkUpdateMonitoringShareTargetsResponse {
    pub(crate) preview_hash: String,
    pub(crate) applied: bool,
    pub(crate) changes: Vec<MonitoringShareTargetChangeView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct MonitoringSharesMutationResponse {
    pub(crate) shares: Vec<MonitoringShareView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PublicMonitoringShareBootstrapView {
    pub(crate) share: PublicMonitoringShareView,
    pub(crate) visitor_id: Uuid,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PublicMonitoringShareView {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) target_count: usize,
    pub(crate) visibility: MonitoringShareVisibilityView,
    pub(crate) expires_at: String,
}

fn default_true() -> bool {
    true
}

fn default_all_selector() -> String {
    "*".to_string()
}
