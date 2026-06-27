use anyhow::{Context, Result};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use std::collections::HashSet;
use uuid::Uuid;
use vpsman_common::{
    default_webhook_message, expression_matches, expression_referenced_events,
    expression_referenced_roots, is_webhook_rule_delivery_process_status, payload_hash,
    render_template_with_limit, WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED,
    WEBHOOK_RULE_DELIVERY_STATUS_FAILED, WEBHOOK_RULE_DELIVERY_STATUS_PERMANENTLY_FAILED,
    WEBHOOK_RULE_DELIVERY_STATUS_QUEUED,
};
use vpsman_server_core::operator_is_active_authorized;

use crate::{
    model::{AgentView, AuthContext},
    model_webhook_rules::{
        WebhookEventCandidate, WebhookRuleDeliveryCandidate, WebhookRuleDeliveryView,
        WebhookRuleDispatchRequest, WebhookRuleDryRunRequest, WebhookRuleDryRunView,
        WebhookRuleProcessRequest, WebhookRuleView,
    },
    repository_webhook_rules::dry_run_webhook_delivery,
    selector_expression::{agent_expression_context, parse_selector_expression},
    state::AppState,
    unix_now,
};

const WEBHOOK_PROCESS_DRY_RUN_STATUS: &str = "delivery_dry_run";
const WEBHOOK_DELIVERY_LEASE_SECS: i64 = 300;
const MAX_WEBHOOK_DELIVERY_ATTEMPTS: i32 = 4;
const WEBHOOK_RETRY_BACKOFF_SECS: [i64; 3] = [60, 300, 1800];
const WEBHOOK_SIGNATURE_HEADER: &str = "X-Vpsman-Webhook-Signature";
const WEBHOOK_DELIVERY_HEADER: &str = "X-Vpsman-Webhook-Delivery";
const WEBHOOK_EVENT_HEADER: &str = "X-Vpsman-Webhook-Event";

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
        let candidate = webhook_candidate_for_rule(
            &rule,
            request.event_kind.trim(),
            &event_id,
            self.repo.list_agents().await?,
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
        let event_id = request
            .event_id
            .clone()
            .unwrap_or_else(|| format!("{event_kind}:{}", unix_now()));
        let rule_limit = if request.rule_id.is_some() {
            1000
        } else {
            request.limit.unwrap_or(100).clamp(1, 1000)
        };
        let rules = self.repo.list_webhook_rules(rule_limit, Some(true)).await?;
        let agents = self.repo.list_agents().await?;
        let mut candidates = Vec::new();
        let mut matched_rule_id = request.rule_id.is_none();
        for rule in rules {
            if request
                .rule_id
                .is_some_and(|requested_rule_id| rule.id != requested_rule_id)
            {
                continue;
            }
            matched_rule_id = true;
            if let Some(candidate) = webhook_candidate_for_rule(
                &rule,
                event_kind,
                &event_id,
                agents.clone(),
                Some(operator.operator.id),
            )? {
                candidates.push(candidate);
            }
        }
        anyhow::ensure!(
            matched_rule_id,
            "webhook_rule_not_found_or_disabled:{}",
            request
                .rule_id
                .map(|rule_id| rule_id.to_string())
                .unwrap_or_default()
        );
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
        self.repo
            .record_webhook_event(WebhookEventCandidate {
                kind: event_kind.to_string(),
                event_id: event_id.clone(),
                event_predicates: vec![event_kind.to_string()],
                subject_client_ids: Vec::new(),
                payload: json!({
                    "event": {
                        "kind": event_kind,
                        "id": event_id,
                        "source": "manual_dispatch",
                    }
                }),
                actor_id: Some(operator.operator.id),
            })
            .await?;
        Ok(candidates
            .iter()
            .map(|candidate| {
                let mut delivery = dry_run_webhook_delivery(candidate);
                delivery.status = "event_logged".to_string();
                delivery.review_preview_hash = Some(preview_hash.clone());
                delivery
            })
            .collect())
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
        let delivery_ids = deliveries
            .iter()
            .map(|delivery| delivery.id)
            .collect::<Vec<_>>();
        let expected_ids = delivery_ids.iter().copied().collect::<HashSet<_>>();
        let lease_id = Uuid::new_v4();
        let claimed_deliveries = self
            .repo
            .claim_webhook_rule_deliveries_for_process(
                &delivery_ids,
                lease_id,
                WEBHOOK_DELIVERY_LEASE_SECS,
            )
            .await?;
        let claimed_ids = claimed_deliveries
            .iter()
            .map(|delivery| delivery.id)
            .collect::<HashSet<_>>();
        anyhow::ensure!(
            claimed_ids == expected_ids,
            "webhook_rule_process_claim_mismatch"
        );
        let client = reqwest::Client::builder()
            .timeout(tokio::time::Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("failed to build webhook rule client")?;
        let mut processed = Vec::new();
        for delivery in claimed_deliveries {
            if !self.repo.webhook_rule_enabled(delivery.rule_id).await? {
                processed.push(
                    self.repo
                        .cancel_claimed_webhook_rule_delivery(
                            delivery.id,
                            lease_id,
                            "webhook rule disabled",
                        )
                        .await?,
                );
                continue;
            }
            let result = if self
                .webhook_delivery_actor_authorized(delivery.actor_id)
                .await?
            {
                deliver_webhook_rule(&client, &delivery).await
            } else {
                Err(anyhow::anyhow!("actor_authority_revoked"))
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
                        Some(error.to_string()),
                    )
                }
            };
            let next_attempt_after_secs = if status == WEBHOOK_RULE_DELIVERY_STATUS_FAILED {
                webhook_next_retry_after_secs(delivery.attempt_count)
            } else {
                None
            };
            processed.push(
                self.repo
                    .complete_webhook_rule_delivery_attempt(
                        delivery.id,
                        lease_id,
                        status,
                        error.as_deref(),
                        next_attempt_after_secs,
                    )
                    .await?,
            );
        }
        if !dry_run && !processed.is_empty() {
            self.repo
                .record_webhook_rule_process_audit(&processed, operator)
                .await?;
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
        "version": 1,
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
                "payload": candidate.payload,
                "matched_vps": candidate.matched_vps,
                "message": candidate.message,
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

pub(crate) fn webhook_candidate_for_rule(
    rule: &WebhookRuleView,
    event_kind: &str,
    event_id: &str,
    agents: Vec<AgentView>,
    actor_id: Option<Uuid>,
) -> Result<Option<WebhookRuleDeliveryCandidate>> {
    webhook_candidate_for_event(
        rule,
        event_kind,
        event_id,
        &[event_kind.to_string()],
        &Value::Null,
        agents,
        actor_id,
    )
}

pub(crate) fn webhook_candidate_for_event(
    rule: &WebhookRuleView,
    event_kind: &str,
    event_id: &str,
    event_predicates: &[String],
    event_payload: &Value,
    agents: Vec<AgentView>,
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
            let mut context = agent_expression_context(agent).with_event_predicate(event_kind);
            for predicate in event_predicates {
                context = context.with_event_predicate(predicate);
            }
            for root in ["server", "job", "schedule", "alert", "telemetry", "event"] {
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
        signing_secret: rule.signing_secret.clone(),
        cooldown_until_unix: (unix_now() as i64).saturating_add(rule.cooldown_secs),
        actor_id,
    }))
}

