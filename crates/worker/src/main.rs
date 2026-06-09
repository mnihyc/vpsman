use std::{path::PathBuf, str::FromStr, time::Duration};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use croner::Cron;
use ed25519_dalek::SigningKey;
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
    expression_matches, job_command_protocol_version, parse_expression, payload_hash,
    sign_command_envelope, CommandEnvelope, CommandOutput, ExpressionContext,
    GatewayCommandDispatch, GatewayCommandDispatchResult, JobCommand, JobRequest, OutputStream,
    VpsMetadata, MAX_COMMAND_SIGNATURE_AGE_SECS,
};

mod alert_notifications;
mod backup_policy_retention;
mod build_info;
mod rollout_automation;
mod webhook_rules;
mod worker_leases;

use alert_notifications::{
    process_alert_notifications, AlertNotificationWorkerConfig, AlertNotificationWorkerRun,
};
use backup_policy_retention::{
    process_backup_policy_retention_prune, BackupPolicyRetentionPruneConfig,
    BackupPolicyRetentionPruneRun,
};
use rollout_automation::process_rollout_automation;
use webhook_rules::{
    ensure_event_partitions, insert_webhook_event_in_tx, process_webhook_rules,
    WebhookRuleWorkerConfig, WebhookRuleWorkerRun,
};
use worker_leases::acquire_worker_lease;

