use std::collections::HashMap;

use anyhow::Result;
use sqlx::Row;
use uuid::Uuid;
use vpsman_common::{payload_hash, JobCommand};

use crate::{
    model::{
        AuditLogView, AuthContext, BackupPolicyMetadata, BackupPolicyPrunePolicyView,
        BackupPolicyView, BackupRequestStatus, CreateBackupPolicyRequest, CreateScheduleRequest,
        ScheduleView,
    },
    repository::Repository,
    unix_now,
};

const DEFAULT_BACKUP_POLICY_RETENTION_DAYS: i32 = 30;
const DEFAULT_BACKUP_POLICY_KEEP_LAST: i32 = 7;

impl Repository {
    pub(crate) async fn list_backup_policies(&self) -> Result<Vec<BackupPolicyView>> {
        let schedules = self.list_schedules().await?;
        let metadata = self.backup_policy_metadata_by_schedule_id().await?;
        let mut policies = schedules
            .into_iter()
            .filter_map(|schedule| {
                let metadata = metadata
                    .get(&schedule.id)
                    .cloned()
                    .unwrap_or_else(|| default_backup_policy_metadata(&schedule));
                backup_policy_view(schedule, metadata)
            })
            .collect::<Vec<_>>();
        policies.sort_by(|left, right| {
            left.next_run_at
                .cmp(&right.next_run_at)
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(policies)
    }

    pub(crate) async fn create_backup_policy(
        &self,
        request: CreateBackupPolicyRequest,
        operator: &AuthContext,
    ) -> Result<BackupPolicyView> {
        let retention_days = request
            .retention_days
            .unwrap_or(DEFAULT_BACKUP_POLICY_RETENTION_DAYS);
        let keep_last = request.keep_last.unwrap_or(DEFAULT_BACKUP_POLICY_KEEP_LAST);
        let rotation_generation = normalize_policy_generation(request.rotation_generation);
        let schedule_request = CreateScheduleRequest {
            name: request.name,
            operation: JobCommand::Backup {
                paths: request.paths,
                include_config: request.include_config,
                recipient_public_key_hex: request
                    .recipient_public_key_hex
                    .map(|value| value.to_ascii_lowercase()),
            },
            clients: request.clients,
            pools: request.pools,
            tags: request.tags,
            interval_secs: request.interval_secs,
            start_at_unix: request.start_at_unix,
            enabled: request.enabled,
            catch_up_policy: request.catch_up_policy,
            catch_up_limit: request.catch_up_limit,
            retry_delay_secs: request.retry_delay_secs,
            max_failures: request.max_failures,
        };
        let schedule = self.create_schedule(schedule_request, operator).await?;
        let metadata = self
            .upsert_backup_policy_metadata(
                schedule.id,
                retention_days,
                keep_last,
                rotation_generation,
            )
            .await?;
        self.audit_backup_policy_upserted(&schedule, &metadata, operator)
            .await?;
        Ok(backup_policy_view(schedule, metadata)
            .expect("backup policy schedule must carry backup operation"))
    }

    pub(crate) async fn prune_backup_policy_artifacts(
        &self,
        policy: &BackupPolicyView,
        cutoff_unix: u64,
        dry_run: bool,
    ) -> Result<BackupPolicyPrunePolicyView> {
        let outcome = match self {
            Self::Memory(memory) => {
                let artifacts = memory.backup_artifacts.read().await.clone();
                let requests = memory.backup_requests.read().await.clone();
                let mut candidates = requests
                    .iter()
                    .filter(|request| request.source_schedule_id == Some(policy.schedule_id))
                    .filter_map(|request| {
                        let artifact_id = request.artifact_id?;
                        let artifact = artifacts
                            .iter()
                            .find(|artifact| artifact.id == artifact_id)?;
                        Some(BackupPolicyArtifactCandidate {
                            request_id: request.id,
                            artifact_id,
                            client_id: request.client_id.clone(),
                            object_key: artifact.object_key.clone(),
                            created_at: artifact.created_at.clone(),
                        })
                    })
                    .collect::<Vec<_>>();
                candidates.sort_by(|left, right| {
                    left.client_id
                        .cmp(&right.client_id)
                        .then_with(|| right.created_at.cmp(&left.created_at))
                        .then_with(|| right.artifact_id.cmp(&left.artifact_id))
                });
                let mut selected = Vec::new();
                let mut current_client = String::new();
                let mut rank_for_client = 0_i32;
                for candidate in candidates {
                    if candidate.client_id != current_client {
                        current_client = candidate.client_id.clone();
                        rank_for_client = 0;
                    }
                    rank_for_client += 1;
                    if rank_for_client > policy.keep_last
                        && timestamp_before_unix_string(&candidate.created_at, cutoff_unix)
                    {
                        selected.push(candidate);
                    }
                }
                let object_keys = selected
                    .iter()
                    .map(|candidate| candidate.object_key.clone())
                    .collect::<Vec<_>>();
                let matched_rows = selected.len() as i64;
                if !dry_run {
                    let selected_artifact_ids = selected
                        .iter()
                        .map(|candidate| candidate.artifact_id)
                        .collect::<std::collections::HashSet<_>>();
                    let selected_request_ids = selected
                        .iter()
                        .map(|candidate| candidate.request_id)
                        .collect::<std::collections::HashSet<_>>();
                    {
                        let mut stored_requests = memory.backup_requests.write().await;
                        for request in stored_requests
                            .iter_mut()
                            .filter(|request| selected_request_ids.contains(&request.id))
                        {
                            request.artifact_id = None;
                            request.status = BackupRequestStatus::RequestedMetadataOnly
                                .as_str()
                                .to_string();
                        }
                    }
                    memory
                        .backup_artifacts
                        .write()
                        .await
                        .retain(|artifact| !selected_artifact_ids.contains(&artifact.id));
                }
                BackupPolicyPruneOutcome {
                    matched_rows,
                    pruned_rows: if dry_run { 0 } else { matched_rows },
                    object_keys,
                }
            }
            Self::Postgres(pool) => {
                prune_postgres_backup_policy_artifacts(
                    pool,
                    policy.schedule_id,
                    policy.keep_last,
                    cutoff_unix,
                    dry_run,
                )
                .await?
            }
        };
        Ok(BackupPolicyPrunePolicyView {
            schedule_id: policy.schedule_id,
            name: policy.name.clone(),
            enabled: policy.enabled,
            retention_days: policy.retention_days,
            keep_last: policy.keep_last,
            cutoff_unix,
            matched_rows: outcome.matched_rows,
            pruned_rows: outcome.pruned_rows,
            object_keys: outcome.object_keys,
            object_delete_attempted: false,
            metadata_only: true,
            status: if dry_run {
                "dry_run".to_string()
            } else if outcome.pruned_rows == 0 {
                "no_matches".to_string()
            } else {
                "pruned".to_string()
            },
        })
    }

    pub(crate) async fn record_backup_policy_prune_audit(
        &self,
        operator: &AuthContext,
        dry_run: bool,
        metadata_only: Option<bool>,
        policies: &[BackupPolicyPrunePolicyView],
    ) -> Result<()> {
        let metadata = serde_json::json!({
            "dry_run": dry_run,
            "metadata_only_requested": metadata_only,
            "policies": policies.iter().map(|policy| serde_json::json!({
                "schedule_id": policy.schedule_id,
                "name": &policy.name,
                "matched_rows": policy.matched_rows,
                "pruned_rows": policy.pruned_rows,
                "object_key_count": policy.object_keys.len(),
                "metadata_only": policy.metadata_only,
                "object_delete_attempted": policy.object_delete_attempted,
            })).collect::<Vec<_>>(),
            "operator_username": &operator.operator.username,
            "operator_role": &operator.operator.role,
            "session_id": operator.session_id,
        });
        match self {
            Self::Memory(memory) => {
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "backup_policy.retention_pruned".to_string(),
                    target: "backup_policy_retention".to_string(),
                    command_hash: None,
                    metadata,
                    created_at: unix_now().to_string(),
                });
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
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("backup_policy.retention_pruned")
                .bind("backup_policy_retention")
                .bind(metadata)
                .execute(pool)
                .await?;
                Ok(())
            }
        }
    }

