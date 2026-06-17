use anyhow::{ensure, Result};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;
use vpsman_common::{payload_hash, AgentUpdateReleaseStatus};

use crate::{
    model::{AgentUpdateReleaseView, AuditLogView, AuthContext, CreateAgentUpdateReleaseRequest},
    repository::Repository,
    unix_now,
};

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
                        artifact_url_sha256_hex,
                        rollback_artifact_sha256_hex,
                        rollback_artifact_url_sha256_hex,
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
                        artifact_url_sha256_hex,
                        rollback_artifact_sha256_hex,
                        rollback_artifact_url_sha256_hex,
                        rollback_size_bytes,
                        size_bytes,
                        notes
                    )
                    VALUES (
                        $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                        $11, $12, $13
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
                .bind(&release.artifact_url_sha256_hex)
                .bind(&release.rollback_artifact_sha256_hex)
                .bind(&release.rollback_artifact_url_sha256_hex)
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

    pub(crate) async fn agent_update_release_exists_for_artifact(
        &self,
        artifact_sha256_hex: &str,
    ) -> Result<bool> {
        let artifact_sha256_hex = artifact_sha256_hex.trim().to_ascii_lowercase();
        match self {
            Self::Memory(memory) => {
                Ok(memory
                    .agent_update_releases
                    .read()
                    .await
                    .iter()
                    .any(|release| {
                        matches!(
                            AgentUpdateReleaseStatus::from_storage(&release.status),
                            Some(AgentUpdateReleaseStatus::PublishedExternal)
                        ) && release.artifact_sha256_hex == artifact_sha256_hex
                    }))
            }
            Self::Postgres(pool) => {
                let exists: bool = sqlx::query_scalar(
                    r#"
                    SELECT EXISTS (
                        SELECT 1
                        FROM agent_update_releases
                        WHERE status = $1
                          AND artifact_sha256_hex = $2
                    )
                    "#,
                )
                .bind(AgentUpdateReleaseStatus::PublishedExternal.as_str())
                .bind(artifact_sha256_hex)
                .fetch_one(pool)
                .await?;
                Ok(exists)
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
        artifact_url_sha256_hex: row.try_get("artifact_url_sha256_hex")?,
        rollback_artifact_sha256_hex: row.try_get("rollback_artifact_sha256_hex")?,
        rollback_artifact_url_sha256_hex: row.try_get("rollback_artifact_url_sha256_hex")?,
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
        status: AgentUpdateReleaseStatus::PublishedExternal
            .as_str()
            .to_string(),
        artifact_sha256_hex: request.artifact_sha256_hex.trim().to_ascii_lowercase(),
        artifact_url_sha256_hex: Some(payload_hash(request.artifact_url.trim().as_bytes())),
        rollback_artifact_sha256_hex: request
            .rollback_artifact_sha256_hex
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase),
        rollback_artifact_url_sha256_hex: request
            .rollback_artifact_url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(|url| payload_hash(url.as_bytes())),
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
        "artifact_url_sha256_hex": &release.artifact_url_sha256_hex,
        "rollback_artifact_sha256_hex": &release.rollback_artifact_sha256_hex,
        "rollback_artifact_url_sha256_hex": &release.rollback_artifact_url_sha256_hex,
        "rollback_size_bytes": release.rollback_size_bytes,
        "size_bytes": release.size_bytes,
        "confirmed": request.confirmed,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "session_id": operator.session_id,
        "external_https": true,
        "artifact_url_stored": false,
        "rollback_artifact_url_stored": false,
    })
}
