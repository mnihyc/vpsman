use serde::Serialize;
use uuid::Uuid;
use vpsman_common::TunnelAddressPair;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TopologyGraphView {
    pub(crate) nodes: Vec<TopologyGraphNodeView>,
    pub(crate) edges: Vec<TopologyGraphEdgeView>,
    pub(crate) generated_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TopologyGraphNodeView {
    pub(crate) client_id: String,
    pub(crate) display_name: String,
    pub(crate) status: String,
    pub(crate) tags: Vec<String>,
    pub(crate) tunnel_count: i32,
    pub(crate) healthy_tunnel_count: i32,
    pub(crate) degraded_tunnel_count: i32,
    pub(crate) latest_observed_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TopologyGraphEdgeView {
    pub(crate) plan_id: Uuid,
    pub(crate) topology_identity_hash: String,
    pub(crate) plan_name: String,
    pub(crate) interface_name: String,
    pub(crate) kind: String,
    pub(crate) left_client_id: String,
    pub(crate) right_client_id: String,
    pub(crate) enabled: bool,
    pub(crate) health: String,
    pub(crate) left_runtime_state: String,
    pub(crate) right_runtime_state: String,
    pub(crate) left_runtime_reason: Option<String>,
    pub(crate) right_runtime_reason: Option<String>,
    pub(crate) left_reachability_state: String,
    pub(crate) right_reachability_state: String,
    pub(crate) left_reachability_reason: Option<String>,
    pub(crate) right_reachability_reason: Option<String>,
    pub(crate) left_observed_at: Option<String>,
    pub(crate) right_observed_at: Option<String>,
    pub(crate) unavailable_client_ids: Vec<String>,
    pub(crate) availability_reasons: Vec<String>,
    pub(crate) neighbor_state: String,
    pub(crate) probe_state: String,
    pub(crate) runtime_state: String,
    pub(crate) runtime_reasons: Vec<String>,
    pub(crate) adapter_state: String,
    pub(crate) routing_state: String,
    pub(crate) kernel_link_probe_state: String,
    pub(crate) kernel_neighbor_probe_state: String,
    pub(crate) kernel_route_probe_state: String,
    pub(crate) kernel_namespace_covered: bool,
    pub(crate) desired_missing_count: i64,
    pub(crate) stale_present_count: i64,
    pub(crate) bandwidth_mbps: u32,
    pub(crate) recommended_ospf_cost: Option<i32>,
    pub(crate) cost_delta: Option<i32>,
    pub(crate) latency_avg_ms: Option<f64>,
    pub(crate) latency_series_ms: Vec<f64>,
    pub(crate) packet_loss_avg_ratio: Option<f64>,
    pub(crate) throughput_avg_mbps: Option<f64>,
    pub(crate) throughput_max_mbps: Option<f64>,
    pub(crate) sample_count: i64,
    pub(crate) degraded_count: i64,
    pub(crate) latest_observed_at: Option<String>,
    pub(crate) left_tunnel_address: String,
    pub(crate) right_tunnel_address: String,
    pub(crate) ipv4_tunnel: Option<TunnelAddressPair>,
    pub(crate) ipv6_tunnel: Option<TunnelAddressPair>,
    pub(crate) latency_primary_family: String,
}
