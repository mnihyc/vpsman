use anyhow::{ensure, Result};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;
use vpsman_common::payload_hash;

use crate::{
    model::{
        AgentUpdateReleaseView, AuditLogView, AuthContext, CreateAgentUpdateReleaseRequest,
        UploadAgentUpdateArtifactRequest,
    },
    repository::Repository,
    unix_now,
};

const RELEASE_STATUS_METADATA_ONLY: &str = "published_metadata_only";
const RELEASE_STATUS_ARTIFACT_HOSTED: &str = "artifact_hosted";

#[derive(Clone, Debug)]
pub(crate) struct HostedAgentUpdateArtifactRef {
    pub(crate) artifact_sha256_hex: String,
    pub(crate) artifact_object_key: String,
    pub(crate) size_bytes: Option<i64>,
}

#[derive(Clone, Debug)]
pub(crate) struct UploadedAgentUpdateArtifactRef {
    pub(crate) artifact_sha256_hex: String,
    pub(crate) artifact_object_key: String,
    pub(crate) artifact_download_path: String,
    pub(crate) size_bytes: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct UploadedAgentUpdateReleaseArtifacts {
    pub(crate) primary: UploadedAgentUpdateArtifactRef,
    pub(crate) rollback: Option<UploadedAgentUpdateArtifactRef>,
}

#[derive(Clone, Debug)]
pub(crate) struct UploadedAgentUpdateReleaseMetadata {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) channel: String,
    pub(crate) artifact_signature_hex: String,
    pub(crate) artifact_signing_key_hex: String,
    pub(crate) rollback_artifact_signature_hex: Option<String>,
    pub(crate) rollback_artifact_signing_key_hex: Option<String>,
    pub(crate) notes: Option<String>,
    pub(crate) confirmed: bool,
    pub(crate) ingestion_mode: &'static str,
}

impl UploadedAgentUpdateReleaseMetadata {
    pub(crate) fn from_base64_request(request: &UploadAgentUpdateArtifactRequest) -> Self {
        Self {
            name: request.name.clone(),
            version: request.version.clone(),
            channel: request.channel.clone(),
            artifact_signature_hex: request.artifact_signature_hex.clone(),
            artifact_signing_key_hex: request.artifact_signing_key_hex.clone(),
            rollback_artifact_signature_hex: request.rollback_artifact_signature_hex.clone(),
            rollback_artifact_signing_key_hex: request.rollback_artifact_signing_key_hex.clone(),
            notes: request.notes.clone(),
            confirmed: request.confirmed,
            ingestion_mode: "json_base64",
        }
    }
}

