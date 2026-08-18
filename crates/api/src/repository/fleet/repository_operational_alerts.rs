use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{postgres::PgRow, Row};
use uuid::Uuid;
use vpsman_common::{
    alert_policy_state_source_event_id, payload_hash, tunnel_runtime_evidence_identity_hash,
    tunnel_topology_identity_hash, RuntimeTunnelManager,
};

use crate::{
    model::{
        AuditLogView, AuthContext, FleetAlertLifecycleView, FleetAlertQuery, FleetAlertView,
        OperationalAlertEpisodeRecord,
    },
    model_alert_notifications::FleetAlertNotificationMatchRule,
    model_alert_policies::AlertPolicyRuleKind,
    repository::Repository,
    repository_policy_lifecycle::{
        evaluate_due_policy_transitions, record_policy_evidence_in_tx,
        record_policy_source_scope_exits_in_tx, repair_missing_policy_evidence_receipts,
        repair_policy_scope_revision_evidence, resolve_policy_occurrence_episode_in_tx,
        PolicyEvidenceFact,
    },
    util::parse_timestamp_utc,
};

#[cfg(test)]
use crate::{
    model_webhook_rules::WebhookEventCandidate,
    repository_webhook_rules::webhook_event_row,
    util::{compare_timestamps_desc, timestamp_in_optional_bounds},
};

pub(crate) const OPERATIONAL_ALERT_SOURCE_LIMIT: usize = 201;
const OPERATIONAL_RECONCILE_LOCK: &str = "vpsman:operational-alert-reconcile";
const LEGACY_EVENT_SOURCE_HORIZON: usize = 200;
const CONDITION_REPAIR_CLIENT_BATCH: usize = 200;
const STARTUP_RECONCILE_MAX_BATCHES: usize = 100_000;

