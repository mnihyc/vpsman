use std::{net::IpAddr, path::PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use clap::{ArgAction, Args, ValueEnum};
use uuid::Uuid;
use vpsman_common::{
    default_tunnel_mtu, payload_hash, plan_tunnel, render_tunnel_endpoint_config,
    routing_cost_update_privilege_payload, BandwidthMbps, JobCommand, OspfControlMode,
    OspfCostPolicy, RuntimeTunnelManager, RuntimeTunnelOpenvpnTransport, TunnelAddressFamily,
    TunnelAddressPair, TunnelKind, TunnelOspfConfig, TunnelPlan, TunnelPlanInput,
    DEFAULT_MAX_JOB_TIMEOUT_SECS, NETWORK_SPEED_TEST_MAX_CONNECT_TIMEOUT_MS,
    NETWORK_SPEED_TEST_MAX_DURATION_SECS, NETWORK_SPEED_TEST_MAX_MAX_BYTES,
    NETWORK_SPEED_TEST_MAX_PORT, NETWORK_SPEED_TEST_MAX_RATE_LIMIT_KBPS,
    NETWORK_SPEED_TEST_MIN_CONNECT_TIMEOUT_MS, NETWORK_SPEED_TEST_MIN_DURATION_SECS,
    NETWORK_SPEED_TEST_MIN_MAX_BYTES, NETWORK_SPEED_TEST_MIN_PORT,
    NETWORK_SPEED_TEST_MIN_RATE_LIMIT_KBPS, NETWORK_SPEED_TEST_UNLIMITED_MAX_BYTES,
    NETWORK_SPEED_TEST_UNLIMITED_RATE_LIMIT_KBPS, NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES,
};

use crate::{
    commands_schedules::selector_expression_from_targets,
    http::{http_get, http_post_json, http_put_json},
    network_runtime_args::{
        build_runtime_control, build_runtime_topology, RuntimeControlArgs, RuntimeManagerArg,
        RuntimeTopologyArgs,
    },
    privilege::{
        build_privilege_for_db, build_privilege_for_job_command, load_super_password,
        load_super_salt_hex, DbPrivilegeRequest,
    },
};

#[derive(Debug, Args)]
pub(crate) struct TunnelPlanCommand {
    #[arg(long)]
    pub(crate) name: String,
    #[arg(long)]
    pub(crate) interface_name: String,
    #[arg(long, value_enum)]
    pub(crate) kind: TunnelKindArg,
    #[arg(long)]
    pub(crate) left_client_id: String,
    #[arg(long)]
    pub(crate) right_client_id: String,
    #[arg(long, help = "Remote underlay destination reached from the left VPS")]
    pub(crate) left_remote_underlay: String,
    #[arg(
        long,
        help = "Optional local source bound on the left VPS; may be a private address behind NAT"
    )]
    pub(crate) left_local_underlay: Option<String>,
    #[arg(long, help = "Remote underlay destination reached from the right VPS")]
    pub(crate) right_remote_underlay: String,
    #[arg(
        long,
        help = "Optional local source bound on the right VPS; may be a private address behind NAT"
    )]
    pub(crate) right_local_underlay: Option<String>,
    #[arg(
        long,
        default_value = "",
        help = "Allocation context only; use tunnel-allocate to generate endpoint pairs before saving"
    )]
    pub(crate) address_pool_cidr: String,
    #[arg(long, value_delimiter = ',')]
    pub(crate) reserved_addresses: Vec<String>,
    #[arg(long)]
    pub(crate) left_tunnel_ipv4_cidr: Option<String>,
    #[arg(long)]
    pub(crate) right_tunnel_ipv4_cidr: Option<String>,
    #[arg(
        long,
        help = "IPv6 allocation context only; use tunnel-allocate to generate endpoint pairs before saving"
    )]
    pub(crate) ipv6_address_pool_cidr: Option<String>,
    #[arg(long)]
    pub(crate) left_tunnel_ipv6_cidr: Option<String>,
    #[arg(long)]
    pub(crate) right_tunnel_ipv6_cidr: Option<String>,
    #[arg(long, value_enum, default_value = "ipv4")]
    pub(crate) latency_primary_family: TunnelAddressFamilyArg,
    #[arg(long, value_name = "MBPS")]
    pub(crate) bandwidth_mbps: BandwidthMbps,
    #[arg(
        long,
        value_name = "BYTES",
        help = "Agent builtin left endpoint MTU; defaults by tunnel kind for a 1500-byte underlay"
    )]
    pub(crate) left_mtu: Option<u16>,
    #[arg(
        long,
        value_name = "BYTES",
        help = "Agent builtin right endpoint MTU; defaults by tunnel kind for a 1500-byte underlay"
    )]
    pub(crate) right_mtu: Option<u16>,
    #[arg(
        long,
        default_value_t = false,
        help = "Enable external OSPF cost control for this plan"
    )]
    pub(crate) ospf: bool,
    #[arg(long, value_enum, default_value = "reviewed")]
    pub(crate) ospf_mode: OspfControlModeArg,
    #[arg(long, requires = "ospf")]
    pub(crate) ospf_latency_ms: Option<f64>,
    #[arg(long, requires = "ospf")]
    pub(crate) ospf_packet_loss_ratio: Option<f64>,
    #[arg(long, requires = "ospf")]
    pub(crate) ospf_preference: Option<f64>,
    #[arg(long, default_value_t = 5, requires = "ospf")]
    pub(crate) ospf_min_cost_delta: u16,
    #[arg(
        long,
        default_value_t = 2,
        help = "Consecutive recent healthy probe samples required for automatic OSPF apply",
        requires = "ospf"
    )]
    pub(crate) ospf_healthy_windows: u8,
    #[arg(long, default_value_t = 1.0, requires = "ospf")]
    pub(crate) ospf_latency_weight: f64,
    #[arg(long, default_value_t = 400.0, requires = "ospf")]
    pub(crate) ospf_loss_weight: f64,
    #[arg(long, default_value_t = 10.0, requires = "ospf")]
    pub(crate) ospf_bandwidth_weight: f64,
    #[arg(long, default_value_t = 1.0, requires = "ospf")]
    pub(crate) ospf_preference_bias: f64,
    #[arg(long, default_value_t = 5, requires = "ospf")]
    pub(crate) ospf_min_cost: u16,
    #[arg(long, default_value_t = 65_535, requires = "ospf")]
    pub(crate) ospf_max_cost: u16,
    #[arg(
        long,
        requires = "ospf",
        help = "Optional left-endpoint command override; otherwise use that VPS's effective ospf_update_command preset. Invalid overrides and unconfigured effective presets fail"
    )]
    pub(crate) left_routing_adapter_definition_id: Option<String>,
    #[arg(
        long,
        requires = "ospf",
        help = "Optional right-endpoint command override; otherwise use that VPS's effective ospf_update_command preset. Invalid overrides and unconfigured effective presets fail"
    )]
    pub(crate) right_routing_adapter_definition_id: Option<String>,
    #[arg(
        long,
        value_enum,
        default_value = "builtin",
        help = "Runtime ownership: Agent builtin owns a supported kind; External observed is read-only; Custom adapter invokes operator-owned commands"
    )]
    pub(crate) runtime_manager: RuntimeManagerArg,
    #[arg(long)]
    pub(crate) left_runtime_adapter_definition_id: Option<String>,
    #[arg(long)]
    pub(crate) right_runtime_adapter_definition_id: Option<String>,
    #[arg(long)]
    pub(crate) traffic_ingress_kbps: Option<u32>,
    #[arg(long)]
    pub(crate) traffic_egress_kbps: Option<u32>,
    #[arg(long)]
    pub(crate) traffic_burst_kb: Option<u32>,
    #[arg(long)]
    pub(crate) fou_port: Option<u16>,
    #[arg(long)]
    pub(crate) fou_peer_port: Option<u16>,
    #[arg(long)]
    pub(crate) fou_ipproto: Option<u8>,
    #[arg(long, value_name = "PORT")]
    pub(crate) wireguard_left_listen_port: Option<u16>,
    #[arg(long, value_name = "PORT")]
    pub(crate) wireguard_right_listen_port: Option<u16>,
    #[arg(long, value_name = "SECONDS")]
    pub(crate) wireguard_left_keepalive_secs: Option<u16>,
    #[arg(long, value_name = "SECONDS")]
    pub(crate) wireguard_right_keepalive_secs: Option<u16>,
    #[arg(
        long,
        value_enum,
        help = "VPS with a fixed WireGuard address; the other VPS points to it and may roam. Defaults to both"
    )]
    pub(crate) wireguard_endpoint_mode: Option<WireguardEndpointModeArg>,
    #[arg(long, value_enum)]
    pub(crate) openvpn_transport: Option<OpenvpnTransportArg>,
    #[arg(long, value_enum)]
    pub(crate) openvpn_listener_side: Option<TunnelEndpointSideArg>,
    #[arg(long, value_name = "PORT")]
    pub(crate) openvpn_port: Option<u16>,
    #[arg(
        long,
        value_delimiter = ',',
        help = "Agent builtin only: exact desired interface names"
    )]
    pub(crate) topology_desired_interfaces: Vec<String>,
    #[arg(
        long,
        value_delimiter = ',',
        help = "Agent builtin only: exact stale interface names eligible for cleanup"
    )]
    pub(crate) topology_stale_interfaces: Vec<String>,
    #[arg(long, help = "Agent builtin only: exact route declaration")]
    pub(crate) topology_route: Vec<String>,
    #[arg(
        long,
        help = "Agent builtin only: exact stale route eligible for cleanup"
    )]
    pub(crate) topology_stale_route: Vec<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) save: bool,
    #[arg(
        long,
        value_name = "UUID",
        help = "Replace this exact saved plan instead of creating one"
    )]
    pub(crate) update_plan_id: Option<Uuid>,
    #[arg(
        long,
        value_name = "REVISION",
        help = "Exact revision from tunnel-plans; required with --update-plan-id"
    )]
    pub(crate) expected_revision: Option<i64>,
    #[arg(
        long,
        default_value_t = false,
        help = "Create enabled, or explicitly enable during update; omitted updates preserve lifecycle state"
    )]
    pub(crate) enabled: bool,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct TunnelAllocateCommand {
    #[arg(long)]
    pub(crate) ipv4_pool_cidr: Option<String>,
    #[arg(long)]
    pub(crate) ipv6_pool_cidr: Option<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) reserved_addresses: Vec<String>,
    #[arg(long, num_args = 0..=1, default_missing_value = "true", conflicts_with = "no_ipv4")]
    pub(crate) include_ipv4: Option<bool>,
    #[arg(long = "no-ipv4", action = ArgAction::SetTrue)]
    pub(crate) no_ipv4: bool,
    #[arg(long, num_args = 0..=1, default_missing_value = "true", conflicts_with = "no_ipv6")]
    pub(crate) include_ipv6: Option<bool>,
    #[arg(long = "no-ipv6", action = ArgAction::SetTrue)]
    pub(crate) no_ipv6: bool,
}

