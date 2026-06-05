use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vpsman_common::{
    AgentCapabilitySnapshot, AgentNoiseMode, AgentUpdateConfig, BandwidthTier, CommandEnvelope,
    JobCommand, RuntimeTunnelControl, RuntimeTunnelTopologyIntent, ServerEndpoint,
    TunnelEndpointSide, TunnelKind, TunnelPlan, TunnelPlanInput,
};

pub(crate) use crate::auth_model::*;
pub(crate) use crate::model_agent_updates::*;
pub(crate) use crate::model_backups::*;
pub(crate) use crate::model_dashboard::*;
pub(crate) use crate::model_data_sources::*;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct FleetSummary {
    pub(crate) total: usize,
    pub(crate) connected: usize,
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

#[derive(Clone, Debug, Serialize)]
pub(crate) struct AgentView {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) status: String,
    pub(crate) tags: Vec<String>,
    pub(crate) capabilities: AgentCapabilitySnapshot,
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
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TagView {
    pub(crate) name: String,
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
pub(crate) struct AuthProofRotationHistoryView {
    pub(crate) job_id: Uuid,
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) status: String,
    pub(crate) target_count: i32,
    pub(crate) completed_count: i32,
    pub(crate) failed_count: i32,
    pub(crate) pending_count: i32,
    pub(crate) rotation_generation: Option<String>,
    pub(crate) payload_hash: String,
    pub(crate) created_at: String,
    pub(crate) completed_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct JobTargetView {
    pub(crate) job_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) status: String,
    pub(crate) exit_code: Option<i32>,
    pub(crate) started_at: Option<String>,
    pub(crate) completed_at: Option<String>,
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
    pub(crate) created_at: String,
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
    pub(crate) proof_required: bool,
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
}

#[derive(Clone, Debug)]
pub(crate) struct EnrollmentTokenRecord {
    pub(crate) id: Uuid,
    pub(crate) token_hash: String,
    pub(crate) token_prefix: String,
    pub(crate) purpose: String,
    pub(crate) allowed_client_id: Option<String>,
    pub(crate) requires_existing_client: bool,
    pub(crate) preserve_existing_assignments: bool,
    pub(crate) expected_old_public_key_sha256_hex: Option<String>,
    pub(crate) created_by: Option<Uuid>,
    pub(crate) created_at_unix: u64,
    pub(crate) expires_unix: u64,
    pub(crate) used_at_unix: Option<u64>,
    pub(crate) used_by_client_id: Option<String>,
    pub(crate) default_tags: Vec<String>,
    pub(crate) default_display_name: Option<String>,
    pub(crate) unmanaged_update_enabled: bool,
    pub(crate) unmanaged_update_version_url: String,
    pub(crate) unmanaged_update_interval_secs: i64,
    pub(crate) unmanaged_update_jitter_secs: i64,
    pub(crate) unmanaged_update_activate: bool,
    pub(crate) unmanaged_update_restart_agent: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct EnrollmentTokenView {
    pub(crate) id: Uuid,
    pub(crate) token_prefix: String,
    pub(crate) purpose: String,
    pub(crate) assigned_client_id: Option<String>,
    pub(crate) allowed_client_id: Option<String>,
    pub(crate) requires_existing_client: bool,
    pub(crate) preserve_existing_assignments: bool,
    pub(crate) expected_old_public_key_sha256_hex: Option<String>,
    pub(crate) created_by: Option<Uuid>,
    pub(crate) created_at: String,
    pub(crate) expires_at: String,
    pub(crate) used_at: Option<String>,
    pub(crate) used_by_client_id: Option<String>,
    pub(crate) default_tags: Vec<String>,
    pub(crate) default_display_name: Option<String>,
    pub(crate) unmanaged_update_enabled: bool,
    pub(crate) unmanaged_update_version_url: String,
    pub(crate) unmanaged_update_interval_secs: i64,
    pub(crate) unmanaged_update_jitter_secs: i64,
    pub(crate) unmanaged_update_activate: bool,
    pub(crate) unmanaged_update_restart_agent: bool,
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
    pub(crate) server_ed25519_public_key_configured: bool,
    pub(crate) discovery_trusted_server_key_count: usize,
    pub(crate) gateway_server_public_key_configured: bool,
    pub(crate) enrolled_client_count: usize,
    pub(crate) current_key_revoked_count: usize,
    pub(crate) revocation_count: usize,
    pub(crate) rebuild_reenrollment_token_count: usize,
    pub(crate) active_rebuild_reenrollment_token_count: usize,
    pub(crate) clients: Vec<KeyLifecycleClientView>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateClientKeyRevocationRequest {
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateEnrollmentTokenRequest {
    pub(crate) ttl_secs: Option<u64>,
    pub(crate) purpose: Option<String>,
    pub(crate) allowed_client_id: Option<String>,
    #[serde(default)]
    pub(crate) confirmed_reenrollment: bool,
    pub(crate) preserve_existing_assignments: Option<bool>,
    #[serde(default)]
    pub(crate) default_tags: Vec<String>,
    #[serde(default)]
    pub(crate) default_display_name: Option<String>,
    #[serde(default)]
    pub(crate) unmanaged_update_enabled: Option<bool>,
    #[serde(default)]
    pub(crate) unmanaged_update_version_url: Option<String>,
    #[serde(default)]
    pub(crate) unmanaged_update_interval_secs: Option<i64>,
    #[serde(default)]
    pub(crate) unmanaged_update_jitter_secs: Option<i64>,
    #[serde(default)]
    pub(crate) unmanaged_update_activate: Option<bool>,
    #[serde(default)]
    pub(crate) unmanaged_update_restart_agent: Option<bool>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CreateEnrollmentTokenResponse {
    pub(crate) id: Uuid,
    pub(crate) token: String,
    pub(crate) token_prefix: String,
    pub(crate) purpose: String,
    pub(crate) assigned_client_id: Option<String>,
    pub(crate) allowed_client_id: Option<String>,
    pub(crate) requires_existing_client: bool,
    pub(crate) preserve_existing_assignments: bool,
    pub(crate) expected_old_public_key_sha256_hex: Option<String>,
    pub(crate) expires_at: String,
    pub(crate) default_tags: Vec<String>,
    pub(crate) default_display_name: Option<String>,
    pub(crate) unmanaged_update_enabled: bool,
    pub(crate) unmanaged_update_version_url: String,
    pub(crate) unmanaged_update_interval_secs: i64,
    pub(crate) unmanaged_update_jitter_secs: i64,
    pub(crate) unmanaged_update_activate: bool,
    pub(crate) unmanaged_update_restart_agent: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ClaimEnrollmentRequest {
    pub(crate) token: String,
    #[serde(default)]
    pub(crate) client_id: Option<String>,
    pub(crate) client_public_key_hex: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ClaimEnrollmentResponse {
    pub(crate) client_id: String,
    pub(crate) display_name: String,
    pub(crate) tcp_endpoints: Vec<ServerEndpoint>,
    pub(crate) discovery_url: Option<String>,
    pub(crate) noise_mode: AgentNoiseMode,
    pub(crate) gateway_server_public_key_hex: Option<String>,
    pub(crate) server_ed25519_public_key_hex: Option<String>,
    pub(crate) discovery_trusted_server_ed25519_public_keys_hex: Vec<String>,
    pub(crate) telemetry_light_secs: u64,
    pub(crate) telemetry_full_secs: u64,
    pub(crate) tags: Vec<String>,
    pub(crate) update: AgentUpdateConfig,
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
}

#[derive(Debug, Deserialize)]
pub(crate) struct PromoteTelemetryTunnelRequest {
    pub(crate) client_id: String,
    pub(crate) interface: String,
    pub(crate) peer_client_id: String,
    pub(crate) local_underlay: String,
    pub(crate) peer_underlay: String,
    pub(crate) address_pool_cidr: String,
    pub(crate) side: Option<TunnelEndpointSide>,
    pub(crate) name: Option<String>,
    pub(crate) topology_version: Option<String>,
    pub(crate) bandwidth: Option<BandwidthTier>,
    pub(crate) latency_ms: Option<f64>,
    pub(crate) packet_loss_ratio: Option<f64>,
    pub(crate) preference: Option<f64>,
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
pub(crate) struct CreateScheduleRequest {
    pub(crate) name: String,
    pub(crate) operation: JobCommand,
    #[serde(default)]
    pub(crate) clients: Vec<String>,
    #[serde(default)]
    pub(crate) tags: Vec<String>,
    pub(crate) interval_secs: u64,
    pub(crate) start_at_unix: Option<u64>,
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
}

#[derive(Debug, Deserialize)]
pub(crate) struct DispatchScheduledJobRequest {
    #[serde(default)]
    pub(crate) confirmed: bool,
    #[serde(default)]
    pub(crate) timeout_secs: Option<u64>,
    #[serde(default)]
    pub(crate) force_unprivileged: bool,
    pub(crate) envelope: Option<CommandEnvelope>,
    #[serde(default)]
    pub(crate) envelopes: HashMap<String, CommandEnvelope>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ScheduleView {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) enabled: bool,
    pub(crate) command_type: String,
    pub(crate) operation: JobCommand,
    pub(crate) clients: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) interval_secs: i64,
    pub(crate) catch_up_policy: String,
    pub(crate) catch_up_limit: i32,
    pub(crate) retry_delay_secs: i64,
    pub(crate) max_failures: i32,
    pub(crate) failure_count: i32,
    pub(crate) last_error: Option<String>,
    pub(crate) next_run_at: String,
    pub(crate) last_run_at: Option<String>,
    pub(crate) created_at: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ScheduledJobDispatchRecord {
    pub(crate) job_id: Uuid,
    pub(crate) source_schedule_id: Option<Uuid>,
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) command_type: String,
    pub(crate) operation: JobCommand,
    pub(crate) payload_hash: String,
    pub(crate) targets: Vec<String>,
}

fn default_schedule_enabled() -> bool {
    true
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
pub(crate) struct CreateTagRequest {
    pub(crate) name: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AssignTagRequest {
    pub(crate) tag: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UpdateAgentAliasRequest {
    pub(crate) display_name: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BulkResolveRequest {
    #[serde(default)]
    pub(crate) clients: Vec<String>,
    #[serde(default)]
    pub(crate) tags: Vec<String>,
    #[serde(default)]
    pub(crate) tag_mode: Option<String>,
    #[serde(default)]
    pub(crate) destructive: bool,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct BulkResolveResponse {
    pub(crate) targets: Vec<AgentView>,
    pub(crate) target_count: usize,
    pub(crate) destructive: bool,
    pub(crate) confirmation_required: bool,
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
        accepted_targets: usize,
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
        seq: i32,
        done: bool,
    },
    JobFinished {
        job_id: Uuid,
        accepted_targets: usize,
        status: String,
    },
    BackupArtifactRecorded {
        backup_request_id: Uuid,
        client_id: String,
        artifact_id: Uuid,
    },
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateJobRequest {
    #[serde(default)]
    pub(crate) targets: Vec<String>,
    #[serde(default)]
    pub(crate) clients: Vec<String>,
    #[serde(default)]
    pub(crate) tags: Vec<String>,
    #[serde(default)]
    pub(crate) tag_mode: Option<String>,
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
    pub(crate) canary_count: Option<i32>,
    #[serde(default)]
    pub(crate) force_unprivileged: bool,
    pub(crate) privileged: bool,
    #[serde(default)]
    pub(crate) idempotency_key: Option<String>,
    #[serde(default)]
    pub(crate) reconnect_policy: Option<serde_json::Value>,
    pub(crate) envelope: Option<CommandEnvelope>,
    #[serde(default)]
    pub(crate) envelopes: HashMap<String, CommandEnvelope>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CreateJobResponse {
    pub(crate) job_id: Uuid,
    pub(crate) accepted_targets: usize,
    pub(crate) status: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ErrorResponse {
    pub(crate) error: String,
    pub(crate) status: u16,
}
