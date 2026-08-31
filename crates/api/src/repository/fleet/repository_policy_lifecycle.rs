use std::collections::{BTreeSet, HashMap};
use std::fmt;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use sqlx::{postgres::PgRow, types::Json as SqlJson, Row};
use uuid::Uuid;
use vpsman_common::{
    expression_truth, parse_expression, ExpressionContext, ExpressionTruth, VpsMetadata,
    VpsRuleContext,
};

use crate::model_alert_policies::{
    AlertPolicyCorrelationMode, AlertPolicyMetaCondition, AlertPolicyRuleKind,
};

const MAX_LINEAGE: usize = 16;

/// Presentation-neutral fact accepted from a source transaction. Alert copy,
/// severity and category deliberately do not exist at this boundary; those are
/// immutable rule-definition snapshots once an episode triggers.
#[derive(Clone, Debug)]
pub(crate) struct PolicyEvidenceFact {
    pub(crate) source_kind: String,
    pub(crate) source_event_id: String,
    pub(crate) fact_kind: AlertPolicyRuleKind,
    pub(crate) natural_key: String,
    pub(crate) confirmation_bucket_key: String,
    pub(crate) subject_client_id: Option<String>,
    pub(crate) target_kind: String,
    pub(crate) target_id: String,
    pub(crate) source_status: String,
    pub(crate) complete: bool,
    pub(crate) subject_snapshot: Value,
    pub(crate) payload: Value,
    pub(crate) observed_at: DateTime<Utc>,
    pub(crate) state_started_at: Option<DateTime<Utc>>,
    pub(crate) causation_id: Option<Uuid>,
    pub(crate) schedule_lineage: Vec<Uuid>,
}

#[derive(Clone, Debug)]
struct StoredEvidence {
    id: Uuid,
    evidence_seq: i64,
    source_kind: String,
    source_event_id: String,
    fact_kind: AlertPolicyRuleKind,
    natural_key: String,
    confirmation_bucket_key: String,
    subject_client_id: Option<String>,
    target_kind: String,
    target_id: String,
    source_status: String,
    complete: bool,
    subject_snapshot: Value,
    payload: Value,
    observed_at: DateTime<Utc>,
    accepted_at: DateTime<Utc>,
    state_started_at: Option<DateTime<Utc>>,
    causation_id: Option<Uuid>,
    schedule_lineage: Vec<Uuid>,
}

#[derive(Clone, Debug)]
struct EvaluatorRule {
    id: Uuid,
    group_id: Uuid,
    group_name: String,
    group_selector: String,
    rule_version: i32,
    name: String,
    kind: AlertPolicyRuleKind,
    evidence_source: String,
    correlation_mode: AlertPolicyCorrelationMode,
    trigger_expression: String,
    trigger_meta: Option<AlertPolicyMetaCondition>,
    resolve_expression: Option<String>,
    resolve_meta: Option<AlertPolicyMetaCondition>,
    severity: String,
    category: String,
    title_template: String,
    detail_template: String,
    system_seed_key: Option<String>,
    armed_after_evidence_seq: i64,
    armed_at: DateTime<Utc>,
}

/// Enabled immutable rule generations for one evidence source. Telemetry
/// projection loads and generation-stamps them once for its complete claimed
/// client suffix, so an absent consumer bypasses all policy-only work.
pub(crate) struct PolicyEvidenceRuleSet {
    source_kind: String,
    rules: Vec<EvaluatorRule>,
}

impl PolicyEvidenceRuleSet {
    pub(crate) fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

#[derive(Clone, Debug)]
struct EvaluationState {
    truth_state: String,
    last_evidence_seq: Option<i64>,
    last_evidence_source_event_id: Option<String>,
    last_evidence_observed_at: Option<DateTime<Utc>>,
    trigger_confirmed_duration_secs: i64,
    trigger_segment_started_at: Option<DateTime<Utc>>,
    resolve_confirmed_duration_secs: i64,
    resolve_segment_started_at: Option<DateTime<Utc>>,
    trigger_generation: i64,
    active_episode_id: Option<Uuid>,
    active_triggered_at: Option<DateTime<Utc>>,
    occurrence_cohort_id: Option<Uuid>,
}

#[derive(Clone, Debug)]
struct ActiveEpisode {
    id: Uuid,
    last_evidence_id: Uuid,
    generation: i64,
    lifecycle_state: String,
    schedule_lineage: Vec<Uuid>,
    triggered_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GatePhase {
    Trigger,
    Resolve,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScopeTruth {
    InScope,
    OutOfScope,
    Unknown,
}

#[derive(Debug)]
struct DeterministicPolicyEvaluationError(String);

enum PolicyRuleEvaluationDisposition {
    Terminal,
    Retry(anyhow::Error),
}

impl fmt::Display for DeterministicPolicyEvaluationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for DeterministicPolicyEvaluationError {}

/// Converts only failures from pure rule/evidence evaluation boundaries. SQL,
/// transaction, and durable-write failures never pass through this helper and
/// therefore retain retry semantics.
fn deterministic_policy_evaluator_error(boundary: &str, error: anyhow::Error) -> anyhow::Error {
    if error
        .downcast_ref::<DeterministicPolicyEvaluationError>()
        .is_some()
    {
        return error;
    }
    DeterministicPolicyEvaluationError(format!("{boundary}:{error}")).into()
}

impl GatePhase {
    fn storage(self) -> &'static str {
        match self {
            Self::Trigger => "trigger",
            Self::Resolve => "resolve",
        }
    }
}

pub(crate) async fn record_policy_evidence_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    fact: PolicyEvidenceFact,
) -> Result<bool> {
    let fact = prepare_policy_evidence_fact_in_tx(tx, fact).await?;

    let rule_set = load_policy_evidence_rule_set_in_tx(tx, &fact.source_kind).await?;
    insert_and_evaluate_policy_evidence_in_tx(tx, fact, &rule_set).await
}

/// Loads one source's enabled generations under row-key-share ownership.
/// Different producers remain fully concurrent; a mutation waits only for the
/// exact affected generation so it can drain its stamped evidence before
/// replacing that generation.
pub(crate) async fn load_policy_evidence_rule_set_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    source_kind: &str,
) -> Result<PolicyEvidenceRuleSet> {
    Ok(PolicyEvidenceRuleSet {
        source_kind: source_kind.to_string(),
        rules: load_evaluator_rules_in_tx(tx, source_kind).await?,
    })
}

/// Records a fact against generations already claimed and loaded by its source
/// owner. This preserves one definition snapshot across a claimed telemetry
/// suffix without a rule-table query per sample.
pub(crate) async fn record_policy_evidence_with_rule_set_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    fact: PolicyEvidenceFact,
    rule_set: &PolicyEvidenceRuleSet,
) -> Result<bool> {
    anyhow::ensure!(
        fact.source_kind == rule_set.source_kind,
        "policy evidence rule-set source mismatch"
    );
    let fact = prepare_policy_evidence_fact_in_tx(tx, fact).await?;
    insert_and_evaluate_policy_evidence_in_tx(tx, fact, rule_set).await
}

/// Appends a current condition fact without evaluating it. The telemetry
/// activation consumer calls this while it owns one exact client/sample work
/// row; the short finalizer captures the rule arm boundary only after every
/// such row is acknowledged.
pub(crate) async fn materialize_policy_evidence_baseline_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    fact: PolicyEvidenceFact,
) -> Result<bool> {
    let fact = prepare_policy_evidence_fact_in_tx(tx, fact).await?;
    Ok(insert_policy_evidence_in_tx(tx, fact).await?.is_some())
}

async fn prepare_policy_evidence_fact_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    mut fact: PolicyEvidenceFact,
) -> Result<PolicyEvidenceFact> {
    validate_policy_evidence_fact(&fact)?;
    fact.schedule_lineage = canonical_lineage(fact.schedule_lineage)?;

    if let Some(client_id) = fact.subject_client_id.as_deref() {
        if let Some(snapshot) = load_policy_subject_snapshot_in_tx(tx, client_id).await? {
            fact.subject_snapshot = snapshot;
        }
    }
    Ok(fact)
}

async fn insert_policy_evidence_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    fact: PolicyEvidenceFact,
) -> Result<Option<StoredEvidence>> {
    let evidence_id = Uuid::new_v4();
    let row = sqlx::query(
        r#"
        INSERT INTO alert_policy_evidence (
            id, source_kind, source_event_id, fact_kind, natural_key,
            confirmation_bucket_key, subject_client_id, target_kind, target_id,
            source_status, completeness, subject_snapshot, payload, observed_at,
            state_started_at, causation_id, schedule_lineage, evaluation_pending
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
            $12, $13, $14, $15, $16, $17, FALSE
        )
        ON CONFLICT (source_kind, source_event_id) DO NOTHING
        RETURNING id, evidence_seq, created_at
        "#,
    )
    .bind(evidence_id)
    .bind(&fact.source_kind)
    .bind(&fact.source_event_id)
    .bind(rule_kind_storage(fact.fact_kind))
    .bind(&fact.natural_key)
    .bind(&fact.confirmation_bucket_key)
    .bind(&fact.subject_client_id)
    .bind(&fact.target_kind)
    .bind(&fact.target_id)
    .bind(&fact.source_status)
    .bind(if fact.complete { "complete" } else { "unknown" })
    .bind(SqlJson(fact.subject_snapshot.clone()))
    .bind(SqlJson(fact.payload.clone()))
    .bind(fact.observed_at)
    .bind(fact.state_started_at)
    .bind(fact.causation_id)
    .bind(&fact.schedule_lineage)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };

    Ok(Some(StoredEvidence {
        id: row.try_get("id")?,
        evidence_seq: row.try_get("evidence_seq")?,
        source_kind: fact.source_kind,
        source_event_id: fact.source_event_id,
        fact_kind: fact.fact_kind,
        natural_key: fact.natural_key,
        confirmation_bucket_key: fact.confirmation_bucket_key,
        subject_client_id: fact.subject_client_id,
        target_kind: fact.target_kind,
        target_id: fact.target_id,
        source_status: fact.source_status,
        complete: fact.complete,
        subject_snapshot: fact.subject_snapshot,
        payload: fact.payload,
        observed_at: fact.observed_at,
        accepted_at: row.try_get("created_at")?,
        state_started_at: fact.state_started_at,
        causation_id: fact.causation_id,
        schedule_lineage: fact.schedule_lineage,
    }))
}

async fn insert_and_evaluate_policy_evidence_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    fact: PolicyEvidenceFact,
    rule_set: &PolicyEvidenceRuleSet,
) -> Result<bool> {
    let Some(evidence) = insert_policy_evidence_in_tx(tx, fact).await? else {
        return Ok(false);
    };
    stamp_policy_evidence_targets_in_tx(tx, &evidence, &rule_set.rules).await?;
    let mut evaluation_pending = false;
    for rule in &rule_set.rules {
        if evidence.evidence_seq <= rule.armed_after_evidence_seq {
            // Occurrences are strictly prospective. State/metric definitions
            // receive an explicit latest-fact baseline when armed; therefore a
            // normal source hook may safely classify this historical row.
            record_receipt_in_tx(tx, rule, &evidence, "pre_armed", None).await?;
            continue;
        }
        // This transaction has just inserted the globally new evidence_seq.
        // A receipt has an FK to that uncommitted row, so no concurrent
        // transaction can have created one and no earlier row can share the
        // sequence. Replay/deferred paths retain their explicit receipt
        // checks; the fresh source path can evaluate exactly once directly.
        if let PolicyRuleEvaluationDisposition::Retry(error) =
            evaluate_rule_to_receipt_in_tx(tx, rule, &evidence, None).await?
        {
            // Do not terminal-consume a possibly transient evaluator/DB
            // failure. The fact commits with the source mutation, while its
            // explicit lifecycle bit durably owns the ordered retry.
            evaluation_pending = true;
            tracing::warn!(
                policy_rule_id = %rule.id,
                evidence_seq = evidence.evidence_seq,
                error = %error,
                "deferred alert policy evidence evaluation"
            );
        }
    }
    if evaluation_pending {
        sqlx::query("UPDATE alert_policy_evidence SET evaluation_pending=TRUE WHERE id=$1")
            .bind(evidence.id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(true)
}

async fn stamp_policy_evidence_targets_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    evidence: &StoredEvidence,
    rules: &[EvaluatorRule],
) -> Result<()> {
    if rules.is_empty() {
        return Ok(());
    }
    let rule_ids = rules.iter().map(|rule| rule.id).collect::<Vec<_>>();
    let rule_versions = rules
        .iter()
        .map(|rule| rule.rule_version)
        .collect::<Vec<_>>();
    sqlx::query(
        r#"
        INSERT INTO alert_policy_evidence_targets (
            evidence_id, evidence_seq, policy_rule_id, rule_version
        )
        SELECT $1, $2, target.policy_rule_id, target.rule_version
        FROM UNNEST($3::uuid[], $4::integer[])
             AS target(policy_rule_id, rule_version)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(evidence.id)
    .bind(evidence.evidence_seq)
    .bind(rule_ids)
    .bind(rule_versions)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Resolves every episode owned by the requested clients and clears their
/// pending policy gates while the caller holds all exact client lifecycle
/// rows. Suspension is a subject-level scope exit, so this intentionally spans
/// connectivity, resource, traffic, tunnel, backup, job, and future
/// client-scoped policies. Work that is independent of actual episode edges is
/// set-based: a fleet mutation scans/updates each lifecycle relation once.
pub(crate) async fn suppress_client_policy_alerts_for_clients_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_ids: &[String],
) -> Result<HashMap<String, usize>> {
    let client_ids = client_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut resolved_counts = client_ids
        .iter()
        .cloned()
        .map(|client_id| (client_id, 0_usize))
        .collect::<HashMap<_, _>>();
    if client_ids.is_empty() {
        return Ok(resolved_counts);
    }
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **tx)
        .await?;

    // Every alert lifecycle owner acquires evaluation state before its active
    // episode. This covers both client-owned states and any correlated state
    // whose active episode belongs to a client being suppressed. The join
    // replaces one correlated state scan per target with one fleet-bounded
    // lock query; actual lifecycle edges remain proportional to real episodes.
    let locked_states = sqlx::query(
        r#"
        SELECT state.policy_rule_id, state.rule_version,
               state.confirmation_bucket_key,
               rule.rule_kind='occurrence' AS occurrence,
               COALESCE(state.subject_client_id=ANY($1),FALSE) AS subject_owned
        FROM alert_policy_evaluation_states state
        JOIN policy_rules rule
          ON rule.id=state.policy_rule_id AND rule.rule_version=state.rule_version
        LEFT JOIN alert_episodes active_episode
          ON active_episode.id=state.active_episode_id
        WHERE state.subject_client_id=ANY($1)
           OR active_episode.client_id=ANY($1)
        ORDER BY state.policy_rule_id, state.rule_version,
                 state.confirmation_bucket_key COLLATE "C"
        FOR UPDATE OF state
        "#,
    )
    .bind(&client_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut subject_rule_ids = Vec::new();
    let mut subject_rule_versions = Vec::new();
    let mut subject_buckets = Vec::new();
    let mut subject_occurrence = Vec::new();
    for state in &locked_states {
        if !state.try_get::<bool, _>("subject_owned")? {
            continue;
        }
        subject_rule_ids.push(state.try_get::<Uuid, _>("policy_rule_id")?);
        subject_rule_versions.push(state.try_get::<i32, _>("rule_version")?);
        subject_buckets.push(state.try_get::<String, _>("confirmation_bucket_key")?);
        subject_occurrence.push(state.try_get::<bool, _>("occurrence")?);
    }

    let rows = sqlx::query(
        r#"
        SELECT id, client_id, policy_rule_id, policy_rule_version,
               COALESCE(last_evidence_id, trigger_evidence_id) AS evidence_id,
               trigger_generation, lifecycle_state, schedule_lineage,
               triggered_at
        FROM alert_episodes
        WHERE client_id=ANY($1)
          AND resolved_at IS NULL
        ORDER BY id
        FOR UPDATE
        "#,
    )
    .bind(&client_ids)
    .fetch_all(&mut **tx)
    .await?;

    for row in &rows {
        let client_id: String = row.try_get("client_id")?;
        *resolved_counts.entry(client_id).or_default() += 1;
    }
    let episode_ids = rows
        .iter()
        .map(|row| row.try_get::<Uuid, _>("id").map_err(Into::into))
        .collect::<Result<Vec<_>>>()?;
    let resolved_episode_ids = if episode_ids.is_empty() {
        BTreeSet::new()
    } else {
        sqlx::query_scalar::<_, Uuid>(
            r#"
            UPDATE alert_episodes
            SET lifecycle_state='resolved',
                resolved_at=GREATEST($2,last_confirmed_at),
                resolution_reason='source_scope_exited',
                resolution_note=NULL,
                resolution_actor_id=NULL,
                updated_at=$2
            WHERE id=ANY($1) AND resolved_at IS NULL
            RETURNING id
            "#,
        )
        .bind(&episode_ids)
        .bind(now)
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .collect::<BTreeSet<_>>()
    };

    for row in &rows {
        let episode_id: Uuid = row.try_get("id")?;
        if !resolved_episode_ids.contains(&episode_id) {
            continue;
        }

        let (Some(rule_id), Some(rule_version), Some(evidence_id)) = (
            row.try_get::<Option<Uuid>, _>("policy_rule_id")?,
            row.try_get::<Option<i32>, _>("policy_rule_version")?,
            row.try_get::<Option<Uuid>, _>("evidence_id")?,
        ) else {
            continue;
        };
        let rule = load_evaluator_rule_by_id_in_tx(tx, rule_id, rule_version).await?;
        let evidence = load_stored_evidence_in_tx(tx, evidence_id).await?;
        let episode = ActiveEpisode {
            id: episode_id,
            last_evidence_id: evidence_id,
            generation: row.try_get("trigger_generation")?,
            lifecycle_state: "resolved".to_string(),
            schedule_lineage: row.try_get("schedule_lineage")?,
            triggered_at: row.try_get("triggered_at")?,
        };
        emit_lifecycle_edge_in_tx(tx, &rule, &evidence, &episode, "alert.resolved", now).await?;
    }

    // Mark only generations whose triggered edge can still create or complete
    // webhook delivery after this transaction. The delivery owner checks this
    // provenance both during materialization and immediately before HTTP. A
    // completed receipt + processed event with no live delivery is terminal
    // history and must not be rewritten. All joins below use the lifecycle,
    // receipt, event-sequence, and event-identity indexes from the clean schema.
    // Unsuspend creates fresh episode rows, so the marker cannot suppress a
    // genuinely new incident.
    sqlx::query(
        r#"
        UPDATE alert_episodes episode
        SET evidence=episode.evidence || jsonb_build_object(
                '_vpsman_client_suspension',
                jsonb_build_object('client_id',episode.client_id,'suppressed_at',$2)
            ),
            updated_at=GREATEST(episode.updated_at,$2)
        WHERE episode.client_id=ANY($1)
          AND EXISTS (
                SELECT 1
                FROM alert_lifecycle_events lifecycle
                WHERE lifecycle.episode_id=episode.id
                  AND lifecycle.trigger_generation=episode.trigger_generation
                  AND lifecycle.edge_kind='alert.triggered'
                  AND (
                        NOT EXISTS (
                            SELECT 1
                            FROM alert_lifecycle_consumer_receipts receipt
                            WHERE receipt.consumer_kind='webhook'
                              AND receipt.event_seq=lifecycle.event_seq
                              AND receipt.status='completed'
                        )
                        OR EXISTS (
                            SELECT 1
                            FROM webhook_events event
                            WHERE event.alert_lifecycle_event_seq=lifecycle.event_seq
                              AND (
                                    event.processed_at IS NULL
                                    OR EXISTS (
                                        SELECT 1
                                        FROM webhook_rule_deliveries delivery
                                        WHERE delivery.event_kind=event.kind
                                          AND delivery.event_id=event.event_id
                                          AND delivery.status IN (
                                                'queued','in_progress','failed'
                                          )
                                    )
                              )
                        )
                  )
          )
        "#,
    )
    .bind(&client_ids)
    .bind(now)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"
        DELETE FROM alert_policy_confirmations confirmation
        USING UNNEST($1::uuid[], $2::integer[], $3::text[])
              AS target(policy_rule_id,rule_version,confirmation_bucket_key)
        WHERE confirmation.policy_rule_id=target.policy_rule_id
          AND confirmation.rule_version=target.rule_version
          AND confirmation.confirmation_bucket_key=target.confirmation_bucket_key
        "#,
    )
    .bind(&subject_rule_ids)
    .bind(&subject_rule_versions)
    .bind(&subject_buckets)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"
        UPDATE alert_policy_evaluation_states state
        SET active_episode_id=NULL,
            truth_state='not_matched', next_transition_at=NULL,
            trigger_confirmed_duration_secs=0,
            trigger_segment_started_at=NULL,
            resolve_confirmed_duration_secs=0,
            resolve_segment_started_at=NULL,
            occurrence_cohort_id=CASE
                WHEN target.occurrence THEN gen_random_uuid()
                ELSE state.occurrence_cohort_id
            END,
            last_evaluated_at=$5, updated_at=$5
        FROM UNNEST($1::uuid[], $2::integer[], $3::text[], $4::boolean[])
             AS target(
                policy_rule_id,rule_version,confirmation_bucket_key,occurrence
             )
        WHERE state.policy_rule_id=target.policy_rule_id
          AND state.rule_version=target.rule_version
          AND state.confirmation_bucket_key=target.confirmation_bucket_key
        "#,
    )
    .bind(&subject_rule_ids)
    .bind(&subject_rule_versions)
    .bind(&subject_buckets)
    .bind(&subject_occurrence)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(resolved_counts)
}

