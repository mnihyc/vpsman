use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{types::Json as SqlJson, Row};
use uuid::Uuid;

use crate::{
    model::{AgentView, AuditLogView, AuthContext, FleetAlertView, TelemetryRollupView},
    model_alert_policies::{
        CreateFleetAlertPolicyRequest, PolicyAlertQuery, PolicyAlertRecord, PolicyDryRunRequest,
        PolicyDryRunResponse, PolicyDryRunRulePreview, PolicyGroupRecord, PolicyRuleRecord,
        PolicyRuleRequest, PolicyRuleStateRecord, TrafficAccountingQuery, TrafficAccountingRecord,
        TrafficAccountingSelectorBreakdown, TrafficCounterSampleRecord, VpsRuleChangePreview,
        VpsRuleQuery, VpsRuleValueRecord, VpsRulesBulkUnsetRequest, VpsRulesBulkUpsertRequest,
        VpsRulesDryRunRequest, VpsRulesDryRunResponse, VPS_RULE_KEY_TRAFFIC_QUOTA_RX,
        VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL, VPS_RULE_KEY_TRAFFIC_QUOTA_TX,
        VPS_RULE_KEY_TRAFFIC_RESET_DAY, VPS_RULE_KEY_TRAFFIC_SELECTORS,
    },
    model_webhook_rules::WebhookEventCandidate,
    repository::Repository,
    repository_webhook_rules::{record_webhook_event_in_tx, webhook_event_row},
    selector_expression::{agent_matches_selector_expression, parse_selector_expression},
    unix_now,
    util::{compare_timestamps_desc, timestamp_in_optional_bounds},
};

