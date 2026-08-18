use std::collections::BTreeSet;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{types::Json as SqlJson, Postgres, Row, Transaction};
use uuid::Uuid;
use vpsman_common::{
    alert_policy_state_source_event_id, tunnel_runtime_evidence_identity_hash,
    tunnel_topology_identity_hash, RuntimeTunnelManager, TunnelBuiltinCredentials, TunnelPlan,
};

// This adapter owns only facts written by worker source transactions. Policy
// evaluation and lifecycle/webhook projection stay with the API's durable
// evaluator (including its bounded missing-receipt repair).
const OPERATIONAL_RECONCILE_LOCK: &str = "vpsman:operational-alert-reconcile";
const POLICY_EVIDENCE_ARM_LOCK: &str = "vpsman.alert_policy_evidence_arm";
const MAX_SCHEDULE_LINEAGE: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProbeState {
    Complete,
    Unknown,
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
}

#[derive(Clone, Debug)]
struct ConditionProbe {
    state: ProbeState,
    source: AlertSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EvidenceFactKind {
    State,
    Occurrence,
}

impl EvidenceFactKind {
    fn storage(self) -> &'static str {
        match self {
            Self::State => "state",
            Self::Occurrence => "occurrence",
        }
    }
}

#[derive(Debug)]
struct PolicyEvidenceFact {
    source_kind: String,
    source_event_id: String,
    fact_kind: EvidenceFactKind,
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
    state_started_at: Option<DateTime<Utc>>,
    causation_id: Option<Uuid>,
    schedule_lineage: Vec<Uuid>,
}