/// Fences pending evidence evaluation against a client suspension.
/// Normal evidence producers already own the client row; the deferred evaluator
/// does not, so this narrow advisory lock prevents a deferred fact from
/// committing an episode just after suspension's final suppression scan.
pub(crate) async fn lock_client_policy_suppressions_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_ids: &[String],
) -> Result<()> {
    let lock_keys = client_ids
        .iter()
        .map(|client_id| vpsman_server_core::client_policy_suppression_lock_key(client_id))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    sqlx::query(
        r#"
        SELECT pg_advisory_xact_lock(hashtextextended(lock_key, 0))
        FROM unnest($1::text[]) AS requested(lock_key)
        ORDER BY lock_key COLLATE "C"
        "#,
    )
    .bind(lock_keys)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn lock_client_policy_suppression_shared_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
) -> Result<()> {
    lock_client_policy_suppressions_shared_in_tx(tx, &[client_id.to_string()]).await?;
    Ok(())
}

pub(crate) async fn lock_client_policy_suppressions_shared_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_ids: &[String],
) -> Result<()> {
    let lock_keys = client_ids
        .iter()
        .map(|client_id| vpsman_server_core::client_policy_suppression_lock_key(client_id))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    sqlx::query(
        r#"
        SELECT pg_advisory_xact_lock_shared(hashtextextended(lock_key, 0))
        FROM unnest($1::text[]) AS requested(lock_key)
        ORDER BY lock_key COLLATE "C"
        "#,
    )
    .bind(lock_keys)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

// One MVCC statement gives a coherent client/tags/rules subject. A source
// transaction reads its own prior mutations; deferred evidence maintenance reads
// the last committed subject without taking a client-row lock after the
// operational advisory lock. Evidence stores a textual subject identity, so
// deletion fencing is unnecessary and would invert the ingest lock order.
pub(crate) async fn load_policy_subject_snapshot_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
) -> Result<Option<Value>> {
    let row = sqlx::query(
        r#"
        SELECT jsonb_strip_nulls(jsonb_build_object(
            'scope_complete', TRUE,
            'scope_revision', client.policy_scope_revision,
            'client_id', client.id,
            'display_name', client.display_name,
            'status', client.status,
            'registration_ip', host(client.registration_ip),
            'last_ip', host(client.last_ip),
            'last_seen_at', client.last_seen_at,
            'internal_build_number', client.internal_build_number,
            'stale_since', client.stale_since,
            'stale_reason', client.stale_reason,
            'tags', COALESCE(
                (SELECT jsonb_agg(tag.name ORDER BY tag.display_order, tag.name)
                 FROM client_tags assignment
                 JOIN tags tag ON tag.id=assignment.tag_id
                 WHERE assignment.client_id=client.id),
                '[]'::jsonb
            ),
            'vps_rules', COALESCE(
                (SELECT jsonb_object_agg(
                    rule_value.key,
                    jsonb_build_object(
                        'value_raw', rule_value.value_raw,
                        'value_json', rule_value.value_json
                    ) ORDER BY rule_value.key
                 )
                 FROM vps_rule_values rule_value
                 WHERE rule_value.client_id=client.id),
                '{}'::jsonb
            )
        )) AS snapshot
        FROM clients client
        WHERE client.id=$1
        "#,
    )
    .bind(client_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| {
        row.try_get::<SqlJson<Value>, _>("snapshot")
            .map(|value| value.0)
    })
    .transpose()
    .map_err(Into::into)
}

/// Re-evaluates one client's latest authoritative condition facts against its
/// exact queued selector snapshot without changing their source/state boundary.
/// The consumer owns the client row before the queue row, matching mutation
/// producers' client-then-trigger order and preventing a queue/client lock
/// inversion. No caller may use this as an inline mutation hook.
pub(crate) async fn record_claimed_policy_scope_revision_evidence_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
) -> Result<usize> {
    let work = sqlx::query(
        r#"
        SELECT target_revision, requires_revision_advance
        FROM alert_policy_scope_dirty_clients
        WHERE client_id=$1
        FOR UPDATE
        "#,
    )
    .bind(client_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(work) = work else {
        return Ok(0);
    };
    let requires_revision_advance: bool = work.try_get("requires_revision_advance")?;
    if requires_revision_advance {
        // Enabling or changing a last-seen selector creates a new scope
        // boundary even for an offline client whose last source fact already
        // carries the otherwise-current revision.
        sqlx::query(
            r#"
            UPDATE clients
            SET policy_scope_revision=policy_scope_revision + 1
            WHERE id=$1
            "#,
        )
        .bind(client_id)
        .execute(&mut **tx)
        .await?;
    }
    // Bind the coalesced request to the revision protected by the already-held
    // client lock. The revision-advance trigger may have raised it above the
    // value initially enqueued by the policy-definition producer.
    sqlx::query(
        r#"
        UPDATE alert_policy_scope_dirty_clients dirty
        SET target_revision=client.policy_scope_revision
        FROM clients client
        WHERE dirty.client_id=client.id
          AND client.id=$1
        "#,
    )
    .bind(client_id)
    .execute(&mut **tx)
    .await?;
    let rows = sqlx::query(
        r#"
        SELECT evidence.id, client.policy_scope_revision, client.status
        FROM alert_policy_effective_current_evidence current_fact
        JOIN alert_policy_evidence evidence
          ON evidence.id=current_fact.evidence_id
        JOIN clients client ON client.id=current_fact.subject_client_id
        WHERE current_fact.subject_client_id=$1
          AND COALESCE(
                NULLIF(evidence.subject_snapshot->>'scope_revision','')::bigint,
                0
              ) <> client.policy_scope_revision
        ORDER BY current_fact.subject_client_id,
                 current_fact.source_kind,
                 current_fact.natural_key
        "#,
    )
    .bind(client_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut recorded = 0_usize;
    for row in rows {
        let evidence = load_stored_evidence_in_tx(tx, row.try_get("id")?).await?;
        let scope_revision: i64 = row.try_get("policy_scope_revision")?;
        let client_status: String = row.try_get("status")?;
        let metric_available =
            evidence.fact_kind != AlertPolicyRuleKind::Metric || client_status == "online";
        let identity_hash = vpsman_common::payload_hash(
            format!("{}:{}", evidence.source_kind, evidence.natural_key).as_bytes(),
        );
        let inserted = record_policy_evidence_in_tx(
            tx,
            PolicyEvidenceFact {
                source_kind: evidence.source_kind,
                // A newer authoritative source fact can arrive with the same
                // stale subject revision after an earlier R clone. Include
                // its monotonic evidence version so R/E2 is evaluated rather
                // than conflicting with the already-materialized R/E1 clone.
                source_event_id: format!(
                    "scope:{scope_revision}:{}:{identity_hash}",
                    evidence.evidence_seq
                ),
                fact_kind: evidence.fact_kind,
                natural_key: evidence.natural_key,
                confirmation_bucket_key: evidence.confirmation_bucket_key,
                subject_client_id: evidence.subject_client_id,
                target_kind: evidence.target_kind,
                target_id: evidence.target_id,
                source_status: if metric_available {
                    evidence.source_status
                } else {
                    "incomplete".to_string()
                },
                complete: evidence.complete && metric_available,
                subject_snapshot: evidence.subject_snapshot,
                payload: evidence.payload,
                observed_at: evidence.observed_at,
                state_started_at: evidence.state_started_at,
                causation_id: evidence.causation_id,
                schedule_lineage: evidence.schedule_lineage,
            },
        )
        .await?;
        recorded += usize::from(inserted);
    }
    sqlx::query(
        r#"
        DELETE FROM alert_policy_scope_dirty_clients dirty
        USING clients client
        WHERE dirty.client_id=client.id
          AND client.id=$1
          AND dirty.target_revision=client.policy_scope_revision
        "#,
    )
    .bind(client_id)
    .execute(&mut **tx)
    .await?;
    Ok(recorded)
}

pub(crate) async fn materialize_pending_policy_scope_revisions(
    pool: &sqlx::PgPool,
    limit: i64,
) -> Result<usize> {
    let mut consumed = 0_usize;
    for _ in 0..limit.clamp(1, 500) {
        let mut tx = pool.begin().await?;
        // The client row is the exact claim and is acquired before the queue
        // row. SKIP LOCKED lets API replicas consume unrelated clients without
        // waiting or selecting the same work; each transaction owns one client
        // only, so a slow evaluation cannot form a multi-client lock convoy.
        let client_id = sqlx::query_scalar::<_, String>(
            r#"
            SELECT client.id
            FROM alert_policy_scope_dirty_clients dirty
            JOIN clients client ON client.id=dirty.client_id
            ORDER BY dirty.dirty_at, dirty.client_id
            FOR NO KEY UPDATE OF client SKIP LOCKED
            LIMIT 1
            "#,
        )
        .fetch_optional(&mut *tx)
        .await?;
        let Some(client_id) = client_id else {
            tx.commit().await?;
            break;
        };
        record_claimed_policy_scope_revision_evidence_in_tx(&mut tx, &client_id).await?;
        tx.commit().await?;
        consumed += 1;
    }
    // A client with no current source identity still consumed real durable
    // work. The outer evaluator immediately continues after a full page.
    Ok(consumed)
}

/// Closes state-source identities that disappeared from an authoritative
/// source snapshot. The caller owns and locks the source identity (client,
/// plan, or equivalent) before appending generation-stamped evidence.
///
/// A source exit is an immutable fact rather than an inferred false sample:
/// it resets any pending confirmation gate and resolves an active confirmed
/// episode with the exact `source_scope_exited` cause. A stable identity based
/// on the last accepted evidence sequence makes retries idempotent.
pub(crate) async fn record_policy_source_scope_exits_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    source_kinds: &[&str],
    subject_client_ids: &[String],
    present_identities: &BTreeSet<(String, String)>,
) -> Result<usize> {
    if source_kinds.is_empty() || subject_client_ids.is_empty() {
        return Ok(0);
    }
    let candidates = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT current_fact.evidence_id
        FROM alert_policy_current_evidence current_fact
        WHERE current_fact.fact_kind='state'
          AND current_fact.source_kind=ANY($1::text[])
          AND current_fact.subject_client_id=ANY($2::text[])
        ORDER BY current_fact.subject_client_id,
                 current_fact.source_kind,
                 current_fact.natural_key
        "#,
    )
    .bind(source_kinds)
    .bind(subject_client_ids)
    .fetch_all(&mut **tx)
    .await?;
    let exited_at: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **tx)
        .await?;
    let mut recorded = 0_usize;
    for evidence_id in candidates {
        let evidence = load_stored_evidence_in_tx(tx, evidence_id).await?;
        if present_identities
            .contains(&(evidence.source_kind.clone(), evidence.natural_key.clone()))
            || evidence
                .payload
                .get("source_present")
                .and_then(Value::as_bool)
                == Some(false)
        {
            continue;
        }
        let identity_hash = vpsman_common::payload_hash(
            format!("{}:{}", evidence.source_kind, evidence.natural_key).as_bytes(),
        );
        let mut payload = evidence.payload.clone();
        let payload = payload
            .as_object_mut()
            .context("policy source-scope evidence payload is invalid")?;
        payload.insert("status".to_string(), json!("source_scope_exited"));
        payload.insert("source_status".to_string(), json!("source_scope_exited"));
        payload.insert("source_present".to_string(), json!(false));
        payload.insert(
            "resolution_reason".to_string(),
            json!("source_scope_exited"),
        );
        let inserted = record_policy_evidence_in_tx(
            tx,
            PolicyEvidenceFact {
                source_kind: evidence.source_kind,
                source_event_id: format!("source-exit:{}:{identity_hash}", evidence.evidence_seq),
                fact_kind: AlertPolicyRuleKind::State,
                natural_key: evidence.natural_key.clone(),
                confirmation_bucket_key: evidence.natural_key,
                subject_client_id: evidence.subject_client_id,
                target_kind: evidence.target_kind,
                target_id: evidence.target_id,
                source_status: "source_scope_exited".to_string(),
                complete: false,
                subject_snapshot: evidence.subject_snapshot,
                payload: Value::Object(payload.clone()),
                observed_at: exited_at,
                state_started_at: Some(exited_at),
                causation_id: evidence.causation_id,
                schedule_lineage: evidence.schedule_lineage,
            },
        )
        .await?;
        recorded += usize::from(inserted);
    }
    Ok(recorded)
}

