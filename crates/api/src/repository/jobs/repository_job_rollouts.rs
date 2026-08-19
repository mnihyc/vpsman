use anyhow::{bail, ensure, Result};
use serde_json::json;
use sqlx::{postgres::PgRow, Row};
use uuid::Uuid;

use crate::{
    model::{
        AuditLogView, AuthContext, JobRolloutTargetView, JobRolloutView, MemoryJobRolloutRecord,
    },
    repository::{MemoryState, Repository},
    unix_now,
};

const ROLLOUT_STATUS_RUNNING: &str = "running";
const ROLLOUT_STATUS_PAUSED: &str = "paused";
const ROLLOUT_STATUS_COMPLETED: &str = "completed";
const ROLLOUT_STATUS_ABORTED: &str = "aborted";
const ROLLOUT_PAUSE_CURRENT_BATCH_ASSIGNMENT_MISSING: &str = "current_batch_assignment_missing";

impl Repository {
    pub(crate) async fn list_job_rollouts(&self, limit: i64) -> Result<Vec<JobRolloutView>> {
        match self {
            Self::Memory(memory) => {
                let mut records = memory.job_rollouts.read().await.clone();
                records.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
                records.truncate(limit.clamp(1, 200) as usize);
                let mut views = Vec::with_capacity(records.len());
                for record in records {
                    views.push(memory_rollout_view(memory, &record).await?);
                }
                Ok(views)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(&format!(
                    "{} ORDER BY rollout.updated_at DESC, rollout.job_id LIMIT $1",
                    postgres_rollout_select()
                ))
                .bind(limit.clamp(1, 200))
                .fetch_all(pool)
                .await?;
                let mut views = Vec::with_capacity(rows.len());
                for row in rows {
                    views.push(postgres_rollout_view(pool, row).await?);
                }
                Ok(views)
            }
        }
    }

    pub(crate) async fn get_job_rollout(&self, job_id: Uuid) -> Result<Option<JobRolloutView>> {
        match self {
            Self::Memory(memory) => {
                let record = memory
                    .job_rollouts
                    .read()
                    .await
                    .iter()
                    .find(|record| record.job_id == job_id)
                    .cloned();
                match record {
                    Some(record) => Ok(Some(memory_rollout_view(memory, &record).await?)),
                    None => Ok(None),
                }
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(&format!(
                    "{} WHERE rollout.job_id = $1",
                    postgres_rollout_select()
                ))
                .bind(job_id)
                .fetch_optional(pool)
                .await?;
                match row {
                    Some(row) => Ok(Some(postgres_rollout_view(pool, row).await?)),
                    None => Ok(None),
                }
            }
        }
    }

