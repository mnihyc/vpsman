use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use sqlx::{postgres::PgRow, PgPool, Postgres, Row};
use uuid::Uuid;
use vpsman_common::{
    NetworkTrafficImportBucket, NetworkTrafficImportResult, MIN_TRAFFIC_COUNTER_RETENTION_DAYS,
    NETWORK_TRAFFIC_IMPORT_MAX_BUCKETS_PER_INTERFACE, NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES,
};

use crate::{
    model_alert_policies::{TrafficCounterRollupRecord, TrafficCounterSampleRecord},
    repository::Repository,
};

pub(crate) const VNSTAT_IMPORT_SOURCE_PREFIX: &str = "vnstat_import:";
const MAX_IMPORT_BUCKET_DURATION_SECS: u64 = 367 * 24 * 60 * 60;
const POSTGRES_IMPORT_MAX_PREPARATION_ATTEMPTS: usize = 3;
// Canonical imported raw spans at most cutoff-60 through the latest current-day
// minute. Exceeding this derived bound indicates a prior interrupted/legacy
// incident that must be audited; it is not an alternate replacement policy.
const POSTGRES_IMPORT_MAX_RAW_ROWS_PER_INTERFACE: usize =
    (MIN_TRAFFIC_COUNTER_RETENTION_DAYS as usize + 1) * 24 * 60;
const POSTGRES_IMPORT_WORK_MEM_SQL: &str = "SET LOCAL work_mem = '32MB'";
const POSTGRES_IMPORT_SAME_SHAPE_UPDATE_BEGIN_SQL: &str =
    "SELECT set_config('vpsman.traffic_import_same_shape_update', 'on', true)";
const POSTGRES_IMPORT_SAME_SHAPE_UPDATE_END_SQL: &str =
    "SELECT set_config('vpsman.traffic_import_same_shape_update', 'off', true)";

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
    initial_rx_counter_epoch: i64,
    initial_tx_counter_epoch: i64,
    include_baseline: bool,
    import_source: String,
    traffic: ExpandedMinuteTraffic,
    imported_rx_bytes: u64,
    imported_tx_bytes: u64,
}

#[derive(Clone, Copy, Debug)]
struct PostgresImportEpochAdjustment {
    successor_unix: i64,
    rx_delta: i64,
    tx_delta: i64,
}

#[derive(Clone, Copy, Debug)]
struct PostgresImportRawPlan {
    minimum_unix: u64,
    rx_counter_epoch: i64,
    tx_counter_epoch: i64,
    delete_inbound_predecessor_unix: Option<i64>,
    successor_adjustment: Option<PostgresImportEpochAdjustment>,
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

#[derive(Clone, Debug)]
struct PostgresImportSnapshot {
    utc_day_start_unix: u64,
    raw_cutoff_unix: u64,
    boundary_samples: Vec<TrafficCounterSampleRecord>,
    imported_raw_stats: Vec<PostgresImportOwnedRawStats>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PostgresImportOwnedRawStats {
    interface: String,
    count: i64,
    first_observed_unix: Option<i64>,
    last_observed_unix: Option<i64>,
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
struct PreparedPostgresInterfaceImport {
    prepared: PreparedInterfaceImport,
    rollups: PreparedImportRollupRows,
    raw: PreparedImportRawRows,
}

#[derive(Debug)]
struct PreparedPostgresImport {
    snapshot: PostgresImportSnapshot,
    interfaces: Vec<PreparedPostgresInterfaceImport>,
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
                // Prepare every derived row before mutating either collection.  A
                // malformed segment must leave the in-memory repository exactly as
                // it was, just like the PostgreSQL transaction below.
                let imported_samples = prepare_memory_import_samples(client_id, &prepared)?;
                let (utc_day_start_unix, raw_cutoff_unix) =
                    memory_import_retention_boundaries(now_unix)?;
                let imported_rollups = prepare_memory_import_rollups(
                    client_id,
                    &prepared,
                    utc_day_start_unix,
                    raw_cutoff_unix,
                )?;
                apply_memory_import_rows(&mut samples, client_id, &prepared, &imported_samples);
                drop(samples);

                let mut rollups = memory.traffic_counter_rollups.write().await;
                rollups.retain(|rollup| {
                    rollup.client_id != client_id
                        || rollup.source_kind != "host"
                        || !resolved_interfaces.contains(&rollup.interface)
                        || rollup.origin_kind != "vnstat_import"
                });
                rollups.extend(imported_rollups);
                Ok(import_summary(&prepared))
            }
            Self::Postgres(pool) => {
                let effective_starts =
                    effective_interface_starts(resolved_interfaces, start_unix, result)?;
                for attempt in 0..POSTGRES_IMPORT_MAX_PREPARATION_ATTEMPTS {
                    let snapshot = load_postgres_import_preflight_snapshot(
                        pool,
                        client_id,
                        resolved_interfaces,
                        &effective_starts,
                    )
                    .await?;
                    ensure_postgres_import_owned_raw_is_bounded(&snapshot)?;
                    let postgres_prepared = prepare_postgres_import_outside_transaction(
                        job_id,
                        client_id,
                        resolved_interfaces,
                        start_unix,
                        result,
                        buckets,
                        now_unix,
                        snapshot,
                    )
                    .await?;

                    let mut tx = pool.begin().await?;
                    lock_postgres_traffic_import_client(&mut tx, client_id).await?;
                    lock_postgres_traffic_counter_streams(&mut tx, client_id).await?;
                    let locked_snapshot = load_postgres_import_snapshot(
                        &mut tx,
                        client_id,
                        resolved_interfaces,
                        &effective_starts,
                        true,
                        true,
                    )
                    .await?;
                    ensure_postgres_import_owned_raw_is_bounded(&locked_snapshot)?;
                    if !postgres_import_snapshots_match(
                        &postgres_prepared.snapshot,
                        &locked_snapshot,
                    ) {
                        tx.rollback().await?;
                        if attempt + 1 == POSTGRES_IMPORT_MAX_PREPARATION_ATTEMPTS {
                            anyhow::bail!(
                                "network_traffic_import_preflight_changed_after_{}_attempts",
                                POSTGRES_IMPORT_MAX_PREPARATION_ATTEMPTS
                            );
                        }
                        continue;
                    }

                    let mut raw_plans = Vec::with_capacity(postgres_prepared.interfaces.len());
                    for item in &postgres_prepared.interfaces {
                        raw_plans.push(
                            prepare_postgres_import_raw_plan(
                                &mut tx,
                                client_id,
                                &item.prepared,
                                locked_snapshot.raw_cutoff_unix,
                            )
                            .await?,
                        );
                    }
                    sqlx::query(POSTGRES_IMPORT_WORK_MEM_SQL)
                        .execute(&mut *tx)
                        .await?;

                    // vnStat replacement owns only its traffic-counter ledger.
                    // Every raw mutation is one retention-bounded interface
                    // statement; live telemetry remains independent.
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
                    for (item, raw_plan) in postgres_prepared.interfaces.iter().zip(&raw_plans) {
                        let same_shape = postgres_import_can_update_same_shape(
                            locked_snapshot.imported_raw_stats(&item.prepared.interface)?,
                            &item.raw,
                            raw_plan,
                        )?;
                        insert_postgres_import_rollups(
                            &mut tx,
                            client_id,
                            &item.prepared.interface,
                            &item.rollups,
                        )
                        .await?;
                        let same_shape_updated = if same_shape {
                            update_postgres_import_samples_same_shape(
                                &mut tx,
                                client_id,
                                &item.prepared,
                                &item.raw,
                                raw_plan,
                            )
                            .await?
                        } else {
                            false
                        };
                        if !same_shape_updated {
                            delete_postgres_import_samples(
                                &mut tx,
                                client_id,
                                &item.prepared.interface,
                                locked_snapshot.imported_raw_count(&item.prepared.interface)?,
                            )
                            .await?;
                            insert_postgres_import_samples(
                                &mut tx,
                                client_id,
                                &item.prepared,
                                &item.raw,
                                raw_plan,
                            )
                            .await?;
                        }
                        adjust_postgres_import_successor_epochs(
                            &mut tx,
                            client_id,
                            &item.prepared.interface,
                            raw_plan.successor_adjustment,
                        )
                        .await?;
                    }
                    tx.commit().await?;
                    let prepared = postgres_prepared
                        .interfaces
                        .iter()
                        .map(|item| &item.prepared)
                        .collect::<Vec<_>>();
                    return Ok(import_summary_refs(&prepared));
                }
                unreachable!("bounded PostgreSQL import preparation loop returns or errors")
            }
        }
    }
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
            extract(epoch FROM (
                utc_day_start - make_interval(days => $1)
            ))::bigint
        FROM boundary
        "#,
    )
    .bind(MIN_TRAFFIC_COUNTER_RETENTION_DAYS)
    .fetch_one(&mut **tx)
    .await?;
    Ok((
        u64::try_from(utc_day_start_unix)
            .context("network_traffic_import_invalid:utc_day_start_out_of_range")?,
        u64::try_from(raw_cutoff_unix)
            .context("network_traffic_import_invalid:raw_cutoff_out_of_range")?,
    ))
}

