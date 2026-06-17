use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use reqwest::Url;
use serde_json::json;
use sqlx::{types::Json as SqlJson, Row};
use uuid::Uuid;
use vpsman_common::{
    validate_template, WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED, WEBHOOK_RULE_DELIVERY_STATUS_FAILED,
    WEBHOOK_RULE_DELIVERY_STATUS_MATCHED_DRY_RUN, WEBHOOK_RULE_DELIVERY_STATUS_PERMANENTLY_FAILED,
    WEBHOOK_RULE_DELIVERY_STATUS_QUEUED,
};

use crate::{
    model::{AgentView, AuditLogView, AuthContext},
    model_webhook_rules::{
        CreateWebhookRuleRequest, WebhookDeliveryRotationRequest, WebhookDeliveryRotationResponse,
        WebhookEventCandidate, WebhookEventRow, WebhookRuleDeliveryCandidate,
        WebhookRuleDeliveryView, WebhookRuleView,
    },
    repository::Repository,
    selector_expression::parse_selector_expression,
    unix_now,
};

const DEFAULT_COOLDOWN_SECS: i64 = 300;
const MAX_COOLDOWN_SECS: i64 = 30 * 24 * 60 * 60;
const MAX_NAME_BYTES: usize = 128;
const MAX_EXPRESSION_BYTES: usize = 4096;
const MAX_TARGET_BYTES: usize = 512;
const MAX_TEMPLATE_BYTES: usize = 4096;
const MAX_NOTES_BYTES: usize = 1024;

