use std::collections::HashSet;

use anyhow::{Context, Result};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    model::{
        AuthContext, BackupRequestStatus, BackupRequestView, CreateBackupRequest, JobOutputView,
        ListQuery,
    },
    repository::Repository,
    repository_key_lifecycle::require_visible_postgres_clients_in_tx,
    unix_now,
    util::{limit_or_default, offset_or_default, search_pattern, sort_descending},
};

const MAX_BACKUP_SCHEDULE_LINEAGE: usize = 16;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct BackupRequestSourceLink {
    pub(crate) job_id: Option<Uuid>,
    pub(crate) schedule_id: Option<Uuid>,
    pub(crate) causation_id: Option<Uuid>,
    pub(crate) schedule_lineage: Vec<Uuid>,
}

impl BackupRequestSourceLink {
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.schedule_lineage.len() <= MAX_BACKUP_SCHEDULE_LINEAGE,
            "backup_schedule_lineage_overflow"
        );
        let unique = self
            .schedule_lineage
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        anyhow::ensure!(
            unique.len() == self.schedule_lineage.len(),
            "backup_schedule_lineage_duplicate"
        );
        Ok(())
    }
}

fn backup_request_order_by(sort: Option<&str>, descending: bool) -> &'static str {
    match (sort.unwrap_or("created_at"), descending) {
        ("artifact_id" | "artifact", true) => "artifact_id DESC NULLS LAST, id DESC",
        ("artifact_id" | "artifact", false) => "artifact_id ASC NULLS LAST, id ASC",
        ("client_id" | "client", true) => "client_id DESC, id DESC",
        ("client_id" | "client", false) => "client_id ASC, id ASC",
        ("include_config" | "scope", true) => "include_config DESC, id DESC",
        ("include_config" | "scope", false) => "include_config ASC, id ASC",
        ("paths", true) => "cardinality(paths) DESC, id DESC",
        ("paths", false) => "cardinality(paths) ASC, id ASC",
        ("payload_hash" | "hash", true) => "payload_hash DESC, id DESC",
        ("payload_hash" | "hash", false) => "payload_hash ASC, id ASC",
        ("command_scope", true) => "command_scope DESC, id DESC",
        ("command_scope", false) => "command_scope ASC, id ASC",
        ("status", true) => "status DESC, id DESC",
        ("status", false) => "status ASC, id ASC",
        (_, true) => "created_at DESC, id DESC",
        (_, false) => "created_at ASC, id ASC",
    }
}