pub(crate) fn notification_rule_matches_alert(
    rules: &[FleetAlertNotificationMatchRule],
    severity: &str,
    category: &str,
    operator_state: &str,
    client_id: Option<&str>,
) -> bool {
    let severity_rank = match severity {
        "critical" => 0,
        "warning" => 1,
        "info" => 2,
        _ => 3,
    };
    rules.iter().any(|rule| {
        severity_rank <= rule.min_severity_rank
            && (rule.categories.is_empty() || rule.categories.iter().any(|value| value == category))
            && (rule.operator_states.is_empty()
                || rule
                    .operator_states
                    .iter()
                    .any(|value| value == operator_state))
            && rule.client_ids.as_ref().is_none_or(|client_ids| {
                client_id
                    .is_some_and(|client_id| client_ids.iter().any(|allowed| allowed == client_id))
            })
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProbeState {
    Confirmed,
    Healthy,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TunnelEvidenceMode {
    Exact,
    Unavailable,
    LegacyBootstrap,
}

#[derive(Clone, Debug)]
struct AlertSource {
    producer_kind: String,
    natural_key: String,
    severity: String,
    category: String,
    target_kind: String,
    target_id: String,
    client_id: Option<String>,
    title: String,
    detail: String,
    source_status: String,
    evidence: Value,
    observed_at: String,
}

#[derive(Clone, Debug)]
struct ConditionProbe {
    source: AlertSource,
    state: ProbeState,
    backfilled: bool,
}

#[derive(Clone, Debug)]
struct EventSource {
    source: AlertSource,
    backfilled: bool,
}

#[derive(Default)]
struct OperationalSnapshot {
    conditions: Vec<ConditionProbe>,
    events: Vec<EventSource>,
}

#[cfg(test)]
#[derive(Default)]
struct ReconcileResult {
    changed: Vec<OperationalAlertEpisodeRecord>,
    edges: Vec<(WebhookEventCandidate, DateTime<Utc>)>,
}

async fn next_postgres_condition_client_batch(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    cursor: Option<String>,
) -> Result<Vec<String>> {
    async fn load_after(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        cursor: Option<&str>,
    ) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            r#"
            WITH repair_clients AS (
                SELECT id AS client_id
                FROM clients
                WHERE hidden_at IS NULL
                UNION
                SELECT client_id
                FROM alert_episodes
                WHERE record_kind = 'condition'
                  AND resolved_at IS NULL
                  AND client_id IS NOT NULL
            )
            SELECT client_id
            FROM repair_clients
            WHERE ($1::text IS NULL OR client_id > $1)
            ORDER BY client_id
            LIMIT $2
            "#,
        )
        .bind(cursor)
        .bind((CONDITION_REPAIR_CLIENT_BATCH + 1) as i64)
        .fetch_all(&mut **tx)
        .await?)
    }

    let mut rows = load_after(tx, cursor.as_deref()).await?;
    if rows.is_empty() && cursor.is_some() {
        rows = load_after(tx, None).await?;
    }
    let has_more = rows.len() > CONDITION_REPAIR_CLIENT_BATCH;
    rows.truncate(CONDITION_REPAIR_CLIENT_BATCH);
    let next_cursor = if has_more { rows.last().cloned() } else { None };
    sqlx::query(
        r#"
        UPDATE operational_alert_lifecycle_meta
        SET condition_client_cursor = $1
        WHERE singleton
        "#,
    )
    .bind(next_cursor)
    .execute(&mut **tx)
    .await?;
    Ok(rows)
}

impl Repository {
    /// Reconciles every non-policy Fleet-alert producer into its durable episode owner.
    /// The first run after migration seeds existing source rows as Persisting without
    /// synthesizing historical Triggered edges.
    pub(crate) async fn reconcile_operational_alerts(&self) -> Result<()> {
        match self {
            // Policy-owned lifecycle requires durable evidence receipts,
            // timers, and independent outbox consumers. Memory remains only
            // non-lifecycle fixture storage and deliberately emits no alerts.
            Self::Memory(_) => Ok(()),
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
                    .bind(OPERATIONAL_RECONCILE_LOCK)
                    .execute(&mut *tx)
                    .await?;
                let meta = sqlx::query(
                    r#"
                    SELECT lifecycle.cutover_at,
                           NOT lifecycle.legacy_condition_bootstrap_completed
                               AS legacy_condition_bootstrapping,
                           NOT lifecycle.legacy_event_bootstrap_completed
                               AS legacy_event_bootstrapping,
                           operational.condition_client_cursor
                    FROM alert_policy_lifecycle_meta lifecycle
                    CROSS JOIN operational_alert_lifecycle_meta operational
                    WHERE lifecycle.singleton AND operational.singleton
                    FOR UPDATE OF operational
                    "#,
                )
                .fetch_one(&mut *tx)
                .await?;
                let event_source_cutoff_at: DateTime<Utc> = meta.try_get("cutover_at")?;
                let legacy_condition_bootstrapping: bool =
                    meta.try_get("legacy_condition_bootstrapping")?;
                let legacy_event_bootstrapping: bool =
                    meta.try_get("legacy_event_bootstrapping")?;
                let condition_client_ids = next_postgres_condition_client_batch(
                    &mut tx,
                    meta.try_get("condition_client_cursor")?,
                )
                .await?;
                let snapshot = load_postgres_snapshot(
                    &mut tx,
                    legacy_condition_bootstrapping,
                    legacy_event_bootstrapping,
                    event_source_cutoff_at,
                    Some(&condition_client_ids),
                )
                .await?;
                let present_identities = policy_condition_probe_identities(&snapshot.conditions)?;
                record_postgres_policy_condition_probes_in_tx(&mut tx, snapshot.conditions).await?;
                record_policy_source_scope_exits_in_tx(
                    &mut tx,
                    &[
                        "agent.status",
                        "agent.access",
                        "tunnel.adapter",
                        "tunnel.traffic",
                    ],
                    &condition_client_ids,
                    &present_identities,
                )
                .await?;
                record_postgres_policy_event_sources_in_tx(
                    &mut tx,
                    snapshot
                        .events
                        .into_iter()
                        .map(|event| {
                            let mut source = event.source;
                            if event.backfilled {
                                let public_id = legacy_public_id(&source);
                                source
                                    .evidence
                                    .as_object_mut()
                                    .expect("operational evidence is always an object")
                                    .insert("backfilled".to_string(), json!(true));
                                source
                                    .evidence
                                    .as_object_mut()
                                    .expect("operational evidence is always an object")
                                    .insert("legacy_public_id".to_string(), json!(public_id));
                            }
                            source
                        })
                        .collect(),
                )
                .await?;
                if legacy_event_bootstrapping {
                    sqlx::query(
                        r#"
                        UPDATE alert_policy_lifecycle_meta
                        SET legacy_event_bootstrap_completed=TRUE
                        WHERE singleton AND NOT legacy_event_bootstrap_completed
                        "#,
                    )
                    .execute(&mut *tx)
                    .await?;
                }
                if legacy_condition_bootstrapping {
                    sqlx::query(
                        r#"
                        UPDATE alert_policy_lifecycle_meta
                        SET legacy_condition_bootstrap_completed=TRUE
                        WHERE singleton
                          AND NOT legacy_condition_bootstrap_completed
                          AND (SELECT condition_client_cursor IS NULL
                               FROM operational_alert_lifecycle_meta
                               WHERE singleton)
                        "#,
                    )
                    .execute(&mut *tx)
                    .await?;
                }
                sqlx::query(
                    r#"
                    UPDATE operational_alert_lifecycle_meta operational
                    SET backfill_completed=TRUE,
                        completed_at=COALESCE(completed_at,clock_timestamp())
                    FROM alert_policy_lifecycle_meta lifecycle
                    WHERE operational.singleton AND lifecycle.singleton
                      AND lifecycle.legacy_condition_bootstrap_completed
                      AND lifecycle.legacy_event_bootstrap_completed
                      AND NOT operational.backfill_completed
                    "#,
                )
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                self.repair_combined_telemetry_policy_evidence(200).await?;
                repair_policy_scope_revision_evidence(pool, 200).await?;
                repair_missing_policy_evidence_receipts(pool, 500).await?;
                evaluate_due_policy_transitions(pool, 200).await?;
                Ok(())
            }
        }
    }

    /// Drains the maintenance-gated policy cutover before the API listener is
    /// exposed. Periodic reconciliation remains bounded to one fleet slice;
    /// startup instead resumes the durable client cursor through a complete
    /// pass, then drains every bounded repair queue to a stable high-watermark.
    pub(crate) async fn reconcile_operational_alerts_startup(&self) -> Result<()> {
        let Self::Postgres(pool) = self else {
            return Ok(());
        };
        for batch in 0..STARTUP_RECONCILE_MAX_BATCHES {
            self.reconcile_operational_alerts().await?;
            let cursor: Option<String> = sqlx::query_scalar(
                "SELECT condition_client_cursor FROM operational_alert_lifecycle_meta WHERE singleton",
            )
            .fetch_one(pool)
            .await?;
            let event_sources_remaining = postgres_policy_event_source_backlog_exists(pool).await?;
            if cursor.is_none() && !event_sources_remaining {
                break;
            }
            anyhow::ensure!(
                batch + 1 < STARTUP_RECONCILE_MAX_BATCHES,
                "policy startup client reconciliation did not converge"
            );
        }

        for batch in 0..STARTUP_RECONCILE_MAX_BATCHES {
            let telemetry = self.repair_combined_telemetry_policy_evidence(200).await?;
            let scope = repair_policy_scope_revision_evidence(pool, 200).await?;
            let receipts = repair_missing_policy_evidence_receipts(pool, 500).await?;
            evaluate_due_policy_transitions(pool, 200).await?;
            let event_sources_remaining = postgres_policy_event_source_backlog_exists(pool).await?;
            if event_sources_remaining {
                self.reconcile_operational_alerts().await?;
            }
            let due_remaining: bool = sqlx::query_scalar(
                r#"
                SELECT EXISTS (
                    SELECT 1
                    FROM alert_policy_evaluation_states state
                    JOIN policy_rules rule
                      ON rule.id=state.policy_rule_id
                     AND rule.rule_version=state.rule_version
                    JOIN policy_groups group_row ON group_row.id=rule.group_id
                    WHERE state.next_transition_at IS NOT NULL
                      AND state.next_transition_at <= clock_timestamp()
                      AND rule.enabled AND group_row.enabled
                )
                "#,
            )
            .fetch_one(pool)
            .await?;
            if telemetry == 0
                && scope == 0
                && receipts == 0
                && !due_remaining
                && !event_sources_remaining
            {
                sqlx::query(
                    r#"
                    UPDATE alert_policy_lifecycle_meta
                    SET startup_reconciled_at=COALESCE(startup_reconciled_at,clock_timestamp())
                    WHERE singleton
                    "#,
                )
                .execute(pool)
                .await?;
                return Ok(());
            }
            anyhow::ensure!(
                batch + 1 < STARTUP_RECONCILE_MAX_BATCHES,
                "policy startup repair reconciliation did not converge"
            );
        }
        anyhow::bail!("policy startup reconciliation did not converge")
    }

    pub(crate) async fn list_operational_alert_episodes(
        &self,
        query: &FleetAlertQuery,
        include_resolved: bool,
        confirmed_active_only: bool,
        record_kind: Option<&str>,
        allowed_client_ids: Option<&HashSet<String>>,
        include_global: bool,
        start_unix: Option<u64>,
        end_unix: Option<u64>,
        limit: usize,
        notification_rules: Option<&[FleetAlertNotificationMatchRule]>,
    ) -> Result<Vec<OperationalAlertEpisodeRecord>> {
        if matches!(self, Self::Memory(_)) {
            return Ok(Vec::new());
        }
        let limit = limit.clamp(1, OPERATIONAL_ALERT_SOURCE_LIMIT);
        match self {
            #[cfg(test)]
            Self::Memory(memory) => {
                let states = memory.fleet_alert_states.read().await;
                let now = crate::unix_now() as i64;
                let mut rows = memory
                    .operational_alert_episodes
                    .read()
                    .await
                    .iter()
                    .filter(|row| {
                        include_resolved
                            || (row.resolved_at.is_none() && row.last_confirmed_at.is_some())
                    })
                    .filter(|row| record_kind.is_none_or(|kind| row.record_kind == kind))
                    .filter(|row| {
                        !confirmed_active_only
                            || (row.last_confirmed_at.is_some()
                                && matches!(
                                    row.lifecycle_state.as_str(),
                                    "triggered" | "persisting"
                                ))
                    })
                    .filter(|row| {
                        allowed_client_ids.is_none_or(|allowed| {
                            row.client_id
                                .as_ref()
                                .is_some_and(|client_id| allowed.contains(client_id))
                                || (row.client_id.is_none() && include_global)
                        })
                    })
                    .filter(|row| {
                        timestamp_in_optional_bounds(&row.triggered_at, start_unix, end_unix)
                    })
                    .filter(|row| {
                        query
                            .client_id
                            .as_deref()
                            .is_none_or(|client_id| row.client_id.as_deref() == Some(client_id))
                    })
                    .filter(|row| {
                        query
                            .severity
                            .as_deref()
                            .is_none_or(|severity| row.severity == severity)
                    })
                    .filter(|row| {
                        query
                            .category
                            .as_deref()
                            .is_none_or(|category| row.category == category)
                    })
                    .filter(|row| {
                        let state = effective_memory_operator_state(
                            states.iter().find(|state| state.alert_id == row.public_id),
                            now,
                        );
                        (query.include_muted.unwrap_or(false) || state != "muted")
                            && query
                                .operator_state
                                .as_deref()
                                .is_none_or(|expected| state == expected)
                            && notification_rules.is_none_or(|rules| {
                                notification_rule_matches_alert(
                                    rules,
                                    &row.severity,
                                    &row.category,
                                    state,
                                    row.client_id.as_deref(),
                                )
                            })
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                sort_operational_episodes(&mut rows, include_resolved);
                rows.truncate(limit);
                Ok(rows)
            }
            #[cfg(not(test))]
            Self::Memory(_) => unreachable!("Memory lifecycle listing is disabled"),
            Self::Postgres(pool) => {
                let sql = format!(
                    r#"
                    {}
                    LEFT JOIN fleet_alert_states triage
                      ON triage.alert_id = e.public_id
                    WHERE ($1::boolean OR (
                        resolved_at IS NULL AND last_confirmed_at IS NOT NULL
                    ))
                      AND ($2::text IS NULL OR client_id = $2)
                      AND ($3::text IS NULL OR severity = $3)
                      AND ($4::text IS NULL OR category = $4)
                      AND (NOT $5::boolean OR (
                        lifecycle_state IN ('triggered', 'persisting')
                        AND last_confirmed_at IS NOT NULL
                      ))
                      AND (
                        $6::text[] IS NULL
                        OR client_id = ANY($6)
                        OR (client_id IS NULL AND $7::boolean)
                      )
                      AND (
                        $8::text IS NULL
                        OR CASE
                          WHEN triage.state = 'muted'
                           AND triage.muted_until_unix IS NOT NULL
                           AND triage.muted_until_unix <= $10
                          THEN 'open'
                          ELSE COALESCE(triage.state, 'open')
                        END = $8
                      )
                      AND (
                        $9::boolean
                        OR CASE
                          WHEN triage.state = 'muted'
                           AND triage.muted_until_unix IS NOT NULL
                           AND triage.muted_until_unix <= $10
                          THEN 'open'
                          ELSE COALESCE(triage.state, 'open')
                        END <> 'muted'
                      )
                      AND ($11::double precision IS NULL OR triggered_at >= to_timestamp($11))
                      AND ($12::double precision IS NULL OR triggered_at <= to_timestamp($12))
                      AND ($14::text IS NULL OR record_kind = $14)
                      AND (
                        $13::jsonb IS NULL
                        OR EXISTS (
                          SELECT 1
                          FROM jsonb_array_elements($13::jsonb) rule
                          WHERE CASE e.severity
                                  WHEN 'critical' THEN 0
                                  WHEN 'warning' THEN 1
                                  WHEN 'info' THEN 2
                                  ELSE 3
                                END <= (rule->>'min_severity_rank')::integer
                            AND (
                              jsonb_array_length(rule->'categories') = 0
                              OR rule->'categories' ? e.category
                            )
                            AND (
                              jsonb_array_length(rule->'operator_states') = 0
                              OR rule->'operator_states' ? CASE
                                WHEN triage.state = 'muted'
                                 AND triage.muted_until_unix IS NOT NULL
                                 AND triage.muted_until_unix <= $10
                                THEN 'open'
                                ELSE COALESCE(triage.state, 'open')
                              END
                            )
                            AND (
                              rule->'client_ids' = 'null'::jsonb
                              OR (
                                e.client_id IS NOT NULL
                                AND rule->'client_ids' ? e.client_id
                              )
                            )
                        )
                      )
                    ORDER BY
                      CASE WHEN $1::boolean THEN 0
                           WHEN record_kind = 'condition' THEN 0 ELSE 1 END,
                      CASE WHEN $1::boolean THEN 0
                           WHEN lifecycle_state IN ('triggered', 'persisting') THEN 0
                           WHEN lifecycle_state = 'unknown' THEN 1 ELSE 2 END,
                      CASE WHEN $1::boolean THEN 0
                           WHEN severity = 'critical' THEN 0
                           WHEN severity = 'warning' THEN 1 ELSE 2 END,
                      triggered_at DESC,
                      id DESC
                    LIMIT $15
                    "#,
                    operational_episode_select_sql("")
                );
                let rows = sqlx::query(&sql)
                    .bind(include_resolved)
                    .bind(query.client_id.as_deref())
                    .bind(query.severity.as_deref())
                    .bind(query.category.as_deref())
                    .bind(confirmed_active_only)
                    .bind(allowed_client_ids.map(|ids| ids.iter().cloned().collect::<Vec<_>>()))
                    .bind(include_global)
                    .bind(query.operator_state.as_deref())
                    .bind(query.include_muted.unwrap_or(false))
                    .bind(crate::unix_now() as i64)
                    .bind(start_unix.map(|value| value as f64))
                    .bind(end_unix.map(|value| value as f64))
                    .bind(notification_rules.map(serde_json::to_value).transpose()?)
                    .bind(record_kind)
                    .bind(limit as i64)
                    .fetch_all(pool)
                    .await?;
                rows.into_iter().map(operational_episode_from_row).collect()
            }
        }
    }

    pub(crate) async fn list_unresolved_operational_alert_events_page(
        &self,
        query: &FleetAlertQuery,
        cursor: Option<(DateTime<Utc>, Uuid)>,
        limit: usize,
    ) -> Result<Vec<OperationalAlertEpisodeRecord>> {
        if matches!(self, Self::Memory(_)) {
            return Ok(Vec::new());
        }
        let limit = limit.clamp(1, OPERATIONAL_ALERT_SOURCE_LIMIT);
        match self {
            #[cfg(test)]
            Self::Memory(memory) => {
                let states = memory.fleet_alert_states.read().await;
                let now = crate::unix_now() as i64;
                let mut rows = memory
                    .operational_alert_episodes
                    .read()
                    .await
                    .iter()
                    .filter(|row| row.record_kind == "event" && row.resolved_at.is_none())
                    .filter(|row| {
                        query
                            .client_id
                            .as_deref()
                            .is_none_or(|client_id| row.client_id.as_deref() == Some(client_id))
                    })
                    .filter(|row| {
                        query
                            .severity
                            .as_deref()
                            .is_none_or(|severity| row.severity == severity)
                    })
                    .filter(|row| {
                        query
                            .category
                            .as_deref()
                            .is_none_or(|category| row.category == category)
                    })
                    .filter(|row| {
                        let state = effective_memory_operator_state(
                            states.iter().find(|state| state.alert_id == row.public_id),
                            now,
                        );
                        (query.include_muted.unwrap_or(false) || state != "muted")
                            && query
                                .operator_state
                                .as_deref()
                                .is_none_or(|expected| state == expected)
                    })
                    .filter(|row| {
                        cursor.is_none_or(|(cursor_time, cursor_id)| {
                            parse_timestamp_utc(&row.triggered_at).is_some_and(|row_time| {
                                row_time < cursor_time
                                    || (row_time == cursor_time && row.id < cursor_id)
                            })
                        })
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                rows.sort_by(|left, right| {
                    compare_timestamps_desc(&left.triggered_at, &right.triggered_at)
                        .then_with(|| right.id.cmp(&left.id))
                });
                rows.truncate(limit);
                Ok(rows)
            }
            #[cfg(not(test))]
            Self::Memory(_) => unreachable!("Memory lifecycle listing is disabled"),
            Self::Postgres(pool) => {
                let sql = format!(
                    r#"
                    {}
                    LEFT JOIN fleet_alert_states triage
                      ON triage.alert_id = e.public_id
                    WHERE e.record_kind = 'event'
                      AND e.resolved_at IS NULL
                      AND ($1::text IS NULL OR e.client_id = $1)
                      AND ($2::text IS NULL OR e.severity = $2)
                      AND ($3::text IS NULL OR e.category = $3)
                      AND (
                        $4::text IS NULL
                        OR CASE
                          WHEN triage.state = 'muted'
                           AND triage.muted_until_unix IS NOT NULL
                           AND triage.muted_until_unix <= $6
                          THEN 'open'
                          ELSE COALESCE(triage.state, 'open')
                        END = $4
                      )
                      AND (
                        $5::boolean
                        OR CASE
                          WHEN triage.state = 'muted'
                           AND triage.muted_until_unix IS NOT NULL
                           AND triage.muted_until_unix <= $6
                          THEN 'open'
                          ELSE COALESCE(triage.state, 'open')
                        END <> 'muted'
                      )
                      AND (
                        $7::timestamptz IS NULL
                        OR e.triggered_at < $7::timestamptz
                        OR (e.triggered_at = $7::timestamptz AND e.id < $8::uuid)
                      )
                    ORDER BY e.triggered_at DESC, e.id DESC
                    LIMIT $9
                    "#,
                    operational_episode_select_sql("")
                );
                let rows = sqlx::query(&sql)
                    .bind(query.client_id.as_deref())
                    .bind(query.severity.as_deref())
                    .bind(query.category.as_deref())
                    .bind(query.operator_state.as_deref())
                    .bind(query.include_muted.unwrap_or(false))
                    .bind(crate::unix_now() as i64)
                    .bind(cursor.map(|(time, _)| time))
                    .bind(cursor.map(|(_, id)| id))
                    .bind(limit as i64)
                    .fetch_all(pool)
                    .await?;
                rows.into_iter().map(operational_episode_from_row).collect()
            }
        }
    }

    pub(crate) async fn resolve_operational_alert_event(
        &self,
        public_id: &str,
        reason: &str,
        operator: &AuthContext,
    ) -> Result<OperationalAlertEpisodeRecord> {
        anyhow::ensure!(
            !matches!(self, Self::Memory(_)),
            "policy_owned_alert_lifecycle_requires_postgres"
        );
        let public_id = public_id.trim();
        let reason = reason.trim();
        anyhow::ensure!(!public_id.is_empty(), "fleet_alert_id_required");
        anyhow::ensure!(
            reason.len() <= 1024 && !reason.is_empty(),
            "fleet_alert_resolution_reason_invalid"
        );
        match self {
            #[cfg(test)]
            Self::Memory(memory) => {
                let _mutation = memory.operational_alert_mutation.lock().await;
                let mut episodes = memory.operational_alert_episodes.write().await;
                let episode = episodes
                    .iter_mut()
                    .find(|episode| episode.public_id == public_id)
                    .context("fleet_alert_not_found")?;
                anyhow::ensure!(
                    episode.record_kind == "event",
                    "fleet_alert_condition_not_operator_resolvable"
                );
                if episode.resolved_at.is_some() {
                    return Ok(episode.clone());
                }
                let resolved_at = causal_now(episode);
                resolve_episode(
                    episode,
                    &resolved_at,
                    "operator_resolved",
                    Some(reason.to_string()),
                    Some(operator.operator.id),
                );
                let stored = episode.clone();
                drop(episodes);
                append_memory_webhook_edges(
                    memory,
                    vec![(
                        operational_lifecycle_event(&stored, false),
                        parse_episode_time(&resolved_at)?,
                    )],
                )
                .await?;
                memory
                    .audits
                    .write()
                    .await
                    .push(operational_resolution_audit(&stored, operator, reason));
                Ok(stored)
            }
            #[cfg(not(test))]
            Self::Memory(_) => unreachable!("Memory lifecycle resolution is disabled"),
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let transitioned = resolve_policy_occurrence_episode_in_tx(
                    &mut tx,
                    public_id,
                    reason,
                    operator.operator.id,
                )
                .await?;
                let sql = format!(
                    "{} WHERE public_id = $1 FOR UPDATE",
                    operational_episode_select_sql("")
                );
                let row = sqlx::query(&sql)
                    .bind(public_id)
                    .fetch_optional(&mut *tx)
                    .await?
                    .context("fleet_alert_not_found")?;
                let episode = operational_episode_from_row(row)?;
                if transitioned {
                    insert_operational_resolution_audit_in_tx(&mut tx, &episode, operator, reason)
                        .await?;
                }
                tx.commit().await?;
                Ok(episode)
            }
        }
    }

    pub(crate) async fn reconcile_memory_agent_alert_transition(
        &self,
        client_id: &str,
        to_status: &str,
        observed_at: &str,
    ) -> Result<()> {
        let _ = (self, client_id, to_status, observed_at);
        Ok(())
    }

    pub(crate) async fn reconcile_memory_client_status_alert_transition(
        &self,
        client_id: &str,
        to_status: &str,
        observed_at: &str,
    ) -> Result<()> {
        let _ = (self, client_id, to_status, observed_at);
        Ok(())
    }

    pub(crate) async fn reconcile_memory_tunnel_alerts_for_clients(
        &self,
        client_ids: &[String],
    ) -> Result<()> {
        let _ = (self, client_ids);
        Ok(())
    }

    pub(crate) async fn mark_memory_tunnel_alerts_unknown_for_clients(
        &self,
        client_ids: &[String],
        status_boundary_at: &str,
    ) -> Result<()> {
        // Alert lifecycle ownership is PostgreSQL-only. Memory remains useful
        // for non-lifecycle repository fixtures, but gateway/session changes
        // must never revive the retired in-process episode/webhook owner.
        let _ = (self, client_ids, status_boundary_at);
        Ok(())
    }

    pub(crate) async fn reconcile_memory_job_event_sources(&self, job_id: Uuid) -> Result<()> {
        let _ = (self, job_id);
        Ok(())
    }

    pub(crate) async fn reconcile_memory_backup_event_source(
        &self,
        backup_id: Uuid,
        terminal_at: &str,
    ) -> Result<()> {
        let _ = (self, backup_id, terminal_at);
        Ok(())
    }
}

async fn postgres_policy_event_source_backlog_exists(pool: &sqlx::PgPool) -> Result<bool> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM jobs job
            CROSS JOIN alert_policy_lifecycle_meta meta
            WHERE meta.singleton
              AND job.status IN (
                    'partial_success', 'canceled', 'rejected', 'failed',
                    'agent_timeout', 'control_timeout'
                  )
              AND job.alert_terminal_at >= meta.cutover_at
              AND NOT EXISTS (
                    SELECT 1 FROM alert_policy_evidence evidence
                    WHERE evidence.source_kind='job.terminal'
                      AND evidence.source_event_id=job.id::text
                  )
            UNION ALL
            SELECT 1
            FROM backup_requests request
            CROSS JOIN alert_policy_lifecycle_meta meta
            WHERE meta.singleton
              AND request.status='execution_failed'
              AND request.terminal_at >= meta.cutover_at
              AND NOT EXISTS (
                    SELECT 1 FROM alert_policy_evidence evidence
                    WHERE evidence.source_kind='backup.failure'
                      AND evidence.source_event_id=request.id::text
                  )
            UNION ALL
            SELECT 1
            FROM job_targets target
            JOIN jobs job ON job.id=target.job_id
            CROSS JOIN alert_policy_lifecycle_meta meta
            WHERE meta.singleton
              AND target.status='skipped'
              AND target.capability_degraded_reason IS NOT NULL
              AND target.capability_degraded_hint IS NOT NULL
              AND target.capability_alert_at >= meta.cutover_at
              AND NOT EXISTS (
                    SELECT 1 FROM alert_policy_evidence evidence
                    WHERE evidence.source_kind='job.capability'
                      AND evidence.source_event_id=
                          target.job_id::text || ':' || target.client_id
                  )
            LIMIT 1
        )
        "#,
    )
    .fetch_one(pool)
    .await?)
}