pub(crate) async fn deliver_webhook_rule(
    client: &reqwest::Client,
    delivery: &WebhookRuleDeliveryView,
) -> Result<()> {
    crate::repository_webhook_rules::validate_webhook_rule_target(&delivery.target)?;
    let body = serde_json::to_vec(&delivery.payload).context("failed to encode webhook payload")?;
    let mut request = client
        .post(delivery.target.trim())
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
    Ok(())
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
    for root in ["server", "job", "schedule", "alert", "telemetry"] {
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
mod tests {
    use super::*;
    use vpsman_common::AgentCapabilitySnapshot;

    fn agent(id: &str, tags: &[&str]) -> AgentView {
        AgentView {
            id: id.to_string(),
            display_name: id.to_string(),
            status: "online".to_string(),
            tags: tags.iter().map(|tag| tag.to_string()).collect(),
            registration_ip: None,
            last_ip: None,
            last_seen_at: None,
            arch: None,
            internal_build_number: 1,
            process_incarnation_id: None,
            stale_since: None,
            stale_reason: None,
            capabilities: AgentCapabilitySnapshot::default(),
        }
    }

    #[test]
    fn webhook_candidate_aggregates_matched_vps_and_renders_template() {
        let rule = WebhookRuleView {
            id: Uuid::nil(),
            name: "edge-online".to_string(),
            enabled: true,
            expression: "interval.30sec && tag:edge".to_string(),
            target: "https://hooks.example/vpsman".to_string(),
            body_template: "{rule.name} {event.kind} {vps.id}".to_string(),
            cooldown_secs: 30,
            signing_secret: Some("secret".to_string()),
            signing_secret_set: true,
            notes: None,
            actor_id: None,
            created_at: "0".to_string(),
            updated_at: "0".to_string(),
        };
        let candidate = webhook_candidate_for_rule(
            &rule,
            "interval.30sec",
            "interval.30sec:1",
            vec![agent("edge-a", &["edge"]), agent("core-a", &["core"])],
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(candidate.matched_vps.len(), 1);
        assert_eq!(candidate.message, "edge-online interval.30sec edge-a");
        assert_eq!(candidate.signing_secret.as_deref(), Some("secret"));
    }

    #[test]
    fn webhook_signature_uses_payload_bytes() {
        let signature = webhook_signature("secret", br#"{"hello":"world"}"#).unwrap();
        assert_eq!(
            signature,
            "sha256=2677ad3e7c090b2fa2c0fb13020d66d5420879b8316eb356a2d60fb9073bc778"
        );
    }
}
