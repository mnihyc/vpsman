use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{postgres::PgRow, Postgres, QueryBuilder, Row};
use uuid::Uuid;
use vpsman_common::{
    alert_policy_state_source_event_id, alert_policy_state_source_revision_event_id,
    tunnel_runtime_evidence_identity_hash, tunnel_topology_identity_hash, RuntimeTunnelManager,
};

use crate::{
    model::{
        AuditLogView, AuthContext, FleetAlertLifecycleView, FleetAlertQuery, FleetAlertView,
        OperationalAlertEpisodeRecord,
    },
    model_alert_notifications::FleetAlertNotificationMatchRule,
    model_alert_policies::AlertPolicyRuleKind,
    model_alert_states::FleetAlertStateView,
    repository::Repository,
    repository_policy_lifecycle::{
        lock_client_policy_suppressions_shared_in_tx, lock_policy_rule_generations_shared_in_tx,
        record_policy_evidence_in_tx, record_policy_source_scope_exits_in_tx,
        resolve_policy_occurrence_episodes_prelocked_in_tx, PolicyEvidenceFact,
    },
    util::parse_timestamp_utc,
};

pub(crate) const OPERATIONAL_ALERT_SOURCE_LIMIT: usize = 201;
const OPERATIONAL_RECONCILE_CLIENT_LOCK_PREFIX: &str = "vpsman:operational-alert-reconcile:client:";

async fn lock_postgres_operational_reconcile_clients_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_ids: &[String],
) -> Result<()> {
    let mut client_ids = client_ids.to_vec();
    client_ids.sort_unstable();
    client_ids.dedup();
    // Operational evidence has one exact writer fence per subject.  The
    // current-evidence trigger uses MVCC/CAS rows and does not own the client
    // lifecycle row, so taking a client-row read lock here only couples alert materializing
    // to telemetry/status producers without adding correctness.
    for client_id in &client_ids {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text || $2::text, 0))")
            .bind(OPERATIONAL_RECONCILE_CLIENT_LOCK_PREFIX)
            .bind(client_id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
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
}

#[derive(Clone, Debug)]
struct AlertSource {
    producer_kind: String,
    natural_key: String,
    target_kind: String,
    target_id: String,
    client_id: Option<String>,
    detail: String,
    source_status: String,
    evidence: Value,
    observed_at: String,
    source_revision_token: Option<String>,
}

#[derive(Clone, Debug)]
struct ConditionProbe {
    source: AlertSource,
    state: ProbeState,
}

#[derive(Default)]
struct OperationalSnapshot {
    conditions: Vec<ConditionProbe>,
}

pub(crate) struct OperationalAlertEventSync {
    pub(crate) current: Vec<OperationalAlertEpisodeRecord>,
    pub(crate) head: Vec<OperationalAlertEpisodeRecord>,
    pub(crate) states: Vec<FleetAlertStateView>,
}

impl Repository {
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

