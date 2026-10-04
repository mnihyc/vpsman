use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
#[cfg(test)]
use chrono::{TimeZone, Utc};
use sqlx::{postgres::PgRow, Postgres, Row};
use uuid::Uuid;
use vpsman_common::{
    NetworkTrafficImportBucket, NetworkTrafficImportResult,
    NETWORK_TRAFFIC_IMPORT_MAX_BUCKETS_PER_INTERFACE, NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES,
    TRAFFIC_COUNTER_RAW_RETENTION_DAYS,
};

#[cfg(test)]
use crate::model_alert_policies::TrafficCounterRollupRecord;
#[cfg(test)]
use crate::model_alert_policies::TrafficCounterSampleRecord;
use crate::repository::Repository;

pub(crate) const VNSTAT_IMPORT_SOURCE_PREFIX: &str = "vnstat_import:";
const MAX_IMPORT_BUCKET_DURATION_SECS: u64 = 367 * 24 * 60 * 60;
const POSTGRES_IMPORT_MAX_PREPARATION_ATTEMPTS: usize = 3;
// The hour-aligned raw frontier preserves every exact point in the configured
// raw-retention window.
// At most one partial boundary hour plus one legacy sequencing predecessor sits
// beside that window. Preserve oversized streams (including pending retention)
// rather than expanding the read or widening the replacement range.
const POSTGRES_IMPORT_MAX_RAW_ROWS_PER_INTERFACE: usize =
    (TRAFFIC_COUNTER_RAW_RETENTION_DAYS as usize * 24 + 1) * 60 + 1;
const POSTGRES_IMPORT_WORK_MEM_SQL: &str = "SET LOCAL work_mem = '32MB'";
#[derive(Clone, Debug)]
pub(crate) struct NetworkTrafficImportSummary {
    pub(crate) message: String,
}

