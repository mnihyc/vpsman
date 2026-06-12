use anyhow::Result;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    auth_model::AuthContext, model::NewServerArtifact,
    model_file_transfer::FileTransferSourceArtifactView, repository::Repository, unix_now,
};

impl Repository {
    pub(crate) async fn list_file_transfer_source_artifacts(
        &self,
        limit: i64,
    ) -> Result<Vec<FileTransferSourceArtifactView>> {
        let limit = limit.clamp(1, 200);
        match self {
            Self::Memory(memory) => {
                let artifacts = memory.file_transfer_source_artifacts.read().await;
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
                        name,
                        object_key,
                        sha256_hex,
                        size_bytes,
                        created_by,
                        created_at::text AS created_at
                    FROM file_transfer_source_artifacts
                    ORDER BY created_at DESC, id DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(file_transfer_source_artifact_from_row)
                    .collect()
            }
        }
    }

    pub(crate) async fn get_file_transfer_source_artifact(
        &self,
        artifact_id: Uuid,
    ) -> Result<Option<FileTransferSourceArtifactView>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .file_transfer_source_artifacts
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
                        name,
                        object_key,
                        sha256_hex,
                        size_bytes,
                        created_by,
                        created_at::text AS created_at
                    FROM file_transfer_source_artifacts
                    WHERE id = $1
                    "#,
                )
                .bind(artifact_id)
                .fetch_optional(pool)
                .await?;
                row.map(file_transfer_source_artifact_from_row).transpose()
            }
        }
    }

    pub(crate) async fn record_file_transfer_source_artifact(
        &self,
        name: String,
        object_key: String,
        sha256_hex: String,
        size_bytes: i64,
        operator: &AuthContext,
    ) -> Result<FileTransferSourceArtifactView> {
        let artifact_id = Uuid::new_v4();
        let artifact = match self {
            Self::Memory(memory) => {
                let artifact = FileTransferSourceArtifactView {
                    id: artifact_id,
                    name,
                    object_key,
                    sha256_hex,
                    size_bytes,
                    created_by: Some(operator.operator.id),
                    created_at: unix_now().to_string(),
                    download_path: file_transfer_source_artifact_download_path(artifact_id),
                };
                memory
                    .file_transfer_source_artifacts
                    .write()
                    .await
                    .push(artifact.clone());
                artifact
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    INSERT INTO file_transfer_source_artifacts (
                        id, name, object_key, sha256_hex, size_bytes, created_by
                    )
                    VALUES ($1, $2, $3, $4, $5, $6)
                    RETURNING
                        id,
                        name,
                        object_key,
                        sha256_hex,
                        size_bytes,
                        created_by,
                        created_at::text AS created_at
                    "#,
                )
                .bind(artifact_id)
                .bind(name)
                .bind(object_key)
                .bind(sha256_hex)
                .bind(size_bytes)
                .bind(operator.operator.id)
                .fetch_one(pool)
                .await?;
                file_transfer_source_artifact_from_row(row)?
            }
        };
        self.register_server_artifact(NewServerArtifact {
            domain: "file_transfer_source".to_string(),
            object_key: artifact.object_key.clone(),
            sha256_hex: artifact.sha256_hex.clone(),
            size_bytes: artifact.size_bytes,
            job_id: None,
            client_id: None,
            stream: None,
            seq: None,
            backup_request_id: None,
            backup_artifact_id: None,
            release_id: None,
            metadata: serde_json::json!({
                "source_artifact_id": artifact.id,
                "name": &artifact.name,
                "created_by": artifact.created_by,
            }),
        })
        .await?;
        Ok(artifact)
    }
}

fn file_transfer_source_artifact_from_row(
    row: sqlx::postgres::PgRow,
) -> Result<FileTransferSourceArtifactView> {
    let id: Uuid = row.try_get("id")?;
    Ok(FileTransferSourceArtifactView {
        id,
        name: row.try_get("name")?,
        object_key: row.try_get("object_key")?,
        sha256_hex: row.try_get("sha256_hex")?,
        size_bytes: row.try_get("size_bytes")?,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
        download_path: file_transfer_source_artifact_download_path(id),
    })
}

pub(crate) fn file_transfer_source_artifact_object_key(sha256_hex: &str) -> String {
    format!("file-transfer-sources/{sha256_hex}.bin")
}

pub(crate) fn file_transfer_source_artifact_download_path(artifact_id: Uuid) -> String {
    format!("/api/v1/file-transfer-sources/{artifact_id}/artifact")
}
