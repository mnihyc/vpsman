use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vpsman_common::PrivilegeAssertion;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BackupRequestStatus {
    RequestedMetadataOnly,
    ArtifactMetadataRecorded,
}

impl BackupRequestStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::RequestedMetadataOnly => "requested_metadata_only",
            Self::ArtifactMetadataRecorded => "artifact_metadata_recorded",
        }
    }

    pub(crate) fn from_storage(value: &str) -> Option<Self> {
        match value {
            "requested_metadata_only" => Some(Self::RequestedMetadataOnly),
            "artifact_metadata_recorded" => Some(Self::ArtifactMetadataRecorded),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RestorePlanStatus {
    PlannedMetadataOnly,
}

impl RestorePlanStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::PlannedMetadataOnly => "planned_metadata_only",
        }
    }

    pub(crate) fn from_storage(value: &str) -> Option<Self> {
        match value {
            "planned_metadata_only" => Some(Self::PlannedMetadataOnly),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MigrationLinkStatus {
    LinkedMetadataOnly,
}

impl MigrationLinkStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::LinkedMetadataOnly => "linked_metadata_only",
        }
    }

    pub(crate) fn from_storage(value: &str) -> Option<Self> {
        match value {
            "linked_metadata_only" => Some(Self::LinkedMetadataOnly),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BackupRequestView {
    pub(crate) id: Uuid,
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) client_id: String,
    pub(crate) paths: Vec<String>,
    pub(crate) include_config: bool,
    pub(crate) status: String,
    pub(crate) payload_hash: String,
    pub(crate) signed_command_scope: String,
    pub(crate) signed_command_id: Option<Uuid>,
    pub(crate) signed_command_expires_unix: Option<u64>,
    pub(crate) artifact_id: Option<Uuid>,
    pub(crate) source_job_id: Option<Uuid>,
    pub(crate) source_schedule_id: Option<Uuid>,
    pub(crate) note: Option<String>,
    pub(crate) created_at: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BackupArtifactView {
    pub(crate) id: Uuid,
    pub(crate) client_id: String,
    pub(crate) object_key: String,
    pub(crate) sha256_hex: String,
    pub(crate) encrypted: bool,
    pub(crate) size_bytes: i64,
    pub(crate) created_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateBackupRequest {
    pub(crate) client_id: String,
    #[serde(default)]
    pub(crate) paths: Vec<String>,
    #[serde(default)]
    pub(crate) include_config: bool,
    pub(crate) recipient_public_key_hex: Option<String>,
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) note: Option<String>,
    #[serde(default)]
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BackupPolicyView {
    pub(crate) schedule_id: Uuid,
    pub(crate) name: String,
    pub(crate) enabled: bool,
    pub(crate) selector_expression: String,
    pub(crate) paths: Vec<String>,
    pub(crate) include_config: bool,
    pub(crate) recipient_public_key_hex: Option<String>,
    pub(crate) retention_days: i32,
    pub(crate) keep_last: i32,
    pub(crate) rotation_generation: Option<String>,
    pub(crate) cron_expr: String,
    pub(crate) timezone: String,
    pub(crate) next_runs: Vec<String>,
    pub(crate) catch_up_policy: String,
    pub(crate) catch_up_limit: i32,
    pub(crate) retry_delay_secs: i64,
    pub(crate) max_failures: i32,
    pub(crate) failure_count: i32,
    pub(crate) last_error: Option<String>,
    pub(crate) next_run_at: String,
    pub(crate) last_run_at: Option<String>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Clone, Debug)]
pub(crate) struct BackupPolicyMetadata {
    pub(crate) schedule_id: Uuid,
    pub(crate) retention_days: i32,
    pub(crate) keep_last: i32,
    pub(crate) rotation_generation: Option<String>,
    pub(crate) updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateBackupPolicyRequest {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) selector_expression: String,
    #[serde(default)]
    pub(crate) paths: Vec<String>,
    #[serde(default)]
    pub(crate) include_config: bool,
    pub(crate) recipient_public_key_hex: Option<String>,
    pub(crate) retention_days: Option<i32>,
    pub(crate) keep_last: Option<i32>,
    pub(crate) rotation_generation: Option<String>,
    pub(crate) cron_expr: String,
    #[serde(default = "backup_policy_default_timezone")]
    pub(crate) timezone: String,
    #[serde(default = "backup_policy_default_enabled")]
    pub(crate) enabled: bool,
    #[serde(default = "backup_policy_default_catch_up_policy")]
    pub(crate) catch_up_policy: String,
    #[serde(default = "backup_policy_default_catch_up_limit")]
    pub(crate) catch_up_limit: i32,
    #[serde(default = "backup_policy_default_retry_delay_secs")]
    pub(crate) retry_delay_secs: i64,
    #[serde(default = "backup_policy_default_max_failures")]
    pub(crate) max_failures: i32,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BackupPolicyPruneRequest {
    pub(crate) schedule_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) dry_run: bool,
    pub(crate) metadata_only: Option<bool>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BackupPolicyPruneResponse {
    pub(crate) dry_run: bool,
    pub(crate) metadata_only_requested: Option<bool>,
    pub(crate) policies: Vec<BackupPolicyPrunePolicyView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BackupPolicyPrunePolicyView {
    pub(crate) schedule_id: Uuid,
    pub(crate) name: String,
    pub(crate) enabled: bool,
    pub(crate) retention_days: i32,
    pub(crate) keep_last: i32,
    pub(crate) cutoff_unix: u64,
    pub(crate) matched_rows: i64,
    pub(crate) pruned_rows: i64,
    pub(crate) object_keys: Vec<String>,
    pub(crate) object_delete_attempted: bool,
    pub(crate) metadata_only: bool,
    pub(crate) status: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RecordBackupArtifactMetadataRequest {
    pub(crate) object_key: String,
    pub(crate) sha256_hex: String,
    #[serde(default)]
    pub(crate) encrypted: bool,
    pub(crate) size_bytes: i64,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UploadBackupArtifactRequest {
    pub(crate) object_key: String,
    pub(crate) artifact_base64: String,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BackupArtifactUploadSessionCreateRequest {
    pub(crate) object_key: String,
    pub(crate) expected_sha256_hex: String,
    pub(crate) expected_size_bytes: i64,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BackupArtifactUploadChunkRequest {
    pub(crate) offset_bytes: i64,
    pub(crate) data_base64: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BackupArtifactUploadCommitRequest {
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BackupArtifactUploadSessionView {
    pub(crate) upload_id: Uuid,
    pub(crate) backup_request_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) object_key: String,
    pub(crate) expected_sha256_hex: String,
    pub(crate) expected_size_bytes: i64,
    pub(crate) received_bytes: i64,
    pub(crate) next_offset_bytes: i64,
    pub(crate) chunk_count: u64,
    pub(crate) max_chunk_bytes: usize,
    pub(crate) status: String,
    pub(crate) created_unix: u64,
    pub(crate) updated_unix: u64,
    pub(crate) expires_unix: u64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BackupArtifactHandoffRequest {
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) job_id: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BackupArtifactHandoffView {
    pub(crate) artifact: BackupArtifactView,
    pub(crate) source_job_id: Uuid,
    pub(crate) source_chunk_count: usize,
    pub(crate) source: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PrepareBackupArtifactRestoreRequest {
    pub(crate) private_key_hex: String,
    #[serde(default)]
    pub(crate) artifact_base64: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PreparedBackupArtifactRestoreView {
    pub(crate) archive_base64: String,
    pub(crate) archive_sha256_hex: String,
    pub(crate) archive_size_bytes: u64,
    pub(crate) artifact_client_id: String,
    pub(crate) file_count: usize,
    pub(crate) archive_format: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RestorePlanView {
    pub(crate) id: Uuid,
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) source_backup_request_id: Uuid,
    pub(crate) source_client_id: String,
    pub(crate) target_client_id: String,
    pub(crate) paths: Vec<String>,
    pub(crate) include_config: bool,
    pub(crate) destination_root: Option<String>,
    pub(crate) status: String,
    pub(crate) payload_hash: String,
    pub(crate) signed_command_scope: String,
    pub(crate) signed_command_id: Option<Uuid>,
    pub(crate) signed_command_expires_unix: Option<u64>,
    pub(crate) note: Option<String>,
    pub(crate) created_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateRestorePlanRequest {
    pub(crate) source_backup_request_id: Uuid,
    pub(crate) target_client_id: String,
    #[serde(default)]
    pub(crate) paths: Vec<String>,
    #[serde(default)]
    pub(crate) include_config: bool,
    pub(crate) destination_root: Option<String>,
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) note: Option<String>,
    #[serde(default)]
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct MigrationLinkView {
    pub(crate) id: Uuid,
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) restore_plan_id: Uuid,
    pub(crate) source_backup_request_id: Uuid,
    pub(crate) source_client_id: String,
    pub(crate) target_client_id: String,
    pub(crate) paths: Vec<String>,
    pub(crate) include_config: bool,
    pub(crate) destination_root: Option<String>,
    pub(crate) status: String,
    pub(crate) note: Option<String>,
    pub(crate) created_at: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateMigrationLinkRequest {
    pub(crate) restore_plan_id: Uuid,
    #[serde(default)]
    pub(crate) confirmed: bool,
    pub(crate) note: Option<String>,
}

fn backup_policy_default_enabled() -> bool {
    true
}

fn backup_policy_default_timezone() -> String {
    "UTC".to_string()
}

fn backup_policy_default_catch_up_policy() -> String {
    "skip_missed".to_string()
}

fn backup_policy_default_catch_up_limit() -> i32 {
    1
}

fn backup_policy_default_retry_delay_secs() -> i64 {
    300
}

fn backup_policy_default_max_failures() -> i32 {
    3
}
