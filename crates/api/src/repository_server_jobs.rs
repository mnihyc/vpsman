use anyhow::{ensure, Result};
use serde_json::json;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;
use vpsman_common::{
    expression_matches, parse_expression, payload_hash, Expression, ExpressionContext,
    ARTIFACT_CLEANUP_RUNNING_TIMEOUT_SECS, SERVER_JOB_STATUS_CANCELED, SERVER_JOB_STATUS_FAILED,
    SERVER_JOB_STATUS_QUEUED, SERVER_JOB_STATUS_RUNNING, SERVER_JOB_TYPE_ARTIFACT_CLEANUP,
};

use crate::{
    model::{
        ArtifactCleanupPreviewObjectView, ArtifactCleanupPreviewView, AuthContext,
        NewServerArtifact, ServerArtifactCleanupCandidate, ServerJobView,
    },
    repository::Repository,
    unix_now,
};

impl Repository {
    pub(crate) async fn register_server_artifact(&self, artifact: NewServerArtifact) -> Result<()> {
        match self {
            Self::Memory(memory) => upsert_memory_server_artifact(memory, artifact, "active").await,
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                insert_server_artifact_in_tx(&mut tx, &artifact, "active").await?;
                tx.commit().await?;
                Ok(())
            }
        }
    }

    pub(crate) async fn reserve_server_artifact(&self, artifact: NewServerArtifact) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                upsert_memory_server_artifact(memory, artifact, "creating").await
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                insert_server_artifact_in_tx(&mut tx, &artifact, "creating").await?;
                tx.commit().await?;
                Ok(())
            }
        }
    }

    pub(crate) async fn active_server_artifact_matches(
        &self,
        domain: &str,
        object_key: &str,
        sha256_hex: &str,
        size_bytes: i64,
    ) -> Result<bool> {
        match self {
            Self::Memory(memory) => {
                Ok(memory.server_artifacts.read().await.iter().any(|artifact| {
                    artifact.domain == domain
                        && artifact.object_key == object_key
                        && artifact.sha256_hex == sha256_hex
                        && artifact.size_bytes == size_bytes
                        && artifact.status == "active"
                }))
            }
            Self::Postgres(pool) => {
                let exists: bool = sqlx::query_scalar(
                    r#"
                    SELECT EXISTS (
                        SELECT 1
                        FROM server_artifacts
                        WHERE domain = $1
                          AND object_key = $2
                          AND sha256_hex = $3
                          AND size_bytes = $4
                          AND status = 'active'
                    )
                    "#,
                )
                .bind(domain)
                .bind(object_key)
                .bind(sha256_hex)
                .bind(size_bytes)
                .fetch_one(pool)
                .await?;
                Ok(exists)
            }
        }
    }

    pub(crate) async fn upsert_server_artifact_in_tx(
        tx: &mut Transaction<'_, Postgres>,
        artifact: &NewServerArtifact,
        status: &str,
    ) -> Result<()> {
        insert_server_artifact_in_tx(tx, artifact, status).await
    }

    pub(crate) async fn mark_server_artifact_deleted_in_tx(
        tx: &mut Transaction<'_, Postgres>,
        object_key: &str,
    ) -> Result<()> {
        mark_server_artifact_deleted_in_tx(tx, object_key).await
    }

    pub(crate) async fn discard_server_artifact_reservation(&self, object_key: &str) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                memory.server_artifacts.write().await.retain(|artifact| {
                    !(artifact.object_key == object_key && artifact.status == "creating")
                });
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    DELETE FROM server_artifacts
                    WHERE object_key = $1
                      AND status = 'creating'
                    "#,
                )
                .bind(object_key)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn mark_server_artifact_delete_failed(
        &self,
        object_key: &str,
        error: &str,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                if let Some(artifact) =
                    memory
                        .server_artifacts
                        .write()
                        .await
                        .iter_mut()
                        .find(|artifact| {
                            artifact.object_key == object_key
                                && matches!(
                                    artifact.status.as_str(),
                                    "creating" | "active" | "deleting" | "delete_failed"
                                )
                        })
                {
                    artifact.status = "delete_failed".to_string();
                }
                let _ = error;
                Ok(())
            }
            Self::Postgres(pool) => {
                mark_server_artifact_delete_failed_in_pool(pool, object_key, error).await
            }
        }
    }

    pub(crate) async fn mark_server_artifact_delete_failed_in_pool(
        pool: &sqlx::PgPool,
        object_key: &str,
        error: &str,
    ) -> Result<()> {
        mark_server_artifact_delete_failed_in_pool(pool, object_key, error).await
    }

    pub(crate) async fn mark_server_artifact_deleting_in_pool(
        pool: &sqlx::PgPool,
        object_key: &str,
    ) -> Result<bool> {
        mark_server_artifact_deleting_in_pool(pool, object_key).await
    }

    pub(crate) async fn preview_artifact_cleanup(
        &self,
        expression: &str,
        domains: &[String],
    ) -> Result<ArtifactCleanupPreviewView> {
        let (preview, _) = self
            .artifact_cleanup_preview_matches(expression, domains)
            .await?;
        Ok(preview)
    }

    async fn artifact_cleanup_preview_matches(
        &self,
        expression: &str,
        domains: &[String],
    ) -> Result<(
        ArtifactCleanupPreviewView,
        Vec<ServerArtifactCleanupCandidate>,
    )> {
        let expression = normalize_cleanup_expression(expression)?;
        let parsed = parse_cleanup_expression(&expression)?;
        let candidates = self.artifact_cleanup_candidates(domains).await?;
        let matched = candidates
            .into_iter()
            .filter(|candidate| artifact_matches_cleanup_expression(candidate, parsed.as_ref()))
            .collect::<Vec<_>>();
        let preview = cleanup_preview_from_matches(expression, domains, &matched);
        Ok((preview, matched))
    }

    pub(crate) async fn create_artifact_cleanup_job(
        &self,
        expression: &str,
        domains: &[String],
        preview_hash: &str,
        operator: &AuthContext,
    ) -> Result<ServerJobView> {
        let (preview, matched_artifacts) = self
            .artifact_cleanup_preview_matches(expression, domains)
            .await?;
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
                    metadata: json!({ "domains": preview.domains }),
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
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
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
                .bind(json!({ "domains": preview.domains }))
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
        self.expire_stale_running_artifact_cleanup_jobs().await?;
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
        self.expire_stale_running_artifact_cleanup_jobs().await?;
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

    pub(crate) async fn expire_stale_running_artifact_cleanup_jobs(&self) -> Result<i64> {
        let cutoff_unix = unix_now().saturating_sub(ARTIFACT_CLEANUP_RUNNING_TIMEOUT_SECS as u64);
        match self {
            Self::Memory(memory) => {
                let mut expired = 0_i64;
                let now = unix_now().to_string();
                let mut jobs = memory.server_jobs.write().await;
                for job in jobs.iter_mut().filter(|job| {
                    job.job_type == SERVER_JOB_TYPE_ARTIFACT_CLEANUP
                        && job.status == SERVER_JOB_STATUS_RUNNING
                }) {
                    let Some(started_at) = job.started_at.as_deref() else {
                        continue;
                    };
                    let Ok(started_unix) = started_at.parse::<u64>() else {
                        continue;
                    };
                    if started_unix >= cutoff_unix {
                        continue;
                    }
                    job.status = SERVER_JOB_STATUS_FAILED.to_string();
                    job.error = Some("artifact_cleanup_running_timeout".to_string());
                    job.completed_at = Some(now.clone());
                    expired += 1;
                }
                Ok(expired)
            }
            Self::Postgres(pool) => expire_stale_artifact_cleanup_jobs_in_pool(pool).await,
        }
    }

    async fn artifact_cleanup_candidates(
        &self,
        domains: &[String],
    ) -> Result<Vec<ServerArtifactCleanupCandidate>> {
        match self {
            Self::Memory(memory) => {
                let internal_domains = artifact_cleanup_internal_domains(domains);
                let backup_requests = memory.backup_requests.read().await.clone();
                let backup_artifacts = memory.backup_artifacts.read().await.clone();
                Ok(memory
                    .server_artifacts
                    .read()
                    .await
                    .iter()
                    .filter(|artifact| {
                        matches!(
                            artifact.status.as_str(),
                            "creating" | "active" | "deleting" | "delete_failed"
                        ) && internal_domains.contains(&artifact.domain)
                    })
                    .map(|artifact| {
                        let mut artifact = artifact.clone();
                        artifact.reference_protected = artifact.domain == "backup_artifact"
                            && backup_requests.iter().any(|request| {
                                request.artifact_id.is_some_and(|request_artifact_id| {
                                    artifact.backup_artifact_id == Some(request_artifact_id)
                                        || backup_artifacts.iter().any(|backup_artifact| {
                                            backup_artifact.id == request_artifact_id
                                                && backup_artifact.object_key == artifact.object_key
                                        })
                                })
                            });
                        artifact
                    })
                    .collect())
            }
            Self::Postgres(pool) => {
                let internal_domains = artifact_cleanup_internal_domains(domains);
                let rows = sqlx::query(
                    r#"
                    SELECT
                        artifact.id,
                        artifact.domain,
                        artifact.object_key,
                        artifact.sha256_hex,
                        artifact.size_bytes,
                        artifact.status,
                        artifact.job_id,
                        artifact.client_id,
                        artifact.stream,
                        artifact.seq,
                        artifact.backup_artifact_id,
                        artifact.created_at::text AS created_at,
                        CASE
                            WHEN artifact.domain = 'backup_artifact' THEN EXISTS (
                                SELECT 1
                                FROM backup_requests requests
                                JOIN backup_artifacts artifacts ON artifacts.id = requests.artifact_id
                                WHERE (
                                    artifact.backup_artifact_id IS NOT NULL
                                    AND artifacts.id = artifact.backup_artifact_id
                                )
                                OR artifacts.object_key = artifact.object_key
                            )
                            ELSE false
                        END AS reference_protected
                    FROM server_artifacts artifact
                    WHERE artifact.status IN ('creating', 'active', 'deleting', 'delete_failed')
                      AND artifact.domain = ANY($1)
                    ORDER BY artifact.created_at DESC, artifact.object_key ASC
                    LIMIT 10000
                    "#,
                )
                .bind(internal_domains)
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

async fn insert_server_artifact_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    artifact: &NewServerArtifact,
    status: &str,
) -> Result<()> {
    let row = sqlx::query(
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
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
        ON CONFLICT (object_key)
        DO UPDATE SET
            status = EXCLUDED.status,
            sha256_hex = EXCLUDED.sha256_hex,
            size_bytes = EXCLUDED.size_bytes,
            metadata = EXCLUDED.metadata,
            tombstoned_at = NULL,
            deleted_at = NULL
        WHERE server_artifacts.domain = EXCLUDED.domain
          AND server_artifacts.sha256_hex = EXCLUDED.sha256_hex
          AND server_artifacts.size_bytes = EXCLUDED.size_bytes
          AND server_artifacts.job_id IS NOT DISTINCT FROM EXCLUDED.job_id
          AND server_artifacts.client_id IS NOT DISTINCT FROM EXCLUDED.client_id
          AND server_artifacts.stream IS NOT DISTINCT FROM EXCLUDED.stream
          AND server_artifacts.seq IS NOT DISTINCT FROM EXCLUDED.seq
          AND server_artifacts.backup_request_id IS NOT DISTINCT FROM EXCLUDED.backup_request_id
          AND server_artifacts.backup_artifact_id IS NOT DISTINCT FROM EXCLUDED.backup_artifact_id
          AND server_artifacts.release_id IS NOT DISTINCT FROM EXCLUDED.release_id
          AND server_artifacts.status IN ('creating', 'active', 'delete_failed')
        RETURNING id
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(&artifact.domain)
    .bind(&artifact.object_key)
    .bind(&artifact.sha256_hex)
    .bind(artifact.size_bytes)
    .bind(status)
    .bind(artifact.job_id)
    .bind(&artifact.client_id)
    .bind(&artifact.stream)
    .bind(artifact.seq)
    .bind(artifact.backup_request_id)
    .bind(artifact.backup_artifact_id)
    .bind(artifact.release_id)
    .bind(&artifact.metadata)
    .fetch_optional(&mut **tx)
    .await?;
    ensure!(row.is_some(), "server_artifact_object_key_conflict");
    Ok(())
}

async fn mark_server_artifact_deleting_in_pool(
    pool: &sqlx::PgPool,
    object_key: &str,
) -> Result<bool> {
    let updated = sqlx::query(
        r#"
        UPDATE server_artifacts
        SET status = 'deleting',
            metadata = metadata - 'delete_error' - 'delete_failed_at'
        WHERE object_key = $1
          AND status IN ('creating', 'active', 'deleting', 'delete_failed')
        "#,
    )
    .bind(object_key)
    .execute(pool)
    .await?;
    Ok(updated.rows_affected() > 0)
}

async fn mark_server_artifact_delete_failed_in_pool(
    pool: &sqlx::PgPool,
    object_key: &str,
    error: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE server_artifacts
        SET status = 'delete_failed',
            metadata = metadata || jsonb_build_object(
                'delete_error', left($2, 1000),
                'delete_failed_at', now()::text
            )
        WHERE object_key = $1
          AND status IN ('creating', 'active', 'deleting', 'delete_failed')
        "#,
    )
    .bind(object_key)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

async fn mark_server_artifact_deleted_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    object_key: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE server_artifacts
        SET status = 'deleted',
            deleted_at = now()
        WHERE object_key = $1
          AND status IN ('creating', 'active', 'deleting', 'delete_failed')
        "#,
    )
    .bind(object_key)
    .execute(&mut **tx)
    .await?;
    Ok(())
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
                "reference_protected": candidate.reference_protected,
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
    domains: &[String],
    matched: &[ServerArtifactCleanupCandidate],
) -> ArtifactCleanupPreviewView {
    let matched_count = matched.len() as i64;
    let matched_bytes = matched.iter().map(|candidate| candidate.size_bytes).sum();
    let reference_protected_count = matched
        .iter()
        .filter(|candidate| candidate.reference_protected)
        .count() as i64;
    let retained_count = matched_count - reference_protected_count;
    let mut chronological = matched.to_vec();
    chronological.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.domain.cmp(&right.domain))
            .then_with(|| left.object_key.cmp(&right.object_key))
    });
    let oldest_created_at = chronological
        .first()
        .map(|candidate| candidate.created_at.clone());
    let newest_created_at = chronological
        .last()
        .map(|candidate| candidate.created_at.clone());
    let representative_objects = chronological
        .iter()
        .take(20)
        .map(|candidate| ArtifactCleanupPreviewObjectView {
            id: candidate.id,
            domain: candidate.domain.clone(),
            object_key: candidate.object_key.clone(),
            size_bytes: candidate.size_bytes,
            status: candidate.status.clone(),
            created_at: candidate.created_at.clone(),
            reference_protected: candidate.reference_protected,
            reason: candidate
                .reference_protected
                .then(|| "Reference protected by backup request".to_string()),
        })
        .collect();
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
    identity.insert(0, format!("domains:{}", domains.join(",")));
    let preview_hash = payload_hash(identity.join("\n").as_bytes());
    ArtifactCleanupPreviewView {
        expression,
        domains: domains.to_vec(),
        preview_hash,
        matched_count,
        matched_bytes,
        oldest_created_at,
        newest_created_at,
        retained_count,
        reference_protected_count,
        representative_objects,
        full_list_download_url: None,
    }
}

