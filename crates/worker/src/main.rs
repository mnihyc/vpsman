use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use anyhow::{bail, Context, Result};
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
#[cfg(test)]
use vpsman_common::VpsMetadata;
use vpsman_common::{
    expression_matches, parse_expression, payload_hash, AgentCapabilitySnapshot, Expression,
    ExpressionContext, JobCommand, SuiteConfig, SERVER_JOB_STATUS_COMPLETED,
    SERVER_JOB_STATUS_FAILED, SERVER_JOB_STATUS_QUEUED, SERVER_JOB_STATUS_RUNNING,
    SERVER_JOB_TYPE_ARTIFACT_CLEANUP,
};
use vpsman_server_core::{
    job_command_type_label, scheduled_command_type_label, split_targets_by_capability,
    validate_network_apply_target, CapabilitySkip, TargetCapability, JOB_STATUS_PARTIAL_SUCCESS,
    JOB_STATUS_QUEUED, JOB_STATUS_SKIPPED, TARGET_STATUS_QUEUED, TARGET_STATUS_SKIPPED,
};

mod alert_notifications;
mod backup_policy_retention;
mod build_info;
mod webhook_rules;
mod worker_leases;

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
    #[arg(
        long,
        env = "VPSMAN_WORKER_SCHEDULE_COMMAND_TIMEOUT_SECS",
        default_value_t = 30
    )]
    schedule_command_timeout_secs: u64,
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
    backup_policy_prune_object_store_dir: Option<PathBuf>,
}

impl WorkerRuntimeConfig {
    fn from_args(args: &Args) -> Self {
        Self {
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
            ),
            backup_policy_prune_config: BackupPolicyRetentionPruneConfig::new(
                args.backup_policy_prune_enabled,
                args.backup_policy_prune_limit,
                args.backup_policy_prune_dry_run,
                args.backup_policy_prune_include_disabled,
                args.backup_policy_prune_delete_objects,
                args.backup_policy_prune_object_store_dir.clone(),
            ),
            schedule_dispatch_config: ScheduleDispatchConfig::new(
                args.schedule_command_timeout_secs,
                args.require_registered_agent_updates,
            ),
            backup_policy_prune_object_store_dir: args.backup_policy_prune_object_store_dir.clone(),
        }
    }
}

fn load_worker_runtime_config(base_args: &Args) -> Result<WorkerRuntimeConfig> {
    let mut args = base_args.clone();
    let suite_config =
        SuiteConfig::load_optional(&args.suite_config).map_err(anyhow::Error::msg)?;
    args.apply_suite_config(&suite_config)
        .map_err(anyhow::Error::msg)?;
    Ok(WorkerRuntimeConfig::from_args(&args))
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
                .as_deref()
                .or(config.storage.object_store_dir.as_deref()),
        );
        apply_u64_default(
            &mut self.schedule_command_timeout_secs,
            "VPSMAN_WORKER_SCHEDULE_COMMAND_TIMEOUT_SECS",
            config
                .worker
                .schedule_command_timeout_secs
                .or(config.timeout.worker_schedule_command_secs),
        );
        apply_bool_default(
            &mut self.require_registered_agent_updates,
            "VPSMAN_REQUIRE_REGISTERED_AGENT_UPDATES",
            config.worker.require_registered_agent_updates,
        );
        Ok(())
    }
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
        version = env!("CARGO_PKG_VERSION"),
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
        let runtime_config = WorkerRuntimeConfig::from_args(&args);
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
            runtime_config.backup_policy_prune_object_store_dir.as_ref(),
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
                WorkerRuntimeConfig::from_args(&args)
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
            runtime_config.backup_policy_prune_object_store_dir.as_ref(),
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

