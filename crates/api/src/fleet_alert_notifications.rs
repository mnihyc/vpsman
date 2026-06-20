use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use reqwest::{redirect::Policy, Url};
use serde_json::json;
use tokio::time::Duration;
use uuid::Uuid;
use vpsman_common::{
    is_fleet_alert_notification_delivery_process_status, payload_hash,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_DELIVERED,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_FAILED,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_MATCHED_DRY_RUN,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_PERMANENTLY_FAILED,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_QUEUED,
};
use vpsman_server_core::operator_is_active_authorized;

use crate::{
    fleet_alerts::{build_agent_alert_scopes, AgentAlertScope},
    model::{AuthContext, FleetAlertQuery, FleetAlertView},
    model_alert_notifications::{
        FleetAlertNotificationCandidate, FleetAlertNotificationChannelView,
        FleetAlertNotificationDeliveryView, FleetAlertNotificationDispatchRequest,
        FleetAlertNotificationProcessRequest,
    },
    repository_alert_notifications::notification_status_for_kind,
    state::AppState,
    unix_now,
};

const NOTIFICATION_WEBHOOK_TIMEOUT_SECS: u64 = 5;
const NOTIFICATION_PROCESS_DRY_RUN_STATUS: &str = "delivery_dry_run";
const NOTIFICATION_DELIVERY_LEASE_SECS: i64 = 300;
const MAX_NOTIFICATION_DELIVERY_ATTEMPTS: i32 = 4;
const NOTIFICATION_RETRY_BACKOFF_SECS: [i64; 3] = [60, 300, 1800];

impl AppState {
    pub(crate) async fn dispatch_fleet_alert_notifications(
        &self,
        request: &FleetAlertNotificationDispatchRequest,
        operator: &AuthContext,
    ) -> Result<Vec<FleetAlertNotificationDeliveryView>> {
        let dry_run = request.dry_run.unwrap_or(false);
        anyhow::ensure!(
            dry_run || request.confirmed,
            "fleet_alert_notification_dispatch_confirmation_required"
        );
        let alerts = self
            .list_fleet_alerts(FleetAlertQuery {
                limit: request.limit.or(Some(200)),
                client_id: request.client_id.clone(),
                severity: request.severity.clone(),
                category: request.category.clone(),
                operator_state: request.operator_state.clone(),
                include_muted: request.include_muted,
            })
            .await?;
        let channels = self
            .repo
            .list_fleet_alert_notification_channels(1000, Some(true), None, None, None)
            .await?;
        let agents = self.repo.list_agents().await?;
        let agent_scopes = build_agent_alert_scopes(&agents);
        let candidates = notification_candidates(&alerts, &channels, &agent_scopes);
        let preview_hash = notification_dispatch_preview_hash(request, &candidates)?;
        if dry_run {
            return Ok(candidates
                .iter()
                .map(|candidate| {
                    let mut delivery = dry_run_delivery(candidate, operator);
                    delivery.review_preview_hash = Some(preview_hash.clone());
                    delivery
                })
                .collect());
        }
        anyhow::ensure!(
            request.preview_hash.as_deref() == Some(preview_hash.as_str()),
            "fleet_alert_notification_dispatch_preview_hash_mismatch"
        );
        self.repo
            .record_fleet_alert_notification_deliveries(&candidates, operator)
            .await
    }

