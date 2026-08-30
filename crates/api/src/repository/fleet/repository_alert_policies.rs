use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    sync::OnceLock,
};

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{postgres::PgRow, types::Json as SqlJson, Row};
use tokio::sync::Notify;
use uuid::Uuid;
use vpsman_common::{
    expression_references_vps_rules, ordinal_admission_mask_has_exact_shape,
    parse_vps_rule_value as parse_common_vps_rule_value, projected_telemetry_tunnel_identity,
    AgentMetrics, Expression, ExpressionTruth, NetworkInterfacePolicy, NetworkInterfaceSource,
    ParsedVpsRuleValue, ProjectedTelemetryTunnelIdentity, VpsRuleContext,
};

use crate::{
    model::{AgentView, AuthContext},
    model_alert_notifications::FleetAlertNotificationMatchRule,
    model_alert_policies::{
        AlertPolicyCorrelationMode, AlertPolicyMetaCondition, AlertPolicyRuleKind,
        CreateFleetAlertPolicyRequest, NetworkRateInterfaceSelection, PolicyAlertQuery,
        PolicyAlertRecord, PolicyDryRunRequest, PolicyDryRunResponse, PolicyDryRunRulePreview,
        PolicyGroupRecord, PolicyRuleRecord, PolicyRuleRequest, PolicyRuleStateRecord,
        TrafficAccountingQuery, TrafficAccountingRecord, TrafficAccountingSelectorBreakdown,
        VpsRuleChangePreview, VpsRuleQuery, VpsRuleValueRecord, VpsRulesBulkUnsetRequest,
        VpsRulesBulkUpsertRequest, VpsRulesDryRunRequest, VpsRulesDryRunResponse,
        VPS_RULE_KEY_BILLING_CYCLE, VPS_RULE_KEY_BILLING_PRICE, VPS_RULE_KEY_NETWORK_INTERFACES,
        VPS_RULE_KEY_NETWORK_RATE_INTERFACES, VPS_RULE_KEY_TRAFFIC_QUOTA_RX,
        VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL, VPS_RULE_KEY_TRAFFIC_QUOTA_TX,
        VPS_RULE_KEY_TRAFFIC_RESET_DAY, VPS_RULE_KEY_TRAFFIC_SELECTORS,
    },
    model_monitoring::TrafficHistoryPointView,
    repository::Repository,
    repository_ingest::{
        admitted_network_interface, combined_metric_evidence_payload,
        reconstruct_projected_policy_traffic_in_tx,
    },
    repository_key_lifecycle::{
        lock_postgres_definition_lifecycles_in_tx, lock_postgres_definitions_and_clients_in_tx,
        require_visible_postgres_clients_in_tx,
    },
    repository_network_traffic_import::{
        is_vnstat_import_source, lock_postgres_traffic_counter_streams,
    },
    repository_telemetry_policy_activation::{
        mark_telemetry_policy_activation_may_be_pending,
        reconcile_telemetry_policy_activation_request_in_tx, wake_telemetry_policy_activation,
    },
    selector_expression::{
        agent_matches_selector_expression_with_rules, parse_selector_expression,
        vps_rule_contexts_by_client,
    },
    unix_now,
};

#[cfg(test)]
use crate::{
    model::TelemetryRollupView,
    model_alert_policies::{TrafficCounterRollupRecord, TrafficCounterSampleRecord},
    repository_network_traffic_import::is_intentional_vnstat_import_boundary,
    util::parse_timestamp_utc,
};

const MAX_POLICY_NAME_BYTES: usize = 128;
const MAX_POLICY_NOTES_BYTES: usize = 1024;
const MAX_RULE_NAME_BYTES: usize = 128;
const MAX_SELECTOR_EXPRESSION_BYTES: usize = 4096;
const MAX_CONDITION_EXPRESSION_BYTES: usize = 4096;
const MAX_POLICY_ALERT_CANDIDATE_ROWS: usize = 201;
// These values bound one transaction/scheduler page. They are not backlog
// throughput caps: the background evaluator immediately continues after a
// full page and applies its configured interval only once every page is short.
const POLICY_SCOPE_MAINTENANCE_PAGE: i64 = 200;
const POLICY_EVIDENCE_MAINTENANCE_PAGE: i64 = 500;
const POLICY_DUE_MAINTENANCE_PAGE: i64 = 200;
static POLICY_EVALUATOR_WAKE: OnceLock<Notify> = OnceLock::new();

pub(crate) fn wake_policy_evaluator() {
    POLICY_EVALUATOR_WAKE.get_or_init(Notify::new).notify_one();
}

