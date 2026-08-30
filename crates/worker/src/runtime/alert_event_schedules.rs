use anyhow::{ensure, Result};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{types::Json as SqlJson, PgPool, Postgres, Row, Transaction};
use tracing::warn;
use uuid::Uuid;
use vpsman_common::{
    encode_json, expression_matches, parse_and_validate_alert_event_expression, payload_hash,
    render_alert_event_job_command, ExpressionContext, JobCommand,
};

use super::{
    actor_authorized_in_tx, materialize_due_schedule, DueSchedule, ScheduleDispatchConfig,
    ScheduleMaterializationContext,
};

const MAX_ALERT_EVENT_LINEAGE: usize = 16;
const EVENT_SCHEDULE_COMPONENT: &str = "alert-event-schedule-worker";
const EVENT_SCHEDULE_REQUIRED_SCOPES: &[&str] = &[
    "fleet:read",
    "backups:read",
    "jobs:write",
    "schedules:write",
];

#[derive(Clone, Debug)]
struct LifecycleEvent {
    event_seq: i64,
    id: Uuid,
    episode_id: Uuid,
    trigger_generation: i64,
    edge_kind: String,
    event_id: String,
    event_predicates: Vec<String>,
    subject_client_ids: Vec<String>,
    payload: Value,
    causation_id: Option<Uuid>,
    schedule_lineage: Vec<Uuid>,
    occurred_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
struct EventSchedule {
    id: Uuid,
    actor_id: Option<Uuid>,
    name: String,
    definition_revision: i64,
    event_expression: String,
    event_argv_template: Option<Vec<String>>,
    target_client_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum LineageDecision {
    Dispatch(Vec<Uuid>),
    Cycle,
    Overflow,
}

pub(crate) async fn process_alert_event_schedules(
    pool: &PgPool,
    limit: i64,
    dispatch_config: &ScheduleDispatchConfig,
) -> Result<usize> {
    let event_seq_through = current_alert_lifecycle_frontier(pool).await?;
    process_alert_event_schedules_through(pool, limit, dispatch_config, event_seq_through).await
}

async fn process_alert_event_schedules_through(
    pool: &PgPool,
    limit: i64,
    dispatch_config: &ScheduleDispatchConfig,
    event_seq_through: i64,
) -> Result<usize> {
    let page_limit = limit.clamp(1, 100);
    loop {
        let ingested =
            ingest_alert_lifecycle_events_through(pool, page_limit, event_seq_through).await?;
        if ingested < page_limit as usize {
            break;
        }
        // The page bounds one transaction, not the amount of due work handled
        // per wake. Yield between pages so unrelated owners can use the pool.
        tokio::task::yield_now().await;
    }

    let mut attempted_receipts = Vec::new();
    let mut dispatched = 0_usize;
    loop {
        let receipt_ids = load_ready_alert_event_receipt_ids(
            pool,
            page_limit,
            &attempted_receipts,
            event_seq_through,
        )
        .await?;
        if receipt_ids.is_empty() {
            break;
        }
        for receipt_id in receipt_ids {
            attempted_receipts.push(receipt_id);
            match dispatch_alert_event_receipt(pool, receipt_id, dispatch_config).await {
                Ok(true) => dispatched += 1,
                Ok(false) => {}
                Err(error) => {
                    warn!(
                        %receipt_id,
                        error = %error,
                        "alert-event schedule dispatch failed transiently; receipt remains pending"
                    );
                }
            }
        }
        // Every receipt is attempted at most once in this drain. Pending
        // dependency/error receipts remain durable for the next recovery wake,
        // while later independent receipts are never hidden behind the page.
        tokio::task::yield_now().await;
    }
    Ok(dispatched)
}

async fn current_alert_lifecycle_frontier(pool: &PgPool) -> Result<i64> {
    Ok(
        sqlx::query_scalar(
            "SELECT COALESCE(max(event_seq), 0)::bigint FROM alert_lifecycle_events",
        )
        .fetch_one(pool)
        .await?,
    )
}

async fn load_ready_alert_event_receipt_ids(
    pool: &PgPool,
    limit: i64,
    attempted_receipts: &[Uuid],
    event_seq_through: i64,
) -> Result<Vec<Uuid>> {
    let receipt_ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT receipt.id
        FROM schedule_event_receipts receipt
        WHERE receipt.status = 'pending'
          AND NOT (receipt.id = ANY($2::uuid[]))
          AND receipt.event_seq <= $3
          AND (
              receipt.event_kind <> 'alert.resolved'
              OR NOT EXISTS (
                  SELECT 1
                  FROM schedule_event_receipts trigger_receipt
                  WHERE trigger_receipt.schedule_id = receipt.schedule_id
                    AND trigger_receipt.episode_id = receipt.episode_id
                    AND trigger_receipt.trigger_generation = receipt.trigger_generation
                    AND trigger_receipt.event_kind = 'alert.triggered'
              )
              OR EXISTS (
                  SELECT 1
                  FROM schedule_event_receipts trigger_receipt
                  LEFT JOIN jobs trigger_job ON trigger_job.id = trigger_receipt.job_id
                  WHERE trigger_receipt.schedule_id = receipt.schedule_id
                    AND trigger_receipt.episode_id = receipt.episode_id
                    AND trigger_receipt.trigger_generation = receipt.trigger_generation
                    AND trigger_receipt.event_kind = 'alert.triggered'
                    AND (
                        trigger_receipt.status NOT IN ('pending', 'dispatched')
                        OR (
                            trigger_receipt.status = 'dispatched'
                            AND (
                                trigger_receipt.job_id IS NULL
                                OR trigger_job.completed_at IS NOT NULL
                            )
                        )
                    )
              )
          )
        ORDER BY receipt.event_seq ASC, receipt.schedule_id ASC
        LIMIT $1
        "#,
    )
    .bind(limit.clamp(1, 100))
    .bind(attempted_receipts)
    .bind(event_seq_through)
    .fetch_all(pool)
    .await?;
    Ok(receipt_ids)
}

#[cfg(test)]
async fn ingest_alert_lifecycle_events(pool: &PgPool, limit: i64) -> Result<usize> {
    let event_seq_through = current_alert_lifecycle_frontier(pool).await?;
    ingest_alert_lifecycle_events_through(pool, limit, event_seq_through).await
}

async fn ingest_alert_lifecycle_events_through(
    pool: &PgPool,
    limit: i64,
    event_seq_through: i64,
) -> Result<usize> {
    let mut tx = pool.begin().await?;
    let page_limit = limit.clamp(1, 100);
    sqlx::query(
        r#"
        INSERT INTO alert_lifecycle_consumer_receipts (
            consumer_kind, event_seq, status
        )
        SELECT 'schedule', lifecycle.event_seq, 'pending'
        FROM alert_lifecycle_events lifecycle
        WHERE lifecycle.event_seq <= $2
          AND NOT EXISTS (
            SELECT 1
            FROM alert_lifecycle_consumer_receipts receipt
            WHERE receipt.consumer_kind='schedule'
              AND receipt.event_seq=lifecycle.event_seq
        )
        ORDER BY lifecycle.event_seq
        LIMIT $1
        ON CONFLICT (consumer_kind,event_seq) DO NOTHING
        "#,
    )
    .bind(page_limit)
    .bind(event_seq_through)
    .execute(&mut *tx)
    .await?;
    let claim_id = Uuid::new_v4();
    let rows = sqlx::query(
        r#"
        WITH candidate AS (
            SELECT receipt.event_seq
            FROM alert_lifecycle_consumer_receipts receipt
            WHERE receipt.consumer_kind='schedule'
              AND receipt.status IN ('pending','failed')
              AND receipt.event_seq <= $2
            ORDER BY receipt.event_seq
            FOR UPDATE SKIP LOCKED
            LIMIT $1
        ), claimed AS (
            UPDATE alert_lifecycle_consumer_receipts receipt
            SET status='in_progress', claim_id=$3,
                attempt_count=receipt.attempt_count+1,
                error=NULL, updated_at=clock_timestamp()
            FROM candidate
            WHERE receipt.consumer_kind='schedule'
              AND receipt.event_seq=candidate.event_seq
            RETURNING receipt.event_seq
        )
        SELECT
            lifecycle.event_seq, lifecycle.id, lifecycle.episode_id,
            lifecycle.trigger_generation, lifecycle.edge_kind,
            lifecycle.event_id, lifecycle.event_predicates,
            lifecycle.subject_client_ids, lifecycle.payload,
            lifecycle.causation_id, lifecycle.schedule_lineage,
            lifecycle.occurred_at, lifecycle.created_at
        FROM claimed
        JOIN alert_lifecycle_events lifecycle USING (event_seq)
        ORDER BY lifecycle.event_seq ASC
        "#,
    )
    .bind(page_limit)
    .bind(event_seq_through)
    .bind(claim_id)
    .fetch_all(&mut *tx)
    .await?;

    let events = rows
        .into_iter()
        .map(lifecycle_event_from_row)
        .collect::<Result<Vec<_>>>()?;
    for event in &events {
        ingest_lifecycle_event_in_tx(&mut tx, event).await?;
        let acknowledged = sqlx::query(
            r#"
            UPDATE alert_lifecycle_consumer_receipts
            SET status='completed', claim_id=NULL,
                error=NULL, updated_at=clock_timestamp()
            WHERE consumer_kind='schedule' AND event_seq=$1
              AND status='in_progress' AND claim_id=$2
            "#,
        )
        .bind(event.event_seq)
        .bind(claim_id)
        .execute(&mut *tx)
        .await?;
        ensure!(
            acknowledged.rows_affected() == 1,
            "schedule_lifecycle_receipt_claim_lost"
        );
    }
    tx.commit().await?;
    Ok(events.len())
}

async fn ingest_lifecycle_event_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    event: &LifecycleEvent,
) -> Result<()> {
    ensure!(
        matches!(
            event.edge_kind.as_str(),
            "alert.triggered" | "alert.resolved"
        ),
        "alert_lifecycle_event_kind_invalid"
    );

    let schedules = load_eligible_event_schedules(tx, event.occurred_at, event.created_at).await?;
    for schedule in schedules {
        if !actor_authorized_in_tx(
            tx,
            schedule.actor_id,
            "operator",
            EVENT_SCHEDULE_REQUIRED_SCOPES,
        )
        .await?
        {
            disable_revoked_event_schedule_in_tx(tx, &schedule).await?;
            continue;
        }
        let expression = match parse_and_validate_alert_event_expression(&schedule.event_expression)
        {
            Ok(expression) => expression,
            Err(error) => {
                disable_invalid_event_schedule_in_tx(tx, &schedule, &error).await?;
                continue;
            }
        };
        let expression_context = expression_context_for_lifecycle_event(event);
        if !expression_matches(&expression_context, &expression) {
            continue;
        }

        let source_payload_hash = payload_hash(&encode_json(&event.payload)?);
        let causation_id = event.causation_id.unwrap_or(event.id);
        let lineage = extend_schedule_lineage(&event.schedule_lineage, schedule.id);
        let (status, status_reason, dispatched_lineage, operation, operation_hash, error) =
            match lineage {
                LineageDecision::Cycle => (
                    "skipped",
                    Some("schedule_lineage_cycle"),
                    event.schedule_lineage.clone(),
                    None,
                    None,
                    None,
                ),
                LineageDecision::Overflow => (
                    "lineage_overflow",
                    Some("lineage_overflow"),
                    event.schedule_lineage.clone(),
                    None,
                    None,
                    None,
                ),
                LineageDecision::Dispatch(dispatched_lineage) => {
                    let template_context = template_context_for_lifecycle_event(event, &schedule);
                    match render_alert_event_job_command(
                        schedule.event_argv_template.as_deref(),
                        &template_context,
                    ) {
                        Ok((operation, hash)) => (
                            "pending",
                            Some("matched_alert_lifecycle_event"),
                            dispatched_lineage,
                            Some(operation),
                            Some(hash),
                            None,
                        ),
                        Err(error) => (
                            "failed",
                            Some("event_argv_render_failed"),
                            dispatched_lineage,
                            None,
                            None,
                            Some(error.to_string()),
                        ),
                    }
                }
            };
        let rendered_operation = operation.as_ref().map(serde_json::to_value).transpose()?;
        let receipt_id = Uuid::new_v4();
        let inserted = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO schedule_event_receipts (
                id, schedule_id, definition_revision, event_seq, event_kind,
                event_id, episode_id, trigger_generation, edge_ordinal, status,
                status_reason, source_occurred_at, source_payload_hash,
                matched_subject_client_ids, fixed_target_client_ids, causation_id,
                source_schedule_lineage, dispatched_schedule_lineage,
                rendered_operation, rendered_operation_hash, error,
                actor_id, schedule_name
            )
            VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8,
                CASE WHEN $5 = 'alert.triggered' THEN 1 ELSE 2 END,
                $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20,
                $21, $22
            )
            ON CONFLICT (schedule_id, event_kind, event_id) DO NOTHING
            RETURNING id
            "#,
        )
        .bind(receipt_id)
        .bind(schedule.id)
        .bind(schedule.definition_revision)
        .bind(event.event_seq)
        .bind(&event.edge_kind)
        .bind(&event.event_id)
        .bind(event.episode_id)
        .bind(event.trigger_generation)
        .bind(status)
        .bind(status_reason)
        .bind(event.occurred_at)
        .bind(&source_payload_hash)
        .bind(&event.subject_client_ids)
        .bind(&schedule.target_client_ids)
        .bind(causation_id)
        .bind(&event.schedule_lineage)
        .bind(&dispatched_lineage)
        .bind(rendered_operation.map(SqlJson))
        .bind(&operation_hash)
        .bind(error.as_deref().map(truncate_error))
        .bind(schedule.actor_id)
        .bind(&schedule.name)
        .fetch_optional(&mut **tx)
        .await?;
        let Some(receipt_id) = inserted else {
            continue;
        };

        if status == "failed" {
            account_schedule_definition_failure_in_tx(
                tx,
                schedule.id,
                schedule.definition_revision,
                error.as_deref().unwrap_or("event_argv_render_failed"),
            )
            .await?;
        }
        if status == "pending" && event.edge_kind == "alert.resolved" {
            capture_resolve_dependencies_in_tx(
                tx,
                receipt_id,
                schedule.id,
                event.episode_id,
                event.trigger_generation,
            )
            .await?;
        }
        audit_receipt_created_in_tx(
            tx,
            receipt_id,
            &schedule,
            event,
            status,
            status_reason,
            &source_payload_hash,
            operation_hash.as_deref(),
            causation_id,
            &dispatched_lineage,
        )
        .await?;
    }
    Ok(())
}

