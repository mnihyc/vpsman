use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelKind {
    Gre,
    Gre6,
    Ipip,
    Sit,
    Fou,
    Openvpn,
    Wireguard,
    TunTap,
    Custom,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelEndpointSide {
    #[default]
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
    AgentBuiltin,
    ExternalObserved,
    CustomAdapter,
}

impl TunnelKind {
    pub const fn iproute2_underlay_family(
        self,
        fou_kind: RuntimeTunnelFouKind,
    ) -> Option<TunnelAddressFamily> {
        match self {
            Self::Gre | Self::Ipip | Self::Sit => Some(TunnelAddressFamily::Ipv4),
            Self::Gre6 => Some(TunnelAddressFamily::Ipv6),
            Self::Fou => Some(fou_kind.underlay_family()),
            _ => None,
        }
    }
}

pub type BandwidthMbps = u32;
pub const MIN_TUNNEL_MTU: u16 = 68;
pub const MIN_IPV6_TUNNEL_MTU: u16 = 1280;
pub const MAX_TUNNEL_MTU: u16 = u16::MAX;
pub const ROUTING_COST_ADAPTER_CONTRACT_VERSION: u16 = 2;

/// Returns the editable endpoint MTU baseline for a 1500-byte underlay.
///
/// Agent builtin tunnel kinds use an editable baseline suitable for a
/// 1500-byte underlay. External-only and custom kinds return `None` because
/// MTU ownership belongs to their runtime rather than the tunnel plan.
pub const fn default_tunnel_mtu(kind: TunnelKind) -> Option<u16> {
    match kind {
        TunnelKind::Gre => Some(1476),
        // IPv6 + GRE + Linux's default tunnel encapsulation-limit option.
        TunnelKind::Gre6 => Some(1448), // 1500 - 40 - 4 - 8.
        TunnelKind::Ipip | TunnelKind::Sit => Some(1480),
        TunnelKind::Fou => Some(RuntimeTunnelFouKind::Gre.default_mtu()),
        TunnelKind::Wireguard => Some(1420),
        TunnelKind::Openvpn => Some(1500),
        TunnelKind::TunTap | TunnelKind::Custom => None,
    }
}

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
    Reviewed,
    #[default]
    Automatic,
}

#[derive(Clone, Debug)]
pub struct TunnelOspfConfig {
    pub mode: OspfControlMode,
    pub planned_latency_ms: f64,
    pub planned_packet_loss_ratio: f64,
    pub preference: f64,
    pub left_cost_offset: f64,
    pub right_cost_offset: f64,
    pub left_cost_multiplier: f64,
    pub right_cost_multiplier: f64,
    pub cost_floor: u16,
    pub policy: OspfCostPolicy,
    pub min_cost_delta: u16,
    pub healthy_windows: u8,
    pub left_adapter_definition_id: Option<String>,
    pub right_adapter_definition_id: Option<String>,
    /// Preserve the signed bytes of already accepted commands that predate the
    /// directional fields. Newly planned configurations always clear this flag.
    #[doc(hidden)]
    pub legacy_cost_wire: bool,
}

impl TunnelOspfConfig {
    pub fn has_directional_cost_fields(&self) -> bool {
        !self.legacy_cost_wire
            || self.left_cost_offset != 0.0
            || self.right_cost_offset != 0.0
            || self.left_cost_multiplier != default_ospf_cost_multiplier()
            || self.right_cost_multiplier != default_ospf_cost_multiplier()
            || self.cost_floor != default_ospf_cost_floor()
    }
}

// Wire presence is replay metadata, not an operator-configured policy value.
impl PartialEq for TunnelOspfConfig {
    fn eq(&self, other: &Self) -> bool {
        self.mode == other.mode
            && self.planned_latency_ms == other.planned_latency_ms
            && self.planned_packet_loss_ratio == other.planned_packet_loss_ratio
            && self.preference == other.preference
            && self.left_cost_offset == other.left_cost_offset
            && self.right_cost_offset == other.right_cost_offset
            && self.left_cost_multiplier == other.left_cost_multiplier
            && self.right_cost_multiplier == other.right_cost_multiplier
            && self.cost_floor == other.cost_floor
            && self.policy == other.policy
            && self.min_cost_delta == other.min_cost_delta
            && self.healthy_windows == other.healthy_windows
            && self.left_adapter_definition_id == other.left_adapter_definition_id
            && self.right_adapter_definition_id == other.right_adapter_definition_id
    }
}

