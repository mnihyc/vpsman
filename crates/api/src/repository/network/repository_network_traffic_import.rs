use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use sqlx::{Postgres, QueryBuilder, Row};
use uuid::Uuid;
use vpsman_common::{
    NetworkTrafficImportBucket, NetworkTrafficImportResult,
    NETWORK_TRAFFIC_IMPORT_MAX_BUCKETS_PER_INTERFACE, NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES,
    NETWORK_TRAFFIC_IMPORT_MAX_LOOKBACK_SECS,
};

use crate::{model_alert_policies::TrafficCounterSampleRecord, repository::Repository};

pub(crate) const VNSTAT_IMPORT_SOURCE_PREFIX: &str = "vnstat_import:";
const IMPORT_INSERT_BATCH_ROWS: usize = 500;
const MAX_IMPORT_BUCKET_DURATION_SECS: u64 = 25 * 60 * 60;

#[derive(Clone, Debug)]
pub(crate) struct NetworkTrafficImportSummary {
    pub(crate) message: String,
}

#[derive(Clone, Debug)]
struct PreparedInterfaceImport {
    interface: String,
    start_unix: u64,
    end_unix: u64,
    samples: Vec<TrafficCounterSampleRecord>,
    imported_rx_bytes: u64,
    imported_tx_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct MinuteAssignment {
    rx_bytes: u64,
    tx_bytes: u64,
    assigned: bool,
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
        match self {
            Self::Memory(memory) => {
                let mut samples = memory.traffic_counter_samples.write().await;
                let prepared = prepare_imports(
                    job_id,
                    client_id,
                    interfaces,
                    start_unix,
                    result,
                    buckets,
                    now_unix,
                    &samples,
                )?;
                apply_memory_import(&mut samples, client_id, &prepared);
                let epochs = samples
                    .iter()
                    .filter(|sample| {
                        sample.client_id == client_id
                            && sample.source_kind == "host"
                            && interfaces.contains(&sample.interface)
                    })
                    .map(|sample| {
                        (
                            (sample.interface.clone(), sample.observed_unix),
                            (sample.rx_counter_epoch, sample.tx_counter_epoch),
                        )
                    })
                    .collect::<HashMap<_, _>>();
                drop(samples);

                let mut rates = memory.telemetry_network_rates.write().await;
                for rate in rates.iter_mut().filter(|rate| {
                    rate.client_id == client_id && interfaces.contains(&rate.interface)
                }) {
                    let Ok(observed_at) = chrono::DateTime::parse_from_rfc3339(&rate.bucket_start)
                    else {
                        continue;
                    };
                    if let Some((rx_epoch, tx_epoch)) =
                        epochs.get(&(rate.interface.clone(), observed_at.timestamp()))
                    {
                        rate.rx_counter_epoch = *rx_epoch;
                        rate.tx_counter_epoch = *tx_epoch;
                    }
                }
                Ok(import_summary(&prepared))
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_postgres_traffic_counter_streams(&mut tx, client_id).await?;
                let rows = sqlx::query(
                    r#"
                    SELECT
                        client_id,
                        source_kind,
                        interface,
                        observed_at::text AS observed_at,
                        EXTRACT(EPOCH FROM observed_at)::bigint AS observed_unix,
                        rx_bytes,
                        tx_bytes,
                        rx_counter_epoch,
                        tx_counter_epoch,
                        sample_source
                    FROM traffic_counter_samples
                    WHERE client_id = $1
                      AND source_kind = 'host'
                      AND interface = ANY($2::text[])
                    ORDER BY interface ASC, observed_at ASC
                    FOR UPDATE
                    "#,
                )
                .bind(client_id)
                .bind(interfaces)
                .fetch_all(&mut *tx)
                .await?;
                let existing = rows
                    .into_iter()
                    .map(|row| {
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
                    })
                    .collect::<std::result::Result<Vec<_>, sqlx::Error>>()?;
                let prepared = prepare_imports(
                    job_id,
                    client_id,
                    interfaces,
                    start_unix,
                    result,
                    buckets,
                    now_unix,
                    &existing,
                )?;

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
                .bind(interfaces)
                .execute(&mut *tx)
                .await?;
                for item in &prepared {
                    insert_postgres_import_samples(&mut tx, &item.samples).await?;
                    recompute_postgres_stream_epochs(&mut tx, client_id, &item.interface).await?;
                }
                sqlx::query(
                    r#"
                    UPDATE telemetry_network_rates rate
                    SET
                        rx_counter_epoch = sample.rx_counter_epoch,
                        tx_counter_epoch = sample.tx_counter_epoch,
                        updated_at = now()
                    FROM traffic_counter_samples sample
                    WHERE sample.client_id = $1
                      AND sample.source_kind = 'host'
                      AND sample.interface = ANY($2::text[])
                      AND rate.client_id = sample.client_id
                      AND rate.interface = sample.interface
                      AND rate.bucket_start = sample.observed_at
                    "#,
                )
                .bind(client_id)
                .bind(interfaces)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(import_summary(&prepared))
            }
        }
    }
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
        !interfaces.is_empty() && interfaces.len() <= NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES,
        "interface_count_out_of_range",
    )?;
    invalid_ensure(
        start_unix >= 60 && start_unix.is_multiple_of(60),
        "start_not_minute_aligned",
    )?;
    invalid_ensure(
        floor_minute(now_unix).saturating_sub(start_unix)
            <= NETWORK_TRAFFIC_IMPORT_MAX_LOOKBACK_SECS,
        "range_exceeds_lookback_limit",
    )?;

    let requested = interfaces.iter().cloned().collect::<BTreeSet<_>>();
    invalid_ensure(requested.len() == interfaces.len(), "duplicate_interface")?;
    let result_interfaces = result.interfaces.iter().cloned().collect::<BTreeSet<_>>();
    invalid_ensure(
        result_interfaces == requested && result.interfaces.len() == interfaces.len(),
        "agent_result_interface_mismatch",
    )?;
    let sources = result
        .sources
        .iter()
        .map(|source| source.interface.clone())
        .collect::<BTreeSet<_>>();
    invalid_ensure(
        sources == requested && result.sources.len() == interfaces.len(),
        "source_interface_mismatch",
    )?;
    invalid_ensure(
        buckets.len()
            <= interfaces.len() * NETWORK_TRAFFIC_IMPORT_MAX_BUCKETS_PER_INTERFACE,
        "bucket_count_exceeds_limit",
    )?;
    invalid_ensure(
        u32::try_from(buckets.len()).ok() == Some(result.bucket_count),
        "bucket_count_mismatch",
    )?;
    invalid_ensure(
        buckets
            .iter()
            .all(|bucket| requested.contains(&bucket.interface)),
        "bucket_interface_mismatch",
    )?;
    for interface in interfaces {
        invalid_ensure(
            buckets
                .iter()
                .filter(|bucket| bucket.interface == *interface)
                .count()
                <= NETWORK_TRAFFIC_IMPORT_MAX_BUCKETS_PER_INTERFACE,
            "interface_bucket_count_exceeds_limit",
        )?;
    }
    Ok(())
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
        let first_live_unix = existing
            .iter()
            .filter(|sample| {
                sample.client_id == client_id
                    && sample.source_kind == "host"
                    && sample.interface == *interface
                    && !is_vnstat_import_source(&sample.sample_source)
                    && sample.observed_unix >= i64::try_from(start_unix).unwrap_or(i64::MAX)
            })
            .map(|sample| sample.observed_unix)
            .min()
            .and_then(|value| u64::try_from(value).ok())
            .context("network_traffic_import_invalid:first_live_agent_sample_missing")?;
        invalid_ensure(
            first_live_unix > start_unix && first_live_unix <= now_minute,
            "range_already_covered_by_agent",
        )?;
        invalid_ensure(
            result.collected_until_unix >= first_live_unix,
            "agent_collection_predates_live_boundary",
        )?;

        let source = source_by_interface
            .get(interface.as_str())
            .context("network_traffic_import_invalid:source_missing")?;
        invalid_ensure(
            source
                .database_created_unix
                .is_some_and(|created| created <= start_unix),
            "vnstat_database_created_after_start",
        )?;
        invalid_ensure(
            source
                .source_updated_unix
                .map(floor_minute)
                .is_some_and(|updated| updated >= first_live_unix),
            "vnstat_source_not_updated_through_live_boundary",
        )?;

        let (minute_deltas, imported_rx_bytes, imported_tx_bytes) =
            expand_buckets_to_minutes(buckets, interface, start_unix, first_live_unix)?;
        let previous = existing
            .iter()
            .filter(|sample| {
                sample.client_id == client_id
                    && sample.source_kind == "host"
                    && sample.interface == *interface
                    && !is_vnstat_import_source(&sample.sample_source)
                    && sample.observed_unix < i64::try_from(start_unix).unwrap_or(i64::MAX)
            })
            .max_by_key(|sample| sample.observed_unix);
        let mut cumulative_rx = previous.map_or(0, |sample| sample.rx_bytes);
        let mut cumulative_tx = previous.map_or(0, |sample| sample.tx_bytes);
        invalid_ensure(
            cumulative_rx >= 0 && cumulative_tx >= 0,
            "negative_counter_baseline",
        )?;

        let mut samples = Vec::with_capacity(
            minute_deltas.len() + if previous.is_none() { 1 } else { 0 },
        );
        if previous.is_none() {
            samples.push(sample_record(
                client_id,
                interface,
                start_unix - 60,
                0,
                0,
                &import_source,
            )?);
        }
        for (observed_unix, rx_delta, tx_delta) in minute_deltas {
            cumulative_rx = cumulative_rx
                .checked_add(i64::try_from(rx_delta).context(
                    "network_traffic_import_invalid:rx_delta_exceeds_database_range",
                )?)
                .context("network_traffic_import_invalid:rx_counter_overflow")?;
            cumulative_tx = cumulative_tx
                .checked_add(i64::try_from(tx_delta).context(
                    "network_traffic_import_invalid:tx_delta_exceeds_database_range",
                )?)
                .context("network_traffic_import_invalid:tx_counter_overflow")?;
            samples.push(sample_record(
                client_id,
                interface,
                observed_unix,
                cumulative_rx,
                cumulative_tx,
                &import_source,
            )?);
        }
        prepared.push(PreparedInterfaceImport {
            interface: interface.clone(),
            start_unix,
            end_unix: first_live_unix,
            samples,
            imported_rx_bytes,
            imported_tx_bytes,
        });
    }
    Ok(prepared)
}

