use std::{collections::HashSet, path::PathBuf, str::FromStr, time::Duration};

use anyhow::{bail, ensure, Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use croner::Cron;
use serde_json::Value;
use sqlx::{
    postgres::{PgListener, PgPoolOptions},
    types::Json as SqlJson,
    PgPool, Row,
};
use tokio::time;
use tracing::{debug, info, warn};
use uuid::Uuid;
use vpsman_common::{
    encode_json, job_command_operation_type, payload_hash, read_secret_file_ref,
    AgentCapabilitySnapshot, JobCommand, SuiteConfig, ARTIFACT_CLEANUP_RUNNING_TIMEOUT_SECS,
    DEFAULT_MAX_JOB_TIMEOUT_SECS, MAX_CONFIGURABLE_JOB_TIMEOUT_SECS, SERVER_JOB_STATUS_COMPLETED,
    SERVER_JOB_STATUS_FAILED, SERVER_JOB_STATUS_QUEUED, SERVER_JOB_STATUS_RUNNING,
    SERVER_JOB_TYPE_ARTIFACT_CLEANUP,
};
#[cfg(test)]
use vpsman_common::{
    expression_matches, parse_expression, plan_tunnel, BandwidthTier, ExpressionContext,
    OspfCostPolicy, TunnelAddressPair, TunnelEndpointSide, TunnelKind, TunnelPlanInput,
    VpsMetadata,
};
use vpsman_object_store::{BackupObjectStore, S3BackupObjectStoreSettings};
use vpsman_server_core::{
    job_command_type_label, scheduled_command_type_label, split_targets_by_capability,
    validate_network_command_targets, CapabilitySkip, TargetCapability, JOB_STATUS_QUEUED,
    JOB_STATUS_SKIPPED, TARGET_STATUS_QUEUED, TARGET_STATUS_SKIPPED,
};

const DEFAULT_BACKUP_OBJECT_STORE_DIR: &str = "runtime/data/objects/backups";
mod actor_authority;
mod alert_notifications;
mod backup_policy_retention;
mod build_info;
mod webhook_rules;
mod worker_leases;

use actor_authority::{actor_authorized, actor_authorized_in_tx};
use alert_notifications::{
    process_alert_notifications, AlertNotificationWorkerConfig, AlertNotificationWorkerRun,
};
use backup_policy_retention::{
    process_backup_policy_retention_prune, BackupPolicyRetentionPruneConfig,
    BackupPolicyRetentionPruneRun,
};
use webhook_rules::{
    ensure_event_partitions, insert_webhook_event_in_tx, process_webhook_rules,
    WebhookRuleWorkerConfig, WebhookRuleWorkerRun,
};
use worker_leases::acquire_worker_lease;

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
    #[arg(long, env = "VPSMAN_WORKER_ID")]
    worker_id: Option<String>,
    #[arg(long, env = "VPSMAN_WORKER_LEASE_SECS", default_value_t = 60)]
    worker_lease_secs: i32,
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
    worker_lease_secs: i32,
    agent_offline_timeout_secs: i64,
    alert_notification_config: AlertNotificationWorkerConfig,
    webhook_rule_config: WebhookRuleWorkerConfig,
    backup_policy_prune_config: BackupPolicyRetentionPruneConfig,
    schedule_dispatch_config: ScheduleDispatchConfig,
    backup_object_store: BackupObjectStore,
}

#[derive(Clone, Copy)]
struct ArtifactObjectStores<'a> {
    backup: &'a BackupObjectStore,
}

