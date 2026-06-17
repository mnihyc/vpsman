use std::cmp::Reverse;

use anyhow::{ensure, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    model::{AuditLogView, AuthContext, BackupRequestStatus},
    model_history::{
        HistoryDomain, HistoryRetentionPolicyView, HistoryRetentionPruneOutcome,
        HistoryRetentionPrunePlan, UpsertHistoryRetentionPolicyRequest,
    },
    repository::Repository,
    unix_now,
};

impl Repository {
    pub(crate) async fn list_history_retention_policies(
        &self,
    ) -> Result<Vec<HistoryRetentionPolicyView>> {
        let mut policies = HistoryDomain::ALL
            .iter()
            .copied()
            .map(default_policy)
            .collect::<Vec<_>>();
        let stored = match self {
            Self::Memory(memory) => memory.history_retention_policies.read().await.clone(),
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        domain,
                        retention_days,
                        prune_limit,
                        enabled,
                        metadata_only,
                        export_enabled,
                        notes,
                        updated_by,
                        updated_at::text AS updated_at
                    FROM history_retention_policies
                    ORDER BY domain ASC
                    "#,
                )
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(history_retention_policy_from_row)
                    .collect::<Result<Vec<_>>>()?
            }
        };
        for stored_policy in stored {
            if let Some(policy) = policies
                .iter_mut()
                .find(|policy| policy.domain == stored_policy.domain)
            {
                *policy = stored_policy;
            }
        }
        policies.sort_by(|left, right| left.domain.cmp(&right.domain));
        Ok(policies)
    }

    pub(crate) async fn upsert_history_retention_policy(
        &self,
        request: UpsertHistoryRetentionPolicyRequest,
        operator: &AuthContext,
    ) -> Result<HistoryRetentionPolicyView> {
        ensure!(
            request.confirmed,
            "history_retention_update_requires_confirmation"
        );
        let domain = HistoryDomain::from_str(&request.domain)
            .ok_or_else(|| anyhow::anyhow!("invalid_history_retention_domain"))?;
        let mut policy = self
            .list_history_retention_policies()
            .await?
            .into_iter()
            .find(|policy| policy.domain == domain.as_str())
            .unwrap_or_else(|| default_policy(domain));
        if let Some(retention_days) = request.retention_days {
            ensure!(
                (1..=3650).contains(&retention_days),
                "history_retention_days_out_of_range"
            );
            policy.retention_days = retention_days;
        }
        if let Some(prune_limit) = request.prune_limit {
            ensure!(
                (1..=100_000).contains(&prune_limit),
                "history_prune_limit_out_of_range"
            );
            policy.prune_limit = prune_limit;
        }
        if let Some(enabled) = request.enabled {
            policy.enabled = enabled;
        }
        if let Some(metadata_only) = request.metadata_only {
            policy.metadata_only = metadata_only;
        }
        if let Some(export_enabled) = request.export_enabled {
            policy.export_enabled = export_enabled;
        }
        if request.clear_notes {
            policy.notes = None;
        } else if let Some(notes) = request.notes {
            let notes = notes.trim().to_string();
            ensure!(notes.len() <= 1000, "history_retention_notes_too_long");
            policy.notes = (!notes.is_empty()).then_some(notes);
        }
        policy.updated_by = Some(operator.operator.id);
        policy.updated_at = unix_now().to_string();
        policy.built_in_default = false;

        match self {
            Self::Memory(memory) => {
                let mut policies = memory.history_retention_policies.write().await;
                if let Some(existing) = policies
                    .iter_mut()
                    .find(|stored| stored.domain == policy.domain)
                {
                    *existing = policy.clone();
                } else {
                    policies.push(policy.clone());
                }
                memory.audits.write().await.push(history_retention_audit(
                    "history_retention.policy_updated",
                    &policy.domain,
                    operator,
                    json!({
                        "domain": &policy.domain,
                        "retention_days": policy.retention_days,
                        "prune_limit": policy.prune_limit,
                        "enabled": policy.enabled,
                        "metadata_only": policy.metadata_only,
                        "export_enabled": policy.export_enabled,
                        "operator_username": &operator.operator.username,
                        "operator_role": &operator.operator.role,
                        "session_id": operator.session_id,
                    }),
                    policy.updated_at.clone(),
                ));
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    INSERT INTO history_retention_policies (
                        domain,
                        retention_days,
                        prune_limit,
                        enabled,
                        metadata_only,
                        export_enabled,
                        notes,
                        updated_by
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                    ON CONFLICT (domain)
                    DO UPDATE SET
                        retention_days = EXCLUDED.retention_days,
                        prune_limit = EXCLUDED.prune_limit,
                        enabled = EXCLUDED.enabled,
                        metadata_only = EXCLUDED.metadata_only,
                        export_enabled = EXCLUDED.export_enabled,
                        notes = EXCLUDED.notes,
                        updated_by = EXCLUDED.updated_by,
                        updated_at = now()
                    RETURNING updated_at::text AS updated_at
                    "#,
                )
                .bind(&policy.domain)
                .bind(policy.retention_days)
                .bind(policy.prune_limit)
                .bind(policy.enabled)
                .bind(policy.metadata_only)
                .bind(policy.export_enabled)
                .bind(&policy.notes)
                .bind(operator.operator.id)
                .fetch_one(pool)
                .await?;
                policy.updated_at = row.try_get("updated_at")?;
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
                .bind("history_retention.policy_updated")
                .bind(format!("history_retention:{}", policy.domain))
                .bind(Option::<String>::None)
                .bind(json!({
                    "domain": &policy.domain,
                    "retention_days": policy.retention_days,
                    "prune_limit": policy.prune_limit,
                    "enabled": policy.enabled,
                    "metadata_only": policy.metadata_only,
                    "export_enabled": policy.export_enabled,
                    "operator_username": &operator.operator.username,
                    "operator_role": &operator.operator.role,
                    "session_id": operator.session_id,
                }))
                .execute(pool)
                .await?;
            }
        }
        Ok(policy)
    }

    pub(crate) async fn prune_history_domain(
        &self,
        plan: &HistoryRetentionPrunePlan,
        cutoff_unix: u64,
        dry_run: bool,
    ) -> Result<HistoryRetentionPruneOutcome> {
        if !plan.enabled {
            return Ok(HistoryRetentionPruneOutcome {
                matched_rows: 0,
                pruned_rows: 0,
                object_keys: Vec::new(),
            });
        }
        match self {
            Self::Memory(memory) => {
                let limit = plan.prune_limit as usize;
                match plan.domain {
                    HistoryDomain::AuditLogs => {
                        let matched_rows =
                            prune_memory_vec(&memory.audits, cutoff_unix, limit, dry_run, |row| {
                                &row.created_at
                            })
                            .await?;
                        Ok(HistoryRetentionPruneOutcome {
                            matched_rows,
                            pruned_rows: if dry_run { 0 } else { matched_rows },
                            object_keys: Vec::new(),
                        })
                    }
                    HistoryDomain::TelemetryRollups => {
                        let matched_rows = prune_memory_vec(
                            &memory.telemetry_rollups,
                            cutoff_unix,
                            limit,
                            dry_run,
                            |row| &row.bucket_start,
                        )
                        .await?;
                        Ok(HistoryRetentionPruneOutcome {
                            matched_rows,
                            pruned_rows: if dry_run { 0 } else { matched_rows },
                            object_keys: Vec::new(),
                        })
                    }
                    HistoryDomain::SystemMetricRollups => {
                        let matched_rows = prune_memory_vec(
                            &memory.system_metric_rollups,
                            cutoff_unix,
                            limit,
                            dry_run,
                            |row| &row.bucket_start,
                        )
                        .await?;
                        Ok(HistoryRetentionPruneOutcome {
                            matched_rows,
                            pruned_rows: if dry_run { 0 } else { matched_rows },
                            object_keys: Vec::new(),
                        })
                    }
                    HistoryDomain::JobOutputs => {
                        let mut rows = memory.job_outputs.write().await;
                        let mut matched_indices = rows
                            .iter()
                            .enumerate()
                            .filter(|(_, row)| timestamp_before(&row.created_at, cutoff_unix))
                            .map(|(index, row)| (index, row.artifact_object_key.clone()))
                            .take(limit)
                            .collect::<Vec<_>>();
                        let object_keys = matched_indices
                            .iter()
                            .filter_map(|(_, object_key)| object_key.clone())
                            .collect::<Vec<_>>();
                        let matched_rows = matched_indices.len() as i64;
                        if !dry_run {
                            matched_indices.sort_unstable_by_key(|(index, _)| Reverse(*index));
                            for (index, _) in matched_indices {
                                rows.remove(index);
                            }
                        }
                        Ok(HistoryRetentionPruneOutcome {
                            matched_rows,
                            pruned_rows: if dry_run { 0 } else { matched_rows },
                            object_keys,
                        })
                    }
                    HistoryDomain::BackupArtifacts => {
                        let mut rows = memory.backup_artifacts.write().await;
                        let mut matched_indices = rows
                            .iter()
                            .enumerate()
                            .filter(|(_, row)| timestamp_before(&row.created_at, cutoff_unix))
                            .map(|(index, row)| (index, row.object_key.clone()))
                            .take(limit)
                            .collect::<Vec<_>>();
                        let object_keys = matched_indices
                            .iter()
                            .map(|(_, object_key)| object_key.clone())
                            .collect::<Vec<_>>();
                        let matched_rows = matched_indices.len() as i64;
                        if !dry_run {
                            matched_indices.sort_unstable_by_key(|(index, _)| Reverse(*index));
                            for (index, _) in matched_indices {
                                rows.remove(index);
                            }
                        }
                        Ok(HistoryRetentionPruneOutcome {
                            matched_rows,
                            pruned_rows: if dry_run { 0 } else { matched_rows },
                            object_keys,
                        })
                    }
                    HistoryDomain::NetworkObservations => {
                        let matched_rows = prune_memory_vec(
                            &memory.network_observations,
                            cutoff_unix,
                            limit,
                            dry_run,
                            |row| &row.observed_at,
                        )
                        .await?;
                        Ok(HistoryRetentionPruneOutcome {
                            matched_rows,
                            pruned_rows: if dry_run { 0 } else { matched_rows },
                            object_keys: Vec::new(),
                        })
                    }
                    HistoryDomain::TopologyHistory => {
                        let mut rows = memory.network_observations.write().await;
                        let mut matched_indices = rows
                            .iter()
                            .enumerate()
                            .filter(|(_, row)| {
                                (row.plan_name.is_some() || row.interface_name.is_some())
                                    && timestamp_before(&row.observed_at, cutoff_unix)
                            })
                            .map(|(index, _)| index)
                            .take(limit)
                            .collect::<Vec<_>>();
                        let matched_rows = matched_indices.len() as i64;
                        if !dry_run {
                            matched_indices.sort_unstable_by_key(|index| Reverse(*index));
                            for index in matched_indices {
                                rows.remove(index);
                            }
                        }
                        Ok(HistoryRetentionPruneOutcome {
                            matched_rows,
                            pruned_rows: if dry_run { 0 } else { matched_rows },
                            object_keys: Vec::new(),
                        })
                    }
                }
            }
            Self::Postgres(pool) => {
                prune_postgres_history_domain(
                    pool,
                    plan.domain,
                    cutoff_unix,
                    plan.prune_limit,
                    dry_run,
                )
                .await
            }
        }
    }

    pub(crate) async fn list_history_retention_object_candidates(
        &self,
        plan: &HistoryRetentionPrunePlan,
        cutoff_unix: u64,
    ) -> Result<Vec<HistoryRetentionObjectCandidate>> {
        if !plan.enabled {
            return Ok(Vec::new());
        }
        let limit = plan.prune_limit as usize;
        match self {
            Self::Memory(memory) => match plan.domain {
                HistoryDomain::JobOutputs => {
                    let rows = memory.job_outputs.read().await;
                    Ok(rows
                        .iter()
                        .filter(|row| timestamp_before(&row.created_at, cutoff_unix))
                        .take(limit)
                        .map(|row| HistoryRetentionObjectCandidate::JobOutput {
                            job_id: row.job_id,
                            client_id: row.client_id.clone(),
                            seq: row.seq,
                            object_key: row.artifact_object_key.clone(),
                        })
                        .collect())
                }
                HistoryDomain::BackupArtifacts => {
                    let rows = memory.backup_artifacts.read().await;
                    Ok(rows
                        .iter()
                        .filter(|row| timestamp_before(&row.created_at, cutoff_unix))
                        .take(limit)
                        .map(|row| HistoryRetentionObjectCandidate::BackupArtifact {
                            artifact_id: row.id,
                            object_key: row.object_key.clone(),
                        })
                        .collect())
                }
                _ => Ok(Vec::new()),
            },
            Self::Postgres(pool) => {
                list_postgres_history_retention_object_candidates(
                    pool,
                    plan.domain,
                    cutoff_unix,
                    plan.prune_limit,
                )
                .await
            }
        }
    }

    pub(crate) async fn prune_history_retention_object_candidate(
        &self,
        candidate: &HistoryRetentionObjectCandidate,
    ) -> Result<i64> {
        match self {
            Self::Memory(memory) => match candidate {
                HistoryRetentionObjectCandidate::JobOutput {
                    job_id,
                    client_id,
                    seq,
                    ..
                } => {
                    let mut rows = memory.job_outputs.write().await;
                    let before = rows.len();
                    rows.retain(|row| {
                        row.job_id != *job_id || row.client_id != *client_id || row.seq != *seq
                    });
                    Ok((before.saturating_sub(rows.len())) as i64)
                }
                HistoryRetentionObjectCandidate::BackupArtifact { artifact_id, .. } => {
                    {
                        let mut requests = memory.backup_requests.write().await;
                        for request in requests
                            .iter_mut()
                            .filter(|request| request.artifact_id == Some(*artifact_id))
                        {
                            request.artifact_id = None;
                            request.status = BackupRequestStatus::RequestedMetadataOnly
                                .as_str()
                                .to_string();
                        }
                    }
                    let mut rows = memory.backup_artifacts.write().await;
                    let before = rows.len();
                    rows.retain(|row| row.id != *artifact_id);
                    Ok((before.saturating_sub(rows.len())) as i64)
                }
            },
            Self::Postgres(pool) => {
                prune_postgres_history_retention_object_candidate(pool, candidate).await
            }
        }
    }

    pub(crate) async fn prune_history_retention_object_candidates(
        &self,
        candidates: &[HistoryRetentionObjectCandidate],
    ) -> Result<i64> {
        let mut pruned_rows = 0_i64;
        for candidate in candidates {
            pruned_rows += self
                .prune_history_retention_object_candidate(candidate)
                .await?;
        }
        Ok(pruned_rows)
    }

    pub(crate) async fn record_history_retention_prune_audit(
        &self,
        operator: &AuthContext,
        dry_run: bool,
        metadata_only: Option<bool>,
        domains: &[serde_json::Value],
    ) -> Result<()> {
        let now = unix_now().to_string();
        let metadata = json!({
            "dry_run": dry_run,
            "metadata_only_requested": metadata_only,
            "domains": domains,
            "operator_username": &operator.operator.username,
            "operator_role": &operator.operator.role,
            "session_id": operator.session_id,
        });
        match self {
            Self::Memory(memory) => {
                memory.audits.write().await.push(history_retention_audit(
                    "history_retention.pruned",
                    "history_retention",
                    operator,
                    metadata,
                    now,
                ));
            }
            Self::Postgres(pool) => {
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
                .bind("history_retention.pruned")
                .bind("history_retention")
                .bind(Option::<String>::None)
                .bind(metadata)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn export_job_outputs(
        &self,
        limit: i64,
        client_id: Option<&str>,
        job_id: Option<Uuid>,
    ) -> Result<Vec<serde_json::Value>> {
        match self {
            Self::Memory(memory) => {
                let mut rows = memory
                    .job_outputs
                    .read()
                    .await
                    .iter()
                    .filter(|row| client_id.is_none_or(|expected| row.client_id == expected))
                    .filter(|row| job_id.is_none_or(|expected| row.job_id == expected))
                    .map(|row| {
                        json!({
                            "job_id": row.job_id,
                            "client_id": &row.client_id,
                            "seq": row.seq,
                            "stream": &row.stream,
                            "data_base64": &row.data_base64,
                            "storage": &row.storage,
                            "artifact_object_key": &row.artifact_object_key,
                            "artifact_sha256_hex": &row.artifact_sha256_hex,
                            "artifact_size_bytes": row.artifact_size_bytes,
                            "exit_code": row.exit_code,
                            "done": row.done,
                            "received_at": &row.received_at,
                            "created_at": &row.created_at,
                        })
                    })
                    .collect::<Vec<_>>();
                rows.truncate(limit.clamp(1, 200) as usize);
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        job_id,
                        client_id,
                        seq,
                        stream,
                        data,
                        storage,
                        object_key,
                        data_sha256_hex,
                        data_size_bytes,
                        exit_code,
                        done,
                        received_at::text AS received_at,
                        created_at::text AS created_at
                    FROM job_outputs
                    WHERE ($1::TEXT IS NULL OR client_id = $1)
                      AND ($2::UUID IS NULL OR job_id = $2)
                    ORDER BY created_at DESC, job_id DESC, client_id ASC, seq ASC
                    LIMIT $3
                    "#,
                )
                .bind(client_id)
                .bind(job_id)
                .bind(limit.clamp(1, 200))
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let data: Vec<u8> = row.try_get("data")?;
                        Ok(json!({
                            "job_id": row.try_get::<Uuid, _>("job_id")?,
                            "client_id": row.try_get::<String, _>("client_id")?,
                            "seq": row.try_get::<i32, _>("seq")?,
                            "stream": row.try_get::<String, _>("stream")?,
                            "data_base64": BASE64.encode(data),
                            "storage": row.try_get::<String, _>("storage")?,
                            "artifact_object_key": row.try_get::<Option<String>, _>("object_key")?,
                            "artifact_sha256_hex": row.try_get::<Option<String>, _>("data_sha256_hex")?,
                            "artifact_size_bytes": row.try_get::<Option<i64>, _>("data_size_bytes")?,
                            "exit_code": row.try_get::<Option<i32>, _>("exit_code")?,
                            "done": row.try_get::<bool, _>("done")?,
                            "received_at": row.try_get::<Option<String>, _>("received_at")?,
                            "created_at": row.try_get::<String, _>("created_at")?,
                        }))
                    })
                    .collect()
            }
        }
    }
}

