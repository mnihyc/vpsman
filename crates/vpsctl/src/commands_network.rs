use std::{net::IpAddr, path::PathBuf};

use anyhow::{Context, Result};
use clap::{ArgAction, Args, ValueEnum};
use uuid::Uuid;
use vpsman_common::{
    payload_hash, plan_tunnel, render_tunnel_endpoint_config, BandwidthTier, JobCommand,
    OspfCostPolicy, TunnelAddressFamily, TunnelAddressPair, TunnelEndpointSide, TunnelKind,
    TunnelPlan, TunnelPlanInput, DEFAULT_MAX_JOB_TIMEOUT_SECS,
    NETWORK_SPEED_TEST_MAX_CONNECT_TIMEOUT_MS, NETWORK_SPEED_TEST_MAX_DURATION_SECS,
    NETWORK_SPEED_TEST_MAX_MAX_BYTES, NETWORK_SPEED_TEST_MAX_PORT,
    NETWORK_SPEED_TEST_MAX_RATE_LIMIT_KBPS, NETWORK_SPEED_TEST_MIN_CONNECT_TIMEOUT_MS,
    NETWORK_SPEED_TEST_MIN_DURATION_SECS, NETWORK_SPEED_TEST_MIN_MAX_BYTES,
    NETWORK_SPEED_TEST_MIN_PORT, NETWORK_SPEED_TEST_MIN_RATE_LIMIT_KBPS,
};

use crate::{
    commands_schedules::selector_expression_from_targets,
    http::{http_get, http_post_json},
    network_runtime_args::{
        build_runtime_control, build_runtime_topology, RuntimeControlArgs, RuntimeManagerArg,
        RuntimeTopologyArgs,
    },
    privilege::{build_privilege_for_job_command, load_super_password, load_super_salt_hex},
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
    #[arg(long)]
    pub(crate) left_underlay: String,
    #[arg(long)]
    pub(crate) right_underlay: String,
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
    #[arg(long, value_enum)]
    pub(crate) bandwidth: BandwidthTierArg,
    #[arg(long)]
    pub(crate) latency_ms: f64,
    #[arg(long, default_value_t = 0.0)]
    pub(crate) packet_loss_ratio: f64,
    #[arg(long, default_value_t = 1.0)]
    pub(crate) preference: f64,
    #[arg(long, value_enum, default_value = "agent_iproute2_managed")]
    pub(crate) runtime_manager: RuntimeManagerArg,
    #[arg(long, value_delimiter = ',')]
    pub(crate) runtime_startup_argv: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) runtime_stop_argv: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) runtime_cleanup_argv: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) runtime_restart_argv: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) runtime_status_argv: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) runtime_traffic_limit_argv: Vec<String>,
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
    #[arg(long, value_delimiter = ',')]
    pub(crate) topology_desired_interfaces: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) topology_stale_interfaces: Vec<String>,
    #[arg(long)]
    pub(crate) topology_route: Vec<String>,
    #[arg(long)]
    pub(crate) topology_stale_route: Vec<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) save: bool,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct TunnelPromoteExternalObserveCommand {
    #[arg(long)]
    pub(crate) client_id: String,
    #[arg(long)]
    pub(crate) interface: String,
    #[arg(long)]
    pub(crate) peer_client_id: String,
    #[arg(long)]
    pub(crate) local_underlay: String,
    #[arg(long)]
    pub(crate) peer_underlay: String,
    #[arg(
        long,
        default_value = "",
        help = "Allocation context only; external observe still requires explicit tunnel endpoints"
    )]
    pub(crate) address_pool_cidr: String,
    #[arg(long)]
    pub(crate) left_tunnel_ipv4_cidr: Option<String>,
    #[arg(long)]
    pub(crate) right_tunnel_ipv4_cidr: Option<String>,
    #[arg(
        long,
        help = "IPv6 allocation context only; external observe still requires explicit tunnel endpoints"
    )]
    pub(crate) ipv6_address_pool_cidr: Option<String>,
    #[arg(long)]
    pub(crate) left_tunnel_ipv6_cidr: Option<String>,
    #[arg(long)]
    pub(crate) right_tunnel_ipv6_cidr: Option<String>,
    #[arg(long, value_enum, default_value = "ipv4")]
    pub(crate) latency_primary_family: TunnelAddressFamilyArg,
    #[arg(long, value_enum, default_value = "left")]
    pub(crate) side: TunnelApplySideArg,
    #[arg(long)]
    pub(crate) name: Option<String>,
    #[arg(long, value_enum)]
    pub(crate) bandwidth: Option<BandwidthTierArg>,
    #[arg(long)]
    pub(crate) latency_ms: Option<f64>,
    #[arg(long)]
    pub(crate) packet_loss_ratio: Option<f64>,
    #[arg(long)]
    pub(crate) preference: Option<f64>,
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
pub(crate) struct TunnelPromoteCustomAdapterCommand {
    #[arg(long)]
    pub(crate) plan_id: String,
    #[arg(long, value_delimiter = ',')]
    pub(crate) runtime_startup_argv: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) runtime_stop_argv: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) runtime_cleanup_argv: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) runtime_restart_argv: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) runtime_status_argv: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) runtime_traffic_limit_argv: Vec<String>,
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
    #[arg(long, value_delimiter = ',')]
    pub(crate) topology_desired_interfaces: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) topology_stale_interfaces: Vec<String>,
    #[arg(long)]
    pub(crate) topology_route: Vec<String>,
    #[arg(long)]
    pub(crate) topology_stale_route: Vec<String>,
    #[arg(long)]
    pub(crate) name: Option<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
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
pub(crate) enum TunnelApplySideArg {
    Left,
    Right,
}

