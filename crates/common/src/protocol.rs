use std::collections::BTreeMap;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    AgentMetrics, FilePushChunk, PrivilegeAssertion, ProtocolError, TunnelConfigBackend,
    TunnelEndpointSide, TunnelPlan, MAX_DIRECT_FILE_DOWNLOAD_BYTES,
};

pub const NETWORK_SPEED_TEST_MIN_DURATION_SECS: u8 = 1;
pub const NETWORK_SPEED_TEST_MAX_DURATION_SECS: u8 = 30;
pub const NETWORK_SPEED_TEST_MIN_MAX_BYTES: u64 = 16 * 1024;
pub const NETWORK_SPEED_TEST_MAX_MAX_BYTES: u64 = 256 * 1024 * 1024;
pub const NETWORK_SPEED_TEST_MIN_RATE_LIMIT_KBPS: u32 = 64;
pub const NETWORK_SPEED_TEST_MAX_RATE_LIMIT_KBPS: u32 = 1_000_000;
pub const NETWORK_SPEED_TEST_MIN_PORT: u16 = 1024;
pub const NETWORK_SPEED_TEST_MAX_PORT: u16 = 65_535;
pub const NETWORK_SPEED_TEST_MIN_CONNECT_TIMEOUT_MS: u16 = 100;
pub const NETWORK_SPEED_TEST_MAX_CONNECT_TIMEOUT_MS: u16 = 30_000;
pub const MAX_SHELL_SCRIPT_BYTES: usize = 16 * 1024;
pub const MAX_TERMINAL_INPUT_BYTES: usize = 8 * 1024;
pub const MAX_TERMINAL_REASON_BYTES: usize = 240;
pub const MIN_TERMINAL_COLS: u16 = 20;
pub const MAX_TERMINAL_COLS: u16 = 240;
pub const MIN_TERMINAL_ROWS: u16 = 5;
pub const MAX_TERMINAL_ROWS: u16 = 120;
pub const MIN_TERMINAL_IDLE_TIMEOUT_SECS: u32 = 10;
pub const MAX_TERMINAL_IDLE_TIMEOUT_SECS: u32 = 86_400;
pub const MIN_TERMINAL_FLOW_WINDOW_BYTES: u32 = 4 * 1024;
pub const MAX_TERMINAL_FLOW_WINDOW_BYTES: u32 = 1024 * 1024;
pub const CURRENT_COMMAND_PROTOCOL_VERSION: u16 = 1;
pub const MIN_COMMAND_PROTOCOL_VERSION: u16 = 1;
pub const SHELL_COMMAND_PROTOCOL_VERSION: u16 = 1;
pub const SHELL_SCRIPT_COMMAND_PROTOCOL_VERSION: u16 = 1;
pub const TERMINAL_COMMAND_PROTOCOL_VERSION: u16 = 1;
pub const FILE_COMMAND_PROTOCOL_VERSION: u16 = 1;
pub const CONFIG_COMMAND_PROTOCOL_VERSION: u16 = 1;
pub const AGENT_UPDATE_COMMAND_PROTOCOL_VERSION: u16 = 1;
pub const USER_SESSIONS_COMMAND_PROTOCOL_VERSION: u16 = 1;
pub const PROCESS_COMMAND_PROTOCOL_VERSION: u16 = 1;
pub const BACKUP_COMMAND_PROTOCOL_VERSION: u16 = 1;
pub const RESTORE_COMMAND_PROTOCOL_VERSION: u16 = 1;
pub const NETWORK_COMMAND_PROTOCOL_VERSION: u16 = 1;

pub const JOB_STATUS_QUEUED: &str = "queued";
pub const JOB_STATUS_RUNNING: &str = "running";
pub const JOB_STATUS_COMPLETED: &str = "completed";
pub const JOB_STATUS_PARTIAL_SUCCESS: &str = "partial_success";
pub const JOB_STATUS_SKIPPED: &str = "skipped";
pub const JOB_STATUS_REJECTED: &str = "rejected";
pub const JOB_STATUS_FAILED: &str = "failed";
pub const JOB_STATUS_AGENT_TIMEOUT: &str = "agent_timeout";
pub const JOB_STATUS_CONTROL_TIMEOUT: &str = "control_timeout";
pub const JOB_STATUS_CANCELED: &str = "canceled";

pub const JOB_STATUSES: [&str; 10] = [
    JOB_STATUS_QUEUED,
    JOB_STATUS_RUNNING,
    JOB_STATUS_COMPLETED,
    JOB_STATUS_PARTIAL_SUCCESS,
    JOB_STATUS_SKIPPED,
    JOB_STATUS_REJECTED,
    JOB_STATUS_FAILED,
    JOB_STATUS_AGENT_TIMEOUT,
    JOB_STATUS_CONTROL_TIMEOUT,
    JOB_STATUS_CANCELED,
];

pub const JOB_TERMINAL_STATUSES: [&str; 8] = [
    JOB_STATUS_COMPLETED,
    JOB_STATUS_PARTIAL_SUCCESS,
    JOB_STATUS_SKIPPED,
    JOB_STATUS_REJECTED,
    JOB_STATUS_FAILED,
    JOB_STATUS_AGENT_TIMEOUT,
    JOB_STATUS_CONTROL_TIMEOUT,
    JOB_STATUS_CANCELED,
];

pub const JOB_STATUS_CLASS_IN_PROGRESS: &str = "in_progress";
pub const JOB_STATUS_CLASS_SUCCESSFUL: &str = "successful";
pub const JOB_STATUS_CLASS_PARTIAL_SUCCESS: &str = "partial_success";
pub const JOB_STATUS_CLASS_SKIPPED: &str = "skipped";
pub const JOB_STATUS_CLASS_UNSUCCESSFUL: &str = "unsuccessful";

pub const JOB_STATUS_CLASSES: [&str; 5] = [
    JOB_STATUS_CLASS_IN_PROGRESS,
    JOB_STATUS_CLASS_SUCCESSFUL,
    JOB_STATUS_CLASS_PARTIAL_SUCCESS,
    JOB_STATUS_CLASS_SKIPPED,
    JOB_STATUS_CLASS_UNSUCCESSFUL,
];

pub const TARGET_STATUS_QUEUED: &str = "queued";
pub const TARGET_STATUS_DISPATCHING: &str = "dispatching";
pub const TARGET_STATUS_RUNNING: &str = "running";
pub const TARGET_STATUS_COMPLETED: &str = "completed";
pub const TARGET_STATUS_SKIPPED: &str = "skipped";
pub const TARGET_STATUS_REJECTED: &str = "rejected";
pub const TARGET_STATUS_FAILED: &str = "failed";
pub const TARGET_STATUS_AGENT_TIMEOUT: &str = "agent_timeout";
pub const TARGET_STATUS_CONTROL_TIMEOUT: &str = "control_timeout";
pub const TARGET_STATUS_CANCELED: &str = "canceled";

