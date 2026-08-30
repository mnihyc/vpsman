use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::{types::Json as SqlJson, Executor, PgPool, Postgres, Row, Transaction};
use tokio::time::Duration;
use uuid::Uuid;
use vpsman_common::{
    default_webhook_message, expression_matches, expression_referenced_events,
    expression_referenced_roots, expression_references_vps_rules,
    ordinal_admission_mask_has_exact_shape, parse_expression, parse_vps_rule_value, payload_hash,
    projected_telemetry_tunnel_identity, render_template_with_limit, validate_template,
    AgentMetrics, Expression, ExpressionContext, NetworkInterfacePolicy, NetworkInterfaceSource,
    ProjectedTelemetryTunnelIdentity, VpsMetadata, VpsRuleContext,
    VPS_RULE_KEY_NETWORK_RATE_INTERFACES, VPS_RULE_KEY_TRAFFIC_SELECTORS,
    WEBHOOK_RULE_DELIVERY_STATUS_CANCELED_DISABLED, WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED,
    WEBHOOK_RULE_DELIVERY_STATUS_FAILED, WEBHOOK_RULE_DELIVERY_STATUS_PERMANENTLY_FAILED,
};
use vpsman_server_core::prepare_webhook_target;

use crate::actor_authority::actor_authorized;
const DEFAULT_WEBHOOK_TIMEOUT_SECS: u64 = 5;
const MAX_ERROR_BYTES: usize = 1024;
const MAX_AUDIT_DELIVERY_ROWS: usize = 100;
const MAX_DELIVERY_ATTEMPTS: i32 = 4;
const RETRY_BACKOFF_SECS: [i64; 3] = [60, 5 * 60, 30 * 60];
const WEBHOOK_SIGNATURE_HEADER: &str = "X-Vpsman-Webhook-Signature";
const WEBHOOK_DELIVERY_HEADER: &str = "X-Vpsman-Webhook-Delivery";
const WEBHOOK_EVENT_HEADER: &str = "X-Vpsman-Webhook-Event";
const RULE_CONFIGURATION_EVENT_KIND: &str = "webhook.rule_configuration";
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
const INTERVAL_EVENTS: &[(&str, i64)] = &[
    ("interval.30sec", 30),
    ("interval.1min", 60),
    ("interval.5min", 5 * 60),
    ("interval.1h", 60 * 60),
];

type HmacSha256 = Hmac<Sha256>;

fn telemetry_webhook_source_rows(rule_count: usize, materialize_limit: i64) -> i64 {
    let rule_count = i64::try_from(rule_count).unwrap_or(i64::MAX).max(1);
    // Reuse the configured materialization transaction bound instead of
    // inventing a second operational threshold. A source is irreducible: all
    // enabled rules are evaluated atomically before its cursor advances.
    (materialize_limit.max(1) / rule_count).max(1)
}

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
    #[serde(skip)]
    retained_tombstone: bool,
    #[serde(skip)]
    vps_rules: VpsRuleContext,
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
    occurred_at_unix: i64,
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
    signing_secret: Option<String>,
    payload: Value,
    attempt_count: i32,
}

struct ClientAlertWebhookSendEligibilityRevision {
    eligibility: ClientAlertWebhookSendEligibility,
    revision: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ClientAlertWebhookSendEligibility {
    Deliverable,
    ClientSuspended,
    RuleDisabled,
    InvalidClientScope,
    LeaseLost,
}

impl ClientAlertWebhookSendEligibility {
    fn cancellation_reason(self) -> Option<&'static str> {
        match self {
            Self::ClientSuspended => Some("client_suspended"),
            Self::RuleDisabled => Some("webhook rule disabled"),
            Self::InvalidClientScope => Some("client_alert_scope_invalid"),
            Self::Deliverable | Self::LeaseLost => None,
        }
    }
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
    let mut run = process_webhook_periodic_maintenance(pool, config).await?;
    let events = process_webhook_event_materialization_work(pool, config).await?;
    let telemetry = process_telemetry_webhook_materialization_work(pool, config, &[]).await?;
    let deliveries = process_due_webhook_deliveries(pool, config).await?;
    run.materialized = run
        .materialized
        .saturating_add(events.materialized)
        .saturating_add(telemetry.materialized);
    run.processed = deliveries.processed;
    run.delivered = deliveries.delivered;
    run.failed = deliveries.failed;
    Ok(run)
}

/// Performs periodic, database-only webhook maintenance. Delivery HTTP is a
/// separate durable consumer and is never awaited by this producer.
pub(crate) async fn process_webhook_periodic_maintenance(
    pool: &PgPool,
    config: WebhookRuleWorkerConfig,
) -> Result<WebhookRuleWorkerRun> {
    Ok(WebhookRuleWorkerRun {
        materialized: materialize_interval_events(pool, config).await?,
        pruned: drain_webhook_retention(pool, config).await?,
        ..WebhookRuleWorkerRun::default()
    })
}

/// Drains only telemetry-owned webhook cursor work for a projection wake.
///
/// A non-empty client list is an exact, losslessly coalesced notification
/// scope. The overwhelmingly common no-enabled-rule path can therefore seek
/// those cursor primary keys directly instead of scanning the fleet. An empty
/// list is the immediate global fallback for an unrecognized projection
/// notice. Its independent worker lane also runs an empty-scope global scan on
/// every configured periodic recovery cycle.
///
/// Materialization transaction limits bound lock/WAL bursts, not wake
/// throughput: the cursor and delivery-row production drain to completion.
/// The independent delivery consumer owns all HTTP I/O.
pub(crate) async fn process_telemetry_webhook_materialization_work(
    pool: &PgPool,
    config: WebhookRuleWorkerConfig,
    client_ids: &[String],
) -> Result<WebhookRuleWorkerRun> {
    let materialized = if client_ids.is_empty() {
        drain_telemetry_projection_events(pool, config).await?
    } else {
        match drain_telemetry_projection_without_enabled_rules_for_clients(
            pool,
            config.materialize_limit,
            client_ids,
        )
        .await?
        {
            Some(()) => 0,
            None => drain_telemetry_projection_events(pool, config).await?,
        }
    };
    Ok(WebhookRuleWorkerRun {
        materialized,
        ..WebhookRuleWorkerRun::default()
    })
}

/// Drains durable lifecycle and outbox materialization. Limits inside the
/// called functions bound one transaction; they never cap a wake's throughput.
/// Telemetry cursors and delivery HTTP have independent consumers.
pub(crate) async fn process_webhook_event_materialization_work(
    pool: &PgPool,
    config: WebhookRuleWorkerConfig,
) -> Result<WebhookRuleWorkerRun> {
    let mut run = WebhookRuleWorkerRun::default();
    loop {
        project_alert_lifecycle_events(pool, config.materialize_limit).await?;
        let events = process_webhook_events(pool, config).await?;
        run.materialized = run.materialized.saturating_add(events);
        if !webhook_event_materialization_pending(pool).await? {
            return Ok(run);
        }
        tokio::task::yield_now().await;
    }
}

async fn webhook_event_materialization_pending(pool: &PgPool) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT
            EXISTS (
                SELECT 1
                FROM alert_lifecycle_events lifecycle
                WHERE NOT EXISTS (
                    SELECT 1
                    FROM alert_lifecycle_consumer_receipts receipt
                    WHERE receipt.consumer_kind='webhook'
                      AND receipt.event_seq=lifecycle.event_seq
                      AND receipt.status='completed'
                )
            )
            OR EXISTS (
                SELECT 1 FROM webhook_events WHERE processed_at IS NULL
            )
        "#,
    )
    .fetch_one(pool)
    .await?)
}

/// Drains only due delivery rows. Each row is leased before HTTP begins, so
/// this consumer can run independently from every materialization producer.
pub(crate) async fn process_due_webhook_deliveries(
    pool: &PgPool,
    config: WebhookRuleWorkerConfig,
) -> Result<WebhookRuleWorkerRun> {
    let mut run = WebhookRuleWorkerRun::default();
    loop {
        let (claimed, processed, delivered, failed) =
            process_queued_deliveries(pool, config).await?;
        run.processed = run.processed.saturating_add(processed);
        run.delivered = run.delivered.saturating_add(delivered);
        run.failed = run.failed.saturating_add(failed);
        if claimed == 0 {
            return Ok(run);
        }
    }
}

async fn drain_webhook_retention(pool: &PgPool, config: WebhookRuleWorkerConfig) -> Result<usize> {
    let mut pruned = 0_usize;
    loop {
        let page = prune_webhook_events(pool, config).await?;
        pruned = pruned.saturating_add(page);
        if page < config.retention_prune_limit as usize {
            break;
        }
        tokio::task::yield_now().await;
    }
    loop {
        let page = prune_deliveries(pool, config).await?;
        pruned = pruned.saturating_add(page);
        if page < config.retention_prune_limit as usize {
            return Ok(pruned);
        }
        tokio::task::yield_now().await;
    }
}