#[cfg(test)]
pub(crate) async fn evaluate_pending_policy_evidence(
    pool: &sqlx::PgPool,
    limit: i64,
) -> Result<usize> {
    Ok(evaluate_pending_policy_evidence_page(pool, limit)
        .await?
        .changed)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PolicyMaintenancePage {
    pub(crate) examined: usize,
    pub(crate) changed: usize,
    /// At least one durable candidate was observed but is currently owned by
    /// another replica. The scheduler must follow this page immediately even
    /// when it was shorter than the ordinary candidate-page boundary.
    pub(crate) still_due: bool,
}

/// Evaluates only facts whose source transaction durably declared unfinished
/// policy work. Only a short page with no contended durable owner is an idle
/// signal; full or still-due pages are followed immediately by the caller and
/// are never throughput limits.
pub(crate) async fn evaluate_pending_policy_evidence_page(
    pool: &sqlx::PgPool,
    limit: i64,
) -> Result<PolicyMaintenancePage> {
    let candidates = sqlx::query(
        r#"
        SELECT evidence.id AS evidence_id, evidence.evidence_seq,
               evidence.subject_client_id
        FROM alert_policy_evidence evidence
        WHERE evidence.evaluation_pending
        ORDER BY evidence.evidence_seq
        LIMIT $1
        "#,
    )
    .bind(limit.clamp(1, 1000))
    .fetch_all(pool)
    .await?;
    let examined = candidates.len();
    let mut completed = 0_usize;
    let mut still_due = false;
    for candidate in candidates {
        let evidence_id: Uuid = candidate.try_get("evidence_id")?;
        let subject_client_id: Option<String> = candidate.try_get("subject_client_id")?;
        let mut tx = pool.begin().await?;
        if let Some(client_id) = subject_client_id.as_deref() {
            lock_client_policy_suppression_shared_in_tx(&mut tx, client_id).await?;
        }
        let target_generations = sqlx::query_as::<_, (Uuid, i32)>(
            r#"
            SELECT policy_rule_id, rule_version
            FROM alert_policy_evidence_targets
            WHERE evidence_id=$1
            ORDER BY policy_rule_id, rule_version
            "#,
        )
        .bind(evidence_id)
        .fetch_all(&mut *tx)
        .await?;
        let mut rules = Vec::with_capacity(target_generations.len());
        for (rule_id, rule_version) in target_generations {
            lock_evaluator_rule_generation_in_tx(&mut tx, rule_id, rule_version).await?;
            rules.push(load_evaluator_rule_by_id_in_tx(&mut tx, rule_id, rule_version).await?);
        }
        // Client suppression and every immutable rule generation have already
        // been acquired in the global lock order. Claim only this final,
        // durable work row without waiting: another replica that owns the same
        // evidence must not convoy the remaining ordered candidates in this
        // page. A skipped row remains evaluation_pending and is therefore
        // visible to the next immediate page while its current owner works.
        let still_pending = sqlx::query_scalar::<_, bool>(
            "SELECT evaluation_pending FROM alert_policy_evidence WHERE id=$1 FOR UPDATE SKIP LOCKED",
        )
        .bind(evidence_id)
        .fetch_optional(&mut *tx)
        .await?;
        if still_pending.is_none() {
            // The candidate was pending in the page snapshot but its exact
            // row could not be claimed. Conservatively keep the scheduler hot;
            // a concurrent delete merely causes one harmless extra page.
            still_due = true;
        }
        if still_pending != Some(true) {
            tx.commit().await?;
            continue;
        }
        let evidence = load_stored_evidence_in_tx(&mut tx, evidence_id).await?;
        let subject_suspended = if let Some(client_id) = subject_client_id.as_deref() {
            sqlx::query_scalar::<_, bool>("SELECT status='suspended' FROM clients WHERE id=$1")
                .bind(client_id)
                .fetch_optional(&mut *tx)
                .await?
                .unwrap_or(false)
        } else {
            false
        };
        let mut retry_error = None;
        for rule in rules {
            if evidence.evidence_seq <= rule.armed_after_evidence_seq
                || receipt_exists_in_tx(&mut tx, &rule, evidence.evidence_seq).await?
            {
                continue;
            }
            if subject_suspended {
                record_receipt_in_tx(
                    &mut tx,
                    &rule,
                    &evidence,
                    "out_of_scope",
                    Some("client_suspended"),
                )
                .await?;
                continue;
            }
            if let PolicyRuleEvaluationDisposition::Retry(error) = evaluate_rule_to_receipt_in_tx(
                &mut tx,
                &rule,
                &evidence,
                Some("pending_evaluation"),
            )
            .await?
            {
                retry_error = Some(error);
                break;
            }
        }
        if let Some(error) = retry_error {
            // The locked row was already TRUE. Commit any terminal receipts
            // preceding this transient rule without rewriting the partial
            // index entry, then end this ordered drain wave for idle retry.
            tx.commit().await?;
            return Err(error.context("pending alert policy evidence evaluation failed"));
        }
        let remains_pending = recompute_policy_evidence_pending_in_tx(&mut tx, evidence_id).await?;
        tx.commit().await?;
        completed += usize::from(!remains_pending);
    }
    Ok(PolicyMaintenancePage {
        examined,
        changed: completed,
        still_due,
    })
}

/// Recomputes the durable work bit from the evidence's immutable generation
/// targets and exact-version terminal receipts.
pub(crate) async fn recompute_policy_evidence_pending_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    evidence_id: Uuid,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        r#"
        WITH desired AS MATERIALIZED (
            SELECT evidence.id, EXISTS (
                SELECT 1
                FROM alert_policy_evidence_targets target
                WHERE target.evidence_id=evidence.id
                  AND NOT EXISTS (
                        SELECT 1
                        FROM alert_policy_evidence_receipts receipt
                        WHERE receipt.policy_rule_id=target.policy_rule_id
                          AND receipt.rule_version=target.rule_version
                          AND receipt.evidence_seq=evidence.evidence_seq
                  )
            ) AS evaluation_pending
            FROM alert_policy_evidence evidence
            WHERE evidence.id=$1
        ), updated AS (
            UPDATE alert_policy_evidence evidence
            SET evaluation_pending=desired.evaluation_pending
            FROM desired
            WHERE evidence.id=desired.id
              AND evidence.evaluation_pending IS DISTINCT FROM desired.evaluation_pending
            RETURNING evidence.evaluation_pending
        )
        SELECT COALESCE(
            (SELECT evaluation_pending FROM updated),
            (SELECT evaluation_pending FROM desired),
            FALSE
        )
        "#,
    )
    .bind(evidence_id)
    .fetch_optional(&mut **tx)
    .await?
    .unwrap_or(false))
}

/// Recomputes the same durable work bit for a definition-change set in one
/// statement. The caller does not observe an individual boolean; it owns the
/// whole post-mutation fence and commits only after every supplied identity is
/// consistent with its remaining exact-version targets.
pub(crate) async fn recompute_policy_evidences_pending_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    evidence_ids: &[Uuid],
) -> Result<()> {
    if evidence_ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        r#"
        WITH desired AS MATERIALIZED (
            SELECT evidence.id, EXISTS (
                SELECT 1
                FROM alert_policy_evidence_targets target
                WHERE target.evidence_id=evidence.id
                  AND NOT EXISTS (
                        SELECT 1
                        FROM alert_policy_evidence_receipts receipt
                        WHERE receipt.policy_rule_id=target.policy_rule_id
                          AND receipt.rule_version=target.rule_version
                          AND receipt.evidence_seq=evidence.evidence_seq
                  )
            ) AS evaluation_pending
            FROM alert_policy_evidence evidence
            WHERE evidence.id=ANY($1::uuid[])
        ), locked AS MATERIALIZED (
            SELECT evidence.id, desired.evaluation_pending
            FROM alert_policy_evidence evidence
            JOIN desired ON desired.id=evidence.id
            WHERE evidence.evaluation_pending IS DISTINCT FROM desired.evaluation_pending
            ORDER BY evidence.id
            FOR UPDATE OF evidence
        )
        UPDATE alert_policy_evidence evidence
        SET evaluation_pending=locked.evaluation_pending
        FROM locked
        WHERE evidence.id=locked.id
        "#,
    )
    .bind(evidence_ids)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Linearizes a definition change after every committed target for the affected