fn default_policy(domain: HistoryDomain) -> HistoryRetentionPolicyView {
    HistoryRetentionPolicyView {
        domain: domain.as_str().to_string(),
        retention_days: domain.default_retention_days(),
        prune_limit: domain.default_prune_limit(),
        enabled: true,
        metadata_only: false,
        export_enabled: true,
        notes: None,
        updated_by: None,
        updated_at: unix_now().to_string(),
        built_in_default: true,
    }
}

fn history_retention_policy_from_row(
    row: sqlx::postgres::PgRow,
) -> Result<HistoryRetentionPolicyView> {
    Ok(HistoryRetentionPolicyView {
        domain: row.try_get("domain")?,
        retention_days: row.try_get("retention_days")?,
        prune_limit: row.try_get("prune_limit")?,
        enabled: row.try_get("enabled")?,
        metadata_only: row.try_get("metadata_only")?,
        export_enabled: row.try_get("export_enabled")?,
        notes: row.try_get("notes")?,
        updated_by: row.try_get("updated_by")?,
        updated_at: row.try_get("updated_at")?,
        built_in_default: false,
    })
}

#[derive(Clone, Debug)]
pub(crate) enum HistoryRetentionObjectCandidate {
    JobOutput {
        job_id: Uuid,
        client_id: String,
        seq: i32,
        object_key: Option<String>,
    },
    BackupArtifact {
        artifact_id: Uuid,
        object_key: String,
    },
}

