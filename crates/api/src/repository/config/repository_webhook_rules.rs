use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{types::Json as SqlJson, Executor, Postgres, QueryBuilder, Row, Transaction};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;
use vpsman_common::{
    expression_references_vps_rules, payload_hash, validate_template,
    WEBHOOK_RULE_DELIVERY_STATUS_CANCELED_DISABLED, WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED,
    WEBHOOK_RULE_DELIVERY_STATUS_FAILED, WEBHOOK_RULE_DELIVERY_STATUS_MATCHED_DRY_RUN,
    WEBHOOK_RULE_DELIVERY_STATUS_PERMANENTLY_FAILED, WEBHOOK_RULE_DELIVERY_STATUS_QUEUED,
};

use crate::{
    model::{AgentView, AuthContext},
    model_webhook_rules::{
        CreateWebhookRuleRequest, WebhookDeliveryRotationRequest, WebhookDeliveryRotationResponse,
        WebhookEventCandidate, WebhookEventRow, WebhookRuleBulkAction, WebhookRuleBulkOutcome,
        WebhookRuleBulkRequest, WebhookRuleBulkResponse, WebhookRuleDeliveryCandidate,
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
const MAX_SIGNING_SECRET_BYTES: usize = 1024;
const WEBHOOK_ROTATION_SCAN_BATCH_SIZE: i64 = 1_000;

pub(crate) struct WebhookRuleAlertSendEligibilityRevision {
    eligibility: WebhookRuleAlertSendEligibility,
    revision: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WebhookRuleAlertSendEligibility {
    Deliverable,
    ClientSuspended,
    RuleDisabled,
    InvalidClientScope,
    LeaseLost,
}

impl WebhookRuleAlertSendEligibilityRevision {
    pub(crate) fn is_deliverable(&self) -> bool {
        self.eligibility == WebhookRuleAlertSendEligibility::Deliverable
    }

    pub(crate) fn cancellation_reason(&self) -> Option<&'static str> {
        match self.eligibility {
            WebhookRuleAlertSendEligibility::ClientSuspended => Some("client_suspended"),
            WebhookRuleAlertSendEligibility::RuleDisabled => Some("webhook rule disabled"),
            WebhookRuleAlertSendEligibility::InvalidClientScope => {
                Some("client_alert_scope_invalid")
            }
            WebhookRuleAlertSendEligibility::Deliverable
            | WebhookRuleAlertSendEligibility::LeaseLost => None,
        }
    }

    pub(crate) fn revision(&self) -> Option<i64> {
        self.revision
    }
}

impl Repository {
    pub(crate) async fn list_webhook_rules(
        &self,
        limit: i64,
        enabled: Option<bool>,
    ) -> Result<Vec<WebhookRuleView>> {
        self.query_webhook_rules(Some(limit.clamp(1, 1000)), enabled)
            .await
    }

    pub(crate) async fn list_all_webhook_rules(&self) -> Result<Vec<WebhookRuleView>> {
        self.query_webhook_rules(None, None).await
    }

    async fn query_webhook_rules(
        &self,
        limit: Option<i64>,
        enabled: Option<bool>,
    ) -> Result<Vec<WebhookRuleView>> {
        match self {
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
                        signing_secret,
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
                .bind(limit)
                .bind(enabled)
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(webhook_rule_from_row).collect()
            }
        }
    }

    pub(crate) async fn webhook_rule_by_id(
        &self,
        rule_id: Uuid,
    ) -> Result<Option<WebhookRuleView>> {
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        id,
                        name,
                        enabled,
                        expression,
                        target,
                        body_template,
                        signing_secret,
                        cooldown_secs,
                        notes,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM webhook_rules
                    WHERE id = $1
                    "#,
                )
                .bind(rule_id)
                .fetch_optional(pool)
                .await?;
                row.map(webhook_rule_from_row).transpose()
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
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                if request.id.is_none() {
                    sqlx::query(
                        r#"
                        SELECT pg_advisory_xact_lock(hashtextextended(
                            'vpsman:webhook-rule-name:' || $1::text,
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
                            enabled,
                            expression,
                            target,
                            body_template,
                            signing_secret,
                            cooldown_secs,
                            notes,
                            actor_id,
                            created_at::text AS created_at,
                            updated_at::text AS updated_at
                        FROM webhook_rules
                        WHERE name = $1
                        FOR UPDATE
                        "#,
                    )
                    .bind(&candidate.name)
                    .fetch_optional(&mut *tx)
                    .await?
                    .map(webhook_rule_from_row)
                    .transpose()?;
                    if let Some(existing) = existing {
                        anyhow::ensure!(
                            webhook_rule_material_matches(&existing, &candidate),
                            "webhook_rule_name_conflict"
                        );
                        tx.commit().await?;
                        return Ok(existing);
                    }
                }
                let row = sqlx::query(
                    r#"
                    INSERT INTO webhook_rules (
                        id,
                        name,
                        enabled,
                        expression,
                        target,
                        body_template,
                        signing_secret,
                        cooldown_secs,
                        notes,
                        actor_id
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                    ON CONFLICT (id) DO UPDATE SET
                        name = EXCLUDED.name,
                        enabled = EXCLUDED.enabled,
                        expression = EXCLUDED.expression,
                        target = EXCLUDED.target,
                        body_template = EXCLUDED.body_template,
                        signing_secret = CASE
                            WHEN $11 THEN NULL
                            WHEN $7::text IS NOT NULL THEN EXCLUDED.signing_secret
                            ELSE webhook_rules.signing_secret
                        END,
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
                        signing_secret,
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
                .bind(&candidate.signing_secret)
                .bind(candidate.cooldown_secs)
                .bind(&candidate.notes)
                .bind(operator.operator.id)
                .bind(request.clear_signing_secret)
                .fetch_one(&mut *tx)
                .await
                .map_err(webhook_rule_database_error)?;
                let rule = webhook_rule_from_row(row)?;
                if !rule.enabled {
                    // The rule row is the durable source state. The worker is
                    // the sole delivery terminalizer and rechecks this state
                    // immediately before external I/O.
                    sqlx::query("SELECT pg_notify('webhook_events', 'webhook_rule_state')")
                        .execute(&mut *tx)
                        .await?;
                }
                insert_webhook_rule_audit(&mut tx, &rule, operator).await?;
                tx.commit().await?;
                Ok(rule)
            }
        }
    }

    pub(crate) async fn delete_webhook_rule(
        &self,
        rule_id: Uuid,
        reviewed_name: &str,
        operator: &AuthContext,
    ) -> Result<()> {
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let current_name = sqlx::query_scalar::<_, String>(
                    "SELECT name FROM webhook_rules WHERE id = $1 FOR UPDATE",
                )
                .bind(rule_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| anyhow::anyhow!("webhook_rule_not_found:{rule_id}"))?;
                anyhow::ensure!(
                    current_name == reviewed_name.trim(),
                    "webhook_rule_delete_review_stale"
                );
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
                        signing_secret,
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
                sqlx::query("SELECT pg_notify('webhook_events', 'webhook_rule_state')")
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                Ok(())
            }
        }
    }

    pub(crate) async fn bulk_mutate_webhook_rules(
        &self,
        request: &WebhookRuleBulkRequest,
        operator: &AuthContext,
        allow_vps_rule_selectors: bool,
    ) -> Result<WebhookRuleBulkResponse> {
        anyhow::ensure!(
            (1..=1_000).contains(&request.items.len()),
            "webhook_rule_bulk_items_invalid"
        );
        let requested_ids = request.items.iter().map(|item| item.id).collect::<Vec<_>>();
        let unique_ids = requested_ids.iter().copied().collect::<HashSet<_>>();
        anyhow::ensure!(
            unique_ids.len() == requested_ids.len(),
            "webhook_rule_bulk_duplicate_item"
        );

        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id, name, enabled, expression, target, body_template,
                        signing_secret, cooldown_secs, notes, actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM webhook_rules
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
                    .map(webhook_rule_from_row)
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .map(|rule| (rule.id, rule))
                    .collect::<HashMap<_, _>>();
                anyhow::ensure!(
                    current.len() == request.items.len(),
                    "webhook_rule_not_found"
                );
                let desired_enabled = match request.action {
                    WebhookRuleBulkAction::Enable => Some(true),
                    WebhookRuleBulkAction::Disable => Some(false),
                    WebhookRuleBulkAction::Delete => None,
                };
                for item in &request.items {
                    let rule = current.get(&item.id).context("webhook_rule_not_found")?;
                    anyhow::ensure!(
                        rule.name == item.reviewed_name.trim()
                            && rule.updated_at == item.expected_updated_at.trim(),
                        "webhook_rule_bulk_review_stale"
                    );
                    if let Some(enabled) = desired_enabled {
                        anyhow::ensure!(rule.enabled != enabled, "webhook_rule_bulk_state_stale");
                        let expression = parse_selector_expression(&rule.expression)
                            .map_err(|error| {
                                anyhow::anyhow!("invalid selector expression: {error}")
                            })?
                            .context("selector expression is empty")?;
                        anyhow::ensure!(
                            allow_vps_rule_selectors
                                || !expression_references_vps_rules(&expression),
                            "vps_rule_selector_scope_required"
                        );
                    }
                }

                let changed_rows = if let Some(enabled) = desired_enabled {
                    sqlx::query(
                        r#"
                        UPDATE webhook_rules
                        SET enabled = $2, actor_id = $3, updated_at = now()
                        WHERE id = ANY($1)
                        RETURNING
                            id, name, enabled, expression, target, body_template,
                            signing_secret, cooldown_secs, notes, actor_id,
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
                        DELETE FROM webhook_rules
                        WHERE id = ANY($1)
                        RETURNING
                            id, name, enabled, expression, target, body_template,
                            signing_secret, cooldown_secs, notes, actor_id,
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
                    .map(webhook_rule_from_row)
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .map(|rule| (rule.id, rule))
                    .collect();
                anyhow::ensure!(
                    current.len() == request.items.len(),
                    "webhook_rule_bulk_snapshot_stale"
                );

                let (audit_action, result) = match request.action {
                    WebhookRuleBulkAction::Enable => ("webhook.rule_upserted", "enabled"),
                    WebhookRuleBulkAction::Disable => ("webhook.rule_upserted", "disabled"),
                    WebhookRuleBulkAction::Delete => ("webhook_rule.deleted", "deleted"),
                };
                let ordered_rules = request
                    .items
                    .iter()
                    .map(|item| {
                        current
                            .get(&item.id)
                            .context("webhook_rule_bulk_snapshot_stale")
                    })
                    .collect::<Result<Vec<_>>>()?;
                insert_webhook_rule_audits_with_action(
                    &mut tx,
                    &ordered_rules,
                    operator,
                    audit_action,
                )
                .await?;
                let outcomes: Vec<WebhookRuleBulkOutcome> = ordered_rules
                    .into_iter()
                    .map(|rule| WebhookRuleBulkOutcome {
                        id: rule.id,
                        name: rule.name.clone(),
                        result: result.to_string(),
                        record: (request.action != WebhookRuleBulkAction::Delete)
                            .then(|| rule.clone()),
                    })
                    .collect();
                if request.action != WebhookRuleBulkAction::Enable {
                    // Source state changed once; the delivery worker remains the
                    // sole owner of leases and terminal delivery transitions.
                    sqlx::query("SELECT pg_notify('webhook_events', 'webhook_rule_state')")
                        .execute(&mut *tx)
                        .await?;
                }
                tx.commit().await?;
                Ok(WebhookRuleBulkResponse {
                    action: request.action,
                    outcomes,
                })
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

    pub(crate) async fn record_webhook_rule_deliveries(
        &self,
        candidates: &[WebhookRuleDeliveryCandidate],
    ) -> Result<Vec<WebhookRuleDeliveryView>> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let mut rule_ids = candidates
                    .iter()
                    .map(|candidate| candidate.rule_id)
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                rule_ids.sort_unstable();
                let current_rule_revisions = sqlx::query(
                    r#"
                    SELECT
                        id, name, enabled, expression, target, body_template,
                        signing_secret, cooldown_secs, notes, actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM webhook_rules
                    WHERE id = ANY($1::uuid[])
                    ORDER BY id
                    FOR UPDATE
                    "#,
                )
                .bind(&rule_ids)
                .fetch_all(&mut *tx)
                .await?
                .into_iter()
                .map(webhook_rule_from_row)
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .filter(|rule| rule.enabled)
                .map(|rule| Ok((rule.id, webhook_rule_revision_hash(&rule)?)))
                .collect::<Result<HashMap<_, _>>>()?;
                let mut persisted = Vec::new();
                for candidate in candidates {
                    if current_rule_revisions.get(&candidate.rule_id)
                        != Some(&candidate.rule_revision_hash)
                    {
                        continue;
                    }
                    let delivery = webhook_delivery_from_candidate(
                        candidate,
                        WEBHOOK_RULE_DELIVERY_STATUS_QUEUED,
                    );
                    if let Some(row) = insert_delivery_query(&delivery)
                        .fetch_optional(&mut *tx)
                        .await?
                    {
                        persisted.push(webhook_delivery_from_row(row)?);
                    }
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
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = record_webhook_event_in_tx(&mut tx, event, occurred_at).await?;
                tx.commit().await?;
                Ok(row)
            }
        }
    }

    pub(crate) async fn rotate_webhook_delivery_history(
        &self,
        request: &WebhookDeliveryRotationRequest,
    ) -> Result<WebhookDeliveryRotationResponse> {
        let older_than = rotation_older_than(request)?;
        let older_than_text = older_than.as_ref().map(DateTime::<Utc>::to_rfc3339);
        let status = normalize_optional_status(request.status.as_deref())?;
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let transaction_mode = if request.confirmed {
                    "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ"
                } else {
                    "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY"
                };
                sqlx::query(transaction_mode).execute(&mut *tx).await?;
                let (matched_count, preview_hash) = postgres_webhook_rotation_snapshot(
                    &mut tx,
                    older_than_text.as_deref(),
                    status.as_deref(),
                    request.rule_id,
                )
                .await?;
                if request.confirmed {
                    anyhow::ensure!(
                        request.preview_hash.as_deref() == Some(preview_hash.as_str()),
                        "webhook_delivery_rotation_preview_hash_mismatch"
                    );
                }
                let deleted = if request.confirmed {
                    let deleted = sqlx::query(
                        r#"
                        DELETE FROM webhook_rule_deliveries
                        WHERE ($1::text IS NULL OR created_at < $1::timestamptz)
                          AND ($2::text IS NULL OR status = $2)
                          AND ($3::uuid IS NULL OR rule_id = $3)
                        "#,
                    )
                    .bind(older_than_text.as_deref())
                    .bind(status.as_deref())
                    .bind(request.rule_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(webhook_rotation_confirmation_sql_error)?
                    .rows_affected();
                    let deleted = usize::try_from(deleted)
                        .context("webhook delivery rotation delete count is invalid")?;
                    anyhow::ensure!(
                        deleted == matched_count,
                        "webhook_delivery_rotation_changed_during_confirmation"
                    );
                    deleted
                } else {
                    0
                };
                tx.commit()
                    .await
                    .map_err(webhook_rotation_confirmation_sql_error)?;
                Ok(WebhookDeliveryRotationResponse {
                    matched_count,
                    deleted_count: deleted,
                    confirmation_required: !request.confirmed,
                    older_than: older_than_text,
                    status,
                    rule_id: request.rule_id,
                    preview_hash,
                })
            }
        }
    }

    pub(crate) async fn claim_webhook_rule_delivery_for_process(
        &self,
        delivery_id: Uuid,
        lease_id: Uuid,
        lease_secs: i64,
    ) -> Result<Option<WebhookRuleDeliveryView>> {
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    WITH claim AS (
                        SELECT delivery.id, rule.signing_secret
                        FROM webhook_rule_deliveries delivery
                        JOIN webhook_rules rule
                          ON rule.id = delivery.rule_id
                         AND rule.enabled = TRUE
                        WHERE delivery.id = $1
                          AND delivery.status IN ('queued', 'failed')
                          AND NOT (
                              delivery.event_kind='alert.triggered'
                              AND EXISTS (
                                  SELECT 1
                                  FROM jsonb_array_elements(delivery.matched_vps) matched
                                  JOIN clients subject ON subject.id=matched->>'id'
                                  WHERE subject.status='suspended'
                              )
                          )
                        FOR UPDATE OF delivery SKIP LOCKED
                    )
                    UPDATE webhook_rule_deliveries delivery
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
                        delivery.rule_id,
                        delivery.rule_name,
                        delivery.event_kind,
                        delivery.event_id,
                        delivery.status,
                        delivery.target,
                        delivery.dedupe_key,
                        delivery.payload,
                        delivery.matched_vps,
                        delivery.message,
                        claim.signing_secret,
                        delivery.error,
                        delivery.cooldown_until_unix,
                        delivery.attempt_count,
                        delivery.next_attempt_at::text AS next_attempt_at,
                        delivery.last_attempt_at::text AS last_attempt_at,
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
                row.map(webhook_delivery_from_row).transpose()
            }
        }
    }

    pub(crate) async fn webhook_rule_enabled(&self, rule_id: Uuid) -> Result<bool> {
        match self {
            Self::Postgres(pool) => {
                let enabled = sqlx::query_scalar::<_, bool>(
                    r#"
                    SELECT enabled
                    FROM webhook_rules
                    WHERE id = $1
                    "#,
                )
                .bind(rule_id)
                .fetch_optional(pool)
                .await?
                .unwrap_or(false);
                Ok(enabled)
            }
        }
    }

    pub(crate) async fn begin_webhook_rule_alert_send(
        &self,
        delivery_id: Uuid,
        lease_id: Uuid,
    ) -> Result<WebhookRuleAlertSendEligibilityRevision> {
        match self {
            Self::Postgres(pool) => {
                postgres_arm_webhook_rule_alert_send(pool, delivery_id, lease_id).await
            }
        }
    }

    pub(crate) async fn complete_webhook_rule_delivery_attempt(
        &self,
        delivery_id: Uuid,
        lease_id: Uuid,
        status: &str,
        error: Option<&str>,
        next_attempt_after_secs: Option<i64>,
        eligibility_revision: Option<i64>,
    ) -> Result<WebhookRuleDeliveryView> {
        let status = normalize_delivery_attempt_status(status)?;
        let error = error
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.chars().take(MAX_NOTES_BYTES).collect::<String>());
        match self {
            Self::Postgres(pool) => {
                postgres_complete_webhook_rule_delivery_attempt(
                    pool,
                    delivery_id,
                    lease_id,
                    status,
                    error.as_deref(),
                    next_attempt_after_secs,
                    eligibility_revision,
                )
                .await
            }
        }
    }

    pub(crate) async fn cancel_claimed_webhook_rule_delivery(
        &self,
        delivery_id: Uuid,
        lease_id: Uuid,
        error: &str,
    ) -> Result<WebhookRuleDeliveryView> {
        let error = error
            .trim()
            .chars()
            .take(MAX_NOTES_BYTES)
            .collect::<String>();
        match self {
            Self::Postgres(pool) => {
                postgres_cancel_claimed_webhook_rule_delivery(pool, delivery_id, lease_id, &error)
                    .await
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
        let delivered_count = deliveries
            .iter()
            .filter(|delivery| delivery.status == WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED)
            .count();
        let non_delivered_count = deliveries.len().saturating_sub(delivered_count);
        let metadata = json!({
            "delivery_count": deliveries.len(),
            "delivered_count": delivered_count,
            "failed_count": deliveries.iter().filter(|delivery| matches!(delivery.status.as_str(), WEBHOOK_RULE_DELIVERY_STATUS_FAILED | WEBHOOK_RULE_DELIVERY_STATUS_PERMANENTLY_FAILED)).count(),
            "non_delivered_count": non_delivered_count,
            "result": if non_delivered_count == 0 { "succeeded" } else { "partial" },
            "operator_id": operator.operator.id,
            "operator_username": &operator.operator.username,
            "operator_role": &operator.operator.role,
            "operator_session_id": operator.audit_session_id(),
            "origin_kind": "operator_request",
            "component": "webhook-delivery-controller",
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
    anyhow::ensure!(
        !(request.clear_signing_secret
            && request
                .signing_secret
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())),
        "webhook rule signing secret cannot be set and cleared together"
    );
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
    let signing_secret = normalize_signing_secret(request.signing_secret.as_deref())?;
    Ok(WebhookRuleView {
        id: request.id.unwrap_or_else(Uuid::new_v4),
        name: request.name.trim().to_string(),
        enabled: request.enabled,
        expression: request.expression.trim().to_string(),
        target: request.target.trim().to_string(),
        body_template: request.body_template.trim().to_string(),
        signing_secret_set: signing_secret.is_some(),
        signing_secret,
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

fn webhook_rule_database_error(error: sqlx::Error) -> anyhow::Error {
    if error
        .as_database_error()
        .and_then(|database_error| database_error.constraint())
        == Some("webhook_rules_name_key")
    {
        anyhow::anyhow!("webhook_rule_name_conflict")
    } else {
        error.into()
    }
}

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
        ON CONFLICT (rule_id, event_id) DO NOTHING
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

pub(crate) fn webhook_rule_revision_hash(rule: &WebhookRuleView) -> Result<String> {
    Ok(payload_hash(&serde_json::to_vec(&json!({
        "id": rule.id,
        "name": &rule.name,
        "enabled": rule.enabled,
        "expression": &rule.expression,
        "target": &rule.target,
        "body_template": &rule.body_template,
        "cooldown_secs": rule.cooldown_secs,
        "signing_secret": &rule.signing_secret,
    }))?))
}

fn webhook_rule_from_row(row: sqlx::postgres::PgRow) -> Result<WebhookRuleView> {
    let signing_secret: Option<String> = row.try_get("signing_secret")?;
    Ok(WebhookRuleView {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        enabled: row.try_get("enabled")?,
        expression: row.try_get("expression")?,
        target: row.try_get("target")?,
        body_template: row.try_get("body_template")?,
        signing_secret_set: signing_secret.is_some(),
        signing_secret,
        cooldown_secs: row.try_get("cooldown_secs")?,
        notes: row.try_get("notes")?,
        actor_id: row.try_get("actor_id")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn webhook_rule_material_matches(existing: &WebhookRuleView, candidate: &WebhookRuleView) -> bool {
    existing.name == candidate.name
        && existing.enabled == candidate.enabled
        && existing.expression == candidate.expression
        && existing.target == candidate.target
        && existing.body_template == candidate.body_template
        && existing.signing_secret == candidate.signing_secret
        && existing.cooldown_secs == candidate.cooldown_secs
        && existing.notes == candidate.notes
}

fn webhook_delivery_from_row(row: sqlx::postgres::PgRow) -> Result<WebhookRuleDeliveryView> {
    let payload: SqlJson<serde_json::Value> = row.try_get("payload")?;
    let matched_vps: SqlJson<Vec<AgentView>> = row.try_get("matched_vps")?;
    let signing_secret = row
        .try_get::<Option<String>, _>("signing_secret")
        .ok()
        .flatten();
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
        signing_secret,
        error: row.try_get("error")?,
        cooldown_until_unix: row.try_get("cooldown_until_unix")?,
        attempt_count: row.try_get("attempt_count")?,
        next_attempt_at: row.try_get("next_attempt_at")?,
        last_attempt_at: row.try_get("last_attempt_at")?,
        actor_id: row.try_get("actor_id")?,
        created_at: row.try_get("created_at")?,
        delivered_at: row.try_get("delivered_at")?,
        review_preview_hash: None,
        process_outcome: None,
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
        signing_secret: candidate.signing_secret.clone(),
        error: None,
        cooldown_until_unix: candidate.cooldown_until_unix,
        attempt_count: 0,
        next_attempt_at: None,
        last_attempt_at: None,
        actor_id: candidate.actor_id,
        created_at: unix_now().to_string(),
        delivered_at: (status == WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED)
            .then(|| unix_now().to_string()),
        review_preview_hash: None,
        process_outcome: None,
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

async fn insert_webhook_rule_audits_with_action(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rules: &[&WebhookRuleView],
    operator: &AuthContext,
    action: &str,
) -> Result<()> {
    if rules.is_empty() {
        return Ok(());
    }
    let mut query = QueryBuilder::<Postgres>::new(
        r#"
        INSERT INTO audit_logs (
            id, actor_id, action, target, command_hash, metadata
        )
        "#,
    );
    query.push_values(rules, |mut row, rule| {
        row.push_bind(Uuid::new_v4())
            .push_bind(operator.operator.id)
            .push_bind(action)
            .push_bind(format!("webhook_rule:{}", rule.id))
            .push("NULL")
            .push_bind(webhook_rule_metadata(rule, operator));
    });
    query.build().execute(&mut **tx).await?;
    Ok(())
}

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
        "result": "queued",
        "origin_kind": "control_plane",
        "component": "webhook-rule-dispatcher",
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

fn webhook_rule_metadata(rule: &WebhookRuleView, operator: &AuthContext) -> serde_json::Value {
    json!({
        "rule_id": rule.id,
        "name": &rule.name,
        "enabled": rule.enabled,
        "expression": &rule.expression,
        "target": &rule.target,
        "cooldown_secs": rule.cooldown_secs,
        "result": "succeeded",
        "operator_id": operator.operator.id,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "operator_session_id": operator.audit_session_id(),
        "origin_kind": "operator_request",
        "component": "webhook-rule-controller",
    })
}

fn validate_webhook_url(target: &str) -> Result<reqwest::Url> {
    validate_required_text(target, MAX_TARGET_BYTES, "webhook target")?;
    vpsman_server_core::validate_webhook_target(target)
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

fn normalize_signing_secret(value: Option<&str>) -> Result<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    anyhow::ensure!(
        value.len() <= MAX_SIGNING_SECRET_BYTES && !value.as_bytes().contains(&0),
        "webhook rule signing secret is invalid"
    );
    Ok(Some(value.to_string()))
}

pub(crate) async fn record_webhook_event_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event: WebhookEventCandidate,
    occurred_at: DateTime<Utc>,
) -> Result<WebhookEventRow> {
    let row = webhook_event_row(event, occurred_at)?;
    anyhow::ensure!(
        row.kind != "telemetry.rollup",
        "telemetry webhook events are materialized from canonical samples"
    );
    let lock_name = format!("vpsman:webhook-event:{}:{}", row.kind, row.event_id);
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(lock_name)
        .execute(&mut **tx)
        .await?;
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
    .bind(SqlJson(&row.payload))
    .bind(&row.occurred_at)
    .bind(row.actor_id)
    .execute(&mut **tx)
    .await?;
    if inserted.rows_affected() > 0 {
        sqlx::query("SELECT pg_notify('webhook_events', $1)")
            .bind(row.event_id.clone())
            .execute(&mut **tx)
            .await?;
        return Ok(row);
    }
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
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(stored) = stored {
        return webhook_event_from_row(stored);
    }
    anyhow::bail!("webhook event dedupe source disappeared")
}

pub(crate) fn webhook_event_row(
    event: WebhookEventCandidate,
    occurred_at: DateTime<Utc>,
) -> Result<WebhookEventRow> {
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
    Ok(row)
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

#[cfg(test)]
fn webhook_rotation_preview_hash(
    older_than: Option<&str>,
    status: Option<&str>,
    rule_id: Option<Uuid>,
    matched_ids: &mut [Uuid],
) -> Result<String> {
    matched_ids.sort_unstable();
    let mut hasher = webhook_rotation_preview_hasher(older_than, status, rule_id)?;
    for id in matched_ids {
        hasher.update(id.as_bytes());
    }
    Ok(hex::encode(hasher.finalize()))
}

fn webhook_rotation_preview_hasher(
    older_than: Option<&str>,
    status: Option<&str>,
    rule_id: Option<Uuid>,
) -> Result<Sha256> {
    let payload = serde_json::to_vec(&json!({
        "version": 2,
        "kind": "webhook_delivery_rotation",
        "older_than": older_than,
        "status": status,
        "rule_id": rule_id,
    }))?;
    let mut hasher = Sha256::new();
    hasher.update((payload.len() as u64).to_be_bytes());
    hasher.update(payload);
    Ok(hasher)
}

async fn postgres_webhook_rotation_snapshot(
    tx: &mut Transaction<'_, Postgres>,
    older_than: Option<&str>,
    status: Option<&str>,
    rule_id: Option<Uuid>,
) -> Result<(usize, String)> {
    let mut matched_count = 0usize;
    let mut after_id = None;
    let mut hasher = webhook_rotation_preview_hasher(older_than, status, rule_id)?;
    loop {
        let rows = sqlx::query(
            r#"
            SELECT id
            FROM webhook_rule_deliveries
            WHERE ($1::text IS NULL OR created_at < $1::timestamptz)
              AND ($2::text IS NULL OR status = $2)
              AND ($3::uuid IS NULL OR rule_id = $3)
              AND ($4::uuid IS NULL OR id > $4)
            ORDER BY id ASC
            LIMIT $5
            "#,
        )
        .bind(older_than)
        .bind(status)
        .bind(rule_id)
        .bind(after_id)
        .bind(WEBHOOK_ROTATION_SCAN_BATCH_SIZE)
        .fetch_all(&mut **tx)
        .await?;
        if rows.is_empty() {
            break;
        }
        let row_count = rows.len();
        for row in rows {
            let id = row.try_get::<Uuid, _>("id")?;
            hasher.update(id.as_bytes());
            after_id = Some(id);
            matched_count = matched_count
                .checked_add(1)
                .context("webhook delivery rotation match count overflow")?;
        }
        if row_count < WEBHOOK_ROTATION_SCAN_BATCH_SIZE as usize {
            break;
        }
    }
    Ok((matched_count, hex::encode(hasher.finalize())))
}

fn webhook_rotation_confirmation_sql_error(error: sqlx::Error) -> anyhow::Error {
    if matches!(
        &error,
        sqlx::Error::Database(database) if database.code().as_deref() == Some("40001")
    ) {
        return anyhow::anyhow!("webhook_delivery_rotation_changed_during_confirmation");
    }
    error.into()
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
                        | WEBHOOK_RULE_DELIVERY_STATUS_CANCELED_DISABLED
                        | WEBHOOK_RULE_DELIVERY_STATUS_PERMANENTLY_FAILED
                ),
                "webhook rule delivery status is invalid"
            );
            Ok(value.to_string())
        })
        .transpose()
}

async fn postgres_arm_webhook_rule_alert_send<'e, E>(
    executor: E,
    delivery_id: Uuid,
    lease_id: Uuid,
) -> Result<WebhookRuleAlertSendEligibilityRevision>
where
    E: Executor<'e, Database = Postgres>,
{
    let Some(row) = sqlx::query(
        r#"
        WITH delivery_scope AS MATERIALIZED (
            SELECT delivery.id, delivery.event_kind, delivery.event_id,
                   delivery.status='in_progress'
                     AND delivery.delivery_lease_id=$2 AS lease_owned,
                   rule.enabled AS rule_enabled,
                   jsonb_array_length(delivery.matched_vps) AS matched_count,
                   ARRAY(
                        SELECT DISTINCT matched->>'id'
                        FROM jsonb_array_elements(delivery.matched_vps) matched
                        WHERE jsonb_typeof(matched)='object'
                          AND NULLIF(btrim(matched->>'id'),'') IS NOT NULL
                        ORDER BY matched->>'id'
                   ) AS client_ids
            FROM webhook_rule_deliveries delivery
            JOIN webhook_rules rule ON rule.id=delivery.rule_id
            WHERE delivery.id=$1
        ), eligibility AS MATERIALIZED (
            SELECT scope.*,
                   scope.event_kind<>'alert.triggered'
                     OR scope.matched_count=0
                     OR (
                        cardinality(scope.client_ids)=scope.matched_count
                        AND (
                            SELECT count(*)
                            FROM clients subject
                            WHERE subject.id=ANY(scope.client_ids)
                        )=cardinality(scope.client_ids)
                     ) AS scope_exact,
                   scope.event_kind='alert.triggered'
                     AND EXISTS (
                        SELECT 1 FROM clients subject
                        WHERE subject.id=ANY(scope.client_ids)
                          AND subject.status='suspended'
                     ) AS subject_suspended,
                   scope.event_kind='alert.triggered'
                     AND EXISTS (
                        SELECT 1
                        FROM alert_lifecycle_events lifecycle
                        JOIN alert_episodes episode
                          ON episode.id=lifecycle.episode_id
                         AND episode.trigger_generation=lifecycle.trigger_generation
                        WHERE lifecycle.edge_kind='alert.triggered'
                          AND lifecycle.event_id=scope.event_id
                          AND episode.evidence#>>'{_vpsman_client_suspension,client_id}'
                                = ANY(scope.client_ids)
                     ) AS source_suppressed
            FROM delivery_scope scope
        ), armed AS (
            UPDATE webhook_rule_deliveries delivery
            SET eligibility_revision=delivery.eligibility_revision+1
            FROM eligibility
            WHERE delivery.id=eligibility.id
              AND eligibility.lease_owned AND eligibility.rule_enabled
              AND eligibility.scope_exact
              AND NOT eligibility.subject_suspended
              AND NOT eligibility.source_suppressed
            RETURNING delivery.eligibility_revision
        )
        SELECT eligibility.lease_owned, eligibility.rule_enabled,
               eligibility.scope_exact, eligibility.subject_suspended,
               eligibility.source_suppressed, armed.eligibility_revision
        FROM eligibility LEFT JOIN armed ON TRUE
        "#,
    )
    .bind(delivery_id)
    .bind(lease_id)
    .fetch_optional(executor)
    .await?
    else {
        return Ok(WebhookRuleAlertSendEligibilityRevision {
            eligibility: WebhookRuleAlertSendEligibility::LeaseLost,
            revision: None,
        });
    };
    let eligibility = if !row.try_get::<bool, _>("lease_owned")? {
        WebhookRuleAlertSendEligibility::LeaseLost
    } else if !row.try_get::<bool, _>("rule_enabled")? {
        WebhookRuleAlertSendEligibility::RuleDisabled
    } else if !row.try_get::<bool, _>("scope_exact")? {
        WebhookRuleAlertSendEligibility::InvalidClientScope
    } else if row.try_get::<bool, _>("subject_suspended")?
        || row.try_get::<bool, _>("source_suppressed")?
    {
        WebhookRuleAlertSendEligibility::ClientSuspended
    } else {
        WebhookRuleAlertSendEligibility::Deliverable
    };
    let revision: Option<i64> = row.try_get("eligibility_revision")?;
    anyhow::ensure!(
        eligibility != WebhookRuleAlertSendEligibility::Deliverable || revision.is_some(),
        "webhook delivery eligibility revision was not armed"
    );
    Ok(WebhookRuleAlertSendEligibilityRevision {
        eligibility,
        revision,
    })
}

async fn postgres_complete_webhook_rule_delivery_attempt<'e, E>(
    executor: E,
    delivery_id: Uuid,
    lease_id: Uuid,
    status: &str,
    error: Option<&str>,
    next_attempt_after_secs: Option<i64>,
    eligibility_revision: Option<i64>,
) -> Result<WebhookRuleDeliveryView>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query(
        r#"
        UPDATE webhook_rule_deliveries
        SET status=$2, error=$3, attempt_count=attempt_count+1,
            delivery_lease_id=NULL, delivery_lease_until=NULL,
            next_attempt_at=CASE
                WHEN $5::bigint IS NULL THEN NULL
                ELSE now() + ($5::bigint * interval '1 second')
            END,
            last_attempt_at=now(),
            delivered_at=CASE WHEN $2='delivered' THEN now() ELSE NULL END
        WHERE id=$1 AND status='in_progress' AND delivery_lease_id=$4
          AND ($6::bigint IS NULL OR eligibility_revision=$6)
        RETURNING
            id, rule_id, rule_name, event_kind, event_id, status, target,
            dedupe_key, payload, matched_vps, message, error,
            cooldown_until_unix, attempt_count,
            next_attempt_at::text AS next_attempt_at,
            last_attempt_at::text AS last_attempt_at, actor_id,
            created_at::text AS created_at,
            delivered_at::text AS delivered_at
        "#,
    )
    .bind(delivery_id)
    .bind(status)
    .bind(error)
    .bind(lease_id)
    .bind(next_attempt_after_secs.filter(|seconds| *seconds > 0))
    .bind(eligibility_revision)
    .fetch_optional(executor)
    .await?
    .context("webhook rule delivery not found or not claimed")?;
    webhook_delivery_from_row(row)
}

async fn postgres_cancel_claimed_webhook_rule_delivery<'e, E>(
    executor: E,
    delivery_id: Uuid,
    lease_id: Uuid,
    error: &str,
) -> Result<WebhookRuleDeliveryView>
where
    E: Executor<'e, Database = Postgres>,
{
    let row = sqlx::query(
        r#"
        WITH updated AS (
            UPDATE webhook_rule_deliveries
            SET status='canceled_disabled', error=$3,
                delivery_lease_id=NULL, delivery_lease_until=NULL,
                next_attempt_at=NULL, delivered_at=NULL
            WHERE id=$1 AND status='in_progress' AND delivery_lease_id=$2
            RETURNING
                id, rule_id, rule_name, event_kind, event_id, status, target,
                dedupe_key, payload, matched_vps, message, error,
                cooldown_until_unix, attempt_count,
                next_attempt_at::text AS next_attempt_at,
                last_attempt_at::text AS last_attempt_at, actor_id,
                created_at::text AS created_at,
                delivered_at::text AS delivered_at
        )
        SELECT * FROM updated
        UNION ALL
        SELECT
            id, rule_id, rule_name, event_kind, event_id, status, target,
            dedupe_key, payload, matched_vps, message, error,
            cooldown_until_unix, attempt_count,
            next_attempt_at::text AS next_attempt_at,
            last_attempt_at::text AS last_attempt_at, actor_id,
            created_at::text AS created_at,
            delivered_at::text AS delivered_at
        FROM webhook_rule_deliveries
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
    .context("webhook rule delivery not found or not claimed")?;
    webhook_delivery_from_row(row)
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
#[path = "tests_repository_webhook_rules.rs"]
mod tests;
