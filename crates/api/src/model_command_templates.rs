use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CommandTemplateView {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) scope_kind: String,
    pub(crate) scope_value: Option<String>,
    pub(crate) command_type: String,
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
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct UpsertCommandTemplateRequest {
    pub(crate) name: String,
    pub(crate) scope_kind: String,
    #[serde(default)]
    pub(crate) scope_value: Option<String>,
    pub(crate) command_type: String,
    pub(crate) operation: serde_json::Value,
    #[serde(default)]
    pub(crate) defaults: serde_json::Value,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct JobOutputComparisonView {
    pub(crate) job_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) output_sha256_hex: String,
    pub(crate) stream_count: i32,
    pub(crate) byte_count: i64,
    pub(crate) exit_code: Option<i32>,
    pub(crate) matches_majority: bool,
    pub(crate) preview: String,
    pub(crate) compared_at: String,
}
