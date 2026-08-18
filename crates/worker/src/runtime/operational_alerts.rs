use std::collections::HashSet;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{types::Json as SqlJson, Postgres, Row, Transaction};
use uuid::Uuid;
use vpsman_common::{
    tunnel_runtime_evidence_identity_hash, tunnel_topology_identity_hash, RuntimeTunnelManager,
    TunnelBuiltinCredentials, TunnelPlan,
};

use crate::webhook_rules::insert_webhook_event_at_in_tx;

// This adapter intentionally owns only worker-written source transitions. The
// canonical model remains repository_operational_alerts.rs in vpsman-api.
// Keep the lock, source constants, persistence columns, and edge payload below
// byte-for-byte aligned with that owner; do not grow this into another general
// alert evaluator.
const OPERATIONAL_RECONCILE_LOCK: &str = "vpsman:operational-alert-reconcile";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProbeState {
    Confirmed,
    Healthy,
    Unknown,
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
    state: ProbeState,
    resolution_reason: &'static str,
    source: AlertSource,
}

#[derive(Clone, Debug)]
struct Episode {
    id: Uuid,
    public_id: String,
    producer_kind: String,
    natural_key: String,
    record_kind: String,
    trigger_generation: i64,
    trigger_severity: String,
    trigger_category: String,
    severity: String,
    category: String,
    target_kind: String,
    target_id: String,
    client_id: Option<String>,
    title: String,
    detail: String,
    source_status: String,
    evidence: Value,
    lifecycle_state: String,
    triggered_at: String,
    last_confirmed_at: Option<String>,
    resolved_at: Option<String>,
    resolution_reason: Option<String>,
    resolution_note: Option<String>,
    resolution_actor_id: Option<Uuid>,
    backfilled: bool,
    created_at: String,
    updated_at: String,
}

#[derive(Clone)]
struct LifecycleEdge {
    episode: Episode,
    triggered: bool,
}

/// Reconciles the worker's authoritative status transition before its source
/// transaction commits. The caller already holds the changed client row, so
/// the lock order is source row -> global lifecycle advisory -> episode rows.
pub(crate) async fn reconcile_agent_status_transition_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    to_status: &str,
) -> Result<()> {
    let observed_at = lifecycle_clock(tx).await?.to_rfc3339();
    lock_lifecycle(tx).await?;

    let identity = sqlx::query(
        r#"
        SELECT c.display_name, c.status, c.capabilities,
               c.operational_alert_tunnel_boundary_at AS tunnel_boundary_at,
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

    let mut status_probes = Vec::new();
    let mut tunnel_boundary_at = None;
    if let Some(row) = identity {
        let current_status: String = row.try_get("status")?;
        if current_status != to_status {
            return Ok(());
        }
        let display_name: String = row.try_get("display_name")?;
        let tags = serde_json::from_value::<Vec<String>>(row.try_get("tags")?).unwrap_or_default();
        let capabilities: Value = row.try_get("capabilities")?;
        tunnel_boundary_at = row
            .try_get::<Option<DateTime<Utc>>, _>("tunnel_boundary_at")?
            .map(|value| value.to_rfc3339());
        status_probes = agent_probes(
            client_id,
            &display_name,
            &current_status,
            &tags,
            &observed_at,
            capabilities.get("privilege_mode"),
        );
    }

    reconcile_condition_probe_set(
        tx,
        client_id,
        &["agent_status", "agent_access"],
        status_probes,
    )
    .await?;

    let tunnel_probes = match tunnel_boundary_at {
        Some(boundary) => load_tunnel_unknown_probes(tx, client_id, &boundary).await?,
        None => Vec::new(),
    };
    reconcile_condition_probe_set(
        tx,
        client_id,
        &["tunnel_adapter", "tunnel_traffic"],
        tunnel_probes,
    )
    .await
}

/// Materializes only the job and capability event sources written by the
/// schedule worker. Event immutability and exact edge dedupe make retries safe.
pub(crate) async fn reconcile_scheduled_job_event_sources_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
) -> Result<()> {
    let Some(job) = sqlx::query(
        r#"
        SELECT id, command_type, status, target_count, alert_terminal_at
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
    let alert_terminal_at = job.try_get::<Option<DateTime<Utc>>, _>("alert_terminal_at")?;
    let mut sources = Vec::new();
    if let Some(alert_terminal_at) = alert_terminal_at.filter(|_| {
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
            source_status: status.clone(),
            evidence: json!({
                "job_id": job_id,
                "command_type": &command_type,
                "target_count": job.try_get::<i32, _>("target_count")?,
                "retained_identity": true,
            }),
            observed_at: alert_terminal_at.to_rfc3339(),
        });
    }

    let target_rows = sqlx::query(
        r#"
        SELECT target.client_id, target.status, target.message, target.exit_code,
               target.started_at, target.completed_at, target.capability_alert_at,
               target.capability_degraded_reason, target.capability_degraded_hint,
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
    for row in target_rows {
        let client_id: String = row.try_get("client_id")?;
        let display_name = row
            .try_get::<Option<String>, _>("display_name")?
            .unwrap_or_else(|| client_id.clone());
        let tags = serde_json::from_value::<Vec<String>>(row.try_get("tags")?).unwrap_or_default();
        let reason: String = row.try_get("capability_degraded_reason")?;
        let hint: String = row.try_get("capability_degraded_hint")?;
        let started_at = optional_timestamp_string(&row, "started_at")?;
        let completed_at = optional_timestamp_string(&row, "completed_at")?;
        let capability_alert_at = timestamp_string(&row, "capability_alert_at")?;
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
                    "command_type": &command_type,
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
            observed_at: capability_alert_at,
        });
    }
    persist_new_event_sources(tx, sources).await
}

