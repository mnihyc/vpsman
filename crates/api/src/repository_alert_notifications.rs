use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use serde_json::json;
use sqlx::{types::Json as SqlJson, Row};
use std::collections::HashSet;
use uuid::Uuid;
use vpsman_common::{
    is_fleet_alert_notification_delivery_status,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_CANCELED_DISABLED,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_DELIVERED,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_FAILED,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_IN_PROGRESS,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_PERMANENTLY_FAILED,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_QUEUED,
};

use crate::{
    model::{AuditLogView, AuthContext},
    model_alert_notifications::{
        CreateFleetAlertNotificationChannelRequest, FleetAlertNotificationCandidate,
        FleetAlertNotificationChannelView, FleetAlertNotificationDeliveryView,
    },
    repository::Repository,
    repository_webhook_rules::validate_webhook_rule_target,
    unix_now,
};

const SCOPE_GLOBAL: &str = "global";
const SCOPE_PROVIDER: &str = "provider";
const SCOPE_TAG: &str = "tag";
const SCOPE_CLIENT: &str = "client";
const DEFAULT_MIN_SEVERITY: &str = "warning";
const DEFAULT_COOLDOWN_SECS: i64 = 3600;
const MAX_COOLDOWN_SECS: i64 = 30 * 24 * 60 * 60;
const MAX_NAME_BYTES: usize = 128;
const MAX_SCOPE_VALUE_BYTES: usize = 128;
const MAX_TARGET_BYTES: usize = 512;
const MAX_NOTES_BYTES: usize = 1024;
const DELIVERY_KIND_WEBHOOK: &str = "webhook";