#[derive(Clone, Debug)]
struct PreparedInterfaceImport {
    interface: String,
    start_unix: u64,
    end_unix: u64,
    initial_rx_bytes: i64,
    initial_tx_bytes: i64,
    include_baseline: bool,
    traffic: ExpandedMinuteTraffic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MinuteAssignmentSegment {
    start_unix: u64,
    end_unix: u64,
    rx_bytes: u64,
    tx_bytes: u64,
}

#[derive(Clone, Debug)]
struct ExpandedMinuteTraffic {
    segments: Vec<MinuteAssignmentSegment>,
    minute_count: u64,
    total_rx_bytes: u64,
    total_tx_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PreparedImportRollup {
    interface: String,
    bucket_secs: i32,
    bucket_start_unix: u64,
    rx_bytes: u64,
    tx_bytes: u64,
    rx_valid_count: u32,
    tx_valid_count: u32,
    any_valid_count: u32,
    first_observed_unix: u64,
    latest_observed_unix: u64,
}

#[derive(Debug)]
struct PreparedImportRollupRows {
    bucket_secs: Vec<i32>,
    bucket_start_unix: Vec<i64>,
    rx_bytes: Vec<i64>,
    tx_bytes: Vec<i64>,
    rx_valid_count: Vec<i32>,
    tx_valid_count: Vec<i32>,
    any_valid_count: Vec<i32>,
    first_observed_unix: Vec<i64>,
    latest_observed_unix: Vec<i64>,
}

#[derive(Clone, Debug)]
struct PreparedImportRawRows {
    observed_unix: Vec<i64>,
    rx_bytes: Vec<i64>,
    tx_bytes: Vec<i64>,
    inbound_promoted: Vec<bool>,
}

#[derive(Debug)]
struct AssignmentState {
    assigned_rx_bytes: u64,
    assigned_tx_bytes: u64,
    uncovered_ranges: Vec<(u64, u64)>,
}

impl Repository {
    pub(crate) async fn import_vnstat_traffic_history(
        &self,
        job_id: Uuid,
        client_id: &str,
        interfaces: &[String],
        start_unix: u64,
        result: &NetworkTrafficImportResult,
        buckets: &[NetworkTrafficImportBucket],
        now_unix: u64,
    ) -> Result<NetworkTrafficImportSummary> {
        validate_result_contract(interfaces, start_unix, result, buckets, now_unix)?;
        ranges::import(
            self, job_id, client_id, interfaces, start_unix, result, buckets,
        )
        .await
    }
}

#[path = "repository_network_traffic_import_ranges.rs"]
pub(crate) mod ranges;

/// Rechecks the durable-interface boundary before publishing a prepared patch.
/// Excluded interfaces are normally preserved during preparation; the caller's
/// transaction ensures that a changed policy cannot leave a partial import.
pub(crate) async fn ensure_postgres_vnstat_interfaces_admitted(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    client_id: &str,
    interfaces: &[String],
) -> Result<()> {
    let rejected = sqlx::query_scalar::<_, Option<Vec<String>>>(
        r#"
        WITH policy AS MATERIALIZED (
            SELECT *
            FROM public.resolve_telemetry_interface_policies(ARRAY[$1])
        ), rejected AS (
            SELECT requested.interface
            FROM unnest($2::TEXT[]) requested(interface)
            CROSS JOIN policy
            WHERE NOT public.telemetry_interface_is_admitted_resolved(
                policy.admission_mode,
                policy.interface_patterns,
                policy.managed_tunnel_interfaces,
                'host',
                requested.interface
            )
        )
        SELECT array_agg(interface ORDER BY interface)
        FROM rejected
        "#,
    )
    .bind(client_id)
    .bind(interfaces)
    .fetch_one(&mut **tx)
    .await?;
    if let Some(rejected) = rejected.filter(|interfaces| !interfaces.is_empty()) {
        anyhow::bail!(
            "network_traffic_import_invalid:interface_excluded_by_network_policy:{}",
            rejected.join(",")
        );
    }
    Ok(())
}

async fn lock_postgres_traffic_import_client(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    client_id: &str,
) -> Result<()> {
    sqlx::query_scalar::<_, String>("SELECT id FROM clients WHERE id = $1 FOR UPDATE")
        .bind(client_id)
        .fetch_optional(&mut **tx)
        .await?
        .context("network_traffic_import_client_not_found")?;
    Ok(())
}

async fn postgres_import_retention_boundaries(
    tx: &mut sqlx::Transaction<'_, Postgres>,
) -> Result<(u64, u64)> {
    let (utc_day_start_unix, raw_cutoff_unix): (i64, i64) = sqlx::query_as(
        r#"
        WITH boundary AS (
            SELECT date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                AS utc_day_start
        )
        SELECT
            extract(epoch FROM utc_day_start)::bigint,
            extract(epoch FROM date_bin(
                interval '1 hour', now() - make_interval(days => $1),
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
            ))::bigint
        FROM boundary
        "#,
    )
    .bind(TRAFFIC_COUNTER_RAW_RETENTION_DAYS)
    .fetch_one(&mut **tx)
    .await?;
    Ok((
        u64::try_from(utc_day_start_unix)
            .context("network_traffic_import_invalid:utc_day_start_out_of_range")?,
        u64::try_from(raw_cutoff_unix)
            .context("network_traffic_import_invalid:raw_cutoff_out_of_range")?,
    ))
}

pub(crate) async fn lock_postgres_traffic_counter_streams(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    client_id: &str,
) -> Result<()> {
    let lock_key = format!("traffic-counters:{client_id}");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(lock_key)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn validate_result_contract(
    interfaces: &[String],
    start_unix: u64,
    result: &NetworkTrafficImportResult,
    buckets: &[NetworkTrafficImportBucket],
    now_unix: u64,
) -> Result<()> {
    invalid_ensure(
        result.r#type == "network_traffic_import_vnstat" && result.status == "collected",
        "agent_result_type_invalid",
    )?;
    invalid_ensure(
        result.requested_start_unix == start_unix,
        "agent_result_start_mismatch",
    )?;
    invalid_ensure(
        result.collected_until_unix.is_multiple_of(60)
            && result.collected_until_unix <= floor_minute(now_unix.saturating_add(300)),
        "agent_result_collection_time_invalid",
    )?;
    invalid_ensure(
        interfaces.len() <= NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES,
        "interface_count_out_of_range",
    )?;
    invalid_ensure(
        start_unix >= 60 && start_unix.is_multiple_of(60),
        "start_not_minute_aligned",
    )?;
    invalid_ensure(start_unix < floor_minute(now_unix), "start_not_in_past")?;

    invalid_ensure(
        interfaces
            .iter()
            .all(|selector| vpsman_common::valid_network_traffic_import_selector(selector)),
        "interface_selector_invalid",
    )?;
    let requested = interfaces.iter().cloned().collect::<BTreeSet<_>>();
    invalid_ensure(requested.len() == interfaces.len(), "duplicate_interface")?;
    let result_interfaces = result.interfaces.iter().cloned().collect::<BTreeSet<_>>();
    invalid_ensure(
        result.interfaces.len() <= NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES
            && result_interfaces.len() == result.interfaces.len(),
        "agent_result_interface_count_out_of_range",
    )?;
    invalid_ensure(
        result.interfaces.iter().all(|name| {
            !name.contains('*') && vpsman_common::valid_network_traffic_import_selector(name)
        }),
        "agent_result_interface_invalid",
    )?;
    if !interfaces.is_empty() {
        invalid_ensure(
            result_interfaces.iter().all(|interface| {
                requested.iter().any(|selector| {
                    vpsman_common::network_interface_pattern_matches(selector, interface)
                })
            }),
            "agent_result_interface_mismatch",
        )?;
    }
    let sources = result
        .sources
        .iter()
        .map(|source| source.interface.clone())
        .collect::<BTreeSet<_>>();
    invalid_ensure(
        sources.is_subset(&result_interfaces) && sources.len() == result.sources.len(),
        "source_interface_mismatch",
    )?;
    invalid_ensure(
        buckets.len() <= result.interfaces.len() * NETWORK_TRAFFIC_IMPORT_MAX_BUCKETS_PER_INTERFACE,
        "bucket_count_exceeds_limit",
    )?;
    invalid_ensure(
        u32::try_from(buckets.len()).ok() == Some(result.bucket_count),
        "bucket_count_mismatch",
    )?;
    invalid_ensure(
        buckets
            .iter()
            .all(|bucket| sources.contains(&bucket.interface)),
        "bucket_interface_mismatch",
    )?;
    for interface in &result.interfaces {
        invalid_ensure(
            buckets
                .iter()
                .filter(|bucket| bucket.interface == *interface)
                .count()
                <= NETWORK_TRAFFIC_IMPORT_MAX_BUCKETS_PER_INTERFACE,
            "interface_bucket_count_exceeds_limit",
        )?;
    }
    for source in &result.sources {
        let database_created_unix = source
            .database_created_unix
            .context("network_traffic_import_invalid:vnstat_database_created_missing")?;
        let database_available_unix = ceil_minute(database_created_unix)
            .context("network_traffic_import_invalid:vnstat_database_created_out_of_range")?;
        let source_updated_unix = source
            .source_updated_unix
            .map(floor_minute)
            .context("network_traffic_import_invalid:vnstat_source_updated_missing")?;
        invalid_ensure(
            source.retained_start_unix.is_multiple_of(60)
                && source.retained_start_unix >= database_available_unix
                && source.retained_start_unix < source_updated_unix,
            "vnstat_retained_start_invalid",
        )?;
        let (derived_start_unix, derived_end_unix) =
            latest_continuous_coverage(buckets, &source.interface)?;
        invalid_ensure(
            source.retained_start_unix == derived_start_unix
                && derived_end_unix <= source_updated_unix,
            "vnstat_retained_coverage_mismatch",
        )?;
    }
    Ok(())
}

fn prepare_import_rollup_rows(
    interface: &str,
    rollups: Vec<PreparedImportRollup>,
) -> Result<PreparedImportRollupRows> {
    let mut rows = PreparedImportRollupRows {
        bucket_secs: Vec::with_capacity(rollups.len()),
        bucket_start_unix: Vec::with_capacity(rollups.len()),
        rx_bytes: Vec::with_capacity(rollups.len()),
        tx_bytes: Vec::with_capacity(rollups.len()),
        rx_valid_count: Vec::with_capacity(rollups.len()),
        tx_valid_count: Vec::with_capacity(rollups.len()),
        any_valid_count: Vec::with_capacity(rollups.len()),
        first_observed_unix: Vec::with_capacity(rollups.len()),
        latest_observed_unix: Vec::with_capacity(rollups.len()),
    };
    for rollup in rollups {
        anyhow::ensure!(
            rollup.interface == interface,
            "network_traffic_import_rollup_interface_mismatch"
        );
        rows.bucket_secs.push(rollup.bucket_secs);
        rows.bucket_start_unix.push(
            i64::try_from(rollup.bucket_start_unix)
                .context("network_traffic_import_invalid:rollup_timestamp_out_of_range")?,
        );
        rows.rx_bytes.push(
            i64::try_from(rollup.rx_bytes)
                .context("network_traffic_import_invalid:rx_delta_exceeds_database_range")?,
        );
        rows.tx_bytes.push(
            i64::try_from(rollup.tx_bytes)
                .context("network_traffic_import_invalid:tx_delta_exceeds_database_range")?,
        );
        rows.rx_valid_count.push(
            i32::try_from(rollup.rx_valid_count)
                .context("network_traffic_import_invalid:rollup_valid_count_out_of_range")?,
        );
        rows.tx_valid_count.push(
            i32::try_from(rollup.tx_valid_count)
                .context("network_traffic_import_invalid:rollup_valid_count_out_of_range")?,
        );
        rows.any_valid_count.push(
            i32::try_from(rollup.any_valid_count)
                .context("network_traffic_import_invalid:rollup_valid_count_out_of_range")?,
        );
        rows.first_observed_unix.push(
            i64::try_from(rollup.first_observed_unix)
                .context("network_traffic_import_invalid:rollup_timestamp_out_of_range")?,
        );
        rows.latest_observed_unix.push(
            i64::try_from(rollup.latest_observed_unix)
                .context("network_traffic_import_invalid:rollup_timestamp_out_of_range")?,
        );
    }
    Ok(rows)
}

fn prepare_import_raw_rows(
    prepared: &PreparedInterfaceImport,
    minimum_unix: u64,
    raw_cutoff_unix: u64,
) -> Result<PreparedImportRawRows> {
    invalid_ensure(
        minimum_unix.is_multiple_of(60) && raw_cutoff_unix.is_multiple_of(60),
        "raw_retention_cutoff_not_minute_aligned",
    )?;
    let natural_start = if prepared.include_baseline {
        prepared.start_unix - 60
    } else {
        prepared.start_unix
    };
    let mut next_unix = minimum_unix.max(natural_start).min(prepared.end_unix);
    let baseline_selected = prepared.include_baseline && next_unix == prepared.start_unix - 60;
    let capacity = usize::try_from((prepared.end_unix - next_unix) / 60)
        .context("network_traffic_import_raw_capacity_out_of_range")?;
    let mut rows = PreparedImportRawRows {
        observed_unix: Vec::with_capacity(capacity),
        rx_bytes: Vec::with_capacity(capacity),
        tx_bytes: Vec::with_capacity(capacity),
        inbound_promoted: Vec::with_capacity(capacity),
    };
    let (mut cumulative_rx, mut cumulative_tx, mut segment_index) = if baseline_selected {
        let observed_unix = i64::try_from(next_unix)
            .context("network_traffic_import_invalid:sample_timestamp_out_of_range")?;
        rows.observed_unix.push(observed_unix);
        rows.rx_bytes.push(prepared.initial_rx_bytes);
        rows.tx_bytes.push(prepared.initial_tx_bytes);
        rows.inbound_promoted.push(next_unix < raw_cutoff_unix);
        next_unix = prepared.start_unix;
        (prepared.initial_rx_bytes, prepared.initial_tx_bytes, 0)
    } else {
        next_unix = next_unix.max(prepared.start_unix);
        let (prefix_rx, prefix_tx) =
            assignment_totals_in_range(&prepared.traffic.segments, prepared.start_unix, next_unix)?;
        let cumulative_rx = prepared
            .initial_rx_bytes
            .checked_add(
                i64::try_from(prefix_rx)
                    .context("network_traffic_import_invalid:rx_counter_overflow")?,
            )
            .context("network_traffic_import_invalid:rx_counter_overflow")?;
        let cumulative_tx = prepared
            .initial_tx_bytes
            .checked_add(
                i64::try_from(prefix_tx)
                    .context("network_traffic_import_invalid:tx_counter_overflow")?,
            )
            .context("network_traffic_import_invalid:tx_counter_overflow")?;
        let segment_index = prepared
            .traffic
            .segments
            .partition_point(|segment| segment.end_unix <= next_unix);
        (cumulative_rx, cumulative_tx, segment_index)
    };

    while next_unix < prepared.end_unix {
        while prepared
            .traffic
            .segments
            .get(segment_index)
            .is_some_and(|segment| segment.end_unix <= next_unix)
        {
            segment_index += 1;
        }
        let segment = prepared
            .traffic
            .segments
            .get(segment_index)
            .context("network_traffic_import_invalid:prepared_history_gap")?;
        invalid_ensure(
            segment.start_unix <= next_unix && segment.end_unix > next_unix,
            "prepared_history_gap",
        )?;
        cumulative_rx = cumulative_rx
            .checked_add(
                i64::try_from(segment.rx_bytes)
                    .context("network_traffic_import_invalid:rx_counter_overflow")?,
            )
            .context("network_traffic_import_invalid:rx_counter_overflow")?;
        cumulative_tx = cumulative_tx
            .checked_add(
                i64::try_from(segment.tx_bytes)
                    .context("network_traffic_import_invalid:tx_counter_overflow")?,
            )
            .context("network_traffic_import_invalid:tx_counter_overflow")?;
        rows.observed_unix.push(
            i64::try_from(next_unix)
                .context("network_traffic_import_invalid:sample_timestamp_out_of_range")?,
        );
        rows.rx_bytes.push(cumulative_rx);
        rows.tx_bytes.push(cumulative_tx);
        rows.inbound_promoted.push(next_unix < raw_cutoff_unix);
        next_unix = next_unix
            .checked_add(60)
            .context("network_traffic_import_invalid:sample_timestamp_overflow")?;
    }
    anyhow::ensure!(
        rows.observed_unix.len() <= POSTGRES_IMPORT_MAX_RAW_ROWS_PER_INTERFACE,
        "network_traffic_import_prepared_raw_rows_exceed_retention_bound"
    );
    anyhow::ensure!(
        rows.observed_unix.windows(2).all(|pair| pair[0] < pair[1]),
        "network_traffic_import_raw_timestamps_not_ordered"
    );
    Ok(rows)
}

fn latest_continuous_coverage(
    buckets: &[NetworkTrafficImportBucket],
    interface: &str,
) -> Result<(u64, u64)> {
    merged_coverage_components(buckets, interface)?
        .into_iter()
        .max_by_key(|(start_unix, end_unix)| (*end_unix, std::cmp::Reverse(*start_unix)))
        .context("network_traffic_import_invalid:vnstat_retained_coverage_missing")
}

fn merged_coverage_components(
    buckets: &[NetworkTrafficImportBucket],
    interface: &str,
) -> Result<Vec<(u64, u64)>> {
    let mut intervals = buckets
        .iter()
        .filter(|bucket| bucket.interface == interface)
        .map(|bucket| {
            let end_unix = bucket
                .start_unix
                .checked_add(u64::from(bucket.duration_secs))
                .context("network_traffic_import_invalid:bucket_end_overflow")?;
            invalid_ensure(
                bucket.start_unix < end_unix
                    && bucket.start_unix.is_multiple_of(60)
                    && end_unix.is_multiple_of(60),
                "bucket_interval_invalid",
            )?;
            Ok((bucket.start_unix, end_unix))
        })
        .collect::<Result<Vec<_>>>()?;
    intervals.sort_unstable();
    let mut components = Vec::<(u64, u64)>::new();
    for (start_unix, end_unix) in intervals {
        if let Some(last) = components.last_mut() {
            if start_unix <= last.1 {
                last.1 = last.1.max(end_unix);
                continue;
            }
        }
        components.push((start_unix, end_unix));
    }
    Ok(components)
}

fn expand_buckets_to_minutes(
    buckets: &[NetworkTrafficImportBucket],
    interface: &str,
    start_unix: u64,
    end_unix: u64,
) -> Result<ExpandedMinuteTraffic> {
    invalid_ensure(end_unix > start_unix, "empty_range")?;
    let mut relevant = Vec::new();
    let mut identities = BTreeSet::new();
    for bucket in buckets
        .iter()
        .filter(|bucket| bucket.interface == interface)
    {
        invalid_ensure(bucket.start_unix % 60 == 0, "bucket_not_minute_aligned")?;
        invalid_ensure(
            bucket.duration_secs >= 60 && bucket.duration_secs % 60 == 0,
            "bucket_duration_invalid",
        )?;
        invalid_ensure(
            u64::from(bucket.duration_secs) <= MAX_IMPORT_BUCKET_DURATION_SECS,
            "bucket_duration_exceeds_limit",
        )?;
        invalid_ensure(
            identities.insert((bucket.start_unix, bucket.duration_secs)),
            "duplicate_bucket",
        )?;
        let bucket_end = bucket
            .start_unix
            .checked_add(u64::from(bucket.duration_secs))
            .context("network_traffic_import_invalid:bucket_end_overflow")?;
        if bucket_end > start_unix && bucket.start_unix < end_unix {
            relevant.push(bucket);
        }
    }
    invalid_ensure(!relevant.is_empty(), "vnstat_history_missing")?;

    relevant.sort_by(|left, right| {
        left.duration_secs
            .cmp(&right.duration_secs)
            .then_with(|| left.start_unix.cmp(&right.start_unix))
    });
    validate_same_resolution_buckets_do_not_overlap(&relevant)?;

    let span_start = relevant
        .iter()
        .map(|bucket| bucket.start_unix)
        .min()
        .context("network_traffic_import_invalid:vnstat_history_missing")?;
    let span_end = relevant
        .iter()
        .map(|bucket| {
            bucket
                .start_unix
                .saturating_add(u64::from(bucket.duration_secs))
        })
        .max()
        .context("network_traffic_import_invalid:vnstat_history_missing")?;
    invalid_ensure(
        span_start <= start_unix && span_end >= end_unix,
        "vnstat_history_does_not_cover_range",
    )?;
    let mut assignments = Vec::new();

    for bucket in relevant {
        let bucket_end = bucket
            .start_unix
            .checked_add(u64::from(bucket.duration_secs))
            .context("network_traffic_import_invalid:bucket_end_overflow")?;
        let state = assignment_state_in_range(&assignments, bucket.start_unix, bucket_end)?;
        invalid_ensure(
            state.assigned_rx_bytes <= bucket.rx_bytes
                && state.assigned_tx_bytes <= bucket.tx_bytes,
            "finer_bucket_total_exceeds_coarse_bucket",
        )?;
        let uncovered = state
            .uncovered_ranges
            .iter()
            .try_fold(0_u64, |total, (start, end)| {
                total
                    .checked_add((end - start) / 60)
                    .context("network_traffic_import_invalid:uncovered_minute_count_overflow")
            })?;
        if uncovered == 0 {
            invalid_ensure(
                state.assigned_rx_bytes == bucket.rx_bytes
                    && state.assigned_tx_bytes == bucket.tx_bytes,
                "fully_covered_bucket_total_mismatch",
            )?;
            continue;
        }

        let added = distribute_residual(
            &state.uncovered_ranges,
            bucket.rx_bytes - state.assigned_rx_bytes,
            bucket.tx_bytes - state.assigned_tx_bytes,
            uncovered,
        )?;
        merge_assignment_segments(&mut assignments, added)?;
    }

    let mut requested_segments = Vec::new();
    let mut cursor = start_unix;
    let mut total_rx = 0_u64;
    let mut total_tx = 0_u64;
    for segment in assignments {
        if segment.end_unix <= cursor || segment.start_unix >= end_unix {
            continue;
        }
        if segment.start_unix > cursor {
            anyhow::bail!("network_traffic_import_invalid:vnstat_history_gap_at_{cursor}");
        }
        let clipped = MinuteAssignmentSegment {
            start_unix: cursor.max(segment.start_unix),
            end_unix: end_unix.min(segment.end_unix),
            rx_bytes: segment.rx_bytes,
            tx_bytes: segment.tx_bytes,
        };
        let minutes = (clipped.end_unix - clipped.start_unix) / 60;
        total_rx = total_rx
            .checked_add(
                clipped
                    .rx_bytes
                    .checked_mul(minutes)
                    .context("network_traffic_import_invalid:rx_total_overflow")?,
            )
            .context("network_traffic_import_invalid:rx_total_overflow")?;
        total_tx = total_tx
            .checked_add(
                clipped
                    .tx_bytes
                    .checked_mul(minutes)
                    .context("network_traffic_import_invalid:tx_total_overflow")?,
            )
            .context("network_traffic_import_invalid:tx_total_overflow")?;
        cursor = clipped.end_unix;
        push_assignment_segment(&mut requested_segments, clipped)?;
        if cursor == end_unix {
            break;
        }
    }
    if cursor < end_unix {
        anyhow::bail!("network_traffic_import_invalid:vnstat_history_gap_at_{cursor}");
    }
    Ok(ExpandedMinuteTraffic {
        segments: requested_segments,
        minute_count: (end_unix - start_unix) / 60,
        total_rx_bytes: total_rx,
        total_tx_bytes: total_tx,
    })
}

fn validate_same_resolution_buckets_do_not_overlap(
    buckets: &[&NetworkTrafficImportBucket],
) -> Result<()> {
    let mut last_end_by_duration = BTreeMap::<u32, u64>::new();
    for bucket in buckets {
        let end = bucket
            .start_unix
            .checked_add(u64::from(bucket.duration_secs))
            .context("network_traffic_import_invalid:bucket_end_overflow")?;
        if let Some(previous_end) = last_end_by_duration.get(&bucket.duration_secs) {
            invalid_ensure(
                bucket.start_unix >= *previous_end,
                "same_resolution_bucket_overlap",
            )?;
        }
        last_end_by_duration.insert(bucket.duration_secs, end);
    }
    Ok(())
}

fn assignment_state_in_range(
    assignments: &[MinuteAssignmentSegment],
    start_unix: u64,
    end_unix: u64,
) -> Result<AssignmentState> {
    let mut assigned_rx = 0_u64;
    let mut assigned_tx = 0_u64;
    let mut uncovered = Vec::new();
    let mut cursor = start_unix;
    for segment in assignments {
        if segment.end_unix <= start_unix {
            continue;
        }
        if segment.start_unix >= end_unix {
            break;
        }
        let overlap_start = start_unix.max(segment.start_unix);
        let overlap_end = end_unix.min(segment.end_unix);
        if cursor < overlap_start {
            uncovered.push((cursor, overlap_start));
        }
        let minutes = (overlap_end - overlap_start) / 60;
        assigned_rx = assigned_rx
            .checked_add(
                segment
                    .rx_bytes
                    .checked_mul(minutes)
                    .context("network_traffic_import_invalid:assigned_rx_overflow")?,
            )
            .context("network_traffic_import_invalid:assigned_rx_overflow")?;
        assigned_tx = assigned_tx
            .checked_add(
                segment
                    .tx_bytes
                    .checked_mul(minutes)
                    .context("network_traffic_import_invalid:assigned_tx_overflow")?,
            )
            .context("network_traffic_import_invalid:assigned_tx_overflow")?;
        cursor = cursor.max(overlap_end);
    }
    if cursor < end_unix {
        uncovered.push((cursor, end_unix));
    }
    Ok(AssignmentState {
        assigned_rx_bytes: assigned_rx,
        assigned_tx_bytes: assigned_tx,
        uncovered_ranges: uncovered,
    })
}

fn distribute_residual(
    uncovered_ranges: &[(u64, u64)],
    residual_rx: u64,
    residual_tx: u64,
    uncovered: u64,
) -> Result<Vec<MinuteAssignmentSegment>> {
    invalid_ensure(uncovered > 0, "uncovered_minute_count_invalid")?;
    let rx_base = residual_rx / uncovered;
    let rx_remainder = residual_rx % uncovered;
    let tx_base = residual_tx / uncovered;
    let tx_remainder = residual_tx % uncovered;
    let mut rank = 0_u64;
    let mut segments = Vec::new();
    for &(start_unix, end_unix) in uncovered_ranges {
        let minutes = (end_unix - start_unix) / 60;
        let mut cuts = vec![0, minutes];
        for remainder in [rx_remainder, tx_remainder] {
            if remainder > rank && remainder < rank.saturating_add(minutes) {
                cuts.push(remainder - rank);
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for pair in cuts.windows(2) {
            let first = pair[0];
            let last = pair[1];
            if first == last {
                continue;
            }
            let segment_start = start_unix
                .checked_add(first.saturating_mul(60))
                .context("network_traffic_import_invalid:minute_timestamp_overflow")?;
            let segment_end = start_unix
                .checked_add(last.saturating_mul(60))
                .context("network_traffic_import_invalid:minute_timestamp_overflow")?;
            push_assignment_segment(
                &mut segments,
                MinuteAssignmentSegment {
                    start_unix: segment_start,
                    end_unix: segment_end,
                    rx_bytes: rx_base + u64::from(rank + first < rx_remainder),
                    tx_bytes: tx_base + u64::from(rank + first < tx_remainder),
                },
            )?;
        }
        rank = rank
            .checked_add(minutes)
            .context("network_traffic_import_invalid:uncovered_minute_count_overflow")?;
    }
    invalid_ensure(rank == uncovered, "uncovered_minute_count_changed")?;
    Ok(segments)
}

fn merge_assignment_segments(
    assignments: &mut Vec<MinuteAssignmentSegment>,
    added: Vec<MinuteAssignmentSegment>,
) -> Result<()> {
    let mut existing = std::mem::take(assignments).into_iter().peekable();
    let mut added = added.into_iter().peekable();
    while existing.peek().is_some() || added.peek().is_some() {
        let take_existing = match (existing.peek(), added.peek()) {
            (Some(left), Some(right)) => left.start_unix <= right.start_unix,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        let segment = if take_existing {
            existing.next().expect("peeked assignment segment")
        } else {
            added.next().expect("peeked added segment")
        };
        push_assignment_segment(assignments, segment)?;
    }
    Ok(())
}

fn push_assignment_segment(
    segments: &mut Vec<MinuteAssignmentSegment>,
    segment: MinuteAssignmentSegment,
) -> Result<()> {
    invalid_ensure(
        segment.start_unix < segment.end_unix
            && segment.start_unix.is_multiple_of(60)
            && segment.end_unix.is_multiple_of(60),
        "assignment_segment_invalid",
    )?;
    if let Some(previous) = segments.last_mut() {
        invalid_ensure(
            previous.end_unix <= segment.start_unix,
            "assignment_segment_overlap",
        )?;
        if previous.end_unix == segment.start_unix
            && previous.rx_bytes == segment.rx_bytes
            && previous.tx_bytes == segment.tx_bytes
        {
            previous.end_unix = segment.end_unix;
            return Ok(());
        }
    }
    segments.push(segment);
    Ok(())
}

#[cfg(test)]
fn sample_record(
    client_id: &str,
    interface: &str,
    observed_unix: u64,
    rx_bytes: i64,
    tx_bytes: i64,
    sample_source: &str,
) -> Result<TrafficCounterSampleRecord> {
    let observed_unix_i64 = i64::try_from(observed_unix)
        .context("network_traffic_import_invalid:sample_timestamp_out_of_range")?;
    let observed_at = Utc
        .timestamp_opt(observed_unix_i64, 0)
        .single()
        .context("network_traffic_import_invalid:sample_timestamp_invalid")?
        .to_rfc3339();
    Ok(TrafficCounterSampleRecord {
        client_id: client_id.to_string(),
        source_kind: "host".to_string(),
        interface: interface.to_string(),
        observed_at,
        observed_unix: observed_unix_i64,
        rx_bytes,
        tx_bytes,
        rx_counter_epoch: 0,
        tx_counter_epoch: 0,
        sample_source: sample_source.to_string(),
    })
}

#[cfg(test)]
struct PreparedImportSampleIter<'a> {
    client_id: &'a str,
    prepared: &'a PreparedInterfaceImport,
    segment_index: usize,
    next_unix: u64,
    cumulative_rx: i64,
    cumulative_tx: i64,
    baseline_pending: bool,
}

impl PreparedInterfaceImport {
    #[cfg(test)]
    fn samples<'a>(&'a self, client_id: &'a str) -> PreparedImportSampleIter<'a> {
        PreparedImportSampleIter {
            client_id,
            prepared: self,
            segment_index: 0,
            next_unix: self.start_unix,
            cumulative_rx: self.initial_rx_bytes,
            cumulative_tx: self.initial_tx_bytes,
            baseline_pending: self.include_baseline,
        }
    }

    #[cfg(test)]
    fn samples_from<'a>(
        &'a self,
        client_id: &'a str,
        minimum_unix: u64,
    ) -> Result<PreparedImportSampleIter<'a>> {
        invalid_ensure(
            minimum_unix.is_multiple_of(60),
            "raw_retention_cutoff_not_minute_aligned",
        )?;
        let natural_start = if self.include_baseline {
            self.start_unix - 60
        } else {
            self.start_unix
        };
        let next_unix = minimum_unix.max(natural_start).min(self.end_unix);
        if self.include_baseline && next_unix == self.start_unix - 60 {
            return Ok(self.samples(client_id));
        }
        let next_unix = next_unix.max(self.start_unix);
        let (prefix_rx, prefix_tx) =
            assignment_totals_in_range(&self.traffic.segments, self.start_unix, next_unix)?;
        let cumulative_rx = self
            .initial_rx_bytes
            .checked_add(
                i64::try_from(prefix_rx)
                    .context("network_traffic_import_invalid:rx_counter_overflow")?,
            )
            .context("network_traffic_import_invalid:rx_counter_overflow")?;
        let cumulative_tx = self
            .initial_tx_bytes
            .checked_add(
                i64::try_from(prefix_tx)
                    .context("network_traffic_import_invalid:tx_counter_overflow")?,
            )
            .context("network_traffic_import_invalid:tx_counter_overflow")?;
        let segment_index = self
            .traffic
            .segments
            .partition_point(|segment| segment.end_unix <= next_unix);
        Ok(PreparedImportSampleIter {
            client_id,
            prepared: self,
            segment_index,
            next_unix,
            cumulative_rx,
            cumulative_tx,
            baseline_pending: false,
        })
    }
}

fn assignment_totals_in_range(
    segments: &[MinuteAssignmentSegment],
    start_unix: u64,
    end_unix: u64,
) -> Result<(u64, u64)> {
    if end_unix <= start_unix {
        return Ok((0, 0));
    }
    let mut rx_total = 0_u64;
    let mut tx_total = 0_u64;
    for segment in &segments[segments.partition_point(|segment| segment.end_unix <= start_unix)..] {
        if segment.start_unix >= end_unix {
            break;
        }
        let overlap_start = segment.start_unix.max(start_unix);
        let overlap_end = segment.end_unix.min(end_unix);
        let minutes = (overlap_end - overlap_start) / 60;
        rx_total = rx_total
            .checked_add(
                segment
                    .rx_bytes
                    .checked_mul(minutes)
                    .context("network_traffic_import_invalid:rx_total_overflow")?,
            )
            .context("network_traffic_import_invalid:rx_total_overflow")?;
        tx_total = tx_total
            .checked_add(
                segment
                    .tx_bytes
                    .checked_mul(minutes)
                    .context("network_traffic_import_invalid:tx_total_overflow")?,
            )
            .context("network_traffic_import_invalid:tx_total_overflow")?;
    }
    Ok((rx_total, tx_total))
}

fn prepare_import_rollups(
    prepared: &PreparedInterfaceImport,
    utc_day_start_unix: u64,
    raw_cutoff_unix: u64,
) -> Result<Vec<PreparedImportRollup>> {
    invalid_ensure(
        utc_day_start_unix.is_multiple_of(86_400)
            && raw_cutoff_unix.is_multiple_of(3_600)
            && raw_cutoff_unix <= utc_day_start_unix,
        "retention_boundary_invalid",
    )?;
    let mut rollups = BTreeMap::<(i32, u64), PreparedImportRollup>::new();
    if prepared.include_baseline && prepared.start_unix - 60 < raw_cutoff_unix {
        accumulate_import_rollup(
            &mut rollups,
            &prepared.interface,
            utc_day_start_unix,
            prepared.start_unix - 60,
            prepared.start_unix,
            0,
            0,
            0,
        )?;
    }
    for segment in &prepared.traffic.segments {
        let mut cursor = segment.start_unix.max(prepared.start_unix);
        let segment_end = segment.end_unix.min(prepared.end_unix).min(raw_cutoff_unix);
        while cursor < segment_end {
            let bucket_secs = import_rollup_bucket_secs(cursor, utc_day_start_unix);
            let bucket_secs_u64 = u64::try_from(bucket_secs)
                .context("network_traffic_import_invalid:rollup_bucket_size_invalid")?;
            let bucket_start = cursor - cursor % bucket_secs_u64;
            let bucket_end = bucket_start
                .checked_add(bucket_secs_u64)
                .context("network_traffic_import_invalid:rollup_bucket_end_overflow")?;
            let next_tier = next_import_rollup_tier_boundary(cursor, utc_day_start_unix);
            let piece_end = segment_end.min(bucket_end).min(next_tier);
            invalid_ensure(piece_end > cursor, "rollup_piece_empty")?;
            let minute_count = (piece_end - cursor) / 60;
            accumulate_import_rollup(
                &mut rollups,
                &prepared.interface,
                utc_day_start_unix,
                cursor,
                piece_end,
                segment.rx_bytes,
                segment.tx_bytes,
                minute_count,
            )?;
            cursor = piece_end;
        }
    }
    let rollups = rollups.into_values().collect::<Vec<_>>();
    for rollup in &rollups {
        validate_prepared_import_rollup(rollup)?;
    }
    Ok(rollups)
}

fn validate_prepared_import_rollup(rollup: &PreparedImportRollup) -> Result<()> {
    i64::try_from(rollup.bucket_start_unix)
        .context("network_traffic_import_invalid:rollup_timestamp_out_of_range")?;
    i64::try_from(rollup.first_observed_unix)
        .context("network_traffic_import_invalid:rollup_timestamp_out_of_range")?;
    i64::try_from(rollup.latest_observed_unix)
        .context("network_traffic_import_invalid:rollup_timestamp_out_of_range")?;
    i64::try_from(rollup.rx_bytes)
        .context("network_traffic_import_invalid:rx_delta_exceeds_database_range")?;
    i64::try_from(rollup.tx_bytes)
        .context("network_traffic_import_invalid:tx_delta_exceeds_database_range")?;
    i32::try_from(rollup.rx_valid_count)
        .context("network_traffic_import_invalid:rollup_valid_count_out_of_range")?;
    i32::try_from(rollup.tx_valid_count)
        .context("network_traffic_import_invalid:rollup_valid_count_out_of_range")?;
    i32::try_from(rollup.any_valid_count)
        .context("network_traffic_import_invalid:rollup_valid_count_out_of_range")?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn accumulate_import_rollup(
    rollups: &mut BTreeMap<(i32, u64), PreparedImportRollup>,
    interface: &str,
    utc_day_start_unix: u64,
    first_observed_unix: u64,
    observed_end_unix: u64,
    rx_bytes_per_minute: u64,
    tx_bytes_per_minute: u64,
    minute_count: u64,
) -> Result<()> {
    let bucket_secs = import_rollup_bucket_secs(first_observed_unix, utc_day_start_unix);
    let bucket_secs_u64 = u64::try_from(bucket_secs)
        .context("network_traffic_import_invalid:rollup_bucket_size_invalid")?;
    let bucket_start_unix = first_observed_unix - first_observed_unix % bucket_secs_u64;
    invalid_ensure(
        observed_end_unix > first_observed_unix
            && observed_end_unix <= bucket_start_unix + bucket_secs_u64,
        "rollup_piece_outside_bucket",
    )?;
    let latest_observed_unix = observed_end_unix - 60;
    let rx_bytes = rx_bytes_per_minute
        .checked_mul(minute_count)
        .context("network_traffic_import_invalid:rx_total_overflow")?;
    let tx_bytes = tx_bytes_per_minute
        .checked_mul(minute_count)
        .context("network_traffic_import_invalid:tx_total_overflow")?;
    let valid_count = u32::try_from(minute_count)
        .context("network_traffic_import_invalid:rollup_valid_count_overflow")?;
    let entry = rollups
        .entry((bucket_secs, bucket_start_unix))
        .or_insert_with(|| PreparedImportRollup {
            interface: interface.to_string(),
            bucket_secs,
            bucket_start_unix,
            rx_bytes: 0,
            tx_bytes: 0,
            rx_valid_count: 0,
            tx_valid_count: 0,
            any_valid_count: 0,
            first_observed_unix,
            latest_observed_unix,
        });
    entry.rx_bytes = entry
        .rx_bytes
        .checked_add(rx_bytes)
        .context("network_traffic_import_invalid:rx_total_overflow")?;
    entry.tx_bytes = entry
        .tx_bytes
        .checked_add(tx_bytes)
        .context("network_traffic_import_invalid:tx_total_overflow")?;
    entry.rx_valid_count = entry
        .rx_valid_count
        .checked_add(valid_count)
        .context("network_traffic_import_invalid:rollup_valid_count_overflow")?;
    entry.tx_valid_count = entry
        .tx_valid_count
        .checked_add(valid_count)
        .context("network_traffic_import_invalid:rollup_valid_count_overflow")?;
    entry.any_valid_count = entry
        .any_valid_count
        .checked_add(valid_count)
        .context("network_traffic_import_invalid:rollup_valid_count_overflow")?;
    entry.first_observed_unix = entry.first_observed_unix.min(first_observed_unix);
    entry.latest_observed_unix = entry.latest_observed_unix.max(latest_observed_unix);
    Ok(())
}

fn import_rollup_bucket_secs(observed_unix: u64, utc_day_start_unix: u64) -> i32 {
    if observed_unix >= utc_day_start_unix.saturating_sub(91 * 86_400) {
        3_600
    } else if observed_unix >= utc_day_start_unix.saturating_sub(181 * 86_400) {
        10_800
    } else if observed_unix >= utc_day_start_unix.saturating_sub(366 * 86_400) {
        21_600
    } else {
        86_400
    }
}

fn next_import_rollup_tier_boundary(observed_unix: u64, utc_day_start_unix: u64) -> u64 {
    for boundary in [
        utc_day_start_unix.saturating_sub(366 * 86_400),
        utc_day_start_unix.saturating_sub(181 * 86_400),
        utc_day_start_unix.saturating_sub(91 * 86_400),
    ] {
        if boundary > observed_unix {
            return boundary;
        }
    }
    u64::MAX
}

#[cfg(test)]
impl Iterator for PreparedImportSampleIter<'_> {
    type Item = Result<TrafficCounterSampleRecord>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.baseline_pending {
            self.baseline_pending = false;
            return Some(sample_record(
                self.client_id,
                &self.prepared.interface,
                self.prepared.start_unix - 60,
                self.cumulative_rx,
                self.cumulative_tx,
                "vnstat_import:fixture",
            ));
        }
        if self.next_unix >= self.prepared.end_unix {
            return None;
        }
        while self
            .prepared
            .traffic
            .segments
            .get(self.segment_index)
            .is_some_and(|segment| segment.end_unix <= self.next_unix)
        {
            self.segment_index += 1;
        }
        let Some(segment) = self.prepared.traffic.segments.get(self.segment_index) else {
            return Some(Err(anyhow::anyhow!(
                "network_traffic_import_invalid:prepared_history_gap"
            )));
        };
        if segment.start_unix > self.next_unix || segment.end_unix <= self.next_unix {
            return Some(Err(anyhow::anyhow!(
                "network_traffic_import_invalid:prepared_history_gap"
            )));
        }
        self.cumulative_rx = match i64::try_from(segment.rx_bytes)
            .ok()
            .and_then(|delta| self.cumulative_rx.checked_add(delta))
        {
            Some(value) => value,
            None => {
                return Some(Err(anyhow::anyhow!(
                    "network_traffic_import_invalid:rx_counter_overflow"
                )))
            }
        };
        self.cumulative_tx = match i64::try_from(segment.tx_bytes)
            .ok()
            .and_then(|delta| self.cumulative_tx.checked_add(delta))
        {
            Some(value) => value,
            None => {
                return Some(Err(anyhow::anyhow!(
                    "network_traffic_import_invalid:tx_counter_overflow"
                )))
            }
        };
        let observed_unix = self.next_unix;
        self.next_unix = self.next_unix.saturating_add(60);
        Some(sample_record(
            self.client_id,
            &self.prepared.interface,
            observed_unix,
            self.cumulative_rx,
            self.cumulative_tx,
            "vnstat_import:fixture",
        ))
    }
}

#[cfg(test)]
fn prepare_test_import_rollups(
    client_id: &str,
    prepared: &[PreparedInterfaceImport],
    utc_day_start_unix: u64,
    raw_cutoff_unix: u64,
) -> Result<Vec<TrafficCounterRollupRecord>> {
    let mut rows = Vec::new();
    for item in prepared {
        for rollup in prepare_import_rollups(item, utc_day_start_unix, raw_cutoff_unix)? {
            let bucket_start_unix = i64::try_from(rollup.bucket_start_unix)
                .context("network_traffic_import_invalid:rollup_timestamp_out_of_range")?;
            let first_observed_unix = i64::try_from(rollup.first_observed_unix)
                .context("network_traffic_import_invalid:rollup_timestamp_out_of_range")?;
            let latest_observed_unix = i64::try_from(rollup.latest_observed_unix)
                .context("network_traffic_import_invalid:rollup_timestamp_out_of_range")?;
            let bucket_start = Utc
                .timestamp_opt(bucket_start_unix, 0)
                .single()
                .context("network_traffic_import_invalid:rollup_timestamp_invalid")?
                .to_rfc3339();
            rows.push(TrafficCounterRollupRecord {
                client_id: client_id.to_string(),
                source_kind: "host".to_string(),
                interface: rollup.interface,
                origin_kind: "vnstat_import".to_string(),
                bucket_start,
                bucket_start_unix,
                bucket_secs: rollup.bucket_secs,
                rx_bytes: i64::try_from(rollup.rx_bytes)
                    .context("network_traffic_import_invalid:rx_delta_exceeds_database_range")?,
                tx_bytes: i64::try_from(rollup.tx_bytes)
                    .context("network_traffic_import_invalid:tx_delta_exceeds_database_range")?,
                rx_valid_count: i32::try_from(rollup.rx_valid_count)
                    .context("network_traffic_import_invalid:rollup_valid_count_out_of_range")?,
                tx_valid_count: i32::try_from(rollup.tx_valid_count)
                    .context("network_traffic_import_invalid:rollup_valid_count_out_of_range")?,
                any_valid_count: i32::try_from(rollup.any_valid_count)
                    .context("network_traffic_import_invalid:rollup_valid_count_out_of_range")?,
                rx_reset_count: 0,
                tx_reset_count: 0,
                any_reset_count: 0,
                first_observed_unix,
                latest_observed_unix,
            });
        }
    }
    Ok(rows)
}

async fn insert_postgres_import_rollups(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    client_id: &str,
    interface: &str,
    rows: &PreparedImportRollupRows,
) -> Result<()> {
    let row_count = rows.bucket_secs.len();
    anyhow::ensure!(
        [
            rows.bucket_start_unix.len(),
            rows.rx_bytes.len(),
            rows.tx_bytes.len(),
            rows.rx_valid_count.len(),
            rows.tx_valid_count.len(),
            rows.any_valid_count.len(),
            rows.first_observed_unix.len(),
            rows.latest_observed_unix.len(),
        ]
        .into_iter()
        .all(|len| len == row_count),
        "network_traffic_import_rollup_array_length_mismatch"
    );
    if row_count == 0 {
        return Ok(());
    }
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        )
        SELECT
            $1,
            'host',
            $2,
            'vnstat_import',
            imported.bucket_secs,
            to_timestamp(imported.bucket_start_unix::double precision),
            imported.rx_bytes,
            imported.tx_bytes,
            imported.rx_valid_count,
            imported.tx_valid_count,
            imported.any_valid_count,
            0,
            0,
            0,
            to_timestamp(imported.first_observed_unix::double precision),
            to_timestamp(imported.latest_observed_unix::double precision)
        FROM unnest(
            $3::int[], $4::bigint[], $5::bigint[], $6::bigint[],
            $7::int[], $8::int[], $9::int[], $10::bigint[], $11::bigint[]
        ) AS imported(
            bucket_secs, bucket_start_unix, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            first_observed_unix, latest_observed_unix
        )
        ON CONFLICT (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start
        ) DO UPDATE SET
            rx_bytes = EXCLUDED.rx_bytes,
            tx_bytes = EXCLUDED.tx_bytes,
            rx_valid_count = EXCLUDED.rx_valid_count,
            tx_valid_count = EXCLUDED.tx_valid_count,
            any_valid_count = EXCLUDED.any_valid_count,
            rx_reset_count = EXCLUDED.rx_reset_count,
            tx_reset_count = EXCLUDED.tx_reset_count,
            any_reset_count = EXCLUDED.any_reset_count,
            first_observed_at = EXCLUDED.first_observed_at,
            latest_observed_at = EXCLUDED.latest_observed_at,
            updated_at = now()
        "#,
    )
    .bind(client_id)
    .bind(interface)
    .bind(&rows.bucket_secs)
    .bind(&rows.bucket_start_unix)
    .bind(&rows.rx_bytes)
    .bind(&rows.tx_bytes)
    .bind(&rows.rx_valid_count)
    .bind(&rows.tx_valid_count)
    .bind(&rows.any_valid_count)
    .bind(&rows.first_observed_unix)
    .bind(&rows.latest_observed_unix)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) fn is_vnstat_import_source(source: &str) -> bool {
    source.starts_with(VNSTAT_IMPORT_SOURCE_PREFIX)
}

#[cfg(test)]
pub(crate) fn is_intentional_vnstat_import_boundary(
    previous_source: &str,
    current_source: &str,
) -> bool {
    is_vnstat_import_source(previous_source) && !is_vnstat_import_source(current_source)
}

fn invalid_ensure(condition: bool, code: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        anyhow::bail!("network_traffic_import_invalid:{code}")
    }
}

fn floor_minute(unix: u64) -> u64 {
    unix - unix % 60
}

fn ceil_minute(unix: u64) -> Option<u64> {
    unix.checked_add(59).map(floor_minute)
}

#[cfg(test)]
#[path = "tests_repository_network_traffic_import.rs"]
mod tests;