impl HistoryRetentionObjectCandidate {
    pub(crate) fn object_key(&self) -> Option<&str> {
        match self {
            Self::JobOutput { object_key, .. } => object_key.as_deref(),
            Self::BackupArtifact { object_key, .. } => Some(object_key),
        }
    }
}

async fn prune_memory_vec<T>(
    rows: &tokio::sync::RwLock<Vec<T>>,
    cutoff_unix: u64,
    limit: usize,
    dry_run: bool,
    timestamp: impl Fn(&T) -> &str,
) -> Result<i64> {
    let mut rows = rows.write().await;
    let mut matched_indices = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| timestamp_before(timestamp(row), cutoff_unix))
        .map(|(index, _)| index)
        .take(limit)
        .collect::<Vec<_>>();
    let matched_rows = matched_indices.len();
    if !dry_run {
        matched_indices.sort_unstable_by(|left, right| right.cmp(left));
        for index in matched_indices {
            rows.remove(index);
        }
    }
    Ok(matched_rows as i64)
}

async fn list_postgres_history_retention_object_candidates(
    pool: &sqlx::PgPool,
    domain: HistoryDomain,
    cutoff_unix: u64,
    limit: i32,
) -> Result<Vec<HistoryRetentionObjectCandidate>> {
    match domain {
        HistoryDomain::JobOutputs => {
            let rows = sqlx::query(
                r#"
                SELECT job_id, client_id, seq, object_key
                FROM job_outputs
                WHERE created_at < to_timestamp($1)
                ORDER BY created_at ASC, job_id ASC, client_id ASC, seq ASC
                LIMIT $2
                "#,
            )
            .bind(cutoff_unix as i64)
            .bind(limit)
            .fetch_all(pool)
            .await?;
            rows.into_iter()
                .map(|row| {
                    Ok(HistoryRetentionObjectCandidate::JobOutput {
                        job_id: row.try_get("job_id")?,
                        client_id: row.try_get("client_id")?,
                        seq: row.try_get("seq")?,
                        object_key: row.try_get("object_key")?,
                    })
                })
                .collect()
        }
        HistoryDomain::BackupArtifacts => {
            let rows = sqlx::query(
                r#"
                SELECT id AS artifact_id, object_key
                FROM backup_artifacts
                WHERE created_at < to_timestamp($1)
                ORDER BY created_at ASC, id ASC
                LIMIT $2
                "#,
            )
            .bind(cutoff_unix as i64)
            .bind(limit)
            .fetch_all(pool)
            .await?;
            rows.into_iter()
                .map(|row| {
                    Ok(HistoryRetentionObjectCandidate::BackupArtifact {
                        artifact_id: row.try_get("artifact_id")?,
                        object_key: row.try_get("object_key")?,
                    })
                })
                .collect()
        }
        _ => Ok(Vec::new()),
    }
}

