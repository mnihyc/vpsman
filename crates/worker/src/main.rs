use std::{
    collections::HashSet,
    path::PathBuf,
    str::FromStr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{bail, ensure, Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use croner::Cron;
use serde_json::Value;
use sqlx::{
    postgres::{PgConnectOptions, PgListener, PgPoolOptions},
    types::Json as SqlJson,
    Connection, PgConnection, PgPool, Row,
};
use tokio::{
    sync::{mpsc, watch},
    task::{JoinHandle, JoinSet},
    time,
};
use tracing::{debug, info, warn};
use tracing_subscriber::fmt::writer::MakeWriterExt;
use uuid::Uuid;
use vpsman_common::{
    encode_json, job_command_operation_type, payload_hash, read_secret_file_ref,
    AgentCapabilitySnapshot, JobCommand, SuiteConfig, DEFAULT_MAX_JOB_TIMEOUT_SECS,
    MAX_ARTIFACT_CLEANUP_REVIEWED_TARGETS, MAX_CONFIGURABLE_JOB_TIMEOUT_SECS,
    SERVER_JOB_STATUS_COMPLETED, SERVER_JOB_STATUS_FAILED, SERVER_JOB_STATUS_QUEUED,
    SERVER_JOB_STATUS_RUNNING, SERVER_JOB_TYPE_ARTIFACT_CLEANUP, TRAFFIC_COUNTER_HISTORY_TIERS,
};
#[cfg(test)]
use vpsman_common::{
    expression_matches, parse_expression, plan_tunnel, AgentPrivilegeMode, ExpressionContext,
    TunnelAddressPair, TunnelEndpointSide, TunnelKind, TunnelPlanInput, VpsMetadata,
};
use vpsman_object_store::{BackupObjectStore, S3BackupObjectStoreSettings};
use vpsman_server_core::{
    job_command_type_label, scheduled_command_type_label, split_targets_by_capability,
    validate_network_command_targets, CapabilitySkip, TargetCapability, JOB_STATUS_QUEUED,
    JOB_STATUS_SKIPPED, TARGET_STATUS_QUEUED, TARGET_STATUS_SKIPPED,
};

const DEFAULT_BACKUP_OBJECT_STORE_DIR: &str = "runtime/data/objects/backups";
const SQLX_METADATA_SCHEMA: &str = "vpsman_internal";
const SQLX_METADATA_SCHEMA_LOCK_KEY: i64 = 0x5650_534d_5351_4c58;
// Lost NOTIFY messages can move an exact cached retention frontier earlier.
// Reconnect is therefore bounded by the backend catch-up contract; this is
// transport recovery, not a retention-owner cadence.
const WORKER_NOTIFICATION_RECONNECT_DELAY: Duration = Duration::from_secs(5);
const OFFLINE_BATCH: usize = 100;
const OFFLINE_CANDIDATE_SQL: &str = r#"
    SELECT id
    FROM clients
    WHERE hidden_at IS NULL
      AND status = 'online'
      AND last_seen_at < now() - make_interval(secs => $1)
    ORDER BY last_seen_at ASC
    LIMIT 1
    FOR UPDATE SKIP LOCKED
"#;
const SCHEDULE_CRON_INVALID: &str = "schedule_cron_invalid";
const SCHEDULE_CRON_NO_FUTURE_OCCURRENCE: &str = "schedule_cron_no_future_occurrence";
const SCHEDULE_OPERATION_INVALID: &str = "schedule_operation_invalid";
#[path = "runtime/actor_authority.rs"]
mod actor_authority;
#[path = "runtime/alert_event_schedules.rs"]
mod alert_event_schedules;
#[path = "delivery/alert_notifications.rs"]
mod alert_notifications;
#[path = "runtime/alert_policy_retention.rs"]
mod alert_policy_retention;
#[path = "runtime/artifact_deletion.rs"]
mod artifact_deletion;
#[path = "retention/backup_policy_retention.rs"]
mod backup_policy_retention;
#[path = "runtime/build_info.rs"]
mod build_info;
#[path = "retention/history_retention.rs"]
mod history_retention;
#[path = "retention/network_observation_retention.rs"]
mod network_observation_retention;
#[path = "runtime/operational_alerts.rs"]
mod operational_alerts;
#[path = "retention/telemetry_minute_materialization.rs"]
mod telemetry_minute_materialization;
#[cfg(test)]
#[path = "runtime/test_support.rs"]
mod test_support;
#[path = "retention/traffic_retention.rs"]
mod traffic_retention;
#[path = "delivery/webhook_rules.rs"]
mod webhook_rules;

fn telemetry_retention_pool_options() -> PgPoolOptions {
    PgPoolOptions::new()
        .min_connections(0)
        .max_connections(2)
        .after_connect(|connection, _metadata| {
            Box::pin(async move {
                // Retention boundaries and durable cursors make these plans
                // intrinsically parameter-selective. A generic prepared plan
                // can turn a bounded time seek into a retained-history scan.
                sqlx::query("SET plan_cache_mode = force_custom_plan")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
}
use actor_authority::{actor_authorized, actor_authorized_in_tx};
use alert_event_schedules::process_alert_event_schedules;
use alert_notifications::{
    process_alert_notifications, process_due_alert_notifications, AlertNotificationWorkerConfig,
    AlertNotificationWorkerRun,
};
use alert_policy_retention::{process_alert_policy_retention, AlertPolicyRetentionConfig};
use artifact_deletion::{
    artifact_deletion_completion_signal, claim_artifact_deletion, defer_artifact_deletion,
    enqueue_artifact_deletion, fail_artifact_deletion, finish_artifact_deletion_in_tx,
    lock_owned_artifact_deletion_in_tx, publish_artifact_deletion_completion,
    publish_artifact_deletion_completion_in_tx, spawn_artifact_deletion_heartbeat,
    wait_for_artifact_deletion_work, ArtifactDeletionOwner, ArtifactDeletionReview,
    ARTIFACT_DELETION_COMPLETED_CHANNEL,
};
use backup_policy_retention::{
    delete_backup_policy_artifact, process_backup_policy_retention_prune,
    BackupPolicyRetentionPruneConfig,
};
use history_retention::{
    TelemetryHistoryRetentionDrain, TelemetryHistoryRetentionPage,
    TelemetryHistoryRetentionPageReadiness, TelemetryHistoryRetentionRun,
    TelemetryHistoryRetentionStep, TELEMETRY_HISTORY_RETENTION_RECOVERY_INTERVAL,
};
use operational_alerts::{
    reconcile_agent_status_transition_in_tx, reconcile_scheduled_job_event_sources_in_tx,
};
use traffic_retention::{
    process_next_traffic_active_cycle_rebuild, TrafficActiveCycleRebuildOutcome,
};
use webhook_rules::{
    insert_webhook_event_in_tx, insert_webhook_event_with_provenance_at_in_tx,
    process_due_webhook_deliveries, process_telemetry_webhook_materialization_work,
    process_webhook_event_materialization_work, process_webhook_periodic_maintenance,
    process_webhook_rules, WebhookRuleWorkerConfig,
};

#[derive(Clone, Debug, Parser)]
#[command(name = "vpsman-worker", about = "Background scheduler for vpsman")]
struct Args {
    #[arg(
        long,
        env = "VPSMAN_SUITE_CONFIG",
        default_value = "config/vpsman.toml"
    )]
    suite_config: PathBuf,
    #[arg(long, env = "VPSMAN_WORKER_TICK_SECS", default_value_t = 30)]
    tick_secs: u64,
    #[arg(long, env = "VPSMAN_POSTGRES_URL")]
    postgres_url: Option<String>,
    #[arg(long, env = "VPSMAN_MIGRATIONS_DIR", default_value = "migrations")]
    migrations_dir: PathBuf,
    #[arg(long, env = "VPSMAN_WORKER_DB_MAX_CONNECTIONS", default_value_t = 8)]
    db_max_connections: u32,
    #[arg(long, env = "VPSMAN_WORKER_ONCE", default_value_t = false)]
    once: bool,
    #[arg(long, env = "VPSMAN_AGENT_OFFLINE_TIMEOUT_SECS", default_value_t = 300)]
    agent_offline_timeout_secs: i64,
    #[arg(
        long,
        env = "VPSMAN_WORKER_NOTIFICATION_DELIVERY_LIMIT",
        default_value_t = 25
    )]
    notification_delivery_limit: i64,
    #[arg(
        long,
        env = "VPSMAN_WORKER_NOTIFICATION_RETENTION_DAYS",
        default_value_t = 90
    )]
    notification_retention_days: i64,
    #[arg(
        long,
        env = "VPSMAN_WORKER_NOTIFICATION_RETENTION_PRUNE_LIMIT",
        default_value_t = 1000
    )]
    notification_retention_prune_limit: i64,
    #[arg(
        long,
        env = "VPSMAN_WORKER_NOTIFICATION_WEBHOOK_TIMEOUT_SECS",
        default_value_t = 5
    )]
    notification_webhook_timeout_secs: u64,
    #[arg(
        long,
        env = "VPSMAN_WORKER_WEBHOOK_RULE_DELIVERY_LIMIT",
        default_value_t = 25
    )]
    webhook_rule_delivery_limit: i64,
    #[arg(
        long,
        env = "VPSMAN_WORKER_WEBHOOK_RULE_MATERIALIZE_LIMIT",
        default_value_t = 100
    )]
    webhook_rule_materialize_limit: i64,
    #[arg(
        long,
        env = "VPSMAN_WORKER_WEBHOOK_RULE_RETENTION_DAYS",
        default_value_t = 90
    )]
    webhook_rule_retention_days: i64,
    #[arg(
        long,
        env = "VPSMAN_WORKER_WEBHOOK_RULE_RETENTION_PRUNE_LIMIT",
        default_value_t = 1000
    )]
    webhook_rule_retention_prune_limit: i64,
    #[arg(
        long,
        env = "VPSMAN_WORKER_WEBHOOK_RULE_TIMEOUT_SECS",
        default_value_t = 5
    )]
    webhook_rule_timeout_secs: u64,
    #[arg(
        long,
        env = "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_ENABLED",
        default_value_t = false
    )]
    backup_policy_prune_enabled: bool,
    #[arg(
        long,
        env = "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_LIMIT",
        default_value_t = 50
    )]
    backup_policy_prune_limit: i64,
    #[arg(
        long,
        env = "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_DRY_RUN",
        default_value_t = false
    )]
    backup_policy_prune_dry_run: bool,
    #[arg(
        long,
        env = "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_INCLUDE_DISABLED",
        default_value_t = false
    )]
    backup_policy_prune_include_disabled: bool,
    #[arg(
        long,
        env = "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_DELETE_OBJECTS",
        default_value_t = false
    )]
    backup_policy_prune_delete_objects: bool,
    #[arg(long, env = "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_OBJECT_STORE_DIR")]
    backup_policy_prune_object_store_dir: Option<PathBuf>,
    #[arg(long, env = "VPSMAN_BACKUP_OBJECT_STORE_DIR")]
    backup_object_store_dir: Option<PathBuf>,
    #[arg(long, env = "VPSMAN_OBJECT_ENDPOINT")]
    object_endpoint: Option<String>,
    #[arg(long, env = "VPSMAN_OBJECT_BUCKET")]
    object_bucket: Option<String>,
    #[arg(long, env = "VPSMAN_OBJECT_ACCESS_KEY")]
    object_access_key: Option<String>,
    #[arg(long, env = "VPSMAN_OBJECT_SECRET_KEY")]
    object_secret_key: Option<String>,
    #[arg(long, env = "VPSMAN_OBJECT_REGION", default_value = "us-east-1")]
    object_region: String,
    #[arg(long, env = "VPSMAN_OBJECT_CREATE_BUCKET", default_value_t = false)]
    object_create_bucket: bool,
    #[arg(
        long,
        env = "VPSMAN_WORKER_SCHEDULE_JOB_MAX_TIMEOUT_SECS",
        default_value_t = DEFAULT_MAX_JOB_TIMEOUT_SECS
    )]
    schedule_job_max_timeout_secs: u64,
    #[arg(
        long,
        env = "VPSMAN_MAX_JOB_TIMEOUT_SECS",
        default_value_t = DEFAULT_MAX_JOB_TIMEOUT_SECS
    )]
    max_job_timeout_secs: u64,
    #[arg(
        long,
        env = "VPSMAN_REQUIRE_REGISTERED_AGENT_UPDATES",
        default_value_t = false
    )]
    require_registered_agent_updates: bool,
}

