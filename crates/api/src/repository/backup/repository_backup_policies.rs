use std::collections::HashMap;

use anyhow::{ensure, Context, Result};
use sqlx::Row;
use uuid::Uuid;
use vpsman_common::JobCommand;

use crate::{
    model::{
        AuthContext, BackupPolicyMetadata, BackupPolicyPrunePolicyView, BackupPolicyView,
        CreateBackupPolicyRequest, ListQuery, ScheduleView,
    },
    repository::Repository,
    repository_artifact_deletions::{
        finish_owned_artifact_deletion_in_tx, lock_owned_artifact_deletion_in_tx,
        ArtifactDeletionOwner,
    },
    repository_schedules::{
        backup_policy_schedule_by_id_postgres, backup_policy_schedule_by_id_postgres_in_tx,
        create_schedule_record_postgres_in_tx, update_schedule_record_postgres_in_tx,
        ScheduleCreateInput, ScheduleSnapshotExpectation,
    },
};

const DEFAULT_BACKUP_POLICY_RETENTION_DAYS: i32 = 30;
const DEFAULT_BACKUP_POLICY_KEEP_LAST: i32 = 7;

impl Repository {
    pub(crate) async fn list_backup_policies(
        &self,
        query: &ListQuery,
    ) -> Result<Vec<BackupPolicyView>> {
        let mut bounded_query = query.clone();
        bounded_query.limit = Some(query.limit.unwrap_or(1000));
        let schedules = self.query_backup_policy_schedules(&bounded_query).await?;
        let schedule_ids = schedules
            .iter()
            .map(|schedule| schedule.id)
            .collect::<Vec<_>>();
        let metadata = self
            .backup_policy_metadata_by_schedule_id(&schedule_ids)
            .await?;
        let policies = schedules
            .into_iter()
            .filter_map(|schedule| {
                metadata
                    .get(&schedule.id)
                    .cloned()
                    .and_then(|metadata| backup_policy_view(schedule, metadata))
            })
            .collect::<Vec<_>>();
        Ok(policies)
    }