#[derive(Debug, Args)]
pub(crate) struct TunnelPlanExportCommand {
    #[arg(long)]
    pub(crate) plan_id: String,
    #[arg(long)]
    pub(crate) output_file: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct TunnelPlanMutationCommand {
    #[arg(long)]
    pub(crate) plan_id: String,
    #[arg(long, value_name = "REVISION")]
    pub(crate) expected_revision: Option<i64>,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct TunnelOspfStatusRefreshCommand {
    #[arg(long)]
    pub(crate) plan_id: String,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
#[value(rename_all = "snake_case")]
pub(crate) enum TunnelKindArg {
    Gre,
    Ipip,
    Sit,
    Fou,
    Openvpn,
    Wireguard,
    TunTap,
    Custom,
}

impl From<TunnelKindArg> for TunnelKind {
    fn from(value: TunnelKindArg) -> Self {
        match value {
            TunnelKindArg::Gre => Self::Gre,
            TunnelKindArg::Ipip => Self::Ipip,
            TunnelKindArg::Sit => Self::Sit,
            TunnelKindArg::Fou => Self::Fou,
            TunnelKindArg::Openvpn => Self::Openvpn,
            TunnelKindArg::Wireguard => Self::Wireguard,
            TunnelKindArg::TunTap => Self::TunTap,
            TunnelKindArg::Custom => Self::Custom,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum TunnelEndpointSideArg {
    Left,
    Right,
}

impl From<TunnelEndpointSideArg> for vpsman_common::TunnelEndpointSide {
    fn from(value: TunnelEndpointSideArg) -> Self {
        match value {
            TunnelEndpointSideArg::Left => Self::Left,
            TunnelEndpointSideArg::Right => Self::Right,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum OpenvpnTransportArg {
    Udp,
    Tcp,
}

impl From<OpenvpnTransportArg> for RuntimeTunnelOpenvpnTransport {
    fn from(value: OpenvpnTransportArg) -> Self {
        match value {
            OpenvpnTransportArg::Udp => Self::Udp,
            OpenvpnTransportArg::Tcp => Self::Tcp,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum WireguardEndpointModeArg {
    Left,
    Right,
    Both,
}

impl From<WireguardEndpointModeArg> for vpsman_common::RuntimeTunnelWireguardEndpointMode {
    fn from(value: WireguardEndpointModeArg) -> Self {
        match value {
            WireguardEndpointModeArg::Left => Self::Left,
            WireguardEndpointModeArg::Right => Self::Right,
            WireguardEndpointModeArg::Both => Self::Both,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum TunnelAddressFamilyArg {
    Ipv4,
    Ipv6,
}

impl From<TunnelAddressFamilyArg> for TunnelAddressFamily {
    fn from(value: TunnelAddressFamilyArg) -> Self {
        match value {
            TunnelAddressFamilyArg::Ipv4 => Self::Ipv4,
            TunnelAddressFamilyArg::Ipv6 => Self::Ipv6,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum OspfControlModeArg {
    Reviewed,
    Automatic,
}

impl From<OspfControlModeArg> for OspfControlMode {
    fn from(value: OspfControlModeArg) -> Self {
        match value {
            OspfControlModeArg::Reviewed => Self::Reviewed,
            OspfControlModeArg::Automatic => Self::Automatic,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct TunnelOspfCostUpdateCommand {
    #[arg(long)]
    pub(crate) plan_id: String,
    #[arg(long, help = "Plan revision from network-ospf-update-plans")]
    pub(crate) plan_revision: i64,
    #[arg(long, help = "Recommendation ID from network-ospf-update-plans")]
    pub(crate) recommendation_id: String,
    #[arg(long)]
    pub(crate) left_current_ospf_cost: Option<u16>,
    #[arg(long)]
    pub(crate) right_current_ospf_cost: Option<u16>,
    #[arg(long)]
    pub(crate) desired_ospf_cost: u16,
    #[arg(long)]
    pub(crate) left_adapter_definition_hash: String,
    #[arg(long)]
    pub(crate) right_adapter_definition_hash: String,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
    #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
    pub(crate) password_env: String,
    #[arg(long)]
    pub(crate) super_salt_hex: Option<String>,
    #[arg(long, default_value_t = 300)]
    pub(crate) privilege_ttl_secs: u64,
}

#[derive(Debug, Args)]
pub(crate) struct TunnelStatusCommand {
    #[arg(long)]
    pub(crate) plan_id: Uuid,
    #[arg(long, value_enum)]
    pub(crate) side: TunnelEndpointSideArg,
    #[arg(long, default_value_t = 60)]
    pub(crate) max_timeout_secs: u64,
}

#[derive(Debug, Args)]
pub(crate) struct TunnelProbeCommand {
    #[arg(long)]
    pub(crate) plan_id: Uuid,
    #[arg(long, value_enum)]
    pub(crate) side: TunnelEndpointSideArg,
    #[arg(long, default_value_t = 5)]
    pub(crate) count: u8,
    #[arg(long, default_value_t = 500)]
    pub(crate) interval_ms: u16,
    #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
    pub(crate) password_env: String,
    #[arg(long)]
    pub(crate) super_salt_hex: Option<String>,
    #[arg(long, default_value_t = 300)]
    pub(crate) privilege_ttl_secs: u64,
    #[arg(long, default_value_t = DEFAULT_MAX_JOB_TIMEOUT_SECS)]
    pub(crate) max_timeout_secs: u64,
}

#[derive(Debug, Args)]
pub(crate) struct NetworkTrafficImportVnstatCommand {
    #[arg(long, value_delimiter = ',', required = true)]
    pub(crate) clients: Vec<String>,
    #[arg(
        long = "interface",
        value_delimiter = ',',
        required = true,
        help = "Host interface name; repeat the flag or use a comma-separated list"
    )]
    pub(crate) interfaces: Vec<String>,
    #[arg(
        long,
        help = "Import start as YYYY-MM-DD (UTC midnight) or an RFC3339 timestamp aligned to a minute; there is no fixed lookback limit, and the end is derived from each interface's first live agent sample"
    )]
    pub(crate) start: String,
    #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
    pub(crate) password_env: String,
    #[arg(long)]
    pub(crate) super_salt_hex: Option<String>,
    #[arg(long, default_value_t = 300)]
    pub(crate) privilege_ttl_secs: u64,
    #[arg(long, default_value_t = 300)]
    pub(crate) max_timeout_secs: u64,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct TunnelSpeedTestCommand {
    #[arg(long)]
    pub(crate) plan_id: Uuid,
    #[arg(long, value_enum)]
    pub(crate) server_side: TunnelEndpointSideArg,
    #[arg(long, default_value_t = 10)]
    pub(crate) duration_secs: u8,
    #[arg(long, default_value_t = NETWORK_SPEED_TEST_UNLIMITED_MAX_BYTES)]
    pub(crate) max_bytes: u64,
    #[arg(long, default_value_t = NETWORK_SPEED_TEST_UNLIMITED_RATE_LIMIT_KBPS)]
    pub(crate) rate_limit_kbps: u32,
    #[arg(long, default_value_t = 5201)]
    pub(crate) port: u16,
    #[arg(long, default_value_t = 5_000)]
    pub(crate) connect_timeout_ms: u16,
    #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
    pub(crate) password_env: String,
    #[arg(long)]
    pub(crate) super_salt_hex: Option<String>,
    #[arg(long, default_value_t = 300)]
    pub(crate) privilege_ttl_secs: u64,
    #[arg(long, default_value_t = DEFAULT_MAX_JOB_TIMEOUT_SECS)]
    pub(crate) max_timeout_secs: u64,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

pub(crate) fn tunnel_plans(api_url: &str, token: Option<&str>) -> Result<()> {
    println!("{}", http_get(api_url, "/api/v1/tunnel-plans", token)?);
    Ok(())
}

pub(crate) fn tunnel_allocate(
    api_url: &str,
    token: Option<&str>,
    request: TunnelAllocateCommand,
) -> Result<()> {
    let include_ipv4 = match (request.include_ipv4, request.no_ipv4) {
        (_, true) => Some(false),
        (Some(value), false) => Some(value),
        (None, false) => None,
    };
    let include_ipv6 = match (request.include_ipv6, request.no_ipv6) {
        (_, true) => Some(false),
        (Some(value), false) => Some(value),
        (None, false) => None,
    };
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/tunnel-plans/allocate",
            token,
            &serde_json::json!({
                "ipv4_pool_cidr": request.ipv4_pool_cidr,
                "ipv6_pool_cidr": request.ipv6_pool_cidr,
                "reserved_addresses": request.reserved_addresses,
                "include_ipv4": include_ipv4,
                "include_ipv6": include_ipv6,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn tunnel_plan_export(
    api_url: &str,
    token: Option<&str>,
    request: TunnelPlanExportCommand,
) -> Result<()> {
    let plan = http_get(
        api_url,
        &format!("/api/v1/tunnel-plans/{}/plan", request.plan_id),
        token,
    )?;
    if let Some(path) = request.output_file {
        std::fs::write(&path, plan)
            .with_context(|| format!("failed to write tunnel plan {}", path.display()))?;
    } else {
        println!("{plan}");
    }
    Ok(())
}

pub(crate) fn set_tunnel_plan_enabled(
    api_url: &str,
    token: Option<&str>,
    request: TunnelPlanMutationCommand,
    enabled: bool,
) -> Result<()> {
    let operation = if enabled { "enable" } else { "disable" };
    let (plan_id, expected_revision) = validate_tunnel_plan_mutation(request, operation)?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/tunnel-plans/{plan_id}/{operation}"),
            token,
            &serde_json::json!({
                "confirmed": true,
                "expected_revision": expected_revision,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn delete_tunnel_plan(
    api_url: &str,
    token: Option<&str>,
    request: TunnelPlanMutationCommand,
) -> Result<()> {
    let (plan_id, expected_revision) = validate_tunnel_plan_mutation(request, "delete")?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/tunnel-plans/{plan_id}/delete"),
            token,
            &serde_json::json!({
                "confirmed": true,
                "expected_revision": expected_revision,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn rotate_tunnel_plan_credentials(
    api_url: &str,
    token: Option<&str>,
    request: TunnelPlanMutationCommand,
) -> Result<()> {
    let (plan_id, expected_revision) =
        validate_tunnel_plan_mutation(request, "credential rotation")?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/tunnel-plans/{plan_id}/credentials/rotate"),
            token,
            &serde_json::json!({
                "confirmed": true,
                "expected_revision": expected_revision,
            }),
        )?
    );
    Ok(())
}

fn validate_tunnel_plan_mutation(
    request: TunnelPlanMutationCommand,
    operation: &str,
) -> Result<(Uuid, i64)> {
    anyhow::ensure!(
        request.confirmed,
        "tunnel plan {operation} requires --confirmed"
    );
    let plan_id = Uuid::parse_str(&request.plan_id).context("invalid --plan-id UUID")?;
    let expected_revision = request
        .expected_revision
        .with_context(|| format!("tunnel plan {operation} requires --expected-revision"))?;
    anyhow::ensure!(
        expected_revision > 0,
        "--expected-revision must be positive"
    );
    Ok((plan_id, expected_revision))
}

pub(crate) fn refresh_tunnel_ospf_status(
    api_url: &str,
    token: Option<&str>,
    request: TunnelOspfStatusRefreshCommand,
) -> Result<()> {
    let plan_id = Uuid::parse_str(&request.plan_id).context("invalid --plan-id UUID")?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/tunnel-plans/{plan_id}/ospf-status"),
            token,
            &serde_json::json!({}),
        )?
    );
    Ok(())
}

pub(crate) fn tunnel_ospf_cost_update(
    api_url: &str,
    token: Option<&str>,
    request: TunnelOspfCostUpdateCommand,
) -> Result<()> {
    anyhow::ensure!(
        request.confirmed,
        "tunnel-ospf-cost-update requires --confirmed"
    );
    anyhow::ensure!(
        !request.recommendation_id.trim().is_empty(),
        "tunnel-ospf-cost-update requires --recommendation-id"
    );
    anyhow::ensure!(
        request.plan_revision > 0,
        "tunnel-ospf-cost-update requires a positive --plan-revision"
    );
    anyhow::ensure!(
        request.left_current_ospf_cost != Some(request.desired_ospf_cost)
            || request.right_current_ospf_cost != Some(request.desired_ospf_cost),
        "tunnel-ospf-cost-update requires at least one endpoint cost change"
    );
    validate_definition_hash(
        &request.left_adapter_definition_hash,
        "--left-adapter-definition-hash",
    )?;
    validate_definition_hash(
        &request.right_adapter_definition_hash,
        "--right-adapter-definition-hash",
    )?;
    let plan_id = Uuid::parse_str(&request.plan_id).context("invalid --plan-id UUID")?;
    let plan_raw = http_get(
        api_url,
        &format!("/api/v1/tunnel-plans/{}/plan", request.plan_id),
        token,
    )?;
    let plan: TunnelPlan =
        serde_json::from_str(&plan_raw).context("failed to parse tunnel plan export")?;
    let target_client_ids = tunnel_plan_client_ids(&plan)?;
    let payload_hash = tunnel_ospf_cost_payload_hash(
        plan_id,
        request.plan_revision,
        &request.recommendation_id,
        request.left_current_ospf_cost,
        request.right_current_ospf_cost,
        request.desired_ospf_cost,
        &request.left_adapter_definition_hash,
        &request.right_adapter_definition_hash,
    );
    let password = load_super_password(&request.password_env)?;
    let salt_hex = load_super_salt_hex(request.super_salt_hex.as_deref())?;
    let target = tunnel_plan_privilege_target(plan_id);
    let privilege_assertion = build_privilege_for_db(
        DbPrivilegeRequest {
            action: tunnel_ospf_cost_action(),
            target: &target,
            selector_expression: None,
            resolved_targets: &target_client_ids,
            confirmed: true,
            payload_hash: Some(&payload_hash),
        },
        &password,
        &salt_hex,
        request.privilege_ttl_secs,
    )?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/tunnel-plans/{}/ospf-cost", request.plan_id),
            token,
            &serde_json::json!({
                "plan_revision": request.plan_revision,
                "recommendation_id": request.recommendation_id,
                "left_current_ospf_cost": request.left_current_ospf_cost,
                "right_current_ospf_cost": request.right_current_ospf_cost,
                "desired_ospf_cost": request.desired_ospf_cost,
                "left_adapter_definition_hash": request.left_adapter_definition_hash,
                "right_adapter_definition_hash": request.right_adapter_definition_hash,
                "confirmed": request.confirmed,
                "privilege_assertion": privilege_assertion,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn tunnel_plan_client_ids(plan: &TunnelPlan) -> Result<Vec<String>> {
    anyhow::ensure!(
        !plan.left_client_id.trim().is_empty() && !plan.right_client_id.trim().is_empty(),
        "tunnel plan export missing endpoint client IDs"
    );
    Ok(vec![
        plan.left_client_id.trim().to_string(),
        plan.right_client_id.trim().to_string(),
    ])
}

pub(crate) fn tunnel_plan_privilege_target(plan_id: Uuid) -> String {
    format!("tunnel_plan:{plan_id}")
}

pub(crate) fn tunnel_ospf_cost_action() -> &'static str {
    "network.ospf_cost.apply"
}

pub(crate) fn tunnel_ospf_cost_payload_hash(
    plan_id: Uuid,
    plan_revision: i64,
    recommendation_id: &str,
    left_current_ospf_cost: Option<u16>,
    right_current_ospf_cost: Option<u16>,
    desired_ospf_cost: u16,
    left_adapter_definition_hash: &str,
    right_adapter_definition_hash: &str,
) -> String {
    payload_hash(
        routing_cost_update_privilege_payload(
            plan_id,
            plan_revision,
            recommendation_id,
            left_current_ospf_cost,
            right_current_ospf_cost,
            desired_ospf_cost,
            left_adapter_definition_hash,
            right_adapter_definition_hash,
        )
        .as_bytes(),
    )
}

fn validate_definition_hash(value: &str, flag: &str) -> Result<()> {
    anyhow::ensure!(
        value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "{flag} must be a 64-character hexadecimal SHA-256 hash"
    );
    Ok(())
}

pub(crate) fn network_traffic_import_vnstat(
    api_url: &str,
    token: Option<&str>,
    mut request: NetworkTrafficImportVnstatCommand,
) -> Result<()> {
    anyhow::ensure!(
        request.confirmed,
        "network-traffic-import-vnstat requires --confirmed because it rewrites historical traffic samples"
    );
    normalize_unique_nonempty(&mut request.clients, "--clients")?;
    normalize_unique_nonempty(&mut request.interfaces, "--interface")?;
    anyhow::ensure!(
        request.interfaces.len() <= NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES,
        "--interface accepts at most {NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES} values"
    );
    anyhow::ensure!(
        request
            .interfaces
            .iter()
            .all(|interface| valid_import_interface_name(interface)),
        "--interface values must be 1-64 characters containing only letters, digits, '_', '-', '.', or ':'"
    );
    let start_unix = parse_network_traffic_import_start(&request.start)?;
    let now_unix = Utc::now().timestamp();
    let now_minute = u64::try_from(now_unix.max(0)).unwrap_or_default() / 60 * 60;
    anyhow::ensure!(
        start_unix < now_minute,
        "--start must be before the current UTC minute"
    );
    let operation = JobCommand::NetworkTrafficImportVnstat {
        interfaces: request.interfaces,
        start_unix,
    };
    let password = load_super_password(&request.password_env)?;
    let salt_hex = load_super_salt_hex(request.super_salt_hex.as_deref())?;
    println!(
        "{}",
        submit_network_job(
            api_url,
            token,
            "network_traffic_import_vnstat",
            request.clients,
            operation,
            Some((&password, &salt_hex, request.privilege_ttl_secs)),
            request.max_timeout_secs,
            true,
            true,
            false,
        )?
    );
    Ok(())
}

fn parse_network_traffic_import_start(value: &str) -> Result<u64> {
    let value = value.trim();
    anyhow::ensure!(!value.is_empty(), "--start is required");
    let timestamp = if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        Utc.from_utc_datetime(
            &date
                .and_hms_opt(0, 0, 0)
                .context("--start date is outside the supported range")?,
        )
    } else {
        DateTime::parse_from_rfc3339(value)
            .context("--start must be YYYY-MM-DD or RFC3339")?
            .with_timezone(&Utc)
    };
    anyhow::ensure!(
        timestamp.timestamp_subsec_nanos() == 0 && timestamp.timestamp() % 60 == 0,
        "--start must be aligned to a UTC minute"
    );
    u64::try_from(timestamp.timestamp())
        .context("--start must be at or after 1970-01-01T00:01:00Z")
        .and_then(|unix| {
            anyhow::ensure!(
                unix >= 60,
                "--start must be at or after 1970-01-01T00:01:00Z"
            );
            Ok(unix)
        })
}

fn normalize_unique_nonempty(values: &mut Vec<String>, flag: &str) -> Result<()> {
    for value in values.iter_mut() {
        *value = value.trim().to_string();
    }
    anyhow::ensure!(
        values.iter().all(|value| !value.is_empty()),
        "{flag} contains an empty value"
    );
    values.sort();
    values.dedup();
    anyhow::ensure!(!values.is_empty(), "{flag} requires at least one value");
    Ok(())
}

fn valid_import_interface_name(interface: &str) -> bool {
    !interface.is_empty()
        && interface.len() <= 64
        && interface
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
}

pub(crate) fn tunnel_status(
    api_url: &str,
    token: Option<&str>,
    request: TunnelStatusCommand,
) -> Result<()> {
    let plan = fetch_tunnel_plan(api_url, token, request.plan_id)?;
    let side = request.side.into();
    let endpoint = render_tunnel_endpoint_config(&plan, side)?;
    let operation = JobCommand::NetworkStatus {
        plan_id: request.plan_id.to_string(),
        plan: Box::new(plan),
        side,
        runtime_adapter: None,
    };
    println!(
        "{}",
        submit_network_job(
            api_url,
            token,
            "network_status",
            vec![endpoint.local_client_id],
            operation,
            None,
            request.max_timeout_secs,
            false,
            false,
            true,
        )?
    );
    Ok(())
}

pub(crate) fn tunnel_probe(
    api_url: &str,
    token: Option<&str>,
    request: TunnelProbeCommand,
) -> Result<()> {
    anyhow::ensure!(
        (1..=20).contains(&request.count),
        "tunnel-probe --count must be between 1 and 20"
    );
    anyhow::ensure!(
        (200..=10_000).contains(&request.interval_ms),
        "tunnel-probe --interval-ms must be between 200 and 10000"
    );
    let plan = fetch_tunnel_plan(api_url, token, request.plan_id)?;
    let side = request.side.into();
    let endpoint = render_tunnel_endpoint_config(&plan, side)?;
    let operation = JobCommand::NetworkProbe {
        plan_id: request.plan_id.to_string(),
        plan: Box::new(plan),
        side,
        count: request.count,
        interval_ms: request.interval_ms,
    };
    let password = load_super_password(&request.password_env)?;
    let salt_hex = load_super_salt_hex(request.super_salt_hex.as_deref())?;
    println!(
        "{}",
        submit_network_job(
            api_url,
            token,
            "network_probe",
            vec![endpoint.local_client_id],
            operation,
            Some((&password, &salt_hex, request.privilege_ttl_secs)),
            request.max_timeout_secs,
            false,
            false,
            false,
        )?
    );
    Ok(())
}

pub(crate) fn tunnel_speed_test(
    api_url: &str,
    token: Option<&str>,
    request: TunnelSpeedTestCommand,
) -> Result<()> {
    anyhow::ensure!(
        request.confirmed,
        "tunnel-speed-test requires --confirmed because it opens a listener and sends traffic"
    );
    validate_speed_test_bounds(
        request.duration_secs,
        request.max_bytes,
        request.rate_limit_kbps,
        request.port,
        request.connect_timeout_ms,
    )?;
    let plan = fetch_tunnel_plan(api_url, token, request.plan_id)?;
    let server_side = request.server_side.into();
    let server_endpoint = render_tunnel_endpoint_config(&plan, server_side)?;
    let target_clients = vec![
        server_endpoint.local_client_id.clone(),
        server_endpoint.peer_client_id.clone(),
    ];
    let operation = JobCommand::NetworkSpeedTest {
        plan_id: request.plan_id.to_string(),
        plan: Box::new(plan),
        server_side,
        duration_secs: request.duration_secs,
        max_bytes: request.max_bytes,
        rate_limit_kbps: request.rate_limit_kbps,
        port: request.port,
        connect_timeout_ms: request.connect_timeout_ms,
    };
    let password = load_super_password(&request.password_env)?;
    let salt_hex = load_super_salt_hex(request.super_salt_hex.as_deref())?;
    println!(
        "{}",
        submit_network_job(
            api_url,
            token,
            "network_speed_test",
            target_clients,
            operation,
            Some((&password, &salt_hex, request.privilege_ttl_secs)),
            request.max_timeout_secs,
            false,
            request.confirmed,
            false,
        )?
    );
    Ok(())
}

fn submit_network_job(
    api_url: &str,
    token: Option<&str>,
    command_label: &str,
    target_clients: Vec<String>,
    operation: JobCommand,
    privilege_material: Option<(&str, &str, u64)>,
    max_timeout_secs: u64,
    destructive: bool,
    confirmed: bool,
    force_unprivileged: bool,
) -> Result<String> {
    let selector_expression = selector_expression_from_targets(&target_clients, &[]);
    let privilege_assertion = privilege_material
        .map(|(password, salt_hex, ttl_secs)| {
            build_privilege_for_job_command(
                &target_clients,
                &operation,
                command_label,
                &selector_expression,
                password,
                salt_hex,
                ttl_secs,
                max_timeout_secs,
                force_unprivileged,
                true,
            )
            .map(|privilege| privilege.privilege_assertion)
        })
        .transpose()?;
    let privileged = privilege_material.is_some();
    http_post_json(
        api_url,
        "/api/v1/jobs",
        token,
        &serde_json::json!({
            "job_id": Uuid::new_v4(),
            "command": command_label,
            "argv": [],
            "selector_expression": selector_expression,
            "target_client_ids": target_clients,
            "privileged": privileged,
            "destructive": destructive,
            "confirmed": confirmed,
            "force_unprivileged": force_unprivileged,
            "max_timeout_secs": max_timeout_secs,
            "operation": operation,
            "privilege_assertion": privilege_assertion,
        }),
    )
}

fn validate_speed_test_bounds(
    duration_secs: u8,
    max_bytes: u64,
    rate_limit_kbps: u32,
    port: u16,
    connect_timeout_ms: u16,
) -> Result<()> {
    anyhow::ensure!(
        (NETWORK_SPEED_TEST_MIN_DURATION_SECS..=NETWORK_SPEED_TEST_MAX_DURATION_SECS)
            .contains(&duration_secs),
        "tunnel-speed-test --duration-secs must be between {} and {}",
        NETWORK_SPEED_TEST_MIN_DURATION_SECS,
        NETWORK_SPEED_TEST_MAX_DURATION_SECS
    );
    anyhow::ensure!(
        max_bytes == NETWORK_SPEED_TEST_UNLIMITED_MAX_BYTES
            || (NETWORK_SPEED_TEST_MIN_MAX_BYTES..=NETWORK_SPEED_TEST_MAX_MAX_BYTES)
                .contains(&max_bytes),
        "tunnel-speed-test --max-bytes must be 0 (unlimited) or between {} and {}",
        NETWORK_SPEED_TEST_MIN_MAX_BYTES,
        NETWORK_SPEED_TEST_MAX_MAX_BYTES
    );
    anyhow::ensure!(
        rate_limit_kbps == NETWORK_SPEED_TEST_UNLIMITED_RATE_LIMIT_KBPS
            || (NETWORK_SPEED_TEST_MIN_RATE_LIMIT_KBPS..=NETWORK_SPEED_TEST_MAX_RATE_LIMIT_KBPS)
                .contains(&rate_limit_kbps),
        "tunnel-speed-test --rate-limit-kbps must be 0 (unlimited) or between {} and {}",
        NETWORK_SPEED_TEST_MIN_RATE_LIMIT_KBPS,
        NETWORK_SPEED_TEST_MAX_RATE_LIMIT_KBPS
    );
    anyhow::ensure!(
        (NETWORK_SPEED_TEST_MIN_PORT..=NETWORK_SPEED_TEST_MAX_PORT).contains(&port),
        "tunnel-speed-test --port must be between {} and {}",
        NETWORK_SPEED_TEST_MIN_PORT,
        NETWORK_SPEED_TEST_MAX_PORT
    );
    anyhow::ensure!(
        (NETWORK_SPEED_TEST_MIN_CONNECT_TIMEOUT_MS..=NETWORK_SPEED_TEST_MAX_CONNECT_TIMEOUT_MS)
            .contains(&connect_timeout_ms),
        "tunnel-speed-test --connect-timeout-ms must be between {} and {}",
        NETWORK_SPEED_TEST_MIN_CONNECT_TIMEOUT_MS,
        NETWORK_SPEED_TEST_MAX_CONNECT_TIMEOUT_MS
    );
    Ok(())
}

pub(crate) fn fetch_tunnel_plan(
    api_url: &str,
    token: Option<&str>,
    plan_id: Uuid,
) -> Result<TunnelPlan> {
    let plan_text = http_get(
        api_url,
        &format!("/api/v1/tunnel-plans/{plan_id}/plan"),
        token,
    )?;
    serde_json::from_str(&plan_text).context("tunnel plan response is invalid")
}

pub(crate) fn tunnel_plan(
    api_url: &str,
    token: Option<&str>,
    request: TunnelPlanCommand,
) -> Result<()> {
    let kind: TunnelKind = request.kind.into();
    let runtime_manager: RuntimeTunnelManager = request.runtime_manager.into();
    let default_mtu = (runtime_manager == RuntimeTunnelManager::AgentBuiltin)
        .then(|| default_tunnel_mtu(kind))
        .flatten();
    let ospf = if request.ospf {
        Some(TunnelOspfConfig {
            mode: request.ospf_mode.into(),
            planned_latency_ms: request
                .ospf_latency_ms
                .context("tunnel-plan --ospf requires --ospf-latency-ms")?,
            planned_packet_loss_ratio: request.ospf_packet_loss_ratio.unwrap_or(0.0),
            preference: request.ospf_preference.unwrap_or(1.0),
            policy: OspfCostPolicy {
                latency_weight: request.ospf_latency_weight,
                loss_weight: request.ospf_loss_weight,
                bandwidth_weight: request.ospf_bandwidth_weight,
                preference_bias: request.ospf_preference_bias,
                min_cost: request.ospf_min_cost,
                max_cost: request.ospf_max_cost,
            },
            min_cost_delta: request.ospf_min_cost_delta,
            healthy_windows: request.ospf_healthy_windows,
            left_adapter_definition_id: request.left_routing_adapter_definition_id,
            right_adapter_definition_id: request.right_routing_adapter_definition_id,
        })
    } else {
        None
    };
    let input = TunnelPlanInput {
        name: request.name,
        interface_name: request.interface_name,
        kind,
        runtime_control: build_runtime_control(RuntimeControlArgs {
            manager: runtime_manager,
            left_adapter_definition_id: request.left_runtime_adapter_definition_id.as_deref(),
            right_adapter_definition_id: request.right_runtime_adapter_definition_id.as_deref(),
            traffic_ingress_kbps: request.traffic_ingress_kbps,
            traffic_egress_kbps: request.traffic_egress_kbps,
            traffic_burst_kb: request.traffic_burst_kb,
            fou_port: request.fou_port,
            fou_peer_port: request.fou_peer_port,
            fou_ipproto: request.fou_ipproto,
            wireguard_left_listen_port: request.wireguard_left_listen_port,
            wireguard_right_listen_port: request.wireguard_right_listen_port,
            wireguard_left_keepalive_secs: request.wireguard_left_keepalive_secs,
            wireguard_right_keepalive_secs: request.wireguard_right_keepalive_secs,
            wireguard_endpoint_mode: request.wireguard_endpoint_mode.map(Into::into),
            openvpn_transport: request.openvpn_transport.map(Into::into),
            openvpn_listener_side: request.openvpn_listener_side.map(Into::into),
            openvpn_port: request.openvpn_port,
        }),
        runtime_topology: build_runtime_topology(RuntimeTopologyArgs {
            version: None,
            desired_interfaces: &request.topology_desired_interfaces,
            stale_interfaces: &request.topology_stale_interfaces,
            routes: &request.topology_route,
            stale_routes: &request.topology_stale_route,
        })?,
        left_client_id: request.left_client_id,
        right_client_id: request.right_client_id,
        left_remote_underlay: request.left_remote_underlay,
        left_local_underlay: request.left_local_underlay,
        right_remote_underlay: request.right_remote_underlay,
        right_local_underlay: request.right_local_underlay,
        address_pool_cidr: request.address_pool_cidr,
        reserved_addresses: request.reserved_addresses,
        ipv4_tunnel: build_address_pair_from_cidrs(
            request.left_tunnel_ipv4_cidr,
            request.right_tunnel_ipv4_cidr,
            TunnelAddressFamily::Ipv4,
            "IPv4",
        )?,
        ipv6_address_pool_cidr: request.ipv6_address_pool_cidr,
        ipv6_tunnel: build_address_pair_from_cidrs(
            request.left_tunnel_ipv6_cidr,
            request.right_tunnel_ipv6_cidr,
            TunnelAddressFamily::Ipv6,
            "IPv6",
        )?,
        latency_primary_family: request.latency_primary_family.into(),
        bandwidth_mbps: request.bandwidth_mbps,
        left_mtu: request.left_mtu.or(default_mtu),
        right_mtu: request.right_mtu.or(default_mtu),
        ospf,
    };
    ensure_explicit_tunnel_endpoints(&input.ipv4_tunnel, &input.ipv6_tunnel, "tunnel-plan")?;
    let plan = plan_tunnel(&input)?;
    if request.save {
        anyhow::ensure!(request.confirmed, "tunnel-plan --save requires --confirmed");
        anyhow::ensure!(
            request.update_plan_id.is_some() == request.expected_revision.is_some(),
            "tunnel-plan update requires both --update-plan-id and --expected-revision"
        );
        anyhow::ensure!(
            request
                .expected_revision
                .is_none_or(|revision| revision > 0),
            "tunnel-plan --expected-revision must be positive"
        );
        let mut body = serde_json::to_value(&input)?;
        if let Some(object) = body.as_object_mut() {
            object.insert("confirmed".to_string(), serde_json::Value::Bool(true));
            if request.update_plan_id.is_none() || request.enabled {
                object.insert(
                    "enabled".to_string(),
                    serde_json::Value::Bool(request.enabled),
                );
            }
            if let Some(expected_revision) = request.expected_revision {
                object.insert(
                    "expected_revision".to_string(),
                    serde_json::Value::Number(expected_revision.into()),
                );
            }
        }
        let response = if let Some(plan_id) = request.update_plan_id {
            http_put_json(
                api_url,
                &format!("/api/v1/tunnel-plans/{plan_id}"),
                token,
                &body,
            )?
        } else {
            http_post_json(api_url, "/api/v1/tunnel-plans", token, &body)?
        };
        println!("{response}");
    } else {
        println!("{}", serde_json::to_string_pretty(&plan)?);
    }
    Ok(())
}

fn build_address_pair_from_cidrs(
    left: Option<String>,
    right: Option<String>,
    family: TunnelAddressFamily,
    label: &str,
) -> Result<Option<TunnelAddressPair>> {
    match (left, right) {
        (Some(left), Some(right)) => {
            let (left, left_prefix) = parse_endpoint_cidr(&left, family, label)?;
            let (right, right_prefix) = parse_endpoint_cidr(&right, family, label)?;
            anyhow::ensure!(
                left_prefix == right_prefix,
                "{label} tunnel endpoint CIDRs must use the same prefix length"
            );
            Ok(Some(TunnelAddressPair {
                left,
                right,
                prefix_len: left_prefix,
            }))
        }
        (None, None) => Ok(None),
        _ => anyhow::bail!("{label} tunnel endpoints require both left and right CIDRs"),
    }
}

fn parse_endpoint_cidr(
    value: &str,
    family: TunnelAddressFamily,
    label: &str,
) -> Result<(String, u8)> {
    let (address, prefix) = value
        .split_once('/')
        .with_context(|| format!("{label} tunnel endpoint must be address/prefix CIDR"))?;
    let ip: IpAddr = address
        .parse()
        .with_context(|| format!("{label} tunnel endpoint address {address} is invalid"))?;
    match (family, ip) {
        (TunnelAddressFamily::Ipv4, IpAddr::V4(_)) => {}
        (TunnelAddressFamily::Ipv6, IpAddr::V6(_)) => {}
        (TunnelAddressFamily::Ipv4, IpAddr::V6(_)) => {
            anyhow::bail!("{label} tunnel endpoint must be IPv4")
        }
        (TunnelAddressFamily::Ipv6, IpAddr::V4(_)) => {
            anyhow::bail!("{label} tunnel endpoint must be IPv6")
        }
    }
    let prefix_len = prefix
        .parse::<u8>()
        .with_context(|| format!("{label} tunnel endpoint prefix {prefix} is invalid"))?;
    let max_prefix = match family {
        TunnelAddressFamily::Ipv4 => 32,
        TunnelAddressFamily::Ipv6 => 128,
    };
    anyhow::ensure!(
        prefix_len <= max_prefix,
        "{label} tunnel endpoint prefix must be <= {max_prefix}"
    );
    Ok((address.to_string(), prefix_len))
}

fn ensure_explicit_tunnel_endpoints(
    ipv4_tunnel: &Option<TunnelAddressPair>,
    ipv6_tunnel: &Option<TunnelAddressPair>,
    command: &str,
) -> Result<()> {
    anyhow::ensure!(
        ipv4_tunnel.is_some() || ipv6_tunnel.is_some(),
        "{command} requires explicit IPv4 or IPv6 tunnel endpoint CIDRs; run tunnel-allocate for non-overlapping suggestions, then pass --left-tunnel-ipv4-cidr/--right-tunnel-ipv4-cidr or --left-tunnel-ipv6-cidr/--right-tunnel-ipv6-cidr"
    );
    Ok(())
}

#[cfg(test)]
#[path = "tests_commands_network_traffic_import.rs"]
mod tests_network_traffic_import;