#[derive(Clone)]
struct WorkerRuntimeConfig {
    tick_secs: u64,
    agent_offline_timeout_secs: i64,
    alert_notification_config: AlertNotificationWorkerConfig,
    alert_policy_retention_config: AlertPolicyRetentionConfig,
    webhook_rule_config: WebhookRuleWorkerConfig,
    backup_policy_prune_config: BackupPolicyRetentionPruneConfig,
    schedule_dispatch_config: ScheduleDispatchConfig,
    backup_object_store: BackupObjectStore,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkerLoopWake {
    TelemetryProjectionWork,
    TrafficActiveCycleRebuildWork,
    WebhookWork,
    AlertNotificationWork,
    ArtifactDeletionCompletion,
}

#[derive(Debug, Default, Eq, PartialEq)]
struct TelemetryProjectionWakeScope {
    client_ids: Vec<String>,
    global: bool,
}

#[derive(Debug, Default)]
struct PendingTelemetryProjectionWakes {
    client_ids: HashSet<String>,
    global: bool,
}

struct WorkerNotificationPump {
    task: JoinHandle<Result<()>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TelemetryRetentionNotification {
    ProjectionMinute {
        ready_at_unix: i64,
        sample_prune_ready_at_unix: Option<i64>,
    },
    OrdinaryRollupPublished {
        domain: TelemetryRetentionRollupDomain,
        due_event_ready_at_unix: Option<i64>,
    },
    DueSpanPublished(TelemetryDueSpanWake),
    Effect(TelemetryRetentionEffect),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TelemetryDueSpanWake {
    domain: TelemetryRetentionRollupDomain,
    source_bucket_secs: i32,
    destination_bucket_secs: i32,
    due_at_unix: i64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum TelemetryRetentionRollupDomain {
    Resource,
    NetworkRate,
    Ping,
    SystemMetric,
    NetworkObservation,
}

impl TelemetryRetentionRollupDomain {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "telemetry_rollups" => Some(Self::Resource),
            "telemetry_network_rates" => Some(Self::NetworkRate),
            "telemetry_ping_rollups" => Some(Self::Ping),
            "system_metric_rollups" => Some(Self::SystemMetric),
            "network_observation_rollups" => Some(Self::NetworkObservation),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Resource => "telemetry_rollups",
            Self::NetworkRate => "telemetry_network_rates",
            Self::Ping => "telemetry_ping_rollups",
            Self::SystemMetric => "system_metric_rollups",
            Self::NetworkObservation => "network_observation_rollups",
        }
    }

    fn supports_edge(self, source_bucket_secs: i32, destination_bucket_secs: i32) -> bool {
        match self {
            Self::NetworkObservation => network_observation_retention::NETWORK_OBSERVATION_TIERS
                .iter()
                .any(|&(_, source, destination)| {
                    source == source_bucket_secs && destination == destination_bucket_secs
                }),
            _ => vpsman_common::TELEMETRY_HISTORY_TIERS
                .windows(2)
                .any(|tiers| {
                    tiers[0].bucket_secs == source_bucket_secs
                        && tiers[1].bucket_secs == destination_bucket_secs
                }),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum TelemetryRetentionPolicyDomain {
    Resource,
    NetworkRate,
    Ping,
    SystemMetric,
    NetworkObservation,
    TrafficCounter,
}

impl TelemetryRetentionPolicyDomain {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "telemetry_rollups" => Some(Self::Resource),
            "telemetry_network_rates" => Some(Self::NetworkRate),
            "telemetry_ping_rollups" => Some(Self::Ping),
            "system_metric_rollups" => Some(Self::SystemMetric),
            "network_observations" => Some(Self::NetworkObservation),
            "traffic_counter_rollups" => Some(Self::TrafficCounter),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Resource => "telemetry_rollups",
            Self::NetworkRate => "telemetry_network_rates",
            Self::Ping => "telemetry_ping_rollups",
            Self::SystemMetric => "system_metric_rollups",
            Self::NetworkObservation => "network_observations",
            Self::TrafficCounter => "traffic_counter_rollups",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TelemetryRetentionEffect {
    CoreMinuteFrontierAdvanced,
    TrafficMinuteFrontierAdvanced,
    PingFactsPublished,
    PingFactsDeleted,
    PingCurrentDeleted,
    TelemetrySamplesDeleted,
    SamplePruneFrontierAdvanced,
    NetworkObservationHistoryPublished,
    NetworkObservationSeriesDeactivated,
    NetworkObservationLatestDeleted,
    TrafficSamplesPublished,
    TrafficRollupPublished {
        bucket_secs: i32,
    },
    RetentionPolicyChanged {
        domain: TelemetryRetentionPolicyDomain,
    },
    PingTopologyChanged,
    PingRollupsDeleted,
    NetworkObservationHistoryDeleted,
}

#[derive(Debug, Default, Eq, PartialEq)]
struct TelemetryRetentionWakeScope {
    projection_minute_ready_at_unix: Option<i64>,
    due_event_ready_at_unix: Option<i64>,
    ordinary_rollup_domains: Vec<TelemetryRetentionRollupDomain>,
    due_spans: Vec<TelemetryDueSpanWake>,
    core_minute_frontier_advanced: bool,
    traffic_minute_frontier_advanced: bool,
    ping_facts_published: bool,
    ping_facts_deleted: bool,
    ping_current_deleted: bool,
    telemetry_samples_deleted: bool,
    sample_prune_ready_at_unix: Option<i64>,
    sample_prune_frontier_advanced: bool,
    network_observation_history_published: bool,
    network_observation_series_deactivated: bool,
    traffic_samples_published: bool,
    traffic_rollup_bucket_secs: Vec<i32>,
    retention_policy_domains: Vec<TelemetryRetentionPolicyDomain>,
    ping_topology_changed: bool,
    ping_rollups_deleted: bool,
    network_observation_history_deleted: bool,
    network_observation_latest_deleted: bool,
    recover_external_writer_frontiers: bool,
}

#[derive(Debug, Default)]
struct PendingTelemetryRetentionWakes {
    projection_minute_ready_at_unix: Option<i64>,
    due_event_ready_at_unix: Option<i64>,
    ordinary_rollup_domains: HashSet<TelemetryRetentionRollupDomain>,
    due_spans: Vec<TelemetryDueSpanWake>,
    core_minute_frontier_advanced: bool,
    traffic_minute_frontier_advanced: bool,
    ping_facts_published: bool,
    ping_facts_deleted: bool,
    ping_current_deleted: bool,
    telemetry_samples_deleted: bool,
    sample_prune_ready_at_unix: Option<i64>,
    sample_prune_frontier_advanced: bool,
    network_observation_history_published: bool,
    network_observation_series_deactivated: bool,
    traffic_samples_published: bool,
    traffic_rollup_bucket_secs: HashSet<i32>,
    retention_policy_domains: HashSet<TelemetryRetentionPolicyDomain>,
    ping_topology_changed: bool,
    ping_rollups_deleted: bool,
    network_observation_history_deleted: bool,
    network_observation_latest_deleted: bool,
    recover_external_writer_frontiers: bool,
}

#[derive(Clone)]
struct TelemetryRetentionWakeSender {
    wake_tx: mpsc::Sender<()>,
    pending: Arc<Mutex<PendingTelemetryRetentionWakes>>,
}

struct TelemetryRetentionWakeReceiver {
    wake_rx: mpsc::Receiver<()>,
    pending: Arc<Mutex<PendingTelemetryRetentionWakes>>,
}

#[derive(Clone)]
struct WorkerWakeSenders {
    telemetry_projection_tx: mpsc::Sender<()>,
    telemetry_projection_pending: Arc<Mutex<PendingTelemetryProjectionWakes>>,
    traffic_active_cycle_rebuild_tx: mpsc::Sender<()>,
    webhook_event_tx: mpsc::Sender<()>,
    webhook_delivery_tx: mpsc::Sender<()>,
    alert_notification_tx: mpsc::Sender<()>,
    telemetry_retention: TelemetryRetentionWakeSender,
}

struct WorkerWakeReceivers {
    telemetry_projection_rx: mpsc::Receiver<()>,
    telemetry_projection_pending: Arc<Mutex<PendingTelemetryProjectionWakes>>,
    traffic_active_cycle_rebuild_rx: mpsc::Receiver<()>,
    webhook_event_rx: mpsc::Receiver<()>,
    webhook_delivery_rx: mpsc::Receiver<()>,
    alert_notification_rx: mpsc::Receiver<()>,
}

impl WorkerNotificationPump {
    fn spawn(mut listener: PgListener, senders: WorkerWakeSenders) -> Self {
        let task_telemetry_projection_pending = Arc::clone(&senders.telemetry_projection_pending);
        let task = tokio::spawn(async move {
            loop {
                let notification = listener
                    .recv()
                    .await
                    .context("worker notification listener receive failed")?;
                debug!(
                    channel = notification.channel(),
                    payload = notification.payload(),
                    "PostgreSQL notification woke worker"
                );
                if !queue_telemetry_retention_notification(
                    &senders.telemetry_retention,
                    notification.channel(),
                    notification.payload(),
                ) {
                    return Ok(());
                }
                let queued =
                    match notification_work_owner(notification.channel(), notification.payload()) {
                        Some(WorkerLoopWake::TelemetryProjectionWork) => {
                            queue_telemetry_projection_wake(
                                &task_telemetry_projection_pending,
                                &senders.telemetry_projection_tx,
                                notification.payload(),
                            )
                        }
                        Some(WorkerLoopWake::TrafficActiveCycleRebuildWork) => {
                            coalesce_worker_wake(&senders.traffic_active_cycle_rebuild_tx)
                        }
                        Some(WorkerLoopWake::WebhookWork) => {
                            let event_open = coalesce_worker_wake(&senders.webhook_event_tx);
                            let delivery_open = coalesce_worker_wake(&senders.webhook_delivery_tx);
                            event_open && delivery_open
                        }
                        Some(WorkerLoopWake::AlertNotificationWork) => {
                            coalesce_worker_wake(&senders.alert_notification_tx)
                        }
                        Some(WorkerLoopWake::ArtifactDeletionCompletion) => {
                            publish_artifact_deletion_completion();
                            true
                        }
                        _ => continue,
                    };
                if !queued {
                    return Ok(());
                }
            }
        });
        Self { task }
    }
}

fn telemetry_retention_wake_channel(
) -> (TelemetryRetentionWakeSender, TelemetryRetentionWakeReceiver) {
    let (wake_tx, wake_rx) = mpsc::channel(1);
    let pending = Arc::new(Mutex::new(PendingTelemetryRetentionWakes::default()));
    (
        TelemetryRetentionWakeSender {
            wake_tx,
            pending: Arc::clone(&pending),
        },
        TelemetryRetentionWakeReceiver { wake_rx, pending },
    )
}

fn worker_wake_channels(
    telemetry_retention: TelemetryRetentionWakeSender,
) -> (WorkerWakeSenders, WorkerWakeReceivers) {
    // Tokens are process-local latency hints. Exact durable rows remain the
    // authority, and every receiver also performs periodic database recovery.
    let (telemetry_projection_tx, telemetry_projection_rx) = mpsc::channel(1);
    let telemetry_projection_pending =
        Arc::new(Mutex::new(PendingTelemetryProjectionWakes::default()));
    let (traffic_active_cycle_rebuild_tx, traffic_active_cycle_rebuild_rx) = mpsc::channel(1);
    let (webhook_event_tx, webhook_event_rx) = mpsc::channel(1);
    let (webhook_delivery_tx, webhook_delivery_rx) = mpsc::channel(1);
    let (alert_notification_tx, alert_notification_rx) = mpsc::channel(1);
    (
        WorkerWakeSenders {
            telemetry_projection_tx,
            telemetry_projection_pending: Arc::clone(&telemetry_projection_pending),
            traffic_active_cycle_rebuild_tx,
            webhook_event_tx,
            webhook_delivery_tx,
            alert_notification_tx,
            telemetry_retention,
        },
        WorkerWakeReceivers {
            telemetry_projection_rx,
            telemetry_projection_pending,
            traffic_active_cycle_rebuild_rx,
            webhook_event_rx,
            webhook_delivery_rx,
            alert_notification_rx,
        },
    )
}

fn parse_telemetry_retention_notification(
    channel: &str,
    payload: &str,
) -> Option<TelemetryRetentionNotification> {
    let payload = serde_json::from_str::<Value>(payload).ok()?;
    match channel {
        "vpsman_telemetry_projection"
            if payload.get("owner").is_none()
                && payload
                    .get("client_id")
                    .and_then(Value::as_str)
                    .is_some_and(|client_id| !client_id.is_empty())
                && payload
                    .get("projected_seq")
                    .and_then(Value::as_i64)
                    .is_some() =>
        {
            let ready_at_unix = payload
                .get("retention_minute_ready_at_unix")
                .and_then(Value::as_i64)?;
            DateTime::<Utc>::from_timestamp(ready_at_unix, 0)?;
            let sample_prune_ready_at_unix = match payload.get("sample_prune_ready_at_unix") {
                None | Some(Value::Null) => None,
                Some(value) => {
                    let ready_at_unix = value.as_i64()?;
                    DateTime::<Utc>::from_timestamp(ready_at_unix, 0)?;
                    Some(ready_at_unix)
                }
            };
            Some(TelemetryRetentionNotification::ProjectionMinute {
                ready_at_unix,
                sample_prune_ready_at_unix,
            })
        }
        "vpsman_telemetry_retention"
            if payload.get("owner").and_then(Value::as_str) == Some("history_retention")
                && payload.get("effect").and_then(Value::as_str)
                    == Some("ordinary_rollup_published") =>
        {
            let domain = TelemetryRetentionRollupDomain::parse(
                payload.get("domain").and_then(Value::as_str)?,
            )?;
            let due_event_ready_at_unix = match payload.get("ready_at_unix") {
                None | Some(Value::Null) => None,
                Some(value) => {
                    let ready_at_unix = value.as_i64()?;
                    DateTime::<Utc>::from_timestamp(ready_at_unix, 0)?;
                    Some(ready_at_unix)
                }
            };
            Some(TelemetryRetentionNotification::OrdinaryRollupPublished {
                domain,
                due_event_ready_at_unix,
            })
        }
        "vpsman_telemetry_retention"
            if payload.get("owner").and_then(Value::as_str) == Some("history_retention")
                && payload.get("effect").and_then(Value::as_str) == Some("due_span_published") =>
        {
            let domain = TelemetryRetentionRollupDomain::parse(
                payload.get("domain").and_then(Value::as_str)?,
            )?;
            let source_bucket_secs =
                i32::try_from(payload.get("source_bucket_secs").and_then(Value::as_i64)?).ok()?;
            let destination_bucket_secs = i32::try_from(
                payload
                    .get("destination_bucket_secs")
                    .and_then(Value::as_i64)?,
            )
            .ok()?;
            domain
                .supports_edge(source_bucket_secs, destination_bucket_secs)
                .then_some(())?;
            let due_at_unix = payload.get("due_at_unix").and_then(Value::as_i64)?;
            DateTime::<Utc>::from_timestamp(due_at_unix, 0)?;
            Some(TelemetryRetentionNotification::DueSpanPublished(
                TelemetryDueSpanWake {
                    domain,
                    source_bucket_secs,
                    destination_bucket_secs,
                    due_at_unix,
                },
            ))
        }
        "vpsman_telemetry_retention"
            if payload.get("owner").and_then(Value::as_str) == Some("history_retention") =>
        {
            let effect = match payload.get("effect").and_then(Value::as_str)? {
                "core_minute_frontier_advanced" => {
                    TelemetryRetentionEffect::CoreMinuteFrontierAdvanced
                }
                "traffic_minute_frontier_advanced" => {
                    TelemetryRetentionEffect::TrafficMinuteFrontierAdvanced
                }
                "ping_facts_published" => TelemetryRetentionEffect::PingFactsPublished,
                "ping_facts_deleted" => TelemetryRetentionEffect::PingFactsDeleted,
                "ping_current_deleted" => TelemetryRetentionEffect::PingCurrentDeleted,
                "telemetry_samples_deleted" => TelemetryRetentionEffect::TelemetrySamplesDeleted,
                "sample_prune_frontier_advanced" => {
                    TelemetryRetentionEffect::SamplePruneFrontierAdvanced
                }
                "network_observation_history_published" => {
                    TelemetryRetentionEffect::NetworkObservationHistoryPublished
                }
                "network_observation_series_deactivated" => {
                    TelemetryRetentionEffect::NetworkObservationSeriesDeactivated
                }
                "traffic_samples_published" => TelemetryRetentionEffect::TrafficSamplesPublished,
                "traffic_rollup_published" => {
                    let bucket_secs =
                        i32::try_from(payload.get("bucket_secs").and_then(Value::as_i64)?).ok()?;
                    TRAFFIC_COUNTER_HISTORY_TIERS
                        .iter()
                        .any(|tier| tier.bucket_secs == bucket_secs)
                        .then_some(TelemetryRetentionEffect::TrafficRollupPublished {
                            bucket_secs,
                        })?
                }
                "retention_policy_changed" => TelemetryRetentionEffect::RetentionPolicyChanged {
                    domain: TelemetryRetentionPolicyDomain::parse(
                        payload.get("domain").and_then(Value::as_str)?,
                    )?,
                },
                "ping_topology_changed" => TelemetryRetentionEffect::PingTopologyChanged,
                "ping_rollups_deleted" => TelemetryRetentionEffect::PingRollupsDeleted,
                "network_observation_history_deleted" => {
                    TelemetryRetentionEffect::NetworkObservationHistoryDeleted
                }
                "network_observation_latest_deleted" => {
                    TelemetryRetentionEffect::NetworkObservationLatestDeleted
                }
                _ => return None,
            };
            Some(TelemetryRetentionNotification::Effect(effect))
        }
        _ => None,
    }
}

fn merge_earliest_unix(current: &mut Option<i64>, candidate: i64) {
    *current = Some(current.map_or(candidate, |current| current.min(candidate)));
}

fn queue_telemetry_retention_notification(
    sender: &TelemetryRetentionWakeSender,
    channel: &str,
    payload: &str,
) -> bool {
    let Some(notification) = parse_telemetry_retention_notification(channel, payload) else {
        return true;
    };
    queue_telemetry_retention_notification_effect(sender, notification)
}

fn queue_telemetry_retention_effect(
    sender: &TelemetryRetentionWakeSender,
    effect: TelemetryRetentionEffect,
) -> bool {
    let mut pending = match sender.pending.lock() {
        Ok(pending) => pending,
        Err(poisoned) => poisoned.into_inner(),
    };
    match effect {
        TelemetryRetentionEffect::CoreMinuteFrontierAdvanced => {
            pending.core_minute_frontier_advanced = true;
        }
        TelemetryRetentionEffect::TrafficMinuteFrontierAdvanced => {
            pending.traffic_minute_frontier_advanced = true;
        }
        TelemetryRetentionEffect::PingFactsPublished => {
            pending.ping_facts_published = true;
        }
        TelemetryRetentionEffect::PingFactsDeleted => {
            pending.ping_facts_deleted = true;
        }
        TelemetryRetentionEffect::PingCurrentDeleted => {
            pending.ping_current_deleted = true;
        }
        TelemetryRetentionEffect::TelemetrySamplesDeleted => {
            pending.telemetry_samples_deleted = true;
        }
        TelemetryRetentionEffect::SamplePruneFrontierAdvanced => {
            pending.sample_prune_frontier_advanced = true;
        }
        TelemetryRetentionEffect::NetworkObservationHistoryPublished => {
            pending.network_observation_history_published = true;
        }
        TelemetryRetentionEffect::NetworkObservationSeriesDeactivated => {
            pending.network_observation_series_deactivated = true;
        }
        TelemetryRetentionEffect::TrafficSamplesPublished => {
            pending.traffic_samples_published = true;
        }
        TelemetryRetentionEffect::TrafficRollupPublished { bucket_secs } => {
            pending.traffic_rollup_bucket_secs.insert(bucket_secs);
        }
        TelemetryRetentionEffect::RetentionPolicyChanged { domain } => {
            pending.retention_policy_domains.insert(domain);
        }
        TelemetryRetentionEffect::PingTopologyChanged => {
            pending.ping_topology_changed = true;
        }
        TelemetryRetentionEffect::PingRollupsDeleted => {
            pending.ping_rollups_deleted = true;
        }
        TelemetryRetentionEffect::NetworkObservationHistoryDeleted => {
            pending.network_observation_history_deleted = true;
        }
        TelemetryRetentionEffect::NetworkObservationLatestDeleted => {
            pending.network_observation_latest_deleted = true;
        }
    }
    coalesce_worker_wake(&sender.wake_tx)
}

fn queue_telemetry_retention_notification_effect(
    sender: &TelemetryRetentionWakeSender,
    notification: TelemetryRetentionNotification,
) -> bool {
    if let TelemetryRetentionNotification::Effect(effect) = notification {
        return queue_telemetry_retention_effect(sender, effect);
    }
    let mut pending = match sender.pending.lock() {
        Ok(pending) => pending,
        Err(poisoned) => poisoned.into_inner(),
    };
    match notification {
        TelemetryRetentionNotification::ProjectionMinute {
            ready_at_unix,
            sample_prune_ready_at_unix,
        } => {
            merge_earliest_unix(&mut pending.projection_minute_ready_at_unix, ready_at_unix);
            if let Some(sample_prune_ready_at_unix) = sample_prune_ready_at_unix {
                merge_earliest_unix(
                    &mut pending.sample_prune_ready_at_unix,
                    sample_prune_ready_at_unix,
                );
            }
        }
        TelemetryRetentionNotification::OrdinaryRollupPublished {
            domain,
            due_event_ready_at_unix,
        } => {
            pending.ordinary_rollup_domains.insert(domain);
            if let Some(ready_at_unix) = due_event_ready_at_unix {
                merge_earliest_unix(&mut pending.due_event_ready_at_unix, ready_at_unix);
            }
        }
        TelemetryRetentionNotification::DueSpanPublished(candidate) => {
            if let Some(existing) = pending.due_spans.iter_mut().find(|existing| {
                existing.domain == candidate.domain
                    && existing.source_bucket_secs == candidate.source_bucket_secs
                    && existing.destination_bucket_secs == candidate.destination_bucket_secs
            }) {
                existing.due_at_unix = existing.due_at_unix.min(candidate.due_at_unix);
            } else {
                pending.due_spans.push(candidate);
            }
        }
        TelemetryRetentionNotification::Effect(_) => {
            unreachable!("typed retention effects return before deadline merging")
        }
    }
    coalesce_worker_wake(&sender.wake_tx)
}

fn queue_telemetry_retention_external_writer_recovery(
    sender: &TelemetryRetentionWakeSender,
) -> bool {
    let mut pending = match sender.pending.lock() {
        Ok(pending) => pending,
        Err(poisoned) => poisoned.into_inner(),
    };
    pending.recover_external_writer_frontiers = true;
    coalesce_worker_wake(&sender.wake_tx)
}

fn take_telemetry_retention_wake_scope(
    pending: &Mutex<PendingTelemetryRetentionWakes>,
) -> TelemetryRetentionWakeScope {
    let mut pending = match pending.lock() {
        Ok(pending) => pending,
        Err(poisoned) => poisoned.into_inner(),
    };
    let pending = std::mem::take(&mut *pending);
    let mut traffic_rollup_bucket_secs = pending
        .traffic_rollup_bucket_secs
        .into_iter()
        .collect::<Vec<_>>();
    traffic_rollup_bucket_secs.sort_unstable();
    let mut ordinary_rollup_domains = pending
        .ordinary_rollup_domains
        .into_iter()
        .collect::<Vec<_>>();
    ordinary_rollup_domains.sort_unstable();
    let mut due_spans = pending.due_spans;
    due_spans.sort_unstable_by_key(|span| {
        (
            span.domain,
            span.source_bucket_secs,
            span.destination_bucket_secs,
        )
    });
    let mut retention_policy_domains = pending
        .retention_policy_domains
        .into_iter()
        .collect::<Vec<_>>();
    retention_policy_domains.sort_unstable();
    TelemetryRetentionWakeScope {
        projection_minute_ready_at_unix: pending.projection_minute_ready_at_unix,
        due_event_ready_at_unix: pending.due_event_ready_at_unix,
        ordinary_rollup_domains,
        due_spans,
        core_minute_frontier_advanced: pending.core_minute_frontier_advanced,
        traffic_minute_frontier_advanced: pending.traffic_minute_frontier_advanced,
        ping_facts_published: pending.ping_facts_published,
        ping_facts_deleted: pending.ping_facts_deleted,
        ping_current_deleted: pending.ping_current_deleted,
        telemetry_samples_deleted: pending.telemetry_samples_deleted,
        sample_prune_ready_at_unix: pending.sample_prune_ready_at_unix,
        sample_prune_frontier_advanced: pending.sample_prune_frontier_advanced,
        network_observation_history_published: pending.network_observation_history_published,
        network_observation_series_deactivated: pending.network_observation_series_deactivated,
        traffic_samples_published: pending.traffic_samples_published,
        traffic_rollup_bucket_secs,
        retention_policy_domains,
        ping_topology_changed: pending.ping_topology_changed,
        ping_rollups_deleted: pending.ping_rollups_deleted,
        network_observation_history_deleted: pending.network_observation_history_deleted,
        network_observation_latest_deleted: pending.network_observation_latest_deleted,
        recover_external_writer_frontiers: pending.recover_external_writer_frontiers,
    }
}

fn notification_work_owner(channel: &str, payload: &str) -> Option<WorkerLoopWake> {
    match (channel, payload) {
        ("vpsman_telemetry_projection", _) => Some(WorkerLoopWake::TelemetryProjectionWork),
        ("vpsman_traffic_active_cycle_rebuild", _) => {
            Some(WorkerLoopWake::TrafficActiveCycleRebuildWork)
        }
        ("webhook_events", "alert_notification") => Some(WorkerLoopWake::AlertNotificationWork),
        ("webhook_events", _) => Some(WorkerLoopWake::WebhookWork),
        (ARTIFACT_DELETION_COMPLETED_CHANNEL, _) => {
            Some(WorkerLoopWake::ArtifactDeletionCompletion)
        }
        _ => None,
    }
}

fn queue_telemetry_projection_wake(
    pending: &Mutex<PendingTelemetryProjectionWakes>,
    wake_tx: &mpsc::Sender<()>,
    payload: &str,
) -> bool {
    let payload = serde_json::from_str::<Value>(payload).ok();
    // Dashboard resident publications share this PostgreSQL channel with the
    // canonical telemetry projector, but own no webhook cursor work.
    if payload
        .as_ref()
        .and_then(|payload| payload.get("owner"))
        .and_then(Value::as_str)
        == Some("dashboard")
    {
        return true;
    }
    let client_id = payload
        .as_ref()
        .filter(|payload| {
            payload
                .get("projected_seq")
                .and_then(Value::as_i64)
                .is_some()
        })
        .and_then(|payload| payload.get("client_id"))
        .and_then(Value::as_str)
        .filter(|client_id| !client_id.is_empty())
        .map(str::to_owned);
    let mut pending = match pending.lock() {
        Ok(pending) => pending,
        Err(poisoned) => poisoned.into_inner(),
    };
    match client_id {
        Some(client_id) => {
            pending.client_ids.insert(client_id);
        }
        None => pending.global = true,
    }
    coalesce_worker_wake(wake_tx)
}

fn take_telemetry_projection_wake_scope(
    pending: &Mutex<PendingTelemetryProjectionWakes>,
) -> TelemetryProjectionWakeScope {
    let mut pending = match pending.lock() {
        Ok(pending) => pending,
        Err(poisoned) => poisoned.into_inner(),
    };
    let mut client_ids = pending.client_ids.drain().collect::<Vec<_>>();
    client_ids.sort();
    TelemetryProjectionWakeScope {
        client_ids,
        global: std::mem::take(&mut pending.global),
    }
}

fn telemetry_projection_scope_for_wake(
    pending: &Mutex<PendingTelemetryProjectionWakes>,
    periodic: bool,
) -> Option<TelemetryProjectionWakeScope> {
    let pending_scope = take_telemetry_projection_wake_scope(pending);
    (!periodic).then_some(pending_scope)
}

fn coalesce_worker_wake(wake_tx: &mpsc::Sender<()>) -> bool {
    match wake_tx.try_send(()) {
        Ok(()) | Err(mpsc::error::TrySendError::Full(())) => true,
        Err(mpsc::error::TrySendError::Closed(())) => false,
    }
}

#[cfg(test)]
mod worker_wake_owner_tests {
    use super::*;

    #[tokio::test]
    async fn notification_burst_has_one_pending_wake_token() {
        let (wake_tx, mut wake_rx) = mpsc::channel(1);
        for _ in 0..120 {
            assert!(coalesce_worker_wake(&wake_tx));
        }
        assert_eq!(wake_rx.try_recv(), Ok(()));
        assert!(matches!(
            wake_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        drop(wake_rx);
        assert!(!coalesce_worker_wake(&wake_tx));
    }

    #[tokio::test]
    async fn telemetry_projection_mailbox_losslessly_coalesces_exact_clients() {
        let pending = Mutex::new(PendingTelemetryProjectionWakes::default());
        let (wake_tx, mut wake_rx) = mpsc::channel(1);
        for payload in [
            r#"{"client_id":"client-b","generation":4,"projected_seq":7}"#,
            r#"{"client_id":"client-a","generation":3,"projected_seq":6}"#,
            r#"{"client_id":"client-b","generation":4,"projected_seq":8}"#,
        ] {
            assert!(queue_telemetry_projection_wake(&pending, &wake_tx, payload));
        }
        assert_eq!(wake_rx.recv().await, Some(()));
        assert_eq!(
            take_telemetry_projection_wake_scope(&pending),
            TelemetryProjectionWakeScope {
                client_ids: vec!["client-a".to_string(), "client-b".to_string()],
                global: false,
            }
        );
        assert!(matches!(
            wake_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));

        assert!(queue_telemetry_projection_wake(
            &pending,
            &wake_tx,
            r#"{"owner":"dashboard","client_id":"client-c","revision":9}"#,
        ));
        assert!(matches!(
            wake_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert_eq!(
            take_telemetry_projection_wake_scope(&pending),
            TelemetryProjectionWakeScope::default()
        );

        assert!(queue_telemetry_projection_wake(
            &pending,
            &wake_tx,
            "unrecognized-payload"
        ));
        assert_eq!(wake_rx.recv().await, Some(()));
        assert_eq!(
            take_telemetry_projection_wake_scope(&pending),
            TelemetryProjectionWakeScope {
                client_ids: Vec::new(),
                global: true,
            }
        );
    }

    #[tokio::test]
    async fn retention_wake_burst_preserves_each_earliest_owner_deadline() {
        let (wake_tx, mut wake_rx) = telemetry_retention_wake_channel();
        for (channel, payload) in [
            (
                "vpsman_telemetry_projection",
                r#"{"client_id":"client-a","projected_seq":1,"retention_minute_ready_at_unix":180,"sample_prune_ready_at_unix":250}"#,
            ),
            (
                "vpsman_telemetry_projection",
                r#"{"client_id":"client-b","projected_seq":2,"retention_minute_ready_at_unix":120,"sample_prune_ready_at_unix":150}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"ordinary_rollup_published","domain":"system_metric_rollups","ready_at_unix":600}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"ordinary_rollup_published","domain":"telemetry_rollups","ready_at_unix":300}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"ordinary_rollup_published","domain":"network_observation_rollups"}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"due_span_published","domain":"telemetry_rollups","source_bucket_secs":60,"destination_bucket_secs":300,"due_at_unix":900}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"due_span_published","domain":"telemetry_rollups","source_bucket_secs":60,"destination_bucket_secs":300,"due_at_unix":800}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"core_minute_frontier_advanced"}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"traffic_minute_frontier_advanced"}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"ping_facts_published"}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"ping_facts_deleted"}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"ping_current_deleted"}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"telemetry_samples_deleted"}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"network_observation_history_published"}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"network_observation_series_deactivated"}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"traffic_samples_published"}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"traffic_rollup_published","bucket_secs":86400}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"traffic_rollup_published","bucket_secs":3600}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"retention_policy_changed","domain":"network_observations"}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"retention_policy_changed","domain":"telemetry_rollups"}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"ping_topology_changed"}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"ping_rollups_deleted"}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"network_observation_history_deleted"}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"network_observation_latest_deleted"}"#,
            ),
        ] {
            assert!(queue_telemetry_retention_notification(
                &wake_tx, channel, payload
            ));
        }
        assert!(queue_telemetry_retention_external_writer_recovery(&wake_tx));
        assert_eq!(wake_rx.wake_rx.recv().await, Some(()));
        assert_eq!(
            take_telemetry_retention_wake_scope(&wake_rx.pending),
            TelemetryRetentionWakeScope {
                projection_minute_ready_at_unix: Some(120),
                due_event_ready_at_unix: Some(300),
                ordinary_rollup_domains: vec![
                    TelemetryRetentionRollupDomain::Resource,
                    TelemetryRetentionRollupDomain::SystemMetric,
                    TelemetryRetentionRollupDomain::NetworkObservation,
                ],
                due_spans: vec![TelemetryDueSpanWake {
                    domain: TelemetryRetentionRollupDomain::Resource,
                    source_bucket_secs: 60,
                    destination_bucket_secs: 300,
                    due_at_unix: 800,
                }],
                core_minute_frontier_advanced: true,
                traffic_minute_frontier_advanced: true,
                ping_facts_published: true,
                ping_facts_deleted: true,
                ping_current_deleted: true,
                telemetry_samples_deleted: true,
                sample_prune_ready_at_unix: Some(150),
                network_observation_history_published: true,
                network_observation_series_deactivated: true,
                traffic_samples_published: true,
                traffic_rollup_bucket_secs: vec![3_600, 86_400],
                retention_policy_domains: vec![
                    TelemetryRetentionPolicyDomain::Resource,
                    TelemetryRetentionPolicyDomain::NetworkObservation,
                ],
                ping_topology_changed: true,
                ping_rollups_deleted: true,
                network_observation_history_deleted: true,
                network_observation_latest_deleted: true,
                recover_external_writer_frontiers: true,
                ..TelemetryRetentionWakeScope::default()
            }
        );
        assert!(matches!(
            wake_rx.wake_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn retention_wake_parser_excludes_dashboard_and_malformed_hints() {
        assert_eq!(
            parse_telemetry_retention_notification(
                "vpsman_telemetry_projection",
                r#"{"client_id":"client-a","projected_seq":7,"retention_minute_ready_at_unix":120,"sample_prune_ready_at_unix":240}"#,
            ),
            Some(TelemetryRetentionNotification::ProjectionMinute {
                ready_at_unix: 120,
                sample_prune_ready_at_unix: Some(240),
            })
        );
        assert_eq!(
            parse_telemetry_retention_notification(
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"ordinary_rollup_published","domain":"system_metric_rollups","ready_at_unix":300}"#,
            ),
            Some(TelemetryRetentionNotification::OrdinaryRollupPublished {
                domain: TelemetryRetentionRollupDomain::SystemMetric,
                due_event_ready_at_unix: Some(300),
            })
        );
        assert_eq!(
            parse_telemetry_retention_notification(
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"ordinary_rollup_published","domain":"network_observation_rollups"}"#,
            ),
            Some(TelemetryRetentionNotification::OrdinaryRollupPublished {
                domain: TelemetryRetentionRollupDomain::NetworkObservation,
                due_event_ready_at_unix: None,
            })
        );
        assert_eq!(
            parse_telemetry_retention_notification(
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"traffic_rollup_published","bucket_secs":21600}"#,
            ),
            Some(TelemetryRetentionNotification::Effect(
                TelemetryRetentionEffect::TrafficRollupPublished {
                    bucket_secs: 21_600,
                }
            ))
        );
        assert_eq!(
            parse_telemetry_retention_notification(
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"retention_policy_changed","domain":"system_metric_rollups"}"#,
            ),
            Some(TelemetryRetentionNotification::Effect(
                TelemetryRetentionEffect::RetentionPolicyChanged {
                    domain: TelemetryRetentionPolicyDomain::SystemMetric,
                }
            ))
        );
        for (channel, payload) in [
            (
                "vpsman_telemetry_projection",
                r#"{"owner":"dashboard","client_id":"client-a","revision":1}"#,
            ),
            (
                "vpsman_telemetry_projection",
                r#"{"client_id":"client-a","projected_seq":7}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"traffic_rollup_published","bucket_secs":42}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"retention_policy_changed","domain":"telemetry_samples"}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"ordinary_rollup_published","domain":"traffic_counter_rollups"}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","effect":"due_span_published","domain":"telemetry_rollups","source_bucket_secs":60,"destination_bucket_secs":86400,"due_at_unix":300}"#,
            ),
            (
                "vpsman_telemetry_retention",
                r#"{"owner":"history_retention","phase":"due_event_coalescing","ready_at_unix":300}"#,
            ),
            ("vpsman_telemetry_retention", "not-json"),
        ] {
            assert_eq!(
                parse_telemetry_retention_notification(channel, payload),
                None
            );
        }
    }

    #[tokio::test]
    async fn retention_wake_page_boundary_consumes_external_effect_during_continuous_work() {
        let (wake_tx, mut wake_rx) = telemetry_retention_wake_channel();
        let mut drain = TelemetryHistoryRetentionDrain::new(Duration::from_secs(60));
        let mut applied_after_page = None;

        for page in 0..64 {
            if page == 17 {
                assert!(queue_telemetry_retention_effect(
                    &wake_tx,
                    TelemetryRetentionEffect::PingTopologyChanged,
                ));
            }
            match apply_ready_telemetry_retention_wake(&mut drain, &mut wake_rx) {
                TelemetryRetentionWakePoll::Applied => {
                    applied_after_page = Some(page);
                    break;
                }
                TelemetryRetentionWakePoll::Empty => {}
                TelemetryRetentionWakePoll::Closed => panic!("retention wake mailbox closed"),
            }
        }

        assert_eq!(applied_after_page, Some(17));
        assert!(matches!(
            wake_rx.wake_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert_eq!(
            take_telemetry_retention_wake_scope(&wake_rx.pending),
            TelemetryRetentionWakeScope::default()
        );
    }

    #[tokio::test]
    async fn periodic_projection_recovery_consumes_only_its_covered_hint_scope() {
        let pending = Mutex::new(PendingTelemetryProjectionWakes::default());
        let (wake_tx, mut wake_rx) = mpsc::channel(1);
        assert!(queue_telemetry_projection_wake(
            &pending,
            &wake_tx,
            r#"{"client_id":"covered","projected_seq":1}"#,
        ));

        assert_eq!(telemetry_projection_scope_for_wake(&pending, true), None);

        // The already-buffered token may be selected after the global drain,
        // but new commits still populate a fresh exact scope behind it.
        assert!(queue_telemetry_projection_wake(
            &pending,
            &wake_tx,
            r#"{"client_id":"fresh","projected_seq":2}"#,
        ));
        assert_eq!(wake_rx.recv().await, Some(()));
        assert_eq!(
            telemetry_projection_scope_for_wake(&pending, false),
            Some(TelemetryProjectionWakeScope {
                client_ids: vec!["fresh".to_string()],
                global: false,
            })
        );
    }

    #[test]
    fn notification_channels_wake_only_their_durable_work_owner() {
        assert_eq!(
            notification_work_owner("vpsman_telemetry_projection", "client-a"),
            Some(WorkerLoopWake::TelemetryProjectionWork)
        );
        assert_eq!(
            notification_work_owner("vpsman_traffic_active_cycle_rebuild", "ready"),
            Some(WorkerLoopWake::TrafficActiveCycleRebuildWork)
        );
        assert_eq!(
            notification_work_owner("webhook_events", "alert_notification"),
            Some(WorkerLoopWake::AlertNotificationWork)
        );
        assert_eq!(
            notification_work_owner("webhook_events", "alert_lifecycle"),
            Some(WorkerLoopWake::WebhookWork)
        );
        assert_eq!(
            notification_work_owner(ARTIFACT_DELETION_COMPLETED_CHANNEL, "source-id"),
            Some(WorkerLoopWake::ArtifactDeletionCompletion)
        );
        assert_eq!(
            notification_work_owner("vpsman_telemetry_retention", "due"),
            None
        );
        assert_eq!(notification_work_owner("unrelated", "event"), None);
    }
}

impl WorkerRuntimeConfig {
    fn from_args(args: &Args) -> Result<Self> {
        let backup_object_store = build_backup_object_store(args)?;
        let backup_policy_prune_object_store = build_backup_policy_prune_object_store(args)?
            .or_else(|| Some(backup_object_store.clone()));
        Ok(Self {
            tick_secs: args.tick_secs.max(1),
            agent_offline_timeout_secs: args.agent_offline_timeout_secs,
            alert_notification_config: AlertNotificationWorkerConfig::new(
                args.notification_delivery_limit,
                args.notification_retention_days,
                args.notification_retention_prune_limit,
                args.notification_webhook_timeout_secs,
            ),
            alert_policy_retention_config: AlertPolicyRetentionConfig::default(),
            webhook_rule_config: WebhookRuleWorkerConfig::new(
                args.webhook_rule_delivery_limit,
                args.webhook_rule_materialize_limit,
                args.webhook_rule_retention_days,
                args.webhook_rule_retention_prune_limit,
                args.webhook_rule_timeout_secs,
            )?,
            backup_policy_prune_config: BackupPolicyRetentionPruneConfig::new(
                args.backup_policy_prune_enabled,
                args.backup_policy_prune_limit,
                args.backup_policy_prune_dry_run,
                args.backup_policy_prune_include_disabled,
                args.backup_policy_prune_delete_objects,
                backup_policy_prune_object_store,
            ),
            schedule_dispatch_config: ScheduleDispatchConfig::new(
                args.schedule_job_max_timeout_secs,
                args.max_job_timeout_secs,
                args.require_registered_agent_updates,
            ),
            backup_object_store,
        })
    }
}

fn load_worker_runtime_config(base_args: &Args) -> Result<WorkerRuntimeConfig> {
    let mut args = base_args.clone();
    let suite_config =
        SuiteConfig::load_optional(&args.suite_config).map_err(anyhow::Error::msg)?;
    args.apply_suite_config(&suite_config)
        .map_err(anyhow::Error::msg)?;
    WorkerRuntimeConfig::from_args(&args)
}

impl Args {
    fn apply_suite_config(&mut self, config: &SuiteConfig) -> std::result::Result<(), String> {
        apply_opt_string(
            &mut self.postgres_url,
            "VPSMAN_POSTGRES_URL",
            config.database.postgres_url.as_deref(),
        );
        apply_path_default(
            &mut self.migrations_dir,
            "VPSMAN_MIGRATIONS_DIR",
            config.database.migrations_dir.as_deref(),
        );
        apply_u32_default(
            &mut self.db_max_connections,
            "VPSMAN_WORKER_DB_MAX_CONNECTIONS",
            config.capacity.worker_db_pool,
        );
        apply_u64_default(
            &mut self.tick_secs,
            "VPSMAN_WORKER_TICK_SECS",
            config.worker.tick_secs,
        );
        apply_bool_default(&mut self.once, "VPSMAN_WORKER_ONCE", config.worker.once);
        apply_i64_default(
            &mut self.agent_offline_timeout_secs,
            "VPSMAN_AGENT_OFFLINE_TIMEOUT_SECS",
            config.worker.agent_offline_timeout_secs,
        );
        apply_i64_default(
            &mut self.notification_delivery_limit,
            "VPSMAN_WORKER_NOTIFICATION_DELIVERY_LIMIT",
            config.worker.notification_delivery_limit,
        );
        apply_i64_default(
            &mut self.notification_retention_days,
            "VPSMAN_WORKER_NOTIFICATION_RETENTION_DAYS",
            config.worker.notification_retention_days,
        );
        apply_i64_default(
            &mut self.notification_retention_prune_limit,
            "VPSMAN_WORKER_NOTIFICATION_RETENTION_PRUNE_LIMIT",
            config.worker.notification_retention_prune_limit,
        );
        apply_u64_default(
            &mut self.notification_webhook_timeout_secs,
            "VPSMAN_WORKER_NOTIFICATION_WEBHOOK_TIMEOUT_SECS",
            config.worker.notification_webhook_timeout_secs,
        );
        apply_i64_default(
            &mut self.webhook_rule_delivery_limit,
            "VPSMAN_WORKER_WEBHOOK_RULE_DELIVERY_LIMIT",
            config.worker.webhook_rule_delivery_limit,
        );
        apply_i64_default(
            &mut self.webhook_rule_materialize_limit,
            "VPSMAN_WORKER_WEBHOOK_RULE_MATERIALIZE_LIMIT",
            config.worker.webhook_rule_materialize_limit,
        );
        apply_i64_default(
            &mut self.webhook_rule_retention_days,
            "VPSMAN_WORKER_WEBHOOK_RULE_RETENTION_DAYS",
            config.worker.webhook_rule_retention_days,
        );
        apply_i64_default(
            &mut self.webhook_rule_retention_prune_limit,
            "VPSMAN_WORKER_WEBHOOK_RULE_RETENTION_PRUNE_LIMIT",
            config.worker.webhook_rule_retention_prune_limit,
        );
        apply_u64_default(
            &mut self.webhook_rule_timeout_secs,
            "VPSMAN_WORKER_WEBHOOK_RULE_TIMEOUT_SECS",
            config.worker.webhook_rule_timeout_secs,
        );
        apply_bool_default(
            &mut self.backup_policy_prune_enabled,
            "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_ENABLED",
            config.worker.backup_policy_prune_enabled,
        );
        apply_i64_default(
            &mut self.backup_policy_prune_limit,
            "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_LIMIT",
            config.worker.backup_policy_prune_limit,
        );
        apply_bool_default(
            &mut self.backup_policy_prune_dry_run,
            "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_DRY_RUN",
            config.worker.backup_policy_prune_dry_run,
        );
        apply_bool_default(
            &mut self.backup_policy_prune_include_disabled,
            "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_INCLUDE_DISABLED",
            config.worker.backup_policy_prune_include_disabled,
        );
        apply_bool_default(
            &mut self.backup_policy_prune_delete_objects,
            "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_DELETE_OBJECTS",
            config.worker.backup_policy_prune_delete_objects,
        );
        apply_opt_path(
            &mut self.backup_policy_prune_object_store_dir,
            "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_OBJECT_STORE_DIR",
            config
                .worker
                .backup_policy_prune_object_store_dir
                .as_deref(),
        );
        apply_opt_path(
            &mut self.backup_object_store_dir,
            "VPSMAN_BACKUP_OBJECT_STORE_DIR",
            config.storage.backup_object_store_dir.as_deref(),
        );
        apply_opt_string(
            &mut self.object_endpoint,
            "VPSMAN_OBJECT_ENDPOINT",
            config.storage.object_endpoint.as_deref(),
        );
        apply_opt_string(
            &mut self.object_bucket,
            "VPSMAN_OBJECT_BUCKET",
            config.storage.object_bucket.as_deref(),
        );
        apply_string_default(
            &mut self.object_region,
            "VPSMAN_OBJECT_REGION",
            config.storage.object_region.as_deref(),
        );
        apply_bool_default(
            &mut self.object_create_bucket,
            "VPSMAN_OBJECT_CREATE_BUCKET",
            config.storage.object_create_bucket,
        );
        if self.object_access_key.is_none() && env_absent("VPSMAN_OBJECT_ACCESS_KEY") {
            self.object_access_key =
                read_secret_file_ref(config.secrets.object_access_key_file.as_deref())?;
        }
        if self.object_secret_key.is_none() && env_absent("VPSMAN_OBJECT_SECRET_KEY") {
            self.object_secret_key =
                read_secret_file_ref(config.secrets.object_secret_key_file.as_deref())?;
        }
        apply_u64_default(
            &mut self.schedule_job_max_timeout_secs,
            "VPSMAN_WORKER_SCHEDULE_JOB_MAX_TIMEOUT_SECS",
            config.worker.schedule_job_max_timeout_secs,
        );
        apply_u64_default(
            &mut self.max_job_timeout_secs,
            "VPSMAN_MAX_JOB_TIMEOUT_SECS",
            config.timeout.max_job_timeout_secs,
        );
        apply_bool_default(
            &mut self.require_registered_agent_updates,
            "VPSMAN_REQUIRE_REGISTERED_AGENT_UPDATES",
            config.worker.require_registered_agent_updates,
        );
        Ok(())
    }
}

fn build_backup_policy_prune_object_store(args: &Args) -> Result<Option<BackupObjectStore>> {
    args.backup_policy_prune_object_store_dir
        .clone()
        .filter(|path| !path.as_os_str().is_empty())
        .map(BackupObjectStore::filesystem)
        .transpose()
}

fn build_backup_object_store(args: &Args) -> Result<BackupObjectStore> {
    if let Some(store) = args
        .backup_object_store_dir
        .clone()
        .filter(|path| !path.as_os_str().is_empty())
        .map(BackupObjectStore::filesystem)
        .transpose()?
    {
        return Ok(store);
    }

    if let Some(store) = build_s3_object_store(
        &args.object_endpoint,
        &args.object_bucket,
        &args.object_access_key,
        &args.object_secret_key,
        &args.object_region,
        args.object_create_bucket,
        "S3 object storage requires VPSMAN_OBJECT_ENDPOINT, VPSMAN_OBJECT_BUCKET, VPSMAN_OBJECT_ACCESS_KEY, and VPSMAN_OBJECT_SECRET_KEY",
    )? {
        return Ok(store);
    }

    BackupObjectStore::filesystem(PathBuf::from(DEFAULT_BACKUP_OBJECT_STORE_DIR))
}

fn build_s3_object_store(
    endpoint: &Option<String>,
    bucket: &Option<String>,
    access_key: &Option<String>,
    secret_key: &Option<String>,
    region: &str,
    create_bucket: bool,
    incomplete_config_message: &'static str,
) -> Result<Option<BackupObjectStore>> {
    let s3_fields = [
        endpoint.as_deref(),
        bucket.as_deref(),
        access_key.as_deref(),
        secret_key.as_deref(),
    ];
    let s3_field_count = s3_fields
        .iter()
        .filter(|value| value.is_some_and(|value| !value.trim().is_empty()))
        .count();
    if s3_field_count == 0 {
        return Ok(None);
    }
    ensure!(s3_field_count == s3_fields.len(), incomplete_config_message);
    Ok(Some(BackupObjectStore::s3(S3BackupObjectStoreSettings {
        endpoint: endpoint.clone().unwrap_or_default(),
        bucket: bucket.clone().unwrap_or_default(),
        access_key: access_key.clone().unwrap_or_default(),
        secret_key: secret_key.clone().unwrap_or_default(),
        region: region.to_string(),
        create_bucket,
    })?))
}

fn env_absent(name: &str) -> bool {
    std::env::var_os(name).is_none()
}

fn apply_opt_string(target: &mut Option<String>, env_name: &str, value: Option<&str>) {
    if target.is_none() && env_absent(env_name) {
        if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
            *target = Some(value.to_string());
        }
    }
}

fn apply_opt_path(target: &mut Option<PathBuf>, env_name: &str, value: Option<&str>) {
    if target.is_none() && env_absent(env_name) {
        if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
            *target = Some(PathBuf::from(value));
        }
    }
}

fn apply_path_default(target: &mut PathBuf, env_name: &str, value: Option<&str>) {
    if env_absent(env_name) {
        if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
            *target = PathBuf::from(value);
        }
    }
}

fn apply_string_default(target: &mut String, env_name: &str, value: Option<&str>) {
    if env_absent(env_name) {
        if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
            *target = value.to_string();
        }
    }
}

fn apply_u32_default(target: &mut u32, env_name: &str, value: Option<u32>) {
    if env_absent(env_name) {
        if let Some(value) = value {
            *target = value;
        }
    }
}

fn apply_u64_default(target: &mut u64, env_name: &str, value: Option<u64>) {
    if env_absent(env_name) {
        if let Some(value) = value {
            *target = value;
        }
    }
}

fn apply_i64_default(target: &mut i64, env_name: &str, value: Option<i64>) {
    if env_absent(env_name) {
        if let Some(value) = value {
            *target = value;
        }
    }
}

fn apply_bool_default(target: &mut bool, env_name: &str, value: Option<bool>) {
    if !*target && env_absent(env_name) {
        if let Some(value) = value {
            *target = value;
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let log_writer = std::io::stderr
        .with_max_level(tracing::Level::WARN)
        .or_else(std::io::stdout.with_min_level(tracing::Level::INFO));
    tracing_subscriber::fmt()
        .with_writer(log_writer)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,vpsman_worker=info".into()),
        )
        .init();

    let mut args = Args::parse();
    let base_args = args.clone();
    let suite_config =
        SuiteConfig::load_optional(&args.suite_config).map_err(anyhow::Error::msg)?;
    args.apply_suite_config(&suite_config)
        .map_err(anyhow::Error::msg)?;
    info!(
        version = build_info::release_version(),
        server_build_number = build_info::server_build_number(),
        "worker build metadata"
    );
    let Some(postgres_url) = args.postgres_url.as_deref() else {
        if args.once {
            bail!("VPSMAN_POSTGRES_URL is required when --once is used");
        }
        bail!("VPSMAN_POSTGRES_URL is required; worker cannot process durable queues");
    };
    // Independent lanes never retain a pool connection while doing external
    // I/O. One connection is therefore valid; larger configured pools provide
    // real cross-lane database concurrency rather than a correctness fence.
    let db_max_connections = args.db_max_connections.clamp(1, 256);
    let pool = connect_postgres(postgres_url, &args.migrations_dir, db_max_connections).await?;
    info!(tick_secs = args.tick_secs, "worker started");
    if args.once {
        let runtime_config = WorkerRuntimeConfig::from_args(&args)?;
        let schedules_processed =
            process_due_schedule_work(&pool, 25, &runtime_config.schedule_dispatch_config).await?;
        let alert_notifications =
            process_alert_notification_work(&pool, runtime_config.alert_notification_config, true)
                .await?;
        let webhook_rules =
            process_webhook_rules(&pool, runtime_config.webhook_rule_config).await?;
        let alert_policy_retention =
            process_alert_policy_retention(&pool, runtime_config.alert_policy_retention_config)
                .await?;
        let backup_policy_prune = process_backup_policy_retention_prune(
            &pool,
            runtime_config.backup_policy_prune_config.clone(),
        )
        .await?;
        let mut backup_artifact_deletions = 0_u64;
        while process_next_artifact_deletion_intent(&pool, &runtime_config.backup_object_store)
            .await?
        {
            backup_artifact_deletions = backup_artifact_deletions.saturating_add(1);
            tokio::task::yield_now().await;
        }
        let telemetry_retention = process_telemetry_history_retention_drain(&pool).await?;
        let mut traffic_active_cycle_rebuilds = 0_u64;
        let mut traffic_active_cycle_rebuild_failures = 0_u64;
        loop {
            match process_next_traffic_active_cycle_rebuild(&pool).await? {
                TrafficActiveCycleRebuildOutcome::Current => break,
                TrafficActiveCycleRebuildOutcome::Published => {
                    traffic_active_cycle_rebuilds = traffic_active_cycle_rebuilds.saturating_add(1);
                }
                TrafficActiveCycleRebuildOutcome::Deferred { client_id, error } => {
                    traffic_active_cycle_rebuild_failures =
                        traffic_active_cycle_rebuild_failures.saturating_add(1);
                    warn!(%client_id, %error, "deferred traffic active-cycle rule projection");
                }
            }
            tokio::task::yield_now().await;
        }
        let artifact_cleanup = process_artifact_cleanup_jobs(&pool).await?;
        info!(
            schedules_processed,
            alert_notification_processed = alert_notifications.processed,
            alert_notification_delivered = alert_notifications.delivered,
            alert_notification_failed = alert_notifications.failed,
            alert_notification_pruned = alert_notifications.pruned,
            webhook_rule_materialized = webhook_rules.materialized,
            webhook_rule_processed = webhook_rules.processed,
            webhook_rule_delivered = webhook_rules.delivered,
            webhook_rule_failed = webhook_rules.failed,
            webhook_rule_pruned = webhook_rules.pruned,
            alert_policy_evidence_receipts_pruned = alert_policy_retention.evidence_receipts_pruned,
            alert_policy_evidence_pruned = alert_policy_retention.evidence_pruned,
            alert_lifecycle_events_pruned = alert_policy_retention.lifecycle_events_pruned,
            backup_policy_prune_policies = backup_policy_prune.policies_scanned,
            backup_policy_prune_matched = backup_policy_prune.matched_rows,
            backup_policy_prune_pruned = backup_policy_prune.pruned_rows,
            backup_artifact_deletions,
            telemetry_core_minute_source_rows = telemetry_retention.core_minute_source_rows,
            telemetry_core_minute_derived_rows = telemetry_retention.core_minute_derived_rows,
            traffic_minute_source_rows = telemetry_retention.traffic_minute_source_rows,
            traffic_minute_derived_rows = telemetry_retention.traffic_minute_derived_rows,
            telemetry_samples_pruned = telemetry_retention.samples_pruned,
            telemetry_resource_spans_merged = telemetry_retention.resource_spans_merged,
            telemetry_rollups_pruned = telemetry_retention.rollups_pruned,
            telemetry_network_rate_spans_merged = telemetry_retention.network_rate_spans_merged,
            telemetry_network_rates_pruned = telemetry_retention.network_rates_pruned,
            telemetry_ping_spans_merged = telemetry_retention.ping_spans_merged,
            telemetry_ping_rollups_pruned = telemetry_retention.ping_rollups_pruned,
            telemetry_ping_facts_pruned = telemetry_retention.ping_facts_pruned,
            telemetry_ping_current_pruned = telemetry_retention.ping_current_pruned,
            telemetry_ping_series_pruned = telemetry_retention.ping_series_pruned,
            system_metric_rollups_pruned = telemetry_retention.system_metric_rollups_pruned,
            traffic_counter_samples_pruned = telemetry_retention.traffic_counter_samples_pruned,
            traffic_raw_rows_promoted = telemetry_retention.traffic_raw_rows_promoted,
            traffic_rollup_rows_promoted = telemetry_retention.traffic_rollup_rows_promoted,
            traffic_rollup_rows_pruned = telemetry_retention.traffic_rollup_rows_pruned,
            network_observation_source_rows_promoted =
                telemetry_retention.network_observation_source_rows_promoted,
            network_observation_destination_rows_written =
                telemetry_retention.network_observation_destination_rows_written,
            network_observation_expired_exact_rows_pruned =
                telemetry_retention.network_observation_expired_exact_rows_pruned,
            network_observation_expired_rollup_rows_pruned =
                telemetry_retention.network_observation_expired_rollup_rows_pruned,
            network_observation_inactive_latest_pruned =
                telemetry_retention.network_observation_inactive_latest_pruned,
            network_observation_inactive_series_pruned =
                telemetry_retention.network_observation_inactive_series_pruned,
            traffic_active_cycle_rebuilds,
            traffic_active_cycle_rebuild_failures,
            artifact_cleanup_jobs = artifact_cleanup.jobs,
            artifact_cleanup_failed_jobs = artifact_cleanup.failed_jobs,
            artifact_cleanup_deleted = artifact_cleanup.deleted_rows,
            "worker once completed"
        );
        return Ok(());
    }

    // Retention keeps a dedicated pool so bounded history work cannot consume
    // the ordinary workflow's minimum connection budget. Every retention page
    // now claims its exact durable owner; there is no outer lease connection.
    let telemetry_retention_pool = telemetry_retention_pool_options()
        .connect(postgres_url)
        .await
        .context("failed to connect telemetry retention scheduler to PostgreSQL")?;
    let (telemetry_retention_wake_tx, telemetry_retention_wake_rx) =
        telemetry_retention_wake_channel();
    let mut telemetry_retention_scheduler = TelemetryRetentionScheduler::spawn(
        telemetry_retention_pool,
        TELEMETRY_HISTORY_RETENTION_RECOVERY_INTERVAL,
        telemetry_retention_wake_rx,
    );
    let worker_result = tokio::select! {
        result = run_worker_loop(
            &pool,
            postgres_url,
            &base_args,
            &args,
            telemetry_retention_wake_tx,
        ) => result,
        result = telemetry_retention_scheduler.wait_for_unexpected_exit() => result,
    };
    let scheduler_result = telemetry_retention_scheduler.shutdown().await;
    scheduler_result?;
    worker_result
}

async fn run_worker_loop(
    pool: &PgPool,
    postgres_url: &str,
    base_args: &Args,
    args: &Args,
    telemetry_retention_wake: TelemetryRetentionWakeSender,
) -> Result<()> {
    let startup_runtime_config = WorkerRuntimeConfig::from_args(args)?;
    let (runtime_config_tx, runtime_config_rx) = watch::channel(startup_runtime_config.clone());
    let (wake_senders, wake_receivers) = worker_wake_channels(telemetry_retention_wake);
    let WorkerWakeReceivers {
        telemetry_projection_rx,
        telemetry_projection_pending,
        traffic_active_cycle_rebuild_rx,
        webhook_event_rx,
        webhook_delivery_rx,
        alert_notification_rx,
    } = wake_receivers;

    // These are fixed consumers, not per-wake tasks. A slow external target
    // can occupy only the durable queue that owns it; database producers and
    // unrelated owners continue independently. Process-local watches and wake
    // tokens affect latency only because every lane periodically rediscovers
    // its committed work in PostgreSQL.
    let mut lanes: JoinSet<(&'static str, Result<()>)> = JoinSet::new();

    let config_base_args = base_args.clone();
    lanes.spawn(async move {
        (
            "runtime-config-publisher",
            run_worker_config_publisher(
                config_base_args,
                startup_runtime_config,
                runtime_config_tx,
            )
            .await,
        )
    });

    let listener_postgres_url = postgres_url.to_string();
    let listener_runtime_config_rx = runtime_config_rx.clone();
    let listener_wake_senders = wake_senders.clone();
    lanes.spawn(async move {
        (
            "postgres-notification-listener",
            run_worker_notification_listener(
                listener_postgres_url,
                listener_runtime_config_rx,
                listener_wake_senders,
            )
            .await,
        )
    });

    let schedule_pool = pool.clone();
    let schedule_runtime_config_rx = runtime_config_rx.clone();
    lanes.spawn(async move {
        (
            "schedule-materialization",
            run_schedule_lane(schedule_pool, schedule_runtime_config_rx).await,
        )
    });

    let alert_pool = pool.clone();
    let alert_runtime_config_rx = runtime_config_rx.clone();
    lanes.spawn(async move {
        (
            "alert-notification-delivery",
            run_alert_notification_lane(alert_pool, alert_runtime_config_rx, alert_notification_rx)
                .await,
        )
    });

    let webhook_maintenance_pool = pool.clone();
    let webhook_maintenance_runtime_config_rx = runtime_config_rx.clone();
    let webhook_maintenance_event_tx = wake_senders.webhook_event_tx.clone();
    lanes.spawn(async move {
        (
            "webhook-periodic-maintenance",
            run_webhook_maintenance_lane(
                webhook_maintenance_pool,
                webhook_maintenance_runtime_config_rx,
                webhook_maintenance_event_tx,
            )
            .await,
        )
    });

    let webhook_event_pool = pool.clone();
    let webhook_event_runtime_config_rx = runtime_config_rx.clone();
    let webhook_event_delivery_tx = wake_senders.webhook_delivery_tx.clone();
    lanes.spawn(async move {
        (
            "webhook-event-materialization",
            run_webhook_event_materialization_lane(
                webhook_event_pool,
                webhook_event_runtime_config_rx,
                webhook_event_rx,
                webhook_event_delivery_tx,
            )
            .await,
        )
    });

    let telemetry_webhook_pool = pool.clone();
    let telemetry_webhook_runtime_config_rx = runtime_config_rx.clone();
    let telemetry_webhook_delivery_tx = wake_senders.webhook_delivery_tx.clone();
    lanes.spawn(async move {
        (
            "telemetry-webhook-materialization",
            run_telemetry_webhook_materialization_lane(
                telemetry_webhook_pool,
                telemetry_webhook_runtime_config_rx,
                telemetry_projection_rx,
                telemetry_projection_pending,
                telemetry_webhook_delivery_tx,
            )
            .await,
        )
    });

    let traffic_active_cycle_pool = pool.clone();
    let traffic_active_cycle_runtime_config_rx = runtime_config_rx.clone();
    lanes.spawn(async move {
        (
            "traffic-active-cycle-rebuild",
            run_traffic_active_cycle_rebuild_lane(
                traffic_active_cycle_pool,
                traffic_active_cycle_runtime_config_rx,
                traffic_active_cycle_rebuild_rx,
            )
            .await,
        )
    });

    let webhook_delivery_pool = pool.clone();
    let webhook_delivery_runtime_config_rx = runtime_config_rx.clone();
    lanes.spawn(async move {
        (
            "webhook-delivery",
            run_webhook_delivery_lane(
                webhook_delivery_pool,
                webhook_delivery_runtime_config_rx,
                webhook_delivery_rx,
            )
            .await,
        )
    });

    let alert_retention_pool = pool.clone();
    let alert_retention_runtime_config_rx = runtime_config_rx.clone();
    lanes.spawn(async move {
        (
            "alert-policy-retention",
            run_alert_policy_retention_lane(
                alert_retention_pool,
                alert_retention_runtime_config_rx,
            )
            .await,
        )
    });

    let client_maintenance_pool = pool.clone();
    let client_maintenance_runtime_config_rx = runtime_config_rx.clone();
    lanes.spawn(async move {
        (
            "client-session-maintenance",
            run_client_session_maintenance_lane(
                client_maintenance_pool,
                client_maintenance_runtime_config_rx,
            )
            .await,
        )
    });

    let artifact_pool = pool.clone();
    let artifact_runtime_config_rx = runtime_config_rx.clone();
    lanes.spawn(async move {
        (
            "artifact-intent-production",
            run_artifact_producer_lane(artifact_pool, artifact_runtime_config_rx).await,
        )
    });

    let artifact_deletion_pool = pool.clone();
    lanes.spawn(async move {
        (
            "artifact-object-deletion",
            run_artifact_deletion_lane(artifact_deletion_pool, runtime_config_rx).await,
        )
    });

    let Some(joined) = lanes.join_next().await else {
        bail!("worker has no active consumer lanes");
    };
    let (lane, result) = joined.context("worker consumer lane task failed")?;
    match result {
        Ok(()) => bail!("worker consumer lane {lane} exited unexpectedly"),
        Err(error) => Err(error).with_context(|| format!("worker consumer lane {lane} failed")),
    }
}

async fn run_worker_config_publisher(
    base_args: Args,
    startup_runtime_config: WorkerRuntimeConfig,
    runtime_config_tx: watch::Sender<WorkerRuntimeConfig>,
) -> Result<()> {
    let mut tick_secs = startup_runtime_config.tick_secs;
    let mut ticker = time::interval(Duration::from_secs(tick_secs));
    loop {
        ticker.tick().await;
        let runtime_config = match load_worker_runtime_config(&base_args) {
            Ok(config) => config,
            Err(error) => {
                warn!(%error, "failed to hot-reload worker suite config; using startup runtime config");
                startup_runtime_config.clone()
            }
        };
        runtime_config_tx.send_replace(runtime_config.clone());
        if runtime_config.tick_secs != tick_secs {
            tick_secs = runtime_config.tick_secs;
            let duration = Duration::from_secs(tick_secs);
            ticker = time::interval_at(tokio::time::Instant::now() + duration, duration);
            info!(tick_secs, "worker tick interval hot-reloaded");
        }
    }
}

async fn wait_for_worker_cycle(
    runtime_config_rx: &mut watch::Receiver<WorkerRuntimeConfig>,
) -> Result<WorkerRuntimeConfig> {
    runtime_config_rx
        .changed()
        .await
        .context("worker runtime config publisher stopped")?;
    Ok(runtime_config_rx.borrow_and_update().clone())
}

async fn wait_for_worker_cycle_or_hint(
    runtime_config_rx: &mut watch::Receiver<WorkerRuntimeConfig>,
    hint_rx: &mut mpsc::Receiver<()>,
) -> Result<(WorkerRuntimeConfig, bool)> {
    tokio::select! {
        changed = runtime_config_rx.changed() => {
            changed.context("worker runtime config publisher stopped")?;
            Ok((runtime_config_rx.borrow_and_update().clone(), true))
        }
        hint = hint_rx.recv() => {
            hint.context("worker wake channel stopped")?;
            Ok((runtime_config_rx.borrow().clone(), false))
        }
    }
}

async fn run_worker_notification_listener(
    postgres_url: String,
    mut runtime_config_rx: watch::Receiver<WorkerRuntimeConfig>,
    wake_senders: WorkerWakeSenders,
) -> Result<()> {
    loop {
        match connect_worker_notification_listener(&postgres_url).await {
            Ok(listener) => {
                // A live listener observes only future commits. Recover only
                // the named frontiers owned by commit notifications. Durable
                // rows remain authoritative when a disconnect loses hints.
                if !queue_telemetry_retention_external_writer_recovery(
                    &wake_senders.telemetry_retention,
                ) {
                    return Ok(());
                }
                publish_artifact_deletion_completion();
                runtime_config_rx.borrow_and_update();
                info!("worker PostgreSQL notification listener connected");
                let pump = WorkerNotificationPump::spawn(listener, wake_senders.clone());
                match pump.task.await {
                    Ok(Ok(())) => debug!("worker PostgreSQL notification listener stopped"),
                    Ok(Err(error)) => {
                        warn!(%error, "worker PostgreSQL notification listener failed; periodic recovery remains active");
                    }
                    Err(error) => {
                        warn!(%error, "worker PostgreSQL notification listener task failed; periodic recovery remains active");
                    }
                }
            }
            Err(error) => {
                debug!(%error, "worker PostgreSQL notification listener connect failed; periodic recovery remains active");
            }
        }
        wait_for_worker_notification_reconnect(&mut runtime_config_rx).await?;
    }
}

async fn wait_for_worker_notification_reconnect(
    runtime_config_rx: &mut watch::Receiver<WorkerRuntimeConfig>,
) -> Result<()> {
    tokio::select! {
        _ = time::sleep(WORKER_NOTIFICATION_RECONNECT_DELAY) => Ok(()),
        changed = runtime_config_rx.changed() => {
            changed.context("worker runtime config publisher stopped")?;
            runtime_config_rx.borrow_and_update();
            Ok(())
        }
    }
}

async fn run_schedule_lane(
    pool: PgPool,
    mut runtime_config_rx: watch::Receiver<WorkerRuntimeConfig>,
) -> Result<()> {
    loop {
        let runtime_config = wait_for_worker_cycle(&mut runtime_config_rx).await?;
        match process_due_schedule_work(&pool, 25, &runtime_config.schedule_dispatch_config).await {
            Ok(processed) if processed > 0 => info!(processed, "processed due schedules"),
            Ok(_) => {}
            Err(error) => warn!(%error, "failed to process due schedules"),
        }
    }
}

async fn run_alert_notification_lane(
    pool: PgPool,
    mut runtime_config_rx: watch::Receiver<WorkerRuntimeConfig>,
    mut hint_rx: mpsc::Receiver<()>,
) -> Result<()> {
    loop {
        let (runtime_config, periodic) =
            wait_for_worker_cycle_or_hint(&mut runtime_config_rx, &mut hint_rx).await?;
        match process_alert_notification_work(
            &pool,
            runtime_config.alert_notification_config,
            periodic,
        )
        .await
        {
            Ok(run) if run.processed > 0 || run.pruned > 0 => info!(
                processed = run.processed,
                delivered = run.delivered,
                failed = run.failed,
                pruned = run.pruned,
                "processed fleet alert notifications"
            ),
            Ok(_) => {}
            Err(error) => warn!(%error, "failed to process fleet alert notifications"),
        }
    }
}

async fn run_webhook_maintenance_lane(
    pool: PgPool,
    mut runtime_config_rx: watch::Receiver<WorkerRuntimeConfig>,
    webhook_event_tx: mpsc::Sender<()>,
) -> Result<()> {
    loop {
        let runtime_config = wait_for_worker_cycle(&mut runtime_config_rx).await?;
        match process_webhook_periodic_maintenance(&pool, runtime_config.webhook_rule_config).await
        {
            Ok(run) => {
                if run.materialized > 0 {
                    ensure!(
                        coalesce_worker_wake(&webhook_event_tx),
                        "webhook event materialization lane stopped"
                    );
                }
                if run.materialized > 0 || run.pruned > 0 {
                    info!(
                        materialized = run.materialized,
                        pruned = run.pruned,
                        "processed webhook periodic maintenance"
                    );
                }
            }
            Err(error) => warn!(%error, "failed to process webhook periodic maintenance"),
        }
    }
}

async fn run_webhook_event_materialization_lane(
    pool: PgPool,
    mut runtime_config_rx: watch::Receiver<WorkerRuntimeConfig>,
    mut hint_rx: mpsc::Receiver<()>,
    webhook_delivery_tx: mpsc::Sender<()>,
) -> Result<()> {
    loop {
        let (runtime_config, _) =
            wait_for_worker_cycle_or_hint(&mut runtime_config_rx, &mut hint_rx).await?;
        match process_webhook_event_materialization_work(&pool, runtime_config.webhook_rule_config)
            .await
        {
            Ok(run) => {
                if run.materialized > 0 {
                    ensure!(
                        coalesce_worker_wake(&webhook_delivery_tx),
                        "webhook delivery lane stopped"
                    );
                    info!(
                        materialized = run.materialized,
                        "materialized webhook event deliveries"
                    );
                }
            }
            Err(error) => warn!(%error, "failed to materialize webhook events"),
        }
    }
}

async fn run_telemetry_webhook_materialization_lane(
    pool: PgPool,
    mut runtime_config_rx: watch::Receiver<WorkerRuntimeConfig>,
    mut hint_rx: mpsc::Receiver<()>,
    pending: Arc<Mutex<PendingTelemetryProjectionWakes>>,
    webhook_delivery_tx: mpsc::Sender<()>,
) -> Result<()> {
    loop {
        let (runtime_config, periodic) =
            wait_for_worker_cycle_or_hint(&mut runtime_config_rx, &mut hint_rx).await?;
        // A periodic global recovery owns every cursor committed when its
        // drain begins. Consume the coalesced exact scope at that same
        // boundary so its still-buffered hint becomes a no-op rather than a
        // second, already-covered database pass. A projection committed
        // during/after this drain records a fresh scope and remains wakeable.
        let scope = telemetry_projection_scope_for_wake(&pending, periodic);
        let client_ids = match scope.as_ref() {
            None | Some(TelemetryProjectionWakeScope { global: true, .. }) => &[][..],
            Some(scope) if !scope.client_ids.is_empty() => scope.client_ids.as_slice(),
            Some(_) => continue,
        };
        match process_telemetry_webhook_materialization_work(
            &pool,
            runtime_config.webhook_rule_config,
            client_ids,
        )
        .await
        {
            Ok(run) => {
                if run.materialized > 0 {
                    ensure!(
                        coalesce_worker_wake(&webhook_delivery_tx),
                        "webhook delivery lane stopped"
                    );
                    info!(
                        materialized = run.materialized,
                        "materialized telemetry webhook deliveries"
                    );
                }
            }
            Err(error) => warn!(%error, "failed to materialize telemetry webhook deliveries"),
        }
    }
}

async fn run_webhook_delivery_lane(
    pool: PgPool,
    mut runtime_config_rx: watch::Receiver<WorkerRuntimeConfig>,
    mut hint_rx: mpsc::Receiver<()>,
) -> Result<()> {
    loop {
        let (runtime_config, _) =
            wait_for_worker_cycle_or_hint(&mut runtime_config_rx, &mut hint_rx).await?;
        match process_due_webhook_deliveries(&pool, runtime_config.webhook_rule_config).await {
            Ok(run) if run.processed > 0 => info!(
                processed = run.processed,
                delivered = run.delivered,
                failed = run.failed,
                "processed webhook deliveries"
            ),
            Ok(_) => {}
            Err(error) => warn!(%error, "failed to process webhook deliveries"),
        }
    }
}

async fn run_traffic_active_cycle_rebuild_lane(
    pool: PgPool,
    mut runtime_config_rx: watch::Receiver<WorkerRuntimeConfig>,
    mut hint_rx: mpsc::Receiver<()>,
) -> Result<()> {
    loop {
        let _ = wait_for_worker_cycle_or_hint(&mut runtime_config_rx, &mut hint_rx).await?;
        let mut rebuilt = 0_u64;
        let mut deferred = 0_u64;
        loop {
            match process_next_traffic_active_cycle_rebuild(&pool).await? {
                TrafficActiveCycleRebuildOutcome::Current => break,
                TrafficActiveCycleRebuildOutcome::Published => {
                    rebuilt = rebuilt.saturating_add(1);
                }
                TrafficActiveCycleRebuildOutcome::Deferred { client_id, error } => {
                    deferred = deferred.saturating_add(1);
                    warn!(%client_id, %error, "deferred traffic active-cycle rule projection");
                }
            }
            // Yield between independent client owners without delaying any
            // already-due work or imposing a throughput ceiling.
            tokio::task::yield_now().await;
        }
        if rebuilt > 0 || deferred > 0 {
            info!(
                rebuilt,
                deferred, "processed traffic active-cycle rule projections"
            );
        }
    }
}

async fn run_alert_policy_retention_lane(
    pool: PgPool,
    mut runtime_config_rx: watch::Receiver<WorkerRuntimeConfig>,
) -> Result<()> {
    loop {
        let runtime_config = wait_for_worker_cycle(&mut runtime_config_rx).await?;
        match process_alert_policy_retention(&pool, runtime_config.alert_policy_retention_config)
            .await
        {
            Ok(run) if run.evidence_pruned > 0 || run.lifecycle_events_pruned > 0 => info!(
                evidence_receipts_pruned = run.evidence_receipts_pruned,
                evidence_pruned = run.evidence_pruned,
                lifecycle_events_pruned = run.lifecycle_events_pruned,
                "processed alert policy retention"
            ),
            Ok(_) => {}
            Err(error) => warn!(%error, "failed to process alert policy retention"),
        }
    }
}

async fn run_client_session_maintenance_lane(
    pool: PgPool,
    mut runtime_config_rx: watch::Receiver<WorkerRuntimeConfig>,
) -> Result<()> {
    let mut last_offline_check = tokio::time::Instant::now();
    loop {
        let runtime_config = wait_for_worker_cycle(&mut runtime_config_rx).await?;
        if last_offline_check.elapsed() < Duration::from_secs(60) {
            continue;
        }
        match drain_offline_agents(&pool, runtime_config.agent_offline_timeout_secs).await {
            Ok(count) if count > 0 => info!(count, "detected offline agents"),
            Ok(_) => {}
            Err(error) => warn!(%error, "failed to detect offline agents"),
        }
        // This is idle/retry cadence from the completed drain, not a cap on
        // already-due per-client transitions.
        last_offline_check = tokio::time::Instant::now();
        match expire_stale_gateway_sessions(&pool, runtime_config.agent_offline_timeout_secs).await
        {
            Ok(count) if count > 0 => info!(count, "expired stale gateway sessions"),
            Ok(_) => {}
            Err(error) => warn!(%error, "failed to expire stale gateway sessions"),
        }
    }
}

async fn run_artifact_producer_lane(
    pool: PgPool,
    mut runtime_config_rx: watch::Receiver<WorkerRuntimeConfig>,
) -> Result<()> {
    loop {
        let runtime_config = wait_for_worker_cycle(&mut runtime_config_rx).await?;
        match process_backup_policy_retention_prune(
            &pool,
            runtime_config.backup_policy_prune_config.clone(),
        )
        .await
        {
            Ok(run) if run.matched_rows > 0 || run.pruned_rows > 0 => info!(
                policies_scanned = run.policies_scanned,
                matched_rows = run.matched_rows,
                pruned_rows = run.pruned_rows,
                "processed backup policy retention prune"
            ),
            Ok(_) => {}
            Err(error) => warn!(%error, "failed to process backup policy retention prune"),
        }
        match process_artifact_cleanup_jobs(&pool).await {
            Ok(run) if run.jobs > 0 || run.deleted_rows > 0 => info!(
                jobs = run.jobs,
                failed_jobs = run.failed_jobs,
                deleted_rows = run.deleted_rows,
                deleted_bytes = run.deleted_bytes,
                "processed artifact cleanup jobs"
            ),
            Ok(_) => {}
            Err(error) => warn!(%error, "failed to process artifact cleanup jobs"),
        }
    }
}

async fn run_artifact_deletion_lane(
    pool: PgPool,
    mut runtime_config_rx: watch::Receiver<WorkerRuntimeConfig>,
) -> Result<()> {
    loop {
        let runtime_config = runtime_config_rx.borrow().clone();
        let mut deleted = 0_u64;
        loop {
            match process_next_artifact_deletion_intent(&pool, &runtime_config.backup_object_store)
                .await
            {
                Ok(true) => {
                    deleted = deleted.saturating_add(1);
                    tokio::task::yield_now().await;
                }
                Ok(false) => break,
                Err(error) if error.is::<ClaimedArtifactDeletionError>() => {
                    warn!(%error, "artifact deletion consumer deferred or failed owned work");
                    // The exact intent retains a lease or has already moved to
                    // its durable retry/terminal state. Continue with later
                    // independent owners in this wake without changing that
                    // intent's retry delay.
                    tokio::task::yield_now().await;
                }
                Err(error) => {
                    warn!(%error, "artifact deletion consumer could not claim work");
                    break;
                }
            }
        }
        if deleted > 0 {
            info!(deleted, "consumed durable artifact deletions");
        }
        tokio::select! {
            changed = runtime_config_rx.changed() => {
                changed.context("worker runtime config publisher stopped")?;
                runtime_config_rx.borrow_and_update();
            }
            _ = wait_for_artifact_deletion_work() => {}
        }
    }
}

async fn detect_offline_agents(pool: &PgPool, offline_timeout_secs: i64) -> Result<u64> {
    let mut transitioned = 0_u64;
    for _ in 0..OFFLINE_BATCH {
        let mut tx = pool.begin().await?;
        let client_id = sqlx::query_scalar::<_, String>(OFFLINE_CANDIDATE_SQL)
            .bind(offline_timeout_secs as f64)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(client_id) = client_id else {
            tx.rollback().await?;
            break;
        };
        sqlx::query_scalar::<_, String>(
            r#"
            UPDATE clients
            SET status = 'offline'
            WHERE id = $1
              AND hidden_at IS NULL
              AND status = 'online'
              AND last_seen_at < now() - make_interval(secs => $2)
            RETURNING id
            "#,
        )
        .bind(&client_id)
        .bind(offline_timeout_secs as f64)
        .fetch_optional(&mut *tx)
        .await?
        .context("locked offline candidate was no longer eligible")?;
        let metadata = serde_json::json!({
            "from_status": "online",
            "to_status": "offline",
            "reason": "agent_offline_timeout",
            "offline_timeout_secs": offline_timeout_secs,
            "result": "transitioned",
            "origin_kind": "worker",
            "component": "agent-offline-reconciler",
        });
        sqlx::query(
            r#"
            INSERT INTO client_status_history (
                id, client_id, from_status, to_status, reason, metadata
            )
            VALUES ($1, $2, 'online', 'offline', 'agent_offline_timeout', $3)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&client_id)
        .bind(SqlJson(&metadata))
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO audit_logs (
                id, actor_id, action, target, command_hash, metadata
            )
            VALUES ($1, NULL, 'agent.status_offline', $2, NULL, $3)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(format!("client:{client_id}"))
        .bind(SqlJson(&metadata))
        .execute(&mut *tx)
        .await?;
        let event_id = format!("vps.status_changed:{client_id}:offline:{}", Uuid::new_v4());
        let predicates = vec![
            "vps.status.offline".to_string(),
            "vps.status.become_offline".to_string(),
        ];
        reconcile_agent_status_transition_in_tx(&mut tx, &client_id, "offline").await?;
        insert_webhook_event_in_tx(
            &mut tx,
            "vps.status_changed",
            &event_id,
            &predicates,
            std::slice::from_ref(&client_id),
            serde_json::json!({
                "event": {
                    "kind": "vps.status_changed",
                    "from_status": "online",
                    "to_status": "offline",
                    "reason": "agent_offline_timeout",
                },
                "vps_status": {
                    "client_id": client_id,
                    "from_status": "online",
                    "to_status": "offline",
                    "reason": "agent_offline_timeout",
                    "metadata": metadata,
                }
            }),
        )
        .await?;
        tx.commit().await?;
        transitioned += 1;
    }
    Ok(transitioned)
}

async fn drain_offline_agents(pool: &PgPool, offline_timeout_secs: i64) -> Result<u64> {
    let mut transitioned = 0_u64;
    loop {
        let page = detect_offline_agents(pool, offline_timeout_secs).await?;
        transitioned = transitioned
            .checked_add(page)
            .context("offline transition count overflow")?;
        if page < OFFLINE_BATCH as u64 {
            return Ok(transitioned);
        }
        // OFFLINE_BATCH bounds one scheduler page of short per-client
        // transactions, not the already-due clients handled in this pass.
        tokio::task::yield_now().await;
    }
}

async fn expire_stale_gateway_sessions(pool: &PgPool, offline_timeout_secs: i64) -> Result<u64> {
    let mut expired = 0_u64;
    loop {
        let rows = sqlx::query(
            r#"
            WITH candidate AS (
                SELECT session.id
                FROM gateway_sessions session
                JOIN clients client ON client.id = session.client_id
                WHERE session.status = 'active'
                  AND (
                    client.hidden_at IS NOT NULL
                    OR client.status IN ('offline', 'disconnected')
                    OR client.last_seen_at IS NULL
                    OR client.last_seen_at < now() - make_interval(secs => $1)
                  )
                ORDER BY session.last_seen_at, session.id
                FOR UPDATE OF session SKIP LOCKED
                LIMIT $2
            )
            UPDATE gateway_sessions session
            SET
                status = 'expired',
                last_seen_at = now(),
                ended_at = COALESCE(session.ended_at, now()),
                end_reason = COALESCE(session.end_reason, 'agent_offline_timeout')
            FROM candidate
            WHERE session.id = candidate.id
            RETURNING session.id
            "#,
        )
        .bind(offline_timeout_secs as f64)
        .bind(OFFLINE_BATCH as i64)
        .fetch_all(pool)
        .await?;
        let page = rows.len() as u64;
        expired = expired
            .checked_add(page)
            .context("expired gateway session count overflow")?;
        if page < OFFLINE_BATCH as u64 {
            return Ok(expired);
        }
        tokio::task::yield_now().await;
    }
}

async fn connect_postgres(
    postgres_url: &str,
    migrations_dir: &std::path::Path,
    max_connections: u32,
) -> Result<PgPool> {
    let connect_options = PgConnectOptions::from_str(postgres_url)
        .context("failed to parse the PostgreSQL connection URL")?;
    migrate_postgres_database(&connect_options, migrations_dir).await?;
    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .connect_with(connect_options.clone().options([("search_path", "public")]))
        .await
        .context("failed to connect to PostgreSQL")?;
    Ok(pool)
}

async fn migrate_postgres_database(
    connect_options: &PgConnectOptions,
    migrations_dir: &std::path::Path,
) -> Result<()> {
    let mut migration_connection = PgConnection::connect_with(connect_options)
        .await
        .context("failed to open the dedicated PostgreSQL migration connection")?;

    // This transaction-scoped owner exists only to make first creation of the
    // SQLx metadata schema deterministic when API and worker start together.
    // SQLx owns its separate database migration lock after this transaction.
    let mut schema_transaction = migration_connection
        .begin()
        .await
        .context("failed to begin the SQLx metadata schema transaction")?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(SQLX_METADATA_SCHEMA_LOCK_KEY)
        .execute(&mut *schema_transaction)
        .await
        .context("failed to acquire the SQLx metadata schema owner")?;
    sqlx::query("CREATE SCHEMA IF NOT EXISTS vpsman_internal AUTHORIZATION CURRENT_USER")
        .execute(&mut *schema_transaction)
        .await
        .context("failed to provision the SQLx metadata schema")?;
    schema_transaction
        .commit()
        .await
        .context("failed to commit the SQLx metadata schema transaction")?;

    sqlx::query("SET search_path TO vpsman_internal, public")
        .execute(&mut migration_connection)
        .await
        .context("failed to select the private SQLx metadata schema")?;
    let (current_schema, owned_by_current_user): (String, bool) = sqlx::query_as(
        r#"
        SELECT
            current_schema(),
            namespace.nspowner = (
                SELECT role.oid FROM pg_roles role WHERE role.rolname = current_user
            )
        FROM pg_namespace namespace
        WHERE namespace.nspname = $1
        "#,
    )
    .bind(SQLX_METADATA_SCHEMA)
    .fetch_one(&mut migration_connection)
    .await
    .context("failed to verify the private SQLx metadata schema")?;
    ensure!(
        current_schema == SQLX_METADATA_SCHEMA && owned_by_current_user,
        "private SQLx metadata schema is not the current role-owned schema"
    );

    sqlx::migrate::Migrator::new(migrations_dir)
        .await
        .with_context(|| {
            format!(
                "failed to load migrations from {}",
                migrations_dir.display()
            )
        })?
        .run(&mut migration_connection)
        .await
        .context("failed to run PostgreSQL migrations")?;
    migration_connection
        .close()
        .await
        .context("failed to close the dedicated PostgreSQL migration connection")?;
    Ok(())
}

async fn connect_worker_notification_listener(postgres_url: &str) -> Result<PgListener> {
    let mut listener = PgListener::connect(postgres_url)
        .await
        .context("failed to connect PostgreSQL worker notification listener")?;
    listener
        .listen("webhook_events")
        .await
        .context("failed to listen for webhook_events notifications")?;
    listener
        .listen("vpsman_telemetry_projection")
        .await
        .context("failed to listen for vpsman_telemetry_projection notifications")?;
    listener
        .listen("vpsman_telemetry_retention")
        .await
        .context("failed to listen for vpsman_telemetry_retention notifications")?;
    listener
        .listen("vpsman_traffic_active_cycle_rebuild")
        .await
        .context("failed to listen for traffic active-cycle rebuild notifications")?;
    listener
        .listen(ARTIFACT_DELETION_COMPLETED_CHANNEL)
        .await
        .context("failed to listen for artifact deletion completion notifications")?;
    Ok(listener)
}

async fn process_alert_notification_work(
    pool: &PgPool,
    config: AlertNotificationWorkerConfig,
    include_periodic_maintenance: bool,
) -> Result<AlertNotificationWorkerRun> {
    if !include_periodic_maintenance {
        // Due delivery rows carry their own send lease and are claimed with
        // SKIP LOCKED, so event wakes may drain independently.
        return process_due_alert_notifications(pool, config).await;
    }
    // Terminal delivery retention cannot overlap an active send, while due
    // sends are atomically claimed by their own durable delivery leases.
    process_alert_notifications(pool, config).await
}

#[derive(Clone, Copy)]
struct TelemetryRetentionSchedulerControl {
    shutdown: bool,
}

struct TelemetryRetentionScheduler {
    control_tx: watch::Sender<TelemetryRetentionSchedulerControl>,
    task: Option<JoinHandle<()>>,
    pool: PgPool,
}

impl TelemetryRetentionScheduler {
    fn spawn(
        pool: PgPool,
        recovery_interval: Duration,
        wake_rx: TelemetryRetentionWakeReceiver,
    ) -> Self {
        let (control_tx, control_rx) =
            watch::channel(TelemetryRetentionSchedulerControl { shutdown: false });
        let task_pool = pool.clone();
        let task = tokio::spawn(run_telemetry_retention_scheduler(
            task_pool,
            recovery_interval,
            control_rx,
            wake_rx,
        ));
        Self {
            control_tx,
            task: Some(task),
            pool,
        }
    }

    async fn wait_for_unexpected_exit(&mut self) -> Result<()> {
        let task_result = self
            .task
            .as_mut()
            .context("telemetry retention scheduler task is not running")?
            .await;
        self.task = None;
        unexpected_telemetry_retention_scheduler_exit(task_result)
    }

    async fn shutdown(mut self) -> Result<()> {
        self.control_tx
            .send_modify(|control| control.shutdown = true);
        let task_result = if let Some(task) = self.task.take() {
            Some(task.await)
        } else {
            None
        };
        self.pool.close().await;
        if let Some(task_result) = task_result {
            task_result.context("telemetry retention scheduler task failed")?;
        }
        Ok(())
    }
}

fn unexpected_telemetry_retention_scheduler_exit(
    task_result: std::result::Result<(), tokio::task::JoinError>,
) -> Result<()> {
    task_result.context("telemetry retention scheduler task failed")?;
    bail!("telemetry retention scheduler exited unexpectedly")
}

#[derive(Debug)]
enum TelemetryRetentionPageState {
    MoreWork,
    CurrentUntil(Instant),
    OwnerFailed(anyhow::Error),
}

async fn run_telemetry_retention_scheduler(
    pool: PgPool,
    recovery_interval: Duration,
    mut control_rx: watch::Receiver<TelemetryRetentionSchedulerControl>,
    mut wake_rx: TelemetryRetentionWakeReceiver,
) {
    let mut drain = TelemetryHistoryRetentionDrain::new(recovery_interval);
    loop {
        if control_rx.borrow().shutdown {
            return;
        }

        let page = process_telemetry_history_retention_page(&pool, &mut drain).await;
        match apply_ready_telemetry_retention_wake(&mut drain, &mut wake_rx) {
            TelemetryRetentionWakePoll::Applied => continue,
            TelemetryRetentionWakePoll::Closed => return,
            TelemetryRetentionWakePoll::Empty => {}
        }
        let deadline = match page {
            Ok(TelemetryRetentionPageState::MoreWork) => {
                // Transactions stay bounded to one exact durable owner, while
                // overdue owners remain work-conserving and fairly rotated.
                tokio::task::yield_now().await;
                continue;
            }
            Ok(TelemetryRetentionPageState::CurrentUntil(deadline)) => {
                log_telemetry_history_retention_run(drain.take_run());
                deadline
            }
            Ok(TelemetryRetentionPageState::OwnerFailed(error)) => {
                warn!(
                    error = %format!("{error:#}"),
                    "one telemetry retention owner failed; other owners remain runnable"
                );
                tokio::task::yield_now().await;
                continue;
            }
            Err(error) => {
                warn!(%error, "failed to process telemetry history retention");
                log_telemetry_history_retention_run(drain.take_run());
                Instant::now() + recovery_interval
            }
        };

        // Control changes wake this task but cannot postpone the per-owner
        // proof deadline. There is no delay between pages while any owner is
        // StillDue or its Current proof has expired.
        loop {
            tokio::select! {
                _ = time::sleep_until(time::Instant::from_std(deadline)) => break,
                changed = control_rx.changed() => {
                    if changed.is_err() || control_rx.borrow().shutdown {
                        return;
                    }
                }
                wake = wake_rx.wake_rx.recv() => {
                    if wake.is_none() {
                        return;
                    }
                    apply_telemetry_retention_wake_scope(
                        &mut drain,
                        take_telemetry_retention_wake_scope(&wake_rx.pending),
                    );
                    break;
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TelemetryRetentionWakePoll {
    Applied,
    Empty,
    Closed,
}

fn apply_ready_telemetry_retention_wake(
    drain: &mut TelemetryHistoryRetentionDrain,
    wake_rx: &mut TelemetryRetentionWakeReceiver,
) -> TelemetryRetentionWakePoll {
    match wake_rx.wake_rx.try_recv() {
        Ok(()) => {
            apply_telemetry_retention_wake_scope(
                drain,
                take_telemetry_retention_wake_scope(&wake_rx.pending),
            );
            TelemetryRetentionWakePoll::Applied
        }
        Err(mpsc::error::TryRecvError::Empty) => TelemetryRetentionWakePoll::Empty,
        Err(mpsc::error::TryRecvError::Disconnected) => TelemetryRetentionWakePoll::Closed,
    }
}

fn telemetry_retention_database_at(ready_at_unix: i64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(ready_at_unix, 0)
        .expect("retention notification timestamp was validated before queueing")
}

fn apply_telemetry_retention_wake_scope(
    drain: &mut TelemetryHistoryRetentionDrain,
    scope: TelemetryRetentionWakeScope,
) {
    if scope.recover_external_writer_frontiers {
        drain.recover_external_writer_frontiers();
    }
    if let Some(ready_at_unix) = scope.projection_minute_ready_at_unix {
        drain.notify_projection_minute_ready_at(telemetry_retention_database_at(ready_at_unix));
    }
    if let Some(ready_at_unix) = scope.due_event_ready_at_unix {
        drain.notify_due_events_ready_at(telemetry_retention_database_at(ready_at_unix));
    }
    for domain in scope.ordinary_rollup_domains {
        drain
            .notify_ordinary_rollup_published_now(domain.as_str())
            .expect("validated ordinary-rollup domain must have a typed prune consumer");
    }
    for span in scope.due_spans {
        drain
            .notify_due_span_published_at(
                span.domain.as_str(),
                span.source_bucket_secs,
                span.destination_bucket_secs,
                telemetry_retention_database_at(span.due_at_unix),
            )
            .expect("validated due-span publication must have one typed promotion consumer");
    }
    if scope.core_minute_frontier_advanced {
        drain.notify_core_minute_frontier_advanced_now();
    }
    if scope.traffic_minute_frontier_advanced {
        drain.notify_traffic_minute_frontier_advanced_now();
    }
    if scope.ping_facts_published {
        drain.notify_ping_facts_published_now();
    }
    if scope.ping_facts_deleted {
        drain.notify_ping_facts_deleted_now();
    }
    if scope.ping_current_deleted {
        drain.notify_ping_current_deleted_now();
    }
    if scope.telemetry_samples_deleted {
        drain.notify_telemetry_samples_deleted_now();
    }
    if let Some(ready_at_unix) = scope.sample_prune_ready_at_unix {
        drain.notify_sample_prune_ready_at(telemetry_retention_database_at(ready_at_unix));
    }
    if scope.sample_prune_frontier_advanced {
        drain.notify_sample_prune_now();
    }
    if scope.network_observation_history_published {
        drain.notify_manual_network_observation_now();
    }
    if scope.network_observation_series_deactivated {
        drain.notify_network_observation_series_deactivated_now();
    }
    if scope.traffic_samples_published {
        drain
            .notify_traffic_samples_published_now()
            .expect("traffic-sample publication must have a typed raw-promotion consumer");
    }
    for bucket_secs in scope.traffic_rollup_bucket_secs {
        drain
            .notify_traffic_rollup_published(bucket_secs)
            .expect("validated traffic-rollup wake width must have a typed retention consumer");
    }
    for domain in scope.retention_policy_domains {
        drain
            .notify_retention_policy_changed_now(domain.as_str())
            .expect("validated retention-policy wake domain must have a typed expiry owner");
    }
    if scope.ping_topology_changed {
        drain.notify_ping_topology_changed_now();
    }
    if scope.ping_rollups_deleted {
        drain
            .notify_ping_rollups_deleted_now()
            .expect("ping-rollup deletion effect must have a typed Ping-current consumer");
    }
    if scope.network_observation_history_deleted {
        drain
            .notify_network_observation_history_deleted_now()
            .expect("network-observation deletion effect must have a typed series consumer");
    }
    if scope.network_observation_latest_deleted {
        drain
            .notify_network_observation_latest_deleted_now()
            .expect("network-observation latest deletion must have a typed series consumer");
    }
}

fn log_telemetry_history_retention_run(run: TelemetryHistoryRetentionRun) {
    if run.has_activity() {
        info!(
            telemetry_core_minute_source_rows = run.core_minute_source_rows,
            telemetry_core_minute_derived_rows = run.core_minute_derived_rows,
            traffic_minute_source_rows = run.traffic_minute_source_rows,
            traffic_minute_derived_rows = run.traffic_minute_derived_rows,
            telemetry_samples_pruned = run.samples_pruned,
            telemetry_resource_spans_merged = run.resource_spans_merged,
            telemetry_rollups_pruned = run.rollups_pruned,
            telemetry_network_rate_spans_merged = run.network_rate_spans_merged,
            telemetry_network_rates_pruned = run.network_rates_pruned,
            telemetry_ping_spans_merged = run.ping_spans_merged,
            telemetry_ping_rollups_pruned = run.ping_rollups_pruned,
            telemetry_ping_facts_pruned = run.ping_facts_pruned,
            telemetry_ping_current_pruned = run.ping_current_pruned,
            telemetry_ping_series_pruned = run.ping_series_pruned,
            system_metric_rollups_pruned = run.system_metric_rollups_pruned,
            traffic_counter_samples_pruned = run.traffic_counter_samples_pruned,
            traffic_raw_rows_promoted = run.traffic_raw_rows_promoted,
            traffic_rollup_rows_promoted = run.traffic_rollup_rows_promoted,
            traffic_rollup_rows_pruned = run.traffic_rollup_rows_pruned,
            network_observation_source_rows_promoted = run.network_observation_source_rows_promoted,
            network_observation_destination_rows_written =
                run.network_observation_destination_rows_written,
            network_observation_expired_exact_rows_pruned =
                run.network_observation_expired_exact_rows_pruned,
            network_observation_expired_rollup_rows_pruned =
                run.network_observation_expired_rollup_rows_pruned,
            network_observation_inactive_latest_pruned =
                run.network_observation_inactive_latest_pruned,
            network_observation_inactive_series_pruned =
                run.network_observation_inactive_series_pruned,
            "processed telemetry history retention"
        );
    }
}

async fn process_telemetry_history_retention_page(
    pool: &PgPool,
    drain: &mut TelemetryHistoryRetentionDrain,
) -> Result<TelemetryRetentionPageState> {
    if let TelemetryHistoryRetentionStep::CurrentUntil(deadline) = drain.next_step() {
        return Ok(TelemetryRetentionPageState::CurrentUntil(deadline));
    }
    if let TelemetryHistoryRetentionPageReadiness::NoWork(page) = drain.prepare_page(pool).await? {
        return Ok(match page {
            TelemetryHistoryRetentionPage::MoreWork => TelemetryRetentionPageState::MoreWork,
            TelemetryHistoryRetentionPage::CurrentUntil(deadline) => {
                TelemetryRetentionPageState::CurrentUntil(deadline)
            }
            TelemetryHistoryRetentionPage::OwnerFailed(error) => {
                TelemetryRetentionPageState::OwnerFailed(error)
            }
        });
    }
    match drain.process_page(pool).await? {
        TelemetryHistoryRetentionPage::MoreWork => Ok(TelemetryRetentionPageState::MoreWork),
        TelemetryHistoryRetentionPage::CurrentUntil(deadline) => {
            Ok(TelemetryRetentionPageState::CurrentUntil(deadline))
        }
        TelemetryHistoryRetentionPage::OwnerFailed(error) => {
            Ok(TelemetryRetentionPageState::OwnerFailed(error))
        }
    }
}

async fn process_telemetry_history_retention_drain(
    pool: &PgPool,
) -> Result<TelemetryHistoryRetentionRun> {
    let mut drain = TelemetryHistoryRetentionDrain::default();
    loop {
        match process_telemetry_history_retention_page(pool, &mut drain).await? {
            TelemetryRetentionPageState::MoreWork => tokio::task::yield_now().await,
            TelemetryRetentionPageState::OwnerFailed(error) => return Err(error),
            TelemetryRetentionPageState::CurrentUntil(_) => return Ok(drain.finish()),
        }
    }
}

#[derive(Default)]
struct ArtifactCleanupRun {
    jobs: i64,
    failed_jobs: i64,
    deleted_rows: i64,
    deleted_bytes: i64,
    tombstoned_rows: i64,
    tombstoned_bytes: i64,
    skipped_rows: i64,
}

impl ArtifactCleanupRun {
    fn merge(&mut self, page: Self) {
        self.jobs = self.jobs.saturating_add(page.jobs);
        self.failed_jobs = self.failed_jobs.saturating_add(page.failed_jobs);
        self.deleted_rows = self.deleted_rows.saturating_add(page.deleted_rows);
        self.deleted_bytes = self.deleted_bytes.saturating_add(page.deleted_bytes);
        self.tombstoned_rows = self.tombstoned_rows.saturating_add(page.tombstoned_rows);
        self.tombstoned_bytes = self.tombstoned_bytes.saturating_add(page.tombstoned_bytes);
        self.skipped_rows = self.skipped_rows.saturating_add(page.skipped_rows);
    }
}

struct ArtifactCleanupJob {
    id: Uuid,
    created_by: Option<Uuid>,
    metadata: Value,
    lease_id: Uuid,
}

#[derive(Clone, Copy)]
struct ArtifactCleanupRoundFrontier {
    created_at: DateTime<Utc>,
    id: Uuid,
}

struct ArtifactCleanupCandidate {
    id: Uuid,
    domain: String,
    object_key: String,
    sha256_hex: String,
    size_bytes: i64,
    status: String,
    backup_artifact_id: Option<Uuid>,
    identity_matches_review: bool,
}

const ARTIFACT_CLEANUP_JOB_LEASE_SECS: i32 = 30;
const ARTIFACT_CLEANUP_JOB_RENEW_SECS: u64 = 10;

#[derive(Debug)]
struct ClaimedArtifactDeletionError(anyhow::Error);

impl std::fmt::Display for ClaimedArtifactDeletionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for ClaimedArtifactDeletionError {}

async fn process_next_artifact_deletion_intent(
    pool: &PgPool,
    object_store: &BackupObjectStore,
) -> Result<bool> {
    let Some(owner) = claim_artifact_deletion(pool, None, None, None).await? else {
        return Ok(false);
    };
    let result = match owner.source_kind.as_str() {
        "backup_policy" => delete_backup_policy_artifact(pool, object_store, &owner)
            .await
            .map(|_| ()),
        "manual_cleanup" => consume_manual_artifact_deletion(pool, object_store, &owner).await,
        "history_retention" => consume_history_artifact_deletion(pool, object_store, &owner).await,
        source_kind => {
            let error = format!("unsupported artifact deletion source {source_kind}");
            ensure!(
                defer_artifact_deletion(pool, &owner, &error).await?,
                "artifact deletion ownership lost while deferring unsupported source"
            );
            Err(anyhow::anyhow!(error))
        }
    };
    // This is a latency hint for a producer waiting on its exact durable
    // target. The intent/target rows remain the completion authority.
    publish_artifact_deletion_completion();
    if let Err(error) = result {
        return Err(ClaimedArtifactDeletionError(error).into());
    }
    Ok(true)
}

async fn consume_manual_artifact_deletion(
    pool: &PgPool,
    object_store: &BackupObjectStore,
    owner: &ArtifactDeletionOwner,
) -> Result<()> {
    let Some((job, candidate)) = load_owned_manual_artifact_deletion(pool, owner).await? else {
        let status =
            sqlx::query_scalar::<_, String>("SELECT status FROM server_jobs WHERE id = $1")
                .bind(owner.source_id)
                .fetch_optional(pool)
                .await?;
        if status
            .as_deref()
            .is_some_and(|status| matches!(status, "completed" | "failed" | "canceled"))
        {
            ensure!(
                fail_artifact_deletion(pool, owner, "artifact cleanup source job is terminal")
                    .await?,
                "artifact deletion ownership lost while closing terminal source"
            );
        } else {
            ensure!(
                defer_artifact_deletion(pool, owner, "artifact cleanup producer is not active")
                    .await?,
                "artifact deletion ownership lost while awaiting source producer"
            );
        }
        return Ok(());
    };

    match delete_artifact_cleanup_object(pool, object_store, &job, &candidate, owner).await {
        Ok(ArtifactCleanupDisposition::Deleted) => Ok(()),
        Ok(_) => bail!("manual artifact deletion returned an invalid disposition"),
        Err(error) => {
            let message = error.to_string();
            let mut tx = pool.begin().await?;
            let marked = sqlx::query(
                r#"
                WITH target AS (
                    UPDATE server_job_artifact_cleanup_targets target
                    SET outcome = 'skipped',
                        outcome_reason = 'object_store_delete_failed',
                        processed_at = now()
                    FROM server_jobs job
                    WHERE target.server_job_id = $1
                      AND target.artifact_id = $2
                      AND target.outcome = 'pending'
                      AND job.id = target.server_job_id
                      AND job.status = 'running'
                      AND job.lease_id = $3
                      AND job.lease_until > now()
                    RETURNING target.server_job_id
                )
                UPDATE server_jobs job
                SET error = left($4, 1000)
                FROM target
                WHERE job.id = target.server_job_id
                  AND job.status = 'running'
                  AND job.lease_id = $3
                "#,
            )
            .bind(job.id)
            .bind(candidate.id)
            .bind(job.lease_id)
            .bind(&message)
            .execute(&mut *tx)
            .await?;
            ensure!(
                marked.rows_affected() == 1,
                "artifact cleanup ownership lost while publishing deletion failure"
            );
            publish_artifact_deletion_completion_in_tx(&mut tx, job.id).await?;
            tx.commit().await?;
            Err(error)
        }
    }
}

async fn load_owned_manual_artifact_deletion(
    pool: &PgPool,
    owner: &ArtifactDeletionOwner,
) -> Result<Option<(ArtifactCleanupJob, ArtifactCleanupCandidate)>> {
    ensure!(
        owner.source_kind == "manual_cleanup" && owner.source_revision == 1,
        "manual artifact deletion source identity invalid"
    );
    let row = sqlx::query(
        r#"
        SELECT
            job.created_by,
            job.metadata,
            job.lease_id,
            target.domain,
            target.object_key,
            target.sha256_hex,
            target.size_bytes,
            COALESCE(artifact.status, 'missing') AS status,
            artifact.backup_artifact_id,
            artifact.id IS NOT NULL
              AND artifact.domain = target.domain
              AND artifact.object_key = target.object_key
              AND artifact.sha256_hex = target.sha256_hex
              AND artifact.size_bytes = target.size_bytes AS identity_matches_review
        FROM server_jobs job
        JOIN server_job_artifact_cleanup_targets target
          ON target.server_job_id = job.id
         AND target.artifact_id = $2
         AND target.outcome = 'pending'
        LEFT JOIN server_artifacts artifact ON artifact.id = target.artifact_id
        WHERE job.id = $1
          AND job.status = 'running'
          AND job.lease_until > now()
        "#,
    )
    .bind(owner.source_id)
    .bind(owner.artifact_id)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        let job = ArtifactCleanupJob {
            id: owner.source_id,
            created_by: row.try_get("created_by")?,
            metadata: row.try_get("metadata")?,
            lease_id: row.try_get("lease_id")?,
        };
        let candidate = ArtifactCleanupCandidate {
            id: owner.artifact_id,
            domain: row.try_get("domain")?,
            object_key: row.try_get("object_key")?,
            sha256_hex: row.try_get("sha256_hex")?,
            size_bytes: row.try_get("size_bytes")?,
            status: row.try_get("status")?,
            backup_artifact_id: row.try_get("backup_artifact_id")?,
            identity_matches_review: row.try_get("identity_matches_review")?,
        };
        let expected_identity = serde_json::json!({
            "server_job_id": job.id,
            "artifact_id": candidate.id,
            "domain": candidate.domain,
            "object_key": candidate.object_key,
            "sha256_hex": candidate.sha256_hex,
            "size_bytes": candidate.size_bytes,
        });
        ensure!(
            owner.source_identity == expected_identity,
            "manual artifact deletion reviewed identity changed"
        );
        Ok((job, candidate))
    })
    .transpose()
}

async fn consume_history_artifact_deletion(
    pool: &PgPool,
    object_store: &BackupObjectStore,
    owner: &ArtifactDeletionOwner,
) -> Result<()> {
    let job_id = artifact_deletion_identity_uuid(&owner.source_identity, "job_id")?;
    let client_id = artifact_deletion_identity_str(&owner.source_identity, "client_id")?;
    let seq = artifact_deletion_identity_i64(&owner.source_identity, "seq")?;
    ensure!(
        owner.source_id == job_id
            && owner.source_revision == seq.saturating_add(1).max(1)
            && owner
                .source_identity
                .get("object_key")
                .and_then(Value::as_str)
                == Some(owner.object_key.as_str()),
        "history artifact deletion source identity changed"
    );

    let (heartbeat_stop, heartbeat) =
        spawn_artifact_deletion_heartbeat(pool.clone(), owner.clone());
    let deletion = delete_object_key_confirmed(object_store, &owner.object_key).await;
    let _ = heartbeat_stop.send(());
    ensure!(
        heartbeat
            .await
            .context("artifact deletion heartbeat stopped unexpectedly")??,
        "artifact deletion ownership lost"
    );
    if let Err(error) = deletion {
        ensure!(
            defer_artifact_deletion(pool, owner, &error.to_string()).await?,
            "history artifact deletion ownership lost after object-store failure"
        );
        return Err(error);
    }

    let seq = i32::try_from(seq).context("history artifact deletion sequence invalid")?;
    let mut tx = pool.begin().await?;
    ensure!(
        lock_owned_artifact_deletion_in_tx(&mut tx, owner).await?,
        "history artifact deletion ownership lost before finalization"
    );
    sqlx::query(
        r#"
        DELETE FROM job_outputs
        WHERE job_id = $1 AND client_id = $2 AND seq = $3 AND object_key = $4
        "#,
    )
    .bind(job_id)
    .bind(client_id)
    .bind(seq)
    .bind(&owner.object_key)
    .execute(&mut *tx)
    .await?;
    let marked = sqlx::query(
        r#"
        UPDATE server_artifacts
        SET status = 'deleted', deleted_at = now()
        WHERE id = $1 AND object_key = $2 AND status = 'deleting'
        "#,
    )
    .bind(owner.artifact_id)
    .bind(&owner.object_key)
    .execute(&mut *tx)
    .await?;
    ensure!(
        marked.rows_affected() == 1,
        "history artifact registry identity changed"
    );
    ensure!(
        finish_artifact_deletion_in_tx(&mut tx, owner).await?,
        "history artifact deletion ownership lost during finalization"
    );
    tx.commit().await?;
    Ok(())
}

fn artifact_deletion_identity_uuid(identity: &Value, field: &str) -> Result<Uuid> {
    Uuid::parse_str(artifact_deletion_identity_str(identity, field)?)
        .with_context(|| format!("artifact deletion {field} invalid"))
}

fn artifact_deletion_identity_str<'a>(identity: &'a Value, field: &str) -> Result<&'a str> {
    identity
        .get(field)
        .and_then(Value::as_str)
        .with_context(|| format!("artifact deletion {field} missing"))
}

fn artifact_deletion_identity_i64(identity: &Value, field: &str) -> Result<i64> {
    identity
        .get(field)
        .and_then(Value::as_i64)
        .with_context(|| format!("artifact deletion {field} missing"))
}

async fn process_artifact_cleanup_jobs(pool: &PgPool) -> Result<ArtifactCleanupRun> {
    let Some(frontier) = artifact_cleanup_round_frontier(pool).await? else {
        return Ok(ArtifactCleanupRun::default());
    };
    let mut run = ArtifactCleanupRun::default();
    loop {
        let page = process_next_artifact_cleanup_job(pool, frontier).await?;
        if page.jobs == 0 {
            return Ok(run);
        }
        run.merge(page);
        // One job is one externally visible owner. Yield between owners but
        // drain the finite round immediately instead of imposing one job per
        // shared worker tick.
        tokio::task::yield_now().await;
    }
}

async fn process_next_artifact_cleanup_job(
    pool: &PgPool,
    frontier: ArtifactCleanupRoundFrontier,
) -> Result<ArtifactCleanupRun> {
    let Some(job) = claim_artifact_cleanup_job_through(pool, Some(frontier)).await? else {
        return Ok(ArtifactCleanupRun::default());
    };
    let (heartbeat_stop, heartbeat) =
        spawn_artifact_cleanup_job_heartbeat(pool.clone(), job.id, job.lease_id);
    for required_scope in artifact_cleanup_job_required_scopes(&job.metadata)? {
        if !actor_authorized(pool, job.created_by, "operator", &[required_scope]).await? {
            let _ = heartbeat_stop.send(true);
            ensure!(
                heartbeat
                    .await
                    .context("artifact cleanup heartbeat stopped unexpectedly")??,
                "artifact cleanup ownership lost"
            );
            ensure!(
                mark_artifact_cleanup_job_failed(pool, &job, "actor_authority_revoked").await?,
                "artifact cleanup ownership lost"
            );
            return Ok(ArtifactCleanupRun {
                jobs: 1,
                failed_jobs: 1,
                ..ArtifactCleanupRun::default()
            });
        }
    }
    let result = run_artifact_cleanup_job(pool, &job).await;
    let _ = heartbeat_stop.send(true);
    ensure!(
        heartbeat
            .await
            .context("artifact cleanup heartbeat stopped unexpectedly")??,
        "artifact cleanup ownership lost"
    );
    match result {
        Ok(run) => {
            let completed = sqlx::query(
                r#"
                UPDATE server_jobs
                SET
                    status = $2,
                    deleted_count = $3,
                    deleted_bytes = $4,
                    metadata = metadata || jsonb_build_object(
                        'tombstoned_count', $5::bigint,
                        'tombstoned_bytes', $6::bigint,
                        'skipped_count', $7::bigint
                    ),
                    completed_at = now(),
                    error = NULL,
                    lease_id = NULL,
                    lease_until = NULL
                WHERE id = $1
                  AND status = $8
                  AND lease_id = $9
                  AND lease_until > now()
                "#,
            )
            .bind(job.id)
            .bind(SERVER_JOB_STATUS_COMPLETED)
            .bind(run.deleted_rows)
            .bind(run.deleted_bytes)
            .bind(run.tombstoned_rows)
            .bind(run.tombstoned_bytes)
            .bind(run.skipped_rows)
            .bind(SERVER_JOB_STATUS_RUNNING)
            .bind(job.lease_id)
            .execute(pool)
            .await?;
            ensure!(
                completed.rows_affected() == 1,
                "artifact cleanup ownership lost during completion"
            );
            Ok(ArtifactCleanupRun { jobs: 1, ..run })
        }
        Err(error) => {
            ensure!(
                mark_artifact_cleanup_job_failed(pool, &job, &error.to_string()).await?,
                "artifact cleanup ownership lost during failure"
            );
            warn!(job_id = %job.id, %error, "artifact cleanup job failed");
            Ok(ArtifactCleanupRun {
                jobs: 1,
                failed_jobs: 1,
                ..ArtifactCleanupRun::default()
            })
        }
    }
}

async fn artifact_cleanup_round_frontier(
    pool: &PgPool,
) -> Result<Option<ArtifactCleanupRoundFrontier>> {
    let row = sqlx::query(
        r#"
        SELECT created_at, id
        FROM server_jobs
        WHERE job_type=$1
          AND (
                (status=$2 AND next_attempt_at <= now())
                OR (status=$3 AND lease_until <= now())
              )
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(SERVER_JOB_TYPE_ARTIFACT_CLEANUP)
    .bind(SERVER_JOB_STATUS_QUEUED)
    .bind(SERVER_JOB_STATUS_RUNNING)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        Ok(ArtifactCleanupRoundFrontier {
            created_at: row.try_get("created_at")?,
            id: row.try_get("id")?,
        })
    })
    .transpose()
}

async fn mark_artifact_cleanup_job_failed(
    pool: &PgPool,
    job: &ArtifactCleanupJob,
    error: &str,
) -> Result<bool> {
    let failed = sqlx::query(
        r#"
        UPDATE server_jobs
        SET
            status = $2,
            error = $3,
            completed_at = now(),
            lease_id = NULL,
            lease_until = NULL
        WHERE id = $1
          AND status = $4
          AND lease_id = $5
        "#,
    )
    .bind(job.id)
    .bind(SERVER_JOB_STATUS_FAILED)
    .bind(error)
    .bind(SERVER_JOB_STATUS_RUNNING)
    .bind(job.lease_id)
    .execute(pool)
    .await?;
    Ok(failed.rows_affected() == 1)
}

#[cfg(test)]
async fn claim_artifact_cleanup_job(pool: &PgPool) -> Result<Option<ArtifactCleanupJob>> {
    claim_artifact_cleanup_job_through(pool, None).await
}

async fn claim_artifact_cleanup_job_through(
    pool: &PgPool,
    frontier: Option<ArtifactCleanupRoundFrontier>,
) -> Result<Option<ArtifactCleanupJob>> {
    let lease_id = Uuid::new_v4();
    let row = sqlx::query(
        r#"
        WITH claimed AS (
            SELECT id
            FROM server_jobs
            WHERE job_type = $1
              AND (
                (status = $2 AND next_attempt_at <= now())
                OR (status = $3 AND lease_until <= now())
              )
              AND (
                    $6::timestamptz IS NULL
                    OR (created_at, id) <= ($6, $7)
                  )
            ORDER BY created_at ASC, id ASC
            LIMIT 1
            FOR UPDATE SKIP LOCKED
        )
        UPDATE server_jobs job
        SET status = $3,
            started_at = COALESCE(job.started_at, now()),
            lease_id = $4,
            lease_until = now() + ($5::int * interval '1 second'),
            attempt_count = job.attempt_count + 1,
            error = NULL,
            metadata = CASE
                WHEN job.status = $3 THEN job.metadata || jsonb_build_object(
                    'owner_recovered_at', now()::text
                )
                ELSE job.metadata
            END
        FROM claimed
        WHERE job.id = claimed.id
        RETURNING job.id, job.created_by, job.metadata, job.lease_id
        "#,
    )
    .bind(SERVER_JOB_TYPE_ARTIFACT_CLEANUP)
    .bind(SERVER_JOB_STATUS_QUEUED)
    .bind(SERVER_JOB_STATUS_RUNNING)
    .bind(lease_id)
    .bind(ARTIFACT_CLEANUP_JOB_LEASE_SECS)
    .bind(frontier.map(|frontier| frontier.created_at))
    .bind(frontier.map(|frontier| frontier.id))
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        Ok(ArtifactCleanupJob {
            id: row.try_get("id")?,
            created_by: row.try_get("created_by")?,
            metadata: row.try_get("metadata")?,
            lease_id: row.try_get("lease_id")?,
        })
    })
    .transpose()
}

fn spawn_artifact_cleanup_job_heartbeat(
    pool: PgPool,
    job_id: Uuid,
    lease_id: Uuid,
) -> (watch::Sender<bool>, JoinHandle<Result<bool>>) {
    let (stop_tx, mut stop_rx) = watch::channel(false);
    let task = tokio::spawn(async move {
        let mut ticker = time::interval(Duration::from_secs(ARTIFACT_CLEANUP_JOB_RENEW_SECS));
        ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        ticker.tick().await;
        loop {
            tokio::select! {
                changed = stop_rx.changed() => {
                    if changed.is_err() || *stop_rx.borrow() {
                        return Ok(true);
                    }
                }
                _ = ticker.tick() => {
                    let renewed = sqlx::query(
                        r#"
                        UPDATE server_jobs
                        SET lease_until = now() + ($3::int * interval '1 second')
                        WHERE id = $1
                          AND status = 'running'
                          AND lease_id = $2
                          AND lease_until > now()
                        "#,
                    )
                    .bind(job_id)
                    .bind(lease_id)
                    .bind(ARTIFACT_CLEANUP_JOB_LEASE_SECS)
                    .execute(&pool)
                    .await?;
                    if renewed.rows_affected() != 1 {
                        return Ok(false);
                    }
                }
            }
        }
    });
    (stop_tx, task)
}

fn artifact_cleanup_job_required_scopes(metadata: &Value) -> Result<Vec<&'static str>> {
    let domains = metadata
        .get("domains")
        .and_then(Value::as_array)
        .context("artifact_cleanup_domains_required")?;
    ensure!(!domains.is_empty(), "artifact_cleanup_domains_required");
    let mut scopes = Vec::new();
    for domain in domains {
        let Some(domain) = domain.as_str() else {
            bail!("artifact_cleanup_domain_invalid");
        };
        let scope = match domain {
            "backup_artifact" => "backups:write",
            "job_output" | "file_transfer" => "jobs:write",
            _ => bail!("artifact_cleanup_domain_invalid"),
        };
        if !scopes.contains(&scope) {
            scopes.push(scope);
        }
    }
    Ok(scopes)
}

async fn run_artifact_cleanup_job(
    pool: &PgPool,
    job: &ArtifactCleanupJob,
) -> Result<ArtifactCleanupRun> {
    let mut run = load_artifact_cleanup_progress(pool, job.id).await?;
    loop {
        ensure_artifact_cleanup_job_owner_healthy(pool, job).await?;
        // Register before observing the durable target frontier so a fast
        // consumer completion cannot be lost between the query and wait.
        let completed = artifact_deletion_completion_signal().notified();
        tokio::pin!(completed);
        completed.as_mut().enable();

        let candidates = artifact_cleanup_targets(pool, job.id).await?;
        validate_artifact_cleanup_candidate_sizes(&candidates)?;
        if candidates.is_empty() {
            return load_artifact_cleanup_progress(pool, job.id).await;
        }
        let mut waiting_for_consumer = false;
        for candidate in &candidates {
            if !candidate.identity_matches_review
                || !matches!(
                    candidate.status.as_str(),
                    "creating" | "active" | "deleting" | "delete_failed"
                )
            {
                ensure!(
                    mark_artifact_cleanup_target_outcome(
                        pool,
                        job,
                        candidate.id,
                        "skipped",
                        "reviewed_identity_no_longer_eligible",
                    )
                    .await?,
                    "artifact cleanup ownership lost while skipping target"
                );
                run.skipped_rows += 1;
                persist_artifact_cleanup_progress(pool, job, &run).await?;
                continue;
            }
            match apply_artifact_cleanup_candidate(pool, job, candidate).await? {
                ArtifactCleanupDisposition::Pending => waiting_for_consumer = true,
                ArtifactCleanupDisposition::Deleted => {
                    run.deleted_rows += 1;
                    run.deleted_bytes = run
                        .deleted_bytes
                        .checked_add(candidate.size_bytes)
                        .context("artifact cleanup deleted byte total overflow")?;
                }
                ArtifactCleanupDisposition::Tombstoned => {
                    run.tombstoned_rows += 1;
                    run.tombstoned_bytes =
                        run.tombstoned_bytes
                            .checked_add(candidate.size_bytes)
                            .context("artifact cleanup tombstoned byte total overflow")?;
                }
                ArtifactCleanupDisposition::Skipped => run.skipped_rows += 1,
            }
            persist_artifact_cleanup_progress(pool, job, &run).await?;
        }
        if waiting_for_consumer {
            completed.await;
        }
    }
}

async fn ensure_artifact_cleanup_job_owner_healthy(
    pool: &PgPool,
    job: &ArtifactCleanupJob,
) -> Result<()> {
    let error = sqlx::query_scalar::<_, Option<String>>(
        r#"
        SELECT error
        FROM server_jobs
        WHERE id = $1
          AND status = 'running'
          AND lease_id = $2
          AND lease_until > now()
        "#,
    )
    .bind(job.id)
    .bind(job.lease_id)
    .fetch_optional(pool)
    .await?
    .context("artifact cleanup ownership lost")?;
    if let Some(error) = error {
        bail!("{error}");
    }
    Ok(())
}

fn validate_artifact_cleanup_candidate_sizes(
    candidates: &[ArtifactCleanupCandidate],
) -> Result<()> {
    candidates.iter().try_fold(0_i64, |total, candidate| {
        ensure!(
            candidate.size_bytes >= 0,
            "artifact_cleanup_reviewed_target_numeric_invalid: artifact {} has a negative size",
            candidate.id
        );
        total.checked_add(candidate.size_bytes).with_context(|| {
            format!(
                "artifact_cleanup_reviewed_target_numeric_invalid: byte total overflow at artifact {}",
                candidate.id
            )
        })
    })?;
    Ok(())
}

async fn load_artifact_cleanup_progress(pool: &PgPool, job_id: Uuid) -> Result<ArtifactCleanupRun> {
    let row = sqlx::query(
        r#"
        SELECT
            count(*) FILTER (WHERE outcome = 'deleted')::bigint AS deleted_rows,
            COALESCE(sum(size_bytes) FILTER (WHERE outcome = 'deleted'), 0)::bigint AS deleted_bytes,
            count(*) FILTER (WHERE outcome = 'tombstoned')::bigint AS tombstoned_rows,
            COALESCE(sum(size_bytes) FILTER (WHERE outcome = 'tombstoned'), 0)::bigint AS tombstoned_bytes,
            count(*) FILTER (WHERE outcome = 'skipped')::bigint AS skipped_rows
        FROM server_job_artifact_cleanup_targets
        WHERE server_job_id = $1
        "#,
    )
    .bind(job_id)
    .fetch_one(pool)
    .await?;
    Ok(ArtifactCleanupRun {
        deleted_rows: row.try_get("deleted_rows")?,
        deleted_bytes: row.try_get("deleted_bytes")?,
        tombstoned_rows: row.try_get("tombstoned_rows")?,
        tombstoned_bytes: row.try_get("tombstoned_bytes")?,
        skipped_rows: row.try_get("skipped_rows")?,
        ..ArtifactCleanupRun::default()
    })
}

async fn mark_artifact_cleanup_target_outcome(
    pool: &PgPool,
    job: &ArtifactCleanupJob,
    artifact_id: Uuid,
    outcome: &str,
    reason: &str,
) -> Result<bool> {
    let updated = sqlx::query(
        r#"
        UPDATE server_job_artifact_cleanup_targets target
        SET outcome = $3,
            outcome_reason = $4,
            processed_at = now()
        FROM server_jobs job
        WHERE target.server_job_id = $1
          AND target.artifact_id = $2
          AND target.outcome = 'pending'
          AND job.id = target.server_job_id
          AND job.status = 'running'
          AND job.lease_id = $5
          AND job.lease_until > now()
        "#,
    )
    .bind(job.id)
    .bind(artifact_id)
    .bind(outcome)
    .bind(reason)
    .bind(job.lease_id)
    .execute(pool)
    .await?;
    Ok(updated.rows_affected() == 1)
}

async fn persist_artifact_cleanup_progress(
    pool: &PgPool,
    job: &ArtifactCleanupJob,
    run: &ArtifactCleanupRun,
) -> Result<()> {
    let updated = sqlx::query(
        r#"
        UPDATE server_jobs
        SET deleted_count = $2,
            deleted_bytes = $3,
            metadata = metadata || jsonb_build_object(
                'tombstoned_count', $4::bigint,
                'tombstoned_bytes', $5::bigint,
                'skipped_count', $6::bigint
            )
        WHERE id = $1
          AND status = $7
          AND lease_id = $8
          AND lease_until > now()
        "#,
    )
    .bind(job.id)
    .bind(run.deleted_rows)
    .bind(run.deleted_bytes)
    .bind(run.tombstoned_rows)
    .bind(run.tombstoned_bytes)
    .bind(run.skipped_rows)
    .bind(SERVER_JOB_STATUS_RUNNING)
    .bind(job.lease_id)
    .execute(pool)
    .await?;
    ensure!(
        updated.rows_affected() == 1,
        "artifact cleanup ownership lost while persisting progress"
    );
    Ok(())
}

enum ArtifactCleanupDisposition {
    Pending,
    Deleted,
    Tombstoned,
    Skipped,
}

async fn apply_artifact_cleanup_candidate(
    pool: &PgPool,
    job: &ArtifactCleanupJob,
    candidate: &ArtifactCleanupCandidate,
) -> Result<ArtifactCleanupDisposition> {
    if candidate.status == "creating" {
        ensure!(
            mark_artifact_cleanup_target_outcome(
                pool,
                job,
                candidate.id,
                "skipped",
                "artifact_creation_owned_by_producer",
            )
            .await?,
            "artifact cleanup ownership lost while skipping reservation"
        );
        return Ok(ArtifactCleanupDisposition::Skipped);
    }
    if candidate.domain == "backup_artifact"
        && backup_artifact_is_referenced(pool, candidate.backup_artifact_id, &candidate.object_key)
            .await?
    {
        return tombstone_server_artifact(pool, job, candidate, "backup_reference_protected").await;
    }
    if !matches!(
        candidate.domain.as_str(),
        "job_output" | "file_transfer_handoff" | "file_transfer_source" | "backup_artifact"
    ) {
        return tombstone_server_artifact(pool, job, candidate, "unsupported_artifact_domain")
            .await;
    }
    let inserted = enqueue_artifact_deletion(
        pool,
        &ArtifactDeletionReview {
            artifact_id: candidate.id,
            object_key: candidate.object_key.clone(),
            sha256_hex: candidate.sha256_hex.clone(),
            size_bytes: candidate.size_bytes,
            source_kind: "manual_cleanup",
            source_id: job.id,
            source_revision: 1,
            source_identity: serde_json::json!({
                "server_job_id": job.id,
                "artifact_id": candidate.id,
                "domain": candidate.domain,
                "object_key": candidate.object_key,
                "sha256_hex": candidate.sha256_hex,
                "size_bytes": candidate.size_bytes,
            }),
        },
    )
    .await?;
    let owned_by_job = inserted
        || sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM server_artifact_deletion_intents
                WHERE artifact_id = $1
                  AND source_kind = 'manual_cleanup'
                  AND source_id = $2
            )
            "#,
        )
        .bind(candidate.id)
        .bind(job.id)
        .fetch_one(pool)
        .await?;
    if !owned_by_job {
        ensure!(
            mark_artifact_cleanup_target_outcome(
                pool,
                job,
                candidate.id,
                "skipped",
                "artifact_deletion_owned_elsewhere",
            )
            .await?,
            "artifact cleanup ownership lost while skipping owned target"
        );
        return Ok(ArtifactCleanupDisposition::Skipped);
    }
    Ok(ArtifactCleanupDisposition::Pending)
}

async fn delete_artifact_cleanup_object(
    pool: &PgPool,
    object_store: &BackupObjectStore,
    job: &ArtifactCleanupJob,
    candidate: &ArtifactCleanupCandidate,
    owner: &ArtifactDeletionOwner,
) -> Result<ArtifactCleanupDisposition> {
    let (heartbeat_stop, heartbeat) =
        spawn_artifact_deletion_heartbeat(pool.clone(), owner.clone());
    let delete_result = delete_object_key_confirmed(object_store, &candidate.object_key).await;
    let _ = heartbeat_stop.send(());
    ensure!(
        heartbeat
            .await
            .context("artifact deletion heartbeat stopped unexpectedly")??,
        "artifact deletion ownership lost"
    );
    if let Err(error) = delete_result {
        ensure!(
            fail_artifact_deletion(pool, owner, &error.to_string()).await?,
            "artifact deletion ownership lost after object-store failure"
        );
        return Err(error);
    }
    finalize_artifact_cleanup_deletion(pool, job, candidate, owner).await?;
    Ok(ArtifactCleanupDisposition::Deleted)
}

async fn lock_artifact_cleanup_job_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job: &ArtifactCleanupJob,
) -> Result<bool> {
    sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM server_jobs
            WHERE id = $1
              AND status = 'running'
              AND lease_id = $2
              AND lease_until > now()
            FOR UPDATE
        )
        "#,
    )
    .bind(job.id)
    .bind(job.lease_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(Into::into)
}

async fn finalize_artifact_cleanup_deletion(
    pool: &PgPool,
    job: &ArtifactCleanupJob,
    candidate: &ArtifactCleanupCandidate,
    owner: &ArtifactDeletionOwner,
) -> Result<()> {
    ensure!(
        owner.source_kind == "manual_cleanup",
        "artifact deletion source mismatch"
    );
    ensure!(
        owner.source_id == job.id,
        "artifact deletion source owner mismatch"
    );
    let mut tx = pool.begin().await?;
    ensure!(
        lock_artifact_cleanup_job_in_tx(&mut tx, job).await?,
        "artifact cleanup ownership lost before finalization"
    );
    ensure!(
        lock_owned_artifact_deletion_in_tx(&mut tx, owner).await?,
        "artifact deletion ownership lost before finalization"
    );
    match candidate.domain.as_str() {
        "job_output" => {
            sqlx::query(
                r#"
                UPDATE job_outputs
                SET storage = 'artifact_deleted', object_key = NULL
                WHERE object_key = $1
                "#,
            )
            .bind(&candidate.object_key)
            .execute(&mut *tx)
            .await?;
        }
        "file_transfer_source" => {
            sqlx::query("DELETE FROM file_transfer_source_artifacts WHERE object_key = $1")
                .bind(&candidate.object_key)
                .execute(&mut *tx)
                .await?;
        }
        "backup_artifact" => {
            let deleted = if let Some(backup_artifact_id) = candidate.backup_artifact_id {
                sqlx::query(
                    r#"
                    DELETE FROM backup_artifacts artifact
                    WHERE artifact.id = $1
                      AND artifact.object_key = $2
                      AND NOT EXISTS (
                          SELECT 1 FROM backup_requests request
                          WHERE request.artifact_id = artifact.id
                      )
                    "#,
                )
                .bind(backup_artifact_id)
                .bind(&candidate.object_key)
                .execute(&mut *tx)
                .await?
            } else {
                sqlx::query(
                    r#"
                    DELETE FROM backup_artifacts artifact
                    WHERE artifact.object_key = $1
                      AND NOT EXISTS (
                          SELECT 1 FROM backup_requests request
                          WHERE request.artifact_id = artifact.id
                      )
                    "#,
                )
                .bind(&candidate.object_key)
                .execute(&mut *tx)
                .await?
            };
            ensure!(
                deleted.rows_affected() <= 1,
                "backup artifact registry identity is not unique"
            );
        }
        "file_transfer_handoff" => {}
        _ => bail!("artifact cleanup domain invalid during finalization"),
    }
    let marked = sqlx::query(
        r#"
        UPDATE server_artifacts
        SET status = 'deleted', deleted_at = now()
        WHERE id = $1
          AND object_key = $2
          AND sha256_hex = $3
          AND size_bytes = $4
          AND status = 'deleting'
        "#,
    )
    .bind(candidate.id)
    .bind(&candidate.object_key)
    .bind(&candidate.sha256_hex)
    .bind(candidate.size_bytes)
    .execute(&mut *tx)
    .await?;
    ensure!(
        marked.rows_affected() == 1,
        "artifact registry identity changed"
    );
    let target = sqlx::query(
        r#"
        UPDATE server_job_artifact_cleanup_targets
        SET outcome = 'deleted', outcome_reason = NULL, processed_at = now()
        WHERE server_job_id = $1
          AND artifact_id = $2
          AND outcome = 'pending'
        "#,
    )
    .bind(job.id)
    .bind(candidate.id)
    .execute(&mut *tx)
    .await?;
    ensure!(
        target.rows_affected() == 1,
        "artifact cleanup target already resolved"
    );
    ensure!(
        finish_artifact_deletion_in_tx(&mut tx, owner).await?,
        "artifact deletion ownership lost during finalization"
    );
    publish_artifact_deletion_completion_in_tx(&mut tx, job.id).await?;
    tx.commit().await?;
    Ok(())
}

async fn tombstone_server_artifact(
    pool: &PgPool,
    job: &ArtifactCleanupJob,
    candidate: &ArtifactCleanupCandidate,
    reason: &str,
) -> Result<ArtifactCleanupDisposition> {
    let mut tx = pool.begin().await?;
    ensure!(
        lock_artifact_cleanup_job_in_tx(&mut tx, job).await?,
        "artifact cleanup ownership lost while tombstoning"
    );
    let marked = sqlx::query(
        r#"
        UPDATE server_artifacts artifact
        SET status = 'tombstoned', tombstoned_at = now()
        WHERE artifact.id = $1
          AND artifact.object_key = $2
          AND artifact.sha256_hex = $3
          AND artifact.size_bytes = $4
          AND artifact.status IN ('active', 'delete_failed')
          AND NOT EXISTS (
              SELECT 1 FROM server_artifact_deletion_intents intent
              WHERE intent.artifact_id = artifact.id
          )
        "#,
    )
    .bind(candidate.id)
    .bind(&candidate.object_key)
    .bind(&candidate.sha256_hex)
    .bind(candidate.size_bytes)
    .execute(&mut *tx)
    .await?;
    if marked.rows_affected() != 1 {
        tx.rollback().await?;
        ensure!(
            mark_artifact_cleanup_target_outcome(
                pool,
                job,
                candidate.id,
                "skipped",
                "artifact_deletion_owned_elsewhere",
            )
            .await?,
            "artifact cleanup ownership lost while skipping tombstone"
        );
        return Ok(ArtifactCleanupDisposition::Skipped);
    }
    let target = sqlx::query(
        r#"
        UPDATE server_job_artifact_cleanup_targets
        SET outcome = 'tombstoned', outcome_reason = $3, processed_at = now()
        WHERE server_job_id = $1
          AND artifact_id = $2
          AND outcome = 'pending'
        "#,
    )
    .bind(job.id)
    .bind(candidate.id)
    .bind(reason)
    .execute(&mut *tx)
    .await?;
    ensure!(
        target.rows_affected() == 1,
        "artifact cleanup target already resolved"
    );
    tx.commit().await?;
    Ok(ArtifactCleanupDisposition::Tombstoned)
}

async fn backup_artifact_is_referenced(
    pool: &PgPool,
    backup_artifact_id: Option<Uuid>,
    object_key: &str,
) -> Result<bool> {
    let referenced = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM backup_requests requests
            JOIN backup_artifacts artifacts ON artifacts.id = requests.artifact_id
            WHERE ($1::uuid IS NOT NULL AND artifacts.id = $1)
               OR artifacts.object_key = $2
        )
        "#,
    )
    .bind(backup_artifact_id)
    .bind(object_key)
    .fetch_one(pool)
    .await?;
    Ok(referenced)
}

