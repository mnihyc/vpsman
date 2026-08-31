use std::collections::{BTreeMap, BTreeSet};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    AgentMetrics, AgentRuntimeConfig, FilePushChunk, PrivilegeAssertion, ProtocolError,
    RoutingCostAdapterCommands, TunnelEndpointSide, TunnelPlan, MAX_DIRECT_FILE_DOWNLOAD_BYTES,
};

pub const NETWORK_SPEED_TEST_MIN_DURATION_SECS: u8 = 1;
pub const NETWORK_SPEED_TEST_MAX_DURATION_SECS: u8 = 30;
pub const NETWORK_SPEED_TEST_MIN_MAX_BYTES: u64 = 16 * 1024;
pub const NETWORK_SPEED_TEST_MAX_MAX_BYTES: u64 = 256 * 1024 * 1024;
pub const NETWORK_SPEED_TEST_UNLIMITED_MAX_BYTES: u64 = 0;
pub const NETWORK_SPEED_TEST_MIN_RATE_LIMIT_KBPS: u32 = 64;
pub const NETWORK_SPEED_TEST_MAX_RATE_LIMIT_KBPS: u32 = 1_000_000;
pub const NETWORK_SPEED_TEST_UNLIMITED_RATE_LIMIT_KBPS: u32 = 0;
pub const NETWORK_SPEED_TEST_MIN_PORT: u16 = 1024;
pub const NETWORK_SPEED_TEST_MAX_PORT: u16 = 65_535;
pub const NETWORK_SPEED_TEST_MIN_CONNECT_TIMEOUT_MS: u16 = 100;
pub const NETWORK_SPEED_TEST_MAX_CONNECT_TIMEOUT_MS: u16 = 30_000;
pub const NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES: usize = 16;
pub const NETWORK_TRAFFIC_IMPORT_MAX_BUCKETS_PER_INTERFACE: usize = 20_000;
pub const NETWORK_TRAFFIC_IMPORT_BUCKETS_PER_OUTPUT: usize = 128;
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
pub const CURRENT_COMMAND_PROTOCOL_VERSION: u16 = 5;
pub const MIN_COMMAND_PROTOCOL_VERSION: u16 = 1;
pub const SHELL_COMMAND_PROTOCOL_VERSION: u16 = 1;
pub const SHELL_SCRIPT_COMMAND_PROTOCOL_VERSION: u16 = 1;
pub const TERMINAL_COMMAND_PROTOCOL_VERSION: u16 = 1;
pub const FILE_COMMAND_PROTOCOL_VERSION: u16 = 1;
pub const CONFIG_COMMAND_PROTOCOL_VERSION: u16 = 3;
pub const AGENT_UPDATE_COMMAND_PROTOCOL_VERSION: u16 = 1;
pub const AGENT_LIFECYCLE_COMMAND_PROTOCOL_VERSION: u16 = 5;
pub const USER_SESSIONS_COMMAND_PROTOCOL_VERSION: u16 = 1;
pub const PROCESS_COMMAND_PROTOCOL_VERSION: u16 = 1;
pub const HOST_SERVICE_COMMAND_PROTOCOL_VERSION: u16 = 2;
pub const HOST_PACKAGE_COMMAND_PROTOCOL_VERSION: u16 = 2;
pub const HOST_STORAGE_COMMAND_PROTOCOL_VERSION: u16 = 2;
pub const BACKUP_COMMAND_PROTOCOL_VERSION: u16 = 3;
pub const RESTORE_COMMAND_PROTOCOL_VERSION: u16 = 2;
pub const NETWORK_COMMAND_PROTOCOL_VERSION: u16 = 2;
pub const NETWORK_ROUTING_COMMAND_PROTOCOL_VERSION: u16 = 3;
pub const NETWORK_TRAFFIC_IMPORT_COMMAND_PROTOCOL_VERSION: u16 = 4;

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
pub const TARGET_STATUS_AGENT_LOST: &str = "agent_lost";
pub const TARGET_STATUS_AGENT_TIMEOUT: &str = "agent_timeout";
pub const TARGET_STATUS_CONTROL_TIMEOUT: &str = "control_timeout";
pub const TARGET_STATUS_CANCELED: &str = "canceled";

pub const JOB_TARGET_STATUSES: [&str; 11] = [
    TARGET_STATUS_QUEUED,
    TARGET_STATUS_DISPATCHING,
    TARGET_STATUS_RUNNING,
    TARGET_STATUS_COMPLETED,
    TARGET_STATUS_SKIPPED,
    TARGET_STATUS_REJECTED,
    TARGET_STATUS_FAILED,
    TARGET_STATUS_AGENT_LOST,
    TARGET_STATUS_AGENT_TIMEOUT,
    TARGET_STATUS_CONTROL_TIMEOUT,
    TARGET_STATUS_CANCELED,
];

pub const JOB_TARGET_TERMINAL_STATUSES: [&str; 8] = [
    TARGET_STATUS_COMPLETED,
    TARGET_STATUS_SKIPPED,
    TARGET_STATUS_REJECTED,
    TARGET_STATUS_FAILED,
    TARGET_STATUS_AGENT_LOST,
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

pub const JOB_TARGET_STATUS_CLASS_BY_STATUS: [(&str, &str); 11] = [
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
        TARGET_STATUS_AGENT_LOST,
        JOB_TARGET_STATUS_CLASS_UNSUCCESSFUL,
    ),
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

pub const WORKFLOW_STATUS_CLASS_IN_PROGRESS: &str = "in_progress";
pub const WORKFLOW_STATUS_CLASS_SUCCESSFUL: &str = "successful";
pub const WORKFLOW_STATUS_CLASS_WARNING: &str = "warning";
pub const WORKFLOW_STATUS_CLASS_NEUTRAL: &str = "neutral";

pub const WORKFLOW_STATUS_CLASSES: [&str; 4] = [
    WORKFLOW_STATUS_CLASS_IN_PROGRESS,
    WORKFLOW_STATUS_CLASS_SUCCESSFUL,
    WORKFLOW_STATUS_CLASS_WARNING,
    WORKFLOW_STATUS_CLASS_NEUTRAL,
];

pub const TERMINAL_SESSION_STATE_CLASS_BY_STATE: [(&str, &str); 7] = [
    ("opening", WORKFLOW_STATUS_CLASS_IN_PROGRESS),
    ("open", WORKFLOW_STATUS_CLASS_IN_PROGRESS),
    ("closed", WORKFLOW_STATUS_CLASS_SUCCESSFUL),
    ("missing", WORKFLOW_STATUS_CLASS_WARNING),
    ("rejected", WORKFLOW_STATUS_CLASS_WARNING),
    ("failed", WORKFLOW_STATUS_CLASS_WARNING),
    ("exited", WORKFLOW_STATUS_CLASS_SUCCESSFUL),
];

pub const TERMINAL_SESSION_STATUS_CLASS_BY_STATUS: [(&str, &str); 14] = [
    ("opening", WORKFLOW_STATUS_CLASS_IN_PROGRESS),
    ("opened", WORKFLOW_STATUS_CLASS_IN_PROGRESS),
    ("attached", WORKFLOW_STATUS_CLASS_IN_PROGRESS),
    ("rejected", WORKFLOW_STATUS_CLASS_WARNING),
    ("failed", WORKFLOW_STATUS_CLASS_WARNING),
    ("accepted", WORKFLOW_STATUS_CLASS_SUCCESSFUL),
    ("resized", WORKFLOW_STATUS_CLASS_SUCCESSFUL),
    ("closed", WORKFLOW_STATUS_CLASS_SUCCESSFUL),
    ("missing", WORKFLOW_STATUS_CLASS_WARNING),
    ("streaming", WORKFLOW_STATUS_CLASS_IN_PROGRESS),
    ("exited", WORKFLOW_STATUS_CLASS_SUCCESSFUL),
    ("idle_timeout", WORKFLOW_STATUS_CLASS_WARNING),
    ("disconnected_timeout", WORKFLOW_STATUS_CLASS_WARNING),
    ("lifecycle_disconnected", WORKFLOW_STATUS_CLASS_WARNING),
];

pub const FILE_TRANSFER_SESSION_STATUS_CLASS_BY_STATUS: [(&str, &str); 5] = [
    ("started", WORKFLOW_STATUS_CLASS_IN_PROGRESS),
    ("transferring", WORKFLOW_STATUS_CLASS_IN_PROGRESS),
    ("completed", WORKFLOW_STATUS_CLASS_SUCCESSFUL),
    ("aborted", WORKFLOW_STATUS_CLASS_WARNING),
    ("unknown", WORKFLOW_STATUS_CLASS_NEUTRAL),
];

pub const BACKUP_REQUEST_STATUS_CLASS_BY_STATUS: [(&str, &str); 4] = [
    ("requested_metadata_only", WORKFLOW_STATUS_CLASS_IN_PROGRESS),
    (
        "artifact_metadata_recorded",
        WORKFLOW_STATUS_CLASS_SUCCESSFUL,
    ),
    ("execution_failed", WORKFLOW_STATUS_CLASS_WARNING),
    ("execution_canceled", WORKFLOW_STATUS_CLASS_WARNING),
];

pub const RESTORE_PLAN_STATUS_CLASS_BY_STATUS: [(&str, &str); 1] =
    [("planned_metadata_only", WORKFLOW_STATUS_CLASS_NEUTRAL)];

pub const MIGRATION_LINK_STATUS_CLASS_BY_STATUS: [(&str, &str); 1] =
    [("linked_metadata_only", WORKFLOW_STATUS_CLASS_SUCCESSFUL)];

pub const AGENT_UPDATE_RELEASE_STATUS_CLASS_BY_STATUS: [(&str, &str); 1] =
    [("published_external", WORKFLOW_STATUS_CLASS_SUCCESSFUL)];

pub const SERVER_JOB_STATUS_CLASS_BY_STATUS: [(&str, &str); 5] = [
    ("queued", WORKFLOW_STATUS_CLASS_IN_PROGRESS),
    ("running", WORKFLOW_STATUS_CLASS_IN_PROGRESS),
    ("completed", WORKFLOW_STATUS_CLASS_SUCCESSFUL),
    ("failed", WORKFLOW_STATUS_CLASS_WARNING),
    ("canceled", WORKFLOW_STATUS_CLASS_WARNING),
];

pub const FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_CLASS_BY_STATUS: [(&str, &str); 7] = [
    ("queued", WORKFLOW_STATUS_CLASS_IN_PROGRESS),
    ("in_progress", WORKFLOW_STATUS_CLASS_IN_PROGRESS),
    ("failed", WORKFLOW_STATUS_CLASS_WARNING),
    ("permanently_failed", WORKFLOW_STATUS_CLASS_WARNING),
    ("canceled_disabled", WORKFLOW_STATUS_CLASS_NEUTRAL),
    ("delivered", WORKFLOW_STATUS_CLASS_SUCCESSFUL),
    ("matched_dry_run", WORKFLOW_STATUS_CLASS_NEUTRAL),
];

pub const FLEET_ALERT_NOTIFICATION_DELIVERY_PROCESS_STATUS_CLASS_BY_STATUS: [(&str, &str); 2] = [
    ("queued", WORKFLOW_STATUS_CLASS_IN_PROGRESS),
    ("failed", WORKFLOW_STATUS_CLASS_WARNING),
];

pub const WEBHOOK_RULE_DELIVERY_STATUS_CLASS_BY_STATUS: [(&str, &str); 7] = [
    ("queued", WORKFLOW_STATUS_CLASS_IN_PROGRESS),
    ("in_progress", WORKFLOW_STATUS_CLASS_IN_PROGRESS),
    ("failed", WORKFLOW_STATUS_CLASS_WARNING),
    ("permanently_failed", WORKFLOW_STATUS_CLASS_WARNING),
    ("canceled_disabled", WORKFLOW_STATUS_CLASS_NEUTRAL),
    ("delivered", WORKFLOW_STATUS_CLASS_SUCCESSFUL),
    ("matched_dry_run", WORKFLOW_STATUS_CLASS_NEUTRAL),
];

pub const WEBHOOK_RULE_DELIVERY_HISTORY_STATUS_CLASS_BY_STATUS: [(&str, &str); 6] = [
    ("queued", WORKFLOW_STATUS_CLASS_IN_PROGRESS),
    ("in_progress", WORKFLOW_STATUS_CLASS_IN_PROGRESS),
    ("failed", WORKFLOW_STATUS_CLASS_WARNING),
    ("permanently_failed", WORKFLOW_STATUS_CLASS_WARNING),
    ("canceled_disabled", WORKFLOW_STATUS_CLASS_NEUTRAL),
    ("delivered", WORKFLOW_STATUS_CLASS_SUCCESSFUL),
];

pub const WEBHOOK_RULE_DELIVERY_PROCESS_STATUS_CLASS_BY_STATUS: [(&str, &str); 2] = [
    ("queued", WORKFLOW_STATUS_CLASS_IN_PROGRESS),
    ("failed", WORKFLOW_STATUS_CLASS_WARNING),
];

pub const TOPOLOGY_NODE_STATUS_CLASS_BY_STATUS: [(&str, &str); 5] = [
    ("online", WORKFLOW_STATUS_CLASS_SUCCESSFUL),
    ("offline", WORKFLOW_STATUS_CLASS_WARNING),
    ("never", WORKFLOW_STATUS_CLASS_NEUTRAL),
    ("stale", WORKFLOW_STATUS_CLASS_WARNING),
    ("unknown", WORKFLOW_STATUS_CLASS_NEUTRAL),
];

pub const TOPOLOGY_EDGE_HEALTH_STATUS_CLASS_BY_STATUS: [(&str, &str); 4] = [
    ("disabled", WORKFLOW_STATUS_CLASS_NEUTRAL),
    ("unknown", WORKFLOW_STATUS_CLASS_NEUTRAL),
    ("healthy", WORKFLOW_STATUS_CLASS_SUCCESSFUL),
    ("degraded", WORKFLOW_STATUS_CLASS_WARNING),
];

pub const TOPOLOGY_NEIGHBOR_STATE_CLASS_BY_STATE: [(&str, &str); 5] = [
    ("unknown", WORKFLOW_STATUS_CLASS_NEUTRAL),
    ("healthy", WORKFLOW_STATUS_CLASS_SUCCESSFUL),
    ("kernel_probe_success", WORKFLOW_STATUS_CLASS_SUCCESSFUL),
    ("kernel_probe_failed", WORKFLOW_STATUS_CLASS_WARNING),
    ("not_probed", WORKFLOW_STATUS_CLASS_NEUTRAL),
];

pub const TOPOLOGY_PROBE_STATE_CLASS_BY_STATE: [(&str, &str); 4] = [
    ("unknown", WORKFLOW_STATUS_CLASS_NEUTRAL),
    ("success", WORKFLOW_STATUS_CLASS_SUCCESSFUL),
    ("failed", WORKFLOW_STATUS_CLASS_WARNING),
    ("skipped", WORKFLOW_STATUS_CLASS_NEUTRAL),
];

pub const TOPOLOGY_RUNTIME_STATE_CLASS_BY_STATE: [(&str, &str); 11] = [
    ("unknown", WORKFLOW_STATUS_CLASS_NEUTRAL),
    ("adapter_unhealthy", WORKFLOW_STATUS_CLASS_WARNING),
    ("routing_unhealthy", WORKFLOW_STATUS_CLASS_WARNING),
    ("drift", WORKFLOW_STATUS_CLASS_WARNING),
    ("unhealthy", WORKFLOW_STATUS_CLASS_WARNING),
    ("degraded", WORKFLOW_STATUS_CLASS_WARNING),
    ("observed", WORKFLOW_STATUS_CLASS_NEUTRAL),
    ("healthy", WORKFLOW_STATUS_CLASS_SUCCESSFUL),
    ("not_applicable", WORKFLOW_STATUS_CLASS_NEUTRAL),
    ("not_configured", WORKFLOW_STATUS_CLASS_NEUTRAL),
    ("skipped", WORKFLOW_STATUS_CLASS_NEUTRAL),
];

pub const TOPOLOGY_OBSERVATION_STATE_CLASS_BY_STATE: [(&str, &str); 4] = [
    ("unknown", WORKFLOW_STATUS_CLASS_NEUTRAL),
    ("healthy", WORKFLOW_STATUS_CLASS_SUCCESSFUL),
    ("degraded", WORKFLOW_STATUS_CLASS_WARNING),
    ("recorded", WORKFLOW_STATUS_CLASS_SUCCESSFUL),
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
    AgentLost,
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
            Self::AgentLost => TARGET_STATUS_AGENT_LOST,
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
            TARGET_STATUS_AGENT_LOST => Some(Self::AgentLost),
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
            | Self::AgentLost
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

pub fn workflow_status_classes() -> &'static [&'static str] {
    &WORKFLOW_STATUS_CLASSES
}

pub fn terminal_session_state_class_by_state() -> &'static [(&'static str, &'static str)] {
    &TERMINAL_SESSION_STATE_CLASS_BY_STATE
}