impl Repository {
    pub(crate) async fn list_fleet_alert_notification_channels(
        &self,
        limit: i64,
        enabled: Option<bool>,
        scope_kind: Option<&str>,
        scope_value: Option<&str>,
        delivery_kind: Option<&str>,
    ) -> Result<Vec<FleetAlertNotificationChannelView>> {
        let scope_kind = normalize_optional_scope_kind(scope_kind)?;
        let scope_value = normalize_optional_filter(scope_value);
        let delivery_kind = normalize_optional_delivery_kind(delivery_kind)?;
        match self {
            Self::Memory(memory) => {
                let mut rows = memory
                    .fleet_alert_notification_channels
                    .read()
                    .await
                    .iter()
                    .filter(|row| enabled.is_none_or(|value| row.enabled == value))
                    .filter(|row| {
                        scope_kind
                            .as_deref()
                            .is_none_or(|value| row.scope_kind == value)
                    })
                    .filter(|row| {
                        scope_value
                            .as_deref()
                            .is_none_or(|value| row.scope_value.as_deref() == Some(value))
                    })
                    .filter(|row| {
                        delivery_kind
                            .as_deref()
                            .is_none_or(|value| row.delivery_kind == value)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                sort_channels(&mut rows);
                rows.truncate(limit.clamp(1, 1000) as usize);
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        name,
                        scope_kind,
                        scope_value,
                        min_severity,
                        categories,
                        operator_states,
                        delivery_kind,
                        target,
                        cooldown_secs,
                        enabled,
                        notes,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM fleet_alert_notification_channels
                    WHERE ($2::boolean IS NULL OR enabled = $2)
                      AND ($3::text IS NULL OR scope_kind = $3)
                      AND ($4::text IS NULL OR scope_value = $4)
                      AND ($5::text IS NULL OR delivery_kind = $5)
                    ORDER BY enabled DESC, scope_kind, scope_value, name
                    LIMIT $1
                    "#,
                )
                .bind(limit.clamp(1, 1000))
                .bind(enabled)
                .bind(scope_kind.as_deref())
                .bind(scope_value.as_deref())
                .bind(delivery_kind.as_deref())
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(channel_from_row).collect()
            }
        }
    }

    pub(crate) async fn upsert_fleet_alert_notification_channel(
        &self,
        request: &CreateFleetAlertNotificationChannelRequest,
        operator: &AuthContext,
    ) -> Result<FleetAlertNotificationChannelView> {
        let candidate = channel_from_request(request, operator)?;
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut channels = memory.fleet_alert_notification_channels.write().await;
                anyhow::ensure!(
                    !channels.iter().any(|stored| {
                        stored.name == candidate.name && Some(stored.id) != request.id
                    }),
                    "fleet_alert_notification_channel_name_conflict"
                );
                let channel = if let Some(stored) = channels
                    .iter_mut()
                    .find(|stored| request.id.is_some_and(|id| stored.id == id))
                {
                    stored.name = candidate.name.clone();
                    stored.scope_kind = candidate.scope_kind.clone();
                    stored.scope_value = candidate.scope_value.clone();
                    stored.min_severity = candidate.min_severity.clone();
                    stored.categories = candidate.categories.clone();
                    stored.operator_states = candidate.operator_states.clone();
                    stored.delivery_kind = candidate.delivery_kind.clone();
                    stored.target = candidate.target.clone();
                    stored.cooldown_secs = candidate.cooldown_secs;
                    stored.enabled = candidate.enabled;
                    stored.notes = candidate.notes.clone();
                    stored.actor_id = candidate.actor_id;
                    stored.updated_at = now.clone();
                    stored.clone()
                } else {
                    channels.push(candidate.clone());
                    candidate
                };
                drop(channels);
                if !channel.enabled {
                    let mut deliveries = memory.fleet_alert_notification_deliveries.write().await;
                    cancel_memory_fleet_alert_notification_deliveries(
                        &mut deliveries,
                        channel.id,
                        "fleet alert notification channel disabled",
                    );
                }
                memory
                    .audits
                    .write()
                    .await
                    .push(notification_channel_audit(&channel, operator, now));
                Ok(channel)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    INSERT INTO fleet_alert_notification_channels (
                        id,
                        name,
                        scope_kind,
                        scope_value,
                        min_severity,
                        categories,
                        operator_states,
                        delivery_kind,
                        target,
                        cooldown_secs,
                        enabled,
                        notes,
                        actor_id
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
                    ON CONFLICT (id) DO UPDATE SET
                        name = EXCLUDED.name,
                        scope_kind = EXCLUDED.scope_kind,
                        scope_value = EXCLUDED.scope_value,
                        min_severity = EXCLUDED.min_severity,
                        categories = EXCLUDED.categories,
                        operator_states = EXCLUDED.operator_states,
                        delivery_kind = EXCLUDED.delivery_kind,
                        target = EXCLUDED.target,
                        cooldown_secs = EXCLUDED.cooldown_secs,
                        enabled = EXCLUDED.enabled,
                        notes = EXCLUDED.notes,
                        actor_id = EXCLUDED.actor_id,
                        updated_at = now()
                    RETURNING
                        id,
                        name,
                        scope_kind,
                        scope_value,
                        min_severity,
                        categories,
                        operator_states,
                        delivery_kind,
                        target,
                        cooldown_secs,
                        enabled,
                        notes,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    "#,
                )
                .bind(candidate.id)
                .bind(&candidate.name)
                .bind(&candidate.scope_kind)
                .bind(&candidate.scope_value)
                .bind(&candidate.min_severity)
                .bind(SqlJson(&candidate.categories))
                .bind(SqlJson(&candidate.operator_states))
                .bind(&candidate.delivery_kind)
                .bind(&candidate.target)
                .bind(candidate.cooldown_secs)
                .bind(candidate.enabled)
                .bind(&candidate.notes)
                .bind(operator.operator.id)
                .fetch_one(&mut *tx)
                .await?;
                let channel = channel_from_row(row)?;
                if !channel.enabled {
                    sqlx::query(
                        r#"
                        UPDATE fleet_alert_notification_deliveries
                        SET
                            status = 'canceled_disabled',
                            error = $2,
                            delivery_lease_id = NULL,
                            delivery_lease_until = NULL,
                            next_attempt_at = NULL,
                            delivered_at = NULL
                        WHERE channel_id = $1
                          AND status IN ('queued', 'failed', 'in_progress')
                        "#,
                    )
                    .bind(channel.id)
                    .bind("fleet alert notification channel disabled")
                    .execute(&mut *tx)
                    .await?;
                }
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("fleet.alert_notification_channel_upserted")
                .bind(format!("fleet_alert_notification_channel:{}", channel.id))
                .bind(notification_channel_metadata(&channel, operator))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(channel)
            }
        }
    }

    pub(crate) async fn delete_fleet_alert_notification_channel(
        &self,
        channel_id: Uuid,
        operator: &AuthContext,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let mut channels = memory.fleet_alert_notification_channels.write().await;
                let index = channels
                    .iter()
                    .position(|channel| channel.id == channel_id)
                    .ok_or_else(|| {
                        anyhow::anyhow!("fleet_alert_notification_channel_not_found:{channel_id}")
                    })?;
                let channel = channels.remove(index);
                drop(channels);
                memory
                    .fleet_alert_notification_deliveries
                    .write()
                    .await
                    .retain(|delivery| delivery.channel_id != channel_id);
                let mut audit =
                    notification_channel_audit(&channel, operator, unix_now().to_string());
                audit.action = "fleet.alert_notification_channel_deleted".to_string();
                memory.audits.write().await.push(audit);
                Ok(())
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    DELETE FROM fleet_alert_notification_channels
                    WHERE id = $1
                    RETURNING
                        id,
                        name,
                        scope_kind,
                        scope_value,
                        min_severity,
                        categories,
                        operator_states,
                        delivery_kind,
                        target,
                        cooldown_secs,
                        enabled,
                        notes,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    "#,
                )
                .bind(channel_id)
                .fetch_one(&mut *tx)
                .await?;
                let channel = channel_from_row(row)?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("fleet.alert_notification_channel_deleted")
                .bind(format!("fleet_alert_notification_channel:{}", channel.id))
                .bind(notification_channel_metadata(&channel, operator))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(())
            }
        }
    }

    pub(crate) async fn list_fleet_alert_notification_deliveries(
        &self,
        limit: i64,
        channel_id: Option<Uuid>,
        alert_id: Option<&str>,
        status: Option<&str>,
    ) -> Result<Vec<FleetAlertNotificationDeliveryView>> {
        let alert_id = normalize_optional_alert_id(alert_id)?;
        let status = normalize_optional_status(status)?;
        match self {
            Self::Memory(memory) => {
                let mut rows = memory
                    .fleet_alert_notification_deliveries
                    .read()
                    .await
                    .iter()
                    .filter(|row| channel_id.is_none_or(|value| row.channel_id == value))
                    .filter(|row| {
                        alert_id
                            .as_deref()
                            .is_none_or(|value| row.alert_id == value)
                    })
                    .filter(|row| status.as_deref().is_none_or(|value| row.status == value))
                    .cloned()
                    .collect::<Vec<_>>();
                sort_deliveries(&mut rows);
                rows.truncate(limit.clamp(1, 1000) as usize);
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        channel_id,
                        channel_name,
                        alert_id,
                        alert_severity,
                        alert_category,
                        status,
                        delivery_kind,
                        target,
                        dedupe_key,
                        payload,
                        error,
                        attempt_count,
                        next_attempt_at::text AS next_attempt_at,
                        last_attempt_at::text AS last_attempt_at,
                        cooldown_until_unix,
                        actor_id,
                        created_at::text AS created_at,
                        delivered_at::text AS delivered_at
                    FROM fleet_alert_notification_deliveries
                    WHERE ($2::uuid IS NULL OR channel_id = $2)
                      AND ($3::text IS NULL OR alert_id = $3)
                      AND ($4::text IS NULL OR status = $4)
                    ORDER BY created_at DESC, alert_id ASC
                    LIMIT $1
                    "#,
                )
                .bind(limit.clamp(1, 1000))
                .bind(channel_id)
                .bind(alert_id.as_deref())
                .bind(status.as_deref())
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(delivery_from_row).collect()
            }
        }
    }

    pub(crate) async fn record_fleet_alert_notification_deliveries(
        &self,
        candidates: &[FleetAlertNotificationCandidate],
        operator: &AuthContext,
    ) -> Result<Vec<FleetAlertNotificationDeliveryView>> {
        let now = unix_now();
        match self {
            Self::Memory(memory) => {
                let mut persisted = Vec::new();
                let mut deliveries = memory.fleet_alert_notification_deliveries.write().await;
                for candidate in candidates {
                    if deliveries.iter().any(|stored| {
                        stored.dedupe_key == candidate.dedupe_key
                            && stored.cooldown_until_unix > now as i64
                    }) {
                        continue;
                    }
                    let delivery = delivery_from_candidate(candidate, operator, now);
                    deliveries.push(delivery.clone());
                    persisted.push(delivery);
                }
                drop(deliveries);
                if !persisted.is_empty() {
                    memory
                        .audits
                        .write()
                        .await
                        .push(notification_dispatch_audit(
                            &persisted,
                            operator,
                            now.to_string(),
                        ));
                }
                Ok(persisted)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let mut persisted = Vec::new();
                for candidate in candidates {
                    let duplicate = sqlx::query_scalar::<_, i64>(
                        r#"
                        SELECT 1::bigint
                        FROM fleet_alert_notification_deliveries
                        WHERE dedupe_key = $1
                          AND cooldown_until_unix > $2
                        LIMIT 1
                        "#,
                    )
                    .bind(&candidate.dedupe_key)
                    .bind(now as i64)
                    .fetch_optional(&mut *tx)
                    .await?
                    .is_some();
                    if duplicate {
                        continue;
                    }
                    let delivery = delivery_from_candidate(candidate, operator, now);
                    let row = sqlx::query(
                        r#"
                        INSERT INTO fleet_alert_notification_deliveries (
                            id,
                            channel_id,
                            channel_name,
                            alert_id,
                            alert_severity,
                            alert_category,
                            status,
                            delivery_kind,
                            target,
                            dedupe_key,
                            payload,
                            error,
                            attempt_count,
                            next_attempt_at,
                            last_attempt_at,
                            cooldown_until_unix,
                            actor_id,
                            delivered_at
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, 0, NULL, NULL, $13, $14, CASE WHEN $7 = 'delivered' THEN now() ELSE NULL END)
                        RETURNING
                            id,
                            channel_id,
                            channel_name,
                            alert_id,
                            alert_severity,
                            alert_category,
                            status,
                            delivery_kind,
                            target,
                            dedupe_key,
                            payload,
                            error,
                            attempt_count,
                            next_attempt_at::text AS next_attempt_at,
                            last_attempt_at::text AS last_attempt_at,
                            cooldown_until_unix,
                            actor_id,
                            created_at::text AS created_at,
                            delivered_at::text AS delivered_at
                        "#,
                    )
                    .bind(delivery.id)
                    .bind(delivery.channel_id)
                    .bind(&delivery.channel_name)
                    .bind(&delivery.alert_id)
                    .bind(&delivery.alert_severity)
                    .bind(&delivery.alert_category)
                    .bind(&delivery.status)
                    .bind(&delivery.delivery_kind)
                    .bind(&delivery.target)
                    .bind(&delivery.dedupe_key)
                    .bind(SqlJson(&delivery.payload))
                    .bind(&delivery.error)
                    .bind(delivery.cooldown_until_unix)
                    .bind(operator.operator.id)
                    .fetch_one(&mut *tx)
                    .await?;
                    persisted.push(delivery_from_row(row)?);
                }
                if !persisted.is_empty() {
                    sqlx::query(
                        r#"
                        INSERT INTO audit_logs (
                            id, actor_id, action, target, command_hash, metadata
                        )
                        VALUES ($1, $2, $3, $4, NULL, $5)
                        "#,
                    )
                    .bind(Uuid::new_v4())
                    .bind(operator.operator.id)
                    .bind("fleet.alert_notifications_dispatched")
                    .bind("fleet_alert_notifications")
                    .bind(notification_dispatch_metadata(&persisted, operator))
                    .execute(&mut *tx)
                    .await?;
                }
                tx.commit().await?;
                Ok(persisted)
            }
        }
    }

    pub(crate) async fn claim_fleet_alert_notification_deliveries_for_process(
        &self,
        delivery_ids: &[Uuid],
        lease_id: Uuid,
        lease_secs: i64,
    ) -> Result<Vec<FleetAlertNotificationDeliveryView>> {
        if delivery_ids.is_empty() {
            return Ok(Vec::new());
        }
        let id_set = delivery_ids.iter().copied().collect::<HashSet<_>>();
        match self {
            Self::Memory(memory) => {
                let enabled_channel_ids = memory
                    .fleet_alert_notification_channels
                    .read()
                    .await
                    .iter()
                    .filter(|channel| channel.enabled)
                    .map(|channel| channel.id)
                    .collect::<HashSet<_>>();
                let mut deliveries = memory.fleet_alert_notification_deliveries.write().await;
                let mut claimed = Vec::new();
                for delivery in deliveries.iter_mut() {
                    if !id_set.contains(&delivery.id)
                        || !enabled_channel_ids.contains(&delivery.channel_id)
                        || delivery.delivery_kind != DELIVERY_KIND_WEBHOOK
                        || !matches!(
                            delivery.status.as_str(),
                            FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_QUEUED
                                | FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_FAILED
                        )
                    {
                        continue;
                    }
                    delivery.status =
                        FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_IN_PROGRESS.to_string();
                    delivery.error = None;
                    delivery.next_attempt_at = None;
                    claimed.push(delivery.clone());
                }
                Ok(claimed)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH requested AS (
                        SELECT unnest($1::uuid[]) AS id
                    ),
                    claim AS (
                        SELECT delivery.id
                        FROM fleet_alert_notification_deliveries delivery
                        JOIN requested ON requested.id = delivery.id
                        JOIN fleet_alert_notification_channels channel
                          ON channel.id = delivery.channel_id
                         AND channel.enabled = TRUE
                        WHERE delivery.status IN ('queued', 'failed')
                          AND delivery.delivery_kind = 'webhook'
                        ORDER BY delivery.created_at ASC, delivery.id ASC
                        FOR UPDATE OF delivery SKIP LOCKED
                    )
                    UPDATE fleet_alert_notification_deliveries delivery
                    SET
                        status = 'in_progress',
                        error = NULL,
                        delivery_lease_id = $2,
                        delivery_lease_until = now() + ($3::bigint * interval '1 second'),
                        next_attempt_at = NULL
                    FROM claim
                    WHERE delivery.id = claim.id
                    RETURNING
                        delivery.id,
                        delivery.channel_id,
                        delivery.channel_name,
                        delivery.alert_id,
                        delivery.alert_severity,
                        delivery.alert_category,
                        delivery.status,
                        delivery.delivery_kind,
                        delivery.target,
                        delivery.dedupe_key,
                        delivery.payload,
                        delivery.error,
                        delivery.attempt_count,
                        delivery.next_attempt_at::text AS next_attempt_at,
                        delivery.last_attempt_at::text AS last_attempt_at,
                        delivery.cooldown_until_unix,
                        delivery.actor_id,
                        delivery.created_at::text AS created_at,
                        delivery.delivered_at::text AS delivered_at
                    "#,
                )
                .bind(delivery_ids)
                .bind(lease_id)
                .bind(lease_secs.max(1))
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(delivery_from_row).collect()
            }
        }
    }

    pub(crate) async fn fleet_alert_notification_channel_enabled(
        &self,
        channel_id: Uuid,
    ) -> Result<bool> {
        match self {
            Self::Memory(memory) => Ok(memory
                .fleet_alert_notification_channels
                .read()
                .await
                .iter()
                .any(|channel| channel.id == channel_id && channel.enabled)),
            Self::Postgres(pool) => {
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
        }
    }

    pub(crate) async fn complete_fleet_alert_notification_delivery_attempt(
        &self,
        delivery_id: Uuid,
        lease_id: Uuid,
        status: &str,
        error: Option<&str>,
        next_attempt_after_secs: Option<i64>,
    ) -> Result<FleetAlertNotificationDeliveryView> {
        let status = normalize_delivery_attempt_status(status)?;
        let error = error
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.chars().take(MAX_NOTES_BYTES).collect::<String>());
        let now = unix_now().to_string();
        match self {
            Self::Memory(memory) => {
                let mut deliveries = memory.fleet_alert_notification_deliveries.write().await;
                let delivery = deliveries
                    .iter_mut()
                    .find(|delivery| delivery.id == delivery_id)
                    .context("fleet alert notification delivery not found")?;
                anyhow::ensure!(
                    delivery.status == FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_IN_PROGRESS,
                    "fleet alert notification delivery is not claimed"
                );
                delivery.status = status.to_string();
                delivery.error = error;
                delivery.attempt_count = delivery.attempt_count.saturating_add(1);
                delivery.next_attempt_at = next_attempt_after_secs
                    .filter(|seconds| *seconds > 0)
                    .map(|seconds| (Utc::now() + Duration::seconds(seconds)).to_rfc3339());
                delivery.last_attempt_at = Some(now.clone());
                delivery.delivered_at =
                    (status == FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_DELIVERED).then_some(now);
                Ok(delivery.clone())
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    UPDATE fleet_alert_notification_deliveries
                    SET
                        status = $2,
                        error = $3,
                        attempt_count = attempt_count + 1,
                        delivery_lease_id = NULL,
                        delivery_lease_until = NULL,
                        next_attempt_at = CASE
                            WHEN $5::bigint IS NULL THEN NULL
                            ELSE now() + ($5::bigint * interval '1 second')
                        END,
                        last_attempt_at = now(),
                        delivered_at = CASE WHEN $2 = 'delivered' THEN now() ELSE NULL END
                    WHERE id = $1
                      AND status = 'in_progress'
                      AND delivery_lease_id = $4
                    RETURNING
                        id,
                        channel_id,
                        channel_name,
                        alert_id,
                        alert_severity,
                        alert_category,
                        status,
                        delivery_kind,
                        target,
                        dedupe_key,
                        payload,
                        error,
                        attempt_count,
                        next_attempt_at::text AS next_attempt_at,
                        last_attempt_at::text AS last_attempt_at,
                        cooldown_until_unix,
                        actor_id,
                        created_at::text AS created_at,
                        delivered_at::text AS delivered_at
                    "#,
                )
                .bind(delivery_id)
                .bind(status)
                .bind(error.as_deref())
                .bind(lease_id)
                .bind(next_attempt_after_secs.filter(|seconds| *seconds > 0))
                .fetch_optional(pool)
                .await?
                .context("fleet alert notification delivery not found or not claimed")?;
                delivery_from_row(row)
            }
        }
    }

    pub(crate) async fn cancel_claimed_fleet_alert_notification_delivery(
        &self,
        delivery_id: Uuid,
        lease_id: Uuid,
        error: &str,
    ) -> Result<FleetAlertNotificationDeliveryView> {
        let error = error
            .trim()
            .chars()
            .take(MAX_NOTES_BYTES)
            .collect::<String>();
        match self {
            Self::Memory(memory) => {
                let mut deliveries = memory.fleet_alert_notification_deliveries.write().await;
                let delivery = deliveries
                    .iter_mut()
                    .find(|delivery| delivery.id == delivery_id)
                    .context("fleet alert notification delivery not found")?;
                anyhow::ensure!(
                    delivery.status == FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_IN_PROGRESS,
                    "fleet alert notification delivery is not claimed"
                );
                delivery.status =
                    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_CANCELED_DISABLED.to_string();
                delivery.error = Some(error);
                delivery.next_attempt_at = None;
                delivery.delivered_at = None;
                Ok(delivery.clone())
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
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
                    RETURNING
                        id,
                        channel_id,
                        channel_name,
                        alert_id,
                        alert_severity,
                        alert_category,
                        status,
                        delivery_kind,
                        target,
                        dedupe_key,
                        payload,
                        error,
                        attempt_count,
                        next_attempt_at::text AS next_attempt_at,
                        last_attempt_at::text AS last_attempt_at,
                        cooldown_until_unix,
                        actor_id,
                        created_at::text AS created_at,
                        delivered_at::text AS delivered_at
                    "#,
                )
                .bind(delivery_id)
                .bind(lease_id)
                .bind(&error)
                .fetch_optional(pool)
                .await?
                .context("fleet alert notification delivery not found or not claimed")?;
                delivery_from_row(row)
            }
        }
    }

    pub(crate) async fn record_fleet_alert_notification_process_audit(
        &self,
        deliveries: &[FleetAlertNotificationDeliveryView],
        operator: &AuthContext,
    ) -> Result<()> {
        if deliveries.is_empty() {
            return Ok(());
        }
        let created_at = unix_now().to_string();
        match self {
            Self::Memory(memory) => {
                memory
                    .audits
                    .write()
                    .await
                    .push(notification_process_audit(deliveries, operator, created_at));
                Ok(())
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("fleet.alert_notification_deliveries_processed")
                .bind("fleet_alert_notifications")
                .bind(notification_process_metadata(deliveries, operator))
                .execute(pool)
                .await?;
                Ok(())
            }
        }
    }
}