    pub(crate) async fn pause_job_rollout(
        &self,
        job_id: Uuid,
        operator: &AuthContext,
        reason: Option<&str>,
    ) -> Result<JobRolloutView> {
        let reason = normalized_reason(reason, "operator_requested");
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut rollouts = memory.job_rollouts.write().await;
                let rollout = rollouts
                    .iter_mut()
                    .find(|rollout| rollout.job_id == job_id)
                    .ok_or_else(|| anyhow::anyhow!("job_rollout_not_found"))?;
                if is_rollout_terminal(&rollout.status) {
                    bail!("job_rollout_terminal");
                }
                if rollout.status == ROLLOUT_STATUS_RUNNING {
                    rollout.status = ROLLOUT_STATUS_PAUSED.to_string();
                    rollout.pause_reason = Some(reason.clone());
                    rollout.updated_at = now.clone();
                }
                let record = rollout.clone();
                drop(rollouts);
                push_memory_rollout_audit(
                    memory,
                    operator,
                    job_id,
                    "job.rollout_paused",
                    &reason,
                    &now,
                )
                .await;
                memory_rollout_view(memory, &record).await
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let status: Option<String> = sqlx::query_scalar(
                    "SELECT status FROM job_rollouts WHERE job_id = $1 FOR UPDATE",
                )
                .bind(job_id)
                .fetch_optional(&mut *tx)
                .await?;
                let status = status.ok_or_else(|| anyhow::anyhow!("job_rollout_not_found"))?;
                if is_rollout_terminal(&status) {
                    bail!("job_rollout_terminal");
                }
                if status == ROLLOUT_STATUS_RUNNING {
                    sqlx::query(
                        r#"
                        UPDATE job_rollouts
                        SET status = 'paused', pause_reason = $2, updated_at = now()
                        WHERE job_id = $1
                        "#,
                    )
                    .bind(job_id)
                    .bind(&reason)
                    .execute(&mut *tx)
                    .await?;
                }
                insert_postgres_rollout_audit(
                    &mut tx,
                    operator,
                    job_id,
                    "job.rollout_paused",
                    &reason,
                )
                .await?;
                let view = postgres_rollout_view_in_tx(&mut tx, job_id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("job_rollout_not_found"))?;
                tx.commit().await?;
                Ok(view)
            }
        }
    }

    pub(crate) async fn resume_job_rollout(
        &self,
        job_id: Uuid,
        operator: &AuthContext,
        reason: Option<&str>,
    ) -> Result<JobRolloutView> {
        let reason = normalized_reason(reason, "operator_confirmed");
        match self {
            Self::Memory(memory) => {
                let now_unix = unix_now();
                let now = now_unix.to_string();
                let failure_count = memory_rollout_failure_count(memory, job_id).await?;
                let mut rollouts = memory.job_rollouts.write().await;
                let rollout = rollouts
                    .iter_mut()
                    .find(|rollout| rollout.job_id == job_id)
                    .ok_or_else(|| anyhow::anyhow!("job_rollout_not_found"))?;
                if is_rollout_terminal(&rollout.status) {
                    bail!("job_rollout_terminal");
                }
                if rollout.status == ROLLOUT_STATUS_PAUSED {
                    rollout.status = ROLLOUT_STATUS_RUNNING.to_string();
                    rollout.failure_baseline = failure_count;
                    rollout.pause_reason = None;
                    rollout.next_batch_unix = now_unix;
                    rollout.updated_at = now.clone();
                }
                let record = rollout.clone();
                drop(rollouts);
                push_memory_rollout_audit(
                    memory,
                    operator,
                    job_id,
                    "job.rollout_resumed",
                    &reason,
                    &now,
                )
                .await;
                memory_rollout_view(memory, &record).await
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    "SELECT status, current_batch FROM job_rollouts WHERE job_id = $1 FOR UPDATE",
                )
                .bind(job_id)
                .fetch_optional(&mut *tx)
                .await?;
                let row = row.ok_or_else(|| anyhow::anyhow!("job_rollout_not_found"))?;
                let status: String = row.try_get("status")?;
                if is_rollout_terminal(&status) {
                    bail!("job_rollout_terminal");
                }
                if status == ROLLOUT_STATUS_PAUSED {
                    let current_batch: i32 = row.try_get("current_batch")?;
                    let failure_count: i64 = sqlx::query_scalar(
                        r#"
                        SELECT count(*)
                        FROM job_rollout_targets assignment
                        JOIN job_targets target
                          ON target.job_id = assignment.job_id
                         AND target.client_id = assignment.client_id
                        WHERE assignment.job_id = $1
                          AND assignment.batch_index <= $2
                          AND target.completed_at IS NOT NULL
                          AND target.status <> 'completed'
                        "#,
                    )
                    .bind(job_id)
                    .bind(current_batch)
                    .fetch_one(&mut *tx)
                    .await?;
                    sqlx::query(
                        r#"
                        UPDATE job_rollouts
                        SET
                            status = 'running',
                            failure_baseline = $2,
                            pause_reason = NULL,
                            next_batch_at = now(),
                            updated_at = now()
                        WHERE job_id = $1
                        "#,
                    )
                    .bind(job_id)
                    .bind(i32::try_from(failure_count)?)
                    .execute(&mut *tx)
                    .await?;
                }
                insert_postgres_rollout_audit(
                    &mut tx,
                    operator,
                    job_id,
                    "job.rollout_resumed",
                    &reason,
                )
                .await?;
                let view = postgres_rollout_view_in_tx(&mut tx, job_id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("job_rollout_not_found"))?;
                tx.commit().await?;
                Ok(view)
            }
        }
    }

    pub(crate) async fn reconcile_job_rollouts(&self, limit: i64) -> Result<usize> {
        match self {
            Self::Memory(memory) => reconcile_memory_rollouts(memory, limit).await,
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let rows = sqlx::query(
                    r#"
                    WITH locked_rollouts AS (
                        SELECT
                            job_id,
                            current_batch,
                            total_batches,
                            max_failures,
                            pause_after_canary,
                            batch_delay_secs,
                            failure_baseline
                        FROM job_rollouts
                        WHERE status = 'running'
                          AND completed_at IS NULL
                        ORDER BY updated_at, job_id
                        LIMIT $1
                        FOR UPDATE SKIP LOCKED
                    )
                    SELECT
                        rollout.job_id,
                        rollout.current_batch,
                        rollout.total_batches,
                        rollout.max_failures,
                        rollout.pause_after_canary,
                        rollout.batch_delay_secs,
                        rollout.failure_baseline,
                        count(*) FILTER (
                            WHERE assignment.batch_index = rollout.current_batch
                        ) AS current_count,
                        count(*) FILTER (
                            WHERE assignment.batch_index = rollout.current_batch
                              AND target.completed_at IS NOT NULL
                        ) AS current_terminal_count,
                        count(*) FILTER (
                            WHERE assignment.batch_index <= rollout.current_batch
                              AND target.completed_at IS NOT NULL
                              AND target.status <> 'completed'
                        ) AS failure_count
                    FROM locked_rollouts rollout
                    LEFT JOIN job_rollout_targets assignment
                      ON assignment.job_id = rollout.job_id
                    LEFT JOIN job_targets target
                      ON target.job_id = assignment.job_id
                     AND target.client_id = assignment.client_id
                    GROUP BY
                        rollout.job_id,
                        rollout.current_batch,
                        rollout.total_batches,
                        rollout.max_failures,
                        rollout.pause_after_canary,
                        rollout.batch_delay_secs,
                        rollout.failure_baseline
                    ORDER BY rollout.job_id
                    "#,
                )
                .bind(limit.clamp(1, 500))
                .fetch_all(&mut *tx)
                .await?;
                let mut changed = 0;
                for row in rows {
                    let job_id: Uuid = row.try_get("job_id")?;
                    let current_count: i64 = row.try_get("current_count")?;
                    let terminal_count: i64 = row.try_get("current_terminal_count")?;
                    if current_count == 0 {
                        let result = sqlx::query(
                            r#"
                            UPDATE job_rollouts
                            SET
                                status = 'paused',
                                pause_reason = $2,
                                next_batch_at = now(),
                                updated_at = now()
                            WHERE job_id = $1 AND status = 'running'
                            "#,
                        )
                        .bind(job_id)
                        .bind(ROLLOUT_PAUSE_CURRENT_BATCH_ASSIGNMENT_MISSING)
                        .execute(&mut *tx)
                        .await?;
                        changed += usize::try_from(result.rows_affected())?;
                        continue;
                    }
                    if terminal_count != current_count {
                        continue;
                    }
                    let current_batch: i32 = row.try_get("current_batch")?;
                    let total_batches: i32 = row.try_get("total_batches")?;
                    let failure_count: i64 = row.try_get("failure_count")?;
                    let failure_baseline: i32 = row.try_get("failure_baseline")?;
                    let max_failures: i32 = row.try_get("max_failures")?;
                    let pause_after_canary: bool = row.try_get("pause_after_canary")?;
                    let batch_delay_secs: i64 = row.try_get("batch_delay_secs")?;
                    let next_batch = current_batch + 1;
                    if next_batch >= total_batches {
                        sqlx::query(
                            r#"
                            UPDATE job_rollouts
                            SET
                                status = 'completed',
                                pause_reason = NULL,
                                completed_at = now(),
                                updated_at = now()
                            WHERE job_id = $1 AND status = 'running'
                            "#,
                        )
                        .bind(job_id)
                        .execute(&mut *tx)
                        .await?;
                    } else if failure_count - i64::from(failure_baseline) > i64::from(max_failures)
                    {
                        sqlx::query(
                            r#"
                            UPDATE job_rollouts
                            SET
                                status = 'paused',
                                current_batch = $2,
                                pause_reason = 'failure_threshold',
                                next_batch_at = now(),
                                updated_at = now()
                            WHERE job_id = $1 AND status = 'running'
                            "#,
                        )
                        .bind(job_id)
                        .bind(next_batch)
                        .execute(&mut *tx)
                        .await?;
                    } else if current_batch == 0 && pause_after_canary {
                        sqlx::query(
                            r#"
                            UPDATE job_rollouts
                            SET
                                status = 'paused',
                                current_batch = $2,
                                pause_reason = 'canary_review',
                                next_batch_at = now(),
                                updated_at = now()
                            WHERE job_id = $1 AND status = 'running'
                            "#,
                        )
                        .bind(job_id)
                        .bind(next_batch)
                        .execute(&mut *tx)
                        .await?;
                    } else {
                        sqlx::query(
                            r#"
                            UPDATE job_rollouts
                            SET
                                current_batch = $2,
                                pause_reason = NULL,
                                next_batch_at = now() + make_interval(secs => $3::integer),
                                updated_at = now()
                            WHERE job_id = $1 AND status = 'running'
                            "#,
                        )
                        .bind(job_id)
                        .bind(next_batch)
                        .bind(i32::try_from(batch_delay_secs)?)
                        .execute(&mut *tx)
                        .await?;
                    }
                    changed += 1;
                }
                tx.commit().await?;
                Ok(changed)
            }
        }
    }
}