pub(crate) const POSTGRES_IMPORT_PREVIOUS_BOUNDARIES_SQL: &str = r#"
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
        WHERE ROW(
                  sample.client_id,
                  sample.source_kind,
                  sample.interface,
                  (sample.sample_source LIKE 'vnstat_import:%'),
                  sample.observed_at
              ) < ROW(
                  $1::text,
                  'host'::text,
                  requested.interface,
                  $4::boolean,
                  to_timestamp(requested.start_unix::double precision)
              )
          AND sample.client_id = $1::text
          AND sample.source_kind = 'host'
          AND sample.interface = requested.interface
          AND (sample.sample_source LIKE 'vnstat_import:%') = $4::boolean
          AND sample.observed_at < to_timestamp(requested.start_unix::double precision)
        ORDER BY
            sample.client_id DESC,
            sample.source_kind DESC,
            sample.interface DESC,
            (sample.sample_source LIKE 'vnstat_import:%') DESC,
            sample.observed_at DESC
        LIMIT 1
        FOR UPDATE OF sample
    ) boundary
    ORDER BY boundary.interface ASC
"#;

pub(crate) const POSTGRES_IMPORT_PREVIOUS_BOUNDARIES_NONLOCKING_SQL: &str = r#"
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
        WHERE ROW(
                  sample.client_id,
                  sample.source_kind,
                  sample.interface,
                  (sample.sample_source LIKE 'vnstat_import:%'),
                  sample.observed_at
              ) < ROW(
                  $1::text,
                  'host'::text,
                  requested.interface,
                  $4::boolean,
                  to_timestamp(requested.start_unix::double precision)
              )
          AND sample.client_id = $1::text
          AND sample.source_kind = 'host'
          AND sample.interface = requested.interface
          AND (sample.sample_source LIKE 'vnstat_import:%') = $4::boolean
          AND sample.observed_at < to_timestamp(requested.start_unix::double precision)
        ORDER BY
            sample.client_id DESC,
            sample.source_kind DESC,
            sample.interface DESC,
            (sample.sample_source LIKE 'vnstat_import:%') DESC,
            sample.observed_at DESC
        LIMIT 1
    ) boundary
    ORDER BY boundary.interface ASC
"#;