/// Returns `Some(())` after the exact client scope is fully acknowledged when
/// no rule is enabled. Returns `None` without acknowledging another row as
/// soon as an enabled-rule snapshot is observed, handing that work to the
/// existing repeatable-read materializer. Rule enable/disable commits thus
/// keep the same configuration boundary as the global path.
async fn drain_telemetry_projection_without_enabled_rules_for_clients(
    pool: &PgPool,
    materialize_limit: i64,
    client_ids: &[String],
) -> Result<Option<()>> {
    debug_assert!(!client_ids.is_empty());
    loop {
        let (no_enabled, candidates, advanced, notifications) =
            sqlx::query_as::<_, (bool, i64, i64, i64)>(
                r#"
            WITH configuration AS MATERIALIZED (
                SELECT NOT EXISTS (
                    SELECT 1 FROM webhook_rules WHERE enabled = TRUE
                ) AS no_enabled
            ), candidates AS MATERIALIZED (
                SELECT cursor.client_id, cursor.last_sample_seq,
                       head.projected_seq,
                       head.latest_projected_sample_id
                FROM telemetry_webhook_cursors cursor
                JOIN telemetry_projection_heads head USING (client_id)
                CROSS JOIN configuration
                WHERE configuration.no_enabled
                  AND cursor.client_id = ANY($2::TEXT[])
                  AND cursor.last_sample_seq < head.projected_seq
                ORDER BY head.accepted_at ASC, cursor.client_id ASC
                LIMIT $1
                FOR UPDATE OF cursor SKIP LOCKED
            ), sources AS MATERIALIZED (
                SELECT candidate.client_id, candidate.last_sample_seq,
                       candidate.projected_seq,
                       first_sample.observed_at
                            < now() - make_interval(days => $3)
                       AND first_sample.id IS DISTINCT FROM
                            candidate.latest_projected_sample_id
                       AND first_sample.accepted_seq <= core_minute.materialized_seq
                       AND first_sample.accepted_seq <= traffic_minute.materialized_seq
                            AS sample_prune_due
                FROM candidates candidate
                JOIN telemetry_samples first_sample
                  ON first_sample.client_id = candidate.client_id
                 AND first_sample.accepted_seq = candidate.last_sample_seq + 1
                JOIN telemetry_minute_materialization_heads core_minute
                  ON core_minute.client_id = candidate.client_id
                JOIN traffic_counter_minute_heads traffic_minute
                  ON traffic_minute.client_id = candidate.client_id
            ), source_completeness AS MATERIALIZED (
                SELECT
                    (SELECT count(*) FROM candidates) =
                    (SELECT count(*) FROM sources) AS complete
            ), advanced AS (
                UPDATE telemetry_webhook_cursors cursor
                SET last_sample_seq = source.projected_seq
                FROM sources source
                CROSS JOIN source_completeness completeness
                WHERE completeness.complete
                  AND cursor.client_id = source.client_id
                  AND cursor.last_sample_seq = source.last_sample_seq
                  AND EXISTS (
                        SELECT 1
                        FROM telemetry_projection_heads head
                        WHERE head.client_id = cursor.client_id
                          AND source.projected_seq <= head.projected_seq
                  )
                RETURNING source.sample_prune_due
            ), notification AS MATERIALIZED (
                SELECT pg_notify(
                    'vpsman_telemetry_retention',
                    json_build_object(
                        'owner', 'history_retention',
                        'effect', 'sample_prune_frontier_advanced'
                    )::text
                )
                FROM advanced
                WHERE sample_prune_due
                LIMIT 1
            )
            SELECT
                configuration.no_enabled,
                (SELECT count(*)::bigint FROM candidates),
                (SELECT count(*)::bigint FROM advanced),
                (SELECT count(*)::bigint FROM notification)
            FROM configuration
            "#,
            )
            .bind(materialize_limit.max(1))
            .bind(client_ids)
            .bind(vpsman_common::DEFAULT_TELEMETRY_SAMPLE_RETENTION_DAYS)
            .fetch_one(pool)
            .await?;
        if !no_enabled {
            return Ok(None);
        }
        anyhow::ensure!(
            candidates == advanced,
            "telemetry webhook cursor source sample is missing"
        );
        anyhow::ensure!(
            (0..=1).contains(&notifications),
            "telemetry webhook sample-prune notification was not statement-coalesced"
        );

        let pending = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM telemetry_webhook_cursors cursor
                JOIN telemetry_projection_heads head USING (client_id)
                WHERE cursor.client_id = ANY($1::TEXT[])
                  AND cursor.last_sample_seq < head.projected_seq
            )
            "#,
        )
        .bind(client_ids)
        .fetch_one(pool)
        .await?;
        if !pending {
            return Ok(Some(()));
        }
        tokio::task::yield_now().await;
    }
}

async fn drain_telemetry_projection_events(
    pool: &PgPool,
    config: WebhookRuleWorkerConfig,
) -> Result<usize> {
    let mut inserted = 0_usize;
    loop {
        inserted = inserted
            .checked_add(process_telemetry_projection_events(pool, config).await?)
            .context("telemetry webhook delivery count overflow")?;
        let pending = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM telemetry_webhook_cursors cursor
                JOIN telemetry_projection_heads head USING (client_id)
                WHERE cursor.last_sample_seq < head.projected_seq
            )
            "#,
        )
        .fetch_one(pool)
        .await?;
        if !pending {
            return Ok(inserted);
        }
        tokio::task::yield_now().await;
    }
}

async fn project_alert_lifecycle_events(pool: &PgPool, limit: i64) -> Result<usize> {
    let mut tx = pool.begin().await?;
    let page_limit = limit.clamp(1, 1000);
    sqlx::query(
        r#"
        INSERT INTO alert_lifecycle_consumer_receipts (
            consumer_kind, event_seq, status
        )
        SELECT 'webhook', lifecycle.event_seq, 'pending'
        FROM alert_lifecycle_events lifecycle
        WHERE NOT EXISTS (
            SELECT 1
            FROM alert_lifecycle_consumer_receipts receipt
            WHERE receipt.consumer_kind='webhook'
              AND receipt.event_seq=lifecycle.event_seq
        )
        ORDER BY lifecycle.event_seq
        LIMIT $1
        ON CONFLICT (consumer_kind,event_seq) DO NOTHING
        "#,
    )
    .bind(page_limit)
    .execute(&mut *tx)
    .await?;
    let claim_id = Uuid::new_v4();
    let rows = sqlx::query(
        r#"
        WITH candidate AS (
            SELECT receipt.event_seq
            FROM alert_lifecycle_consumer_receipts receipt
            WHERE receipt.consumer_kind='webhook'
              AND receipt.status IN ('pending','failed')
            ORDER BY receipt.event_seq
            FOR UPDATE SKIP LOCKED
            LIMIT $1
        ), claimed AS (
            UPDATE alert_lifecycle_consumer_receipts receipt
            SET status='in_progress', claim_id=$2,
                attempt_count=receipt.attempt_count+1,
                error=NULL, updated_at=clock_timestamp()
            FROM candidate
            WHERE receipt.consumer_kind='webhook'
              AND receipt.event_seq=candidate.event_seq
            RETURNING receipt.event_seq
        )
        SELECT lifecycle.event_seq, lifecycle.edge_kind, lifecycle.event_id,
               lifecycle.event_predicates, lifecycle.subject_client_ids,
               lifecycle.payload, lifecycle.occurred_at, lifecycle.causation_id,
               lifecycle.schedule_lineage
        FROM claimed
        JOIN alert_lifecycle_events lifecycle USING (event_seq)
        ORDER BY lifecycle.event_seq
        "#,
    )
    .bind(page_limit)
    .bind(claim_id)
    .fetch_all(&mut *tx)
    .await?;
    for row in &rows {
        let event_seq: i64 = row.try_get("event_seq")?;
        let kind: String = row.try_get("edge_kind")?;
        let event_id: String = row.try_get("event_id")?;
        let predicates: Vec<String> = row.try_get("event_predicates")?;
        let subjects: Vec<String> = row.try_get("subject_client_ids")?;
        let payload: Value = row.try_get::<SqlJson<Value>, _>("payload")?.0;
        let occurred_at: DateTime<Utc> = row.try_get("occurred_at")?;
        let causation_id: Option<Uuid> = row.try_get("causation_id")?;
        let lineage: Vec<Uuid> = row.try_get("schedule_lineage")?;
        insert_webhook_event_with_provenance_at_in_tx(
            &mut tx,
            &kind,
            &event_id,
            &predicates,
            &subjects,
            payload,
            occurred_at,
            Some(event_seq),
            causation_id,
            &lineage,
        )
        .await?;
        let projected = sqlx::query(
            r#"
            SELECT id, occurred_at
            FROM webhook_events
            WHERE alert_lifecycle_event_seq=$1
               OR (kind=$2 AND event_id=$3)
            ORDER BY (alert_lifecycle_event_seq=$1) DESC, occurred_at DESC, id DESC
            LIMIT 1
            "#,
        )
        .bind(event_seq)
        .bind(&kind)
        .bind(&event_id)
        .fetch_one(&mut *tx)
        .await?;
        let webhook_id: Uuid = projected.try_get("id")?;
        let webhook_occurred_at: DateTime<Utc> = projected.try_get("occurred_at")?;
        sqlx::query(
            r#"
            UPDATE webhook_events
            SET alert_lifecycle_event_seq=$2,
                causation_id=COALESCE(causation_id,$3),
                schedule_lineage=$4
            WHERE id=$1
              AND (alert_lifecycle_event_seq IS NULL OR alert_lifecycle_event_seq=$2)
            "#,
        )
        .bind(webhook_id)
        .bind(event_seq)
        .bind(causation_id)
        .bind(&lineage)
        .execute(&mut *tx)
        .await?;
        let acknowledged = sqlx::query(
            r#"
            UPDATE alert_lifecycle_consumer_receipts
            SET status='completed', claim_id=NULL,
                output_id=$3, output_occurred_at=$4,
                error=NULL, updated_at=clock_timestamp()
            WHERE consumer_kind='webhook' AND event_seq=$1
              AND status='in_progress' AND claim_id=$2
            "#,
        )
        .bind(event_seq)
        .bind(claim_id)
        .bind(webhook_id)
        .bind(webhook_occurred_at)
        .execute(&mut *tx)
        .await?;
        anyhow::ensure!(
            acknowledged.rows_affected() == 1,
            "webhook_lifecycle_receipt_claim_lost"
        );
    }
    tx.commit().await?;
    Ok(rows.len())
}