pub(crate) fn notification_status_for_kind(_delivery_kind: &str) -> &'static str {
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_QUEUED
}

fn channel_from_request(
    request: &CreateFleetAlertNotificationChannelRequest,
    operator: &AuthContext,
) -> Result<FleetAlertNotificationChannelView> {
    anyhow::ensure!(
        request.confirmed,
        "fleet_alert_notification_channel_confirmation_required"
    );
    validate_name(&request.name)?;
    let scope_kind = normalize_scope_kind(&request.scope_kind)?;
    let scope_value = normalize_scope_value(&scope_kind, request.scope_value.as_deref())?;
    let min_severity = normalize_severity(
        request
            .min_severity
            .as_deref()
            .unwrap_or(DEFAULT_MIN_SEVERITY),
    )?;
    let categories = normalize_tokens(request.categories.as_deref().unwrap_or(&[]), "category")?;
    let operator_states =
        normalize_operator_states(request.operator_states.as_deref().unwrap_or(&[]))?;
    let delivery_kind = normalize_delivery_kind(&request.delivery_kind)?;
    validate_target(&request.target)?;
    validate_notes(request.notes.as_deref())?;
    let cooldown_secs = request.cooldown_secs.unwrap_or(DEFAULT_COOLDOWN_SECS);
    anyhow::ensure!(
        (0..=MAX_COOLDOWN_SECS).contains(&cooldown_secs),
        "fleet alert notification cooldown is invalid"
    );
    Ok(FleetAlertNotificationChannelView {
        id: request.id.unwrap_or_else(Uuid::new_v4),
        name: request.name.trim().to_string(),
        scope_kind,
        scope_value,
        min_severity,
        categories,
        operator_states,
        delivery_kind,
        target: request.target.trim().to_string(),
        cooldown_secs,
        enabled: request.enabled.unwrap_or(true),
        notes: request
            .notes
            .as_deref()
            .map(str::trim)
            .filter(|notes| !notes.is_empty())
            .map(ToOwned::to_owned),
        actor_id: Some(operator.operator.id),
        created_at: unix_now().to_string(),
        updated_at: unix_now().to_string(),
    })
}