async fn load_tunnel_unknown_probes(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    status_boundary_at: &str,
) -> Result<Vec<ConditionProbe>> {
    // The caller holds this client row. Tunnel-plan writers lock endpoint
    // clients before their plan row, so these reads cannot race a newer plan
    // transition and do not invert the global lifecycle lock.
    let identity = sqlx::query(
        r#"
        SELECT c.display_name,
               COALESCE(
                   (SELECT jsonb_agg(t.name ORDER BY t.display_order, t.name)
                    FROM client_tags ct JOIN tags t ON t.id = ct.tag_id
                    WHERE ct.client_id = c.id),
                   '[]'::jsonb
               ) AS tags
        FROM clients c
        WHERE c.id = $1 AND c.hidden_at IS NULL
        "#,
    )
    .bind(client_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(identity) = identity else {
        return Ok(Vec::new());
    };
    let display_name: String = identity.try_get("display_name")?;
    let tags = serde_json::from_value::<Vec<String>>(identity.try_get("tags")?).unwrap_or_default();
    let plan_rows = sqlx::query(
        r#"
        SELECT id, name, revision, left_client_id, right_client_id, plan,
               builtin_credentials, operational_alert_runtime_boundary_at
        FROM tunnel_plans
        WHERE enabled AND deleted_at IS NULL
          AND (left_client_id = $1 OR right_client_id = $1)
        ORDER BY id
        "#,
    )
    .bind(client_id)
    .fetch_all(&mut **tx)
    .await?;

    let mut probes = Vec::new();
    for row in plan_rows {
        let plan_id: Uuid = row.try_get("id")?;
        let plan_name: String = row.try_get("name")?;
        let revision: i64 = row.try_get("revision")?;
        let left_client_id: String = row.try_get("left_client_id")?;
        let right_client_id: String = row.try_get("right_client_id")?;
        let plan = serde_json::from_value::<TunnelPlan>(row.try_get("plan")?)
            .context("invalid persisted tunnel plan during worker alert reconciliation")?;
        let credential_generation = row
            .try_get::<Option<SqlJson<Value>>, _>("builtin_credentials")?
            .map(|value| {
                serde_json::from_value::<TunnelBuiltinCredentials>(value.0)
                    .context(
                        "invalid persisted tunnel credentials during worker alert reconciliation",
                    )
                    .map(|credentials| credentials.generation())
            })
            .transpose()?;
        let runtime_boundary_at = timestamp_string(&row, "operational_alert_runtime_boundary_at")?;
        let (side, peer_client_id) = if left_client_id == client_id {
            ("left", right_client_id.as_str())
        } else {
            ("right", left_client_id.as_str())
        };
        let topology_identity_hash = tunnel_topology_identity_hash(plan_id, &plan);
        let runtime_evidence_identity_hash =
            tunnel_runtime_evidence_identity_hash(plan_id, &plan, credential_generation);
        let base_evidence = json!({
            "subject": {
                "client_id": client_id,
                "display_name": &display_name,
                "tags": &tags,
            },
            "plan": {
                "id": plan_id,
                "name": &plan_name,
                "revision": revision,
                "topology_identity_hash": &topology_identity_hash,
                "runtime_evidence_identity_hash": &runtime_evidence_identity_hash,
                "endpoint_side": side,
                "peer_client_id": peer_client_id,
                "interface": &plan.interface_name,
            },
            "telemetry_observed_at": Value::Null,
            "telemetry_accepted_at": Value::Null,
            "reported_topology_identity_hash": Value::Null,
            "reported_runtime_evidence_identity_hash": Value::Null,
            "status_boundary_at": status_boundary_at,
            "runtime_boundary_at": &runtime_boundary_at,
            "topology_identity_validation": "unavailable",
        });
        let natural_key = format!("{plan_id}:{runtime_evidence_identity_hash}:{side}");
        if plan.runtime_control.manager == RuntimeTunnelManager::CustomAdapter {
            probes.push(ConditionProbe {
                state: ProbeState::Unknown,
                resolution_reason: "condition_recovered",
                source: tunnel_source(
                    "tunnel_adapter",
                    &natural_key,
                    client_id,
                    &plan.interface_name,
                    "critical",
                    "tunnel_adapter_evidence_missing",
                    "Tunnel adapter status is unavailable",
                    "Tunnel adapter health evidence is unavailable",
                    merge_json(
                        base_evidence.clone(),
                        json!({"adapter_health": Value::Null}),
                    ),
                    status_boundary_at,
                ),
            });
        }
        probes.push(ConditionProbe {
            state: ProbeState::Unknown,
            resolution_reason: "condition_recovered",
            source: tunnel_source(
                "tunnel_traffic",
                &natural_key,
                client_id,
                &plan.interface_name,
                "warning",
                "tunnel_traffic_evidence_missing",
                "Tunnel traffic status is unavailable",
                "Tunnel traffic counter evidence is unavailable",
                merge_json(
                    base_evidence,
                    json!({
                        "traffic_source": Value::Null,
                        "traffic_status": Value::Null,
                        "traffic_reason": Value::Null,
                    }),
                ),
                status_boundary_at,
            ),
        });
    }
    Ok(probes)
}

#[allow(clippy::too_many_arguments)]
fn tunnel_source(
    producer_kind: &str,
    natural_key: &str,
    client_id: &str,
    interface_name: &str,
    severity: &str,
    source_status: &str,
    title: &str,
    detail: &str,
    evidence: Value,
    observed_at: &str,
) -> AlertSource {
    AlertSource {
        producer_kind: producer_kind.to_string(),
        natural_key: natural_key.to_string(),
        record_kind: "condition".to_string(),
        severity: severity.to_string(),
        category: "network".to_string(),
        target_kind: "tunnel".to_string(),
        target_id: format!("{client_id}:{interface_name}"),
        client_id: Some(client_id.to_string()),
        title: title.to_string(),
        detail: detail.to_string(),
        source_status: source_status.to_string(),
        evidence,
        observed_at: observed_at.to_string(),
    }
}

fn agent_probes(
    client_id: &str,
    display_name: &str,
    status: &str,
    tags: &[String],
    observed_at: &str,
    privilege_mode: Option<&Value>,
) -> Vec<ConditionProbe> {
    let evidence = merge_json(
        source_identity_evidence(client_id, Some(display_name), tags),
        json!({"capability_privilege_mode": privilege_mode}),
    );
    let connectivity_confirmed = matches!(status, "never" | "disconnected" | "offline" | "stale");
    vec![
        ConditionProbe {
            state: if connectivity_confirmed {
                ProbeState::Confirmed
            } else {
                ProbeState::Healthy
            },
            resolution_reason: if status == "revoked" {
                "source_scope_exited"
            } else {
                "condition_recovered"
            },
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
        },
        ConditionProbe {
            state: if status == "revoked" {
                ProbeState::Confirmed
            } else {
                ProbeState::Healthy
            },
            resolution_reason: "condition_recovered",
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
                detail: format!(
                    "{display_name} cannot reconnect until an operator assigns a new key"
                ),
                source_status: status.to_string(),
                evidence,
                observed_at: observed_at.to_string(),
            },
        },
    ]
}