pub(crate) async fn materialize_interval_events(
    pool: &PgPool,
    config: WebhookRuleWorkerConfig,
) -> Result<usize> {
    let now = chrono::Utc::now().timestamp();
    let rules = list_enabled_rules(pool, config.materialize_limit).await?;
    if rules.is_empty() {
        return Ok(0);
    }
    let mut referenced_events = std::collections::HashSet::new();
    let mut invalid_rules = Vec::new();
    for rule in &rules {
        match validated_rule_expression(rule) {
            Ok(expression) => referenced_events.extend(expression_referenced_events(&expression)),
            Err(error) => invalid_rules.push((rule, format_delivery_error(&error))),
        }
    }

    let mut materialized = 0_usize;
    if !invalid_rules.is_empty() {
        let mut tx = pool.begin().await?;
        for (rule, error) in invalid_rules {
            if insert_rule_materialization_failure(&mut tx, rule, None, &error).await? {
                materialized += 1;
            }
        }
        tx.commit().await?;
    }
    for &(event_kind, bucket_secs) in INTERVAL_EVENTS {
        let event_id = format!("{event_kind}:{}", now - now.rem_euclid(bucket_secs));
        if !referenced_events.contains(event_kind) {
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
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await?;
    let rules = list_enabled_rules_in_tx(&mut tx, limit).await?;
    tx.commit().await?;
    Ok(rules)
}

async fn list_enabled_rules_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    limit: i64,
) -> Result<Vec<RuleRow>> {
    let page_limit = limit.clamp(1, 1000);
    let mut cursor = None;
    let mut rules = Vec::new();
    loop {
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
              AND ($1::uuid IS NULL OR id > $1)
            ORDER BY id ASC
            LIMIT $2
            "#,
        )
        .bind(cursor)
        .bind(page_limit)
        .fetch_all(&mut **tx)
        .await?;
        let page = rows
            .into_iter()
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
            .collect::<Result<Vec<_>>>()?;
        let next_cursor = next_enabled_rule_cursor(&page, page_limit as usize);
        rules.extend(page);
        let Some(next_cursor) = next_cursor else {
            break;
        };
        anyhow::ensure!(
            cursor.is_none_or(|previous| next_cursor > previous),
            "enabled webhook rule pagination cursor did not advance"
        );
        cursor = Some(next_cursor);
    }
    Ok(rules)
}

fn next_enabled_rule_cursor(page: &[RuleRow], page_limit: usize) -> Option<Uuid> {
    (page.len() == page_limit)
        .then(|| page.last().map(|rule| rule.id))
        .flatten()
}

fn validated_rule_expression(rule: &RuleRow) -> Result<Expression> {
    let expression = parse_expression(&rule.expression)
        .map_err(|error| anyhow::anyhow!("invalid webhook rule expression: {error}"))?
        .context("webhook rule expression is empty")?;
    if !rule.body_template.trim().is_empty() {
        validate_template(&rule.body_template)
            .map_err(|error| anyhow::anyhow!("invalid webhook rule template: {error}"))?;
    }
    Ok(expression)
}

async fn list_event_vps(
    tx: &mut Transaction<'_, Postgres>,
    include_vps_rules: bool,
    explicit_subject_client_ids: &[String],
) -> Result<Vec<VpsRow>> {
    let rows = sqlx::query(
        r#"
        SELECT
            c.id,
            c.display_name,
            c.status,
            host(c.registration_ip) AS registration_ip,
            host(c.last_ip) AS last_ip,
            c.last_seen_at::text AS last_seen_at,
            c.internal_build_number,
            c.stale_since::text AS stale_since,
            c.stale_reason,
            c.capabilities,
            c.hidden_at IS NOT NULL AS retained_tombstone,
            COALESCE(array_agg(t.name ORDER BY t.name) FILTER (WHERE t.name IS NOT NULL), ARRAY[]::TEXT[]) AS tags
        FROM clients c
        LEFT JOIN client_tags ct ON ct.client_id = c.id
        LEFT JOIN tags t ON t.id = ct.tag_id
        WHERE c.hidden_at IS NULL OR c.id = ANY($1::TEXT[])
        GROUP BY c.id, c.display_name, c.status, c.registration_ip, c.last_ip, c.last_seen_at, c.internal_build_number, c.stale_since, c.stale_reason, c.capabilities, c.hidden_at
        ORDER BY c.id
        "#,
    )
    .bind(explicit_subject_client_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut vps_rows = rows
        .into_iter()
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
                retained_tombstone: row.try_get("retained_tombstone")?,
                vps_rules: VpsRuleContext::default(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if include_vps_rules && !vps_rows.is_empty() {
        let client_ids = vps_rows
            .iter()
            .filter(|vps| !vps.retained_tombstone)
            .map(|vps| vps.id.clone())
            .collect::<Vec<_>>();
        let rows = if client_ids.is_empty() {
            Vec::new()
        } else {
            sqlx::query(
                r#"
            SELECT client_id, key, value_raw, value_json
            FROM vps_rule_values
            WHERE client_id = ANY($1::TEXT[])
            ORDER BY client_id ASC, key ASC
            "#,
            )
            .bind(&client_ids)
            .fetch_all(&mut **tx)
            .await?
        };
        let by_id = vps_rows
            .iter_mut()
            .map(|vps| (vps.id.clone(), &mut vps.vps_rules))
            .collect::<std::collections::HashMap<_, _>>();
        let mut by_id = by_id;
        for row in rows {
            let client_id: String = row.try_get("client_id")?;
            if let Some(context) = by_id.get_mut(&client_id) {
                let value_json: SqlJson<Value> = row.try_get("value_json")?;
                insert_persisted_vps_rule(
                    context,
                    row.try_get::<String, _>("key")?,
                    row.try_get::<String, _>("value_raw")?,
                    value_json.0,
                )?;
            }
        }
    }
    Ok(vps_rows)
}

fn insert_persisted_vps_rule(
    context: &mut VpsRuleContext,
    key: String,
    value_raw: String,
    stored_json: Value,
) -> Result<()> {
    let parsed = parse_vps_rule_value(&key, &value_raw)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("invalid persisted VPS rule {key}"))?;
    if key == VPS_RULE_KEY_NETWORK_RATE_INTERFACES {
        anyhow::ensure!(
            parsed.json == stored_json,
            "network_rate_selector_storage_invalid"
        );
    } else if key == VPS_RULE_KEY_TRAFFIC_SELECTORS {
        anyhow::ensure!(
            parsed.json == stored_json,
            "traffic_selector_storage_invalid"
        );
    }
    context.insert(key, parsed.raw, parsed.json);
    Ok(())
}

pub(crate) async fn insert_webhook_event(
    pool: &PgPool,
    kind: &str,
    event_id: &str,
    event_predicates: &[&str],
    subject_client_ids: &[String],
    payload: Value,
) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let predicates = event_predicates
        .iter()
        .map(|predicate| (*predicate).to_string())
        .collect::<Vec<_>>();
    let inserted = insert_webhook_event_in_tx(
        &mut tx,
        kind,
        event_id,
        &predicates,
        subject_client_ids,
        payload,
    )
    .await?;
    tx.commit().await?;
    Ok(inserted)
}

pub(crate) async fn insert_webhook_event_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    kind: &str,
    event_id: &str,
    event_predicates: &[String],
    subject_client_ids: &[String],
    payload: Value,
) -> Result<bool> {
    insert_webhook_event_at_in_tx(
        tx,
        kind,
        event_id,
        event_predicates,
        subject_client_ids,
        payload,
        Utc::now(),
    )
    .await
}

/// Inserts an event at its authoritative source time. Operational alert
/// lifecycle edges use this entry point so a worker source transaction has the
/// same occurrence-time and exact-dedupe contract as the API lifecycle owner.
pub(crate) async fn insert_webhook_event_at_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    kind: &str,
    event_id: &str,
    event_predicates: &[String],
    subject_client_ids: &[String],
    payload: Value,
    occurred_at: DateTime<Utc>,
) -> Result<bool> {
    insert_webhook_event_with_provenance_at_in_tx(
        tx,
        kind,
        event_id,
        event_predicates,
        subject_client_ids,
        payload,
        occurred_at,
        None,
        None,
        &[],
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert_webhook_event_with_provenance_at_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    kind: &str,
    event_id: &str,
    event_predicates: &[String],
    subject_client_ids: &[String],
    payload: Value,
    occurred_at: DateTime<Utc>,
    alert_lifecycle_event_seq: Option<i64>,
    causation_id: Option<Uuid>,
    schedule_lineage: &[Uuid],
) -> Result<bool> {
    let predicate_refs = event_predicates
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let predicates = normalize_event_predicates(kind, &predicate_refs);
    let lock_name = format!("vpsman:webhook-event:{kind}:{event_id}");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(lock_name)
        .execute(&mut **tx)
        .await?;
    // The ordinary table is the durable event outbox. Its exact event key is
    // serialized here; bounded retention owns only processed rows and never
    // takes a relation-wide DDL lock against producers.
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
            alert_lifecycle_event_seq,
            causation_id,
            schedule_lineage
        )
        SELECT $1, $2, $3, $4, $5, $6, $7::timestamptz, $8, $9, $10
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
    .bind(alert_lifecycle_event_seq)
    .bind(causation_id)
    .bind(schedule_lineage)
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

async fn mark_webhook_event_processed_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    event: &EventRow,
) -> Result<()> {
    let updated = sqlx::query(
        "UPDATE webhook_events SET processed_at = now() WHERE id = $1 AND processed_at IS NULL",
    )
    .bind(event.id)
    .execute(&mut **tx)
    .await?;
    anyhow::ensure!(
        updated.rows_affected() == 1,
        "webhook event processing cursor did not advance exactly once"
    );
    Ok(())
}