pub(crate) async fn wait_for_policy_evaluator_wake() {
    POLICY_EVALUATOR_WAKE
        .get_or_init(Notify::new)
        .notified()
        .await;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PolicyEvaluationPage {
    scope_examined: usize,
    evidence_examined: usize,
    evidence_still_due: bool,
    due_examined: usize,
    due_transitioned: usize,
}

impl PolicyEvaluationPage {
    fn may_have_more(self) -> bool {
        self.scope_examined == POLICY_SCOPE_MAINTENANCE_PAGE as usize
            || self.evidence_examined == POLICY_EVIDENCE_MAINTENANCE_PAGE as usize
            || self.evidence_still_due
            || self.due_examined == POLICY_DUE_MAINTENANCE_PAGE as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PolicyAlertSelectionMode {
    History,
    CurrentFleet,
    ConfirmedActive,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct TrafficSelector {
    source: String,
    interface: String,
    direction: String,
    canonical: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TrafficSelectorSpec {
    All,
    Exact(Vec<TrafficSelector>),
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

pub(crate) type TrafficStreamIdentity = (String, String);

/// The current accepted sample's traffic counters. The traffic stream owner
/// persists natural-minute state asynchronously; policy evaluation overlays
/// this immutable event on the preceding durable stream snapshot so removing
/// per-event stream DML cannot delay traffic-trigger semantics by one sample.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectedTrafficCounterOverlay {
    pub(crate) observed_at: DateTime<Utc>,
    pub(crate) counters: Vec<ProjectedTrafficCounter>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectedTrafficCounter {
    pub(crate) source_kind: String,
    pub(crate) interface: String,
    pub(crate) rx_bytes: i64,
    pub(crate) tx_bytes: i64,
    pub(crate) sample_source: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct NetworkInterfaceInventory {
    traffic_streams: BTreeSet<TrafficStreamIdentity>,
    current_host_interfaces: BTreeSet<String>,
    current_tunnel_interfaces: BTreeSet<String>,
}

fn projected_traffic_streams_with_policy(
    metrics: &AgentMetrics,
    policy: &NetworkInterfacePolicy,
    network_admission_mask: &[u8],
    tunnel_admission_mask: &[u8],
    current_tunnel_identities: &HashSet<ProjectedTelemetryTunnelIdentity>,
    managed_tunnel_interfaces: &HashSet<String>,
) -> HashSet<TrafficStreamIdentity> {
    let network_mask_is_exact =
        ordinal_admission_mask_has_exact_shape(network_admission_mask, metrics.networks.len());
    let tunnel_mask_is_exact =
        ordinal_admission_mask_has_exact_shape(tunnel_admission_mask, metrics.tunnels.len());
    let host_streams = metrics
        .networks
        .iter()
        .enumerate()
        .filter(|(ordinal, network)| {
            network_mask_is_exact
                && (1..=64).contains(&network.interface.len())
                && ordinal_mask_bit(network_admission_mask, *ordinal)
                && admitted_network_interface(
                    policy,
                    NetworkInterfaceSource::Host,
                    &network.interface,
                    managed_tunnel_interfaces,
                )
        })
        .map(|(_, network)| ("host".to_string(), network.interface.clone()));
    let tunnel_streams = metrics
        .tunnels
        .iter()
        .enumerate()
        .filter(|(ordinal, tunnel)| {
            tunnel_mask_is_exact
                && ordinal_mask_bit(tunnel_admission_mask, *ordinal)
                && projected_telemetry_tunnel_identity(tunnel)
                    .is_some_and(|identity| current_tunnel_identities.contains(&identity))
                && admitted_network_interface(
                    policy,
                    NetworkInterfaceSource::Tunnel,
                    &tunnel.interface,
                    managed_tunnel_interfaces,
                )
        })
        .map(|(_, tunnel)| ("tunnel".to_string(), tunnel.interface.clone()));
    host_streams.chain(tunnel_streams).collect()
}

fn ordinal_mask_bit(mask: &[u8], ordinal: usize) -> bool {
    mask.get(ordinal / 8)
        .is_some_and(|byte| byte & (1_u8 << (ordinal % 8)) != 0)
}

#[cfg(test)]
fn projected_traffic_streams(metrics: &AgentMetrics) -> HashSet<TrafficStreamIdentity> {
    fn all_mask(item_count: usize) -> Vec<u8> {
        let mut mask = vec![0xff; item_count.div_ceil(8)];
        if let (Some(final_byte), remainder) = (mask.last_mut(), item_count % 8) {
            if remainder != 0 {
                *final_byte = ((1_u16 << remainder) - 1) as u8;
            }
        }
        mask
    }
    let network_mask = all_mask(metrics.networks.len());
    let tunnel_mask = all_mask(metrics.tunnels.len());
    let current_tunnel_identities = metrics
        .tunnels
        .iter()
        .filter_map(projected_telemetry_tunnel_identity)
        .collect::<HashSet<_>>();
    let managed_tunnel_interfaces = current_tunnel_identities
        .iter()
        .map(|identity| identity.interface.clone())
        .collect();
    projected_traffic_streams_with_policy(
        metrics,
        &NetworkInterfacePolicy::All,
        &network_mask,
        &tunnel_mask,
        &current_tunnel_identities,
        &managed_tunnel_interfaces,
    )
}

fn projected_traffic_stream_contains(
    streams: &HashSet<TrafficStreamIdentity>,
    source_kind: &str,
    interface: &str,
) -> bool {
    streams
        .iter()
        .any(|stream| stream.0 == source_kind && stream.1 == interface)
}

const NO_RESET_TRAFFIC_START_UNIX: i64 = 0;

// Current monthly cycles read one transactionally maintained completed-hour
// prefix and at most its single authoritative open hourly row. The stream head
// supplies the current counter values, so the ordinary path never revisits raw
// samples. A stream with no later samples remains a bounded last-known value as
// wall time moves; crossing a reset boundary gives it a zero prefix for the new
// cycle.
// Coverage is decided independently for every stream. Every reader omits an
// unready stream so the accounting assembler marks only that selector
// incomplete; repair remains exclusively writer-owned.
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
        SELECT
            requested.client_id,
            requested.source_kind,
            requested.interface,
            requested.cycle_start_unix,
            active.rx_bytes AS completed_cycle_rx,
            active.tx_bytes AS completed_cycle_tx,
            active.rx_reset_count AS completed_rx_resets,
            active.tx_reset_count AS completed_tx_resets,
            active.cycle_start AS active_cycle_start,
            streams.latest_sample_observed_at,
            streams.latest_sample_rx_bytes,
            streams.latest_sample_tx_bytes,
            streams.latest_sample_source,
            CASE
                WHEN active.cycle_start =
                        to_timestamp(requested.cycle_start_unix)
                THEN active.completed_through
                ELSE to_timestamp(requested.cycle_start_unix)
            END AS tail_start,
            to_timestamp(requested.cycle_start_unix) = date_bin(
                interval '1 hour',
                to_timestamp(requested.cycle_start_unix),
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
            )
            AND streams.client_id IS NOT NULL
            AND streams.source_revision = streams.materialized_revision
            AND streams.sample_edge_revision = streams.materialized_revision
            AND streams.promoted_boundary_safe
            AND active.client_id IS NOT NULL
            AND active.source_revision = active.materialized_revision
            AND (
                (
                    active.cycle_start =
                        to_timestamp(requested.cycle_start_unix)
                    AND active.completed_through <= date_bin(
                        interval '1 hour',
                        to_timestamp($5),
                        TIMESTAMPTZ '1970-01-01 00:00:00+00'
                    )
                    -- Each owner is self-ready; this head edge
                    -- distinguishes inactivity from an unpublished
                    -- completed prefix.
                    AND streams.latest_sample_observed_at <
                        active.completed_through + interval '1 hour'
                )
                OR (
                    active.cycle_start <
                        to_timestamp(requested.cycle_start_unix)
                    AND streams.latest_sample_observed_at <
                        to_timestamp(requested.cycle_start_unix)
                )
            ) AS valid
        FROM requested
        LEFT JOIN traffic_counter_streams streams
         ON streams.client_id = requested.client_id
         AND streams.source_kind = requested.source_kind
         AND streams.interface = requested.interface
        LEFT JOIN traffic_counter_active_cycle_usage active
          ON active.client_id = requested.client_id
         AND active.source_kind = requested.source_kind
         AND active.interface = requested.interface
    ),
    fast_requested AS MATERIALIZED (
        SELECT
            client_id, source_kind, interface, cycle_start_unix,
            CASE WHEN active_cycle_start = to_timestamp(cycle_start_unix)
                 THEN completed_cycle_rx ELSE 0 END AS completed_cycle_rx,
            CASE WHEN active_cycle_start = to_timestamp(cycle_start_unix)
                 THEN completed_cycle_tx ELSE 0 END AS completed_cycle_tx,
            CASE WHEN active_cycle_start = to_timestamp(cycle_start_unix)
                 THEN completed_rx_resets ELSE 0 END AS completed_rx_resets,
            CASE WHEN active_cycle_start = to_timestamp(cycle_start_unix)
                 THEN completed_tx_resets ELSE 0 END AS completed_tx_resets,
            tail_start,
            latest_sample_observed_at,
            latest_sample_rx_bytes,
            latest_sample_tx_bytes,
            latest_sample_source
        FROM coverage
        WHERE valid
    ),
    fast_usage AS (
        SELECT
            requested.client_id,
            requested.source_kind,
            requested.interface,
            requested.completed_cycle_rx
                + CASE WHEN requested.latest_sample_observed_at >=
                            requested.tail_start
                       THEN tail.rx_bytes ELSE 0 END AS cycle_rx,
            requested.completed_cycle_tx
                + CASE WHEN requested.latest_sample_observed_at >=
                            requested.tail_start
                       THEN tail.tx_bytes ELSE 0 END AS cycle_tx,
            requested.latest_sample_rx_bytes AS latest_rx,
            requested.latest_sample_tx_bytes AS latest_tx,
            requested.latest_sample_source AS last_sample_source,
            EXTRACT(EPOCH FROM requested.latest_sample_observed_at)::bigint
                AS last_sample_unix,
            1 + requested.completed_rx_resets
                + CASE WHEN requested.latest_sample_observed_at >=
                            requested.tail_start
                       THEN tail.rx_reset_count ELSE 0 END
                AS rx_counter_epochs_seen,
            1 + requested.completed_tx_resets
                + CASE WHEN requested.latest_sample_observed_at >=
                            requested.tail_start
                       THEN tail.tx_reset_count ELSE 0 END
                AS tx_counter_epochs_seen
        FROM fast_requested requested
        LEFT JOIN traffic_counter_hourly_usage tail
          ON tail.client_id = requested.client_id
         AND tail.source_kind = requested.source_kind
         AND tail.interface = requested.interface
         AND tail.bucket_start = requested.tail_start
        WHERE requested.latest_sample_observed_at <= to_timestamp($5)
          AND (
              requested.latest_sample_observed_at IS NULL
              OR
              requested.latest_sample_observed_at < requested.tail_start
              OR tail.latest_observed_at = requested.latest_sample_observed_at
          )
    )
    SELECT * FROM fast_usage
    ORDER BY client_id, source_kind, interface
"#;

// Long-term traffic is the sum of the ready exact-hour stream owner and
// its non-overlapping retained tier owners. An absent retained registry plus
// absent tier summaries and an empty full-key rollup probe is the normal
// zero-retained state for a new stream.
// Every unready or overlapping authority is omitted; readers never inspect raw
// samples or reconstruct writer-owned projections.
pub(crate) const NO_RESET_TRAFFIC_COUNTER_USAGE_SQL: &str = r#"
    WITH requested AS MATERIALIZED (
        SELECT client_id, source_kind, interface
        FROM UNNEST(
            $1::text[],
            $2::text[],
            $3::text[]
        ) AS request(client_id, source_kind, interface)
    ),
    retained_tiers AS MATERIALIZED (
        SELECT
            summary.*,
            stream.first_exact_observed_at,
            stream.last_exact_observed_at
        FROM requested
        JOIN traffic_counter_rollup_tier_summaries summary
          ON summary.client_id = requested.client_id
         AND summary.source_kind = requested.source_kind
         AND summary.interface = requested.interface
        JOIN traffic_counter_streams stream
          ON stream.client_id = requested.client_id
         AND stream.source_kind = requested.source_kind
         AND stream.interface = requested.interface
    ),
    retained_tier_order AS MATERIALIZED (
        SELECT
            tier.*,
            MIN(first_bucket_start) OVER finer AS finer_first_bucket_start,
            MAX(last_bucket_end) OVER finer AS finer_last_bucket_end
        FROM retained_tiers tier
        WINDOW finer AS (
            PARTITION BY client_id, source_kind, interface, origin_kind
            ORDER BY bucket_secs
            ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING
        )
    ),
    retained_totals AS MATERIALIZED (
        SELECT
            tier.client_id,
            tier.source_kind,
            tier.interface,
            COALESCE(SUM(tier.rx_bytes), 0)::bigint AS rx_bytes,
            COALESCE(SUM(tier.tx_bytes), 0)::bigint AS tx_bytes,
            COALESCE(SUM(tier.rx_reset_count), 0)::bigint
                AS rx_reset_count,
            COALESCE(SUM(tier.tx_reset_count), 0)::bigint
                AS tx_reset_count,
            COALESCE(SUM(tier.rollup_row_count), 0)::bigint
                AS rollup_row_count,
            COUNT(*)::integer AS tier_count,
            MIN(tier.materialized_revision) AS minimum_revision,
            MAX(tier.materialized_revision) AS maximum_revision,
            bool_or(
                (
                    tier.finer_first_bucket_start IS NOT NULL
                    AND tier.first_bucket_start < tier.finer_last_bucket_end
                    AND tier.last_bucket_end > tier.finer_first_bucket_start
                )
                OR tier.latest_bucket_start > to_timestamp($4)
                OR (
                    tier.first_exact_observed_at IS NOT NULL
                    AND tier.first_bucket_start <= tier.last_exact_observed_at
                    AND tier.last_bucket_end > tier.first_exact_observed_at
                )
            ) AS ineligible
        FROM retained_tier_order tier
        GROUP BY tier.client_id, tier.source_kind, tier.interface
    )
    SELECT
        requested.client_id,
        requested.source_kind,
        requested.interface,
        stream.usage_rx_bytes
            + COALESCE(tier.rx_bytes, 0) AS cycle_rx,
        stream.usage_tx_bytes
            + COALESCE(tier.tx_bytes, 0) AS cycle_tx,
        stream.latest_sample_rx_bytes AS latest_rx,
        stream.latest_sample_tx_bytes AS latest_tx,
        stream.latest_sample_source AS last_sample_source,
        EXTRACT(EPOCH FROM stream.latest_sample_observed_at)::bigint
            AS last_sample_unix,
        1 + stream.usage_rx_reset_count
            + COALESCE(tier.rx_reset_count, 0)
                AS rx_counter_epochs_seen,
        1 + stream.usage_tx_reset_count
            + COALESCE(tier.tx_reset_count, 0)
                AS tx_counter_epochs_seen
    FROM requested
    JOIN traffic_counter_streams stream
      ON stream.client_id = requested.client_id
     AND stream.source_kind = requested.source_kind
     AND stream.interface = requested.interface
    LEFT JOIN traffic_counter_rollup_summary_streams retained
      ON retained.client_id = requested.client_id
     AND retained.source_kind = requested.source_kind
     AND retained.interface = requested.interface
    LEFT JOIN retained_totals tier
      ON tier.client_id = requested.client_id
     AND tier.source_kind = requested.source_kind
     AND tier.interface = requested.interface
    WHERE stream.source_revision = stream.materialized_revision
      AND stream.sample_edge_revision = stream.materialized_revision
      AND stream.promoted_boundary_safe
      AND stream.latest_sample_observed_at IS NOT NULL
      AND stream.latest_sample_observed_at <= to_timestamp($4)
      AND (
            tier.client_id IS NOT NULL
            OR NOT EXISTS (
                SELECT 1
                FROM traffic_counter_rollups rollup
                WHERE rollup.client_id = requested.client_id
                  AND rollup.source_kind = requested.source_kind
                  AND rollup.interface = requested.interface
            )
      )
      AND (
            (
                retained.client_id IS NULL
                AND tier.client_id IS NULL
            )
            OR (
                retained.client_id IS NOT NULL
                AND retained.source_revision = retained.materialized_revision
                AND retained.tier_count = COALESCE(tier.tier_count, 0)
                AND retained.rollup_row_count =
                    COALESCE(tier.rollup_row_count, 0)
                AND NOT COALESCE(tier.ineligible, FALSE)
                AND (
                    tier.client_id IS NULL
                    OR (
                        tier.minimum_revision = retained.materialized_revision
                        AND tier.maximum_revision =
                            retained.materialized_revision
                    )
                )
            )
      )
    ORDER BY client_id ASC, source_kind ASC, interface ASC
"#;

// The all-history range needs only one raw edge and the compact retained-tier
// bounds for each selected traffic stream. Both sources are maintained in the
// same transaction as their authoritative rows, so this stays exact without
// walking a stream's complete retained history.
pub(crate) const TRAFFIC_HISTORY_START_SQL: &str = r#"
    WITH requested AS (
        SELECT source_kind, interface
        FROM UNNEST($2::text[], $3::text[])
            AS stream(source_kind, interface)
    ), bounded_starts AS (
        SELECT raw.first_observed_at AS first_at
        FROM requested
        LEFT JOIN LATERAL (
            SELECT sample.observed_at AS first_observed_at
            FROM traffic_counter_samples sample
            -- The two tuple bounds are the exact non-null primary-key prefix
            -- for this stream. Unlike timestamp filtering, the prefix range
            -- cannot walk unrelated streams before finding their first edge.
            WHERE (
                    sample.client_id,
                    sample.source_kind,
                    sample.interface,
                    sample.observed_at
                  ) >= (
                    $1,
                    requested.source_kind,
                    requested.interface,
                    '-infinity'::timestamptz
                  )
              AND (
                    sample.client_id,
                    sample.source_kind,
                    sample.interface,
                    sample.observed_at
                  ) <= (
                    $1,
                    requested.source_kind,
                    requested.interface,
                    'infinity'::timestamptz
                  )
            ORDER BY
                sample.client_id,
                sample.source_kind,
                sample.interface,
                sample.observed_at
            LIMIT 1
        ) raw ON TRUE
        UNION ALL
        SELECT rollup.first_bucket_start AS first_at
        FROM requested
        LEFT JOIN LATERAL (
            SELECT min(summary.first_bucket_start)
                AS first_bucket_start
            FROM traffic_counter_rollup_tier_summaries summary
            WHERE summary.client_id = $1
              AND summary.source_kind = requested.source_kind
              AND summary.interface = requested.interface
        ) rollup ON TRUE
    )
    SELECT extract(epoch FROM min(first_at))::double precision
    FROM bounded_starts
"#;

#[derive(Clone, Debug, Eq, PartialEq)]
struct TrafficHistoryStream {
    source_kind: String,
    interface: String,
    direction_mask: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TrafficCounterStreamUsage {
    client_id: String,
    source_kind: String,
    interface: String,
    cycle_rx: i64,
    cycle_tx: i64,
    latest_rx: i64,
    latest_tx: i64,
    last_sample_source: String,
    last_sample_unix: i64,
    rx_counter_epochs_seen: i64,
    tx_counter_epochs_seen: i64,
}

/// Compact policy-facing traffic state owned by the exact-client telemetry
/// projection cursor.  Natural-minute materialization remains the sole owner
/// of normalized traffic history; this vector only carries the unmaterialized
/// counter frontier needed to evaluate every accepted policy sample in order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ProjectedTrafficAccountingFrontierStream {
    source_kind: String,
    interface: String,
    cycle_start_unix: i64,
    cycle_rx: i64,
    cycle_tx: i64,
    latest_rx: i64,
    latest_tx: i64,
    last_sample_source: String,
    last_sample_unix: i64,
    rx_counter_epochs_seen: i64,
    tx_counter_epochs_seen: i64,
}

pub(crate) type ProjectedTrafficAccountingFrontier = Vec<ProjectedTrafficAccountingFrontierStream>;

pub(crate) struct ProjectedTrafficAccountingContext {
    client_id: String,
    rules: Vec<VpsRuleValueRecord>,
    expands_all_streams: bool,
    durable_streams: BTreeSet<TrafficStreamIdentity>,
    current_tunnel_interfaces: HashSet<String>,
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
        last_sample_source: row.try_get("last_sample_source")?,
        last_sample_unix: row.try_get("last_sample_unix")?,
        rx_counter_epochs_seen: row.try_get("rx_counter_epochs_seen")?,
        tx_counter_epochs_seen: row.try_get("tx_counter_epochs_seen")?,
    })
}

type ParsedRuleValue = ParsedVpsRuleValue;

#[cfg(test)]
#[derive(Clone, Debug)]
struct PolicyEvaluation {
    condition_true: bool,
    incomplete: bool,
    incomplete_reasons: Vec<String>,
    actual_value: Option<f64>,
    threshold_value: Option<f64>,
}

async fn preview_current_policy_rule(
    pool: &sqlx::PgPool,
    rule: &PolicyRuleRequest,
    matched_client_ids: &[String],
) -> Result<(i64, i64, i64, Vec<String>)> {
    if matched_client_ids.is_empty() {
        return Ok((0, 0, 0, Vec::new()));
    }
    if rule.evidence_source == "telemetry.combined" {
        return preview_current_combined_telemetry_policy_rule(pool, rule, matched_client_ids)
            .await;
    }
    let rows = sqlx::query(
        r#"
        SELECT evidence.subject_client_id, evidence.completeness,
               evidence.payload, evidence.subject_snapshot
        FROM alert_policy_effective_current_evidence current_fact
        JOIN alert_policy_evidence evidence
          ON evidence.id=current_fact.evidence_id
        WHERE current_fact.source_kind=$1
          AND current_fact.subject_client_id=ANY($2::text[])
          AND evidence.payload->>'source_present' IS DISTINCT FROM 'false'
        ORDER BY current_fact.subject_client_id,current_fact.natural_key
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

/// Previews telemetry policies from the projection owner's canonical latest
/// sample rather than requiring a policy evidence row to exist while every
/// telemetry policy is disabled. Admission masks preserve the exact interface
/// decision made when that sample was projected.
async fn preview_current_combined_telemetry_policy_rule(
    pool: &sqlx::PgPool,
    rule: &PolicyRuleRequest,
    matched_client_ids: &[String],
) -> Result<(i64, i64, i64, Vec<String>)> {
    let mut tx = pool.begin().await?;
    let rows = sqlx::query(
        r#"
        SELECT head.client_id, sample.id, sample.accepted_seq, sample.payload,
               sample.source_gateway_session_id,
               sample.source_process_incarnation_id,
               sample.source_telemetry_seq,
               sample.reported_observed_unix,
               sample.network_admission_mask,
               sample.tunnel_admission_mask
        FROM telemetry_projection_heads head
        JOIN telemetry_samples sample
          ON sample.id = head.latest_projected_sample_id
         AND sample.client_id = head.client_id
        WHERE head.client_id=ANY($1::text[])
          AND head.latest_projected_sample_id IS NOT NULL
        ORDER BY head.client_id
        "#,
    )
    .bind(matched_client_ids)
    .fetch_all(&mut *tx)
    .await?;
    let mut true_count = 0_i64;
    let mut false_count = 0_i64;
    let mut incomplete_count = 0_i64;
    let mut seen_subjects = HashSet::new();
    let mut incomplete_subjects = BTreeSet::new();
    for row in rows {
        let client_id: String = row.try_get("client_id")?;
        let sample_id: Uuid = row.try_get("id")?;
        let accepted_seq: i64 = row.try_get("accepted_seq")?;
        let metrics = row.try_get::<SqlJson<AgentMetrics>, _>("payload")?.0;
        let gateway_session_id: Uuid = row.try_get("source_gateway_session_id")?;
        let process_incarnation_id: Uuid = row.try_get("source_process_incarnation_id")?;
        let telemetry_seq = u64::try_from(row.try_get::<i64, _>("source_telemetry_seq")?)
            .context("negative latest telemetry projection source sequence")?;
        let reported_observed_unix =
            u64::try_from(row.try_get::<i64, _>("reported_observed_unix")?)
                .context("negative latest telemetry projection reported observed time")?;
        let network_admission_mask: Vec<u8> = row.try_get("network_admission_mask")?;
        let tunnel_admission_mask: Vec<u8> = row.try_get("tunnel_admission_mask")?;

        let traffic = reconstruct_projected_policy_traffic_in_tx(
            &mut tx,
            &client_id,
            accepted_seq,
            &metrics,
            &network_admission_mask,
            &tunnel_admission_mask,
        )
        .await?;
        let Some(subject) = crate::repository_policy_lifecycle::load_policy_subject_snapshot_in_tx(
            &mut tx, &client_id,
        )
        .await?
        else {
            continue;
        };
        let payload = combined_metric_evidence_payload(
            &metrics,
            &traffic,
            gateway_session_id,
            process_incarnation_id,
            telemetry_seq,
            sample_id,
            reported_observed_unix,
        );
        seen_subjects.insert(client_id.clone());
        match crate::repository_policy_lifecycle::policy_expression_truth_for_preview(
            rule.rule_kind,
            &rule.trigger_condition_expression,
            &payload,
            &subject,
            true,
        )? {
            ExpressionTruth::True => true_count += 1,
            ExpressionTruth::False => false_count += 1,
            ExpressionTruth::Unknown => {
                incomplete_count += 1;
                incomplete_subjects.insert(client_id);
            }
        }
    }
    for client_id in matched_client_ids {
        if !seen_subjects.contains(client_id) {
            incomplete_count += 1;
            incomplete_subjects.insert(client_id.clone());
        }
    }
    tx.commit().await?;
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
                    VPS_RULE_KEY_NETWORK_INTERFACES,
                    VPS_RULE_KEY_NETWORK_RATE_INTERFACES,
                    VPS_RULE_KEY_TRAFFIC_SELECTORS,
                ],
            )
            .await?;
        self.network_rate_interface_selection_from_rules(client_ids, &rules)
            .await
    }

    pub(crate) async fn network_rate_interface_selection_from_rules(
        &self,
        client_ids: &[String],
        rules: &[VpsRuleValueRecord],
    ) -> Result<NetworkRateInterfaceSelection> {
        let requested = client_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let configured_clients = rules
            .iter()
            .filter(|rule| {
                rule.key == VPS_RULE_KEY_NETWORK_RATE_INTERFACES
                    && requested.contains(rule.client_id.as_str())
            })
            .map(|rule| rule.client_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let inventories = self
            .network_interface_inventories_for_clients(&configured_clients, &[])
            .await?;
        resolve_network_rate_interface_selection(client_ids, rules, &inventories)
    }

    async fn network_interface_inventories_for_clients(
        &self,
        current_client_ids: &[String],
        traffic_client_ids: &[String],
    ) -> Result<HashMap<String, NetworkInterfaceInventory>> {
        if current_client_ids.is_empty() && traffic_client_ids.is_empty() {
            return Ok(HashMap::new());
        }
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT client_id, 'traffic' AS inventory_kind,
                           source_kind, interface
                    FROM traffic_counter_streams
                    WHERE client_id = ANY($2::TEXT[])
                    UNION ALL
                    SELECT client_id, 'current_host' AS inventory_kind,
                           'host' AS source_kind, interface
                    FROM telemetry_network_current_identities_source($1::TEXT[])
                    WHERE client_id = ANY($1::TEXT[])
                    UNION ALL
                    SELECT plan.left_client_id AS client_id,
                           'current_tunnel' AS inventory_kind,
                           'tunnel' AS source_kind,
                           plan.plan ->> 'interface_name' AS interface
                    FROM tunnel_plans plan
                    WHERE plan.left_client_id = ANY($1::TEXT[])
                      AND plan.enabled IS TRUE
                      AND plan.deleted_at IS NULL
                    UNION ALL
                    SELECT plan.right_client_id AS client_id,
                           'current_tunnel' AS inventory_kind,
                           'tunnel' AS source_kind,
                           plan.plan ->> 'interface_name' AS interface
                    FROM tunnel_plans plan
                    WHERE plan.right_client_id = ANY($1::TEXT[])
                      AND plan.enabled IS TRUE
                      AND plan.deleted_at IS NULL
                    ORDER BY client_id, inventory_kind, source_kind, interface
                    "#,
                )
                .bind(current_client_ids)
                .bind(traffic_client_ids)
                .fetch_all(pool)
                .await?;
                let mut by_client = HashMap::<String, NetworkInterfaceInventory>::new();
                for row in rows {
                    let inventory = by_client.entry(row.try_get("client_id")?).or_default();
                    let inventory_kind: String = row.try_get("inventory_kind")?;
                    let source_kind: String = row.try_get("source_kind")?;
                    let interface: String = row.try_get("interface")?;
                    match inventory_kind.as_str() {
                        "traffic" => {
                            inventory.traffic_streams.insert((source_kind, interface));
                        }
                        "current_host" => {
                            inventory.current_host_interfaces.insert(interface);
                        }
                        "current_tunnel" => {
                            inventory.current_tunnel_interfaces.insert(interface);
                        }
                        _ => anyhow::bail!("network_interface_inventory_kind_invalid"),
                    }
                }
                Ok(by_client)
            }
        }
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
            wake_policy_evaluator();
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
            wake_policy_evaluator();
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
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let (agents, stored) = postgres_vps_rule_snapshot_in_tx(&mut tx).await?;
                let initial_preview = build_vps_rule_preview(
                    operation,
                    selector_expression,
                    values,
                    keys,
                    &agents,
                    &stored,
                )?;
                let target_client_ids = initial_preview
                    .changes
                    .iter()
                    .map(|change| change.client_id.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                let definition_identities = initial_preview
                    .changes
                    .iter()
                    .map(|change| format!("vps-rule:{}:{}", change.client_id, change.key))
                    .collect::<Vec<_>>();
                lock_postgres_definitions_and_clients_in_tx(
                    &mut tx,
                    &definition_identities,
                    &target_client_ids,
                )
                .await?;
                // Rebuild after acquiring every reviewed owner. A tag/client
                // mutation that won the race therefore produces the existing
                // preview-stale response instead of changing the committed set.
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
                    lock_postgres_traffic_reset_rule_targets(&mut tx, &preview).await?;
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

    async fn traffic_accounting_for_selected_agents_with_rules(
        &self,
        selected_agents: &[AgentView],
        rules: &[VpsRuleValueRecord],
        now: DateTime<Utc>,
    ) -> Result<Vec<TrafficAccountingRecord>> {
        let client_ids = selected_agents
            .iter()
            .map(|agent| agent.id.clone())
            .collect::<Vec<_>>();
        let selected = client_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let mut configured_clients = BTreeSet::new();
        let mut all_stream_clients = BTreeSet::new();
        for rule in rules.iter().filter(|rule| {
            rule.key == VPS_RULE_KEY_TRAFFIC_SELECTORS && selected.contains(rule.client_id.as_str())
        }) {
            configured_clients.insert(rule.client_id.clone());
            if traffic_selector_spec_from_rule(rule)? == TrafficSelectorSpec::All {
                all_stream_clients.insert(rule.client_id.clone());
            }
        }
        let interface_inventories = self
            .network_interface_inventories_for_clients(
                &configured_clients.into_iter().collect::<Vec<_>>(),
                &all_stream_clients.into_iter().collect::<Vec<_>>(),
            )
            .await?;
        let cycle_starts = traffic_cycle_starts_for_clients(
            selected_agents.iter().map(|agent| agent.id.as_str()),
            rules,
            now,
        );
        let stream_requests =
            traffic_stream_requests_from_rules(&cycle_starts, rules, &interface_inventories)?
                .into_iter()
                .collect::<Vec<_>>();
        // One indexed array lookup supplies every current-generation boundary
        // for the requested client set.
        let projected_streams = self
            .latest_projected_traffic_streams(&client_ids, rules)
            .await?;
        let traffic_usage = self
            .list_traffic_counter_usage_for_streams(&stream_requests, now.timestamp())
            .await?;
        Ok(traffic_accounting_for_agents(
            selected_agents,
            rules,
            &traffic_usage,
            now,
            &projected_streams,
            &interface_inventories,
        ))
    }

    async fn latest_projected_traffic_streams(
        &self,
        client_ids: &[String],
        rules: &[VpsRuleValueRecord],
    ) -> Result<HashMap<String, HashSet<TrafficStreamIdentity>>> {
        if client_ids.is_empty() {
            return Ok(HashMap::new());
        }
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH current_tunnels AS MATERIALIZED (
                        SELECT
                            identity.client_id,
                            jsonb_agg(
                                jsonb_build_object(
                                    'plan_id', identity.telemetry_plan_id,
                                    'plan_name', identity.telemetry_plan_name,
                                    'interface', identity.interface,
                                    'kind', identity.kind,
                                    'endpoint_side',
                                        identity.telemetry_endpoint_side,
                                    'peer_client_id',
                                        identity.telemetry_peer_client_id
                                )
                                ORDER BY identity.interface COLLATE "C"
                            ) AS identities
                        FROM telemetry_current_tunnels identity
                        WHERE identity.client_id = ANY($1::TEXT[])
                        GROUP BY identity.client_id
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
                            WHERE plan.left_client_id = ANY($1::TEXT[])
                              AND plan.enabled IS TRUE
                              AND plan.deleted_at IS NULL
                            UNION ALL
                            SELECT plan.right_client_id AS client_id,
                                   plan.plan ->> 'interface_name' AS interface
                            FROM tunnel_plans plan
                            WHERE plan.right_client_id = ANY($1::TEXT[])
                              AND plan.enabled IS TRUE
                              AND plan.deleted_at IS NULL
                        ) endpoint
                        GROUP BY endpoint.client_id
                    )
                    SELECT
                        projection.client_id,
                        latest.payload,
                        latest.network_admission_mask,
                        latest.tunnel_admission_mask,
                        COALESCE(
                            current_tunnels.identities,
                            '[]'::JSONB
                        ) AS current_tunnel_identities,
                        COALESCE(
                            managed_tunnel_interfaces.interfaces,
                            ARRAY[]::TEXT[]
                        ) AS managed_tunnel_interfaces
                    FROM telemetry_projection_heads projection
                    JOIN telemetry_samples latest
                      ON latest.id = projection.latest_projected_sample_id
                     AND latest.client_id = projection.client_id
                    LEFT JOIN current_tunnels
                      ON current_tunnels.client_id = projection.client_id
                    LEFT JOIN managed_tunnel_interfaces
                      ON managed_tunnel_interfaces.client_id = projection.client_id
                    WHERE projection.client_id = ANY($1::text[])
                    "#,
                )
                .bind(client_ids)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let client_id: String = row.try_get("client_id")?;
                        let payload: SqlJson<AgentMetrics> = row.try_get("payload")?;
                        let network_admission_mask: Vec<u8> =
                            row.try_get("network_admission_mask")?;
                        let tunnel_admission_mask: Vec<u8> =
                            row.try_get("tunnel_admission_mask")?;
                        let current_tunnel_identities = row
                            .try_get::<SqlJson<Vec<ProjectedTelemetryTunnelIdentity>>, _>(
                                "current_tunnel_identities",
                            )?
                            .0
                            .into_iter()
                            .collect::<HashSet<_>>();
                        let managed_tunnel_interfaces = row
                            .try_get::<Vec<String>, _>("managed_tunnel_interfaces")?
                            .into_iter()
                            .collect::<HashSet<_>>();
                        let policy = network_interface_policy_for_client(&client_id, rules)?;
                        Ok((
                            client_id,
                            projected_traffic_streams_with_policy(
                                &payload.0,
                                &policy,
                                &network_admission_mask,
                                &tunnel_admission_mask,
                                &current_tunnel_identities,
                                &managed_tunnel_interfaces,
                            ),
                        ))
                    })
                    .collect()
            }
        }
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
            Self::Postgres(pool) => {
                let source_kinds = streams
                    .iter()
                    .map(|stream| stream.source_kind.clone())
                    .collect::<Vec<_>>();
                let interfaces = streams
                    .iter()
                    .map(|stream| stream.interface.clone())
                    .collect::<Vec<_>>();
                let value = sqlx::query_scalar::<_, Option<f64>>(TRAFFIC_HISTORY_START_SQL)
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
    ) -> Result<Vec<TrafficHistoryPointView>> {
        let streams = self.traffic_history_streams(client_id).await?;
        if streams.is_empty() || start_unix > end_unix {
            return Ok(Vec::new());
        }
        let step_secs = step_secs.max(60);
        match self {
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
                let rows = sqlx::query(
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
                                FALSE AS baseline_only,
                                sample.observed_at,
                                sample.rx_bytes,
                                sample.tx_bytes,
                                sample.rx_counter_epoch,
                                sample.tx_counter_epoch,
                                sample.sample_source,
                                sample.usage_authoritative,
                                sample.rx_usage_bytes,
                                sample.tx_usage_bytes,
                                sample.rx_valid_count,
                                sample.tx_valid_count,
                                sample.any_valid_count,
                                sample.rx_reset_count,
                                sample.tx_reset_count,
                                sample.any_reset_count
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
                                TRUE AS baseline_only,
                                previous.observed_at,
                                previous.rx_bytes,
                                previous.tx_bytes,
                                previous.rx_counter_epoch,
                                previous.tx_counter_epoch,
                                previous.sample_source,
                                previous.usage_authoritative,
                                previous.rx_usage_bytes,
                                previous.tx_usage_bytes,
                                previous.rx_valid_count,
                                previous.tx_valid_count,
                                previous.any_valid_count,
                                previous.rx_reset_count,
                                previous.tx_reset_count,
                                previous.any_reset_count
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
                                    sample_source,
                                    usage_authoritative,
                                    rx_usage_bytes,
                                    tx_usage_bytes,
                                    rx_valid_count,
                                    tx_valid_count,
                                    any_valid_count,
                                    rx_reset_count,
                                    tx_reset_count,
                                    any_reset_count
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
                                CASE WHEN usage_authoritative
                                     THEN rx_usage_bytes
                                     WHEN rx_counter_epoch = previous_rx_counter_epoch
                                      AND rx_bytes >= previous_rx_bytes
                                     THEN rx_bytes - previous_rx_bytes
                                     ELSE 0 END::bigint
                                    AS rx_bytes,
                                CASE WHEN usage_authoritative
                                     THEN tx_usage_bytes
                                     WHEN tx_counter_epoch = previous_tx_counter_epoch
                                      AND tx_bytes >= previous_tx_bytes
                                     THEN tx_bytes - previous_tx_bytes
                                     ELSE 0 END::bigint
                                    AS tx_bytes,
                                CASE WHEN usage_authoritative
                                     THEN rx_valid_count
                                     WHEN rx_counter_epoch = previous_rx_counter_epoch
                                      AND rx_bytes >= previous_rx_bytes
                                     THEN 1 ELSE 0 END::integer
                                    AS rx_valid_count,
                                CASE WHEN usage_authoritative
                                     THEN tx_valid_count
                                     WHEN tx_counter_epoch = previous_tx_counter_epoch
                                      AND tx_bytes >= previous_tx_bytes
                                     THEN 1 ELSE 0 END::integer
                                    AS tx_valid_count,
                                CASE WHEN usage_authoritative
                                     THEN any_valid_count
                                     WHEN (rx_counter_epoch = previous_rx_counter_epoch
                                           AND rx_bytes >= previous_rx_bytes)
                                       OR (tx_counter_epoch = previous_tx_counter_epoch
                                           AND tx_bytes >= previous_tx_bytes)
                                     THEN 1 ELSE 0 END::integer
                                    AS any_valid_count,
                                CASE WHEN usage_authoritative
                                     THEN rx_reset_count
                                     WHEN previous_rx_counter_epoch IS NOT NULL
                                      AND rx_counter_epoch <>
                                            previous_rx_counter_epoch
                                      AND NOT (
                                          previous_sample_source LIKE
                                                'vnstat_import:%'
                                          AND sample_source NOT LIKE
                                                'vnstat_import:%'
                                      )
                                     THEN 1 ELSE 0 END::integer
                                    AS rx_reset_count,
                                CASE WHEN usage_authoritative
                                     THEN tx_reset_count
                                     WHEN previous_tx_counter_epoch IS NOT NULL
                                      AND tx_counter_epoch <>
                                            previous_tx_counter_epoch
                                      AND NOT (
                                          previous_sample_source LIKE
                                                'vnstat_import:%'
                                          AND sample_source NOT LIKE
                                                'vnstat_import:%'
                                      )
                                     THEN 1 ELSE 0 END::integer
                                    AS tx_reset_count,
                                CASE WHEN usage_authoritative
                                     THEN any_reset_count
                                     WHEN previous_rx_counter_epoch IS NOT NULL
                                      AND (
                                          rx_counter_epoch <>
                                                previous_rx_counter_epoch
                                          OR tx_counter_epoch <>
                                                previous_tx_counter_epoch
                                      )
                                      AND NOT (
                                          previous_sample_source LIKE
                                                'vnstat_import:%'
                                          AND sample_source NOT LIKE
                                                'vnstat_import:%'
                                      )
                                     THEN 1 ELSE 0 END::integer
                                    AS any_reset_count
                            FROM raw_sequenced
                            WHERE NOT baseline_only
                              AND (
                                  usage_authoritative
                                  OR previous_rx_counter_epoch IS NOT NULL
                                  OR previous_tx_counter_epoch IS NOT NULL
                              )
                              AND observed_at >= to_timestamp($5)
                              AND observed_at <= to_timestamp($6)
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
                .await?;
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
        let client_ids = vec![client_id.to_string()];
        let rules = self
            .list_vps_rules_for_clients(
                &client_ids,
                &[
                    VPS_RULE_KEY_NETWORK_INTERFACES,
                    VPS_RULE_KEY_TRAFFIC_SELECTORS,
                ],
            )
            .await?;
        let Some(rule) = rules
            .iter()
            .find(|rule| rule.key == VPS_RULE_KEY_TRAFFIC_SELECTORS)
        else {
            return Ok(Vec::new());
        };
        let policy = network_interface_policy_for_client(client_id, &rules)?;
        let selector_spec = traffic_selector_spec_from_rule(rule)?;
        let traffic_clients = if selector_spec == TrafficSelectorSpec::All {
            vec![client_id.to_string()]
        } else {
            Vec::new()
        };
        let inventories_by_client = self
            .network_interface_inventories_for_clients(&client_ids, &traffic_clients)
            .await?;
        let inventory = inventories_by_client.get(client_id);
        let selectors = eligible_traffic_selectors(selector_spec, &policy, inventory);
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
                } else {
                    match self {
                        Self::Postgres(pool) => {
                            preview_current_policy_rule(pool, rule, &matched_client_ids).await?
                        }
                    }
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
        self.list_fleet_alert_policies_inner(
            Some(limit),
            enabled,
            selector_expression,
            client_id,
            allow_vps_rule_selectors,
        )
        .await
    }

    async fn list_fleet_alert_policies_inner(
        &self,
        limit: Option<i64>,
        enabled: Option<bool>,
        selector_expression: Option<&str>,
        client_id: Option<&str>,
        allow_vps_rule_selectors: bool,
    ) -> Result<Vec<PolicyGroupRecord>> {
        let definition_limit = if selector_expression.is_none() && client_id.is_none() {
            limit.map(|limit| limit.clamp(1, 1000) as usize)
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
        if let Some(limit) = limit {
            groups.truncate(limit.clamp(1, 1000) as usize);
        }
        Ok(groups)
    }

    pub(crate) async fn list_all_fleet_alert_policies_with_context(
        &self,
        allow_vps_rule_selectors: bool,
        agents: &[AgentView],
        rules: &[VpsRuleValueRecord],
    ) -> Result<Vec<PolicyGroupRecord>> {
        self.list_fleet_alert_policies_with_context_inner(
            None,
            allow_vps_rule_selectors,
            agents,
            rules,
        )
        .await
    }

    async fn list_fleet_alert_policies_with_context_inner(
        &self,
        limit: Option<i64>,
        allow_vps_rule_selectors: bool,
        agents: &[AgentView],
        rules: &[VpsRuleValueRecord],
    ) -> Result<Vec<PolicyGroupRecord>> {
        let mut groups = self
            .list_fleet_alert_policy_definitions(
                limit.map(|limit| limit.clamp(1, 1000) as usize),
                None,
            )
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
        if let Some(limit) = limit {
            groups.truncate(limit.clamp(1, 1000) as usize);
        }
        Ok(groups)
    }

    async fn list_fleet_alert_policy_definitions(
        &self,
        result_limit: Option<usize>,
        enabled: Option<bool>,
    ) -> Result<Vec<PolicyGroupRecord>> {
        let mut groups = match self {
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
        let (group, telemetry_activation_changed) = match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let mut definition_identities =
                    vec![format!("alert-policy-name:{}", request.name.trim())];
                if let Some(id) = request.id {
                    definition_identities.push(format!("alert-policy:{id}"));
                }
                lock_postgres_definition_lifecycles_in_tx(&mut tx, &definition_identities).await?;
                let existing_uses_telemetry: bool = sqlx::query_scalar(
                    r#"
                    SELECT EXISTS (
                        SELECT 1
                        FROM policy_rules rule
                        JOIN policy_groups policy ON policy.id=rule.group_id
                        WHERE (($1::uuid IS NOT NULL AND policy.id=$1)
                               OR policy.name=$2)
                          AND rule.evidence_source='telemetry.combined'
                    )
                    "#,
                )
                .bind(request.id)
                .bind(request.name.trim())
                .fetch_one(&mut *tx)
                .await?;
                let requested_uses_telemetry = request
                    .rules
                    .iter()
                    .any(|rule| rule.evidence_source.trim() == "telemetry.combined");
                let touches_telemetry_activation =
                    existing_uses_telemetry || requested_uses_telemetry;
                if existing_uses_telemetry || requested_uses_telemetry {
                    lock_postgres_definition_lifecycles_in_tx(
                        &mut tx,
                        &["alert-policy-telemetry-consumer".to_string()],
                    )
                    .await?;
                }
                let telemetry_policy_was_enabled: bool = sqlx::query_scalar(
                    r#"
                    SELECT EXISTS (
                        SELECT 1
                        FROM policy_rules rule
                        JOIN policy_groups policy ON policy.id=rule.group_id
                        WHERE policy.enabled
                          AND rule.enabled
                          AND rule.evidence_source='telemetry.combined'
                    )
                    "#,
                )
                .fetch_one(&mut *tx)
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
                if !invalidated_rule_ids.is_empty() {
                    sqlx::query(
                        r#"
                        SELECT id
                        FROM policy_rules
                        WHERE id=ANY($1::uuid[])
                        ORDER BY id
                        FOR UPDATE
                        "#,
                    )
                    .bind(&invalidated_rule_ids)
                    .fetch_all(&mut *tx)
                    .await?;
                }
                let drained_evidence_ids =
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
                let baseline_rule_ids = if telemetry_policy_was_enabled {
                    retained_rule_ids.clone()
                } else {
                    group
                        .rules
                        .iter()
                        .filter(|rule| rule.evidence_source != "telemetry.combined")
                        .map(|rule| rule.id)
                        .collect::<Vec<_>>()
                };
                crate::repository_policy_lifecycle::evaluate_policy_rule_baselines_in_tx(
                    &mut tx,
                    &baseline_rule_ids,
                )
                .await?;
                for evidence_id in drained_evidence_ids {
                    crate::repository_policy_lifecycle::recompute_policy_evidence_pending_in_tx(
                        &mut tx,
                        evidence_id,
                    )
                    .await?;
                }
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
                let telemetry_activation_changed = if touches_telemetry_activation {
                    reconcile_telemetry_policy_activation_request_in_tx(&mut tx).await?
                } else {
                    false
                };
                if telemetry_activation_changed {
                    mark_telemetry_policy_activation_may_be_pending();
                }
                tx.commit().await?;
                (group, telemetry_activation_changed)
            }
        };
        let policy_id = group.id;
        if telemetry_activation_changed {
            wake_telemetry_policy_activation();
        }
        wake_policy_evaluator();
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
        let telemetry_activation_changed = match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_postgres_definition_lifecycles_in_tx(
                    &mut tx,
                    &[format!("alert-policy:{policy_id}")],
                )
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
                let uses_telemetry = policy
                    .rules
                    .iter()
                    .any(|rule| rule.evidence_source == "telemetry.combined");
                if uses_telemetry {
                    lock_postgres_definition_lifecycles_in_tx(
                        &mut tx,
                        &["alert-policy-telemetry-consumer".to_string()],
                    )
                    .await?;
                }
                let rule_ids = policy.rules.iter().map(|rule| rule.id).collect::<Vec<_>>();
                if !rule_ids.is_empty() {
                    sqlx::query(
                        r#"
                        SELECT id
                        FROM policy_rules
                        WHERE id=ANY($1::uuid[])
                        ORDER BY id
                        FOR UPDATE
                        "#,
                    )
                    .bind(&rule_ids)
                    .fetch_all(&mut *tx)
                    .await?;
                }
                let drained_evidence_ids =
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
                for evidence_id in drained_evidence_ids {
                    crate::repository_policy_lifecycle::recompute_policy_evidence_pending_in_tx(
                        &mut tx,
                        evidence_id,
                    )
                    .await?;
                }
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
                let telemetry_activation_changed = if uses_telemetry {
                    reconcile_telemetry_policy_activation_request_in_tx(&mut tx).await?
                } else {
                    false
                };
                if telemetry_activation_changed {
                    mark_telemetry_policy_activation_may_be_pending();
                }
                tx.commit().await?;
                telemetry_activation_changed
            }
        };
        if telemetry_activation_changed {
            wake_telemetry_policy_activation();
        }
        wake_policy_evaluator();
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
        let allowed_client_id_values =
            allowed_client_ids.map(|client_ids| client_ids.iter().cloned().collect::<Vec<_>>());
        match self {
            Self::Postgres(pool) => {
                list_unified_policy_alerts_postgres(
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
                .await
            }
        }
    }

    async fn evaluate_policy_rules_page(&self) -> Result<PolicyEvaluationPage> {
        match self {
            Self::Postgres(pool) => {
                let scope_examined =
                    crate::repository_policy_lifecycle::materialize_pending_policy_scope_revisions(
                        pool,
                        POLICY_SCOPE_MAINTENANCE_PAGE,
                    )
                    .await?;
                let evidence =
                    crate::repository_policy_lifecycle::evaluate_pending_policy_evidence_page(
                        pool,
                        POLICY_EVIDENCE_MAINTENANCE_PAGE,
                    )
                    .await?;
                let due = crate::repository_policy_lifecycle::evaluate_due_policy_transitions_page(
                    pool,
                    POLICY_DUE_MAINTENANCE_PAGE,
                )
                .await?;
                Ok(PolicyEvaluationPage {
                    scope_examined,
                    evidence_examined: evidence.examined,
                    evidence_still_due: evidence.still_due,
                    due_examined: due.examined,
                    due_transitioned: due.changed,
                })
            }
        }
    }

    /// Drains already-due durable work without turning the transaction page
    /// sizes into throughput caps. The configured scheduler interval applies
    /// only after every queue returned a short page.
    pub(crate) async fn drain_policy_rule_backlog(&self) -> Result<usize> {
        let mut transitioned = 0_usize;
        loop {
            let page = self.evaluate_policy_rules_page().await?;
            transitioned = transitioned
                .checked_add(page.due_transitioned)
                .context("policy transition count overflow")?;
            if !page.may_have_more() {
                return Ok(transitioned);
            }
            tokio::task::yield_now().await;
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
                    // A cached generic plan estimates UNNEST at ten rows and can multiply
                    // otherwise bounded probes across the wrong join order. JIT compilation
                    // is also much more expensive than the one-shot read. Keep both
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
            Self::Postgres(pool) => Ok(sqlx::query(
                r#"
                SELECT policy_rule_id, client_id, trigger_generation
                FROM alert_episodes
                WHERE policy_rule_id = ANY($1)
                  AND client_id IS NOT NULL
                  AND lifecycle_state IN ('triggered', 'persisting')
                  AND resolved_at IS NULL
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
}

async fn lock_postgres_traffic_reset_rule_targets(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    preview: &VpsRulesDryRunResponse,
) -> Result<()> {
    // Reset-day and reset-hour are one canonical rule value. Its trigger
    // rebuilds both host and tunnel active-cycle prefixes, so it must join the
    // same per-client traffic-ledger owner as live projection and vnStat
    // replacement before the rule DML establishes its READ COMMITTED
    // snapshot. BTreeSet supplies the canonical cross-client lock order.
    let client_ids = preview
        .changes
        .iter()
        .filter(|change| {
            change.key == VPS_RULE_KEY_TRAFFIC_RESET_DAY
                && matches!(change.action.as_str(), "set" | "unset")
        })
        .map(|change| change.client_id.as_str())
        .collect::<BTreeSet<_>>();
    for client_id in client_ids {
        lock_postgres_traffic_counter_streams(tx, client_id).await?;
    }
    Ok(())
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

#[cfg(test)]
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

#[cfg(test)]
fn policy_state_is_alert_eligible(state: &PolicyRuleStateRecord) -> bool {
    state.condition_true && state.window_satisfied && !state.incomplete
}

fn traffic_cycle_starts_for_clients<'a>(
    client_ids: impl IntoIterator<Item = &'a str>,
    rules: &[VpsRuleValueRecord],
    now: DateTime<Utc>,
) -> Vec<(String, i64)> {
    let reset_boundaries = rules
        .iter()
        .filter(|rule| rule.key == VPS_RULE_KEY_TRAFFIC_RESET_DAY)
        .filter_map(|rule| {
            parsed_traffic_reset(rule).map(|boundary| (rule.client_id.as_str(), boundary))
        })
        .collect::<HashMap<_, _>>();
    client_ids
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|client_id| {
            let (reset_day, reset_hour) =
                reset_boundaries.get(client_id).copied().unwrap_or((1, 0));
            (
                client_id.to_string(),
                if reset_day == -1 {
                    NO_RESET_TRAFFIC_START_UNIX
                } else {
                    cycle_bounds(reset_day, reset_hour, now).0.timestamp()
                },
            )
        })
        .collect()
}

fn parsed_traffic_reset(rule: &VpsRuleValueRecord) -> Option<(i32, i32)> {
    let parsed = parse_vps_rule_value(&rule.key, &rule.value_raw).ok()?;
    Some((
        parsed.json.get("day")?.as_i64()? as i32,
        parsed.json.get("hour")?.as_i64()? as i32,
    ))
}

fn apply_projected_traffic_counter_overlay(
    usage: &mut Vec<TrafficCounterStreamUsage>,
    requests: &[TrafficStreamRequest],
    overlay: &ProjectedTrafficCounterOverlay,
    as_of: DateTime<Utc>,
) {
    if overlay.observed_at > as_of {
        return;
    }
    let cycle_starts = requests
        .iter()
        .map(|request| {
            (
                (request.source_kind.as_str(), request.interface.as_str()),
                request.cycle_start_unix,
            )
        })
        .collect::<HashMap<_, _>>();
    let observed_unix = overlay.observed_at.timestamp();

    for counter in &overlay.counters {
        let Some(cycle_start_unix) = cycle_starts
            .get(&(counter.source_kind.as_str(), counter.interface.as_str()))
            .copied()
        else {
            continue;
        };
        let rx_bytes = counter.rx_bytes.max(0);
        let tx_bytes = counter.tx_bytes.max(0);
        if let Some(stream) = usage.iter_mut().find(|stream| {
            stream.source_kind == counter.source_kind && stream.interface == counter.interface
        }) {
            if stream.last_sample_unix > observed_unix {
                continue;
            }
            let intentional_import_boundary = is_vnstat_import_source(&stream.last_sample_source)
                && !is_vnstat_import_source(&counter.sample_source);
            if observed_unix >= cycle_start_unix && !intentional_import_boundary {
                if rx_bytes >= stream.latest_rx {
                    stream.cycle_rx = stream
                        .cycle_rx
                        .saturating_add(rx_bytes.saturating_sub(stream.latest_rx));
                } else {
                    stream.rx_counter_epochs_seen = stream.rx_counter_epochs_seen.saturating_add(1);
                }
                if tx_bytes >= stream.latest_tx {
                    stream.cycle_tx = stream
                        .cycle_tx
                        .saturating_add(tx_bytes.saturating_sub(stream.latest_tx));
                } else {
                    stream.tx_counter_epochs_seen = stream.tx_counter_epochs_seen.saturating_add(1);
                }
            }
            stream.latest_rx = rx_bytes;
            stream.latest_tx = tx_bytes;
            stream.last_sample_source.clone_from(&counter.sample_source);
            stream.last_sample_unix = observed_unix;
            continue;
        }

        let client_id = requests
            .iter()
            .find(|request| {
                request.source_kind == counter.source_kind && request.interface == counter.interface
            })
            .map(|request| request.client_id.clone())
            .unwrap_or_default();
        usage.push(TrafficCounterStreamUsage {
            client_id,
            source_kind: counter.source_kind.clone(),
            interface: counter.interface.clone(),
            cycle_rx: 0,
            cycle_tx: 0,
            latest_rx: rx_bytes,
            latest_tx: tx_bytes,
            last_sample_source: counter.sample_source.clone(),
            last_sample_unix: observed_unix,
            rx_counter_epochs_seen: 1,
            tx_counter_epochs_seen: 1,
        });
    }
    usage.sort_by(|left, right| {
        left.client_id
            .cmp(&right.client_id)
            .then_with(|| left.source_kind.cmp(&right.source_kind))
            .then_with(|| left.interface.cmp(&right.interface))
    });
}

impl ProjectedTrafficAccountingContext {
    fn requests_and_inventory(
        &self,
        as_of: DateTime<Utc>,
        metrics: &AgentMetrics,
    ) -> Result<(Vec<TrafficStreamRequest>, NetworkInterfaceInventory)> {
        let inventory = network_interface_inventory_from_metrics(
            metrics,
            self.durable_streams.clone(),
            &self.current_tunnel_interfaces,
        );
        let inventories = HashMap::from([(self.client_id.clone(), inventory.clone())]);
        let cycle_starts =
            traffic_cycle_starts_for_clients([self.client_id.as_str()], &self.rules, as_of);
        let requests =
            traffic_stream_requests_from_rules(&cycle_starts, &self.rules, &inventories)?
                .into_iter()
                .collect();
        Ok((requests, inventory))
    }

    fn accounting_record(
        &self,
        as_of: DateTime<Utc>,
        usage: &[TrafficCounterStreamUsage],
        inventory: &NetworkInterfaceInventory,
        projected_streams: &HashSet<TrafficStreamIdentity>,
    ) -> TrafficAccountingRecord {
        traffic_accounting_for_client_with_freshness_and_candidates(
            &self.client_id,
            &self.rules,
            usage,
            as_of,
            None,
            Some(TrafficFreshnessBoundary {
                projected_streams: Some(projected_streams),
                online: true,
            }),
            Some(inventory),
        )
    }
}

/// Loads one immutable traffic-accounting definition snapshot for a claimed
/// client suffix.  The only optional database expansion is the durable stream
/// key set required by an explicit `traffic.selectors = *`; counters and
/// normalized history remain owned by the minute materializer.
pub(crate) async fn load_projected_traffic_accounting_context_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    current_tunnel_interfaces: &HashSet<String>,
) -> Result<ProjectedTrafficAccountingContext> {
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
    let expands_all_streams = rules
        .iter()
        .find(|rule| rule.key == VPS_RULE_KEY_TRAFFIC_SELECTORS)
        .map(traffic_selector_spec_from_rule)
        .transpose()?
        .is_some_and(|spec| spec == TrafficSelectorSpec::All);
    let durable_streams = if expands_all_streams {
        postgres_traffic_stream_identities_in_tx(tx, client_id).await?
    } else {
        BTreeSet::new()
    };
    Ok(ProjectedTrafficAccountingContext {
        client_id: client_id.to_string(),
        rules,
        expands_all_streams,
        durable_streams,
        current_tunnel_interfaces: current_tunnel_interfaces.clone(),
    })
}

/// Refreshes only the normalized stream-key inventory used by an explicit
/// wildcard selector.  Rules and current topology stay frozen to the claimed
/// projection suffix; callers fence this read with the traffic-minute cursor.
pub(crate) async fn refresh_projected_traffic_accounting_durable_streams_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    context: &mut ProjectedTrafficAccountingContext,
) -> Result<()> {
    if context.expands_all_streams {
        context.durable_streams =
            postgres_traffic_stream_identities_in_tx(tx, &context.client_id).await?;
    }
    Ok(())
}

fn frontier_requests_match(
    frontier: &ProjectedTrafficAccountingFrontier,
    requests: &[TrafficStreamRequest],
) -> bool {
    frontier.len() == requests.len()
        && frontier.iter().zip(requests).all(|(stream, request)| {
            stream.source_kind == request.source_kind
                && stream.interface == request.interface
                && stream.cycle_start_unix == request.cycle_start_unix
        })
}

fn usage_from_projected_traffic_frontier(
    client_id: &str,
    frontier: &ProjectedTrafficAccountingFrontier,
) -> Vec<TrafficCounterStreamUsage> {
    frontier
        .iter()
        .map(|stream| TrafficCounterStreamUsage {
            client_id: client_id.to_string(),
            source_kind: stream.source_kind.clone(),
            interface: stream.interface.clone(),
            cycle_rx: stream.cycle_rx,
            cycle_tx: stream.cycle_tx,
            latest_rx: stream.latest_rx,
            latest_tx: stream.latest_tx,
            last_sample_source: stream.last_sample_source.clone(),
            last_sample_unix: stream.last_sample_unix,
            rx_counter_epochs_seen: stream.rx_counter_epochs_seen,
            tx_counter_epochs_seen: stream.tx_counter_epochs_seen,
        })
        .collect()
}

fn projected_traffic_frontier_from_usage(
    requests: &[TrafficStreamRequest],
    usage: &[TrafficCounterStreamUsage],
) -> ProjectedTrafficAccountingFrontier {
    let cycle_starts = requests
        .iter()
        .map(|request| {
            (
                (request.source_kind.as_str(), request.interface.as_str()),
                request.cycle_start_unix,
            )
        })
        .collect::<HashMap<_, _>>();
    usage
        .iter()
        .filter_map(|stream| {
            let cycle_start_unix = cycle_starts
                .get(&(stream.source_kind.as_str(), stream.interface.as_str()))
                .copied()?;
            Some(ProjectedTrafficAccountingFrontierStream {
                source_kind: stream.source_kind.clone(),
                interface: stream.interface.clone(),
                cycle_start_unix,
                cycle_rx: stream.cycle_rx,
                cycle_tx: stream.cycle_tx,
                latest_rx: stream.latest_rx,
                latest_tx: stream.latest_tx,
                last_sample_source: stream.last_sample_source.clone(),
                last_sample_unix: stream.last_sample_unix,
                rx_counter_epochs_seen: stream.rx_counter_epochs_seen,
                tx_counter_epochs_seen: stream.tx_counter_epochs_seen,
            })
        })
        .collect()
}

/// Advances a coherent compact frontier with exactly one admitted sample.  A
/// request/cycle mismatch asks the caller to rebase from the minute owner; it
/// is never papered over by a partial vector or a delayed policy evaluation.
pub(crate) fn advance_projected_traffic_accounting_frontier(
    context: &ProjectedTrafficAccountingContext,
    as_of: DateTime<Utc>,
    metrics: &AgentMetrics,
    projected_streams: &HashSet<TrafficStreamIdentity>,
    overlay: &ProjectedTrafficCounterOverlay,
    frontier: &ProjectedTrafficAccountingFrontier,
) -> Result<Option<(TrafficAccountingRecord, ProjectedTrafficAccountingFrontier)>> {
    let (requests, inventory) = context.requests_and_inventory(as_of, metrics)?;
    if !frontier_requests_match(frontier, &requests) {
        return Ok(None);
    }
    let mut usage = usage_from_projected_traffic_frontier(&context.client_id, frontier);
    apply_projected_traffic_counter_overlay(&mut usage, &requests, overlay, as_of);
    let traffic = context.accounting_record(as_of, &usage, &inventory, projected_streams);
    let frontier = projected_traffic_frontier_from_usage(&requests, &usage);
    Ok(Some((traffic, frontier)))
}

/// Reconstructs a frontier from the durable minute snapshot plus the exact
/// ordered raw suffix that has not crossed that cursor.  One snapshot query
/// per request class replaces the former full accounting query per sample.
pub(crate) async fn rebase_projected_traffic_accounting_frontier_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    context: &ProjectedTrafficAccountingContext,
    as_of: DateTime<Utc>,
    metrics: &AgentMetrics,
    projected_streams: &HashSet<TrafficStreamIdentity>,
    overlays: &[ProjectedTrafficCounterOverlay],
) -> Result<(TrafficAccountingRecord, ProjectedTrafficAccountingFrontier)> {
    let (requests, inventory) = context.requests_and_inventory(as_of, metrics)?;
    let mut usage =
        postgres_traffic_counter_usage_snapshot_in_tx(tx, &requests, as_of.timestamp()).await?;
    for overlay in overlays {
        apply_projected_traffic_counter_overlay(&mut usage, &requests, overlay, as_of);
    }
    let traffic = context.accounting_record(as_of, &usage, &inventory, projected_streams);
    let frontier = projected_traffic_frontier_from_usage(&requests, &usage);
    Ok((traffic, frontier))
}