async fn load_eligible_event_schedules(
    tx: &mut Transaction<'_, Postgres>,
    event_occurred_at: DateTime<Utc>,
    event_created_at: DateTime<Utc>,
) -> Result<Vec<EventSchedule>> {
    let rows = sqlx::query(
        r#"
        SELECT
            id, actor_id, name, definition_revision, event_expression,
            event_argv_template, target_client_ids
        FROM schedules
        WHERE trigger_kind = 'event'
          AND enabled = TRUE
          AND deleted_at IS NULL
          -- The immutable event creation instant is the schedule-generation
          -- boundary. Unlike a sequence high-water mark, it remains exact
          -- when two producers allocate sequence values and commit out of
          -- order; no producer/consumer barrier is required.
          AND event_armed_at <= $2::timestamptz
          AND (
              deferred_until IS NULL
              OR (
                  deferred_until <= clock_timestamp()
                  AND $1::timestamptz >= deferred_until
                  AND $2::timestamptz >= deferred_until
              )
          )
        ORDER BY id
        "#,
    )
    .bind(event_occurred_at)
    .bind(event_created_at)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(EventSchedule {
                id: row.try_get("id")?,
                actor_id: row.try_get("actor_id")?,
                name: row.try_get("name")?,
                definition_revision: row.try_get("definition_revision")?,
                event_expression: row.try_get("event_expression")?,
                event_argv_template: row
                    .try_get::<Option<SqlJson<Vec<String>>>, _>("event_argv_template")?
                    .map(|value| value.0),
                target_client_ids: row.try_get("target_client_ids")?,
            })
        })
        .collect()
}

