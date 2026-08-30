use anyhow::{ensure, Result};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    model::{
        AuthContext, BackupArtifactView, BackupRequestStatus, BackupRequestView, ListQuery,
        NewServerArtifact, RecordBackupArtifactMetadataRequest,
    },
    repository::Repository,
    unix_now,
    util::{limit_or_default, offset_or_default, search_pattern, sort_descending},
};

fn backup_artifact_order_by(sort: Option<&str>, descending: bool) -> &'static str {
    match (sort.unwrap_or("created_at"), descending) {
        ("client_id" | "client", true) => "artifact.client_id DESC, artifact.id DESC",
        ("client_id" | "client", false) => "artifact.client_id ASC, artifact.id ASC",
        ("object_key" | "object", true) => "artifact.object_key DESC, artifact.id DESC",
        ("object_key" | "object", false) => "artifact.object_key ASC, artifact.id ASC",
        ("sha256_hex" | "hash", true) => "artifact.sha256_hex DESC, artifact.id DESC",
        ("sha256_hex" | "hash", false) => "artifact.sha256_hex ASC, artifact.id ASC",
        ("size_bytes" | "size", true) => "artifact.size_bytes DESC, artifact.id DESC",
        ("size_bytes" | "size", false) => "artifact.size_bytes ASC, artifact.id ASC",
        (_, true) => "artifact.created_at DESC, artifact.id DESC",
        (_, false) => "artifact.created_at ASC, artifact.id ASC",
    }
}