#[derive(Debug, Parser)]
#[command(name = "vpsman-worker", about = "Background scheduler for vpsman")]
struct Args {
    #[arg(long, env = "VPSMAN_WORKER_TICK_SECS", default_value_t = 30)]
    tick_secs: u64,
    #[arg(long, env = "VPSMAN_POSTGRES_URL")]
    postgres_url: Option<String>,
    #[arg(long, env = "VPSMAN_MIGRATIONS_DIR", default_value = "migrations")]
    migrations_dir: PathBuf,
    #[arg(long, env = "VPSMAN_WORKER_ONCE", default_value_t = false)]
    once: bool,
    #[arg(long, env = "VPSMAN_WORKER_ID")]
    worker_id: Option<String>,
    #[arg(long, env = "VPSMAN_WORKER_LEASE_SECS", default_value_t = 60)]
    worker_lease_secs: i32,
    #[arg(
        long,
        env = "VPSMAN_WORKER_ROLLOUT_HEARTBEAT_TIMEOUT_SECS",
        default_value_t = 900
    )]
    rollout_heartbeat_timeout_secs: i32,
    #[arg(
        long,
        env = "VPSMAN_WORKER_ROLLOUT_RECONCILE_LIMIT",
        default_value_t = 50
    )]
    rollout_reconcile_limit: i64,
    #[arg(long, env = "VPSMAN_WORKER_ROLLOUT_WORKER_ID")]
    rollout_worker_id: Option<String>,
    #[arg(long, env = "VPSMAN_WORKER_ROLLOUT_LEASE_SECS", default_value_t = 60)]
    rollout_lease_secs: i32,
    #[arg(
        long,
        env = "VPSMAN_AGENT_OFFLINE_TIMEOUT_SECS",
        default_value_t = 300
    )]
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
    #[arg(long, env = "VPSMAN_WORKER_GATEWAY_CONTROL_URL")]
    schedule_gateway_control_url: Option<String>,
    #[arg(long, env = "VPSMAN_WORKER_INTERNAL_TOKEN")]
    schedule_internal_token: Option<String>,
    #[arg(long, env = "VPSMAN_WORKER_SERVER_SIGNING_KEY_HEX")]
    schedule_server_signing_key_hex: Option<String>,
    #[arg(
        long,
        env = "VPSMAN_WORKER_SCHEDULE_COMMAND_TIMEOUT_SECS",
        default_value_t = 30
    )]
    schedule_command_timeout_secs: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "vpsman_worker=info".into()),
        )
        .init();

    let args = Args::parse();
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
    let pool = connect_postgres(postgres_url, &args.migrations_dir).await?;
    let worker_id = args
        .worker_id
        .clone()
        .or_else(|| args.rollout_worker_id.clone())
        .unwrap_or_else(|| format!("vpsman-worker-{}", std::process::id()));
    let rollout_worker_id = args
        .rollout_worker_id
        .clone()
        .unwrap_or_else(|| worker_id.clone());
    let alert_notification_config = AlertNotificationWorkerConfig::new(
        args.notification_delivery_limit,
        args.notification_retention_days,
        args.notification_retention_prune_limit,
        args.notification_webhook_timeout_secs,
    );
    let webhook_rule_config = WebhookRuleWorkerConfig::new(
        args.webhook_rule_delivery_limit,
        args.webhook_rule_materialize_limit,
        args.webhook_rule_retention_days,
        args.webhook_rule_retention_prune_limit,
        args.webhook_rule_timeout_secs,
    );
    let backup_policy_prune_config = BackupPolicyRetentionPruneConfig::new(
        args.backup_policy_prune_enabled,
        args.backup_policy_prune_limit,
        args.backup_policy_prune_dry_run,
        args.backup_policy_prune_include_disabled,
        args.backup_policy_prune_delete_objects,
        args.backup_policy_prune_object_store_dir.clone(),
    );
    let schedule_dispatch_config = ScheduleDispatchConfig::new(
        args.schedule_gateway_control_url.clone(),
        args.schedule_internal_token.clone(),
        args.schedule_server_signing_key_hex.clone(),
        args.schedule_command_timeout_secs,
    )?;
    info!(tick_secs = args.tick_secs, "worker started");
    if args.once {
        let schedules_processed = process_due_schedules_if_leader(
            &pool,
            25,
            &worker_id,
            args.worker_lease_secs,
            &schedule_dispatch_config,
        )
        .await?;
        let alert_notifications = process_alert_notifications_if_leader(
            &pool,
            alert_notification_config,
            &worker_id,
            args.worker_lease_secs,
        )
        .await?;
        let webhook_rules = process_webhook_rules_if_leader(
            &pool,
            webhook_rule_config,
            &worker_id,
            args.worker_lease_secs,
        )
        .await?;
        let rollout_automation = process_rollout_automation(
            &pool,
            args.rollout_reconcile_limit,
            args.rollout_heartbeat_timeout_secs,
            &rollout_worker_id,
            args.rollout_lease_secs,
        )
        .await?;
        let backup_policy_prune = process_backup_policy_retention_prune_if_leader(
            &pool,
            backup_policy_prune_config.clone(),
            &worker_id,
            args.worker_lease_secs,
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
            expired_rollout_heartbeats = rollout_automation.expired_heartbeats,
            reconciled_rollouts = rollout_automation.reconciled_rollouts,
            backup_policy_prune_policies = backup_policy_prune.policies_scanned,
            backup_policy_prune_matched = backup_policy_prune.matched_rows,
            backup_policy_prune_pruned = backup_policy_prune.pruned_rows,
            "worker once completed"
        );
        return Ok(());
    }

    let mut ticker = time::interval(Duration::from_secs(args.tick_secs.max(1)));
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
        match process_due_schedules_if_leader(
            &pool,
            25,
            &worker_id,
            args.worker_lease_secs,
            &schedule_dispatch_config,
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
            alert_notification_config,
            &worker_id,
            args.worker_lease_secs,
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
            webhook_rule_config,
            &worker_id,
            args.worker_lease_secs,
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
        match process_rollout_automation(
            &pool,
            args.rollout_reconcile_limit,
            args.rollout_heartbeat_timeout_secs,
            &rollout_worker_id,
            args.rollout_lease_secs,
        )
        .await
        {
            Ok(run) => {
                if run.expired_heartbeats > 0 || run.reconciled_rollouts > 0 {
                    info!(
                        expired_heartbeats = run.expired_heartbeats,
                        reconciled_rollouts = run.reconciled_rollouts,
                        "processed rollout automation"
                    );
                }
            }
            Err(error) => warn!(%error, "failed to process rollout automation"),
        }
        match process_backup_policy_retention_prune_if_leader(
            &pool,
            backup_policy_prune_config.clone(),
            &worker_id,
            args.worker_lease_secs,
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
        if last_offline_check.elapsed() >= Duration::from_secs(60) {
            last_offline_check = tokio::time::Instant::now();
            match detect_offline_agents(&pool, args.agent_offline_timeout_secs).await {
                Ok(count) => {
                    if count > 0 {
                        info!(count, "detected offline agents");
                    }
                }
                Err(error) => warn!(%error, "failed to detect offline agents"),
            }
        }
    }
}

