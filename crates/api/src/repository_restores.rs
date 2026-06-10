use std::cmp::Ordering;

use anyhow::Result;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    model::{
        AuditLogView, AuthContext, BackupRequestView, CreateRestorePlanRequest, ListQuery,
        RestorePlanStatus, RestorePlanView,
    },
    repository::Repository,
    repository_backups::backup_request_from_row,
    unix_now,
    util::{limit_or_default, offset_or_default, search_pattern, sort_descending},
};

fn compare_text_or_number(left: &str, right: &str) -> Ordering {
    match (left.parse::<i128>(), right.parse::<i128>()) {
        (Ok(left), Ok(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

fn compare_restore_plan(
    left: &RestorePlanView,
    right: &RestorePlanView,
    sort: Option<&str>,
) -> Ordering {
    match sort.unwrap_or("created_at") {
        "destination_root" | "destination" => left.destination_root.cmp(&right.destination_root),
        "include_config" | "scope" => left.include_config.cmp(&right.include_config),
        "paths" => left.paths.len().cmp(&right.paths.len()),
        "payload_hash" | "hash" => left.payload_hash.cmp(&right.payload_hash),
        "source_client_id" | "source" => left.source_client_id.cmp(&right.source_client_id),
        "status" => left.status.cmp(&right.status),
        "target_client_id" | "target" => left.target_client_id.cmp(&right.target_client_id),
        _ => compare_text_or_number(&left.created_at, &right.created_at),
    }
}

fn restore_plan_matches_search(plan: &RestorePlanView, needle: &str) -> bool {
    plan.id.to_string().to_ascii_lowercase().contains(needle)
        || plan
            .actor_id
            .map(|id| id.to_string().to_ascii_lowercase().contains(needle))
            .unwrap_or(false)
        || plan
            .source_backup_request_id
            .to_string()
            .to_ascii_lowercase()
            .contains(needle)
        || plan.source_client_id.to_ascii_lowercase().contains(needle)
        || plan.target_client_id.to_ascii_lowercase().contains(needle)
        || plan.status.to_ascii_lowercase().contains(needle)
        || plan.payload_hash.to_ascii_lowercase().contains(needle)
        || plan
            .destination_root
            .as_deref()
            .map(|value| value.to_ascii_lowercase().contains(needle))
            .unwrap_or(false)
        || plan
            .note
            .as_deref()
            .map(|value| value.to_ascii_lowercase().contains(needle))
            .unwrap_or(false)
        || plan
            .paths
            .iter()
            .any(|path| path.to_ascii_lowercase().contains(needle))
}

fn restore_plan_order_by(sort: Option<&str>, descending: bool) -> &'static str {
    match (sort.unwrap_or("created_at"), descending) {
        ("destination_root" | "destination", true) => "destination_root DESC NULLS LAST, id DESC",
        ("destination_root" | "destination", false) => "destination_root ASC NULLS LAST, id ASC",
        ("include_config" | "scope", true) => "include_config DESC, id DESC",
        ("include_config" | "scope", false) => "include_config ASC, id ASC",
        ("paths", true) => "cardinality(paths) DESC, id DESC",
        ("paths", false) => "cardinality(paths) ASC, id ASC",
        ("payload_hash" | "hash", true) => "payload_hash DESC, id DESC",
        ("payload_hash" | "hash", false) => "payload_hash ASC, id ASC",
        ("source_client_id" | "source", true) => "source_client_id DESC, id DESC",
        ("source_client_id" | "source", false) => "source_client_id ASC, id ASC",
        ("status", true) => "status DESC, id DESC",
        ("status", false) => "status ASC, id ASC",
        ("target_client_id" | "target", true) => "target_client_id DESC, id DESC",
        ("target_client_id" | "target", false) => "target_client_id ASC, id ASC",
        (_, true) => "created_at DESC, id DESC",
        (_, false) => "created_at ASC, id ASC",
    }
}

impl Repository {
    pub(crate) async fn list_restore_plans(&self, limit: i64) -> Result<Vec<RestorePlanView>> {
        match self {
            Self::Memory(memory) => {
                let plans = memory.restore_plans.read().await;
                Ok(plans.iter().rev().take(limit as usize).cloned().collect())
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        source_backup_request_id,
                        source_client_id,
                        target_client_id,
                        paths,
                        include_config,
                        destination_root,
                        status,
                        payload_hash,
                        command_scope,
                        note,
                        created_at::text AS created_at
                    FROM restore_plans
                    ORDER BY created_at DESC, id DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(restore_plan_from_row).collect()
            }
        }
    }

    pub(crate) async fn query_restore_plans(
        &self,
        query: &ListQuery,
    ) -> Result<Vec<RestorePlanView>> {
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
                let mut plans = memory
                    .restore_plans
                    .read()
                    .await
                    .iter()
                    .filter(|plan| {
                        q.as_deref()
                            .map(|needle| restore_plan_matches_search(plan, needle))
                            .unwrap_or(true)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                plans.sort_by(|left, right| {
                    compare_restore_plan(left, right, query.sort.as_deref())
                        .then_with(|| left.id.cmp(&right.id))
                });
                if descending {
                    plans.reverse();
                }
                Ok(plans
                    .into_iter()
                    .skip(offset as usize)
                    .take(limit as usize)
                    .collect())
            }
            Self::Postgres(pool) => {
                let order_by = restore_plan_order_by(query.sort.as_deref(), descending);
                let rows = sqlx::query(&format!(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        source_backup_request_id,
                        source_client_id,
                        target_client_id,
                        paths,
                        include_config,
                        destination_root,
                        status,
                        payload_hash,
                        command_scope,
                        note,
                        created_at::text AS created_at
                    FROM restore_plans
                    WHERE (
                        $3::text IS NULL
                        OR id::text ILIKE $3 ESCAPE '\'
                        OR actor_id::text ILIKE $3 ESCAPE '\'
                        OR source_backup_request_id::text ILIKE $3 ESCAPE '\'
                        OR source_client_id ILIKE $3 ESCAPE '\'
                        OR target_client_id ILIKE $3 ESCAPE '\'
                        OR array_to_string(paths, ' ') ILIKE $3 ESCAPE '\'
                        OR destination_root ILIKE $3 ESCAPE '\'
                        OR status ILIKE $3 ESCAPE '\'
                        OR payload_hash ILIKE $3 ESCAPE '\'
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
                rows.into_iter().map(restore_plan_from_row).collect()
            }
        }
    }

    pub(crate) async fn find_backup_request(&self, id: Uuid) -> Result<Option<BackupRequestView>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .backup_requests
                .read()
                .await
                .iter()
                .find(|request| request.id == id)
                .cloned()),
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        client_id,
                        paths,
                        include_config,
                        status,
                        payload_hash,
                        command_scope,
                        artifact_id,
                        source_job_id,
                        source_schedule_id,
                        note,
                        created_at::text AS created_at
                    FROM backup_requests
                    WHERE id = $1
                    "#,
                )
                .bind(id)
                .fetch_optional(pool)
                .await?;
                row.map(backup_request_from_row).transpose()
            }
        }
    }

    pub(crate) async fn record_restore_plan(
        &self,
        request: &CreateRestorePlanRequest,
        source_backup: &BackupRequestView,
        payload_hash: &str,
        command_scope: &str,
        operator: &AuthContext,
        status: RestorePlanStatus,
    ) -> Result<RestorePlanView> {
        let view = RestorePlanView {
            id: Uuid::new_v4(),
            actor_id: Some(operator.operator.id),
            source_backup_request_id: request.source_backup_request_id,
            source_client_id: source_backup.client_id.clone(),
            target_client_id: request.target_client_id.clone(),
            paths: request.paths.clone(),
            include_config: request.include_config,
            destination_root: request.destination_root.clone(),
            status: status.as_str().to_string(),
            payload_hash: payload_hash.to_string(),
            command_scope: command_scope.to_string(),
            note: request.note.clone(),
            created_at: unix_now().to_string(),
        };
        match self {
            Self::Memory(memory) => {
                memory.restore_plans.write().await.push(view.clone());
                memory.audits.write().await.push(restore_plan_audit(
                    &view,
                    request.confirmed,
                    operator,
                    unix_now().to_string(),
                ));
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    INSERT INTO restore_plans (
                        id,
                        actor_id,
                        source_backup_request_id,
                        source_client_id,
                        target_client_id,
                        paths,
                        include_config,
                        destination_root,
                        status,
                        payload_hash,
                        command_scope,
                        note
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                    RETURNING created_at::text AS created_at
                    "#,
                )
                .bind(view.id)
                .bind(operator.operator.id)
                .bind(view.source_backup_request_id)
                .bind(&view.source_client_id)
                .bind(&view.target_client_id)
                .bind(&view.paths)
                .bind(view.include_config)
                .bind(&view.destination_root)
                .bind(&view.status)
                .bind(&view.payload_hash)
                .bind(&view.command_scope)
                .bind(&view.note)
                .fetch_one(&mut *tx)
                .await?;
                let persisted = RestorePlanView {
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
                .bind("restore.planned_metadata_only")
                .bind(format!("restore_plan:{}", persisted.id))
                .bind(&persisted.payload_hash)
                .bind(restore_plan_metadata(
                    &persisted,
                    request.confirmed,
                    operator,
                ))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                return Ok(persisted);
            }
        }
        Ok(view)
    }

    pub(crate) async fn record_rejected_restore_plan(
        &self,
        request: &CreateRestorePlanRequest,
        payload_hash: Option<&str>,
        operator: &AuthContext,
        reason: &'static str,
    ) -> Result<()> {
        let metadata = restore_rejection_metadata(request, payload_hash, operator, reason);
        match self {
            Self::Memory(memory) => {
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "restore.rejected_authorization_required".to_string(),
                    target: format!("client:{}", request.target_client_id),
                    command_hash: payload_hash.map(ToOwned::to_owned),
                    metadata,
                    created_at: unix_now().to_string(),
                });
            }
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
                .bind("restore.rejected_authorization_required")
                .bind(format!("client:{}", request.target_client_id))
                .bind(payload_hash)
                .bind(metadata)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }
}

fn restore_plan_from_row(row: sqlx::postgres::PgRow) -> Result<RestorePlanView> {
    let status: String = row.try_get("status")?;
    Ok(RestorePlanView {
        id: row.try_get("id")?,
        actor_id: row.try_get("actor_id")?,
        source_backup_request_id: row.try_get("source_backup_request_id")?,
        source_client_id: row.try_get("source_client_id")?,
        target_client_id: row.try_get("target_client_id")?,
        paths: row.try_get("paths")?,
        include_config: row.try_get("include_config")?,
        destination_root: row.try_get("destination_root")?,
        status: RestorePlanStatus::from_storage(&status)
            .map(|status| status.as_str().to_string())
            .unwrap_or(status),
        payload_hash: row.try_get("payload_hash")?,
        command_scope: row.try_get("command_scope")?,
        note: row.try_get("note")?,
        created_at: row.try_get("created_at")?,
    })
}

fn restore_plan_audit(
    view: &RestorePlanView,
    confirmed: bool,
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: "restore.planned_metadata_only".to_string(),
        target: format!("restore_plan:{}", view.id),
        command_hash: Some(view.payload_hash.clone()),
        metadata: restore_plan_metadata(view, confirmed, operator),
        created_at,
    }
}

fn restore_plan_metadata(
    view: &RestorePlanView,
    confirmed: bool,
    operator: &AuthContext,
) -> serde_json::Value {
    json!({
        "source_backup_request_id": view.source_backup_request_id,
        "source_client_id": &view.source_client_id,
        "target_client_id": &view.target_client_id,
        "paths": &view.paths,
        "include_config": view.include_config,
        "destination_root": &view.destination_root,
        "status": &view.status,
        "payload_hash": &view.payload_hash,
        "command_scope": &view.command_scope,
        "confirmed": confirmed,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "session_id": operator.session_id,
        "metadata_only": true,
    })
}

fn restore_rejection_metadata(
    request: &CreateRestorePlanRequest,
    payload_hash: Option<&str>,
    operator: &AuthContext,
    reason: &'static str,
) -> serde_json::Value {
    json!({
        "source_backup_request_id": request.source_backup_request_id,
        "target_client_id": &request.target_client_id,
        "paths": &request.paths,
        "include_config": request.include_config,
        "destination_root": &request.destination_root,
        "confirmed": request.confirmed,
        "payload_hash": payload_hash,
        "reason": reason,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "session_id": operator.session_id,
        "metadata_only": true,
    })
}
