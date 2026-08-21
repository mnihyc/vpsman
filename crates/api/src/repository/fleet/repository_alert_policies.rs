use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{postgres::PgRow, types::Json as SqlJson, Row};
use uuid::Uuid;
use vpsman_common::{
    expression_references_vps_rules,
    parse_persisted_vps_rule_value as parse_common_persisted_vps_rule_value,
    parse_vps_rule_value as parse_common_vps_rule_value, Expression, ExpressionTruth,
    ParsedVpsRuleValue, VpsRuleContext,
};

use crate::{
    model::{AgentView, AuditLogView, AuthContext, TelemetryRollupView, TelemetrySampleView},
    model_alert_notifications::FleetAlertNotificationMatchRule,
    model_alert_policies::{
        AlertPolicyCorrelationMode, AlertPolicyMetaCondition, AlertPolicyRuleKind,
        CreateFleetAlertPolicyRequest, NetworkRateInterfaceSelection, PolicyAlertQuery,
        PolicyAlertRecord, PolicyDryRunRequest, PolicyDryRunResponse, PolicyDryRunRulePreview,
        PolicyGroupRecord, PolicyRuleRecord, PolicyRuleRequest, PolicyRuleStateRecord,
        TrafficAccountingQuery, TrafficAccountingRecord, TrafficAccountingSelectorBreakdown,
        TrafficCounterRollupRecord, TrafficCounterSampleRecord, VpsRuleChangePreview, VpsRuleQuery,
        VpsRuleValueRecord, VpsRulesBulkUnsetRequest, VpsRulesBulkUpsertRequest,
        VpsRulesDryRunRequest, VpsRulesDryRunResponse, VPS_RULE_KEY_BILLING_CYCLE,
        VPS_RULE_KEY_BILLING_PRICE, VPS_RULE_KEY_NETWORK_RATE_INTERFACES,
        VPS_RULE_KEY_TRAFFIC_QUOTA_RX, VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL,
        VPS_RULE_KEY_TRAFFIC_QUOTA_TX, VPS_RULE_KEY_TRAFFIC_RESET_DAY,
        VPS_RULE_KEY_TRAFFIC_SELECTORS,
    },
    model_monitoring::TrafficHistoryPointView,
    model_webhook_rules::WebhookEventCandidate,
    repository::{MemoryState, Repository},
    repository_key_lifecycle::{
        lock_postgres_agent_identity_lifecycle, require_visible_memory_clients,
        require_visible_postgres_clients_in_tx,
    },
    repository_network_traffic_import::{
        is_intentional_vnstat_import_boundary, is_vnstat_import_source,
    },
    repository_operational_alerts::notification_rule_matches_alert,
    repository_webhook_rules::webhook_event_row,
    selector_expression::{
        agent_matches_selector_expression_with_rules, parse_selector_expression,
        vps_rule_contexts_by_client,
    },
    unix_now,
    util::{
        compare_timestamps_desc, parse_timestamp_unix, parse_timestamp_utc,
        timestamp_in_optional_bounds,
    },
};

const MAX_POLICY_NAME_BYTES: usize = 128;
const MAX_POLICY_NOTES_BYTES: usize = 1024;
const MAX_RULE_NAME_BYTES: usize = 128;
const MAX_SELECTOR_EXPRESSION_BYTES: usize = 4096;
const MAX_CONDITION_EXPRESSION_BYTES: usize = 4096;
const TRAFFIC_SAMPLE_STALE_SECS: i64 = 900;
const POLICY_WEBHOOK_REPAIR_WINDOW_SECS: i64 = 3600;
const POLICY_ALERT_TRIGGERED: &str = "triggered";
const POLICY_ALERT_PERSISTING: &str = "persisting";
const POLICY_ALERT_UNKNOWN: &str = "unknown";
const POLICY_ALERT_RESOLVED: &str = "resolved";
const MAX_POLICY_ALERT_CANDIDATE_ROWS: usize = 201;
static POLICY_EVALUATION_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PolicyAlertSelectionMode {
    History,
    CurrentFleet,
    ConfirmedActive,
}

type PolicyVpsRuleInput = (String, String, Value);

fn policy_alert_severity_rank(severity: &str) -> usize {
    match severity {
        "critical" => 0,
        "warning" => 1,
        "info" => 2,
        _ => 3,
    }
}

fn policy_alert_lifecycle_rank(lifecycle_state: &str) -> usize {
    match lifecycle_state {
        POLICY_ALERT_TRIGGERED | POLICY_ALERT_PERSISTING => 0,
        _ => 1,
    }
}