async fn detect_offline_agents(pool: &PgPool, timeout_secs: i64) -> Result<u64> {
    let result = sqlx::query(
        r#"
        WITH updated AS (
            UPDATE clients
            SET status = 'offline'
            WHERE status = 'online'
              AND last_seen_at < now() - make_interval(secs => $1)
            RETURNING id
        ),
        inserted AS (
            INSERT INTO webhook_events (id, event_type, client_id, metadata, created_at)
            SELECT gen_random_uuid(), 'agent.status_offline', id,
                jsonb_build_object(
                    'from_status', 'online',
                    'to_status', 'offline',
                    'reason', 'agent_offline_timeout'
                ),
                now()
            FROM updated
        )
        SELECT count(*) AS client_count FROM updated
        "#,
    )
    .bind(timeout_secs as f64)
    .fetch_one(pool)
    .await?;
    let count: i64 = result.try_get("client_count")?;
    if count > 0 {
        let _ = sqlx::query("SELECT pg_notify('webhook_events', 'offline_detection')")
            .execute(pool)
            .await;
    }
    Ok(count as u64)
}

async fn connect_postgres(postgres_url: &str, migrations_dir: &std::path::Path) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(3)
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
        let mut dispatches = Vec::new();
        for run_index in 0..run_count {
            if let Some(dispatch) =
                materialize_due_schedule(&mut tx, &schedule, run_index, run_count).await?
            {
                dispatches.push(dispatch);
            }
        }
        advance_schedule_after_success(&mut tx, &schedule, run_count).await?;
        tx.commit().await?;
        for dispatch in dispatches {
            if let Err(error) = dispatch_scheduled_run(pool, dispatch_config, dispatch).await {
                warn!(%error, schedule_id = %schedule.id, "scheduled command dispatch failed");
            }
        }
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
struct ScheduledDispatch {
    job_id: Uuid,
    schedule_id: Uuid,
    schedule_name: String,
    selector_expression: String,
    actor_id: Option<Uuid>,
    operation: JobCommand,
    command_type: String,
    command_hash: String,
    targets: Vec<String>,
}

#[derive(Clone)]
struct ScheduleDispatchConfig {
    gateway_control_url: Option<String>,
    internal_token: Option<String>,
    signing_key: Option<SigningKey>,
    timeout_secs: u64,
    http: reqwest::Client,
}

impl ScheduleDispatchConfig {
    fn new(
        gateway_control_url: Option<String>,
        internal_token: Option<String>,
        server_signing_key_hex: Option<String>,
        timeout_secs: u64,
    ) -> Result<Self> {
        let signing_key = server_signing_key_hex
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(decode_server_signing_key)
            .transpose()?;
        Ok(Self {
            gateway_control_url: gateway_control_url
                .map(|value| value.trim_end_matches('/').to_string())
                .filter(|value| !value.is_empty()),
            internal_token: internal_token
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            signing_key,
            timeout_secs: timeout_secs.clamp(1, 3600),
            http: reqwest::Client::new(),
        })
    }

    fn configured(&self) -> bool {
        self.gateway_control_url.is_some()
            && self.internal_token.is_some()
            && self.signing_key.is_some()
    }
}