async fn artifact_cleanup_targets(
    pool: &PgPool,
    server_job_id: Uuid,
) -> Result<Vec<ArtifactCleanupCandidate>> {
    let detection_limit = i64::try_from(MAX_ARTIFACT_CLEANUP_REVIEWED_TARGETS)?
        .checked_add(1)
        .context("artifact cleanup target detection limit overflow")?;
    let rows = sqlx::query(
        r#"
        SELECT
            target.artifact_id AS id,
            COALESCE(artifact.domain, target.domain) AS domain,
            COALESCE(artifact.object_key, target.object_key) AS object_key,
            COALESCE(artifact.sha256_hex, target.sha256_hex) AS sha256_hex,
            COALESCE(artifact.size_bytes, target.size_bytes) AS size_bytes,
            COALESCE(artifact.status, 'missing') AS status,
            artifact.backup_artifact_id,
            (
                artifact.id IS NOT NULL
                AND artifact.domain = target.domain
                AND artifact.object_key = target.object_key
                AND artifact.sha256_hex = target.sha256_hex
                AND artifact.size_bytes = target.size_bytes
            ) AS identity_matches_review
        FROM server_job_artifact_cleanup_targets target
        LEFT JOIN server_artifacts artifact ON artifact.id = target.artifact_id
        WHERE target.server_job_id = $1
          AND target.outcome = 'pending'
        ORDER BY target.created_at ASC, target.artifact_id ASC
        LIMIT $2
        "#,
    )
    .bind(server_job_id)
    .bind(detection_limit)
    .fetch_all(pool)
    .await?;
    ensure_artifact_cleanup_target_count(rows.len())?;
    rows.into_iter()
        .map(|row| {
            Ok(ArtifactCleanupCandidate {
                id: row.try_get("id")?,
                domain: row.try_get("domain")?,
                object_key: row.try_get("object_key")?,
                sha256_hex: row.try_get("sha256_hex")?,
                size_bytes: row.try_get("size_bytes")?,
                status: row.try_get("status")?,
                backup_artifact_id: row.try_get("backup_artifact_id")?,
                identity_matches_review: row.try_get("identity_matches_review")?,
            })
        })
        .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
        .map_err(Into::into)
}