pub fn terminal_session_status_class_by_status() -> &'static [(&'static str, &'static str)] {
    &TERMINAL_SESSION_STATUS_CLASS_BY_STATUS
}

pub fn file_transfer_session_status_class_by_status() -> &'static [(&'static str, &'static str)] {
    &FILE_TRANSFER_SESSION_STATUS_CLASS_BY_STATUS
}

pub fn backup_request_status_class_by_status() -> &'static [(&'static str, &'static str)] {
    &BACKUP_REQUEST_STATUS_CLASS_BY_STATUS
}

pub fn restore_plan_status_class_by_status() -> &'static [(&'static str, &'static str)] {
    &RESTORE_PLAN_STATUS_CLASS_BY_STATUS
}

pub fn migration_link_status_class_by_status() -> &'static [(&'static str, &'static str)] {
    &MIGRATION_LINK_STATUS_CLASS_BY_STATUS
}

pub fn agent_update_release_status_class_by_status() -> &'static [(&'static str, &'static str)] {
    &AGENT_UPDATE_RELEASE_STATUS_CLASS_BY_STATUS
}

pub fn server_job_status_class_by_status() -> &'static [(&'static str, &'static str)] {
    &SERVER_JOB_STATUS_CLASS_BY_STATUS
}

pub fn fleet_alert_notification_delivery_status_class_by_status(
) -> &'static [(&'static str, &'static str)] {
    &FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_CLASS_BY_STATUS
}

pub fn fleet_alert_notification_delivery_process_status_class_by_status(
) -> &'static [(&'static str, &'static str)] {
    &FLEET_ALERT_NOTIFICATION_DELIVERY_PROCESS_STATUS_CLASS_BY_STATUS
}

pub fn webhook_rule_delivery_status_class_by_status() -> &'static [(&'static str, &'static str)] {
    &WEBHOOK_RULE_DELIVERY_STATUS_CLASS_BY_STATUS
}

pub fn webhook_rule_delivery_history_status_class_by_status(
) -> &'static [(&'static str, &'static str)] {
    &WEBHOOK_RULE_DELIVERY_HISTORY_STATUS_CLASS_BY_STATUS
}

pub fn webhook_rule_delivery_process_status_class_by_status(
) -> &'static [(&'static str, &'static str)] {
    &WEBHOOK_RULE_DELIVERY_PROCESS_STATUS_CLASS_BY_STATUS
}

pub fn topology_node_status_class_by_status() -> &'static [(&'static str, &'static str)] {
    &TOPOLOGY_NODE_STATUS_CLASS_BY_STATUS
}

pub fn topology_edge_health_status_class_by_status() -> &'static [(&'static str, &'static str)] {
    &TOPOLOGY_EDGE_HEALTH_STATUS_CLASS_BY_STATUS
}

pub fn topology_neighbor_state_class_by_state() -> &'static [(&'static str, &'static str)] {
    &TOPOLOGY_NEIGHBOR_STATE_CLASS_BY_STATE
}

pub fn topology_probe_state_class_by_state() -> &'static [(&'static str, &'static str)] {
    &TOPOLOGY_PROBE_STATE_CLASS_BY_STATE
}

pub fn topology_runtime_state_class_by_state() -> &'static [(&'static str, &'static str)] {
    &TOPOLOGY_RUNTIME_STATE_CLASS_BY_STATE
}

pub fn topology_observation_state_class_by_state() -> &'static [(&'static str, &'static str)] {
    &TOPOLOGY_OBSERVATION_STATE_CLASS_BY_STATE
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
    3600
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
    #[serde(default = "default_agent_max_job_timeout_secs")]
    pub max_job_timeout_secs: u64,
    #[serde(default)]
    pub can_attempt_privileged_ops: bool,
    #[serde(default)]
    pub can_manage_runtime_tunnels: bool,
    #[serde(default)]
    pub builtin_tunnel_drivers: AgentBuiltinTunnelDriverCapabilities,
    #[serde(default)]
    pub can_apply_process_limits: bool,
    #[serde(default)]
    pub port_forwarding: crate::PortForwardCapability,
    #[serde(default)]
    pub unprivileged_hint: Option<String>,
}