fn network_interface_inventory_from_metrics(
    metrics: &AgentMetrics,
    traffic_streams: BTreeSet<TrafficStreamIdentity>,
    current_tunnel_interfaces: &HashSet<String>,
) -> NetworkInterfaceInventory {
    NetworkInterfaceInventory {
        traffic_streams,
        current_host_interfaces: metrics
            .networks
            .iter()
            .filter(|network| (1..=64).contains(&network.interface.len()))
            .map(|network| network.interface.clone())
            .collect(),
        current_tunnel_interfaces: current_tunnel_interfaces.iter().cloned().collect(),
    }
}

async fn postgres_traffic_stream_identities_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
) -> Result<BTreeSet<TrafficStreamIdentity>> {
    let rows = sqlx::query(
        r#"
        SELECT source_kind, interface
        FROM traffic_counter_streams
        WHERE client_id = $1
        ORDER BY source_kind, interface
        "#,
    )
    .bind(client_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| Ok((row.try_get("source_kind")?, row.try_get("interface")?)))
        .collect()
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
        // This is the same plan-sensitive statement used by the public traffic
        // accounting path.  Ingest runs it inside the transaction that owns the
        // accepted sample, so apply the same transaction-local safeguards here:
        // JIT compilation costs more than a healthy bounded read, and a cached
        // generic UNNEST plan can multiply the bounded probes in a poor join order.
        sqlx::query(
            r#"SELECT set_config('jit', 'off', true),
                      set_config('plan_cache_mode', 'force_custom_plan', true)"#,
        )
        .execute(&mut **tx)
        .await?;
        let rows = sqlx::query(NO_RESET_TRAFFIC_COUNTER_USAGE_SQL)
            .persistent(false)
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
    interface_inventories: &HashMap<String, NetworkInterfaceInventory>,
) -> Result<BTreeSet<TrafficStreamRequest>> {
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
        let policy = network_interface_policy_for_client(&rule.client_id, rules)?;
        let selectors = eligible_traffic_selectors(
            traffic_selector_spec_from_rule(rule)?,
            &policy,
            interface_inventories.get(&rule.client_id),
        );
        for selector in selectors {
            requests.insert(TrafficStreamRequest {
                client_id: rule.client_id.clone(),
                source_kind: selector.source,
                interface: selector.interface,
                cycle_start_unix,
            });
        }
    }
    Ok(requests)
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct MemoryCounterEpochEndpoints<'a> {
    first: &'a TrafficCounterSampleRecord,
    last: &'a TrafficCounterSampleRecord,
}