fn ensure_artifact_cleanup_target_count(count: usize) -> Result<()> {
    ensure!(
        count <= MAX_ARTIFACT_CLEANUP_REVIEWED_TARGETS,
        "artifact_cleanup_reviewed_target_limit_exceeded: job contains more than \
         {MAX_ARTIFACT_CLEANUP_REVIEWED_TARGETS} reviewed targets"
    );
    Ok(())
}

async fn delete_object_key_confirmed(
    object_store: &BackupObjectStore,
    object_key: &str,
) -> Result<()> {
    object_store
        .delete_confirmed(object_key)
        .await
        .with_context(|| format!("failed to delete object {object_key}"))
}

async fn process_due_schedule_work(
    pool: &PgPool,
    limit: i64,
    dispatch_config: &ScheduleDispatchConfig,
) -> Result<usize> {
    // Lifecycle consumption owns its cursor row, event receipts own a scoped
    // advisory lock, and cron materialization reclaims each due schedule row.
    // These independent durable owners make a fleet-wide scheduler lock both
    // redundant and needlessly serializing.
    let event_jobs = process_alert_event_schedules(pool, limit, dispatch_config).await?;
    let cron_jobs = process_due_schedules(pool, limit, dispatch_config).await?;
    Ok(event_jobs + cron_jobs)
}