impl From<TunnelApplySideArg> for vpsman_common::TunnelEndpointSide {
    fn from(value: TunnelApplySideArg) -> Self {
        match value {
            TunnelApplySideArg::Left => Self::Left,
            TunnelApplySideArg::Right => Self::Right,
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
pub(crate) enum BandwidthTierArg {
    #[value(name = "10m")]
    M10,
    #[value(name = "100m")]
    M100,
    #[value(name = "1000m")]
    M1000,
}

impl From<BandwidthTierArg> for BandwidthTier {
    fn from(value: BandwidthTierArg) -> Self {
        match value {
            BandwidthTierArg::M10 => Self::M10,
            BandwidthTierArg::M100 => Self::M100,
            BandwidthTierArg::M1000 => Self::M1000,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct TunnelApplyCommand {
    #[arg(long)]
    pub(crate) plan_file: PathBuf,
    #[arg(long, value_enum)]
    pub(crate) side: TunnelApplySideArg,
    #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
    pub(crate) password_env: String,
    #[arg(long)]
    pub(crate) super_salt_hex: Option<String>,
    #[arg(long, default_value_t = 300)]
    pub(crate) privilege_ttl_secs: u64,
    #[arg(long, default_value_t = 60)]
    pub(crate) max_timeout_secs: u64,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
    #[arg(long, default_value_t = false)]
    pub(crate) force_unprivileged: bool,
}

#[derive(Debug, Args)]
pub(crate) struct TunnelOspfCostUpdateCommand {
    #[arg(long)]
    pub(crate) plan_file: PathBuf,
    #[arg(long, value_enum)]
    pub(crate) side: TunnelApplySideArg,
    #[arg(long)]
    pub(crate) current_ospf_cost: u16,
    #[arg(long)]
    pub(crate) recommended_ospf_cost: u16,
    #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
    pub(crate) password_env: String,
    #[arg(long)]
    pub(crate) super_salt_hex: Option<String>,
    #[arg(long, default_value_t = 300)]
    pub(crate) privilege_ttl_secs: u64,
    #[arg(long, default_value_t = 60)]
    pub(crate) max_timeout_secs: u64,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
    #[arg(long, default_value_t = false)]
    pub(crate) force_unprivileged: bool,
}

#[derive(Debug, Args)]
pub(crate) struct TunnelRollbackCommand {
    #[arg(long)]
    pub(crate) plan_file: PathBuf,
    #[arg(long, value_enum)]
    pub(crate) side: TunnelApplySideArg,
    #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
    pub(crate) password_env: String,
    #[arg(long)]
    pub(crate) super_salt_hex: Option<String>,
    #[arg(long, default_value_t = 300)]
    pub(crate) privilege_ttl_secs: u64,
    #[arg(long, default_value_t = 60)]
    pub(crate) max_timeout_secs: u64,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
    #[arg(long, default_value_t = false)]
    pub(crate) force_unprivileged: bool,
}

#[derive(Debug, Args)]
pub(crate) struct TunnelStatusCommand {
    #[arg(long)]
    pub(crate) plan_file: PathBuf,
    #[arg(long, value_enum)]
    pub(crate) side: TunnelApplySideArg,
    #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
    pub(crate) password_env: String,
    #[arg(long)]
    pub(crate) super_salt_hex: Option<String>,
    #[arg(long, default_value_t = 300)]
    pub(crate) privilege_ttl_secs: u64,
    #[arg(long, default_value_t = 60)]
    pub(crate) max_timeout_secs: u64,
}

#[derive(Debug, Args)]
pub(crate) struct TunnelProbeCommand {
    #[arg(long)]
    pub(crate) plan_file: PathBuf,
    #[arg(long, value_enum)]
    pub(crate) side: TunnelApplySideArg,
    #[arg(long, default_value_t = 3)]
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
pub(crate) struct TunnelSpeedTestCommand {
    #[arg(long)]
    pub(crate) plan_file: PathBuf,
    #[arg(long, value_enum)]
    pub(crate) server_side: TunnelApplySideArg,
    #[arg(long, default_value_t = 3)]
    pub(crate) duration_secs: u8,
    #[arg(long, default_value_t = 16 * 1024 * 1024)]
    pub(crate) max_bytes: u64,
    #[arg(long, default_value_t = 100_000)]
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

pub(crate) fn tunnel_apply(
    api_url: &str,
    token: Option<&str>,
    request: TunnelApplyCommand,
) -> Result<()> {
    anyhow::ensure!(request.confirmed, "tunnel-apply requires --confirmed");
    let plan = read_tunnel_plan(&request.plan_file)?;
    let side = request.side.into();
    let endpoint = render_tunnel_endpoint_config(&plan, side)?;
    let operation = JobCommand::NetworkApply {
        plan: Box::new(plan),
        side,
    };
    let password = load_super_password(&request.password_env)?;
    let salt_hex = load_super_salt_hex(request.super_salt_hex.as_deref())?;
    println!(
        "{}",
        submit_network_job(
            api_url,
            token,
            "network_apply",
            vec![endpoint.local_client_id],
            operation,
            &password,
            &salt_hex,
            request.privilege_ttl_secs,
            request.max_timeout_secs,
            true,
            request.confirmed,
            request.force_unprivileged,
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
        request.current_ospf_cost != request.recommended_ospf_cost,
        "tunnel-ospf-cost-update requires a changed OSPF cost"
    );
    let mut plan = read_tunnel_plan(&request.plan_file)?;
    plan.recommended_ospf_cost = request.recommended_ospf_cost;
    let side = request.side.into();
    let endpoint = render_tunnel_endpoint_config(&plan, side)?;
    let operation = JobCommand::NetworkOspfCostUpdate {
        plan: Box::new(plan),
        side,
        current_ospf_cost: request.current_ospf_cost,
        recommended_ospf_cost: request.recommended_ospf_cost,
        bird2_sha256_hex: payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
    };
    let password = load_super_password(&request.password_env)?;
    let salt_hex = load_super_salt_hex(request.super_salt_hex.as_deref())?;
    println!(
        "{}",
        submit_network_job(
            api_url,
            token,
            "network_ospf_cost_update",
            vec![endpoint.local_client_id],
            operation,
            &password,
            &salt_hex,
            request.privilege_ttl_secs,
            request.max_timeout_secs,
            true,
            request.confirmed,
            request.force_unprivileged,
        )?
    );
    Ok(())
}

pub(crate) fn tunnel_rollback(
    api_url: &str,
    token: Option<&str>,
    request: TunnelRollbackCommand,
) -> Result<()> {
    anyhow::ensure!(request.confirmed, "tunnel-rollback requires --confirmed");
    let plan = read_tunnel_plan(&request.plan_file)?;
    let side = request.side.into();
    let endpoint = render_tunnel_endpoint_config(&plan, side)?;
    let operation = JobCommand::NetworkRollback {
        plan: Box::new(plan),
        side,
    };
    let password = load_super_password(&request.password_env)?;
    let salt_hex = load_super_salt_hex(request.super_salt_hex.as_deref())?;
    println!(
        "{}",
        submit_network_job(
            api_url,
            token,
            "network_rollback",
            vec![endpoint.local_client_id],
            operation,
            &password,
            &salt_hex,
            request.privilege_ttl_secs,
            request.max_timeout_secs,
            true,
            request.confirmed,
            request.force_unprivileged,
        )?
    );
    Ok(())
}

pub(crate) fn tunnel_status(
    api_url: &str,
    token: Option<&str>,
    request: TunnelStatusCommand,
) -> Result<()> {
    let plan = read_tunnel_plan(&request.plan_file)?;
    let side = request.side.into();
    let endpoint = render_tunnel_endpoint_config(&plan, side)?;
    let operation = JobCommand::NetworkStatus {
        plan: Box::new(plan),
        side,
    };
    let password = load_super_password(&request.password_env)?;
    let salt_hex = load_super_salt_hex(request.super_salt_hex.as_deref())?;
    println!(
        "{}",
        submit_network_job(
            api_url,
            token,
            "network_status",
            vec![endpoint.local_client_id],
            operation,
            &password,
            &salt_hex,
            request.privilege_ttl_secs,
            request.max_timeout_secs,
            false,
            false,
            false,
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
    let plan = read_tunnel_plan(&request.plan_file)?;
    let side = request.side.into();
    let endpoint = render_tunnel_endpoint_config(&plan, side)?;
    let operation = JobCommand::NetworkProbe {
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
            &password,
            &salt_hex,
            request.privilege_ttl_secs,
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
    let plan = read_tunnel_plan(&request.plan_file)?;
    let server_side = request.server_side.into();
    let server_endpoint = render_tunnel_endpoint_config(&plan, server_side)?;
    let target_clients = vec![
        server_endpoint.local_client_id.clone(),
        server_endpoint.peer_client_id.clone(),
    ];
    let operation = JobCommand::NetworkSpeedTest {
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
            &password,
            &salt_hex,
            request.privilege_ttl_secs,
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
    password: &str,
    salt_hex: &str,
    ttl_secs: u64,
    max_timeout_secs: u64,
    destructive: bool,
    confirmed: bool,
    force_unprivileged: bool,
) -> Result<String> {
    let selector_expression = selector_expression_from_targets(&target_clients, &[]);
    let privilege = build_privilege_for_job_command(
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
    )?;
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
            "privileged": true,
            "destructive": destructive,
            "confirmed": confirmed,
            "force_unprivileged": force_unprivileged,
            "max_timeout_secs": max_timeout_secs,
            "operation": operation,
            "privilege_assertion": privilege.privilege_assertion,
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
        (NETWORK_SPEED_TEST_MIN_MAX_BYTES..=NETWORK_SPEED_TEST_MAX_MAX_BYTES).contains(&max_bytes),
        "tunnel-speed-test --max-bytes must be between {} and {}",
        NETWORK_SPEED_TEST_MIN_MAX_BYTES,
        NETWORK_SPEED_TEST_MAX_MAX_BYTES
    );
    anyhow::ensure!(
        (NETWORK_SPEED_TEST_MIN_RATE_LIMIT_KBPS..=NETWORK_SPEED_TEST_MAX_RATE_LIMIT_KBPS)
            .contains(&rate_limit_kbps),
        "tunnel-speed-test --rate-limit-kbps must be between {} and {}",
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

fn read_tunnel_plan(plan_file: &PathBuf) -> Result<TunnelPlan> {
    let plan_text = std::fs::read_to_string(plan_file)
        .with_context(|| format!("failed to read tunnel plan {}", plan_file.display()))?;
    serde_json::from_str(&plan_text).context("tunnel plan JSON is invalid")
}

pub(crate) fn tunnel_plan(
    api_url: &str,
    token: Option<&str>,
    request: TunnelPlanCommand,
) -> Result<()> {
    let input = TunnelPlanInput {
        name: request.name,
        interface_name: request.interface_name,
        kind: request.kind.into(),
        runtime_control: build_runtime_control(RuntimeControlArgs {
            manager: request.runtime_manager.into(),
            startup_argv: &request.runtime_startup_argv,
            stop_argv: &request.runtime_stop_argv,
            cleanup_argv: &request.runtime_cleanup_argv,
            restart_argv: &request.runtime_restart_argv,
            status_argv: &request.runtime_status_argv,
            traffic_limit_argv: &request.runtime_traffic_limit_argv,
            traffic_ingress_kbps: request.traffic_ingress_kbps,
            traffic_egress_kbps: request.traffic_egress_kbps,
            traffic_burst_kb: request.traffic_burst_kb,
            fou_port: request.fou_port,
            fou_peer_port: request.fou_peer_port,
            fou_ipproto: request.fou_ipproto,
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
        left_underlay: request.left_underlay,
        right_underlay: request.right_underlay,
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
        bandwidth: request.bandwidth.into(),
        latency_ms: request.latency_ms,
        packet_loss_ratio: request.packet_loss_ratio,
        preference: request.preference,
        ospf_policy: OspfCostPolicy::default(),
    };
    ensure_explicit_tunnel_endpoints(&input.ipv4_tunnel, &input.ipv6_tunnel, "tunnel-plan")?;
    if request.save {
        anyhow::ensure!(request.confirmed, "tunnel-plan --save requires --confirmed");
        let mut body = serde_json::to_value(&input)?;
        if let Some(object) = body.as_object_mut() {
            object.insert("confirmed".to_string(), serde_json::Value::Bool(true));
        }
        println!(
            "{}",
            http_post_json(api_url, "/api/v1/tunnel-plans", token, &body,)?
        );
    } else {
        let plan = plan_tunnel(&input)?;
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

pub(crate) fn tunnel_promote_external_observe(
    api_url: &str,
    token: Option<&str>,
    request: TunnelPromoteExternalObserveCommand,
) -> Result<()> {
    anyhow::ensure!(
        request.confirmed,
        "tunnel-promote-external-observe requires --confirmed"
    );
    let ipv4_tunnel = build_address_pair_from_cidrs(
        request.left_tunnel_ipv4_cidr,
        request.right_tunnel_ipv4_cidr,
        TunnelAddressFamily::Ipv4,
        "IPv4",
    )?;
    let ipv6_tunnel = build_address_pair_from_cidrs(
        request.left_tunnel_ipv6_cidr,
        request.right_tunnel_ipv6_cidr,
        TunnelAddressFamily::Ipv6,
        "IPv6",
    )?;
    ensure_explicit_tunnel_endpoints(
        &ipv4_tunnel,
        &ipv6_tunnel,
        "tunnel-promote-external-observe",
    )?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/tunnel-plans/promote-telemetry",
            token,
            &serde_json::json!({
                "client_id": request.client_id,
                "interface": request.interface,
                "peer_client_id": request.peer_client_id,
                "local_underlay": request.local_underlay,
                "peer_underlay": request.peer_underlay,
                "address_pool_cidr": request.address_pool_cidr,
                "ipv4_tunnel": ipv4_tunnel,
                "ipv6_address_pool_cidr": request.ipv6_address_pool_cidr,
                "ipv6_tunnel": ipv6_tunnel,
                "latency_primary_family": TunnelAddressFamily::from(request.latency_primary_family),
                "side": TunnelEndpointSide::from(request.side),
                "name": request.name,
                "bandwidth": request.bandwidth.map(BandwidthTier::from),
                "latency_ms": request.latency_ms,
                "packet_loss_ratio": request.packet_loss_ratio,
                "preference": request.preference,
                "confirmed": request.confirmed,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn tunnel_promote_custom_adapter(
    api_url: &str,
    token: Option<&str>,
    request: TunnelPromoteCustomAdapterCommand,
) -> Result<()> {
    let runtime_control = build_runtime_control(RuntimeControlArgs {
        manager: vpsman_common::RuntimeTunnelManager::ExternalManagedAdapter,
        startup_argv: &request.runtime_startup_argv,
        stop_argv: &request.runtime_stop_argv,
        cleanup_argv: &request.runtime_cleanup_argv,
        restart_argv: &request.runtime_restart_argv,
        status_argv: &request.runtime_status_argv,
        traffic_limit_argv: &request.runtime_traffic_limit_argv,
        traffic_ingress_kbps: request.traffic_ingress_kbps,
        traffic_egress_kbps: request.traffic_egress_kbps,
        traffic_burst_kb: request.traffic_burst_kb,
        fou_port: request.fou_port,
        fou_peer_port: request.fou_peer_port,
        fou_ipproto: request.fou_ipproto,
    });
    let runtime_topology = build_runtime_topology(RuntimeTopologyArgs {
        version: None,
        desired_interfaces: &request.topology_desired_interfaces,
        stale_interfaces: &request.topology_stale_interfaces,
        routes: &request.topology_route,
        stale_routes: &request.topology_stale_route,
    })?;
    let runtime_topology = if runtime_topology.is_default() {
        None
    } else {
        Some(runtime_topology)
    };
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/tunnel-plans/promote-custom-adapter",
            token,
            &serde_json::json!({
                "plan_id": request.plan_id,
                "runtime_control": runtime_control,
                "runtime_topology": runtime_topology,
                "name": request.name,
                "confirmed": request.confirmed,
            }),
        )?
    );
    Ok(())
}