async fn reconcile_memory_rollouts(memory: &MemoryState, limit: i64) -> Result<usize> {
    let targets = memory.job_targets.read().await.clone();
    let assignments = memory.job_rollout_targets.read().await.clone();
    let now_unix = unix_now();
    let now = now_unix.to_string();
    let mut changed = 0;
    let mut rollouts = memory.job_rollouts.write().await;
    for rollout in rollouts
        .iter_mut()
        .filter(|rollout| rollout.status == ROLLOUT_STATUS_RUNNING)
        .take(limit.clamp(1, 500) as usize)
    {
        let current_targets = targets
            .iter()
            .filter(|target| {
                target.job_id == rollout.job_id
                    && assignments.get(&(target.job_id, target.client_id.clone()))
                        == Some(&rollout.current_batch)
            })
            .collect::<Vec<_>>();
        if current_targets.is_empty() {
            rollout.status = ROLLOUT_STATUS_PAUSED.to_string();
            rollout.pause_reason = Some(ROLLOUT_PAUSE_CURRENT_BATCH_ASSIGNMENT_MISSING.to_string());
            rollout.next_batch_unix = now_unix;
            rollout.updated_at = now.clone();
            changed += 1;
            continue;
        }
        if current_targets
            .iter()
            .any(|target| target.completed_at.is_none())
        {
            continue;
        }
        let failure_count = targets
            .iter()
            .filter(|target| {
                target.job_id == rollout.job_id
                    && target.completed_at.is_some()
                    && target.status != "completed"
                    && assignments
                        .get(&(target.job_id, target.client_id.clone()))
                        .is_some_and(|batch| *batch <= rollout.current_batch)
            })
            .count();
        let failure_count = u16::try_from(failure_count)?;
        let next_batch = rollout.current_batch + 1;
        if next_batch >= rollout.total_batches {
            rollout.status = ROLLOUT_STATUS_COMPLETED.to_string();
            rollout.pause_reason = None;
            rollout.completed_at = Some(now.clone());
        } else if failure_count.saturating_sub(rollout.failure_baseline)
            > rollout.policy.max_failures
        {
            rollout.status = ROLLOUT_STATUS_PAUSED.to_string();
            rollout.current_batch = next_batch;
            rollout.pause_reason = Some("failure_threshold".to_string());
            rollout.next_batch_unix = now_unix;
        } else if rollout.current_batch == 0 && rollout.policy.pause_after_canary {
            rollout.status = ROLLOUT_STATUS_PAUSED.to_string();
            rollout.current_batch = next_batch;
            rollout.pause_reason = Some("canary_review".to_string());
            rollout.next_batch_unix = now_unix;
        } else {
            rollout.current_batch = next_batch;
            rollout.pause_reason = None;
            rollout.next_batch_unix = now_unix + u64::from(rollout.policy.batch_delay_secs);
        }
        rollout.updated_at = now.clone();
        changed += 1;
    }
    Ok(changed)
}