    pub(crate) async fn process_fleet_alert_notifications(
        &self,
        request: &FleetAlertNotificationProcessRequest,
        operator: &AuthContext,
    ) -> Result<Vec<FleetAlertNotificationDeliveryView>> {
        let dry_run = request.dry_run.unwrap_or(false);
        anyhow::ensure!(
            dry_run || request.confirmed,
            "fleet_alert_notification_process_confirmation_required"
        );
        let status = request
            .status
            .as_deref()
            .unwrap_or(FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_QUEUED);
        anyhow::ensure!(
            is_fleet_alert_notification_delivery_process_status(status),
            "fleet alert notification process status must be queued or failed"
        );
        let delivery_kind_filter = request
            .delivery_kind
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(delivery_kind) = delivery_kind_filter {
            anyhow::ensure!(
                delivery_kind == "webhook",
                "fleet alert notification delivery kind is invalid"
            );
        }
        let limit = request.limit.unwrap_or(50).clamp(1, 200);
        let deliveries = self
            .repo
            .list_fleet_alert_notification_deliveries(limit, None, None, Some(status))
            .await?;
        let filtered_deliveries = deliveries
            .into_iter()
            .filter(|delivery| {
                delivery_kind_filter.is_none_or(|kind| delivery.delivery_kind == kind)
            })
            .collect::<Vec<_>>();
        let preview_hash = notification_process_preview_hash(request, &filtered_deliveries)?;
        if !dry_run {
            anyhow::ensure!(
                request.preview_hash.as_deref() == Some(preview_hash.as_str()),
                "fleet_alert_notification_process_preview_hash_mismatch"
            );
        }
        if dry_run {
            return Ok(filtered_deliveries
                .iter()
                .map(|delivery| {
                    let mut delivery = dry_run_process_delivery(delivery);
                    delivery.review_preview_hash = Some(preview_hash.clone());
                    delivery
                })
                .collect());
        }
        let delivery_ids = filtered_deliveries
            .iter()
            .map(|delivery| delivery.id)
            .collect::<Vec<_>>();
        let expected_ids = delivery_ids.iter().copied().collect::<HashSet<_>>();
        let lease_id = Uuid::new_v4();
        let claimed_deliveries = self
            .repo
            .claim_fleet_alert_notification_deliveries_for_process(
                &delivery_ids,
                lease_id,
                NOTIFICATION_DELIVERY_LEASE_SECS,
            )
            .await?;
        let claimed_ids = claimed_deliveries
            .iter()
            .map(|delivery| delivery.id)
            .collect::<HashSet<_>>();
        anyhow::ensure!(
            claimed_ids == expected_ids,
            "fleet_alert_notification_process_claim_mismatch"
        );
        let mut processed = Vec::new();
        let client = webhook_client()?;
        for delivery in claimed_deliveries {
            if !self
                .repo
                .fleet_alert_notification_channel_enabled(delivery.channel_id)
                .await?
            {
                processed.push(
                    self.repo
                        .cancel_claimed_fleet_alert_notification_delivery(
                            delivery.id,
                            lease_id,
                            "fleet alert notification channel disabled",
                        )
                        .await?,
                );
                continue;
            }
            let result = if self
                .fleet_alert_delivery_actor_authorized(delivery.actor_id)
                .await?
            {
                deliver_notification(&client, &delivery).await
            } else {
                Err(anyhow::anyhow!("actor_authority_revoked"))
            };
            let (status, error, next_attempt_after_secs) = match result {
                Ok(()) => (
                    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_DELIVERED,
                    None,
                    None,
                ),
                Err(error) if error.to_string() == "actor_authority_revoked" => (
                    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_PERMANENTLY_FAILED,
                    Some("actor_authority_revoked".to_string()),
                    None,
                ),
                Err(error) => {
                    let next_attempt_after_secs =
                        notification_next_retry_after_secs(delivery.attempt_count);
                    (
                        if next_attempt_after_secs.is_some() {
                            FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_FAILED
                        } else {
                            FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_PERMANENTLY_FAILED
                        },
                        Some(error.to_string()),
                        next_attempt_after_secs,
                    )
                }
            };
            processed.push(
                self.repo
                    .complete_fleet_alert_notification_delivery_attempt(
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
                .record_fleet_alert_notification_process_audit(&processed, operator)
                .await?;
        }
        Ok(processed)
    }

    async fn fleet_alert_delivery_actor_authorized(&self, actor_id: Option<Uuid>) -> Result<bool> {
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

fn notification_candidates(
    alerts: &[FleetAlertView],
    channels: &[FleetAlertNotificationChannelView],
    agent_scopes: &HashMap<String, AgentAlertScope>,
) -> Vec<FleetAlertNotificationCandidate> {
    let now = unix_now() as i64;
    let mut candidates = Vec::new();
    for alert in alerts {
        let alert_scope = alert_scope(alert, agent_scopes);
        for channel in channels {
            if !channel_matches_alert(channel, alert, alert_scope) {
                continue;
            }
            let status = notification_status_for_kind(&channel.delivery_kind).to_string();
            let dedupe_key = notification_dedupe_key(channel, alert);
            candidates.push(FleetAlertNotificationCandidate {
                channel_id: channel.id,
                channel_name: channel.name.clone(),
                alert_id: alert.id.clone(),
                alert_severity: alert.severity.clone(),
                alert_category: alert.category.clone(),
                status,
                delivery_kind: channel.delivery_kind.clone(),
                target: channel.target.clone(),
                dedupe_key,
                payload: notification_payload(channel, alert),
                cooldown_until_unix: now.saturating_add(channel.cooldown_secs),
            });
        }
    }
    candidates
}

fn channel_matches_alert(
    channel: &FleetAlertNotificationChannelView,
    alert: &FleetAlertView,
    scope: Option<&AgentAlertScope>,
) -> bool {
    severity_rank(&alert.severity) <= severity_rank(&channel.min_severity)
        && token_filter_matches(&channel.categories, &alert.category)
        && token_filter_matches(&channel.operator_states, &alert.operator_state)
        && scope_matches(channel, alert, scope)
}

fn scope_matches(
    channel: &FleetAlertNotificationChannelView,
    alert: &FleetAlertView,
    scope: Option<&AgentAlertScope>,
) -> bool {
    match (channel.scope_kind.as_str(), channel.scope_value.as_deref()) {
        ("global", _) => true,
        ("client", Some(client_id)) => {
            alert.client_id.as_deref() == Some(client_id)
                || (alert.target_kind == "agent" && alert.target_id == client_id)
        }
        ("provider", Some(provider)) => {
            scope.and_then(|scope| scope.provider.as_deref()) == Some(provider)
        }
        ("tag", Some(tag)) => {
            scope.is_some_and(|scope| scope.tags.iter().any(|stored| stored == tag))
        }
        _ => false,
    }
}

fn alert_scope<'a>(
    alert: &FleetAlertView,
    scopes: &'a HashMap<String, AgentAlertScope>,
) -> Option<&'a AgentAlertScope> {
    alert
        .client_id
        .as_deref()
        .or_else(|| (alert.target_kind == "agent").then_some(alert.target_id.as_str()))
        .and_then(|client_id| scopes.get(client_id))
}

fn token_filter_matches(filter: &[String], value: &str) -> bool {
    filter.is_empty() || filter.iter().any(|candidate| candidate == value)
}

fn severity_rank(severity: &str) -> u8 {
    match severity {
        "critical" => 0,
        "warning" => 1,
        "info" => 2,
        _ => 3,
    }
}

fn notification_dedupe_key(
    channel: &FleetAlertNotificationChannelView,
    alert: &FleetAlertView,
) -> String {
    let fingerprint = json!({
        "channel_id": channel.id,
        "alert_id": &alert.id,
        "status": &alert.status,
        "operator_state": &alert.operator_state,
    });
    let hash = payload_hash(fingerprint.to_string().as_bytes());
    format!("fleet-alert-notification:{}", &hash[..32])
}

fn notification_payload(
    channel: &FleetAlertNotificationChannelView,
    alert: &FleetAlertView,
) -> serde_json::Value {
    json!({
        "schema": "vpsman.fleet_alert.notification.v1",
        "channel": {
            "id": channel.id,
            "name": &channel.name,
            "scope_kind": &channel.scope_kind,
            "scope_value": &channel.scope_value,
            "delivery_kind": &channel.delivery_kind,
            "target": &channel.target,
        },
        "alert": alert,
    })
}

fn dry_run_delivery(
    candidate: &FleetAlertNotificationCandidate,
    operator: &AuthContext,
) -> FleetAlertNotificationDeliveryView {
    FleetAlertNotificationDeliveryView {
        id: Uuid::new_v4(),
        channel_id: candidate.channel_id,
        channel_name: candidate.channel_name.clone(),
        alert_id: candidate.alert_id.clone(),
        alert_severity: candidate.alert_severity.clone(),
        alert_category: candidate.alert_category.clone(),
        status: FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_MATCHED_DRY_RUN.to_string(),
        delivery_kind: candidate.delivery_kind.clone(),
        target: candidate.target.clone(),
        dedupe_key: candidate.dedupe_key.clone(),
        payload: candidate.payload.clone(),
        error: None,
        attempt_count: 0,
        next_attempt_at: None,
        last_attempt_at: None,
        cooldown_until_unix: candidate.cooldown_until_unix,
        actor_id: Some(operator.operator.id),
        created_at: unix_now().to_string(),
        delivered_at: None,
        review_preview_hash: None,
    }
}

fn dry_run_process_delivery(
    delivery: &FleetAlertNotificationDeliveryView,
) -> FleetAlertNotificationDeliveryView {
    let mut delivery = delivery.clone();
    delivery.status = NOTIFICATION_PROCESS_DRY_RUN_STATUS.to_string();
    delivery
}

fn notification_dispatch_preview_hash(
    request: &FleetAlertNotificationDispatchRequest,
    candidates: &[FleetAlertNotificationCandidate],
) -> Result<String> {
    let payload = serde_json::to_vec(&json!({
        "version": 1,
        "kind": "fleet_alert_notification_dispatch",
        "request": {
            "limit": request.limit,
            "client_id": request.client_id,
            "severity": request.severity,
            "category": request.category,
            "operator_state": request.operator_state,
            "include_muted": request.include_muted,
        },
        "candidates": candidates.iter().map(|candidate| {
            json!({
                "channel_id": candidate.channel_id,
                "alert_id": candidate.alert_id,
                "status": candidate.status,
                "delivery_kind": candidate.delivery_kind,
                "target": candidate.target,
                "dedupe_key": candidate.dedupe_key,
                "payload": candidate.payload,
            })
        }).collect::<Vec<_>>(),
    }))?;
    Ok(payload_hash(&payload))
}

fn notification_process_preview_hash(
    request: &FleetAlertNotificationProcessRequest,
    deliveries: &[FleetAlertNotificationDeliveryView],
) -> Result<String> {
    let payload = serde_json::to_vec(&json!({
        "version": 1,
        "kind": "fleet_alert_notification_process",
        "request": {
            "limit": request.limit,
            "status": request.status,
            "delivery_kind": request.delivery_kind,
        },
        "deliveries": deliveries.iter().map(|delivery| {
            json!({
                "id": delivery.id,
                "status": delivery.status,
                "delivery_kind": delivery.delivery_kind,
                "target": delivery.target,
                "dedupe_key": delivery.dedupe_key,
                "attempt_count": delivery.attempt_count,
            })
        }).collect::<Vec<_>>(),
    }))?;
    Ok(payload_hash(&payload))
}

fn webhook_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(NOTIFICATION_WEBHOOK_TIMEOUT_SECS))
        .redirect(Policy::none())
        .build()
        .context("failed to build fleet alert notification webhook client")
}

