use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{postgres::PgRow, Row};
use uuid::Uuid;
use vpsman_common::{
    payload_hash, tunnel_runtime_evidence_identity_hash, tunnel_topology_identity_hash,
    RuntimeTunnelManager,
};

use crate::{
    model::{
        AuditLogView, AuthContext, FleetAlertLifecycleView, FleetAlertQuery, FleetAlertView,
        OperationalAlertEpisodeRecord,
    },
    model_alert_notifications::FleetAlertNotificationMatchRule,
    model_webhook_rules::WebhookEventCandidate,
    repository::Repository,
    repository_webhook_rules::{record_webhook_event_in_tx, webhook_event_row},
    util::{compare_timestamps_desc, parse_timestamp_utc, timestamp_in_optional_bounds},
};

pub(crate) const OPERATIONAL_ALERT_SOURCE_LIMIT: usize = 201;
const OPERATIONAL_RECONCILE_LOCK: &str = "vpsman:operational-alert-reconcile";
const LEGACY_EVENT_SOURCE_HORIZON: usize = 200;
const CONDITION_REPAIR_CLIENT_BATCH: usize = 200;
const TRIGGERED_EVENT_REPAIR_BATCH: usize = 200;

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
    record_kind: String,
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
    resolution_reason: &'static str,
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
                FROM operational_alert_episodes
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
            Self::Memory(memory) => {
                let _mutation = memory.operational_alert_mutation.lock().await;
                let bootstrapping = !*memory.operational_alert_bootstrapped.read().await;
                let event_source_cutoff_at = {
                    let mut cutoff = memory
                        .operational_alert_event_source_cutoff_at
                        .write()
                        .await;
                    cutoff
                        .get_or_insert_with(|| {
                            parse_timestamp_utc(&crate::unix_now().to_string())
                                .expect("unix clock must produce a valid operational cutoff")
                                .to_rfc3339()
                        })
                        .clone()
                };
                let condition_client_ids = if bootstrapping {
                    None
                } else {
                    Some(next_memory_condition_client_batch(memory).await)
                };
                let snapshot = load_memory_snapshot(
                    memory,
                    bootstrapping,
                    &event_source_cutoff_at,
                    condition_client_ids.as_deref(),
                )
                .await;
                let mut episodes = memory.operational_alert_episodes.write().await;
                let result = if let Some(condition_client_ids) = condition_client_ids.as_ref() {
                    let condition_client_ids = condition_client_ids
                        .iter()
                        .map(String::as_str)
                        .collect::<HashSet<_>>();
                    reconcile_scoped_snapshot(&mut episodes, snapshot, &condition_client_ids)?
                } else {
                    reconcile_snapshot(&mut episodes, snapshot, bootstrapping)?
                };
                drop(episodes);
                append_memory_webhook_edges(memory, result.edges).await?;
                if bootstrapping {
                    *memory.operational_alert_bootstrapped.write().await = true;
                }
                Ok(())
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
                    .bind(OPERATIONAL_RECONCILE_LOCK)
                    .execute(&mut *tx)
                    .await?;
                let meta = sqlx::query(
                    r#"
                    SELECT NOT backfill_completed AS bootstrapping,
                           event_source_cutoff_at,
                           condition_client_cursor
                    FROM operational_alert_lifecycle_meta
                    WHERE singleton
                    FOR UPDATE
                    "#,
                )
                .fetch_one(&mut *tx)
                .await?;
                let bootstrapping: bool = meta.try_get("bootstrapping")?;
                let event_source_cutoff_at: DateTime<Utc> =
                    meta.try_get("event_source_cutoff_at")?;
                let condition_client_ids = if bootstrapping {
                    None
                } else {
                    Some(
                        next_postgres_condition_client_batch(
                            &mut tx,
                            meta.try_get("condition_client_cursor")?,
                        )
                        .await?,
                    )
                };
                let snapshot = load_postgres_snapshot(
                    &mut tx,
                    bootstrapping,
                    event_source_cutoff_at,
                    condition_client_ids.as_deref(),
                )
                .await?;
                let rows = sqlx::query(&operational_episode_select_sql(
                    r#"
                    WHERE $1::boolean
                       OR (
                            e.record_kind = 'condition'
                            AND e.client_id = ANY($2::text[])
                            AND (
                                e.resolved_at IS NULL
                                OR NOT EXISTS (
                                    SELECT 1
                                    FROM operational_alert_episodes latest
                                    WHERE latest.record_kind = 'condition'
                                      AND latest.producer_kind = e.producer_kind
                                      AND latest.natural_key = e.natural_key
                                      AND latest.trigger_generation > e.trigger_generation
                                )
                            )
                       )
                       OR (
                            e.id IN (
                                SELECT triggered.id
                                FROM operational_alert_episodes triggered
                                WHERE triggered.record_kind = 'event'
                                  AND triggered.resolved_at IS NULL
                                  AND triggered.lifecycle_state = 'triggered'
                                ORDER BY triggered.triggered_at, triggered.id
                                LIMIT $3
                            )
                       )
                    FOR UPDATE
                    "#,
                ))
                .bind(bootstrapping)
                .bind(condition_client_ids.as_deref().unwrap_or(&[]))
                .bind(TRIGGERED_EVENT_REPAIR_BATCH as i64)
                .fetch_all(&mut *tx)
                .await?;
                let mut episodes = rows
                    .into_iter()
                    .map(operational_episode_from_row)
                    .collect::<Result<Vec<_>>>()?;
                let result = if let Some(client_ids) = condition_client_ids.as_ref() {
                    let scope = client_ids
                        .iter()
                        .map(String::as_str)
                        .collect::<HashSet<_>>();
                    reconcile_scoped_snapshot(&mut episodes, snapshot, &scope)?
                } else {
                    reconcile_snapshot(&mut episodes, snapshot, bootstrapping)?
                };
                for episode in &result.changed {
                    upsert_operational_episode_in_tx(&mut tx, episode).await?;
                }
                for (event, occurred_at) in result.edges {
                    record_webhook_event_in_tx(&mut tx, event, occurred_at).await?;
                }
                if bootstrapping {
                    sqlx::query(
                        r#"
                        UPDATE operational_alert_lifecycle_meta
                        SET backfill_completed = TRUE, completed_at = now()
                        WHERE singleton AND NOT backfill_completed
                        "#,
                    )
                    .execute(&mut *tx)
                    .await?;
                }
                tx.commit().await?;
                Ok(())
            }
        }
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
        let limit = limit.clamp(1, OPERATIONAL_ALERT_SOURCE_LIMIT);
        match self {
            Self::Memory(memory) => {
                let states = memory.fleet_alert_states.read().await;
                let now = crate::unix_now() as i64;
                let mut rows = memory
                    .operational_alert_episodes
                    .read()
                    .await
                    .iter()
                    .filter(|row| include_resolved || row.resolved_at.is_none())
                    .filter(|row| record_kind.is_none_or(|kind| row.record_kind == kind))
                    .filter(|row| {
                        !confirmed_active_only
                            || matches!(row.lifecycle_state.as_str(), "triggered" | "persisting")
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
            Self::Postgres(pool) => {
                let sql = format!(
                    r#"
                    {}
                    LEFT JOIN fleet_alert_states triage
                      ON triage.alert_id = e.public_id
                    WHERE ($1::boolean OR resolved_at IS NULL)
                      AND ($2::text IS NULL OR client_id = $2)
                      AND ($3::text IS NULL OR severity = $3)
                      AND ($4::text IS NULL OR category = $4)
                      AND (NOT $5::boolean OR lifecycle_state IN ('triggered', 'persisting'))
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
        let limit = limit.clamp(1, OPERATIONAL_ALERT_SOURCE_LIMIT);
        match self {
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
        let public_id = public_id.trim();
        let reason = reason.trim();
        anyhow::ensure!(!public_id.is_empty(), "fleet_alert_id_required");
        anyhow::ensure!(
            reason.len() <= 1024 && !reason.is_empty(),
            "fleet_alert_resolution_reason_invalid"
        );
        match self {
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
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
                    .bind(format!("vpsman:operational-alert:{public_id}"))
                    .execute(&mut *tx)
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
                let mut episode = operational_episode_from_row(row)?;
                anyhow::ensure!(
                    episode.record_kind == "event",
                    "fleet_alert_condition_not_operator_resolvable"
                );
                if episode.resolved_at.is_some() {
                    tx.commit().await?;
                    return Ok(episode);
                }
                let resolved_at = causal_now(&episode);
                resolve_episode(
                    &mut episode,
                    &resolved_at,
                    "operator_resolved",
                    Some(reason.to_string()),
                    Some(operator.operator.id),
                );
                upsert_operational_episode_in_tx(&mut tx, &episode).await?;
                record_webhook_event_in_tx(
                    &mut tx,
                    operational_lifecycle_event(&episode, false),
                    parse_episode_time(&resolved_at)?,
                )
                .await?;
                insert_operational_resolution_audit_in_tx(&mut tx, &episode, operator, reason)
                    .await?;
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
        self.reconcile_memory_agent_alert_transition_inner(client_id, to_status, observed_at, false)
            .await
    }

    pub(crate) async fn reconcile_memory_client_status_alert_transition(
        &self,
        client_id: &str,
        to_status: &str,
        observed_at: &str,
    ) -> Result<()> {
        self.reconcile_memory_agent_alert_transition_inner(client_id, to_status, observed_at, true)
            .await
    }

    async fn reconcile_memory_agent_alert_transition_inner(
        &self,
        client_id: &str,
        to_status: &str,
        observed_at: &str,
        mark_tunnels_unknown: bool,
    ) -> Result<()> {
        let Self::Memory(memory) = self else {
            return Ok(());
        };
        let _mutation = memory.operational_alert_mutation.lock().await;
        let hidden = memory.hidden_clients.read().await.contains(client_id);
        let agent = memory
            .agents
            .read()
            .await
            .iter()
            .find(|agent| agent.id == client_id)
            .cloned();
        if !hidden
            && to_status != "deleted"
            && agent
                .as_ref()
                .is_some_and(|agent| agent.status != to_status)
        {
            return Ok(());
        }
        let probes = if hidden || to_status == "deleted" {
            Vec::new()
        } else if let Some(agent) = agent {
            agent_probes(
                client_id,
                &agent.display_name,
                &agent.status,
                &agent.tags,
                observed_at,
                json!({"capability_privilege_mode": agent.capabilities.privilege_mode}),
                false,
            )
        } else {
            Vec::new()
        };
        let tunnel_probes = if mark_tunnels_unknown {
            let status_boundary_at = canonical_source_time(observed_at)?;
            memory
                .operational_alert_tunnel_boundaries
                .write()
                .await
                .insert(client_id.to_string(), status_boundary_at.clone());
            let plans = memory.tunnel_plans.read().await.clone();
            let tunnels = memory.telemetry_tunnels.read().await.clone();
            let agents = memory.agents.read().await.clone();
            let hidden = memory.hidden_clients.read().await.clone();
            let tunnel_boundaries = memory
                .operational_alert_tunnel_boundaries
                .read()
                .await
                .clone();
            let plan_boundaries = memory
                .operational_alert_tunnel_plan_boundaries
                .read()
                .await
                .clone();
            let mut snapshot = OperationalSnapshot::default();
            append_tunnel_probes(
                &mut snapshot,
                &plans,
                &tunnels,
                &agents,
                &hidden,
                &tunnel_boundaries,
                &plan_boundaries,
                TunnelEvidenceMode::Unavailable,
            );
            snapshot
                .conditions
                .retain(|probe| probe.source.client_id.as_deref() == Some(client_id));
            for probe in &mut snapshot.conditions {
                probe.source.observed_at = status_boundary_at.clone();
                if let Some(evidence) = probe.source.evidence.as_object_mut() {
                    evidence.insert("status_boundary_at".to_string(), json!(status_boundary_at));
                }
            }
            snapshot.conditions
        } else {
            Vec::new()
        };
        let mut episodes = memory.operational_alert_episodes.write().await;
        let result = reconcile_condition_scope(
            &mut episodes,
            probes,
            |episode| {
                episode.client_id.as_deref() == Some(client_id)
                    && matches!(
                        episode.producer_kind.as_str(),
                        "agent_status" | "agent_access"
                    )
            },
            false,
        )?;
        let mut edges = result.edges;
        if mark_tunnels_unknown {
            let tunnel_result = reconcile_condition_scope(
                &mut episodes,
                tunnel_probes,
                |episode| {
                    episode.client_id.as_deref() == Some(client_id)
                        && matches!(
                            episode.producer_kind.as_str(),
                            "tunnel_adapter" | "tunnel_traffic"
                        )
                },
                false,
            )?;
            edges.extend(tunnel_result.edges);
        }
        drop(episodes);
        append_memory_webhook_edges(memory, edges).await
    }

    pub(crate) async fn reconcile_memory_tunnel_alerts_for_clients(
        &self,
        client_ids: &[String],
    ) -> Result<()> {
        self.reconcile_memory_tunnel_alerts_for_clients_with_evidence(
            client_ids,
            TunnelEvidenceMode::Exact,
            None,
        )
        .await
    }

    pub(crate) async fn mark_memory_tunnel_alerts_unknown_for_clients(
        &self,
        client_ids: &[String],
        status_boundary_at: &str,
    ) -> Result<()> {
        self.reconcile_memory_tunnel_alerts_for_clients_with_evidence(
            client_ids,
            TunnelEvidenceMode::Unavailable,
            Some(canonical_source_time(status_boundary_at)?),
        )
        .await
    }

    async fn reconcile_memory_tunnel_alerts_for_clients_with_evidence(
        &self,
        client_ids: &[String],
        evidence_mode: TunnelEvidenceMode,
        status_boundary_at: Option<String>,
    ) -> Result<()> {
        let Self::Memory(memory) = self else {
            return Ok(());
        };
        if client_ids.is_empty() {
            return Ok(());
        }
        let _mutation = memory.operational_alert_mutation.lock().await;
        if let Some(status_boundary_at) = status_boundary_at.as_ref() {
            let mut boundaries = memory.operational_alert_tunnel_boundaries.write().await;
            for client_id in client_ids {
                boundaries.insert(client_id.clone(), status_boundary_at.clone());
            }
        }
        let plans = memory.tunnel_plans.read().await.clone();
        let tunnels = memory.telemetry_tunnels.read().await.clone();
        let agents = memory.agents.read().await.clone();
        let hidden = memory.hidden_clients.read().await.clone();
        let tunnel_boundaries = memory
            .operational_alert_tunnel_boundaries
            .read()
            .await
            .clone();
        let plan_boundaries = memory
            .operational_alert_tunnel_plan_boundaries
            .read()
            .await
            .clone();
        let ids = client_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let mut snapshot = OperationalSnapshot::default();
        append_tunnel_probes(
            &mut snapshot,
            &plans,
            &tunnels,
            &agents,
            &hidden,
            &tunnel_boundaries,
            &plan_boundaries,
            evidence_mode,
        );
        if let Some(status_boundary_at) = status_boundary_at {
            for probe in &mut snapshot.conditions {
                probe.source.observed_at = status_boundary_at.clone();
                if let Some(evidence) = probe.source.evidence.as_object_mut() {
                    evidence.insert("status_boundary_at".to_string(), json!(status_boundary_at));
                }
            }
        }
        snapshot.conditions.retain(|probe| {
            probe
                .source
                .client_id
                .as_deref()
                .is_some_and(|client_id| ids.contains(client_id))
        });
        let mut episodes = memory.operational_alert_episodes.write().await;
        let result = reconcile_condition_scope(
            &mut episodes,
            snapshot.conditions,
            |episode| {
                episode
                    .client_id
                    .as_deref()
                    .is_some_and(|client_id| ids.contains(client_id))
                    && matches!(
                        episode.producer_kind.as_str(),
                        "tunnel_adapter" | "tunnel_traffic"
                    )
            },
            false,
        )?;
        drop(episodes);
        append_memory_webhook_edges(memory, result.edges).await
    }

    pub(crate) async fn reconcile_memory_job_event_sources(&self, job_id: Uuid) -> Result<()> {
        let Self::Memory(memory) = self else {
            return Ok(());
        };
        let Some(job) = memory
            .jobs
            .read()
            .await
            .iter()
            .find(|job| job.id == job_id)
            .cloned()
        else {
            return Ok(());
        };
        let agents = memory.agents.read().await.clone();
        let targets = memory
            .job_targets
            .read()
            .await
            .iter()
            .filter(|target| target.job_id == job_id && target.status == "skipped")
            .cloned()
            .collect::<Vec<_>>();
        let capability = memory.capability_degraded_job_targets.read().await.clone();
        let mut sources = job_event_source(&job).into_iter().collect::<Vec<_>>();
        for target in targets {
            let Some((reason, hint)) = capability.get(&(job_id, target.client_id.clone())) else {
                continue;
            };
            let agent = agents.iter().find(|agent| agent.id == target.client_id);
            sources.push(capability_event_source(&job, &target, reason, hint, agent));
        }
        reconcile_memory_event_sources(memory, sources).await
    }

    pub(crate) async fn reconcile_memory_backup_event_source(
        &self,
        backup_id: Uuid,
        terminal_at: &str,
    ) -> Result<()> {
        let Self::Memory(memory) = self else {
            return Ok(());
        };
        let Some(backup) = memory
            .backup_requests
            .read()
            .await
            .iter()
            .find(|request| request.id == backup_id && request.status == "execution_failed")
            .cloned()
        else {
            return Ok(());
        };
        let agents = memory.agents.read().await;
        let agent = agents.iter().find(|agent| agent.id == backup.client_id);
        let mut source = backup_event_source(&backup, agent);
        if let Some(evidence) = source.evidence.as_object_mut() {
            evidence.insert("request_created_at".to_string(), json!(backup.created_at));
        }
        source.observed_at = canonical_source_time(terminal_at)?;
        drop(agents);
        reconcile_memory_event_sources(memory, vec![source]).await
    }
}

async fn next_memory_condition_client_batch(
    memory: &crate::repository::MemoryState,
) -> Vec<String> {
    let hidden = memory.hidden_clients.read().await;
    let agents = memory.agents.read().await;
    let episodes = memory.operational_alert_episodes.read().await;
    let mut clients = agents
        .iter()
        .filter(|agent| !hidden.contains(&agent.id))
        .map(|agent| agent.id.clone())
        .chain(
            episodes
                .iter()
                .filter(|episode| {
                    episode.record_kind == "condition" && episode.resolved_at.is_none()
                })
                .filter_map(|episode| episode.client_id.clone()),
        )
        .collect::<Vec<_>>();
    clients.sort();
    clients.dedup();
    drop(episodes);
    drop(agents);
    drop(hidden);

    let mut cursor = memory
        .operational_alert_condition_client_cursor
        .write()
        .await;
    let start = cursor
        .as_deref()
        .map(|cursor| clients.partition_point(|client_id| client_id.as_str() <= cursor))
        .unwrap_or(0);
    let start = if start == clients.len() && cursor.is_some() {
        0
    } else {
        start
    };
    let end = (start + CONDITION_REPAIR_CLIENT_BATCH).min(clients.len());
    let batch = clients[start..end].to_vec();
    *cursor = (end < clients.len())
        .then(|| batch.last().cloned())
        .flatten();
    batch
}

pub(crate) async fn reconcile_postgres_agent_alert_transition_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    to_status: &str,
) -> Result<()> {
    let observed_at = postgres_lifecycle_clock(tx).await?;
    reconcile_postgres_agent_alert_transition_at_in_tx(tx, client_id, to_status, observed_at).await
}

async fn reconcile_postgres_agent_alert_transition_at_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    to_status: &str,
    observed_at: DateTime<Utc>,
) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(OPERATIONAL_RECONCILE_LOCK)
        .execute(&mut **tx)
        .await?;
    let identity = sqlx::query(
        r#"
        SELECT c.display_name, c.status, c.capabilities,
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
    let sql = format!(
        r#"
        {}
        WHERE e.client_id = $1
          AND e.producer_kind IN ('agent_status', 'agent_access')
        FOR UPDATE
        "#,
        operational_episode_select_sql("")
    );
    let rows = sqlx::query(&sql)
        .bind(client_id)
        .fetch_all(&mut **tx)
        .await?;
    let mut episodes = rows
        .into_iter()
        .map(operational_episode_from_row)
        .collect::<Result<Vec<_>>>()?;
    let result = reconcile_condition_scope(&mut episodes, probes, |_| true, false)?;
    persist_reconcile_result_in_tx(tx, result).await
}

pub(crate) async fn reconcile_postgres_job_event_sources_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job_id: Uuid,
) -> Result<()> {
    let Some(job) = sqlx::query(
        r#"
        SELECT id, command_type, status, target_count,
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
            record_kind: "event".to_string(),
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
            record_kind: "event".to_string(),
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
        record_kind: "event".to_string(),
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

async fn postgres_lifecycle_clock(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<DateTime<Utc>> {
    Ok(sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **tx)
        .await?)
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
    let sql = format!(
        r#"
        {}
        WHERE e.client_id = ANY($1::text[])
          AND e.producer_kind IN ('tunnel_adapter', 'tunnel_traffic')
        FOR UPDATE
        "#,
        operational_episode_select_sql("")
    );
    let rows = sqlx::query(&sql)
        .bind(client_ids)
        .fetch_all(&mut **tx)
        .await?;
    let mut episodes = rows
        .into_iter()
        .map(operational_episode_from_row)
        .collect::<Result<Vec<_>>>()?;
    let ids = client_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let result = reconcile_condition_scope(
        &mut episodes,
        probes,
        |episode| {
            episode
                .client_id
                .as_deref()
                .is_some_and(|client_id| ids.contains(client_id))
        },
        false,
    )?;
    persist_reconcile_result_in_tx(tx, result).await
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
        state_reason: None,
        state_actor_id: None,
        state_updated_at: None,
    }
}

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

fn reconcile_scoped_snapshot(
    episodes: &mut Vec<OperationalAlertEpisodeRecord>,
    snapshot: OperationalSnapshot,
    condition_client_ids: &HashSet<&str>,
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
            && episode
                .client_id
                .as_deref()
                .is_some_and(|client_id| condition_client_ids.contains(client_id))
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

fn reconcile_condition_scope<F>(
    episodes: &mut Vec<OperationalAlertEpisodeRecord>,
    probes: Vec<ConditionProbe>,
    in_scope: F,
    _bootstrapping: bool,
) -> Result<ReconcileResult>
where
    F: Fn(&OperationalAlertEpisodeRecord) -> bool,
{
    let keys = probes
        .iter()
        .map(|probe| {
            (
                probe.source.producer_kind.clone(),
                probe.source.natural_key.clone(),
            )
        })
        .collect::<HashSet<_>>();
    let mut result = ReconcileResult::default();
    for probe in probes {
        reconcile_condition_probe(episodes, probe, &mut result)?;
    }
    let now = now_string();
    for episode in episodes.iter_mut().filter(|episode| {
        episode.record_kind == "condition"
            && episode.resolved_at.is_none()
            && in_scope(episode)
            && !keys.contains(&(episode.producer_kind.clone(), episode.natural_key.clone()))
    }) {
        let resolved_at = causal_resolution_time(episode, &now);
        resolve_episode(episode, &resolved_at, "source_scope_exited", None, None);
        result.changed.push(episode.clone());
        result.edges.push((
            operational_lifecycle_event(episode, false),
            parse_episode_time(&resolved_at)?,
        ));
    }
    Ok(result)
}

async fn persist_reconcile_result_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    result: ReconcileResult,
) -> Result<()> {
    for episode in &result.changed {
        upsert_operational_episode_in_tx(tx, episode).await?;
    }
    for (event, occurred_at) in result.edges {
        record_webhook_event_in_tx(tx, event, occurred_at).await?;
    }
    Ok(())
}

async fn reconcile_postgres_event_sources_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    mut sources: Vec<AlertSource>,
) -> Result<()> {
    let event_source_cutoff_at: DateTime<Utc> = sqlx::query_scalar(
        "SELECT event_source_cutoff_at FROM operational_alert_lifecycle_meta WHERE singleton",
    )
    .fetch_one(&mut **tx)
    .await?;
    let mut post_cutoff_sources = Vec::with_capacity(sources.len());
    for source in sources {
        let observed_at = parse_episode_time(&source.observed_at)?;
        if observed_at >= event_source_cutoff_at {
            post_cutoff_sources.push(source);
        }
    }
    sources = post_cutoff_sources;
    if sources.is_empty() {
        return Ok(());
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(OPERATIONAL_RECONCILE_LOCK)
        .execute(&mut **tx)
        .await?;
    let producer_kinds = sources
        .iter()
        .map(|source| source.producer_kind.clone())
        .collect::<Vec<_>>();
    let natural_keys = sources
        .iter()
        .map(|source| source.natural_key.clone())
        .collect::<Vec<_>>();
    let sql = format!(
        r#"
        {}
        WHERE e.record_kind = 'event'
          AND e.producer_kind = ANY($1::text[])
          AND e.natural_key = ANY($2::text[])
        FOR UPDATE
        "#,
        operational_episode_select_sql("")
    );
    let rows = sqlx::query(&sql)
        .bind(producer_kinds)
        .bind(natural_keys)
        .fetch_all(&mut **tx)
        .await?;
    let mut episodes = rows
        .into_iter()
        .map(operational_episode_from_row)
        .collect::<Result<Vec<_>>>()?;
    let mut result = ReconcileResult::default();
    reconcile_new_event_sources(
        &mut episodes,
        sources
            .into_iter()
            .map(|source| EventSource {
                source,
                backfilled: false,
            })
            .collect(),
        &mut result,
    )?;
    persist_reconcile_result_in_tx(tx, result).await
}

async fn reconcile_memory_event_sources(
    memory: &crate::repository::MemoryState,
    mut sources: Vec<AlertSource>,
) -> Result<()> {
    if sources.is_empty() {
        return Ok(());
    }
    let _mutation = memory.operational_alert_mutation.lock().await;
    for source in &mut sources {
        source.observed_at = canonical_source_time(&source.observed_at)?;
    }
    let source_floor = sources
        .iter()
        .filter_map(|source| parse_timestamp_utc(&source.observed_at))
        .min()
        .context("operational event source timestamp is invalid")?;
    let cutoff = {
        let mut cutoff = memory
            .operational_alert_event_source_cutoff_at
            .write()
            .await;
        let cutoff = cutoff.get_or_insert_with(|| source_floor.to_rfc3339());
        parse_episode_time(cutoff)?
    };
    sources.retain(|source| {
        parse_timestamp_utc(&source.observed_at).is_some_and(|observed_at| observed_at >= cutoff)
    });
    if sources.is_empty() {
        return Ok(());
    }
    let mut episodes = memory.operational_alert_episodes.write().await;
    let mut result = ReconcileResult::default();
    reconcile_new_event_sources(
        &mut episodes,
        sources
            .into_iter()
            .map(|source| EventSource {
                source,
                backfilled: false,
            })
            .collect(),
        &mut result,
    )?;
    drop(episodes);
    append_memory_webhook_edges(memory, result.edges).await
}

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
            resolve_episode(episode, &resolved_at, probe.resolution_reason, None, None);
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

#[derive(Clone, Debug, Eq, PartialEq)]
enum TunnelEvidenceBoundaryDecision {
    Current,
    Stale(String),
    Unverifiable,
}

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
        record_kind: source.record_kind.clone(),
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

fn operational_lifecycle_event(
    episode: &OperationalAlertEpisodeRecord,
    triggered: bool,
) -> WebhookEventCandidate {
    let state = if triggered { "triggered" } else { "resolved" };
    let kind = format!("alert.{state}");
    let mut predicates = vec![
        kind.clone(),
        format!("alert.category:{}", episode.trigger_category),
        format!("alert.severity:{}", episode.trigger_severity),
    ];
    if triggered {
        predicates.push("alert.open".to_string());
    }
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
        FROM operational_alert_episodes e
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

async fn upsert_operational_episode_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    episode: &OperationalAlertEpisodeRecord,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO operational_alert_episodes (
            id, public_id, producer_kind, natural_key, record_kind,
            trigger_generation, trigger_severity, trigger_category,
            severity, category, target_kind, target_id,
            client_id, title, detail, source_status, evidence, lifecycle_state,
            triggered_at, last_confirmed_at, resolved_at, resolution_reason,
            resolution_note, resolution_actor_id, backfilled, created_at, updated_at
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
            $11, $12, $13, $14, $15, $16, $17, $18,
            $19::timestamptz, $20::timestamptz, $21::timestamptz,
            $22, $23, $24, $25, $26::timestamptz, $27::timestamptz
        )
        ON CONFLICT (id) DO UPDATE SET
            public_id = EXCLUDED.public_id,
            severity = EXCLUDED.severity,
            category = EXCLUDED.category,
            target_kind = EXCLUDED.target_kind,
            target_id = EXCLUDED.target_id,
            client_id = EXCLUDED.client_id,
            title = EXCLUDED.title,
            detail = EXCLUDED.detail,
            source_status = EXCLUDED.source_status,
            evidence = EXCLUDED.evidence,
            lifecycle_state = EXCLUDED.lifecycle_state,
            last_confirmed_at = EXCLUDED.last_confirmed_at,
            resolved_at = EXCLUDED.resolved_at,
            resolution_reason = EXCLUDED.resolution_reason,
            resolution_note = EXCLUDED.resolution_note,
            resolution_actor_id = EXCLUDED.resolution_actor_id,
            updated_at = EXCLUDED.updated_at
        "#,
    )
    .bind(episode.id)
    .bind(&episode.public_id)
    .bind(&episode.producer_kind)
    .bind(&episode.natural_key)
    .bind(&episode.record_kind)
    .bind(episode.trigger_generation)
    .bind(&episode.trigger_severity)
    .bind(&episode.trigger_category)
    .bind(&episode.severity)
    .bind(&episode.category)
    .bind(&episode.target_kind)
    .bind(&episode.target_id)
    .bind(&episode.client_id)
    .bind(&episode.title)
    .bind(&episode.detail)
    .bind(&episode.source_status)
    .bind(&episode.evidence)
    .bind(&episode.lifecycle_state)
    .bind(&episode.triggered_at)
    .bind(&episode.last_confirmed_at)
    .bind(&episode.resolved_at)
    .bind(&episode.resolution_reason)
    .bind(&episode.resolution_note)
    .bind(episode.resolution_actor_id)
    .bind(episode.backfilled)
    .bind(&episode.created_at)
    .bind(&episode.updated_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

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

fn record_kind_rank(kind: &str) -> usize {
    usize::from(kind != "condition")
}

fn lifecycle_rank(state: &str) -> usize {
    match state {
        "triggered" | "persisting" => 0,
        "unknown" => 1,
        _ => 2,
    }
}

fn severity_rank(severity: &str) -> usize {
    match severity {
        "critical" => 0,
        "warning" => 1,
        _ => 2,
    }
}

fn now_string() -> String {
    Utc::now().to_rfc3339()
}

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

fn memory_event_candidates<F>(
    sources: Vec<AlertSource>,
    bootstrapping: bool,
    cutoff_at: &str,
    existing_keys: &HashSet<(String, String)>,
    mut legacy_order: F,
) -> Vec<EventSource>
where
    F: FnMut(&AlertSource, &AlertSource) -> std::cmp::Ordering,
{
    let mut result = Vec::new();
    if bootstrapping {
        let mut legacy = sources
            .iter()
            .filter(|source| timestamp_is_before(&source.observed_at, cutoff_at))
            .cloned()
            .collect::<Vec<_>>();
        legacy.sort_by(&mut legacy_order);
        legacy.truncate(LEGACY_EVENT_SOURCE_HORIZON);
        result.extend(legacy.into_iter().map(|source| EventSource {
            source,
            backfilled: true,
        }));
    }
    let mut current = sources
        .into_iter()
        .filter(|source| {
            !timestamp_is_before(&source.observed_at, cutoff_at)
                && !existing_keys
                    .contains(&(source.producer_kind.clone(), source.natural_key.clone()))
        })
        .collect::<Vec<_>>();
    current.sort_by(|left, right| {
        parse_timestamp_utc(&left.observed_at)
            .cmp(&parse_timestamp_utc(&right.observed_at))
            .then_with(|| left.target_id.cmp(&right.target_id))
    });
    current.truncate(LEGACY_EVENT_SOURCE_HORIZON);
    result.extend(current.into_iter().map(|source| EventSource {
        source,
        backfilled: false,
    }));
    result
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

fn causal_resolution_time(episode: &OperationalAlertEpisodeRecord, candidate: &str) -> String {
    let confirmed = episode.last_confirmed_at.as_deref();
    max_time_string(confirmed, &max_time_string(Some(candidate), &now_string()))
}

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

async fn load_memory_snapshot(
    memory: &crate::repository::MemoryState,
    bootstrapping: bool,
    event_source_cutoff_at: &str,
    condition_client_ids: Option<&[String]>,
) -> OperationalSnapshot {
    let agents = memory.agents.read().await.clone();
    let hidden = memory.hidden_clients.read().await.clone();
    let histories = memory.client_status_history.read().await.clone();
    let plans = memory.tunnel_plans.read().await.clone();
    let tunnels = memory.telemetry_tunnels.read().await.clone();
    let tunnel_boundaries = memory
        .operational_alert_tunnel_boundaries
        .read()
        .await
        .clone();
    if bootstrapping {
        let mut plan_boundaries = memory
            .operational_alert_tunnel_plan_boundaries
            .write()
            .await;
        for plan in &plans {
            plan_boundaries
                .entry(plan.id)
                .or_insert_with(|| plan.updated_at.clone());
        }
    }
    let plan_boundaries = memory
        .operational_alert_tunnel_plan_boundaries
        .read()
        .await
        .clone();
    let jobs = memory.jobs.read().await.clone();
    let backups = memory.backup_requests.read().await.clone();
    let targets = memory.job_targets.read().await.clone();
    let capability = memory.capability_degraded_job_targets.read().await.clone();
    let audits = memory.audits.read().await.clone();
    let existing_event_keys = memory
        .operational_alert_episodes
        .read()
        .await
        .iter()
        .filter(|episode| episode.record_kind == "event")
        .map(|episode| (episode.producer_kind.clone(), episode.natural_key.clone()))
        .collect::<HashSet<_>>();
    let mut snapshot = OperationalSnapshot::default();

    for agent in agents.iter().filter(|agent| {
        !hidden.contains(&agent.id)
            && condition_client_ids.is_none_or(|client_ids| client_ids.contains(&agent.id))
    }) {
        let observed_at = histories
            .iter()
            .filter(|history| history.client_id == agent.id && history.to_status == agent.status)
            .max_by(|left, right| compare_timestamps_desc(&right.created_at, &left.created_at))
            .map(|history| history.created_at.clone())
            .or_else(|| agent.last_seen_at.clone())
            .unwrap_or_else(now_string);
        snapshot.conditions.extend(agent_probes(
            &agent.id,
            &agent.display_name,
            &agent.status,
            &agent.tags,
            &observed_at,
            json!({"capability_privilege_mode": agent.capabilities.privilege_mode}),
            bootstrapping && timestamp_is_before(&observed_at, event_source_cutoff_at),
        ));
    }

    let condition_plans = plans
        .iter()
        .filter(|plan| {
            condition_client_ids.is_none_or(|client_ids| {
                client_ids.contains(&plan.left_client_id)
                    || client_ids.contains(&plan.right_client_id)
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    append_tunnel_probes(
        &mut snapshot,
        &condition_plans,
        &tunnels,
        &agents,
        &hidden,
        &tunnel_boundaries,
        &plan_boundaries,
        if bootstrapping {
            TunnelEvidenceMode::LegacyBootstrap
        } else {
            TunnelEvidenceMode::Exact
        },
    );
    if let Some(condition_client_ids) = condition_client_ids {
        snapshot.conditions.retain(|probe| {
            probe
                .source
                .client_id
                .as_ref()
                .is_some_and(|client_id| condition_client_ids.contains(client_id))
        });
    }
    if bootstrapping {
        classify_bootstrap_condition_probes(&mut snapshot.conditions, event_source_cutoff_at);
    }
    let job_sources = jobs.iter().filter_map(job_event_source).collect::<Vec<_>>();
    snapshot.events.extend(memory_event_candidates(
        job_sources,
        bootstrapping,
        event_source_cutoff_at,
        &existing_event_keys,
        |left, right| {
            severity_rank(&left.severity)
                .cmp(&severity_rank(&right.severity))
                .then_with(|| compare_timestamps_desc(&left.observed_at, &right.observed_at))
                .then_with(|| right.target_id.cmp(&left.target_id))
        },
    ));
    let backup_sources = backups
        .iter()
        .filter(|backup| backup.status == "execution_failed")
        .map(|backup| {
            let agent = agents.iter().find(|agent| agent.id == backup.client_id);
            let mut source = backup_event_source(backup, agent);
            if let Some(audit) = audits
                .iter()
                .filter(|audit| {
                    audit.action == "backup.execution_failed"
                        && audit.target == format!("backup_request:{}", backup.id)
                })
                .max_by(|left, right| compare_timestamps_desc(&right.created_at, &left.created_at))
            {
                if let Some(evidence) = source.evidence.as_object_mut() {
                    evidence.insert("request_created_at".to_string(), json!(backup.created_at));
                }
                source.observed_at = audit.created_at.clone();
            }
            source
        })
        .collect::<Vec<_>>();
    snapshot.events.extend(memory_event_candidates(
        backup_sources,
        bootstrapping,
        event_source_cutoff_at,
        &existing_event_keys,
        |left, right| {
            compare_timestamps_desc(&left.observed_at, &right.observed_at)
                .then_with(|| right.target_id.cmp(&left.target_id))
        },
    ));
    let jobs_by_id = jobs
        .iter()
        .map(|job| (job.id, job))
        .collect::<HashMap<_, _>>();
    let mut capability_sources = Vec::new();
    for target in targets.iter().filter(|target| target.status == "skipped") {
        let Some((reason, hint)) = capability.get(&(target.job_id, target.client_id.clone()))
        else {
            continue;
        };
        let Some(job) = jobs_by_id.get(&target.job_id) else {
            continue;
        };
        let agent = agents.iter().find(|agent| agent.id == target.client_id);
        capability_sources.push(capability_event_source(job, target, reason, hint, agent));
    }
    snapshot.events.extend(memory_event_candidates(
        capability_sources,
        bootstrapping,
        event_source_cutoff_at,
        &existing_event_keys,
        |left, right| {
            let (left_job, left_client) = left.target_id.split_once(':').unwrap_or(("", ""));
            let (right_job, right_client) = right.target_id.split_once(':').unwrap_or(("", ""));
            compare_timestamps_desc(&left.observed_at, &right.observed_at)
                .then_with(|| right_job.cmp(left_job))
                .then_with(|| left_client.cmp(right_client))
        },
    ));
    snapshot
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
    let connectivity_reason = if status == "revoked" {
        "source_scope_exited"
    } else {
        "condition_recovered"
    };
    let connectivity = ConditionProbe {
        state: if connectivity_confirmed {
            ProbeState::Confirmed
        } else {
            ProbeState::Healthy
        },
        resolution_reason: connectivity_reason,
        backfilled,
        source: AlertSource {
            producer_kind: "agent_status".to_string(),
            natural_key: format!("{client_id}:connectivity"),
            record_kind: "condition".to_string(),
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
        resolution_reason: "condition_recovered",
        backfilled,
        source: AlertSource {
            producer_kind: "agent_access".to_string(),
            natural_key: format!("{client_id}:access"),
            record_kind: "condition".to_string(),
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

fn append_tunnel_probes(
    snapshot: &mut OperationalSnapshot,
    plans: &[crate::model::TunnelPlanView],
    tunnels: &[crate::model::TelemetryTunnelView],
    agents: &[crate::model::AgentView],
    hidden: &HashSet<String>,
    tunnel_boundaries: &HashMap<String, String>,
    plan_boundaries: &HashMap<Uuid, String>,
    evidence_mode: TunnelEvidenceMode,
) {
    for plan in plans
        .iter()
        .filter(|plan| plan.enabled && plan.deleted_at.is_none())
    {
        for (side, client_id, peer_client_id) in [
            ("left", &plan.left_client_id, &plan.right_client_id),
            ("right", &plan.right_client_id, &plan.left_client_id),
        ] {
            if hidden.contains(client_id) {
                continue;
            }
            let agent = agents.iter().find(|agent| &agent.id == client_id);
            let tunnel = tunnels
                .iter()
                .filter(|tunnel| {
                    tunnel.client_id == *client_id
                        && tunnel.interface == plan.plan.interface_name
                        && tunnel.plan_id == Some(plan.id)
                        && tunnel.endpoint_side.as_deref() == Some(side)
                })
                .max_by(|left, right| {
                    parse_timestamp_utc(&left.accepted_at)
                        .cmp(&parse_timestamp_utc(&right.accepted_at))
                        .then_with(|| {
                            parse_timestamp_utc(&left.observed_at)
                                .cmp(&parse_timestamp_utc(&right.observed_at))
                        })
                });
            append_tunnel_endpoint_probes(
                snapshot,
                plan,
                side,
                client_id,
                peer_client_id,
                agent,
                tunnel_boundaries.get(client_id).map(String::as_str),
                plan_boundaries
                    .get(&plan.id)
                    .map(String::as_str)
                    .or_else(|| {
                        (evidence_mode == TunnelEvidenceMode::LegacyBootstrap)
                            .then_some(plan.updated_at.as_str())
                    }),
                tunnel,
                evidence_mode,
            );
        }
    }
}

fn append_tunnel_endpoint_probes(
    snapshot: &mut OperationalSnapshot,
    plan: &crate::model::TunnelPlanView,
    side: &str,
    client_id: &str,
    peer_client_id: &str,
    agent: Option<&crate::model::AgentView>,
    status_boundary_at: Option<&str>,
    runtime_boundary_at: Option<&str>,
    tunnel: Option<&crate::model::TelemetryTunnelView>,
    evidence_mode: TunnelEvidenceMode,
) {
    let expected_topology_identity_hash = tunnel_topology_identity_hash(plan.id, &plan.plan);
    let expected_runtime_evidence_identity_hash = tunnel_runtime_evidence_identity_hash(
        plan.id,
        &plan.plan,
        plan.builtin_credentials
            .as_ref()
            .map(|value| value.generation()),
    );
    let exact_identity = tunnel.is_some_and(|tunnel| {
        tunnel.runtime_evidence_identity_hash.as_deref()
            == Some(expected_runtime_evidence_identity_hash.as_str())
            && runtime_boundary_at
                .is_some_and(|boundary| timestamp_is_after(&tunnel.accepted_at, boundary))
    });
    let legacy_identity = evidence_mode == TunnelEvidenceMode::LegacyBootstrap
        && tunnel.is_some_and(|tunnel| {
            tunnel.runtime_evidence_identity_hash.is_none()
                && runtime_boundary_at
                    .is_some_and(|boundary| timestamp_is_at_or_after(&tunnel.accepted_at, boundary))
        });
    let evidence_available = evidence_mode != TunnelEvidenceMode::Unavailable
        && agent.is_some_and(|agent| agent.status == "online")
        && status_boundary_at.is_none_or(|boundary| {
            tunnel.is_some_and(|tunnel| timestamp_is_after(&tunnel.accepted_at, boundary))
        })
        && (exact_identity || legacy_identity);
    let legacy_evidence = evidence_available && legacy_identity;
    let tunnel = evidence_available.then_some(tunnel).flatten();
    let observed_at = tunnel
        .map(|tunnel| tunnel.accepted_at.clone())
        .or_else(|| status_boundary_at.map(ToOwned::to_owned))
        .or_else(|| runtime_boundary_at.map(ToOwned::to_owned))
        .unwrap_or_else(|| plan.updated_at.clone());
    let base_evidence = json!({
        "subject": {
            "client_id": client_id,
            "display_name": agent.map(|agent| agent.display_name.as_str()).unwrap_or(client_id),
            "tags": agent.map(|agent| agent.tags.as_slice()).unwrap_or(&[]),
        },
        "plan": {
            "id": plan.id,
            "name": &plan.name,
            "revision": plan.revision,
            "topology_identity_hash": &expected_topology_identity_hash,
            "runtime_evidence_identity_hash": &expected_runtime_evidence_identity_hash,
            "endpoint_side": side,
            "peer_client_id": peer_client_id,
            "interface": &plan.plan.interface_name,
        },
        "telemetry_observed_at": tunnel.map(|tunnel| tunnel.observed_at.as_str()),
        "telemetry_accepted_at": tunnel.map(|tunnel| tunnel.accepted_at.as_str()),
        "reported_topology_identity_hash": tunnel
            .and_then(|tunnel| tunnel.topology_identity_hash.as_deref()),
        "reported_runtime_evidence_identity_hash": tunnel
            .and_then(|tunnel| tunnel.runtime_evidence_identity_hash.as_deref()),
        "status_boundary_at": status_boundary_at,
        "runtime_boundary_at": runtime_boundary_at,
        "topology_identity_validation": if legacy_evidence {
            "legacy_backfill"
        } else if tunnel.is_some() {
            "exact"
        } else {
            "unavailable"
        },
    });
    if plan.plan.runtime_control.manager == RuntimeTunnelManager::CustomAdapter {
        let (state, status, detail, adapter) =
            match tunnel.and_then(|tunnel| tunnel.adapter_health.as_ref()) {
                Some(health) if health.success => (
                    ProbeState::Healthy,
                    "tunnel_adapter_healthy",
                    "Tunnel adapter status is healthy".to_string(),
                    json!(health),
                ),
                Some(health) => (
                    ProbeState::Confirmed,
                    "tunnel_adapter_degraded",
                    health.reason.clone().unwrap_or_else(|| {
                        "adapter command did not report healthy status".to_string()
                    }),
                    json!(health),
                ),
                None => (
                    ProbeState::Unknown,
                    "tunnel_adapter_evidence_missing",
                    "Tunnel adapter health evidence is unavailable".to_string(),
                    Value::Null,
                ),
            };
        let title = if state == ProbeState::Unknown {
            "Tunnel adapter status is unavailable"
        } else {
            "Tunnel adapter status failed"
        };
        snapshot.conditions.push(ConditionProbe {
            state,
            resolution_reason: "condition_recovered",
            backfilled: legacy_evidence,
            source: tunnel_source(
                "tunnel_adapter",
                plan,
                side,
                client_id,
                "critical",
                status,
                title,
                detail,
                merge_json(base_evidence.clone(), json!({"adapter_health": adapter})),
                &observed_at,
            ),
        });
    }
    let (state, status, detail) = match tunnel.and_then(|tunnel| tunnel.traffic_status.as_deref()) {
        Some("ok") => (
            ProbeState::Healthy,
            "tunnel_traffic_ok",
            "Tunnel interface counters are healthy".to_string(),
        ),
        Some(_) => (
            ProbeState::Confirmed,
            "tunnel_traffic_degraded",
            tunnel
                .and_then(|tunnel| tunnel.traffic_reason.clone())
                .unwrap_or_else(|| "tunnel interface counters are not reporting ok".to_string()),
        ),
        None => (
            ProbeState::Unknown,
            "tunnel_traffic_evidence_missing",
            "Tunnel traffic counter evidence is unavailable".to_string(),
        ),
    };
    let title = if state == ProbeState::Unknown {
        "Tunnel traffic status is unavailable"
    } else {
        "Tunnel interface counters are degraded"
    };
    snapshot.conditions.push(ConditionProbe {
        state,
        resolution_reason: "condition_recovered",
        backfilled: legacy_evidence,
        source: tunnel_source(
            "tunnel_traffic",
            plan,
            side,
            client_id,
            "warning",
            status,
            title,
            detail,
            merge_json(
                base_evidence,
                json!({
                    "traffic_source": tunnel.and_then(|tunnel| tunnel.traffic_source.as_ref()),
                    "traffic_status": tunnel.and_then(|tunnel| tunnel.traffic_status.as_ref()),
                    "traffic_reason": tunnel.and_then(|tunnel| tunnel.traffic_reason.as_ref()),
                }),
            ),
            &observed_at,
        ),
    });
}

fn tunnel_source(
    producer_kind: &str,
    plan: &crate::model::TunnelPlanView,
    side: &str,
    client_id: &str,
    severity: &str,
    status: &str,
    title: &str,
    detail: String,
    evidence: Value,
    observed_at: &str,
) -> AlertSource {
    AlertSource {
        producer_kind: producer_kind.to_string(),
        natural_key: format!(
            "{}:{}:{side}",
            plan.id,
            tunnel_runtime_evidence_identity_hash(
                plan.id,
                &plan.plan,
                plan.builtin_credentials
                    .as_ref()
                    .map(|value| value.generation()),
            )
        ),
        record_kind: "condition".to_string(),
        severity: severity.to_string(),
        category: "network".to_string(),
        target_kind: "tunnel".to_string(),
        target_id: format!("{}:{}", client_id, plan.plan.interface_name),
        client_id: Some(client_id.to_string()),
        title: title.to_string(),
        detail,
        source_status: status.to_string(),
        evidence,
        observed_at: observed_at.to_string(),
    }
}

fn merge_json(mut base: Value, extra: Value) -> Value {
    if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
        base.extend(extra.clone());
    }
    base
}

fn job_event_source(job: &crate::model::JobHistoryView) -> Option<AlertSource> {
    if !matches!(
        job.status.as_str(),
        "partial_success"
            | "canceled"
            | "rejected"
            | "failed"
            | "agent_timeout"
            | "control_timeout"
    ) {
        return None;
    }
    let severity = if job.status == "partial_success" {
        "warning"
    } else {
        "critical"
    };
    let category = if job.command_type.contains("backup") || job.command_type.contains("restore") {
        "backup"
    } else if job.command_type.contains("agent_update") {
        "agent_update"
    } else {
        "job"
    };
    Some(AlertSource {
        producer_kind: "job".to_string(),
        natural_key: job.id.to_string(),
        record_kind: "event".to_string(),
        severity: severity.to_string(),
        category: category.to_string(),
        target_kind: "job".to_string(),
        target_id: job.id.to_string(),
        client_id: None,
        title: "Job requires operator attention".to_string(),
        detail: format!("{} job {}", job.command_type, job.status),
        source_status: job.status.clone(),
        evidence: json!({
            "job_id": job.id,
            "command_type": &job.command_type,
            "target_count": job.target_count,
            "retained_identity": true,
        }),
        observed_at: job
            .completed_at
            .clone()
            .unwrap_or_else(|| job.created_at.clone()),
    })
}

fn backup_event_source(
    backup: &crate::model::BackupRequestView,
    agent: Option<&crate::model::AgentView>,
) -> AlertSource {
    AlertSource {
        producer_kind: "backup_request".to_string(),
        natural_key: backup.id.to_string(),
        record_kind: "event".to_string(),
        severity: "critical".to_string(),
        category: "backup".to_string(),
        target_kind: "backup_request".to_string(),
        target_id: backup.id.to_string(),
        client_id: Some(backup.client_id.clone()),
        title: "Backup request failed".to_string(),
        detail: format!("backup request {} is {}", backup.id, backup.status),
        source_status: backup.status.clone(),
        evidence: merge_json(
            source_identity_evidence(
                &backup.client_id,
                agent.map(|agent| agent.display_name.as_str()),
                agent.map(|agent| agent.tags.as_slice()).unwrap_or(&[]),
            ),
            json!({
                "paths": &backup.paths,
                "include_config": backup.include_config,
                "artifact_id": backup.artifact_id,
                "retained_identity": true,
            }),
        ),
        observed_at: backup.created_at.clone(),
    }
}

fn capability_event_source(
    job: &crate::model::JobHistoryView,
    target: &crate::model::JobTargetView,
    reason: &str,
    hint: &str,
    agent: Option<&crate::model::AgentView>,
) -> AlertSource {
    AlertSource {
        producer_kind: "capability_degraded".to_string(),
        natural_key: format!("{}:{}", job.id, target.client_id),
        record_kind: "event".to_string(),
        severity: "warning".to_string(),
        category: "capability_degraded".to_string(),
        target_kind: "job_target".to_string(),
        target_id: format!("{}:{}", job.id, target.client_id),
        client_id: Some(target.client_id.clone()),
        title: "Operation skipped because the agent lacks a required capability".to_string(),
        detail: hint.to_string(),
        source_status: reason.to_string(),
        evidence: merge_json(
            source_identity_evidence(
                &target.client_id,
                agent.map(|agent| agent.display_name.as_str()),
                agent.map(|agent| agent.tags.as_slice()).unwrap_or(&[]),
            ),
            json!({
                "job_id": job.id,
                "command_type": &job.command_type,
                "target_status": &target.status,
                "target_message": &target.message,
                "reason": reason,
                "hint": hint,
                "exit_code": target.exit_code,
                "started_at": &target.started_at,
                "completed_at": &target.completed_at,
                "retained_identity": true,
            }),
        ),
        observed_at: target
            .completed_at
            .clone()
            .or_else(|| target.started_at.clone())
            .unwrap_or_else(|| job.created_at.clone()),
    }
}

async fn load_postgres_snapshot(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    bootstrapping: bool,
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
            bootstrapping && legacy_status,
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
                if bootstrapping {
                    TunnelEvidenceMode::LegacyBootstrap
                } else {
                    TunnelEvidenceMode::Exact
                },
            );
        }
    }

    append_postgres_event_sources(tx, &mut snapshot, bootstrapping, event_source_cutoff_at).await?;
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
        record_kind: "condition".to_string(),
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
            resolution_reason: "condition_recovered",
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
        resolution_reason: "condition_recovered",
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
                   FALSE AS backfilled
            FROM jobs
            WHERE status IN ('partial_success', 'canceled', 'rejected', 'failed', 'agent_timeout', 'control_timeout')
              AND alert_terminal_at >= $2
              AND NOT EXISTS (
                  SELECT 1 FROM operational_alert_episodes episode
                  WHERE episode.producer_kind = 'job'
                    AND episode.natural_key = jobs.id::text
              )
            ORDER BY alert_terminal_at ASC, id ASC
            LIMIT $3
        )
        SELECT id, command_type, status, target_count,
               alert_terminal_at::text AS alert_terminal_at,
               backfilled
        FROM legacy
        UNION ALL
        SELECT id, command_type, status, target_count,
               alert_terminal_at::text AS alert_terminal_at,
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
                record_kind: "event".to_string(),
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
                   created_at, terminal_at,
                   TRUE AS backfilled
            FROM backup_requests
            WHERE $1::boolean
              AND status = 'execution_failed'
              AND terminal_at < $2
            ORDER BY terminal_at DESC, id DESC
            LIMIT $3
        ), new_sources AS (
            SELECT id, client_id, paths, include_config, artifact_id, status,
                   created_at, terminal_at,
                   FALSE AS backfilled
            FROM backup_requests
            WHERE status = 'execution_failed'
              AND terminal_at >= $2
              AND NOT EXISTS (
                  SELECT 1 FROM operational_alert_episodes episode
                  WHERE episode.producer_kind = 'backup_request'
                    AND episode.natural_key = backup_requests.id::text
              )
            ORDER BY terminal_at ASC, id ASC
            LIMIT $3
        )
        SELECT id, client_id, paths, include_config, artifact_id,
               status, created_at::text AS created_at,
               terminal_at::text AS terminal_at, backfilled
        FROM legacy
        UNION ALL
        SELECT id, client_id, paths, include_config, artifact_id,
               status, created_at::text AS created_at,
               terminal_at::text AS terminal_at, backfilled
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
                record_kind: "event".to_string(),
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
                   j.command_type,
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
                   j.command_type,
                   FALSE AS backfilled
            FROM job_targets t
            JOIN jobs j ON j.id = t.job_id
            WHERE t.status = 'skipped'
              AND t.capability_degraded_reason IS NOT NULL
              AND t.capability_degraded_hint IS NOT NULL
              AND t.capability_alert_at >= $2
              AND NOT EXISTS (
                  SELECT 1 FROM operational_alert_episodes episode
                  WHERE episode.producer_kind = 'capability_degraded'
                    AND episode.natural_key = t.job_id::text || ':' || t.client_id
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
               command_type, backfilled
        FROM legacy
        UNION ALL
        SELECT job_id, client_id, status, message, exit_code,
               started_at::text AS started_at, completed_at::text AS completed_at,
               capability_alert_at::text AS capability_alert_at,
               capability_degraded_reason, capability_degraded_hint,
               command_type, backfilled
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
                record_kind: "event".to_string(),
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
            resolution_reason: "condition_recovered",
            backfilled: false,
            source: AlertSource {
                producer_kind: "agent_status".to_string(),
                natural_key: "vps-a".to_string(),
                record_kind: "condition".to_string(),
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
            resolution_reason: "condition_recovered",
            backfilled,
            source: AlertSource {
                producer_kind: "tunnel_adapter".to_string(),
                natural_key: "plan-a:runtime-a:left".to_string(),
                record_kind: "condition".to_string(),
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
}