/// generation has reached an exact-version receipt. Callers own only those
/// generation rows, never a repository-global arm.
///
/// Worker-owned sources durably mark evidence pending for evaluation.
/// Advancing a rule's arm boundary before draining that owned work would
/// strand a pre-edit occurrence forever:
/// the old version would no longer exist and the new version would classify it
/// as pre-armed. A transient evaluator/SQL failure therefore aborts the edit;
/// deterministic definition/data failures are terminalized exactly as in the
/// normal pending-work path. The returned evidence identities must be
/// recomputed against the post-mutation definition set before commit.
pub(crate) async fn drain_policy_rule_pending_evidence_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule_ids: &[Uuid],
) -> Result<Vec<Uuid>> {
    if rule_ids.is_empty() {
        return Ok(Vec::new());
    }
    let candidates = sqlx::query(
        r#"
        SELECT rule.id AS rule_id, rule.rule_version,
               evidence.id AS evidence_id, evidence.evidence_seq,
               COALESCE(subject.status='suspended',FALSE) AS subject_suspended
        FROM alert_policy_evidence_targets target
        JOIN policy_rules rule
          ON rule.id=target.policy_rule_id
         AND rule.rule_version=target.rule_version
        JOIN policy_groups group_row ON group_row.id=rule.group_id
        JOIN alert_policy_evidence evidence
          ON evidence.id=target.evidence_id
         AND evidence.evidence_seq=target.evidence_seq
         AND evidence.evaluation_pending
        LEFT JOIN clients subject ON subject.id=evidence.subject_client_id
        LEFT JOIN alert_policy_evidence_receipts receipt
          ON receipt.policy_rule_id=rule.id
         AND receipt.rule_version=rule.rule_version
         AND receipt.evidence_seq=evidence.evidence_seq
        WHERE target.policy_rule_id=ANY($1::uuid[])
          AND rule.enabled AND group_row.enabled
          AND receipt.evidence_seq IS NULL
        ORDER BY evidence.evidence_seq, rule.id
        "#,
    )
    .bind(rule_ids)
    .fetch_all(&mut **tx)
    .await?;
    let rule_generations = candidates
        .iter()
        .map(|candidate| {
            Ok((
                candidate.try_get::<Uuid, _>("rule_id")?,
                candidate.try_get::<i32, _>("rule_version")?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let evidence_ids = candidates
        .iter()
        .map(|candidate| candidate.try_get::<Uuid, _>("evidence_id"))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let rules = load_evaluator_rules_by_generation_in_tx(tx, &rule_generations).await?;
    let evidences = load_stored_evidences_in_tx(tx, &evidence_ids).await?;
    let mut drained_evidence_ids = BTreeSet::new();
    for candidate in candidates {
        let rule_id: Uuid = candidate.try_get("rule_id")?;
        let rule_version: i32 = candidate.try_get("rule_version")?;
        let evidence_id: Uuid = candidate.try_get("evidence_id")?;
        let evidence_seq: i64 = candidate.try_get("evidence_seq")?;
        let subject_suspended: bool = candidate.try_get("subject_suspended")?;
        let rule = rules
            .get(&(rule_id, rule_version))
            .context("alert policy generation missing while draining evidence")?;
        let evidence = evidences
            .get(&evidence_id)
            .context("alert policy evidence missing while draining definition fence")?;
        if receipt_exists_in_tx(tx, rule, evidence_seq).await? {
            continue;
        }
        if subject_suspended {
            record_receipt_in_tx(tx, rule, evidence, "out_of_scope", Some("client_suspended"))
                .await?;
            drained_evidence_ids.insert(evidence_id);
            continue;
        }
        if let PolicyRuleEvaluationDisposition::Retry(error) =
            evaluate_rule_to_receipt_in_tx(tx, rule, evidence, Some("definition_fence_drain"))
                .await?
        {
            return Err(error);
        }
        drained_evidence_ids.insert(evidence_id);
    }
    for evidence_id in sqlx::query_scalar::<_, Uuid>(
        r#"
        DELETE FROM alert_policy_evidence_targets
        WHERE policy_rule_id=ANY($1::uuid[])
        RETURNING evidence_id
        "#,
    )
    .bind(rule_ids)
    .fetch_all(&mut **tx)
    .await?
    {
        drained_evidence_ids.insert(evidence_id);
    }
    Ok(drained_evidence_ids.into_iter().collect())
}

/// Advances only persisted, due confirmation/expiry timers. Evaluator ticks do
/// not create evidence or Count confirmations; they re-use the last accepted
/// authoritative fact under the state-row lock.
#[cfg(test)]
pub(crate) async fn evaluate_due_policy_transitions(
    pool: &sqlx::PgPool,
    limit: i64,
) -> Result<usize> {
    Ok(evaluate_due_policy_transitions_page(pool, limit)
        .await?
        .changed)
}

pub(crate) async fn evaluate_due_policy_transitions_page(
    pool: &sqlx::PgPool,
    limit: i64,
) -> Result<PolicyMaintenancePage> {
    let mut examined = 0_usize;
    let mut transitioned = 0_usize;
    for _ in 0..limit.clamp(1, 200) {
        let mut tx = pool.begin().await?;
        let due = sqlx::query(
            r#"
            SELECT state.policy_rule_id, state.rule_version,
                   state.confirmation_bucket_key, state.last_evidence_id,
                   state.active_episode_id, state.subject_client_id
            FROM alert_policy_evaluation_states state
            JOIN policy_rules rule
              ON rule.id=state.policy_rule_id AND rule.rule_version=state.rule_version
            JOIN policy_groups group_row ON group_row.id=rule.group_id
            WHERE state.next_transition_at IS NOT NULL
              AND state.next_transition_at <= clock_timestamp()
              AND rule.enabled AND group_row.enabled
              AND NOT EXISTS (
                    SELECT 1 FROM clients suspended_subject
                    WHERE suspended_subject.id=state.subject_client_id
                      AND suspended_subject.status='suspended'
              )
              AND (
                    rule.rule_kind='occurrence'
                    OR NOT EXISTS (
                        SELECT 1
                        FROM alert_policy_evidence newer
                        WHERE newer.evaluation_pending
                          AND newer.source_kind=rule.evidence_source
                          AND state.confirmation_bucket_key=
                              ('natural:' || newer.natural_key)
                          AND newer.evidence_seq > COALESCE(
                                state.last_evidence_seq,
                                rule.armed_after_evidence_seq
                              )
                          AND EXISTS (
                                SELECT 1
                                FROM alert_policy_evidence_targets target
                                WHERE target.evidence_id=newer.id
                                  AND target.evidence_seq=newer.evidence_seq
                                  AND target.policy_rule_id=rule.id
                                  AND target.rule_version=rule.rule_version
                          )
                          AND NOT EXISTS (
                                SELECT 1
                                FROM alert_policy_evidence_receipts receipt
                                WHERE receipt.policy_rule_id=rule.id
                                  AND receipt.rule_version=rule.rule_version
                                  AND receipt.evidence_seq=newer.evidence_seq
                          )
                    )
              )
              AND (
                    rule.rule_kind='occurrence'
                    OR EXISTS (
                        SELECT 1
                        FROM alert_policy_evidence last_evidence
                        JOIN clients subject
                          ON subject.id=state.subject_client_id
                        WHERE last_evidence.id=state.last_evidence_id
                          AND COALESCE(
                                NULLIF(
                                    last_evidence.subject_snapshot->>'scope_revision',
                                    ''
                                )::bigint,
                                0
                              ) = subject.policy_scope_revision
                          AND NOT EXISTS (
                                SELECT 1
                                FROM alert_policy_scope_dirty_clients scope_work
                                WHERE scope_work.client_id=subject.id
                                  AND scope_work.requires_revision_advance
                          )
                    )
              )
            ORDER BY state.next_transition_at, state.policy_rule_id,
                     state.confirmation_bucket_key
            FOR UPDATE OF state SKIP LOCKED
            LIMIT 1
            "#,
        )
        .fetch_optional(&mut *tx)
        .await?;
        let Some(due) = due else {
            tx.commit().await?;
            break;
        };
        examined += 1;
        let rule_id: Uuid = due.try_get("policy_rule_id")?;
        let rule_version: i32 = due.try_get("rule_version")?;
        let bucket: String = due.try_get("confirmation_bucket_key")?;
        let evidence_id: Option<Uuid> = due.try_get("last_evidence_id")?;
        let active_episode_id: Option<Uuid> = due.try_get("active_episode_id")?;
        let subject_client_id: Option<String> = due.try_get("subject_client_id")?;
        let Some(evidence_id) = evidence_id else {
            sqlx::query(
                r#"
                UPDATE alert_policy_evaluation_states
                SET next_transition_at=NULL, truth_state='unknown',
                    last_evaluated_at=clock_timestamp(), updated_at=clock_timestamp()
                WHERE policy_rule_id=$1 AND rule_version=$2
                  AND confirmation_bucket_key=$3
                "#,
            )
            .bind(rule_id)
            .bind(rule_version)
            .bind(&bucket)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            continue;
        };
        let rule = load_evaluator_rule_by_id_in_tx(&mut tx, rule_id, rule_version).await?;
        let mut evidence = load_stored_evidence_in_tx(&mut tx, evidence_id).await?;
        if rule.kind != AlertPolicyRuleKind::Occurrence {
            let current_scope_revision = if let Some(client_id) = subject_client_id.as_deref() {
                sqlx::query_scalar::<_, i64>(
                    r#"
                    SELECT policy_scope_revision
                    FROM clients
                    WHERE id=$1
                      AND NOT EXISTS (
                            SELECT 1
                            FROM alert_policy_scope_dirty_clients scope_work
                            WHERE scope_work.client_id=clients.id
                              AND scope_work.requires_revision_advance
                      )
                    "#,
                )
                .bind(client_id)
                .fetch_optional(&mut *tx)
                .await?
            } else {
                None
            };
            let evidence_scope_revision = evidence
                .subject_snapshot
                .get("scope_revision")
                .and_then(Value::as_i64);
            if current_scope_revision != evidence_scope_revision {
                // A concurrent scope mutation queues its exact revision fact.
                // This due transition therefore linearizes before that newer
                // fact, which owns the subsequent correction.
                tx.commit().await?;
                continue;
            }
        }
        // The state row is keyed by the rule-owned effective correlation
        // bucket. Source evidence stores only its neutral/natural bucket, so
        // every direct due-path helper must use the locked state's bucket.
        evidence.confirmation_bucket_key.clone_from(&bucket);
        if rule.kind == AlertPolicyRuleKind::Occurrence {
            let Some(AlertPolicyMetaCondition::ElapsedSinceTrigger { seconds }) =
                rule.resolve_meta.as_ref()
            else {
                anyhow::bail!("occurrence rule missing elapsed resolution");
            };
            let Some(active) = load_active_episode_in_tx(&mut tx, active_episode_id).await? else {
                sqlx::query(
                    r#"
                    UPDATE alert_policy_evaluation_states SET next_transition_at=NULL,
                        last_evaluated_at=clock_timestamp(), updated_at=clock_timestamp()
                    WHERE policy_rule_id=$1 AND rule_version=$2
                      AND confirmation_bucket_key=$3
                    "#,
                )
                .bind(rule_id)
                .bind(rule_version)
                .bind(&bucket)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                continue;
            };
            // State.last_evidence_id may point at a later nonmatching
            // occurrence that merely shared this correlation bucket. Elapsed
            // resolution belongs to the open episode and must use only its
            // contributing evidence/provenance.
            let mut episode_evidence =
                load_stored_evidence_in_tx(&mut tx, active.last_evidence_id).await?;
            episode_evidence.confirmation_bucket_key.clone_from(&bucket);
            let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
                .fetch_one(&mut *tx)
                .await?;
            if active.triggered_at + Duration::seconds(*seconds) > now {
                // A stale due scan cannot move the immutable trigger deadline.
                sqlx::query(
                    r#"
                    UPDATE alert_policy_evaluation_states
                    SET next_transition_at=$4, updated_at=clock_timestamp()
                    WHERE policy_rule_id=$1 AND rule_version=$2
                      AND confirmation_bucket_key=$3
                    "#,
                )
                .bind(rule_id)
                .bind(rule_version)
                .bind(&bucket)
                .bind(active.triggered_at + Duration::seconds(*seconds))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                continue;
            }
            resolve_episode_in_tx(
                &mut tx,
                &rule,
                &episode_evidence,
                &active,
                now,
                Some("policy_time_elapsed"),
            )
            .await?;
            // Resolving provenance comes from the episode, while evaluation
            // state keeps its actual latest accepted fact (which may be a
            // later nonmatching occurrence). Do not split last_evidence_id
            // from the state's last_evidence_seq/source tuple.
            let mut state = lock_or_create_state_in_tx(&mut tx, &rule, &evidence).await?;
            state.active_episode_id = None;
            state.active_triggered_at = None;
            state.truth_state = "not_matched".to_string();
            state.occurrence_cohort_id = Some(Uuid::new_v4());
            state.trigger_confirmed_duration_secs = 0;
            state.trigger_segment_started_at = None;
            state.resolve_confirmed_duration_secs = 0;
            state.resolve_segment_started_at = None;
            clear_confirmations_in_tx(&mut tx, &rule, &evidence, GatePhase::Trigger).await?;
            clear_confirmations_in_tx(&mut tx, &rule, &evidence, GatePhase::Resolve).await?;
            persist_state_in_tx(&mut tx, &rule, &evidence, &state, now).await?;
            transitioned += 1;
        } else if rule.kind == AlertPolicyRuleKind::Metric {
            // Metric Sustained gates are interval-evaluated. A timer may mark
            // their dwell boundary as reached, but it must not turn one old
            // sample into a new authoritative confirmation. The next accepted
            // metric revision re-evaluates the stored segment and may cross it.
            sqlx::query(
                r#"
                UPDATE alert_policy_evaluation_states
                SET next_transition_at=NULL,
                    last_evaluated_at=clock_timestamp(), updated_at=clock_timestamp()
                WHERE policy_rule_id=$1 AND rule_version=$2
                  AND confirmation_bucket_key=$3
                "#,
            )
            .bind(rule_id)
            .bind(rule_version)
            .bind(&bucket)
            .execute(&mut *tx)
            .await?;
        } else {
            let before: Option<Uuid> = active_episode_id;
            sqlx::query("SAVEPOINT alert_policy_due_evaluation")
                .execute(&mut *tx)
                .await?;
            if let Err(error) = evaluate_rule_in_tx(&mut tx, &rule, &evidence, true).await {
                sqlx::query("ROLLBACK TO SAVEPOINT alert_policy_due_evaluation")
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("RELEASE SAVEPOINT alert_policy_due_evaluation")
                    .execute(&mut *tx)
                    .await?;
                let detail = if is_lineage_overflow_error(&error) {
                    Some("policy_schedule_lineage_overflow")
                } else {
                    error
                        .downcast_ref::<DeterministicPolicyEvaluationError>()
                        .map(|error| error.0.as_str())
                };
                let Some(detail) = detail else {
                    return Err(error);
                };
                sqlx::query(
                    r#"
                    UPDATE alert_policy_evaluation_states
                    SET truth_state='unknown', next_transition_at=NULL,
                        trigger_confirmed_duration_secs=0,
                        trigger_segment_started_at=NULL,
                        resolve_confirmed_duration_secs=0,
                        resolve_segment_started_at=NULL,
                        last_evaluated_at=clock_timestamp(), updated_at=clock_timestamp()
                    WHERE policy_rule_id=$1 AND rule_version=$2
                      AND confirmation_bucket_key=$3
                    "#,
                )
                .bind(rule_id)
                .bind(rule_version)
                .bind(&bucket)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    r#"
                    UPDATE alert_policy_evidence_receipts
                    SET result=$4, detail=$5, evaluated_at=clock_timestamp()
                    WHERE policy_rule_id=$1 AND rule_version=$2 AND evidence_seq=$3
                    "#,
                )
                .bind(rule_id)
                .bind(rule_version)
                .bind(evidence.evidence_seq)
                .bind(if is_lineage_overflow_error(&error) {
                    "lineage_overflow"
                } else {
                    "error"
                })
                .bind(detail)
                .execute(&mut *tx)
                .await?;
                audit_policy_evaluation_skip_in_tx(&mut tx, &rule, &evidence, detail).await?;
                tx.commit().await?;
                continue;
            }
            sqlx::query("RELEASE SAVEPOINT alert_policy_due_evaluation")
                .execute(&mut *tx)
                .await?;
            let after: Option<Uuid> = sqlx::query_scalar(
                r#"
                SELECT active_episode_id FROM alert_policy_evaluation_states
                WHERE policy_rule_id=$1 AND rule_version=$2
                  AND confirmation_bucket_key=$3
                "#,
            )
            .bind(rule_id)
            .bind(rule_version)
            .bind(&bucket)
            .fetch_one(&mut *tx)
            .await?;
            if before != after {
                transitioned += 1;
            }
        }
        tx.commit().await?;
    }
    Ok(PolicyMaintenancePage {
        examined,
        changed: transitioned,
        still_due: false,
    })
}

/// Evaluates the latest authoritative condition snapshot for a newly armed
/// state/metric rule. Occurrence facts are intentionally never replayed.
pub(crate) async fn evaluate_policy_rule_baselines_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule_ids: &[Uuid],
) -> Result<usize> {
    let candidates = sqlx::query(
        r#"
        SELECT rule.id AS rule_id, rule.rule_version,
               current_fact.evidence_id
        FROM policy_rules rule
        JOIN policy_groups group_row ON group_row.id=rule.group_id
        JOIN alert_policy_effective_current_evidence current_fact
          ON current_fact.source_kind=rule.evidence_source
         AND current_fact.evidence_seq <= rule.armed_after_evidence_seq
        WHERE rule.id=ANY($1::uuid[]) AND rule.enabled AND group_row.enabled
          AND rule.rule_kind IN ('metric','state')
        ORDER BY rule.id, current_fact.natural_key,
                 current_fact.subject_client_id
        "#,
    )
    .bind(rule_ids)
    .fetch_all(&mut **tx)
    .await?;
    let rule_generations = candidates
        .iter()
        .map(|candidate| {
            Ok((
                candidate.try_get::<Uuid, _>("rule_id")?,
                candidate.try_get::<i32, _>("rule_version")?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let evidence_ids = candidates
        .iter()
        .map(|candidate| candidate.try_get::<Uuid, _>("evidence_id"))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let rules = load_evaluator_rules_by_generation_in_tx(tx, &rule_generations).await?;
    let evidences = load_stored_evidences_in_tx(tx, &evidence_ids).await?;
    let mut evaluated = 0_usize;
    for candidate in candidates {
        let rule_id: Uuid = candidate.try_get("rule_id")?;
        let rule_version: i32 = candidate.try_get("rule_version")?;
        let evidence_id: Uuid = candidate.try_get("evidence_id")?;
        let rule = rules
            .get(&(rule_id, rule_version))
            .context("alert policy baseline generation missing")?;
        let evidence = evidences
            .get(&evidence_id)
            .context("alert policy baseline evidence missing")?;
        if receipt_exists_in_tx(tx, rule, evidence.evidence_seq).await? {
            continue;
        }
        if !evidence_subject_scope_revision_is_current_in_tx(tx, evidence).await? {
            // The policy mutation owns this generation. A stale baseline
            // is terminal for that arm; the scope-revision owner appends
            // the current fact above the new boundary.
            record_receipt_in_tx(
                tx,
                rule,
                evidence,
                "unknown",
                Some("armed_baseline_scope_revision_stale"),
            )
            .await?;
            evaluated += 1;
            continue;
        }
        sqlx::query("SAVEPOINT alert_policy_baseline_evaluation")
            .execute(&mut **tx)
            .await?;
        match evaluate_rule_in_tx(tx, rule, evidence, false).await {
            Ok(result) => {
                sqlx::query("RELEASE SAVEPOINT alert_policy_baseline_evaluation")
                    .execute(&mut **tx)
                    .await?;
                record_receipt_in_tx(tx, rule, evidence, result, Some("armed_baseline")).await?;
            }
            Err(error) => {
                sqlx::query("ROLLBACK TO SAVEPOINT alert_policy_baseline_evaluation")
                    .execute(&mut **tx)
                    .await?;
                sqlx::query("RELEASE SAVEPOINT alert_policy_baseline_evaluation")
                    .execute(&mut **tx)
                    .await?;
                let detail = if is_lineage_overflow_error(&error) {
                    Some("policy_schedule_lineage_overflow")
                } else {
                    error
                        .downcast_ref::<DeterministicPolicyEvaluationError>()
                        .map(|error| error.0.as_str())
                };
                let Some(detail) = detail else {
                    return Err(error);
                };
                record_receipt_in_tx(
                    tx,
                    rule,
                    evidence,
                    if is_lineage_overflow_error(&error) {
                        "lineage_overflow"
                    } else {
                        "error"
                    },
                    Some(detail),
                )
                .await?;
                audit_policy_evaluation_skip_in_tx(tx, rule, evidence, detail).await?;
            }
        }
        evaluated += 1;
    }
    Ok(evaluated)
}

async fn evidence_subject_scope_revision_is_current_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    evidence: &StoredEvidence,
) -> Result<bool> {
    if evidence
        .payload
        .get("source_present")
        .and_then(Value::as_bool)
        == Some(false)
    {
        return Ok(true);
    }
    let Some(client_id) = evidence.subject_client_id.as_deref() else {
        return Ok(false);
    };
    let snapshot_revision = evidence
        .subject_snapshot
        .get("scope_revision")
        .and_then(Value::as_i64);
    let current_revision = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT policy_scope_revision
        FROM clients
        WHERE id=$1
          AND NOT EXISTS (
                SELECT 1
                FROM alert_policy_scope_dirty_clients scope_work
                WHERE scope_work.client_id=clients.id
                  AND scope_work.requires_revision_advance
          )
        "#,
    )
    .bind(client_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(snapshot_revision == current_revision)
}

pub(crate) async fn resolve_policy_rules_for_definition_change_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule_ids: &[Uuid],
    reason: &str,
) -> Result<()> {
    anyhow::ensure!(
        matches!(
            reason,
            "policy_disabled" | "policy_scope_changed" | "policy_changed" | "policy_deleted"
        ),
        "invalid policy definition resolution reason"
    );
    if rule_ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        r#"
        SELECT state.policy_rule_id, state.rule_version,
               state.confirmation_bucket_key
        FROM alert_policy_evaluation_states state
        WHERE state.policy_rule_id=ANY($1::uuid[])
        ORDER BY state.policy_rule_id, state.rule_version,
                 state.confirmation_bucket_key COLLATE "C"
        FOR UPDATE OF state
        "#,
    )
    .bind(rule_ids)
    .fetch_all(&mut **tx)
    .await?;
    let rows = sqlx::query(
        r#"
        SELECT episode.id, episode.policy_rule_id, episode.policy_rule_version,
               COALESCE(episode.last_evidence_id, episode.trigger_evidence_id) AS evidence_id,
               episode.trigger_generation, episode.lifecycle_state,
               episode.schedule_lineage, episode.triggered_at
        FROM alert_episodes episode
        WHERE episode.policy_rule_id=ANY($1::uuid[])
          AND episode.resolved_at IS NULL
        ORDER BY episode.id
        FOR UPDATE
        "#,
    )
    .bind(rule_ids)
    .fetch_all(&mut **tx)
    .await?;
    let generations = rows
        .iter()
        .map(|row| {
            Ok((
                row.try_get::<Uuid, _>("policy_rule_id")?,
                row.try_get::<i32, _>("policy_rule_version")?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let evidence_ids = rows
        .iter()
        .map(|row| row.try_get::<Uuid, _>("evidence_id"))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let rules = load_evaluator_rules_by_generation_in_tx(tx, &generations).await?;
    let evidences = load_stored_evidences_in_tx(tx, &evidence_ids).await?;
    for row in rows {
        let episode_id: Uuid = row.try_get("id")?;
        let rule_id: Uuid = row.try_get("policy_rule_id")?;
        let rule_version: i32 = row.try_get("policy_rule_version")?;
        let evidence_id: Uuid = row.try_get("evidence_id")?;
        let rule = rules
            .get(&(rule_id, rule_version))
            .context("active policy episode generation missing")?;
        let evidence = evidences
            .get(&evidence_id)
            .context("active policy episode evidence missing")?;
        let episode = ActiveEpisode {
            id: episode_id,
            last_evidence_id: evidence_id,
            generation: row.try_get("trigger_generation")?,
            lifecycle_state: row.try_get("lifecycle_state")?,
            schedule_lineage: row.try_get("schedule_lineage")?,
            triggered_at: row.try_get("triggered_at")?,
        };
        let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut **tx)
            .await?;
        sqlx::query(
            r#"
            UPDATE alert_episodes
            SET lifecycle_state='resolved',
                resolved_at=GREATEST($2,last_confirmed_at),
                resolution_reason=$3, updated_at=$2
            WHERE id=$1 AND resolved_at IS NULL
            "#,
        )
        .bind(episode_id)
        .bind(now)
        .bind(reason)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            r#"
            UPDATE alert_policy_evaluation_states
            SET active_episode_id=NULL, next_transition_at=NULL,
                truth_state='not_matched', updated_at=$2, last_evaluated_at=$2
            WHERE active_episode_id=$1
            "#,
        )
        .bind(episode_id)
        .bind(now)
        .execute(&mut **tx)
        .await?;
        emit_lifecycle_edge_in_tx(tx, rule, evidence, &episode, "alert.resolved", now).await?;
    }
    Ok(())
}

/// Resolves an ordered reviewed occurrence set after the caller has acquired
/// the subject suppression fences, immutable policy generations,
/// evaluation-state rows, and episode rows in that order. Immutable rule and
/// evidence snapshots are loaded once; every episode still performs the exact
/// same ordered state transition, confirmation cleanup and lifecycle edge.
pub(crate) async fn resolve_policy_occurrence_episodes_prelocked_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    public_ids: &[String],
    resolution_note: &str,
    actor_id: Uuid,
) -> Result<BTreeSet<String>> {
    let rows = sqlx::query(
        r#"
        SELECT episode.public_id, episode.id, episode.record_kind, episode.resolved_at,
               episode.resolution_reason, episode.resolution_note,
               episode.resolution_actor_id, episode.last_confirmed_at,
               episode.policy_rule_id, episode.policy_rule_version,
               COALESCE(episode.last_evidence_id,episode.trigger_evidence_id) AS evidence_id,
               episode.trigger_generation,
               episode.lifecycle_state, episode.schedule_lineage,
               episode.triggered_at
        FROM alert_episodes episode
        WHERE episode.public_id=ANY($1::text[])
        ORDER BY episode.public_id COLLATE "C"
        FOR UPDATE OF episode
        "#,
    )
    .bind(public_ids)
    .fetch_all(&mut **tx)
    .await?;
    anyhow::ensure!(rows.len() == public_ids.len(), "fleet_alert_not_found");

    let mut candidates = Vec::with_capacity(rows.len());
    for (row, expected_public_id) in rows.into_iter().zip(public_ids) {
        let public_id: String = row.try_get("public_id")?;
        anyhow::ensure!(&public_id == expected_public_id, "fleet_alert_not_found");
        anyhow::ensure!(
            row.try_get::<String, _>("record_kind")? == "event",
            "fleet_alert_condition_not_operator_resolvable"
        );
        let resolved_at: Option<DateTime<Utc>> = row.try_get("resolved_at")?;
        if resolved_at.is_some() {
            anyhow::ensure!(
                row.try_get::<Option<String>, _>("resolution_reason")?
                    .as_deref()
                    == Some("operator_resolved")
                    && row
                        .try_get::<Option<String>, _>("resolution_note")?
                        .is_some_and(|note| note.trim() == resolution_note.trim())
                    && row.try_get::<Option<Uuid>, _>("resolution_actor_id")? == Some(actor_id),
                "fleet_alert_already_resolved"
            );
            continue;
        }
        candidates.push(OccurrenceResolutionCandidate {
            public_id,
            id: row.try_get("id")?,
            last_confirmed_at: row.try_get("last_confirmed_at")?,
            rule_id: row.try_get("policy_rule_id")?,
            rule_version: row.try_get("policy_rule_version")?,
            evidence_id: row.try_get("evidence_id")?,
            trigger_generation: row.try_get("trigger_generation")?,
            lifecycle_state: row.try_get("lifecycle_state")?,
            schedule_lineage: row.try_get("schedule_lineage")?,
            triggered_at: row.try_get("triggered_at")?,
        });
    }
    let generations = candidates
        .iter()
        .map(|candidate| (candidate.rule_id, candidate.rule_version))
        .collect::<Vec<_>>();
    let evidence_ids = candidates
        .iter()
        .map(|candidate| candidate.evidence_id)
        .collect::<Vec<_>>();
    let rules = load_evaluator_rules_by_generation_in_tx(tx, &generations).await?;
    let evidences = load_stored_evidences_in_tx(tx, &evidence_ids).await?;
    let mut transitioned = BTreeSet::new();
    for candidate in &candidates {
        let rule = rules
            .get(&(candidate.rule_id, candidate.rule_version))
            .context("fleet alert policy generation missing")?;
        anyhow::ensure!(
            rule.kind == AlertPolicyRuleKind::Occurrence,
            "fleet_alert_condition_not_operator_resolvable"
        );
        let mut evidence = evidences
            .get(&candidate.evidence_id)
            .context("fleet alert evidence missing")?
            .clone();
        let episode = ActiveEpisode {
            id: candidate.id,
            last_evidence_id: candidate.evidence_id,
            generation: candidate.trigger_generation,
            lifecycle_state: candidate.lifecycle_state.clone(),
            schedule_lineage: candidate.schedule_lineage.clone(),
            triggered_at: candidate.triggered_at,
        };
        let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut **tx)
            .await?;
        let resolved_at = now.max(candidate.last_confirmed_at);
        sqlx::query(
            r#"
            UPDATE alert_episodes
            SET lifecycle_state='resolved', resolved_at=$2,
                resolution_reason='operator_resolved', resolution_note=$3,
                resolution_actor_id=$4, updated_at=$2
            WHERE id=$1 AND resolved_at IS NULL
            "#,
        )
        .bind(episode.id)
        .bind(resolved_at)
        .bind(resolution_note)
        .bind(actor_id)
        .execute(&mut **tx)
        .await?;
        let bucket = effective_confirmation_bucket(rule, &evidence)?;
        evidence.confirmation_bucket_key.clone_from(&bucket);
        let updated = sqlx::query(
            r#"
            UPDATE alert_policy_evaluation_states
            SET active_episode_id=NULL,
                occurrence_cohort_id=$4, truth_state='not_matched',
                next_transition_at=NULL,
                trigger_confirmed_duration_secs=0, trigger_segment_started_at=NULL,
                resolve_confirmed_duration_secs=0, resolve_segment_started_at=NULL,
                last_evaluated_at=$5, updated_at=$5
            WHERE policy_rule_id=$1 AND rule_version=$2
              AND confirmation_bucket_key=$3 AND active_episode_id=$6
            "#,
        )
        .bind(rule.id)
        .bind(rule.rule_version)
        .bind(&bucket)
        .bind(Uuid::new_v4())
        .bind(resolved_at)
        .bind(episode.id)
        .execute(&mut **tx)
        .await?;
        anyhow::ensure!(
            updated.rows_affected() == 1,
            "fleet_alert_resolution_snapshot_stale"
        );
        clear_confirmations_in_tx(tx, rule, &evidence, GatePhase::Trigger).await?;
        clear_confirmations_in_tx(tx, rule, &evidence, GatePhase::Resolve).await?;
        let mut resolved = episode;
        resolved.lifecycle_state = "resolved".to_string();
        emit_lifecycle_edge_in_tx(
            tx,
            rule,
            &evidence,
            &resolved,
            "alert.resolved",
            resolved_at,
        )
        .await?;
        transitioned.insert(candidate.public_id.clone());
    }
    Ok(transitioned)
}

struct OccurrenceResolutionCandidate {
    public_id: String,
    id: Uuid,
    last_confirmed_at: DateTime<Utc>,
    rule_id: Uuid,
    rule_version: i32,
    evidence_id: Uuid,
    trigger_generation: i64,
    lifecycle_state: String,
    schedule_lineage: Vec<Uuid>,
    triggered_at: DateTime<Utc>,
}

async fn evaluate_rule_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
    timer_recheck: bool,
) -> Result<&'static str> {
    let mut effective_evidence = evidence.clone();
    effective_evidence.confirmation_bucket_key = effective_confirmation_bucket(rule, evidence)?;
    let evidence = &effective_evidence;
    if rule.kind != evidence.fact_kind {
        return Err(DeterministicPolicyEvaluationError(
            "policy_evidence_kind_mismatch".to_string(),
        )
        .into());
    }
    let mut state = lock_or_create_state_in_tx(tx, rule, evidence).await?;
    if matches!(
        rule.kind,
        AlertPolicyRuleKind::Metric | AlertPolicyRuleKind::State
    ) && !timer_recheck
        && state.last_evidence_observed_at.is_some_and(|last| {
            evidence.observed_at < last
                || (evidence.observed_at == last
                    && state
                        .last_evidence_seq
                        .is_some_and(|last_seq| evidence.evidence_seq <= last_seq))
        })
    {
        // Receipt ownership still advances, but delayed evaluation cannot mutate
        // truth, scope, presentation, or timers established by a newer fact.
        return Ok("stale");
    }
    if evidence
        .payload
        .get("source_present")
        .and_then(Value::as_bool)
        == Some(false)
        && evidence
            .payload
            .get("resolution_reason")
            .and_then(Value::as_str)
            == Some("source_scope_exited")
    {
        resolve_source_scope_exit_in_tx(tx, rule, evidence, &mut state).await?;
        return Ok("source_scope_exited");
    }
    match policy_scope_truth(rule, evidence)? {
        ScopeTruth::OutOfScope => {
            if rule.kind != AlertPolicyRuleKind::Occurrence {
                resolve_scope_exit_in_tx(tx, rule, evidence, &mut state).await?;
            }
            return Ok("out_of_scope");
        }
        ScopeTruth::Unknown => {
            let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
                .fetch_one(&mut **tx)
                .await?;
            if rule.kind == AlertPolicyRuleKind::Occurrence {
                // An indeterminate selector on a later occurrence is not a
                // state transition for an already-open immutable event. Keep
                // its elapsed deadline, cohort, presentation, and provenance
                // unchanged while consuming this candidate as Unknown.
                state.last_evidence_seq = Some(evidence.evidence_seq);
                state.last_evidence_source_event_id = Some(evidence.source_event_id.clone());
                state.last_evidence_observed_at = Some(evidence.observed_at);
                persist_state_in_tx(tx, rule, evidence, &state, now).await?;
                return Ok("unknown");
            }
            if let Some(active) = load_active_episode_in_tx(tx, state.active_episode_id).await? {
                reset_gate_if_contradicted_in_tx(
                    tx,
                    rule,
                    evidence,
                    &mut state,
                    GatePhase::Resolve,
                    ExpressionTruth::Unknown,
                    now,
                )
                .await?;
                mark_episode_unknown_in_tx(tx, evidence, &active, now).await?;
            } else {
                reset_gate_if_contradicted_in_tx(
                    tx,
                    rule,
                    evidence,
                    &mut state,
                    GatePhase::Trigger,
                    ExpressionTruth::Unknown,
                    now,
                )
                .await?;
            }
            state.truth_state = "unknown".to_string();
            state.last_evidence_seq = Some(evidence.evidence_seq);
            state.last_evidence_source_event_id = Some(evidence.source_event_id.clone());
            state.last_evidence_observed_at = Some(evidence.observed_at);
            persist_state_in_tx(tx, rule, evidence, &state, now).await?;
            return Ok("unknown");
        }
        ScopeTruth::InScope => {}
    }
    let trigger_truth = expression_truth_for_evidence(rule, &rule.trigger_expression, evidence)?;
    let resolve_truth = if rule.kind == AlertPolicyRuleKind::Occurrence {
        ExpressionTruth::False
    } else if let Some(expression) = rule.resolve_expression.as_deref() {
        expression_truth_for_evidence(rule, expression, evidence)?
    } else {
        match trigger_truth {
            ExpressionTruth::True => ExpressionTruth::False,
            ExpressionTruth::False => ExpressionTruth::True,
            ExpressionTruth::Unknown => ExpressionTruth::Unknown,
        }
    };

    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **tx)
        .await?;
    let mut active = load_active_episode_in_tx(tx, state.active_episode_id).await?;

    if active.is_some() && resolve_truth == ExpressionTruth::True {
        if advance_gate_in_tx(
            tx,
            rule,
            evidence,
            &mut state,
            GatePhase::Resolve,
            rule.resolve_meta.as_ref(),
            now,
        )
        .await?
        {
            let episode = active.take().context("active policy episode missing")?;
            resolve_episode_in_tx(tx, rule, evidence, &episode, now, None).await?;
            state.active_episode_id = None;
            state.active_triggered_at = None;
            state.resolve_confirmed_duration_secs = 0;
            state.resolve_segment_started_at = None;
            clear_confirmations_in_tx(tx, rule, evidence, GatePhase::Resolve).await?;
            if rule.kind == AlertPolicyRuleKind::Occurrence {
                clear_confirmations_in_tx(tx, rule, evidence, GatePhase::Trigger).await?;
                state.occurrence_cohort_id = Some(Uuid::new_v4());
            }
        } else if rule.kind != AlertPolicyRuleKind::Occurrence {
            // Recovery evidence is conclusive even while its Sustained/Count
            // gate is still pending. Keep the latched incident visibly
            // Persisting; Unknown is reserved for unavailable/indeterminate
            // evidence, not for a known recovery awaiting confirmation.
            persist_episode_confirmation_in_tx(
                tx,
                rule,
                evidence,
                active.as_ref().expect("checked active episode"),
                now,
            )
            .await?;
        }
    } else if active.is_some() {
        reset_gate_if_contradicted_in_tx(
            tx,
            rule,
            evidence,
            &mut state,
            GatePhase::Resolve,
            resolve_truth,
            now,
        )
        .await?;
        if trigger_truth == ExpressionTruth::True {
            persist_episode_confirmation_in_tx(
                tx,
                rule,
                evidence,
                active.as_ref().expect("checked active episode"),
                now,
            )
            .await?;
            // Occurrence facts while an episode is open belong to the current
            // cohort and are consumed; they never pre-fill the next cohort.
            if rule.kind == AlertPolicyRuleKind::Occurrence {
                clear_confirmations_in_tx(tx, rule, evidence, GatePhase::Trigger).await?;
            }
        } else if (trigger_truth == ExpressionTruth::Unknown
            || resolve_truth == ExpressionTruth::Unknown)
            && rule.kind != AlertPolicyRuleKind::Occurrence
        {
            mark_episode_unknown_in_tx(tx, evidence, active.as_ref().unwrap(), now).await?;
        } else if rule.kind != AlertPolicyRuleKind::Occurrence
            && trigger_truth == ExpressionTruth::False
            && resolve_truth == ExpressionTruth::False
        {
            // A complete sample in an explicit hysteresis band proves the
            // latched condition is still current even though neither phase
            // matches. Restore Persisting after an Unknown interval without
            // advancing the trigger confirmation gate.
            persist_episode_confirmation_in_tx(
                tx,
                rule,
                evidence,
                active.as_ref().expect("checked active episode"),
                now,
            )
            .await?;
        }
    } else if trigger_truth == ExpressionTruth::True {
        let gate_passed = advance_gate_in_tx(
            tx,
            rule,
            evidence,
            &mut state,
            GatePhase::Trigger,
            rule.trigger_meta.as_ref(),
            now,
        )
        .await?;
        if gate_passed {
            if let Some(episode) =
                trigger_episode_in_tx(tx, rule, evidence, &mut state, now).await?
            {
                state.active_episode_id = Some(episode.id);
                state.active_triggered_at = Some(episode.triggered_at);
            } else if rule.kind == AlertPolicyRuleKind::Occurrence {
                state.occurrence_cohort_id = Some(Uuid::new_v4());
            }
            // A completed trigger gate belongs only to the generation it
            // opened. Retaining its dwell would let a later recovery followed
            // by the same condition retrigger immediately.
            state.trigger_confirmed_duration_secs = 0;
            state.trigger_segment_started_at = None;
        }
    } else {
        reset_gate_if_contradicted_in_tx(
            tx,
            rule,
            evidence,
            &mut state,
            GatePhase::Trigger,
            trigger_truth,
            now,
        )
        .await?;
    }

    state.truth_state = match trigger_truth {
        ExpressionTruth::True => "matched",
        ExpressionTruth::False => "not_matched",
        ExpressionTruth::Unknown => "unknown",
    }
    .to_string();
    state.last_evidence_observed_at = Some(evidence.observed_at);
    state.last_evidence_seq = Some(evidence.evidence_seq);
    state.last_evidence_source_event_id = Some(evidence.source_event_id.clone());
    persist_state_in_tx(tx, rule, evidence, &state, now).await?;
    Ok(match trigger_truth {
        ExpressionTruth::True => "matched",
        ExpressionTruth::False => "not_matched",
        ExpressionTruth::Unknown => "unknown",
    })
}