async fn prune_postgres_history_retention_object_candidate(
    pool: &sqlx::PgPool,
    candidate: &HistoryRetentionObjectCandidate,
) -> Result<i64> {
    match candidate {
        HistoryRetentionObjectCandidate::JobOutput {
            job_id,
            client_id,
            seq,
            ..
        } => sqlx::query_scalar::<_, i64>(
            r#"
                WITH deleted_outputs AS (
                    DELETE FROM job_outputs
                    WHERE job_id = $1
                      AND client_id = $2
                      AND seq = $3
                    RETURNING object_key
                ),
                marked_artifacts AS (
                    UPDATE server_artifacts artifact
                    SET status = 'deleting'
                    FROM deleted_outputs deleted
                    WHERE deleted.object_key IS NOT NULL
                      AND artifact.object_key = deleted.object_key
                      AND artifact.status = 'active'
                    RETURNING artifact.id
                )
                SELECT count(*)::bigint FROM deleted_outputs
                "#,
        )
        .bind(job_id)
        .bind(client_id)
        .bind(seq)
        .fetch_one(pool)
        .await
        .map_err(Into::into),
        HistoryRetentionObjectCandidate::BackupArtifact { artifact_id, .. } => {
            sqlx::query_scalar::<_, i64>(
                r#"
                WITH doomed AS (
                    SELECT artifact.id AS artifact_id, artifact.object_key
                    FROM backup_artifacts artifact
                    WHERE artifact.id = $1
                ),
                cleared_requests AS (
                    UPDATE backup_requests request
                    SET artifact_id = NULL,
                        status = 'requested_metadata_only'
                    FROM doomed
                    WHERE request.artifact_id = doomed.artifact_id
                    RETURNING request.id
                ),
                deleted_artifacts AS (
                    DELETE FROM backup_artifacts artifact
                    USING doomed
                    WHERE artifact.id = doomed.artifact_id
                    RETURNING artifact.object_key
                ),
                marked_artifacts AS (
                    UPDATE server_artifacts artifact
                    SET status = 'deleting'
                    FROM deleted_artifacts deleted
                    WHERE artifact.object_key = deleted.object_key
                      AND artifact.status = 'active'
                    RETURNING artifact.id
                )
                SELECT count(*)::bigint FROM deleted_artifacts
                "#,
            )
            .bind(artifact_id)
            .fetch_one(pool)
            .await
            .map_err(Into::into)
        }
    }
}

