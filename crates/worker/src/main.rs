use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use clap::Parser;
use serde_json::Value;
use sqlx::{postgres::PgPoolOptions, types::Json as SqlJson, PgPool, Row};
use tokio::time;
use tracing::{debug, info, warn};
use uuid::Uuid;
use vpsman_common::payload_hash;

mod alert_notifications;
mod backup_policy_retention;
mod rollout_automation;
mod worker_leases;

use alert_notifications::{
    process_alert_notifications, AlertNotificationWorkerConfig, AlertNotificationWorkerRun,
};
use backup_policy_retention::{
    process_backup_policy_retention_prune, BackupPolicyRetentionPruneConfig,
    BackupPolicyRetentionPruneRun,
};
use rollout_automation::process_rollout_automation;
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
    let backup_policy_prune_config = BackupPolicyRetentionPruneConfig::new(
        args.backup_policy_prune_enabled,
        args.backup_policy_prune_limit,
        args.backup_policy_prune_dry_run,
        args.backup_policy_prune_include_disabled,
        args.backup_policy_prune_delete_objects,
        args.backup_policy_prune_object_store_dir.clone(),
    );
    info!(tick_secs = args.tick_secs, "worker started");
    if args.once {
        let schedules_processed =
            process_due_schedules_if_leader(&pool, 25, &worker_id, args.worker_lease_secs).await?;
        let alert_notifications = process_alert_notifications_if_leader(
            &pool,
            alert_notification_config,
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
    loop {
        ticker.tick().await;
        match process_due_schedules_if_leader(&pool, 25, &worker_id, args.worker_lease_secs).await {
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
    }
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
) -> Result<usize> {
    let acquired = acquire_worker_lease(pool, "schedules", worker_id, lease_secs).await?;
    if !acquired {
        debug!(
            worker_id,
            "skipped due schedules because another worker holds the lease"
        );
        return Ok(0);
    }
    process_due_schedules(pool, limit).await
}

async fn process_due_schedules(pool: &PgPool, limit: i64) -> Result<usize> {
    let mut tx = pool.begin().await?;
    let due_count: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*)
        FROM schedules
        WHERE enabled = TRUE AND next_run_at <= now()
        "#,
    )
    .fetch_one(&mut *tx)
    .await?;
    let rows = sqlx::query(
        r#"
        SELECT id
        FROM schedules
        WHERE enabled = TRUE AND next_run_at <= now()
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
        materialized += process_due_schedule(pool, schedule_id).await?;
    }
    Ok(materialized)
}