pub const JOB_TARGET_STATUSES: [&str; 10] = [
    TARGET_STATUS_QUEUED,
    TARGET_STATUS_DISPATCHING,
    TARGET_STATUS_RUNNING,
    TARGET_STATUS_COMPLETED,
    TARGET_STATUS_SKIPPED,
    TARGET_STATUS_REJECTED,
    TARGET_STATUS_FAILED,
    TARGET_STATUS_AGENT_TIMEOUT,
    TARGET_STATUS_CONTROL_TIMEOUT,
    TARGET_STATUS_CANCELED,
];

pub const JOB_TARGET_TERMINAL_STATUSES: [&str; 7] = [
    TARGET_STATUS_COMPLETED,
    TARGET_STATUS_SKIPPED,
    TARGET_STATUS_REJECTED,
    TARGET_STATUS_FAILED,
    TARGET_STATUS_AGENT_TIMEOUT,
    TARGET_STATUS_CONTROL_TIMEOUT,
    TARGET_STATUS_CANCELED,
];

pub const JOB_TARGET_STATUS_CLASS_IN_PROGRESS: &str = "in_progress";
pub const JOB_TARGET_STATUS_CLASS_SUCCESSFUL: &str = "successful";
pub const JOB_TARGET_STATUS_CLASS_SKIPPED: &str = "skipped";
pub const JOB_TARGET_STATUS_CLASS_UNSUCCESSFUL: &str = "unsuccessful";

pub const JOB_TARGET_STATUS_CLASSES: [&str; 4] = [
    JOB_TARGET_STATUS_CLASS_IN_PROGRESS,
    JOB_TARGET_STATUS_CLASS_SUCCESSFUL,
    JOB_TARGET_STATUS_CLASS_SKIPPED,
    JOB_TARGET_STATUS_CLASS_UNSUCCESSFUL,
];

pub const JOB_STATUS_CLASS_BY_STATUS: [(&str, &str); 10] = [
    (JOB_STATUS_QUEUED, JOB_STATUS_CLASS_IN_PROGRESS),
    (JOB_STATUS_RUNNING, JOB_STATUS_CLASS_IN_PROGRESS),
    (JOB_STATUS_COMPLETED, JOB_STATUS_CLASS_SUCCESSFUL),
    (JOB_STATUS_PARTIAL_SUCCESS, JOB_STATUS_CLASS_PARTIAL_SUCCESS),
    (JOB_STATUS_SKIPPED, JOB_STATUS_CLASS_SKIPPED),
    (JOB_STATUS_REJECTED, JOB_STATUS_CLASS_UNSUCCESSFUL),
    (JOB_STATUS_FAILED, JOB_STATUS_CLASS_UNSUCCESSFUL),
    (JOB_STATUS_AGENT_TIMEOUT, JOB_STATUS_CLASS_UNSUCCESSFUL),
    (JOB_STATUS_CONTROL_TIMEOUT, JOB_STATUS_CLASS_UNSUCCESSFUL),
    (JOB_STATUS_CANCELED, JOB_STATUS_CLASS_UNSUCCESSFUL),
];