async fn notify_sample_prune_frontier_advanced_in_tx(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<()> {
    sqlx::query(
        r#"
        SELECT pg_notify(
            'vpsman_telemetry_retention',
            json_build_object(
                'owner', 'history_retention',
                'effect', 'sample_prune_frontier_advanced'
            )::text
        )
        "#,
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn advance_telemetry_webhook_cursor_without_valid_rules_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    materialize_limit: i64,
) -> Result<()> {
    // With no valid consumer, advance a bounded set of webhook-owned cursor
    // rows in one statement without deserializing payload history. Canonical
    // acceptance and telemetry projection never lock these rows.
    let (candidates, advanced, notifications) = sqlx::query_as::<_, (i64, i64, i64)>(
        r#"
        WITH candidates AS MATERIALIZED (
            SELECT cursor.client_id, cursor.last_sample_seq,
                   head.projected_seq,
                   head.latest_projected_sample_id
            FROM telemetry_webhook_cursors cursor
            JOIN telemetry_projection_heads head USING (client_id)
            WHERE cursor.last_sample_seq < head.projected_seq
            ORDER BY head.accepted_at ASC, cursor.client_id ASC
            LIMIT $1
            FOR UPDATE OF cursor SKIP LOCKED
        ), sources AS MATERIALIZED (
            SELECT candidate.client_id, candidate.last_sample_seq,
                   candidate.projected_seq,
                   first_sample.observed_at
                        < now() - make_interval(days => $2)
                   AND first_sample.id IS DISTINCT FROM
                        candidate.latest_projected_sample_id
                   AND first_sample.accepted_seq <= core_minute.materialized_seq
                   AND first_sample.accepted_seq <= traffic_minute.materialized_seq
                        AS sample_prune_due
            FROM candidates candidate
            JOIN telemetry_samples first_sample
              ON first_sample.client_id = candidate.client_id
             AND first_sample.accepted_seq = candidate.last_sample_seq + 1
            JOIN telemetry_minute_materialization_heads core_minute
              ON core_minute.client_id = candidate.client_id
            JOIN traffic_counter_minute_heads traffic_minute
              ON traffic_minute.client_id = candidate.client_id
        ), advanced AS (
            UPDATE telemetry_webhook_cursors cursor
            SET last_sample_seq = source.projected_seq
            FROM sources source
            WHERE cursor.client_id = source.client_id
              AND cursor.last_sample_seq = source.last_sample_seq
              AND EXISTS (
                    SELECT 1
                    FROM telemetry_projection_heads head
                    WHERE head.client_id = cursor.client_id
                      AND source.projected_seq <= head.projected_seq
              )
            RETURNING source.sample_prune_due
        ), notification AS MATERIALIZED (
            SELECT pg_notify(
                'vpsman_telemetry_retention',
                json_build_object(
                    'owner', 'history_retention',
                    'effect', 'sample_prune_frontier_advanced'
                )::text
            )
            FROM advanced
            WHERE sample_prune_due
            LIMIT 1
        )
        SELECT
            (SELECT count(*)::bigint FROM candidates),
            (SELECT count(*)::bigint FROM advanced),
            (SELECT count(*)::bigint FROM notification)
        "#,
    )
    .bind(materialize_limit.max(1))
    .bind(vpsman_common::DEFAULT_TELEMETRY_SAMPLE_RETENTION_DAYS)
    .fetch_one(&mut **tx)
    .await?;
    anyhow::ensure!(
        candidates == advanced,
        "telemetry webhook cursor source sample is missing"
    );
    anyhow::ensure!(
        (0..=1).contains(&notifications),
        "telemetry webhook sample-prune notification was not statement-coalesced"
    );
    Ok(())
}

/// Advances the telemetry cursor in one READ COMMITTED statement only when
/// that statement's configuration snapshot contains no enabled webhook rule.
/// The committed projection frontier is the source authority even when there
/// is no delivery consumer: projection publishes first, then this independent
/// owner advances and deterministically prunes their jointly consumed queue
/// prefix. Canonical acceptance may remain ahead without becoming webhook work.
async fn try_advance_telemetry_webhook_cursor_without_enabled_rules(
    pool: &PgPool,
    materialize_limit: i64,
) -> Result<bool> {
    let (no_enabled, candidates, advanced, notifications) =
        sqlx::query_as::<_, (bool, i64, i64, i64)>(
            r#"
        WITH configuration AS MATERIALIZED (
            SELECT NOT EXISTS (
                SELECT 1 FROM webhook_rules WHERE enabled = TRUE
            ) AS no_enabled
        ), candidates AS MATERIALIZED (
            SELECT cursor.client_id, cursor.last_sample_seq,
                   head.projected_seq,
                   head.latest_projected_sample_id
            FROM telemetry_webhook_cursors cursor
            JOIN telemetry_projection_heads head USING (client_id)
            CROSS JOIN configuration
            WHERE configuration.no_enabled
              AND cursor.last_sample_seq < head.projected_seq
            ORDER BY head.accepted_at ASC, cursor.client_id ASC
            LIMIT $1
            FOR UPDATE OF cursor SKIP LOCKED
        ), sources AS MATERIALIZED (
            SELECT candidate.client_id, candidate.last_sample_seq,
                   candidate.projected_seq,
                   first_sample.observed_at
                        < now() - make_interval(days => $2)
                   AND first_sample.id IS DISTINCT FROM
                        candidate.latest_projected_sample_id
                   AND first_sample.accepted_seq <= core_minute.materialized_seq
                   AND first_sample.accepted_seq <= traffic_minute.materialized_seq
                        AS sample_prune_due
            FROM candidates candidate
            JOIN telemetry_samples first_sample
              ON first_sample.client_id = candidate.client_id
             AND first_sample.accepted_seq = candidate.last_sample_seq + 1
            JOIN telemetry_minute_materialization_heads core_minute
              ON core_minute.client_id = candidate.client_id
            JOIN traffic_counter_minute_heads traffic_minute
              ON traffic_minute.client_id = candidate.client_id
        ), source_completeness AS MATERIALIZED (
            SELECT
                (SELECT count(*) FROM candidates) =
                (SELECT count(*) FROM sources) AS complete
        ), advanced AS (
            UPDATE telemetry_webhook_cursors cursor
            SET last_sample_seq = source.projected_seq
            FROM sources source
            CROSS JOIN source_completeness completeness
            WHERE completeness.complete
              AND cursor.client_id = source.client_id
              AND cursor.last_sample_seq = source.last_sample_seq
              AND EXISTS (
                SELECT 1
                FROM telemetry_projection_heads head
                WHERE head.client_id = cursor.client_id
                      AND source.projected_seq <= head.projected_seq
              )
            RETURNING source.sample_prune_due
        ), notification AS MATERIALIZED (
            SELECT pg_notify(
                'vpsman_telemetry_retention',
                json_build_object(
                    'owner', 'history_retention',
                    'effect', 'sample_prune_frontier_advanced'
                )::text
            )
            FROM advanced
            WHERE sample_prune_due
            LIMIT 1
        )
        SELECT
            configuration.no_enabled,
            (SELECT count(*)::bigint FROM candidates),
            (SELECT count(*)::bigint FROM advanced),
            (SELECT count(*)::bigint FROM notification)
        FROM configuration
        "#,
        )
        .bind(materialize_limit.max(1))
        .bind(vpsman_common::DEFAULT_TELEMETRY_SAMPLE_RETENTION_DAYS)
        .fetch_one(pool)
        .await?;
    anyhow::ensure!(
        candidates == advanced,
        "telemetry webhook cursor source sample is missing"
    );
    anyhow::ensure!(
        (0..=1).contains(&notifications),
        "telemetry webhook sample-prune notification was not statement-coalesced"
    );
    Ok(no_enabled)
}

/// Materializes telemetry webhook deliveries directly from the canonical
/// telemetry cursor.  The immutable telemetry sample is already the durable
/// replay source, so copying its full JSON into `webhook_events` only to delete
/// it again adds heap, TOAST, WAL, and worker wake amplification without adding
/// a recovery boundary.  Non-telemetry events continue to use the ordinary
/// outbox below.
async fn process_telemetry_projection_events(
    pool: &PgPool,
    config: WebhookRuleWorkerConfig,
) -> Result<usize> {
    let (mut tx, rules) = loop {
        if try_advance_telemetry_webhook_cursor_without_enabled_rules(
            pool,
            config.materialize_limit,
        )
        .await?
        {
            return Ok(0);
        }

        let mut tx = pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *tx)
            .await?;
        let rules = list_enabled_rules_in_tx(&mut tx, config.materialize_limit).await?;
        if rules.is_empty() {
            // A rule may be disabled between the read-committed probe and this
            // snapshot. Retry the single-statement no-rule path rather than
            // updating a hot telemetry head from a repeatable-read snapshot.
            tx.rollback().await?;
            continue;
        }
        break (tx, rules);
    };
    let mut inserted = 0_usize;
    let mut validated_rules = Vec::with_capacity(rules.len());
    for rule in &rules {
        match validated_rule_expression(rule) {
            Ok(expression) => validated_rules.push((rule, expression)),
            Err(error) => {
                let error = format_delivery_error(&error);
                if insert_rule_materialization_failure(&mut tx, rule, None, &error).await? {
                    inserted += 1;
                }
            }
        }
    }

    if validated_rules.is_empty() {
        // Enabled-but-invalid rules retain the existing repeatable-read
        // transaction so their durable materialization failures and cursor
        // advancement remain one coherent result.
        advance_telemetry_webhook_cursor_without_valid_rules_in_tx(
            &mut tx,
            config.materialize_limit,
        )
        .await?;
        tx.commit().await?;
        return Ok(inserted);
    }

    let source_rows =
        telemetry_webhook_source_rows(validated_rules.len(), config.materialize_limit);

    // Lock the same bounded candidate set used below and prove its exact next
    // canonical row exists before materializing anything. The per-row check
    // below proves the remainder of every returned prefix is contiguous; this
    // preflight also makes a completely missing source fail closed instead of
    // looking like an idle sweep.
    let missing_source_client = sqlx::query_scalar::<_, String>(
        r#"
        WITH candidate_heads AS MATERIALIZED (
            SELECT cursor.client_id, cursor.last_sample_seq
            FROM telemetry_webhook_cursors cursor
            JOIN telemetry_projection_heads head USING (client_id)
            WHERE cursor.last_sample_seq < head.projected_seq
            ORDER BY head.accepted_at ASC, cursor.client_id ASC
            LIMIT $1
            FOR UPDATE OF cursor SKIP LOCKED
        )
        SELECT head.client_id
        FROM candidate_heads head
        WHERE NOT EXISTS (
            SELECT 1
            FROM telemetry_samples sample
            WHERE sample.client_id = head.client_id
              AND sample.accepted_seq = head.last_sample_seq + 1
        )
        ORDER BY head.client_id
        LIMIT 1
        "#,
    )
    .bind(source_rows)
    .fetch_optional(&mut *tx)
    .await?;
    anyhow::ensure!(
        missing_source_client.is_none(),
        "telemetry webhook cursor source sample is missing for client {}",
        missing_source_client.as_deref().unwrap_or_default()
    );

    let rows = sqlx::query(
        r#"
        WITH candidate_heads AS MATERIALIZED (
            SELECT
                cursor.client_id,
                cursor.last_sample_seq AS initial_cursor_seq,
                head.projected_seq AS final_projected_seq,
                head.accepted_at AS head_accepted_at,
                head.latest_projected_sample_id,
                core_minute.materialized_seq AS core_minute_seq,
                traffic_minute.materialized_seq AS traffic_minute_seq
            FROM telemetry_webhook_cursors cursor
            JOIN telemetry_projection_heads head USING (client_id)
            JOIN telemetry_minute_materialization_heads core_minute USING (client_id)
            JOIN traffic_counter_minute_heads traffic_minute USING (client_id)
            WHERE cursor.last_sample_seq < head.projected_seq
            ORDER BY head.accepted_at ASC, cursor.client_id ASC
            LIMIT $1
            FOR UPDATE OF cursor SKIP LOCKED
        ), current_tunnels AS MATERIALIZED (
            SELECT
                tunnel.client_id,
                jsonb_agg(
                    jsonb_build_object(
                        'plan_id', tunnel.telemetry_plan_id,
                        'plan_name', tunnel.telemetry_plan_name,
                        'interface', tunnel.interface,
                        'kind', tunnel.kind,
                        'endpoint_side', tunnel.telemetry_endpoint_side,
                        'peer_client_id', tunnel.telemetry_peer_client_id
                    )
                    ORDER BY tunnel.interface COLLATE "C"
                ) AS identities
            FROM telemetry_current_tunnels tunnel
            JOIN candidate_heads head
              ON head.client_id = tunnel.client_id
            GROUP BY tunnel.client_id
        ), managed_tunnel_interfaces AS MATERIALIZED (
            SELECT endpoint.client_id,
                   array_agg(
                       DISTINCT endpoint.interface COLLATE "C"
                       ORDER BY endpoint.interface COLLATE "C"
                   ) AS interfaces
            FROM (
                SELECT plan.left_client_id AS client_id,
                       plan.plan ->> 'interface_name' AS interface
                FROM tunnel_plans plan
                JOIN candidate_heads head
                  ON head.client_id = plan.left_client_id
                WHERE plan.enabled IS TRUE
                  AND plan.deleted_at IS NULL
                UNION ALL
                SELECT plan.right_client_id AS client_id,
                       plan.plan ->> 'interface_name' AS interface
                FROM tunnel_plans plan
                JOIN candidate_heads head
                  ON head.client_id = plan.right_client_id
                WHERE plan.enabled IS TRUE
                  AND plan.deleted_at IS NULL
            ) endpoint
            GROUP BY endpoint.client_id
        )
        SELECT
            sample.id,
            sample.client_id,
            sample.accepted_seq,
            sample.accepted_at,
            sample.payload,
            sample.source_gateway_id,
            sample.source_gateway_session_id,
            sample.source_process_incarnation_id,
            sample.source_telemetry_seq,
            sample.reported_observed_unix,
            sample.network_admission_mask,
            sample.tunnel_admission_mask,
            interface_policy.value_json AS network_interface_rule,
            COALESCE(
                current_tunnels.identities,
                '[]'::JSONB
            ) AS current_tunnel_identities,
            COALESCE(
                managed_tunnel_interfaces.interfaces,
                ARRAY[]::TEXT[]
            ) AS managed_tunnel_interfaces,
            head.initial_cursor_seq,
            sample.accepted_seq = head.initial_cursor_seq + 1
              AND sample.observed_at
                    < now() - make_interval(days => $2)
              AND sample.id IS DISTINCT FROM head.latest_projected_sample_id
              AND sample.accepted_seq <= head.core_minute_seq
              AND sample.accepted_seq <= head.traffic_minute_seq
                AS sample_prune_due
        FROM candidate_heads head
        JOIN telemetry_samples sample
          ON sample.client_id = head.client_id
         AND sample.accepted_seq > head.initial_cursor_seq
         AND sample.accepted_seq <= head.final_projected_seq
        LEFT JOIN vps_rule_values interface_policy
          ON interface_policy.client_id = sample.client_id
         AND interface_policy.key = 'network.interfaces'
        LEFT JOIN current_tunnels
          ON current_tunnels.client_id = sample.client_id
        LEFT JOIN managed_tunnel_interfaces
          ON managed_tunnel_interfaces.client_id = sample.client_id
        ORDER BY head.head_accepted_at ASC, sample.client_id ASC,
                 sample.accepted_seq ASC
        LIMIT $1
        "#,
    )
    .bind(source_rows)
    .bind(vpsman_common::DEFAULT_TELEMETRY_SAMPLE_RETENTION_DAYS)
    .fetch_all(&mut *tx)
    .await?;
    if rows.is_empty() {
        tx.commit().await?;
        return Ok(inserted);
    }

    let mut events = Vec::with_capacity(rows.len());
    let mut advances = std::collections::BTreeMap::<String, (i64, i64)>::new();
    let mut sample_prune_due = false;
    for row in rows {
        let client_id: String = row.try_get("client_id")?;
        let accepted_seq: i64 = row.try_get("accepted_seq")?;
        let initial_cursor_seq: i64 = row.try_get("initial_cursor_seq")?;
        let accepted_at: DateTime<Utc> = row.try_get("accepted_at")?;
        let advance = advances
            .entry(client_id.clone())
            .or_insert((initial_cursor_seq, initial_cursor_seq));
        anyhow::ensure!(
            advance.0 == initial_cursor_seq && accepted_seq == advance.1.saturating_add(1),
            "telemetry webhook cursor source sequence is not contiguous"
        );
        advance.1 = accepted_seq;
        sample_prune_due |= row.try_get::<bool, _>("sample_prune_due")?;
        let event = telemetry_event_from_projection_row(&row, &client_id, accepted_at)?;
        events.push(event);
    }

    let include_vps_rules = validated_rules
        .iter()
        .any(|(_, expression)| expression_references_vps_rules(expression));
    let mut explicit_subject_client_ids = events
        .iter()
        .flat_map(|event| event.subject_client_ids.iter().cloned())
        .collect::<Vec<_>>();
    explicit_subject_client_ids.sort();
    explicit_subject_client_ids.dedup();
    let vps_rows = list_event_vps(&mut tx, include_vps_rules, &explicit_subject_client_ids).await?;

    for event in events {
        for (rule, expression) in &validated_rules {
            match event_candidate_for_validated_rule(rule, expression, &event, &vps_rows) {
                Ok(Some(candidate)) => {
                    if insert_delivery_candidate(&mut tx, &candidate).await? {
                        inserted += 1;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    let error = format_delivery_error(&error);
                    if insert_rule_materialization_failure(&mut tx, rule, Some(&event), &error)
                        .await?
                    {
                        inserted += 1;
                    }
                }
            }
        }
    }
    let client_ids = advances.keys().cloned().collect::<Vec<_>>();
    let initial_sequences = advances
        .values()
        .map(|advance| advance.0)
        .collect::<Vec<_>>();
    let final_sequences = advances
        .values()
        .map(|advance| advance.1)
        .collect::<Vec<_>>();
    let advanced = sqlx::query(
        r#"
        WITH advances AS (
            SELECT *
            FROM UNNEST(
                $1::TEXT[],
                $2::BIGINT[],
                $3::BIGINT[]
            ) AS row(client_id, initial_seq, final_seq)
        )
        UPDATE telemetry_webhook_cursors cursor
        SET last_sample_seq = advance.final_seq
        FROM advances advance
        WHERE cursor.client_id = advance.client_id
          AND cursor.last_sample_seq = advance.initial_seq
          AND EXISTS (
                SELECT 1
                FROM telemetry_projection_heads head
                WHERE head.client_id = cursor.client_id
                  AND advance.final_seq <= head.projected_seq
          )
        "#,
    )
    .bind(&client_ids)
    .bind(&initial_sequences)
    .bind(&final_sequences)
    .execute(&mut *tx)
    .await?;
    anyhow::ensure!(
        advanced.rows_affected() == advances.len() as u64,
        "telemetry webhook cursor did not advance exactly once per client"
    );
    if sample_prune_due {
        notify_sample_prune_frontier_advanced_in_tx(&mut tx).await?;
    }
    tx.commit().await?;
    Ok(inserted)
}

fn telemetry_event_from_projection_row(
    row: &sqlx::postgres::PgRow,
    client_id: &str,
    accepted_at: DateTime<Utc>,
) -> Result<EventRow> {
    let metrics: SqlJson<AgentMetrics> = row.try_get("payload")?;
    let mut metrics = metrics.0;
    metrics.observed_unix = u64::try_from(row.try_get::<i64, _>("reported_observed_unix")?)
        .context("negative telemetry webhook reported observation time")?;
    let interface_rule: Option<SqlJson<Value>> = row.try_get("network_interface_rule")?;
    let interface_policy =
        NetworkInterfacePolicy::from_rule_json(interface_rule.as_ref().map(|rule| &rule.0))
            .map_err(anyhow::Error::msg)
            .context("invalid persisted network.interfaces rule")?;
    let network_admission_mask: Vec<u8> = row.try_get("network_admission_mask")?;
    let tunnel_admission_mask: Vec<u8> = row.try_get("tunnel_admission_mask")?;
    let current_tunnel_identities = row
        .try_get::<SqlJson<Vec<ProjectedTelemetryTunnelIdentity>>, _>("current_tunnel_identities")?
        .0
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    let managed_tunnel_interfaces = row
        .try_get::<Vec<String>, _>("managed_tunnel_interfaces")?
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    let interfaces = telemetry_webhook_interfaces(
        &metrics,
        &interface_policy,
        &network_admission_mask,
        &tunnel_admission_mask,
        &current_tunnel_identities,
        &managed_tunnel_interfaces,
    )?;
    let gateway_id: String = row.try_get("source_gateway_id")?;
    let gateway_session_id: Uuid = row.try_get("source_gateway_session_id")?;
    let process_incarnation_id: Uuid = row.try_get("source_process_incarnation_id")?;
    let telemetry_seq = u64::try_from(row.try_get::<i64, _>("source_telemetry_seq")?)
        .context("negative telemetry webhook source sequence")?;
    let mut predicates = vec!["telemetry.rollup".to_string()];
    if !interfaces.networks.is_empty() {
        predicates.push("telemetry.network_rate".to_string());
    }
    if !metrics.tunnels.is_empty() {
        predicates.push("telemetry.tunnel".to_string());
    }
    if !metrics.tunnel_reachability.is_empty() {
        predicates.push("network.reachability".to_string());
    }
    predicates.sort();
    predicates.dedup();
    let disk = telemetry_persistent_disk_totals(&metrics);
    let network_rx = telemetry_sum_u64(interfaces.networks.iter().map(|row| row.rx_bytes));
    let network_tx = telemetry_sum_u64(interfaces.networks.iter().map(|row| row.tx_bytes));
    let event_id = format!(
        "telemetry:{client_id}:{gateway_session_id}:{process_incarnation_id}:{telemetry_seq}"
    );
    Ok(EventRow {
        id: row.try_get("id")?,
        actor_id: None,
        kind: "telemetry.rollup".to_string(),
        event_id: event_id.clone(),
        event_predicates: predicates.clone(),
        subject_client_ids: vec![client_id.to_string()],
        payload: json!({
            "event": {
                "kind": "telemetry.rollup",
                "id": &event_id,
                "predicates": &predicates,
            },
            "telemetry": {
                "client_id": client_id,
                "gateway_id": gateway_id,
                "observed_unix": metrics.observed_unix,
                "hostname": &metrics.hostname,
                "uptime_secs": metrics.uptime_secs,
                "disk_collection_available": disk.is_some(),
                "disk_total_bytes": disk.map(|(total, _)| total),
                "disk_available_bytes": disk.map(|(_, available)| available),
                "network_rx_bytes": network_rx,
                "network_tx_bytes": network_tx,
                "network_count": interfaces.networks.len(),
                "tunnel_count": metrics.tunnels.len(),
                "networks": &interfaces.networks,
                "tunnels": &interfaces.tunnels,
            },
        }),
        occurred_at_unix: accepted_at.timestamp(),
    })
}

fn telemetry_persistent_disk_totals(metrics: &AgentMetrics) -> Option<(i64, i64)> {
    metrics
        .has_persistent_block_filesystem_disk_sample()
        .then(|| {
            (
                telemetry_sum_u64(metrics.disks.iter().map(|disk| disk.total_bytes)),
                telemetry_sum_u64(metrics.disks.iter().map(|disk| disk.available_bytes)),
            )
        })
}

struct TelemetryWebhookInterfaces<'a> {
    networks: Vec<&'a vpsman_common::NetworkStat>,
    tunnels: Vec<Value>,
}

/// Shapes only network-byte content. Tunnel presence and operational fields
/// remain in the event so network.interfaces cannot alter tunnel lifecycle or
/// reachability semantics.
fn telemetry_webhook_interfaces<'a>(
    metrics: &'a AgentMetrics,
    policy: &NetworkInterfacePolicy,
    network_admission_mask: &[u8],
    tunnel_admission_mask: &[u8],
    current_tunnel_identities: &std::collections::HashSet<ProjectedTelemetryTunnelIdentity>,
    managed_tunnel_interfaces: &std::collections::HashSet<String>,
) -> Result<TelemetryWebhookInterfaces<'a>> {
    let network_mask_is_exact =
        ordinal_admission_mask_has_exact_shape(network_admission_mask, metrics.networks.len());
    let tunnel_mask_is_exact =
        ordinal_admission_mask_has_exact_shape(tunnel_admission_mask, metrics.tunnels.len());
    let networks = metrics
        .networks
        .iter()
        .enumerate()
        .filter(|(ordinal, network)| {
            network_mask_is_exact
                && ordinal_admitted(network_admission_mask, *ordinal)
                && policy.matches(NetworkInterfaceSource::Host, &network.interface)
                && !(*policy == NetworkInterfacePolicy::DefaultPhysical
                    && managed_tunnel_interfaces.contains(&network.interface))
        })
        .map(|(_, network)| network)
        .collect::<Vec<_>>();
    let tunnels = metrics
        .tunnels
        .iter()
        .enumerate()
        .map(|(ordinal, tunnel)| {
            let mut value = serde_json::to_value(tunnel)?;
            if !tunnel_mask_is_exact
                || !ordinal_admitted(tunnel_admission_mask, ordinal)
                || !projected_telemetry_tunnel_identity(tunnel)
                    .is_some_and(|identity| current_tunnel_identities.contains(&identity))
                || !policy.matches(NetworkInterfaceSource::Tunnel, &tunnel.interface)
            {
                let object = value
                    .as_object_mut()
                    .context("serialized telemetry tunnel is not an object")?;
                for field in [
                    "rx_bytes",
                    "tx_bytes",
                    "traffic_source",
                    "traffic_status",
                    "traffic_reason",
                    "traffic_checked_unix",
                ] {
                    object.remove(field);
                }
            }
            Ok(value)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(TelemetryWebhookInterfaces { networks, tunnels })
}

fn ordinal_admitted(mask: &[u8], ordinal: usize) -> bool {
    mask.get(ordinal / 8)
        .is_some_and(|byte| byte & (1_u8 << (ordinal % 8)) != 0)
}

fn telemetry_sum_u64(values: impl Iterator<Item = u64>) -> i64 {
    values
        .fold(0_u128, |total, value| total.saturating_add(value as u128))
        .min(i64::MAX as u128) as i64
}

pub(crate) async fn process_webhook_events(
    pool: &PgPool,
    config: WebhookRuleWorkerConfig,
) -> Result<usize> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await?;
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
    let rules = list_enabled_rules_in_tx(&mut tx, config.materialize_limit).await?;
    let mut inserted = 0_usize;
    let mut validated_rules = Vec::with_capacity(rules.len());
    for rule in &rules {
        match validated_rule_expression(rule) {
            Ok(expression) => validated_rules.push((rule, expression)),
            Err(error) => {
                let error = format_delivery_error(&error);
                if insert_rule_materialization_failure(&mut tx, rule, None, &error).await? {
                    inserted += 1;
                }
            }
        }
    }
    let include_vps_rules = validated_rules
        .iter()
        .any(|(_, expression)| expression_references_vps_rules(expression));
    let mut explicit_subject_client_ids = rows
        .iter()
        .map(|row| row.try_get::<Vec<String>, _>("subject_client_ids"))
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    explicit_subject_client_ids.sort();
    explicit_subject_client_ids.dedup();
    let vps_rows = list_event_vps(&mut tx, include_vps_rules, &explicit_subject_client_ids).await?;
    for row in rows {
        let event = event_from_row(row)?;
        for (rule, expression) in &validated_rules {
            match event_candidate_for_validated_rule(rule, expression, &event, &vps_rows) {
                Ok(Some(candidate)) => {
                    if insert_delivery_candidate(&mut tx, &candidate).await? {
                        inserted += 1;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    let error = format_delivery_error(&error);
                    if insert_rule_materialization_failure(&mut tx, rule, Some(&event), &error)
                        .await?
                    {
                        inserted += 1;
                    }
                }
            }
        }
        mark_webhook_event_processed_in_tx(&mut tx, &event).await?;
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

#[cfg(test)]
fn event_candidate_for_rule(
    rule: &RuleRow,
    event: &EventRow,
    vps_rows: &[VpsRow],
) -> Result<Option<DeliveryCandidate>> {
    let expression = validated_rule_expression(rule)?;
    event_candidate_for_validated_rule(rule, &expression, event, vps_rows)
}

fn event_candidate_for_validated_rule(
    rule: &RuleRow,
    expression: &Expression,
    event: &EventRow,
    vps_rows: &[VpsRow],
) -> Result<Option<DeliveryCandidate>> {
    let subject_ids = event
        .subject_client_ids
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    let matched_vps = if subject_ids.is_empty() && generic_alert_lifecycle_edge(&event.kind) {
        if expression_references_vps(expression)
            || !expression_matches(&expression_context_for_subjectless_event(event), expression)
        {
            return Ok(None);
        }
        Vec::new()
    } else {
        let requires_live_vps = expression_requires_live_vps(expression);
        let matched = vps_rows
            .iter()
            .filter(|vps| subject_ids.is_empty() || subject_ids.contains(&vps.id))
            .filter(|vps| {
                !vps.retained_tombstone || (!subject_ids.is_empty() && !requires_live_vps)
            })
            .filter(|vps| {
                let context = expression_context_for_event(vps, event);
                expression_matches(&context, expression)
            })
            .cloned()
            .collect::<Vec<_>>();
        if matched.is_empty() {
            return Ok(None);
        }
        matched
    };
    let referenced_roots = expression_referenced_roots(expression)
        .into_iter()
        .collect::<Vec<_>>();
    let referenced_events = expression_referenced_events(expression)
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
        occurred_at_unix: event.occurred_at_unix,
        cooldown_until_unix: event.occurred_at_unix.saturating_add(rule.cooldown_secs),
    }))
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum VpsExpressionDependency {
    None,
    StableIdentity,
    LiveState,
}

fn expression_requires_live_vps(expression: &Expression) -> bool {
    expression_vps_dependency(expression) == VpsExpressionDependency::LiveState
}

fn expression_references_vps(expression: &Expression) -> bool {
    expression_vps_dependency(expression) != VpsExpressionDependency::None
}

fn expression_vps_dependency(expression: &Expression) -> VpsExpressionDependency {
    use vpsman_common::Predicate;

    match expression {
        Expression::Predicate(Predicate::Event(_)) => VpsExpressionDependency::None,
        Expression::Predicate(Predicate::Comparison { field, .. })
        | Expression::Predicate(Predicate::Membership { field, .. }) => match field.as_str() {
            "vps.id" => VpsExpressionDependency::StableIdentity,
            field if field.starts_with("vps.") => VpsExpressionDependency::LiveState,
            _ => VpsExpressionDependency::None,
        },
        Expression::Predicate(Predicate::Bare(_) | Predicate::Untagged) => {
            VpsExpressionDependency::LiveState
        }
        Expression::Not(inner) => expression_vps_dependency(inner),
        Expression::And(left, right) | Expression::Or(left, right) => {
            expression_vps_dependency(left).max(expression_vps_dependency(right))
        }
    }
}

fn expression_context_for_event(vps: &VpsRow, event: &EventRow) -> ExpressionContext {
    let context = ExpressionContext::for_vps(VpsMetadata {
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
    .with_vps_rules(vps.vps_rules.clone());
    expression_context_with_event(context, event)
}

fn expression_context_for_subjectless_event(event: &EventRow) -> ExpressionContext {
    expression_context_with_event(ExpressionContext::default(), event)
}

fn expression_context_with_event(
    mut context: ExpressionContext,
    event: &EventRow,
) -> ExpressionContext {
    context = context.with_event_predicate(&event.kind);
    for predicate in &event.event_predicates {
        context = context.with_event_predicate(predicate);
    }
    for root in EVENT_EXPRESSION_ROOTS {
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

fn client_scoped_alert_trigger_ids(event_kind: &str, matched_vps: &[VpsRow]) -> Vec<String> {
    if event_kind != "alert.triggered" {
        return Vec::new();
    }
    let mut client_ids = matched_vps
        .iter()
        .map(|vps| vps.id.clone())
        .collect::<Vec<_>>();
    client_ids.sort();
    client_ids.dedup();
    client_ids
}

async fn client_alert_trigger_materialization_cancellation_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    candidate: &DeliveryCandidate,
) -> Result<Option<&'static str>> {
    let client_ids = client_scoped_alert_trigger_ids(&candidate.event_kind, &candidate.matched_vps);
    if client_ids.is_empty() {
        return Ok(None);
    }
    // Materialization records the event against this transaction's coherent
    // subject snapshot. It does not own client lifecycle rows: the delivery
    // consumer revalidates current suspension immediately before HTTP and its
    // eligibility revision fences completion against a concurrent change.
    let subjects =
        sqlx::query("SELECT id, status FROM clients WHERE id=ANY($1::text[]) ORDER BY id")
            .bind(&client_ids)
            .fetch_all(&mut **tx)
            .await?;
    if subjects.len() != client_ids.len() {
        return Ok(Some("client_alert_scope_invalid"));
    }
    if subjects
        .iter()
        .any(|row| row.get::<String, _>("status") == "suspended")
    {
        return Ok(Some("client_suspended"));
    }
    let source_suppressed = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM alert_lifecycle_events lifecycle
            JOIN alert_episodes episode
              ON episode.id=lifecycle.episode_id
             AND episode.trigger_generation=lifecycle.trigger_generation
            WHERE lifecycle.edge_kind='alert.triggered'
              AND lifecycle.event_id=$1
              AND episode.evidence#>>'{_vpsman_client_suspension,client_id}'
                    = ANY($2::text[])
        )
        "#,
    )
    .bind(&candidate.event_id)
    .bind(&client_ids)
    .fetch_one(&mut **tx)
    .await?;
    Ok(source_suppressed.then_some("client_suspended"))
}

async fn insert_delivery_candidate(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    candidate: &DeliveryCandidate,
) -> Result<bool> {
    let rule_enabled = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM webhook_rules WHERE id = $1 AND enabled = TRUE FOR UPDATE",
    )
    .bind(candidate.rule_id)
    .fetch_optional(&mut **tx)
    .await?
    .is_some();
    if !rule_enabled {
        return Ok(false);
    }
    let cancellation_reason =
        client_alert_trigger_materialization_cancellation_in_tx(tx, candidate).await?;
    let (duplicate, latest_cooldown_until_unix) = sqlx::query_as::<_, (bool, i64)>(
        r#"
        SELECT
            EXISTS (
                SELECT 1
                FROM webhook_rule_deliveries
                WHERE rule_id = $1 AND event_id = $2
            ),
            COALESCE((
                SELECT cooldown_until_unix
                FROM webhook_rule_deliveries
                WHERE rule_id = $1
                ORDER BY cooldown_until_unix DESC
                LIMIT 1
            ), 0)
        "#,
    )
    .bind(candidate.rule_id)
    .bind(&candidate.event_id)
    .fetch_one(&mut **tx)
    .await?;
    if delivery_candidate_is_suppressed(
        duplicate,
        latest_cooldown_until_unix,
        candidate.occurred_at_unix,
        &candidate.event_kind,
    ) {
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
        VALUES ($1, $2, $3, $4, $5, $13, $6, $7, $8, $9, $10, $14, $11, 0, NULL, NULL, $12, NULL)
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
    .bind(if cancellation_reason.is_some() {
        WEBHOOK_RULE_DELIVERY_STATUS_CANCELED_DISABLED
    } else {
        "queued"
    })
    .bind(cancellation_reason)
    .execute(&mut **tx)
    .await?;
    Ok(inserted.rows_affected() > 0)
}

async fn insert_rule_materialization_failure(
    tx: &mut Transaction<'_, Postgres>,
    rule: &RuleRow,
    event: Option<&EventRow>,
    error: &str,
) -> Result<bool> {
    let rule_enabled = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM webhook_rules WHERE id = $1 AND enabled = TRUE FOR UPDATE",
    )
    .bind(rule.id)
    .fetch_optional(&mut **tx)
    .await?
    .is_some();
    if !rule_enabled {
        return Ok(false);
    }

    let error = truncate_error(error);
    let (event_kind, event_id, occurred_at_unix, actor_id, source_event) = match event {
        Some(event) => (
            event.kind.clone(),
            event.event_id.clone(),
            event.occurred_at_unix.max(0),
            event.actor_id.or(rule.actor_id),
            Some(json!({
                "kind": &event.kind,
                "id": &event.event_id,
                "predicates": &event.event_predicates,
                "occurred_at_unix": event.occurred_at_unix,
            })),
        ),
        None => (
            RULE_CONFIGURATION_EVENT_KIND.to_string(),
            rule_configuration_failure_event_id(rule),
            Utc::now().timestamp().max(0),
            rule.actor_id,
            None,
        ),
    };
    let payload = json!({
        "schema": "vpsman.webhook_rule.materialization_failure.v1",
        "rule": {
            "id": rule.id,
            "name": &rule.name,
            "expression": &rule.expression,
        },
        "event": source_event,
        "failure": {
            "phase": if event.is_some() { "event_materialization" } else { "configuration_validation" },
            "error": &error,
        },
    });
    let dedupe_fingerprint = json!({
        "rule_id": rule.id,
        "event_kind": &event_kind,
        "event_id": &event_id,
        "failure": "materialization",
    });
    let dedupe_hash = payload_hash(dedupe_fingerprint.to_string().as_bytes());
    let delivery_id = Uuid::new_v4();
    let matched_vps = Vec::<VpsRow>::new();
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
        VALUES ($1, $2, $3, $4, $5, 'permanently_failed', $6, $7, $8, $9, $10, $11, $12, 0, NULL, NULL, $13, NULL)
        ON CONFLICT (rule_id, event_id) DO NOTHING
        "#,
    )
    .bind(delivery_id)
    .bind(rule.id)
    .bind(&rule.name)
    .bind(&event_kind)
    .bind(&event_id)
    .bind(&rule.target)
    .bind(format!("webhook-rule-failure:{}", &dedupe_hash[..32]))
    .bind(SqlJson(&payload))
    .bind(SqlJson(&matched_vps))
    .bind("Webhook rule delivery could not be materialized")
    .bind(&error)
    .bind(occurred_at_unix)
    .bind(actor_id)
    .execute(&mut **tx)
    .await?;
    if inserted.rows_affected() == 0 {
        return Ok(false);
    }

    let delivery = DeliveryRow {
        id: delivery_id,
        rule_id: rule.id,
        actor_id,
        rule_name: rule.name.clone(),
        event_kind,
        event_id,
        target: rule.target.clone(),
        signing_secret: None,
        payload,
        attempt_count: 0,
    };
    insert_permanent_failure_audit(tx, &delivery, Some(error)).await?;
    Ok(true)
}

