use anyhow::{ensure, Result};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;
use vpsman_common::{
    expression_matches, parse_expression, payload_hash, Expression, ExpressionContext,
    SERVER_JOB_STATUS_CANCELED, SERVER_JOB_STATUS_QUEUED, SERVER_JOB_TYPE_ARTIFACT_CLEANUP,
};

use crate::{
    model::{
        ArtifactCleanupPreviewView, AuthContext, NewServerArtifact, ServerArtifactCleanupCandidate,
        ServerJobView,
    },
    repository::Repository,
    unix_now,
};

impl Repository {
    pub(crate) async fn register_server_artifact(&self, artifact: NewServerArtifact) -> Result<()> {
        let Self::Postgres(pool) = self else {
            return Ok(());
        };
        sqlx::query(
            r#"
            INSERT INTO server_artifacts (
                id,
                domain,
                object_key,
                sha256_hex,
                size_bytes,
                status,
                job_id,
                client_id,
                stream,
                seq,
                backup_request_id,
                backup_artifact_id,
                release_id,
                metadata
            )
            VALUES ($1, $2, $3, $4, $5, 'active', $6, $7, $8, $9, $10, $11, $12, $13)
            ON CONFLICT (object_key)
            DO UPDATE SET
                domain = EXCLUDED.domain,
                sha256_hex = EXCLUDED.sha256_hex,
                size_bytes = EXCLUDED.size_bytes,
                status = 'active',
                job_id = EXCLUDED.job_id,
                client_id = EXCLUDED.client_id,
                stream = EXCLUDED.stream,
                seq = EXCLUDED.seq,
                backup_request_id = EXCLUDED.backup_request_id,
                backup_artifact_id = EXCLUDED.backup_artifact_id,
                release_id = EXCLUDED.release_id,
                metadata = EXCLUDED.metadata,
                tombstoned_at = NULL,
                deleted_at = NULL
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&artifact.domain)
        .bind(&artifact.object_key)
        .bind(&artifact.sha256_hex)
        .bind(artifact.size_bytes)
        .bind(artifact.job_id)
        .bind(&artifact.client_id)
        .bind(&artifact.stream)
        .bind(artifact.seq)
        .bind(artifact.backup_request_id)
        .bind(artifact.backup_artifact_id)
        .bind(artifact.release_id)
        .bind(artifact.metadata)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub(crate) async fn preview_artifact_cleanup(
        &self,
        expression: &str,
    ) -> Result<ArtifactCleanupPreviewView> {
        let (preview, _) = self.artifact_cleanup_preview_matches(expression).await?;
        Ok(preview)
    }

    async fn artifact_cleanup_preview_matches(
        &self,
        expression: &str,
    ) -> Result<(
        ArtifactCleanupPreviewView,
        Vec<ServerArtifactCleanupCandidate>,
    )> {
        let expression = normalize_cleanup_expression(expression)?;
        let parsed = parse_cleanup_expression(&expression)?;
        let candidates = self.artifact_cleanup_candidates(&expression).await?;
        let matched = candidates
            .into_iter()
            .filter(|candidate| artifact_matches_cleanup_expression(candidate, parsed.as_ref()))
            .collect::<Vec<_>>();
        let preview = cleanup_preview_from_matches(expression, &matched);
        Ok((preview, matched))
    }

    pub(crate) async fn create_artifact_cleanup_job(
        &self,
        expression: &str,
        preview_hash: &str,
        operator: &AuthContext,
    ) -> Result<ServerJobView> {
        let (preview, matched_artifacts) =
            self.artifact_cleanup_preview_matches(expression).await?;
        ensure!(
            preview.preview_hash == preview_hash,
            "artifact_cleanup_preview_hash_mismatch"
        );
        let job_id = Uuid::new_v4();
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let view = ServerJobView {
                    id: job_id,
                    job_type: SERVER_JOB_TYPE_ARTIFACT_CLEANUP.to_string(),
                    status: SERVER_JOB_STATUS_QUEUED.to_string(),
                    expression: Some(preview.expression),
                    preview_hash: Some(preview.preview_hash),
                    matched_count: preview.matched_count,
                    matched_bytes: preview.matched_bytes,
                    deleted_count: 0,
                    deleted_bytes: 0,
                    error: None,
                    created_by: Some(operator.operator.id),
                    metadata: json!({}),
                    created_at: now,
                    started_at: None,
                    completed_at: None,
                    canceled_at: None,
                };
                memory.server_jobs.write().await.push(view.clone());
                Ok(view)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    INSERT INTO server_jobs (
                        id,
                        job_type,
                        status,
                        expression,
                        preview_hash,
                        matched_count,
                        matched_bytes,
                        created_by,
                        metadata
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, '{}'::jsonb)
                    RETURNING
                        id,
                        job_type,
                        status,
                        expression,
                        preview_hash,
                        matched_count,
                        matched_bytes,
                        deleted_count,
                        deleted_bytes,
                        error,
                        created_by,
                        metadata,
                        created_at::text AS created_at,
                        started_at::text AS started_at,
                        completed_at::text AS completed_at,
                        canceled_at::text AS canceled_at
                    "#,
                )
                .bind(job_id)
                .bind(SERVER_JOB_TYPE_ARTIFACT_CLEANUP)
                .bind(SERVER_JOB_STATUS_QUEUED)
                .bind(&preview.expression)
                .bind(&preview.preview_hash)
                .bind(preview.matched_count)
                .bind(preview.matched_bytes)
                .bind(operator.operator.id)
                .fetch_one(&mut *tx)
                .await?;
                for artifact in &matched_artifacts {
                    sqlx::query(
                        r#"
                        INSERT INTO server_job_artifact_cleanup_targets (
                            server_job_id,
                            artifact_id,
                            domain,
                            object_key,
                            sha256_hex,
                            size_bytes,
                            status_at_review
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, $7)
                        "#,
                    )
                    .bind(job_id)
                    .bind(artifact.id)
                    .bind(&artifact.domain)
                    .bind(&artifact.object_key)
                    .bind(&artifact.sha256_hex)
                    .bind(artifact.size_bytes)
                    .bind(&artifact.status)
                    .execute(&mut *tx)
                    .await?;
                }
                tx.commit().await?;
                Ok(server_job_from_row(row)?)
            }
        }
    }

    pub(crate) async fn list_server_jobs(&self, limit: i64) -> Result<Vec<ServerJobView>> {
        let limit = limit.clamp(1, 200);
        match self {
            Self::Memory(memory) => Ok(memory
                .server_jobs
                .read()
                .await
                .iter()
                .rev()
                .take(limit as usize)
                .cloned()
                .collect()),
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        job_type,
                        status,
                        expression,
                        preview_hash,
                        matched_count,
                        matched_bytes,
                        deleted_count,
                        deleted_bytes,
                        error,
                        created_by,
                        metadata,
                        created_at::text AS created_at,
                        started_at::text AS started_at,
                        completed_at::text AS completed_at,
                        canceled_at::text AS canceled_at
                    FROM server_jobs
                    ORDER BY created_at DESC, id DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(server_job_from_row)
                    .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
                    .map_err(Into::into)
            }
        }
    }

    pub(crate) async fn cancel_server_job(&self, job_id: Uuid) -> Result<Option<ServerJobView>> {
        match self {
            Self::Memory(memory) => {
                let mut jobs = memory.server_jobs.write().await;
                let Some(job) = jobs.iter_mut().find(|job| job.id == job_id) else {
                    return Ok(None);
                };
                if job.status == SERVER_JOB_STATUS_QUEUED {
                    job.status = SERVER_JOB_STATUS_CANCELED.to_string();
                    job.canceled_at = Some(unix_now().to_string());
                    job.completed_at = job.canceled_at.clone();
                }
                Ok(Some(job.clone()))
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    UPDATE server_jobs
                    SET
                        status = 'canceled',
                        canceled_at = now(),
                        completed_at = now()
                    WHERE id = $1
                      AND status = 'queued'
                    RETURNING
                        id,
                        job_type,
                        status,
                        expression,
                        preview_hash,
                        matched_count,
                        matched_bytes,
                        deleted_count,
                        deleted_bytes,
                        error,
                        created_by,
                        metadata,
                        created_at::text AS created_at,
                        started_at::text AS started_at,
                        completed_at::text AS completed_at,
                        canceled_at::text AS canceled_at
                    "#,
                )
                .bind(job_id)
                .fetch_optional(pool)
                .await?;
                row.map(server_job_from_row).transpose().map_err(Into::into)
            }
        }
    }

    async fn artifact_cleanup_candidates(
        &self,
        _expression: &str,
    ) -> Result<Vec<ServerArtifactCleanupCandidate>> {
        match self {
            Self::Memory(_) => Ok(Vec::new()),
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        domain,
                        object_key,
                        sha256_hex,
                        size_bytes,
                        status,
                        job_id,
                        client_id,
                        stream,
                        seq,
                        created_at::text AS created_at
                    FROM server_artifacts
                    WHERE status IN ('active', 'deleting')
                    ORDER BY created_at DESC, object_key ASC
                    LIMIT 10000
                    "#,
                )
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(server_artifact_candidate_from_row)
                    .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
                    .map_err(Into::into)
            }
        }
    }
}