#[derive(Debug)]
struct ScheduledTargetOutcome {
    client_id: String,
    status: String,
    exit_code: Option<i32>,
    command_version: Option<u16>,
    accepted: bool,
    message: String,
    outputs: Vec<CommandOutput>,
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
) -> Result<Option<ScheduledDispatch>> {
    let targets = resolve_schedule_targets(tx, schedule).await?;
    let operation_bytes = serde_json::to_vec(&schedule.operation)?;
    let command_hash = payload_hash(&operation_bytes);
    let operation: JobCommand = serde_json::from_value(schedule.operation.clone())
        .context("scheduled operation is not a valid job command")?;
    let operation_type = schedule
        .operation
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let job_id = Uuid::new_v4();
    let status = if targets.is_empty() {
        "schedule_no_targets"
    } else {
        "dispatching"
    };
    let command_type = format!(
        "scheduled_{}",
        scheduled_command_type_label(&operation, operation_type)
    );
    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, actor_id, command_type, privileged, status, target_count,
            payload_hash, operation, source_schedule_id, completed_at
        )
        VALUES ($1, $2, $3, TRUE, $4, $5, $6, $7, $8,
            CASE WHEN $4 = 'schedule_no_targets' THEN now() ELSE NULL END)
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
    .execute(&mut **tx)
    .await?;

    for client_id in &targets {
        sqlx::query(
            r#"
            INSERT INTO job_targets (job_id, client_id, status, message)
            VALUES ($1, $2, $3, NULL)
            "#,
        )
        .bind(job_id)
        .bind(client_id)
        .bind("queued")
        .execute(&mut **tx)
        .await?;
    }

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
        "schedule.due_dispatching"
    })
    .bind(format!("schedule:{}", schedule.id))
    .bind(&command_hash)
    .bind(serde_json::json!({
        "schedule_id": schedule.id,
        "schedule_name": schedule.name,
        "operation_type": operation_type,
        "job_id": job_id,
        "resolved_targets": &targets,
        "selector_expression": &schedule.selector_expression,
        "catch_up_policy": &schedule.catch_up_policy,
        "catch_up_run_index": run_index + 1,
        "catch_up_run_count": run_count,
        "retry_delay_secs": schedule.retry_delay_secs,
        "max_failures": schedule.max_failures,
        "failure_count_before_run": schedule.failure_count,
        "last_error_before_run": &schedule.last_error,
        "reason": "saved schedule intent was previously privilege-unlocked; worker automation is dispatching through private gateway control",
    }))
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

    if targets.is_empty() {
        Ok(None)
    } else {
        Ok(Some(ScheduledDispatch {
            job_id,
            schedule_id: schedule.id,
            schedule_name: schedule.name.clone(),
            selector_expression: schedule.selector_expression.clone(),
            actor_id: schedule.actor_id,
            operation,
            command_type,
            command_hash,
            targets,
        }))
    }
}

async fn dispatch_scheduled_run(
    pool: &PgPool,
    config: &ScheduleDispatchConfig,
    dispatch: ScheduledDispatch,
) -> Result<()> {
    if !config.configured() {
        let outcomes = dispatch
            .targets
            .iter()
            .map(|client_id| ScheduledTargetOutcome {
                client_id: client_id.clone(),
                status: "dispatch_failed".to_string(),
                exit_code: None,
                command_version: None,
                accepted: false,
                message: "worker schedule gateway dispatch is not configured".to_string(),
                outputs: Vec::new(),
            })
            .collect::<Vec<_>>();
        record_scheduled_dispatch_outcomes(pool, &dispatch, outcomes).await?;
        return Ok(());
    }
    let signing_key = config
        .signing_key
        .as_ref()
        .context("worker schedule signing key is not configured")?;
    let mut outcomes = Vec::new();
    for client_id in &dispatch.targets {
        let envelope = signed_schedule_envelope(client_id, &dispatch.command_hash, signing_key);
        let request = JobRequest {
            job_id: dispatch.job_id,
            command_version: job_command_protocol_version(&dispatch.operation),
            command: dispatch.operation.clone(),
            envelope,
            timeout_secs: config.timeout_secs,
        };
        let result = dispatch_schedule_command(config, client_id, request).await;
        outcomes.push(match result {
            Ok(result) => scheduled_outcome_from_gateway(result),
            Err(error) => ScheduledTargetOutcome {
                client_id: client_id.clone(),
                status: "dispatch_failed".to_string(),
                exit_code: None,
                command_version: None,
                accepted: false,
                message: error.to_string(),
                outputs: Vec::new(),
            },
        });
    }
    record_scheduled_dispatch_outcomes(pool, &dispatch, outcomes).await
}

fn signed_schedule_envelope(
    client_id: &str,
    command_hash: &str,
    signing_key: &SigningKey,
) -> CommandEnvelope {
    let now = Utc::now().timestamp().max(0) as u64;
    let mut envelope = CommandEnvelope {
        command_id: Uuid::new_v4(),
        scope: format!("client:{client_id}"),
        payload_hash_hex: command_hash.to_string(),
        signed_unix: now,
        expires_unix: now.saturating_add(MAX_COMMAND_SIGNATURE_AGE_SECS),
        server_signature: Vec::new(),
    };
    envelope.server_signature = sign_command_envelope(signing_key, &envelope);
    envelope
}