async fn detect_offline_agents(pool: &PgPool, timeout_secs: i64) -> Result<u64> {
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
    .bind(timeout_secs as f64)
    .fetch_all(&mut *tx)
    .await?;
    for row in &rows {
        let client_id: String = row.try_get("id")?;
        let metadata = serde_json::json!({
            "from_status": "online",
            "to_status": "offline",
            "reason": "agent_offline_timeout",
            "offline_timeout_secs": timeout_secs,
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

async fn expire_stale_gateway_sessions(pool: &PgPool, timeout_secs: i64) -> Result<u64> {
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
    .bind(timeout_secs as f64)
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
}

struct ArtifactCleanupJob {
    id: Uuid,
    expression: String,
}

struct ArtifactCleanupCandidate {
    id: Uuid,
    domain: String,
    object_key: String,
    sha256_hex: String,
    size_bytes: i64,
    status: String,
    job_id: Option<Uuid>,
    client_id: Option<String>,
    stream: Option<String>,
    seq: Option<i32>,
    backup_artifact_id: Option<Uuid>,
    created_at: String,
}

async fn process_artifact_cleanup_jobs_if_leader(
    pool: &PgPool,
    object_store_dir: Option<&PathBuf>,
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
    process_artifact_cleanup_jobs(pool, object_store_dir).await
}

async fn process_artifact_cleanup_jobs(
    pool: &PgPool,
    object_store_dir: Option<&PathBuf>,
) -> Result<ArtifactCleanupRun> {
    let Some(job) = claim_artifact_cleanup_job(pool).await? else {
        return Ok(ArtifactCleanupRun::default());
    };
    let result = run_artifact_cleanup_job(pool, object_store_dir, &job).await;
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
                        'tombstoned_bytes', $6::bigint
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
            .execute(pool)
            .await?;
            Ok(ArtifactCleanupRun { jobs: 1, ..run })
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
        RETURNING job.id, job.expression
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
            expression: row
                .try_get::<Option<String>, _>("expression")?
                .unwrap_or_default(),
        })
    })
    .transpose()
}

async fn run_artifact_cleanup_job(
    pool: &PgPool,
    object_store_dir: Option<&PathBuf>,
    job: &ArtifactCleanupJob,
) -> Result<ArtifactCleanupRun> {
    let object_store_dir = object_store_dir
        .context("artifact cleanup requires VPSMAN_WORKER_BACKUP_POLICY_PRUNE_OBJECT_STORE_DIR")?;
    let parsed = parse_expression(&job.expression).map_err(|error| anyhow::anyhow!(error))?;
    let candidates = artifact_cleanup_candidates(pool).await?;
    let mut run = ArtifactCleanupRun::default();
    for candidate in candidates
        .iter()
        .filter(|candidate| artifact_cleanup_candidate_matches(candidate, parsed.as_ref()))
        .take(1000)
    {
        match apply_artifact_cleanup_candidate(pool, object_store_dir, candidate).await? {
            ArtifactCleanupDisposition::Deleted => {
                run.deleted_rows += 1;
                run.deleted_bytes += candidate.size_bytes;
            }
            ArtifactCleanupDisposition::Tombstoned => {
                run.tombstoned_rows += 1;
                run.tombstoned_bytes += candidate.size_bytes;
            }
        }
    }
    Ok(run)
}

enum ArtifactCleanupDisposition {
    Deleted,
    Tombstoned,
}

async fn apply_artifact_cleanup_candidate(
    pool: &PgPool,
    object_store_dir: &Path,
    candidate: &ArtifactCleanupCandidate,
) -> Result<ArtifactCleanupDisposition> {
    match candidate.domain.as_str() {
        "job_output" => delete_job_output_artifact(pool, object_store_dir, candidate).await,
        "file_transfer_handoff" => {
            delete_unreferenced_server_artifact(pool, object_store_dir, candidate).await
        }
        "file_transfer_source" => {
            delete_file_transfer_source_artifact(pool, object_store_dir, candidate).await
        }
        "agent_update" => {
            if agent_update_artifact_is_referenced(pool, &candidate.object_key).await? {
                tombstone_server_artifact(pool, candidate.id).await
            } else {
                delete_unreferenced_server_artifact(pool, object_store_dir, candidate).await
            }
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
                delete_backup_artifact(pool, object_store_dir, candidate).await
            }
        }
        _ => tombstone_server_artifact(pool, candidate.id).await,
    }
}

async fn delete_job_output_artifact(
    pool: &PgPool,
    object_store_dir: &Path,
    candidate: &ArtifactCleanupCandidate,
) -> Result<ArtifactCleanupDisposition> {
    delete_object_key_best_effort(object_store_dir, &candidate.object_key).await;
    let mut tx = pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE job_outputs
        SET storage = 'inline', object_key = NULL
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
    object_store_dir: &Path,
    candidate: &ArtifactCleanupCandidate,
) -> Result<ArtifactCleanupDisposition> {
    delete_object_key_best_effort(object_store_dir, &candidate.object_key).await;
    let mut tx = pool.begin().await?;
    mark_server_artifact_deleted(&mut tx, candidate.id).await?;
    tx.commit().await?;
    Ok(ArtifactCleanupDisposition::Deleted)
}

