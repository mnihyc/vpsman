use anyhow::{Context, Result};
use reqwest::{redirect::Policy, Url};
use serde_json::{json, Value};
use sqlx::{types::Json as SqlJson, PgPool, Row};
use tokio::time::Duration;
use uuid::Uuid;
use vpsman_common::{
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_CANCELED_DISABLED,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_DELIVERED,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_FAILED,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_PERMANENTLY_FAILED,
};

use crate::actor_authority::actor_authorized;

const DEFAULT_WEBHOOK_TIMEOUT_SECS: u64 = 5;
const MAX_ERROR_BYTES: usize = 1024;
const MAX_AUDIT_DELIVERY_ROWS: usize = 100;
const MAX_DELIVERY_ATTEMPTS: i32 = 4;
const RETRY_BACKOFF_SECS: [i64; 3] = [60, 300, 1800];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AlertNotificationWorkerConfig {
    pub(crate) delivery_limit: i64,
    pub(crate) retention_days: i64,
    pub(crate) retention_prune_limit: i64,
    pub(crate) webhook_timeout_secs: u64,
}

impl AlertNotificationWorkerConfig {
    pub(crate) fn new(
        delivery_limit: i64,
        retention_days: i64,
        retention_prune_limit: i64,
        webhook_timeout_secs: u64,
    ) -> Self {
        Self {
            delivery_limit: delivery_limit.clamp(1, 200),
            retention_days: retention_days.clamp(1, 3_650),
            retention_prune_limit: retention_prune_limit.clamp(1, 10_000),
            webhook_timeout_secs: webhook_timeout_secs.clamp(1, 60),
        }
    }
}

impl Default for AlertNotificationWorkerConfig {
    fn default() -> Self {
        Self::new(25, 90, 1_000, DEFAULT_WEBHOOK_TIMEOUT_SECS)
    }
}

#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct AlertNotificationWorkerRun {
    pub(crate) processed: usize,
    pub(crate) delivered: usize,
    pub(crate) failed: usize,
    pub(crate) pruned: usize,
}

#[derive(Clone, Debug)]
struct DeliveryRow {
    id: Uuid,
    actor_id: Option<Uuid>,
    channel_id: Uuid,
    channel_name: String,
    alert_id: String,
    alert_severity: String,
    alert_category: String,
    delivery_kind: String,
    target: String,
    dedupe_key: String,
    payload: Value,
    attempt_count: i32,
    created_at: String,
}

#[derive(Clone, Debug)]
struct DeliveryOutcome {
    id: Uuid,
    channel_id: Uuid,
    alert_id: String,
    status: String,
    delivery_kind: String,
    attempt_count: i32,
    error: Option<String>,
}

#[derive(Clone, Debug)]
struct PrunedDelivery {
    id: Uuid,
    status: String,
    delivery_kind: String,
    created_at: String,
}

pub(crate) async fn process_alert_notifications(
    pool: &PgPool,
    config: AlertNotificationWorkerConfig,
) -> Result<AlertNotificationWorkerRun> {
    let (processed, delivered, failed) = process_queued_deliveries(pool, config).await?;
    let pruned = prune_deliveries(pool, config).await?;
    Ok(AlertNotificationWorkerRun {
        processed,
        delivered,
        failed,
        pruned,
    })
}

