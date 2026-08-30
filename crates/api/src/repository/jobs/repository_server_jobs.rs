use anyhow::{ensure, Context, Result};
use serde_json::json;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;
use vpsman_common::{
    expression_matches, parse_expression, payload_hash, Expression, ExpressionContext,
    MAX_ARTIFACT_CLEANUP_REVIEWED_TARGETS, SERVER_JOB_STATUS_QUEUED,
    SERVER_JOB_TYPE_ARTIFACT_CLEANUP,
};

use crate::{
    model::{
        ArtifactCleanupPreviewObjectView, ArtifactCleanupPreviewView, AuthContext,
        NewServerArtifact, ServerArtifactCleanupCandidate, ServerArtifactReservation,
        ServerJobView,
    },
    repository::Repository,
};

const ARTIFACT_CLEANUP_CANDIDATE_PAGE_SIZE: i64 = 500;

impl Repository {
    pub(crate) async fn register_server_artifact(&self, artifact: NewServerArtifact) -> Result<()> {
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                register_active_server_artifact_in_tx(&mut tx, &artifact).await?;
                tx.commit().await?;
                Ok(())
            }
        }
    }

    pub(crate) async fn reserve_server_artifact(
        &self,
        artifact: NewServerArtifact,
    ) -> Result<ServerArtifactReservation> {
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                ensure!(
                    artifact.size_bytes >= 0,
                    "server_artifact_size_bytes_invalid"
                );
                let reservation_token = Uuid::new_v4();
                let inserted = sqlx::query_scalar::<_, Uuid>(
                    r#"
                    INSERT INTO server_artifacts (
                        id,
                        domain,
                        object_key,
                        sha256_hex,
                        size_bytes,
                        status,
                        reservation_token,
                        job_id,
                        client_id,
                        stream,
                        seq,
                        backup_request_id,
                        backup_artifact_id,
                        release_id,
                        metadata
                    )
                    VALUES (
                        $1, $2, $3, $4, $5, 'creating', $6,
                        $7, $8, $9, $10, $11, $12, $13, $14
                    )
                    ON CONFLICT (object_key) DO NOTHING
                    RETURNING reservation_token
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(&artifact.domain)
                .bind(&artifact.object_key)
                .bind(&artifact.sha256_hex)
                .bind(artifact.size_bytes)
                .bind(reservation_token)
                .bind(artifact.job_id)
                .bind(&artifact.client_id)
                .bind(&artifact.stream)
                .bind(artifact.seq)
                .bind(artifact.backup_request_id)
                .bind(artifact.backup_artifact_id)
                .bind(artifact.release_id)
                .bind(&artifact.metadata)
                .fetch_optional(&mut *tx)
                .await?;
                let outcome = if inserted.is_some() {
                    ServerArtifactReservation::Created(reservation_token)
                } else {
                    let identical_active =
                        existing_active_artifact_matches_in_tx(&mut tx, &artifact).await?;
                    if identical_active {
                        ServerArtifactReservation::AlreadyActiveIdentical
                    } else {
                        ServerArtifactReservation::Conflict
                    }
                };
                tx.commit().await?;
                Ok(outcome)
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

    pub(crate) async fn activate_server_artifact_reservation(
        &self,
        artifact: NewServerArtifact,
        reservation_token: Uuid,
    ) -> Result<()> {
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                activate_server_artifact_reservation_in_tx(&mut tx, &artifact, reservation_token)
                    .await?;
                tx.commit().await?;
                Ok(())
            }
        }
    }

    pub(crate) async fn upsert_server_artifact_in_tx(
        tx: &mut Transaction<'_, Postgres>,
        artifact: &NewServerArtifact,
        status: &str,
        reservation_token: Option<Uuid>,
    ) -> Result<()> {
        ensure!(status == "active", "server_artifact_status_invalid");
        match reservation_token {
            Some(token) => activate_server_artifact_reservation_in_tx(tx, artifact, token).await,
            None => register_active_server_artifact_in_tx(tx, artifact).await,
        }
    }

    pub(crate) async fn discard_server_artifact_reservation(
        &self,
        object_key: &str,
        reservation_token: Uuid,
    ) -> Result<bool> {
        match self {
            Self::Postgres(pool) => {
                let deleted = sqlx::query(
                    r#"
                    DELETE FROM server_artifacts
                    WHERE object_key = $1
                      AND status = 'creating'
                      AND reservation_token = $2
                    "#,
                )
                .bind(object_key)
                .bind(reservation_token)
                .execute(pool)
                .await?;
                Ok(deleted.rows_affected() == 1)
            }
        }
    }

    pub(crate) async fn fail_server_artifact_reservation(
        &self,
        object_key: &str,
        reservation_token: Uuid,
        error: &str,
    ) -> Result<bool> {
        match self {
            Self::Postgres(pool) => {
                let failed = sqlx::query(
                    r#"
                    UPDATE server_artifacts
                    SET status = 'delete_failed',
                        reservation_token = NULL,
                        metadata = metadata || jsonb_build_object(
                            'delete_error', left($3, 1000),
                            'delete_failed_at', now()::text
                        )
                    WHERE object_key = $1
                      AND status = 'creating'
                      AND reservation_token = $2
                    "#,
                )
                .bind(object_key)
                .bind(reservation_token)
                .bind(error)
                .execute(pool)
                .await?;
                Ok(failed.rows_affected() == 1)
            }
        }
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
        let matched = self
            .artifact_cleanup_matches(domains, parsed.as_ref())
            .await?;
        let preview = cleanup_preview_from_matches(expression, domains, &matched)?;
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
        match self {
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

    async fn artifact_cleanup_matches(
        &self,
        domains: &[String],
        expression: Option<&Expression>,
    ) -> Result<Vec<ServerArtifactCleanupCandidate>> {
        match self {
            Self::Postgres(pool) => {
                let internal_domains = artifact_cleanup_internal_domains(domains);
                let mut tx = pool.begin().await?;
                sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
                    .execute(&mut *tx)
                    .await?;
                let mut matched = Vec::new();
                let mut after_id = None;
                loop {
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
                            artifact.created_at::text AS created_at,
                            CASE
                                WHEN artifact.domain = 'backup_artifact' THEN EXISTS (
                                    SELECT 1
                                    FROM backup_requests requests
                                    JOIN backup_artifacts artifacts
                                      ON artifacts.id = requests.artifact_id
                                    WHERE (
                                        artifact.backup_artifact_id IS NOT NULL
                                        AND artifacts.id = artifact.backup_artifact_id
                                    )
                                    OR artifacts.object_key = artifact.object_key
                                )
                                ELSE false
                            END AS reference_protected
                        FROM server_artifacts artifact
                        WHERE artifact.status IN (
                            'creating',
                            'active',
                            'deleting',
                            'delete_failed'
                        )
                          AND artifact.domain = ANY($1)
                          AND ($2::uuid IS NULL OR artifact.id > $2)
                        ORDER BY artifact.id ASC
                        LIMIT $3
                        "#,
                    )
                    .bind(&internal_domains)
                    .bind(after_id)
                    .bind(ARTIFACT_CLEANUP_CANDIDATE_PAGE_SIZE)
                    .fetch_all(&mut *tx)
                    .await?;
                    if rows.is_empty() {
                        break;
                    }
                    let row_count = rows.len();
                    for row in rows {
                        let candidate = server_artifact_candidate_from_row(row)?;
                        after_id = Some(candidate.id);
                        if artifact_matches_cleanup_expression(&candidate, expression) {
                            ensure_artifact_cleanup_match_capacity(matched.len())?;
                            matched.push(candidate);
                        }
                    }
                    if row_count < ARTIFACT_CLEANUP_CANDIDATE_PAGE_SIZE as usize {
                        break;
                    }
                }
                tx.commit().await?;
                Ok(matched)
            }
        }
    }
}

