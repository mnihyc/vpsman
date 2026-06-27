use serde::{Deserialize, Serialize};

pub const MANAGED_IFUPDOWN_FILE: &str = "/etc/network/interfaces.d/vpsman-tunnels";
pub const MANAGED_BIRD2_FILE: &str = "/etc/bird/vpsman-ospf.conf";
pub const MANAGED_NETPLAN_FILE: &str = "/etc/netplan/90-vpsman-tunnels.yaml";
pub const MANAGED_SYSTEMD_NETWORKD_NETDEV_FILE: &str =
    "/etc/systemd/network/90-vpsman-tunnels.netdev";
pub const MANAGED_SYSTEMD_NETWORKD_NETWORK_FILE: &str =
    "/etc/systemd/network/90-vpsman-tunnels.network";

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelConfigBackend {
    #[default]
    Ifupdown,
    Netplan,
    SystemdNetworkd,
}

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

    pub(crate) fn bird2_label(self) -> &'static str {
        match self {
            Self::Gre => "GRE",
            Self::Ipip => "IPIP",
            Self::Sit => "SIT",
            Self::Fou => "FOU",
            Self::Openvpn => "OpenVPN",
            Self::Wireguard => "WireGuard",
            Self::TunTap => "TUN/TAP",
            Self::Custom => "custom",
        }
    }
}

pub type BandwidthMbps = u32;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup: Option<RuntimeTunnelCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<RuntimeTunnelCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<RuntimeTunnelCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart: Option<RuntimeTunnelCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<RuntimeTunnelCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traffic_limit_apply: Option<RuntimeTunnelCommand>,
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
pub struct LegacyBirdPeer {
    pub protocol_name: String,
    pub interface_name: String,
    pub peer_name: Option<String>,
    pub cost: Option<u16>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct LegacyBirdConfig {
    pub router_id: Option<String>,
    pub node_name: Option<String>,
    pub peers: Vec<LegacyBirdPeer>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct IfupdownConfig {
    pub interfaces: Vec<IfupdownInterface>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct IfupdownInterface {
    pub source_path: String,
    pub name: String,
    pub address: Option<String>,
    pub point_to_point: Option<String>,
    pub tunnel_kind: Option<TunnelKind>,
    pub tunnel_local: Option<String>,
    pub tunnel_remote: Option<String>,
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
    pub left_underlay: String,
    pub right_underlay: String,
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
    pub latency_ms: f64,
    pub packet_loss_ratio: f64,
    pub preference: f64,
    #[serde(default)]
    pub ospf_policy: OspfCostPolicy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
    pub left_underlay: String,
    pub right_underlay: String,
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
    pub recommended_ospf_cost: u16,
    pub ifupdown_file: String,
    pub bird2_file: String,
    pub ifupdown_snippet: String,
    pub bird2_interface_snippet: String,
    pub touched_files: Vec<String>,
    pub validation_steps: Vec<String>,
    pub rollback_notes: Vec<String>,
    pub conflicts: Vec<String>,
    pub mutates_host: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TunnelEndpointConfig {
    pub side: TunnelEndpointSide,
    pub local_client_id: String,
    pub peer_client_id: String,
    #[serde(default, skip_serializing_if = "RuntimeTunnelControl::is_default")]
    pub runtime_control: RuntimeTunnelControl,
    pub ifupdown_file: String,
    pub bird2_file: String,
    pub ifupdown_snippet: String,
    pub bird2_interface_snippet: String,
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TunnelBackendFile {
    pub managed_path: &'static str,
    pub block_kind: &'static str,
    pub contents: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TunnelBackendConfig {
    pub backend: TunnelConfigBackend,
    pub files: Vec<TunnelBackendFile>,
}