impl Serialize for TunnelOspfConfig {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let directional = self.has_directional_cost_fields();
        let fields = 7
            + if directional { 5 } else { 0 }
            + usize::from(self.left_adapter_definition_id.is_some())
            + usize::from(self.right_adapter_definition_id.is_some());
        let mut state = serializer.serialize_struct("TunnelOspfConfig", fields)?;
        // Preserve the existing typed field order, including the historical
        // adapter_template_id names, because command hashes cover these bytes.
        state.serialize_field("mode", &self.mode)?;
        state.serialize_field("planned_latency_ms", &self.planned_latency_ms)?;
        state.serialize_field("planned_packet_loss_ratio", &self.planned_packet_loss_ratio)?;
        state.serialize_field("preference", &self.preference)?;
        if directional {
            state.serialize_field("left_cost_offset", &self.left_cost_offset)?;
            state.serialize_field("right_cost_offset", &self.right_cost_offset)?;
            state.serialize_field("left_cost_multiplier", &self.left_cost_multiplier)?;
            state.serialize_field("right_cost_multiplier", &self.right_cost_multiplier)?;
            state.serialize_field("cost_floor", &self.cost_floor)?;
        }
        state.serialize_field("policy", &self.policy)?;
        state.serialize_field("min_cost_delta", &self.min_cost_delta)?;
        state.serialize_field("healthy_windows", &self.healthy_windows)?;
        if let Some(id) = &self.left_adapter_definition_id {
            state.serialize_field("left_adapter_template_id", id)?;
        }
        if let Some(id) = &self.right_adapter_definition_id {
            state.serialize_field("right_adapter_template_id", id)?;
        }
        state.end()
    }
}

impl<'de> Deserialize<'de> for TunnelOspfConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // A present null is invalid numeric input, not an absent legacy field.
        fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
            deserializer: D,
        ) -> Result<Option<T>, D::Error> {
            T::deserialize(deserializer).map(Some)
        }
        #[derive(Deserialize)]
        struct Wire {
            #[serde(default)]
            mode: OspfControlMode,
            planned_latency_ms: f64,
            planned_packet_loss_ratio: f64,
            preference: f64,
            #[serde(default, deserialize_with = "present")]
            left_cost_offset: Option<f64>,
            #[serde(default, deserialize_with = "present")]
            right_cost_offset: Option<f64>,
            #[serde(default, deserialize_with = "present")]
            left_cost_multiplier: Option<f64>,
            #[serde(default, deserialize_with = "present")]
            right_cost_multiplier: Option<f64>,
            #[serde(default, deserialize_with = "present")]
            cost_floor: Option<u16>,
            #[serde(default)]
            policy: OspfCostPolicy,
            #[serde(default = "default_ospf_min_cost_delta")]
            min_cost_delta: u16,
            #[serde(default = "default_ospf_healthy_windows")]
            healthy_windows: u8,
            #[serde(default, rename = "left_adapter_template_id")]
            left_adapter_definition_id: Option<String>,
            #[serde(default, rename = "right_adapter_template_id")]
            right_adapter_definition_id: Option<String>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let legacy_cost_wire = wire.left_cost_offset.is_none()
            && wire.right_cost_offset.is_none()
            && wire.left_cost_multiplier.is_none()
            && wire.right_cost_multiplier.is_none()
            && wire.cost_floor.is_none();
        Ok(Self {
            mode: wire.mode,
            planned_latency_ms: wire.planned_latency_ms,
            planned_packet_loss_ratio: wire.planned_packet_loss_ratio,
            preference: wire.preference,
            left_cost_offset: wire.left_cost_offset.unwrap_or_default(),
            right_cost_offset: wire.right_cost_offset.unwrap_or_default(),
            left_cost_multiplier: wire
                .left_cost_multiplier
                .unwrap_or_else(default_ospf_cost_multiplier),
            right_cost_multiplier: wire
                .right_cost_multiplier
                .unwrap_or_else(default_ospf_cost_multiplier),
            cost_floor: wire.cost_floor.unwrap_or_else(default_ospf_cost_floor),
            policy: wire.policy,
            min_cost_delta: wire.min_cost_delta,
            healthy_windows: wire.healthy_windows,
            left_adapter_definition_id: wire.left_adapter_definition_id,
            right_adapter_definition_id: wire.right_adapter_definition_id,
            legacy_cost_wire,
        })
    }
}