#[cfg(test)]
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

#[cfg(test)]
#[derive(Default)]
struct MemoryNoResetStreamAccumulator<'a> {
    latest: Option<&'a TrafficCounterSampleRecord>,
    rx_epochs: BTreeMap<i64, MemoryCounterEpochEndpoints<'a>>,
    tx_epochs: BTreeMap<i64, MemoryCounterEpochEndpoints<'a>>,
}

#[cfg(test)]
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

#[cfg(test)]
fn test_no_reset_direction_usage(
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

#[cfg(test)]
fn aggregate_test_no_reset_traffic_counter_usage(
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
                test_no_reset_direction_usage(&accumulator.rx_epochs, true);
            let (mut cycle_tx, mut tx_counter_epochs_seen) =
                test_no_reset_direction_usage(&accumulator.tx_epochs, false);
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
                last_sample_source: latest.sample_source.clone(),
                last_sample_unix: latest.observed_unix,
                rx_counter_epochs_seen,
                tx_counter_epochs_seen,
            })
        })
        .collect()
}

#[cfg(test)]
fn aggregate_test_traffic_counter_usage(
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
    let mut rows = aggregate_test_no_reset_traffic_counter_usage(
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
        let last_sample_source = selected
            .iter()
            .max_by_key(|sample| sample.observed_unix)
            .expect("non-empty selected traffic samples have a latest source")
            .sample_source
            .clone();
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
            last_sample_source,
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

#[cfg(test)]
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

#[cfg(test)]
fn aggregate_test_traffic_history(
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
    let parsed = parse_vps_rule_value(&rule.key, &rule.value_raw)?;
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

fn traffic_selector_spec_from_parsed_rule(parsed: &ParsedRuleValue) -> Result<TrafficSelectorSpec> {
    match parsed.json.get("mode").and_then(Value::as_str) {
        Some("all") => Ok(TrafficSelectorSpec::All),
        Some("exact") => Ok(TrafficSelectorSpec::Exact(
            traffic_selectors_from_parsed_rule(parsed)?,
        )),
        _ => anyhow::bail!("traffic_selector_storage_invalid"),
    }
}

fn traffic_selector_spec_from_rule(rule: &VpsRuleValueRecord) -> Result<TrafficSelectorSpec> {
    anyhow::ensure!(
        rule.key == VPS_RULE_KEY_TRAFFIC_SELECTORS,
        "traffic_selector_storage_invalid"
    );
    parse_traffic_selector_spec(&rule.value_raw)
}

fn resolve_network_rate_interface_selection(
    client_ids: &[String],
    rules: &[VpsRuleValueRecord],
    interface_inventories: &HashMap<String, NetworkInterfaceInventory>,
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
        let policy = network_interface_policy_from_rules(client_rules)?;
        let inventory = interface_inventories.get(client_id);
        let rate_rule = client_rules
            .and_then(|rules| rules.get(VPS_RULE_KEY_NETWORK_RATE_INTERFACES))
            .copied();
        let Some(rate_rule) = rate_rule else {
            selection.select_exact(client_id.clone(), BTreeSet::new());
            continue;
        };
        let spec = network_rate_selector_spec_from_rule(rate_rule)?;
        match spec {
            NetworkRateSelectorSpec::All => selection.select_exact(
                client_id.clone(),
                all_eligible_host_rate_interfaces(&policy, inventory),
            ),
            NetworkRateSelectorSpec::Exact(selectors) => selection.select_exact(
                client_id.clone(),
                host_rate_interfaces(&selectors, &policy, inventory),
            ),
            NetworkRateSelectorSpec::Reference(NetworkRateSelectorReference::TrafficSelectors) => {
                let inherited = match client_rules
                    .and_then(|rules| rules.get(VPS_RULE_KEY_TRAFFIC_SELECTORS))
                {
                    Some(rule) => match traffic_selector_spec_from_rule(rule)? {
                        TrafficSelectorSpec::All => {
                            all_eligible_host_rate_interfaces(&policy, inventory)
                        }
                        TrafficSelectorSpec::Exact(selectors) => {
                            host_rate_interfaces(&selectors, &policy, inventory)
                        }
                    },
                    None => BTreeSet::new(),
                };
                selection.select_exact(client_id.clone(), inherited);
            }
        }
    }
    Ok(selection)
}

fn network_interface_policy_from_rules(
    rules: Option<&HashMap<&str, &VpsRuleValueRecord>>,
) -> Result<NetworkInterfacePolicy> {
    let parsed = rules
        .and_then(|rules| rules.get(VPS_RULE_KEY_NETWORK_INTERFACES))
        .map(|rule| parse_vps_rule_value(&rule.key, &rule.value_raw))
        .transpose()?;
    NetworkInterfacePolicy::from_rule_json(parsed.as_ref().map(|value| &value.json))
        .map_err(anyhow::Error::msg)
}

fn network_interface_policy_for_client(
    client_id: &str,
    rules: &[VpsRuleValueRecord],
) -> Result<NetworkInterfacePolicy> {
    let parsed = rules
        .iter()
        .find(|rule| rule.client_id == client_id && rule.key == VPS_RULE_KEY_NETWORK_INTERFACES)
        .map(|rule| parse_vps_rule_value(&rule.key, &rule.value_raw))
        .transpose()?;
    NetworkInterfacePolicy::from_rule_json(parsed.as_ref().map(|value| &value.json))
        .map_err(anyhow::Error::msg)
}

fn known_stream_is_admitted(
    policy: &NetworkInterfacePolicy,
    inventory: Option<&NetworkInterfaceInventory>,
    source: NetworkInterfaceSource,
    interface: &str,
) -> bool {
    if !policy.matches(source, interface) {
        return false;
    }
    !(*policy == NetworkInterfacePolicy::DefaultPhysical
        && source == NetworkInterfaceSource::Host
        && inventory
            .is_some_and(|inventory| inventory.current_tunnel_interfaces.contains(interface)))
}

fn traffic_selector_source(source: &str) -> Option<NetworkInterfaceSource> {
    match source {
        "host" => Some(NetworkInterfaceSource::Host),
        "tunnel" => Some(NetworkInterfaceSource::Tunnel),
        _ => None,
    }
}

fn eligible_traffic_selectors(
    spec: TrafficSelectorSpec,
    policy: &NetworkInterfacePolicy,
    inventory: Option<&NetworkInterfaceInventory>,
) -> Vec<TrafficSelector> {
    match spec {
        TrafficSelectorSpec::Exact(selectors) => selectors
            .into_iter()
            .filter(|selector| {
                traffic_selector_source(&selector.source).is_some_and(|source| {
                    known_stream_is_admitted(policy, inventory, source, &selector.interface)
                })
            })
            .collect(),
        TrafficSelectorSpec::All => inventory
            .into_iter()
            .flat_map(|inventory| &inventory.traffic_streams)
            .filter_map(|(source, interface)| {
                let source_kind = traffic_selector_source(source)?;
                known_stream_is_admitted(policy, inventory, source_kind, interface).then(|| {
                    TrafficSelector {
                        source: source.clone(),
                        interface: interface.clone(),
                        direction: "total".to_string(),
                        canonical: if source == "host" {
                            interface.clone()
                        } else {
                            format!("{source}:{interface}")
                        },
                    }
                })
            })
            .collect(),
    }
}

fn all_eligible_host_rate_interfaces(
    policy: &NetworkInterfacePolicy,
    inventory: Option<&NetworkInterfaceInventory>,
) -> BTreeSet<String> {
    inventory
        .into_iter()
        .flat_map(|inventory| &inventory.current_host_interfaces)
        .filter(|interface| {
            known_stream_is_admitted(policy, inventory, NetworkInterfaceSource::Host, interface)
        })
        .cloned()
        .collect()
}

fn host_rate_interfaces(
    selectors: &[TrafficSelector],
    policy: &NetworkInterfacePolicy,
    inventory: Option<&NetworkInterfaceInventory>,
) -> BTreeSet<String> {
    let mut selected = BTreeSet::new();
    for selector in selectors.iter().filter(|selector| {
        selector.source == "host"
            && known_stream_is_admitted(
                policy,
                inventory,
                NetworkInterfaceSource::Host,
                &selector.interface,
            )
    }) {
        selected.insert(selector.interface.clone());
    }
    selected
}

fn parse_traffic_selector_list(input: &str) -> Result<Vec<TrafficSelector>> {
    let parsed = parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_SELECTORS, input)?;
    match traffic_selector_spec_from_parsed_rule(&parsed)? {
        TrafficSelectorSpec::Exact(selectors) => Ok(selectors),
        TrafficSelectorSpec::All => anyhow::bail!("traffic_selector_expansion_required"),
    }
}

fn parse_traffic_selector_spec(input: &str) -> Result<TrafficSelectorSpec> {
    let parsed = parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_SELECTORS, input)?;
    traffic_selector_spec_from_parsed_rule(&parsed)
}

