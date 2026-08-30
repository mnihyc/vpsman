use anyhow::{Context, Result};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use std::collections::HashMap;
use uuid::Uuid;
use vpsman_common::{
    default_webhook_message, expression_matches, expression_referenced_events,
    expression_referenced_roots, is_webhook_rule_delivery_process_status, payload_hash,
    render_template_with_limit, VpsRuleContext, WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED,
    WEBHOOK_RULE_DELIVERY_STATUS_FAILED, WEBHOOK_RULE_DELIVERY_STATUS_PERMANENTLY_FAILED,
    WEBHOOK_RULE_DELIVERY_STATUS_QUEUED,
};
use vpsman_server_core::operator_is_active_authorized;

use crate::{
    model::{AgentView, AuthContext},
    model_webhook_rules::{
        WebhookRuleDeliveryCandidate, WebhookRuleDeliveryView, WebhookRuleDispatchRequest,
        WebhookRuleDryRunRequest, WebhookRuleDryRunView, WebhookRuleProcessRequest,
        WebhookRuleView,
    },
    repository_webhook_rules::dry_run_webhook_delivery,
    security::{operator_has_scope, SCOPE_CONFIG_READ},
    selector_expression::{agent_expression_context, parse_selector_expression},
    state::AppState,
    unix_now,
};

const WEBHOOK_PROCESS_DRY_RUN_STATUS: &str = "delivery_dry_run";
const WEBHOOK_PROCESS_OUTCOME_SKIPPED_CURRENT_OWNER: &str = "skipped_current_owner";
const WEBHOOK_DELIVERY_TIMEOUT_SECS: i64 = 5;
const WEBHOOK_DELIVERY_LEASE_MARGIN_SECS: i64 = 60;
const MAX_WEBHOOK_ERROR_BYTES: usize = 1024;
const MAX_WEBHOOK_DELIVERY_ATTEMPTS: i32 = 4;
const WEBHOOK_RETRY_BACKOFF_SECS: [i64; 3] = [60, 300, 1800];
const WEBHOOK_SIGNATURE_HEADER: &str = "X-Vpsman-Webhook-Signature";
const WEBHOOK_DELIVERY_HEADER: &str = "X-Vpsman-Webhook-Delivery";
const WEBHOOK_EVENT_HEADER: &str = "X-Vpsman-Webhook-Event";
const EVENT_EXPRESSION_ROOTS: [&str; 9] = [
    "server",
    "job",
    "schedule",
    "alert",
    "telemetry",
    "event",
    "policy",
    "policy_rule",
    "traffic",
];
const EVENT_DELIVERY_ROOTS: [&str; 8] = [
    "server",
    "job",
    "schedule",
    "alert",
    "telemetry",
    "policy",
    "policy_rule",
    "traffic",
];

type HmacSha256 = Hmac<Sha256>;

impl AppState {
    pub(crate) async fn dry_run_webhook_rule(
        &self,
        request: &WebhookRuleDryRunRequest,
        operator: &AuthContext,
    ) -> Result<WebhookRuleDryRunView> {
        let now = unix_now().to_string();
        let rule = WebhookRuleView {
            id: Uuid::nil(),
            name: optional_trimmed(&request.name).unwrap_or_else(|| "dry-run".to_string()),
            enabled: request.enabled.unwrap_or(true),
            expression: request.expression.trim().to_string(),
            target: optional_trimmed(&request.target)
                .unwrap_or_else(|| "https://dry-run.invalid/webhook".to_string()),
            body_template: request.body_template.trim().to_string(),
            cooldown_secs: request.cooldown_secs.unwrap_or(0),
            signing_secret: None,
            signing_secret_set: false,
            notes: optional_trimmed(&request.notes),
            actor_id: Some(operator.operator.id),
            created_at: now.clone(),
            updated_at: now,
        };
        let event_id = request
            .event_id
            .clone()
            .unwrap_or_else(|| format!("{}:{}", request.event_kind.trim(), unix_now()));
        let agents = self.repo.list_agents().await?;
        let expression = parse_selector_expression(&rule.expression)
            .map_err(|error| anyhow::anyhow!("invalid webhook rule expression: {error}"))?
            .context("webhook rule expression is empty")?;
        let rules_by_client = if vpsman_common::expression_references_vps_rules(&expression) {
            self.repo.vps_rule_contexts_for_agents(&agents).await?
        } else {
            HashMap::new()
        };
        let candidate = webhook_candidate_for_rule_with_vps_rules(
            &rule,
            request.event_kind.trim(),
            &event_id,
            agents,
            &rules_by_client,
            Some(operator.operator.id),
        )?;
        let Some(candidate) = candidate else {
            return Ok(WebhookRuleDryRunView {
                rendered_message: String::new(),
                matched_vps: Vec::new(),
                payload_context: empty_payload_context(&rule, request.event_kind.trim(), &event_id),
                validation_errors: Vec::new(),
                delivery: None,
            });
        };
        Ok(WebhookRuleDryRunView {
            rendered_message: candidate.message.clone(),
            matched_vps: candidate.matched_vps.clone(),
            payload_context: candidate.payload.clone(),
            validation_errors: Vec::new(),
            delivery: Some(dry_run_webhook_delivery(&candidate)),
        })
    }