    pub(crate) async fn backup_policy_by_schedule_id(
        &self,
        schedule_id: Uuid,
    ) -> Result<Option<BackupPolicyView>> {
        let schedule = match self {
            Self::Postgres(pool) => {
                backup_policy_schedule_by_id_postgres(pool, schedule_id).await?
            }
        };
        let Some(schedule) = schedule else {
            return Ok(None);
        };
        let metadata = self
            .backup_policy_metadata_by_schedule_id(&[schedule_id])
            .await?
            .remove(&schedule_id);
        Ok(metadata.and_then(|metadata| backup_policy_view(schedule, metadata)))
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
        let rotation_generation = normalize_policy_generation(request.rotation_generation.clone());
        let schedule_request = backup_policy_schedule_input(&request);
        let (schedule, metadata) = match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let schedule =
                    create_schedule_record_postgres_in_tx(&mut tx, &schedule_request, operator)
                        .await?;
                let metadata = upsert_backup_policy_metadata_postgres_in_tx(
                    &mut tx,
                    schedule.id,
                    retention_days,
                    keep_last,
                    rotation_generation,
                )
                .await?;
                insert_backup_policy_audit_postgres_in_tx(&mut tx, &schedule, &metadata, operator)
                    .await?;
                tx.commit().await?;
                (schedule, metadata)
            }
        };
        Ok(backup_policy_view(schedule, metadata)
            .expect("backup policy schedule must carry backup operation"))
    }

    pub(crate) async fn update_backup_policy(
        &self,
        schedule_id: Uuid,
        request: CreateBackupPolicyRequest,
        expectation: &ScheduleSnapshotExpectation,
        operator: &AuthContext,
    ) -> Result<Option<BackupPolicyView>> {
        let retention_days = request
            .retention_days
            .context("backup_policy_retention_days_required")?;
        let keep_last = request
            .keep_last
            .context("backup_policy_keep_last_required")?;
        let rotation_generation = normalize_policy_generation(request.rotation_generation.clone());
        let mut schedule_request = backup_policy_schedule_input(&request);
        schedule_request.expected_definition_revision = Some(expectation.definition_revision);
        let (schedule, metadata) = match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                if backup_policy_schedule_by_id_postgres_in_tx(&mut tx, schedule_id)
                    .await?
                    .is_none()
                {
                    return Ok(None);
                }
                let schedule = update_schedule_record_postgres_in_tx(
                    &mut tx,
                    schedule_id,
                    &schedule_request,
                    Some(expectation),
                    operator,
                )
                .await?;
                let metadata = upsert_backup_policy_metadata_postgres_in_tx(
                    &mut tx,
                    schedule.id,
                    retention_days,
                    keep_last,
                    rotation_generation,
                )
                .await?;
                insert_backup_policy_audit_postgres_in_tx(&mut tx, &schedule, &metadata, operator)
                    .await?;
                tx.commit().await?;
                (schedule, metadata)
            }
        };
        Ok(Some(backup_policy_view(schedule, metadata).expect(
            "updated backup policy must carry backup operation",
        )))
    }

    pub(crate) async fn list_backup_policy_prune_candidates(
        &self,
        policy: &BackupPolicyView,
        cutoff_unix: u64,
    ) -> Result<Vec<BackupPolicyPruneCandidate>> {
        match self {
            Self::Postgres(pool) => {
                list_postgres_backup_policy_prune_candidates(
                    pool,
                    policy.schedule_id,
                    policy.keep_last,
                    cutoff_unix,
                )
                .await
            }
        }
    }

    pub(crate) async fn prune_backup_policy_candidates_metadata(
        &self,
        candidates: &[BackupPolicyPruneCandidate],
    ) -> Result<i64> {
        if candidates.is_empty() {
            return Ok(0);
        }
        match self {
            Self::Postgres(pool) => {
                prune_postgres_backup_policy_candidates_metadata(pool, candidates).await
            }
        }
    }

    pub(crate) async fn finalize_backup_policy_candidate_object_delete(
        &self,
        policy: &BackupPolicyView,
        candidate: &BackupPolicyPruneCandidate,
        owner: &ArtifactDeletionOwner,
    ) -> Result<i64> {
        ensure!(
            owner.source_kind == "backup_policy"
                && owner.source_id == policy.schedule_id
                && owner.source_revision == policy.definition_revision
                && owner.source_identity == candidate.deletion_identity()
                && owner.object_key == candidate.object_key,
            "backup policy artifact deletion review changed"
        );
        match self {
            Self::Postgres(pool) => {
                finalize_postgres_backup_policy_candidate_object_delete(pool, candidate, owner)
                    .await
            }
        }
    }

    pub(crate) fn backup_policy_prune_view(
        &self,
        policy: &BackupPolicyView,
        cutoff_unix: u64,
        matched_rows: i64,
        pruned_rows: i64,
        object_keys: Vec<String>,
        object_delete_attempted: bool,
        object_delete_errors: Vec<String>,
        metadata_only: bool,
        status: &str,
    ) -> BackupPolicyPrunePolicyView {
        BackupPolicyPrunePolicyView {
            schedule_id: policy.schedule_id,
            name: policy.name.clone(),
            enabled: policy.enabled,
            retention_days: policy.retention_days,
            keep_last: policy.keep_last,
            cutoff_unix,
            matched_rows,
            pruned_rows,
            object_keys,
            object_delete_attempted,
            object_delete_errors,
            metadata_only,
            status: status.to_string(),
        }
    }

    pub(crate) async fn record_backup_policy_prune_audit(
        &self,
        operator: &AuthContext,
        dry_run: bool,
        metadata_only: Option<bool>,
        policies: &[BackupPolicyPrunePolicyView],
    ) -> Result<()> {
        let result = if dry_run {
            "previewed"
        } else if policies
            .iter()
            .any(|policy| !policy.object_delete_errors.is_empty())
        {
            "partial"
        } else {
            "succeeded"
        };
        let metadata = serde_json::json!({
            "dry_run": dry_run,
            "metadata_only_requested": metadata_only,
            "result": result,
            "policies": policies.iter().map(|policy| serde_json::json!({
                "schedule_id": policy.schedule_id,
                "name": &policy.name,
                "matched_rows": policy.matched_rows,
                "pruned_rows": policy.pruned_rows,
                "object_key_count": policy.object_keys.len(),
                "metadata_only": policy.metadata_only,
                "object_delete_attempted": policy.object_delete_attempted,
                "object_delete_errors": &policy.object_delete_errors,
                "status": &policy.status,
            })).collect::<Vec<_>>(),
            "operator_id": operator.operator.id,
            "operator_username": &operator.operator.username,
            "operator_role": &operator.operator.role,
            "operator_session_id": operator.audit_session_id(),
            "origin_kind": "operator_request",
            "component": "backup-retention-controller",
        });
        match self {
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
        schedule_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, BackupPolicyMetadata>> {
        if schedule_ids.is_empty() {
            return Ok(HashMap::new());
        }
        match self {
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
                    WHERE schedule_id = ANY($1)
                    "#,
                )
                .bind(schedule_ids)
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
}

async fn upsert_backup_policy_metadata_postgres_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    schedule_id: Uuid,
    retention_days: i32,
    keep_last: i32,
    rotation_generation: Option<String>,
) -> Result<BackupPolicyMetadata> {
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
    .bind(rotation_generation)
    .fetch_one(&mut **tx)
    .await?;
    Ok(BackupPolicyMetadata {
        schedule_id: row.try_get("schedule_id")?,
        retention_days: row.try_get("retention_days")?,
        keep_last: row.try_get("keep_last")?,
        rotation_generation: row.try_get("rotation_generation")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn backup_policy_audit_metadata(
    schedule: &ScheduleView,
    metadata: &BackupPolicyMetadata,
    operator: &AuthContext,
) -> serde_json::Value {
    serde_json::json!({
        "name": &schedule.name,
        "selector_expression": &schedule.selector_expression,
        "cron_expr": &schedule.cron_expr,
        "timezone": &schedule.timezone,
        "next_runs": &schedule.next_runs,
        "cadence_error": &schedule.cadence_error,
        "retention_days": metadata.retention_days,
        "keep_last": metadata.keep_last,
        "rotation_generation": &metadata.rotation_generation,
        "result": "succeeded",
        "operator_id": operator.operator.id,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "operator_session_id": operator.audit_session_id(),
        "origin_kind": "operator_request",
        "component": "backup-policy-controller",
    })
}

async fn insert_backup_policy_audit_postgres_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    schedule: &ScheduleView,
    metadata: &BackupPolicyMetadata,
    operator: &AuthContext,
) -> Result<()> {
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
    .bind(backup_policy_audit_metadata(schedule, metadata, operator))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[derive(Clone, Debug)]
pub(crate) struct BackupPolicyPruneCandidate {
    pub(crate) request_id: Uuid,
    pub(crate) artifact_id: Uuid,
    client_id: String,
    pub(crate) object_key: String,
    created_at: String,
}

impl BackupPolicyPruneCandidate {
    #[cfg(test)]
    pub(crate) fn for_test(
        request_id: Uuid,
        artifact_id: Uuid,
        client_id: String,
        object_key: String,
        created_at: String,
    ) -> Self {
        Self {
            request_id,
            artifact_id,
            client_id,
            object_key,
            created_at,
        }
    }

    pub(crate) fn preview_hash_key(&self) -> serde_json::Value {
        serde_json::json!({
            "request_id": self.request_id,
            "artifact_id": self.artifact_id,
            "client_id": &self.client_id,
            "object_key": &self.object_key,
            "created_at": &self.created_at,
        })
    }

    pub(crate) fn deletion_identity(&self) -> serde_json::Value {
        serde_json::json!({
            "request_id": self.request_id,
            "backup_artifact_id": self.artifact_id,
            "object_key": &self.object_key,
        })
    }
}

async fn list_postgres_backup_policy_prune_candidates(
    pool: &sqlx::PgPool,
    schedule_id: Uuid,
    keep_last: i32,
    cutoff_unix: u64,
) -> Result<Vec<BackupPolicyPruneCandidate>> {
    let rows = sqlx::query(
        r#"
        WITH ranked AS (
            SELECT
                request.id AS request_id,
                artifact.id AS artifact_id,
                request.client_id,
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
        SELECT request_id, artifact_id, client_id, object_key, created_at::text AS created_at
        FROM ranked
        WHERE retained_rank > $2
          AND created_at < to_timestamp($3)
        ORDER BY created_at ASC, artifact_id ASC
        "#,
    )
    .bind(schedule_id)
    .bind(keep_last)
    .bind(cutoff_unix as i64)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(BackupPolicyPruneCandidate {
                request_id: row.try_get("request_id")?,
                artifact_id: row.try_get("artifact_id")?,
                client_id: row.try_get("client_id")?,
                object_key: row.try_get("object_key")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect()
}

async fn finalize_postgres_backup_policy_candidate_object_delete(
    pool: &sqlx::PgPool,
    candidate: &BackupPolicyPruneCandidate,
    owner: &ArtifactDeletionOwner,
) -> Result<i64> {
    let mut tx = pool.begin().await?;
    ensure!(
        lock_owned_artifact_deletion_in_tx(&mut tx, owner).await?,
        "artifact deletion ownership lost before finalization"
    );
    let pruned_rows = sqlx::query_scalar::<_, i64>(
        r#"
        WITH doomed AS (
            SELECT
                $1::uuid AS request_id,
                artifact.id AS artifact_id,
                artifact.object_key
            FROM backup_artifacts artifact
            WHERE artifact.id = $2
              AND artifact.object_key = $3
        ),
        cleared_requests AS (
            UPDATE backup_requests request
            SET artifact_id = NULL,
                status = 'requested_metadata_only'
            FROM doomed
            WHERE request.id = doomed.request_id
              AND request.artifact_id = doomed.artifact_id
            RETURNING request.id
        ),
        deleted_artifacts AS (
            DELETE FROM backup_artifacts artifact
            USING doomed
            WHERE artifact.id = doomed.artifact_id
            RETURNING artifact.object_key
        )
        SELECT count(*)::bigint FROM deleted_artifacts
        "#,
    )
    .bind(candidate.request_id)
    .bind(candidate.artifact_id)
    .bind(&candidate.object_key)
    .fetch_one(&mut *tx)
    .await?;
    ensure!(
        finish_owned_artifact_deletion_in_tx(&mut tx, owner).await?,
        "artifact deletion ownership lost during finalization"
    );
    tx.commit().await?;
    Ok(pruned_rows)
}

async fn prune_postgres_backup_policy_candidates_metadata(
    pool: &sqlx::PgPool,
    candidates: &[BackupPolicyPruneCandidate],
) -> Result<i64> {
    let request_ids = candidates
        .iter()
        .map(|candidate| candidate.request_id)
        .collect::<Vec<_>>();
    let artifact_ids = candidates
        .iter()
        .map(|candidate| candidate.artifact_id)
        .collect::<Vec<_>>();
    let pruned_rows = sqlx::query_scalar::<_, i64>(
        r#"
        WITH selected AS (
            SELECT *
            FROM unnest($1::uuid[], $2::uuid[]) AS doomed(request_id, artifact_id)
        ),
        doomed AS (
            SELECT
                selected.request_id,
                artifact.id AS artifact_id,
                artifact.object_key
            FROM selected
            JOIN backup_artifacts artifact ON artifact.id = selected.artifact_id
        ),
        cleared_requests AS (
            UPDATE backup_requests request
            SET artifact_id = NULL,
                status = 'requested_metadata_only'
            FROM doomed
            WHERE request.id = doomed.request_id
              AND request.artifact_id = doomed.artifact_id
            RETURNING request.id
        ),
        deleted_artifacts AS (
            DELETE FROM backup_artifacts artifact
            USING doomed
            WHERE artifact.id = doomed.artifact_id
            RETURNING artifact.object_key
        )
        SELECT count(*)::bigint FROM deleted_artifacts
        "#,
    )
    .bind(request_ids)
    .bind(artifact_ids)
    .fetch_one(pool)
    .await?;
    Ok(pruned_rows)
}

fn backup_policy_view(
    schedule: ScheduleView,
    metadata: BackupPolicyMetadata,
) -> Option<BackupPolicyView> {
    let Some(JobCommand::Backup {
        paths,
        include_config,
        follow_symlinks,
        missing_path_policy,
    }) = schedule.operation.clone()
    else {
        return None;
    };
    Some(BackupPolicyView {
        schedule_id: schedule.id,
        definition_revision: schedule.definition_revision,
        name: schedule.name,
        enabled: schedule.enabled,
        selector_expression: schedule.selector_expression,
        target_client_ids: schedule.target_client_ids,
        paths,
        include_config,
        follow_symlinks,
        missing_path_policy,
        retention_days: metadata.retention_days,
        keep_last: metadata.keep_last,
        rotation_generation: metadata.rotation_generation,
        cron_expr: schedule.cron_expr?,
        timezone: schedule.timezone?,
        next_runs: schedule.next_runs,
        cadence_error: schedule.cadence_error,
        catch_up_policy: schedule.catch_up_policy?,
        catch_up_limit: schedule.catch_up_limit?,
        retry_delay_secs: schedule.retry_delay_secs?,
        max_failures: schedule.max_failures,
        failure_count: schedule.failure_count,
        last_error: schedule.last_error,
        next_run_at: schedule.next_run_at?,
        last_run_at: schedule.last_run_at,
        created_at: schedule.created_at,
        updated_at: metadata.updated_at,
    })
}

fn normalize_policy_generation(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn backup_policy_schedule_input(request: &CreateBackupPolicyRequest) -> ScheduleCreateInput {
    ScheduleCreateInput {
        name: request.name.clone(),
        operation: Some(JobCommand::Backup {
            paths: request.paths.clone(),
            include_config: request.include_config,
            follow_symlinks: request.follow_symlinks,
            missing_path_policy: request.missing_path_policy,
        }),
        event_argv_template: None,
        selector_expression: request.selector_expression.clone(),
        target_client_ids: request.target_client_ids.clone(),
        trigger_kind: crate::model::ScheduleTriggerKind::Cron,
        cron_expr: Some(request.cron_expr.clone()),
        timezone: Some(request.timezone.clone()),
        event_expression: None,
        enabled: request.enabled,
        catch_up_policy: Some(request.catch_up_policy.clone()),
        catch_up_limit: Some(request.catch_up_limit),
        retry_delay_secs: Some(request.retry_delay_secs),
        max_failures: request.max_failures,
        expected_definition_revision: None,
    }
}