impl WorkerRuntimeConfig {
    fn from_args(args: &Args) -> Result<Self> {
        let backup_object_store = build_backup_object_store(args)?;
        let backup_policy_prune_object_store = build_backup_policy_prune_object_store(args)?
            .or_else(|| Some(backup_object_store.clone()));
        Ok(Self {
            tick_secs: args.tick_secs.max(1),
            worker_lease_secs: args.worker_lease_secs,
            agent_offline_timeout_secs: args.agent_offline_timeout_secs,
            alert_notification_config: AlertNotificationWorkerConfig::new(
                args.notification_delivery_limit,
                args.notification_retention_days,
                args.notification_retention_prune_limit,
                args.notification_webhook_timeout_secs,
            ),
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
        apply_opt_string(
            &mut self.worker_id,
            "VPSMAN_WORKER_ID",
            config.worker.worker_id.as_deref(),
        );
        apply_i32_default(
            &mut self.worker_lease_secs,
            "VPSMAN_WORKER_LEASE_SECS",
            config.worker.worker_lease_secs,
        );
        apply_i64_default(
            &mut self.agent_offline_timeout_secs,
            "VPSMAN_AGENT_OFFLINE_TIMEOUT_SECS",
            config
                .worker
                .agent_offline_timeout_secs
                .or(config.timeout.agent_offline_secs),
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
            config
                .worker
                .schedule_job_max_timeout_secs
                .or(config.timeout.worker_schedule_job_max_timeout_secs),
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

fn apply_i32_default(target: &mut i32, env_name: &str, value: Option<i32>) {
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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "vpsman_worker=info".into()),
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
        warn!("VPSMAN_POSTGRES_URL is not configured; worker cannot process durable queues");
        if args.once {
            return Ok(());
        }
        let mut ticker = time::interval(Duration::from_secs(args.tick_secs.max(1)));
        loop {
            ticker.tick().await;
            warn!("worker tick skipped: PostgreSQL is not configured");
        }
    };
    let pool = connect_postgres(
        postgres_url,
        &args.migrations_dir,
        args.db_max_connections.clamp(1, 256),
    )
    .await?;
    let worker_id = args
        .worker_id
        .clone()
        .unwrap_or_else(|| format!("vpsman-worker-{}", std::process::id()));
    info!(tick_secs = args.tick_secs, "worker started");
    if args.once {
        let runtime_config = WorkerRuntimeConfig::from_args(&args)?;
        let schedules_processed = process_due_schedules_if_leader(
            &pool,
            25,
            &worker_id,
            runtime_config.worker_lease_secs,
            &runtime_config.schedule_dispatch_config,
        )
        .await?;
        let alert_notifications = process_alert_notifications_if_leader(
            &pool,
            runtime_config.alert_notification_config,
            &worker_id,
            runtime_config.worker_lease_secs,
        )
        .await?;
        let webhook_rules = process_webhook_rules_if_leader(
            &pool,
            runtime_config.webhook_rule_config,
            &worker_id,
            runtime_config.worker_lease_secs,
        )
        .await?;
        let backup_policy_prune = process_backup_policy_retention_prune_if_leader(
            &pool,
            runtime_config.backup_policy_prune_config.clone(),
            &worker_id,
            runtime_config.worker_lease_secs,
        )
        .await?;
        let artifact_cleanup = process_artifact_cleanup_jobs_if_leader(
            &pool,
            ArtifactObjectStores {
                backup: &runtime_config.backup_object_store,
            },
            &worker_id,
            runtime_config.worker_lease_secs,
        )
        .await?;
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
            backup_policy_prune_policies = backup_policy_prune.policies_scanned,
            backup_policy_prune_matched = backup_policy_prune.matched_rows,
            backup_policy_prune_pruned = backup_policy_prune.pruned_rows,
            artifact_cleanup_jobs = artifact_cleanup.jobs,
            artifact_cleanup_deleted = artifact_cleanup.deleted_rows,
            "worker once completed"
        );
        return Ok(());
    }

    let mut current_tick_secs = args.tick_secs.max(1);
    let mut ticker = time::interval(Duration::from_secs(current_tick_secs));
    let mut last_offline_check = tokio::time::Instant::now();
    let mut webhook_listener = match connect_webhook_listener(postgres_url).await {
        Ok(listener) => Some(listener),
        Err(error) => {
            warn!(%error, "failed to start webhook event listener; polling fallback remains active");
            None
        }
    };
    loop {
        let mut webhook_listener_failed = false;
        if let Some(listener) = webhook_listener.as_mut() {
            tokio::select! {
                _ = ticker.tick() => {}
                notification = listener.recv() => {
                    match notification {
                        Ok(notification) => {
                            debug!(
                                channel = notification.channel(),
                                payload = notification.payload(),
                                "webhook event notification woke worker"
                            );
                        }
                        Err(error) => {
                            warn!(%error, "webhook event listener failed; returning to polling fallback");
                            webhook_listener_failed = true;
                        }
                    }
                }
            }
            if webhook_listener_failed {
                webhook_listener = None;
            }
        } else {
            ticker.tick().await;
            match connect_webhook_listener(postgres_url).await {
                Ok(listener) => {
                    info!("webhook event listener reconnected");
                    webhook_listener = Some(listener);
                }
                Err(error) => debug!(%error, "webhook event listener reconnect failed"),
            }
        }
        let runtime_config = match load_worker_runtime_config(&base_args) {
            Ok(config) => config,
            Err(error) => {
                warn!(%error, "failed to hot-reload worker suite config; using startup runtime config");
                match WorkerRuntimeConfig::from_args(&args) {
                    Ok(config) => config,
                    Err(error) => {
                        warn!(%error, "failed to build startup worker runtime config");
                        continue;
                    }
                }
            }
        };
        if runtime_config.tick_secs != current_tick_secs {
            current_tick_secs = runtime_config.tick_secs;
            ticker = time::interval(Duration::from_secs(current_tick_secs));
            info!(
                tick_secs = current_tick_secs,
                "worker tick interval hot-reloaded"
            );
        }
        match process_due_schedules_if_leader(
            &pool,
            25,
            &worker_id,
            runtime_config.worker_lease_secs,
            &runtime_config.schedule_dispatch_config,
        )
        .await
        {
            Ok(processed) => {
                if processed > 0 {
                    info!(processed, "processed due schedules");
                }
            }
            Err(error) => warn!(%error, "failed to process due schedules"),
        }
        match process_alert_notifications_if_leader(
            &pool,
            runtime_config.alert_notification_config,
            &worker_id,
            runtime_config.worker_lease_secs,
        )
        .await
        {
            Ok(run) => {
                if run.processed > 0 || run.pruned > 0 {
                    info!(
                        processed = run.processed,
                        delivered = run.delivered,
                        failed = run.failed,
                        pruned = run.pruned,
                        "processed fleet alert notifications"
                    );
                }
            }
            Err(error) => warn!(%error, "failed to process fleet alert notifications"),
        }
        match process_webhook_rules_if_leader(
            &pool,
            runtime_config.webhook_rule_config,
            &worker_id,
            runtime_config.worker_lease_secs,
        )
        .await
        {
            Ok(run) => {
                if run.materialized > 0 || run.processed > 0 || run.pruned > 0 {
                    info!(
                        materialized = run.materialized,
                        processed = run.processed,
                        delivered = run.delivered,
                        failed = run.failed,
                        pruned = run.pruned,
                        "processed webhook rules"
                    );
                }
            }
            Err(error) => warn!(%error, "failed to process webhook rules"),
        }
        match process_backup_policy_retention_prune_if_leader(
            &pool,
            runtime_config.backup_policy_prune_config.clone(),
            &worker_id,
            runtime_config.worker_lease_secs,
        )
        .await
        {
            Ok(run) => {
                if run.matched_rows > 0 || run.pruned_rows > 0 {
                    info!(
                        policies_scanned = run.policies_scanned,
                        matched_rows = run.matched_rows,
                        pruned_rows = run.pruned_rows,
                        "processed backup policy retention prune"
                    );
                }
            }
            Err(error) => warn!(%error, "failed to process backup policy retention prune"),
        }
        match process_artifact_cleanup_jobs_if_leader(
            &pool,
            ArtifactObjectStores {
                backup: &runtime_config.backup_object_store,
            },
            &worker_id,
            runtime_config.worker_lease_secs,
        )
        .await
        {
            Ok(run) => {
                if run.jobs > 0 || run.deleted_rows > 0 {
                    info!(
                        jobs = run.jobs,
                        deleted_rows = run.deleted_rows,
                        deleted_bytes = run.deleted_bytes,
                        "processed artifact cleanup jobs"
                    );
                }
            }
            Err(error) => warn!(%error, "failed to process artifact cleanup jobs"),
        }
        if last_offline_check.elapsed() >= Duration::from_secs(60) {
            last_offline_check = tokio::time::Instant::now();
            match detect_offline_agents(&pool, runtime_config.agent_offline_timeout_secs).await {
                Ok(count) => {
                    if count > 0 {
                        info!(count, "detected offline agents");
                    }
                }
                Err(error) => warn!(%error, "failed to detect offline agents"),
            }
            match expire_stale_gateway_sessions(&pool, runtime_config.agent_offline_timeout_secs)
                .await
            {
                Ok(count) => {
                    if count > 0 {
                        info!(count, "expired stale gateway sessions");
                    }
                }
                Err(error) => warn!(%error, "failed to expire stale gateway sessions"),
            }
        }
    }
}

async fn detect_offline_agents(pool: &PgPool, offline_timeout_secs: i64) -> Result<u64> {
    let mut tx = pool.begin().await?;
    let rows = sqlx::query(
        r#"
        UPDATE clients
        SET status = 'offline'
        WHERE status = 'online'
          AND last_seen_at < now() - make_interval(secs => $1)
        RETURNING id
        "#,
    )
    .bind(offline_timeout_secs as f64)
    .fetch_all(&mut *tx)
    .await?;
    for row in &rows {
        let client_id: String = row.try_get("id")?;
        let metadata = serde_json::json!({
            "from_status": "online",
            "to_status": "offline",
            "reason": "agent_offline_timeout",
            "offline_timeout_secs": offline_timeout_secs,
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
    }
    tx.commit().await?;
    Ok(rows.len() as u64)
}

async fn expire_stale_gateway_sessions(pool: &PgPool, offline_timeout_secs: i64) -> Result<u64> {
    let rows = sqlx::query(
        r#"
        UPDATE gateway_sessions session
        SET
            status = 'expired',
            last_seen_at = now(),
            ended_at = COALESCE(session.ended_at, now()),
            end_reason = COALESCE(session.end_reason, 'agent_offline_timeout')
        FROM clients client
        WHERE session.client_id = client.id
          AND session.status = 'active'
          AND (
            client.hidden_at IS NOT NULL
            OR client.status IN ('offline', 'disconnected')
            OR client.last_seen_at IS NULL
            OR client.last_seen_at < now() - make_interval(secs => $1)
          )
        RETURNING session.id
        "#,
    )
    .bind(offline_timeout_secs as f64)
    .fetch_all(pool)
    .await?;
    Ok(rows.len() as u64)
}

async fn connect_postgres(
    postgres_url: &str,
    migrations_dir: &std::path::Path,
    max_connections: u32,
) -> Result<PgPool> {
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
    Ok(pool)
}

async fn connect_webhook_listener(postgres_url: &str) -> Result<PgListener> {
    let mut listener = PgListener::connect(postgres_url)
        .await
        .context("failed to connect PostgreSQL webhook listener")?;
    listener
        .listen("webhook_events")
        .await
        .context("failed to listen for webhook_events notifications")?;
    Ok(listener)
}

async fn process_webhook_rules_if_leader(
    pool: &PgPool,
    config: WebhookRuleWorkerConfig,
    worker_id: &str,
    lease_secs: i32,
) -> Result<WebhookRuleWorkerRun> {
    if !acquire_worker_lease(pool, "webhook_rules", worker_id, lease_secs).await? {
        debug!(
            worker_id,
            "skipped webhook rules because another worker holds the lease"
        );
        return Ok(WebhookRuleWorkerRun::default());
    }
    process_webhook_rules(pool, config).await
}

async fn process_alert_notifications_if_leader(
    pool: &PgPool,
    config: AlertNotificationWorkerConfig,
    worker_id: &str,
    lease_secs: i32,
) -> Result<AlertNotificationWorkerRun> {
    if !acquire_worker_lease(pool, "alert_notifications", worker_id, lease_secs).await? {
        debug!(
            worker_id,
            "skipped fleet alert notifications because another worker holds the lease"
        );
        return Ok(AlertNotificationWorkerRun::default());
    }
    process_alert_notifications(pool, config).await
}

async fn process_backup_policy_retention_prune_if_leader(
    pool: &PgPool,
    config: BackupPolicyRetentionPruneConfig,
    worker_id: &str,
    lease_secs: i32,
) -> Result<BackupPolicyRetentionPruneRun> {
    if !config.enabled {
        return Ok(BackupPolicyRetentionPruneRun::default());
    }
    if !acquire_worker_lease(pool, "backup_policy_retention_prune", worker_id, lease_secs).await? {
        debug!(
            worker_id,
            "skipped backup policy retention prune because another worker holds the lease"
        );
        return Ok(BackupPolicyRetentionPruneRun::default());
    }
    process_backup_policy_retention_prune(pool, config).await
}

#[derive(Default)]
struct ArtifactCleanupRun {
    jobs: i64,
    deleted_rows: i64,
    deleted_bytes: i64,
    tombstoned_rows: i64,
    tombstoned_bytes: i64,
    skipped_rows: i64,
}

struct ArtifactCleanupJob {
    id: Uuid,
    created_by: Option<Uuid>,
    metadata: Value,
}

struct ArtifactCleanupCandidate {
    id: Uuid,
    domain: String,
    object_key: String,
    size_bytes: i64,
    status: String,
    backup_artifact_id: Option<Uuid>,
    identity_matches_review: bool,
}

async fn process_artifact_cleanup_jobs_if_leader(
    pool: &PgPool,
    object_stores: ArtifactObjectStores<'_>,
    worker_id: &str,
    lease_secs: i32,
) -> Result<ArtifactCleanupRun> {
    if !acquire_worker_lease(pool, "artifact_cleanup_jobs", worker_id, lease_secs).await? {
        debug!(
            worker_id,
            "skipped artifact cleanup jobs because another worker holds the lease"
        );
        return Ok(ArtifactCleanupRun::default());
    }
    process_artifact_cleanup_jobs(pool, object_stores).await
}

async fn process_artifact_cleanup_jobs(
    pool: &PgPool,
    object_stores: ArtifactObjectStores<'_>,
) -> Result<ArtifactCleanupRun> {
    let expired = expire_stale_artifact_cleanup_jobs(pool).await?;
    let Some(job) = claim_artifact_cleanup_job(pool).await? else {
        return Ok(ArtifactCleanupRun {
            jobs: expired,
            ..ArtifactCleanupRun::default()
        });
    };
    for required_scope in artifact_cleanup_job_required_scopes(&job.metadata)? {
        if !actor_authorized(pool, job.created_by, "operator", &[required_scope]).await? {
            mark_artifact_cleanup_job_failed(pool, job.id, "actor_authority_revoked").await?;
            return Ok(ArtifactCleanupRun {
                jobs: expired + 1,
                ..ArtifactCleanupRun::default()
            });
        }
    }
    let result = run_artifact_cleanup_job(pool, object_stores, &job).await;
    match result {
        Ok(run) => {
            sqlx::query(
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
                    error = NULL
                WHERE id = $1
                "#,
            )
            .bind(job.id)
            .bind(SERVER_JOB_STATUS_COMPLETED)
            .bind(run.deleted_rows)
            .bind(run.deleted_bytes)
            .bind(run.tombstoned_rows)
            .bind(run.tombstoned_bytes)
            .bind(run.skipped_rows)
            .execute(pool)
            .await?;
            Ok(ArtifactCleanupRun {
                jobs: expired + 1,
                ..run
            })
        }
        Err(error) => {
            sqlx::query(
                r#"
                UPDATE server_jobs
                SET
                    status = $2,
                    error = $3,
                    completed_at = now()
                WHERE id = $1
                "#,
            )
            .bind(job.id)
            .bind(SERVER_JOB_STATUS_FAILED)
            .bind(error.to_string())
            .execute(pool)
            .await?;
            Err(error)
        }
    }
}

async fn expire_stale_artifact_cleanup_jobs(pool: &PgPool) -> Result<i64> {
    let result = sqlx::query(
        r#"
        UPDATE server_jobs
        SET
            status = $3,
            error = 'artifact_cleanup_running_timeout',
            completed_at = now(),
            metadata = metadata || jsonb_build_object(
                'running_timeout_secs', $4::bigint
            )
        WHERE job_type = $1
          AND status = $2
          AND started_at IS NOT NULL
          AND started_at <= now() - ($4::bigint * interval '1 second')
        "#,
    )
    .bind(SERVER_JOB_TYPE_ARTIFACT_CLEANUP)
    .bind(SERVER_JOB_STATUS_RUNNING)
    .bind(SERVER_JOB_STATUS_FAILED)
    .bind(ARTIFACT_CLEANUP_RUNNING_TIMEOUT_SECS)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() as i64)
}

async fn mark_artifact_cleanup_job_failed(pool: &PgPool, job_id: Uuid, error: &str) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE server_jobs
        SET
            status = $2,
            error = $3,
            completed_at = now()
        WHERE id = $1
        "#,
    )
    .bind(job_id)
    .bind(SERVER_JOB_STATUS_FAILED)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

async fn claim_artifact_cleanup_job(pool: &PgPool) -> Result<Option<ArtifactCleanupJob>> {
    let row = sqlx::query(
        r#"
        WITH claimed AS (
            SELECT id
            FROM server_jobs
            WHERE job_type = $1
              AND status = $2
            ORDER BY created_at ASC, id ASC
            LIMIT 1
            FOR UPDATE SKIP LOCKED
        )
        UPDATE server_jobs job
        SET status = $3, started_at = now()
        FROM claimed
        WHERE job.id = claimed.id
        RETURNING job.id, job.created_by, job.metadata
        "#,
    )
    .bind(SERVER_JOB_TYPE_ARTIFACT_CLEANUP)
    .bind(SERVER_JOB_STATUS_QUEUED)
    .bind(SERVER_JOB_STATUS_RUNNING)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        Ok(ArtifactCleanupJob {
            id: row.try_get("id")?,
            created_by: row.try_get("created_by")?,
            metadata: row.try_get("metadata")?,
        })
    })
    .transpose()
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
    object_stores: ArtifactObjectStores<'_>,
    job: &ArtifactCleanupJob,
) -> Result<ArtifactCleanupRun> {
    let candidates = artifact_cleanup_targets(pool, job.id).await?;
    let mut run = ArtifactCleanupRun::default();
    for candidate in &candidates {
        if !candidate.identity_matches_review
            || !matches!(
                candidate.status.as_str(),
                "creating" | "active" | "deleting" | "delete_failed"
            )
        {
            run.skipped_rows += 1;
            persist_artifact_cleanup_progress(pool, job.id, &run).await?;
            continue;
        }
        match apply_artifact_cleanup_candidate(pool, object_stores, candidate).await? {
            ArtifactCleanupDisposition::Deleted => {
                run.deleted_rows += 1;
                run.deleted_bytes += candidate.size_bytes;
            }
            ArtifactCleanupDisposition::Tombstoned => {
                run.tombstoned_rows += 1;
                run.tombstoned_bytes += candidate.size_bytes;
            }
        }
        persist_artifact_cleanup_progress(pool, job.id, &run).await?;
    }
    Ok(run)
}