    pub(crate) async fn dispatch_webhook_rules(
        &self,
        request: &WebhookRuleDispatchRequest,
        operator: &AuthContext,
    ) -> Result<Vec<WebhookRuleDeliveryView>> {
        let dry_run = request.dry_run.unwrap_or(false);
        anyhow::ensure!(
            dry_run || request.confirmed,
            "webhook_rule_dispatch_confirmation_required"
        );
        let event_kind = request.event_kind.trim();
        let event_id = match request
            .event_id
            .as_deref()
            .map(str::trim)
            .filter(|event_id| !event_id.is_empty())
        {
            Some(event_id) => event_id.to_string(),
            None if dry_run => format!("{event_kind}:{}", unix_now()),
            None => anyhow::bail!("webhook_rule_dispatch_event_id_required"),
        };
        let rules = if let Some(rule_id) = request.rule_id {
            vec![self
                .repo
                .webhook_rule_by_id(rule_id)
                .await?
                .with_context(|| format!("webhook_rule_not_found:{rule_id}"))?]
        } else {
            self.repo
                .list_webhook_rules(request.limit.unwrap_or(100).clamp(1, 1000), Some(true))
                .await?
        };
        let agents = self.repo.list_agents().await?;
        let uses_vps_rules = rules.iter().any(|rule| {
            parse_selector_expression(&rule.expression)
                .ok()
                .flatten()
                .is_some_and(|expression| {
                    vpsman_common::expression_references_vps_rules(&expression)
                })
        });
        anyhow::ensure!(
            !uses_vps_rules || operator_has_scope(&operator.operator.scopes, SCOPE_CONFIG_READ),
            "vps_rule_selector_scope_required"
        );
        let rules_by_client = if uses_vps_rules {
            self.repo.vps_rule_contexts_for_agents(&agents).await?
        } else {
            HashMap::new()
        };
        let mut candidates = Vec::new();
        for rule in rules {
            if let Some(candidate) = webhook_candidate_for_rule_with_vps_rules(
                &rule,
                event_kind,
                &event_id,
                agents.clone(),
                &rules_by_client,
                Some(operator.operator.id),
            )? {
                candidates.push(candidate);
            }
        }
        let preview_hash =
            webhook_dispatch_preview_hash(request, event_kind, &event_id, &candidates)?;
        if dry_run {
            return Ok(candidates
                .iter()
                .map(|candidate| {
                    let mut delivery = dry_run_webhook_delivery(candidate);
                    delivery.review_preview_hash = Some(preview_hash.clone());
                    delivery
                })
                .collect::<Vec<_>>());
        }
        anyhow::ensure!(
            request.preview_hash.as_deref() == Some(preview_hash.as_str()),
            "webhook_rule_dispatch_preview_hash_mismatch"
        );
        self.repo.record_webhook_rule_deliveries(&candidates).await
    }

