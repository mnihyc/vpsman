use anyhow::Result;
use sqlx::Row;
use uuid::Uuid;
use vpsman_common::AgentRuntimeConfig;

use crate::{
    model::{
        AuthContext, RuntimeConfigApplyStateRecord, RuntimeConfigApplyStateView,
        RuntimeConfigOverrideView,
    },
    repository::Repository,
    unix_now,
};

impl Repository {
    pub(crate) async fn list_runtime_config_apply_states(
        &self,
        client_id: Option<&str>,
    ) -> Result<Vec<RuntimeConfigApplyStateView>> {
        match self {
            Self::Memory(memory) => {
                let mut states = memory
                    .runtime_config_apply_states
                    .read()
                    .await
                    .iter()
                    .filter(|state| {
                        client_id
                            .map(|client_id| state.client_id == client_id)
                            .unwrap_or(true)
                    })
                    .map(RuntimeConfigApplyStateRecord::view)
                    .collect::<Vec<_>>();
                states.sort_by(|left, right| left.client_id.cmp(&right.client_id));
                Ok(states)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        client_id,
                        applied_version,
                        applied_content_hash,
                        applied_job_id,
                        applied_at::text AS applied_at,
                        pending_version,
                        pending_content_hash,
                        pending_job_id,
                        pending_reason,
                        pending_status,
                        pending_error,
                        pending_updated_at::text AS pending_updated_at,
                        updated_at::text AS updated_at
                    FROM client_runtime_config_apply_state
                    WHERE ($1::text IS NULL OR client_id = $1)
                    ORDER BY client_id
                    "#,
                )
                .bind(client_id)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let applied_version: Option<i64> = row.try_get("applied_version")?;
                        let pending_version: Option<i64> = row.try_get("pending_version")?;
                        Ok(RuntimeConfigApplyStateView {
                            client_id: row.try_get("client_id")?,
                            applied_version: applied_version.map(|value| value as u64),
                            applied_content_hash: row.try_get("applied_content_hash")?,
                            applied_job_id: row.try_get("applied_job_id")?,
                            applied_at: row.try_get("applied_at")?,
                            pending_version: pending_version.map(|value| value as u64),
                            pending_content_hash: row.try_get("pending_content_hash")?,
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

    pub(crate) async fn runtime_config_applied_state_for_client(
        &self,
        client_id: &str,
    ) -> Result<Option<(u64, String, AgentRuntimeConfig)>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .runtime_config_apply_states
                .read()
                .await
                .iter()
                .find(|state| state.client_id == client_id)
                .and_then(|state| {
                    Some((
                        state.applied_version?,
                        state.applied_content_hash.clone()?,
                        state.applied_config.clone()?,
                    ))
                })),
            Self::Postgres(pool) => {
                let Some(row) = sqlx::query(
                    r#"
                    SELECT applied_version, applied_content_hash, applied_config
                    FROM client_runtime_config_apply_state
                    WHERE client_id = $1
                      AND applied_version IS NOT NULL
                      AND applied_content_hash IS NOT NULL
                      AND applied_config IS NOT NULL
                    "#,
                )
                .bind(client_id)
                .fetch_optional(pool)
                .await?
                else {
                    return Ok(None);
                };
                let version: i64 = row.try_get("applied_version")?;
                let hash: String = row.try_get("applied_content_hash")?;
                let config: sqlx::types::Json<AgentRuntimeConfig> =
                    row.try_get("applied_config")?;
                Ok(Some((version as u64, hash, config.0)))
            }
        }
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

    pub(crate) async fn queue_runtime_config_apply(
        &self,
        client_id: &str,
        version: u64,
        content_hash: &str,
        config: &AgentRuntimeConfig,
        job_id: Uuid,
        reason: &str,
    ) -> Result<()> {
        let reason = reason.chars().take(4096).collect::<String>();
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut states = memory.runtime_config_apply_states.write().await;
                if let Some(state) = states.iter_mut().find(|state| state.client_id == client_id) {
                    state.pending_version = Some(version);
                    state.pending_content_hash = Some(content_hash.to_string());
                    state.pending_config = Some(config.clone());
                    state.pending_job_id = Some(job_id);
                    state.pending_reason = Some(reason);
                    state.pending_status = Some("queued".to_string());
                    state.pending_error = None;
                    state.pending_updated_at = Some(now.clone());
                    state.updated_at = now;
                } else {
                    states.push(RuntimeConfigApplyStateRecord {
                        client_id: client_id.to_string(),
                        applied_version: None,
                        applied_content_hash: None,
                        applied_config: None,
                        applied_job_id: None,
                        applied_at: None,
                        pending_version: Some(version),
                        pending_content_hash: Some(content_hash.to_string()),
                        pending_config: Some(config.clone()),
                        pending_job_id: Some(job_id),
                        pending_reason: Some(reason),
                        pending_status: Some("queued".to_string()),
                        pending_error: None,
                        pending_updated_at: Some(now.clone()),
                        updated_at: now,
                    });
                }
            }
            Self::Postgres(pool) => {
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
                    "#,
                )
                .bind(client_id)
                .bind(version as i64)
                .bind(content_hash)
                .bind(sqlx::types::Json(config.clone()))
                .bind(job_id)
                .bind(reason)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn mark_runtime_config_apply_job_create_failed(
        &self,
        client_id: &str,
        job_id: Uuid,
        error: &str,
    ) -> Result<()> {
        self.mark_runtime_config_apply_failed_for_job(job_id, client_id, error)
            .await
    }

    pub(crate) async fn record_runtime_config_apply_terminal_for_target_status(
        &self,
        job_id: Uuid,
        client_id: &str,
        target_status: &str,
        message: Option<&str>,
    ) -> Result<()> {
        let Some(operation) = self.job_operation(job_id).await? else {
            return Ok(());
        };
        if !matches!(
            operation,
            vpsman_common::JobCommand::RuntimeConfigSync { .. }
        ) {
            return Ok(());
        }
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
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                if let Some(state) = memory
                    .runtime_config_apply_states
                    .write()
                    .await
                    .iter_mut()
                    .find(|state| {
                        state.client_id == client_id && state.pending_job_id == Some(job_id)
                    })
                {
                    state.applied_version = state.pending_version;
                    state.applied_content_hash = state.pending_content_hash.clone();
                    state.applied_config = state.pending_config.clone();
                    state.applied_job_id = Some(job_id);
                    state.applied_at = Some(now.clone());
                    state.pending_version = None;
                    state.pending_content_hash = None;
                    state.pending_config = None;
                    state.pending_job_id = None;
                    state.pending_reason = None;
                    state.pending_status = None;
                    state.pending_error = None;
                    state.pending_updated_at = None;
                    state.updated_at = now;
                }
            }
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
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                if let Some(state) = memory
                    .runtime_config_apply_states
                    .write()
                    .await
                    .iter_mut()
                    .find(|state| {
                        state.client_id == client_id && state.pending_job_id == Some(job_id)
                    })
                {
                    state.pending_status = Some("failed".to_string());
                    state.pending_error = Some(error);
                    state.pending_updated_at = Some(now.clone());
                    state.updated_at = now;
                }
            }
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
            Self::Memory(memory) => {
                let mut overrides = memory.runtime_config_overrides.read().await.clone();
                if let Some(client_id) = client_id {
                    overrides.retain(|override_record| override_record.client_id == client_id);
                }
                overrides.sort_by(|left, right| left.client_id.cmp(&right.client_id));
                Ok(overrides)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT client_id, toml, reason, updated_at::text AS updated_at, updated_by
                    FROM client_runtime_config_overrides
                    WHERE ($1::text IS NULL OR client_id = $1)
                    ORDER BY client_id
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

    pub(crate) async fn upsert_runtime_config_overrides(
        &self,
        client_ids: &[String],
        toml: &str,
        reason: &str,
        operator: &AuthContext,
    ) -> Result<Vec<RuntimeConfigOverrideView>> {
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut overrides = memory.runtime_config_overrides.write().await;
                for client_id in client_ids {
                    if let Some(existing) = overrides
                        .iter_mut()
                        .find(|override_record| override_record.client_id == *client_id)
                    {
                        existing.toml = toml.to_string();
                        existing.reason = reason.to_string();
                        existing.updated_at = now.clone();
                        existing.updated_by = Some(operator.operator.id);
                    } else {
                        overrides.push(RuntimeConfigOverrideView {
                            client_id: client_id.clone(),
                            toml: toml.to_string(),
                            reason: reason.to_string(),
                            updated_at: now.clone(),
                            updated_by: Some(operator.operator.id),
                        });
                    }
                }
                Ok(overrides
                    .iter()
                    .filter(|override_record| client_ids.contains(&override_record.client_id))
                    .cloned()
                    .collect())
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                for client_id in client_ids {
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
                    .bind(client_id)
                    .bind(toml)
                    .bind(reason)
                    .bind(operator.operator.id)
                    .execute(&mut *tx)
                    .await?;
                    sqlx::query(
                        r#"
                        INSERT INTO audit_logs (id, actor_id, action, target, metadata)
                        VALUES ($1, $2, 'runtime_config.client_patch_upserted', $3, $4)
                        "#,
                    )
                    .bind(Uuid::new_v4())
                    .bind(operator.operator.id)
                    .bind(format!("client:{client_id}"))
                    .bind(serde_json::json!({
                        "client_id": client_id,
                        "reason": reason,
                    }))
                    .execute(&mut *tx)
                    .await?;
                }
                tx.commit().await?;
                self.list_runtime_config_overrides(None)
                    .await
                    .map(|records| {
                        records
                            .into_iter()
                            .filter(|record| client_ids.contains(&record.client_id))
                            .collect()
                    })
            }
        }
    }
}
