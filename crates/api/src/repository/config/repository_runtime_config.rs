use anyhow::{Context, Result};
use sqlx::{postgres::PgPool, Postgres, Row, Transaction};
use uuid::Uuid;
use vpsman_common::AgentRuntimeConfig;

use crate::runtime_config_workspace::runtime_config_override_revision;
use crate::{
    model::{
        AuthContext, RuntimeConfigApplyStateRecord, RuntimeConfigApplyStateView,
        RuntimeConfigOverrideReplacement, RuntimeConfigOverrideView,
    },
    repository::Repository,
    repository_key_lifecycle::lock_postgres_client_lifecycles_in_tx,
};

#[derive(Clone, Debug)]
pub(crate) struct ClaimedRuntimeConfigReconciliation {
    pub(crate) client_id: String,
    pub(crate) desired_revision: i64,
    pub(crate) reason: String,
    pub(crate) claim_token: Uuid,
    pub(crate) apply_version: u64,
    pool: PgPool,
}

pub(crate) enum RuntimeConfigDesiredStateGuard {
    Postgres { tx: Transaction<'static, Postgres> },
}

impl ClaimedRuntimeConfigReconciliation {
    pub(crate) async fn renew(&self, lease_secs: i32) -> Result<bool> {
        let result = sqlx::query(
            r#"
            UPDATE client_runtime_config_reconcile_work
            SET lease_until = now() + make_interval(secs => $4), updated_at = now()
            WHERE client_id = $1
              AND desired_revision = $2
              AND claim_token = $3
            "#,
        )
        .bind(&self.client_id)
        .bind(self.desired_revision)
        .bind(self.claim_token)
        .bind(lease_secs)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub(crate) async fn defer(self, error: &str, retry_secs: i32) -> Result<bool> {
        let error = error.chars().take(4096).collect::<String>();
        let result = sqlx::query(
            r#"
            UPDATE client_runtime_config_reconcile_work
            SET claim_token = NULL,
                claim_revision = NULL,
                apply_version = NULL,
                lease_until = NULL,
                next_attempt_at = now() + make_interval(secs => $4),
                last_error = $5,
                updated_at = now()
            WHERE client_id = $1
              AND desired_revision = $2
              AND claim_token = $3
            "#,
        )
        .bind(&self.client_id)
        .bind(self.desired_revision)
        .bind(self.claim_token)
        .bind(retry_secs)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Completes an effective no-op under the same token/revision/lease fence
    /// used by job creation. The nested option distinguishes "not current"
    /// from "already applied" (no job id) and "same job queued".
    pub(crate) async fn acknowledge_if_content_current(
        &self,
        content_hash: &str,
    ) -> Result<Option<Option<Uuid>>> {
        let mut tx = self.pool.begin().await?;
        let owns_revision = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT source_revision
            FROM client_runtime_config_owners
            WHERE client_id = $1
              AND source_revision = $2
            FOR UPDATE
            "#,
        )
        .bind(&self.client_id)
        .bind(self.desired_revision)
        .fetch_optional(&mut *tx)
        .await?
        .is_some();
        if !owns_revision {
            tx.rollback().await?;
            return Ok(None);
        }

        let owns_work = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT desired_revision
            FROM client_runtime_config_reconcile_work
            WHERE client_id = $1
              AND desired_revision = $2
              AND claim_token = $3
              AND apply_version = $4
              AND lease_until > now()
            FOR UPDATE
            "#,
        )
        .bind(&self.client_id)
        .bind(self.desired_revision)
        .bind(self.claim_token)
        .bind(self.apply_version as i64)
        .fetch_optional(&mut *tx)
        .await?
        .is_some();
        if !owns_work {
            tx.rollback().await?;
            return Ok(None);
        }

        let state = sqlx::query(
            r#"
            SELECT
                COALESCE(lower(applied_content_hash) = lower($2), FALSE)
                    AS applied_current,
                COALESCE(
                    pending_status = 'queued'
                    AND lower(pending_content_hash) = lower($2),
                    FALSE
                ) AS pending_current,
                pending_job_id
            FROM client_runtime_config_apply_state
            WHERE client_id = $1
            FOR UPDATE
            "#,
        )
        .bind(&self.client_id)
        .bind(content_hash)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(state) = state else {
            tx.rollback().await?;
            return Ok(None);
        };
        let applied_current: bool = state.try_get("applied_current")?;
        let pending_current: bool = state.try_get("pending_current")?;
        if !applied_current && !pending_current {
            tx.rollback().await?;
            return Ok(None);
        }
        let pending_job_id: Option<Uuid> = if pending_current {
            state.try_get("pending_job_id")?
        } else {
            None
        };

        let advanced = sqlx::query(
            r#"
            UPDATE client_runtime_config_owners
            SET reconciled_revision = $2, updated_at = now()
            WHERE client_id = $1
              AND source_revision = $2
              AND reconciled_revision <= $2
            "#,
        )
        .bind(&self.client_id)
        .bind(self.desired_revision)
        .execute(&mut *tx)
        .await?;
        anyhow::ensure!(
            advanced.rows_affected() == 1,
            "runtime_config_reconcile_claim_lost"
        );
        let deleted = sqlx::query(
            r#"
            DELETE FROM client_runtime_config_reconcile_work
            WHERE client_id = $1
              AND desired_revision = $2
              AND claim_token = $3
              AND apply_version = $4
            "#,
        )
        .bind(&self.client_id)
        .bind(self.desired_revision)
        .bind(self.claim_token)
        .bind(self.apply_version as i64)
        .execute(&mut *tx)
        .await?;
        anyhow::ensure!(
            deleted.rows_affected() == 1,
            "runtime_config_reconcile_claim_lost"
        );
        tx.commit().await?;
        Ok(Some(pending_job_id))
    }
}

pub(crate) async fn queue_runtime_config_apply_postgres_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    desired_revision: i64,
    version: u64,
    content_hash: &str,
    config: &AgentRuntimeConfig,
    job_id: Uuid,
    reason: &str,
    claim_token: Uuid,
) -> Result<()> {
    let reason = reason.chars().take(4096).collect::<String>();
    // Producers and consumers both acquire the exact desired-state owner
    // before its work row. This prevents the source-trigger owner->work path
    // from cycling with completion while retaining the same revision fence.
    let owned_revision = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT source_revision
        FROM client_runtime_config_owners
        WHERE client_id = $1
          AND source_revision = $2
        FOR UPDATE
        "#,
    )
    .bind(client_id)
    .bind(desired_revision)
    .fetch_optional(&mut **tx)
    .await?
    .context("runtime_config_reconcile_claim_lost")?;
    let deleted_revision = sqlx::query_scalar::<_, i64>(
        r#"
        DELETE FROM client_runtime_config_reconcile_work
        WHERE client_id = $1
          AND desired_revision = $2
          AND claim_revision = $2
          AND apply_version = $3
          AND claim_token = $4
          AND lease_until > now()
        RETURNING desired_revision
        "#,
    )
    .bind(client_id)
    .bind(desired_revision)
    .bind(version as i64)
    .bind(claim_token)
    .fetch_optional(&mut **tx)
    .await?;
    let deleted_revision = deleted_revision.context("runtime_config_reconcile_claim_lost")?;
    anyhow::ensure!(
        deleted_revision == owned_revision,
        "runtime_config_reconcile_claim_lost"
    );
    let advanced = sqlx::query(
        r#"
        UPDATE client_runtime_config_owners
        SET reconciled_revision = $2, updated_at = now()
        WHERE client_id = $1
          AND source_revision = $2
          AND reconciled_revision <= $2
        "#,
    )
    .bind(client_id)
    .bind(owned_revision)
    .execute(&mut **tx)
    .await?;
    anyhow::ensure!(
        advanced.rows_affected() == 1,
        "runtime_config_reconcile_claim_lost"
    );
    sqlx::query(
        r#"
        INSERT INTO client_runtime_config_apply_state (
            client_id,
            pending_version,
            pending_content_hash,
            pending_config,
            pending_job_id,
            pending_reason,
            pending_status,
            pending_error,
            pending_updated_at,
            updated_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, 'queued', NULL, now(), now())
        ON CONFLICT (client_id)
        DO UPDATE SET
            pending_version = EXCLUDED.pending_version,
            pending_content_hash = EXCLUDED.pending_content_hash,
            pending_config = EXCLUDED.pending_config,
            pending_job_id = EXCLUDED.pending_job_id,
            pending_reason = EXCLUDED.pending_reason,
            pending_status = 'queued',
            pending_error = NULL,
            pending_updated_at = now(),
            updated_at = now()
        WHERE EXCLUDED.pending_version > GREATEST(
            COALESCE(client_runtime_config_apply_state.applied_version, 0),
            COALESCE(client_runtime_config_apply_state.pending_version, 0)
        )
        "#,
    )
    .bind(client_id)
    .bind(version as i64)
    .bind(content_hash)
    .bind(sqlx::types::Json(config.clone()))
    .bind(job_id)
    .bind(reason)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