    /// Reads the newest unresolved occurrences and revalidates every occurrence
    /// already retained by the caller from one PostgreSQL statement/snapshot.
    /// Transport pagination therefore never becomes a second lifecycle owner:
    /// a known id omitted from `current` is no longer an unresolved occurrence.
    pub(crate) async fn sync_unresolved_operational_alert_events(
        &self,
        known_public_ids: &[String],
        head_limit: usize,
    ) -> Result<OperationalAlertEventSync> {
        let head_limit = head_limit.clamp(1, OPERATIONAL_ALERT_SOURCE_LIMIT);
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH head AS MATERIALIZED (
                        SELECT e.id
                        FROM alert_episodes e
                        WHERE e.record_kind = 'event'
                          AND e.resolved_at IS NULL
                        ORDER BY e.triggered_at DESC, e.id DESC
                        LIMIT $2
                    ), selected AS (
                        SELECT head.id, true AS in_head, false AS is_known
                        FROM head
                        UNION ALL
                        SELECT e.id, false AS in_head, true AS is_known
                        FROM alert_episodes e
                        WHERE e.public_id=ANY($1::text[])
                          AND e.record_kind = 'event'
                          AND e.resolved_at IS NULL
                    ), deduplicated AS (
                        SELECT id, bool_or(in_head) AS in_head,
                               bool_or(is_known) AS is_known
                        FROM selected
                        GROUP BY id
                    )
                    SELECT
                        e.id, e.public_id, e.producer_kind, e.natural_key, e.record_kind,
                        e.trigger_generation, e.trigger_severity, e.trigger_category,
                        e.severity, e.category, e.target_kind, e.target_id,
                        e.client_id, e.title, e.detail, e.source_status, e.evidence,
                        e.lifecycle_state, e.triggered_at::text AS triggered_at,
                        e.last_confirmed_at::text AS last_confirmed_at,
                        e.resolved_at::text AS resolved_at, e.resolution_reason,
                        e.resolution_note, e.resolution_actor_id,
                        e.created_at::text AS created_at,
                        e.updated_at::text AS updated_at,
                        triage.state AS triage_state,
                        triage.muted_until_unix AS triage_muted_until_unix,
                        triage.escalation_level AS triage_escalation_level,
                        triage.revision AS triage_revision,
                        triage.reason AS triage_reason,
                        triage.actor_id AS triage_actor_id,
                        triage.created_at::text AS triage_created_at,
                        triage.updated_at::text AS triage_updated_at,
                        selected.in_head, selected.is_known
                    FROM deduplicated selected
                    JOIN alert_episodes e ON e.id=selected.id
                    LEFT JOIN fleet_alert_states triage ON triage.alert_id=e.public_id
                    ORDER BY e.triggered_at DESC, e.id DESC
                    "#,
                )
                .bind(known_public_ids)
                .bind(head_limit as i64)
                .fetch_all(pool)
                .await?;
                let mut head = Vec::new();
                let mut current = Vec::new();
                let mut states = Vec::new();
                for row in rows {
                    let in_head: bool = row.try_get("in_head")?;
                    let is_known: bool = row.try_get("is_known")?;
                    if let Some(state) = row.try_get::<Option<String>, _>("triage_state")? {
                        states.push(FleetAlertStateView {
                            alert_id: row.try_get("public_id")?,
                            state,
                            muted_until_unix: row.try_get("triage_muted_until_unix")?,
                            escalation_level: row.try_get("triage_escalation_level")?,
                            revision: row.try_get("triage_revision")?,
                            reason: row.try_get("triage_reason")?,
                            actor_id: row.try_get("triage_actor_id")?,
                            created_at: row.try_get("triage_created_at")?,
                            updated_at: row.try_get("triage_updated_at")?,
                        });
                    }
                    let episode = operational_episode_from_row(row)?;
                    if in_head {
                        head.push(episode.clone());
                    }
                    if is_known {
                        current.push(episode);
                    }
                }
                Ok(OperationalAlertEventSync {
                    current,
                    head,
                    states,
                })
            }
        }
    }

    pub(crate) async fn resolve_operational_alert_event(
        &self,
        public_id: &str,
        reason: &str,
        operator: &AuthContext,
    ) -> Result<OperationalAlertEpisodeRecord> {
        let (_, mut episodes) = self
            .resolve_operational_alert_events(&[(public_id.to_string(), None)], reason, operator)
            .await?;
        episodes
            .pop()
            .context("fleet alert resolution returned no episode")
    }

    pub(crate) async fn resolve_operational_alert_events(
        &self,
        resolution_items: &[(String, Option<i64>)],
        reason: &str,
        operator: &AuthContext,
    ) -> Result<(Uuid, Vec<OperationalAlertEpisodeRecord>)> {
        let reason = reason.trim();
        anyhow::ensure!(
            !resolution_items.is_empty() && resolution_items.len() <= 1_000,
            "fleet_alert_resolution_items_invalid"
        );
        anyhow::ensure!(
            reason.len() <= 1024 && !reason.is_empty(),
            "fleet_alert_resolution_reason_invalid"
        );
        let mut unique_ids = BTreeSet::new();
        let mut normalized_items = Vec::with_capacity(resolution_items.len());
        for (public_id, expected_generation) in resolution_items {
            let public_id = public_id.trim();
            anyhow::ensure!(
                !public_id.is_empty() && public_id.len() <= 192,
                "fleet_alert_id_required"
            );
            anyhow::ensure!(
                unique_ids.insert(public_id.to_string()),
                "fleet_alert_resolution_duplicate_item"
            );
            if let Some(expected_generation) = expected_generation {
                anyhow::ensure!(
                    *expected_generation >= 1,
                    "fleet_alert_resolution_generation_invalid"
                );
            }
            normalized_items.push((public_id.to_string(), *expected_generation));
        }
        normalized_items.sort_by(|left, right| left.0.cmp(&right.0));
        let normalized_ids = normalized_items
            .iter()
            .map(|(public_id, _)| public_id.clone())
            .collect::<Vec<_>>();
        let batch_id = Uuid::new_v4();
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let client_ids = sqlx::query_scalar::<_, String>(
                    r#"
                    SELECT selected.client_id
                    FROM (
                        SELECT DISTINCT client_id
                        FROM alert_episodes
                        WHERE public_id=ANY($1::text[])
                          AND client_id IS NOT NULL
                    ) selected
                    ORDER BY selected.client_id COLLATE "C"
                    "#,
                )
                .bind(&normalized_ids)
                .fetch_all(&mut *tx)
                .await?;
                lock_client_policy_suppressions_shared_in_tx(&mut tx, &client_ids).await?;

                let unresolved_generations = sqlx::query_as::<_, (Uuid, i32)>(
                    r#"
                    SELECT DISTINCT policy_rule_id, policy_rule_version
                    FROM alert_episodes
                    WHERE public_id=ANY($1::text[])
                      AND resolved_at IS NULL
                      AND policy_rule_id IS NOT NULL
                      AND policy_rule_version IS NOT NULL
                    ORDER BY policy_rule_id, policy_rule_version
                    "#,
                )
                .bind(&normalized_ids)
                .fetch_all(&mut *tx)
                .await?;
                let locked_generations =
                    lock_policy_rule_generations_shared_in_tx(&mut tx, &unresolved_generations)
                        .await?;
                anyhow::ensure!(
                    locked_generations == unresolved_generations.len(),
                    "fleet_alert_resolution_snapshot_stale"
                );

                let state_owned_ids = sqlx::query_scalar::<_, String>(
                    r#"
                    SELECT episode.public_id
                    FROM alert_policy_evaluation_states AS state
                    JOIN alert_episodes AS episode ON episode.id=state.active_episode_id
                    WHERE episode.public_id=ANY($1::text[])
                    ORDER BY state.policy_rule_id, state.rule_version,
                             state.confirmation_bucket_key COLLATE "C"
                    FOR UPDATE OF state
                    "#,
                )
                .bind(&normalized_ids)
                .fetch_all(&mut *tx)
                .await?;
                let state_owned_id_set = state_owned_ids.iter().cloned().collect::<HashSet<_>>();
                anyhow::ensure!(
                    state_owned_id_set.len() == state_owned_ids.len(),
                    "fleet_alert_resolution_state_owner_invalid"
                );

                let locked = sqlx::query_as::<_, (String, i64, Option<DateTime<Utc>>)>(
                    r#"
                    SELECT public_id, trigger_generation, resolved_at
                    FROM alert_episodes
                    WHERE public_id=ANY($1::text[])
                    ORDER BY public_id COLLATE "C"
                    FOR UPDATE
                    "#,
                )
                .bind(&normalized_ids)
                .fetch_all(&mut *tx)
                .await?;
                anyhow::ensure!(
                    locked.len() == normalized_items.len(),
                    "fleet_alert_not_found"
                );
                for (
                    (locked_id, locked_generation, resolved_at),
                    (expected_id, expected_generation),
                ) in locked.iter().zip(&normalized_items)
                {
                    anyhow::ensure!(locked_id == expected_id, "fleet_alert_not_found");
                    if let Some(expected_generation) = expected_generation {
                        anyhow::ensure!(
                            locked_generation == expected_generation,
                            "fleet_alert_resolution_snapshot_stale"
                        );
                    }
                    if resolved_at.is_none() {
                        anyhow::ensure!(
                            state_owned_id_set.contains(locked_id),
                            "fleet_alert_resolution_snapshot_stale"
                        );
                    }
                }

                let transitioned_ids = resolve_policy_occurrence_episodes_prelocked_in_tx(
                    &mut tx,
                    &normalized_ids,
                    reason,
                    operator.operator.id,
                )
                .await?;
                let sql = operational_episode_select_sql(
                    r#"
                    WHERE e.public_id=ANY($1::text[])
                    ORDER BY e.public_id COLLATE "C"
                    FOR UPDATE OF e
                    "#,
                );
                let episodes = sqlx::query(&sql)
                    .bind(&normalized_ids)
                    .fetch_all(&mut *tx)
                    .await?
                    .into_iter()
                    .map(operational_episode_from_row)
                    .collect::<Result<Vec<_>>>()?;
                anyhow::ensure!(
                    episodes.len() == normalized_ids.len()
                        && episodes
                            .iter()
                            .map(|episode| &episode.public_id)
                            .eq(normalized_ids.iter()),
                    "fleet_alert_not_found"
                );
                insert_operational_resolution_audits_in_tx(
                    &mut tx,
                    episodes
                        .iter()
                        .filter(|episode| transitioned_ids.contains(episode.public_id.as_str())),
                    operator,
                    reason,
                    batch_id,
                    normalized_ids.len(),
                )
                .await?;
                tx.commit().await?;
                Ok((batch_id, episodes))
            }
        }
    }
}