async fn reconcile_condition_probe_set(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    producer_kinds: &[&str],
    probes: Vec<ConditionProbe>,
) -> Result<()> {
    let producer_kinds = producer_kinds
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
    let rows = sqlx::query(
        r#"
        SELECT id, public_id, producer_kind, natural_key, record_kind,
               trigger_generation, trigger_severity, trigger_category,
               severity, category, target_kind, target_id, client_id,
               title, detail, source_status, evidence, lifecycle_state,
               triggered_at, last_confirmed_at, resolved_at, resolution_reason,
               resolution_note, resolution_actor_id, backfilled, created_at, updated_at
        FROM operational_alert_episodes
        WHERE client_id = $1 AND producer_kind = ANY($2::text[])
        FOR UPDATE
        "#,
    )
    .bind(client_id)
    .bind(&producer_kinds)
    .fetch_all(&mut **tx)
    .await?;
    let mut episodes = rows
        .into_iter()
        .map(episode_from_row)
        .collect::<Result<Vec<_>>>()?;
    let keys = probes
        .iter()
        .map(|probe| {
            (
                probe.source.producer_kind.clone(),
                probe.source.natural_key.clone(),
            )
        })
        .collect::<HashSet<_>>();
    let mut changed = Vec::new();
    let mut edges = Vec::new();

    for probe in probes {
        let current_index = episodes.iter().position(|episode| {
            episode.producer_kind == probe.source.producer_kind
                && episode.natural_key == probe.source.natural_key
                && episode.resolved_at.is_none()
        });
        match (probe.state, current_index) {
            (ProbeState::Confirmed, Some(index)) => {
                let episode = &mut episodes[index];
                refresh_episode_from_source(episode, &probe.source);
                episode.lifecycle_state = "persisting".to_string();
                episode.last_confirmed_at = Some(max_time_string(
                    episode.last_confirmed_at.as_deref(),
                    &probe.source.observed_at,
                ));
                episode.updated_at = Utc::now().to_rfc3339();
                changed.push(episode.clone());
            }
            (ProbeState::Confirmed, None) => {
                let generation = next_generation(&episodes, &probe.source);
                let episode = new_episode(&probe.source, generation);
                changed.push(episode.clone());
                edges.push(LifecycleEdge {
                    episode: episode.clone(),
                    triggered: true,
                });
                episodes.push(episode);
            }
            (ProbeState::Healthy, Some(index)) => {
                let episode = &mut episodes[index];
                let resolved_at = causal_now(episode);
                record_resolution_evidence(episode, &probe.source);
                resolve_episode(episode, &resolved_at, probe.resolution_reason);
                changed.push(episode.clone());
                edges.push(LifecycleEdge {
                    episode: episode.clone(),
                    triggered: false,
                });
            }
            (ProbeState::Unknown, Some(index)) => {
                let episode = &mut episodes[index];
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
                episode.updated_at = Utc::now().to_rfc3339();
                changed.push(episode.clone());
            }
            (ProbeState::Healthy | ProbeState::Unknown, None) => {}
        }
    }

    for episode in episodes.iter_mut().filter(|episode| {
        episode.record_kind == "condition"
            && episode.resolved_at.is_none()
            && !keys.contains(&(episode.producer_kind.clone(), episode.natural_key.clone()))
    }) {
        let resolved_at = causal_now(episode);
        resolve_episode(episode, &resolved_at, "source_scope_exited");
        changed.push(episode.clone());
        edges.push(LifecycleEdge {
            episode: episode.clone(),
            triggered: false,
        });
    }
    persist_changes_and_edges(tx, changed, edges).await
}

