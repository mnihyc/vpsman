use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Accepted raw telemetry samples support realtime and short-range inspection.
// Tiered rollups are the authoritative long-term historical source.
pub const DEFAULT_TELEMETRY_SAMPLE_RETENTION_DAYS: i32 = 1;
pub const DEFAULT_TELEMETRY_ROLLUP_RETENTION_DAYS: i32 = 3_650;
pub const TRAFFIC_COUNTER_RAW_RETENTION_DAYS: i32 = DEFAULT_TELEMETRY_SAMPLE_RETENTION_DAYS;
// Keep the configurable final observation horizon at least as long as the
// fixed exact inspection copy. Rollup fragments may be retained longer, but
// never disappear while they are the only copy of an automatic observation.
pub const MIN_NETWORK_OBSERVATION_ROLLUP_RETENTION_DAYS: i32 =
    DEFAULT_TELEMETRY_SAMPLE_RETENTION_DAYS;
pub const DEFAULT_TELEMETRY_RETENTION_PRUNE_LIMIT: i32 = 10_000;
pub const DEFAULT_NETWORK_OBSERVATION_RETENTION_PRUNE_LIMIT: i32 = 5_000;
pub const TELEMETRY_HISTORY_TIERS: [HistoryTier; 7] = [
    HistoryTier::new(60, 2),
    HistoryTier::new(5 * 60, 8),
    HistoryTier::new(30 * 60, 31),
    HistoryTier::new(60 * 60, 91),
    HistoryTier::new(3 * 60 * 60, 181),
    HistoryTier::new(6 * 60 * 60, 366),
    HistoryTier::new(24 * 60 * 60, 3_650),
];
pub const TRAFFIC_COUNTER_HISTORY_TIERS: [HistoryTier; 4] = [
    TELEMETRY_HISTORY_TIERS[3],
    TELEMETRY_HISTORY_TIERS[4],
    TELEMETRY_HISTORY_TIERS[5],
    TELEMETRY_HISTORY_TIERS[6],
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryTier {
    pub bucket_secs: i32,
    pub retain_days: i32,
}

impl HistoryTier {
    pub const fn new(bucket_secs: i32, retain_days: i32) -> Self {
        Self {
            bucket_secs,
            retain_days,
        }
    }
}
// The operator policy controls the final traffic-rollup horizon, not exact
// samples. Keep one complete maximum-length monthly cycle reconstructable.
pub const MIN_TRAFFIC_COUNTER_ROLLUP_RETENTION_DAYS: i32 = 32;
pub const MAX_TELEMETRY_DISKS: usize = 256;
pub const DISK_SEMANTICS_PERSISTENT_BLOCK_FILESYSTEMS_V1: &str = "persistent_block_filesystems_v1";
pub const MAX_TELEMETRY_NETWORKS: usize = 512;
pub const MAX_TELEMETRY_TUNNELS: usize = 512;
pub const MAX_TELEMETRY_PING_RESULTS: usize = 16;
pub const MAX_TUNNEL_REACHABILITY_OBSERVATIONS: usize = 512;
pub const MIN_TUNNEL_REACHABILITY_FRESH_SECS: u64 = 45;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct LoadAverage {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct MemoryStat {
    pub total_bytes: u64,
    pub available_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_total_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_available_bytes: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CpuStat {
    pub load: LoadAverage,
    pub cores: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utilization_ratio: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct DiskStat {
    pub mountpoint: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct NetworkStat {
    pub interface: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// Kernel socket-table entry counts reported by the agent. TCP includes every
/// state (including listeners); UDP is the corresponding UDP table count.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ConnectionStat {
    pub tcp: u64,
    pub udp: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct RuntimeTunnelAdapterHealthStat {
    pub status: String,
    pub checked_unix: u64,
    #[serde(default)]
    pub configured: bool,
    #[serde(default)]
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_sha256_hex: Option<String>,
    #[serde(default)]
    pub timed_out: bool,
    #[serde(default)]
    pub output_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout_sha256_hex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr_sha256_hex: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct RuntimeTunnelStat {
    pub interface: String,
    pub kind: String,
    pub ownership_mode: String,
    #[serde(default = "default_runtime_tunnel_mutation_policy")]
    pub mutation_policy: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operstate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtu: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_type: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traffic_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traffic_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traffic_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traffic_checked_unix: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topology_identity_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_evidence_identity_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_runtime_manager: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_side: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_health: Option<RuntimeTunnelAdapterHealthStat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_monitoring_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_primary_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_checked_unix: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_avg_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packet_loss_ratio: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_healthy_windows: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_missed_windows: Option<u8>,
}

/// Canonical reported endpoint identity. A structurally plausible tunnel is
/// not current merely because it carries a UUID: projection must match this
/// complete tuple against the enabled, undeleted plan endpoint owned by the
/// client transaction.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ProjectedTelemetryTunnelIdentity {
    pub plan_id: Uuid,
    pub plan_name: String,
    pub interface: String,
    pub kind: String,
    pub endpoint_side: String,
    pub peer_client_id: String,
}

/// Checks only whether a reported runtime-tunnel observation has structurally
/// plausible projection metadata. This deliberately preserves the broader
/// ingest/duplicate-validation predicate: a missing peer can still make the
/// complete normalized identity below unavailable. Structural validity never
/// makes the observation current; callers must match the complete tuple to an
/// enabled, undeleted plan endpoint.
pub fn structurally_valid_projected_telemetry_tunnel(tunnel: &RuntimeTunnelStat) -> bool {
    valid_projected_telemetry_name(&tunnel.interface)
        && valid_projected_telemetry_name(&tunnel.kind)
        && tunnel
            .plan_id
            .as_deref()
            .is_some_and(|value| Uuid::parse_str(value).is_ok())
        && tunnel
            .topology_identity_hash
            .as_deref()
            .is_none_or(valid_lower_hex_sha256)
        && tunnel
            .runtime_evidence_identity_hash
            .as_deref()
            .is_none_or(valid_lower_hex_sha256)
        && tunnel
            .plan_name
            .as_deref()
            .is_some_and(valid_projected_telemetry_plan_name)
        && matches!(tunnel.endpoint_side.as_deref(), Some("left" | "right"))
}

/// Normalizes every reported field that participates in the plan-current
/// identity. Callers still must match the returned tuple to the current plan
/// endpoint; this function deliberately performs no database inference.
pub fn projected_telemetry_tunnel_identity(
    tunnel: &RuntimeTunnelStat,
) -> Option<ProjectedTelemetryTunnelIdentity> {
    structurally_valid_projected_telemetry_tunnel(tunnel).then_some(())?;
    Some(ProjectedTelemetryTunnelIdentity {
        plan_id: Uuid::parse_str(tunnel.plan_id.as_deref()?).ok()?,
        plan_name: tunnel.plan_name.clone()?,
        interface: tunnel.interface.clone(),
        kind: tunnel.kind.clone(),
        endpoint_side: tunnel.endpoint_side.clone()?,
        peer_client_id: tunnel.peer_client_id.clone()?,
    })
}

/// Returns whether an ordinal admission mask is the one exact encoding for a
/// vector of `item_count` entries. Bits are numbered least-significant-first;
/// consequently, every unused high bit in the final byte must remain zero.
///
/// Consumers must reject the complete vector when this returns `false`. A
/// truncated mask is not a partially usable prefix and an oversized mask is
/// not a forward-compatibility envelope.
pub fn ordinal_admission_mask_has_exact_shape(mask: &[u8], item_count: usize) -> bool {
    if mask.len() != item_count.div_ceil(8) {
        return false;
    }
    let final_byte_bits = item_count % 8;
    final_byte_bits == 0
        || mask.last().is_some_and(|byte| {
            let allowed_bits = ((1_u16 << final_byte_bits) - 1) as u8;
            byte & !allowed_bits == 0
        })
}

fn valid_projected_telemetry_name(value: &str) -> bool {
    (1..=64).contains(&value.len())
}

fn valid_projected_telemetry_plan_name(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

fn valid_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn default_runtime_tunnel_mutation_policy() -> String {
    "unknown".to_string()
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelReachabilitySource {
    Automatic,
    Manual,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TunnelReachabilityObservation {
    pub id: Uuid,
    pub source: TunnelReachabilitySource,
    pub plan_id: Uuid,
    pub topology_identity_hash: String,
    pub endpoint_side: crate::TunnelEndpointSide,
    pub peer_client_id: String,
    pub interface_name: String,
    pub address_family: crate::TunnelAddressFamily,
    pub target: String,
    pub measured_unix: u64,
    pub stale_after_secs: u64,
    pub transmitted: u32,
    pub received: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_min_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_avg_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_max_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_mdev_ms: Option<f64>,
    pub packet_loss_ratio: f64,
    pub healthy: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl TunnelReachabilityObservation {
    pub fn values_are_coherent(&self) -> bool {
        self.topology_identity_hash.len() == 64
            && self
                .topology_identity_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            && !self.peer_client_id.trim().is_empty()
            && !self.interface_name.trim().is_empty()
            && !self.target.trim().is_empty()
            && self.stale_after_secs >= MIN_TUNNEL_REACHABILITY_FRESH_SECS
            && self.received <= self.transmitted
            && self.packet_loss_ratio.is_finite()
            && (0.0..=1.0).contains(&self.packet_loss_ratio)
            && [
                self.latency_min_ms,
                self.latency_avg_ms,
                self.latency_max_ms,
                self.latency_mdev_ms,
            ]
            .into_iter()
            .flatten()
            .all(|value| value.is_finite() && value >= 0.0)
            && (!self.healthy || (self.received > 0 && self.latency_avg_ms.is_some()))
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct PingTargetResult {
    pub target_id: String,
    pub generation: u64,
    pub checked_unix: u64,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_avg_ms: Option<f64>,
    pub loss_ratio: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl PingTargetResult {
    pub fn values_are_coherent(&self) -> bool {
        match self.status.as_str() {
            "ok" => self.latency_avg_ms.is_some() && self.loss_ratio == 0.0,
            "degraded" => {
                self.latency_avg_ms.is_some() && self.loss_ratio > 0.0 && self.loss_ratio < 1.0
            }
            "down" | "error" => self.latency_avg_ms.is_none() && self.loss_ratio == 1.0,
            _ => false,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AgentMetrics {
    pub observed_unix: u64,
    pub hostname: String,
    pub uptime_secs: u64,
    pub cpu: CpuStat,
    pub memory: MemoryStat,
    pub disks: Vec<DiskStat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_collection_available: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_semantics: Option<String>,
    pub networks: Vec<NetworkStat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connections: Option<ConnectionStat>,
    #[serde(default)]
    pub tunnels: Vec<RuntimeTunnelStat>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ping_results: Vec<PingTargetResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tunnel_reachability: Vec<TunnelReachabilityObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port_forwarding: Option<crate::PortForwardRuntimeSnapshot>,
}

impl AgentMetrics {
    pub fn has_persistent_block_filesystem_disk_sample(&self) -> bool {
        self.disk_collection_available == Some(true)
            && self.disk_semantics.as_deref()
                == Some(DISK_SEMANTICS_PERSISTENT_BLOCK_FILESYSTEMS_V1)
    }
}

#[cfg(test)]
#[path = "tests_metrics.rs"]
mod tests;