    pub(crate) async fn process_webhook_rule_deliveries(
        &self,
        request: &WebhookRuleProcessRequest,
        operator: &AuthContext,
    ) -> Result<Vec<WebhookRuleDeliveryView>> {
        let dry_run = request.dry_run.unwrap_or(false);
        anyhow::ensure!(
            dry_run || request.confirmed,
            "webhook_rule_delivery_process_confirmation_required"
        );
        let status = request
            .status
            .as_deref()
            .unwrap_or(WEBHOOK_RULE_DELIVERY_STATUS_QUEUED);
        anyhow::ensure!(
            is_webhook_rule_delivery_process_status(status),
            "webhook rule delivery process status must be queued or failed"
        );
        let deliveries = self
            .repo
            .list_webhook_rule_deliveries(
                request.limit.unwrap_or(50).clamp(1, 200),
                None,
                None,
                Some(status),
            )
            .await?;
        let preview_hash = webhook_process_preview_hash(request, &deliveries)?;
        if !dry_run {
            anyhow::ensure!(
                request.preview_hash.as_deref() == Some(preview_hash.as_str()),
                "webhook_rule_process_preview_hash_mismatch"
            );
        }
        if dry_run {
            return Ok(deliveries
                .into_iter()
                .map(|mut delivery| {
                    delivery.status = WEBHOOK_PROCESS_DRY_RUN_STATUS.to_string();
                    delivery.review_preview_hash = Some(preview_hash.clone());
                    delivery
                })
                .collect());
        }
        let lease_secs = delivery_lease_secs();
        let mut processed = Vec::new();
        for requested_delivery in deliveries {
            let delivery_id = requested_delivery.id;
            let lease_id = Uuid::new_v4();
            let Some(delivery) = self
                .repo
                .claim_webhook_rule_delivery_for_process(delivery_id, lease_id, lease_secs)
                .await?
            else {
                // The automatic oldest-first consumer owns this exact row (or
                // it already left the reviewed status). Keep durable state
                // untouched and make the request-local skip explicit.
                let mut skipped = requested_delivery;
                skipped.process_outcome =
                    Some(WEBHOOK_PROCESS_OUTCOME_SKIPPED_CURRENT_OWNER.to_string());
                processed.push(skipped);
                continue;
            };
            anyhow::ensure!(
                delivery.id == delivery_id,
                "webhook_rule_process_claim_mismatch"
            );
            if !self.repo.webhook_rule_enabled(delivery.rule_id).await? {
                let canceled = self
                    .repo
                    .cancel_claimed_webhook_rule_delivery(
                        delivery.id,
                        lease_id,
                        "webhook rule disabled",
                    )
                    .await?;
                self.repo
                    .record_webhook_rule_process_audit(std::slice::from_ref(&canceled), operator)
                    .await?;
                processed.push(canceled);
                continue;
            }
            let actor_authorized = self
                .webhook_delivery_actor_authorized(delivery.actor_id)
                .await?;
            let (result, eligibility_revision) = if actor_authorized {
                let send_eligibility = self
                    .repo
                    .begin_webhook_rule_alert_send(delivery.id, lease_id)
                    .await?;
                if !send_eligibility.is_deliverable() {
                    let cancellation_reason = send_eligibility.cancellation_reason();
                    let canceled = if let Some(reason) = cancellation_reason {
                        Some(
                            self.repo
                                .cancel_claimed_webhook_rule_delivery(delivery.id, lease_id, reason)
                                .await,
                        )
                    } else {
                        None
                    };
                    if let Some(canceled) = canceled {
                        let canceled = canceled?;
                        self.repo
                            .record_webhook_rule_process_audit(
                                std::slice::from_ref(&canceled),
                                operator,
                            )
                            .await?;
                        processed.push(canceled);
                    } else {
                        let mut skipped = delivery;
                        skipped.process_outcome =
                            Some(WEBHOOK_PROCESS_OUTCOME_SKIPPED_CURRENT_OWNER.to_string());
                        processed.push(skipped);
                    }
                    continue;
                }
                (
                    deliver_webhook_rule(&delivery).await,
                    send_eligibility.revision(),
                )
            } else {
                (Err(anyhow::anyhow!("actor_authority_revoked")), None)
            };
            let (status, error) = match result {
                Ok(()) => (WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED, None),
                Err(error) if error.to_string() == "actor_authority_revoked" => (
                    WEBHOOK_RULE_DELIVERY_STATUS_PERMANENTLY_FAILED,
                    Some("actor_authority_revoked".to_string()),
                ),
                Err(error) => {
                    let next_attempt_after_secs =
                        webhook_next_retry_after_secs(delivery.attempt_count);
                    (
                        if next_attempt_after_secs.is_some() {
                            WEBHOOK_RULE_DELIVERY_STATUS_FAILED
                        } else {
                            WEBHOOK_RULE_DELIVERY_STATUS_PERMANENTLY_FAILED
                        },
                        Some(format_delivery_error(&error)),
                    )
                }
            };
            let next_attempt_after_secs = if status == WEBHOOK_RULE_DELIVERY_STATUS_FAILED {
                webhook_next_retry_after_secs(delivery.attempt_count)
            } else {
                None
            };
            let completion = self
                .repo
                .complete_webhook_rule_delivery_attempt(
                    delivery.id,
                    lease_id,
                    status,
                    error.as_deref(),
                    next_attempt_after_secs,
                    eligibility_revision,
                )
                .await?;
            self.repo
                .record_webhook_rule_process_audit(std::slice::from_ref(&completion), operator)
                .await?;
            processed.push(completion);
        }
        Ok(processed)
    }