impl Repository {
    pub(crate) async fn list_agent_update_releases(
        &self,
        limit: i64,
    ) -> Result<Vec<AgentUpdateReleaseView>> {
        match self {
            Self::Memory(memory) => {
                let releases = memory.agent_update_releases.read().await;
                Ok(releases
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
                        actor_id,
                        name,
                        version,
                        channel,
                        status,
                        artifact_sha256_hex,
                        artifact_signature_provided,
                        artifact_signature_sha256_hex,
                        artifact_signing_key_sha256_hex,
                        artifact_url_sha256_hex,
                        artifact_object_key,
                        artifact_download_path,
                        rollback_artifact_sha256_hex,
                        rollback_artifact_signature_provided,
                        rollback_artifact_signature_sha256_hex,
                        rollback_artifact_signing_key_sha256_hex,
                        rollback_artifact_url_sha256_hex,
                        rollback_artifact_object_key,
                        rollback_artifact_download_path,
                        rollback_size_bytes,
                        size_bytes,
                        notes,
                        created_at::text AS created_at
                    FROM agent_update_releases
                    ORDER BY created_at DESC, id DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(agent_update_release_from_row)
                    .collect()
            }
        }
    }

    pub(crate) async fn record_agent_update_release(
        &self,
        request: &CreateAgentUpdateReleaseRequest,
        operator: &AuthContext,
    ) -> Result<AgentUpdateReleaseView> {
        let release = agent_update_release_view(request, operator);
        match self {
            Self::Memory(memory) => {
                {
                    let releases = memory.agent_update_releases.read().await;
                    ensure!(
                        !releases.iter().any(|stored| {
                            stored.name == release.name
                                && stored.version == release.version
                                && stored.channel == release.channel
                        }),
                        "agent_update_release_already_exists"
                    );
                }
                memory
                    .agent_update_releases
                    .write()
                    .await
                    .push(release.clone());
                memory.audits.write().await.push(release_audit(
                    &release,
                    request,
                    operator,
                    release.created_at.clone(),
                ));
                Ok(release)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    INSERT INTO agent_update_releases (
                        id,
                        actor_id,
                        name,
                        version,
                        channel,
                        status,
                        artifact_sha256_hex,
                        artifact_signature_provided,
                        artifact_signature_sha256_hex,
                        artifact_signing_key_sha256_hex,
                        artifact_url_sha256_hex,
                        artifact_object_key,
                        artifact_download_path,
                        rollback_artifact_sha256_hex,
                        rollback_artifact_signature_provided,
                        rollback_artifact_signature_sha256_hex,
                        rollback_artifact_signing_key_sha256_hex,
                        rollback_artifact_url_sha256_hex,
                        rollback_artifact_object_key,
                        rollback_artifact_download_path,
                        rollback_size_bytes,
                        size_bytes,
                        notes
                    )
                    VALUES (
                        $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                        $11, $12, $13, $14, $15, $16, $17, $18,
                        $19, $20, $21, $22, $23
                    )
                    RETURNING created_at::text AS created_at
                    "#,
                )
                .bind(release.id)
                .bind(release.actor_id)
                .bind(&release.name)
                .bind(&release.version)
                .bind(&release.channel)
                .bind(&release.status)
                .bind(&release.artifact_sha256_hex)
                .bind(release.artifact_signature_provided)
                .bind(&release.artifact_signature_sha256_hex)
                .bind(&release.artifact_signing_key_sha256_hex)
                .bind(&release.artifact_url_sha256_hex)
                .bind(&release.artifact_object_key)
                .bind(&release.artifact_download_path)
                .bind(&release.rollback_artifact_sha256_hex)
                .bind(release.rollback_artifact_signature_provided)
                .bind(&release.rollback_artifact_signature_sha256_hex)
                .bind(&release.rollback_artifact_signing_key_sha256_hex)
                .bind(&release.rollback_artifact_url_sha256_hex)
                .bind(&release.rollback_artifact_object_key)
                .bind(&release.rollback_artifact_download_path)
                .bind(release.rollback_size_bytes)
                .bind(release.size_bytes)
                .bind(&release.notes)
                .fetch_one(&mut *tx)
                .await?;
                let persisted = AgentUpdateReleaseView {
                    created_at: row.try_get("created_at")?,
                    ..release
                };
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("agent_update.release_recorded")
                .bind(format!("agent_update_release:{}", persisted.id))
                .bind(release_metadata(&persisted, request, operator))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(persisted)
            }
        }
    }