fn rule_configuration_failure_event_id(rule: &RuleRow) -> String {
    let fingerprint = json!({
        "rule_id": rule.id,
        "expression": &rule.expression,
        "body_template": &rule.body_template,
    });
    let hash = payload_hash(fingerprint.to_string().as_bytes());
    format!("webhook-rule-configuration:{}", &hash[..32])
}

fn delivery_candidate_is_suppressed(
    duplicate: bool,
    latest_cooldown_until_unix: i64,
    occurred_at_unix: i64,
    event_kind: &str,
) -> bool {
    duplicate
        || (!alert_lifecycle_edge(event_kind) && latest_cooldown_until_unix > occurred_at_unix)
}

fn alert_lifecycle_edge(event_kind: &str) -> bool {
    matches!(event_kind, "alert.triggered" | "alert.resolved")
}

fn generic_alert_lifecycle_edge(event_kind: &str) -> bool {
    matches!(event_kind, "alert.triggered" | "alert.resolved")
}

async fn begin_client_alert_webhook_send(
    pool: &PgPool,
    delivery_id: Uuid,
    lease_id: Uuid,
) -> Result<ClientAlertWebhookSendEligibilityRevision> {
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
    .fetch_optional(pool)
    .await?
    else {
        return Ok(ClientAlertWebhookSendEligibilityRevision {
            eligibility: ClientAlertWebhookSendEligibility::LeaseLost,
            revision: None,
        });
    };
    let eligibility = if !row.try_get::<bool, _>("lease_owned")? {
        ClientAlertWebhookSendEligibility::LeaseLost
    } else if !row.try_get::<bool, _>("rule_enabled")? {
        ClientAlertWebhookSendEligibility::RuleDisabled
    } else if !row.try_get::<bool, _>("scope_exact")? {
        ClientAlertWebhookSendEligibility::InvalidClientScope
    } else if row.try_get::<bool, _>("subject_suspended")?
        || row.try_get::<bool, _>("source_suppressed")?
    {
        ClientAlertWebhookSendEligibility::ClientSuspended
    } else {
        ClientAlertWebhookSendEligibility::Deliverable
    };
    let revision: Option<i64> = row.try_get("eligibility_revision")?;
    anyhow::ensure!(
        eligibility != ClientAlertWebhookSendEligibility::Deliverable || revision.is_some(),
        "webhook delivery eligibility revision was not armed"
    );
    Ok(ClientAlertWebhookSendEligibilityRevision {
        eligibility,
        revision,
    })
}