async fn prune_postgres_history_domain(
    pool: &sqlx::PgPool,
    domain: HistoryDomain,
    cutoff_unix: u64,
    limit: i32,
    dry_run: bool,
) -> Result<HistoryRetentionPruneOutcome> {
    match (domain, dry_run) {
        (HistoryDomain::AuditLogs, true) => {
            select_id_count(
                pool,
                "audit_logs",
                "created_at",
                "id",
                "TRUE",
                cutoff_unix,
                limit,
            )
            .await
        }
        (HistoryDomain::AuditLogs, false) => {
            delete_by_id(
                pool,
                "audit_logs",
                "created_at",
                "id",
                "TRUE",
                cutoff_unix,
                limit,
            )
            .await
        }
        (HistoryDomain::TelemetryRollups, true) => {
            prune_telemetry_rollups(pool, cutoff_unix, limit, true).await
        }
        (HistoryDomain::TelemetryRollups, false) => {
            prune_telemetry_rollups(pool, cutoff_unix, limit, false).await
        }
        (HistoryDomain::SystemMetricRollups, true) => {
            prune_system_metric_rollups(pool, cutoff_unix, limit, true).await
        }
        (HistoryDomain::SystemMetricRollups, false) => {
            prune_system_metric_rollups(pool, cutoff_unix, limit, false).await
        }
        (HistoryDomain::JobOutputs, true) => {
            prune_job_outputs(pool, cutoff_unix, limit, true).await
        }
        (HistoryDomain::JobOutputs, false) => {
            prune_job_outputs(pool, cutoff_unix, limit, false).await
        }
        (HistoryDomain::BackupArtifacts, true) => {
            prune_backup_artifacts(pool, cutoff_unix, limit, true).await
        }
        (HistoryDomain::BackupArtifacts, false) => {
            prune_backup_artifacts(pool, cutoff_unix, limit, false).await
        }
        (HistoryDomain::NetworkObservations, true) => {
            select_id_count(
                pool,
                "network_observations",
                "observed_at",
                "id",
                "TRUE",
                cutoff_unix,
                limit,
            )
            .await
        }
        (HistoryDomain::NetworkObservations, false) => {
            delete_by_id(
                pool,
                "network_observations",
                "observed_at",
                "id",
                "TRUE",
                cutoff_unix,
                limit,
            )
            .await
        }
        (HistoryDomain::TopologyHistory, true) => {
            select_id_count(
                pool,
                "network_observations",
                "observed_at",
                "id",
                "(plan_name IS NOT NULL OR interface_name IS NOT NULL)",
                cutoff_unix,
                limit,
            )
            .await
        }
        (HistoryDomain::TopologyHistory, false) => {
            delete_by_id(
                pool,
                "network_observations",
                "observed_at",
                "id",
                "(plan_name IS NOT NULL OR interface_name IS NOT NULL)",
                cutoff_unix,
                limit,
            )
            .await
        }
    }
}

