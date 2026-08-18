use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use tracing::warn;
use uuid::Uuid;

const POLICY_EVIDENCE_ARM_LOCK: &str = "vpsman.alert_policy_evidence_arm";
const RETENTION_LOCK: &str = "vpsman.alert_policy_retention";
const REQUIRED_RETENTION_INDEXES: [&str; 7] = [
    "alert_policy_evaluation_states_last_evidence_id_idx",
    "alert_policy_evaluation_states_last_evidence_seq_idx",
    "alert_policy_confirmations_evidence_idx",
    "alert_policy_evidence_receipts_evidence_id_idx",
    "alert_episodes_trigger_evidence_idx",
    "alert_episodes_last_evidence_idx",
    "schedule_event_receipts_event_idx",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AlertPolicyRetentionConfig {
    lifecycle_retention_days: i64,
    prune_limit: i64,
}

impl AlertPolicyRetentionConfig {
    pub(crate) fn new(lifecycle_retention_days: i64, prune_limit: i64) -> Self {
        Self {
            lifecycle_retention_days: lifecycle_retention_days.clamp(1, 3_650),
            prune_limit: prune_limit.clamp(1, 10_000),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct AlertPolicyRetentionRun {
    pub(crate) skipped_missing_indexes: bool,
    pub(crate) skipped_busy: bool,
    pub(crate) evidence_scanned: usize,
    pub(crate) evidence_receipts_pruned: usize,
    pub(crate) evidence_pruned: usize,
    pub(crate) evidence_pruned_through_seq: i64,
    pub(crate) schedule_dependencies_pruned: usize,
    pub(crate) schedule_receipts_pruned: usize,
    pub(crate) webhook_receipts_pruned: usize,
    pub(crate) lifecycle_events_pruned: usize,
}

#[derive(Clone, Copy, Debug)]
struct EvidenceScanRow {
    id: Uuid,
    evidence_seq: i64,
    created_at: DateTime<Utc>,
}

pub(crate) async fn process_alert_policy_retention(
    pool: &PgPool,
    config: AlertPolicyRetentionConfig,
) -> Result<AlertPolicyRetentionRun> {
    let missing_indexes = missing_retention_indexes(pool).await?;
    if !missing_indexes.is_empty() {
        warn!(
            indexes = ?missing_indexes,
            "alert policy retention skipped because required bounded-lookup indexes are absent"
        );
        return Ok(AlertPolicyRetentionRun {
            skipped_missing_indexes: true,
            ..AlertPolicyRetentionRun::default()
        });
    }

    let mut run = match prune_policy_evidence(pool, config.prune_limit).await {
        Ok(run) => run,
        Err(error) if is_lock_timeout(&error) => {
            warn!(%error, "alert policy retention skipped because its bounded lock wait expired");
            return Ok(AlertPolicyRetentionRun {
                skipped_busy: true,
                ..AlertPolicyRetentionRun::default()
            });
        }
        Err(error) => return Err(error),
    };
    let lifecycle = match prune_lifecycle_events(pool, config).await {
        Ok(run) => run,
        Err(error) if is_lock_timeout(&error) => {
            warn!(%error, "alert lifecycle retention skipped because its bounded lock wait expired");
            run.skipped_busy = true;
            return Ok(run);
        }
        Err(error) => return Err(error),
    };
    run.schedule_dependencies_pruned = lifecycle.schedule_dependencies_pruned;
    run.schedule_receipts_pruned = lifecycle.schedule_receipts_pruned;
    run.webhook_receipts_pruned = lifecycle.webhook_receipts_pruned;
    run.lifecycle_events_pruned = lifecycle.lifecycle_events_pruned;
    Ok(run)
}

fn is_lock_timeout(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<sqlx::Error>(),
        Some(sqlx::Error::Database(database)) if database.code().as_deref() == Some("55P03")
    )
}

async fn missing_retention_indexes(pool: &PgPool) -> Result<Vec<String>> {
    let rows = sqlx::query_scalar::<_, String>(
        r#"
        SELECT class.relname
        FROM pg_class class
        JOIN pg_namespace namespace ON namespace.oid = class.relnamespace
        WHERE namespace.nspname = current_schema()
          AND class.relkind = 'i'
          AND class.relname = ANY($1::text[])
        "#,
    )
    .bind(REQUIRED_RETENTION_INDEXES.as_slice())
    .fetch_all(pool)
    .await?;
    let mut missing = REQUIRED_RETENTION_INDEXES
        .iter()
        .filter(|required| !rows.iter().any(|present| present == **required))
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
    let lifecycle_cursor_present: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM information_schema.columns
            WHERE table_schema = current_schema()
              AND table_name = 'alert_policy_lifecycle_meta'
              AND column_name = 'lifecycle_retention_cursor_seq'
        )
        "#,
    )
    .fetch_one(pool)
    .await?;
    if !lifecycle_cursor_present {
        missing.push("alert_policy_lifecycle_meta.lifecycle_retention_cursor_seq".to_string());
    }
    missing.sort();
    Ok(missing)
}

async fn prune_policy_evidence(pool: &PgPool, prune_limit: i64) -> Result<AlertPolicyRetentionRun> {
    let mut tx = pool.begin().await?;
    set_retention_transaction_bounds(&mut tx).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1)::bigint)")
        .bind(RETENTION_LOCK)
        .execute(&mut *tx)
        .await?;
    // Evidence writers and due/repair evaluators hold the shared side of this
    // fence. Draining them makes receipt deletion plus evidence deletion one
    // atomic, immutable-prefix operation.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1)::bigint)")
        .bind(POLICY_EVIDENCE_ARM_LOCK)
        .execute(&mut *tx)
        .await?;

    let meta = sqlx::query(
        r#"
        SELECT evidence_pruned_through_seq,
               clock_timestamp()
                   - (evidence_retention_days::bigint * interval '1 day') AS cutoff
        FROM alert_policy_lifecycle_meta
        WHERE singleton
        FOR UPDATE
        "#,
    )
    .fetch_one(&mut *tx)
    .await?;
    let previous_waterline: i64 = meta.try_get("evidence_pruned_through_seq")?;
    let cutoff: DateTime<Utc> = meta.try_get("cutoff")?;
    let rows = sqlx::query(
        r#"
        SELECT id, evidence_seq, created_at
        FROM alert_policy_evidence
        WHERE evidence_seq > $1
        ORDER BY evidence_seq ASC
        LIMIT $2
        FOR UPDATE
        "#,
    )
    .bind(previous_waterline)
    .bind(prune_limit)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|row| {
        Ok(EvidenceScanRow {
            id: row.try_get("id")?,
            evidence_seq: row.try_get("evidence_seq")?,
            created_at: row.try_get("created_at")?,
        })
    })
    .collect::<Result<Vec<_>>>()?;

    // Sequence allocation is fenced but created_at is authoritative for age.
    // Never cross the first not-yet-expired record in the durable prefix.
    let mut expired = rows
        .into_iter()
        .take_while(|row| row.created_at <= cutoff)
        .collect::<Vec<_>>();
    if expired.is_empty() {
        tx.commit().await?;
        return Ok(AlertPolicyRetentionRun {
            evidence_pruned_through_seq: previous_waterline,
            ..AlertPolicyRetentionRun::default()
        });
    }

    // A current enabled rule must terminally consume a post-arm fact before
    // the retention waterline can pass it. Transient evaluator failures do not
    // create receipts, so repair gets another chance on the next worker tick.
    let expired_ids = expired.iter().map(|row| row.id).collect::<Vec<_>>();
    let first_unsettled: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT min(evidence.evidence_seq)
        FROM alert_policy_evidence evidence
        WHERE evidence.id = ANY($1::uuid[])
          AND (
              EXISTS (
                  SELECT 1
                  FROM alert_policy_evidence_receipts recent_receipt
                  WHERE recent_receipt.evidence_id = evidence.id
                    AND recent_receipt.evaluated_at > $2
              )
              OR EXISTS (
                  SELECT 1
                  FROM policy_rules rule
                  JOIN policy_groups group_row ON group_row.id = rule.group_id
                  WHERE rule.enabled
                    AND group_row.enabled
                    AND rule.evidence_source = evidence.source_kind
                    AND evidence.evidence_seq > rule.armed_after_evidence_seq
                    AND NOT EXISTS (
                        SELECT 1
                        FROM alert_policy_evidence_receipts receipt
                        WHERE receipt.policy_rule_id = rule.id
                          AND receipt.rule_version = rule.rule_version
                          AND receipt.evidence_seq = evidence.evidence_seq
                    )
              )
          )
        "#,
    )
    .bind(&expired_ids)
    .bind(cutoff)
    .fetch_one(&mut *tx)
    .await?;
    if let Some(first_unsettled) = first_unsettled {
        expired.retain(|row| row.evidence_seq < first_unsettled);
    }
    if expired.is_empty() {
        tx.commit().await?;
        return Ok(AlertPolicyRetentionRun {
            evidence_pruned_through_seq: previous_waterline,
            ..AlertPolicyRetentionRun::default()
        });
    }

    let scanned = expired.len();
    let waterline = expired
        .last()
        .map(|row| row.evidence_seq)
        .unwrap_or(previous_waterline);
    let settled_ids = expired.iter().map(|row| row.id).collect::<Vec<_>>();
    let candidates = sqlx::query(
        r#"
        SELECT evidence.id, evidence.evidence_seq,
               count(receipt.evidence_seq)::bigint AS receipt_count
        FROM alert_policy_evidence evidence
        LEFT JOIN alert_policy_evidence_receipts receipt
          ON receipt.evidence_id = evidence.id
        WHERE evidence.id = ANY($1::uuid[])
          AND (
              evidence.fact_kind = 'occurrence'
              OR EXISTS (
                  SELECT 1
                  FROM alert_policy_evidence newer
                  WHERE newer.source_kind = evidence.source_kind
                    AND newer.natural_key = evidence.natural_key
                    AND (
                        evidence.source_event_id LIKE 'scope:%'
                        OR newer.source_event_id NOT LIKE 'scope:%'
                    )
                    AND (
                        newer.observed_at > evidence.observed_at
                        OR (
                            newer.observed_at = evidence.observed_at
                            AND newer.evidence_seq > evidence.evidence_seq
                        )
                    )
              )
          )
          AND NOT EXISTS (
              SELECT 1 FROM alert_policy_evaluation_states state
              WHERE state.last_evidence_id = evidence.id
                 OR state.last_evidence_seq = evidence.evidence_seq
          )
          AND NOT EXISTS (
              SELECT 1 FROM alert_policy_confirmations confirmation
              WHERE confirmation.evidence_id = evidence.id
          )
          AND NOT EXISTS (
              SELECT 1 FROM alert_episodes episode
              WHERE episode.trigger_evidence_id = evidence.id
                 OR episode.last_evidence_id = evidence.id
          )
        GROUP BY evidence.id, evidence.evidence_seq
        ORDER BY evidence.evidence_seq ASC
        "#,
    )
    .bind(&settled_ids)
    .fetch_all(&mut *tx)
    .await?;

    // Receipt fan-out is independently bounded. A pathological fact with more
    // receipts than one batch is retained intact rather than partially pruned
    // and then re-created by receipt repair.
    let mut selected_ids = Vec::new();
    let mut selected_receipts = 0_i64;
    for candidate in candidates {
        let id: Uuid = candidate.try_get("id")?;
        let receipt_count: i64 = candidate.try_get("receipt_count")?;
        if receipt_count > prune_limit
            || selected_receipts.saturating_add(receipt_count) > prune_limit
            || selected_ids.len() >= prune_limit as usize
        {
            continue;
        }
        selected_receipts += receipt_count;
        selected_ids.push(id);
    }

    let receipts_pruned = if selected_ids.is_empty() {
        0
    } else {
        sqlx::query(
            r#"
            DELETE FROM alert_policy_evidence_receipts
            WHERE evidence_id = ANY($1::uuid[])
            "#,
        )
        .bind(&selected_ids)
        .execute(&mut *tx)
        .await?
        .rows_affected() as usize
    };
    let evidence_pruned = if selected_ids.is_empty() {
        0
    } else {
        sqlx::query(
            r#"
            DELETE FROM alert_policy_evidence
            WHERE id = ANY($1::uuid[])
            "#,
        )
        .bind(&selected_ids)
        .execute(&mut *tx)
        .await?
        .rows_affected() as usize
    };
    sqlx::query(
        r#"
        UPDATE alert_policy_lifecycle_meta
        SET evidence_pruned_through_seq = GREATEST(evidence_pruned_through_seq, $1)
        WHERE singleton
        "#,
    )
    .bind(waterline)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(AlertPolicyRetentionRun {
        evidence_scanned: scanned,
        evidence_receipts_pruned: receipts_pruned,
        evidence_pruned,
        evidence_pruned_through_seq: waterline,
        ..AlertPolicyRetentionRun::default()
    })
}