fn delivery_from_candidate(
    candidate: &FleetAlertNotificationCandidate,
    operator: &AuthContext,
    now: u64,
) -> FleetAlertNotificationDeliveryView {
    FleetAlertNotificationDeliveryView {
        id: Uuid::new_v4(),
        channel_id: candidate.channel_id,
        channel_name: candidate.channel_name.clone(),
        alert_id: candidate.alert_id.clone(),
        alert_severity: candidate.alert_severity.clone(),
        alert_category: candidate.alert_category.clone(),
        status: candidate.status.clone(),
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
        created_at: now.to_string(),
        delivered_at: (candidate.status == FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_DELIVERED)
            .then(|| now.to_string()),
        review_preview_hash: None,
    }
}

fn channel_from_row(row: sqlx::postgres::PgRow) -> Result<FleetAlertNotificationChannelView> {
    let categories: SqlJson<Vec<String>> = row.try_get("categories")?;
    let operator_states: SqlJson<Vec<String>> = row.try_get("operator_states")?;
    Ok(FleetAlertNotificationChannelView {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        scope_kind: row.try_get("scope_kind")?,
        scope_value: row.try_get("scope_value")?,
        min_severity: row.try_get("min_severity")?,
        categories: categories.0,
        operator_states: operator_states.0,
        delivery_kind: row.try_get("delivery_kind")?,
        target: row.try_get("target")?,
        cooldown_secs: row.try_get("cooldown_secs")?,
        enabled: row.try_get("enabled")?,
        notes: row.try_get("notes")?,
        actor_id: row.try_get("actor_id")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn delivery_from_row(row: sqlx::postgres::PgRow) -> Result<FleetAlertNotificationDeliveryView> {
    let payload: SqlJson<serde_json::Value> = row.try_get("payload")?;
    Ok(FleetAlertNotificationDeliveryView {
        id: row.try_get("id")?,
        channel_id: row.try_get("channel_id")?,
        channel_name: row.try_get("channel_name")?,
        alert_id: row.try_get("alert_id")?,
        alert_severity: row.try_get("alert_severity")?,
        alert_category: row.try_get("alert_category")?,
        status: row.try_get("status")?,
        delivery_kind: row.try_get("delivery_kind")?,
        target: row.try_get("target")?,
        dedupe_key: row.try_get("dedupe_key")?,
        payload: payload.0,
        error: row.try_get("error")?,
        attempt_count: row.try_get("attempt_count")?,
        next_attempt_at: row.try_get("next_attempt_at")?,
        last_attempt_at: row.try_get("last_attempt_at")?,
        cooldown_until_unix: row.try_get("cooldown_until_unix")?,
        actor_id: row.try_get("actor_id")?,
        created_at: row.try_get("created_at")?,
        delivered_at: row.try_get("delivered_at")?,
        review_preview_hash: None,
    })
}

fn sort_channels(rows: &mut [FleetAlertNotificationChannelView]) {
    rows.sort_by(|left, right| {
        right
            .enabled
            .cmp(&left.enabled)
            .then_with(|| left.scope_kind.cmp(&right.scope_kind))
            .then_with(|| left.scope_value.cmp(&right.scope_value))
            .then_with(|| left.name.cmp(&right.name))
    });
}

fn sort_deliveries(rows: &mut [FleetAlertNotificationDeliveryView]) {
    rows.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.alert_id.cmp(&right.alert_id))
    });
}