impl Repository {
    /// Ensures an exact client has durable work without superseding a producer
    /// revision already committed by the source mutation. This is the
    /// post-commit response/wake path; source triggers remain the authority.
    pub(crate) async fn ensure_runtime_config_reconciliations(
        &self,
        client_ids: &[String],
        reason: &str,
        requested_by: Option<Uuid>,
    ) -> Result<()> {
        if client_ids.is_empty() {
            return Ok(());
        }
        let reason = reason.chars().take(4096).collect::<String>();
        match self {
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    UPDATE client_runtime_config_reconcile_work
                    SET reason = $2, requested_by = $3, updated_at = now()
                    WHERE client_id = ANY($1::text[])
                      AND claim_token IS NULL
                    "#,
                )
                .bind(client_ids)
                .bind(reason)
                .bind(requested_by)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    /// Explicit reloads are themselves desired-state events. Unlike the
    /// response-only ensure path, they supersede an older claim atomically.
    pub(crate) async fn enqueue_runtime_config_reconciliations(
        &self,
        client_ids: &[String],
        reason: &str,
        requested_by: Option<Uuid>,
    ) -> Result<()> {
        if client_ids.is_empty() {
            return Ok(());
        }
        match self {
            Self::Postgres(pool) => {
                sqlx::query("SELECT enqueue_runtime_config_reconcile($1, $2, $3)")
                    .bind(client_ids)
                    .bind(reason)
                    .bind(requested_by)
                    .execute(pool)
                    .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn claim_runtime_config_reconciliation(
        &self,
        client_id: Option<&str>,
        lease_secs: i32,
    ) -> Result<Option<ClaimedRuntimeConfigReconciliation>> {
        match self {
            Self::Postgres(pool) => {
                let claim_token = Uuid::new_v4();
                let row = sqlx::query(
                    r#"
                    WITH candidate AS (
                        SELECT work.client_id
                        FROM client_runtime_config_reconcile_work work
                        JOIN visible_clients client ON client.id = work.client_id
                        WHERE ($1::text IS NULL OR work.client_id = $1)
                          AND client.status NOT IN ('suspended', 'revoked', 'deleted')
                          AND client.status <> 'never'
                          AND client.process_incarnation_id IS NOT NULL
                          AND work.next_attempt_at <= now()
                          AND (work.claim_token IS NULL OR work.lease_until <= now())
                        ORDER BY work.next_attempt_at, work.updated_at, work.client_id
                        FOR UPDATE OF work SKIP LOCKED
                        LIMIT 1
                    )
                    UPDATE client_runtime_config_reconcile_work work
                    SET claim_token = $2,
                        claim_revision = work.desired_revision,
                        apply_version = nextval('runtime_config_apply_version_seq'),
                        lease_until = now() + make_interval(secs => $3),
                        attempt_count = work.attempt_count + 1,
                        last_error = NULL,
                        updated_at = now()
                    FROM candidate
                    WHERE work.client_id = candidate.client_id
                    RETURNING
                        work.client_id,
                        work.desired_revision,
                        work.reason,
                        work.claim_token,
                        work.apply_version
                    "#,
                )
                .bind(client_id)
                .bind(claim_token)
                .bind(lease_secs)
                .fetch_optional(pool)
                .await?;
                row.map(|row| {
                    let apply_version: i64 = row.try_get("apply_version")?;
                    anyhow::ensure!(apply_version > 0, "runtime_config_apply_version_invalid");
                    Ok(ClaimedRuntimeConfigReconciliation {
                        client_id: row.try_get("client_id")?,
                        desired_revision: row.try_get("desired_revision")?,
                        reason: row.try_get("reason")?,
                        claim_token: row.try_get("claim_token")?,
                        apply_version: apply_version as u64,
                        pool: pool.clone(),
                    })
                })
                .transpose()
            }
        }
    }

    pub(crate) async fn list_runtime_config_apply_records(
        &self,
        client_id: Option<&str>,
    ) -> Result<Vec<RuntimeConfigApplyStateRecord>> {
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        state.client_id,
                        applied_version,
                        applied_content_hash,
                        applied_config,
                        applied_job_id,
                        applied_at::text AS applied_at,
                        pending_version,
                        pending_content_hash,
                        pending_config,
                        pending_job_id,
                        pending_reason,
                        pending_status,
                        pending_error,
                        pending_updated_at::text AS pending_updated_at,
                        updated_at::text AS updated_at
                    FROM client_runtime_config_apply_state state
                    JOIN visible_clients client ON client.id = state.client_id
                    WHERE ($1::text IS NULL OR state.client_id = $1)
                    ORDER BY state.client_id
                    "#,
                )
                .bind(client_id)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let applied_version: Option<i64> = row.try_get("applied_version")?;
                        let pending_version: Option<i64> = row.try_get("pending_version")?;
                        let applied_config: Option<sqlx::types::Json<AgentRuntimeConfig>> =
                            row.try_get("applied_config")?;
                        let pending_config: Option<sqlx::types::Json<AgentRuntimeConfig>> =
                            row.try_get("pending_config")?;
                        Ok(RuntimeConfigApplyStateRecord {
                            client_id: row.try_get("client_id")?,
                            applied_version: applied_version.map(|value| value as u64),
                            applied_content_hash: row.try_get("applied_content_hash")?,
                            applied_config: applied_config.map(|config| config.0),
                            applied_job_id: row.try_get("applied_job_id")?,
                            applied_at: row.try_get("applied_at")?,
                            pending_version: pending_version.map(|value| value as u64),
                            pending_content_hash: row.try_get("pending_content_hash")?,
                            pending_config: pending_config.map(|config| config.0),
                            pending_job_id: row.try_get("pending_job_id")?,
                            pending_reason: row.try_get("pending_reason")?,
                            pending_status: row.try_get("pending_status")?,
                            pending_error: row.try_get("pending_error")?,
                            pending_updated_at: row.try_get("pending_updated_at")?,
                            updated_at: row.try_get("updated_at")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn list_runtime_config_apply_states(
        &self,
        client_id: Option<&str>,
    ) -> Result<Vec<RuntimeConfigApplyStateView>> {
        Ok(self
            .list_runtime_config_apply_records(client_id)
            .await?
            .iter()
            .map(RuntimeConfigApplyStateRecord::view)
            .collect())
    }

    pub(crate) async fn runtime_config_pending_state_for_client(
        &self,
        client_id: &str,
    ) -> Result<Option<RuntimeConfigApplyStateView>> {
        Ok(self
            .list_runtime_config_apply_states(Some(client_id))
            .await?
            .into_iter()
            .next()
            .filter(|state| state.pending_status.is_some()))
    }

    pub(crate) async fn promote_runtime_config_apply_from_agent_hash(
        &self,
        client_id: &str,
        content_hash: &str,
    ) -> Result<()> {
        match self {
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    UPDATE client_runtime_config_apply_state
                    SET
                        applied_version = pending_version,
                        applied_content_hash = pending_content_hash,
                        applied_config = pending_config,
                        applied_job_id = pending_job_id,
                        applied_at = now(),
                        pending_version = NULL,
                        pending_content_hash = NULL,
                        pending_config = NULL,
                        pending_job_id = NULL,
                        pending_reason = NULL,
                        pending_status = NULL,
                        pending_error = NULL,
                        pending_updated_at = NULL,
                        updated_at = now()
                    WHERE client_id = $1
                      AND lower(pending_content_hash) = lower($2)
                    "#,
                )
                .bind(client_id)
                .bind(content_hash)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn record_runtime_config_apply_terminal_for_target_status(
        &self,
        job_id: Uuid,
        client_id: &str,
        target_status: &str,
        message: Option<&str>,
    ) -> Result<()> {
        // pending_job_id is the durable relation between a runtime-config apply
        // and its target job. The update helpers below are compare-and-set
        // no-ops for unrelated jobs, so terminal processing does not need to
        // decode jobs.operation (which may be corrupt persisted data).
        if target_status == vpsman_server_core::TARGET_STATUS_COMPLETED {
            self.promote_runtime_config_apply_for_job(job_id, client_id)
                .await?;
        } else {
            let reason = message
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(target_status);
            self.mark_runtime_config_apply_failed_for_job(job_id, client_id, reason)
                .await?;
        }
        Ok(())
    }

    async fn promote_runtime_config_apply_for_job(
        &self,
        job_id: Uuid,
        client_id: &str,
    ) -> Result<()> {
        match self {
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    UPDATE client_runtime_config_apply_state
                    SET
                        applied_version = pending_version,
                        applied_content_hash = pending_content_hash,
                        applied_config = pending_config,
                        applied_job_id = pending_job_id,
                        applied_at = now(),
                        pending_version = NULL,
                        pending_content_hash = NULL,
                        pending_config = NULL,
                        pending_job_id = NULL,
                        pending_reason = NULL,
                        pending_status = NULL,
                        pending_error = NULL,
                        pending_updated_at = NULL,
                        updated_at = now()
                    WHERE client_id = $1
                      AND pending_job_id = $2
                    "#,
                )
                .bind(client_id)
                .bind(job_id)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    async fn mark_runtime_config_apply_failed_for_job(
        &self,
        job_id: Uuid,
        client_id: &str,
        error: &str,
    ) -> Result<()> {
        let error = error.chars().take(4096).collect::<String>();
        match self {
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    UPDATE client_runtime_config_apply_state
                    SET
                        pending_status = 'failed',
                        pending_error = $3,
                        pending_updated_at = now(),
                        updated_at = now()
                    WHERE client_id = $1
                      AND pending_job_id = $2
                    "#,
                )
                .bind(client_id)
                .bind(job_id)
                .bind(error)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn list_runtime_config_overrides(
        &self,
        client_id: Option<&str>,
    ) -> Result<Vec<RuntimeConfigOverrideView>> {
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        override_record.client_id,
                        override_record.toml,
                        override_record.reason,
                        override_record.updated_at::text AS updated_at,
                        override_record.updated_by
                    FROM client_runtime_config_overrides override_record
                    JOIN visible_clients client ON client.id = override_record.client_id
                    WHERE ($1::text IS NULL OR override_record.client_id = $1)
                    ORDER BY override_record.client_id
                    "#,
                )
                .bind(client_id)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(RuntimeConfigOverrideView {
                            client_id: row.try_get("client_id")?,
                            toml: row.try_get("toml")?,
                            reason: row.try_get("reason")?,
                            updated_at: row.try_get("updated_at")?,
                            updated_by: row.try_get("updated_by")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn list_runtime_config_overrides_for_clients(
        &self,
        client_ids: &[String],
    ) -> Result<Vec<RuntimeConfigOverrideView>> {
        if client_ids.is_empty() {
            return Ok(Vec::new());
        }
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        override_record.client_id,
                        override_record.toml,
                        override_record.reason,
                        override_record.updated_at::text AS updated_at,
                        override_record.updated_by
                    FROM client_runtime_config_overrides override_record
                    JOIN visible_clients client ON client.id = override_record.client_id
                    WHERE override_record.client_id = ANY($1::text[])
                    ORDER BY override_record.client_id
                    "#,
                )
                .bind(client_ids)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(RuntimeConfigOverrideView {
                            client_id: row.try_get("client_id")?,
                            toml: row.try_get("toml")?,
                            reason: row.try_get("reason")?,
                            updated_at: row.try_get("updated_at")?,
                            updated_by: row.try_get("updated_by")?,
                        })
                    })
                    .collect()
            }
        }
    }

    /// Serializes a reviewed mutation only with producers for the same VPSes.
    /// The canonical order is exact client lifecycle, visible client identity,
    /// then desired-state owner. Lifecycle-sensitive writers use that order;
    /// other composed-source writers serialize at their BEFORE trigger without
    /// taking a source-row lock after the owner. Reviewed mutations therefore
    /// cannot invert client and desired-state ownership.
    pub(crate) async fn lock_runtime_config_desired_state(
        &self,
        client_ids: &[String],
    ) -> Result<RuntimeConfigDesiredStateGuard> {
        match self {
            Self::Postgres(pool) => {
                let mut client_ids = client_ids.to_vec();
                client_ids.sort();
                client_ids.dedup();
                anyhow::ensure!(!client_ids.is_empty(), "runtime_config_targets_required");
                let mut tx = pool.begin().await?;
                sqlx::query("SET LOCAL lock_timeout = '10s'")
                    .execute(&mut *tx)
                    .await?;
                lock_postgres_client_lifecycles_in_tx(&mut tx, &client_ids).await?;
                let visible = sqlx::query_scalar::<_, String>(
                    r#"
                    SELECT id
                    FROM visible_clients
                    WHERE id = ANY($1::text[])
                    ORDER BY id
                    FOR KEY SHARE
                    "#,
                )
                .bind(&client_ids)
                .fetch_all(&mut *tx)
                .await?;
                anyhow::ensure!(
                    visible.len() == client_ids.len(),
                    "runtime_config_target_no_longer_available"
                );
                sqlx::query(
                    r#"
                    INSERT INTO client_runtime_config_owners (
                        client_id, source_revision, reconciled_revision
                    )
                    SELECT
                        client.id,
                        0,
                        0
                    FROM visible_clients client
                    WHERE client.id = ANY($1::text[])
                    ORDER BY client.id COLLATE "C"
                    ON CONFLICT (client_id) DO NOTHING
                    "#,
                )
                .bind(&client_ids)
                .execute(&mut *tx)
                .await?;
                let locked = sqlx::query_scalar::<_, String>(
                    r#"
                    SELECT client_id
                    FROM client_runtime_config_owners
                    WHERE client_id = ANY($1::text[])
                    ORDER BY client_id
                    FOR UPDATE
                    "#,
                )
                .bind(&client_ids)
                .fetch_all(&mut *tx)
                .await?;
                anyhow::ensure!(
                    locked.len() == client_ids.len(),
                    "runtime_config_target_no_longer_available"
                );
                Ok(RuntimeConfigDesiredStateGuard::Postgres { tx })
            }
        }
    }

    /// Atomically compare-and-replace a set of sparse per-VPS overrides. This
    /// convenience path acquires the same complete desired-state guard used by
    /// the preview/apply routes.
    #[cfg(test)]
    pub(crate) async fn replace_runtime_config_overrides_cas(
        &self,
        replacements: &[RuntimeConfigOverrideReplacement],
        reason: &str,
        operator: &AuthContext,
    ) -> Result<Vec<RuntimeConfigOverrideView>> {
        let client_ids = replacements
            .iter()
            .map(|replacement| replacement.client_id.clone())
            .collect::<Vec<_>>();
        let guard = self.lock_runtime_config_desired_state(&client_ids).await?;
        self.replace_runtime_config_overrides_cas_locked(guard, replacements, reason, operator)
            .await
    }

    /// Commits against the exact desired-state snapshot protected by `guard`.
    /// Callers may safely re-preview after acquiring the guard and before
    /// entering this method; no contributing source can change in between.
    pub(crate) async fn replace_runtime_config_overrides_cas_locked(
        &self,
        guard: RuntimeConfigDesiredStateGuard,
        replacements: &[RuntimeConfigOverrideReplacement],
        reason: &str,
        operator: &AuthContext,
    ) -> Result<Vec<RuntimeConfigOverrideView>> {
        let mut replacements = replacements.to_vec();
        replacements.sort_by(|left, right| left.client_id.cmp(&right.client_id));
        replacements.dedup_by(|left, right| left.client_id == right.client_id);
        let client_ids = replacements
            .iter()
            .map(|replacement| replacement.client_id.clone())
            .collect::<Vec<_>>();
        match guard {
            RuntimeConfigDesiredStateGuard::Postgres { mut tx } => {
                let mutation: Result<Vec<RuntimeConfigOverrideView>> = async {
                    for replacement in &replacements {
                        let current = sqlx::query(
                            r#"
                        SELECT client_id, toml, reason, updated_at::text AS updated_at, updated_by
                        FROM client_runtime_config_overrides
                        WHERE client_id = $1
                        FOR UPDATE
                        "#,
                        )
                        .bind(&replacement.client_id)
                        .fetch_optional(&mut *tx)
                        .await?
                        .map(runtime_config_override_from_row)
                        .transpose()?;
                        anyhow::ensure!(
                            runtime_config_override_revision(current.as_ref())
                                == replacement.expected_revision,
                            "runtime_config_override_review_stale"
                        );
                    }
                    for replacement in &replacements {
                        if let Some(toml) = replacement.toml.as_deref() {
                            sqlx::query(
                                r#"
                            INSERT INTO client_runtime_config_overrides (
                                client_id, toml, reason, updated_by, updated_at
                            )
                            VALUES ($1, $2, $3, $4, now())
                            ON CONFLICT (client_id)
                            DO UPDATE SET
                                toml = EXCLUDED.toml,
                                reason = EXCLUDED.reason,
                                updated_by = EXCLUDED.updated_by,
                                updated_at = now()
                            "#,
                            )
                            .bind(&replacement.client_id)
                            .bind(toml)
                            .bind(reason)
                            .bind(operator.operator.id)
                            .execute(&mut *tx)
                            .await?;
                        } else {
                            sqlx::query(
                                r#"
                                UPDATE client_runtime_config_overrides
                                SET reason = $2, updated_by = $3, updated_at = now()
                                WHERE client_id = $1
                                "#,
                            )
                            .bind(&replacement.client_id)
                            .bind(reason)
                            .bind(operator.operator.id)
                            .execute(&mut *tx)
                            .await?;
                            sqlx::query(
                                "DELETE FROM client_runtime_config_overrides WHERE client_id = $1",
                            )
                            .bind(&replacement.client_id)
                            .execute(&mut *tx)
                            .await?;
                        }
                        sqlx::query(
                            r#"
                        INSERT INTO audit_logs (id, actor_id, action, target, metadata)
                        VALUES ($1, $2, $3, $4, $5)
                        "#,
                        )
                        .bind(Uuid::new_v4())
                        .bind(operator.operator.id)
                        .bind(if replacement.toml.is_some() {
                            "runtime_config.client_override_replaced"
                        } else {
                            "runtime_config.client_override_reset"
                        })
                        .bind(format!("client:{}", replacement.client_id))
                        .bind(runtime_config_override_audit_metadata(
                            &replacement.client_id,
                            reason,
                            operator,
                        ))
                        .execute(&mut *tx)
                        .await?;
                    }
                    let selected = sqlx::query(
                        r#"
                    SELECT client_id, toml, reason, updated_at::text AS updated_at, updated_by
                    FROM client_runtime_config_overrides
                    WHERE client_id = ANY($1::text[])
                    ORDER BY client_id
                    "#,
                    )
                    .bind(&client_ids)
                    .fetch_all(&mut *tx)
                    .await?
                    .into_iter()
                    .map(runtime_config_override_from_row)
                    .collect::<Result<Vec<_>>>()?;
                    Ok(selected)
                }
                .await;
                match mutation {
                    Ok(selected) => {
                        tx.commit().await?;
                        Ok(selected)
                    }
                    Err(error) => {
                        tx.rollback().await?;
                        Err(error)
                    }
                }
            }
        }
    }
}

fn runtime_config_override_from_row(
    row: sqlx::postgres::PgRow,
) -> Result<RuntimeConfigOverrideView> {
    Ok(RuntimeConfigOverrideView {
        client_id: row.try_get("client_id")?,
        toml: row.try_get("toml")?,
        reason: row.try_get("reason")?,
        updated_at: row.try_get("updated_at")?,
        updated_by: row.try_get("updated_by")?,
    })
}

fn runtime_config_override_audit_metadata(
    client_id: &str,
    reason: &str,
    operator: &AuthContext,
) -> serde_json::Value {
    serde_json::json!({
        "client_id": client_id,
        "reason": reason,
        "result": "succeeded",
        "operator_id": operator.operator.id,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "operator_session_id": operator.audit_session_id(),
        "origin_kind": "operator_request",
        "component": "runtime-config-controller",
    })
}

#[cfg(test)]
mod lock_order_tests {
    fn assert_owner_precedes_work(section: &str) {
        let owner = section
            .find("client_runtime_config_owners")
            .expect("runtime-config owner access");
        let work = section
            .find("client_runtime_config_reconcile_work")
            .expect("runtime-config work access");
        assert!(owner < work, "desired-state owner must precede work");
    }

    #[test]
    fn runtime_config_producer_and_completions_share_owner_then_work_order() {
        let source = include_str!("repository_runtime_config.rs");
        let acknowledge = source
            .split_once("pub(crate) async fn acknowledge_if_content_current")
            .expect("no-op completion")
            .1
            .split_once("pub(crate) async fn queue_runtime_config_apply_postgres_in_tx")
            .expect("job completion boundary")
            .0;
        assert_owner_precedes_work(acknowledge);
        assert!(!acknowledge.contains("FOR UPDATE OF work, owner"));

        let queue = source
            .split_once("pub(crate) async fn queue_runtime_config_apply_postgres_in_tx")
            .expect("job completion")
            .1
            .split_once("impl Repository")
            .expect("repository boundary")
            .0;
        assert_owner_precedes_work(queue);

        let schema = include_str!("../../../../../migrations/0009_config_presets_transfers.sql");
        let producer = schema
            .split_once("CREATE FUNCTION public.enqueue_runtime_config_reconcile")
            .expect("runtime-config producer")
            .1
            .split_once("CREATE FUNCTION public.produce_runtime_config_override_reconcile")
            .expect("producer boundary")
            .0;
        assert_owner_precedes_work(producer);
    }

    #[test]
    fn runtime_config_claims_only_deliverable_clients_and_hello_wakes_exact_work() {
        let source = include_str!("repository_runtime_config.rs");
        let claim = source
            .split_once("pub(crate) async fn claim_runtime_config_reconciliation")
            .expect("runtime-config claim")
            .1
            .split_once("pub(crate) async fn list_runtime_config_apply_records")
            .expect("claim boundary")
            .0;
        assert!(claim.contains("client.status <> 'never'"));
        assert!(claim.contains("client.process_incarnation_id IS NOT NULL"));

        let schema = include_str!("../../../../../migrations/0009_config_presets_transfers.sql");
        let lifecycle = schema
            .split_once("CREATE FUNCTION public.maintain_runtime_config_work_for_client_lifecycle")
            .expect("runtime-config lifecycle")
            .1
            .split_once("CREATE TRIGGER client_runtime_config_overrides_reconcile")
            .expect("lifecycle boundary")
            .0;
        assert!(lifecycle
            .contains("OLD.process_incarnation_id IS DISTINCT FROM NEW.process_incarnation_id"));
        assert!(lifecycle.contains("next_attempt_at = LEAST(work.next_attempt_at, now())"));
        assert!(lifecycle.contains("pg_notify('runtime_config_reconcile', 'ready')"));
    }
}