pub const JOB_TARGET_STATUS_CLASS_BY_STATUS: [(&str, &str); 10] = [
    (TARGET_STATUS_QUEUED, JOB_TARGET_STATUS_CLASS_IN_PROGRESS),
    (
        TARGET_STATUS_DISPATCHING,
        JOB_TARGET_STATUS_CLASS_IN_PROGRESS,
    ),
    (TARGET_STATUS_RUNNING, JOB_TARGET_STATUS_CLASS_IN_PROGRESS),
    (TARGET_STATUS_COMPLETED, JOB_TARGET_STATUS_CLASS_SUCCESSFUL),
    (TARGET_STATUS_SKIPPED, JOB_TARGET_STATUS_CLASS_SKIPPED),
    (TARGET_STATUS_REJECTED, JOB_TARGET_STATUS_CLASS_UNSUCCESSFUL),
    (TARGET_STATUS_FAILED, JOB_TARGET_STATUS_CLASS_UNSUCCESSFUL),
    (
        TARGET_STATUS_AGENT_TIMEOUT,
        JOB_TARGET_STATUS_CLASS_UNSUCCESSFUL,
    ),
    (
        TARGET_STATUS_CONTROL_TIMEOUT,
        JOB_TARGET_STATUS_CLASS_UNSUCCESSFUL,
    ),
    (TARGET_STATUS_CANCELED, JOB_TARGET_STATUS_CLASS_UNSUCCESSFUL),
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatusClass {
    InProgress,
    Successful,
    PartialSuccess,
    Skipped,
    Unsuccessful,
}

impl JobStatusClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InProgress => JOB_STATUS_CLASS_IN_PROGRESS,
            Self::Successful => JOB_STATUS_CLASS_SUCCESSFUL,
            Self::PartialSuccess => JOB_STATUS_CLASS_PARTIAL_SUCCESS,
            Self::Skipped => JOB_STATUS_CLASS_SKIPPED,
            Self::Unsuccessful => JOB_STATUS_CLASS_UNSUCCESSFUL,
        }
    }

    pub fn parse(status_class: &str) -> Option<Self> {
        match status_class {
            JOB_STATUS_CLASS_IN_PROGRESS => Some(Self::InProgress),
            JOB_STATUS_CLASS_SUCCESSFUL => Some(Self::Successful),
            JOB_STATUS_CLASS_PARTIAL_SUCCESS => Some(Self::PartialSuccess),
            JOB_STATUS_CLASS_SKIPPED => Some(Self::Skipped),
            JOB_STATUS_CLASS_UNSUCCESSFUL => Some(Self::Unsuccessful),
            _ => None,
        }
    }

    pub fn is_in_progress(self) -> bool {
        matches!(self, Self::InProgress)
    }

    pub fn is_terminal(self) -> bool {
        !self.is_in_progress()
    }

    pub fn is_successful_outcome(self) -> bool {
        matches!(self, Self::Successful | Self::PartialSuccess)
    }

    pub fn is_unsuccessful_outcome(self) -> bool {
        matches!(self, Self::Unsuccessful)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobTargetStatusClass {
    InProgress,
    Successful,
    Skipped,
    Unsuccessful,
}

impl JobTargetStatusClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InProgress => JOB_TARGET_STATUS_CLASS_IN_PROGRESS,
            Self::Successful => JOB_TARGET_STATUS_CLASS_SUCCESSFUL,
            Self::Skipped => JOB_TARGET_STATUS_CLASS_SKIPPED,
            Self::Unsuccessful => JOB_TARGET_STATUS_CLASS_UNSUCCESSFUL,
        }
    }

    pub fn parse(status_class: &str) -> Option<Self> {
        match status_class {
            JOB_TARGET_STATUS_CLASS_IN_PROGRESS => Some(Self::InProgress),
            JOB_TARGET_STATUS_CLASS_SUCCESSFUL => Some(Self::Successful),
            JOB_TARGET_STATUS_CLASS_SKIPPED => Some(Self::Skipped),
            JOB_TARGET_STATUS_CLASS_UNSUCCESSFUL => Some(Self::Unsuccessful),
            _ => None,
        }
    }

    pub fn is_in_progress(self) -> bool {
        matches!(self, Self::InProgress)
    }

    pub fn is_terminal(self) -> bool {
        !self.is_in_progress()
    }

    pub fn is_successful_outcome(self) -> bool {
        matches!(self, Self::Successful)
    }

    pub fn is_unsuccessful_outcome(self) -> bool {
        matches!(self, Self::Unsuccessful)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Completed,
    PartialSuccess,
    Skipped,
    Rejected,
    Failed,
    AgentTimeout,
    ControlTimeout,
    Canceled,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => JOB_STATUS_QUEUED,
            Self::Running => JOB_STATUS_RUNNING,
            Self::Completed => JOB_STATUS_COMPLETED,
            Self::PartialSuccess => JOB_STATUS_PARTIAL_SUCCESS,
            Self::Skipped => JOB_STATUS_SKIPPED,
            Self::Rejected => JOB_STATUS_REJECTED,
            Self::Failed => JOB_STATUS_FAILED,
            Self::AgentTimeout => JOB_STATUS_AGENT_TIMEOUT,
            Self::ControlTimeout => JOB_STATUS_CONTROL_TIMEOUT,
            Self::Canceled => JOB_STATUS_CANCELED,
        }
    }

    pub fn parse(status: &str) -> Option<Self> {
        match status {
            JOB_STATUS_QUEUED => Some(Self::Queued),
            JOB_STATUS_RUNNING => Some(Self::Running),
            JOB_STATUS_COMPLETED => Some(Self::Completed),
            JOB_STATUS_PARTIAL_SUCCESS => Some(Self::PartialSuccess),
            JOB_STATUS_SKIPPED => Some(Self::Skipped),
            JOB_STATUS_REJECTED => Some(Self::Rejected),
            JOB_STATUS_FAILED => Some(Self::Failed),
            JOB_STATUS_AGENT_TIMEOUT => Some(Self::AgentTimeout),
            JOB_STATUS_CONTROL_TIMEOUT => Some(Self::ControlTimeout),
            JOB_STATUS_CANCELED => Some(Self::Canceled),
            _ => None,
        }
    }

    pub fn class(self) -> JobStatusClass {
        match self {
            Self::Queued | Self::Running => JobStatusClass::InProgress,
            Self::Completed => JobStatusClass::Successful,
            Self::PartialSuccess => JobStatusClass::PartialSuccess,
            Self::Skipped => JobStatusClass::Skipped,
            Self::Rejected
            | Self::Failed
            | Self::AgentTimeout
            | Self::ControlTimeout
            | Self::Canceled => JobStatusClass::Unsuccessful,
        }
    }

    pub fn is_in_progress(self) -> bool {
        self.class().is_in_progress()
    }

    pub fn is_terminal(self) -> bool {
        self.class().is_terminal()
    }

    pub fn is_success(self) -> bool {
        self.class().is_successful_outcome()
    }

    pub fn is_unsuccessful_terminal(self) -> bool {
        self.class().is_unsuccessful_outcome()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobTargetStatus {
    Queued,
    Dispatching,
    Running,
    Completed,
    Skipped,
    Rejected,
    Failed,
    AgentTimeout,
    ControlTimeout,
    Canceled,
}

impl JobTargetStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => TARGET_STATUS_QUEUED,
            Self::Dispatching => TARGET_STATUS_DISPATCHING,
            Self::Running => TARGET_STATUS_RUNNING,
            Self::Completed => TARGET_STATUS_COMPLETED,
            Self::Skipped => TARGET_STATUS_SKIPPED,
            Self::Rejected => TARGET_STATUS_REJECTED,
            Self::Failed => TARGET_STATUS_FAILED,
            Self::AgentTimeout => TARGET_STATUS_AGENT_TIMEOUT,
            Self::ControlTimeout => TARGET_STATUS_CONTROL_TIMEOUT,
            Self::Canceled => TARGET_STATUS_CANCELED,
        }
    }

    pub fn parse(status: &str) -> Option<Self> {
        match status {
            TARGET_STATUS_QUEUED => Some(Self::Queued),
            TARGET_STATUS_DISPATCHING => Some(Self::Dispatching),
            TARGET_STATUS_RUNNING => Some(Self::Running),
            TARGET_STATUS_COMPLETED => Some(Self::Completed),
            TARGET_STATUS_SKIPPED => Some(Self::Skipped),
            TARGET_STATUS_REJECTED => Some(Self::Rejected),
            TARGET_STATUS_FAILED => Some(Self::Failed),
            TARGET_STATUS_AGENT_TIMEOUT => Some(Self::AgentTimeout),
            TARGET_STATUS_CONTROL_TIMEOUT => Some(Self::ControlTimeout),
            TARGET_STATUS_CANCELED => Some(Self::Canceled),
            _ => None,
        }
    }

    pub fn class(self) -> JobTargetStatusClass {
        match self {
            Self::Queued | Self::Dispatching | Self::Running => JobTargetStatusClass::InProgress,
            Self::Completed => JobTargetStatusClass::Successful,
            Self::Skipped => JobTargetStatusClass::Skipped,
            Self::Rejected
            | Self::Failed
            | Self::AgentTimeout
            | Self::ControlTimeout
            | Self::Canceled => JobTargetStatusClass::Unsuccessful,
        }
    }

    pub fn is_active(self) -> bool {
        self.class().is_in_progress()
    }

    pub fn is_terminal(self) -> bool {
        self.class().is_terminal()
    }

    pub fn is_success(self) -> bool {
        self.class().is_successful_outcome()
    }

    pub fn is_unsuccessful_terminal(self) -> bool {
        self.class().is_unsuccessful_outcome()
    }
}

pub fn job_statuses() -> &'static [&'static str] {
    &JOB_STATUSES
}

pub fn job_terminal_statuses() -> &'static [&'static str] {
    &JOB_TERMINAL_STATUSES
}

pub fn job_status_classes() -> &'static [&'static str] {
    &JOB_STATUS_CLASSES
}

pub fn job_status_class_by_status() -> &'static [(&'static str, &'static str)] {
    &JOB_STATUS_CLASS_BY_STATUS
}

pub fn job_target_statuses() -> &'static [&'static str] {
    &JOB_TARGET_STATUSES
}

pub fn job_target_terminal_statuses() -> &'static [&'static str] {
    &JOB_TARGET_TERMINAL_STATUSES
}

pub fn job_target_status_classes() -> &'static [&'static str] {
    &JOB_TARGET_STATUS_CLASSES
}