impl Repository {
    pub(crate) async fn list_webhook_rules(
        &self,
        limit: i64,
        enabled: Option<bool>,
    ) -> Result<Vec<WebhookRuleView>> {
        match self {
            Self::Memory(memory) => {
                let mut rows = memory
                    .webhook_rules
                    .read()
                    .await
                    .iter()
                    .filter(|rule| enabled.is_none_or(|value| rule.enabled == value))
                    .cloned()
                    .collect::<Vec<_>>();
                rows.sort_by(|left, right| {
                    right.enabled.cmp(&left.enabled).then_with(|| {
                        left.name
                            .to_ascii_lowercase()
                            .cmp(&right.name.to_ascii_lowercase())
                    })
                });
                rows.truncate(limit.clamp(1, 1000) as usize);
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        name,
                        enabled,
                        expression,
                        target,
                        body_template,
                        cooldown_secs,
                        notes,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM webhook_rules
                    WHERE ($2::boolean IS NULL OR enabled = $2)
                    ORDER BY enabled DESC, name ASC, id ASC
                    LIMIT $1
                    "#,
                )
                .bind(limit.clamp(1, 1000))
                .bind(enabled)
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(webhook_rule_from_row).collect()
            }
        }
    }

    pub(crate) async fn upsert_webhook_rule(
        &self,
        request: &CreateWebhookRuleRequest,
        operator: &AuthContext,
    ) -> Result<WebhookRuleView> {
        let candidate = webhook_rule_from_request(request, operator)?;
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut rules = memory.webhook_rules.write().await;
                anyhow::ensure!(
                    !rules.iter().any(|stored| {
                        stored.name == candidate.name && Some(stored.id) != request.id
                    }),
                    "webhook_rule_name_conflict"
                );
                let rule = if let Some(stored) = rules
                    .iter_mut()
                    .find(|stored| request.id.is_some_and(|id| stored.id == id))
                {
                    stored.name = candidate.name.clone();
                    stored.enabled = candidate.enabled;
                    stored.expression = candidate.expression.clone();
                    stored.target = candidate.target.clone();
                    stored.body_template = candidate.body_template.clone();
                    stored.cooldown_secs = candidate.cooldown_secs;
                    stored.notes = candidate.notes.clone();
                    stored.actor_id = candidate.actor_id;
                    stored.updated_at = now.clone();
                    stored.clone()
                } else {
                    rules.push(candidate.clone());
                    candidate
                };
                drop(rules);
                memory
                    .audits
                    .write()
                    .await
                    .push(webhook_rule_audit(&rule, operator, now));
                Ok(rule)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    INSERT INTO webhook_rules (
                        id,
                        name,
                        enabled,
                        expression,
                        target,
                        body_template,
                        cooldown_secs,
                        notes,
                        actor_id
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                    ON CONFLICT (id) DO UPDATE SET
                        name = EXCLUDED.name,
                        enabled = EXCLUDED.enabled,
                        expression = EXCLUDED.expression,
                        target = EXCLUDED.target,
                        body_template = EXCLUDED.body_template,
                        cooldown_secs = EXCLUDED.cooldown_secs,
                        notes = EXCLUDED.notes,
                        actor_id = EXCLUDED.actor_id,
                        updated_at = now()
                    RETURNING
                        id,
                        name,
                        enabled,
                        expression,
                        target,
                        body_template,
                        cooldown_secs,
                        notes,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    "#,
                )
                .bind(candidate.id)
                .bind(&candidate.name)
                .bind(candidate.enabled)
                .bind(&candidate.expression)
                .bind(&candidate.target)
                .bind(&candidate.body_template)
                .bind(candidate.cooldown_secs)
                .bind(&candidate.notes)
                .bind(operator.operator.id)
                .fetch_one(&mut *tx)
                .await?;
                let rule = webhook_rule_from_row(row)?;
                insert_webhook_rule_audit(&mut tx, &rule, operator).await?;
                tx.commit().await?;
                Ok(rule)
            }
        }
    }

    pub(crate) async fn delete_webhook_rule(
        &self,
        rule_id: Uuid,
        operator: &AuthContext,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let mut rules = memory.webhook_rules.write().await;
                let position = rules
                    .iter()
                    .position(|rule| rule.id == rule_id)
                    .ok_or_else(|| anyhow::anyhow!("webhook_rule_not_found:{rule_id}"))?;
                let rule = rules.remove(position);
                drop(rules);
                memory
                    .audits
                    .write()
                    .await
                    .push(webhook_rule_audit_with_action(
                        &rule,
                        operator,
                        unix_now().to_string(),
                        "webhook_rule.deleted",
                    ));
                Ok(())
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    DELETE FROM webhook_rules
                    WHERE id = $1
                    RETURNING
                        id,
                        name,
                        enabled,
                        expression,
                        target,
                        body_template,
                        cooldown_secs,
                        notes,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    "#,
                )
                .bind(rule_id)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(row) = row else {
                    anyhow::bail!("webhook_rule_not_found:{rule_id}");
                };
                let rule = webhook_rule_from_row(row)?;
                insert_webhook_rule_audit_with_action(
                    &mut tx,
                    &rule,
                    operator,
                    "webhook_rule.deleted",
                )
                .await?;
                tx.commit().await?;
                Ok(())
            }
        }
    }

    pub(crate) async fn list_webhook_rule_deliveries(
        &self,
        limit: i64,
        rule_id: Option<Uuid>,
        event_kind: Option<&str>,
        status: Option<&str>,
    ) -> Result<Vec<WebhookRuleDeliveryView>> {
        let event_kind = normalize_optional_filter(event_kind);
        let status = normalize_optional_status(status)?;
        match self {
            Self::Memory(memory) => {
                let mut rows = memory
                    .webhook_rule_deliveries
                    .read()
                    .await
                    .iter()
                    .filter(|delivery| rule_id.is_none_or(|value| delivery.rule_id == value))
                    .filter(|delivery| {
                        event_kind
                            .as_deref()
                            .is_none_or(|value| delivery.event_kind == value)
                    })
                    .filter(|delivery| {
                        status
                            .as_deref()
                            .is_none_or(|value| delivery.status == value)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                rows.sort_by(|left, right| right.created_at.cmp(&left.created_at));
                rows.truncate(limit.clamp(1, 1000) as usize);
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        rule_id,
                        rule_name,
                        event_kind,
                        event_id,
                        status,
                        target,
                        dedupe_key,
                        payload,
                        matched_vps,
                        message,
                        error,
                        cooldown_until_unix,
                        attempt_count,
                        next_attempt_at::text AS next_attempt_at,
                        last_attempt_at::text AS last_attempt_at,
                        actor_id,
                        created_at::text AS created_at,
                        delivered_at::text AS delivered_at
                    FROM webhook_rule_deliveries
                    WHERE ($2::uuid IS NULL OR rule_id = $2)
                      AND ($3::text IS NULL OR event_kind = $3)
                      AND ($4::text IS NULL OR status = $4)
                    ORDER BY created_at DESC, id DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit.clamp(1, 1000))
                .bind(rule_id)
                .bind(event_kind.as_deref())
                .bind(status.as_deref())
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(webhook_delivery_from_row).collect()
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) async fn record_webhook_rule_deliveries(
        &self,
        candidates: &[WebhookRuleDeliveryCandidate],
    ) -> Result<Vec<WebhookRuleDeliveryView>> {
        match self {
            Self::Memory(memory) => {
                let mut persisted = Vec::new();
                let mut deliveries = memory.webhook_rule_deliveries.write().await;
                for candidate in candidates {
                    if deliveries.iter().any(|stored| {
                        stored.rule_id == candidate.rule_id && stored.event_id == candidate.event_id
                    }) {
                        continue;
                    }
                    let delivery = webhook_delivery_from_candidate(
                        candidate,
                        WEBHOOK_RULE_DELIVERY_STATUS_QUEUED,
                    );
                    deliveries.push(delivery.clone());
                    persisted.push(delivery);
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
                        FROM webhook_rule_deliveries
                        WHERE rule_id = $1
                          AND event_id = $2
                        LIMIT 1
                        "#,
                    )
                    .bind(candidate.rule_id)
                    .bind(&candidate.event_id)
                    .fetch_optional(&mut *tx)
                    .await?
                    .is_some();
                    if duplicate {
                        continue;
                    }
                    let delivery = webhook_delivery_from_candidate(
                        candidate,
                        WEBHOOK_RULE_DELIVERY_STATUS_QUEUED,
                    );
                    let row = insert_delivery_query(&delivery).fetch_one(&mut *tx).await?;
                    persisted.push(webhook_delivery_from_row(row)?);
                }
                if !persisted.is_empty() {
                    insert_webhook_dispatch_audit(&mut tx, &persisted).await?;
                }
                tx.commit().await?;
                Ok(persisted)
            }
        }
    }

    pub(crate) async fn record_webhook_event(
        &self,
        event: WebhookEventCandidate,
    ) -> Result<WebhookEventRow> {
        let occurred_at = Utc::now();
        let row = WebhookEventRow {
            id: Uuid::new_v4(),
            kind: event.kind.trim().to_string(),
            event_id: event.event_id.trim().to_string(),
            event_predicates: normalize_event_predicates(&event.kind, &event.event_predicates),
            subject_client_ids: normalize_subject_client_ids(&event.subject_client_ids),
            payload: event.payload,
            occurred_at: occurred_at.to_rfc3339(),
            actor_id: event.actor_id,
        };
        anyhow::ensure!(!row.kind.is_empty(), "webhook event kind is required");
        anyhow::ensure!(!row.event_id.is_empty(), "webhook event id is required");
        match self {
            Self::Memory(memory) => {
                let mut events = memory.webhook_events.write().await;
                if let Some(stored) = events
                    .iter()
                    .find(|stored| stored.kind == row.kind && stored.event_id == row.event_id)
                    .cloned()
                {
                    return Ok(stored);
                }
                events.push(row.clone());
                Ok(row)
            }
            Self::Postgres(pool) => {
                ensure_webhook_event_partition(pool, occurred_at).await?;
                let payload = SqlJson(&row.payload);
                let inserted = sqlx::query(
                    r#"
                    INSERT INTO webhook_events (
                        id,
                        kind,
                        event_id,
                        event_predicates,
                        subject_client_ids,
                        payload,
                        occurred_at,
                        actor_id
                    )
                    SELECT $1, $2, $3, $4, $5, $6, $7::timestamptz, $8
                    WHERE NOT EXISTS (
                        SELECT 1
                        FROM webhook_events
                        WHERE kind = $2 AND event_id = $3
                    )
                    "#,
                )
                .bind(row.id)
                .bind(&row.kind)
                .bind(&row.event_id)
                .bind(&row.event_predicates)
                .bind(&row.subject_client_ids)
                .bind(payload)
                .bind(occurred_at.to_rfc3339())
                .bind(row.actor_id)
                .execute(pool)
                .await?;
                if inserted.rows_affected() > 0 {
                    let _ = sqlx::query("SELECT pg_notify('webhook_events', $1)")
                        .bind(row.event_id.clone())
                        .execute(pool)
                        .await?;
                    Ok(row)
                } else {
                    let stored = sqlx::query(
                        r#"
                        SELECT
                            id,
                            kind,
                            event_id,
                            event_predicates,
                            subject_client_ids,
                            payload,
                            occurred_at::text AS occurred_at,
                            actor_id
                        FROM webhook_events
                        WHERE kind = $1 AND event_id = $2
                        ORDER BY occurred_at DESC
                        LIMIT 1
                        "#,
                    )
                    .bind(&row.kind)
                    .bind(&row.event_id)
                    .fetch_one(pool)
                    .await?;
                    webhook_event_from_row(stored)
                }
            }
        }
    }

    pub(crate) async fn rotate_webhook_delivery_history(
        &self,
        request: &WebhookDeliveryRotationRequest,
    ) -> Result<WebhookDeliveryRotationResponse> {
        let older_than = rotation_older_than(request)?;
        let status = normalize_optional_status(request.status.as_deref())?;
        match self {
            Self::Memory(memory) => {
                let mut deliveries = memory.webhook_rule_deliveries.write().await;
                let matched = deliveries
                    .iter()
                    .filter(|delivery| {
                        rotation_delivery_matches(
                            delivery,
                            older_than,
                            status.as_deref(),
                            request.rule_id,
                        )
                    })
                    .count();
                let deleted = if request.confirmed {
                    let before = deliveries.len();
                    deliveries.retain(|delivery| {
                        !rotation_delivery_matches(
                            delivery,
                            older_than,
                            status.as_deref(),
                            request.rule_id,
                        )
                    });
                    before.saturating_sub(deliveries.len())
                } else {
                    0
                };
                Ok(WebhookDeliveryRotationResponse {
                    matched_count: matched,
                    deleted_count: deleted,
                    confirmation_required: !request.confirmed,
                    older_than: older_than.map(|value| value.to_rfc3339()),
                    status,
                    rule_id: request.rule_id,
                })
            }
            Self::Postgres(pool) => {
                let matched = sqlx::query_scalar::<_, i64>(
                    r#"
                    SELECT count(*)::bigint
                    FROM webhook_rule_deliveries
                    WHERE ($1::text IS NULL OR created_at < $1::timestamptz)
                      AND ($2::text IS NULL OR status = $2)
                      AND ($3::uuid IS NULL OR rule_id = $3)
                    "#,
                )
                .bind(older_than.as_ref().map(DateTime::<Utc>::to_rfc3339))
                .bind(status.as_deref())
                .bind(request.rule_id)
                .fetch_one(pool)
                .await? as usize;
                let deleted = if request.confirmed {
                    sqlx::query(
                        r#"
                        DELETE FROM webhook_rule_deliveries
                        WHERE ($1::text IS NULL OR created_at < $1::timestamptz)
                          AND ($2::text IS NULL OR status = $2)
                          AND ($3::uuid IS NULL OR rule_id = $3)
                        "#,
                    )
                    .bind(older_than.as_ref().map(DateTime::<Utc>::to_rfc3339))
                    .bind(status.as_deref())
                    .bind(request.rule_id)
                    .execute(pool)
                    .await?
                    .rows_affected() as usize
                } else {
                    0
                };
                Ok(WebhookDeliveryRotationResponse {
                    matched_count: matched,
                    deleted_count: deleted,
                    confirmation_required: !request.confirmed,
                    older_than: older_than.map(|value| value.to_rfc3339()),
                    status,
                    rule_id: request.rule_id,
                })
            }
        }
    }

    pub(crate) async fn update_webhook_rule_delivery_attempt(
        &self,
        delivery_id: Uuid,
        status: &str,
        error: Option<&str>,
    ) -> Result<WebhookRuleDeliveryView> {
        let status = normalize_delivery_attempt_status(status)?;
        let error = error
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.chars().take(MAX_NOTES_BYTES).collect::<String>());
        let now = unix_now().to_string();
        match self {
            Self::Memory(memory) => {
                let mut deliveries = memory.webhook_rule_deliveries.write().await;
                let delivery = deliveries
                    .iter_mut()
                    .find(|delivery| delivery.id == delivery_id)
                    .context("webhook rule delivery not found")?;
                anyhow::ensure!(
                    matches!(
                        delivery.status.as_str(),
                        WEBHOOK_RULE_DELIVERY_STATUS_QUEUED | WEBHOOK_RULE_DELIVERY_STATUS_FAILED
                    ),
                    "webhook rule delivery is not retryable"
                );
                delivery.status = status.to_string();
                delivery.error = error;
                delivery.attempt_count = delivery.attempt_count.saturating_add(1);
                delivery.last_attempt_at = Some(now.clone());
                delivery.delivered_at =
                    (status == WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED).then_some(now);
                Ok(delivery.clone())
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    UPDATE webhook_rule_deliveries
                    SET
                        status = $2,
                        error = $3,
                        attempt_count = attempt_count + 1,
                        last_attempt_at = now(),
                        delivered_at = CASE WHEN $2 = 'delivered' THEN now() ELSE delivered_at END
                    WHERE id = $1
                      AND status IN ('queued', 'failed')
                    RETURNING
                        id,
                        rule_id,
                        rule_name,
                        event_kind,
                        event_id,
                        status,
                        target,
                        dedupe_key,
                        payload,
                        matched_vps,
                        message,
                        error,
                        cooldown_until_unix,
                        attempt_count,
                        next_attempt_at::text AS next_attempt_at,
                        last_attempt_at::text AS last_attempt_at,
                        actor_id,
                        created_at::text AS created_at,
                        delivered_at::text AS delivered_at
                    "#,
                )
                .bind(delivery_id)
                .bind(status)
                .bind(error.as_deref())
                .fetch_optional(pool)
                .await?
                .context("webhook rule delivery not found or not retryable")?;
                webhook_delivery_from_row(row)
            }
        }
    }

    pub(crate) async fn record_webhook_rule_process_audit(
        &self,
        deliveries: &[WebhookRuleDeliveryView],
        operator: &AuthContext,
    ) -> Result<()> {
        if deliveries.is_empty() {
            return Ok(());
        }
        let metadata = json!({
            "delivery_count": deliveries.len(),
            "delivered_count": deliveries.iter().filter(|delivery| delivery.status == WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED).count(),
            "failed_count": deliveries.iter().filter(|delivery| delivery.status == WEBHOOK_RULE_DELIVERY_STATUS_FAILED).count(),
            "deliveries": deliveries.iter().take(100).map(|delivery| json!({
                "id": delivery.id,
                "rule_id": delivery.rule_id,
                "rule_name": &delivery.rule_name,
                "event_kind": &delivery.event_kind,
                "event_id": &delivery.event_id,
                "status": &delivery.status,
                "attempt_count": delivery.attempt_count,
                "error": &delivery.error,
            })).collect::<Vec<_>>(),
        });
        match self {
            Self::Memory(memory) => {
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "webhook.rule_deliveries_processed".to_string(),
                    target: "webhook_rules".to_string(),
                    command_hash: None,
                    metadata,
                    created_at: unix_now().to_string(),
                });
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
                .bind("webhook.rule_deliveries_processed")
                .bind("webhook_rules")
                .bind(metadata)
                .execute(pool)
                .await?;
                Ok(())
            }
        }
    }
}