pub(crate) async fn reconcile_postgres_agent_alert_transition_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    to_status: &str,
) -> Result<()> {
    reconcile_postgres_agent_alert_transition_at_in_tx(tx, client_id, to_status).await
}

async fn reconcile_postgres_agent_alert_transition_at_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    to_status: &str,
) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(OPERATIONAL_RECONCILE_LOCK)
        .execute(&mut **tx)
        .await?;
    let identity = sqlx::query(
        r#"
        SELECT c.display_name, c.status, c.capabilities,
               c.operational_alert_status_at AS status_boundary_at,
               COALESCE(
                   (SELECT jsonb_agg(t.name ORDER BY t.display_order, t.name)
                    FROM client_tags ct JOIN tags t ON t.id = ct.tag_id
                    WHERE ct.client_id = c.id),
                   '[]'::jsonb
               ) AS tags
        FROM clients c
        WHERE c.id = $1 AND c.hidden_at IS NULL AND $2 <> 'deleted'
        "#,
    )
    .bind(client_id)
    .bind(to_status)
    .fetch_optional(&mut **tx)
    .await?;
    let probes = if let Some(row) = identity {
        let display_name: String = row.try_get("display_name")?;
        let current_status: String = row.try_get("status")?;
        if current_status != to_status {
            return Ok(());
        }
        let observed_at: DateTime<Utc> = row.try_get("status_boundary_at")?;
        let tags: Value = row.try_get("tags")?;
        let tags = serde_json::from_value::<Vec<String>>(tags).unwrap_or_default();
        let capabilities: Value = row.try_get("capabilities")?;
        agent_probes(
            client_id,
            &display_name,
            &current_status,
            &tags,
            &observed_at.to_rfc3339(),
            json!({"capability_privilege_mode": capabilities.get("privilege_mode")}),
            false,
        )
    } else {
        Vec::new()
    };
    let present_identities = policy_condition_probe_identities(&probes)?;
    record_postgres_policy_condition_probes_in_tx(tx, probes).await?;
    record_policy_source_scope_exits_in_tx(
        tx,
        &["agent.status", "agent.access"],
        &[client_id.to_string()],
        &present_identities,
    )
    .await?;
    Ok(())
}

pub(crate) async fn reconcile_postgres_job_event_sources_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job_id: Uuid,
) -> Result<()> {
    let Some(job) = sqlx::query(
        r#"
        SELECT id, command_type, status, target_count, causation_id, schedule_lineage,
               alert_terminal_at::text AS alert_terminal_at
        FROM jobs
        WHERE id = $1
        "#,
    )
    .bind(job_id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(());
    };
    let command_type: String = job.try_get("command_type")?;
    let status: String = job.try_get("status")?;
    let causation_id: Option<Uuid> = job.try_get("causation_id")?;
    let schedule_lineage: Vec<Uuid> = job.try_get("schedule_lineage")?;
    let job_alert_terminal_at: Option<String> = job.try_get("alert_terminal_at")?;
    let mut sources = Vec::new();
    if let Some(job_alert_terminal_at) = job_alert_terminal_at.filter(|_| {
        matches!(
            status.as_str(),
            "partial_success"
                | "canceled"
                | "rejected"
                | "failed"
                | "agent_timeout"
                | "control_timeout"
        )
    }) {
        let severity = if status == "partial_success" {
            "warning"
        } else {
            "critical"
        };
        let category = if command_type.contains("backup") || command_type.contains("restore") {
            "backup"
        } else if command_type.contains("agent_update") {
            "agent_update"
        } else {
            "job"
        };
        sources.push(AlertSource {
            producer_kind: "job".to_string(),
            natural_key: job_id.to_string(),
            severity: severity.to_string(),
            category: category.to_string(),
            target_kind: "job".to_string(),
            target_id: job_id.to_string(),
            client_id: None,
            title: "Job requires operator attention".to_string(),
            detail: format!("{command_type} job {status}"),
            source_status: status,
            evidence: json!({
                "job_id": job_id,
                "command_type": command_type,
                "target_count": job.try_get::<i32, _>("target_count")?,
                "causation_id": causation_id,
                "schedule_lineage": schedule_lineage,
                "retained_identity": true,
            }),
            observed_at: job_alert_terminal_at,
        });
    }
    let rows = sqlx::query(
        r#"
        SELECT target.client_id, target.status, target.message, target.exit_code,
               target.started_at::text AS started_at,
               target.completed_at::text AS completed_at,
               target.capability_alert_at::text AS capability_alert_at,
               target.capability_degraded_reason,
               target.capability_degraded_hint,
               client.display_name,
               COALESCE(
                   (SELECT jsonb_agg(tag.name ORDER BY tag.display_order, tag.name)
                    FROM client_tags client_tag
                    JOIN tags tag ON tag.id = client_tag.tag_id
                    WHERE client_tag.client_id = target.client_id),
                   '[]'::jsonb
               ) AS tags
        FROM job_targets target
        LEFT JOIN clients client ON client.id = target.client_id
        WHERE target.job_id = $1
          AND target.status = 'skipped'
          AND target.capability_degraded_reason IS NOT NULL
          AND target.capability_degraded_hint IS NOT NULL
        ORDER BY target.client_id
        "#,
    )
    .bind(job_id)
    .fetch_all(&mut **tx)
    .await?;
    for row in rows {
        let client_id: String = row.try_get("client_id")?;
        let display_name = row
            .try_get::<Option<String>, _>("display_name")?
            .unwrap_or_else(|| client_id.clone());
        let tags = serde_json::from_value::<Vec<String>>(row.try_get("tags")?).unwrap_or_default();
        let reason: String = row.try_get("capability_degraded_reason")?;
        let hint: String = row.try_get("capability_degraded_hint")?;
        let target_completed_at: Option<String> = row.try_get("completed_at")?;
        let started_at: Option<String> = row.try_get("started_at")?;
        let capability_alert_at: String = row.try_get("capability_alert_at")?;
        sources.push(AlertSource {
            producer_kind: "capability_degraded".to_string(),
            natural_key: format!("{job_id}:{client_id}"),
            severity: "warning".to_string(),
            category: "capability_degraded".to_string(),
            target_kind: "job_target".to_string(),
            target_id: format!("{job_id}:{client_id}"),
            client_id: Some(client_id.clone()),
            title: "Operation skipped because the agent lacks a required capability".to_string(),
            detail: hint.clone(),
            source_status: reason.clone(),
            evidence: merge_json(
                source_identity_evidence(&client_id, Some(&display_name), &tags),
                json!({
                    "job_id": job_id,
                    "command_type": command_type,
                    "target_status": row.try_get::<String, _>("status")?,
                    "target_message": row.try_get::<Option<String>, _>("message")?,
                    "reason": reason,
                    "hint": hint,
                    "exit_code": row.try_get::<Option<i32>, _>("exit_code")?,
                    "started_at": started_at,
                    "completed_at": target_completed_at,
                    "causation_id": causation_id,
                    "schedule_lineage": schedule_lineage,
                    "retained_identity": true,
                }),
            ),
            observed_at: capability_alert_at,
        });
    }
    reconcile_postgres_event_sources_in_tx(tx, sources).await
}

pub(crate) async fn reconcile_postgres_backup_event_source_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    backup_id: Uuid,
) -> Result<()> {
    let Some(row) = sqlx::query(
        r#"
        SELECT request.id, request.client_id, request.paths, request.include_config,
               request.artifact_id, request.created_at::text AS created_at,
               request.terminal_at::text AS terminal_at,
               request.causation_id, request.schedule_lineage,
               client.display_name,
               COALESCE(
                   (SELECT jsonb_agg(tag.name ORDER BY tag.display_order, tag.name)
                    FROM client_tags client_tag
                    JOIN tags tag ON tag.id = client_tag.tag_id
                    WHERE client_tag.client_id = request.client_id),
                   '[]'::jsonb
               ) AS tags
        FROM backup_requests request
        LEFT JOIN clients client ON client.id = request.client_id
        WHERE request.id = $1
          AND request.status = 'execution_failed'
          AND request.terminal_at IS NOT NULL
        "#,
    )
    .bind(backup_id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(());
    };
    let client_id: String = row.try_get("client_id")?;
    let display_name = row
        .try_get::<Option<String>, _>("display_name")?
        .unwrap_or_else(|| client_id.clone());
    let tags = serde_json::from_value::<Vec<String>>(row.try_get("tags")?).unwrap_or_default();
    let source = AlertSource {
        producer_kind: "backup_request".to_string(),
        natural_key: backup_id.to_string(),
        severity: "critical".to_string(),
        category: "backup".to_string(),
        target_kind: "backup_request".to_string(),
        target_id: backup_id.to_string(),
        client_id: Some(client_id.clone()),
        title: "Backup request failed".to_string(),
        detail: format!("backup request {backup_id} is execution_failed"),
        source_status: "execution_failed".to_string(),
        evidence: merge_json(
            source_identity_evidence(&client_id, Some(&display_name), &tags),
            json!({
                "paths": row.try_get::<Vec<String>, _>("paths")?,
                "include_config": row.try_get::<bool, _>("include_config")?,
                "artifact_id": row.try_get::<Option<Uuid>, _>("artifact_id")?,
                "request_created_at": row.try_get::<String, _>("created_at")?,
                "causation_id": row.try_get::<Option<Uuid>, _>("causation_id")?,
                "schedule_lineage": row.try_get::<Vec<Uuid>, _>("schedule_lineage")?,
                "retained_identity": true,
            }),
        ),
        observed_at: row.try_get("terminal_at")?,
    };
    reconcile_postgres_event_sources_in_tx(tx, vec![source]).await
}

pub(crate) async fn reconcile_postgres_tunnel_alerts_for_clients_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_ids: &[String],
) -> Result<()> {
    reconcile_postgres_tunnel_alerts_for_clients_with_evidence_in_tx(
        tx,
        client_ids,
        TunnelEvidenceMode::Exact,
        None,
    )
    .await
}

pub(crate) async fn mark_postgres_tunnel_alerts_unknown_for_clients_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_ids: &[String],
) -> Result<()> {
    let rows = sqlx::query(
        r#"
        SELECT id, operational_alert_tunnel_boundary_at::text AS boundary_at
        FROM clients
        WHERE id = ANY($1::text[])
        "#,
    )
    .bind(client_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut status_boundaries = HashMap::new();
    for row in rows {
        let client_id: String = row.try_get("id")?;
        let boundary_at: Option<String> = row.try_get("boundary_at")?;
        status_boundaries.insert(
            client_id.clone(),
            boundary_at.with_context(|| {
                format!("operational tunnel boundary missing for client {client_id}")
            })?,
        );
    }
    let requested = client_ids.iter().collect::<HashSet<_>>();
    anyhow::ensure!(
        status_boundaries.len() == requested.len(),
        "operational tunnel boundary client missing"
    );
    reconcile_postgres_tunnel_alerts_for_clients_with_evidence_in_tx(
        tx,
        client_ids,
        TunnelEvidenceMode::Unavailable,
        Some(&status_boundaries),
    )
    .await
}

async fn reconcile_postgres_tunnel_alerts_for_clients_with_evidence_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_ids: &[String],
    evidence_mode: TunnelEvidenceMode,
    status_boundaries: Option<&HashMap<String, String>>,
) -> Result<()> {
    if client_ids.is_empty() {
        return Ok(());
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(OPERATIONAL_RECONCILE_LOCK)
        .execute(&mut **tx)
        .await?;
    let mut probes =
        load_postgres_tunnel_probes_for_clients_in_tx(tx, client_ids, evidence_mode).await?;
    if let Some(status_boundaries) = status_boundaries {
        for probe in &mut probes {
            let client_id = probe
                .source
                .client_id
                .as_deref()
                .context("operational tunnel probe client missing")?;
            let status_boundary_at = status_boundaries
                .get(client_id)
                .with_context(|| format!("operational tunnel boundary missing for {client_id}"))?;
            probe.source.observed_at = status_boundary_at.clone();
            if let Some(evidence) = probe.source.evidence.as_object_mut() {
                evidence.insert("status_boundary_at".to_string(), json!(status_boundary_at));
            }
        }
    }
    let present_identities = policy_condition_probe_identities(&probes)?;
    record_postgres_policy_condition_probes_in_tx(tx, probes).await?;
    record_policy_source_scope_exits_in_tx(
        tx,
        &["tunnel.adapter", "tunnel.traffic"],
        client_ids,
        &present_identities,
    )
    .await?;
    Ok(())
}

pub(crate) fn operational_episode_to_fleet_alert(
    episode: &OperationalAlertEpisodeRecord,
) -> FleetAlertView {
    FleetAlertView {
        id: episode.public_id.clone(),
        record_kind: episode.record_kind.clone(),
        lifecycle: FleetAlertLifecycleView {
            state: episode.lifecycle_state.clone(),
            trigger_generation: episode.trigger_generation,
            triggered_at: episode.triggered_at.clone(),
            last_confirmed_at: episode.last_confirmed_at.clone(),
            resolved_at: episode.resolved_at.clone(),
            resolution_reason: episode.resolution_reason.clone(),
            resolution_note: episode.resolution_note.clone(),
            resolution_actor_id: episode.resolution_actor_id,
        },
        severity: episode.severity.clone(),
        category: episode.category.clone(),
        target_kind: episode.target_kind.clone(),
        target_id: episode.target_id.clone(),
        client_id: episode.client_id.clone(),
        title: episode.title.clone(),
        detail: episode.detail.clone(),
        status: episode.source_status.clone(),
        evidence: episode.evidence.clone(),
        observed_at: episode
            .last_confirmed_at
            .clone()
            .unwrap_or_else(|| episode.triggered_at.clone()),
        operator_state: "open".to_string(),
        muted_until_unix: None,
        escalation_level: 0,
        state_revision: 0,
        state_reason: None,
        state_actor_id: None,
        state_updated_at: None,
    }
}

#[cfg(test)]
fn reconcile_snapshot(
    episodes: &mut Vec<OperationalAlertEpisodeRecord>,
    snapshot: OperationalSnapshot,
    _bootstrapping: bool,
) -> Result<ReconcileResult> {
    let mut result = ReconcileResult::default();
    let existing_triggered_event_ids = episodes
        .iter()
        .filter(|episode| {
            episode.record_kind == "event"
                && episode.resolved_at.is_none()
                && episode.lifecycle_state == "triggered"
        })
        .map(|episode| episode.id)
        .collect::<HashSet<_>>();
    let condition_keys = snapshot
        .conditions
        .iter()
        .map(|probe| {
            (
                probe.source.producer_kind.clone(),
                probe.source.natural_key.clone(),
            )
        })
        .collect::<HashSet<_>>();

    for probe in snapshot.conditions {
        reconcile_condition_probe(episodes, probe, &mut result)?;
    }

    let now = now_string();
    for episode in episodes.iter_mut().filter(|episode| {
        episode.record_kind == "condition"
            && episode.resolved_at.is_none()
            && !condition_keys
                .contains(&(episode.producer_kind.clone(), episode.natural_key.clone()))
    }) {
        let resolved_at = causal_resolution_time(episode, &now);
        resolve_episode(episode, &resolved_at, "source_scope_exited", None, None);
        result.changed.push(episode.clone());
        result.edges.push((
            operational_lifecycle_event(episode, false),
            parse_episode_time(&resolved_at)?,
        ));
    }

    reconcile_new_event_sources(episodes, snapshot.events, &mut result)?;
    for episode in episodes.iter_mut().filter(|episode| {
        existing_triggered_event_ids.contains(&episode.id)
            && episode.record_kind == "event"
            && episode.resolved_at.is_none()
            && episode.lifecycle_state == "triggered"
    }) {
        episode.lifecycle_state = "persisting".to_string();
        episode.updated_at = now_string();
        if !result
            .changed
            .iter()
            .any(|changed| changed.id == episode.id)
        {
            result.changed.push(episode.clone());
        }
    }
    Ok(result)
}

#[cfg(test)]
fn reconcile_new_event_sources(
    episodes: &mut Vec<OperationalAlertEpisodeRecord>,
    events: Vec<EventSource>,
    result: &mut ReconcileResult,
) -> Result<()> {
    for event in events {
        let mut source = event.source;
        source.observed_at = canonical_source_time(&source.observed_at)?;
        if episodes.iter().any(|episode| {
            episode.producer_kind == source.producer_kind
                && episode.natural_key == source.natural_key
        }) {
            continue;
        }
        let episode = new_episode(&source, 1, event.backfilled);
        if !event.backfilled {
            result.edges.push((
                operational_lifecycle_event(&episode, true),
                parse_episode_time(&episode.triggered_at)?,
            ));
        }
        result.changed.push(episode.clone());
        episodes.push(episode);
    }
    Ok(())
}

async fn reconcile_postgres_event_sources_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    sources: Vec<AlertSource>,
) -> Result<()> {
    record_postgres_policy_event_sources_in_tx(tx, sources).await
}