impl Default for AgentCapabilitySnapshot {
    fn default() -> Self {
        Self {
            privilege_mode: AgentPrivilegeMode::Unknown,
            effective_uid: None,
            max_job_timeout_secs: default_agent_max_job_timeout_secs(),
            can_attempt_privileged_ops: false,
            can_manage_runtime_tunnels: false,
            builtin_tunnel_drivers: AgentBuiltinTunnelDriverCapabilities::default(),
            can_apply_process_limits: false,
            port_forwarding: crate::PortForwardCapability::default(),
            unprivileged_hint: None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentBuiltinTunnelDriverCapabilities {
    #[serde(default)]
    pub iproute2: AgentBuiltinTunnelDriverCapability,
    #[serde(default)]
    pub wireguard: AgentBuiltinTunnelDriverCapability,
    #[serde(default)]
    pub openvpn: AgentBuiltinTunnelDriverCapability,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentBuiltinTunnelDriverCapability {
    #[serde(default)]
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

fn default_agent_max_job_timeout_secs() -> u64 {
    DEFAULT_MAX_JOB_TIMEOUT_SECS
}

pub const DEFAULT_MAX_JOB_TIMEOUT_SECS: u64 = 3600;
pub const MAX_CONFIGURABLE_JOB_TIMEOUT_SECS: u64 = 7 * 24 * 60 * 60;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentHello {
    pub client_id: String,
    pub process_incarnation_id: Uuid,
    pub agent_version: String,
    pub internal_build_number: u64,
    pub os_release: String,
    pub arch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_release: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtualization: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_heartbeat: Option<AgentUpdateHeartbeat>,
    #[serde(default)]
    pub capabilities: AgentCapabilitySnapshot,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServerHello {
    pub server_id: String,
    pub server_version: String,
    pub server_build_number: u64,
    pub accepted: bool,
    pub message: String,
    pub telemetry_interval_secs: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TelemetryEnvelope {
    pub client_id: String,
    pub metrics: AgentMetrics,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayAgentHelloIngest {
    pub gateway_id: String,
    pub gateway_session_id: Uuid,
    pub noise_public_key_hex: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_ip: Option<String>,
    pub hello: AgentHello,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentRuntimeConfigReloadRequest {
    pub client_id: String,
    pub current_content_hash: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub requires_authoritative_sync: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reconcile_resources: Vec<RuntimeConfigReconcileResource>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeConfigReconcileResource {
    RuntimeTunnels,
    PortForwarding,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RuntimeConfigReconcileScope {
    pub authoritative: bool,
    pub resources: BTreeSet<RuntimeConfigReconcileResource>,
}

impl RuntimeConfigReconcileScope {
    pub fn from_reload_request(request: &AgentRuntimeConfigReloadRequest) -> Self {
        let resources = request
            .reconcile_resources
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        Self {
            authoritative: request.requires_authoritative_sync,
            resources,
        }
    }

    pub fn requires_reconcile(&self) -> bool {
        self.authoritative || !self.resources.is_empty()
    }

    pub fn includes(&self, resource: RuntimeConfigReconcileResource) -> bool {
        self.authoritative || self.resources.contains(&resource)
    }

    pub fn covers(&self, requested: &Self) -> bool {
        self.authoritative
            || (!requested.authoritative && requested.resources.is_subset(&self.resources))
    }
}

pub fn runtime_config_reconcile_scope_from_reason(reason: &str) -> RuntimeConfigReconcileScope {
    let mut scope = RuntimeConfigReconcileScope::default();
    match reason {
        "agent_reconnect_authoritative_sync" => {
            scope.authoritative = true;
        }
        "agent_reconnect_authoritative_port_forwarding_sync" => {
            scope.authoritative = true;
            scope
                .resources
                .insert(RuntimeConfigReconcileResource::PortForwarding);
        }
        "port_forward_table_reapply"
        | "port_forward_bulk_reapply"
        | "agent_reconnect_port_forwarding_sync" => {
            scope
                .resources
                .insert(RuntimeConfigReconcileResource::PortForwarding);
        }
        "tunnel_plan_saved_enabled"
        | "tunnel_plan_updated"
        | "tunnel_plan_enabled"
        | "tunnel_plan_disabled"
        | "tunnel_plan_deleted"
        | "agent_reconnect_runtime_tunnels_sync" => {
            scope
                .resources
                .insert(RuntimeConfigReconcileResource::RuntimeTunnels);
        }
        "agent_reconnect_network_resources_sync" => {
            scope
                .resources
                .insert(RuntimeConfigReconcileResource::RuntimeTunnels);
            scope
                .resources
                .insert(RuntimeConfigReconcileResource::PortForwarding);
        }
        _ => {}
    }
    scope
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayRuntimeConfigReloadRequest {
    pub gateway_id: String,
    pub gateway_session_id: Uuid,
    pub process_incarnation_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_ip: Option<String>,
    pub request: AgentRuntimeConfigReloadRequest,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayTelemetryIngest {
    pub gateway_id: String,
    pub gateway_session_id: Uuid,
    pub process_incarnation_id: Uuid,
    pub telemetry_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_ip: Option<String>,
    pub telemetry: TelemetryEnvelope,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayCommandOutputIngest {
    pub gateway_id: String,
    pub gateway_session_id: Uuid,
    pub process_incarnation_id: Uuid,
    #[serde(default, skip_serializing_if = "is_false")]
    pub spooled_replay: bool,
    pub client_id: String,
    pub job_id: Uuid,
    pub payload_hash: String,
    pub seq: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_unix: Option<u64>,
    pub output: CommandOutput,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayTerminalOutputIngest {
    pub gateway_id: String,
    pub gateway_session_id: Uuid,
    pub process_incarnation_id: Uuid,
    #[serde(default, skip_serializing_if = "is_false")]
    pub spooled_replay: bool,
    pub client_id: String,
    pub output: TerminalStreamOutput,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentUpdateVerificationRequest {
    pub job_id: Uuid,
    pub version_url: String,
    pub artifact_url: String,
    pub asset_name: String,
    pub sha256_hex: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentUpdateVerificationResult {
    pub job_id: Uuid,
    pub approved: bool,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayAgentUpdateVerificationIngest {
    pub gateway_id: String,
    pub gateway_session_id: Uuid,
    pub process_incarnation_id: Uuid,
    pub client_id: String,
    pub request: AgentUpdateVerificationRequest,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewaySessionLifecycleIngest {
    pub gateway_id: String,
    pub client_id: String,
    pub session_id: Uuid,
    pub noise_public_key_hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_ip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_version: Option<String>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayCommandDispatch {
    pub client_id: String,
    pub request: JobRequest,
    pub expected_process_incarnation_id: Uuid,
    /// Captured before the durable DB eligibility read. A gateway restart
    /// invalidates that proof and forces the dispatcher to read DB again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_gateway_epoch: Option<Uuid>,
    pub payload_hash: String,
    /// An expired lifecycle lease forces the dispatcher back through its
    /// durable database eligibility check. The exact gateway owner returned
    /// by that rejection proves the retry happened after the lease gap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle_recheck: Option<GatewayClientDispatchFenceOwner>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayCommandCancel {
    pub client_id: String,
    pub request: JobCancelRequest,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayTerminalControl {
    pub client_id: String,
    pub expected_process_incarnation_id: Uuid,
    pub request: TerminalControlRequest,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayTerminalControlResult {
    pub client_id: String,
    pub ack: TerminalControlAck,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewaySessionDisconnect {
    pub client_id: String,
    pub reason: String,
    /// Lifecycle finalizers may disconnect only while their exact committed
    /// fence still owns the client. Generic operational disconnects omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_dispatch_fence_owner: Option<GatewayClientDispatchFenceOwner>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewaySessionDisconnectResult {
    pub client_id: String,
    pub accepted: bool,
    pub disconnected: bool,
    pub message: String,
}

pub const GATEWAY_CONTROL_BATCH_MAX_ITEMS: usize = 500;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewaySessionDisconnectBatchRequest {
    pub items: Vec<GatewaySessionDisconnect>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewaySessionDisconnectBatchResult {
    pub results: Vec<GatewaySessionDisconnectResult>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayClientDispatchFencePrepare {
    pub client_id: String,
    pub token: Uuid,
    pub gateway_epoch: Uuid,
    pub generation: u64,
    /// Initial installation is authorized by a separately acquired
    /// generation. Renewal/proof may only touch the already exact owner.
    #[serde(default)]
    pub renewal: bool,
    pub lease_secs: u64,
    pub purpose: GatewayClientDispatchFencePurpose,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayClientDispatchFenceAcquire {
    pub client_id: String,
    pub token: Uuid,
    pub purpose: GatewayClientDispatchFencePurpose,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GatewayClientDispatchFenceOwner {
    pub token: Uuid,
    pub gateway_epoch: Uuid,
    pub generation: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayClientDispatchFenceAcquireResult {
    pub client_id: String,
    pub owner: GatewayClientDispatchFenceOwner,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayClientDispatchFencePromote {
    pub client_id: String,
    pub token: Uuid,
    pub gateway_epoch: Uuid,
    pub generation: u64,
    pub purpose: GatewayClientDispatchFencePurpose,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayClientDispatchFenceClear {
    pub client_id: String,
    /// Every clear is exact-token scoped. A lifecycle transition can never
    /// erase a newer target owner.
    pub expected_token: Uuid,
    pub gateway_epoch: Uuid,
    pub expected_generation: u64,
    /// A rejected/rolled-back transition restores the persistent suspension
    /// fence it temporarily replaced; a committed transition removes it.
    #[serde(default)]
    pub restore_fallback: bool,
    pub reason: String,
}

pub const GATEWAY_CLIENT_DISPATCH_FENCE_BATCH_MAX_ITEMS: usize = 500;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayClientDispatchFencePurpose {
    Suspension,
    Deletion,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayClientDispatchFencePrepareBatchRequest {
    pub items: Vec<GatewayClientDispatchFencePrepare>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayClientDispatchFencePromoteBatchRequest {
    pub items: Vec<GatewayClientDispatchFencePromote>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayClientDispatchFenceClearBatchRequest {
    pub items: Vec<GatewayClientDispatchFenceClear>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayClientDispatchFenceResult {
    pub client_id: String,
    pub accepted: bool,
    pub fenced: bool,
    /// `true` only when prepare found and renewed the same active token and
    /// purpose. A fresh install cannot prove uninterrupted ownership to a
    /// database transaction that is about to commit.
    #[serde(default)]
    pub ownership_continuous: bool,
    pub message: String,
    /// Jobs whose command enqueue linearized before fence installation. The
    /// API must not relabel these as never dispatched.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enqueued_job_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayClientDispatchFenceBatchResult {
    pub results: Vec<GatewayClientDispatchFenceResult>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentSessionDisconnect {
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CommandResume {
    pub job_id: Uuid,
    pub command_version: u16,
    pub payload_hash: String,
    pub next_output_seq: i32,
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
pub struct GatewayPrivilegeVerificationBatchItem {
    /// Opaque correlation key supplied by the caller. It has no client or
    /// workflow semantics.
    pub request_id: String,
    pub verification: GatewayPrivilegeVerification,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayPrivilegeVerificationBatchRequest {
    pub items: Vec<GatewayPrivilegeVerificationBatchItem>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayPrivilegeVerificationBatchItemResult {
    pub request_id: String,
    pub approved: bool,
    pub intent_hash_hex: Option<String>,
    pub message: String,
    pub error_code: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayPrivilegeVerificationBatchResult {
    pub results: Vec<GatewayPrivilegeVerificationBatchItemResult>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayCommandDispatchResult {
    pub client_id: String,
    pub job_id: Uuid,
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TerminalControlAction {
    Input {
        data_base64: String,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    Close {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

impl TerminalControlAction {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Input { .. } => "input",
            Self::Resize { .. } => "resize",
            Self::Close { .. } => "close",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalControlRequest {
    pub request_id: Uuid,
    pub session_id: Uuid,
    pub action: TerminalControlAction,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TerminalControlAck {
    pub request_id: Uuid,
    pub session_id: Uuid,
    pub action: String,
    pub accepted: bool,
    pub status: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
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
    #[serde(default)]
    pub protocol_conflict: u64,
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
    pub rejected_agent_connections: u64,
    #[serde(default)]
    pub telemetry_admission_limit: u64,
    #[serde(default)]
    pub telemetry_admission_active: u64,
    #[serde(default)]
    pub telemetry_admission_waiting: u64,
    #[serde(default)]
    pub unhealthy: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobRequest {
    pub job_id: Uuid,
    pub command_version: u16,
    pub command: JobCommand,
    pub max_timeout_secs: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobCancelRequest {
    pub job_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

pub fn default_internal_build_number() -> u64 {
    1
}

pub fn job_command_variant_names() -> &'static [&'static str] {
    &[
        "shell",
        "shell_script",
        "terminal_open",
        "config_read",
        "runtime_config_sync",
        "agent_update",
        "agent_update_activate",
        "agent_update_rollback",
        "agent_update_check",
        "agent_stop",
        "agent_restart",
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
        "storage_inventory",
        "process_start",
        "process_stop",
        "process_restart",
        "process_status",
        "process_logs",
        "service_inventory",
        "service_action",
        "service_logs",
        "package_update_plan",
        "package_update_apply",
        "backup",
        "restore",
        "restore_rollback",
        "network_status",
        "network_interfaces",
        "network_traffic_import_vnstat",
        "network_probe",
        "network_speed_test",
        "network_routing_status",
        "network_routing_apply",
    ]
}

pub const JOB_COMMAND_SAFETY_READ: &str = "read";
pub const JOB_COMMAND_SAFETY_WRITE: &str = "write";
pub const JOB_COMMAND_SAFETY_EXEC: &str = "exec";
pub const JOB_COMMAND_SAFETY_EXCLUSIVE: &str = "exclusive";

pub const JOB_COMMAND_TYPE_LABELS: [&str; 56] = [
    "shell_argv",
    "shell_pty",
    "shell_script",
    "terminal_open",
    "config_read",
    "runtime_config_sync",
    "agent_update",
    "agent_update_activate",
    "agent_update_rollback",
    "agent_update_check",
    "agent_stop",
    "agent_restart",
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
    "storage_inventory",
    "process_start",
    "process_stop",
    "process_restart",
    "process_status",
    "process_logs",
    "service_inventory",
    "service_action",
    "service_logs",
    "package_update_plan",
    "package_update_apply",
    "backup",
    "restore",
    "restore_rollback",
    "network_status",
    "network_interfaces",
    "network_traffic_import_vnstat",
    "network_probe",
    "network_speed_test",
    "network_routing_status",
    "network_routing_apply",
];

pub const JOB_COMMAND_SAFETY_BY_OPERATION_TYPE: [(&str, &str); 55] = [
    ("shell", JOB_COMMAND_SAFETY_EXEC),
    ("shell_script", JOB_COMMAND_SAFETY_EXEC),
    ("terminal_open", JOB_COMMAND_SAFETY_EXEC),
    ("config_read", JOB_COMMAND_SAFETY_READ),
    ("runtime_config_sync", JOB_COMMAND_SAFETY_EXCLUSIVE),
    ("agent_update", JOB_COMMAND_SAFETY_EXCLUSIVE),
    ("agent_update_activate", JOB_COMMAND_SAFETY_EXCLUSIVE),
    ("agent_update_rollback", JOB_COMMAND_SAFETY_EXCLUSIVE),
    ("agent_update_check", JOB_COMMAND_SAFETY_EXCLUSIVE),
    ("agent_stop", JOB_COMMAND_SAFETY_EXCLUSIVE),
    ("agent_restart", JOB_COMMAND_SAFETY_EXCLUSIVE),
    ("file_pull", JOB_COMMAND_SAFETY_READ),
    ("file_push", JOB_COMMAND_SAFETY_WRITE),
    ("file_push_chunked", JOB_COMMAND_SAFETY_WRITE),
    ("file_transfer_start", JOB_COMMAND_SAFETY_WRITE),
    ("file_transfer_chunk", JOB_COMMAND_SAFETY_WRITE),
    ("file_transfer_commit", JOB_COMMAND_SAFETY_WRITE),
    ("file_transfer_abort", JOB_COMMAND_SAFETY_WRITE),
    ("file_transfer_download_start", JOB_COMMAND_SAFETY_READ),
    ("file_transfer_download_chunk", JOB_COMMAND_SAFETY_READ),
    ("file_stat", JOB_COMMAND_SAFETY_READ),
    ("file_list_dir", JOB_COMMAND_SAFETY_READ),
    ("file_read_text", JOB_COMMAND_SAFETY_READ),
    ("file_mkdir", JOB_COMMAND_SAFETY_WRITE),
    ("file_write_text", JOB_COMMAND_SAFETY_WRITE),
    ("file_rename", JOB_COMMAND_SAFETY_WRITE),
    ("file_delete", JOB_COMMAND_SAFETY_WRITE),
    ("file_chmod", JOB_COMMAND_SAFETY_WRITE),
    ("file_chown", JOB_COMMAND_SAFETY_WRITE),
    ("file_copy", JOB_COMMAND_SAFETY_WRITE),
    ("file_download", JOB_COMMAND_SAFETY_READ),
    ("file_archive_tar", JOB_COMMAND_SAFETY_READ),
    ("user_sessions", JOB_COMMAND_SAFETY_READ),
    ("process_list", JOB_COMMAND_SAFETY_READ),
    ("storage_inventory", JOB_COMMAND_SAFETY_READ),
    ("process_start", JOB_COMMAND_SAFETY_EXEC),
    ("process_stop", JOB_COMMAND_SAFETY_EXEC),
    ("process_restart", JOB_COMMAND_SAFETY_EXEC),
    ("process_status", JOB_COMMAND_SAFETY_READ),
    ("process_logs", JOB_COMMAND_SAFETY_READ),
    ("service_inventory", JOB_COMMAND_SAFETY_READ),
    ("service_action", JOB_COMMAND_SAFETY_EXEC),
    ("service_logs", JOB_COMMAND_SAFETY_READ),
    ("package_update_plan", JOB_COMMAND_SAFETY_EXEC),
    ("package_update_apply", JOB_COMMAND_SAFETY_EXCLUSIVE),
    ("backup", JOB_COMMAND_SAFETY_READ),
    ("restore", JOB_COMMAND_SAFETY_WRITE),
    ("restore_rollback", JOB_COMMAND_SAFETY_WRITE),
    ("network_status", JOB_COMMAND_SAFETY_READ),
    ("network_interfaces", JOB_COMMAND_SAFETY_READ),
    ("network_traffic_import_vnstat", JOB_COMMAND_SAFETY_WRITE),
    ("network_probe", JOB_COMMAND_SAFETY_READ),
    ("network_speed_test", JOB_COMMAND_SAFETY_EXEC),
    ("network_routing_status", JOB_COMMAND_SAFETY_READ),
    ("network_routing_apply", JOB_COMMAND_SAFETY_EXEC),
];

pub const JOB_COMMAND_CONFIRMATION_REQUIRED_BY_OPERATION_TYPE: [(&str, bool); 55] = [
    ("shell", true),
    ("shell_script", true),
    ("terminal_open", true),
    ("config_read", false),
    ("runtime_config_sync", true),
    ("agent_update", true),
    ("agent_update_activate", true),
    ("agent_update_rollback", true),
    ("agent_update_check", true),
    ("agent_stop", true),
    ("agent_restart", true),
    ("file_pull", false),
    ("file_push", true),
    ("file_push_chunked", true),
    ("file_transfer_start", true),
    ("file_transfer_chunk", true),
    ("file_transfer_commit", true),
    ("file_transfer_abort", true),
    ("file_transfer_download_start", false),
    ("file_transfer_download_chunk", false),
    ("file_stat", false),
    ("file_list_dir", false),
    ("file_read_text", false),
    ("file_mkdir", true),
    ("file_write_text", true),
    ("file_rename", true),
    ("file_delete", true),
    ("file_chmod", true),
    ("file_chown", true),
    ("file_copy", true),
    ("file_download", false),
    ("file_archive_tar", false),
    ("user_sessions", false),
    ("process_list", false),
    ("storage_inventory", false),
    ("process_start", true),
    ("process_stop", true),
    ("process_restart", true),
    ("process_status", false),
    ("process_logs", false),
    ("service_inventory", false),
    ("service_action", true),
    ("service_logs", false),
    ("package_update_plan", false),
    ("package_update_apply", true),
    ("backup", true),
    ("restore", true),
    ("restore_rollback", true),
    ("network_status", false),
    ("network_interfaces", false),
    ("network_traffic_import_vnstat", true),
    ("network_probe", false),
    ("network_speed_test", true),
    ("network_routing_status", false),
    ("network_routing_apply", true),
];

pub const JOB_COMMAND_TYPE_BY_OPERATION_TYPE: [(&str, &str); 55] = [
    ("shell", "shell_argv"),
    ("shell_script", "shell_script"),
    ("terminal_open", "terminal_open"),
    ("config_read", "config_read"),
    ("runtime_config_sync", "runtime_config_sync"),
    ("agent_update", "agent_update"),
    ("agent_update_activate", "agent_update_activate"),
    ("agent_update_rollback", "agent_update_rollback"),
    ("agent_update_check", "agent_update_check"),
    ("agent_stop", "agent_stop"),
    ("agent_restart", "agent_restart"),
    ("file_pull", "file_pull"),
    ("file_push", "file_push"),
    ("file_push_chunked", "file_push_chunked"),
    ("file_transfer_start", "file_transfer_start"),
    ("file_transfer_chunk", "file_transfer_chunk"),
    ("file_transfer_commit", "file_transfer_commit"),
    ("file_transfer_abort", "file_transfer_abort"),
    (
        "file_transfer_download_start",
        "file_transfer_download_start",
    ),
    (
        "file_transfer_download_chunk",
        "file_transfer_download_chunk",
    ),
    ("file_stat", "file_stat"),
    ("file_list_dir", "file_list_dir"),
    ("file_read_text", "file_read_text"),
    ("file_mkdir", "file_mkdir"),
    ("file_write_text", "file_write_text"),
    ("file_rename", "file_rename"),
    ("file_delete", "file_delete"),
    ("file_chmod", "file_chmod"),
    ("file_chown", "file_chown"),
    ("file_copy", "file_copy"),
    ("file_download", "file_download"),
    ("file_archive_tar", "file_archive_tar"),
    ("user_sessions", "user_sessions"),
    ("process_list", "process_list"),
    ("storage_inventory", "storage_inventory"),
    ("process_start", "process_start"),
    ("process_stop", "process_stop"),
    ("process_restart", "process_restart"),
    ("process_status", "process_status"),
    ("process_logs", "process_logs"),
    ("service_inventory", "service_inventory"),
    ("service_action", "service_action"),
    ("service_logs", "service_logs"),
    ("package_update_plan", "package_update_plan"),
    ("package_update_apply", "package_update_apply"),
    ("backup", "backup"),
    ("restore", "restore"),
    ("restore_rollback", "restore_rollback"),
    ("network_status", "network_status"),
    ("network_interfaces", "network_interfaces"),
    (
        "network_traffic_import_vnstat",
        "network_traffic_import_vnstat",
    ),
    ("network_probe", "network_probe"),
    ("network_speed_test", "network_speed_test"),
    ("network_routing_status", "network_routing_status"),
    ("network_routing_apply", "network_routing_apply"),
];

pub const JOB_COMMAND_DISPLAY_GROUP_BY_COMMAND_TYPE: [(&str, &str); 56] = [
    ("shell_argv", "shell"),
    ("shell_pty", "shell"),
    ("shell_script", "shell"),
    ("terminal_open", "terminal"),
    ("config_read", "config"),
    ("runtime_config_sync", "config"),
    ("agent_update", "agent_update"),
    ("agent_update_activate", "agent_update"),
    ("agent_update_rollback", "agent_update"),
    ("agent_update_check", "agent_update"),
    ("agent_stop", "agent_lifecycle"),
    ("agent_restart", "agent_lifecycle"),
    ("file_pull", "file"),
    ("file_push", "file"),
    ("file_push_chunked", "file"),
    ("file_transfer_start", "file_transfer"),
    ("file_transfer_chunk", "file_transfer"),
    ("file_transfer_commit", "file_transfer"),
    ("file_transfer_abort", "file_transfer"),
    ("file_transfer_download_start", "file_transfer"),
    ("file_transfer_download_chunk", "file_transfer"),
    ("file_stat", "file"),
    ("file_list_dir", "file"),
    ("file_read_text", "file"),
    ("file_mkdir", "file"),
    ("file_write_text", "file"),
    ("file_rename", "file"),
    ("file_delete", "file"),
    ("file_chmod", "file"),
    ("file_chown", "file"),
    ("file_copy", "file"),
    ("file_download", "file"),
    ("file_archive_tar", "file"),
    ("user_sessions", "inventory"),
    ("process_list", "process"),
    ("storage_inventory", "storage"),
    ("process_start", "process"),
    ("process_stop", "process"),
    ("process_restart", "process"),
    ("process_status", "process"),
    ("process_logs", "process"),
    ("service_inventory", "service"),
    ("service_action", "service"),
    ("service_logs", "service"),
    ("package_update_plan", "os_update"),
    ("package_update_apply", "os_update"),
    ("backup", "backup"),
    ("restore", "restore"),
    ("restore_rollback", "restore"),
    ("network_status", "network"),
    ("network_interfaces", "network"),
    ("network_traffic_import_vnstat", "network"),
    ("network_probe", "network"),
    ("network_speed_test", "network"),
    ("network_routing_status", "network"),
    ("network_routing_apply", "network"),
];

pub fn job_command_type_labels() -> &'static [&'static str] {
    &JOB_COMMAND_TYPE_LABELS
}

pub fn job_command_safety_by_operation_type() -> &'static [(&'static str, &'static str)] {
    &JOB_COMMAND_SAFETY_BY_OPERATION_TYPE
}

pub fn job_command_confirmation_required_by_operation_type() -> &'static [(&'static str, bool)] {
    &JOB_COMMAND_CONFIRMATION_REQUIRED_BY_OPERATION_TYPE
}

pub fn job_command_type_by_operation_type() -> &'static [(&'static str, &'static str)] {
    &JOB_COMMAND_TYPE_BY_OPERATION_TYPE
}

pub fn job_command_display_group_by_command_type() -> &'static [(&'static str, &'static str)] {
    &JOB_COMMAND_DISPLAY_GROUP_BY_COMMAND_TYPE
}

pub fn job_command_requires_confirmation_by_operation_type(operation_type: &str) -> Option<bool> {
    JOB_COMMAND_CONFIRMATION_REQUIRED_BY_OPERATION_TYPE
        .iter()
        .find_map(|(candidate, required)| (*candidate == operation_type).then_some(*required))
}

pub fn job_command_type_label_from_operation_type(operation_type: &str) -> Option<&'static str> {
    JOB_COMMAND_TYPE_BY_OPERATION_TYPE
        .iter()
        .find_map(|(candidate, label)| (*candidate == operation_type).then_some(*label))
}

pub fn job_command_display_group(command_type: &str) -> Option<&'static str> {
    JOB_COMMAND_DISPLAY_GROUP_BY_COMMAND_TYPE
        .iter()
        .find_map(|(candidate, group)| (*candidate == command_type).then_some(*group))
}

pub fn job_privilege_intent_fields() -> &'static [&'static str] {
    &[
        "version",
        "action",
        "selector_expression",
        "command_type",
        "operation_payload_hash",
        "rollout_policy_hash",
        "resolved_targets",
        "max_timeout_secs",
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
        "max_timeout_secs",
        "force_unprivileged",
        "privileged",
        "privilege_assertion",
        "rollout",
    ]
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JobRolloutPolicy {
    pub canary_client_ids: Vec<String>,
    pub batch_size: u16,
    pub max_failures: u16,
    pub pause_after_canary: bool,
    pub batch_delay_secs: u32,
}

pub fn schedule_privilege_intent_fields() -> &'static [&'static str] {
    &[
        "version",
        "action",
        "schedule_id",
        "definition_revision",
        "name",
        "command_type",
        "operation_payload_hash",
        "selector_expression",
        "resolved_targets",
        "trigger_kind",
        "cron_expr",
        "timezone",
        "event_expression",
        "enabled",
        "catch_up_policy",
        "catch_up_limit",
        "retry_delay_secs",
        "max_failures",
        "deferred_until",
        "deleted",
    ]
}

pub fn db_privilege_intent_fields() -> &'static [&'static str] {
    &[
        "version",
        "action",
        "target",
        "selector_expression",
        "resolved_targets",
        "confirmed",
        "payload_hash",
    ]
}

#[derive(Serialize)]
pub struct JobPrivilegeIntent<'a> {
    version: u8,
    action: &'static str,
    selector_expression: &'a str,
    command_type: &'a str,
    operation_payload_hash: &'a str,
    rollout_policy_hash: Option<&'a str>,
    resolved_targets: Vec<&'a str>,
    max_timeout_secs: u64,
    force_unprivileged: bool,
    privileged: bool,
}

impl<'a> JobPrivilegeIntent<'a> {
    pub fn new(input: JobPrivilegeIntentInput<'a>) -> Self {
        Self {
            version: 1,
            action: "job.dispatch",
            selector_expression: input.selector_expression.trim(),
            command_type: input.command_type,
            operation_payload_hash: input.operation_payload_hash,
            rollout_policy_hash: input.rollout_policy_hash,
            resolved_targets: sorted_str_refs(input.resolved_targets),
            max_timeout_secs: input.max_timeout_secs.max(1),
            force_unprivileged: input.force_unprivileged,
            privileged: input.privileged,
        }
    }
}

pub struct JobPrivilegeIntentInput<'a> {
    pub selector_expression: &'a str,
    pub command_type: &'a str,
    pub operation_payload_hash: &'a str,
    pub rollout_policy_hash: Option<&'a str>,
    pub resolved_targets: &'a [String],
    pub max_timeout_secs: u64,
    pub force_unprivileged: bool,
    pub privileged: bool,
}

#[derive(Serialize)]
pub struct SchedulePrivilegeIntent<'a> {
    version: u8,
    action: &'a str,
    schedule_id: Option<&'a str>,
    definition_revision: Option<i64>,
    name: &'a str,
    command_type: &'a str,
    operation_payload_hash: &'a str,
    selector_expression: &'a str,
    resolved_targets: Vec<&'a str>,
    trigger_kind: &'a str,
    cron_expr: Option<&'a str>,
    timezone: Option<&'a str>,
    event_expression: Option<&'a str>,
    enabled: bool,
    catch_up_policy: Option<&'a str>,
    catch_up_limit: Option<i32>,
    retry_delay_secs: Option<i64>,
    max_failures: i32,
    deferred_until: Option<&'a str>,
    deleted: bool,
}

impl<'a> SchedulePrivilegeIntent<'a> {
    pub fn new(input: SchedulePrivilegeIntentInput<'a>) -> Self {
        Self {
            version: 2,
            action: input.action,
            schedule_id: input.schedule_id,
            definition_revision: input.definition_revision,
            name: input.name.trim(),
            command_type: input.command_type,
            operation_payload_hash: input.operation_payload_hash,
            selector_expression: input.selector_expression.trim(),
            resolved_targets: sorted_str_refs(input.resolved_targets),
            trigger_kind: input.trigger_kind,
            cron_expr: input.cron_expr.map(str::trim),
            timezone: input.timezone,
            event_expression: input.event_expression.map(str::trim),
            enabled: input.enabled,
            catch_up_policy: input.catch_up_policy,
            catch_up_limit: input.catch_up_limit,
            retry_delay_secs: input.retry_delay_secs,
            max_failures: input.max_failures,
            deferred_until: input.deferred_until,
            deleted: input.deleted,
        }
    }
}

pub struct SchedulePrivilegeIntentInput<'a> {
    pub action: &'a str,
    pub schedule_id: Option<&'a str>,
    pub definition_revision: Option<i64>,
    pub name: &'a str,
    pub command_type: &'a str,
    pub operation_payload_hash: &'a str,
    pub selector_expression: &'a str,
    pub resolved_targets: &'a [String],
    pub trigger_kind: &'a str,
    pub cron_expr: Option<&'a str>,
    pub timezone: Option<&'a str>,
    pub event_expression: Option<&'a str>,
    pub enabled: bool,
    pub catch_up_policy: Option<&'a str>,
    pub catch_up_limit: Option<i32>,
    pub retry_delay_secs: Option<i64>,
    pub max_failures: i32,
    pub deferred_until: Option<&'a str>,
    pub deleted: bool,
}

#[derive(Serialize)]
pub struct DbPrivilegeIntent<'a> {
    version: u8,
    action: &'a str,
    target: &'a str,
    selector_expression: Option<&'a str>,
    resolved_targets: Vec<&'a str>,
    confirmed: bool,
    payload_hash: Option<&'a str>,
}

impl<'a> DbPrivilegeIntent<'a> {
    pub fn new(
        action: &'a str,
        target: &'a str,
        selector_expression: Option<&'a str>,
        resolved_targets: &'a [String],
        confirmed: bool,
        payload_hash: Option<&'a str>,
    ) -> Self {
        Self {
            version: 1,
            action,
            target,
            selector_expression: selector_expression.map(str::trim),
            resolved_targets: sorted_str_refs(resolved_targets),
            confirmed,
            payload_hash: payload_hash.map(str::trim),
        }
    }
}

pub struct OperatorDbPayloadInput<'a> {
    pub action: &'a str,
    pub target: &'a str,
    pub username: Option<&'a str>,
    pub role: Option<&'a str>,
    pub scopes: &'a [String],
    pub session_refresh_ttl_secs: Option<u64>,
    pub status: Option<&'a str>,
    pub admin_risk_acknowledged: bool,
}

pub struct AgentIdentityPayloadInput<'a> {
    pub client_id: &'a str,
    pub public_key: &'a [u8],
    pub display_name: Option<&'a str>,
    pub tags: &'a [String],
    pub replace_existing_key: bool,
}

#[derive(Serialize)]
struct OperatorDbPayload<'a> {
    version: u8,
    action: &'a str,
    target: &'a str,
    username: Option<&'a str>,
    role: Option<&'a str>,
    scopes: Vec<&'a str>,
    session_refresh_ttl_secs: Option<u64>,
    status: Option<&'a str>,
    admin_risk_acknowledged: bool,
}

impl<'a> OperatorDbPayload<'a> {
    fn new(input: OperatorDbPayloadInput<'a>) -> Self {
        Self {
            version: 1,
            action: input.action,
            target: input.target,
            username: input.username.map(str::trim),
            role: input.role.map(str::trim),
            scopes: sorted_str_refs(input.scopes),
            session_refresh_ttl_secs: input.session_refresh_ttl_secs,
            status: input.status.map(str::trim),
            admin_risk_acknowledged: input.admin_risk_acknowledged,
        }
    }
}

pub fn canonical_job_privilege_intent(
    input: JobPrivilegeIntentInput<'_>,
) -> serde_json::Result<String> {
    serde_json::to_string(&JobPrivilegeIntent::new(input))
}

pub fn canonical_schedule_privilege_intent(
    input: SchedulePrivilegeIntentInput<'_>,
) -> serde_json::Result<String> {
    serde_json::to_string(&SchedulePrivilegeIntent::new(input))
}

pub fn canonical_db_privilege_intent(
    action: &str,
    target: &str,
    selector_expression: Option<&str>,
    resolved_targets: &[String],
    confirmed: bool,
    payload_hash: Option<&str>,
) -> serde_json::Result<String> {
    serde_json::to_string(&DbPrivilegeIntent::new(
        action,
        target,
        selector_expression,
        resolved_targets,
        confirmed,
        payload_hash,
    ))
}

pub fn operator_db_payload_hash(input: OperatorDbPayloadInput<'_>) -> serde_json::Result<String> {
    let payload = serde_json::to_string(&OperatorDbPayload::new(input))?;
    Ok(crate::auth::payload_hash(payload.as_bytes()))
}

pub fn agent_identity_payload_hash(input: AgentIdentityPayloadInput<'_>) -> String {
    let mut tags = input
        .tags
        .iter()
        .map(|tag| tag.trim())
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>();
    tags.sort_unstable();
    tags.dedup();
    let payload = serde_json::json!([
        ["version", serde_json::json!(1)],
        ["client_id", serde_json::json!(input.client_id.trim())],
        [
            "public_key_sha256_hex",
            serde_json::json!(crate::auth::payload_hash(input.public_key))
        ],
        [
            "display_name",
            serde_json::json!(input
                .display_name
                .map(str::trim)
                .filter(|value| !value.is_empty()))
        ],
        ["tags", serde_json::json!(tags)],
        [
            "replace_existing_key",
            serde_json::json!(input.replace_existing_key)
        ],
    ]);
    crate::auth::payload_hash(
        &serde_json::to_vec(&payload).expect("agent identity payload serializes"),
    )
}

fn sorted_str_refs(values: &[String]) -> Vec<&str> {
    let mut values = values.iter().map(String::as_str).collect::<Vec<_>>();
    values.sort_unstable();
    values
}

pub const TERMINAL_COMMAND_TYPES: &[&str] = &["terminal_open"];

pub const TERMINAL_SESSION_EVENTS: &[&str] = &[
    "terminal_open",
    "terminal_input",
    "terminal_resize",
    "terminal_close",
    "terminal_stream",
];

pub const TERMINAL_SESSION_STATUSES: &[&str] = &[
    "opening",
    "opened",
    "attached",
    "rejected",
    "failed",
    "accepted",
    "resized",
    "closed",
    "missing",
    "streaming",
    "exited",
    "idle_timeout",
    "disconnected_timeout",
    "lifecycle_disconnected",
];

pub const TERMINAL_SESSION_STATES: &[&str] = &[
    "opening", "open", "closed", "missing", "rejected", "failed", "exited",
];

pub const FILE_TRANSFER_COMMAND_TYPES: &[&str] = &[
    "file_transfer_start",
    "file_transfer_chunk",
    "file_transfer_commit",
    "file_transfer_abort",
    "file_transfer_download_start",
    "file_transfer_download_chunk",
];

pub const FILE_TRANSFER_DIRECTIONS: &[&str] = &["upload", "download"];

pub const FILE_TRANSFER_SESSION_EVENTS: &[&str] = &[
    "file_transfer_start",
    "file_transfer_chunk_ack",
    "file_transfer_commit",
    "file_transfer_abort",
    "file_transfer_download_start",
    "file_transfer_download_chunk",
];

pub const FILE_TRANSFER_SESSION_STATUSES: &[&str] =
    &["started", "transferring", "completed", "aborted", "unknown"];

pub const BACKUP_REQUEST_STATUSES: &[&str] = &[
    "requested_metadata_only",
    "artifact_metadata_recorded",
    "execution_failed",
    "execution_canceled",
];
pub const RESTORE_PLAN_STATUSES: &[&str] = &["planned_metadata_only"];
pub const MIGRATION_LINK_STATUSES: &[&str] = &["linked_metadata_only"];
pub const AGENT_UPDATE_RELEASE_STATUSES: &[&str] = &["published_external"];

pub const SERVER_JOB_TYPE_ARTIFACT_CLEANUP: &str = "artifact_cleanup";
pub const SERVER_JOB_STATUS_QUEUED: &str = "queued";
pub const SERVER_JOB_STATUS_RUNNING: &str = "running";
pub const SERVER_JOB_STATUS_COMPLETED: &str = "completed";
pub const SERVER_JOB_STATUS_FAILED: &str = "failed";
pub const SERVER_JOB_STATUS_CANCELED: &str = "canceled";
pub const ARTIFACT_CLEANUP_RUNNING_TIMEOUT_SECS: i64 = 6 * 60 * 60;
pub const MAX_ARTIFACT_CLEANUP_REVIEWED_TARGETS: usize = 10_000;
pub const SERVER_JOB_TYPES: &[&str] = &[SERVER_JOB_TYPE_ARTIFACT_CLEANUP];
pub const SERVER_JOB_STATUSES: &[&str] = &[
    SERVER_JOB_STATUS_QUEUED,
    SERVER_JOB_STATUS_RUNNING,
    SERVER_JOB_STATUS_COMPLETED,
    SERVER_JOB_STATUS_FAILED,
    SERVER_JOB_STATUS_CANCELED,
];

pub const FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_QUEUED: &str = "queued";
pub const FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_IN_PROGRESS: &str = "in_progress";
pub const FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_FAILED: &str = "failed";
pub const FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_PERMANENTLY_FAILED: &str = "permanently_failed";
pub const FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_CANCELED_DISABLED: &str = "canceled_disabled";
pub const FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_DELIVERED: &str = "delivered";
pub const FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_MATCHED_DRY_RUN: &str = "matched_dry_run";
pub const FLEET_ALERT_NOTIFICATION_DELIVERY_STATUSES: &[&str] = &[
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_QUEUED,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_IN_PROGRESS,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_FAILED,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_PERMANENTLY_FAILED,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_CANCELED_DISABLED,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_DELIVERED,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_MATCHED_DRY_RUN,
];
pub const FLEET_ALERT_NOTIFICATION_DELIVERY_PROCESS_STATUSES: &[&str] = &[
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_QUEUED,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_FAILED,
];

pub const WEBHOOK_RULE_DELIVERY_STATUS_QUEUED: &str = "queued";
pub const WEBHOOK_RULE_DELIVERY_STATUS_IN_PROGRESS: &str = "in_progress";
pub const WEBHOOK_RULE_DELIVERY_STATUS_FAILED: &str = "failed";
pub const WEBHOOK_RULE_DELIVERY_STATUS_PERMANENTLY_FAILED: &str = "permanently_failed";
pub const WEBHOOK_RULE_DELIVERY_STATUS_CANCELED_DISABLED: &str = "canceled_disabled";
pub const WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED: &str = "delivered";
pub const WEBHOOK_RULE_DELIVERY_STATUS_MATCHED_DRY_RUN: &str = "matched_dry_run";
pub const WEBHOOK_RULE_DELIVERY_STATUSES: &[&str] = &[
    WEBHOOK_RULE_DELIVERY_STATUS_QUEUED,
    WEBHOOK_RULE_DELIVERY_STATUS_IN_PROGRESS,
    WEBHOOK_RULE_DELIVERY_STATUS_FAILED,
    WEBHOOK_RULE_DELIVERY_STATUS_PERMANENTLY_FAILED,
    WEBHOOK_RULE_DELIVERY_STATUS_CANCELED_DISABLED,
    WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED,
    WEBHOOK_RULE_DELIVERY_STATUS_MATCHED_DRY_RUN,
];
pub const WEBHOOK_RULE_DELIVERY_HISTORY_STATUSES: &[&str] = &[
    WEBHOOK_RULE_DELIVERY_STATUS_QUEUED,
    WEBHOOK_RULE_DELIVERY_STATUS_IN_PROGRESS,
    WEBHOOK_RULE_DELIVERY_STATUS_FAILED,
    WEBHOOK_RULE_DELIVERY_STATUS_PERMANENTLY_FAILED,
    WEBHOOK_RULE_DELIVERY_STATUS_CANCELED_DISABLED,
    WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED,
];
pub const WEBHOOK_RULE_DELIVERY_PROCESS_STATUSES: &[&str] = &[
    WEBHOOK_RULE_DELIVERY_STATUS_QUEUED,
    WEBHOOK_RULE_DELIVERY_STATUS_FAILED,
];

pub const TOPOLOGY_NODE_STATUSES: &[&str] = &["online", "offline", "never", "stale", "unknown"];
pub const TOPOLOGY_EDGE_HEALTH_STATUSES: &[&str] = &["disabled", "unknown", "healthy", "degraded"];
pub const TOPOLOGY_NEIGHBOR_STATES: &[&str] = &[
    "unknown",
    "healthy",
    "kernel_probe_success",
    "kernel_probe_failed",
    "not_probed",
];
pub const TOPOLOGY_PROBE_STATES: &[&str] = &["unknown", "success", "failed", "skipped"];
pub const TOPOLOGY_RUNTIME_STATES: &[&str] = &[
    "unknown",
    "adapter_unhealthy",
    "routing_unhealthy",
    "drift",
    "unhealthy",
    "degraded",
    "observed",
    "healthy",
    "not_applicable",
    "not_configured",
    "skipped",
];
pub const TOPOLOGY_OBSERVATION_STATES: &[&str] = &["unknown", "healthy", "degraded", "recorded"];

pub fn terminal_command_types() -> &'static [&'static str] {
    TERMINAL_COMMAND_TYPES
}

pub fn terminal_session_events() -> &'static [&'static str] {
    TERMINAL_SESSION_EVENTS
}

pub fn terminal_session_statuses() -> &'static [&'static str] {
    TERMINAL_SESSION_STATUSES
}

pub fn terminal_session_states() -> &'static [&'static str] {
    TERMINAL_SESSION_STATES
}

pub fn file_transfer_command_types() -> &'static [&'static str] {
    FILE_TRANSFER_COMMAND_TYPES
}

pub fn file_transfer_directions() -> &'static [&'static str] {
    FILE_TRANSFER_DIRECTIONS
}

pub fn file_transfer_session_events() -> &'static [&'static str] {
    FILE_TRANSFER_SESSION_EVENTS
}

pub fn file_transfer_session_statuses() -> &'static [&'static str] {
    FILE_TRANSFER_SESSION_STATUSES
}

pub fn backup_request_statuses() -> &'static [&'static str] {
    BACKUP_REQUEST_STATUSES
}

pub fn restore_plan_statuses() -> &'static [&'static str] {
    RESTORE_PLAN_STATUSES
}

pub fn migration_link_statuses() -> &'static [&'static str] {
    MIGRATION_LINK_STATUSES
}

pub fn agent_update_release_statuses() -> &'static [&'static str] {
    AGENT_UPDATE_RELEASE_STATUSES
}

pub fn server_job_types() -> &'static [&'static str] {
    SERVER_JOB_TYPES
}

pub fn server_job_statuses() -> &'static [&'static str] {
    SERVER_JOB_STATUSES
}

pub fn fleet_alert_notification_delivery_statuses() -> &'static [&'static str] {
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUSES
}

pub fn fleet_alert_notification_delivery_process_statuses() -> &'static [&'static str] {
    FLEET_ALERT_NOTIFICATION_DELIVERY_PROCESS_STATUSES
}

pub fn webhook_rule_delivery_statuses() -> &'static [&'static str] {
    WEBHOOK_RULE_DELIVERY_STATUSES
}

pub fn webhook_rule_delivery_history_statuses() -> &'static [&'static str] {
    WEBHOOK_RULE_DELIVERY_HISTORY_STATUSES
}

pub fn webhook_rule_delivery_process_statuses() -> &'static [&'static str] {
    WEBHOOK_RULE_DELIVERY_PROCESS_STATUSES
}

pub fn topology_node_statuses() -> &'static [&'static str] {
    TOPOLOGY_NODE_STATUSES
}

pub fn topology_edge_health_statuses() -> &'static [&'static str] {
    TOPOLOGY_EDGE_HEALTH_STATUSES
}

pub fn topology_neighbor_states() -> &'static [&'static str] {
    TOPOLOGY_NEIGHBOR_STATES
}

pub fn topology_probe_states() -> &'static [&'static str] {
    TOPOLOGY_PROBE_STATES
}

pub fn topology_runtime_states() -> &'static [&'static str] {
    TOPOLOGY_RUNTIME_STATES
}

pub fn topology_observation_states() -> &'static [&'static str] {
    TOPOLOGY_OBSERVATION_STATES
}

fn contains_static(values: &'static [&'static str], value: &str) -> bool {
    values.contains(&value)
}

pub fn is_terminal_command_type(command_type: &str) -> bool {
    contains_static(TERMINAL_COMMAND_TYPES, command_type)
}

pub fn is_terminal_session_event(event_type: &str) -> bool {
    contains_static(TERMINAL_SESSION_EVENTS, event_type)
}

pub fn terminal_session_state(
    event_type: &str,
    status: &str,
    session_exited: bool,
) -> &'static str {
    match (event_type, status) {
        ("terminal_open", "opening") => "opening",
        ("terminal_close", "closed") => "closed",
        ("terminal_close", "missing") | (_, "missing") => "missing",
        ("terminal_open", "rejected") => "rejected",
        ("terminal_open", "failed") => "failed",
        _ if session_exited => "exited",
        ("terminal_open", "opened" | "attached") => "open",
        ("terminal_input", "accepted") => "open",
        ("terminal_resize", "resized") => "open",
        ("terminal_stream", "streaming") => "open",
        (
            "terminal_stream",
            "closed"
            | "exited"
            | "idle_timeout"
            | "disconnected_timeout"
            | "lifecycle_disconnected",
        ) => "closed",
        _ => "unknown",
    }
}

pub fn is_file_transfer_command_type(command_type: &str) -> bool {
    contains_static(FILE_TRANSFER_COMMAND_TYPES, command_type)
}

pub fn is_file_transfer_session_event(event_type: &str) -> bool {
    contains_static(FILE_TRANSFER_SESSION_EVENTS, event_type)
}

pub fn file_transfer_session_status(event_type: &str, download_complete: bool) -> &'static str {
    match event_type {
        "file_transfer_commit" => "completed",
        "file_transfer_abort" => "aborted",
        "file_transfer_download_chunk" if download_complete => "completed",
        "file_transfer_chunk_ack" | "file_transfer_download_chunk" => "transferring",
        "file_transfer_start" | "file_transfer_download_start" => "started",
        _ => "unknown",
    }
}

pub fn is_topology_node_status(status: &str) -> bool {
    contains_static(TOPOLOGY_NODE_STATUSES, status)
}

pub fn is_topology_edge_health_status(status: &str) -> bool {
    contains_static(TOPOLOGY_EDGE_HEALTH_STATUSES, status)
}

pub fn is_topology_neighbor_state(status: &str) -> bool {
    contains_static(TOPOLOGY_NEIGHBOR_STATES, status)
}

pub fn is_topology_probe_state(status: &str) -> bool {
    contains_static(TOPOLOGY_PROBE_STATES, status)
}

pub fn is_topology_runtime_state(status: &str) -> bool {
    contains_static(TOPOLOGY_RUNTIME_STATES, status)
}

pub fn is_topology_observation_state(status: &str) -> bool {
    contains_static(TOPOLOGY_OBSERVATION_STATES, status)
}

pub fn is_server_job_type(job_type: &str) -> bool {
    contains_static(SERVER_JOB_TYPES, job_type)
}

pub fn is_server_job_status(status: &str) -> bool {
    contains_static(SERVER_JOB_STATUSES, status)
}

pub fn is_fleet_alert_notification_delivery_status(status: &str) -> bool {
    contains_static(FLEET_ALERT_NOTIFICATION_DELIVERY_STATUSES, status)
}

pub fn is_fleet_alert_notification_delivery_process_status(status: &str) -> bool {
    contains_static(FLEET_ALERT_NOTIFICATION_DELIVERY_PROCESS_STATUSES, status)
}

pub fn is_webhook_rule_delivery_status(status: &str) -> bool {
    contains_static(WEBHOOK_RULE_DELIVERY_STATUSES, status)
}

pub fn is_webhook_rule_delivery_history_status(status: &str) -> bool {
    contains_static(WEBHOOK_RULE_DELIVERY_HISTORY_STATUSES, status)
}

pub fn is_webhook_rule_delivery_process_status(status: &str) -> bool {
    contains_static(WEBHOOK_RULE_DELIVERY_PROCESS_STATUSES, status)
}

pub fn normalize_topology_runtime_state(value: &str) -> &'static str {
    match value {
        "adapter_unhealthy" => "adapter_unhealthy",
        "routing_unhealthy" => "routing_unhealthy",
        "drift" => "drift",
        "unhealthy" => "unhealthy",
        "degraded" => "degraded",
        "observed" => "observed",
        "healthy" => "healthy",
        "not_applicable" => "not_applicable",
        "not_configured" => "not_configured",
        "skipped" => "skipped",
        _ => "unknown",
    }
}

pub fn topology_runtime_state_rank(value: &str) -> u8 {
    match value {
        "adapter_unhealthy" | "routing_unhealthy" | "drift" | "unhealthy" => 5,
        "degraded" => 4,
        "observed" | "unknown" => 3,
        "healthy" => 2,
        "not_applicable" | "not_configured" | "skipped" => 1,
        _ => 0,
    }
}

pub fn aggregate_topology_runtime_state(current: &str, next: &str) -> &'static str {
    if current == "unknown" && next != "unknown" {
        return normalize_topology_runtime_state(next);
    }
    if next == "unknown" {
        return normalize_topology_runtime_state(current);
    }
    if topology_runtime_state_rank(next) >= topology_runtime_state_rank(current) {
        normalize_topology_runtime_state(next)
    } else {
        normalize_topology_runtime_state(current)
    }
}

pub fn topology_runtime_state_is_degraded(value: &str) -> bool {
    matches!(
        value,
        "adapter_unhealthy" | "routing_unhealthy" | "drift" | "unhealthy" | "degraded"
    )
}

pub fn normalize_topology_probe_state(value: &str) -> &'static str {
    match value {
        "failed" => "failed",
        "success" => "success",
        "skipped" => "skipped",
        _ => "unknown",
    }
}

