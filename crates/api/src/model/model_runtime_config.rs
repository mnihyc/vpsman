use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
use vpsman_common::{AgentRuntimeConfig, PrivilegeAssertion};

use crate::model::RuntimeConfigDispatchView;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RuntimeConfigOverrideView {
    pub(crate) client_id: String,
    pub(crate) toml: String,
    pub(crate) reason: String,
    pub(crate) updated_at: String,
    pub(crate) updated_by: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RuntimeConfigApplyStateView {
    pub(crate) client_id: String,
    pub(crate) applied_version: Option<u64>,
    pub(crate) applied_content_hash: Option<String>,
    pub(crate) applied_job_id: Option<Uuid>,
    pub(crate) applied_at: Option<String>,
    pub(crate) pending_version: Option<u64>,
    pub(crate) pending_content_hash: Option<String>,
    pub(crate) pending_job_id: Option<Uuid>,
    pub(crate) pending_reason: Option<String>,
    pub(crate) pending_status: Option<String>,
    pub(crate) pending_error: Option<String>,
    pub(crate) pending_updated_at: Option<String>,
    pub(crate) updated_at: String,
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeConfigApplyStateRecord {
    pub(crate) client_id: String,
    pub(crate) applied_version: Option<u64>,
    pub(crate) applied_content_hash: Option<String>,
    pub(crate) applied_config: Option<AgentRuntimeConfig>,
    pub(crate) applied_job_id: Option<Uuid>,
    pub(crate) applied_at: Option<String>,
    pub(crate) pending_version: Option<u64>,
    pub(crate) pending_content_hash: Option<String>,
    pub(crate) pending_config: Option<AgentRuntimeConfig>,
    pub(crate) pending_job_id: Option<Uuid>,
    pub(crate) pending_reason: Option<String>,
    pub(crate) pending_status: Option<String>,
    pub(crate) pending_error: Option<String>,
    pub(crate) pending_updated_at: Option<String>,
    pub(crate) updated_at: String,
}

impl RuntimeConfigApplyStateRecord {
    pub(crate) fn view(&self) -> RuntimeConfigApplyStateView {
        RuntimeConfigApplyStateView {
            client_id: self.client_id.clone(),
            applied_version: self.applied_version,
            applied_content_hash: self.applied_content_hash.clone(),
            applied_job_id: self.applied_job_id,
            applied_at: self.applied_at.clone(),
            pending_version: self.pending_version,
            pending_content_hash: self.pending_content_hash.clone(),
            pending_job_id: self.pending_job_id,
            pending_reason: self.pending_reason.clone(),
            pending_status: self.pending_status.clone(),
            pending_error: self.pending_error.clone(),
            pending_updated_at: self.pending_updated_at.clone(),
            updated_at: self.updated_at.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RuntimeConfigOverrideCandidate {
    Toml { toml: String },
    Structured { value: Value },
    Reset,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeConfigOverridePreviewRequest {
    pub(crate) candidate: RuntimeConfigOverrideCandidate,
    #[serde(default)]
    pub(crate) reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeConfigOverrideApplyRequest {
    pub(crate) candidate: RuntimeConfigOverrideCandidate,
    #[serde(default)]
    pub(crate) reason: Option<String>,
    pub(crate) expected_override_revision: String,
    pub(crate) expected_desired_hash: String,
    pub(crate) preview_hash: String,
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeConfigBulkPreviewRequest {
    #[serde(default)]
    pub(crate) selector_expression: String,
    pub(crate) target_client_ids: Vec<String>,
    pub(crate) patch: String,
    #[serde(default)]
    pub(crate) reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuntimeConfigBulkApplyRequest {
    #[serde(default)]
    pub(crate) selector_expression: String,
    pub(crate) target_client_ids: Vec<String>,
    pub(crate) patch: String,
    #[serde(default)]
    pub(crate) reason: Option<String>,
    pub(crate) preview_hash: String,
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RuntimeConfigSavedOverrideView {
    pub(crate) exists: bool,
    pub(crate) toml: String,
    pub(crate) parsed: Option<Value>,
    pub(crate) diagnostic: Option<String>,
    pub(crate) reason: Option<String>,
    pub(crate) updated_at: Option<String>,
    pub(crate) updated_by: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RuntimeConfigFieldPolicyView {
    pub(crate) pointer: String,
    pub(crate) path: String,
    pub(crate) label: String,
    pub(crate) value_type: String,
    pub(crate) control: String,
    pub(crate) editable: bool,
    pub(crate) collection: bool,
    pub(crate) owner: String,
    pub(crate) owner_link: Option<String>,
    pub(crate) allowed_operations: Vec<String>,
    pub(crate) enum_values: Vec<String>,
    pub(crate) unit: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RuntimeConfigProvenanceView {
    pub(crate) pointer: String,
    pub(crate) path: String,
    pub(crate) source: String,
    pub(crate) source_chain: Vec<String>,
    pub(crate) locked: bool,
    pub(crate) owner: String,
    pub(crate) owner_link: Option<String>,
    pub(crate) shadowed_override: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RuntimeConfigWorkspaceView {
    pub(crate) client_id: String,
    pub(crate) inherited: Value,
    pub(crate) desired: Value,
    pub(crate) desired_toml: String,
    pub(crate) saved_override: RuntimeConfigSavedOverrideView,
    pub(crate) apply_state: Option<RuntimeConfigApplyStateView>,
    pub(crate) override_revision: String,
    pub(crate) desired_content_hash: String,
    pub(crate) desired_hash: String,
    pub(crate) provenance: Vec<RuntimeConfigProvenanceView>,
    pub(crate) field_schema: Vec<RuntimeConfigFieldPolicyView>,
    pub(crate) generated_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RuntimeConfigPathChangeView {
    pub(crate) pointer: String,
    pub(crate) path: String,
    pub(crate) before: Option<Value>,
    pub(crate) after: Option<Value>,
    pub(crate) kind: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RuntimeConfigOverridePreviewView {
    pub(crate) client_id: String,
    pub(crate) canonical_toml: Option<String>,
    pub(crate) candidate_override: Value,
    pub(crate) desired: Value,
    pub(crate) desired_toml: String,
    pub(crate) provenance: Vec<RuntimeConfigProvenanceView>,
    pub(crate) changes: Vec<RuntimeConfigPathChangeView>,
    pub(crate) storage_only: bool,
    pub(crate) recovery_sync_required: bool,
    pub(crate) override_revision: String,
    pub(crate) desired_hash: String,
    pub(crate) preview_hash: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RuntimeConfigOverrideApplyResponse {
    pub(crate) preview: RuntimeConfigOverridePreviewView,
    pub(crate) override_record: Option<RuntimeConfigOverrideView>,
    pub(crate) sync: Vec<RuntimeConfigDispatchView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RuntimeConfigPatchOperationView {
    pub(crate) operation: String,
    pub(crate) path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) value: Option<Value>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RuntimeConfigBulkTargetPreviewView {
    pub(crate) client_id: String,
    pub(crate) candidate_override_hash: String,
    pub(crate) override_revision: String,
    pub(crate) desired_hash: String,
    pub(crate) changes: Vec<RuntimeConfigPathChangeView>,
    pub(crate) no_op: bool,
    pub(crate) storage_only: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RuntimeConfigBulkPreviewView {
    pub(crate) selector_expression: String,
    pub(crate) target_client_ids: Vec<String>,
    pub(crate) operations: Vec<RuntimeConfigPatchOperationView>,
    pub(crate) targets: Vec<RuntimeConfigBulkTargetPreviewView>,
    pub(crate) changed_target_count: usize,
    pub(crate) preview_hash: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RuntimeConfigBulkApplyResponse {
    pub(crate) preview: RuntimeConfigBulkPreviewView,
    pub(crate) overrides: Vec<RuntimeConfigOverrideView>,
    pub(crate) sync_job_ids: Vec<Uuid>,
    pub(crate) sync: Vec<RuntimeConfigDispatchView>,
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeConfigOverrideReplacement {
    pub(crate) client_id: String,
    pub(crate) expected_revision: String,
    pub(crate) toml: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RuntimeConfigPatchGeneratorView {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) category: String,
    pub(crate) domain: String,
    pub(crate) description: String,
    pub(crate) field_schema: serde_json::Value,
    pub(crate) raw_generator_body: String,
    pub(crate) docs_metadata: serde_json::Value,
    pub(crate) built_in: bool,
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct UpsertRuntimeConfigPatchGeneratorRequest {
    pub(crate) id: Option<Uuid>,
    pub(crate) name: String,
    pub(crate) category: String,
    pub(crate) domain: String,
    pub(crate) description: String,
    pub(crate) field_schema: serde_json::Value,
    pub(crate) raw_generator_body: String,
    pub(crate) docs_metadata: serde_json::Value,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeleteRuntimeConfigPatchGeneratorRequest {
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) reviewed_name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct RenderRuntimeConfigPatchGeneratorRequest {
    #[serde(default)]
    pub(crate) values: serde_json::Value,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RuntimeConfigPatchGeneratorRenderView {
    pub(crate) generator_id: Uuid,
    pub(crate) name: String,
    pub(crate) toml: String,
    pub(crate) patch: serde_json::Value,
    pub(crate) affected_sections: Vec<String>,
    pub(crate) docs_metadata: serde_json::Value,
    pub(crate) generated_at: String,
}
