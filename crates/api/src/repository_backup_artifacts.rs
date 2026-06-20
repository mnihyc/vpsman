use std::cmp::Ordering;

use anyhow::{ensure, Result};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    model::{
        AuditLogView, AuthContext, BackupArtifactView, BackupRequestStatus, BackupRequestView,
        ListQuery, NewServerArtifact, RecordBackupArtifactMetadataRequest,
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

fn compare_backup_artifact(
    left: &BackupArtifactView,
    right: &BackupArtifactView,
    sort: Option<&str>,
) -> Ordering {
    match sort.unwrap_or("created_at") {
        "client_id" | "client" => left.client_id.cmp(&right.client_id),
        "object_key" | "object" => left.object_key.cmp(&right.object_key),
        "sha256_hex" | "hash" => left.sha256_hex.cmp(&right.sha256_hex),
        "size_bytes" | "size" => left.size_bytes.cmp(&right.size_bytes),
        _ => compare_text_or_number(&left.created_at, &right.created_at),
    }
}

fn backup_artifact_matches_search(artifact: &BackupArtifactView, needle: &str) -> bool {
    artifact
        .id
        .to_string()
        .to_ascii_lowercase()
        .contains(needle)
        || artifact.client_id.to_ascii_lowercase().contains(needle)
        || artifact.object_key.to_ascii_lowercase().contains(needle)
        || artifact.sha256_hex.to_ascii_lowercase().contains(needle)
}

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
            Self::Memory(memory) => {
                let artifacts = memory.backup_artifacts.read().await;
                Ok(artifacts
                    .iter()
                    .rev()
                    .take(limit as usize)
                    .cloned()
                    .collect())
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        artifact.id,
                        artifact.client_id,
                        artifact.object_key,
                        artifact.sha256_hex,
                        artifact.size_bytes,
                        COALESCE(server_artifact.status, 'active') AS status,
                        artifact.created_at::text AS created_at
                    FROM backup_artifacts artifact
                    LEFT JOIN server_artifacts server_artifact
                      ON server_artifact.object_key = artifact.object_key
                     AND server_artifact.status <> 'deleted'
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
        let q = query
            .q
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        match self {
            Self::Memory(memory) => {
                let q = q.map(|value| value.to_ascii_lowercase());
                let mut artifacts = memory
                    .backup_artifacts
                    .read()
                    .await
                    .iter()
                    .filter(|artifact| {
                        q.as_deref()
                            .map(|needle| backup_artifact_matches_search(artifact, needle))
                            .unwrap_or(true)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                artifacts.sort_by(|left, right| {
                    compare_backup_artifact(left, right, query.sort.as_deref())
                        .then_with(|| left.id.cmp(&right.id))
                });
                if descending {
                    artifacts.reverse();
                }
                Ok(artifacts
                    .into_iter()
                    .skip(offset as usize)
                    .take(limit as usize)
                    .collect())
            }
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
                        COALESCE(server_artifact.status, 'active') AS status,
                        artifact.created_at::text AS created_at
                    FROM backup_artifacts artifact
                    LEFT JOIN server_artifacts server_artifact
                      ON server_artifact.object_key = artifact.object_key
                     AND server_artifact.status <> 'deleted'
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
            Self::Memory(memory) => Ok(memory
                .backup_artifacts
                .read()
                .await
                .iter()
                .find(|artifact| artifact.id == artifact_id)
                .cloned()),
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        artifact.id,
                        artifact.client_id,
                        artifact.object_key,
                        artifact.sha256_hex,
                        artifact.size_bytes,
                        COALESCE(server_artifact.status, 'active') AS status,
                        artifact.created_at::text AS created_at
                    FROM backup_artifacts artifact
                    LEFT JOIN server_artifacts server_artifact
                      ON server_artifact.object_key = artifact.object_key
                     AND server_artifact.status <> 'deleted'
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
        operator: &AuthContext,
    ) -> Result<BackupArtifactView> {
        let artifact = BackupArtifactView {
            id: artifact_id,
            client_id: backup_request.client_id.clone(),
            object_key: request.object_key.clone(),
            sha256_hex: request.sha256_hex.clone(),
            size_bytes: request.size_bytes,
            status: "active".to_string(),
            created_at: unix_now().to_string(),
        };
        match self {
            Self::Memory(memory) => {
                {
                    let mut backup_requests = memory.backup_requests.write().await;
                    let stored = backup_requests
                        .iter_mut()
                        .find(|stored| stored.id == backup_request.id)
                        .ok_or_else(|| anyhow::anyhow!("backup_request_not_found"))?;
                    ensure!(
                        stored.artifact_id.is_none(),
                        "backup_artifact_already_recorded"
                    );
                    stored.artifact_id = Some(artifact.id);
                    stored.status = BackupRequestStatus::ArtifactMetadataRecorded
                        .as_str()
                        .to_string();
                }
                memory.backup_artifacts.write().await.push(artifact.clone());
                memory.audits.write().await.push(backup_artifact_audit(
                    backup_request,
                    &artifact,
                    request.confirmed,
                    operator,
                    unix_now().to_string(),
                ));
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
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
                return Ok(persisted);
            }
        }
        Ok(artifact)
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
        created_at: row.try_get("created_at")?,
    })
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

fn backup_artifact_audit(
    backup_request: &BackupRequestView,
    artifact: &BackupArtifactView,
    confirmed: bool,
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: "backup.artifact_metadata_recorded".to_string(),
        target: format!("backup_artifact:{}", artifact.id),
        command_hash: Some(backup_request.payload_hash.clone()),
        metadata: backup_artifact_metadata(backup_request, artifact, confirmed, operator),
        created_at,
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
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "session_id": operator.session_id,
        "metadata_only": true,
        "artifact_upload_verified": false,
        "restore_verified": false,
    })
}