    async fn backup_policy_metadata_by_schedule_id(
        &self,
    ) -> Result<HashMap<Uuid, BackupPolicyMetadata>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .backup_policies
                .read()
                .await
                .iter()
                .cloned()
                .map(|metadata| (metadata.schedule_id, metadata))
                .collect()),
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        schedule_id,
                        retention_days,
                        keep_last,
                        rotation_generation,
                        updated_at::text AS updated_at
                    FROM backup_policies
                    "#,
                )
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let metadata = BackupPolicyMetadata {
                            schedule_id: row.try_get("schedule_id")?,
                            retention_days: row.try_get("retention_days")?,
                            keep_last: row.try_get("keep_last")?,
                            rotation_generation: row.try_get("rotation_generation")?,
                            updated_at: row.try_get("updated_at")?,
                        };
                        Ok((metadata.schedule_id, metadata))
                    })
                    .collect()
            }
        }
    }

    async fn upsert_backup_policy_metadata(
        &self,
        schedule_id: Uuid,
        retention_days: i32,
        keep_last: i32,
        rotation_generation: Option<String>,
    ) -> Result<BackupPolicyMetadata> {
        match self {
            Self::Memory(memory) => {
                let updated_at = unix_now().to_string();
                let metadata = BackupPolicyMetadata {
                    schedule_id,
                    retention_days,
                    keep_last,
                    rotation_generation,
                    updated_at,
                };
                let mut policies = memory.backup_policies.write().await;
                if let Some(existing) = policies
                    .iter_mut()
                    .find(|existing| existing.schedule_id == schedule_id)
                {
                    *existing = metadata.clone();
                } else {
                    policies.push(metadata.clone());
                }
                Ok(metadata)
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    INSERT INTO backup_policies (
                        schedule_id,
                        retention_days,
                        keep_last,
                        rotation_generation
                    )
                    VALUES ($1, $2, $3, $4)
                    ON CONFLICT (schedule_id) DO UPDATE SET
                        retention_days = EXCLUDED.retention_days,
                        keep_last = EXCLUDED.keep_last,
                        rotation_generation = EXCLUDED.rotation_generation,
                        updated_at = now()
                    RETURNING
                        schedule_id,
                        retention_days,
                        keep_last,
                        rotation_generation,
                        updated_at::text AS updated_at
                    "#,
                )
                .bind(schedule_id)
                .bind(retention_days)
                .bind(keep_last)
                .bind(&rotation_generation)
                .fetch_one(pool)
                .await?;
                Ok(BackupPolicyMetadata {
                    schedule_id: row.try_get("schedule_id")?,
                    retention_days: row.try_get("retention_days")?,
                    keep_last: row.try_get("keep_last")?,
                    rotation_generation: row.try_get("rotation_generation")?,
                    updated_at: row.try_get("updated_at")?,
                })
            }
        }
    }

    async fn audit_backup_policy_upserted(
        &self,
        schedule: &ScheduleView,
        metadata: &BackupPolicyMetadata,
        operator: &AuthContext,
    ) -> Result<()> {
        let recipient_key_sha256_hex = match &schedule.operation {
            JobCommand::Backup {
                recipient_public_key_hex: Some(recipient_public_key_hex),
                ..
            } => Some(payload_hash(
                recipient_public_key_hex.to_ascii_lowercase().as_bytes(),
            )),
            _ => None,
        };
        let audit_metadata = serde_json::json!({
            "name": &schedule.name,
            "clients": &schedule.clients,
            "pools": &schedule.pools,
            "tags": &schedule.tags,
            "interval_secs": schedule.interval_secs,
            "retention_days": metadata.retention_days,
            "keep_last": metadata.keep_last,
            "rotation_generation": &metadata.rotation_generation,
            "recipient_public_key_sha256_hex": recipient_key_sha256_hex,
            "operator_username": &operator.operator.username,
            "session_id": operator.session_id,
        });
        match self {
            Self::Memory(memory) => {
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "backup_policy.upserted".to_string(),
                    target: format!("backup_policy:{}", schedule.id),
                    command_hash: None,
                    metadata: audit_metadata,
                    created_at: unix_now().to_string(),
                });
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
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("backup_policy.upserted")
                .bind(format!("backup_policy:{}", schedule.id))
                .bind(audit_metadata)
                .execute(pool)
                .await?;
                Ok(())
            }
        }
    }
}