async fn delete_file_transfer_source_artifact(
    pool: &PgPool,
    object_store_dir: &Path,
    candidate: &ArtifactCleanupCandidate,
) -> Result<ArtifactCleanupDisposition> {
    delete_object_key_best_effort(object_store_dir, &candidate.object_key).await;
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
    object_store_dir: &Path,
    candidate: &ArtifactCleanupCandidate,
) -> Result<ArtifactCleanupDisposition> {
    delete_object_key_best_effort(object_store_dir, &candidate.object_key).await;
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

async fn mark_server_artifact_deleted(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    artifact_id: Uuid,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE server_artifacts
        SET status = 'deleted', deleted_at = now()
        WHERE id = $1
        "#,
    )
    .bind(artifact_id)
    .execute(&mut **tx)
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
        "#,
    )
    .bind(artifact_id)
    .execute(pool)
    .await?;
    Ok(ArtifactCleanupDisposition::Tombstoned)
}

async fn agent_update_artifact_is_referenced(pool: &PgPool, object_key: &str) -> Result<bool> {
    let referenced = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM agent_update_releases
            WHERE artifact_object_key = $1
               OR rollback_artifact_object_key = $1
        )
        "#,
    )
    .bind(object_key)
    .fetch_one(pool)
    .await?;
    Ok(referenced)
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

