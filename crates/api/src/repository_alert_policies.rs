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
    selector_expression::{agent_matches_selector_expression, parse_selector_expression},
    unix_now,
};

const MAX_POLICY_NAME_BYTES: usize = 128;
const MAX_POLICY_NOTES_BYTES: usize = 1024;
const MAX_RULE_NAME_BYTES: usize = 128;
const MAX_SELECTOR_EXPRESSION_BYTES: usize = 4096;
const MAX_CONDITION_EXPRESSION_BYTES: usize = 4096;
const MAX_VPS_RULE_VALUE_BYTES: usize = 4096;
const MAX_TRAFFIC_SELECTOR_ITEMS: usize = 16;
const TRAFFIC_SAMPLE_STALE_SECS: i64 = 900;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct TrafficSelector {
    source: String,
    interface: String,
    direction: String,
    canonical: String,
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
                ORDER BY client_id ASC, key ASC
                LIMIT $1
                "#,
            )
            .bind(query.limit.unwrap_or(1000).clamp(1, 5000))
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(vps_rule_from_row)
            .collect::<Result<Vec<_>>>()?,
        };
        let agents = self.list_agents().await?;
        let allowed_clients = query
            .selector_expression
            .as_deref()
            .map(|selector| resolve_agents(&agents, selector))
            .transpose()?
            .map(|agents| {
                agents
                    .into_iter()
                    .map(|agent| agent.id)
                    .collect::<HashSet<_>>()
            });
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
        rows.truncate(query.limit.unwrap_or(1000).clamp(1, 5000) as usize);
        Ok(rows)
    }

    pub(crate) async fn effective_vps_rules(
        &self,
        client_id: &str,
    ) -> Result<Vec<VpsRuleValueRecord>> {
        self.list_vps_rules(&VpsRuleQuery {
            limit: Some(100),
            client_id: Some(client_id.to_string()),
            selector_expression: None,
            key: None,
            state: None,
        })
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
        self.evaluate_policy_rules().await?;
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
        self.evaluate_policy_rules().await?;
        Ok(preview)
    }

    pub(crate) async fn list_traffic_accounting(
        &self,
        query: &TrafficAccountingQuery,
    ) -> Result<Vec<TrafficAccountingRecord>> {
        let agents = self.list_agents().await?;
        let selected_agents = if let Some(selector) = query.selector_expression.as_deref() {
            resolve_agents(&agents, selector)?
        } else {
            agents
        };
        let allowed_clients = selected_agents
            .iter()
            .map(|agent| agent.id.clone())
            .collect::<HashSet<_>>();
        let rules = self
            .list_vps_rules(&VpsRuleQuery {
                limit: Some(5000),
                client_id: None,
                selector_expression: None,
                key: None,
                state: None,
            })
            .await?;
        let samples = self.list_traffic_counter_samples().await?;
        let now = Utc::now();
        let mut records = Vec::new();
        for agent in selected_agents {
            if query
                .client_id
                .as_deref()
                .is_some_and(|client_id| client_id != agent.id)
            {
                continue;
            }
            if !allowed_clients.contains(&agent.id) {
                continue;
            }
            let record = traffic_accounting_for_client(&agent.id, &rules, &samples, now);
            if query
                .state
                .as_deref()
                .is_none_or(|state| record.state == state)
            {
                records.push(record);
            }
        }
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
        )?;
        let agents = self.list_agents().await?;
        let matched = resolve_agents(&agents, &request.selector_expression)?;
        let mut validation_errors = Vec::new();
        let traffic = self
            .list_traffic_accounting(&TrafficAccountingQuery {
                selector_expression: Some(request.selector_expression.clone()),
                client_id: None,
                state: None,
                limit: Some(5000),
            })
            .await?;
        let rollups = latest_rollups(self.list_telemetry_rollups(5000, None, None).await?);
        let traffic_by_client = traffic
            .iter()
            .map(|record| (record.client_id.clone(), record))
            .collect::<HashMap<_, _>>();
        let rules = self
            .list_vps_rules(&VpsRuleQuery {
                limit: Some(5000),
                client_id: None,
                selector_expression: None,
                key: None,
                state: None,
            })
            .await?;
        let samples = self.list_traffic_counter_samples().await?;
        let now = Utc::now();
        let mut rule_previews = Vec::new();
        let mut incomplete_clients = BTreeSet::new();
        for rule in &request.rules {
            match validate_policy_rule_request(rule) {
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
                    traffic_override_for_rule(&agent.id, rule, &rules, &samples, now);
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
                .bind(limit.clamp(1, 1000))
                .bind(enabled)
                .fetch_all(pool)
                .await?;
                let mut groups = Vec::new();
                for row in rows {
                    groups.push(self.policy_group_from_row(row).await?);
                }
                groups
            }
        };
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

    pub(crate) async fn get_fleet_alert_policy(&self, id: Uuid) -> Result<PolicyGroupRecord> {
        self.list_fleet_alert_policies(1000, None, None, None)
            .await?
            .into_iter()
            .find(|group| group.id == id)
            .context("fleet_alert_policy_not_found")
    }

    pub(crate) async fn upsert_fleet_alert_policy(
        &self,
        request: &CreateFleetAlertPolicyRequest,
        operator: &AuthContext,
    ) -> Result<PolicyGroupRecord> {
        validate_policy_group_request(request, true)?;
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
        let group_id = request.id.unwrap_or_else(Uuid::new_v4);
        let existing_group = self.get_fleet_alert_policy(group_id).await.ok();
        let mut rules = Vec::new();
        for (index, rule) in request.rules.iter().enumerate() {
            rules.push(policy_rule_from_request(
                group_id,
                rule,
                index as i32,
                &now,
                existing_group.as_ref(),
            ));
        }
        let group = PolicyGroupRecord {
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
            created_by: Some(operator.operator.id),
            updated_by: Some(operator.operator.id),
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        match self {
            Self::Memory(memory) => {
                let mut groups = memory.policy_groups.write().await;
                groups.retain(|stored| stored.id != group.id && stored.name != group.name);
                groups.push(group.clone());
                drop(groups);
                memory.audits.write().await.push(policy_group_audit(
                    "fleet.alert_policy_upserted",
                    &group,
                    operator,
                    now,
                ));
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
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
                sqlx::query("DELETE FROM policy_rules WHERE group_id = $1")
                    .bind(group.id)
                    .execute(&mut *tx)
                    .await?;
                for rule in &group.rules {
                    sqlx::query(
                        r#"
                        INSERT INTO policy_rules (
                            id, group_id, rule_version, sort_order, name, enabled,
                            traffic_selector, condition_expression, window_secs, severity
                        )
                        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
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
            }
        }
        self.evaluate_policy_rules().await?;
        self.get_fleet_alert_policy(group.id).await
    }

    pub(crate) async fn delete_fleet_alert_policy(
        &self,
        policy_id: Uuid,
        operator: &AuthContext,
    ) -> Result<()> {
        let policy = self.get_fleet_alert_policy(policy_id).await?;
        match self {
            Self::Memory(memory) => {
                memory
                    .policy_groups
                    .write()
                    .await
                    .retain(|stored| stored.id != policy_id);
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
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query("DELETE FROM policy_groups WHERE id = $1")
                    .bind(policy_id)
                    .execute(&mut *tx)
                    .await?;
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
        let _ = self.evaluate_policy_rules().await;
        let mut alerts = match self {
            Self::Memory(memory) => memory.policy_alerts.read().await.clone(),
            Self::Postgres(pool) => sqlx::query(
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
                ORDER BY observed_at DESC, id DESC
                LIMIT $1
                "#,
            )
            .bind(query.limit.unwrap_or(200).clamp(1, 1000))
            .bind(query.client_id.as_deref())
            .bind(query.severity.as_deref())
            .bind(query.category.as_deref())
            .bind(query.policy_group_id)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(policy_alert_from_row)
            .collect::<Result<Vec<_>>>()?,
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
        });
        alerts.sort_by(|left, right| right.observed_at.cmp(&left.observed_at));
        alerts.truncate(query.limit.unwrap_or(200).clamp(1, 1000) as usize);
        Ok(alerts)
    }

    pub(crate) async fn evaluate_policy_rules(&self) -> Result<usize> {
        let groups = self
            .list_fleet_alert_policies(1000, Some(true), None, None)
            .await?;
        if groups.is_empty() {
            return Ok(0);
        }
        let agents = self.list_agents().await?;
        let traffic = self
            .list_traffic_accounting(&TrafficAccountingQuery {
                selector_expression: None,
                client_id: None,
                state: None,
                limit: Some(5000),
            })
            .await?;
        let traffic_by_client = traffic
            .iter()
            .map(|record| (record.client_id.clone(), record))
            .collect::<HashMap<_, _>>();
        let rules = self
            .list_vps_rules(&VpsRuleQuery {
                limit: Some(5000),
                client_id: None,
                selector_expression: None,
                key: None,
                state: None,
            })
            .await?;
        let samples = self.list_traffic_counter_samples().await?;
        let rollups = latest_rollups(self.list_telemetry_rollups(5000, None, None).await?);
        let now = Utc::now();
        let now_text = now.to_rfc3339();
        let mut fired = 0_usize;
        for group in groups {
            let matched = resolve_agents(&agents, &group.selector_expression)?;
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
                        traffic_override_for_rule(&agent.id, &request, &rules, &samples, now);
                    let traffic_record = override_traffic
                        .as_ref()
                        .or_else(|| traffic_by_client.get(&agent.id).copied());
                    let evaluation =
                        evaluate_rule_for_client(&request, traffic_record, rollups.get(&agent.id));
                    let (state, should_fire) = self
                        .upsert_policy_rule_state(&group, rule, agent, evaluation.clone(), now)
                        .await?;
                    if should_fire
                        && self
                            .insert_policy_alert(
                                &group,
                                rule,
                                agent,
                                &state,
                                &evaluation,
                                &now_text,
                            )
                            .await?
                    {
                        self.mark_policy_rule_state_fired(
                            rule.id,
                            &agent.id,
                            rule.rule_version,
                            &now_text,
                        )
                        .await?;
                        fired += 1;
                    }
                }
            }
        }
        Ok(fired)
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
            .list_vps_rules(&VpsRuleQuery {
                limit: Some(5000),
                client_id: None,
                selector_expression: None,
                key: None,
                state: None,
            })
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

    async fn list_traffic_counter_samples(&self) -> Result<Vec<TrafficCounterSampleRecord>> {
        match self {
            Self::Memory(memory) => Ok(memory.traffic_counter_samples.read().await.clone()),
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        client_id,
                        source_kind,
                        interface,
                        observed_at::text AS observed_at,
                        EXTRACT(EPOCH FROM observed_at)::bigint AS observed_unix,
                        rx_bytes,
                        tx_bytes,
                        counter_epoch,
                        sample_source
                    FROM traffic_counter_samples
                    ORDER BY observed_at ASC, client_id ASC, source_kind ASC, interface ASC
                    LIMIT 200000
                    "#,
                )
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(TrafficCounterSampleRecord {
                            client_id: row.try_get("client_id")?,
                            source_kind: row.try_get("source_kind")?,
                            interface: row.try_get("interface")?,
                            observed_at: row.try_get("observed_at")?,
                            observed_unix: row.try_get("observed_unix")?,
                            rx_bytes: row.try_get("rx_bytes")?,
                            tx_bytes: row.try_get("tx_bytes")?,
                            counter_epoch: row.try_get("counter_epoch")?,
                            sample_source: row.try_get("sample_source")?,
                        })
                    })
                    .collect()
            }
        }
    }

    async fn policy_group_from_row(&self, row: sqlx::postgres::PgRow) -> Result<PolicyGroupRecord> {
        let group_id: Uuid = row.try_get("id")?;
        let rules = self.policy_rules_for_group(group_id).await?;
        Ok(PolicyGroupRecord {
            id: group_id,
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

    async fn policy_rules_for_group(&self, group_id: Uuid) -> Result<Vec<PolicyRuleRecord>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .policy_groups
                .read()
                .await
                .iter()
                .find(|group| group.id == group_id)
                .map(|group| group.rules.clone())
                .unwrap_or_default()),
            Self::Postgres(pool) => {
                let rows = sqlx::query(
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
                .bind(group_id)
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(policy_rule_from_row).collect()
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

    async fn upsert_policy_rule_state(
        &self,
        group: &PolicyGroupRecord,
        rule: &PolicyRuleRecord,
        agent: &AgentView,
        evaluation: PolicyEvaluation,
        now: DateTime<Utc>,
    ) -> Result<(PolicyRuleStateRecord, bool)> {
        let now_text = now.to_rfc3339();
        let existing = self
            .policy_rule_state(rule.id, &agent.id, rule.rule_version)
            .await?;
        let previous_condition_true = existing
            .as_ref()
            .map(|state| state.condition_true)
            .unwrap_or(false);
        let previous_window_satisfied = existing
            .as_ref()
            .map(|state| state.window_satisfied)
            .unwrap_or(false);
        let mut first_true_at = existing
            .as_ref()
            .and_then(|state| state.first_true_at.clone());
        let mut last_false_at = existing
            .as_ref()
            .and_then(|state| state.last_false_at.clone());
        let mut last_true_at = existing
            .as_ref()
            .and_then(|state| state.last_true_at.clone());
        let mut trigger_generation = existing
            .as_ref()
            .map(|state| state.trigger_generation)
            .unwrap_or(0);
        if evaluation.condition_true && !previous_condition_true {
            first_true_at = Some(now_text.clone());
            trigger_generation += 1;
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
        let state = PolicyRuleStateRecord {
            policy_rule_id: rule.id,
            client_id: agent.id.clone(),
            rule_version: rule.rule_version,
            condition_true: evaluation.condition_true,
            previous_condition_true,
            window_satisfied,
            first_true_at,
            last_true_at,
            last_false_at,
            last_evaluated_at: now_text.clone(),
            incomplete: evaluation.incomplete,
            incomplete_reasons: evaluation.incomplete_reasons,
            last_actual_value: evaluation.actual_value,
            last_threshold_value: evaluation.threshold_value,
            last_fired_at: existing
                .as_ref()
                .and_then(|state| state.last_fired_at.clone()),
            trigger_generation,
            updated_at: now_text,
        };
        match self {
            Self::Memory(memory) => {
                let mut states = memory.policy_rule_states.write().await;
                states.retain(|stored| {
                    !(stored.policy_rule_id == state.policy_rule_id
                        && stored.client_id == state.client_id
                        && stored.rule_version == state.rule_version)
                });
                states.push(state.clone());
            }
            Self::Postgres(pool) => {
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
                .execute(pool)
                .await?;
            }
        }
        let should_fire = state.condition_true
            && state.window_satisfied
            && !state.incomplete
            && !previous_window_satisfied;
        let _ = group;
        Ok((state, should_fire))
    }

    async fn policy_rule_state(
        &self,
        rule_id: Uuid,
        client_id: &str,
        rule_version: i32,
    ) -> Result<Option<PolicyRuleStateRecord>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .policy_rule_states
                .read()
                .await
                .iter()
                .find(|state| {
                    state.policy_rule_id == rule_id
                        && state.client_id == client_id
                        && state.rule_version == rule_version
                })
                .cloned()),
            Self::Postgres(pool) => {
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
                        updated_at::text AS updated_at
                    FROM policy_rule_states
                    WHERE policy_rule_id = $1 AND client_id = $2 AND rule_version = $3
                    "#,
                )
                .bind(rule_id)
                .bind(client_id)
                .bind(rule_version)
                .fetch_optional(pool)
                .await?;
                row.map(policy_rule_state_from_row).transpose()
            }
        }
    }

    async fn insert_policy_alert(
        &self,
        group: &PolicyGroupRecord,
        rule: &PolicyRuleRecord,
        agent: &AgentView,
        state: &PolicyRuleStateRecord,
        evaluation: &PolicyEvaluation,
        now_text: &str,
    ) -> Result<bool> {
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
                    "condition_expression": rule.condition_expression,
                    "traffic_selector": rule.traffic_selector,
                    "window_secs": rule.window_secs,
                }),
            );
        }
        let alert = PolicyAlertRecord {
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
            payload: payload.clone(),
            observed_at: now_text.to_string(),
            created_at: now_text.to_string(),
        };
        let inserted = match self {
            Self::Memory(memory) => {
                let mut alerts = memory.policy_alerts.write().await;
                if alerts.iter().any(|stored| {
                    stored.policy_rule_id == alert.policy_rule_id
                        && stored.client_id == alert.client_id
                        && stored.trigger_generation == alert.trigger_generation
                }) {
                    return Ok(false);
                }
                alerts.push(alert.clone());
                true
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO policy_alerts (
                        id, policy_group_id, policy_rule_id, client_id, trigger_generation,
                        severity, category, title, detail, actual_value, threshold_value,
                        payload, observed_at
                    )
                    VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13::timestamptz)
                    ON CONFLICT (policy_rule_id, client_id, trigger_generation) DO NOTHING
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
                .bind(SqlJson(alert.payload.clone()))
                .bind(&alert.observed_at)
                .execute(pool)
                .await?
                .rows_affected()
                    > 0
            }
        };
        if !inserted {
            return Ok(false);
        }
        self.record_webhook_event(WebhookEventCandidate {
            kind: "alert.policy_reached".to_string(),
            event_id: format!("policy-alert:{}", alert.id),
            event_predicates: vec![
                "alert.policy_reached".to_string(),
                "alert.open".to_string(),
                format!("alert.category:{}", alert.category),
                format!("alert.severity:{}", alert.severity),
            ],
            subject_client_ids: vec![alert.client_id.clone()],
            payload,
            actor_id: None,
        })
        .await?;
        Ok(true)
    }

    async fn mark_policy_rule_state_fired(
        &self,
        rule_id: Uuid,
        client_id: &str,
        rule_version: i32,
        fired_at: &str,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                if let Some(state) =
                    memory
                        .policy_rule_states
                        .write()
                        .await
                        .iter_mut()
                        .find(|state| {
                            state.policy_rule_id == rule_id
                                && state.client_id == client_id
                                && state.rule_version == rule_version
                        })
                {
                    state.last_fired_at = Some(fired_at.to_string());
                    state.updated_at = fired_at.to_string();
                }
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    UPDATE policy_rule_states
                    SET last_fired_at = $4::timestamptz, updated_at = now()
                    WHERE policy_rule_id = $1 AND client_id = $2 AND rule_version = $3
                    "#,
                )
                .bind(rule_id)
                .bind(client_id)
                .bind(rule_version)
                .bind(fired_at)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }
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
            let selectors = parse_traffic_selector_list(raw)?;
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
    let raw = input.trim();
    anyhow::ensure!(!raw.is_empty(), "traffic_selector_empty");
    let mut selectors = Vec::new();
    let mut seen = BTreeSet::new();
    for item in raw.split(',') {
        let selector = parse_traffic_selector(item)?;
        anyhow::ensure!(
            seen.insert(selector.canonical.clone()),
            "traffic_selector_duplicate"
        );
        selectors.push(selector);
    }
    anyhow::ensure!(
        selectors.len() <= MAX_TRAFFIC_SELECTOR_ITEMS,
        "traffic_selector_too_many_items"
    );
    Ok(selectors)
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
        !interface
            .chars()
            .any(|ch| ch == ',' || ch == '+' || ch == ':' || ch.is_whitespace()),
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
        .context("byte size number is invalid")?;
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

