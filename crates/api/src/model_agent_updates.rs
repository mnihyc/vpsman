use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct AgentUpdateReleaseView {
    pub(crate) id: Uuid,
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) channel: String,
    pub(crate) status: String,
    pub(crate) artifact_sha256_hex: String,
    pub(crate) artifact_signature_provided: bool,
    pub(crate) artifact_signature_sha256_hex: Option<String>,
    pub(crate) artifact_signing_key_sha256_hex: String,
    pub(crate) artifact_url_sha256_hex: Option<String>,
    pub(crate) artifact_object_key: Option<String>,
    pub(crate) artifact_download_path: Option<String>,
    pub(crate) rollback_artifact_sha256_hex: Option<String>,
    pub(crate) rollback_artifact_signature_provided: bool,
    pub(crate) rollback_artifact_signature_sha256_hex: Option<String>,
    pub(crate) rollback_artifact_signing_key_sha256_hex: Option<String>,
    pub(crate) rollback_artifact_url_sha256_hex: Option<String>,
    pub(crate) rollback_artifact_object_key: Option<String>,
    pub(crate) rollback_artifact_download_path: Option<String>,
    pub(crate) rollback_size_bytes: Option<i64>,
    pub(crate) size_bytes: Option<i64>,
    pub(crate) notes: Option<String>,
    pub(crate) created_at: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateAgentUpdateReleaseRequest {
    pub(crate) name: String,
    pub(crate) version: String,
    #[serde(default = "default_release_channel")]
    pub(crate) channel: String,
    pub(crate) artifact_sha256_hex: String,
    pub(crate) artifact_signature_hex: String,
    pub(crate) artifact_signing_key_hex: String,
    pub(crate) artifact_url: Option<String>,
    #[serde(default)]
    pub(crate) rollback_artifact_sha256_hex: Option<String>,
    #[serde(default)]
    pub(crate) rollback_artifact_signature_hex: Option<String>,
    #[serde(default)]
    pub(crate) rollback_artifact_signing_key_hex: Option<String>,
    #[serde(default)]
    pub(crate) rollback_artifact_url: Option<String>,
    #[serde(default)]
    pub(crate) rollback_size_bytes: Option<i64>,
    pub(crate) size_bytes: Option<i64>,
    pub(crate) notes: Option<String>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UploadAgentUpdateArtifactRequest {
    pub(crate) name: String,
    pub(crate) version: String,
    #[serde(default = "default_release_channel")]
    pub(crate) channel: String,
    pub(crate) artifact_base64: String,
    pub(crate) artifact_signature_hex: String,
    pub(crate) artifact_signing_key_hex: String,
    #[serde(default)]
    pub(crate) rollback_artifact_base64: Option<String>,
    #[serde(default)]
    pub(crate) rollback_artifact_signature_hex: Option<String>,
    #[serde(default)]
    pub(crate) rollback_artifact_signing_key_hex: Option<String>,
    pub(crate) notes: Option<String>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct StreamedAgentUpdateArtifactView {
    pub(crate) artifact_sha256_hex: String,
    pub(crate) artifact_signature_provided: bool,
    pub(crate) artifact_signature_sha256_hex: String,
    pub(crate) artifact_signing_key_sha256_hex: String,
    pub(crate) artifact_object_key: String,
    pub(crate) artifact_download_path: String,
    pub(crate) size_bytes: i64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateHostedAgentUpdateReleaseRequest {
    pub(crate) name: String,
    pub(crate) version: String,
    #[serde(default = "default_release_channel")]
    pub(crate) channel: String,
    pub(crate) artifact_sha256_hex: String,
    pub(crate) artifact_signature_hex: String,
    pub(crate) artifact_signing_key_hex: String,
    #[serde(default)]
    pub(crate) rollback_artifact_sha256_hex: Option<String>,
    #[serde(default)]
    pub(crate) rollback_artifact_signature_hex: Option<String>,
    #[serde(default)]
    pub(crate) rollback_artifact_signing_key_hex: Option<String>,
    pub(crate) notes: Option<String>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

fn default_release_channel() -> String {
    "stable".to_string()
}
