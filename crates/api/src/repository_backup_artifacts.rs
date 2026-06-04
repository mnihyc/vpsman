use anyhow::{ensure, Result};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    model::{
        AuditLogView, AuthContext, BackupArtifactView, BackupRequestStatus, BackupRequestView,
        RecordBackupArtifactMetadataRequest,
    },
    repository::Repository,
    unix_now,
};

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
                        id,
                        client_id,
                        object_key,
                        sha256_hex,
                        encrypted,
                        size_bytes,
                        created_at::text AS created_at
                    FROM backup_artifacts
                    ORDER BY created_at DESC, id DESC
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
                        id,
                        client_id,
                        object_key,
                        sha256_hex,
                        encrypted,
                        size_bytes,
                        created_at::text AS created_at
                    FROM backup_artifacts
                    WHERE id = $1
                    "#,
                )
                .bind(artifact_id)
                .fetch_optional(pool)
                .await?;
                row.map(backup_artifact_from_row).transpose()
            }
        }
    }

    pub(crate) async fn backup_artifact_object_key_exists(&self, object_key: &str) -> Result<bool> {
        match self {
            Self::Memory(memory) => Ok(memory
                .backup_artifacts
                .read()
                .await
                .iter()
                .any(|artifact| artifact.object_key == object_key)),
            Self::Postgres(pool) => {
                let exists = sqlx::query_scalar::<_, bool>(
                    r#"
                    SELECT EXISTS(
                        SELECT 1
                        FROM backup_artifacts
                        WHERE object_key = $1
                    )
                    "#,
                )
                .bind(object_key)
                .fetch_one(pool)
                .await?;
                Ok(exists)
            }
        }
    }

    pub(crate) async fn record_backup_artifact_metadata(
        &self,
        backup_request: &BackupRequestView,
        request: &RecordBackupArtifactMetadataRequest,
        operator: &AuthContext,
    ) -> Result<BackupArtifactView> {
        let artifact = BackupArtifactView {
            id: Uuid::new_v4(),
            client_id: backup_request.client_id.clone(),
            object_key: request.object_key.clone(),
            sha256_hex: request.sha256_hex.clone(),
            encrypted: request.encrypted,
            size_bytes: request.size_bytes,
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
                        encrypted,
                        size_bytes
                    )
                    VALUES ($1, $2, $3, $4, $5, $6)
                    RETURNING created_at::text AS created_at
                    "#,
                )
                .bind(artifact.id)
                .bind(&artifact.client_id)
                .bind(&artifact.object_key)
                .bind(&artifact.sha256_hex)
                .bind(artifact.encrypted)
                .bind(artifact.size_bytes)
                .fetch_one(&mut *tx)
                .await?;
                let persisted = BackupArtifactView {
                    created_at: row.try_get("created_at")?,
                    ..artifact
                };
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
        encrypted: row.try_get("encrypted")?,
        size_bytes: row.try_get("size_bytes")?,
        created_at: row.try_get("created_at")?,
    })
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
        "encrypted": artifact.encrypted,
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