async fn process_due_schedule(pool: &PgPool, schedule_id: Uuid) -> Result<usize> {
    let result: Result<usize> = async {
        let mut tx = pool.begin().await?;
        let Some(row) = sqlx::query(
        r#"
        SELECT
            id,
            actor_id,
            name,
            operation,
            target_clients,
            target_tags,
            interval_secs,
            catch_up_policy,
            catch_up_limit,
            retry_delay_secs,
            max_failures,
            failure_count,
            last_error,
            GREATEST(
                1,
                FLOOR(EXTRACT(EPOCH FROM (now() - next_run_at)) / GREATEST(interval_secs, 1))::bigint + 1
            ) AS due_occurrences
        FROM schedules
        WHERE id = $1
          AND enabled = TRUE
          AND next_run_at <= now()
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
            target_clients: row.try_get("target_clients")?,
            target_tags: row.try_get("target_tags")?,
            interval_secs: row.try_get("interval_secs")?,
            catch_up_policy: row.try_get("catch_up_policy")?,
            catch_up_limit: row.try_get("catch_up_limit")?,
            retry_delay_secs: row.try_get("retry_delay_secs")?,
            max_failures: row.try_get("max_failures")?,
            failure_count: row.try_get("failure_count")?,
            last_error: row.try_get("last_error")?,
        };
        let due_occurrences = row.try_get("due_occurrences")?;
        let run_count = catch_up_run_count(&schedule, due_occurrences);
        for run_index in 0..run_count {
            materialize_due_schedule(&mut tx, &schedule, run_index, run_count).await?;
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
    target_clients: Vec<String>,
    target_tags: Vec<String>,
    interval_secs: i64,
    catch_up_policy: String,
    catch_up_limit: i32,
    retry_delay_secs: i64,
    max_failures: i32,
    failure_count: i32,
    last_error: Option<String>,
}

async fn materialize_due_schedule(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    schedule: &DueSchedule,
    run_index: i64,
    run_count: i64,
) -> Result<()> {
    let targets = resolve_schedule_targets(tx, schedule).await?;
    let operation_bytes = serde_json::to_vec(&schedule.operation)?;
    let command_hash = payload_hash(&operation_bytes);
    let operation_type = schedule
        .operation
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let job_id = Uuid::new_v4();
    let status = if targets.is_empty() {
        "schedule_no_targets"
    } else {
        "approval_required"
    };
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
    .bind(format!("scheduled_{operation_type}"))
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
            INSERT INTO job_targets (job_id, client_id, status)
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(job_id)
        .bind(client_id)
        .bind("approval_required")
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
        "schedule.due_approval_required"
    })
    .bind(format!("schedule:{}", schedule.id))
    .bind(&command_hash)
    .bind(serde_json::json!({
        "schedule_id": schedule.id,
        "schedule_name": schedule.name,
        "operation_type": operation_type,
        "job_id": job_id,
        "resolved_targets": &targets,
        "target_clients": &schedule.target_clients,
        "target_tags": &schedule.target_tags,
        "catch_up_policy": &schedule.catch_up_policy,
        "catch_up_run_index": run_index + 1,
        "catch_up_run_count": run_count,
        "retry_delay_secs": schedule.retry_delay_secs,
        "max_failures": schedule.max_failures,
        "failure_count_before_run": schedule.failure_count,
        "last_error_before_run": &schedule.last_error,
        "reason": "server cannot generate fresh super-password proof; operator approval is required before dispatch",
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn advance_schedule_after_success(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    schedule: &DueSchedule,
    run_count: i64,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE schedules
        SET
            last_run_at = now(),
            next_run_at = CASE
                WHEN $3 = 'skip_missed' THEN now() + ($2 * interval '1 second')
                ELSE next_run_at + (($4::bigint * $2)::text || ' seconds')::interval
            END,
            failure_count = 0,
            last_error = NULL,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(schedule.id)
    .bind(schedule.interval_secs)
    .bind(&schedule.catch_up_policy)
    .bind(run_count)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn record_schedule_failure(pool: &PgPool, schedule_id: Uuid, error: &str) -> Result<()> {
    let bounded_error = truncate_schedule_error(error);
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
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(());
    };
    let actor_id: Option<Uuid> = row.try_get("actor_id")?;
    let failure_count: i32 = row.try_get("failure_count")?;
    let max_failures: i32 = row.try_get("max_failures")?;
    let enabled: bool = row.try_get("enabled")?;
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
        "schedule_name": row.try_get::<String, _>("name")?,
        "failure_count": failure_count,
        "max_failures": max_failures,
        "retry_delay_secs": row.try_get::<i64, _>("retry_delay_secs")?,
        "next_run_at": row.try_get::<String, _>("next_run_at")?,
        "disabled": !enabled,
        "error": bounded_error,
    }))
    .execute(pool)
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

fn truncate_schedule_error(error: &str) -> String {
    error.chars().take(1024).collect()
}

async fn resolve_schedule_targets(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    schedule: &DueSchedule,
) -> Result<Vec<String>> {
    let rows = sqlx::query(
        r#"
        WITH explicit_targets AS (
            SELECT unnest($1::TEXT[]) AS client_id
        ),
        tag_targets AS (
            SELECT ct.client_id
            FROM client_tags ct
            JOIN tags t ON t.id = ct.tag_id
            WHERE t.name = ANY($2::TEXT[])
        )
        SELECT DISTINCT client_id
        FROM (
            SELECT client_id FROM explicit_targets
            UNION ALL
            SELECT client_id FROM tag_targets
        ) targets
        WHERE client_id IN (SELECT id FROM clients)
        ORDER BY client_id
        "#,
    )
    .bind(&schedule.target_clients)
    .bind(&schedule.target_tags)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| row.try_get("client_id").map_err(Into::into))
        .collect()
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
            target_clients: Vec::new(),
            target_tags: vec!["edge".to_string()],
            interval_secs: 60,
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
    fn schedule_error_is_bounded() {
        let error = "x".repeat(1200);
        assert_eq!(truncate_schedule_error(&error).len(), 1024);
    }
}