pub(crate) async fn reconcile_postgres_agent_alert_transition_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    to_status: &str,
) -> Result<()> {
    reconcile_postgres_agent_alert_transition_at_in_tx(tx, client_id, to_status).await
}

pub(crate) async fn reconcile_postgres_deleted_agent_alert_transitions_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_ids: &[String],
) -> Result<()> {
    lock_postgres_operational_reconcile_clients_in_tx(tx, client_ids).await?;
    record_policy_source_scope_exits_in_tx(
        tx,
        &["agent.status", "agent.access"],
        client_ids,
        &BTreeSet::new(),
    )
    .await
    .map(|_| ())
}

async fn reconcile_postgres_agent_alert_transition_at_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    to_status: &str,
) -> Result<()> {
    lock_postgres_operational_reconcile_clients_in_tx(tx, &[client_id.to_string()]).await?;
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
        sources.push(AlertSource {
            producer_kind: "job".to_string(),
            natural_key: job_id.to_string(),
            target_kind: "job".to_string(),
            target_id: job_id.to_string(),
            client_id: None,
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
            source_revision_token: None,
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
            target_kind: "job_target".to_string(),
            target_id: format!("{job_id}:{client_id}"),
            client_id: Some(client_id.clone()),
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
            source_revision_token: None,
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
        target_kind: "backup_request".to_string(),
        target_id: backup_id.to_string(),
        client_id: Some(client_id.clone()),
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
        source_revision_token: None,
    };
    reconcile_postgres_event_sources_in_tx(tx, vec![source]).await
}

pub(crate) async fn reconcile_postgres_tunnel_alerts_for_clients_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_ids: &[String],
) -> Result<()> {
    lock_postgres_operational_reconcile_clients_in_tx(tx, client_ids).await?;
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
    lock_postgres_operational_reconcile_clients_in_tx(tx, client_ids).await?;
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
        observed_at: episode.last_confirmed_at.clone(),
        operator_state: "open".to_string(),
        muted_until_unix: None,
        escalation_level: 0,
        state_revision: 0,
        state_reason: None,
        state_actor_id: None,
        state_updated_at: None,
    }
}