async fn process_due_schedules(
    pool: &PgPool,
    limit: i64,
    dispatch_config: &ScheduleDispatchConfig,
) -> Result<usize> {
    let round_started_at: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(pool)
        .await?;
    process_due_schedules_through(pool, limit, dispatch_config, round_started_at).await
}

async fn process_due_schedules_through(
    pool: &PgPool,
    limit: i64,
    dispatch_config: &ScheduleDispatchConfig,
    round_started_at: DateTime<Utc>,
) -> Result<usize> {
    let page_limit = limit.clamp(1, 100);
    let mut materialized = 0_usize;
    loop {
        let mut tx = pool.begin().await?;
        let rows = sqlx::query(
            r#"
            SELECT id
            FROM schedules
            WHERE enabled = TRUE
              AND deleted_at IS NULL
              AND trigger_kind = 'cron'
              AND created_at <= $2
              AND next_run_at <= $2
              AND (deferred_until IS NULL OR deferred_until <= $2)
            ORDER BY next_run_at, id
            LIMIT $1
            FOR UPDATE SKIP LOCKED
            "#,
        )
        .bind(page_limit)
        .bind(round_started_at)
        .fetch_all(&mut *tx)
        .await?;
        let selected = rows.len();
        let schedule_ids = rows
            .into_iter()
            .map(|row| row.try_get("id").map_err(Into::into))
            .collect::<Result<Vec<Uuid>>>()?;
        tx.commit().await?;

        if selected > 0 {
            info!(selected, "claimed due schedule page");
        }
        for schedule_id in schedule_ids {
            materialized += process_due_schedule(pool, schedule_id, dispatch_config).await?;
        }
        if selected < page_limit as usize {
            return Ok(materialized);
        }
        // The page controls transaction/lock footprint only. Continue until
        // indexed due work is empty instead of waiting for another 30s cycle.
        tokio::task::yield_now().await;
    }
}