    async fn webhook_delivery_actor_authorized(&self, actor_id: Option<Uuid>) -> Result<bool> {
        let Some(actor_id) = actor_id.filter(|id| !id.is_nil()) else {
            return Ok(false);
        };
        let Some(operator) = self.repo.operator_by_id(actor_id).await? else {
            return Ok(false);
        };
        Ok(operator_is_active_authorized(
            &operator.status,
            &operator.role,
            &operator.scopes,
            "operator",
            &["integrations:write"],
        ))
    }
}

fn optional_trimmed(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn webhook_dispatch_preview_hash(
    request: &WebhookRuleDispatchRequest,
    event_kind: &str,
    event_id: &str,
    candidates: &[WebhookRuleDeliveryCandidate],
) -> Result<String> {
    let payload = serde_json::to_vec(&json!({
        "version": 2,
        "kind": "webhook_rule_dispatch",
        "request": {
            "rule_id": request.rule_id,
            "event_kind": event_kind,
            "event_id": event_id,
            "limit": request.limit,
        },
        "candidates": candidates.iter().map(|candidate| {
            json!({
                "rule_id": candidate.rule_id,
                "event_kind": candidate.event_kind,
                "event_id": candidate.event_id,
                "target": candidate.target,
                "dedupe_key": candidate.dedupe_key,
                "rule_revision_hash": candidate.rule_revision_hash,
                "matched_vps": candidate.matched_vps.iter().map(|agent| json!({
                    "id": agent.id,
                    "display_name": agent.display_name,
                    "status": agent.status,
                    "tags": agent.tags,
                })).collect::<Vec<_>>(),
            })
        }).collect::<Vec<_>>(),
    }))?;
    Ok(payload_hash(&payload))
}

fn webhook_process_preview_hash(
    request: &WebhookRuleProcessRequest,
    deliveries: &[WebhookRuleDeliveryView],
) -> Result<String> {
    let payload = serde_json::to_vec(&json!({
        "version": 1,
        "kind": "webhook_rule_process",
        "request": {
            "limit": request.limit,
            "status": request.status,
        },
        "deliveries": deliveries.iter().map(|delivery| {
            json!({
                "id": delivery.id,
                "rule_id": delivery.rule_id,
                "event_kind": delivery.event_kind,
                "event_id": delivery.event_id,
                "status": delivery.status,
                "target": delivery.target,
                "dedupe_key": delivery.dedupe_key,
                "attempt_count": delivery.attempt_count,
            })
        }).collect::<Vec<_>>(),
    }))?;
    Ok(payload_hash(&payload))
}

#[cfg(test)]
pub(crate) fn webhook_candidate_for_rule(
    rule: &WebhookRuleView,
    event_kind: &str,
    event_id: &str,
    agents: Vec<AgentView>,
    actor_id: Option<Uuid>,
) -> Result<Option<WebhookRuleDeliveryCandidate>> {
    webhook_candidate_for_rule_with_vps_rules(
        rule,
        event_kind,
        event_id,
        agents,
        &HashMap::new(),
        actor_id,
    )
}

fn webhook_candidate_for_rule_with_vps_rules(
    rule: &WebhookRuleView,
    event_kind: &str,
    event_id: &str,
    agents: Vec<AgentView>,
    rules_by_client: &HashMap<String, VpsRuleContext>,
    actor_id: Option<Uuid>,
) -> Result<Option<WebhookRuleDeliveryCandidate>> {
    webhook_candidate_for_event_with_vps_rules(
        rule,
        event_kind,
        event_id,
        &[event_kind.to_string()],
        &Value::Null,
        agents,
        rules_by_client,
        actor_id,
    )
}

#[cfg(test)]
pub(crate) fn webhook_candidate_for_event(
    rule: &WebhookRuleView,
    event_kind: &str,
    event_id: &str,
    event_predicates: &[String],
    event_payload: &Value,
    agents: Vec<AgentView>,
    actor_id: Option<Uuid>,
) -> Result<Option<WebhookRuleDeliveryCandidate>> {
    webhook_candidate_for_event_with_vps_rules(
        rule,
        event_kind,
        event_id,
        event_predicates,
        event_payload,
        agents,
        &HashMap::new(),
        actor_id,
    )
}

fn webhook_candidate_for_event_with_vps_rules(
    rule: &WebhookRuleView,
    event_kind: &str,
    event_id: &str,
    event_predicates: &[String],
    event_payload: &Value,
    agents: Vec<AgentView>,
    rules_by_client: &HashMap<String, VpsRuleContext>,
    actor_id: Option<Uuid>,
) -> Result<Option<WebhookRuleDeliveryCandidate>> {
    let expression = parse_selector_expression(&rule.expression)
        .map_err(|error| anyhow::anyhow!("invalid webhook rule expression: {error}"))?
        .context("webhook rule expression is empty")?;
    let event_kind = event_kind.trim();
    let event_id = event_id.trim();
    anyhow::ensure!(!event_kind.is_empty(), "webhook event kind is required");
    anyhow::ensure!(!event_id.is_empty(), "webhook event id is required");
    let matched_vps = agents
        .into_iter()
        .filter(|agent| {
            let mut context = agent_expression_context(agent);
            if let Some(rules) = rules_by_client.get(&agent.id) {
                context = context.with_vps_rules(rules.clone());
            }
            context = context.with_event_predicate(event_kind);
            for predicate in event_predicates {
                context = context.with_event_predicate(predicate);
            }
            for root in EVENT_EXPRESSION_ROOTS {
                if let Some(value) = event_payload.get(root).cloned() {
                    context = context.with_json_root(root, value);
                }
            }
            expression_matches(&context, &expression)
        })
        .collect::<Vec<_>>();
    if matched_vps.is_empty() {
        return Ok(None);
    }
    let referenced_roots = expression_referenced_roots(&expression)
        .into_iter()
        .collect::<Vec<_>>();
    let referenced_events = expression_referenced_events(&expression)
        .into_iter()
        .collect::<Vec<_>>();
    let mut payload = json!({
        "schema": "vpsman.webhook_rule.delivery.v1",
        "rule": {
            "id": rule.id,
            "name": &rule.name,
            "expression": &rule.expression,
            "enabled": rule.enabled,
        },
        "event": {
            "kind": event_kind,
            "id": event_id,
            "predicates": event_predicates,
            "occurred_at_unix": unix_now(),
        },
        "query": {
            "expression": &rule.expression,
            "referenced_roots": referenced_roots,
            "referenced_events": referenced_events,
        },
        "matched_vps": &matched_vps,
    });
    merge_event_payload_roots(&mut payload, event_payload);
    let message = render_message_from_payload(rule, &payload)?;
    if let Some(object) = payload.as_object_mut() {
        object.insert("message".to_string(), Value::String(message.clone()));
    }
    let dedupe_fingerprint = json!({
        "rule_id": rule.id,
        "event_id": event_id,
    });
    let hash = payload_hash(dedupe_fingerprint.to_string().as_bytes());
    let rule_revision_hash = payload_hash(&serde_json::to_vec(&json!({
        "id": rule.id,
        "name": &rule.name,
        "enabled": rule.enabled,
        "expression": &rule.expression,
        "target": &rule.target,
        "body_template": &rule.body_template,
        "cooldown_secs": rule.cooldown_secs,
        "signing_secret": &rule.signing_secret,
    }))?);
    Ok(Some(WebhookRuleDeliveryCandidate {
        rule_id: rule.id,
        rule_name: rule.name.clone(),
        event_kind: event_kind.to_string(),
        event_id: event_id.to_string(),
        target: rule.target.clone(),
        dedupe_key: format!("webhook-rule:{}", &hash[..32]),
        payload,
        matched_vps,
        message,
        rule_revision_hash,
        signing_secret: rule.signing_secret.clone(),
        cooldown_until_unix: (unix_now() as i64).saturating_add(rule.cooldown_secs),
        actor_id,
    }))
}

pub(crate) async fn deliver_webhook_rule(delivery: &WebhookRuleDeliveryView) -> Result<()> {
    let timeout = tokio::time::Duration::from_secs(WEBHOOK_DELIVERY_TIMEOUT_SECS as u64);
    tokio::time::timeout(timeout, async {
        let target = vpsman_server_core::prepare_webhook_target(&delivery.target, timeout).await?;
        let body =
            serde_json::to_vec(&delivery.payload).context("failed to encode webhook payload")?;
        let mut request = target
            .client()
            .post(target.url().clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(WEBHOOK_DELIVERY_HEADER, delivery.id.to_string())
            .header(WEBHOOK_EVENT_HEADER, delivery.event_kind.trim())
            .body(body.clone());
        if let Some(secret) = delivery.signing_secret.as_deref() {
            request = request.header(WEBHOOK_SIGNATURE_HEADER, webhook_signature(secret, &body)?);
        }
        let response = request.send().await.context("webhook request failed")?;
        let status = response.status();
        anyhow::ensure!(
            status.is_success(),
            "webhook returned non-success status {}",
            status.as_u16()
        );
        Ok::<(), anyhow::Error>(())
    })
    .await
    .context("webhook delivery timed out")?
}

fn delivery_lease_secs() -> i64 {
    WEBHOOK_DELIVERY_TIMEOUT_SECS
        .saturating_add(WEBHOOK_DELIVERY_LEASE_MARGIN_SECS)
        .max(WEBHOOK_DELIVERY_LEASE_MARGIN_SECS)
}

fn webhook_signature(secret: &str, body: &[u8]) -> Result<String> {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).context("invalid webhook signing secret")?;
    mac.update(body);
    Ok(format!(
        "sha256={}",
        hex::encode(mac.finalize().into_bytes())
    ))
}

fn webhook_next_retry_after_secs(attempt_count: i32) -> Option<i64> {
    let next_attempt_count = attempt_count.saturating_add(1);
    if next_attempt_count >= MAX_WEBHOOK_DELIVERY_ATTEMPTS {
        return None;
    }
    let index = next_attempt_count.saturating_sub(1) as usize;
    Some(
        WEBHOOK_RETRY_BACKOFF_SECS
            .get(index)
            .copied()
            .unwrap_or_else(|| *WEBHOOK_RETRY_BACKOFF_SECS.last().unwrap_or(&1800)),
    )
}

fn format_delivery_error(error: &anyhow::Error) -> String {
    format!("{error:#}")
        .chars()
        .take(MAX_WEBHOOK_ERROR_BYTES)
        .collect()
}

fn render_message_from_payload(rule: &WebhookRuleView, payload: &Value) -> Result<String> {
    if rule.body_template.trim().is_empty() {
        let matched_vps_count = payload
            .get("matched_vps")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        return Ok(default_webhook_message(&rule.name, matched_vps_count));
    }
    render_template_with_limit(&rule.body_template, payload, 16 * 1024)
        .map_err(|error| anyhow::anyhow!("webhook template render failed: {error}"))
}

fn merge_event_payload_roots(payload: &mut Value, event_payload: &Value) {
    let Some(target) = payload.as_object_mut() else {
        return;
    };
    for root in EVENT_DELIVERY_ROOTS {
        if let Some(value) = event_payload.get(root).cloned() {
            target.insert(root.to_string(), value);
        }
    }
    if let Some(event) = event_payload.get("event").and_then(Value::as_object) {
        if let Some(target_event) = target.get_mut("event").and_then(Value::as_object_mut) {
            for (key, value) in event {
                target_event
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
        }
    }
}

fn empty_payload_context(rule: &WebhookRuleView, event_kind: &str, event_id: &str) -> Value {
    json!({
        "schema": "vpsman.webhook_rule.delivery.v1",
        "rule": {
            "id": rule.id,
            "name": &rule.name,
            "expression": &rule.expression,
            "enabled": rule.enabled,
        },
        "event": {
            "kind": event_kind,
            "id": event_id,
            "predicates": [event_kind],
            "occurred_at_unix": unix_now(),
        },
        "query": {
            "expression": &rule.expression,
            "referenced_roots": [],
            "referenced_events": [],
        },
        "matched_vps": [],
    })
}

#[cfg(test)]
#[path = "tests_webhook_rules.rs"]
mod tests;