fn normalize_cleanup_expression(expression: &str) -> Result<String> {
    let expression = expression.trim();
    ensure!(
        !expression.is_empty(),
        "artifact_cleanup_expression_required"
    );
    ensure!(
        expression.len() <= 4096 && !expression.as_bytes().contains(&0),
        "artifact_cleanup_expression_invalid"
    );
    Ok(expression.to_string())
}

fn parse_cleanup_expression(expression: &str) -> Result<Option<Expression>> {
    parse_expression(expression)
        .map_err(|error| anyhow::anyhow!("artifact_cleanup_expression_invalid: {error}"))
}

fn artifact_matches_cleanup_expression(
    candidate: &ServerArtifactCleanupCandidate,
    expression: Option<&Expression>,
) -> bool {
    let Some(expression) = expression else {
        return true;
    };
    let context = ExpressionContext {
        objects: [(
            "artifact".to_string(),
            json!({
                "domain": &candidate.domain,
                "object": &candidate.object_key,
                "size": candidate.size_bytes,
                "status": &candidate.status,
                "job": candidate.job_id.map(|id| id.to_string()),
                "client": candidate.client_id.as_deref(),
                "stream": candidate.stream.as_deref(),
                "seq": candidate.seq,
                "sha256": &candidate.sha256_hex,
                "created_at": &candidate.created_at,
            }),
        )]
        .into_iter()
        .collect(),
        ..ExpressionContext::default()
    };
    expression_matches(&context, expression)
}

