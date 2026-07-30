use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelKind {
    Gre,
    Ipip,
    Sit,
    Fou,
    Openvpn,
    Wireguard,
    TunTap,
    Custom,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelEndpointSide {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelAddressFamily {
    #[default]
    Ipv4,
    Ipv6,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTunnelManager {
    #[default]
    AgentIproute2Managed,
    ExternalObserved,
    ExternalManagedAdapter,
}

impl TunnelKind {
    pub(crate) fn linux_tunnel_mode(self) -> Option<&'static str> {
        match self {
            Self::Gre => Some("gre"),
            Self::Ipip | Self::Fou => Some("ipip"),
            Self::Sit => Some("sit"),
            Self::Openvpn | Self::Wireguard | Self::TunTap | Self::Custom => None,
        }
    }
}

pub type BandwidthMbps = u32;
pub const ROUTING_COST_ADAPTER_CONTRACT_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct OspfCostPolicy {
    pub latency_weight: f64,
    pub loss_weight: f64,
    pub bandwidth_weight: f64,
    pub preference_bias: f64,
    pub min_cost: u16,
    pub max_cost: u16,
}

impl Default for OspfCostPolicy {
    fn default() -> Self {
        Self {
            latency_weight: 1.0,
            loss_weight: 400.0,
            bandwidth_weight: 10.0,
            preference_bias: 1.0,
            min_cost: 5,
            max_cost: 65535,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OspfControlMode {
    #[default]
    Reviewed,
    Automatic,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TunnelOspfConfig {
    #[serde(default)]
    pub mode: OspfControlMode,
    pub planned_latency_ms: f64,
    pub planned_packet_loss_ratio: f64,
    pub preference: f64,
    #[serde(default)]
    pub policy: OspfCostPolicy,
    #[serde(default = "default_ospf_min_cost_delta")]
    pub min_cost_delta: u16,
    #[serde(default = "default_ospf_healthy_windows")]
    pub healthy_windows: u8,
    #[serde(
        default,
        rename = "left_adapter_template_id",
        alias = "left_adapter_definition_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub left_adapter_definition_id: Option<String>,
    #[serde(
        default,
        rename = "right_adapter_template_id",
        alias = "right_adapter_definition_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub right_adapter_definition_id: Option<String>,
}

pub fn default_ospf_min_cost_delta() -> u16 {
    5
}

pub fn default_ospf_healthy_windows() -> u8 {
    2
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct TunnelObservation {
    pub latency_ms: f64,
    pub packet_loss_ratio: f64,
    pub bandwidth_mbps: BandwidthMbps,
    pub preference: f64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeTunnelCommand {
    pub argv: Vec<String>,
    #[serde(default = "default_runtime_command_timeout_secs")]
    pub max_timeout_secs: u64,
    #[serde(default = "default_runtime_command_max_output_bytes")]
    pub max_output_bytes: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RoutingCostAdapterCommands {
    #[serde(default)]
    pub source: RoutingCostCommandSource,
    #[serde(rename = "template_id", alias = "definition_id")]
    pub definition_id: String,
    #[serde(rename = "template_name", alias = "definition_name")]
    pub definition_name: String,
    pub definition_hash: String,
    pub status: RuntimeTunnelCommand,
    pub update: RuntimeTunnelCommand,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingCostCommandSource {
    #[default]
    PlanOverride,
    ConfigurationPreset,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeTunnelAdapterCommands {
    #[serde(rename = "template_id", alias = "definition_id")]
    pub definition_id: String,
    #[serde(rename = "template_name", alias = "definition_name")]
    pub definition_name: String,
    pub definition_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup: Option<RuntimeTunnelCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<RuntimeTunnelCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<RuntimeTunnelCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart: Option<RuntimeTunnelCommand>,
    pub status: RuntimeTunnelCommand,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traffic_limit_apply: Option<RuntimeTunnelCommand>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingCostAdapterOperation {
    Status,
    Apply,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RoutingCostAdapterRequest {
    pub contract_version: u16,
    pub operation: RoutingCostAdapterOperation,
    pub plan_id: String,
    pub plan_name: String,
    pub interface_name: String,
    pub endpoint_side: TunnelEndpointSide,
    pub client_id: String,
    pub peer_client_id: String,
    pub local_underlay: Option<String>,
    pub remote_underlay: String,
    pub local_address: String,
    pub remote_address: String,
    pub prefix_len: u8,
    pub expected_current_cost: Option<u16>,
    pub desired_cost: Option<u16>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingCostAdapterResponse {
    pub contract_version: u16,
    pub interface_name: String,
    pub ready: bool,
    pub current_cost: Option<u16>,
    pub applied_cost: Option<u16>,
    pub adapter_version: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingCostAdapterJobResult {
    pub contract_version: u16,
    pub operation: RoutingCostAdapterOperation,
    pub plan_id: String,
    pub endpoint_side: TunnelEndpointSide,
    pub client_id: String,
    #[serde(rename = "adapter_template_id", alias = "adapter_definition_id")]
    pub adapter_definition_id: String,
    pub adapter_definition_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<RoutingCostAdapterResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<RoutingCostAdapterResponse>,
    pub after: RoutingCostAdapterResponse,
}

impl Default for RuntimeTunnelCommand {
    fn default() -> Self {
        Self {
            argv: Vec::new(),
            max_timeout_secs: default_runtime_command_timeout_secs(),
            max_output_bytes: default_runtime_command_max_output_bytes(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeTunnelTrafficLimit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingress_kbps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub egress_kbps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub burst_kb: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeTunnelFouOptions {
    #[serde(default = "default_runtime_fou_port")]
    pub port: u16,
    #[serde(default = "default_runtime_fou_peer_port")]
    pub peer_port: u16,
    #[serde(default = "default_runtime_fou_ipproto")]
    pub ipproto: u8,
}

impl RuntimeTunnelFouOptions {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

impl Default for RuntimeTunnelFouOptions {
    fn default() -> Self {
        Self {
            port: default_runtime_fou_port(),
            peer_port: default_runtime_fou_peer_port(),
            ipproto: default_runtime_fou_ipproto(),
        }
    }
}

impl RuntimeTunnelTrafficLimit {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeTunnelRoute {
    pub destination_cidr: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<u32>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeTunnelTopologyIntent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub desired_interfaces: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stale_interfaces: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<RuntimeTunnelRoute>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stale_routes: Vec<RuntimeTunnelRoute>,
}

impl RuntimeTunnelTopologyIntent {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeTunnelControl {
    #[serde(default)]
    pub manager: RuntimeTunnelManager,
    #[serde(
        default,
        rename = "left_adapter_template_id",
        alias = "left_adapter_definition_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub left_adapter_definition_id: Option<String>,
    #[serde(
        default,
        rename = "right_adapter_template_id",
        alias = "right_adapter_definition_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub right_adapter_definition_id: Option<String>,
    #[serde(default, skip_serializing_if = "RuntimeTunnelTrafficLimit::is_default")]
    pub traffic_limit: RuntimeTunnelTrafficLimit,
    #[serde(default, skip_serializing_if = "RuntimeTunnelFouOptions::is_default")]
    pub fou: RuntimeTunnelFouOptions,
}

impl RuntimeTunnelControl {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

pub fn default_runtime_command_timeout_secs() -> u64 {
    10
}

pub fn default_runtime_command_max_output_bytes() -> u32 {
    16 * 1024
}

pub fn default_runtime_fou_port() -> u16 {
    5555
}

pub fn default_runtime_fou_peer_port() -> u16 {
    5555
}

pub fn default_runtime_fou_ipproto() -> u8 {
    4
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TunnelAddressPair {
    pub left: String,
    pub right: String,
    pub prefix_len: u8,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TunnelPlanInput {
    pub name: String,
    pub interface_name: String,
    pub kind: TunnelKind,
    #[serde(default, skip_serializing_if = "RuntimeTunnelControl::is_default")]
    pub runtime_control: RuntimeTunnelControl,
    #[serde(
        default,
        skip_serializing_if = "RuntimeTunnelTopologyIntent::is_default"
    )]
    pub runtime_topology: RuntimeTunnelTopologyIntent,
    pub left_client_id: String,
    pub right_client_id: String,
    pub left_remote_underlay: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_local_underlay: Option<String>,
    pub right_remote_underlay: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_local_underlay: Option<String>,
    pub address_pool_cidr: String,
    #[serde(default)]
    pub reserved_addresses: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv4_tunnel: Option<TunnelAddressPair>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv6_address_pool_cidr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv6_tunnel: Option<TunnelAddressPair>,
    #[serde(default)]
    pub latency_primary_family: TunnelAddressFamily,
    pub bandwidth_mbps: BandwidthMbps,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ospf: Option<TunnelOspfConfig>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TunnelPlan {
    pub name: String,
    pub interface_name: String,
    pub kind: TunnelKind,
    #[serde(default, skip_serializing_if = "RuntimeTunnelControl::is_default")]
    pub runtime_control: RuntimeTunnelControl,
    #[serde(
        default,
        skip_serializing_if = "RuntimeTunnelTopologyIntent::is_default"
    )]
    pub runtime_topology: RuntimeTunnelTopologyIntent,
    pub left_client_id: String,
    pub right_client_id: String,
    pub left_remote_underlay: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_local_underlay: Option<String>,
    pub right_remote_underlay: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_local_underlay: Option<String>,
    pub left_tunnel_address: String,
    pub right_tunnel_address: String,
    pub tunnel_prefix_len: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv4_tunnel: Option<TunnelAddressPair>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv6_tunnel: Option<TunnelAddressPair>,
    #[serde(default)]
    pub latency_primary_family: TunnelAddressFamily,
    pub bandwidth_mbps: BandwidthMbps,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ospf: Option<TunnelOspfConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_ospf_cost: Option<u16>,
    pub conflicts: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TunnelEndpointConfig {
    pub side: TunnelEndpointSide,
    pub local_client_id: String,
    pub peer_client_id: String,
    #[serde(default, skip_serializing_if = "RuntimeTunnelControl::is_default")]
    pub runtime_control: RuntimeTunnelControl,
    pub remote_underlay: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_underlay: Option<String>,
    pub local_tunnel_address: String,
    pub remote_tunnel_address: String,
    pub tunnel_prefix_len: u8,
    #[serde(default)]
    pub primary_family: TunnelAddressFamily,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv4_tunnel: Option<TunnelAddressPair>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv6_tunnel: Option<TunnelAddressPair>,
}