    pub(crate) async fn record_uploaded_agent_update_release(
        &self,
        metadata: &UploadedAgentUpdateReleaseMetadata,
        artifacts: &UploadedAgentUpdateReleaseArtifacts,
        operator: &AuthContext,
    ) -> Result<AgentUpdateReleaseView> {
        let release = uploaded_agent_update_release_view(metadata, artifacts, operator);
        match self {
            Self::Memory(memory) => {
                {
                    let releases = memory.agent_update_releases.read().await;
                    ensure!(
                        !releases.iter().any(|stored| {
                            stored.name == release.name
                                && stored.version == release.version
                                && stored.channel == release.channel
                        }),
                        "agent_update_release_already_exists"
                    );
                }
                memory
                    .agent_update_releases
                    .write()
                    .await
                    .push(release.clone());
                memory.audits.write().await.push(upload_release_audit(
                    &release,
                    metadata,
                    operator,
                    release.created_at.clone(),
                ));
                Ok(release)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    INSERT INTO agent_update_releases (
                        id,
                        actor_id,
                        name,
                        version,
                        channel,
                        status,
                        artifact_sha256_hex,
                        artifact_signature_provided,
                        artifact_signature_sha256_hex,
                        artifact_signing_key_sha256_hex,
                        artifact_url_sha256_hex,
                        artifact_object_key,
                        artifact_download_path,
                        rollback_artifact_sha256_hex,
                        rollback_artifact_signature_provided,
                        rollback_artifact_signature_sha256_hex,
                        rollback_artifact_signing_key_sha256_hex,
                        rollback_artifact_url_sha256_hex,
                        rollback_artifact_object_key,
                        rollback_artifact_download_path,
                        rollback_size_bytes,
                        size_bytes,
                        notes
                    )
                    VALUES (
                        $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                        $11, $12, $13, $14, $15, $16, $17, $18,
                        $19, $20, $21, $22, $23
                    )
                    RETURNING created_at::text AS created_at
                    "#,
                )
                .bind(release.id)
                .bind(release.actor_id)
                .bind(&release.name)
                .bind(&release.version)
                .bind(&release.channel)
                .bind(&release.status)
                .bind(&release.artifact_sha256_hex)
                .bind(release.artifact_signature_provided)
                .bind(&release.artifact_signature_sha256_hex)
                .bind(&release.artifact_signing_key_sha256_hex)
                .bind(&release.artifact_url_sha256_hex)
                .bind(&release.artifact_object_key)
                .bind(&release.artifact_download_path)
                .bind(&release.rollback_artifact_sha256_hex)
                .bind(release.rollback_artifact_signature_provided)
                .bind(&release.rollback_artifact_signature_sha256_hex)
                .bind(&release.rollback_artifact_signing_key_sha256_hex)
                .bind(&release.rollback_artifact_url_sha256_hex)
                .bind(&release.rollback_artifact_object_key)
                .bind(&release.rollback_artifact_download_path)
                .bind(release.rollback_size_bytes)
                .bind(release.size_bytes)
                .bind(&release.notes)
                .fetch_one(&mut *tx)
                .await?;
                let persisted = AgentUpdateReleaseView {
                    created_at: row.try_get("created_at")?,
                    ..release
                };
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("agent_update.artifact_uploaded")
                .bind(format!("agent_update_release:{}", persisted.id))
                .bind(upload_release_metadata(&persisted, metadata, operator))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(persisted)
            }
        }
    }

    pub(crate) async fn agent_update_release_exists_for_artifact(
        &self,
        artifact_sha256_hex: &str,
        artifact_signing_key_hex: Option<&str>,
    ) -> Result<bool> {
        let Some(signing_key_hex) = artifact_signing_key_hex else {
            return Ok(false);
        };
        let artifact_sha256_hex = artifact_sha256_hex.to_ascii_lowercase();
        let signing_key_sha256_hex = payload_hash(signing_key_hex.to_ascii_lowercase().as_bytes());
        match self {
            Self::Memory(memory) => {
                Ok(memory
                    .agent_update_releases
                    .read()
                    .await
                    .iter()
                    .any(|release| {
                        matches!(
                            release.status.as_str(),
                            RELEASE_STATUS_METADATA_ONLY | RELEASE_STATUS_ARTIFACT_HOSTED
                        ) && release.artifact_sha256_hex == artifact_sha256_hex
                            && release.artifact_signing_key_sha256_hex == signing_key_sha256_hex
                    }))
            }
            Self::Postgres(pool) => {
                let exists: bool = sqlx::query_scalar(
                    r#"
                    SELECT EXISTS (
                        SELECT 1
                        FROM agent_update_releases
                        WHERE status IN ($1, $2)
                          AND artifact_sha256_hex = $3
                          AND artifact_signing_key_sha256_hex = $4
                    )
                    "#,
                )
                .bind(RELEASE_STATUS_METADATA_ONLY)
                .bind(RELEASE_STATUS_ARTIFACT_HOSTED)
                .bind(artifact_sha256_hex)
                .bind(signing_key_sha256_hex)
                .fetch_one(pool)
                .await?;
                Ok(exists)
            }
        }
    }

    pub(crate) async fn find_agent_update_release_for_artifact(
        &self,
        artifact_sha256_hex: &str,
        artifact_signing_key_hex: Option<&str>,
    ) -> Result<Option<AgentUpdateReleaseView>> {
        let Some(signing_key_hex) = artifact_signing_key_hex else {
            return Ok(None);
        };
        let artifact_sha256_hex = artifact_sha256_hex.to_ascii_lowercase();
        let signing_key_sha256_hex = payload_hash(signing_key_hex.to_ascii_lowercase().as_bytes());
        match self {
            Self::Memory(memory) => Ok(memory
                .agent_update_releases
                .read()
                .await
                .iter()
                .rev()
                .find(|release| {
                    matches!(
                        release.status.as_str(),
                        RELEASE_STATUS_METADATA_ONLY | RELEASE_STATUS_ARTIFACT_HOSTED
                    ) && release.artifact_sha256_hex == artifact_sha256_hex
                        && release.artifact_signing_key_sha256_hex == signing_key_sha256_hex
                })
                .cloned()),
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        name,
                        version,
                        channel,
                        status,
                        artifact_sha256_hex,
                        artifact_signature_provided,
                        artifact_signature_sha256_hex,
                        artifact_signing_key_sha256_hex,
                        artifact_url_sha256_hex,
                        artifact_object_key,
                        artifact_download_path,
                        rollback_artifact_sha256_hex,
                        rollback_artifact_signature_provided,
                        rollback_artifact_signature_sha256_hex,
                        rollback_artifact_signing_key_sha256_hex,
                        rollback_artifact_url_sha256_hex,
                        rollback_artifact_object_key,
                        rollback_artifact_download_path,
                        rollback_size_bytes,
                        size_bytes,
                        notes,
                        created_at::text AS created_at
                    FROM agent_update_releases
                    WHERE status IN ($1, $2)
                      AND artifact_sha256_hex = $3
                      AND artifact_signing_key_sha256_hex = $4
                    ORDER BY created_at DESC, id DESC
                    LIMIT 1
                    "#,
                )
                .bind(RELEASE_STATUS_METADATA_ONLY)
                .bind(RELEASE_STATUS_ARTIFACT_HOSTED)
                .bind(artifact_sha256_hex)
                .bind(signing_key_sha256_hex)
                .fetch_optional(pool)
                .await?;
                row.map(agent_update_release_from_row).transpose()
            }
        }
    }

    pub(crate) async fn get_hosted_agent_update_artifact_ref(
        &self,
        artifact_sha256_hex: &str,
    ) -> Result<Option<HostedAgentUpdateArtifactRef>> {
        let artifact_sha256_hex = artifact_sha256_hex.trim().to_ascii_lowercase();
        match self {
            Self::Memory(memory) => Ok(memory
                .agent_update_releases
                .read()
                .await
                .iter()
                .rev()
                .find(|release| {
                    release.status == RELEASE_STATUS_ARTIFACT_HOSTED
                        && ((release.artifact_sha256_hex == artifact_sha256_hex
                            && release.artifact_object_key.is_some())
                            || (release
                                .rollback_artifact_sha256_hex
                                .as_deref()
                                .is_some_and(|sha| sha == artifact_sha256_hex)
                                && release.rollback_artifact_object_key.is_some()))
                })
                .and_then(|release| {
                    if release.artifact_sha256_hex == artifact_sha256_hex {
                        return Some(HostedAgentUpdateArtifactRef {
                            artifact_sha256_hex: release.artifact_sha256_hex.clone(),
                            artifact_object_key: release.artifact_object_key.clone()?,
                            size_bytes: release.size_bytes,
                        });
                    }
                    Some(HostedAgentUpdateArtifactRef {
                        artifact_sha256_hex: release.rollback_artifact_sha256_hex.clone()?,
                        artifact_object_key: release.rollback_artifact_object_key.clone()?,
                        size_bytes: release.rollback_size_bytes,
                    })
                })),
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        CASE
                            WHEN artifact_sha256_hex = $2 THEN artifact_sha256_hex
                            ELSE rollback_artifact_sha256_hex
                        END AS artifact_sha256_hex,
                        CASE
                            WHEN artifact_sha256_hex = $2 THEN artifact_object_key
                            ELSE rollback_artifact_object_key
                        END AS artifact_object_key,
                        CASE
                            WHEN artifact_sha256_hex = $2 THEN size_bytes
                            ELSE rollback_size_bytes
                        END AS size_bytes
                    FROM agent_update_releases
                    WHERE status = $1
                      AND (
                        (artifact_sha256_hex = $2 AND artifact_object_key IS NOT NULL)
                        OR (
                            rollback_artifact_sha256_hex = $2
                            AND rollback_artifact_object_key IS NOT NULL
                        )
                      )
                    ORDER BY created_at DESC, id DESC
                    LIMIT 1
                    "#,
                )
                .bind(RELEASE_STATUS_ARTIFACT_HOSTED)
                .bind(artifact_sha256_hex)
                .fetch_optional(pool)
                .await?;
                row.map(|row| {
                    Ok(HostedAgentUpdateArtifactRef {
                        artifact_sha256_hex: row.try_get("artifact_sha256_hex")?,
                        artifact_object_key: row.try_get("artifact_object_key")?,
                        size_bytes: row.try_get("size_bytes")?,
                    })
                })
                .transpose()
            }
        }
    }

    pub(crate) async fn record_streamed_agent_update_artifact_audit(
        &self,
        artifact: &UploadedAgentUpdateArtifactRef,
        artifact_signature_sha256_hex: &str,
        artifact_signing_key_sha256_hex: &str,
        operator: &AuthContext,
    ) -> Result<()> {
        let audit = AuditLogView {
            id: Uuid::new_v4(),
            actor_id: Some(operator.operator.id),
            action: "agent_update.artifact_streamed".to_string(),
            target: format!("agent_update_artifact:{}", artifact.artifact_sha256_hex),
            command_hash: None,
            metadata: json!({
                "artifact_sha256_hex": &artifact.artifact_sha256_hex,
                "artifact_signature_provided": true,
                "artifact_signature_sha256_hex": artifact_signature_sha256_hex,
                "artifact_signing_key_sha256_hex": artifact_signing_key_sha256_hex,
                "artifact_object_key": &artifact.artifact_object_key,
                "artifact_download_path": &artifact.artifact_download_path,
                "size_bytes": artifact.size_bytes,
                "artifact_ingestion_mode": "raw_body_stream",
                "artifact_bytes_stored_in_audit": false,
                "artifact_signature_stored": false,
                "artifact_signing_key_stored": false,
                "operator_username": &operator.operator.username,
                "operator_role": &operator.operator.role,
                "session_id": operator.session_id,
            }),
            created_at: unix_now().to_string(),
        };
        match self {
            Self::Memory(memory) => {
                memory.audits.write().await.push(audit);
                Ok(())
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(audit.id)
                .bind(audit.actor_id)
                .bind(&audit.action)
                .bind(&audit.target)
                .bind(&audit.metadata)
                .execute(pool)
                .await?;
                Ok(())
            }
        }
    }
}