#[cfg(test)]
fn parse_traffic_selector(input: &str) -> Result<TrafficSelector> {
    let mut selectors = parse_traffic_selector_list(input)?;
    anyhow::ensure!(selectors.len() == 1, "traffic_selector_single_required");
    Ok(selectors.remove(0))
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

#[cfg(test)]
fn traffic_accounting_for_client(
    client_id: &str,
    rules: &[VpsRuleValueRecord],
    traffic_usage: &[TrafficCounterStreamUsage],
    now: DateTime<Utc>,
) -> TrafficAccountingRecord {
    let inventory = network_interface_inventory_from_usage(client_id, traffic_usage);
    traffic_accounting_for_client_with_freshness_and_candidates(
        client_id,
        rules,
        traffic_usage,
        now,
        None,
        None,
        Some(&inventory),
    )
}

fn traffic_accounting_for_agents(
    agents: &[AgentView],
    rules: &[VpsRuleValueRecord],
    traffic_usage: &[TrafficCounterStreamUsage],
    now: DateTime<Utc>,
    projected_streams: &HashMap<String, HashSet<TrafficStreamIdentity>>,
    interface_inventories: &HashMap<String, NetworkInterfaceInventory>,
) -> Vec<TrafficAccountingRecord> {
    agents
        .iter()
        .map(|agent| {
            traffic_accounting_for_client_with_freshness_and_candidates(
                &agent.id,
                rules,
                traffic_usage,
                now,
                None,
                Some(TrafficFreshnessBoundary {
                    projected_streams: projected_streams.get(&agent.id),
                    online: agent.status == "online",
                }),
                interface_inventories.get(&agent.id),
            )
        })
        .collect()
}

#[cfg(test)]
fn traffic_accounting_for_client_with_selector_override(
    client_id: &str,
    rules: &[VpsRuleValueRecord],
    traffic_usage: &[TrafficCounterStreamUsage],
    now: DateTime<Utc>,
    selector_override: Option<&str>,
) -> TrafficAccountingRecord {
    let inventory = network_interface_inventory_from_usage(client_id, traffic_usage);
    traffic_accounting_for_client_with_freshness_and_candidates(
        client_id,
        rules,
        traffic_usage,
        now,
        selector_override,
        None,
        Some(&inventory),
    )
}

#[cfg(test)]
fn network_interface_inventory_from_usage(
    client_id: &str,
    traffic_usage: &[TrafficCounterStreamUsage],
) -> NetworkInterfaceInventory {
    let mut inventory = NetworkInterfaceInventory::default();
    for usage in traffic_usage
        .iter()
        .filter(|usage| usage.client_id == client_id)
    {
        inventory
            .traffic_streams
            .insert((usage.source_kind.clone(), usage.interface.clone()));
        if usage.source_kind == "host" {
            inventory
                .current_host_interfaces
                .insert(usage.interface.clone());
        } else if usage.source_kind == "tunnel" {
            inventory
                .current_tunnel_interfaces
                .insert(usage.interface.clone());
        }
    }
    inventory
}

/// Current traffic is exact stream membership in the canonical projected
/// telemetry sample, not a fixed wall-age or timestamp-equality window.
/// `online` preserves last-known counters for diagnosis while marking a
/// disconnected client's evidence non-current. A missing projected sample
/// cannot make imported/historical counters current.
#[derive(Clone, Copy)]
struct TrafficFreshnessBoundary<'a> {
    projected_streams: Option<&'a HashSet<TrafficStreamIdentity>>,
    online: bool,
}