pub(crate) fn webhook_rule_from_request(
    request: &CreateWebhookRuleRequest,
    operator: &AuthContext,
) -> Result<WebhookRuleView> {
    anyhow::ensure!(request.confirmed, "webhook_rule_confirmation_required");
    validate_required_text(&request.name, MAX_NAME_BYTES, "webhook rule name")?;
    validate_required_text(
        &request.expression,
        MAX_EXPRESSION_BYTES,
        "webhook rule expression",
    )?;
    parse_selector_expression(&request.expression)
        .map_err(|error| anyhow::anyhow!("invalid webhook rule expression: {error}"))?
        .context("webhook rule expression is empty")?;
    validate_webhook_url(&request.target)?;
    anyhow::ensure!(
        request.body_template.len() <= MAX_TEMPLATE_BYTES,
        "webhook rule body template is too long"
    );
    if !request.body_template.trim().is_empty() {
        validate_template(&request.body_template)
            .map_err(|error| anyhow::anyhow!("invalid webhook rule template: {error}"))?;
    }
    let cooldown_secs = request.cooldown_secs.unwrap_or(DEFAULT_COOLDOWN_SECS);
    anyhow::ensure!(
        (0..=MAX_COOLDOWN_SECS).contains(&cooldown_secs),
        "webhook rule cooldown is invalid"
    );
    validate_optional_text(
        request.notes.as_deref(),
        MAX_NOTES_BYTES,
        "webhook rule notes",
    )?;
    Ok(WebhookRuleView {
        id: request.id.unwrap_or_else(Uuid::new_v4),
        name: request.name.trim().to_string(),
        enabled: request.enabled,
        expression: request.expression.trim().to_string(),
        target: request.target.trim().to_string(),
        body_template: request.body_template.trim().to_string(),
        cooldown_secs,
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

pub(crate) fn dry_run_webhook_delivery(
    candidate: &WebhookRuleDeliveryCandidate,
) -> WebhookRuleDeliveryView {
    webhook_delivery_from_candidate(candidate, WEBHOOK_RULE_DELIVERY_STATUS_MATCHED_DRY_RUN)
}

pub(crate) fn validate_webhook_rule_target(target: &str) -> Result<()> {
    validate_webhook_url(target).map(|_| ())
}

#[allow(dead_code)]
fn insert_delivery_query(
    delivery: &WebhookRuleDeliveryView,
) -> sqlx::query::Query<'_, sqlx::Postgres, sqlx::postgres::PgArguments> {
    sqlx::query(
        r#"
        INSERT INTO webhook_rule_deliveries (
            id,
            rule_id,
            rule_name,
            event_kind,
            event_id,
            status,
            target,
            dedupe_key,
            payload,
            matched_vps,
            message,
            error,
            cooldown_until_unix,
            attempt_count,
            next_attempt_at,
            last_attempt_at,
            actor_id,
            delivered_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, NULL, NULL, $15, CASE WHEN $6 = 'delivered' THEN now() ELSE NULL END)
        RETURNING
            id,
            rule_id,
            rule_name,
            event_kind,
            event_id,
            status,
            target,
            dedupe_key,
            payload,
            matched_vps,
            message,
            error,
            cooldown_until_unix,
            attempt_count,
            next_attempt_at::text AS next_attempt_at,
            last_attempt_at::text AS last_attempt_at,
            actor_id,
            created_at::text AS created_at,
            delivered_at::text AS delivered_at
        "#,
    )
    .bind(delivery.id)
    .bind(delivery.rule_id)
    .bind(&delivery.rule_name)
    .bind(&delivery.event_kind)
    .bind(&delivery.event_id)
    .bind(&delivery.status)
    .bind(&delivery.target)
    .bind(&delivery.dedupe_key)
    .bind(SqlJson(&delivery.payload))
    .bind(SqlJson(&delivery.matched_vps))
    .bind(&delivery.message)
    .bind(&delivery.error)
    .bind(delivery.cooldown_until_unix)
    .bind(delivery.attempt_count)
    .bind(delivery.actor_id)
}

fn webhook_rule_from_row(row: sqlx::postgres::PgRow) -> Result<WebhookRuleView> {
    Ok(WebhookRuleView {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        enabled: row.try_get("enabled")?,
        expression: row.try_get("expression")?,
        target: row.try_get("target")?,
        body_template: row.try_get("body_template")?,
        cooldown_secs: row.try_get("cooldown_secs")?,
        notes: row.try_get("notes")?,
        actor_id: row.try_get("actor_id")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn webhook_delivery_from_row(row: sqlx::postgres::PgRow) -> Result<WebhookRuleDeliveryView> {
    let payload: SqlJson<serde_json::Value> = row.try_get("payload")?;
    let matched_vps: SqlJson<Vec<AgentView>> = row.try_get("matched_vps")?;
    Ok(WebhookRuleDeliveryView {
        id: row.try_get("id")?,
        rule_id: row.try_get("rule_id")?,
        rule_name: row.try_get("rule_name")?,
        event_kind: row.try_get("event_kind")?,
        event_id: row.try_get("event_id")?,
        status: row.try_get("status")?,
        target: row.try_get("target")?,
        dedupe_key: row.try_get("dedupe_key")?,
        payload: payload.0,
        matched_vps: matched_vps.0,
        message: row.try_get("message")?,
        error: row.try_get("error")?,
        cooldown_until_unix: row.try_get("cooldown_until_unix")?,
        attempt_count: row.try_get("attempt_count")?,
        next_attempt_at: row.try_get("next_attempt_at")?,
        last_attempt_at: row.try_get("last_attempt_at")?,
        actor_id: row.try_get("actor_id")?,
        created_at: row.try_get("created_at")?,
        delivered_at: row.try_get("delivered_at")?,
    })
}

fn webhook_delivery_from_candidate(
    candidate: &WebhookRuleDeliveryCandidate,
    status: &str,
) -> WebhookRuleDeliveryView {
    WebhookRuleDeliveryView {
        id: Uuid::new_v4(),
        rule_id: candidate.rule_id,
        rule_name: candidate.rule_name.clone(),
        event_kind: candidate.event_kind.clone(),
        event_id: candidate.event_id.clone(),
        status: status.to_string(),
        target: candidate.target.clone(),
        dedupe_key: candidate.dedupe_key.clone(),
        payload: candidate.payload.clone(),
        matched_vps: candidate.matched_vps.clone(),
        message: candidate.message.clone(),
        error: None,
        cooldown_until_unix: candidate.cooldown_until_unix,
        attempt_count: 0,
        next_attempt_at: None,
        last_attempt_at: None,
        actor_id: candidate.actor_id,
        created_at: unix_now().to_string(),
        delivered_at: (status == WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED)
            .then(|| unix_now().to_string()),
    }
}

async fn insert_webhook_rule_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &WebhookRuleView,
    operator: &AuthContext,
) -> Result<()> {
    insert_webhook_rule_audit_with_action(tx, rule, operator, "webhook.rule_upserted").await
}

async fn insert_webhook_rule_audit_with_action(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &WebhookRuleView,
    operator: &AuthContext,
    action: &str,
) -> Result<()> {
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
    .bind(action)
    .bind(format!("webhook_rule:{}", rule.id))
    .bind(webhook_rule_metadata(rule, operator))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[allow(dead_code)]
async fn insert_webhook_dispatch_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    deliveries: &[WebhookRuleDeliveryView],
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
    .bind("webhook.rule_deliveries_queued")
    .bind("webhook_rules")
    .bind(json!({
        "delivery_count": deliveries.len(),
        "deliveries": deliveries.iter().take(100).map(|delivery| json!({
            "id": delivery.id,
            "rule_id": delivery.rule_id,
            "rule_name": &delivery.rule_name,
            "event_kind": &delivery.event_kind,
            "event_id": &delivery.event_id,
            "matched_vps_count": delivery.matched_vps.len(),
        })).collect::<Vec<_>>(),
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn webhook_rule_audit(
    rule: &WebhookRuleView,
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    webhook_rule_audit_with_action(rule, operator, created_at, "webhook.rule_upserted")
}

fn webhook_rule_audit_with_action(
    rule: &WebhookRuleView,
    operator: &AuthContext,
    created_at: String,
    action: &str,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: action.to_string(),
        target: format!("webhook_rule:{}", rule.id),
        command_hash: None,
        metadata: webhook_rule_metadata(rule, operator),
        created_at,
    }
}

fn webhook_rule_metadata(rule: &WebhookRuleView, operator: &AuthContext) -> serde_json::Value {
    json!({
        "rule_id": rule.id,
        "name": &rule.name,
        "enabled": rule.enabled,
        "expression": &rule.expression,
        "target": &rule.target,
        "cooldown_secs": rule.cooldown_secs,
        "operator": {
            "id": operator.operator.id,
            "username": &operator.operator.username,
        },
    })
}

fn validate_webhook_url(target: &str) -> Result<Url> {
    validate_required_text(target, MAX_TARGET_BYTES, "webhook target")?;
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

fn validate_required_text(value: &str, max_bytes: usize, label: &str) -> Result<()> {
    let value = value.trim();
    anyhow::ensure!(
        !value.is_empty() && value.len() <= max_bytes && !value.as_bytes().contains(&0),
        "{label} is invalid"
    );
    Ok(())
}

fn validate_optional_text(value: Option<&str>, max_bytes: usize, label: &str) -> Result<()> {
    if let Some(value) = value {
        anyhow::ensure!(
            value.len() <= max_bytes && !value.as_bytes().contains(&0),
            "{label} is invalid"
        );
    }
    Ok(())
}

pub(crate) async fn ensure_webhook_event_partition(
    pool: &sqlx::PgPool,
    timestamp: DateTime<Utc>,
) -> Result<()> {
    let date = timestamp.date_naive();
    let next = date
        .succ_opt()
        .context("failed to calculate webhook event partition date")?;
    let table_name = format!("webhook_events_{}", date.format("%Y%m%d"));
    let sql = format!(
        r#"
        CREATE TABLE IF NOT EXISTS {table_name}
        PARTITION OF webhook_events
        FOR VALUES FROM ('{date}') TO ('{next}')
        "#
    );
    sqlx::query(&sql).execute(pool).await?;
    Ok(())
}

fn webhook_event_from_row(row: sqlx::postgres::PgRow) -> Result<WebhookEventRow> {
    let payload: SqlJson<serde_json::Value> = row.try_get("payload")?;
    Ok(WebhookEventRow {
        id: row.try_get("id")?,
        kind: row.try_get("kind")?,
        event_id: row.try_get("event_id")?,
        event_predicates: row.try_get("event_predicates")?,
        subject_client_ids: row.try_get("subject_client_ids")?,
        payload: payload.0,
        occurred_at: row.try_get("occurred_at")?,
        actor_id: row.try_get("actor_id")?,
    })
}

fn normalize_event_predicates(kind: &str, predicates: &[String]) -> Vec<String> {
    let mut values = predicates
        .iter()
        .map(|predicate| predicate.trim().to_ascii_lowercase())
        .filter(|predicate| !predicate.is_empty())
        .collect::<Vec<_>>();
    let kind = kind.trim().to_ascii_lowercase();
    if !kind.is_empty() {
        values.push(kind);
    }
    values.sort();
    values.dedup();
    values
}

fn normalize_subject_client_ids(subject_client_ids: &[String]) -> Vec<String> {
    let mut values = subject_client_ids
        .iter()
        .map(|client_id| client_id.trim().to_string())
        .filter(|client_id| !client_id.is_empty())
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

fn rotation_older_than(request: &WebhookDeliveryRotationRequest) -> Result<Option<DateTime<Utc>>> {
    if let Some(value) = request.older_than.as_deref() {
        let parsed = DateTime::parse_from_rfc3339(value)
            .context("webhook delivery rotation older_than is invalid")?
            .with_timezone(&Utc);
        return Ok(Some(parsed));
    }
    if let Some(days) = request.older_than_days {
        anyhow::ensure!(
            (1..=3650).contains(&days),
            "webhook delivery rotation older_than_days is invalid"
        );
        return Ok(Some(Utc::now() - Duration::days(days)));
    }
    Ok(None)
}

fn rotation_delivery_matches(
    delivery: &WebhookRuleDeliveryView,
    older_than: Option<DateTime<Utc>>,
    status: Option<&str>,
    rule_id: Option<Uuid>,
) -> bool {
    if let Some(rule_id) = rule_id {
        if delivery.rule_id != rule_id {
            return false;
        }
    }
    if let Some(status) = status {
        if delivery.status != status {
            return false;
        }
    }
    if let Some(older_than) = older_than {
        let Ok(created_at) = DateTime::parse_from_rfc3339(&delivery.created_at) else {
            return false;
        };
        if created_at.with_timezone(&Utc) >= older_than {
            return false;
        }
    }
    true
}

fn normalize_optional_filter(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn normalize_optional_status(status: Option<&str>) -> Result<Option<String>> {
    status
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            anyhow::ensure!(
                matches!(
                    value,
                    WEBHOOK_RULE_DELIVERY_STATUS_QUEUED
                        | WEBHOOK_RULE_DELIVERY_STATUS_FAILED
                        | WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED
                        | WEBHOOK_RULE_DELIVERY_STATUS_PERMANENTLY_FAILED
                ),
                "webhook rule delivery status is invalid"
            );
            Ok(value.to_string())
        })
        .transpose()
}

fn normalize_delivery_attempt_status(status: &str) -> Result<&'static str> {
    match status.trim() {
        WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED => Ok(WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED),
        WEBHOOK_RULE_DELIVERY_STATUS_FAILED => Ok(WEBHOOK_RULE_DELIVERY_STATUS_FAILED),
        WEBHOOK_RULE_DELIVERY_STATUS_PERMANENTLY_FAILED => {
            Ok(WEBHOOK_RULE_DELIVERY_STATUS_PERMANENTLY_FAILED)
        }
        _ => anyhow::bail!("webhook rule delivery attempt status is invalid"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{OperatorPreferences, OperatorView};

    fn operator() -> AuthContext {
        AuthContext {
            operator: OperatorView {
                id: Uuid::nil(),
                username: "test".to_string(),
                role: "admin".to_string(),
                scopes: Vec::new(),
                preferences: OperatorPreferences::default(),
                totp_enabled: false,
                status: "active".to_string(),
                session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
                created_at: crate::unix_now().to_string(),
                disabled_at: None,
                deleted_at: None,
            },
            session_id: Uuid::nil(),
        }
    }

    #[test]
    fn webhook_url_policy_allows_https_and_local_http_only() {
        assert!(validate_webhook_rule_target("https://hooks.example/vpsman").is_ok());
        assert!(validate_webhook_rule_target("http://localhost:9000/hook").is_ok());
        assert!(validate_webhook_rule_target("http://127.0.0.1:9000/hook").is_ok());
        assert!(validate_webhook_rule_target("http://hooks.example/hook").is_err());
        assert!(validate_webhook_rule_target("https://user:secret@example.com/hook").is_err());
    }

    #[test]
    fn webhook_rule_request_validates_expression_and_target() {
        let mut request = CreateWebhookRuleRequest {
            id: None,
            name: "stale edge".to_string(),
            enabled: true,
            expression: "status = stale && tag:edge".to_string(),
            target: "https://hooks.example/vpsman".to_string(),
            body_template: "{vps.name} stale".to_string(),
            cooldown_secs: Some(60),
            notes: None,
            confirmed: true,
        };
        assert!(webhook_rule_from_request(&request, &operator()).is_ok());
        request.expression = "status in []".to_string();
        assert!(webhook_rule_from_request(&request, &operator()).is_err());
    }
}
