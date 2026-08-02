use anyhow::Result;
use sqlx::Row;

use crate::{
    model_host_management::{HostJobAttemptView, HostJobEvidence},
    repository::Repository,
};

impl Repository {
    pub(crate) async fn host_process_job_evidence(
        &self,
        client_id: &str,
    ) -> Result<HostJobEvidence> {
        self.host_job_evidence(client_id, "process_list").await
    }

    pub(crate) async fn host_service_job_evidence(
        &self,
        client_id: &str,
    ) -> Result<HostJobEvidence> {
        self.host_job_evidence(client_id, "service_inventory").await
    }

    pub(crate) async fn host_storage_job_evidence(
        &self,
        client_id: &str,
    ) -> Result<HostJobEvidence> {
        self.host_job_evidence(client_id, "storage_inventory").await
    }

    pub(crate) async fn host_package_plan_job_evidence(
        &self,
        client_id: &str,
    ) -> Result<HostJobEvidence> {
        self.host_job_evidence(client_id, "package_update_plan")
            .await
    }

    async fn host_job_evidence(
        &self,
        client_id: &str,
        command_type: &str,
    ) -> Result<HostJobEvidence> {
        match self {
            Self::Memory(memory) => {
                let jobs = memory.jobs.read().await;
                let targets = memory.job_targets.read().await;
                let mut attempts = targets
                    .iter()
                    .filter(|target| target.client_id == client_id)
                    .filter_map(|target| {
                        let job = jobs.iter().find(|job| {
                            job.id == target.job_id && job.command_type == command_type
                        })?;
                        Some((job, target))
                    })
                    .collect::<Vec<_>>();
                attempts.sort_by(|(left_job, left_target), (right_job, right_target)| {
                    right_target
                        .completed_at
                        .as_ref()
                        .unwrap_or(&right_job.created_at)
                        .cmp(
                            left_target
                                .completed_at
                                .as_ref()
                                .unwrap_or(&left_job.created_at),
                        )
                        .then_with(|| right_job.id.cmp(&left_job.id))
                });
                let attempts = attempts
                    .into_iter()
                    .map(|(job, target)| HostJobAttemptView {
                        job_id: job.id,
                        status: target.status.clone(),
                        message: target.message.clone(),
                        completed_at: target.completed_at.clone(),
                    })
                    .collect::<Vec<_>>();
                Ok(host_job_evidence_from_newest_attempts(&attempts))
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        latest_attempt.job_id,
                        latest_attempt.status,
                        latest_attempt.message,
                        latest_attempt.completed_at,
                        latest_success.job_id AS latest_success_job_id
                    FROM LATERAL (
                        SELECT
                            job.id AS job_id,
                            target.status,
                            target.message,
                            target.completed_at::text AS completed_at
                        FROM jobs job
                        JOIN job_targets target ON target.job_id = job.id
                        WHERE job.command_type = $2
                          AND target.client_id = $1
                        ORDER BY
                            COALESCE(target.completed_at, job.created_at) DESC,
                            job.id DESC
                        LIMIT 1
                    ) latest_attempt
                    LEFT JOIN LATERAL (
                        SELECT job.id AS job_id
                        FROM jobs job
                        JOIN job_targets target ON target.job_id = job.id
                        WHERE job.command_type = $2
                          AND target.client_id = $1
                          AND target.status = 'completed'
                        ORDER BY
                            COALESCE(target.completed_at, job.created_at) DESC,
                            job.id DESC
                        LIMIT 1
                    ) latest_success ON true
                    "#,
                )
                .bind(client_id)
                .bind(command_type)
                .fetch_optional(pool)
                .await?;
                let latest_attempt = if let Some(row) = row.as_ref() {
                    Some(HostJobAttemptView {
                        job_id: row.try_get("job_id")?,
                        status: row.try_get("status")?,
                        message: row.try_get("message")?,
                        completed_at: row.try_get("completed_at")?,
                    })
                } else {
                    None
                };
                let latest_success_job_id = row
                    .as_ref()
                    .map(|row| row.try_get("latest_success_job_id"))
                    .transpose()?
                    .flatten();
                Ok(HostJobEvidence {
                    latest_attempt,
                    latest_success_job_id,
                })
            }
        }
    }
}

fn host_job_evidence_from_newest_attempts(attempts: &[HostJobAttemptView]) -> HostJobEvidence {
    HostJobEvidence {
        latest_attempt: attempts.first().cloned(),
        latest_success_job_id: attempts
            .iter()
            .find(|attempt| attempt.status == "completed")
            .map(|attempt| attempt.job_id),
    }
}

#[cfg(test)]
#[path = "tests_repository_host_management.rs"]
mod tests;