async fn process_queued_deliveries(
    pool: &PgPool,
    config: AlertNotificationWorkerConfig,
) -> Result<(usize, usize, usize)> {
    let lease_id = Uuid::new_v4();
    let lease_secs = delivery_lease_secs(config.delivery_limit, config.webhook_timeout_secs);
    let rows = sqlx::query(
        r#"
        WITH claim AS (
            SELECT delivery.id
            FROM fleet_alert_notification_deliveries delivery
            JOIN fleet_alert_notification_channels channel
              ON channel.id = delivery.channel_id
             AND channel.enabled = TRUE
            WHERE delivery.delivery_kind = 'webhook'
              AND (
                delivery.status = 'queued'
                OR (
                    delivery.status = 'failed'
                    AND (delivery.next_attempt_at IS NULL OR delivery.next_attempt_at <= now())
                )
                OR (
                    delivery.status = 'in_progress'
                    AND delivery.delivery_lease_until < now()
                )
              )
            ORDER BY delivery.created_at ASC, delivery.id ASC
            LIMIT $1
            FOR UPDATE OF delivery SKIP LOCKED
        )
        UPDATE fleet_alert_notification_deliveries delivery
        SET status = 'in_progress',
            error = NULL,
            delivery_lease_id = $2,
            delivery_lease_until = now() + make_interval(secs => $3::integer),
            next_attempt_at = NULL
        FROM claim
        WHERE delivery.id = claim.id
        RETURNING
            delivery.id,
            delivery.actor_id,
            delivery.channel_id,
            delivery.channel_name,
            delivery.alert_id,
            delivery.alert_severity,
            delivery.alert_category,
            delivery.delivery_kind,
            delivery.target,
            delivery.dedupe_key,
            delivery.payload,
            delivery.attempt_count,
            delivery.created_at::text AS created_at
        "#,
    )
    .bind(config.delivery_limit)
    .bind(lease_id)
    .bind(lease_secs)
    .fetch_all(pool)
    .await?;

    let client = webhook_client(config.webhook_timeout_secs)?;
    let mut outcomes = Vec::new();
    for row in rows {
        let delivery = delivery_from_row(row)?;
        if !alert_notification_channel_enabled(pool, delivery.channel_id).await? {
            let updated = sqlx::query(
                r#"
                UPDATE fleet_alert_notification_deliveries
                SET
                    status = 'canceled_disabled',
                    error = $3,
                    delivery_lease_id = NULL,
                    delivery_lease_until = NULL,
                    next_attempt_at = NULL,
                    delivered_at = NULL
                WHERE id = $1
                  AND status = 'in_progress'
                  AND delivery_lease_id = $2
                RETURNING attempt_count
                "#,
            )
            .bind(delivery.id)
            .bind(lease_id)
            .bind("fleet alert notification channel disabled")
            .fetch_optional(pool)
            .await?;
            let Some(updated) = updated else {
                continue;
            };
            let recorded_attempt_count: i32 = updated.try_get("attempt_count")?;
            outcomes.push(DeliveryOutcome {
                id: delivery.id,
                channel_id: delivery.channel_id,
                alert_id: delivery.alert_id,
                status: FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_CANCELED_DISABLED.to_string(),
                delivery_kind: delivery.delivery_kind,
                attempt_count: recorded_attempt_count,
                error: Some("fleet alert notification channel disabled".to_string()),
            });
            continue;
        }
        let result =
            if actor_authorized(pool, delivery.actor_id, "operator", &["integrations:write"])
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
                let next_attempt_after_secs = next_retry_after_secs(delivery.attempt_count);
                (
                    if next_attempt_after_secs.is_some() {
                        FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_FAILED
                    } else {
                        FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_PERMANENTLY_FAILED
                    },
                    Some(truncate_error(&error.to_string())),
                    next_attempt_after_secs,
                )
            }
        };
        let updated = sqlx::query(
            r#"
            UPDATE fleet_alert_notification_deliveries
            SET
                status = $2,
                error = $3,
                attempt_count = attempt_count + 1,
                next_attempt_at = CASE
                    WHEN $5::bigint IS NULL THEN NULL
                    ELSE now() + ($5::bigint * interval '1 second')
                END,
                last_attempt_at = now(),
                delivered_at = CASE WHEN $2 = 'delivered' THEN now() ELSE NULL END,
                delivery_lease_id = NULL,
                delivery_lease_until = NULL
            WHERE id = $1
              AND status = 'in_progress'
              AND delivery_lease_id = $4
            RETURNING attempt_count
            "#,
        )
        .bind(delivery.id)
        .bind(status)
        .bind(error.as_deref())
        .bind(lease_id)
        .bind(next_attempt_after_secs)
        .fetch_optional(pool)
        .await?;
        let Some(updated) = updated else {
            continue;
        };
        let recorded_attempt_count: i32 = updated.try_get("attempt_count")?;
        outcomes.push(DeliveryOutcome {
            id: delivery.id,
            channel_id: delivery.channel_id,
            alert_id: delivery.alert_id,
            status: status.to_string(),
            delivery_kind: delivery.delivery_kind,
            attempt_count: recorded_attempt_count,
            error,
        });
    }

    if !outcomes.is_empty() {
        let mut tx = pool.begin().await?;
        insert_process_audit(&mut tx, &outcomes).await?;
        tx.commit().await?;
    }

    let delivered = outcomes
        .iter()
        .filter(|outcome| outcome.status == FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_DELIVERED)
        .count();
    let failed = outcomes.len().saturating_sub(delivered);
    Ok((outcomes.len(), delivered, failed))
}