pub(crate) const POSTGRES_IMPORT_LIVE_BOUNDARIES_SQL: &str = r#"
    WITH desired_class AS MATERIALIZED (
        SELECT $4::boolean AS imported
    )
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
    FROM desired_class
    CROSS JOIN LATERAL (
        SELECT
            candidate.client_id,
            candidate.source_kind,
            candidate.interface,
            candidate.observed_at,
            candidate.rx_bytes,
            candidate.tx_bytes,
            candidate.rx_counter_epoch,
            candidate.tx_counter_epoch,
            candidate.sample_source
        FROM (
            (
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
                    0::integer AS branch_priority
                FROM traffic_counter_samples sample
                WHERE ROW(
                          sample.client_id,
                          sample.source_kind,
                          sample.interface,
                          (sample.sample_source LIKE 'vnstat_import:%'),
                          sample.observed_at
                      ) >= ROW(
                          $1::text,
                          'host'::text,
                          $2::text,
                          desired_class.imported,
                          to_timestamp($3::double precision)
                      )
                  AND sample.client_id = $1::text
                  AND sample.source_kind = 'host'
                  AND sample.interface = $2::text
                  AND (sample.sample_source LIKE 'vnstat_import:%')
                        = desired_class.imported
                  AND sample.observed_at >= to_timestamp($3::double precision)
                ORDER BY
                    sample.client_id ASC,
                    sample.source_kind ASC,
                    sample.interface ASC,
                    (sample.sample_source LIKE 'vnstat_import:%') ASC,
                    sample.observed_at ASC
                LIMIT 1
            )
            UNION ALL
            (
                SELECT
                    rollup.client_id,
                    rollup.source_kind,
                    rollup.interface,
                    GREATEST(
                        rollup.first_observed_at,
                        to_timestamp($3::double precision)
                    ),
                    0::bigint,
                    0::bigint,
                    0::bigint,
                    0::bigint,
                    'retained_live_rollup'::text,
                    1::integer AS branch_priority
                FROM traffic_counter_rollups rollup
                WHERE ROW(
                          rollup.client_id,
                          rollup.source_kind,
                          rollup.interface,
                          rollup.bucket_start
                      ) >= ROW(
                          $1::text,
                          'host'::text,
                          $2::text,
                          '-infinity'::timestamptz
                      )
                  AND ROW(
                          rollup.client_id,
                          rollup.source_kind,
                          rollup.interface,
                          rollup.bucket_start
                      ) <= ROW(
                          $1::text,
                          'host'::text,
                          $2::text,
                          'infinity'::timestamptz
                      )
                  AND rollup.client_id = $1::text
                  AND rollup.source_kind = 'host'
                  AND rollup.interface = $2::text
                  AND rollup.origin_kind = 'live'
                  AND rollup.latest_observed_at >= to_timestamp($3::double precision)
                ORDER BY
                    GREATEST(
                        rollup.first_observed_at,
                        to_timestamp($3::double precision)
                    ) ASC,
                    rollup.client_id ASC,
                    rollup.source_kind ASC,
                    rollup.interface ASC,
                    rollup.origin_kind ASC,
                    rollup.bucket_secs ASC,
                    rollup.bucket_start ASC
                LIMIT 1
            )
        ) candidate
        ORDER BY candidate.observed_at ASC, candidate.branch_priority ASC
        LIMIT 1
    ) boundary
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
    load_postgres_import_boundary_samples_for_starts(tx, client_id, interfaces, &starts, true).await
}

async fn load_postgres_import_boundary_samples_for_starts(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    client_id: &str,
    interfaces: &[String],
    effective_starts: &[i64],
    lock_previous: bool,
) -> Result<Vec<TrafficCounterSampleRecord>> {
    anyhow::ensure!(
        interfaces.len() == effective_starts.len(),
        "network_traffic_import_invalid:effective_start_count_mismatch"
    );
    let mut samples = Vec::with_capacity(interfaces.len().saturating_mul(2));
    let previous_query = if lock_previous {
        POSTGRES_IMPORT_PREVIOUS_BOUNDARIES_SQL
    } else {
        POSTGRES_IMPORT_PREVIOUS_BOUNDARIES_NONLOCKING_SQL
    };
    let previous_rows = sqlx::query(previous_query)
        .bind(client_id)
        .bind(interfaces)
        .bind(effective_starts)
        .bind(false)
        .fetch_all(&mut **tx)
        .await?;
    samples.extend(
        previous_rows
            .into_iter()
            .map(postgres_traffic_counter_sample)
            .collect::<std::result::Result<Vec<_>, sqlx::Error>>()?,
    );
    for (interface, effective_start) in interfaces.iter().zip(effective_starts) {
        let Some(row) = sqlx::query(POSTGRES_IMPORT_LIVE_BOUNDARIES_SQL)
            .bind(client_id)
            .bind(interface)
            .bind(effective_start)
            .bind(false)
            .fetch_optional(&mut **tx)
            .await?
        else {
            continue;
        };
        let sample = postgres_traffic_counter_sample(row)?;
        anyhow::ensure!(
            sample.interface == *interface,
            "network_traffic_import_live_boundary_mismatch"
        );
        samples.push(sample);
    }
    Ok(samples)
}

pub(crate) const POSTGRES_IMPORT_OWNED_RAW_COUNTS_SQL: &str = r#"
    SELECT
        $2::text AS interface,
        count(*)::bigint,
        min(extract(epoch FROM bounded_imported.observed_at))::bigint,
        max(extract(epoch FROM bounded_imported.observed_at))::bigint
    FROM (
        SELECT sample.observed_at
        FROM traffic_counter_samples sample
        WHERE ROW(
                  sample.client_id,
                  sample.source_kind,
                  sample.interface,
                  (sample.sample_source LIKE 'vnstat_import:%'),
                  sample.observed_at
              ) >= ROW(
                  $1::text,
                  'host'::text,
                  $2::text,
                  $4::boolean,
                  '-infinity'::timestamptz
              )
          AND ROW(
                  sample.client_id,
                  sample.source_kind,
                  sample.interface,
                  (sample.sample_source LIKE 'vnstat_import:%'),
                  sample.observed_at
              ) <= ROW(
                  $1::text,
                  'host'::text,
                  $2::text,
                  $4::boolean,
                  'infinity'::timestamptz
              )
          AND sample.client_id = $1::text
          AND sample.source_kind = 'host'
          AND sample.interface = $2::text
          AND (sample.sample_source LIKE 'vnstat_import:%') = $4::boolean
        LIMIT $3
    ) bounded_imported
"#;

async fn load_postgres_import_owned_raw_counts(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    client_id: &str,
    interfaces: &[String],
) -> Result<Vec<PostgresImportOwnedRawStats>> {
    let probe_limit = i64::try_from(POSTGRES_IMPORT_MAX_RAW_ROWS_PER_INTERFACE + 1)
        .context("network_traffic_import_raw_probe_limit_out_of_range")?;
    let mut counts = Vec::with_capacity(interfaces.len());
    for interface in interfaces {
        let (interface, count, first_observed_unix, last_observed_unix) =
            sqlx::query_as::<_, (String, i64, Option<i64>, Option<i64>)>(
                POSTGRES_IMPORT_OWNED_RAW_COUNTS_SQL,
            )
            .bind(client_id)
            .bind(interface)
            .bind(probe_limit)
            .bind(true)
            .fetch_one(&mut **tx)
            .await?;
        counts.push(PostgresImportOwnedRawStats {
            interface,
            count,
            first_observed_unix,
            last_observed_unix,
        });
    }
    anyhow::ensure!(
        counts.len() == interfaces.len()
            && counts
                .iter()
                .zip(interfaces)
                .all(|(returned, requested)| returned.interface == *requested),
        "network_traffic_import_raw_count_mismatch"
    );
    Ok(counts)
}