fn agent_update_release_from_row(row: sqlx::postgres::PgRow) -> Result<AgentUpdateReleaseView> {
    Ok(AgentUpdateReleaseView {
        id: row.try_get("id")?,
        actor_id: row.try_get("actor_id")?,
        name: row.try_get("name")?,
        version: row.try_get("version")?,
        channel: row.try_get("channel")?,
        status: row.try_get("status")?,
        artifact_sha256_hex: row.try_get("artifact_sha256_hex")?,
        artifact_signature_provided: row.try_get("artifact_signature_provided")?,
        artifact_signature_sha256_hex: row.try_get("artifact_signature_sha256_hex")?,
        artifact_signing_key_sha256_hex: row.try_get("artifact_signing_key_sha256_hex")?,
        artifact_url_sha256_hex: row.try_get("artifact_url_sha256_hex")?,
        artifact_object_key: row.try_get("artifact_object_key")?,
        artifact_download_path: row.try_get("artifact_download_path")?,
        artifact_download_url: None,
        rollback_artifact_sha256_hex: row.try_get("rollback_artifact_sha256_hex")?,
        rollback_artifact_signature_provided: row
            .try_get("rollback_artifact_signature_provided")?,
        rollback_artifact_signature_sha256_hex: row
            .try_get("rollback_artifact_signature_sha256_hex")?,
        rollback_artifact_signing_key_sha256_hex: row
            .try_get("rollback_artifact_signing_key_sha256_hex")?,
        rollback_artifact_url_sha256_hex: row.try_get("rollback_artifact_url_sha256_hex")?,
        rollback_artifact_object_key: row.try_get("rollback_artifact_object_key")?,
        rollback_artifact_download_path: row.try_get("rollback_artifact_download_path")?,
        rollback_artifact_download_url: None,
        rollback_size_bytes: row.try_get("rollback_size_bytes")?,
        size_bytes: row.try_get("size_bytes")?,
        notes: row.try_get("notes")?,
        created_at: row.try_get("created_at")?,
    })
}

