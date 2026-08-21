use anyhow::Result;
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::sync::Mutex;
use tracing::warn;
use vpsman_common::DEFAULT_TELEMETRY_RETENTION_PRUNE_LIMIT;

const RAW_RETENTION_DAYS: i32 = 32;
const DEFAULT_FINAL_RETENTION_DAYS: i32 = 3_650;
const CLIENT_BATCH: i64 = 16;
const GROUP_BATCH: i64 = 128;
const PROMOTION_SOURCE_ROW_LIMIT: i64 = 20_000;
const MAX_RAW_UNIT_SOURCE_ROWS: i64 = 86_400 / 60 + 1;
const BUDGETED_RAW_STREAM_BATCH: i64 = PROMOTION_SOURCE_ROW_LIMIT / MAX_RAW_UNIT_SOURCE_ROWS;
// The cursor must not advance past a registry stream that the client or global
// source-row budget can filter. Daily raw units are the maximum-cost case.
const CANDIDATE_STREAM_SCAN_LIMIT: i64 = if BUDGETED_RAW_STREAM_BATCH < CLIENT_BATCH {
    BUDGETED_RAW_STREAM_BATCH
} else {
    CLIENT_BATCH
};
// One retained boundary plus a full group batch. GROUP BY is only allowed
// after this fixed per-stream index prefix has been materialized.
const CANDIDATE_RAW_PREFIX_LIMIT: i64 = GROUP_BATCH + 1;

static CANDIDATE_STREAM_CURSOR: Mutex<Option<TrafficStreamKey>> = Mutex::new(None);

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TrafficRetentionRun {
    pub(crate) raw_rows_promoted: u64,
    pub(crate) rollup_rows_promoted: u64,
    pub(crate) rollup_rows_pruned: u64,
    pub(crate) conflicts: u64,
}

#[derive(Clone, Copy)]
struct Tier {
    source_secs: &'static [i32],
    destination_secs: i32,
    source_retention_days: i32,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct TrafficStreamKey {
    client_id: String,
    source_kind: String,
    interface: String,
}

#[derive(Clone, Debug)]
struct TrafficCandidateStream {
    key: TrafficStreamKey,
    oldest: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug)]
struct TrafficCandidateClient {
    client_id: String,
    streams: Vec<TrafficStreamKey>,
}

const TIERS: [Tier; 3] = [
    Tier {
        source_secs: &[3_600, 10_800, 21_600],
        destination_secs: 86_400,
        source_retention_days: 366,
    },
    Tier {
        source_secs: &[3_600, 10_800],
        destination_secs: 21_600,
        source_retention_days: 181,
    },
    Tier {
        source_secs: &[3_600],
        destination_secs: 10_800,
        source_retention_days: 91,
    },
];

/// Materializes old counter transitions before deleting exact endpoints, then
/// promotes them through the fixed LTS tiers. The caller must run this under
/// the telemetry-history worker lease; per-client advisory locks serialize it
/// with live ingest and vnStat replacement.
pub(crate) async fn process_traffic_retention(pool: &PgPool) -> Result<TrafficRetentionRun> {
    let policy = sqlx::query(
        r#"
        SELECT enabled, retention_days, prune_limit
        FROM history_retention_policies
        WHERE domain = 'traffic_counter_samples'
        "#,
    )
    .fetch_optional(pool)
    .await?;
    let pruning_enabled = policy
        .as_ref()
        .map(|row| row.try_get::<bool, _>("enabled"))
        .transpose()?
        .unwrap_or(true);
    let final_retention_days = policy
        .as_ref()
        .map(|row| row.try_get::<i32, _>("retention_days"))
        .transpose()?
        .unwrap_or(DEFAULT_FINAL_RETENTION_DAYS)
        .clamp(RAW_RETENTION_DAYS, DEFAULT_FINAL_RETENTION_DAYS);
    let prune_limit = policy
        .as_ref()
        .map(|row| row.try_get::<i32, _>("prune_limit"))
        .transpose()?
        .unwrap_or(DEFAULT_TELEMETRY_RETENTION_PRUNE_LIMIT)
        .clamp(1, 100_000);
    let stream_prefix = load_candidate_stream_prefix(pool).await?;
    let candidate_streams =
        load_candidate_streams(pool, &stream_prefix, final_retention_days, pruning_enabled).await?;
    let candidate_clients = candidate_clients(candidate_streams);
    let mut run = TrafficRetentionRun::default();
    // The operator policy bounds terminal deletion for the whole worker pass;
    // fixed raw and tier-promotion batches remain independent and lossless.
    let mut remaining_prune_budget = if pruning_enabled {
        u64::try_from(prune_limit)?
    } else {
        0
    };
    for candidate in candidate_clients {
        let client_prune_limit = i64::try_from(remaining_prune_budget)?;
        match process_client_traffic_retention(
            pool,
            &candidate.client_id,
            &candidate.streams,
            final_retention_days,
            client_prune_limit,
        )
        .await
        {
            Ok(client_run) => {
                remaining_prune_budget =
                    remaining_prune_budget.saturating_sub(client_run.rollup_rows_pruned);
                merge_run(&mut run, client_run);
            }
            Err(error) if is_statement_timeout(&error) => {
                warn!(client_id = candidate.client_id, %error, "traffic retention timed out for one client; continuing with the remaining candidates");
            }
            Err(error) => return Err(error),
        }
    }
    if run.conflicts > 0 {
        warn!(
            conflicts = run.conflicts,
            "traffic retention preserved source rows because destination tiers already existed"
        );
    }
    Ok(run)
}