async fn persist_new_event_sources(
    tx: &mut Transaction<'_, Postgres>,
    sources: Vec<AlertSource>,
) -> Result<()> {
    let cutoff: DateTime<Utc> = sqlx::query_scalar(
        "SELECT event_source_cutoff_at FROM operational_alert_lifecycle_meta WHERE singleton",
    )
    .fetch_one(&mut **tx)
    .await?;
    let sources = sources
        .into_iter()
        .filter(|source| {
            parse_canonical_timestamp(&source.observed_at)
                .is_some_and(|observed_at| observed_at >= cutoff)
        })
        .collect::<Vec<_>>();
    if sources.is_empty() {
        return Ok(());
    }
    lock_lifecycle(tx).await?;
    for source in sources {
        let existing = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT id
            FROM operational_alert_episodes
            WHERE record_kind = 'event' AND producer_kind = $1 AND natural_key = $2
            FOR UPDATE
            "#,
        )
        .bind(&source.producer_kind)
        .bind(&source.natural_key)
        .fetch_optional(&mut **tx)
        .await?;
        if existing.is_some() {
            continue;
        }
        let episode = new_episode(&source, 1);
        save_episode(tx, &episode).await?;
        emit_lifecycle_edge(tx, &episode, true).await?;
    }
    Ok(())
}