pub fn topology_probe_state_rank(value: &str) -> u8 {
    match value {
        "failed" => 4,
        "success" => 3,
        "skipped" => 2,
        "unknown" => 1,
        _ => 0,
    }
}

pub fn aggregate_topology_probe_state(current: &str, next: &str) -> &'static str {
    if topology_probe_state_rank(next) >= topology_probe_state_rank(current) {
        normalize_topology_probe_state(next)
    } else {
        normalize_topology_probe_state(current)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackupRequestStatus {
    RequestedMetadataOnly,
    ArtifactMetadataRecorded,
    ExecutionFailed,
    ExecutionCanceled,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupMissingPathPolicy {
    #[default]
    Fail,
    Skip,
}

impl BackupMissingPathPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fail => "fail",
            Self::Skip => "skip",
        }
    }

    pub fn from_storage(value: &str) -> Option<Self> {
        match value {
            "fail" => Some(Self::Fail),
            "skip" => Some(Self::Skip),
            _ => None,
        }
    }
}

impl BackupRequestStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RequestedMetadataOnly => "requested_metadata_only",
            Self::ArtifactMetadataRecorded => "artifact_metadata_recorded",
            Self::ExecutionFailed => "execution_failed",
            Self::ExecutionCanceled => "execution_canceled",
        }
    }

    pub fn from_storage(value: &str) -> Option<Self> {
        match value {
            "requested_metadata_only" => Some(Self::RequestedMetadataOnly),
            "artifact_metadata_recorded" => Some(Self::ArtifactMetadataRecorded),
            "execution_failed" => Some(Self::ExecutionFailed),
            "execution_canceled" => Some(Self::ExecutionCanceled),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestorePlanStatus {
    PlannedMetadataOnly,
}

impl RestorePlanStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PlannedMetadataOnly => "planned_metadata_only",
        }
    }

    pub fn from_storage(value: &str) -> Option<Self> {
        match value {
            "planned_metadata_only" => Some(Self::PlannedMetadataOnly),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MigrationLinkStatus {
    LinkedMetadataOnly,
}

impl MigrationLinkStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LinkedMetadataOnly => "linked_metadata_only",
        }
    }

    pub fn from_storage(value: &str) -> Option<Self> {
        match value {
            "linked_metadata_only" => Some(Self::LinkedMetadataOnly),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentUpdateReleaseStatus {
    PublishedExternal,
}

impl AgentUpdateReleaseStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PublishedExternal => "published_external",
        }
    }

    pub fn from_storage(value: &str) -> Option<Self> {
        match value {
            "published_external" => Some(Self::PublishedExternal),
            _ => None,
        }
    }
}