fn traffic_accounting_for_client(
    client_id: &str,
    rules: &[VpsRuleValueRecord],
    samples: &[TrafficCounterSampleRecord],
    now: DateTime<Utc>,
) -> TrafficAccountingRecord {
    traffic_accounting_for_client_with_selector_override(client_id, rules, samples, now, None)
}

fn traffic_override_for_rule(
    client_id: &str,
    rule: &PolicyRuleRequest,
    rules: &[VpsRuleValueRecord],
    samples: &[TrafficCounterSampleRecord],
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
        samples,
        now,
        Some(selector),
    ))
}

fn traffic_accounting_for_client_with_selector_override(
    client_id: &str,
    rules: &[VpsRuleValueRecord],
    samples: &[TrafficCounterSampleRecord],
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
    let selectors = selector_override
        .and_then(|selector| parse_traffic_selector_list(selector).ok())
        .or_else(|| {
            rule_map
                .get(VPS_RULE_KEY_TRAFFIC_SELECTORS)
                .and_then(|rule| parse_traffic_selector_list(&rule.value_raw).ok())
        })
        .unwrap_or_default();
    if selectors.is_empty() {
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
    let mut last_sample_unix = None;
    let mut counter_epochs_seen = HashSet::new();
    let mut breakdown = Vec::new();
    for selector in &selectors {
        let selected_samples = samples
            .iter()
            .filter(|sample| {
                sample.client_id == client_id
                    && sample.source_kind == selector.source
                    && sample.interface == selector.interface
                    && sample.observed_unix <= now.timestamp()
            })
            .cloned()
            .collect::<Vec<_>>();
        if selected_samples.is_empty() {
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
        }
        for sample in &selected_samples {
            counter_epochs_seen.insert((
                sample.source_kind.clone(),
                sample.interface.clone(),
                sample.counter_epoch,
            ));
        }
        let usage = derive_cycle_usage(&selected_samples, cycle_start.timestamp(), now.timestamp());
        last_sample_unix = last_sample_unix.max(usage.last_sample_unix);
        let sample_age = usage.last_sample_unix.map(|unix| now.timestamp() - unix);
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
        rx_bytes += selected_cycle_rx;
        tx_bytes += selected_cycle_tx;
        latest_rx += selected_latest_rx;
        latest_tx += selected_latest_tx;
        let mut row_state = "ok".to_string();
        let mut row_reasons = Vec::new();
        if sample_age.is_some_and(|age| age > TRAFFIC_SAMPLE_STALE_SECS) {
            row_state = "stale".to_string();
            row_reasons.push("stale sample".to_string());
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
    let state = if incomplete_reasons.is_empty() {
        if last_sample_unix.is_none() {
            "unknown"
        } else {
            "ok"
        }
    } else {
        "incomplete"
    };
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
        counter_epochs_seen: i64::try_from(counter_epochs_seen.len()).unwrap_or(i64::MAX),
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
                let rx_delta = if sample.rx_bytes >= prev.rx_bytes {
                    sample.rx_bytes - prev.rx_bytes
                } else {
                    0
                };
                let tx_delta = if sample.tx_bytes >= prev.tx_bytes {
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

fn validate_policy_group_request(
    request: &CreateFleetAlertPolicyRequest,
    require_confirmed: bool,
) -> Result<()> {
    if require_confirmed {
        anyhow::ensure!(
            request.confirmed,
            "fleet_alert_policy_confirmation_required"
        );
    }
    validate_name(
        &request.name,
        MAX_POLICY_NAME_BYTES,
        "fleet alert policy name",
    )?;
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
    for rule in &request.rules {
        validate_policy_rule_request(rule)?;
    }
    Ok(())
}

fn validate_policy_rule_request(rule: &PolicyRuleRequest) -> Result<()> {
    validate_name(
        &rule.name,
        MAX_RULE_NAME_BYTES,
        "fleet alert policy rule name",
    )?;
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
    let existing_rule = request.id.and_then(|id| {
        existing_group.and_then(|group| group.rules.iter().find(|rule| rule.id == id))
    });
    let rule_version = existing_rule
        .map(|existing| {
            if policy_rule_material_matches(existing, request, sort_order) {
                existing.rule_version
            } else {
                existing.rule_version.saturating_add(1)
            }
        })
        .unwrap_or(1);
    PolicyRuleRecord {
        id: request.id.unwrap_or_else(Uuid::new_v4),
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
        created_at: now.to_string(),
        updated_at: now.to_string(),
    }
}

fn policy_rule_material_matches(
    existing: &PolicyRuleRecord,
    request: &PolicyRuleRequest,
    sort_order: i32,
) -> bool {
    existing.sort_order == sort_order
        && existing.name == request.name.trim()
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
        if traffic.state == "incomplete" {
            for reason in &traffic.incomplete_reasons {
                push_incomplete(incomplete, reason);
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
    let parsed = parse_vps_rule_value(&key, &raw)?;
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
