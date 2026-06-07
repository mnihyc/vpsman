use std::cmp::Ordering;

use anyhow::Result;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;
use vpsman_common::JobCommand;

use crate::model::*;
use crate::model_rollout_policies::ResolvedAgentUpdateRolloutPolicy;
use crate::repository::Repository;
use crate::repository_rollouts::{
    insert_postgres_agent_update_rollout, record_memory_agent_update_rollout,
    rollout_status_for_job_status, rollout_target_summary,
};
use crate::util::{limit_or_default, offset_or_default, search_pattern, sort_descending};
use crate::{unix_now, TargetDispatchOutcome};

fn agent_update_activation_failure_status(status: &str) -> bool {
    matches!(
        status,
        "failed" | "dispatch_failed" | "rejected_by_agent" | "timed_out" | "canceled"
    )
}

fn compare_text_or_number(left: &str, right: &str) -> Ordering {
    match (left.parse::<i128>(), right.parse::<i128>()) {
        (Ok(left), Ok(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

fn compare_job_history(
    left: &JobHistoryView,
    right: &JobHistoryView,
    sort: Option<&str>,
) -> Ordering {
    match sort.unwrap_or("created_at") {
        "actor_id" => left.actor_id.cmp(&right.actor_id),
        "command_type" | "command" => left.command_type.cmp(&right.command_type),
        "payload_hash" | "hash" => left.payload_hash.cmp(&right.payload_hash),
        "privileged" => left.privileged.cmp(&right.privileged),
        "status" => left.status.cmp(&right.status),
        "target_count" | "targets" => left.target_count.cmp(&right.target_count),
        "completed_at" => left.completed_at.cmp(&right.completed_at),
        _ => compare_text_or_number(&left.created_at, &right.created_at),
    }
}

fn job_matches_search(job: &JobHistoryView, needle: &str) -> bool {
    job.id.to_string().to_ascii_lowercase().contains(needle)
        || job
            .actor_id
            .map(|id| id.to_string().to_ascii_lowercase().contains(needle))
            .unwrap_or(false)
        || job.command_type.to_ascii_lowercase().contains(needle)
        || job.status.to_ascii_lowercase().contains(needle)
        || job.payload_hash.to_ascii_lowercase().contains(needle)
}

fn job_history_order_by(sort: Option<&str>, descending: bool) -> &'static str {
    match (sort.unwrap_or("created_at"), descending) {
        ("actor_id", true) => "actor_id DESC NULLS LAST, id DESC",
        ("actor_id", false) => "actor_id ASC NULLS LAST, id ASC",
        ("command_type" | "command", true) => "command_type DESC, id DESC",
        ("command_type" | "command", false) => "command_type ASC, id ASC",
        ("payload_hash" | "hash", true) => "payload_hash DESC, id DESC",
        ("payload_hash" | "hash", false) => "payload_hash ASC, id ASC",
        ("privileged", true) => "privileged DESC, id DESC",
        ("privileged", false) => "privileged ASC, id ASC",
        ("status", true) => "status DESC, id DESC",
        ("status", false) => "status ASC, id ASC",
        ("target_count" | "targets", true) => "target_count DESC, id DESC",
        ("target_count" | "targets", false) => "target_count ASC, id ASC",
        ("completed_at", true) => "completed_at DESC NULLS LAST, id DESC",
        ("completed_at", false) => "completed_at ASC NULLS LAST, id ASC",
        (_, true) => "created_at DESC, id DESC",
        (_, false) => "created_at ASC, id ASC",
    }
}

fn compare_audit_log(left: &AuditLogView, right: &AuditLogView, sort: Option<&str>) -> Ordering {
    match sort.unwrap_or("created_at") {
        "actor_id" | "operator" => left.actor_id.cmp(&right.actor_id),
        "action" => left.action.cmp(&right.action),
        "command_hash" | "hash" => left.command_hash.cmp(&right.command_hash),
        "target" => left.target.cmp(&right.target),
        _ => compare_text_or_number(&left.created_at, &right.created_at),
    }
}

fn audit_matches_search(audit: &AuditLogView, needle: &str) -> bool {
    audit.id.to_string().to_ascii_lowercase().contains(needle)
        || audit
            .actor_id
            .map(|id| id.to_string().to_ascii_lowercase().contains(needle))
            .unwrap_or(false)
        || audit.action.to_ascii_lowercase().contains(needle)
        || audit.target.to_ascii_lowercase().contains(needle)
        || audit
            .command_hash
            .as_deref()
            .map(|value| value.to_ascii_lowercase().contains(needle))
            .unwrap_or(false)
}

fn audit_log_order_by(sort: Option<&str>, descending: bool) -> &'static str {
    match (sort.unwrap_or("created_at"), descending) {
        ("actor_id" | "operator", true) => "actor_id DESC NULLS LAST, id DESC",
        ("actor_id" | "operator", false) => "actor_id ASC NULLS LAST, id ASC",
        ("action", true) => "action DESC, id DESC",
        ("action", false) => "action ASC, id ASC",
        ("command_hash" | "hash", true) => "command_hash DESC NULLS LAST, id DESC",
        ("command_hash" | "hash", false) => "command_hash ASC NULLS LAST, id ASC",
        ("target", true) => "target DESC, id DESC",
        ("target", false) => "target ASC, id ASC",
        (_, true) => "created_at DESC, id DESC",
        (_, false) => "created_at ASC, id ASC",
    }
}

impl Repository {
    pub(crate) async fn find_job_by_idempotency_key(
        &self,
        actor_id: Uuid,
        idempotency_key: &str,
    ) -> Result<Option<JobHistoryView>> {
        match self {
            Self::Memory(memory) => {
                let Some(job_id) = memory
                    .job_idempotency_keys
                    .read()
                    .await
                    .get(&(actor_id, idempotency_key.to_string()))
                    .copied()
                else {
                    return Ok(None);
                };
                Ok(memory
                    .jobs
                    .read()
                    .await
                    .iter()
                    .find(|job| job.id == job_id)
                    .cloned())
            }
            Self::Postgres(pool) => {
                let Some(row) = sqlx::query(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        command_type,
                        privileged,
                        status,
                        target_count,
                        payload_hash,
                        created_at::text AS created_at,
                        completed_at::text AS completed_at
                    FROM jobs
                    WHERE actor_id = $1
                      AND idempotency_key = $2
                    ORDER BY created_at DESC
                    LIMIT 1
                    "#,
                )
                .bind(actor_id)
                .bind(idempotency_key)
                .fetch_optional(pool)
                .await?
                else {
                    return Ok(None);
                };
                Ok(Some(JobHistoryView {
                    id: row.try_get("id")?,
                    actor_id: row.try_get("actor_id")?,
                    command_type: row.try_get("command_type")?,
                    privileged: row.try_get("privileged")?,
                    status: row.try_get("status")?,
                    target_count: row.try_get("target_count")?,
                    payload_hash: row.try_get("payload_hash")?,
                    created_at: row.try_get("created_at")?,
                    completed_at: row.try_get("completed_at")?,
                }))
            }
        }
    }

    pub(crate) async fn get_job(&self, job_id: Uuid) -> Result<Option<JobHistoryView>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .jobs
                .read()
                .await
                .iter()
                .find(|job| job.id == job_id)
                .cloned()),
            Self::Postgres(pool) => {
                let Some(row) = sqlx::query(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        command_type,
                        privileged,
                        status,
                        target_count,
                        payload_hash,
                        created_at::text AS created_at,
                        completed_at::text AS completed_at
                    FROM jobs
                    WHERE id = $1
                    "#,
                )
                .bind(job_id)
                .fetch_optional(pool)
                .await?
                else {
                    return Ok(None);
                };
                Ok(Some(JobHistoryView {
                    id: row.try_get("id")?,
                    actor_id: row.try_get("actor_id")?,
                    command_type: row.try_get("command_type")?,
                    privileged: row.try_get("privileged")?,
                    status: row.try_get("status")?,
                    target_count: row.try_get("target_count")?,
                    payload_hash: row.try_get("payload_hash")?,
                    created_at: row.try_get("created_at")?,
                    completed_at: row.try_get("completed_at")?,
                }))
            }
        }
    }

    pub(crate) async fn list_jobs(&self, limit: i64) -> Result<Vec<JobHistoryView>> {
        match self {
            Self::Memory(memory) => {
                let jobs = memory.jobs.read().await;
                Ok(jobs.iter().rev().take(limit as usize).cloned().collect())
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        command_type,
                        privileged,
                        status,
                        target_count,
                        payload_hash,
                        created_at::text AS created_at,
                        completed_at::text AS completed_at
                    FROM jobs
                    ORDER BY created_at DESC, id DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(JobHistoryView {
                            id: row.try_get("id")?,
                            actor_id: row.try_get("actor_id")?,
                            command_type: row.try_get("command_type")?,
                            privileged: row.try_get("privileged")?,
                            status: row.try_get("status")?,
                            target_count: row.try_get("target_count")?,
                            payload_hash: row.try_get("payload_hash")?,
                            created_at: row.try_get("created_at")?,
                            completed_at: row.try_get("completed_at")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn query_jobs(&self, query: &ListQuery) -> Result<Vec<JobHistoryView>> {
        let limit = limit_or_default(query.limit);
        let offset = offset_or_default(query.offset);
        let descending = sort_descending(query.dir.as_deref(), true);
        let q = query
            .q
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        match self {
            Self::Memory(memory) => {
                let q = q.map(|value| value.to_ascii_lowercase());
                let mut jobs = memory
                    .jobs
                    .read()
                    .await
                    .iter()
                    .filter(|job| {
                        q.as_deref()
                            .map(|needle| job_matches_search(job, needle))
                            .unwrap_or(true)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                jobs.sort_by(|left, right| {
                    compare_job_history(left, right, query.sort.as_deref())
                        .then_with(|| left.id.cmp(&right.id))
                });
                if descending {
                    jobs.reverse();
                }
                Ok(jobs
                    .into_iter()
                    .skip(offset as usize)
                    .take(limit as usize)
                    .collect())
            }
            Self::Postgres(pool) => {
                let order_by = job_history_order_by(query.sort.as_deref(), descending);
                let rows = sqlx::query(&format!(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        command_type,
                        privileged,
                        status,
                        target_count,
                        payload_hash,
                        created_at::text AS created_at,
                        completed_at::text AS completed_at
                    FROM jobs
                    WHERE (
                        $3::text IS NULL
                        OR id::text ILIKE $3 ESCAPE '\'
                        OR actor_id::text ILIKE $3 ESCAPE '\'
                        OR command_type ILIKE $3 ESCAPE '\'
                        OR status ILIKE $3 ESCAPE '\'
                        OR payload_hash ILIKE $3 ESCAPE '\'
                    )
                    ORDER BY {order_by}
                    LIMIT $1
                    OFFSET $2
                    "#,
                ))
                .bind(limit)
                .bind(offset)
                .bind(search_pattern(&query.q))
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(JobHistoryView {
                            id: row.try_get("id")?,
                            actor_id: row.try_get("actor_id")?,
                            command_type: row.try_get("command_type")?,
                            privileged: row.try_get("privileged")?,
                            status: row.try_get("status")?,
                            target_count: row.try_get("target_count")?,
                            payload_hash: row.try_get("payload_hash")?,
                            created_at: row.try_get("created_at")?,
                            completed_at: row.try_get("completed_at")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn list_auth_proof_rotation_history(
        &self,
        limit: i64,
    ) -> Result<Vec<AuthProofRotationHistoryView>> {
        match self {
            Self::Memory(memory) => {
                let jobs = memory.jobs.read().await;
                let operations = memory.job_operations.read().await;
                let targets = memory.job_targets.read().await;
                let rows =
                    jobs.iter()
                        .rev()
                        .filter_map(|job| {
                            let JobCommand::AuthProofKeyRotate {
                                rotation_generation,
                                ..
                            } = operations.get(&job.id)?
                            else {
                                return None;
                            };
                            let counts = target_status_counts(
                                targets.iter().filter(|target| target.job_id == job.id).map(
                                    |target| {
                                        (
                                            target.status.as_str(),
                                            target.completed_at.as_deref().is_none(),
                                        )
                                    },
                                ),
                            );
                            Some(AuthProofRotationHistoryView {
                                job_id: job.id,
                                actor_id: job.actor_id,
                                status: job.status.clone(),
                                target_count: job.target_count,
                                completed_count: counts.completed,
                                failed_count: counts.failed,
                                pending_count: counts.pending,
                                rotation_generation: rotation_generation.clone(),
                                payload_hash: job.payload_hash.clone(),
                                created_at: job.created_at.clone(),
                                completed_at: job.completed_at.clone(),
                            })
                        })
                        .take(limit as usize)
                        .collect();
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        job.id,
                        job.actor_id,
                        job.status,
                        job.target_count,
                        job.payload_hash,
                        job.operation ->> 'rotation_generation' AS rotation_generation,
                        job.created_at::text AS created_at,
                        job.completed_at::text AS completed_at,
                        COUNT(target.client_id) FILTER (WHERE target.status = 'completed') AS completed_count,
                        COUNT(target.client_id) FILTER (
                            WHERE target.status IN ('queued', 'dispatching')
                              AND target.completed_at IS NULL
                        ) AS pending_count,
                        COUNT(target.client_id) FILTER (
                            WHERE target.client_id IS NOT NULL
                              AND target.status <> 'completed'
                              AND NOT (
                                  target.status IN ('queued', 'dispatching')
                                  AND target.completed_at IS NULL
                              )
                        ) AS failed_count
                    FROM jobs job
                    LEFT JOIN job_targets target ON target.job_id = job.id
                    WHERE job.command_type = 'auth_proof_key_rotate'
                       OR job.operation ->> 'type' = 'auth_proof_key_rotate'
                    GROUP BY job.id
                    ORDER BY job.created_at DESC, job.id DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(AuthProofRotationHistoryView {
                            job_id: row.try_get("id")?,
                            actor_id: row.try_get("actor_id")?,
                            status: row.try_get("status")?,
                            target_count: row.try_get("target_count")?,
                            completed_count: i64_to_i32(row.try_get("completed_count")?),
                            failed_count: i64_to_i32(row.try_get("failed_count")?),
                            pending_count: i64_to_i32(row.try_get("pending_count")?),
                            rotation_generation: row.try_get("rotation_generation")?,
                            payload_hash: row.try_get("payload_hash")?,
                            created_at: row.try_get("created_at")?,
                            completed_at: row.try_get("completed_at")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn list_job_targets(&self, job_id: Uuid) -> Result<Vec<JobTargetView>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .job_targets
                .read()
                .await
                .iter()
                .filter(|target| target.job_id == job_id)
                .cloned()
                .collect()),
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        job_id,
                        client_id,
                        status,
                        exit_code,
                        started_at::text AS started_at,
                        completed_at::text AS completed_at
                    FROM job_targets
                    WHERE job_id = $1
                    ORDER BY client_id
                    "#,
                )
                .bind(job_id)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(JobTargetView {
                            job_id: row.try_get("job_id")?,
                            client_id: row.try_get("client_id")?,
                            status: row.try_get("status")?,
                            exit_code: row.try_get("exit_code")?,
                            started_at: row.try_get("started_at")?,
                            completed_at: row.try_get("completed_at")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn list_audit_logs(&self, limit: i64) -> Result<Vec<AuditLogView>> {
        match self {
            Self::Memory(memory) => {
                let audits = memory.audits.read().await;
                Ok(audits.iter().rev().take(limit as usize).cloned().collect())
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        action,
                        target,
                        command_hash,
                        metadata,
                        created_at::text AS created_at
                    FROM audit_logs
                    ORDER BY created_at DESC, id DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let metadata: sqlx::types::Json<serde_json::Value> =
                            row.try_get("metadata")?;
                        Ok(AuditLogView {
                            id: row.try_get("id")?,
                            actor_id: row.try_get("actor_id")?,
                            action: row.try_get("action")?,
                            target: row.try_get("target")?,
                            command_hash: row.try_get("command_hash")?,
                            metadata: metadata.0,
                            created_at: row.try_get("created_at")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn query_audit_logs(&self, query: &ListQuery) -> Result<Vec<AuditLogView>> {
        let limit = limit_or_default(query.limit);
        let offset = offset_or_default(query.offset);
        let descending = sort_descending(query.dir.as_deref(), true);
        let q = query
            .q
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        match self {
            Self::Memory(memory) => {
                let q = q.map(|value| value.to_ascii_lowercase());
                let mut audits = memory
                    .audits
                    .read()
                    .await
                    .iter()
                    .filter(|audit| {
                        q.as_deref()
                            .map(|needle| audit_matches_search(audit, needle))
                            .unwrap_or(true)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                audits.sort_by(|left, right| {
                    compare_audit_log(left, right, query.sort.as_deref())
                        .then_with(|| left.id.cmp(&right.id))
                });
                if descending {
                    audits.reverse();
                }
                Ok(audits
                    .into_iter()
                    .skip(offset as usize)
                    .take(limit as usize)
                    .collect())
            }
            Self::Postgres(pool) => {
                let order_by = audit_log_order_by(query.sort.as_deref(), descending);
                let rows = sqlx::query(&format!(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        action,
                        target,
                        command_hash,
                        metadata,
                        created_at::text AS created_at
                    FROM audit_logs
                    WHERE (
                        $3::text IS NULL
                        OR id::text ILIKE $3 ESCAPE '\'
                        OR actor_id::text ILIKE $3 ESCAPE '\'
                        OR action ILIKE $3 ESCAPE '\'
                        OR target ILIKE $3 ESCAPE '\'
                        OR command_hash ILIKE $3 ESCAPE '\'
                    )
                    ORDER BY {order_by}
                    LIMIT $1
                    OFFSET $2
                    "#,
                ))
                .bind(limit)
                .bind(offset)
                .bind(search_pattern(&query.q))
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let metadata: sqlx::types::Json<serde_json::Value> =
                            row.try_get("metadata")?;
                        Ok(AuditLogView {
                            id: row.try_get("id")?,
                            actor_id: row.try_get("actor_id")?,
                            action: row.try_get("action")?,
                            target: row.try_get("target")?,
                            command_hash: row.try_get("command_hash")?,
                            metadata: metadata.0,
                            created_at: row.try_get("created_at")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn record_rejected_job(
        &self,
        request: &CreateJobRequest,
        command_hash: &str,
        operator: &AuthContext,
    ) -> Result<Uuid> {
        let job_id = Uuid::new_v4();
        let selection = request.target_selection();
        let resolved_agents = self.resolve_bulk_targets(&selection).await?;
        let resolved_targets = resolved_agents
            .targets
            .into_iter()
            .map(|agent| agent.id)
            .collect::<Vec<_>>();
        let metadata = json!({
            "selector_expression": request.selector_expression,
            "resolved_targets": &resolved_targets,
            "destructive": request.destructive,
            "confirmed": request.confirmed,
            "privileged": request.privileged,
            "force_unprivileged": request.force_unprivileged,
            "idempotency_key": request.idempotency_key,
            "reconnect_policy": request.reconnect_policy,
            "has_envelope": request.envelope.is_some(),
            "operator_id": operator.operator.id,
            "operator_username": operator.operator.username,
            "operator_role": operator.operator.role,
            "session_id": operator.session_id,
        });
        let operation = request.job_command().ok();
        match self {
            Self::Memory(memory) => {
                let created_at = unix_now().to_string();
                memory.jobs.write().await.push(JobHistoryView {
                    id: job_id,
                    actor_id: Some(operator.operator.id),
                    command_type: "api_job_request".to_string(),
                    privileged: request.privileged,
                    status: "rejected_authorization_required".to_string(),
                    target_count: resolved_targets.len() as i32,
                    payload_hash: command_hash.to_string(),
                    created_at: created_at.clone(),
                    completed_at: None,
                });
                if let Some(key) = request.idempotency_key.as_deref() {
                    memory
                        .job_idempotency_keys
                        .write()
                        .await
                        .insert((operator.operator.id, key.to_string()), job_id);
                }
                memory
                    .job_targets
                    .write()
                    .await
                    .extend(
                        resolved_targets
                            .iter()
                            .cloned()
                            .map(|client_id| JobTargetView {
                                job_id,
                                client_id,
                                status: "rejected_authorization_required".to_string(),
                                exit_code: None,
                                started_at: None,
                                completed_at: Some(created_at.clone()),
                            }),
                    );
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "job.rejected_authorization_required".to_string(),
                    target: "api:/api/v1/jobs".to_string(),
                    command_hash: Some(command_hash.to_string()),
                    metadata,
                    created_at,
                });
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query(
                    r#"
                    INSERT INTO jobs (
                        id, actor_id, command_type, privileged, status,
                        target_count, payload_hash, operation, idempotency_key,
                        reconnect_policy
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, COALESCE($10, '{"duplicate_delivery":"ignore_completed","resume_outputs":true,"cancel_on_disconnect":false}'::jsonb))
                    "#,
                )
                .bind(job_id)
                .bind(operator.operator.id)
                .bind("api_job_request")
                .bind(request.privileged)
                .bind("rejected_authorization_required")
                .bind(resolved_targets.len() as i32)
                .bind(command_hash)
                .bind(operation.as_ref().map(sqlx::types::Json))
                .bind(request.idempotency_key.as_deref())
                .bind(request.reconnect_policy.as_ref().map(sqlx::types::Json))
                .execute(&mut *tx)
                .await?;
                for client_id in &resolved_targets {
                    sqlx::query(
                        r#"
                        INSERT INTO job_targets (
                            job_id, client_id, status, completed_at
                        )
                        VALUES ($1, $2, $3, now())
                        "#,
                    )
                    .bind(job_id)
                    .bind(client_id)
                    .bind("rejected_authorization_required")
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
                .bind(operator.operator.id)
                .bind("job.rejected_authorization_required")
                .bind("api:/api/v1/jobs")
                .bind(command_hash)
                .bind(metadata)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
            }
        }
        Ok(job_id)
    }

    #[cfg(test)]
    pub(crate) async fn record_dispatching_job(
        &self,
        request: &CreateJobRequest,
        command_hash: &str,
        operator: &AuthContext,
        resolved_targets: &[String],
    ) -> Result<Uuid> {
        self.record_dispatching_job_with_rollout_policy(
            request,
            command_hash,
            operator,
            resolved_targets,
            &ResolvedAgentUpdateRolloutPolicy::default(),
        )
        .await
    }

    pub(crate) async fn record_dispatching_job_with_rollout_policy(
        &self,
        request: &CreateJobRequest,
        command_hash: &str,
        operator: &AuthContext,
        resolved_targets: &[String],
        rollout_policy: &ResolvedAgentUpdateRolloutPolicy,
    ) -> Result<Uuid> {
        let job_id = Uuid::new_v4();
        let metadata = json!({
            "selector_expression": request.selector_expression,
            "resolved_targets": resolved_targets,
            "destructive": request.destructive,
            "confirmed": request.confirmed,
            "privileged": request.privileged,
            "force_unprivileged": request.force_unprivileged,
            "canary_count": request.canary_count,
            "idempotency_key": request.idempotency_key,
            "reconnect_policy": request.reconnect_policy,
            "rollout_policy_id": rollout_policy.policy_id,
            "rollout_policy_name": &rollout_policy.policy_name,
            "has_legacy_envelope": request.envelope.is_some(),
            "envelope_count": request.envelopes.len(),
            "operator_id": operator.operator.id,
            "operator_username": operator.operator.username,
            "operator_role": operator.operator.role,
            "session_id": operator.session_id,
        });
        let operation = request
            .job_command()
            .map_err(|error| anyhow::anyhow!(error.code))?;
        match self {
            Self::Memory(memory) => {
                let created_at = unix_now().to_string();
                memory.jobs.write().await.push(JobHistoryView {
                    id: job_id,
                    actor_id: Some(operator.operator.id),
                    command_type: request.command_type_label().to_string(),
                    privileged: request.privileged,
                    status: "dispatching".to_string(),
                    target_count: resolved_targets.len() as i32,
                    payload_hash: command_hash.to_string(),
                    created_at: created_at.clone(),
                    completed_at: None,
                });
                if let Some(key) = request.idempotency_key.as_deref() {
                    memory
                        .job_idempotency_keys
                        .write()
                        .await
                        .insert((operator.operator.id, key.to_string()), job_id);
                }
                memory
                    .job_operations
                    .write()
                    .await
                    .insert(job_id, operation.clone());
                memory
                    .job_targets
                    .write()
                    .await
                    .extend(
                        resolved_targets
                            .iter()
                            .cloned()
                            .map(|client_id| JobTargetView {
                                job_id,
                                client_id,
                                status: "queued".to_string(),
                                exit_code: None,
                                started_at: None,
                                completed_at: None,
                            }),
                    );
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "job.dispatch_requested".to_string(),
                    target: "api:/api/v1/jobs".to_string(),
                    command_hash: Some(command_hash.to_string()),
                    metadata,
                    created_at: created_at.clone(),
                });
                if let JobCommand::UpdateAgent {
                    sha256_hex,
                    artifact_signature_hex,
                    artifact_signing_key_hex,
                    ..
                } = &operation
                {
                    record_memory_agent_update_rollout(
                        memory,
                        job_id,
                        operator,
                        command_hash,
                        resolved_targets,
                        sha256_hex,
                        artifact_signature_hex.is_some(),
                        artifact_signing_key_hex.as_deref(),
                        request
                            .canary_count
                            .or(rollout_policy.canary_count)
                            .unwrap_or(0)
                            .clamp(0, resolved_targets.len() as i32),
                        rollout_policy,
                        &created_at,
                    )
                    .await;
                }
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query(
                    r#"
                    INSERT INTO jobs (
                        id, actor_id, command_type, privileged, status,
                        target_count, payload_hash, operation, idempotency_key,
                        reconnect_policy
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, COALESCE($10, '{"duplicate_delivery":"ignore_completed","resume_outputs":true,"cancel_on_disconnect":false}'::jsonb))
                    "#,
                )
                .bind(job_id)
                .bind(operator.operator.id)
                .bind(request.command_type_label())
                .bind(request.privileged)
                .bind("dispatching")
                .bind(resolved_targets.len() as i32)
                .bind(command_hash)
                .bind(sqlx::types::Json(&operation))
                .bind(request.idempotency_key.as_deref())
                .bind(request.reconnect_policy.as_ref().map(sqlx::types::Json))
                .execute(&mut *tx)
                .await?;
                for client_id in resolved_targets {
                    sqlx::query(
                        r#"
                        INSERT INTO job_targets (
                            job_id, client_id, status
                        )
                        VALUES ($1, $2, $3)
                        "#,
                    )
                    .bind(job_id)
                    .bind(client_id)
                    .bind("queued")
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
                .bind(operator.operator.id)
                .bind("job.dispatch_requested")
                .bind("api:/api/v1/jobs")
                .bind(command_hash)
                .bind(metadata)
                .execute(&mut *tx)
                .await?;
                if let JobCommand::UpdateAgent {
                    sha256_hex,
                    artifact_signature_hex,
                    artifact_signing_key_hex,
                    ..
                } = &operation
                {
                    insert_postgres_agent_update_rollout(
                        &mut tx,
                        job_id,
                        operator,
                        command_hash,
                        resolved_targets,
                        sha256_hex,
                        artifact_signature_hex.is_some(),
                        artifact_signing_key_hex.as_deref(),
                        request
                            .canary_count
                            .or(rollout_policy.canary_count)
                            .unwrap_or(0)
                            .clamp(0, resolved_targets.len() as i32),
                        rollout_policy,
                    )
                    .await?;
                }
                tx.commit().await?;
            }
        }
        Ok(job_id)
    }

    pub(crate) async fn update_job_target_result(
        &self,
        job_id: Uuid,
        client_id: &str,
        outcome: &TargetDispatchOutcome,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let completed_at = unix_now().to_string();
                if let Some(target) = memory
                    .job_targets
                    .write()
                    .await
                    .iter_mut()
                    .find(|target| target.job_id == job_id && target.client_id == client_id)
                {
                    target.status = outcome.status.clone();
                    target.exit_code = outcome.exit_code;
                    target
                        .started_at
                        .get_or_insert_with(|| completed_at.clone());
                    target.completed_at = Some(completed_at.clone());
                }
                if let Some(rollout) = memory
                    .agent_update_rollouts
                    .write()
                    .await
                    .iter_mut()
                    .find(|rollout| rollout.job_id == job_id)
                {
                    if let Some(target) = rollout
                        .targets
                        .iter_mut()
                        .find(|target| target.client_id == client_id)
                    {
                        target.status = outcome.status.clone();
                        target.exit_code = outcome.exit_code;
                        target.updated_at = completed_at.clone();
                    }
                    let (completed_count, failed_count, pending_count) =
                        rollout_target_summary(&rollout.targets);
                    rollout.completed_count = completed_count;
                    rollout.failed_count = failed_count;
                    rollout.pending_count = pending_count;
                    rollout.updated_at = completed_at.clone();
                }
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: None,
                    action: "job.target_result".to_string(),
                    target: format!("client:{client_id}"),
                    command_hash: None,
                    metadata: json!({
                        "job_id": job_id,
                        "status": outcome.status,
                        "exit_code": outcome.exit_code,
                        "accepted": outcome.accepted,
                        "message": outcome.message,
                    }),
                    created_at: completed_at,
                });
                let update_lifecycle_operation = if outcome.status == "completed"
                    || agent_update_activation_failure_status(&outcome.status)
                {
                    match memory.job_operations.read().await.get(&job_id).cloned() {
                        Some(
                            operation @ (JobCommand::AgentUpdateActivate { .. }
                            | JobCommand::AgentUpdateRollback { .. }),
                        ) => Some(operation),
                        _ => None,
                    }
                } else {
                    None
                };
                match update_lifecycle_operation {
                    Some(JobCommand::AgentUpdateActivate {
                        staged_sha256_hex, ..
                    }) if outcome.status == "completed" => {
                        self.record_agent_update_activation_completed(
                            client_id,
                            job_id,
                            &staged_sha256_hex,
                        )
                        .await?;
                    }
                    Some(JobCommand::AgentUpdateActivate {
                        staged_sha256_hex, ..
                    }) if agent_update_activation_failure_status(&outcome.status) => {
                        self.record_agent_update_activation_failed(
                            client_id,
                            job_id,
                            &staged_sha256_hex,
                            &outcome.status,
                            outcome.exit_code,
                            &outcome.message,
                        )
                        .await?;
                    }
                    Some(JobCommand::AgentUpdateRollback {
                        rollback_sha256_hex,
                    }) if outcome.status == "completed" => {
                        self.record_agent_update_rollback_completed(
                            client_id,
                            job_id,
                            rollback_sha256_hex.as_deref(),
                        )
                        .await?;
                    }
                    _ => {}
                }
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    UPDATE job_targets
                    SET status = $3,
                        exit_code = $4,
                        started_at = COALESCE(started_at, now()),
                        completed_at = now()
                    WHERE job_id = $1 AND client_id = $2
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .bind(&outcome.status)
                .bind(outcome.exit_code)
                .execute(pool)
                .await?;
                sqlx::query(
                    r#"
                    UPDATE agent_update_rollout_targets target
                    SET status = $3,
                        exit_code = $4,
                        updated_at = now()
                    FROM agent_update_rollouts rollout
                    WHERE target.rollout_id = rollout.id
                      AND rollout.job_id = $1
                      AND target.client_id = $2
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .bind(&outcome.status)
                .bind(outcome.exit_code)
                .execute(pool)
                .await?;
                sqlx::query(
                    r#"
                    UPDATE agent_update_rollouts
                    SET updated_at = now()
                    WHERE job_id = $1
                    "#,
                )
                .bind(job_id)
                .execute(pool)
                .await?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, NULL, $2, $3, NULL, $4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind("job.target_result")
                .bind(format!("client:{client_id}"))
                .bind(json!({
                    "job_id": job_id,
                    "status": outcome.status,
                    "exit_code": outcome.exit_code,
                    "accepted": outcome.accepted,
                    "message": outcome.message,
                }))
                .execute(pool)
                .await?;
                let update_lifecycle_operation = if outcome.status == "completed"
                    || agent_update_activation_failure_status(&outcome.status)
                {
                    let row = sqlx::query(
                        r#"
                        SELECT operation
                        FROM jobs
                        WHERE id = $1
                        "#,
                    )
                    .bind(job_id)
                    .fetch_optional(pool)
                    .await?;
                    match row {
                        Some(row) => {
                            let operation: sqlx::types::Json<JobCommand> =
                                row.try_get("operation")?;
                            match operation.0 {
                                operation @ (JobCommand::AgentUpdateActivate { .. }
                                | JobCommand::AgentUpdateRollback { .. }) => Some(operation),
                                _ => None,
                            }
                        }
                        None => None,
                    }
                } else {
                    None
                };
                match update_lifecycle_operation {
                    Some(JobCommand::AgentUpdateActivate {
                        staged_sha256_hex, ..
                    }) if outcome.status == "completed" => {
                        self.record_agent_update_activation_completed(
                            client_id,
                            job_id,
                            &staged_sha256_hex,
                        )
                        .await?;
                    }
                    Some(JobCommand::AgentUpdateActivate {
                        staged_sha256_hex, ..
                    }) if agent_update_activation_failure_status(&outcome.status) => {
                        self.record_agent_update_activation_failed(
                            client_id,
                            job_id,
                            &staged_sha256_hex,
                            &outcome.status,
                            outcome.exit_code,
                            &outcome.message,
                        )
                        .await?;
                    }
                    Some(JobCommand::AgentUpdateRollback {
                        rollback_sha256_hex,
                    }) if outcome.status == "completed" => {
                        self.record_agent_update_rollback_completed(
                            client_id,
                            job_id,
                            rollback_sha256_hex.as_deref(),
                        )
                        .await?;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn finish_job(&self, job_id: Uuid, status: &str) -> Result<()> {
        let completed_operation = match self {
            Self::Memory(memory) => {
                let completed_at = unix_now().to_string();
                if let Some(job) = memory
                    .jobs
                    .write()
                    .await
                    .iter_mut()
                    .find(|job| job.id == job_id)
                {
                    job.status = status.to_string();
                    job.completed_at = Some(completed_at);
                }
                if let Some(rollout) = memory
                    .agent_update_rollouts
                    .write()
                    .await
                    .iter_mut()
                    .find(|rollout| rollout.job_id == job_id)
                {
                    rollout.status = rollout_status_for_job_status(status).to_string();
                    rollout.updated_at = unix_now().to_string();
                }
                if status == "completed" {
                    memory.job_operations.read().await.get(&job_id).cloned()
                } else {
                    None
                }
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    UPDATE jobs
                    SET status = $2, completed_at = now()
                    WHERE id = $1
                    "#,
                )
                .bind(job_id)
                .bind(status)
                .execute(pool)
                .await?;
                sqlx::query(
                    r#"
                    UPDATE agent_update_rollouts
                    SET status = $2,
                        updated_at = now()
                    WHERE job_id = $1
                    "#,
                )
                .bind(job_id)
                .bind(rollout_status_for_job_status(status))
                .execute(pool)
                .await?;
                if status == "completed" {
                    let row = sqlx::query(
                        r#"
                        SELECT operation
                        FROM jobs
                        WHERE id = $1
                        "#,
                    )
                    .bind(job_id)
                    .fetch_optional(pool)
                    .await?;
                    match row {
                        Some(row) => {
                            let operation: sqlx::types::Json<JobCommand> =
                                row.try_get("operation")?;
                            Some(operation.0)
                        }
                        None => None,
                    }
                } else {
                    None
                }
            }
        };
        if let Some(operation) = completed_operation {
            self.record_tunnel_plan_execution(job_id, &operation, status)
                .await?;
        }
        Ok(())
    }
}

struct TargetStatusCounts {
    completed: i32,
    failed: i32,
    pending: i32,
}

fn target_status_counts<'a>(targets: impl Iterator<Item = (&'a str, bool)>) -> TargetStatusCounts {
    let mut counts = TargetStatusCounts {
        completed: 0,
        failed: 0,
        pending: 0,
    };
    for (status, pending_without_completion) in targets {
        if status == "completed" {
            counts.completed += 1;
        } else if pending_without_completion && matches!(status, "queued" | "dispatching") {
            counts.pending += 1;
        } else {
            counts.failed += 1;
        }
    }
    counts
}

fn i64_to_i32(value: i64) -> i32 {
    value.clamp(0, i32::MAX as i64) as i32
}