async fn advance_gate_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
    state: &mut EvaluationState,
    phase: GatePhase,
    meta: Option<&AlertPolicyMetaCondition>,
    now: DateTime<Utc>,
) -> Result<bool> {
    match meta {
        None | Some(AlertPolicyMetaCondition::Immediate) => Ok(true),
        Some(AlertPolicyMetaCondition::Sustained { seconds }) => {
            retain_sustained_confirmation_anchor_in_tx(tx, rule, evidence, phase).await?;
            let (accumulated, segment) = phase_duration_fields(state, phase);
            let segment = segment.unwrap_or_else(|| evidence_state_boundary(rule, evidence, now));
            set_phase_segment(state, phase, Some(segment));
            let elapsed = now.signed_duration_since(segment).num_seconds().max(0);
            let satisfied = accumulated.saturating_add(elapsed) >= *seconds;
            Ok(satisfied)
        }
        Some(AlertPolicyMetaCondition::Count {
            confirmations,
            within_seconds,
        }) => {
            // A scope revision can reset/reevaluate truth, but it is not a new
            // authoritative source-state observation and therefore cannot
            // manufacture a Count confirmation.
            if evidence_is_source_confirmation(evidence) {
                insert_confirmation_in_tx(tx, rule, evidence, phase).await?;
            }
            let cutoff = now - Duration::seconds(*within_seconds);
            sqlx::query(
                r#"
                DELETE FROM alert_policy_confirmations
                WHERE policy_rule_id = $1 AND rule_version = $2
                  AND confirmation_bucket_key = $3 AND phase = $4
                  AND accepted_at < $5
                "#,
            )
            .bind(rule.id)
            .bind(rule.rule_version)
            .bind(&evidence.confirmation_bucket_key)
            .bind(phase.storage())
            .bind(cutoff)
            .execute(&mut **tx)
            .await?;
            let count: i64 = sqlx::query_scalar(
                r#"
                SELECT count(*)
                FROM alert_policy_confirmations
                WHERE policy_rule_id = $1 AND rule_version = $2
                  AND confirmation_bucket_key = $3 AND phase = $4
                "#,
            )
            .bind(rule.id)
            .bind(rule.rule_version)
            .bind(&evidence.confirmation_bucket_key)
            .bind(phase.storage())
            .fetch_one(&mut **tx)
            .await?;
            Ok(count >= i64::from(*confirmations))
        }
        Some(AlertPolicyMetaCondition::ElapsedSinceTrigger { .. }) => Ok(false),
    }
}

async fn reset_gate_if_contradicted_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
    state: &mut EvaluationState,
    phase: GatePhase,
    truth: ExpressionTruth,
    now: DateTime<Utc>,
) -> Result<()> {
    let meta = match phase {
        GatePhase::Trigger => rule.trigger_meta.as_ref(),
        GatePhase::Resolve => rule.resolve_meta.as_ref(),
    };
    match truth {
        ExpressionTruth::False => {
            set_phase_duration(state, phase, 0);
            set_phase_segment(state, phase, None);
            if matches!(meta, Some(AlertPolicyMetaCondition::Count { .. }))
                && rule.kind == AlertPolicyRuleKind::Occurrence
                && phase == GatePhase::Trigger
            {
                prune_count_confirmations_in_tx(tx, rule, evidence, phase, meta, now).await?;
            } else {
                clear_confirmations_in_tx(tx, rule, evidence, phase).await?;
            }
        }
        ExpressionTruth::Unknown => {
            let (accumulated, segment) = phase_duration_fields(state, phase);
            if let Some(segment) = segment {
                let pause_at = evidence_state_boundary(rule, evidence, now).max(segment);
                let confirmed = pause_at.signed_duration_since(segment).num_seconds().max(0);
                set_phase_duration(state, phase, accumulated.saturating_add(confirmed));
                set_phase_segment(state, phase, None);
            }
            // Unknown contributes no count sample; existing samples continue
            // to age by their DB acceptance time and are not reset.
            prune_count_confirmations_in_tx(tx, rule, evidence, phase, meta, now).await?;
        }
        ExpressionTruth::True => {}
    }
    Ok(())
}

async fn trigger_episode_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
    state: &mut EvaluationState,
    now: DateTime<Utc>,
) -> Result<Option<ActiveEpisode>> {
    if rule.correlation_mode != AlertPolicyCorrelationMode::Global {
        if let Some(client_id) = evidence.subject_client_id.as_deref() {
            let suppression_lock_acquired: bool = sqlx::query_scalar(
                "SELECT pg_try_advisory_xact_lock_shared(hashtextextended($1, 0))",
            )
            .bind(vpsman_server_core::client_policy_suppression_lock_key(
                client_id,
            ))
            .fetch_one(&mut **tx)
            .await?;
            let subject_suspended = suppression_lock_acquired
                && sqlx::query_scalar::<_, bool>(
                    "SELECT status='suspended' FROM clients WHERE id=$1",
                )
                .bind(client_id)
                .fetch_optional(&mut **tx)
                .await?
                .unwrap_or(false);
            if !suppression_lock_acquired || subject_suspended {
                clear_confirmations_in_tx(tx, rule, evidence, GatePhase::Trigger).await?;
                return Ok(None);
            }
        }
    }
    let id = Uuid::new_v4();
    state.trigger_generation = state.trigger_generation.saturating_add(1);
    let natural_key = if rule.kind == AlertPolicyRuleKind::Occurrence
        && matches!(
            rule.trigger_meta,
            Some(AlertPolicyMetaCondition::Count { .. })
        ) {
        let cohort = state.occurrence_cohort_id.get_or_insert_with(Uuid::new_v4);
        format!("cohort:{cohort}")
    } else {
        evidence.natural_key.clone()
    };
    let lineage = contributing_lineage_in_tx(tx, rule, evidence, GatePhase::Trigger, &[]).await?;
    let mut episode_evidence = evidence.clone();
    if rule.correlation_mode == AlertPolicyCorrelationMode::Global {
        episode_evidence.subject_client_id = None;
        episode_evidence.target_kind = "policy_source".to_string();
        episode_evidence.target_id = format!("global:{}", evidence.source_kind);
        episode_evidence.subject_snapshot = json!({
            "scope": "global",
            "evidence_source": evidence.source_kind,
        });
    }
    let context = lifecycle_template_context(rule, &episode_evidence);
    let title = render_policy_template(&rule.title_template, &context, 256)?;
    let detail = render_policy_template(&rule.detail_template, &context, 4096)?;
    let payload =
        episode_evidence_payload(rule, evidence, &context, GatePhase::Trigger, tx).await?;
    let triggered_at = now;
    let lifecycle_state = "triggered";
    let record_kind = match rule.kind {
        AlertPolicyRuleKind::Occurrence => "event",
        AlertPolicyRuleKind::Metric | AlertPolicyRuleKind::State => "condition",
    };
    let public_id = format!("policy-alert:{id}");

    sqlx::query(
        r#"
        INSERT INTO alert_episodes (
            id, public_id, producer_kind, natural_key, record_kind,
            trigger_generation, trigger_severity, trigger_category,
            severity, category, target_kind, target_id, client_id,
            title, detail, source_status, evidence, lifecycle_state,
            triggered_at, last_confirmed_at,
            policy_group_id, policy_rule_id, policy_rule_version, policy_rule_kind,
            policy_group_name, policy_rule_name, policy_rule_system_seed_key,
            trigger_evidence_id, last_evidence_id,
            causation_id, schedule_lineage, created_at, updated_at
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $7, $8, $9, $10, $11,
            $12, $13, $14, $15, $17,
            $16, $16,
            $18, $19, $20, $21, $22, $23, $24, $25, $25,
            $26, $27, $28, $28
        )
        "#,
    )
    .bind(id)
    .bind(&public_id)
    .bind(&evidence.source_kind)
    .bind(&natural_key)
    .bind(record_kind)
    .bind(state.trigger_generation)
    .bind(&rule.severity)
    .bind(&rule.category)
    .bind(&episode_evidence.target_kind)
    .bind(&episode_evidence.target_id)
    .bind(&episode_evidence.subject_client_id)
    .bind(&title)
    .bind(&detail)
    .bind(&evidence.source_status)
    .bind(SqlJson(payload))
    .bind(triggered_at)
    .bind(lifecycle_state)
    .bind(rule.group_id)
    .bind(rule.id)
    .bind(rule.rule_version)
    .bind(rule_kind_storage(rule.kind))
    .bind(&rule.group_name)
    .bind(&rule.name)
    .bind(&rule.system_seed_key)
    .bind(evidence.id)
    .bind(evidence.causation_id)
    .bind(&lineage)
    .bind(now)
    .execute(&mut **tx)
    .await?;

    let episode = ActiveEpisode {
        id,
        last_evidence_id: evidence.id,
        generation: state.trigger_generation,
        lifecycle_state: lifecycle_state.to_string(),
        schedule_lineage: lineage,
        triggered_at,
    };
    emit_lifecycle_edge_in_tx(
        tx,
        rule,
        &episode_evidence,
        &episode,
        "alert.triggered",
        now,
    )
    .await?;

    if let Some(AlertPolicyMetaCondition::ElapsedSinceTrigger { seconds }) = rule.resolve_meta {
        // The due timestamp is persisted by the state upsert below.
        let _ = seconds;
    }
    clear_confirmations_in_tx(tx, rule, evidence, GatePhase::Trigger).await?;
    Ok(Some(episode))
}

