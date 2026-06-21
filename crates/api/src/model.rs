use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vpsman_common::{
    AgentCapabilitySnapshot, BandwidthTier, JobCommand, PrivilegeAssertion, RuntimeTunnelControl,
    RuntimeTunnelTopologyIntent, TunnelAddressFamily, TunnelAddressPair, TunnelEndpointSide,
    TunnelKind, TunnelPlan, TunnelPlanInput,
};

pub(crate) use crate::auth_model::*;
pub(crate) use crate::model_agent_updates::*;
pub(crate) use crate::model_backups::*;
pub(crate) use crate::model_dashboard::*;
pub(crate) use crate::model_data_sources::*;
pub(crate) use crate::model_server_jobs::*;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct FleetSummary {
    pub(crate) total: usize,
    pub(crate) online: usize,
    pub(crate) offline: usize,
    pub(crate) never: usize,
    pub(crate) stale: usize,
    pub(crate) warnings: usize,
    pub(crate) running_jobs: usize,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct FleetAlertView {
    pub(crate) id: String,
    pub(crate) severity: String,
    pub(crate) category: String,
    pub(crate) target_kind: String,
    pub(crate) target_id: String,
    pub(crate) client_id: Option<String>,
    pub(crate) title: String,
    pub(crate) detail: String,
    pub(crate) status: String,
    pub(crate) evidence: serde_json::Value,
    pub(crate) observed_at: String,
    pub(crate) operator_state: String,
    pub(crate) muted_until_unix: Option<i64>,
    pub(crate) escalation_level: i32,
    pub(crate) state_reason: Option<String>,
    pub(crate) state_actor_id: Option<Uuid>,
    pub(crate) state_updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FleetAlertQuery {
    pub(crate) limit: Option<i64>,
    pub(crate) client_id: Option<String>,
    pub(crate) severity: Option<String>,
    pub(crate) category: Option<String>,
    pub(crate) operator_state: Option<String>,
    pub(crate) include_muted: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct AgentView {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) status: String,
    pub(crate) tags: Vec<String>,
    pub(crate) registration_ip: Option<String>,
    pub(crate) last_ip: Option<String>,
    pub(crate) last_seen_at: Option<String>,
    pub(crate) internal_build_number: u64,
    pub(crate) process_incarnation_id: Option<Uuid>,
    pub(crate) stale_since: Option<String>,
    pub(crate) stale_reason: Option<String>,
    pub(crate) capabilities: AgentCapabilitySnapshot,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct DeleteAgentRequest {
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) reason: Option<String>,
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DeleteAgentResponse {
    pub(crate) client_id: String,
    pub(crate) deleted: bool,
    pub(crate) deleted_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct GatewaySessionView {
    pub(crate) id: Uuid,
    pub(crate) gateway_id: String,
    pub(crate) client_id: String,
    pub(crate) status: String,
    pub(crate) noise_public_key_hex: Option<String>,
    pub(crate) started_at: String,
    pub(crate) last_seen_at: String,
    pub(crate) ended_at: Option<String>,
    pub(crate) end_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ClientStatusHistoryView {
    pub(crate) id: Uuid,
    pub(crate) client_id: String,
    pub(crate) from_status: Option<String>,
    pub(crate) to_status: String,
    pub(crate) reason: String,
    pub(crate) metadata: serde_json::Value,
    pub(crate) created_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TelemetryRollupView {
    pub(crate) client_id: String,
    pub(crate) bucket_start: String,
    pub(crate) bucket_secs: i32,
    pub(crate) sample_count: i32,
    pub(crate) cpu_load_1_avg: f64,
    pub(crate) cpu_load_1_max: f64,
    pub(crate) memory_total_bytes_max: i64,
    pub(crate) memory_available_bytes_avg: i64,
    pub(crate) memory_available_bytes_min: i64,
    pub(crate) disk_total_bytes_max: i64,
    pub(crate) disk_available_bytes_avg: i64,
    pub(crate) disk_available_bytes_min: i64,
    pub(crate) network_rx_bytes_max: i64,
    pub(crate) network_tx_bytes_max: i64,
    pub(crate) latest_observed_at: String,
    pub(crate) updated_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TelemetryNetworkRateView {
    pub(crate) client_id: String,
    pub(crate) interface: String,
    pub(crate) bucket_start: String,
    pub(crate) bucket_secs: i32,
    pub(crate) sample_count: i32,
    pub(crate) rx_bytes_avg: i64,
    pub(crate) tx_bytes_avg: i64,
    pub(crate) rx_bytes_delta: i64,
    pub(crate) tx_bytes_delta: i64,
    pub(crate) rx_bps_avg: f64,
    pub(crate) tx_bps_avg: f64,
    pub(crate) updated_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TelemetryTunnelAdapterHealthView {
    pub(crate) status: String,
    pub(crate) checked_unix: i64,
    pub(crate) configured: bool,
    pub(crate) success: bool,
    pub(crate) exit_code: Option<i32>,
    pub(crate) reason: Option<String>,
    pub(crate) duration_ms: i64,
    pub(crate) command_sha256_hex: Option<String>,
    pub(crate) timed_out: bool,
    pub(crate) output_truncated: bool,
    pub(crate) stdout_sha256_hex: Option<String>,
    pub(crate) stderr_sha256_hex: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TelemetryTunnelView {
    pub(crate) client_id: String,
    pub(crate) observed_at: String,
    pub(crate) interface: String,
    pub(crate) kind: String,
    pub(crate) ownership_mode: String,
    pub(crate) mutation_policy: String,
    pub(crate) promotion_required: bool,
    pub(crate) plan_correlation: String,
    pub(crate) plan_id: Option<Uuid>,
    pub(crate) plan_name: Option<String>,
    pub(crate) plan_runtime_manager: Option<String>,
    pub(crate) endpoint_side: Option<String>,
    pub(crate) peer_client_id: Option<String>,
    pub(crate) source: String,
    pub(crate) operstate: Option<String>,
    pub(crate) mtu: Option<i64>,
    pub(crate) link_type: Option<i64>,
    pub(crate) address: Option<String>,
    pub(crate) rx_bytes: i64,
    pub(crate) tx_bytes: i64,
    pub(crate) traffic_source: Option<String>,
    pub(crate) traffic_status: Option<String>,
    pub(crate) traffic_reason: Option<String>,
    pub(crate) traffic_checked_unix: Option<i64>,
    pub(crate) adapter_health: Option<TelemetryTunnelAdapterHealthView>,
    pub(crate) latency_monitoring_enabled: Option<bool>,
    pub(crate) latency_status: Option<String>,
    pub(crate) latency_reason: Option<String>,
    pub(crate) latency_primary_family: Option<String>,
    pub(crate) latency_target: Option<String>,
    pub(crate) latency_checked_unix: Option<i64>,
    pub(crate) latency_avg_ms: Option<f64>,
    pub(crate) packet_loss_ratio: Option<f64>,
    pub(crate) latency_healthy_windows: Option<i32>,
    pub(crate) latency_missed_windows: Option<i32>,
    pub(crate) auto_ospf_enabled: Option<bool>,
    pub(crate) auto_ospf_status: Option<String>,
    pub(crate) auto_ospf_reason: Option<String>,
    pub(crate) auto_ospf_current_cost: Option<i32>,
    pub(crate) auto_ospf_recommended_cost: Option<i32>,
    pub(crate) auto_ospf_updated_unix: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TagView {
    pub(crate) name: String,
    pub(crate) display_order: i64,
    pub(crate) clients: Vec<AgentView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct JobHistoryView {
    pub(crate) id: Uuid,
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) command_type: String,
    pub(crate) privileged: bool,
    pub(crate) status: String,
    pub(crate) target_count: i32,
    pub(crate) payload_hash: String,
    pub(crate) created_at: String,
    pub(crate) completed_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct JobTargetView {
    pub(crate) job_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) status: String,
    pub(crate) message: Option<String>,
    pub(crate) exit_code: Option<i32>,
    pub(crate) started_at: Option<String>,
    pub(crate) completed_at: Option<String>,
    pub(crate) process_incarnation_id: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct JobOutputView {
    pub(crate) job_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) seq: i32,
    pub(crate) stream: String,
    pub(crate) data_base64: String,
    pub(crate) storage: String,
    pub(crate) artifact_object_key: Option<String>,
    pub(crate) artifact_sha256_hex: Option<String>,
    pub(crate) artifact_size_bytes: Option<i64>,
    pub(crate) exit_code: Option<i32>,
    pub(crate) done: bool,
    pub(crate) received_at: Option<String>,
    pub(crate) created_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct JobOutputListItemView {
    pub(crate) job_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) seq: i32,
    pub(crate) stream: String,
    pub(crate) data_base64: Option<String>,
    pub(crate) storage: String,
    pub(crate) artifact_object_key: Option<String>,
    pub(crate) artifact_sha256_hex: Option<String>,
    pub(crate) artifact_size_bytes: Option<i64>,
    pub(crate) exit_code: Option<i32>,
    pub(crate) done: bool,
    pub(crate) received_at: Option<String>,
    pub(crate) created_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct JobOutputListPageView {
    pub(crate) items: Vec<JobOutputListItemView>,
    pub(crate) limit: i64,
    pub(crate) next_cursor: Option<String>,
    pub(crate) has_more: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ProcessSupervisorInventoryView {
    pub(crate) client_id: String,
    pub(crate) name: String,
    pub(crate) status: String,
    pub(crate) pid: Option<i64>,
    pub(crate) process_exit_code: Option<i32>,
    pub(crate) source_job_id: Uuid,
    pub(crate) source_command_type: String,
    pub(crate) stdout_log: Option<String>,
    pub(crate) stderr_log: Option<String>,
    pub(crate) started_unix: Option<u64>,
    pub(crate) restart_attempts: Option<u16>,
    pub(crate) last_exit_code: Option<i32>,
    pub(crate) last_exit_unix: Option<u64>,
    pub(crate) last_restart_unix: Option<u64>,
    pub(crate) limit_effectiveness_status: Option<String>,
    pub(crate) cgroup_status: Option<String>,
    pub(crate) cgroup_process_count: Option<u64>,
    pub(crate) cgroup_cpu_weight: Option<u64>,
    pub(crate) cgroup_memory_current_bytes: Option<u64>,
    pub(crate) cgroup_pids_current: Option<u64>,
    pub(crate) observed_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct NetworkObservationView {
    pub(crate) id: Uuid,
    pub(crate) job_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) seq: i32,
    pub(crate) kind: String,
    pub(crate) role: Option<String>,
    pub(crate) plan_name: Option<String>,
    pub(crate) interface_name: Option<String>,
    pub(crate) peer_client_id: Option<String>,
    pub(crate) target: Option<String>,
    pub(crate) healthy: Option<bool>,
    pub(crate) latency_avg_ms: Option<f64>,
    pub(crate) packet_loss_ratio: Option<f64>,
    pub(crate) throughput_mbps: Option<f64>,
    pub(crate) bytes: Option<i64>,
    pub(crate) metadata: serde_json::Value,
    pub(crate) observed_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct NetworkObservationTrendView {
    pub(crate) kind: String,
    pub(crate) plan_name: Option<String>,
    pub(crate) interface_name: Option<String>,
    pub(crate) client_id: String,
    pub(crate) peer_client_id: Option<String>,
    pub(crate) sample_count: i64,
    pub(crate) healthy_count: i64,
    pub(crate) degraded_count: i64,
    pub(crate) latency_avg_ms: Option<f64>,
    pub(crate) latency_min_ms: Option<f64>,
    pub(crate) latency_max_ms: Option<f64>,
    pub(crate) packet_loss_avg_ratio: Option<f64>,
    pub(crate) throughput_avg_mbps: Option<f64>,
    pub(crate) throughput_max_mbps: Option<f64>,
    pub(crate) bytes_total: i64,
    pub(crate) latest_observed_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct NetworkOspfRecommendationView {
    pub(crate) plan_id: Uuid,
    pub(crate) plan_name: String,
    pub(crate) interface_name: String,
    pub(crate) left_client_id: String,
    pub(crate) right_client_id: String,
    pub(crate) configured_bandwidth: String,
    pub(crate) effective_bandwidth: String,
    pub(crate) plan_ospf_cost: i32,
    pub(crate) recommended_ospf_cost: i32,
    pub(crate) cost_delta: i32,
    pub(crate) latency_avg_ms: Option<f64>,
    pub(crate) packet_loss_avg_ratio: Option<f64>,
    pub(crate) throughput_avg_mbps: Option<f64>,
    pub(crate) throughput_max_mbps: Option<f64>,
    pub(crate) sample_count: i64,
    pub(crate) degraded_count: i64,
    pub(crate) latest_observed_at: Option<String>,
    pub(crate) confidence: String,
    pub(crate) reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct NetworkOspfUpdateEvidenceView {
    pub(crate) configured_bandwidth: String,
    pub(crate) effective_bandwidth: String,
    pub(crate) latency_avg_ms: Option<f64>,
    pub(crate) packet_loss_avg_ratio: Option<f64>,
    pub(crate) throughput_avg_mbps: Option<f64>,
    pub(crate) throughput_max_mbps: Option<f64>,
    pub(crate) sample_count: i64,
    pub(crate) degraded_count: i64,
    pub(crate) latest_observed_at: Option<String>,
    pub(crate) reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct NetworkOspfUpdatePlanView {
    pub(crate) plan_id: Uuid,
    pub(crate) plan_name: String,
    pub(crate) interface_name: String,
    pub(crate) left_client_id: String,
    pub(crate) right_client_id: String,
    pub(crate) bird2_file: String,
    pub(crate) current_ospf_cost: i32,
    pub(crate) recommended_ospf_cost: i32,
    pub(crate) cost_delta: i32,
    pub(crate) status: String,
    pub(crate) confidence: String,
    pub(crate) requires_approval: bool,
    pub(crate) privilege_required: bool,
    pub(crate) mutation_mode: String,
    pub(crate) approval_scope: Vec<String>,
    pub(crate) evidence: NetworkOspfUpdateEvidenceView,
    pub(crate) proposed_left_bird2_interface_snippet: String,
    pub(crate) proposed_right_bird2_interface_snippet: String,
    pub(crate) change_summary: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct AuditLogView {
    pub(crate) id: Uuid,
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) action: String,
    pub(crate) target: String,
    pub(crate) command_hash: Option<String>,
    pub(crate) metadata: serde_json::Value,
    pub(crate) created_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TunnelPlanView {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) kind: TunnelKind,
    pub(crate) enabled: bool,
    pub(crate) left_client_id: String,
    pub(crate) right_client_id: String,
    pub(crate) left_status: String,
    pub(crate) right_status: String,
    pub(crate) recommended_ospf_cost: i32,
    pub(crate) status: String,
    pub(crate) last_apply_job_id: Option<Uuid>,
    pub(crate) last_rollback_job_id: Option<Uuid>,
    pub(crate) input: TunnelPlanInput,
    pub(crate) plan: TunnelPlan,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) deleted_at: Option<String>,
    pub(crate) deleted_by: Option<Uuid>,
    pub(crate) deleted_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct AgentIdentityView {
    pub(crate) client_id: String,
    pub(crate) display_name: String,
    pub(crate) status: String,
    pub(crate) current_public_key_sha256_hex: String,
    pub(crate) tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UpsertAgentIdentityRequest {
    #[serde(default)]
    pub(crate) client_id: Option<String>,
    pub(crate) client_public_key_hex: String,
    pub(crate) display_name: Option<String>,
    #[serde(default)]
    pub(crate) tags: Vec<String>,
    #[serde(default)]
    pub(crate) replace_existing_key: bool,
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ClientKeyRevocationView {
    pub(crate) id: Uuid,
    pub(crate) client_id: String,
    pub(crate) public_key_sha256_hex: String,
    pub(crate) reason: Option<String>,
    pub(crate) revoked_by: Option<Uuid>,
    pub(crate) created_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct KeyLifecycleClientView {
    pub(crate) client_id: String,
    pub(crate) display_name: String,
    pub(crate) status: String,
    pub(crate) current_public_key_sha256_hex: Option<String>,
    pub(crate) current_key_revoked: bool,
    pub(crate) latest_revoked_at: Option<String>,
    pub(crate) latest_revocation_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct KeyLifecycleReportView {
    pub(crate) direct_identity_client_count: usize,
    pub(crate) current_key_revoked_count: usize,
    pub(crate) revocation_count: usize,
    pub(crate) clients: Vec<KeyLifecycleClientView>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateClientKeyRevocationRequest {
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) reason: Option<String>,
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GatewayIdentityValidationRequest {
    pub(crate) client_id: String,
    pub(crate) noise_public_key_hex: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct GatewayIdentityValidationResponse {
    pub(crate) accepted: bool,
    pub(crate) message: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateTunnelPlanRequest {
    #[serde(flatten)]
    pub(crate) input: TunnelPlanInput,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AllocateTunnelEndpointsRequest {
    #[serde(default)]
    pub(crate) ipv4_pool_cidr: Option<String>,
    #[serde(default)]
    pub(crate) ipv6_pool_cidr: Option<String>,
    #[serde(default)]
    pub(crate) reserved_addresses: Vec<String>,
    #[serde(default = "default_true")]
    pub(crate) include_ipv4: bool,
    #[serde(default)]
    pub(crate) include_ipv6: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct AllocateTunnelEndpointsResponse {
    pub(crate) ipv4_tunnel: Option<TunnelAddressPair>,
    pub(crate) ipv6_tunnel: Option<TunnelAddressPair>,
    pub(crate) latency_primary_family: TunnelAddressFamily,
    pub(crate) conflicts: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PromoteTelemetryTunnelRequest {
    pub(crate) client_id: String,
    pub(crate) interface: String,
    pub(crate) peer_client_id: String,
    pub(crate) local_underlay: String,
    pub(crate) peer_underlay: String,
    pub(crate) address_pool_cidr: String,
    #[serde(default)]
    pub(crate) ipv4_tunnel: Option<TunnelAddressPair>,
    #[serde(default)]
    pub(crate) ipv6_address_pool_cidr: Option<String>,
    #[serde(default)]
    pub(crate) ipv6_tunnel: Option<TunnelAddressPair>,
    #[serde(default)]
    pub(crate) latency_primary_family: TunnelAddressFamily,
    pub(crate) side: Option<TunnelEndpointSide>,
    pub(crate) name: Option<String>,
    pub(crate) topology_version: Option<String>,
    pub(crate) bandwidth: Option<BandwidthTier>,
    pub(crate) latency_ms: Option<f64>,
    pub(crate) packet_loss_ratio: Option<f64>,
    pub(crate) preference: Option<f64>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub(crate) struct PromoteTunnelPlanToAdapterRequest {
    pub(crate) plan_id: Uuid,
    pub(crate) runtime_control: RuntimeTunnelControl,
    #[serde(default)]
    pub(crate) runtime_topology: Option<RuntimeTunnelTopologyIntent>,
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HistoryQuery {
    pub(crate) limit: Option<i64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct ListQuery {
    pub(crate) limit: Option<i64>,
    pub(crate) offset: Option<i64>,
    pub(crate) sort: Option<String>,
    pub(crate) dir: Option<String>,
    pub(crate) q: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TelemetryRollupQuery {
    pub(crate) limit: Option<i64>,
    pub(crate) client_id: Option<String>,
    pub(crate) bucket_secs: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TelemetryNetworkRateQuery {
    pub(crate) limit: Option<i64>,
    pub(crate) client_id: Option<String>,
    pub(crate) interface: Option<String>,
    pub(crate) bucket_secs: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TelemetryTunnelQuery {
    pub(crate) limit: Option<i64>,
    pub(crate) client_id: Option<String>,
    pub(crate) interface: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateScheduleRequest {
    pub(crate) name: String,
    pub(crate) operation: JobCommand,
    #[serde(default)]
    pub(crate) selector_expression: String,
    pub(crate) target_client_ids: Vec<String>,
    pub(crate) cron_expr: String,
    #[serde(default = "default_schedule_timezone")]
    pub(crate) timezone: String,
    #[serde(default = "default_schedule_enabled")]
    pub(crate) enabled: bool,
    #[serde(default = "default_schedule_catch_up_policy")]
    pub(crate) catch_up_policy: String,
    #[serde(default = "default_schedule_catch_up_limit")]
    pub(crate) catch_up_limit: i32,
    #[serde(default = "default_schedule_retry_delay_secs")]
    pub(crate) retry_delay_secs: i64,
    #[serde(default = "default_schedule_max_failures")]
    pub(crate) max_failures: i32,
    #[serde(default)]
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateScheduleRequest {
    pub(crate) name: String,
    pub(crate) operation: JobCommand,
    #[serde(default)]
    pub(crate) selector_expression: String,
    pub(crate) target_client_ids: Vec<String>,
    pub(crate) cron_expr: String,
    #[serde(default = "default_schedule_timezone")]
    pub(crate) timezone: String,
    #[serde(default = "default_schedule_enabled")]
    pub(crate) enabled: bool,
    #[serde(default = "default_schedule_catch_up_policy")]
    pub(crate) catch_up_policy: String,
    #[serde(default = "default_schedule_catch_up_limit")]
    pub(crate) catch_up_limit: i32,
    #[serde(default = "default_schedule_retry_delay_secs")]
    pub(crate) retry_delay_secs: i64,
    #[serde(default = "default_schedule_max_failures")]
    pub(crate) max_failures: i32,
    #[serde(default)]
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeferScheduleRequest {
    pub(crate) deferred_until: String,
    #[serde(default)]
    pub(crate) reason: Option<String>,
    #[serde(default)]
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SchedulePrivilegeMutationRequest {
    #[serde(default)]
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateScheduleTargetsRequest {
    #[serde(default)]
    pub(crate) selector_expression: String,
    pub(crate) target_client_ids: Vec<String>,
    #[serde(default)]
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ScheduleView {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) enabled: bool,
    pub(crate) command_type: String,
    pub(crate) operation: JobCommand,
    pub(crate) selector_expression: String,
    pub(crate) target_client_ids: Vec<String>,
    pub(crate) cron_expr: String,
    pub(crate) timezone: String,
    pub(crate) next_runs: Vec<String>,
    pub(crate) catch_up_policy: String,
    pub(crate) catch_up_limit: i32,
    pub(crate) retry_delay_secs: i64,
    pub(crate) max_failures: i32,
    pub(crate) failure_count: i32,
    pub(crate) last_error: Option<String>,
    pub(crate) next_run_at: String,
    pub(crate) last_run_at: Option<String>,
    pub(crate) deferred_until: Option<String>,
    pub(crate) deleted_at: Option<String>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

fn default_schedule_enabled() -> bool {
    true
}

fn default_schedule_timezone() -> String {
    "UTC".to_string()
}

fn default_schedule_catch_up_policy() -> String {
    "skip_missed".to_string()
}

fn default_schedule_catch_up_limit() -> i32 {
    1
}

fn default_schedule_retry_delay_secs() -> i64 {
    300
}

fn default_schedule_max_failures() -> i32 {
    3
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateTagRequest {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) confirmed: bool,
    #[serde(default)]
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AssignTagRequest {
    pub(crate) tag: String,
    #[serde(default)]
    pub(crate) confirmed: bool,
    #[serde(default)]
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BulkTagMutationAction {
    Add,
    Remove,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BulkTagMutationRequest {
    pub(crate) action: BulkTagMutationAction,
    pub(crate) tag: String,
    pub(crate) selector_expression: String,
    pub(crate) target_client_ids: Vec<String>,
    #[serde(default)]
    pub(crate) confirmed: bool,
    #[serde(default)]
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeleteTagRequest {
    #[serde(default)]
    pub(crate) confirmed: bool,
    #[serde(default)]
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateTagOrderRequest {
    pub(crate) ordered_tags: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TagMutationResponse {
    pub(crate) tag: String,
    pub(crate) action: String,
    pub(crate) target_count: usize,
    pub(crate) changed_count: usize,
    pub(crate) skipped_count: usize,
    pub(crate) affected: Vec<AgentView>,
    pub(crate) schedule_impacts: Vec<ScheduleImpactView>,
    pub(crate) confirmation_required: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ScheduleImpactView {
    pub(crate) schedule_id: Uuid,
    pub(crate) name: String,
    pub(crate) command_type: String,
    pub(crate) selector_expression: String,
    pub(crate) before_target_count: usize,
    pub(crate) after_target_count: usize,
    pub(crate) added_target_count: usize,
    pub(crate) removed_target_count: usize,
    pub(crate) unchanged_target_count: usize,
    pub(crate) added_targets: Vec<AgentView>,
    pub(crate) removed_targets: Vec<AgentView>,
    pub(crate) summary: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UpdateAgentAliasRequest {
    pub(crate) display_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BulkResolveRequest {
    #[serde(default)]
    pub(crate) selector_expression: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct BulkResolveResponse {
    pub(crate) targets: Vec<AgentView>,
    pub(crate) target_count: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum WsEvent {
    Hello {
        service: String,
        stream: String,
    },
    FleetSnapshot {
        summary: FleetSummary,
        agents: Vec<AgentView>,
    },
    AgentUpdated {
        client_id: String,
        gateway_id: String,
    },
    TelemetryUpdated {
        client_id: String,
        observed_unix: u64,
        gateway_id: String,
    },
    JobRejected {
        job_id: Uuid,
        status: String,
    },
    JobOutputRecorded {
        job_id: Uuid,
        client_id: String,
        seq: i32,
        done: bool,
    },
    TerminalOutputRecorded {
        job_id: Uuid,
        client_id: String,
        session_id: Uuid,
        terminal_seq: Option<u64>,
        done: bool,
    },
    JobFinished {
        job_id: Uuid,
        status: String,
    },
    BackupArtifactRecorded {
        backup_request_id: Uuid,
        client_id: String,
        artifact_id: Uuid,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateJobRequest {
    #[serde(default)]
    pub(crate) job_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) selector_expression: String,
    pub(crate) target_client_ids: Vec<String>,
    #[serde(default)]
    pub(crate) destructive: bool,
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) command: String,
    #[serde(default)]
    pub(crate) argv: Vec<String>,
    #[serde(default)]
    pub(crate) operation: Option<JobCommand>,
    #[serde(default)]
    pub(crate) timeout_secs: Option<u64>,
    #[serde(default)]
    pub(crate) force_unprivileged: bool,
    pub(crate) privileged: bool,
    #[serde(default)]
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CreateJobTargetCounts {
    pub(crate) total: usize,
    pub(crate) queued: usize,
    pub(crate) dispatching: usize,
    pub(crate) running: usize,
    pub(crate) completed: usize,
    pub(crate) skipped: usize,
    pub(crate) rejected: usize,
    pub(crate) failed: usize,
    pub(crate) agent_lost: usize,
    pub(crate) agent_timeout: usize,
    pub(crate) control_timeout: usize,
    pub(crate) canceled: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct CreateJobResponse {
    pub(crate) job_id: Uuid,
    pub(crate) target_count: usize,
    pub(crate) status: String,
    pub(crate) target_counts: CreateJobTargetCounts,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateMigrationRunRequest {
    pub(crate) link: CreateMigrationLinkRequest,
    pub(crate) job: CreateJobRequest,
}

#[derive(Debug, Serialize)]
pub(crate) struct CreateMigrationRunResponse {
    pub(crate) migration_link: MigrationLinkView,
    pub(crate) restore_job: CreateJobResponse,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CancelJobRequest {
    #[serde(default)]
    pub(crate) reason: Option<String>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct CancelJobTargetResult {
    pub(crate) client_id: String,
    pub(crate) acked: bool,
    pub(crate) accepted: bool,
    pub(crate) applied: bool,
    pub(crate) message: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct CancelJobResponse {
    pub(crate) job_id: Uuid,
    pub(crate) requested_targets: usize,
    pub(crate) pending_canceled: usize,
    pub(crate) cancel_acks: Vec<CancelJobTargetResult>,
    pub(crate) status: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ErrorResponse {
    pub(crate) error: String,
    pub(crate) status: u16,
}
