use serde::{Deserialize, Serialize};

fn default_service_inventory_type() -> String {
    "service_inventory".to_string()
}

fn default_service_action_type() -> String {
    "service_action".to_string()
}

fn default_service_logs_type() -> String {
    "service_logs".to_string()
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostServiceProvider {
    Systemd,
    Openrc,
    Sysv,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostServiceCapabilityStatus {
    Supported,
    Ambiguous,
    ProbeFailed,
    #[default]
    Unsupported,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostServiceAction {
    Start,
    Stop,
    Restart,
    Enable,
    Disable,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostServiceCapability {
    #[serde(default)]
    pub status: HostServiceCapabilityStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<HostServiceProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_version: Option<String>,
    #[serde(default)]
    pub can_inventory: bool,
    #[serde(default)]
    pub can_start_stop_restart: bool,
    #[serde(default)]
    pub can_enable_disable: bool,
    #[serde(default)]
    pub can_read_logs: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl HostServiceCapability {
    pub fn supported(&self) -> bool {
        self.status == HostServiceCapabilityStatus::Supported && self.provider.is_some()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostServiceRecord {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub load_state: String,
    #[serde(default)]
    pub active_state: String,
    #[serde(default)]
    pub sub_state: String,
    #[serde(default)]
    pub enabled_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostServiceSnapshot {
    #[serde(default = "default_service_inventory_type")]
    pub r#type: String,
    pub capability: HostServiceCapability,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub services: Vec<HostServiceRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostServiceActionResult {
    #[serde(default = "default_service_action_type")]
    pub r#type: String,
    pub provider: HostServiceProvider,
    pub service: String,
    pub action: HostServiceAction,
    pub before: HostServiceRecord,
    pub after: HostServiceRecord,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostServiceLogSnapshot {
    #[serde(default = "default_service_logs_type")]
    pub r#type: String,
    pub provider: HostServiceProvider,
    pub service: String,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub lines: Vec<String>,
}
