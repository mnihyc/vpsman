use serde::{Deserialize, Serialize};

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
    #[serde(default)]
    pub promotion_required: bool,
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
}

fn default_runtime_tunnel_mutation_policy() -> String {
    "unknown".to_string()
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
    #[serde(default)]
    pub tunnels: Vec<RuntimeTunnelStat>,
}