async fn reconcile_postgres_event_sources_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    sources: Vec<AlertSource>,
) -> Result<()> {
    record_postgres_policy_event_sources_in_tx(tx, sources).await
}

fn operational_resolution_audit(
    episode: &OperationalAlertEpisodeRecord,
    operator: &AuthContext,
    reason: &str,
    batch_id: Uuid,
    batch_size: usize,
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
            "batch_id": batch_id,
            "batch_size": batch_size,
            "result": "succeeded",
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

async fn insert_operational_resolution_audits_in_tx<'a>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    episodes: impl IntoIterator<Item = &'a OperationalAlertEpisodeRecord>,
    operator: &AuthContext,
    reason: &str,
    batch_id: Uuid,
    batch_size: usize,
) -> Result<()> {
    let audits = episodes
        .into_iter()
        .map(|episode| {
            operational_resolution_audit(episode, operator, reason, batch_id, batch_size)
        })
        .collect::<Vec<_>>();
    if audits.is_empty() {
        return Ok(());
    }
    let mut query = QueryBuilder::<Postgres>::new(
        r#"
        INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata, created_at)
        "#,
    );
    query.push_values(audits, |mut row, audit| {
        row.push_bind(audit.id)
            .push_bind(audit.actor_id)
            .push_bind(audit.action)
            .push_bind(audit.target)
            .push_bind(audit.command_hash)
            .push_bind(audit.metadata)
            .push_bind(audit.created_at)
            .push_unseparated("::timestamptz");
    });
    query.build().execute(&mut **tx).await?;
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
            e.resolution_actor_id, e.created_at::text AS created_at,
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
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
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