fn delivery_lease_secs(limit: i64, webhook_timeout_secs: u64) -> i32 {
    let per_attempt = i64::try_from(webhook_timeout_secs).unwrap_or(i64::MAX);
    limit
        .clamp(1, 200)
        .saturating_mul(per_attempt.clamp(1, 60))
        .saturating_add(60)
        .clamp(60, i32::MAX as i64) as i32
}

async fn alert_notification_channel_enabled(pool: &PgPool, channel_id: Uuid) -> Result<bool> {
    let enabled = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT enabled
        FROM fleet_alert_notification_channels
        WHERE id = $1
        "#,
    )
    .bind(channel_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or(false);
    Ok(enabled)
}

async fn deliver_notification(client: &reqwest::Client, delivery: &DeliveryRow) -> Result<()> {
    anyhow::ensure!(
        delivery.delivery_kind == "webhook",
        "fleet alert notification delivery kind is invalid"
    );
    deliver_webhook_payload(client, delivery).await
}

async fn deliver_webhook_payload(client: &reqwest::Client, delivery: &DeliveryRow) -> Result<()> {
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

async fn prune_deliveries(pool: &PgPool, config: AlertNotificationWorkerConfig) -> Result<usize> {
    let rows = sqlx::query(
        r#"
        WITH candidates AS (
            SELECT id
            FROM fleet_alert_notification_deliveries
            WHERE status IN ('delivered', 'failed', 'permanently_failed', 'canceled_disabled')
              AND created_at <= now() - ($1::bigint * interval '1 day')
            ORDER BY created_at ASC, id ASC
            LIMIT $2
        ),
        deleted AS (
            DELETE FROM fleet_alert_notification_deliveries deliveries
            USING candidates
            WHERE deliveries.id = candidates.id
            RETURNING
                deliveries.id,
                deliveries.status,
                deliveries.delivery_kind,
                deliveries.created_at::text AS created_at
        )
        SELECT id, status, delivery_kind, created_at
        FROM deleted
        "#,
    )
    .bind(config.retention_days)
    .bind(config.retention_prune_limit)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(0);
    }
    let pruned = rows
        .into_iter()
        .map(|row| {
            Ok(PrunedDelivery {
                id: row.try_get("id")?,
                status: row.try_get("status")?,
                delivery_kind: row.try_get("delivery_kind")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    insert_prune_audit(pool, config, &pruned).await?;
    Ok(pruned.len())
}

async fn insert_process_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    outcomes: &[DeliveryOutcome],
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO audit_logs (
            id, actor_id, action, target, command_hash, metadata
        )
        VALUES ($1, NULL, $2, $3, NULL, $4)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind("fleet.alert_notification_deliveries_worker_processed")
    .bind("fleet_alert_notifications")
    .bind(json!({
        "worker": "alert_notification_worker",
        "delivery_count": outcomes.len(),
        "delivered_count": outcomes.iter().filter(|outcome| outcome.status == FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_DELIVERED).count(),
        "failed_count": outcomes.iter().filter(|outcome| outcome.status == FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_FAILED).count(),
        "deliveries": outcomes.iter().take(MAX_AUDIT_DELIVERY_ROWS).map(|outcome| json!({
            "id": outcome.id,
            "channel_id": outcome.channel_id,
            "alert_id": &outcome.alert_id,
            "status": &outcome.status,
            "delivery_kind": &outcome.delivery_kind,
            "attempt_count": outcome.attempt_count,
            "error": &outcome.error,
        })).collect::<Vec<_>>(),
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_prune_audit(
    pool: &PgPool,
    config: AlertNotificationWorkerConfig,
    pruned: &[PrunedDelivery],
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO audit_logs (
            id, actor_id, action, target, command_hash, metadata
        )
        VALUES ($1, NULL, $2, $3, NULL, $4)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind("fleet.alert_notification_deliveries_pruned")
    .bind("fleet_alert_notifications")
    .bind(json!({
        "worker": "alert_notification_worker",
        "retention_days": config.retention_days,
        "pruned_count": pruned.len(),
        "deliveries": pruned.iter().take(MAX_AUDIT_DELIVERY_ROWS).map(|delivery| json!({
            "id": delivery.id,
            "status": &delivery.status,
            "delivery_kind": &delivery.delivery_kind,
            "created_at": &delivery.created_at,
        })).collect::<Vec<_>>(),
    }))
    .execute(pool)
    .await?;
    Ok(())
}

fn delivery_from_row(row: sqlx::postgres::PgRow) -> Result<DeliveryRow> {
    let payload: SqlJson<Value> = row.try_get("payload")?;
    Ok(DeliveryRow {
        id: row.try_get("id")?,
        actor_id: row.try_get("actor_id")?,
        channel_id: row.try_get("channel_id")?,
        channel_name: row.try_get("channel_name")?,
        alert_id: row.try_get("alert_id")?,
        alert_severity: row.try_get("alert_severity")?,
        alert_category: row.try_get("alert_category")?,
        delivery_kind: row.try_get("delivery_kind")?,
        target: row.try_get("target")?,
        dedupe_key: row.try_get("dedupe_key")?,
        payload: payload.0,
        attempt_count: row.try_get("attempt_count")?,
        created_at: row.try_get("created_at")?,
    })
}

fn webhook_client(timeout_secs: u64) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs.clamp(1, 60)))
        .redirect(Policy::none())
        .build()
        .context("failed to build fleet alert notification webhook client")
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

fn truncate_error(error: &str) -> String {
    error.chars().take(MAX_ERROR_BYTES).collect()
}

fn next_retry_after_secs(attempt_count: i32) -> Option<i64> {
    let next_attempt_count = attempt_count.saturating_add(1);
    if next_attempt_count >= MAX_DELIVERY_ATTEMPTS {
        return None;
    }
    let index = next_attempt_count.saturating_sub(1) as usize;
    Some(
        RETRY_BACKOFF_SECS
            .get(index)
            .copied()
            .unwrap_or_else(|| *RETRY_BACKOFF_SECS.last().unwrap_or(&1800)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_worker_config_clamps_bounds() {
        assert_eq!(
            AlertNotificationWorkerConfig::new(0, 0, 0, 0),
            AlertNotificationWorkerConfig {
                delivery_limit: 1,
                retention_days: 1,
                retention_prune_limit: 1,
                webhook_timeout_secs: 1,
            }
        );
        assert_eq!(
            AlertNotificationWorkerConfig::new(10_000, 10_000, 20_000, 120),
            AlertNotificationWorkerConfig {
                delivery_limit: 200,
                retention_days: 3_650,
                retention_prune_limit: 10_000,
                webhook_timeout_secs: 60,
            }
        );
    }

    #[test]
    fn webhook_url_policy_allows_https_and_local_http_only() {
        assert!(validate_webhook_url("https://hooks.example/vpsman").is_ok());
        assert!(validate_webhook_url("http://localhost:9000/hook").is_ok());
        assert!(validate_webhook_url("http://127.0.0.1:9000/hook").is_ok());
        assert!(validate_webhook_url("http://hooks.example/hook").is_err());
        assert!(validate_webhook_url("https://user:secret@example.com/hook").is_err());
    }

    #[test]
    fn delivery_error_is_bounded() {
        let error = "x".repeat(MAX_ERROR_BYTES + 100);
        assert_eq!(truncate_error(&error).len(), MAX_ERROR_BYTES);
    }
}