async fn memory_rollout_view(
    memory: &MemoryState,
    record: &MemoryJobRolloutRecord,
) -> Result<JobRolloutView> {
    let assignments = memory.job_rollout_targets.read().await;
    let targets = memory.job_targets.read().await;
    let mut target_views = targets
        .iter()
        .filter(|target| target.job_id == record.job_id)
        .filter_map(|target| {
            assignments
                .get(&(record.job_id, target.client_id.clone()))
                .copied()
                .map(|batch_index| JobRolloutTargetView {
                    client_id: target.client_id.clone(),
                    batch_index,
                    status: target.status.clone(),
                    message: target.message.clone(),
                })
        })
        .collect::<Vec<_>>();
    target_views.sort_by(|left, right| {
        left.batch_index
            .cmp(&right.batch_index)
            .then_with(|| left.client_id.cmp(&right.client_id))
    });
    ensure!(
        target_views.len()
            == assignments
                .keys()
                .filter(|(job_id, _)| *job_id == record.job_id)
                .count(),
        "job_rollout_target_evidence_incomplete"
    );
    Ok(JobRolloutView {
        job_id: record.job_id,
        status: record.status.clone(),
        canary_client_ids: record.policy.canary_client_ids.clone(),
        batch_size: record.policy.batch_size,
        max_failures: record.policy.max_failures,
        pause_after_canary: record.policy.pause_after_canary,
        batch_delay_secs: record.policy.batch_delay_secs,
        current_batch: record.current_batch,
        total_batches: record.total_batches,
        failure_baseline: record.failure_baseline,
        pause_reason: record.pause_reason.clone(),
        next_batch_at: record.next_batch_unix.to_string(),
        created_at: record.created_at.clone(),
        updated_at: record.updated_at.clone(),
        completed_at: record.completed_at.clone(),
        targets: target_views,
    })
}