impl Repository {
    pub(crate) async fn list_dashboard_backup_requests(
        &self,
        client_ids: &[String],
        start_unix: u64,
        end_unix: u64,
        limit: i64,
    ) -> Result<Vec<BackupRequestView>> {
        if client_ids.is_empty() {
            return Ok(Vec::new());
        }
        // Dashboard callers request one sentinel row beyond the visible page
        // so count saturation can be disclosed instead of shown as exact.
        let limit = limit.clamp(1, 201);
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        client_id,
                        paths,
                        include_config,
                        follow_symlinks,
                        missing_path_policy,
                        status,
                        payload_hash,
                        command_scope,
                        artifact_id,
                        source_job_id,
                        source_schedule_id,
                        causation_id,
                        schedule_lineage,
                        note,
                        created_at::text AS created_at
                    FROM backup_requests
                    WHERE client_id = ANY($1::text[])
                      AND created_at >= to_timestamp($2::double precision)
                      AND created_at <= to_timestamp($3::double precision)
                    ORDER BY created_at DESC, id DESC
                    LIMIT $4
                    "#,
                )
                .bind(client_ids)
                .bind(start_unix as f64)
                .bind(end_unix as f64)
                .bind(limit)
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(backup_request_from_row).collect()
            }
        }
    }

    pub(crate) async fn query_backup_requests(
        &self,
        query: &ListQuery,
    ) -> Result<Vec<BackupRequestView>> {
        let limit = limit_or_default(query.limit);
        let offset = offset_or_default(query.offset);
        let descending = sort_descending(query.dir.as_deref(), true);
        match self {
            Self::Postgres(pool) => {
                let order_by = backup_request_order_by(query.sort.as_deref(), descending);
                let rows = sqlx::query(&format!(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        client_id,
                        paths,
                        include_config,
                        follow_symlinks,
                        missing_path_policy,
                        status,
                        payload_hash,
                        command_scope,
                        artifact_id,
                        source_job_id,
                        source_schedule_id,
                        causation_id,
                        schedule_lineage,
                        note,
                        created_at::text AS created_at
                    FROM backup_requests
                    WHERE (
                        $3::text IS NULL
                        OR id::text ILIKE $3 ESCAPE '\'
                        OR actor_id::text ILIKE $3 ESCAPE '\'
                        OR client_id ILIKE $3 ESCAPE '\'
                        OR array_to_string(paths, ' ') ILIKE $3 ESCAPE '\'
                        OR status ILIKE $3 ESCAPE '\'
                        OR payload_hash ILIKE $3 ESCAPE '\'
                        OR command_scope ILIKE $3 ESCAPE '\'
                        OR artifact_id::text ILIKE $3 ESCAPE '\'
                        OR source_job_id::text ILIKE $3 ESCAPE '\'
                        OR source_schedule_id::text ILIKE $3 ESCAPE '\'
                        OR causation_id::text ILIKE $3 ESCAPE '\'
                        OR array_to_string(schedule_lineage, ' ') ILIKE $3 ESCAPE '\'
                        OR note ILIKE $3 ESCAPE '\'
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
                rows.into_iter().map(backup_request_from_row).collect()
            }
        }
    }

    pub(crate) async fn record_backup_request(
        &self,
        request: &CreateBackupRequest,
        payload_hash: &str,
        command_scope: &str,
        operator: &AuthContext,
        status: BackupRequestStatus,
    ) -> Result<BackupRequestView> {
        self.record_backup_request_with_source(
            request,
            payload_hash,
            command_scope,
            operator,
            status,
            BackupRequestSourceLink::default(),
        )
        .await
    }

    pub(crate) async fn record_backup_request_with_source(
        &self,
        request: &CreateBackupRequest,
        payload_hash: &str,
        command_scope: &str,
        operator: &AuthContext,
        status: BackupRequestStatus,
        source: BackupRequestSourceLink,
    ) -> Result<BackupRequestView> {
        source.validate()?;
        let view = BackupRequestView {
            id: Uuid::new_v4(),
            actor_id: Some(operator.operator.id),
            client_id: request.client_id.clone(),
            paths: request.paths.clone(),
            include_config: request.include_config,
            follow_symlinks: request.follow_symlinks,
            missing_path_policy: request.missing_path_policy,
            status: status.as_str().to_string(),
            payload_hash: payload_hash.to_string(),
            command_scope: command_scope.to_string(),
            artifact_id: None,
            source_job_id: source.job_id,
            source_schedule_id: source.schedule_id,
            causation_id: source.causation_id,
            schedule_lineage: source.schedule_lineage.clone(),
            note: request.note.clone(),
            created_at: unix_now().to_string(),
        };
        let requires_live_target = status == BackupRequestStatus::RequestedMetadataOnly;
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                if requires_live_target {
                    require_visible_postgres_clients_in_tx(
                        &mut tx,
                        std::slice::from_ref(&view.client_id),
                        "backup_target_unavailable",
                    )
                    .await?;
                }
                let row = sqlx::query(
                    r#"
                    INSERT INTO backup_requests (
                        id,
                        actor_id,
                        client_id,
                        paths,
                        include_config,
                        follow_symlinks,
                        missing_path_policy,
                        status,
                        payload_hash,
                        command_scope,
                        artifact_id,
                        source_job_id,
                        source_schedule_id,
                        causation_id,
                        schedule_lineage,
                        note
                    )
                    VALUES (
                        $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NULL,
                        $11, $12, $13, $14, $15
                    )
                    RETURNING created_at::text AS created_at
                    "#,
                )
                .bind(view.id)
                .bind(operator.operator.id)
                .bind(&view.client_id)
                .bind(&view.paths)
                .bind(view.include_config)
                .bind(view.follow_symlinks)
                .bind(view.missing_path_policy.as_str())
                .bind(&view.status)
                .bind(&view.payload_hash)
                .bind(&view.command_scope)
                .bind(source.job_id)
                .bind(source.schedule_id)
                .bind(source.causation_id)
                .bind(&source.schedule_lineage)
                .bind(&view.note)
                .fetch_one(&mut *tx)
                .await?;
                let persisted = BackupRequestView {
                    created_at: row.try_get("created_at")?,
                    ..view
                };
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
                .bind("backup.requested_metadata_only")
                .bind(format!("backup_request:{}", persisted.id))
                .bind(&persisted.payload_hash)
                .bind(backup_request_metadata(
                    &persisted,
                    request.confirmed,
                    operator,
                ))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(persisted)
            }
        }
    }

    pub(crate) async fn attach_backup_request_source(
        &self,
        backup_request_id: Uuid,
        source: &BackupRequestSourceLink,
    ) -> Result<Option<BackupRequestView>> {
        source.validate()?;
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        client_id,
                        paths,
                        include_config,
                        follow_symlinks,
                        missing_path_policy,
                        status,
                        payload_hash,
                        command_scope,
                        artifact_id,
                        source_job_id,
                        source_schedule_id,
                        causation_id,
                        schedule_lineage,
                        note,
                        created_at::text AS created_at
                    FROM backup_requests
                    WHERE id = $1
                      AND source_job_id IS NOT DISTINCT FROM $2
                      AND source_schedule_id IS NOT DISTINCT FROM $3
                      AND causation_id IS NOT DISTINCT FROM $4
                      AND schedule_lineage = $5
                    "#,
                )
                .bind(backup_request_id)
                .bind(source.job_id)
                .bind(source.schedule_id)
                .bind(source.causation_id)
                .bind(&source.schedule_lineage)
                .fetch_optional(pool)
                .await?;
                row.map(backup_request_from_row).transpose()
            }
        }
    }

    pub(crate) async fn find_open_backup_request_for_source(
        &self,
        client_id: &str,
        payload_hash: &str,
        source: &BackupRequestSourceLink,
    ) -> Result<Option<BackupRequestView>> {
        source.validate()?;
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        client_id,
                        paths,
                        include_config,
                        follow_symlinks,
                        missing_path_policy,
                        status,
                        payload_hash,
                        command_scope,
                        artifact_id,
                        source_job_id,
                        source_schedule_id,
                        causation_id,
                        schedule_lineage,
                        note,
                        created_at::text AS created_at
                    FROM backup_requests
                    WHERE client_id = $1
                      AND payload_hash = $2
                      AND artifact_id IS NULL
                      AND status = 'requested_metadata_only'
                      AND source_job_id IS NOT DISTINCT FROM $3
                      AND source_schedule_id IS NOT DISTINCT FROM $4
                      AND causation_id IS NOT DISTINCT FROM $5
                      AND schedule_lineage = $6
                    ORDER BY created_at DESC, id DESC
                    LIMIT 1
                    "#,
                )
                .bind(client_id)
                .bind(payload_hash)
                .bind(source.job_id)
                .bind(source.schedule_id)
                .bind(source.causation_id)
                .bind(&source.schedule_lineage)
                .fetch_optional(pool)
                .await?;
                row.map(backup_request_from_row).transpose()
            }
        }
    }

    pub(crate) async fn find_open_backup_request_for_job_artifact(
        &self,
        client_id: &str,
        payload_hash: &str,
        source_job_id: Uuid,
    ) -> Result<Option<BackupRequestView>> {
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        client_id,
                        paths,
                        include_config,
                        follow_symlinks,
                        missing_path_policy,
                        status,
                        payload_hash,
                        command_scope,
                        artifact_id,
                        source_job_id,
                        source_schedule_id,
                        causation_id,
                        schedule_lineage,
                        note,
                        created_at::text AS created_at
                    FROM backup_requests
                    WHERE client_id = $1
                      AND payload_hash = $2
                      AND source_job_id = $3
                      AND artifact_id IS NULL
                      AND status = 'requested_metadata_only'
                    ORDER BY created_at DESC, id DESC
                    LIMIT 1
                    "#,
                )
                .bind(client_id)
                .bind(payload_hash)
                .bind(source_job_id)
                .fetch_optional(pool)
                .await?;
                row.map(backup_request_from_row).transpose()
            }
        }
    }

    pub(crate) async fn record_rejected_backup_request(
        &self,
        request: &CreateBackupRequest,
        payload_hash: &str,
        operator: &AuthContext,
        reason: &'static str,
    ) -> Result<()> {
        let metadata = backup_rejection_metadata(request, payload_hash, operator, reason);
        match self {
            Self::Postgres(pool) => {
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
                .bind("backup.rejected_authorization_required")
                .bind(format!("client:{}", request.client_id))
                .bind(payload_hash)
                .bind(metadata)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn mark_open_backup_request_execution_terminal(
        &self,
        job_id: Uuid,
        client_id: &str,
        status: BackupRequestStatus,
        operator: Option<&AuthContext>,
    ) -> Result<Option<BackupRequestView>> {
        self.mark_open_backup_request_execution_terminal_with_reason(
            job_id, client_id, status, operator, None,
        )
        .await
    }

    pub(crate) async fn mark_open_backup_request_artifact_validation_failed(
        &self,
        job_id: Uuid,
        client_id: &str,
        reason: &str,
    ) -> Result<Option<BackupRequestView>> {
        self.mark_open_backup_request_execution_terminal_with_reason(
            job_id,
            client_id,
            BackupRequestStatus::ExecutionFailed,
            None,
            Some(reason),
        )
        .await
    }

    async fn mark_open_backup_request_execution_terminal_with_reason(
        &self,
        job_id: Uuid,
        client_id: &str,
        status: BackupRequestStatus,
        operator: Option<&AuthContext>,
        reason: Option<&str>,
    ) -> Result<Option<BackupRequestView>> {
        if !matches!(
            status,
            BackupRequestStatus::ExecutionFailed | BackupRequestStatus::ExecutionCanceled
        ) {
            anyhow::bail!("invalid backup request terminal execution status");
        }
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let Some(row) = sqlx::query(
                    r#"
                    UPDATE backup_requests
                    SET status = $3
                    WHERE source_job_id = $1
                      AND client_id = $2
                      AND artifact_id IS NULL
                      AND status = 'requested_metadata_only'
                    RETURNING
                        id,
                        actor_id,
                        client_id,
                        paths,
                        include_config,
                        follow_symlinks,
                        missing_path_policy,
                        status,
                        payload_hash,
                        command_scope,
                        artifact_id,
                        source_job_id,
                        source_schedule_id,
                        causation_id,
                        schedule_lineage,
                        note,
                        created_at::text AS created_at
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .bind(status.as_str())
                .fetch_optional(&mut *tx)
                .await?
                else {
                    tx.commit().await?;
                    return Ok(None);
                };
                let view = backup_request_from_row(row)?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, $5, $6)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.map(|operator| operator.operator.id))
                .bind(backup_request_execution_action(&view.status))
                .bind(format!("backup_request:{}", view.id))
                .bind(&view.payload_hash)
                .bind(backup_request_execution_metadata(&view, operator, reason))
                .execute(&mut *tx)
                .await?;
                if status == BackupRequestStatus::ExecutionFailed {
                    crate::repository_operational_alerts::reconcile_postgres_backup_event_source_in_tx(
                        &mut tx,
                        view.id,
                    )
                    .await?;
                }
                tx.commit().await?;
                Ok(Some(view))
            }
        }
    }

    pub(crate) async fn find_backup_artifact_output_candidate(
        &self,
        backup_request: &BackupRequestView,
        selected_job_id: Option<Uuid>,
    ) -> Result<Option<BackupArtifactOutputCandidate>> {
        let job_id = match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        job.id
                    FROM jobs job
                    JOIN job_targets target
                      ON target.job_id = job.id
                     AND target.client_id = $2
                     AND target.status = 'completed'
                    WHERE job.operation->>'type' = 'backup'
                      AND job.payload_hash = $1
                      AND ($3::uuid IS NULL OR job.id = $3)
                      AND EXISTS (
                        SELECT 1
                        FROM job_outputs output
                        WHERE output.job_id = job.id
                          AND output.client_id = $2
                          AND output.stream = 'stdout'
                      )
                    ORDER BY job.created_at DESC, job.id DESC
                    LIMIT 1
                    "#,
                )
                .bind(&backup_request.payload_hash)
                .bind(&backup_request.client_id)
                .bind(selected_job_id)
                .fetch_optional(pool)
                .await?;
                row.map(|row| row.try_get("id")).transpose()?
            }
        };
        let Some(job_id) = job_id else {
            return Ok(None);
        };
        let outputs = self
            .list_job_outputs_for_target(job_id, &backup_request.client_id)
            .await?
            .into_iter()
            .filter(|output| output.stream == "stdout")
            .collect::<Vec<_>>();
        Ok((!outputs.is_empty()).then_some(BackupArtifactOutputCandidate { job_id, outputs }))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct BackupArtifactOutputCandidate {
    pub(crate) job_id: Uuid,
    pub(crate) outputs: Vec<JobOutputView>,
}