async fn load_postgres_import_snapshot(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    client_id: &str,
    interfaces: &[String],
    effective_starts: &[i64],
    lock_previous: bool,
    include_imported_raw_stats: bool,
) -> Result<PostgresImportSnapshot> {
    let (utc_day_start_unix, raw_cutoff_unix) = postgres_import_retention_boundaries(tx).await?;
    // The locked snapshot probes the owned-row bound before boundary
    // discovery: a legacy dirty ledger must fail closed without scanning
    // through millions of imported rows while looking for the first
    // non-import successor.  The repeatable-read preflight intentionally
    // omits this duplicate probe; its boundary snapshot is revalidated under
    // the client/advisory locks, where this bound is checked authoritatively.
    let imported_raw_counts = if include_imported_raw_stats {
        let counts = load_postgres_import_owned_raw_counts(tx, client_id, interfaces).await?;
        ensure_postgres_import_owned_raw_counts_are_bounded(&counts)?;
        counts
    } else {
        Vec::new()
    };
    let boundary_samples = load_postgres_import_boundary_samples_for_starts(
        tx,
        client_id,
        interfaces,
        effective_starts,
        lock_previous,
    )
    .await?;
    Ok(PostgresImportSnapshot {
        utc_day_start_unix,
        raw_cutoff_unix,
        boundary_samples,
        imported_raw_stats: imported_raw_counts,
    })
}

async fn load_postgres_import_preflight_snapshot(
    pool: &PgPool,
    client_id: &str,
    interfaces: &[String],
    effective_starts: &[i64],
) -> Result<PostgresImportSnapshot> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let snapshot = load_postgres_import_snapshot(
        &mut tx,
        client_id,
        interfaces,
        effective_starts,
        false,
        false,
    )
    .await?;
    tx.commit().await?;
    Ok(snapshot)
}

impl PostgresImportSnapshot {
    fn imported_raw_stats(&self, interface: &str) -> Result<&PostgresImportOwnedRawStats> {
        let count = self
            .imported_raw_stats
            .iter()
            .find(|stats| stats.interface == interface)
            .context("network_traffic_import_raw_count_missing")?;
        Ok(count)
    }

    fn imported_raw_count(&self, interface: &str) -> Result<u64> {
        u64::try_from(self.imported_raw_stats(interface)?.count)
            .context("network_traffic_import_raw_count_negative")
    }
}

fn ensure_postgres_import_owned_raw_is_bounded(snapshot: &PostgresImportSnapshot) -> Result<()> {
    ensure_postgres_import_owned_raw_counts_are_bounded(&snapshot.imported_raw_stats)
}

fn ensure_postgres_import_owned_raw_counts_are_bounded(
    imported_raw_stats: &[PostgresImportOwnedRawStats],
) -> Result<()> {
    let maximum = i64::try_from(POSTGRES_IMPORT_MAX_RAW_ROWS_PER_INTERFACE)
        .context("network_traffic_import_raw_limit_out_of_range")?;
    for stats in imported_raw_stats {
        anyhow::ensure!(
            stats.count >= 0,
            "network_traffic_import_raw_count_negative"
        );
        anyhow::ensure!(
            (stats.count == 0)
                == (stats.first_observed_unix.is_none() && stats.last_observed_unix.is_none()),
            "network_traffic_import_raw_bounds_missing"
        );
        if stats.count > 0 {
            let first = stats
                .first_observed_unix
                .context("network_traffic_import_raw_first_missing")?;
            let last = stats
                .last_observed_unix
                .context("network_traffic_import_raw_last_missing")?;
            anyhow::ensure!(
                first >= 0 && last >= first && first % 60 == 0 && last % 60 == 0,
                "network_traffic_import_raw_bounds_invalid"
            );
        }
        if stats.count > maximum {
            anyhow::bail!(
                "network_traffic_import_recovery_required:imported_raw_rows_exceed_retention_bound:{}:at_least_{}:max_{}",
                stats.interface,
                stats.count,
                maximum
            );
        }
    }
    Ok(())
}

fn postgres_import_snapshots_match(
    prepared: &PostgresImportSnapshot,
    locked: &PostgresImportSnapshot,
) -> bool {
    prepared.utc_day_start_unix == locked.utc_day_start_unix
        && prepared.raw_cutoff_unix == locked.raw_cutoff_unix
        && (prepared.imported_raw_stats.is_empty()
            || prepared.imported_raw_stats == locked.imported_raw_stats)
        && prepared.boundary_samples.len() == locked.boundary_samples.len()
        && prepared
            .boundary_samples
            .iter()
            .zip(&locked.boundary_samples)
            .all(|(prepared, locked)| {
                prepared.client_id == locked.client_id
                    && prepared.source_kind == locked.source_kind
                    && prepared.interface == locked.interface
                    && prepared.observed_at == locked.observed_at
                    && prepared.observed_unix == locked.observed_unix
                    && prepared.rx_bytes == locked.rx_bytes
                    && prepared.tx_bytes == locked.tx_bytes
                    && prepared.rx_counter_epoch == locked.rx_counter_epoch
                    && prepared.tx_counter_epoch == locked.tx_counter_epoch
                    && prepared.sample_source == locked.sample_source
            })
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
        let initial_rx_counter_epoch = previous.map_or(0, |sample| sample.rx_counter_epoch);
        let initial_tx_counter_epoch = previous.map_or(0, |sample| sample.tx_counter_epoch);
        invalid_ensure(
            cumulative_rx >= 0 && cumulative_tx >= 0,
            "negative_counter_baseline",
        )?;
        anyhow::ensure!(
            initial_rx_counter_epoch >= 0 && initial_tx_counter_epoch >= 0,
            "network_traffic_import_predecessor_epoch_negative"
        );
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
            initial_rx_counter_epoch,
            initial_tx_counter_epoch,
            include_baseline: previous.is_none(),
            import_source: import_source.clone(),
            traffic,
            imported_rx_bytes,
            imported_tx_bytes,
        });
    }
    Ok(prepared)
}

#[allow(clippy::too_many_arguments)]
async fn prepare_postgres_import_outside_transaction(
    job_id: Uuid,
    client_id: &str,
    interfaces: &[String],
    start_unix: u64,
    result: &NetworkTrafficImportResult,
    buckets: &[NetworkTrafficImportBucket],
    now_unix: u64,
    snapshot: PostgresImportSnapshot,
) -> Result<PreparedPostgresImport> {
    let client_id = client_id.to_string();
    let interfaces = interfaces.to_vec();
    let result = result.clone();
    let buckets = buckets.to_vec();
    tokio::task::spawn_blocking(move || {
        prepare_postgres_import(
            job_id,
            &client_id,
            &interfaces,
            start_unix,
            &result,
            &buckets,
            now_unix,
            snapshot,
        )
    })
    .await
    .context("network_traffic_import_preparation_task_failed")?
}

