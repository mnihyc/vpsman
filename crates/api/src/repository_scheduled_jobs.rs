use anyhow::Result;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;
use vpsman_common::JobCommand;

use crate::{
    model::{AuditLogView, AuthContext, ScheduledJobDispatchRecord},
    repository::Repository,
    unix_now,
};

impl Repository {
    pub(crate) async fn load_scheduled_job_for_dispatch(
        &self,
        job_id: Uuid,
    ) -> Result<Option<ScheduledJobDispatchRecord>> {
        match self {
            Self::Memory(memory) => {
                let scheduled = memory.scheduled_jobs.read().await;
                let Some(record) = scheduled.get(&job_id) else {
                    return Ok(None);
                };
                let jobs = memory.jobs.read().await;
                let Some(job) = jobs.iter().find(|job| job.id == job_id) else {
                    return Ok(None);
                };
                if job.status != "approval_required" {
                    return Ok(None);
                }
                Ok(Some(record.clone()))
            }
            Self::Postgres(pool) => {
                let Some(row) = sqlx::query(
                    r#"
                    SELECT
                        id,
                        source_schedule_id,
                        actor_id,
                        command_type,
                        operation,
                        payload_hash
                    FROM jobs
                    WHERE id = $1
                      AND status = 'approval_required'
                      AND source_schedule_id IS NOT NULL
                    "#,
                )
                .bind(job_id)
                .fetch_optional(pool)
                .await?
                else {
                    return Ok(None);
                };
                let targets = sqlx::query(
                    r#"
                    SELECT client_id
                    FROM job_targets
                    WHERE job_id = $1 AND status = 'approval_required'
                    ORDER BY client_id
                    "#,
                )
                .bind(job_id)
                .fetch_all(pool)
                .await?
                .into_iter()
                .map(|row| row.try_get("client_id").map_err(Into::into))
                .collect::<Result<Vec<String>>>()?;
                let operation: sqlx::types::Json<JobCommand> = row.try_get("operation")?;
                Ok(Some(ScheduledJobDispatchRecord {
                    job_id: row.try_get("id")?,
                    source_schedule_id: row.try_get("source_schedule_id")?,
                    actor_id: row.try_get("actor_id")?,
                    command_type: row.try_get("command_type")?,
                    operation: operation.0,
                    payload_hash: row.try_get("payload_hash")?,
                    targets,
                }))
            }
        }
    }

    pub(crate) async fn mark_scheduled_job_dispatching(
        &self,
        scheduled: &ScheduledJobDispatchRecord,
        operator: &AuthContext,
        envelope_count: usize,
    ) -> Result<()> {
        let metadata = json!({
            "scheduled_job_id": scheduled.job_id,
            "scheduled_actor_id": scheduled.actor_id,
            "operation_type": scheduled.command_type,
            "resolved_targets": &scheduled.targets,
            "approved_by_operator_id": operator.operator.id,
            "approved_by_operator_username": operator.operator.username,
            "session_id": operator.session_id,
            "envelope_count": envelope_count,
        });
        match self {
            Self::Memory(memory) => {
                if let Some(job) = memory
                    .jobs
                    .write()
                    .await
                    .iter_mut()
                    .find(|job| job.id == scheduled.job_id)
                {
                    job.status = "dispatching".to_string();
                }
                for target in memory
                    .job_targets
                    .write()
                    .await
                    .iter_mut()
                    .filter(|target| target.job_id == scheduled.job_id)
                {
                    target.status = "queued".to_string();
                }
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "schedule.dispatch_approved".to_string(),
                    target: format!("job:{}", scheduled.job_id),
                    command_hash: Some(scheduled.payload_hash.clone()),
                    metadata,
                    created_at: unix_now().to_string(),
                });
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let updated = sqlx::query(
                    r#"
                    UPDATE jobs
                    SET status = 'dispatching'
                    WHERE id = $1
                      AND status = 'approval_required'
                      AND source_schedule_id IS NOT NULL
                    "#,
                )
                .bind(scheduled.job_id)
                .execute(&mut *tx)
                .await?
                .rows_affected();
                anyhow::ensure!(updated == 1, "scheduled job is no longer approval-required");
                sqlx::query(
                    r#"
                    UPDATE job_targets
                    SET status = 'queued'
                    WHERE job_id = $1 AND status = 'approval_required'
                    "#,
                )
                .bind(scheduled.job_id)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, $5, $6)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("schedule.dispatch_approved")
                .bind(format!("job:{}", scheduled.job_id))
                .bind(&scheduled.payload_hash)
                .bind(metadata)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
            }
        }
        Ok(())
    }
}
