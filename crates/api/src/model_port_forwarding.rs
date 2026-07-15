use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vpsman_common::{PortForwardMapping, PortForwardProtocol, PortForwardRuntimeSnapshot};

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PortForwardRuleView {
    pub(crate) id: Uuid,
    pub(crate) client_id: String,
    pub(crate) name: String,
    pub(crate) protocol: PortForwardProtocol,
    pub(crate) target_ip: IpAddr,
    pub(crate) mappings: Vec<PortForwardMapping>,
    pub(crate) masquerade: bool,
    pub(crate) enabled: bool,
    pub(crate) revision: i64,
    pub(crate) desired_status: String,
    pub(crate) runtime_status: String,
    pub(crate) nat_matches: u64,
    pub(crate) desired_hash: Option<String>,
    pub(crate) agent_desired_hash: Option<String>,
    pub(crate) observed_hash: Option<String>,
    pub(crate) nft_version: Option<String>,
    pub(crate) forwarding_enabled: Option<bool>,
    pub(crate) runtime_observed_unix: Option<u64>,
    pub(crate) runtime_error_code: Option<String>,
    pub(crate) runtime_error: Option<String>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) deleted_at: Option<String>,
    pub(crate) removal_confirmed_at: Option<String>,
    pub(crate) forgotten_at: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct PortForwardRuleRecord {
    pub(crate) id: Uuid,
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) client_id: String,
    pub(crate) name: String,
    pub(crate) protocol: PortForwardProtocol,
    pub(crate) target_ip: IpAddr,
    pub(crate) mappings: Vec<PortForwardMapping>,
    pub(crate) masquerade: bool,
    pub(crate) enabled: bool,
    pub(crate) revision: i64,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) deleted_at: Option<String>,
    pub(crate) deleted_by: Option<Uuid>,
    pub(crate) deleted_reason: Option<String>,
    pub(crate) removal_confirmed_at: Option<String>,
    pub(crate) forgotten_at: Option<String>,
    pub(crate) forgotten_by: Option<Uuid>,
    pub(crate) forget_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreatePortForwardRuleRequest {
    pub(crate) client_id: String,
    pub(crate) name: String,
    pub(crate) protocol: PortForwardProtocol,
    pub(crate) target_ip: IpAddr,
    pub(crate) mappings: Vec<PortForwardMapping>,
    #[serde(default = "default_true")]
    pub(crate) masquerade: bool,
    #[serde(default = "default_true")]
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdatePortForwardRuleRequest {
    pub(crate) expected_revision: i64,
    pub(crate) name: String,
    pub(crate) protocol: PortForwardProtocol,
    pub(crate) target_ip: IpAddr,
    pub(crate) mappings: Vec<PortForwardMapping>,
    #[serde(default = "default_true")]
    pub(crate) masquerade: bool,
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PortForwardMutationRequest {
    pub(crate) expected_revision: i64,
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PortForwardSyncView {
    pub(crate) status: String,
    pub(crate) job_id: Option<Uuid>,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PortForwardMutationResponse {
    pub(crate) rule: PortForwardRuleView,
    pub(crate) sync: PortForwardSyncView,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PortForwardBulkAction {
    Enable,
    Disable,
    Reapply,
    Delete,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PortForwardBulkItem {
    pub(crate) id: Uuid,
    pub(crate) expected_revision: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PortForwardBulkRequest {
    pub(crate) action: PortForwardBulkAction,
    pub(crate) items: Vec<PortForwardBulkItem>,
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PortForwardBulkResponse {
    pub(crate) rules: Vec<PortForwardRuleView>,
    pub(crate) sync: Vec<PortForwardClientSyncView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PortForwardClientSyncView {
    pub(crate) client_id: String,
    pub(crate) sync: PortForwardSyncView,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResolveHostnameRequest {
    pub(crate) hostname: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ResolvedAddressView {
    pub(crate) address: IpAddr,
    pub(crate) family: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ResolveHostnameResponse {
    pub(crate) hostname: String,
    pub(crate) candidates: Vec<ResolvedAddressView>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PortForwardRuntimeRecord {
    pub(crate) snapshot: PortForwardRuntimeSnapshot,
}

const fn default_true() -> bool {
    true
}