async fn process_queued_deliveries(
    pool: &PgPool,
    config: WebhookRuleWorkerConfig,
) -> Result<(usize, usize, usize, usize)> {
    let lease_secs = delivery_lease_secs(config.webhook_timeout_secs);
    let mut claimed = 0_usize;
    let mut outcomes = Vec::new();
    // The configured limit bounds only one audit/scheduling page. Each HTTP
    // attempt acquires its own row immediately before processing, so later
    // deliveries never wait while carrying a lease they cannot yet use.
    for _ in 0..config.delivery_limit {
        let lease_id = Uuid::new_v4();
        let Some(row) = sqlx::query(
            r#"
            WITH claim AS (
                SELECT delivery.id, rule.signing_secret
                FROM webhook_rule_deliveries delivery
                LEFT JOIN webhook_rules rule ON rule.id = delivery.rule_id
                WHERE (
                        (
                            delivery.status IN ('queued', 'failed')
                            AND (delivery.next_attempt_at IS NULL OR delivery.next_attempt_at <= now())
                        )
                        OR (
                            delivery.status = 'in_progress'
                            AND delivery.delivery_lease_until < now()
                        )
                      )
                ORDER BY delivery.created_at ASC, delivery.id ASC
                LIMIT 1
                FOR UPDATE OF delivery SKIP LOCKED
            )
            UPDATE webhook_rule_deliveries delivery
            SET status = 'in_progress',
                error = NULL,
                delivery_lease_id = $1,
                delivery_lease_until = now() + make_interval(secs => $2::integer),
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
                claim.signing_secret,
                delivery.payload,
                delivery.attempt_count
            "#,
        )
        .bind(lease_id)
        .bind(lease_secs)
        .fetch_optional(pool)
        .await?
        else {
            break;
        };
        claimed = claimed.saturating_add(1);
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
        let send_eligibility = begin_client_alert_webhook_send(pool, delivery.id, lease_id).await?;
        if send_eligibility.eligibility != ClientAlertWebhookSendEligibility::Deliverable {
            let cancellation_reason = send_eligibility.eligibility.cancellation_reason();
            let updated = match cancellation_reason {
                Some(reason) => {
                    cancel_claimed_webhook_rule_delivery(pool, delivery.id, lease_id, reason).await
                }
                None => Ok(None),
            };
            let Some(recorded_attempt_count) = updated? else {
                continue;
            };
            let cancellation_reason =
                cancellation_reason.expect("canceled client alert webhook must have a reason");
            outcomes.push(DeliveryOutcome {
                id: delivery.id,
                rule_id: delivery.rule_id,
                rule_name: delivery.rule_name,
                event_kind: delivery.event_kind,
                event_id: delivery.event_id,
                status: WEBHOOK_RULE_DELIVERY_STATUS_CANCELED_DISABLED.to_string(),
                attempt_count: recorded_attempt_count,
                error: Some(cancellation_reason.to_string()),
            });
            continue;
        }
        let eligibility_revision = send_eligibility.revision;
        let actor_authorized =
            actor_authorized(pool, delivery.actor_id, "operator", &["integrations:write"]).await?;
        let result = if actor_authorized {
            deliver_webhook(&delivery, config.webhook_timeout_secs).await
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
                Some(format_delivery_error(&error)),
                None,
            ),
            Err(error) => (
                WEBHOOK_RULE_DELIVERY_STATUS_FAILED,
                Some(format_delivery_error(&error)),
                retry_backoff_secs(next_attempt_count),
            ),
        };
        let completion = complete_webhook_rule_delivery_on_pool(
            pool,
            &delivery,
            lease_id,
            eligibility_revision,
            status,
            error.as_deref(),
            next_attempt_after_secs,
        )
        .await;
        let Some(recorded_attempt_count) = completion? else {
            continue;
        };
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
    Ok((claimed, outcomes.len(), delivered, failed))
}