async fn artifact_cleanup_candidates(pool: &PgPool) -> Result<Vec<ArtifactCleanupCandidate>> {
    let rows = sqlx::query(
        r#"
        SELECT
            id,
            domain,
            object_key,
            sha256_hex,
            size_bytes,
            status,
            job_id,
            client_id,
            stream,
            seq,
            backup_artifact_id,
            created_at::text AS created_at
        FROM server_artifacts
        WHERE status = 'active'
        ORDER BY created_at DESC, object_key ASC
        LIMIT 10000
        "#,
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ArtifactCleanupCandidate {
                id: row.try_get("id")?,
                domain: row.try_get("domain")?,
                object_key: row.try_get("object_key")?,
                sha256_hex: row.try_get("sha256_hex")?,
                size_bytes: row.try_get("size_bytes")?,
                status: row.try_get("status")?,
                job_id: row.try_get("job_id")?,
                client_id: row.try_get("client_id")?,
                stream: row.try_get("stream")?,
                seq: row.try_get("seq")?,
                backup_artifact_id: row.try_get("backup_artifact_id")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
        .map_err(Into::into)
}

fn artifact_cleanup_candidate_matches(
    candidate: &ArtifactCleanupCandidate,
    expression: Option<&Expression>,
) -> bool {
    let Some(expression) = expression else {
        return true;
    };
    let mut objects = BTreeMap::new();
    objects.insert(
        "artifact".to_string(),
        serde_json::json!({
            "domain": &candidate.domain,
            "object": &candidate.object_key,
            "size": candidate.size_bytes,
            "status": &candidate.status,
            "job": candidate.job_id.map(|id| id.to_string()),
            "client": candidate.client_id.as_deref(),
            "stream": candidate.stream.as_deref(),
            "seq": candidate.seq,
            "sha256": &candidate.sha256_hex,
            "created_at": &candidate.created_at,
        }),
    );
    expression_matches(
        &ExpressionContext {
            objects,
            ..ExpressionContext::default()
        },
        expression,
    )
}

async fn delete_object_key_best_effort(root: &Path, object_key: &str) {
    if object_key.starts_with('/')
        || object_key.contains('\\')
        || object_key.split('/').any(|segment| {
            segment.is_empty()
                || segment == "."
                || segment == ".."
                || segment.as_bytes().contains(&0)
        })
    {
        return;
    }
    let mut path = root.to_path_buf();
    for segment in object_key.split('/') {
        path.push(segment);
    }
    let _ = tokio::fs::remove_file(path).await;
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
            operation: row.try_get::<SqlJson<Value>, _>("operation")?.0,
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
    operation: Value,
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
    timeout_secs: u64,
    require_registered_agent_updates: bool,
}

impl ScheduleDispatchConfig {
    fn new(timeout_secs: u64, require_registered_agent_updates: bool) -> Self {
        Self {
            timeout_secs: timeout_secs.clamp(1, 3600),
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
    let operation_bytes = serde_json::to_vec(&schedule.operation)?;
    let command_hash = payload_hash(&operation_bytes);
    let operation: JobCommand = serde_json::from_value(schedule.operation.clone())
        .context("scheduled operation is not a valid job command")?;
    validate_network_apply_target(&operation, &targets)
        .map_err(|error| anyhow::anyhow!(error.code()))?;
    if !scheduled_agent_update_release_policy_allows(
        tx,
        &operation,
        dispatch_config.require_registered_agent_updates,
    )
    .await?
    {
        bail!("registered agent update release missing");
    }
    let target_capabilities = load_schedule_target_capabilities(tx, &targets).await?;
    let timeout_secs = effective_schedule_timeout_secs(
        dispatch_config.timeout_secs,
        &targets,
        &target_capabilities,
    );
    let (dispatch_targets, capability_skips) =
        split_targets_by_capability(&operation, &targets, &target_capabilities, false);
    let operation_type = schedule
        .operation
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let job_id = Uuid::new_v4();
    let status = if targets.is_empty() {
        JOB_STATUS_SKIPPED
    } else if dispatch_targets.is_empty() && !capability_skips.is_empty() {
        JOB_STATUS_PARTIAL_SUCCESS
    } else {
        JOB_STATUS_QUEUED
    };
    let job_completed_immediately =
        matches!(status, JOB_STATUS_SKIPPED | JOB_STATUS_PARTIAL_SUCCESS);
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
        "timeout_secs": timeout_secs,
        "privileged": true,
        "force_unprivileged": false,
        "source_schedule_id": schedule.id,
    }))?);
    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, actor_id, command_type, privileged, status, target_count,
            payload_hash, operation, source_schedule_id, request_fingerprint,
            timeout_secs, completed_at
        )
        VALUES ($1, $2, $3, TRUE, $4, $5, $6, $7, $8, $9, $10,
            CASE WHEN $11 THEN now() ELSE NULL END)
        "#,
    )
    .bind(job_id)
    .bind(schedule.actor_id)
    .bind(&command_type)
    .bind(status)
    .bind(targets.len() as i32)
    .bind(&command_hash)
    .bind(SqlJson(&schedule.operation))
    .bind(schedule.id)
    .bind(&request_fingerprint)
    .bind(timeout_secs as i64)
    .bind(job_completed_immediately)
    .execute(&mut **tx)
    .await?;

    for client_id in &targets {
        let skip = capability_skips
            .iter()
            .find(|skip| skip.client_id == *client_id);
        let target_status = if skip.is_some() {
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
        .bind(skip.map(|skip| skip.failure.message))
        .bind(skip.map(|_| 0_i32))
        .execute(&mut **tx)
        .await?;
    }

    record_schedule_capability_skip_outputs(tx, job_id, &operation, &capability_skips).await?;

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
        "selector_expression": &schedule.selector_expression,
        "catch_up_policy": &schedule.catch_up_policy,
        "catch_up_run_index": run_index + 1,
        "catch_up_run_count": run_count,
        "retry_delay_secs": schedule.retry_delay_secs,
        "max_failures": schedule.max_failures,
        "failure_count_before_run": schedule.failure_count,
        "last_error_before_run": &schedule.last_error,
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
                WHEN $3 IN ('completed', 'partial_success', 'skipped') THEN NULL
                ELSE $3
            END,
            failure_count = CASE WHEN $4 THEN 0 ELSE failure_count END,
            last_error = CASE WHEN $4 THEN NULL ELSE last_error END,
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
            targets: &targets,
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
            &targets,
        )
        .await?;
    }

    Ok(!targets.is_empty())
}

