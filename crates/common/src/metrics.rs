use serde::{Deserialize, Serialize};

// Accepted raw telemetry samples support realtime and short-range inspection.
// One-minute rollups are the authoritative long-term historical source.
pub const DEFAULT_TELEMETRY_SAMPLE_RETENTION_DAYS: i32 = 90;
pub const DEFAULT_TELEMETRY_ROLLUP_RETENTION_DAYS: i32 = 3_650;
pub const DEFAULT_TELEMETRY_RETENTION_PRUNE_LIMIT: i32 = 10_000;
// Monthly traffic cycles span at most 31 days; the extra day keeps retention
// strictly behind the active cycle boundary.
pub const MIN_TRAFFIC_COUNTER_RETENTION_DAYS: i32 = 32;
pub const MAX_TELEMETRY_DISKS: usize = 256;
pub const MAX_TELEMETRY_NETWORKS: usize = 512;
pub const MAX_TELEMETRY_TUNNELS: usize = 512;
pub const MAX_TELEMETRY_PING_RESULTS: usize = 16;

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

fn default_runtime_tunnel_mutation_policy() -> String {
    "unknown".to_string()
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
    pub networks: Vec<NetworkStat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connections: Option<ConnectionStat>,
    #[serde(default)]
    pub tunnels: Vec<RuntimeTunnelStat>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ping_results: Vec<PingTargetResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port_forwarding: Option<crate::PortForwardRuntimeSnapshot>,
}

#[cfg(test)]
mod tests {
    use super::{AgentMetrics, ConnectionStat, CpuStat};

    #[test]
    fn cpu_utilization_is_an_optional_additive_wire_field() {
        let without_utilization: CpuStat = serde_json::from_value(serde_json::json!({
            "load": {"one": 0.5, "five": 0.4, "fifteen": 0.3},
            "cores": 2
        }))
        .unwrap();
        assert_eq!(without_utilization.utilization_ratio, None);
        assert!(serde_json::to_value(&without_utilization)
            .unwrap()
            .get("utilization_ratio")
            .is_none());

        let with_utilization: CpuStat = serde_json::from_value(serde_json::json!({
            "load": {"one": 0.5, "five": 0.4, "fifteen": 0.3},
            "cores": 2,
            "utilization_ratio": 0.75
        }))
        .unwrap();
        assert_eq!(with_utilization.utilization_ratio, Some(0.75));
    }

    #[test]
    fn socket_counts_are_atomic_and_missing_is_not_zero() {
        let mut metrics = AgentMetrics::default();
        assert!(serde_json::to_value(&metrics)
            .unwrap()
            .get("connections")
            .is_none());
        metrics.connections = Some(ConnectionStat { tcp: 18, udp: 4 });
        let value = serde_json::to_value(&metrics).unwrap();
        assert_eq!(value["connections"]["tcp"], 18);
        assert_eq!(value["connections"]["udp"], 4);
    }
}