async fn prune_lifecycle_events(
    pool: &PgPool,
    config: AlertPolicyRetentionConfig,
) -> Result<AlertPolicyRetentionRun> {
    let mut tx = pool.begin().await?;
    set_retention_transaction_bounds(&mut tx).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1)::bigint)")
        .bind(RETENTION_LOCK)
        .execute(&mut *tx)
        .await?;

    let retention_cursor: i64 = sqlx::query_scalar(
        r#"
        SELECT lifecycle_retention_cursor_seq
        FROM alert_policy_lifecycle_meta
        WHERE singleton
        FOR UPDATE
        "#,
    )
    .fetch_one(&mut *tx)
    .await?;
    let consumer_watermark: i64 = sqlx::query_scalar(
        r#"
        SELECT CASE WHEN count(*) = 2 THEN min(last_event_seq) ELSE 0 END
        FROM alert_lifecycle_consumer_cursors
        WHERE consumer_kind IN ('webhook', 'schedule')
        "#,
    )
    .fetch_one(&mut *tx)
    .await?;
    let rows = sqlx::query(
        r#"
        SELECT event_seq,
               created_at <= clock_timestamp()
                   - ($3::bigint * interval '1 day') AS retention_age_met
        FROM alert_lifecycle_events
        WHERE event_seq > $1 AND event_seq <= $2
        ORDER BY event_seq ASC
        LIMIT $4
        FOR UPDATE
        "#,
    )
    .bind(retention_cursor)
    .bind(consumer_watermark)
    .bind(config.lifecycle_retention_days)
    .bind(config.prune_limit)
    .fetch_all(&mut *tx)
    .await?;
    if rows.is_empty() {
        if retention_cursor != 0 {
            sqlx::query(
                r#"
                UPDATE alert_policy_lifecycle_meta
                SET lifecycle_retention_cursor_seq = 0
                WHERE singleton
                "#,
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        return Ok(AlertPolicyRetentionRun::default());
    }
    let next_cursor: i64 = rows
        .last()
        .expect("nonempty lifecycle retention scan")
        .try_get("event_seq")?;
    let mut expired_event_seqs = Vec::new();
    for row in rows {
        if row.try_get::<bool, _>("retention_age_met")? {
            expired_event_seqs.push(row.try_get::<i64, _>("event_seq")?);
        }
    }
    sqlx::query(
        r#"
        UPDATE alert_policy_lifecycle_meta
        SET lifecycle_retention_cursor_seq = $1
        WHERE singleton
        "#,
    )
    .bind(next_cursor)
    .execute(&mut *tx)
    .await?;
    if expired_event_seqs.is_empty() {
        tx.commit().await?;
        return Ok(AlertPolicyRetentionRun::default());
    }

    let safe_event_seqs = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT lifecycle.event_seq
        FROM alert_lifecycle_events lifecycle
        JOIN alert_lifecycle_webhook_receipts webhook_receipt
          ON webhook_receipt.event_seq = lifecycle.event_seq
        WHERE lifecycle.event_seq = ANY($1::bigint[])
          AND webhook_receipt.status = 'projected'
          AND NOT EXISTS (
              SELECT 1
              FROM webhook_events webhook_event
              WHERE webhook_event.occurred_at = webhook_receipt.webhook_event_occurred_at
                AND webhook_event.id = webhook_receipt.webhook_event_id
                AND webhook_event.processed_at IS NULL
          )
          AND NOT EXISTS (
              SELECT 1
              FROM webhook_rule_deliveries delivery
              WHERE delivery.event_kind = lifecycle.edge_kind
                AND delivery.event_id = lifecycle.event_id
                AND delivery.status IN ('queued', 'in_progress', 'failed')
          )
          AND NOT EXISTS (
              SELECT 1
              FROM schedule_event_receipts schedule_receipt
              LEFT JOIN jobs dispatched_job ON dispatched_job.id = schedule_receipt.job_id
              WHERE schedule_receipt.event_seq = lifecycle.event_seq
                AND (
                    schedule_receipt.status = 'pending'
                    OR (
                        schedule_receipt.status = 'dispatched'
                        AND (
                            schedule_receipt.job_id IS NULL
                            OR dispatched_job.completed_at IS NULL
                        )
                    )
                )
          )
        ORDER BY lifecycle.event_seq ASC
        "#,
    )
    .bind(&expired_event_seqs)
    .fetch_all(&mut *tx)
    .await?;
    if safe_event_seqs.is_empty() {
        tx.commit().await?;
        return Ok(AlertPolicyRetentionRun::default());
    }

    let schedule_dependencies_pruned = sqlx::query(
        r#"
        DELETE FROM schedule_event_dependencies dependency
        USING schedule_event_receipts receipt
        WHERE dependency.receipt_id = receipt.id
          AND receipt.event_seq = ANY($1::bigint[])
        "#,
    )
    .bind(&safe_event_seqs)
    .execute(&mut *tx)
    .await?
    .rows_affected() as usize;
    let schedule_receipts_pruned = sqlx::query(
        r#"
        DELETE FROM schedule_event_receipts
        WHERE event_seq = ANY($1::bigint[])
        "#,
    )
    .bind(&safe_event_seqs)
    .execute(&mut *tx)
    .await?
    .rows_affected() as usize;
    let webhook_receipts_pruned = sqlx::query(
        r#"
        DELETE FROM alert_lifecycle_webhook_receipts
        WHERE event_seq = ANY($1::bigint[])
        "#,
    )
    .bind(&safe_event_seqs)
    .execute(&mut *tx)
    .await?
    .rows_affected() as usize;
    let lifecycle_events_pruned = sqlx::query(
        r#"
        DELETE FROM alert_lifecycle_events
        WHERE event_seq = ANY($1::bigint[])
        "#,
    )
    .bind(&safe_event_seqs)
    .execute(&mut *tx)
    .await?
    .rows_affected() as usize;
    tx.commit().await?;

    Ok(AlertPolicyRetentionRun {
        schedule_dependencies_pruned,
        schedule_receipts_pruned,
        webhook_receipts_pruned,
        lifecycle_events_pruned,
        ..AlertPolicyRetentionRun::default()
    })
}

async fn set_retention_transaction_bounds(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<()> {
    sqlx::query("SET LOCAL lock_timeout = '2s'")
        .execute(&mut **tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout = '15s'")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[cfg(test)]
#[path = "tests_alert_policy_retention.rs"]
mod tests;
