use std::cmp::Ordering;

use anyhow::Result;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;
use vpsman_common::JobCommand;
use vpsman_server_core::{target_status_counts_as_accepted, target_status_is_pending};

pub(crate) use vpsman_server_core::aggregate_job_status_from_statuses;

use crate::model::*;
use crate::model_webhook_rules::WebhookEventCandidate;
use crate::repository::Repository;
use crate::util::{limit_or_default, offset_or_default, search_pattern, sort_descending};
use crate::{unix_now, TargetDispatchOutcome};

fn agent_update_activation_failure_status(status: &str) -> bool {
    matches!(
        status,
        "failed" | "dispatch_failed" | "rejected_by_agent" | "timed_out"
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

fn aggregate_job_status_from_targets(targets: &[JobTargetView]) -> &'static str {
    let statuses = targets
        .iter()
        .map(|target| target.status.clone())
        .collect::<Vec<_>>();
    aggregate_job_status_from_statuses(&statuses, targets.len())
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

#[derive(Clone, Debug)]
pub(crate) struct ClaimedJobTarget {
    pub(crate) job_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) command_type: String,
    pub(crate) payload_hash: String,
    pub(crate) operation: JobCommand,
    pub(crate) source_schedule_id: Option<Uuid>,
    pub(crate) timeout_secs: u64,
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

    pub(crate) async fn count_job_accepted_targets(&self, job_id: Uuid) -> Result<usize> {
        match self {
            Self::Memory(memory) => Ok(memory
                .job_targets
                .read()
                .await
                .iter()
                .filter(|target| {
                    target.job_id == job_id && target_status_counts_as_accepted(&target.status)
                })
                .count()),
            Self::Postgres(pool) => {
                let count: i64 = sqlx::query_scalar(
                    r#"
                    SELECT count(*)
                    FROM job_targets
                    WHERE job_id = $1
                      AND status IN ('accepted', 'completed', 'failed', 'timed_out')
                    "#,
                )
                .bind(job_id)
                .fetch_one(pool)
                .await?;
                Ok(count.max(0) as usize)
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
            "reconnect_policy": request.reconnect_policy,
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
                        timeout_secs, reconnect_policy, completed_at
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, COALESCE($11, '{"duplicate_delivery":"ignore_completed","resume_outputs":true}'::jsonb), now())
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
                .bind(request.reconnect_policy.as_ref().map(sqlx::types::Json))
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
            "reconnect_policy": request.reconnect_policy,
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
                    status: "dispatching".to_string(),
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
                        timeout_secs, reconnect_policy
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, COALESCE($12, '{"duplicate_delivery":"ignore_completed","resume_outputs":true}'::jsonb))
                    "#,
                )
                .bind(job_id)
                .bind(operator.operator.id)
                .bind(&command_type)
                .bind(request.privileged)
                .bind("dispatching")
                .bind(resolved_targets.len() as i32)
                .bind(command_hash)
                .bind(sqlx::types::Json(operation.clone()))
                .bind(source_schedule_id)
                .bind(request_fingerprint)
                .bind(request.timeout_secs.unwrap_or(30) as i64)
                .bind(request.reconnect_policy.as_ref().map(sqlx::types::Json))
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
                tx.commit().await?;
            }
        }
        self.record_job_created_webhook_event(JobCreatedWebhookEvent {
            job_id,
            command_type: &command_type,
            status: "dispatching",
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
                let jobs = memory.jobs.read().await.clone();
                let mut targets = memory.job_targets.write().await;
                let mut claimed = Vec::new();
                for target in targets
                    .iter_mut()
                    .filter(|target| target.completed_at.is_none() && target.status == "queued")
                {
                    if claimed.len() >= limit.clamp(1, 100) as usize {
                        break;
                    }
                    let Some(job) = jobs.iter().find(|job| job.id == target.job_id) else {
                        continue;
                    };
                    let Some(operation) = operations.get(&target.job_id).cloned() else {
                        continue;
                    };
                    target.status = "dispatching".to_string();
                    target.started_at.get_or_insert_with(|| now.clone());
                    claimed.push(ClaimedJobTarget {
                        job_id: target.job_id,
                        client_id: target.client_id.clone(),
                        actor_id: job.actor_id,
                        command_type: job.command_type.clone(),
                        payload_hash: job.payload_hash.clone(),
                        operation,
                        source_schedule_id: None,
                        timeout_secs: 30,
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
                          AND target.status IN ('queued', 'dispatching')
                          AND job.completed_at IS NULL
                          AND job.status IN ('queued', 'dispatching', 'running')
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
                .bind(limit.clamp(1, 100))
                .bind(lease_secs.clamp(1, 3600) as i32)
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
    ) -> Result<Option<(String, usize)>> {
        let Some(job) = self.get_job(job_id).await? else {
            return Ok(None);
        };
        let accepted_targets = self.count_job_accepted_targets(job_id).await?;
        if job.completed_at.is_some() {
            return Ok(None);
        }
        let targets = self.list_job_targets(job_id).await?;
        if targets.is_empty()
            || targets
                .iter()
                .any(|target| target_status_is_pending(&target.status))
        {
            return Ok(Some((job.status, accepted_targets)));
        }
        let status = aggregate_job_status_from_targets(&targets);
        self.finish_job(job_id, status).await?;
        Ok(Some((status.to_string(), accepted_targets)))
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
                sqlx::query(
                    r#"
                    UPDATE job_targets
                    SET status = $3,
                        message = $4,
                        exit_code = $5,
                        started_at = COALESCE(started_at, now()),
                        completed_at = now(),
                        dispatch_lease_until = NULL,
                        last_dispatch_error = CASE WHEN $3 = 'dispatch_failed' THEN $4 ELSE NULL END
                    WHERE job_id = $1 AND client_id = $2
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .bind(&outcome.status)
                .bind(&outcome.message)
                .bind(outcome.exit_code)
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
        self.record_job_status_webhook_event(job_id, status).await?;
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
