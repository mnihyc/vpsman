use anyhow::{bail, Result};
use serde_json::json;
use sqlx::{types::Json as SqlJson, PgPool, Row};
use uuid::Uuid;
use vpsman_object_store::BackupObjectStore;

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
    artifact_id: Uuid,
    object_key: String,
}

pub(crate) async fn process_backup_policy_retention_prune(
    pool: &PgPool,
    config: BackupPolicyRetentionPruneConfig,
) -> Result<BackupPolicyRetentionPruneRun> {
    if !config.enabled {
        return Ok(BackupPolicyRetentionPruneRun::default());
    }
    if config.delete_objects && !config.dry_run && config.object_store.is_none() {
        bail!("backup policy prune object deletion requires configured artifact object store");
    }
    let policies = list_backup_policy_retention_candidates(pool, &config).await?;
    let mut outcomes = Vec::new();
    for policy in &policies {
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
    let rows = sqlx::query(
        r#"
        SELECT
            schedule.id AS schedule_id,
            schedule.name,
            schedule.enabled,
            policy.retention_days,
            policy.keep_last
        FROM backup_policies policy
        JOIN schedules schedule ON schedule.id = policy.schedule_id
        WHERE ($1 OR schedule.enabled = TRUE)
          AND schedule.operation ->> 'type' = 'backup'
        ORDER BY schedule.name ASC, schedule.id ASC
        LIMIT $2
        "#,
    )
    .bind(config.include_disabled)
    .bind(config.limit)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(BackupPolicyRetentionPolicy {
                schedule_id: row.try_get("schedule_id")?,
                name: row.try_get("name")?,
                enabled: row.try_get("enabled")?,
                retention_days: row.try_get("retention_days")?,
                keep_last: row.try_get("keep_last")?,
            })
        })
        .collect()
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
                artifact_id: row.try_get("artifact_id")?,
                object_key: row.try_get("object_key")?,
            })
        })
        .collect::<std::result::Result<Vec<_>, sqlx::Error>>()?;
    let object_delete_attempted = config.delete_objects && !config.dry_run;
    let mut pruned_rows = 0_i64;
    let mut object_delete_errors = Vec::new();
    if !config.dry_run && object_delete_attempted {
        let object_store = config
            .object_store
            .as_ref()
            .expect("object store checked before pruning");
        for candidate in &candidates {
            match object_store.delete_confirmed(&candidate.object_key).await {
                Ok(()) => {
                    pruned_rows +=
                        prune_backup_policy_rows(pool, std::slice::from_ref(candidate)).await?;
                }
                Err(error) => {
                    object_delete_errors.push(format!("{}: {error}", candidate.object_key));
                    break;
                }
            }
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
                artifact.created_at,
                row_number() OVER (
                    PARTITION BY request.client_id
                    ORDER BY artifact.created_at DESC, artifact.id DESC
                ) AS retained_rank
            FROM backup_requests request
            JOIN backup_artifacts artifact ON artifact.id = request.artifact_id
            WHERE request.source_schedule_id = $1
        )
        SELECT request_id, artifact_id, object_key
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
        .map(|candidate| candidate.artifact_id)
        .collect::<Vec<_>>();
    let result = sqlx::query(
        r#"
        WITH doomed AS (
            SELECT *
            FROM unnest($1::uuid[], $2::uuid[]) AS doomed(request_id, artifact_id)
        ),
        cleared_requests AS (
            UPDATE backup_requests request
            SET artifact_id = NULL,
                status = 'requested_metadata_only'
            FROM doomed
            WHERE request.id = doomed.request_id
              AND request.artifact_id = doomed.artifact_id
            RETURNING request.id
        )
        DELETE FROM backup_artifacts artifact
        USING doomed
        WHERE artifact.id = doomed.artifact_id
        "#,
    )
    .bind(request_ids)
    .bind(artifact_ids)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() as i64)
}

async fn insert_prune_audit(
    pool: &PgPool,
    config: BackupPolicyRetentionPruneConfig,
    run: &BackupPolicyRetentionPruneRun,
    outcomes: &[BackupPolicyRetentionPruneOutcome],
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
    .bind("backup_policy.retention_pruned")
    .bind("backup_policy_retention")
    .bind(SqlJson(json!({
        "worker": "backup_policy_retention_worker",
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
mod tests {
    use super::BackupPolicyRetentionPruneConfig;

    #[test]
    fn backup_policy_prune_config_clamps_bounds() {
        let low = BackupPolicyRetentionPruneConfig::new(true, -5, false, false, false, None);
        assert_eq!(low.limit, 1);
        assert!(low.enabled);

        let high = BackupPolicyRetentionPruneConfig::new(true, 50_000, true, true, true, None);
        assert_eq!(high.limit, 1_000);
        assert!(high.dry_run);
        assert!(high.include_disabled);
        assert!(high.delete_objects);
    }

    #[test]
    fn backup_policy_retention_candidate_query_returns_prune_identities() {
        let query = super::backup_policy_retention_candidate_query();
        assert!(query.contains("SELECT request_id, artifact_id, object_key"));
    }
}
