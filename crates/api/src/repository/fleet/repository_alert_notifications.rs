use crate::{
    model::AuthContext,
    model_alert_notifications::{
        CreateFleetAlertNotificationChannelRequest, FleetAlertNotificationCandidate,
        FleetAlertNotificationChannelBulkAction, FleetAlertNotificationChannelBulkOutcome,
        FleetAlertNotificationChannelBulkRequest, FleetAlertNotificationChannelBulkResponse,
        FleetAlertNotificationChannelView, FleetAlertNotificationDeliveryView,
    },
    repository::Repository,
    repository_webhook_rules::validate_webhook_rule_target,
    unix_now,
};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use sqlx::{types::Json as SqlJson, Executor, Postgres, QueryBuilder, Row};
use std::collections::{HashMap, HashSet};
use tracing::warn;
use uuid::Uuid;
use vpsman_common::{
    is_fleet_alert_notification_delivery_status,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_DELIVERED,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_FAILED,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_PERMANENTLY_FAILED,
    FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_QUEUED,
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
const FLEET_ALERT_NOTIFICATION_DISPATCH_CHANNEL_LIMIT: i64 = 1_000;

pub(crate) struct FleetAlertNotificationSendEligibility {
    channel_enabled: bool,
    eligibility_revision: Option<i64>,
}

impl FleetAlertNotificationSendEligibility {
    pub(crate) fn channel_enabled(&self) -> bool {
        self.channel_enabled
    }

    pub(crate) fn is_deliverable(&self) -> bool {
        self.eligibility_revision.is_some()
    }

    pub(crate) fn revision(&self) -> Option<i64> {
        self.eligibility_revision
    }
}

impl Repository {
    pub(crate) async fn list_fleet_alert_notification_channels(
        &self,
        limit: i64,
        enabled: Option<bool>,
        scope_kind: Option<&str>,
        scope_value: Option<&str>,
        delivery_kind: Option<&str>,
    ) -> Result<Vec<FleetAlertNotificationChannelView>> {
        self.query_fleet_alert_notification_channels(
            Some(limit.clamp(1, 1000)),
            enabled,
            scope_kind,
            scope_value,
            delivery_kind,
        )
        .await
    }

    pub(crate) async fn list_all_fleet_alert_notification_channels(
        &self,
    ) -> Result<Vec<FleetAlertNotificationChannelView>> {
        self.query_fleet_alert_notification_channels(None, None, None, None, None)
            .await
    }

    pub(crate) async fn list_enabled_fleet_alert_notification_channels_for_dispatch(
        &self,
    ) -> Result<Vec<FleetAlertNotificationChannelView>> {
        let channels = self
            .query_fleet_alert_notification_channels(
                Some(FLEET_ALERT_NOTIFICATION_DISPATCH_CHANNEL_LIMIT + 1),
                Some(true),
                None,
                None,
                None,
            )
            .await?;
        anyhow::ensure!(
            channels.len() <= FLEET_ALERT_NOTIFICATION_DISPATCH_CHANNEL_LIMIT as usize,
            "fleet_alert_notification_dispatch_channel_limit_exceeded:{}",
            FLEET_ALERT_NOTIFICATION_DISPATCH_CHANNEL_LIMIT
        );
        Ok(channels
            .into_iter()
            .filter(|channel| {
                if let Some(error) = channel.configuration_error.as_deref() {
                    warn!(
                        channel_id = %channel.id,
                        channel_name = %channel.name,
                        %error,
                        "skipping malformed fleet alert notification channel"
                    );
                    false
                } else {
                    true
                }
            })
            .collect())
    }

    async fn query_fleet_alert_notification_channels(
        &self,
        limit: Option<i64>,
        enabled: Option<bool>,
        scope_kind: Option<&str>,
        scope_value: Option<&str>,
        delivery_kind: Option<&str>,
    ) -> Result<Vec<FleetAlertNotificationChannelView>> {
        let scope_kind = normalize_optional_scope_kind(scope_kind)?;
        let scope_value = normalize_optional_filter(scope_value);
        let delivery_kind = normalize_optional_delivery_kind(delivery_kind)?;
        match self {
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
                .bind(limit.map(|limit| limit.max(1)))
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
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                if request.id.is_none() {
                    sqlx::query(
                        r#"
                        SELECT pg_advisory_xact_lock(hashtextextended(
                            'vpsman:alert-notification-channel-name:' || $1::text,
                            0
                        ))
                        "#,
                    )
                    .bind(&candidate.name)
                    .execute(&mut *tx)
                    .await?;
                    let existing = sqlx::query(
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
                        WHERE name = $1
                        FOR UPDATE
                        "#,
                    )
                    .bind(&candidate.name)
                    .fetch_optional(&mut *tx)
                    .await?
                    .map(channel_from_row)
                    .transpose()?;
                    if let Some(existing) = existing {
                        anyhow::ensure!(
                            fleet_alert_notification_channel_material_matches(
                                &existing, &candidate
                            ),
                            "fleet_alert_notification_channel_name_conflict"
                        );
                        tx.commit().await?;
                        return Ok(existing);
                    }
                }
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
                .await
                .map_err(fleet_alert_notification_channel_database_error)?;
                let channel = channel_from_row(row)?;
                if !channel.enabled {
                    // The channel row is the durable source state. The worker
                    // owns every delivery lease and terminal transition.
                    sqlx::query("SELECT pg_notify('webhook_events', 'alert_notification')")
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
        reviewed_name: &str,
        operator: &AuthContext,
    ) -> Result<()> {
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let current_name = sqlx::query_scalar::<_, String>(
                    "SELECT name FROM fleet_alert_notification_channels WHERE id = $1 FOR UPDATE",
                )
                .bind(channel_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| {
                    anyhow::anyhow!("fleet_alert_notification_channel_not_found:{channel_id}")
                })?;
                anyhow::ensure!(
                    current_name == reviewed_name.trim(),
                    "fleet_alert_notification_channel_delete_review_stale"
                );
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
                sqlx::query("SELECT pg_notify('webhook_events', 'alert_notification')")
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                Ok(())
            }
        }
    }

    pub(crate) async fn bulk_mutate_fleet_alert_notification_channels(
        &self,
        request: &FleetAlertNotificationChannelBulkRequest,
        operator: &AuthContext,
    ) -> Result<FleetAlertNotificationChannelBulkResponse> {
        anyhow::ensure!(
            (1..=500).contains(&request.items.len()),
            "fleet_alert_notification_channel_bulk_items_invalid"
        );
        let requested_ids = request.items.iter().map(|item| item.id).collect::<Vec<_>>();
        let unique_ids = requested_ids.iter().copied().collect::<HashSet<_>>();
        anyhow::ensure!(
            unique_ids.len() == requested_ids.len(),
            "fleet_alert_notification_channel_bulk_duplicate_item"
        );

        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id, name, scope_kind, scope_value, min_severity,
                        categories, operator_states, delivery_kind, target,
                        cooldown_secs, enabled, notes, actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM fleet_alert_notification_channels
                    WHERE id = ANY($1)
                    ORDER BY id
                    FOR UPDATE
                    "#,
                )
                .bind(&requested_ids)
                .fetch_all(&mut *tx)
                .await?;
                let mut current = rows
                    .into_iter()
                    .map(channel_from_row)
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .map(|channel| (channel.id, channel))
                    .collect::<HashMap<_, _>>();
                anyhow::ensure!(
                    current.len() == request.items.len(),
                    "fleet_alert_notification_channel_not_found"
                );
                let desired_enabled = match request.action {
                    FleetAlertNotificationChannelBulkAction::Enable => Some(true),
                    FleetAlertNotificationChannelBulkAction::Disable => Some(false),
                    FleetAlertNotificationChannelBulkAction::Delete => None,
                };
                for item in &request.items {
                    let channel = current
                        .get(&item.id)
                        .context("fleet_alert_notification_channel_not_found")?;
                    anyhow::ensure!(
                        channel.name == item.reviewed_name.trim()
                            && channel.updated_at == item.expected_updated_at.trim(),
                        "fleet_alert_notification_channel_bulk_review_stale"
                    );
                    if let Some(enabled) = desired_enabled {
                        anyhow::ensure!(
                            channel.enabled != enabled,
                            "fleet_alert_notification_channel_bulk_state_stale"
                        );
                    }
                }

                let changed_rows = if let Some(enabled) = desired_enabled {
                    sqlx::query(
                        r#"
                        UPDATE fleet_alert_notification_channels
                        SET enabled = $2, actor_id = $3, updated_at = now()
                        WHERE id = ANY($1)
                        RETURNING
                            id, name, scope_kind, scope_value, min_severity,
                            categories, operator_states, delivery_kind, target,
                            cooldown_secs, enabled, notes, actor_id,
                            created_at::text AS created_at,
                            updated_at::text AS updated_at
                        "#,
                    )
                    .bind(&requested_ids)
                    .bind(enabled)
                    .bind(operator.operator.id)
                    .fetch_all(&mut *tx)
                    .await?
                } else {
                    sqlx::query(
                        r#"
                        DELETE FROM fleet_alert_notification_channels
                        WHERE id = ANY($1)
                        RETURNING
                            id, name, scope_kind, scope_value, min_severity,
                            categories, operator_states, delivery_kind, target,
                            cooldown_secs, enabled, notes, actor_id,
                            created_at::text AS created_at,
                            updated_at::text AS updated_at
                        "#,
                    )
                    .bind(&requested_ids)
                    .fetch_all(&mut *tx)
                    .await?
                };
                current = changed_rows
                    .into_iter()
                    .map(channel_from_row)
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .map(|channel| (channel.id, channel))
                    .collect();
                anyhow::ensure!(
                    current.len() == request.items.len(),
                    "fleet_alert_notification_channel_bulk_snapshot_stale"
                );

                let (audit_action, result) = match request.action {
                    FleetAlertNotificationChannelBulkAction::Enable => {
                        ("fleet.alert_notification_channel_upserted", "enabled")
                    }
                    FleetAlertNotificationChannelBulkAction::Disable => {
                        ("fleet.alert_notification_channel_upserted", "disabled")
                    }
                    FleetAlertNotificationChannelBulkAction::Delete => {
                        ("fleet.alert_notification_channel_deleted", "deleted")
                    }
                };
                let ordered_channels = request
                    .items
                    .iter()
                    .map(|item| {
                        current
                            .get(&item.id)
                            .context("fleet_alert_notification_channel_bulk_snapshot_stale")
                    })
                    .collect::<Result<Vec<_>>>()?;
                let mut audit = QueryBuilder::<Postgres>::new(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    "#,
                );
                audit.push_values(&ordered_channels, |mut row, channel| {
                    row.push_bind(Uuid::new_v4())
                        .push_bind(operator.operator.id)
                        .push_bind(audit_action)
                        .push_bind(format!("fleet_alert_notification_channel:{}", channel.id))
                        .push("NULL")
                        .push_bind(notification_channel_metadata(channel, operator));
                });
                audit.build().execute(&mut *tx).await?;
                let outcomes: Vec<FleetAlertNotificationChannelBulkOutcome> = ordered_channels
                    .into_iter()
                    .map(|channel| FleetAlertNotificationChannelBulkOutcome {
                        id: channel.id,
                        name: channel.name.clone(),
                        result: result.to_string(),
                        record: (request.action != FleetAlertNotificationChannelBulkAction::Delete)
                            .then(|| channel.clone()),
                    })
                    .collect();
                if request.action != FleetAlertNotificationChannelBulkAction::Enable {
                    // Source state changed once; the delivery worker remains the
                    // sole owner of leases and terminal delivery transitions.
                    sqlx::query("SELECT pg_notify('webhook_events', 'alert_notification')")
                        .execute(&mut *tx)
                        .await?;
                }
                tx.commit().await?;
                Ok(FleetAlertNotificationChannelBulkResponse {
                    action: request.action,
                    outcomes,
                })
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
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let mut channel_ids = candidates
                    .iter()
                    .map(|candidate| candidate.channel_id)
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                channel_ids.sort_unstable();
                let enabled_channel_ids = sqlx::query_scalar::<_, Uuid>(
                    r#"
                    SELECT id
                    FROM fleet_alert_notification_channels
                    WHERE id = ANY($1) AND enabled = TRUE
                    ORDER BY id
                    "#,
                )
                .bind(&channel_ids)
                .fetch_all(&mut *tx)
                .await?
                .into_iter()
                .collect::<HashSet<_>>();
                let mut persisted = Vec::new();
                for candidate in candidates {
                    if !enabled_channel_ids.contains(&candidate.channel_id) {
                        continue;
                    }
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
                if persisted.iter().any(|delivery| {
                    delivery.status == FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_QUEUED
                }) {
                    // The queue row and its wake commit atomically. NOTIFY is
                    // only an acceleration hint; the periodic worker tick is
                    // still the durable retry fallback.
                    sqlx::query("SELECT pg_notify('webhook_events', 'alert_notification')")
                        .execute(&mut *tx)
                        .await?;
                }
                tx.commit().await?;
                Ok(persisted)
            }
        }
    }

    pub(crate) async fn claim_fleet_alert_notification_delivery_for_process(
        &self,
        delivery_id: Uuid,
        lease_id: Uuid,
        lease_secs: i64,
    ) -> Result<Option<FleetAlertNotificationDeliveryView>> {
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    WITH claim AS (
                        SELECT delivery.id
                        FROM fleet_alert_notification_deliveries delivery
                        JOIN fleet_alert_notification_channels channel
                          ON channel.id = delivery.channel_id
                         AND channel.enabled = TRUE
                        WHERE delivery.id = $1
                          AND delivery.status IN ('queued', 'failed')
                          AND delivery.delivery_kind = 'webhook'
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
                .bind(delivery_id)
                .bind(lease_id)
                .bind(lease_secs.max(1))
                .fetch_optional(pool)
                .await?;
                row.map(delivery_from_row).transpose()
            }
        }
    }

    pub(crate) async fn fleet_alert_notification_channel_enabled(
        &self,
        channel_id: Uuid,
    ) -> Result<bool> {
        match self {
            Self::Postgres(pool) => Ok(sqlx::query_scalar::<_, bool>(
                "SELECT enabled FROM fleet_alert_notification_channels WHERE id=$1",
            )
            .bind(channel_id)
            .fetch_optional(pool)
            .await?
            .unwrap_or(false)),
        }
    }

    pub(crate) async fn begin_fleet_alert_notification_send(
        &self,
        delivery_id: Uuid,
        channel_id: Uuid,
        alert_id: &str,
        lease_id: Uuid,
    ) -> Result<FleetAlertNotificationSendEligibility> {
        match self {
            Self::Postgres(pool) => {
                let (channel_enabled, eligibility_revision) =
                    postgres_arm_fleet_alert_notification_send(
                        pool,
                        delivery_id,
                        channel_id,
                        alert_id,
                        lease_id,
                    )
                    .await?;
                Ok(FleetAlertNotificationSendEligibility {
                    channel_enabled,
                    eligibility_revision,
                })
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
        eligibility_revision: Option<i64>,
    ) -> Result<FleetAlertNotificationDeliveryView> {
        let status = normalize_delivery_attempt_status(status)?;
        let error = error
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.chars().take(MAX_NOTES_BYTES).collect::<String>());
        match self {
            Self::Postgres(pool) => {
                postgres_complete_fleet_alert_notification_delivery_attempt(
                    pool,
                    delivery_id,
                    lease_id,
                    eligibility_revision,
                    status,
                    error.as_deref(),
                    next_attempt_after_secs,
                )
                .await
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
            Self::Postgres(pool) => {
                postgres_cancel_claimed_fleet_alert_notification_delivery(
                    pool,
                    delivery_id,
                    lease_id,
                    &error,
                )
                .await
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
        match self {
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

async fn postgres_complete_fleet_alert_notification_delivery_attempt<'e, E>(
    executor: E,
    delivery_id: Uuid,
    lease_id: Uuid,
    eligibility_revision: Option<i64>,
    status: &str,
    error: Option<&str>,
    next_attempt_after_secs: Option<i64>,
) -> Result<FleetAlertNotificationDeliveryView>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query(
        r#"
        UPDATE fleet_alert_notification_deliveries
        SET
            status = $3,
            error = $4,
            attempt_count = attempt_count + 1,
            delivery_lease_id = NULL,
            delivery_lease_until = NULL,
            next_attempt_at = CASE
                WHEN $6::bigint IS NULL THEN NULL
                ELSE now() + ($6::bigint * interval '1 second')
            END,
            last_attempt_at = now(),
            delivered_at = CASE WHEN $3 = 'delivered' THEN now() ELSE NULL END
        WHERE id = $1
          AND status = 'in_progress'
          AND delivery_lease_id = $2
          AND ($5::bigint IS NULL OR eligibility_revision=$5)
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
    .bind(status)
    .bind(error)
    .bind(eligibility_revision)
    .bind(next_attempt_after_secs.filter(|seconds| *seconds > 0))
    .fetch_optional(executor)
    .await?
    .context("fleet alert notification delivery not found or not claimed")?;
    delivery_from_row(row)
}

async fn postgres_cancel_claimed_fleet_alert_notification_delivery<'e, E>(
    executor: E,
    delivery_id: Uuid,
    lease_id: Uuid,
    error: &str,
) -> Result<FleetAlertNotificationDeliveryView>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query(
        r#"
        WITH updated AS (
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
                id, channel_id, channel_name, alert_id, alert_severity,
                alert_category, status, delivery_kind, target, dedupe_key,
                payload, error, attempt_count,
                next_attempt_at::text AS next_attempt_at,
                last_attempt_at::text AS last_attempt_at,
                cooldown_until_unix, actor_id,
                created_at::text AS created_at,
                delivered_at::text AS delivered_at
        )
        SELECT * FROM updated
        UNION ALL
        SELECT
            id, channel_id, channel_name, alert_id, alert_severity,
            alert_category, status, delivery_kind, target, dedupe_key,
            payload, error, attempt_count,
            next_attempt_at::text AS next_attempt_at,
            last_attempt_at::text AS last_attempt_at,
            cooldown_until_unix, actor_id,
            created_at::text AS created_at,
            delivered_at::text AS delivered_at
        FROM fleet_alert_notification_deliveries
        WHERE id=$1 AND status='canceled_disabled'
          AND NOT EXISTS (SELECT 1 FROM updated)
        LIMIT 1
        "#,
    )
    .bind(delivery_id)
    .bind(lease_id)
    .bind(error)
    .fetch_optional(executor)
    .await?
    .context("fleet alert notification delivery not found or not claimed")?;
    delivery_from_row(row)
}

async fn postgres_arm_fleet_alert_notification_send<'e, E>(
    executor: E,
    delivery_id: Uuid,
    channel_id: Uuid,
    alert_id: &str,
    lease_id: Uuid,
) -> Result<(bool, Option<i64>)>
where
    E: Executor<'e, Database = Postgres>,
{
    let state = sqlx::query_as::<_, (bool, Option<i64>)>(
        r#"
        WITH eligibility AS MATERIALIZED (
          SELECT
            EXISTS (
                SELECT 1
                FROM fleet_alert_notification_channels channel
                WHERE channel.id=$2 AND channel.enabled
            ) AS channel_enabled,
            (
                EXISTS (
                    SELECT 1
                    FROM fleet_alert_notification_deliveries delivery
                    WHERE delivery.id=$1
                      AND delivery.channel_id=$2
                      AND delivery.alert_id=$3
                      AND delivery.status='in_progress'
                      AND delivery.delivery_lease_id=$4
                )
                AND EXISTS (
                    SELECT 1
                    FROM fleet_alert_notification_channels channel
                    WHERE channel.id=$2 AND channel.enabled
                )
                AND EXISTS (
                    SELECT 1
                    FROM alert_episodes episode
                    LEFT JOIN clients subject ON subject.id=episode.client_id
                    WHERE episode.public_id=$3
                      AND episode.resolved_at IS NULL
                      AND (
                            episode.client_id IS NULL
                            OR (
                                subject.id IS NOT NULL
                                AND subject.status <> 'suspended'
                            )
                      )
                )
            ) AS deliverable
        ), armed AS (
            UPDATE fleet_alert_notification_deliveries delivery
            SET eligibility_revision=delivery.eligibility_revision+1
            FROM eligibility
            WHERE delivery.id=$1 AND eligibility.deliverable
              AND delivery.status='in_progress'
              AND delivery.delivery_lease_id=$4
            RETURNING delivery.eligibility_revision
        )
        SELECT eligibility.channel_enabled, armed.eligibility_revision
        FROM eligibility LEFT JOIN armed ON TRUE
        "#,
    )
    .bind(delivery_id)
    .bind(channel_id)
    .bind(alert_id)
    .bind(lease_id)
    .fetch_one(executor)
    .await?;
    Ok(state)
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
        configuration_error: None,
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
        process_outcome: None,
    }
}

fn channel_from_row(row: sqlx::postgres::PgRow) -> Result<FleetAlertNotificationChannelView> {
    let categories: SqlJson<Value> = row.try_get("categories")?;
    let operator_states: SqlJson<Value> = row.try_get("operator_states")?;
    let (categories, categories_valid) = persisted_channel_tokens(categories.0, false);
    let (operator_states, operator_states_valid) =
        persisted_channel_tokens(operator_states.0, true);
    let configuration_error = (!categories_valid || !operator_states_valid)
        .then(|| "fleet_alert_notification_channel_filters_invalid".to_string());
    Ok(FleetAlertNotificationChannelView {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        scope_kind: row.try_get("scope_kind")?,
        scope_value: row.try_get("scope_value")?,
        min_severity: row.try_get("min_severity")?,
        categories,
        operator_states,
        delivery_kind: row.try_get("delivery_kind")?,
        target: row.try_get("target")?,
        cooldown_secs: row.try_get("cooldown_secs")?,
        enabled: row.try_get("enabled")?,
        configuration_error,
        notes: row.try_get("notes")?,
        actor_id: row.try_get("actor_id")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn fleet_alert_notification_channel_material_matches(
    existing: &FleetAlertNotificationChannelView,
    candidate: &FleetAlertNotificationChannelView,
) -> bool {
    existing.configuration_error.is_none()
        && existing.name == candidate.name
        && existing.scope_kind == candidate.scope_kind
        && existing.scope_value == candidate.scope_value
        && existing.min_severity == candidate.min_severity
        && existing.categories == candidate.categories
        && existing.operator_states == candidate.operator_states
        && existing.delivery_kind == candidate.delivery_kind
        && existing.target == candidate.target
        && existing.cooldown_secs == candidate.cooldown_secs
        && existing.enabled == candidate.enabled
        && existing.notes == candidate.notes
}

fn persisted_channel_tokens(value: Value, operator_states: bool) -> (Vec<String>, bool) {
    let Ok(stored) = serde_json::from_value::<Vec<String>>(value) else {
        return (Vec::new(), false);
    };
    let normalized = if operator_states {
        normalize_operator_states(&stored)
    } else {
        normalize_tokens(&stored, "category")
    };
    match normalized {
        Ok(normalized) if normalized == stored => (stored, true),
        _ => (Vec::new(), false),
    }
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
        process_outcome: None,
    })
}

fn validate_name(name: &str) -> Result<()> {
    let name = name.trim();
    anyhow::ensure!(
        !name.is_empty() && name.len() <= MAX_NAME_BYTES,
        "fleet alert notification channel name is invalid"
    );
    Ok(())
}

fn fleet_alert_notification_channel_database_error(error: sqlx::Error) -> anyhow::Error {
    if error
        .as_database_error()
        .and_then(|database_error| database_error.constraint())
        == Some("fleet_alert_notification_channels_name_key")
    {
        anyhow::anyhow!("fleet_alert_notification_channel_name_conflict")
    } else {
        error.into()
    }
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
        "configuration_error": &channel.configuration_error,
        "result": "succeeded",
        "operator_id": operator.operator.id,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "operator_session_id": operator.audit_session_id(),
        "origin_kind": "operator_request",
        "component": "alert-notification-controller",
    })
}