async fn memory_rollout_failure_count(memory: &MemoryState, job_id: Uuid) -> Result<u16> {
    let current_batch = memory
        .job_rollouts
        .read()
        .await
        .iter()
        .find(|rollout| rollout.job_id == job_id)
        .map(|rollout| rollout.current_batch)
        .ok_or_else(|| anyhow::anyhow!("job_rollout_not_found"))?;
    let assignments = memory.job_rollout_targets.read().await;
    let targets = memory.job_targets.read().await;
    u16::try_from(
        targets
            .iter()
            .filter(|target| {
                target.job_id == job_id
                    && target.completed_at.is_some()
                    && target.status != "completed"
                    && assignments
                        .get(&(job_id, target.client_id.clone()))
                        .is_some_and(|batch| *batch <= current_batch)
            })
            .count(),
    )
    .map_err(Into::into)
}

fn postgres_rollout_select() -> &'static str {
    r#"
    SELECT
        rollout.job_id,
        rollout.status,
        rollout.canary_client_ids,
        rollout.batch_size,
        rollout.max_failures,
        rollout.pause_after_canary,
        rollout.batch_delay_secs,
        rollout.current_batch,
        rollout.total_batches,
        rollout.failure_baseline,
        rollout.pause_reason,
        rollout.next_batch_at::text AS next_batch_at,
        rollout.created_at::text AS created_at,
        rollout.updated_at::text AS updated_at,
        rollout.completed_at::text AS completed_at
    FROM job_rollouts rollout
    "#
}

async fn postgres_rollout_view(pool: &sqlx::PgPool, row: PgRow) -> Result<JobRolloutView> {
    let job_id: Uuid = row.try_get("job_id")?;
    let target_rows = sqlx::query(
        r#"
        SELECT
            assignment.client_id,
            assignment.batch_index,
            target.status,
            target.message
        FROM job_rollout_targets assignment
        JOIN job_targets target
          ON target.job_id = assignment.job_id
         AND target.client_id = assignment.client_id
        WHERE assignment.job_id = $1
        ORDER BY assignment.batch_index, assignment.client_id
        "#,
    )
    .bind(job_id)
    .fetch_all(pool)
    .await?;
    postgres_rollout_view_from_rows(row, target_rows)
}