async fn deliver_notification(
    client: &reqwest::Client,
    delivery: &FleetAlertNotificationDeliveryView,
) -> Result<()> {
    anyhow::ensure!(
        delivery.delivery_kind == "webhook",
        "fleet alert notification delivery kind is invalid"
    );
    deliver_webhook_payload(client, delivery).await
}

fn notification_next_retry_after_secs(attempt_count: i32) -> Option<i64> {
    let next_attempt_count = attempt_count.saturating_add(1);
    if next_attempt_count >= MAX_NOTIFICATION_DELIVERY_ATTEMPTS {
        return None;
    }
    let index = next_attempt_count.saturating_sub(1) as usize;
    Some(
        NOTIFICATION_RETRY_BACKOFF_SECS
            .get(index)
            .copied()
            .unwrap_or_else(|| *NOTIFICATION_RETRY_BACKOFF_SECS.last().unwrap_or(&1800)),
    )
}

async fn deliver_webhook_payload(
    client: &reqwest::Client,
    delivery: &FleetAlertNotificationDeliveryView,
) -> Result<()> {
    let url = validate_webhook_url(&delivery.target)?;
    let body = json!({
        "schema": "vpsman.fleet_alert.webhook_delivery.v1",
        "delivery": {
            "id": delivery.id,
            "channel_id": delivery.channel_id,
            "channel_name": &delivery.channel_name,
            "alert_id": &delivery.alert_id,
            "alert_severity": &delivery.alert_severity,
            "alert_category": &delivery.alert_category,
            "dedupe_key": &delivery.dedupe_key,
            "attempt": delivery.attempt_count.saturating_add(1),
            "created_at": &delivery.created_at,
        },
        "payload": &delivery.payload,
    });
    let response = client
        .post(url)
        .json(&body)
        .send()
        .await
        .context("webhook request failed")?;
    let status = response.status();
    anyhow::ensure!(
        status.is_success(),
        "webhook returned non-success status {}",
        status.as_u16()
    );
    Ok(())
}