pub fn default_ospf_min_cost_delta() -> u16 {
    5
}

pub fn default_ospf_cost_multiplier() -> f64 {
    1.0
}

pub fn default_ospf_cost_floor() -> u16 {
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
    #[serde(rename = "template_id")]
    pub definition_id: String,
    #[serde(rename = "template_name")]
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
    #[serde(rename = "template_id")]
    pub definition_id: String,
    #[serde(rename = "template_name")]
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
#[serde(deny_unknown_fields)]
pub struct RoutingCostAdapterJobResult {
    pub contract_version: u16,
    pub operation: RoutingCostAdapterOperation,
    pub plan_id: String,
    pub endpoint_side: TunnelEndpointSide,
    pub client_id: String,
    #[serde(rename = "adapter_template_id")]
    pub adapter_definition_id: String,
    pub adapter_definition_hash: String,
    pub previous_cost: Option<u16>,
    pub current_cost: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
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

/// Commands run around native lifecycle transitions, not routine reconciliation.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeTunnelEndpointHooks {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_start: Option<RuntimeTunnelCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_start: Option<RuntimeTunnelCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_shutdown: Option<RuntimeTunnelCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_shutdown: Option<RuntimeTunnelCommand>,
}

impl RuntimeTunnelEndpointHooks {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeTunnelHooks {
    #[serde(
        default,
        skip_serializing_if = "RuntimeTunnelEndpointHooks::is_default"
    )]
    pub left: RuntimeTunnelEndpointHooks,
    #[serde(
        default,
        skip_serializing_if = "RuntimeTunnelEndpointHooks::is_default"
    )]
    pub right: RuntimeTunnelEndpointHooks,
}

impl RuntimeTunnelHooks {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    pub fn for_side(&self, side: TunnelEndpointSide) -> &RuntimeTunnelEndpointHooks {
        match side {
            TunnelEndpointSide::Left => &self.left,
            TunnelEndpointSide::Right => &self.right,
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

/// FOU transports one declared layer-3 tunnel protocol over UDP. This single
/// choice owns both the transmit link type and receive-port IP protocol.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTunnelFouKind {
    #[default]
    Gre,
    Gre6,
    Ipip,
    Sit,
}

impl RuntimeTunnelFouKind {
    pub const ALL: [Self; 4] = [Self::Gre, Self::Gre6, Self::Ipip, Self::Sit];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Gre => "gre",
            Self::Gre6 => "gre6",
            Self::Ipip => "ipip",
            Self::Sit => "sit",
        }
    }

    pub const fn underlay_family(self) -> TunnelAddressFamily {
        match self {
            Self::Gre6 => TunnelAddressFamily::Ipv6,
            _ => TunnelAddressFamily::Ipv4,
        }
    }

