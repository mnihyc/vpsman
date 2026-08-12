use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use sqlx::{postgres::PgRow, Postgres, QueryBuilder, Row};
use uuid::Uuid;
use vpsman_common::{
    NetworkTrafficImportBucket, NetworkTrafficImportResult, MIN_TRAFFIC_COUNTER_RETENTION_DAYS,
    NETWORK_TRAFFIC_IMPORT_MAX_BUCKETS_PER_INTERFACE, NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES,
};

use crate::{model_alert_policies::TrafficCounterSampleRecord, repository::Repository};

pub(crate) const VNSTAT_IMPORT_SOURCE_PREFIX: &str = "vnstat_import:";
const IMPORT_INSERT_BATCH_ROWS: usize = 500;
const MAX_IMPORT_BUCKET_DURATION_SECS: u64 = 367 * 24 * 60 * 60;

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
    import_source: String,
    traffic: ExpandedMinuteTraffic,
    imported_rx_bytes: u64,
    imported_tx_bytes: u64,
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
        let resolved_interfaces = &result.interfaces;
        match self {
            Self::Memory(memory) => {
                let mut samples = memory.traffic_counter_samples.write().await;
                let prepared = prepare_imports(
                    job_id,
                    client_id,
                    resolved_interfaces,
                    start_unix,
                    result,
                    buckets,
                    now_unix,
                    &samples,
                )?;
                apply_memory_import(&mut samples, client_id, &prepared)?;
                memory
                    .traffic_counter_rollups
                    .write()
                    .await
                    .retain(|rollup| {
                        rollup.client_id != client_id
                            || rollup.source_kind != "host"
                            || !resolved_interfaces.contains(&rollup.interface)
                            || rollup.origin_kind != "vnstat_import"
                    });
                drop(samples);
                Ok(import_summary(&prepared))
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_postgres_traffic_counter_streams(&mut tx, client_id).await?;
                let effective_starts =
                    effective_interface_starts(resolved_interfaces, start_unix, result)?;
                let existing = load_postgres_import_boundary_samples_for_starts(
                    &mut tx,
                    client_id,
                    resolved_interfaces,
                    &effective_starts,
                )
                .await?;
                let prepared = prepare_imports(
                    job_id,
                    client_id,
                    resolved_interfaces,
                    start_unix,
                    result,
                    buckets,
                    now_unix,
                    &existing,
                )?;

                // vnStat replacement owns only the traffic-counter ledger.
                // Live network-rate rows are independent agent telemetry and
                // their counter epochs must not be rewritten by an import.
                sqlx::query(
                    r#"
                    DELETE FROM traffic_counter_rollups
                    WHERE client_id = $1
                      AND source_kind = 'host'
                      AND interface = ANY($2::text[])
                      AND origin_kind = 'vnstat_import'
                    "#,
                )
                .bind(client_id)
                .bind(resolved_interfaces)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    r#"
                    DELETE FROM traffic_counter_samples
                    WHERE client_id = $1
                      AND source_kind = 'host'
                      AND interface = ANY($2::text[])
                      AND sample_source LIKE 'vnstat_import:%'
                    "#,
                )
                .bind(client_id)
                .bind(resolved_interfaces)
                .execute(&mut *tx)
                .await?;
                for item in &prepared {
                    insert_postgres_import_samples(&mut tx, client_id, item).await?;
                    recompute_postgres_stream_epochs(&mut tx, client_id, &item.interface).await?;
                }
                rebuild_postgres_import_rollups(&mut tx, client_id, resolved_interfaces).await?;
                tx.commit().await?;
                Ok(import_summary(&prepared))
            }
        }
    }
}

const POSTGRES_IMPORT_PREVIOUS_BOUNDARIES_SQL: &str = r#"
    SELECT
        boundary.client_id,
        boundary.source_kind,
        boundary.interface,
        boundary.observed_at::text AS observed_at,
        EXTRACT(EPOCH FROM boundary.observed_at)::bigint AS observed_unix,
        boundary.rx_bytes,
        boundary.tx_bytes,
        boundary.rx_counter_epoch,
        boundary.tx_counter_epoch,
        boundary.sample_source
    FROM unnest($2::text[], $3::bigint[]) AS requested(interface, start_unix)
    CROSS JOIN LATERAL (
        SELECT sample.*
        FROM traffic_counter_samples sample
        WHERE sample.client_id = $1
          AND sample.source_kind = 'host'
          AND sample.interface = requested.interface
          AND sample.sample_source NOT LIKE 'vnstat_import:%'
          AND sample.observed_at < to_timestamp(requested.start_unix::double precision)
        ORDER BY sample.observed_at DESC
        LIMIT 1
        FOR UPDATE OF sample
    ) boundary
    ORDER BY boundary.interface ASC