fn validate_webhook_url(target: &str) -> Result<Url> {
    let url = Url::parse(target.trim()).context("webhook target must be an absolute URL")?;
    match url.scheme() {
        "https" => {}
        "http" if is_local_http_webhook(&url) => {}
        _ => anyhow::bail!("webhook target must use https, or http for localhost only"),
    }
    anyhow::ensure!(
        url.username().is_empty() && url.password().is_none(),
        "webhook target must not embed credentials"
    );
    Ok(url)
}

fn is_local_http_webhook(url: &Url) -> bool {
    matches!(
        url.host_str(),
        Some("localhost") | Some("127.0.0.1") | Some("::1") | Some("[::1]")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_threshold_matches_more_severe_alerts() {
        assert!(severity_rank("critical") <= severity_rank("warning"));
        assert!(severity_rank("warning") <= severity_rank("warning"));
        assert!(severity_rank("info") > severity_rank("warning"));
    }

    #[test]
    fn webhook_url_policy_allows_https_and_local_http_only() {
        assert!(validate_webhook_url("https://hooks.example/vpsman").is_ok());
        assert!(validate_webhook_url("http://127.0.0.1:9000/hook").is_ok());
        assert!(validate_webhook_url("http://example.com/hook").is_err());
        assert!(validate_webhook_url("https://user:secret@example.com/hook").is_err());
    }
}