async fn existing_active_artifact_matches_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    artifact: &NewServerArtifact,
) -> Result<bool> {
    let identical = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM server_artifacts
            WHERE object_key = $1
              AND status = 'active'
              AND domain = $2
              AND sha256_hex = $3
              AND size_bytes = $4
              AND job_id IS NOT DISTINCT FROM $5
              AND client_id IS NOT DISTINCT FROM $6
              AND stream IS NOT DISTINCT FROM $7
              AND seq IS NOT DISTINCT FROM $8
              AND backup_request_id IS NOT DISTINCT FROM $9
              AND backup_artifact_id IS NOT DISTINCT FROM $10
              AND release_id IS NOT DISTINCT FROM $11
        )
        "#,
    )
    .bind(&artifact.object_key)
    .bind(&artifact.domain)
    .bind(&artifact.sha256_hex)
    .bind(artifact.size_bytes)
    .bind(artifact.job_id)
    .bind(&artifact.client_id)
    .bind(&artifact.stream)
    .bind(artifact.seq)
    .bind(artifact.backup_request_id)
    .bind(artifact.backup_artifact_id)
    .bind(artifact.release_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(identical)
}

async fn register_active_server_artifact_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    artifact: &NewServerArtifact,
) -> Result<()> {
    ensure!(
        artifact.size_bytes >= 0,
        "server_artifact_size_bytes_invalid"
    );
    let inserted = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO server_artifacts (
            id,
            domain,
            object_key,
            sha256_hex,
            size_bytes,
            status,
            reservation_token,
            job_id,
            client_id,
            stream,
            seq,
            backup_request_id,
            backup_artifact_id,
            release_id,
            metadata
        )
        VALUES ($1, $2, $3, $4, $5, 'active', NULL, $6, $7, $8, $9, $10, $11, $12, $13)
        ON CONFLICT (object_key) DO NOTHING
        RETURNING id
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
    .bind(&artifact.metadata)
    .fetch_optional(&mut **tx)
    .await?;
    if inserted.is_none() {
        ensure!(
            existing_active_artifact_matches_in_tx(tx, artifact).await?,
            "server_artifact_object_key_conflict"
        );
    }
    Ok(())
}