fn validate_name(name: &str) -> Result<()> {
    let name = name.trim();
    anyhow::ensure!(
        !name.is_empty() && name.len() <= MAX_NAME_BYTES,
        "fleet alert notification channel name is invalid"
    );
    Ok(())
}

fn normalize_scope_kind(scope_kind: &str) -> Result<String> {
    let scope_kind = scope_kind.trim();
    match scope_kind {
        SCOPE_GLOBAL | SCOPE_PROVIDER | SCOPE_TAG | SCOPE_CLIENT => Ok(scope_kind.to_string()),
        _ => anyhow::bail!("fleet alert notification scope kind is invalid"),
    }
}

fn normalize_optional_scope_kind(scope_kind: Option<&str>) -> Result<Option<String>> {
    scope_kind
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(normalize_scope_kind)
        .transpose()
}

fn normalize_scope_value(scope_kind: &str, scope_value: Option<&str>) -> Result<Option<String>> {
    let value = scope_value.map(str::trim).filter(|value| !value.is_empty());
    if scope_kind == SCOPE_GLOBAL {
        anyhow::ensure!(
            value.is_none(),
            "fleet alert notification global scope must not have a scope value"
        );
        return Ok(None);
    }
    let value = value.context("fleet alert notification scope value is required")?;
    anyhow::ensure!(
        value.len() <= MAX_SCOPE_VALUE_BYTES,
        "fleet alert notification scope value is too long"
    );
    Ok(Some(value.to_string()))
}