fn policy_alert_matches_selection(
    alert: &PolicyAlertRecord,
    mode: PolicyAlertSelectionMode,
) -> bool {
    match mode {
        PolicyAlertSelectionMode::History => true,
        PolicyAlertSelectionMode::CurrentFleet => {
            alert.resolved_at.is_none() && alert.last_confirmed_at.is_some()
        }
        PolicyAlertSelectionMode::ConfirmedActive => {
            alert.resolved_at.is_none()
                && alert.last_confirmed_at.is_some()
                && matches!(
                    alert.lifecycle_state.as_str(),
                    POLICY_ALERT_TRIGGERED | POLICY_ALERT_PERSISTING
                )
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct TrafficSelector {
    source: String,
    interface: String,
    direction: String,
    canonical: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum NetworkRateSelectorSpec {
    All,
    Exact(Vec<TrafficSelector>),
    Reference(NetworkRateSelectorReference),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NetworkRateSelectorReference {
    TrafficSelectors,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct TrafficStreamRequest {
    client_id: String,
    source_kind: String,
    interface: String,
    cycle_start_unix: i64,
}

const NO_RESET_TRAFFIC_START_UNIX: i64 = 0;

// Current monthly cycles use the transactionally maintained hourly transition
// ledger for completed UTC hours and scan at most the current raw hour. The
// coverage revision is part of the same transaction as every raw mutation. If
// coverage is absent or deliberately marked dirty, this statement selects the
// original raw-LAG oracle for the complete request set instead of returning a
// potentially stale partial result.
pub(crate) const MONTHLY_TRAFFIC_COUNTER_USAGE_SQL: &str = r#"
    WITH requested AS MATERIALIZED (
        SELECT client_id, source_kind, interface, cycle_start_unix
        FROM UNNEST(
            $1::text[],
            $2::text[],
            $3::text[],
            $4::bigint[]
        ) AS request(
            client_id,
            source_kind,
            interface,
            cycle_start_unix
        )
    ),
    coverage AS MATERIALIZED (
        SELECT NOT EXISTS (
            SELECT 1
            FROM requested
            WHERE to_timestamp(cycle_start_unix) <> date_bin(
                interval '1 hour',
                to_timestamp(cycle_start_unix),
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
            )
        ) AND NOT EXISTS (
            SELECT 1
            FROM requested
            JOIN LATERAL (
                SELECT 1 AS present
                FROM traffic_counter_samples sample
                WHERE sample.client_id = requested.client_id
                  AND sample.source_kind = requested.source_kind
                  AND sample.interface = requested.interface
                  AND sample.observed_at <= to_timestamp($5)
                ORDER BY sample.observed_at DESC
                LIMIT 1
            ) existing ON TRUE
            LEFT JOIN traffic_counter_hourly_usage_streams streams
              ON streams.client_id = requested.client_id
             AND streams.source_kind = requested.source_kind
             AND streams.interface = requested.interface
            WHERE streams.client_id IS NULL
               OR streams.source_revision <> streams.materialized_revision
        ) AS valid
    ),
    fast_latest AS (
        SELECT
            requested.client_id,
            requested.source_kind,
            requested.interface,
            requested.cycle_start_unix,
            sample.rx_bytes AS latest_rx,
            sample.tx_bytes AS latest_tx,
            EXTRACT(EPOCH FROM sample.observed_at)::bigint AS last_sample_unix
        FROM requested
        JOIN coverage ON coverage.valid
        JOIN LATERAL (
            SELECT sample.observed_at, sample.rx_bytes, sample.tx_bytes
            FROM traffic_counter_samples sample
            WHERE sample.client_id = requested.client_id
              AND sample.source_kind = requested.source_kind
              AND sample.interface = requested.interface
              AND sample.observed_at <= to_timestamp($5)
            ORDER BY sample.observed_at DESC
            LIMIT 1
        ) sample ON TRUE
    ),
    fast_completed_usage AS (
        SELECT
            requested.client_id,
            requested.source_kind,
            requested.interface,
            COALESCE(SUM(usage.rx_bytes), 0)::bigint AS cycle_rx,
            COALESCE(SUM(usage.tx_bytes), 0)::bigint AS cycle_tx,
            COALESCE(SUM(usage.rx_reset_count), 0)::bigint AS rx_resets,
            COALESCE(SUM(usage.tx_reset_count), 0)::bigint AS tx_resets
        FROM requested
        JOIN coverage ON coverage.valid
        JOIN traffic_counter_hourly_usage usage
          ON usage.client_id = requested.client_id
         AND usage.source_kind = requested.source_kind
         AND usage.interface = requested.interface
         AND usage.bucket_start >= to_timestamp(requested.cycle_start_unix)
         AND usage.bucket_start < date_bin(
                interval '1 hour',
                to_timestamp($5),
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
             )
        GROUP BY
            requested.client_id,
            requested.source_kind,
            requested.interface
    ),
    fast_tail_samples AS (
        SELECT
            requested.client_id,
            requested.source_kind,
            requested.interface,
            requested.cycle_start_unix,
            sample.observed_at,
            sample.rx_bytes,
            sample.tx_bytes,
            sample.rx_counter_epoch,
            sample.tx_counter_epoch,
            sample.sample_source
        FROM requested
        JOIN coverage ON coverage.valid
        JOIN traffic_counter_samples sample
          ON sample.client_id = requested.client_id
         AND sample.source_kind = requested.source_kind
         AND sample.interface = requested.interface
         AND sample.observed_at >= date_bin(
                interval '1 hour',
                to_timestamp($5),
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
             )
         AND sample.observed_at <= to_timestamp($5)
        UNION ALL
        SELECT
            requested.client_id,
            requested.source_kind,
            requested.interface,
            requested.cycle_start_unix,
            sample.observed_at,
            sample.rx_bytes,
            sample.tx_bytes,
            sample.rx_counter_epoch,
            sample.tx_counter_epoch,
            sample.sample_source
        FROM requested
        JOIN coverage ON coverage.valid
        JOIN LATERAL (
            SELECT
                sample.observed_at,
                sample.rx_bytes,
                sample.tx_bytes,
                sample.rx_counter_epoch,
                sample.tx_counter_epoch,
                sample.sample_source
            FROM traffic_counter_samples sample
            WHERE sample.client_id = requested.client_id
              AND sample.source_kind = requested.source_kind
              AND sample.interface = requested.interface
              AND sample.observed_at < date_bin(
                    interval '1 hour',
                    to_timestamp($5),
                    TIMESTAMPTZ '1970-01-01 00:00:00+00'
                  )
            ORDER BY sample.observed_at DESC
            LIMIT 1
        ) sample ON TRUE
    ),
    fast_tail_sequenced AS (
        SELECT
            fast_tail_samples.*,
            LAG(observed_at) OVER stream AS previous_observed_at,
            LAG(rx_bytes) OVER stream AS previous_rx_bytes,
            LAG(tx_bytes) OVER stream AS previous_tx_bytes,
            LAG(rx_counter_epoch) OVER stream AS previous_rx_counter_epoch,
            LAG(tx_counter_epoch) OVER stream AS previous_tx_counter_epoch,
            LAG(sample_source) OVER stream AS previous_sample_source
        FROM fast_tail_samples
        WINDOW stream AS (
            PARTITION BY client_id, source_kind, interface
            ORDER BY observed_at
        )
    ),
    fast_tail_usage AS (
        SELECT
            client_id,
            source_kind,
            interface,
            COALESCE(SUM(
                CASE
                    WHEN observed_at >= to_timestamp(cycle_start_unix)
                     AND previous_observed_at IS NOT NULL
                     AND rx_counter_epoch = previous_rx_counter_epoch
                     AND rx_bytes >= previous_rx_bytes
                    THEN rx_bytes - previous_rx_bytes
                    ELSE 0
                END
            ), 0)::bigint AS cycle_rx,
            COALESCE(SUM(
                CASE
                    WHEN observed_at >= to_timestamp(cycle_start_unix)
                     AND previous_observed_at IS NOT NULL
                     AND tx_counter_epoch = previous_tx_counter_epoch
                     AND tx_bytes >= previous_tx_bytes
                    THEN tx_bytes - previous_tx_bytes
                    ELSE 0
                END
            ), 0)::bigint AS cycle_tx,
            COUNT(*) FILTER (
                WHERE observed_at >= to_timestamp(cycle_start_unix)
                  AND previous_rx_counter_epoch IS NOT NULL
                  AND rx_counter_epoch <> previous_rx_counter_epoch
                  AND NOT (
                      previous_sample_source LIKE 'vnstat_import:%'
                      AND sample_source NOT LIKE 'vnstat_import:%'
                  )
            )::bigint AS rx_resets,
            COUNT(*) FILTER (
                WHERE observed_at >= to_timestamp(cycle_start_unix)
                  AND previous_tx_counter_epoch IS NOT NULL
                  AND tx_counter_epoch <> previous_tx_counter_epoch
                  AND NOT (
                      previous_sample_source LIKE 'vnstat_import:%'
                      AND sample_source NOT LIKE 'vnstat_import:%'
                  )
            )::bigint AS tx_resets
        FROM fast_tail_sequenced
        GROUP BY client_id, source_kind, interface
    ),
    fast_usage AS (
        SELECT
            latest.client_id,
            latest.source_kind,
            latest.interface,
            COALESCE(completed.cycle_rx, 0)
                + COALESCE(tail.cycle_rx, 0) AS cycle_rx,
            COALESCE(completed.cycle_tx, 0)
                + COALESCE(tail.cycle_tx, 0) AS cycle_tx,
            latest.latest_rx,
            latest.latest_tx,
            latest.last_sample_unix,
            1 + COALESCE(completed.rx_resets, 0)
                + COALESCE(tail.rx_resets, 0) AS rx_counter_epochs_seen,
            1 + COALESCE(completed.tx_resets, 0)
                + COALESCE(tail.tx_resets, 0) AS tx_counter_epochs_seen
        FROM fast_latest latest
        LEFT JOIN fast_completed_usage completed
          ON completed.client_id = latest.client_id
         AND completed.source_kind = latest.source_kind
         AND completed.interface = latest.interface
        LEFT JOIN fast_tail_usage tail
          ON tail.client_id = latest.client_id
         AND tail.source_kind = latest.source_kind
         AND tail.interface = latest.interface
    ),
    fallback_requested AS MATERIALIZED (
        SELECT requested.*
        FROM requested
        JOIN coverage ON NOT coverage.valid
    ),
    fallback_cycle_samples AS (
        SELECT
            sample.client_id,
            sample.source_kind,
            sample.interface,
            sample.observed_at,
            sample.rx_bytes,
            sample.tx_bytes,
            sample.rx_counter_epoch,
            sample.tx_counter_epoch,
            sample.sample_source,
            requested.cycle_start_unix
        FROM traffic_counter_samples sample
        JOIN fallback_requested requested
          ON requested.client_id = sample.client_id
         AND requested.source_kind = sample.source_kind
         AND requested.interface = sample.interface
        WHERE sample.observed_at >= to_timestamp(requested.cycle_start_unix)
          AND sample.observed_at <= to_timestamp($5)
    ),
    fallback_baseline_samples AS (
        SELECT
            requested.client_id,
            requested.source_kind,
            requested.interface,
            sample.observed_at,
            sample.rx_bytes,
            sample.tx_bytes,
            sample.rx_counter_epoch,
            sample.tx_counter_epoch,
            sample.sample_source,
            requested.cycle_start_unix
        FROM fallback_requested requested
        JOIN LATERAL (
            SELECT
                sample.observed_at,
                sample.rx_bytes,
                sample.tx_bytes,
                sample.rx_counter_epoch,
                sample.tx_counter_epoch,
                sample.sample_source
            FROM traffic_counter_samples sample
            WHERE sample.client_id = requested.client_id
              AND sample.source_kind = requested.source_kind
              AND sample.interface = requested.interface
              AND sample.observed_at < to_timestamp(requested.cycle_start_unix)
              AND sample.observed_at <= to_timestamp($5)
            ORDER BY sample.observed_at DESC
            LIMIT 1
        ) sample ON TRUE
    ),
    fallback_selected_samples AS (
        SELECT * FROM fallback_cycle_samples
        UNION ALL
        SELECT * FROM fallback_baseline_samples
    ),
    fallback_sequenced_samples AS (
        SELECT
            fallback_selected_samples.*,
            LAG(observed_at) OVER stream AS previous_observed_at,
            LAG(rx_bytes) OVER stream AS previous_rx_bytes,
            LAG(tx_bytes) OVER stream AS previous_tx_bytes,
            LAG(rx_counter_epoch) OVER stream AS previous_rx_counter_epoch,
            LAG(tx_counter_epoch) OVER stream AS previous_tx_counter_epoch,
            LAG(sample_source) OVER stream AS previous_sample_source
        FROM fallback_selected_samples
        WINDOW stream AS (
            PARTITION BY client_id, source_kind, interface
            ORDER BY observed_at ASC
        )
    ),
    fallback_usage AS (
        SELECT
            client_id,
            source_kind,
            interface,
            COALESCE(SUM(
                CASE
                    WHEN observed_at >= to_timestamp(cycle_start_unix)
                     AND previous_observed_at IS NOT NULL
                     AND rx_counter_epoch = previous_rx_counter_epoch
                     AND rx_bytes >= previous_rx_bytes
                    THEN rx_bytes - previous_rx_bytes
                    ELSE 0
                END
            ), 0)::bigint AS cycle_rx,
            COALESCE(SUM(
                CASE
                    WHEN observed_at >= to_timestamp(cycle_start_unix)
                     AND previous_observed_at IS NOT NULL
                     AND tx_counter_epoch = previous_tx_counter_epoch
                     AND tx_bytes >= previous_tx_bytes
                    THEN tx_bytes - previous_tx_bytes
                    ELSE 0
                END
            ), 0)::bigint AS cycle_tx,
            (1 + COUNT(*) FILTER (
                WHERE previous_rx_counter_epoch IS NOT NULL
                  AND rx_counter_epoch <> previous_rx_counter_epoch
                  AND NOT (
                      previous_sample_source LIKE 'vnstat_import:%'
                      AND sample_source NOT LIKE 'vnstat_import:%'
                  )
            ))::bigint AS rx_counter_epochs_seen,
            (1 + COUNT(*) FILTER (
                WHERE previous_tx_counter_epoch IS NOT NULL
                  AND tx_counter_epoch <> previous_tx_counter_epoch
                  AND NOT (
                      previous_sample_source LIKE 'vnstat_import:%'
                      AND sample_source NOT LIKE 'vnstat_import:%'
                  )
            ))::bigint AS tx_counter_epochs_seen
        FROM fallback_sequenced_samples
        GROUP BY client_id, source_kind, interface
    ),
    fallback_latest AS (
        SELECT DISTINCT ON (client_id, source_kind, interface)
            client_id,
            source_kind,
            interface,
            rx_bytes AS latest_rx,
            tx_bytes AS latest_tx,
            EXTRACT(EPOCH FROM observed_at)::bigint AS last_sample_unix
        FROM fallback_sequenced_samples
        ORDER BY client_id, source_kind, interface, observed_at DESC
    ),
    fallback_result AS (
        SELECT
            usage.client_id,
            usage.source_kind,
            usage.interface,
            usage.cycle_rx,
            usage.cycle_tx,
            latest.latest_rx,
            latest.latest_tx,
            latest.last_sample_unix,
            usage.rx_counter_epochs_seen,
            usage.tx_counter_epochs_seen
        FROM fallback_usage usage
        JOIN fallback_latest latest
          ON latest.client_id = usage.client_id
         AND latest.source_kind = usage.source_kind
         AND latest.interface = usage.interface
    )
    SELECT * FROM fast_usage
    UNION ALL
    SELECT * FROM fallback_result
    ORDER BY client_id, source_kind, interface
"#;

// Long-term rows are a non-overlapping ledger of valid counter transitions.
// Healthy hourly coverage bounds the retained raw scan to the as-of hour;
// promoted rows cover older time. Dirty coverage falls back to the complete
// raw-LAG oracle, and intentional vnStat-to-live boundaries contribute neither
// bytes nor resets.
pub(crate) const NO_RESET_TRAFFIC_COUNTER_USAGE_SQL: &str = r#"
    WITH requested AS MATERIALIZED (
        SELECT client_id, source_kind, interface
        FROM UNNEST(
            $1::text[],
            $2::text[],
            $3::text[]
        ) AS request(client_id, source_kind, interface)
    ),
    coverage AS MATERIALIZED (
        SELECT
            requested.client_id,
            requested.source_kind,
            requested.interface,
            NOT EXISTS (
                SELECT 1
                FROM traffic_counter_samples sample
                WHERE sample.client_id = requested.client_id
                  AND sample.source_kind = requested.source_kind
                  AND sample.interface = requested.interface
                  AND sample.observed_at <= to_timestamp($4)
            ) OR (
                streams.client_id IS NOT NULL
                AND streams.source_revision = streams.materialized_revision
                AND NOT EXISTS (
                    -- The hourly ledger deliberately has no promoted-row
                    -- dimension. Normal retention leaves one predecessor
                    -- boundary with no older raw row. Fail closed for this
                    -- stream if a promoted row has an exact predecessor.
                    SELECT 1
                    FROM traffic_counter_samples promoted
                    WHERE promoted.client_id = requested.client_id
                      AND promoted.source_kind = requested.source_kind
                      AND promoted.interface = requested.interface
                      AND promoted.inbound_promoted
                      AND promoted.observed_at <= to_timestamp($4)
                      AND EXISTS (
                          SELECT 1
                          FROM traffic_counter_samples predecessor
                          WHERE predecessor.client_id = promoted.client_id
                            AND predecessor.source_kind = promoted.source_kind
                            AND predecessor.interface = promoted.interface
                            AND predecessor.observed_at < promoted.observed_at
                            AND predecessor.observed_at <= to_timestamp($4)
                      )
                )
            ) AS valid
        FROM requested
        LEFT JOIN traffic_counter_hourly_usage_streams streams
          ON streams.client_id = requested.client_id
         AND streams.source_kind = requested.source_kind
         AND streams.interface = requested.interface
    ),
    fast_requested AS MATERIALIZED (
        SELECT requested.*
        FROM requested
        JOIN coverage
          ON coverage.client_id = requested.client_id
         AND coverage.source_kind = requested.source_kind
         AND coverage.interface = requested.interface
        WHERE coverage.valid
    ),
    latest AS (
        SELECT
            requested.client_id,
            requested.source_kind,
            requested.interface,
            sample.rx_bytes AS latest_rx,
            sample.tx_bytes AS latest_tx,
            EXTRACT(EPOCH FROM sample.observed_at)::bigint AS last_sample_unix
        FROM requested
        JOIN LATERAL (
            SELECT
                sample.observed_at,
                sample.rx_bytes,
                sample.tx_bytes
            FROM traffic_counter_samples sample
            WHERE sample.client_id = requested.client_id
              AND sample.source_kind = requested.source_kind
              AND sample.interface = requested.interface
              AND sample.observed_at <= to_timestamp($4)
            ORDER BY sample.observed_at DESC
            LIMIT 1
        ) sample ON TRUE
    ),
    fast_completed_usage AS (
        SELECT
            requested.client_id,
            requested.source_kind,
            requested.interface,
            COALESCE(SUM(usage.rx_bytes), 0)::bigint AS cycle_rx,
            COALESCE(SUM(usage.tx_bytes), 0)::bigint AS cycle_tx,
            COALESCE(SUM(usage.rx_reset_count), 0)::bigint AS rx_resets,
            COALESCE(SUM(usage.tx_reset_count), 0)::bigint AS tx_resets
        FROM fast_requested requested
        JOIN traffic_counter_hourly_usage usage
          ON usage.client_id = requested.client_id
         AND usage.source_kind = requested.source_kind
         AND usage.interface = requested.interface
         AND usage.bucket_start < date_bin(
                interval '1 hour',
                to_timestamp($4),
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
             )
        GROUP BY
            requested.client_id,
            requested.source_kind,
            requested.interface
    ),
    fast_tail_samples AS (
        SELECT
            requested.client_id,
            requested.source_kind,
            requested.interface,
            sample.observed_at,
            sample.rx_bytes,
            sample.tx_bytes,
            sample.rx_counter_epoch,
            sample.tx_counter_epoch,
            sample.sample_source,
            sample.inbound_promoted
        FROM fast_requested requested
        JOIN traffic_counter_samples sample
          ON sample.client_id = requested.client_id
         AND sample.source_kind = requested.source_kind
         AND sample.interface = requested.interface
         AND sample.observed_at >= date_bin(
                interval '1 hour',
                to_timestamp($4),
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
             )
         AND sample.observed_at <= to_timestamp($4)
        UNION ALL
        SELECT
            requested.client_id,
            requested.source_kind,
            requested.interface,
            sample.observed_at,
            sample.rx_bytes,
            sample.tx_bytes,
            sample.rx_counter_epoch,
            sample.tx_counter_epoch,
            sample.sample_source,
            sample.inbound_promoted
        FROM fast_requested requested
        JOIN LATERAL (
            SELECT
                sample.observed_at,
                sample.rx_bytes,
                sample.tx_bytes,
                sample.rx_counter_epoch,
                sample.tx_counter_epoch,
                sample.sample_source,
                sample.inbound_promoted
            FROM traffic_counter_samples sample
            WHERE sample.client_id = requested.client_id
              AND sample.source_kind = requested.source_kind
              AND sample.interface = requested.interface
              AND sample.observed_at < date_bin(
                    interval '1 hour',
                    to_timestamp($4),
                    TIMESTAMPTZ '1970-01-01 00:00:00+00'
                  )
            ORDER BY sample.observed_at DESC
            LIMIT 1
        ) sample ON TRUE
    ),
    fast_tail_sequenced AS (
        SELECT
            fast_tail_samples.*,
            LAG(rx_bytes) OVER stream AS previous_rx_bytes,
            LAG(tx_bytes) OVER stream AS previous_tx_bytes,
            LAG(rx_counter_epoch) OVER stream AS previous_rx_counter_epoch,
            LAG(tx_counter_epoch) OVER stream AS previous_tx_counter_epoch,
            LAG(sample_source) OVER stream AS previous_sample_source
        FROM fast_tail_samples
        WINDOW stream AS (
            PARTITION BY client_id, source_kind, interface
            ORDER BY observed_at
        )
    ),
    fast_tail_usage AS (
        SELECT
            client_id,
            source_kind,
            interface,
            COALESCE(SUM(
                CASE WHEN rx_counter_epoch = previous_rx_counter_epoch
                           AND rx_bytes >= previous_rx_bytes
                     THEN rx_bytes - previous_rx_bytes ELSE 0 END
            ), 0)::bigint AS cycle_rx,
            COALESCE(SUM(
                CASE WHEN tx_counter_epoch = previous_tx_counter_epoch
                           AND tx_bytes >= previous_tx_bytes
                     THEN tx_bytes - previous_tx_bytes ELSE 0 END
            ), 0)::bigint AS cycle_tx,
            COUNT(*) FILTER (
                WHERE previous_rx_counter_epoch IS NOT NULL
                  AND rx_counter_epoch <> previous_rx_counter_epoch
                  AND NOT (
                      previous_sample_source LIKE 'vnstat_import:%'
                      AND sample_source NOT LIKE 'vnstat_import:%'
                  )
            )::bigint AS rx_resets,
            COUNT(*) FILTER (
                WHERE previous_tx_counter_epoch IS NOT NULL
                  AND tx_counter_epoch <> previous_tx_counter_epoch
                  AND NOT (
                      previous_sample_source LIKE 'vnstat_import:%'
                      AND sample_source NOT LIKE 'vnstat_import:%'
                  )
            )::bigint AS tx_resets
        FROM fast_tail_sequenced
        WHERE NOT inbound_promoted
        GROUP BY client_id, source_kind, interface
    ),
    fast_raw_usage AS (
        SELECT
            requested.client_id,
            requested.source_kind,
            requested.interface,
            COALESCE(completed.cycle_rx, 0)
                + COALESCE(tail.cycle_rx, 0) AS cycle_rx,
            COALESCE(completed.cycle_tx, 0)
                + COALESCE(tail.cycle_tx, 0) AS cycle_tx,
            COALESCE(completed.rx_resets, 0)
                + COALESCE(tail.rx_resets, 0) AS rx_resets,
            COALESCE(completed.tx_resets, 0)
                + COALESCE(tail.tx_resets, 0) AS tx_resets
        FROM fast_requested requested
        LEFT JOIN fast_completed_usage completed
          ON completed.client_id = requested.client_id
         AND completed.source_kind = requested.source_kind
         AND completed.interface = requested.interface
        LEFT JOIN fast_tail_usage tail
          ON tail.client_id = requested.client_id
         AND tail.source_kind = requested.source_kind
         AND tail.interface = requested.interface
    ),
    fallback_requested AS MATERIALIZED (
        SELECT requested.*
        FROM requested
        JOIN coverage
          ON coverage.client_id = requested.client_id
         AND coverage.source_kind = requested.source_kind
         AND coverage.interface = requested.interface
        WHERE NOT coverage.valid
    ),
    fallback_raw_selected AS (
        SELECT
            sample.client_id,
            sample.source_kind,
            sample.interface,
            sample.observed_at,
            sample.rx_bytes,
            sample.tx_bytes,
            sample.rx_counter_epoch,
            sample.tx_counter_epoch,
            sample.sample_source,
            sample.inbound_promoted
        FROM traffic_counter_samples sample
        JOIN fallback_requested requested
          ON requested.client_id = sample.client_id
         AND requested.source_kind = sample.source_kind
         AND requested.interface = sample.interface
        WHERE sample.observed_at <= to_timestamp($4)
    ),
    fallback_raw_sequenced AS (
        SELECT
            fallback_raw_selected.*,
            LAG(rx_bytes) OVER stream AS previous_rx_bytes,
            LAG(tx_bytes) OVER stream AS previous_tx_bytes,
            LAG(rx_counter_epoch) OVER stream AS previous_rx_counter_epoch,
            LAG(tx_counter_epoch) OVER stream AS previous_tx_counter_epoch,
            LAG(sample_source) OVER stream AS previous_sample_source
        FROM fallback_raw_selected
        WINDOW stream AS (
            PARTITION BY client_id, source_kind, interface
            ORDER BY observed_at
        )
    ),
    fallback_raw_usage AS (
        SELECT
            client_id,
            source_kind,
            interface,
            COALESCE(SUM(
                CASE WHEN rx_counter_epoch = previous_rx_counter_epoch
                           AND rx_bytes >= previous_rx_bytes
                     THEN rx_bytes - previous_rx_bytes ELSE 0 END
            ), 0)::bigint AS cycle_rx,
            COALESCE(SUM(
                CASE WHEN tx_counter_epoch = previous_tx_counter_epoch
                           AND tx_bytes >= previous_tx_bytes
                     THEN tx_bytes - previous_tx_bytes ELSE 0 END
            ), 0)::bigint AS cycle_tx,
            COUNT(*) FILTER (
                WHERE previous_rx_counter_epoch IS NOT NULL
                  AND rx_counter_epoch <> previous_rx_counter_epoch
                  AND NOT (
                      previous_sample_source LIKE 'vnstat_import:%'
                      AND sample_source NOT LIKE 'vnstat_import:%'
                  )
            )::bigint AS rx_resets,
            COUNT(*) FILTER (
                WHERE previous_tx_counter_epoch IS NOT NULL
                  AND tx_counter_epoch <> previous_tx_counter_epoch
                  AND NOT (
                      previous_sample_source LIKE 'vnstat_import:%'
                      AND sample_source NOT LIKE 'vnstat_import:%'
                  )
            )::bigint AS tx_resets
        FROM fallback_raw_sequenced
        WHERE NOT inbound_promoted
        GROUP BY client_id, source_kind, interface
    ),
    raw_usage AS (
        SELECT * FROM fast_raw_usage
        UNION ALL
        SELECT * FROM fallback_raw_usage
    ),
    raw_bounds AS MATERIALIZED (
        SELECT
            requested.client_id,
            requested.source_kind,
            requested.interface,
            earliest.observed_at AS first_exact_at,
            latest.observed_at AS last_exact_at
        FROM requested
        LEFT JOIN LATERAL (
            SELECT sample.observed_at
            FROM traffic_counter_samples sample
            WHERE sample.client_id = requested.client_id
              AND sample.source_kind = requested.source_kind
              AND sample.interface = requested.interface
              AND NOT sample.inbound_promoted
            ORDER BY sample.observed_at ASC
            LIMIT 1
        ) earliest ON TRUE
        LEFT JOIN LATERAL (
            SELECT sample.observed_at
            FROM traffic_counter_samples sample
            WHERE sample.client_id = requested.client_id
              AND sample.source_kind = requested.source_kind
              AND sample.interface = requested.interface
              AND NOT sample.inbound_promoted
            ORDER BY sample.observed_at DESC
            LIMIT 1
        ) latest ON TRUE
    ),
    finer_ranges AS MATERIALIZED (
        SELECT
            rollup.client_id,
            rollup.source_kind,
            rollup.interface,
            rollup.origin_kind,
            rollup.bucket_secs,
            MIN(rollup.bucket_start) AS first_bucket_start,
            MAX(rollup.bucket_start + make_interval(secs => rollup.bucket_secs))
                AS last_bucket_end
        FROM traffic_counter_rollups rollup
        JOIN requested
          ON requested.client_id = rollup.client_id
         AND requested.source_kind = rollup.source_kind
         AND requested.interface = rollup.interface
        WHERE rollup.bucket_secs < 86400
        GROUP BY
            rollup.client_id,
            rollup.source_kind,
            rollup.interface,
            rollup.origin_kind,
            rollup.bucket_secs
    ),
    retained_usage AS (
        SELECT
            rollup.client_id,
            rollup.source_kind,
            rollup.interface,
            COALESCE(SUM(rollup.rx_bytes), 0)::bigint AS cycle_rx,
            COALESCE(SUM(rollup.tx_bytes), 0)::bigint AS cycle_tx,
            COALESCE(SUM(rollup.rx_reset_count), 0)::bigint AS rx_resets,
            COALESCE(SUM(rollup.tx_reset_count), 0)::bigint AS tx_resets
        FROM traffic_counter_rollups rollup
        JOIN requested
          ON requested.client_id = rollup.client_id
         AND requested.source_kind = rollup.source_kind
         AND requested.interface = rollup.interface
        JOIN raw_bounds
          ON raw_bounds.client_id = rollup.client_id
         AND raw_bounds.source_kind = rollup.source_kind
         AND raw_bounds.interface = rollup.interface
        WHERE rollup.bucket_start <= to_timestamp($4)
          AND NOT EXISTS (
                SELECT 1
                FROM finer_ranges range
                JOIN traffic_counter_rollups finer
                  ON finer.client_id = range.client_id
                 AND finer.source_kind = range.source_kind
                 AND finer.interface = range.interface
                 AND finer.origin_kind = range.origin_kind
                 AND finer.bucket_secs = range.bucket_secs
                 AND finer.bucket_start < rollup.bucket_start
                        + make_interval(secs => rollup.bucket_secs)
                 AND finer.bucket_start
                        + make_interval(secs => finer.bucket_secs)
                        > rollup.bucket_start
                WHERE range.client_id = rollup.client_id
                  AND range.source_kind = rollup.source_kind
                  AND range.interface = rollup.interface
                  AND range.origin_kind = rollup.origin_kind
                  AND range.bucket_secs < rollup.bucket_secs
                  AND range.first_bucket_start < rollup.bucket_start
                        + make_interval(secs => rollup.bucket_secs)
                  AND range.last_bucket_end > rollup.bucket_start
          )
          AND (
                raw_bounds.first_exact_at IS NULL
                OR rollup.bucket_start + make_interval(secs => rollup.bucket_secs)
                    <= raw_bounds.first_exact_at
                OR rollup.bucket_start > raw_bounds.last_exact_at
                OR NOT EXISTS (
                    SELECT 1
                    FROM traffic_counter_samples exact
                    WHERE exact.client_id = rollup.client_id
                      AND exact.source_kind = rollup.source_kind
                      AND exact.interface = rollup.interface
                      AND NOT exact.inbound_promoted
                      AND (CASE WHEN exact.sample_source LIKE 'vnstat_import:%'
                                THEN 'vnstat_import' ELSE 'live' END) = rollup.origin_kind
                      AND exact.observed_at >= rollup.bucket_start
                      AND exact.observed_at < rollup.bucket_start
                            + make_interval(secs => rollup.bucket_secs)
                )
          )
        GROUP BY rollup.client_id, rollup.source_kind, rollup.interface
    )
    SELECT
        latest.client_id,
        latest.source_kind,
        latest.interface,
        COALESCE(raw_usage.cycle_rx, 0)
            + COALESCE(retained_usage.cycle_rx, 0) AS cycle_rx,
        COALESCE(raw_usage.cycle_tx, 0)
            + COALESCE(retained_usage.cycle_tx, 0) AS cycle_tx,
        latest.latest_rx,
        latest.latest_tx,
        latest.last_sample_unix,
        1 + COALESCE(raw_usage.rx_resets, 0)
            + COALESCE(retained_usage.rx_resets, 0) AS rx_counter_epochs_seen,
        1 + COALESCE(raw_usage.tx_resets, 0)
            + COALESCE(retained_usage.tx_resets, 0) AS tx_counter_epochs_seen
    FROM latest
    LEFT JOIN raw_usage
      ON raw_usage.client_id = latest.client_id
     AND raw_usage.source_kind = latest.source_kind
     AND raw_usage.interface = latest.interface
    LEFT JOIN retained_usage
      ON retained_usage.client_id = latest.client_id
     AND retained_usage.source_kind = latest.source_kind
     AND retained_usage.interface = latest.interface
    ORDER BY latest.client_id ASC, latest.source_kind ASC, latest.interface ASC
"#;

#[derive(Clone, Debug, Eq, PartialEq)]
struct TrafficHistoryStream {
    source_kind: String,
    interface: String,
    direction_mask: i32,
}

#[derive(Clone, Debug)]
struct TrafficCounterStreamUsage {
    client_id: String,
    source_kind: String,
    interface: String,
    cycle_rx: i64,
    cycle_tx: i64,
    latest_rx: i64,
    latest_tx: i64,
    last_sample_unix: i64,
    rx_counter_epochs_seen: i64,
    tx_counter_epochs_seen: i64,
}

fn traffic_counter_stream_usage_from_row(row: PgRow) -> Result<TrafficCounterStreamUsage> {
    Ok(TrafficCounterStreamUsage {
        client_id: row.try_get("client_id")?,
        source_kind: row.try_get("source_kind")?,
        interface: row.try_get("interface")?,
        cycle_rx: row.try_get("cycle_rx")?,
        cycle_tx: row.try_get("cycle_tx")?,
        latest_rx: row.try_get("latest_rx")?,
        latest_tx: row.try_get("latest_tx")?,
        last_sample_unix: row.try_get("last_sample_unix")?,
        rx_counter_epochs_seen: row.try_get("rx_counter_epochs_seen")?,
        tx_counter_epochs_seen: row.try_get("tx_counter_epochs_seen")?,
    })
}

type ParsedRuleValue = ParsedVpsRuleValue;

#[derive(Clone, Debug)]
struct PolicyEvaluation {
    condition_true: bool,
    incomplete: bool,
    incomplete_reasons: Vec<String>,
    actual_value: Option<f64>,
    threshold_value: Option<f64>,
    category: String,
    payload: Value,
}

async fn preview_current_policy_rule(
    pool: &sqlx::PgPool,
    rule: &PolicyRuleRequest,
    matched_client_ids: &[String],
) -> Result<(i64, i64, i64, Vec<String>)> {
    if matched_client_ids.is_empty() {
        return Ok((0, 0, 0, Vec::new()));
    }
    let rows = sqlx::query(
        r#"
        WITH latest AS (
            SELECT DISTINCT ON (evidence.source_kind, evidence.natural_key)
                   evidence.subject_client_id, evidence.completeness,
                   evidence.payload, evidence.subject_snapshot
            FROM alert_policy_evidence evidence
            WHERE evidence.source_kind=$1
              AND evidence.subject_client_id=ANY($2::text[])
            ORDER BY evidence.source_kind, evidence.natural_key,
                     evidence.observed_at DESC, evidence.evidence_seq DESC
        )
        SELECT subject_client_id, completeness, payload, subject_snapshot
        FROM latest
        WHERE payload->>'source_present' IS DISTINCT FROM 'false'
        ORDER BY subject_client_id
        "#,
    )
    .bind(&rule.evidence_source)
    .bind(matched_client_ids)
    .fetch_all(pool)
    .await?;
    let mut true_count = 0_i64;
    let mut false_count = 0_i64;
    let mut incomplete_count = 0_i64;
    let mut seen_subjects = HashSet::new();
    let mut incomplete_subjects = BTreeSet::new();
    for row in rows {
        let subject_client_id: String = row.try_get("subject_client_id")?;
        let complete = row.try_get::<String, _>("completeness")? == "complete";
        let payload = row.try_get::<SqlJson<Value>, _>("payload")?.0;
        let subject = row.try_get::<SqlJson<Value>, _>("subject_snapshot")?.0;
        seen_subjects.insert(subject_client_id.clone());
        match crate::repository_policy_lifecycle::policy_expression_truth_for_preview(
            rule.rule_kind,
            &rule.trigger_condition_expression,
            &payload,
            &subject,
            complete,
        )? {
            ExpressionTruth::True => true_count += 1,
            ExpressionTruth::False => false_count += 1,
            ExpressionTruth::Unknown => {
                incomplete_count += 1;
                incomplete_subjects.insert(subject_client_id);
            }
        }
    }
    if matches!(
        rule.evidence_source.as_str(),
        "telemetry.combined" | "agent.status" | "agent.access"
    ) {
        for client_id in matched_client_ids {
            if !seen_subjects.contains(client_id) {
                incomplete_count += 1;
                incomplete_subjects.insert(client_id.clone());
            }
        }
    }
    Ok((
        true_count,
        false_count,
        incomplete_count,
        incomplete_subjects.into_iter().collect(),
    ))
}

fn normalized_policy_preview_rule(rule: &PolicyRuleRequest) -> Value {
    json!({
        "id": rule.id,
        "name": rule.name.trim(),
        "enabled": rule.enabled,
        "rule_kind": rule.rule_kind,
        "evidence_source": rule.evidence_source.trim(),
        "correlation_mode": rule.correlation_mode,
        "traffic_selector": clean_optional_text(rule.traffic_selector.as_deref()),
        "trigger_condition_expression": rule.trigger_condition_expression.trim(),
        "trigger_meta_condition": canonical_policy_meta(rule.trigger_meta_condition.as_ref()),
        "resolve_condition_expression": clean_optional_text(
            rule.resolve_condition_expression.as_deref()
        ),
        "resolve_meta_condition": canonical_policy_meta(rule.resolve_meta_condition.as_ref()),
        "severity": rule.severity.trim(),
        "category": rule.category.trim(),
        "title_template": rule.title_template.trim(),
        "detail_template": rule.detail_template.trim(),
    })
}

impl Repository {
    pub(crate) async fn list_vps_rules(
        &self,
        query: &VpsRuleQuery,
    ) -> Result<Vec<VpsRuleValueRecord>> {
        let result_limit = query.limit.unwrap_or(1000).clamp(1, 5000) as usize;
        self.list_vps_rules_matching(query, Some(result_limit))
            .await
    }

    /// Returns the complete directly configured rule set for visible clients.
    /// Used by the fleet snapshot so local selector evaluation never interprets
    /// a truncated rule set as an absent rule.
    pub(crate) async fn list_all_vps_rules(&self) -> Result<Vec<VpsRuleValueRecord>> {
        self.list_vps_rules_matching(
            &VpsRuleQuery {
                limit: None,
                client_id: None,
                selector_expression: None,
                key: None,
                state: None,
            },
            None,
        )
        .await
    }

    async fn list_vps_rules_matching(
        &self,
        query: &VpsRuleQuery,
        result_limit: Option<usize>,
    ) -> Result<Vec<VpsRuleValueRecord>> {
        let agents = self.list_agents().await?;
        let allowed_clients = if let Some(selector) = query.selector_expression.as_deref() {
            self.resolve_agents_for_selector(&agents, selector)
                .await?
                .into_iter()
                .map(|agent| agent.id)
                .collect::<HashSet<_>>()
        } else {
            agents
                .into_iter()
                .map(|agent| agent.id)
                .collect::<HashSet<_>>()
        };
        let allowed_client_ids = allowed_clients.iter().cloned().collect::<Vec<_>>();
        // `state` is derived while parsing persisted values, so PostgreSQL may
        // apply a result limit only when every requested filter is represented
        // in SQL. Otherwise rows are state-filtered before the API limit below.
        let database_limit = query
            .state
            .is_none()
            .then_some(result_limit)
            .flatten()
            .map(|limit| limit as i64);
        let mut rows = match self {
            Self::Memory(memory) => memory
                .vps_rule_values
                .read()
                .await
                .iter()
                .filter(|row| {
                    allowed_clients.contains(&row.client_id)
                        && query
                            .client_id
                            .as_deref()
                            .is_none_or(|client_id| row.client_id == client_id)
                        && query.key.as_deref().is_none_or(|key| row.key == key)
                })
                .cloned()
                .map(canonicalize_vps_rule_record)
                .collect::<Result<Vec<_>>>()?,
            Self::Postgres(pool) => sqlx::query(
                r#"
                SELECT
                    client_id,
                    key,
                    value_raw,
                    value_json,
                    source_kind,
                    source_id,
                    updated_by,
                    updated_at::text AS updated_at
                FROM vps_rule_values
                WHERE ($1::text IS NULL OR client_id = $1)
                  AND ($2::text IS NULL OR key = $2)
                  AND ($3::text[] IS NULL OR client_id = ANY($3))
                ORDER BY client_id ASC, key ASC
                LIMIT $4
                "#,
            )
            .bind(query.client_id.as_deref())
            .bind(query.key.as_deref())
            .bind(&allowed_client_ids)
            .bind(database_limit)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(vps_rule_from_row)
            .collect::<Result<Vec<_>>>()?,
        };
        rows.retain(|row| {
            query
                .client_id
                .as_deref()
                .is_none_or(|client_id| row.client_id == client_id)
                && query.key.as_deref().is_none_or(|key| row.key == key)
                && query
                    .state
                    .as_deref()
                    .is_none_or(|state| row.state == state)
                && allowed_clients.contains(&row.client_id)
        });
        rows.sort_by(|left, right| {
            left.client_id
                .cmp(&right.client_id)
                .then_with(|| left.key.cmp(&right.key))
        });
        if let Some(result_limit) = result_limit {
            rows.truncate(result_limit);
        }
        Ok(rows)
    }

    pub(crate) async fn effective_vps_rules(
        &self,
        client_id: &str,
    ) -> Result<Vec<VpsRuleValueRecord>> {
        anyhow::ensure!(
            self.list_agents()
                .await?
                .iter()
                .any(|agent| agent.id == client_id),
            "vps_rules_target_not_found"
        );
        self.list_vps_rules_matching(
            &VpsRuleQuery {
                limit: None,
                client_id: Some(client_id.to_string()),
                selector_expression: None,
                key: None,
                state: None,
            },
            None,
        )
        .await
    }

    pub(crate) async fn list_vps_rules_for_clients(
        &self,
        client_ids: &[String],
        keys: &[&str],
    ) -> Result<Vec<VpsRuleValueRecord>> {
        let keys = keys
            .iter()
            .map(|key| normalize_vps_rule_key(key))
            .collect::<Result<Vec<_>>>()?;
        if client_ids.is_empty() || keys.is_empty() {
            return Ok(Vec::new());
        }
        match self {
            Self::Memory(memory) => {
                let allowed = client_ids
                    .iter()
                    .map(String::as_str)
                    .collect::<HashSet<_>>();
                let mut rows = memory
                    .vps_rule_values
                    .read()
                    .await
                    .iter()
                    .filter(|row| {
                        allowed.contains(row.client_id.as_str()) && keys.contains(&row.key)
                    })
                    .cloned()
                    .map(canonicalize_vps_rule_record)
                    .collect::<Result<Vec<_>>>()?;
                rows.sort_by(|left, right| left.client_id.cmp(&right.client_id));
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        client_id,
                        key,
                        value_raw,
                        value_json,
                        source_kind,
                        source_id,
                        updated_by,
                        updated_at::text AS updated_at
                    FROM vps_rule_values
                    WHERE client_id = ANY($1::TEXT[]) AND key = ANY($2::TEXT[])
                    ORDER BY client_id ASC, key ASC
                    "#,
                )
                .bind(client_ids)
                .bind(&keys)
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(vps_rule_from_row).collect()
            }
        }
    }

    /// Loads every directly configured VPS rule for the supplied clients.
    ///
    /// Selector evaluation deliberately uses this unbounded-by-row-count path:
    /// the schema permits at most one row for each of the ten supported keys,
    /// so the caller-controlled client set is the natural and complete bound.
    pub(crate) async fn list_all_vps_rules_for_clients(
        &self,
        client_ids: &[String],
    ) -> Result<Vec<VpsRuleValueRecord>> {
        if client_ids.is_empty() {
            return Ok(Vec::new());
        }
        match self {
            Self::Memory(memory) => {
                let allowed = client_ids
                    .iter()
                    .map(String::as_str)
                    .collect::<HashSet<_>>();
                let mut rows = memory
                    .vps_rule_values
                    .read()
                    .await
                    .iter()
                    .filter(|row| allowed.contains(row.client_id.as_str()))
                    .cloned()
                    .map(canonicalize_vps_rule_record)
                    .collect::<Result<Vec<_>>>()?;
                rows.sort_by(|left, right| {
                    left.client_id
                        .cmp(&right.client_id)
                        .then_with(|| left.key.cmp(&right.key))
                });
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        client_id,
                        key,
                        value_raw,
                        value_json,
                        source_kind,
                        source_id,
                        updated_by,
                        updated_at::text AS updated_at
                    FROM vps_rule_values
                    WHERE client_id = ANY($1::TEXT[])
                    ORDER BY client_id ASC, key ASC
                    "#,
                )
                .bind(client_ids)
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(vps_rule_from_row).collect()
            }
        }
    }

    pub(crate) async fn network_rate_interface_selection_for_clients(
        &self,
        client_ids: &[String],
    ) -> Result<NetworkRateInterfaceSelection> {
        let rules = self
            .list_vps_rules_for_clients(
                client_ids,
                &[
                    VPS_RULE_KEY_NETWORK_RATE_INTERFACES,
                    VPS_RULE_KEY_TRAFFIC_SELECTORS,
                ],
            )
            .await?;
        resolve_network_rate_interface_selection(client_ids, &rules)
    }

    pub(crate) fn network_rate_interface_selection_from_rules(
        client_ids: &[String],
        rules: &[VpsRuleValueRecord],
    ) -> Result<NetworkRateInterfaceSelection> {
        resolve_network_rate_interface_selection(client_ids, rules)
    }

    pub(crate) async fn dry_run_vps_rules(
        &self,
        request: &VpsRulesDryRunRequest,
    ) -> Result<VpsRulesDryRunResponse> {
        let operation = request.operation.trim().to_ascii_lowercase();
        anyhow::ensure!(
            operation == "upsert" || operation == "unset",
            "vps_rules_operation_invalid"
        );
        let (values, keys) = if operation == "upsert" {
            (normalize_vps_rule_values(&request.values)?, Vec::new())
        } else {
            (BTreeMap::new(), normalize_vps_rule_keys(&request.keys)?)
        };
        self.vps_rule_preview(&operation, &request.selector_expression, &values, &keys)
            .await
    }

    pub(crate) async fn bulk_upsert_vps_rules(
        &self,
        request: &VpsRulesBulkUpsertRequest,
        operator: &AuthContext,
    ) -> Result<VpsRulesDryRunResponse> {
        anyhow::ensure!(request.confirmed, "vps_rules_confirmation_required");
        let values = normalize_vps_rule_values(&request.values)?;
        validate_vps_rule_values(&values)?;
        let preview = self
            .commit_confirmed_vps_rule_changes(
                "upsert",
                &request.selector_expression,
                &values,
                &[],
                &request.preview_hash,
                operator,
            )
            .await?;
        if preview.changed_row_count > 0 {
            if let Err(error) = self.evaluate_policy_rules().await {
                tracing::warn!(%error, "deferred policy evaluation after VPS rule update");
            }
        }
        Ok(preview)
    }

    pub(crate) async fn bulk_unset_vps_rules(
        &self,
        request: &VpsRulesBulkUnsetRequest,
        operator: &AuthContext,
    ) -> Result<VpsRulesDryRunResponse> {
        anyhow::ensure!(request.confirmed, "vps_rules_confirmation_required");
        let keys = normalize_vps_rule_keys(&request.keys)?;
        let preview = self
            .commit_confirmed_vps_rule_changes(
                "unset",
                &request.selector_expression,
                &BTreeMap::new(),
                &keys,
                &request.preview_hash,
                operator,
            )
            .await?;
        if preview.changed_row_count > 0 {
            if let Err(error) = self.evaluate_policy_rules().await {
                tracing::warn!(%error, "deferred policy evaluation after VPS rule removal");
            }
        }
        Ok(preview)
    }

    async fn commit_confirmed_vps_rule_changes(
        &self,
        operation: &str,
        selector_expression: &str,
        values: &BTreeMap<String, String>,
        keys: &[String],
        expected_preview_hash: &str,
        operator: &AuthContext,
    ) -> Result<VpsRulesDryRunResponse> {
        match self {
            Self::Memory(memory) => {
                // Keep rule mutations before the shared agent lifecycle lock. Agent/tag
                // writers only take the latter, so there is no reverse lock order.
                let _rule_mutation_guard = memory.vps_rule_mutation.lock().await;
                let _agent_lifecycle_guard = memory.agent_key_lifecycle.lock().await;
                let preview = self
                    .vps_rule_preview(operation, selector_expression, values, keys)
                    .await?;
                validate_confirmed_vps_rule_preview(&preview, expected_preview_hash)?;
                if preview.changed_row_count > 0 {
                    apply_vps_rule_changes_memory(memory, &preview, operator).await?;
                }
                Ok(preview)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_postgres_vps_rule_mutations(&mut tx).await?;
                // Lock target visibility/tags in the same order used by Memory. Keeping
                // both locks in this transaction makes max_connections=1 safe and lets
                // cancellation release them automatically by rolling the transaction back.
                lock_postgres_agent_identity_lifecycle(&mut tx).await?;
                let (agents, stored) = postgres_vps_rule_snapshot_in_tx(&mut tx).await?;
                let preview = build_vps_rule_preview(
                    operation,
                    selector_expression,
                    values,
                    keys,
                    &agents,
                    &stored,
                )?;
                validate_confirmed_vps_rule_preview(&preview, expected_preview_hash)?;
                if preview.changed_row_count > 0 {
                    apply_vps_rule_changes_postgres_in_tx(&mut tx, &preview, operator).await?;
                }
                tx.commit().await?;
                Ok(preview)
            }
        }
    }

    pub(crate) async fn list_traffic_accounting(
        &self,
        query: &TrafficAccountingQuery,
    ) -> Result<Vec<TrafficAccountingRecord>> {
        self.list_traffic_accounting_at(query, Utc::now()).await
    }

    async fn list_traffic_accounting_at(
        &self,
        query: &TrafficAccountingQuery,
        now: DateTime<Utc>,
    ) -> Result<Vec<TrafficAccountingRecord>> {
        let agents = self.list_agents().await?;
        let rules = self
            .list_all_vps_rules_for_clients(
                &agents
                    .iter()
                    .map(|agent| agent.id.clone())
                    .collect::<Vec<_>>(),
            )
            .await?;
        let rule_contexts = vps_rule_contexts_by_client(&rules);
        let mut selected_agents = if let Some(selector) = query.selector_expression.as_deref() {
            resolve_agents_with_rule_contexts(&agents, selector, &rule_contexts)?
        } else {
            agents
        };
        if let Some(client_id) = query.client_id.as_deref() {
            selected_agents.retain(|agent| agent.id == client_id);
        }
        let mut records = self
            .traffic_accounting_for_selected_agents_with_rules(&selected_agents, &rules, now)
            .await?;
        records.retain(|record| {
            query
                .state
                .as_deref()
                .is_none_or(|state| record.state == state)
        });
        records.sort_by(|left, right| left.client_id.cmp(&right.client_id));
        records.truncate(query.limit.unwrap_or(1000).clamp(1, 5000) as usize);
        Ok(records)
    }

    pub(crate) async fn list_traffic_accounting_for_agents_with_rules(
        &self,
        agents: &[AgentView],
        rules: &[VpsRuleValueRecord],
    ) -> Result<Vec<TrafficAccountingRecord>> {
        let mut records = self
            .traffic_accounting_for_selected_agents_with_rules(agents, rules, Utc::now())
            .await?;
        records.sort_by(|left, right| left.client_id.cmp(&right.client_id));
        Ok(records)
    }

    /// Projects the unfiltered snapshot page without aggregating traffic for
    /// clients that cannot survive its canonical client-id sort and limit.
    /// `traffic_accounting_for_agents` emits exactly one row per supplied
    /// visible agent, and `list_traffic_accounting` applies no other filter for
    /// the full-snapshot query, so selecting the first client IDs here is
    /// equivalent to sorting every projected row and truncating afterward.
    pub(crate) async fn list_snapshot_traffic_accounting_with_context(
        &self,
        agents: &[AgentView],
        rules: &[VpsRuleValueRecord],
        limit: i64,
    ) -> Result<Vec<TrafficAccountingRecord>> {
        let mut selected_agents = agents.to_vec();
        selected_agents.sort_by(|left, right| left.id.cmp(&right.id));
        selected_agents.truncate(limit.clamp(1, 5000) as usize);
        self.list_traffic_accounting_for_agents_with_rules(&selected_agents, rules)
            .await
    }

    async fn traffic_accounting_for_selected_agents_with_rules(
        &self,
        selected_agents: &[AgentView],
        rules: &[VpsRuleValueRecord],
        now: DateTime<Utc>,
    ) -> Result<Vec<TrafficAccountingRecord>> {
        let cycle_starts = traffic_cycle_starts_for_clients(
            selected_agents.iter().map(|agent| agent.id.as_str()),
            rules,
            now,
        );
        let stream_requests = traffic_stream_requests_from_rules(&cycle_starts, rules)
            .into_iter()
            .collect::<Vec<_>>();
        let traffic_usage = self
            .list_traffic_counter_usage_for_streams(&stream_requests, now.timestamp())
            .await?;
        Ok(traffic_accounting_for_agents(
            selected_agents,
            rules,
            &traffic_usage,
            now,
        ))
    }

    pub(crate) async fn get_traffic_accounting(
        &self,
        client_id: &str,
    ) -> Result<TrafficAccountingRecord> {
        let client_ids = vec![client_id.to_string()];
        let agents = self.list_agents_for_client_ids(&client_ids).await?;
        let rules = self.list_all_vps_rules_for_clients(&client_ids).await?;
        self.traffic_accounting_for_selected_agents_with_rules(&agents, &rules, Utc::now())
            .await?
            .into_iter()
            .next()
            .context("traffic_accounting_not_found")
    }

    pub(crate) async fn traffic_history_start_unix(&self, client_id: &str) -> Result<Option<u64>> {
        let streams = self.traffic_history_streams(client_id).await?;
        if streams.is_empty() {
            return Ok(None);
        }
        match self {
            Self::Memory(memory) => {
                let samples = memory.traffic_counter_samples.read().await;
                let rollups = memory.traffic_counter_rollups.read().await;
                Ok(samples
                    .iter()
                    .filter(|sample| sample.client_id == client_id)
                    .filter(|sample| {
                        streams.iter().any(|stream| {
                            stream.source_kind == sample.source_kind
                                && stream.interface == sample.interface
                        })
                    })
                    .filter_map(|sample| u64::try_from(sample.observed_unix).ok())
                    .chain(
                        rollups
                            .iter()
                            .filter(|rollup| rollup.client_id == client_id)
                            .filter(|rollup| {
                                streams.iter().any(|stream| {
                                    stream.source_kind == rollup.source_kind
                                        && stream.interface == rollup.interface
                                })
                            })
                            .filter_map(|rollup| u64::try_from(rollup.bucket_start_unix).ok()),
                    )
                    .min())
            }
            Self::Postgres(pool) => {
                let source_kinds = streams
                    .iter()
                    .map(|stream| stream.source_kind.clone())
                    .collect::<Vec<_>>();
                let interfaces = streams
                    .iter()
                    .map(|stream| stream.interface.clone())
                    .collect::<Vec<_>>();
                let value = sqlx::query_scalar::<_, Option<f64>>(
                    r#"
                    WITH requested AS (
                        SELECT source_kind, interface
                        FROM UNNEST($2::text[], $3::text[])
                            AS stream(source_kind, interface)
                    )
                    SELECT min(history.observed_unix)::double precision
                    FROM (
                        SELECT extract(epoch FROM sample.observed_at) AS observed_unix
                        FROM traffic_counter_samples sample
                        JOIN requested
                          ON requested.source_kind = sample.source_kind
                         AND requested.interface = sample.interface
                        WHERE sample.client_id = $1
                        UNION ALL
                        SELECT extract(epoch FROM rollup.bucket_start) AS observed_unix
                        FROM traffic_counter_rollups rollup
                        JOIN requested
                          ON requested.source_kind = rollup.source_kind
                         AND requested.interface = rollup.interface
                        WHERE rollup.client_id = $1
                    ) history
                    "#,
                )
                .bind(client_id)
                .bind(&source_kinds)
                .bind(&interfaces)
                .fetch_one(pool)
                .await?;
                Ok(value
                    .filter(|value| value.is_finite() && *value >= 0.0)
                    .map(|value| value as u64))
            }
        }
    }

    pub(crate) async fn list_traffic_history(
        &self,
        client_id: &str,
        start_unix: u64,
        end_unix: u64,
        step_secs: i32,
        raw: bool,
    ) -> Result<Vec<TrafficHistoryPointView>> {
        let streams = self.traffic_history_streams(client_id).await?;
        if streams.is_empty() || start_unix > end_unix {
            return Ok(Vec::new());
        }
        let step_secs = step_secs.max(60);
        match self {
            Self::Memory(memory) => {
                if raw {
                    let live_samples = raw_memory_traffic_samples(
                        &memory.telemetry_samples.read().await,
                        client_id,
                        &streams,
                    )?;
                    let exact_samples = memory
                        .traffic_counter_samples
                        .read()
                        .await
                        .iter()
                        .filter(|sample| sample.client_id == client_id)
                        .cloned()
                        .collect::<Vec<_>>();
                    Ok(aggregate_memory_raw_traffic_history(
                        live_samples,
                        &exact_samples,
                        &streams,
                        start_unix,
                        end_unix,
                        step_secs,
                    ))
                } else {
                    let samples = memory
                        .traffic_counter_samples
                        .read()
                        .await
                        .iter()
                        .filter(|sample| sample.client_id == client_id)
                        .cloned()
                        .collect();
                    let rollups = memory
                        .traffic_counter_rollups
                        .read()
                        .await
                        .iter()
                        .filter(|rollup| rollup.client_id == client_id)
                        .cloned()
                        .collect::<Vec<_>>();
                    Ok(aggregate_memory_traffic_history(
                        samples, &rollups, &streams, start_unix, end_unix, step_secs,
                    ))
                }
            }
            Self::Postgres(pool) => {
                let source_kinds = streams
                    .iter()
                    .map(|stream| stream.source_kind.clone())
                    .collect::<Vec<_>>();
                let interfaces = streams
                    .iter()
                    .map(|stream| stream.interface.clone())
                    .collect::<Vec<_>>();
                let direction_masks = streams
                    .iter()
                    .map(|stream| stream.direction_mask)
                    .collect::<Vec<_>>();
                let rows = if raw {
                    sqlx::query(
                        r#"
                        WITH requested AS (
                            SELECT source_kind, interface, direction_mask
                            FROM UNNEST($2::text[], $3::text[], $4::integer[])
                                AS stream(source_kind, interface, direction_mask)
                        ), expanded_range AS (
                            SELECT
                                requested.source_kind,
                                requested.interface,
                                requested.direction_mask,
                                fact.sample_id,
                                fact.ordinal,
                                fact.observed_at,
                                fact.rx_bytes,
                                fact.tx_bytes
                            FROM telemetry_counter_facts fact
                            JOIN requested
                              ON requested.source_kind = fact.source_kind
                             AND requested.interface = fact.interface
                            WHERE fact.client_id = $1
                              AND fact.observed_at >= to_timestamp($5)
                              AND fact.observed_at <= to_timestamp($6)
                        ), baseline AS (
                            SELECT
                                requested.source_kind,
                                requested.interface,
                                requested.direction_mask,
                                previous.sample_id,
                                previous.ordinal,
                                previous.observed_at,
                                previous.rx_bytes,
                                previous.tx_bytes
                            FROM requested
                            JOIN LATERAL (
                                SELECT
                                    fact.sample_id,
                                    fact.ordinal,
                                    fact.observed_at,
                                    fact.rx_bytes,
                                    fact.tx_bytes
                                FROM telemetry_counter_facts fact
                                WHERE fact.client_id = $1
                                  AND fact.observed_at < to_timestamp($5)
                                  AND fact.source_kind = requested.source_kind
                                  AND fact.interface = requested.interface
                                ORDER BY fact.observed_at DESC, fact.sample_id DESC, fact.ordinal DESC
                                LIMIT 1
                            ) previous ON TRUE
                        ), selected AS (
                            SELECT * FROM expanded_range
                            UNION ALL
                            SELECT * FROM baseline
                        ), sequenced AS (
                            SELECT
                                selected.*,
                                lag(rx_bytes) OVER stream AS previous_rx_bytes,
                                lag(tx_bytes) OVER stream AS previous_tx_bytes
                            FROM selected
                            WINDOW stream AS (
                                PARTITION BY source_kind, interface
                                ORDER BY observed_at, sample_id, ordinal
                            )
                        ), raw_deltas AS (
                            SELECT
                                floor(
                                    extract(epoch FROM observed_at)::double precision
                                        / $7::double precision
                                )::bigint * $7::bigint AS bucket_epoch,
                                direction_mask,
                                rx_bytes,
                                tx_bytes,
                                previous_rx_bytes,
                                previous_tx_bytes,
                                (direction_mask & 1) <> 0 AS selected_rx,
                                (direction_mask & 2) <> 0 AS selected_tx,
                                rx_bytes >= previous_rx_bytes AS valid_rx,
                                tx_bytes >= previous_tx_bytes AS valid_tx
                            FROM sequenced
                            WHERE observed_at >= to_timestamp($5)
                              AND observed_at <= to_timestamp($6)
                              AND previous_rx_bytes IS NOT NULL
                        ), import_range AS (
                            SELECT
                                requested.source_kind,
                                requested.interface,
                                requested.direction_mask,
                                sample.observed_at,
                                sample.rx_bytes,
                                sample.tx_bytes,
                                sample.rx_counter_epoch,
                                sample.tx_counter_epoch,
                                sample.sample_source
                            FROM traffic_counter_samples sample
                            JOIN requested
                              ON requested.source_kind = sample.source_kind
                             AND requested.interface = sample.interface
                            WHERE sample.client_id = $1
                              AND sample.observed_at >= to_timestamp($5)
                              AND sample.observed_at <= to_timestamp($6)
                              AND sample.sample_source LIKE 'vnstat_import:%'
                              -- Promoted exact rows are retained only as
                              -- sequencing boundaries; their inbound delta is
                              -- already represented by a rollup.
                              AND NOT sample.inbound_promoted
                        ), import_first AS (
                            SELECT
                                source_kind,
                                interface,
                                direction_mask,
                                min(observed_at) AS observed_at
                            FROM import_range
                            GROUP BY source_kind, interface, direction_mask
                        ), import_baseline AS (
                            SELECT
                                import_first.source_kind,
                                import_first.interface,
                                import_first.direction_mask,
                                previous.observed_at,
                                previous.rx_bytes,
                                previous.tx_bytes,
                                previous.rx_counter_epoch,
                                previous.tx_counter_epoch,
                                previous.sample_source
                            FROM import_first
                            JOIN LATERAL (
                                SELECT
                                    sample.observed_at,
                                    sample.rx_bytes,
                                    sample.tx_bytes,
                                    sample.rx_counter_epoch,
                                    sample.tx_counter_epoch,
                                    sample.sample_source
                                FROM traffic_counter_samples sample
                                WHERE sample.client_id = $1
                                  AND sample.source_kind = import_first.source_kind
                                  AND sample.interface = import_first.interface
                                  AND sample.observed_at < import_first.observed_at
                                ORDER BY sample.observed_at DESC
                                LIMIT 1
                            ) previous ON TRUE
                        ), import_selected AS (
                            SELECT * FROM import_range
                            UNION ALL
                            SELECT * FROM import_baseline
                        ), import_sequenced AS (
                            SELECT
                                import_selected.*,
                                lag(rx_bytes) OVER stream AS previous_rx_bytes,
                                lag(tx_bytes) OVER stream AS previous_tx_bytes,
                                lag(rx_counter_epoch) OVER stream
                                    AS previous_rx_counter_epoch,
                                lag(tx_counter_epoch) OVER stream
                                    AS previous_tx_counter_epoch
                            FROM import_selected
                            WINDOW stream AS (
                                PARTITION BY source_kind, interface
                                ORDER BY observed_at
                            )
                        ), import_deltas AS (
                            SELECT
                                floor(
                                    extract(epoch FROM observed_at)::double precision
                                        / $7::double precision
                                )::bigint * $7::bigint AS bucket_epoch,
                                direction_mask,
                                rx_bytes,
                                tx_bytes,
                                previous_rx_bytes,
                                previous_tx_bytes,
                                (direction_mask & 1) <> 0 AS selected_rx,
                                (direction_mask & 2) <> 0 AS selected_tx,
                                (rx_counter_epoch = previous_rx_counter_epoch
                                  AND rx_bytes >= previous_rx_bytes) AS valid_rx,
                                (tx_counter_epoch = previous_tx_counter_epoch
                                  AND tx_bytes >= previous_tx_bytes) AS valid_tx
                            FROM import_sequenced
                            WHERE observed_at >= to_timestamp($5)
                              AND observed_at <= to_timestamp($6)
                              AND sample_source LIKE 'vnstat_import:%'
                              AND previous_rx_bytes IS NOT NULL
                        ), deltas AS (
                            -- Keep raw live and imported durable sequences
                            -- independent; importing is restricted to the
                            -- pre-live interval, so these facts are additive.
                            SELECT * FROM raw_deltas
                            UNION ALL
                            SELECT * FROM import_deltas
                        )
                        SELECT
                            to_timestamp(bucket_epoch)::text AS bucket_start,
                            $7::integer AS bucket_secs,
                            count(*) FILTER (
                                WHERE (selected_rx AND valid_rx)
                                   OR (selected_tx AND valid_tx)
                            )::integer AS sample_count,
                            count(*) FILTER (
                                WHERE (selected_rx AND NOT valid_rx)
                                   OR (selected_tx AND NOT valid_tx)
                            )::integer AS reset_count,
                            CASE
                            WHEN count(*) FILTER (
                                WHERE (selected_rx AND valid_rx)
                                   OR (selected_tx AND valid_tx)
                            ) = 0 THEN NULL
                            WHEN bool_or(selected_rx) AND count(*) FILTER (
                                WHERE selected_rx AND valid_rx
                            ) = 0 THEN NULL
                            ELSE
                                COALESCE(sum(
                                    CASE WHEN selected_rx AND valid_rx
                                        THEN rx_bytes - previous_rx_bytes ELSE 0 END
                                ), 0)::bigint
                            END AS rx_bytes,
                            CASE
                            WHEN count(*) FILTER (
                                WHERE (selected_rx AND valid_rx)
                                   OR (selected_tx AND valid_tx)
                            ) = 0 THEN NULL
                            WHEN bool_or(selected_tx) AND count(*) FILTER (
                                WHERE selected_tx AND valid_tx
                            ) = 0 THEN NULL
                            ELSE
                                COALESCE(sum(
                                    CASE WHEN selected_tx AND valid_tx
                                        THEN tx_bytes - previous_tx_bytes ELSE 0 END
                                ), 0)::bigint
                            END AS tx_bytes
                        FROM deltas
                        GROUP BY bucket_epoch
                        ORDER BY bucket_epoch
                        "#,
                    )
                    .bind(client_id)
                    .bind(&source_kinds)
                    .bind(&interfaces)
                    .bind(&direction_masks)
                    .bind(start_unix as i64)
                    .bind(end_unix as i64)
                    .bind(step_secs)
                    .fetch_all(pool)
                    .await?
                } else {
                    sqlx::query(
                        r#"
                        WITH requested AS (
                            SELECT source_kind, interface, direction_mask
                            FROM UNNEST($2::text[], $3::text[], $4::integer[])
                                AS stream(source_kind, interface, direction_mask)
                        ), raw_range AS (
                            SELECT
                                sample.source_kind,
                                sample.interface,
                                requested.direction_mask,
                                sample.observed_at,
                                sample.rx_bytes,
                                sample.tx_bytes,
                                sample.rx_counter_epoch,
                                sample.tx_counter_epoch,
                                sample.sample_source
                            FROM traffic_counter_samples sample
                            JOIN requested
                              ON requested.source_kind = sample.source_kind
                             AND requested.interface = sample.interface
                            WHERE sample.client_id = $1
                              AND sample.observed_at >= to_timestamp($5)
                              AND sample.observed_at <= to_timestamp($6)
                              AND NOT sample.inbound_promoted
                        ), raw_baseline AS (
                            SELECT
                                requested.source_kind,
                                requested.interface,
                                requested.direction_mask,
                                previous.observed_at,
                                previous.rx_bytes,
                                previous.tx_bytes,
                                previous.rx_counter_epoch,
                                previous.tx_counter_epoch,
                                previous.sample_source
                            FROM requested
                            JOIN LATERAL (
                                SELECT min(sample.observed_at) AS observed_at
                                FROM traffic_counter_samples sample
                                WHERE sample.client_id = $1
                                  AND sample.source_kind = requested.source_kind
                                  AND sample.interface = requested.interface
                                  AND sample.observed_at >= to_timestamp($5)
                                  AND sample.observed_at <= to_timestamp($6)
                                  AND NOT sample.inbound_promoted
                            ) first_raw ON first_raw.observed_at IS NOT NULL
                            JOIN LATERAL (
                                SELECT
                                    observed_at,
                                    rx_bytes,
                                    tx_bytes,
                                    rx_counter_epoch,
                                    tx_counter_epoch,
                                    sample_source
                                FROM traffic_counter_samples sample
                                WHERE sample.client_id = $1
                                  AND sample.source_kind = requested.source_kind
                                  AND sample.interface = requested.interface
                                  AND sample.observed_at < first_raw.observed_at
                                ORDER BY sample.observed_at DESC
                                LIMIT 1
                            ) previous ON TRUE
                        ), raw_selected AS (
                            SELECT * FROM raw_range
                            UNION ALL
                            SELECT * FROM raw_baseline
                        ), raw_sequenced AS (
                            SELECT
                                raw_selected.*,
                                lag(rx_bytes) OVER stream AS previous_rx_bytes,
                                lag(tx_bytes) OVER stream AS previous_tx_bytes,
                                lag(rx_counter_epoch) OVER stream AS previous_rx_counter_epoch,
                                lag(tx_counter_epoch) OVER stream AS previous_tx_counter_epoch,
                                lag(sample_source) OVER stream AS previous_sample_source
                            FROM raw_selected
                            WINDOW stream AS (
                                PARTITION BY source_kind, interface
                                ORDER BY observed_at
                            )
                        ), raw_native AS (
                            SELECT
                                floor(
                                    extract(epoch FROM observed_at)::double precision / 60.0
                                )::bigint * 60::bigint AS bucket_epoch,
                                60::integer AS native_secs,
                                direction_mask,
                                CASE WHEN rx_counter_epoch = previous_rx_counter_epoch
                                           AND rx_bytes >= previous_rx_bytes
                                     THEN rx_bytes - previous_rx_bytes ELSE 0 END::bigint
                                    AS rx_bytes,
                                CASE WHEN tx_counter_epoch = previous_tx_counter_epoch
                                           AND tx_bytes >= previous_tx_bytes
                                     THEN tx_bytes - previous_tx_bytes ELSE 0 END::bigint
                                    AS tx_bytes,
                                (rx_counter_epoch = previous_rx_counter_epoch
                                  AND rx_bytes >= previous_rx_bytes)::integer AS rx_valid_count,
                                (tx_counter_epoch = previous_tx_counter_epoch
                                  AND tx_bytes >= previous_tx_bytes)::integer AS tx_valid_count,
                                ((rx_counter_epoch = previous_rx_counter_epoch
                                    AND rx_bytes >= previous_rx_bytes)
                                  OR (tx_counter_epoch = previous_tx_counter_epoch
                                    AND tx_bytes >= previous_tx_bytes))::integer
                                    AS any_valid_count,
                                (NOT (
                                    previous_sample_source LIKE 'vnstat_import:%'
                                    AND sample_source NOT LIKE 'vnstat_import:%'
                                 ) AND rx_counter_epoch <> previous_rx_counter_epoch)::integer
                                    AS rx_reset_count,
                                (NOT (
                                    previous_sample_source LIKE 'vnstat_import:%'
                                    AND sample_source NOT LIKE 'vnstat_import:%'
                                 ) AND tx_counter_epoch <> previous_tx_counter_epoch)::integer
                                    AS tx_reset_count,
                                (NOT (
                                    previous_sample_source LIKE 'vnstat_import:%'
                                    AND sample_source NOT LIKE 'vnstat_import:%'
                                 ) AND (
                                    rx_counter_epoch <> previous_rx_counter_epoch
                                    OR tx_counter_epoch <> previous_tx_counter_epoch
                                 ))::integer AS any_reset_count
                            FROM raw_sequenced
                            WHERE observed_at >= to_timestamp($5)
                              AND observed_at <= to_timestamp($6)
                              AND previous_rx_bytes IS NOT NULL
                        ), retained_native AS (
                            SELECT
                                floor(
                                    extract(epoch FROM rollup.bucket_start)::double precision
                                )::bigint AS bucket_epoch,
                                rollup.bucket_secs AS native_secs,
                                requested.direction_mask,
                                rollup.rx_bytes,
                                rollup.tx_bytes,
                                rollup.rx_valid_count,
                                rollup.tx_valid_count,
                                rollup.any_valid_count,
                                rollup.rx_reset_count,
                                rollup.tx_reset_count,
                                rollup.any_reset_count
                            FROM traffic_counter_rollups rollup
                            JOIN requested
                              ON requested.source_kind = rollup.source_kind
                             AND requested.interface = rollup.interface
                            WHERE rollup.client_id = $1
                              AND rollup.bucket_start
                                    < to_timestamp($6)
                                        + make_interval(secs => 1)
                              AND rollup.bucket_start
                                    + make_interval(secs => rollup.bucket_secs)
                                    > to_timestamp($5)
                              AND NOT EXISTS (
                                    SELECT 1
                                    FROM traffic_counter_rollups finer
                                    WHERE finer.client_id = rollup.client_id
                                      AND finer.source_kind = rollup.source_kind
                                      AND finer.interface = rollup.interface
                                      AND finer.origin_kind = rollup.origin_kind
                                      AND finer.bucket_secs < rollup.bucket_secs
                                      AND finer.bucket_start < rollup.bucket_start
                                            + make_interval(secs => rollup.bucket_secs)
                                      AND finer.bucket_start
                                            + make_interval(secs => finer.bucket_secs)
                                            > rollup.bucket_start
                                    -- Keep this interval-existence check
                                    -- correlated. PostgreSQL otherwise
                                    -- flattens it into a quadratic hash
                                    -- anti-join for long retained ranges.
                                    OFFSET 0
                              )
                              AND NOT EXISTS (
                                    SELECT 1
                                    FROM traffic_counter_samples exact
                                    WHERE exact.client_id = rollup.client_id
                                      AND exact.source_kind = rollup.source_kind
                                      AND exact.interface = rollup.interface
                                      AND NOT exact.inbound_promoted
                                      AND (CASE
                                            WHEN exact.sample_source LIKE 'vnstat_import:%'
                                            THEN 'vnstat_import' ELSE 'live'
                                          END) = rollup.origin_kind
                                      AND exact.observed_at >= rollup.bucket_start
                                      AND exact.observed_at < rollup.bucket_start
                                            + make_interval(secs => rollup.bucket_secs)
                                    -- Preserve indexed per-bucket probes for
                                    -- the exact tail instead of comparing
                                    -- every exact row with every rollup.
                                    OFFSET 0
                              )
                        ), native AS (
                            SELECT * FROM raw_native
                            UNION ALL
                            SELECT * FROM retained_native
                        ), output AS (
                            SELECT
                                floor(
                                    bucket_epoch::double precision
                                        / GREATEST($7::integer, native_secs)::double precision
                                )::bigint * GREATEST($7::integer, native_secs)::bigint
                                    AS output_epoch,
                                GREATEST($7::integer, native_secs) AS output_secs,
                                direction_mask,
                                rx_bytes,
                                tx_bytes,
                                rx_valid_count,
                                tx_valid_count,
                                any_valid_count,
                                rx_reset_count,
                                tx_reset_count,
                                any_reset_count
                            FROM native
                        )
                        SELECT
                            to_timestamp(output_epoch)::text AS bucket_start,
                            output_secs AS bucket_secs,
                            LEAST(sum(CASE direction_mask
                                WHEN 1 THEN rx_valid_count
                                WHEN 2 THEN tx_valid_count
                                ELSE any_valid_count
                            END), 2147483647)::integer AS sample_count,
                            LEAST(sum(CASE direction_mask
                                WHEN 1 THEN rx_reset_count
                                WHEN 2 THEN tx_reset_count
                                ELSE any_reset_count
                            END), 2147483647)::integer AS reset_count,
                            CASE
                            WHEN sum(CASE direction_mask
                                WHEN 1 THEN rx_valid_count
                                WHEN 2 THEN tx_valid_count
                                ELSE any_valid_count
                            END) = 0
                              OR (bool_or((direction_mask & 1) <> 0)
                                  AND sum(CASE
                                      WHEN (direction_mask & 1) <> 0
                                      THEN rx_valid_count ELSE 0
                                  END) = 0)
                            THEN NULL
                            ELSE COALESCE(sum(CASE
                                WHEN (direction_mask & 1) <> 0 THEN rx_bytes ELSE 0
                            END), 0)::bigint
                            END AS rx_bytes,
                            CASE
                            WHEN sum(CASE direction_mask
                                WHEN 1 THEN rx_valid_count
                                WHEN 2 THEN tx_valid_count
                                ELSE any_valid_count
                            END) = 0
                              OR (bool_or((direction_mask & 2) <> 0)
                                  AND sum(CASE
                                      WHEN (direction_mask & 2) <> 0
                                      THEN tx_valid_count ELSE 0
                                  END) = 0)
                            THEN NULL
                            ELSE COALESCE(sum(CASE
                                WHEN (direction_mask & 2) <> 0 THEN tx_bytes ELSE 0
                            END), 0)::bigint
                            END AS tx_bytes
                        FROM output
                        GROUP BY output_epoch, output_secs
                        ORDER BY output_epoch, output_secs
                        "#,
                    )
                    .bind(client_id)
                    .bind(&source_kinds)
                    .bind(&interfaces)
                    .bind(&direction_masks)
                    .bind(start_unix as i64)
                    .bind(end_unix as i64)
                    .bind(step_secs)
                    .fetch_all(pool)
                    .await?
                };
                rows.into_iter()
                    .map(|row| {
                        let rx_bytes: Option<i64> = row.try_get("rx_bytes")?;
                        let tx_bytes: Option<i64> = row.try_get("tx_bytes")?;
                        Ok(TrafficHistoryPointView {
                            bucket_start: row.try_get("bucket_start")?,
                            bucket_secs: row.try_get("bucket_secs")?,
                            sample_count: row.try_get("sample_count")?,
                            reset_count: row.try_get("reset_count")?,
                            rx_bytes,
                            tx_bytes,
                            total_bytes: rx_bytes
                                .zip(tx_bytes)
                                .map(|(rx, tx)| rx.saturating_add(tx)),
                        })
                    })
                    .collect()
            }
        }
    }

    async fn traffic_history_streams(&self, client_id: &str) -> Result<Vec<TrafficHistoryStream>> {
        let rules = self
            .list_vps_rules_matching(
                &VpsRuleQuery {
                    limit: None,
                    client_id: Some(client_id.to_string()),
                    selector_expression: None,
                    key: Some(VPS_RULE_KEY_TRAFFIC_SELECTORS.to_string()),
                    state: None,
                },
                None,
            )
            .await?;
        let Some(rule) = rules.into_iter().next() else {
            return Ok(Vec::new());
        };
        let Ok(selectors) = traffic_selectors_from_rule(&rule) else {
            return Ok(Vec::new());
        };
        let mut streams = BTreeSet::<(String, String)>::new();
        for selector in selectors {
            streams.insert((selector.source, selector.interface));
        }
        Ok(streams
            .into_iter()
            .map(|(source_kind, interface)| TrafficHistoryStream {
                source_kind,
                interface,
                // Billing direction controls accounting only. Diagnostic
                // history always exposes both counters for a selected stream.
                direction_mask: 0b11,
            })
            .collect())
    }

    pub(crate) async fn dry_run_fleet_alert_policy(
        &self,
        request: &PolicyDryRunRequest,
    ) -> Result<PolicyDryRunResponse> {
        validate_policy_group_request(
            &CreateFleetAlertPolicyRequest {
                id: request.id,
                name: request.name.clone(),
                enabled: request.enabled,
                selector_expression: request.selector_expression.clone(),
                rules: request.rules.clone(),
                notes: request.notes.clone(),
                confirmed: true,
                preview_hash: None,
            },
            false,
            false,
        )?;
        let agents = self.list_agents().await?;
        let mut validation_errors = Vec::new();
        let rules = self
            .list_vps_rules_matching(
                &VpsRuleQuery {
                    limit: None,
                    client_id: None,
                    selector_expression: None,
                    key: None,
                    state: None,
                },
                None,
            )
            .await?;
        let rule_contexts = vps_rule_contexts_by_client(&rules);
        let matched = resolve_agents_with_rule_contexts(
            &agents,
            &request.selector_expression,
            &rule_contexts,
        )?;
        let mut matched_client_ids = matched
            .iter()
            .map(|agent| agent.id.clone())
            .collect::<Vec<_>>();
        matched_client_ids.sort();
        let mut rule_previews = Vec::new();
        let mut incomplete_clients = BTreeSet::new();
        for rule in &request.rules {
            match validate_policy_rule_request_for_preview(rule) {
                Ok(()) => {}
                Err(error) => {
                    validation_errors.push(error.to_string());
                    continue;
                }
            }
            let preview_mode = if rule.rule_kind == AlertPolicyRuleKind::Occurrence {
                "prospective"
            } else {
                "current"
            };
            let (true_count, false_count, incomplete_count, incomplete_subjects) =
                if rule.rule_kind == AlertPolicyRuleKind::Occurrence {
                    (0, 0, 0, Vec::new())
                } else if let Self::Postgres(pool) = self {
                    preview_current_policy_rule(pool, rule, &matched_client_ids).await?
                } else {
                    // Memory does not own the durable neutral evidence stream.
                    // Report unavailable current facts as incomplete rather
                    // than fabricating false metric/state evaluations.
                    (
                        0,
                        0,
                        matched_client_ids.len() as i64,
                        matched_client_ids.clone(),
                    )
                };
            incomplete_clients.extend(incomplete_subjects);
            rule_previews.push(PolicyDryRunRulePreview {
                rule_name: rule.name.clone(),
                preview_mode: preview_mode.to_string(),
                trigger_condition_expression: rule.trigger_condition_expression.clone(),
                trigger_meta_condition: canonical_policy_meta(rule.trigger_meta_condition.as_ref()),
                resolve_condition_expression: rule.resolve_condition_expression.clone(),
                resolve_meta_condition: canonical_policy_meta(rule.resolve_meta_condition.as_ref()),
                category: policy_rule_category(rule),
                severity: rule.severity.clone(),
                true_count,
                false_count,
                incomplete_count,
            });
        }
        let preview_payload = json!({
            "id": request.id,
            "name": request.name.trim(),
            "enabled": request.enabled,
            "selector_expression": request.selector_expression.trim(),
            "notes": clean_optional_text(request.notes.as_deref()),
            "rules": request.rules.iter().map(normalized_policy_preview_rule).collect::<Vec<_>>(),
            "matched": &matched_client_ids,
        });
        Ok(PolicyDryRunResponse {
            matched_vps_count: matched.len(),
            invalid_rule_count: validation_errors.len(),
            incomplete_vps_count: incomplete_clients.len(),
            preview_hash: preview_hash(&preview_payload),
            matched_vps: matched.into_iter().map(|agent| agent.id).collect(),
            rule_previews,
            validation_errors,
        })
    }

    pub(crate) async fn list_fleet_alert_policies(
        &self,
        limit: i64,
        enabled: Option<bool>,
        selector_expression: Option<&str>,
        client_id: Option<&str>,
        allow_vps_rule_selectors: bool,
    ) -> Result<Vec<PolicyGroupRecord>> {
        let definition_limit = if selector_expression.is_none() && client_id.is_none() {
            Some(limit.clamp(1, 1000) as usize)
        } else {
            None
        };
        let mut groups = self
            .list_fleet_alert_policy_definitions(definition_limit, enabled)
            .await?;
        if let Some(enabled) = enabled {
            groups.retain(|group| group.enabled == enabled);
        }
        let query_expression = selector_expression
            .map(|selector| {
                parse_selector_expression(selector)
                    .map_err(|error| anyhow::anyhow!("invalid selector expression: {error}"))?
                    .context("selector expression is empty")
            })
            .transpose()?;
        let group_expressions = groups
            .iter()
            .map(|group| {
                parse_selector_expression(&group.selector_expression)
                    .map_err(|error| anyhow::anyhow!("invalid selector expression: {error}"))?
                    .context("selector expression is empty")
            })
            .collect::<Result<Vec<_>>>()?;
        anyhow::ensure!(
            allow_vps_rule_selectors
                || !query_expression
                    .iter()
                    .chain(group_expressions.iter())
                    .any(expression_references_vps_rules),
            "vps_rule_selector_scope_required"
        );
        let mut enrichment_context = None;
        if selector_expression.is_some() || client_id.is_some() {
            let agents = self.list_agents().await?;
            let rule_contexts = if query_expression
                .iter()
                .chain(group_expressions.iter())
                .any(expression_references_vps_rules)
            {
                self.vps_rule_contexts_for_agents(&agents).await?
            } else {
                HashMap::new()
            };
            let selected = query_expression.as_ref().map(|expression| {
                Repository::resolve_agents_for_expression_with_rule_contexts(
                    &agents,
                    expression,
                    &rule_contexts,
                )
                .into_iter()
                .map(|agent| agent.id)
                .collect::<HashSet<_>>()
            });
            let mut retained = Vec::with_capacity(groups.len());
            for (group, expression) in groups.into_iter().zip(group_expressions) {
                let matched = Repository::resolve_agents_for_expression_with_rule_contexts(
                    &agents,
                    &expression,
                    &rule_contexts,
                );
                let intersects_selected = selected.as_ref().is_none_or(|selected| {
                    matched.iter().any(|agent| selected.contains(&agent.id))
                });
                let contains_client = client_id
                    .is_none_or(|client_id| matched.iter().any(|agent| agent.id == client_id));
                if intersects_selected && contains_client {
                    retained.push(group);
                }
            }
            groups = retained;
            enrichment_context = Some((agents, rule_contexts));
        }
        if let Some((agents, rule_contexts)) = enrichment_context {
            self.enrich_policy_group_summaries_with_rule_contexts(
                &mut groups,
                &agents,
                &rule_contexts,
            )
            .await?;
        } else {
            self.enrich_policy_group_summaries(&mut groups).await?;
        }
        groups.sort_by(|left, right| {
            right
                .enabled
                .cmp(&left.enabled)
                .then_with(|| left.name.cmp(&right.name))
        });
        groups.truncate(limit.clamp(1, 1000) as usize);
        Ok(groups)
    }

    pub(crate) async fn list_fleet_alert_policies_with_context(
        &self,
        limit: i64,
        allow_vps_rule_selectors: bool,
        agents: &[AgentView],
        rules: &[VpsRuleValueRecord],
    ) -> Result<Vec<PolicyGroupRecord>> {
        let mut groups = self
            .list_fleet_alert_policy_definitions(Some(limit.clamp(1, 1000) as usize), None)
            .await?;
        let expressions = groups
            .iter()
            .map(|group| {
                parse_selector_expression(&group.selector_expression)
                    .map_err(|error| anyhow::anyhow!("invalid selector expression: {error}"))?
                    .context("selector expression is empty")
            })
            .collect::<Result<Vec<_>>>()?;
        anyhow::ensure!(
            allow_vps_rule_selectors || !expressions.iter().any(expression_references_vps_rules),
            "vps_rule_selector_scope_required"
        );
        let rule_contexts = if expressions.iter().any(expression_references_vps_rules) {
            vps_rule_contexts_by_client(rules)
        } else {
            HashMap::new()
        };
        self.enrich_policy_group_summaries_with_rule_contexts(&mut groups, agents, &rule_contexts)
            .await?;
        groups.sort_by(|left, right| {
            right
                .enabled
                .cmp(&left.enabled)
                .then_with(|| left.name.cmp(&right.name))
        });
        groups.truncate(limit.clamp(1, 1000) as usize);
        Ok(groups)
    }

    async fn list_fleet_alert_policy_definitions(
        &self,
        result_limit: Option<usize>,
        enabled: Option<bool>,
    ) -> Result<Vec<PolicyGroupRecord>> {
        let mut groups = match self {
            Self::Memory(memory) => memory.policy_groups.read().await.clone(),
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        name,
                        enabled,
                        selector_expression,
                        notes,
                        created_by,
                        updated_by,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM policy_groups
                    WHERE ($2::boolean IS NULL OR enabled = $2)
                    ORDER BY enabled DESC, name ASC
                    LIMIT $1
                    "#,
                )
                .bind(result_limit.map(|limit| limit as i64))
                .bind(enabled)
                .fetch_all(pool)
                .await?;
                let group_ids = rows
                    .iter()
                    .map(|row| row.try_get::<Uuid, _>("id"))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let mut rules_by_group = HashMap::<Uuid, Vec<PolicyRuleRecord>>::new();
                if !group_ids.is_empty() {
                    for row in sqlx::query(
                        r#"
                        SELECT
                            id,
                            group_id,
                            rule_version,
                            sort_order,
                            name,
                            enabled,
                            rule_kind,
                            evidence_source,
                            correlation_mode,
                            traffic_selector,
                            trigger_condition_expression,
                            trigger_meta_condition,
                            resolve_condition_expression,
                            resolve_meta_condition,
                            severity,
                            category,
                            title_template,
                            detail_template,
                            system_seed_key,
                            armed_after_evidence_seq,
                            armed_at::text AS armed_at,
                            created_at::text AS created_at,
                            updated_at::text AS updated_at
                        FROM policy_rules
                        WHERE group_id = ANY($1)
                        ORDER BY group_id ASC, sort_order ASC, created_at ASC
                        "#,
                    )
                    .bind(&group_ids)
                    .fetch_all(pool)
                    .await?
                    {
                        let rule = policy_rule_from_row(row)?;
                        rules_by_group.entry(rule.group_id).or_default().push(rule);
                    }
                }
                let mut groups = Vec::new();
                for row in rows {
                    let group_id: Uuid = row.try_get("id")?;
                    groups.push(policy_group_from_row(
                        row,
                        rules_by_group.remove(&group_id).unwrap_or_default(),
                    )?);
                }
                groups
            }
        };
        if let Some(enabled) = enabled {
            groups.retain(|group| group.enabled == enabled);
        }
        groups.sort_by(|left, right| {
            right
                .enabled
                .cmp(&left.enabled)
                .then_with(|| left.name.cmp(&right.name))
        });
        if let Some(result_limit) = result_limit {
            groups.truncate(result_limit);
        }
        Ok(groups)
    }

    async fn get_fleet_alert_policy_definition(&self, id: Uuid) -> Result<PolicyGroupRecord> {
        match self {
            Self::Memory(memory) => memory
                .policy_groups
                .read()
                .await
                .iter()
                .find(|group| group.id == id)
                .cloned()
                .context("fleet_alert_policy_not_found"),
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        id,
                        name,
                        enabled,
                        selector_expression,
                        notes,
                        created_by,
                        updated_by,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM policy_groups
                    WHERE id = $1
                    "#,
                )
                .bind(id)
                .fetch_optional(pool)
                .await?
                .context("fleet_alert_policy_not_found")?;
                let rules = sqlx::query(
                    r#"
                    SELECT
                        id,
                        group_id,
                        rule_version,
                        sort_order,
                        name,
                        enabled,
                        rule_kind,
                        evidence_source,
                        correlation_mode,
                        traffic_selector,
                        trigger_condition_expression,
                        trigger_meta_condition,
                        resolve_condition_expression,
                        resolve_meta_condition,
                        severity,
                        category,
                        title_template,
                        detail_template,
                        system_seed_key,
                        armed_after_evidence_seq,
                        armed_at::text AS armed_at,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM policy_rules
                    WHERE group_id = $1
                    ORDER BY sort_order ASC, created_at ASC
                    "#,
                )
                .bind(id)
                .fetch_all(pool)
                .await?
                .into_iter()
                .map(policy_rule_from_row)
                .collect::<Result<Vec<_>>>()?;
                policy_group_from_row(row, rules)
            }
        }
    }

    pub(crate) async fn get_fleet_alert_policy(
        &self,
        id: Uuid,
        allow_vps_rule_selectors: bool,
    ) -> Result<PolicyGroupRecord> {
        let group = self.get_fleet_alert_policy_definition(id).await?;
        let expression = parse_selector_expression(&group.selector_expression)
            .map_err(|error| anyhow::anyhow!("invalid selector expression: {error}"))?
            .context("selector expression is empty")?;
        anyhow::ensure!(
            allow_vps_rule_selectors || !expression_references_vps_rules(&expression),
            "vps_rule_selector_scope_required"
        );
        let mut groups = vec![group];
        self.enrich_policy_group_summaries(&mut groups).await?;
        groups.pop().context("fleet_alert_policy_not_found")
    }

    pub(crate) async fn upsert_fleet_alert_policy(
        &self,
        request: &CreateFleetAlertPolicyRequest,
        operator: &AuthContext,
    ) -> Result<PolicyGroupRecord> {
        validate_policy_group_request(request, true, true)?;
        let dry_run = self
            .dry_run_fleet_alert_policy(&PolicyDryRunRequest {
                id: request.id,
                name: request.name.clone(),
                enabled: request.enabled,
                selector_expression: request.selector_expression.clone(),
                rules: request.rules.clone(),
                notes: request.notes.clone(),
            })
            .await?;
        if let Some(hash) = request.preview_hash.as_deref() {
            anyhow::ensure!(
                hash == dry_run.preview_hash,
                "fleet_alert_policy_preview_hash_mismatch"
            );
        }
        let now = unix_now().to_string();
        let group = match self {
            Self::Memory(memory) => {
                let mut groups = memory.policy_groups.write().await;
                let existing_group =
                    select_existing_policy_group(&groups, request.id, request.name.trim())?;
                let group = policy_group_from_request(
                    request,
                    &dry_run,
                    &now,
                    existing_group.as_ref(),
                    operator,
                )?;
                let scope_changed = policy_group_scope_changed(existing_group.as_ref(), &group);
                let invalidated_rule_ids =
                    invalidated_policy_rule_ids(existing_group.as_ref(), &group);
                let resolution_reason =
                    policy_change_resolution_reason(existing_group.as_ref(), &group);
                resolve_memory_policy_alerts_for_rules(
                    memory,
                    &invalidated_rule_ids,
                    resolution_reason,
                )
                .await?;
                let mut states = memory.policy_rule_states.write().await;
                if let Some(existing) = existing_group.as_ref() {
                    let existing_rule_ids = existing
                        .rules
                        .iter()
                        .map(|rule| rule.id)
                        .collect::<HashSet<_>>();
                    states.retain(|state| {
                        if !existing_rule_ids.contains(&state.policy_rule_id) {
                            return true;
                        }
                        !scope_changed
                            && group.rules.iter().any(|rule| {
                                rule.id == state.policy_rule_id
                                    && rule.rule_version == state.rule_version
                            })
                    });
                }
                groups.retain(|stored| stored.id != group.id && stored.name != group.name);
                groups.push(group.clone());
                drop(states);
                drop(groups);
                memory.audits.write().await.push(policy_group_audit(
                    "fleet.alert_policy_upserted",
                    &group,
                    operator,
                    now.clone(),
                ));
                group
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_policy_group_identity_upserts_in_tx(&mut tx).await?;
                sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1)::bigint)")
                    .bind("vpsman.alert_policy_evidence_arm")
                    .execute(&mut *tx)
                    .await?;
                let armed_after_evidence_seq: i64 = sqlx::query_scalar(
                    "SELECT COALESCE(max(evidence_seq), 0) FROM alert_policy_evidence",
                )
                .fetch_one(&mut *tx)
                .await?;
                let armed_at: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
                    .fetch_one(&mut *tx)
                    .await?;
                let existing_groups =
                    policy_groups_for_identity_in_tx(&mut tx, request.id, request.name.trim())
                        .await?;
                let existing_group = select_existing_policy_group(
                    &existing_groups,
                    request.id,
                    request.name.trim(),
                )?;
                let mut group = policy_group_from_request(
                    request,
                    &dry_run,
                    &now,
                    existing_group.as_ref(),
                    operator,
                )?;
                let scope_changed = policy_group_scope_changed(existing_group.as_ref(), &group);
                for rule in &mut group.rules {
                    let previous = existing_group
                        .as_ref()
                        .and_then(|existing| existing.rules.iter().find(|old| old.id == rule.id));
                    if scope_changed {
                        if let Some(previous) = previous {
                            if rule.rule_version == previous.rule_version {
                                rule.rule_version = rule.rule_version.saturating_add(1);
                            }
                        }
                    }
                    if scope_changed
                        || previous.is_none_or(|old| old.rule_version != rule.rule_version)
                    {
                        rule.armed_after_evidence_seq = armed_after_evidence_seq;
                        rule.armed_at = armed_at.to_rfc3339();
                    }
                }
                let invalidated_rule_ids =
                    invalidated_policy_rule_ids(existing_group.as_ref(), &group);
                let invalidated_rule_ids = invalidated_rule_ids.into_iter().collect::<Vec<_>>();
                let resolution_reason =
                    policy_change_resolution_reason(existing_group.as_ref(), &group);
                crate::repository_policy_lifecycle::drain_policy_rule_pending_evidence_in_tx(
                    &mut tx,
                    &invalidated_rule_ids,
                )
                .await?;
                sqlx::query(
                    r#"
                    INSERT INTO policy_groups (
                        id, name, enabled, selector_expression, notes, created_by, updated_by
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $6)
                    ON CONFLICT (id) DO UPDATE SET
                        name = EXCLUDED.name,
                        enabled = EXCLUDED.enabled,
                        selector_expression = EXCLUDED.selector_expression,
                        notes = EXCLUDED.notes,
                        updated_by = EXCLUDED.updated_by,
                        updated_at = now()
                    "#,
                )
                .bind(group.id)
                .bind(&group.name)
                .bind(group.enabled)
                .bind(&group.selector_expression)
                .bind(&group.notes)
                .bind(operator.operator.id)
                .execute(&mut *tx)
                .await?;
                resolve_policy_alerts_for_rules_in_tx(
                    &mut tx,
                    &invalidated_rule_ids,
                    resolution_reason,
                )
                .await?;
                let retained_rule_ids = group.rules.iter().map(|rule| rule.id).collect::<Vec<_>>();
                sqlx::query(
                    r#"
                    DELETE FROM policy_rules
                    WHERE group_id = $1
                      AND NOT (id = ANY($2::uuid[]))
                    "#,
                )
                .bind(group.id)
                .bind(&retained_rule_ids)
                .execute(&mut *tx)
                .await?;
                for rule in &group.rules {
                    let result = sqlx::query(
                        r#"
                        INSERT INTO policy_rules (
                            id, group_id, rule_version, sort_order, name, enabled,
                            rule_kind, evidence_source, correlation_mode,
                            traffic_selector, trigger_condition_expression,
                            trigger_meta_condition, resolve_condition_expression,
                            resolve_meta_condition, severity, category,
                            title_template, detail_template, system_seed_key,
                            armed_after_evidence_seq, armed_at
                        )
                        VALUES (
                            $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,
                            $15,$16,$17,$18,$19,$20,$21::timestamptz
                        )
                        ON CONFLICT (id) DO UPDATE SET
                            rule_version = EXCLUDED.rule_version,
                            sort_order = EXCLUDED.sort_order,
                            name = EXCLUDED.name,
                            enabled = EXCLUDED.enabled,
                            rule_kind = EXCLUDED.rule_kind,
                            evidence_source = EXCLUDED.evidence_source,
                            correlation_mode = EXCLUDED.correlation_mode,
                            traffic_selector = EXCLUDED.traffic_selector,
                            trigger_condition_expression = EXCLUDED.trigger_condition_expression,
                            trigger_meta_condition = EXCLUDED.trigger_meta_condition,
                            resolve_condition_expression = EXCLUDED.resolve_condition_expression,
                            resolve_meta_condition = EXCLUDED.resolve_meta_condition,
                            severity = EXCLUDED.severity,
                            category = EXCLUDED.category,
                            title_template = EXCLUDED.title_template,
                            detail_template = EXCLUDED.detail_template,
                            system_seed_key = EXCLUDED.system_seed_key,
                            armed_after_evidence_seq = EXCLUDED.armed_after_evidence_seq,
                            armed_at = EXCLUDED.armed_at,
                            updated_at = now()
                        WHERE policy_rules.group_id = EXCLUDED.group_id
                        "#,
                    )
                    .bind(rule.id)
                    .bind(rule.group_id)
                    .bind(rule.rule_version)
                    .bind(rule.sort_order)
                    .bind(&rule.name)
                    .bind(rule.enabled)
                    .bind(policy_rule_kind_storage(rule.rule_kind))
                    .bind(&rule.evidence_source)
                    .bind(policy_correlation_mode_storage(rule.correlation_mode))
                    .bind(&rule.traffic_selector)
                    .bind(&rule.trigger_condition_expression)
                    .bind(rule.trigger_meta_condition.as_ref().map(SqlJson))
                    .bind(&rule.resolve_condition_expression)
                    .bind(rule.resolve_meta_condition.as_ref().map(SqlJson))
                    .bind(&rule.severity)
                    .bind(&rule.category)
                    .bind(&rule.title_template)
                    .bind(&rule.detail_template)
                    .bind(&rule.system_seed_key)
                    .bind(rule.armed_after_evidence_seq)
                    .bind(&rule.armed_at)
                    .execute(&mut *tx)
                    .await?;
                    anyhow::ensure!(
                        result.rows_affected() == 1,
                        "fleet_alert_policy_rule_id_conflict:{}",
                        rule.id
                    );
                    sqlx::query(
                        r#"
                        DELETE FROM alert_policy_evaluation_states
                        WHERE policy_rule_id = $1 AND rule_version <> $2
                        "#,
                    )
                    .bind(rule.id)
                    .bind(rule.rule_version)
                    .execute(&mut *tx)
                    .await?;
                    sqlx::query(
                        r#"
                        DELETE FROM alert_policy_confirmations
                        WHERE policy_rule_id = $1 AND rule_version <> $2
                        "#,
                    )
                    .bind(rule.id)
                    .bind(rule.rule_version)
                    .execute(&mut *tx)
                    .await?;
                }
                if scope_changed && !retained_rule_ids.is_empty() {
                    sqlx::query(
                        "DELETE FROM alert_policy_evaluation_states WHERE policy_rule_id = ANY($1::uuid[])",
                    )
                    .bind(&retained_rule_ids)
                    .execute(&mut *tx)
                    .await?;
                    sqlx::query(
                        "DELETE FROM alert_policy_confirmations WHERE policy_rule_id = ANY($1::uuid[])",
                    )
                    .bind(&retained_rule_ids)
                    .execute(&mut *tx)
                    .await?;
                }
                crate::repository_policy_lifecycle::evaluate_policy_rule_baselines_in_tx(
                    &mut tx,
                    &retained_rule_ids,
                )
                .await?;
                group = policy_groups_for_identity_in_tx(&mut tx, Some(group.id), &group.name)
                    .await?
                    .into_iter()
                    .find(|persisted| persisted.id == group.id)
                    .context("fleet_alert_policy_not_found_after_upsert")?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("fleet.alert_policy_upserted")
                .bind(format!("fleet_alert_policy:{}", group.id))
                .bind(policy_group_metadata(&group, operator))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                group
            }
        };
        let policy_id = group.id;
        if let Self::Postgres(pool) = self {
            if let Err(error) =
                crate::repository_policy_lifecycle::evaluate_due_policy_transitions(pool, 200).await
            {
                tracing::warn!(
                    %error,
                    %policy_id,
                    "deferred policy evaluation after policy update"
                );
            }
        } else if let Err(error) = self.evaluate_policy_rules().await {
            tracing::warn!(
                %error,
                %policy_id,
                "deferred policy evaluation after policy update"
            );
        }
        let mut group = group;
        if let Err(error) = self
            .enrich_policy_group_summaries(std::slice::from_mut(&mut group))
            .await
        {
            tracing::warn!(
                %error,
                %policy_id,
                "policy summary enrichment after policy update"
            );
        }
        Ok(group)
    }

    pub(crate) async fn delete_fleet_alert_policy(
        &self,
        policy_id: Uuid,
        reviewed_name: &str,
        operator: &AuthContext,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let mut groups = memory.policy_groups.write().await;
                let policy = groups
                    .iter()
                    .find(|policy| policy.id == policy_id)
                    .cloned()
                    .context("fleet_alert_policy_not_found")?;
                anyhow::ensure!(
                    policy.name == reviewed_name.trim(),
                    "fleet_alert_policy_delete_review_stale"
                );
                let rule_ids = policy
                    .rules
                    .iter()
                    .map(|rule| rule.id)
                    .collect::<HashSet<_>>();
                resolve_memory_policy_alerts_for_rules(memory, &rule_ids, "policy_deleted").await?;
                groups.retain(|stored| stored.id != policy_id);
                memory.policy_rule_states.write().await.retain(|state| {
                    !policy
                        .rules
                        .iter()
                        .any(|rule| rule.id == state.policy_rule_id)
                });
                memory.audits.write().await.push(policy_group_audit(
                    "fleet.alert_policy_deleted",
                    &policy,
                    operator,
                    unix_now().to_string(),
                ));
                drop(groups);
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1)::bigint)")
                    .bind("vpsman.alert_policy_evidence_arm")
                    .execute(&mut *tx)
                    .await?;
                let row = sqlx::query(
                    r#"
                    SELECT
                        id,
                        name,
                        enabled,
                        selector_expression,
                        notes,
                        created_by,
                        updated_by,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM policy_groups
                    WHERE id = $1
                    FOR UPDATE
                    "#,
                )
                .bind(policy_id)
                .fetch_optional(&mut *tx)
                .await?
                .context("fleet_alert_policy_not_found")?;
                let current_name: String = row.try_get("name")?;
                anyhow::ensure!(
                    current_name == reviewed_name.trim(),
                    "fleet_alert_policy_delete_review_stale"
                );
                let rules = sqlx::query(
                    r#"
                    SELECT
                        id,
                        group_id,
                        rule_version,
                        sort_order,
                        name,
                        enabled,
                        rule_kind,
                        evidence_source,
                        correlation_mode,
                        traffic_selector,
                        trigger_condition_expression,
                        trigger_meta_condition,
                        resolve_condition_expression,
                        resolve_meta_condition,
                        severity,
                        category,
                        title_template,
                        detail_template,
                        system_seed_key,
                        armed_after_evidence_seq,
                        armed_at::text AS armed_at,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM policy_rules
                    WHERE group_id = $1
                    ORDER BY sort_order ASC, created_at ASC
                    "#,
                )
                .bind(policy_id)
                .fetch_all(&mut *tx)
                .await?
                .into_iter()
                .map(policy_rule_from_row)
                .collect::<Result<Vec<_>>>()?;
                let policy = policy_group_from_row(row, rules)?;
                let rule_ids = policy.rules.iter().map(|rule| rule.id).collect::<Vec<_>>();
                crate::repository_policy_lifecycle::drain_policy_rule_pending_evidence_in_tx(
                    &mut tx, &rule_ids,
                )
                .await?;
                resolve_policy_alerts_for_rules_in_tx(&mut tx, &rule_ids, "policy_deleted").await?;
                let deleted = sqlx::query("DELETE FROM policy_groups WHERE id = $1")
                    .bind(policy_id)
                    .execute(&mut *tx)
                    .await?;
                anyhow::ensure!(deleted.rows_affected() == 1, "fleet_alert_policy_not_found");
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("fleet.alert_policy_deleted")
                .bind(format!("fleet_alert_policy:{}", policy.id))
                .bind(policy_group_metadata(&policy, operator))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn list_policy_alerts(
        &self,
        query: &PolicyAlertQuery,
    ) -> Result<Vec<PolicyAlertRecord>> {
        self.list_policy_alerts_matching(
            query,
            Some(query.limit.unwrap_or(200).clamp(1, 1000) as usize),
            false,
            PolicyAlertSelectionMode::History,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
    }

    pub(crate) async fn list_policy_alert_candidates(
        &self,
        query: &PolicyAlertQuery,
        limit: usize,
        allowed_client_ids: Option<&HashSet<String>>,
        start_unix: Option<u64>,
        end_unix: Option<u64>,
    ) -> Result<Vec<PolicyAlertRecord>> {
        // Fleet alerts expose at most 200 rows. The snapshot loader may request
        // one sentinel row so it can report an exact truncation boundary
        // without a separate COUNT query.
        self.list_policy_alerts_matching(
            query,
            Some(limit.clamp(1, MAX_POLICY_ALERT_CANDIDATE_ROWS)),
            true,
            PolicyAlertSelectionMode::CurrentFleet,
            allowed_client_ids,
            start_unix,
            end_unix,
            None,
            None,
            None,
        )
        .await
    }

    async fn list_policy_alerts_matching(
        &self,
        query: &PolicyAlertQuery,
        result_limit: Option<usize>,
        prioritize_severity: bool,
        selection_mode: PolicyAlertSelectionMode,
        allowed_client_ids: Option<&HashSet<String>>,
        start_unix: Option<u64>,
        end_unix: Option<u64>,
        operator_state: Option<&str>,
        include_muted: Option<bool>,
        notification_rules: Option<&[FleetAlertNotificationMatchRule]>,
    ) -> Result<Vec<PolicyAlertRecord>> {
        if matches!(self, Self::Memory(_)) {
            return Ok(Vec::new());
        }
        let allowed_client_id_values =
            allowed_client_ids.map(|client_ids| client_ids.iter().cloned().collect::<Vec<_>>());
        if let Self::Postgres(pool) = self {
            return list_unified_policy_alerts_postgres(
                pool,
                query,
                result_limit,
                prioritize_severity,
                selection_mode,
                allowed_client_id_values.as_deref(),
                start_unix,
                end_unix,
                operator_state,
                include_muted.unwrap_or(true),
                notification_rules,
            )
            .await;
        }
        let mut alerts: Vec<PolicyAlertRecord> = match self {
            Self::Memory(memory) => {
                let hidden = memory.hidden_clients.read().await;
                let states = memory.fleet_alert_states.read().await;
                let now = crate::unix_now() as i64;
                memory
                    .policy_alerts
                    .read()
                    .await
                    .iter()
                    .filter(|alert| {
                        selection_mode == PolicyAlertSelectionMode::History
                            || !hidden.contains(&alert.client_id)
                    })
                    .filter(|alert| policy_alert_matches_selection(alert, selection_mode))
                    .filter(|alert| {
                        let public_id = format!("policy-alert:{}", alert.id);
                        let state = states.iter().find(|state| state.alert_id == public_id);
                        let effective = match state {
                            Some(state)
                                if state.state == "muted"
                                    && state.muted_until_unix.is_some_and(|until| until <= now) =>
                            {
                                "open"
                            }
                            Some(state) => state.state.as_str(),
                            None => "open",
                        };
                        (include_muted.unwrap_or(true) || effective != "muted")
                            && operator_state.is_none_or(|expected| effective == expected)
                            && notification_rules.is_none_or(|rules| {
                                notification_rule_matches_alert(
                                    rules,
                                    &alert.severity,
                                    &alert.category,
                                    effective,
                                    Some(&alert.client_id),
                                )
                            })
                    })
                    .cloned()
                    .collect()
            }
            Self::Postgres(_) => unreachable!("Postgres alerts use the unified lifecycle query"),
        };
        alerts.retain(|alert| {
            query
                .client_id
                .as_deref()
                .is_none_or(|client_id| alert.client_id == client_id)
                && query
                    .severity
                    .as_deref()
                    .is_none_or(|severity| alert.severity == severity)
                && query
                    .category
                    .as_deref()
                    .is_none_or(|category| alert.category == category)
                && query
                    .policy_group_id
                    .is_none_or(|policy_group_id| alert.policy_group_id == policy_group_id)
                && allowed_client_ids.is_none_or(|client_ids| client_ids.contains(&alert.client_id))
                && timestamp_in_optional_bounds(
                    if selection_mode == PolicyAlertSelectionMode::History {
                        &alert.created_at
                    } else {
                        &alert.observed_at
                    },
                    start_unix,
                    end_unix,
                )
                && policy_alert_matches_selection(alert, selection_mode)
        });
        alerts.sort_by(|left, right| {
            if prioritize_severity {
                policy_alert_lifecycle_rank(&left.lifecycle_state)
                    .cmp(&policy_alert_lifecycle_rank(&right.lifecycle_state))
                    .then_with(|| {
                        policy_alert_severity_rank(&left.severity)
                            .cmp(&policy_alert_severity_rank(&right.severity))
                    })
                    .then_with(|| compare_timestamps_desc(&left.observed_at, &right.observed_at))
                    .then_with(|| right.id.cmp(&left.id))
            } else {
                compare_timestamps_desc(&left.created_at, &right.created_at)
                    .then_with(|| right.id.cmp(&left.id))
            }
        });
        if let Some(result_limit) = result_limit {
            alerts.truncate(result_limit);
        }
        Ok(alerts)
    }

    pub(crate) async fn evaluate_policy_rules(&self) -> Result<usize> {
        if let Self::Postgres(pool) = self {
            crate::repository_policy_lifecycle::repair_missing_policy_evidence_receipts(pool, 500)
                .await?;
            return crate::repository_policy_lifecycle::evaluate_due_policy_transitions(pool, 200)
                .await;
        }
        // The Memory repository has no durable evidence/receipt/outbox owner.
        // It is retained only for non-lifecycle unit fixtures and must never
        // run the retired in-process alert state machine.
        return Ok(0);

        #[allow(unreachable_code)]
        // Configuration writes and the periodic evaluator can otherwise repeat
        // the same expensive fleet snapshot concurrently in one API process.
        let _evaluation_guard = POLICY_EVALUATION_LOCK.lock().await;
        let groups = self
            .list_fleet_alert_policy_definitions(None, Some(true))
            .await?;
        if groups.is_empty() {
            return Ok(0);
        }
        let agents = self.list_agents().await?;
        let now = Utc::now();
        let rules = self
            .list_vps_rules_matching(
                &VpsRuleQuery {
                    limit: None,
                    client_id: None,
                    selector_expression: None,
                    key: None,
                    state: None,
                },
                None,
            )
            .await?;
        let evaluated_vps_rule_inputs = policy_vps_rule_inputs_by_client(&rules);
        let cycle_starts = traffic_cycle_starts_for_clients(
            agents.iter().map(|agent| agent.id.as_str()),
            &rules,
            now,
        );
        let rule_contexts = vps_rule_contexts_by_client(&rules);
        let mut matched_groups = Vec::with_capacity(groups.len());
        let mut selector_failures = Vec::new();
        for group in groups {
            match resolve_agents_with_rule_contexts(
                &agents,
                &group.selector_expression,
                &rule_contexts,
            ) {
                Ok(matched) => matched_groups.push((group, matched)),
                Err(error) => selector_failures.push((group.id, group.name, error.to_string())),
            }
        }
        let mut stream_requests = traffic_stream_requests_from_rules(&cycle_starts, &rules);
        for (group, matched) in &matched_groups {
            for rule in group.rules.iter().filter(|rule| rule.enabled) {
                if policy_condition_uses_traffic(&rule.trigger_condition_expression)
                    .unwrap_or(false)
                {
                    if let Some(selector) = rule.traffic_selector.as_deref() {
                        add_traffic_selector_requests(
                            &mut stream_requests,
                            matched.iter().map(|agent| agent.id.as_str()),
                            &cycle_starts,
                            selector,
                        );
                    }
                }
            }
        }
        let stream_requests = stream_requests.into_iter().collect::<Vec<_>>();
        let traffic_usage = self
            .list_traffic_counter_usage_for_streams(&stream_requests, now.timestamp())
            .await?;
        let traffic = traffic_accounting_for_agents(&agents, &rules, &traffic_usage, now);
        let traffic_by_client = traffic
            .iter()
            .map(|record| (record.client_id.clone(), record))
            .collect::<HashMap<_, _>>();
        let rollup_client_ids = matched_groups
            .iter()
            .flat_map(|(_, matched)| matched.iter().map(|agent| agent.id.clone()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let rollups = latest_rollups(
            self.list_latest_telemetry_rollups_for_clients(&rollup_client_ids, None)
                .await?,
        );
        let mut fired = 0_usize;
        for (group, matched) in matched_groups {
            for rule in group.rules.iter().filter(|rule| rule.enabled) {
                let Self::Memory(memory) = self else {
                    unreachable!("Postgres policy evaluation uses the unified lifecycle engine");
                };
                resolve_memory_policy_states_outside_scope(memory, &group, rule).await?;
                let request = PolicyRuleRequest {
                    id: Some(rule.id),
                    name: rule.name.clone(),
                    enabled: rule.enabled,
                    rule_kind: rule.rule_kind,
                    evidence_source: rule.evidence_source.clone(),
                    correlation_mode: rule.correlation_mode,
                    traffic_selector: rule.traffic_selector.clone(),
                    trigger_condition_expression: rule.trigger_condition_expression.clone(),
                    trigger_meta_condition: rule.trigger_meta_condition.clone(),
                    resolve_condition_expression: rule.resolve_condition_expression.clone(),
                    resolve_meta_condition: rule.resolve_meta_condition.clone(),
                    severity: rule.severity.clone(),
                    category: rule.category.clone(),
                    title_template: rule.title_template.clone(),
                    detail_template: rule.detail_template.clone(),
                };
                for agent in &matched {
                    let override_traffic =
                        traffic_override_for_rule(&agent.id, &request, &rules, &traffic_usage, now);
                    let traffic_record = override_traffic
                        .as_ref()
                        .or_else(|| traffic_by_client.get(&agent.id).copied());
                    let evaluation =
                        evaluate_rule_for_client(&request, traffic_record, rollups.get(&agent.id));
                    let evaluated_client_vps_rule_inputs = evaluated_vps_rule_inputs
                        .get(&agent.id)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]);
                    if self
                        .persist_policy_evaluation(
                            &group,
                            rule,
                            agent,
                            evaluated_client_vps_rule_inputs,
                            evaluation,
                            now,
                        )
                        .await?
                    {
                        fired += 1;
                    }
                }
            }
        }
        if selector_failures.is_empty() {
            Ok(fired)
        } else {
            Err(policy_selector_partial_failure(&selector_failures))
        }
    }

    async fn vps_rule_preview(
        &self,
        operation: &str,
        selector_expression: &str,
        values: &BTreeMap<String, String>,
        keys: &[String],
    ) -> Result<VpsRulesDryRunResponse> {
        let agents = self.list_agents().await?;
        let stored = self
            .list_vps_rules_matching(
                &VpsRuleQuery {
                    limit: None,
                    client_id: None,
                    selector_expression: None,
                    key: None,
                    state: None,
                },
                None,
            )
            .await?;
        build_vps_rule_preview(
            operation,
            selector_expression,
            values,
            keys,
            &agents,
            &stored,
        )
    }

    async fn list_traffic_counter_usage_for_streams(
        &self,
        requests: &[TrafficStreamRequest],
        now_unix: i64,
    ) -> Result<Vec<TrafficCounterStreamUsage>> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        match self {
            Self::Memory(memory) => Ok(aggregate_memory_traffic_counter_usage(
                &memory.traffic_counter_samples.read().await,
                &memory.traffic_counter_rollups.read().await,
                requests,
                now_unix,
            )),
            Self::Postgres(pool) => {
                let (no_reset_requests, monthly_requests): (Vec<_>, Vec<_>) = requests
                    .iter()
                    .partition(|request| request.cycle_start_unix == NO_RESET_TRAFFIC_START_UNIX);
                let mut usages = Vec::with_capacity(requests.len());

                if !monthly_requests.is_empty() {
                    let client_ids = monthly_requests
                        .iter()
                        .map(|request| request.client_id.clone())
                        .collect::<Vec<_>>();
                    let source_kinds = monthly_requests
                        .iter()
                        .map(|request| request.source_kind.clone())
                        .collect::<Vec<_>>();
                    let interfaces = monthly_requests
                        .iter()
                        .map(|request| request.interface.clone())
                        .collect::<Vec<_>>();
                    let cycle_start_values = monthly_requests
                        .iter()
                        .map(|request| request.cycle_start_unix)
                        .collect::<Vec<_>>();
                    let rows = sqlx::query(MONTHLY_TRAFFIC_COUNTER_USAGE_SQL)
                        .bind(client_ids)
                        .bind(source_kinds)
                        .bind(interfaces)
                        .bind(cycle_start_values)
                        .bind(now_unix)
                        .fetch_all(pool)
                        .await?;
                    usages.extend(
                        rows.into_iter()
                            .map(traffic_counter_stream_usage_from_row)
                            .collect::<Result<Vec<_>>>()?,
                    );
                }

                if !no_reset_requests.is_empty() {
                    let client_ids = no_reset_requests
                        .iter()
                        .map(|request| request.client_id.clone())
                        .collect::<Vec<_>>();
                    let source_kinds = no_reset_requests
                        .iter()
                        .map(|request| request.source_kind.clone())
                        .collect::<Vec<_>>();
                    let interfaces = no_reset_requests
                        .iter()
                        .map(|request| request.interface.clone())
                        .collect::<Vec<_>>();
                    // This statement is deliberately plan-sensitive: the request arrays are
                    // small for a detail read but can contain the whole fleet for a snapshot.
                    // A cached generic plan estimates UNNEST at ten rows and can choose a
                    // multi-million-row nested-loop/raw scan.  JIT compilation is also much
                    // more expensive than the one-shot read on the API path.  Keep both
                    // mitigations transaction-local and use an unnamed statement so callers
                    // cannot inherit either setting and small/fleet requests remain
                    // cardinality-aware.
                    let mut tx = pool.begin().await?;
                    sqlx::query(
                        r#"SELECT set_config('jit', 'off', true),
                                  set_config('plan_cache_mode', 'force_custom_plan', true)"#,
                    )
                    .execute(&mut *tx)
                    .await?;
                    let rows = sqlx::query(NO_RESET_TRAFFIC_COUNTER_USAGE_SQL)
                        .persistent(false)
                        .bind(client_ids)
                        .bind(source_kinds)
                        .bind(interfaces)
                        .bind(now_unix)
                        .fetch_all(&mut *tx)
                        .await?;
                    tx.commit().await?;
                    usages.extend(
                        rows.into_iter()
                            .map(traffic_counter_stream_usage_from_row)
                            .collect::<Result<Vec<_>>>()?,
                    );
                }

                usages.sort_by(|left, right| {
                    left.client_id
                        .cmp(&right.client_id)
                        .then_with(|| left.source_kind.cmp(&right.source_kind))
                        .then_with(|| left.interface.cmp(&right.interface))
                });
                Ok(usages)
            }
        }
    }

    async fn enrich_policy_group_summaries(&self, groups: &mut [PolicyGroupRecord]) -> Result<()> {
        if groups.is_empty() {
            return Ok(());
        }
        let agents = self.list_agents().await?;
        let expressions = groups
            .iter()
            .map(|group| {
                parse_selector_expression(&group.selector_expression)
                    .map_err(|error| anyhow::anyhow!("invalid selector expression: {error}"))?
                    .context("selector expression is empty")
            })
            .collect::<Result<Vec<_>>>()?;
        let rule_contexts = if expressions.iter().any(expression_references_vps_rules) {
            self.vps_rule_contexts_for_agents(&agents).await?
        } else {
            HashMap::new()
        };
        self.enrich_policy_group_summaries_with_rule_contexts(groups, &agents, &rule_contexts)
            .await
    }

    async fn enrich_policy_group_summaries_with_rule_contexts(
        &self,
        groups: &mut [PolicyGroupRecord],
        agents: &[AgentView],
        rule_contexts: &HashMap<String, VpsRuleContext>,
    ) -> Result<()> {
        let rule_ids = groups
            .iter()
            .flat_map(|group| group.rules.iter().map(|rule| rule.id))
            .collect::<Vec<_>>();
        let states = self.policy_rule_states_for_rules(&rule_ids).await?;
        let confirmed_active_alerts = self
            .confirmed_active_policy_alert_keys_for_rules(&rule_ids)
            .await?;
        let matched_groups = groups
            .iter()
            .map(|group| {
                resolve_agents_with_rule_contexts(agents, &group.selector_expression, rule_contexts)
            })
            .collect::<Result<Vec<_>>>()?;
        for (group, matched) in groups.iter_mut().zip(matched_groups) {
            let matched_ids = matched
                .iter()
                .map(|agent| agent.id.as_str())
                .collect::<HashSet<_>>();
            let enabled_rules = group
                .rules
                .iter()
                .filter(|rule| rule.enabled)
                .collect::<Vec<_>>();
            let rule_by_id = group
                .rules
                .iter()
                .map(|rule| (rule.id, rule))
                .collect::<HashMap<_, _>>();
            let mut active_info = 0_i64;
            let mut active_warning = 0_i64;
            let mut active_critical = 0_i64;
            let mut incomplete_clients = BTreeSet::new();
            let mut last_evaluated_at = None::<String>;
            for state in &states {
                if !matched_ids.contains(state.client_id.as_str()) {
                    continue;
                }
                let Some(rule) = rule_by_id.get(&state.policy_rule_id) else {
                    continue;
                };
                if !rule.enabled || rule.rule_version != state.rule_version {
                    continue;
                }
                if state.incomplete {
                    incomplete_clients.insert(state.client_id.clone());
                }
                if state.condition_true
                    && state.window_satisfied
                    && !state.incomplete
                    && confirmed_active_alerts.contains(&(
                        state.policy_rule_id,
                        state.client_id.clone(),
                        state.trigger_generation,
                    ))
                {
                    match rule.severity.as_str() {
                        "critical" => active_critical += 1,
                        "warning" => active_warning += 1,
                        "info" => active_info += 1,
                        _ => {}
                    }
                }
                if last_evaluated_at
                    .as_deref()
                    .is_none_or(|stored| state.last_evaluated_at.as_str() > stored)
                {
                    last_evaluated_at = Some(state.last_evaluated_at.clone());
                }
            }
            group.matched_vps_count = matched.len() as i64;
            group.rule_count = group.rules.len() as i64;
            group.enabled_rule_count = enabled_rules.len() as i64;
            group.active_info_count = active_info;
            group.active_warning_count = active_warning;
            group.active_critical_count = active_critical;
            group.incomplete_vps_count = incomplete_clients.len() as i64;
            group.last_evaluated_at = last_evaluated_at;
        }
        Ok(())
    }

    async fn policy_rule_states_for_rules(
        &self,
        rule_ids: &[Uuid],
    ) -> Result<Vec<PolicyRuleStateRecord>> {
        if rule_ids.is_empty() {
            return Ok(Vec::new());
        }
        match self {
            Self::Memory(memory) => {
                let ids = rule_ids.iter().copied().collect::<HashSet<_>>();
                Ok(memory
                    .policy_rule_states
                    .read()
                    .await
                    .iter()
                    .filter(|state| ids.contains(&state.policy_rule_id))
                    .cloned()
                    .collect())
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        state.policy_rule_id,
                        state.subject_client_id AS client_id,
                        state.rule_version,
                        state.truth_state = 'matched' AS condition_true,
                        FALSE AS previous_condition_true,
                        state.active_episode_id IS NOT NULL AS window_satisfied,
                        state.trigger_segment_started_at::text AS first_true_at,
                        CASE WHEN state.truth_state = 'matched'
                            THEN state.last_evidence_observed_at::text END AS last_true_at,
                        CASE WHEN state.truth_state = 'not_matched'
                            THEN state.last_evidence_observed_at::text END AS last_false_at,
                        state.last_evaluated_at::text AS last_evaluated_at,
                        state.truth_state = 'unknown' AS incomplete,
                        CASE WHEN state.truth_state = 'unknown'
                            THEN ARRAY['evidence_unknown']::text[]
                            ELSE ARRAY[]::text[] END AS incomplete_reasons,
                        NULL::double precision AS last_actual_value,
                        NULL::double precision AS last_threshold_value,
                        episode.last_confirmed_at::text AS last_fired_at,
                        state.trigger_generation,
                        state.updated_at::text AS updated_at
                    FROM alert_policy_evaluation_states state
                    LEFT JOIN alert_episodes episode ON episode.id = state.active_episode_id
                    WHERE state.policy_rule_id = ANY($1)
                      AND state.subject_client_id IS NOT NULL
                    "#,
                )
                .bind(rule_ids)
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(policy_rule_state_from_row).collect()
            }
        }
    }

    async fn confirmed_active_policy_alert_keys_for_rules(
        &self,
        rule_ids: &[Uuid],
    ) -> Result<HashSet<(Uuid, String, i64)>> {
        if rule_ids.is_empty() {
            return Ok(HashSet::new());
        }
        match self {
            Self::Memory(memory) => {
                let rule_ids = rule_ids.iter().copied().collect::<HashSet<_>>();
                Ok(memory
                    .policy_alerts
                    .read()
                    .await
                    .iter()
                    .filter(|alert| {
                        rule_ids.contains(&alert.policy_rule_id)
                            && alert.resolved_at.is_none()
                            && alert.last_confirmed_at.is_some()
                            && matches!(
                                alert.lifecycle_state.as_str(),
                                POLICY_ALERT_TRIGGERED | POLICY_ALERT_PERSISTING
                            )
                    })
                    .map(|alert| {
                        (
                            alert.policy_rule_id,
                            alert.client_id.clone(),
                            alert.trigger_generation,
                        )
                    })
                    .collect())
            }
            Self::Postgres(pool) => Ok(sqlx::query(
                r#"
                SELECT policy_rule_id, client_id, trigger_generation
                FROM alert_episodes
                WHERE policy_rule_id = ANY($1)
                  AND client_id IS NOT NULL
                  AND lifecycle_state IN ('triggered', 'persisting')
                  AND resolved_at IS NULL
                  AND last_confirmed_at IS NOT NULL
                "#,
            )
            .bind(rule_ids)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|row| {
                Ok((
                    row.try_get("policy_rule_id")?,
                    row.try_get("client_id")?,
                    row.try_get("trigger_generation")?,
                ))
            })
            .collect::<Result<HashSet<_>>>()?),
        }
    }

    async fn persist_policy_evaluation(
        &self,
        group: &PolicyGroupRecord,
        rule: &PolicyRuleRecord,
        agent: &AgentView,
        evaluated_vps_rule_inputs: &[PolicyVpsRuleInput],
        evaluation: PolicyEvaluation,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let now_text = now.to_rfc3339();
        let selector = parse_policy_selector(&group.selector_expression)?;
        match self {
            Self::Memory(memory) => {
                let _lifecycle = memory.agent_key_lifecycle.lock().await;
                if require_visible_memory_clients(
                    memory,
                    std::slice::from_ref(&agent.id),
                    "policy_alert_target_unavailable",
                )
                .await
                .is_err()
                {
                    return Ok(false);
                }
                // Keep policy edits ordered before state/alert/event writes and
                // reject an evaluator that loaded an obsolete policy snapshot.
                let groups = memory.policy_groups.read().await;
                if !memory_policy_snapshot_is_current(&groups, group, rule) {
                    return Ok(false);
                }
                let (mut current_agents, current_vps_rules) = matching_memory_policy_agents(
                    memory,
                    std::slice::from_ref(&agent.id),
                    &selector,
                )
                .await?;
                if evaluated_vps_rule_inputs
                    != policy_vps_rule_inputs(&current_vps_rules, &agent.id).as_slice()
                {
                    return Ok(false);
                }
                let Some(current_agent) = current_agents.remove(&agent.id) else {
                    return Ok(false);
                };
                let mut states = memory.policy_rule_states.write().await;
                let mut alerts = memory.policy_alerts.write().await;
                let existing = states
                    .iter()
                    .find(|state| {
                        state.policy_rule_id == rule.id
                            && state.client_id == agent.id
                            && state.rule_version == rule.rule_version
                    })
                    .cloned();
                if policy_state_is_newer_than(existing.as_ref(), now) {
                    return Ok(false);
                }
                let max_generation = alerts
                    .iter()
                    .filter(|alert| alert.policy_rule_id == rule.id && alert.client_id == agent.id)
                    .map(|alert| alert.trigger_generation)
                    .max()
                    .unwrap_or(0);
                let mut state = next_policy_rule_state(
                    rule,
                    &agent.id,
                    &evaluation,
                    existing.as_ref(),
                    max_generation,
                    now,
                )?;
                let eligible = policy_state_is_alert_eligible(&state);
                let alert_index = alerts.iter().position(|alert| {
                    alert.policy_rule_id == state.policy_rule_id
                        && alert.client_id == state.client_id
                        && alert.trigger_generation == state.trigger_generation
                        && alert.resolved_at.is_none()
                });
                let mut inserted = false;
                let mut resolved = false;
                let mut alert = alert_index.map(|index| alerts[index].clone());
                if evaluation.incomplete {
                    if let Some(index) = alert_index {
                        mark_policy_alert_unknown(&mut alerts[index]);
                        alert = Some(alerts[index].clone());
                    }
                } else if !evaluation.condition_true {
                    if let Some(index) =
                        alert_index.filter(|index| alerts[*index].last_confirmed_at.is_some())
                    {
                        let state_last_evaluated_at = parse_policy_lifecycle_timestamp(
                            &state.last_evaluated_at,
                            "policy state last_evaluated_at",
                        )?;
                        resolve_policy_alert(
                            &mut alerts[index],
                            Utc::now(),
                            Some(state_last_evaluated_at),
                            "condition_recovered",
                        )?;
                        resolved = true;
                        alert = Some(alerts[index].clone());
                    }
                } else if eligible {
                    if let Some(index) = alert_index {
                        mark_policy_alert_persisting(&mut alerts[index], &now_text);
                        alert = Some(alerts[index].clone());
                    } else {
                        alert = Some(policy_alert_for_evaluation(
                            group,
                            rule,
                            &current_agent,
                            &state,
                            &evaluation,
                            &now_text,
                        ));
                        inserted = true;
                    }
                }
                let event = if resolved {
                    alert.as_ref().map(policy_alert_resolved_webhook_event)
                } else {
                    alert
                        .as_ref()
                        .filter(|alert| {
                            alert.last_confirmed_at.is_some()
                                && (inserted
                                    || policy_webhook_repair_is_recent(&alert.observed_at, now))
                        })
                        .map(policy_alert_webhook_event)
                };
                let event_occurred_at = if resolved {
                    alert
                        .as_ref()
                        .map(policy_alert_resolution_timestamp)
                        .transpose()?
                        .context("resolved policy alert is missing")?
                } else {
                    now
                };
                let event_row = event
                    .map(|event| webhook_event_row(event, event_occurred_at))
                    .transpose()?;
                let mut events = if event_row.is_some() {
                    Some(memory.webhook_events.write().await)
                } else {
                    None
                };
                if let Some(alert) = alert.as_ref() {
                    state.last_fired_at = Some(alert.observed_at.clone());
                }
                states.retain(|stored| {
                    !(stored.policy_rule_id == state.policy_rule_id
                        && stored.client_id == state.client_id
                        && stored.rule_version == state.rule_version)
                });
                states.push(state.clone());
                if inserted {
                    alerts.push(alert.expect("eligible policy evaluation must build an alert"));
                }
                if let (Some(events), Some(event)) = (events.as_mut(), event_row) {
                    if !events.iter().any(|stored| {
                        stored.kind == event.kind && stored.event_id == event.event_id
                    }) {
                        events.push(event);
                    }
                }
                Ok(inserted)
            }
            Self::Postgres(_) => {
                unreachable!("Postgres policy evaluation uses the unified lifecycle engine")
            }
        }
    }
}