/// Reconciles the worker's authoritative status transition before its source
/// transaction commits. The caller already holds the changed client row, so
/// the lock order is source row -> global source advisory -> evidence arm.
pub(crate) async fn reconcile_agent_status_transition_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    to_status: &str,
) -> Result<()> {
    lock_lifecycle(tx).await?;

    let identity = sqlx::query(
        r#"
        SELECT c.display_name, c.status, c.capabilities,
               c.operational_alert_status_at AS status_boundary_at,
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
        let observed_at = row
            .try_get::<DateTime<Utc>, _>("status_boundary_at")?
            .to_rfc3339();
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

    let mut probes = status_probes;
    probes.extend(match tunnel_boundary_at {
        Some(boundary) => load_tunnel_unknown_probes(tx, client_id, &boundary).await?,
        None => Vec::new(),
    });
    record_policy_condition_probes_in_tx(tx, probes).await
}

/// Records only the job and capability facts written by the schedule worker.
/// Occurrence source identity makes retries safe without direct lifecycle edges.
pub(crate) async fn reconcile_scheduled_job_event_sources_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
) -> Result<()> {
    let Some(job) = sqlx::query(
        r#"
        SELECT id, command_type, status, target_count, causation_id,
               schedule_lineage, alert_terminal_at
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
        sources.push(AlertSource {
            producer_kind: "job".to_string(),
            natural_key: job_id.to_string(),
            target_kind: "job".to_string(),
            target_id: job_id.to_string(),
            client_id: None,
            detail: format!("{command_type} job {status}"),
            source_status: status.clone(),
            evidence: json!({
                "job_id": job_id,
                "command_type": &command_type,
                "target_count": job.try_get::<i32, _>("target_count")?,
                "causation_id": causation_id,
                "schedule_lineage": &schedule_lineage,
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
            target_kind: "job_target".to_string(),
            target_id: format!("{job_id}:{client_id}"),
            client_id: Some(client_id.clone()),
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
                    "causation_id": causation_id,
                    "schedule_lineage": &schedule_lineage,
                    "retained_identity": true,
                }),
            ),
            observed_at: capability_alert_at,
        });
    }
    record_policy_event_sources_in_tx(tx, sources).await
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
                source: tunnel_source(
                    "tunnel_adapter",
                    &natural_key,
                    client_id,
                    &plan.interface_name,
                    "tunnel_adapter_evidence_missing",
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
            source: tunnel_source(
                "tunnel_traffic",
                &natural_key,
                client_id,
                &plan.interface_name,
                "tunnel_traffic_evidence_missing",
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

fn tunnel_source(
    producer_kind: &str,
    natural_key: &str,
    client_id: &str,
    interface_name: &str,
    source_status: &str,
    detail: &str,
    evidence: Value,
    observed_at: &str,
) -> AlertSource {
    AlertSource {
        producer_kind: producer_kind.to_string(),
        natural_key: natural_key.to_string(),
        target_kind: "tunnel".to_string(),
        target_id: format!("{client_id}:{interface_name}"),
        client_id: Some(client_id.to_string()),
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
    vec![
        ConditionProbe {
            state: ProbeState::Complete,
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
            },
        },
        ConditionProbe {
            state: ProbeState::Complete,
            source: AlertSource {
                producer_kind: "agent_access".to_string(),
                natural_key: format!("{client_id}:access"),
                target_kind: "agent".to_string(),
                target_id: client_id.to_string(),
                client_id: Some(client_id.to_string()),
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

async fn record_policy_condition_probes_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    probes: Vec<ConditionProbe>,
) -> Result<()> {
    let facts = probes
        .into_iter()
        .map(|probe| policy_fact_from_source(probe.source, Some(probe.state)))
        .collect::<Result<Vec<_>>>()?;
    insert_policy_evidence_facts_in_tx(tx, facts).await
}

async fn record_policy_event_sources_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    sources: Vec<AlertSource>,
) -> Result<()> {
    let facts = sources
        .into_iter()
        .map(|source| policy_fact_from_source(source, None))
        .collect::<Result<Vec<_>>>()?;
    insert_policy_evidence_facts_in_tx(tx, facts).await
}

async fn insert_policy_evidence_facts_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    facts: Vec<PolicyEvidenceFact>,
) -> Result<()> {
    if facts.is_empty() {
        return Ok(());
    }

    // Rule edits take the exclusive counterpart. Holding the shared fence for
    // the complete source batch prevents an arm operation from splitting facts
    // emitted by one authoritative source transaction.
    sqlx::query("SELECT pg_advisory_xact_lock_shared(hashtext($1)::bigint)")
        .bind(POLICY_EVIDENCE_ARM_LOCK)
        .execute(&mut **tx)
        .await?;

    for mut fact in facts {
        if let Some(client_id) = fact.subject_client_id.as_deref() {
            if let Some(snapshot) = load_policy_subject_snapshot_in_tx(tx, client_id).await? {
                fact.subject_snapshot = snapshot;
            }
        }
        sqlx::query(
            r#"
            INSERT INTO alert_policy_evidence (
                id, source_kind, source_event_id, fact_kind, natural_key,
                confirmation_bucket_key, subject_client_id, target_kind, target_id,
                source_status, completeness, subject_snapshot, payload, observed_at,
                state_started_at, causation_id, schedule_lineage
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                $12, $13, $14, $15, $16, $17
            )
            ON CONFLICT (source_kind, source_event_id) DO NOTHING
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&fact.source_kind)
        .bind(&fact.source_event_id)
        .bind(fact.fact_kind.storage())
        .bind(&fact.natural_key)
        .bind(&fact.confirmation_bucket_key)
        .bind(&fact.subject_client_id)
        .bind(&fact.target_kind)
        .bind(&fact.target_id)
        .bind(&fact.source_status)
        .bind(if fact.complete { "complete" } else { "unknown" })
        .bind(SqlJson(fact.subject_snapshot))
        .bind(SqlJson(fact.payload))
        .bind(fact.observed_at)
        .bind(fact.state_started_at)
        .bind(fact.causation_id)
        .bind(&fact.schedule_lineage)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn load_policy_subject_snapshot_in_tx(
    tx: &mut Transaction<'_, Postgres>,
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
            .map(|snapshot| snapshot.0)
    })
    .transpose()
    .map_err(Into::into)
}

fn policy_fact_from_source(
    source: AlertSource,
    probe_state: Option<ProbeState>,
) -> Result<PolicyEvidenceFact> {
    let observed_at = DateTime::parse_from_rfc3339(&source.observed_at)
        .with_context(|| {
            format!(
                "invalid worker policy evidence timestamp: {}",
                source.observed_at
            )
        })?
        .with_timezone(&Utc);
    let source_kind = policy_source_kind_for_producer(&source.producer_kind)?;
    let fact_kind = match source_kind {
        "agent.status" | "agent.access" | "tunnel.adapter" | "tunnel.traffic" => {
            EvidenceFactKind::State
        }
        "job.terminal" | "job.capability" => EvidenceFactKind::Occurrence,
        _ => unreachable!("worker policy source mapping is exhaustive"),
    };

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
    let schedule_lineage = canonical_schedule_lineage(
        source
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
            .unwrap_or_default(),
    )?;

    let normalized_status = if source_kind == "job.capability" {
        "skipped".to_string()
    } else {
        source.source_status.clone()
    };
    let mut payload = source.evidence.clone();
    let payload_object = payload
        .as_object_mut()
        .context("worker policy evidence payload is not an object")?;
    payload_object.insert("status".to_string(), json!(&normalized_status));
    payload_object.insert("source_status".to_string(), json!(&source.source_status));
    payload_object.insert("reason".to_string(), json!(&source.detail));
    payload_object.insert("client_id".to_string(), json!(&source.client_id));
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
            payload_object.insert("job_id".to_string(), json!(&source.target_id));
        }
        _ => {}
    }

    let source_event_id = if fact_kind == EvidenceFactKind::Occurrence {
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
        source_status: normalized_status,
        complete: probe_state != Some(ProbeState::Unknown),
        subject_snapshot,
        payload,
        observed_at,
        state_started_at: (fact_kind == EvidenceFactKind::State).then_some(observed_at),
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
        "capability_degraded" => Ok("job.capability"),
        other => anyhow::bail!("unsupported worker policy evidence source {other}"),
    }
}

fn canonical_schedule_lineage(values: Vec<Uuid>) -> Result<Vec<Uuid>> {
    let values = values.into_iter().collect::<BTreeSet<_>>();
    anyhow::ensure!(
        values.len() <= MAX_SCHEDULE_LINEAGE,
        "policy_schedule_lineage_overflow"
    );
    Ok(values.into_iter().collect())
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

async fn lock_lifecycle(tx: &mut Transaction<'_, Postgres>) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(OPERATIONAL_RECONCILE_LOCK)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