async fn load_schedule_target_capabilities(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    targets: &[String],
) -> Result<Vec<TargetCapability>> {
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT id, capabilities
        FROM clients
        WHERE hidden_at IS NULL
          AND id = ANY($1)
        "#,
    )
    .bind(targets.to_vec())
    .fetch_all(&mut **tx)
    .await?;
    let mut capabilities = Vec::with_capacity(rows.len());
    for row in rows {
        let client_id: String = row.try_get("id")?;
        let snapshot: SqlJson<AgentCapabilitySnapshot> = row.try_get("capabilities")?;
        capabilities.push(TargetCapability {
            client_id,
            capabilities: snapshot.0,
        });
    }
    for target in targets {
        if !capabilities
            .iter()
            .any(|capability| capability.client_id == *target)
        {
            bail!("fixed_target_not_found");
        }
    }
    Ok(capabilities)
}

fn effective_schedule_timeout_secs(
    configured_timeout_secs: u64,
    targets: &[String],
    capabilities: &[TargetCapability],
) -> u64 {
    targets
        .iter()
        .filter_map(|client_id| {
            capabilities
                .iter()
                .find(|capability| capability.client_id == *client_id)
                .map(|capability| capability.capabilities.command_timeout_secs.clamp(1, 3600))
        })
        .fold(configured_timeout_secs.clamp(1, 3600), u64::min)
}

async fn scheduled_agent_update_release_policy_allows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &JobCommand,
    require_registered_agent_updates: bool,
) -> Result<bool> {
    if !require_registered_agent_updates {
        return Ok(true);
    }
    let JobCommand::UpdateAgent {
        sha256_hex,
        artifact_signing_key_hex,
        ..
    } = command
    else {
        return Ok(true);
    };
    let Some(signing_key_hex) = artifact_signing_key_hex else {
        return Ok(false);
    };
    let artifact_sha256_hex = sha256_hex.to_ascii_lowercase();
    let signing_key_sha256_hex = payload_hash(signing_key_hex.to_ascii_lowercase().as_bytes());
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM agent_update_releases
            WHERE status IN ('published_metadata_only', 'artifact_hosted')
              AND artifact_sha256_hex = $1
              AND artifact_signing_key_sha256_hex = $2
        )
        "#,
    )
    .bind(artifact_sha256_hex)
    .bind(signing_key_sha256_hex)
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

    fn schedule_with_policy(policy: &str, limit: i32) -> DueSchedule {
        DueSchedule {
            id: Uuid::nil(),
            actor_id: None,
            name: "test".to_string(),
            operation: serde_json::json!({"type": "shell", "argv": ["/bin/true"], "pty": false}),
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
    fn schedule_timeout_clamps_to_target_capabilities() {
        let targets = vec!["edge-a".to_string(), "edge-b".to_string()];
        let capabilities = vec![
            TargetCapability {
                client_id: "edge-a".to_string(),
                capabilities: AgentCapabilitySnapshot {
                    command_timeout_secs: 20,
                    ..AgentCapabilitySnapshot::default()
                },
            },
            TargetCapability {
                client_id: "edge-b".to_string(),
                capabilities: AgentCapabilitySnapshot {
                    command_timeout_secs: 120,
                    ..AgentCapabilitySnapshot::default()
                },
            },
        ];

        assert_eq!(
            effective_schedule_timeout_secs(90, &targets, &capabilities),
            20
        );
        assert_eq!(
            effective_schedule_timeout_secs(10, &targets, &capabilities),
            10
        );
        assert_eq!(effective_schedule_timeout_secs(90, &[], &[]), 90);
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
            assert_eq!(runtime.schedule_dispatch_config.timeout_secs, 41);
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
                    .object_store_dir
                    .as_deref(),
                Some(object_dir.as_path())
            );

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
            assert_eq!(runtime.schedule_dispatch_config.timeout_secs, 55);
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
    fn suite_bool_defaults_do_not_disable_explicit_true_flags() {
        let env_name = "VPSMAN_WORKER_APPLY_BOOL_DEFAULT_TEST_UNSET";

        let mut explicit_true = true;
        apply_bool_default(&mut explicit_true, env_name, Some(false));
        assert!(explicit_true);

        let mut default_false = false;
        apply_bool_default(&mut default_false, env_name, Some(true));
        assert!(default_false);
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
        "VPSMAN_WORKER_SCHEDULE_COMMAND_TIMEOUT_SECS",
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
        schedule_command_timeout_secs: u64,
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
schedule_command_timeout_secs = {schedule_command_timeout_secs}
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
"#
        )
    }
}
