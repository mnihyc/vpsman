use std::cmp::Ordering;

use anyhow::Result;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;
use vpsman_common::{
    job_command_safety, job_command_safety_by_operation_type, JobCommand, JobCommandSafety,
    JOB_COMMAND_SAFETY_EXCLUSIVE,
};
use vpsman_server_core::{
    target_status_is_active, JOB_STATUS_COMPLETED, JOB_STATUS_PARTIAL_SUCCESS, JOB_STATUS_QUEUED,
    JOB_STATUS_SKIPPED, TARGET_STATUS_AGENT_TIMEOUT, TARGET_STATUS_CANCELED,
    TARGET_STATUS_COMPLETED, TARGET_STATUS_CONTROL_TIMEOUT, TARGET_STATUS_DISPATCHING,
    TARGET_STATUS_FAILED, TARGET_STATUS_QUEUED, TARGET_STATUS_REJECTED, TARGET_STATUS_RUNNING,
};

pub(crate) use vpsman_server_core::aggregate_job_status_from_statuses;

const EXCLUSIVE_DISPATCH_ADVISORY_LOCK_CLASS: i32 = 0x5650_534d;

use crate::model::*;
use crate::model_webhook_rules::WebhookEventCandidate;
use crate::repository::Repository;
use crate::util::{limit_or_default, offset_or_default, search_pattern, sort_descending};
use crate::{unix_now, TargetDispatchOutcome};

fn agent_update_activation_failure_status(status: &str) -> bool {
    matches!(
        status,
        TARGET_STATUS_FAILED
            | TARGET_STATUS_REJECTED
            | TARGET_STATUS_AGENT_TIMEOUT
            | TARGET_STATUS_CONTROL_TIMEOUT
            | TARGET_STATUS_CANCELED
    )
}