async fn process_client_traffic_retention(
    pool: &PgPool,
    client_id: &str,
    streams: &[TrafficStreamKey],
    final_retention_days: i32,
    terminal_prune_limit: i64,
) -> Result<TrafficRetentionRun> {
    if streams.is_empty() {
        return Ok(TrafficRetentionRun::default());
    }
    let mut tx = pool.begin().await?;
    // Keep one slow or adversarial client from monopolizing the shared
    // history-retention lease. A timeout rolls this client back, and the outer
    // loop continues with the remaining oldest candidates.
    sqlx::query("SET LOCAL statement_timeout = '2s'")
        .execute(&mut *tx)
        .await?;
    if !try_lock_client_row_then_traffic(&mut tx, client_id).await? {
        tx.rollback().await?;
        return Ok(TrafficRetentionRun::default());
    }
    let source_kinds = streams
        .iter()
        .map(|stream| stream.source_kind.as_str())
        .collect::<Vec<_>>();
    let interfaces = streams
        .iter()
        .map(|stream| stream.interface.as_str())
        .collect::<Vec<_>>();
    let raw = sqlx::query(raw_promotion_sql())
        .bind(client_id)
        .bind(&source_kinds)
        .bind(&interfaces)
        .bind(RAW_RETENTION_DAYS)
        .bind(GROUP_BATCH)
        .bind(PROMOTION_SOURCE_ROW_LIMIT)
        .bind(CANDIDATE_RAW_PREFIX_LIMIT)
        .fetch_one(&mut *tx)
        .await?;
    let raw_insert_race_conflicts = raw.try_get::<i64, _>("insert_race_conflicts")?.max(0) as u64;
    let raw_conflicts = raw.try_get::<i64, _>("conflicts")?.max(0) as u64;
    if raw_insert_race_conflicts > 0 {
        // The client advisory makes this unreachable for supported writers,
        // but an out-of-band destination race must not leave a subset of the
        // origin aggregates committed beside still-raw source rows.
        tx.rollback().await?;
        return Ok(TrafficRetentionRun {
            conflicts: raw_conflicts,
            ..TrafficRetentionRun::default()
        });
    }
    let mut run = TrafficRetentionRun {
        raw_rows_promoted: raw.try_get::<i64, _>("deleted_rows")?.max(0) as u64,
        conflicts: raw_conflicts,
        ..TrafficRetentionRun::default()
    };

    for tier in TIERS {
        let source_bucket_secs = *tier
            .source_secs
            .last()
            .expect("traffic tier has an immediate predecessor");
        let promoted = sqlx::query(rollup_promotion_sql())
            .bind(client_id)
            .bind(&source_kinds)
            .bind(&interfaces)
            .bind(tier.source_secs.to_vec())
            .bind(tier.destination_secs)
            .bind(source_bucket_secs)
            .bind(tier.source_retention_days)
            .bind(GROUP_BATCH)
            .bind(PROMOTION_SOURCE_ROW_LIMIT)
            .fetch_one(&mut *tx)
            .await?;
        run.rollup_rows_promoted = run
            .rollup_rows_promoted
            .saturating_add(promoted.try_get::<i64, _>("deleted_rows")?.max(0) as u64);
        run.conflicts = run
            .conflicts
            .saturating_add(promoted.try_get::<i64, _>("conflicts")?.max(0) as u64);
    }
    if terminal_prune_limit > 0 {
        let stream_origin_tier_count = i64::try_from(streams.len())?
            .saturating_mul(2)
            .saturating_mul(4)
            .max(1);
        let per_source_prune_limit = terminal_prune_limit
            .saturating_add(stream_origin_tier_count - 1)
            / stream_origin_tier_count;
        run.rollup_rows_pruned = sqlx::query(rollup_prune_sql())
            .bind(client_id)
            .bind(&source_kinds)
            .bind(&interfaces)
            .bind(final_retention_days)
            .bind(terminal_prune_limit)
            .bind(per_source_prune_limit.max(1))
            .execute(&mut *tx)
            .await?
            .rows_affected();
    }
    tx.commit().await?;
    Ok(run)
}

fn is_statement_timeout(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(|error| match error {
            sqlx::Error::Database(database) => database.code(),
            _ => None,
        })
        .is_some_and(|code| code == "57014")
}

fn merge_run(total: &mut TrafficRetentionRun, current: TrafficRetentionRun) {
    total.raw_rows_promoted = total
        .raw_rows_promoted
        .saturating_add(current.raw_rows_promoted);
    total.rollup_rows_promoted = total
        .rollup_rows_promoted
        .saturating_add(current.rollup_rows_promoted);
    total.rollup_rows_pruned = total
        .rollup_rows_pruned
        .saturating_add(current.rollup_rows_pruned);
    total.conflicts = total.conflicts.saturating_add(current.conflicts);
}

async fn load_candidate_stream_prefix(pool: &PgPool) -> Result<Vec<TrafficStreamKey>> {
    let cursor = candidate_stream_cursor();
    let mut streams = fetch_candidate_stream_prefix(pool, cursor.as_ref()).await?;
    if streams.is_empty() && cursor.is_some() {
        streams = fetch_candidate_stream_prefix(pool, None).await?;
    }
    // This cursor only schedules which fixed registry prefix is inspected next.
    // Restarting from the beginning can repeat work but cannot affect retention
    // correctness, and advancement never depends on eligibility or lock success.
    if let Some(last) = streams.last() {
        set_candidate_stream_cursor(Some(last.clone()));
    } else {
        set_candidate_stream_cursor(None);
    }
    Ok(streams)
}

async fn fetch_candidate_stream_prefix(
    pool: &PgPool,
    cursor: Option<&TrafficStreamKey>,
) -> Result<Vec<TrafficStreamKey>> {
    // Keep the first and subsequent registry pages as separate prepared SQL
    // shapes. A nullable `cursor IS NULL OR key > cursor` predicate prevents a
    // generic plan from using the latter as a btree scan key and can make later
    // pages rescan and filter the complete registry prefix.
    let rows = if let Some(cursor) = cursor {
        sqlx::query(candidate_stream_prefix_after_sql())
            .bind(cursor.client_id.as_str())
            .bind(cursor.source_kind.as_str())
            .bind(cursor.interface.as_str())
            .bind(CANDIDATE_STREAM_SCAN_LIMIT)
            .fetch_all(pool)
            .await?
    } else {
        sqlx::query(candidate_stream_prefix_start_sql())
            .bind(CANDIDATE_STREAM_SCAN_LIMIT)
            .fetch_all(pool)
            .await?
    };
    rows.into_iter()
        .map(|row| {
            Ok(TrafficStreamKey {
                client_id: row.try_get("client_id")?,
                source_kind: row.try_get("source_kind")?,
                interface: row.try_get("interface")?,
            })
        })
        .collect()
}

fn candidate_stream_cursor() -> Option<TrafficStreamKey> {
    CANDIDATE_STREAM_CURSOR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

fn set_candidate_stream_cursor(cursor: Option<TrafficStreamKey>) {
    *CANDIDATE_STREAM_CURSOR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = cursor;
}

#[cfg(test)]
pub(crate) fn reset_candidate_stream_cursor_for_pressure_proof() {
    set_candidate_stream_cursor(None);
}

#[cfg(test)]
pub(crate) fn candidate_stream_scan_limit_for_pressure_proof() -> i64 {
    CANDIDATE_STREAM_SCAN_LIMIT
}

async fn load_candidate_streams(
    pool: &PgPool,
    streams: &[TrafficStreamKey],
    final_retention_days: i32,
    pruning_enabled: bool,
) -> Result<Vec<TrafficCandidateStream>> {
    if streams.is_empty() {
        return Ok(Vec::new());
    }
    let client_ids = streams
        .iter()
        .map(|stream| stream.client_id.as_str())
        .collect::<Vec<_>>();
    let source_kinds = streams
        .iter()
        .map(|stream| stream.source_kind.as_str())
        .collect::<Vec<_>>();
    let interfaces = streams
        .iter()
        .map(|stream| stream.interface.as_str())
        .collect::<Vec<_>>();
    let rows = sqlx::query(traffic_candidate_streams_sql())
        .bind(&client_ids)
        .bind(&source_kinds)
        .bind(&interfaces)
        .bind(RAW_RETENTION_DAYS)
        .bind(CANDIDATE_RAW_PREFIX_LIMIT)
        .bind(final_retention_days)
        .bind(pruning_enabled)
        .fetch_all(pool)
        .await?;
    rows.into_iter()
        .map(|row| {
            Ok(TrafficCandidateStream {
                key: TrafficStreamKey {
                    client_id: row.try_get("client_id")?,
                    source_kind: row.try_get("source_kind")?,
                    interface: row.try_get("interface")?,
                },
                oldest: row.try_get("oldest")?,
            })
        })
        .collect()
}

fn candidate_clients(mut streams: Vec<TrafficCandidateStream>) -> Vec<TrafficCandidateClient> {
    streams.sort_by(|left, right| {
        left.oldest
            .cmp(&right.oldest)
            .then_with(|| left.key.cmp(&right.key))
    });
    let mut clients = Vec::<TrafficCandidateClient>::new();
    for candidate in streams {
        if let Some(client) = clients
            .iter_mut()
            .find(|client| client.client_id == candidate.key.client_id)
        {
            client.streams.push(candidate.key);
            continue;
        }
        if clients.len() >= CLIENT_BATCH as usize {
            continue;
        }
        clients.push(TrafficCandidateClient {
            client_id: candidate.key.client_id.clone(),
            streams: vec![candidate.key],
        });
    }
    clients
}

async fn try_lock_client_row_then_traffic(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
) -> Result<bool> {
    // Telemetry and vnStat replacement acquire the client row before this
    // advisory. Take the same FK-strength row lock first, without waiting, so a
    // retention INSERT can never create the inverse advisory -> client order.
    let client = sqlx::query_scalar::<_, String>(
        "SELECT id FROM clients WHERE id = $1 FOR KEY SHARE SKIP LOCKED",
    )
    .bind(client_id)
    .fetch_optional(&mut **tx)
    .await?;
    if client.is_none() {
        return Ok(false);
    }
    let key = format!("traffic-counters:{client_id}");
    Ok(
        sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(key)
            .fetch_one(&mut **tx)
            .await?,
    )
}

fn candidate_stream_prefix_start_sql() -> &'static str {
    r#"
        SELECT client_id, source_kind, interface
        FROM traffic_counter_hourly_usage_streams
        ORDER BY client_id, source_kind, interface
        LIMIT $1
    "#
}