async fn process_due_schedule(
    pool: &PgPool,
    schedule_id: Uuid,
    dispatch_config: &ScheduleDispatchConfig,
) -> Result<usize> {
    let mut claimed_definition_revision = None;
    let result: Result<usize> = async {
        let mut tx = pool.begin().await?;
        let Some(row) = sqlx::query(
            r#"
        SELECT
            id,
            actor_id,
            (SELECT username FROM operators WHERE id = schedules.actor_id) AS actor_username,
            (SELECT role FROM operators WHERE id = schedules.actor_id) AS actor_role,
            name,
            definition_revision,
            operation,
            selector_expression,
            target_client_ids,
            cron_expr,
            EXTRACT(EPOCH FROM next_run_at)::BIGINT AS next_run_at_unix,
            catch_up_policy,
            catch_up_limit,
            retry_delay_secs,
            max_failures,
            failure_count,
            last_error
        FROM schedules
        WHERE id = $1
          AND enabled = TRUE
          AND deleted_at IS NULL
          AND trigger_kind = 'cron'
          AND next_run_at <= now()
          AND (deferred_until IS NULL OR deferred_until <= now())
        FOR UPDATE SKIP LOCKED
        "#,
        )
        .bind(schedule_id)
        .fetch_optional(&mut *tx)
        .await?
        else {
            tx.commit().await?;
            return Ok(0);
        };
        let id: Uuid = row.try_get("id")?;
        let actor_id: Option<Uuid> = row.try_get("actor_id")?;
        let actor_username: Option<String> = row.try_get("actor_username")?;
        let actor_role: Option<String> = row.try_get("actor_role")?;
        let name: String = row.try_get("name")?;
        let raw_operation = row.try_get::<SqlJson<Value>, _>("operation")?.0;
        let operation_revision = payload_hash(raw_operation.to_string().as_bytes());
        let operation = match serde_json::from_value(raw_operation) {
            Ok(operation) => operation,
            Err(_) => {
                disable_schedule_for_invalid_operation(
                    &mut tx,
                    id,
                    actor_id,
                    actor_username.as_deref(),
                    actor_role.as_deref(),
                    &name,
                    &operation_revision,
                )
                .await?;
                tx.commit().await?;
                return Ok(0);
            }
        };
        let definition_revision = row.try_get("definition_revision")?;
        claimed_definition_revision = Some(definition_revision);
        let schedule = DueSchedule {
            id,
            actor_id,
            actor_username,
            actor_role,
            name,
            definition_revision,
            trigger_kind: "cron".to_string(),
            operation,
            selector_expression: row.try_get("selector_expression")?,
            target_client_ids: row.try_get("target_client_ids")?,
            cron_expr: row.try_get("cron_expr")?,
            next_run_at_unix: row.try_get("next_run_at_unix")?,
            catch_up_policy: row.try_get("catch_up_policy")?,
            catch_up_limit: row.try_get("catch_up_limit")?,
            retry_delay_secs: row.try_get("retry_delay_secs")?,
            max_failures: row.try_get("max_failures")?,
            failure_count: row.try_get("failure_count")?,
            last_error: row.try_get("last_error")?,
            materialization: ScheduleMaterializationContext::default(),
        };
        if !actor_authorized_in_tx(
            &mut tx,
            schedule.actor_id,
            "operator",
            &["jobs:write", "schedules:write"],
        )
        .await?
        {
            disable_schedule_for_revoked_actor(&mut tx, &schedule).await?;
            tx.commit().await?;
            return Ok(0);
        }
        if let Some(cadence_error) = schedule_cadence_error(&schedule.cron_expr, Utc::now()) {
            disable_schedule_for_invalid_cadence(&mut tx, &schedule, cadence_error).await?;
            tx.commit().await?;
            return Ok(0);
        }
        let due_occurrences = calculate_due_occurrences(&schedule, Utc::now())?;
        let run_count = catch_up_run_count(&schedule, due_occurrences);
        for run_index in 0..run_count {
            materialize_due_schedule(&mut tx, &schedule, run_index, run_count, dispatch_config)
                .await?;
        }
        advance_schedule_after_materialization(&mut tx, &schedule, run_count).await?;
        tx.commit().await?;
        Ok(run_count as usize)
    }
    .await;

    match result {
        Ok(processed) => Ok(processed),
        Err(error) => {
            if let Some(definition_revision) = claimed_definition_revision {
                record_schedule_failure(pool, schedule_id, definition_revision, &error.to_string())
                    .await?;
            }
            Ok(0)
        }
    }
}

