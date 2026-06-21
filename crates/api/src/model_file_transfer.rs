use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct FileTransferSessionView {
    pub(crate) session_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) direction: String,
    pub(crate) status: String,
    pub(crate) path: String,
    pub(crate) size_bytes: Option<i64>,
    pub(crate) progress_bytes: i64,
    pub(crate) progress_ratio: Option<f64>,
    pub(crate) sha256_hex: Option<String>,
    pub(crate) chunk_size_bytes: Option<i64>,
    pub(crate) last_chunk_size_bytes: Option<i64>,
    pub(crate) last_chunk_sha256_hex: Option<String>,
    pub(crate) rate_limit_kbps: Option<i64>,
    pub(crate) resumed: Option<bool>,
    pub(crate) last_event: String,
    pub(crate) last_job_id: Uuid,
    pub(crate) last_command_type: String,
    pub(crate) last_seq: i32,
    pub(crate) observed_at: String,
    pub(crate) handoff_available: bool,
    pub(crate) handoff_evidence_status: String,
    pub(crate) handoff_unavailable_reason: Option<String>,
    pub(crate) handoff_object_key: Option<String>,
    pub(crate) handoff_download_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct FileTransferHandoffRequest {
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct FileTransferHandoffView {
    pub(crate) client_id: String,
    pub(crate) session_id: Uuid,
    pub(crate) object_key: String,
    pub(crate) sha256_hex: String,
    pub(crate) size_bytes: i64,
    pub(crate) chunk_count: usize,
    pub(crate) source: String,
    pub(crate) download_path: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct FileTransferSourceArtifactView {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    pub(crate) object_key: String,
    pub(crate) sha256_hex: String,
    pub(crate) size_bytes: i64,
    pub(crate) status: String,
    pub(crate) created_by: Option<Uuid>,
    pub(crate) created_at: String,
    pub(crate) download_path: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct UploadFileTransferSourceArtifactRequest {
    pub(crate) name: Option<String>,
    pub(crate) source_base64: String,
    pub(crate) sha256_hex: String,
    pub(crate) size_bytes: i64,
    #[serde(default)]
    pub(crate) confirmed: bool,
}
