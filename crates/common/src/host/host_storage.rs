use serde::{Deserialize, Serialize};

fn default_storage_inventory_type() -> String {
    "storage_inventory".to_string()
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostStorageProvider {
    LsblkJson,
    LsblkPairs,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostStorageCapabilityStatus {
    Supported,
    ProbeFailed,
    #[default]
    Unsupported,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostStorageCapability {
    #[serde(default)]
    pub status: HostStorageCapabilityStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<HostStorageProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_version: Option<String>,
    #[serde(default)]
    pub available_columns: Vec<String>,
    #[serde(default)]
    pub can_report_filesystem_usage: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl HostStorageCapability {
    pub fn supported(&self) -> bool {
        self.status == HostStorageCapabilityStatus::Supported && self.provider.is_some()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostBlockDeviceRecord {
    pub name: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_path: Option<String>,
    pub device_type: String,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filesystem_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filesystem_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    #[serde(default)]
    pub mount_points: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filesystem_available_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filesystem_used_percent: Option<u8>,
    pub read_only: bool,
    pub removable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub major_minor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostMountRecord {
    pub mount_id: u64,
    pub parent_id: u64,
    pub major_minor: String,
    pub root: String,
    pub target: String,
    pub filesystem_type: String,
    pub source: String,
    #[serde(default)]
    pub options: Vec<String>,
    pub read_only: bool,
    pub pseudo: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostStorageSnapshot {
    #[serde(default = "default_storage_inventory_type")]
    pub r#type: String,
    pub capability: HostStorageCapability,
    #[serde(default)]
    pub include_pseudo_mounts: bool,
    #[serde(default)]
    pub devices_truncated: bool,
    #[serde(default)]
    pub mounts_truncated: bool,
    #[serde(default)]
    pub devices: Vec<HostBlockDeviceRecord>,
    #[serde(default)]
    pub mounts: Vec<HostMountRecord>,
}
