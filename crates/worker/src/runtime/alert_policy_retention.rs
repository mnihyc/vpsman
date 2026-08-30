use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use tracing::warn;
use uuid::Uuid;

const REQUIRED_RETENTION_INDEXES: [&str; 17] = [
    "alert_policy_evaluation_states_active_episode_idx",
    "alert_policy_evaluation_states_last_evidence_id_idx",
    "alert_policy_evaluation_states_last_evidence_seq_idx",
    "alert_policy_confirmations_evidence_idx",
    "alert_policy_evidence_receipts_evidence_id_idx",
    "alert_episodes_trigger_evidence_idx",
    "alert_episodes_last_evidence_idx",
    "alert_episodes_identity_idx",
    "alert_policy_evidence_prune_candidates_retry_idx",
    "schedule_event_receipts_event_idx",
    "schedule_event_receipts_episode_idx",
    "alert_lifecycle_events_episode_idx",
    "alert_episodes_resolved_retention_idx",
    "webhook_events_kind_idx",
    "webhook_events_processed_retention_idx",
    "webhook_rule_deliveries_event_idx",
    "fleet_alert_notification_deliveries_alert_idx",
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

impl Default for AlertPolicyRetentionConfig {
    fn default() -> Self {
        // Keep alert lifecycle ownership independent of webhook delivery
        // settings while retaining the independent 90-day history horizon.
        Self::new(90, 1_000)
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
    pub(crate) consumer_receipts_pruned: usize,
    pub(crate) lifecycle_events_pruned: usize,
    pub(crate) episode_evidence_enqueued: usize,
    pub(crate) resolved_episodes_pruned: usize,
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
        Err(error) if is_retention_lock_timeout(&error) => {
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
        Err(error) if is_retention_lock_timeout(&error) => {
            warn!(%error, "alert lifecycle retention skipped because its bounded lock wait expired");
            run.skipped_busy = true;
            return Ok(run);
        }
        Err(error) => return Err(error),
    };
    run.schedule_dependencies_pruned = lifecycle.schedule_dependencies_pruned;
    run.schedule_receipts_pruned = lifecycle.schedule_receipts_pruned;
    run.consumer_receipts_pruned = lifecycle.consumer_receipts_pruned;
    run.lifecycle_events_pruned = lifecycle.lifecycle_events_pruned;
    run.episode_evidence_enqueued = lifecycle.episode_evidence_enqueued;
    run.resolved_episodes_pruned = lifecycle.resolved_episodes_pruned;
    Ok(run)
}

fn is_retention_lock_timeout(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<sqlx::Error>(),
        Some(sqlx::Error::Database(database))
            if is_retention_lock_timeout_code(database.code().as_deref())
    )
}

fn is_retention_lock_timeout_code(code: Option<&str>) -> bool {
    code == Some("55P03")
}

async fn missing_retention_indexes(pool: &PgPool) -> Result<Vec<String>> {
    let rows = sqlx::query_scalar::<_, String>(
        r#"
        SELECT class.relname
        FROM pg_class class
        JOIN pg_namespace namespace ON namespace.oid = class.relnamespace
        WHERE namespace.nspname = current_schema()
          AND class.relkind IN ('i', 'I')
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
    missing.sort();
    Ok(missing)
}

async fn prune_policy_evidence(pool: &PgPool, prune_limit: i64) -> Result<AlertPolicyRetentionRun> {
    // A round boundary is a snapshot of durable work, not a throughput cap.
    // Rows committed after it belong to the next recovery round, so a live
    // producer cannot make this drain infinite.
    let scan_through_seq: i64 =
        sqlx::query_scalar("SELECT COALESCE(max(evidence_seq), 0) FROM alert_policy_evidence")
            .fetch_one(pool)
            .await?;
    let mut run = AlertPolicyRetentionRun::default();
    loop {
        let page =
            process_policy_evidence_page(pool, prune_limit, Some(scan_through_seq), None).await?;
        run.evidence_scanned = run
            .evidence_scanned
            .saturating_add(page.run.evidence_scanned);
        run.evidence_pruned_through_seq = run
            .evidence_pruned_through_seq
            .max(page.run.evidence_pruned_through_seq);
        if page.run.evidence_scanned < prune_limit as usize {
            break;
        }
        tokio::task::yield_now().await;
    }

    // Enqueue is complete before the consumer frontier is captured. Every
    // candidate already durable at this instant is attempted at most once in
    // this pass; referenced survivors rotate behind independent candidates.
    let candidate_round_started_at: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(pool)
        .await?;
    loop {
        let page =
            process_policy_evidence_page(pool, prune_limit, None, Some(candidate_round_started_at))
                .await?;
        run.evidence_receipts_pruned = run
            .evidence_receipts_pruned
            .saturating_add(page.run.evidence_receipts_pruned);
        run.evidence_pruned = run.evidence_pruned.saturating_add(page.run.evidence_pruned);
        if page.candidates_attempted < prune_limit as usize {
            break;
        }
        tokio::task::yield_now().await;
    }
    Ok(run)
}

struct PolicyEvidenceRetentionPage {
    run: AlertPolicyRetentionRun,
    candidates_attempted: usize,
}

async fn process_policy_evidence_page(
    pool: &PgPool,
    prune_limit: i64,
    scan_through_seq: Option<i64>,
    candidate_round_started_at: Option<DateTime<Utc>>,
) -> Result<PolicyEvidenceRetentionPage> {
    debug_assert!(scan_through_seq.is_some() ^ candidate_round_started_at.is_some());
    let mut tx = pool.begin().await?;
    set_retention_transaction_bounds(&mut tx).await?;

    // The singleton waterline row is the natural owner of the ordered prefix;
    // candidate evidence rows are the independent deletion owners. Every
    // durable reference is checked both while selecting and while deleting,
    // and schema FKs linearize a concurrent reference against the parent-row
    // delete. No process- or repository-global retention lock is required.

    let meta_sql = if scan_through_seq.is_some() {
        r#"
        SELECT evidence_pruned_through_seq,
               clock_timestamp()
                   - (evidence_retention_days::bigint * interval '1 day') AS cutoff
        FROM alert_policy_lifecycle_meta
        WHERE singleton
        FOR UPDATE
        "#
    } else {
        r#"
        SELECT evidence_pruned_through_seq,
               clock_timestamp()
                   - (evidence_retention_days::bigint * interval '1 day') AS cutoff
        FROM alert_policy_lifecycle_meta
        WHERE singleton
        "#
    };
    let meta = sqlx::query(meta_sql).fetch_one(&mut *tx).await?;
    let previous_waterline: i64 = meta.try_get("evidence_pruned_through_seq")?;
    let cutoff: DateTime<Utc> = meta.try_get("cutoff")?;

    // Preserve the configured window for occurrence evidence, but enqueue one
    // bounded immutable prefix instead of assuming every row can be
    // deleted as the sequence waterline crosses it. Referenced rows remain in
    // the fair queue after the waterline advances. Superseded metric and state
    // facts enter the same queue directly through the current-pointer triggers;
    // occurrence facts retain the configured history window.
    let (expired_scanned, waterline) = if let Some(scan_through_seq) = scan_through_seq {
        let expired_scan = sqlx::query(
            r#"
            WITH bounded AS MATERIALIZED (
                SELECT id, evidence_seq, source_kind,
                       subject_client_id, natural_key, created_at
                FROM alert_policy_evidence
                WHERE evidence_seq > $1 AND evidence_seq <= $2
                ORDER BY evidence_seq ASC
                LIMIT $3
            ), prefix AS MATERIALIZED (
                SELECT id, evidence_seq, source_kind,
                       subject_client_id, natural_key
                FROM bounded
                WHERE evidence_seq < COALESCE(
                    (SELECT min(evidence_seq)
                     FROM bounded
                     WHERE created_at > $4),
                    9223372036854775807::bigint
                )
            ), enqueued AS (
                INSERT INTO alert_policy_evidence_prune_candidates (
                    evidence_id, source_kind, subject_client_id, natural_key
                )
                SELECT id, source_kind, subject_client_id, natural_key
                FROM prefix
                ON CONFLICT (evidence_id) DO NOTHING
                RETURNING evidence_id
            )
            SELECT count(*)::bigint AS scanned,
                   COALESCE(max(evidence_seq), $1)::bigint AS waterline,
                   (SELECT count(*) FROM enqueued)::bigint AS enqueued
            FROM prefix
            "#,
        )
        .bind(previous_waterline)
        .bind(scan_through_seq)
        .bind(prune_limit)
        .bind(cutoff)
        .fetch_one(&mut *tx)
        .await?;
        (
            expired_scan.try_get::<i64, _>("scanned")? as usize,
            expired_scan.try_get("waterline")?,
        )
    } else {
        (0, previous_waterline)
    };

    // This asynchronous retention transaction is the only evidence-deletion
    // owner. Lock retry rows before evidence rows. Pending evaluation shares
    // the evidence-row lock but never takes the retry-row lock, so it can make
    // retention wait without forming the reverse half of a cycle. Rotating
    // every attempted survivor prevents
    // a permanent current fact from starving an older fact after its temporary
    // reference disappears.
    let candidate_ids = if let Some(round_started_at) = candidate_round_started_at {
        sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT candidate.evidence_id
            FROM alert_policy_evidence_prune_candidates candidate
            WHERE candidate.enqueued_at <= $2
              AND (
                    candidate.last_attempted_at IS NULL
                    OR candidate.last_attempted_at < $2
                  )
            ORDER BY candidate.last_attempted_at ASC NULLS FIRST,
                     candidate.enqueued_at ASC,
                     candidate.evidence_id ASC
            LIMIT $1
            FOR UPDATE OF candidate SKIP LOCKED
            "#,
        )
        .bind(prune_limit)
        .bind(round_started_at)
        .fetch_all(&mut *tx)
        .await?
    } else {
        Vec::new()
    };
    if let Some(round_started_at) = candidate_round_started_at.filter(|_| !candidate_ids.is_empty())
    {
        sqlx::query(
            r#"
            UPDATE alert_policy_evidence_prune_candidates
            SET last_attempted_at = $2
            WHERE evidence_id = ANY($1::uuid[])
            "#,
        )
        .bind(&candidate_ids)
        .bind(round_started_at)
        .execute(&mut *tx)
        .await?;
    }

    let eligible_ids = if candidate_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT evidence.id
            FROM alert_policy_evidence evidence
            WHERE evidence.id = ANY($1::uuid[])
              AND NOT evidence.evaluation_pending
              -- Displaced metric/state facts are stream history and are
              -- eligible without waiting for the history-age cutoff. Their
              -- current/effective, episode, confirmation, evaluation-state
              -- and receipt ownership checks below are unchanged.
              AND (
                  evidence.fact_kind IN ('metric', 'state')
                  OR evidence.fact_kind = 'occurrence'
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
                  OR (
                      evidence.subject_client_id IS NOT NULL
                      AND NOT EXISTS (
                          SELECT 1 FROM clients client
                          WHERE client.id = evidence.subject_client_id
                      )
                  )
              )
              AND (
                  evidence.fact_kind IN ('metric', 'state')
                  OR (
                      evidence.created_at <= $2
                      AND NOT EXISTS (
                          SELECT 1
                          FROM alert_policy_evidence_receipts recent_receipt
                          WHERE recent_receipt.evidence_id = evidence.id
                            AND recent_receipt.evaluated_at > $2
                      )
                  )
              )
              AND NOT EXISTS (
                  SELECT 1 FROM alert_policy_current_evidence current_evidence
                  WHERE current_evidence.evidence_id = evidence.id
              )
              AND NOT EXISTS (
                  SELECT 1
                  FROM alert_policy_effective_current_evidence effective_evidence
                  WHERE effective_evidence.evidence_id = evidence.id
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
            ORDER BY evidence.evidence_seq ASC
            FOR UPDATE OF evidence SKIP LOCKED
            "#,
        )
        .bind(&candidate_ids)
        .bind(cutoff)
        .fetch_all(&mut *tx)
        .await?
    };

    // The limit is an evidence-row budget, not a receipt-row budget. Delete
    // every terminal receipt for a selected fact atomically before that fact,
    // keeping throughput independent of enabled-rule fan-out.
    let receipts_pruned = if eligible_ids.is_empty() {
        0
    } else {
        sqlx::query(
            r#"
            DELETE FROM alert_policy_evidence_receipts
            WHERE evidence_id = ANY($1::uuid[])
            "#,
        )
        .bind(&eligible_ids)
        .execute(&mut *tx)
        .await?
        .rows_affected() as usize
    };
    let evidence_pruned = if eligible_ids.is_empty() {
        0
    } else {
        sqlx::query(
            r#"
            DELETE FROM alert_policy_evidence candidate
            WHERE candidate.id = ANY($1::uuid[])
              AND NOT candidate.evaluation_pending
              AND NOT EXISTS (
                  SELECT 1 FROM alert_policy_current_evidence current_evidence
                  WHERE current_evidence.evidence_id = candidate.id
              )
              AND NOT EXISTS (
                  SELECT 1
                  FROM alert_policy_effective_current_evidence effective_evidence
                  WHERE effective_evidence.evidence_id = candidate.id
              )
              AND NOT EXISTS (
                  SELECT 1 FROM alert_policy_confirmations confirmation
                  WHERE confirmation.evidence_id = candidate.id
              )
              AND NOT EXISTS (
                  SELECT 1 FROM alert_policy_evaluation_states state
                  WHERE state.last_evidence_id = candidate.id
                     OR state.last_evidence_seq = candidate.evidence_seq
              )
              AND NOT EXISTS (
                  SELECT 1 FROM alert_episodes episode
                  WHERE episode.trigger_evidence_id = candidate.id
                     OR episode.last_evidence_id = candidate.id
              )
              AND NOT EXISTS (
                  SELECT 1 FROM alert_policy_evidence_receipts receipt
                  WHERE receipt.evidence_id = candidate.id
              )
            "#,
        )
        .bind(&eligible_ids)
        .execute(&mut *tx)
        .await?
        .rows_affected() as usize
    };
    anyhow::ensure!(
        evidence_pruned == eligible_ids.len(),
        "alert policy evidence retention lost its locked eligibility"
    );
    if scan_through_seq.is_some() {
        sqlx::query(
            r#"
            UPDATE alert_policy_lifecycle_meta
            SET evidence_pruned_through_seq = GREATEST(
                    evidence_pruned_through_seq, $1
                )
            WHERE singleton
            "#,
        )
        .bind(waterline)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    Ok(PolicyEvidenceRetentionPage {
        run: AlertPolicyRetentionRun {
            evidence_scanned: expired_scanned,
            evidence_receipts_pruned: receipts_pruned,
            evidence_pruned,
            evidence_pruned_through_seq: waterline,
            ..AlertPolicyRetentionRun::default()
        },
        candidates_attempted: candidate_ids.len(),
    })
}

async fn prune_lifecycle_events(
    pool: &PgPool,
    config: AlertPolicyRetentionConfig,
) -> Result<AlertPolicyRetentionRun> {
    let round = sqlx::query(
        r#"
        SELECT clock_timestamp() AS started_at,
               COALESCE(max(event_seq), 0)::bigint AS event_seq_through
        FROM alert_lifecycle_events
        "#,
    )
    .fetch_one(pool)
    .await?;
    let round_started_at: DateTime<Utc> = round.try_get("started_at")?;
    let event_seq_through: i64 = round.try_get("event_seq_through")?;
    let mut run = AlertPolicyRetentionRun::default();
    loop {
        let page =
            prune_lifecycle_events_page(pool, config, event_seq_through, round_started_at).await?;
        run.schedule_dependencies_pruned = run
            .schedule_dependencies_pruned
            .saturating_add(page.schedule_dependencies_pruned);
        run.schedule_receipts_pruned = run
            .schedule_receipts_pruned
            .saturating_add(page.schedule_receipts_pruned);
        run.consumer_receipts_pruned = run
            .consumer_receipts_pruned
            .saturating_add(page.consumer_receipts_pruned);
        run.lifecycle_events_pruned = run
            .lifecycle_events_pruned
            .saturating_add(page.lifecycle_events_pruned);
        run.episode_evidence_enqueued = run
            .episode_evidence_enqueued
            .saturating_add(page.episode_evidence_enqueued);
        run.resolved_episodes_pruned = run
            .resolved_episodes_pruned
            .saturating_add(page.resolved_episodes_pruned);
        if page.lifecycle_events_pruned < config.prune_limit as usize
            && page.resolved_episodes_pruned < config.prune_limit as usize
        {
            return Ok(run);
        }
        tokio::task::yield_now().await;
    }
}

async fn prune_lifecycle_events_page(
    pool: &PgPool,
    config: AlertPolicyRetentionConfig,
    event_seq_through: i64,
    round_started_at: DateTime<Utc>,
) -> Result<AlertPolicyRetentionRun> {
    let mut tx = pool.begin().await?;
    set_retention_transaction_bounds(&mut tx).await?;

    // One database-owned transaction timestamp keeps lifecycle-event and
    // episode eligibility identical. Binding the resulting value also makes
    // the resolved-at bound a real btree index condition; clock_timestamp()
    // is volatile and would turn that bound into a post-scan filter.
    let lifecycle_cutoff: DateTime<Utc> = sqlx::query_scalar(
        r#"
        SELECT transaction_timestamp()
               - ($1::bigint * interval '1 day')
        "#,
    )
    .bind(config.lifecycle_retention_days)
    .fetch_one(&mut *tx)
    .await?;

    // Apply every terminal-owner predicate before LIMIT. An unsafe old event
    // remains durable, but cannot repeatedly occupy a bounded transaction page
    // and starve a later independent event that is already safe to remove.
    let safe_event_seqs = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT lifecycle.event_seq
        FROM alert_lifecycle_events lifecycle
        JOIN alert_lifecycle_consumer_receipts webhook_receipt
          ON webhook_receipt.event_seq = lifecycle.event_seq
         AND webhook_receipt.consumer_kind='webhook'
         AND webhook_receipt.status='completed'
        WHERE lifecycle.created_at <= $1::timestamptz
          AND lifecycle.event_seq <= $2
          AND EXISTS (
                SELECT 1
                FROM alert_lifecycle_consumer_receipts schedule_receipt
                WHERE schedule_receipt.consumer_kind='schedule'
                  AND schedule_receipt.event_seq=lifecycle.event_seq
                  AND schedule_receipt.status='completed'
          )
          AND NOT EXISTS (
              SELECT 1
              FROM webhook_events webhook_event
              WHERE webhook_event.occurred_at = webhook_receipt.output_occurred_at
                AND webhook_event.id = webhook_receipt.output_id
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
              FROM alert_episodes episode
              JOIN fleet_alert_notification_deliveries delivery
                ON delivery.alert_id = episode.public_id
              WHERE episode.id = lifecycle.episode_id
                AND delivery.status IN ('queued', 'in_progress', 'failed')
          )
          AND NOT EXISTS (
              SELECT 1
              FROM schedule_event_receipts schedule_receipt
              LEFT JOIN jobs dispatched_job
                ON dispatched_job.id = schedule_receipt.job_id
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
        LIMIT $3
        FOR UPDATE OF lifecycle SKIP LOCKED
        "#,
    )
    .bind(lifecycle_cutoff)
    .bind(event_seq_through)
    .bind(config.prune_limit)
    .fetch_all(&mut *tx)
    .await?;

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
    let consumer_receipts_pruned = sqlx::query(
        r#"
        DELETE FROM alert_lifecycle_consumer_receipts
        WHERE event_seq = ANY($1::bigint[])
        "#,
    )
    .bind(&safe_event_seqs)
    .execute(&mut *tx)
    .await?
    .rows_affected() as usize;
    let deleted_episode_ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        DELETE FROM alert_lifecycle_events
        WHERE event_seq = ANY($1::bigint[])
        RETURNING episode_id
        "#,
    )
    .bind(&safe_event_seqs)
    .fetch_all(&mut *tx)
    .await?;
    let lifecycle_events_pruned = deleted_episode_ids.len();

    // Normal episodes become candidates as their last age-expired lifecycle
    // edge is consumed above. The bounded resolved-retention index page also
    // drains finite edge-less resolved rows. Every dependent consumer is
    // rechecked here; ambiguous or live ownership fails closed.
    // Fleet alert state is deliberately not deleted: it is operator-created,
    // is not telemetry-rate data, and has no lock/FK tying a concurrent triage
    // mutation to the episode row.
    let episode_prune = sqlx::query(
        r#"
        WITH orphan_prefix AS MATERIALIZED (
            SELECT episode.id
            FROM alert_episodes episode
            WHERE episode.lifecycle_state = 'resolved'
              AND episode.resolved_at <= $2::timestamptz
              AND episode.created_at <= $4::timestamptz
              AND NOT EXISTS (
                  SELECT 1
                  FROM alert_policy_evaluation_states state
                  WHERE state.active_episode_id = episode.id
              )
              AND NOT EXISTS (
                  SELECT 1 FROM alert_lifecycle_events lifecycle
                  WHERE lifecycle.episode_id = episode.id
              )
              AND NOT EXISTS (
                  SELECT 1 FROM schedule_event_receipts schedule_receipt
                  WHERE schedule_receipt.episode_id = episode.id
              )
              AND NOT EXISTS (
                  SELECT 1 FROM webhook_events webhook_event
                  WHERE webhook_event.kind IN (
                            'alert.triggered', 'alert.resolved'
                        )
                    AND webhook_event.event_id IN (
                        'fleet-alert:' || episode.id::text || ':triggered',
                        'fleet-alert:' || episode.id::text || ':resolved'
                    )
                    AND webhook_event.processed_at IS NULL
              )
              AND NOT EXISTS (
                  SELECT 1 FROM webhook_rule_deliveries delivery
                  WHERE delivery.event_kind IN (
                            'alert.triggered', 'alert.resolved'
                        )
                    AND delivery.event_id IN (
                        'fleet-alert:' || episode.id::text || ':triggered',
                        'fleet-alert:' || episode.id::text || ':resolved'
                    )
                    AND delivery.status IN ('queued', 'in_progress', 'failed')
              )
              AND NOT EXISTS (
                  SELECT 1
                  FROM fleet_alert_notification_deliveries delivery
                  WHERE delivery.alert_id = episode.public_id
                    AND delivery.status IN ('queued', 'in_progress', 'failed')
              )
            ORDER BY episode.resolved_at DESC, episode.id DESC
            LIMIT $3
        ), source_ids AS MATERIALIZED (
            SELECT unnest($1::uuid[]) AS id
            UNION
            SELECT id FROM orphan_prefix
        ), candidates AS MATERIALIZED (
            SELECT episode.id, episode.trigger_evidence_id,
                   episode.last_evidence_id
            FROM source_ids source
            JOIN alert_episodes episode ON episode.id = source.id
            WHERE episode.lifecycle_state = 'resolved'
              AND episode.resolved_at IS NOT NULL
              AND episode.resolved_at <= $2::timestamptz
              AND episode.triggered_at <= $2::timestamptz
              AND episode.created_at <= $4::timestamptz
              AND NOT EXISTS (
                  SELECT 1
                  FROM alert_policy_evaluation_states state
                  WHERE state.active_episode_id = episode.id
              )
              AND NOT EXISTS (
                  SELECT 1 FROM alert_lifecycle_events lifecycle
                  WHERE lifecycle.episode_id = episode.id
              )
              AND NOT EXISTS (
                  SELECT 1 FROM schedule_event_receipts schedule_receipt
                  WHERE schedule_receipt.episode_id = episode.id
              )
              AND NOT EXISTS (
                  SELECT 1 FROM webhook_events webhook_event
                  WHERE webhook_event.kind IN (
                            'alert.triggered', 'alert.resolved'
                        )
                    AND webhook_event.event_id IN (
                        'fleet-alert:' || episode.id::text || ':triggered',
                        'fleet-alert:' || episode.id::text || ':resolved'
                    )
                    AND webhook_event.processed_at IS NULL
              )
              AND NOT EXISTS (
                  SELECT 1 FROM webhook_rule_deliveries delivery
                  WHERE delivery.event_kind IN (
                            'alert.triggered', 'alert.resolved'
                        )
                    AND delivery.event_id IN (
                        'fleet-alert:' || episode.id::text || ':triggered',
                        'fleet-alert:' || episode.id::text || ':resolved'
                    )
                    AND delivery.status IN ('queued', 'in_progress', 'failed')
              )
              AND NOT EXISTS (
                  SELECT 1
                  FROM fleet_alert_notification_deliveries delivery
                  WHERE delivery.alert_id = episode.public_id
                    AND delivery.status IN ('queued', 'in_progress', 'failed')
              )
            ORDER BY episode.triggered_at DESC, episode.id DESC
            LIMIT $3
            FOR UPDATE OF episode SKIP LOCKED
        ), evidence_to_queue AS MATERIALIZED (
            SELECT trigger_evidence_id AS evidence_id FROM candidates
            WHERE trigger_evidence_id IS NOT NULL
            UNION
            SELECT last_evidence_id FROM candidates
            WHERE last_evidence_id IS NOT NULL
        ), enqueued AS (
            INSERT INTO alert_policy_evidence_prune_candidates (
                evidence_id, source_kind, subject_client_id, natural_key
            )
            SELECT evidence.id, evidence.source_kind,
                   evidence.subject_client_id, evidence.natural_key
            FROM evidence_to_queue held
            JOIN alert_policy_evidence evidence ON evidence.id = held.evidence_id
            ON CONFLICT (evidence_id) DO NOTHING
            RETURNING evidence_id
        ), deleted AS (
            DELETE FROM alert_episodes episode
            USING candidates candidate
            WHERE episode.id = candidate.id
              AND (SELECT count(*) FROM enqueued) >= 0
            RETURNING episode.id
        )
        SELECT
            (SELECT count(*)::bigint FROM enqueued) AS evidence_enqueued,
            (SELECT count(*)::bigint FROM deleted) AS episodes_pruned
        "#,
    )
    .bind(&deleted_episode_ids)
    .bind(lifecycle_cutoff)
    .bind(config.prune_limit)
    .bind(round_started_at)
    .fetch_one(&mut *tx)
    .await?;
    let episode_evidence_enqueued = episode_prune.try_get::<i64, _>("evidence_enqueued")? as usize;
    let resolved_episodes_pruned = episode_prune.try_get::<i64, _>("episodes_pruned")? as usize;
    tx.commit().await?;

    Ok(AlertPolicyRetentionRun {
        schedule_dependencies_pruned,
        schedule_receipts_pruned,
        consumer_receipts_pruned,
        lifecycle_events_pruned,
        episode_evidence_enqueued,
        resolved_episodes_pruned,
        ..AlertPolicyRetentionRun::default()
    })
}

async fn set_retention_transaction_bounds(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<()> {
    // Bound only waits on foreground-owned locks. Indexed page limits bound
    // each maintenance transaction; a valid page must finish or return its
    // error instead of being reported as successful skipped work.
    sqlx::query("SET LOCAL lock_timeout = '2s'")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[cfg(test)]
#[path = "tests_alert_policy_retention.rs"]
mod tests;