const MAX_POLICY_NAME_BYTES: usize = 128;
const MAX_POLICY_NOTES_BYTES: usize = 1024;
const MAX_RULE_NAME_BYTES: usize = 128;
const MAX_SELECTOR_EXPRESSION_BYTES: usize = 4096;
const MAX_CONDITION_EXPRESSION_BYTES: usize = 4096;
const MAX_VPS_RULE_VALUE_BYTES: usize = 4096;
const MAX_TRAFFIC_SELECTOR_ITEMS: usize = 16;
const MAX_TRAFFIC_INTERFACE_BYTES: usize = 128;
const TRAFFIC_SAMPLE_STALE_SECS: i64 = 900;
const POLICY_WEBHOOK_REPAIR_WINDOW_SECS: i64 = 3600;
static POLICY_EVALUATION_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn policy_alert_severity_rank(severity: &str) -> usize {
    match severity {
        "critical" => 0,
        "warning" => 1,
        "info" => 2,
        _ => 3,
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct TrafficSelector {
    source: String,
    interface: String,
    direction: String,
    canonical: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct TrafficStreamRequest {
    client_id: String,
    source_kind: String,
    interface: String,
    cycle_start_unix: i64,
}

#[derive(Clone, Debug)]
struct TrafficCounterStreamUsage {
    client_id: String,
    source_kind: String,
    interface: String,
    cycle_rx: i64,
    cycle_tx: i64,
    latest_rx: i64,
    latest_tx: i64,
    last_sample_unix: i64,
    counter_epochs_seen: i64,
}

#[derive(Clone, Debug)]
struct ParsedRuleValue {
    raw: String,
    json: Value,
    display: String,
}

#[derive(Clone, Debug)]
struct PolicyEvaluation {
    condition_true: bool,
    incomplete: bool,
    incomplete_reasons: Vec<String>,
    actual_value: Option<f64>,
    threshold_value: Option<f64>,
    category: String,
    payload: Value,
}

impl Repository {
    pub(crate) async fn list_vps_rules(
        &self,
        query: &VpsRuleQuery,
    ) -> Result<Vec<VpsRuleValueRecord>> {
        let result_limit = query.limit.unwrap_or(1000).clamp(1, 5000) as usize;
        self.list_vps_rules_matching(query, Some(result_limit))
            .await
    }

    async fn list_vps_rules_matching(
        &self,
        query: &VpsRuleQuery,
        result_limit: Option<usize>,
    ) -> Result<Vec<VpsRuleValueRecord>> {
        let allowed_clients = if let Some(selector) = query.selector_expression.as_deref() {
            let agents = self.list_agents().await?;
            Some(
                resolve_agents(&agents, selector)?
                    .into_iter()
                    .map(|agent| agent.id)
                    .collect::<HashSet<_>>(),
            )
        } else {
            None
        };
        let allowed_client_ids = allowed_clients
            .as_ref()
            .map(|clients| clients.iter().cloned().collect::<Vec<_>>());
        // `state` is derived while parsing persisted values, so PostgreSQL may
        // apply a result limit only when every requested filter is represented
        // in SQL. Otherwise rows are state-filtered before the API limit below.
        let database_limit = query
            .state
            .is_none()
            .then_some(result_limit)
            .flatten()
            .map(|limit| limit as i64);
        let mut rows = match self {
            Self::Memory(memory) => memory.vps_rule_values.read().await.clone(),
            Self::Postgres(pool) => sqlx::query(
                r#"
                SELECT
                    client_id,
                    key,
                    value_raw,
                    value_json,
                    source_kind,
                    source_id,
                    updated_by,
                    updated_at::text AS updated_at
                FROM vps_rule_values
                WHERE ($1::text IS NULL OR client_id = $1)
                  AND ($2::text IS NULL OR key = $2)
                  AND ($3::text[] IS NULL OR client_id = ANY($3))
                ORDER BY client_id ASC, key ASC
                LIMIT $4
                "#,
            )
            .bind(query.client_id.as_deref())
            .bind(query.key.as_deref())
            .bind(allowed_client_ids.as_deref())
            .bind(database_limit)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(vps_rule_from_row)
            .collect::<Result<Vec<_>>>()?,
        };
        rows.retain(|row| {
            query
                .client_id
                .as_deref()
                .is_none_or(|client_id| row.client_id == client_id)
                && query.key.as_deref().is_none_or(|key| row.key == key)
                && query
                    .state
                    .as_deref()
                    .is_none_or(|state| row.state == state)
                && allowed_clients
                    .as_ref()
                    .is_none_or(|clients| clients.contains(&row.client_id))
        });
        rows.sort_by(|left, right| {
            left.client_id
                .cmp(&right.client_id)
                .then_with(|| left.key.cmp(&right.key))
        });
        if let Some(result_limit) = result_limit {
            rows.truncate(result_limit);
        }
        Ok(rows)
    }

    pub(crate) async fn effective_vps_rules(
        &self,
        client_id: &str,
    ) -> Result<Vec<VpsRuleValueRecord>> {
        self.list_vps_rules_matching(
            &VpsRuleQuery {
                limit: None,
                client_id: Some(client_id.to_string()),
                selector_expression: None,
                key: None,
                state: None,
            },
            None,
        )
        .await
    }

    pub(crate) async fn dry_run_vps_rules(
        &self,
        request: &VpsRulesDryRunRequest,
    ) -> Result<VpsRulesDryRunResponse> {
        let operation = request.operation.trim().to_ascii_lowercase();
        anyhow::ensure!(
            operation == "upsert" || operation == "unset",
            "vps_rules_operation_invalid"
        );
        if operation == "upsert" {
            validate_vps_rule_values(&request.values)?;
        } else {
            validate_vps_rule_keys(&request.keys)?;
        }
        self.vps_rule_preview(
            &operation,
            &request.selector_expression,
            &request.values,
            &request.keys,
        )
        .await
    }

    pub(crate) async fn bulk_upsert_vps_rules(
        &self,
        request: &VpsRulesBulkUpsertRequest,
        operator: &AuthContext,
    ) -> Result<VpsRulesDryRunResponse> {
        anyhow::ensure!(request.confirmed, "vps_rules_confirmation_required");
        validate_vps_rule_values(&request.values)?;
        let preview = self
            .vps_rule_preview("upsert", &request.selector_expression, &request.values, &[])
            .await?;
        anyhow::ensure!(
            preview.preview_hash == request.preview_hash,
            "vps_rules_preview_hash_mismatch"
        );
        self.apply_vps_rule_changes(&preview, operator).await?;
        if let Err(error) = self.evaluate_policy_rules().await {
            tracing::warn!(%error, "deferred policy evaluation after VPS rule update");
        }
        Ok(preview)
    }

    pub(crate) async fn bulk_unset_vps_rules(
        &self,
        request: &VpsRulesBulkUnsetRequest,
        operator: &AuthContext,
    ) -> Result<VpsRulesDryRunResponse> {
        anyhow::ensure!(request.confirmed, "vps_rules_confirmation_required");
        validate_vps_rule_keys(&request.keys)?;
        let preview = self
            .vps_rule_preview(
                "unset",
                &request.selector_expression,
                &BTreeMap::new(),
                &request.keys,
            )
            .await?;
        anyhow::ensure!(
            preview.preview_hash == request.preview_hash,
            "vps_rules_preview_hash_mismatch"
        );
        self.apply_vps_rule_changes(&preview, operator).await?;
        if let Err(error) = self.evaluate_policy_rules().await {
            tracing::warn!(%error, "deferred policy evaluation after VPS rule removal");
        }
        Ok(preview)
    }

    pub(crate) async fn list_traffic_accounting(
        &self,
        query: &TrafficAccountingQuery,
    ) -> Result<Vec<TrafficAccountingRecord>> {
        self.list_traffic_accounting_at(query, Utc::now()).await
    }

    async fn list_traffic_accounting_at(
        &self,
        query: &TrafficAccountingQuery,
        now: DateTime<Utc>,
    ) -> Result<Vec<TrafficAccountingRecord>> {
        let agents = self.list_agents().await?;
        let mut selected_agents = if let Some(selector) = query.selector_expression.as_deref() {
            resolve_agents(&agents, selector)?
        } else {
            agents
        };
        if let Some(client_id) = query.client_id.as_deref() {
            selected_agents.retain(|agent| agent.id == client_id);
        }
        let rules = self
            .list_vps_rules_matching(
                &VpsRuleQuery {
                    limit: None,
                    client_id: None,
                    selector_expression: None,
                    key: None,
                    state: None,
                },
                None,
            )
            .await?;
        let cycle_starts = traffic_cycle_starts_for_clients(
            selected_agents.iter().map(|agent| agent.id.as_str()),
            &rules,
            now,
        );
        let stream_requests = traffic_stream_requests_from_rules(&cycle_starts, &rules)
            .into_iter()
            .collect::<Vec<_>>();
        let traffic_usage = self
            .list_traffic_counter_usage_for_streams(&stream_requests, now.timestamp())
            .await?;
        let mut records =
            traffic_accounting_for_agents(&selected_agents, &rules, &traffic_usage, now);
        records.retain(|record| {
            query
                .state
                .as_deref()
                .is_none_or(|state| record.state == state)
        });
        records.sort_by(|left, right| left.client_id.cmp(&right.client_id));
        records.truncate(query.limit.unwrap_or(1000).clamp(1, 5000) as usize);
        Ok(records)
    }

    pub(crate) async fn get_traffic_accounting(
        &self,
        client_id: &str,
    ) -> Result<TrafficAccountingRecord> {
        self.list_traffic_accounting(&TrafficAccountingQuery {
            selector_expression: None,
            client_id: Some(client_id.to_string()),
            state: None,
            limit: Some(1),
        })
        .await?
        .into_iter()
        .next()
        .context("traffic_accounting_not_found")
    }

    pub(crate) async fn dry_run_fleet_alert_policy(
        &self,
        request: &PolicyDryRunRequest,
    ) -> Result<PolicyDryRunResponse> {
        validate_policy_group_request(
            &CreateFleetAlertPolicyRequest {
                id: request.id,
                name: request.name.clone(),
                enabled: request.enabled,
                selector_expression: request.selector_expression.clone(),
                rules: request.rules.clone(),
                notes: request.notes.clone(),
                confirmed: true,
                preview_hash: None,
            },
            false,
            false,
        )?;
        let agents = self.list_agents().await?;
        let matched = resolve_agents(&agents, &request.selector_expression)?;
        let now = Utc::now();
        let mut validation_errors = Vec::new();
        let rules = self
            .list_vps_rules_matching(
                &VpsRuleQuery {
                    limit: None,
                    client_id: None,
                    selector_expression: None,
                    key: None,
                    state: None,
                },
                None,
            )
            .await?;
        let cycle_starts = traffic_cycle_starts_for_clients(
            matched.iter().map(|agent| agent.id.as_str()),
            &rules,
            now,
        );
        let mut stream_requests = traffic_stream_requests_from_rules(&cycle_starts, &rules);
        for rule in &request.rules {
            if policy_condition_uses_traffic(&rule.condition_expression).unwrap_or(false) {
                if let Some(selector) = rule.traffic_selector.as_deref() {
                    add_traffic_selector_requests(
                        &mut stream_requests,
                        matched.iter().map(|agent| agent.id.as_str()),
                        &cycle_starts,
                        selector,
                    );
                }
            }
        }
        let stream_requests = stream_requests.into_iter().collect::<Vec<_>>();
        let traffic_usage = self
            .list_traffic_counter_usage_for_streams(&stream_requests, now.timestamp())
            .await?;
        let traffic = traffic_accounting_for_agents(&matched, &rules, &traffic_usage, now);
        let rollup_client_ids = matched
            .iter()
            .map(|agent| agent.id.clone())
            .collect::<Vec<_>>();
        let rollups = latest_rollups(
            self.list_latest_telemetry_rollups_for_clients(&rollup_client_ids, None)
                .await?,
        );
        let traffic_by_client = traffic
            .iter()
            .map(|record| (record.client_id.clone(), record))
            .collect::<HashMap<_, _>>();
        let mut rule_previews = Vec::new();
        let mut incomplete_clients = BTreeSet::new();
        for rule in &request.rules {
            match validate_policy_rule_request_for_preview(rule) {
                Ok(()) => {}
                Err(error) => {
                    validation_errors.push(error.to_string());
                    continue;
                }
            }
            let mut true_count = 0;
            let mut false_count = 0;
            let mut incomplete_count = 0;
            for agent in &matched {
                let override_traffic =
                    traffic_override_for_rule(&agent.id, rule, &rules, &traffic_usage, now);
                let traffic_record = override_traffic
                    .as_ref()
                    .or_else(|| traffic_by_client.get(&agent.id).copied());
                let evaluation =
                    evaluate_rule_for_client(rule, traffic_record, rollups.get(&agent.id));
                if evaluation.incomplete {
                    incomplete_count += 1;
                    incomplete_clients.insert(agent.id.clone());
                } else if evaluation.condition_true {
                    true_count += 1;
                } else {
                    false_count += 1;
                }
            }
            rule_previews.push(PolicyDryRunRulePreview {
                rule_name: rule.name.clone(),
                condition_expression: rule.condition_expression.clone(),
                category: policy_rule_category(rule),
                severity: rule.severity.clone(),
                true_count,
                false_count,
                incomplete_count,
            });
        }
        let preview_payload = json!({
            "name": request.name,
            "enabled": request.enabled,
            "selector_expression": request.selector_expression,
            "rules": request.rules,
            "matched": matched.iter().map(|agent| &agent.id).collect::<Vec<_>>(),
            "validation_errors": validation_errors,
        });
        Ok(PolicyDryRunResponse {
            matched_vps_count: matched.len(),
            invalid_rule_count: validation_errors.len(),
            incomplete_vps_count: incomplete_clients.len(),
            preview_hash: preview_hash(&preview_payload),
            matched_vps: matched.into_iter().map(|agent| agent.id).collect(),
            rule_previews,
            validation_errors,
        })
    }

    pub(crate) async fn list_fleet_alert_policies(
        &self,
        limit: i64,
        enabled: Option<bool>,
        selector_expression: Option<&str>,
        client_id: Option<&str>,
    ) -> Result<Vec<PolicyGroupRecord>> {
        let definition_limit = if selector_expression.is_none() && client_id.is_none() {
            Some(limit.clamp(1, 1000) as usize)
        } else {
            None
        };
        let mut groups = self
            .list_fleet_alert_policy_definitions(definition_limit, enabled)
            .await?;
        if let Some(enabled) = enabled {
            groups.retain(|group| group.enabled == enabled);
        }
        if let Some(selector) = selector_expression {
            let agents = self.list_agents().await?;
            let selected = resolve_agents(&agents, selector)?
                .into_iter()
                .map(|agent| agent.id)
                .collect::<HashSet<_>>();
            groups.retain(|group| {
                resolve_agents(&agents, &group.selector_expression)
                    .map(|matched| {
                        matched
                            .into_iter()
                            .any(|agent| selected.contains(&agent.id))
                    })
                    .unwrap_or(false)
            });
        }
        if let Some(client_id) = client_id {
            let agents = self.list_agents().await?;
            groups.retain(|group| {
                resolve_agents(&agents, &group.selector_expression)
                    .map(|matched| matched.into_iter().any(|agent| agent.id == client_id))
                    .unwrap_or(false)
            });
        }
        self.enrich_policy_group_summaries(&mut groups).await?;
        groups.sort_by(|left, right| {
            right
                .enabled
                .cmp(&left.enabled)
                .then_with(|| left.name.cmp(&right.name))
        });
        groups.truncate(limit.clamp(1, 1000) as usize);
        Ok(groups)
    }

    async fn list_fleet_alert_policy_definitions(
        &self,
        result_limit: Option<usize>,
        enabled: Option<bool>,
    ) -> Result<Vec<PolicyGroupRecord>> {
        let mut groups = match self {
            Self::Memory(memory) => memory.policy_groups.read().await.clone(),
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        name,
                        enabled,
                        selector_expression,
                        notes,
                        created_by,
                        updated_by,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM policy_groups
                    WHERE ($2::boolean IS NULL OR enabled = $2)
                    ORDER BY enabled DESC, name ASC
                    LIMIT $1
                    "#,
                )
                .bind(result_limit.map(|limit| limit as i64))
                .bind(enabled)
                .fetch_all(pool)
                .await?;
                let group_ids = rows
                    .iter()
                    .map(|row| row.try_get::<Uuid, _>("id"))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let mut rules_by_group = HashMap::<Uuid, Vec<PolicyRuleRecord>>::new();
                if !group_ids.is_empty() {
                    for row in sqlx::query(
                        r#"
                        SELECT
                            id,
                            group_id,
                            rule_version,
                            sort_order,
                            name,
                            enabled,
                            traffic_selector,
                            condition_expression,
                            window_secs,
                            severity,
                            created_at::text AS created_at,
                            updated_at::text AS updated_at
                        FROM policy_rules
                        WHERE group_id = ANY($1)
                        ORDER BY group_id ASC, sort_order ASC, created_at ASC
                        "#,
                    )
                    .bind(&group_ids)
                    .fetch_all(pool)
                    .await?
                    {
                        let rule = policy_rule_from_row(row)?;
                        rules_by_group.entry(rule.group_id).or_default().push(rule);
                    }
                }
                let mut groups = Vec::new();
                for row in rows {
                    let group_id: Uuid = row.try_get("id")?;
                    groups.push(policy_group_from_row(
                        row,
                        rules_by_group.remove(&group_id).unwrap_or_default(),
                    )?);
                }
                groups
            }
        };
        if let Some(enabled) = enabled {
            groups.retain(|group| group.enabled == enabled);
        }
        groups.sort_by(|left, right| {
            right
                .enabled
                .cmp(&left.enabled)
                .then_with(|| left.name.cmp(&right.name))
        });
        if let Some(result_limit) = result_limit {
            groups.truncate(result_limit);
        }
        Ok(groups)
    }

    async fn get_fleet_alert_policy_definition(&self, id: Uuid) -> Result<PolicyGroupRecord> {
        match self {
            Self::Memory(memory) => memory
                .policy_groups
                .read()
                .await
                .iter()
                .find(|group| group.id == id)
                .cloned()
                .context("fleet_alert_policy_not_found"),
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        id,
                        name,
                        enabled,
                        selector_expression,
                        notes,
                        created_by,
                        updated_by,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM policy_groups
                    WHERE id = $1
                    "#,
                )
                .bind(id)
                .fetch_optional(pool)
                .await?
                .context("fleet_alert_policy_not_found")?;
                let rules = sqlx::query(
                    r#"
                    SELECT
                        id,
                        group_id,
                        rule_version,
                        sort_order,
                        name,
                        enabled,
                        traffic_selector,
                        condition_expression,
                        window_secs,
                        severity,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM policy_rules
                    WHERE group_id = $1
                    ORDER BY sort_order ASC, created_at ASC
                    "#,
                )
                .bind(id)
                .fetch_all(pool)
                .await?
                .into_iter()
                .map(policy_rule_from_row)
                .collect::<Result<Vec<_>>>()?;
                policy_group_from_row(row, rules)
            }
        }
    }

    pub(crate) async fn get_fleet_alert_policy(&self, id: Uuid) -> Result<PolicyGroupRecord> {
        let mut groups = vec![self.get_fleet_alert_policy_definition(id).await?];
        self.enrich_policy_group_summaries(&mut groups).await?;
        groups.pop().context("fleet_alert_policy_not_found")
    }

    pub(crate) async fn upsert_fleet_alert_policy(
        &self,
        request: &CreateFleetAlertPolicyRequest,
        operator: &AuthContext,
    ) -> Result<PolicyGroupRecord> {
        validate_policy_group_request(request, true, true)?;
        let dry_run = self
            .dry_run_fleet_alert_policy(&PolicyDryRunRequest {
                id: request.id,
                name: request.name.clone(),
                enabled: request.enabled,
                selector_expression: request.selector_expression.clone(),
                rules: request.rules.clone(),
                notes: request.notes.clone(),
            })
            .await?;
        if let Some(hash) = request.preview_hash.as_deref() {
            anyhow::ensure!(
                hash == dry_run.preview_hash,
                "fleet_alert_policy_preview_hash_mismatch"
            );
        }
        let now = unix_now().to_string();
        let group = match self {
            Self::Memory(memory) => {
                let mut groups = memory.policy_groups.write().await;
                let existing_group =
                    select_existing_policy_group(&groups, request.id, request.name.trim())?;
                let group = policy_group_from_request(
                    request,
                    &dry_run,
                    &now,
                    existing_group.as_ref(),
                    operator,
                )?;
                let scope_changed = policy_group_scope_changed(existing_group.as_ref(), &group);
                let mut states = memory.policy_rule_states.write().await;
                if let Some(existing) = existing_group.as_ref() {
                    let existing_rule_ids = existing
                        .rules
                        .iter()
                        .map(|rule| rule.id)
                        .collect::<HashSet<_>>();
                    states.retain(|state| {
                        if !existing_rule_ids.contains(&state.policy_rule_id) {
                            return true;
                        }
                        !scope_changed
                            && group.rules.iter().any(|rule| {
                                rule.id == state.policy_rule_id
                                    && rule.rule_version == state.rule_version
                            })
                    });
                }
                groups.retain(|stored| stored.id != group.id && stored.name != group.name);
                groups.push(group.clone());
                drop(states);
                drop(groups);
                memory.audits.write().await.push(policy_group_audit(
                    "fleet.alert_policy_upserted",
                    &group,
                    operator,
                    now.clone(),
                ));
                group
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_policy_group_identity_upserts_in_tx(&mut tx).await?;
                let existing_groups =
                    policy_groups_for_identity_in_tx(&mut tx, request.id, request.name.trim())
                        .await?;
                let existing_group = select_existing_policy_group(
                    &existing_groups,
                    request.id,
                    request.name.trim(),
                )?;
                let group = policy_group_from_request(
                    request,
                    &dry_run,
                    &now,
                    existing_group.as_ref(),
                    operator,
                )?;
                let scope_changed = policy_group_scope_changed(existing_group.as_ref(), &group);
                sqlx::query(
                    r#"
                    INSERT INTO policy_groups (
                        id, name, enabled, selector_expression, notes, created_by, updated_by
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $6)
                    ON CONFLICT (id) DO UPDATE SET
                        name = EXCLUDED.name,
                        enabled = EXCLUDED.enabled,
                        selector_expression = EXCLUDED.selector_expression,
                        notes = EXCLUDED.notes,
                        updated_by = EXCLUDED.updated_by,
                        updated_at = now()
                    "#,
                )
                .bind(group.id)
                .bind(&group.name)
                .bind(group.enabled)
                .bind(&group.selector_expression)
                .bind(&group.notes)
                .bind(operator.operator.id)
                .execute(&mut *tx)
                .await?;
                let retained_rule_ids = group.rules.iter().map(|rule| rule.id).collect::<Vec<_>>();
                sqlx::query(
                    r#"
                    DELETE FROM policy_rules
                    WHERE group_id = $1
                      AND NOT (id = ANY($2::uuid[]))
                    "#,
                )
                .bind(group.id)
                .bind(&retained_rule_ids)
                .execute(&mut *tx)
                .await?;
                for rule in &group.rules {
                    let result = sqlx::query(
                        r#"
                        INSERT INTO policy_rules (
                            id, group_id, rule_version, sort_order, name, enabled,
                            traffic_selector, condition_expression, window_secs, severity
                        )
                        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
                        ON CONFLICT (id) DO UPDATE SET
                            rule_version = EXCLUDED.rule_version,
                            sort_order = EXCLUDED.sort_order,
                            name = EXCLUDED.name,
                            enabled = EXCLUDED.enabled,
                            traffic_selector = EXCLUDED.traffic_selector,
                            condition_expression = EXCLUDED.condition_expression,
                            window_secs = EXCLUDED.window_secs,
                            severity = EXCLUDED.severity,
                            updated_at = now()
                        WHERE policy_rules.group_id = EXCLUDED.group_id
                        "#,
                    )
                    .bind(rule.id)
                    .bind(rule.group_id)
                    .bind(rule.rule_version)
                    .bind(rule.sort_order)
                    .bind(&rule.name)
                    .bind(rule.enabled)
                    .bind(&rule.traffic_selector)
                    .bind(&rule.condition_expression)
                    .bind(rule.window_secs)
                    .bind(&rule.severity)
                    .execute(&mut *tx)
                    .await?;
                    anyhow::ensure!(
                        result.rows_affected() == 1,
                        "fleet_alert_policy_rule_id_conflict:{}",
                        rule.id
                    );
                    sqlx::query(
                        r#"
                        DELETE FROM policy_rule_states
                        WHERE policy_rule_id = $1 AND rule_version <> $2
                        "#,
                    )
                    .bind(rule.id)
                    .bind(rule.rule_version)
                    .execute(&mut *tx)
                    .await?;
                }
                if scope_changed && !retained_rule_ids.is_empty() {
                    sqlx::query(
                        "DELETE FROM policy_rule_states WHERE policy_rule_id = ANY($1::uuid[])",
                    )
                    .bind(&retained_rule_ids)
                    .execute(&mut *tx)
                    .await?;
                }
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("fleet.alert_policy_upserted")
                .bind(format!("fleet_alert_policy:{}", group.id))
                .bind(policy_group_metadata(&group, operator))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                group
            }
        };
        if let Err(error) = self.evaluate_policy_rules().await {
            tracing::warn!(%error, "deferred policy evaluation after policy update");
        }
        self.get_fleet_alert_policy(group.id).await
    }

    pub(crate) async fn delete_fleet_alert_policy(
        &self,
        policy_id: Uuid,
        reviewed_name: &str,
        operator: &AuthContext,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let mut groups = memory.policy_groups.write().await;
                let policy = groups
                    .iter()
                    .find(|policy| policy.id == policy_id)
                    .cloned()
                    .context("fleet_alert_policy_not_found")?;
                anyhow::ensure!(
                    policy.name == reviewed_name.trim(),
                    "fleet_alert_policy_delete_review_stale"
                );
                groups.retain(|stored| stored.id != policy_id);
                memory.policy_rule_states.write().await.retain(|state| {
                    !policy
                        .rules
                        .iter()
                        .any(|rule| rule.id == state.policy_rule_id)
                });
                memory.audits.write().await.push(policy_group_audit(
                    "fleet.alert_policy_deleted",
                    &policy,
                    operator,
                    unix_now().to_string(),
                ));
                drop(groups);
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    SELECT
                        id,
                        name,
                        enabled,
                        selector_expression,
                        notes,
                        created_by,
                        updated_by,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM policy_groups
                    WHERE id = $1
                    FOR UPDATE
                    "#,
                )
                .bind(policy_id)
                .fetch_optional(&mut *tx)
                .await?
                .context("fleet_alert_policy_not_found")?;
                let current_name: String = row.try_get("name")?;
                anyhow::ensure!(
                    current_name == reviewed_name.trim(),
                    "fleet_alert_policy_delete_review_stale"
                );
                let rules = sqlx::query(
                    r#"
                    SELECT
                        id,
                        group_id,
                        rule_version,
                        sort_order,
                        name,
                        enabled,
                        traffic_selector,
                        condition_expression,
                        window_secs,
                        severity,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM policy_rules
                    WHERE group_id = $1
                    ORDER BY sort_order ASC, created_at ASC
                    "#,
                )
                .bind(policy_id)
                .fetch_all(&mut *tx)
                .await?
                .into_iter()
                .map(policy_rule_from_row)
                .collect::<Result<Vec<_>>>()?;
                let policy = policy_group_from_row(row, rules)?;
                let deleted = sqlx::query("DELETE FROM policy_groups WHERE id = $1")
                    .bind(policy_id)
                    .execute(&mut *tx)
                    .await?;
                anyhow::ensure!(deleted.rows_affected() == 1, "fleet_alert_policy_not_found");
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("fleet.alert_policy_deleted")
                .bind(format!("fleet_alert_policy:{}", policy.id))
                .bind(policy_group_metadata(&policy, operator))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn list_policy_alerts(
        &self,
        query: &PolicyAlertQuery,
    ) -> Result<Vec<PolicyAlertRecord>> {
        self.list_policy_alerts_matching(
            query,
            Some(query.limit.unwrap_or(200).clamp(1, 1000) as usize),
            false,
            None,
            None,
            None,
        )
        .await
    }

    pub(crate) async fn list_policy_alert_candidates(
        &self,
        query: &PolicyAlertQuery,
        limit: usize,
        allowed_client_ids: Option<&HashSet<String>>,
        start_unix: Option<u64>,
        end_unix: Option<u64>,
    ) -> Result<Vec<PolicyAlertRecord>> {
        // Fleet alerts expose at most 200 rows. Apply native query filters
        // first, then keep one source-local top-K so dashboard polling remains
        // bounded as policy history grows.
        self.list_policy_alerts_matching(
            query,
            Some(limit.clamp(1, 200)),
            true,
            allowed_client_ids,
            start_unix,
            end_unix,
        )
        .await
    }

    async fn list_policy_alerts_matching(
        &self,
        query: &PolicyAlertQuery,
        result_limit: Option<usize>,
        prioritize_severity: bool,
        allowed_client_ids: Option<&HashSet<String>>,
        start_unix: Option<u64>,
        end_unix: Option<u64>,
    ) -> Result<Vec<PolicyAlertRecord>> {
        let allowed_client_id_values =
            allowed_client_ids.map(|client_ids| client_ids.iter().cloned().collect::<Vec<_>>());
        let mut alerts = match self {
            Self::Memory(memory) => memory.policy_alerts.read().await.clone(),
            Self::Postgres(pool) => {
                let sql = if prioritize_severity {
                    r#"
                SELECT
                    id,
                    policy_group_id,
                    policy_rule_id,
                    client_id,
                    trigger_generation,
                    severity,
                    category,
                    title,
                    detail,
                    actual_value,
                    threshold_value,
                    payload,
                    observed_at::text AS observed_at,
                    created_at::text AS created_at
                FROM policy_alerts
                WHERE ($2::text IS NULL OR client_id = $2)
                  AND ($3::text IS NULL OR severity = $3)
                  AND ($4::text IS NULL OR category = $4)
                  AND ($5::uuid IS NULL OR policy_group_id = $5)
                  AND ($6::text[] IS NULL OR client_id = ANY($6))
                  AND ($7::double precision IS NULL OR observed_at >= to_timestamp($7))
                  AND ($8::double precision IS NULL OR observed_at <= to_timestamp($8))
                ORDER BY
                    CASE severity
                        WHEN 'critical' THEN 0
                        WHEN 'warning' THEN 1
                        WHEN 'info' THEN 2
                        ELSE 3
                    END ASC,
                    observed_at DESC,
                    id DESC
                LIMIT $1
                "#
                } else {
                    r#"
                SELECT
                    id,
                    policy_group_id,
                    policy_rule_id,
                    client_id,
                    trigger_generation,
                    severity,
                    category,
                    title,
                    detail,
                    actual_value,
                    threshold_value,
                    payload,
                    observed_at::text AS observed_at,
                    created_at::text AS created_at
                FROM policy_alerts
                WHERE ($2::text IS NULL OR client_id = $2)
                  AND ($3::text IS NULL OR severity = $3)
                  AND ($4::text IS NULL OR category = $4)
                  AND ($5::uuid IS NULL OR policy_group_id = $5)
                  AND ($6::text[] IS NULL OR client_id = ANY($6))
                  AND ($7::double precision IS NULL OR observed_at >= to_timestamp($7))
                  AND ($8::double precision IS NULL OR observed_at <= to_timestamp($8))
                ORDER BY observed_at DESC, id DESC
                LIMIT $1
                "#
                };
                sqlx::query(sql)
                    .bind(result_limit.map(|limit| limit as i64))
                    .bind(query.client_id.as_deref())
                    .bind(query.severity.as_deref())
                    .bind(query.category.as_deref())
                    .bind(query.policy_group_id)
                    .bind(allowed_client_id_values.as_deref())
                    .bind(start_unix.map(|value| value as f64))
                    .bind(end_unix.map(|value| value as f64))
                    .fetch_all(pool)
                    .await?
                    .into_iter()
                    .map(policy_alert_from_row)
                    .collect::<Result<Vec<_>>>()?
            }
        };
        alerts.retain(|alert| {
            query
                .client_id
                .as_deref()
                .is_none_or(|client_id| alert.client_id == client_id)
                && query
                    .severity
                    .as_deref()
                    .is_none_or(|severity| alert.severity == severity)
                && query
                    .category
                    .as_deref()
                    .is_none_or(|category| alert.category == category)
                && query
                    .policy_group_id
                    .is_none_or(|policy_group_id| alert.policy_group_id == policy_group_id)
                && allowed_client_ids.is_none_or(|client_ids| client_ids.contains(&alert.client_id))
                && timestamp_in_optional_bounds(&alert.observed_at, start_unix, end_unix)
        });
        alerts.sort_by(|left, right| {
            if prioritize_severity {
                policy_alert_severity_rank(&left.severity)
                    .cmp(&policy_alert_severity_rank(&right.severity))
                    .then_with(|| compare_timestamps_desc(&left.observed_at, &right.observed_at))
                    .then_with(|| right.id.cmp(&left.id))
            } else {
                compare_timestamps_desc(&left.observed_at, &right.observed_at)
                    .then_with(|| right.id.cmp(&left.id))
            }
        });
        if let Some(result_limit) = result_limit {
            alerts.truncate(result_limit);
        }
        Ok(alerts)
    }

    pub(crate) async fn evaluate_policy_rules(&self) -> Result<usize> {
        // Configuration writes and the periodic evaluator can otherwise repeat
        // the same expensive fleet snapshot concurrently in one API process.
        let _evaluation_guard = POLICY_EVALUATION_LOCK.lock().await;
        let groups = self
            .list_fleet_alert_policy_definitions(None, Some(true))
            .await?;
        if groups.is_empty() {
            return Ok(0);
        }
        let agents = self.list_agents().await?;
        let now = Utc::now();
        let rules = self
            .list_vps_rules_matching(
                &VpsRuleQuery {
                    limit: None,
                    client_id: None,
                    selector_expression: None,
                    key: None,
                    state: None,
                },
                None,
            )
            .await?;
        let cycle_starts = traffic_cycle_starts_for_clients(
            agents.iter().map(|agent| agent.id.as_str()),
            &rules,
            now,
        );
        let mut matched_groups = Vec::with_capacity(groups.len());
        let mut selector_failures = Vec::new();
        for group in groups {
            match resolve_agents(&agents, &group.selector_expression) {
                Ok(matched) => matched_groups.push((group, matched)),
                Err(error) => selector_failures.push((group.id, group.name, error.to_string())),
            }
        }
        let mut stream_requests = traffic_stream_requests_from_rules(&cycle_starts, &rules);
        for (group, matched) in &matched_groups {
            for rule in group.rules.iter().filter(|rule| rule.enabled) {
                if policy_condition_uses_traffic(&rule.condition_expression).unwrap_or(false) {
                    if let Some(selector) = rule.traffic_selector.as_deref() {
                        add_traffic_selector_requests(
                            &mut stream_requests,
                            matched.iter().map(|agent| agent.id.as_str()),
                            &cycle_starts,
                            selector,
                        );
                    }
                }
            }
        }
        let stream_requests = stream_requests.into_iter().collect::<Vec<_>>();
        let traffic_usage = self
            .list_traffic_counter_usage_for_streams(&stream_requests, now.timestamp())
            .await?;
        let traffic = traffic_accounting_for_agents(&agents, &rules, &traffic_usage, now);
        let traffic_by_client = traffic
            .iter()
            .map(|record| (record.client_id.clone(), record))
            .collect::<HashMap<_, _>>();
        let rollup_client_ids = matched_groups
            .iter()
            .flat_map(|(_, matched)| matched.iter().map(|agent| agent.id.clone()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let rollups = latest_rollups(
            self.list_latest_telemetry_rollups_for_clients(&rollup_client_ids, None)
                .await?,
        );
        let mut fired = 0_usize;
        for (group, matched) in matched_groups {
            for rule in group.rules.iter().filter(|rule| rule.enabled) {
                let request = PolicyRuleRequest {
                    id: Some(rule.id),
                    name: rule.name.clone(),
                    enabled: rule.enabled,
                    traffic_selector: rule.traffic_selector.clone(),
                    condition_expression: rule.condition_expression.clone(),
                    window_secs: rule.window_secs,
                    severity: rule.severity.clone(),
                };
                for agent in &matched {
                    let override_traffic =
                        traffic_override_for_rule(&agent.id, &request, &rules, &traffic_usage, now);
                    let traffic_record = override_traffic
                        .as_ref()
                        .or_else(|| traffic_by_client.get(&agent.id).copied());
                    let evaluation =
                        evaluate_rule_for_client(&request, traffic_record, rollups.get(&agent.id));
                    if self
                        .persist_policy_evaluation(&group, rule, agent, evaluation, now)
                        .await?
                    {
                        fired += 1;
                    }
                }
            }
        }
        if selector_failures.is_empty() {
            Ok(fired)
        } else {
            Err(policy_selector_partial_failure(&selector_failures))
        }
    }

    async fn vps_rule_preview(
        &self,
        operation: &str,
        selector_expression: &str,
        values: &BTreeMap<String, String>,
        keys: &[String],
    ) -> Result<VpsRulesDryRunResponse> {
        let agents = self.list_agents().await?;
        let matched = resolve_agents(&agents, selector_expression)?;
        let stored = self
            .list_vps_rules_matching(
                &VpsRuleQuery {
                    limit: None,
                    client_id: None,
                    selector_expression: None,
                    key: None,
                    state: None,
                },
                None,
            )
            .await?;
        let stored_map = stored
            .iter()
            .map(|row| {
                (
                    (row.client_id.clone(), row.key.clone()),
                    row.value_raw.clone(),
                )
            })
            .collect::<HashMap<_, _>>();
        let mut changes = Vec::new();
        for agent in &matched {
            if operation == "upsert" {
                for (key, value) in values {
                    let parsed = parse_vps_rule_value(key, value);
                    let before = stored_map.get(&(agent.id.clone(), key.clone())).cloned();
                    let validation_errors = parsed
                        .as_ref()
                        .err()
                        .map(|error| vec![error.to_string()])
                        .unwrap_or_default();
                    let action = if !validation_errors.is_empty() {
                        "invalid"
                    } else if before.as_deref() == Some(value.trim()) {
                        "unchanged"
                    } else {
                        "set"
                    };
                    changes.push(VpsRuleChangePreview {
                        client_id: agent.id.clone(),
                        display_name: agent.display_name.clone(),
                        key: key.clone(),
                        before,
                        after: Some(value.trim().to_string()),
                        action: action.to_string(),
                        validation: if validation_errors.is_empty() {
                            "ok".to_string()
                        } else {
                            "invalid".to_string()
                        },
                        validation_errors,
                    });
                }
            } else {
                for key in keys {
                    let before = stored_map.get(&(agent.id.clone(), key.clone())).cloned();
                    changes.push(VpsRuleChangePreview {
                        client_id: agent.id.clone(),
                        display_name: agent.display_name.clone(),
                        key: key.clone(),
                        before: before.clone(),
                        after: None,
                        action: if before.is_some() {
                            "unset"
                        } else {
                            "unchanged"
                        }
                        .to_string(),
                        validation: "ok".to_string(),
                        validation_errors: Vec::new(),
                    });
                }
            }
        }
        let changed_row_count = changes
            .iter()
            .filter(|change| matches!(change.action.as_str(), "set" | "unset"))
            .count();
        let invalid_row_count = changes
            .iter()
            .filter(|change| change.action == "invalid")
            .count();
        let hash_payload = json!({
            "operation": operation,
            "selector_expression": selector_expression.trim(),
            "changes": changes,
        });
        Ok(VpsRulesDryRunResponse {
            matched_vps_count: matched.len(),
            changed_row_count,
            invalid_row_count,
            preview_hash: preview_hash(&hash_payload),
            changes,
        })
    }

    async fn apply_vps_rule_changes(
        &self,
        preview: &VpsRulesDryRunResponse,
        operator: &AuthContext,
    ) -> Result<()> {
        anyhow::ensure!(
            preview.invalid_row_count == 0,
            "vps_rules_preview_contains_invalid_rows"
        );
        let now = unix_now().to_string();
        match self {
            Self::Memory(memory) => {
                let mut rows = memory.vps_rule_values.write().await;
                for change in &preview.changes {
                    if change.action == "unchanged" {
                        continue;
                    }
                    rows.retain(|row| {
                        !(row.client_id == change.client_id && row.key == change.key)
                    });
                    if change.action == "set" {
                        let raw = change.after.clone().context("vps rule set missing value")?;
                        let parsed = parse_vps_rule_value(&change.key, &raw)?;
                        rows.push(VpsRuleValueRecord {
                            client_id: change.client_id.clone(),
                            key: change.key.clone(),
                            value_raw: parsed.raw,
                            value_json: parsed.json,
                            parsed_display: parsed.display,
                            state: "ok".to_string(),
                            validation_errors: Vec::new(),
                            source_kind: "operator".to_string(),
                            source_id: None,
                            updated_by: Some(operator.operator.id),
                            updated_at: now.clone(),
                        });
                    }
                }
                drop(rows);
                memory.audits.write().await.push(vps_rules_audit(
                    "fleet.vps_rules_updated",
                    preview,
                    operator,
                    now,
                ));
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                for change in &preview.changes {
                    if change.action == "unchanged" {
                        continue;
                    }
                    if change.action == "unset" {
                        sqlx::query(
                            "DELETE FROM vps_rule_values WHERE client_id = $1 AND key = $2",
                        )
                        .bind(&change.client_id)
                        .bind(&change.key)
                        .execute(&mut *tx)
                        .await?;
                    } else if change.action == "set" {
                        let raw = change.after.clone().context("vps rule set missing value")?;
                        let parsed = parse_vps_rule_value(&change.key, &raw)?;
                        sqlx::query(
                            r#"
                            INSERT INTO vps_rule_values (
                                client_id, key, value_raw, value_json, source_kind, source_id, updated_by
                            )
                            VALUES ($1, $2, $3, $4, 'operator', NULL, $5)
                            ON CONFLICT (client_id, key) DO UPDATE SET
                                value_raw = EXCLUDED.value_raw,
                                value_json = EXCLUDED.value_json,
                                source_kind = EXCLUDED.source_kind,
                                source_id = EXCLUDED.source_id,
                                updated_by = EXCLUDED.updated_by,
                                updated_at = now()
                            "#,
                        )
                        .bind(&change.client_id)
                        .bind(&change.key)
                        .bind(&parsed.raw)
                        .bind(SqlJson(parsed.json))
                        .bind(operator.operator.id)
                        .execute(&mut *tx)
                        .await?;
                    }
                }
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("fleet.vps_rules_updated")
                .bind("vps_rules")
                .bind(json!({
                    "preview_hash": preview.preview_hash,
                    "matched_vps_count": preview.matched_vps_count,
                    "changed_row_count": preview.changed_row_count,
                }))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
            }
        }
        Ok(())
    }

    async fn list_traffic_counter_usage_for_streams(
        &self,
        requests: &[TrafficStreamRequest],
        now_unix: i64,
    ) -> Result<Vec<TrafficCounterStreamUsage>> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        match self {
            Self::Memory(memory) => Ok(aggregate_memory_traffic_counter_usage(
                &memory.traffic_counter_samples.read().await,
                requests,
                now_unix,
            )),
            Self::Postgres(pool) => {
                let client_ids = requests
                    .iter()
                    .map(|request| request.client_id.clone())
                    .collect::<Vec<_>>();
                let source_kinds = requests
                    .iter()
                    .map(|request| request.source_kind.clone())
                    .collect::<Vec<_>>();
                let interfaces = requests
                    .iter()
                    .map(|request| request.interface.clone())
                    .collect::<Vec<_>>();
                let cycle_start_values = requests
                    .iter()
                    .map(|request| request.cycle_start_unix)
                    .collect::<Vec<_>>();
                let rows = sqlx::query(
                    r#"
                    WITH requested AS (
                        SELECT client_id, source_kind, interface, cycle_start_unix
                        FROM UNNEST(
                            $1::text[],
                            $2::text[],
                            $3::text[],
                            $4::bigint[]
                        ) AS request(
                            client_id,
                            source_kind,
                            interface,
                            cycle_start_unix
                        )
                    ),
                    cycle_samples AS (
                        SELECT
                            sample.client_id,
                            sample.source_kind,
                            sample.interface,
                            sample.observed_at,
                            sample.rx_bytes,
                            sample.tx_bytes,
                            sample.counter_epoch,
                            requested.cycle_start_unix
                        FROM traffic_counter_samples sample
                        JOIN requested
                          ON requested.client_id = sample.client_id
                         AND requested.source_kind = sample.source_kind
                         AND requested.interface = sample.interface
                        WHERE sample.observed_at >= to_timestamp(requested.cycle_start_unix)
                          AND sample.observed_at <= to_timestamp($5)
                    ),
                    baseline_samples AS (
                        SELECT
                            requested.client_id,
                            requested.source_kind,
                            requested.interface,
                            sample.observed_at,
                            sample.rx_bytes,
                            sample.tx_bytes,
                            sample.counter_epoch,
                            requested.cycle_start_unix
                        FROM requested
                        JOIN LATERAL (
                            SELECT
                                sample.observed_at,
                                sample.rx_bytes,
                                sample.tx_bytes,
                                sample.counter_epoch
                            FROM traffic_counter_samples sample
                            WHERE sample.client_id = requested.client_id
                              AND sample.source_kind = requested.source_kind
                              AND sample.interface = requested.interface
                              AND sample.observed_at < to_timestamp(
                                  requested.cycle_start_unix
                              )
                              AND sample.observed_at <= to_timestamp($5)
                            ORDER BY sample.observed_at DESC
                            LIMIT 1
                        ) sample ON TRUE
                    ),
                    selected_samples AS (
                        SELECT * FROM cycle_samples
                        UNION ALL
                        SELECT * FROM baseline_samples
                    ),
                    sequenced_samples AS (
                        SELECT
                            selected_samples.*,
                            LAG(observed_at) OVER stream AS previous_observed_at,
                            LAG(rx_bytes) OVER stream AS previous_rx_bytes,
                            LAG(tx_bytes) OVER stream AS previous_tx_bytes,
                            LAG(counter_epoch) OVER stream AS previous_counter_epoch
                        FROM selected_samples
                        WINDOW stream AS (
                            PARTITION BY client_id, source_kind, interface
                            ORDER BY observed_at ASC
                        )
                    ),
                    usage AS (
                        SELECT
                            client_id,
                            source_kind,
                            interface,
                            COALESCE(SUM(
                                CASE
                                    WHEN observed_at >= to_timestamp(cycle_start_unix)
                                     AND previous_observed_at IS NOT NULL
                                     AND counter_epoch = previous_counter_epoch
                                     AND rx_bytes >= previous_rx_bytes
                                    THEN rx_bytes - previous_rx_bytes
                                    ELSE 0
                                END
                            ), 0)::bigint AS cycle_rx,
                            COALESCE(SUM(
                                CASE
                                    WHEN observed_at >= to_timestamp(cycle_start_unix)
                                     AND previous_observed_at IS NOT NULL
                                     AND counter_epoch = previous_counter_epoch
                                     AND tx_bytes >= previous_tx_bytes
                                    THEN tx_bytes - previous_tx_bytes
                                    ELSE 0
                                END
                            ), 0)::bigint AS cycle_tx,
                            COUNT(DISTINCT counter_epoch)::bigint AS counter_epochs_seen
                        FROM sequenced_samples
                        GROUP BY client_id, source_kind, interface
                    ),
                    latest AS (
                        SELECT DISTINCT ON (client_id, source_kind, interface)
                            client_id,
                            source_kind,
                            interface,
                            rx_bytes AS latest_rx,
                            tx_bytes AS latest_tx,
                            EXTRACT(EPOCH FROM observed_at)::bigint AS last_sample_unix
                        FROM sequenced_samples
                        ORDER BY
                            client_id ASC,
                            source_kind ASC,
                            interface ASC,
                            observed_at DESC
                    )
                    SELECT
                        usage.client_id,
                        usage.source_kind,
                        usage.interface,
                        usage.cycle_rx,
                        usage.cycle_tx,
                        latest.latest_rx,
                        latest.latest_tx,
                        latest.last_sample_unix,
                        usage.counter_epochs_seen
                    FROM usage
                    JOIN latest
                      ON latest.client_id = usage.client_id
                     AND latest.source_kind = usage.source_kind
                     AND latest.interface = usage.interface
                    ORDER BY usage.client_id ASC, usage.source_kind ASC, usage.interface ASC
                    "#,
                )
                .bind(client_ids)
                .bind(source_kinds)
                .bind(interfaces)
                .bind(cycle_start_values)
                .bind(now_unix)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(TrafficCounterStreamUsage {
                            client_id: row.try_get("client_id")?,
                            source_kind: row.try_get("source_kind")?,
                            interface: row.try_get("interface")?,
                            cycle_rx: row.try_get("cycle_rx")?,
                            cycle_tx: row.try_get("cycle_tx")?,
                            latest_rx: row.try_get("latest_rx")?,
                            latest_tx: row.try_get("latest_tx")?,
                            last_sample_unix: row.try_get("last_sample_unix")?,
                            counter_epochs_seen: row.try_get("counter_epochs_seen")?,
                        })
                    })
                    .collect()
            }
        }
    }

    async fn enrich_policy_group_summaries(&self, groups: &mut [PolicyGroupRecord]) -> Result<()> {
        if groups.is_empty() {
            return Ok(());
        }
        let agents = self.list_agents().await?;
        let rule_ids = groups
            .iter()
            .flat_map(|group| group.rules.iter().map(|rule| rule.id))
            .collect::<Vec<_>>();
        let states = self.policy_rule_states_for_rules(&rule_ids).await?;
        for group in groups {
            let matched = resolve_agents(&agents, &group.selector_expression)?;
            let matched_ids = matched
                .iter()
                .map(|agent| agent.id.as_str())
                .collect::<HashSet<_>>();
            let enabled_rules = group
                .rules
                .iter()
                .filter(|rule| rule.enabled)
                .collect::<Vec<_>>();
            let rule_by_id = group
                .rules
                .iter()
                .map(|rule| (rule.id, rule))
                .collect::<HashMap<_, _>>();
            let mut active_warning = 0_i64;
            let mut active_critical = 0_i64;
            let mut incomplete_clients = BTreeSet::new();
            let mut last_evaluated_at = None::<String>;
            for state in &states {
                if !matched_ids.contains(state.client_id.as_str()) {
                    continue;
                }
                let Some(rule) = rule_by_id.get(&state.policy_rule_id) else {
                    continue;
                };
                if !rule.enabled || rule.rule_version != state.rule_version {
                    continue;
                }
                if state.incomplete {
                    incomplete_clients.insert(state.client_id.clone());
                }
                if state.condition_true && state.window_satisfied && !state.incomplete {
                    match rule.severity.as_str() {
                        "critical" => active_critical += 1,
                        "warning" => active_warning += 1,
                        _ => {}
                    }
                }
                if last_evaluated_at
                    .as_deref()
                    .is_none_or(|stored| state.last_evaluated_at.as_str() > stored)
                {
                    last_evaluated_at = Some(state.last_evaluated_at.clone());
                }
            }
            group.matched_vps_count = matched.len() as i64;
            group.rule_count = group.rules.len() as i64;
            group.enabled_rule_count = enabled_rules.len() as i64;
            group.active_warning_count = active_warning;
            group.active_critical_count = active_critical;
            group.incomplete_vps_count = incomplete_clients.len() as i64;
            group.last_evaluated_at = last_evaluated_at;
        }
        Ok(())
    }

    async fn policy_rule_states_for_rules(
        &self,
        rule_ids: &[Uuid],
    ) -> Result<Vec<PolicyRuleStateRecord>> {
        if rule_ids.is_empty() {
            return Ok(Vec::new());
        }
        match self {
            Self::Memory(memory) => {
                let ids = rule_ids.iter().copied().collect::<HashSet<_>>();
                Ok(memory
                    .policy_rule_states
                    .read()
                    .await
                    .iter()
                    .filter(|state| ids.contains(&state.policy_rule_id))
                    .cloned()
                    .collect())
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        policy_rule_id,
                        client_id,
                        rule_version,
                        condition_true,
                        previous_condition_true,
                        window_satisfied,
                        first_true_at::text AS first_true_at,
                        last_true_at::text AS last_true_at,
                        last_false_at::text AS last_false_at,
                        last_evaluated_at::text AS last_evaluated_at,
                        incomplete,
                        incomplete_reasons,
                        last_actual_value,
                        last_threshold_value,
                        last_fired_at::text AS last_fired_at,
                        trigger_generation,
                        updated_at::text AS updated_at
                    FROM policy_rule_states
                    WHERE policy_rule_id = ANY($1)
                    "#,
                )
                .bind(rule_ids)
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(policy_rule_state_from_row).collect()
            }
        }
    }

    async fn persist_policy_evaluation(
        &self,
        group: &PolicyGroupRecord,
        rule: &PolicyRuleRecord,
        agent: &AgentView,
        evaluation: PolicyEvaluation,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let now_text = now.to_rfc3339();
        match self {
            Self::Memory(memory) => {
                // Keep policy edits ordered before state/alert/event writes and
                // reject an evaluator that loaded an obsolete policy snapshot.
                let groups = memory.policy_groups.read().await;
                let policy_is_current = groups.iter().any(|stored_group| {
                    stored_group.id == group.id
                        && stored_group.enabled
                        && stored_group.name == group.name
                        && stored_group.selector_expression == group.selector_expression
                        && stored_group.rules.iter().any(|stored_rule| {
                            stored_rule.id == rule.id
                                && stored_rule.enabled
                                && stored_rule.rule_version == rule.rule_version
                        })
                });
                if !policy_is_current {
                    return Ok(false);
                }
                let mut states = memory.policy_rule_states.write().await;
                let mut alerts = memory.policy_alerts.write().await;
                let existing = states
                    .iter()
                    .find(|state| {
                        state.policy_rule_id == rule.id
                            && state.client_id == agent.id
                            && state.rule_version == rule.rule_version
                    })
                    .cloned();
                if policy_state_is_newer_than(existing.as_ref(), now) {
                    return Ok(false);
                }
                let max_generation = alerts
                    .iter()
                    .filter(|alert| alert.policy_rule_id == rule.id && alert.client_id == agent.id)
                    .map(|alert| alert.trigger_generation)
                    .max()
                    .unwrap_or(0);
                let mut state = next_policy_rule_state(
                    rule,
                    &agent.id,
                    &evaluation,
                    existing.as_ref(),
                    max_generation,
                    now,
                )?;
                let eligible = policy_state_is_alert_eligible(&state);
                let (alert, inserted) = if eligible {
                    if let Some(alert) = alerts
                        .iter()
                        .find(|alert| {
                            alert.policy_rule_id == state.policy_rule_id
                                && alert.client_id == state.client_id
                                && alert.trigger_generation == state.trigger_generation
                        })
                        .cloned()
                    {
                        (Some(alert), false)
                    } else {
                        (
                            Some(policy_alert_for_evaluation(
                                group,
                                rule,
                                agent,
                                &state,
                                &evaluation,
                                &now_text,
                            )),
                            true,
                        )
                    }
                } else {
                    (None, false)
                };
                let event_row = alert
                    .as_ref()
                    .filter(|alert| {
                        inserted || policy_webhook_repair_is_recent(&alert.observed_at, now)
                    })
                    .map(|alert| webhook_event_row(policy_alert_webhook_event(alert), now))
                    .transpose()?;
                let mut events = if event_row.is_some() {
                    Some(memory.webhook_events.write().await)
                } else {
                    None
                };
                if let Some(alert) = alert.as_ref() {
                    state.last_fired_at = Some(alert.observed_at.clone());
                }
                states.retain(|stored| {
                    !(stored.policy_rule_id == state.policy_rule_id
                        && stored.client_id == state.client_id
                        && stored.rule_version == state.rule_version)
                });
                states.push(state.clone());
                if inserted {
                    alerts.push(alert.expect("eligible policy evaluation must build an alert"));
                }
                if let (Some(events), Some(event)) = (events.as_mut(), event_row) {
                    if !events.iter().any(|stored| {
                        stored.kind == event.kind && stored.event_id == event.event_id
                    }) {
                        events.push(event);
                    }
                }
                Ok(inserted)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let lock_name = format!("vpsman:policy-rule-state:{}:{}", rule.id, agent.id);
                sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
                    .bind(lock_name)
                    .execute(&mut *tx)
                    .await?;

                // Lock group before rule, matching the editor's group -> rule
                // order. A join can lock the rule first and deadlock with an
                // editor that already updated the group and is deleting rules.
                let policy_group_is_current = sqlx::query_scalar::<_, Uuid>(
                    r#"
                    SELECT id
                    FROM policy_groups
                    WHERE id = $1
                      AND enabled = TRUE
                      AND name = $2
                      AND selector_expression = $3
                      AND updated_at = $4::timestamptz
                    FOR SHARE
                    "#,
                )
                .bind(group.id)
                .bind(&group.name)
                .bind(&group.selector_expression)
                .bind(&group.updated_at)
                .fetch_optional(&mut *tx)
                .await?
                .is_some();
                if !policy_group_is_current {
                    tx.commit().await?;
                    return Ok(false);
                }
                let policy_rule_is_current = sqlx::query_scalar::<_, i32>(
                    r#"
                    SELECT rule_version
                    FROM policy_rules
                    WHERE id = $1
                      AND group_id = $2
                      AND rule_version = $3
                      AND enabled = TRUE
                    FOR SHARE
                    "#,
                )
                .bind(rule.id)
                .bind(group.id)
                .bind(rule.rule_version)
                .fetch_optional(&mut *tx)
                .await?
                .is_some();
                if !policy_rule_is_current {
                    tx.commit().await?;
                    return Ok(false);
                }

                let row = sqlx::query(
                    r#"
                    SELECT
                        policy_rule_id,
                        client_id,
                        rule_version,
                        condition_true,
                        previous_condition_true,
                        window_satisfied,
                        first_true_at::text AS first_true_at,
                        last_true_at::text AS last_true_at,
                        last_false_at::text AS last_false_at,
                        last_evaluated_at::text AS last_evaluated_at,
                        incomplete,
                        incomplete_reasons,
                        last_actual_value,
                        last_threshold_value,
                        last_fired_at::text AS last_fired_at,
                        trigger_generation,
                        updated_at::text AS updated_at,
                        last_evaluated_at > $4::timestamptz AS evaluation_is_stale
                    FROM policy_rule_states
                    WHERE policy_rule_id = $1 AND client_id = $2 AND rule_version = $3
                    FOR UPDATE
                    "#,
                )
                .bind(rule.id)
                .bind(&agent.id)
                .bind(rule.rule_version)
                .bind(&now_text)
                .fetch_optional(&mut *tx)
                .await?;
                let evaluation_is_stale = row
                    .as_ref()
                    .map(|row| row.try_get::<bool, _>("evaluation_is_stale"))
                    .transpose()?
                    .unwrap_or(false);
                if evaluation_is_stale {
                    tx.commit().await?;
                    return Ok(false);
                }
                let existing = row.map(policy_rule_state_from_row).transpose()?;
                let max_generation = sqlx::query_scalar::<_, i64>(
                    r#"
                    SELECT COALESCE(MAX(trigger_generation), 0)::bigint
                    FROM policy_alerts
                    WHERE policy_rule_id = $1 AND client_id = $2
                    "#,
                )
                .bind(rule.id)
                .bind(&agent.id)
                .fetch_one(&mut *tx)
                .await?;
                let mut state = next_policy_rule_state(
                    rule,
                    &agent.id,
                    &evaluation,
                    existing.as_ref(),
                    max_generation,
                    now,
                )?;
                let mut inserted = false;
                let alert = if policy_state_is_alert_eligible(&state) {
                    let existing_alert = sqlx::query(
                        r#"
                        SELECT
                            id,
                            policy_group_id,
                            policy_rule_id,
                            client_id,
                            trigger_generation,
                            severity,
                            category,
                            title,
                            detail,
                            actual_value,
                            threshold_value,
                            payload,
                            observed_at::text AS observed_at,
                            created_at::text AS created_at
                        FROM policy_alerts
                        WHERE policy_rule_id = $1
                          AND client_id = $2
                          AND trigger_generation = $3
                        FOR UPDATE
                        "#,
                    )
                    .bind(rule.id)
                    .bind(&agent.id)
                    .bind(state.trigger_generation)
                    .fetch_optional(&mut *tx)
                    .await?
                    .map(policy_alert_from_row)
                    .transpose()?;
                    if let Some(alert) = existing_alert {
                        Some(alert)
                    } else {
                        let alert = policy_alert_for_evaluation(
                            group,
                            rule,
                            agent,
                            &state,
                            &evaluation,
                            &now_text,
                        );
                        insert_policy_alert_in_tx(&mut tx, &alert).await?;
                        inserted = true;
                        Some(alert)
                    }
                } else {
                    None
                };
                if let Some(alert) = alert.as_ref() {
                    state.last_fired_at = Some(alert.observed_at.clone());
                }
                upsert_policy_rule_state_in_tx(&mut tx, &state).await?;
                if let Some(alert) = alert.as_ref().filter(|alert| {
                    inserted || policy_webhook_repair_is_recent(&alert.observed_at, now)
                }) {
                    let event = policy_alert_webhook_event(alert);
                    let event_exists = sqlx::query_scalar::<_, bool>(
                        r#"
                        SELECT EXISTS (
                            SELECT 1
                            FROM webhook_events
                            WHERE kind = $1 AND event_id = $2
                        )
                        "#,
                    )
                    .bind(&event.kind)
                    .bind(&event.event_id)
                    .fetch_one(&mut *tx)
                    .await?;
                    if !event_exists {
                        record_webhook_event_in_tx(&mut tx, event, now).await?;
                    }
                }
                tx.commit().await?;
                Ok(inserted)
            }
        }
    }
}