pub(crate) fn backup_request_from_row(row: sqlx::postgres::PgRow) -> Result<BackupRequestView> {
    let status: String = row.try_get("status")?;
    Ok(BackupRequestView {
        id: row.try_get("id")?,
        actor_id: row.try_get("actor_id")?,
        client_id: row.try_get("client_id")?,
        paths: row.try_get("paths")?,
        include_config: row.try_get("include_config")?,
        follow_symlinks: row.try_get("follow_symlinks")?,
        missing_path_policy: crate::model::BackupMissingPathPolicy::from_storage(
            row.try_get::<String, _>("missing_path_policy")?.as_str(),
        )
        .context("backup request missing path policy is invalid")?,
        status: BackupRequestStatus::from_storage(&status)
            .map(|status| status.as_str().to_string())
            .unwrap_or(status),
        payload_hash: row.try_get("payload_hash")?,
        command_scope: row.try_get("command_scope")?,
        artifact_id: row.try_get("artifact_id")?,
        source_job_id: row.try_get("source_job_id")?,
        source_schedule_id: row.try_get("source_schedule_id")?,
        causation_id: row.try_get("causation_id")?,
        schedule_lineage: row.try_get("schedule_lineage")?,
        note: row.try_get("note")?,
        created_at: row.try_get("created_at")?,
    })
}

