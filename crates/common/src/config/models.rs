use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    OspfCostPolicy, RuntimeTunnelCommand, TunnelConfigBackend, TunnelEndpointSide, TunnelPlan,
    DEFAULT_MAX_COMMAND_TIMEOUT_SECS,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServerEndpoint {
    pub label: String,
    pub tcp_addr: String,
    pub priority: u16,
}

pub const MAX_AGENT_HOT_CONFIG_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    pub client_id: String,
    pub display_name: String,
    pub tcp_endpoints: Vec<ServerEndpoint>,
    #[serde(default)]
    pub noise: AgentNoiseConfig,
    #[serde(default)]
    pub auth: AgentAuthConfig,
    #[serde(default)]
    pub backup: AgentBackupConfig,
    #[serde(default)]
    pub update: AgentUpdateConfig,
    #[serde(default)]
    pub execution: AgentExecutionConfig,
    #[serde(default)]
    pub telemetry: AgentTelemetryConfig,
    #[serde(default)]
    pub network: AgentNetworkConfig,
    pub telemetry_light_secs: u64,
    pub telemetry_full_secs: u64,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentNoiseConfig {
    pub mode: AgentNoiseMode,
    pub client_private_key_hex: Option<String>,
    pub server_public_key_hex: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentAuthConfig {
    pub command_timeout_secs: u64,
    #[serde(default = "default_agent_gateway_retry_secs")]
    pub gateway_retry_secs: u64,
    #[serde(default = "default_agent_gateway_connect_timeout_secs")]
    pub gateway_connect_timeout_secs: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentBackupConfig {
    #[serde(
        default = "default_agent_backup_max_uncompressed_bytes",
        alias = "max_plaintext_bytes"
    )]
    pub max_uncompressed_bytes: u64,
    #[serde(default = "default_agent_backup_max_archive_bytes")]
    pub max_archive_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentUpdateConfig {
    #[serde(default = "default_agent_unmanaged_update_enabled")]
    pub unmanaged_enabled: bool,
    #[serde(default = "default_agent_unmanaged_update_version_url")]
    pub unmanaged_version_url: String,
    #[serde(default = "default_agent_unmanaged_update_interval_secs")]
    pub unmanaged_interval_secs: u64,
    #[serde(default = "default_agent_unmanaged_update_jitter_secs")]
    pub unmanaged_jitter_secs: u64,
    #[serde(default = "default_agent_unmanaged_update_activate")]
    pub unmanaged_activate: bool,
    #[serde(default = "default_agent_unmanaged_update_restart_agent")]
    pub unmanaged_restart_agent: bool,
}

impl Default for AgentUpdateConfig {
    fn default() -> Self {
        Self {
            unmanaged_enabled: default_agent_unmanaged_update_enabled(),
            unmanaged_version_url: default_agent_unmanaged_update_version_url(),
            unmanaged_interval_secs: default_agent_unmanaged_update_interval_secs(),
            unmanaged_jitter_secs: default_agent_unmanaged_update_jitter_secs(),
            unmanaged_activate: default_agent_unmanaged_update_activate(),
            unmanaged_restart_agent: default_agent_unmanaged_update_restart_agent(),
        }
    }
}

pub fn default_agent_unmanaged_update_enabled() -> bool {
    false
}

pub fn default_agent_unmanaged_update_version_url() -> String {
    "https://github.com/mnihyc/vpsman/releases/latest/download/version.json".to_string()
}

pub fn default_agent_unmanaged_update_interval_secs() -> u64 {
    24 * 60 * 60
}

pub fn default_agent_unmanaged_update_jitter_secs() -> u64 {
    24 * 60 * 60
}

pub fn default_agent_unmanaged_update_activate() -> bool {
    true
}

pub fn default_agent_unmanaged_update_restart_agent() -> bool {
    true
}

pub fn default_agent_gateway_retry_secs() -> u64 {
    60
}

pub fn default_agent_gateway_connect_timeout_secs() -> u64 {
    10
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentExecutionConfig {
    #[serde(default = "default_execution_shell_script_argv")]
    pub shell_script_argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    #[serde(default)]
    pub environment_policy: AgentExecutionEnvironmentPolicy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment_keep: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub environment_set: BTreeMap<String, String>,
    #[serde(default)]
    pub pty_policy: AgentExecutionPtyPolicy,
    #[serde(default)]
    pub process_cleanup: AgentExecutionProcessCleanupPolicy,
    #[serde(default)]
    pub user_sessions_source: AgentUserSessionsSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_sessions_command: Option<RuntimeTunnelCommand>,
    #[serde(default)]
    pub process_inventory_source: AgentProcessInventorySource,
    #[serde(default = "default_execution_process_proc_root")]
    pub process_proc_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_inventory_command: Option<RuntimeTunnelCommand>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentExecutionEnvironmentPolicy {
    #[default]
    Inherit,
    Clean,
    MinimalPath,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentExecutionPtyPolicy {
    #[default]
    NativePty,
    Disabled,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentExecutionProcessCleanupPolicy {
    #[default]
    ProcessGroup,
    DirectChild,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentUserSessionsSource {
    #[default]
    LinuxWWhoPreset,
    CustomCommand,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentProcessInventorySource {
    #[default]
    LinuxProcfs,
    CustomCommand,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentTelemetryConfig {
    #[serde(default)]
    pub source: AgentTelemetrySource,
    #[serde(default = "default_telemetry_proc_root")]
    pub proc_root: String,
    #[serde(default = "default_telemetry_sys_class_net_dir")]
    pub sys_class_net_dir: String,
    #[serde(default = "default_telemetry_hostname_file")]
    pub hostname_file: Option<String>,
    #[serde(default = "default_telemetry_os_release_file")]
    pub os_release_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_metrics_command: Option<RuntimeTunnelCommand>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTelemetrySource {
    #[default]
    LinuxProcfs,
    CustomCommand,
    LinuxProcfsAndCustomCommand,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AgentNetworkConfig {
    #[serde(default)]
    pub apply_enabled: bool,
    #[serde(default)]
    pub runtime_reconcile_enabled: bool,
    #[serde(default)]
    pub allow_routing_without_runtime_ready: bool,
    #[serde(default)]
    pub runtime_unprivileged_mutation_policy: AgentRuntimeUnprivilegedMutationPolicy,
    #[serde(default)]
    pub backend: TunnelConfigBackend,
    #[serde(default)]
    pub preset: Option<AgentNetworkPreset>,
    #[serde(default = "default_network_root_dir")]
    pub root_dir: String,
    #[serde(default)]
    pub validate_enabled: bool,
    #[serde(default)]
    pub reload_enabled: bool,
    #[serde(default = "default_network_hook_timeout_secs")]
    pub hook_timeout_secs: u64,
    #[serde(default = "default_network_runtime_ip_argv")]
    pub runtime_ip_argv: Vec<String>,
    #[serde(default = "default_network_runtime_tc_argv")]
    pub runtime_tc_argv: Vec<String>,
    #[serde(default = "default_network_runtime_command_timeout_secs")]
    pub runtime_command_timeout_secs: u64,
    #[serde(default = "default_network_runtime_command_max_output_bytes")]
    pub runtime_command_max_output_bytes: u32,
    #[serde(default)]
    pub ifupdown_validate_argv: Vec<String>,
    #[serde(default)]
    pub bird2_validate_argv: Vec<String>,
    #[serde(default)]
    pub reload_argv: Vec<Vec<String>>,
    #[serde(default)]
    pub bird2_reload_argv: Vec<Vec<String>>,
    #[serde(default)]
    pub bird2_status_argv: Vec<String>,
    #[serde(default)]
    pub probe_ping_argv: Vec<String>,
    #[serde(default = "default_network_status_probe_timeout_secs")]
    pub status_probe_timeout_secs: u64,
    #[serde(default = "default_network_status_probe_max_output_bytes")]
    pub status_probe_max_output_bytes: u32,
    #[serde(default = "default_true")]
    pub runtime_status_telemetry_enabled: bool,
    #[serde(default = "default_network_runtime_status_telemetry_interval_secs")]
    pub runtime_status_telemetry_interval_secs: u64,
    #[serde(default = "default_network_runtime_vnstat_argv")]
    pub runtime_vnstat_argv: Vec<String>,
    #[serde(default = "default_true")]
    pub latency_monitoring_enabled: bool,
    #[serde(default = "default_network_latency_monitoring_interval_secs")]
    pub latency_monitoring_interval_secs: u64,
    #[serde(default = "default_network_latency_down_windows")]
    pub latency_down_windows: u8,
    #[serde(default)]
    pub auto_ospf_enabled: bool,
    #[serde(default = "default_network_auto_ospf_min_cost_delta")]
    pub auto_ospf_min_cost_delta: u16,
    #[serde(default = "default_network_auto_ospf_healthy_windows")]
    pub auto_ospf_healthy_windows: u8,
    #[serde(default)]
    pub auto_ospf_policy: OspfCostPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_ospf_updater: Option<RuntimeTunnelCommand>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runtime_status_telemetry_plans: Vec<AgentRuntimeStatusTelemetryPlan>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentRuntimeStatusTelemetryPlan {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<String>,
    pub endpoint_side: TunnelEndpointSide,
    pub plan: TunnelPlan,
    #[serde(default)]
    pub traffic_source: AgentRuntimeTrafficSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traffic_command: Option<RuntimeTunnelCommand>,
    #[serde(default = "default_true")]
    pub latency_monitoring_enabled: bool,
    #[serde(default)]
    pub auto_ospf_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_ospf_updater: Option<RuntimeTunnelCommand>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRuntimeTrafficSource {
    #[default]
    InterfaceCounters,
    Vnstat,
    CustomCommand,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRuntimeUnprivilegedMutationPolicy {
    #[default]
    Skip,
    TryExternalAdapters,
    TryAll,
}

impl Default for AgentBackupConfig {
    fn default() -> Self {
        Self {
            max_uncompressed_bytes: default_agent_backup_max_uncompressed_bytes(),
            max_archive_bytes: default_agent_backup_max_archive_bytes(),
        }
    }
}

pub fn default_agent_backup_max_uncompressed_bytes() -> u64 {
    1024 * 1024
}

pub fn default_agent_backup_max_archive_bytes() -> u64 {
    2 * 1024 * 1024
}

impl Default for AgentTelemetryConfig {
    fn default() -> Self {
        Self {
            source: AgentTelemetrySource::LinuxProcfs,
            proc_root: default_telemetry_proc_root(),
            sys_class_net_dir: default_telemetry_sys_class_net_dir(),
            hostname_file: default_telemetry_hostname_file(),
            os_release_file: default_telemetry_os_release_file(),
            custom_metrics_command: None,
        }
    }
}

impl Default for AgentExecutionConfig {
    fn default() -> Self {
        Self {
            shell_script_argv: default_execution_shell_script_argv(),
            working_directory: None,
            environment_policy: AgentExecutionEnvironmentPolicy::Inherit,
            environment_keep: Vec::new(),
            environment_set: BTreeMap::new(),
            pty_policy: AgentExecutionPtyPolicy::NativePty,
            process_cleanup: AgentExecutionProcessCleanupPolicy::ProcessGroup,
            user_sessions_source: AgentUserSessionsSource::LinuxWWhoPreset,
            user_sessions_command: None,
            process_inventory_source: AgentProcessInventorySource::LinuxProcfs,
            process_proc_root: default_execution_process_proc_root(),
            process_inventory_command: None,
        }
    }
}

impl Default for AgentNetworkConfig {
    fn default() -> Self {
        Self {
            apply_enabled: false,
            runtime_reconcile_enabled: false,
            allow_routing_without_runtime_ready: false,
            runtime_unprivileged_mutation_policy: AgentRuntimeUnprivilegedMutationPolicy::default(),
            backend: TunnelConfigBackend::Ifupdown,
            preset: None,
            root_dir: default_network_root_dir(),
            validate_enabled: false,
            reload_enabled: false,
            hook_timeout_secs: default_network_hook_timeout_secs(),
            runtime_ip_argv: default_network_runtime_ip_argv(),
            runtime_tc_argv: default_network_runtime_tc_argv(),
            runtime_command_timeout_secs: default_network_runtime_command_timeout_secs(),
            runtime_command_max_output_bytes: default_network_runtime_command_max_output_bytes(),
            ifupdown_validate_argv: Vec::new(),
            bird2_validate_argv: Vec::new(),
            reload_argv: Vec::new(),
            bird2_reload_argv: Vec::new(),
            bird2_status_argv: Vec::new(),
            probe_ping_argv: Vec::new(),
            status_probe_timeout_secs: default_network_status_probe_timeout_secs(),
            status_probe_max_output_bytes: default_network_status_probe_max_output_bytes(),
            runtime_status_telemetry_enabled: true,
            runtime_status_telemetry_interval_secs:
                default_network_runtime_status_telemetry_interval_secs(),
            runtime_vnstat_argv: default_network_runtime_vnstat_argv(),
            latency_monitoring_enabled: true,
            latency_monitoring_interval_secs: default_network_latency_monitoring_interval_secs(),
            latency_down_windows: default_network_latency_down_windows(),
            auto_ospf_enabled: false,
            auto_ospf_min_cost_delta: default_network_auto_ospf_min_cost_delta(),
            auto_ospf_healthy_windows: default_network_auto_ospf_healthy_windows(),
            auto_ospf_policy: OspfCostPolicy::default(),
            auto_ospf_updater: None,
            runtime_status_telemetry_plans: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentNetworkPreset {
    DebianIfupdown2Bird2,
    DebianIfupdownBird2,
    DebianNetplanBird2,
    DebianSystemdNetworkdBird2,
}

fn default_network_root_dir() -> String {
    "/".to_string()
}

fn default_network_hook_timeout_secs() -> u64 {
    10
}

fn default_network_runtime_ip_argv() -> Vec<String> {
    vec!["/sbin/ip".to_string()]
}

fn default_network_runtime_tc_argv() -> Vec<String> {
    vec!["/sbin/tc".to_string()]
}

fn default_network_runtime_command_timeout_secs() -> u64 {
    10
}

fn default_network_runtime_command_max_output_bytes() -> u32 {
    16 * 1024
}

fn default_network_status_probe_timeout_secs() -> u64 {
    5
}

fn default_network_status_probe_max_output_bytes() -> u32 {
    16 * 1024
}

fn default_network_runtime_status_telemetry_interval_secs() -> u64 {
    60
}

fn default_network_runtime_vnstat_argv() -> Vec<String> {
    Vec::new()
}

fn default_true() -> bool {
    true
}

fn default_network_latency_monitoring_interval_secs() -> u64 {
    60
}

fn default_network_latency_down_windows() -> u8 {
    3
}

fn default_network_auto_ospf_min_cost_delta() -> u16 {
    5
}

fn default_network_auto_ospf_healthy_windows() -> u8 {
    2
}

fn default_telemetry_proc_root() -> String {
    "/proc".to_string()
}

fn default_telemetry_sys_class_net_dir() -> String {
    "/sys/class/net".to_string()
}

fn default_telemetry_hostname_file() -> Option<String> {
    Some("/etc/hostname".to_string())
}

fn default_telemetry_os_release_file() -> Option<String> {
    Some("/etc/os-release".to_string())
}

fn default_execution_shell_script_argv() -> Vec<String> {
    vec!["/bin/sh".to_string(), "-lc".to_string()]
}

fn default_execution_process_proc_root() -> String {
    "/proc".to_string()
}

impl Default for AgentAuthConfig {
    fn default() -> Self {
        Self {
            command_timeout_secs: DEFAULT_MAX_COMMAND_TIMEOUT_SECS,
            gateway_retry_secs: default_agent_gateway_retry_secs(),
            gateway_connect_timeout_secs: default_agent_gateway_connect_timeout_secs(),
        }
    }
}

impl Default for AgentNoiseConfig {
    fn default() -> Self {
        Self {
            mode: AgentNoiseMode::EnrolledIk,
            client_private_key_hex: Some("11".repeat(32)),
            server_public_key_hex: Some("22".repeat(32)),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentNoiseMode {
    EnrolledIk,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            client_id: "unregistered".to_string(),
            display_name: "unregistered".to_string(),
            tcp_endpoints: vec![ServerEndpoint {
                label: "local".to_string(),
                tcp_addr: "127.0.0.1:9443".to_string(),
                priority: 10,
            }],
            noise: AgentNoiseConfig::default(),
            auth: AgentAuthConfig::default(),
            backup: AgentBackupConfig::default(),
            update: AgentUpdateConfig::default(),
            execution: AgentExecutionConfig::default(),
            telemetry: AgentTelemetryConfig::default(),
            network: AgentNetworkConfig::default(),
            telemetry_light_secs: 15,
            telemetry_full_secs: 60,
            tags: Vec::new(),
        }
    }
}