fn parse_episode_time(value: &str) -> Result<DateTime<Utc>> {
    parse_timestamp_utc(value)
        .with_context(|| format!("invalid operational alert timestamp: {value}"))
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
    for probe in probes {
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
        source.source_revision_token.as_deref().map_or_else(
            || {
                alert_policy_state_source_event_id(
                    source_kind,
                    &source.natural_key,
                    observed_at.timestamp_nanos_opt().unwrap_or_default(),
                    &payload,
                )
            },
            |revision_token| {
                alert_policy_state_source_revision_event_id(
                    source_kind,
                    &source.natural_key,
                    revision_token,
                    &payload,
                )
            },
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
        source: AlertSource {
            producer_kind: "agent_status".to_string(),
            natural_key: format!("{client_id}:connectivity"),
            target_kind: "agent".to_string(),
            target_id: client_id.to_string(),
            client_id: Some(client_id.to_string()),
            detail: format!("{display_name} currently reports {status}"),
            source_status: status.to_string(),
            evidence: evidence.clone(),
            observed_at: observed_at.to_string(),
            source_revision_token: None,
        },
    };
    let access = ConditionProbe {
        state: if status == "revoked" {
            ProbeState::Confirmed
        } else {
            ProbeState::Healthy
        },
        source: AlertSource {
            producer_kind: "agent_access".to_string(),
            natural_key: format!("{client_id}:access"),
            target_kind: "agent".to_string(),
            target_id: client_id.to_string(),
            client_id: Some(client_id.to_string()),
            detail: format!("{display_name} cannot reconnect until an operator assigns a new key"),
            source_status: status.to_string(),
            evidence,
            observed_at: observed_at.to_string(),
            source_revision_token: None,
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

struct PostgresTunnelEvidence {
    client_id: String,
    interface: String,
    observed_at: String,
    accepted_at: String,
    plan_id: Option<Uuid>,
    topology_identity_hash: Option<String>,
    runtime_evidence_identity_hash: Option<String>,
    endpoint_side: Option<String>,
    traffic_source: Option<String>,
    traffic_status: Option<String>,
    traffic_reason: Option<String>,
    traffic_checked_unix: Option<i64>,
    adapter_health: Option<Value>,
}

fn tunnel_alert_source_revision_token(
    producer: &str,
    traffic_checked_unix: Option<i64>,
    adapter_health: Option<&Value>,
    status_boundary_at: Option<&str>,
    runtime_boundary_at: &str,
) -> String {
    let checked = match producer {
        "tunnel_adapter" => adapter_health
            .and_then(|value| value.get("checked_unix"))
            .and_then(|value| {
                value
                    .as_i64()
                    .map(|value| value.to_string())
                    .or_else(|| value.as_u64().map(|value| value.to_string()))
            })
            .unwrap_or_else(|| "missing".to_string()),
        "tunnel_traffic" => traffic_checked_unix
            .map(|value| value.to_string())
            .unwrap_or_else(|| "missing".to_string()),
        _ => "missing".to_string(),
    };
    format!(
        "{producer}:checked={checked}:status-boundary={}:runtime-boundary={runtime_boundary_at}",
        status_boundary_at.unwrap_or("none")
    )
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
               telemetry_endpoint_side, traffic_source, traffic_status,
               traffic_reason, traffic_checked_unix, adapter_health
        FROM telemetry_current_tunnels
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
                plan_id: row.try_get("telemetry_plan_id")?,
                topology_identity_hash: row.try_get("telemetry_topology_identity_hash")?,
                runtime_evidence_identity_hash: row
                    .try_get("telemetry_runtime_evidence_identity_hash")?,
                endpoint_side: row.try_get("telemetry_endpoint_side")?,
                traffic_source: row.try_get("traffic_source")?,
                traffic_status: row.try_get("traffic_status")?,
                traffic_reason: row.try_get("traffic_reason")?,
                traffic_checked_unix: row.try_get("traffic_checked_unix")?,
                adapter_health: row.try_get("adapter_health")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let plan_rows = sqlx::query(
        r#"
        SELECT id, name, revision, left_client_id, right_client_id, plan,
               builtin_credentials,
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
    let evidence_available = status_evidence_available && exact_identity;
    let attributed_tunnel = evidence_available.then_some(tunnel).flatten();
    let reported_tunnel = attributed_tunnel;
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
        "topology_identity_validation": if attributed_tunnel.is_some() {
            "exact"
        } else {
            "unavailable"
        },
    });
    let build = |producer: &str, status: &str, detail: String, evidence: Value| {
        let source_revision_token = attributed_tunnel.map(|tunnel| {
            tunnel_alert_source_revision_token(
                producer,
                tunnel.traffic_checked_unix,
                tunnel.adapter_health.as_ref(),
                status_boundary_at,
                runtime_boundary_at,
            )
        });
        AlertSource {
            producer_kind: producer.to_string(),
            natural_key: format!("{plan_id}:{expected_runtime_evidence_identity_hash}:{side}"),
            target_kind: "tunnel".to_string(),
            target_id: format!("{}:{}", client_id, plan.interface_name),
            client_id: Some(client_id.to_string()),
            detail,
            source_status: status.to_string(),
            evidence,
            observed_at: observed_at.to_string(),
            source_revision_token,
        }
    };
    if plan.runtime_control.manager == RuntimeTunnelManager::CustomAdapter {
        let adapter = attributed_tunnel.and_then(|tunnel| tunnel.adapter_health.as_ref());
        let success = adapter
            .and_then(|value| value.get("success"))
            .and_then(Value::as_bool);
        let (state, status, detail) = match success {
            Some(true) => (
                ProbeState::Healthy,
                "tunnel_adapter_healthy",
                "Tunnel adapter status is healthy".to_string(),
            ),
            Some(false) => (
                ProbeState::Confirmed,
                "tunnel_adapter_degraded",
                adapter
                    .and_then(|value| value.get("reason"))
                    .and_then(Value::as_str)
                    .unwrap_or("adapter command did not report healthy status")
                    .to_string(),
            ),
            None => (
                ProbeState::Unknown,
                "tunnel_adapter_evidence_missing",
                "Tunnel adapter health evidence is unavailable".to_string(),
            ),
        };
        snapshot.conditions.push(ConditionProbe {
            state,
            source: build(
                "tunnel_adapter",
                status,
                detail,
                merge_json(
                    base.clone(),
                    json!({
                        "adapter_health": adapter,
                    }),
                ),
            ),
        });
    }
    let traffic_status = attributed_tunnel.and_then(|tunnel| tunnel.traffic_status.as_deref());
    let (state, status, detail) = match traffic_status {
        Some("ok") => (
            ProbeState::Healthy,
            "tunnel_traffic_ok",
            "Tunnel interface counters are healthy".to_string(),
        ),
        Some(_) => (
            ProbeState::Confirmed,
            "tunnel_traffic_degraded",
            attributed_tunnel
                .and_then(|tunnel| tunnel.traffic_reason.clone())
                .unwrap_or_else(|| "tunnel interface counters are not reporting ok".to_string()),
        ),
        None => (
            ProbeState::Unknown,
            "tunnel_traffic_evidence_missing",
            "Tunnel traffic counter evidence is unavailable".to_string(),
        ),
    };
    snapshot.conditions.push(ConditionProbe {
        state,
        source: build(
            "tunnel_traffic",
            status,
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
                }),
            ),
        ),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operational_evidence_serializes_only_its_exact_subject_identity() {
        let source = include_str!("repository_operational_alerts.rs");
        let (_, owner) = source
            .split_once("async fn lock_postgres_operational_reconcile_clients_in_tx")
            .expect("operational evidence owner");
        let (owner, _) = owner
            .split_once("#[derive(Clone, Copy, Debug, Eq, PartialEq)]")
            .expect("operational evidence owner boundary");
        assert!(owner.contains("client_ids.sort_unstable()"));
        assert!(owner.contains("client_ids.dedup()"));
        assert!(owner.contains("pg_advisory_xact_lock"));
        assert!(!owner.contains("FROM clients"));
        assert!(!owner.contains("FOR SHARE"));
        assert!(!owner.contains("FOR UPDATE"));
    }

    #[test]
    fn state_source_identity_ignores_subject_and_presentation_metadata() {
        let original = agent_probes(
            "vps-a",
            "edge-original",
            "offline",
            &["old-tag".to_string()],
            "2026-01-01T00:00:00Z",
            json!({}),
        )
        .into_iter()
        .next()
        .expect("connectivity source")
        .source;
        let renamed = agent_probes(
            "vps-a",
            "edge-renamed",
            "offline",
            &["new-tag".to_string()],
            "2026-01-01T00:00:00Z",
            json!({}),
        )
        .into_iter()
        .next()
        .expect("connectivity source")
        .source;

        let original = policy_fact_from_source(original, Some(ProbeState::Confirmed)).unwrap();
        let renamed = policy_fact_from_source(renamed, Some(ProbeState::Confirmed)).unwrap();
        assert_eq!(original.source_event_id, renamed.source_event_id);
        assert_ne!(original.payload, renamed.payload);

        let recovered = agent_probes(
            "vps-a",
            "edge-renamed",
            "online",
            &["new-tag".to_string()],
            "2026-01-01T00:00:00Z",
            json!({}),
        )
        .into_iter()
        .next()
        .expect("connectivity source")
        .source;
        let recovered = policy_fact_from_source(recovered, Some(ProbeState::Healthy)).unwrap();
        assert_ne!(original.source_event_id, recovered.source_event_id);
    }

    fn tunnel_traffic_test_source(
        observed_at: &str,
        source_revision_token: Option<String>,
    ) -> AlertSource {
        AlertSource {
            producer_kind: "tunnel_traffic".to_string(),
            natural_key: "plan:runtime:left".to_string(),
            target_kind: "tunnel".to_string(),
            target_id: "vps-a:wg0".to_string(),
            client_id: Some("vps-a".to_string()),
            detail: "Tunnel interface counters are healthy".to_string(),
            source_status: "tunnel_traffic_ok".to_string(),
            evidence: json!({
                "plan": {"interface": "wg0"},
                "traffic_status": "ok",
                "traffic_reason": null,
            }),
            observed_at: observed_at.to_string(),
            source_revision_token,
        }
    }

    #[test]
    fn tunnel_alert_revision_four_cached_receipts_are_one_authoritative_source() {
        let token = tunnel_alert_source_revision_token(
            "tunnel_traffic",
            Some(42),
            None,
            Some("2026-01-01T00:00:00Z"),
            "2026-01-01T00:00:00Z",
        );
        let source_ids = ["01", "02", "03", "04"]
            .into_iter()
            .map(|second| {
                let source = tunnel_traffic_test_source(
                    &format!("2026-01-01T00:00:{second}Z"),
                    Some(token.clone()),
                );
                policy_fact_from_source(source, Some(ProbeState::Healthy))
                    .unwrap()
                    .source_event_id
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(source_ids.len(), 1);
    }

    #[test]
    fn tunnel_alert_revision_checks_and_boundaries_keep_sources_independent() {
        let adapter_42 = json!({"checked_unix": 42, "success": true});
        let adapter_43 = json!({"checked_unix": 43, "success": true});
        let traffic_before = tunnel_alert_source_revision_token(
            "tunnel_traffic",
            Some(42),
            Some(&adapter_42),
            Some("2026-01-01T00:00:00Z"),
            "2026-01-01T00:00:00Z",
        );
        let traffic_after_adapter_check = tunnel_alert_source_revision_token(
            "tunnel_traffic",
            Some(42),
            Some(&adapter_43),
            Some("2026-01-01T00:00:00Z"),
            "2026-01-01T00:00:00Z",
        );
        assert_eq!(traffic_before, traffic_after_adapter_check);

        let adapter_before = tunnel_alert_source_revision_token(
            "tunnel_adapter",
            Some(42),
            Some(&adapter_42),
            Some("2026-01-01T00:00:00Z"),
            "2026-01-01T00:00:00Z",
        );
        let adapter_after = tunnel_alert_source_revision_token(
            "tunnel_adapter",
            Some(42),
            Some(&adapter_43),
            Some("2026-01-01T00:00:00Z"),
            "2026-01-01T00:00:00Z",
        );
        assert_ne!(adapter_before, adapter_after);

        let traffic_checked = tunnel_alert_source_revision_token(
            "tunnel_traffic",
            Some(43),
            Some(&adapter_43),
            Some("2026-01-01T00:00:00Z"),
            "2026-01-01T00:00:00Z",
        );
        let traffic_boundary = tunnel_alert_source_revision_token(
            "tunnel_traffic",
            Some(42),
            Some(&adapter_43),
            Some("2026-01-01T00:05:00Z"),
            "2026-01-01T00:00:00Z",
        );
        assert_ne!(traffic_before, traffic_checked);
        assert_ne!(traffic_before, traffic_boundary);

        let first_fresh = policy_fact_from_source(
            tunnel_traffic_test_source("2026-01-01T00:05:01Z", Some(traffic_boundary.clone())),
            Some(ProbeState::Healthy),
        )
        .unwrap();
        let cached_fresh = policy_fact_from_source(
            tunnel_traffic_test_source("2026-01-01T00:05:15Z", Some(traffic_boundary)),
            Some(ProbeState::Healthy),
        )
        .unwrap();
        assert_eq!(first_fresh.source_event_id, cached_fresh.source_event_id);
    }
}