async fn upsert_policy_rule_state_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    state: &PolicyRuleStateRecord,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO policy_rule_states (
            policy_rule_id, client_id, rule_version, condition_true,
            previous_condition_true, window_satisfied, first_true_at, last_true_at,
            last_false_at, last_evaluated_at, incomplete, incomplete_reasons,
            last_actual_value, last_threshold_value, last_fired_at,
            trigger_generation
        )
        VALUES ($1,$2,$3,$4,$5,$6,$7::timestamptz,$8::timestamptz,$9::timestamptz,$10::timestamptz,$11,$12,$13,$14,$15::timestamptz,$16)
        ON CONFLICT (policy_rule_id, client_id, rule_version) DO UPDATE SET
            condition_true = EXCLUDED.condition_true,
            previous_condition_true = EXCLUDED.previous_condition_true,
            window_satisfied = EXCLUDED.window_satisfied,
            first_true_at = EXCLUDED.first_true_at,
            last_true_at = EXCLUDED.last_true_at,
            last_false_at = EXCLUDED.last_false_at,
            last_evaluated_at = EXCLUDED.last_evaluated_at,
            incomplete = EXCLUDED.incomplete,
            incomplete_reasons = EXCLUDED.incomplete_reasons,
            last_actual_value = EXCLUDED.last_actual_value,
            last_threshold_value = EXCLUDED.last_threshold_value,
            last_fired_at = EXCLUDED.last_fired_at,
            trigger_generation = EXCLUDED.trigger_generation,
            updated_at = now()
        "#,
    )
    .bind(state.policy_rule_id)
    .bind(&state.client_id)
    .bind(state.rule_version)
    .bind(state.condition_true)
    .bind(state.previous_condition_true)
    .bind(state.window_satisfied)
    .bind(state.first_true_at.as_deref())
    .bind(state.last_true_at.as_deref())
    .bind(state.last_false_at.as_deref())
    .bind(&state.last_evaluated_at)
    .bind(state.incomplete)
    .bind(&state.incomplete_reasons)
    .bind(state.last_actual_value)
    .bind(state.last_threshold_value)
    .bind(state.last_fired_at.as_deref())
    .bind(state.trigger_generation)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_policy_alert_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    alert: &PolicyAlertRecord,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO policy_alerts (
            id, policy_group_id, policy_rule_id, client_id, trigger_generation,
            severity, category, title, detail, actual_value, threshold_value,
            payload, observed_at
        )
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13::timestamptz)
        "#,
    )
    .bind(alert.id)
    .bind(alert.policy_group_id)
    .bind(alert.policy_rule_id)
    .bind(&alert.client_id)
    .bind(alert.trigger_generation)
    .bind(&alert.severity)
    .bind(&alert.category)
    .bind(&alert.title)
    .bind(&alert.detail)
    .bind(alert.actual_value)
    .bind(alert.threshold_value)
    .bind(SqlJson(&alert.payload))
    .bind(&alert.observed_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn next_policy_rule_state(
    rule: &PolicyRuleRecord,
    client_id: &str,
    evaluation: &PolicyEvaluation,
    existing: Option<&PolicyRuleStateRecord>,
    max_alert_generation: i64,
    now: DateTime<Utc>,
) -> Result<PolicyRuleStateRecord> {
    let now_text = now.to_rfc3339();
    let previous_condition_true = existing.map(|state| state.condition_true).unwrap_or(false);
    let mut first_true_at = existing.and_then(|state| state.first_true_at.clone());
    let mut last_false_at = existing.and_then(|state| state.last_false_at.clone());
    let mut last_true_at = existing.and_then(|state| state.last_true_at.clone());
    let mut trigger_generation = existing
        .map(|state| state.trigger_generation)
        .unwrap_or(0)
        .max(max_alert_generation)
        .max(0);
    if evaluation.condition_true && !previous_condition_true {
        first_true_at = Some(now_text.clone());
        trigger_generation = trigger_generation
            .checked_add(1)
            .context("policy_alert_trigger_generation_exhausted")?;
    }
    if evaluation.condition_true {
        last_true_at = Some(now_text.clone());
    } else {
        first_true_at = None;
        last_false_at = Some(now_text.clone());
    }
    let window_satisfied = if evaluation.incomplete || !evaluation.condition_true {
        false
    } else if rule.window_secs <= 0 {
        true
    } else {
        first_true_at
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|first| now.timestamp() - first.timestamp() >= rule.window_secs)
            .unwrap_or(false)
    };
    Ok(PolicyRuleStateRecord {
        policy_rule_id: rule.id,
        client_id: client_id.to_string(),
        rule_version: rule.rule_version,
        condition_true: evaluation.condition_true,
        previous_condition_true,
        window_satisfied,
        first_true_at,
        last_true_at,
        last_false_at,
        last_evaluated_at: now_text.clone(),
        incomplete: evaluation.incomplete,
        incomplete_reasons: evaluation.incomplete_reasons.clone(),
        last_actual_value: evaluation.actual_value,
        last_threshold_value: evaluation.threshold_value,
        last_fired_at: existing.and_then(|state| state.last_fired_at.clone()),
        trigger_generation,
        updated_at: now_text,
    })
}