fn agent_update_release_view(
    request: &CreateAgentUpdateReleaseRequest,
    operator: &AuthContext,
) -> AgentUpdateReleaseView {
    AgentUpdateReleaseView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        name: request.name.trim().to_string(),
        version: request.version.trim().to_string(),
        channel: request.channel.trim().to_ascii_lowercase(),
        status: RELEASE_STATUS_METADATA_ONLY.to_string(),
        artifact_sha256_hex: request.artifact_sha256_hex.trim().to_ascii_lowercase(),
        artifact_signature_provided: true,
        artifact_signature_sha256_hex: Some(payload_hash(
            request
                .artifact_signature_hex
                .trim()
                .to_ascii_lowercase()
                .as_bytes(),
        )),
        artifact_signing_key_sha256_hex: payload_hash(
            request
                .artifact_signing_key_hex
                .trim()
                .to_ascii_lowercase()
                .as_bytes(),
        ),
        artifact_url_sha256_hex: request
            .artifact_url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(|url| payload_hash(url.as_bytes())),
        artifact_object_key: None,
        artifact_download_path: None,
        artifact_download_url: None,
        rollback_artifact_sha256_hex: request
            .rollback_artifact_sha256_hex
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase),
        rollback_artifact_signature_provided: request.rollback_artifact_signature_hex.is_some(),
        rollback_artifact_signature_sha256_hex: request
            .rollback_artifact_signature_hex
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| payload_hash(value.to_ascii_lowercase().as_bytes())),
        rollback_artifact_signing_key_sha256_hex: request
            .rollback_artifact_signing_key_hex
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| payload_hash(value.to_ascii_lowercase().as_bytes())),
        rollback_artifact_url_sha256_hex: request
            .rollback_artifact_url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(|url| payload_hash(url.as_bytes())),
        rollback_artifact_object_key: None,
        rollback_artifact_download_path: None,
        rollback_artifact_download_url: None,
        rollback_size_bytes: request.rollback_size_bytes,
        size_bytes: request.size_bytes,
        notes: request.notes.as_deref().map(str::trim).and_then(|notes| {
            if notes.is_empty() {
                None
            } else {
                Some(notes.to_string())
            }
        }),
        created_at: unix_now().to_string(),
    }
}