"#;

const POSTGRES_IMPORT_LIVE_BOUNDARIES_SQL: &str = r#"
    SELECT
        boundary.client_id,
        boundary.source_kind,
        boundary.interface,
        boundary.observed_at::text AS observed_at,
        EXTRACT(EPOCH FROM boundary.observed_at)::bigint AS observed_unix,
        boundary.rx_bytes,
        boundary.tx_bytes,
        boundary.rx_counter_epoch,
        boundary.tx_counter_epoch,
        boundary.sample_source
    FROM unnest($2::text[], $3::bigint[]) AS requested(interface, start_unix)
    CROSS JOIN LATERAL (
        SELECT candidate.*
        FROM (
            SELECT
                sample.client_id,
                sample.source_kind,
                sample.interface,
                sample.observed_at,
                sample.rx_bytes,
                sample.tx_bytes,
                sample.rx_counter_epoch,
                sample.tx_counter_epoch,
                sample.sample_source
            FROM traffic_counter_samples sample
            WHERE sample.client_id = $1
              AND sample.source_kind = 'host'
              AND sample.interface = requested.interface
              AND sample.sample_source NOT LIKE 'vnstat_import:%'
              AND sample.observed_at
                    >= to_timestamp(requested.start_unix::double precision)
            UNION ALL
            SELECT
                rollup.client_id,
                rollup.source_kind,
                rollup.interface,
                GREATEST(
                    rollup.first_observed_at,
                    to_timestamp(requested.start_unix::double precision)
                ),
                0::bigint,
                0::bigint,
                0::bigint,
                0::bigint,
                'retained_live_rollup'::text
            FROM traffic_counter_rollups rollup
            WHERE rollup.client_id = $1
              AND rollup.source_kind = 'host'
              AND rollup.interface = requested.interface
              AND rollup.origin_kind = 'live'
              AND rollup.latest_observed_at
                    >= to_timestamp(requested.start_unix::double precision)
        ) candidate
        ORDER BY candidate.observed_at ASC
        LIMIT 1
    ) boundary
    ORDER BY boundary.interface ASC
"#;

#[cfg(test)]
pub(crate) async fn load_postgres_import_boundary_samples(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    client_id: &str,
    interfaces: &[String],
    start_unix: u64,
) -> Result<Vec<TrafficCounterSampleRecord>> {
    let start_unix = i64::try_from(start_unix)
        .context("network_traffic_import_invalid:start_timestamp_out_of_range")?;
    let starts = vec![start_unix; interfaces.len()];
    load_postgres_import_boundary_samples_for_starts(tx, client_id, interfaces, &starts).await
}

async fn load_postgres_import_boundary_samples_for_starts(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    client_id: &str,
    interfaces: &[String],
    effective_starts: &[i64],
) -> Result<Vec<TrafficCounterSampleRecord>> {
    anyhow::ensure!(
        interfaces.len() == effective_starts.len(),
        "network_traffic_import_invalid:effective_start_count_mismatch"
    );
    let mut samples = Vec::with_capacity(interfaces.len().saturating_mul(2));
    for query in [
        POSTGRES_IMPORT_PREVIOUS_BOUNDARIES_SQL,
        POSTGRES_IMPORT_LIVE_BOUNDARIES_SQL,
    ] {
        let rows = sqlx::query(query)
            .bind(client_id)
            .bind(interfaces)
            .bind(effective_starts)
            .fetch_all(&mut **tx)
            .await?;
        samples.extend(
            rows.into_iter()
                .map(postgres_traffic_counter_sample)
                .collect::<std::result::Result<Vec<_>, sqlx::Error>>()?,
        );
    }
    Ok(samples)
}