fn normalize_severity(severity: &str) -> Result<String> {
    let severity = severity.trim();
    match severity {
        "info" | "warning" | "critical" => Ok(severity.to_string()),
        _ => anyhow::bail!("fleet alert notification severity is invalid"),
    }
}

fn normalize_delivery_kind(delivery_kind: &str) -> Result<String> {
    let delivery_kind = delivery_kind.trim();
    anyhow::ensure!(
        delivery_kind == DELIVERY_KIND_WEBHOOK,
        "fleet alert notification delivery kind is invalid"
    );
    Ok(delivery_kind.to_string())
}

fn normalize_optional_delivery_kind(delivery_kind: Option<&str>) -> Result<Option<String>> {
    delivery_kind
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(normalize_delivery_kind)
        .transpose()
}

fn validate_target(target: &str) -> Result<()> {
    let target = target.trim();
    anyhow::ensure!(
        !target.is_empty() && target.len() <= MAX_TARGET_BYTES && !target.as_bytes().contains(&0),
        "fleet alert notification target is invalid"
    );
    validate_webhook_rule_target(target)?;
    Ok(())
}

fn validate_notes(notes: Option<&str>) -> Result<()> {
    if let Some(notes) = notes {
        anyhow::ensure!(
            notes.len() <= MAX_NOTES_BYTES,
            "fleet alert notification notes are too long"
        );
    }
    Ok(())
}