async fn resolve_episode_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
    episode: &ActiveEpisode,
    now: DateTime<Utc>,
    reason_override: Option<&str>,
) -> Result<()> {
    let lineage = contributing_lineage_in_tx(
        tx,
        rule,
        evidence,
        GatePhase::Resolve,
        &episode.schedule_lineage,
    )
    .await?;
    let reason = reason_override.unwrap_or_else(|| {
        if rule.resolve_expression.is_some() {
            "recovery_expression_matched"
        } else {
            "condition_recovered"
        }
    });
    let context = lifecycle_template_context(rule, evidence);
    let resolution_snapshot =
        episode_evidence_payload(rule, evidence, &context, GatePhase::Resolve, tx).await?;
    sqlx::query(
        r#"
        UPDATE alert_episodes
        SET lifecycle_state = 'resolved', resolved_at = $2,
            resolution_reason = $3, last_evidence_id = $4,
            source_status = $5,
            evidence = evidence || jsonb_build_object(
                'resolution_evidence_id', $4::uuid,
                'resolution_observed_at', $6::timestamptz,
                'resolution_evidence_snapshot', $9::jsonb,
                'resolution_confirmation_evidence', COALESCE(
                    $9::jsonb->'confirmation_evidence', '[]'::jsonb
                )
            ),
            causation_id = COALESCE($7, causation_id),
            schedule_lineage = $8, updated_at = $2
        WHERE id = $1 AND resolved_at IS NULL
        "#,
    )
    .bind(episode.id)
    .bind(now)
    .bind(reason)
    .bind(evidence.id)
    .bind(&evidence.source_status)
    .bind(evidence.observed_at)
    .bind(evidence.causation_id)
    .bind(&lineage)
    .bind(SqlJson(resolution_snapshot))
    .execute(&mut **tx)
    .await?;
    let mut resolved = episode.clone();
    resolved.lifecycle_state = "resolved".to_string();
    resolved.schedule_lineage = lineage;
    emit_lifecycle_edge_in_tx(tx, rule, evidence, &resolved, "alert.resolved", now).await
}

async fn persist_episode_confirmation_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
    episode: &ActiveEpisode,
    now: DateTime<Utc>,
) -> Result<()> {
    let lineage = contributing_lineage_in_tx(
        tx,
        rule,
        evidence,
        GatePhase::Trigger,
        &episode.schedule_lineage,
    )
    .await?;
    let context = lifecycle_template_context(rule, evidence);
    let title = render_policy_template(&rule.title_template, &context, 256)?;
    let detail = render_policy_template(&rule.detail_template, &context, 4096)?;
    sqlx::query(
        r#"
        UPDATE alert_episodes
        SET lifecycle_state = 'persisting', severity = $2, category = $3,
            title = $4, detail = $5, source_status = $6,
            evidence = $7 || jsonb_build_object(
                'trigger_evidence_snapshot', COALESCE(
                    evidence->'trigger_evidence_snapshot', evidence
                ),
                'confirmation_evidence', COALESCE(
                    evidence->'confirmation_evidence', '[]'::jsonb
                )
            ),
            last_evidence_id = $8, last_confirmed_at = $9,
            causation_id = COALESCE($10, causation_id), schedule_lineage = $11,
            updated_at = $9
        WHERE id = $1 AND resolved_at IS NULL
        "#,
    )
    .bind(episode.id)
    .bind(&rule.severity)
    .bind(&rule.category)
    .bind(title)
    .bind(detail)
    .bind(&evidence.source_status)
    .bind(SqlJson(
        episode_evidence_payload(rule, evidence, &context, GatePhase::Trigger, tx).await?,
    ))
    .bind(evidence.id)
    .bind(now)
    .bind(evidence.causation_id)
    .bind(lineage)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn mark_episode_unknown_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    evidence: &StoredEvidence,
    episode: &ActiveEpisode,
    now: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE alert_episodes
        SET lifecycle_state = 'unknown', source_status = $2,
            evidence = evidence || jsonb_build_object(
                'latest_evidence_id', $3::uuid,
                'latest_evidence_observed_at', $4::timestamptz,
                'evidence_completeness', 'unknown'
            ),
            last_evidence_id = $3, updated_at = $5
        WHERE id = $1 AND resolved_at IS NULL
        "#,
    )
    .bind(episode.id)
    .bind(&evidence.source_status)
    .bind(evidence.id)
    .bind(evidence.observed_at)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn emit_lifecycle_edge_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
    episode: &ActiveEpisode,
    edge_kind: &str,
    occurred_at: DateTime<Utc>,
) -> Result<()> {
    let suffix = if edge_kind == "alert.triggered" {
        "triggered"
    } else {
        "resolved"
    };
    let row = sqlx::query(
        r#"
        SELECT public_id, record_kind, lifecycle_state, title, detail,
               severity, category, target_kind, target_id, client_id,
               source_status, resolution_reason, resolution_note,
               triggered_at, resolved_at,
               policy_group_id, policy_group_name, policy_rule_id,
               policy_rule_name, policy_rule_version, policy_rule_kind,
               policy_rule_system_seed_key, causation_id, schedule_lineage
        FROM alert_episodes
        WHERE id=$1 AND trigger_generation=$2
        FOR SHARE
        "#,
    )
    .bind(episode.id)
    .bind(episode.generation)
    .fetch_one(&mut **tx)
    .await?;
    let public_id: String = row.try_get("public_id")?;
    let record_kind: String = row.try_get("record_kind")?;
    let lifecycle_state: String = row.try_get("lifecycle_state")?;
    let title: String = row.try_get("title")?;
    let detail: String = row.try_get("detail")?;
    let severity: String = row.try_get("severity")?;
    let category: String = row.try_get("category")?;
    let target_kind: String = row.try_get("target_kind")?;
    let target_id: String = row.try_get("target_id")?;
    let client_id: Option<String> = row.try_get("client_id")?;
    let source_status: String = row.try_get("source_status")?;
    let resolution_reason: Option<String> = row.try_get("resolution_reason")?;
    let resolution_note: Option<String> = row.try_get("resolution_note")?;
    let triggered_at: DateTime<Utc> = row.try_get("triggered_at")?;
    let resolved_at: Option<DateTime<Utc>> = row.try_get("resolved_at")?;
    let policy_group_id: Uuid = row.try_get("policy_group_id")?;
    let policy_group_name: String = row.try_get("policy_group_name")?;
    let policy_rule_id: Uuid = row.try_get("policy_rule_id")?;
    let policy_rule_name: String = row.try_get("policy_rule_name")?;
    let policy_rule_version: i32 = row.try_get("policy_rule_version")?;
    let policy_rule_kind: String = row.try_get("policy_rule_kind")?;
    let system_seed_key: Option<String> = row.try_get("policy_rule_system_seed_key")?;
    let causation_id: Option<Uuid> = row.try_get("causation_id")?;
    let schedule_lineage: Vec<Uuid> = row.try_get("schedule_lineage")?;
    let recorded_at: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **tx)
        .await?;
    let event_id = format!("fleet-alert:{}:{suffix}", episode.id);
    let predicates = vec![
        edge_kind.to_string(),
        format!("alert.category:{category}"),
        format!("alert.severity:{severity}"),
    ];
    let subject_ids = client_id.iter().cloned().collect::<Vec<_>>();
    let payload = json!({
        "event": {
            "id": event_id,
            "kind": edge_kind,
            "occurred_at": occurred_at.to_rfc3339(),
            "recorded_at": recorded_at.to_rfc3339(),
            "predicates": predicates,
        },
        "alert": {
            "id": public_id,
            "public_id": public_id,
            "episode_id": episode.id,
            "record_kind": record_kind,
            "title": title,
            "detail": detail,
            "lifecycle_state": lifecycle_state,
            "trigger_generation": episode.generation,
            "severity": severity,
            "category": category,
            "target_kind": target_kind,
            "target_id": target_id,
            "client_id": client_id,
            "source_status": source_status,
            "resolution_reason": resolution_reason,
            "resolution_note": resolution_note,
            "triggered_at": triggered_at.to_rfc3339(),
            "resolved_at": resolved_at.map(|value| value.to_rfc3339()),
        },
        "policy": {"id": policy_group_id, "name": policy_group_name},
        "policy_rule": {
            "id": policy_rule_id,
            "name": policy_rule_name,
            "rule_version": policy_rule_version,
            "rule_kind": policy_rule_kind,
            "evidence_source": rule.evidence_source,
            "system_seed_key": system_seed_key,
            "trigger_meta_condition": normalized_meta(rule.trigger_meta.as_ref()),
            "resolve_meta_condition": normalized_meta(rule.resolve_meta.as_ref()),
        },
        "evidence": evidence.payload,
    });
    let inserted = sqlx::query(
        r#"
        INSERT INTO alert_lifecycle_events (
            id, episode_id, trigger_generation, edge_kind, event_id,
            event_predicates, subject_client_ids, payload, causation_id,
            schedule_lineage, occurred_at, created_at
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
        ON CONFLICT (episode_id, trigger_generation, edge_kind) DO NOTHING
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(episode.id)
    .bind(episode.generation)
    .bind(edge_kind)
    .bind(event_id)
    .bind(predicates)
    .bind(subject_ids)
    .bind(SqlJson(payload))
    .bind(causation_id)
    .bind(&schedule_lineage)
    .bind(occurred_at)
    .bind(recorded_at)
    .execute(&mut **tx)
    .await?;
    if inserted.rows_affected() == 1 {
        // The lifecycle row is the durable work owner. PostgreSQL publishes
        // this wake only if the enclosing source transaction commits, so the
        // worker can never observe a notification without its replay row.
        sqlx::query("SELECT pg_notify('webhook_events', 'alert_lifecycle')")
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

async fn load_evaluator_rules_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    source_kind: &str,
) -> Result<Vec<EvaluatorRule>> {
    let rows = sqlx::query(
        r#"
        SELECT rule.id, rule.group_id, group_row.name AS group_name,
               group_row.selector_expression AS group_selector,
               rule.rule_version, rule.name, rule.rule_kind, rule.evidence_source,
               rule.correlation_mode, rule.trigger_condition_expression,
               rule.trigger_meta_condition, rule.resolve_condition_expression,
               rule.resolve_meta_condition, rule.severity, rule.category,
               rule.title_template, rule.detail_template, rule.system_seed_key,
               rule.armed_after_evidence_seq, rule.armed_at
        FROM policy_rules rule
        JOIN policy_groups group_row ON group_row.id = rule.group_id
        WHERE rule.enabled AND group_row.enabled AND rule.evidence_source = $1
          AND ($1<>'telemetry.combined' OR EXISTS (
              SELECT 1
              FROM alert_telemetry_policy_activation activation
              WHERE activation.singleton
                AND activation.desired_enabled
                AND activation.effective_generation=activation.generation
          ))
        ORDER BY rule.group_id, rule.sort_order, rule.id
        FOR KEY SHARE OF rule
        "#,
    )
    .bind(source_kind)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter().map(evaluator_rule_from_row).collect()
}

async fn load_evaluator_rule_by_id_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule_id: Uuid,
    rule_version: i32,
) -> Result<EvaluatorRule> {
    let row = sqlx::query(
        r#"
        SELECT rule.id, rule.group_id, group_row.name AS group_name,
               group_row.selector_expression AS group_selector,
               rule.rule_version, rule.name, rule.rule_kind, rule.evidence_source,
               rule.correlation_mode, rule.trigger_condition_expression,
               rule.trigger_meta_condition, rule.resolve_condition_expression,
               rule.resolve_meta_condition, rule.severity, rule.category,
               rule.title_template, rule.detail_template, rule.system_seed_key,
               rule.armed_after_evidence_seq, rule.armed_at
        FROM policy_rules rule
        JOIN policy_groups group_row ON group_row.id=rule.group_id
        WHERE rule.id=$1 AND rule.rule_version=$2
        "#,
    )
    .bind(rule_id)
    .bind(rule_version)
    .fetch_one(&mut **tx)
    .await?;
    evaluator_rule_from_row(row)
}

async fn load_evaluator_rules_by_generation_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    generations: &[(Uuid, i32)],
) -> Result<HashMap<(Uuid, i32), EvaluatorRule>> {
    let generations = generations.iter().copied().collect::<BTreeSet<_>>();
    if generations.is_empty() {
        return Ok(HashMap::new());
    }
    let rule_ids = generations
        .iter()
        .map(|(rule_id, _)| *rule_id)
        .collect::<Vec<_>>();
    let rule_versions = generations
        .iter()
        .map(|(_, rule_version)| *rule_version)
        .collect::<Vec<_>>();
    let rows = sqlx::query(
        r#"
        SELECT rule.id, rule.group_id, group_row.name AS group_name,
               group_row.selector_expression AS group_selector,
               rule.rule_version, rule.name, rule.rule_kind, rule.evidence_source,
               rule.correlation_mode, rule.trigger_condition_expression,
               rule.trigger_meta_condition, rule.resolve_condition_expression,
               rule.resolve_meta_condition, rule.severity, rule.category,
               rule.title_template, rule.detail_template, rule.system_seed_key,
               rule.armed_after_evidence_seq, rule.armed_at
        FROM unnest($1::uuid[], $2::integer[])
             AS requested(id, rule_version)
        JOIN policy_rules rule
          ON rule.id=requested.id AND rule.rule_version=requested.rule_version
        JOIN policy_groups group_row ON group_row.id=rule.group_id
        ORDER BY rule.id, rule.rule_version
        "#,
    )
    .bind(rule_ids)
    .bind(rule_versions)
    .fetch_all(&mut **tx)
    .await?;
    let rules = rows
        .into_iter()
        .map(evaluator_rule_from_row)
        .collect::<Result<Vec<_>>>()?;
    anyhow::ensure!(
        rules.len() == generations.len(),
        "alert policy generation missing"
    );
    Ok(rules
        .into_iter()
        .map(|rule| ((rule.id, rule.rule_version), rule))
        .collect())
}

async fn lock_evaluator_rule_generation_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule_id: Uuid,
    rule_version: i32,
) -> Result<()> {
    let locked = lock_policy_rule_generations_shared_in_tx(tx, &[(rule_id, rule_version)]).await?;
    anyhow::ensure!(
        locked == 1,
        "alert policy generation retired before its evidence drained"
    );
    Ok(())
}

pub(crate) async fn lock_policy_rule_generations_shared_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    generations: &[(Uuid, i32)],
) -> Result<usize> {
    let generations = generations.iter().copied().collect::<BTreeSet<_>>();
    if generations.is_empty() {
        return Ok(0);
    }
    let rule_ids = generations
        .iter()
        .map(|(rule_id, _)| *rule_id)
        .collect::<Vec<_>>();
    let rule_versions = generations
        .iter()
        .map(|(_, rule_version)| *rule_version)
        .collect::<Vec<_>>();
    let locked = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT rule.id
        FROM unnest($1::uuid[], $2::integer[])
             AS requested(id, rule_version)
        JOIN policy_rules rule
          ON rule.id=requested.id AND rule.rule_version=requested.rule_version
        ORDER BY rule.id, rule.rule_version
        FOR KEY SHARE OF rule
        "#,
    )
    .bind(rule_ids)
    .bind(rule_versions)
    .fetch_all(&mut **tx)
    .await?;
    Ok(locked.len())
}

async fn load_stored_evidence_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    evidence_id: Uuid,
) -> Result<StoredEvidence> {
    let row = sqlx::query(
        r#"
        SELECT id, evidence_seq, source_kind, source_event_id, fact_kind,
               natural_key, confirmation_bucket_key, subject_client_id,
               target_kind, target_id, source_status, completeness,
               subject_snapshot, payload, observed_at, state_started_at, causation_id,
               schedule_lineage, created_at
        FROM alert_policy_evidence
        WHERE id=$1
        "#,
    )
    .bind(evidence_id)
    .fetch_one(&mut **tx)
    .await?;
    stored_evidence_from_row(row)
}

async fn load_stored_evidences_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    evidence_ids: &[Uuid],
) -> Result<HashMap<Uuid, StoredEvidence>> {
    let evidence_ids = evidence_ids.iter().copied().collect::<BTreeSet<_>>();
    if evidence_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT id, evidence_seq, source_kind, source_event_id, fact_kind,
               natural_key, confirmation_bucket_key, subject_client_id,
               target_kind, target_id, source_status, completeness,
               subject_snapshot, payload, observed_at, state_started_at, causation_id,
               schedule_lineage, created_at
        FROM alert_policy_evidence
        WHERE id=ANY($1::uuid[])
        ORDER BY id
        "#,
    )
    .bind(evidence_ids.iter().copied().collect::<Vec<_>>())
    .fetch_all(&mut **tx)
    .await?;
    let evidences = rows
        .into_iter()
        .map(stored_evidence_from_row)
        .collect::<Result<Vec<_>>>()?;
    anyhow::ensure!(
        evidences.len() == evidence_ids.len(),
        "alert policy evidence missing"
    );
    Ok(evidences
        .into_iter()
        .map(|evidence| (evidence.id, evidence))
        .collect())
}

fn stored_evidence_from_row(row: PgRow) -> Result<StoredEvidence> {
    Ok(StoredEvidence {
        id: row.try_get("id")?,
        evidence_seq: row.try_get("evidence_seq")?,
        source_kind: row.try_get("source_kind")?,
        source_event_id: row.try_get("source_event_id")?,
        fact_kind: parse_rule_kind(&row.try_get::<String, _>("fact_kind")?)?,
        natural_key: row.try_get("natural_key")?,
        confirmation_bucket_key: row.try_get("confirmation_bucket_key")?,
        subject_client_id: row.try_get("subject_client_id")?,
        target_kind: row.try_get("target_kind")?,
        target_id: row.try_get("target_id")?,
        source_status: row.try_get("source_status")?,
        complete: row.try_get::<String, _>("completeness")? == "complete",
        subject_snapshot: row.try_get::<SqlJson<Value>, _>("subject_snapshot")?.0,
        payload: row.try_get::<SqlJson<Value>, _>("payload")?.0,
        observed_at: row.try_get("observed_at")?,
        accepted_at: row.try_get("created_at")?,
        state_started_at: row.try_get("state_started_at")?,
        causation_id: row.try_get("causation_id")?,
        schedule_lineage: row.try_get("schedule_lineage")?,
    })
}