#[cfg(test)]
fn reconcile_condition_probe(
    episodes: &mut Vec<OperationalAlertEpisodeRecord>,
    mut probe: ConditionProbe,
    result: &mut ReconcileResult,
) -> Result<()> {
    probe.source.observed_at = canonical_source_time(&probe.source.observed_at)?;
    let current_index = episodes.iter().position(|episode| {
        episode.producer_kind == probe.source.producer_kind
            && episode.natural_key == probe.source.natural_key
            && episode.resolved_at.is_none()
    });
    let boundary_decision = current_index.and_then(|index| {
        let episode = &episodes[index];
        (episode.lifecycle_state == "unknown"
            && matches!(
                episode.producer_kind.as_str(),
                "tunnel_adapter" | "tunnel_traffic"
            )
            && matches!(probe.state, ProbeState::Confirmed | ProbeState::Healthy))
        .then(|| tunnel_evidence_boundary_decision(&probe, episode))
    });
    if let Some(boundary_decision) =
        boundary_decision.filter(|decision| *decision != TunnelEvidenceBoundaryDecision::Current)
    {
        probe.state = ProbeState::Unknown;
        probe.source.title = match probe.source.producer_kind.as_str() {
            "tunnel_adapter" => "Tunnel adapter status is unavailable",
            _ => "Tunnel traffic status is unavailable",
        }
        .to_string();
        let (source_status, detail, evidence_status, boundary) = match boundary_decision {
            TunnelEvidenceBoundaryDecision::Stale(boundary) => (
                "tunnel_evidence_precedes_status_boundary",
                "Tunnel evidence was accepted before the latest agent status boundary",
                "stale_before_status_boundary",
                Some(boundary),
            ),
            TunnelEvidenceBoundaryDecision::Unverifiable => (
                "tunnel_evidence_boundary_unverifiable",
                "Tunnel evidence cannot be compared with the latest agent status boundary",
                "status_boundary_unverifiable",
                None,
            ),
            TunnelEvidenceBoundaryDecision::Current => unreachable!("filtered current evidence"),
        };
        probe.source.source_status = source_status.to_string();
        probe.source.detail = detail.to_string();
        if let Some(evidence) = probe.source.evidence.as_object_mut() {
            evidence.insert("evidence_status".to_string(), json!(evidence_status));
            if let Some(boundary) = boundary {
                evidence.insert("status_boundary_at".to_string(), json!(boundary));
            }
        }
    }
    match (probe.state, current_index) {
        (ProbeState::Confirmed, Some(index)) => {
            let episode = &mut episodes[index];
            let before = episode.clone();
            refresh_episode_from_source(episode, &probe.source);
            episode.lifecycle_state = "persisting".to_string();
            episode.last_confirmed_at = Some(max_time_string(
                episode.last_confirmed_at.as_deref(),
                &probe.source.observed_at,
            ));
            if *episode != before {
                episode.updated_at = now_string();
                result.changed.push(episode.clone());
            }
        }
        (ProbeState::Confirmed, None) => {
            let generation = next_generation(episodes, &probe.source);
            let episode = new_episode(&probe.source, generation, probe.backfilled);
            if !probe.backfilled {
                result.edges.push((
                    operational_lifecycle_event(&episode, true),
                    parse_episode_time(&episode.triggered_at)?,
                ));
            }
            result.changed.push(episode.clone());
            episodes.push(episode);
        }
        (ProbeState::Healthy, Some(index)) => {
            let episode = &mut episodes[index];
            let resolved_at = causal_resolution_time(episode, &probe.source.observed_at);
            record_resolution_evidence(episode, &probe.source);
            let resolution_reason = if probe.source.producer_kind == "agent_status"
                && probe.source.source_status == "revoked"
            {
                "source_scope_exited"
            } else {
                "condition_recovered"
            };
            resolve_episode(episode, &resolved_at, resolution_reason, None, None);
            result.changed.push(episode.clone());
            result.edges.push((
                operational_lifecycle_event(episode, false),
                parse_episode_time(&resolved_at)?,
            ));
        }
        (ProbeState::Unknown, Some(index)) => {
            let episode = &mut episodes[index];
            let before = episode.clone();
            let retain_legacy_presentation = episode
                .evidence
                .get("retain_unknown_backfill")
                .and_then(Value::as_bool)
                == Some(true);
            let mut evidence = episode.evidence.clone();
            if let (Some(target), Some(source)) =
                (evidence.as_object_mut(), probe.source.evidence.as_object())
            {
                if retain_legacy_presentation {
                    for key in ["status_boundary_at", "runtime_boundary_at"] {
                        if let Some(value) = source.get(key).filter(|value| !value.is_null()) {
                            target.insert(key.to_string(), value.clone());
                        }
                    }
                    target.insert("retain_unknown_backfill".to_string(), json!(true));
                } else {
                    target.extend(source.clone());
                }
            }
            if !retain_legacy_presentation {
                refresh_episode_from_source(episode, &probe.source);
            }
            episode.lifecycle_state = "unknown".to_string();
            episode.evidence = evidence;
            if *episode != before {
                episode.updated_at = now_string();
                result.changed.push(episode.clone());
            }
        }
        (ProbeState::Unknown, None)
            if probe.backfilled
                && probe
                    .source
                    .evidence
                    .get("retain_unknown_backfill")
                    .and_then(Value::as_bool)
                    == Some(true) =>
        {
            let generation = next_generation(episodes, &probe.source);
            let mut episode = new_episode(&probe.source, generation, true);
            episode.lifecycle_state = "unknown".to_string();
            result.changed.push(episode.clone());
            episodes.push(episode);
        }
        (ProbeState::Healthy | ProbeState::Unknown, None) => {}
    }
    Ok(())
}

#[cfg(test)]
fn record_resolution_evidence(episode: &mut OperationalAlertEpisodeRecord, source: &AlertSource) {
    episode.source_status = source.source_status.clone();
    if let Some(evidence) = episode.evidence.as_object_mut() {
        evidence.insert(
            "resolution_evidence".to_string(),
            json!({
                "observed_at": &source.observed_at,
                "status": &source.source_status,
                "evidence": &source.evidence,
            }),
        );
    }
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
enum TunnelEvidenceBoundaryDecision {
    Current,
    Stale(String),
    Unverifiable,
}

#[cfg(test)]
fn tunnel_evidence_boundary_decision(
    probe: &ConditionProbe,
    episode: &OperationalAlertEpisodeRecord,
) -> TunnelEvidenceBoundaryDecision {
    let boundary_value = episode.evidence.get("status_boundary_at");
    if boundary_value.is_none_or(Value::is_null) {
        return TunnelEvidenceBoundaryDecision::Current;
    }
    let Some((boundary_text, boundary)) = boundary_value
        .and_then(Value::as_str)
        .and_then(|value| parse_timestamp_utc(value).map(|parsed| (value, parsed)))
    else {
        return TunnelEvidenceBoundaryDecision::Unverifiable;
    };
    let Some(accepted_at) = probe
        .source
        .evidence
        .get("telemetry_accepted_at")
        .and_then(Value::as_str)
        .and_then(parse_timestamp_utc)
    else {
        return TunnelEvidenceBoundaryDecision::Unverifiable;
    };
    if accepted_at > boundary {
        TunnelEvidenceBoundaryDecision::Current
    } else {
        TunnelEvidenceBoundaryDecision::Stale(boundary_text.to_string())
    }
}

#[cfg(test)]
fn new_episode(
    source: &AlertSource,
    generation: i64,
    backfilled: bool,
) -> OperationalAlertEpisodeRecord {
    let id = Uuid::new_v4();
    let now = now_string();
    OperationalAlertEpisodeRecord {
        id,
        public_id: if backfilled {
            legacy_public_id(source)
        } else {
            format!("operational-alert:{id}")
        },
        producer_kind: source.producer_kind.clone(),
        natural_key: source.natural_key.clone(),
        record_kind: if matches!(
            source.producer_kind.as_str(),
            "job" | "backup_request" | "capability_degraded"
        ) {
            "event"
        } else {
            "condition"
        }
        .to_string(),
        trigger_generation: generation,
        trigger_severity: source.severity.clone(),
        trigger_category: source.category.clone(),
        severity: source.severity.clone(),
        category: source.category.clone(),
        target_kind: source.target_kind.clone(),
        target_id: source.target_id.clone(),
        client_id: source.client_id.clone(),
        title: source.title.clone(),
        detail: source.detail.clone(),
        source_status: source.source_status.clone(),
        evidence: source.evidence.clone(),
        lifecycle_state: if backfilled {
            "persisting"
        } else {
            "triggered"
        }
        .to_string(),
        triggered_at: source.observed_at.clone(),
        last_confirmed_at: Some(source.observed_at.clone()),
        resolved_at: None,
        resolution_reason: None,
        resolution_note: None,
        resolution_actor_id: None,
        backfilled,
        created_at: now.clone(),
        updated_at: now,
    }
}

#[cfg(test)]
fn refresh_episode_from_source(episode: &mut OperationalAlertEpisodeRecord, source: &AlertSource) {
    episode.severity = source.severity.clone();
    episode.category = source.category.clone();
    episode.target_kind = source.target_kind.clone();
    episode.target_id = source.target_id.clone();
    episode.client_id = source.client_id.clone();
    episode.title = source.title.clone();
    episode.detail = source.detail.clone();
    episode.source_status = source.source_status.clone();
    episode.evidence = source.evidence.clone();
}

#[cfg(test)]
fn resolve_episode(
    episode: &mut OperationalAlertEpisodeRecord,
    resolved_at: &str,
    resolution_reason: &str,
    resolution_note: Option<String>,
    resolution_actor_id: Option<Uuid>,
) {
    episode.lifecycle_state = "resolved".to_string();
    episode.resolved_at = Some(resolved_at.to_string());
    episode.resolution_reason = Some(resolution_reason.to_string());
    episode.resolution_note = resolution_note;
    episode.resolution_actor_id = resolution_actor_id;
    episode.updated_at = resolved_at.to_string();
}

#[cfg(test)]
fn next_generation(episodes: &[OperationalAlertEpisodeRecord], source: &AlertSource) -> i64 {
    episodes
        .iter()
        .filter(|episode| {
            episode.producer_kind == source.producer_kind
                && episode.natural_key == source.natural_key
        })
        .map(|episode| episode.trigger_generation)
        .max()
        .unwrap_or(0)
        + 1
}

fn legacy_public_id(source: &AlertSource) -> String {
    let legacy_status = if source.producer_kind == "tunnel_traffic" {
        "tunnel_traffic_degraded"
    } else {
        source.source_status.as_str()
    };
    let fingerprint = json!({
        "severity": source.severity,
        "category": source.category,
        "target_kind": source.target_kind,
        "target_id": source.target_id,
        "title": source.title,
        "status": legacy_status,
    });
    let hash = payload_hash(fingerprint.to_string().as_bytes());
    format!("{}:{}:{}", source.category, source.target_kind, &hash[..16])
}

#[cfg(test)]
fn operational_lifecycle_event(
    episode: &OperationalAlertEpisodeRecord,
    triggered: bool,
) -> WebhookEventCandidate {
    let state = if triggered { "triggered" } else { "resolved" };
    let kind = format!("alert.{state}");
    let predicates = vec![
        kind.clone(),
        format!("alert.category:{}", episode.trigger_category),
        format!("alert.severity:{}", episode.trigger_severity),
    ];
    WebhookEventCandidate {
        kind: kind.clone(),
        event_id: format!("fleet-alert:{}:{state}", episode.id),
        event_predicates: predicates,
        subject_client_ids: episode.client_id.iter().cloned().collect(),
        payload: json!({
            "event": {
                "kind": kind,
                "id": format!("fleet-alert:{}:{state}", episode.id),
                "occurred_at": if triggered { &episode.triggered_at } else { episode.resolved_at.as_ref().unwrap_or(&episode.updated_at) },
            },
            "alert": {
                "id": &episode.public_id,
                "episode_id": episode.id,
                "record_kind": &episode.record_kind,
                "producer_kind": &episode.producer_kind,
                "trigger_generation": episode.trigger_generation,
                "lifecycle_state": state,
                "severity": &episode.trigger_severity,
                "category": &episode.trigger_category,
                "current_severity": &episode.severity,
                "current_category": &episode.category,
                "target_kind": &episode.target_kind,
                "target_id": &episode.target_id,
                "client_id": &episode.client_id,
                "title": &episode.title,
                "detail": &episode.detail,
                "source_status": &episode.source_status,
                "status": &episode.source_status,
                "triggered_at": &episode.triggered_at,
                "last_confirmed_at": &episode.last_confirmed_at,
                "resolved_at": &episode.resolved_at,
                "resolution_reason": &episode.resolution_reason,
                "resolution_note": &episode.resolution_note,
                "resolution_actor_id": &episode.resolution_actor_id,
                "evidence": &episode.evidence,
            }
        }),
        actor_id: episode.resolution_actor_id,
    }
}

#[cfg(test)]
async fn append_memory_webhook_edges(
    memory: &crate::repository::MemoryState,
    edges: Vec<(WebhookEventCandidate, DateTime<Utc>)>,
) -> Result<()> {
    if edges.is_empty() {
        return Ok(());
    }
    let mut stored = memory.webhook_events.write().await;
    for (candidate, occurred_at) in edges {
        let row = webhook_event_row(candidate, occurred_at)?;
        if !stored
            .iter()
            .any(|existing| existing.kind == row.kind && existing.event_id == row.event_id)
        {
            stored.push(row);
        }
    }
    Ok(())
}

fn operational_resolution_audit(
    episode: &OperationalAlertEpisodeRecord,
    operator: &AuthContext,
    reason: &str,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: "fleet_alert.lifecycle_resolved".to_string(),
        target: format!("fleet_alert:{}", episode.public_id),
        command_hash: None,
        metadata: json!({
            "episode_id": episode.id,
            "public_id": &episode.public_id,
            "producer_kind": &episode.producer_kind,
            "trigger_generation": episode.trigger_generation,
            "resolution_reason": "operator_resolved",
            "resolution_note": reason,
            "operator_id": operator.operator.id,
            "operator_username": &operator.operator.username,
            "operator_role": &operator.operator.role,
            "operator_session_id": operator.audit_session_id(),
            "origin_kind": "operator_request",
            "component": "fleet-alert-lifecycle-controller",
        }),
        created_at: episode.updated_at.clone(),
    }
}