pub fn job_target_status_class_by_status() -> &'static [(&'static str, &'static str)] {
    &JOB_TARGET_STATUS_CLASS_BY_STATUS
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessRestartPolicy {
    #[default]
    Never,
    OnFailure,
    Always,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessRunPolicy {
    #[serde(default)]
    pub restart: ProcessRestartPolicy,
    #[serde(default)]
    pub restart_max_retries: u16,
    #[serde(default = "default_process_restart_backoff_secs")]
    pub restart_backoff_secs: u64,
    #[serde(default = "default_process_graceful_stop_secs")]
    pub graceful_stop_secs: u64,
}

impl Default for ProcessRunPolicy {
    fn default() -> Self {
        Self {
            restart: ProcessRestartPolicy::Never,
            restart_max_retries: 0,
            restart_backoff_secs: default_process_restart_backoff_secs(),
            graceful_stop_secs: default_process_graceful_stop_secs(),
        }
    }
}

impl ProcessRunPolicy {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessResourceLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_max_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pids_max: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_files_max: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_shares: Option<u32>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_new_privileges: bool,
}

impl ProcessResourceLimits {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

pub fn default_process_restart_backoff_secs() -> u64 {
    5
}

pub fn default_process_graceful_stop_secs() -> u64 {
    5
}

pub fn default_terminal_idle_timeout_secs() -> u32 {
    1800
}

pub fn default_terminal_flow_window_bytes() -> u32 {
    64 * 1024
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalUserPolicy {
    #[default]
    Fail,
    Fallback,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestoreRollbackFile {
    pub archive_path: String,
    pub destination_path: String,
    pub rollback_path: Option<String>,
    pub restored_size_bytes: u64,
    pub restored_sha256_hex: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentUpdateHeartbeat {
    pub activation_job_id: Uuid,
    pub sha256_hex: String,
    pub marker_unix: u64,
    pub observed_unix: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPrivilegeMode {
    #[default]
    Unknown,
    Root,
    Unprivileged,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentCapabilitySnapshot {
    #[serde(default)]
    pub privilege_mode: AgentPrivilegeMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_uid: Option<u32>,
    #[serde(default = "default_agent_command_timeout_secs")]
    pub command_timeout_secs: u64,
    #[serde(default)]
    pub can_attempt_privileged_ops: bool,
    #[serde(default)]
    pub can_manage_runtime_tunnels: bool,
    #[serde(default)]
    pub can_apply_process_limits: bool,
    #[serde(default)]
    pub unprivileged_hint: Option<String>,
}

impl Default for AgentCapabilitySnapshot {
    fn default() -> Self {
        Self {
            privilege_mode: AgentPrivilegeMode::Unknown,
            effective_uid: None,
            command_timeout_secs: default_agent_command_timeout_secs(),
            can_attempt_privileged_ops: false,
            can_manage_runtime_tunnels: false,
            can_apply_process_limits: false,
            unprivileged_hint: None,
        }
    }
}

fn default_agent_command_timeout_secs() -> u64 {
    3600
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentHello {
    pub client_id: String,
    pub agent_version: String,
    #[serde(default = "default_internal_build_number")]
    pub internal_build_number: u64,
    pub os_release: String,
    pub arch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_heartbeat: Option<AgentUpdateHeartbeat>,
    #[serde(default)]
    pub capabilities: AgentCapabilitySnapshot,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServerHello {
    pub server_id: String,
    pub server_version: String,
    #[serde(default = "default_internal_build_number")]
    pub server_build_number: u64,
    pub accepted: bool,
    pub message: String,
    pub telemetry_light_secs: u64,
    pub telemetry_full_secs: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TelemetryEnvelope {
    pub client_id: String,
    pub metrics: AgentMetrics,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayAgentHelloIngest {
    pub gateway_id: String,
    pub noise_public_key_hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_ip: Option<String>,
    pub hello: AgentHello,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayTelemetryIngest {
    pub gateway_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_ip: Option<String>,
    pub telemetry: TelemetryEnvelope,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayCommandOutputIngest {
    pub gateway_id: String,
    pub client_id: String,
    pub job_id: Uuid,
    pub seq: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_unix: Option<u64>,
    pub output: CommandOutput,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayTerminalOutputIngest {
    pub gateway_id: String,
    pub client_id: String,
    pub output: TerminalStreamOutput,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewaySessionLifecycleIngest {
    pub gateway_id: String,
    pub client_id: String,
    pub session_id: Uuid,
    pub noise_public_key_hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_ip: Option<String>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayCommandDispatch {
    pub client_id: String,
    pub request: JobRequest,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayCommandCancel {
    pub client_id: String,
    pub request: JobCancelRequest,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayPrivilegeVerification {
    pub intent: String,
    pub assertion: PrivilegeAssertion,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayPrivilegeVerificationResult {
    pub approved: bool,
    pub intent_hash_hex: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayCommandDispatchResult {
    pub client_id: String,
    pub job_id: Uuid,
    #[serde(default = "default_command_protocol_version")]
    pub command_version: u16,
    pub accepted: bool,
    pub message: String,
    pub outputs: Vec<CommandOutput>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayCommandCancelResult {
    pub client_id: String,
    pub job_id: Uuid,
    pub acked: bool,
    pub accepted: bool,
    pub applied: bool,
    pub message: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct GatewayForwardEventKindCounters {
    #[serde(default)]
    pub telemetry: u64,
    #[serde(default)]
    pub command_output: u64,
    #[serde(default)]
    pub lifecycle: u64,
    #[serde(default)]
    pub terminal_output: u64,
    #[serde(default)]
    pub other: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct GatewayForwardDropReasonCounters {
    #[serde(default)]
    pub global_queue_full: u64,
    #[serde(default)]
    pub target_queue_full: u64,
    #[serde(default)]
    pub expired: u64,
    #[serde(default)]
    pub coalesced: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct GatewayForwardCriticalFailureCounters {
    #[serde(default)]
    pub global_queue_full: u64,
    #[serde(default)]
    pub target_queue_full: u64,
    #[serde(default)]
    pub expired: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct GatewayForwardMetricsSnapshot {
    pub queued_events: u64,
    pub delivered_events: u64,
    pub retry_attempts: u64,
    pub active_queues: u64,
    #[serde(default)]
    pub current_queue_depth: u64,
    #[serde(default)]
    pub oldest_event_age_secs: Option<u64>,
    #[serde(default)]
    pub dropped_events: u64,
    #[serde(default)]
    pub telemetry_dropped_events: u64,
    #[serde(default)]
    pub expired_events: u64,
    #[serde(default)]
    pub critical_failures: u64,
    #[serde(default)]
    pub dropped_by_kind: GatewayForwardEventKindCounters,
    #[serde(default)]
    pub dropped_by_reason: GatewayForwardDropReasonCounters,
    #[serde(default)]
    pub critical_failures_by_reason: GatewayForwardCriticalFailureCounters,
    #[serde(default)]
    pub retained_output_truncated_events: u64,
    #[serde(default)]
    pub unhealthy: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobRequest {
    pub job_id: Uuid,
    #[serde(default = "default_command_protocol_version")]
    pub command_version: u16,
    pub command: JobCommand,
    pub timeout_secs: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobCancelRequest {
    pub job_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

pub fn default_command_protocol_version() -> u16 {
    CURRENT_COMMAND_PROTOCOL_VERSION
}

pub fn default_internal_build_number() -> u64 {
    1
}

pub fn job_command_variant_names() -> &'static [&'static str] {
    &[
        "shell",
        "shell_script",
        "terminal_open",
        "terminal_input",
        "terminal_poll",
        "terminal_resize",
        "terminal_close",
        "config_read",
        "hot_config",
        "data_source_config_patch",
        "agent_update",
        "agent_update_activate",
        "agent_update_rollback",
        "agent_update_check",
        "file_pull",
        "file_push",
        "file_push_chunked",
        "file_transfer_start",
        "file_transfer_chunk",
        "file_transfer_commit",
        "file_transfer_abort",
        "file_transfer_download_start",
        "file_transfer_download_chunk",
        "file_stat",
        "file_list_dir",
        "file_read_text",
        "file_mkdir",
        "file_write_text",
        "file_rename",
        "file_delete",
        "file_chmod",
        "file_chown",
        "file_copy",
        "file_download",
        "file_archive_tar",
        "user_sessions",
        "process_list",
        "process_start",
        "process_stop",
        "process_restart",
        "process_status",
        "process_logs",
        "backup",
        "restore",
        "restore_rollback",
        "network_apply",
        "network_ospf_cost_update",
        "network_rollback",
        "network_status",
        "network_interfaces",
        "network_probe",
        "network_speed_test",
    ]
}

pub fn job_privilege_intent_fields() -> &'static [&'static str] {
    &[
        "version",
        "action",
        "selector_expression",
        "command_type",
        "operation_payload_hash",
        "resolved_targets",
        "timeout_secs",
        "force_unprivileged",
        "privileged",
    ]
}

pub fn create_job_request_fields() -> &'static [&'static str] {
    &[
        "job_id",
        "selector_expression",
        "target_client_ids",
        "destructive",
        "confirmed",
        "command",
        "argv",
        "operation",
        "timeout_secs",
        "force_unprivileged",
        "privileged",
        "privilege_assertion",
    ]
}

pub fn schedule_privilege_intent_fields() -> &'static [&'static str] {
    &[
        "version",
        "action",
        "schedule_id",
        "name",
        "command_type",
        "operation_payload_hash",
        "selector_expression",
        "resolved_targets",
        "cron_expr",
        "timezone",
        "enabled",
        "catch_up_policy",
        "catch_up_limit",
        "retry_delay_secs",
        "max_failures",
        "deferred_until",
        "deleted",
    ]
}

pub fn parse_build_number(value: Option<&str>) -> u64 {
    value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or_else(default_internal_build_number)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum JobCommand {
    Shell {
        argv: Vec<String>,
        pty: bool,
    },
    ShellScript {
        script: String,
    },
    TerminalOpen {
        session_id: Uuid,
        argv: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user: Option<String>,
        #[serde(default)]
        user_policy: TerminalUserPolicy,
        cols: u16,
        rows: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replay_from_seq: Option<u64>,
        #[serde(default = "default_terminal_idle_timeout_secs")]
        idle_timeout_secs: u32,
        #[serde(default = "default_terminal_flow_window_bytes")]
        flow_window_bytes: u32,
    },
    TerminalInput {
        session_id: Uuid,
        input_seq: u64,
        data_base64: String,
    },
    TerminalPoll {
        session_id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replay_from_seq: Option<u64>,
    },
    TerminalResize {
        session_id: Uuid,
        cols: u16,
        rows: u16,
    },
    TerminalClose {
        session_id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    ConfigRead,
    HotConfig {
        toml: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preserve_redacted: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base_config_sha256_hex: Option<String>,
    },
    DataSourceConfigPatch {
        toml: String,
    },
    #[serde(rename = "agent_update", alias = "update_agent")]
    UpdateAgent {
        artifact_url: String,
        sha256_hex: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        artifact_signature_hex: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        artifact_signing_key_hex: Option<String>,
    },
    AgentUpdateActivate {
        staged_sha256_hex: String,
        #[serde(default, skip_serializing_if = "is_false")]
        restart_agent: bool,
    },
    AgentUpdateRollback {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rollback_sha256_hex: Option<String>,
    },
    AgentUpdateCheck {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version_url: Option<String>,
        #[serde(default = "default_agent_update_check_activate")]
        activate: bool,
        #[serde(default = "default_agent_update_check_restart_agent")]
        restart_agent: bool,
    },
    FilePull {
        path: String,
    },
    FilePush {
        path: String,
        mode: u32,
        size_bytes: u64,
        sha256_hex: String,
        data_base64: String,
        #[serde(default, skip_serializing_if = "FileExistingPolicy::is_default")]
        existing_policy: FileExistingPolicy,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gid: Option<u32>,
        #[serde(default, skip_serializing_if = "FileOwnershipPolicy::is_default")]
        ownership_policy: FileOwnershipPolicy,
    },
    FilePushChunked {
        path: String,
        mode: u32,
        size_bytes: u64,
        sha256_hex: String,
        chunks: Vec<FilePushChunk>,
        #[serde(default, skip_serializing_if = "FileExistingPolicy::is_default")]
        existing_policy: FileExistingPolicy,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gid: Option<u32>,
        #[serde(default, skip_serializing_if = "FileOwnershipPolicy::is_default")]
        ownership_policy: FileOwnershipPolicy,
    },
    FileTransferStart {
        session_id: Uuid,
        path: String,
        mode: u32,
        size_bytes: u64,
        sha256_hex: String,
        chunk_size_bytes: u32,
        #[serde(default)]
        rate_limit_kbps: u32,
        #[serde(default, skip_serializing_if = "FileExistingPolicy::is_default")]
        existing_policy: FileExistingPolicy,
        resume_token_hash: String,
    },
    FileTransferChunk {
        session_id: Uuid,
        offset: u64,
        chunk: FilePushChunk,
        resume_token_hash: String,
    },
    FileTransferCommit {
        session_id: Uuid,
        resume_token_hash: String,
    },
    FileTransferAbort {
        session_id: Uuid,
        resume_token_hash: String,
    },
    FileTransferDownloadStart {
        session_id: Uuid,
        path: String,
        chunk_size_bytes: u32,
        rate_limit_kbps: u32,
        resume_token_hash: String,
    },
    FileTransferDownloadChunk {
        session_id: Uuid,
        offset: u64,
        max_bytes: u32,
        resume_token_hash: String,
    },
    FileStat {
        path: String,
    },
    FileListDir {
        path: String,
        #[serde(default)]
        offset: u32,
        #[serde(default = "default_file_list_limit")]
        limit: u32,
        #[serde(default)]
        show_hidden: bool,
    },
    FileReadText {
        path: String,
        #[serde(default = "default_file_read_max_bytes")]
        max_bytes: u64,
    },
    FileWriteText {
        path: String,
        mode: u32,
        size_bytes: u64,
        sha256_hex: String,
        content_base64: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_sha256_hex: Option<String>,
        #[serde(default)]
        create: bool,
        #[serde(default)]
        policy: FileActionPolicy,
    },
    FileMkdir {
        path: String,
        mode: u32,
        #[serde(default)]
        recursive: bool,
        #[serde(default)]
        policy: FileActionPolicy,
    },
    FileRename {
        path: String,
        new_path: String,
        #[serde(default)]
        overwrite: bool,
        #[serde(default)]
        policy: FileActionPolicy,
    },
    FileDelete {
        path: String,
        #[serde(default)]
        recursive: bool,
        #[serde(default)]
        policy: FileActionPolicy,
    },
    FileChmod {
        path: String,
        mode: u32,
        #[serde(default)]
        recursive: bool,
        #[serde(default)]
        policy: FileActionPolicy,
    },
    FileChown {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gid: Option<u32>,
        #[serde(default)]
        recursive: bool,
        #[serde(default, skip_serializing_if = "FileOwnershipPolicy::is_default")]
        ownership_policy: FileOwnershipPolicy,
        #[serde(default)]
        policy: FileActionPolicy,
    },
    FileCopy {
        path: String,
        new_path: String,
        #[serde(default)]
        overwrite: bool,
        #[serde(default)]
        recursive: bool,
        #[serde(default)]
        policy: FileActionPolicy,
    },
    FileDownload {
        path: String,
        #[serde(default = "default_file_download_max_bytes")]
        max_bytes: u64,
    },
    FileArchiveTar {
        path: String,
        #[serde(default = "default_file_archive_max_bytes")]
        max_bytes: u64,
    },
    UserSessions,
    ProcessList {
        limit: u16,
    },
    ProcessStart {
        name: String,
        argv: Vec<String>,
        cwd: Option<String>,
        env: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "ProcessRunPolicy::is_default")]
        policy: ProcessRunPolicy,
        #[serde(default, skip_serializing_if = "ProcessResourceLimits::is_default")]
        limits: ProcessResourceLimits,
    },
    ProcessStop {
        name: String,
    },
    ProcessRestart {
        name: String,
    },
    ProcessStatus {
        name: Option<String>,
    },
    ProcessLogs {
        name: String,
        max_bytes: u32,
    },
    Backup {
        paths: Vec<String>,
        include_config: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recipient_public_key_hex: Option<String>,
    },
    Restore {
        source_backup_request_id: Uuid,
        paths: Vec<String>,
        include_config: bool,
        destination_root: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        archive_path: Option<String>,
        archive_base64: Option<String>,
        archive_size_bytes: Option<u64>,
        archive_sha256_hex: Option<String>,
        #[serde(default, skip_serializing_if = "is_false")]
        dry_run: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        post_restore_argv: Vec<String>,
    },
    RestoreRollback {
        source_restore_job_id: Uuid,
        restored_files: Vec<RestoreRollbackFile>,
    },
    NetworkApply {
        plan: Box<TunnelPlan>,
        side: TunnelEndpointSide,
        #[serde(default)]
        config_backend: TunnelConfigBackend,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        config_sha256_hex: Option<String>,
        ifupdown_sha256_hex: String,
        bird2_sha256_hex: String,
    },
    NetworkOspfCostUpdate {
        plan: Box<TunnelPlan>,
        side: TunnelEndpointSide,
        current_ospf_cost: u16,
        recommended_ospf_cost: u16,
        bird2_sha256_hex: String,
    },
    NetworkRollback {
        plan: Box<TunnelPlan>,
        side: TunnelEndpointSide,
    },
    NetworkStatus {
        plan: Box<TunnelPlan>,
        side: TunnelEndpointSide,
    },
    NetworkInterfaces,
    NetworkProbe {
        plan: Box<TunnelPlan>,
        side: TunnelEndpointSide,
        count: u8,
        interval_ms: u16,
    },
    NetworkSpeedTest {
        plan: Box<TunnelPlan>,
        server_side: TunnelEndpointSide,
        duration_secs: u8,
        max_bytes: u64,
        rate_limit_kbps: u32,
        port: u16,
        connect_timeout_ms: u16,
    },
}

pub fn job_command_protocol_version(command: &JobCommand) -> u16 {
    match command {
        JobCommand::Shell { .. } => SHELL_COMMAND_PROTOCOL_VERSION,
        JobCommand::ShellScript { .. } => SHELL_SCRIPT_COMMAND_PROTOCOL_VERSION,
        JobCommand::TerminalOpen { .. }
        | JobCommand::TerminalInput { .. }
        | JobCommand::TerminalPoll { .. }
        | JobCommand::TerminalResize { .. }
        | JobCommand::TerminalClose { .. } => TERMINAL_COMMAND_PROTOCOL_VERSION,
        JobCommand::FilePull { .. }
        | JobCommand::FilePush { .. }
        | JobCommand::FilePushChunked { .. }
        | JobCommand::FileTransferStart { .. }
        | JobCommand::FileTransferChunk { .. }
        | JobCommand::FileTransferCommit { .. }
        | JobCommand::FileTransferAbort { .. }
        | JobCommand::FileTransferDownloadStart { .. }
        | JobCommand::FileTransferDownloadChunk { .. }
        | JobCommand::FileStat { .. }
        | JobCommand::FileListDir { .. }
        | JobCommand::FileReadText { .. }
        | JobCommand::FileWriteText { .. }
        | JobCommand::FileMkdir { .. }
        | JobCommand::FileRename { .. }
        | JobCommand::FileDelete { .. }
        | JobCommand::FileChmod { .. }
        | JobCommand::FileChown { .. }
        | JobCommand::FileCopy { .. }
        | JobCommand::FileDownload { .. }
        | JobCommand::FileArchiveTar { .. } => FILE_COMMAND_PROTOCOL_VERSION,
        JobCommand::ConfigRead
        | JobCommand::HotConfig { .. }
        | JobCommand::DataSourceConfigPatch { .. } => CONFIG_COMMAND_PROTOCOL_VERSION,
        JobCommand::UpdateAgent { .. }
        | JobCommand::AgentUpdateActivate { .. }
        | JobCommand::AgentUpdateRollback { .. }
        | JobCommand::AgentUpdateCheck { .. } => AGENT_UPDATE_COMMAND_PROTOCOL_VERSION,
        JobCommand::UserSessions => USER_SESSIONS_COMMAND_PROTOCOL_VERSION,
        JobCommand::ProcessList { .. }
        | JobCommand::ProcessStart { .. }
        | JobCommand::ProcessStop { .. }
        | JobCommand::ProcessRestart { .. }
        | JobCommand::ProcessStatus { .. }
        | JobCommand::ProcessLogs { .. } => PROCESS_COMMAND_PROTOCOL_VERSION,
        JobCommand::Backup { .. } => BACKUP_COMMAND_PROTOCOL_VERSION,
        JobCommand::Restore { .. } | JobCommand::RestoreRollback { .. } => {
            RESTORE_COMMAND_PROTOCOL_VERSION
        }
        JobCommand::NetworkApply { .. }
        | JobCommand::NetworkOspfCostUpdate { .. }
        | JobCommand::NetworkRollback { .. }
        | JobCommand::NetworkStatus { .. }
        | JobCommand::NetworkInterfaces
        | JobCommand::NetworkProbe { .. }
        | JobCommand::NetworkSpeedTest { .. } => NETWORK_COMMAND_PROTOCOL_VERSION,
    }
}

pub fn job_command_min_supported_protocol_version(command: &JobCommand) -> u16 {
    match command {
        JobCommand::Shell { .. }
        | JobCommand::ShellScript { .. }
        | JobCommand::TerminalOpen { .. }
        | JobCommand::TerminalInput { .. }
        | JobCommand::TerminalPoll { .. }
        | JobCommand::TerminalResize { .. }
        | JobCommand::TerminalClose { .. }
        | JobCommand::ConfigRead
        | JobCommand::HotConfig { .. }
        | JobCommand::DataSourceConfigPatch { .. }
        | JobCommand::UpdateAgent { .. }
        | JobCommand::AgentUpdateActivate { .. }
        | JobCommand::AgentUpdateRollback { .. }
        | JobCommand::AgentUpdateCheck { .. }
        | JobCommand::FilePull { .. }
        | JobCommand::FilePush { .. }
        | JobCommand::FilePushChunked { .. }
        | JobCommand::FileTransferStart { .. }
        | JobCommand::FileTransferChunk { .. }
        | JobCommand::FileTransferCommit { .. }
        | JobCommand::FileTransferAbort { .. }
        | JobCommand::FileTransferDownloadStart { .. }
        | JobCommand::FileTransferDownloadChunk { .. }
        | JobCommand::FileStat { .. }
        | JobCommand::FileListDir { .. }
        | JobCommand::FileReadText { .. }
        | JobCommand::FileWriteText { .. }
        | JobCommand::FileMkdir { .. }
        | JobCommand::FileRename { .. }
        | JobCommand::FileDelete { .. }
        | JobCommand::FileChmod { .. }
        | JobCommand::FileChown { .. }
        | JobCommand::FileCopy { .. }
        | JobCommand::FileDownload { .. }
        | JobCommand::FileArchiveTar { .. }
        | JobCommand::UserSessions
        | JobCommand::ProcessList { .. }
        | JobCommand::ProcessStart { .. }
        | JobCommand::ProcessStop { .. }
        | JobCommand::ProcessRestart { .. }
        | JobCommand::ProcessStatus { .. }
        | JobCommand::ProcessLogs { .. }
        | JobCommand::Backup { .. }
        | JobCommand::Restore { .. }
        | JobCommand::RestoreRollback { .. }
        | JobCommand::NetworkApply { .. }
        | JobCommand::NetworkOspfCostUpdate { .. }
        | JobCommand::NetworkRollback { .. }
        | JobCommand::NetworkStatus { .. }
        | JobCommand::NetworkInterfaces
        | JobCommand::NetworkProbe { .. }
        | JobCommand::NetworkSpeedTest { .. } => MIN_COMMAND_PROTOCOL_VERSION,
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobCommandSafety {
    ReadOnly,
    Exclusive,
}

pub fn job_command_safety(command: &JobCommand) -> JobCommandSafety {
    match command {
        JobCommand::ConfigRead
        | JobCommand::FilePull { .. }
        | JobCommand::FileStat { .. }
        | JobCommand::FileListDir { .. }
        | JobCommand::FileReadText { .. }
        | JobCommand::FileDownload { .. }
        | JobCommand::FileArchiveTar { .. }
        | JobCommand::FileTransferDownloadStart { .. }
        | JobCommand::FileTransferDownloadChunk { .. }
        | JobCommand::UserSessions
        | JobCommand::ProcessList { .. }
        | JobCommand::ProcessStatus { .. }
        | JobCommand::ProcessLogs { .. }
        | JobCommand::NetworkStatus { .. }
        | JobCommand::NetworkInterfaces
        | JobCommand::NetworkProbe { .. }
        | JobCommand::NetworkSpeedTest { .. } => JobCommandSafety::ReadOnly,
        JobCommand::Shell { .. }
        | JobCommand::ShellScript { .. }
        | JobCommand::TerminalOpen { .. }
        | JobCommand::TerminalInput { .. }
        | JobCommand::TerminalPoll { .. }
        | JobCommand::TerminalResize { .. }
        | JobCommand::TerminalClose { .. }
        | JobCommand::HotConfig { .. }
        | JobCommand::DataSourceConfigPatch { .. }
        | JobCommand::UpdateAgent { .. }
        | JobCommand::AgentUpdateActivate { .. }
        | JobCommand::AgentUpdateRollback { .. }
        | JobCommand::AgentUpdateCheck { .. }
        | JobCommand::FilePush { .. }
        | JobCommand::FilePushChunked { .. }
        | JobCommand::FileTransferStart { .. }
        | JobCommand::FileTransferChunk { .. }
        | JobCommand::FileTransferCommit { .. }
        | JobCommand::FileTransferAbort { .. }
        | JobCommand::FileWriteText { .. }
        | JobCommand::FileMkdir { .. }
        | JobCommand::FileRename { .. }
        | JobCommand::FileDelete { .. }
        | JobCommand::FileChmod { .. }
        | JobCommand::FileChown { .. }
        | JobCommand::FileCopy { .. }
        | JobCommand::ProcessStart { .. }
        | JobCommand::ProcessStop { .. }
        | JobCommand::ProcessRestart { .. }
        | JobCommand::Backup { .. }
        | JobCommand::Restore { .. }
        | JobCommand::RestoreRollback { .. }
        | JobCommand::NetworkApply { .. }
        | JobCommand::NetworkOspfCostUpdate { .. }
        | JobCommand::NetworkRollback { .. } => JobCommandSafety::Exclusive,
    }
}

pub fn job_command_requires_confirmation(command: &JobCommand) -> bool {
    job_command_safety(command) == JobCommandSafety::Exclusive
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileActionPolicy {
    #[default]
    Fail,
    Ensure,
    Ignore,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileExistingPolicy {
    #[default]
    Skip,
    Replace,
}

impl FileExistingPolicy {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOwnershipPolicy {
    #[default]
    Fail,
    Ignore,
}

impl FileOwnershipPolicy {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

fn default_file_list_limit() -> u32 {
    250
}

fn default_file_read_max_bytes() -> u64 {
    1024 * 1024
}

fn default_file_archive_max_bytes() -> u64 {
    MAX_DIRECT_FILE_DOWNLOAD_BYTES
}

fn default_file_download_max_bytes() -> u64 {
    MAX_DIRECT_FILE_DOWNLOAD_BYTES
}

fn default_agent_update_check_activate() -> bool {
    true
}

fn default_agent_update_check_restart_agent() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobAck {
    pub job_id: Uuid,
    pub accepted: bool,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobCancelAck {
    pub job_id: Uuid,
    pub accepted: bool,
    pub applied: bool,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CommandOutput {
    pub job_id: Uuid,
    pub stream: OutputStream,
    pub data: Vec<u8>,
    pub exit_code: Option<i32>,
    pub done: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TerminalStreamOutput {
    pub job_id: Uuid,
    pub session_id: Uuid,
    pub terminal_seq: Option<u64>,
    pub output_first_seq: Option<u64>,
    pub output_next_seq: u64,
    pub output_retained_first_seq: Option<u64>,
    pub output_retained_bytes: u64,
    pub output_dropped_bytes: u64,
    pub output_dropped_chunks: u64,
    pub output_replay_truncated: bool,
    pub output: CommandOutput,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    Stdout,
    Stderr,
    Pty,
    Status,
}

pub fn encode_json<T: Serialize>(value: &T) -> Result<Vec<u8>, ProtocolError> {
    Ok(serde_json::to_vec(value)?)
}

pub fn decode_json<T: DeserializeOwned>(payload: &[u8]) -> Result<T, ProtocolError> {
    Ok(serde_json::from_slice(payload)?)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{
        job_status_class_by_status, job_status_classes, job_statuses,
        job_target_status_class_by_status, job_target_status_classes, job_target_statuses,
        parse_build_number, JobCommand, JobStatus, JobStatusClass, JobTargetStatus,
        JobTargetStatusClass, ServerHello, JOB_STATUS_CLASSES, JOB_STATUS_PARTIAL_SUCCESS,
        JOB_STATUS_SKIPPED, JOB_TARGET_STATUS_CLASSES, TARGET_STATUS_SKIPPED,
    };

    #[test]
    fn parses_positive_build_numbers_with_default_fallback() {
        assert_eq!(parse_build_number(Some("42")), 42);
        assert_eq!(parse_build_number(Some(" 7 ")), 7);
        assert_eq!(parse_build_number(Some("0")), 1);
        assert_eq!(parse_build_number(Some("not-a-number")), 1);
        assert_eq!(parse_build_number(None), 1);
    }

    #[test]
    fn server_hello_carries_server_version_and_build_number() {
        let hello = ServerHello {
            server_id: "gateway-a".to_string(),
            server_version: "0.1.0".to_string(),
            server_build_number: 1001,
            accepted: true,
            message: "accepted".to_string(),
            telemetry_light_secs: 15,
            telemetry_full_secs: 60,
        };

        let encoded = serde_json::to_value(&hello).unwrap();
        assert_eq!(encoded["server_version"], "0.1.0");
        assert_eq!(encoded["server_build_number"], 1001);
    }

    #[test]
    fn job_status_model_is_total_and_strict() {
        let mut mapped_statuses = BTreeSet::new();
        let mut used_classes = BTreeSet::new();
        for (status, status_class) in job_status_class_by_status() {
            mapped_statuses.insert(*status);
            used_classes.insert(*status_class);
            let parsed_status = JobStatus::parse(status).expect("canonical job status parses");
            let parsed_class =
                JobStatusClass::parse(status_class).expect("canonical job status class parses");
            assert_eq!(parsed_status.as_str(), *status);
            assert_eq!(parsed_status.class(), parsed_class);
            assert_eq!(
                parsed_status.is_in_progress(),
                parsed_class.is_in_progress()
            );
            assert_eq!(parsed_status.is_terminal(), parsed_class.is_terminal());
            assert_eq!(
                parsed_status.is_success(),
                parsed_class.is_successful_outcome()
            );
            assert_eq!(
                parsed_status.is_unsuccessful_terminal(),
                parsed_class.is_unsuccessful_outcome()
            );
        }
        assert_eq!(
            mapped_statuses,
            job_statuses().iter().copied().collect::<BTreeSet<_>>()
        );
        assert_eq!(
            used_classes,
            job_status_classes()
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
        );
        assert_eq!(
            job_status_classes(),
            &JOB_STATUS_CLASSES,
            "generated helper must expose the canonical class array"
        );
        assert!(JobStatus::parse("not_canonical_job_status").is_none());
        assert_eq!(
            JobStatus::parse(JOB_STATUS_PARTIAL_SUCCESS)
                .unwrap()
                .class(),
            JobStatusClass::PartialSuccess
        );
        assert_eq!(
            JobStatus::parse(JOB_STATUS_SKIPPED).unwrap().class(),
            JobStatusClass::Skipped
        );
    }

    #[test]
    fn target_status_model_is_total_and_strict() {
        let mut mapped_statuses = BTreeSet::new();
        let mut used_classes = BTreeSet::new();
        for (status, status_class) in job_target_status_class_by_status() {
            mapped_statuses.insert(*status);
            used_classes.insert(*status_class);
            let parsed_status =
                JobTargetStatus::parse(status).expect("canonical target status parses");
            let parsed_class = JobTargetStatusClass::parse(status_class)
                .expect("canonical target status class parses");
            assert_eq!(parsed_status.as_str(), *status);
            assert_eq!(parsed_status.class(), parsed_class);
            assert_eq!(parsed_status.is_active(), parsed_class.is_in_progress());
            assert_eq!(parsed_status.is_terminal(), parsed_class.is_terminal());
            assert_eq!(
                parsed_status.is_success(),
                parsed_class.is_successful_outcome()
            );
            assert_eq!(
                parsed_status.is_unsuccessful_terminal(),
                parsed_class.is_unsuccessful_outcome()
            );
        }
        assert_eq!(
            mapped_statuses,
            job_target_statuses()
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
        );
        assert_eq!(
            used_classes,
            job_target_status_classes()
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
        );
        assert_eq!(
            job_target_status_classes(),
            &JOB_TARGET_STATUS_CLASSES,
            "generated helper must expose the canonical target class array"
        );
        assert!(JobTargetStatus::parse("not_canonical_target_status").is_none());
        assert_eq!(
            JobTargetStatus::parse(TARGET_STATUS_SKIPPED)
                .unwrap()
                .class(),
            JobTargetStatusClass::Skipped
        );
    }

    #[test]
    fn serializes_agent_update_with_canonical_name_and_accepts_legacy_alias() {
        let command = JobCommand::UpdateAgent {
            artifact_url: "https://updates.example/vpsman-agent".to_string(),
            sha256_hex: "ab".repeat(32),
            artifact_signature_hex: None,
            artifact_signing_key_hex: None,
        };
        let encoded = serde_json::to_value(&command).unwrap();
        assert_eq!(encoded["type"], "agent_update");
        assert!(encoded.get("artifact_signature_hex").is_none());

        let legacy = serde_json::json!({
            "type": "update_agent",
            "artifact_url": "https://updates.example/vpsman-agent",
            "sha256_hex": "ab".repeat(32),
        });
        assert!(matches!(
            serde_json::from_value::<JobCommand>(legacy).unwrap(),
            JobCommand::UpdateAgent { .. }
        ));
    }

    #[test]
    fn omits_false_agent_update_restart_flag_from_payload_hash_shape() {
        let command = JobCommand::AgentUpdateActivate {
            staged_sha256_hex: "ab".repeat(32),
            restart_agent: false,
        };
        let encoded = serde_json::to_value(&command).unwrap();
        assert_eq!(encoded["type"], "agent_update_activate");
        assert!(encoded.get("restart_agent").is_none());

        let restart = JobCommand::AgentUpdateActivate {
            staged_sha256_hex: "ab".repeat(32),
            restart_agent: true,
        };
        let encoded = serde_json::to_value(&restart).unwrap();
        assert_eq!(encoded["restart_agent"], true);
    }
}
