use serde::{Deserialize, Serialize};

fn default_package_plan_type() -> String {
    "package_update_plan".to_string()
}

fn default_package_apply_type() -> String {
    "package_update_apply".to_string()
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostPackageProvider {
    Apt,
    Dnf,
    Yum,
    Pacman,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostPackageCapabilityStatus {
    Supported,
    Ambiguous,
    ProbeFailed,
    #[default]
    Unsupported,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostPackageCapability {
    #[serde(default)]
    pub status: HostPackageCapabilityStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<HostPackageProvider>,
    #[serde(default)]
    pub distro_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distro_version: Option<String>,
    #[serde(default)]
    pub can_plan_cached: bool,
    #[serde(default)]
    pub can_refresh_metadata: bool,
    #[serde(default)]
    pub can_apply: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl HostPackageCapability {
    pub fn supported(&self) -> bool {
        self.status == HostPackageCapabilityStatus::Supported && self.provider.is_some()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct HostPackageUpdateRecord {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_version: Option<String>,
    pub candidate_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostPackageUpdatePlanSnapshot {
    #[serde(default = "default_package_plan_type")]
    pub r#type: String,
    pub capability: HostPackageCapability,
    #[serde(default)]
    pub metadata_refresh_requested: bool,
    #[serde(default)]
    pub metadata_refreshed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_hash: Option<String>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub packages: Vec<HostPackageUpdateRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reboot_required_before: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostPackageUpdateApplyResult {
    #[serde(default = "default_package_apply_type")]
    pub r#type: String,
    pub provider: HostPackageProvider,
    pub accepted_plan_hash: String,
    pub applied_package_count: usize,
    #[serde(default)]
    pub remaining_packages: Vec<HostPackageUpdateRecord>,
    pub completed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reboot_required_after: Option<bool>,
}