fn backup_request_metadata(
    view: &BackupRequestView,
    confirmed: bool,
    operator: &AuthContext,
) -> serde_json::Value {
    json!({
        "client_id": &view.client_id,
        "paths": &view.paths,
        "include_config": view.include_config,
        "follow_symlinks": view.follow_symlinks,
        "missing_path_policy": view.missing_path_policy.as_str(),
        "status": &view.status,
        "payload_hash": &view.payload_hash,
        "command_scope": &view.command_scope,
        "artifact_id": view.artifact_id,
        "source_job_id": view.source_job_id,
        "source_schedule_id": view.source_schedule_id,
        "causation_id": view.causation_id,
        "schedule_lineage": &view.schedule_lineage,
        "confirmed": confirmed,
        "operator_id": operator.operator.id,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "operator_session_id": operator.audit_session_id(),
        "result": "requested",
        "origin_kind": "operator_request",
        "component": "backup-controller",
        "metadata_only": true,
    })
}

fn backup_request_execution_action(status: &str) -> &'static str {
    match status {
        "execution_canceled" => "backup.execution_canceled",
        _ => "backup.execution_failed",
    }
}

fn backup_request_execution_metadata(
    view: &BackupRequestView,
    operator: Option<&AuthContext>,
    reason: Option<&str>,
) -> serde_json::Value {
    let mut metadata = json!({
        "client_id": &view.client_id,
        "paths": &view.paths,
        "include_config": view.include_config,
        "follow_symlinks": view.follow_symlinks,
        "missing_path_policy": view.missing_path_policy.as_str(),
        "status": &view.status,
        "payload_hash": &view.payload_hash,
        "command_scope": &view.command_scope,
        "artifact_id": view.artifact_id,
        "source_job_id": view.source_job_id,
        "source_schedule_id": view.source_schedule_id,
        "causation_id": view.causation_id,
        "schedule_lineage": &view.schedule_lineage,
        "operator_id": operator.map(|operator| operator.operator.id),
        "operator_username": operator.map(|operator| operator.operator.username.as_str()),
        "operator_role": operator.map(|operator| operator.operator.role.as_str()),
        "operator_session_id": operator.and_then(AuthContext::audit_session_id),
        "result": &view.status,
        "origin_kind": if operator.is_some() { "operator_request" } else { "control_plane" },
        "component": "backup-controller",
        "metadata_only": true,
    });
    if let Some(reason) = reason {
        metadata["reason"] = serde_json::Value::String(reason.to_string());
        metadata["failure_phase"] = serde_json::Value::String("artifact_validation".to_string());
    }
    metadata
}

fn backup_rejection_metadata(
    request: &CreateBackupRequest,
    payload_hash: &str,
    operator: &AuthContext,
    reason: &'static str,
) -> serde_json::Value {
    json!({
        "client_id": &request.client_id,
        "paths": &request.paths,
        "include_config": request.include_config,
        "follow_symlinks": request.follow_symlinks,
        "missing_path_policy": request.missing_path_policy.as_str(),
        "confirmed": request.confirmed,
        "payload_hash": payload_hash,
        "reason": reason,
        "result": "rejected",
        "operator_id": operator.operator.id,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "operator_session_id": operator.audit_session_id(),
        "origin_kind": "operator_request",
        "component": "backup-controller",
        "metadata_only": true,
    })
}