fn schedule_job_outcome_error(status: &str) -> Option<&str> {
    if matches!(
        status,
        JOB_STATUS_COMPLETED | JOB_STATUS_PARTIAL_SUCCESS | JOB_STATUS_SKIPPED
    ) {
        None
    } else {
        Some(status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_operation_types_follow_shared_command_safety() {
        let exclusive = exclusive_operation_types();
        assert!(exclusive.contains(&"backup"));
        assert!(exclusive.contains(&"shell"));
        assert!(exclusive.contains(&"network_apply"));
        assert!(!exclusive.contains(&"network_status"));
    }
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

fn aggregate_job_status_from_targets(targets: &[JobTargetView]) -> &'static str {
    let statuses = targets
        .iter()
        .map(|target| target.status.clone())
        .collect::<Vec<_>>();
    aggregate_job_status_from_statuses(&statuses, targets.len())
}

fn exclusive_operation_types() -> Vec<&'static str> {
    job_command_safety_by_operation_type()
        .iter()
        .filter_map(|(operation_type, safety)| {
            (*safety == JOB_COMMAND_SAFETY_EXCLUSIVE).then_some(*operation_type)
        })
        .collect()
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

struct WebhookJobSummary {
    actor_id: Option<Uuid>,
    command_type: String,
    privileged: bool,
    status: String,
    target_count: i32,
    payload_hash: String,
    source_schedule_id: Option<Uuid>,
    targets: Vec<String>,
}

struct JobCreatedWebhookEvent<'a> {
    job_id: Uuid,
    command_type: &'a str,
    status: &'a str,
    privileged: bool,
    command_hash: &'a str,
    resolved_targets: &'a [String],
    actor_id: Option<Uuid>,
    source_schedule_id: Option<Uuid>,
    operation: Option<&'a JobCommand>,
}

struct ScheduleJobOutcome {
    schedule_id: Uuid,
    schedule_name: String,
    job_id: Uuid,
    status: String,
    error: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ClaimedJobTarget {
    pub(crate) job_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) actor_id: Option<Uuid>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) command_type: String,
    pub(crate) payload_hash: String,
    pub(crate) operation: JobCommand,
    pub(crate) source_schedule_id: Option<Uuid>,
    pub(crate) timeout_secs: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct DeadlineExpiredJobTarget {
    pub(crate) job_id: Uuid,
    pub(crate) client_id: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct JobCancelPlan {
    pub(crate) cancel_targets: Vec<String>,
    pub(crate) pending_canceled: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct JobCompletionContext {
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) payload_hash: String,
    pub(crate) operation: JobCommand,
}

fn job_webhook_predicates(command_type: &str, status: &str, include_created: bool) -> Vec<String> {
    let mut predicates = vec![
        format!("job.status:{status}"),
        format!("job.status.become_{status}"),
        format!("job.type:{command_type}"),
    ];
    if include_created {
        predicates.push("job.created".to_string());
    }
    predicates.sort();
    predicates.dedup();
    predicates
}

impl Repository {
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

    pub(crate) async fn get_job_completion_context(
        &self,
        job_id: Uuid,
    ) -> Result<Option<JobCompletionContext>> {
        match self {
            Self::Memory(memory) => {
                let Some(job) = memory
                    .jobs
                    .read()
                    .await
                    .iter()
                    .find(|job| job.id == job_id)
                    .cloned()
                else {
                    return Ok(None);
                };
                let Some(operation) = memory.job_operations.read().await.get(&job_id).cloned()
                else {
                    return Ok(None);
                };
                Ok(Some(JobCompletionContext {
                    actor_id: job.actor_id,
                    payload_hash: job.payload_hash,
                    operation,
                }))
            }
            Self::Postgres(pool) => {
                let Some(row) = sqlx::query(
                    r#"
                    SELECT actor_id, payload_hash, operation
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
                let operation: sqlx::types::Json<JobCommand> = row.try_get("operation")?;
                Ok(Some(JobCompletionContext {
                    actor_id: row.try_get("actor_id")?,
                    payload_hash: row.try_get("payload_hash")?,
                    operation: operation.0,
                }))
            }
        }
    }

    pub(crate) async fn get_job_request_fingerprint(&self, job_id: Uuid) -> Result<Option<String>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .job_request_fingerprints
                .read()
                .await
                .get(&job_id)
                .cloned()),
            Self::Postgres(pool) => sqlx::query_scalar(
                r#"
                    SELECT request_fingerprint
                    FROM jobs
                    WHERE id = $1
                    "#,
            )
            .bind(job_id)
            .fetch_optional(pool)
            .await
            .map_err(Into::into),
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
                        message,
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
                            message: row.try_get("message")?,
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
        job_id: Uuid,
        request: &CreateJobRequest,
        command_hash: &str,
        request_fingerprint: &str,
        operator: &AuthContext,
        status: &str,
        reason: &str,
    ) -> Result<Uuid> {
        let resolved_targets = request.fixed_target_ids().unwrap_or_default();
        let metadata = json!({
            "selector_expression": request.selector_expression,
            "resolved_targets": &resolved_targets,
            "destructive": request.destructive,
            "confirmed": request.confirmed,
            "privileged": request.privileged,
            "force_unprivileged": request.force_unprivileged,
            "operator_id": operator.operator.id,
            "operator_username": operator.operator.username,
            "operator_role": operator.operator.role,
            "session_id": operator.session_id,
            "reason": reason,
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
                    status: status.to_string(),
                    target_count: resolved_targets.len() as i32,
                    payload_hash: command_hash.to_string(),
                    created_at: created_at.clone(),
                    completed_at: Some(created_at.clone()),
                });
                memory
                    .job_request_fingerprints
                    .write()
                    .await
                    .insert(job_id, request_fingerprint.to_string());
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
                                status: status.to_string(),
                                message: Some(reason.to_string()),
                                exit_code: None,
                                started_at: None,
                                completed_at: Some(created_at.clone()),
                            }),
                    );
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: format!("job.{status}"),
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
                        target_count, payload_hash, operation, request_fingerprint,
                        timeout_secs, completed_at
                    )
	                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, now())
                    "#,
                )
                .bind(job_id)
                .bind(operator.operator.id)
                .bind("api_job_request")
                .bind(request.privileged)
                .bind(status)
                .bind(resolved_targets.len() as i32)
                .bind(command_hash)
                .bind(operation.clone().map(sqlx::types::Json))
                .bind(request_fingerprint)
                .bind(request.timeout_secs.unwrap_or(30) as i64)
                .execute(&mut *tx)
                .await?;
                for client_id in &resolved_targets {
                    sqlx::query(
                        r#"
                        INSERT INTO job_targets (
                            job_id, client_id, status, message, completed_at
                        )
                        VALUES ($1, $2, $3, $4, now())
                        "#,
                    )
                    .bind(job_id)
                    .bind(client_id)
                    .bind(status)
                    .bind(reason)
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
                .bind(format!("job.{status}"))
                .bind("api:/api/v1/jobs")
                .bind(command_hash)
                .bind(metadata)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
            }
        }
        self.record_job_created_webhook_event(JobCreatedWebhookEvent {
            job_id,
            command_type: "api_job_request",
            status,
            privileged: request.privileged,
            command_hash,
            resolved_targets: &resolved_targets,
            actor_id: Some(operator.operator.id),
            source_schedule_id: None,
            operation: operation.as_ref(),
        })
        .await?;
        Ok(job_id)
    }

    pub(crate) async fn record_dispatching_job(
        &self,
        job_id: Uuid,
        request: &CreateJobRequest,
        command_hash: &str,
        request_fingerprint: &str,
        operator: &AuthContext,
        resolved_targets: &[String],
    ) -> Result<Uuid> {
        self.record_dispatching_job_with_source(
            job_id,
            request,
            command_hash,
            request_fingerprint,
            operator,
            resolved_targets,
            None,
        )
        .await
    }
    pub(crate) async fn record_dispatching_job_from_schedule(
        &self,
        job_id: Uuid,
        request: &CreateJobRequest,
        command_hash: &str,
        request_fingerprint: &str,
        operator: &AuthContext,
        resolved_targets: &[String],
        source_schedule_id: Uuid,
    ) -> Result<Uuid> {
        self.record_dispatching_job_with_source(
            job_id,
            request,
            command_hash,
            request_fingerprint,
            operator,
            resolved_targets,
            Some(source_schedule_id),
        )
        .await
    }
    async fn record_dispatching_job_with_source(
        &self,
        job_id: Uuid,
        request: &CreateJobRequest,
        command_hash: &str,
        request_fingerprint: &str,
        operator: &AuthContext,
        resolved_targets: &[String],
        source_schedule_id: Option<Uuid>,
    ) -> Result<Uuid> {
        let command_type = request.command_type_label().to_string();
        let metadata = json!({
            "selector_expression": request.selector_expression,
            "resolved_targets": resolved_targets,
            "destructive": request.destructive,
            "confirmed": request.confirmed,
            "privileged": request.privileged,
            "force_unprivileged": request.force_unprivileged,
            "source_schedule_id": source_schedule_id,
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
                    command_type: command_type.clone(),
                    privileged: request.privileged,
                    status: JOB_STATUS_QUEUED.to_string(),
                    target_count: resolved_targets.len() as i32,
                    payload_hash: command_hash.to_string(),
                    created_at: created_at.clone(),
                    completed_at: None,
                });
                memory
                    .job_request_fingerprints
                    .write()
                    .await
                    .insert(job_id, request_fingerprint.to_string());
                memory
                    .job_operations
                    .write()
                    .await
                    .insert(job_id, operation.clone());
                memory
                    .job_timeouts
                    .write()
                    .await
                    .insert(job_id, request.timeout_secs.unwrap_or(30).clamp(1, 3600));
                if let Some(schedule_id) = source_schedule_id {
                    memory
                        .job_source_schedule_ids
                        .write()
                        .await
                        .insert(job_id, schedule_id);
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
                                status: TARGET_STATUS_QUEUED.to_string(),
                                message: None,
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
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query(
                    r#"
                    INSERT INTO jobs (
                        id, actor_id, command_type, privileged, status,
                        target_count, payload_hash, operation, source_schedule_id, request_fingerprint,
                        timeout_secs
                    )
	                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                    "#,
                )
                .bind(job_id)
                .bind(operator.operator.id)
                .bind(&command_type)
                .bind(request.privileged)
                    .bind(JOB_STATUS_QUEUED)
                .bind(resolved_targets.len() as i32)
                .bind(command_hash)
                .bind(sqlx::types::Json(operation.clone()))
                .bind(source_schedule_id)
                .bind(request_fingerprint)
                .bind(request.timeout_secs.unwrap_or(30) as i64)
                .execute(&mut *tx)
                .await?;
                for client_id in resolved_targets {
                    sqlx::query(
                        r#"
                        INSERT INTO job_targets (
                            job_id, client_id, status, message
                        )
                        VALUES ($1, $2, $3, NULL)
                        "#,
                    )
                    .bind(job_id)
                    .bind(client_id)
                    .bind(TARGET_STATUS_QUEUED)
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
                tx.commit().await?;
            }
        }
        self.record_job_created_webhook_event(JobCreatedWebhookEvent {
            job_id,
            command_type: &command_type,
            status: JOB_STATUS_QUEUED,
            privileged: request.privileged,
            command_hash,
            resolved_targets,
            actor_id: Some(operator.operator.id),
            source_schedule_id,
            operation: Some(&operation),
        })
        .await?;
        Ok(job_id)
    }

    pub(crate) async fn claim_due_job_targets(
        &self,
        limit: i64,
        lease_secs: i64,
    ) -> Result<Vec<ClaimedJobTarget>> {
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let operations = memory.job_operations.read().await.clone();
                let source_schedule_ids = memory.job_source_schedule_ids.read().await.clone();
                let timeouts = memory.job_timeouts.read().await.clone();
                let jobs = memory.jobs.read().await.clone();
                let target_snapshot = memory.job_targets.read().await.clone();
                let mut active_exclusive_clients = target_snapshot
                    .iter()
                    .filter(|target| {
                        target.completed_at.is_none()
                            && matches!(
                                target.status.as_str(),
                                TARGET_STATUS_DISPATCHING | TARGET_STATUS_RUNNING
                            )
                    })
                    .filter_map(|target| {
                        let operation = operations.get(&target.job_id)?;
                        (job_command_safety(operation) == JobCommandSafety::Exclusive)
                            .then(|| target.client_id.clone())
                    })
                    .collect::<std::collections::HashSet<_>>();
                let mut targets = memory.job_targets.write().await;
                let mut claimed = Vec::new();
                for target in targets.iter_mut().filter(|target| {
                    target.completed_at.is_none() && target.status == TARGET_STATUS_QUEUED
                }) {
                    if claimed.len() >= limit.clamp(1, 500) as usize {
                        break;
                    }
                    let Some(job) = jobs.iter().find(|job| job.id == target.job_id) else {
                        continue;
                    };
                    let Some(operation) = operations.get(&target.job_id).cloned() else {
                        continue;
                    };
                    if job_command_safety(&operation) == JobCommandSafety::Exclusive
                        && active_exclusive_clients.contains(&target.client_id)
                    {
                        continue;
                    }
                    let is_exclusive =
                        job_command_safety(&operation) == JobCommandSafety::Exclusive;
                    target.status = TARGET_STATUS_DISPATCHING.to_string();
                    target.started_at.get_or_insert_with(|| now.clone());
                    if is_exclusive {
                        active_exclusive_clients.insert(target.client_id.clone());
                    }
                    claimed.push(ClaimedJobTarget {
                        job_id: target.job_id,
                        client_id: target.client_id.clone(),
                        actor_id: job.actor_id,
                        command_type: job.command_type.clone(),
                        payload_hash: job.payload_hash.clone(),
                        operation,
                        source_schedule_id: source_schedule_ids.get(&target.job_id).copied(),
                        timeout_secs: timeouts
                            .get(&target.job_id)
                            .copied()
                            .unwrap_or(30)
                            .clamp(1, 3600),
                    });
                }
                Ok(claimed)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH due AS (
                        SELECT
                            target.job_id,
                            target.client_id,
                            job.actor_id,
                            job.command_type,
                            job.payload_hash,
                            job.operation,
                            job.source_schedule_id,
                            job.timeout_secs
                        FROM job_targets target
                        JOIN jobs job ON job.id = target.job_id
	                        WHERE target.completed_at IS NULL
                              AND target.cancel_requested_at IS NULL
	                          AND target.status IN ('queued', 'dispatching')
	                          AND job.completed_at IS NULL
	                          AND job.status IN ('queued', 'running')
                              AND (
                                target.deadline_at IS NULL
                                OR target.deadline_at > now()
                              )
                              AND (
                                COALESCE(job.operation ->> 'type', '') <> ALL($3::text[])
                                OR (
                                  pg_try_advisory_xact_lock(
                                    $4::integer,
                                    hashtext(target.client_id)
                                  )
                                  AND
                                  NOT EXISTS (
                                    SELECT 1
                                    FROM job_targets active_target
                                    JOIN jobs active_job
                                      ON active_job.id = active_target.job_id
                                    WHERE active_target.client_id = target.client_id
                                      AND active_target.completed_at IS NULL
                                      AND active_target.status IN ('dispatching', 'running')
                                      AND active_job.completed_at IS NULL
                                      AND COALESCE(active_job.operation ->> 'type', '') = ANY($3::text[])
                                      AND (
                                        active_target.job_id <> target.job_id
                                        OR active_target.client_id <> target.client_id
                                      )
                                  )
                                  AND NOT EXISTS (
                                    SELECT 1
                                    FROM job_targets earlier_target
                                    JOIN jobs earlier_job
                                      ON earlier_job.id = earlier_target.job_id
                                    WHERE earlier_target.client_id = target.client_id
                                      AND earlier_target.completed_at IS NULL
                                      AND earlier_target.cancel_requested_at IS NULL
                                      AND earlier_target.status IN ('queued', 'dispatching')
                                      AND earlier_job.completed_at IS NULL
                                      AND earlier_job.status IN ('queued', 'running')
                                      AND COALESCE(earlier_job.operation ->> 'type', '') = ANY($3::text[])
                                      AND (
                                        earlier_target.deadline_at IS NULL
                                        OR earlier_target.deadline_at > now()
                                      )
                                      AND (
                                        earlier_target.status = 'queued'
                                        OR earlier_target.dispatch_lease_until IS NULL
                                        OR earlier_target.dispatch_lease_until < now()
                                      )
                                      AND (
                                        earlier_job.created_at,
                                        earlier_target.job_id,
                                        earlier_target.client_id
                                      ) < (
                                        job.created_at,
                                        target.job_id,
                                        target.client_id
                                      )
                                  )
                                )
                              )
                              AND (
                                target.status = 'queued'
                                OR target.dispatch_lease_until IS NULL
                                OR target.dispatch_lease_until < now()
                              )
                        ORDER BY job.created_at ASC, target.client_id ASC
                        LIMIT $1
                        FOR UPDATE SKIP LOCKED
                    )
                    UPDATE job_targets target
                    SET
	                        status = 'dispatching',
	                        started_at = COALESCE(target.started_at, now()),
	                        dispatch_attempts = target.dispatch_attempts + 1,
	                        dispatch_lease_until = now() + make_interval(secs => $2::integer),
                            deadline_at = COALESCE(target.deadline_at, now() + make_interval(secs => due.timeout_secs::integer)),
	                        last_dispatch_error = NULL
                    FROM due
                    WHERE target.job_id = due.job_id
                      AND target.client_id = due.client_id
                    RETURNING
                        due.job_id,
                        due.client_id,
                        due.actor_id,
                        due.command_type,
                        due.payload_hash,
                        due.operation,
                        due.source_schedule_id,
                        due.timeout_secs
                    "#,
                )
                .bind(limit.clamp(1, 500))
                .bind(lease_secs.clamp(1, 3600) as i32)
                .bind(exclusive_operation_types())
                .bind(EXCLUSIVE_DISPATCH_ADVISORY_LOCK_CLASS)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let operation: sqlx::types::Json<JobCommand> = row.try_get("operation")?;
                        let timeout_secs =
                            row.try_get::<i64, _>("timeout_secs")?.clamp(1, 3600) as u64;
                        Ok(ClaimedJobTarget {
                            job_id: row.try_get("job_id")?,
                            client_id: row.try_get("client_id")?,
                            actor_id: row.try_get("actor_id")?,
                            command_type: row.try_get("command_type")?,
                            payload_hash: row.try_get("payload_hash")?,
                            operation: operation.0,
                            source_schedule_id: row.try_get("source_schedule_id")?,
                            timeout_secs,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn refresh_job_status_from_targets(
        &self,
        job_id: Uuid,
    ) -> Result<Option<String>> {
        let Some(job) = self.get_job(job_id).await? else {
            return Ok(None);
        };
        if job.completed_at.is_some() {
            return Ok(None);
        }
        let targets = self.list_job_targets(job_id).await?;
        if targets.is_empty()
            || targets
                .iter()
                .any(|target| target_status_is_active(&target.status))
        {
            return Ok(Some(job.status));
        }
        let status = aggregate_job_status_from_targets(&targets);
        self.finish_job(job_id, status).await?;
        Ok(Some(status.to_string()))
    }

    pub(crate) async fn mark_job_target_running(
        &self,
        job_id: Uuid,
        client_id: &str,
        message: &str,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                if let Some(job) = memory
                    .jobs
                    .write()
                    .await
                    .iter_mut()
                    .find(|job| job.id == job_id)
                {
                    job.status = "running".to_string();
                }
                if let Some(target) = memory
                    .job_targets
                    .write()
                    .await
                    .iter_mut()
                    .find(|target| target.job_id == job_id && target.client_id == client_id)
                {
                    target.status = TARGET_STATUS_RUNNING.to_string();
                    target.message = Some(message.to_string());
                    target
                        .started_at
                        .get_or_insert_with(|| unix_now().to_string());
                }
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query(
                    r#"
                    UPDATE job_targets
                    SET status = 'running',
                        message = $3,
                        delivered_at = COALESCE(delivered_at, now()),
                        acked_at = COALESCE(acked_at, now()),
                        started_at = COALESCE(started_at, now()),
                        dispatch_lease_until = NULL,
                        last_dispatch_error = NULL
                    WHERE job_id = $1
                      AND client_id = $2
                      AND completed_at IS NULL
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .bind(message)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    r#"
                    UPDATE jobs
                    SET status = 'running'
                    WHERE id = $1
                      AND completed_at IS NULL
                      AND status = 'queued'
                    "#,
                )
                .bind(job_id)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn record_job_target_delivery_error(
        &self,
        job_id: Uuid,
        client_id: &str,
        message: &str,
    ) -> Result<()> {
        match self {
            Self::Memory(_) => {}
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    UPDATE job_targets
                    SET last_dispatch_error = $3
                    WHERE job_id = $1
                      AND client_id = $2
                      AND completed_at IS NULL
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .bind(message)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn expire_control_timeout_targets(
        &self,
        limit: i64,
    ) -> Result<Vec<DeadlineExpiredJobTarget>> {
        match self {
            Self::Memory(memory) => {
                let now = unix_now();
                let completed_at = now.to_string();
                let timeouts = memory.job_timeouts.read().await.clone();
                let mut expired = Vec::new();
                let mut targets = memory.job_targets.write().await;
                for target in targets
                    .iter_mut()
                    .filter(|target| {
                        target.completed_at.is_none()
                            && matches!(
                                target.status.as_str(),
                                TARGET_STATUS_DISPATCHING | TARGET_STATUS_RUNNING
                            )
                    })
                    .take(limit.clamp(1, 500) as usize)
                {
                    let Some(started_at) = target
                        .started_at
                        .as_deref()
                        .and_then(|value| value.parse::<u64>().ok())
                    else {
                        continue;
                    };
                    let timeout_secs = timeouts
                        .get(&target.job_id)
                        .copied()
                        .unwrap_or(30)
                        .clamp(1, 3600);
                    if now.saturating_sub(started_at) < timeout_secs {
                        continue;
                    }
                    target.status = TARGET_STATUS_CONTROL_TIMEOUT.to_string();
                    target.message =
                        Some("control deadline elapsed before final command output".to_string());
                    target.completed_at = Some(completed_at.clone());
                    expired.push(DeadlineExpiredJobTarget {
                        job_id: target.job_id,
                        client_id: target.client_id.clone(),
                    });
                }
                Ok(expired)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH expired AS (
                        SELECT job_id, client_id
                        FROM job_targets
                        WHERE completed_at IS NULL
                          AND status IN ('dispatching', 'running')
                          AND deadline_at IS NOT NULL
                          AND deadline_at <= now()
                        ORDER BY deadline_at ASC, job_id, client_id
                        LIMIT $1
                        FOR UPDATE SKIP LOCKED
                    )
                    UPDATE job_targets target
                    SET status = 'control_timeout',
                        message = COALESCE(target.last_dispatch_error, 'control deadline elapsed before final command output'),
                        completed_at = now(),
                        dispatch_lease_until = NULL,
                        cancel_requested_at = COALESCE(cancel_requested_at, now())
                    FROM expired
                    WHERE target.job_id = expired.job_id
                      AND target.client_id = expired.client_id
                    RETURNING target.job_id, target.client_id
                    "#,
                )
                .bind(limit.clamp(1, 500))
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(DeadlineExpiredJobTarget {
                            job_id: row.try_get("job_id")?,
                            client_id: row.try_get("client_id")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn request_job_cancel(
        &self,
        job_id: Uuid,
        actor_id: Uuid,
        reason: Option<&str>,
    ) -> Result<JobCancelPlan> {
        let message = reason
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("operator_cancel_requested");
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut pending_canceled = 0_usize;
                let mut cancel_targets = Vec::new();
                for target in memory
                    .job_targets
                    .write()
                    .await
                    .iter_mut()
                    .filter(|target| target.job_id == job_id && target.completed_at.is_none())
                {
                    match target.status.as_str() {
                        TARGET_STATUS_QUEUED => {
                            target.status = TARGET_STATUS_CANCELED.to_string();
                            target.message = Some(message.to_string());
                            target.completed_at = Some(now.clone());
                            pending_canceled += 1;
                        }
                        TARGET_STATUS_DISPATCHING | TARGET_STATUS_RUNNING => {
                            cancel_targets.push(target.client_id.clone());
                        }
                        _ => {}
                    }
                }
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(actor_id),
                    action: "job.cancel_requested".to_string(),
                    target: format!("job:{job_id}"),
                    command_hash: None,
                    metadata: json!({
                        "job_id": job_id,
                        "reason": message,
                        "pending_canceled": pending_canceled,
                        "cancel_targets": cancel_targets,
                    }),
                    created_at: now,
                });
                Ok(JobCancelPlan {
                    cancel_targets,
                    pending_canceled,
                })
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let pending_rows = sqlx::query(
                    r#"
                    UPDATE job_targets
                    SET
                        status = 'canceled',
                        message = $2,
                        completed_at = now(),
                        dispatch_lease_until = NULL,
                        cancel_requested_at = COALESCE(cancel_requested_at, now())
                    WHERE job_id = $1
                      AND completed_at IS NULL
                      AND status = 'queued'
                    RETURNING client_id
                    "#,
                )
                .bind(job_id)
                .bind(message)
                .fetch_all(&mut *tx)
                .await?;
                let active_rows = sqlx::query(
                    r#"
                    UPDATE job_targets
                    SET
                        cancel_requested_at = COALESCE(cancel_requested_at, now()),
                        message = COALESCE(message, $2)
                    WHERE job_id = $1
                      AND completed_at IS NULL
                      AND status IN ('dispatching', 'running')
                    RETURNING client_id
                    "#,
                )
                .bind(job_id)
                .bind(message)
                .fetch_all(&mut *tx)
                .await?;
                let pending_canceled = pending_rows.len();
                let cancel_targets = active_rows
                    .into_iter()
                    .map(|row| row.try_get("client_id").map_err(Into::into))
                    .collect::<Result<Vec<String>>>()?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, 'job.cancel_requested', $3, NULL, $4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(actor_id)
                .bind(format!("job:{job_id}"))
                .bind(json!({
                    "job_id": job_id,
                    "reason": message,
                    "pending_canceled": pending_canceled,
                    "cancel_targets": &cancel_targets,
                }))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(JobCancelPlan {
                    cancel_targets,
                    pending_canceled,
                })
            }
        }
    }

    pub(crate) async fn record_job_target_cancel_result(
        &self,
        job_id: Uuid,
        client_id: &str,
        _accepted: bool,
        acked: bool,
        applied: bool,
        message: &str,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                if applied {
                    let now = unix_now().to_string();
                    if let Some(target) =
                        memory.job_targets.write().await.iter_mut().find(|target| {
                            target.job_id == job_id
                                && target.client_id == client_id
                                && target.completed_at.is_none()
                        })
                    {
                        target.status = TARGET_STATUS_CANCELED.to_string();
                        target.message = Some(message.to_string());
                        target.completed_at = Some(now);
                    }
                }
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    UPDATE job_targets
                    SET
                        cancel_sent_at = COALESCE(cancel_sent_at, now()),
                        cancel_acked_at = CASE WHEN $3 THEN COALESCE(cancel_acked_at, now()) ELSE cancel_acked_at END,
                        status = CASE WHEN $4 AND completed_at IS NULL THEN 'canceled' ELSE status END,
                        completed_at = CASE WHEN $4 AND completed_at IS NULL THEN now() ELSE completed_at END,
                        dispatch_lease_until = CASE WHEN $4 AND completed_at IS NULL THEN NULL ELSE dispatch_lease_until END,
                        message = CASE WHEN $4 AND completed_at IS NULL THEN $5 ELSE COALESCE(message, $5) END,
                        last_dispatch_error = CASE WHEN $4 THEN NULL ELSE $5 END
                    WHERE job_id = $1
                      AND client_id = $2
                      AND (
                        completed_at IS NULL
                        OR status IN ('control_timeout', 'canceled')
                      )
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .bind(acked)
                .bind(applied)
                .bind(message)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn record_job_target_cancel_sent(
        &self,
        job_id: Uuid,
        client_id: &str,
    ) -> Result<()> {
        match self {
            Self::Memory(_) => {}
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    UPDATE job_targets
                    SET cancel_sent_at = COALESCE(cancel_sent_at, now())
                    WHERE job_id = $1
                      AND client_id = $2
                      AND (
                        completed_at IS NULL
                        OR status IN ('control_timeout', 'canceled')
                      )
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
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
                    target.message = Some(outcome.message.clone());
                    target.exit_code = outcome.exit_code;
                    target
                        .started_at
                        .get_or_insert_with(|| completed_at.clone());
                    target.completed_at = Some(completed_at.clone());
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
                        "received_at": outcome.received_at,
                    }),
                    created_at: completed_at,
                });
                let update_lifecycle_operation = if outcome.status == TARGET_STATUS_COMPLETED
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
                    }) if outcome.status == TARGET_STATUS_COMPLETED => {
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
                    }) if outcome.status == TARGET_STATUS_COMPLETED => {
                        self.record_agent_update_rollback_completed(
                            client_id,
                            job_id,
                            rollback_sha256_hex.as_deref(),
                        )
                        .await?;
                    }
                    Some(JobCommand::AgentUpdateRollback {
                        rollback_sha256_hex,
                    }) if agent_update_activation_failure_status(&outcome.status) => {
                        self.record_agent_update_rollback_failed(
                            client_id,
                            job_id,
                            rollback_sha256_hex.as_deref(),
                            &outcome.status,
                            outcome.exit_code,
                            &outcome.message,
                        )
                        .await?;
                    }
                    _ => {}
                }
            }
            Self::Postgres(pool) => {
                let updated = sqlx::query(
                    r#"
                    UPDATE job_targets
                    SET status = $3,
                        message = $4,
                        exit_code = $5,
                        started_at = COALESCE(started_at, now()),
                        completed_at = now(),
                        result_received_at = COALESCE($6::timestamptz, now()),
                        dispatch_lease_until = NULL,
                        last_dispatch_error = CASE WHEN $3 IN ('failed', 'control_timeout') THEN $4 ELSE NULL END
                    WHERE job_id = $1
                      AND client_id = $2
                      AND (
                        cancel_acked_at IS NULL
                        OR COALESCE($6::timestamptz, now()) < cancel_acked_at
                      )
                      AND (
                        completed_at IS NULL
                        OR status = 'control_timeout'
                      )
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .bind(&outcome.status)
                .bind(&outcome.message)
                .bind(outcome.exit_code)
                .bind(outcome.received_at.as_deref())
                .execute(pool)
                .await?;
                if updated.rows_affected() == 0 {
                    return Ok(());
                }
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
                    "received_at": outcome.received_at,
                }))
                .execute(pool)
                .await?;
                let update_lifecycle_operation = if outcome.status == TARGET_STATUS_COMPLETED
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
                    }) if outcome.status == TARGET_STATUS_COMPLETED => {
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
                    }) if outcome.status == TARGET_STATUS_COMPLETED => {
                        self.record_agent_update_rollback_completed(
                            client_id,
                            job_id,
                            rollback_sha256_hex.as_deref(),
                        )
                        .await?;
                    }
                    Some(JobCommand::AgentUpdateRollback {
                        rollback_sha256_hex,
                    }) if agent_update_activation_failure_status(&outcome.status) => {
                        self.record_agent_update_rollback_failed(
                            client_id,
                            job_id,
                            rollback_sha256_hex.as_deref(),
                            &outcome.status,
                            outcome.exit_code,
                            &outcome.message,
                        )
                        .await?;
                    }
                    _ => {}
                }
            }
        }
        self.record_job_target_webhook_event(job_id, client_id, outcome)
            .await?;
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
                if status == JOB_STATUS_COMPLETED {
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
                if status == JOB_STATUS_COMPLETED {
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
        self.record_job_status_webhook_event(job_id, status).await?;
        self.record_schedule_job_outcome(job_id, status).await?;
        Ok(())
    }

    async fn record_schedule_job_outcome(&self, job_id: Uuid, status: &str) -> Result<()> {
        let Some(summary) = self.webhook_job_summary(job_id).await? else {
            return Ok(());
        };
        let Some(schedule_id) = summary.source_schedule_id else {
            return Ok(());
        };
        let outcome_error = schedule_job_outcome_error(status);
        let schedule_outcome = match self {
            Self::Memory(_) => None,
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    UPDATE schedules
                    SET
                        last_job_id = $2,
                        last_job_status = $3,
                        last_job_completed_at = now(),
                        last_job_error = $4,
                        updated_at = now()
                    WHERE id = $1
                    RETURNING name
                    "#,
                )
                .bind(schedule_id)
                .bind(job_id)
                .bind(status)
                .bind(outcome_error)
                .fetch_optional(pool)
                .await?;
                row.map(|row| {
                    let schedule_name: String = row.try_get("name")?;
                    Ok::<_, sqlx::Error>(ScheduleJobOutcome {
                        schedule_id,
                        schedule_name,
                        job_id,
                        status: status.to_string(),
                        error: outcome_error.map(ToOwned::to_owned),
                    })
                })
                .transpose()?
            }
        };
        let Some(schedule_outcome) = schedule_outcome else {
            return Ok(());
        };
        let event_id = format!("schedule:{}:job:{}:finished", schedule_id, job_id);
        let mut predicates = vec![
            "schedule.job_finished".to_string(),
            format!("schedule.id:{}", schedule_outcome.schedule_id),
            format!("schedule.name:{}", schedule_outcome.schedule_name),
            format!("job.status:{}", schedule_outcome.status),
            format!("job.status.become_{}", schedule_outcome.status),
            format!("job.type:{}", summary.command_type),
        ];
        predicates.sort();
        predicates.dedup();
        self.record_webhook_event(WebhookEventCandidate {
            kind: "schedule.job_finished".to_string(),
            event_id: event_id.clone(),
            event_predicates: predicates.clone(),
            subject_client_ids: summary.targets.clone(),
            actor_id: summary.actor_id,
            payload: json!({
                "event": {
                    "kind": "schedule.job_finished",
                    "id": &event_id,
                    "predicates": &predicates,
                },
                "schedule": {
                    "id": schedule_outcome.schedule_id,
                    "name": &schedule_outcome.schedule_name,
                    "last_job_id": schedule_outcome.job_id,
                    "last_job_status": &schedule_outcome.status,
                    "last_job_error": &schedule_outcome.error,
                },
                "job": {
                    "id": job_id,
                    "status": status,
                    "type": &summary.command_type,
                    "privileged": summary.privileged,
                    "payload_hash": &summary.payload_hash,
                    "source_schedule_id": schedule_id,
                    "target_count": summary.target_count,
                    "target_ids": &summary.targets,
                },
            }),
        })
        .await?;
        Ok(())
    }

    async fn record_job_created_webhook_event(
        &self,
        event: JobCreatedWebhookEvent<'_>,
    ) -> Result<()> {
        let event_id = format!("job:{}:created", event.job_id);
        let predicates = job_webhook_predicates(event.command_type, event.status, true);
        self.record_webhook_event(WebhookEventCandidate {
            kind: "job.created".to_string(),
            event_id: event_id.clone(),
            event_predicates: predicates.clone(),
            subject_client_ids: event.resolved_targets.to_vec(),
            actor_id: event.actor_id,
            payload: json!({
                "event": {
                    "kind": "job.created",
                    "id": &event_id,
                    "predicates": &predicates,
                },
                "job": {
                    "id": event.job_id,
                    "status": event.status,
                    "type": event.command_type,
                    "privileged": event.privileged,
                    "payload_hash": event.command_hash,
                    "source_schedule_id": event.source_schedule_id,
                    "target_count": event.resolved_targets.len(),
                    "target_ids": event.resolved_targets,
                    "operation": event.operation
                        .map(|value| json!(value))
                        .unwrap_or(serde_json::Value::Null),
                },
            }),
        })
        .await?;
        Ok(())
    }

    async fn record_job_target_webhook_event(
        &self,
        job_id: Uuid,
        client_id: &str,
        outcome: &TargetDispatchOutcome,
    ) -> Result<()> {
        let Some(summary) = self.webhook_job_summary(job_id).await? else {
            return Ok(());
        };
        let event_id = format!(
            "job:{job_id}:target:{client_id}:status:{}:{}",
            outcome.status,
            Uuid::new_v4()
        );
        let mut predicates = job_webhook_predicates(&summary.command_type, &summary.status, false);
        predicates.push(format!("job.target.status:{}", outcome.status));
        predicates.sort();
        predicates.dedup();
        self.record_webhook_event(WebhookEventCandidate {
            kind: "job.target.status".to_string(),
            event_id: event_id.clone(),
            event_predicates: predicates.clone(),
            subject_client_ids: vec![client_id.to_string()],
            actor_id: summary.actor_id,
            payload: json!({
                "event": {
                    "kind": "job.target.status",
                    "id": &event_id,
                    "predicates": &predicates,
                },
                "job": {
                    "id": job_id,
                    "status": &summary.status,
                    "type": &summary.command_type,
                    "privileged": summary.privileged,
                    "payload_hash": &summary.payload_hash,
                    "source_schedule_id": summary.source_schedule_id,
                    "target_count": summary.target_count,
                    "target_ids": &summary.targets,
                    "target": {
                        "client_id": client_id,
                        "status": &outcome.status,
                        "accepted": outcome.accepted,
                        "exit_code": outcome.exit_code,
                        "message": &outcome.message,
                    },
                },
            }),
        })
        .await?;
        Ok(())
    }

    async fn record_job_status_webhook_event(&self, job_id: Uuid, status: &str) -> Result<()> {
        let Some(summary) = self.webhook_job_summary(job_id).await? else {
            return Ok(());
        };
        let event_id = format!("job:{job_id}:status:{status}");
        let predicates = job_webhook_predicates(&summary.command_type, status, false);
        self.record_webhook_event(WebhookEventCandidate {
            kind: "job.status".to_string(),
            event_id: event_id.clone(),
            event_predicates: predicates.clone(),
            subject_client_ids: summary.targets.clone(),
            actor_id: summary.actor_id,
            payload: json!({
                "event": {
                    "kind": "job.status",
                    "id": &event_id,
                    "predicates": &predicates,
                },
                "job": {
                    "id": job_id,
                    "status": status,
                    "type": &summary.command_type,
                    "privileged": summary.privileged,
                    "payload_hash": &summary.payload_hash,
                    "source_schedule_id": summary.source_schedule_id,
                    "target_count": summary.target_count,
                    "target_ids": &summary.targets,
                },
            }),
        })
        .await?;
        Ok(())
    }

    async fn webhook_job_summary(&self, job_id: Uuid) -> Result<Option<WebhookJobSummary>> {
        match self {
            Self::Memory(memory) => {
                let Some(job) = memory
                    .jobs
                    .read()
                    .await
                    .iter()
                    .find(|job| job.id == job_id)
                    .cloned()
                else {
                    return Ok(None);
                };
                let targets = memory
                    .job_targets
                    .read()
                    .await
                    .iter()
                    .filter(|target| target.job_id == job_id)
                    .map(|target| target.client_id.clone())
                    .collect::<Vec<_>>();
                Ok(Some(WebhookJobSummary {
                    actor_id: job.actor_id,
                    command_type: job.command_type,
                    privileged: job.privileged,
                    status: job.status,
                    target_count: job.target_count,
                    payload_hash: job.payload_hash,
                    source_schedule_id: None,
                    targets,
                }))
            }
            Self::Postgres(pool) => {
                let Some(row) = sqlx::query(
                    r#"
                    SELECT
                        job.actor_id,
                        job.command_type,
                        job.privileged,
                        job.status,
                        job.target_count,
                        job.payload_hash,
                        job.source_schedule_id,
                        COALESCE(
                            (
                                SELECT array_agg(target.client_id ORDER BY target.client_id)
                                FROM job_targets target
                                WHERE target.job_id = job.id
                            ),
                            ARRAY[]::TEXT[]
                        ) AS targets
                    FROM jobs job
                    WHERE job.id = $1
                    "#,
                )
                .bind(job_id)
                .fetch_optional(pool)
                .await?
                else {
                    return Ok(None);
                };
                Ok(Some(WebhookJobSummary {
                    actor_id: row.try_get("actor_id")?,
                    command_type: row.try_get("command_type")?,
                    privileged: row.try_get("privileged")?,
                    status: row.try_get("status")?,
                    target_count: row.try_get("target_count")?,
                    payload_hash: row.try_get("payload_hash")?,
                    source_schedule_id: row.try_get("source_schedule_id")?,
                    targets: row.try_get("targets")?,
                }))
            }
        }
    }
}
