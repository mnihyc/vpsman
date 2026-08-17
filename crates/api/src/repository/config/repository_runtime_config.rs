use anyhow::Result;
use sqlx::{Postgres, Row, Transaction};
use tokio::sync::OwnedMutexGuard;
use uuid::Uuid;
use vpsman_common::AgentRuntimeConfig;

use crate::runtime_config_workspace::runtime_config_override_revision;
use crate::{
    model::{
        AuditLogView, AuthContext, RuntimeConfigApplyStateRecord, RuntimeConfigApplyStateView,
        RuntimeConfigOverrideReplacement, RuntimeConfigOverrideView,
    },
    repository::{MemoryState, Repository},
    repository_key_lifecycle::require_visible_memory_clients,
    unix_now,
};

pub(crate) enum RuntimeConfigDesiredStateGuard {
    Memory {
        memory: Box<MemoryState>,
        _agent_lifecycle: OwnedMutexGuard<()>,
    },
    Postgres {
        tx: Transaction<'static, Postgres>,
    },
}

pub(crate) async fn queue_runtime_config_apply_memory_state(
    memory: &MemoryState,
    client_id: &str,
    version: u64,
    content_hash: &str,
    config: &AgentRuntimeConfig,
    job_id: Uuid,
    reason: &str,
) {
    let now = unix_now().to_string();
    let reason = reason.chars().take(4096).collect::<String>();
    let mut states = memory.runtime_config_apply_states.write().await;
    if let Some(state) = states.iter_mut().find(|state| state.client_id == client_id) {
        let newest_recorded_version = [state.applied_version, state.pending_version]
            .into_iter()
            .flatten()
            .max()
            .unwrap_or(0);
        if version <= newest_recorded_version {
            return;
        }
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

pub(crate) async fn queue_runtime_config_apply_postgres_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    version: u64,
    content_hash: &str,
    config: &AgentRuntimeConfig,
    job_id: Uuid,
    reason: &str,
) -> Result<()> {
    let reason = reason.chars().take(4096).collect::<String>();
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
    pub(crate) async fn list_runtime_config_apply_records(
        &self,
        client_id: Option<&str>,
    ) -> Result<Vec<RuntimeConfigApplyStateRecord>> {
        match self {
            Self::Memory(memory) => {
                let hidden = memory.hidden_clients.read().await;
                let mut states = memory
                    .runtime_config_apply_states
                    .read()
                    .await
                    .iter()
                    .filter(|state| {
                        !hidden.contains(&state.client_id)
                            && client_id
                                .map(|client_id| state.client_id == client_id)
                                .unwrap_or(true)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                states.sort_by(|left, right| left.client_id.cmp(&right.client_id));
                Ok(states)
            }
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

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn runtime_config_applied_state_for_client(
        &self,
        client_id: &str,
    ) -> Result<Option<(u64, String, AgentRuntimeConfig)>> {
        match self {
            Self::Memory(memory) => {
                if memory.hidden_clients.read().await.contains(client_id) {
                    return Ok(None);
                }
                Ok(memory
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
                    }))
            }
            Self::Postgres(pool) => {
                let Some(row) = sqlx::query(
                    r#"
                    SELECT applied_version, applied_content_hash, applied_config
                    FROM client_runtime_config_apply_state state
                    JOIN visible_clients client ON client.id = state.client_id
                    WHERE state.client_id = $1
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

    #[cfg(test)]
    pub(crate) async fn queue_runtime_config_apply(
        &self,
        client_id: &str,
        version: u64,
        content_hash: &str,
        config: &AgentRuntimeConfig,
        job_id: Uuid,
        reason: &str,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                queue_runtime_config_apply_memory_state(
                    memory,
                    client_id,
                    version,
                    content_hash,
                    config,
                    job_id,
                    reason,
                )
                .await;
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                queue_runtime_config_apply_postgres_in_tx(
                    &mut tx,
                    client_id,
                    version,
                    content_hash,
                    config,
                    job_id,
                    reason,
                )
                .await?;
                tx.commit().await?;
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn mark_runtime_config_apply_job_create_failed(
        &self,
        client_id: &str,
        job_id: Uuid,
        error: &str,
    ) -> Result<()> {
        self.mark_runtime_config_apply_failed_for_job(job_id, client_id, error)
            .await
    }

    pub(crate) async fn promote_runtime_config_apply_from_agent_hash(
        &self,
        client_id: &str,
        content_hash: &str,
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
                        state.client_id == client_id
                            && state
                                .pending_content_hash
                                .as_deref()
                                .is_some_and(|hash| hash.eq_ignore_ascii_case(content_hash))
                    })
                {
                    state.applied_version = state.pending_version;
                    state.applied_content_hash = state.pending_content_hash.clone();
                    state.applied_config = state.pending_config.clone();
                    state.applied_job_id = state.pending_job_id;
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
        // decode jobs.operation (which may be corrupt legacy data).
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
                let hidden = memory.hidden_clients.read().await;
                let mut overrides = memory.runtime_config_overrides.read().await.clone();
                overrides.retain(|override_record| !hidden.contains(&override_record.client_id));
                if let Some(client_id) = client_id {
                    overrides.retain(|override_record| override_record.client_id == client_id);
                }
                overrides.sort_by(|left, right| left.client_id.cmp(&right.client_id));
                Ok(overrides)
            }
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
            Self::Memory(memory) => {
                let selected = client_ids
                    .iter()
                    .map(String::as_str)
                    .collect::<std::collections::BTreeSet<_>>();
                let hidden = memory.hidden_clients.read().await;
                let mut overrides = memory
                    .runtime_config_overrides
                    .read()
                    .await
                    .iter()
                    .filter(|record| {
                        selected.contains(record.client_id.as_str())
                            && !hidden.contains(&record.client_id)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                overrides.sort_by(|left, right| left.client_id.cmp(&right.client_id));
                Ok(overrides)
            }
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

    /// Holds every desired-config input stable while a reviewed override is
    /// re-previewed and committed. Reads continue; preset/Ping/client changes,
    /// tunnel or adapter changes, and port-forward changes wait for the guard.
    pub(crate) async fn lock_runtime_config_desired_state(
        &self,
    ) -> Result<RuntimeConfigDesiredStateGuard> {
        match self {
            Self::Memory(memory) => {
                let agent_lifecycle = memory.agent_key_lifecycle.clone().lock_owned().await;
                Ok(RuntimeConfigDesiredStateGuard::Memory {
                    memory: Box::new(memory.clone()),
                    _agent_lifecycle: agent_lifecycle,
                })
            }
            Self::Postgres(pool) => {
                anyhow::ensure!(
                    pool.options().get_max_connections() >= 2,
                    "runtime_config_desired_state_pool_capacity_too_small"
                );
                let mut tx = pool.begin().await?;
                sqlx::query("SET LOCAL lock_timeout = '10s'")
                    .execute(&mut *tx)
                    .await?;
                let lifecycle_locked = sqlx::query_scalar::<_, bool>(
                    "SELECT pg_try_advisory_xact_lock(hashtext('vpsman.agent_key_lifecycle'))",
                )
                .fetch_one(&mut *tx)
                .await?;
                anyhow::ensure!(lifecycle_locked, "runtime_config_desired_state_busy");
                sqlx::query(
                    "LOCK TABLE tunnel_plans, network_adapter_definitions, port_forward_rules IN SHARE MODE",
                )
                .execute(&mut *tx)
                .await?;
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
        let guard = self.lock_runtime_config_desired_state().await?;
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
            RuntimeConfigDesiredStateGuard::Memory {
                memory,
                _agent_lifecycle,
            } => {
                require_visible_memory_clients(
                    &memory,
                    &client_ids,
                    "runtime_config_target_no_longer_available",
                )
                .await?;
                let mut overrides = memory.runtime_config_overrides.write().await;
                for replacement in &replacements {
                    let current = overrides
                        .iter()
                        .find(|record| record.client_id == replacement.client_id);
                    anyhow::ensure!(
                        runtime_config_override_revision(current) == replacement.expected_revision,
                        "runtime_config_override_review_stale"
                    );
                }
                let now = unix_now().to_string();
                for replacement in &replacements {
                    match replacement.toml.as_deref() {
                        Some(toml) => {
                            if let Some(existing) = overrides
                                .iter_mut()
                                .find(|record| record.client_id == replacement.client_id)
                            {
                                existing.toml = toml.to_string();
                                existing.reason = reason.to_string();
                                existing.updated_at = now.clone();
                                existing.updated_by = Some(operator.operator.id);
                            } else {
                                overrides.push(RuntimeConfigOverrideView {
                                    client_id: replacement.client_id.clone(),
                                    toml: toml.to_string(),
                                    reason: reason.to_string(),
                                    updated_at: now.clone(),
                                    updated_by: Some(operator.operator.id),
                                });
                            }
                        }
                        None => {
                            overrides.retain(|record| record.client_id != replacement.client_id)
                        }
                    }
                }
                let selected = overrides
                    .iter()
                    .filter(|record| client_ids.contains(&record.client_id))
                    .cloned()
                    .collect::<Vec<_>>();
                drop(overrides);
                let mut audits = memory.audits.write().await;
                for replacement in &replacements {
                    audits.push(AuditLogView {
                        id: Uuid::new_v4(),
                        actor_id: Some(operator.operator.id),
                        action: if replacement.toml.is_some() {
                            "runtime_config.client_override_replaced".to_string()
                        } else {
                            "runtime_config.client_override_reset".to_string()
                        },
                        target: format!("client:{}", replacement.client_id),
                        command_hash: None,
                        metadata: runtime_config_override_audit_metadata(
                            &replacement.client_id,
                            reason,
                            operator,
                        ),
                        created_at: now.clone(),
                    });
                }
                drop(_agent_lifecycle);
                Ok(selected)
            }
            RuntimeConfigDesiredStateGuard::Postgres { mut tx } => {
                let mutation: Result<Vec<RuntimeConfigOverrideView>> = async {
                    for client_id in &client_ids {
                        let visible = sqlx::query_scalar::<_, String>(
                            "SELECT id FROM clients WHERE id = $1 AND hidden_at IS NULL FOR UPDATE",
                        )
                        .bind(client_id)
                        .fetch_optional(&mut *tx)
                        .await?;
                        anyhow::ensure!(
                            visible.is_some(),
                            "runtime_config_target_no_longer_available"
                        );
                    }
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