    pub const fn linux_tunnel_mode(self) -> &'static str {
        match self {
            Self::Gre => "gre",
            Self::Gre6 => "ip6gre",
            Self::Ipip => "ipip",
            Self::Sit => "sit",
        }
    }

    pub const fn ip_protocol(self) -> u8 {
        match self {
            Self::Gre | Self::Gre6 => 47,
            Self::Ipip => 4,
            Self::Sit => 41,
        }
    }

    /// Editable baseline: 1500-byte underlay minus outer IP, UDP and tunnel headers.
    pub const fn default_mtu(self) -> u16 {
        match self {
            Self::Gre => 1468,              // 1500 - 20 IPv4 - 8 UDP - 4 GRE.
            Self::Gre6 => 1440,             // 1500 - 40 IPv6 - 8 UDP - 4 GRE - 8 encap limit.
            Self::Ipip | Self::Sit => 1472, // No header between UDP and inner IP.
        }
    }

    pub const fn supports_family(self, family: TunnelAddressFamily) -> bool {
        matches!(
            (self, family),
            (Self::Gre | Self::Gre6, _)
                | (Self::Ipip, TunnelAddressFamily::Ipv4)
                | (Self::Sit, TunnelAddressFamily::Ipv6)
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeTunnelFouOptions {
    #[serde(default = "default_runtime_fou_port")]
    pub port: u16,
    #[serde(default = "default_runtime_fou_peer_port")]
    pub peer_port: u16,
    #[serde(default)]
    pub tunnel_kind: RuntimeTunnelFouKind,
}

impl RuntimeTunnelFouOptions {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeTunnelWireguardOptions {
    #[serde(default)]
    pub endpoint_mode: RuntimeTunnelWireguardEndpointMode,
    #[serde(default = "default_runtime_wireguard_listen_port")]
    pub left_listen_port: u16,
    #[serde(default = "default_runtime_wireguard_listen_port")]
    pub right_listen_port: u16,
    #[serde(default = "default_runtime_wireguard_keepalive_secs")]
    pub left_keepalive_secs: u16,
    #[serde(default = "default_runtime_wireguard_keepalive_secs")]
    pub right_keepalive_secs: u16,
}

impl RuntimeTunnelWireguardOptions {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    pub fn listen_port(&self, side: TunnelEndpointSide) -> u16 {
        match side {
            TunnelEndpointSide::Left => self.left_listen_port,
            TunnelEndpointSide::Right => self.right_listen_port,
        }
    }

    pub fn peer_listen_port(&self, side: TunnelEndpointSide) -> u16 {
        match side {
            TunnelEndpointSide::Left => self.right_listen_port,
            TunnelEndpointSide::Right => self.left_listen_port,
        }
    }

    pub fn keepalive_secs(&self, side: TunnelEndpointSide) -> u16 {
        match side {
            TunnelEndpointSide::Left => self.left_keepalive_secs,
            TunnelEndpointSide::Right => self.right_keepalive_secs,
        }
    }

    pub fn configures_peer_endpoint(&self, side: TunnelEndpointSide) -> bool {
        match self.endpoint_mode {
            RuntimeTunnelWireguardEndpointMode::Both => true,
            // A roaming side points at the fixed VPS. The fixed side omits the
            // roaming peer's destination and learns it from authenticated
            // WireGuard traffic.
            RuntimeTunnelWireguardEndpointMode::Left => side == TunnelEndpointSide::Right,
            RuntimeTunnelWireguardEndpointMode::Right => side == TunnelEndpointSide::Left,
        }
    }
}

impl Default for RuntimeTunnelWireguardOptions {
    fn default() -> Self {
        Self {
            endpoint_mode: RuntimeTunnelWireguardEndpointMode::Both,
            left_listen_port: default_runtime_wireguard_listen_port(),
            right_listen_port: default_runtime_wireguard_listen_port(),
            left_keepalive_secs: default_runtime_wireguard_keepalive_secs(),
            right_keepalive_secs: default_runtime_wireguard_keepalive_secs(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTunnelWireguardEndpointMode {
    Left,
    Right,
    #[default]
    Both,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTunnelOpenvpnTransport {
    #[default]
    Udp,
    Tcp,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeTunnelOpenvpnOptions {
    #[serde(default)]
    pub transport: RuntimeTunnelOpenvpnTransport,
    #[serde(default)]
    pub listener_side: TunnelEndpointSide,
    #[serde(default = "default_runtime_openvpn_port")]
    pub port: u16,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_config_override",
        skip_serializing_if = "Option::is_none"
    )]
    pub left_config_override: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_config_override",
        skip_serializing_if = "Option::is_none"
    )]
    pub right_config_override: Option<String>,
}

fn deserialize_optional_config_override<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.filter(|text| !text.trim().is_empty()))
}

impl RuntimeTunnelOpenvpnOptions {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    pub fn config_override(&self, side: TunnelEndpointSide) -> Option<&str> {
        match side {
            TunnelEndpointSide::Left => self.left_config_override.as_deref(),
            TunnelEndpointSide::Right => self.right_config_override.as_deref(),
        }
    }
}

impl Default for RuntimeTunnelOpenvpnOptions {
    fn default() -> Self {
        Self {
            transport: RuntimeTunnelOpenvpnTransport::Udp,
            listener_side: TunnelEndpointSide::Left,
            port: default_runtime_openvpn_port(),
            left_config_override: None,
            right_config_override: None,
        }
    }
}

impl Default for RuntimeTunnelFouOptions {
    fn default() -> Self {
        Self {
            port: default_runtime_fou_port(),
            peer_port: default_runtime_fou_peer_port(),
            tunnel_kind: RuntimeTunnelFouKind::default(),
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
        skip_serializing_if = "Option::is_none"
    )]
    pub left_adapter_definition_id: Option<String>,
    #[serde(
        default,
        rename = "right_adapter_template_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub right_adapter_definition_id: Option<String>,
    #[serde(default, skip_serializing_if = "RuntimeTunnelTrafficLimit::is_default")]
    pub traffic_limit: RuntimeTunnelTrafficLimit,
    #[serde(default, skip_serializing_if = "RuntimeTunnelFouOptions::is_default")]
    pub fou: RuntimeTunnelFouOptions,
    #[serde(
        default,
        skip_serializing_if = "RuntimeTunnelWireguardOptions::is_default"
    )]
    pub wireguard: RuntimeTunnelWireguardOptions,
    #[serde(
        default,
        skip_serializing_if = "RuntimeTunnelOpenvpnOptions::is_default"
    )]
    pub openvpn: RuntimeTunnelOpenvpnOptions,
    #[serde(default, skip_serializing_if = "RuntimeTunnelHooks::is_default")]
    pub hooks: RuntimeTunnelHooks,
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