fn uploaded_agent_update_release_view(
    metadata: &UploadedAgentUpdateReleaseMetadata,
    artifacts: &UploadedAgentUpdateReleaseArtifacts,
    operator: &AuthContext,
) -> AgentUpdateReleaseView {
    AgentUpdateReleaseView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        name: metadata.name.trim().to_string(),
        version: metadata.version.trim().to_string(),
        channel: metadata.channel.trim().to_ascii_lowercase(),
        status: RELEASE_STATUS_ARTIFACT_HOSTED.to_string(),
        artifact_sha256_hex: artifacts.primary.artifact_sha256_hex.to_ascii_lowercase(),
        artifact_signature_provided: true,
        artifact_signature_sha256_hex: Some(payload_hash(
            metadata
                .artifact_signature_hex
                .trim()
                .to_ascii_lowercase()
                .as_bytes(),
        )),
        artifact_signing_key_sha256_hex: payload_hash(
            metadata
                .artifact_signing_key_hex
                .trim()
                .to_ascii_lowercase()
                .as_bytes(),
        ),
        artifact_url_sha256_hex: None,
        artifact_object_key: Some(artifacts.primary.artifact_object_key.clone()),
        artifact_download_path: Some(artifacts.primary.artifact_download_path.clone()),
        artifact_download_url: None,
        rollback_artifact_sha256_hex: artifacts
            .rollback
            .as_ref()
            .map(|artifact| artifact.artifact_sha256_hex.clone()),
        rollback_artifact_signature_provided: metadata.rollback_artifact_signature_hex.is_some(),
        rollback_artifact_signature_sha256_hex: metadata
            .rollback_artifact_signature_hex
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| payload_hash(value.to_ascii_lowercase().as_bytes())),
        rollback_artifact_signing_key_sha256_hex: metadata
            .rollback_artifact_signing_key_hex
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| payload_hash(value.to_ascii_lowercase().as_bytes())),
        rollback_artifact_url_sha256_hex: None,
        rollback_artifact_object_key: artifacts
            .rollback
            .as_ref()
            .map(|artifact| artifact.artifact_object_key.clone()),
        rollback_artifact_download_path: artifacts
            .rollback
            .as_ref()
            .map(|artifact| artifact.artifact_download_path.clone()),
        rollback_artifact_download_url: None,
        rollback_size_bytes: artifacts
            .rollback
            .as_ref()
            .map(|artifact| artifact.size_bytes),
        size_bytes: Some(artifacts.primary.size_bytes),
        notes: metadata.notes.as_deref().map(str::trim).and_then(|notes| {
            if notes.is_empty() {
                None
            } else {
                Some(notes.to_string())
            }
        }),
        created_at: unix_now().to_string(),
    }
}