struct DueSchedule {
    id: Uuid,
    actor_id: Option<Uuid>,
    actor_username: Option<String>,
    actor_role: Option<String>,
    name: String,
    definition_revision: i64,
    trigger_kind: String,
    operation: JobCommand,
    selector_expression: String,
    target_client_ids: Vec<String>,
    cron_expr: String,
    next_run_at_unix: i64,
    catch_up_policy: String,
    catch_up_limit: i32,
    retry_delay_secs: i64,
    max_failures: i32,
    failure_count: i32,
    last_error: Option<String>,
    materialization: ScheduleMaterializationContext,
}

#[derive(Clone, Debug, Default)]
struct ScheduleMaterializationContext {
    job_id: Option<Uuid>,
    causation_id: Option<Uuid>,
    schedule_lineage: Vec<Uuid>,
    source_lifecycle_event_seq: Option<i64>,
    source_lifecycle_event_id: Option<Uuid>,
    source_event_id: Option<String>,
    source_payload_hash: Option<String>,
    rendered_operation_hash: Option<String>,
}

#[derive(Clone)]
struct ScheduleDispatchConfig {
    max_timeout_secs: u64,
    max_job_timeout_secs: u64,
    require_registered_agent_updates: bool,
}

impl ScheduleDispatchConfig {
    fn new(
        max_timeout_secs: u64,
        max_job_timeout_secs: u64,
        require_registered_agent_updates: bool,
    ) -> Self {
        let max_job_timeout_secs = max_job_timeout_secs.clamp(1, MAX_CONFIGURABLE_JOB_TIMEOUT_SECS);
        Self {
            max_timeout_secs: max_timeout_secs.clamp(1, max_job_timeout_secs),
            max_job_timeout_secs,
            require_registered_agent_updates,
        }
    }
}

struct ScheduleDueWebhookEvent<'a> {
    schedule: &'a DueSchedule,
    job_id: Uuid,
    command_type: &'a str,
    job_status: &'a str,
    targets: &'a [String],
    run_index: i64,
    run_count: i64,
}

#[derive(Clone, Debug)]
struct ScheduleTargetAvailability {
    capabilities: Vec<TargetCapability>,
    suspended_targets: Vec<String>,
    unavailable_targets: Vec<String>,
    never_connected_targets: Vec<String>,
    missing_targets: Vec<String>,
}

#[derive(Clone, Debug)]
struct ScheduleTargetSkip {
    client_id: String,
    output_type: &'static str,
    reason: &'static str,
    hint: &'static str,
    message: &'static str,
    accepted: bool,
}

async fn materialize_due_schedule(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    schedule: &DueSchedule,
    run_index: i64,
    run_count: i64,
    dispatch_config: &ScheduleDispatchConfig,
) -> Result<bool> {
    let mut targets = schedule
        .target_client_ids
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    targets.sort();
    targets.dedup();
    let operation = schedule.operation.clone();
    let operation_bytes = encode_json(&operation)?;
    let command_hash = payload_hash(&operation_bytes);
    validate_network_command_targets(&operation, &targets)
        .map_err(|error| anyhow::anyhow!(error.code()))?;
    let target_availability = load_schedule_target_capabilities(tx, &targets).await?;
    let available_targets = available_schedule_targets(&targets, &target_availability);
    let max_timeout_secs = effective_schedule_max_timeout_secs(
        dispatch_config.max_timeout_secs,
        dispatch_config.max_job_timeout_secs,
        &available_targets,
        &target_availability.capabilities,
    );
    let (dispatch_targets, capability_skips) = split_targets_by_capability(
        &operation,
        &available_targets,
        &target_availability.capabilities,
        false,
    );
    let operation_type = job_command_operation_type(&operation);
    let job_id = schedule.materialization.job_id.unwrap_or_else(Uuid::new_v4);
    let busy_update_skips =
        load_schedule_busy_update_skips(tx, &operation, &dispatch_targets).await?;
    let busy_update_skip_set = busy_update_skips
        .iter()
        .map(|skip| skip.client_id.as_str())
        .collect::<HashSet<_>>();
    let mut dispatch_targets_after_precomplete = dispatch_targets
        .iter()
        .filter(|client_id| !busy_update_skip_set.contains(client_id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let network_speed_peer_skips =
        network_speed_test_peer_schedule_skips(&operation, &dispatch_targets_after_precomplete);
    let network_speed_peer_skip_set = network_speed_peer_skips
        .iter()
        .map(|skip| skip.client_id.as_str())
        .collect::<HashSet<_>>();
    dispatch_targets_after_precomplete
        .retain(|client_id| !network_speed_peer_skip_set.contains(client_id.as_str()));
    if !scheduled_agent_update_release_policy_allows(
        tx,
        &operation,
        dispatch_config.require_registered_agent_updates,
        &dispatch_targets_after_precomplete,
        &target_availability.capabilities,
    )
    .await?
    {
        bail!("registered agent update release missing");
    }
    let suspended_skips = target_availability
        .suspended_targets
        .iter()
        .cloned()
        .map(suspended_schedule_target_skip)
        .collect::<Vec<_>>();
    let unavailable_skips = target_availability
        .unavailable_targets
        .iter()
        .cloned()
        .map(unavailable_schedule_target_skip)
        .collect::<Vec<_>>();
    let never_connected_skips = target_availability
        .never_connected_targets
        .iter()
        .cloned()
        .map(never_connected_schedule_target_skip)
        .collect::<Vec<_>>();
    let missing_target_skips = target_availability
        .missing_targets
        .iter()
        .cloned()
        .map(missing_schedule_target_skip)
        .collect::<Vec<_>>();
    let busy_update_target_skips = busy_update_skips.clone();
    let schedule_target_skips = suspended_skips
        .iter()
        .chain(unavailable_skips.iter())
        .chain(never_connected_skips.iter())
        .chain(missing_target_skips.iter())
        .chain(busy_update_target_skips.iter())
        .chain(network_speed_peer_skips.iter())
        .cloned()
        .collect::<Vec<_>>();
    let materialized_targets =
        materialized_schedule_targets(&targets, &available_targets, &schedule_target_skips);
    let no_dispatchable_targets = dispatch_targets_after_precomplete.is_empty();
    let status = if no_dispatchable_targets {
        JOB_STATUS_SKIPPED
    } else {
        JOB_STATUS_QUEUED
    };
    let job_completed_immediately = status == JOB_STATUS_SKIPPED;
    let all_targets_skipped = status == JOB_STATUS_SKIPPED;
    let command_type = format!(
        "scheduled_{}",
        scheduled_command_type_label(&operation, operation_type)
    );
    let mut fingerprint_targets = targets.clone();
    fingerprint_targets.sort();
    let request_fingerprint = payload_hash(&serde_json::to_vec(&serde_json::json!({
        "selector_expression": schedule.selector_expression.trim(),
        "command_type": &command_type,
        "operation_payload_hash": &command_hash,
        "targets": fingerprint_targets,
        "max_timeout_secs": max_timeout_secs,
        "privileged": true,
        "force_unprivileged": false,
        "source_schedule_id": schedule.id,
        "schedule_definition_revision": schedule.definition_revision,
        "causation_id": schedule.materialization.causation_id,
        "schedule_lineage": &schedule.materialization.schedule_lineage,
    }))?);
    if let Some(rendered_operation_hash) = &schedule.materialization.rendered_operation_hash {
        ensure!(
            rendered_operation_hash == &command_hash,
            "schedule_rendered_operation_hash_mismatch"
        );
    }
    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, actor_id, command_type, privileged, status, target_count,
            payload_hash, operation, source_schedule_id, request_fingerprint,
            max_timeout_secs, completed_at, causation_id, schedule_lineage
        )
        VALUES ($1, $2, $3, TRUE, $4, $5, $6, $7, $8, $9, $10,
            CASE WHEN $11 THEN now() ELSE NULL END, $12, $13)
        "#,
    )
    .bind(job_id)
    .bind(schedule.actor_id)
    .bind(&command_type)
    .bind(status)
    .bind(materialized_targets.len() as i32)
    .bind(&command_hash)
    .bind(SqlJson(&operation))
    .bind(schedule.id)
    .bind(&request_fingerprint)
    .bind(max_timeout_secs as i64)
    .bind(job_completed_immediately)
    .bind(schedule.materialization.causation_id)
    .bind(&schedule.materialization.schedule_lineage)
    .execute(&mut **tx)
    .await?;

    for client_id in &materialized_targets {
        let skip = capability_skips
            .iter()
            .find(|skip| skip.client_id == *client_id);
        let schedule_skip = schedule_target_skips
            .iter()
            .find(|skip| skip.client_id == *client_id);
        let target_status = if skip.is_some() || schedule_skip.is_some() {
            TARGET_STATUS_SKIPPED
        } else {
            TARGET_STATUS_QUEUED
        };
        sqlx::query(
            r#"
            INSERT INTO job_targets (
                job_id,
                client_id,
                status,
                message,
                exit_code,
                started_at,
                completed_at,
                capability_degraded_reason,
                capability_degraded_hint
            )
            VALUES (
                $1,
                $2,
                $3,
                $4,
                $5,
                CASE WHEN $3 = 'skipped' THEN now() ELSE NULL END,
                CASE WHEN $3 = 'skipped' THEN now() ELSE NULL END,
                $6,
                $7
            )
            "#,
        )
        .bind(job_id)
        .bind(client_id)
        .bind(target_status)
        .bind(
            skip.map(|skip| skip.failure.message)
                .or_else(|| schedule_skip.map(|skip| skip.message)),
        )
        .bind(if skip.is_some() || schedule_skip.is_some() {
            Some(0_i32)
        } else {
            None
        })
        .bind(skip.map(|skip| skip.failure.reason))
        .bind(skip.map(|skip| skip.failure.hint))
        .execute(&mut **tx)
        .await?;
    }

    record_schedule_capability_skip_outputs(tx, job_id, &operation, &capability_skips).await?;
    record_schedule_target_skip_outputs(tx, job_id, &operation, &schedule_target_skips).await?;
    reconcile_scheduled_job_event_sources_in_tx(tx, job_id).await?;

    sqlx::query(
        r#"
        INSERT INTO audit_logs (
            id, actor_id, action, target, command_hash, metadata
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(schedule.actor_id)
    .bind(if targets.is_empty() {
        "schedule.due_no_targets"
    } else {
        "schedule.due_materialized"
    })
    .bind(format!("schedule:{}", schedule.id))
    .bind(&command_hash)
    .bind(serde_json::json!({
        "schedule_id": schedule.id,
        "schedule_name": schedule.name,
        "trigger_kind": &schedule.trigger_kind,
        "definition_revision": schedule.definition_revision,
        "result": if targets.is_empty() { "skipped" } else { "requested" },
        "origin_kind": "worker",
        "component": "schedule-dispatch-worker",
        "operator_id": schedule.actor_id,
        "operator_username": &schedule.actor_username,
        "operator_role": &schedule.actor_role,
        "operation_type": operation_type,
        "job_id": job_id,
        "causation_id": schedule.materialization.causation_id,
        "schedule_lineage": &schedule.materialization.schedule_lineage,
        "source_lifecycle_event_seq": schedule.materialization.source_lifecycle_event_seq,
        "source_lifecycle_event_id": schedule.materialization.source_lifecycle_event_id,
        "source_event_id": &schedule.materialization.source_event_id,
        "source_payload_hash": &schedule.materialization.source_payload_hash,
        "rendered_operation_hash": schedule.materialization.rendered_operation_hash.as_deref().unwrap_or(&command_hash),
        "fixed_targets": &targets,
        "materialized_targets": &materialized_targets,
        "suspended_fixed_targets": &target_availability.suspended_targets,
        "unavailable_fixed_targets": &target_availability.unavailable_targets,
        "never_connected_fixed_targets": &target_availability.never_connected_targets,
        "missing_fixed_targets": &target_availability.missing_targets,
        "busy_update_targets": busy_update_skips.iter().map(|skip| &skip.client_id).collect::<Vec<_>>(),
        "selector_expression": &schedule.selector_expression,
        "catch_up_policy": &schedule.catch_up_policy,
        "catch_up_run_index": run_index + 1,
        "catch_up_run_count": run_count,
        "retry_delay_secs": schedule.retry_delay_secs,
        "max_failures": schedule.max_failures,
            "failure_count_before_run": schedule.failure_count,
            "last_error_before_run": &schedule.last_error,
            "no_work_reason": if all_targets_skipped { Some("all_targets_skipped") } else { None },
            "reason": "saved schedule intent was previously privilege-unlocked; worker materialized a durable job from the fixed target snapshot",
        }))
    .execute(&mut **tx)
    .await?;

    let schedule_update = sqlx::query(
        r#"
        UPDATE schedules
        SET
            last_job_id = $2,
            last_job_status = $3,
            last_job_completed_at = CASE WHEN $4 THEN now() ELSE NULL END,
            last_job_error = CASE
                WHEN NOT $4 THEN NULL
                WHEN $3 IN ('completed', 'skipped') THEN NULL
                ELSE $3
            END,
            failure_count = CASE
                WHEN $4 AND $3 != 'skipped' THEN 0
                ELSE failure_count
            END,
            last_error = CASE
                WHEN $4 AND $3 != 'skipped' THEN NULL
                ELSE last_error
            END,
            updated_at = now()
        WHERE id = $1
          AND definition_revision = $5
        "#,
    )
    .bind(schedule.id)
    .bind(job_id)
    .bind(status)
    .bind(job_completed_immediately)
    .bind(schedule.definition_revision)
    .execute(&mut **tx)
    .await?;
    if schedule.trigger_kind == "cron" {
        ensure!(
            schedule_update.rows_affected() == 1,
            "schedule_definition_revision_changed_before_materialization"
        );
    }

    record_schedule_due_webhook_event(
        tx,
        ScheduleDueWebhookEvent {
            schedule,
            job_id,
            command_type: &command_type,
            job_status: status,
            targets: &materialized_targets,
            run_index,
            run_count,
        },
    )
    .await?;
    if job_completed_immediately {
        record_schedule_job_finished_webhook_event(
            tx,
            schedule,
            job_id,
            &command_type,
            status,
            &materialized_targets,
        )
        .await?;
    }

    Ok(!dispatch_targets_after_precomplete.is_empty())
}

async fn load_schedule_target_capabilities(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    targets: &[String],
) -> Result<ScheduleTargetAvailability> {
    if targets.is_empty() {
        return Ok(ScheduleTargetAvailability {
            capabilities: Vec::new(),
            suspended_targets: Vec::new(),
            unavailable_targets: Vec::new(),
            never_connected_targets: Vec::new(),
            missing_targets: Vec::new(),
        });
    }
    let rows = sqlx::query(
        r#"
        SELECT
            id,
            arch,
            capabilities,
            hidden_at IS NOT NULL AS hidden,
            status,
            process_incarnation_id
        FROM clients
        WHERE id = ANY($1)
        ORDER BY id
        "#,
    )
    .bind(targets.to_vec())
    .fetch_all(&mut **tx)
    .await?;
    let mut present_targets = HashSet::with_capacity(rows.len());
    let mut capabilities = Vec::new();
    let mut suspended_targets = Vec::new();
    let mut unavailable_targets = Vec::new();
    let mut never_connected_targets = Vec::new();
    for row in rows {
        let client_id: String = row.try_get("id")?;
        let hidden: bool = row.try_get("hidden")?;
        let status: String = row.try_get("status")?;
        let process_incarnation_id: Option<Uuid> = row.try_get("process_incarnation_id")?;
        present_targets.insert(client_id.clone());
        if hidden || matches!(status.as_str(), "deleted" | "revoked") {
            unavailable_targets.push(client_id);
        } else if status == "suspended" {
            suspended_targets.push(client_id);
        } else if status == "never" || process_incarnation_id.is_none() {
            never_connected_targets.push(client_id);
        } else {
            let snapshot: SqlJson<AgentCapabilitySnapshot> = row.try_get("capabilities")?;
            capabilities.push(TargetCapability {
                client_id,
                arch: row.try_get("arch")?,
                capabilities: snapshot.0,
            });
        }
    }
    let missing_targets = targets
        .iter()
        .filter(|target| !present_targets.contains(target.as_str()))
        .cloned()
        .collect();
    Ok(ScheduleTargetAvailability {
        capabilities,
        suspended_targets,
        unavailable_targets,
        never_connected_targets,
        missing_targets,
    })
}

fn available_schedule_targets(
    targets: &[String],
    availability: &ScheduleTargetAvailability,
) -> Vec<String> {
    targets
        .iter()
        .filter(|client_id| {
            availability
                .capabilities
                .iter()
                .any(|capability| capability.client_id == **client_id)
        })
        .cloned()
        .collect()
}

fn materialized_schedule_targets(
    targets: &[String],
    available_targets: &[String],
    schedule_target_skips: &[ScheduleTargetSkip],
) -> Vec<String> {
    targets
        .iter()
        .filter(|client_id| {
            available_targets.iter().any(|target| target == *client_id)
                || schedule_target_skips
                    .iter()
                    .any(|skip| skip.client_id == client_id.as_str())
        })
        .cloned()
        .collect()
}

async fn load_schedule_busy_update_skips(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &JobCommand,
    dispatch_targets: &[String],
) -> Result<Vec<ScheduleTargetSkip>> {
    if !is_update_lifecycle_command(command) || dispatch_targets.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT DISTINCT client_id
        FROM job_targets
        WHERE client_id = ANY($1::text[])
          AND completed_at IS NULL
          AND status IN ('queued', 'dispatching', 'running')
        "#,
    )
    .bind(dispatch_targets.to_vec())
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| row.try_get::<String, _>("client_id"))
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .map(busy_update_schedule_target_skip)
        .collect())
}