fn normalize_tokens(values: &[String], label: &str) -> Result<Vec<String>> {
    let mut normalized = Vec::new();
    for value in values {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        validate_token(value, label)?;
        if !normalized.iter().any(|stored| stored == value) {
            normalized.push(value.to_string());
        }
    }
    Ok(normalized)
}

fn normalize_operator_states(values: &[String]) -> Result<Vec<String>> {
    let mut normalized = Vec::new();
    for value in values {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match value {
            "open" | "acknowledged" | "muted" | "escalated" => {}
            _ => anyhow::bail!("fleet alert notification operator state is invalid"),
        }
        if !normalized.iter().any(|stored| stored == value) {
            normalized.push(value.to_string());
        }
    }
    Ok(normalized)
}

fn validate_token(value: &str, label: &str) -> Result<()> {
    anyhow::ensure!(
        value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_' | b'.')
        }),
        "{label} contains unsupported characters"
    );
    Ok(())
}

fn normalize_optional_filter(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn normalize_optional_alert_id(alert_id: Option<&str>) -> Result<Option<String>> {
    alert_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            anyhow::ensure!(
                value.len() <= 192,
                "fleet alert notification alert id is invalid"
            );
            validate_token(value, "fleet alert notification alert id")?;
            Ok(value.to_string())
        })
        .transpose()
}