fn policy_state_is_alert_eligible(state: &PolicyRuleStateRecord) -> bool {
    state.condition_true && state.window_satisfied && !state.incomplete
}

fn policy_state_is_newer_than(
    state: Option<&PolicyRuleStateRecord>,
    evaluation_time: DateTime<Utc>,
) -> bool {
    state
        .and_then(|state| DateTime::parse_from_rfc3339(&state.last_evaluated_at).ok())
        .is_some_and(|last_evaluated| last_evaluated > evaluation_time)
}

fn policy_webhook_repair_is_recent(observed_at: &str, evaluation_time: DateTime<Utc>) -> bool {
    DateTime::parse_from_rfc3339(observed_at)
        .or_else(|_| DateTime::parse_from_str(observed_at, "%Y-%m-%d %H:%M:%S%.f%#z"))
        .ok()
        .map(|observed_at| {
            let age = evaluation_time.timestamp() - observed_at.timestamp();
            (0..=POLICY_WEBHOOK_REPAIR_WINDOW_SECS).contains(&age)
        })
        .unwrap_or(false)
}

fn policy_alert_for_evaluation(
    group: &PolicyGroupRecord,
    rule: &PolicyRuleRecord,
    agent: &AgentView,
    state: &PolicyRuleStateRecord,
    evaluation: &PolicyEvaluation,
    now_text: &str,
) -> PolicyAlertRecord {
    let alert_id = Uuid::new_v4();
    let title = if evaluation.category == "traffic" {
        "Traffic quota threshold reached"
    } else {
        "Resource policy threshold reached"
    }
    .to_string();
    let detail = format!(
        "{} matched policy condition {}",
        agent.display_name, rule.condition_expression
    );
    let mut payload = evaluation.payload.clone();
    if let Some(object) = payload.as_object_mut() {
        object.insert(
            "event".to_string(),
            json!({
                "kind": "alert.policy_reached",
                "id": format!("policy-alert:{alert_id}"),
                "occurred_at": now_text,
            }),
        );
        object.insert(
            "alert".to_string(),
            json!({
                "id": alert_id,
                "category": evaluation.category,
                "severity": rule.severity,
                "title": title,
                "state": "open",
            }),
        );
        object.insert(
            "vps".to_string(),
            json!({
                "id": agent.id,
                "name": agent.display_name,
                "tags": agent.tags,
            }),
        );
        object.insert(
            "policy".to_string(),
            json!({
                "id": group.id,
                "name": group.name,
            }),
        );
        object.insert(
            "rule".to_string(),
            json!({
                "id": rule.id,
                "name": rule.name,
                "rule_version": rule.rule_version,
                "condition_expression": rule.condition_expression,
                "traffic_selector": rule.traffic_selector,
                "window_secs": rule.window_secs,
            }),
        );
    }
    PolicyAlertRecord {
        id: alert_id,
        policy_group_id: group.id,
        policy_rule_id: rule.id,
        client_id: agent.id.clone(),
        trigger_generation: state.trigger_generation,
        severity: rule.severity.clone(),
        category: evaluation.category.clone(),
        title,
        detail,
        actual_value: evaluation.actual_value,
        threshold_value: evaluation.threshold_value,
        payload,
        observed_at: now_text.to_string(),
        created_at: now_text.to_string(),
    }
}

fn policy_alert_webhook_event(alert: &PolicyAlertRecord) -> WebhookEventCandidate {
    WebhookEventCandidate {
        kind: "alert.policy_reached".to_string(),
        event_id: format!("policy-alert:{}", alert.id),
        event_predicates: vec![
            "alert.policy_reached".to_string(),
            "alert.open".to_string(),
            format!("alert.category:{}", alert.category),
            format!("alert.severity:{}", alert.severity),
        ],
        subject_client_ids: vec![alert.client_id.clone()],
        payload: alert.payload.clone(),
        actor_id: None,
    }
}

fn traffic_cycle_starts_for_clients<'a>(
    client_ids: impl IntoIterator<Item = &'a str>,
    rules: &[VpsRuleValueRecord],
    now: DateTime<Utc>,
) -> Vec<(String, i64)> {
    let reset_days = rules
        .iter()
        .filter(|rule| rule.key == VPS_RULE_KEY_TRAFFIC_RESET_DAY)
        .filter_map(|rule| {
            rule.value_json
                .get("day")
                .and_then(Value::as_i64)
                .map(|day| (rule.client_id.as_str(), day as i32))
        })
        .collect::<HashMap<_, _>>();
    client_ids
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|client_id| {
            let reset_day = reset_days.get(client_id).copied().unwrap_or(1);
            (
                client_id.to_string(),
                cycle_bounds(reset_day, now).0.timestamp(),
            )
        })
        .collect()
}

fn traffic_stream_requests_from_rules(
    cycle_starts: &[(String, i64)],
    rules: &[VpsRuleValueRecord],
) -> BTreeSet<TrafficStreamRequest> {
    let cycle_starts = cycle_starts
        .iter()
        .map(|(client_id, cycle_start)| (client_id.as_str(), *cycle_start))
        .collect::<HashMap<_, _>>();
    let mut requests = BTreeSet::new();
    for rule in rules
        .iter()
        .filter(|rule| rule.key == VPS_RULE_KEY_TRAFFIC_SELECTORS)
    {
        let Some(cycle_start_unix) = cycle_starts.get(rule.client_id.as_str()).copied() else {
            continue;
        };
        let Ok(selectors) = parse_persisted_traffic_selector_list(&rule.value_raw) else {
            continue;
        };
        for selector in selectors {
            requests.insert(TrafficStreamRequest {
                client_id: rule.client_id.clone(),
                source_kind: selector.source,
                interface: selector.interface,
                cycle_start_unix,
            });
        }
    }
    requests
}

fn add_traffic_selector_requests<'a>(
    requests: &mut BTreeSet<TrafficStreamRequest>,
    client_ids: impl IntoIterator<Item = &'a str>,
    cycle_starts: &[(String, i64)],
    selector_expression: &str,
) {
    let Ok(selectors) = parse_persisted_traffic_selector_list(selector_expression) else {
        return;
    };
    let cycle_starts = cycle_starts
        .iter()
        .map(|(client_id, cycle_start)| (client_id.as_str(), *cycle_start))
        .collect::<HashMap<_, _>>();
    for client_id in client_ids.into_iter().collect::<BTreeSet<_>>() {
        let Some(cycle_start_unix) = cycle_starts.get(client_id).copied() else {
            continue;
        };
        for selector in &selectors {
            requests.insert(TrafficStreamRequest {
                client_id: client_id.to_string(),
                source_kind: selector.source.clone(),
                interface: selector.interface.clone(),
                cycle_start_unix,
            });
        }
    }
}