fn evaluator_rule_from_row(row: PgRow) -> Result<EvaluatorRule> {
    Ok(EvaluatorRule {
        id: row.try_get("id")?,
        group_id: row.try_get("group_id")?,
        group_name: row.try_get("group_name")?,
        group_selector: row.try_get("group_selector")?,
        rule_version: row.try_get("rule_version")?,
        name: row.try_get("name")?,
        kind: parse_rule_kind(&row.try_get::<String, _>("rule_kind")?)?,
        evidence_source: row.try_get("evidence_source")?,
        correlation_mode: parse_correlation_mode(&row.try_get::<String, _>("correlation_mode")?)?,
        trigger_expression: row.try_get("trigger_condition_expression")?,
        trigger_meta: parse_meta(row.try_get("trigger_meta_condition")?)?,
        resolve_expression: row.try_get("resolve_condition_expression")?,
        resolve_meta: parse_meta(row.try_get("resolve_meta_condition")?)?,
        severity: row.try_get("severity")?,
        category: row.try_get("category")?,
        title_template: row.try_get("title_template")?,
        detail_template: row.try_get("detail_template")?,
        system_seed_key: row.try_get("system_seed_key")?,
        armed_after_evidence_seq: row.try_get("armed_after_evidence_seq")?,
        armed_at: row.try_get("armed_at")?,
    })
}

fn parse_meta(value: Option<SqlJson<Value>>) -> Result<Option<AlertPolicyMetaCondition>> {
    value
        .map(|value| serde_json::from_value(value.0).context("invalid policy meta condition"))
        .transpose()
}

fn parse_rule_kind(value: &str) -> Result<AlertPolicyRuleKind> {
    match value {
        "metric" => Ok(AlertPolicyRuleKind::Metric),
        "state" => Ok(AlertPolicyRuleKind::State),
        "occurrence" => Ok(AlertPolicyRuleKind::Occurrence),
        _ => anyhow::bail!("invalid policy rule kind"),
    }
}

fn parse_correlation_mode(value: &str) -> Result<AlertPolicyCorrelationMode> {
    match value {
        "natural_key" => Ok(AlertPolicyCorrelationMode::NaturalKey),
        "subject" => Ok(AlertPolicyCorrelationMode::Subject),
        "global" => Ok(AlertPolicyCorrelationMode::Global),
        _ => anyhow::bail!("invalid policy correlation mode"),
    }
}

fn policy_scope_truth(rule: &EvaluatorRule, evidence: &StoredEvidence) -> Result<ScopeTruth> {
    policy_scope_truth_inner(rule, evidence).map_err(|error| {
        deterministic_policy_evaluator_error("policy_scope_evaluation_failed", error)
    })
}

fn policy_scope_truth_inner(rule: &EvaluatorRule, evidence: &StoredEvidence) -> Result<ScopeTruth> {
    if evidence
        .subject_snapshot
        .get("status")
        .and_then(Value::as_str)
        == Some("suspended")
    {
        return Ok(ScopeTruth::OutOfScope);
    }
    if rule.group_selector.trim() == "*" {
        return Ok(ScopeTruth::InScope);
    }
    let Some(client_id) = evidence.subject_client_id.as_ref() else {
        return Ok(ScopeTruth::OutOfScope);
    };
    let snapshot = evidence
        .subject_snapshot
        .as_object()
        .context("policy evidence subject snapshot is invalid")?;
    if snapshot.get("scope_complete").and_then(Value::as_bool) != Some(true) {
        // Retained/migrated occurrence identity can be intentionally partial.
        // Missing selector facts are not proof that an active condition left
        // scope, so fail closed as Unknown rather than interpreting defaults.
        return Ok(ScopeTruth::Unknown);
    }
    let tags = snapshot
        .get("tags")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let vps = VpsMetadata {
        id: client_id.clone(),
        display_name: snapshot
            .get("display_name")
            .and_then(Value::as_str)
            .unwrap_or(client_id)
            .to_string(),
        status: snapshot
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        tags,
        registration_ip: snapshot
            .get("registration_ip")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        last_ip: snapshot
            .get("last_ip")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        last_seen_at: snapshot
            .get("last_seen_at")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        internal_build_number: snapshot
            .get("internal_build_number")
            .and_then(Value::as_u64),
        stale_since: snapshot
            .get("stale_since")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        stale_reason: snapshot
            .get("stale_reason")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        extra: snapshot.get("extra").cloned(),
    };
    let Some(rule_values) = snapshot.get("vps_rules").and_then(Value::as_object) else {
        return Ok(ScopeTruth::Unknown);
    };
    let mut vps_rules = VpsRuleContext::default();
    for (key, value) in rule_values {
        let Some(value_raw) = value.get("value_raw").and_then(Value::as_str) else {
            return Ok(ScopeTruth::Unknown);
        };
        let Some(value_json) = value.get("value_json") else {
            return Ok(ScopeTruth::Unknown);
        };
        vps_rules.insert(key.clone(), value_raw.to_string(), value_json.clone());
    }
    let expression = parse_expression(&rule.group_selector)
        .map_err(|error| anyhow::anyhow!("invalid persisted policy selector: {error}"))?
        .context("persisted policy selector is empty")?;
    let context = ExpressionContext::for_vps(vps).with_vps_rules(vps_rules);
    Ok(match expression_truth(&context, &expression) {
        ExpressionTruth::True => ScopeTruth::InScope,
        ExpressionTruth::False => ScopeTruth::OutOfScope,
        ExpressionTruth::Unknown => ScopeTruth::Unknown,
    })
}

fn expression_truth_for_evidence(
    rule: &EvaluatorRule,
    expression: &str,
    evidence: &StoredEvidence,
) -> Result<ExpressionTruth> {
    policy_expression_truth_for_preview(
        rule.kind,
        expression,
        &evidence.payload,
        &evidence.subject_snapshot,
        evidence.complete,
    )
    .map_err(|error| {
        deterministic_policy_evaluator_error("policy_expression_evaluation_failed", error)
    })
}

/// Shared save-preview/runtime truth boundary. Metric rules retain the typed
/// arithmetic evaluator; state rules use strict Kleene selector semantics over
/// the same evidence and subject JSON roots as live evaluation.
pub(crate) fn policy_expression_truth_for_preview(
    rule_kind: AlertPolicyRuleKind,
    expression: &str,
    payload: &Value,
    subject_snapshot: &Value,
    complete: bool,
) -> Result<ExpressionTruth> {
    if rule_kind == AlertPolicyRuleKind::Metric {
        return crate::repository_alert_policies::metric_policy_expression_truth(
            expression, payload, complete,
        );
    }
    if !complete {
        return Ok(ExpressionTruth::Unknown);
    }
    let expression = parse_expression(expression)
        .map_err(|error| anyhow::anyhow!("invalid persisted policy expression: {error}"))?
        .context("persisted policy expression is empty")?;
    let context = ExpressionContext::default()
        .with_json_root("evidence", payload.clone())
        .with_json_root("subject", subject_snapshot.clone());
    Ok(expression_truth(&context, &expression))
}

async fn lock_or_create_state_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
) -> Result<EvaluationState> {
    if let Some(state) = load_state_for_update_in_tx(tx, rule, evidence).await? {
        return Ok(state);
    }

    // State creation is exceptional: the common telemetry path already has
    // one durable row for each armed rule/bucket. Concurrent first evidence
    // may race this insert; ON CONFLICT establishes one owner and the second
    // locked read below observes it without changing evaluation order.
    sqlx::query(
        r#"
        INSERT INTO alert_policy_evaluation_states (
            policy_rule_id, rule_version, confirmation_bucket_key,
            occurrence_cohort_id, subject_client_id, truth_state,
            trigger_generation, last_evaluated_at
        ) VALUES ($1,$2,$3,$4,$5,'unknown',0,clock_timestamp())
        ON CONFLICT (policy_rule_id, rule_version, confirmation_bucket_key) DO NOTHING
        "#,
    )
    .bind(rule.id)
    .bind(rule.rule_version)
    .bind(&evidence.confirmation_bucket_key)
    .bind(
        (rule.kind == AlertPolicyRuleKind::Occurrence
            && matches!(
                rule.trigger_meta,
                Some(AlertPolicyMetaCondition::Count { .. })
            ))
        .then(Uuid::new_v4),
    )
    .bind(&evidence.subject_client_id)
    .execute(&mut **tx)
    .await?;
    load_state_for_update_in_tx(tx, rule, evidence)
        .await?
        .context("alert policy evaluation state missing after creation")
}

async fn load_state_for_update_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
) -> Result<Option<EvaluationState>> {
    let row = sqlx::query(
        r#"
        SELECT truth_state, last_evidence_seq, last_evidence_source_event_id,
               last_evidence_observed_at,
               trigger_confirmed_duration_secs, trigger_segment_started_at,
               resolve_confirmed_duration_secs, resolve_segment_started_at,
               state.trigger_generation, state.active_episode_id, state.occurrence_cohort_id,
               episode.triggered_at AS active_triggered_at
        FROM alert_policy_evaluation_states state
        LEFT JOIN alert_episodes episode ON episode.id=state.active_episode_id
        WHERE state.policy_rule_id=$1 AND state.rule_version=$2
          AND state.confirmation_bucket_key=$3
        FOR UPDATE OF state
        "#,
    )
    .bind(rule.id)
    .bind(rule.rule_version)
    .bind(&evidence.confirmation_bucket_key)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| {
        Ok(EvaluationState {
            truth_state: row.try_get("truth_state")?,
            last_evidence_seq: row.try_get("last_evidence_seq")?,
            last_evidence_source_event_id: row.try_get("last_evidence_source_event_id")?,
            last_evidence_observed_at: row.try_get("last_evidence_observed_at")?,
            trigger_confirmed_duration_secs: row.try_get("trigger_confirmed_duration_secs")?,
            trigger_segment_started_at: row.try_get("trigger_segment_started_at")?,
            resolve_confirmed_duration_secs: row.try_get("resolve_confirmed_duration_secs")?,
            resolve_segment_started_at: row.try_get("resolve_segment_started_at")?,
            trigger_generation: row.try_get("trigger_generation")?,
            active_episode_id: row.try_get("active_episode_id")?,
            active_triggered_at: row.try_get("active_triggered_at")?,
            occurrence_cohort_id: row.try_get("occurrence_cohort_id")?,
        })
    })
    .transpose()
}

async fn persist_state_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
    state: &EvaluationState,
    now: DateTime<Utc>,
) -> Result<()> {
    let next_transition_at = next_transition_at(rule, state, now);
    sqlx::query(
        r#"
        UPDATE alert_policy_evaluation_states
        SET occurrence_cohort_id=$4, subject_client_id=$5, truth_state=$6,
            last_evidence_id=$7, last_evidence_seq=$8,
            last_evidence_source_event_id=$9, last_evidence_observed_at=$10,
            trigger_confirmed_duration_secs=$11, trigger_segment_started_at=$12,
            resolve_confirmed_duration_secs=$13, resolve_segment_started_at=$14,
            trigger_generation=$15, active_episode_id=$16,
            next_transition_at=$18, last_evaluated_at=$17, updated_at=$17
        WHERE policy_rule_id=$1 AND rule_version=$2 AND confirmation_bucket_key=$3
        "#,
    )
    .bind(rule.id)
    .bind(rule.rule_version)
    .bind(&evidence.confirmation_bucket_key)
    .bind(state.occurrence_cohort_id)
    .bind(
        if rule.correlation_mode == AlertPolicyCorrelationMode::Global {
            None
        } else {
            evidence.subject_client_id.as_deref()
        },
    )
    .bind(&state.truth_state)
    .bind(evidence.id)
    .bind(state.last_evidence_seq)
    .bind(&state.last_evidence_source_event_id)
    .bind(state.last_evidence_observed_at)
    .bind(state.trigger_confirmed_duration_secs)
    .bind(state.trigger_segment_started_at)
    .bind(state.resolve_confirmed_duration_secs)
    .bind(state.resolve_segment_started_at)
    .bind(state.trigger_generation)
    .bind(state.active_episode_id)
    .bind(now)
    .bind(next_transition_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn next_transition_at(
    rule: &EvaluatorRule,
    state: &EvaluationState,
    _now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    if state.active_episode_id.is_some() {
        match rule.resolve_meta.as_ref() {
            Some(AlertPolicyMetaCondition::Sustained { seconds }) => {
                state.resolve_segment_started_at.map(|start| {
                    start
                        + Duration::seconds(
                            seconds.saturating_sub(state.resolve_confirmed_duration_secs),
                        )
                })
            }
            Some(AlertPolicyMetaCondition::ElapsedSinceTrigger { seconds }) => state
                .active_triggered_at
                .map(|triggered_at| triggered_at + Duration::seconds(*seconds)),
            _ => None,
        }
    } else {
        match rule.trigger_meta.as_ref() {
            Some(AlertPolicyMetaCondition::Sustained { seconds }) => {
                state.trigger_segment_started_at.map(|start| {
                    start
                        + Duration::seconds(
                            seconds.saturating_sub(state.trigger_confirmed_duration_secs),
                        )
                })
            }
            _ => None,
        }
    }
}

async fn load_active_episode_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Option<Uuid>,
) -> Result<Option<ActiveEpisode>> {
    let Some(id) = id else { return Ok(None) };
    let row = sqlx::query(
        r#"
        SELECT id, COALESCE(last_evidence_id,trigger_evidence_id) AS last_evidence_id,
               trigger_generation, lifecycle_state,
               schedule_lineage, triggered_at
        FROM alert_episodes
        WHERE id=$1 AND resolved_at IS NULL
        FOR UPDATE
        "#,
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| {
        Ok(ActiveEpisode {
            id: row.try_get("id")?,
            last_evidence_id: row.try_get("last_evidence_id")?,
            generation: row.try_get("trigger_generation")?,
            lifecycle_state: row.try_get("lifecycle_state")?,
            schedule_lineage: row.try_get("schedule_lineage")?,
            triggered_at: row.try_get("triggered_at")?,
        })
    })
    .transpose()
}

async fn receipt_exists_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &EvaluatorRule,
    evidence_seq: i64,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM alert_policy_evidence_receipts
            WHERE policy_rule_id=$1 AND rule_version=$2 AND evidence_seq=$3
        )
        "#,
    )
    .bind(rule.id)
    .bind(rule.rule_version)
    .bind(evidence_seq)
    .fetch_one(&mut **tx)
    .await?)
}

async fn evaluate_rule_to_receipt_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
    terminal_detail: Option<&str>,
) -> Result<PolicyRuleEvaluationDisposition> {
    sqlx::query("SAVEPOINT alert_policy_rule_evaluation")
        .execute(&mut **tx)
        .await?;
    match evaluate_rule_in_tx(tx, rule, evidence, false).await {
        Ok(result) => {
            sqlx::query("RELEASE SAVEPOINT alert_policy_rule_evaluation")
                .execute(&mut **tx)
                .await?;
            record_receipt_in_tx(tx, rule, evidence, result, terminal_detail).await?;
            Ok(PolicyRuleEvaluationDisposition::Terminal)
        }
        Err(error) => {
            sqlx::query("ROLLBACK TO SAVEPOINT alert_policy_rule_evaluation")
                .execute(&mut **tx)
                .await?;
            sqlx::query("RELEASE SAVEPOINT alert_policy_rule_evaluation")
                .execute(&mut **tx)
                .await?;
            if is_lineage_overflow_error(&error) {
                record_receipt_in_tx(
                    tx,
                    rule,
                    evidence,
                    "lineage_overflow",
                    Some("policy_schedule_lineage_overflow"),
                )
                .await?;
                audit_policy_evaluation_skip_in_tx(tx, rule, evidence, "lineage_overflow").await?;
                Ok(PolicyRuleEvaluationDisposition::Terminal)
            } else if let Some(deterministic) =
                error.downcast_ref::<DeterministicPolicyEvaluationError>()
            {
                record_receipt_in_tx(tx, rule, evidence, "error", Some(&deterministic.0)).await?;
                audit_policy_evaluation_skip_in_tx(tx, rule, evidence, &deterministic.0).await?;
                Ok(PolicyRuleEvaluationDisposition::Terminal)
            } else {
                Ok(PolicyRuleEvaluationDisposition::Retry(error))
            }
        }
    }
}

async fn record_receipt_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
    result: &str,
    detail: Option<&str>,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO alert_policy_evidence_receipts (
            policy_rule_id, rule_version, evidence_seq, evidence_id,
            natural_key, confirmation_bucket_key, result, detail
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
        ON CONFLICT (policy_rule_id, rule_version, evidence_seq) DO NOTHING
        "#,
    )
    .bind(rule.id)
    .bind(rule.rule_version)
    .bind(evidence.evidence_seq)
    .bind(evidence.id)
    .bind(&evidence.natural_key)
    .bind(effective_confirmation_bucket(rule, evidence)?)
    .bind(result)
    .bind(detail)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn is_lineage_overflow_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .to_string()
            .contains("policy_schedule_lineage_overflow")
    })
}

