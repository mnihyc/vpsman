use std::cmp::Ordering;

use anyhow::Result;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;
use vpsman_common::JobCommand;

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
                    completed_at: Some(created_at.clone()),
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
                                message: Some("authorization required".to_string()),
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
                        reconnect_policy, completed_at
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, COALESCE($10, '{"duplicate_delivery":"ignore_completed","resume_outputs":true}'::jsonb), now())
                    "#,
                )
                .bind(job_id)
                .bind(operator.operator.id)
                .bind("api_job_request")
                .bind(request.privileged)
                .bind("rejected_authorization_required")
                .bind(resolved_targets.len() as i32)
                .bind(command_hash)
                .bind(operation.clone().map(sqlx::types::Json))
                .bind(request.idempotency_key.as_deref())
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
                    .bind("rejected_authorization_required")
                    .bind("authorization required")
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
        self.record_job_created_webhook_event(JobCreatedWebhookEvent {
            job_id,
            command_type: "api_job_request",
            status: "rejected_authorization_required",
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
        request: &CreateJobRequest,
        command_hash: &str,
        operator: &AuthContext,
        resolved_targets: &[String],
    ) -> Result<Uuid> {
        self.record_dispatching_job_with_source(
            request,
            command_hash,
            operator,
            resolved_targets,
            None,
        )
        .await
    }

    pub(crate) async fn record_dispatching_job_from_schedule(
        &self,
        request: &CreateJobRequest,
        command_hash: &str,
        operator: &AuthContext,
        resolved_targets: &[String],
        source_schedule_id: Uuid,
    ) -> Result<Uuid> {
        self.record_dispatching_job_with_source(
            request,
            command_hash,
            operator,
            resolved_targets,
            Some(source_schedule_id),
        )
        .await
    }

    async fn record_dispatching_job_with_source(
        &self,
        request: &CreateJobRequest,
        command_hash: &str,
        operator: &AuthContext,
        resolved_targets: &[String],
        source_schedule_id: Option<Uuid>,
    ) -> Result<Uuid> {
        let job_id = Uuid::new_v4();
        let command_type = request.command_type_label().to_string();
        let metadata = json!({
            "selector_expression": request.selector_expression,
            "resolved_targets": resolved_targets,
            "destructive": request.destructive,
            "confirmed": request.confirmed,
            "privileged": request.privileged,
            "force_unprivileged": request.force_unprivileged,
            "idempotency_key": request.idempotency_key,
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
                        target_count, payload_hash, operation, source_schedule_id, idempotency_key,
                        reconnect_policy
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, COALESCE($11, '{"duplicate_delivery":"ignore_completed","resume_outputs":true}'::jsonb))
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
                .bind(request.idempotency_key.as_deref())
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

    pub(crate) async fn mark_job_targets_dispatching(
        &self,
        job_id: Uuid,
        client_ids: &[String],
    ) -> Result<()> {
        if client_ids.is_empty() {
            return Ok(());
        }
        match self {
            Self::Memory(memory) => {
                let started_at = unix_now().to_string();
                let mut targets = memory.job_targets.write().await;
                for target in targets.iter_mut().filter(|target| {
                    target.job_id == job_id
                        && client_ids
                            .iter()
                            .any(|client_id| client_id == &target.client_id)
                        && target.completed_at.is_none()
                }) {
                    target.status = "dispatching".to_string();
                    target.started_at.get_or_insert_with(|| started_at.clone());
                }
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    UPDATE job_targets
                    SET status = 'dispatching',
                        started_at = COALESCE(started_at, now())
                    WHERE job_id = $1
                      AND client_id = ANY($2)
                      AND completed_at IS NULL
                    "#,
                )
                .bind(job_id)
                .bind(client_ids.to_vec())
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
                        message = $4,
                        exit_code = $5,
                        started_at = COALESCE(started_at, now()),
                        completed_at = now()
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