fn delivery_lease_secs(webhook_timeout_secs: u64) -> i32 {
    let per_attempt = i64::try_from(webhook_timeout_secs).unwrap_or(i64::MAX);
    per_attempt
        .clamp(1, 60)
        .saturating_add(60)
        .clamp(60, i32::MAX as i64) as i32
}

async fn cancel_claimed_webhook_rule_delivery<'e, E>(
    executor: E,
    delivery_id: Uuid,
    lease_id: Uuid,
    reason: &str,
) -> Result<Option<i32>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_scalar::<_, i32>(
        r#"
        UPDATE webhook_rule_deliveries
        SET status='canceled_disabled', error=$3,
            delivery_lease_id=NULL, delivery_lease_until=NULL,
            next_attempt_at=NULL, delivered_at=NULL
        WHERE id=$1 AND status='in_progress' AND delivery_lease_id=$2
        RETURNING attempt_count
        "#,
    )
    .bind(delivery_id)
    .bind(lease_id)
    .bind(reason)
    .fetch_optional(executor)
    .await?)
}

async fn complete_webhook_rule_delivery_on_pool(
    pool: &PgPool,
    delivery: &DeliveryRow,
    lease_id: Uuid,
    eligibility_revision: Option<i64>,
    status: &str,
    error: Option<&str>,
    next_attempt_after_secs: Option<i64>,
) -> Result<Option<i32>> {
    let mut tx = pool.begin().await?;
    let result = complete_webhook_rule_delivery_in_tx(
        &mut tx,
        delivery,
        lease_id,
        eligibility_revision,
        status,
        error,
        next_attempt_after_secs,
    )
    .await?;
    tx.commit().await?;
    Ok(result)
}