async fn dispatch_alert_event_receipt(
    pool: &PgPool,
    receipt_id: Uuid,
    dispatch_config: &ScheduleDispatchConfig,
) -> Result<bool> {
    let Some(snapshot) = sqlx::query(
        r#"
        SELECT schedule_id, fixed_target_client_ids
        FROM schedule_event_receipts
        WHERE id = $1 AND status = 'pending'
        "#,
    )
    .bind(receipt_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(false);
    };
    let snapshot_schedule_id: Uuid = snapshot.try_get("schedule_id")?;
    let snapshot_targets: Vec<String> = snapshot.try_get("fixed_target_client_ids")?;

    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("vpsman:schedule-event-receipt:{receipt_id}"))
        .execute(&mut *tx)
        .await?;
    let Some(row) = sqlx::query(
        r#"
        SELECT
            receipt.id AS receipt_id,
            receipt.schedule_id,
            receipt.definition_revision AS receipt_definition_revision,
            receipt.event_seq,
            lifecycle.id AS lifecycle_event_id,
            receipt.event_kind,
            receipt.event_id,
            receipt.episode_id,
            receipt.trigger_generation,
            receipt.source_payload_hash,
            receipt.causation_id,
            receipt.dispatched_schedule_lineage,
            receipt.fixed_target_client_ids,
            receipt.rendered_operation,
            receipt.rendered_operation_hash,
            receipt.actor_id,
            operator.username AS actor_username,
            operator.role AS actor_role,
            receipt.schedule_name,
            schedule.max_failures,
            schedule.failure_count,
            schedule.last_error
        FROM schedule_event_receipts receipt
        JOIN schedules schedule ON schedule.id = receipt.schedule_id
        JOIN alert_lifecycle_events lifecycle ON lifecycle.event_seq = receipt.event_seq
        LEFT JOIN operators operator ON operator.id = receipt.actor_id
        WHERE receipt.id = $1 AND receipt.status = 'pending'
        FOR UPDATE OF receipt, schedule
        "#,
    )
    .bind(receipt_id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        tx.commit().await?;
        return Ok(false);
    };

    let schedule_id: Uuid = row.try_get("schedule_id")?;
    let receipt_revision: i64 = row.try_get("receipt_definition_revision")?;
    let schedule_name: String = row.try_get("schedule_name")?;
    let frozen_targets: Vec<String> = row.try_get("fixed_target_client_ids")?;
    ensure!(
        schedule_id == snapshot_schedule_id && frozen_targets == snapshot_targets,
        "schedule_event_receipt_snapshot_changed"
    );

    let actor_id: Option<Uuid> = row.try_get("actor_id")?;
    if !actor_authorized_in_tx(
        &mut tx,
        actor_id,
        "operator",
        EVENT_SCHEDULE_REQUIRED_SCOPES,
    )
    .await?
    {
        sqlx::query(
            r#"
            UPDATE schedules
            SET enabled = FALSE,
                last_error = 'actor_authority_revoked',
                updated_at = clock_timestamp()
            WHERE id = $1 AND definition_revision = $2
            "#,
        )
        .bind(schedule_id)
        .bind(receipt_revision)
        .execute(&mut *tx)
        .await?;
        fail_receipt_in_tx(&mut tx, receipt_id, "actor_authority_revoked").await?;
        let rendered_operation_hash: Option<String> = row.try_get("rendered_operation_hash")?;
        audit_terminal_receipt_in_tx(
            &mut tx,
            actor_id,
            schedule_id,
            &schedule_name,
            receipt_id,
            receipt_revision,
            "schedule.event_actor_revoked",
            "rejected",
            "actor_authority_revoked",
            rendered_operation_hash.as_deref(),
        )
        .await?;
        tx.commit().await?;
        return Ok(false);
    }

    let episode_id: Uuid = row.try_get("episode_id")?;
    let trigger_generation: i64 = row.try_get("trigger_generation")?;
    let event_kind: String = row.try_get("event_kind")?;
    if event_kind == "alert.resolved" {
        let trigger_receipt = sqlx::query(
            r#"
            SELECT status, job_id
            FROM schedule_event_receipts
            WHERE schedule_id = $1
              AND episode_id = $2
              AND trigger_generation = $3
              AND event_kind = 'alert.triggered'
            ORDER BY event_seq DESC
            LIMIT 1
            "#,
        )
        .bind(schedule_id)
        .bind(episode_id)
        .bind(trigger_generation)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(trigger_receipt) = trigger_receipt {
            let trigger_status: String = trigger_receipt.try_get("status")?;
            let trigger_job_id: Option<Uuid> = trigger_receipt.try_get("job_id")?;
            match (trigger_status.as_str(), trigger_job_id) {
                ("pending", _) => {
                    tx.commit().await?;
                    return Ok(false);
                }
                ("dispatched", Some(_)) => {
                    capture_resolve_dependencies_in_tx(
                        &mut tx,
                        receipt_id,
                        schedule_id,
                        episode_id,
                        trigger_generation,
                    )
                    .await?;
                    let prerequisites_pending: bool = sqlx::query_scalar(
                        r#"
                        SELECT EXISTS (
                            SELECT 1
                            FROM schedule_event_dependencies dependency
                            JOIN jobs job ON job.id = dependency.prerequisite_job_id
                            WHERE dependency.receipt_id = $1
                              AND job.completed_at IS NULL
                        )
                        "#,
                    )
                    .bind(receipt_id)
                    .fetch_one(&mut *tx)
                    .await?;
                    if prerequisites_pending {
                        tx.commit().await?;
                        return Ok(false);
                    }
                }
                _ => {
                    fail_receipt_with_reason_in_tx(
                        &mut tx,
                        receipt_id,
                        "trigger_dependency_failed",
                        "trigger_dependency_failed",
                    )
                    .await?;
                    let rendered_operation_hash: Option<String> =
                        row.try_get("rendered_operation_hash")?;
                    audit_terminal_receipt_in_tx(
                        &mut tx,
                        actor_id,
                        schedule_id,
                        &schedule_name,
                        receipt_id,
                        receipt_revision,
                        "schedule.event_dependency_failed",
                        "failed",
                        "trigger_dependency_failed",
                        rendered_operation_hash.as_deref(),
                    )
                    .await?;
                    tx.commit().await?;
                    return Ok(false);
                }
            }
        }
    }

    let rendered_operation = row.try_get::<Option<SqlJson<Value>>, _>("rendered_operation")?;
    let rendered_operation_hash = row.try_get::<Option<String>, _>("rendered_operation_hash")?;
    let (Some(rendered_operation), Some(rendered_operation_hash)) =
        (rendered_operation, rendered_operation_hash)
    else {
        terminal_config_receipt_failure_in_tx(
            &mut tx,
            actor_id,
            schedule_id,
            &schedule_name,
            receipt_id,
            receipt_revision,
            "pending_event_receipt_missing_rendered_operation",
            None,
        )
        .await?;
        tx.commit().await?;
        return Ok(false);
    };
    let operation: JobCommand = match serde_json::from_value(rendered_operation.0) {
        Ok(operation) => operation,
        Err(_) => {
            terminal_config_receipt_failure_in_tx(
                &mut tx,
                actor_id,
                schedule_id,
                &schedule_name,
                receipt_id,
                receipt_revision,
                "pending_event_receipt_rendered_operation_invalid",
                Some(&rendered_operation_hash),
            )
            .await?;
            tx.commit().await?;
            return Ok(false);
        }
    };
    if payload_hash(&encode_json(&operation)?) != rendered_operation_hash {
        terminal_config_receipt_failure_in_tx(
            &mut tx,
            actor_id,
            schedule_id,
            &schedule_name,
            receipt_id,
            receipt_revision,
            "schedule_event_receipt_operation_hash_mismatch",
            Some(&rendered_operation_hash),
        )
        .await?;
        tx.commit().await?;
        return Ok(false);
    }
    let job_id = Uuid::new_v4();
    let schedule = DueSchedule {
        id: schedule_id,
        actor_id,
        actor_username: row.try_get("actor_username")?,
        actor_role: row.try_get("actor_role")?,
        name: schedule_name,
        definition_revision: receipt_revision,
        trigger_kind: "event".to_string(),
        operation,
        selector_expression: format!("event-receipt:{receipt_id}"),
        target_client_ids: frozen_targets,
        cron_expr: String::new(),
        next_run_at_unix: 0,
        catch_up_policy: "event_edge".to_string(),
        catch_up_limit: 1,
        retry_delay_secs: 0,
        max_failures: row.try_get("max_failures")?,
        failure_count: row.try_get("failure_count")?,
        last_error: row.try_get("last_error")?,
        materialization: ScheduleMaterializationContext {
            job_id: Some(job_id),
            causation_id: Some(row.try_get("causation_id")?),
            schedule_lineage: row.try_get("dispatched_schedule_lineage")?,
            source_lifecycle_event_seq: Some(row.try_get("event_seq")?),
            source_lifecycle_event_id: Some(row.try_get("lifecycle_event_id")?),
            source_event_id: Some(row.try_get("event_id")?),
            source_payload_hash: Some(row.try_get("source_payload_hash")?),
            rendered_operation_hash: Some(rendered_operation_hash.clone()),
        },
    };
    materialize_due_schedule(&mut tx, &schedule, 0, 1, dispatch_config).await?;
    let updated = sqlx::query(
        r#"
        UPDATE schedule_event_receipts
        SET status = 'dispatched',
            status_reason = 'durable_job_materialized',
            job_id = $2,
            dispatched_at = clock_timestamp(),
            updated_at = clock_timestamp()
        WHERE id = $1 AND status = 'pending'
        "#,
    )
    .bind(receipt_id)
    .bind(job_id)
    .execute(&mut *tx)
    .await?;
    ensure!(
        updated.rows_affected() == 1,
        "schedule_event_receipt_dispatch_cas_failed"
    );
    sqlx::query(
        r#"
        INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
        VALUES ($1, $2, 'schedule.event_dispatched', $3, $4, $5)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(actor_id)
    .bind(format!("schedule:{schedule_id}"))
    .bind(&rendered_operation_hash)
    .bind(json!({
        "result": "succeeded",
        "origin_kind": "worker",
        "component": EVENT_SCHEDULE_COMPONENT,
        "receipt_id": receipt_id,
        "schedule_id": schedule_id,
        "schedule_name": &schedule.name,
        "definition_revision": receipt_revision,
        "job_id": job_id,
        "event_kind": event_kind,
        "event_id": &schedule.materialization.source_event_id,
        "lifecycle_event_id": schedule.materialization.source_lifecycle_event_id,
        "event_seq": schedule.materialization.source_lifecycle_event_seq,
        "source_payload_hash": &schedule.materialization.source_payload_hash,
        "rendered_operation_hash": rendered_operation_hash,
        "causation_id": schedule.materialization.causation_id,
        "schedule_lineage": &schedule.materialization.schedule_lineage,
    }))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

async fn capture_resolve_dependencies_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    receipt_id: Uuid,
    schedule_id: Uuid,
    episode_id: Uuid,
    trigger_generation: i64,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO schedule_event_dependencies (receipt_id, prerequisite_job_id)
        SELECT $1, trigger_receipt.job_id
        FROM schedule_event_receipts trigger_receipt
        WHERE trigger_receipt.schedule_id = $2
          AND trigger_receipt.episode_id = $3
          AND trigger_receipt.trigger_generation = $4
          AND trigger_receipt.event_kind = 'alert.triggered'
          AND trigger_receipt.status = 'dispatched'
          AND trigger_receipt.job_id IS NOT NULL
        ON CONFLICT (receipt_id, prerequisite_job_id) DO NOTHING
        "#,
    )
    .bind(receipt_id)
    .bind(schedule_id)
    .bind(episode_id)
    .bind(trigger_generation)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn fail_receipt_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    receipt_id: Uuid,
    error: &str,
) -> Result<()> {
    fail_receipt_with_reason_in_tx(tx, receipt_id, "dispatch_failed", error).await
}

async fn fail_receipt_with_reason_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    receipt_id: Uuid,
    status_reason: &str,
    error: &str,
) -> Result<()> {
    let status_reason = truncate_error(status_reason);
    let error = truncate_error(error);
    sqlx::query(
        r#"
        UPDATE schedule_event_receipts
        SET status = 'failed',
            status_reason = $2,
            error = $3,
            updated_at = clock_timestamp()
        WHERE id = $1 AND status = 'pending'
        "#,
    )
    .bind(receipt_id)
    .bind(status_reason)
    .bind(error)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn account_schedule_definition_failure_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    schedule_id: Uuid,
    definition_revision: i64,
    error: &str,
) -> Result<bool> {
    let error = truncate_error(error);
    let updated = sqlx::query(
        r#"
        UPDATE schedules
        SET failure_count = failure_count + 1,
            last_error = $3,
            enabled = CASE
                WHEN failure_count + 1 >= max_failures THEN FALSE
                ELSE enabled
            END,
            updated_at = clock_timestamp()
        WHERE id = $1 AND definition_revision = $2
        "#,
    )
    .bind(schedule_id)
    .bind(definition_revision)
    .bind(error)
    .execute(&mut **tx)
    .await?;
    Ok(updated.rows_affected() == 1)
}

