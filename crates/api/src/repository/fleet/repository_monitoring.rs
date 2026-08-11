use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::{bail, Context, Result};
use sqlx::Row;
use uuid::Uuid;
use vpsman_common::{
    AgentMetrics, AgentPingProbeKind, AgentPingTarget, PingTargetResult, MAX_AGENT_PING_TARGETS,
};

use crate::{
    model::{
        AuditLogView, AuthContext, MonitoringShareRecord, MonitoringShareTargetRecord,
        MonitoringShareTargetReplacement, MonitoringShareView, MonitoringShareVisibilityView,
        MonitoringShareVisitorRecord, PingRollupView, PingTargetAssignmentRecord,
        PingTargetAssignmentReplacement, PingTargetAssignmentView, PingTargetDetailView,
        PingTargetRecord, PingTargetRuntimeSyncView, PingTargetView, SystemInformationView,
        TelemetryUptimeView,
    },
    model_monitoring::CurrentPingView,
    repository::Repository,
    repository_key_lifecycle::{
        lock_postgres_agent_identity_lifecycle, require_visible_memory_clients,
        require_visible_postgres_clients_in_tx,
    },
    repository_telemetry_rollups::{
        fragment_final_minute_timestamp, logical_span_fragments, proportional_fragment_count,
        LogicalSpanFragment,
    },
    security::{constant_time_eq, generate_token},
    util::parse_timestamp_unix,
};

const CURRENT_PING_LOSS_WINDOW_SECS: u64 = 15 * 60;
const CURRENT_PING_DEGRADED_LOSS_RATIO: f64 = 0.10;