pub fn parse_build_number(value: Option<&str>) -> u64 {
    value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or_else(default_internal_build_number)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NetworkTrafficImportBucket {
    pub interface: String,
    pub start_unix: u64,
    pub duration_secs: u32,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NetworkTrafficImportBatch {
    pub r#type: String,
    pub batch_index: u32,
    pub buckets: Vec<NetworkTrafficImportBucket>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NetworkTrafficImportSource {
    pub interface: String,
    pub database_created_unix: Option<u64>,
    pub retained_start_unix: u64,
    pub source_updated_unix: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NetworkTrafficImportResult {
    pub r#type: String,
    pub status: String,
    pub requested_start_unix: u64,
    pub collected_until_unix: u64,
    pub interfaces: Vec<String>,
    pub sources: Vec<NetworkTrafficImportSource>,
    pub batch_count: u32,
    pub bucket_count: u32,
    pub message: String,
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
    ConfigRead,
    RuntimeConfigSync {
        desired_version: u64,
        reason: String,
        config: Box<AgentRuntimeConfig>,
    },
    #[serde(rename = "agent_update")]
    UpdateAgent {
        artifact_url: String,
        sha256_hex: String,
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
    AgentStop,
    AgentRestart,
    FilePull {
        path: String,
        follow_symlinks: bool,
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
        follow_symlinks: bool,
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
        #[serde(default, skip_serializing_if = "is_false")]
        follow_symlinks: bool,
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
        #[serde(default, skip_serializing_if = "is_false")]
        follow_symlinks: bool,
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
        #[serde(default, skip_serializing_if = "is_false")]
        follow_symlinks: bool,
        #[serde(default)]
        policy: FileActionPolicy,
    },
    FileDownload {
        path: String,
        #[serde(default = "default_file_download_max_bytes")]
        max_bytes: u64,
        #[serde(default, skip_serializing_if = "is_false")]
        follow_symlinks: bool,
    },
    FileArchiveTar {
        path: String,
        #[serde(default = "default_file_archive_max_bytes")]
        max_bytes: u64,
        #[serde(default, skip_serializing_if = "is_false")]
        follow_symlinks: bool,
    },
    UserSessions,
    ProcessList {
        limit: u16,
    },
    StorageInventory {
        #[serde(default)]
        include_pseudo_mounts: bool,
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
    ServiceInventory {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_provider: Option<crate::HostServiceProvider>,
        limit: u16,
    },
    ServiceAction {
        provider: crate::HostServiceProvider,
        service: String,
        action: crate::HostServiceAction,
        expected_active_state: String,
        expected_enabled_state: String,
    },
    ServiceLogs {
        provider: crate::HostServiceProvider,
        service: String,
        max_lines: u16,
    },
    PackageUpdatePlan {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_provider: Option<crate::HostPackageProvider>,
        refresh_metadata: bool,
    },
    PackageUpdateApply {
        provider: crate::HostPackageProvider,
        plan_hash: String,
    },
    Backup {
        paths: Vec<String>,
        include_config: bool,
        follow_symlinks: bool,
        #[serde(default)]
        missing_path_policy: BackupMissingPathPolicy,
    },
    Restore {
        source_backup_request_id: Uuid,
        archive_transfer_session_id: Uuid,
        paths: Vec<String>,
        include_config: bool,
        destination_root: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        archive_path: Option<String>,
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
    NetworkStatus {
        plan_id: String,
        plan: Box<TunnelPlan>,
        side: TunnelEndpointSide,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runtime_adapter: Option<crate::RuntimeTunnelAdapterCommands>,
    },
    NetworkInterfaces,
    NetworkTrafficImportVnstat {
        interfaces: Vec<String>,
        start_unix: u64,
    },
    NetworkProbe {
        plan_id: String,
        plan: Box<TunnelPlan>,
        side: TunnelEndpointSide,
        count: u8,
        interval_ms: u16,
    },
    NetworkSpeedTest {
        plan_id: String,
        plan: Box<TunnelPlan>,
        server_side: TunnelEndpointSide,
        duration_secs: u8,
        max_bytes: u64,
        rate_limit_kbps: u32,
        port: u16,
        connect_timeout_ms: u16,
    },
    NetworkRoutingStatus {
        plan_id: String,
        plan: Box<TunnelPlan>,
        side: TunnelEndpointSide,
        adapter: RoutingCostAdapterCommands,
    },
    NetworkRoutingApply {
        plan_id: String,
        plan: Box<TunnelPlan>,
        side: TunnelEndpointSide,
        adapter: RoutingCostAdapterCommands,
        expected_current_cost: Option<u16>,
        desired_cost: u16,
    },
}

pub fn job_command_protocol_version(command: &JobCommand) -> u16 {
    match command {
        JobCommand::Shell { .. } => SHELL_COMMAND_PROTOCOL_VERSION,
        JobCommand::ShellScript { .. } => SHELL_SCRIPT_COMMAND_PROTOCOL_VERSION,
        JobCommand::TerminalOpen { .. } => TERMINAL_COMMAND_PROTOCOL_VERSION,
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
        JobCommand::ConfigRead | JobCommand::RuntimeConfigSync { .. } => {
            CONFIG_COMMAND_PROTOCOL_VERSION
        }
        JobCommand::UpdateAgent { .. }
        | JobCommand::AgentUpdateActivate { .. }
        | JobCommand::AgentUpdateRollback { .. }
        | JobCommand::AgentUpdateCheck { .. } => AGENT_UPDATE_COMMAND_PROTOCOL_VERSION,
        JobCommand::AgentStop | JobCommand::AgentRestart => {
            AGENT_LIFECYCLE_COMMAND_PROTOCOL_VERSION
        }
        JobCommand::UserSessions => USER_SESSIONS_COMMAND_PROTOCOL_VERSION,
        JobCommand::ProcessList { .. }
        | JobCommand::ProcessStart { .. }
        | JobCommand::ProcessStop { .. }
        | JobCommand::ProcessRestart { .. }
        | JobCommand::ProcessStatus { .. }
        | JobCommand::ProcessLogs { .. } => PROCESS_COMMAND_PROTOCOL_VERSION,
        JobCommand::StorageInventory { .. } => HOST_STORAGE_COMMAND_PROTOCOL_VERSION,
        JobCommand::ServiceInventory { .. }
        | JobCommand::ServiceAction { .. }
        | JobCommand::ServiceLogs { .. } => HOST_SERVICE_COMMAND_PROTOCOL_VERSION,
        JobCommand::PackageUpdatePlan { .. } | JobCommand::PackageUpdateApply { .. } => {
            HOST_PACKAGE_COMMAND_PROTOCOL_VERSION
        }
        JobCommand::Backup { .. } => BACKUP_COMMAND_PROTOCOL_VERSION,
        JobCommand::Restore { .. } | JobCommand::RestoreRollback { .. } => {
            RESTORE_COMMAND_PROTOCOL_VERSION
        }
        JobCommand::NetworkStatus { .. }
        | JobCommand::NetworkInterfaces
        | JobCommand::NetworkProbe { .. }
        | JobCommand::NetworkSpeedTest { .. } => NETWORK_COMMAND_PROTOCOL_VERSION,
        JobCommand::NetworkTrafficImportVnstat { .. } => {
            NETWORK_TRAFFIC_IMPORT_COMMAND_PROTOCOL_VERSION
        }
        JobCommand::NetworkRoutingStatus { .. } | JobCommand::NetworkRoutingApply { .. } => {
            NETWORK_ROUTING_COMMAND_PROTOCOL_VERSION
        }
    }
}

pub fn job_command_dispatch_protocol_version(command: &JobCommand) -> u16 {
    match command {
        // Update commands are the permanent rolling-upgrade escape hatch. Their
        // v1 wire shapes stay frozen so an old agent can always update.
        JobCommand::UpdateAgent { .. }
        | JobCommand::AgentUpdateActivate { .. }
        | JobCommand::AgentUpdateRollback { .. }
        | JobCommand::AgentUpdateCheck { .. } => MIN_COMMAND_PROTOCOL_VERSION,
        // This read-only v1 wire shape did not change when its command family
        // gained v2 operations. Keep it usable during rolling updates.
        JobCommand::NetworkInterfaces => MIN_COMMAND_PROTOCOL_VERSION,
        _ => job_command_protocol_version(command),
    }
}

pub fn job_command_min_supported_protocol_version(command: &JobCommand) -> u16 {
    match command {
        JobCommand::Shell { .. }
        | JobCommand::ShellScript { .. }
        | JobCommand::TerminalOpen { .. }
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
        | JobCommand::NetworkInterfaces => MIN_COMMAND_PROTOCOL_VERSION,
        JobCommand::ConfigRead | JobCommand::RuntimeConfigSync { .. } => {
            CONFIG_COMMAND_PROTOCOL_VERSION
        }
        JobCommand::AgentStop | JobCommand::AgentRestart => {
            AGENT_LIFECYCLE_COMMAND_PROTOCOL_VERSION
        }
        JobCommand::NetworkStatus { .. }
        | JobCommand::NetworkProbe { .. }
        | JobCommand::NetworkSpeedTest { .. } => NETWORK_COMMAND_PROTOCOL_VERSION,
        JobCommand::NetworkTrafficImportVnstat { .. } => {
            NETWORK_TRAFFIC_IMPORT_COMMAND_PROTOCOL_VERSION
        }
        JobCommand::NetworkRoutingStatus { .. } | JobCommand::NetworkRoutingApply { .. } => {
            NETWORK_ROUTING_COMMAND_PROTOCOL_VERSION
        }
        JobCommand::StorageInventory { .. } => HOST_STORAGE_COMMAND_PROTOCOL_VERSION,
        JobCommand::ServiceInventory { .. }
        | JobCommand::ServiceAction { .. }
        | JobCommand::ServiceLogs { .. } => HOST_SERVICE_COMMAND_PROTOCOL_VERSION,
        JobCommand::PackageUpdatePlan { .. } | JobCommand::PackageUpdateApply { .. } => {
            HOST_PACKAGE_COMMAND_PROTOCOL_VERSION
        }
    }
}

pub fn job_command_type_label(command: &JobCommand) -> &'static str {
    match command {
        JobCommand::Shell { pty: true, .. } => "shell_pty",
        JobCommand::Shell { .. } => "shell_argv",
        JobCommand::ShellScript { .. } => "shell_script",
        JobCommand::TerminalOpen { .. } => "terminal_open",
        JobCommand::ConfigRead => "config_read",
        JobCommand::RuntimeConfigSync { .. } => "runtime_config_sync",
        JobCommand::UpdateAgent { .. } => "agent_update",
        JobCommand::AgentUpdateActivate { .. } => "agent_update_activate",
        JobCommand::AgentUpdateRollback { .. } => "agent_update_rollback",
        JobCommand::AgentUpdateCheck { .. } => "agent_update_check",
        JobCommand::AgentStop => "agent_stop",
        JobCommand::AgentRestart => "agent_restart",
        JobCommand::FilePull { .. } => "file_pull",
        JobCommand::FilePush { .. } => "file_push",
        JobCommand::FilePushChunked { .. } => "file_push_chunked",
        JobCommand::FileTransferStart { .. } => "file_transfer_start",
        JobCommand::FileTransferChunk { .. } => "file_transfer_chunk",
        JobCommand::FileTransferCommit { .. } => "file_transfer_commit",
        JobCommand::FileTransferAbort { .. } => "file_transfer_abort",
        JobCommand::FileTransferDownloadStart { .. } => "file_transfer_download_start",
        JobCommand::FileTransferDownloadChunk { .. } => "file_transfer_download_chunk",
        JobCommand::FileStat { .. } => "file_stat",
        JobCommand::FileListDir { .. } => "file_list_dir",
        JobCommand::FileReadText { .. } => "file_read_text",
        JobCommand::FileWriteText { .. } => "file_write_text",
        JobCommand::FileMkdir { .. } => "file_mkdir",
        JobCommand::FileRename { .. } => "file_rename",
        JobCommand::FileDelete { .. } => "file_delete",
        JobCommand::FileChmod { .. } => "file_chmod",
        JobCommand::FileChown { .. } => "file_chown",
        JobCommand::FileCopy { .. } => "file_copy",
        JobCommand::FileDownload { .. } => "file_download",
        JobCommand::FileArchiveTar { .. } => "file_archive_tar",
        JobCommand::UserSessions => "user_sessions",
        JobCommand::ProcessList { .. } => "process_list",
        JobCommand::StorageInventory { .. } => "storage_inventory",
        JobCommand::ProcessStart { .. } => "process_start",
        JobCommand::ProcessStop { .. } => "process_stop",
        JobCommand::ProcessRestart { .. } => "process_restart",
        JobCommand::ProcessStatus { .. } => "process_status",
        JobCommand::ProcessLogs { .. } => "process_logs",
        JobCommand::ServiceInventory { .. } => "service_inventory",
        JobCommand::ServiceAction { .. } => "service_action",
        JobCommand::ServiceLogs { .. } => "service_logs",
        JobCommand::PackageUpdatePlan { .. } => "package_update_plan",
        JobCommand::PackageUpdateApply { .. } => "package_update_apply",
        JobCommand::Backup { .. } => "backup",
        JobCommand::Restore { .. } => "restore",
        JobCommand::RestoreRollback { .. } => "restore_rollback",
        JobCommand::NetworkStatus { .. } => "network_status",
        JobCommand::NetworkInterfaces => "network_interfaces",
        JobCommand::NetworkTrafficImportVnstat { .. } => "network_traffic_import_vnstat",
        JobCommand::NetworkProbe { .. } => "network_probe",
        JobCommand::NetworkSpeedTest { .. } => "network_speed_test",
        JobCommand::NetworkRoutingStatus { .. } => "network_routing_status",
        JobCommand::NetworkRoutingApply { .. } => "network_routing_apply",
    }
}

pub fn job_command_operation_type(command: &JobCommand) -> &'static str {
    match command {
        JobCommand::Shell { .. } => "shell",
        _ => job_command_type_label(command),
    }
}

pub fn scheduled_command_type_label(command: &JobCommand, fallback: &str) -> String {
    match command {
        JobCommand::Shell { .. }
        | JobCommand::ShellScript { .. }
        | JobCommand::RuntimeConfigSync { .. }
        | JobCommand::Backup { .. }
        | JobCommand::Restore { .. }
        | JobCommand::RestoreRollback { .. }
        | JobCommand::NetworkStatus { .. }
        | JobCommand::NetworkInterfaces
        | JobCommand::NetworkTrafficImportVnstat { .. }
        | JobCommand::NetworkProbe { .. }
        | JobCommand::NetworkSpeedTest { .. }
        | JobCommand::NetworkRoutingStatus { .. }
        | JobCommand::NetworkRoutingApply { .. }
        | JobCommand::UpdateAgent { .. }
        | JobCommand::AgentUpdateActivate { .. }
        | JobCommand::AgentUpdateRollback { .. }
        | JobCommand::AgentUpdateCheck { .. }
        | JobCommand::AgentStop
        | JobCommand::AgentRestart
        | JobCommand::ServiceInventory { .. }
        | JobCommand::ServiceAction { .. }
        | JobCommand::ServiceLogs { .. } => job_command_type_label(command).to_string(),
        JobCommand::StorageInventory { .. } => job_command_type_label(command).to_string(),
        JobCommand::PackageUpdatePlan { .. } | JobCommand::PackageUpdateApply { .. } => {
            job_command_type_label(command).to_string()
        }
        _ => fallback.to_string(),
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobCommandSafety {
    Read,
    Write,
    Exec,
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
        | JobCommand::StorageInventory { .. }
        | JobCommand::ServiceInventory { .. }
        | JobCommand::ServiceLogs { .. }
        | JobCommand::NetworkStatus { .. }
        | JobCommand::NetworkInterfaces
        | JobCommand::NetworkProbe { .. }
        | JobCommand::NetworkRoutingStatus { .. } => JobCommandSafety::Read,
        JobCommand::Shell { .. }
        | JobCommand::ShellScript { .. }
        | JobCommand::TerminalOpen { .. }
        | JobCommand::PackageUpdatePlan { .. } => JobCommandSafety::Exec,
        JobCommand::RuntimeConfigSync { .. }
        | JobCommand::UpdateAgent { .. }
        | JobCommand::AgentUpdateActivate { .. }
        | JobCommand::AgentUpdateRollback { .. }
        | JobCommand::AgentUpdateCheck { .. }
        | JobCommand::AgentStop
        | JobCommand::AgentRestart
        | JobCommand::PackageUpdateApply { .. } => JobCommandSafety::Exclusive,
        JobCommand::FilePush { .. }
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
        | JobCommand::Restore { .. }
        | JobCommand::RestoreRollback { .. }
        | JobCommand::NetworkTrafficImportVnstat { .. } => JobCommandSafety::Write,
        JobCommand::ProcessStart { .. }
        | JobCommand::ProcessStop { .. }
        | JobCommand::ProcessRestart { .. }
        | JobCommand::ServiceAction { .. }
        | JobCommand::NetworkSpeedTest { .. }
        | JobCommand::NetworkRoutingApply { .. } => JobCommandSafety::Exec,
        JobCommand::Backup { .. } => JobCommandSafety::Read,
    }
}

pub fn job_command_requires_confirmation(command: &JobCommand) -> bool {
    job_command_requires_confirmation_by_operation_type(job_command_operation_type(command))
        .unwrap_or(false)
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
pub struct SequencedCommandOutput {
    pub seq: i32,
    pub output: CommandOutput,
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
#[path = "tests_protocol.rs"]
mod tests;