fn cleanup_preview_from_matches(
    expression: String,
    matched: &[ServerArtifactCleanupCandidate],
) -> ArtifactCleanupPreviewView {
    let matched_count = matched.len() as i64;
    let matched_bytes = matched.iter().map(|candidate| candidate.size_bytes).sum();
    let mut identity = matched
        .iter()
        .map(|candidate| {
            format!(
                "{}:{}:{}:{}",
                candidate.id, candidate.domain, candidate.object_key, candidate.sha256_hex
            )
        })
        .collect::<Vec<_>>();
    identity.sort();
    let preview_hash = payload_hash(identity.join("\n").as_bytes());
    ArtifactCleanupPreviewView {
        expression,
        preview_hash,
        matched_count,
        matched_bytes,
    }
}

fn server_artifact_candidate_from_row(
    row: sqlx::postgres::PgRow,
) -> std::result::Result<ServerArtifactCleanupCandidate, sqlx::Error> {
    Ok(ServerArtifactCleanupCandidate {
        id: row.try_get("id")?,
        domain: row.try_get("domain")?,
        object_key: row.try_get("object_key")?,
        sha256_hex: row.try_get("sha256_hex")?,
        size_bytes: row.try_get("size_bytes")?,
        status: row.try_get("status")?,
        job_id: row.try_get("job_id")?,
        client_id: row.try_get("client_id")?,
        stream: row.try_get("stream")?,
        seq: row.try_get("seq")?,
        created_at: row.try_get("created_at")?,
    })
}

fn server_job_from_row(
    row: sqlx::postgres::PgRow,
) -> std::result::Result<ServerJobView, sqlx::Error> {
    Ok(ServerJobView {
        id: row.try_get("id")?,
        job_type: row.try_get("job_type")?,
        status: row.try_get("status")?,
        expression: row.try_get("expression")?,
        preview_hash: row.try_get("preview_hash")?,
        matched_count: row.try_get("matched_count")?,
        matched_bytes: row.try_get("matched_bytes")?,
        deleted_count: row.try_get("deleted_count")?,
        deleted_bytes: row.try_get("deleted_bytes")?,
        error: row.try_get("error")?,
        created_by: row.try_get("created_by")?,
        metadata: row.try_get("metadata")?,
        created_at: row.try_get("created_at")?,
        started_at: row.try_get("started_at")?,
        completed_at: row.try_get("completed_at")?,
        canceled_at: row.try_get("canceled_at")?,
    })
}