#[derive(Clone, Debug)]
struct BackupPolicyArtifactCandidate {
    request_id: Uuid,
    artifact_id: Uuid,
    client_id: String,
    object_key: String,
    created_at: String,
}

#[derive(Clone, Debug)]
struct BackupPolicyPruneOutcome {
    matched_rows: i64,
    pruned_rows: i64,
    object_keys: Vec<String>,
}

async fn prune_postgres_backup_policy_artifacts(
    pool: &sqlx::PgPool,
    schedule_id: Uuid,
    keep_last: i32,
    cutoff_unix: u64,
    dry_run: bool,
) -> Result<BackupPolicyPruneOutcome> {
    let query = if dry_run {
        r#"
        WITH ranked AS (
            SELECT
                request.id AS request_id,
                artifact.id AS artifact_id,
                artifact.object_key,
                artifact.created_at,
                row_number() OVER (
                    PARTITION BY request.client_id
                    ORDER BY artifact.created_at DESC, artifact.id DESC
                ) AS retained_rank
            FROM backup_requests request
            JOIN backup_artifacts artifact ON artifact.id = request.artifact_id
            WHERE request.source_schedule_id = $1
        )
        SELECT object_key
        FROM ranked
        WHERE retained_rank > $2
          AND created_at < to_timestamp($3)
        ORDER BY created_at ASC, artifact_id ASC
        "#
    } else {
        r#"
        WITH ranked AS (
            SELECT
                request.id AS request_id,
                artifact.id AS artifact_id,
                artifact.object_key,
                artifact.created_at,
                row_number() OVER (
                    PARTITION BY request.client_id
                    ORDER BY artifact.created_at DESC, artifact.id DESC
                ) AS retained_rank
            FROM backup_requests request
            JOIN backup_artifacts artifact ON artifact.id = request.artifact_id
            WHERE request.source_schedule_id = $1
        ),
        doomed AS (
            SELECT request_id, artifact_id, object_key
            FROM ranked
            WHERE retained_rank > $2
              AND created_at < to_timestamp($3)
            ORDER BY created_at ASC, artifact_id ASC
        ),
        cleared_requests AS (
            UPDATE backup_requests request
            SET artifact_id = NULL,
                status = 'requested_metadata_only'
            FROM doomed
            WHERE request.id = doomed.request_id
            RETURNING request.id
        )
        DELETE FROM backup_artifacts artifact
        USING doomed
        WHERE artifact.id = doomed.artifact_id
        RETURNING artifact.object_key
        "#
    };
    let rows = sqlx::query(query)
        .bind(schedule_id)
        .bind(keep_last)
        .bind(cutoff_unix as i64)
        .fetch_all(pool)
        .await?;
    let object_keys = rows
        .iter()
        .filter_map(|row| {
            row.try_get::<Option<String>, _>("object_key")
                .ok()
                .flatten()
                .or_else(|| row.try_get::<String, _>("object_key").ok())
        })
        .collect::<Vec<_>>();
    Ok(BackupPolicyPruneOutcome {
        matched_rows: rows.len() as i64,
        pruned_rows: if dry_run { 0 } else { rows.len() as i64 },
        object_keys,
    })
}