type MinuteTrafficRows = Vec<(u64, u64, u64)>;

fn expand_buckets_to_minutes(
    buckets: &[NetworkTrafficImportBucket],
    interface: &str,
    start_unix: u64,
    end_unix: u64,
) -> Result<(MinuteTrafficRows, u64, u64)> {
    invalid_ensure(end_unix > start_unix, "empty_range")?;
    let mut relevant = Vec::new();
    let mut identities = BTreeSet::new();
    for bucket in buckets.iter().filter(|bucket| bucket.interface == interface) {
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
    let span_minutes = usize::try_from((span_end - span_start) / 60)
        .context("network_traffic_import_invalid:minute_count_out_of_range")?;
    let maximum_span_minutes = usize::try_from(
        (NETWORK_TRAFFIC_IMPORT_MAX_LOOKBACK_SECS + 2 * MAX_IMPORT_BUCKET_DURATION_SECS) / 60,
    )
    .unwrap_or(usize::MAX);
    invalid_ensure(
        span_minutes <= maximum_span_minutes,
        "minute_count_exceeds_limit",
    )?;
    let mut assignments = vec![MinuteAssignment::default(); span_minutes];

    for bucket in relevant {
        let first = usize::try_from((bucket.start_unix - span_start) / 60)
            .context("network_traffic_import_invalid:bucket_index_out_of_range")?;
        let count = usize::try_from(u64::from(bucket.duration_secs) / 60)
            .context("network_traffic_import_invalid:bucket_duration_out_of_range")?;
        let last = first
            .checked_add(count)
            .context("network_traffic_import_invalid:bucket_index_overflow")?;
        invalid_ensure(last <= assignments.len(), "bucket_index_out_of_range")?;

        let cells = &mut assignments[first..last];
        let assigned_rx = cells.iter().try_fold(0_u64, |total, cell| {
            total
                .checked_add(cell.rx_bytes)
                .context("network_traffic_import_invalid:assigned_rx_overflow")
        })?;
        let assigned_tx = cells.iter().try_fold(0_u64, |total, cell| {
            total
                .checked_add(cell.tx_bytes)
                .context("network_traffic_import_invalid:assigned_tx_overflow")
        })?;
        invalid_ensure(
            assigned_rx <= bucket.rx_bytes && assigned_tx <= bucket.tx_bytes,
            "finer_bucket_total_exceeds_coarse_bucket",
        )?;
        let uncovered = cells.iter().filter(|cell| !cell.assigned).count();
        if uncovered == 0 {
            invalid_ensure(
                assigned_rx == bucket.rx_bytes && assigned_tx == bucket.tx_bytes,
                "fully_covered_bucket_total_mismatch",
            )?;
            continue;
        }

        distribute_residual(
            cells,
            bucket.rx_bytes - assigned_rx,
            bucket.tx_bytes - assigned_tx,
            uncovered,
        )?;
    }

    let requested_first = usize::try_from((start_unix - span_start) / 60)
        .context("network_traffic_import_invalid:requested_start_index_out_of_range")?;
    let requested_count = usize::try_from((end_unix - start_unix) / 60)
        .context("network_traffic_import_invalid:requested_minute_count_out_of_range")?;
    let requested_last = requested_first
        .checked_add(requested_count)
        .context("network_traffic_import_invalid:requested_end_index_overflow")?;
    invalid_ensure(
        requested_last <= assignments.len(),
        "requested_range_out_of_bounds",
    )?;

    let mut minute_rows = Vec::with_capacity(requested_count);
    let mut total_rx = 0_u64;
    let mut total_tx = 0_u64;
    for (offset, cell) in assignments[requested_first..requested_last]
        .iter()
        .enumerate()
    {
        if !cell.assigned {
            let gap_unix = start_unix.saturating_add(
                u64::try_from(offset).unwrap_or(u64::MAX).saturating_mul(60),
            );
            anyhow::bail!("network_traffic_import_invalid:vnstat_history_gap_at_{gap_unix}");
        }
        let observed_unix = start_unix
            .checked_add(u64::try_from(offset).unwrap_or(u64::MAX).saturating_mul(60))
            .context("network_traffic_import_invalid:minute_timestamp_overflow")?;
        total_rx = total_rx
            .checked_add(cell.rx_bytes)
            .context("network_traffic_import_invalid:rx_total_overflow")?;
        total_tx = total_tx
            .checked_add(cell.tx_bytes)
            .context("network_traffic_import_invalid:tx_total_overflow")?;
        minute_rows.push((observed_unix, cell.rx_bytes, cell.tx_bytes));
    }
    Ok((minute_rows, total_rx, total_tx))
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

fn distribute_residual(
    cells: &mut [MinuteAssignment],
    residual_rx: u64,
    residual_tx: u64,
    uncovered: usize,
) -> Result<()> {
    let uncovered_u64 = u64::try_from(uncovered)
        .context("network_traffic_import_invalid:uncovered_minute_count_out_of_range")?;
    invalid_ensure(uncovered_u64 > 0, "uncovered_minute_count_invalid")?;
    let rx_base = residual_rx / uncovered_u64;
    let rx_remainder = residual_rx % uncovered_u64;
    let tx_base = residual_tx / uncovered_u64;
    let tx_remainder = residual_tx % uncovered_u64;
    let mut index = 0_u64;
    for cell in cells.iter_mut().filter(|cell| !cell.assigned) {
        cell.rx_bytes = rx_base + u64::from(index < rx_remainder);
        cell.tx_bytes = tx_base + u64::from(index < tx_remainder);
        cell.assigned = true;
        index = index.saturating_add(1);
    }
    invalid_ensure(index == uncovered_u64, "uncovered_minute_count_changed")
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

fn apply_memory_import(
    samples: &mut Vec<TrafficCounterSampleRecord>,
    client_id: &str,
    prepared: &[PreparedInterfaceImport],
) {
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
        samples.extend(item.samples.iter().cloned());
    }
    for interface in interfaces {
        recompute_memory_stream_epochs(samples, client_id, interface);
    }
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
    samples: &[TrafficCounterSampleRecord],
) -> Result<()> {
    for chunk in samples.chunks(IMPORT_INSERT_BATCH_ROWS) {
        let mut builder = QueryBuilder::<Postgres>::new(
            "INSERT INTO traffic_counter_samples (client_id, source_kind, interface, observed_at, rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source) ",
        );
        builder.push_values(chunk, |mut values, sample| {
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
            " ON CONFLICT (client_id, source_kind, interface, observed_at) DO UPDATE SET rx_bytes = EXCLUDED.rx_bytes, tx_bytes = EXCLUDED.tx_bytes, rx_counter_epoch = EXCLUDED.rx_counter_epoch, tx_counter_epoch = EXCLUDED.tx_counter_epoch, sample_source = EXCLUDED.sample_source",
        );
        builder.build().execute(&mut **tx).await?;
    }
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

fn import_summary(prepared: &[PreparedInterfaceImport]) -> NetworkTrafficImportSummary {
    let minutes = prepared
        .iter()
        .map(|item| (item.end_unix - item.start_unix) / 60)
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

#[cfg(test)]
#[path = "tests_repository_network_traffic_import.rs"]
mod tests;