fn postgres_traffic_counter_sample(
    row: PgRow,
) -> std::result::Result<TrafficCounterSampleRecord, sqlx::Error> {
    Ok(TrafficCounterSampleRecord {
        client_id: row.try_get("client_id")?,
        source_kind: row.try_get("source_kind")?,
        interface: row.try_get("interface")?,
        observed_at: row.try_get("observed_at")?,
        observed_unix: row.try_get("observed_unix")?,
        rx_bytes: row.try_get("rx_bytes")?,
        tx_bytes: row.try_get("tx_bytes")?,
        rx_counter_epoch: row.try_get("rx_counter_epoch")?,
        tx_counter_epoch: row.try_get("tx_counter_epoch")?,
        sample_source: row.try_get("sample_source")?,
    })
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

    let requested = interfaces.iter().cloned().collect::<BTreeSet<_>>();
    invalid_ensure(requested.len() == interfaces.len(), "duplicate_interface")?;
    let result_interfaces = result.interfaces.iter().cloned().collect::<BTreeSet<_>>();
    invalid_ensure(
        !result.interfaces.is_empty()
            && result.interfaces.len() <= NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES
            && result_interfaces.len() == result.interfaces.len(),
        "agent_result_interface_count_out_of_range",
    )?;
    if !interfaces.is_empty() {
        invalid_ensure(
            result_interfaces == requested && result.interfaces.len() == interfaces.len(),
            "agent_result_interface_mismatch",
        )?;
    }
    let sources = result
        .sources
        .iter()
        .map(|source| source.interface.clone())
        .collect::<BTreeSet<_>>();
    invalid_ensure(
        sources == result_interfaces && result.sources.len() == result.interfaces.len(),
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
            .all(|bucket| result_interfaces.contains(&bucket.interface)),
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

fn effective_interface_starts(
    interfaces: &[String],
    requested_start_unix: u64,
    result: &NetworkTrafficImportResult,
) -> Result<Vec<i64>> {
    let source_by_interface = result
        .sources
        .iter()
        .map(|source| (source.interface.as_str(), source))
        .collect::<HashMap<_, _>>();
    interfaces
        .iter()
        .map(|interface| {
            let source = source_by_interface
                .get(interface.as_str())
                .context("network_traffic_import_invalid:source_missing")?;
            i64::try_from(requested_start_unix.max(source.retained_start_unix))
                .context("network_traffic_import_invalid:effective_start_out_of_range")
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn prepare_imports(
    job_id: Uuid,
    client_id: &str,
    interfaces: &[String],
    start_unix: u64,
    result: &NetworkTrafficImportResult,
    buckets: &[NetworkTrafficImportBucket],
    now_unix: u64,
    existing: &[TrafficCounterSampleRecord],
) -> Result<Vec<PreparedInterfaceImport>> {
    let source_by_interface = result
        .sources
        .iter()
        .map(|source| (source.interface.as_str(), source))
        .collect::<HashMap<_, _>>();
    let import_source = format!("{VNSTAT_IMPORT_SOURCE_PREFIX}{job_id}");
    let now_minute = floor_minute(now_unix);
    let mut prepared = Vec::new();

    for interface in interfaces {
        let source = source_by_interface
            .get(interface.as_str())
            .context("network_traffic_import_invalid:source_missing")?;
        let effective_start_unix = start_unix.max(source.retained_start_unix);
        let first_live_unix = existing
            .iter()
            .filter(|sample| {
                sample.client_id == client_id
                    && sample.source_kind == "host"
                    && sample.interface == *interface
                    && !is_vnstat_import_source(&sample.sample_source)
                    && sample.observed_unix
                        >= i64::try_from(effective_start_unix).unwrap_or(i64::MAX)
            })
            .map(|sample| sample.observed_unix)
            .min()
            .and_then(|value| u64::try_from(value).ok())
            .context("network_traffic_import_invalid:first_live_agent_sample_missing")?;
        let retained_start_through_live =
            continuous_coverage_start_through(buckets, interface, first_live_unix)?;
        invalid_ensure(
            retained_start_through_live == source.retained_start_unix,
            "vnstat_retained_coverage_does_not_reach_live_boundary",
        )?;
        invalid_ensure(
            first_live_unix > effective_start_unix && first_live_unix <= now_minute,
            "range_already_covered_by_agent",
        )?;
        invalid_ensure(
            result.collected_until_unix >= first_live_unix,
            "agent_collection_predates_live_boundary",
        )?;

        invalid_ensure(
            source
                .source_updated_unix
                .map(floor_minute)
                .is_some_and(|updated| updated >= first_live_unix),
            "vnstat_source_not_updated_through_live_boundary",
        )?;

        let traffic =
            expand_buckets_to_minutes(buckets, interface, effective_start_unix, first_live_unix)?;
        let imported_rx_bytes = traffic.total_rx_bytes;
        let imported_tx_bytes = traffic.total_tx_bytes;
        let previous = existing
            .iter()
            .filter(|sample| {
                sample.client_id == client_id
                    && sample.source_kind == "host"
                    && sample.interface == *interface
                    && !is_vnstat_import_source(&sample.sample_source)
                    && sample.observed_unix
                        < i64::try_from(effective_start_unix).unwrap_or(i64::MAX)
            })
            .max_by_key(|sample| sample.observed_unix);
        let cumulative_rx = previous.map_or(0, |sample| sample.rx_bytes);
        let cumulative_tx = previous.map_or(0, |sample| sample.tx_bytes);
        invalid_ensure(
            cumulative_rx >= 0 && cumulative_tx >= 0,
            "negative_counter_baseline",
        )?;
        cumulative_rx
            .checked_add(
                i64::try_from(imported_rx_bytes)
                    .context("network_traffic_import_invalid:rx_delta_exceeds_database_range")?,
            )
            .context("network_traffic_import_invalid:rx_counter_overflow")?;
        cumulative_tx
            .checked_add(
                i64::try_from(imported_tx_bytes)
                    .context("network_traffic_import_invalid:tx_delta_exceeds_database_range")?,
            )
            .context("network_traffic_import_invalid:tx_counter_overflow")?;
        sample_record(
            client_id,
            interface,
            effective_start_unix - 60,
            cumulative_rx,
            cumulative_tx,
            &import_source,
        )?;
        sample_record(
            client_id,
            interface,
            first_live_unix - 60,
            cumulative_rx,
            cumulative_tx,
            &import_source,
        )?;
        prepared.push(PreparedInterfaceImport {
            interface: interface.clone(),
            start_unix: effective_start_unix,
            end_unix: first_live_unix,
            initial_rx_bytes: cumulative_rx,
            initial_tx_bytes: cumulative_tx,
            include_baseline: previous.is_none(),
            import_source: import_source.clone(),
            traffic,
            imported_rx_bytes,
            imported_tx_bytes,
        });
    }
    Ok(prepared)
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

fn continuous_coverage_start_through(
    buckets: &[NetworkTrafficImportBucket],
    interface: &str,
    through_unix: u64,
) -> Result<u64> {
    merged_coverage_components(buckets, interface)?
        .into_iter()
        .find(|(start_unix, end_unix)| *start_unix < through_unix && *end_unix >= through_unix)
        .map(|(start_unix, _)| start_unix)
        .context("network_traffic_import_invalid:vnstat_history_does_not_reach_live_boundary")
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
}

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
                &self.prepared.import_source,
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
            &self.prepared.import_source,
        ))
    }
}

fn apply_memory_import(
    samples: &mut Vec<TrafficCounterSampleRecord>,
    client_id: &str,
    prepared: &[PreparedInterfaceImport],
) -> Result<()> {
    let interfaces = prepared
        .iter()
        .map(|item| item.interface.as_str())
        .collect::<BTreeSet<_>>();
    samples.retain(|sample| {
        !(sample.client_id == client_id
            && sample.source_kind == "host"
            && interfaces.contains(sample.interface.as_str())
            && is_vnstat_import_source(&sample.sample_source))
    });
    for item in prepared {
        for sample in item.samples(client_id) {
            samples.push(sample?);
        }
    }
    for interface in interfaces {
        recompute_memory_stream_epochs(samples, client_id, interface);
    }
    Ok(())
}

fn recompute_memory_stream_epochs(
    samples: &mut [TrafficCounterSampleRecord],
    client_id: &str,
    interface: &str,
) {
    let mut indices = samples
        .iter()
        .enumerate()
        .filter(|(_, sample)| {
            sample.client_id == client_id
                && sample.source_kind == "host"
                && sample.interface == interface
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    indices.sort_by_key(|index| samples[*index].observed_unix);
    let mut previous = None::<(i64, i64, bool)>;
    let mut rx_epoch = 0_i64;
    let mut tx_epoch = 0_i64;
    for index in indices {
        let imported = is_vnstat_import_source(&samples[index].sample_source);
        if let Some((previous_rx, previous_tx, previous_imported)) = previous {
            if samples[index].rx_bytes < previous_rx || (previous_imported && !imported) {
                rx_epoch = rx_epoch.saturating_add(1);
            }
            if samples[index].tx_bytes < previous_tx || (previous_imported && !imported) {
                tx_epoch = tx_epoch.saturating_add(1);
            }
        }
        samples[index].rx_counter_epoch = rx_epoch;
        samples[index].tx_counter_epoch = tx_epoch;
        previous = Some((samples[index].rx_bytes, samples[index].tx_bytes, imported));
    }
}

async fn insert_postgres_import_samples(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    client_id: &str,
    prepared: &PreparedInterfaceImport,
) -> Result<()> {
    let mut chunk = Vec::with_capacity(IMPORT_INSERT_BATCH_ROWS);
    for sample in prepared.samples(client_id) {
        chunk.push(sample?);
        if chunk.len() == IMPORT_INSERT_BATCH_ROWS {
            insert_postgres_import_sample_chunk(tx, &chunk).await?;
            chunk.clear();
        }
    }
    if !chunk.is_empty() {
        insert_postgres_import_sample_chunk(tx, &chunk).await?;
    }
    Ok(())
}

async fn insert_postgres_import_sample_chunk(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    samples: &[TrafficCounterSampleRecord],
) -> Result<()> {
    let mut builder = QueryBuilder::<Postgres>::new(
        "INSERT INTO traffic_counter_samples (client_id, source_kind, interface, observed_at, rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source) ",
    );
    builder.push_values(samples, |mut values, sample| {
        let observed_at = Utc
            .timestamp_opt(sample.observed_unix, 0)
            .single()
            .expect("validated import timestamp");
        values
            .push_bind(&sample.client_id)
            .push_bind(&sample.source_kind)
            .push_bind(&sample.interface)
            .push_bind(observed_at)
            .push_bind(sample.rx_bytes)
            .push_bind(sample.tx_bytes)
            .push_bind(sample.rx_counter_epoch)
            .push_bind(sample.tx_counter_epoch)
            .push_bind(&sample.sample_source);
    });
    builder.push(
        " ON CONFLICT (client_id, source_kind, interface, observed_at) DO UPDATE SET rx_bytes = EXCLUDED.rx_bytes, tx_bytes = EXCLUDED.tx_bytes, rx_counter_epoch = EXCLUDED.rx_counter_epoch, tx_counter_epoch = EXCLUDED.tx_counter_epoch, sample_source = EXCLUDED.sample_source, inbound_promoted = FALSE",
    );
    builder.build().execute(&mut **tx).await?;
    Ok(())
}

async fn recompute_postgres_stream_epochs(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    client_id: &str,
    interface: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        WITH ordered AS (
            SELECT
                observed_at,
                rx_bytes,
                tx_bytes,
                sample_source,
                LAG(rx_bytes) OVER (ORDER BY observed_at) AS previous_rx_bytes,
                LAG(tx_bytes) OVER (ORDER BY observed_at) AS previous_tx_bytes,
                LAG(sample_source) OVER (ORDER BY observed_at) AS previous_sample_source
            FROM traffic_counter_samples
            WHERE client_id = $1
              AND source_kind = 'host'
              AND interface = $2
        ),
        flags AS (
            SELECT
                observed_at,
                CASE
                    WHEN previous_rx_bytes IS NULL THEN 0
                    WHEN rx_bytes < previous_rx_bytes THEN 1
                    WHEN previous_sample_source LIKE 'vnstat_import:%'
                     AND sample_source NOT LIKE 'vnstat_import:%' THEN 1
                    ELSE 0
                END AS rx_increment,
                CASE
                    WHEN previous_tx_bytes IS NULL THEN 0
                    WHEN tx_bytes < previous_tx_bytes THEN 1
                    WHEN previous_sample_source LIKE 'vnstat_import:%'
                     AND sample_source NOT LIKE 'vnstat_import:%' THEN 1
                    ELSE 0
                END AS tx_increment
            FROM ordered
        ),
        epochs AS (
            SELECT
                observed_at,
                SUM(rx_increment) OVER (ORDER BY observed_at)::bigint AS rx_counter_epoch,
                SUM(tx_increment) OVER (ORDER BY observed_at)::bigint AS tx_counter_epoch
            FROM flags
        )
        UPDATE traffic_counter_samples sample
        SET
            rx_counter_epoch = epochs.rx_counter_epoch,
            tx_counter_epoch = epochs.tx_counter_epoch
        FROM epochs
        WHERE sample.client_id = $1
          AND sample.source_kind = 'host'
          AND sample.interface = $2
          AND sample.observed_at = epochs.observed_at
        "#,
    )
    .bind(client_id)
    .bind(interface)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn rebuild_postgres_import_rollups(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    client_id: &str,
    interfaces: &[String],
) -> Result<()> {
    sqlx::query(
        r#"
        WITH cutoff AS (
            SELECT (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
            ) - make_interval(days => $3) AS value
        ), sequenced AS MATERIALIZED (
            SELECT
                sample.interface,
                sample.observed_at,
                sample.rx_bytes,
                sample.tx_bytes,
                sample.rx_counter_epoch,
                sample.tx_counter_epoch,
                sample.sample_source,
                lag(sample.rx_bytes) OVER stream AS previous_rx_bytes,
                lag(sample.tx_bytes) OVER stream AS previous_tx_bytes,
                lag(sample.rx_counter_epoch) OVER stream
                    AS previous_rx_counter_epoch,
                lag(sample.tx_counter_epoch) OVER stream
                    AS previous_tx_counter_epoch,
                lag(sample.sample_source) OVER stream AS previous_sample_source
            FROM traffic_counter_samples sample
            WHERE sample.client_id = $1
              AND sample.source_kind = 'host'
              AND sample.interface = ANY($2::text[])
            WINDOW stream AS (
                PARTITION BY sample.interface
                ORDER BY sample.observed_at
            )
        ), direct AS MATERIALIZED (
            SELECT
                interface,
                CASE
                    WHEN observed_at >= (
                        date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                    ) - interval '91 days' THEN 3600
                    WHEN observed_at >= (
                        date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                    ) - interval '181 days' THEN 10800
                    WHEN observed_at >= (
                        date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                    ) - interval '366 days' THEN 21600
                    ELSE 86400
                END::integer AS bucket_secs,
                observed_at,
                rx_bytes,
                tx_bytes,
                rx_counter_epoch,
                tx_counter_epoch,
                sample_source,
                previous_rx_bytes,
                previous_tx_bytes,
                previous_rx_counter_epoch,
                previous_tx_counter_epoch,
                previous_sample_source
            FROM sequenced, cutoff
            WHERE sample_source LIKE 'vnstat_import:%'
              AND observed_at < cutoff.value
        ), bucketed AS (
            SELECT
                $1::text AS client_id,
                'host'::text AS source_kind,
                interface,
                'vnstat_import'::text AS origin_kind,
                bucket_secs,
                date_bin(
                    make_interval(secs => bucket_secs),
                    observed_at,
                    TIMESTAMPTZ '1970-01-01 00:00:00+00'
                ) AS bucket_start,
                coalesce(sum(CASE
                    WHEN rx_counter_epoch = previous_rx_counter_epoch
                     AND rx_bytes >= previous_rx_bytes
                    THEN rx_bytes - previous_rx_bytes ELSE 0 END), 0)::bigint
                    AS rx_bytes,
                coalesce(sum(CASE
                    WHEN tx_counter_epoch = previous_tx_counter_epoch
                     AND tx_bytes >= previous_tx_bytes
                    THEN tx_bytes - previous_tx_bytes ELSE 0 END), 0)::bigint
                    AS tx_bytes,
                count(*) FILTER (
                    WHERE rx_counter_epoch = previous_rx_counter_epoch
                      AND rx_bytes >= previous_rx_bytes
                )::integer AS rx_valid_count,
                count(*) FILTER (
                    WHERE tx_counter_epoch = previous_tx_counter_epoch
                      AND tx_bytes >= previous_tx_bytes
                )::integer AS tx_valid_count,
                count(*) FILTER (
                    WHERE (rx_counter_epoch = previous_rx_counter_epoch
                           AND rx_bytes >= previous_rx_bytes)
                       OR (tx_counter_epoch = previous_tx_counter_epoch
                           AND tx_bytes >= previous_tx_bytes)
                )::integer AS any_valid_count,
                count(*) FILTER (
                    WHERE previous_rx_counter_epoch IS NOT NULL
                      AND rx_counter_epoch <> previous_rx_counter_epoch
                      AND NOT (previous_sample_source LIKE 'vnstat_import:%'
                               AND sample_source NOT LIKE 'vnstat_import:%')
                )::integer AS rx_reset_count,
                count(*) FILTER (
                    WHERE previous_tx_counter_epoch IS NOT NULL
                      AND tx_counter_epoch <> previous_tx_counter_epoch
                      AND NOT (previous_sample_source LIKE 'vnstat_import:%'
                               AND sample_source NOT LIKE 'vnstat_import:%')
                )::integer AS tx_reset_count,
                count(*) FILTER (
                    WHERE previous_rx_counter_epoch IS NOT NULL
                      AND (rx_counter_epoch <> previous_rx_counter_epoch
                           OR tx_counter_epoch <> previous_tx_counter_epoch)
                      AND NOT (previous_sample_source LIKE 'vnstat_import:%'
                               AND sample_source NOT LIKE 'vnstat_import:%')
                )::integer AS any_reset_count,
                min(observed_at) AS first_observed_at,
                max(observed_at) AS latest_observed_at
            FROM direct
            GROUP BY interface, bucket_secs, bucket_start
        )
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        )
        SELECT
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        FROM bucketed
        ON CONFLICT (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start
        ) DO UPDATE SET
            rx_bytes = excluded.rx_bytes,
            tx_bytes = excluded.tx_bytes,
            rx_valid_count = excluded.rx_valid_count,
            tx_valid_count = excluded.tx_valid_count,
            any_valid_count = excluded.any_valid_count,
            rx_reset_count = excluded.rx_reset_count,
            tx_reset_count = excluded.tx_reset_count,
            any_reset_count = excluded.any_reset_count,
            first_observed_at = excluded.first_observed_at,
            latest_observed_at = excluded.latest_observed_at,
            updated_at = now()
        "#,
    )
    .bind(client_id)
    .bind(interfaces)
    .bind(MIN_TRAFFIC_COUNTER_RETENTION_DAYS)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"
        WITH cutoff AS (
            SELECT (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
            ) - make_interval(days => $3) AS value
        ), ranked AS MATERIALIZED (
            SELECT
                sample.ctid,
                sample.sample_source LIKE 'vnstat_import:%' AS imported,
                row_number() OVER (
                    PARTITION BY sample.interface
                    ORDER BY sample.observed_at DESC
                ) AS predecessor_rank
            FROM traffic_counter_samples sample, cutoff
            WHERE sample.client_id = $1
              AND sample.source_kind = 'host'
              AND sample.interface = ANY($2::text[])
              AND sample.observed_at < cutoff.value
        ), marked AS (
            UPDATE traffic_counter_samples sample
            SET inbound_promoted = TRUE
            FROM ranked
            WHERE sample.ctid = ranked.ctid
              AND ranked.imported
              AND ranked.predecessor_rank = 1
            RETURNING sample.ctid
        ), deleted AS (
            DELETE FROM traffic_counter_samples sample
            USING ranked
            WHERE sample.ctid = ranked.ctid
              AND (
                    (ranked.imported AND ranked.predecessor_rank > 1)
                    OR (sample.inbound_promoted AND ranked.predecessor_rank > 1)
              )
            RETURNING sample.ctid
        )
        SELECT
            (SELECT count(*) FROM marked)::bigint AS marked,
            (SELECT count(*) FROM deleted)::bigint AS deleted
        "#,
    )
    .bind(client_id)
    .bind(interfaces)
    .bind(MIN_TRAFFIC_COUNTER_RETENTION_DAYS)
    .fetch_one(&mut **tx)
    .await?;
    Ok(())
}

fn import_summary(prepared: &[PreparedInterfaceImport]) -> NetworkTrafficImportSummary {
    let minutes = prepared
        .iter()
        .map(|item| item.traffic.minute_count)
        .sum::<u64>();
    let rx = prepared
        .iter()
        .map(|item| item.imported_rx_bytes)
        .fold(0_u64, u64::saturating_add);
    let tx = prepared
        .iter()
        .map(|item| item.imported_tx_bytes)
        .fold(0_u64, u64::saturating_add);
    NetworkTrafficImportSummary {
        message: format!(
            "vnStat history imported: {} interface(s), {minutes} synthetic minute samples, {rx} RX bytes, {tx} TX bytes; live agent counters continue at the existing boundary",
            prepared.len()
        ),
    }
}

pub(crate) fn is_vnstat_import_source(source: &str) -> bool {
    source.starts_with(VNSTAT_IMPORT_SOURCE_PREFIX)
}

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
