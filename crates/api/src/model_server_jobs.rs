use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ArtifactCleanupPreviewRequest {
    pub(crate) expression: String,
    #[serde(default)]
    pub(crate) domains: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ArtifactCleanupPreviewView {
    pub(crate) expression: String,
    pub(crate) domains: Vec<String>,
    pub(crate) preview_hash: String,
    pub(crate) matched_count: i64,
    pub(crate) matched_bytes: i64,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ArtifactCleanupCreateRequest {
    pub(crate) expression: String,
    #[serde(default)]
    pub(crate) domains: Vec<String>,
    pub(crate) preview_hash: String,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ServerJobView {
    pub(crate) id: Uuid,
    pub(crate) job_type: String,
    pub(crate) status: String,
    pub(crate) expression: Option<String>,
    pub(crate) preview_hash: Option<String>,
    pub(crate) matched_count: i64,
    pub(crate) matched_bytes: i64,
    pub(crate) deleted_count: i64,
    pub(crate) deleted_bytes: i64,
    pub(crate) error: Option<String>,
    pub(crate) created_by: Option<Uuid>,
    pub(crate) metadata: serde_json::Value,
    pub(crate) created_at: String,
    pub(crate) started_at: Option<String>,
    pub(crate) completed_at: Option<String>,
    pub(crate) canceled_at: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct NewServerArtifact {
    pub(crate) domain: String,
    pub(crate) object_key: String,
    pub(crate) sha256_hex: String,
    pub(crate) size_bytes: i64,
    pub(crate) job_id: Option<Uuid>,
    pub(crate) client_id: Option<String>,
    pub(crate) stream: Option<String>,
    pub(crate) seq: Option<i32>,
    pub(crate) backup_request_id: Option<Uuid>,
    pub(crate) backup_artifact_id: Option<Uuid>,
    pub(crate) release_id: Option<Uuid>,
    pub(crate) metadata: serde_json::Value,
}

#[derive(Clone, Debug)]
pub(crate) struct ServerArtifactCleanupCandidate {
    pub(crate) id: Uuid,
    pub(crate) domain: String,
    pub(crate) object_key: String,
    pub(crate) sha256_hex: String,
    pub(crate) size_bytes: i64,
    pub(crate) status: String,
    pub(crate) job_id: Option<Uuid>,
    pub(crate) client_id: Option<String>,
    pub(crate) stream: Option<String>,
    pub(crate) seq: Option<i32>,
    pub(crate) created_at: String,
}