fn release_audit(
    release: &AgentUpdateReleaseView,
    request: &CreateAgentUpdateReleaseRequest,
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: "agent_update.release_recorded".to_string(),
        target: format!("agent_update_release:{}", release.id),
        command_hash: None,
        metadata: release_metadata(release, request, operator),
        created_at,
    }
}

fn release_metadata(
    release: &AgentUpdateReleaseView,
    request: &CreateAgentUpdateReleaseRequest,
    operator: &AuthContext,
) -> serde_json::Value {
    json!({
        "release_id": release.id,
        "name": &release.name,
        "version": &release.version,
        "channel": &release.channel,
        "status": &release.status,
        "artifact_sha256_hex": &release.artifact_sha256_hex,
        "artifact_signature_provided": true,
        "artifact_signature_sha256_hex": &release.artifact_signature_sha256_hex,
        "artifact_signing_key_sha256_hex": &release.artifact_signing_key_sha256_hex,
        "artifact_url_sha256_hex": &release.artifact_url_sha256_hex,
        "rollback_artifact_sha256_hex": &release.rollback_artifact_sha256_hex,
        "rollback_artifact_signature_provided": release.rollback_artifact_signature_provided,
        "rollback_artifact_signature_sha256_hex": &release.rollback_artifact_signature_sha256_hex,
        "rollback_artifact_signing_key_sha256_hex": &release.rollback_artifact_signing_key_sha256_hex,
        "rollback_artifact_url_sha256_hex": &release.rollback_artifact_url_sha256_hex,
        "rollback_size_bytes": release.rollback_size_bytes,
        "size_bytes": release.size_bytes,
        "confirmed": request.confirmed,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "session_id": operator.session_id,
        "metadata_only": true,
        "artifact_hosted": false,
        "artifact_object_key": serde_json::Value::Null,
        "artifact_download_path": serde_json::Value::Null,
        "rollback_artifact_object_key": serde_json::Value::Null,
        "rollback_artifact_download_path": serde_json::Value::Null,
        "artifact_url_stored": false,
        "artifact_signature_stored": false,
        "artifact_signing_key_stored": false,
        "rollback_artifact_url_stored": false,
        "rollback_artifact_signature_stored": false,
        "rollback_artifact_signing_key_stored": false,
    })
}