async fn persist_artifact_cleanup_progress(
    pool: &PgPool,
    job_id: Uuid,
    run: &ArtifactCleanupRun,
) -> Result<()> {
    sqlx::query(
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
        "#,
    )
    .bind(job_id)
    .bind(run.deleted_rows)
    .bind(run.deleted_bytes)
    .bind(run.tombstoned_rows)
    .bind(run.tombstoned_bytes)
    .bind(run.skipped_rows)
    .execute(pool)
    .await?;
    Ok(())
}

enum ArtifactCleanupDisposition {
    Deleted,
    Tombstoned,
}

async fn apply_artifact_cleanup_candidate(
    pool: &PgPool,
    object_stores: ArtifactObjectStores<'_>,
    candidate: &ArtifactCleanupCandidate,
) -> Result<ArtifactCleanupDisposition> {
    match candidate.domain.as_str() {
        "job_output" => delete_job_output_artifact(pool, object_stores.backup, candidate).await,
        "file_transfer_handoff" => {
            delete_unreferenced_server_artifact(pool, object_stores.backup, candidate).await
        }
        "file_transfer_source" => {
            delete_file_transfer_source_artifact(pool, object_stores.backup, candidate).await
        }
        "backup_artifact" => {
            if backup_artifact_is_referenced(
                pool,
                candidate.backup_artifact_id,
                &candidate.object_key,
            )
            .await?
            {
                tombstone_server_artifact(pool, candidate.id).await
            } else {
                delete_backup_artifact(pool, object_stores.backup, candidate).await
            }
        }
        _ => tombstone_server_artifact(pool, candidate.id).await,
    }
}

async fn delete_job_output_artifact(
    pool: &PgPool,
    object_store: &BackupObjectStore,
    candidate: &ArtifactCleanupCandidate,
) -> Result<ArtifactCleanupDisposition> {
    let mut tx = pool.begin().await?;
    if !mark_server_artifact_deleting(&mut tx, candidate.id).await? {
        tx.rollback().await?;
        return Ok(ArtifactCleanupDisposition::Tombstoned);
    }
    tx.commit().await?;
    if let Err(error) = delete_object_key_confirmed(object_store, &candidate.object_key).await {
        mark_server_artifact_delete_failed(pool, candidate.id, &error.to_string()).await?;
        return Err(error);
    }
    let mut tx = pool.begin().await?;
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
    mark_server_artifact_deleted(&mut tx, candidate.id).await?;
    tx.commit().await?;
    Ok(ArtifactCleanupDisposition::Deleted)
}

async fn delete_unreferenced_server_artifact(
    pool: &PgPool,
    object_store: &BackupObjectStore,
    candidate: &ArtifactCleanupCandidate,
) -> Result<ArtifactCleanupDisposition> {
    let mut tx = pool.begin().await?;
    if !mark_server_artifact_deleting(&mut tx, candidate.id).await? {
        tx.rollback().await?;
        return Ok(ArtifactCleanupDisposition::Tombstoned);
    }
    tx.commit().await?;
    if let Err(error) = delete_object_key_confirmed(object_store, &candidate.object_key).await {
        mark_server_artifact_delete_failed(pool, candidate.id, &error.to_string()).await?;
        return Err(error);
    }
    let mut tx = pool.begin().await?;
    mark_server_artifact_deleted(&mut tx, candidate.id).await?;
    tx.commit().await?;
    Ok(ArtifactCleanupDisposition::Deleted)
}

async fn delete_file_transfer_source_artifact(
    pool: &PgPool,
    object_store: &BackupObjectStore,
    candidate: &ArtifactCleanupCandidate,
) -> Result<ArtifactCleanupDisposition> {
    let mut tx = pool.begin().await?;
    if !mark_server_artifact_deleting(&mut tx, candidate.id).await? {
        tx.rollback().await?;
        return Ok(ArtifactCleanupDisposition::Tombstoned);
    }
    tx.commit().await?;
    if let Err(error) = delete_object_key_confirmed(object_store, &candidate.object_key).await {
        mark_server_artifact_delete_failed(pool, candidate.id, &error.to_string()).await?;
        return Err(error);
    }
    let mut tx = pool.begin().await?;
    sqlx::query(
        r#"
        DELETE FROM file_transfer_source_artifacts
        WHERE object_key = $1
        "#,
    )
    .bind(&candidate.object_key)
    .execute(&mut *tx)
    .await?;
    mark_server_artifact_deleted(&mut tx, candidate.id).await?;
    tx.commit().await?;
    Ok(ArtifactCleanupDisposition::Deleted)
}

async fn delete_backup_artifact(
    pool: &PgPool,
    object_store: &BackupObjectStore,
    candidate: &ArtifactCleanupCandidate,
) -> Result<ArtifactCleanupDisposition> {
    let mut tx = pool.begin().await?;
    if !mark_server_artifact_deleting(&mut tx, candidate.id).await? {
        tx.rollback().await?;
        return Ok(ArtifactCleanupDisposition::Tombstoned);
    }
    tx.commit().await?;
    if let Err(error) = delete_object_key_confirmed(object_store, &candidate.object_key).await {
        mark_server_artifact_delete_failed(pool, candidate.id, &error.to_string()).await?;
        return Err(error);
    }
    let mut tx = pool.begin().await?;
    if let Some(backup_artifact_id) = candidate.backup_artifact_id {
        sqlx::query(
            r#"
            DELETE FROM backup_artifacts
            WHERE id = $1
            "#,
        )
        .bind(backup_artifact_id)
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query(
            r#"
            DELETE FROM backup_artifacts
            WHERE object_key = $1
            "#,
        )
        .bind(&candidate.object_key)
        .execute(&mut *tx)
        .await?;
    }
    mark_server_artifact_deleted(&mut tx, candidate.id).await?;
    tx.commit().await?;
    Ok(ArtifactCleanupDisposition::Deleted)
}