pub fn default_runtime_wireguard_listen_port() -> u16 {
    51820
}

pub fn default_runtime_wireguard_keepalive_secs() -> u16 {
    25
}

pub fn default_runtime_openvpn_port() -> u16 {
    1194
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TunnelWireguardIdentity {
    pub private_key_base64: String,
    pub public_key_base64: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TunnelOpenvpnIdentity {
    pub private_key_pem: String,
    pub certificate_pem: String,
    pub issuer_certificate_pem: String,
    pub certificate_sha256_fingerprint: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TunnelBuiltinCredentials {
    Wireguard {
        generation: u64,
        left: TunnelWireguardIdentity,
        right: TunnelWireguardIdentity,
    },
    Openvpn {
        generation: u64,
        left: TunnelOpenvpnIdentity,
        right: TunnelOpenvpnIdentity,
    },
}

impl TunnelBuiltinCredentials {
    pub fn generation(&self) -> u64 {
        match self {
            Self::Wireguard { generation, .. } | Self::Openvpn { generation, .. } => *generation,
        }
    }

    pub fn endpoint(&self, side: TunnelEndpointSide) -> TunnelEndpointBuiltinCredentials {
        match self {
            Self::Wireguard {
                generation,
                left,
                right,
            } => {
                let (local, peer) = match side {
                    TunnelEndpointSide::Left => (left, right),
                    TunnelEndpointSide::Right => (right, left),
                };
                TunnelEndpointBuiltinCredentials::Wireguard {
                    generation: *generation,
                    local_private_key_base64: local.private_key_base64.clone(),
                    local_public_key_base64: local.public_key_base64.clone(),
                    peer_public_key_base64: peer.public_key_base64.clone(),
                }
            }
            Self::Openvpn {
                generation,
                left,
                right,
            } => {
                let (local, peer) = match side {
                    TunnelEndpointSide::Left => (left, right),
                    TunnelEndpointSide::Right => (right, left),
                };
                TunnelEndpointBuiltinCredentials::Openvpn {
                    generation: *generation,
                    local_private_key_pem: local.private_key_pem.clone(),
                    local_certificate_pem: local.certificate_pem.clone(),
                    peer_issuer_certificate_pem: peer.issuer_certificate_pem.clone(),
                    peer_certificate_sha256_fingerprint: peer
                        .certificate_sha256_fingerprint
                        .clone(),
                }
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TunnelEndpointBuiltinCredentials {
    Wireguard {
        generation: u64,
        local_private_key_base64: String,
        local_public_key_base64: String,
        peer_public_key_base64: String,
    },
    Openvpn {
        generation: u64,
        local_private_key_pem: String,
        local_certificate_pem: String,
        peer_issuer_certificate_pem: String,
        peer_certificate_sha256_fingerprint: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TunnelAddressPair {
    pub left: String,
    pub right: String,
    pub prefix_len: u8,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TunnelEndpointAdditionalAddresses {
    #[serde(default)]
    pub ipv4: Vec<String>,
    #[serde(default)]
    pub ipv6: Vec<String>,
}

impl TunnelEndpointAdditionalAddresses {
    pub fn is_empty(&self) -> bool {
        self.ipv4.is_empty() && self.ipv6.is_empty()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TunnelAdditionalAddresses {
    #[serde(default)]
    pub left: TunnelEndpointAdditionalAddresses,
    #[serde(default)]
    pub right: TunnelEndpointAdditionalAddresses,
}

impl TunnelAdditionalAddresses {
    pub fn is_empty(&self) -> bool {
        self.left.is_empty() && self.right.is_empty()
    }

    pub fn for_side(&self, side: TunnelEndpointSide) -> &TunnelEndpointAdditionalAddresses {
        match side {
            TunnelEndpointSide::Left => &self.left,
            TunnelEndpointSide::Right => &self.right,
        }
    }
}

fn default_manage_link_local() -> bool {
    true
}

fn managed_link_local_default(value: &bool) -> bool {
    *value
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
    #[serde(default, skip_serializing_if = "TunnelAdditionalAddresses::is_empty")]
    pub additional_addresses: TunnelAdditionalAddresses,
    #[serde(
        default = "default_manage_link_local",
        skip_serializing_if = "managed_link_local_default"
    )]
    pub manage_link_local: bool,
    #[serde(default)]
    pub latency_primary_family: TunnelAddressFamily,
    pub bandwidth_mbps: BandwidthMbps,
    #[serde(default)]
    pub dynamic_bandwidth: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_mtu: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_mtu: Option<u16>,
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
    #[serde(default, skip_serializing_if = "TunnelAdditionalAddresses::is_empty")]
    pub additional_addresses: TunnelAdditionalAddresses,
    #[serde(
        default = "default_manage_link_local",
        skip_serializing_if = "managed_link_local_default"
    )]
    pub manage_link_local: bool,
    #[serde(default)]
    pub latency_primary_family: TunnelAddressFamily,
    pub bandwidth_mbps: BandwidthMbps,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_mtu: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_mtu: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ospf: Option<TunnelOspfConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_ospf_cost: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_recommended_ospf_cost: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_recommended_ospf_cost: Option<u16>,
    pub conflicts: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TunnelEndpointConfig {
    pub side: TunnelEndpointSide,
    pub local_client_id: String,
    pub peer_client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_mtu: Option<u16>,
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
    #[serde(
        default,
        skip_serializing_if = "TunnelEndpointAdditionalAddresses::is_empty"
    )]
    pub additional_addresses: TunnelEndpointAdditionalAddresses,
    #[serde(
        default,
        skip_serializing_if = "TunnelEndpointAdditionalAddresses::is_empty"
    )]
    pub peer_additional_addresses: TunnelEndpointAdditionalAddresses,
    #[serde(
        default = "default_manage_link_local",
        skip_serializing_if = "managed_link_local_default"
    )]
    pub manage_link_local: bool,
}