fn notification_dispatch_metadata(
    deliveries: &[FleetAlertNotificationDeliveryView],
    operator: &AuthContext,
) -> serde_json::Value {
    json!({
        "delivery_count": deliveries.len(),
        "result": "queued",
        "deliveries": deliveries.iter().map(|delivery| json!({
            "id": delivery.id,
            "channel_id": delivery.channel_id,
            "alert_id": &delivery.alert_id,
            "status": &delivery.status,
            "delivery_kind": &delivery.delivery_kind,
        })).collect::<Vec<_>>(),
        "operator_id": operator.operator.id,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "operator_session_id": operator.audit_session_id(),
        "origin_kind": "operator_request",
        "component": "alert-notification-controller",
    })
}

fn notification_process_metadata(
    deliveries: &[FleetAlertNotificationDeliveryView],
    operator: &AuthContext,
) -> serde_json::Value {
    let delivered_count = deliveries
        .iter()
        .filter(|delivery| delivery.status == FLEET_ALERT_NOTIFICATION_DELIVERY_STATUS_DELIVERED)
        .count();
    let non_delivered_count = deliveries.len().saturating_sub(delivered_count);
    json!({
        "delivery_count": deliveries.len(),
        "delivered_count": delivered_count,
        "non_delivered_count": non_delivered_count,
        "result": if non_delivered_count == 0 { "succeeded" } else { "partial" },
        "deliveries": deliveries.iter().map(|delivery| json!({
            "id": delivery.id,
            "channel_id": delivery.channel_id,
            "alert_id": &delivery.alert_id,
            "status": &delivery.status,
            "delivery_kind": &delivery.delivery_kind,
            "attempt_count": delivery.attempt_count,
            "error": &delivery.error,
        })).collect::<Vec<_>>(),
        "operator_id": operator.operator.id,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "operator_session_id": operator.audit_session_id(),
        "origin_kind": "operator_request",
        "component": "alert-notification-controller",
    })
}

