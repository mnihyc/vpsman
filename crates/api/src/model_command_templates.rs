use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CommandTemplateView {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) scope_kind: String,
    pub(crate) scope_value: Option<String>,
    pub(crate) command_type: String,
    pub(crate) display_group: Option<String>,
    pub(crate) operation: serde_json::Value,
    pub(crate) defaults: serde_json::Value,
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CommandTemplateQuery {
    pub(crate) limit: Option<i64>,
    pub(crate) scope_kind: Option<String>,
    pub(crate) scope_value: Option<String>,
    pub(crate) command_type: Option<String>,
    pub(crate) display_group: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpsertCommandTemplateRequest {
    pub(crate) name: String,
    pub(crate) scope_kind: String,
    #[serde(default)]
    pub(crate) scope_value: Option<String>,
    #[serde(default)]
    pub(crate) display_group: Option<String>,
    pub(crate) operation: serde_json::Value,
    #[serde(default)]
    pub(crate) defaults: serde_json::Value,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct JobOutputComparisonView {
    pub(crate) job_id: Uuid,
    pub(crate) mode: String,
    pub(crate) compared_at: String,
    pub(crate) total_targets: i32,
    pub(crate) compared_targets: i32,
    pub(crate) group_count: i32,
    pub(crate) groups: Vec<JobOutputComparisonGroupView>,
    pub(crate) rows: Vec<JobOutputComparisonRowView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct JobOutputComparisonGroupView {
    pub(crate) group_id: String,
    pub(crate) status: String,
    pub(crate) exit_code: Option<i32>,
    pub(crate) output_digest_hex: String,
    pub(crate) output_compare_basis: String,
    pub(crate) target_count: i32,
    pub(crate) stream_count: i32,
    pub(crate) byte_count: i64,
    pub(crate) representative_client_id: String,
    pub(crate) client_ids: Vec<String>,
    pub(crate) preview: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct JobOutputComparisonRowView {
    pub(crate) job_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) group_id: String,
    pub(crate) status: String,
    pub(crate) exit_code: Option<i32>,
    pub(crate) output_digest_hex: String,
    pub(crate) output_compare_basis: String,
    pub(crate) stream_count: i32,
    pub(crate) byte_count: i64,
    pub(crate) matches_largest_group: bool,
    pub(crate) preview: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct JobOutputComparisonQuery {
    pub(crate) mode: Option<String>,
}