impl Repository {
    pub(crate) async fn list_backup_artifacts(
        &self,
        limit: i64,
    ) -> Result<Vec<BackupArtifactView>> {
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        artifact.id,
                        artifact.client_id,
                        artifact.object_key,
                        artifact.sha256_hex,
                        artifact.size_bytes,
                        COALESCE(server_artifact.status, 'missing') AS status,
                        COALESCE(server_artifact.status = 'active', false) AS content_available,
                        artifact.created_at::text AS created_at
                    FROM backup_artifacts artifact
                    LEFT JOIN server_artifacts server_artifact
                      ON server_artifact.object_key = artifact.object_key
                    ORDER BY artifact.created_at DESC, artifact.id DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(backup_artifact_from_row).collect()
            }
        }
    }

    pub(crate) async fn query_backup_artifacts(
        &self,
        query: &ListQuery,
    ) -> Result<Vec<BackupArtifactView>> {
        let limit = limit_or_default(query.limit);
        let offset = offset_or_default(query.offset);
        let descending = sort_descending(query.dir.as_deref(), true);
        match self {
            Self::Postgres(pool) => {
                let order_by = backup_artifact_order_by(query.sort.as_deref(), descending);
                let rows = sqlx::query(&format!(
                    r#"
                    SELECT
                        artifact.id,
                        artifact.client_id,
                        artifact.object_key,
                        artifact.sha256_hex,
                        artifact.size_bytes,
                        COALESCE(server_artifact.status, 'missing') AS status,
                        COALESCE(server_artifact.status = 'active', false) AS content_available,
                        artifact.created_at::text AS created_at
                    FROM backup_artifacts artifact
                    LEFT JOIN server_artifacts server_artifact
                      ON server_artifact.object_key = artifact.object_key
                    WHERE (
                        $3::text IS NULL
                        OR artifact.id::text ILIKE $3 ESCAPE '\'
                        OR artifact.client_id ILIKE $3 ESCAPE '\'
                        OR artifact.object_key ILIKE $3 ESCAPE '\'
                        OR artifact.sha256_hex ILIKE $3 ESCAPE '\'
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
                rows.into_iter().map(backup_artifact_from_row).collect()
            }
        }
    }

    pub(crate) async fn find_backup_artifact(
        &self,
        artifact_id: Uuid,
    ) -> Result<Option<BackupArtifactView>> {
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        artifact.id,
                        artifact.client_id,
                        artifact.object_key,
                        artifact.sha256_hex,
                        artifact.size_bytes,
                        COALESCE(server_artifact.status, 'missing') AS status,
                        COALESCE(server_artifact.status = 'active', false) AS content_available,
                        artifact.created_at::text AS created_at
                    FROM backup_artifacts artifact
                    LEFT JOIN server_artifacts server_artifact
                      ON server_artifact.object_key = artifact.object_key
                    WHERE artifact.id = $1
                    "#,
                )
                .bind(artifact_id)
                .fetch_optional(pool)
                .await?;
                row.map(backup_artifact_from_row).transpose()
            }
        }
    }

    pub(crate) async fn record_backup_artifact_metadata(
        &self,
        backup_request: &BackupRequestView,
        artifact_id: Uuid,
        request: &RecordBackupArtifactMetadataRequest,
        reservation_token: Option<Uuid>,
        operator: &AuthContext,
    ) -> Result<BackupArtifactView> {
        let artifact = BackupArtifactView {
            id: artifact_id,
            client_id: backup_request.client_id.clone(),
            object_key: request.object_key.clone(),
            sha256_hex: request.sha256_hex.clone(),
            size_bytes: request.size_bytes,
            status: "active".to_string(),
            content_available: true,
            created_at: unix_now().to_string(),
        };
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let linked_artifact_id: Option<Option<Uuid>> = sqlx::query_scalar(
                    r#"
                    SELECT artifact_id
                    FROM backup_requests
                    WHERE id = $1
                    FOR UPDATE
                    "#,
                )
                .bind(backup_request.id)
                .fetch_optional(&mut *tx)
                .await?;
                let linked_artifact_id = linked_artifact_id
                    .ok_or_else(|| anyhow::anyhow!("backup_request_not_found"))?;
                if let Some(linked_artifact_id) = linked_artifact_id {
                    let row = sqlx::query(
                        r#"
                        SELECT
                            id,
                            client_id,
                            object_key,
                            sha256_hex,
                            size_bytes,
                            'active'::text AS status,
                            TRUE AS content_available,
                            created_at::text AS created_at
                        FROM backup_artifacts
                        WHERE id = $1
                        "#,
                    )
                    .bind(linked_artifact_id)
                    .fetch_optional(&mut *tx)
                    .await?;
                    let existing = row
                        .map(backup_artifact_from_row)
                        .transpose()?
                        .ok_or_else(|| anyhow::anyhow!("backup_artifact_already_recorded"))?;
                    ensure!(
                        backup_artifact_matches(&existing, &artifact),
                        "backup_artifact_already_recorded"
                    );
                    Repository::upsert_server_artifact_in_tx(
                        &mut tx,
                        &backup_server_artifact(backup_request, &existing),
                        "active",
                        None,
                    )
                    .await?;
                    tx.commit().await?;
                    return Ok(existing);
                }
                let row = sqlx::query(
                    r#"
                    INSERT INTO backup_artifacts (
                        id,
                        client_id,
                        object_key,
                        sha256_hex,
                        size_bytes
                    )
                    VALUES ($1, $2, $3, $4, $5)
                    RETURNING created_at::text AS created_at
                    "#,
                )
                .bind(artifact.id)
                .bind(&artifact.client_id)
                .bind(&artifact.object_key)
                .bind(&artifact.sha256_hex)
                .bind(artifact.size_bytes)
                .fetch_one(&mut *tx)
                .await?;
                let persisted = BackupArtifactView {
                    created_at: row.try_get("created_at")?,
                    ..artifact
                };
                Repository::upsert_server_artifact_in_tx(
                    &mut tx,
                    &backup_server_artifact(backup_request, &persisted),
                    "active",
                    reservation_token,
                )
                .await?;
                let update = sqlx::query(
                    r#"
                    UPDATE backup_requests
                    SET artifact_id = $1, status = $2
                    WHERE id = $3 AND artifact_id IS NULL
                    "#,
                )
                .bind(persisted.id)
                .bind(BackupRequestStatus::ArtifactMetadataRecorded.as_str())
                .bind(backup_request.id)
                .execute(&mut *tx)
                .await?;
                ensure!(
                    update.rows_affected() == 1,
                    "backup_artifact_already_recorded"
                );
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
                .bind("backup.artifact_metadata_recorded")
                .bind(format!("backup_artifact:{}", persisted.id))
                .bind(&backup_request.payload_hash)
                .bind(backup_artifact_metadata(
                    backup_request,
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
}

pub(crate) fn backup_artifact_from_row(row: sqlx::postgres::PgRow) -> Result<BackupArtifactView> {
    Ok(BackupArtifactView {
        id: row.try_get("id")?,
        client_id: row.try_get("client_id")?,
        object_key: row.try_get("object_key")?,
        sha256_hex: row.try_get("sha256_hex")?,
        size_bytes: row.try_get("size_bytes")?,
        status: row.try_get("status")?,
        content_available: row.try_get("content_available")?,
        created_at: row.try_get("created_at")?,
    })
}

fn backup_artifact_matches(existing: &BackupArtifactView, expected: &BackupArtifactView) -> bool {
    existing.id == expected.id
        && existing.client_id == expected.client_id
        && existing.object_key == expected.object_key
        && existing.sha256_hex == expected.sha256_hex
        && existing.size_bytes == expected.size_bytes
}

pub(crate) fn backup_server_artifact(
    backup_request: &BackupRequestView,
    artifact: &BackupArtifactView,
) -> NewServerArtifact {
    NewServerArtifact {
        domain: "backup_artifact".to_string(),
        object_key: artifact.object_key.clone(),
        sha256_hex: artifact.sha256_hex.clone(),
        size_bytes: artifact.size_bytes,
        job_id: backup_request.source_job_id,
        client_id: Some(artifact.client_id.clone()),
        stream: None,
        seq: None,
        backup_request_id: Some(backup_request.id),
        backup_artifact_id: Some(artifact.id),
        release_id: None,
        metadata: json!({
            "backup_request_id": backup_request.id,
            "backup_artifact_id": artifact.id,
        }),
    }
}

fn backup_artifact_metadata(
    backup_request: &BackupRequestView,
    artifact: &BackupArtifactView,
    confirmed: bool,
    operator: &AuthContext,
) -> serde_json::Value {
    json!({
        "backup_request_id": backup_request.id,
        "client_id": &artifact.client_id,
        "object_key": &artifact.object_key,
        "sha256_hex": &artifact.sha256_hex,
        "size_bytes": artifact.size_bytes,
        "confirmed": confirmed,
        "result": "succeeded",
        "operator_id": operator.operator.id,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "operator_session_id": operator.audit_session_id(),
        "origin_kind": "operator_request",
        "component": "backup-artifact-controller",
        "metadata_only": !artifact.content_available,
        "artifact_upload_verified": artifact.content_available,
        "restore_verified": false,
    })
}