fn is_update_lifecycle_command(command: &JobCommand) -> bool {
    matches!(
        command,
        JobCommand::UpdateAgent { .. }
            | JobCommand::AgentUpdateActivate { .. }
            | JobCommand::AgentUpdateRollback { .. }
            | JobCommand::AgentUpdateCheck { .. }
    )
}

fn suspended_schedule_target_skip(client_id: String) -> ScheduleTargetSkip {
    ScheduleTargetSkip {
        client_id,
        output_type: "target_suspended",
        reason: "target_suspended",
        hint:
            "manually unsuspend the VPS or wait for an authenticated online event before dispatch",
        message: "target_suspended: target skipped because VPS is suspended",
        accepted: false,
    }
}

fn unavailable_schedule_target_skip(client_id: String) -> ScheduleTargetSkip {
    ScheduleTargetSkip {
        client_id,
        output_type: "schedule_target_skipped",
        reason: "fixed_target_unavailable",
        hint:
            "fixed schedule target is hidden, deleted, revoked, or no longer available for dispatch",
        message: "fixed_target_unavailable: schedule target skipped",
        accepted: false,
    }
}

fn never_connected_schedule_target_skip(client_id: String) -> ScheduleTargetSkip {
    ScheduleTargetSkip {
        client_id,
        output_type: "schedule_target_skipped",
        reason: "target_never_connected",
        hint: "fixed schedule target has no accepted agent process incarnation; start or reconnect the agent before dispatch",
        message: "target_never_connected: schedule target skipped",
        accepted: false,
    }
}

fn missing_schedule_target_skip(client_id: String) -> ScheduleTargetSkip {
    ScheduleTargetSkip {
        client_id,
        output_type: "schedule_target_skipped",
        reason: "fixed_target_missing",
        hint: "fixed schedule target no longer has an inventory row",
        message: "fixed_target_missing: schedule target skipped",
        accepted: false,
    }
}

fn busy_update_schedule_target_skip(client_id: String) -> ScheduleTargetSkip {
    ScheduleTargetSkip {
        client_id,
        output_type: "busy_update_skipped",
        reason: "busy_agent_active_jobs",
        hint: "update command was not dispatched because the client already has another active job target",
        message: "busy_agent_active_jobs: target has another active job; update skipped",
        accepted: true,
    }
}

fn network_speed_test_peer_schedule_skips(
    command: &JobCommand,
    dispatch_targets: &[String],
) -> Vec<ScheduleTargetSkip> {
    let JobCommand::NetworkSpeedTest { plan, .. } = command else {
        return Vec::new();
    };
    let left_dispatchable = dispatch_targets
        .iter()
        .any(|target| target == &plan.left_client_id);
    let right_dispatchable = dispatch_targets
        .iter()
        .any(|target| target == &plan.right_client_id);
    if left_dispatchable == right_dispatchable {
        return Vec::new();
    }
    if left_dispatchable {
        return vec![network_speed_test_peer_schedule_skip(
            plan.left_client_id.clone(),
        )];
    }
    vec![network_speed_test_peer_schedule_skip(
        plan.right_client_id.clone(),
    )]
}

fn network_speed_test_peer_schedule_skip(client_id: String) -> ScheduleTargetSkip {
    ScheduleTargetSkip {
        client_id,
        output_type: "network_speed_test_peer_unavailable",
        reason: "network_speed_test_peer_unavailable",
        hint: "network speed tests require both tunnel endpoints to remain dispatchable after availability filtering",
        message: "network_speed_test_peer_unavailable: peer target was skipped; speed test requires both endpoints",
        accepted: false,
    }
}

fn effective_schedule_max_timeout_secs(
    configured_max_timeout_secs: u64,
    max_job_timeout_secs: u64,
    _targets: &[String],
    _capabilities: &[TargetCapability],
) -> u64 {
    configured_max_timeout_secs.clamp(1, max_job_timeout_secs)
}

async fn scheduled_agent_update_release_policy_allows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &JobCommand,
    require_registered_agent_updates: bool,
    _dispatch_targets: &[String],
    _target_capabilities: &[TargetCapability],
) -> Result<bool> {
    if !require_registered_agent_updates {
        return Ok(true);
    }
    let (column, sha256_hex) = match command {
        JobCommand::UpdateAgent { sha256_hex, .. }
        | JobCommand::AgentUpdateActivate {
            staged_sha256_hex: sha256_hex,
            ..
        } => ("artifact_sha256_hex", sha256_hex.as_str()),
        JobCommand::AgentUpdateRollback {
            rollback_sha256_hex: Some(sha256_hex),
        } => ("rollback_artifact_sha256_hex", sha256_hex.as_str()),
        JobCommand::AgentUpdateRollback {
            rollback_sha256_hex: None,
        } => return Ok(false),
        JobCommand::AgentUpdateCheck { .. } => return Ok(true),
        _ => return Ok(true),
    };
    let artifact_sha256_hex = sha256_hex.to_ascii_lowercase();
    let query = format!(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM agent_update_releases
            WHERE status = 'published_external'
              AND {column} = $1
        )
        "#
    );
    let exists: bool = sqlx::query_scalar(&query)
        .bind(artifact_sha256_hex)
        .fetch_one(&mut **tx)
        .await?;
    Ok(exists)
}

async fn record_schedule_capability_skip_outputs(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job_id: Uuid,
    command: &JobCommand,
    skips: &[CapabilitySkip],
) -> Result<()> {
    for skip in skips {
        let status = serde_json::json!({
            "type": "capability_degraded",
            "status": TARGET_STATUS_SKIPPED,
            "client_id": skip.client_id,
            "command_type": job_command_type_label(command),
            "reason": skip.failure.reason,
            "hint": skip.failure.hint,
        });
        let data = serde_json::to_vec(&status)?;
        sqlx::query(
            r#"
            INSERT INTO job_outputs (
                job_id,
                client_id,
                seq,
                stream,
                data,
                storage,
                object_key,
                data_sha256_hex,
                data_size_bytes,
                exit_code,
                done
            )
            VALUES ($1, $2, 0, 'status', $3, 'inline', NULL, $4, $5, 0, TRUE)
            ON CONFLICT (job_id, client_id, seq)
            DO UPDATE SET
                stream = EXCLUDED.stream,
                data = EXCLUDED.data,
                storage = EXCLUDED.storage,
                object_key = EXCLUDED.object_key,
                data_sha256_hex = EXCLUDED.data_sha256_hex,
                data_size_bytes = EXCLUDED.data_size_bytes,
                exit_code = EXCLUDED.exit_code,
                done = EXCLUDED.done
            "#,
        )
        .bind(job_id)
        .bind(&skip.client_id)
        .bind(&data)
        .bind(payload_hash(&data))
        .bind(data.len() as i64)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO audit_logs (
                id, actor_id, action, target, command_hash, metadata
            )
            VALUES ($1, NULL, 'job.target_result', $2, NULL, $3)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(format!("client:{}", skip.client_id))
        .bind(serde_json::json!({
            "job_id": job_id,
            "status": TARGET_STATUS_SKIPPED,
            "result": TARGET_STATUS_SKIPPED,
            "exit_code": 0,
            "accepted": false,
            "message": skip.failure.message,
            "origin_kind": "worker",
            "component": "schedule-dispatch-worker",
        }))
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn record_schedule_target_skip_outputs(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job_id: Uuid,
    command: &JobCommand,
    skips: &[ScheduleTargetSkip],
) -> Result<()> {
    for skip in skips {
        let status = serde_json::json!({
            "type": skip.output_type,
            "status": TARGET_STATUS_SKIPPED,
            "client_id": skip.client_id,
            "command_type": job_command_type_label(command),
            "reason": skip.reason,
            "hint": skip.hint,
        });
        let data = serde_json::to_vec(&status)?;
        sqlx::query(
            r#"
            INSERT INTO job_outputs (
                job_id,
                client_id,
                seq,
                stream,
                data,
                storage,
                object_key,
                data_sha256_hex,
                data_size_bytes,
                exit_code,
                done
            )
            VALUES ($1, $2, 0, 'status', $3, 'inline', NULL, $4, $5, 0, TRUE)
            ON CONFLICT (job_id, client_id, seq)
            DO UPDATE SET
                stream = EXCLUDED.stream,
                data = EXCLUDED.data,
                storage = EXCLUDED.storage,
                object_key = EXCLUDED.object_key,
                data_sha256_hex = EXCLUDED.data_sha256_hex,
                data_size_bytes = EXCLUDED.data_size_bytes,
                exit_code = EXCLUDED.exit_code,
                done = EXCLUDED.done
            "#,
        )
        .bind(job_id)
        .bind(&skip.client_id)
        .bind(&data)
        .bind(payload_hash(&data))
        .bind(data.len() as i64)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO audit_logs (
                id, actor_id, action, target, command_hash, metadata
            )
            VALUES ($1, NULL, 'job.target_result', $2, NULL, $3)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(format!("client:{}", skip.client_id))
        .bind(serde_json::json!({
            "job_id": job_id,
            "status": TARGET_STATUS_SKIPPED,
            "result": TARGET_STATUS_SKIPPED,
            "exit_code": 0,
            "accepted": skip.accepted,
            "message": skip.message,
            "origin_kind": "worker",
            "component": "schedule-dispatch-worker",
        }))
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn record_schedule_due_webhook_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    input: ScheduleDueWebhookEvent<'_>,
) -> Result<()> {
    let schedule = input.schedule;
    let event_id = format!("schedule:{}:job:{}:due", schedule.id, input.job_id);
    let predicates = schedule_job_predicates(
        schedule,
        "schedule.due",
        input.command_type,
        input.job_status,
    );
    insert_webhook_event_with_provenance_at_in_tx(
        tx,
        "schedule.due",
        &event_id,
        &predicates,
        input.targets,
        serde_json::json!({
            "event": {
                "kind": "schedule.due",
                "id": event_id,
                "predicates": &predicates,
            },
            "schedule": {
                "id": schedule.id,
                "name": &schedule.name,
                "trigger_kind": &schedule.trigger_kind,
                "definition_revision": schedule.definition_revision,
                "command_type": input.command_type,
                "selector_expression": &schedule.selector_expression,
                "fixed_target_ids": input.targets,
                "catch_up_policy": &schedule.catch_up_policy,
                "catch_up_run_index": input.run_index + 1,
                "catch_up_run_count": input.run_count,
                "target_ids": input.targets,
            },
            "job": {
                "id": input.job_id,
                "status": input.job_status,
                "type": input.command_type,
                "source_schedule_id": schedule.id,
                "causation_id": schedule.materialization.causation_id,
                "schedule_lineage": &schedule.materialization.schedule_lineage,
                "target_count": input.targets.len(),
            },
        }),
        Utc::now(),
        schedule.materialization.source_lifecycle_event_seq,
        schedule.materialization.causation_id,
        &schedule.materialization.schedule_lineage,
    )
    .await?;
    Ok(())
}

async fn record_schedule_job_finished_webhook_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    schedule: &DueSchedule,
    job_id: Uuid,
    command_type: &str,
    job_status: &str,
    targets: &[String],
) -> Result<()> {
    let event_id = format!("schedule:{}:job:{}:finished", schedule.id, job_id);
    let mut predicates = vec![
        "schedule.job_finished".to_string(),
        format!("schedule.id:{}", schedule.id),
        format!("schedule.name:{}", schedule.name),
        format!("job.status:{job_status}"),
        format!("job.status.become_{job_status}"),
        format!("job.type:{command_type}"),
    ];
    predicates.sort();
    predicates.dedup();
    insert_webhook_event_with_provenance_at_in_tx(
        tx,
        "schedule.job_finished",
        &event_id,
        &predicates,
        targets,
        serde_json::json!({
            "event": {
                "kind": "schedule.job_finished",
                "id": event_id,
                "predicates": &predicates,
            },
            "schedule": {
                "id": schedule.id,
                "name": &schedule.name,
                "trigger_kind": &schedule.trigger_kind,
                "definition_revision": schedule.definition_revision,
                "last_job_id": job_id,
                "last_job_status": job_status,
                "last_job_error": null,
            },
            "job": {
                "id": job_id,
                "status": job_status,
                "type": command_type,
                "source_schedule_id": schedule.id,
                "causation_id": schedule.materialization.causation_id,
                "schedule_lineage": &schedule.materialization.schedule_lineage,
                "target_count": targets.len(),
                "target_ids": targets,
            },
        }),
        Utc::now(),
        schedule.materialization.source_lifecycle_event_seq,
        schedule.materialization.causation_id,
        &schedule.materialization.schedule_lineage,
    )
    .await?;
    Ok(())
}

fn schedule_job_predicates(
    schedule: &DueSchedule,
    schedule_predicate: &str,
    command_type: &str,
    job_status: &str,
) -> Vec<String> {
    let mut predicates = vec![
        schedule_predicate.to_string(),
        format!("schedule.id:{}", schedule.id),
        format!("schedule.name:{}", schedule.name),
        "job.created".to_string(),
        format!("job.status:{job_status}"),
        format!("job.status.become_{job_status}"),
        format!("job.type:{command_type}"),
    ];
    predicates.sort();
    predicates.dedup();
    predicates
}

async fn advance_schedule_after_materialization(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    schedule: &DueSchedule,
    run_count: i64,
) -> Result<()> {
    let next_run_at = next_run_after_success(schedule, run_count, Utc::now())?;
    let advanced = sqlx::query(
        r#"
        UPDATE schedules
        SET
            last_run_at = now(),
            next_run_at = to_timestamp($2),
            updated_at = now()
        WHERE id = $1
          AND definition_revision = $3
        "#,
    )
    .bind(schedule.id)
    .bind(next_run_at.timestamp() as f64)
    .bind(schedule.definition_revision)
    .execute(&mut **tx)
    .await?;
    ensure!(
        advanced.rows_affected() == 1,
        "schedule_definition_revision_changed_before_advance"
    );
    Ok(())
}

async fn record_schedule_failure(
    pool: &PgPool,
    schedule_id: Uuid,
    definition_revision: i64,
    error: &str,
) -> Result<()> {
    let bounded_error = truncate_schedule_error(error);
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        r#"
        UPDATE schedules
        SET
            failure_count = failure_count + 1,
            last_error = $2,
            enabled = CASE
                WHEN failure_count + 1 >= max_failures THEN FALSE
                ELSE enabled
            END,
            next_run_at = CASE
                WHEN failure_count + 1 >= max_failures THEN next_run_at
                ELSE now() + (retry_delay_secs * interval '1 second')
            END,
            updated_at = now()
        WHERE id = $1
          AND definition_revision = $3
          AND enabled = TRUE
          AND deleted_at IS NULL
          AND trigger_kind = 'cron'
        RETURNING
            id,
            actor_id,
            name,
            enabled,
            failure_count,
            max_failures,
            retry_delay_secs,
            next_run_at::text AS next_run_at
    "#,
    )
    .bind(schedule_id)
    .bind(&bounded_error)
    .bind(definition_revision)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(());
    };
    let actor_id: Option<Uuid> = row.try_get("actor_id")?;
    let (actor_username, actor_role) = if let Some(actor_id) = actor_id {
        sqlx::query_as::<_, (String, String)>("SELECT username, role FROM operators WHERE id = $1")
            .bind(actor_id)
            .fetch_optional(&mut *tx)
            .await?
            .map_or((None, None), |(username, role)| {
                (Some(username), Some(role))
            })
    } else {
        (None, None)
    };
    let failure_count: i32 = row.try_get("failure_count")?;
    let max_failures: i32 = row.try_get("max_failures")?;
    let enabled: bool = row.try_get("enabled")?;
    let schedule_name: String = row.try_get("name")?;
    let retry_delay_secs: i64 = row.try_get("retry_delay_secs")?;
    let next_run_at: String = row.try_get("next_run_at")?;
    sqlx::query(
        r#"
        INSERT INTO audit_logs (
            id, actor_id, action, target, command_hash, metadata
        )
        VALUES ($1, $2, $3, $4, NULL, $5)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(actor_id)
    .bind("schedule.due_failed")
    .bind(format!("schedule:{schedule_id}"))
    .bind(serde_json::json!({
        "origin_kind": "worker",
        "component": "schedule-dispatch-worker",
        "result": "failed",
        "schedule_id": schedule_id,
        "schedule_name": &schedule_name,
        "operator_id": actor_id,
        "operator_username": &actor_username,
        "operator_role": &actor_role,
        "failure_count": failure_count,
        "max_failures": max_failures,
        "retry_delay_secs": retry_delay_secs,
        "next_run_at": &next_run_at,
        "disabled": !enabled,
        "error": &bounded_error,
    }))
    .execute(&mut *tx)
    .await?;
    let event_id = format!("schedule:{schedule_id}:failed:{}", Uuid::new_v4());
    let predicates = vec![
        "schedule.failed".to_string(),
        format!("schedule.id:{schedule_id}"),
        format!("schedule.name:{schedule_name}"),
    ];
    insert_webhook_event_in_tx(
        &mut tx,
        "schedule.failed",
        &event_id,
        &predicates,
        &[],
        serde_json::json!({
            "event": {
                "kind": "schedule.failed",
                "id": event_id,
                "predicates": &predicates,
            },
            "schedule": {
                "id": schedule_id,
                "name": &schedule_name,
                "failure_count": failure_count,
                "max_failures": max_failures,
                "retry_delay_secs": retry_delay_secs,
                "next_run_at": &next_run_at,
                "disabled": !enabled,
                "error": &bounded_error,
            },
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn disable_schedule_for_revoked_actor(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    schedule: &DueSchedule,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE schedules
        SET enabled = FALSE,
            last_error = 'actor_authority_revoked',
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(schedule.id)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO audit_logs (
            id, actor_id, action, target, command_hash, metadata
        )
        VALUES ($1, NULL, $2, $3, NULL, $4)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind("schedule.disabled_actor_authority_revoked")
    .bind(format!("schedule:{}", schedule.id))
    .bind(serde_json::json!({
        "worker": "schedule_dispatch_worker",
        "origin_kind": "worker",
        "component": "schedule-dispatch-worker",
        "result": "rejected",
        "schedule_id": schedule.id,
        "schedule_name": &schedule.name,
        "referenced_operator_id": schedule.actor_id,
        "referenced_operator_username": &schedule.actor_username,
        "referenced_operator_role": &schedule.actor_role,
        "reason": "actor_authority_revoked",
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn disable_schedule_for_invalid_operation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    schedule_id: Uuid,
    actor_id: Option<Uuid>,
    actor_username: Option<&str>,
    actor_role: Option<&str>,
    schedule_name: &str,
    operation_revision: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE schedules
        SET enabled = FALSE,
            last_error = $2,
            updated_at = now()
        WHERE id = $1
          AND enabled = TRUE
        "#,
    )
    .bind(schedule_id)
    .bind(SCHEDULE_OPERATION_INVALID)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO audit_logs (
            id, actor_id, action, target, command_hash, metadata
        )
        VALUES ($1, $2, 'schedule.due_failed', $3, $4, $5)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(actor_id)
    .bind(format!("schedule:{schedule_id}"))
    .bind(operation_revision)
    .bind(serde_json::json!({
        "worker": "schedule_dispatch_worker",
        "origin_kind": "worker",
        "component": "schedule-dispatch-worker",
        "result": "failed",
        "schedule_id": schedule_id,
        "schedule_name": schedule_name,
        "operator_id": actor_id,
        "operator_username": actor_username,
        "operator_role": actor_role,
        "disabled": true,
        "permanent": true,
        "error": SCHEDULE_OPERATION_INVALID,
        "operation_payload_hash": operation_revision,
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn schedule_cadence_error(cron_expr: &str, now: DateTime<Utc>) -> Option<&'static str> {
    let Ok(cron) = Cron::from_str(cron_expr) else {
        return Some(SCHEDULE_CRON_INVALID);
    };
    if cron.iter_after(now).next().is_none() {
        return Some(SCHEDULE_CRON_NO_FUTURE_OCCURRENCE);
    }
    None
}

async fn disable_schedule_for_invalid_cadence(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    schedule: &DueSchedule,
    cadence_error: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE schedules
        SET enabled = FALSE,
            last_error = $2,
            updated_at = now()
        WHERE id = $1
          AND enabled = TRUE
        "#,
    )
    .bind(schedule.id)
    .bind(cadence_error)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO audit_logs (
            id, actor_id, action, target, command_hash, metadata
        )
        VALUES ($1, $2, 'schedule.due_failed', $3, NULL, $4)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(schedule.actor_id)
    .bind(format!("schedule:{}", schedule.id))
    .bind(serde_json::json!({
        "worker": "schedule_dispatch_worker",
        "origin_kind": "worker",
        "component": "schedule-dispatch-worker",
        "result": "failed",
        "schedule_id": schedule.id,
        "schedule_name": &schedule.name,
        "operator_id": schedule.actor_id,
        "operator_username": &schedule.actor_username,
        "operator_role": &schedule.actor_role,
        "disabled": true,
        "permanent": true,
        "error": cadence_error,
    }))
    .execute(&mut **tx)
    .await?;

    let cadence_revision = payload_hash(schedule.cron_expr.as_bytes());
    let event_id = format!(
        "schedule:{}:invalid_cadence:{}",
        schedule.id,
        &cadence_revision[..16]
    );
    let predicates = vec![
        "schedule.failed".to_string(),
        format!("schedule.id:{}", schedule.id),
        format!("schedule.name:{}", schedule.name),
    ];
    insert_webhook_event_in_tx(
        tx,
        "schedule.failed",
        &event_id,
        &predicates,
        &[],
        serde_json::json!({
            "event": {
                "kind": "schedule.failed",
                "id": event_id,
                "predicates": &predicates,
            },
            "schedule": {
                "id": schedule.id,
                "name": &schedule.name,
                "disabled": true,
                "permanent": true,
                "error": cadence_error,
            },
        }),
    )
    .await?;
    Ok(())
}

fn catch_up_run_count(schedule: &DueSchedule, due_occurrences: i64) -> i64 {
    let due_occurrences = due_occurrences.max(1);
    match schedule.catch_up_policy.as_str() {
        "run_all_limited" => due_occurrences
            .min(schedule.catch_up_limit as i64)
            .clamp(1, 25),
        "run_once" => 1,
        _ => 1,
    }
}

fn calculate_due_occurrences(schedule: &DueSchedule, now: DateTime<Utc>) -> Result<i64> {
    if schedule.catch_up_policy != "run_all_limited" {
        return Ok(1);
    }
    let current = date_time_from_unix(schedule.next_run_at_unix)?;
    let cron = Cron::from_str(&schedule.cron_expr)
        .with_context(|| format!("invalid cron expression for schedule {}", schedule.id))?;
    let mut count = 1_i64;
    let max_count = i64::from(schedule.catch_up_limit.clamp(1, 25));
    for run in cron.iter_after(current) {
        if run > now || count >= max_count {
            break;
        }
        count += 1;
    }
    Ok(count)
}

fn next_run_after_success(
    schedule: &DueSchedule,
    run_count: i64,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>> {
    let cron = Cron::from_str(&schedule.cron_expr)
        .with_context(|| format!("invalid cron expression for schedule {}", schedule.id))?;
    let mut cursor = if schedule.catch_up_policy == "skip_missed" {
        now
    } else {
        date_time_from_unix(schedule.next_run_at_unix)?
    };
    let steps = if schedule.catch_up_policy == "skip_missed" {
        1
    } else {
        run_count.max(1)
    };
    for _ in 0..steps {
        cursor = cron
            .iter_after(cursor)
            .next()
            .context("cron expression produced no future run")?;
    }
    Ok(cursor)
}

fn date_time_from_unix(timestamp: i64) -> Result<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp(timestamp, 0).context("invalid schedule timestamp")
}

fn truncate_schedule_error(error: &str) -> String {
    error.chars().take(1024).collect()
}

#[cfg(test)]
#[path = "runtime/tests_operational_alerts.rs"]
mod operational_alert_tests;
#[cfg(test)]
#[path = "runtime/tests_schedule.rs"]
mod schedule_tests;