fn validate_confirmed_vps_rule_preview(
    preview: &VpsRulesDryRunResponse,
    expected_preview_hash: &str,
) -> Result<()> {
    anyhow::ensure!(
        preview.preview_hash == expected_preview_hash,
        "vps_rules_preview_hash_mismatch"
    );
    anyhow::ensure!(
        preview.invalid_row_count == 0,
        "vps_rules_preview_contains_invalid_rows"
    );
    Ok(())
}

async fn lock_postgres_vps_rule_mutations(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('vpsman.vps_rule_mutation'))")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn postgres_vps_rule_snapshot_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(Vec<AgentView>, Vec<VpsRuleValueRecord>)> {
    let agent_rows = sqlx::query(
        r#"
        SELECT
            c.id,
            c.display_name,
            c.status,
            host(c.registration_ip) AS registration_ip,
            host(c.last_ip) AS last_ip,
            c.last_seen_at::text AS last_seen_at,
            c.arch,
            c.internal_build_number,
            c.process_incarnation_id,
            c.stale_since::text AS stale_since,
            c.stale_reason,
            c.capabilities,
            COALESCE(
                array_remove(
                    array_agg(t.name ORDER BY t.display_order, t.created_at, t.name),
                    NULL
                ),
                ARRAY[]::TEXT[]
            ) AS tags
        FROM visible_clients c
        LEFT JOIN client_tags ct ON ct.client_id = c.id
        LEFT JOIN tags t ON t.id = ct.tag_id
        GROUP BY
            c.id,
            c.display_name,
            c.status,
            c.registration_ip,
            c.last_ip,
            c.last_seen_at,
            c.arch,
            c.internal_build_number,
            c.process_incarnation_id,
            c.stale_since,
            c.stale_reason,
            c.capabilities
        ORDER BY c.display_name, c.id
        "#,
    )
    .fetch_all(&mut **tx)
    .await?;
    let agents = agent_rows
        .into_iter()
        .map(|row| {
            Ok(AgentView {
                id: row.try_get("id")?,
                display_name: row.try_get("display_name")?,
                status: row.try_get("status")?,
                tags: row.try_get("tags")?,
                registration_ip: row.try_get("registration_ip")?,
                last_ip: row.try_get("last_ip")?,
                last_seen_at: row.try_get("last_seen_at")?,
                arch: row.try_get("arch")?,
                internal_build_number: row.try_get::<i64, _>("internal_build_number")?.max(1)
                    as u64,
                process_incarnation_id: row.try_get("process_incarnation_id")?,
                stale_since: row.try_get("stale_since")?,
                stale_reason: row.try_get("stale_reason")?,
                capabilities: row
                    .try_get::<SqlJson<vpsman_common::AgentCapabilitySnapshot>, _>("capabilities")?
                    .0,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let rule_rows = sqlx::query(
        r#"
        SELECT
            rule.client_id,
            rule.key,
            rule.value_raw,
            rule.value_json,
            rule.source_kind,
            rule.source_id,
            rule.updated_by,
            rule.updated_at::text AS updated_at
        FROM vps_rule_values rule
        JOIN visible_clients client ON client.id = rule.client_id
        ORDER BY rule.client_id, rule.key
        "#,
    )
    .fetch_all(&mut **tx)
    .await?;
    let rules = rule_rows
        .into_iter()
        .map(vps_rule_from_row)
        .collect::<Result<Vec<_>>>()?;
    Ok((agents, rules))
}

fn build_vps_rule_preview(
    operation: &str,
    selector_expression: &str,
    values: &BTreeMap<String, String>,
    keys: &[String],
    agents: &[AgentView],
    stored: &[VpsRuleValueRecord],
) -> Result<VpsRulesDryRunResponse> {
    let rule_contexts = vps_rule_contexts_by_client(stored);
    let matched = resolve_agents_with_rule_contexts(agents, selector_expression, &rule_contexts)?;
    let stored_map = stored
        .iter()
        .map(|row| {
            (
                (row.client_id.clone(), row.key.clone()),
                (
                    row.value_raw.clone(),
                    row.stored_value_raw
                        .clone()
                        .unwrap_or_else(|| row.value_raw.clone()),
                ),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut changes = Vec::new();
    for agent in &matched {
        let billing_touched = if operation == "upsert" {
            values.keys().any(|key| is_billing_rule_key(key))
        } else {
            keys.iter().any(|key| is_billing_rule_key(key))
        };
        if operation == "upsert" {
            for (key, value) in values {
                let parsed = parse_vps_rule_value(key, value);
                let stored_before = stored_map.get(&(agent.id.clone(), key.clone()));
                let before = stored_before.map(|(_, physical)| physical.clone());
                let validation_errors = parsed
                    .as_ref()
                    .err()
                    .map(|error| vec![error.to_string()])
                    .unwrap_or_default();
                let canonical_after = parsed
                    .as_ref()
                    .map(|parsed| parsed.raw.clone())
                    .unwrap_or_else(|_| value.trim().to_string());
                let action = if !validation_errors.is_empty() {
                    "invalid"
                } else if stored_before.is_some_and(|(_, physical)| physical == &canonical_after) {
                    "unchanged"
                } else {
                    "set"
                };
                changes.push(VpsRuleChangePreview {
                    client_id: agent.id.clone(),
                    display_name: agent.display_name.clone(),
                    key: key.clone(),
                    before,
                    after: Some(canonical_after),
                    action: action.to_string(),
                    validation: if validation_errors.is_empty() {
                        "ok".to_string()
                    } else {
                        "invalid".to_string()
                    },
                    validation_errors,
                });
            }
        } else {
            for key in keys {
                let before = stored_map
                    .get(&(agent.id.clone(), key.clone()))
                    .map(|(_, physical)| physical.clone());
                changes.push(VpsRuleChangePreview {
                    client_id: agent.id.clone(),
                    display_name: agent.display_name.clone(),
                    key: key.clone(),
                    before: before.clone(),
                    after: None,
                    action: if before.is_some() {
                        "unset"
                    } else {
                        "unchanged"
                    }
                    .to_string(),
                    validation: "ok".to_string(),
                    validation_errors: Vec::new(),
                });
            }
        }
        if billing_touched {
            let mut price = stored_map
                .get(&(agent.id.clone(), VPS_RULE_KEY_BILLING_PRICE.to_string()))
                .map(|(canonical, _)| canonical.clone());
            let mut cycle = stored_map
                .get(&(agent.id.clone(), VPS_RULE_KEY_BILLING_CYCLE.to_string()))
                .map(|(canonical, _)| canonical.clone());
            if operation == "upsert" {
                if let Some(value) = values.get(VPS_RULE_KEY_BILLING_PRICE) {
                    price = parse_vps_rule_value(VPS_RULE_KEY_BILLING_PRICE, value)
                        .ok()
                        .map(|parsed| parsed.raw);
                }
                if let Some(value) = values.get(VPS_RULE_KEY_BILLING_CYCLE) {
                    cycle = parse_vps_rule_value(VPS_RULE_KEY_BILLING_CYCLE, value)
                        .ok()
                        .map(|parsed| parsed.raw);
                }
            } else {
                if keys.iter().any(|key| key == VPS_RULE_KEY_BILLING_PRICE) {
                    price = None;
                }
                if keys.iter().any(|key| key == VPS_RULE_KEY_BILLING_CYCLE) {
                    cycle = None;
                }
            }
            if let Err(error) = validate_billing_rule_group(price.as_deref(), cycle.as_deref()) {
                for change in changes.iter_mut().filter(|change| {
                    change.client_id == agent.id && is_billing_rule_key(&change.key)
                }) {
                    change.action = "invalid".to_string();
                    change.validation = "invalid".to_string();
                    change.validation_errors.push(error.to_string());
                }
            }
        }
    }
    let changed_row_count = changes
        .iter()
        .filter(|change| matches!(change.action.as_str(), "set" | "unset"))
        .count();
    let invalid_row_count = changes
        .iter()
        .filter(|change| change.action == "invalid")
        .count();
    let hash_payload = json!({
        "operation": operation,
        "selector_expression": selector_expression.trim(),
        "changes": changes,
    });
    Ok(VpsRulesDryRunResponse {
        matched_vps_count: matched.len(),
        changed_row_count,
        invalid_row_count,
        preview_hash: preview_hash(&hash_payload),
        changes,
    })
}

async fn apply_vps_rule_changes_memory(
    memory: &crate::repository::MemoryState,
    preview: &VpsRulesDryRunResponse,
    operator: &AuthContext,
) -> Result<()> {
    anyhow::ensure!(
        preview.invalid_row_count == 0,
        "vps_rules_preview_contains_invalid_rows"
    );
    let target_client_ids = preview
        .changes
        .iter()
        .map(|change| change.client_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    require_visible_memory_clients(
        memory,
        &target_client_ids,
        "vps_rules_target_no_longer_available",
    )
    .await?;
    let now = unix_now().to_string();
    let mut rows = memory.vps_rule_values.write().await;
    for change in &preview.changes {
        if change.action == "unchanged" {
            continue;
        }
        rows.retain(|row| !(row.client_id == change.client_id && row.key == change.key));
        if change.action == "set" {
            let raw = change.after.clone().context("vps rule set missing value")?;
            let parsed = parse_vps_rule_value(&change.key, &raw)?;
            rows.push(VpsRuleValueRecord {
                client_id: change.client_id.clone(),
                key: change.key.clone(),
                value_raw: parsed.raw,
                stored_value_raw: None,
                value_json: parsed.json,
                parsed_display: parsed.display,
                state: "ok".to_string(),
                validation_errors: Vec::new(),
                source_kind: "operator".to_string(),
                source_id: None,
                updated_by: Some(operator.operator.id),
                updated_at: now.clone(),
            });
        }
    }
    drop(rows);
    memory.audits.write().await.push(vps_rules_audit(
        "fleet.vps_rules_updated",
        preview,
        operator,
        now,
    ));
    Ok(())
}

async fn apply_vps_rule_changes_postgres_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    preview: &VpsRulesDryRunResponse,
    operator: &AuthContext,
) -> Result<()> {
    anyhow::ensure!(
        preview.invalid_row_count == 0,
        "vps_rules_preview_contains_invalid_rows"
    );
    let target_client_ids = preview
        .changes
        .iter()
        .map(|change| change.client_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    require_visible_postgres_clients_in_tx(
        tx,
        &target_client_ids,
        "vps_rules_target_no_longer_available",
    )
    .await?;
    for change in &preview.changes {
        if change.action == "unchanged" {
            continue;
        }
        if change.action == "unset" {
            sqlx::query("DELETE FROM vps_rule_values WHERE client_id = $1 AND key = $2")
                .bind(&change.client_id)
                .bind(&change.key)
                .execute(&mut **tx)
                .await?;
        } else if change.action == "set" {
            let raw = change.after.clone().context("vps rule set missing value")?;
            let parsed = parse_vps_rule_value(&change.key, &raw)?;
            sqlx::query(
                r#"
                INSERT INTO vps_rule_values (
                    client_id, key, value_raw, value_json, source_kind, source_id, updated_by
                )
                VALUES ($1, $2, $3, $4, 'operator', NULL, $5)
                ON CONFLICT (client_id, key) DO UPDATE SET
                    value_raw = EXCLUDED.value_raw,
                    value_json = EXCLUDED.value_json,
                    source_kind = EXCLUDED.source_kind,
                    source_id = EXCLUDED.source_id,
                    updated_by = EXCLUDED.updated_by,
                    updated_at = now()
                "#,
            )
            .bind(&change.client_id)
            .bind(&change.key)
            .bind(&parsed.raw)
            .bind(SqlJson(parsed.json))
            .bind(operator.operator.id)
            .execute(&mut **tx)
            .await?;
        }
    }
    crate::repository_policy_lifecycle::record_policy_scope_revision_evidence_for_clients_in_tx(
        tx,
        &target_client_ids,
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(operator.operator.id)
    .bind("fleet.vps_rules_updated")
    .bind("vps_rules")
    .bind(&preview.preview_hash)
    .bind(json!({
        "preview_hash": &preview.preview_hash,
        "matched_vps_count": preview.matched_vps_count,
        "changed_row_count": preview.changed_row_count,
        "result": "succeeded",
        "operator_id": operator.operator.id,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "operator_session_id": operator.audit_session_id(),
        "origin_kind": "operator_request",
        "component": "vps-rules-controller",
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn is_billing_rule_key(key: &str) -> bool {
    matches!(key, VPS_RULE_KEY_BILLING_PRICE | VPS_RULE_KEY_BILLING_CYCLE)
}

fn validate_billing_rule_group(price: Option<&str>, cycle: Option<&str>) -> Result<()> {
    let Some(price) = price else {
        anyhow::ensure!(cycle.is_none(), "billing_cycle_requires_price");
        return Ok(());
    };
    let price = parse_vps_rule_value(VPS_RULE_KEY_BILLING_PRICE, price)?;
    if price
        .json
        .get("disabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        anyhow::ensure!(cycle.is_none(), "billing_cycle_disabled_price_invalid");
        return Ok(());
    }
    let Some(cycle) = cycle else {
        return Ok(());
    };
    let cycle = parse_vps_rule_value(VPS_RULE_KEY_BILLING_CYCLE, cycle)?;
    let has_month = cycle
        .json
        .get("month")
        .is_some_and(|month| !month.is_null());
    match price.json.get("period_code").and_then(Value::as_str) {
        Some("m") => anyhow::ensure!(!has_month, "billing_month_cycle_requires_day"),
        Some("q" | "hy" | "y") => {
            anyhow::ensure!(has_month, "billing_long_cycle_requires_month_day")
        }
        _ => anyhow::bail!("billing_plan_period_invalid"),
    }
    Ok(())
}

async fn resolve_memory_policy_alerts_for_rules(
    memory: &MemoryState,
    rule_ids: &HashSet<Uuid>,
    reason: &str,
) -> Result<()> {
    if rule_ids.is_empty() {
        return Ok(());
    }
    let mut state_evaluated_at = HashMap::<(Uuid, String, i64), DateTime<Utc>>::new();
    for state in memory
        .policy_rule_states
        .read()
        .await
        .iter()
        .filter(|state| rule_ids.contains(&state.policy_rule_id))
    {
        let evaluated_at = parse_policy_lifecycle_timestamp(
            &state.last_evaluated_at,
            "policy state last_evaluated_at",
        )?;
        state_evaluated_at
            .entry((
                state.policy_rule_id,
                state.client_id.clone(),
                state.trigger_generation,
            ))
            .and_modify(|stored| *stored = (*stored).max(evaluated_at))
            .or_insert(evaluated_at);
    }
    let mut alerts = memory.policy_alerts.write().await;
    let mut resolved = Vec::new();
    for alert in alerts.iter_mut().filter(|alert| {
        rule_ids.contains(&alert.policy_rule_id)
            && alert.resolved_at.is_none()
            && alert.last_confirmed_at.is_some()
    }) {
        let evaluated_at = state_evaluated_at
            .get(&(
                alert.policy_rule_id,
                alert.client_id.clone(),
                alert.trigger_generation,
            ))
            .copied();
        let resolved_at = resolve_policy_alert(alert, Utc::now(), evaluated_at, reason)?;
        resolved.push((alert.clone(), resolved_at));
    }
    drop(alerts);
    if resolved.is_empty() {
        return Ok(());
    }
    let mut events = memory.webhook_events.write().await;
    for (alert, resolved_at) in resolved {
        let event = webhook_event_row(policy_alert_resolved_webhook_event(&alert), resolved_at)?;
        if !events
            .iter()
            .any(|stored| stored.kind == event.kind && stored.event_id == event.event_id)
        {
            events.push(event);
        }
    }
    Ok(())
}

async fn resolve_policy_alerts_for_rules_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule_ids: &[Uuid],
    reason: &str,
) -> Result<()> {
    crate::repository_policy_lifecycle::resolve_policy_rules_for_definition_change_in_tx(
        tx, rule_ids, reason,
    )
    .await
}

fn memory_policy_snapshot_is_current(
    groups: &[PolicyGroupRecord],
    group: &PolicyGroupRecord,
    rule: &PolicyRuleRecord,
) -> bool {
    groups.iter().any(|stored_group| {
        stored_group.id == group.id
            && stored_group.enabled
            && stored_group.name == group.name
            && stored_group.selector_expression == group.selector_expression
            && stored_group.rules.iter().any(|stored_rule| {
                stored_rule.id == rule.id
                    && stored_rule.group_id == group.id
                    && stored_rule.enabled
                    && stored_rule.rule_version == rule.rule_version
            })
    })
}

async fn matching_memory_policy_agents(
    memory: &MemoryState,
    candidate_client_ids: &[String],
    selector: &Expression,
) -> Result<(HashMap<String, AgentView>, Vec<VpsRuleValueRecord>)> {
    if candidate_client_ids.is_empty() {
        return Ok((HashMap::new(), Vec::new()));
    }
    let candidates = candidate_client_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let hidden = memory.hidden_clients.read().await;
    let agents = memory
        .agents
        .read()
        .await
        .iter()
        .filter(|agent| {
            candidates.contains(agent.id.as_str()) && !hidden.contains(agent.id.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    drop(hidden);
    let rules = memory
        .vps_rule_values
        .read()
        .await
        .iter()
        .filter(|row| candidates.contains(row.client_id.as_str()))
        .cloned()
        .map(canonicalize_vps_rule_record)
        .collect::<Result<Vec<_>>>()?;
    Ok((matching_policy_agents(&agents, &rules, selector), rules))
}

async fn resolve_memory_policy_states_outside_scope(
    memory: &MemoryState,
    group: &PolicyGroupRecord,
    rule: &PolicyRuleRecord,
) -> Result<()> {
    let selector = parse_policy_selector(&group.selector_expression)?;
    let _lifecycle = memory.agent_key_lifecycle.lock().await;
    let groups = memory.policy_groups.read().await;
    if !memory_policy_snapshot_is_current(&groups, group, rule) {
        return Ok(());
    }
    let candidate_client_ids = memory
        .policy_rule_states
        .read()
        .await
        .iter()
        .filter(|state| state.policy_rule_id == rule.id && state.rule_version == rule.rule_version)
        .map(|state| state.client_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if candidate_client_ids.is_empty() {
        return Ok(());
    }
    let matched_client_ids =
        matching_memory_policy_agents(memory, &candidate_client_ids, &selector)
            .await?
            .0
            .into_keys()
            .collect::<HashSet<_>>();
    let outside_client_ids = candidate_client_ids
        .into_iter()
        .filter(|client_id| !matched_client_ids.contains(client_id))
        .collect::<HashSet<_>>();
    if outside_client_ids.is_empty() {
        return Ok(());
    }
    let mut states = memory.policy_rule_states.write().await;
    let outside = states
        .iter()
        .filter(|state| {
            state.policy_rule_id == rule.id
                && state.rule_version == rule.rule_version
                && outside_client_ids.contains(&state.client_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    if outside.is_empty() {
        return Ok(());
    }
    let outside_evaluated_at = outside
        .iter()
        .map(|state| {
            Ok((
                (state.client_id.clone(), state.trigger_generation),
                parse_policy_lifecycle_timestamp(
                    &state.last_evaluated_at,
                    "policy state last_evaluated_at",
                )?,
            ))
        })
        .collect::<Result<HashMap<_, _>>>()?;
    states.retain(|state| {
        state.policy_rule_id != rule.id
            || state.rule_version != rule.rule_version
            || !outside_client_ids.contains(&state.client_id)
    });
    drop(states);

    let mut alerts = memory.policy_alerts.write().await;
    let mut resolved = Vec::new();
    for alert in alerts.iter_mut().filter(|alert| {
        alert.policy_rule_id == rule.id
            && outside_evaluated_at
                .contains_key(&(alert.client_id.clone(), alert.trigger_generation))
            && alert.resolved_at.is_none()
            && alert.last_confirmed_at.is_some()
    }) {
        let evaluated_at = outside_evaluated_at
            .get(&(alert.client_id.clone(), alert.trigger_generation))
            .copied();
        let resolved_at =
            resolve_policy_alert(alert, Utc::now(), evaluated_at, "policy_scope_exited")?;
        resolved.push((alert.clone(), resolved_at));
    }
    drop(alerts);
    let mut events = memory.webhook_events.write().await;
    for (alert, resolved_at) in resolved {
        let event = webhook_event_row(policy_alert_resolved_webhook_event(&alert), resolved_at)?;
        if !events
            .iter()
            .any(|stored| stored.kind == event.kind && stored.event_id == event.event_id)
        {
            events.push(event);
        }
    }
    Ok(())
}

fn mark_policy_alert_persisting(alert: &mut PolicyAlertRecord, now: &str) {
    alert.lifecycle_state = POLICY_ALERT_PERSISTING.to_string();
    alert.last_confirmed_at = Some(now.to_string());
    alert.resolved_at = None;
    alert.resolution_reason = None;
}

fn mark_policy_alert_unknown(alert: &mut PolicyAlertRecord) {
    alert.lifecycle_state = POLICY_ALERT_UNKNOWN.to_string();
    alert.resolved_at = None;
    alert.resolution_reason = None;
}

fn resolve_policy_alert(
    alert: &mut PolicyAlertRecord,
    after_lock: DateTime<Utc>,
    state_last_evaluated_at: Option<DateTime<Utc>>,
    reason: &str,
) -> Result<DateTime<Utc>> {
    let last_confirmed_at = alert
        .last_confirmed_at
        .as_deref()
        .context("confirmed policy alert is missing last_confirmed_at")
        .and_then(|value| {
            parse_policy_lifecycle_timestamp(value, "policy alert last_confirmed_at")
        })?;
    let resolved_at = after_lock
        .max(last_confirmed_at)
        .max(state_last_evaluated_at.unwrap_or(last_confirmed_at));
    alert.lifecycle_state = POLICY_ALERT_RESOLVED.to_string();
    alert.resolved_at = Some(resolved_at.to_rfc3339());
    alert.resolution_reason = Some(reason.to_string());
    Ok(resolved_at)
}

fn parse_policy_lifecycle_timestamp(value: &str, field: &str) -> Result<DateTime<Utc>> {
    parse_timestamp_utc(value).with_context(|| format!("invalid {field}: {value}"))
}

fn policy_alert_resolution_timestamp(alert: &PolicyAlertRecord) -> Result<DateTime<Utc>> {
    alert
        .resolved_at
        .as_deref()
        .context("resolved policy alert is missing resolved_at")
        .and_then(|value| parse_policy_lifecycle_timestamp(value, "policy alert resolved_at"))
}

fn next_policy_rule_state(
    rule: &PolicyRuleRecord,
    client_id: &str,
    evaluation: &PolicyEvaluation,
    existing: Option<&PolicyRuleStateRecord>,
    max_alert_generation: i64,
    now: DateTime<Utc>,
) -> Result<PolicyRuleStateRecord> {
    let now_text = now.to_rfc3339();
    let previous_condition_true = existing.map(|state| state.condition_true).unwrap_or(false);
    let mut first_true_at = existing.and_then(|state| state.first_true_at.clone());
    let mut last_false_at = existing.and_then(|state| state.last_false_at.clone());
    let mut last_true_at = existing.and_then(|state| state.last_true_at.clone());
    let mut trigger_generation = existing
        .map(|state| state.trigger_generation)
        .unwrap_or(0)
        .max(max_alert_generation)
        .max(0);
    if !evaluation.incomplete && evaluation.condition_true && !previous_condition_true {
        first_true_at = Some(now_text.clone());
        trigger_generation = trigger_generation
            .checked_add(1)
            .context("policy_alert_trigger_generation_exhausted")?;
    }
    let pauses_dwell = previous_condition_true
        && !existing.is_some_and(|state| state.window_satisfied)
        && (evaluation.incomplete
            || (existing.is_some_and(|state| state.incomplete) && evaluation.condition_true));
    if pauses_dwell {
        let unknown_elapsed = existing
            .and_then(|state| parse_timestamp_utc(&state.last_evaluated_at))
            .map(|last_evaluated| now.signed_duration_since(last_evaluated))
            .unwrap_or_default();
        if unknown_elapsed > chrono::Duration::zero() {
            first_true_at = first_true_at
                .as_deref()
                .and_then(parse_timestamp_utc)
                .map(|first| (first + unknown_elapsed).to_rfc3339());
        }
    }
    if evaluation.incomplete {
        // Unknown input is not recovery. Preserve the last proven condition
        // and generation, while the adjustment above pauses an unfinished
        // dwell window until valid evidence returns.
    } else if evaluation.condition_true {
        last_true_at = Some(now_text.clone());
    } else {
        first_true_at = None;
        last_false_at = Some(now_text.clone());
    }
    let window_satisfied = if evaluation.incomplete {
        existing
            .map(|state| state.window_satisfied)
            .unwrap_or(false)
    } else if !evaluation.condition_true {
        false
    } else if trigger_sustained_seconds(&rule.trigger_meta_condition).is_none() {
        true
    } else {
        let sustained_seconds = trigger_sustained_seconds(&rule.trigger_meta_condition)
            .expect("checked sustained trigger meta condition");
        first_true_at
            .as_deref()
            .and_then(parse_timestamp_utc)
            .map(|first| {
                now.signed_duration_since(first) >= chrono::Duration::seconds(sustained_seconds)
            })
            .unwrap_or(false)
    };
    Ok(PolicyRuleStateRecord {
        policy_rule_id: rule.id,
        client_id: client_id.to_string(),
        rule_version: rule.rule_version,
        condition_true: if evaluation.incomplete {
            previous_condition_true
        } else {
            evaluation.condition_true
        },
        previous_condition_true,
        window_satisfied,
        first_true_at,
        last_true_at,
        last_false_at,
        last_evaluated_at: now_text.clone(),
        incomplete: evaluation.incomplete,
        incomplete_reasons: evaluation.incomplete_reasons.clone(),
        last_actual_value: evaluation.actual_value,
        last_threshold_value: evaluation.threshold_value,
        last_fired_at: existing.and_then(|state| state.last_fired_at.clone()),
        trigger_generation,
        updated_at: now_text,
    })
}

fn policy_state_is_alert_eligible(state: &PolicyRuleStateRecord) -> bool {
    state.condition_true && state.window_satisfied && !state.incomplete
}

fn policy_state_is_newer_than(
    state: Option<&PolicyRuleStateRecord>,
    evaluation_time: DateTime<Utc>,
) -> bool {
    state
        .and_then(|state| parse_timestamp_utc(&state.last_evaluated_at))
        .is_some_and(|last_evaluated| last_evaluated > evaluation_time)
}

fn policy_webhook_repair_is_recent(observed_at: &str, evaluation_time: DateTime<Utc>) -> bool {
    DateTime::parse_from_rfc3339(observed_at)
        .or_else(|_| DateTime::parse_from_str(observed_at, "%Y-%m-%d %H:%M:%S%.f%#z"))
        .ok()
        .map(|observed_at| {
            let age = evaluation_time.timestamp() - observed_at.timestamp();
            (0..=POLICY_WEBHOOK_REPAIR_WINDOW_SECS).contains(&age)
        })
        .unwrap_or(false)
}

fn policy_alert_for_evaluation(
    group: &PolicyGroupRecord,
    rule: &PolicyRuleRecord,
    agent: &AgentView,
    state: &PolicyRuleStateRecord,
    evaluation: &PolicyEvaluation,
    now_text: &str,
) -> PolicyAlertRecord {
    let alert_id = Uuid::new_v4();
    let title = if evaluation.category == "traffic" {
        "Traffic quota threshold reached"
    } else {
        "Resource policy threshold reached"
    }
    .to_string();
    let detail = format!(
        "{} matched policy condition {}",
        agent.display_name, rule.trigger_condition_expression
    );
    let mut payload = evaluation.payload.clone();
    if let Some(object) = payload.as_object_mut() {
        object.insert(
            "event".to_string(),
            json!({
                "kind": "alert.triggered",
                "id": format!("policy-alert:{alert_id}"),
                "occurred_at": now_text,
            }),
        );
        object.insert(
            "alert".to_string(),
            json!({
                "id": alert_id,
                "public_id": format!("policy-alert:{alert_id}"),
                "record_kind": "condition",
                "producer_kind": "policy_rule",
                "category": evaluation.category,
                "severity": rule.severity,
                "title": title,
                "detail": detail,
                "source_status": "policy_condition_reached",
                "status": "policy_condition_reached",
                "target_kind": "policy_rule",
                "target_id": rule.id,
                "client_id": agent.id,
                "lifecycle_state": POLICY_ALERT_TRIGGERED,
                "trigger_generation": state.trigger_generation,
                "triggered_at": now_text,
                "last_confirmed_at": now_text,
                "resolved_at": null,
                "resolution_reason": null,
                "resolution_note": null,
                "resolution_actor_id": null,
            }),
        );
        object.insert(
            "vps".to_string(),
            json!({
                "id": agent.id,
                "name": agent.display_name,
                "tags": agent.tags,
            }),
        );
        object.insert(
            "policy".to_string(),
            json!({
                "id": group.id,
                "name": group.name,
            }),
        );
        object.insert(
            "policy_rule".to_string(),
            json!({
                "id": rule.id,
                "name": rule.name,
                "rule_version": rule.rule_version,
                "trigger_condition_expression": rule.trigger_condition_expression,
                "traffic_selector": rule.traffic_selector,
                "rule_kind": rule.rule_kind,
                "evidence_source": rule.evidence_source,
                "system_seed_key": rule.system_seed_key,
                "trigger_meta_condition": normalized_meta_condition_payload(
                    rule.trigger_meta_condition.as_ref()
                ),
                "resolve_meta_condition": normalized_meta_condition_payload(
                    rule.resolve_meta_condition.as_ref()
                ),
            }),
        );
    }
    PolicyAlertRecord {
        id: alert_id,
        policy_group_id: group.id,
        policy_rule_id: rule.id,
        client_id: agent.id.clone(),
        trigger_generation: state.trigger_generation,
        severity: rule.severity.clone(),
        category: evaluation.category.clone(),
        title,
        detail,
        actual_value: evaluation.actual_value,
        threshold_value: evaluation.threshold_value,
        payload,
        lifecycle_state: POLICY_ALERT_TRIGGERED.to_string(),
        last_confirmed_at: Some(now_text.to_string()),
        resolved_at: None,
        resolution_reason: None,
        observed_at: now_text.to_string(),
        created_at: now_text.to_string(),
    }
}

fn policy_alert_webhook_event(alert: &PolicyAlertRecord) -> WebhookEventCandidate {
    WebhookEventCandidate {
        kind: "alert.triggered".to_string(),
        event_id: format!("policy-alert:{}", alert.id),
        event_predicates: vec![
            "alert.triggered".to_string(),
            format!("alert.category:{}", alert.category),
            format!("alert.severity:{}", alert.severity),
        ],
        subject_client_ids: vec![alert.client_id.clone()],
        payload: alert.payload.clone(),
        actor_id: None,
    }
}

fn policy_alert_resolved_webhook_event(alert: &PolicyAlertRecord) -> WebhookEventCandidate {
    let mut payload = alert.payload.clone();
    if let Some(object) = payload.as_object_mut() {
        object.insert(
            "event".to_string(),
            json!({
                "kind": "alert.resolved",
                "id": format!("policy-alert:{}:resolved", alert.id),
                "occurred_at": alert.resolved_at,
            }),
        );
        object.insert(
            "alert".to_string(),
            json!({
                "id": alert.id,
                "public_id": format!("policy-alert:{}", alert.id),
                "record_kind": "condition",
                "producer_kind": "policy_rule",
                "category": alert.category,
                "severity": alert.severity,
                "title": alert.title,
                "detail": alert.detail,
                "source_status": "policy_condition_resolved",
                "status": "policy_condition_resolved",
                "target_kind": "policy_rule",
                "target_id": alert.policy_rule_id,
                "client_id": alert.client_id,
                "lifecycle_state": POLICY_ALERT_RESOLVED,
                "trigger_generation": alert.trigger_generation,
                "triggered_at": alert.created_at,
                "last_confirmed_at": alert.last_confirmed_at,
                "resolved_at": alert.resolved_at,
                "resolution_reason": alert.resolution_reason,
                "resolution_note": null,
                "resolution_actor_id": null,
            }),
        );
    }
    WebhookEventCandidate {
        kind: "alert.resolved".to_string(),
        event_id: format!("policy-alert:{}:resolved", alert.id),
        event_predicates: vec![
            "alert.resolved".to_string(),
            format!("alert.category:{}", alert.category),
            format!("alert.severity:{}", alert.severity),
        ],
        subject_client_ids: vec![alert.client_id.clone()],
        payload,
        actor_id: None,
    }
}

fn traffic_cycle_starts_for_clients<'a>(
    client_ids: impl IntoIterator<Item = &'a str>,
    rules: &[VpsRuleValueRecord],
    now: DateTime<Utc>,
) -> Vec<(String, i64)> {
    let reset_days = rules
        .iter()
        .filter(|rule| rule.key == VPS_RULE_KEY_TRAFFIC_RESET_DAY)
        .filter_map(|rule| {
            parse_persisted_vps_rule_value(&rule.key, &rule.value_raw)
                .ok()?
                .json
                .get("day")
                .and_then(Value::as_i64)
                .map(|day| (rule.client_id.as_str(), day as i32))
        })
        .collect::<HashMap<_, _>>();
    client_ids
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|client_id| {
            let reset_day = reset_days.get(client_id).copied().unwrap_or(1);
            (
                client_id.to_string(),
                if reset_day == -1 {
                    NO_RESET_TRAFFIC_START_UNIX
                } else {
                    cycle_bounds(reset_day, now).0.timestamp()
                },
            )
        })
        .collect()
}

/// Builds the traffic portion of a telemetry policy fact from the same
/// PostgreSQL transaction that accepted the telemetry sample. The ingest path
/// already holds the client row and every traffic stream lock, so neither VPS
/// rule context nor a later packet can leak into this snapshot.
pub(crate) async fn postgres_traffic_accounting_snapshot_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    as_of: DateTime<Utc>,
) -> Result<TrafficAccountingRecord> {
    let rule_rows = sqlx::query(
        r#"
        SELECT client_id, key, value_raw, value_json, source_kind, source_id,
               updated_by, updated_at::text AS updated_at
        FROM vps_rule_values
        WHERE client_id=$1
        ORDER BY key
        "#,
    )
    .bind(client_id)
    .fetch_all(&mut **tx)
    .await?;
    let rules = rule_rows
        .into_iter()
        .map(vps_rule_from_row)
        .collect::<Result<Vec<_>>>()?;
    let cycle_starts = traffic_cycle_starts_for_clients([client_id], &rules, as_of);
    let requests = traffic_stream_requests_from_rules(&cycle_starts, &rules)
        .into_iter()
        .collect::<Vec<_>>();
    let usage =
        postgres_traffic_counter_usage_snapshot_in_tx(tx, &requests, as_of.timestamp()).await?;
    Ok(traffic_accounting_for_client(
        client_id, &rules, &usage, as_of,
    ))
}

async fn postgres_traffic_counter_usage_snapshot_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    requests: &[TrafficStreamRequest],
    now_unix: i64,
) -> Result<Vec<TrafficCounterStreamUsage>> {
    if requests.is_empty() {
        return Ok(Vec::new());
    }
    let (no_reset_requests, monthly_requests): (Vec<_>, Vec<_>) = requests
        .iter()
        .partition(|request| request.cycle_start_unix == NO_RESET_TRAFFIC_START_UNIX);
    let mut usages = Vec::with_capacity(requests.len());

    if !monthly_requests.is_empty() {
        let client_ids = monthly_requests
            .iter()
            .map(|request| request.client_id.clone())
            .collect::<Vec<_>>();
        let source_kinds = monthly_requests
            .iter()
            .map(|request| request.source_kind.clone())
            .collect::<Vec<_>>();
        let interfaces = monthly_requests
            .iter()
            .map(|request| request.interface.clone())
            .collect::<Vec<_>>();
        let cycle_start_values = monthly_requests
            .iter()
            .map(|request| request.cycle_start_unix)
            .collect::<Vec<_>>();
        let rows = sqlx::query(MONTHLY_TRAFFIC_COUNTER_USAGE_SQL)
            .bind(client_ids)
            .bind(source_kinds)
            .bind(interfaces)
            .bind(cycle_start_values)
            .bind(now_unix)
            .fetch_all(&mut **tx)
            .await?;
        usages.extend(
            rows.into_iter()
                .map(traffic_counter_stream_usage_from_row)
                .collect::<Result<Vec<_>>>()?,
        );
    }

    if !no_reset_requests.is_empty() {
        let client_ids = no_reset_requests
            .iter()
            .map(|request| request.client_id.clone())
            .collect::<Vec<_>>();
        let source_kinds = no_reset_requests
            .iter()
            .map(|request| request.source_kind.clone())
            .collect::<Vec<_>>();
        let interfaces = no_reset_requests
            .iter()
            .map(|request| request.interface.clone())
            .collect::<Vec<_>>();
        let rows = sqlx::query(NO_RESET_TRAFFIC_COUNTER_USAGE_SQL)
            .bind(client_ids)
            .bind(source_kinds)
            .bind(interfaces)
            .bind(now_unix)
            .fetch_all(&mut **tx)
            .await?;
        usages.extend(
            rows.into_iter()
                .map(traffic_counter_stream_usage_from_row)
                .collect::<Result<Vec<_>>>()?,
        );
    }

    usages.sort_by(|left, right| {
        left.client_id
            .cmp(&right.client_id)
            .then_with(|| left.source_kind.cmp(&right.source_kind))
            .then_with(|| left.interface.cmp(&right.interface))
    });
    Ok(usages)
}

fn traffic_stream_requests_from_rules(
    cycle_starts: &[(String, i64)],
    rules: &[VpsRuleValueRecord],
) -> BTreeSet<TrafficStreamRequest> {
    let cycle_starts = cycle_starts
        .iter()
        .map(|(client_id, cycle_start)| (client_id.as_str(), *cycle_start))
        .collect::<HashMap<_, _>>();
    let mut requests = BTreeSet::new();
    for rule in rules
        .iter()
        .filter(|rule| rule.key == VPS_RULE_KEY_TRAFFIC_SELECTORS)
    {
        let Some(cycle_start_unix) = cycle_starts.get(rule.client_id.as_str()).copied() else {
            continue;
        };
        let Ok(selectors) = traffic_selectors_from_rule(rule) else {
            continue;
        };
        for selector in selectors {
            requests.insert(TrafficStreamRequest {
                client_id: rule.client_id.clone(),
                source_kind: selector.source,
                interface: selector.interface,
                cycle_start_unix,
            });
        }
    }
    requests
}

fn add_traffic_selector_requests<'a>(
    requests: &mut BTreeSet<TrafficStreamRequest>,
    client_ids: impl IntoIterator<Item = &'a str>,
    cycle_starts: &[(String, i64)],
    selector_expression: &str,
) {
    let Ok(selectors) = parse_persisted_traffic_selector_list(selector_expression) else {
        return;
    };
    let cycle_starts = cycle_starts
        .iter()
        .map(|(client_id, cycle_start)| (client_id.as_str(), *cycle_start))
        .collect::<HashMap<_, _>>();
    for client_id in client_ids.into_iter().collect::<BTreeSet<_>>() {
        let Some(cycle_start_unix) = cycle_starts.get(client_id).copied() else {
            continue;
        };
        for selector in &selectors {
            requests.insert(TrafficStreamRequest {
                client_id: client_id.to_string(),
                source_kind: selector.source.clone(),
                interface: selector.interface.clone(),
                cycle_start_unix,
            });
        }
    }
}

#[derive(Clone, Copy)]
struct MemoryCounterEpochEndpoints<'a> {
    first: &'a TrafficCounterSampleRecord,
    last: &'a TrafficCounterSampleRecord,
}

impl<'a> MemoryCounterEpochEndpoints<'a> {
    fn new(sample: &'a TrafficCounterSampleRecord) -> Self {
        Self {
            first: sample,
            last: sample,
        }
    }

    fn observe(&mut self, sample: &'a TrafficCounterSampleRecord) {
        if sample.observed_unix < self.first.observed_unix {
            self.first = sample;
        }
        if sample.observed_unix > self.last.observed_unix {
            self.last = sample;
        }
    }
}

#[derive(Default)]
struct MemoryNoResetStreamAccumulator<'a> {
    latest: Option<&'a TrafficCounterSampleRecord>,
    rx_epochs: BTreeMap<i64, MemoryCounterEpochEndpoints<'a>>,
    tx_epochs: BTreeMap<i64, MemoryCounterEpochEndpoints<'a>>,
}

impl<'a> MemoryNoResetStreamAccumulator<'a> {
    fn observe(&mut self, sample: &'a TrafficCounterSampleRecord) {
        if self
            .latest
            .is_none_or(|latest| sample.observed_unix > latest.observed_unix)
        {
            self.latest = Some(sample);
        }
        self.rx_epochs
            .entry(sample.rx_counter_epoch)
            .and_modify(|endpoints| endpoints.observe(sample))
            .or_insert_with(|| MemoryCounterEpochEndpoints::new(sample));
        self.tx_epochs
            .entry(sample.tx_counter_epoch)
            .and_modify(|endpoints| endpoints.observe(sample))
            .or_insert_with(|| MemoryCounterEpochEndpoints::new(sample));
    }
}

fn memory_no_reset_direction_usage(
    epochs: &BTreeMap<i64, MemoryCounterEpochEndpoints<'_>>,
    rx_direction: bool,
) -> (i64, i64) {
    let mut usage = 0_i64;
    let mut epochs_seen = i64::from(!epochs.is_empty());
    let mut previous_source = None::<&str>;
    for endpoints in epochs.values() {
        let (first_bytes, last_bytes) = if rx_direction {
            (endpoints.first.rx_bytes, endpoints.last.rx_bytes)
        } else {
            (endpoints.first.tx_bytes, endpoints.last.tx_bytes)
        };
        if last_bytes >= first_bytes {
            usage = usage.saturating_add(last_bytes - first_bytes);
        }
        if previous_source.is_some_and(|source| {
            !is_intentional_vnstat_import_boundary(source, &endpoints.first.sample_source)
        }) {
            epochs_seen = epochs_seen.saturating_add(1);
        }
        previous_source = Some(&endpoints.last.sample_source);
    }
    (usage, epochs_seen)
}

fn aggregate_memory_no_reset_traffic_counter_usage(
    samples: &[TrafficCounterSampleRecord],
    rollups: &[TrafficCounterRollupRecord],
    requests: &[TrafficStreamRequest],
    now_unix: i64,
) -> Vec<TrafficCounterStreamUsage> {
    if requests.is_empty() {
        return Vec::new();
    }
    let mut request_indices = HashMap::<(&str, &str, &str), Vec<usize>>::new();
    for (index, request) in requests.iter().enumerate() {
        request_indices
            .entry((
                request.client_id.as_str(),
                request.source_kind.as_str(),
                request.interface.as_str(),
            ))
            .or_default()
            .push(index);
    }
    let mut accumulators = (0..requests.len())
        .map(|_| MemoryNoResetStreamAccumulator::default())
        .collect::<Vec<_>>();
    for sample in samples
        .iter()
        .filter(|sample| sample.observed_unix <= now_unix)
    {
        let Some(indices) = request_indices.get(&(
            sample.client_id.as_str(),
            sample.source_kind.as_str(),
            sample.interface.as_str(),
        )) else {
            continue;
        };
        for index in indices {
            accumulators[*index].observe(sample);
        }
    }

    requests
        .iter()
        .zip(accumulators)
        .filter_map(|(request, accumulator)| {
            let latest = accumulator.latest?;
            let (mut cycle_rx, mut rx_counter_epochs_seen) =
                memory_no_reset_direction_usage(&accumulator.rx_epochs, true);
            let (mut cycle_tx, mut tx_counter_epochs_seen) =
                memory_no_reset_direction_usage(&accumulator.tx_epochs, false);
            for rollup in rollups.iter().filter(|rollup| {
                rollup.client_id == request.client_id
                    && rollup.source_kind == request.source_kind
                    && rollup.interface == request.interface
                    && rollup.bucket_start_unix <= now_unix
            }) {
                cycle_rx = cycle_rx.saturating_add(rollup.rx_bytes);
                cycle_tx = cycle_tx.saturating_add(rollup.tx_bytes);
                rx_counter_epochs_seen =
                    rx_counter_epochs_seen.saturating_add(i64::from(rollup.rx_reset_count));
                tx_counter_epochs_seen =
                    tx_counter_epochs_seen.saturating_add(i64::from(rollup.tx_reset_count));
            }
            Some(TrafficCounterStreamUsage {
                client_id: request.client_id.clone(),
                source_kind: request.source_kind.clone(),
                interface: request.interface.clone(),
                cycle_rx,
                cycle_tx,
                latest_rx: latest.rx_bytes,
                latest_tx: latest.tx_bytes,
                last_sample_unix: latest.observed_unix,
                rx_counter_epochs_seen,
                tx_counter_epochs_seen,
            })
        })
        .collect()
}

fn aggregate_memory_traffic_counter_usage(
    samples: &[TrafficCounterSampleRecord],
    rollups: &[TrafficCounterRollupRecord],
    requests: &[TrafficStreamRequest],
    now_unix: i64,
) -> Vec<TrafficCounterStreamUsage> {
    let no_reset_requests = requests
        .iter()
        .filter(|request| request.cycle_start_unix == NO_RESET_TRAFFIC_START_UNIX)
        .cloned()
        .collect::<Vec<_>>();
    let monthly_requests = requests
        .iter()
        .filter(|request| request.cycle_start_unix != NO_RESET_TRAFFIC_START_UNIX)
        .cloned()
        .collect::<Vec<_>>();
    let mut rows = aggregate_memory_no_reset_traffic_counter_usage(
        samples,
        rollups,
        &no_reset_requests,
        now_unix,
    );
    if monthly_requests.is_empty() {
        rows.sort_by(|left, right| {
            left.client_id
                .cmp(&right.client_id)
                .then_with(|| left.source_kind.cmp(&right.source_kind))
                .then_with(|| left.interface.cmp(&right.interface))
        });
        return rows;
    }

    let mut request_indices = HashMap::<(&str, &str, &str), Vec<usize>>::new();
    for (index, request) in monthly_requests.iter().enumerate() {
        request_indices
            .entry((
                request.client_id.as_str(),
                request.source_kind.as_str(),
                request.interface.as_str(),
            ))
            .or_default()
            .push(index);
    }
    let mut selected_by_request = vec![Vec::new(); monthly_requests.len()];
    let mut baselines = vec![None::<TrafficCounterSampleRecord>; monthly_requests.len()];
    for sample in samples
        .iter()
        .filter(|sample| sample.observed_unix <= now_unix)
    {
        let Some(indices) = request_indices.get(&(
            sample.client_id.as_str(),
            sample.source_kind.as_str(),
            sample.interface.as_str(),
        )) else {
            continue;
        };
        for index in indices {
            if sample.observed_unix >= monthly_requests[*index].cycle_start_unix {
                selected_by_request[*index].push(sample.clone());
            } else if baselines[*index]
                .as_ref()
                .is_none_or(|baseline| sample.observed_unix > baseline.observed_unix)
            {
                baselines[*index] = Some(sample.clone());
            }
        }
    }

    for ((request, mut selected), baseline) in monthly_requests
        .iter()
        .zip(selected_by_request)
        .zip(baselines)
    {
        if let Some(baseline) = baseline {
            selected.push(baseline);
        }
        if selected.is_empty() {
            continue;
        }
        let usage = derive_cycle_usage(&selected, request.cycle_start_unix, now_unix);
        let rx_counter_epochs_seen = unexpected_counter_epochs_seen(&selected, true);
        let tx_counter_epochs_seen = unexpected_counter_epochs_seen(&selected, false);
        rows.push(TrafficCounterStreamUsage {
            client_id: request.client_id.clone(),
            source_kind: request.source_kind.clone(),
            interface: request.interface.clone(),
            cycle_rx: usage.cycle_rx,
            cycle_tx: usage.cycle_tx,
            latest_rx: usage.latest_rx,
            latest_tx: usage.latest_tx,
            last_sample_unix: usage
                .last_sample_unix
                .expect("non-empty selected traffic samples have a latest timestamp"),
            rx_counter_epochs_seen: i64::try_from(rx_counter_epochs_seen).unwrap_or(i64::MAX),
            tx_counter_epochs_seen: i64::try_from(tx_counter_epochs_seen).unwrap_or(i64::MAX),
        });
    }
    rows.sort_by(|left, right| {
        left.client_id
            .cmp(&right.client_id)
            .then_with(|| left.source_kind.cmp(&right.source_kind))
            .then_with(|| left.interface.cmp(&right.interface))
    });
    rows
}

fn unexpected_counter_epochs_seen(
    samples: &[TrafficCounterSampleRecord],
    rx_direction: bool,
) -> usize {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by_key(|sample| sample.observed_unix);
    let mut epochs_seen = 1_usize;
    for pair in sorted.windows(2) {
        let previous = &pair[0];
        let current = &pair[1];
        let epoch_changed = if rx_direction {
            current.rx_counter_epoch != previous.rx_counter_epoch
        } else {
            current.tx_counter_epoch != previous.tx_counter_epoch
        };
        if epoch_changed
            && !is_intentional_vnstat_import_boundary(
                &previous.sample_source,
                &current.sample_source,
            )
        {
            epochs_seen = epochs_seen.saturating_add(1);
        }
    }
    epochs_seen
}

fn raw_memory_traffic_samples(
    samples: &[TelemetrySampleView],
    client_id: &str,
    streams: &[TrafficHistoryStream],
) -> Result<Vec<TrafficCounterSampleRecord>> {
    let selected = streams
        .iter()
        .map(|stream| (stream.source_kind.as_str(), stream.interface.as_str()))
        .collect::<HashSet<_>>();
    let mut rows = Vec::new();
    for sample in samples
        .iter()
        .filter(|sample| sample.client_id == client_id)
    {
        let metrics: vpsman_common::AgentMetrics =
            serde_json::from_value(sample.payload.clone())
                .context("stored raw telemetry payload is invalid")?;
        let observed_unix = i64::try_from(
            parse_timestamp_unix(&sample.observed_at)
                .context("stored raw telemetry receive timestamp is invalid")?,
        )
        .context("stored raw telemetry receive timestamp exceeds supported range")?;
        for network in metrics
            .networks
            .into_iter()
            .filter(|network| selected.contains(&("host", network.interface.as_str())))
        {
            rows.push(TrafficCounterSampleRecord {
                client_id: client_id.to_string(),
                source_kind: "host".to_string(),
                interface: network.interface,
                observed_at: sample.observed_at.clone(),
                observed_unix,
                rx_bytes: i64::try_from(network.rx_bytes).unwrap_or(i64::MAX),
                tx_bytes: i64::try_from(network.tx_bytes).unwrap_or(i64::MAX),
                rx_counter_epoch: 0,
                tx_counter_epoch: 0,
                sample_source: "raw_agent_networks".to_string(),
            });
        }
        for tunnel in metrics
            .tunnels
            .into_iter()
            .filter(|tunnel| selected.contains(&("tunnel", tunnel.interface.as_str())))
        {
            rows.push(TrafficCounterSampleRecord {
                client_id: client_id.to_string(),
                source_kind: "tunnel".to_string(),
                interface: tunnel.interface,
                observed_at: sample.observed_at.clone(),
                observed_unix,
                rx_bytes: i64::try_from(tunnel.rx_bytes).unwrap_or(i64::MAX),
                tx_bytes: i64::try_from(tunnel.tx_bytes).unwrap_or(i64::MAX),
                rx_counter_epoch: 0,
                tx_counter_epoch: 0,
                sample_source: "raw_runtime_tunnel".to_string(),
            });
        }
    }
    Ok(rows)
}

fn aggregate_memory_raw_traffic_history(
    live_samples: Vec<TrafficCounterSampleRecord>,
    exact_samples: &[TrafficCounterSampleRecord],
    streams: &[TrafficHistoryStream],
    start_unix: u64,
    end_unix: u64,
    step_secs: i32,
) -> Vec<TrafficHistoryPointView> {
    // vnStat import is authoritative for the interval before live collection
    // starts. Sequence it independently so the join cannot invent a transition
    // between imported durable counters and partial-minute live observations.
    let imported_samples = exact_samples
        .iter()
        .filter(|sample| is_vnstat_import_source(&sample.sample_source))
        .cloned()
        .collect::<Vec<_>>();
    merge_traffic_history_points(
        aggregate_memory_traffic_history(
            live_samples,
            &[],
            streams,
            start_unix,
            end_unix,
            step_secs,
        ),
        aggregate_memory_traffic_history(
            imported_samples,
            &[],
            streams,
            start_unix,
            end_unix,
            step_secs,
        ),
    )
}

fn merge_traffic_history_points(
    primary: Vec<TrafficHistoryPointView>,
    additional: Vec<TrafficHistoryPointView>,
) -> Vec<TrafficHistoryPointView> {
    let mut buckets = BTreeMap::<(i64, i32), TrafficHistoryPointView>::new();
    for point in primary.into_iter().chain(additional) {
        let bucket_unix = point.bucket_start.parse::<i64>().unwrap_or_default();
        let entry = buckets
            .entry((bucket_unix, point.bucket_secs))
            .or_insert_with(|| TrafficHistoryPointView {
                bucket_start: point.bucket_start.clone(),
                bucket_secs: point.bucket_secs,
                sample_count: 0,
                reset_count: 0,
                rx_bytes: None,
                tx_bytes: None,
                total_bytes: None,
            });
        entry.sample_count = entry.sample_count.saturating_add(point.sample_count);
        entry.reset_count = entry.reset_count.saturating_add(point.reset_count);
        entry.rx_bytes = add_optional_traffic_bytes(entry.rx_bytes, point.rx_bytes);
        entry.tx_bytes = add_optional_traffic_bytes(entry.tx_bytes, point.tx_bytes);
        entry.total_bytes = entry
            .rx_bytes
            .zip(entry.tx_bytes)
            .map(|(rx, tx)| rx.saturating_add(tx));
    }
    buckets.into_values().collect()
}

fn add_optional_traffic_bytes(left: Option<i64>, right: Option<i64>) -> Option<i64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.saturating_add(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn aggregate_memory_traffic_history(
    samples: Vec<TrafficCounterSampleRecord>,
    rollups: &[TrafficCounterRollupRecord],
    streams: &[TrafficHistoryStream],
    start_unix: u64,
    end_unix: u64,
    step_secs: i32,
) -> Vec<TrafficHistoryPointView> {
    #[derive(Default)]
    struct Bucket {
        sample_count: i32,
        reset_count: i32,
        selected_rx: bool,
        selected_tx: bool,
        valid_rx_count: i32,
        valid_tx_count: i32,
        rx_bytes: i64,
        tx_bytes: i64,
    }

    let start_unix = i64::try_from(start_unix).unwrap_or(i64::MAX);
    let end_unix = i64::try_from(end_unix).unwrap_or(i64::MAX);
    let step = i64::from(step_secs.max(60));
    let mut buckets = BTreeMap::<(i64, i64), Bucket>::new();
    for stream in streams {
        let mut selected = samples
            .iter()
            .filter(|sample| {
                sample.source_kind == stream.source_kind
                    && sample.interface == stream.interface
                    && sample.observed_unix <= end_unix
            })
            .cloned()
            .collect::<Vec<_>>();
        selected.sort_by_key(|sample| sample.observed_unix);
        selected.dedup_by_key(|sample| sample.observed_unix);
        let mut previous = None::<TrafficCounterSampleRecord>;
        for sample in selected {
            let Some(prior) = previous.replace(sample.clone()) else {
                continue;
            };
            if sample.observed_unix < start_unix {
                continue;
            }
            let bucket = buckets
                .entry((sample.observed_unix.div_euclid(step) * step, step))
                .or_default();
            let selected_rx = stream.direction_mask & 1 != 0;
            let selected_tx = stream.direction_mask & 2 != 0;
            let valid_rx = sample.rx_counter_epoch == prior.rx_counter_epoch
                && sample.rx_bytes >= prior.rx_bytes;
            let valid_tx = sample.tx_counter_epoch == prior.tx_counter_epoch
                && sample.tx_bytes >= prior.tx_bytes;
            bucket.selected_rx |= selected_rx;
            bucket.selected_tx |= selected_tx;
            let intentional_boundary =
                is_intentional_vnstat_import_boundary(&prior.sample_source, &sample.sample_source);
            if !intentional_boundary && ((selected_rx && !valid_rx) || (selected_tx && !valid_tx)) {
                bucket.reset_count = bucket.reset_count.saturating_add(1);
            }
            if (selected_rx && valid_rx) || (selected_tx && valid_tx) {
                bucket.sample_count = bucket.sample_count.saturating_add(1);
            }
            if selected_rx && valid_rx {
                bucket.valid_rx_count = bucket.valid_rx_count.saturating_add(1);
                bucket.rx_bytes = bucket
                    .rx_bytes
                    .saturating_add(sample.rx_bytes.saturating_sub(prior.rx_bytes));
            }
            if selected_tx && valid_tx {
                bucket.valid_tx_count = bucket.valid_tx_count.saturating_add(1);
                bucket.tx_bytes = bucket
                    .tx_bytes
                    .saturating_add(sample.tx_bytes.saturating_sub(prior.tx_bytes));
            }
        }
    }
    for stream in streams {
        for rollup in rollups.iter().filter(|rollup| {
            rollup.source_kind == stream.source_kind
                && rollup.interface == stream.interface
                && rollup.bucket_start_unix <= end_unix
                && rollup
                    .bucket_start_unix
                    .saturating_add(i64::from(rollup.bucket_secs))
                    > start_unix
        }) {
            let output_step = step.max(i64::from(rollup.bucket_secs));
            let bucket = buckets
                .entry((
                    rollup.bucket_start_unix.div_euclid(output_step) * output_step,
                    output_step,
                ))
                .or_default();
            let selected_rx = stream.direction_mask & 1 != 0;
            let selected_tx = stream.direction_mask & 2 != 0;
            bucket.selected_rx |= selected_rx;
            bucket.selected_tx |= selected_tx;
            let valid_count = match stream.direction_mask {
                0b01 => rollup.rx_valid_count,
                0b10 => rollup.tx_valid_count,
                _ => rollup.any_valid_count,
            };
            let reset_count = match stream.direction_mask {
                0b01 => rollup.rx_reset_count,
                0b10 => rollup.tx_reset_count,
                _ => rollup.any_reset_count,
            };
            bucket.sample_count = bucket.sample_count.saturating_add(valid_count);
            bucket.reset_count = bucket.reset_count.saturating_add(reset_count);
            if selected_rx {
                bucket.valid_rx_count = bucket.valid_rx_count.saturating_add(rollup.rx_valid_count);
                bucket.rx_bytes = bucket.rx_bytes.saturating_add(rollup.rx_bytes);
            }
            if selected_tx {
                bucket.valid_tx_count = bucket.valid_tx_count.saturating_add(rollup.tx_valid_count);
                bucket.tx_bytes = bucket.tx_bytes.saturating_add(rollup.tx_bytes);
            }
        }
    }
    buckets
        .into_iter()
        .map(|((bucket_start, bucket_secs), bucket)| {
            let has_samples = bucket.sample_count > 0;
            let rx_bytes = (has_samples && (!bucket.selected_rx || bucket.valid_rx_count > 0))
                .then_some(bucket.rx_bytes);
            let tx_bytes = (has_samples && (!bucket.selected_tx || bucket.valid_tx_count > 0))
                .then_some(bucket.tx_bytes);
            TrafficHistoryPointView {
                bucket_start: bucket_start.to_string(),
                bucket_secs: i32::try_from(bucket_secs).unwrap_or(i32::MAX),
                sample_count: bucket.sample_count,
                reset_count: bucket.reset_count,
                rx_bytes,
                tx_bytes,
                total_bytes: rx_bytes.zip(tx_bytes).map(|(rx, tx)| rx.saturating_add(tx)),
            }
        })
        .collect()
}

fn validate_vps_rule_values(values: &BTreeMap<String, String>) -> Result<()> {
    for (key, value) in values {
        parse_vps_rule_value(key, value)?;
    }
    Ok(())
}

fn normalize_vps_rule_values(
    values: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    anyhow::ensure!(!values.is_empty(), "vps_rules_values_required");
    let mut normalized = BTreeMap::new();
    for (key, value) in values {
        let key = key.trim().to_string();
        anyhow::ensure!(
            normalized.insert(key, value.clone()).is_none(),
            "vps_rules_duplicate_key"
        );
    }
    Ok(normalized)
}

fn normalize_vps_rule_keys(keys: &[String]) -> Result<Vec<String>> {
    anyhow::ensure!(!keys.is_empty(), "vps_rules_keys_required");
    let mut seen = HashSet::new();
    let mut normalized_keys = Vec::with_capacity(keys.len());
    for key in keys {
        let normalized = normalize_vps_rule_key(key)?;
        anyhow::ensure!(seen.insert(normalized.clone()), "vps_rules_duplicate_key");
        normalized_keys.push(normalized);
    }
    Ok(normalized_keys)
}

fn normalize_vps_rule_key(key: &str) -> Result<String> {
    let key = key.trim();
    anyhow::ensure!(
        vpsman_common::SUPPORTED_VPS_RULE_KEYS.contains(&key),
        "vps_rules_key_unsupported"
    );
    Ok(key.to_string())
}

fn parse_vps_rule_value(key: &str, value: &str) -> Result<ParsedRuleValue> {
    parse_common_vps_rule_value(key, value).map_err(anyhow::Error::msg)
}

fn parse_persisted_vps_rule_value(key: &str, value: &str) -> Result<ParsedRuleValue> {
    parse_common_persisted_vps_rule_value(key, value).map_err(anyhow::Error::msg)
}

#[cfg(test)]
fn parse_network_rate_interfaces(value: &str) -> Result<ParsedRuleValue> {
    parse_vps_rule_value(VPS_RULE_KEY_NETWORK_RATE_INTERFACES, value)
}

#[cfg(test)]
fn parse_port_speed(value: &str) -> Result<ParsedRuleValue> {
    parse_vps_rule_value(vpsman_common::VPS_RULE_KEY_NETWORK_PORT_SPEED, value)
}

#[cfg(test)]
fn parse_billing_price(value: &str) -> Result<ParsedRuleValue> {
    parse_vps_rule_value(VPS_RULE_KEY_BILLING_PRICE, value)
}

#[cfg(test)]
fn parse_billing_cycle(value: &str) -> Result<ParsedRuleValue> {
    parse_vps_rule_value(VPS_RULE_KEY_BILLING_CYCLE, value)
}

fn network_rate_selector_spec_from_rule(
    rule: &VpsRuleValueRecord,
) -> Result<NetworkRateSelectorSpec> {
    anyhow::ensure!(
        rule.key == VPS_RULE_KEY_NETWORK_RATE_INTERFACES,
        "network_rate_selector_storage_invalid"
    );
    let parsed = parse_persisted_vps_rule_value(&rule.key, &rule.value_raw)?;
    let mode = parsed
        .json
        .get("mode")
        .and_then(Value::as_str)
        .context("network_rate_selector_storage_invalid")?;
    match mode {
        "all" => Ok(NetworkRateSelectorSpec::All),
        "exact" => Ok(NetworkRateSelectorSpec::Exact(
            traffic_selectors_from_parsed_rule(&parsed)?,
        )),
        "reference" => {
            anyhow::ensure!(
                parsed
                    .json
                    .get("reference")
                    .and_then(|reference| reference.get("rule"))
                    .and_then(Value::as_str)
                    == Some(VPS_RULE_KEY_TRAFFIC_SELECTORS),
                "network_rate_selector_storage_invalid"
            );
            Ok(NetworkRateSelectorSpec::Reference(
                NetworkRateSelectorReference::TrafficSelectors,
            ))
        }
        _ => anyhow::bail!("network_rate_selector_storage_invalid"),
    }
}

fn traffic_selectors_from_parsed_rule(parsed: &ParsedRuleValue) -> Result<Vec<TrafficSelector>> {
    parsed
        .json
        .get("selectors")
        .and_then(Value::as_array)
        .context("traffic_selector_storage_invalid")?
        .iter()
        .map(|item| {
            Ok(TrafficSelector {
                source: item
                    .get("source")
                    .and_then(Value::as_str)
                    .context("traffic_selector_storage_invalid")?
                    .to_string(),
                interface: item
                    .get("interface")
                    .and_then(Value::as_str)
                    .context("traffic_selector_storage_invalid")?
                    .to_string(),
                direction: item
                    .get("direction")
                    .and_then(Value::as_str)
                    .context("traffic_selector_storage_invalid")?
                    .to_string(),
                canonical: item
                    .get("canonical")
                    .and_then(Value::as_str)
                    .context("traffic_selector_storage_invalid")?
                    .to_string(),
            })
        })
        .collect()
}

fn traffic_selectors_from_rule(rule: &VpsRuleValueRecord) -> Result<Vec<TrafficSelector>> {
    anyhow::ensure!(
        rule.key == VPS_RULE_KEY_TRAFFIC_SELECTORS,
        "traffic_selector_storage_invalid"
    );
    let parsed = parse_persisted_vps_rule_value(&rule.key, &rule.value_raw)?;
    traffic_selectors_from_parsed_rule(&parsed)
}

fn resolve_network_rate_interface_selection(
    client_ids: &[String],
    rules: &[VpsRuleValueRecord],
) -> Result<NetworkRateInterfaceSelection> {
    let rules_by_client = rules.iter().fold(
        HashMap::<&str, HashMap<&str, &VpsRuleValueRecord>>::new(),
        |mut by_client, rule| {
            by_client
                .entry(rule.client_id.as_str())
                .or_default()
                .insert(rule.key.as_str(), rule);
            by_client
        },
    );
    let mut selection = NetworkRateInterfaceSelection::default();
    for client_id in client_ids {
        let client_rules = rules_by_client.get(client_id.as_str());
        let rate_rule = client_rules
            .and_then(|rules| rules.get(VPS_RULE_KEY_NETWORK_RATE_INTERFACES))
            .copied();
        let spec = rate_rule.map_or(
            Ok(NetworkRateSelectorSpec::Reference(
                NetworkRateSelectorReference::TrafficSelectors,
            )),
            network_rate_selector_spec_from_rule,
        )?;
        match spec {
            NetworkRateSelectorSpec::All => selection.select_all(client_id.clone()),
            NetworkRateSelectorSpec::Exact(selectors) => {
                selection.select_exact(client_id.clone(), host_rate_interfaces(&selectors))
            }
            NetworkRateSelectorSpec::Reference(NetworkRateSelectorReference::TrafficSelectors) => {
                let inherited = match client_rules
                    .and_then(|rules| rules.get(VPS_RULE_KEY_TRAFFIC_SELECTORS))
                {
                    Some(rule) => host_rate_interfaces(&traffic_selectors_from_rule(rule)?),
                    None => BTreeSet::new(),
                };
                selection.select_exact(client_id.clone(), inherited);
            }
        }
    }
    Ok(selection)
}

fn host_rate_interfaces(selectors: &[TrafficSelector]) -> BTreeSet<String> {
    let mut selected = BTreeSet::new();
    for selector in selectors
        .iter()
        .filter(|selector| selector.source == "host")
    {
        selected.insert(selector.interface.clone());
    }
    selected
}

fn parse_traffic_selector_list(input: &str) -> Result<Vec<TrafficSelector>> {
    let parsed = parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_SELECTORS, input)?;
    traffic_selectors_from_parsed_rule(&parsed)
}

#[cfg(test)]
fn parse_traffic_selector(input: &str) -> Result<TrafficSelector> {
    let mut selectors = parse_traffic_selector_list(input)?;
    anyhow::ensure!(selectors.len() == 1, "traffic_selector_single_required");
    Ok(selectors.remove(0))
}

fn parse_persisted_traffic_selector_list(input: &str) -> Result<Vec<TrafficSelector>> {
    let parsed = parse_persisted_vps_rule_value(VPS_RULE_KEY_TRAFFIC_SELECTORS, input)?;
    traffic_selectors_from_parsed_rule(&parsed)
}

fn traffic_selector_direction_mask(selector: &TrafficSelector) -> u8 {
    match selector.direction.as_str() {
        "rx" => 0b01,
        "tx" => 0b10,
        _ => 0b11,
    }
}

fn claim_traffic_selector_directions(
    counted: &mut HashMap<(String, String), u8>,
    selector: &TrafficSelector,
) -> (bool, bool) {
    let requested = traffic_selector_direction_mask(selector);
    let counted = counted
        .entry((selector.source.clone(), selector.interface.clone()))
        .or_default();
    let newly_counted = requested & !*counted;
    *counted |= requested;
    (newly_counted & 0b01 != 0, newly_counted & 0b10 != 0)
}

fn traffic_selector_total(selector: &TrafficSelector, rx_bytes: i64, tx_bytes: i64) -> i64 {
    if selector.direction == "tx/rx" {
        rx_bytes.max(tx_bytes)
    } else {
        rx_bytes.saturating_add(tx_bytes)
    }
}

fn parse_byte_size(input: &str) -> Result<i64> {
    let value = input.trim();
    anyhow::ensure!(!value.is_empty(), "byte_size_empty");
    let split_at = value
        .find(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .unwrap_or(value.len());
    let number = value[..split_at]
        .parse::<f64>()
        .map_err(|_| anyhow::anyhow!("byte_size_number_invalid"))?;
    anyhow::ensure!(
        number.is_finite() && number > 0.0,
        "byte_size_number_invalid"
    );
    let suffix = value[split_at..].trim().to_ascii_lowercase();
    let multiplier = match suffix.as_str() {
        "" | "b" => 1_f64,
        "kb" => 1_000_f64,
        "mb" => 1_000_000_f64,
        "gb" => 1_000_000_000_f64,
        "tb" => 1_000_000_000_000_f64,
        "kib" => 1024_f64,
        "mib" => 1024_f64.powi(2),
        "gib" => 1024_f64.powi(3),
        "tib" => 1024_f64.powi(4),
        _ => anyhow::bail!("byte_size_unit_invalid"),
    };
    let bytes = (number * multiplier).round();
    anyhow::ensure!(bytes <= i64::MAX as f64, "byte_size_too_large");
    Ok(bytes as i64)
}

fn resolve_agents_with_rule_contexts(
    agents: &[AgentView],
    selector: &str,
    rules_by_client: &HashMap<String, VpsRuleContext>,
) -> Result<Vec<AgentView>> {
    let expression = parse_policy_selector(selector)?;
    Ok(agents
        .iter()
        .filter(|agent| {
            agent_matches_selector_expression_with_rules(
                agent,
                &expression,
                rules_by_client.get(&agent.id),
            )
        })
        .cloned()
        .collect())
}

fn parse_policy_selector(selector: &str) -> Result<Expression> {
    parse_selector_expression(selector)
        .map_err(|error| anyhow::anyhow!("invalid selector expression: {error}"))?
        .context("selector expression is empty")
}

fn matching_policy_agents(
    agents: &[AgentView],
    rules: &[VpsRuleValueRecord],
    selector: &Expression,
) -> HashMap<String, AgentView> {
    let rules_by_client = vps_rule_contexts_by_client(rules);
    agents
        .iter()
        .filter(|agent| {
            agent_matches_selector_expression_with_rules(
                agent,
                selector,
                rules_by_client.get(&agent.id),
            )
        })
        .cloned()
        .map(|agent| (agent.id.clone(), agent))
        .collect()
}

fn policy_vps_rule_inputs_by_client(
    rules: &[VpsRuleValueRecord],
) -> HashMap<String, Vec<PolicyVpsRuleInput>> {
    let mut inputs = HashMap::<String, Vec<PolicyVpsRuleInput>>::new();
    for rule in rules {
        inputs
            .entry(rule.client_id.clone())
            .or_default()
            .push(policy_vps_rule_input(rule));
    }
    for client_inputs in inputs.values_mut() {
        client_inputs.sort_by(|left, right| left.0.cmp(&right.0));
    }
    inputs
}

fn policy_vps_rule_inputs(
    rules: &[VpsRuleValueRecord],
    client_id: &str,
) -> Vec<PolicyVpsRuleInput> {
    let mut inputs = rules
        .iter()
        .filter(|rule| rule.client_id == client_id)
        .map(policy_vps_rule_input)
        .collect::<Vec<_>>();
    inputs.sort_by(|left, right| left.0.cmp(&right.0));
    inputs
}

fn policy_vps_rule_input(rule: &VpsRuleValueRecord) -> PolicyVpsRuleInput {
    (
        rule.key.clone(),
        rule.value_raw.clone(),
        rule.value_json.clone(),
    )
}

fn policy_selector_partial_failure(failures: &[(Uuid, String, String)]) -> anyhow::Error {
    const MAX_REPORTED_FAILURES: usize = 8;
    const MAX_ERROR_CHARS: usize = 256;

    let details = failures
        .iter()
        .take(MAX_REPORTED_FAILURES)
        .map(|(policy_id, name, error)| {
            let name = name.chars().take(MAX_POLICY_NAME_BYTES).collect::<String>();
            let error = error.chars().take(MAX_ERROR_CHARS).collect::<String>();
            format!("{policy_id} ({name}): {error}")
        })
        .collect::<Vec<_>>()
        .join("; ");
    let omitted = failures.len().saturating_sub(MAX_REPORTED_FAILURES);
    anyhow::anyhow!(
        "fleet_alert_policy_evaluation_partial_failure: {} malformed persisted group selector(s): {}{}",
        failures.len(),
        details,
        if omitted == 0 {
            String::new()
        } else {
            format!("; {omitted} additional failure(s) omitted")
        }
    )
}

fn traffic_accounting_for_client(
    client_id: &str,
    rules: &[VpsRuleValueRecord],
    traffic_usage: &[TrafficCounterStreamUsage],
    now: DateTime<Utc>,
) -> TrafficAccountingRecord {
    traffic_accounting_for_client_with_selector_override(client_id, rules, traffic_usage, now, None)
}

fn traffic_accounting_for_agents(
    agents: &[AgentView],
    rules: &[VpsRuleValueRecord],
    traffic_usage: &[TrafficCounterStreamUsage],
    now: DateTime<Utc>,
) -> Vec<TrafficAccountingRecord> {
    agents
        .iter()
        .map(|agent| traffic_accounting_for_client(&agent.id, rules, traffic_usage, now))
        .collect()
}

fn traffic_override_for_rule(
    client_id: &str,
    rule: &PolicyRuleRequest,
    rules: &[VpsRuleValueRecord],
    traffic_usage: &[TrafficCounterStreamUsage],
    now: DateTime<Utc>,
) -> Option<TrafficAccountingRecord> {
    if !policy_condition_uses_traffic(&rule.trigger_condition_expression).unwrap_or(false) {
        return None;
    }
    let selector = rule
        .traffic_selector
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some(traffic_accounting_for_client_with_selector_override(
        client_id,
        rules,
        traffic_usage,
        now,
        Some(selector),
    ))
}

fn traffic_accounting_for_client_with_selector_override(
    client_id: &str,
    rules: &[VpsRuleValueRecord],
    traffic_usage: &[TrafficCounterStreamUsage],
    now: DateTime<Utc>,
    selector_override: Option<&str>,
) -> TrafficAccountingRecord {
    let rule_map = rules
        .iter()
        .filter(|rule| rule.client_id == client_id)
        .map(|rule| (rule.key.as_str(), rule))
        .collect::<HashMap<_, _>>();
    let mut incomplete_reasons = Vec::new();
    let reset_day = rule_map
        .get(VPS_RULE_KEY_TRAFFIC_RESET_DAY)
        .and_then(|rule| parse_persisted_vps_rule_value(&rule.key, &rule.value_raw).ok())
        .and_then(|parsed| parsed.json.get("day").and_then(Value::as_i64))
        .map(|value| value as i32);
    if reset_day.is_none() {
        incomplete_reasons.push("traffic.reset_day missing".to_string());
    }
    let (selectors, selector_error) = match selector_override {
        Some(selector) => match parse_persisted_traffic_selector_list(selector) {
            Ok(selectors) => (selectors, None),
            Err(error) => (
                Vec::new(),
                Some(format!("traffic.policy_selector invalid: {error}")),
            ),
        },
        None => match rule_map.get(VPS_RULE_KEY_TRAFFIC_SELECTORS) {
            Some(rule) => match traffic_selectors_from_rule(rule) {
                Ok(selectors) => (selectors, None),
                Err(error) => (
                    Vec::new(),
                    Some(format!("traffic.selectors invalid: {error}")),
                ),
            },
            None => (Vec::new(), None),
        },
    };
    if let Some(error) = selector_error {
        incomplete_reasons.push(error);
    } else if selectors.is_empty() {
        incomplete_reasons.push("traffic.selectors missing".to_string());
    }
    let quota_total = quota_value(&rule_map, VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL);
    let quota_rx = quota_value(&rule_map, VPS_RULE_KEY_TRAFFIC_QUOTA_RX);
    let quota_tx = quota_value(&rule_map, VPS_RULE_KEY_TRAFFIC_QUOTA_TX);
    let cycle_bounds = (reset_day != Some(-1)).then(|| cycle_bounds(reset_day.unwrap_or(1), now));
    let mut rx_bytes = 0_i64;
    let mut tx_bytes = 0_i64;
    let mut total_bytes = 0_i64;
    let mut diagnostic_rx_bytes = 0_i64;
    let mut diagnostic_tx_bytes = 0_i64;
    let mut latest_rx = 0_i64;
    let mut latest_tx = 0_i64;
    let mut latest_total = 0_i64;
    let mut last_sample_unix = None::<i64>;
    let mut counted_directions = HashMap::new();
    let mut diagnostic_streams = HashSet::new();
    let mut stale_reasons = Vec::new();
    let mut breakdown = Vec::new();
    for selector in &selectors {
        let selected_usage = traffic_usage
            .iter()
            .find(|usage| {
                usage.client_id == client_id
                    && usage.source_kind == selector.source
                    && usage.interface == selector.interface
                    && usage.last_sample_unix <= now.timestamp()
            })
            .cloned();
        let Some(usage) = selected_usage else {
            breakdown.push(TrafficAccountingSelectorBreakdown {
                source: selector.source.clone(),
                interface: selector.interface.clone(),
                direction: selector.direction.clone(),
                latest_rx_bytes: 0,
                latest_tx_bytes: 0,
                cycle_rx_bytes: 0,
                cycle_tx_bytes: 0,
                cycle_total_bytes: 0,
                sample_age_secs: None,
                state: "incomplete".to_string(),
                incomplete_reasons: vec!["runtime interface data missing".to_string()],
            });
            incomplete_reasons.push(format!("{} sample missing", selector.canonical));
            continue;
        };
        last_sample_unix = Some(last_sample_unix.map_or(usage.last_sample_unix, |last| {
            last.min(usage.last_sample_unix)
        }));
        let sample_age = Some(now.timestamp() - usage.last_sample_unix);
        let mut selected_cycle_rx = usage.cycle_rx;
        let mut selected_cycle_tx = usage.cycle_tx;
        let mut selected_latest_rx = usage.latest_rx;
        let mut selected_latest_tx = usage.latest_tx;
        let diagnostic_cycle_rx = usage.cycle_rx;
        let diagnostic_cycle_tx = usage.cycle_tx;
        let diagnostic_latest_rx = usage.latest_rx;
        let diagnostic_latest_tx = usage.latest_tx;
        if diagnostic_streams.insert((selector.source.clone(), selector.interface.clone())) {
            diagnostic_rx_bytes = diagnostic_rx_bytes.saturating_add(diagnostic_cycle_rx);
            diagnostic_tx_bytes = diagnostic_tx_bytes.saturating_add(diagnostic_cycle_tx);
        }
        match selector.direction.as_str() {
            "rx" => {
                selected_cycle_tx = 0;
                selected_latest_tx = 0;
            }
            "tx" => {
                selected_cycle_rx = 0;
                selected_latest_rx = 0;
            }
            _ => {}
        }
        let (count_rx, count_tx) =
            claim_traffic_selector_directions(&mut counted_directions, selector);
        if !count_rx {
            selected_cycle_rx = 0;
            selected_latest_rx = 0;
        }
        if !count_tx {
            selected_cycle_tx = 0;
            selected_latest_tx = 0;
        }
        rx_bytes += selected_cycle_rx;
        tx_bytes += selected_cycle_tx;
        total_bytes = total_bytes.saturating_add(traffic_selector_total(
            selector,
            selected_cycle_rx,
            selected_cycle_tx,
        ));
        latest_rx += selected_latest_rx;
        latest_tx += selected_latest_tx;
        latest_total = latest_total.saturating_add(traffic_selector_total(
            selector,
            selected_latest_rx,
            selected_latest_tx,
        ));
        let mut row_state = "ok".to_string();
        let mut row_reasons = Vec::new();
        if sample_age.is_some_and(|age| age > TRAFFIC_SAMPLE_STALE_SECS) {
            if row_state == "ok" {
                row_state = "stale".to_string();
            }
            row_reasons.push("stale sample".to_string());
            stale_reasons.push(format!("{} sample stale", selector.canonical));
        }
        breakdown.push(TrafficAccountingSelectorBreakdown {
            source: selector.source.clone(),
            interface: selector.interface.clone(),
            direction: selector.direction.clone(),
            latest_rx_bytes: diagnostic_latest_rx,
            latest_tx_bytes: diagnostic_latest_tx,
            cycle_rx_bytes: diagnostic_cycle_rx,
            cycle_tx_bytes: diagnostic_cycle_tx,
            cycle_total_bytes: traffic_selector_total(
                selector,
                diagnostic_cycle_rx,
                diagnostic_cycle_tx,
            ),
            sample_age_secs: sample_age,
            state: row_state,
            incomplete_reasons: row_reasons,
        });
    }
    let diagnostic_total_bytes = diagnostic_rx_bytes.saturating_add(diagnostic_tx_bytes);
    let cycle_percent = [
        quota_total
            .filter(|quota| *quota > 0)
            .map(|quota| percent(total_bytes, quota)),
        quota_rx
            .filter(|quota| *quota > 0)
            .map(|quota| percent(rx_bytes, quota)),
        quota_tx
            .filter(|quota| *quota > 0)
            .map(|quota| percent(tx_bytes, quota)),
    ]
    .into_iter()
    .flatten()
    .max_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let state = if !incomplete_reasons.is_empty() {
        "incomplete"
    } else if last_sample_unix.is_none() {
        "unknown"
    } else if !stale_reasons.is_empty() {
        "stale"
    } else {
        "ok"
    };
    incomplete_reasons.extend(stale_reasons);
    incomplete_reasons.sort();
    incomplete_reasons.dedup();
    let counter_epochs_seen = counted_directions
        .iter()
        .filter_map(|((source_kind, interface), direction_mask)| {
            traffic_usage
                .iter()
                .find(|usage| {
                    usage.client_id == client_id
                        && usage.source_kind == *source_kind
                        && usage.interface == *interface
                        && usage.last_sample_unix <= now.timestamp()
                })
                .map(|usage| {
                    let rx_epochs = if direction_mask & 0b01 != 0 {
                        usage.rx_counter_epochs_seen
                    } else {
                        0
                    };
                    let tx_epochs = if direction_mask & 0b10 != 0 {
                        usage.tx_counter_epochs_seen
                    } else {
                        0
                    };
                    rx_epochs.max(tx_epochs)
                })
        })
        .fold(0_i64, i64::saturating_add);
    let selector_hash = selector_hash(
        &selectors
            .iter()
            .map(|selector| selector.canonical.clone())
            .collect::<Vec<_>>(),
    );
    TrafficAccountingRecord {
        client_id: client_id.to_string(),
        selectors: selectors
            .iter()
            .map(|selector| selector.canonical.clone())
            .collect(),
        selector_hash,
        cycle_start: cycle_bounds.map(|(start, _)| start.to_rfc3339()),
        cycle_end: cycle_bounds.map(|(_, end)| end.to_rfc3339()),
        reset_day,
        rx_bytes,
        tx_bytes,
        total_bytes,
        diagnostic_rx_bytes,
        diagnostic_tx_bytes,
        diagnostic_total_bytes,
        latest_rx_bytes: latest_rx,
        latest_tx_bytes: latest_tx,
        latest_total_bytes: latest_total,
        quota_rx_bytes: quota_rx,
        quota_tx_bytes: quota_tx,
        quota_total_bytes: quota_total,
        cycle_percent,
        state: state.to_string(),
        incomplete_reasons,
        last_sample_at: last_sample_unix
            .and_then(|unix| Utc.timestamp_opt(unix, 0).single())
            .map(|value| value.to_rfc3339()),
        counter_epochs_seen,
        updated_at: now.to_rfc3339(),
        selector_breakdown: breakdown,
    }
}

#[derive(Default)]
struct CycleUsage {
    cycle_rx: i64,
    cycle_tx: i64,
    latest_rx: i64,
    latest_tx: i64,
    last_sample_unix: Option<i64>,
}

fn derive_cycle_usage(
    samples: &[TrafficCounterSampleRecord],
    cycle_start_unix: i64,
    now_unix: i64,
) -> CycleUsage {
    let mut sorted = samples.to_vec();
    sorted.sort_by_key(|sample| sample.observed_unix);
    let mut usage = CycleUsage::default();
    let mut previous: Option<TrafficCounterSampleRecord> = None;
    for sample in sorted {
        if sample.observed_unix > now_unix {
            continue;
        }
        usage.latest_rx = sample.rx_bytes;
        usage.latest_tx = sample.tx_bytes;
        usage.last_sample_unix = Some(sample.observed_unix);
        if let Some(prev) = previous.as_ref() {
            if sample.observed_unix >= cycle_start_unix {
                let rx_delta = if sample.rx_counter_epoch == prev.rx_counter_epoch
                    && sample.rx_bytes >= prev.rx_bytes
                {
                    sample.rx_bytes - prev.rx_bytes
                } else {
                    0
                };
                let tx_delta = if sample.tx_counter_epoch == prev.tx_counter_epoch
                    && sample.tx_bytes >= prev.tx_bytes
                {
                    sample.tx_bytes - prev.tx_bytes
                } else {
                    0
                };
                usage.cycle_rx += rx_delta;
                usage.cycle_tx += tx_delta;
            }
        }
        previous = Some(sample);
    }
    usage
}

fn quota_value(rule_map: &HashMap<&str, &VpsRuleValueRecord>, key: &str) -> Option<i64> {
    rule_map
        .get(key)
        .and_then(|rule| parse_persisted_vps_rule_value(&rule.key, &rule.value_raw).ok())
        .and_then(|parsed| parsed.json.get("bytes").and_then(Value::as_i64))
}

fn percent(value: i64, quota: i64) -> f64 {
    if quota <= 0 {
        0.0
    } else {
        (value as f64 / quota as f64) * 100.0
    }
}

fn cycle_bounds(reset_day: i32, now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let current_boundary = boundary_for_month(now.year(), now.month(), reset_day);
    if now >= current_boundary {
        let (next_year, next_month) = if now.month() == 12 {
            (now.year() + 1, 1)
        } else {
            (now.year(), now.month() + 1)
        };
        (
            current_boundary,
            boundary_for_month(next_year, next_month, reset_day),
        )
    } else {
        let (prev_year, prev_month) = if now.month() == 1 {
            (now.year() - 1, 12)
        } else {
            (now.year(), now.month() - 1)
        };
        (
            boundary_for_month(prev_year, prev_month, reset_day),
            current_boundary,
        )
    }
}

fn boundary_for_month(year: i32, month: u32, reset_day: i32) -> DateTime<Utc> {
    let day = reset_day.clamp(1, days_in_month(year, month) as i32) as u32;
    Utc.with_ymd_and_hms(year, month, day, 0, 0, 0)
        .single()
        .expect("valid clamped UTC cycle boundary")
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let first_next = Utc
        .with_ymd_and_hms(next_year, next_month, 1, 0, 0, 0)
        .single()
        .expect("valid next month");
    (first_next - chrono::Duration::days(1)).day()
}

async fn lock_policy_group_identity_upserts_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind("vpsman:policy-group-identity-upserts")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn policy_groups_for_identity_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    requested_id: Option<Uuid>,
    requested_name: &str,
) -> Result<Vec<PolicyGroupRecord>> {
    let rows = sqlx::query(
        r#"
        SELECT
            id,
            name,
            enabled,
            selector_expression,
            notes,
            created_by,
            updated_by,
            created_at::text AS created_at,
            updated_at::text AS updated_at
        FROM policy_groups
        WHERE ($1::uuid IS NOT NULL AND id = $1)
           OR name = $2
        ORDER BY id
        FOR UPDATE
        "#,
    )
    .bind(requested_id)
    .bind(requested_name)
    .fetch_all(&mut **tx)
    .await?;
    let group_ids = rows
        .iter()
        .map(|row| row.try_get::<Uuid, _>("id"))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut rules_by_group = HashMap::<Uuid, Vec<PolicyRuleRecord>>::new();
    if !group_ids.is_empty() {
        for row in sqlx::query(
            r#"
            SELECT
                id,
                group_id,
                rule_version,
                sort_order,
                name,
                enabled,
                rule_kind,
                evidence_source,
                correlation_mode,
                traffic_selector,
                trigger_condition_expression,
                trigger_meta_condition,
                resolve_condition_expression,
                resolve_meta_condition,
                severity,
                category,
                title_template,
                detail_template,
                system_seed_key,
                armed_after_evidence_seq,
                armed_at::text AS armed_at,
                created_at::text AS created_at,
                updated_at::text AS updated_at
            FROM policy_rules
            WHERE group_id = ANY($1)
            ORDER BY group_id ASC, sort_order ASC, created_at ASC
            "#,
        )
        .bind(&group_ids)
        .fetch_all(&mut **tx)
        .await?
        {
            let rule = policy_rule_from_row(row)?;
            rules_by_group.entry(rule.group_id).or_default().push(rule);
        }
    }
    rows.into_iter()
        .map(|row| {
            let group_id: Uuid = row.try_get("id")?;
            policy_group_from_row(row, rules_by_group.remove(&group_id).unwrap_or_default())
        })
        .collect()
}

fn select_existing_policy_group(
    groups: &[PolicyGroupRecord],
    requested_id: Option<Uuid>,
    requested_name: &str,
) -> Result<Option<PolicyGroupRecord>> {
    let existing_by_id = requested_id.and_then(|id| groups.iter().find(|group| group.id == id));
    let existing_by_name = groups
        .iter()
        .find(|group| group.name == requested_name.trim());
    if let (Some(requested_id), Some(named_group)) = (requested_id, existing_by_name) {
        anyhow::ensure!(
            named_group.id == requested_id,
            "fleet_alert_policy_name_conflict"
        );
    }
    Ok(existing_by_id.or(existing_by_name).cloned())
}

fn policy_group_from_request(
    request: &CreateFleetAlertPolicyRequest,
    dry_run: &PolicyDryRunResponse,
    now: &str,
    existing_group: Option<&PolicyGroupRecord>,
    operator: &AuthContext,
) -> Result<PolicyGroupRecord> {
    let group_id = existing_group
        .map(|group| group.id)
        .or(request.id)
        .unwrap_or_else(Uuid::new_v4);
    let existing_rule_ids = existing_group
        .map(|group| {
            group
                .rules
                .iter()
                .map(|rule| rule.id)
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    for rule in &request.rules {
        if let Some(id) = rule.id {
            anyhow::ensure!(
                existing_group.is_some() && existing_rule_ids.contains(&id),
                "fleet_alert_policy_rule_id_unknown:{id}"
            );
        }
    }
    let rules = request
        .rules
        .iter()
        .enumerate()
        .map(|(index, rule)| {
            policy_rule_from_request(group_id, rule, index as i32, now, existing_group)
        })
        .collect::<Vec<_>>();
    anyhow::ensure!(
        rules
            .iter()
            .map(|rule| rule.id)
            .collect::<HashSet<_>>()
            .len()
            == rules.len(),
        "fleet_alert_policy_rule_identity_conflict"
    );
    Ok(PolicyGroupRecord {
        id: group_id,
        name: request.name.trim().to_string(),
        enabled: request.enabled,
        selector_expression: request.selector_expression.trim().to_string(),
        notes: clean_optional_text(request.notes.as_deref()),
        matched_vps_count: dry_run.matched_vps_count as i64,
        rule_count: rules.len() as i64,
        enabled_rule_count: rules.iter().filter(|rule| rule.enabled).count() as i64,
        active_info_count: 0,
        active_warning_count: 0,
        active_critical_count: 0,
        incomplete_vps_count: dry_run.incomplete_vps_count as i64,
        last_evaluated_at: None,
        rules,
        created_by: existing_group
            .and_then(|existing| existing.created_by)
            .or(Some(operator.operator.id)),
        updated_by: Some(operator.operator.id),
        created_at: existing_group
            .map(|existing| existing.created_at.clone())
            .unwrap_or_else(|| now.to_string()),
        updated_at: now.to_string(),
    })
}

fn policy_group_scope_changed(
    existing_group: Option<&PolicyGroupRecord>,
    group: &PolicyGroupRecord,
) -> bool {
    existing_group.is_some_and(|existing| {
        existing.enabled != group.enabled
            || existing.selector_expression != group.selector_expression
    })
}

fn invalidated_policy_rule_ids(
    existing_group: Option<&PolicyGroupRecord>,
    group: &PolicyGroupRecord,
) -> HashSet<Uuid> {
    let Some(existing) = existing_group else {
        return HashSet::new();
    };
    if policy_group_scope_changed(Some(existing), group) {
        return existing.rules.iter().map(|rule| rule.id).collect();
    }
    existing
        .rules
        .iter()
        .filter(|existing_rule| {
            !group.rules.iter().any(|rule| {
                rule.id == existing_rule.id
                    && rule.rule_version == existing_rule.rule_version
                    && rule.enabled
            })
        })
        .map(|rule| rule.id)
        .collect()
}

fn policy_change_resolution_reason(
    existing_group: Option<&PolicyGroupRecord>,
    group: &PolicyGroupRecord,
) -> &'static str {
    match existing_group {
        Some(existing) if existing.enabled && !group.enabled => "policy_disabled",
        Some(existing) if existing.selector_expression != group.selector_expression => {
            "policy_scope_changed"
        }
        _ => "policy_changed",
    }
}

fn validate_policy_group_request(
    request: &CreateFleetAlertPolicyRequest,
    require_confirmed: bool,
    require_names: bool,
) -> Result<()> {
    if require_confirmed {
        anyhow::ensure!(
            request.confirmed,
            "fleet_alert_policy_confirmation_required"
        );
    }
    if require_names {
        validate_name(
            &request.name,
            MAX_POLICY_NAME_BYTES,
            "fleet alert policy name",
        )?;
    }
    anyhow::ensure!(
        !request.selector_expression.trim().is_empty()
            && request.selector_expression.len() <= MAX_SELECTOR_EXPRESSION_BYTES,
        "fleet alert policy selector expression is invalid"
    );
    parse_selector_expression(&request.selector_expression)
        .map_err(|error| anyhow::anyhow!("invalid selector expression: {error}"))?
        .context("selector expression is empty")?;
    if let Some(notes) = request.notes.as_deref() {
        anyhow::ensure!(
            notes.len() <= MAX_POLICY_NOTES_BYTES,
            "fleet alert policy notes are too long"
        );
    }
    anyhow::ensure!(
        !request.rules.is_empty(),
        "fleet alert policy requires at least one rule"
    );
    let mut rule_ids = HashSet::new();
    for rule in &request.rules {
        if let Some(rule_id) = rule.id {
            anyhow::ensure!(
                rule_ids.insert(rule_id),
                "fleet_alert_policy_duplicate_rule_id"
            );
        }
        if require_names {
            validate_policy_rule_request(rule)?;
        } else {
            validate_policy_rule_request_for_preview(rule)?;
        }
    }
    anyhow::ensure!(
        request.selector_expression.trim() == "*"
            || !request
                .rules
                .iter()
                .any(|rule| rule.enabled && rule.evidence_source == "job.terminal"),
        "fleet_alert_policy_subjectless_source_requires_global_scope"
    );
    Ok(())
}

fn validate_policy_rule_request(rule: &PolicyRuleRequest) -> Result<()> {
    validate_name(
        &rule.name,
        MAX_RULE_NAME_BYTES,
        "fleet alert policy rule name",
    )?;
    validate_policy_rule_request_for_preview(rule)
}

fn validate_policy_rule_request_for_preview(rule: &PolicyRuleRequest) -> Result<()> {
    anyhow::ensure!(
        matches!(rule.severity.as_str(), "info" | "warning" | "critical"),
        "fleet_alert_policy_severity_invalid"
    );
    validate_policy_rule_source_shape(rule)?;
    validate_policy_meta_condition(rule.trigger_meta_condition.as_ref(), false)?;
    validate_policy_meta_condition(rule.resolve_meta_condition.as_ref(), true)?;
    anyhow::ensure!(
        !rule.trigger_condition_expression.trim().is_empty()
            && rule.trigger_condition_expression.len() <= MAX_CONDITION_EXPRESSION_BYTES,
        "fleet_alert_policy_condition_invalid"
    );
    validate_policy_expression_for_source(
        rule.rule_kind,
        &rule.evidence_source,
        &rule.trigger_condition_expression,
    )?;
    if let Some(expression) = rule.resolve_condition_expression.as_deref() {
        anyhow::ensure!(
            !expression.trim().is_empty() && expression.len() <= MAX_CONDITION_EXPRESSION_BYTES,
            "fleet_alert_policy_resolve_condition_invalid"
        );
        validate_policy_expression_for_source(rule.rule_kind, &rule.evidence_source, expression)?;
    }
    if let Some(selector) = rule
        .traffic_selector
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        parse_traffic_selector_list(selector)?;
    }
    if rule.rule_kind != AlertPolicyRuleKind::Metric
        || !policy_condition_uses_traffic(&rule.trigger_condition_expression)?
    {
        anyhow::ensure!(
            rule.traffic_selector
                .as_deref()
                .is_none_or(|value| value.trim().is_empty()),
            "fleet_alert_policy_traffic_selector_requires_traffic_metric"
        );
    }
    anyhow::ensure!(
        matches!(
            rule.category.as_str(),
            "agent_status"
                | "network"
                | "backup"
                | "agent_update"
                | "job"
                | "capability_degraded"
                | "traffic"
                | "resource"
        ),
        "fleet_alert_policy_category_invalid"
    );
    validate_policy_presentation_template(
        rule,
        &rule.title_template,
        256,
        "fleet_alert_policy_title_template_invalid",
    )?;
    validate_policy_presentation_template(
        rule,
        &rule.detail_template,
        4096,
        "fleet_alert_policy_detail_template_invalid",
    )?;
    Ok(())
}

fn validate_policy_presentation_template(
    rule: &PolicyRuleRequest,
    template: &str,
    max_bytes: usize,
    error_code: &str,
) -> Result<()> {
    anyhow::ensure!(
        !template.trim().is_empty() && template.len() <= max_bytes && !template.contains('\0'),
        "{error_code}"
    );
    let mut remaining = template;
    while let Some(open) = remaining.find('{') {
        anyhow::ensure!(!remaining[..open].contains('}'), "{error_code}");
        let after = &remaining[open + 1..];
        let close = after.find('}').context(error_code.to_string())?;
        let path = after[..close].trim();
        anyhow::ensure!(
            !path.is_empty()
                && !path.contains(['{', '[', ']', '(', ')', '|'])
                && policy_template_path_allowed(rule, path),
            "{error_code}:{path}"
        );
        remaining = &after[close + 1..];
    }
    anyhow::ensure!(!remaining.contains('}'), "{error_code}");
    Ok(())
}

fn policy_template_path_allowed(rule: &PolicyRuleRequest, path: &str) -> bool {
    if matches!(
        path,
        "policy.id"
            | "policy.name"
            | "policy_rule.id"
            | "policy_rule.name"
            | "policy_rule.rule_version"
            | "policy_rule.rule_kind"
            | "policy_rule.trigger_condition_expression"
    ) {
        return true;
    }
    if path.starts_with("subject.") {
        return rule.correlation_mode != AlertPolicyCorrelationMode::Global
            && rule.evidence_source != "job.terminal"
            && matches!(
                path,
                "subject.client_id" | "subject.display_name" | "subject.status"
            );
    }
    if let Some(field) = path.strip_prefix("evidence.") {
        if rule.correlation_mode == AlertPolicyCorrelationMode::Global && field == "client_id" {
            return false;
        }
        if rule.evidence_source == "telemetry.combined" {
            return matches!(
                field,
                "traffic.quota.total"
                    | "traffic.quota.rx"
                    | "traffic.quota.tx"
                    | "traffic.cycle.total"
                    | "traffic.cycle.rx"
                    | "traffic.cycle.tx"
                    | "traffic.cycle_percent"
                    | "cpu.utilization_ratio"
                    | "cpu.load_1"
                    | "cpu.load_saturation"
                    | "memory.available_ratio"
                    | "disk.available_ratio"
            );
        }
        return policy_source_field_allowed(&rule.evidence_source, path);
    }
    false
}

fn validate_policy_rule_source_shape(rule: &PolicyRuleRequest) -> Result<()> {
    let expected_kind = match rule.evidence_source.as_str() {
        "telemetry.combined" => AlertPolicyRuleKind::Metric,
        "agent.status" | "agent.access" | "tunnel.adapter" | "tunnel.traffic" => {
            AlertPolicyRuleKind::State
        }
        "job.terminal" | "backup.failure" | "job.capability" => AlertPolicyRuleKind::Occurrence,
        _ => anyhow::bail!("fleet_alert_policy_evidence_source_unsupported"),
    };
    anyhow::ensure!(
        rule.rule_kind == expected_kind,
        "fleet_alert_policy_evidence_source_rule_kind_mismatch"
    );
    match rule.rule_kind {
        AlertPolicyRuleKind::Metric | AlertPolicyRuleKind::State => {
            anyhow::ensure!(
                rule.correlation_mode == AlertPolicyCorrelationMode::NaturalKey,
                "fleet_alert_policy_correlation_mode_invalid"
            );
            anyhow::ensure!(
                !matches!(
                    rule.trigger_meta_condition,
                    Some(AlertPolicyMetaCondition::ElapsedSinceTrigger { .. })
                ),
                "fleet_alert_policy_trigger_meta_invalid"
            );
            anyhow::ensure!(
                !matches!(
                    rule.resolve_meta_condition,
                    Some(AlertPolicyMetaCondition::ElapsedSinceTrigger { .. })
                ),
                "fleet_alert_policy_resolve_meta_invalid"
            );
        }
        AlertPolicyRuleKind::Occurrence => {
            let trigger_is_count = matches!(
                rule.trigger_meta_condition,
                Some(AlertPolicyMetaCondition::Count { .. })
            );
            anyhow::ensure!(
                if trigger_is_count {
                    matches!(
                        rule.correlation_mode,
                        AlertPolicyCorrelationMode::Subject | AlertPolicyCorrelationMode::Global
                    )
                } else {
                    rule.correlation_mode == AlertPolicyCorrelationMode::NaturalKey
                        && rule
                            .trigger_meta_condition
                            .as_ref()
                            .is_none_or(|meta| matches!(meta, AlertPolicyMetaCondition::Immediate))
                },
                "fleet_alert_policy_correlation_mode_invalid"
            );
            anyhow::ensure!(
                !(rule.evidence_source == "job.terminal"
                    && rule.correlation_mode == AlertPolicyCorrelationMode::Subject),
                "fleet_alert_policy_subject_correlation_unavailable"
            );
            anyhow::ensure!(
                rule.resolve_condition_expression.is_none()
                    && matches!(
                        rule.resolve_meta_condition,
                        Some(AlertPolicyMetaCondition::ElapsedSinceTrigger { .. })
                    ),
                "fleet_alert_policy_occurrence_resolution_invalid"
            );
        }
    }
    Ok(())
}

fn validate_policy_meta_condition(
    condition: Option<&AlertPolicyMetaCondition>,
    allow_elapsed: bool,
) -> Result<()> {
    match condition {
        None | Some(AlertPolicyMetaCondition::Immediate) => Ok(()),
        Some(AlertPolicyMetaCondition::Sustained { seconds }) => {
            anyhow::ensure!(
                (1..=2_592_000).contains(seconds),
                "fleet_alert_policy_sustained_seconds_invalid"
            );
            Ok(())
        }
        Some(AlertPolicyMetaCondition::Count {
            confirmations,
            within_seconds,
        }) => {
            anyhow::ensure!(
                (1..=1000).contains(confirmations) && (1..=2_592_000).contains(within_seconds),
                "fleet_alert_policy_count_invalid"
            );
            Ok(())
        }
        Some(AlertPolicyMetaCondition::ElapsedSinceTrigger { seconds }) => {
            anyhow::ensure!(
                allow_elapsed && (1..=31_536_000).contains(seconds),
                "fleet_alert_policy_elapsed_invalid"
            );
            Ok(())
        }
    }
}

fn validate_policy_expression_for_source(
    rule_kind: AlertPolicyRuleKind,
    evidence_source: &str,
    expression: &str,
) -> Result<()> {
    if rule_kind == AlertPolicyRuleKind::Metric {
        parse_policy_trigger_condition_expression(expression)
            .map(|_| ())
            .map_err(|error| anyhow::anyhow!("fleet_alert_policy_condition_invalid: {error}"))
    } else {
        let parsed = vpsman_common::parse_expression(expression)
            .map_err(|error| anyhow::anyhow!("fleet_alert_policy_condition_invalid: {error}"))?
            .context("fleet_alert_policy_condition_invalid")?;
        validate_policy_expression_fields(&parsed, evidence_source)
    }
}

fn validate_policy_expression_fields(expression: &Expression, evidence_source: &str) -> Result<()> {
    use vpsman_common::Predicate;
    match expression {
        Expression::Predicate(Predicate::Comparison { field, .. })
        | Expression::Predicate(Predicate::Membership { field, .. }) => {
            anyhow::ensure!(
                policy_source_field_allowed(evidence_source, field),
                "fleet_alert_policy_evidence_field_unsupported:{field}"
            );
            Ok(())
        }
        Expression::Predicate(_) => anyhow::bail!("fleet_alert_policy_predicate_unsupported"),
        Expression::Not(inner) => validate_policy_expression_fields(inner, evidence_source),
        Expression::And(left, right) | Expression::Or(left, right) => {
            validate_policy_expression_fields(left, evidence_source)?;
            validate_policy_expression_fields(right, evidence_source)
        }
    }
}

fn policy_source_field_allowed(evidence_source: &str, field: &str) -> bool {
    let allowed: &[&str] = match evidence_source {
        "agent.status" | "agent.access" => &["evidence.status"],
        "tunnel.adapter" => &[
            "evidence.adapter.success",
            "evidence.interface",
            "evidence.reason",
        ],
        "tunnel.traffic" => &[
            "evidence.traffic.status",
            "evidence.interface",
            "evidence.reason",
        ],
        "job.terminal" => &[
            "evidence.status",
            "evidence.command_type",
            "evidence.job_id",
            "evidence.target_count",
        ],
        "backup.failure" => &[
            "evidence.status",
            "evidence.backup_request_id",
            "evidence.client_id",
        ],
        "job.capability" => &[
            "evidence.status",
            "evidence.reason",
            "evidence.hint",
            "evidence.job_id",
            "evidence.command_type",
            "evidence.client_id",
        ],
        _ => &[],
    };
    allowed
        .iter()
        .any(|allowed| field.eq_ignore_ascii_case(allowed))
}

fn validate_name(value: &str, max_bytes: usize, field: &str) -> Result<()> {
    let value = value.trim();
    anyhow::ensure!(!value.is_empty(), "{field} is required");
    anyhow::ensure!(value.len() <= max_bytes, "{field} is too long");
    anyhow::ensure!(
        value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'.' | b'_' | b'-' | b':')
        }),
        "{field} contains unsupported characters"
    );
    Ok(())
}

fn clean_optional_text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn policy_rule_from_request(
    group_id: Uuid,
    request: &PolicyRuleRequest,
    sort_order: i32,
    now: &str,
    existing_group: Option<&PolicyGroupRecord>,
) -> PolicyRuleRecord {
    let existing_rule = existing_group.and_then(|group| match request.id {
        Some(id) => group.rules.iter().find(|rule| rule.id == id),
        None => group.rules.iter().find(|rule| {
            rule.sort_order == sort_order && policy_rule_material_matches(rule, request)
        }),
    });
    let rule_version = existing_rule
        .map(|existing| {
            if policy_rule_material_matches(existing, request) {
                existing.rule_version
            } else {
                existing.rule_version.saturating_add(1)
            }
        })
        .unwrap_or(1);
    PolicyRuleRecord {
        id: existing_rule
            .map(|rule| rule.id)
            .or(request.id)
            .unwrap_or_else(Uuid::new_v4),
        group_id,
        rule_version,
        sort_order,
        name: request.name.trim().to_string(),
        enabled: request.enabled,
        rule_kind: request.rule_kind,
        evidence_source: request.evidence_source.trim().to_string(),
        correlation_mode: request.correlation_mode,
        traffic_selector: request
            .traffic_selector
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        trigger_condition_expression: request.trigger_condition_expression.trim().to_string(),
        trigger_meta_condition: canonical_policy_meta(request.trigger_meta_condition.as_ref()),
        resolve_condition_expression: clean_optional_text(
            request.resolve_condition_expression.as_deref(),
        ),
        resolve_meta_condition: canonical_policy_meta(request.resolve_meta_condition.as_ref()),
        severity: request.severity.trim().to_string(),
        category: request.category.trim().to_string(),
        title_template: request.title_template.trim().to_string(),
        detail_template: request.detail_template.trim().to_string(),
        system_seed_key: existing_rule.and_then(|rule| rule.system_seed_key.clone()),
        armed_after_evidence_seq: existing_rule
            .map(|rule| rule.armed_after_evidence_seq)
            .unwrap_or(0),
        armed_at: existing_rule
            .map(|rule| rule.armed_at.clone())
            .unwrap_or_else(|| now.to_string()),
        created_at: existing_rule
            .map(|rule| rule.created_at.clone())
            .unwrap_or_else(|| now.to_string()),
        updated_at: now.to_string(),
    }
}

fn policy_rule_material_matches(existing: &PolicyRuleRecord, request: &PolicyRuleRequest) -> bool {
    existing.name == request.name.trim()
        && existing.enabled == request.enabled
        && existing.traffic_selector.as_deref()
            == request
                .traffic_selector
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        && existing.trigger_condition_expression == request.trigger_condition_expression.trim()
        && existing.rule_kind == request.rule_kind
        && existing.evidence_source == request.evidence_source.trim()
        && existing.correlation_mode == request.correlation_mode
        && existing.trigger_meta_condition
            == canonical_policy_meta(request.trigger_meta_condition.as_ref())
        && existing.resolve_condition_expression.as_deref()
            == request
                .resolve_condition_expression
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        && existing.resolve_meta_condition
            == canonical_policy_meta(request.resolve_meta_condition.as_ref())
        && existing.severity == request.severity.trim()
        && existing.category == request.category.trim()
        && existing.title_template == request.title_template.trim()
        && existing.detail_template == request.detail_template.trim()
}

fn canonical_policy_meta(
    value: Option<&AlertPolicyMetaCondition>,
) -> Option<AlertPolicyMetaCondition> {
    match value {
        None | Some(AlertPolicyMetaCondition::Immediate) => None,
        Some(value) => Some(value.clone()),
    }
}

fn evaluate_rule_for_client(
    rule: &PolicyRuleRequest,
    traffic: Option<&TrafficAccountingRecord>,
    rollup: Option<&TelemetryRollupView>,
) -> PolicyEvaluation {
    let mut incomplete_reasons = Vec::new();
    let parsed = match parse_policy_trigger_condition_expression(&rule.trigger_condition_expression)
    {
        Ok(parsed) => parsed,
        Err(error) => {
            incomplete_reasons.push(format!("condition expression invalid: {error}"));
            return policy_evaluation_from_parts(
                false,
                incomplete_reasons,
                None,
                None,
                "resource",
                traffic,
            );
        }
    };
    let result = match evaluate_policy_condition(&parsed, traffic, rollup, &mut incomplete_reasons)
    {
        Ok(result) => result,
        Err(error) => {
            incomplete_reasons.push(format!("condition expression invalid: {error}"));
            ConditionEvaluation {
                truth: ExpressionTruth::Unknown,
                actual_value: None,
                threshold_value: None,
            }
        }
    };
    let category = if parsed.uses_traffic {
        "traffic"
    } else {
        "resource"
    };
    let condition_true = result.truth == ExpressionTruth::True;
    if result.truth != ExpressionTruth::Unknown {
        incomplete_reasons.clear();
    } else if incomplete_reasons.is_empty() {
        incomplete_reasons.push("condition evidence is incomplete".to_string());
    }
    policy_evaluation_from_parts(
        condition_true,
        incomplete_reasons,
        result.actual_value,
        result.threshold_value,
        category,
        traffic,
    )
}

fn policy_evaluation_from_parts(
    condition_true: bool,
    incomplete_reasons: Vec<String>,
    actual_value: Option<f64>,
    threshold_value: Option<f64>,
    category: &str,
    traffic: Option<&TrafficAccountingRecord>,
) -> PolicyEvaluation {
    let payload = if let Some(traffic) = traffic {
        json!({
            "traffic": {
                "selectors": traffic.selectors,
                "cycle_start": traffic.cycle_start,
                "cycle_end": traffic.cycle_end,
                "rx_bytes": traffic.rx_bytes,
                "tx_bytes": traffic.tx_bytes,
                "total_bytes": traffic.total_bytes,
                "quota_rx_bytes": traffic.quota_rx_bytes,
                "quota_tx_bytes": traffic.quota_tx_bytes,
                "quota_total_bytes": traffic.quota_total_bytes,
                "cycle_percent": traffic.cycle_percent,
                "reset_day": traffic.reset_day,
            }
        })
    } else {
        json!({})
    };
    PolicyEvaluation {
        condition_true,
        incomplete: !incomplete_reasons.is_empty(),
        incomplete_reasons,
        actual_value,
        threshold_value,
        category: category.to_string(),
        payload,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArithmeticOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    UnaryPlus,
    UnaryMinus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PolicyComparisonOperator {
    Eq,
    NotEq,
    Lt,
    Lte,
    Gt,
    Gte,
}

#[derive(Clone, Debug)]
enum PolicyConditionNode {
    Not(Box<PolicyConditionNode>),
    And(Box<PolicyConditionNode>, Box<PolicyConditionNode>),
    Or(Box<PolicyConditionNode>, Box<PolicyConditionNode>),
    Comparison {
        left: PolicyNumericNode,
        operator: PolicyComparisonOperator,
        right: PolicyNumericNode,
    },
}

#[derive(Clone, Debug)]
enum PolicyNumericNode {
    Number(f64),
    Identifier(String),
    Unary {
        operator: ArithmeticOperator,
        operand: Box<PolicyNumericNode>,
    },
    Binary {
        left: Box<PolicyNumericNode>,
        operator: ArithmeticOperator,
        right: Box<PolicyNumericNode>,
    },
}

#[derive(Clone, Debug)]
struct PolicyConditionExpression {
    root: PolicyConditionNode,
    uses_traffic: bool,
}

#[derive(Clone, Debug)]
struct ConditionEvaluation {
    truth: ExpressionTruth,
    actual_value: Option<f64>,
    threshold_value: Option<f64>,
}

#[derive(Clone, Debug)]
enum PolicyConditionToken {
    Number(f64),
    Identifier(String),
    Arithmetic(ArithmeticOperator),
    Comparison(PolicyComparisonOperator),
    And,
    Or,
    Not,
    LeftParen,
    RightParen,
}

#[derive(Clone)]
struct PolicyConditionParser {
    tokens: Vec<PolicyConditionToken>,
    position: usize,
}

fn parse_policy_trigger_condition_expression(
    expression: &str,
) -> Result<PolicyConditionExpression> {
    let tokens = tokenize_policy_condition(expression)?;
    anyhow::ensure!(!tokens.is_empty(), "condition expression is empty");
    let mut parser = PolicyConditionParser {
        tokens,
        position: 0,
    };
    let root = parser.parse_or()?;
    anyhow::ensure!(
        parser.peek().is_none(),
        "unexpected token after condition expression"
    );
    let uses_traffic = condition_node_uses_traffic(&root);
    Ok(PolicyConditionExpression { root, uses_traffic })
}

fn policy_condition_uses_traffic(expression: &str) -> Result<bool> {
    Ok(parse_policy_trigger_condition_expression(expression)?.uses_traffic)
}

/// Canonical runtime evaluator for persisted metric-policy evidence. Save,
/// preview, source evaluation, and timer rechecks all parse the same arithmetic
/// AST; missing/null/non-numeric evidence is Kleene Unknown.
pub(crate) fn metric_policy_expression_truth(
    expression: &str,
    evidence: &Value,
    complete: bool,
) -> Result<ExpressionTruth> {
    if !complete {
        return Ok(ExpressionTruth::Unknown);
    }
    let expression = parse_policy_trigger_condition_expression(expression)?;
    evaluate_metric_condition_node(&expression.root, evidence)
}

fn evaluate_metric_condition_node(
    node: &PolicyConditionNode,
    evidence: &Value,
) -> Result<ExpressionTruth> {
    Ok(match node {
        PolicyConditionNode::Not(inner) => match evaluate_metric_condition_node(inner, evidence)? {
            ExpressionTruth::True => ExpressionTruth::False,
            ExpressionTruth::False => ExpressionTruth::True,
            ExpressionTruth::Unknown => ExpressionTruth::Unknown,
        },
        PolicyConditionNode::And(left, right) => match (
            evaluate_metric_condition_node(left, evidence)?,
            evaluate_metric_condition_node(right, evidence)?,
        ) {
            (ExpressionTruth::False, _) | (_, ExpressionTruth::False) => ExpressionTruth::False,
            (ExpressionTruth::True, ExpressionTruth::True) => ExpressionTruth::True,
            _ => ExpressionTruth::Unknown,
        },
        PolicyConditionNode::Or(left, right) => match (
            evaluate_metric_condition_node(left, evidence)?,
            evaluate_metric_condition_node(right, evidence)?,
        ) {
            (ExpressionTruth::True, _) | (_, ExpressionTruth::True) => ExpressionTruth::True,
            (ExpressionTruth::False, ExpressionTruth::False) => ExpressionTruth::False,
            _ => ExpressionTruth::Unknown,
        },
        PolicyConditionNode::Comparison {
            left,
            operator,
            right,
        } => match (
            evaluate_metric_numeric_node(left, evidence)?,
            evaluate_metric_numeric_node(right, evidence)?,
        ) {
            (Some(left), Some(right)) => {
                if compare_policy_values(left, right, *operator) {
                    ExpressionTruth::True
                } else {
                    ExpressionTruth::False
                }
            }
            _ => ExpressionTruth::Unknown,
        },
    })
}

fn evaluate_metric_numeric_node(node: &PolicyNumericNode, evidence: &Value) -> Result<Option<f64>> {
    let value = match node {
        PolicyNumericNode::Number(value) => Some(*value),
        PolicyNumericNode::Identifier(identifier) => metric_evidence_number(evidence, identifier),
        PolicyNumericNode::Unary { operator, operand } => {
            evaluate_metric_numeric_node(operand, evidence)?.map(|value| match operator {
                ArithmeticOperator::UnaryPlus => value,
                ArithmeticOperator::UnaryMinus => -value,
                _ => value,
            })
        }
        PolicyNumericNode::Binary {
            left,
            operator,
            right,
        } => match (
            evaluate_metric_numeric_node(left, evidence)?,
            evaluate_metric_numeric_node(right, evidence)?,
        ) {
            (Some(left), Some(right)) => {
                let result = match operator {
                    ArithmeticOperator::Add => left + right,
                    ArithmeticOperator::Subtract => left - right,
                    ArithmeticOperator::Multiply => left * right,
                    ArithmeticOperator::Divide => {
                        anyhow::ensure!(right != 0.0, "condition division by zero");
                        left / right
                    }
                    ArithmeticOperator::UnaryPlus | ArithmeticOperator::UnaryMinus => {
                        anyhow::bail!("invalid unary operator placement")
                    }
                };
                anyhow::ensure!(
                    result.is_finite(),
                    "condition numeric result must be finite"
                );
                Some(result)
            }
            _ => None,
        },
    };
    Ok(value)
}

fn metric_evidence_number(evidence: &Value, path: &str) -> Option<f64> {
    path.split('.')
        .try_fold(evidence, |value, segment| value.get(segment))?
        .as_f64()
        .filter(|value| value.is_finite())
}

fn policy_rule_category(rule: &PolicyRuleRequest) -> String {
    rule.category.clone()
}

fn policy_rule_kind_storage(kind: AlertPolicyRuleKind) -> &'static str {
    match kind {
        AlertPolicyRuleKind::Metric => "metric",
        AlertPolicyRuleKind::State => "state",
        AlertPolicyRuleKind::Occurrence => "occurrence",
    }
}

fn policy_correlation_mode_storage(mode: AlertPolicyCorrelationMode) -> &'static str {
    match mode {
        AlertPolicyCorrelationMode::NaturalKey => "natural_key",
        AlertPolicyCorrelationMode::Subject => "subject",
        AlertPolicyCorrelationMode::Global => "global",
    }
}

fn trigger_sustained_seconds(condition: &Option<AlertPolicyMetaCondition>) -> Option<i64> {
    match condition {
        Some(AlertPolicyMetaCondition::Sustained { seconds }) => Some(*seconds),
        _ => None,
    }
}

fn normalized_meta_condition_payload(condition: Option<&AlertPolicyMetaCondition>) -> Value {
    match condition {
        None | Some(AlertPolicyMetaCondition::Immediate) => {
            json!({"kind": "immediate", "window_seconds": 0})
        }
        Some(AlertPolicyMetaCondition::Sustained { seconds }) => {
            json!({"kind": "sustained", "window_seconds": seconds})
        }
        Some(AlertPolicyMetaCondition::Count {
            confirmations,
            within_seconds,
        }) => json!({
            "kind": "count",
            "confirmations": confirmations,
            "window_seconds": within_seconds,
        }),
        Some(AlertPolicyMetaCondition::ElapsedSinceTrigger { seconds }) => {
            json!({"kind": "elapsed_since_trigger", "window_seconds": seconds})
        }
    }
}

fn evaluate_policy_condition(
    expression: &PolicyConditionExpression,
    traffic: Option<&TrafficAccountingRecord>,
    rollup: Option<&TelemetryRollupView>,
    incomplete: &mut Vec<String>,
) -> Result<ConditionEvaluation> {
    let mut first_pair = None;
    let truth = evaluate_condition_node(
        &expression.root,
        traffic,
        rollup,
        incomplete,
        &mut first_pair,
    )?;
    let (actual_value, threshold_value) = first_pair.unwrap_or((None, None));
    Ok(ConditionEvaluation {
        truth,
        actual_value,
        threshold_value,
    })
}

fn evaluate_condition_node(
    node: &PolicyConditionNode,
    traffic: Option<&TrafficAccountingRecord>,
    rollup: Option<&TelemetryRollupView>,
    incomplete: &mut Vec<String>,
    first_pair: &mut Option<(Option<f64>, Option<f64>)>,
) -> Result<ExpressionTruth> {
    match node {
        PolicyConditionNode::Not(inner) => Ok(
            match evaluate_condition_node(inner, traffic, rollup, incomplete, first_pair)? {
                ExpressionTruth::True => ExpressionTruth::False,
                ExpressionTruth::False => ExpressionTruth::True,
                ExpressionTruth::Unknown => ExpressionTruth::Unknown,
            },
        ),
        PolicyConditionNode::And(left, right) => {
            let left_value =
                evaluate_condition_node(left, traffic, rollup, incomplete, first_pair)?;
            let right_value =
                evaluate_condition_node(right, traffic, rollup, incomplete, first_pair)?;
            Ok(match (left_value, right_value) {
                (ExpressionTruth::False, _) | (_, ExpressionTruth::False) => ExpressionTruth::False,
                (ExpressionTruth::True, ExpressionTruth::True) => ExpressionTruth::True,
                _ => ExpressionTruth::Unknown,
            })
        }
        PolicyConditionNode::Or(left, right) => {
            let left_value =
                evaluate_condition_node(left, traffic, rollup, incomplete, first_pair)?;
            let right_value =
                evaluate_condition_node(right, traffic, rollup, incomplete, first_pair)?;
            Ok(match (left_value, right_value) {
                (ExpressionTruth::True, _) | (_, ExpressionTruth::True) => ExpressionTruth::True,
                (ExpressionTruth::False, ExpressionTruth::False) => ExpressionTruth::False,
                _ => ExpressionTruth::Unknown,
            })
        }
        PolicyConditionNode::Comparison {
            left,
            operator,
            right,
        } => {
            let left_value = evaluate_numeric_node(left, traffic, rollup, incomplete)?;
            let right_value = evaluate_numeric_node(right, traffic, rollup, incomplete)?;
            if first_pair.is_none() {
                *first_pair = Some((left_value, right_value));
            }
            Ok(left_value
                .zip(right_value)
                .map(|(left, right)| {
                    if compare_policy_values(left, right, *operator) {
                        ExpressionTruth::True
                    } else {
                        ExpressionTruth::False
                    }
                })
                .unwrap_or(ExpressionTruth::Unknown))
        }
    }
}

fn evaluate_numeric_node(
    node: &PolicyNumericNode,
    traffic: Option<&TrafficAccountingRecord>,
    rollup: Option<&TelemetryRollupView>,
    incomplete: &mut Vec<String>,
) -> Result<Option<f64>> {
    let value = match node {
        PolicyNumericNode::Number(value) => Some(*value),
        PolicyNumericNode::Identifier(identifier) => {
            policy_identifier_value(identifier, traffic, rollup, incomplete)
        }
        PolicyNumericNode::Unary { operator, operand } => {
            let value = evaluate_numeric_node(operand, traffic, rollup, incomplete)?;
            value.map(|value| match operator {
                ArithmeticOperator::UnaryPlus => value,
                ArithmeticOperator::UnaryMinus => -value,
                _ => value,
            })
        }
        PolicyNumericNode::Binary {
            left,
            operator,
            right,
        } => {
            let left = evaluate_numeric_node(left, traffic, rollup, incomplete)?;
            let right = evaluate_numeric_node(right, traffic, rollup, incomplete)?;
            match left.zip(right) {
                Some((left, right)) => {
                    let result = match operator {
                        ArithmeticOperator::Add => left + right,
                        ArithmeticOperator::Subtract => left - right,
                        ArithmeticOperator::Multiply => left * right,
                        ArithmeticOperator::Divide => {
                            anyhow::ensure!(right != 0.0, "condition division by zero");
                            left / right
                        }
                        ArithmeticOperator::UnaryPlus | ArithmeticOperator::UnaryMinus => {
                            anyhow::bail!("invalid unary operator placement")
                        }
                    };
                    anyhow::ensure!(
                        result.is_finite(),
                        "condition numeric result must be finite"
                    );
                    Some(result)
                }
                None => None,
            }
        }
    };
    Ok(value)
}

impl PolicyConditionParser {
    fn parse_or(&mut self) -> Result<PolicyConditionNode> {
        let mut node = self.parse_and()?;
        while matches!(self.peek(), Some(PolicyConditionToken::Or)) {
            self.position += 1;
            let right = self.parse_and()?;
            node = PolicyConditionNode::Or(Box::new(node), Box::new(right));
        }
        Ok(node)
    }

    fn parse_and(&mut self) -> Result<PolicyConditionNode> {
        let mut node = self.parse_not()?;
        while matches!(self.peek(), Some(PolicyConditionToken::And)) {
            self.position += 1;
            let right = self.parse_not()?;
            node = PolicyConditionNode::And(Box::new(node), Box::new(right));
        }
        Ok(node)
    }

    fn parse_not(&mut self) -> Result<PolicyConditionNode> {
        if matches!(self.peek(), Some(PolicyConditionToken::Not)) {
            self.position += 1;
            return Ok(PolicyConditionNode::Not(Box::new(self.parse_not()?)));
        }
        self.parse_boolean_primary()
    }

    fn parse_boolean_primary(&mut self) -> Result<PolicyConditionNode> {
        let snapshot = self.clone();
        if let Ok(comparison) = self.parse_comparison() {
            return Ok(comparison);
        }
        *self = snapshot;
        if matches!(self.peek(), Some(PolicyConditionToken::LeftParen)) {
            self.position += 1;
            let node = self.parse_or()?;
            self.expect_right_paren()?;
            return Ok(node);
        }
        anyhow::bail!("condition expression must compare numeric expressions")
    }

    fn parse_comparison(&mut self) -> Result<PolicyConditionNode> {
        let left = self.parse_numeric_expression()?;
        let operator = match self.next() {
            Some(PolicyConditionToken::Comparison(operator)) => operator,
            _ => anyhow::bail!("condition comparison operator is required"),
        };
        let right = self.parse_numeric_expression()?;
        Ok(PolicyConditionNode::Comparison {
            left,
            operator,
            right,
        })
    }

    fn parse_numeric_expression(&mut self) -> Result<PolicyNumericNode> {
        let mut node = self.parse_numeric_term()?;
        loop {
            let operator = match self.peek() {
                Some(PolicyConditionToken::Arithmetic(ArithmeticOperator::Add)) => {
                    ArithmeticOperator::Add
                }
                Some(PolicyConditionToken::Arithmetic(ArithmeticOperator::Subtract)) => {
                    ArithmeticOperator::Subtract
                }
                _ => break,
            };
            self.position += 1;
            let right = self.parse_numeric_term()?;
            node = PolicyNumericNode::Binary {
                left: Box::new(node),
                operator,
                right: Box::new(right),
            };
        }
        Ok(node)
    }

    fn parse_numeric_term(&mut self) -> Result<PolicyNumericNode> {
        let mut node = self.parse_numeric_factor()?;
        loop {
            let operator = match self.peek() {
                Some(PolicyConditionToken::Arithmetic(ArithmeticOperator::Multiply)) => {
                    ArithmeticOperator::Multiply
                }
                Some(PolicyConditionToken::Arithmetic(ArithmeticOperator::Divide)) => {
                    ArithmeticOperator::Divide
                }
                _ => break,
            };
            self.position += 1;
            let right = self.parse_numeric_factor()?;
            node = PolicyNumericNode::Binary {
                left: Box::new(node),
                operator,
                right: Box::new(right),
            };
        }
        Ok(node)
    }

    fn parse_numeric_factor(&mut self) -> Result<PolicyNumericNode> {
        match self.next() {
            Some(PolicyConditionToken::Number(value)) => Ok(PolicyNumericNode::Number(value)),
            Some(PolicyConditionToken::Identifier(identifier)) => {
                Ok(PolicyNumericNode::Identifier(identifier))
            }
            Some(PolicyConditionToken::Arithmetic(ArithmeticOperator::Add)) => {
                Ok(PolicyNumericNode::Unary {
                    operator: ArithmeticOperator::UnaryPlus,
                    operand: Box::new(self.parse_numeric_factor()?),
                })
            }
            Some(PolicyConditionToken::Arithmetic(ArithmeticOperator::Subtract)) => {
                Ok(PolicyNumericNode::Unary {
                    operator: ArithmeticOperator::UnaryMinus,
                    operand: Box::new(self.parse_numeric_factor()?),
                })
            }
            Some(PolicyConditionToken::LeftParen) => {
                let node = self.parse_numeric_expression()?;
                self.expect_right_paren()?;
                Ok(node)
            }
            _ => anyhow::bail!("numeric expression operand is required"),
        }
    }

    fn expect_right_paren(&mut self) -> Result<()> {
        match self.next() {
            Some(PolicyConditionToken::RightParen) => Ok(()),
            _ => anyhow::bail!("condition expression has unmatched '('"),
        }
    }

    fn peek(&self) -> Option<&PolicyConditionToken> {
        self.tokens.get(self.position)
    }

    fn next(&mut self) -> Option<PolicyConditionToken> {
        let token = self.tokens.get(self.position).cloned();
        if token.is_some() {
            self.position += 1;
        }
        token
    }
}

fn tokenize_policy_condition(expression: &str) -> Result<Vec<PolicyConditionToken>> {
    let input = expression.trim();
    anyhow::ensure!(!input.is_empty(), "condition expression is empty");
    let chars = input.char_indices().collect::<Vec<_>>();
    let mut tokens = Vec::new();
    let mut index = 0_usize;
    while index < chars.len() {
        let (byte_index, ch) = chars[index];
        if ch.is_whitespace() {
            index += 1;
            continue;
        }
        match ch {
            '(' => {
                tokens.push(PolicyConditionToken::LeftParen);
                index += 1;
            }
            ')' => {
                tokens.push(PolicyConditionToken::RightParen);
                index += 1;
            }
            '&' => {
                anyhow::ensure!(
                    chars.get(index + 1).is_some_and(|(_, next)| *next == '&'),
                    "condition '&' must be written as &&"
                );
                tokens.push(PolicyConditionToken::And);
                index += 2;
            }
            '|' => {
                anyhow::ensure!(
                    chars.get(index + 1).is_some_and(|(_, next)| *next == '|'),
                    "condition '|' must be written as ||"
                );
                tokens.push(PolicyConditionToken::Or);
                index += 2;
            }
            '!' => {
                if chars.get(index + 1).is_some_and(|(_, next)| *next == '=') {
                    tokens.push(PolicyConditionToken::Comparison(
                        PolicyComparisonOperator::NotEq,
                    ));
                    index += 2;
                } else {
                    tokens.push(PolicyConditionToken::Not);
                    index += 1;
                }
            }
            '~' => {
                tokens.push(PolicyConditionToken::Not);
                index += 1;
            }
            '>' | '<' | '=' => {
                let next_is_equal = chars.get(index + 1).is_some_and(|(_, next)| *next == '=');
                let operator = match (ch, next_is_equal) {
                    ('>', true) => PolicyComparisonOperator::Gte,
                    ('>', false) => PolicyComparisonOperator::Gt,
                    ('<', true) => PolicyComparisonOperator::Lte,
                    ('<', false) => PolicyComparisonOperator::Lt,
                    ('=', true) | ('=', false) => PolicyComparisonOperator::Eq,
                    _ => unreachable!("comparison branch only handles comparison tokens"),
                };
                tokens.push(PolicyConditionToken::Comparison(operator));
                index += if next_is_equal { 2 } else { 1 };
            }
            '+' | '-' | '*' | '/' => {
                let operator = match ch {
                    '+' => ArithmeticOperator::Add,
                    '-' => ArithmeticOperator::Subtract,
                    '*' => ArithmeticOperator::Multiply,
                    '/' => ArithmeticOperator::Divide,
                    _ => unreachable!("arithmetic branch only handles arithmetic tokens"),
                };
                tokens.push(PolicyConditionToken::Arithmetic(operator));
                index += 1;
            }
            _ if ch.is_ascii_digit() || ch == '.' => {
                let start = byte_index;
                let mut end = byte_index + ch.len_utf8();
                index += 1;
                while index < chars.len() {
                    let (next_index, next_ch) = chars[index];
                    if next_ch.is_ascii_alphanumeric() || matches!(next_ch, '.' | '_') {
                        end = next_index + next_ch.len_utf8();
                        index += 1;
                    } else {
                        break;
                    }
                }
                let raw = &input[start..end];
                tokens.push(PolicyConditionToken::Number(parse_policy_number(raw)?));
            }
            _ if is_policy_identifier_start(ch) => {
                let start = byte_index;
                let mut end = byte_index + ch.len_utf8();
                index += 1;
                while index < chars.len() {
                    let (next_index, next_ch) = chars[index];
                    if is_policy_identifier_continue(next_ch) {
                        end = next_index + next_ch.len_utf8();
                        index += 1;
                    } else {
                        break;
                    }
                }
                let identifier = input[start..end].to_string();
                match identifier.to_ascii_lowercase().as_str() {
                    "and" => tokens.push(PolicyConditionToken::And),
                    "or" => tokens.push(PolicyConditionToken::Or),
                    "not" => tokens.push(PolicyConditionToken::Not),
                    _ => {
                        validate_policy_identifier(&identifier)?;
                        tokens.push(PolicyConditionToken::Identifier(identifier));
                    }
                }
            }
            _ => anyhow::bail!("unsupported condition expression character: {ch}"),
        }
    }
    Ok(tokens)
}

fn parse_policy_number(raw: &str) -> Result<f64> {
    if raw.chars().any(|ch| ch.is_ascii_alphabetic()) {
        return Ok(parse_byte_size(raw)? as f64);
    }
    let value = raw
        .parse::<f64>()
        .with_context(|| format!("number literal {raw} is invalid"))?;
    anyhow::ensure!(value.is_finite(), "number literal must be finite");
    Ok(value)
}

fn policy_identifier_value(
    identifier: &str,
    traffic: Option<&TrafficAccountingRecord>,
    rollup: Option<&TelemetryRollupView>,
    incomplete: &mut Vec<String>,
) -> Option<f64> {
    if identifier.starts_with("traffic.") {
        let Some(traffic) = traffic else {
            push_incomplete(incomplete, "traffic accounting missing");
            return None;
        };
        if traffic.state != "ok" {
            for reason in &traffic.incomplete_reasons {
                push_incomplete(incomplete, reason);
            }
            if traffic.incomplete_reasons.is_empty() {
                push_incomplete(
                    incomplete,
                    format!("traffic accounting state is {}", traffic.state),
                );
            }
            return None;
        }
    }
    let value = match identifier {
        "traffic.quota.total" => traffic
            .and_then(|traffic| traffic.quota_total_bytes)
            .map(|value| value as f64),
        "traffic.quota.rx" => traffic
            .and_then(|traffic| traffic.quota_rx_bytes)
            .map(|value| value as f64),
        "traffic.quota.tx" => traffic
            .and_then(|traffic| traffic.quota_tx_bytes)
            .map(|value| value as f64),
        "traffic.cycle.total" => traffic.map(|traffic| traffic.total_bytes as f64),
        "traffic.cycle.rx" => traffic.map(|traffic| traffic.rx_bytes as f64),
        "traffic.cycle.tx" => traffic.map(|traffic| traffic.tx_bytes as f64),
        "traffic.cycle_percent" => traffic.and_then(|traffic| traffic.cycle_percent),
        "cpu.utilization_ratio" => rollup.and_then(|rollup| rollup.cpu_usage_max),
        "cpu.load_1" => rollup.map(|rollup| rollup.cpu_load_1_max),
        "cpu.load_saturation" => rollup.and_then(|rollup| {
            (rollup.cpu_cores_max > 0)
                .then(|| rollup.cpu_load_1_max / f64::from(rollup.cpu_cores_max))
        }),
        "memory.available_ratio" => rollup.and_then(|rollup| {
            (rollup.memory_total_bytes_max > 0)
                .then(|| (1.0 - rollup.memory_used_ratio_max).clamp(0.0, 1.0))
        }),
        "disk.available_ratio" => rollup.and_then(|rollup| {
            (rollup.disk_sample_count > 0 && rollup.disk_total_bytes_max > 0)
                .then(|| (1.0 - rollup.disk_used_ratio_max).clamp(0.0, 1.0))
        }),
        _ => None,
    };
    if value.is_none() {
        push_incomplete(incomplete, format!("{identifier} missing"));
    }
    value
}

fn validate_policy_identifier(identifier: &str) -> Result<()> {
    anyhow::ensure!(
        matches!(
            identifier,
            "traffic.quota.total"
                | "traffic.quota.rx"
                | "traffic.quota.tx"
                | "traffic.cycle.total"
                | "traffic.cycle.rx"
                | "traffic.cycle.tx"
                | "traffic.cycle_percent"
                | "cpu.utilization_ratio"
                | "cpu.load_1"
                | "cpu.load_saturation"
                | "memory.available_ratio"
                | "disk.available_ratio"
        ),
        "unsupported condition variable: {identifier}"
    );
    Ok(())
}

fn is_policy_identifier_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_policy_identifier_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_')
}

fn compare_policy_values(left: f64, right: f64, operator: PolicyComparisonOperator) -> bool {
    match operator {
        PolicyComparisonOperator::Eq => (left - right).abs() < f64::EPSILON,
        PolicyComparisonOperator::NotEq => (left - right).abs() >= f64::EPSILON,
        PolicyComparisonOperator::Lt => left < right,
        PolicyComparisonOperator::Lte => left <= right,
        PolicyComparisonOperator::Gt => left > right,
        PolicyComparisonOperator::Gte => left >= right,
    }
}

fn condition_node_uses_traffic(node: &PolicyConditionNode) -> bool {
    match node {
        PolicyConditionNode::Not(inner) => condition_node_uses_traffic(inner),
        PolicyConditionNode::And(left, right) | PolicyConditionNode::Or(left, right) => {
            condition_node_uses_traffic(left) || condition_node_uses_traffic(right)
        }
        PolicyConditionNode::Comparison { left, right, .. } => {
            numeric_node_uses_traffic(left) || numeric_node_uses_traffic(right)
        }
    }
}

fn numeric_node_uses_traffic(node: &PolicyNumericNode) -> bool {
    match node {
        PolicyNumericNode::Identifier(identifier) => identifier.starts_with("traffic."),
        PolicyNumericNode::Number(_) => false,
        PolicyNumericNode::Unary { operand, .. } => numeric_node_uses_traffic(operand),
        PolicyNumericNode::Binary { left, right, .. } => {
            numeric_node_uses_traffic(left) || numeric_node_uses_traffic(right)
        }
    }
}

fn push_incomplete(reasons: &mut Vec<String>, reason: impl AsRef<str>) {
    let reason = reason.as_ref();
    if !reasons.iter().any(|stored| stored == reason) {
        reasons.push(reason.to_string());
    }
}

fn latest_rollups(rollups: Vec<TelemetryRollupView>) -> HashMap<String, TelemetryRollupView> {
    let mut latest = HashMap::new();
    for rollup in rollups {
        let replace = latest
            .get(&rollup.client_id)
            .map(|stored: &TelemetryRollupView| rollup.bucket_start > stored.bucket_start)
            .unwrap_or(true);
        if replace {
            latest.insert(rollup.client_id.clone(), rollup);
        }
    }
    latest
}

fn preview_hash(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    let digest = Sha256::digest(bytes);
    format!("sha256:{}", hex::encode(digest))
}

fn selector_hash(selectors: &[String]) -> String {
    let digest = Sha256::digest(selectors.join(",").as_bytes());
    hex::encode(&digest[..16])
}

fn canonicalize_vps_rule_record(mut record: VpsRuleValueRecord) -> Result<VpsRuleValueRecord> {
    let stored_value_raw = record
        .stored_value_raw
        .clone()
        .unwrap_or_else(|| record.value_raw.clone());
    let parsed = parse_persisted_vps_rule_value(&record.key, &stored_value_raw)?;
    if record.key == VPS_RULE_KEY_NETWORK_RATE_INTERFACES {
        anyhow::ensure!(
            parsed.json == record.value_json,
            "network_rate_selector_storage_invalid"
        );
    } else if record.key == VPS_RULE_KEY_TRAFFIC_SELECTORS {
        anyhow::ensure!(
            parsed.json == record.value_json,
            "traffic_selector_storage_invalid"
        );
    }
    record.value_raw = parsed.raw;
    record.stored_value_raw = Some(stored_value_raw);
    record.value_json = parsed.json;
    record.parsed_display = parsed.display;
    Ok(record)
}

fn vps_rule_from_row(row: sqlx::postgres::PgRow) -> Result<VpsRuleValueRecord> {
    let key: String = row.try_get("key")?;
    let raw: String = row.try_get("value_raw")?;
    let stored_json = row.try_get::<SqlJson<Value>, _>("value_json")?.0;
    let parsed = parse_persisted_vps_rule_value(&key, &raw)?;
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
    Ok(VpsRuleValueRecord {
        client_id: row.try_get("client_id")?,
        key,
        value_raw: parsed.raw,
        stored_value_raw: Some(raw),
        value_json: parsed.json,
        parsed_display: parsed.display,
        state: "ok".to_string(),
        validation_errors: Vec::new(),
        source_kind: row.try_get("source_kind")?,
        source_id: row.try_get("source_id")?,
        updated_by: row.try_get("updated_by")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn policy_group_from_row(
    row: sqlx::postgres::PgRow,
    rules: Vec<PolicyRuleRecord>,
) -> Result<PolicyGroupRecord> {
    Ok(PolicyGroupRecord {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        enabled: row.try_get("enabled")?,
        selector_expression: row.try_get("selector_expression")?,
        notes: row.try_get("notes")?,
        matched_vps_count: 0,
        rule_count: rules.len() as i64,
        enabled_rule_count: rules.iter().filter(|rule| rule.enabled).count() as i64,
        active_info_count: 0,
        active_warning_count: 0,
        active_critical_count: 0,
        incomplete_vps_count: 0,
        last_evaluated_at: None,
        rules,
        created_by: row.try_get("created_by")?,
        updated_by: row.try_get("updated_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn policy_rule_from_row(row: sqlx::postgres::PgRow) -> Result<PolicyRuleRecord> {
    let rule_kind = match row.try_get::<String, _>("rule_kind")?.as_str() {
        "metric" => AlertPolicyRuleKind::Metric,
        "state" => AlertPolicyRuleKind::State,
        "occurrence" => AlertPolicyRuleKind::Occurrence,
        value => anyhow::bail!("invalid alert policy rule kind: {value}"),
    };
    let correlation_mode = match row.try_get::<String, _>("correlation_mode")?.as_str() {
        "natural_key" => AlertPolicyCorrelationMode::NaturalKey,
        "subject" => AlertPolicyCorrelationMode::Subject,
        "global" => AlertPolicyCorrelationMode::Global,
        value => anyhow::bail!("invalid alert policy correlation mode: {value}"),
    };
    Ok(PolicyRuleRecord {
        id: row.try_get("id")?,
        group_id: row.try_get("group_id")?,
        rule_version: row.try_get("rule_version")?,
        sort_order: row.try_get("sort_order")?,
        name: row.try_get("name")?,
        enabled: row.try_get("enabled")?,
        rule_kind,
        evidence_source: row.try_get("evidence_source")?,
        correlation_mode,
        traffic_selector: row.try_get("traffic_selector")?,
        trigger_condition_expression: row.try_get("trigger_condition_expression")?,
        trigger_meta_condition: row
            .try_get::<Option<SqlJson<AlertPolicyMetaCondition>>, _>("trigger_meta_condition")?
            .map(|value| value.0),
        resolve_condition_expression: row.try_get("resolve_condition_expression")?,
        resolve_meta_condition: row
            .try_get::<Option<SqlJson<AlertPolicyMetaCondition>>, _>("resolve_meta_condition")?
            .map(|value| value.0),
        severity: row.try_get("severity")?,
        category: row.try_get("category")?,
        title_template: row.try_get("title_template")?,
        detail_template: row.try_get("detail_template")?,
        system_seed_key: row.try_get("system_seed_key")?,
        armed_after_evidence_seq: row.try_get("armed_after_evidence_seq")?,
        armed_at: row.try_get("armed_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn policy_rule_state_from_row(row: sqlx::postgres::PgRow) -> Result<PolicyRuleStateRecord> {
    Ok(PolicyRuleStateRecord {
        policy_rule_id: row.try_get("policy_rule_id")?,
        client_id: row.try_get("client_id")?,
        rule_version: row.try_get("rule_version")?,
        condition_true: row.try_get("condition_true")?,
        previous_condition_true: row.try_get("previous_condition_true")?,
        window_satisfied: row.try_get("window_satisfied")?,
        first_true_at: row.try_get("first_true_at")?,
        last_true_at: row.try_get("last_true_at")?,
        last_false_at: row.try_get("last_false_at")?,
        last_evaluated_at: row.try_get("last_evaluated_at")?,
        incomplete: row.try_get("incomplete")?,
        incomplete_reasons: row.try_get("incomplete_reasons")?,
        last_actual_value: row.try_get("last_actual_value")?,
        last_threshold_value: row.try_get("last_threshold_value")?,
        last_fired_at: row.try_get("last_fired_at")?,
        trigger_generation: row.try_get("trigger_generation")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn policy_alert_from_row(row: sqlx::postgres::PgRow) -> Result<PolicyAlertRecord> {
    Ok(PolicyAlertRecord {
        id: row.try_get("id")?,
        policy_group_id: row.try_get("policy_group_id")?,
        policy_rule_id: row.try_get("policy_rule_id")?,
        client_id: row.try_get("client_id")?,
        trigger_generation: row.try_get("trigger_generation")?,
        severity: row.try_get("severity")?,
        category: row.try_get("category")?,
        title: row.try_get("title")?,
        detail: row.try_get("detail")?,
        actual_value: row.try_get("actual_value")?,
        threshold_value: row.try_get("threshold_value")?,
        payload: row.try_get::<SqlJson<Value>, _>("payload")?.0,
        lifecycle_state: row.try_get("lifecycle_state")?,
        last_confirmed_at: row.try_get("last_confirmed_at")?,
        resolved_at: row.try_get("resolved_at")?,
        resolution_reason: row.try_get("resolution_reason")?,
        observed_at: row.try_get("observed_at")?,
        created_at: row.try_get("created_at")?,
    })
}

#[allow(clippy::too_many_arguments)]
async fn list_unified_policy_alerts_postgres(
    pool: &sqlx::PgPool,
    query: &PolicyAlertQuery,
    result_limit: Option<usize>,
    prioritize_severity: bool,
    selection_mode: PolicyAlertSelectionMode,
    allowed_client_ids: Option<&[String]>,
    start_unix: Option<u64>,
    end_unix: Option<u64>,
    operator_state: Option<&str>,
    include_muted: bool,
    notification_rules: Option<&[FleetAlertNotificationMatchRule]>,
) -> Result<Vec<PolicyAlertRecord>> {
    let rows = sqlx::query(
        r#"
        SELECT
            e.id, e.policy_group_id, e.policy_rule_id, e.client_id,
            e.trigger_generation, e.severity, e.category, e.title, e.detail,
            NULLIF(e.evidence #>> '{source,actual_value}', '')::double precision
                AS actual_value,
            NULLIF(e.evidence #>> '{source,threshold_value}', '')::double precision
                AS threshold_value,
            e.evidence AS payload, e.lifecycle_state,
            e.last_confirmed_at::text AS last_confirmed_at,
            e.resolved_at::text AS resolved_at, e.resolution_reason,
            COALESCE(e.last_confirmed_at,e.triggered_at)::text AS observed_at,
            e.created_at::text AS created_at
        FROM alert_episodes e
        LEFT JOIN fleet_alert_states triage ON triage.alert_id=e.public_id
        WHERE e.policy_rule_id IS NOT NULL AND e.client_id IS NOT NULL
          AND ($2::text IS NULL OR e.client_id=$2)
          AND ($14::boolean OR EXISTS (
              SELECT 1 FROM visible_clients visible WHERE visible.id=e.client_id
          ))
          AND ($3::text IS NULL OR e.severity=$3)
          AND ($4::text IS NULL OR e.category=$4)
          AND ($5::uuid IS NULL OR e.policy_group_id=$5)
          AND ($6::text[] IS NULL OR e.client_id=ANY($6))
          AND ($7::double precision IS NULL OR e.triggered_at>=to_timestamp($7))
          AND ($8::double precision IS NULL OR e.triggered_at<=to_timestamp($8))
          AND ($14::boolean OR (
              e.resolved_at IS NULL AND e.last_confirmed_at IS NOT NULL
          ))
          AND ($9::boolean=FALSE OR (
              e.lifecycle_state IN ('triggered','persisting')
              AND e.last_confirmed_at IS NOT NULL
          ))
          AND (
            $10::text IS NULL OR CASE
              WHEN triage.state='muted' AND triage.muted_until_unix IS NOT NULL
                   AND triage.muted_until_unix <= $12 THEN 'open'
              ELSE COALESCE(triage.state,'open') END = $10
          )
          AND (
            $11::boolean OR CASE
              WHEN triage.state='muted' AND triage.muted_until_unix IS NOT NULL
                   AND triage.muted_until_unix <= $12 THEN 'open'
              ELSE COALESCE(triage.state,'open') END <> 'muted'
          )
          AND (
            $13::jsonb IS NULL OR EXISTS (
              SELECT 1 FROM jsonb_array_elements($13::jsonb) rule
              WHERE CASE e.severity WHEN 'critical' THEN 0 WHEN 'warning' THEN 1
                        WHEN 'info' THEN 2 ELSE 3 END
                    <= (rule->>'min_severity_rank')::integer
                AND (jsonb_array_length(rule->'categories')=0
                     OR rule->'categories' ? e.category)
                AND (jsonb_array_length(rule->'operator_states')=0
                     OR rule->'operator_states' ? CASE
                       WHEN triage.state='muted' AND triage.muted_until_unix IS NOT NULL
                            AND triage.muted_until_unix <= $12 THEN 'open'
                       ELSE COALESCE(triage.state,'open') END)
                AND (rule->'client_ids'='null'::jsonb
                     OR rule->'client_ids' ? e.client_id)
            )
          )
        ORDER BY
          CASE WHEN $15::boolean AND e.lifecycle_state IN ('triggered','persisting')
               THEN 0 ELSE 1 END,
          CASE WHEN $15::boolean THEN CASE e.severity
               WHEN 'critical' THEN 0 WHEN 'warning' THEN 1
               WHEN 'info' THEN 2 ELSE 3 END ELSE 0 END,
          e.triggered_at DESC, e.id DESC
        LIMIT $1
        "#,
    )
    .bind(result_limit.map(|value| value as i64))
    .bind(query.client_id.as_deref())
    .bind(query.severity.as_deref())
    .bind(query.category.as_deref())
    .bind(query.policy_group_id)
    .bind(allowed_client_ids)
    .bind(start_unix.map(|value| value as f64))
    .bind(end_unix.map(|value| value as f64))
    .bind(selection_mode == PolicyAlertSelectionMode::ConfirmedActive)
    .bind(operator_state)
    .bind(include_muted)
    .bind(crate::unix_now() as i64)
    .bind(notification_rules.map(serde_json::to_value).transpose()?)
    .bind(selection_mode == PolicyAlertSelectionMode::History)
    .bind(prioritize_severity)
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(policy_alert_from_row).collect()
}

fn policy_group_audit(
    action: &str,
    policy: &PolicyGroupRecord,
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: action.to_string(),
        target: format!("fleet_alert_policy:{}", policy.id),
        command_hash: None,
        metadata: policy_group_metadata(policy, operator),
        created_at,
    }
}

fn policy_group_metadata(policy: &PolicyGroupRecord, operator: &AuthContext) -> Value {
    json!({
        "operator_id": operator.operator.id,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "operator_session_id": operator.audit_session_id(),
        "result": "succeeded",
        "origin_kind": "operator_request",
        "component": "alert-policy-controller",
        "policy": policy,
    })
}

fn vps_rules_audit(
    action: &str,
    preview: &VpsRulesDryRunResponse,
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: action.to_string(),
        target: "vps_rules".to_string(),
        command_hash: Some(preview.preview_hash.clone()),
        metadata: json!({
            "preview_hash": &preview.preview_hash,
            "matched_vps_count": preview.matched_vps_count,
            "changed_row_count": preview.changed_row_count,
            "result": "succeeded",
            "operator_id": operator.operator.id,
            "operator_username": &operator.operator.username,
            "operator_role": &operator.operator.role,
            "operator_session_id": operator.audit_session_id(),
            "origin_kind": "operator_request",
            "component": "vps-rules-controller",
        }),
        created_at,
    }
}

#[cfg(test)]
#[path = "tests_repository_alert_policies.rs"]
mod tests;