async fn select_id_count(
    pool: &sqlx::PgPool,
    table: &str,
    timestamp_column: &str,
    id_column: &str,
    filter: &str,
    cutoff_unix: u64,
    limit: i32,
) -> Result<HistoryRetentionPruneOutcome> {
    let query = format!(
        r#"
        SELECT {id_column}
        FROM {table}
        WHERE {timestamp_column} < to_timestamp($1)
          AND {filter}
        ORDER BY {timestamp_column} ASC, {id_column} ASC
        LIMIT $2
        "#
    );
    let rows = sqlx::query(&query)
        .bind(cutoff_unix as i64)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    Ok(HistoryRetentionPruneOutcome {
        matched_rows: rows.len() as i64,
        pruned_rows: 0,
        object_keys: Vec::new(),
    })
}

async fn delete_by_id(
    pool: &sqlx::PgPool,
    table: &str,
    timestamp_column: &str,
    id_column: &str,
    filter: &str,
    cutoff_unix: u64,
    limit: i32,
) -> Result<HistoryRetentionPruneOutcome> {
    let query = format!(
        r#"
        WITH doomed AS (
            SELECT {id_column}
            FROM {table}
            WHERE {timestamp_column} < to_timestamp($1)
              AND {filter}
            ORDER BY {timestamp_column} ASC, {id_column} ASC
            LIMIT $2
        )
        DELETE FROM {table} target_row
        USING doomed
        WHERE target_row.{id_column} = doomed.{id_column}
        RETURNING target_row.{id_column}
        "#
    );
    let rows = sqlx::query(&query)
        .bind(cutoff_unix as i64)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    Ok(HistoryRetentionPruneOutcome {
        matched_rows: rows.len() as i64,
        pruned_rows: rows.len() as i64,
        object_keys: Vec::new(),
    })
}

