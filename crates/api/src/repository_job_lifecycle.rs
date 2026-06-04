use anyhow::Result;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    job_lifecycle::{
        job_status_is_active_cancelable, job_status_is_cancelable, ActiveJobCancellationRecord,
        JobCancellationRecord, CANCELED_STATUS, CANCEL_REQUESTED_STATUS,
    },
    model::{AuditLogView, AuthContext},
    repository::Repository,
    unix_now,
};

impl Repository {
    pub(crate) async fn cancel_pending_job(
        &self,
        job_id: Uuid,
        operator: &AuthContext,
        reason: Option<&str>,
    ) -> Result<Option<JobCancellationRecord>> {
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut jobs = memory.jobs.write().await;
                let Some(job) = jobs.iter_mut().find(|job| job.id == job_id) else {
                    return Ok(None);
                };
                if job.status == CANCELED_STATUS || !job_status_is_cancelable(&job.status) {
                    return Ok(Some(JobCancellationRecord {
                        job_id,
                        canceled: false,
                        status: job.status.clone(),
                        canceled_targets: 0,
                    }));
                }
                let previous_status = job.status.clone();
                let command_hash = job.payload_hash.clone();
                job.status = CANCELED_STATUS.to_string();
                job.completed_at = Some(now.clone());
                drop(jobs);

                let mut canceled_targets = 0_i64;
                for target in memory
                    .job_targets
                    .write()
                    .await
                    .iter_mut()
                    .filter(|target| target.job_id == job_id)
                {
                    if matches!(target.status.as_str(), "approval_required" | "queued") {
                        target.status = CANCELED_STATUS.to_string();
                        target.completed_at = Some(now.clone());
                        canceled_targets += 1;
                    }
                }
                memory.scheduled_jobs.write().await.remove(&job_id);
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "job.canceled".to_string(),
                    target: format!("job:{job_id}"),
                    command_hash: Some(command_hash),
                    metadata: cancellation_metadata(
                        job_id,
                        &previous_status,
                        canceled_targets,
                        reason,
                        operator,
                    ),
                    created_at: now,
                });
                Ok(Some(JobCancellationRecord {
                    job_id,
                    canceled: true,
                    status: CANCELED_STATUS.to_string(),
                    canceled_targets,
                }))
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let Some(row) = sqlx::query(
                    r#"
                    SELECT status, payload_hash
                    FROM jobs
                    WHERE id = $1
                    FOR UPDATE
                    "#,
                )
                .bind(job_id)
                .fetch_optional(&mut *tx)
                .await?
                else {
                    tx.commit().await?;
                    return Ok(None);
                };
                let status: String = row.try_get("status")?;
                let command_hash: String = row.try_get("payload_hash")?;
                if status == CANCELED_STATUS || !job_status_is_cancelable(&status) {
                    tx.commit().await?;
                    return Ok(Some(JobCancellationRecord {
                        job_id,
                        canceled: false,
                        status,
                        canceled_targets: 0,
                    }));
                }

                sqlx::query(
                    r#"
                    UPDATE jobs
                    SET status = $2, completed_at = now()
                    WHERE id = $1
                    "#,
                )
                .bind(job_id)
                .bind(CANCELED_STATUS)
                .execute(&mut *tx)
                .await?;
                let canceled_targets = sqlx::query(
                    r#"
                    UPDATE job_targets
                    SET status = $2, completed_at = now()
                    WHERE job_id = $1
                      AND status IN ('approval_required', 'queued')
                    "#,
                )
                .bind(job_id)
                .bind(CANCELED_STATUS)
                .execute(&mut *tx)
                .await?
                .rows_affected() as i64;
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
                .bind("job.canceled")
                .bind(format!("job:{job_id}"))
                .bind(&command_hash)
                .bind(cancellation_metadata(
                    job_id,
                    &status,
                    canceled_targets,
                    reason,
                    operator,
                ))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(Some(JobCancellationRecord {
                    job_id,
                    canceled: true,
                    status: CANCELED_STATUS.to_string(),
                    canceled_targets,
                }))
            }
        }
    }

    pub(crate) async fn request_active_job_cancel(
        &self,
        job_id: Uuid,
        operator: &AuthContext,
        reason: Option<&str>,
    ) -> Result<Option<ActiveJobCancellationRecord>> {
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut jobs = memory.jobs.write().await;
                let Some(job) = jobs.iter_mut().find(|job| job.id == job_id) else {
                    return Ok(None);
                };
                if !job_status_is_active_cancelable(&job.status) {
                    return Ok(Some(ActiveJobCancellationRecord {
                        job_id,
                        requested: false,
                        status: job.status.clone(),
                        target_clients: Vec::new(),
                    }));
                }
                let previous_status = job.status.clone();
                let command_hash = job.payload_hash.clone();
                job.status = CANCEL_REQUESTED_STATUS.to_string();
                drop(jobs);

                let target_clients = memory
                    .job_targets
                    .read()
                    .await
                    .iter()
                    .filter(|target| {
                        target.job_id == job_id
                            && matches!(
                                target.status.as_str(),
                                "queued" | "dispatching" | "accepted"
                            )
                    })
                    .map(|target| target.client_id.clone())
                    .collect::<Vec<_>>();
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "job.cancel_requested".to_string(),
                    target: format!("job:{job_id}"),
                    command_hash: Some(command_hash),
                    metadata: active_cancellation_metadata(
                        job_id,
                        &previous_status,
                        target_clients.len() as i64,
                        reason,
                        operator,
                    ),
                    created_at: now,
                });
                Ok(Some(ActiveJobCancellationRecord {
                    job_id,
                    requested: true,
                    status: CANCEL_REQUESTED_STATUS.to_string(),
                    target_clients,
                }))
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let Some(row) = sqlx::query(
                    r#"
                    SELECT status, payload_hash
                    FROM jobs
                    WHERE id = $1
                    FOR UPDATE
                    "#,
                )
                .bind(job_id)
                .fetch_optional(&mut *tx)
                .await?
                else {
                    tx.commit().await?;
                    return Ok(None);
                };
                let status: String = row.try_get("status")?;
                let command_hash: String = row.try_get("payload_hash")?;
                if !job_status_is_active_cancelable(&status) {
                    tx.commit().await?;
                    return Ok(Some(ActiveJobCancellationRecord {
                        job_id,
                        requested: false,
                        status,
                        target_clients: Vec::new(),
                    }));
                }
                sqlx::query(
                    r#"
                    UPDATE jobs
                    SET status = $2
                    WHERE id = $1
                    "#,
                )
                .bind(job_id)
                .bind(CANCEL_REQUESTED_STATUS)
                .execute(&mut *tx)
                .await?;
                let target_rows = sqlx::query(
                    r#"
                    SELECT client_id
                    FROM job_targets
                    WHERE job_id = $1
                      AND status IN ('queued', 'dispatching', 'accepted')
                    ORDER BY client_id
                    "#,
                )
                .bind(job_id)
                .fetch_all(&mut *tx)
                .await?;
                let target_clients = target_rows
                    .into_iter()
                    .map(|row| row.try_get::<String, _>("client_id"))
                    .collect::<Result<Vec<_>, _>>()?;
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
                .bind("job.cancel_requested")
                .bind(format!("job:{job_id}"))
                .bind(&command_hash)
                .bind(active_cancellation_metadata(
                    job_id,
                    &status,
                    target_clients.len() as i64,
                    reason,
                    operator,
                ))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(Some(ActiveJobCancellationRecord {
                    job_id,
                    requested: true,
                    status: CANCEL_REQUESTED_STATUS.to_string(),
                    target_clients,
                }))
            }
        }
    }
}

fn cancellation_metadata(
    job_id: Uuid,
    previous_status: &str,
    canceled_targets: i64,
    reason: Option<&str>,
    operator: &AuthContext,
) -> serde_json::Value {
    json!({
        "job_id": job_id,
        "previous_status": previous_status,
        "status": CANCELED_STATUS,
        "canceled_targets": canceled_targets,
        "reason": reason,
        "operator_id": operator.operator.id,
        "operator_username": operator.operator.username,
        "operator_role": operator.operator.role,
        "session_id": operator.session_id,
    })
}

fn active_cancellation_metadata(
    job_id: Uuid,
    previous_status: &str,
    requested_targets: i64,
    reason: Option<&str>,
    operator: &AuthContext,
) -> serde_json::Value {
    json!({
        "job_id": job_id,
        "previous_status": previous_status,
        "status": CANCEL_REQUESTED_STATUS,
        "requested_targets": requested_targets,
        "reason": reason,
        "operator_id": operator.operator.id,
        "operator_username": operator.operator.username,
        "operator_role": operator.operator.role,
        "session_id": operator.session_id,
    })
}
