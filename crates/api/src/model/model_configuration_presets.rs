use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vpsman_common::{PrivilegeAssertion, RuntimeTunnelCommand};

use crate::model::RuntimeConfigDispatchView;

pub(crate) const CONFIGURATION_BEHAVIORS: &[&str] = &[
    "host_metrics",
    "tunnel_traffic",
    "latency_probe",
    "ospf_update_command",
    "process_inventory",
    "user_sessions",
    "command_execution",
];

#[derive(Clone, Debug)]
pub(crate) struct ResolvedOspfCommandSource {
    pub(crate) origin: String,
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) definition_hash: String,
    pub(crate) status: RuntimeTunnelCommand,
    pub(crate) update: RuntimeTunnelCommand,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ConfigurationPresetView {
    pub(crate) id: Uuid,
    pub(crate) behavior: String,
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) is_default: bool,
    pub(crate) description: Option<String>,
    pub(crate) definition: serde_json::Value,
    pub(crate) effective_vps_count: i64,
    pub(crate) override_vps_count: i64,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ConfigurationPresetOverrideRecord {
    pub(crate) client_id: String,
    pub(crate) behavior: String,
    pub(crate) preset_id: Uuid,
    pub(crate) updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigurationPresetQuery {
    pub(crate) behavior: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateConfigurationPresetRequest {
    pub(crate) behavior: String,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) definition: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CloneConfigurationPresetRequest {
    pub(crate) name: String,
    pub(crate) description: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreviewConfigurationPresetRequest {
    pub(crate) description: Option<String>,
    pub(crate) definition: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateConfigurationPresetRequest {
    pub(crate) description: Option<String>,
    pub(crate) definition: serde_json::Value,
    pub(crate) preview_hash: String,
    #[serde(default)]
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ConfigurationPresetPreviewView {
    pub(crate) preset_id: Uuid,
    pub(crate) behavior: String,
    pub(crate) name: String,
    pub(crate) current_description: Option<String>,
    #[serde(skip_serializing)]
    pub(crate) current_updated_at: String,
    pub(crate) candidate_description: Option<String>,
    pub(crate) current_definition: serde_json::Value,
    pub(crate) candidate_definition: serde_json::Value,
    pub(crate) changed_keys: Vec<String>,
    pub(crate) affected_client_ids: Vec<String>,
    pub(crate) affected_client_count: i64,
    pub(crate) sections: serde_json::Value,
    pub(crate) toml: String,
    pub(crate) preview_hash: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct UpdateConfigurationPresetResponse {
    pub(crate) preset: ConfigurationPresetView,
    pub(crate) preview: ConfigurationPresetPreviewView,
    pub(crate) sync: Vec<RuntimeConfigDispatchView>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigurationSourceQuery {
    pub(crate) client_id: Option<String>,
    pub(crate) behavior: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ConfigurationRuntimeSyncView {
    pub(crate) state: String,
    pub(crate) reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ConfigurationReadinessView {
    pub(crate) state: String,
    pub(crate) reason: String,
    pub(crate) evidence: serde_json::Value,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ConfigurationSourceView {
    pub(crate) client_id: String,
    pub(crate) behavior: String,
    pub(crate) effective_preset_id: Uuid,
    pub(crate) effective_preset_name: String,
    pub(crate) effective_preset_kind: String,
    pub(crate) selection_origin: String,
    pub(crate) override_updated_at: Option<String>,
    pub(crate) runtime_sync: ConfigurationRuntimeSyncView,
    pub(crate) readiness: ConfigurationReadinessView,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConfigurationOverrideAction {
    Set,
    Reset,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreviewConfigurationSourceOverrideRequest {
    pub(crate) action: ConfigurationOverrideAction,
    pub(crate) behavior: String,
    pub(crate) preset_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) selector_expression: String,
    pub(crate) target_client_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApplyConfigurationSourceOverrideRequest {
    pub(crate) action: ConfigurationOverrideAction,
    pub(crate) behavior: String,
    pub(crate) preset_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) selector_expression: String,
    pub(crate) target_client_ids: Vec<String>,
    pub(crate) preview_hash: String,
    #[serde(default)]
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ConfigurationSourceChangeView {
    pub(crate) client_id: String,
    pub(crate) before_preset_id: Uuid,
    pub(crate) before_preset_name: String,
    pub(crate) before_origin: String,
    pub(crate) after_preset_id: Uuid,
    pub(crate) after_preset_name: String,
    pub(crate) after_origin: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ConfigurationSourceOverridePreviewView {
    pub(crate) action: ConfigurationOverrideAction,
    pub(crate) behavior: String,
    pub(crate) preset: Option<ConfigurationPresetView>,
    pub(crate) selector_expression: String,
    pub(crate) target_count: usize,
    pub(crate) targets: Vec<ConfigurationSourceChangeView>,
    pub(crate) preview_hash: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ApplyConfigurationSourceOverrideResponse {
    #[serde(flatten)]
    pub(crate) preview: ConfigurationSourceOverridePreviewView,
    pub(crate) sync: Vec<RuntimeConfigDispatchView>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EffectiveAgentConfigQuery {
    pub(crate) client_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct EffectiveAgentConfigView {
    pub(crate) client_id: String,
    pub(crate) sections: serde_json::Value,
    pub(crate) toml: String,
    pub(crate) sources: Vec<ConfigurationSourceView>,
    pub(crate) generated_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct NetworkAdapterDefinitionView {
    pub(crate) id: Uuid,
    pub(crate) adapter_kind: String,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) definition: serde_json::Value,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NetworkAdapterDefinitionQuery {
    pub(crate) adapter_kind: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpsertNetworkAdapterDefinitionRequest {
    pub(crate) adapter_kind: String,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) definition: serde_json::Value,
}
