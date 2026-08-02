use serde::{Deserialize, Serialize};
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

#[derive(Debug, Deserialize)]
pub(crate) struct RuntimeConfigPatchRequest {
    #[serde(default)]
    pub(crate) selector_expression: String,
    pub(crate) target_client_ids: Vec<String>,
    pub(crate) toml: String,
    #[serde(default)]
    pub(crate) reason: Option<String>,
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RuntimeConfigPatchResponse {
    pub(crate) target_count: usize,
    pub(crate) overrides: Vec<RuntimeConfigOverrideView>,
    pub(crate) sync_job_ids: Vec<Uuid>,
    pub(crate) sync: Vec<RuntimeConfigDispatchView>,
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