async fn prune_telemetry_rollups(
    pool: &sqlx::PgPool,
    cutoff_unix: u64,
    limit: i32,
    dry_run: bool,
) -> Result<HistoryRetentionPruneOutcome> {
    let query = if dry_run {
        r#"
        SELECT client_id, bucket_secs, bucket_start
        FROM telemetry_rollups
        WHERE bucket_start < to_timestamp($1)
        ORDER BY bucket_start ASC, client_id ASC
        LIMIT $2
        "#
    } else {
        r#"
        WITH doomed AS (
            SELECT client_id, bucket_secs, bucket_start
            FROM telemetry_rollups
            WHERE bucket_start < to_timestamp($1)
            ORDER BY bucket_start ASC, client_id ASC
            LIMIT $2
        )
        DELETE FROM telemetry_rollups rollup
        USING doomed
        WHERE rollup.client_id = doomed.client_id
          AND rollup.bucket_secs = doomed.bucket_secs
          AND rollup.bucket_start = doomed.bucket_start
        RETURNING rollup.client_id
        "#
    };
    let rows = sqlx::query(query)
        .bind(cutoff_unix as i64)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    Ok(HistoryRetentionPruneOutcome {
        matched_rows: rows.len() as i64,
        pruned_rows: if dry_run { 0 } else { rows.len() as i64 },
        object_keys: Vec::new(),
    })
}

