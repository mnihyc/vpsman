use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HistoryDomain {
    AuditLogs,
    SystemMetricRollups,
    TelemetryRollups,
    JobOutputs,
    BackupArtifacts,
    NetworkObservations,
    TopologyHistory,
}

impl HistoryDomain {
    pub(crate) const ALL: [Self; 7] = [
        Self::AuditLogs,
        Self::SystemMetricRollups,
        Self::TelemetryRollups,
        Self::JobOutputs,
        Self::BackupArtifacts,
        Self::NetworkObservations,
        Self::TopologyHistory,
    ];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::AuditLogs => "audit_logs",
            Self::SystemMetricRollups => "system_metric_rollups",
            Self::TelemetryRollups => "telemetry_rollups",
            Self::JobOutputs => "job_outputs",
            Self::BackupArtifacts => "backup_artifacts",
            Self::NetworkObservations => "network_observations",
            Self::TopologyHistory => "topology_history",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value.trim() {
            "audit_logs" | "audit" => Some(Self::AuditLogs),
            "system_metric_rollups" | "system_metrics" | "system" => {
                Some(Self::SystemMetricRollups)
            }
            "telemetry_rollups" | "telemetry" => Some(Self::TelemetryRollups),
            "job_outputs" | "jobs" => Some(Self::JobOutputs),
            "backup_artifacts" | "backups" => Some(Self::BackupArtifacts),
            "network_observations" | "network" => Some(Self::NetworkObservations),
            "topology_history" | "topology" => Some(Self::TopologyHistory),
            _ => None,
        }
    }

    pub(crate) fn default_retention_days(self) -> i32 {
        match self {
            Self::JobOutputs => 30,
            Self::SystemMetricRollups => 3650,
            Self::TelemetryRollups => 3650,
            Self::NetworkObservations | Self::TopologyHistory => 180,
            Self::AuditLogs => 365,
            Self::BackupArtifacts => 3650,
        }
    }

    pub(crate) fn default_prune_limit(self) -> i32 {
        match self {
            Self::JobOutputs | Self::NetworkObservations => 5_000,
            Self::SystemMetricRollups | Self::TelemetryRollups | Self::TopologyHistory => 2_000,
            Self::AuditLogs | Self::BackupArtifacts => 1_000,
        }
    }

    pub(crate) fn object_backed(self) -> bool {
        matches!(self, Self::JobOutputs | Self::BackupArtifacts)
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HistoryRetentionPolicyView {
    pub(crate) domain: String,
    pub(crate) retention_days: i32,
    pub(crate) prune_limit: i32,
    pub(crate) enabled: bool,
    pub(crate) metadata_only: bool,
    pub(crate) export_enabled: bool,
    pub(crate) notes: Option<String>,
    pub(crate) updated_by: Option<uuid::Uuid>,
    pub(crate) updated_at: String,
    pub(crate) built_in_default: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct UpsertHistoryRetentionPolicyRequest {
    pub(crate) domain: String,
    pub(crate) retention_days: Option<i32>,
    pub(crate) prune_limit: Option<i32>,
    pub(crate) enabled: Option<bool>,
    pub(crate) metadata_only: Option<bool>,
    pub(crate) export_enabled: Option<bool>,
    pub(crate) notes: Option<String>,
    #[serde(default)]
    pub(crate) clear_notes: bool,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct HistoryRetentionPruneRequest {
    pub(crate) domain: Option<String>,
    #[serde(default)]
    pub(crate) dry_run: bool,
    pub(crate) metadata_only: Option<bool>,
    pub(crate) preview_hash: Option<String>,
    #[serde(default)]
    pub(crate) confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HistoryRetentionPruneResponse {
    pub(crate) dry_run: bool,
    pub(crate) metadata_only_requested: Option<bool>,
    pub(crate) preview_hash: String,
    pub(crate) domains: Vec<HistoryRetentionPruneDomainView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HistoryRetentionPruneDomainView {
    pub(crate) domain: String,
    pub(crate) enabled: bool,
    pub(crate) retention_days: i32,
    pub(crate) cutoff_unix: u64,
    pub(crate) matched_rows: i64,
    pub(crate) pruned_rows: i64,
    pub(crate) object_keys: Vec<String>,
    pub(crate) object_delete_attempted: bool,
    pub(crate) object_delete_errors: Vec<String>,
    pub(crate) metadata_only: bool,
    pub(crate) status: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct HistoryExportQuery {
    pub(crate) domains: Option<String>,
    pub(crate) limit: Option<i64>,
    pub(crate) client_id: Option<String>,
    pub(crate) job_id: Option<uuid::Uuid>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HistoryExportView {
    pub(crate) generated_at: String,
    pub(crate) limit: i64,
    pub(crate) domains: Vec<String>,
    pub(crate) data: serde_json::Value,
}

#[derive(Clone, Debug)]
pub(crate) struct HistoryRetentionPrunePlan {
    pub(crate) domain: HistoryDomain,
    pub(crate) prune_limit: i32,
    pub(crate) enabled: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct HistoryRetentionPruneOutcome {
    pub(crate) matched_rows: i64,
    pub(crate) pruned_rows: i64,
    pub(crate) object_keys: Vec<String>,
}
