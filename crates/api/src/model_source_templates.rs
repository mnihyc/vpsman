use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SourceTemplateView {
    pub(crate) id: Uuid,
    pub(crate) domain: String,
    pub(crate) name: String,
    pub(crate) scope: String,
    pub(crate) built_in: bool,
    pub(crate) is_default: bool,
    pub(crate) owner_client_id: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) definition: serde_json::Value,
    pub(crate) assigned_client_count: i64,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SourceTemplateAssignmentView {
    pub(crate) client_id: String,
    pub(crate) domain: String,
    pub(crate) template_id: Uuid,
    pub(crate) template_name: String,
    pub(crate) template_scope: String,
    pub(crate) assigned_at: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SourceTemplateQuery {
    pub(crate) domain: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateSourceTemplateRequest {
    pub(crate) domain: String,
    pub(crate) name: String,
    pub(crate) scope: String,
    pub(crate) owner_client_id: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) definition: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CloneSourceTemplateRequest {
    pub(crate) name: String,
    pub(crate) scope: String,
    pub(crate) owner_client_id: Option<String>,
    pub(crate) description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SourceTemplateDiffRequest {
    pub(crate) description: Option<String>,
    pub(crate) definition: serde_json::Value,
    #[serde(default)]
    pub(crate) keep_description: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TestSourceTemplateRequest {
    pub(crate) definition: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UpdateSourceTemplateRequest {
    pub(crate) description: Option<String>,
    pub(crate) definition: serde_json::Value,
    #[serde(default)]
    pub(crate) confirmed: bool,
    #[serde(default)]
    pub(crate) keep_description: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SourceTemplateDiffView {
    pub(crate) template_id: Uuid,
    pub(crate) domain: String,
    pub(crate) template_name: String,
    pub(crate) current_description: Option<String>,
    pub(crate) candidate_description: Option<String>,
    pub(crate) current_definition: serde_json::Value,
    pub(crate) candidate_definition: serde_json::Value,
    pub(crate) description_changed: bool,
    pub(crate) definition_changed: bool,
    pub(crate) changed_keys: Vec<String>,
    pub(crate) affected_client_count: i64,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SourceTemplateTestView {
    pub(crate) template_id: Uuid,
    pub(crate) domain: String,
    pub(crate) template_name: String,
    pub(crate) affected_client_count: i64,
    pub(crate) valid: bool,
    pub(crate) renderable: bool,
    pub(crate) error: Option<String>,
    pub(crate) sections: serde_json::Value,
    pub(crate) toml: String,
    pub(crate) unsupported_domains: Vec<String>,
    pub(crate) render_notes: Vec<String>,
    pub(crate) generated_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct UpdateSourceTemplateResponse {
    pub(crate) template: SourceTemplateView,
    pub(crate) diff: SourceTemplateDiffView,
    pub(crate) affected_client_count: i64,
    pub(crate) confirmation_required: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SourceTemplateAssignmentQuery {
    pub(crate) client_id: Option<String>,
    pub(crate) domain: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SourceStatusQuery {
    pub(crate) client_id: Option<String>,
    pub(crate) domain: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SourceConfigPatchQuery {
    pub(crate) client_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AssignSourceTemplateRequest {
    pub(crate) domain: String,
    pub(crate) template_id: Uuid,
    #[serde(default)]
    pub(crate) selector_expression: String,
    pub(crate) target_client_ids: Vec<String>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct AssignSourceTemplateResponse {
    pub(crate) template: SourceTemplateView,
    pub(crate) target_count: usize,
    pub(crate) confirmation_required: bool,
    pub(crate) assignments: Vec<SourceTemplateAssignmentView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SourceConfigPatchView {
    pub(crate) client_id: String,
    pub(crate) sections: serde_json::Value,
    pub(crate) toml: String,
    pub(crate) assignments: Vec<SourceTemplateAssignmentView>,
    pub(crate) unsupported_domains: Vec<String>,
    pub(crate) render_notes: Vec<String>,
    pub(crate) generated_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SourceStatusView {
    pub(crate) client_id: String,
    pub(crate) display_name: String,
    pub(crate) client_status: String,
    pub(crate) domain: String,
    pub(crate) module: String,
    pub(crate) template_id: Uuid,
    pub(crate) template_name: String,
    pub(crate) template_scope: String,
    pub(crate) source_kind: String,
    pub(crate) status: String,
    pub(crate) status_reason: String,
    pub(crate) evidence: serde_json::Value,
    pub(crate) assigned_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HotConfigPatchGeneratorView {
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
pub(crate) struct UpsertHotConfigPatchGeneratorRequest {
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
pub(crate) struct DeleteHotConfigPatchGeneratorRequest {
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) reviewed_name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct RenderHotConfigPatchGeneratorRequest {
    #[serde(default)]
    pub(crate) values: serde_json::Value,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HotConfigPatchGeneratorRenderView {
    pub(crate) generator_id: Uuid,
    pub(crate) name: String,
    pub(crate) toml: String,
    pub(crate) patch: serde_json::Value,
    pub(crate) affected_sections: Vec<String>,
    pub(crate) docs_metadata: serde_json::Value,
    pub(crate) generated_at: String,
}