async fn dispatch_schedule_command(
    config: &ScheduleDispatchConfig,
    client_id: &str,
    request: JobRequest,
) -> Result<GatewayCommandDispatchResult> {
    let url = format!(
        "{}/internal/v1/gateway/command",
        config
            .gateway_control_url
            .as_deref()
            .context("worker schedule gateway control URL is not configured")?
    );
    let response = config
        .http
        .post(url)
        .bearer_auth(
            config
                .internal_token
                .as_deref()
                .context("worker schedule internal token is not configured")?,
        )
        .json(&GatewayCommandDispatch {
            client_id: client_id.to_string(),
            request,
        })
        .send()
        .await
        .with_context(|| format!("failed to dispatch scheduled command to {client_id}"))?;
    let status = response.status();
    let body = response.bytes().await?;
    anyhow::ensure!(
        status.is_success(),
        "gateway control returned {status}: {}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).context("failed to decode gateway scheduled dispatch result")
}

fn scheduled_outcome_from_gateway(result: GatewayCommandDispatchResult) -> ScheduledTargetOutcome {
    if !result.accepted {
        let message =
            target_message_from_outputs(&result.outputs, &result.message, "rejected_by_agent");
        return ScheduledTargetOutcome {
            client_id: result.client_id,
            status: "rejected_by_agent".to_string(),
            exit_code: None,
            command_version: Some(result.command_version),
            accepted: false,
            message,
            outputs: result.outputs,
        };
    }
    let final_output = result.outputs.iter().rev().find(|output| output.done);
    let exit_code = final_output.and_then(|output| output.exit_code);
    let status = if final_output.is_some_and(output_indicates_timeout) {
        "timed_out"
    } else if final_output.is_some_and(output_indicates_canceled) {
        "canceled"
    } else {
        match exit_code {
            Some(0) => "completed",
            Some(_) => "failed",
            None => "accepted",
        }
    };
    let message = if target_status_needs_reason(status) {
        target_message_from_outputs(&result.outputs, &result.message, status)
    } else {
        result.message
    };
    ScheduledTargetOutcome {
        client_id: result.client_id,
        status: status.to_string(),
        exit_code,
        command_version: Some(result.command_version),
        accepted: true,
        message,
        outputs: result.outputs,
    }
}

fn scheduled_protocol_mismatch_reason(
    outcome: &ScheduledTargetOutcome,
    expected_command_version: u16,
) -> Option<String> {
    if outcome
        .command_version
        .is_some_and(|seen| seen < expected_command_version)
    {
        return Some("agent_returned_lower_command_version".to_string());
    }
    if outcome.message == "unsupported_command_version" {
        return Some("agent_rejected_unsupported_command_version".to_string());
    }
    outcome.outputs.iter().find_map(|output| {
        if output.stream != OutputStream::Status {
            return None;
        }
        let value = serde_json::from_slice::<serde_json::Value>(&output.data).ok()?;
        let kind = value.get("type").and_then(serde_json::Value::as_str)?;
        if kind == "unsupported_command_version" {
            return Some("agent_rejected_unsupported_command_version".to_string());
        }
        let response_version = value
            .get("command_version")
            .and_then(serde_json::Value::as_u64)
            .and_then(|version| u16::try_from(version).ok())?;
        (response_version < expected_command_version)
            .then(|| "agent_returned_lower_command_version".to_string())
    })
}

async fn mark_scheduled_agent_stale(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    dispatch: &ScheduledDispatch,
    outcome: &ScheduledTargetOutcome,
    reason: &str,
) -> Result<()> {
    let prior = sqlx::query(
        r#"
        SELECT status, internal_build_number
        FROM clients
        WHERE id = $1 AND hidden_at IS NULL
        FOR UPDATE
        "#,
    )
    .bind(&outcome.client_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(prior) = prior else {
        return Ok(());
    };
    let from_status: String = prior.try_get("status")?;
    let internal_build_number = prior.try_get::<i64, _>("internal_build_number")?.max(1);
    sqlx::query(
        r#"
        UPDATE clients
        SET
            status = 'stale',
            stale_since = COALESCE(stale_since, now()),
            stale_reason = $2,
            stale_build_number = COALESCE(stale_build_number, internal_build_number)
        WHERE id = $1 AND hidden_at IS NULL
        "#,
    )
    .bind(&outcome.client_id)
    .bind(reason)
    .execute(&mut **tx)
    .await?;
    if from_status != "stale" {
        let metadata = serde_json::json!({
            "reason": reason,
            "schedule_id": dispatch.schedule_id,
            "job_id": dispatch.job_id,
            "client_id": outcome.client_id,
            "command_type": scheduled_command_type_label(&dispatch.operation, "unknown"),
            "internal_build_number": internal_build_number,
            "response_command_version": outcome.command_version,
            "message": outcome.message,
        });
        sqlx::query(
            r#"
            INSERT INTO client_status_history (
                id, client_id, from_status, to_status, reason, metadata
            )
            VALUES ($1, $2, $3, 'stale', $4, $5)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&outcome.client_id)
        .bind(&from_status)
        .bind(reason)
        .bind(metadata.clone())
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO audit_logs (
                id, actor_id, action, target, command_hash, metadata
            )
            VALUES ($1, NULL, 'agent.status_stale', $2, $3, $4)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(format!("client:{}", outcome.client_id))
        .bind(&dispatch.command_hash)
        .bind(metadata)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn record_scheduled_dispatch_outcomes(
    pool: &PgPool,
    dispatch: &ScheduledDispatch,
    outcomes: Vec<ScheduledTargetOutcome>,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let mut statuses = Vec::new();
    for outcome in &outcomes {
        statuses.push(outcome.status.clone());
        let stale_reason = scheduled_protocol_mismatch_reason(
            outcome,
            job_command_protocol_version(&dispatch.operation),
        );
        let message = if let Some(reason) = stale_reason.as_deref() {
            stale_target_message(&outcome.message, reason)
        } else {
            outcome.message.clone()
        };
        if let Some(reason) = stale_reason {
            mark_scheduled_agent_stale(&mut tx, dispatch, outcome, &reason).await?;
        }
        sqlx::query(
            r#"
            UPDATE job_targets
            SET
                status = $3,
                message = $4,
                exit_code = $5,
                started_at = COALESCE(started_at, now()),
                completed_at = CASE
                    WHEN $3 IN ('completed', 'failed', 'timed_out', 'canceled', 'rejected_by_agent', 'dispatch_failed') THEN now()
                    ELSE completed_at
                END
            WHERE job_id = $1 AND client_id = $2
            "#,
        )
        .bind(dispatch.job_id)
        .bind(&outcome.client_id)
        .bind(&outcome.status)
        .bind(&message)
        .bind(outcome.exit_code)
        .execute(&mut *tx)
        .await?;
        for (seq, output) in outcome.outputs.iter().enumerate() {
            sqlx::query(
                r#"
                INSERT INTO job_outputs (job_id, client_id, seq, stream, data, exit_code, done)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                ON CONFLICT (job_id, client_id, seq) DO UPDATE
                SET stream = EXCLUDED.stream,
                    data = EXCLUDED.data,
                    exit_code = EXCLUDED.exit_code,
                    done = EXCLUDED.done
                "#,
            )
            .bind(dispatch.job_id)
            .bind(&outcome.client_id)
            .bind(seq as i32)
            .bind(output_stream_name(output.stream))
            .bind(&output.data)
            .bind(output.exit_code)
            .bind(output.done)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            r#"
            INSERT INTO audit_logs (
                id, actor_id, action, target, command_hash, metadata
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(dispatch.actor_id)
        .bind("schedule.target_result")
        .bind(format!(
            "job:{}:client:{}",
            dispatch.job_id, outcome.client_id
        ))
        .bind(&dispatch.command_hash)
        .bind(serde_json::json!({
            "schedule_id": dispatch.schedule_id,
            "job_id": dispatch.job_id,
            "client_id": outcome.client_id,
            "status": outcome.status,
            "accepted": outcome.accepted,
            "message": message,
        }))
        .execute(&mut *tx)
        .await?;
    }
    let job_status = aggregate_job_status(&statuses, dispatch.targets.len());
    sqlx::query(
        r#"
        UPDATE jobs
        SET status = $2, completed_at = now()
        WHERE id = $1
        "#,
    )
    .bind(dispatch.job_id)
    .bind(job_status)
    .execute(&mut *tx)
    .await?;
    record_schedule_dispatched_webhook_event(&mut tx, dispatch, &outcomes, job_status).await?;
    tx.commit().await?;
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

async fn record_schedule_dispatched_webhook_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    dispatch: &ScheduledDispatch,
    outcomes: &[ScheduledTargetOutcome],
    job_status: &str,
) -> Result<()> {
    let event_id = format!(
        "schedule:{}:job:{}:dispatched",
        dispatch.schedule_id, dispatch.job_id
    );
    let mut predicates = vec![
        "schedule.dispatched".to_string(),
        format!("schedule.id:{}", dispatch.schedule_id),
        format!("schedule.name:{}", dispatch.schedule_name),
        format!("job.status:{job_status}"),
        format!("job.status.become_{job_status}"),
        format!("job.type:{}", dispatch.command_type),
    ];
    for outcome in outcomes {
        predicates.push(format!("job.target.status:{}", outcome.status));
    }
    predicates.sort();
    predicates.dedup();
    insert_webhook_event_in_tx(
        tx,
        "schedule.dispatched",
        &event_id,
        &predicates,
        &dispatch.targets,
        serde_json::json!({
            "event": {
                "kind": "schedule.dispatched",
                "id": event_id,
                "predicates": &predicates,
            },
            "schedule": {
                "id": dispatch.schedule_id,
                "name": &dispatch.schedule_name,
                "command_type": &dispatch.command_type,
                "selector_expression": &dispatch.selector_expression,
                "target_ids": &dispatch.targets,
            },
            "job": {
                "id": dispatch.job_id,
                "status": job_status,
                "type": &dispatch.command_type,
                "source_schedule_id": dispatch.schedule_id,
                "target_count": dispatch.targets.len(),
                "targets": outcomes
                    .iter()
                    .map(|outcome| serde_json::json!({
                        "client_id": &outcome.client_id,
                        "status": &outcome.status,
                        "accepted": outcome.accepted,
                        "exit_code": outcome.exit_code,
                        "message": &outcome.message,
                    }))
                    .collect::<Vec<_>>(),
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

fn stale_target_message(message: &str, reason: &str) -> String {
    let trimmed = message.trim();
    if trimmed.to_ascii_lowercase().contains("stale") {
        return trimmed.to_string();
    }
    if trimmed.is_empty() || trimmed == reason {
        return format!("stale: {reason}");
    }
    format!("stale: {reason}; {trimmed}")
}

fn target_status_needs_reason(status: &str) -> bool {
    !matches!(status, "accepted" | "completed")
}

fn target_message_from_outputs(outputs: &[CommandOutput], fallback: &str, status: &str) -> String {
    if let Some(message) = outputs.iter().rev().find_map(status_output_message) {
        return message;
    }
    let trimmed = fallback.trim();
    if trimmed.is_empty() || trimmed == "accepted" {
        status.to_string()
    } else {
        trimmed.to_string()
    }
}

fn status_output_message(output: &CommandOutput) -> Option<String> {
    if output.stream != OutputStream::Status {
        return None;
    }
    let value = serde_json::from_slice::<serde_json::Value>(&output.data).ok()?;
    status_value_message(&value)
}

fn status_value_message(value: &serde_json::Value) -> Option<String> {
    let kind = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let primary = ["message", "error", "reason", "hint", "status"]
        .iter()
        .find_map(|field| {
            value
                .get(*field)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        });
    match (kind, primary) {
        (Some(kind), Some(primary)) if kind != primary => Some(format!("{kind}: {primary}")),
        (Some(kind), _) => Some(kind.to_string()),
        (_, Some(primary)) => Some(primary.to_string()),
        _ => None,
    }
}

fn output_indicates_canceled(output: &CommandOutput) -> bool {
    if output.stream != OutputStream::Status {
        return false;
    }
    serde_json::from_slice::<serde_json::Value>(&output.data)
        .ok()
        .and_then(|value| {
            value
                .get("type")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .is_some_and(|kind| kind == "command_canceled")
}

fn output_indicates_timeout(output: &CommandOutput) -> bool {
    if output.stream != OutputStream::Status {
        return false;
    }
    serde_json::from_slice::<serde_json::Value>(&output.data)
        .ok()
        .and_then(|value| {
            value
                .get("type")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .is_some_and(|kind| kind == "command_timeout")
}

fn aggregate_job_status(target_statuses: &[String], target_count: usize) -> &'static str {
    let completed = target_statuses
        .iter()
        .filter(|status| status.as_str() == "completed")
        .count();
    if target_count > 0 && completed == target_count {
        return "completed";
    }
    if completed > 0 {
        return "partially_completed";
    }
    if target_statuses
        .iter()
        .any(|status| status.as_str() == "accepted")
    {
        return "accepted";
    }
    if target_statuses
        .iter()
        .any(|status| status.as_str() == "timed_out")
    {
        return "timed_out";
    }
    if target_statuses
        .iter()
        .any(|status| status.as_str() == "canceled")
    {
        return "canceled";
    }
    if target_statuses
        .iter()
        .any(|status| matches!(status.as_str(), "failed" | "rejected_by_agent"))
    {
        return "failed";
    }
    "dispatch_failed"
}

fn output_stream_name(stream: OutputStream) -> &'static str {
    match stream {
        OutputStream::Stdout => "stdout",
        OutputStream::Stderr => "stderr",
        OutputStream::Pty => "pty",
        OutputStream::Status => "status",
    }
}

fn scheduled_command_type_label(command: &JobCommand, fallback: &str) -> String {
    match command {
        JobCommand::Shell { pty: true, .. } => "shell_pty",
        JobCommand::Shell { .. } => "shell_argv",
        JobCommand::ShellScript { .. } => "shell_script",
        JobCommand::Backup { .. } => "backup",
        JobCommand::Restore { .. } => "restore",
        JobCommand::RestoreRollback { .. } => "restore_rollback",
        JobCommand::NetworkApply { .. } => "network_apply",
        JobCommand::NetworkOspfCostUpdate { .. } => "network_ospf_cost_update",
        JobCommand::NetworkRollback { .. } => "network_rollback",
        JobCommand::NetworkStatus { .. } => "network_status",
        JobCommand::NetworkInterfaces => "network_interfaces",
        JobCommand::NetworkProbe { .. } => "network_probe",
        JobCommand::NetworkSpeedTest { .. } => "network_speed_test",
        JobCommand::UpdateAgent { .. } => "agent_update",
        JobCommand::AgentUpdateActivate { .. } => "agent_update_activate",
        JobCommand::AgentUpdateRollback { .. } => "agent_update_rollback",
        JobCommand::AgentUpdateCheck { .. } => "agent_update_check",
        _ => fallback,
    }
    .to_string()
}

fn decode_server_signing_key(value: &str) -> Result<SigningKey> {
    let bytes = hex::decode(value.trim()).context("invalid worker server signing key hex")?;
    let bytes: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("worker server signing key must be 32 bytes"))?;
    Ok(SigningKey::from_bytes(&bytes))
}

async fn advance_schedule_after_success(
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
            failure_count = 0,
            last_error = NULL,
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

async fn resolve_schedule_targets(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    schedule: &DueSchedule,
) -> Result<Vec<String>> {
    let expression = parse_expression(&schedule.selector_expression)
        .map_err(anyhow::Error::msg)?
        .context("schedule selector expression is empty")?;
    let rows = sqlx::query(
        r#"
        SELECT
            c.id,
            c.display_name,
            c.status,
            c.registration_ip::text AS registration_ip,
            c.last_ip::text AS last_ip,
            c.last_seen_at::text AS last_seen_at,
            c.internal_build_number,
            c.stale_since::text AS stale_since,
            c.stale_reason,
            COALESCE(array_agg(t.name ORDER BY t.name) FILTER (WHERE t.name IS NOT NULL), ARRAY[]::TEXT[]) AS tags
        FROM clients c
        LEFT JOIN client_tags ct ON ct.client_id = c.id
        LEFT JOIN tags t ON t.id = ct.tag_id
        WHERE c.hidden_at IS NULL
        GROUP BY c.id, c.display_name, c.status, c.registration_ip, c.last_ip, c.last_seen_at, c.internal_build_number, c.stale_since, c.stale_reason
        ORDER BY c.id
        "#,
    )
    .fetch_all(&mut **tx)
    .await?;
    let mut targets = Vec::new();
    for row in rows {
        let id: String = row.try_get("id")?;
        let context = ExpressionContext::for_vps(VpsMetadata {
            id: id.clone(),
            display_name: row.try_get("display_name")?,
            status: row.try_get("status")?,
            registration_ip: row.try_get("registration_ip")?,
            last_ip: row.try_get("last_ip")?,
            last_seen_at: row.try_get("last_seen_at")?,
            internal_build_number: Some(
                row.try_get::<i64, _>("internal_build_number")?.max(1) as u64
            ),
            stale_since: row.try_get("stale_since")?,
            stale_reason: row.try_get("stale_reason")?,
            tags: row.try_get("tags")?,
            extra: None,
        });
        if expression_matches(&context, &expression) {
            targets.push(id);
        }
    }
    Ok(targets)
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
}