async fn prune_system_metric_rollups(
    pool: &sqlx::PgPool,
    cutoff_unix: u64,
    limit: i32,
    dry_run: bool,
) -> Result<HistoryRetentionPruneOutcome> {
    let query = if dry_run {
        r#"
        SELECT metric, bucket_secs, bucket_start
        FROM system_metric_rollups
        WHERE bucket_start < to_timestamp($1)
        ORDER BY bucket_start ASC, metric ASC
        LIMIT $2
        "#
    } else {
        r#"
        WITH doomed AS (
            SELECT metric, bucket_secs, bucket_start
            FROM system_metric_rollups
            WHERE bucket_start < to_timestamp($1)
            ORDER BY bucket_start ASC, metric ASC
            LIMIT $2
        )
        DELETE FROM system_metric_rollups rollup
        USING doomed
        WHERE rollup.metric = doomed.metric
          AND rollup.bucket_secs = doomed.bucket_secs
          AND rollup.bucket_start = doomed.bucket_start
        RETURNING rollup.metric
        "#
    };
    let rows = sqlx::query(query)
        .bind(cutoff_unix as i64)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    Ok(HistoryRetentionPruneOutcome {
        matched_rows: rows.len() as i64,
        pruned_rows: if dry_run { 0 } else { rows.len() as i64 },
        object_keys: Vec::new(),
    })
}

async fn prune_job_outputs(
    pool: &sqlx::PgPool,
    cutoff_unix: u64,
    limit: i32,
    dry_run: bool,
) -> Result<HistoryRetentionPruneOutcome> {
    let query = if dry_run {
        r#"
        SELECT object_key
        FROM job_outputs
        WHERE created_at < to_timestamp($1)
        ORDER BY created_at ASC, job_id ASC, client_id ASC, seq ASC
        LIMIT $2
        "#
    } else {
        r#"
        WITH doomed AS (
            SELECT job_id, client_id, seq, object_key
            FROM job_outputs
            WHERE created_at < to_timestamp($1)
            ORDER BY created_at ASC, job_id ASC, client_id ASC, seq ASC
            LIMIT $2
        )
        DELETE FROM job_outputs output
        USING doomed
        WHERE output.job_id = doomed.job_id
          AND output.client_id = doomed.client_id
          AND output.seq = doomed.seq
        RETURNING output.object_key
        "#
    };
    object_key_outcome(pool, query, cutoff_unix, limit, dry_run).await
}

async fn prune_backup_artifacts(
    pool: &sqlx::PgPool,
    cutoff_unix: u64,
    limit: i32,
    dry_run: bool,
) -> Result<HistoryRetentionPruneOutcome> {
    let query = if dry_run {
        r#"
        SELECT object_key
        FROM backup_artifacts
        WHERE created_at < to_timestamp($1)
        ORDER BY created_at ASC, id ASC
        LIMIT $2
        "#
    } else {
        r#"
        WITH doomed AS (
            SELECT id, object_key
            FROM backup_artifacts
            WHERE created_at < to_timestamp($1)
            ORDER BY created_at ASC, id ASC
            LIMIT $2
        ),
        cleared_requests AS (
            UPDATE backup_requests request
            SET artifact_id = NULL,
                status = 'requested_metadata_only'
            FROM doomed
            WHERE request.artifact_id = doomed.id
            RETURNING request.id
        )
        DELETE FROM backup_artifacts artifact
        USING doomed
        WHERE artifact.id = doomed.id
        RETURNING artifact.object_key
        "#
    };
    object_key_outcome(pool, query, cutoff_unix, limit, dry_run).await
}

async fn object_key_outcome(
    pool: &sqlx::PgPool,
    query: &str,
    cutoff_unix: u64,
    limit: i32,
    dry_run: bool,
) -> Result<HistoryRetentionPruneOutcome> {
    let rows = sqlx::query(query)
        .bind(cutoff_unix as i64)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    let object_keys = rows
        .iter()
        .filter_map(|row| {
            row.try_get::<Option<String>, _>("object_key")
                .ok()
                .flatten()
        })
        .collect::<Vec<_>>();
    Ok(HistoryRetentionPruneOutcome {
        matched_rows: rows.len() as i64,
        pruned_rows: if dry_run { 0 } else { rows.len() as i64 },
        object_keys,
    })
}

fn history_retention_audit(
    action: &str,
    target: &str,
    operator: &AuthContext,
    metadata: serde_json::Value,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: action.to_string(),
        target: target.to_string(),
        command_hash: None,
        metadata,
        created_at,
    }
}

fn timestamp_before(value: &str, cutoff_unix: u64) -> bool {
    value
        .parse::<u64>()
        .map(|observed| observed < cutoff_unix)
        .unwrap_or(false)
}