fn upload_release_audit(
    release: &AgentUpdateReleaseView,
    metadata: &UploadedAgentUpdateReleaseMetadata,
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: "agent_update.artifact_uploaded".to_string(),
        target: format!("agent_update_release:{}", release.id),
        command_hash: None,
        metadata: upload_release_metadata(release, metadata, operator),
        created_at,
    }
}

fn upload_release_metadata(
    release: &AgentUpdateReleaseView,
    metadata: &UploadedAgentUpdateReleaseMetadata,
    operator: &AuthContext,
) -> serde_json::Value {
    json!({
        "release_id": release.id,
        "name": &release.name,
        "version": &release.version,
        "channel": &release.channel,
        "status": &release.status,
        "artifact_sha256_hex": &release.artifact_sha256_hex,
        "artifact_signature_provided": true,
        "artifact_signature_sha256_hex": &release.artifact_signature_sha256_hex,
        "artifact_signing_key_sha256_hex": &release.artifact_signing_key_sha256_hex,
        "artifact_url_sha256_hex": serde_json::Value::Null,
        "artifact_object_key": &release.artifact_object_key,
        "artifact_download_path": &release.artifact_download_path,
        "rollback_artifact_sha256_hex": &release.rollback_artifact_sha256_hex,
        "rollback_artifact_signature_provided": release.rollback_artifact_signature_provided,
        "rollback_artifact_signature_sha256_hex": &release.rollback_artifact_signature_sha256_hex,
        "rollback_artifact_signing_key_sha256_hex": &release.rollback_artifact_signing_key_sha256_hex,
        "rollback_artifact_url_sha256_hex": serde_json::Value::Null,
        "rollback_artifact_object_key": &release.rollback_artifact_object_key,
        "rollback_artifact_download_path": &release.rollback_artifact_download_path,
        "rollback_size_bytes": release.rollback_size_bytes,
        "size_bytes": release.size_bytes,
        "confirmed": metadata.confirmed,
        "artifact_ingestion_mode": metadata.ingestion_mode,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "session_id": operator.session_id,
        "metadata_only": false,
        "artifact_hosted": true,
        "artifact_url_stored": false,
        "artifact_signature_stored": false,
        "artifact_signing_key_stored": false,
        "rollback_artifact_url_stored": false,
        "rollback_artifact_signature_stored": false,
        "rollback_artifact_signing_key_stored": false,
    })
}