fn backup_policy_view(
    schedule: ScheduleView,
    metadata: BackupPolicyMetadata,
) -> Option<BackupPolicyView> {
    let JobCommand::Backup {
        paths,
        include_config,
        recipient_public_key_hex,
    } = schedule.operation.clone()
    else {
        return None;
    };
    Some(BackupPolicyView {
        schedule_id: schedule.id,
        name: schedule.name,
        enabled: schedule.enabled,
        clients: schedule.clients,
        pools: schedule.pools,
        tags: schedule.tags,
        paths,
        include_config,
        recipient_public_key_hex,
        retention_days: metadata.retention_days,
        keep_last: metadata.keep_last,
        rotation_generation: metadata.rotation_generation,
        interval_secs: schedule.interval_secs,
        catch_up_policy: schedule.catch_up_policy,
        catch_up_limit: schedule.catch_up_limit,
        retry_delay_secs: schedule.retry_delay_secs,
        max_failures: schedule.max_failures,
        failure_count: schedule.failure_count,
        last_error: schedule.last_error,
        next_run_at: schedule.next_run_at,
        last_run_at: schedule.last_run_at,
        created_at: schedule.created_at,
        updated_at: metadata.updated_at,
    })
}

fn default_backup_policy_metadata(schedule: &ScheduleView) -> BackupPolicyMetadata {
    BackupPolicyMetadata {
        schedule_id: schedule.id,
        retention_days: DEFAULT_BACKUP_POLICY_RETENTION_DAYS,
        keep_last: DEFAULT_BACKUP_POLICY_KEEP_LAST,
        rotation_generation: None,
        updated_at: schedule.created_at.clone(),
    }
}

fn normalize_policy_generation(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn timestamp_before_unix_string(value: &str, cutoff_unix: u64) -> bool {
    value
        .parse::<u64>()
        .map(|observed| observed < cutoff_unix)
        .unwrap_or(false)
}
