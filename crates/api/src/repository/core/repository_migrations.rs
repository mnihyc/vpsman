use anyhow::{bail, Result};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;
use vpsman_server_core::{JOB_STATUS_QUEUED, TARGET_STATUS_QUEUED};

use crate::{
    model::{
        AuthContext, CreateJobRequest, CreateMigrationLinkRequest, ListQuery, MigrationLinkStatus,
        MigrationLinkView, RestorePlanStatus, RestorePlanView,
    },
    repository::Repository,
    repository_jobs::{record_job_created_webhook_event_in_tx, JobCreatedWebhookEvent},
    repository_key_lifecycle::require_visible_postgres_clients_in_tx,
    unix_now,
    util::{limit_or_default, offset_or_default, search_pattern, sort_descending},
};

fn migration_link_order_by(sort: Option<&str>, descending: bool) -> &'static str {
    match (sort.unwrap_or("created_at"), descending) {
        ("destination_root" | "destination", true) => "destination_root DESC NULLS LAST, id DESC",
        ("destination_root" | "destination", false) => "destination_root ASC NULLS LAST, id ASC",
        ("include_config" | "scope", true) => "include_config DESC, id DESC",
        ("include_config" | "scope", false) => "include_config ASC, id ASC",
        ("paths", true) => "cardinality(paths) DESC, id DESC",
        ("paths", false) => "cardinality(paths) ASC, id ASC",
        ("restore_plan_id" | "plan", true) => "restore_plan_id DESC, id DESC",
        ("restore_plan_id" | "plan", false) => "restore_plan_id ASC, id ASC",
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
    pub(crate) async fn query_migration_links(
        &self,
        query: &ListQuery,
    ) -> Result<Vec<MigrationLinkView>> {
        let limit = limit_or_default(query.limit);
        let offset = offset_or_default(query.offset);
        let descending = sort_descending(query.dir.as_deref(), true);
        match self {
            Self::Postgres(pool) => {
                let order_by = migration_link_order_by(query.sort.as_deref(), descending);
                let rows = sqlx::query(&format!(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        restore_plan_id,
                        source_backup_request_id,
                        source_client_id,
                        target_client_id,
                        paths,
                        include_config,
                        destination_root,
                        status,
                        note,
                        created_at::text AS created_at
                    FROM migration_links
                    WHERE (
                        $3::text IS NULL
                        OR id::text ILIKE $3 ESCAPE '\'
                        OR actor_id::text ILIKE $3 ESCAPE '\'
                        OR restore_plan_id::text ILIKE $3 ESCAPE '\'
                        OR source_backup_request_id::text ILIKE $3 ESCAPE '\'
                        OR source_client_id ILIKE $3 ESCAPE '\'
                        OR target_client_id ILIKE $3 ESCAPE '\'
                        OR array_to_string(paths, ' ') ILIKE $3 ESCAPE '\'
                        OR destination_root ILIKE $3 ESCAPE '\'
                        OR status ILIKE $3 ESCAPE '\'
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
                rows.into_iter().map(migration_link_from_row).collect()
            }
        }
    }

    pub(crate) async fn find_restore_plan(&self, id: Uuid) -> Result<Option<RestorePlanView>> {
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
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
                    WHERE id = $1
                    "#,
                )
                .bind(id)
                .fetch_optional(pool)
                .await?;
                row.map(restore_plan_from_row).transpose()
            }
        }
    }

    pub(crate) async fn record_migration_link(
        &self,
        request: &CreateMigrationLinkRequest,
        restore_plan: &RestorePlanView,
        operator: &AuthContext,
        status: MigrationLinkStatus,
    ) -> Result<MigrationLinkView> {
        let view = migration_link_view_from_request(request, restore_plan, operator, status);
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                require_visible_postgres_clients_in_tx(
                    &mut tx,
                    std::slice::from_ref(&view.target_client_id),
                    "migration_target_unavailable",
                )
                .await?;
                let row = sqlx::query(
                    r#"
                    INSERT INTO migration_links (
                        id,
                        actor_id,
                        restore_plan_id,
                        source_backup_request_id,
                        source_client_id,
                        target_client_id,
                        paths,
                        include_config,
                        destination_root,
                        status,
                        note
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                    ON CONFLICT (restore_plan_id) DO NOTHING
                    RETURNING created_at::text AS created_at
                    "#,
                )
                .bind(view.id)
                .bind(operator.operator.id)
                .bind(view.restore_plan_id)
                .bind(view.source_backup_request_id)
                .bind(&view.source_client_id)
                .bind(&view.target_client_id)
                .bind(&view.paths)
                .bind(view.include_config)
                .bind(&view.destination_root)
                .bind(&view.status)
                .bind(&view.note)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(row) = row else {
                    tx.commit().await?;
                    anyhow::bail!("migration_link_already_exists");
                };
                let persisted = MigrationLinkView {
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
                .bind("migration.linked_metadata_only")
                .bind(format!("migration_link:{}", persisted.id))
                .bind(&restore_plan.payload_hash)
                .bind(migration_link_metadata(
                    &persisted,
                    restore_plan,
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

    pub(crate) async fn record_migration_run_restore_job(
        &self,
        link_request: &CreateMigrationLinkRequest,
        restore_plan: &RestorePlanView,
        operator: &AuthContext,
        job_id: Uuid,
        job_request: &CreateJobRequest,
        command_hash: &str,
        request_fingerprint: &str,
        resolved_targets: &[String],
    ) -> Result<MigrationLinkView> {
        let requested_link = migration_link_view_from_request(
            link_request,
            restore_plan,
            operator,
            MigrationLinkStatus::LinkedMetadataOnly,
        );
        let command_type = job_request.command_type_label().to_string();
        let operation = job_request
            .job_command()
            .map_err(|error| anyhow::anyhow!(error.code))?;
        let max_timeout_secs = job_request
            .max_timeout_secs
            .unwrap_or(vpsman_common::DEFAULT_MAX_JOB_TIMEOUT_SECS)
            .max(1);
        match self {
            Self::Postgres(pool) => {
                let target_write_order =
                    crate::repository_jobs::canonical_target_write_order(resolved_targets);
                let mut tx = pool.begin().await?;
                require_visible_postgres_clients_in_tx(
                    &mut tx,
                    resolved_targets,
                    "migration_target_unavailable",
                )
                .await?;
                let locked_restore_plan_id: Option<Uuid> = sqlx::query_scalar(
                    r#"
                    SELECT id
                    FROM restore_plans
                    WHERE id = $1
                      AND status = 'planned_metadata_only'
                    FOR UPDATE
                    "#,
                )
                .bind(restore_plan.id)
                .fetch_optional(&mut *tx)
                .await?;
                if locked_restore_plan_id.is_none() {
                    bail!("migration_restore_plan_not_metadata_only");
                }
                let job_id_exists: bool = sqlx::query_scalar(
                    r#"
                    SELECT EXISTS (
                        SELECT 1
                        FROM jobs
                        WHERE id = $1
                    )
                    "#,
                )
                .bind(job_id)
                .fetch_one(&mut *tx)
                .await?;
                if job_id_exists {
                    bail!("job_id_reused_with_different_request");
                }
                let existing_row = sqlx::query(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        restore_plan_id,
                        source_backup_request_id,
                        source_client_id,
                        target_client_id,
                        paths,
                        include_config,
                        destination_root,
                        status,
                        note,
                        created_at::text AS created_at
                    FROM migration_links
                    WHERE restore_plan_id = $1
                    FOR UPDATE
                    "#,
                )
                .bind(restore_plan.id)
                .fetch_optional(&mut *tx)
                .await?;
                let (persisted_link, inserted_link) = if let Some(row) = existing_row {
                    let existing = migration_link_from_row(row)?;
                    if !migration_link_matches_request(&existing, link_request, restore_plan) {
                        bail!("migration_link_conflicts_with_request");
                    }
                    (existing, false)
                } else {
                    let inserted_row = sqlx::query(
                        r#"
                        INSERT INTO migration_links (
                            id,
                            actor_id,
                            restore_plan_id,
                            source_backup_request_id,
                            source_client_id,
                            target_client_id,
                            paths,
                            include_config,
                            destination_root,
                            status,
                            note
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                        ON CONFLICT (restore_plan_id) DO NOTHING
                        RETURNING created_at::text AS created_at
                        "#,
                    )
                    .bind(requested_link.id)
                    .bind(operator.operator.id)
                    .bind(requested_link.restore_plan_id)
                    .bind(requested_link.source_backup_request_id)
                    .bind(&requested_link.source_client_id)
                    .bind(&requested_link.target_client_id)
                    .bind(&requested_link.paths)
                    .bind(requested_link.include_config)
                    .bind(&requested_link.destination_root)
                    .bind(&requested_link.status)
                    .bind(&requested_link.note)
                    .fetch_optional(&mut *tx)
                    .await?;
                    if let Some(row) = inserted_row {
                        (
                            MigrationLinkView {
                                created_at: row.try_get("created_at")?,
                                ..requested_link.clone()
                            },
                            true,
                        )
                    } else {
                        let row = sqlx::query(
                            r#"
                            SELECT
                                id,
                                actor_id,
                                restore_plan_id,
                                source_backup_request_id,
                                source_client_id,
                                target_client_id,
                                paths,
                                include_config,
                                destination_root,
                                status,
                                note,
                                created_at::text AS created_at
                            FROM migration_links
                            WHERE restore_plan_id = $1
                            FOR UPDATE
                            "#,
                        )
                        .bind(restore_plan.id)
                        .fetch_one(&mut *tx)
                        .await?;
                        let existing = migration_link_from_row(row)?;
                        if !migration_link_matches_request(&existing, link_request, restore_plan) {
                            bail!("migration_link_conflicts_with_request");
                        }
                        (existing, false)
                    }
                };
                sqlx::query(
                    r#"
                    INSERT INTO jobs (
                        id, actor_id, command_type, privileged, status,
                        target_count, payload_hash, operation, source_schedule_id,
                        request_fingerprint, max_timeout_secs
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NULL, $9, $10)
                    "#,
                )
                .bind(job_id)
                .bind(operator.operator.id)
                .bind(&command_type)
                .bind(job_request.privileged)
                .bind(JOB_STATUS_QUEUED)
                .bind(resolved_targets.len() as i32)
                .bind(command_hash)
                .bind(sqlx::types::Json(operation.clone()))
                .bind(request_fingerprint)
                .bind(max_timeout_secs as i64)
                .execute(&mut *tx)
                .await?;
                for client_id in target_write_order {
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
                if inserted_link {
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
                    .bind("migration.linked_metadata_only")
                    .bind(format!("migration_link:{}", persisted_link.id))
                    .bind(&restore_plan.payload_hash)
                    .bind(migration_link_metadata(
                        &persisted_link,
                        restore_plan,
                        link_request.confirmed,
                        operator,
                    ))
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
                .bind(migration_job_audit_metadata(
                    job_id,
                    &persisted_link,
                    restore_plan,
                    job_request,
                    operator,
                    resolved_targets,
                ))
                .execute(&mut *tx)
                .await?;
                record_job_created_webhook_event_in_tx(
                    &mut tx,
                    JobCreatedWebhookEvent {
                        job_id,
                        command_type: &command_type,
                        status: JOB_STATUS_QUEUED,
                        privileged: job_request.privileged,
                        command_hash,
                        resolved_targets,
                        actor_id: Some(operator.operator.id),
                        source_schedule_id: None,
                        operation: Some(&operation),
                    },
                )
                .await?;
                tx.commit().await?;
                Ok(persisted_link)
            }
        }
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

fn migration_link_from_row(row: sqlx::postgres::PgRow) -> Result<MigrationLinkView> {
    let status: String = row.try_get("status")?;
    Ok(MigrationLinkView {
        id: row.try_get("id")?,
        actor_id: row.try_get("actor_id")?,
        restore_plan_id: row.try_get("restore_plan_id")?,
        source_backup_request_id: row.try_get("source_backup_request_id")?,
        source_client_id: row.try_get("source_client_id")?,
        target_client_id: row.try_get("target_client_id")?,
        paths: row.try_get("paths")?,
        include_config: row.try_get("include_config")?,
        destination_root: row.try_get("destination_root")?,
        status: MigrationLinkStatus::from_storage(&status)
            .map(|status| status.as_str().to_string())
            .unwrap_or(status),
        note: row.try_get("note")?,
        created_at: row.try_get("created_at")?,
    })
}

fn migration_link_view_from_request(
    request: &CreateMigrationLinkRequest,
    restore_plan: &RestorePlanView,
    operator: &AuthContext,
    status: MigrationLinkStatus,
) -> MigrationLinkView {
    MigrationLinkView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        restore_plan_id: request.restore_plan_id,
        source_backup_request_id: restore_plan.source_backup_request_id,
        source_client_id: restore_plan.source_client_id.clone(),
        target_client_id: restore_plan.target_client_id.clone(),
        paths: restore_plan.paths.clone(),
        include_config: restore_plan.include_config,
        destination_root: restore_plan.destination_root.clone(),
        status: status.as_str().to_string(),
        note: request.note.clone(),
        created_at: unix_now().to_string(),
    }
}

fn migration_link_matches_request(
    existing: &MigrationLinkView,
    request: &CreateMigrationLinkRequest,
    restore_plan: &RestorePlanView,
) -> bool {
    existing.restore_plan_id == request.restore_plan_id
        && existing.source_backup_request_id == restore_plan.source_backup_request_id
        && existing.source_client_id == restore_plan.source_client_id
        && existing.target_client_id == restore_plan.target_client_id
        && existing.paths == restore_plan.paths
        && existing.include_config == restore_plan.include_config
        && existing.destination_root == restore_plan.destination_root
        && existing.status == MigrationLinkStatus::LinkedMetadataOnly.as_str()
        && existing.note == request.note
}

fn migration_job_audit_metadata(
    job_id: Uuid,
    link: &MigrationLinkView,
    restore_plan: &RestorePlanView,
    job_request: &CreateJobRequest,
    operator: &AuthContext,
    resolved_targets: &[String],
) -> serde_json::Value {
    json!({
        "job_id": job_id,
        "selector_expression": job_request.selector_expression,
        "target_client_ids": resolved_targets,
        "destructive": job_request.destructive,
        "confirmed": job_request.confirmed,
        "privileged": job_request.privileged,
        "force_unprivileged": job_request.force_unprivileged,
        "source_schedule_id": null,
        "result": "requested",
        "operator_id": operator.operator.id,
        "operator_username": operator.operator.username,
        "operator_role": operator.operator.role,
        "operator_session_id": operator.audit_session_id(),
        "origin_kind": "operator_request",
        "component": "migration-controller",
        "migration_link_id": link.id,
        "restore_plan_id": restore_plan.id,
    })
}

fn migration_link_metadata(
    view: &MigrationLinkView,
    restore_plan: &RestorePlanView,
    confirmed: bool,
    operator: &AuthContext,
) -> serde_json::Value {
    let mut metadata = migration_link_metadata_from_view(view, confirmed, operator);
    metadata["restore_plan_payload_hash"] = json!(restore_plan.payload_hash);
    metadata["restore_plan_command_scope"] = json!(restore_plan.command_scope);
    metadata
}

fn migration_link_metadata_from_view(
    view: &MigrationLinkView,
    confirmed: bool,
    operator: &AuthContext,
) -> serde_json::Value {
    json!({
        "restore_plan_id": view.restore_plan_id,
        "source_backup_request_id": view.source_backup_request_id,
        "source_client_id": &view.source_client_id,
        "target_client_id": &view.target_client_id,
        "paths": &view.paths,
        "include_config": view.include_config,
        "destination_root": &view.destination_root,
        "status": &view.status,
        "confirmed": confirmed,
        "operator_id": operator.operator.id,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "operator_session_id": operator.audit_session_id(),
        "result": "succeeded",
        "origin_kind": "operator_request",
        "component": "migration-controller",
        "metadata_only": true,
    })
}