#[allow(clippy::too_many_arguments)]
fn prepare_postgres_import(
    job_id: Uuid,
    client_id: &str,
    interfaces: &[String],
    start_unix: u64,
    result: &NetworkTrafficImportResult,
    buckets: &[NetworkTrafficImportBucket],
    now_unix: u64,
    snapshot: PostgresImportSnapshot,
) -> Result<PreparedPostgresImport> {
    let prepared = prepare_imports(
        job_id,
        client_id,
        interfaces,
        start_unix,
        result,
        buckets,
        now_unix,
        &snapshot.boundary_samples,
    )?;
    let mut postgres_interfaces = Vec::with_capacity(prepared.len());
    for prepared in prepared {
        let rollups = prepare_import_rollup_rows(
            &prepared.interface,
            prepare_import_rollups(
                &prepared,
                snapshot.utc_day_start_unix,
                snapshot.raw_cutoff_unix,
            )?,
        )?;
        let raw_minimum =
            postgres_import_raw_superset_minimum(&prepared, snapshot.raw_cutoff_unix)?;
        let raw = prepare_import_raw_rows(&prepared, raw_minimum, snapshot.raw_cutoff_unix)?;
        postgres_interfaces.push(PreparedPostgresInterfaceImport {
            prepared,
            rollups,
            raw,
        });
    }
    Ok(PreparedPostgresImport {
        snapshot,
        interfaces: postgres_interfaces,
    })
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

fn postgres_import_raw_superset_minimum(
    prepared: &PreparedInterfaceImport,
    raw_cutoff_unix: u64,
) -> Result<u64> {
    invalid_ensure(
        raw_cutoff_unix.is_multiple_of(60),
        "raw_retention_cutoff_not_minute_aligned",
    )?;
    let natural_start = if prepared.include_baseline {
        prepared.start_unix - 60
    } else {
        prepared.start_unix
    };
    if natural_start >= raw_cutoff_unix {
        return Ok(raw_cutoff_unix);
    }
    Ok(prepared
        .end_unix
        .checked_sub(60)
        .context("network_traffic_import_invalid:sample_timestamp_underflow")?
        .min(
            raw_cutoff_unix
                .checked_sub(60)
                .context("network_traffic_import_invalid:raw_cutoff_underflow")?,
        ))
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
    for segment in segments {
        if segment.end_unix <= start_unix {
            continue;
        }
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
            && raw_cutoff_unix.is_multiple_of(86_400)
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

fn prepare_memory_import_samples(
    client_id: &str,
    prepared: &[PreparedInterfaceImport],
) -> Result<Vec<TrafficCounterSampleRecord>> {
    prepared
        .iter()
        .flat_map(|item| item.samples(client_id))
        .collect()
}

fn apply_memory_import_rows(
    samples: &mut Vec<TrafficCounterSampleRecord>,
    client_id: &str,
    prepared: &[PreparedInterfaceImport],
    imported_samples: &[TrafficCounterSampleRecord],
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
    samples.extend(imported_samples.iter().cloned());
    for interface in interfaces {
        recompute_memory_stream_epochs(samples, client_id, interface);
    }
}

fn memory_import_retention_boundaries(now_unix: u64) -> Result<(u64, u64)> {
    let utc_day_start_unix = now_unix - now_unix % 86_400;
    let retention_secs = u64::try_from(MIN_TRAFFIC_COUNTER_RETENTION_DAYS)
        .context("network_traffic_import_invalid:retention_days_out_of_range")?
        .checked_mul(86_400)
        .context("network_traffic_import_invalid:retention_window_overflow")?;
    let raw_cutoff_unix = utc_day_start_unix
        .checked_sub(retention_secs)
        .context("network_traffic_import_invalid:raw_cutoff_out_of_range")?;
    Ok((utc_day_start_unix, raw_cutoff_unix))
}

fn prepare_memory_import_rollups(
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

fn postgres_import_raw_start_index(
    raw: &PreparedImportRawRows,
    raw_plan: &PostgresImportRawPlan,
) -> Result<usize> {
    let row_count = raw.observed_unix.len();
    anyhow::ensure!(
        raw.rx_bytes.len() == row_count
            && raw.tx_bytes.len() == row_count
            && raw.inbound_promoted.len() == row_count,
        "network_traffic_import_raw_array_length_mismatch"
    );
    let minimum_unix = i64::try_from(raw_plan.minimum_unix)
        .context("network_traffic_import_invalid:sample_timestamp_out_of_range")?;
    Ok(raw
        .observed_unix
        .partition_point(|observed_unix| *observed_unix < minimum_unix))
}

fn postgres_import_can_update_same_shape(
    stats: &PostgresImportOwnedRawStats,
    raw: &PreparedImportRawRows,
    raw_plan: &PostgresImportRawPlan,
) -> Result<bool> {
    let start_index = postgres_import_raw_start_index(raw, raw_plan)?;
    let desired = &raw.observed_unix[start_index..];
    let desired_count =
        i64::try_from(desired.len()).context("network_traffic_import_raw_count_out_of_range")?;
    if desired.is_empty() {
        return Ok(stats.count == 0);
    }
    anyhow::ensure!(
        desired.windows(2).all(|pair| pair[1] - pair[0] == 60),
        "network_traffic_import_raw_timestamps_not_dense"
    );
    let first = *desired
        .first()
        .context("network_traffic_import_raw_first_missing")?;
    let last = *desired
        .last()
        .context("network_traffic_import_raw_last_missing")?;
    let dense_count = (last - first)
        .checked_div(60)
        .and_then(|span| span.checked_add(1))
        .context("network_traffic_import_raw_dense_count_overflow")?;
    Ok(stats.count == desired_count
        && stats.first_observed_unix == Some(first)
        && stats.last_observed_unix == Some(last)
        && dense_count == desired_count)
}

async fn delete_postgres_import_samples(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    client_id: &str,
    interface: &str,
    expected_rows: u64,
) -> Result<()> {
    let maximum_rows = u64::try_from(POSTGRES_IMPORT_MAX_RAW_ROWS_PER_INTERFACE)
        .context("network_traffic_import_raw_limit_out_of_range")?;
    anyhow::ensure!(
        expected_rows <= maximum_rows,
        "network_traffic_import_recovery_required:imported_raw_rows_exceed_retention_bound"
    );
    let deleted = sqlx::query(
        r#"
        DELETE FROM traffic_counter_samples
        WHERE client_id = $1
          AND source_kind = 'host'
          AND interface = $2
          AND sample_source LIKE 'vnstat_import:%'
        "#,
    )
    .bind(client_id)
    .bind(interface)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    anyhow::ensure!(
        deleted == expected_rows,
        "network_traffic_import_owned_raw_changed_after_lock:{interface}:expected_{expected_rows}:deleted_{deleted}"
    );
    Ok(())
}

async fn update_postgres_import_samples_same_shape(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    client_id: &str,
    prepared: &PreparedInterfaceImport,
    raw: &PreparedImportRawRows,
    raw_plan: &PostgresImportRawPlan,
) -> Result<bool> {
    sqlx::query("SAVEPOINT vpsman_same_shape_update")
        .execute(&mut **tx)
        .await?;
    if let Some(observed_unix) = raw_plan.delete_inbound_predecessor_unix {
        if let Err(error) = sqlx::query(
            r#"
            DELETE FROM traffic_counter_samples
            WHERE client_id = $1
              AND source_kind = 'host'
              AND interface = $2
              AND observed_at = to_timestamp($3::double precision)
              AND sample_source NOT LIKE 'vnstat_import:%'
              AND inbound_promoted
            "#,
        )
        .bind(client_id)
        .bind(&prepared.interface)
        .bind(observed_unix)
        .execute(&mut **tx)
        .await
        {
            sqlx::query("ROLLBACK TO SAVEPOINT vpsman_same_shape_update")
                .execute(&mut **tx)
                .await?;
            sqlx::query("RELEASE SAVEPOINT vpsman_same_shape_update")
                .execute(&mut **tx)
                .await?;
            return Err(error.into());
        }
    }
    let start_index = postgres_import_raw_start_index(raw, raw_plan)?;
    if start_index == raw.observed_unix.len() {
        sqlx::query("RELEASE SAVEPOINT vpsman_same_shape_update")
            .execute(&mut **tx)
            .await?;
        return Ok(true);
    }
    // `unnest($6, ...)` is intentionally parameterized.  PostgreSQL may
    // otherwise promote this prepared statement to a generic plan that
    // estimates ten rows and performs 47k point probes per client.  Force a
    // custom plan only for this statement so the actual dense array cardinality
    // selects the bounded hash/range join.  Preserve a caller/session-local
    // setting rather than assuming the default plan mode.
    let previous_plan_cache_mode =
        sqlx::query_scalar::<_, String>("SELECT current_setting('plan_cache_mode')")
            .fetch_one(&mut **tx)
            .await?;
    sqlx::query("SELECT set_config('plan_cache_mode', 'force_custom_plan', true)")
        .execute(&mut **tx)
        .await?;
    sqlx::query(POSTGRES_IMPORT_SAME_SHAPE_UPDATE_BEGIN_SQL)
        .execute(&mut **tx)
        .await?;
    let update_result = sqlx::query(
        r#"
        UPDATE traffic_counter_samples AS existing
        SET
            rx_bytes = incoming.rx_bytes,
            tx_bytes = incoming.tx_bytes,
            rx_counter_epoch = $4,
            tx_counter_epoch = $5,
            sample_source = $3,
            inbound_promoted = incoming.inbound_promoted
        FROM unnest(
            $6::bigint[], $7::bigint[], $8::bigint[], $9::boolean[]
        ) AS incoming(observed_unix, rx_bytes, tx_bytes, inbound_promoted)
        WHERE existing.client_id = $1
          AND existing.source_kind = 'host'
          AND existing.interface = $2
          AND existing.observed_at = to_timestamp(incoming.observed_unix::double precision)
          AND starts_with(existing.sample_source, 'vnstat_import:')
        "#,
    )
    .bind(client_id)
    .bind(&prepared.interface)
    .bind(&prepared.import_source)
    .bind(raw_plan.rx_counter_epoch)
    .bind(raw_plan.tx_counter_epoch)
    .bind(&raw.observed_unix[start_index..])
    .bind(&raw.rx_bytes[start_index..])
    .bind(&raw.tx_bytes[start_index..])
    .bind(&raw.inbound_promoted[start_index..])
    .execute(&mut **tx)
    .await;
    let updated = match update_result {
        Ok(update_result) => update_result.rows_affected(),
        Err(update_error) => {
            // Once a statement fails, issuing cleanup SQL before rolling back
            // the savepoint only creates secondary "current transaction is
            // aborted" errors.  Rolling back the savepoint restores both
            // transaction-local settings, so go straight to the rollback.
            sqlx::query("ROLLBACK TO SAVEPOINT vpsman_same_shape_update")
                .execute(&mut **tx)
                .await?;
            sqlx::query("RELEASE SAVEPOINT vpsman_same_shape_update")
                .execute(&mut **tx)
                .await?;
            return Err(update_error.into());
        }
    };
    if let Err(reset_error) = sqlx::query(POSTGRES_IMPORT_SAME_SHAPE_UPDATE_END_SQL)
        .execute(&mut **tx)
        .await
    {
        sqlx::query("ROLLBACK TO SAVEPOINT vpsman_same_shape_update")
            .execute(&mut **tx)
            .await?;
        sqlx::query("RELEASE SAVEPOINT vpsman_same_shape_update")
            .execute(&mut **tx)
            .await?;
        return Err(reset_error.into());
    }
    if let Err(restore_plan_cache_mode_error) =
        sqlx::query("SELECT set_config('plan_cache_mode', $1, true)")
            .bind(&previous_plan_cache_mode)
            .execute(&mut **tx)
            .await
    {
        sqlx::query("ROLLBACK TO SAVEPOINT vpsman_same_shape_update")
            .execute(&mut **tx)
            .await?;
        sqlx::query("RELEASE SAVEPOINT vpsman_same_shape_update")
            .execute(&mut **tx)
            .await?;
        return Err(restore_plan_cache_mode_error.into());
    }
    let expected = u64::try_from(raw.observed_unix.len() - start_index)
        .context("network_traffic_import_raw_count_out_of_range")?;
    if updated != expected {
        sqlx::query("ROLLBACK TO SAVEPOINT vpsman_same_shape_update")
            .execute(&mut **tx)
            .await?;
        sqlx::query("RELEASE SAVEPOINT vpsman_same_shape_update")
            .execute(&mut **tx)
            .await?;
        return Ok(false);
    }
    sqlx::query("RELEASE SAVEPOINT vpsman_same_shape_update")
        .execute(&mut **tx)
        .await?;
    Ok(true)
}

async fn insert_postgres_import_samples(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    client_id: &str,
    prepared: &PreparedInterfaceImport,
    raw: &PreparedImportRawRows,
    raw_plan: &PostgresImportRawPlan,
) -> Result<()> {
    if let Some(observed_unix) = raw_plan.delete_inbound_predecessor_unix {
        sqlx::query(
            r#"
            DELETE FROM traffic_counter_samples
            WHERE client_id = $1
              AND source_kind = 'host'
              AND interface = $2
              AND observed_at = to_timestamp($3::double precision)
              AND sample_source NOT LIKE 'vnstat_import:%'
              AND inbound_promoted
            "#,
        )
        .bind(client_id)
        .bind(&prepared.interface)
        .bind(observed_unix)
        .execute(&mut **tx)
        .await?;
    }
    let row_count = raw.observed_unix.len();
    let start_index = postgres_import_raw_start_index(raw, raw_plan)?;
    if start_index == row_count {
        return Ok(());
    }
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch,
            sample_source, inbound_promoted
        )
        SELECT
            $1,
            'host',
            $2,
            to_timestamp(imported.observed_unix::double precision),
            imported.rx_bytes,
            imported.tx_bytes,
            $4,
            $5,
            $3,
            imported.inbound_promoted
        FROM unnest(
            $6::bigint[], $7::bigint[], $8::bigint[], $9::boolean[]
        ) AS imported(observed_unix, rx_bytes, tx_bytes, inbound_promoted)
        ON CONFLICT (client_id, source_kind, interface, observed_at) DO UPDATE SET
            rx_bytes = EXCLUDED.rx_bytes,
            tx_bytes = EXCLUDED.tx_bytes,
            rx_counter_epoch = EXCLUDED.rx_counter_epoch,
            tx_counter_epoch = EXCLUDED.tx_counter_epoch,
            sample_source = EXCLUDED.sample_source,
            inbound_promoted = EXCLUDED.inbound_promoted
        "#,
    )
    .bind(client_id)
    .bind(&prepared.interface)
    .bind(&prepared.import_source)
    .bind(raw_plan.rx_counter_epoch)
    .bind(raw_plan.tx_counter_epoch)
    .bind(&raw.observed_unix[start_index..])
    .bind(&raw.rx_bytes[start_index..])
    .bind(&raw.tx_bytes[start_index..])
    .bind(&raw.inbound_promoted[start_index..])
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) const POSTGRES_IMPORT_RAW_PREDECESSOR_SQL: &str = r#"
    WITH desired_class AS MATERIALIZED (
        SELECT $4::boolean AS imported
    )
    SELECT
        boundary.observed_unix,
        boundary.inbound_promoted
    FROM desired_class
    CROSS JOIN LATERAL (
        SELECT
            extract(epoch FROM sample.observed_at)::bigint AS observed_unix,
            sample.inbound_promoted
        FROM traffic_counter_samples sample
        WHERE ROW(
                  sample.client_id,
                  sample.source_kind,
                  sample.interface,
                  (sample.sample_source LIKE 'vnstat_import:%'),
                  sample.observed_at
              ) < ROW(
                  $1::text,
                  'host'::text,
                  $2::text,
                  desired_class.imported,
                  to_timestamp($3::double precision)
              )
          AND sample.client_id = $1::text
          AND sample.source_kind = 'host'
          AND sample.interface = $2::text
          AND (sample.sample_source LIKE 'vnstat_import:%') = desired_class.imported
          AND sample.observed_at < to_timestamp($3::double precision)
        ORDER BY
            sample.client_id DESC,
            sample.source_kind DESC,
            sample.interface DESC,
            (sample.sample_source LIKE 'vnstat_import:%') DESC,
            sample.observed_at DESC
        LIMIT 1
        FOR UPDATE OF sample
    ) boundary
"#;

pub(crate) const POSTGRES_IMPORT_RAW_SUCCESSOR_SQL: &str = r#"
    WITH desired_class AS MATERIALIZED (
        SELECT $4::boolean AS imported
    )
    SELECT boundary.rx_counter_epoch, boundary.tx_counter_epoch
    FROM desired_class
    CROSS JOIN LATERAL (
        SELECT sample.rx_counter_epoch, sample.tx_counter_epoch
        FROM traffic_counter_samples sample
        WHERE ROW(
                  sample.client_id,
                  sample.source_kind,
                  sample.interface,
                  (sample.sample_source LIKE 'vnstat_import:%'),
                  sample.observed_at
              ) >= ROW(
                  $1::text,
                  'host'::text,
                  $2::text,
                  desired_class.imported,
                  to_timestamp($3::double precision)
              )
          AND sample.client_id = $1::text
          AND sample.source_kind = 'host'
          AND sample.interface = $2::text
          AND (sample.sample_source LIKE 'vnstat_import:%') = desired_class.imported
          AND sample.observed_at >= to_timestamp($3::double precision)
        ORDER BY
            sample.client_id ASC,
            sample.source_kind ASC,
            sample.interface ASC,
            (sample.sample_source LIKE 'vnstat_import:%') ASC,
            sample.observed_at ASC
        LIMIT 1
        FOR UPDATE OF sample
    ) boundary
"#;

async fn prepare_postgres_import_raw_plan(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    client_id: &str,
    prepared: &PreparedInterfaceImport,
    raw_cutoff_unix: u64,
) -> Result<PostgresImportRawPlan> {
    let natural_start = if prepared.include_baseline {
        prepared.start_unix - 60
    } else {
        prepared.start_unix
    };
    let mut minimum_unix = raw_cutoff_unix;
    let mut delete_inbound_predecessor_unix = None;
    if natural_start < raw_cutoff_unix {
        let candidate_unix = prepared
            .end_unix
            .checked_sub(60)
            .context("network_traffic_import_invalid:sample_timestamp_underflow")?
            .min(
                raw_cutoff_unix
                    .checked_sub(60)
                    .context("network_traffic_import_invalid:raw_cutoff_underflow")?,
            );
        let candidate_unix_i64 = i64::try_from(candidate_unix)
            .context("network_traffic_import_invalid:sample_timestamp_out_of_range")?;
        let raw_cutoff_unix_i64 = i64::try_from(raw_cutoff_unix)
            .context("network_traffic_import_invalid:raw_cutoff_out_of_range")?;
        let existing = sqlx::query_as::<_, (i64, bool)>(POSTGRES_IMPORT_RAW_PREDECESSOR_SQL)
            .bind(client_id)
            .bind(&prepared.interface)
            .bind(raw_cutoff_unix_i64)
            .bind(false)
            .fetch_optional(&mut **tx)
            .await?;
        let retain_import_predecessor = existing
            .as_ref()
            .is_none_or(|(observed_unix, _)| *observed_unix < candidate_unix_i64);
        if retain_import_predecessor {
            minimum_unix = candidate_unix;
            if let Some((observed_unix, true)) = existing {
                delete_inbound_predecessor_unix = Some(observed_unix);
            }
        }
    }

    if prepared.end_unix <= raw_cutoff_unix {
        if minimum_unix >= prepared.end_unix {
            return Ok(PostgresImportRawPlan {
                minimum_unix,
                rx_counter_epoch: prepared.initial_rx_counter_epoch,
                tx_counter_epoch: prepared.initial_tx_counter_epoch,
                delete_inbound_predecessor_unix,
                successor_adjustment: None,
            });
        }
        let end_unix = i64::try_from(prepared.end_unix)
            .context("network_traffic_import_invalid:sample_timestamp_out_of_range")?;
        let successor_epochs = sqlx::query_as::<_, (i64, i64)>(POSTGRES_IMPORT_RAW_SUCCESSOR_SQL)
            .bind(client_id)
            .bind(&prepared.interface)
            .bind(end_unix)
            .bind(false)
            .fetch_optional(&mut **tx)
            .await?;
        let (rx_counter_epoch, tx_counter_epoch) = successor_epochs.map_or_else(
            || {
                Ok::<_, anyhow::Error>((
                    prepared
                        .initial_rx_counter_epoch
                        .checked_add(1)
                        .context("network_traffic_import_rx_epoch_overflow")?,
                    prepared
                        .initial_tx_counter_epoch
                        .checked_add(1)
                        .context("network_traffic_import_tx_epoch_overflow")?,
                ))
            },
            |(successor_rx_epoch, successor_tx_epoch)| {
                anyhow::ensure!(
                    successor_rx_epoch >= 0 && successor_tx_epoch >= 0,
                    "network_traffic_import_successor_epoch_negative"
                );
                Ok((
                    successor_rx_epoch
                        .checked_add(1)
                        .context("network_traffic_import_rx_epoch_overflow")?,
                    successor_tx_epoch
                        .checked_add(1)
                        .context("network_traffic_import_tx_epoch_overflow")?,
                ))
            },
        )?;
        return Ok(PostgresImportRawPlan {
            minimum_unix,
            rx_counter_epoch,
            tx_counter_epoch,
            delete_inbound_predecessor_unix,
            successor_adjustment: None,
        });
    }

    let successor_unix = i64::try_from(prepared.end_unix)
        .context("network_traffic_import_invalid:sample_timestamp_out_of_range")?;
    let (successor_rx_epoch, successor_tx_epoch) = sqlx::query_as::<_, (i64, i64)>(
        r#"
        SELECT rx_counter_epoch, tx_counter_epoch
        FROM traffic_counter_samples
        WHERE client_id = $1
          AND source_kind = 'host'
          AND interface = $2
          AND sample_source NOT LIKE 'vnstat_import:%'
          AND observed_at = to_timestamp($3::double precision)
        FOR UPDATE
        "#,
    )
    .bind(client_id)
    .bind(&prepared.interface)
    .bind(successor_unix)
    .fetch_optional(&mut **tx)
    .await?
    .context("network_traffic_import_live_successor_missing")?;
    anyhow::ensure!(
        successor_rx_epoch >= 0 && successor_tx_epoch >= 0,
        "network_traffic_import_successor_epoch_negative"
    );
    let desired_rx_epoch = prepared
        .initial_rx_counter_epoch
        .checked_add(1)
        .context("network_traffic_import_rx_epoch_overflow")?;
    let desired_tx_epoch = prepared
        .initial_tx_counter_epoch
        .checked_add(1)
        .context("network_traffic_import_tx_epoch_overflow")?;
    anyhow::ensure!(
        successor_rx_epoch <= desired_rx_epoch && successor_tx_epoch <= desired_tx_epoch,
        "network_traffic_import_successor_epoch_exceeds_expected_transition"
    );
    let rx_delta = desired_rx_epoch - successor_rx_epoch;
    let tx_delta = desired_tx_epoch - successor_tx_epoch;
    if rx_delta != 0 || tx_delta != 0 {
        let (max_rx_epoch, max_tx_epoch): (i64, i64) = sqlx::query_as(
            r#"
            SELECT
                coalesce(max(rx_counter_epoch), 0)::bigint,
                coalesce(max(tx_counter_epoch), 0)::bigint
            FROM traffic_counter_samples
            WHERE client_id = $1
              AND source_kind = 'host'
              AND interface = $2
              AND sample_source NOT LIKE 'vnstat_import:%'
              AND observed_at >= to_timestamp($3::double precision)
            "#,
        )
        .bind(client_id)
        .bind(&prepared.interface)
        .bind(successor_unix)
        .fetch_one(&mut **tx)
        .await?;
        max_rx_epoch
            .checked_add(rx_delta)
            .context("network_traffic_import_rx_epoch_overflow")?;
        max_tx_epoch
            .checked_add(tx_delta)
            .context("network_traffic_import_tx_epoch_overflow")?;
    }
    Ok(PostgresImportRawPlan {
        minimum_unix,
        rx_counter_epoch: prepared.initial_rx_counter_epoch,
        tx_counter_epoch: prepared.initial_tx_counter_epoch,
        delete_inbound_predecessor_unix,
        successor_adjustment: Some(PostgresImportEpochAdjustment {
            successor_unix,
            rx_delta,
            tx_delta,
        }),
    })
}

async fn adjust_postgres_import_successor_epochs(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    client_id: &str,
    interface: &str,
    adjustment: Option<PostgresImportEpochAdjustment>,
) -> Result<()> {
    let Some(adjustment) = adjustment else {
        return Ok(());
    };
    if adjustment.rx_delta == 0 && adjustment.tx_delta == 0 {
        return Ok(());
    }
    sqlx::query(
        r#"
        UPDATE traffic_counter_samples
        SET
            rx_counter_epoch = rx_counter_epoch + $3,
            tx_counter_epoch = tx_counter_epoch + $4
        WHERE client_id = $1
          AND source_kind = 'host'
          AND interface = $2
          AND sample_source NOT LIKE 'vnstat_import:%'
          AND observed_at >= to_timestamp($5::double precision)
        "#,
    )
    .bind(client_id)
    .bind(interface)
    .bind(adjustment.rx_delta)
    .bind(adjustment.tx_delta)
    .bind(adjustment.successor_unix)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn import_summary(prepared: &[PreparedInterfaceImport]) -> NetworkTrafficImportSummary {
    import_summary_refs(&prepared.iter().collect::<Vec<_>>())
}

fn import_summary_refs(prepared: &[&PreparedInterfaceImport]) -> NetworkTrafficImportSummary {
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