async fn insert_operational_resolution_audit_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    episode: &OperationalAlertEpisodeRecord,
    operator: &AuthContext,
    reason: &str,
) -> Result<()> {
    let audit = operational_resolution_audit(episode, operator, reason);
    sqlx::query(
        r#"
        INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7::timestamptz)
        "#,
    )
    .bind(audit.id)
    .bind(audit.actor_id)
    .bind(&audit.action)
    .bind(&audit.target)
    .bind(&audit.command_hash)
    .bind(&audit.metadata)
    .bind(&audit.created_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn operational_episode_select_sql(suffix: &str) -> String {
    format!(
        r#"
        SELECT
            e.id, e.public_id, e.producer_kind, e.natural_key, e.record_kind,
            e.trigger_generation, e.trigger_severity, e.trigger_category,
            e.severity, e.category, e.target_kind, e.target_id,
            e.client_id, e.title, e.detail, e.source_status, e.evidence,
            e.lifecycle_state, e.triggered_at::text AS triggered_at,
            e.last_confirmed_at::text AS last_confirmed_at,
            e.resolved_at::text AS resolved_at, e.resolution_reason, e.resolution_note,
            e.resolution_actor_id, e.backfilled, e.created_at::text AS created_at,
            e.updated_at::text AS updated_at
        FROM alert_episodes e
        {suffix}
        "#
    )
}

fn operational_episode_from_row(row: PgRow) -> Result<OperationalAlertEpisodeRecord> {
    Ok(OperationalAlertEpisodeRecord {
        id: row.try_get("id")?,
        public_id: row.try_get("public_id")?,
        producer_kind: row.try_get("producer_kind")?,
        natural_key: row.try_get("natural_key")?,
        record_kind: row.try_get("record_kind")?,
        trigger_generation: row.try_get("trigger_generation")?,
        trigger_severity: row.try_get("trigger_severity")?,
        trigger_category: row.try_get("trigger_category")?,
        severity: row.try_get("severity")?,
        category: row.try_get("category")?,
        target_kind: row.try_get("target_kind")?,
        target_id: row.try_get("target_id")?,
        client_id: row.try_get("client_id")?,
        title: row.try_get("title")?,
        detail: row.try_get("detail")?,
        source_status: row.try_get("source_status")?,
        evidence: row.try_get("evidence")?,
        lifecycle_state: row.try_get("lifecycle_state")?,
        triggered_at: row.try_get("triggered_at")?,
        last_confirmed_at: row.try_get("last_confirmed_at")?,
        resolved_at: row.try_get("resolved_at")?,
        resolution_reason: row.try_get("resolution_reason")?,
        resolution_note: row.try_get("resolution_note")?,
        resolution_actor_id: row.try_get("resolution_actor_id")?,
        backfilled: row.try_get("backfilled")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

#[cfg(test)]
fn sort_operational_episodes(rows: &mut [OperationalAlertEpisodeRecord], history: bool) {
    rows.sort_by(|left, right| {
        (if history {
            std::cmp::Ordering::Equal
        } else {
            record_kind_rank(&left.record_kind).cmp(&record_kind_rank(&right.record_kind))
        })
        .then_with(|| {
            if history {
                std::cmp::Ordering::Equal
            } else {
                lifecycle_rank(&left.lifecycle_state).cmp(&lifecycle_rank(&right.lifecycle_state))
            }
        })
        .then_with(|| {
            if history {
                std::cmp::Ordering::Equal
            } else {
                severity_rank(&left.severity).cmp(&severity_rank(&right.severity))
            }
        })
        .then_with(|| compare_timestamps_desc(&left.triggered_at, &right.triggered_at))
        .then_with(|| right.id.cmp(&left.id))
    });
}

#[cfg(test)]
fn effective_memory_operator_state(
    state: Option<&crate::model_alert_states::FleetAlertStateView>,
    now: i64,
) -> &str {
    match state {
        Some(state)
            if state.state == "muted"
                && state.muted_until_unix.is_some_and(|until| until <= now) =>
        {
            "open"
        }
        Some(state) => state.state.as_str(),
        None => "open",
    }
}

#[cfg(test)]
fn record_kind_rank(kind: &str) -> usize {
    usize::from(kind != "condition")
}

#[cfg(test)]
fn lifecycle_rank(state: &str) -> usize {
    match state {
        "triggered" | "persisting" => 0,
        "unknown" => 1,
        _ => 2,
    }
}

#[cfg(test)]
fn severity_rank(severity: &str) -> usize {
    match severity {
        "critical" => 0,
        "warning" => 1,
        _ => 2,
    }
}

#[cfg(test)]
fn now_string() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
fn canonical_source_time(value: &str) -> Result<String> {
    parse_timestamp_utc(value)
        .map(|value| value.to_rfc3339())
        .with_context(|| format!("invalid operational alert source timestamp: {value}"))
}

fn timestamp_is_after(candidate: &str, boundary: &str) -> bool {
    match (
        parse_timestamp_utc(candidate),
        parse_timestamp_utc(boundary),
    ) {
        (Some(candidate), Some(boundary)) => candidate > boundary,
        _ => false,
    }
}

fn timestamp_is_at_or_after(candidate: &str, boundary: &str) -> bool {
    match (
        parse_timestamp_utc(candidate),
        parse_timestamp_utc(boundary),
    ) {
        (Some(candidate), Some(boundary)) => candidate >= boundary,
        _ => false,
    }
}

fn timestamp_is_before(candidate: &str, boundary: &str) -> bool {
    match (
        parse_timestamp_utc(candidate),
        parse_timestamp_utc(boundary),
    ) {
        (Some(candidate), Some(boundary)) => candidate < boundary,
        _ => false,
    }
}

fn classify_bootstrap_condition_probes(probes: &mut [ConditionProbe], cutoff_at: &str) {
    for probe in probes {
        let retained_unknown_backfill = probe
            .source
            .evidence
            .get("retain_unknown_backfill")
            .and_then(Value::as_bool)
            == Some(true);
        probe.backfilled =
            retained_unknown_backfill || timestamp_is_before(&probe.source.observed_at, cutoff_at);
        let legacy_tunnel_identity = probe
            .source
            .evidence
            .get("topology_identity_validation")
            .and_then(Value::as_str)
            == Some("legacy_backfill");
        if legacy_tunnel_identity && !probe.backfilled {
            probe.state = ProbeState::Unknown;
            probe.source.source_status = match probe.source.producer_kind.as_str() {
                "tunnel_adapter" => "tunnel_adapter_evidence_missing",
                _ => "tunnel_traffic_evidence_missing",
            }
            .to_string();
            probe.source.title = match probe.source.producer_kind.as_str() {
                "tunnel_adapter" => "Tunnel adapter status is unavailable",
                _ => "Tunnel traffic status is unavailable",
            }
            .to_string();
            probe.source.detail =
                "Tunnel evidence does not identify the applied runtime configuration".to_string();
            if let Some(evidence) = probe.source.evidence.as_object_mut() {
                evidence.insert(
                    "topology_identity_validation".to_string(),
                    json!("unavailable"),
                );
                evidence.insert("evidence_status".to_string(), json!("identity_missing"));
            }
        }
    }
}

fn parse_episode_time(value: &str) -> Result<DateTime<Utc>> {
    parse_timestamp_utc(value)
        .with_context(|| format!("invalid operational alert timestamp: {value}"))
}

#[cfg(test)]
fn max_time_string(current: Option<&str>, candidate: &str) -> String {
    let current_time = current.and_then(parse_timestamp_utc);
    let candidate_time = parse_timestamp_utc(candidate);
    match (current_time, candidate_time) {
        (Some(current), Some(candidate)) if current >= candidate => current.to_rfc3339(),
        (_, Some(candidate)) => candidate.to_rfc3339(),
        (Some(current), None) => current.to_rfc3339(),
        (None, None) => now_string(),
    }
}

#[cfg(test)]
fn causal_resolution_time(episode: &OperationalAlertEpisodeRecord, candidate: &str) -> String {
    let confirmed = episode.last_confirmed_at.as_deref();
    max_time_string(confirmed, &max_time_string(Some(candidate), &now_string()))
}

#[cfg(test)]
fn causal_now(episode: &OperationalAlertEpisodeRecord) -> String {
    causal_resolution_time(episode, &now_string())
}

fn source_identity_evidence(client_id: &str, display_name: Option<&str>, tags: &[String]) -> Value {
    json!({
        "subject": {
            "client_id": client_id,
            "display_name": display_name.unwrap_or(client_id),
            "tags": tags,
        }
    })
}

async fn record_postgres_policy_condition_probes_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    probes: Vec<ConditionProbe>,
) -> Result<()> {
    for mut probe in probes {
        if probe.backfilled {
            let public_id = legacy_public_id(&probe.source);
            let evidence = probe
                .source
                .evidence
                .as_object_mut()
                .context("operational evidence is always an object")?;
            evidence.insert("backfilled".to_string(), json!(true));
            evidence.insert("legacy_public_id".to_string(), json!(public_id));
        }
        record_policy_evidence_in_tx(
            tx,
            policy_fact_from_source(probe.source, Some(probe.state))?,
        )
        .await?;
    }
    Ok(())
}

fn policy_condition_probe_identities(
    probes: &[ConditionProbe],
) -> Result<BTreeSet<(String, String)>> {
    probes
        .iter()
        .map(|probe| {
            Ok((
                policy_source_kind_for_producer(&probe.source.producer_kind)?.to_string(),
                probe.source.natural_key.clone(),
            ))
        })
        .collect()
}

async fn record_postgres_policy_event_sources_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    sources: Vec<AlertSource>,
) -> Result<()> {
    for source in sources {
        record_policy_evidence_in_tx(tx, policy_fact_from_source(source, None)?).await?;
    }
    Ok(())
}

fn policy_fact_from_source(
    source: AlertSource,
    probe_state: Option<ProbeState>,
) -> Result<PolicyEvidenceFact> {
    let observed_at = parse_episode_time(&source.observed_at)?;
    let source_kind = policy_source_kind_for_producer(&source.producer_kind)?;
    let fact_kind = match source_kind {
        "agent.status" | "agent.access" | "tunnel.adapter" | "tunnel.traffic" => {
            AlertPolicyRuleKind::State
        }
        "job.terminal" | "backup.failure" | "job.capability" => AlertPolicyRuleKind::Occurrence,
        _ => unreachable!("policy source mapping is exhaustive"),
    };
    let mut payload = source.evidence.clone();
    let payload_object = payload
        .as_object_mut()
        .context("operational policy evidence payload is not an object")?;
    let normalized_status = match source_kind {
        "job.capability" => "skipped",
        _ => source.source_status.as_str(),
    };
    payload_object.insert("status".to_string(), json!(normalized_status));
    payload_object.insert("source_status".to_string(), json!(source.source_status));
    payload_object.insert("reason".to_string(), json!(source.detail));
    payload_object.insert("client_id".to_string(), json!(source.client_id));
    match source_kind {
        "tunnel.adapter" => {
            let adapter = payload_object
                .get("adapter_health")
                .cloned()
                .unwrap_or(Value::Null);
            payload_object.insert("adapter".to_string(), adapter);
            if let Some(interface) = payload_object
                .get("plan")
                .and_then(|plan| plan.get("interface"))
                .cloned()
            {
                payload_object.insert("interface".to_string(), interface);
            }
        }
        "tunnel.traffic" => {
            let traffic_status = payload_object
                .get("traffic_status")
                .cloned()
                .unwrap_or(Value::Null);
            payload_object.insert("traffic".to_string(), json!({"status": traffic_status}));
            if let Some(interface) = payload_object
                .get("plan")
                .and_then(|plan| plan.get("interface"))
                .cloned()
            {
                payload_object.insert("interface".to_string(), interface);
            }
        }
        "job.terminal" => {
            payload_object.insert("job_id".to_string(), json!(source.target_id));
        }
        "backup.failure" => {
            payload_object.insert("backup_request_id".to_string(), json!(source.target_id));
        }
        _ => {}
    }

    let subject_snapshot = source
        .evidence
        .get("subject")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let causation_id = source
        .evidence
        .get("causation_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok());
    let schedule_lineage = source
        .evidence
        .get("schedule_lineage")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .filter_map(|value| Uuid::parse_str(value).ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    // State identity is derived only from source-semantic fields. Subject
    // labels/tags and rendered presentation are evaluated through the separate
    // scope-revision fact; hashing them here would turn a rename/tag edit plus
    // the next repair probe into two fake source confirmations.
    let source_event_id = if fact_kind == AlertPolicyRuleKind::Occurrence {
        source.natural_key.clone()
    } else {
        alert_policy_state_source_event_id(
            source_kind,
            &source.natural_key,
            observed_at.timestamp_nanos_opt().unwrap_or_default(),
            &payload,
        )
    };
    Ok(PolicyEvidenceFact {
        source_kind: source_kind.to_string(),
        source_event_id,
        fact_kind,
        natural_key: source.natural_key.clone(),
        confirmation_bucket_key: source.natural_key,
        subject_client_id: source.client_id,
        target_kind: source.target_kind,
        target_id: source.target_id,
        source_status: normalized_status.to_string(),
        complete: probe_state.is_none_or(|state| state != ProbeState::Unknown),
        subject_snapshot,
        payload,
        observed_at,
        state_started_at: (fact_kind != AlertPolicyRuleKind::Occurrence).then_some(observed_at),
        causation_id,
        schedule_lineage,
    })
}

fn policy_source_kind_for_producer(producer_kind: &str) -> Result<&'static str> {
    match producer_kind {
        "agent_status" => Ok("agent.status"),
        "agent_access" => Ok("agent.access"),
        "tunnel_adapter" => Ok("tunnel.adapter"),
        "tunnel_traffic" => Ok("tunnel.traffic"),
        "job" => Ok("job.terminal"),
        "backup_request" => Ok("backup.failure"),
        "capability_degraded" => Ok("job.capability"),
        other => anyhow::bail!("unsupported operational policy evidence source {other}"),
    }
}

fn agent_probes(
    client_id: &str,
    display_name: &str,
    status: &str,
    tags: &[String],
    observed_at: &str,
    extra_evidence: Value,
    backfilled: bool,
) -> Vec<ConditionProbe> {
    let mut evidence = source_identity_evidence(client_id, Some(display_name), tags);
    if let (Some(target), Some(extra)) = (evidence.as_object_mut(), extra_evidence.as_object()) {
        target.extend(extra.clone());
    }
    let connectivity_confirmed = matches!(status, "never" | "disconnected" | "offline" | "stale");
    let connectivity = ConditionProbe {
        state: if connectivity_confirmed {
            ProbeState::Confirmed
        } else {
            ProbeState::Healthy
        },
        backfilled,
        source: AlertSource {
            producer_kind: "agent_status".to_string(),
            natural_key: format!("{client_id}:connectivity"),
            severity: if status == "offline" {
                "critical"
            } else {
                "warning"
            }
            .to_string(),
            category: "agent_status".to_string(),
            target_kind: "agent".to_string(),
            target_id: client_id.to_string(),
            client_id: Some(client_id.to_string()),
            title: "Agent is not online".to_string(),
            detail: format!("{display_name} currently reports {status}"),
            source_status: status.to_string(),
            evidence: evidence.clone(),
            observed_at: observed_at.to_string(),
        },
    };
    let access = ConditionProbe {
        state: if status == "revoked" {
            ProbeState::Confirmed
        } else {
            ProbeState::Healthy
        },
        backfilled,
        source: AlertSource {
            producer_kind: "agent_access".to_string(),
            natural_key: format!("{client_id}:access"),
            severity: "critical".to_string(),
            category: "agent_status".to_string(),
            target_kind: "agent".to_string(),
            target_id: client_id.to_string(),
            client_id: Some(client_id.to_string()),
            title: "VPS access revoked".to_string(),
            detail: format!("{display_name} cannot reconnect until an operator assigns a new key"),
            source_status: status.to_string(),
            evidence,
            observed_at: observed_at.to_string(),
        },
    };
    vec![connectivity, access]
}

fn merge_json(mut base: Value, extra: Value) -> Value {
    if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
        base.extend(extra.clone());
    }
    base
}

async fn load_postgres_snapshot(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    condition_bootstrapping: bool,
    event_source_bootstrapping: bool,
    event_source_cutoff_at: DateTime<Utc>,
    condition_client_ids: Option<&[String]>,
) -> Result<OperationalSnapshot> {
    let mut snapshot = OperationalSnapshot::default();
    let agent_rows = sqlx::query(
        r#"
        SELECT c.id, c.display_name, c.status, c.capabilities,
               c.operational_alert_status_at::text AS observed_at,
               c.operational_alert_legacy_status,
               c.operational_alert_tunnel_boundary_at::text AS tunnel_boundary_at,
               COALESCE(
                   (SELECT jsonb_agg(t.name ORDER BY t.display_order, t.name)
                    FROM client_tags ct JOIN tags t ON t.id = ct.tag_id
                    WHERE ct.client_id = c.id),
                   '[]'::jsonb
               ) AS tags
        FROM clients c
        WHERE c.hidden_at IS NULL
          AND ($1::text[] IS NULL OR c.id = ANY($1))
        ORDER BY c.id
        "#,
    )
    .bind(condition_client_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut visible_agent_identity =
        HashMap::<String, (String, Vec<String>, String, Option<String>)>::new();
    for row in agent_rows {
        let client_id: String = row.try_get("id")?;
        let display_name: String = row.try_get("display_name")?;
        let status: String = row.try_get("status")?;
        let tags: Value = row.try_get("tags")?;
        let tags = serde_json::from_value::<Vec<String>>(tags).unwrap_or_default();
        let capabilities: Value = row.try_get("capabilities")?;
        let observed_at: String = row.try_get("observed_at")?;
        let legacy_status: bool = row.try_get("operational_alert_legacy_status")?;
        let tunnel_boundary_at: Option<String> = row.try_get("tunnel_boundary_at")?;
        snapshot.conditions.extend(agent_probes(
            &client_id,
            &display_name,
            &status,
            &tags,
            &observed_at,
            json!({
                "capability_privilege_mode": capabilities.get("privilege_mode"),
            }),
            condition_bootstrapping && legacy_status,
        ));
        visible_agent_identity.insert(client_id, (display_name, tags, status, tunnel_boundary_at));
    }

    let telemetry_rows = sqlx::query(
        r#"
        SELECT client_id, interface, observed_at::text AS observed_at,
               updated_at::text AS accepted_at,
               telemetry_plan_id, telemetry_topology_identity_hash,
               telemetry_runtime_evidence_identity_hash,
               operational_alert_legacy_identity,
               telemetry_endpoint_side,
               traffic_source, traffic_status, traffic_reason, adapter_health
        FROM telemetry_tunnels
        WHERE ($1::text[] IS NULL OR client_id = ANY($1))
        ORDER BY client_id, interface
        "#,
    )
    .bind(condition_client_ids)
    .fetch_all(&mut **tx)
    .await?;
    let telemetry = telemetry_rows
        .into_iter()
        .map(|row| {
            Ok(PostgresTunnelEvidence {
                client_id: row.try_get("client_id")?,
                interface: row.try_get("interface")?,
                observed_at: row.try_get("observed_at")?,
                accepted_at: row.try_get("accepted_at")?,
                plan_id: row
                    .try_get::<Option<String>, _>("telemetry_plan_id")?
                    .and_then(|value| Uuid::parse_str(&value).ok()),
                topology_identity_hash: row.try_get("telemetry_topology_identity_hash")?,
                runtime_evidence_identity_hash: row
                    .try_get("telemetry_runtime_evidence_identity_hash")?,
                legacy_identity: row.try_get("operational_alert_legacy_identity")?,
                endpoint_side: row.try_get("telemetry_endpoint_side")?,
                traffic_source: row.try_get("traffic_source")?,
                traffic_status: row.try_get("traffic_status")?,
                traffic_reason: row.try_get("traffic_reason")?,
                adapter_health: row.try_get("adapter_health")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let plan_rows = sqlx::query(
        r#"
        SELECT id, name, revision, left_client_id, right_client_id, plan,
               builtin_credentials,
               operational_alert_legacy_runtime_identity,
               operational_alert_runtime_boundary_at::text AS runtime_boundary_at
        FROM tunnel_plans
        WHERE enabled AND deleted_at IS NULL
          AND (
            $1::text[] IS NULL
            OR left_client_id = ANY($1)
            OR right_client_id = ANY($1)
          )
        ORDER BY id
        "#,
    )
    .bind(condition_client_ids)
    .fetch_all(&mut **tx)
    .await?;
    for row in plan_rows {
        let id: Uuid = row.try_get("id")?;
        let name: String = row.try_get("name")?;
        let revision: i64 = row.try_get("revision")?;
        let left: String = row.try_get("left_client_id")?;
        let right: String = row.try_get("right_client_id")?;
        let plan: Value = row.try_get("plan")?;
        let plan = serde_json::from_value::<vpsman_common::TunnelPlan>(plan)
            .context("invalid persisted tunnel plan during alert reconciliation")?;
        let credential_generation = row
            .try_get::<Option<sqlx::types::Json<serde_json::Value>>, _>("builtin_credentials")?
            .map(|value| {
                serde_json::from_value::<vpsman_common::TunnelBuiltinCredentials>(value.0)
                    .context("invalid persisted tunnel credentials during alert reconciliation")
                    .map(|credentials| credentials.generation())
            })
            .transpose()?;
        let legacy_runtime_identity: bool =
            row.try_get("operational_alert_legacy_runtime_identity")?;
        let runtime_boundary_at: String = row.try_get("runtime_boundary_at")?;
        for (side, client_id, peer_client_id) in [
            ("left", left.as_str(), right.as_str()),
            ("right", right.as_str(), left.as_str()),
        ] {
            let Some((display_name, tags, agent_status, status_boundary_at)) =
                visible_agent_identity.get(client_id)
            else {
                continue;
            };
            let tunnel = telemetry.iter().find(|tunnel| {
                tunnel.client_id == client_id
                    && tunnel.interface == plan.interface_name
                    && tunnel.plan_id == Some(id)
                    && tunnel.endpoint_side.as_deref() == Some(side)
            });
            append_postgres_tunnel_endpoint_probes(
                &mut snapshot,
                id,
                revision,
                &name,
                &plan,
                credential_generation,
                legacy_runtime_identity,
                &runtime_boundary_at,
                side,
                client_id,
                peer_client_id,
                display_name,
                tags,
                agent_status,
                status_boundary_at.as_deref(),
                tunnel,
                if condition_bootstrapping {
                    TunnelEvidenceMode::LegacyBootstrap
                } else {
                    TunnelEvidenceMode::Exact
                },
            );
        }
    }

    append_postgres_event_sources(
        tx,
        &mut snapshot,
        event_source_bootstrapping,
        event_source_cutoff_at,
    )
    .await?;
    if condition_bootstrapping {
        classify_bootstrap_condition_probes(
            &mut snapshot.conditions,
            &event_source_cutoff_at.to_rfc3339(),
        );
    }
    Ok(snapshot)
}

struct PostgresTunnelEvidence {
    client_id: String,
    interface: String,
    observed_at: String,
    accepted_at: String,
    plan_id: Option<Uuid>,
    topology_identity_hash: Option<String>,
    runtime_evidence_identity_hash: Option<String>,
    legacy_identity: bool,
    endpoint_side: Option<String>,
    traffic_source: Option<String>,
    traffic_status: Option<String>,
    traffic_reason: Option<String>,
    adapter_health: Option<Value>,
}

async fn load_postgres_tunnel_probes_for_clients_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_ids: &[String],
    evidence_mode: TunnelEvidenceMode,
) -> Result<Vec<ConditionProbe>> {
    let agent_rows = sqlx::query(
        r#"
        SELECT c.id, c.display_name, c.status,
               c.operational_alert_tunnel_boundary_at::text AS tunnel_boundary_at,
               COALESCE(
                   (SELECT jsonb_agg(t.name ORDER BY t.display_order, t.name)
                    FROM client_tags ct JOIN tags t ON t.id = ct.tag_id
                    WHERE ct.client_id = c.id),
                   '[]'::jsonb
               ) AS tags
        FROM clients c
        WHERE c.id = ANY($1::text[]) AND c.hidden_at IS NULL
        "#,
    )
    .bind(client_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut agents = HashMap::<String, (String, Vec<String>, String, Option<String>)>::new();
    for row in agent_rows {
        let tags = serde_json::from_value::<Vec<String>>(row.try_get("tags")?).unwrap_or_default();
        agents.insert(
            row.try_get("id")?,
            (
                row.try_get("display_name")?,
                tags,
                row.try_get("status")?,
                row.try_get("tunnel_boundary_at")?,
            ),
        );
    }

    let telemetry_rows = sqlx::query(
        r#"
        SELECT client_id, interface, observed_at::text AS observed_at,
               updated_at::text AS accepted_at,
               telemetry_plan_id, telemetry_topology_identity_hash,
               telemetry_runtime_evidence_identity_hash,
               operational_alert_legacy_identity,
               telemetry_endpoint_side, traffic_source, traffic_status,
               traffic_reason, adapter_health
        FROM telemetry_tunnels
        WHERE client_id = ANY($1::text[])
        "#,
    )
    .bind(client_ids)
    .fetch_all(&mut **tx)
    .await?;
    let telemetry = telemetry_rows
        .into_iter()
        .map(|row| {
            Ok(PostgresTunnelEvidence {
                client_id: row.try_get("client_id")?,
                interface: row.try_get("interface")?,
                observed_at: row.try_get("observed_at")?,
                accepted_at: row.try_get("accepted_at")?,
                plan_id: row
                    .try_get::<Option<String>, _>("telemetry_plan_id")?
                    .and_then(|value| Uuid::parse_str(&value).ok()),
                topology_identity_hash: row.try_get("telemetry_topology_identity_hash")?,
                runtime_evidence_identity_hash: row
                    .try_get("telemetry_runtime_evidence_identity_hash")?,
                legacy_identity: row.try_get("operational_alert_legacy_identity")?,
                endpoint_side: row.try_get("telemetry_endpoint_side")?,
                traffic_source: row.try_get("traffic_source")?,
                traffic_status: row.try_get("traffic_status")?,
                traffic_reason: row.try_get("traffic_reason")?,
                adapter_health: row.try_get("adapter_health")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let plan_rows = sqlx::query(
        r#"
        SELECT id, name, revision, left_client_id, right_client_id, plan,
               builtin_credentials,
               operational_alert_legacy_runtime_identity,
               operational_alert_runtime_boundary_at::text AS runtime_boundary_at
        FROM tunnel_plans
        WHERE enabled AND deleted_at IS NULL
          AND (left_client_id = ANY($1::text[]) OR right_client_id = ANY($1::text[]))
        ORDER BY id
        "#,
    )
    .bind(client_ids)
    .fetch_all(&mut **tx)
    .await?;
    let requested = client_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut snapshot = OperationalSnapshot::default();
    for row in plan_rows {
        let id: Uuid = row.try_get("id")?;
        let name: String = row.try_get("name")?;
        let revision: i64 = row.try_get("revision")?;
        let left: String = row.try_get("left_client_id")?;
        let right: String = row.try_get("right_client_id")?;
        let plan = serde_json::from_value::<vpsman_common::TunnelPlan>(row.try_get("plan")?)
            .context("invalid persisted tunnel plan during alert reconciliation")?;
        let credential_generation = row
            .try_get::<Option<sqlx::types::Json<serde_json::Value>>, _>("builtin_credentials")?
            .map(|value| {
                serde_json::from_value::<vpsman_common::TunnelBuiltinCredentials>(value.0)
                    .context("invalid persisted tunnel credentials during alert reconciliation")
                    .map(|credentials| credentials.generation())
            })
            .transpose()?;
        let legacy_runtime_identity: bool =
            row.try_get("operational_alert_legacy_runtime_identity")?;
        let runtime_boundary_at: String = row.try_get("runtime_boundary_at")?;
        for (side, client_id, peer_client_id) in [
            ("left", left.as_str(), right.as_str()),
            ("right", right.as_str(), left.as_str()),
        ] {
            if !requested.contains(client_id) {
                continue;
            }
            let Some((display_name, tags, agent_status, status_boundary_at)) =
                agents.get(client_id)
            else {
                continue;
            };
            let tunnel = telemetry.iter().find(|tunnel| {
                tunnel.client_id == client_id
                    && tunnel.interface == plan.interface_name
                    && tunnel.plan_id == Some(id)
                    && tunnel.endpoint_side.as_deref() == Some(side)
            });
            append_postgres_tunnel_endpoint_probes(
                &mut snapshot,
                id,
                revision,
                &name,
                &plan,
                credential_generation,
                legacy_runtime_identity,
                &runtime_boundary_at,
                side,
                client_id,
                peer_client_id,
                display_name,
                tags,
                agent_status,
                status_boundary_at.as_deref(),
                tunnel,
                evidence_mode,
            );
        }
    }
    Ok(snapshot.conditions)
}

#[allow(clippy::too_many_arguments)]
fn append_postgres_tunnel_endpoint_probes(
    snapshot: &mut OperationalSnapshot,
    plan_id: Uuid,
    plan_revision: i64,
    plan_name: &str,
    plan: &vpsman_common::TunnelPlan,
    credential_generation: Option<u64>,
    legacy_runtime_identity: bool,
    runtime_boundary_at: &str,
    side: &str,
    client_id: &str,
    peer_client_id: &str,
    display_name: &str,
    tags: &[String],
    agent_status: &str,
    status_boundary_at: Option<&str>,
    tunnel: Option<&PostgresTunnelEvidence>,
    evidence_mode: TunnelEvidenceMode,
) {
    let expected_topology_identity_hash = tunnel_topology_identity_hash(plan_id, plan);
    let expected_runtime_evidence_identity_hash =
        tunnel_runtime_evidence_identity_hash(plan_id, plan, credential_generation);
    let exact_identity = tunnel.is_some_and(|tunnel| {
        tunnel.runtime_evidence_identity_hash.as_deref()
            == Some(expected_runtime_evidence_identity_hash.as_str())
            && timestamp_is_after(&tunnel.accepted_at, runtime_boundary_at)
    });
    let status_evidence_available = evidence_mode != TunnelEvidenceMode::Unavailable
        && agent_status == "online"
        && status_boundary_at.is_none_or(|boundary| {
            tunnel.is_some_and(|tunnel| timestamp_is_after(&tunnel.accepted_at, boundary))
        });
    let marked_legacy_identity = evidence_mode == TunnelEvidenceMode::LegacyBootstrap
        && tunnel.is_some_and(|tunnel| {
            legacy_runtime_identity
                && tunnel.legacy_identity
                && tunnel.runtime_evidence_identity_hash.is_none()
        });
    let legacy_identity = marked_legacy_identity
        && tunnel.is_some_and(|tunnel| {
            timestamp_is_at_or_after(&tunnel.accepted_at, runtime_boundary_at)
        });
    let evidence_available = status_evidence_available && (exact_identity || legacy_identity);
    let retain_unattributed_bootstrap_degradation = marked_legacy_identity && !evidence_available;
    let legacy_evidence = evidence_available && legacy_identity;
    let attributed_tunnel = evidence_available.then_some(tunnel).flatten();
    let reported_tunnel = attributed_tunnel.or_else(|| {
        retain_unattributed_bootstrap_degradation
            .then_some(tunnel)
            .flatten()
    });
    let observed_at = reported_tunnel
        .map(|tunnel| tunnel.accepted_at.as_str())
        .or(status_boundary_at)
        .unwrap_or(runtime_boundary_at);
    let base = json!({
        "subject": {"client_id": client_id, "display_name": display_name, "tags": tags},
        "plan": {
            "id": plan_id,
            "name": plan_name,
            "revision": plan_revision,
            "topology_identity_hash": &expected_topology_identity_hash,
            "runtime_evidence_identity_hash": &expected_runtime_evidence_identity_hash,
            "endpoint_side": side,
            "peer_client_id": peer_client_id,
            "interface": &plan.interface_name,
        },
        "telemetry_observed_at": reported_tunnel.map(|tunnel| tunnel.observed_at.as_str()),
        "telemetry_accepted_at": reported_tunnel.map(|tunnel| tunnel.accepted_at.as_str()),
        "reported_topology_identity_hash": reported_tunnel
            .and_then(|tunnel| tunnel.topology_identity_hash.as_deref()),
        "reported_runtime_evidence_identity_hash": reported_tunnel
            .and_then(|tunnel| tunnel.runtime_evidence_identity_hash.as_deref()),
        "status_boundary_at": status_boundary_at,
        "runtime_boundary_at": runtime_boundary_at,
        "topology_identity_validation": if legacy_evidence {
            "legacy_backfill"
        } else if retain_unattributed_bootstrap_degradation {
            "bootstrap_current_attribution_unavailable"
        } else if attributed_tunnel.is_some() {
            "exact"
        } else {
            "unavailable"
        },
    });
    let build = |producer: &str,
                 severity: &str,
                 status: &str,
                 title: &str,
                 detail: String,
                 evidence: Value| AlertSource {
        producer_kind: producer.to_string(),
        natural_key: format!("{plan_id}:{expected_runtime_evidence_identity_hash}:{side}"),
        severity: severity.to_string(),
        category: "network".to_string(),
        target_kind: "tunnel".to_string(),
        target_id: format!("{}:{}", client_id, plan.interface_name),
        client_id: Some(client_id.to_string()),
        title: title.to_string(),
        detail,
        source_status: status.to_string(),
        evidence,
        observed_at: observed_at.to_string(),
    };
    if plan.runtime_control.manager == RuntimeTunnelManager::CustomAdapter {
        let adapter = attributed_tunnel.and_then(|tunnel| tunnel.adapter_health.as_ref());
        let retained_adapter = retain_unattributed_bootstrap_degradation
            .then(|| tunnel.and_then(|tunnel| tunnel.adapter_health.as_ref()))
            .flatten();
        let success = adapter
            .and_then(|value| value.get("success"))
            .and_then(Value::as_bool);
        let retained_degradation = success.is_none()
            && retained_adapter
                .and_then(|value| value.get("success"))
                .and_then(Value::as_bool)
                == Some(false);
        let (state, status, detail) = match (success, retained_degradation) {
            (Some(true), _) => (
                ProbeState::Healthy,
                "tunnel_adapter_healthy",
                "Tunnel adapter status is healthy".to_string(),
            ),
            (Some(false), _) => (
                ProbeState::Confirmed,
                "tunnel_adapter_degraded",
                adapter
                    .and_then(|value| value.get("reason"))
                    .and_then(Value::as_str)
                    .unwrap_or("adapter command did not report healthy status")
                    .to_string(),
            ),
            (None, true) => (
                ProbeState::Unknown,
                "tunnel_adapter_degraded",
                retained_adapter
                    .and_then(|value| value.get("reason"))
                    .and_then(Value::as_str)
                    .unwrap_or("adapter command did not report healthy status")
                    .to_string(),
            ),
            (None, false) => (
                ProbeState::Unknown,
                "tunnel_adapter_evidence_missing",
                "Tunnel adapter health evidence is unavailable".to_string(),
            ),
        };
        let title = if state == ProbeState::Unknown && !retained_degradation {
            "Tunnel adapter status is unavailable"
        } else {
            "Tunnel adapter status failed"
        };
        snapshot.conditions.push(ConditionProbe {
            state,
            backfilled: legacy_evidence || retained_degradation,
            source: build(
                "tunnel_adapter",
                "critical",
                status,
                title,
                detail,
                merge_json(
                    base.clone(),
                    json!({
                        "adapter_health": adapter.or(retained_adapter),
                        "retain_unknown_backfill": retained_degradation,
                        "evidence_status": retained_degradation
                            .then_some("retained_degradation_current_attribution_unavailable"),
                    }),
                ),
            ),
        });
    }
    let traffic_status = attributed_tunnel.and_then(|tunnel| tunnel.traffic_status.as_deref());
    let retained_traffic_status = retain_unattributed_bootstrap_degradation
        .then(|| tunnel.and_then(|tunnel| tunnel.traffic_status.as_deref()))
        .flatten();
    let retained_degradation =
        traffic_status.is_none() && retained_traffic_status.is_some_and(|status| status != "ok");
    let (state, status, detail) = match (traffic_status, retained_degradation) {
        (Some("ok"), _) => (
            ProbeState::Healthy,
            "tunnel_traffic_ok",
            "Tunnel interface counters are healthy".to_string(),
        ),
        (Some(_), _) => (
            ProbeState::Confirmed,
            "tunnel_traffic_degraded",
            attributed_tunnel
                .and_then(|tunnel| tunnel.traffic_reason.clone())
                .unwrap_or_else(|| "tunnel interface counters are not reporting ok".to_string()),
        ),
        (None, true) => (
            ProbeState::Unknown,
            "tunnel_traffic_degraded",
            tunnel
                .and_then(|tunnel| tunnel.traffic_reason.clone())
                .unwrap_or_else(|| "tunnel interface counters are not reporting ok".to_string()),
        ),
        (None, false) => (
            ProbeState::Unknown,
            "tunnel_traffic_evidence_missing",
            "Tunnel traffic counter evidence is unavailable".to_string(),
        ),
    };
    let title = if state == ProbeState::Unknown && !retained_degradation {
        "Tunnel traffic status is unavailable"
    } else {
        "Tunnel interface counters are degraded"
    };
    snapshot.conditions.push(ConditionProbe {
        state,
        backfilled: legacy_evidence || retained_degradation,
        source: build(
            "tunnel_traffic",
            "warning",
            status,
            title,
            detail,
            merge_json(
                base,
                json!({
                    "traffic_source": reported_tunnel
                        .and_then(|tunnel| tunnel.traffic_source.as_ref()),
                    "traffic_status": reported_tunnel
                        .and_then(|tunnel| tunnel.traffic_status.as_ref()),
                    "traffic_reason": reported_tunnel
                        .and_then(|tunnel| tunnel.traffic_reason.as_ref()),
                    "retain_unknown_backfill": retained_degradation,
                    "evidence_status": retained_degradation
                        .then_some("retained_degradation_current_attribution_unavailable"),
                }),
            ),
        ),
    });
}

async fn load_postgres_retained_identities(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_ids: &[String],
) -> Result<HashMap<String, (String, Vec<String>)>> {
    if client_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT client.id, client.display_name,
               COALESCE(
                   (SELECT jsonb_agg(tag.name ORDER BY tag.display_order, tag.name)
                    FROM client_tags client_tag
                    JOIN tags tag ON tag.id = client_tag.tag_id
                    WHERE client_tag.client_id = client.id),
                   '[]'::jsonb
               ) AS tags
        FROM clients client
        WHERE client.id = ANY($1::text[])
        "#,
    )
    .bind(client_ids)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            let client_id: String = row.try_get("id")?;
            let tags =
                serde_json::from_value::<Vec<String>>(row.try_get("tags")?).unwrap_or_default();
            Ok((client_id, (row.try_get("display_name")?, tags)))
        })
        .collect()
}

async fn append_postgres_event_sources(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    snapshot: &mut OperationalSnapshot,
    bootstrapping: bool,
    event_source_cutoff_at: DateTime<Utc>,
) -> Result<()> {
    let rows = sqlx::query(
        r#"
        WITH legacy AS (
            SELECT id, command_type, status, target_count, alert_terminal_at,
                   causation_id, schedule_lineage,
                   TRUE AS backfilled
            FROM jobs
            WHERE $1::boolean
              AND status IN ('partial_success', 'canceled', 'rejected', 'failed', 'agent_timeout', 'control_timeout')
              AND alert_terminal_at < $2
            ORDER BY
                CASE WHEN status = 'partial_success' THEN 1 ELSE 0 END ASC,
                alert_terminal_at DESC,
                id DESC
            LIMIT $3
        ), new_sources AS (
            SELECT id, command_type, status, target_count, alert_terminal_at,
                   causation_id, schedule_lineage,
                   FALSE AS backfilled
            FROM jobs
            WHERE status IN ('partial_success', 'canceled', 'rejected', 'failed', 'agent_timeout', 'control_timeout')
              AND alert_terminal_at >= $2
              AND NOT EXISTS (
                  SELECT 1 FROM alert_policy_evidence evidence
                  WHERE evidence.source_kind = 'job.terminal'
                    AND evidence.source_event_id = jobs.id::text
              )
            ORDER BY alert_terminal_at ASC, id ASC
            LIMIT $3
        )
        SELECT id, command_type, status, target_count,
               alert_terminal_at::text AS alert_terminal_at,
               causation_id, schedule_lineage,
               backfilled
        FROM legacy
        UNION ALL
        SELECT id, command_type, status, target_count,
               alert_terminal_at::text AS alert_terminal_at,
               causation_id, schedule_lineage,
               backfilled
        FROM new_sources
        "#,
    )
    .bind(bootstrapping)
    .bind(event_source_cutoff_at)
    .bind(LEGACY_EVENT_SOURCE_HORIZON as i64)
    .fetch_all(&mut **tx)
    .await?;
    for row in rows {
        let id: Uuid = row.try_get("id")?;
        let command_type: String = row.try_get("command_type")?;
        let status: String = row.try_get("status")?;
        let severity = if status == "partial_success" {
            "warning"
        } else {
            "critical"
        };
        let category = if command_type.contains("backup") || command_type.contains("restore") {
            "backup"
        } else if command_type.contains("agent_update") {
            "agent_update"
        } else {
            "job"
        };
        snapshot.events.push(EventSource {
            source: AlertSource {
                producer_kind: "job".to_string(),
                natural_key: id.to_string(),
                severity: severity.to_string(),
                category: category.to_string(),
                target_kind: "job".to_string(),
                target_id: id.to_string(),
                client_id: None,
                title: "Job requires operator attention".to_string(),
                detail: format!("{command_type} job {status}"),
                source_status: status,
                evidence: json!({
                    "job_id": id,
                    "command_type": command_type,
                    "target_count": row.try_get::<i32, _>("target_count")?,
                    "causation_id": row.try_get::<Option<Uuid>, _>("causation_id")?,
                    "schedule_lineage": row.try_get::<Vec<Uuid>, _>("schedule_lineage")?,
                    "retained_identity": true,
                }),
                observed_at: row.try_get("alert_terminal_at")?,
            },
            backfilled: row.try_get("backfilled")?,
        });
    }
    let rows = sqlx::query(
        r#"
        WITH legacy AS (
            SELECT id, client_id, paths, include_config, artifact_id, status,
                   created_at, terminal_at, causation_id, schedule_lineage,
                   TRUE AS backfilled
            FROM backup_requests
            WHERE $1::boolean
              AND status = 'execution_failed'
              AND terminal_at < $2
            ORDER BY terminal_at DESC, id DESC
            LIMIT $3
        ), new_sources AS (
            SELECT id, client_id, paths, include_config, artifact_id, status,
                   created_at, terminal_at, causation_id, schedule_lineage,
                   FALSE AS backfilled
            FROM backup_requests
            WHERE status = 'execution_failed'
              AND terminal_at >= $2
              AND NOT EXISTS (
                  SELECT 1 FROM alert_policy_evidence evidence
                  WHERE evidence.source_kind = 'backup.failure'
                    AND evidence.source_event_id = backup_requests.id::text
              )
            ORDER BY terminal_at ASC, id ASC
            LIMIT $3
        )
        SELECT id, client_id, paths, include_config, artifact_id,
               status, created_at::text AS created_at,
               terminal_at::text AS terminal_at,
               causation_id, schedule_lineage, backfilled
        FROM legacy
        UNION ALL
        SELECT id, client_id, paths, include_config, artifact_id,
               status, created_at::text AS created_at,
               terminal_at::text AS terminal_at,
               causation_id, schedule_lineage, backfilled
        FROM new_sources
        "#,
    )
    .bind(bootstrapping)
    .bind(event_source_cutoff_at)
    .bind(LEGACY_EVENT_SOURCE_HORIZON as i64)
    .fetch_all(&mut **tx)
    .await?;
    let backup_client_ids = rows
        .iter()
        .map(|row| row.try_get("client_id"))
        .collect::<std::result::Result<Vec<String>, _>>()?;
    let backup_identities = load_postgres_retained_identities(tx, &backup_client_ids).await?;
    for row in rows {
        let id: Uuid = row.try_get("id")?;
        let client_id: String = row.try_get("client_id")?;
        let (display_name, tags) = backup_identities
            .get(&client_id)
            .map(|(name, tags)| (name.as_str(), tags.as_slice()))
            .unwrap_or((client_id.as_str(), &[]));
        snapshot.events.push(EventSource {
            source: AlertSource {
                producer_kind: "backup_request".to_string(),
                natural_key: id.to_string(),
                severity: "critical".to_string(),
                category: "backup".to_string(),
                target_kind: "backup_request".to_string(),
                target_id: id.to_string(),
                client_id: Some(client_id.clone()),
                title: "Backup request failed".to_string(),
                detail: format!("backup request {id} is execution_failed"),
                source_status: "execution_failed".to_string(),
                evidence: merge_json(
                    source_identity_evidence(&client_id, Some(display_name), tags),
                    json!({
                        "paths": row.try_get::<Vec<String>, _>("paths")?,
                        "include_config": row.try_get::<bool, _>("include_config")?,
                        "artifact_id": row.try_get::<Option<Uuid>, _>("artifact_id")?,
                        "request_created_at": row.try_get::<String, _>("created_at")?,
                        "causation_id": row.try_get::<Option<Uuid>, _>("causation_id")?,
                        "schedule_lineage": row.try_get::<Vec<Uuid>, _>("schedule_lineage")?,
                        "retained_identity": true,
                    }),
                ),
                observed_at: row.try_get("terminal_at")?,
            },
            backfilled: row.try_get("backfilled")?,
        });
    }
    let rows = sqlx::query(
        r#"
        WITH legacy AS (
            SELECT t.job_id, t.client_id, t.status, t.message, t.exit_code,
                   t.started_at, t.completed_at,
                   t.capability_alert_at,
                   t.capability_degraded_reason, t.capability_degraded_hint,
                   j.command_type, j.causation_id, j.schedule_lineage,
                   TRUE AS backfilled
            FROM job_targets t
            JOIN jobs j ON j.id = t.job_id
            WHERE $1::boolean
              AND t.status = 'skipped'
              AND t.capability_degraded_reason IS NOT NULL
              AND t.capability_degraded_hint IS NOT NULL
              AND t.capability_alert_at < $2
            ORDER BY
                t.capability_alert_at DESC,
                t.job_id DESC,
                t.client_id ASC
            LIMIT $3
        ), new_sources AS (
            SELECT t.job_id, t.client_id, t.status, t.message, t.exit_code,
                   t.started_at, t.completed_at,
                   t.capability_alert_at,
                   t.capability_degraded_reason, t.capability_degraded_hint,
                   j.command_type, j.causation_id, j.schedule_lineage,
                   FALSE AS backfilled
            FROM job_targets t
            JOIN jobs j ON j.id = t.job_id
            WHERE t.status = 'skipped'
              AND t.capability_degraded_reason IS NOT NULL
              AND t.capability_degraded_hint IS NOT NULL
              AND t.capability_alert_at >= $2
              AND NOT EXISTS (
                  SELECT 1 FROM alert_policy_evidence evidence
                  WHERE evidence.source_kind = 'job.capability'
                    AND evidence.source_event_id = t.job_id::text || ':' || t.client_id
              )
            ORDER BY
                t.capability_alert_at ASC,
                t.job_id ASC,
                t.client_id ASC
            LIMIT $3
        )
        SELECT job_id, client_id, status, message, exit_code,
               started_at::text AS started_at, completed_at::text AS completed_at,
               capability_alert_at::text AS capability_alert_at,
               capability_degraded_reason, capability_degraded_hint,
               command_type, causation_id, schedule_lineage, backfilled
        FROM legacy
        UNION ALL
        SELECT job_id, client_id, status, message, exit_code,
               started_at::text AS started_at, completed_at::text AS completed_at,
               capability_alert_at::text AS capability_alert_at,
               capability_degraded_reason, capability_degraded_hint,
               command_type, causation_id, schedule_lineage, backfilled
        FROM new_sources
        "#,
    )
    .bind(bootstrapping)
    .bind(event_source_cutoff_at)
    .bind(LEGACY_EVENT_SOURCE_HORIZON as i64)
    .fetch_all(&mut **tx)
    .await?;
    let capability_client_ids = rows
        .iter()
        .map(|row| row.try_get("client_id"))
        .collect::<std::result::Result<Vec<String>, _>>()?;
    let capability_identities =
        load_postgres_retained_identities(tx, &capability_client_ids).await?;
    for row in rows {
        let job_id: Uuid = row.try_get("job_id")?;
        let client_id: String = row.try_get("client_id")?;
        let reason: String = row.try_get("capability_degraded_reason")?;
        let hint: String = row.try_get("capability_degraded_hint")?;
        let (display_name, tags) = capability_identities
            .get(&client_id)
            .map(|(name, tags)| (name.as_str(), tags.as_slice()))
            .unwrap_or((client_id.as_str(), &[]));
        let completed_at: Option<String> = row.try_get("completed_at")?;
        let started_at: Option<String> = row.try_get("started_at")?;
        snapshot.events.push(EventSource {
            source: AlertSource {
                producer_kind: "capability_degraded".to_string(),
                natural_key: format!("{job_id}:{client_id}"),
                severity: "warning".to_string(),
                category: "capability_degraded".to_string(),
                target_kind: "job_target".to_string(),
                target_id: format!("{job_id}:{client_id}"),
                client_id: Some(client_id.clone()),
                title: "Operation skipped because the agent lacks a required capability"
                    .to_string(),
                detail: hint.clone(),
                source_status: reason.clone(),
                evidence: merge_json(
                    source_identity_evidence(&client_id, Some(display_name), tags),
                    json!({
                        "job_id": job_id,
                        "command_type": row.try_get::<String, _>("command_type")?,
                        "target_status": row.try_get::<String, _>("status")?,
                        "target_message": row.try_get::<Option<String>, _>("message")?,
                        "reason": reason,
                        "hint": hint,
                        "exit_code": row.try_get::<Option<i32>, _>("exit_code")?,
                        "started_at": started_at,
                        "completed_at": completed_at,
                        "causation_id": row.try_get::<Option<Uuid>, _>("causation_id")?,
                        "schedule_lineage": row.try_get::<Vec<Uuid>, _>("schedule_lineage")?,
                        "retained_identity": true,
                    }),
                ),
                observed_at: row.try_get("capability_alert_at")?,
            },
            backfilled: row.try_get("backfilled")?,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn condition(status: &str, at: &str) -> ConditionProbe {
        ConditionProbe {
            state: match status {
                "online" => ProbeState::Healthy,
                "unknown" => ProbeState::Unknown,
                _ => ProbeState::Confirmed,
            },
            backfilled: false,
            source: AlertSource {
                producer_kind: "agent_status".to_string(),
                natural_key: "vps-a".to_string(),
                severity: "critical".to_string(),
                category: "agent_status".to_string(),
                target_kind: "agent".to_string(),
                target_id: "vps-a".to_string(),
                client_id: Some("vps-a".to_string()),
                title: "Agent is not online".to_string(),
                detail: status.to_string(),
                source_status: status.to_string(),
                evidence: json!({"subject":{"client_id":"vps-a"}}),
                observed_at: at.to_string(),
            },
        }
    }

    #[test]
    fn condition_lifecycle_is_trigger_persist_unknown_resolve_and_recur() {
        let mut rows = Vec::new();
        let first = reconcile_snapshot(
            &mut rows,
            OperationalSnapshot {
                conditions: vec![condition("offline", "2026-01-01T00:00:00Z")],
                events: Vec::new(),
            },
            false,
        )
        .unwrap();
        assert_eq!(rows[0].lifecycle_state, "triggered");
        assert_eq!(first.edges.len(), 1);
        let id = rows[0].id;
        reconcile_snapshot(
            &mut rows,
            OperationalSnapshot {
                conditions: vec![condition("offline", "2026-01-01T00:01:00Z")],
                events: Vec::new(),
            },
            false,
        )
        .unwrap();
        assert_eq!(rows[0].lifecycle_state, "persisting");
        reconcile_snapshot(
            &mut rows,
            OperationalSnapshot {
                conditions: vec![condition("unknown", "2026-01-01T00:02:00Z")],
                events: Vec::new(),
            },
            false,
        )
        .unwrap();
        assert_eq!(rows[0].lifecycle_state, "unknown");
        let resolved = reconcile_snapshot(
            &mut rows,
            OperationalSnapshot {
                conditions: vec![condition("online", "2026-01-01T00:03:00Z")],
                events: Vec::new(),
            },
            false,
        )
        .unwrap();
        assert_eq!(rows[0].lifecycle_state, "resolved");
        assert_eq!(resolved.edges.len(), 1);
        reconcile_snapshot(
            &mut rows,
            OperationalSnapshot {
                conditions: vec![condition("offline", "2026-01-01T00:04:00Z")],
                events: Vec::new(),
            },
            false,
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].trigger_generation, 2);
        assert_ne!(rows[1].id, id);
    }

    #[test]
    fn backfill_is_persisting_without_trigger_edge_and_keeps_legacy_id() {
        let mut rows = Vec::new();
        let mut legacy = condition("offline", "2026-01-01T00:00:00Z");
        legacy.backfilled = true;
        let result = reconcile_snapshot(
            &mut rows,
            OperationalSnapshot {
                conditions: vec![legacy],
                events: Vec::new(),
            },
            true,
        )
        .unwrap();
        assert_eq!(rows[0].lifecycle_state, "persisting");
        assert!(rows[0].backfilled);
        assert!(rows[0].public_id.starts_with("agent_status:agent:"));
        assert!(result.edges.is_empty());
    }

    #[test]
    fn bootstrap_classifies_condition_edges_by_the_persisted_cutoff() {
        let cutoff = "2026-01-01T00:00:00Z";
        let mut probes = vec![condition("offline", "2025-12-31T23:59:59Z"), {
            let mut current = condition("offline", "2026-01-01T00:00:01Z");
            current.source.natural_key = "vps-b".to_string();
            current.source.target_id = "vps-b".to_string();
            current.source.client_id = Some("vps-b".to_string());
            current
        }];
        classify_bootstrap_condition_probes(&mut probes, cutoff);
        let mut rows = Vec::new();
        let result = reconcile_snapshot(
            &mut rows,
            OperationalSnapshot {
                conditions: probes,
                events: Vec::new(),
            },
            true,
        )
        .unwrap();

        let legacy = rows.iter().find(|row| row.natural_key == "vps-a").unwrap();
        let current = rows.iter().find(|row| row.natural_key == "vps-b").unwrap();
        assert_eq!(legacy.lifecycle_state, "persisting");
        assert!(legacy.backfilled);
        assert_eq!(current.lifecycle_state, "triggered");
        assert!(!current.backfilled);
        assert_eq!(result.edges.len(), 1);
        assert_eq!(
            result.edges[0].0.event_id,
            format!("{}:triggered", current.id)
        );
    }

    #[test]
    fn unknown_refreshes_current_presentation_without_losing_confirmation() {
        let mut rows = Vec::new();
        reconcile_snapshot(
            &mut rows,
            OperationalSnapshot {
                conditions: vec![condition("offline", "2026-01-01T00:00:00Z")],
                events: Vec::new(),
            },
            false,
        )
        .unwrap();
        let confirmed_at = rows[0].last_confirmed_at.clone();
        let mut unknown = condition("unknown", "2026-01-01T00:01:00Z");
        unknown.source.title = "Agent status evidence is unavailable".to_string();
        unknown.source.detail = "No authoritative status evidence is available".to_string();
        unknown.source.severity = "warning".to_string();
        reconcile_snapshot(
            &mut rows,
            OperationalSnapshot {
                conditions: vec![unknown],
                events: Vec::new(),
            },
            false,
        )
        .unwrap();

        assert_eq!(rows[0].lifecycle_state, "unknown");
        assert_eq!(rows[0].title, "Agent status evidence is unavailable");
        assert_eq!(
            rows[0].detail,
            "No authoritative status evidence is available"
        );
        assert_eq!(rows[0].severity, "warning");
        assert_eq!(rows[0].trigger_severity, "critical");
        assert_eq!(rows[0].last_confirmed_at, confirmed_at);
    }

    #[test]
    fn null_tunnel_status_boundary_allows_confirmation_and_recovery() {
        let probe = |state: ProbeState, status: &str, at: &str, backfilled: bool| ConditionProbe {
            state,
            backfilled,
            source: AlertSource {
                producer_kind: "tunnel_adapter".to_string(),
                natural_key: "plan-a:runtime-a:left".to_string(),
                severity: "critical".to_string(),
                category: "network".to_string(),
                target_kind: "tunnel".to_string(),
                target_id: "vps-a:gre-a".to_string(),
                client_id: Some("vps-a".to_string()),
                title: "Tunnel adapter status failed".to_string(),
                detail: status.to_string(),
                source_status: status.to_string(),
                evidence: json!({
                    "status_boundary_at": null,
                    "telemetry_accepted_at": at,
                    "retain_unknown_backfill": backfilled,
                }),
                observed_at: at.to_string(),
            },
        };

        for (state, status, expected_state, expected_edges) in [
            (
                ProbeState::Confirmed,
                "tunnel_adapter_degraded",
                "persisting",
                0,
            ),
            (ProbeState::Healthy, "tunnel_adapter_healthy", "resolved", 1),
        ] {
            let mut rows = Vec::new();
            reconcile_snapshot(
                &mut rows,
                OperationalSnapshot {
                    conditions: vec![probe(
                        ProbeState::Unknown,
                        "tunnel_adapter_degraded",
                        "2026-01-01T00:00:00Z",
                        true,
                    )],
                    events: Vec::new(),
                },
                true,
            )
            .unwrap();
            assert_eq!(rows[0].lifecycle_state, "unknown");
            let id = rows[0].id;

            let result = reconcile_snapshot(
                &mut rows,
                OperationalSnapshot {
                    conditions: vec![probe(state, status, "2026-01-01T00:01:00Z", false)],
                    events: Vec::new(),
                },
                false,
            )
            .unwrap();

            assert_eq!(rows[0].id, id);
            assert_eq!(rows[0].lifecycle_state, expected_state);
            assert_eq!(result.edges.len(), expected_edges);
        }
    }

    #[test]
    fn state_source_identity_ignores_subject_and_presentation_metadata() {
        let original = condition("offline", "2026-01-01T00:00:00Z");
        let mut renamed = original.clone();
        renamed.source.title = "Renamed presentation".to_string();
        renamed.source.detail = "edge-renamed currently reports offline".to_string();
        renamed.source.evidence = json!({
            "subject": {
                "client_id": "vps-a",
                "display_name": "edge-renamed",
                "tags": ["new-tag"]
            }
        });

        let original =
            policy_fact_from_source(original.source, Some(ProbeState::Confirmed)).unwrap();
        let renamed = policy_fact_from_source(renamed.source, Some(ProbeState::Confirmed)).unwrap();
        assert_eq!(original.source_event_id, renamed.source_event_id);
        assert_ne!(original.payload, renamed.payload);

        let mut recovered = condition("online", "2026-01-01T00:00:00Z");
        recovered.source.evidence = json!({
            "subject": {
                "client_id": "vps-a",
                "display_name": "edge-renamed",
                "tags": ["new-tag"]
            }
        });
        let recovered =
            policy_fact_from_source(recovered.source, Some(ProbeState::Healthy)).unwrap();
        assert_ne!(original.source_event_id, recovered.source_event_id);
    }
}