async fn mark_server_artifact_deleting(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    artifact_id: Uuid,
) -> Result<bool> {
    let result = sqlx::query(
        r#"
        UPDATE server_artifacts
        SET status = 'deleting',
            metadata = metadata - 'delete_error' - 'delete_failed_at'
        WHERE id = $1
          AND status IN ('creating', 'active', 'deleting', 'delete_failed')
        "#,
    )
    .bind(artifact_id)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected() > 0)
}

async fn mark_server_artifact_deleted(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    artifact_id: Uuid,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE server_artifacts
        SET status = 'deleted', deleted_at = now()
        WHERE id = $1
          AND status IN ('creating', 'active', 'deleting', 'delete_failed')
        "#,
    )
    .bind(artifact_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn mark_server_artifact_delete_failed(
    pool: &PgPool,
    artifact_id: Uuid,
    error: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE server_artifacts
        SET status = 'delete_failed',
            metadata = metadata || jsonb_build_object(
                'delete_error', left($2, 1000),
                'delete_failed_at', now()::text
            )
        WHERE id = $1
          AND status IN ('creating', 'active', 'deleting', 'delete_failed')
        "#,
    )
    .bind(artifact_id)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

async fn tombstone_server_artifact(
    pool: &PgPool,
    artifact_id: Uuid,
) -> Result<ArtifactCleanupDisposition> {
    sqlx::query(
        r#"
        UPDATE server_artifacts
        SET status = 'tombstoned', tombstoned_at = now()
        WHERE id = $1
          AND status IN ('creating', 'active', 'deleting', 'delete_failed')
        "#,
    )
    .bind(artifact_id)
    .execute(pool)
    .await?;
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
    let rows = sqlx::query(
        r#"
        SELECT
            target.artifact_id AS id,
            COALESCE(artifact.domain, target.domain) AS domain,
            COALESCE(artifact.object_key, target.object_key) AS object_key,
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
        ORDER BY target.created_at ASC, target.artifact_id ASC
        LIMIT 10000
        "#,
    )
    .bind(server_job_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ArtifactCleanupCandidate {
                id: row.try_get("id")?,
                domain: row.try_get("domain")?,
                object_key: row.try_get("object_key")?,
                size_bytes: row.try_get("size_bytes")?,
                status: row.try_get("status")?,
                backup_artifact_id: row.try_get("backup_artifact_id")?,
                identity_matches_review: row.try_get("identity_matches_review")?,
            })
        })
        .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
        .map_err(Into::into)
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

async fn process_due_schedules_if_leader(
    pool: &PgPool,
    limit: i64,
    worker_id: &str,
    lease_secs: i32,
    dispatch_config: &ScheduleDispatchConfig,
) -> Result<usize> {
    let acquired = acquire_worker_lease(pool, "schedules", worker_id, lease_secs).await?;
    if !acquired {
        debug!(
            worker_id,
            "skipped due schedules because another worker holds the lease"
        );
        return Ok(0);
    }
    process_due_schedules(pool, limit, dispatch_config).await
}

async fn process_due_schedules(
    pool: &PgPool,
    limit: i64,
    dispatch_config: &ScheduleDispatchConfig,
) -> Result<usize> {
    ensure_event_partitions(pool).await?;
    let mut tx = pool.begin().await?;
    let due_count: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*)
        FROM schedules
        WHERE enabled = TRUE
          AND deleted_at IS NULL
          AND next_run_at <= now()
          AND (deferred_until IS NULL OR deferred_until <= now())
        "#,
    )
    .fetch_one(&mut *tx)
    .await?;
    let rows = sqlx::query(
        r#"
        SELECT id
        FROM schedules
        WHERE enabled = TRUE
          AND deleted_at IS NULL
          AND next_run_at <= now()
          AND (deferred_until IS NULL OR deferred_until <= now())
        ORDER BY next_run_at, id
        LIMIT $1
        FOR UPDATE SKIP LOCKED
        "#,
    )
    .bind(limit.clamp(1, 100))
    .fetch_all(&mut *tx)
    .await?;
    if due_count > 0 || !rows.is_empty() {
        info!(due_count, selected = rows.len(), "scanned due schedules");
    }
    let schedule_ids = rows
        .into_iter()
        .map(|row| row.try_get("id").map_err(Into::into))
        .collect::<Result<Vec<Uuid>>>()?;
    tx.commit().await?;

    let mut materialized = 0_usize;
    for schedule_id in schedule_ids {
        materialized += process_due_schedule(pool, schedule_id, dispatch_config).await?;
    }
    Ok(materialized)
}