impl Repository {
    pub(crate) async fn list_latest_telemetry_uptimes(&self) -> Result<Vec<TelemetryUptimeView>> {
        match self {
            Self::Memory(memory) => {
                let mut visible_clients = memory
                    .agents
                    .read()
                    .await
                    .iter()
                    .map(|agent| agent.id.clone())
                    .collect::<HashSet<_>>();
                let hidden_clients = memory.hidden_clients.read().await;
                visible_clients.retain(|client_id| !hidden_clients.contains(client_id));
                drop(hidden_clients);
                let samples = memory.telemetry_samples.read().await;
                let mut latest = HashMap::<String, &crate::model::TelemetrySampleView>::new();
                for sample in samples
                    .iter()
                    .filter(|sample| visible_clients.contains(&sample.client_id))
                {
                    let replace = latest.get(&sample.client_id).is_none_or(|current| {
                        parse_timestamp_unix(&sample.observed_at)
                            .cmp(&parse_timestamp_unix(&current.observed_at))
                            .then_with(|| sample.id.cmp(&current.id))
                            .is_gt()
                    });
                    if replace {
                        latest.insert(sample.client_id.clone(), sample);
                    }
                }
                let mut uptimes = latest
                    .into_values()
                    .filter_map(telemetry_uptime_from_sample)
                    .collect::<Vec<_>>();
                uptimes.sort_by(|left, right| left.client_id.cmp(&right.client_id));
                Ok(uptimes)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        client.id AS client_id,
                        latest.observed_at::text AS observed_at,
                        latest.payload
                    FROM visible_clients client
                    JOIN LATERAL (
                        SELECT sample.observed_at, sample.payload
                        FROM telemetry_samples sample
                        WHERE sample.client_id = client.id
                        ORDER BY sample.observed_at DESC, sample.id DESC
                        LIMIT 1
                    ) latest ON TRUE
                    ORDER BY client.id
                    "#,
                )
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .filter_map(|row| {
                        let client_id = match row.try_get("client_id") {
                            Ok(client_id) => client_id,
                            Err(error) => return Some(Err(error.into())),
                        };
                        let observed_at = match row.try_get("observed_at") {
                            Ok(observed_at) => observed_at,
                            Err(error) => return Some(Err(error.into())),
                        };
                        let payload: serde_json::Value = match row.try_get("payload") {
                            Ok(payload) => payload,
                            Err(error) => return Some(Err(error.into())),
                        };
                        payload
                            .get("uptime_secs")
                            .and_then(serde_json::Value::as_u64)
                            .map(|uptime_secs| {
                                Ok(TelemetryUptimeView {
                                    client_id,
                                    uptime_secs,
                                    observed_at,
                                })
                            })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn monitoring_system_information_for_clients(
        &self,
        client_ids: &[String],
    ) -> Result<HashMap<String, SystemInformationView>> {
        if client_ids.is_empty() {
            return Ok(HashMap::new());
        }
        match self {
            Self::Memory(memory) => {
                let facts = memory.client_system_facts.read().await;
                let agents = memory.agents.read().await;
                let hidden_clients = memory.hidden_clients.read().await;
                let samples = memory.telemetry_samples.read().await;
                let mut views = HashMap::new();
                for client_id in client_ids {
                    let agent = agents.iter().find(|agent| agent.id == *client_id);
                    if hidden_clients.contains(client_id) || agent.is_none() {
                        continue;
                    }
                    let facts = facts.get(client_id);
                    let latest_sample = samples
                        .iter()
                        .filter(|sample| sample.client_id == *client_id)
                        .max_by(|left, right| {
                            parse_timestamp_unix(&left.observed_at)
                                .cmp(&parse_timestamp_unix(&right.observed_at))
                                .then_with(|| left.id.cmp(&right.id))
                        });
                    let uptime_secs = latest_sample
                        .and_then(|sample| sample.payload.get("uptime_secs"))
                        .and_then(serde_json::Value::as_u64);
                    let view = system_information_view(
                        facts.map(|facts| facts.os_release.as_str()),
                        facts
                            .map(|facts| facts.architecture.as_str())
                            .or_else(|| agent.and_then(|agent| agent.arch.as_deref())),
                        facts.and_then(|facts| facts.cpu_model.as_deref()),
                        facts.and_then(|facts| facts.kernel_release.as_deref()),
                        facts.and_then(|facts| facts.virtualization.as_deref()),
                        facts.map(|facts| facts.reported_at.clone()),
                        uptime_secs,
                        uptime_secs
                            .and_then(|_| latest_sample.map(|sample| sample.observed_at.clone())),
                    );
                    if let Some(view) = view {
                        views.insert(client_id.clone(), view);
                    }
                }
                Ok(views)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        client.id AS client_id,
                        client.os_release,
                        client.arch,
                        client.cpu_model,
                        client.kernel_release,
                        client.virtualization,
                        client.system_reported_at::text AS system_reported_at,
                        latest.payload AS latest_payload,
                        latest.observed_at::text AS uptime_observed_at
                    FROM visible_clients client
                    LEFT JOIN LATERAL (
                        SELECT sample.payload, sample.observed_at
                        FROM telemetry_samples sample
                        WHERE sample.client_id = client.id
                        ORDER BY sample.observed_at DESC, sample.id DESC
                        LIMIT 1
                    ) latest ON TRUE
                    WHERE client.id = ANY($1::text[])
                    "#,
                )
                .bind(client_ids)
                .fetch_all(pool)
                .await?;
                let mut views = HashMap::with_capacity(rows.len());
                for row in rows {
                    let client_id: String = row.try_get("client_id")?;
                    let latest_payload: Option<serde_json::Value> =
                        row.try_get("latest_payload")?;
                    let uptime_secs = latest_payload
                        .as_ref()
                        .and_then(|payload| payload.get("uptime_secs"))
                        .and_then(serde_json::Value::as_u64);
                    let uptime_observed_at: Option<String> = row.try_get("uptime_observed_at")?;
                    if let Some(view) = system_information_view(
                        row.try_get::<Option<String>, _>("os_release")?.as_deref(),
                        row.try_get::<Option<String>, _>("arch")?.as_deref(),
                        row.try_get::<Option<String>, _>("cpu_model")?.as_deref(),
                        row.try_get::<Option<String>, _>("kernel_release")?
                            .as_deref(),
                        row.try_get::<Option<String>, _>("virtualization")?
                            .as_deref(),
                        row.try_get("system_reported_at")?,
                        uptime_secs,
                        uptime_secs.and(uptime_observed_at),
                    ) {
                        views.insert(client_id, view);
                    }
                }
                Ok(views)
            }
        }
    }

    pub(crate) async fn list_ping_targets(&self) -> Result<Vec<PingTargetView>> {
        let records = self.list_ping_target_records().await?;
        let assignments = self.list_ping_target_assignment_records(None).await?;
        let mut assigned = HashMap::<Uuid, Vec<String>>::new();
        let mut primary = HashMap::<Uuid, usize>::new();
        for assignment in assignments {
            assigned
                .entry(assignment.target_id)
                .or_default()
                .push(assignment.client_id);
            if assignment.is_primary {
                *primary.entry(assignment.target_id).or_default() += 1;
            }
        }
        for client_ids in assigned.values_mut() {
            client_ids.sort();
            client_ids.dedup();
        }
        Ok(records
            .into_iter()
            .map(|record| ping_target_view(&record, &assigned, &primary))
            .collect())
    }

    pub(crate) async fn get_ping_target_detail(
        &self,
        target_id: Uuid,
    ) -> Result<Option<PingTargetDetailView>> {
        let Some(record) = self
            .list_ping_target_records()
            .await?
            .into_iter()
            .find(|record| record.id == target_id)
        else {
            return Ok(None);
        };
        let assignment_records = self
            .list_ping_target_assignment_records(Some(target_id))
            .await?;
        let ids = assignment_records
            .iter()
            .map(|record| record.client_id.clone())
            .collect::<Vec<_>>();
        let agents = self
            .list_agents_for_client_ids(&ids)
            .await?
            .into_iter()
            .map(|agent| (agent.id.clone(), agent))
            .collect::<HashMap<_, _>>();
        let assignments = assignment_records
            .iter()
            .filter_map(|assignment| {
                agents
                    .get(&assignment.client_id)
                    .cloned()
                    .map(|client| PingTargetAssignmentView {
                        target_id,
                        client,
                        is_primary: assignment.is_primary,
                        assigned_at: assignment.assigned_at.clone(),
                    })
            })
            .collect::<Vec<_>>();
        let mut assigned = HashMap::new();
        assigned.insert(
            target_id,
            assignments
                .iter()
                .map(|assignment| assignment.client.id.clone())
                .collect(),
        );
        let mut primary = HashMap::new();
        primary.insert(
            target_id,
            assignments
                .iter()
                .filter(|assignment| assignment.is_primary)
                .count(),
        );
        Ok(Some(PingTargetDetailView {
            target: ping_target_view(&record, &assigned, &primary),
            assignments,
        }))
    }

    pub(crate) async fn ping_target_record(
        &self,
        target_id: Uuid,
    ) -> Result<Option<PingTargetRecord>> {
        Ok(self
            .list_ping_target_records()
            .await?
            .into_iter()
            .find(|record| record.id == target_id))
    }

    async fn list_ping_target_records(&self) -> Result<Vec<PingTargetRecord>> {
        match self {
            Self::Memory(memory) => {
                let mut records = memory.ping_targets.read().await.clone();
                records.sort_by(|left, right| {
                    left.name
                        .to_lowercase()
                        .cmp(&right.name.to_lowercase())
                        .then_with(|| left.id.cmp(&right.id))
                });
                Ok(records)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id, name, host, probe_kind, port, enabled,
                        selector_expression, generation, created_by, updated_by,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM ping_targets
                    ORDER BY lower(name), id
                    "#,
                )
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(ping_target_record_from_row).collect()
            }
        }
    }

    pub(crate) async fn list_ping_target_assignment_records(
        &self,
        target_id: Option<Uuid>,
    ) -> Result<Vec<PingTargetAssignmentRecord>> {
        match self {
            Self::Memory(memory) => {
                let visible = visible_memory_ping_client_ids(memory).await;
                let mut records = memory
                    .ping_target_assignments
                    .read()
                    .await
                    .iter()
                    .filter(|record| {
                        visible.contains(&record.client_id)
                            && target_id.is_none_or(|id| record.target_id == id)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                records.sort_by(|left, right| {
                    left.target_id
                        .cmp(&right.target_id)
                        .then_with(|| left.client_id.cmp(&right.client_id))
                });
                Ok(records)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        target_id,
                        client_id,
                        is_primary,
                        assigned_at::text AS assigned_at
                    FROM ping_target_assignments assignment
                    JOIN visible_clients client ON client.id = assignment.client_id
                    WHERE $1::UUID IS NULL OR assignment.target_id = $1
                    ORDER BY assignment.target_id, assignment.client_id
                    "#,
                )
                .bind(target_id)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(PingTargetAssignmentRecord {
                            target_id: row.try_get("target_id")?,
                            client_id: row.try_get("client_id")?,
                            is_primary: row.try_get("is_primary")?,
                            assigned_at: row.try_get("assigned_at")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn upsert_ping_target(
        &self,
        record: PingTargetRecord,
        target_client_ids: &[String],
        expected: Option<&PingTargetAssignmentReplacement>,
        operator: &AuthContext,
        action: &str,
    ) -> Result<PingTargetDetailView> {
        let target_ids = normalized_client_ids(target_client_ids);
        if action == "ping_target.updated" && expected.is_none() {
            bail!("ping_target_update_stale");
        }
        if expected.is_some_and(|expected| {
            expected.expected_target.id != record.id
                || normalized_client_ids(&expected.next_client_ids) != target_ids
        }) {
            bail!("ping_target_update_stale");
        }
        match self {
            Self::Memory(memory) => {
                let _lifecycle_guard = memory.agent_key_lifecycle.lock().await;
                let hidden_clients = memory.hidden_clients.read().await;
                let agents = memory.agents.read().await;
                let visible_client_ids = agents
                    .iter()
                    .filter(|agent| !hidden_clients.contains(&agent.id))
                    .map(|agent| agent.id.clone())
                    .collect::<BTreeSet<_>>();
                if target_ids
                    .iter()
                    .any(|client_id| !visible_client_ids.contains(client_id))
                {
                    bail!("ping_target_resolution_stale");
                }
                let mut targets = memory.ping_targets.write().await;
                let mut assignments = memory.ping_target_assignments.write().await;
                let mut next_targets = targets.clone();
                if action == "ping_target.updated"
                    && !next_targets.iter().any(|stored| stored.id == record.id)
                {
                    bail!("ping_target_not_found");
                }
                if let Some(expected) = expected {
                    let Some(stored) = next_targets
                        .iter()
                        .find(|stored| stored.id == expected.expected_target.id)
                    else {
                        bail!("ping_target_update_stale");
                    };
                    let current_assignments = assignments
                        .iter()
                        .filter(|assignment| {
                            assignment.target_id == record.id
                                && visible_client_ids.contains(&assignment.client_id)
                        })
                        .map(|assignment| assignment.client_id.clone())
                        .collect::<Vec<_>>();
                    if !same_ping_target_revision(stored, &expected.expected_target)
                        || normalized_client_ids(&current_assignments)
                            != normalized_client_ids(&expected.expected_client_ids)
                    {
                        bail!("ping_target_update_stale");
                    }
                }
                if next_targets.iter().any(|stored| {
                    stored.id != record.id && stored.name.eq_ignore_ascii_case(&record.name)
                }) {
                    bail!("ping_target_name_conflict");
                }
                if let Some(stored) = next_targets
                    .iter_mut()
                    .find(|stored| stored.id == record.id)
                {
                    *stored = record.clone();
                } else {
                    next_targets.push(record.clone());
                }
                let enabled_targets = next_targets
                    .iter()
                    .filter(|target| target.enabled)
                    .map(|target| target.id)
                    .collect::<BTreeSet<_>>();
                let next_assignments = next_memory_ping_assignments(
                    &assignments,
                    &visible_client_ids,
                    &enabled_targets,
                    record.id,
                    &target_ids,
                )?;
                *targets = next_targets;
                *assignments = next_assignments;
                drop(assignments);
                drop(targets);
                drop(agents);
                drop(hidden_clients);
                record_memory_monitoring_audit(
                    memory,
                    operator,
                    action,
                    format!("ping_target:{}", record.id),
                    ping_target_audit_metadata(&record, &target_ids, operator),
                )
                .await;
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_postgres_ping_clients(&mut tx, &[record.id], &target_ids).await?;
                let locked_targets = lock_postgres_ping_targets(&mut tx, &[record.id]).await?;
                if action == "ping_target.updated" && locked_targets.len() != 1 {
                    bail!("ping_target_not_found");
                }
                if let Some(expected) = expected {
                    let Some(stored) = locked_targets
                        .iter()
                        .find(|stored| stored.id == expected.expected_target.id)
                    else {
                        bail!("ping_target_update_stale");
                    };
                    let current_assignments = sqlx::query_scalar::<_, String>(
                        r#"
                        SELECT assignment.client_id
                        FROM ping_target_assignments assignment
                        JOIN visible_clients client ON client.id = assignment.client_id
                        WHERE assignment.target_id = $1
                        ORDER BY assignment.client_id
                        FOR UPDATE OF assignment
                        "#,
                    )
                    .bind(record.id)
                    .fetch_all(&mut *tx)
                    .await?;
                    if !same_ping_target_revision(stored, &expected.expected_target)
                        || normalized_client_ids(&current_assignments)
                            != normalized_client_ids(&expected.expected_client_ids)
                    {
                        bail!("ping_target_update_stale");
                    }
                }
                sqlx::query(
                    r#"
                    INSERT INTO ping_targets (
                        id, name, host, probe_kind, port, enabled,
                        selector_expression, generation, created_by, updated_by,
                        created_at, updated_at
                    ) VALUES (
                        $1, $2, $3, $4, $5, $6, $7, $8, $9, $9,
                        to_timestamp($10::double precision), now()
                    )
                    ON CONFLICT (id) DO UPDATE SET
                        name = EXCLUDED.name,
                        host = EXCLUDED.host,
                        probe_kind = EXCLUDED.probe_kind,
                        port = EXCLUDED.port,
                        enabled = EXCLUDED.enabled,
                        selector_expression = EXCLUDED.selector_expression,
                        generation = EXCLUDED.generation,
                        updated_by = EXCLUDED.updated_by,
                        updated_at = now()
                    "#,
                )
                .bind(record.id)
                .bind(&record.name)
                .bind(&record.host)
                .bind(&record.probe_kind)
                .bind(record.port)
                .bind(record.enabled)
                .bind(&record.selector_expression)
                .bind(record.generation)
                .bind(operator.operator.id)
                .bind(required_timestamp_f64(&record.created_at)?)
                .execute(&mut *tx)
                .await?;
                replace_postgres_ping_assignments(&mut tx, record.id, &target_ids).await?;
                ensure_postgres_ping_capacity(&mut tx).await?;
                insert_monitoring_audit(
                    &mut tx,
                    Some(operator.operator.id),
                    action,
                    &format!("ping_target:{}", record.id),
                    ping_target_audit_metadata(&record, &target_ids, operator),
                )
                .await?;
                tx.commit().await?;
            }
        }
        self.get_ping_target_detail(record.id)
            .await?
            .context("persisted ping target missing")
    }

    pub(crate) async fn make_primary_ping_target(
        &self,
        target_id: Uuid,
        client_ids: &[String],
        operator: &AuthContext,
    ) -> Result<PingTargetDetailView> {
        let client_ids = normalized_client_ids(client_ids);
        if client_ids.is_empty() {
            bail!("ping_primary_clients_required");
        }
        match self {
            Self::Memory(memory) => {
                let _lifecycle_guard = memory.agent_key_lifecycle.lock().await;
                require_visible_memory_clients(memory, &client_ids, "ping_target_resolution_stale")
                    .await?;
                let targets = memory.ping_targets.read().await;
                if !targets.iter().any(|target| target.id == target_id) {
                    bail!("ping_target_not_found");
                }
                let mut assignments = memory.ping_target_assignments.write().await;
                if client_ids.iter().any(|client_id| {
                    !assignments.iter().any(|assignment| {
                        assignment.target_id == target_id && assignment.client_id == *client_id
                    })
                }) {
                    bail!("ping_primary_assignment_required");
                }
                for assignment in assignments.iter_mut() {
                    if client_ids.contains(&assignment.client_id) {
                        assignment.is_primary = assignment.target_id == target_id;
                    }
                }
                drop(assignments);
                drop(targets);
                record_memory_monitoring_audit(
                    memory,
                    operator,
                    "ping_target.primary_updated",
                    format!("ping_target:{target_id}"),
                    base_monitoring_audit_metadata(
                        operator,
                        serde_json::json!({"target_id": target_id, "client_ids": client_ids}),
                    ),
                )
                .await;
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_postgres_ping_clients(&mut tx, &[target_id], &client_ids).await?;
                if lock_postgres_ping_targets(&mut tx, &[target_id])
                    .await?
                    .len()
                    != 1
                {
                    bail!("ping_target_not_found");
                }
                let assigned_count: i64 = sqlx::query_scalar(
                    r#"
                    SELECT count(*)
                    FROM ping_target_assignments assignment
                    JOIN visible_clients client ON client.id = assignment.client_id
                    WHERE assignment.target_id = $1
                      AND assignment.client_id = ANY($2::TEXT[])
                    "#,
                )
                .bind(target_id)
                .bind(&client_ids)
                .fetch_one(&mut *tx)
                .await?;
                if assigned_count != client_ids.len() as i64 {
                    bail!("ping_primary_assignment_required");
                }
                sqlx::query(
                    "UPDATE ping_target_assignments SET is_primary = FALSE WHERE client_id = ANY($1::TEXT[])",
                )
                .bind(&client_ids)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    r#"
                    UPDATE ping_target_assignments
                    SET is_primary = TRUE
                    WHERE target_id = $1 AND client_id = ANY($2::TEXT[])
                    "#,
                )
                .bind(target_id)
                .bind(&client_ids)
                .execute(&mut *tx)
                .await?;
                insert_monitoring_audit(
                    &mut tx,
                    Some(operator.operator.id),
                    "ping_target.primary_updated",
                    &format!("ping_target:{target_id}"),
                    base_monitoring_audit_metadata(
                        operator,
                        serde_json::json!({"target_id": target_id, "client_ids": client_ids}),
                    ),
                )
                .await?;
                tx.commit().await?;
            }
        }
        self.get_ping_target_detail(target_id)
            .await?
            .context("ping_target_not_found")
    }

    pub(crate) async fn replace_ping_target_assignments_bulk(
        &self,
        replacements: &[PingTargetAssignmentReplacement],
        operator: &AuthContext,
    ) -> Result<Vec<String>> {
        if replacements.is_empty() {
            bail!("ping_target_selection_required");
        }
        let target_ids = replacements
            .iter()
            .map(|replacement| replacement.expected_target.id)
            .collect::<BTreeSet<_>>();
        if target_ids.len() != replacements.len() {
            bail!("ping_target_selection_invalid");
        }
        let changed_target_ids = replacements
            .iter()
            .filter(|replacement| {
                normalized_client_ids(&replacement.expected_client_ids)
                    != normalized_client_ids(&replacement.next_client_ids)
            })
            .map(|replacement| replacement.expected_target.id)
            .collect::<BTreeSet<_>>();
        match self {
            Self::Memory(memory) => {
                let _lifecycle_guard = memory.agent_key_lifecycle.lock().await;
                let proposed_client_ids = replacements
                    .iter()
                    .flat_map(|replacement| normalized_client_ids(&replacement.next_client_ids))
                    .collect::<BTreeSet<_>>();
                let hidden_clients = memory.hidden_clients.read().await;
                let agents = memory.agents.read().await;
                let visible_client_ids = agents
                    .iter()
                    .filter(|agent| !hidden_clients.contains(&agent.id))
                    .map(|agent| agent.id.clone())
                    .collect::<BTreeSet<_>>();
                if proposed_client_ids
                    .iter()
                    .any(|client_id| !visible_client_ids.contains(client_id))
                {
                    bail!("ping_target_resolution_stale");
                }
                // Every in-memory Ping mutation takes target state before
                // assignment state, matching the PostgreSQL lock order.
                let mut stored_targets = memory.ping_targets.write().await;
                for replacement in replacements {
                    let Some(stored) = stored_targets
                        .iter()
                        .find(|target| target.id == replacement.expected_target.id)
                    else {
                        bail!("ping_target_preview_stale");
                    };
                    if !same_ping_target_revision(stored, &replacement.expected_target) {
                        bail!("ping_target_preview_stale");
                    }
                }
                let enabled_targets = stored_targets
                    .iter()
                    .filter(|target| target.enabled)
                    .map(|target| target.id)
                    .collect::<BTreeSet<_>>();
                let mut stored_assignments = memory.ping_target_assignments.write().await;
                let existing = stored_assignments.clone();
                for replacement in replacements {
                    let current = existing
                        .iter()
                        .filter(|assignment| {
                            assignment.target_id == replacement.expected_target.id
                                && visible_client_ids.contains(&assignment.client_id)
                        })
                        .map(|assignment| assignment.client_id.clone())
                        .collect::<BTreeSet<_>>();
                    if current
                        != normalized_client_ids(&replacement.expected_client_ids)
                            .into_iter()
                            .collect()
                    {
                        bail!("ping_target_preview_stale");
                    }
                }
                let primaries = existing
                    .iter()
                    .filter(|assignment| {
                        assignment.is_primary && visible_client_ids.contains(&assignment.client_id)
                    })
                    .map(|assignment| (assignment.target_id, assignment.client_id.clone()))
                    .collect::<BTreeSet<_>>();
                let mut next = existing
                    .into_iter()
                    .filter(|assignment| {
                        !visible_client_ids.contains(&assignment.client_id)
                            || !target_ids.contains(&assignment.target_id)
                    })
                    .collect::<Vec<_>>();
                let now = crate::unix_now().to_string();
                for replacement in replacements {
                    let target_id = replacement.expected_target.id;
                    next.extend(
                        normalized_client_ids(&replacement.next_client_ids)
                            .into_iter()
                            .map(|client_id| PingTargetAssignmentRecord {
                                target_id,
                                is_primary: primaries.contains(&(target_id, client_id.clone())),
                                client_id,
                                assigned_at: now.clone(),
                            }),
                    );
                }
                let mut counts = HashMap::<String, usize>::new();
                for assignment in &next {
                    if visible_client_ids.contains(&assignment.client_id)
                        && enabled_targets.contains(&assignment.target_id)
                    {
                        let count = counts.entry(assignment.client_id.clone()).or_default();
                        *count += 1;
                        if *count > MAX_AGENT_PING_TARGETS {
                            bail!("ping_targets_per_client_too_many:{}", assignment.client_id);
                        }
                    }
                }
                let affected = changed_ping_assignment_clients(replacements);
                *stored_assignments = next;
                for target in stored_targets
                    .iter_mut()
                    .filter(|target| changed_target_ids.contains(&target.id))
                {
                    target.updated_at = now.clone();
                }
                drop(stored_assignments);
                drop(stored_targets);
                drop(agents);
                drop(hidden_clients);
                record_memory_monitoring_audit(
                    memory,
                    operator,
                    "ping_target.targets_bulk_updated",
                    "ping_targets:bulk".to_string(),
                    base_monitoring_audit_metadata(
                        operator,
                        serde_json::json!({
                            "target_ids": target_ids,
                            "client_ids": &affected,
                        }),
                    ),
                )
                .await;
                Ok(affected)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let target_ids = target_ids.iter().copied().collect::<Vec<_>>();
                let proposed_client_ids = replacements
                    .iter()
                    .flat_map(|replacement| normalized_client_ids(&replacement.next_client_ids))
                    .collect::<Vec<_>>();
                lock_postgres_ping_clients(&mut tx, &target_ids, &proposed_client_ids).await?;
                let locked_targets = lock_postgres_ping_targets(&mut tx, &target_ids).await?;
                if locked_targets.len() != target_ids.len() {
                    bail!("ping_target_preview_stale");
                }
                for replacement in replacements {
                    let Some(stored) = locked_targets
                        .iter()
                        .find(|target| target.id == replacement.expected_target.id)
                    else {
                        bail!("ping_target_preview_stale");
                    };
                    if !same_ping_target_revision(stored, &replacement.expected_target) {
                        bail!("ping_target_preview_stale");
                    }
                }
                let current_rows = sqlx::query(
                    r#"
                    SELECT assignment.target_id, assignment.client_id
                    FROM ping_target_assignments assignment
                    JOIN visible_clients client ON client.id = assignment.client_id
                    WHERE assignment.target_id = ANY($1::UUID[])
                    ORDER BY assignment.target_id, assignment.client_id
                    FOR UPDATE OF assignment
                    "#,
                )
                .bind(&target_ids)
                .fetch_all(&mut *tx)
                .await?;
                let mut current_by_target = HashMap::<Uuid, BTreeSet<String>>::new();
                for row in current_rows {
                    current_by_target
                        .entry(row.try_get("target_id")?)
                        .or_default()
                        .insert(row.try_get("client_id")?);
                }
                for replacement in replacements {
                    let current = current_by_target
                        .remove(&replacement.expected_target.id)
                        .unwrap_or_default();
                    if current
                        != normalized_client_ids(&replacement.expected_client_ids)
                            .into_iter()
                            .collect()
                    {
                        bail!("ping_target_preview_stale");
                    }
                }
                for replacement in replacements {
                    replace_postgres_ping_assignments(
                        &mut tx,
                        replacement.expected_target.id,
                        &normalized_client_ids(&replacement.next_client_ids),
                    )
                    .await?;
                }
                if !changed_target_ids.is_empty() {
                    sqlx::query(
                        "UPDATE ping_targets SET updated_by = $2, updated_at = now() WHERE id = ANY($1::UUID[])",
                    )
                    .bind(changed_target_ids.iter().copied().collect::<Vec<_>>())
                    .bind(operator.operator.id)
                    .execute(&mut *tx)
                    .await?;
                }
                ensure_postgres_ping_capacity(&mut tx).await?;
                let affected = changed_ping_assignment_clients(replacements);
                insert_monitoring_audit(
                    &mut tx,
                    Some(operator.operator.id),
                    "ping_target.targets_bulk_updated",
                    "ping_targets:bulk",
                    base_monitoring_audit_metadata(
                        operator,
                        serde_json::json!({
                            "target_ids": target_ids,
                            "client_ids": &affected,
                        }),
                    ),
                )
                .await?;
                tx.commit().await?;
                Ok(affected)
            }
        }
    }

    pub(crate) async fn delete_ping_target(
        &self,
        target_id: Uuid,
        operator: &AuthContext,
    ) -> Result<Vec<String>> {
        match self {
            Self::Memory(memory) => {
                let _lifecycle_guard = memory.agent_key_lifecycle.lock().await;
                let visible_client_ids = visible_memory_ping_client_ids(memory).await;
                let mut targets = memory.ping_targets.write().await;
                let mut assignments = memory.ping_target_assignments.write().await;
                let affected = assignments
                    .iter()
                    .filter(|assignment| {
                        assignment.target_id == target_id
                            && visible_client_ids.contains(&assignment.client_id)
                    })
                    .map(|assignment| assignment.client_id.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                let before = targets.len();
                targets.retain(|target| target.id != target_id);
                let removed = targets.len() != before;
                if !removed {
                    bail!("ping_target_not_found");
                }
                assignments.retain(|assignment| assignment.target_id != target_id);
                drop(assignments);
                drop(targets);
                memory
                    .telemetry_ping_rollups
                    .write()
                    .await
                    .retain(|rollup| rollup.target_id != target_id);
                record_memory_monitoring_audit(
                    memory,
                    operator,
                    "ping_target.deleted",
                    format!("ping_target:{target_id}"),
                    base_monitoring_audit_metadata(
                        operator,
                        serde_json::json!({"target_id": target_id, "client_ids": &affected}),
                    ),
                )
                .await;
                Ok(affected)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let affected = lock_postgres_ping_clients(&mut tx, &[target_id], &[]).await?;
                if lock_postgres_ping_targets(&mut tx, &[target_id])
                    .await?
                    .len()
                    != 1
                {
                    bail!("ping_target_not_found");
                }
                let result = sqlx::query("DELETE FROM ping_targets WHERE id = $1")
                    .bind(target_id)
                    .execute(&mut *tx)
                    .await?;
                if result.rows_affected() == 0 {
                    bail!("ping_target_not_found");
                }
                insert_monitoring_audit(
                    &mut tx,
                    Some(operator.operator.id),
                    "ping_target.deleted",
                    &format!("ping_target:{target_id}"),
                    base_monitoring_audit_metadata(
                        operator,
                        serde_json::json!({"target_id": target_id, "client_ids": &affected}),
                    ),
                )
                .await?;
                tx.commit().await?;
                Ok(affected)
            }
        }
    }

    pub(crate) async fn mutate_ping_targets_bulk(
        &self,
        target_ids: &[Uuid],
        action: &str,
        operator: &AuthContext,
    ) -> Result<Vec<String>> {
        let target_ids = target_ids.iter().copied().collect::<BTreeSet<_>>();
        if target_ids.is_empty() {
            bail!("ping_target_selection_required");
        }
        if !matches!(action, "enable" | "disable" | "delete") {
            bail!("ping_target_lifecycle_action_invalid");
        }
        let audit_action = format!("ping_target.bulk_{action}d");
        match self {
            Self::Memory(memory) => {
                let _lifecycle_guard = memory.agent_key_lifecycle.lock().await;
                let visible_client_ids = visible_memory_ping_client_ids(memory).await;
                let mut stored_targets = memory.ping_targets.write().await;
                if target_ids
                    .iter()
                    .any(|target_id| !stored_targets.iter().any(|target| target.id == *target_id))
                {
                    bail!("ping_target_not_found");
                }
                let mut stored_assignments = memory.ping_target_assignments.write().await;
                let affected = stored_assignments
                    .iter()
                    .filter(|assignment| {
                        target_ids.contains(&assignment.target_id)
                            && visible_client_ids.contains(&assignment.client_id)
                    })
                    .map(|assignment| assignment.client_id.clone())
                    .collect::<BTreeSet<_>>();
                let mut next_targets = stored_targets.clone();
                let mut next_assignments = stored_assignments.clone();
                match action {
                    "enable" | "disable" => {
                        let enabled = action == "enable";
                        for target in &mut next_targets {
                            if target_ids.contains(&target.id) && target.enabled != enabled {
                                target.enabled = enabled;
                                target.generation = target.generation.saturating_add(1);
                                target.updated_at = crate::unix_now().to_string();
                            }
                        }
                        let enabled_targets = next_targets
                            .iter()
                            .filter(|target| target.enabled)
                            .map(|target| target.id)
                            .collect::<BTreeSet<_>>();
                        let mut counts = HashMap::<String, usize>::new();
                        for assignment in &next_assignments {
                            if visible_client_ids.contains(&assignment.client_id)
                                && enabled_targets.contains(&assignment.target_id)
                            {
                                let count = counts.entry(assignment.client_id.clone()).or_default();
                                *count += 1;
                                if *count > MAX_AGENT_PING_TARGETS {
                                    bail!(
                                        "ping_targets_per_client_too_many:{}",
                                        assignment.client_id
                                    );
                                }
                            }
                        }
                    }
                    "delete" => {
                        next_targets.retain(|target| !target_ids.contains(&target.id));
                        next_assignments
                            .retain(|assignment| !target_ids.contains(&assignment.target_id));
                    }
                    _ => unreachable!(),
                }
                *stored_targets = next_targets;
                *stored_assignments = next_assignments;
                drop(stored_assignments);
                drop(stored_targets);
                if action == "delete" {
                    memory
                        .telemetry_ping_rollups
                        .write()
                        .await
                        .retain(|rollup| !target_ids.contains(&rollup.target_id));
                }
                record_memory_monitoring_audit(
                    memory,
                    operator,
                    &audit_action,
                    "ping_targets:bulk".to_string(),
                    base_monitoring_audit_metadata(
                        operator,
                        serde_json::json!({
                            "action": action,
                            "target_ids": &target_ids,
                            "client_ids": &affected,
                        }),
                    ),
                )
                .await;
                Ok(affected.into_iter().collect())
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let ids = target_ids.iter().copied().collect::<Vec<_>>();
                let affected = lock_postgres_ping_clients(&mut tx, &ids, &[]).await?;
                let locked = lock_postgres_ping_targets(&mut tx, &ids).await?;
                if locked.len() != ids.len() {
                    bail!("ping_target_not_found");
                }
                match action {
                    "enable" | "disable" => {
                        sqlx::query(
                            "UPDATE ping_targets SET enabled = $2, generation = generation + 1, updated_by = $3, updated_at = now() WHERE id = ANY($1::UUID[]) AND enabled IS DISTINCT FROM $2",
                        )
                        .bind(&ids)
                        .bind(action == "enable")
                        .bind(operator.operator.id)
                        .execute(&mut *tx)
                        .await?;
                        ensure_postgres_ping_capacity(&mut tx).await?;
                    }
                    "delete" => {
                        sqlx::query("DELETE FROM ping_targets WHERE id = ANY($1::UUID[])")
                            .bind(&ids)
                            .execute(&mut *tx)
                            .await?;
                    }
                    _ => unreachable!(),
                }
                insert_monitoring_audit(
                    &mut tx,
                    Some(operator.operator.id),
                    &audit_action,
                    "ping_targets:bulk",
                    base_monitoring_audit_metadata(
                        operator,
                        serde_json::json!({
                            "action": action,
                            "target_ids": &target_ids,
                            "client_ids": &affected,
                        }),
                    ),
                )
                .await?;
                tx.commit().await?;
                Ok(affected)
            }
        }
    }

    pub(crate) async fn ping_targets_for_client(
        &self,
        client_id: &str,
    ) -> Result<Vec<AgentPingTarget>> {
        let targets = self
            .list_ping_target_records()
            .await?
            .into_iter()
            .map(|target| (target.id, target))
            .collect::<HashMap<_, _>>();
        let mut assigned = self
            .list_ping_target_assignment_records(None)
            .await?
            .into_iter()
            .filter(|assignment| assignment.client_id == client_id)
            .filter_map(|assignment| targets.get(&assignment.target_id).cloned())
            .filter(|target| target.enabled)
            .collect::<Vec<_>>();
        assigned.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        assigned
            .into_iter()
            .map(|target| {
                Ok(AgentPingTarget {
                    id: target.id.to_string(),
                    generation: target.generation.max(1) as u64,
                    name: target.name,
                    host: target.host,
                    kind: match target.probe_kind.as_str() {
                        "icmp" => AgentPingProbeKind::Icmp,
                        "tcp" => AgentPingProbeKind::Tcp,
                        _ => bail!("persisted_ping_target_kind_invalid"),
                    },
                    port: target.port.map(|port| port as u16),
                })
            })
            .collect()
    }

    pub(crate) async fn current_primary_ping_for_clients(
        &self,
        client_ids: &[String],
    ) -> Result<Vec<(String, CurrentPingView)>> {
        self.current_ping_for_clients(client_ids, true).await
    }

    pub(crate) async fn current_ping_targets_for_client(
        &self,
        client_id: &str,
    ) -> Result<Vec<CurrentPingView>> {
        let client_ids = vec![client_id.to_string()];
        Ok(self
            .current_ping_for_clients(&client_ids, false)
            .await?
            .into_iter()
            .map(|(_, ping)| ping)
            .collect())
    }

    async fn current_ping_for_clients(
        &self,
        client_ids: &[String],
        primary_only: bool,
    ) -> Result<Vec<(String, CurrentPingView)>> {
        if client_ids.is_empty() {
            return Ok(Vec::new());
        }
        match self {
            Self::Memory(memory) => {
                let selected = client_ids
                    .iter()
                    .map(String::as_str)
                    .collect::<HashSet<_>>();
                let targets = memory
                    .ping_targets
                    .read()
                    .await
                    .iter()
                    .map(|target| (target.id, target.clone()))
                    .collect::<HashMap<_, _>>();
                let rollups = memory.telemetry_ping_rollups.read().await;
                let mut rows = Vec::new();
                for assignment in
                    memory
                        .ping_target_assignments
                        .read()
                        .await
                        .iter()
                        .filter(|assignment| {
                            (!primary_only || assignment.is_primary)
                                && selected.contains(assignment.client_id.as_str())
                        })
                {
                    let Some(target) = targets.get(&assignment.target_id) else {
                        continue;
                    };
                    let latest = rollups
                        .iter()
                        .filter(|rollup| {
                            rollup.client_id == assignment.client_id
                                && rollup.target_id == assignment.target_id
                                && rollup.generation == target.generation
                        })
                        .max_by(|left, right| {
                            monitoring_timestamp_unix(&left.latest_checked_at)
                                .cmp(&monitoring_timestamp_unix(&right.latest_checked_at))
                        });
                    let rolling_loss_ratio = latest.and_then(|latest| {
                        current_ping_loss_ratio(
                            latest,
                            rollups.iter().filter(|rollup| {
                                rollup.client_id == assignment.client_id
                                    && rollup.target_id == assignment.target_id
                                    && rollup.generation == target.generation
                            }),
                        )
                    });
                    rows.push((
                        assignment.client_id.clone(),
                        current_ping_view(target, latest, rolling_loss_ratio),
                    ));
                }
                rows.sort_by(|left, right| {
                    left.0
                        .cmp(&right.0)
                        .then_with(|| {
                            left.1
                                .target_name
                                .to_lowercase()
                                .cmp(&right.1.target_name.to_lowercase())
                        })
                        .then_with(|| left.1.target_id.cmp(&right.1.target_id))
                });
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        a.client_id,
                        t.id AS target_id,
                        t.name AS target_name,
                        t.enabled,
                        t.generation,
                        latest.latest_status,
                        latest.latency_avg_ms,
                        latest.loss_ratio_avg,
                        rolling.loss_ratio AS rolling_loss_ratio,
                        latest.latest_reason,
                        latest.latest_checked_at::text AS latest_checked_at
                    FROM ping_target_assignments a
                    JOIN ping_targets t ON t.id = a.target_id
                    LEFT JOIN LATERAL (
                        SELECT
                            p.latest_status,
                            p.latency_avg_ms,
                            p.loss_ratio_avg,
                            p.latest_reason,
                            p.latest_checked_at
                        FROM telemetry_ping_rollups p
                        WHERE p.client_id = a.client_id
                          AND p.target_id = a.target_id
                          AND p.generation = t.generation
                        ORDER BY p.latest_checked_at DESC, p.bucket_start DESC
                        LIMIT 1
                    ) latest ON TRUE
                    LEFT JOIN LATERAL (
                        WITH bounds AS (
                            SELECT
                                extract(epoch FROM latest.latest_checked_at)::bigint AS end_unix,
                                GREATEST(
                                    extract(epoch FROM latest.latest_checked_at)::bigint
                                        - ($3::bigint - 1),
                                    0
                                ) AS start_unix
                        ), recent_physical AS (
                            SELECT
                                p.bucket_start,
                                p.bucket_secs,
                                p.sample_count,
                                p.loss_ratio_avg
                            FROM telemetry_ping_rollups p
                            CROSS JOIN bounds
                            WHERE p.client_id = a.client_id
                              AND p.target_id = a.target_id
                              AND p.generation = t.generation
                              AND p.bucket_secs >= 60
                              AND p.bucket_secs % 60 = 0
                              AND p.bucket_start >= to_timestamp(bounds.start_unix)
                              AND p.bucket_start <= to_timestamp(bounds.end_unix)
                        ), preceding_physical AS (
                            SELECT
                                preceding.bucket_start,
                                preceding.bucket_secs,
                                preceding.sample_count,
                                preceding.loss_ratio_avg
                            FROM bounds
                            CROSS JOIN LATERAL (
                                SELECT
                                    candidate.bucket_start
                                FROM telemetry_ping_rollups candidate
                                WHERE candidate.client_id = a.client_id
                                  AND candidate.target_id = a.target_id
                                  AND candidate.generation = t.generation
                                  AND candidate.bucket_secs >= 60
                                  AND candidate.bucket_secs % 60 = 0
                                  AND candidate.bucket_start
                                        < to_timestamp(bounds.start_unix)
                                ORDER BY candidate.bucket_start DESC
                                LIMIT 1
                            ) preceding_start
                            JOIN telemetry_ping_rollups preceding
                              ON preceding.client_id = a.client_id
                             AND preceding.target_id = a.target_id
                             AND preceding.generation = t.generation
                             AND preceding.bucket_start = preceding_start.bucket_start
                             AND preceding.bucket_secs >= 60
                             AND preceding.bucket_secs % 60 = 0
                            WHERE preceding.bucket_start
                                    + make_interval(secs => preceding.bucket_secs - 60)
                                    >= to_timestamp(bounds.start_unix)
                        ), bounded_physical AS (
                            /* The indexed 15-minute range plus one preceding adaptive span
                               covers every non-overlapping physical rollup that can contribute. */
                            SELECT * FROM recent_physical
                            UNION ALL
                            SELECT * FROM preceding_physical
                        ), candidates AS (
                            SELECT
                                p.loss_ratio_avg,
                                p.sample_count::bigint AS sample_count,
                                extract(epoch FROM p.bucket_start)::bigint AS source_start,
                                (p.bucket_secs / 60)::bigint AS source_minutes,
                                bounds.start_unix,
                                bounds.end_unix
                            FROM bounded_physical p
                            CROSS JOIN bounds
                        ), physical AS (
                            SELECT
                                candidates.*,
                                CASE
                                    WHEN start_unix <= source_start THEN 0::bigint
                                    ELSE LEAST(
                                        source_minutes,
                                        (start_unix - source_start + 59) / 60
                                    )
                                END AS first_minute,
                                CASE
                                    WHEN end_unix < source_start THEN 0::bigint
                                    ELSE LEAST(
                                        source_minutes,
                                        (end_unix - source_start) / 60 + 1
                                    )
                                END AS end_minute
                            FROM candidates
                        ), selected AS (
                            SELECT
                                loss_ratio_avg,
                                sample_count * end_minute / source_minutes
                                    - sample_count * first_minute / source_minutes
                                    AS sample_count
                            FROM physical
                            WHERE first_minute < end_minute
                        )
                        SELECT
                            sum(loss_ratio_avg * sample_count::double precision)
                                / NULLIF(sum(sample_count)::double precision, 0)
                                AS loss_ratio
                        FROM selected
                        WHERE sample_count > 0
                    ) rolling ON latest.latest_checked_at IS NOT NULL
                    WHERE a.client_id = ANY($1::TEXT[])
                      AND (NOT $2::BOOLEAN OR a.is_primary)
                    ORDER BY a.client_id, lower(t.name), t.id
                    "#,
                )
                .bind(client_ids)
                .bind(primary_only)
                .bind(CURRENT_PING_LOSS_WINDOW_SECS as i64)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let enabled: bool = row.try_get("enabled")?;
                        let latest_status = row.try_get::<Option<String>, _>("latest_status")?;
                        let latest_loss_ratio = row.try_get::<Option<f64>, _>("loss_ratio_avg")?;
                        let rolling_loss_ratio = row
                            .try_get::<Option<f64>, _>("rolling_loss_ratio")?
                            .or(latest_loss_ratio);
                        let status = latest_status.as_deref().map(|latest_status| {
                            current_ping_status(latest_status, rolling_loss_ratio)
                        });
                        let state = if !enabled {
                            "disabled".to_string()
                        } else {
                            status.clone().unwrap_or_else(|| "pending".to_string())
                        };
                        Ok((
                            row.try_get("client_id")?,
                            CurrentPingView {
                                target_id: row.try_get("target_id")?,
                                target_name: row.try_get("target_name")?,
                                enabled,
                                generation: row.try_get("generation")?,
                                state,
                                status,
                                latency_avg_ms: row.try_get("latency_avg_ms")?,
                                loss_ratio: rolling_loss_ratio,
                                reason: row.try_get("latest_reason")?,
                                checked_at: row.try_get("latest_checked_at")?,
                            },
                        ))
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn record_ping_results_memory(
        &self,
        client_id: &str,
        observed_unix: u64,
        results: &[PingTargetResult],
    ) -> Result<()> {
        let Self::Memory(memory) = self else {
            return Ok(());
        };
        let targets = memory
            .ping_targets
            .read()
            .await
            .iter()
            .map(|target| (target.id, target.clone()))
            .collect::<HashMap<_, _>>();
        let assignments = memory.ping_target_assignments.read().await.clone();
        let mut stored = memory.telemetry_ping_rollups.write().await;
        for result in results.iter().take(MAX_AGENT_PING_TARGETS) {
            let Ok(target_id) = Uuid::parse_str(result.target_id.trim()) else {
                continue;
            };
            let Some(target) = targets.get(&target_id) else {
                continue;
            };
            if !target.enabled
                || target.generation != result.generation as i64
                || !assignments.iter().any(|assignment| {
                    assignment.target_id == target_id && assignment.client_id == client_id
                })
                || !valid_ping_result(result, observed_unix)
            {
                continue;
            }
            upsert_memory_ping_rollup(&mut stored, client_id, target, result);
        }
        Ok(())
    }

    pub(crate) async fn accepted_ping_results_memory(
        &self,
        client_id: &str,
        observed_unix: u64,
        results: &[PingTargetResult],
    ) -> Result<Vec<PingTargetResult>> {
        let Self::Memory(memory) = self else {
            return Ok(Vec::new());
        };
        let enabled_generations = memory
            .ping_targets
            .read()
            .await
            .iter()
            .filter(|target| target.enabled)
            .map(|target| (target.id, target.generation))
            .collect::<HashSet<_>>();
        let assigned = memory
            .ping_target_assignments
            .read()
            .await
            .iter()
            .filter(|assignment| assignment.client_id == client_id)
            .map(|assignment| assignment.target_id)
            .collect::<HashSet<_>>();
        Ok(results
            .iter()
            .take(MAX_AGENT_PING_TARGETS)
            .filter_map(|result| {
                let target_id = Uuid::parse_str(result.target_id.trim()).ok()?;
                (assigned.contains(&target_id)
                    && enabled_generations.contains(&(target_id, result.generation as i64))
                    && valid_ping_result(result, observed_unix))
                .then(|| result.clone())
            })
            .collect())
    }

    pub(crate) async fn list_ping_rollups(
        &self,
        client_id: &str,
        start_unix: Option<u64>,
        end_unix: Option<u64>,
        points_per_target: i64,
        step_secs: i32,
    ) -> Result<Vec<PingRollupView>> {
        let points_per_target = points_per_target.clamp(2, 1_440) as usize;
        let step_secs = step_secs.max(60);
        match self {
            Self::Memory(memory) => {
                let targets = memory
                    .ping_targets
                    .read()
                    .await
                    .iter()
                    .map(|target| (target.id, target.clone()))
                    .collect::<HashMap<_, _>>();
                let primary = memory
                    .ping_target_assignments
                    .read()
                    .await
                    .iter()
                    .filter(|assignment| assignment.client_id == client_id)
                    .map(|assignment| (assignment.target_id, assignment.is_primary))
                    .collect::<HashMap<_, _>>();
                let rows = memory
                    .telemetry_ping_rollups
                    .read()
                    .await
                    .iter()
                    .filter(|row| row.client_id == client_id)
                    .filter_map(|row| {
                        let target = targets.get(&row.target_id)?;
                        let is_primary = primary.get(&row.target_id).copied()?;
                        if row.generation != target.generation {
                            return None;
                        }
                        let mut row = row.clone();
                        row.target_name = target.name.clone();
                        row.is_primary = is_primary;
                        Some(fragment_ping_rollup(row, start_unix, end_unix, step_secs))
                    })
                    .flatten()
                    .collect::<Vec<_>>();
                let mut rows = aggregate_memory_ping_rollups(rows, step_secs);
                retain_fair_ping_points(&mut rows, points_per_target, 50_000);
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH candidates AS (
                        SELECT
                            p.client_id,
                            p.target_id,
                            t.name AS target_name,
                            a.is_primary,
                            p.generation,
                            extract(epoch FROM p.bucket_start)::bigint AS source_start,
                            (p.bucket_secs / 60)::bigint AS source_minutes,
                            p.sample_count,
                            p.success_count,
                            p.latency_avg_ms,
                            p.latency_min_ms,
                            p.latency_max_ms,
                            p.loss_ratio_avg,
                            p.loss_ratio_max,
                            p.latest_status,
                            p.latest_reason,
                            p.latest_checked_at
                        FROM telemetry_ping_rollups p
                        JOIN ping_targets t ON t.id = p.target_id
                        JOIN ping_target_assignments a
                          ON a.target_id = p.target_id AND a.client_id = p.client_id
                        WHERE
                            p.client_id = $1
                            AND p.generation = t.generation
                            AND p.bucket_secs >= 60
                            AND p.bucket_secs % 60 = 0
                            AND ($2::BIGINT IS NULL OR p.bucket_start
                                + make_interval(secs => p.bucket_secs - 60) >= to_timestamp($2))
                            AND ($3::BIGINT IS NULL OR p.bucket_start <= to_timestamp($3))
                    ), physical AS (
                        SELECT
                            candidates.*,
                            CASE
                                WHEN $2::BIGINT IS NULL OR $2 <= source_start THEN 0::bigint
                                ELSE LEAST(
                                    source_minutes,
                                    ($2 - source_start + 59) / 60
                                )
                            END AS first_minute,
                            CASE
                                WHEN $3::BIGINT IS NULL THEN source_minutes
                                WHEN $3 < source_start THEN 0::bigint
                                ELSE LEAST(
                                    source_minutes,
                                    ($3 - source_start) / 60 + 1
                                )
                            END AS end_minute
                        FROM candidates
                    ), fragments AS (
                        SELECT
                            physical.*,
                            chart_epoch,
                            GREATEST(
                                first_minute,
                                ceil((chart_epoch - source_start)::numeric / 60)::bigint
                            ) AS fragment_first_minute,
                            LEAST(
                                end_minute,
                                ceil((chart_epoch + $4::bigint - source_start)::numeric / 60)::bigint
                            ) AS fragment_end_minute
                        FROM physical
                        CROSS JOIN LATERAL generate_series(
                            floor(
                                (source_start + first_minute * 60)::numeric
                                    / $4::numeric
                            )::bigint * $4::bigint,
                            floor(
                                (source_start + (end_minute - 1) * 60)::numeric
                                    / $4::numeric
                            )::bigint * $4::bigint,
                            $4::bigint
                        ) AS generated(chart_epoch)
                        WHERE first_minute < end_minute
                    ), selected AS (
                        SELECT
                            client_id,
                            target_id,
                            target_name,
                            is_primary,
                            generation,
                            to_timestamp(chart_epoch) AS chart_bucket_start,
                            (
                                sample_count::bigint * fragment_end_minute / source_minutes
                                - sample_count::bigint * fragment_first_minute / source_minutes
                            )::integer AS sample_count,
                            (
                                success_count::bigint * fragment_end_minute / source_minutes
                                - success_count::bigint * fragment_first_minute / source_minutes
                            )::integer AS success_count,
                            latency_avg_ms,
                            latency_min_ms,
                            latency_max_ms,
                            loss_ratio_avg,
                            loss_ratio_max,
                            latest_status,
                            latest_reason,
                            latest_checked_at - make_interval(
                                secs => (
                                    (source_minutes - fragment_end_minute) * 60
                                )::double precision
                            ) AS latest_checked_at
                        FROM fragments
                        WHERE sample_count::bigint * fragment_end_minute / source_minutes
                            - sample_count::bigint * fragment_first_minute / source_minutes > 0
                    ), bucketed AS (
                        SELECT
                            client_id,
                            target_id,
                            target_name,
                            bool_or(is_primary) AS is_primary,
                            generation,
                            chart_bucket_start,
                            $4::INTEGER AS bucket_secs,
                            LEAST(sum(sample_count)::bigint, 2147483647)::integer AS sample_count,
                            LEAST(sum(success_count)::bigint, 2147483647)::integer AS success_count,
                            sum(latency_avg_ms * success_count::double precision)
                                / NULLIF(sum(success_count)::double precision, 0)
                                AS latency_avg_ms,
                            min(latency_min_ms)::double precision AS latency_min_ms,
                            max(latency_max_ms)::double precision AS latency_max_ms,
                            sum(loss_ratio_avg * sample_count::double precision)
                                / NULLIF(sum(sample_count)::double precision, 0)
                                AS loss_ratio_avg,
                            max(loss_ratio_max)::double precision AS loss_ratio_max,
                            (array_agg(latest_status ORDER BY latest_checked_at DESC))[1]
                                AS latest_status,
                            (array_agg(latest_reason ORDER BY latest_checked_at DESC))[1]
                                AS latest_reason,
                            max(latest_checked_at) AS latest_checked_at
                        FROM selected
                        GROUP BY
                            client_id, target_id, target_name, generation, chart_bucket_start
                    ), ranked AS (
                        SELECT
                            bucketed.*,
                            row_number() OVER (
                                PARTITION BY client_id, target_id, generation
                                ORDER BY chart_bucket_start DESC
                            ) AS point_rank
                        FROM bucketed
                    )
                    SELECT
                        client_id,
                        target_id,
                        target_name,
                        is_primary,
                        generation,
                        chart_bucket_start::text AS bucket_start,
                        bucket_secs,
                        sample_count,
                        success_count,
                        latency_avg_ms,
                        latency_min_ms,
                        latency_max_ms,
                        COALESCE(loss_ratio_avg, 0) AS loss_ratio_avg,
                        COALESCE(loss_ratio_max, 0) AS loss_ratio_max,
                        latest_status,
                        latest_reason,
                        latest_checked_at::text AS latest_checked_at
                    FROM ranked
                    WHERE point_rank <= $5
                    ORDER BY chart_bucket_start ASC, lower(target_name), target_id, generation
                    LIMIT 50000
                    "#,
                )
                .bind(client_id)
                .bind(start_unix.map(|value| value as i64))
                .bind(end_unix.map(|value| value as i64))
                .bind(step_secs)
                .bind(points_per_target as i64)
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(ping_rollup_from_row).collect()
            }
        }
    }

    pub(crate) async fn list_raw_primary_ping_results_for_clients(
        &self,
        client_ids: &[String],
        start_unix: u64,
        end_unix: u64,
        points_per_client: i64,
        step_secs: i32,
    ) -> Result<Vec<PingRollupView>> {
        if client_ids.is_empty() || start_unix > end_unix {
            return Ok(Vec::new());
        }
        let points_per_client = points_per_client.clamp(2, 1_440) as usize;
        let step_secs = step_secs.max(60);
        if let Self::Postgres(pool) = self {
            let rows = sqlx::query(
                r#"
                WITH raw AS (
                    SELECT
                        sample.client_id,
                        extract(epoch FROM sample.observed_at)::numeric AS observed_unix,
                        CASE
                            WHEN result ->> 'target_id' ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
                            THEN (result ->> 'target_id')::uuid
                        END AS target_id,
                        (result ->> 'generation')::numeric AS generation,
                        (result ->> 'checked_unix')::numeric AS checked_unix,
                        result ->> 'status' AS status,
                        NULLIF(result ->> 'latency_avg_ms', '')::double precision
                            AS latency_avg_ms,
                        (result ->> 'loss_ratio')::double precision AS loss_ratio,
                        result ->> 'reason' AS reason,
                        sample.observed_at
                    FROM telemetry_samples sample
                    CROSS JOIN LATERAL jsonb_array_elements(
                        CASE
                            WHEN jsonb_typeof(sample.payload -> 'ping_results') = 'array'
                            THEN sample.payload -> 'ping_results'
                            ELSE '[]'::jsonb
                        END
                    ) AS result
                    WHERE sample.client_id = ANY($1::TEXT[])
                      AND sample.observed_at >= to_timestamp(GREATEST($2 - 300, 0))
                      AND sample.observed_at <= to_timestamp($3 + 3900)
                ), accepted AS (
                    SELECT
                        raw.client_id,
                        target.id AS target_id,
                        target.name AS target_name,
                        target.generation,
                        raw.checked_unix::bigint AS checked_unix,
                        raw.status,
                        raw.latency_avg_ms,
                        raw.loss_ratio,
                        left(raw.reason, 512) AS reason,
                        raw.observed_at
                    FROM raw
                    JOIN ping_targets target
                      ON target.id = raw.target_id
                     AND target.generation::numeric = raw.generation
                    JOIN ping_target_assignments assignment
                      ON assignment.target_id = target.id
                     AND assignment.client_id = raw.client_id
                     AND assignment.is_primary
                    WHERE raw.generation > 0
                      AND raw.checked_unix > 0
                      AND raw.checked_unix <= raw.observed_unix + 300
                      AND raw.observed_unix - raw.checked_unix <= 3900
                      AND raw.checked_unix >= $2
                      AND raw.checked_unix <= $3
                      AND raw.loss_ratio BETWEEN 0 AND 1
                      AND (raw.reason IS NULL OR length(raw.reason) <= 4096)
                      AND (
                            (raw.status = 'ok'
                                AND raw.latency_avg_ms BETWEEN 0 AND 3600000
                                AND raw.loss_ratio = 0)
                            OR (raw.status = 'degraded'
                                AND raw.latency_avg_ms BETWEEN 0 AND 3600000
                                AND raw.loss_ratio > 0 AND raw.loss_ratio < 1)
                            OR (raw.status IN ('down', 'error')
                                AND raw.latency_avg_ms IS NULL
                                AND raw.loss_ratio = 1)
                      )
                ), deduplicated AS (
                    SELECT DISTINCT ON (client_id, target_id, generation, checked_unix)
                        *
                    FROM accepted
                    ORDER BY
                        client_id, target_id, generation, checked_unix, observed_at DESC
                ), bucketed AS (
                    SELECT
                        client_id,
                        target_id,
                        target_name,
                        generation,
                        floor(checked_unix::numeric / $4::numeric)::bigint
                            * $4::bigint AS chart_epoch,
                        LEAST(count(*)::bigint, 2147483647)::integer AS sample_count,
                        LEAST(count(latency_avg_ms)::bigint, 2147483647)::integer
                            AS success_count,
                        avg(latency_avg_ms)::double precision AS latency_avg_ms,
                        min(latency_avg_ms)::double precision AS latency_min_ms,
                        max(latency_avg_ms)::double precision AS latency_max_ms,
                        avg(loss_ratio)::double precision AS loss_ratio_avg,
                        max(loss_ratio)::double precision AS loss_ratio_max,
                        (array_agg(status ORDER BY checked_unix DESC))[1] AS latest_status,
                        (array_agg(reason ORDER BY checked_unix DESC))[1] AS latest_reason,
                        max(checked_unix)::bigint AS latest_checked_unix
                    FROM deduplicated
                    GROUP BY client_id, target_id, target_name, generation, chart_epoch
                ), ranked AS (
                    SELECT
                        bucketed.*,
                        row_number() OVER (
                            PARTITION BY client_id, target_id, generation
                            ORDER BY chart_epoch DESC
                        ) AS point_rank
                    FROM bucketed
                )
                SELECT
                    client_id,
                    target_id,
                    target_name,
                    TRUE AS is_primary,
                    generation,
                    to_timestamp(chart_epoch)::text AS bucket_start,
                    $4::integer AS bucket_secs,
                    sample_count,
                    success_count,
                    latency_avg_ms,
                    latency_min_ms,
                    latency_max_ms,
                    loss_ratio_avg,
                    loss_ratio_max,
                    latest_status,
                    latest_reason,
                    to_timestamp(latest_checked_unix)::text AS latest_checked_at
                FROM ranked
                WHERE point_rank <= $5
                ORDER BY chart_epoch, client_id, lower(target_name), target_id
                LIMIT 50000
                "#,
            )
            .bind(client_ids)
            .bind(start_unix as i64)
            .bind(end_unix as i64)
            .bind(step_secs)
            .bind(points_per_client as i64)
            .fetch_all(pool)
            .await?;
            return rows.into_iter().map(ping_rollup_from_row).collect();
        }
        let mut rows = Vec::new();
        for client_id in client_ids {
            rows.extend(
                self.list_raw_ping_results(
                    client_id,
                    Some(start_unix),
                    Some(end_unix),
                    points_per_client as i64,
                    step_secs,
                )
                .await?
                .into_iter()
                .filter(|row| row.is_primary),
            );
        }
        rows.sort_by(|left, right| {
            monitoring_timestamp_unix(&left.bucket_start)
                .cmp(&monitoring_timestamp_unix(&right.bucket_start))
                .then_with(|| left.client_id.cmp(&right.client_id))
                .then_with(|| left.target_id.cmp(&right.target_id))
        });
        Ok(rows)
    }

    pub(crate) async fn list_raw_ping_results(
        &self,
        client_id: &str,
        start_unix: Option<u64>,
        end_unix: Option<u64>,
        points_per_target: i64,
        step_secs: i32,
    ) -> Result<Vec<PingRollupView>> {
        let points_per_target = points_per_target.clamp(2, 1_440) as usize;
        let step_secs = step_secs.max(60);
        if let Self::Postgres(pool) = self {
            let rows = sqlx::query(
                r#"
                WITH raw AS (
                    SELECT
                        sample.client_id,
                        extract(epoch FROM sample.observed_at)::numeric AS observed_unix,
                        CASE
                            WHEN result ->> 'target_id' ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
                            THEN (result ->> 'target_id')::uuid
                        END AS target_id,
                        (result ->> 'generation')::numeric AS generation,
                        (result ->> 'checked_unix')::numeric AS checked_unix,
                        result ->> 'status' AS status,
                        NULLIF(result ->> 'latency_avg_ms', '')::double precision
                            AS latency_avg_ms,
                        (result ->> 'loss_ratio')::double precision AS loss_ratio,
                        result ->> 'reason' AS reason,
                        sample.observed_at
                    FROM telemetry_samples sample
                    CROSS JOIN LATERAL jsonb_array_elements(
                        CASE
                            WHEN jsonb_typeof(sample.payload -> 'ping_results') = 'array'
                            THEN sample.payload -> 'ping_results'
                            ELSE '[]'::jsonb
                        END
                    ) AS result
                    WHERE sample.client_id = $1
                      AND (
                            $2::BIGINT IS NULL
                            OR sample.observed_at >= to_timestamp(GREATEST($2 - 300, 0))
                      )
                      AND (
                            $3::BIGINT IS NULL
                            OR sample.observed_at <= to_timestamp($3 + 3900)
                      )
                ), accepted AS (
                    SELECT
                        raw.client_id,
                        target.id AS target_id,
                        target.name AS target_name,
                        assignment.is_primary,
                        target.generation,
                        raw.checked_unix::bigint AS checked_unix,
                        raw.status,
                        raw.latency_avg_ms,
                        raw.loss_ratio,
                        left(raw.reason, 512) AS reason,
                        raw.observed_at
                    FROM raw
                    JOIN ping_targets target
                      ON target.id = raw.target_id
                     AND target.generation::numeric = raw.generation
                    JOIN ping_target_assignments assignment
                      ON assignment.target_id = target.id
                     AND assignment.client_id = raw.client_id
                    WHERE raw.generation > 0
                      AND raw.checked_unix > 0
                      AND raw.checked_unix <= raw.observed_unix + 300
                      AND raw.observed_unix - raw.checked_unix <= 3900
                      AND ($2::BIGINT IS NULL OR raw.checked_unix >= $2)
                      AND ($3::BIGINT IS NULL OR raw.checked_unix <= $3)
                      AND raw.loss_ratio BETWEEN 0 AND 1
                      AND (raw.reason IS NULL OR length(raw.reason) <= 4096)
                      AND (
                            (raw.status = 'ok'
                                AND raw.latency_avg_ms BETWEEN 0 AND 3600000
                                AND raw.loss_ratio = 0)
                            OR (raw.status = 'degraded'
                                AND raw.latency_avg_ms BETWEEN 0 AND 3600000
                                AND raw.loss_ratio > 0 AND raw.loss_ratio < 1)
                            OR (raw.status IN ('down', 'error')
                                AND raw.latency_avg_ms IS NULL
                                AND raw.loss_ratio = 1)
                      )
                ), deduplicated AS (
                    SELECT DISTINCT ON (target_id, generation, checked_unix)
                        *
                    FROM accepted
                    ORDER BY target_id, generation, checked_unix, observed_at DESC
                ), bucketed AS (
                    SELECT
                        client_id,
                        target_id,
                        target_name,
                        bool_or(is_primary) AS is_primary,
                        generation,
                        floor(checked_unix::numeric / $4::numeric)::bigint
                            * $4::bigint AS chart_epoch,
                        LEAST(count(*)::bigint, 2147483647)::integer AS sample_count,
                        LEAST(count(latency_avg_ms)::bigint, 2147483647)::integer
                            AS success_count,
                        avg(latency_avg_ms)::double precision AS latency_avg_ms,
                        min(latency_avg_ms)::double precision AS latency_min_ms,
                        max(latency_avg_ms)::double precision AS latency_max_ms,
                        avg(loss_ratio)::double precision AS loss_ratio_avg,
                        max(loss_ratio)::double precision AS loss_ratio_max,
                        (array_agg(status ORDER BY checked_unix DESC))[1] AS latest_status,
                        (array_agg(reason ORDER BY checked_unix DESC))[1] AS latest_reason,
                        max(checked_unix)::bigint AS latest_checked_unix
                    FROM deduplicated
                    GROUP BY client_id, target_id, target_name, generation, chart_epoch
                ), ranked AS (
                    SELECT
                        bucketed.*,
                        row_number() OVER (
                            PARTITION BY client_id, target_id, generation
                            ORDER BY chart_epoch DESC
                        ) AS point_rank
                    FROM bucketed
                )
                SELECT
                    client_id,
                    target_id,
                    target_name,
                    is_primary,
                    generation,
                    to_timestamp(chart_epoch)::text AS bucket_start,
                    $4::integer AS bucket_secs,
                    sample_count,
                    success_count,
                    latency_avg_ms,
                    latency_min_ms,
                    latency_max_ms,
                    loss_ratio_avg,
                    loss_ratio_max,
                    latest_status,
                    latest_reason,
                    to_timestamp(latest_checked_unix)::text AS latest_checked_at
                FROM ranked
                WHERE point_rank <= $5
                ORDER BY chart_epoch, lower(target_name), target_id
                LIMIT 50000
                "#,
            )
            .bind(client_id)
            .bind(start_unix.map(|value| value as i64))
            .bind(end_unix.map(|value| value as i64))
            .bind(step_secs)
            .bind(points_per_target as i64)
            .fetch_all(pool)
            .await?;
            return rows.into_iter().map(ping_rollup_from_row).collect();
        }
        let targets = self
            .list_ping_target_records()
            .await?
            .into_iter()
            .map(|target| (target.id, target))
            .collect::<HashMap<_, _>>();
        let assigned = self
            .list_ping_target_assignment_records(None)
            .await?
            .into_iter()
            .filter(|assignment| assignment.client_id == client_id)
            .map(|assignment| (assignment.target_id, assignment.is_primary))
            .collect::<HashMap<_, _>>();
        let samples = match self {
            Self::Memory(memory) => memory
                .telemetry_samples
                .read()
                .await
                .iter()
                .filter(|sample| sample.client_id == client_id)
                .filter(|sample| {
                    let observed = monitoring_timestamp_unix(&sample.observed_at);
                    start_unix.is_none_or(|start| observed >= start.saturating_sub(300))
                        && end_unix.is_none_or(|end| observed <= end.saturating_add(3_900))
                })
                .cloned()
                .collect::<Vec<_>>(),
            Self::Postgres(_) => unreachable!(),
        };
        let mut seen = BTreeSet::<(Uuid, u64, u64)>::new();
        let mut rows = Vec::new();
        for sample in samples {
            let metrics: AgentMetrics =
                serde_json::from_value(sample.payload).with_context(|| {
                    format!(
                        "invalid raw telemetry payload for {} at {}",
                        sample.client_id, sample.observed_at
                    )
                })?;
            for result in metrics.ping_results {
                let Ok(target_id) = Uuid::parse_str(result.target_id.trim()) else {
                    continue;
                };
                let Some(target) = targets.get(&target_id) else {
                    continue;
                };
                let Some(is_primary) = assigned.get(&target_id).copied() else {
                    continue;
                };
                if result.generation as i64 != target.generation
                    || !valid_ping_result(&result, metrics.observed_unix)
                    || start_unix.is_some_and(|start| result.checked_unix < start)
                    || end_unix.is_some_and(|end| result.checked_unix > end)
                    || !seen.insert((target_id, result.generation, result.checked_unix))
                {
                    continue;
                }
                rows.push(PingRollupView {
                    client_id: client_id.to_string(),
                    target_id,
                    target_name: target.name.clone(),
                    is_primary,
                    generation: result.generation as i64,
                    bucket_start: result.checked_unix.to_string(),
                    bucket_secs: 0,
                    sample_count: 1,
                    success_count: i32::from(result.latency_avg_ms.is_some()),
                    latency_avg_ms: result.latency_avg_ms,
                    latency_min_ms: result.latency_avg_ms,
                    latency_max_ms: result.latency_avg_ms,
                    loss_ratio_avg: result.loss_ratio,
                    loss_ratio_max: result.loss_ratio,
                    latest_status: result.status,
                    latest_reason: result.reason.as_deref().map(|reason| truncate(reason, 512)),
                    latest_checked_at: result.checked_unix.to_string(),
                });
            }
        }
        let mut rows = aggregate_memory_ping_rollups(rows, step_secs);
        retain_fair_ping_points(&mut rows, points_per_target, 50_000);
        Ok(rows)
    }

    pub(crate) async fn list_ping_rollups_for_export(
        &self,
        client_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<PingRollupView>> {
        match self {
            Self::Memory(memory) => {
                let primary = memory
                    .ping_target_assignments
                    .read()
                    .await
                    .iter()
                    .map(|assignment| {
                        (
                            (assignment.target_id, assignment.client_id.clone()),
                            assignment.is_primary,
                        )
                    })
                    .collect::<HashMap<_, _>>();
                let mut rows = memory
                    .telemetry_ping_rollups
                    .read()
                    .await
                    .iter()
                    .filter(|row| client_id.is_none_or(|id| row.client_id == id))
                    .cloned()
                    .collect::<Vec<_>>();
                for row in &mut rows {
                    row.is_primary = primary
                        .get(&(row.target_id, row.client_id.clone()))
                        .copied()
                        .unwrap_or(false);
                }
                rows.sort_by(|left, right| {
                    right
                        .bucket_start
                        .cmp(&left.bucket_start)
                        .then_with(|| left.client_id.cmp(&right.client_id))
                        .then_with(|| left.target_id.cmp(&right.target_id))
                });
                rows.truncate(limit.clamp(1, 50_000) as usize);
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        p.client_id,
                        p.target_id,
                        t.name AS target_name,
                        COALESCE(a.is_primary, FALSE) AS is_primary,
                        p.generation,
                        p.bucket_start::text AS bucket_start,
                        p.bucket_secs,
                        p.sample_count,
                        p.success_count,
                        p.latency_avg_ms,
                        p.latency_min_ms,
                        p.latency_max_ms,
                        p.loss_ratio_avg,
                        p.loss_ratio_max,
                        p.latest_status,
                        p.latest_reason,
                        p.latest_checked_at::text AS latest_checked_at
                    FROM telemetry_ping_rollups p
                    JOIN ping_targets t ON t.id = p.target_id
                    LEFT JOIN ping_target_assignments a
                      ON a.target_id = p.target_id AND a.client_id = p.client_id
                    WHERE $1::TEXT IS NULL OR p.client_id = $1
                    ORDER BY p.bucket_start DESC, p.client_id, lower(t.name), p.target_id
                    LIMIT $2
                    "#,
                )
                .bind(client_id)
                .bind(limit.clamp(1, 50_000))
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(ping_rollup_from_row).collect()
            }
        }
    }
}

fn telemetry_uptime_from_sample(
    sample: &crate::model::TelemetrySampleView,
) -> Option<TelemetryUptimeView> {
    Some(TelemetryUptimeView {
        client_id: sample.client_id.clone(),
        uptime_secs: sample.payload.get("uptime_secs")?.as_u64()?,
        observed_at: sample.observed_at.clone(),
    })
}

impl Repository {
    pub(crate) async fn list_monitoring_shares(
        &self,
        status: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<MonitoringShareView>> {
        if let Self::Postgres(pool) = self {
            return postgres_monitoring_share_views(
                pool,
                status,
                None,
                limit.clamp(1, 1_000),
                offset.clamp(0, 1_000_000),
            )
            .await;
        }
        let records = self.list_monitoring_share_records().await?;
        let visitors = self.list_monitoring_share_visitor_records().await?;
        let creators = self
            .list_operators()
            .await?
            .into_iter()
            .map(|operator| (operator.id, operator.username))
            .collect::<HashMap<_, _>>();
        let mut views = records
            .iter()
            .map(|record| {
                monitoring_share_view(
                    record,
                    &visitors,
                    record.created_by.and_then(|id| creators.get(&id).cloned()),
                )
            })
            .filter(|view| status.is_none_or(|status| view.status == status))
            .collect::<Vec<_>>();
        views.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(views
            .into_iter()
            .skip(offset.clamp(0, 1_000_000) as usize)
            .take(limit.clamp(1, 1_000) as usize)
            .collect())
    }

    async fn monitoring_share_views_for_ids(
        &self,
        ids: &BTreeSet<Uuid>,
    ) -> Result<Vec<MonitoringShareView>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        if let Self::Postgres(pool) = self {
            return postgres_monitoring_share_views(
                pool,
                None,
                Some(ids.iter().copied().collect()),
                ids.len().min(1_000) as i64,
                0,
            )
            .await;
        }
        let records = self.list_monitoring_share_records().await?;
        let visitors = self.list_monitoring_share_visitor_records().await?;
        let creators = self
            .list_operators()
            .await?
            .into_iter()
            .map(|operator| (operator.id, operator.username))
            .collect::<HashMap<_, _>>();
        Ok(records
            .iter()
            .filter(|record| ids.contains(&record.id))
            .map(|record| {
                monitoring_share_view(
                    record,
                    &visitors,
                    record.created_by.and_then(|id| creators.get(&id).cloned()),
                )
            })
            .collect())
    }

    pub(crate) async fn monitoring_share_record(
        &self,
        share_id: Uuid,
    ) -> Result<Option<MonitoringShareRecord>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .monitoring_shares
                .read()
                .await
                .iter()
                .find(|record| record.id == share_id)
                .cloned()),
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        s.id,
                        s.name,
                        s.token_secret,
                        s.selector_expression,
                        s.show_identity_context,
                        s.show_billing,
                        s.show_system_information,
                        s.show_resources,
                        s.show_network,
                        s.show_traffic,
                        s.show_ping,
                        s.allow_detail_history,
                        s.expires_at::text AS expires_at,
                        s.revoked_at::text AS revoked_at,
                        s.revoked_by,
                        s.created_by,
                        s.created_at::text AS created_at,
                        s.updated_at::text AS updated_at,
                        COALESCE(
                            array_agg(st.client_id ORDER BY st.client_id)
                                FILTER (WHERE st.client_id IS NOT NULL),
                            ARRAY[]::TEXT[]
                        ) AS target_client_ids,
                        COALESCE(
                            array_agg(st.public_client_key ORDER BY st.client_id)
                                FILTER (WHERE st.client_id IS NOT NULL),
                            ARRAY[]::TEXT[]
                        ) AS target_public_client_keys
                    FROM monitoring_share_links s
                    LEFT JOIN monitoring_share_targets st ON st.share_id = s.id
                    WHERE s.id = $1
                    GROUP BY s.id
                    "#,
                )
                .bind(share_id)
                .fetch_optional(pool)
                .await?;
                row.map(monitoring_share_record_from_row).transpose()
            }
        }
    }

    async fn list_monitoring_share_records(&self) -> Result<Vec<MonitoringShareRecord>> {
        match self {
            Self::Memory(memory) => Ok(memory.monitoring_shares.read().await.clone()),
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        s.id,
                        s.name,
                        s.token_secret,
                        s.selector_expression,
                        s.show_identity_context,
                        s.show_billing,
                        s.show_system_information,
                        s.show_resources,
                        s.show_network,
                        s.show_traffic,
                        s.show_ping,
                        s.allow_detail_history,
                        s.expires_at::text AS expires_at,
                        s.revoked_at::text AS revoked_at,
                        s.revoked_by,
                        s.created_by,
                        s.created_at::text AS created_at,
                        s.updated_at::text AS updated_at,
                        COALESCE(
                            array_agg(st.client_id ORDER BY st.client_id)
                                FILTER (WHERE st.client_id IS NOT NULL),
                            ARRAY[]::TEXT[]
                        ) AS target_client_ids,
                        COALESCE(
                            array_agg(st.public_client_key ORDER BY st.client_id)
                                FILTER (WHERE st.client_id IS NOT NULL),
                            ARRAY[]::TEXT[]
                        ) AS target_public_client_keys
                    FROM monitoring_share_links s
                    LEFT JOIN monitoring_share_targets st ON st.share_id = s.id
                    GROUP BY s.id
                    ORDER BY s.created_at DESC, s.id
                    "#,
                )
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(monitoring_share_record_from_row)
                    .collect()
            }
        }
    }

    async fn list_monitoring_share_visitor_records(
        &self,
    ) -> Result<Vec<MonitoringShareVisitorRecord>> {
        match self {
            Self::Memory(memory) => Ok(memory.monitoring_share_visitors.read().await.clone()),
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        share_id,
                        visitor_id,
                        host(source_ip) AS source_ip,
                        user_agent,
                        first_seen_at::text AS first_seen_at,
                        last_seen_at::text AS last_seen_at
                    FROM monitoring_share_visitors
                    ORDER BY share_id, first_seen_at, visitor_id
                    "#,
                )
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(MonitoringShareVisitorRecord {
                            share_id: row.try_get("share_id")?,
                            visitor_id: row.try_get("visitor_id")?,
                            source_ip: row.try_get("source_ip")?,
                            user_agent: row.try_get("user_agent")?,
                            first_seen_at: row.try_get("first_seen_at")?,
                            last_seen_at: row.try_get("last_seen_at")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn create_monitoring_share(
        &self,
        record: MonitoringShareRecord,
        operator: &AuthContext,
    ) -> Result<MonitoringShareView> {
        validate_monitoring_share_targets(&record.targets)?;
        let target_client_ids = record.target_client_ids();
        match self {
            Self::Memory(memory) => {
                let _lifecycle = memory.agent_key_lifecycle.lock().await;
                require_visible_memory_clients(
                    memory,
                    &target_client_ids,
                    "monitoring_share_resolution_stale",
                )
                .await?;
                memory.monitoring_shares.write().await.push(record.clone());
                record_memory_monitoring_audit(
                    memory,
                    operator,
                    "monitoring_share.created",
                    format!("monitoring_share:{}", record.id),
                    share_operator_audit_metadata(&record, operator),
                )
                .await;
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                require_visible_postgres_clients_in_tx(
                    &mut tx,
                    &target_client_ids,
                    "monitoring_share_resolution_stale",
                )
                .await?;
                sqlx::query(
                    r#"
                    INSERT INTO monitoring_share_links (
                        id, name, token_secret, selector_expression,
                        show_identity_context, show_billing, show_system_information,
                        show_resources, show_network, show_traffic, show_ping,
                        allow_detail_history,
                        expires_at, created_by, created_at, updated_at
                    ) VALUES (
                        $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                        $11, $12, to_timestamp($13::double precision), $14,
                        to_timestamp($15::double precision), now()
                    )
                    "#,
                )
                .bind(record.id)
                .bind(&record.name)
                .bind(&record.token_secret)
                .bind(&record.selector_expression)
                .bind(record.visibility.identity_context)
                .bind(record.visibility.billing)
                .bind(record.visibility.system_information)
                .bind(record.visibility.resources)
                .bind(record.visibility.network)
                .bind(record.visibility.traffic)
                .bind(record.visibility.ping)
                .bind(record.visibility.detail_history)
                .bind(required_timestamp_f64(&record.expires_at)?)
                .bind(operator.operator.id)
                .bind(required_timestamp_f64(&record.created_at)?)
                .execute(&mut *tx)
                .await?;
                let public_client_keys = record
                    .targets
                    .iter()
                    .map(|target| target.public_client_key.clone())
                    .collect::<Vec<_>>();
                sqlx::query(
                    r#"
                    INSERT INTO monitoring_share_targets (
                        share_id, client_id, public_client_key
                    )
                    SELECT $1, target.client_id, target.public_client_key
                    FROM unnest($2::TEXT[], $3::TEXT[])
                        AS target(client_id, public_client_key)
                    "#,
                )
                .bind(record.id)
                .bind(&target_client_ids)
                .bind(&public_client_keys)
                .execute(&mut *tx)
                .await?;
                insert_monitoring_audit(
                    &mut tx,
                    Some(operator.operator.id),
                    "monitoring_share.created",
                    &format!("monitoring_share:{}", record.id),
                    share_operator_audit_metadata(&record, operator),
                )
                .await?;
                tx.commit().await?;
            }
        }
        Ok(monitoring_share_view(
            &record,
            &[],
            Some(operator.operator.username.clone()),
        ))
    }

    pub(crate) async fn recover_monitoring_share_url(
        &self,
        share_id: Uuid,
        operator: &AuthContext,
    ) -> Result<String> {
        match self {
            Self::Memory(memory) => {
                let records = memory.monitoring_shares.read().await;
                let record = records
                    .iter()
                    .find(|record| record.id == share_id)
                    .context("monitoring_share_not_found")?;
                if monitoring_share_status(record, crate::unix_now()) != "active" {
                    bail!("monitoring_share_not_active");
                }
                let token_secret = record.token_secret.clone();
                let metadata = base_monitoring_audit_metadata(
                    operator,
                    serde_json::json!({
                        "share_id": record.id,
                        "name": record.name,
                        "target_count": record.targets.len(),
                        "expires_at": record.expires_at,
                    }),
                );
                record_memory_monitoring_audit(
                    memory,
                    operator,
                    "monitoring_share.url_recovered",
                    format!("monitoring_share:{share_id}"),
                    metadata,
                )
                .await;
                drop(records);
                Ok(token_secret)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    SELECT
                        s.name,
                        s.token_secret,
                        s.expires_at::text AS expires_at,
                        s.expires_at > now() AS unexpired,
                        s.revoked_at IS NOT NULL AS revoked,
                        (
                            SELECT count(*)::bigint
                            FROM monitoring_share_targets target
                            WHERE target.share_id = s.id
                        ) AS target_count
                    FROM monitoring_share_links s
                    WHERE s.id = $1
                    FOR SHARE OF s
                    "#,
                )
                .bind(share_id)
                .fetch_optional(&mut *tx)
                .await?
                .context("monitoring_share_not_found")?;
                let unexpired: bool = row.try_get("unexpired")?;
                let revoked: bool = row.try_get("revoked")?;
                if !unexpired || revoked {
                    bail!("monitoring_share_not_active");
                }
                let token_secret: String = row.try_get("token_secret")?;
                let metadata = base_monitoring_audit_metadata(
                    operator,
                    serde_json::json!({
                        "share_id": share_id,
                        "name": row.try_get::<String, _>("name")?,
                        "target_count": row.try_get::<i64, _>("target_count")?,
                        "expires_at": row.try_get::<String, _>("expires_at")?,
                    }),
                );
                insert_monitoring_audit(
                    &mut tx,
                    Some(operator.operator.id),
                    "monitoring_share.url_recovered",
                    &format!("monitoring_share:{share_id}"),
                    metadata,
                )
                .await?;
                tx.commit().await?;
                Ok(token_secret)
            }
        }
    }

    pub(crate) async fn replace_monitoring_share_targets_bulk(
        &self,
        replacements: &[MonitoringShareTargetReplacement],
        operator: &AuthContext,
    ) -> Result<()> {
        if replacements.is_empty() {
            bail!("monitoring_share_selection_required");
        }
        let share_ids = replacements
            .iter()
            .map(|replacement| replacement.expected_share.id)
            .collect::<BTreeSet<_>>();
        if share_ids.len() != replacements.len() || share_ids.len() > 1_000 {
            bail!("monitoring_share_selection_invalid");
        }
        let proposed_client_ids = replacements
            .iter()
            .flat_map(|replacement| normalized_client_ids(&replacement.next_client_ids))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let changed_share_ids = replacements
            .iter()
            .filter(|replacement| {
                normalized_client_ids(&replacement.expected_share.target_client_ids())
                    != normalized_client_ids(&replacement.next_client_ids)
            })
            .map(|replacement| replacement.expected_share.id)
            .collect::<BTreeSet<_>>();
        if changed_share_ids.is_empty() {
            return Ok(());
        }
        let now = crate::unix_now();
        match self {
            Self::Memory(memory) => {
                let _lifecycle = memory.agent_key_lifecycle.lock().await;
                require_visible_memory_clients(
                    memory,
                    &proposed_client_ids,
                    "monitoring_share_resolution_stale",
                )
                .await?;
                let mut records = memory.monitoring_shares.write().await;
                for replacement in replacements {
                    let Some(stored) = records
                        .iter()
                        .find(|record| record.id == replacement.expected_share.id)
                    else {
                        bail!("monitoring_share_preview_stale");
                    };
                    if !same_monitoring_share_revision(stored, &replacement.expected_share)
                        || monitoring_share_status(stored, now) != "active"
                    {
                        bail!("monitoring_share_preview_stale");
                    }
                }
                let mut next_targets_by_share = HashMap::new();
                for replacement in replacements.iter().filter(|replacement| {
                    changed_share_ids.contains(&replacement.expected_share.id)
                }) {
                    let stored = records
                        .iter()
                        .find(|record| record.id == replacement.expected_share.id)
                        .context("monitoring_share_preview_stale")?;
                    let existing_keys = stored
                        .targets
                        .iter()
                        .map(|target| (target.client_id.clone(), target.public_client_key.clone()))
                        .collect::<HashMap<_, _>>();
                    let targets = normalized_client_ids(&replacement.next_client_ids)
                        .into_iter()
                        .map(|client_id| MonitoringShareTargetRecord {
                            public_client_key: existing_keys
                                .get(&client_id)
                                .cloned()
                                .unwrap_or_else(generate_token),
                            client_id,
                        })
                        .collect::<Vec<_>>();
                    validate_monitoring_share_targets(&targets)?;
                    next_targets_by_share.insert(replacement.expected_share.id, targets);
                }
                for (share_id, targets) in next_targets_by_share {
                    let stored = records
                        .iter_mut()
                        .find(|record| record.id == share_id)
                        .context("monitoring_share_preview_stale")?;
                    stored.targets = targets;
                    stored.updated_at = now.to_string();
                }
                drop(records);
                record_memory_monitoring_audit(
                    memory,
                    operator,
                    "monitoring_share.targets_bulk_updated",
                    "monitoring_shares:bulk".to_string(),
                    share_target_updates_audit_metadata(replacements, operator),
                )
                .await;
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                require_visible_postgres_clients_in_tx(
                    &mut tx,
                    &proposed_client_ids,
                    "monitoring_share_resolution_stale",
                )
                .await?;
                let ids = share_ids.iter().copied().collect::<Vec<_>>();
                let locked = sqlx::query(
                    r#"
                    SELECT
                        id,
                        selector_expression,
                        updated_at::text AS updated_at,
                        expires_at > now() AS unexpired,
                        revoked_at IS NOT NULL AS revoked
                    FROM monitoring_share_links
                    WHERE id = ANY($1::UUID[])
                    ORDER BY id
                    FOR UPDATE
                    "#,
                )
                .bind(&ids)
                .fetch_all(&mut *tx)
                .await?;
                if locked.len() != ids.len() {
                    bail!("monitoring_share_preview_stale");
                }
                for row in &locked {
                    let id: Uuid = row.try_get("id")?;
                    let expected = replacements
                        .iter()
                        .find(|replacement| replacement.expected_share.id == id)
                        .context("monitoring_share_preview_stale")?;
                    let selector_expression: String = row.try_get("selector_expression")?;
                    let updated_at: String = row.try_get("updated_at")?;
                    let unexpired: bool = row.try_get("unexpired")?;
                    let revoked: bool = row.try_get("revoked")?;
                    if selector_expression != expected.expected_share.selector_expression
                        || updated_at != expected.expected_share.updated_at
                        || !unexpired
                        || revoked
                    {
                        bail!("monitoring_share_preview_stale");
                    }
                }
                let target_rows = sqlx::query(
                    r#"
                    SELECT share_id, client_id, public_client_key
                    FROM monitoring_share_targets
                    WHERE share_id = ANY($1::UUID[])
                    ORDER BY share_id, client_id
                    FOR UPDATE
                    "#,
                )
                .bind(&ids)
                .fetch_all(&mut *tx)
                .await?;
                let mut current_by_share = HashMap::<Uuid, Vec<MonitoringShareTargetRecord>>::new();
                for row in target_rows {
                    current_by_share
                        .entry(row.try_get("share_id")?)
                        .or_default()
                        .push(MonitoringShareTargetRecord {
                            client_id: row.try_get("client_id")?,
                            public_client_key: row.try_get("public_client_key")?,
                        });
                }
                for replacement in replacements {
                    let current = current_by_share
                        .remove(&replacement.expected_share.id)
                        .unwrap_or_default();
                    if current != replacement.expected_share.targets {
                        bail!("monitoring_share_preview_stale");
                    }
                }
                for replacement in replacements.iter().filter(|replacement| {
                    changed_share_ids.contains(&replacement.expected_share.id)
                }) {
                    let existing_keys = replacement
                        .expected_share
                        .targets
                        .iter()
                        .map(|target| (target.client_id.clone(), target.public_client_key.clone()))
                        .collect::<HashMap<_, _>>();
                    let targets = normalized_client_ids(&replacement.next_client_ids)
                        .into_iter()
                        .map(|client_id| MonitoringShareTargetRecord {
                            public_client_key: existing_keys
                                .get(&client_id)
                                .cloned()
                                .unwrap_or_else(generate_token),
                            client_id,
                        })
                        .collect::<Vec<_>>();
                    validate_monitoring_share_targets(&targets)?;
                    sqlx::query("DELETE FROM monitoring_share_targets WHERE share_id = $1")
                        .bind(replacement.expected_share.id)
                        .execute(&mut *tx)
                        .await?;
                    if !targets.is_empty() {
                        sqlx::query(
                            r#"
                            INSERT INTO monitoring_share_targets (
                                share_id, client_id, public_client_key
                            )
                            SELECT $1, target.client_id, target.public_client_key
                            FROM unnest($2::TEXT[], $3::TEXT[])
                                AS target(client_id, public_client_key)
                            "#,
                        )
                        .bind(replacement.expected_share.id)
                        .bind(
                            targets
                                .iter()
                                .map(|target| target.client_id.clone())
                                .collect::<Vec<_>>(),
                        )
                        .bind(
                            targets
                                .iter()
                                .map(|target| target.public_client_key.clone())
                                .collect::<Vec<_>>(),
                        )
                        .execute(&mut *tx)
                        .await?;
                    }
                    sqlx::query(
                        "UPDATE monitoring_share_links SET updated_at = now() WHERE id = $1",
                    )
                    .bind(replacement.expected_share.id)
                    .execute(&mut *tx)
                    .await?;
                }
                insert_monitoring_audit(
                    &mut tx,
                    Some(operator.operator.id),
                    "monitoring_share.targets_bulk_updated",
                    "monitoring_shares:bulk",
                    share_target_updates_audit_metadata(replacements, operator),
                )
                .await?;
                tx.commit().await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn extend_monitoring_shares(
        &self,
        share_ids: &[Uuid],
        extend_by_secs: u64,
        operator: &AuthContext,
    ) -> Result<Vec<MonitoringShareView>> {
        let ids = share_ids.iter().copied().collect::<BTreeSet<_>>();
        if ids.is_empty() {
            bail!("monitoring_share_selection_required");
        }
        if ids.len() > 1_000 {
            bail!("monitoring_share_selection_too_large");
        }
        let now = crate::unix_now();
        let maximum = now.saturating_add(365 * 24 * 60 * 60);
        match self {
            Self::Memory(memory) => {
                let mut records = memory.monitoring_shares.write().await;
                let selected = records
                    .iter()
                    .filter(|record| ids.contains(&record.id))
                    .collect::<Vec<_>>();
                if selected.len() != ids.len() {
                    bail!("monitoring_share_not_found");
                }
                if selected
                    .iter()
                    .any(|record| monitoring_share_status(record, now) != "active")
                {
                    bail!("monitoring_share_not_active");
                }
                for record in records.iter_mut().filter(|record| ids.contains(&record.id)) {
                    let current = parse_timestamp_unix(&record.expires_at).unwrap_or(now);
                    record.expires_at = current
                        .max(now)
                        .saturating_add(extend_by_secs)
                        .min(maximum)
                        .to_string();
                    record.updated_at = now.to_string();
                }
                drop(records);
                record_memory_monitoring_audit(
                    memory,
                    operator,
                    "monitoring_share.extended",
                    "monitoring_shares:bulk".to_string(),
                    base_monitoring_audit_metadata(
                        operator,
                        serde_json::json!({
                            "share_ids": ids,
                            "extend_by_secs": extend_by_secs,
                        }),
                    ),
                )
                .await;
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let current = sqlx::query(
                    r#"
                    SELECT
                        id,
                        extract(epoch FROM expires_at)::bigint AS expires_unix,
                        revoked_at IS NOT NULL AS revoked
                    FROM monitoring_share_links
                    WHERE id = ANY($1::UUID[])
                    FOR UPDATE
                    "#,
                )
                .bind(ids.iter().copied().collect::<Vec<_>>())
                .fetch_all(&mut *tx)
                .await?;
                if current.len() != ids.len() {
                    bail!("monitoring_share_not_found");
                }
                for row in current {
                    let id: Uuid = row.try_get("id")?;
                    let expires_unix = row.try_get::<i64, _>("expires_unix")?.max(0) as u64;
                    let revoked: bool = row.try_get("revoked")?;
                    if revoked || expires_unix <= now {
                        bail!("monitoring_share_not_active");
                    }
                    let next = expires_unix
                        .max(now)
                        .saturating_add(extend_by_secs)
                        .min(maximum);
                    sqlx::query(
                        "UPDATE monitoring_share_links SET expires_at = to_timestamp($2), updated_at = now() WHERE id = $1",
                    )
                    .bind(id)
                    .bind(next as f64)
                    .execute(&mut *tx)
                    .await?;
                }
                insert_monitoring_audit(
                    &mut tx,
                    Some(operator.operator.id),
                    "monitoring_share.extended",
                    "monitoring_shares:bulk",
                    base_monitoring_audit_metadata(
                        operator,
                        serde_json::json!({
                            "share_ids": ids,
                            "extend_by_secs": extend_by_secs,
                        }),
                    ),
                )
                .await?;
                tx.commit().await?;
            }
        }
        self.monitoring_share_views_for_ids(&ids).await
    }

    pub(crate) async fn revoke_monitoring_shares(
        &self,
        share_ids: &[Uuid],
        operator: &AuthContext,
    ) -> Result<Vec<MonitoringShareView>> {
        let ids = share_ids.iter().copied().collect::<BTreeSet<_>>();
        if ids.is_empty() {
            bail!("monitoring_share_selection_required");
        }
        if ids.len() > 1_000 {
            bail!("monitoring_share_selection_too_large");
        }
        let now_unix = crate::unix_now();
        let now = now_unix.to_string();
        match self {
            Self::Memory(memory) => {
                let mut records = memory.monitoring_shares.write().await;
                let selected = records
                    .iter()
                    .filter(|record| ids.contains(&record.id))
                    .collect::<Vec<_>>();
                if selected.len() != ids.len() {
                    bail!("monitoring_share_not_found");
                }
                if selected
                    .iter()
                    .any(|record| monitoring_share_status(record, now_unix) != "active")
                {
                    bail!("monitoring_share_not_active");
                }
                for record in records.iter_mut().filter(|record| ids.contains(&record.id)) {
                    record.revoked_at = Some(now.clone());
                    record.revoked_by = Some(operator.operator.id);
                    record.updated_at = now.clone();
                }
                drop(records);
                record_memory_monitoring_audit(
                    memory,
                    operator,
                    "monitoring_share.revoked",
                    "monitoring_shares:bulk".to_string(),
                    base_monitoring_audit_metadata(operator, serde_json::json!({"share_ids": ids})),
                )
                .await;
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let current = sqlx::query(
                    r#"
                    SELECT
                        id,
                        extract(epoch FROM expires_at)::bigint AS expires_unix,
                        revoked_at IS NOT NULL AS revoked
                    FROM monitoring_share_links
                    WHERE id = ANY($1::UUID[])
                    FOR UPDATE
                    "#,
                )
                .bind(ids.iter().copied().collect::<Vec<_>>())
                .fetch_all(&mut *tx)
                .await?;
                if current.len() != ids.len() {
                    bail!("monitoring_share_not_found");
                }
                for row in &current {
                    let revoked: bool = row.try_get("revoked")?;
                    let expires_unix: i64 = row.try_get("expires_unix")?;
                    if revoked || expires_unix <= now_unix as i64 {
                        bail!("monitoring_share_not_active");
                    }
                }
                let result = sqlx::query(
                    r#"
                    UPDATE monitoring_share_links
                    SET revoked_at = now(),
                        revoked_by = $2,
                        updated_at = now()
                    WHERE id = ANY($1::UUID[])
                    "#,
                )
                .bind(ids.iter().copied().collect::<Vec<_>>())
                .bind(operator.operator.id)
                .execute(&mut *tx)
                .await?;
                if result.rows_affected() != ids.len() as u64 {
                    bail!("monitoring_share_not_found");
                }
                insert_monitoring_audit(
                    &mut tx,
                    Some(operator.operator.id),
                    "monitoring_share.revoked",
                    "monitoring_shares:bulk",
                    base_monitoring_audit_metadata(operator, serde_json::json!({"share_ids": ids})),
                )
                .await?;
                tx.commit().await?;
            }
        }
        self.monitoring_share_views_for_ids(&ids).await
    }

    pub(crate) async fn authenticate_monitoring_share(
        &self,
        share_id: Uuid,
        secret: &str,
    ) -> Result<Option<MonitoringShareRecord>> {
        let Some(record) = self.monitoring_share_record(share_id).await? else {
            return Ok(None);
        };
        if !constant_time_eq(secret.as_bytes(), record.token_secret.as_bytes()) {
            return Ok(None);
        }
        Ok(Some(record))
    }

    pub(crate) async fn record_monitoring_share_visitor(
        &self,
        share: &MonitoringShareRecord,
        proposed_visitor_id: Option<Uuid>,
        source_ip: &str,
        user_agent: Option<&str>,
    ) -> Result<(Uuid, bool)> {
        let visitor_id = proposed_visitor_id.unwrap_or_else(Uuid::new_v4);
        let user_agent = user_agent.map(|value| truncate(value, 512));
        let now = crate::unix_now().to_string();
        match self {
            Self::Memory(memory) => {
                let mut visitors = memory.monitoring_share_visitors.write().await;
                if let Some(visitor) = visitors.iter_mut().find(|visitor| {
                    visitor.share_id == share.id && visitor.visitor_id == visitor_id
                }) {
                    visitor.last_seen_at = now;
                    visitor.source_ip = Some(source_ip.to_string());
                    visitor.user_agent = user_agent;
                    return Ok((visitor_id, false));
                }
                visitors.push(MonitoringShareVisitorRecord {
                    share_id: share.id,
                    visitor_id,
                    source_ip: Some(source_ip.to_string()),
                    user_agent: user_agent.clone(),
                    first_seen_at: now.clone(),
                    last_seen_at: now,
                });
                drop(visitors);
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: None,
                    action: "monitoring_share.visitor_opened".to_string(),
                    target: format!("monitoring_share:{}", share.id),
                    command_hash: None,
                    metadata: share_visitor_audit_metadata(
                        share,
                        visitor_id,
                        source_ip,
                        user_agent.as_deref(),
                    ),
                    created_at: crate::unix_now().to_string(),
                });
                Ok((visitor_id, true))
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let inserted = sqlx::query_scalar::<_, Uuid>(
                    r#"
                    INSERT INTO monitoring_share_visitors (
                        share_id, visitor_id, source_ip, user_agent, first_seen_at, last_seen_at
                    ) VALUES ($1, $2, $3::inet, $4, now(), now())
                    ON CONFLICT (share_id, visitor_id) DO NOTHING
                    RETURNING visitor_id
                    "#,
                )
                .bind(share.id)
                .bind(visitor_id)
                .bind(source_ip)
                .bind(user_agent.as_deref())
                .fetch_optional(&mut *tx)
                .await?;
                let is_new = inserted.is_some();
                if !is_new {
                    sqlx::query(
                        r#"
                        UPDATE monitoring_share_visitors
                        SET source_ip = $3::inet, user_agent = $4, last_seen_at = now()
                        WHERE share_id = $1 AND visitor_id = $2
                        "#,
                    )
                    .bind(share.id)
                    .bind(visitor_id)
                    .bind(source_ip)
                    .bind(user_agent.as_deref())
                    .execute(&mut *tx)
                    .await?;
                } else {
                    insert_monitoring_audit(
                        &mut tx,
                        None,
                        "monitoring_share.visitor_opened",
                        &format!("monitoring_share:{}", share.id),
                        share_visitor_audit_metadata(
                            share,
                            visitor_id,
                            source_ip,
                            user_agent.as_deref(),
                        ),
                    )
                    .await?;
                }
                tx.commit().await?;
                Ok((visitor_id, is_new))
            }
        }
    }

    pub(crate) async fn touch_monitoring_share_visitor(
        &self,
        share_id: Uuid,
        visitor_id: Uuid,
    ) -> Result<bool> {
        match self {
            Self::Memory(memory) => {
                let mut visitors = memory.monitoring_share_visitors.write().await;
                let Some(visitor) = visitors.iter_mut().find(|visitor| {
                    visitor.share_id == share_id && visitor.visitor_id == visitor_id
                }) else {
                    return Ok(false);
                };
                visitor.last_seen_at = crate::unix_now().to_string();
                Ok(true)
            }
            Self::Postgres(pool) => Ok(sqlx::query(
                r#"
                UPDATE monitoring_share_visitors
                SET last_seen_at = now()
                WHERE share_id = $1 AND visitor_id = $2
                "#,
            )
            .bind(share_id)
            .bind(visitor_id)
            .execute(pool)
            .await?
            .rows_affected()
                == 1),
        }
    }
}

pub(crate) async fn upsert_postgres_ping_results(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    observed_unix: u64,
    results: &[PingTargetResult],
) -> Result<()> {
    for result in results.iter().take(MAX_AGENT_PING_TARGETS) {
        if !valid_ping_result(result, observed_unix) {
            continue;
        }
        let Ok(target_id) = Uuid::parse_str(result.target_id.trim()) else {
            continue;
        };
        let accepted: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM ping_targets t
                JOIN ping_target_assignments a ON a.target_id = t.id
                WHERE
                    t.id = $1
                    AND a.client_id = $2
                    AND t.enabled
                    AND t.generation = $3
            )
            "#,
        )
        .bind(target_id)
        .bind(client_id)
        .bind(result.generation as i64)
        .fetch_one(&mut **tx)
        .await?;
        if !accepted {
            continue;
        }
        let success_count = i32::from(result.latency_avg_ms.is_some());
        sqlx::query(
            r#"
            INSERT INTO telemetry_ping_rollups (
                client_id, target_id, generation, bucket_start, bucket_secs,
                sample_count, success_count,
                latency_avg_ms, latency_min_ms, latency_max_ms,
                loss_ratio_avg, loss_ratio_max,
                latest_status, latest_reason, latest_checked_at, updated_at
            ) VALUES (
                $1, $2, $3, to_timestamp($4::double precision), 60,
                1, $5, $6, $6, $6, $7, $7, $8, $9,
                to_timestamp($10::double precision), now()
            )
            ON CONFLICT (client_id, target_id, generation, bucket_secs, bucket_start) DO UPDATE SET
                sample_count = telemetry_ping_rollups.sample_count + 1,
                success_count = telemetry_ping_rollups.success_count + EXCLUDED.success_count,
                latency_avg_ms = CASE
                    WHEN telemetry_ping_rollups.success_count + EXCLUDED.success_count = 0 THEN NULL
                    ELSE (
                        COALESCE(telemetry_ping_rollups.latency_avg_ms, 0)
                            * telemetry_ping_rollups.success_count::double precision
                        + COALESCE(EXCLUDED.latency_avg_ms, 0)
                            * EXCLUDED.success_count::double precision
                    ) / (telemetry_ping_rollups.success_count + EXCLUDED.success_count)::double precision
                END,
                latency_min_ms = CASE
                    WHEN telemetry_ping_rollups.latency_min_ms IS NULL THEN EXCLUDED.latency_min_ms
                    WHEN EXCLUDED.latency_min_ms IS NULL THEN telemetry_ping_rollups.latency_min_ms
                    ELSE LEAST(telemetry_ping_rollups.latency_min_ms, EXCLUDED.latency_min_ms)
                END,
                latency_max_ms = CASE
                    WHEN telemetry_ping_rollups.latency_max_ms IS NULL THEN EXCLUDED.latency_max_ms
                    WHEN EXCLUDED.latency_max_ms IS NULL THEN telemetry_ping_rollups.latency_max_ms
                    ELSE GREATEST(telemetry_ping_rollups.latency_max_ms, EXCLUDED.latency_max_ms)
                END,
                loss_ratio_avg = (
                    telemetry_ping_rollups.loss_ratio_avg * telemetry_ping_rollups.sample_count::double precision
                    + EXCLUDED.loss_ratio_avg
                ) / (telemetry_ping_rollups.sample_count + 1)::double precision,
                loss_ratio_max = GREATEST(
                    telemetry_ping_rollups.loss_ratio_max,
                    EXCLUDED.loss_ratio_max
                ),
                latest_status = EXCLUDED.latest_status,
                latest_reason = EXCLUDED.latest_reason,
                latest_checked_at = EXCLUDED.latest_checked_at,
                updated_at = now()
            WHERE telemetry_ping_rollups.latest_checked_at < EXCLUDED.latest_checked_at
            "#,
        )
        .bind(client_id)
        .bind(target_id)
        .bind(result.generation as i64)
        .bind((result.checked_unix / 60 * 60) as f64)
        .bind(success_count)
        .bind(result.latency_avg_ms)
        .bind(result.loss_ratio)
        .bind(&result.status)
        .bind(result.reason.as_deref().map(|reason| truncate(reason, 512)))
        .bind(result.checked_unix as f64)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

pub(crate) async fn accepted_postgres_ping_results(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    observed_unix: u64,
    results: &[PingTargetResult],
) -> Result<Vec<PingTargetResult>> {
    let candidates = results
        .iter()
        .take(MAX_AGENT_PING_TARGETS)
        .filter_map(|result| {
            let target_id = Uuid::parse_str(result.target_id.trim()).ok()?;
            valid_ping_result(result, observed_unix).then_some((
                target_id,
                result.generation as i64,
                result,
            ))
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let target_ids = candidates
        .iter()
        .map(|(target_id, _, _)| *target_id)
        .collect::<Vec<_>>();
    let accepted = sqlx::query(
        r#"
        SELECT t.id, t.generation
        FROM ping_targets t
        JOIN ping_target_assignments a ON a.target_id = t.id
        WHERE a.client_id = $1
          AND t.enabled
          AND t.id = ANY($2::UUID[])
        "#,
    )
    .bind(client_id)
    .bind(target_ids)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .map(|row| {
        Ok((
            row.try_get::<Uuid, _>("id")?,
            row.try_get::<i64, _>("generation")?,
        ))
    })
    .collect::<Result<HashSet<_>>>()?;
    Ok(candidates
        .into_iter()
        .filter(|(target_id, generation, _)| accepted.contains(&(*target_id, *generation)))
        .map(|(_, _, result)| result.clone())
        .collect())
}

fn ping_target_view(
    record: &PingTargetRecord,
    assigned: &HashMap<Uuid, Vec<String>>,
    primary: &HashMap<Uuid, usize>,
) -> PingTargetView {
    let target_client_ids = assigned.get(&record.id).cloned().unwrap_or_default();
    PingTargetView {
        id: record.id,
        name: record.name.clone(),
        host: record.host.clone(),
        probe_kind: record.probe_kind.clone(),
        port: record.port,
        enabled: record.enabled,
        selector_expression: record.selector_expression.clone(),
        generation: record.generation,
        assigned_count: target_client_ids.len(),
        target_client_ids,
        primary_count: primary.get(&record.id).copied().unwrap_or(0),
        runtime_sync: PingTargetRuntimeSyncView {
            state: "unknown".to_string(),
            reason: "No durable runtime evidence is available".to_string(),
        },
        target_update_available: false,
        created_at: record.created_at.clone(),
        updated_at: record.updated_at.clone(),
    }
}

fn current_ping_view(
    target: &PingTargetRecord,
    latest: Option<&PingRollupView>,
    rolling_loss_ratio: Option<f64>,
) -> CurrentPingView {
    let loss_ratio = rolling_loss_ratio.or_else(|| latest.map(|rollup| rollup.loss_ratio_avg));
    let status = latest.map(|rollup| current_ping_status(&rollup.latest_status, loss_ratio));
    CurrentPingView {
        target_id: target.id,
        target_name: target.name.clone(),
        enabled: target.enabled,
        generation: target.generation,
        state: if !target.enabled {
            "disabled".to_string()
        } else {
            status.clone().unwrap_or_else(|| "pending".to_string())
        },
        status,
        latency_avg_ms: latest.and_then(|rollup| rollup.latency_avg_ms),
        loss_ratio,
        reason: latest.and_then(|rollup| rollup.latest_reason.clone()),
        checked_at: latest.map(|rollup| rollup.latest_checked_at.clone()),
    }
}

fn current_ping_status(latest_status: &str, rolling_loss_ratio: Option<f64>) -> String {
    if matches!(latest_status, "down" | "error") {
        return latest_status.to_string();
    }
    if rolling_loss_ratio
        .is_some_and(|loss_ratio| loss_ratio + f64::EPSILON >= CURRENT_PING_DEGRADED_LOSS_RATIO)
    {
        "degraded".to_string()
    } else {
        "ok".to_string()
    }
}

fn current_ping_loss_ratio<'a>(
    latest: &PingRollupView,
    rollups: impl Iterator<Item = &'a PingRollupView>,
) -> Option<f64> {
    let latest_checked_at = parse_timestamp_unix(&latest.latest_checked_at)?;
    let window_start = if latest_checked_at >= CURRENT_PING_LOSS_WINDOW_SECS {
        latest_checked_at - (CURRENT_PING_LOSS_WINDOW_SECS - 1)
    } else {
        0
    };
    let mut sample_count = 0_i64;
    let mut loss_weighted_total = 0.0;
    for rollup in rollups {
        let Some(bucket_start) = parse_timestamp_unix(&rollup.bucket_start) else {
            continue;
        };
        for fragment in logical_span_fragments(
            bucket_start,
            rollup.bucket_secs,
            Some(window_start),
            Some(latest_checked_at),
            CURRENT_PING_LOSS_WINDOW_SECS as i32,
        ) {
            let fragment_samples = proportional_fragment_count(rollup.sample_count, fragment);
            sample_count = sample_count.saturating_add(i64::from(fragment_samples));
            loss_weighted_total += rollup.loss_ratio_avg * f64::from(fragment_samples);
        }
    }
    (sample_count > 0).then_some(loss_weighted_total / sample_count as f64)
}

fn ping_target_record_from_row(row: sqlx::postgres::PgRow) -> Result<PingTargetRecord> {
    Ok(PingTargetRecord {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        host: row.try_get("host")?,
        probe_kind: row.try_get("probe_kind")?,
        port: row.try_get("port")?,
        enabled: row.try_get("enabled")?,
        selector_expression: row.try_get("selector_expression")?,
        generation: row.try_get("generation")?,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn next_memory_ping_assignments(
    existing: &[PingTargetAssignmentRecord],
    visible_client_ids: &BTreeSet<String>,
    enabled_targets: &BTreeSet<Uuid>,
    target_id: Uuid,
    target_client_ids: &[String],
) -> Result<Vec<PingTargetAssignmentRecord>> {
    let mut counts = HashMap::<String, usize>::new();
    for assignment in existing.iter().filter(|assignment| {
        visible_client_ids.contains(&assignment.client_id)
            && assignment.target_id != target_id
            && enabled_targets.contains(&assignment.target_id)
    }) {
        *counts.entry(assignment.client_id.clone()).or_default() += 1;
    }
    if enabled_targets.contains(&target_id) {
        for client_id in target_client_ids {
            let count = counts.entry(client_id.clone()).or_default();
            *count += 1;
            if *count > MAX_AGENT_PING_TARGETS {
                bail!("ping_targets_per_client_too_many:{client_id}");
            }
        }
    }
    let existing_primary = existing
        .iter()
        .filter(|assignment| {
            visible_client_ids.contains(&assignment.client_id)
                && assignment.target_id == target_id
                && assignment.is_primary
        })
        .map(|assignment| assignment.client_id.clone())
        .collect::<BTreeSet<_>>();
    let now = crate::unix_now().to_string();
    let mut assignments = existing
        .iter()
        .filter(|assignment| {
            !visible_client_ids.contains(&assignment.client_id) || assignment.target_id != target_id
        })
        .cloned()
        .collect::<Vec<_>>();
    assignments.extend(
        target_client_ids
            .iter()
            .map(|client_id| PingTargetAssignmentRecord {
                target_id,
                client_id: client_id.clone(),
                is_primary: existing_primary.contains(client_id),
                assigned_at: now.clone(),
            }),
    );
    Ok(assignments)
}

async fn replace_postgres_ping_assignments(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    target_id: Uuid,
    target_client_ids: &[String],
) -> Result<()> {
    sqlx::query(
        r#"
        DELETE FROM ping_target_assignments assignment
        USING visible_clients client
        WHERE client.id = assignment.client_id
          AND assignment.target_id = $1
          AND NOT (assignment.client_id = ANY($2::TEXT[]))
        "#,
    )
    .bind(target_id)
    .bind(target_client_ids)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO ping_target_assignments (target_id, client_id)
        SELECT $1, value FROM unnest($2::TEXT[]) AS value
        ON CONFLICT (target_id, client_id) DO NOTHING
        "#,
    )
    .bind(target_id)
    .bind(target_client_ids)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn lock_postgres_ping_targets(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    target_ids: &[Uuid],
) -> Result<Vec<PingTargetRecord>> {
    if target_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut target_ids = target_ids.to_vec();
    target_ids.sort();
    target_ids.dedup();
    let rows = sqlx::query(
        r#"
        SELECT
            id, name, host, probe_kind, port, enabled,
            selector_expression, generation, created_by, updated_by,
            created_at::text AS created_at,
            updated_at::text AS updated_at
        FROM ping_targets
        WHERE id = ANY($1::UUID[])
        ORDER BY id
        FOR UPDATE
        "#,
    )
    .bind(&target_ids)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter().map(ping_target_record_from_row).collect()
}

async fn lock_postgres_ping_clients(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    target_ids: &[Uuid],
    proposed_client_ids: &[String],
) -> Result<Vec<String>> {
    lock_postgres_agent_identity_lifecycle(tx).await?;
    let proposed_client_ids = normalized_client_ids(proposed_client_ids)
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut client_ids = proposed_client_ids.iter().cloned().collect::<Vec<_>>();
    let existing = sqlx::query_scalar::<_, String>(
        r#"
        SELECT DISTINCT assignment.client_id
        FROM ping_target_assignments assignment
        JOIN visible_clients client ON client.id = assignment.client_id
        WHERE assignment.target_id = ANY($1::UUID[])
        ORDER BY assignment.client_id
        "#,
    )
    .bind(target_ids)
    .fetch_all(&mut **tx)
    .await?;
    client_ids.extend(existing);
    client_ids.sort();
    client_ids.dedup();
    if client_ids.is_empty() {
        return Ok(client_ids);
    }
    require_visible_postgres_clients_in_tx(tx, &client_ids, "ping_target_resolution_stale").await?;
    Ok(client_ids)
}

async fn visible_memory_ping_client_ids(
    memory: &crate::repository::MemoryState,
) -> BTreeSet<String> {
    let hidden = memory.hidden_clients.read().await;
    memory
        .agents
        .read()
        .await
        .iter()
        .filter(|agent| !hidden.contains(&agent.id))
        .map(|agent| agent.id.clone())
        .collect()
}

async fn ensure_postgres_ping_capacity(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<()> {
    let exceeded: Option<String> = sqlx::query_scalar(
        r#"
        SELECT client_id
        FROM ping_target_assignments a
        JOIN ping_targets t ON t.id = a.target_id
        JOIN visible_clients client ON client.id = a.client_id
        WHERE t.enabled
        GROUP BY client_id
        HAVING count(*) > $1
        ORDER BY client_id
        LIMIT 1
        "#,
    )
    .bind(MAX_AGENT_PING_TARGETS as i64)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(client_id) = exceeded {
        bail!("ping_targets_per_client_too_many:{client_id}");
    }
    Ok(())
}

fn normalized_client_ids(client_ids: &[String]) -> Vec<String> {
    client_ids
        .iter()
        .map(|client_id| client_id.trim())
        .filter(|client_id| !client_id.is_empty())
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn same_ping_target_revision(left: &PingTargetRecord, right: &PingTargetRecord) -> bool {
    left.id == right.id
        && left.name == right.name
        && left.host == right.host
        && left.probe_kind == right.probe_kind
        && left.port == right.port
        && left.enabled == right.enabled
        && left.selector_expression == right.selector_expression
        && left.generation == right.generation
        && left.created_by == right.created_by
        && left.created_at == right.created_at
        && left.updated_at == right.updated_at
}

fn changed_ping_assignment_clients(
    replacements: &[PingTargetAssignmentReplacement],
) -> Vec<String> {
    let mut affected = BTreeSet::new();
    for replacement in replacements {
        let current = normalized_client_ids(&replacement.expected_client_ids)
            .into_iter()
            .collect::<BTreeSet<_>>();
        let next = normalized_client_ids(&replacement.next_client_ids)
            .into_iter()
            .collect::<BTreeSet<_>>();
        affected.extend(current.symmetric_difference(&next).cloned());
    }
    affected.into_iter().collect()
}

fn valid_ping_result(result: &PingTargetResult, observed_unix: u64) -> bool {
    result.generation > 0
        && result.checked_unix > 0
        && result.checked_unix <= observed_unix.saturating_add(300)
        && observed_unix.saturating_sub(result.checked_unix) <= 3_900
        && result.values_are_coherent()
        && result.loss_ratio.is_finite()
        && (0.0..=1.0).contains(&result.loss_ratio)
        && result
            .latency_avg_ms
            .is_none_or(|latency| latency.is_finite() && (0.0..=3_600_000.0).contains(&latency))
        && result
            .reason
            .as_ref()
            .is_none_or(|reason| reason.len() <= 4096)
}

#[derive(Default)]
struct MemoryPingAggregate {
    client_id: String,
    target_id: Uuid,
    target_name: String,
    is_primary: bool,
    generation: i64,
    sample_count: i64,
    success_count: i64,
    latency_weighted_total: f64,
    latency_min_ms: Option<f64>,
    latency_max_ms: Option<f64>,
    loss_weighted_total: f64,
    loss_ratio_max: f64,
    latest_status: String,
    latest_reason: Option<String>,
    latest_checked_at: String,
}

fn aggregate_memory_ping_rollups(rows: Vec<PingRollupView>, step_secs: i32) -> Vec<PingRollupView> {
    let step_secs = step_secs.max(60) as u64;
    let mut groups = std::collections::BTreeMap::<(Uuid, i64, u64), MemoryPingAggregate>::new();
    for row in rows {
        let timestamp = parse_timestamp_unix(&row.bucket_start).unwrap_or(0);
        let chart_bucket = timestamp / step_secs * step_secs;
        let aggregate = groups
            .entry((row.target_id, row.generation, chart_bucket))
            .or_default();
        aggregate.client_id = row.client_id.clone();
        aggregate.target_id = row.target_id;
        aggregate.target_name = row.target_name.clone();
        aggregate.is_primary |= row.is_primary;
        aggregate.generation = row.generation;
        let sample_count = i64::from(row.sample_count.max(0));
        let success_count = i64::from(row.success_count.max(0));
        aggregate.sample_count = aggregate.sample_count.saturating_add(sample_count);
        aggregate.success_count = aggregate.success_count.saturating_add(success_count);
        if let Some(latency) = row.latency_avg_ms {
            aggregate.latency_weighted_total += latency * success_count as f64;
        }
        if let Some(latency) = row.latency_min_ms {
            aggregate.latency_min_ms = Some(
                aggregate
                    .latency_min_ms
                    .map_or(latency, |current| current.min(latency)),
            );
        }
        if let Some(latency) = row.latency_max_ms {
            aggregate.latency_max_ms = Some(
                aggregate
                    .latency_max_ms
                    .map_or(latency, |current| current.max(latency)),
            );
        }
        aggregate.loss_weighted_total += row.loss_ratio_avg * sample_count as f64;
        aggregate.loss_ratio_max = aggregate.loss_ratio_max.max(row.loss_ratio_max);
        if aggregate.latest_checked_at.is_empty()
            || parse_timestamp_unix(&row.latest_checked_at)
                > parse_timestamp_unix(&aggregate.latest_checked_at)
        {
            aggregate.latest_status = row.latest_status;
            aggregate.latest_reason = row.latest_reason;
            aggregate.latest_checked_at = row.latest_checked_at;
        }
    }
    groups
        .into_iter()
        .map(
            |((_target_id, _generation, bucket_start), aggregate)| PingRollupView {
                client_id: aggregate.client_id,
                target_id: aggregate.target_id,
                target_name: aggregate.target_name,
                is_primary: aggregate.is_primary,
                generation: aggregate.generation,
                bucket_start: bucket_start.to_string(),
                bucket_secs: step_secs as i32,
                sample_count: aggregate.sample_count.min(i64::from(i32::MAX)) as i32,
                success_count: aggregate.success_count.min(i64::from(i32::MAX)) as i32,
                latency_avg_ms: (aggregate.success_count > 0)
                    .then_some(aggregate.latency_weighted_total / aggregate.success_count as f64),
                latency_min_ms: aggregate.latency_min_ms,
                latency_max_ms: aggregate.latency_max_ms,
                loss_ratio_avg: if aggregate.sample_count > 0 {
                    aggregate.loss_weighted_total / aggregate.sample_count as f64
                } else {
                    0.0
                },
                loss_ratio_max: aggregate.loss_ratio_max,
                latest_status: aggregate.latest_status,
                latest_reason: aggregate.latest_reason,
                latest_checked_at: aggregate.latest_checked_at,
            },
        )
        .collect()
}

fn retain_fair_ping_points(
    rows: &mut Vec<PingRollupView>,
    points_per_target: usize,
    total_limit: usize,
) {
    rows.sort_by(|left, right| {
        left.target_id
            .cmp(&right.target_id)
            .then_with(|| left.generation.cmp(&right.generation))
            .then_with(|| {
                parse_timestamp_unix(&right.bucket_start)
                    .cmp(&parse_timestamp_unix(&left.bucket_start))
            })
    });
    let mut counts = HashMap::<(Uuid, i64), usize>::new();
    let mut ranked = std::mem::take(rows)
        .into_iter()
        .filter_map(|row| {
            let count = counts.entry((row.target_id, row.generation)).or_default();
            let rank = *count;
            *count = count.saturating_add(1);
            (rank < points_per_target).then_some((rank, row))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|(left_rank, left), (right_rank, right)| {
        left_rank
            .cmp(right_rank)
            .then_with(|| {
                parse_timestamp_unix(&right.bucket_start)
                    .cmp(&parse_timestamp_unix(&left.bucket_start))
            })
            .then_with(|| left.target_id.cmp(&right.target_id))
            .then_with(|| left.generation.cmp(&right.generation))
    });
    ranked.truncate(total_limit);
    rows.extend(ranked.into_iter().map(|(_, row)| row));
    rows.sort_by(|left, right| {
        parse_timestamp_unix(&left.bucket_start)
            .cmp(&parse_timestamp_unix(&right.bucket_start))
            .then_with(|| left.target_name.cmp(&right.target_name))
            .then_with(|| left.target_id.cmp(&right.target_id))
            .then_with(|| left.generation.cmp(&right.generation))
    });
}

fn fragment_ping_rollup(
    row: PingRollupView,
    start_unix: Option<u64>,
    end_unix: Option<u64>,
    step_secs: i32,
) -> Vec<PingRollupView> {
    let Some(bucket_start) = parse_timestamp_unix(&row.bucket_start) else {
        return Vec::new();
    };
    let chart_step_secs = step_secs.max(60).saturating_add(59) / 60 * 60;
    logical_span_fragments(
        bucket_start,
        row.bucket_secs,
        start_unix,
        end_unix,
        chart_step_secs,
    )
    .into_iter()
    .filter_map(|fragment: LogicalSpanFragment| {
        let sample_count = proportional_fragment_count(row.sample_count, fragment);
        if sample_count == 0 {
            return None;
        }
        let success_count =
            proportional_fragment_count(row.success_count, fragment).min(sample_count);
        let latest_checked_at = fragment_final_minute_timestamp(&row.latest_checked_at, fragment);
        Some(PingRollupView {
            bucket_start: fragment.chart_bucket_start.to_string(),
            bucket_secs: chart_step_secs,
            sample_count,
            success_count,
            latest_checked_at,
            ..row.clone()
        })
    })
    .collect()
}

fn upsert_memory_ping_rollup(
    stored: &mut Vec<PingRollupView>,
    client_id: &str,
    target: &PingTargetRecord,
    result: &PingTargetResult,
) {
    let bucket_start = result.checked_unix / 60 * 60;
    if let Some(row) = stored.iter_mut().find(|row| {
        row.client_id == client_id
            && row.target_id == target.id
            && row.generation == target.generation
            && row.bucket_start == bucket_start.to_string()
    }) {
        if parse_timestamp_unix(&row.latest_checked_at)
            .is_some_and(|checked| checked >= result.checked_unix)
        {
            return;
        }
        let prior_samples = row.sample_count.max(1);
        let prior_successes = row.success_count.max(0);
        row.sample_count = row.sample_count.saturating_add(1);
        if let Some(latency) = result.latency_avg_ms {
            row.latency_avg_ms = Some(match row.latency_avg_ms {
                Some(current) if prior_successes > 0 => {
                    (current * f64::from(prior_successes) + latency)
                        / f64::from(prior_successes + 1)
                }
                _ => latency,
            });
            row.latency_min_ms = Some(
                row.latency_min_ms
                    .map_or(latency, |value| value.min(latency)),
            );
            row.latency_max_ms = Some(
                row.latency_max_ms
                    .map_or(latency, |value| value.max(latency)),
            );
            row.success_count = row.success_count.saturating_add(1);
        }
        row.loss_ratio_avg = (row.loss_ratio_avg * f64::from(prior_samples) + result.loss_ratio)
            / f64::from(prior_samples + 1);
        row.loss_ratio_max = row.loss_ratio_max.max(result.loss_ratio);
        row.latest_status = result.status.clone();
        row.latest_reason = result.reason.as_deref().map(|reason| truncate(reason, 512));
        row.latest_checked_at = result.checked_unix.to_string();
        return;
    }
    stored.push(PingRollupView {
        client_id: client_id.to_string(),
        target_id: target.id,
        target_name: target.name.clone(),
        is_primary: false,
        generation: target.generation,
        bucket_start: bucket_start.to_string(),
        bucket_secs: 60,
        sample_count: 1,
        success_count: i32::from(result.latency_avg_ms.is_some()),
        latency_avg_ms: result.latency_avg_ms,
        latency_min_ms: result.latency_avg_ms,
        latency_max_ms: result.latency_avg_ms,
        loss_ratio_avg: result.loss_ratio,
        loss_ratio_max: result.loss_ratio,
        latest_status: result.status.clone(),
        latest_reason: result.reason.as_deref().map(|reason| truncate(reason, 512)),
        latest_checked_at: result.checked_unix.to_string(),
    });
}

fn ping_rollup_from_row(row: sqlx::postgres::PgRow) -> Result<PingRollupView> {
    Ok(PingRollupView {
        client_id: row.try_get("client_id")?,
        target_id: row.try_get("target_id")?,
        target_name: row.try_get("target_name")?,
        is_primary: row.try_get("is_primary")?,
        generation: row.try_get("generation")?,
        bucket_start: row.try_get("bucket_start")?,
        bucket_secs: row.try_get("bucket_secs")?,
        sample_count: row.try_get("sample_count")?,
        success_count: row.try_get("success_count")?,
        latency_avg_ms: row.try_get("latency_avg_ms")?,
        latency_min_ms: row.try_get("latency_min_ms")?,
        latency_max_ms: row.try_get("latency_max_ms")?,
        loss_ratio_avg: row.try_get("loss_ratio_avg")?,
        loss_ratio_max: row.try_get("loss_ratio_max")?,
        latest_status: row.try_get("latest_status")?,
        latest_reason: row.try_get("latest_reason")?,
        latest_checked_at: row.try_get("latest_checked_at")?,
    })
}

async fn record_memory_monitoring_audit(
    memory: &crate::repository::MemoryState,
    operator: &AuthContext,
    action: &str,
    target: String,
    metadata: serde_json::Value,
) {
    memory.audits.write().await.push(AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: action.to_string(),
        target,
        command_hash: None,
        metadata,
        created_at: crate::unix_now().to_string(),
    });
}

pub(crate) async fn insert_monitoring_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_id: Option<Uuid>,
    action: &str,
    target: &str,
    metadata: serde_json::Value,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
        VALUES ($1, $2, $3, $4, NULL, $5)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(actor_id)
    .bind(action)
    .bind(target)
    .bind(metadata)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn ping_target_audit_metadata(
    record: &PingTargetRecord,
    target_client_ids: &[String],
    operator: &AuthContext,
) -> serde_json::Value {
    base_monitoring_audit_metadata(
        operator,
        serde_json::json!({
            "target_id": record.id,
            "name": record.name,
            "host": record.host,
            "probe_kind": record.probe_kind,
            "port": record.port,
            "enabled": record.enabled,
            "generation": record.generation,
            "selector_expression": record.selector_expression,
            "target_client_ids": target_client_ids,
            "target_count": target_client_ids.len(),
        }),
    )
}

pub(crate) fn base_monitoring_audit_metadata(
    operator: &AuthContext,
    details: serde_json::Value,
) -> serde_json::Value {
    let serde_json::Value::Object(mut metadata) = details else {
        panic!("monitoring audit details must be a JSON object");
    };
    metadata.insert("result".to_string(), serde_json::json!("succeeded"));
    metadata.insert(
        "origin_kind".to_string(),
        serde_json::json!("operator_request"),
    );
    metadata.insert(
        "component".to_string(),
        serde_json::json!("monitoring-controller"),
    );
    metadata.insert(
        "operator_id".to_string(),
        serde_json::json!(operator.operator.id),
    );
    metadata.insert(
        "operator_username".to_string(),
        serde_json::json!(operator.operator.username),
    );
    metadata.insert(
        "operator_role".to_string(),
        serde_json::json!(operator.operator.role),
    );
    metadata.insert(
        "operator_session_id".to_string(),
        serde_json::json!(operator.audit_session_id()),
    );
    serde_json::Value::Object(metadata)
}

fn required_timestamp_f64(value: &str) -> Result<f64> {
    parse_timestamp_unix(value)
        .map(|timestamp| timestamp as f64)
        .context("monitoring timestamp is invalid")
}

fn monitoring_timestamp_unix(value: &str) -> u64 {
    parse_timestamp_unix(value).unwrap_or(0)
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

async fn postgres_monitoring_share_views(
    pool: &sqlx::PgPool,
    status: Option<&str>,
    ids: Option<Vec<Uuid>>,
    limit: i64,
    offset: i64,
) -> Result<Vec<MonitoringShareView>> {
    let rows = sqlx::query(
        r#"
        SELECT
            s.id,
            s.name,
            s.selector_expression,
            target_stats.target_count,
            target_stats.target_client_ids,
            s.show_identity_context,
            s.show_billing,
            s.show_system_information,
            s.show_resources,
            s.show_network,
            s.show_traffic,
            s.show_ping,
            s.allow_detail_history,
            CASE
                WHEN s.revoked_at IS NOT NULL THEN 'revoked'
                WHEN s.expires_at <= now() THEN 'expired'
                ELSE 'active'
            END AS status,
            s.expires_at::text AS expires_at,
            s.revoked_at::text AS revoked_at,
            creator.username AS created_by,
            s.created_at::text AS created_at,
            s.updated_at::text AS updated_at,
            visitor_stats.visitor_count,
            visitor_stats.first_visited_at::text AS first_visited_at,
            visitor_stats.last_visited_at::text AS last_visited_at
        FROM monitoring_share_links s
        LEFT JOIN operators creator ON creator.id = s.created_by
        CROSS JOIN LATERAL (
            SELECT
                count(*)::bigint AS target_count,
                COALESCE(
                    array_agg(target.client_id ORDER BY target.client_id),
                    ARRAY[]::TEXT[]
                ) AS target_client_ids
            FROM monitoring_share_targets target
            WHERE target.share_id = s.id
        ) target_stats
        CROSS JOIN LATERAL (
            SELECT
                count(*)::bigint AS visitor_count,
                min(visitor.first_seen_at) AS first_visited_at,
                max(visitor.last_seen_at) AS last_visited_at
            FROM monitoring_share_visitors visitor
            WHERE visitor.share_id = s.id
        ) visitor_stats
        WHERE (
            $1::text IS NULL
            OR CASE
                WHEN s.revoked_at IS NOT NULL THEN 'revoked'
                WHEN s.expires_at <= now() THEN 'expired'
                ELSE 'active'
            END = $1
        )
          AND ($2::uuid[] IS NULL OR s.id = ANY($2))
        ORDER BY s.created_at DESC, s.id
        LIMIT $3 OFFSET $4
        "#,
    )
    .bind(status)
    .bind(ids)
    .bind(limit.clamp(1, 1_000))
    .bind(offset.clamp(0, 1_000_000))
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            let target_count: i64 = row.try_get("target_count")?;
            let visitor_count: i64 = row.try_get("visitor_count")?;
            let target_count = usize::try_from(target_count)
                .context("monitoring share target count is invalid")?;
            let visitor_count = usize::try_from(visitor_count)
                .context("monitoring share visitor count is invalid")?;
            Ok(MonitoringShareView {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
                selector_expression: row.try_get("selector_expression")?,
                target_count,
                target_client_ids: row.try_get("target_client_ids")?,
                target_update_available: false,
                visibility: MonitoringShareVisibilityView {
                    identity_context: row.try_get("show_identity_context")?,
                    billing: row.try_get("show_billing")?,
                    system_information: row.try_get("show_system_information")?,
                    resources: row.try_get("show_resources")?,
                    network: row.try_get("show_network")?,
                    traffic: row.try_get("show_traffic")?,
                    ping: row.try_get("show_ping")?,
                    detail_history: row.try_get("allow_detail_history")?,
                },
                status: row.try_get("status")?,
                expires_at: row.try_get("expires_at")?,
                revoked_at: row.try_get("revoked_at")?,
                created_by: row.try_get("created_by")?,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
                visitor_count,
                first_visited_at: row.try_get("first_visited_at")?,
                last_visited_at: row.try_get("last_visited_at")?,
            })
        })
        .collect()
}

fn monitoring_share_record_from_row(row: sqlx::postgres::PgRow) -> Result<MonitoringShareRecord> {
    let target_client_ids: Vec<String> = row.try_get("target_client_ids")?;
    let target_public_client_keys: Vec<String> = row.try_get("target_public_client_keys")?;
    anyhow::ensure!(
        target_client_ids.len() == target_public_client_keys.len(),
        "monitoring share target identity arrays are inconsistent"
    );
    let targets = target_client_ids
        .into_iter()
        .zip(target_public_client_keys)
        .map(
            |(client_id, public_client_key)| MonitoringShareTargetRecord {
                client_id,
                public_client_key,
            },
        )
        .collect::<Vec<_>>();
    validate_monitoring_share_targets(&targets)?;
    Ok(MonitoringShareRecord {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        token_secret: row.try_get("token_secret")?,
        selector_expression: row.try_get("selector_expression")?,
        targets,
        visibility: MonitoringShareVisibilityView {
            identity_context: row.try_get("show_identity_context")?,
            billing: row.try_get("show_billing")?,
            system_information: row.try_get("show_system_information")?,
            resources: row.try_get("show_resources")?,
            network: row.try_get("show_network")?,
            traffic: row.try_get("show_traffic")?,
            ping: row.try_get("show_ping")?,
            detail_history: row.try_get("allow_detail_history")?,
        },
        expires_at: row.try_get("expires_at")?,
        revoked_at: row.try_get("revoked_at")?,
        revoked_by: row.try_get("revoked_by")?,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn monitoring_share_view(
    record: &MonitoringShareRecord,
    visitors: &[MonitoringShareVisitorRecord],
    created_by: Option<String>,
) -> MonitoringShareView {
    let share_visitors = visitors
        .iter()
        .filter(|visitor| visitor.share_id == record.id)
        .collect::<Vec<_>>();
    MonitoringShareView {
        id: record.id,
        name: record.name.clone(),
        selector_expression: record.selector_expression.clone(),
        target_count: record.targets.len(),
        target_client_ids: record.target_client_ids(),
        target_update_available: false,
        visibility: record.visibility.clone(),
        status: monitoring_share_status(record, crate::unix_now()).to_string(),
        expires_at: record.expires_at.clone(),
        revoked_at: record.revoked_at.clone(),
        created_by,
        created_at: record.created_at.clone(),
        updated_at: record.updated_at.clone(),
        visitor_count: share_visitors.len(),
        first_visited_at: share_visitors
            .iter()
            .map(|visitor| visitor.first_seen_at.clone())
            .min(),
        last_visited_at: share_visitors
            .iter()
            .map(|visitor| visitor.last_seen_at.clone())
            .max(),
    }
}

pub(crate) fn monitoring_share_status(record: &MonitoringShareRecord, now: u64) -> &'static str {
    if record.revoked_at.is_some() {
        "revoked"
    } else {
        match parse_timestamp_unix(&record.expires_at) {
            Some(expires) if expires > now => "active",
            Some(_) | None => "expired",
        }
    }
}

fn same_monitoring_share_revision(
    stored: &MonitoringShareRecord,
    expected: &MonitoringShareRecord,
) -> bool {
    stored.id == expected.id
        && stored.name == expected.name
        && stored.token_secret == expected.token_secret
        && stored.selector_expression == expected.selector_expression
        && stored.targets == expected.targets
        && stored.visibility == expected.visibility
        && stored.expires_at == expected.expires_at
        && stored.revoked_at == expected.revoked_at
        && stored.updated_at == expected.updated_at
}

fn share_operator_audit_metadata(
    record: &MonitoringShareRecord,
    operator: &AuthContext,
) -> serde_json::Value {
    let target_client_ids = record.target_client_ids();
    base_monitoring_audit_metadata(
        operator,
        serde_json::json!({
            "share_id": record.id,
            "name": record.name,
            "selector_expression": record.selector_expression,
            "target_client_ids": target_client_ids,
            "target_count": record.targets.len(),
            "visibility": record.visibility,
            "expires_at": record.expires_at,
        }),
    )
}

fn share_target_updates_audit_metadata(
    replacements: &[MonitoringShareTargetReplacement],
    operator: &AuthContext,
) -> serde_json::Value {
    let changes = replacements
        .iter()
        .filter_map(|replacement| {
            let before = normalized_client_ids(&replacement.expected_share.target_client_ids());
            let after = normalized_client_ids(&replacement.next_client_ids);
            if before == after {
                return None;
            }
            let before_set = before.iter().cloned().collect::<BTreeSet<_>>();
            let after_set = after.iter().cloned().collect::<BTreeSet<_>>();
            Some(serde_json::json!({
                "share_id": replacement.expected_share.id,
                "name": replacement.expected_share.name,
                "selector_expression": replacement.expected_share.selector_expression,
                "before_target_count": before.len(),
                "after_target_count": after.len(),
                "added_client_ids": after_set.difference(&before_set).cloned().collect::<Vec<_>>(),
                "removed_client_ids": before_set.difference(&after_set).cloned().collect::<Vec<_>>(),
            }))
        })
        .collect::<Vec<_>>();
    let share_ids = changes
        .iter()
        .filter_map(|change| change.get("share_id").cloned())
        .collect::<Vec<_>>();
    base_monitoring_audit_metadata(
        operator,
        serde_json::json!({
            "share_ids": share_ids,
            "changes": changes,
        }),
    )
}

fn share_visitor_audit_metadata(
    share: &MonitoringShareRecord,
    visitor_id: Uuid,
    remote_ip: &str,
    user_agent: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "result": "succeeded",
        "origin_kind": "public_share",
        "component": "monitoring-share-controller",
        "share_id": share.id,
        "visitor_id": visitor_id,
        "remote_ip": remote_ip,
        "user_agent": user_agent,
        "target_count": share.targets.len(),
        "visibility": share.visibility,
    })
}

fn system_information_view(
    os_release: Option<&str>,
    architecture: Option<&str>,
    cpu_model: Option<&str>,
    kernel_release: Option<&str>,
    virtualization: Option<&str>,
    reported_at: Option<String>,
    uptime_secs: Option<u64>,
    uptime_observed_at: Option<String>,
) -> Option<SystemInformationView> {
    let view = SystemInformationView {
        os_name: os_release.and_then(public_os_name),
        architecture: architecture.and_then(normalize_public_system_fact),
        cpu_model: cpu_model.and_then(normalize_public_system_fact),
        kernel_release: kernel_release.and_then(normalize_public_system_fact),
        virtualization: virtualization.and_then(normalize_public_system_fact),
        reported_at,
        uptime_secs,
        uptime_observed_at,
    };
    (view.os_name.is_some()
        || view.architecture.is_some()
        || view.cpu_model.is_some()
        || view.kernel_release.is_some()
        || view.virtualization.is_some()
        || view.uptime_secs.is_some())
    .then_some(view)
}

fn public_os_name(os_release: &str) -> Option<String> {
    let mut pretty_name = None;
    let mut name = None;
    let mut version = None;
    for line in os_release.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, raw_value)) = line.split_once('=') else {
            continue;
        };
        let value = raw_value.trim();
        let value = if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            &value[1..value.len() - 1]
        } else {
            value
        };
        match key.trim() {
            "PRETTY_NAME" => pretty_name = normalize_public_system_fact(value),
            "NAME" => name = normalize_public_system_fact(value),
            "VERSION" | "VERSION_ID" if version.is_none() => {
                version = normalize_public_system_fact(value)
            }
            _ => {}
        }
    }
    pretty_name.or_else(|| match (name, version) {
        (Some(name), Some(version)) => Some(format!("{name} {version}")),
        (Some(name), None) => Some(name),
        _ => None,
    })
}

fn normalize_public_system_fact(value: &str) -> Option<String> {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    (!value.is_empty()
        && value.len() <= 255
        && !value.chars().any(|character| character.is_control()))
    .then_some(value)
}

fn validate_monitoring_share_targets(targets: &[MonitoringShareTargetRecord]) -> Result<()> {
    anyhow::ensure!(
        targets.len() <= 1_000,
        "monitoring_share_target_count_too_large"
    );
    let mut client_ids = HashSet::with_capacity(targets.len());
    let mut public_client_keys = HashSet::with_capacity(targets.len());
    for target in targets {
        anyhow::ensure!(
            !target.client_id.trim().is_empty() && client_ids.insert(target.client_id.as_str()),
            "monitoring_share_target_client_id_invalid"
        );
        anyhow::ensure!(
            target.public_client_key.len() == 64
                && target
                    .public_client_key
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
                && public_client_keys.insert(target.public_client_key.as_str()),
            "monitoring_share_public_client_key_invalid"
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests_repository_monitoring.rs"]
mod tests;