fn candidate_stream_prefix_after_sql() -> &'static str {
    r#"
        SELECT client_id, source_kind, interface
        FROM traffic_counter_hourly_usage_streams
        WHERE (client_id, source_kind, interface) > ($1, $2, $3)
        ORDER BY client_id, source_kind, interface
        LIMIT $4
    "#
}

fn traffic_candidate_streams_sql() -> &'static str {
    r#"
        WITH requested AS MATERIALIZED (
            SELECT client_id, source_kind, interface
            FROM UNNEST($1::text[], $2::text[], $3::text[])
                AS stream(client_id, source_kind, interface)
        ), cutoffs AS MATERIALIZED (
            SELECT
                (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                    AS today,
                (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                    - make_interval(days => $4) AS raw_cutoff
        ), raw_prefix AS MATERIALIZED (
            SELECT requested.client_id, requested.source_kind, requested.interface,
                   prefix.observed_at, prefix.inbound_promoted
            FROM requested
            CROSS JOIN cutoffs
            JOIN LATERAL (
                WITH seek AS MATERIALIZED (
                    SELECT sample.client_id, sample.source_kind,
                           sample.interface, sample.observed_at,
                           sample.inbound_promoted
                    FROM traffic_counter_samples sample
                    WHERE (sample.client_id, sample.source_kind,
                           sample.interface, sample.observed_at) >= (
                            requested.client_id, requested.source_kind,
                            requested.interface, '-infinity'::timestamptz
                    )
                    ORDER BY sample.client_id, sample.source_kind,
                             sample.interface, sample.observed_at
                    LIMIT $5
                )
                SELECT seek.observed_at, seek.inbound_promoted
                FROM seek
                WHERE seek.client_id = requested.client_id
                  AND seek.source_kind = requested.source_kind
                  AND seek.interface = requested.interface
                  AND seek.observed_at < cutoffs.raw_cutoff
            ) prefix ON TRUE
        ), raw_work AS (
            SELECT client_id, source_kind, interface,
                   min(observed_at) AS oldest
            FROM raw_prefix
            WHERE NOT inbound_promoted
            GROUP BY client_id, source_kind, interface
        ), promotion_specs(source_secs, destination_secs, retention_days) AS (
            VALUES (3600, 10800, 91),
                   (10800, 21600, 181),
                   (21600, 86400, 366)
        ), origins(origin_kind) AS (
            VALUES ('live'::text), ('vnstat_import'::text)
        ), rollup_work AS (
            SELECT requested.client_id, requested.source_kind, requested.interface,
                   min(candidate.bucket_start) AS oldest
            FROM requested
            CROSS JOIN cutoffs
            CROSS JOIN promotion_specs spec
            CROSS JOIN origins
            JOIN LATERAL (
                WITH seek AS MATERIALIZED (
                    SELECT rollup.client_id, rollup.source_kind,
                           rollup.interface, rollup.origin_kind,
                           rollup.bucket_secs, rollup.bucket_start
                    FROM traffic_counter_rollups rollup
                    WHERE (rollup.client_id, rollup.source_kind,
                           rollup.interface, rollup.origin_kind,
                           rollup.bucket_secs, rollup.bucket_start) >= (
                            requested.client_id, requested.source_kind,
                            requested.interface, origins.origin_kind,
                            spec.source_secs, '-infinity'::timestamptz
                    )
                    ORDER BY rollup.client_id, rollup.source_kind,
                             rollup.interface, rollup.origin_kind,
                             rollup.bucket_secs, rollup.bucket_start
                    LIMIT 1
                )
                SELECT seek.bucket_start
                FROM seek
                WHERE seek.client_id = requested.client_id
                  AND seek.source_kind = requested.source_kind
                  AND seek.interface = requested.interface
                  AND seek.origin_kind = origins.origin_kind
                  AND seek.bucket_secs = spec.source_secs
                  AND seek.bucket_start <= cutoffs.today
                        - make_interval(days => spec.retention_days)
                        - make_interval(secs => spec.source_secs)
            ) candidate ON TRUE
            GROUP BY requested.client_id, requested.source_kind, requested.interface
        ), prune_work AS (
            SELECT requested.client_id, requested.source_kind, requested.interface,
                   min(candidate.bucket_start) AS oldest
            FROM requested
            CROSS JOIN cutoffs
            CROSS JOIN origins
            CROSS JOIN (VALUES (3600), (10800), (21600), (86400)) tier(bucket_secs)
            JOIN LATERAL (
                WITH seek AS MATERIALIZED (
                    SELECT rollup.client_id, rollup.source_kind,
                           rollup.interface, rollup.origin_kind,
                           rollup.bucket_secs, rollup.bucket_start
                    FROM traffic_counter_rollups rollup
                    WHERE (rollup.client_id, rollup.source_kind,
                           rollup.interface, rollup.origin_kind,
                           rollup.bucket_secs, rollup.bucket_start) >= (
                            requested.client_id, requested.source_kind,
                            requested.interface, origins.origin_kind,
                            tier.bucket_secs, '-infinity'::timestamptz
                    )
                    ORDER BY rollup.client_id, rollup.source_kind,
                             rollup.interface, rollup.origin_kind,
                             rollup.bucket_secs, rollup.bucket_start
                    LIMIT 1
                )
                SELECT seek.bucket_start
                FROM seek
                WHERE $7
                  AND seek.client_id = requested.client_id
                  AND seek.source_kind = requested.source_kind
                  AND seek.interface = requested.interface
                  AND seek.origin_kind = origins.origin_kind
                  AND seek.bucket_secs = tier.bucket_secs
                  AND seek.bucket_start <= cutoffs.today
                        - make_interval(days => $6)
                        - make_interval(secs => tier.bucket_secs)
            ) candidate ON TRUE
            GROUP BY requested.client_id, requested.source_kind, requested.interface
        ), work AS (
            SELECT * FROM raw_work
            UNION ALL SELECT * FROM rollup_work
            UNION ALL SELECT * FROM prune_work
        )
        SELECT client_id, source_kind, interface, min(oldest) AS oldest
        FROM work
        GROUP BY client_id, source_kind, interface
        ORDER BY min(oldest), client_id, source_kind, interface
    "#
}

fn raw_promotion_sql() -> &'static str {
    r#"
        WITH requested AS MATERIALIZED (
            SELECT source_kind, interface
            FROM UNNEST($2::text[], $3::text[])
                AS stream(source_kind, interface)
        ), cutoff AS MATERIALIZED (
            SELECT
                (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                    AS today,
                (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                    - make_interval(days => $4) AS raw_cutoff
        ), seed_prefix AS MATERIALIZED (
            SELECT requested.source_kind, requested.interface,
                   seed.observed_at, seed.sample_source, seed.inbound_promoted
            FROM requested
            CROSS JOIN cutoff
            JOIN LATERAL (
                WITH seek AS MATERIALIZED (
                    SELECT sample.client_id, sample.source_kind,
                           sample.interface, sample.observed_at,
                           sample.sample_source, sample.inbound_promoted
                    FROM traffic_counter_samples sample
                    WHERE (sample.client_id, sample.source_kind,
                           sample.interface, sample.observed_at) >= (
                            $1, requested.source_kind, requested.interface,
                            '-infinity'::timestamptz
                    )
                    ORDER BY sample.client_id, sample.source_kind,
                             sample.interface, sample.observed_at
                    LIMIT $7
                )
                SELECT seek.observed_at, seek.sample_source,
                       seek.inbound_promoted
                FROM seek
                WHERE seek.client_id = $1
                  AND seek.source_kind = requested.source_kind
                  AND seek.interface = requested.interface
                  AND seek.observed_at < cutoff.raw_cutoff
            ) seed ON TRUE
        ), classified_prefix AS MATERIALIZED (
            SELECT source_kind, interface, observed_at,
                CASE
                    WHEN observed_at >= cutoff.today - interval '91 days' THEN 3600
                    WHEN observed_at >= cutoff.today - interval '181 days' THEN 10800
                    WHEN observed_at >= cutoff.today - interval '366 days' THEN 21600
                    ELSE 86400
                END::integer AS destination_secs,
                date_bin(
                    make_interval(secs => CASE
                        WHEN observed_at >= cutoff.today - interval '91 days' THEN 3600
                        WHEN observed_at >= cutoff.today - interval '181 days' THEN 10800
                        WHEN observed_at >= cutoff.today - interval '366 days' THEN 21600
                        ELSE 86400
                    END),
                    observed_at,
                    TIMESTAMPTZ '1970-01-01 00:00:00+00'
                ) AS bucket_start
            FROM seed_prefix
            CROSS JOIN cutoff
            WHERE NOT inbound_promoted
        ), earliest_units AS MATERIALIZED (
            SELECT DISTINCT ON (source_kind, interface)
                   source_kind, interface, observed_at,
                   destination_secs, bucket_start
            FROM classified_prefix
            ORDER BY source_kind, interface, observed_at
        ), unbudgeted_units AS MATERIALIZED (
            SELECT source_kind, interface, destination_secs, bucket_start
            FROM earliest_units
            ORDER BY bucket_start, source_kind, interface
            LIMIT $5
        ), costed_units AS MATERIALIZED (
            SELECT units.*,
                   units.destination_secs::bigint / 60 + 1 AS maximum_rows,
                   sum(units.destination_secs::bigint / 60 + 1) OVER (
                       ORDER BY units.bucket_start, units.source_kind,
                                units.interface
                   ) AS running_rows
            FROM unbudgeted_units units
        ), candidate_units AS MATERIALIZED (
            SELECT *
            FROM costed_units
            WHERE running_rows <= $6
        ), expanded_range AS MATERIALIZED (
            SELECT units.source_kind, units.interface,
                   units.destination_secs, units.bucket_start,
                   units.maximum_rows,
                   source.source_ctid, source.observed_at,
                   source.rx_bytes, source.tx_bytes,
                   source.rx_counter_epoch, source.tx_counter_epoch,
                   source.sample_source, source.inbound_promoted,
                   CASE WHEN source.sample_source LIKE 'vnstat_import:%'
                        THEN 'vnstat_import' ELSE 'live' END AS row_origin_kind
            FROM candidate_units units
            JOIN LATERAL (
                WITH seek AS MATERIALIZED (
                    SELECT sample.ctid AS source_ctid, sample.client_id,
                           sample.source_kind, sample.interface,
                           sample.observed_at, sample.rx_bytes, sample.tx_bytes,
                           sample.rx_counter_epoch, sample.tx_counter_epoch,
                           sample.sample_source, sample.inbound_promoted
                    FROM traffic_counter_samples sample
                    WHERE (sample.client_id, sample.source_kind,
                           sample.interface, sample.observed_at) >= (
                            $1, units.source_kind, units.interface,
                            units.bucket_start
                    )
                    ORDER BY sample.client_id, sample.source_kind,
                             sample.interface, sample.observed_at
                    LIMIT units.maximum_rows
                )
                SELECT seek.source_ctid, seek.observed_at,
                       seek.rx_bytes, seek.tx_bytes,
                       seek.rx_counter_epoch, seek.tx_counter_epoch,
                       seek.sample_source, seek.inbound_promoted
                FROM seek
                WHERE seek.client_id = $1
                  AND seek.source_kind = units.source_kind
                  AND seek.interface = units.interface
                  AND seek.observed_at < units.bucket_start
                        + make_interval(secs => units.destination_secs)
            ) source ON TRUE
        ), predecessors AS MATERIALIZED (
            SELECT units.source_kind, units.interface,
                   units.destination_secs, units.bucket_start,
                   units.maximum_rows,
                   predecessor.source_ctid, predecessor.observed_at,
                   predecessor.rx_bytes, predecessor.tx_bytes,
                   predecessor.rx_counter_epoch, predecessor.tx_counter_epoch,
                   predecessor.sample_source, predecessor.inbound_promoted,
                   CASE WHEN predecessor.sample_source LIKE 'vnstat_import:%'
                        THEN 'vnstat_import' ELSE 'live' END AS row_origin_kind
            FROM candidate_units units
            JOIN LATERAL (
                WITH seek AS MATERIALIZED (
                    SELECT sample.ctid AS source_ctid, sample.client_id,
                           sample.source_kind, sample.interface,
                           sample.observed_at, sample.rx_bytes, sample.tx_bytes,
                           sample.rx_counter_epoch, sample.tx_counter_epoch,
                           sample.sample_source, sample.inbound_promoted
                    FROM traffic_counter_samples sample
                    WHERE (sample.client_id, sample.source_kind,
                           sample.interface, sample.observed_at) < (
                            $1, units.source_kind, units.interface,
                            units.bucket_start
                    )
                    ORDER BY sample.client_id DESC, sample.source_kind DESC,
                             sample.interface DESC, sample.observed_at DESC
                    LIMIT 1
                )
                SELECT seek.source_ctid, seek.observed_at,
                       seek.rx_bytes, seek.tx_bytes,
                       seek.rx_counter_epoch, seek.tx_counter_epoch,
                       seek.sample_source, seek.inbound_promoted
                FROM seek
                WHERE seek.client_id = $1
                  AND seek.source_kind = units.source_kind
                  AND seek.interface = units.interface
                  AND seek.observed_at < units.bucket_start
            ) predecessor ON TRUE
        ), sequencing_rows AS MATERIALIZED (
            SELECT expanded_range.*, TRUE AS in_range
            FROM expanded_range
            UNION ALL
            SELECT predecessors.*, FALSE AS in_range
            FROM predecessors
        ), sequenced AS MATERIALIZED (
            SELECT sequencing_rows.*,
                lag(rx_bytes) OVER stream AS previous_rx_bytes,
                lag(tx_bytes) OVER stream AS previous_tx_bytes,
                lag(rx_counter_epoch) OVER stream AS previous_rx_counter_epoch,
                lag(tx_counter_epoch) OVER stream AS previous_tx_counter_epoch,
                lag(sample_source) OVER stream AS previous_sample_source
            FROM sequencing_rows
            WINDOW stream AS (
                PARTITION BY source_kind, interface,
                             destination_secs, bucket_start
                ORDER BY observed_at
            )
        ), unit_state AS MATERIALIZED (
            SELECT units.source_kind, units.interface,
                   units.destination_secs, units.bucket_start,
                   units.maximum_rows,
                   count(sequencing_rows.source_ctid) FILTER (
                       WHERE sequencing_rows.in_range
                   )::bigint AS range_rows,
                   count(sequencing_rows.source_ctid) FILTER (
                       WHERE sequencing_rows.in_range
                         AND NOT sequencing_rows.inbound_promoted
                   )::bigint AS expected_rows,
                   count(sequencing_rows.source_ctid) FILTER (
                       WHERE sequencing_rows.inbound_promoted
                   )::bigint AS boundary_rows
            FROM candidate_units units
            LEFT JOIN sequencing_rows USING (
                source_kind, interface,
                destination_secs, bucket_start, maximum_rows
            )
            GROUP BY units.source_kind, units.interface,
                     units.destination_secs, units.bucket_start,
                     units.maximum_rows
        ), eligible_units AS MATERIALIZED (
            SELECT *
            FROM unit_state
            WHERE range_rows < maximum_rows
              AND expected_rows > 0
              AND boundary_rows <= 1
        ), candidate_rows AS MATERIALIZED (
            SELECT sequenced.*
            FROM sequenced
            JOIN eligible_units units USING (
                source_kind, interface,
                destination_secs, bucket_start, maximum_rows
            )
            WHERE sequenced.in_range
              AND NOT sequenced.inbound_promoted
        ), origin_groups AS MATERIALIZED (
            SELECT source_kind, interface, destination_secs, bucket_start,
                   row_origin_kind AS origin_kind, count(*)::bigint AS origin_rows
            FROM candidate_rows
            GROUP BY source_kind, interface, destination_secs, bucket_start,
                     row_origin_kind
        ), destination_conflicts AS MATERIALIZED (
            SELECT DISTINCT groups.source_kind, groups.interface,
                   groups.destination_secs, groups.bucket_start
            FROM origin_groups groups
            WHERE EXISTS (
                WITH seek AS MATERIALIZED (
                    SELECT destination.client_id, destination.source_kind,
                           destination.interface, destination.origin_kind,
                           destination.bucket_secs, destination.bucket_start
                    FROM traffic_counter_rollups destination
                    WHERE (destination.client_id, destination.source_kind,
                           destination.interface, destination.origin_kind,
                           destination.bucket_secs,
                           destination.bucket_start) >= (
                            $1, groups.source_kind, groups.interface,
                            groups.origin_kind, groups.destination_secs,
                            groups.bucket_start
                    )
                    ORDER BY destination.client_id, destination.source_kind,
                             destination.interface, destination.origin_kind,
                             destination.bucket_secs, destination.bucket_start
                    LIMIT 1
                )
                SELECT 1
                FROM seek
                WHERE seek.client_id = $1
                  AND seek.source_kind = groups.source_kind
                  AND seek.interface = groups.interface
                  AND seek.origin_kind = groups.origin_kind
                  AND seek.bucket_secs = groups.destination_secs
                  AND seek.bucket_start = groups.bucket_start
            )
        ), lockable_units AS MATERIALIZED (
            SELECT units.*
            FROM eligible_units units
            WHERE NOT EXISTS (
                SELECT 1
                FROM destination_conflicts conflict
                WHERE conflict.source_kind = units.source_kind
                  AND conflict.interface = units.interface
                  AND conflict.destination_secs = units.destination_secs
                  AND conflict.bucket_start = units.bucket_start
            )
        ), boundary_targets AS MATERIALIZED (
            SELECT DISTINCT ON (
                       sequencing.source_kind, sequencing.interface,
                       sequencing.destination_secs, sequencing.bucket_start
                   )
                   sequencing.source_kind, sequencing.interface,
                   sequencing.destination_secs, sequencing.bucket_start,
                   sequencing.source_ctid
            FROM sequencing_rows sequencing
            JOIN lockable_units USING (
                source_kind, interface, destination_secs,
                bucket_start, maximum_rows
            )
            WHERE sequencing.inbound_promoted
            ORDER BY sequencing.source_kind, sequencing.interface,
                     sequencing.destination_secs, sequencing.bucket_start,
                     sequencing.observed_at DESC
        ), lock_targets AS MATERIALIZED (
            SELECT rows.source_kind, rows.interface,
                   rows.destination_secs, rows.bucket_start,
                   rows.source_ctid,
                   FALSE AS is_boundary
            FROM candidate_rows rows
            JOIN lockable_units USING (
                source_kind, interface, destination_secs,
                bucket_start, maximum_rows
            )
            UNION ALL
            SELECT source_kind, interface,
                   destination_secs, bucket_start, source_ctid,
                   TRUE AS is_boundary
            FROM boundary_targets
        ), locked_targets AS MATERIALIZED (
            SELECT targets.*
            FROM lock_targets targets
            JOIN traffic_counter_samples source
              ON source.ctid = targets.source_ctid
            ORDER BY targets.bucket_start, targets.source_kind,
                     targets.interface, targets.source_ctid
            FOR UPDATE OF source SKIP LOCKED
        ), complete_units AS MATERIALIZED (
            SELECT units.source_kind, units.interface,
                   units.destination_secs, units.bucket_start
            FROM lockable_units units
            LEFT JOIN locked_targets targets USING (
                source_kind, interface,
                destination_secs, bucket_start
            )
            GROUP BY units.source_kind, units.interface,
                     units.destination_secs, units.bucket_start,
                     units.expected_rows
            HAVING count(targets.source_ctid) FILTER (
                       WHERE NOT targets.is_boundary
                   ) = units.expected_rows
               AND count(targets.source_ctid) FILTER (
                       WHERE targets.is_boundary
                   ) = (
                       SELECT count(*)
                       FROM boundary_targets boundary
                       WHERE boundary.source_kind = units.source_kind
                         AND boundary.interface = units.interface
                         AND boundary.destination_secs = units.destination_secs
                         AND boundary.bucket_start = units.bucket_start
                   )
        ), locked AS MATERIALIZED (
            SELECT candidate_rows.*
            FROM candidate_rows
            JOIN locked_targets targets
             ON targets.source_ctid = candidate_rows.source_ctid
             AND targets.source_kind = candidate_rows.source_kind
             AND targets.interface = candidate_rows.interface
             AND targets.destination_secs = candidate_rows.destination_secs
             AND targets.bucket_start = candidate_rows.bucket_start
             AND NOT targets.is_boundary
        ), aggregated AS MATERIALIZED (
            SELECT
                $1::text AS client_id,
                locked.source_kind,
                locked.interface,
                locked.row_origin_kind AS origin_kind,
                locked.destination_secs AS bucket_secs,
                locked.bucket_start,
                COALESCE(sum(CASE
                    WHEN rx_counter_epoch = previous_rx_counter_epoch
                     AND rx_bytes >= previous_rx_bytes
                    THEN rx_bytes - previous_rx_bytes ELSE 0 END), 0)::bigint AS rx_bytes,
                COALESCE(sum(CASE
                    WHEN tx_counter_epoch = previous_tx_counter_epoch
                     AND tx_bytes >= previous_tx_bytes
                    THEN tx_bytes - previous_tx_bytes ELSE 0 END), 0)::bigint AS tx_bytes,
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
            FROM locked
            JOIN complete_units USING (
                source_kind, interface,
                destination_secs, bucket_start
            )
            GROUP BY locked.source_kind, locked.interface,
                     locked.row_origin_kind, locked.destination_secs,
                     locked.bucket_start
        ), inserted AS (
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
            FROM aggregated
            ON CONFLICT DO NOTHING
            RETURNING source_kind, interface, origin_kind,
                      bucket_secs, bucket_start
        ), insert_state AS MATERIALIZED (
            SELECT aggregated.source_kind, aggregated.interface,
                   aggregated.bucket_secs AS destination_secs,
                   aggregated.bucket_start,
                   count(*)::bigint AS expected_origins,
                   count(inserted.origin_kind)::bigint AS inserted_origins
            FROM aggregated
            LEFT JOIN inserted
              ON inserted.source_kind = aggregated.source_kind
             AND inserted.interface = aggregated.interface
             AND inserted.origin_kind = aggregated.origin_kind
             AND inserted.bucket_secs = aggregated.bucket_secs
             AND inserted.bucket_start = aggregated.bucket_start
            GROUP BY aggregated.source_kind, aggregated.interface,
                     aggregated.bucket_secs, aggregated.bucket_start
        ), successful_units AS MATERIALIZED (
            SELECT source_kind, interface, destination_secs, bucket_start
            FROM insert_state
            WHERE inserted_origins = expected_origins
        ), insert_race_conflicts AS MATERIALIZED (
            SELECT source_kind, interface, destination_secs, bucket_start
            FROM insert_state
            WHERE inserted_origins <> expected_origins
        ), promoted_rows AS MATERIALIZED (
            SELECT locked.*
            FROM locked
            JOIN successful_units USING (
                source_kind, interface, destination_secs, bucket_start
            )
        ), promoted_boundaries AS MATERIALIZED (
            SELECT DISTINCT ON (
                       source_kind, interface, destination_secs, bucket_start
                   )
                   source_kind, interface, destination_secs, bucket_start,
                   source_ctid
            FROM promoted_rows
            ORDER BY source_kind, interface, destination_secs, bucket_start,
                     observed_at DESC
        ), locked_prior_boundaries AS MATERIALIZED (
            SELECT targets.source_kind, targets.interface,
                   targets.destination_secs,
                   targets.bucket_start, targets.source_ctid
            FROM locked_targets targets
            JOIN successful_units USING (
                source_kind, interface,
                destination_secs, bucket_start
            )
            WHERE targets.is_boundary
        ), marked_boundary AS (
            UPDATE traffic_counter_samples source
            SET inbound_promoted = TRUE
            FROM promoted_boundaries boundary
            WHERE source.ctid = boundary.source_ctid
            RETURNING source.ctid
        ), deleted_new AS (
            DELETE FROM traffic_counter_samples source
            USING promoted_rows promoted, promoted_boundaries boundary
            WHERE source.ctid = promoted.source_ctid
              AND boundary.source_kind = promoted.source_kind
              AND boundary.interface = promoted.interface
              AND boundary.destination_secs = promoted.destination_secs
              AND boundary.bucket_start = promoted.bucket_start
              AND promoted.source_ctid <> boundary.source_ctid
            RETURNING source.ctid
        ), deleted_accounted AS (
            DELETE FROM traffic_counter_samples source
            USING locked_prior_boundaries boundary
            WHERE source.ctid = boundary.source_ctid
            RETURNING source.ctid
        ), overflow_conflicts AS MATERIALIZED (
            SELECT source_kind, interface,
                   destination_secs, bucket_start
            FROM unit_state
            WHERE range_rows >= maximum_rows OR boundary_rows > 1
        )
        SELECT
            ((SELECT count(*) FROM deleted_new)
                + (SELECT count(*) FROM deleted_accounted))::bigint AS deleted_rows,
            ((SELECT count(*) FROM overflow_conflicts)
                + (SELECT count(*) FROM destination_conflicts)
                + (SELECT count(*) FROM insert_race_conflicts))::bigint AS conflicts,
            (SELECT count(*) FROM insert_race_conflicts)::bigint
                AS insert_race_conflicts
    "#
}

fn rollup_promotion_sql() -> &'static str {
    r#"
        WITH requested AS MATERIALIZED (
            SELECT source_kind, interface
            FROM UNNEST($2::text[], $3::text[])
                AS stream(source_kind, interface)
        ), cutoff AS MATERIALIZED (
            SELECT (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                - make_interval(days => $7) AS value
        ), origins(origin_kind) AS (
            VALUES ('live'::text), ('vnstat_import'::text)
        ), immediate_predecessor_seeds AS MATERIALIZED (
            SELECT requested.source_kind, requested.interface,
                   origins.origin_kind, seed.bucket_start
            FROM requested
            CROSS JOIN origins
            CROSS JOIN cutoff
            JOIN LATERAL (
                WITH seek AS MATERIALIZED (
                    SELECT source.client_id, source.source_kind,
                           source.interface, source.origin_kind,
                           source.bucket_secs, source.bucket_start
                    FROM traffic_counter_rollups source
                    WHERE (source.client_id, source.source_kind,
                           source.interface, source.origin_kind,
                           source.bucket_secs, source.bucket_start) >= (
                            $1, requested.source_kind, requested.interface,
                            origins.origin_kind, $6,
                            '-infinity'::timestamptz
                    )
                    ORDER BY source.client_id, source.source_kind,
                             source.interface, source.origin_kind,
                             source.bucket_secs, source.bucket_start
                    LIMIT 1
                )
                SELECT seek.bucket_start
                FROM seek
                WHERE seek.client_id = $1
                  AND seek.source_kind = requested.source_kind
                  AND seek.interface = requested.interface
                  AND seek.origin_kind = origins.origin_kind
                  AND seek.bucket_secs = $6
                  AND seek.bucket_start <= cutoff.value
                        - make_interval(secs => $6)
            ) seed ON TRUE
        ), unbudgeted_groups AS MATERIALIZED (
            SELECT source_kind, interface, origin_kind,
                   date_bin(
                       make_interval(secs => $5), bucket_start,
                       TIMESTAMPTZ '1970-01-01 00:00:00+00'
                   ) AS destination_start
            FROM immediate_predecessor_seeds
            ORDER BY destination_start, source_kind, interface, origin_kind
            LIMIT $8
        ), costed_groups AS MATERIALIZED (
            SELECT groups.*,
                   source_budget.maximum_rows,
                   sum(source_budget.maximum_rows) OVER (
                       ORDER BY groups.destination_start, groups.source_kind,
                                groups.interface, groups.origin_kind
                   ) AS running_rows
            FROM unbudgeted_groups groups
            CROSS JOIN LATERAL (
                SELECT (sum($5::bigint / source_secs) + 1)::bigint
                    AS maximum_rows
                FROM UNNEST($4::integer[]) source(source_secs)
            ) source_budget
        ), candidate_groups AS MATERIALIZED (
            SELECT *
            FROM costed_groups
            WHERE running_rows <= $9
        ), expanded_range AS MATERIALIZED (
            SELECT groups.source_kind, groups.interface, groups.origin_kind,
                   groups.destination_start, groups.maximum_rows,
                   source.source_ctid, source.bucket_secs,
                   source.bucket_start, source.rx_bytes, source.tx_bytes,
                   source.rx_valid_count, source.tx_valid_count,
                   source.any_valid_count, source.rx_reset_count,
                   source.tx_reset_count, source.any_reset_count,
                   source.first_observed_at, source.latest_observed_at
            FROM candidate_groups groups
            JOIN LATERAL (
                SELECT tier_source.*
                FROM UNNEST($4::integer[]) tier(bucket_secs)
                CROSS JOIN LATERAL (
                    WITH seek AS MATERIALIZED (
                        SELECT source.ctid AS source_ctid, source.client_id,
                               source.source_kind, source.interface,
                               source.origin_kind, source.bucket_secs,
                               source.bucket_start, source.rx_bytes,
                               source.tx_bytes, source.rx_valid_count,
                               source.tx_valid_count, source.any_valid_count,
                               source.rx_reset_count, source.tx_reset_count,
                               source.any_reset_count, source.first_observed_at,
                               source.latest_observed_at
                        FROM traffic_counter_rollups source
                        WHERE (source.client_id, source.source_kind,
                               source.interface, source.origin_kind,
                               source.bucket_secs, source.bucket_start) >= (
                                $1, groups.source_kind, groups.interface,
                                groups.origin_kind, tier.bucket_secs,
                                groups.destination_start
                        )
                        ORDER BY source.client_id, source.source_kind,
                                 source.interface, source.origin_kind,
                                 source.bucket_secs, source.bucket_start
                        LIMIT ($5 / tier.bucket_secs)
                    )
                    SELECT seek.source_ctid, seek.bucket_secs,
                           seek.bucket_start, seek.rx_bytes, seek.tx_bytes,
                           seek.rx_valid_count, seek.tx_valid_count,
                           seek.any_valid_count, seek.rx_reset_count,
                           seek.tx_reset_count, seek.any_reset_count,
                           seek.first_observed_at, seek.latest_observed_at
                    FROM seek
                    WHERE seek.client_id = $1
                      AND seek.source_kind = groups.source_kind
                      AND seek.interface = groups.interface
                      AND seek.origin_kind = groups.origin_kind
                      AND seek.bucket_secs = tier.bucket_secs
                      AND seek.bucket_start <= groups.destination_start
                            + make_interval(secs => $5 - tier.bucket_secs)
                ) tier_source
                ORDER BY tier_source.bucket_start, tier_source.bucket_secs
                LIMIT groups.maximum_rows
            ) source ON TRUE
        ), group_state AS MATERIALIZED (
            SELECT groups.source_kind, groups.interface, groups.origin_kind,
                   groups.destination_start, groups.maximum_rows,
                   count(expanded.source_ctid)::bigint AS expected_rows
            FROM candidate_groups groups
            LEFT JOIN expanded_range expanded USING (
                source_kind, interface, origin_kind,
                destination_start, maximum_rows
            )
            GROUP BY groups.source_kind, groups.interface, groups.origin_kind,
                     groups.destination_start, groups.maximum_rows
        ), overflow_groups AS MATERIALIZED (
            SELECT source_kind, interface, origin_kind, destination_start
            FROM group_state
            WHERE expected_rows >= maximum_rows
        ), bounded_groups AS MATERIALIZED (
            SELECT *
            FROM group_state
            WHERE expected_rows > 0
              AND expected_rows < maximum_rows
        ), destination_conflicts AS MATERIALIZED (
            SELECT groups.source_kind, groups.interface, groups.origin_kind,
                   groups.destination_start
            FROM bounded_groups groups
            WHERE EXISTS (
                WITH seek AS MATERIALIZED (
                    SELECT destination.client_id, destination.source_kind,
                           destination.interface, destination.origin_kind,
                           destination.bucket_secs, destination.bucket_start
                    FROM traffic_counter_rollups destination
                    WHERE (destination.client_id, destination.source_kind,
                           destination.interface, destination.origin_kind,
                           destination.bucket_secs,
                           destination.bucket_start) >= (
                            $1, groups.source_kind, groups.interface,
                            groups.origin_kind, $5,
                            groups.destination_start
                    )
                    ORDER BY destination.client_id, destination.source_kind,
                             destination.interface, destination.origin_kind,
                             destination.bucket_secs, destination.bucket_start
                    LIMIT 1
                )
                SELECT 1
                FROM seek
                WHERE seek.client_id = $1
                  AND seek.source_kind = groups.source_kind
                  AND seek.interface = groups.interface
                  AND seek.origin_kind = groups.origin_kind
                  AND seek.bucket_secs = $5
                  AND seek.bucket_start = groups.destination_start
            )
        ), lockable_groups AS MATERIALIZED (
            SELECT groups.*
            FROM bounded_groups groups
            WHERE NOT EXISTS (
                SELECT 1
                FROM destination_conflicts conflict
                WHERE conflict.source_kind = groups.source_kind
                  AND conflict.interface = groups.interface
                  AND conflict.origin_kind = groups.origin_kind
                  AND conflict.destination_start = groups.destination_start
            )
        ), candidate_rows AS MATERIALIZED (
            SELECT expanded.*, groups.expected_rows
            FROM expanded_range expanded
            JOIN lockable_groups groups USING (
                source_kind, interface, origin_kind,
                destination_start, maximum_rows
            )
        ), locked AS MATERIALIZED (
            SELECT candidate_rows.*
            FROM candidate_rows
            JOIN traffic_counter_rollups source
              ON source.ctid = candidate_rows.source_ctid
            ORDER BY candidate_rows.destination_start,
                     candidate_rows.source_kind, candidate_rows.interface,
                     candidate_rows.origin_kind, candidate_rows.bucket_start,
                     candidate_rows.bucket_secs
            FOR UPDATE OF source SKIP LOCKED
        ), complete_groups AS MATERIALIZED (
            SELECT source_kind, interface, origin_kind, destination_start
            FROM locked
            GROUP BY source_kind, interface, origin_kind,
                     destination_start, expected_rows
            HAVING count(*) = expected_rows
        ), ordered_locked AS MATERIALIZED (
            SELECT
                locked.*,
                lag(
                    locked.bucket_start
                        + make_interval(secs => locked.bucket_secs)
                ) OVER (
                    PARTITION BY locked.source_kind, locked.interface,
                                 locked.origin_kind, locked.destination_start
                    ORDER BY locked.bucket_start, locked.bucket_secs
                ) AS previous_end
            FROM locked
        ), aggregated AS MATERIALIZED (
            SELECT
                $1::text AS client_id,
                locked.source_kind,
                locked.interface,
                locked.origin_kind,
                $5::integer AS bucket_secs,
                locked.destination_start AS bucket_start,
                sum(locked.rx_bytes)::bigint AS rx_bytes,
                sum(locked.tx_bytes)::bigint AS tx_bytes,
                LEAST(sum(locked.rx_valid_count), 2147483647)::integer
                    AS rx_valid_count,
                LEAST(sum(locked.tx_valid_count), 2147483647)::integer
                    AS tx_valid_count,
                LEAST(sum(locked.any_valid_count), 2147483647)::integer
                    AS any_valid_count,
                LEAST(sum(locked.rx_reset_count), 2147483647)::integer
                    AS rx_reset_count,
                LEAST(sum(locked.tx_reset_count), 2147483647)::integer
                    AS tx_reset_count,
                LEAST(sum(locked.any_reset_count), 2147483647)::integer
                    AS any_reset_count,
                min(locked.first_observed_at) AS first_observed_at,
                max(locked.latest_observed_at) AS latest_observed_at
            FROM ordered_locked locked
            JOIN complete_groups USING (
                source_kind, interface, origin_kind, destination_start
            )
            GROUP BY locked.source_kind, locked.interface,
                     locked.origin_kind, locked.destination_start
            HAVING count(*) FILTER (
                    WHERE locked.previous_end > locked.bucket_start
               ) = 0
        ), inserted AS (
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
            FROM aggregated
            ON CONFLICT DO NOTHING
            RETURNING source_kind, interface, origin_kind,
                      bucket_secs, bucket_start
        ), deleted AS (
            DELETE FROM traffic_counter_rollups source
            USING locked, inserted
            WHERE source.ctid = locked.source_ctid
              AND inserted.source_kind = locked.source_kind
              AND inserted.interface = locked.interface
              AND inserted.origin_kind = locked.origin_kind
              AND inserted.bucket_secs = $5
              AND inserted.bucket_start = locked.destination_start
            RETURNING source.ctid
        ), overlap_conflicts AS MATERIALIZED (
            SELECT complete.source_kind, complete.interface,
                   complete.origin_kind, complete.destination_start
            FROM complete_groups complete
            WHERE NOT EXISTS (
                SELECT 1
                FROM aggregated
                WHERE aggregated.source_kind = complete.source_kind
                  AND aggregated.interface = complete.interface
                  AND aggregated.origin_kind = complete.origin_kind
                  AND aggregated.bucket_start = complete.destination_start
            )
        )
        SELECT
            (SELECT count(*) FROM deleted)::bigint AS deleted_rows,
            ((SELECT count(*) FROM overflow_groups)
                + (SELECT count(*) FROM destination_conflicts)
                + (SELECT count(*) FROM overlap_conflicts)
                + (SELECT count(*) FROM aggregated)
                - (SELECT count(*) FROM inserted))::bigint AS conflicts
    "#
}

fn rollup_prune_sql() -> &'static str {
    r#"
        WITH requested AS MATERIALIZED (
            SELECT source_kind, interface
            FROM UNNEST($2::text[], $3::text[])
                AS stream(source_kind, interface)
        ), cutoff AS MATERIALIZED (
            SELECT (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                - make_interval(days => $4) AS value
        ), origins(origin_kind) AS (
            VALUES ('live'::text), ('vnstat_import'::text)
        ), tiers(bucket_secs) AS (
            VALUES (3600), (10800), (21600), (86400)
        ), bounded_candidates AS MATERIALIZED (
            SELECT requested.source_kind, requested.interface,
                   origins.origin_kind, tiers.bucket_secs,
                   candidate.bucket_start, candidate.source_ctid
            FROM requested
            CROSS JOIN origins
            CROSS JOIN tiers
            CROSS JOIN cutoff
            JOIN LATERAL (
                WITH seek AS MATERIALIZED (
                    SELECT source.ctid AS source_ctid, source.client_id,
                           source.source_kind, source.interface,
                           source.origin_kind, source.bucket_secs,
                           source.bucket_start
                    FROM traffic_counter_rollups source
                    WHERE (source.client_id, source.source_kind,
                           source.interface, source.origin_kind,
                           source.bucket_secs, source.bucket_start) >= (
                            $1, requested.source_kind, requested.interface,
                            origins.origin_kind, tiers.bucket_secs,
                            '-infinity'::timestamptz
                    )
                    ORDER BY source.client_id, source.source_kind,
                             source.interface, source.origin_kind,
                             source.bucket_secs, source.bucket_start
                    LIMIT $6
                )
                SELECT seek.source_ctid, seek.bucket_start
                FROM seek
                WHERE seek.client_id = $1
                  AND seek.source_kind = requested.source_kind
                  AND seek.interface = requested.interface
                  AND seek.origin_kind = origins.origin_kind
                  AND seek.bucket_secs = tiers.bucket_secs
                  AND seek.bucket_start <= cutoff.value
                        - make_interval(secs => tiers.bucket_secs)
            ) candidate ON TRUE
        ), candidates AS MATERIALIZED (
            SELECT bounded.source_ctid
            FROM bounded_candidates bounded
            JOIN traffic_counter_rollups source
              ON source.ctid = bounded.source_ctid
            ORDER BY bounded.bucket_start, bounded.source_kind,
                     bounded.interface, bounded.origin_kind,
                     bounded.bucket_secs
            LIMIT $5
            FOR UPDATE OF source SKIP LOCKED
        )
        DELETE FROM traffic_counter_rollups source
        USING candidates
        WHERE source.ctid = candidates.source_ctid
    "#
}

#[cfg(test)]
#[path = "tests_traffic_retention.rs"]
mod tests;
