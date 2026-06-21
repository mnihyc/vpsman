use anyhow::{Context, Result};
use chrono::{Duration as ChronoDuration, Utc};
use reqwest::{redirect::Policy, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{types::Json as SqlJson, PgPool, Postgres, Row, Transaction};
use tokio::time::Duration;
use uuid::Uuid;
use vpsman_common::{
    default_webhook_message, expression_matches, expression_referenced_events,
    expression_referenced_roots, parse_expression, payload_hash, render_template_with_limit,
    ExpressionContext, VpsMetadata, WEBHOOK_RULE_DELIVERY_STATUS_CANCELED_DISABLED,
    WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED, WEBHOOK_RULE_DELIVERY_STATUS_FAILED,
    WEBHOOK_RULE_DELIVERY_STATUS_PERMANENTLY_FAILED,
};

use crate::actor_authority::actor_authorized;

const DEFAULT_WEBHOOK_TIMEOUT_SECS: u64 = 5;
const MAX_ERROR_BYTES: usize = 1024;
const MAX_AUDIT_DELIVERY_ROWS: usize = 100;
const MAX_DELIVERY_ATTEMPTS: i32 = 4;
const RETRY_BACKOFF_SECS: [i64; 3] = [60, 5 * 60, 30 * 60];
const INTERVAL_EVENTS: &[(&str, i64)] = &[
    ("interval.30sec", 30),
    ("interval.1min", 60),
    ("interval.5min", 5 * 60),
    ("interval.1h", 60 * 60),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WebhookRuleWorkerConfig {
    pub(crate) delivery_limit: i64,
    pub(crate) materialize_limit: i64,
    pub(crate) retention_days: i64,
    pub(crate) retention_prune_limit: i64,
    pub(crate) webhook_timeout_secs: u64,
}

impl WebhookRuleWorkerConfig {
    pub(crate) fn new(
        delivery_limit: i64,
        materialize_limit: i64,
        retention_days: i64,
        retention_prune_limit: i64,
        webhook_timeout_secs: u64,
    ) -> Result<Self> {
        anyhow::ensure!(
            (1..=3_650).contains(&retention_days),
            "webhook_rule_retention_days_out_of_range"
        );
        Ok(Self {
            delivery_limit: delivery_limit.clamp(1, 200),
            materialize_limit: materialize_limit.clamp(1, 1000),
            retention_days,
            retention_prune_limit: retention_prune_limit.clamp(1, 10_000),
            webhook_timeout_secs: webhook_timeout_secs.clamp(1, 60),
        })
    }
}

impl Default for WebhookRuleWorkerConfig {
    fn default() -> Self {
        Self::new(25, 100, 90, 1_000, DEFAULT_WEBHOOK_TIMEOUT_SECS)
            .expect("default webhook retention config is valid")
    }
}

#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct WebhookRuleWorkerRun {
    pub(crate) materialized: usize,
    pub(crate) processed: usize,
    pub(crate) delivered: usize,
    pub(crate) failed: usize,
    pub(crate) pruned: usize,
}

#[derive(Clone, Debug)]
struct RuleRow {
    id: Uuid,
    actor_id: Option<Uuid>,
    name: String,
    expression: String,
    target: String,
    body_template: String,
    cooldown_secs: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct VpsRow {
    id: String,
    display_name: String,
    status: String,
    tags: Vec<String>,
    registration_ip: Option<String>,
    last_ip: Option<String>,
    last_seen_at: Option<String>,
    internal_build_number: u64,
    stale_since: Option<String>,
    stale_reason: Option<String>,
    capabilities: Value,
}

#[derive(Clone, Debug)]
struct DeliveryCandidate {
    id: Uuid,
    rule_id: Uuid,
    actor_id: Option<Uuid>,
    rule_name: String,
    event_kind: String,
    event_id: String,
    target: String,
    dedupe_key: String,
    payload: Value,
    matched_vps: Vec<VpsRow>,
    message: String,
    cooldown_until_unix: i64,
}

#[derive(Clone, Debug)]
struct DeliveryRow {
    id: Uuid,
    rule_id: Uuid,
    actor_id: Option<Uuid>,
    rule_name: String,
    event_kind: String,
    event_id: String,
    target: String,
    payload: Value,
    attempt_count: i32,
}

#[derive(Clone, Debug)]
struct EventRow {
    id: Uuid,
    actor_id: Option<Uuid>,
    kind: String,
    event_id: String,
    event_predicates: Vec<String>,
    subject_client_ids: Vec<String>,
    payload: Value,
    occurred_at_unix: i64,
}

#[derive(Clone, Debug)]
struct DeliveryOutcome {
    id: Uuid,
    rule_id: Uuid,
    rule_name: String,
    event_kind: String,
    event_id: String,
    status: String,
    attempt_count: i32,
    error: Option<String>,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
struct PrunedDelivery {
    id: Uuid,
    rule_id: Uuid,
    status: String,
    created_at: String,
}

pub(crate) async fn process_webhook_rules(
    pool: &PgPool,
    config: WebhookRuleWorkerConfig,
) -> Result<WebhookRuleWorkerRun> {
    ensure_event_partitions(pool).await?;
    let materialized = materialize_interval_events(pool, config).await?;
    let event_deliveries = process_webhook_events(pool, config).await?;
    let (processed, delivered, failed) = process_queued_deliveries(pool, config).await?;
    let pruned = drop_old_event_partitions(pool, config).await?
        + prune_default_partition_rows(pool, config).await?
        + prune_deliveries(pool, config).await?;
    Ok(WebhookRuleWorkerRun {
        materialized: materialized + event_deliveries,
        processed,
        delivered,
        failed,
        pruned,
    })
}

async fn materialize_interval_events(
    pool: &PgPool,
    config: WebhookRuleWorkerConfig,
) -> Result<usize> {
    let now = chrono::Utc::now().timestamp();
    let rules = list_enabled_rules(pool, config.materialize_limit).await?;
    if rules.is_empty() {
        return Ok(0);
    }
    let mut materialized = 0_usize;
    for &(event_kind, bucket_secs) in INTERVAL_EVENTS {
        let event_id = format!("{event_kind}:{}", now - now.rem_euclid(bucket_secs));
        if !rules.iter().any(|rule| {
            rule.expression
                .to_ascii_lowercase()
                .contains(&event_kind.to_ascii_lowercase())
        }) {
            continue;
        }
        if insert_webhook_event(
            pool,
            event_kind,
            &event_id,
            &[event_kind],
            &[],
            json!({
                "event": {
                    "kind": event_kind,
                    "id": event_id,
                    "bucket_unix": now - now.rem_euclid(bucket_secs),
                }
            }),
        )
        .await?
        {
            materialized += 1;
        }
    }
    Ok(materialized)
}

async fn list_enabled_rules(pool: &PgPool, limit: i64) -> Result<Vec<RuleRow>> {
    let rows = sqlx::query(
        r#"
        SELECT
            id,
            actor_id,
            name,
            expression,
            target,
            body_template,
            cooldown_secs
        FROM webhook_rules
        WHERE enabled = TRUE
        ORDER BY updated_at ASC, id ASC
        LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(RuleRow {
                id: row.try_get("id")?,
                actor_id: row.try_get("actor_id")?,
                name: row.try_get("name")?,
                expression: row.try_get("expression")?,
                target: row.try_get("target")?,
                body_template: row.try_get("body_template")?,
                cooldown_secs: row.try_get("cooldown_secs")?,
            })
        })
        .collect()
}

async fn list_visible_vps(pool: &PgPool) -> Result<Vec<VpsRow>> {
    let rows = sqlx::query(
        r#"
        SELECT
            c.id,
            c.display_name,
            c.status,
            c.registration_ip::text AS registration_ip,
            c.last_ip::text AS last_ip,
            c.last_seen_at::text AS last_seen_at,
            c.internal_build_number,
            c.stale_since::text AS stale_since,
            c.stale_reason,
            c.capabilities,
            COALESCE(array_agg(t.name ORDER BY t.name) FILTER (WHERE t.name IS NOT NULL), ARRAY[]::TEXT[]) AS tags
        FROM clients c
        LEFT JOIN client_tags ct ON ct.client_id = c.id
        LEFT JOIN tags t ON t.id = ct.tag_id
        WHERE c.hidden_at IS NULL
        GROUP BY c.id, c.display_name, c.status, c.registration_ip, c.last_ip, c.last_seen_at, c.internal_build_number, c.stale_since, c.stale_reason, c.capabilities
        ORDER BY c.id
        "#,
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            let capabilities: SqlJson<Value> = row.try_get("capabilities")?;
            Ok(VpsRow {
                id: row.try_get("id")?,
                display_name: row.try_get("display_name")?,
                status: row.try_get("status")?,
                tags: row.try_get("tags")?,
                registration_ip: row.try_get("registration_ip")?,
                last_ip: row.try_get("last_ip")?,
                last_seen_at: row.try_get("last_seen_at")?,
                internal_build_number: row.try_get::<i64, _>("internal_build_number")?.max(1)
                    as u64,
                stale_since: row.try_get("stale_since")?,
                stale_reason: row.try_get("stale_reason")?,
                capabilities: capabilities.0,
            })
        })
        .collect()
}

#[allow(dead_code)]
async fn claim_event_cursor(
    pool: &PgPool,
    rule_id: Uuid,
    event_key: &str,
    event_id: &str,
) -> Result<bool> {
    let row = sqlx::query_scalar::<_, String>(
        r#"
        INSERT INTO webhook_rule_cursors (rule_id, event_key, last_event_id)
        VALUES ($1, $2, $3)
        ON CONFLICT (rule_id, event_key) DO UPDATE SET
            last_event_id = EXCLUDED.last_event_id,
            updated_at = now()
        WHERE webhook_rule_cursors.last_event_id <> EXCLUDED.last_event_id
        RETURNING last_event_id
        "#,
    )
    .bind(rule_id)
    .bind(event_key)
    .bind(event_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

pub(crate) async fn insert_webhook_event(
    pool: &PgPool,
    kind: &str,
    event_id: &str,
    event_predicates: &[&str],
    subject_client_ids: &[String],
    payload: Value,
) -> Result<bool> {
    let occurred_at = Utc::now();
    create_event_partition(pool, occurred_at.date_naive()).await?;
    let predicates = normalize_event_predicates(kind, event_predicates);
    let inserted = sqlx::query(
        r#"
        INSERT INTO webhook_events (
            id,
            actor_id,
            kind,
            event_id,
            event_predicates,
            subject_client_ids,
            payload,
            occurred_at
        )
        SELECT $1, NULL, $2, $3, $4, $5, $6, $7::timestamptz
        WHERE NOT EXISTS (
            SELECT 1 FROM webhook_events WHERE kind = $2 AND event_id = $3
        )
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(kind)
    .bind(event_id)
    .bind(&predicates)
    .bind(subject_client_ids)
    .bind(SqlJson(payload))
    .bind(occurred_at.to_rfc3339())
    .execute(pool)
    .await?;
    if inserted.rows_affected() > 0 {
        let _ = sqlx::query("SELECT pg_notify('webhook_events', $1)")
            .bind(event_id)
            .execute(pool)
            .await?;
        return Ok(true);
    }
    Ok(false)
}

pub(crate) async fn insert_webhook_event_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    kind: &str,
    event_id: &str,
    event_predicates: &[String],
    subject_client_ids: &[String],
    payload: Value,
) -> Result<bool> {
    let occurred_at = Utc::now();
    create_event_partition_in_tx(tx, occurred_at.date_naive()).await?;
    let predicate_refs = event_predicates
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let predicates = normalize_event_predicates(kind, &predicate_refs);
    let inserted = sqlx::query(
        r#"
        INSERT INTO webhook_events (
            id,
            kind,
            event_id,
            event_predicates,
            subject_client_ids,
            payload,
            occurred_at
        )
        SELECT $1, $2, $3, $4, $5, $6, $7::timestamptz
        WHERE NOT EXISTS (
            SELECT 1 FROM webhook_events WHERE kind = $2 AND event_id = $3
        )
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(kind)
    .bind(event_id)
    .bind(&predicates)
    .bind(subject_client_ids)
    .bind(SqlJson(payload))
    .bind(occurred_at.to_rfc3339())
    .execute(&mut **tx)
    .await?;
    if inserted.rows_affected() > 0 {
        let _ = sqlx::query("SELECT pg_notify('webhook_events', $1)")
            .bind(event_id)
            .execute(&mut **tx)
            .await?;
        return Ok(true);
    }
    Ok(false)
}

async fn process_webhook_events(pool: &PgPool, config: WebhookRuleWorkerConfig) -> Result<usize> {
    let mut tx = pool.begin().await?;
    let rows = sqlx::query(
        r#"
        SELECT
            id,
            actor_id,
            kind,
            event_id,
            event_predicates,
            subject_client_ids,
            payload,
            EXTRACT(EPOCH FROM occurred_at)::bigint AS occurred_at_unix
        FROM webhook_events
        WHERE processed_at IS NULL
        ORDER BY occurred_at ASC, id ASC
        LIMIT $1
        FOR UPDATE SKIP LOCKED
        "#,
    )
    .bind(config.materialize_limit)
    .fetch_all(&mut *tx)
    .await?;
    if rows.is_empty() {
        tx.commit().await?;
        return Ok(0);
    }
    let rules = list_enabled_rules(pool, config.materialize_limit).await?;
    let vps_rows = list_visible_vps(pool).await?;
    let mut inserted = 0_usize;
    for row in rows {
        let event = event_from_row(row)?;
        for rule in &rules {
            let candidates = event_candidate_for_rule(rule, &event, &vps_rows)?;
            let Some(candidate) = candidates else {
                continue;
            };
            if insert_delivery_candidate(&mut tx, &candidate).await? {
                inserted += 1;
            }
        }
        sqlx::query("UPDATE webhook_events SET processed_at = now() WHERE id = $1")
            .bind(event.id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(inserted)
}

#[cfg(test)]
fn delivery_candidate_for_rule(
    rule: &RuleRow,
    event_kind: &str,
    event_id: &str,
    vps_rows: &[VpsRow],
    now: i64,
) -> Result<Option<DeliveryCandidate>> {
    event_candidate_for_rule(
        rule,
        &EventRow {
            id: Uuid::nil(),
            kind: event_kind.to_string(),
            event_id: event_id.to_string(),
            event_predicates: vec![event_kind.to_string()],
            subject_client_ids: Vec::new(),
            payload: Value::Null,
            occurred_at_unix: now,
            actor_id: None,
        },
        vps_rows,
    )
}

fn event_candidate_for_rule(
    rule: &RuleRow,
    event: &EventRow,
    vps_rows: &[VpsRow],
) -> Result<Option<DeliveryCandidate>> {
    let expression = parse_expression(&rule.expression)
        .map_err(anyhow::Error::msg)?
        .context("webhook rule expression is empty")?;
    let subject_ids = event
        .subject_client_ids
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    let matched_vps = vps_rows
        .iter()
        .filter(|vps| subject_ids.is_empty() || subject_ids.contains(&vps.id))
        .filter(|vps| {
            let context = expression_context_for_event(vps, event);
            expression_matches(&context, &expression)
        })
        .cloned()
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
        },
        "event": {
            "kind": &event.kind,
            "id": &event.event_id,
            "predicates": &event.event_predicates,
            "occurred_at_unix": event.occurred_at_unix,
        },
        "query": {
            "expression": &rule.expression,
            "referenced_roots": referenced_roots,
            "referenced_events": referenced_events,
        },
        "matched_vps": &matched_vps,
    });
    merge_event_payload_roots(&mut payload, &event.payload);
    let message = render_message(rule, &payload)?;
    if let Some(object) = payload.as_object_mut() {
        object.insert("message".to_string(), Value::String(message.clone()));
    }
    let dedupe_fingerprint = json!({
        "rule_id": rule.id,
        "event_id": &event.event_id,
    });
    let hash = payload_hash(dedupe_fingerprint.to_string().as_bytes());
    Ok(Some(DeliveryCandidate {
        id: Uuid::new_v4(),
        rule_id: rule.id,
        actor_id: event.actor_id.or(rule.actor_id),
        rule_name: rule.name.clone(),
        event_kind: event.kind.clone(),
        event_id: event.event_id.clone(),
        target: rule.target.clone(),
        dedupe_key: format!("webhook-rule:{}", &hash[..32]),
        payload,
        matched_vps,
        message,
        cooldown_until_unix: event.occurred_at_unix.saturating_add(rule.cooldown_secs),
    }))
}

fn expression_context_for_event(vps: &VpsRow, event: &EventRow) -> ExpressionContext {
    let mut context = ExpressionContext::for_vps(VpsMetadata {
        id: vps.id.clone(),
        display_name: vps.display_name.clone(),
        status: vps.status.clone(),
        tags: vps.tags.clone(),
        registration_ip: vps.registration_ip.clone(),
        last_ip: vps.last_ip.clone(),
        last_seen_at: vps.last_seen_at.clone(),
        internal_build_number: Some(vps.internal_build_number),
        stale_since: vps.stale_since.clone(),
        stale_reason: vps.stale_reason.clone(),
        extra: Some(json!({
            "capabilities": &vps.capabilities,
        })),
    })
    .with_event_predicate(&event.kind);
    for predicate in &event.event_predicates {
        context = context.with_event_predicate(predicate);
    }
    for root in ["server", "job", "schedule", "alert", "telemetry", "event"] {
        if let Some(value) = event.payload.get(root).cloned() {
            context = context.with_json_root(root, value);
        }
    }
    context
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

fn event_from_row(row: sqlx::postgres::PgRow) -> Result<EventRow> {
    let payload: SqlJson<Value> = row.try_get("payload")?;
    Ok(EventRow {
        id: row.try_get("id")?,
        actor_id: row.try_get("actor_id")?,
        kind: row.try_get("kind")?,
        event_id: row.try_get("event_id")?,
        event_predicates: row.try_get("event_predicates")?,
        subject_client_ids: row.try_get("subject_client_ids")?,
        payload: payload.0,
        occurred_at_unix: row.try_get("occurred_at_unix")?,
    })
}

fn normalize_event_predicates(kind: &str, predicates: &[&str]) -> Vec<String> {
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

async fn insert_delivery_candidate(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    candidate: &DeliveryCandidate,
) -> Result<bool> {
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
    .fetch_optional(&mut **tx)
    .await?
    .is_some();
    if duplicate {
        return Ok(false);
    }
    let inserted = sqlx::query(
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
        VALUES ($1, $2, $3, $4, $5, 'queued', $6, $7, $8, $9, $10, NULL, $11, 0, NULL, NULL, $12, NULL)
        "#,
    )
    .bind(candidate.id)
    .bind(candidate.rule_id)
    .bind(&candidate.rule_name)
    .bind(&candidate.event_kind)
    .bind(&candidate.event_id)
    .bind(&candidate.target)
    .bind(&candidate.dedupe_key)
    .bind(SqlJson(&candidate.payload))
    .bind(SqlJson(&candidate.matched_vps))
    .bind(&candidate.message)
    .bind(candidate.cooldown_until_unix)
    .bind(candidate.actor_id)
    .execute(&mut **tx)
    .await?;
    Ok(inserted.rows_affected() > 0)
}

async fn process_queued_deliveries(
    pool: &PgPool,
    config: WebhookRuleWorkerConfig,
) -> Result<(usize, usize, usize)> {
    let lease_id = Uuid::new_v4();
    let lease_secs = delivery_lease_secs(config.delivery_limit, config.webhook_timeout_secs);
    let rows = sqlx::query(
        r#"
        WITH claim AS (
            SELECT delivery.id
            FROM webhook_rule_deliveries delivery
            JOIN webhook_rules rule
              ON rule.id = delivery.rule_id
             AND rule.enabled = TRUE
            WHERE (
                    delivery.status IN ('queued', 'failed')
                    AND (delivery.next_attempt_at IS NULL OR delivery.next_attempt_at <= now())
                  )
               OR (
                    delivery.status = 'in_progress'
                    AND delivery.delivery_lease_until < now()
                  )
            ORDER BY delivery.created_at ASC, delivery.id ASC
            LIMIT $1
            FOR UPDATE OF delivery SKIP LOCKED
        )
        UPDATE webhook_rule_deliveries delivery
        SET status = 'in_progress',
            error = NULL,
            delivery_lease_id = $2,
            delivery_lease_until = now() + make_interval(secs => $3::integer),
            next_attempt_at = NULL
        FROM claim
        WHERE delivery.id = claim.id
        RETURNING
            delivery.id,
            delivery.rule_id,
            delivery.actor_id,
            delivery.rule_name,
            delivery.event_kind,
            delivery.event_id,
            delivery.target,
            delivery.payload,
            delivery.attempt_count
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
        if !webhook_rule_enabled(pool, delivery.rule_id).await? {
            let updated = sqlx::query(
                r#"
                UPDATE webhook_rule_deliveries
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
            .bind("webhook rule disabled")
            .fetch_optional(pool)
            .await?;
            let Some(updated) = updated else {
                continue;
            };
            let recorded_attempt_count: i32 = updated.try_get("attempt_count")?;
            outcomes.push(DeliveryOutcome {
                id: delivery.id,
                rule_id: delivery.rule_id,
                rule_name: delivery.rule_name,
                event_kind: delivery.event_kind,
                event_id: delivery.event_id,
                status: WEBHOOK_RULE_DELIVERY_STATUS_CANCELED_DISABLED.to_string(),
                attempt_count: recorded_attempt_count,
                error: Some("webhook rule disabled".to_string()),
            });
            continue;
        }
        let result =
            if actor_authorized(pool, delivery.actor_id, "operator", &["integrations:write"])
                .await?
            {
                deliver_webhook(&client, &delivery).await
            } else {
                Err(anyhow::anyhow!("actor_authority_revoked"))
            };
        let next_attempt_count = delivery.attempt_count.saturating_add(1);
        let (status, error, next_attempt_after_secs) = match result {
            Ok(()) => (WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED, None, None),
            Err(error) if error.to_string() == "actor_authority_revoked" => (
                WEBHOOK_RULE_DELIVERY_STATUS_PERMANENTLY_FAILED,
                Some("actor_authority_revoked".to_string()),
                None,
            ),
            Err(error) if next_attempt_count >= MAX_DELIVERY_ATTEMPTS => (
                WEBHOOK_RULE_DELIVERY_STATUS_PERMANENTLY_FAILED,
                Some(truncate_error(&error.to_string())),
                None,
            ),
            Err(error) => (
                WEBHOOK_RULE_DELIVERY_STATUS_FAILED,
                Some(truncate_error(&error.to_string())),
                retry_backoff_secs(next_attempt_count),
            ),
        };
        let mut tx = pool.begin().await?;
        let updated = sqlx::query(
            r#"
            UPDATE webhook_rule_deliveries
            SET
                status = $2,
                error = $3,
                attempt_count = attempt_count + 1,
                next_attempt_at = CASE
                    WHEN $4::bigint IS NULL THEN NULL
                    ELSE now() + ($4::bigint * interval '1 second')
                END,
                last_attempt_at = now(),
                delivered_at = CASE WHEN $2 = 'delivered' THEN now() ELSE NULL END,
                delivery_lease_id = NULL,
                delivery_lease_until = NULL
            WHERE id = $1
              AND status = 'in_progress'
              AND delivery_lease_id = $5
            RETURNING attempt_count
            "#,
        )
        .bind(delivery.id)
        .bind(status)
        .bind(error.as_deref())
        .bind(next_attempt_after_secs)
        .bind(lease_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(updated) = updated else {
            tx.commit().await?;
            continue;
        };
        let recorded_attempt_count: i32 = updated.try_get("attempt_count")?;
        if status == WEBHOOK_RULE_DELIVERY_STATUS_PERMANENTLY_FAILED {
            insert_permanent_failure_alert(&mut tx, &delivery, error.clone()).await?;
        }
        tx.commit().await?;
        outcomes.push(DeliveryOutcome {
            id: delivery.id,
            rule_id: delivery.rule_id,
            rule_name: delivery.rule_name,
            event_kind: delivery.event_kind,
            event_id: delivery.event_id,
            status: status.to_string(),
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
        .filter(|outcome| outcome.status == WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED)
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

async fn webhook_rule_enabled(pool: &PgPool, rule_id: Uuid) -> Result<bool> {
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

async fn deliver_webhook(client: &reqwest::Client, delivery: &DeliveryRow) -> Result<()> {
    let url = validate_webhook_url(&delivery.target)?;
    let response = client
        .post(url)
        .json(&delivery.payload)
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

pub(crate) async fn ensure_event_partitions(pool: &PgPool) -> Result<()> {
    let today = Utc::now().date_naive();
    for offset in 0..=1_i64 {
        let date = today + ChronoDuration::days(offset);
        create_event_partition(pool, date).await?;
    }
    Ok(())
}

async fn create_event_partition(pool: &PgPool, date: chrono::NaiveDate) -> Result<()> {
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

async fn create_event_partition_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    date: chrono::NaiveDate,
) -> Result<()> {
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
    sqlx::query(&sql).execute(&mut **tx).await?;
    Ok(())
}

async fn drop_old_event_partitions(
    pool: &PgPool,
    config: WebhookRuleWorkerConfig,
) -> Result<usize> {
    let cutoff = Utc::now().date_naive() - ChronoDuration::days(config.retention_days);
    let rows = sqlx::query(
        r#"
        SELECT tablename
        FROM pg_tables
        WHERE schemaname = current_schema()
          AND tablename ~ '^webhook_events_[0-9]{8}$'
        "#,
    )
    .fetch_all(pool)
    .await?;
    let mut dropped = 0_usize;
    for row in rows {
        let table_name: String = row.try_get("tablename")?;
        let Some(suffix) = table_name.strip_prefix("webhook_events_") else {
            continue;
        };
        let Ok(date) = chrono::NaiveDate::parse_from_str(suffix, "%Y%m%d") else {
            continue;
        };
        if date >= cutoff {
            continue;
        }
        let sql = format!("DROP TABLE IF EXISTS {table_name}");
        sqlx::query(&sql).execute(pool).await?;
        dropped += 1;
    }
    Ok(dropped)
}

async fn prune_default_partition_rows(
    pool: &PgPool,
    config: WebhookRuleWorkerConfig,
) -> Result<usize> {
    let rows = sqlx::query(
        r#"
        WITH candidates AS (
            SELECT occurred_at, id
            FROM webhook_events
            WHERE tableoid = 'webhook_events_default'::regclass
              AND processed_at IS NOT NULL
              AND occurred_at <= now() - ($1::bigint * interval '1 day')
            ORDER BY occurred_at ASC, id ASC
            LIMIT $2
        )
        DELETE FROM webhook_events events
        USING candidates
        WHERE events.occurred_at = candidates.occurred_at
          AND events.id = candidates.id
        RETURNING events.id
        "#,
    )
    .bind(config.retention_days)
    .bind(config.retention_prune_limit)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(0);
    }
    insert_default_partition_prune_audit(pool, config, rows.len()).await?;
    Ok(rows.len())
}

async fn insert_default_partition_prune_audit(
    pool: &PgPool,
    config: WebhookRuleWorkerConfig,
    pruned_count: usize,
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
    .bind("webhook.default_partition_pruned")
    .bind("webhook_events_default")
    .bind(json!({
        "worker": "webhook_rule_worker",
        "retention_days": config.retention_days,
        "pruned_count": pruned_count,
    }))
    .execute(pool)
    .await?;
    Ok(())
}

#[allow(dead_code)]
async fn prune_deliveries(pool: &PgPool, config: WebhookRuleWorkerConfig) -> Result<usize> {
    let rows = sqlx::query(
        r#"
        WITH candidates AS (
            SELECT id
            FROM webhook_rule_deliveries
            WHERE status IN ('delivered', 'failed', 'permanently_failed', 'canceled_disabled')
              AND created_at <= now() - ($1::bigint * interval '1 day')
            ORDER BY created_at ASC, id ASC
            LIMIT $2
        ),
        deleted AS (
            DELETE FROM webhook_rule_deliveries deliveries
            USING candidates
            WHERE deliveries.id = candidates.id
            RETURNING
                deliveries.id,
                deliveries.rule_id,
                deliveries.status,
                deliveries.created_at::text AS created_at
        )
        SELECT id, rule_id, status, created_at
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
                rule_id: row.try_get("rule_id")?,
                status: row.try_get("status")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let resolved_alerts = resolve_pruned_delivery_alerts(pool, &pruned).await?;
    insert_prune_audit(pool, config, &pruned, resolved_alerts).await?;
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
    .bind("webhook.rule_deliveries_worker_processed")
    .bind("webhook_rules")
    .bind(json!({
        "worker": "webhook_rule_worker",
        "delivery_count": outcomes.len(),
        "delivered_count": outcomes.iter().filter(|outcome| outcome.status == WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED).count(),
        "failed_count": outcomes.iter().filter(|outcome| outcome.status != WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED).count(),
        "deliveries": outcomes.iter().take(MAX_AUDIT_DELIVERY_ROWS).map(|outcome| json!({
            "id": outcome.id,
            "rule_id": outcome.rule_id,
            "rule_name": &outcome.rule_name,
            "event_kind": &outcome.event_kind,
            "event_id": &outcome.event_id,
            "status": &outcome.status,
            "attempt_count": outcome.attempt_count,
            "error": &outcome.error,
        })).collect::<Vec<_>>(),
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_permanent_failure_alert(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    delivery: &DeliveryRow,
    error: Option<String>,
) -> Result<()> {
    let reason = error
        .clone()
        .unwrap_or_else(|| "webhook delivery permanently failed".to_string());
    sqlx::query(
        r#"
        INSERT INTO fleet_alert_states (
            alert_id,
            state,
            reason
        )
        VALUES ($1, 'open', $2)
        ON CONFLICT (alert_id) DO UPDATE SET
            state = 'open',
            reason = EXCLUDED.reason,
            updated_at = now()
        "#,
    )
    .bind(format!("webhook_delivery:{}", delivery.id))
    .bind(reason)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO audit_logs (
            id, actor_id, action, target, command_hash, metadata
        )
        VALUES ($1, NULL, $2, $3, NULL, $4)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind("webhook.rule_delivery_permanently_failed")
    .bind(format!("webhook_delivery:{}", delivery.id))
    .bind(json!({
        "rule_id": delivery.rule_id,
        "rule_name": &delivery.rule_name,
        "event_kind": &delivery.event_kind,
        "event_id": &delivery.event_id,
        "error": error,
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn resolve_pruned_delivery_alerts(pool: &PgPool, pruned: &[PrunedDelivery]) -> Result<usize> {
    let alert_ids = pruned
        .iter()
        .filter(|delivery| delivery.status == WEBHOOK_RULE_DELIVERY_STATUS_PERMANENTLY_FAILED)
        .map(|delivery| format!("webhook_delivery:{}", delivery.id))
        .collect::<Vec<_>>();
    if alert_ids.is_empty() {
        return Ok(0);
    }
    let updated = sqlx::query(
        r#"
        UPDATE fleet_alert_states
        SET
            state = 'acknowledged',
            reason = 'webhook delivery evidence pruned by retention',
            updated_at = now()
        WHERE alert_id = ANY($1)
          AND state = 'open'
        "#,
    )
    .bind(&alert_ids)
    .execute(pool)
    .await?;
    Ok(updated.rows_affected() as usize)
}

fn retry_backoff_secs(attempt_count: i32) -> Option<i64> {
    let index = attempt_count.saturating_sub(1) as usize;
    RETRY_BACKOFF_SECS.get(index).copied()
}

#[allow(dead_code)]
async fn insert_prune_audit(
    pool: &PgPool,
    config: WebhookRuleWorkerConfig,
    pruned: &[PrunedDelivery],
    resolved_alerts: usize,
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
    .bind("webhook.rule_deliveries_pruned")
    .bind("webhook_rules")
    .bind(json!({
        "worker": "webhook_rule_worker",
        "retention_days": config.retention_days,
        "pruned_count": pruned.len(),
        "resolved_alert_count": resolved_alerts,
        "deliveries": pruned.iter().take(MAX_AUDIT_DELIVERY_ROWS).map(|delivery| json!({
            "id": delivery.id,
            "rule_id": delivery.rule_id,
            "status": &delivery.status,
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
        rule_id: row.try_get("rule_id")?,
        actor_id: row.try_get("actor_id")?,
        rule_name: row.try_get("rule_name")?,
        event_kind: row.try_get("event_kind")?,
        event_id: row.try_get("event_id")?,
        target: row.try_get("target")?,
        payload: payload.0,
        attempt_count: row.try_get("attempt_count")?,
    })
}

fn webhook_client(timeout_secs: u64) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs.clamp(1, 60)))
        .redirect(Policy::none())
        .build()
        .context("failed to build webhook rule client")
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

fn render_message(rule: &RuleRow, payload: &Value) -> Result<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhook_rule_worker_config_clamps_operational_bounds_and_validates_retention() {
        assert_eq!(
            WebhookRuleWorkerConfig::new(0, 0, 1, 0, 0).unwrap(),
            WebhookRuleWorkerConfig {
                delivery_limit: 1,
                materialize_limit: 1,
                retention_days: 1,
                retention_prune_limit: 1,
                webhook_timeout_secs: 1,
            }
        );
        assert_eq!(
            WebhookRuleWorkerConfig::new(10_000, 10_000, 3_650, 20_000, 120).unwrap(),
            WebhookRuleWorkerConfig {
                delivery_limit: 200,
                materialize_limit: 1000,
                retention_days: 3_650,
                retention_prune_limit: 10_000,
                webhook_timeout_secs: 60,
            }
        );
        assert!(WebhookRuleWorkerConfig::new(25, 100, 0, 1_000, 5).is_err());
        assert!(WebhookRuleWorkerConfig::new(25, 100, 3_651, 1_000, 5).is_err());
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

    #[test]
    fn candidate_uses_interval_predicate_and_aggregates_matches() {
        let rule = RuleRow {
            id: Uuid::nil(),
            actor_id: None,
            name: "edge interval".to_string(),
            expression: "interval.30sec && tag:edge".to_string(),
            target: "https://hooks.example/vpsman".to_string(),
            body_template: "{event.kind} {vps.id}".to_string(),
            cooldown_secs: 30,
        };
        let vps_rows = vec![
            VpsRow {
                id: "edge-a".to_string(),
                display_name: "edge-a".to_string(),
                status: "online".to_string(),
                tags: vec!["edge".to_string()],
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                internal_build_number: 1,
                stale_since: None,
                stale_reason: None,
                capabilities: json!({}),
            },
            VpsRow {
                id: "core-a".to_string(),
                display_name: "core-a".to_string(),
                status: "online".to_string(),
                tags: vec!["core".to_string()],
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                internal_build_number: 1,
                stale_since: None,
                stale_reason: None,
                capabilities: json!({}),
            },
        ];
        let candidate =
            delivery_candidate_for_rule(&rule, "interval.30sec", "interval.30sec:1", &vps_rows, 1)
                .unwrap()
                .unwrap();
        assert_eq!(candidate.matched_vps.len(), 1);
        assert_eq!(candidate.message, "interval.30sec edge-a");
    }
}
