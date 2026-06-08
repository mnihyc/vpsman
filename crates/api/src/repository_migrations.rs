use std::cmp::Ordering;

use anyhow::Result;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    model::{
        AuditLogView, AuthContext, CreateMigrationLinkRequest, ListQuery, MigrationLinkStatus,
        MigrationLinkView, RestorePlanStatus, RestorePlanView,
    },
    repository::Repository,
    unix_now,
    util::{limit_or_default, offset_or_default, search_pattern, sort_descending},
};

fn compare_text_or_number(left: &str, right: &str) -> Ordering {
    match (left.parse::<i128>(), right.parse::<i128>()) {
        (Ok(left), Ok(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

fn compare_migration_link(
    left: &MigrationLinkView,
    right: &MigrationLinkView,
    sort: Option<&str>,
) -> Ordering {
    match sort.unwrap_or("created_at") {
        "destination_root" | "destination" => left.destination_root.cmp(&right.destination_root),
        "include_config" | "scope" => left.include_config.cmp(&right.include_config),
        "paths" => left.paths.len().cmp(&right.paths.len()),
        "restore_plan_id" | "plan" => left.restore_plan_id.cmp(&right.restore_plan_id),
        "source_client_id" | "source" => left.source_client_id.cmp(&right.source_client_id),
        "status" => left.status.cmp(&right.status),
        "target_client_id" | "target" => left.target_client_id.cmp(&right.target_client_id),
        _ => compare_text_or_number(&left.created_at, &right.created_at),
    }
}

fn migration_link_matches_search(link: &MigrationLinkView, needle: &str) -> bool {
    link.id.to_string().to_ascii_lowercase().contains(needle)
        || link
            .actor_id
            .map(|id| id.to_string().to_ascii_lowercase().contains(needle))
            .unwrap_or(false)
        || link
            .restore_plan_id
            .to_string()
            .to_ascii_lowercase()
            .contains(needle)
        || link
            .source_backup_request_id
            .to_string()
            .to_ascii_lowercase()
            .contains(needle)
        || link.source_client_id.to_ascii_lowercase().contains(needle)
        || link.target_client_id.to_ascii_lowercase().contains(needle)
        || link.status.to_ascii_lowercase().contains(needle)
        || link
            .destination_root
            .as_deref()
            .map(|value| value.to_ascii_lowercase().contains(needle))
            .unwrap_or(false)
        || link
            .note
            .as_deref()
            .map(|value| value.to_ascii_lowercase().contains(needle))
            .unwrap_or(false)
        || link
            .paths
            .iter()
            .any(|path| path.to_ascii_lowercase().contains(needle))
}

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
    pub(crate) async fn list_migration_links(&self, limit: i64) -> Result<Vec<MigrationLinkView>> {
        match self {
            Self::Memory(memory) => {
                let links = memory.migration_links.read().await;
                Ok(links.iter().rev().take(limit as usize).cloned().collect())
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
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
                    ORDER BY created_at DESC, id DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(migration_link_from_row).collect()
            }
        }
    }

    pub(crate) async fn query_migration_links(
        &self,
        query: &ListQuery,
    ) -> Result<Vec<MigrationLinkView>> {
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
                let mut links = memory
                    .migration_links
                    .read()
                    .await
                    .iter()
                    .filter(|link| {
                        q.as_deref()
                            .map(|needle| migration_link_matches_search(link, needle))
                            .unwrap_or(true)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                links.sort_by(|left, right| {
                    compare_migration_link(left, right, query.sort.as_deref())
                        .then_with(|| left.id.cmp(&right.id))
                });
                if descending {
                    links.reverse();
                }
                Ok(links
                    .into_iter()
                    .skip(offset as usize)
                    .take(limit as usize)
                    .collect())
            }
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
            Self::Memory(memory) => Ok(memory
                .restore_plans
                .read()
                .await
                .iter()
                .find(|plan| plan.id == id)
                .cloned()),
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
                        signed_command_scope,
                        signed_command_id,
                        signed_command_expires_unix,
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
        let view = MigrationLinkView {
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
        };
        match self {
            Self::Memory(memory) => {
                memory.migration_links.write().await.push(view.clone());
                memory.audits.write().await.push(migration_link_audit(
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
                .fetch_one(&mut *tx)
                .await?;
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
                return Ok(persisted);
            }
        }
        Ok(view)
    }
}

fn restore_plan_from_row(row: sqlx::postgres::PgRow) -> Result<RestorePlanView> {
    let signed_command_expires_unix = row
        .try_get::<Option<i64>, _>("signed_command_expires_unix")?
        .map(|value| value.max(0) as u64);
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
        signed_command_scope: row.try_get("signed_command_scope")?,
        signed_command_id: row.try_get("signed_command_id")?,
        signed_command_expires_unix,
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

fn migration_link_audit(
    view: &MigrationLinkView,
    confirmed: bool,
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: "migration.linked_metadata_only".to_string(),
        target: format!("migration_link:{}", view.id),
        command_hash: None,
        metadata: migration_link_metadata_from_view(view, confirmed, operator),
        created_at,
    }
}

fn migration_link_metadata(
    view: &MigrationLinkView,
    restore_plan: &RestorePlanView,
    confirmed: bool,
    operator: &AuthContext,
) -> serde_json::Value {
    let mut metadata = migration_link_metadata_from_view(view, confirmed, operator);
    metadata["restore_plan_payload_hash"] = json!(restore_plan.payload_hash);
    metadata["restore_plan_signed_command_scope"] = json!(restore_plan.signed_command_scope);
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
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "session_id": operator.session_id,
        "metadata_only": true,
    })
}
