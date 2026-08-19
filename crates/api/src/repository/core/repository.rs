use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::{Context, Result};
use sqlx::{postgres::PgPoolOptions, PgPool};
use tokio::sync::{Mutex, RwLock};
use tracing::info;
use uuid::Uuid;

use crate::{
    model::*,
    model_command_templates::CommandTemplateView,
    model_file_transfer::FileTransferSourceArtifactView,
    model_terminal::{TerminalOutputChunkRecord, TerminalSessionView},
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

#[derive(Clone, Copy)]
pub(crate) struct TelemetryIngestWatermark {
    pub(crate) gateway_session_id: Uuid,
    pub(crate) process_incarnation_id: Uuid,
    pub(crate) telemetry_seq: u64,
}

pub(crate) type TelemetryIngestWatermarks = Arc<RwLock<HashMap<String, TelemetryIngestWatermark>>>;
pub(crate) type MemoryPingSourceCheckKey = (String, Uuid, i64, u64);
pub(crate) type MemoryPingSourceChecks = Arc<RwLock<HashMap<MemoryPingSourceCheckKey, u64>>>;
type CapabilityDegradedJobTargets = HashMap<(Uuid, String), (String, String)>;

#[derive(Clone, Debug, Default)]
pub(crate) struct MemoryTagOrderState {
    pub(crate) names: Vec<String>,
    pub(crate) namespace_natural_sort_enabled: bool,
}

#[derive(Clone, Default)]
pub(crate) struct MemoryState {
    pub(crate) agents: Arc<RwLock<Vec<AgentView>>>,
    pub(crate) client_system_facts:
        Arc<RwLock<HashMap<String, crate::model::ClientSystemFactsRecord>>>,
    pub(crate) hidden_clients: Arc<RwLock<HashSet<String>>>,
    pub(crate) gateway_sessions: Arc<RwLock<Vec<GatewaySessionView>>>,
    pub(crate) client_status_history: Arc<RwLock<Vec<ClientStatusHistoryView>>>,
    pub(crate) tag_order: Arc<RwLock<MemoryTagOrderState>>,
    pub(crate) vps_rule_values: Arc<RwLock<Vec<crate::model_alert_policies::VpsRuleValueRecord>>>,
    pub(crate) vps_rule_mutation: Arc<Mutex<()>>,
    pub(crate) traffic_counter_samples:
        Arc<RwLock<Vec<crate::model_alert_policies::TrafficCounterSampleRecord>>>,
    pub(crate) traffic_counter_rollups:
        Arc<RwLock<Vec<crate::model_alert_policies::TrafficCounterRollupRecord>>>,
    pub(crate) policy_groups: Arc<RwLock<Vec<crate::model_alert_policies::PolicyGroupRecord>>>,
    pub(crate) policy_rule_states:
        Arc<RwLock<Vec<crate::model_alert_policies::PolicyRuleStateRecord>>>,
    pub(crate) policy_alerts: Arc<RwLock<Vec<crate::model_alert_policies::PolicyAlertRecord>>>,
    #[cfg(test)]
    pub(crate) operational_alert_episodes:
        Arc<RwLock<Vec<crate::model::OperationalAlertEpisodeRecord>>>,
    #[cfg(test)]
    pub(crate) operational_alert_mutation: Arc<Mutex<()>>,
    pub(crate) operational_alert_tunnel_plan_boundaries: Arc<RwLock<HashMap<Uuid, String>>>,
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
    pub(crate) configuration_presets: Arc<RwLock<Vec<ConfigurationPresetView>>>,
    pub(crate) configuration_preset_overrides: Arc<RwLock<Vec<ConfigurationPresetOverrideRecord>>>,
    pub(crate) configuration_presets_seeded: Arc<RwLock<bool>>,
    pub(crate) network_adapter_definitions: Arc<RwLock<Vec<NetworkAdapterDefinitionView>>>,
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
    pub(crate) job_rollouts: Arc<RwLock<Vec<MemoryJobRolloutRecord>>>,
    pub(crate) job_rollout_targets: Arc<RwLock<HashMap<(Uuid, String), u16>>>,
    pub(crate) job_approvals: Arc<RwLock<Vec<JobApprovalView>>>,
    pub(crate) job_approval_requests: Arc<RwLock<HashMap<Uuid, CreateJobRequest>>>,
    pub(crate) job_approval_ids: Arc<RwLock<HashMap<Uuid, Uuid>>>,
    pub(crate) command_templates: Arc<RwLock<Vec<CommandTemplateView>>>,
    pub(crate) job_targets: Arc<RwLock<Vec<JobTargetView>>>,
    pub(crate) job_outputs: Arc<RwLock<Vec<JobOutputView>>>,
    /// Serializes the Memory fixture's replayable terminal side effects. PostgreSQL uses
    /// durable terminal-event rows for this boundary; the in-memory repository needs one
    /// equivalent critical section so concurrent identical replays cannot double-apply
    /// schedule counters or lifecycle audits.
    pub(crate) job_terminal_side_effects: Arc<Mutex<()>>,
    pub(crate) network_traffic_import_retry_not_before: Arc<RwLock<HashMap<(Uuid, String), u64>>>,
    pub(crate) capability_degraded_job_targets: Arc<RwLock<CapabilityDegradedJobTargets>>,
    pub(crate) server_artifacts: Arc<RwLock<Vec<ServerArtifactCleanupCandidate>>>,
    pub(crate) terminal_sessions: Arc<RwLock<Vec<TerminalSessionView>>>,
    pub(crate) terminal_output_chunks: Arc<RwLock<Vec<TerminalOutputChunkRecord>>>,
    pub(crate) file_transfer_source_artifacts: Arc<RwLock<Vec<FileTransferSourceArtifactView>>>,
    pub(crate) agent_update_releases: Arc<RwLock<Vec<AgentUpdateReleaseView>>>,
    pub(crate) server_jobs: Arc<RwLock<Vec<ServerJobView>>>,
    pub(crate) network_observations: Arc<RwLock<Vec<NetworkObservationView>>>,
    pub(crate) system_metric_rollups:
        Arc<RwLock<Vec<crate::model_dashboard::SystemMetricRollupView>>>,
    pub(crate) telemetry_samples: Arc<RwLock<Vec<TelemetrySampleView>>>,
    pub(crate) telemetry_rollups: Arc<RwLock<Vec<TelemetryRollupView>>>,
    pub(crate) telemetry_network_rates: Arc<RwLock<Vec<TelemetryNetworkRateView>>>,
    pub(crate) ping_targets: Arc<RwLock<Vec<PingTargetRecord>>>,
    pub(crate) ping_target_assignments: Arc<RwLock<Vec<PingTargetAssignmentRecord>>>,
    pub(crate) telemetry_ping_rollups: Arc<RwLock<Vec<PingRollupView>>>,
    pub(crate) telemetry_ping_source_checks: MemoryPingSourceChecks,
    pub(crate) monitoring_shares: Arc<RwLock<Vec<MonitoringShareRecord>>>,
    pub(crate) monitoring_share_visitors: Arc<RwLock<Vec<MonitoringShareVisitorRecord>>>,
    pub(crate) telemetry_tunnels: Arc<RwLock<Vec<TelemetryTunnelView>>>,
    pub(crate) telemetry_ingest_watermarks: TelemetryIngestWatermarks,
    pub(crate) audits: Arc<RwLock<Vec<AuditLogView>>>,
    pub(crate) schedules: Arc<RwLock<Vec<ScheduleView>>>,
    pub(crate) backup_policies: Arc<RwLock<Vec<BackupPolicyMetadata>>>,
    pub(crate) tunnel_plans: Arc<RwLock<Vec<TunnelPlanView>>>,
    pub(crate) automatic_ospf_plan_scans: Arc<RwLock<HashMap<Uuid, u64>>>,
    pub(crate) pending_ospf_plan_reconciliations: Arc<RwLock<HashMap<Uuid, u64>>>,
    pub(crate) port_forward_rules:
        Arc<RwLock<Vec<crate::model_port_forwarding::PortForwardRuleRecord>>>,
    pub(crate) port_forward_runtime:
        Arc<RwLock<HashMap<String, crate::model_port_forwarding::PortForwardRuntimeRecord>>>,
    pub(crate) port_forward_lifecycle: Arc<Mutex<()>>,
    pub(crate) backup_requests: Arc<RwLock<Vec<BackupRequestView>>>,
    pub(crate) backup_artifacts: Arc<RwLock<Vec<BackupArtifactView>>>,
    pub(crate) restore_plans: Arc<RwLock<Vec<RestorePlanView>>>,
    pub(crate) migration_links: Arc<RwLock<Vec<MigrationLinkView>>>,
    pub(crate) agent_key_lifecycle: Arc<Mutex<()>>,
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
        let repository = Self::Postgres(pool);
        repository
            .initialize_system_configuration_presets()
            .await
            .context("failed to initialize system configuration presets")?;
        info!("api using PostgreSQL repository");
        Ok(repository)
    }
}