async fn complete_webhook_rule_delivery_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    delivery: &DeliveryRow,
    lease_id: Uuid,
    eligibility_revision: Option<i64>,
    status: &str,
    error: Option<&str>,
    next_attempt_after_secs: Option<i64>,
) -> Result<Option<i32>> {
    let attempt_count = sqlx::query_scalar::<_, i32>(
        r#"
        UPDATE webhook_rule_deliveries
        SET
            status = $3,
            error = $4,
            attempt_count = attempt_count + 1,
            next_attempt_at = CASE
                WHEN $5::bigint IS NULL THEN NULL
                ELSE now() + ($5::bigint * interval '1 second')
            END,
            last_attempt_at = now(),
            delivered_at = CASE WHEN $3 = 'delivered' THEN now() ELSE NULL END,
            delivery_lease_id = NULL,
            delivery_lease_until = NULL
        WHERE id = $1
          AND status = 'in_progress'
          AND delivery_lease_id = $2
          AND ($6::bigint IS NULL OR eligibility_revision=$6)
        RETURNING attempt_count
        "#,
    )
    .bind(delivery.id)
    .bind(lease_id)
    .bind(status)
    .bind(error)
    .bind(next_attempt_after_secs)
    .bind(eligibility_revision)
    .fetch_optional(&mut **tx)
    .await?;
    if attempt_count.is_some() && status == WEBHOOK_RULE_DELIVERY_STATUS_PERMANENTLY_FAILED {
        insert_permanent_failure_audit(tx, delivery, error.map(ToOwned::to_owned)).await?;
    }
    Ok(attempt_count)
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

async fn deliver_webhook(delivery: &DeliveryRow, webhook_timeout_secs: u64) -> Result<()> {
    let timeout = Duration::from_secs(webhook_timeout_secs.clamp(1, 60));
    tokio::time::timeout(timeout, async {
        let target = prepare_webhook_target(&delivery.target, timeout).await?;
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

fn webhook_signature(secret: &str, body: &[u8]) -> Result<String> {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).context("invalid webhook signing secret")?;
    mac.update(body);
    Ok(format!(
        "sha256={}",
        hex::encode(mac.finalize().into_bytes())
    ))
}

async fn prune_webhook_events(pool: &PgPool, config: WebhookRuleWorkerConfig) -> Result<usize> {
    let rows = sqlx::query(
        r#"
        WITH candidates AS (
            SELECT occurred_at, id
            FROM webhook_events event
            WHERE processed_at IS NOT NULL
              AND occurred_at <= now() - ($1::bigint * interval '1 day')
            ORDER BY occurred_at ASC, id ASC
            LIMIT $2
            FOR UPDATE OF event SKIP LOCKED
        )
        DELETE FROM webhook_events events
        USING candidates
        WHERE events.id = candidates.id
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
    insert_webhook_event_prune_audit(pool, config, rows.len()).await?;
    Ok(rows.len())
}

async fn insert_webhook_event_prune_audit(
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
    .bind("webhook.events_pruned")
    .bind("webhook_events")
    .bind(json!({
        "worker": "webhook_rule_worker",
        "origin_kind": "worker",
        "component": "webhook-rule-worker",
        "result": "succeeded",
        "retention_days": config.retention_days,
        "pruned_count": pruned_count,
    }))
    .execute(pool)
    .await?;
    Ok(())
}

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
    insert_prune_audit(pool, config, &pruned).await?;
    Ok(pruned.len())
}

async fn insert_process_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    outcomes: &[DeliveryOutcome],
) -> Result<()> {
    let delivered_count = outcomes
        .iter()
        .filter(|outcome| outcome.status == WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED)
        .count();
    let failed_count = outcomes.len().saturating_sub(delivered_count);
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
        "origin_kind": "worker",
        "component": "webhook-rule-worker",
        "delivery_count": outcomes.len(),
        "delivered_count": delivered_count,
        "failed_count": failed_count,
        "result": if failed_count == 0 { "succeeded" } else { "partial" },
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

async fn insert_permanent_failure_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    delivery: &DeliveryRow,
    error: Option<String>,
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
    .bind("webhook.rule_delivery_permanently_failed")
    .bind(format!("webhook_delivery:{}", delivery.id))
    .bind(json!({
        "rule_id": delivery.rule_id,
        "rule_name": &delivery.rule_name,
        "event_kind": &delivery.event_kind,
        "event_id": &delivery.event_id,
        "origin_kind": "worker",
        "component": "webhook-rule-worker",
        "result": "failed",
        "error": error,
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn retry_backoff_secs(attempt_count: i32) -> Option<i64> {
    let index = attempt_count.saturating_sub(1) as usize;
    RETRY_BACKOFF_SECS.get(index).copied()
}

async fn insert_prune_audit(
    pool: &PgPool,
    config: WebhookRuleWorkerConfig,
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
    .bind("webhook.rule_deliveries_pruned")
    .bind("webhook_rules")
    .bind(json!({
        "worker": "webhook_rule_worker",
        "origin_kind": "worker",
        "component": "webhook-rule-worker",
        "result": "succeeded",
        "retention_days": config.retention_days,
        "pruned_count": pruned.len(),
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
        signing_secret: row.try_get("signing_secret")?,
        payload: payload.0,
        attempt_count: row.try_get("attempt_count")?,
    })
}

fn truncate_error(error: &str) -> String {
    error.chars().take(MAX_ERROR_BYTES).collect()
}

fn format_delivery_error(error: &anyhow::Error) -> String {
    truncate_error(&format!("{error:#}"))
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
#[path = "tests_webhook_rules.rs"]
mod tests;