fn normalize_optional_status(status: Option<&str>) -> Result<Option<String>> {
    status
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            anyhow::ensure!(
                is_fleet_alert_notification_delivery_status(value),
                "fleet alert notification status is invalid"
            );
            Ok(value.to_string())
        })
        .transpose()
}

fn normalize_delivery_attempt_status(status: &str) -> Result<&'static str> {
    match status.trim() {
        FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_DELIVERED => {
            Ok(FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_DELIVERED)
        }
        FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_FAILED => {
            Ok(FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_FAILED)
        }
        FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_PERMANENTLY_FAILED => {
            Ok(FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_PERMANENTLY_FAILED)
        }
        _ => anyhow::bail!("fleet alert notification delivery attempt status is invalid"),
    }
}

fn cancel_memory_fleet_alert_notification_deliveries(
    deliveries: &mut [FleetAlertNotificationDeliveryView],
    channel_id: Uuid,
    reason: &str,
) {
    for delivery in deliveries.iter_mut() {
        if delivery.channel_id != channel_id
            || !matches!(
                delivery.status.as_str(),
                FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_QUEUED
                    | FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_FAILED
                    | FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_IN_PROGRESS
            )
        {
            continue;
        }
        delivery.status = FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_CANCELED_DISABLED.to_string();
        delivery.error = Some(reason.to_string());
        delivery.next_attempt_at = None;
        delivery.delivered_at = None;
    }
}

fn notification_channel_audit(
    channel: &FleetAlertNotificationChannelView,
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: "fleet.alert_notification_channel_upserted".to_string(),
        target: format!("fleet_alert_notification_channel:{}", channel.id),
        command_hash: None,
        metadata: notification_channel_metadata(channel, operator),
        created_at,
    }
}

fn notification_channel_metadata(
    channel: &FleetAlertNotificationChannelView,
    operator: &AuthContext,
) -> serde_json::Value {
    json!({
        "channel_id": channel.id,
        "name": &channel.name,
        "scope_kind": &channel.scope_kind,
        "scope_value": &channel.scope_value,
        "min_severity": &channel.min_severity,
        "categories": &channel.categories,
        "operator_states": &channel.operator_states,
        "delivery_kind": &channel.delivery_kind,
        "target": &channel.target,
        "cooldown_secs": channel.cooldown_secs,
        "enabled": channel.enabled,
        "operator": {
            "id": operator.operator.id,
            "username": &operator.operator.username,
        },
    })
}

fn notification_dispatch_audit(
    deliveries: &[FleetAlertNotificationDeliveryView],
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: "fleet.alert_notifications_dispatched".to_string(),
        target: "fleet_alert_notifications".to_string(),
        command_hash: None,
        metadata: notification_dispatch_metadata(deliveries, operator),
        created_at,
    }
}

fn notification_dispatch_metadata(
    deliveries: &[FleetAlertNotificationDeliveryView],
    operator: &AuthContext,
) -> serde_json::Value {
    json!({
        "delivery_count": deliveries.len(),
        "deliveries": deliveries.iter().map(|delivery| json!({
            "id": delivery.id,
            "channel_id": delivery.channel_id,
            "alert_id": &delivery.alert_id,
            "status": &delivery.status,
            "delivery_kind": &delivery.delivery_kind,
        })).collect::<Vec<_>>(),
        "operator": {
            "id": operator.operator.id,
            "username": &operator.operator.username,
        },
    })
}

fn notification_process_audit(
    deliveries: &[FleetAlertNotificationDeliveryView],
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: "fleet.alert_notification_deliveries_processed".to_string(),
        target: "fleet_alert_notifications".to_string(),
        command_hash: None,
        metadata: notification_process_metadata(deliveries, operator),
        created_at,
    }
}

fn notification_process_metadata(
    deliveries: &[FleetAlertNotificationDeliveryView],
    operator: &AuthContext,
) -> serde_json::Value {
    json!({
        "delivery_count": deliveries.len(),
        "deliveries": deliveries.iter().map(|delivery| json!({
            "id": delivery.id,
            "channel_id": delivery.channel_id,
            "alert_id": &delivery.alert_id,
            "status": &delivery.status,
            "delivery_kind": &delivery.delivery_kind,
            "attempt_count": delivery.attempt_count,
            "error": &delivery.error,
        })).collect::<Vec<_>>(),
        "operator": {
            "id": operator.operator.id,
            "username": &operator.operator.username,
        },
    })
}