async fn process_due_schedule(
    pool: &PgPool,
    schedule_id: Uuid,
    dispatch_config: &ScheduleDispatchConfig,
) -> Result<usize> {
    let result: Result<usize> = async {
        let mut tx = pool.begin().await?;
        let Some(row) = sqlx::query(
            r#"
        SELECT
            id,
            actor_id,
            name,
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
        let schedule = DueSchedule {
            id: row.try_get("id")?,
            actor_id: row.try_get("actor_id")?,
            name: row.try_get("name")?,
            operation: row.try_get::<SqlJson<JobCommand>, _>("operation")?.0,
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
            record_schedule_failure(pool, schedule_id, &error.to_string()).await?;
            Ok(0)
        }
    }
}

struct DueSchedule {
    id: Uuid,
    actor_id: Option<Uuid>,
    name: String,
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
    let job_id = Uuid::new_v4();
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
    let schedule_target_skips = unavailable_skips
        .iter()
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
    }))?);
    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, actor_id, command_type, privileged, status, target_count,
            payload_hash, operation, source_schedule_id, request_fingerprint,
            max_timeout_secs, completed_at
        )
        VALUES ($1, $2, $3, TRUE, $4, $5, $6, $7, $8, $9, $10,
            CASE WHEN $11 THEN now() ELSE NULL END)
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
                completed_at
            )
            VALUES (
                $1,
                $2,
                $3,
                $4,
                $5,
                CASE WHEN $3 = 'skipped' THEN now() ELSE NULL END,
                CASE WHEN $3 = 'skipped' THEN now() ELSE NULL END
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
        .execute(&mut **tx)
        .await?;
    }

    record_schedule_capability_skip_outputs(tx, job_id, &operation, &capability_skips).await?;
    record_schedule_target_skip_outputs(tx, job_id, &operation, &schedule_target_skips).await?;

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
        "operation_type": operation_type,
        "job_id": job_id,
        "fixed_targets": &targets,
        "materialized_targets": &materialized_targets,
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

    sqlx::query(
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
        "#,
    )
    .bind(schedule.id)
    .bind(job_id)
    .bind(status)
    .bind(job_completed_immediately)
    .execute(&mut **tx)
    .await?;

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
        "#,
    )
    .bind(targets.to_vec())
    .fetch_all(&mut **tx)
    .await?;
    let mut present_targets = HashSet::with_capacity(rows.len());
    let mut capabilities = Vec::new();
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
            "exit_code": 0,
            "accepted": false,
            "message": skip.failure.message,
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
            "exit_code": 0,
            "accepted": skip.accepted,
            "message": skip.message,
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
    insert_webhook_event_in_tx(
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
                "target_count": input.targets.len(),
            },
        }),
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
    insert_webhook_event_in_tx(
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
                "last_job_id": job_id,
                "last_job_status": job_status,
                "last_job_error": null,
            },
            "job": {
                "id": job_id,
                "status": job_status,
                "type": command_type,
                "source_schedule_id": schedule.id,
                "target_count": targets.len(),
                "target_ids": targets,
            },
        }),
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
    sqlx::query(
        r#"
        UPDATE schedules
        SET
            last_run_at = now(),
            next_run_at = to_timestamp($2),
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(schedule.id)
    .bind(next_run_at.timestamp() as f64)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn record_schedule_failure(pool: &PgPool, schedule_id: Uuid, error: &str) -> Result<()> {
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
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(());
    };
    let actor_id: Option<Uuid> = row.try_get("actor_id")?;
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
        "schedule_id": schedule_id,
        "schedule_name": &schedule_name,
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
        VALUES ($1, $2, $3, $4, NULL, $5)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(schedule.actor_id)
    .bind("schedule.disabled_actor_authority_revoked")
    .bind(format!("schedule:{}", schedule.id))
    .bind(serde_json::json!({
        "worker": "schedule_dispatch_worker",
        "schedule_id": schedule.id,
        "schedule_name": &schedule.name,
        "reason": "actor_authority_revoked",
    }))
    .execute(&mut **tx)
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
mod schedule_tests {
    use super::*;
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use std::{path::Path, str::FromStr};

    fn schedule_with_policy(policy: &str, limit: i32) -> DueSchedule {
        DueSchedule {
            id: Uuid::nil(),
            actor_id: None,
            name: "test".to_string(),
            operation: JobCommand::Shell {
                argv: vec!["/bin/true".to_string()],
                pty: false,
            },
            selector_expression: "tag:edge".to_string(),
            target_client_ids: vec!["edge-a".to_string()],
            cron_expr: "* * * * *".to_string(),
            next_run_at_unix: 1_800_000_000,
            catch_up_policy: policy.to_string(),
            catch_up_limit: limit,
            retry_delay_secs: 300,
            max_failures: 3,
            failure_count: 0,
            last_error: None,
        }
    }

    fn scheduled_speed_test_operation() -> serde_json::Value {
        let plan = plan_tunnel(&TunnelPlanInput {
            name: "left-a-right-b".to_string(),
            interface_name: "tunab".to_string(),
            kind: TunnelKind::Gre,
            runtime_control: Default::default(),
            runtime_topology: Default::default(),
            left_client_id: "left-a".to_string(),
            right_client_id: "right-b".to_string(),
            left_underlay: "198.51.100.10".to_string(),
            right_underlay: "203.0.113.20".to_string(),
            address_pool_cidr: "10.255.0.0/30".to_string(),
            reserved_addresses: Vec::new(),
            ipv4_tunnel: Some(TunnelAddressPair {
                left: "10.255.0.0".to_string(),
                right: "10.255.0.1".to_string(),
                prefix_len: 31,
            }),
            ipv6_address_pool_cidr: None,
            ipv6_tunnel: None,
            latency_primary_family: Default::default(),
            bandwidth: BandwidthTier::M100,
            latency_ms: 18.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            ospf_policy: OspfCostPolicy::default(),
        })
        .unwrap();
        serde_json::to_value(JobCommand::NetworkSpeedTest {
            plan: Box::new(plan),
            server_side: TunnelEndpointSide::Left,
            duration_secs: 3,
            max_bytes: 16 * 1024 * 1024,
            rate_limit_kbps: 100_000,
            port: 5201,
            connect_timeout_ms: 5000,
        })
        .unwrap()
    }

    #[test]
    fn schedule_catch_up_run_count_is_bounded() {
        assert_eq!(
            catch_up_run_count(&schedule_with_policy("skip_missed", 1), 50),
            1
        );
        assert_eq!(
            catch_up_run_count(&schedule_with_policy("run_once", 1), 50),
            1
        );
        assert_eq!(
            catch_up_run_count(&schedule_with_policy("run_all_limited", 4), 50),
            4
        );
        assert_eq!(
            catch_up_run_count(&schedule_with_policy("run_all_limited", 30), 50),
            25
        );
        assert_eq!(
            catch_up_run_count(&schedule_with_policy("run_all_limited", 4), 0),
            1
        );
    }

    #[test]
    fn schedule_cron_catch_up_counts_missed_runs() {
        let schedule = schedule_with_policy("run_all_limited", 4);
        let now = date_time_from_unix(schedule.next_run_at_unix + 180).unwrap();
        assert_eq!(calculate_due_occurrences(&schedule, now).unwrap(), 4);
    }

    #[test]
    fn schedule_cron_next_run_advances_from_policy_cursor() {
        let mut schedule = schedule_with_policy("run_once", 1);
        let now = date_time_from_unix(schedule.next_run_at_unix + 3600).unwrap();
        assert_eq!(
            next_run_after_success(&schedule, 1, now)
                .unwrap()
                .timestamp(),
            schedule.next_run_at_unix + 60
        );

        schedule.catch_up_policy = "skip_missed".to_string();
        assert!(next_run_after_success(&schedule, 1, now).unwrap() > now);
    }

    #[test]
    fn schedule_error_is_bounded() {
        let error = "x".repeat(1200);
        assert_eq!(truncate_schedule_error(&error).len(), 1024);
    }

    #[test]
    fn schedule_max_timeout_uses_configured_value_without_agent_cap_clamp() {
        let targets = vec!["edge-a".to_string(), "edge-b".to_string()];
        let capabilities = vec![
            TargetCapability {
                client_id: "edge-a".to_string(),
                arch: Some("x86_64".to_string()),
                capabilities: AgentCapabilitySnapshot {
                    max_job_timeout_secs: 20,
                    ..AgentCapabilitySnapshot::default()
                },
            },
            TargetCapability {
                client_id: "edge-b".to_string(),
                arch: Some("aarch64".to_string()),
                capabilities: AgentCapabilitySnapshot {
                    max_job_timeout_secs: 120,
                    ..AgentCapabilitySnapshot::default()
                },
            },
        ];

        assert_eq!(
            effective_schedule_max_timeout_secs(
                90,
                DEFAULT_MAX_JOB_TIMEOUT_SECS,
                &targets,
                &capabilities
            ),
            90
        );
        assert_eq!(
            effective_schedule_max_timeout_secs(
                10,
                DEFAULT_MAX_JOB_TIMEOUT_SECS,
                &targets,
                &capabilities
            ),
            10
        );
        assert_eq!(
            effective_schedule_max_timeout_secs(90, DEFAULT_MAX_JOB_TIMEOUT_SECS, &[], &[]),
            90
        );
        assert_eq!(
            effective_schedule_max_timeout_secs(7_200, 7_200, &[], &[]),
            7_200
        );
    }

    #[test]
    fn schedule_selector_expression_matches_clients() {
        let expression = parse_expression("provider:alpha && (country:US || id:edge-b)")
            .unwrap()
            .unwrap();
        let context = ExpressionContext::for_vps(VpsMetadata {
            id: "edge-a".to_string(),
            display_name: "edge-a".to_string(),
            status: "online".to_string(),
            tags: vec!["provider:alpha".to_string(), "country:US".to_string()],
            ..VpsMetadata::default()
        });
        assert!(expression_matches(&context, &expression));
    }

    #[tokio::test]
    async fn postgres_due_schedule_skips_unavailable_fixed_targets() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        insert_worker_client(&db.pool, "edge-a", "online", false).await;
        insert_worker_client(&db.pool, "edge-b", "deleted", true).await;
        insert_worker_client(&db.pool, "edge-c", "online", false).await;
        let schedule_id = insert_worker_schedule(
            &db.pool,
            "missing-target-schedule",
            serde_json::json!({"type": "shell", "argv": ["/bin/true"], "pty": false}),
            &["edge-a", "edge-b", "edge-c"],
        )
        .await;

        let processed = process_due_schedule(
            &db.pool,
            schedule_id,
            &ScheduleDispatchConfig::new(60, DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
        )
        .await
        .unwrap();

        assert_eq!(processed, 1);
        let (job_id, status, failure_count, last_error) =
            schedule_result(&db.pool, schedule_id).await;
        assert_eq!(status.as_deref(), Some(JOB_STATUS_QUEUED));
        assert_eq!(failure_count, 0);
        assert_eq!(last_error, None);
        let targets = job_targets(&db.pool, job_id).await;
        assert_eq!(
            targets,
            vec![
                ("edge-a".to_string(), TARGET_STATUS_QUEUED.to_string(), None),
                (
                    "edge-b".to_string(),
                    TARGET_STATUS_SKIPPED.to_string(),
                    Some("fixed_target_unavailable: schedule target skipped".to_string())
                ),
                ("edge-c".to_string(), TARGET_STATUS_QUEUED.to_string(), None),
            ]
        );
        let output = job_status_output(&db.pool, job_id, "edge-b").await;
        assert_eq!(output["type"], "schedule_target_skipped");
        assert_eq!(output["reason"], "fixed_target_unavailable");
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_due_schedule_skips_never_connected_fixed_targets() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        insert_worker_client(&db.pool, "edge-a", "online", false).await;
        insert_worker_client_with_incarnation(&db.pool, "edge-b", "never", false, None).await;
        let schedule_id = insert_worker_schedule(
            &db.pool,
            "never-connected-schedule",
            serde_json::json!({"type": "shell", "argv": ["/bin/true"], "pty": false}),
            &["edge-a", "edge-b"],
        )
        .await;

        let processed = process_due_schedule(
            &db.pool,
            schedule_id,
            &ScheduleDispatchConfig::new(60, DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
        )
        .await
        .unwrap();

        assert_eq!(processed, 1);
        let (job_id, status, failure_count, last_error) =
            schedule_result(&db.pool, schedule_id).await;
        assert_eq!(status.as_deref(), Some(JOB_STATUS_QUEUED));
        assert_eq!(failure_count, 0);
        assert_eq!(last_error, None);
        let targets = job_targets(&db.pool, job_id).await;
        assert_eq!(
            targets,
            vec![
                ("edge-a".to_string(), TARGET_STATUS_QUEUED.to_string(), None),
                (
                    "edge-b".to_string(),
                    TARGET_STATUS_SKIPPED.to_string(),
                    Some("target_never_connected: schedule target skipped".to_string())
                ),
            ]
        );
        let output = job_status_output(&db.pool, job_id, "edge-b").await;
        assert_eq!(output["type"], "schedule_target_skipped");
        assert_eq!(output["reason"], "target_never_connected");
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_due_schedule_speed_test_skips_both_endpoints_when_peer_is_unavailable() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        insert_worker_client(&db.pool, "left-a", "online", false).await;
        insert_worker_client_with_incarnation(&db.pool, "right-b", "never", false, None).await;
        let schedule_id = insert_worker_schedule(
            &db.pool,
            "speed-test-peer-unavailable-schedule",
            scheduled_speed_test_operation(),
            &["left-a", "right-b"],
        )
        .await;

        let processed = process_due_schedule(
            &db.pool,
            schedule_id,
            &ScheduleDispatchConfig::new(60, DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
        )
        .await
        .unwrap();

        assert_eq!(processed, 1);
        let (job_id, status, failure_count, last_error) =
            schedule_result(&db.pool, schedule_id).await;
        assert_eq!(status.as_deref(), Some(JOB_STATUS_SKIPPED));
        assert_eq!(failure_count, 0);
        assert_eq!(last_error, None);
        let targets = job_targets(&db.pool, job_id).await;
        assert_eq!(
            targets,
            vec![
                (
                    "left-a".to_string(),
                    TARGET_STATUS_SKIPPED.to_string(),
                    Some("network_speed_test_peer_unavailable: peer target was skipped; speed test requires both endpoints".to_string())
                ),
                (
                    "right-b".to_string(),
                    TARGET_STATUS_SKIPPED.to_string(),
                    Some("target_never_connected: schedule target skipped".to_string())
                ),
            ]
        );
        let left_output = job_status_output(&db.pool, job_id, "left-a").await;
        assert_eq!(left_output["type"], "network_speed_test_peer_unavailable");
        assert_eq!(left_output["reason"], "network_speed_test_peer_unavailable");
        let right_output = job_status_output(&db.pool, job_id, "right-b").await;
        assert_eq!(right_output["type"], "schedule_target_skipped");
        assert_eq!(right_output["reason"], "target_never_connected");
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_due_schedule_records_missing_fixed_targets_as_skipped_rows() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        insert_worker_client(&db.pool, "edge-a", "online", false).await;
        let schedule_id = insert_worker_schedule(
            &db.pool,
            "missing-fixed-target-schedule",
            serde_json::json!({"type": "shell", "argv": ["/bin/true"], "pty": false}),
            &["edge-a", "edge-missing"],
        )
        .await;

        let processed = process_due_schedule(
            &db.pool,
            schedule_id,
            &ScheduleDispatchConfig::new(60, DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
        )
        .await
        .unwrap();

        assert_eq!(processed, 1);
        let (job_id, status, failure_count, last_error) =
            schedule_result(&db.pool, schedule_id).await;
        assert_eq!(status.as_deref(), Some(JOB_STATUS_QUEUED));
        assert_eq!(failure_count, 0);
        assert_eq!(last_error, None);
        let targets = job_targets(&db.pool, job_id).await;
        assert_eq!(
            targets,
            vec![
                ("edge-a".to_string(), TARGET_STATUS_QUEUED.to_string(), None),
                (
                    "edge-missing".to_string(),
                    TARGET_STATUS_SKIPPED.to_string(),
                    Some("fixed_target_missing: schedule target skipped".to_string())
                ),
            ]
        );
        let output = job_status_output(&db.pool, job_id, "edge-missing").await;
        assert_eq!(output["type"], "schedule_target_skipped");
        assert_eq!(output["reason"], "fixed_target_missing");
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_due_schedule_materializes_canonical_command_hash_and_operation() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        insert_worker_client(&db.pool, "edge-a", "online", false).await;
        let schedule_id = insert_worker_schedule(
            &db.pool,
            "canonical-scheduled-shell",
            serde_json::json!({
                "pty": false,
                "argv": ["/bin/sh", "-c", "printf scheduled"],
                "type": "shell",
            }),
            &["edge-a"],
        )
        .await;

        let processed = process_due_schedule(
            &db.pool,
            schedule_id,
            &ScheduleDispatchConfig::new(60, DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
        )
        .await
        .unwrap();

        assert_eq!(processed, 1);
        let (job_id, status, failure_count, last_error) =
            schedule_result(&db.pool, schedule_id).await;
        assert_eq!(status.as_deref(), Some(JOB_STATUS_QUEUED));
        assert_eq!(failure_count, 0);
        assert_eq!(last_error, None);
        let expected_operation = JobCommand::Shell {
            argv: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf scheduled".to_string(),
            ],
            pty: false,
        };
        let expected_payload_hash = payload_hash(&encode_json(&expected_operation).unwrap());
        let row = sqlx::query(
            r#"
            SELECT payload_hash, operation
            FROM jobs
            WHERE id = $1
            "#,
        )
        .bind(job_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let stored_payload_hash: String = row.try_get("payload_hash").unwrap();
        let stored_operation: SqlJson<JobCommand> = row.try_get("operation").unwrap();
        assert_eq!(stored_payload_hash, expected_payload_hash);
        assert_eq!(
            encode_json(&stored_operation.0).unwrap(),
            encode_json(&expected_operation).unwrap()
        );
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_due_schedule_with_all_unavailable_targets_is_skipped() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        insert_worker_client(&db.pool, "edge-a", "deleted", true).await;
        insert_worker_client(&db.pool, "edge-b", "revoked", false).await;
        let schedule_id = insert_worker_schedule(
            &db.pool,
            "all-unavailable-schedule",
            serde_json::json!({"type": "shell", "argv": ["/bin/true"], "pty": false}),
            &["edge-a", "edge-b"],
        )
        .await;

        let processed = process_due_schedule(
            &db.pool,
            schedule_id,
            &ScheduleDispatchConfig::new(60, DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
        )
        .await
        .unwrap();

        assert_eq!(processed, 1);
        let (job_id, status, failure_count, last_error) =
            schedule_result(&db.pool, schedule_id).await;
        assert_eq!(status.as_deref(), Some(JOB_STATUS_SKIPPED));
        assert_eq!(failure_count, 0);
        assert_eq!(last_error, None);
        let targets = job_targets(&db.pool, job_id).await;
        assert!(targets
            .iter()
            .all(|(_, status, _)| status == TARGET_STATUS_SKIPPED));
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_scheduled_update_skips_busy_targets() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        insert_worker_client(&db.pool, "edge-a", "online", false).await;
        insert_worker_client(&db.pool, "edge-b", "online", false).await;
        insert_active_worker_target(&db.pool, "edge-a").await;
        let schedule_id = insert_worker_schedule(
            &db.pool,
            "busy-update-schedule",
            serde_json::json!({
                "type": "agent_update",
                "artifact_url": "https://updates.example.invalid/agent",
                "sha256_hex": "a".repeat(64),
            }),
            &["edge-a", "edge-b"],
        )
        .await;

        let processed = process_due_schedule(
            &db.pool,
            schedule_id,
            &ScheduleDispatchConfig::new(60, DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
        )
        .await
        .unwrap();

        assert_eq!(processed, 1);
        let (job_id, status, failure_count, last_error) =
            schedule_result(&db.pool, schedule_id).await;
        assert_eq!(status.as_deref(), Some(JOB_STATUS_QUEUED));
        assert_eq!(failure_count, 0);
        assert_eq!(last_error, None);
        let targets = job_targets(&db.pool, job_id).await;
        assert_eq!(
            targets,
            vec![
                (
                    "edge-a".to_string(),
                    TARGET_STATUS_SKIPPED.to_string(),
                    Some(
                        "busy_agent_active_jobs: target has another active job; update skipped"
                            .to_string()
                    )
                ),
                ("edge-b".to_string(), TARGET_STATUS_QUEUED.to_string(), None),
            ]
        );
        let output = job_status_output(&db.pool, job_id, "edge-a").await;
        assert_eq!(output["type"], "busy_update_skipped");
        assert_eq!(output["reason"], "busy_agent_active_jobs");
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_scheduled_update_all_busy_targets_is_skipped() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        insert_worker_client(&db.pool, "edge-a", "online", false).await;
        insert_worker_client(&db.pool, "edge-b", "online", false).await;
        insert_active_worker_target(&db.pool, "edge-a").await;
        insert_active_worker_target(&db.pool, "edge-b").await;
        let schedule_id = insert_worker_schedule(
            &db.pool,
            "all-busy-update-schedule",
            serde_json::json!({
                "type": "agent_update_check",
            }),
            &["edge-a", "edge-b"],
        )
        .await;

        let processed = process_due_schedule(
            &db.pool,
            schedule_id,
            &ScheduleDispatchConfig::new(60, DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
        )
        .await
        .unwrap();

        assert_eq!(processed, 1);
        let (job_id, status, _, last_error) = schedule_result(&db.pool, schedule_id).await;
        assert_eq!(status.as_deref(), Some(JOB_STATUS_SKIPPED));
        assert_eq!(last_error, None);
        let targets = job_targets(&db.pool, job_id).await;
        assert!(targets
            .iter()
            .all(|(_, status, _)| status == TARGET_STATUS_SKIPPED));
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_strict_scheduled_update_policy_is_hash_bound() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let artifact_sha = "12".repeat(32);
        let rollback_sha = "34".repeat(32);
        insert_worker_agent_update_release(&db.pool, &artifact_sha, Some(&rollback_sha)).await;
        let mut tx = db.pool.begin().await.unwrap();
        let policy_targets = vec!["client-a".to_string()];
        let policy_capabilities = vec![TargetCapability {
            client_id: "client-a".to_string(),
            arch: Some("x86_64".to_string()),
            capabilities: AgentCapabilitySnapshot::default(),
        }];

        assert!(scheduled_agent_update_release_policy_allows(
            &mut tx,
            &JobCommand::UpdateAgent {
                artifact_url: "https://updates.example/agent".to_string(),
                sha256_hex: artifact_sha.clone(),
            },
            true,
            &policy_targets,
            &policy_capabilities,
        )
        .await
        .unwrap());
        assert!(scheduled_agent_update_release_policy_allows(
            &mut tx,
            &JobCommand::AgentUpdateActivate {
                staged_sha256_hex: artifact_sha.clone(),
                restart_agent: true,
            },
            true,
            &policy_targets,
            &policy_capabilities,
        )
        .await
        .unwrap());
        assert!(scheduled_agent_update_release_policy_allows(
            &mut tx,
            &JobCommand::AgentUpdateRollback {
                rollback_sha256_hex: Some(rollback_sha.clone()),
            },
            true,
            &policy_targets,
            &policy_capabilities,
        )
        .await
        .unwrap());
        assert!(scheduled_agent_update_release_policy_allows(
            &mut tx,
            &JobCommand::AgentUpdateCheck {
                version_url: Some(local_update_manifest_url(&artifact_sha)),
                activate: true,
                restart_agent: true,
            },
            true,
            &policy_targets,
            &policy_capabilities,
        )
        .await
        .unwrap());
        assert!(scheduled_agent_update_release_policy_allows(
            &mut tx,
            &JobCommand::AgentUpdateCheck {
                version_url: None,
                activate: true,
                restart_agent: true,
            },
            true,
            &policy_targets,
            &policy_capabilities,
        )
        .await
        .unwrap());
        assert!(!scheduled_agent_update_release_policy_allows(
            &mut tx,
            &JobCommand::AgentUpdateRollback {
                rollback_sha256_hex: None,
            },
            true,
            &policy_targets,
            &policy_capabilities,
        )
        .await
        .unwrap());
        assert!(!scheduled_agent_update_release_policy_allows(
            &mut tx,
            &JobCommand::AgentUpdateActivate {
                staged_sha256_hex: "56".repeat(32),
                restart_agent: true,
            },
            true,
            &policy_targets,
            &policy_capabilities,
        )
        .await
        .unwrap());

        tx.rollback().await.unwrap();
        db.cleanup().await;
    }

    fn local_update_manifest_url(artifact_sha256_hex: &str) -> String {
        let root =
            std::env::temp_dir().join(format!("vpsman-worker-update-manifest-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let asset_name = vpsman_common::agent_update_asset_name_for_arch("x86_64").unwrap();
        let sums_path = root.join("SHA256SUMS");
        std::fs::write(&sums_path, format!("{artifact_sha256_hex}  {asset_name}\n")).unwrap();
        let manifest_path = root.join("version.json");
        let manifest = serde_json::json!({
            "schema_version": 2,
            "project": "vpsman",
            "version": "99.0.0",
            "tag": "v99.0.0",
            "assets": [
                {
                    "name": asset_name,
                    "download_url": format!("https://updates.example/{asset_name}")
                }
            ],
            "checksum_manifest": {
                "name": "SHA256SUMS",
                "download_url": format!("file://{}", sums_path.display())
            }
        });
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        format!("file://{}", manifest_path.display())
    }

    #[test]
    fn worker_runtime_config_reloads_suite_file_from_base_args() {
        with_cleared_worker_env(WORKER_HOT_RELOAD_ENV, || {
            let path = temp_suite_config_path("worker-hot-reload");
            let object_dir = path.with_extension("objects");
            std::fs::write(
                &path,
                worker_runtime_toml(
                    7,
                    17,
                    333,
                    41,
                    true,
                    5,
                    45,
                    500,
                    9,
                    6,
                    11,
                    3,
                    300,
                    13,
                    true,
                    object_dir.to_string_lossy().as_ref(),
                ),
            )
            .unwrap();
            let args =
                Args::parse_from(["vpsman-worker", "--suite-config", path.to_str().unwrap()]);

            let runtime = load_worker_runtime_config(&args).unwrap();

            assert_eq!(runtime.tick_secs, 7);
            assert_eq!(runtime.worker_lease_secs, 17);
            assert_eq!(runtime.agent_offline_timeout_secs, 333);
            assert_eq!(runtime.schedule_dispatch_config.max_timeout_secs, 41);
            assert!(
                runtime
                    .schedule_dispatch_config
                    .require_registered_agent_updates
            );
            assert_eq!(runtime.alert_notification_config.delivery_limit, 5);
            assert_eq!(runtime.alert_notification_config.retention_days, 45);
            assert_eq!(runtime.alert_notification_config.retention_prune_limit, 500);
            assert_eq!(runtime.alert_notification_config.webhook_timeout_secs, 9);
            assert_eq!(runtime.webhook_rule_config.delivery_limit, 6);
            assert_eq!(runtime.webhook_rule_config.materialize_limit, 11);
            assert_eq!(runtime.webhook_rule_config.retention_days, 3);
            assert_eq!(runtime.webhook_rule_config.retention_prune_limit, 300);
            assert_eq!(runtime.webhook_rule_config.webhook_timeout_secs, 13);
            assert!(runtime.backup_policy_prune_config.enabled);
            assert_eq!(
                runtime
                    .backup_policy_prune_config
                    .object_store
                    .as_ref()
                    .map(BackupObjectStore::kind),
                Some("filesystem")
            );
            assert_eq!(runtime.backup_object_store.kind(), "filesystem");

            std::fs::write(
                &path,
                worker_runtime_toml(
                    19,
                    29,
                    444,
                    55,
                    false,
                    8,
                    60,
                    700,
                    12,
                    10,
                    14,
                    4,
                    400,
                    16,
                    false,
                    object_dir.to_string_lossy().as_ref(),
                ),
            )
            .unwrap();

            let runtime = load_worker_runtime_config(&args).unwrap();
            assert_eq!(runtime.tick_secs, 19);
            assert_eq!(runtime.worker_lease_secs, 29);
            assert_eq!(runtime.agent_offline_timeout_secs, 444);
            assert_eq!(runtime.schedule_dispatch_config.max_timeout_secs, 55);
            assert!(
                !runtime
                    .schedule_dispatch_config
                    .require_registered_agent_updates
            );
            assert_eq!(runtime.alert_notification_config.delivery_limit, 8);
            assert_eq!(runtime.webhook_rule_config.materialize_limit, 14);
            assert!(!runtime.backup_policy_prune_config.enabled);

            let _ = std::fs::remove_file(path);
        });
    }

    #[test]
    fn backup_policy_prune_store_flag_configures_retention_store() {
        with_cleared_worker_env(WORKER_HOT_RELOAD_ENV, || {
            let object_dir =
                temp_suite_config_path("worker-policy-prune-store").with_extension("objects");
            let args = Args::parse_from([
                "vpsman-worker",
                "--backup-policy-prune-enabled",
                "--backup-policy-prune-delete-objects",
                "--backup-policy-prune-object-store-dir",
                object_dir.to_str().unwrap(),
            ]);

            let runtime = WorkerRuntimeConfig::from_args(&args).unwrap();

            assert_eq!(
                runtime
                    .backup_policy_prune_config
                    .object_store
                    .as_ref()
                    .map(BackupObjectStore::kind),
                Some("filesystem")
            );
            assert_eq!(runtime.backup_object_store.kind(), "filesystem");
        });
    }

    #[test]
    fn suite_bool_defaults_do_not_disable_explicit_true_flags() {
        let env_name = "VPSMAN_WORKER_APPLY_BOOL_DEFAULT_TEST_UNSET";

        let mut explicit_true = true;
        apply_bool_default(&mut explicit_true, env_name, Some(false));
        assert!(explicit_true);

        let mut default_false = false;
        apply_bool_default(&mut default_false, env_name, Some(true));
        assert!(default_false);
    }

    struct PgWorkerTestDb {
        pool: PgPool,
        admin_pool: PgPool,
        db_name: String,
    }

    impl PgWorkerTestDb {
        async fn maybe_new() -> Option<Self> {
            let base_url = match std::env::var("VPSMAN_TEST_POSTGRES_URL") {
                Ok(value) if !value.trim().is_empty() => value,
                _ => {
                    eprintln!("skipping worker Postgres test: VPSMAN_TEST_POSTGRES_URL is unset");
                    return None;
                }
            };
            Some(
                Self::new(&base_url)
                    .await
                    .expect("failed to create worker test database"),
            )
        }

        async fn new(base_url: &str) -> anyhow::Result<Self> {
            let base_options = PgConnectOptions::from_str(base_url)?;
            let admin_pool = PgPoolOptions::new()
                .max_connections(1)
                .connect_with(base_options.clone().database("postgres"))
                .await?;
            let db_name = format!("vpsman_worker_{}", Uuid::new_v4().simple());
            sqlx::query(&format!("CREATE DATABASE {}", quote_ident(&db_name)))
                .execute(&admin_pool)
                .await?;
            let pool = PgPoolOptions::new()
                .max_connections(4)
                .connect_with(base_options.database(&db_name))
                .await?;
            let migrator = sqlx::migrate::Migrator::new(workspace_migrations_dir()).await?;
            migrator.run(&pool).await?;
            Ok(Self {
                pool,
                admin_pool,
                db_name,
            })
        }

        async fn cleanup(self) {
            let Self {
                pool,
                admin_pool,
                db_name,
            } = self;
            pool.close().await;
            let _ = sqlx::query(
                r#"
                SELECT pg_terminate_backend(pid)
                FROM pg_stat_activity
                WHERE datname = $1
                  AND pid <> pg_backend_pid()
                "#,
            )
            .bind(&db_name)
            .execute(&admin_pool)
            .await;
            let _ = sqlx::query(&format!(
                "DROP DATABASE IF EXISTS {}",
                quote_ident(&db_name)
            ))
            .execute(&admin_pool)
            .await;
            admin_pool.close().await;
        }
    }

    fn quote_ident(value: &str) -> String {
        format!("\"{}\"", value.replace('"', "\"\""))
    }

    fn workspace_migrations_dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("migrations")
    }

    async fn insert_worker_client(pool: &PgPool, client_id: &str, status: &str, hidden: bool) {
        insert_worker_client_with_incarnation(
            pool,
            client_id,
            status,
            hidden,
            Some(Uuid::new_v4()),
        )
        .await;
    }

    async fn insert_worker_client_with_incarnation(
        pool: &PgPool,
        client_id: &str,
        status: &str,
        hidden: bool,
        process_incarnation_id: Option<Uuid>,
    ) {
        sqlx::query(
            r#"
            INSERT INTO clients (
                id, display_name, public_key, status, internal_build_number,
                process_incarnation_id, capabilities, hidden_at
            )
            VALUES ($1, $1, decode('', 'hex'), $2, 1, $3, $4, CASE WHEN $5 THEN now() ELSE NULL END)
            "#,
        )
        .bind(client_id)
        .bind(status)
        .bind(process_incarnation_id)
        .bind(SqlJson(AgentCapabilitySnapshot::default()))
        .bind(hidden)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_worker_schedule(
        pool: &PgPool,
        name: &str,
        operation: serde_json::Value,
        targets: &[&str],
    ) -> Uuid {
        let actor_id = insert_worker_operator(
            pool,
            "active",
            "operator",
            &["jobs:write", "schedules:write"],
        )
        .await;
        let schedule_id = Uuid::new_v4();
        let target_client_ids = targets
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>();
        sqlx::query(
            r#"
            INSERT INTO schedules (
                id, actor_id, name, operation, selector_expression, target_client_ids,
                cron_expr, next_run_at, catch_up_policy, catch_up_limit
            )
            VALUES ($1, $2, $3, $4, 'id:*', $5, '* * * * *', now() - interval '60 seconds', 'skip_missed', 1)
            "#,
        )
        .bind(schedule_id)
        .bind(actor_id)
        .bind(name)
        .bind(SqlJson(operation))
        .bind(target_client_ids)
        .execute(pool)
        .await
        .unwrap();
        schedule_id
    }

    async fn insert_worker_agent_update_release(
        pool: &PgPool,
        artifact_sha256_hex: &str,
        rollback_artifact_sha256_hex: Option<&str>,
    ) {
        sqlx::query(
            r#"
            INSERT INTO agent_update_releases (
                id, name, version, channel, status, artifact_sha256_hex,
                artifact_url_sha256_hex, rollback_artifact_sha256_hex,
                rollback_artifact_url_sha256_hex
            )
            VALUES ($1, 'vpsman-agent', '9.9.9', 'stable', 'published_external', $2, $3, $4, $5)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(artifact_sha256_hex)
        .bind("aa".repeat(32))
        .bind(rollback_artifact_sha256_hex)
        .bind(rollback_artifact_sha256_hex.map(|_| "bb".repeat(32)))
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_worker_operator(
        pool: &PgPool,
        status: &str,
        role: &str,
        scopes: &[&str],
    ) -> Uuid {
        let operator_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO operators (id, username, password_hash, status, role, scopes)
            VALUES ($1, $2, 'test-password-hash', $3, $4, $5)
            "#,
        )
        .bind(operator_id)
        .bind(format!("worker-operator-{operator_id}"))
        .bind(status)
        .bind(role)
        .bind(serde_json::json!(scopes))
        .execute(pool)
        .await
        .unwrap();
        operator_id
    }

    async fn insert_active_worker_target(pool: &PgPool, client_id: &str) {
        let job_id = Uuid::new_v4();
        let operation = JobCommand::Shell {
            argv: vec!["sleep".to_string(), "60".to_string()],
            pty: false,
        };
        sqlx::query(
            r#"
            INSERT INTO jobs (
                id, command_type, privileged, status, target_count, payload_hash,
                operation, request_fingerprint, max_timeout_secs
            )
            VALUES ($1, 'shell', TRUE, 'running', 1, $2, $3, $4, 60)
            "#,
        )
        .bind(job_id)
        .bind(format!("hash-{job_id}"))
        .bind(SqlJson(operation))
        .bind(format!("fingerprint-{job_id}"))
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO job_targets (job_id, client_id, status, started_at)
            VALUES ($1, $2, 'running', now())
            "#,
        )
        .bind(job_id)
        .bind(client_id)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn schedule_result(
        pool: &PgPool,
        schedule_id: Uuid,
    ) -> (Uuid, Option<String>, i32, Option<String>) {
        let row = sqlx::query(
            r#"
            SELECT last_job_id, last_job_status, failure_count, last_error
            FROM schedules
            WHERE id = $1
            "#,
        )
        .bind(schedule_id)
        .fetch_one(pool)
        .await
        .unwrap();
        (
            row.try_get::<Option<Uuid>, _>("last_job_id")
                .unwrap()
                .unwrap(),
            row.try_get("last_job_status").unwrap(),
            row.try_get("failure_count").unwrap(),
            row.try_get("last_error").unwrap(),
        )
    }

    async fn job_targets(pool: &PgPool, job_id: Uuid) -> Vec<(String, String, Option<String>)> {
        sqlx::query(
            r#"
            SELECT client_id, status, message
            FROM job_targets
            WHERE job_id = $1
            ORDER BY client_id ASC
            "#,
        )
        .bind(job_id)
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|row| {
            (
                row.try_get("client_id").unwrap(),
                row.try_get("status").unwrap(),
                row.try_get("message").unwrap(),
            )
        })
        .collect()
    }

    async fn job_status_output(pool: &PgPool, job_id: Uuid, client_id: &str) -> serde_json::Value {
        let data: Vec<u8> = sqlx::query_scalar(
            r#"
            SELECT data
            FROM job_outputs
            WHERE job_id = $1 AND client_id = $2 AND seq = 0
            "#,
        )
        .bind(job_id)
        .bind(client_id)
        .fetch_one(pool)
        .await
        .unwrap();
        serde_json::from_slice(&data).unwrap()
    }

    const WORKER_HOT_RELOAD_ENV: &[&str] = &[
        "VPSMAN_WORKER_TICK_SECS",
        "VPSMAN_WORKER_LEASE_SECS",
        "VPSMAN_AGENT_OFFLINE_TIMEOUT_SECS",
        "VPSMAN_WORKER_NOTIFICATION_DELIVERY_LIMIT",
        "VPSMAN_WORKER_NOTIFICATION_RETENTION_DAYS",
        "VPSMAN_WORKER_NOTIFICATION_RETENTION_PRUNE_LIMIT",
        "VPSMAN_WORKER_NOTIFICATION_WEBHOOK_TIMEOUT_SECS",
        "VPSMAN_WORKER_WEBHOOK_RULE_DELIVERY_LIMIT",
        "VPSMAN_WORKER_WEBHOOK_RULE_MATERIALIZE_LIMIT",
        "VPSMAN_WORKER_WEBHOOK_RULE_RETENTION_DAYS",
        "VPSMAN_WORKER_WEBHOOK_RULE_RETENTION_PRUNE_LIMIT",
        "VPSMAN_WORKER_WEBHOOK_RULE_TIMEOUT_SECS",
        "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_ENABLED",
        "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_LIMIT",
        "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_DRY_RUN",
        "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_INCLUDE_DISABLED",
        "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_DELETE_OBJECTS",
        "VPSMAN_WORKER_BACKUP_POLICY_PRUNE_OBJECT_STORE_DIR",
        "VPSMAN_BACKUP_OBJECT_STORE_DIR",
        "VPSMAN_OBJECT_ENDPOINT",
        "VPSMAN_OBJECT_BUCKET",
        "VPSMAN_OBJECT_ACCESS_KEY",
        "VPSMAN_OBJECT_SECRET_KEY",
        "VPSMAN_OBJECT_REGION",
        "VPSMAN_OBJECT_CREATE_BUCKET",
        "VPSMAN_WORKER_SCHEDULE_JOB_MAX_TIMEOUT_SECS",
        "VPSMAN_REQUIRE_REGISTERED_AGENT_UPDATES",
    ];

    static WORKER_SUITE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_cleared_worker_env<R>(names: &[&str], run: impl FnOnce() -> R) -> R {
        let _guard = WORKER_SUITE_ENV_LOCK.lock().unwrap();
        let saved = names
            .iter()
            .map(|name| (*name, std::env::var_os(name)))
            .collect::<Vec<_>>();
        for name in names {
            std::env::remove_var(name);
        }
        let result = run();
        for (name, value) in saved {
            if let Some(value) = value {
                std::env::set_var(name, value);
            } else {
                std::env::remove_var(name);
            }
        }
        result
    }

    fn temp_suite_config_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("vpsman-{label}-{}.toml", Uuid::new_v4()))
    }

    #[allow(clippy::too_many_arguments)]
    fn worker_runtime_toml(
        tick_secs: u64,
        worker_lease_secs: i32,
        agent_offline_timeout_secs: i64,
        schedule_job_max_timeout_secs: u64,
        require_registered_agent_updates: bool,
        notification_delivery_limit: i64,
        notification_retention_days: i64,
        notification_retention_prune_limit: i64,
        notification_webhook_timeout_secs: u64,
        webhook_rule_delivery_limit: i64,
        webhook_rule_materialize_limit: i64,
        webhook_rule_retention_days: i64,
        webhook_rule_retention_prune_limit: i64,
        webhook_rule_timeout_secs: u64,
        backup_policy_prune_enabled: bool,
        object_store_dir: &str,
    ) -> String {
        format!(
            r#"version = 1

[worker]
tick_secs = {tick_secs}
worker_lease_secs = {worker_lease_secs}
agent_offline_timeout_secs = {agent_offline_timeout_secs}
schedule_job_max_timeout_secs = {schedule_job_max_timeout_secs}
require_registered_agent_updates = {require_registered_agent_updates}
notification_delivery_limit = {notification_delivery_limit}
notification_retention_days = {notification_retention_days}
notification_retention_prune_limit = {notification_retention_prune_limit}
notification_webhook_timeout_secs = {notification_webhook_timeout_secs}
webhook_rule_delivery_limit = {webhook_rule_delivery_limit}
webhook_rule_materialize_limit = {webhook_rule_materialize_limit}
webhook_rule_retention_days = {webhook_rule_retention_days}
webhook_rule_retention_prune_limit = {webhook_rule_retention_prune_limit}
webhook_rule_timeout_secs = {webhook_rule_timeout_secs}
backup_policy_prune_enabled = {backup_policy_prune_enabled}
backup_policy_prune_object_store_dir = "{object_store_dir}"

[storage]
backup_object_store_dir = "{object_store_dir}/backups"
"#
        )
    }
}