fn aggregate_memory_traffic_counter_usage(
    samples: &[TrafficCounterSampleRecord],
    requests: &[TrafficStreamRequest],
    now_unix: i64,
) -> Vec<TrafficCounterStreamUsage> {
    let mut request_indices = HashMap::<(&str, &str, &str), Vec<usize>>::new();
    for (index, request) in requests.iter().enumerate() {
        request_indices
            .entry((
                request.client_id.as_str(),
                request.source_kind.as_str(),
                request.interface.as_str(),
            ))
            .or_default()
            .push(index);
    }
    let mut selected_by_request = vec![Vec::new(); requests.len()];
    let mut baselines = vec![None::<TrafficCounterSampleRecord>; requests.len()];
    for sample in samples
        .iter()
        .filter(|sample| sample.observed_unix <= now_unix)
    {
        let Some(indices) = request_indices.get(&(
            sample.client_id.as_str(),
            sample.source_kind.as_str(),
            sample.interface.as_str(),
        )) else {
            continue;
        };
        for index in indices {
            if sample.observed_unix >= requests[*index].cycle_start_unix {
                selected_by_request[*index].push(sample.clone());
            } else if baselines[*index]
                .as_ref()
                .is_none_or(|baseline| sample.observed_unix > baseline.observed_unix)
            {
                baselines[*index] = Some(sample.clone());
            }
        }
    }

    let mut rows = Vec::new();
    for ((request, mut selected), baseline) in
        requests.iter().zip(selected_by_request).zip(baselines)
    {
        if let Some(baseline) = baseline {
            selected.push(baseline);
        }
        if selected.is_empty() {
            continue;
        }
        let usage = derive_cycle_usage(&selected, request.cycle_start_unix, now_unix);
        let counter_epochs_seen = selected
            .iter()
            .map(|sample| sample.counter_epoch)
            .collect::<HashSet<_>>()
            .len();
        rows.push(TrafficCounterStreamUsage {
            client_id: request.client_id.clone(),
            source_kind: request.source_kind.clone(),
            interface: request.interface.clone(),
            cycle_rx: usage.cycle_rx,
            cycle_tx: usage.cycle_tx,
            latest_rx: usage.latest_rx,
            latest_tx: usage.latest_tx,
            last_sample_unix: usage
                .last_sample_unix
                .expect("non-empty selected traffic samples have a latest timestamp"),
            counter_epochs_seen: i64::try_from(counter_epochs_seen).unwrap_or(i64::MAX),
        });
    }
    rows.sort_by(|left, right| {
        left.client_id
            .cmp(&right.client_id)
            .then_with(|| left.source_kind.cmp(&right.source_kind))
            .then_with(|| left.interface.cmp(&right.interface))
    });
    rows
}

fn validate_vps_rule_values(values: &BTreeMap<String, String>) -> Result<()> {
    anyhow::ensure!(!values.is_empty(), "vps_rules_values_required");
    for (key, value) in values {
        parse_vps_rule_value(key, value)?;
    }
    Ok(())
}

fn validate_vps_rule_keys(keys: &[String]) -> Result<()> {
    anyhow::ensure!(!keys.is_empty(), "vps_rules_keys_required");
    let mut seen = HashSet::new();
    for key in keys {
        let normalized = normalize_vps_rule_key(key)?;
        anyhow::ensure!(seen.insert(normalized), "vps_rules_duplicate_key");
    }
    Ok(())
}

fn normalize_vps_rule_key(key: &str) -> Result<String> {
    let key = key.trim();
    anyhow::ensure!(
        matches!(
            key,
            VPS_RULE_KEY_TRAFFIC_RESET_DAY
                | VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL
                | VPS_RULE_KEY_TRAFFIC_QUOTA_RX
                | VPS_RULE_KEY_TRAFFIC_QUOTA_TX
                | VPS_RULE_KEY_TRAFFIC_SELECTORS
        ),
        "vps_rules_key_unsupported"
    );
    Ok(key.to_string())
}

fn parse_vps_rule_value(key: &str, value: &str) -> Result<ParsedRuleValue> {
    parse_vps_rule_value_with_legacy_selector_support(key, value, false)
}

fn parse_persisted_vps_rule_value(key: &str, value: &str) -> Result<ParsedRuleValue> {
    parse_vps_rule_value_with_legacy_selector_support(key, value, true)
}

fn parse_vps_rule_value_with_legacy_selector_support(
    key: &str,
    value: &str,
    allow_direction_overlap: bool,
) -> Result<ParsedRuleValue> {
    let key = normalize_vps_rule_key(key)?;
    let raw = value.trim();
    anyhow::ensure!(!raw.is_empty(), "vps_rules_empty_value_invalid");
    anyhow::ensure!(
        raw.len() <= MAX_VPS_RULE_VALUE_BYTES,
        "vps_rules_value_too_long"
    );
    match key.as_str() {
        VPS_RULE_KEY_TRAFFIC_RESET_DAY => {
            let day = raw
                .parse::<i32>()
                .context("traffic.reset_day must be an integer")?;
            anyhow::ensure!((1..=31).contains(&day), "traffic_reset_day_invalid");
            Ok(ParsedRuleValue {
                raw: raw.to_string(),
                json: json!({"day": day}),
                display: format!("{day} UTC"),
            })
        }
        VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL
        | VPS_RULE_KEY_TRAFFIC_QUOTA_RX
        | VPS_RULE_KEY_TRAFFIC_QUOTA_TX => {
            let bytes = parse_byte_size(raw)?;
            Ok(ParsedRuleValue {
                raw: raw.to_string(),
                json: json!({"bytes": bytes, "display": display_bytes(bytes)}),
                display: format!("{} bytes", bytes),
            })
        }
        VPS_RULE_KEY_TRAFFIC_SELECTORS => {
            let selectors = parse_traffic_selector_list_with_options(raw, allow_direction_overlap)?;
            Ok(ParsedRuleValue {
                raw: selectors
                    .iter()
                    .map(|selector| selector.canonical.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
                json: json!({
                    "selectors": selectors.iter().map(|selector| {
                        json!({
                            "source": selector.source,
                            "interface": selector.interface,
                            "direction": selector.direction,
                            "canonical": selector.canonical,
                        })
                    }).collect::<Vec<_>>()
                }),
                display: format!("{} selectors", selectors.len()),
            })
        }
        _ => unreachable!("normalize_vps_rule_key rejects unsupported keys"),
    }
}

fn parse_traffic_selector_list(input: &str) -> Result<Vec<TrafficSelector>> {
    parse_traffic_selector_list_with_options(input, false)
}

fn parse_persisted_traffic_selector_list(input: &str) -> Result<Vec<TrafficSelector>> {
    parse_traffic_selector_list_with_options(input, true)
}

fn parse_traffic_selector_list_with_options(
    input: &str,
    allow_direction_overlap: bool,
) -> Result<Vec<TrafficSelector>> {
    let raw = input.trim();
    anyhow::ensure!(!raw.is_empty(), "traffic_selector_empty");
    let mut selectors = Vec::new();
    let mut seen = BTreeSet::new();
    let mut selected_directions = BTreeMap::<(String, String), u8>::new();
    for item in raw.split(',') {
        let selector = parse_traffic_selector(item)?;
        anyhow::ensure!(
            seen.insert(selector.canonical.clone()),
            "traffic_selector_duplicate"
        );
        let requested_directions = traffic_selector_direction_mask(&selector);
        let selected = selected_directions
            .entry((selector.source.clone(), selector.interface.clone()))
            .or_default();
        if !allow_direction_overlap {
            anyhow::ensure!(
                *selected & requested_directions == 0,
                "traffic_selector_direction_overlap"
            );
        }
        *selected |= requested_directions;
        selectors.push(selector);
    }
    anyhow::ensure!(
        selectors.len() <= MAX_TRAFFIC_SELECTOR_ITEMS,
        "traffic_selector_too_many_items"
    );
    Ok(selectors)
}

fn traffic_selector_direction_mask(selector: &TrafficSelector) -> u8 {
    match selector.direction.as_str() {
        "rx" => 0b01,
        "tx" => 0b10,
        _ => 0b11,
    }
}

fn claim_traffic_selector_directions(
    counted: &mut HashMap<(String, String), u8>,
    selector: &TrafficSelector,
) -> (bool, bool) {
    let requested = traffic_selector_direction_mask(selector);
    let counted = counted
        .entry((selector.source.clone(), selector.interface.clone()))
        .or_default();
    let newly_counted = requested & !*counted;
    *counted |= requested;
    (newly_counted & 0b01 != 0, newly_counted & 0b10 != 0)
}

fn parse_traffic_selector(item: &str) -> Result<TrafficSelector> {
    let item = item.trim();
    anyhow::ensure!(!item.is_empty(), "traffic_selector_empty_item");
    let (source, rest) = if let Some((source, rest)) = item.split_once(':') {
        let source = source.trim();
        anyhow::ensure!(
            source == "host" || source == "tunnel",
            "traffic_selector_source_invalid"
        );
        (source.to_string(), rest)
    } else {
        ("host".to_string(), item)
    };
    let (interface, direction) = if let Some((interface, direction)) = rest.split_once('+') {
        (interface.trim(), direction.trim())
    } else {
        (rest.trim(), "total")
    };
    anyhow::ensure!(!interface.is_empty(), "traffic_selector_interface_required");
    anyhow::ensure!(
        interface.len() <= MAX_TRAFFIC_INTERFACE_BYTES
            && !interface.chars().any(|ch| {
                ch == ',' || ch == '+' || ch == ':' || ch.is_whitespace() || ch.is_control()
            }),
        "traffic_selector_interface_invalid"
    );
    anyhow::ensure!(
        matches!(direction, "rx" | "tx" | "total"),
        "traffic_selector_direction_invalid"
    );
    Ok(TrafficSelector {
        canonical: if source == "host" {
            if direction == "total" {
                interface.to_string()
            } else {
                format!("{interface}+{direction}")
            }
        } else if direction == "total" {
            format!("{source}:{interface}")
        } else {
            format!("{source}:{interface}+{direction}")
        },
        source,
        interface: interface.to_string(),
        direction: direction.to_string(),
    })
}

fn parse_byte_size(input: &str) -> Result<i64> {
    let value = input.trim();
    anyhow::ensure!(!value.is_empty(), "byte_size_empty");
    let split_at = value
        .find(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .unwrap_or(value.len());
    let number = value[..split_at]
        .parse::<f64>()
        .map_err(|_| anyhow::anyhow!("byte_size_number_invalid"))?;
    anyhow::ensure!(
        number.is_finite() && number >= 0.0,
        "byte_size_number_invalid"
    );
    let suffix = value[split_at..].trim().to_ascii_lowercase();
    let multiplier = match suffix.as_str() {
        "" | "b" => 1_f64,
        "kb" => 1_000_f64,
        "mb" => 1_000_000_f64,
        "gb" => 1_000_000_000_f64,
        "tb" => 1_000_000_000_000_f64,
        "kib" => 1024_f64,
        "mib" => 1024_f64.powi(2),
        "gib" => 1024_f64.powi(3),
        "tib" => 1024_f64.powi(4),
        _ => anyhow::bail!("byte_size_unit_invalid"),
    };
    let bytes = (number * multiplier).round();
    anyhow::ensure!(bytes <= i64::MAX as f64, "byte_size_too_large");
    Ok(bytes as i64)
}

fn display_bytes(bytes: i64) -> String {
    const UNITS: [(&str, f64); 5] = [
        ("TB", 1_000_000_000_000.0),
        ("GB", 1_000_000_000.0),
        ("MB", 1_000_000.0),
        ("KB", 1_000.0),
        ("B", 1.0),
    ];
    for (unit, factor) in UNITS {
        if bytes as f64 >= factor || unit == "B" {
            let value = bytes as f64 / factor;
            return if unit == "B" {
                format!("{bytes} B")
            } else if value >= 10.0 {
                format!("{value:.0} {unit}")
            } else {
                format!("{value:.1} {unit}")
            };
        }
    }
    format!("{bytes} B")
}

fn resolve_agents(agents: &[AgentView], selector: &str) -> Result<Vec<AgentView>> {
    let expression = parse_selector_expression(selector)
        .map_err(|error| anyhow::anyhow!("invalid selector expression: {error}"))?
        .context("selector expression is empty")?;
    Ok(agents
        .iter()
        .filter(|agent| agent_matches_selector_expression(agent, &expression))
        .cloned()
        .collect())
}

fn policy_selector_partial_failure(failures: &[(Uuid, String, String)]) -> anyhow::Error {
    const MAX_REPORTED_FAILURES: usize = 8;
    const MAX_ERROR_CHARS: usize = 256;

    let details = failures
        .iter()
        .take(MAX_REPORTED_FAILURES)
        .map(|(policy_id, name, error)| {
            let name = name.chars().take(MAX_POLICY_NAME_BYTES).collect::<String>();
            let error = error.chars().take(MAX_ERROR_CHARS).collect::<String>();
            format!("{policy_id} ({name}): {error}")
        })
        .collect::<Vec<_>>()
        .join("; ");
    let omitted = failures.len().saturating_sub(MAX_REPORTED_FAILURES);
    anyhow::anyhow!(
        "fleet_alert_policy_evaluation_partial_failure: {} malformed persisted group selector(s): {}{}",
        failures.len(),
        details,
        if omitted == 0 {
            String::new()
        } else {
            format!("; {omitted} additional failure(s) omitted")
        }
    )
}

fn traffic_accounting_for_client(
    client_id: &str,
    rules: &[VpsRuleValueRecord],
    traffic_usage: &[TrafficCounterStreamUsage],
    now: DateTime<Utc>,
) -> TrafficAccountingRecord {
    traffic_accounting_for_client_with_selector_override(client_id, rules, traffic_usage, now, None)
}

fn traffic_accounting_for_agents(
    agents: &[AgentView],
    rules: &[VpsRuleValueRecord],
    traffic_usage: &[TrafficCounterStreamUsage],
    now: DateTime<Utc>,
) -> Vec<TrafficAccountingRecord> {
    agents
        .iter()
        .map(|agent| traffic_accounting_for_client(&agent.id, rules, traffic_usage, now))
        .collect()
}

fn traffic_override_for_rule(
    client_id: &str,
    rule: &PolicyRuleRequest,
    rules: &[VpsRuleValueRecord],
    traffic_usage: &[TrafficCounterStreamUsage],
    now: DateTime<Utc>,
) -> Option<TrafficAccountingRecord> {
    if !policy_condition_uses_traffic(&rule.condition_expression).unwrap_or(false) {
        return None;
    }
    let selector = rule
        .traffic_selector
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some(traffic_accounting_for_client_with_selector_override(
        client_id,
        rules,
        traffic_usage,
        now,
        Some(selector),
    ))
}

fn traffic_accounting_for_client_with_selector_override(
    client_id: &str,
    rules: &[VpsRuleValueRecord],
    traffic_usage: &[TrafficCounterStreamUsage],
    now: DateTime<Utc>,
    selector_override: Option<&str>,
) -> TrafficAccountingRecord {
    let rule_map = rules
        .iter()
        .filter(|rule| rule.client_id == client_id)
        .map(|rule| (rule.key.as_str(), rule))
        .collect::<HashMap<_, _>>();
    let mut incomplete_reasons = Vec::new();
    let reset_day = rule_map
        .get(VPS_RULE_KEY_TRAFFIC_RESET_DAY)
        .and_then(|rule| rule.value_json.get("day"))
        .and_then(Value::as_i64)
        .map(|value| value as i32);
    if reset_day.is_none() {
        incomplete_reasons.push("traffic.reset_day missing".to_string());
    }
    let (selectors, selector_error) = match selector_override {
        Some(selector) => match parse_persisted_traffic_selector_list(selector) {
            Ok(selectors) => (selectors, None),
            Err(error) => (
                Vec::new(),
                Some(format!("traffic.policy_selector invalid: {error}")),
            ),
        },
        None => match rule_map.get(VPS_RULE_KEY_TRAFFIC_SELECTORS) {
            Some(rule) => match parse_persisted_traffic_selector_list(&rule.value_raw) {
                Ok(selectors) => (selectors, None),
                Err(error) => (
                    Vec::new(),
                    Some(format!("traffic.selectors invalid: {error}")),
                ),
            },
            None => (Vec::new(), None),
        },
    };
    if let Some(error) = selector_error {
        incomplete_reasons.push(error);
    } else if selectors.is_empty() {
        incomplete_reasons.push("traffic.selectors missing".to_string());
    }
    let quota_total = quota_value(&rule_map, VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL);
    let quota_rx = quota_value(&rule_map, VPS_RULE_KEY_TRAFFIC_QUOTA_RX);
    let quota_tx = quota_value(&rule_map, VPS_RULE_KEY_TRAFFIC_QUOTA_TX);
    if quota_total.is_none() && quota_rx.is_none() && quota_tx.is_none() {
        incomplete_reasons.push("traffic quota missing".to_string());
    }
    let (cycle_start, cycle_end) = cycle_bounds(reset_day.unwrap_or(1), now);
    let mut rx_bytes = 0_i64;
    let mut tx_bytes = 0_i64;
    let mut latest_rx = 0_i64;
    let mut latest_tx = 0_i64;
    let mut last_sample_unix = None::<i64>;
    let mut counter_epochs_seen = 0_i64;
    let mut counted_streams = HashSet::new();
    let mut counted_directions = HashMap::new();
    let mut stale_reasons = Vec::new();
    let mut breakdown = Vec::new();
    for selector in &selectors {
        let selected_usage = traffic_usage
            .iter()
            .find(|usage| {
                usage.client_id == client_id
                    && usage.source_kind == selector.source
                    && usage.interface == selector.interface
                    && usage.last_sample_unix <= now.timestamp()
            })
            .cloned();
        let Some(usage) = selected_usage else {
            breakdown.push(TrafficAccountingSelectorBreakdown {
                source: selector.source.clone(),
                interface: selector.interface.clone(),
                direction: selector.direction.clone(),
                latest_rx_bytes: 0,
                latest_tx_bytes: 0,
                cycle_rx_bytes: 0,
                cycle_tx_bytes: 0,
                cycle_total_bytes: 0,
                sample_age_secs: None,
                state: "incomplete".to_string(),
                incomplete_reasons: vec!["runtime interface data missing".to_string()],
            });
            incomplete_reasons.push(format!("{} sample missing", selector.canonical));
            continue;
        };
        if counted_streams.insert((usage.source_kind.clone(), usage.interface.clone())) {
            counter_epochs_seen = counter_epochs_seen.saturating_add(usage.counter_epochs_seen);
        }
        last_sample_unix = Some(last_sample_unix.map_or(usage.last_sample_unix, |last| {
            last.min(usage.last_sample_unix)
        }));
        let sample_age = Some(now.timestamp() - usage.last_sample_unix);
        let mut selected_cycle_rx = usage.cycle_rx;
        let mut selected_cycle_tx = usage.cycle_tx;
        let mut selected_latest_rx = usage.latest_rx;
        let mut selected_latest_tx = usage.latest_tx;
        match selector.direction.as_str() {
            "rx" => {
                selected_cycle_tx = 0;
                selected_latest_tx = 0;
            }
            "tx" => {
                selected_cycle_rx = 0;
                selected_latest_rx = 0;
            }
            _ => {}
        }
        let (count_rx, count_tx) =
            claim_traffic_selector_directions(&mut counted_directions, selector);
        if !count_rx {
            selected_cycle_rx = 0;
            selected_latest_rx = 0;
        }
        if !count_tx {
            selected_cycle_tx = 0;
            selected_latest_tx = 0;
        }
        rx_bytes += selected_cycle_rx;
        tx_bytes += selected_cycle_tx;
        latest_rx += selected_latest_rx;
        latest_tx += selected_latest_tx;
        let mut row_state = "ok".to_string();
        let mut row_reasons = Vec::new();
        if sample_age.is_some_and(|age| age > TRAFFIC_SAMPLE_STALE_SECS) {
            row_state = "stale".to_string();
            row_reasons.push("stale sample".to_string());
            stale_reasons.push(format!("{} sample stale", selector.canonical));
        }
        breakdown.push(TrafficAccountingSelectorBreakdown {
            source: selector.source.clone(),
            interface: selector.interface.clone(),
            direction: selector.direction.clone(),
            latest_rx_bytes: selected_latest_rx,
            latest_tx_bytes: selected_latest_tx,
            cycle_rx_bytes: selected_cycle_rx,
            cycle_tx_bytes: selected_cycle_tx,
            cycle_total_bytes: selected_cycle_rx + selected_cycle_tx,
            sample_age_secs: sample_age,
            state: row_state,
            incomplete_reasons: row_reasons,
        });
    }
    let total_bytes = rx_bytes + tx_bytes;
    let latest_total = latest_rx + latest_tx;
    let cycle_percent = [
        quota_total.map(|quota| percent(total_bytes, quota)),
        quota_rx.map(|quota| percent(rx_bytes, quota)),
        quota_tx.map(|quota| percent(tx_bytes, quota)),
    ]
    .into_iter()
    .flatten()
    .max_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let state = if !incomplete_reasons.is_empty() {
        "incomplete"
    } else if last_sample_unix.is_none() {
        "unknown"
    } else if !stale_reasons.is_empty() {
        "stale"
    } else {
        "ok"
    };
    incomplete_reasons.extend(stale_reasons);
    let selector_hash = selector_hash(
        &selectors
            .iter()
            .map(|selector| selector.canonical.clone())
            .collect::<Vec<_>>(),
    );
    TrafficAccountingRecord {
        client_id: client_id.to_string(),
        selectors: selectors
            .iter()
            .map(|selector| selector.canonical.clone())
            .collect(),
        selector_hash,
        cycle_start: cycle_start.to_rfc3339(),
        cycle_end: cycle_end.to_rfc3339(),
        reset_day,
        rx_bytes,
        tx_bytes,
        total_bytes,
        latest_rx_bytes: latest_rx,
        latest_tx_bytes: latest_tx,
        latest_total_bytes: latest_total,
        quota_rx_bytes: quota_rx,
        quota_tx_bytes: quota_tx,
        quota_total_bytes: quota_total,
        cycle_percent,
        state: state.to_string(),
        incomplete_reasons,
        last_sample_at: last_sample_unix
            .and_then(|unix| Utc.timestamp_opt(unix, 0).single())
            .map(|value| value.to_rfc3339()),
        counter_epochs_seen,
        updated_at: now.to_rfc3339(),
        selector_breakdown: breakdown,
    }
}

#[derive(Default)]
struct CycleUsage {
    cycle_rx: i64,
    cycle_tx: i64,
    latest_rx: i64,
    latest_tx: i64,
    last_sample_unix: Option<i64>,
}

fn derive_cycle_usage(
    samples: &[TrafficCounterSampleRecord],
    cycle_start_unix: i64,
    now_unix: i64,
) -> CycleUsage {
    let mut sorted = samples.to_vec();
    sorted.sort_by_key(|sample| sample.observed_unix);
    let mut usage = CycleUsage::default();
    let mut previous: Option<TrafficCounterSampleRecord> = None;
    for sample in sorted {
        if sample.observed_unix > now_unix {
            continue;
        }
        usage.latest_rx = sample.rx_bytes;
        usage.latest_tx = sample.tx_bytes;
        usage.last_sample_unix = Some(sample.observed_unix);
        if let Some(prev) = previous.as_ref() {
            if sample.observed_unix >= cycle_start_unix {
                let same_epoch = sample.counter_epoch == prev.counter_epoch;
                let rx_delta = if same_epoch && sample.rx_bytes >= prev.rx_bytes {
                    sample.rx_bytes - prev.rx_bytes
                } else {
                    0
                };
                let tx_delta = if same_epoch && sample.tx_bytes >= prev.tx_bytes {
                    sample.tx_bytes - prev.tx_bytes
                } else {
                    0
                };
                usage.cycle_rx += rx_delta;
                usage.cycle_tx += tx_delta;
            }
        }
        previous = Some(sample);
    }
    usage
}

fn quota_value(rule_map: &HashMap<&str, &VpsRuleValueRecord>, key: &str) -> Option<i64> {
    rule_map
        .get(key)
        .and_then(|rule| rule.value_json.get("bytes"))
        .and_then(Value::as_i64)
}

fn percent(value: i64, quota: i64) -> f64 {
    if quota <= 0 {
        0.0
    } else {
        (value as f64 / quota as f64) * 100.0
    }
}

fn cycle_bounds(reset_day: i32, now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let current_boundary = boundary_for_month(now.year(), now.month(), reset_day);
    if now >= current_boundary {
        let (next_year, next_month) = if now.month() == 12 {
            (now.year() + 1, 1)
        } else {
            (now.year(), now.month() + 1)
        };
        (
            current_boundary,
            boundary_for_month(next_year, next_month, reset_day),
        )
    } else {
        let (prev_year, prev_month) = if now.month() == 1 {
            (now.year() - 1, 12)
        } else {
            (now.year(), now.month() - 1)
        };
        (
            boundary_for_month(prev_year, prev_month, reset_day),
            current_boundary,
        )
    }
}

fn boundary_for_month(year: i32, month: u32, reset_day: i32) -> DateTime<Utc> {
    let day = reset_day.clamp(1, days_in_month(year, month) as i32) as u32;
    Utc.with_ymd_and_hms(year, month, day, 0, 0, 0)
        .single()
        .expect("valid clamped UTC cycle boundary")
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let first_next = Utc
        .with_ymd_and_hms(next_year, next_month, 1, 0, 0, 0)
        .single()
        .expect("valid next month");
    (first_next - chrono::Duration::days(1)).day()
}

async fn lock_policy_group_identity_upserts_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind("vpsman:policy-group-identity-upserts")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn policy_groups_for_identity_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    requested_id: Option<Uuid>,
    requested_name: &str,
) -> Result<Vec<PolicyGroupRecord>> {
    let rows = sqlx::query(
        r#"
        SELECT
            id,
            name,
            enabled,
            selector_expression,
            notes,
            created_by,
            updated_by,
            created_at::text AS created_at,
            updated_at::text AS updated_at
        FROM policy_groups
        WHERE ($1::uuid IS NOT NULL AND id = $1)
           OR name = $2
        ORDER BY id
        FOR UPDATE
        "#,
    )
    .bind(requested_id)
    .bind(requested_name)
    .fetch_all(&mut **tx)
    .await?;
    let group_ids = rows
        .iter()
        .map(|row| row.try_get::<Uuid, _>("id"))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut rules_by_group = HashMap::<Uuid, Vec<PolicyRuleRecord>>::new();
    if !group_ids.is_empty() {
        for row in sqlx::query(
            r#"
            SELECT
                id,
                group_id,
                rule_version,
                sort_order,
                name,
                enabled,
                traffic_selector,
                condition_expression,
                window_secs,
                severity,
                created_at::text AS created_at,
                updated_at::text AS updated_at
            FROM policy_rules
            WHERE group_id = ANY($1)
            ORDER BY group_id ASC, sort_order ASC, created_at ASC
            "#,
        )
        .bind(&group_ids)
        .fetch_all(&mut **tx)
        .await?
        {
            let rule = policy_rule_from_row(row)?;
            rules_by_group.entry(rule.group_id).or_default().push(rule);
        }
    }
    rows.into_iter()
        .map(|row| {
            let group_id: Uuid = row.try_get("id")?;
            policy_group_from_row(row, rules_by_group.remove(&group_id).unwrap_or_default())
        })
        .collect()
}

fn select_existing_policy_group(
    groups: &[PolicyGroupRecord],
    requested_id: Option<Uuid>,
    requested_name: &str,
) -> Result<Option<PolicyGroupRecord>> {
    let existing_by_id = requested_id.and_then(|id| groups.iter().find(|group| group.id == id));
    let existing_by_name = groups
        .iter()
        .find(|group| group.name == requested_name.trim());
    if let (Some(requested_id), Some(named_group)) = (requested_id, existing_by_name) {
        anyhow::ensure!(
            named_group.id == requested_id,
            "fleet_alert_policy_name_conflict"
        );
    }
    Ok(existing_by_id.or(existing_by_name).cloned())
}

fn policy_group_from_request(
    request: &CreateFleetAlertPolicyRequest,
    dry_run: &PolicyDryRunResponse,
    now: &str,
    existing_group: Option<&PolicyGroupRecord>,
    operator: &AuthContext,
) -> Result<PolicyGroupRecord> {
    let group_id = existing_group
        .map(|group| group.id)
        .or(request.id)
        .unwrap_or_else(Uuid::new_v4);
    let rules = request
        .rules
        .iter()
        .enumerate()
        .map(|(index, rule)| {
            policy_rule_from_request(group_id, rule, index as i32, now, existing_group)
        })
        .collect::<Vec<_>>();
    anyhow::ensure!(
        rules
            .iter()
            .map(|rule| rule.id)
            .collect::<HashSet<_>>()
            .len()
            == rules.len(),
        "fleet_alert_policy_rule_identity_conflict"
    );
    Ok(PolicyGroupRecord {
        id: group_id,
        name: request.name.trim().to_string(),
        enabled: request.enabled,
        selector_expression: request.selector_expression.trim().to_string(),
        notes: clean_optional_text(request.notes.as_deref()),
        matched_vps_count: dry_run.matched_vps_count as i64,
        rule_count: rules.len() as i64,
        enabled_rule_count: rules.iter().filter(|rule| rule.enabled).count() as i64,
        active_warning_count: 0,
        active_critical_count: 0,
        incomplete_vps_count: dry_run.incomplete_vps_count as i64,
        last_evaluated_at: None,
        rules,
        created_by: existing_group
            .and_then(|existing| existing.created_by)
            .or(Some(operator.operator.id)),
        updated_by: Some(operator.operator.id),
        created_at: existing_group
            .map(|existing| existing.created_at.clone())
            .unwrap_or_else(|| now.to_string()),
        updated_at: now.to_string(),
    })
}

fn policy_group_scope_changed(
    existing_group: Option<&PolicyGroupRecord>,
    group: &PolicyGroupRecord,
) -> bool {
    existing_group.is_some_and(|existing| {
        existing.enabled != group.enabled
            || existing.selector_expression != group.selector_expression
    })
}

fn validate_policy_group_request(
    request: &CreateFleetAlertPolicyRequest,
    require_confirmed: bool,
    require_names: bool,
) -> Result<()> {
    if require_confirmed {
        anyhow::ensure!(
            request.confirmed,
            "fleet_alert_policy_confirmation_required"
        );
    }
    if require_names {
        validate_name(
            &request.name,
            MAX_POLICY_NAME_BYTES,
            "fleet alert policy name",
        )?;
    }
    anyhow::ensure!(
        !request.selector_expression.trim().is_empty()
            && request.selector_expression.len() <= MAX_SELECTOR_EXPRESSION_BYTES,
        "fleet alert policy selector expression is invalid"
    );
    parse_selector_expression(&request.selector_expression)
        .map_err(|error| anyhow::anyhow!("invalid selector expression: {error}"))?
        .context("selector expression is empty")?;
    if let Some(notes) = request.notes.as_deref() {
        anyhow::ensure!(
            notes.len() <= MAX_POLICY_NOTES_BYTES,
            "fleet alert policy notes are too long"
        );
    }
    anyhow::ensure!(
        !request.rules.is_empty(),
        "fleet alert policy requires at least one rule"
    );
    let mut rule_ids = HashSet::new();
    for rule in &request.rules {
        if let Some(rule_id) = rule.id {
            anyhow::ensure!(
                rule_ids.insert(rule_id),
                "fleet_alert_policy_duplicate_rule_id"
            );
        }
        if require_names {
            validate_policy_rule_request(rule)?;
        } else {
            validate_policy_rule_request_for_preview(rule)?;
        }
    }
    Ok(())
}

fn validate_policy_rule_request(rule: &PolicyRuleRequest) -> Result<()> {
    validate_name(
        &rule.name,
        MAX_RULE_NAME_BYTES,
        "fleet alert policy rule name",
    )?;
    validate_policy_rule_request_for_preview(rule)
}

fn validate_policy_rule_request_for_preview(rule: &PolicyRuleRequest) -> Result<()> {
    anyhow::ensure!(
        matches!(rule.severity.as_str(), "info" | "warning" | "critical"),
        "fleet_alert_policy_severity_invalid"
    );
    anyhow::ensure!(
        matches!(rule.window_secs, 0 | 60 | 300 | 900),
        "fleet_alert_policy_window_invalid"
    );
    anyhow::ensure!(
        !rule.condition_expression.trim().is_empty()
            && rule.condition_expression.len() <= MAX_CONDITION_EXPRESSION_BYTES,
        "fleet_alert_policy_condition_invalid"
    );
    parse_policy_condition_expression(&rule.condition_expression)
        .map_err(|error| anyhow::anyhow!("fleet_alert_policy_condition_invalid: {error}"))?;
    if let Some(selector) = rule
        .traffic_selector
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        parse_traffic_selector_list(selector)?;
    }
    if !policy_condition_uses_traffic(&rule.condition_expression)? {
        anyhow::ensure!(
            rule.traffic_selector
                .as_deref()
                .is_none_or(|value| value.trim().is_empty()),
            "fleet_alert_policy_traffic_selector_requires_traffic_metric"
        );
    }
    Ok(())
}