async fn activate_server_artifact_reservation_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    artifact: &NewServerArtifact,
    reservation_token: Uuid,
) -> Result<()> {
    ensure!(
        artifact.size_bytes >= 0,
        "server_artifact_size_bytes_invalid"
    );
    let activated = sqlx::query(
        r#"
        UPDATE server_artifacts
        SET status = 'active',
            reservation_token = NULL,
            metadata = $12
        WHERE object_key = $1
          AND status = 'creating'
          AND reservation_token = $2
          AND domain = $3
          AND sha256_hex = $4
          AND size_bytes = $5
          AND job_id IS NOT DISTINCT FROM $6
          AND client_id IS NOT DISTINCT FROM $7
          AND stream IS NOT DISTINCT FROM $8
          AND seq IS NOT DISTINCT FROM $9
          AND backup_request_id IS NOT DISTINCT FROM $10
          AND backup_artifact_id IS NOT DISTINCT FROM $11
          AND release_id IS NOT DISTINCT FROM $13
        "#,
    )
    .bind(&artifact.object_key)
    .bind(reservation_token)
    .bind(&artifact.domain)
    .bind(&artifact.sha256_hex)
    .bind(artifact.size_bytes)
    .bind(artifact.job_id)
    .bind(&artifact.client_id)
    .bind(&artifact.stream)
    .bind(artifact.seq)
    .bind(artifact.backup_request_id)
    .bind(artifact.backup_artifact_id)
    .bind(&artifact.metadata)
    .bind(artifact.release_id)
    .execute(&mut **tx)
    .await?;
    ensure!(
        activated.rows_affected() == 1,
        "server_artifact_reservation_not_owned"
    );
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

fn ensure_artifact_cleanup_match_capacity(current: usize) -> Result<()> {
    ensure!(
        current < MAX_ARTIFACT_CLEANUP_REVIEWED_TARGETS,
        "artifact_cleanup_match_limit_exceeded: selector matches more than \
         {MAX_ARTIFACT_CLEANUP_REVIEWED_TARGETS} artifacts; narrow the domains or expression"
    );
    Ok(())
}

fn cleanup_preview_from_matches(
    expression: String,
    domains: &[String],
    matched: &[ServerArtifactCleanupCandidate],
) -> Result<ArtifactCleanupPreviewView> {
    let matched_count = i64::try_from(matched.len())
        .context("artifact_cleanup_review_numeric_invalid: matched count overflow")?;
    let matched_bytes = matched.iter().try_fold(0_i64, |total, candidate| {
        ensure!(
            candidate.size_bytes >= 0,
            "artifact_cleanup_review_numeric_invalid: artifact {} has a negative size",
            candidate.id
        );
        total.checked_add(candidate.size_bytes).with_context(|| {
            format!(
                "artifact_cleanup_review_numeric_invalid: matched byte total overflow at artifact {}",
                candidate.id
            )
        })
    })?;
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
    Ok(ArtifactCleanupPreviewView {
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
    })
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

#[cfg(test)]
#[path = "tests_repository_server_jobs.rs"]
mod tests;
