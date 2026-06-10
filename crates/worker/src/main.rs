use std::{path::PathBuf, str::FromStr, time::Duration};

use anyhow::{Context, Result};
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
use vpsman_common::{expression_matches, parse_expression, ExpressionContext, VpsMetadata};
use vpsman_common::{payload_hash, JobCommand};

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
        .unwrap_or_else(|| format!("vpsman-worker-{}", std::process::id()));
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
    let schedule_dispatch_config = ScheduleDispatchConfig::new(args.schedule_command_timeout_secs);
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
            materialize_due_schedule(
                &mut tx,
                &schedule,
                run_index,
                run_count,
                dispatch_config.timeout_secs,
            )
            .await?;
        }
        advance_schedule_after_success(&mut tx, &schedule, run_count).await?;
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
}

impl ScheduleDispatchConfig {
    fn new(timeout_secs: u64) -> Self {
        Self {
            timeout_secs: timeout_secs.clamp(1, 3600),
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
    timeout_secs: u64,
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
    let mut fingerprint_targets = targets.clone();
    fingerprint_targets.sort();
    let request_fingerprint = payload_hash(&serde_json::to_vec(&serde_json::json!({
        "selector_expression": schedule.selector_expression.trim(),
        "command_type": &command_type,
        "operation_payload_hash": &command_hash,
        "targets": fingerprint_targets,
        "timeout_secs": timeout_secs.clamp(1, 3600),
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
    .bind(&request_fingerprint)
    .bind(timeout_secs.clamp(1, 3600) as i64)
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
        "fixed_targets": &targets,
        "selector_expression": &schedule.selector_expression,
        "catch_up_policy": &schedule.catch_up_policy,
        "catch_up_run_index": run_index + 1,
        "catch_up_run_count": run_count,
        "retry_delay_secs": schedule.retry_delay_secs,
        "max_failures": schedule.max_failures,
        "failure_count_before_run": schedule.failure_count,
        "last_error_before_run": &schedule.last_error,
        "reason": "saved schedule intent was previously privilege-unlocked; worker materialized a durable queued job from the fixed target snapshot",
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

    Ok(!targets.is_empty())
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