async fn audit_policy_evaluation_skip_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
    reason: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
        VALUES ($1, NULL, 'fleet.alert_policy_evidence_skipped', $2, NULL, $3)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(format!("fleet_alert_policy_rule:{}", rule.id))
    .bind(json!({
        "result": "skipped",
        "origin_kind": "control_plane",
        "component": "alert-policy-evaluator",
        "policy_rule_id": rule.id,
        "policy_rule_version": rule.rule_version,
        "evidence_id": evidence.id,
        "evidence_seq": evidence.evidence_seq,
        "reason": reason,
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_confirmation_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
    phase: GatePhase,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO alert_policy_confirmations (
            policy_rule_id, rule_version, confirmation_bucket_key,
            phase, evidence_id, accepted_at
        ) VALUES ($1,$2,$3,$4,$5,$6)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(rule.id)
    .bind(rule.rule_version)
    .bind(&evidence.confirmation_bucket_key)
    .bind(phase.storage())
    .bind(evidence.id)
    .bind(evidence.accepted_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Sustained truth and elapsed time live in the evaluation state. Keep only
/// the first source fact as durable/public provenance for the current phase,
/// while folding every later fact's schedule lineage into that one anchor.
/// The state-row lock held by the evaluator serializes this read/update pair.
async fn retain_sustained_confirmation_anchor_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
    phase: GatePhase,
) -> Result<()> {
    let existing = sqlx::query(
        r#"
        SELECT evidence_id, confirmation_lineage,
               confirmation_lineage_overflow
        FROM alert_policy_confirmations
        WHERE policy_rule_id=$1 AND rule_version=$2
          AND confirmation_bucket_key=$3 AND phase=$4
        ORDER BY accepted_at,evidence_id DESC
        LIMIT 1
        FOR UPDATE
        "#,
    )
    .bind(rule.id)
    .bind(rule.rule_version)
    .bind(&evidence.confirmation_bucket_key)
    .bind(phase.storage())
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(existing) = existing {
        let anchor_id: Uuid = existing.try_get("evidence_id")?;
        let mut lineage: Vec<Uuid> = existing.try_get("confirmation_lineage")?;
        let mut overflow: bool = existing.try_get("confirmation_lineage_overflow")?;
        if !overflow {
            lineage.extend(evidence.schedule_lineage.iter().copied());
            lineage.sort_unstable();
            lineage.dedup();
            if lineage.len() > MAX_LINEAGE {
                overflow = true;
                lineage.truncate(MAX_LINEAGE);
            }
        }
        sqlx::query(
            r#"
            UPDATE alert_policy_confirmations
            SET confirmation_lineage=$6,
                confirmation_lineage_overflow=$7
            WHERE policy_rule_id=$1 AND rule_version=$2
              AND confirmation_bucket_key=$3 AND phase=$4
              AND evidence_id=$5
            "#,
        )
        .bind(rule.id)
        .bind(rule.rule_version)
        .bind(&evidence.confirmation_bucket_key)
        .bind(phase.storage())
        .bind(anchor_id)
        .bind(&lineage)
        .bind(overflow)
        .execute(&mut **tx)
        .await?;
    } else {
        sqlx::query(
            r#"
            INSERT INTO alert_policy_confirmations (
                policy_rule_id, rule_version, confirmation_bucket_key,
                phase, evidence_id, accepted_at, confirmation_lineage,
                confirmation_lineage_overflow
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,FALSE)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(rule.id)
        .bind(rule.rule_version)
        .bind(&evidence.confirmation_bucket_key)
        .bind(phase.storage())
        .bind(evidence.id)
        .bind(evidence.accepted_at)
        .bind(&evidence.schedule_lineage)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn clear_confirmations_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
    phase: GatePhase,
) -> Result<()> {
    sqlx::query(
        r#"
        DELETE FROM alert_policy_confirmations
        WHERE policy_rule_id=$1 AND rule_version=$2
          AND confirmation_bucket_key=$3 AND phase=$4
        "#,
    )
    .bind(rule.id)
    .bind(rule.rule_version)
    .bind(&evidence.confirmation_bucket_key)
    .bind(phase.storage())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn prune_count_confirmations_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
    phase: GatePhase,
    meta: Option<&AlertPolicyMetaCondition>,
    now: DateTime<Utc>,
) -> Result<()> {
    let Some(AlertPolicyMetaCondition::Count { within_seconds, .. }) = meta else {
        return Ok(());
    };
    sqlx::query(
        r#"
        DELETE FROM alert_policy_confirmations
        WHERE policy_rule_id=$1 AND rule_version=$2
          AND confirmation_bucket_key=$3 AND phase=$4
          AND accepted_at < $5
        "#,
    )
    .bind(rule.id)
    .bind(rule.rule_version)
    .bind(&evidence.confirmation_bucket_key)
    .bind(phase.storage())
    .bind(now - Duration::seconds(*within_seconds))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn contributing_lineage_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
    phase: GatePhase,
    retained: &[Uuid],
) -> Result<Vec<Uuid>> {
    let rows = sqlx::query(
        r#"
        SELECT source.schedule_lineage, confirmation.confirmation_lineage,
               confirmation.confirmation_lineage_overflow
        FROM alert_policy_confirmations confirmation
        JOIN alert_policy_evidence source ON source.id=confirmation.evidence_id
        WHERE confirmation.policy_rule_id=$1
          AND confirmation.rule_version=$2
          AND confirmation.confirmation_bucket_key=$3
          AND confirmation.phase=$4
        ORDER BY confirmation.accepted_at, confirmation.evidence_id
        "#,
    )
    .bind(rule.id)
    .bind(rule.rule_version)
    .bind(&evidence.confirmation_bucket_key)
    .bind(phase.storage())
    .fetch_all(&mut **tx)
    .await?;
    let mut values = retained.to_vec();
    values.extend(evidence.schedule_lineage.iter().copied());
    for row in rows {
        if row.try_get::<bool, _>("confirmation_lineage_overflow")? {
            anyhow::bail!("policy_schedule_lineage_overflow");
        }
        values.extend(row.try_get::<Vec<Uuid>, _>("schedule_lineage")?);
        values.extend(row.try_get::<Vec<Uuid>, _>("confirmation_lineage")?);
    }
    canonical_lineage(values)
}

async fn episode_evidence_payload(
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
    context: &Value,
    phase: GatePhase,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<Value> {
    let confirmations = sqlx::query(
        r#"
        SELECT confirmation.evidence_id, confirmation.accepted_at,
               source.observed_at, source.subject_client_id
        FROM alert_policy_confirmations confirmation
        JOIN alert_policy_evidence source ON source.id=confirmation.evidence_id
        WHERE confirmation.policy_rule_id=$1 AND confirmation.rule_version=$2
          AND confirmation.confirmation_bucket_key=$3 AND confirmation.phase=$4
        ORDER BY confirmation.accepted_at, confirmation.evidence_id
        "#,
    )
    .bind(rule.id)
    .bind(rule.rule_version)
    .bind(&evidence.confirmation_bucket_key)
    .bind(phase.storage())
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .map(|row| -> Result<Value> {
        Ok(json!({
            "evidence_id": row.try_get::<Uuid, _>("evidence_id")?,
            "accepted_at": row.try_get::<DateTime<Utc>, _>("accepted_at")?.to_rfc3339(),
            "observed_at": row.try_get::<DateTime<Utc>, _>("observed_at")?.to_rfc3339(),
            "subject_client_id": row.try_get::<Option<String>, _>("subject_client_id")?,
        }))
    })
    .collect::<Result<Vec<_>>>()?;
    let confirmation_snapshot = json!({
        "source": evidence.payload,
        "subject": evidence.subject_snapshot,
        "source_evidence_id": evidence.id,
        "source_event_id": evidence.source_event_id,
        "source_observed_at": evidence.observed_at.to_rfc3339(),
        "source_completeness": if evidence.complete { "complete" } else { "unknown" },
        "confirmation_evidence": confirmations.clone(),
    });
    let snapshot_key = match phase {
        GatePhase::Trigger => "trigger_evidence_snapshot",
        GatePhase::Resolve => "resolution_evidence_snapshot",
    };
    let mut payload = json!({});
    let object = payload
        .as_object_mut()
        .context("policy episode evidence payload must be an object")?;
    object.insert("source".to_string(), evidence.payload.clone());
    object.insert("subject".to_string(), evidence.subject_snapshot.clone());
    object.insert("source_evidence_id".to_string(), json!(evidence.id));
    object.insert(
        "source_event_id".to_string(),
        json!(evidence.source_event_id),
    );
    object.insert(
        "source_observed_at".to_string(),
        json!(evidence.observed_at.to_rfc3339()),
    );
    object.insert(
        "source_completeness".to_string(),
        json!(if evidence.complete {
            "complete"
        } else {
            "unknown"
        }),
    );
    object.insert(
        "policy".to_string(),
        context.get("policy").cloned().unwrap_or(Value::Null),
    );
    object.insert(
        "policy_rule".to_string(),
        context.get("policy_rule").cloned().unwrap_or(Value::Null),
    );
    object.insert(
        "confirmation_evidence".to_string(),
        Value::Array(confirmations.clone()),
    );
    object.insert(snapshot_key.to_string(), confirmation_snapshot);
    Ok(payload)
}

fn lifecycle_template_context(rule: &EvaluatorRule, evidence: &StoredEvidence) -> Value {
    json!({
        "evidence": evidence.payload,
        "subject": evidence.subject_snapshot,
        "policy": {"id": rule.group_id, "name": rule.group_name},
        "policy_rule": {
            "id": rule.id,
            "name": rule.name,
            "rule_version": rule.rule_version,
            "rule_kind": rule_kind_storage(rule.kind),
            "system_seed_key": rule.system_seed_key,
            "trigger_condition_expression": rule.trigger_expression,
            "trigger_meta_condition": normalized_meta(rule.trigger_meta.as_ref()),
            "resolve_condition_expression": rule.resolve_expression,
            "resolve_meta_condition": normalized_meta(rule.resolve_meta.as_ref()),
        }
    })
}

fn render_policy_template(
    template: &str,
    context: &Value,
    max_bytes: usize,
) -> std::result::Result<String, DeterministicPolicyEvaluationError> {
    let invalid = |code: String| DeterministicPolicyEvaluationError(code);
    let mut rendered = String::with_capacity(template.len());
    let mut remaining = template;
    while let Some(open) = remaining.find('{') {
        rendered.push_str(&remaining[..open]);
        let after = &remaining[open + 1..];
        let close = after
            .find('}')
            .ok_or_else(|| invalid("policy_template_placeholder_unclosed".to_string()))?;
        let path = after[..close].trim();
        if path.is_empty() {
            return Err(invalid("policy_template_placeholder_empty".to_string()));
        }
        if path.contains(['[', ']', '(', ')', '|']) {
            return Err(invalid("policy_template_helper_unsupported".to_string()));
        }
        let value = strict_scalar_path(context, path)
            .ok_or_else(|| invalid(format!("policy_template_field_unavailable:{path}")))?;
        rendered.push_str(&value);
        remaining = &after[close + 1..];
    }
    if remaining.contains('}') {
        return Err(invalid(
            "policy_template_closing_brace_unmatched".to_string(),
        ));
    }
    rendered.push_str(remaining);
    let rendered = rendered.trim().to_string();
    if rendered.is_empty() {
        return Err(invalid("policy_template_rendered_empty".to_string()));
    }
    if rendered.len() > max_bytes {
        return Err(invalid("policy_template_rendered_too_long".to_string()));
    }
    if rendered.contains('\0') {
        return Err(invalid("policy_template_rendered_nul".to_string()));
    }
    Ok(rendered)
}

fn strict_scalar_path(context: &Value, path: &str) -> Option<String> {
    let mut current = context;
    for segment in path.split('.') {
        if segment.is_empty() {
            return None;
        }
        current = current.get(segment)?;
    }
    match current {
        Value::String(value) => Some(value.clone()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

fn normalized_meta(meta: Option<&AlertPolicyMetaCondition>) -> Value {
    match meta {
        None | Some(AlertPolicyMetaCondition::Immediate) => {
            json!({"kind":"immediate","window_seconds":0})
        }
        Some(AlertPolicyMetaCondition::Sustained { seconds }) => {
            json!({"kind":"sustained","window_seconds":seconds})
        }
        Some(AlertPolicyMetaCondition::Count {
            confirmations,
            within_seconds,
        }) => json!({
            "kind":"count", "confirmations":confirmations,
            "window_seconds":within_seconds
        }),
        Some(AlertPolicyMetaCondition::ElapsedSinceTrigger { seconds }) => {
            json!({"kind":"elapsed_since_trigger","window_seconds":seconds})
        }
    }
}

fn phase_duration_fields(
    state: &EvaluationState,
    phase: GatePhase,
) -> (i64, Option<DateTime<Utc>>) {
    match phase {
        GatePhase::Trigger => (
            state.trigger_confirmed_duration_secs,
            state.trigger_segment_started_at,
        ),
        GatePhase::Resolve => (
            state.resolve_confirmed_duration_secs,
            state.resolve_segment_started_at,
        ),
    }
}

fn set_phase_duration(state: &mut EvaluationState, phase: GatePhase, value: i64) {
    match phase {
        GatePhase::Trigger => state.trigger_confirmed_duration_secs = value,
        GatePhase::Resolve => state.resolve_confirmed_duration_secs = value,
    }
}

fn set_phase_segment(state: &mut EvaluationState, phase: GatePhase, value: Option<DateTime<Utc>>) {
    match phase {
        GatePhase::Trigger => state.trigger_segment_started_at = value,
        GatePhase::Resolve => state.resolve_segment_started_at = value,
    }
}

fn evidence_state_boundary(
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
    now: DateTime<Utc>,
) -> DateTime<Utc> {
    let source_boundary = evidence
        .state_started_at
        .unwrap_or(evidence.observed_at)
        .max(rule.armed_at);
    // Selector/scope revisions are new authoritative evaluations of an old
    // source state. Re-entry must begin a fresh dwell at the DB acceptance
    // boundary rather than inheriting the original source transition time.
    let source_boundary = if evidence.source_event_id.starts_with("scope:") {
        source_boundary.max(evidence.accepted_at)
    } else {
        source_boundary
    };
    source_boundary.min(now)
}

fn evidence_is_source_confirmation(evidence: &StoredEvidence) -> bool {
    !evidence.source_event_id.starts_with("scope:")
}

fn canonical_lineage(values: Vec<Uuid>) -> Result<Vec<Uuid>> {
    let values = values.into_iter().collect::<BTreeSet<_>>();
    anyhow::ensure!(
        values.len() <= MAX_LINEAGE,
        "policy_schedule_lineage_overflow"
    );
    Ok(values.into_iter().collect())
}

fn effective_confirmation_bucket(
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
) -> Result<String> {
    match rule.correlation_mode {
        AlertPolicyCorrelationMode::NaturalKey => Ok(format!("natural:{}", evidence.natural_key)),
        AlertPolicyCorrelationMode::Subject => evidence
            .subject_client_id
            .as_ref()
            .map(|client_id| format!("subject:{client_id}"))
            .context("policy subject correlation requires a subject"),
        AlertPolicyCorrelationMode::Global => Ok(format!("global:{}", evidence.source_kind)),
    }
}

fn validate_policy_evidence_fact(fact: &PolicyEvidenceFact) -> Result<()> {
    let expected = match fact.source_kind.as_str() {
        "telemetry.combined" => AlertPolicyRuleKind::Metric,
        "agent.status" | "agent.access" | "tunnel.adapter" | "tunnel.traffic" => {
            AlertPolicyRuleKind::State
        }
        "job.terminal" | "backup.failure" | "job.capability" => AlertPolicyRuleKind::Occurrence,
        _ => anyhow::bail!("unsupported policy evidence source"),
    };
    anyhow::ensure!(fact.fact_kind == expected, "policy evidence kind mismatch");
    anyhow::ensure!(
        !fact.source_event_id.trim().is_empty(),
        "policy evidence event id empty"
    );
    anyhow::ensure!(
        !fact.natural_key.trim().is_empty(),
        "policy evidence natural key empty"
    );
    anyhow::ensure!(
        !fact.confirmation_bucket_key.trim().is_empty(),
        "policy evidence confirmation bucket empty"
    );
    anyhow::ensure!(
        fact.subject_snapshot.is_object(),
        "policy evidence subject invalid"
    );
    anyhow::ensure!(fact.payload.is_object(), "policy evidence payload invalid");
    anyhow::ensure!(
        (fact.fact_kind == AlertPolicyRuleKind::Occurrence && fact.state_started_at.is_none())
            || (fact.fact_kind != AlertPolicyRuleKind::Occurrence
                && fact.state_started_at.is_some()),
        "policy evidence state boundary invalid"
    );
    Ok(())
}

fn rule_kind_storage(kind: AlertPolicyRuleKind) -> &'static str {
    match kind {
        AlertPolicyRuleKind::Metric => "metric",
        AlertPolicyRuleKind::State => "state",
        AlertPolicyRuleKind::Occurrence => "occurrence",
    }
}

async fn resolve_scope_exit_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
    state: &mut EvaluationState,
) -> Result<()> {
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **tx)
        .await?;
    if let Some(episode) = load_active_episode_in_tx(tx, state.active_episode_id).await? {
        let lineage = canonical_lineage(
            episode
                .schedule_lineage
                .iter()
                .copied()
                .chain(evidence.schedule_lineage.iter().copied())
                .collect(),
        )?;
        sqlx::query(
            r#"
            UPDATE alert_episodes
            SET lifecycle_state='resolved', resolved_at=$2,
                resolution_reason='policy_scope_exited', last_evidence_id=$3,
                schedule_lineage=$4, updated_at=$2
            WHERE id=$1 AND resolved_at IS NULL
            "#,
        )
        .bind(episode.id)
        .bind(now)
        .bind(evidence.id)
        .bind(&lineage)
        .execute(&mut **tx)
        .await?;
        let edge_episode = ActiveEpisode {
            schedule_lineage: lineage,
            ..episode
        };
        emit_lifecycle_edge_in_tx(tx, rule, evidence, &edge_episode, "alert.resolved", now).await?;
    }
    state.active_episode_id = None;
    state.active_triggered_at = None;
    state.truth_state = "not_matched".to_string();
    state.trigger_confirmed_duration_secs = 0;
    state.trigger_segment_started_at = None;
    state.resolve_confirmed_duration_secs = 0;
    state.resolve_segment_started_at = None;
    state.last_evidence_seq = Some(evidence.evidence_seq);
    state.last_evidence_source_event_id = Some(evidence.source_event_id.clone());
    state.last_evidence_observed_at = Some(evidence.observed_at);
    clear_confirmations_in_tx(tx, rule, evidence, GatePhase::Trigger).await?;
    clear_confirmations_in_tx(tx, rule, evidence, GatePhase::Resolve).await?;
    persist_state_in_tx(tx, rule, evidence, state, now).await
}

async fn resolve_source_scope_exit_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &EvaluatorRule,
    evidence: &StoredEvidence,
    state: &mut EvaluationState,
) -> Result<()> {
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **tx)
        .await?;
    if let Some(episode) = load_active_episode_in_tx(tx, state.active_episode_id).await? {
        let lineage = canonical_lineage(
            episode
                .schedule_lineage
                .iter()
                .copied()
                .chain(evidence.schedule_lineage.iter().copied())
                .collect(),
        )?;
        sqlx::query(
            r#"
            UPDATE alert_episodes
            SET lifecycle_state='resolved', resolved_at=$2,
                resolution_reason='source_scope_exited', last_evidence_id=$3,
                schedule_lineage=$4, updated_at=$2
            WHERE id=$1 AND resolved_at IS NULL
            "#,
        )
        .bind(episode.id)
        .bind(now)
        .bind(evidence.id)
        .bind(&lineage)
        .execute(&mut **tx)
        .await?;
        let edge_episode = ActiveEpisode {
            schedule_lineage: lineage,
            ..episode
        };
        emit_lifecycle_edge_in_tx(tx, rule, evidence, &edge_episode, "alert.resolved", now).await?;
    }
    state.active_episode_id = None;
    state.active_triggered_at = None;
    state.truth_state = "not_matched".to_string();
    state.trigger_confirmed_duration_secs = 0;
    state.trigger_segment_started_at = None;
    state.resolve_confirmed_duration_secs = 0;
    state.resolve_segment_started_at = None;
    state.last_evidence_seq = Some(evidence.evidence_seq);
    state.last_evidence_source_event_id = Some(evidence.source_event_id.clone());
    state.last_evidence_observed_at = Some(evidence.observed_at);
    clear_confirmations_in_tx(tx, rule, evidence, GatePhase::Trigger).await?;
    clear_confirmations_in_tx(tx, rule, evidence, GatePhase::Resolve).await?;
    persist_state_in_tx(tx, rule, evidence, state, now).await
}
