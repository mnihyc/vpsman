use serde::Serialize;
use vpsman_common::{
    GatewayForwardCriticalFailureCounters, GatewayForwardDropReasonCounters,
    GatewayForwardEventKindCounters,
};

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardOverviewView {
    pub(crate) window: String,
    pub(crate) generated_at: String,
    pub(crate) group_by: String,
    pub(crate) scope: DashboardScopeView,
    pub(crate) time_range: DashboardTimeRangeView,
    pub(crate) available_filters: DashboardAvailableFiltersView,
    pub(crate) summary: DashboardSummaryView,
    pub(crate) operations: DashboardOperationsView,
    pub(crate) resources: DashboardResourcesView,
    pub(crate) resource_curve: DashboardResourceCurveView,
    pub(crate) network: DashboardNetworkView,
    pub(crate) label_clusters: Vec<DashboardLabelClusterView>,
    pub(crate) drilldowns: Vec<DashboardDrilldownView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardScopeView {
    pub(crate) kind: String,
    pub(crate) value: Option<String>,
    pub(crate) label: String,
    pub(crate) query: Option<String>,
    pub(crate) matched_clients: usize,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardTimeRangeView {
    pub(crate) mode: String,
    pub(crate) window: Option<String>,
    pub(crate) start_unix: u64,
    pub(crate) end_unix: u64,
    pub(crate) start_at: String,
    pub(crate) end_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardAvailableFiltersView {
    pub(crate) windows: Vec<DashboardWindowOptionView>,
    pub(crate) group_by_options: Vec<DashboardGroupByOptionView>,
    pub(crate) providers: Vec<DashboardFilterOptionView>,
    pub(crate) countries: Vec<DashboardFilterOptionView>,
    pub(crate) tags: Vec<DashboardFilterOptionView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardGroupByOptionView {
    pub(crate) value: String,
    pub(crate) label: String,
    pub(crate) description: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardWindowOptionView {
    pub(crate) value: String,
    pub(crate) label: String,
    pub(crate) seconds: u64,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardFilterOptionView {
    pub(crate) kind: String,
    pub(crate) value: String,
    pub(crate) label: String,
    pub(crate) query: String,
    pub(crate) count: usize,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardSummaryView {
    pub(crate) total: usize,
    pub(crate) online: usize,
    pub(crate) offline: usize,
    pub(crate) stale: usize,
    pub(crate) warnings: usize,
    pub(crate) running_jobs: usize,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardOperationsView {
    pub(crate) active_alerts: usize,
    pub(crate) critical_alerts: usize,
    pub(crate) warning_alerts: usize,
    pub(crate) stale_agents: usize,
    pub(crate) running_jobs: usize,
    pub(crate) backup_pending: usize,
    pub(crate) backup_completed: usize,
    pub(crate) backup_failed: usize,
    pub(crate) recent_alerts: Vec<DashboardAlertSummaryView>,
    pub(crate) degraded_agents: Vec<DashboardAgentSummaryView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardResourcesView {
    pub(crate) sampled_clients: usize,
    pub(crate) cpu_load_avg: Option<f64>,
    pub(crate) cpu_load_max: Option<f64>,
    pub(crate) memory_used_ratio: Option<f64>,
    pub(crate) disk_free_ratio: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardResourceCurveView {
    pub(crate) metric: String,
    pub(crate) sampled_clients: usize,
    pub(crate) excluded_clients: usize,
    pub(crate) top_limit: usize,
    pub(crate) series: Vec<DashboardResourceSeriesView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardResourceSeriesView {
    pub(crate) client_id: String,
    pub(crate) label: String,
    pub(crate) current: Option<f64>,
    pub(crate) peak: Option<f64>,
    pub(crate) warning_threshold: Option<f64>,
    pub(crate) critical_threshold: Option<f64>,
    pub(crate) threshold_direction: String,
    pub(crate) points: Vec<DashboardResourcePointView>,
    pub(crate) drilldown: DashboardDrilldownView,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardResourcePointView {
    pub(crate) bucket_start: String,
    pub(crate) value: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardNetworkView {
    pub(crate) rx_bps: f64,
    pub(crate) tx_bps: f64,
    pub(crate) points: Vec<DashboardNetworkPointView>,
    pub(crate) traffic_points: Vec<DashboardTrafficPointView>,
    pub(crate) top_clients: Vec<DashboardNetworkClientView>,
    pub(crate) traffic_top_clients: Vec<DashboardTrafficClientView>,
    pub(crate) traffic_series: Vec<DashboardTrafficSeriesView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardNetworkPointView {
    pub(crate) bucket_start: String,
    pub(crate) rx_bps: f64,
    pub(crate) tx_bps: f64,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardNetworkClientView {
    pub(crate) client_id: String,
    pub(crate) label: String,
    pub(crate) rx_bps: f64,
    pub(crate) tx_bps: f64,
    pub(crate) interfaces: Vec<String>,
    pub(crate) drilldown: DashboardDrilldownView,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardTrafficClientView {
    pub(crate) client_id: String,
    pub(crate) label: String,
    pub(crate) rx_bytes: i64,
    pub(crate) tx_bytes: i64,
    pub(crate) interfaces: Vec<String>,
    pub(crate) drilldown: DashboardDrilldownView,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardTrafficPointView {
    pub(crate) bucket_start: String,
    pub(crate) rx_bytes: i64,
    pub(crate) tx_bytes: i64,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardTrafficSeriesView {
    pub(crate) client_id: String,
    pub(crate) label: String,
    pub(crate) rx_bytes: i64,
    pub(crate) tx_bytes: i64,
    pub(crate) interfaces: Vec<String>,
    pub(crate) points: Vec<DashboardTrafficPointView>,
    pub(crate) drilldown: DashboardDrilldownView,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardLabelClusterView {
    pub(crate) label: String,
    pub(crate) kind: String,
    pub(crate) query: Option<String>,
    pub(crate) total: usize,
    pub(crate) online: usize,
    pub(crate) offline: usize,
    pub(crate) stale: usize,
    pub(crate) warnings: usize,
    pub(crate) running_jobs: usize,
    pub(crate) rx_bps: f64,
    pub(crate) tx_bps: f64,
    pub(crate) drilldown: DashboardDrilldownView,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardAlertSummaryView {
    pub(crate) id: String,
    pub(crate) severity: String,
    pub(crate) category: String,
    pub(crate) title: String,
    pub(crate) client_id: Option<String>,
    pub(crate) client_label: Option<String>,
    pub(crate) observed_at: String,
    pub(crate) drilldown: DashboardDrilldownView,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardAgentSummaryView {
    pub(crate) client_id: String,
    pub(crate) label: String,
    pub(crate) status: String,
    pub(crate) tags: Vec<String>,
    pub(crate) drilldown: DashboardDrilldownView,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DashboardDrilldownView {
    pub(crate) label: String,
    pub(crate) view: String,
    pub(crate) subpage: String,
    pub(crate) query: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SystemDashboardDbPoolView {
    pub(crate) max_connections: u32,
    pub(crate) open_connections: u32,
    pub(crate) idle_connections: u32,
    pub(crate) in_use_connections: u32,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct SystemDashboardDispatchView {
    pub(crate) active_jobs: i64,
    pub(crate) queued_jobs: i64,
    pub(crate) running_jobs: i64,
    pub(crate) queue_depth: i64,
    pub(crate) total_dispatch_attempts: i64,
    pub(crate) retried_targets: i64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct SystemDashboardTargetsView {
    pub(crate) queued: i64,
    pub(crate) dispatching: i64,
    pub(crate) running: i64,
    pub(crate) active: i64,
    pub(crate) deadline_expired_active: i64,
    pub(crate) control_timeout_last_24h: i64,
    pub(crate) agent_timeout_last_24h: i64,
    pub(crate) canceled_last_24h: i64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct SystemDashboardCancellationsView {
    pub(crate) requested: i64,
    pub(crate) sent: i64,
    pub(crate) acked: i64,
    pub(crate) awaiting_ack: i64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct SystemDashboardGatewayEventsView {
    pub(crate) queued_events: Option<u64>,
    pub(crate) delivered_events: Option<u64>,
    pub(crate) retry_attempts: Option<u64>,
    pub(crate) active_queues: Option<u64>,
    pub(crate) current_queue_depth: Option<u64>,
    pub(crate) oldest_event_age_secs: Option<u64>,
    pub(crate) dropped_events: Option<u64>,
    pub(crate) telemetry_dropped_events: Option<u64>,
    pub(crate) expired_events: Option<u64>,
    pub(crate) critical_failures: Option<u64>,
    pub(crate) dropped_by_kind: GatewayForwardEventKindCounters,
    pub(crate) dropped_by_reason: GatewayForwardDropReasonCounters,
    pub(crate) critical_failures_by_reason: GatewayForwardCriticalFailureCounters,
    pub(crate) retained_output_truncated_events: Option<u64>,
    pub(crate) rejected_agent_connections: Option<u64>,
    pub(crate) status: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SystemDashboardView {
    pub(crate) generated_at: String,
    pub(crate) window: String,
    pub(crate) bucket_secs: i32,
    pub(crate) current: SystemDashboardSnapshotView,
    pub(crate) capacity: SystemDashboardCapacityView,
    pub(crate) series: Vec<SystemMetricSeriesView>,
    pub(crate) notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SystemDashboardSnapshotView {
    pub(crate) db_pool: SystemDashboardDbPoolView,
    pub(crate) dispatch: SystemDashboardDispatchView,
    pub(crate) targets: SystemDashboardTargetsView,
    pub(crate) cancellations: SystemDashboardCancellationsView,
    pub(crate) gateway_events: SystemDashboardGatewayEventsView,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct SystemDashboardCapacityView {
    pub(crate) api_db_pool: Option<u32>,
    pub(crate) worker_db_pool: Option<u32>,
    pub(crate) dispatcher_batch: Option<i64>,
    pub(crate) dispatcher_in_flight: Option<usize>,
    pub(crate) dispatch_ack_secs: Option<u64>,
    pub(crate) event_post_secs: Option<u64>,
    pub(crate) internal_http_read_secs: Option<u64>,
    pub(crate) control_deadline_grace_secs: Option<u64>,
    pub(crate) worker_schedule_command_secs: Option<u64>,
    pub(crate) agent_offline_secs: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SystemMetricSeriesView {
    pub(crate) metric: String,
    pub(crate) label: String,
    pub(crate) unit: String,
    pub(crate) points: Vec<SystemMetricPointView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SystemMetricPointView {
    pub(crate) bucket_start: String,
    pub(crate) avg_value: f64,
    pub(crate) max_value: f64,
    pub(crate) latest_value: f64,
    pub(crate) sample_count: i32,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SystemMetricRollupView {
    pub(crate) metric: String,
    pub(crate) bucket_start: String,
    pub(crate) sample_count: i32,
    pub(crate) avg_value: f64,
    pub(crate) max_value: f64,
    pub(crate) latest_value: f64,
}