async fn persist_changes_and_edges(
    tx: &mut Transaction<'_, Postgres>,
    changed: Vec<Episode>,
    edges: Vec<LifecycleEdge>,
) -> Result<()> {
    for episode in &changed {
        save_episode(tx, episode).await?;
    }
    for edge in edges {
        emit_lifecycle_edge(tx, &edge.episode, edge.triggered).await?;
    }
    Ok(())
}

async fn save_episode(tx: &mut Transaction<'_, Postgres>, episode: &Episode) -> Result<()> {
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

async fn emit_lifecycle_edge(
    tx: &mut Transaction<'_, Postgres>,
    episode: &Episode,
    triggered: bool,
) -> Result<()> {
    let state = if triggered { "triggered" } else { "resolved" };
    let kind = format!("alert.{state}");
    let event_id = format!("fleet-alert:{}:{state}", episode.id);
    let mut predicates = vec![
        kind.clone(),
        format!("alert.category:{}", episode.trigger_category),
        format!("alert.severity:{}", episode.trigger_severity),
    ];
    if triggered {
        predicates.push("alert.open".to_string());
    }
    let occurred_at = if triggered {
        &episode.triggered_at
    } else {
        episode.resolved_at.as_ref().unwrap_or(&episode.updated_at)
    };
    let occurred_at = parse_canonical_timestamp(occurred_at)
        .context("worker operational lifecycle edge timestamp is invalid")?;
    insert_webhook_event_at_in_tx(
        tx,
        &kind,
        &event_id,
        &predicates,
        &episode.client_id.iter().cloned().collect::<Vec<_>>(),
        json!({
            "event": {
                "kind": kind,
                "id": event_id,
                "occurred_at": occurred_at.to_rfc3339(),
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
        occurred_at,
    )
    .await?;
    Ok(())
}

fn episode_from_row(row: sqlx::postgres::PgRow) -> Result<Episode> {
    Ok(Episode {
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
        triggered_at: timestamp_string(&row, "triggered_at")?,
        last_confirmed_at: optional_timestamp_string(&row, "last_confirmed_at")?,
        resolved_at: optional_timestamp_string(&row, "resolved_at")?,
        resolution_reason: row.try_get("resolution_reason")?,
        resolution_note: row.try_get("resolution_note")?,
        resolution_actor_id: row.try_get("resolution_actor_id")?,
        backfilled: row.try_get("backfilled")?,
        created_at: timestamp_string(&row, "created_at")?,
        updated_at: timestamp_string(&row, "updated_at")?,
    })
}

fn new_episode(source: &AlertSource, generation: i64) -> Episode {
    let id = Uuid::new_v4();
    let now = Utc::now().to_rfc3339();
    Episode {
        id,
        public_id: format!("operational-alert:{id}"),
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
        lifecycle_state: "triggered".to_string(),
        triggered_at: source.observed_at.clone(),
        last_confirmed_at: Some(source.observed_at.clone()),
        resolved_at: None,
        resolution_reason: None,
        resolution_note: None,
        resolution_actor_id: None,
        backfilled: false,
        created_at: now.clone(),
        updated_at: now,
    }
}

fn refresh_episode_from_source(episode: &mut Episode, source: &AlertSource) {
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

fn record_resolution_evidence(episode: &mut Episode, source: &AlertSource) {
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

fn resolve_episode(episode: &mut Episode, resolved_at: &str, reason: &str) {
    episode.lifecycle_state = "resolved".to_string();
    episode.resolved_at = Some(resolved_at.to_string());
    episode.resolution_reason = Some(reason.to_string());
    episode.resolution_note = None;
    episode.resolution_actor_id = None;
    episode.updated_at = resolved_at.to_string();
}

fn next_generation(episodes: &[Episode], source: &AlertSource) -> i64 {
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

fn causal_now(episode: &Episode) -> String {
    max_time_string(
        episode.last_confirmed_at.as_deref(),
        &Utc::now().to_rfc3339(),
    )
}

fn max_time_string(current: Option<&str>, candidate: &str) -> String {
    let current = current.and_then(parse_canonical_timestamp);
    let candidate = parse_canonical_timestamp(candidate);
    match (current, candidate) {
        (Some(current), Some(candidate)) if current >= candidate => current.to_rfc3339(),
        (_, Some(candidate)) => candidate.to_rfc3339(),
        (Some(current), None) => current.to_rfc3339(),
        (None, None) => Utc::now().to_rfc3339(),
    }
}

fn parse_canonical_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn timestamp_string(row: &sqlx::postgres::PgRow, field: &str) -> Result<String> {
    Ok(row.try_get::<DateTime<Utc>, _>(field)?.to_rfc3339())
}

fn optional_timestamp_string(row: &sqlx::postgres::PgRow, field: &str) -> Result<Option<String>> {
    Ok(row
        .try_get::<Option<DateTime<Utc>>, _>(field)?
        .map(|value| value.to_rfc3339()))
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

fn merge_json(mut base: Value, extra: Value) -> Value {
    if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
        base.extend(extra.clone());
    }
    base
}

async fn lifecycle_clock(tx: &mut Transaction<'_, Postgres>) -> Result<DateTime<Utc>> {
    Ok(sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **tx)
        .await?)
}

async fn lock_lifecycle(tx: &mut Transaction<'_, Postgres>) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(OPERATIONAL_RECONCILE_LOCK)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
