use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::{Context, Result};
use sqlx::{postgres::PgPoolOptions, PgPool};
use tokio::sync::RwLock;
use tracing::info;
use uuid::Uuid;

use crate::{
    model::*,
    model_command_templates::CommandTemplateView,
    model_file_transfer::FileTransferSourceArtifactView,
    model_terminal::{TerminalInputRequestRecord, TerminalOutputChunkRecord, TerminalSessionView},
};

#[derive(Clone)]
// Unit tests construct this fixture repository directly in many modules, and
// MemoryState already stores clone-cheap Arc-backed collections. Boxing the
// variant would add broad test churn without reducing production allocation pressure.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Repository {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "unit-test fixture repository is constructed only by tests"
        )
    )]
    Memory(MemoryState),
    Postgres(PgPool),
}

#[derive(Clone, Default)]
pub(crate) struct OperatorAuthThrottleRecord {
    pub(crate) failed_attempts: i64,
    pub(crate) window_started_unix: u64,
    pub(crate) locked_until_unix: Option<u64>,
    pub(crate) last_failure_reason: Option<String>,
}

#[derive(Clone, Default)]
pub(crate) struct MemoryState {
    pub(crate) agents: Arc<RwLock<Vec<AgentView>>>,
    pub(crate) hidden_clients: Arc<RwLock<HashSet<String>>>,
    pub(crate) gateway_sessions: Arc<RwLock<Vec<GatewaySessionView>>>,
    pub(crate) client_status_history: Arc<RwLock<Vec<ClientStatusHistoryView>>>,
    pub(crate) tags: Arc<RwLock<Vec<String>>>,
    pub(crate) fleet_alert_policies:
        Arc<RwLock<Vec<crate::model_alert_policies::FleetAlertPolicyOverrideView>>>,
    pub(crate) fleet_alert_states: Arc<RwLock<Vec<crate::model_alert_states::FleetAlertStateView>>>,
    pub(crate) fleet_alert_notification_channels:
        Arc<RwLock<Vec<crate::model_alert_notifications::FleetAlertNotificationChannelView>>>,
    pub(crate) fleet_alert_notification_deliveries:
        Arc<RwLock<Vec<crate::model_alert_notifications::FleetAlertNotificationDeliveryView>>>,
    pub(crate) webhook_rules: Arc<RwLock<Vec<crate::model_webhook_rules::WebhookRuleView>>>,
    pub(crate) webhook_events: Arc<RwLock<Vec<crate::model_webhook_rules::WebhookEventRow>>>,
    pub(crate) webhook_rule_deliveries:
        Arc<RwLock<Vec<crate::model_webhook_rules::WebhookRuleDeliveryView>>>,
    pub(crate) history_retention_policies:
        Arc<RwLock<Vec<crate::model_history::HistoryRetentionPolicyView>>>,
    pub(crate) source_templates: Arc<RwLock<Vec<SourceTemplateView>>>,
    pub(crate) source_template_assignments: Arc<RwLock<Vec<SourceTemplateAssignmentView>>>,
    pub(crate) runtime_config_overrides: Arc<RwLock<Vec<RuntimeConfigOverrideView>>>,
    pub(crate) runtime_config_apply_states: Arc<RwLock<Vec<RuntimeConfigApplyStateRecord>>>,
    pub(crate) runtime_config_patch_generators: Arc<RwLock<Vec<RuntimeConfigPatchGeneratorView>>>,
    pub(crate) runtime_config_patch_generators_seeded: Arc<RwLock<bool>>,
    pub(crate) operators: Arc<RwLock<Vec<OperatorRecord>>>,
    pub(crate) sessions: Arc<RwLock<Vec<OperatorSessionRecord>>>,
    pub(crate) operator_auth_throttle:
        Arc<RwLock<HashMap<(String, String), OperatorAuthThrottleRecord>>>,
    pub(crate) jobs: Arc<RwLock<Vec<JobHistoryView>>>,
    pub(crate) job_request_fingerprints: Arc<RwLock<HashMap<Uuid, String>>>,
    pub(crate) job_operations: Arc<RwLock<HashMap<Uuid, vpsman_common::JobCommand>>>,
    pub(crate) job_source_schedule_ids: Arc<RwLock<HashMap<Uuid, Uuid>>>,
    pub(crate) job_timeouts: Arc<RwLock<HashMap<Uuid, u64>>>,
    pub(crate) command_templates: Arc<RwLock<Vec<CommandTemplateView>>>,
    pub(crate) job_targets: Arc<RwLock<Vec<JobTargetView>>>,
    pub(crate) job_outputs: Arc<RwLock<Vec<JobOutputView>>>,
    pub(crate) server_artifacts: Arc<RwLock<Vec<ServerArtifactCleanupCandidate>>>,
    pub(crate) terminal_sessions: Arc<RwLock<Vec<TerminalSessionView>>>,
    pub(crate) terminal_input_requests: Arc<RwLock<Vec<TerminalInputRequestRecord>>>,
    pub(crate) terminal_output_chunks: Arc<RwLock<Vec<TerminalOutputChunkRecord>>>,
    pub(crate) file_transfer_source_artifacts: Arc<RwLock<Vec<FileTransferSourceArtifactView>>>,
    pub(crate) agent_update_releases: Arc<RwLock<Vec<AgentUpdateReleaseView>>>,
    pub(crate) server_jobs: Arc<RwLock<Vec<ServerJobView>>>,
    pub(crate) network_observations: Arc<RwLock<Vec<NetworkObservationView>>>,
    pub(crate) system_metric_rollups:
        Arc<RwLock<Vec<crate::model_dashboard::SystemMetricRollupView>>>,
    pub(crate) telemetry_rollups: Arc<RwLock<Vec<TelemetryRollupView>>>,
    pub(crate) telemetry_network_rates: Arc<RwLock<Vec<TelemetryNetworkRateView>>>,
    pub(crate) telemetry_tunnels: Arc<RwLock<Vec<TelemetryTunnelView>>>,
    pub(crate) audits: Arc<RwLock<Vec<AuditLogView>>>,
    pub(crate) schedules: Arc<RwLock<Vec<ScheduleView>>>,
    pub(crate) backup_policies: Arc<RwLock<Vec<BackupPolicyMetadata>>>,
    pub(crate) tunnel_plans: Arc<RwLock<Vec<TunnelPlanView>>>,
    pub(crate) backup_requests: Arc<RwLock<Vec<BackupRequestView>>>,
    pub(crate) backup_artifacts: Arc<RwLock<Vec<BackupArtifactView>>>,
    pub(crate) restore_plans: Arc<RwLock<Vec<RestorePlanView>>>,
    pub(crate) migration_links: Arc<RwLock<Vec<MigrationLinkView>>>,
    pub(crate) client_public_keys: Arc<RwLock<HashMap<String, Vec<u8>>>>,
    pub(crate) client_key_revocations: Arc<RwLock<Vec<ClientKeyRevocationView>>>,
}

impl Repository {
    pub(crate) async fn connect(
        postgres_url: Option<&str>,
        migrations_dir: &std::path::Path,
    ) -> Result<Self> {
        let Some(postgres_url) = postgres_url else {
            anyhow::bail!("VPSMAN_POSTGRES_URL is required");
        };

        let max_connections = std::env::var("VPSMAN_API_DB_MAX_CONNECTIONS")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(32)
            .clamp(1, 256);
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .connect(postgres_url)
            .await
            .context("failed to connect to PostgreSQL")?;
        let migrator = sqlx::migrate::Migrator::new(migrations_dir)
            .await
            .with_context(|| {
                format!(
                    "failed to load migrations from {}",
                    migrations_dir.display()
                )
            })?;
        migrator
            .run(&pool)
            .await
            .context("failed to run PostgreSQL migrations")?;
        info!("api using PostgreSQL repository");
        Ok(Self::Postgres(pool))
    }
}