fn validate_name(value: &str, max_bytes: usize, field: &str) -> Result<()> {
    let value = value.trim();
    anyhow::ensure!(!value.is_empty(), "{field} is required");
    anyhow::ensure!(value.len() <= max_bytes, "{field} is too long");
    anyhow::ensure!(
        value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'.' | b'_' | b'-' | b':')
        }),
        "{field} contains unsupported characters"
    );
    Ok(())
}

fn clean_optional_text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn policy_rule_from_request(
    group_id: Uuid,
    request: &PolicyRuleRequest,
    sort_order: i32,
    now: &str,
    existing_group: Option<&PolicyGroupRecord>,
) -> PolicyRuleRecord {
    let existing_rule = existing_group.and_then(|group| {
        request
            .id
            .and_then(|id| group.rules.iter().find(|rule| rule.id == id))
            .or_else(|| {
                if request.id.is_none() {
                    group
                        .rules
                        .iter()
                        .find(|rule| rule.sort_order == sort_order)
                } else {
                    None
                }
            })
    });
    let rule_version = existing_rule
        .map(|existing| {
            if policy_rule_material_matches(existing, request) {
                existing.rule_version
            } else {
                existing.rule_version.saturating_add(1)
            }
        })
        .unwrap_or(1);
    PolicyRuleRecord {
        id: existing_rule
            .map(|rule| rule.id)
            .or(request.id)
            .unwrap_or_else(Uuid::new_v4),
        group_id,
        rule_version,
        sort_order,
        name: request.name.trim().to_string(),
        enabled: request.enabled,
        traffic_selector: request
            .traffic_selector
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        condition_expression: request.condition_expression.trim().to_string(),
        window_secs: request.window_secs,
        severity: request.severity.trim().to_string(),
        created_at: existing_rule
            .map(|rule| rule.created_at.clone())
            .unwrap_or_else(|| now.to_string()),
        updated_at: now.to_string(),
    }
}

fn policy_rule_material_matches(existing: &PolicyRuleRecord, request: &PolicyRuleRequest) -> bool {
    existing.name == request.name.trim()
        && existing.enabled == request.enabled
        && existing.traffic_selector.as_deref()
            == request
                .traffic_selector
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        && existing.condition_expression == request.condition_expression.trim()
        && existing.window_secs == request.window_secs
        && existing.severity == request.severity.trim()
}

fn evaluate_rule_for_client(
    rule: &PolicyRuleRequest,
    traffic: Option<&TrafficAccountingRecord>,
    rollup: Option<&TelemetryRollupView>,
) -> PolicyEvaluation {
    let mut incomplete_reasons = Vec::new();
    let parsed = match parse_policy_condition_expression(&rule.condition_expression) {
        Ok(parsed) => parsed,
        Err(error) => {
            incomplete_reasons.push(format!("condition expression invalid: {error}"));
            return policy_evaluation_from_parts(
                false,
                incomplete_reasons,
                None,
                None,
                "resource",
                traffic,
            );
        }
    };
    let result = match evaluate_policy_condition(&parsed, traffic, rollup, &mut incomplete_reasons)
    {
        Ok(result) => result,
        Err(error) => {
            incomplete_reasons.push(format!("condition expression invalid: {error}"));
            ConditionEvaluation {
                condition_true: false,
                actual_value: None,
                threshold_value: None,
            }
        }
    };
    let category = if parsed.uses_traffic {
        "traffic"
    } else {
        "resource"
    };
    let condition_true = result.condition_true && incomplete_reasons.is_empty();
    policy_evaluation_from_parts(
        condition_true,
        incomplete_reasons,
        result.actual_value,
        result.threshold_value,
        category,
        traffic,
    )
}