#[allow(clippy::too_many_arguments)]
async fn audit_terminal_receipt_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    actor_id: Option<Uuid>,
    schedule_id: Uuid,
    schedule_name: &str,
    receipt_id: Uuid,
    definition_revision: i64,
    action: &str,
    result: &str,
    reason: &str,
    command_hash: Option<&str>,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(actor_id)
    .bind(action)
    .bind(format!("schedule:{schedule_id}"))
    .bind(command_hash)
    .bind(json!({
        "result": result,
        "origin_kind": "worker",
        "component": EVENT_SCHEDULE_COMPONENT,
        "receipt_id": receipt_id,
        "schedule_id": schedule_id,
        "schedule_name": schedule_name,
        "definition_revision": definition_revision,
        "reason": reason,
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn terminal_config_receipt_failure_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    actor_id: Option<Uuid>,
    schedule_id: Uuid,
    schedule_name: &str,
    receipt_id: Uuid,
    definition_revision: i64,
    reason: &str,
    command_hash: Option<&str>,
) -> Result<()> {
    fail_receipt_in_tx(tx, receipt_id, reason).await?;
    account_schedule_definition_failure_in_tx(tx, schedule_id, definition_revision, reason).await?;
    audit_terminal_receipt_in_tx(
        tx,
        actor_id,
        schedule_id,
        schedule_name,
        receipt_id,
        definition_revision,
        "schedule.event_config_failed",
        "failed",
        reason,
        command_hash,
    )
    .await
}

#[cfg(test)]
async fn record_event_receipt_failure(pool: &PgPool, receipt_id: Uuid, error: &str) -> Result<()> {
    let mut tx = pool.begin().await?;
    let error = truncate_error(error);
    let receipt = sqlx::query(
        r#"
        UPDATE schedule_event_receipts
        SET status = 'failed',
            status_reason = 'dispatch_failed',
            error = $2,
            updated_at = clock_timestamp()
        WHERE id = $1 AND status = 'pending'
        RETURNING schedule_id, definition_revision, rendered_operation_hash
        "#,
    )
    .bind(receipt_id)
    .bind(&error)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(receipt) = receipt else {
        tx.commit().await?;
        return Ok(());
    };
    let schedule_id: Uuid = receipt.try_get("schedule_id")?;
    let definition_revision: i64 = receipt.try_get("definition_revision")?;
    let schedule_failure_accounted = account_schedule_definition_failure_in_tx(
        &mut tx,
        schedule_id,
        definition_revision,
        &error,
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
        SELECT $1, actor_id, 'schedule.event_failed', $2, $3, $4
        FROM schedules
        WHERE id = $5
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(format!("schedule:{schedule_id}"))
    .bind(receipt.try_get::<Option<String>, _>("rendered_operation_hash")?)
    .bind(json!({
        "result": "failed",
        "origin_kind": "worker",
        "component": EVENT_SCHEDULE_COMPONENT,
        "receipt_id": receipt_id,
        "schedule_id": schedule_id,
        "definition_revision": definition_revision,
        "schedule_failure_accounted": schedule_failure_accounted,
        "error": error,
    }))
    .bind(schedule_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

fn expression_context_for_lifecycle_event(event: &LifecycleEvent) -> ExpressionContext {
    let mut context = ExpressionContext::default().with_event_predicate(&event.edge_kind);
    for predicate in &event.event_predicates {
        context = context.with_event_predicate(predicate);
    }
    let payload_root = normalized_lifecycle_payload(event);
    for root_name in ["event", "alert", "policy", "policy_rule"] {
        if let Some(value) = payload_root.get(root_name).cloned() {
            context = context.with_json_root(root_name, value);
        }
    }
    context
}

fn template_context_for_lifecycle_event(event: &LifecycleEvent, schedule: &EventSchedule) -> Value {
    let mut root = normalized_lifecycle_payload(event);
    root.insert(
        "schedule".to_string(),
        json!({
            "id": schedule.id,
            "name": schedule.name,
            "definition_revision": schedule.definition_revision,
            "fixed_target_count": schedule.target_client_ids.len(),
            "matched_subject_count": event.subject_client_ids.len(),
        }),
    );
    Value::Object(root)
}

fn normalized_lifecycle_payload(event: &LifecycleEvent) -> serde_json::Map<String, Value> {
    let mut root = event.payload.as_object().cloned().unwrap_or_default();
    if !root.contains_key("policy_rule") {
        if let Some(rule) = root.get("rule").cloned() {
            root.insert("policy_rule".to_string(), rule);
        }
    }
    let mut event_root = root
        .remove("event")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    event_root.insert("id".to_string(), Value::String(event.event_id.clone()));
    event_root.insert("kind".to_string(), Value::String(event.edge_kind.clone()));
    event_root.insert(
        "occurred_at".to_string(),
        Value::String(event.occurred_at.to_rfc3339()),
    );
    event_root.insert(
        "recorded_at".to_string(),
        Value::String(event.created_at.to_rfc3339()),
    );
    event_root.insert(
        "predicates".to_string(),
        serde_json::to_value(&event.event_predicates).unwrap_or(Value::Array(Vec::new())),
    );
    root.insert("event".to_string(), Value::Object(event_root));
    root
}

fn extend_schedule_lineage(source: &[Uuid], schedule_id: Uuid) -> LineageDecision {
    if source.contains(&schedule_id) {
        return LineageDecision::Cycle;
    }
    if source.len() >= MAX_ALERT_EVENT_LINEAGE {
        return LineageDecision::Overflow;
    }
    let mut dispatched = source.to_vec();
    dispatched.push(schedule_id);
    LineageDecision::Dispatch(dispatched)
}

async fn disable_revoked_event_schedule_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    schedule: &EventSchedule,
) -> Result<()> {
    let updated = sqlx::query(
        r#"
        UPDATE schedules
        SET enabled = FALSE,
            last_error = 'actor_authority_revoked',
            updated_at = clock_timestamp()
        WHERE id = $1 AND definition_revision = $2 AND enabled = TRUE
        "#,
    )
    .bind(schedule.id)
    .bind(schedule.definition_revision)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() == 0 {
        return Ok(());
    }
    sqlx::query(
        r#"
        INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
        VALUES ($1, $2, 'schedule.event_actor_revoked', $3, NULL, $4)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(schedule.actor_id)
    .bind(format!("schedule:{}", schedule.id))
    .bind(json!({
        "result": "rejected",
        "origin_kind": "worker",
        "component": EVENT_SCHEDULE_COMPONENT,
        "schedule_id": schedule.id,
        "definition_revision": schedule.definition_revision,
        "reason": "actor_authority_revoked_before_receipt",
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn disable_invalid_event_schedule_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    schedule: &EventSchedule,
    error: &str,
) -> Result<()> {
    let error = truncate_error(error);
    sqlx::query(
        r#"
        UPDATE schedules
        SET enabled = FALSE, last_error = $3, updated_at = clock_timestamp()
        WHERE id = $1 AND definition_revision = $2 AND enabled = TRUE
        "#,
    )
    .bind(schedule.id)
    .bind(schedule.definition_revision)
    .bind(&error)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
        VALUES ($1, $2, 'schedule.event_definition_invalid', $3, NULL, $4)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(schedule.actor_id)
    .bind(format!("schedule:{}", schedule.id))
    .bind(json!({
        "result": "failed",
        "origin_kind": "worker",
        "component": EVENT_SCHEDULE_COMPONENT,
        "schedule_id": schedule.id,
        "definition_revision": schedule.definition_revision,
        "error": error,
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn audit_receipt_created_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    receipt_id: Uuid,
    schedule: &EventSchedule,
    event: &LifecycleEvent,
    status: &str,
    status_reason: Option<&str>,
    source_payload_hash: &str,
    rendered_operation_hash: Option<&str>,
    causation_id: Uuid,
    dispatched_lineage: &[Uuid],
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
        VALUES ($1, $2, 'schedule.event_receipt_created', $3, $4, $5)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(schedule.actor_id)
    .bind(format!("schedule:{}", schedule.id))
    .bind(rendered_operation_hash)
    .bind(json!({
        "result": status,
        "origin_kind": "worker",
        "component": EVENT_SCHEDULE_COMPONENT,
        "receipt_id": receipt_id,
        "schedule_id": schedule.id,
        "schedule_name": schedule.name,
        "definition_revision": schedule.definition_revision,
        "status": status,
        "status_reason": status_reason,
        "event_seq": event.event_seq,
        "lifecycle_event_id": event.id,
        "event_kind": event.edge_kind,
        "event_id": event.event_id,
        "episode_id": event.episode_id,
        "trigger_generation": event.trigger_generation,
        "source_payload_hash": source_payload_hash,
        "rendered_operation_hash": rendered_operation_hash,
        "causation_id": causation_id,
        "source_schedule_lineage": event.schedule_lineage,
        "dispatched_schedule_lineage": dispatched_lineage,
        "matched_subject_client_ids": event.subject_client_ids,
        "fixed_target_client_ids": schedule.target_client_ids,
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn lifecycle_event_from_row(row: sqlx::postgres::PgRow) -> Result<LifecycleEvent> {
    Ok(LifecycleEvent {
        event_seq: row.try_get("event_seq")?,
        id: row.try_get("id")?,
        episode_id: row.try_get("episode_id")?,
        trigger_generation: row.try_get("trigger_generation")?,
        edge_kind: row.try_get("edge_kind")?,
        event_id: row.try_get("event_id")?,
        event_predicates: row.try_get("event_predicates")?,
        subject_client_ids: row.try_get("subject_client_ids")?,
        payload: row.try_get::<SqlJson<Value>, _>("payload")?.0,
        causation_id: row.try_get("causation_id")?,
        schedule_lineage: row.try_get("schedule_lineage")?,
        occurred_at: row.try_get("occurred_at")?,
        created_at: row.try_get("created_at")?,
    })
}

fn truncate_error(error: &str) -> String {
    error.chars().take(1024).collect()
}

#[cfg(test)]
mod tests {
    use crate::test_support::PgWorkerTestDb;

    use super::*;

    fn event_context() -> Value {
        json!({
            "event": {
                "id": "alert:edge:triggered",
                "kind": "alert.triggered",
                "occurred_at": "2026-08-18T00:00:00Z",
                "recorded_at": "2026-08-18T00:00:01Z"
            },
            "alert": {
                "id": "alert:edge",
                "title": "Traffic threshold",
                "detail": "sustained",
                "category": "traffic",
                "severity": "critical",
                "record_kind": "condition",
                "lifecycle_state": "triggered",
                "trigger_generation": 3,
                "source_status": "threshold_exceeded",
                "resolution_reason": null,
                "client_id": "edge-a",
                "target_kind": "agent",
                "target_id": "edge-a"
            },
            "policy": {"id": "policy-a", "name": "Traffic"},
            "policy_rule": {"id": "rule-a", "name": "Quota", "rule_kind": "metric", "evidence_source": "traffic"},
            "schedule": {"id": Uuid::nil(), "name": "react", "definition_revision": 4, "fixed_target_count": 1, "matched_subject_count": 1}
        })
    }

    #[test]
    fn alert_event_expression_requires_canonical_anchor_on_every_or_branch() {
        assert!(parse_and_validate_alert_event_expression(
            "alert.triggered && alert.category:traffic"
        )
        .is_ok());
        assert!(parse_and_validate_alert_event_expression(
            "(alert.triggered && alert.severity:critical) || (alert.resolved && policy.name = Traffic)"
        )
        .is_ok());
        assert!(parse_and_validate_alert_event_expression(
            "alert.triggered || telemetry.network_rate"
        )
        .is_err());
        assert!(parse_and_validate_alert_event_expression(
            "!alert.triggered && alert.category:traffic"
        )
        .is_err());
    }

    #[test]
    fn lifecycle_expression_context_uses_authoritative_event_row_fields() {
        let event = LifecycleEvent {
            event_seq: 7,
            id: Uuid::new_v4(),
            episode_id: Uuid::new_v4(),
            trigger_generation: 2,
            edge_kind: "alert.triggered".to_string(),
            event_id: "edge-7".to_string(),
            event_predicates: vec!["alert.triggered".to_string()],
            subject_client_ids: Vec::new(),
            payload: json!({
                "event": {"id": "spoofed", "kind": "alert.resolved"},
                "alert": {"category": "traffic"}
            }),
            causation_id: None,
            schedule_lineage: Vec::new(),
            occurred_at: DateTime::parse_from_rfc3339("2026-08-18T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            created_at: DateTime::parse_from_rfc3339("2026-08-18T00:00:01Z")
                .unwrap()
                .with_timezone(&Utc),
        };
        let expression = parse_and_validate_alert_event_expression(
            "alert.triggered && event.kind = alert.triggered && event.id = edge-7",
        )
        .unwrap();
        assert!(expression_matches(
            &expression_context_for_lifecycle_event(&event),
            &expression
        ));
    }

    #[test]
    fn alert_event_argv_renders_each_scalar_without_word_splitting() {
        let template = vec![
            "/usr/bin/printf".to_string(),
            "%s\\n".to_string(),
            "{alert.title} on {alert.target_id}".to_string(),
            "{schedule.definition_revision}".to_string(),
        ];
        let (operation, hash) =
            render_alert_event_job_command(Some(&template), &event_context()).unwrap();
        let JobCommand::Shell { argv, pty } = operation else {
            panic!("expected shell operation");
        };
        assert!(!pty);
        assert_eq!(
            argv,
            vec![
                "/usr/bin/printf",
                "%s\\n",
                "Traffic threshold on edge-a",
                "4"
            ]
        );
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn alert_event_argv_rejects_dynamic_programs_and_non_scalars_without_shell_inference() {
        assert!(render_alert_event_job_command(
            Some(&["{alert.title}".to_string()]),
            &event_context()
        )
        .is_err());
        assert!(render_alert_event_job_command(
            Some(&["/bin/echo".to_string(), "{alert}".to_string()]),
            &event_context()
        )
        .is_err());
        let shell_template = [
            "/bin/sh".to_string(),
            "-c".to_string(),
            "echo {alert.title}".to_string(),
        ];
        let (operation, _) =
            render_alert_event_job_command(Some(&shell_template), &event_context()).unwrap();
        let JobCommand::Shell { argv, pty } = operation else {
            panic!("expected shell operation");
        };
        assert!(!pty);
        assert_eq!(
            argv,
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "echo Traffic threshold".to_string(),
            ]
        );
    }

    #[test]
    fn alert_event_lineage_is_unique_bounded_and_never_truncated() {
        let schedule_id = Uuid::new_v4();
        assert_eq!(
            extend_schedule_lineage(&[], schedule_id),
            LineageDecision::Dispatch(vec![schedule_id])
        );
        assert_eq!(
            extend_schedule_lineage(&[schedule_id], schedule_id),
            LineageDecision::Cycle
        );
        let full = (0..MAX_ALERT_EVENT_LINEAGE)
            .map(|value| Uuid::from_u128(value as u128 + 1))
            .collect::<Vec<_>>();
        assert_eq!(
            extend_schedule_lineage(&full, Uuid::new_v4()),
            LineageDecision::Overflow
        );
        assert_eq!(full.len(), MAX_ALERT_EVENT_LINEAGE);
    }

    #[tokio::test]
    async fn postgres_both_lifecycle_edges_present_dispatch_in_edge_order() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let episode_id = insert_resolved_test_episode(&db.pool).await;
        let actor_id = insert_event_schedule_actor(&db.pool).await;
        let schedule_id = insert_event_schedule(
            &db.pool,
            actor_id,
            "alert.triggered || alert.resolved",
            None,
            &[],
            3,
        )
        .await;
        insert_lifecycle_pair(&db.pool, episode_id).await;

        let dispatched = process_alert_event_schedules(
            &db.pool,
            1,
            &ScheduleDispatchConfig::new(60, vpsman_common::DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
        )
        .await
        .unwrap();
        assert_eq!(dispatched, 2);
        let receipts = sqlx::query(
            r#"
            SELECT event_kind, status, job_id
            FROM schedule_event_receipts
            WHERE schedule_id = $1
            ORDER BY event_seq
            "#,
        )
        .bind(schedule_id)
        .fetch_all(&db.pool)
        .await
        .unwrap();
        assert_eq!(receipts.len(), 2);
        assert_eq!(
            receipts[0].try_get::<String, _>("event_kind").unwrap(),
            "alert.triggered"
        );
        assert_eq!(
            receipts[1].try_get::<String, _>("event_kind").unwrap(),
            "alert.resolved"
        );
        assert!(receipts
            .iter()
            .all(|row| row.try_get::<String, _>("status").unwrap() == "dispatched"));
        let dependency_count: i64 = sqlx::query_scalar(
            r#"
            SELECT count(*)
            FROM schedule_event_dependencies dependency
            JOIN schedule_event_receipts receipt ON receipt.id = dependency.receipt_id
            WHERE receipt.schedule_id = $1 AND receipt.event_kind = 'alert.resolved'
            "#,
        )
        .bind(schedule_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(dependency_count, 1);
        let canonical_audit_count: i64 = sqlx::query_scalar(
            r#"
            SELECT count(*)
            FROM audit_logs
            WHERE target = $1
              AND (
                  (action = 'schedule.event_receipt_created'
                   AND metadata ->> 'result' = 'pending')
                  OR
                  (action = 'schedule.event_dispatched'
                   AND metadata ->> 'result' = 'succeeded')
              )
            "#,
        )
        .bind(format!("schedule:{schedule_id}"))
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(canonical_audit_count, 4);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_schedule_round_excludes_lifecycle_events_appended_after_its_frontier() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let actor_id = insert_event_schedule_actor(&db.pool).await;
        let schedule_id = insert_event_schedule(
            &db.pool,
            actor_id,
            "alert.triggered || alert.resolved",
            None,
            &[],
            3,
        )
        .await;
        let first_episode_id = insert_resolved_test_episode(&db.pool).await;
        insert_lifecycle_pair(&db.pool, first_episode_id).await;
        let event_seq_through = current_alert_lifecycle_frontier(&db.pool).await.unwrap();
        let later_episode_id = insert_resolved_test_episode(&db.pool).await;
        let (later_triggered_seq, _) = insert_lifecycle_pair(&db.pool, later_episode_id).await;
        assert!(later_triggered_seq > event_seq_through);

        let config =
            ScheduleDispatchConfig::new(60, vpsman_common::DEFAULT_MAX_JOB_TIMEOUT_SECS, false);
        assert_eq!(
            process_alert_event_schedules_through(&db.pool, 1, &config, event_seq_through)
                .await
                .unwrap(),
            2
        );
        let (round_receipts, later_receipts): (i64, i64) = sqlx::query_as(
            r#"
            SELECT
                count(*) FILTER (WHERE event_seq <= $2),
                count(*) FILTER (WHERE event_seq > $2)
            FROM schedule_event_receipts
            WHERE schedule_id = $1
            "#,
        )
        .bind(schedule_id)
        .bind(event_seq_through)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!((round_receipts, later_receipts), (2, 0));

        assert_eq!(
            process_alert_event_schedules(&db.pool, 1, &config)
                .await
                .unwrap(),
            2
        );
        let receipt_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM schedule_event_receipts WHERE schedule_id=$1")
                .bind(schedule_id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(receipt_count, 4);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_timestamp_arming_boundaries_cover_create_edit_targets_and_reenable() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let actor_id = insert_event_schedule_actor(&db.pool).await;
        let schedule_id = insert_event_schedule(
            &db.pool,
            actor_id,
            "alert.triggered || alert.resolved",
            None,
            &[],
            3,
        )
        .await;
        let initial_armed_at: DateTime<Utc> =
            sqlx::query_scalar("SELECT event_armed_at FROM schedules WHERE id=$1")
                .bind(schedule_id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        let pre_create = insert_resolved_test_episode(&db.pool).await;
        insert_lifecycle_pair(&db.pool, pre_create).await;
        set_lifecycle_times(
            &db.pool,
            pre_create,
            initial_armed_at - chrono::Duration::seconds(1),
        )
        .await;
        let post_create = insert_resolved_test_episode(&db.pool).await;
        insert_lifecycle_pair(&db.pool, post_create).await;
        ingest_alert_lifecycle_events(&db.pool, 10).await.unwrap();
        assert_eq!(
            receipt_count_for_episode(&db.pool, schedule_id, pre_create).await,
            0
        );
        assert_eq!(
            receipt_count_for_episode(&db.pool, schedule_id, post_create).await,
            2
        );

        let pre_edit = insert_resolved_test_episode(&db.pool).await;
        insert_lifecycle_pair(&db.pool, pre_edit).await;
        let edit_armed_at: DateTime<Utc> = sqlx::query_scalar(
            r#"
            UPDATE schedules
            SET definition_revision=definition_revision+1,
                event_expression='alert.triggered || alert.resolved',
                event_armed_at=clock_timestamp()
            WHERE id=$1
            RETURNING event_armed_at
            "#,
        )
        .bind(schedule_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let pre_edit_created_at: DateTime<Utc> = sqlx::query_scalar(
            "SELECT min(created_at) FROM alert_lifecycle_events WHERE episode_id=$1",
        )
        .bind(pre_edit)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert!(pre_edit_created_at < edit_armed_at);
        let post_edit = insert_resolved_test_episode(&db.pool).await;
        insert_lifecycle_pair(&db.pool, post_edit).await;
        ingest_alert_lifecycle_events(&db.pool, 10).await.unwrap();
        assert_eq!(
            receipt_count_for_episode(&db.pool, schedule_id, pre_edit).await,
            0
        );
        assert_eq!(
            receipt_count_for_episode(&db.pool, schedule_id, post_edit).await,
            2
        );

        let pre_targets = insert_resolved_test_episode(&db.pool).await;
        insert_lifecycle_pair(&db.pool, pre_targets).await;
        let targets_armed_at: DateTime<Utc> = sqlx::query_scalar(
            r#"
            UPDATE schedules
            SET definition_revision=definition_revision+1,
                target_client_ids=ARRAY['target-generation'],
                event_armed_at=clock_timestamp()
            WHERE id=$1
            RETURNING event_armed_at
            "#,
        )
        .bind(schedule_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let pre_targets_created_at: DateTime<Utc> = sqlx::query_scalar(
            "SELECT min(created_at) FROM alert_lifecycle_events WHERE episode_id=$1",
        )
        .bind(pre_targets)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert!(pre_targets_created_at < targets_armed_at);
        let post_targets = insert_resolved_test_episode(&db.pool).await;
        insert_lifecycle_pair(&db.pool, post_targets).await;
        ingest_alert_lifecycle_events(&db.pool, 10).await.unwrap();
        assert_eq!(
            receipt_count_for_episode(&db.pool, schedule_id, pre_targets).await,
            0
        );
        assert_eq!(
            receipt_count_for_episode(&db.pool, schedule_id, post_targets).await,
            2
        );

        let disabled_at: DateTime<Utc> = sqlx::query_scalar(
            r#"
            UPDATE schedules
            SET enabled=FALSE, definition_revision=definition_revision+1,
                event_armed_at=clock_timestamp()
            WHERE id=$1
            RETURNING event_armed_at
            "#,
        )
        .bind(schedule_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let while_disabled = insert_resolved_test_episode(&db.pool).await;
        insert_lifecycle_pair(&db.pool, while_disabled).await;
        let disabled_event_created_at: DateTime<Utc> = sqlx::query_scalar(
            "SELECT min(created_at) FROM alert_lifecycle_events WHERE episode_id=$1",
        )
        .bind(while_disabled)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert!(disabled_event_created_at >= disabled_at);
        ingest_alert_lifecycle_events(&db.pool, 10).await.unwrap();
        assert_eq!(
            receipt_count_for_episode(&db.pool, schedule_id, while_disabled).await,
            0
        );

        let reenabled_at: DateTime<Utc> = sqlx::query_scalar(
            r#"
            UPDATE schedules
            SET enabled=TRUE, definition_revision=definition_revision+1,
                event_armed_at=clock_timestamp()
            WHERE id=$1
            RETURNING event_armed_at
            "#,
        )
        .bind(schedule_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert!(disabled_event_created_at < reenabled_at);
        let post_reenable = insert_resolved_test_episode(&db.pool).await;
        insert_lifecycle_pair(&db.pool, post_reenable).await;
        ingest_alert_lifecycle_events(&db.pool, 10).await.unwrap();
        assert_eq!(
            receipt_count_for_episode(&db.pool, schedule_id, while_disabled).await,
            0
        );
        assert_eq!(
            receipt_count_for_episode(&db.pool, schedule_id, post_reenable).await,
            2
        );
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_resolve_dependencies_are_isolated_per_schedule() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        insert_event_target(&db.pool, "event-edge").await;
        let episode_id = insert_resolved_test_episode(&db.pool).await;
        let actor_id = insert_event_schedule_actor(&db.pool).await;
        let schedule_a = insert_event_schedule(
            &db.pool,
            actor_id,
            "alert.triggered || alert.resolved",
            None,
            &["event-edge"],
            3,
        )
        .await;
        let schedule_b = insert_event_schedule(
            &db.pool,
            actor_id,
            "alert.triggered || alert.resolved",
            None,
            &["event-edge"],
            3,
        )
        .await;
        insert_lifecycle_pair(&db.pool, episode_id).await;
        ingest_alert_lifecycle_events(&db.pool, 10).await.unwrap();
        let config =
            ScheduleDispatchConfig::new(60, vpsman_common::DEFAULT_MAX_JOB_TIMEOUT_SECS, false);

        let trigger_a = receipt_id(&db.pool, schedule_a, "alert.triggered").await;
        let trigger_b = receipt_id(&db.pool, schedule_b, "alert.triggered").await;
        assert!(dispatch_alert_event_receipt(&db.pool, trigger_a, &config)
            .await
            .unwrap());
        assert!(dispatch_alert_event_receipt(&db.pool, trigger_b, &config)
            .await
            .unwrap());
        let job_a: Uuid =
            sqlx::query_scalar("SELECT job_id FROM schedule_event_receipts WHERE id = $1")
                .bind(trigger_a)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        let job_b: Uuid =
            sqlx::query_scalar("SELECT job_id FROM schedule_event_receipts WHERE id = $1")
                .bind(trigger_b)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        sqlx::query("UPDATE jobs SET completed_at = now(), status = 'completed' WHERE id = $1")
            .bind(job_a)
            .execute(&db.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE jobs SET completed_at = NULL, status = 'queued' WHERE id = $1")
            .bind(job_b)
            .execute(&db.pool)
            .await
            .unwrap();

        let resolve_a = receipt_id(&db.pool, schedule_a, "alert.resolved").await;
        let resolve_b = receipt_id(&db.pool, schedule_b, "alert.resolved").await;
        assert!(dispatch_alert_event_receipt(&db.pool, resolve_a, &config)
            .await
            .unwrap());
        assert!(!dispatch_alert_event_receipt(&db.pool, resolve_b, &config)
            .await
            .unwrap());
        let foreign_dependencies: i64 = sqlx::query_scalar(
            r#"
            SELECT count(*)
            FROM schedule_event_dependencies dependency
            JOIN schedule_event_receipts trigger_receipt
              ON trigger_receipt.job_id = dependency.prerequisite_job_id
            WHERE dependency.receipt_id = $1
              AND trigger_receipt.schedule_id <> $2
            "#,
        )
        .bind(resolve_a)
        .bind(schedule_a)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(foreign_dependencies, 0);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_resolve_dispatches_standalone_or_fails_with_terminal_trigger() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let episode_id = insert_resolved_test_episode(&db.pool).await;
        let actor_id = insert_event_schedule_actor(&db.pool).await;
        let standalone_schedule =
            insert_event_schedule(&db.pool, actor_id, "alert.resolved", None, &[], 3).await;
        let dependent_schedule = insert_event_schedule(
            &db.pool,
            actor_id,
            "alert.triggered || alert.resolved",
            None,
            &[],
            3,
        )
        .await;
        insert_lifecycle_pair(&db.pool, episode_id).await;
        ingest_alert_lifecycle_events(&db.pool, 10).await.unwrap();
        let config =
            ScheduleDispatchConfig::new(60, vpsman_common::DEFAULT_MAX_JOB_TIMEOUT_SECS, false);

        let standalone_resolve = receipt_id(&db.pool, standalone_schedule, "alert.resolved").await;
        assert!(
            dispatch_alert_event_receipt(&db.pool, standalone_resolve, &config)
                .await
                .unwrap()
        );

        let dependent_trigger = receipt_id(&db.pool, dependent_schedule, "alert.triggered").await;
        sqlx::query(
            r#"
            UPDATE schedule_event_receipts
            SET status = 'failed',
                status_reason = 'test_terminal_failure',
                error = 'test_terminal_failure',
                updated_at = clock_timestamp()
            WHERE id = $1
            "#,
        )
        .bind(dependent_trigger)
        .execute(&db.pool)
        .await
        .unwrap();
        let dependent_resolve = receipt_id(&db.pool, dependent_schedule, "alert.resolved").await;
        assert!(
            !dispatch_alert_event_receipt(&db.pool, dependent_resolve, &config)
                .await
                .unwrap()
        );
        let (status, status_reason, error): (String, Option<String>, Option<String>) =
            sqlx::query_as(
                "SELECT status, status_reason, error FROM schedule_event_receipts WHERE id = $1",
            )
            .bind(dependent_resolve)
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(status, "failed");
        assert_eq!(status_reason.as_deref(), Some("trigger_dependency_failed"));
        assert_eq!(error.as_deref(), Some("trigger_dependency_failed"));
        let dependency_audit_count: i64 = sqlx::query_scalar(
            r#"
            SELECT count(*)
            FROM audit_logs
            WHERE action = 'schedule.event_dependency_failed'
              AND metadata ->> 'receipt_id' = $1
              AND metadata ->> 'reason' = 'trigger_dependency_failed'
              AND metadata ->> 'result' = 'failed'
            "#,
        )
        .bind(dependent_resolve.to_string())
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(dependency_audit_count, 1);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_per_event_receipts_preserve_out_of_order_commits() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let episode_id = insert_resolved_test_episode(&db.pool).await;
        let actor_id = insert_event_schedule_actor(&db.pool).await;
        let schedule_id = insert_event_schedule(
            &db.pool,
            actor_id,
            "alert.triggered || alert.resolved",
            None,
            &[],
            3,
        )
        .await;

        let mut low_writer = db.pool.begin().await.unwrap();
        let low_seq: i64 = sqlx::query_scalar("SELECT nextval('alert_lifecycle_event_seq')")
            .fetch_one(&mut *low_writer)
            .await
            .unwrap();

        let mut high_writer = db.pool.begin().await.unwrap();
        let high_seq: i64 = sqlx::query_scalar("SELECT nextval('alert_lifecycle_event_seq')")
            .fetch_one(&mut *high_writer)
            .await
            .unwrap();
        insert_lifecycle_edge_in_tx(&mut high_writer, high_seq, episode_id, "alert.resolved").await;
        high_writer.commit().await.unwrap();

        assert_eq!(
            ingest_alert_lifecycle_events(&db.pool, 10).await.unwrap(),
            1
        );
        insert_lifecycle_edge_in_tx(&mut low_writer, low_seq, episode_id, "alert.triggered").await;
        low_writer.commit().await.unwrap();
        assert_eq!(
            ingest_alert_lifecycle_events(&db.pool, 10).await.unwrap(),
            1
        );

        let completed_receipts: i64 = sqlx::query_scalar(
            r#"
            SELECT count(*)
            FROM alert_lifecycle_consumer_receipts
            WHERE consumer_kind='schedule' AND status='completed'
              AND event_seq=ANY($1::bigint[])
            "#,
        )
        .bind(vec![low_seq, high_seq])
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let receipt_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM schedule_event_receipts WHERE schedule_id = $1",
        )
        .bind(schedule_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(completed_receipts, 2);
        assert_eq!(receipt_count, 2);
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_actor_scope_revocation_after_receipt_creates_no_job() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let episode_id = insert_resolved_test_episode(&db.pool).await;
        let actor_id = insert_event_schedule_actor(&db.pool).await;
        let schedule_id =
            insert_event_schedule(&db.pool, actor_id, "alert.triggered", None, &[], 3).await;
        insert_lifecycle_pair(&db.pool, episode_id).await;
        ingest_alert_lifecycle_events(&db.pool, 10).await.unwrap();
        sqlx::query("UPDATE operators SET scopes = '[\"jobs:write\",\"schedules:write\",\"backups:read\"]'::jsonb WHERE id = $1")
            .bind(actor_id)
            .execute(&db.pool)
            .await
            .unwrap();
        let receipt_id = receipt_id(&db.pool, schedule_id, "alert.triggered").await;
        assert!(!dispatch_alert_event_receipt(
            &db.pool,
            receipt_id,
            &ScheduleDispatchConfig::new(60, vpsman_common::DEFAULT_MAX_JOB_TIMEOUT_SECS, false,),
        )
        .await
        .unwrap());
        let (status, job_id): (String, Option<Uuid>) =
            sqlx::query_as("SELECT status, job_id FROM schedule_event_receipts WHERE id = $1")
                .bind(receipt_id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(status, "failed");
        assert!(job_id.is_none());
        let job_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM jobs WHERE source_schedule_id = $1")
                .bind(schedule_id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(job_count, 0);
        let revocation_audit_result: String = sqlx::query_scalar(
            r#"
            SELECT metadata ->> 'result'
            FROM audit_logs
            WHERE action = 'schedule.event_actor_revoked'
              AND metadata ->> 'receipt_id' = $1
            "#,
        )
        .bind(receipt_id.to_string())
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(revocation_audit_result, "rejected");
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_event_dispatch_does_not_block_agent_owned_client_updates() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let client_id = format!("event-dispatch-unlocked-{}", Uuid::new_v4());
        insert_event_target(&db.pool, &client_id).await;
        let episode_id = insert_resolved_test_episode(&db.pool).await;
        let actor_id = insert_event_schedule_actor(&db.pool).await;
        let schedule_id = insert_event_schedule(
            &db.pool,
            actor_id,
            "alert.triggered",
            None,
            &[&client_id],
            3,
        )
        .await;
        insert_lifecycle_pair(&db.pool, episode_id).await;
        ingest_alert_lifecycle_events(&db.pool, 10).await.unwrap();
        let receipt_id = receipt_id(&db.pool, schedule_id, "alert.triggered").await;

        // Agent status/telemetry writers take a non-key-changing row lock when
        // they update this client. Schedule dispatch consumes its immutable
        // receipt and may read that status, but must not take ownership of the
        // client row or wait for the unrelated producer to commit.
        let mut agent_writer = db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM clients WHERE id=$1 FOR NO KEY UPDATE")
            .bind(&client_id)
            .fetch_one(&mut *agent_writer)
            .await
            .unwrap();

        let dispatched = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            dispatch_alert_event_receipt(
                &db.pool,
                receipt_id,
                &ScheduleDispatchConfig::new(
                    60,
                    vpsman_common::DEFAULT_MAX_JOB_TIMEOUT_SECS,
                    false,
                ),
            ),
        )
        .await
        .expect("event dispatch blocked on an agent-owned client row")
        .unwrap();
        assert!(dispatched);
        agent_writer.rollback().await.unwrap();
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_accepted_receipt_survives_edit_disable_delete_and_defer() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let episode_id = insert_resolved_test_episode(&db.pool).await;
        let actor_id = insert_event_schedule_actor(&db.pool).await;
        let schedule_id =
            insert_event_schedule(&db.pool, actor_id, "alert.triggered", None, &[], 3).await;
        insert_lifecycle_pair(&db.pool, episode_id).await;
        ingest_alert_lifecycle_events(&db.pool, 10).await.unwrap();
        let receipt_id = receipt_id(&db.pool, schedule_id, "alert.triggered").await;
        sqlx::query(
            r#"
            UPDATE schedules
            SET definition_revision = 2,
                enabled = FALSE,
                deleted_at = now(),
                deferred_until = now() + interval '1 day'
            WHERE id = $1
            "#,
        )
        .bind(schedule_id)
        .execute(&db.pool)
        .await
        .unwrap();
        assert!(dispatch_alert_event_receipt(
            &db.pool,
            receipt_id,
            &ScheduleDispatchConfig::new(60, vpsman_common::DEFAULT_MAX_JOB_TIMEOUT_SECS, false,),
        )
        .await
        .unwrap());
        let status: String =
            sqlx::query_scalar("SELECT status FROM schedule_event_receipts WHERE id = $1")
                .bind(receipt_id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(status, "dispatched");
        let schedule_last_job: Option<Uuid> =
            sqlx::query_scalar("SELECT last_job_id FROM schedules WHERE id = $1")
                .bind(schedule_id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert!(
            schedule_last_job.is_none(),
            "old receipt must not mutate edited definition"
        );
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_receipt_dispatch_uses_acceptance_actor_and_schedule_name_snapshot() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let episode_id = insert_resolved_test_episode(&db.pool).await;
        let actor_a = insert_event_schedule_actor(&db.pool).await;
        let schedule_id =
            insert_event_schedule(&db.pool, actor_a, "alert.triggered", None, &[], 3).await;
        let original_schedule_name: String =
            sqlx::query_scalar("SELECT name FROM schedules WHERE id = $1")
                .bind(schedule_id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        insert_lifecycle_pair(&db.pool, episode_id).await;
        ingest_alert_lifecycle_events(&db.pool, 10).await.unwrap();

        let actor_b = insert_event_schedule_actor(&db.pool).await;
        sqlx::query("UPDATE operators SET scopes = '[]'::jsonb WHERE id = $1")
            .bind(actor_b)
            .execute(&db.pool)
            .await
            .unwrap();
        sqlx::query(
            r#"
            UPDATE schedules
            SET actor_id = $2,
                name = 'edited-and-deleted-schedule',
                definition_revision = 2,
                enabled = FALSE,
                deleted_at = clock_timestamp()
            WHERE id = $1
            "#,
        )
        .bind(schedule_id)
        .bind(actor_b)
        .execute(&db.pool)
        .await
        .unwrap();

        let receipt_id = receipt_id(&db.pool, schedule_id, "alert.triggered").await;
        assert!(dispatch_alert_event_receipt(
            &db.pool,
            receipt_id,
            &ScheduleDispatchConfig::new(60, vpsman_common::DEFAULT_MAX_JOB_TIMEOUT_SECS, false,),
        )
        .await
        .unwrap());
        let job_actor_id: Option<Uuid> = sqlx::query_scalar(
            r#"
            SELECT job.actor_id
            FROM jobs job
            JOIN schedule_event_receipts receipt ON receipt.job_id = job.id
            WHERE receipt.id = $1
            "#,
        )
        .bind(receipt_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(job_actor_id, Some(actor_a));
        let audit_row = sqlx::query(
            r#"
            SELECT actor_id, metadata ->> 'schedule_name' AS schedule_name
            FROM audit_logs
            WHERE action = 'schedule.event_dispatched'
              AND metadata ->> 'receipt_id' = $1
            "#,
        )
        .bind(receipt_id.to_string())
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(
            audit_row.try_get::<Option<Uuid>, _>("actor_id").unwrap(),
            Some(actor_a)
        );
        assert_eq!(
            audit_row.try_get::<String, _>("schedule_name").unwrap(),
            original_schedule_name
        );
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_events_from_entire_defer_window_are_not_replayed_after_expiry() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let deferred_episode_id = insert_resolved_test_episode(&db.pool).await;
        let actor_id = insert_event_schedule_actor(&db.pool).await;
        let schedule_id = insert_event_schedule(
            &db.pool,
            actor_id,
            "alert.triggered || alert.resolved",
            None,
            &[],
            3,
        )
        .await;
        insert_lifecycle_pair(&db.pool, deferred_episode_id).await;

        // Model a worker that did not run at all while the schedule was
        // deferred: by the time it resumes, the boundary has expired but both
        // lifecycle edges were durably recorded inside the defer window.
        let deferred_until: DateTime<Utc> =
            sqlx::query_scalar("SELECT clock_timestamp() - interval '1 second'")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        sqlx::query("UPDATE schedules SET deferred_until = $2 WHERE id = $1")
            .bind(schedule_id)
            .bind(deferred_until)
            .execute(&db.pool)
            .await
            .unwrap();
        sqlx::query(
            r#"
            UPDATE alert_lifecycle_events
            SET occurred_at = $2::timestamptz - interval '1 second',
                created_at = $2::timestamptz - interval '1 second'
            WHERE episode_id = $1
            "#,
        )
        .bind(deferred_episode_id)
        .bind(deferred_until)
        .execute(&db.pool)
        .await
        .unwrap();

        assert_eq!(
            ingest_alert_lifecycle_events(&db.pool, 10).await.unwrap(),
            2
        );
        let deferred_receipts: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM schedule_event_receipts WHERE schedule_id = $1 AND episode_id = $2",
        )
        .bind(schedule_id)
        .bind(deferred_episode_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(deferred_receipts, 0);

        // Expiry resumes intake for genuinely new edges without replaying the
        // cursor-consumed edges from the defer window.
        let post_defer_episode_id = insert_resolved_test_episode(&db.pool).await;
        insert_lifecycle_pair(&db.pool, post_defer_episode_id).await;
        assert_eq!(
            ingest_alert_lifecycle_events(&db.pool, 10).await.unwrap(),
            2
        );
        let receipt_counts: (i64, i64) = sqlx::query_as(
            r#"
            SELECT
                count(*) FILTER (WHERE episode_id = $2),
                count(*) FILTER (WHERE episode_id = $3)
            FROM schedule_event_receipts
            WHERE schedule_id = $1
            "#,
        )
        .bind(schedule_id)
        .bind(deferred_episode_id)
        .bind(post_defer_episode_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(receipt_counts, (0, 2));
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_waiting_resolves_do_not_starve_newer_ready_receipt_past_batch_limit() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let episode_id = insert_resolved_test_episode(&db.pool).await;
        let actor_id = insert_event_schedule_actor(&db.pool).await;
        let mut waiting_schedule_ids = Vec::new();
        for _ in 0..101 {
            waiting_schedule_ids.push(
                insert_event_schedule(
                    &db.pool,
                    actor_id,
                    "alert.triggered || alert.resolved",
                    None,
                    &[],
                    3,
                )
                .await,
            );
        }
        insert_lifecycle_pair(&db.pool, episode_id).await;
        ingest_alert_lifecycle_events(&db.pool, 10).await.unwrap();

        // Freeze each Trigger as dispatched to an intentionally incomplete job,
        // leaving more than one full dispatch batch of Resolve receipts waiting.
        sqlx::query(
            r#"
            INSERT INTO jobs (
                id, actor_id, command_type, privileged, status, target_count,
                payload_hash, operation, source_schedule_id, request_fingerprint,
                max_timeout_secs
            )
            SELECT
                receipt.id, receipt.actor_id, 'scheduled_shell', TRUE, 'queued', 0,
                receipt.rendered_operation_hash, receipt.rendered_operation,
                receipt.schedule_id, repeat('a', 64), 60
            FROM schedule_event_receipts receipt
            WHERE receipt.schedule_id = ANY($1::uuid[])
              AND receipt.event_kind = 'alert.triggered'
            "#,
        )
        .bind(&waiting_schedule_ids)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            UPDATE schedule_event_receipts
            SET status = 'dispatched',
                status_reason = 'test_incomplete_trigger_job',
                job_id = id,
                dispatched_at = clock_timestamp(),
                updated_at = clock_timestamp()
            WHERE schedule_id = ANY($1::uuid[])
              AND event_kind = 'alert.triggered'
            "#,
        )
        .bind(&waiting_schedule_ids)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query("UPDATE schedules SET enabled = FALSE WHERE id = ANY($1::uuid[])")
            .bind(&waiting_schedule_ids)
            .execute(&db.pool)
            .await
            .unwrap();

        let ready_schedule =
            insert_event_schedule(&db.pool, actor_id, "alert.triggered", None, &[], 3).await;
        let ready_episode = insert_resolved_test_episode(&db.pool).await;
        insert_lifecycle_pair(&db.pool, ready_episode).await;
        ingest_alert_lifecycle_events(&db.pool, 10).await.unwrap();

        let dispatched = process_alert_event_schedules(
            &db.pool,
            100,
            &ScheduleDispatchConfig::new(60, vpsman_common::DEFAULT_MAX_JOB_TIMEOUT_SECS, false),
        )
        .await
        .unwrap();
        assert_eq!(dispatched, 1);
        let ready_status: String = sqlx::query_scalar(
            "SELECT status FROM schedule_event_receipts WHERE schedule_id = $1 AND episode_id = $2",
        )
        .bind(ready_schedule)
        .bind(ready_episode)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(ready_status, "dispatched");
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_subjectless_null_render_failure_trips_revision_scoped_circuit_breaker() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let episode_id = insert_resolved_test_episode(&db.pool).await;
        let actor_id = insert_event_schedule_actor(&db.pool).await;
        let schedule_id = insert_event_schedule(
            &db.pool,
            actor_id,
            "alert.triggered",
            Some(vec![
                "/bin/echo".to_string(),
                "{alert.resolution_reason}".to_string(),
            ]),
            &[],
            1,
        )
        .await;
        insert_lifecycle_pair(&db.pool, episode_id).await;
        ingest_alert_lifecycle_events(&db.pool, 10).await.unwrap();
        let row = sqlx::query(
            r#"
            SELECT receipt.status, receipt.error, schedule.enabled,
                   schedule.failure_count, schedule.last_error
            FROM schedule_event_receipts receipt
            JOIN schedules schedule ON schedule.id = receipt.schedule_id
            WHERE receipt.schedule_id = $1 AND receipt.event_kind = 'alert.triggered'
            "#,
        )
        .bind(schedule_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(row.try_get::<String, _>("status").unwrap(), "failed");
        assert!(row
            .try_get::<String, _>("error")
            .unwrap()
            .contains("resolved to null"));
        assert!(!row.try_get::<bool, _>("enabled").unwrap());
        assert_eq!(row.try_get::<i32, _>("failure_count").unwrap(), 1);
        assert!(row
            .try_get::<String, _>("last_error")
            .unwrap()
            .contains("resolved to null"));
        db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_old_receipt_failure_does_not_mutate_edited_definition() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let episode_id = insert_resolved_test_episode(&db.pool).await;
        let actor_id = insert_event_schedule_actor(&db.pool).await;
        let schedule_id =
            insert_event_schedule(&db.pool, actor_id, "alert.triggered", None, &[], 3).await;
        insert_lifecycle_pair(&db.pool, episode_id).await;
        ingest_alert_lifecycle_events(&db.pool, 10).await.unwrap();
        let receipt_id = receipt_id(&db.pool, schedule_id, "alert.triggered").await;
        sqlx::query(
            "UPDATE schedules SET definition_revision = 2, failure_count = 0, last_error = NULL WHERE id = $1",
        )
        .bind(schedule_id)
        .execute(&db.pool)
        .await
        .unwrap();
        record_event_receipt_failure(&db.pool, receipt_id, "old_job_failed")
            .await
            .unwrap();
        let row = sqlx::query(
            "SELECT definition_revision, failure_count, last_error FROM schedules WHERE id = $1",
        )
        .bind(schedule_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(row.try_get::<i64, _>("definition_revision").unwrap(), 2);
        assert_eq!(row.try_get::<i32, _>("failure_count").unwrap(), 0);
        assert!(row
            .try_get::<Option<String>, _>("last_error")
            .unwrap()
            .is_none());
        db.cleanup().await;
    }

    async fn insert_event_schedule_actor(pool: &PgPool) -> Uuid {
        let actor_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO operators (id, username, password_hash, status, role, scopes)
            VALUES ($1, $2, 'test-password-hash', 'active', 'operator', $3)
            "#,
        )
        .bind(actor_id)
        .bind(format!("event-worker-{actor_id}"))
        .bind(json!(EVENT_SCHEDULE_REQUIRED_SCOPES))
        .execute(pool)
        .await
        .unwrap();
        actor_id
    }

    async fn insert_event_schedule(
        pool: &PgPool,
        actor_id: Uuid,
        expression: &str,
        argv_template: Option<Vec<String>>,
        targets: &[&str],
        max_failures: i32,
    ) -> Uuid {
        let schedule_id = Uuid::new_v4();
        let targets = targets
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>();
        sqlx::query(
            r#"
            INSERT INTO schedules (
                id, actor_id, name, enabled, operation, selector_expression,
                target_client_ids, cron_expr, timezone, next_run_at,
                catch_up_policy, catch_up_limit, retry_delay_secs, max_failures,
                trigger_kind, event_expression, event_argv_template,
                definition_revision, event_armed_at
            )
            VALUES (
                $1, $2, $3, TRUE, NULL, 'id:*', $4, NULL, NULL, NULL,
                NULL, NULL, NULL, $5, 'event', $6, $7, 1, now()
            )
            "#,
        )
        .bind(schedule_id)
        .bind(actor_id)
        .bind(format!("event-schedule-{schedule_id}"))
        .bind(targets)
        .bind(max_failures)
        .bind(expression)
        .bind(argv_template.map(SqlJson))
        .execute(pool)
        .await
        .unwrap();
        schedule_id
    }

    async fn insert_event_target(pool: &PgPool, client_id: &str) {
        sqlx::query(
            r#"
            INSERT INTO clients (
                id, display_name, public_key, status, internal_build_number,
                process_incarnation_id, capabilities
            )
            VALUES ($1, $1, decode('', 'hex'), 'online', 1, $2, $3)
            "#,
        )
        .bind(client_id)
        .bind(Uuid::new_v4())
        .bind(SqlJson(vpsman_common::AgentCapabilitySnapshot::default()))
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_resolved_test_episode(pool: &PgPool) -> Uuid {
        let episode_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO alert_episodes (
                id, public_id, producer_kind, natural_key, record_kind,
                trigger_generation, trigger_severity, trigger_category, severity,
                category, target_kind, target_id, client_id, title, detail,
                source_status, evidence, lifecycle_state, triggered_at,
                last_confirmed_at, resolved_at, resolution_reason,
                policy_group_id, policy_rule_id, policy_rule_version,
                policy_rule_kind, policy_group_name, policy_rule_name,
                policy_rule_system_seed_key
            )
            SELECT
                $1, $2, rule.evidence_source, $3, 'event', 1,
                'warning', 'job', 'warning', 'job', 'job', $2, NULL,
                'Test lifecycle edge', 'Subjectless lifecycle fixture',
                'partial_success', '{}'::jsonb, 'resolved',
                now() - interval '2 seconds', now() - interval '2 seconds',
                now() - interval '1 second', 'policy_time_elapsed',
                rule.group_id, rule.id, rule.rule_version, rule.rule_kind,
                policy.name, rule.name, rule.system_seed_key
            FROM policy_rules rule
            JOIN policy_groups policy ON policy.id = rule.group_id
            WHERE rule.id = 'd1000000-0000-4000-8000-000000000007'
            "#,
        )
        .bind(episode_id)
        .bind(format!("test-alert:{episode_id}"))
        .bind(format!("test-event:{episode_id}"))
        .execute(pool)
        .await
        .unwrap();
        episode_id
    }

    async fn insert_lifecycle_pair(pool: &PgPool, episode_id: Uuid) -> (i64, i64) {
        let mut tx = pool.begin().await.unwrap();
        let triggered_seq: i64 = sqlx::query_scalar("SELECT nextval('alert_lifecycle_event_seq')")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        insert_lifecycle_edge_in_tx(&mut tx, triggered_seq, episode_id, "alert.triggered").await;
        let resolved_seq: i64 = sqlx::query_scalar("SELECT nextval('alert_lifecycle_event_seq')")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        insert_lifecycle_edge_in_tx(&mut tx, resolved_seq, episode_id, "alert.resolved").await;
        tx.commit().await.unwrap();
        (triggered_seq, resolved_seq)
    }

    async fn set_lifecycle_times(pool: &PgPool, episode_id: Uuid, at: DateTime<Utc>) {
        sqlx::query(
            "UPDATE alert_lifecycle_events SET occurred_at=$2, created_at=$2 WHERE episode_id=$1",
        )
        .bind(episode_id)
        .bind(at)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn receipt_count_for_episode(pool: &PgPool, schedule_id: Uuid, episode_id: Uuid) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM schedule_event_receipts WHERE schedule_id=$1 AND episode_id=$2",
        )
        .bind(schedule_id)
        .bind(episode_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn insert_lifecycle_edge_in_tx(
        tx: &mut Transaction<'_, Postgres>,
        event_seq: i64,
        episode_id: Uuid,
        edge_kind: &str,
    ) {
        let state = edge_kind.strip_prefix("alert.").unwrap();
        let event_id = format!("test-alert:{episode_id}:{state}");
        let resolution_reason = (edge_kind == "alert.resolved").then_some("policy_time_elapsed");
        sqlx::query(
            r#"
            INSERT INTO alert_lifecycle_events (
                event_seq, id, episode_id, trigger_generation, edge_kind,
                event_id, event_predicates, subject_client_ids, payload,
                causation_id, schedule_lineage, occurred_at
            )
            VALUES ($1, $2, $3, 1, $4, $5, $6, ARRAY[]::TEXT[], $7,
                    NULL, ARRAY[]::UUID[], now())
            "#,
        )
        .bind(event_seq)
        .bind(Uuid::new_v4())
        .bind(episode_id)
        .bind(edge_kind)
        .bind(&event_id)
        .bind(vec![
            edge_kind.to_string(),
            "alert.category:job".to_string(),
            "alert.severity:warning".to_string(),
        ])
        .bind(SqlJson(json!({
            "event": {"id": event_id, "kind": edge_kind},
            "alert": {
                "id": format!("test-alert:{episode_id}"),
                "episode_id": episode_id,
                "record_kind": "event",
                "lifecycle_state": state,
                "trigger_generation": 1,
                "severity": "warning",
                "category": "job",
                "source_status": "partial_success",
                "resolution_reason": resolution_reason,
                "title": "Test lifecycle edge",
                "detail": "Subjectless lifecycle fixture",
                "client_id": null,
                "target_kind": "job",
                "target_id": format!("test-alert:{episode_id}")
            },
            "policy": {
                "id": "c1000000-0000-4000-8000-000000000001",
                "name": "System operational evidence policies"
            },
            "policy_rule": {
                "id": "d1000000-0000-4000-8000-000000000007",
                "name": "General job partial success",
                "rule_kind": "occurrence",
                "evidence_source": "job.terminal"
            }
        })))
        .execute(&mut **tx)
        .await
        .unwrap();
    }

    async fn receipt_id(pool: &PgPool, schedule_id: Uuid, event_kind: &str) -> Uuid {
        sqlx::query_scalar(
            "SELECT id FROM schedule_event_receipts WHERE schedule_id = $1 AND event_kind = $2",
        )
        .bind(schedule_id)
        .bind(event_kind)
        .fetch_one(pool)
        .await
        .unwrap()
    }
}