async fn postgres_rollout_view_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job_id: Uuid,
) -> Result<Option<JobRolloutView>> {
    let row = sqlx::query(&format!(
        "{} WHERE rollout.job_id = $1",
        postgres_rollout_select()
    ))
    .bind(job_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let target_rows = sqlx::query(
        r#"
        SELECT
            assignment.client_id,
            assignment.batch_index,
            target.status,
            target.message
        FROM job_rollout_targets assignment
        JOIN job_targets target
          ON target.job_id = assignment.job_id
         AND target.client_id = assignment.client_id
        WHERE assignment.job_id = $1
        ORDER BY assignment.batch_index, assignment.client_id
        "#,
    )
    .bind(job_id)
    .fetch_all(&mut **tx)
    .await?;
    postgres_rollout_view_from_rows(row, target_rows).map(Some)
}

fn postgres_rollout_view_from_rows(row: PgRow, target_rows: Vec<PgRow>) -> Result<JobRolloutView> {
    let job_id: Uuid = row.try_get("job_id")?;
    let targets = target_rows
        .into_iter()
        .map(|target| {
            Ok(JobRolloutTargetView {
                client_id: target.try_get("client_id")?,
                batch_index: u16::try_from(target.try_get::<i32, _>("batch_index")?)?,
                status: target.try_get("status")?,
                message: target.try_get("message")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        !targets.is_empty(),
        "job_rollout_target_evidence_incomplete"
    );
    Ok(JobRolloutView {
        job_id,
        status: row.try_get("status")?,
        canary_client_ids: row.try_get("canary_client_ids")?,
        batch_size: u16::try_from(row.try_get::<i32, _>("batch_size")?)?,
        max_failures: u16::try_from(row.try_get::<i32, _>("max_failures")?)?,
        pause_after_canary: row.try_get("pause_after_canary")?,
        batch_delay_secs: u32::try_from(row.try_get::<i64, _>("batch_delay_secs")?)?,
        current_batch: u16::try_from(row.try_get::<i32, _>("current_batch")?)?,
        total_batches: u16::try_from(row.try_get::<i32, _>("total_batches")?)?,
        failure_baseline: u16::try_from(row.try_get::<i32, _>("failure_baseline")?)?,
        pause_reason: row.try_get("pause_reason")?,
        next_batch_at: row.try_get("next_batch_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        completed_at: row.try_get("completed_at")?,
        targets,
    })
}

async fn push_memory_rollout_audit(
    memory: &MemoryState,
    operator: &AuthContext,
    job_id: Uuid,
    action: &str,
    reason: &str,
    now: &str,
) {
    memory.audits.write().await.push(AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: action.to_string(),
        target: format!("job:{job_id}"),
        command_hash: None,
        metadata: json!({
            "job_id": job_id,
            "reason": reason,
            "result": "succeeded",
            "operator_id": operator.operator.id,
            "operator_username": operator.operator.username,
            "operator_role": operator.operator.role,
            "operator_session_id": operator.audit_session_id(),
            "origin_kind": "operator_request",
            "component": "job-rollout-controller",
        }),
        created_at: now.to_string(),
    });
}

async fn insert_postgres_rollout_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operator: &AuthContext,
    job_id: Uuid,
    action: &str,
    reason: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
        VALUES ($1, $2, $3, $4, NULL, $5)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(operator.operator.id)
    .bind(action)
    .bind(format!("job:{job_id}"))
    .bind(json!({
        "job_id": job_id,
        "reason": reason,
        "result": "succeeded",
        "operator_id": operator.operator.id,
        "operator_username": operator.operator.username,
        "operator_role": operator.operator.role,
        "operator_session_id": operator.audit_session_id(),
        "origin_kind": "operator_request",
        "component": "job-rollout-controller",
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn normalized_reason(reason: Option<&str>, fallback: &str) -> String {
    reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .chars()
        .take(512)
        .collect()
}

fn is_rollout_terminal(status: &str) -> bool {
    matches!(status, ROLLOUT_STATUS_COMPLETED | ROLLOUT_STATUS_ABORTED)
}

#[cfg(test)]
#[path = "tests_repository_job_rollouts.rs"]
mod tests;
