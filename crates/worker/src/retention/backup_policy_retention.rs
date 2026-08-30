use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use sqlx::{types::Json as SqlJson, PgPool, Row};
use uuid::Uuid;
use vpsman_object_store::BackupObjectStore;

use crate::actor_authority::actor_authorized;
use crate::artifact_deletion::{
    defer_artifact_deletion, enqueue_artifact_deletion, finish_artifact_deletion_in_tx,
    lock_owned_artifact_deletion_in_tx, spawn_artifact_deletion_heartbeat, ArtifactDeletionOwner,
    ArtifactDeletionReview,
};

#[derive(Clone, Debug)]
pub(crate) struct BackupPolicyRetentionPruneConfig {
    pub(crate) enabled: bool,
    pub(crate) limit: i64,
    pub(crate) dry_run: bool,
    pub(crate) include_disabled: bool,
    pub(crate) delete_objects: bool,
    pub(crate) object_store: Option<BackupObjectStore>,
}

impl BackupPolicyRetentionPruneConfig {
    pub(crate) fn new(
        enabled: bool,
        limit: i64,
        dry_run: bool,
        include_disabled: bool,
        delete_objects: bool,
        object_store: Option<BackupObjectStore>,
    ) -> Self {
        Self {
            enabled,
            limit: limit.clamp(1, 1_000),
            dry_run,
            include_disabled,
            delete_objects,
            object_store,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct BackupPolicyRetentionPruneRun {
    pub(crate) policies_scanned: usize,
    pub(crate) matched_rows: i64,
    pub(crate) pruned_rows: i64,
}

#[derive(Debug)]
struct BackupPolicyRetentionPolicy {
    schedule_id: Uuid,
    definition_revision: i64,
    actor_id: Option<Uuid>,
    name: String,
    enabled: bool,
    retention_days: i32,
    keep_last: i32,
}

#[derive(Debug)]
struct BackupPolicyRetentionPruneOutcome {
    schedule_id: Uuid,
    name: String,
    enabled: bool,
    retention_days: i32,
    keep_last: i32,
    matched_rows: i64,
    pruned_rows: i64,
    object_key_count: usize,
    object_delete_attempted: bool,
    object_delete_errors: Vec<String>,
}

#[derive(Debug)]
struct BackupPolicyRetentionCandidate {
    request_id: Uuid,
    backup_artifact_id: Uuid,
    server_artifact_id: Option<Uuid>,
    object_key: String,
    sha256_hex: String,
    size_bytes: i64,
}

pub(crate) async fn process_backup_policy_retention_prune(
    pool: &PgPool,
    config: BackupPolicyRetentionPruneConfig,
) -> Result<BackupPolicyRetentionPruneRun> {
    if !config.enabled {
        return Ok(BackupPolicyRetentionPruneRun::default());
    }
    let policies = list_backup_policy_retention_candidates(pool, &config).await?;
    let mut outcomes = Vec::new();
    for policy in &policies {
        // Advance the durable round-robin cursor before doing external work.
        // A failed or interrupted policy is retried after the other policies
        // have had a bounded opportunity to run instead of starving them.
        mark_backup_policy_retention_scanned(pool, policy.schedule_id).await?;
        if !actor_authorized(
            pool,
            policy.actor_id,
            "operator",
            &["backups:write", "schedules:write"],
        )
        .await?
        {
            insert_retention_actor_revoked_audit(pool, policy).await?;
            continue;
        }
        let outcome = prune_backup_policy(pool, policy, &config).await?;
        if outcome.matched_rows > 0 || outcome.pruned_rows > 0 {
            outcomes.push(outcome);
        }
    }
    let run = BackupPolicyRetentionPruneRun {
        policies_scanned: policies.len(),
        matched_rows: outcomes.iter().map(|outcome| outcome.matched_rows).sum(),
        pruned_rows: outcomes.iter().map(|outcome| outcome.pruned_rows).sum(),
    };
    if !outcomes.is_empty() {
        insert_prune_audit(pool, config, &run, &outcomes).await?;
    }
    Ok(run)
}

async fn list_backup_policy_retention_candidates(
    pool: &PgPool,
    config: &BackupPolicyRetentionPruneConfig,
) -> Result<Vec<BackupPolicyRetentionPolicy>> {
    let rows = sqlx::query(backup_policy_retention_policies_query())
        .bind(config.include_disabled)
        .bind(config.limit)
        .fetch_all(pool)
        .await?;

    rows.into_iter()
        .map(|row| {
            Ok(BackupPolicyRetentionPolicy {
                schedule_id: row.try_get("schedule_id")?,
                definition_revision: row.try_get("definition_revision")?,
                actor_id: row.try_get("actor_id")?,
                name: row.try_get("name")?,
                enabled: row.try_get("enabled")?,
                retention_days: row.try_get("retention_days")?,
                keep_last: row.try_get("keep_last")?,
            })
        })
        .collect()
}

fn backup_policy_retention_policies_query() -> &'static str {
    r#"
        SELECT
            schedule.id AS schedule_id,
            schedule.definition_revision,
            schedule.actor_id,
            schedule.name,
            schedule.enabled,
            policy.retention_days,
            policy.keep_last
        FROM backup_policies policy
        JOIN schedules schedule ON schedule.id = policy.schedule_id
        WHERE ($1 OR schedule.enabled = TRUE)
          AND schedule.deleted_at IS NULL
          AND schedule.operation ->> 'type' = 'backup'
        ORDER BY
            policy.retention_scanned_at ASC NULLS FIRST,
            schedule.id ASC
        LIMIT $2
        "#
}

async fn mark_backup_policy_retention_scanned(pool: &PgPool, schedule_id: Uuid) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE backup_policies
        SET retention_scanned_at = now()
        WHERE schedule_id = $1
        "#,
    )
    .bind(schedule_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn prune_backup_policy(
    pool: &PgPool,
    policy: &BackupPolicyRetentionPolicy,
    config: &BackupPolicyRetentionPruneConfig,
) -> Result<BackupPolicyRetentionPruneOutcome> {
    let rows = sqlx::query(backup_policy_retention_candidate_query())
        .bind(policy.schedule_id)
        .bind(policy.keep_last)
        .bind(policy.retention_days)
        .fetch_all(pool)
        .await?;
    let candidates = rows
        .into_iter()
        .map(|row| {
            Ok(BackupPolicyRetentionCandidate {
                request_id: row.try_get("request_id")?,
                backup_artifact_id: row.try_get("artifact_id")?,
                server_artifact_id: row.try_get("server_artifact_id")?,
                object_key: row.try_get("object_key")?,
                sha256_hex: row.try_get("sha256_hex")?,
                size_bytes: row.try_get("size_bytes")?,
            })
        })
        .collect::<std::result::Result<Vec<_>, sqlx::Error>>()?;
    let object_delete_attempted = config.delete_objects && !config.dry_run;
    let mut pruned_rows = 0_i64;
    let mut object_delete_errors = Vec::new();
    if !config.dry_run && object_delete_attempted {
        for candidate in &candidates {
            let Some(server_artifact_id) = candidate.server_artifact_id else {
                object_delete_errors.push(format!(
                    "{}: server artifact registry entry missing",
                    candidate.object_key
                ));
                break;
            };
            enqueue_artifact_deletion(
                pool,
                &ArtifactDeletionReview {
                    artifact_id: server_artifact_id,
                    object_key: candidate.object_key.clone(),
                    sha256_hex: candidate.sha256_hex.clone(),
                    size_bytes: candidate.size_bytes,
                    source_kind: "backup_policy",
                    source_id: policy.schedule_id,
                    source_revision: policy.definition_revision,
                    source_identity: json!({
                        "request_id": candidate.request_id,
                        "backup_artifact_id": candidate.backup_artifact_id,
                        "object_key": candidate.object_key,
                    }),
                },
            )
            .await?;
        }
    } else if !config.dry_run && !candidates.is_empty() {
        pruned_rows = prune_backup_policy_rows(pool, &candidates).await?;
    }
    Ok(BackupPolicyRetentionPruneOutcome {
        schedule_id: policy.schedule_id,
        name: policy.name.clone(),
        enabled: policy.enabled,
        retention_days: policy.retention_days,
        keep_last: policy.keep_last,
        matched_rows: candidates.len() as i64,
        pruned_rows,
        object_key_count: candidates.len(),
        object_delete_attempted,
        object_delete_errors,
    })
}

fn backup_policy_retention_candidate_query() -> &'static str {
    r#"
        WITH ranked AS (
            SELECT
                request.id AS request_id,
                artifact.id AS artifact_id,
                artifact.object_key,
                artifact.sha256_hex,
                artifact.size_bytes,
                server_artifact.id AS server_artifact_id,
                artifact.created_at,
                row_number() OVER (
                    PARTITION BY request.client_id
                    ORDER BY artifact.created_at DESC, artifact.id DESC
                ) AS retained_rank
            FROM backup_requests request
            JOIN backup_artifacts artifact ON artifact.id = request.artifact_id
            LEFT JOIN server_artifacts server_artifact
              ON server_artifact.object_key = artifact.object_key
             AND server_artifact.domain = 'backup_artifact'
             AND server_artifact.sha256_hex = artifact.sha256_hex
             AND server_artifact.size_bytes = artifact.size_bytes
             AND server_artifact.status IN ('active', 'deleting', 'delete_failed')
            WHERE request.source_schedule_id = $1
        )
        SELECT
            request_id,
            artifact_id,
            server_artifact_id,
            object_key,
            sha256_hex,
            size_bytes
        FROM ranked
        WHERE retained_rank > $2
          AND created_at < now() - ($3::int * interval '1 day')
        ORDER BY created_at ASC, artifact_id ASC
        "#
}

async fn prune_backup_policy_rows(
    pool: &PgPool,
    candidates: &[BackupPolicyRetentionCandidate],
) -> Result<i64> {
    let request_ids = candidates
        .iter()
        .map(|candidate| candidate.request_id)
        .collect::<Vec<_>>();
    let artifact_ids = candidates
        .iter()
        .map(|candidate| candidate.backup_artifact_id)
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

pub(crate) async fn delete_backup_policy_artifact(
    pool: &PgPool,
    object_store: &BackupObjectStore,
    owner: &ArtifactDeletionOwner,
) -> Result<i64> {
    ensure!(
        owner.source_kind == "backup_policy",
        "artifact deletion source mismatch"
    );
    ensure!(
        owner.source_revision >= 1,
        "artifact deletion revision invalid"
    );
    let (stop_tx, heartbeat) = spawn_artifact_deletion_heartbeat(pool.clone(), owner.clone());
    let delete_result = object_store.delete_confirmed(&owner.object_key).await;
    let _ = stop_tx.send(());
    let still_owned = heartbeat
        .await
        .context("artifact deletion heartbeat stopped unexpectedly")??;
    if !still_owned {
        bail!("artifact deletion ownership lost");
    }
    if let Err(error) = delete_result {
        let error_text = error.to_string();
        if !defer_artifact_deletion(pool, owner, &error_text).await? {
            bail!("artifact deletion ownership lost after object-store failure");
        }
        return Err(error);
    }
    finalize_backup_policy_artifact_deletion(pool, owner).await
}

async fn finalize_backup_policy_artifact_deletion(
    pool: &PgPool,
    owner: &ArtifactDeletionOwner,
) -> Result<i64> {
    let request_id = deletion_identity_uuid(&owner.source_identity, "request_id")?;
    let backup_artifact_id = deletion_identity_uuid(&owner.source_identity, "backup_artifact_id")?;
    let reviewed_object_key = owner
        .source_identity
        .get("object_key")
        .and_then(Value::as_str)
        .context("backup policy deletion object identity missing")?;
    ensure!(
        reviewed_object_key == owner.object_key,
        "backup policy deletion object identity changed"
    );
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
    .bind(request_id)
    .bind(backup_artifact_id)
    .bind(&owner.object_key)
    .fetch_one(&mut *tx)
    .await?;
    let marked = sqlx::query(
        r#"
        UPDATE server_artifacts
        SET status = 'deleted',
            deleted_at = now()
        WHERE id = $1
          AND object_key = $2
          AND status = 'deleting'
        "#,
    )
    .bind(owner.artifact_id)
    .bind(&owner.object_key)
    .execute(&mut *tx)
    .await?;
    ensure!(
        marked.rows_affected() == 1,
        "artifact registry identity changed during finalization"
    );
    ensure!(
        finish_artifact_deletion_in_tx(&mut tx, owner).await?,
        "artifact deletion ownership lost during finalization"
    );
    tx.commit().await?;
    Ok(pruned_rows)
}

fn deletion_identity_uuid(identity: &Value, field: &str) -> Result<Uuid> {
    let raw = identity
        .get(field)
        .and_then(Value::as_str)
        .with_context(|| format!("backup policy deletion {field} missing"))?;
    Uuid::parse_str(raw).with_context(|| format!("backup policy deletion {field} invalid"))
}

async fn insert_retention_actor_revoked_audit(
    pool: &PgPool,
    policy: &BackupPolicyRetentionPolicy,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO audit_logs (
            id, actor_id, action, target, command_hash, metadata
        )
        VALUES ($1, NULL, $2, $3, NULL, $4)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind("backup_policy.retention_actor_authority_revoked")
    .bind(format!("schedule:{}", policy.schedule_id))
    .bind(SqlJson(json!({
        "worker": "backup_policy_retention_worker",
        "origin_kind": "worker",
        "component": "backup-policy-retention-worker",
        "result": "rejected",
        "schedule_id": policy.schedule_id,
        "name": &policy.name,
        "referenced_operator_id": policy.actor_id,
        "reason": "actor_authority_revoked",
    })))
    .execute(pool)
    .await?;
    Ok(())
}

async fn insert_prune_audit(
    pool: &PgPool,
    config: BackupPolicyRetentionPruneConfig,
    run: &BackupPolicyRetentionPruneRun,
    outcomes: &[BackupPolicyRetentionPruneOutcome],
) -> Result<()> {
    let result = if config.dry_run {
        "previewed"
    } else if outcomes
        .iter()
        .any(|outcome| !outcome.object_delete_errors.is_empty())
    {
        "partial"
    } else {
        "succeeded"
    };
    sqlx::query(
        r#"
        INSERT INTO audit_logs (
            id, actor_id, action, target, command_hash, metadata
        )
        VALUES ($1, NULL, $2, $3, NULL, $4)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind("backup_policy.retention_pruned")
    .bind("backup_policy_retention")
    .bind(SqlJson(json!({
        "worker": "backup_policy_retention_worker",
        "origin_kind": "worker",
        "component": "backup-policy-retention-worker",
        "result": result,
        "dry_run": config.dry_run,
        "metadata_only": config.dry_run || !config.delete_objects,
        "object_delete_requested": config.delete_objects,
        "object_delete_configured": config.object_store.is_some(),
        "include_disabled": config.include_disabled,
        "limit": config.limit,
        "policies_scanned": run.policies_scanned,
        "matched_rows": run.matched_rows,
        "pruned_rows": run.pruned_rows,
        "policies": outcomes.iter().map(|outcome| json!({
            "schedule_id": outcome.schedule_id,
            "name": outcome.name,
            "enabled": outcome.enabled,
            "retention_days": outcome.retention_days,
            "keep_last": outcome.keep_last,
            "matched_rows": outcome.matched_rows,
            "pruned_rows": outcome.pruned_rows,
            "object_key_count": outcome.object_key_count,
            "object_delete_attempted": outcome.object_delete_attempted,
            "object_delete_errors": outcome.object_delete_errors.len(),
            "object_delete_error_messages": &outcome.object_delete_errors,
        })).collect::<Vec<_>>(),
    })))
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
#[path = "tests_backup_policy_retention.rs"]
mod tests;