fn policy_evaluation_from_parts(
    condition_true: bool,
    incomplete_reasons: Vec<String>,
    actual_value: Option<f64>,
    threshold_value: Option<f64>,
    category: &str,
    traffic: Option<&TrafficAccountingRecord>,
) -> PolicyEvaluation {
    let payload = if let Some(traffic) = traffic {
        json!({
            "traffic": {
                "selectors": traffic.selectors,
                "cycle_start": traffic.cycle_start,
                "cycle_end": traffic.cycle_end,
                "rx_bytes": traffic.rx_bytes,
                "tx_bytes": traffic.tx_bytes,
                "total_bytes": traffic.total_bytes,
                "quota_rx_bytes": traffic.quota_rx_bytes,
                "quota_tx_bytes": traffic.quota_tx_bytes,
                "quota_total_bytes": traffic.quota_total_bytes,
                "cycle_percent": traffic.cycle_percent,
                "reset_day": traffic.reset_day,
            }
        })
    } else {
        json!({})
    };
    PolicyEvaluation {
        condition_true,
        incomplete: !incomplete_reasons.is_empty(),
        incomplete_reasons,
        actual_value,
        threshold_value,
        category: category.to_string(),
        payload,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArithmeticOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    UnaryPlus,
    UnaryMinus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PolicyComparisonOperator {
    Eq,
    NotEq,
    Lt,
    Lte,
    Gt,
    Gte,
}

#[derive(Clone, Debug)]
enum PolicyConditionNode {
    Not(Box<PolicyConditionNode>),
    And(Box<PolicyConditionNode>, Box<PolicyConditionNode>),
    Or(Box<PolicyConditionNode>, Box<PolicyConditionNode>),
    Comparison {
        left: PolicyNumericNode,
        operator: PolicyComparisonOperator,
        right: PolicyNumericNode,
    },
}

#[derive(Clone, Debug)]
enum PolicyNumericNode {
    Number(f64),
    Identifier(String),
    Unary {
        operator: ArithmeticOperator,
        operand: Box<PolicyNumericNode>,
    },
    Binary {
        left: Box<PolicyNumericNode>,
        operator: ArithmeticOperator,
        right: Box<PolicyNumericNode>,
    },
}

#[derive(Clone, Debug)]
struct PolicyConditionExpression {
    root: PolicyConditionNode,
    uses_traffic: bool,
}

#[derive(Clone, Debug)]
struct ConditionEvaluation {
    condition_true: bool,
    actual_value: Option<f64>,
    threshold_value: Option<f64>,
}

#[derive(Clone, Debug)]
enum PolicyConditionToken {
    Number(f64),
    Identifier(String),
    Arithmetic(ArithmeticOperator),
    Comparison(PolicyComparisonOperator),
    And,
    Or,
    Not,
    LeftParen,
    RightParen,
}

#[derive(Clone)]
struct PolicyConditionParser {
    tokens: Vec<PolicyConditionToken>,
    position: usize,
}

fn parse_policy_condition_expression(expression: &str) -> Result<PolicyConditionExpression> {
    let tokens = tokenize_policy_condition(expression)?;
    anyhow::ensure!(!tokens.is_empty(), "condition expression is empty");
    let mut parser = PolicyConditionParser {
        tokens,
        position: 0,
    };
    let root = parser.parse_or()?;
    anyhow::ensure!(
        parser.peek().is_none(),
        "unexpected token after condition expression"
    );
    let uses_traffic = condition_node_uses_traffic(&root);
    Ok(PolicyConditionExpression { root, uses_traffic })
}

fn policy_condition_uses_traffic(expression: &str) -> Result<bool> {
    Ok(parse_policy_condition_expression(expression)?.uses_traffic)
}

fn policy_rule_category(rule: &PolicyRuleRequest) -> String {
    match parse_policy_condition_expression(&rule.condition_expression) {
        Ok(parsed) if parsed.uses_traffic => "traffic".to_string(),
        _ => "resource".to_string(),
    }
}

fn evaluate_policy_condition(
    expression: &PolicyConditionExpression,
    traffic: Option<&TrafficAccountingRecord>,
    rollup: Option<&TelemetryRollupView>,
    incomplete: &mut Vec<String>,
) -> Result<ConditionEvaluation> {
    let mut first_pair = None;
    let condition_true = evaluate_condition_node(
        &expression.root,
        traffic,
        rollup,
        incomplete,
        &mut first_pair,
    )?;
    let (actual_value, threshold_value) = first_pair.unwrap_or((None, None));
    Ok(ConditionEvaluation {
        condition_true,
        actual_value,
        threshold_value,
    })
}

fn evaluate_condition_node(
    node: &PolicyConditionNode,
    traffic: Option<&TrafficAccountingRecord>,
    rollup: Option<&TelemetryRollupView>,
    incomplete: &mut Vec<String>,
    first_pair: &mut Option<(Option<f64>, Option<f64>)>,
) -> Result<bool> {
    match node {
        PolicyConditionNode::Not(inner) => Ok(!evaluate_condition_node(
            inner, traffic, rollup, incomplete, first_pair,
        )?),
        PolicyConditionNode::And(left, right) => {
            let left_value =
                evaluate_condition_node(left, traffic, rollup, incomplete, first_pair)?;
            let right_value =
                evaluate_condition_node(right, traffic, rollup, incomplete, first_pair)?;
            Ok(left_value && right_value)
        }
        PolicyConditionNode::Or(left, right) => {
            let left_value =
                evaluate_condition_node(left, traffic, rollup, incomplete, first_pair)?;
            let right_value =
                evaluate_condition_node(right, traffic, rollup, incomplete, first_pair)?;
            Ok(left_value || right_value)
        }
        PolicyConditionNode::Comparison {
            left,
            operator,
            right,
        } => {
            let left_value = evaluate_numeric_node(left, traffic, rollup, incomplete)?;
            let right_value = evaluate_numeric_node(right, traffic, rollup, incomplete)?;
            if first_pair.is_none() {
                *first_pair = Some((left_value, right_value));
            }
            Ok(left_value
                .zip(right_value)
                .map(|(left, right)| compare_policy_values(left, right, *operator))
                .unwrap_or(false))
        }
    }
}

fn evaluate_numeric_node(
    node: &PolicyNumericNode,
    traffic: Option<&TrafficAccountingRecord>,
    rollup: Option<&TelemetryRollupView>,
    incomplete: &mut Vec<String>,
) -> Result<Option<f64>> {
    let value = match node {
        PolicyNumericNode::Number(value) => Some(*value),
        PolicyNumericNode::Identifier(identifier) => {
            policy_identifier_value(identifier, traffic, rollup, incomplete)
        }
        PolicyNumericNode::Unary { operator, operand } => {
            let value = evaluate_numeric_node(operand, traffic, rollup, incomplete)?;
            value.map(|value| match operator {
                ArithmeticOperator::UnaryPlus => value,
                ArithmeticOperator::UnaryMinus => -value,
                _ => value,
            })
        }
        PolicyNumericNode::Binary {
            left,
            operator,
            right,
        } => {
            let left = evaluate_numeric_node(left, traffic, rollup, incomplete)?;
            let right = evaluate_numeric_node(right, traffic, rollup, incomplete)?;
            match left.zip(right) {
                Some((left, right)) => {
                    let result = match operator {
                        ArithmeticOperator::Add => left + right,
                        ArithmeticOperator::Subtract => left - right,
                        ArithmeticOperator::Multiply => left * right,
                        ArithmeticOperator::Divide => {
                            anyhow::ensure!(right != 0.0, "condition division by zero");
                            left / right
                        }
                        ArithmeticOperator::UnaryPlus | ArithmeticOperator::UnaryMinus => {
                            anyhow::bail!("invalid unary operator placement")
                        }
                    };
                    anyhow::ensure!(
                        result.is_finite(),
                        "condition numeric result must be finite"
                    );
                    Some(result)
                }
                None => None,
            }
        }
    };
    Ok(value)
}

impl PolicyConditionParser {
    fn parse_or(&mut self) -> Result<PolicyConditionNode> {
        let mut node = self.parse_and()?;
        while matches!(self.peek(), Some(PolicyConditionToken::Or)) {
            self.position += 1;
            let right = self.parse_and()?;
            node = PolicyConditionNode::Or(Box::new(node), Box::new(right));
        }
        Ok(node)
    }

    fn parse_and(&mut self) -> Result<PolicyConditionNode> {
        let mut node = self.parse_not()?;
        while matches!(self.peek(), Some(PolicyConditionToken::And)) {
            self.position += 1;
            let right = self.parse_not()?;
            node = PolicyConditionNode::And(Box::new(node), Box::new(right));
        }
        Ok(node)
    }

    fn parse_not(&mut self) -> Result<PolicyConditionNode> {
        if matches!(self.peek(), Some(PolicyConditionToken::Not)) {
            self.position += 1;
            return Ok(PolicyConditionNode::Not(Box::new(self.parse_not()?)));
        }
        self.parse_boolean_primary()
    }

    fn parse_boolean_primary(&mut self) -> Result<PolicyConditionNode> {
        let snapshot = self.clone();
        if let Ok(comparison) = self.parse_comparison() {
            return Ok(comparison);
        }
        *self = snapshot;
        if matches!(self.peek(), Some(PolicyConditionToken::LeftParen)) {
            self.position += 1;
            let node = self.parse_or()?;
            self.expect_right_paren()?;
            return Ok(node);
        }
        anyhow::bail!("condition expression must compare numeric expressions")
    }

    fn parse_comparison(&mut self) -> Result<PolicyConditionNode> {
        let left = self.parse_numeric_expression()?;
        let operator = match self.next() {
            Some(PolicyConditionToken::Comparison(operator)) => operator,
            _ => anyhow::bail!("condition comparison operator is required"),
        };
        let right = self.parse_numeric_expression()?;
        Ok(PolicyConditionNode::Comparison {
            left,
            operator,
            right,
        })
    }

    fn parse_numeric_expression(&mut self) -> Result<PolicyNumericNode> {
        let mut node = self.parse_numeric_term()?;
        loop {
            let operator = match self.peek() {
                Some(PolicyConditionToken::Arithmetic(ArithmeticOperator::Add)) => {
                    ArithmeticOperator::Add
                }
                Some(PolicyConditionToken::Arithmetic(ArithmeticOperator::Subtract)) => {
                    ArithmeticOperator::Subtract
                }
                _ => break,
            };
            self.position += 1;
            let right = self.parse_numeric_term()?;
            node = PolicyNumericNode::Binary {
                left: Box::new(node),
                operator,
                right: Box::new(right),
            };
        }
        Ok(node)
    }

    fn parse_numeric_term(&mut self) -> Result<PolicyNumericNode> {
        let mut node = self.parse_numeric_factor()?;
        loop {
            let operator = match self.peek() {
                Some(PolicyConditionToken::Arithmetic(ArithmeticOperator::Multiply)) => {
                    ArithmeticOperator::Multiply
                }
                Some(PolicyConditionToken::Arithmetic(ArithmeticOperator::Divide)) => {
                    ArithmeticOperator::Divide
                }
                _ => break,
            };
            self.position += 1;
            let right = self.parse_numeric_factor()?;
            node = PolicyNumericNode::Binary {
                left: Box::new(node),
                operator,
                right: Box::new(right),
            };
        }
        Ok(node)
    }

    fn parse_numeric_factor(&mut self) -> Result<PolicyNumericNode> {
        match self.next() {
            Some(PolicyConditionToken::Number(value)) => Ok(PolicyNumericNode::Number(value)),
            Some(PolicyConditionToken::Identifier(identifier)) => {
                Ok(PolicyNumericNode::Identifier(identifier))
            }
            Some(PolicyConditionToken::Arithmetic(ArithmeticOperator::Add)) => {
                Ok(PolicyNumericNode::Unary {
                    operator: ArithmeticOperator::UnaryPlus,
                    operand: Box::new(self.parse_numeric_factor()?),
                })
            }
            Some(PolicyConditionToken::Arithmetic(ArithmeticOperator::Subtract)) => {
                Ok(PolicyNumericNode::Unary {
                    operator: ArithmeticOperator::UnaryMinus,
                    operand: Box::new(self.parse_numeric_factor()?),
                })
            }
            Some(PolicyConditionToken::LeftParen) => {
                let node = self.parse_numeric_expression()?;
                self.expect_right_paren()?;
                Ok(node)
            }
            _ => anyhow::bail!("numeric expression operand is required"),
        }
    }

    fn expect_right_paren(&mut self) -> Result<()> {
        match self.next() {
            Some(PolicyConditionToken::RightParen) => Ok(()),
            _ => anyhow::bail!("condition expression has unmatched '('"),
        }
    }

    fn peek(&self) -> Option<&PolicyConditionToken> {
        self.tokens.get(self.position)
    }

    fn next(&mut self) -> Option<PolicyConditionToken> {
        let token = self.tokens.get(self.position).cloned();
        if token.is_some() {
            self.position += 1;
        }
        token
    }
}

fn tokenize_policy_condition(expression: &str) -> Result<Vec<PolicyConditionToken>> {
    let input = expression.trim();
    anyhow::ensure!(!input.is_empty(), "condition expression is empty");
    let chars = input.char_indices().collect::<Vec<_>>();
    let mut tokens = Vec::new();
    let mut index = 0_usize;
    while index < chars.len() {
        let (byte_index, ch) = chars[index];
        if ch.is_whitespace() {
            index += 1;
            continue;
        }
        match ch {
            '(' => {
                tokens.push(PolicyConditionToken::LeftParen);
                index += 1;
            }
            ')' => {
                tokens.push(PolicyConditionToken::RightParen);
                index += 1;
            }
            '&' => {
                anyhow::ensure!(
                    chars.get(index + 1).is_some_and(|(_, next)| *next == '&'),
                    "condition '&' must be written as &&"
                );
                tokens.push(PolicyConditionToken::And);
                index += 2;
            }
            '|' => {
                anyhow::ensure!(
                    chars.get(index + 1).is_some_and(|(_, next)| *next == '|'),
                    "condition '|' must be written as ||"
                );
                tokens.push(PolicyConditionToken::Or);
                index += 2;
            }
            '!' => {
                if chars.get(index + 1).is_some_and(|(_, next)| *next == '=') {
                    tokens.push(PolicyConditionToken::Comparison(
                        PolicyComparisonOperator::NotEq,
                    ));
                    index += 2;
                } else {
                    tokens.push(PolicyConditionToken::Not);
                    index += 1;
                }
            }
            '~' => {
                tokens.push(PolicyConditionToken::Not);
                index += 1;
            }
            '>' | '<' | '=' => {
                let next_is_equal = chars.get(index + 1).is_some_and(|(_, next)| *next == '=');
                let operator = match (ch, next_is_equal) {
                    ('>', true) => PolicyComparisonOperator::Gte,
                    ('>', false) => PolicyComparisonOperator::Gt,
                    ('<', true) => PolicyComparisonOperator::Lte,
                    ('<', false) => PolicyComparisonOperator::Lt,
                    ('=', true) | ('=', false) => PolicyComparisonOperator::Eq,
                    _ => unreachable!("comparison branch only handles comparison tokens"),
                };
                tokens.push(PolicyConditionToken::Comparison(operator));
                index += if next_is_equal { 2 } else { 1 };
            }
            '+' | '-' | '*' | '/' => {
                let operator = match ch {
                    '+' => ArithmeticOperator::Add,
                    '-' => ArithmeticOperator::Subtract,
                    '*' => ArithmeticOperator::Multiply,
                    '/' => ArithmeticOperator::Divide,
                    _ => unreachable!("arithmetic branch only handles arithmetic tokens"),
                };
                tokens.push(PolicyConditionToken::Arithmetic(operator));
                index += 1;
            }
            _ if ch.is_ascii_digit() || ch == '.' => {
                let start = byte_index;
                let mut end = byte_index + ch.len_utf8();
                index += 1;
                while index < chars.len() {
                    let (next_index, next_ch) = chars[index];
                    if next_ch.is_ascii_alphanumeric() || matches!(next_ch, '.' | '_') {
                        end = next_index + next_ch.len_utf8();
                        index += 1;
                    } else {
                        break;
                    }
                }
                let raw = &input[start..end];
                tokens.push(PolicyConditionToken::Number(parse_policy_number(raw)?));
            }
            _ if is_policy_identifier_start(ch) => {
                let start = byte_index;
                let mut end = byte_index + ch.len_utf8();
                index += 1;
                while index < chars.len() {
                    let (next_index, next_ch) = chars[index];
                    if is_policy_identifier_continue(next_ch) {
                        end = next_index + next_ch.len_utf8();
                        index += 1;
                    } else {
                        break;
                    }
                }
                let identifier = input[start..end].to_string();
                match identifier.to_ascii_lowercase().as_str() {
                    "and" => tokens.push(PolicyConditionToken::And),
                    "or" => tokens.push(PolicyConditionToken::Or),
                    "not" => tokens.push(PolicyConditionToken::Not),
                    _ => {
                        validate_policy_identifier(&identifier)?;
                        tokens.push(PolicyConditionToken::Identifier(identifier));
                    }
                }
            }
            _ => anyhow::bail!("unsupported condition expression character: {ch}"),
        }
    }
    Ok(tokens)
}

fn parse_policy_number(raw: &str) -> Result<f64> {
    if raw.chars().any(|ch| ch.is_ascii_alphabetic()) {
        return Ok(parse_byte_size(raw)? as f64);
    }
    let value = raw
        .parse::<f64>()
        .with_context(|| format!("number literal {raw} is invalid"))?;
    anyhow::ensure!(value.is_finite(), "number literal must be finite");
    Ok(value)
}

fn policy_identifier_value(
    identifier: &str,
    traffic: Option<&TrafficAccountingRecord>,
    rollup: Option<&TelemetryRollupView>,
    incomplete: &mut Vec<String>,
) -> Option<f64> {
    if identifier.starts_with("traffic.") {
        let Some(traffic) = traffic else {
            push_incomplete(incomplete, "traffic accounting missing");
            return None;
        };
        if traffic.state != "ok" {
            for reason in &traffic.incomplete_reasons {
                push_incomplete(incomplete, reason);
            }
            if traffic.incomplete_reasons.is_empty() {
                push_incomplete(
                    incomplete,
                    format!("traffic accounting state is {}", traffic.state),
                );
            }
            return None;
        }
    }
    let value = match identifier {
        "traffic.quota.total" => traffic
            .and_then(|traffic| traffic.quota_total_bytes)
            .map(|value| value as f64),
        "traffic.quota.rx" => traffic
            .and_then(|traffic| traffic.quota_rx_bytes)
            .map(|value| value as f64),
        "traffic.quota.tx" => traffic
            .and_then(|traffic| traffic.quota_tx_bytes)
            .map(|value| value as f64),
        "traffic.cycle.total" => traffic.map(|traffic| traffic.total_bytes as f64),
        "traffic.cycle.rx" => traffic.map(|traffic| traffic.rx_bytes as f64),
        "traffic.cycle.tx" => traffic.map(|traffic| traffic.tx_bytes as f64),
        "traffic.cycle_percent" => traffic.and_then(|traffic| traffic.cycle_percent),
        "cpu.load_1" => rollup.map(|rollup| rollup.cpu_load_1_max),
        "cpu.load_saturation" => rollup.map(|rollup| rollup.cpu_load_1_max),
        "memory.available_ratio" => rollup.and_then(|rollup| {
            (rollup.memory_total_bytes_max > 0).then(|| {
                rollup.memory_available_bytes_min as f64 / rollup.memory_total_bytes_max as f64
            })
        }),
        "disk.available_ratio" => rollup.and_then(|rollup| {
            (rollup.disk_total_bytes_max > 0).then(|| {
                rollup.disk_available_bytes_min as f64 / rollup.disk_total_bytes_max as f64
            })
        }),
        _ => None,
    };
    if value.is_none() {
        push_incomplete(incomplete, format!("{identifier} missing"));
    }
    value
}

fn validate_policy_identifier(identifier: &str) -> Result<()> {
    anyhow::ensure!(
        matches!(
            identifier,
            "traffic.quota.total"
                | "traffic.quota.rx"
                | "traffic.quota.tx"
                | "traffic.cycle.total"
                | "traffic.cycle.rx"
                | "traffic.cycle.tx"
                | "traffic.cycle_percent"
                | "cpu.load_1"
                | "cpu.load_saturation"
                | "memory.available_ratio"
                | "disk.available_ratio"
        ),
        "unsupported condition variable: {identifier}"
    );
    Ok(())
}

fn is_policy_identifier_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_policy_identifier_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_')
}

fn compare_policy_values(left: f64, right: f64, operator: PolicyComparisonOperator) -> bool {
    match operator {
        PolicyComparisonOperator::Eq => (left - right).abs() < f64::EPSILON,
        PolicyComparisonOperator::NotEq => (left - right).abs() >= f64::EPSILON,
        PolicyComparisonOperator::Lt => left < right,
        PolicyComparisonOperator::Lte => left <= right,
        PolicyComparisonOperator::Gt => left > right,
        PolicyComparisonOperator::Gte => left >= right,
    }
}

fn condition_node_uses_traffic(node: &PolicyConditionNode) -> bool {
    match node {
        PolicyConditionNode::Not(inner) => condition_node_uses_traffic(inner),
        PolicyConditionNode::And(left, right) | PolicyConditionNode::Or(left, right) => {
            condition_node_uses_traffic(left) || condition_node_uses_traffic(right)
        }
        PolicyConditionNode::Comparison { left, right, .. } => {
            numeric_node_uses_traffic(left) || numeric_node_uses_traffic(right)
        }
    }
}

fn numeric_node_uses_traffic(node: &PolicyNumericNode) -> bool {
    match node {
        PolicyNumericNode::Identifier(identifier) => identifier.starts_with("traffic."),
        PolicyNumericNode::Number(_) => false,
        PolicyNumericNode::Unary { operand, .. } => numeric_node_uses_traffic(operand),
        PolicyNumericNode::Binary { left, right, .. } => {
            numeric_node_uses_traffic(left) || numeric_node_uses_traffic(right)
        }
    }
}

fn push_incomplete(reasons: &mut Vec<String>, reason: impl AsRef<str>) {
    let reason = reason.as_ref();
    if !reasons.iter().any(|stored| stored == reason) {
        reasons.push(reason.to_string());
    }
}

fn latest_rollups(rollups: Vec<TelemetryRollupView>) -> HashMap<String, TelemetryRollupView> {
    let mut latest = HashMap::new();
    for rollup in rollups {
        let replace = latest
            .get(&rollup.client_id)
            .map(|stored: &TelemetryRollupView| rollup.bucket_start > stored.bucket_start)
            .unwrap_or(true);
        if replace {
            latest.insert(rollup.client_id.clone(), rollup);
        }
    }
    latest
}

fn preview_hash(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    let digest = Sha256::digest(bytes);
    format!("sha256:{}", hex::encode(digest))
}

fn selector_hash(selectors: &[String]) -> String {
    let digest = Sha256::digest(selectors.join(",").as_bytes());
    hex::encode(&digest[..16])
}

fn vps_rule_from_row(row: sqlx::postgres::PgRow) -> Result<VpsRuleValueRecord> {
    let key: String = row.try_get("key")?;
    let raw: String = row.try_get("value_raw")?;
    let parsed = parse_persisted_vps_rule_value(&key, &raw)?;
    Ok(VpsRuleValueRecord {
        client_id: row.try_get("client_id")?,
        key,
        value_raw: parsed.raw,
        value_json: row
            .try_get::<SqlJson<Value>, _>("value_json")
            .map(|value| value.0)
            .unwrap_or(parsed.json),
        parsed_display: parsed.display,
        state: "ok".to_string(),
        validation_errors: Vec::new(),
        source_kind: row.try_get("source_kind")?,
        source_id: row.try_get("source_id")?,
        updated_by: row.try_get("updated_by")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn policy_group_from_row(
    row: sqlx::postgres::PgRow,
    rules: Vec<PolicyRuleRecord>,
) -> Result<PolicyGroupRecord> {
    Ok(PolicyGroupRecord {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        enabled: row.try_get("enabled")?,
        selector_expression: row.try_get("selector_expression")?,
        notes: row.try_get("notes")?,
        matched_vps_count: 0,
        rule_count: rules.len() as i64,
        enabled_rule_count: rules.iter().filter(|rule| rule.enabled).count() as i64,
        active_warning_count: 0,
        active_critical_count: 0,
        incomplete_vps_count: 0,
        last_evaluated_at: None,
        rules,
        created_by: row.try_get("created_by")?,
        updated_by: row.try_get("updated_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn policy_rule_from_row(row: sqlx::postgres::PgRow) -> Result<PolicyRuleRecord> {
    Ok(PolicyRuleRecord {
        id: row.try_get("id")?,
        group_id: row.try_get("group_id")?,
        rule_version: row.try_get("rule_version")?,
        sort_order: row.try_get("sort_order")?,
        name: row.try_get("name")?,
        enabled: row.try_get("enabled")?,
        traffic_selector: row.try_get("traffic_selector")?,
        condition_expression: row.try_get("condition_expression")?,
        window_secs: row.try_get("window_secs")?,
        severity: row.try_get("severity")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn policy_rule_state_from_row(row: sqlx::postgres::PgRow) -> Result<PolicyRuleStateRecord> {
    Ok(PolicyRuleStateRecord {
        policy_rule_id: row.try_get("policy_rule_id")?,
        client_id: row.try_get("client_id")?,
        rule_version: row.try_get("rule_version")?,
        condition_true: row.try_get("condition_true")?,
        previous_condition_true: row.try_get("previous_condition_true")?,
        window_satisfied: row.try_get("window_satisfied")?,
        first_true_at: row.try_get("first_true_at")?,
        last_true_at: row.try_get("last_true_at")?,
        last_false_at: row.try_get("last_false_at")?,
        last_evaluated_at: row.try_get("last_evaluated_at")?,
        incomplete: row.try_get("incomplete")?,
        incomplete_reasons: row.try_get("incomplete_reasons")?,
        last_actual_value: row.try_get("last_actual_value")?,
        last_threshold_value: row.try_get("last_threshold_value")?,
        last_fired_at: row.try_get("last_fired_at")?,
        trigger_generation: row.try_get("trigger_generation")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn policy_alert_from_row(row: sqlx::postgres::PgRow) -> Result<PolicyAlertRecord> {
    Ok(PolicyAlertRecord {
        id: row.try_get("id")?,
        policy_group_id: row.try_get("policy_group_id")?,
        policy_rule_id: row.try_get("policy_rule_id")?,
        client_id: row.try_get("client_id")?,
        trigger_generation: row.try_get("trigger_generation")?,
        severity: row.try_get("severity")?,
        category: row.try_get("category")?,
        title: row.try_get("title")?,
        detail: row.try_get("detail")?,
        actual_value: row.try_get("actual_value")?,
        threshold_value: row.try_get("threshold_value")?,
        payload: row.try_get::<SqlJson<Value>, _>("payload")?.0,
        observed_at: row.try_get("observed_at")?,
        created_at: row.try_get("created_at")?,
    })
}

fn policy_group_audit(
    action: &str,
    policy: &PolicyGroupRecord,
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: action.to_string(),
        target: format!("fleet_alert_policy:{}", policy.id),
        command_hash: None,
        metadata: policy_group_metadata(policy, operator),
        created_at,
    }
}

fn policy_group_metadata(policy: &PolicyGroupRecord, operator: &AuthContext) -> Value {
    json!({
        "operator": operator.operator.username,
        "policy": policy,
    })
}

fn vps_rules_audit(
    action: &str,
    preview: &VpsRulesDryRunResponse,
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: action.to_string(),
        target: "vps_rules".to_string(),
        command_hash: None,
        metadata: json!({
            "operator": operator.operator.username,
            "preview_hash": preview.preview_hash,
            "matched_vps_count": preview.matched_vps_count,
            "changed_row_count": preview.changed_row_count,
        }),
        created_at,
    }
}

pub(crate) fn policy_alert_to_fleet_alert(alert: &PolicyAlertRecord) -> FleetAlertView {
    FleetAlertView {
        id: format!("policy-alert:{}", alert.id),
        severity: alert.severity.clone(),
        category: alert.category.clone(),
        target_kind: "policy_rule".to_string(),
        target_id: alert.policy_rule_id.to_string(),
        client_id: Some(alert.client_id.clone()),
        title: alert.title.clone(),
        detail: alert.detail.clone(),
        status: "open".to_string(),
        evidence: alert.payload.clone(),
        observed_at: alert.observed_at.clone(),
        operator_state: "open".to_string(),
        muted_until_unix: None,
        escalation_level: 0,
        state_reason: None,
        state_actor_id: None,
        state_updated_at: None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::{
        aggregate_memory_traffic_counter_usage, claim_traffic_selector_directions,
        derive_cycle_usage, next_policy_rule_state, parse_byte_size,
        parse_persisted_traffic_selector_list, parse_traffic_selector, parse_traffic_selector_list,
        policy_identifier_value, policy_state_is_alert_eligible, policy_webhook_repair_is_recent,
        traffic_accounting_for_client, PolicyEvaluation, PolicyRuleRecord, PolicyRuleStateRecord,
        TrafficCounterSampleRecord, TrafficCounterStreamUsage, TrafficStreamRequest,
        VpsRuleValueRecord, VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL, VPS_RULE_KEY_TRAFFIC_RESET_DAY,
        VPS_RULE_KEY_TRAFFIC_SELECTORS,
    };

    #[test]
    fn traffic_stream_aggregation_preserves_usage_across_resets_and_epochs() {
        let samples = vec![
            sample(90, 100, 200, 0),
            sample(105, 110, 220, 0),
            sample(110, 130, 260, 0),
            sample(120, 10, 300, 0),
            sample(125, 20, 320, 0),
            sample(130, 30, 340, 0),
            sample(140, 50, 60, 1),
            sample(145, 60, 70, 1),
            sample(150, 70, 80, 1),
        ];
        let full_usage = derive_cycle_usage(&samples, 100, 150);
        let aggregated = aggregate_memory_traffic_counter_usage(
            &samples,
            &[TrafficStreamRequest {
                client_id: "edge-a".to_string(),
                source_kind: "host".to_string(),
                interface: "eth0".to_string(),
                cycle_start_unix: 100,
            }],
            150,
        );
        assert_eq!(full_usage.cycle_rx, 70);
        assert_eq!(full_usage.cycle_tx, 160);
        assert_eq!(aggregated.len(), 1);
        assert_eq!(aggregated[0].cycle_rx, full_usage.cycle_rx);
        assert_eq!(aggregated[0].cycle_tx, full_usage.cycle_tx);
        assert_eq!(aggregated[0].latest_rx, full_usage.latest_rx);
        assert_eq!(aggregated[0].latest_tx, full_usage.latest_tx);
        assert_eq!(aggregated[0].last_sample_unix, 150);
        assert_eq!(aggregated[0].counter_epochs_seen, 2);
    }

    #[test]
    fn traffic_selectors_reject_overlapping_directions() {
        let error = parse_traffic_selector_list("eth0,eth0+rx").unwrap_err();
        assert!(error
            .to_string()
            .contains("traffic_selector_direction_overlap"));

        let selectors = parse_traffic_selector_list("eth0+rx,eth0+tx").unwrap();
        assert_eq!(selectors.len(), 2);

        assert!(parse_traffic_selector(&format!("{}+rx", "x".repeat(129))).is_err());
        assert!(parse_traffic_selector("eth0\0+rx").is_err());
    }

    #[test]
    fn policy_regression_persisted_traffic_selectors_accept_legacy_direction_overlap() {
        let selectors = parse_persisted_traffic_selector_list("eth0,eth0+rx").unwrap();
        assert_eq!(selectors.len(), 2);
        assert!(parse_traffic_selector_list("eth0,eth0+rx").is_err());
        assert_eq!(
            parse_byte_size("wat").unwrap_err().to_string(),
            "byte_size_number_invalid"
        );
        assert_eq!(
            parse_byte_size("1..2GB").unwrap_err().to_string(),
            "byte_size_number_invalid"
        );
    }

    #[test]
    fn traffic_direction_accounting_is_defensively_idempotent() {
        let rx = parse_traffic_selector("eth0+rx").unwrap();
        let total = parse_traffic_selector("eth0").unwrap();
        let tx = parse_traffic_selector("eth0+tx").unwrap();
        let mut counted = HashMap::new();

        assert_eq!(
            claim_traffic_selector_directions(&mut counted, &rx),
            (true, false)
        );
        assert_eq!(
            claim_traffic_selector_directions(&mut counted, &total),
            (false, true)
        );
        assert_eq!(
            claim_traffic_selector_directions(&mut counted, &tx),
            (false, false)
        );
    }

    #[test]
    fn stale_traffic_fails_policy_evaluation_closed() {
        let now = Utc.timestamp_opt(2_000_000_000, 0).single().unwrap();
        let rules = traffic_rules("eth0");
        let accounting = traffic_accounting_for_client(
            "edge-a",
            &rules,
            &[usage("eth0", now.timestamp() - 901)],
            now,
        );

        assert_eq!(accounting.state, "stale");
        assert_eq!(
            accounting.incomplete_reasons,
            vec!["eth0 sample stale".to_string()]
        );
        let mut incomplete = Vec::new();
        assert_eq!(
            policy_identifier_value(
                "traffic.cycle.total",
                Some(&accounting),
                None,
                &mut incomplete,
            ),
            None
        );
        assert_eq!(incomplete, vec!["eth0 sample stale".to_string()]);
    }

    #[test]
    fn mixed_stream_freshness_uses_the_oldest_selected_sample() {
        let now = Utc.timestamp_opt(2_000_000_000, 0).single().unwrap();
        let rules = traffic_rules("eth0,eth1");
        let accounting = traffic_accounting_for_client(
            "edge-a",
            &rules,
            &[
                usage("eth0", now.timestamp() - 10),
                usage("eth1", now.timestamp() - 901),
            ],
            now,
        );

        assert_eq!(accounting.state, "stale");
        assert_eq!(
            accounting
                .last_sample_at
                .as_deref()
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.timestamp()),
            Some(now.timestamp() - 901)
        );
        assert_eq!(
            accounting.incomplete_reasons,
            vec!["eth1 sample stale".to_string()]
        );
    }

    #[test]
    fn policy_state_generation_advances_past_historical_alerts_after_rule_recreation() {
        let now = Utc.timestamp_opt(2_000_000_000, 0).single().unwrap();
        let state = next_policy_rule_state(
            &policy_rule(2),
            "edge-a",
            &true_policy_evaluation(),
            None,
            7,
            now,
        )
        .unwrap();

        assert_eq!(state.trigger_generation, 8);
        assert!(policy_state_is_alert_eligible(&state));
    }

    #[test]
    fn sustained_true_policy_state_remains_eligible_for_outbox_repair_without_refiring() {
        let now = Utc.timestamp_opt(2_000_000_060, 0).single().unwrap();
        let existing = policy_state(7, true, "2033-05-18T03:32:20+00:00");
        let state = next_policy_rule_state(
            &policy_rule(1),
            "edge-a",
            &true_policy_evaluation(),
            Some(&existing),
            7,
            now,
        )
        .unwrap();

        assert_eq!(state.trigger_generation, 7);
        assert!(state.previous_condition_true);
        assert!(policy_state_is_alert_eligible(&state));
    }

    #[test]
    fn webhook_repair_is_bounded_so_retention_does_not_redeliver_old_alerts() {
        let now = Utc.timestamp_opt(2_000_000_000, 0).single().unwrap();
        assert!(policy_webhook_repair_is_recent(
            &Utc.timestamp_opt(1_999_999_940, 0)
                .single()
                .unwrap()
                .to_rfc3339(),
            now,
        ));
        assert!(!policy_webhook_repair_is_recent(
            &Utc.timestamp_opt(1_999_990_000, 0)
                .single()
                .unwrap()
                .to_rfc3339(),
            now,
        ));
    }

    fn policy_rule(rule_version: i32) -> PolicyRuleRecord {
        PolicyRuleRecord {
            id: uuid::Uuid::from_u128(1),
            group_id: uuid::Uuid::from_u128(2),
            rule_version,
            sort_order: 0,
            name: "quota".to_string(),
            enabled: true,
            traffic_selector: None,
            condition_expression: "cpu.load_1 > 1".to_string(),
            window_secs: 0,
            severity: "warning".to_string(),
            created_at: "test".to_string(),
            updated_at: "test".to_string(),
        }
    }

    fn true_policy_evaluation() -> PolicyEvaluation {
        PolicyEvaluation {
            condition_true: true,
            incomplete: false,
            incomplete_reasons: Vec::new(),
            actual_value: Some(2.0),
            threshold_value: Some(1.0),
            category: "resource".to_string(),
            payload: json!({}),
        }
    }

    fn policy_state(
        trigger_generation: i64,
        condition_true: bool,
        evaluated_at: &str,
    ) -> PolicyRuleStateRecord {
        PolicyRuleStateRecord {
            policy_rule_id: uuid::Uuid::from_u128(1),
            client_id: "edge-a".to_string(),
            rule_version: 1,
            condition_true,
            previous_condition_true: false,
            window_satisfied: condition_true,
            first_true_at: condition_true.then(|| evaluated_at.to_string()),
            last_true_at: condition_true.then(|| evaluated_at.to_string()),
            last_false_at: (!condition_true).then(|| evaluated_at.to_string()),
            last_evaluated_at: evaluated_at.to_string(),
            incomplete: false,
            incomplete_reasons: Vec::new(),
            last_actual_value: Some(2.0),
            last_threshold_value: Some(1.0),
            last_fired_at: condition_true.then(|| evaluated_at.to_string()),
            trigger_generation,
            updated_at: evaluated_at.to_string(),
        }
    }

    fn traffic_rules(selectors: &str) -> Vec<VpsRuleValueRecord> {
        vec![
            rule(VPS_RULE_KEY_TRAFFIC_RESET_DAY, "1", json!({"day": 1})),
            rule(
                VPS_RULE_KEY_TRAFFIC_SELECTORS,
                selectors,
                json!({"selectors": []}),
            ),
            rule(
                VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL,
                "1GB",
                json!({"bytes": 1_000_000_000_i64}),
            ),
        ]
    }

    fn rule(key: &str, value_raw: &str, value_json: serde_json::Value) -> VpsRuleValueRecord {
        VpsRuleValueRecord {
            client_id: "edge-a".to_string(),
            key: key.to_string(),
            value_raw: value_raw.to_string(),
            value_json,
            parsed_display: value_raw.to_string(),
            state: "valid".to_string(),
            validation_errors: Vec::new(),
            source_kind: "test".to_string(),
            source_id: None,
            updated_by: None,
            updated_at: "test".to_string(),
        }
    }

    fn usage(interface: &str, last_sample_unix: i64) -> TrafficCounterStreamUsage {
        TrafficCounterStreamUsage {
            client_id: "edge-a".to_string(),
            source_kind: "host".to_string(),
            interface: interface.to_string(),
            cycle_rx: 100,
            cycle_tx: 200,
            latest_rx: 1_000,
            latest_tx: 2_000,
            last_sample_unix,
            counter_epochs_seen: 1,
        }
    }

    fn sample(
        observed_unix: i64,
        rx_bytes: i64,
        tx_bytes: i64,
        counter_epoch: i64,
    ) -> TrafficCounterSampleRecord {
        TrafficCounterSampleRecord {
            client_id: "edge-a".to_string(),
            source_kind: "host".to_string(),
            interface: "eth0".to_string(),
            observed_at: observed_unix.to_string(),
            observed_unix,
            rx_bytes,
            tx_bytes,
            counter_epoch,
            sample_source: "test".to_string(),
        }
    }
}