#[cfg(test)]
fn traffic_accounting_for_client_with_freshness(
    client_id: &str,
    rules: &[VpsRuleValueRecord],
    traffic_usage: &[TrafficCounterStreamUsage],
    now: DateTime<Utc>,
    selector_override: Option<&str>,
    freshness: Option<TrafficFreshnessBoundary<'_>>,
) -> TrafficAccountingRecord {
    let inventory = network_interface_inventory_from_usage(client_id, traffic_usage);
    traffic_accounting_for_client_with_freshness_and_candidates(
        client_id,
        rules,
        traffic_usage,
        now,
        selector_override,
        freshness,
        Some(&inventory),
    )
}

fn traffic_accounting_for_client_with_freshness_and_candidates(
    client_id: &str,
    rules: &[VpsRuleValueRecord],
    traffic_usage: &[TrafficCounterStreamUsage],
    now: DateTime<Utc>,
    selector_override: Option<&str>,
    freshness: Option<TrafficFreshnessBoundary<'_>>,
    interface_inventory: Option<&NetworkInterfaceInventory>,
) -> TrafficAccountingRecord {
    let rule_map = rules
        .iter()
        .filter(|rule| rule.client_id == client_id)
        .map(|rule| (rule.key.as_str(), rule))
        .collect::<HashMap<_, _>>();
    let mut incomplete_reasons = Vec::new();
    let reset_boundary = rule_map
        .get(VPS_RULE_KEY_TRAFFIC_RESET_DAY)
        .and_then(|rule| parsed_traffic_reset(rule));
    let reset_day = reset_boundary.map(|(day, _)| day);
    let reset_hour = reset_boundary.and_then(|(day, hour)| (day != -1).then_some(hour));
    if reset_day.is_none() {
        incomplete_reasons.push("traffic.reset_day missing".to_string());
    }
    let (selector_spec, selector_error, selector_configured) = match selector_override {
        Some(selector) => match parse_traffic_selector_list(selector) {
            Ok(selectors) => (Some(TrafficSelectorSpec::Exact(selectors)), None, true),
            Err(error) => (
                None,
                Some(format!("traffic.policy_selector invalid: {error}")),
                true,
            ),
        },
        None => match rule_map.get(VPS_RULE_KEY_TRAFFIC_SELECTORS) {
            Some(rule) => match traffic_selector_spec_from_rule(rule) {
                Ok(spec) => (Some(spec), None, true),
                Err(error) => (
                    None,
                    Some(format!("traffic.selectors invalid: {error}")),
                    true,
                ),
            },
            None => (None, None, false),
        },
    };
    let selectors = if let Some(spec) = selector_spec {
        match network_interface_policy_from_rules(Some(&rule_map)) {
            Ok(policy) => eligible_traffic_selectors(spec, &policy, interface_inventory),
            Err(error) => {
                incomplete_reasons.push(format!("network.interfaces invalid: {error}"));
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    if let Some(error) = selector_error {
        incomplete_reasons.push(error);
    } else if selectors.is_empty() {
        incomplete_reasons.push(if selector_configured {
            "traffic.selectors have no eligible interfaces".to_string()
        } else {
            "traffic.selectors missing".to_string()
        });
    }
    let quota_total = quota_value(&rule_map, VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL);
    let quota_rx = quota_value(&rule_map, VPS_RULE_KEY_TRAFFIC_QUOTA_RX);
    let quota_tx = quota_value(&rule_map, VPS_RULE_KEY_TRAFFIC_QUOTA_TX);
    let cycle_bounds = (reset_day != Some(-1))
        .then(|| cycle_bounds(reset_day.unwrap_or(1), reset_hour.unwrap_or(0), now));
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
        if freshness.is_some_and(|boundary| {
            !boundary.online
                || boundary.projected_streams.is_none_or(|streams| {
                    !projected_traffic_stream_contains(
                        streams,
                        &selector.source,
                        &selector.interface,
                    )
                })
        }) {
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
        reset_hour,
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

#[cfg(test)]
#[derive(Default)]
struct CycleUsage {
    cycle_rx: i64,
    cycle_tx: i64,
    latest_rx: i64,
    latest_tx: i64,
    last_sample_unix: Option<i64>,
}

#[cfg(test)]
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
        .and_then(|rule| parse_vps_rule_value(&rule.key, &rule.value_raw).ok())
        .and_then(|parsed| parsed.json.get("bytes").and_then(Value::as_i64))
}

fn percent(value: i64, quota: i64) -> f64 {
    if quota <= 0 {
        0.0
    } else {
        (value as f64 / quota as f64) * 100.0
    }
}

fn cycle_bounds(
    reset_day: i32,
    reset_hour: i32,
    now: DateTime<Utc>,
) -> (DateTime<Utc>, DateTime<Utc>) {
    let current_boundary = boundary_for_month(now.year(), now.month(), reset_day, reset_hour);
    if now >= current_boundary {
        let (next_year, next_month) = if now.month() == 12 {
            (now.year() + 1, 1)
        } else {
            (now.year(), now.month() + 1)
        };
        (
            current_boundary,
            boundary_for_month(next_year, next_month, reset_day, reset_hour),
        )
    } else {
        let (prev_year, prev_month) = if now.month() == 1 {
            (now.year() - 1, 12)
        } else {
            (now.year(), now.month() - 1)
        };
        (
            boundary_for_month(prev_year, prev_month, reset_day, reset_hour),
            current_boundary,
        )
    }
}

fn boundary_for_month(year: i32, month: u32, reset_day: i32, reset_hour: i32) -> DateTime<Utc> {
    let day = reset_day.clamp(1, days_in_month(year, month) as i32) as u32;
    Utc.with_ymd_and_hms(year, month, day, reset_hour.clamp(0, 23) as u32, 0, 0)
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

#[cfg(test)]
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
            return policy_evaluation_from_parts(false, incomplete_reasons, None, None);
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
    )
}

#[cfg(test)]
fn policy_evaluation_from_parts(
    condition_true: bool,
    incomplete_reasons: Vec<String>,
    actual_value: Option<f64>,
    threshold_value: Option<f64>,
) -> PolicyEvaluation {
    PolicyEvaluation {
        condition_true,
        incomplete: !incomplete_reasons.is_empty(),
        incomplete_reasons,
        actual_value,
        threshold_value,
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
#[cfg(test)]
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

#[cfg(test)]
fn trigger_sustained_seconds(condition: &Option<AlertPolicyMetaCondition>) -> Option<i64> {
    match condition {
        Some(AlertPolicyMetaCondition::Sustained { seconds }) => Some(*seconds),
        _ => None,
    }
}

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
fn push_incomplete(reasons: &mut Vec<String>, reason: impl AsRef<str>) {
    let reason = reason.as_ref();
    if !reasons.iter().any(|stored| stored == reason) {
        reasons.push(reason.to_string());
    }
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

fn vps_rule_from_row(row: sqlx::postgres::PgRow) -> Result<VpsRuleValueRecord> {
    let key: String = row.try_get("key")?;
    let raw: String = row.try_get("value_raw")?;
    let stored_json = row.try_get::<SqlJson<Value>, _>("value_json")?.0;
    let parsed = parse_vps_rule_value(&key, &raw)?;
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
            e.last_confirmed_at::text AS observed_at,
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
          AND ($14::boolean OR e.resolved_at IS NULL)
          AND ($9::boolean=FALSE OR e.lifecycle_state IN ('triggered','persisting'))
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

#[cfg(test)]
#[path = "tests_repository_alert_policies.rs"]
mod tests;