fn artifact_cleanup_internal_domains(domains: &[String]) -> Vec<String> {
    let mut internal = Vec::new();
    for domain in domains {
        match domain.as_str() {
            "job_output" => internal.push("job_output".to_string()),
            "file_transfer" => {
                internal.push("file_transfer_handoff".to_string());
                internal.push("file_transfer_source".to_string());
            }
            "backup_artifact" => internal.push("backup_artifact".to_string()),
            _ => {}
        }
    }
    internal
}

async fn upsert_memory_server_artifact(
    memory: &crate::repository::MemoryState,
    artifact: NewServerArtifact,
    status: &str,
) -> Result<()> {
    let mut artifacts = memory.server_artifacts.write().await;
    if let Some(existing) = artifacts
        .iter_mut()
        .find(|existing| existing.object_key == artifact.object_key)
    {
        ensure!(
            existing.domain == artifact.domain
                && existing.sha256_hex == artifact.sha256_hex
                && existing.size_bytes == artifact.size_bytes
                && existing.job_id == artifact.job_id
                && existing.client_id == artifact.client_id
                && existing.stream == artifact.stream
                && existing.seq == artifact.seq
                && existing.backup_artifact_id == artifact.backup_artifact_id
                && matches!(
                    existing.status.as_str(),
                    "creating" | "active" | "delete_failed"
                ),
            "server_artifact_object_key_conflict"
        );
        existing.status = status.to_string();
        return Ok(());
    }
    artifacts.push(ServerArtifactCleanupCandidate {
        id: Uuid::new_v4(),
        domain: artifact.domain,
        object_key: artifact.object_key,
        sha256_hex: artifact.sha256_hex,
        size_bytes: artifact.size_bytes,
        status: status.to_string(),
        job_id: artifact.job_id,
        client_id: artifact.client_id,
        stream: artifact.stream,
        seq: artifact.seq,
        backup_artifact_id: artifact.backup_artifact_id,
        created_at: unix_now().to_string(),
        reference_protected: false,
    });
    Ok(())
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
        backup_artifact_id: row.try_get("backup_artifact_id")?,
        created_at: row.try_get("created_at")?,
        reference_protected: row.try_get("reference_protected")?,
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

async fn expire_stale_artifact_cleanup_jobs_in_pool(pool: &sqlx::PgPool) -> Result<i64> {
    let result = sqlx::query(
        r#"
        UPDATE server_jobs
        SET
            status = $3,
            error = 'artifact_cleanup_running_timeout',
            completed_at = now(),
            metadata = metadata || jsonb_build_object(
                'running_timeout_secs', $4::bigint
            )
        WHERE job_type = $1
          AND status = $2
          AND started_at IS NOT NULL
          AND started_at <= now() - ($4::bigint * interval '1 second')
        "#,
    )
    .bind(SERVER_JOB_TYPE_ARTIFACT_CLEANUP)
    .bind(SERVER_JOB_STATUS_RUNNING)
    .bind(SERVER_JOB_STATUS_FAILED)
    .bind(ARTIFACT_CLEANUP_RUNNING_TIMEOUT_SECS)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_preview_includes_age_protection_and_representative_objects() {
        let first_id = Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap();
        let second_id = Uuid::parse_str("22222222-2222-4333-8444-555555555555").unwrap();
        let matched = vec![
            ServerArtifactCleanupCandidate {
                id: second_id,
                domain: "backup_artifact".to_string(),
                object_key: "backup-artifacts/protected.tar.zst".to_string(),
                sha256_hex: "b".repeat(64),
                size_bytes: 20,
                status: "active".to_string(),
                job_id: None,
                client_id: Some("agent-fra-02".to_string()),
                stream: None,
                seq: None,
                backup_artifact_id: Some(second_id),
                created_at: "2026-06-02T10:00:00Z".to_string(),
                reference_protected: true,
            },
            ServerArtifactCleanupCandidate {
                id: first_id,
                domain: "file_transfer_source".to_string(),
                object_key: "file-transfer-sources/payload.bin".to_string(),
                sha256_hex: "a".repeat(64),
                size_bytes: 10,
                status: "active".to_string(),
                job_id: None,
                client_id: Some("agent-sfo-01".to_string()),
                stream: None,
                seq: None,
                backup_artifact_id: None,
                created_at: "2026-05-31T10:00:00Z".to_string(),
                reference_protected: false,
            },
        ];

        let preview = cleanup_preview_from_matches(
            "artifact.status = \"active\"".to_string(),
            &["file_transfer".to_string(), "backup_artifact".to_string()],
            &matched,
        );

        assert_eq!(preview.matched_count, 2);
        assert_eq!(preview.matched_bytes, 30);
        assert_eq!(
            preview.oldest_created_at.as_deref(),
            Some("2026-05-31T10:00:00Z")
        );
        assert_eq!(
            preview.newest_created_at.as_deref(),
            Some("2026-06-02T10:00:00Z")
        );
        assert_eq!(preview.retained_count, 1);
        assert_eq!(preview.reference_protected_count, 1);
        assert_eq!(preview.representative_objects.len(), 2);
        assert_eq!(
            preview.representative_objects[0].object_key,
            "file-transfer-sources/payload.bin"
        );
        assert!(!preview.representative_objects[0].reference_protected);
        assert!(preview.representative_objects[1].reference_protected);
        assert_eq!(
            preview.representative_objects[1].reason.as_deref(),
            Some("Reference protected by backup request")
        );
    }
}