#[cfg(test)]
mod ownership_tests {
    #[test]
    fn channel_mutations_signal_but_never_terminalize_worker_delivery_rows() {
        let source = include_str!("repository_alert_notifications.rs");
        let (_, upsert) = source
            .split_once("pub(crate) async fn upsert_fleet_alert_notification_channel")
            .expect("notification channel upsert");
        let (upsert, delete_and_after) = upsert
            .split_once("pub(crate) async fn delete_fleet_alert_notification_channel")
            .expect("notification channel delete boundary");
        let (delete, producer_and_after) = delete_and_after
            .split_once("pub(crate) async fn list_fleet_alert_notification_deliveries")
            .expect("notification channel delete end");
        let (_, producer) = producer_and_after
            .split_once("pub(crate) async fn record_fleet_alert_notification_deliveries")
            .expect("notification delivery producer");
        let (producer, _) = producer
            .split_once("pub(crate) async fn claim_fleet_alert_notification_delivery_for_process")
            .expect("notification delivery producer boundary");

        for source_transition in [upsert, delete] {
            assert!(source_transition.contains("pg_notify('webhook_events'"));
            assert!(!source_transition.contains("UPDATE fleet_alert_notification_deliveries"));
        }
        assert!(!producer.contains("FOR UPDATE"));
    }
}
