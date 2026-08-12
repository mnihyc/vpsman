use anyhow::Result;
use sqlx::{PgPool, Postgres, Row, Transaction};
use tracing::warn;
use vpsman_common::DEFAULT_TELEMETRY_RETENTION_PRUNE_LIMIT;

const RAW_RETENTION_DAYS: i32 = 32;
const DEFAULT_FINAL_RETENTION_DAYS: i32 = 3_650;
const CLIENT_BATCH: i64 = 16;
const GROUP_BATCH: i64 = 128;

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
    let client_ids = sqlx::query_scalar::<_, String>(traffic_candidate_clients_sql())
        .bind(RAW_RETENTION_DAYS)
        .bind(CLIENT_BATCH)
        .bind(final_retention_days)
        .bind(pruning_enabled)
        .fetch_all(pool)
        .await?;
    let mut run = TrafficRetentionRun::default();
    // The operator policy bounds terminal deletion for the whole worker pass;
    // fixed raw and tier-promotion batches remain independent and lossless.
    let mut remaining_prune_budget = if pruning_enabled {
        u64::try_from(prune_limit)?
    } else {
        0
    };
    for client_id in client_ids {
        let client_prune_limit = i64::try_from(remaining_prune_budget)?;
        match process_client_traffic_retention(
            pool,
            &client_id,
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
                warn!(client_id, %error, "traffic retention timed out for one client; continuing with the remaining candidates");
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
    final_retention_days: i32,
    terminal_prune_limit: i64,
) -> Result<TrafficRetentionRun> {
    let mut tx = pool.begin().await?;
    // Keep one slow or adversarial client from monopolizing the shared
    // history-retention lease. A timeout rolls this client back, and the outer
    // loop continues with the remaining oldest candidates.
    sqlx::query("SET LOCAL statement_timeout = '2s'")
        .execute(&mut *tx)
        .await?;
    if !try_lock_client(&mut tx, client_id).await? {
        tx.rollback().await?;
        return Ok(TrafficRetentionRun::default());
    }
    let raw = sqlx::query(raw_promotion_sql())
        .bind(client_id)
        .bind(RAW_RETENTION_DAYS)
        .bind(GROUP_BATCH)
        .fetch_one(&mut *tx)
        .await?;
    let mut run = TrafficRetentionRun {
        raw_rows_promoted: raw.try_get::<i64, _>("deleted_rows")?.max(0) as u64,
        conflicts: raw.try_get::<i64, _>("conflicts")?.max(0) as u64,
        ..TrafficRetentionRun::default()
    };

    for tier in TIERS {
        let promoted = sqlx::query(rollup_promotion_sql())
            .bind(client_id)
            .bind(tier.source_secs.to_vec())
            .bind(tier.destination_secs)
            .bind(tier.source_retention_days)
            .bind(GROUP_BATCH)
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
        run.rollup_rows_pruned = sqlx::query(rollup_prune_sql())
            .bind(client_id)
            .bind(final_retention_days)
            .bind(terminal_prune_limit)
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

async fn try_lock_client(tx: &mut Transaction<'_, Postgres>, client_id: &str) -> Result<bool> {
    let key = format!("traffic-counters:{client_id}");
    Ok(
        sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(key)
            .fetch_one(&mut **tx)
            .await?,
    )
}

fn traffic_candidate_clients_sql() -> &'static str {
    r#"
        WITH cutoff AS (
            SELECT (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                - make_interval(days => $1) AS raw_cutoff
        ), candidates AS (
            SELECT sample.client_id, min(sample.observed_at) AS oldest
            FROM traffic_counter_samples sample, cutoff
            WHERE sample.observed_at < cutoff.raw_cutoff
              AND (
                    NOT sample.inbound_promoted
                    OR EXISTS (
                        SELECT 1
                        FROM traffic_counter_samples newer
                        WHERE newer.client_id = sample.client_id
                          AND newer.source_kind = sample.source_kind
                          AND newer.interface = sample.interface
                          AND newer.observed_at < cutoff.raw_cutoff
                          AND newer.observed_at > sample.observed_at
                    )
              )
            GROUP BY sample.client_id
            UNION ALL
            SELECT rollup.client_id, min(rollup.bucket_start) AS oldest
            FROM traffic_counter_rollups rollup
            WHERE (rollup.bucket_secs = 3600
                    AND rollup.bucket_start < now() - interval '91 days')
               OR (rollup.bucket_secs = 10800
                    AND rollup.bucket_start < now() - interval '181 days')
               OR (rollup.bucket_secs = 21600
                    AND rollup.bucket_start < now() - interval '366 days')
               OR ($4 AND rollup.bucket_secs = 86400
                    AND rollup.bucket_start < now() - make_interval(days => $3))
            GROUP BY rollup.client_id
        )
        SELECT client_id
        FROM candidates
        GROUP BY client_id
        ORDER BY min(oldest), client_id
        LIMIT $2
    "#
}

fn raw_promotion_sql() -> &'static str {
    r#"
        WITH cutoff AS (
            SELECT (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                - make_interval(days => $2) AS value
        ), sequenced AS MATERIALIZED (
            SELECT
                sample.ctid,
                sample.source_kind,
                sample.interface,
                sample.observed_at,
                sample.rx_bytes,
                sample.tx_bytes,
                sample.rx_counter_epoch,
                sample.tx_counter_epoch,
                sample.sample_source,
                sample.inbound_promoted,
                lag(sample.rx_bytes) OVER stream AS previous_rx_bytes,
                lag(sample.tx_bytes) OVER stream AS previous_tx_bytes,
                lag(sample.rx_counter_epoch) OVER stream AS previous_rx_counter_epoch,
                lag(sample.tx_counter_epoch) OVER stream AS previous_tx_counter_epoch,
                lag(sample.sample_source) OVER stream AS previous_sample_source
            FROM traffic_counter_samples sample
            WHERE sample.client_id = $1
            WINDOW stream AS (
                PARTITION BY sample.source_kind, sample.interface
                ORDER BY sample.observed_at
            )
        ), candidate_groups AS MATERIALIZED (
            SELECT
                source_kind,
                interface,
                CASE WHEN sample_source LIKE 'vnstat_import:%'
                     THEN 'vnstat_import' ELSE 'live' END AS origin_kind,
                CASE
                    WHEN observed_at >= (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                            - interval '91 days' THEN 3600
                    WHEN observed_at >= (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                            - interval '181 days' THEN 10800
                    WHEN observed_at >= (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                            - interval '366 days' THEN 21600
                    ELSE 86400
                END::integer AS destination_secs,
                date_bin(
                    make_interval(secs => CASE
                        WHEN observed_at >= (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                                - interval '91 days' THEN 3600
                        WHEN observed_at >= (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                                - interval '181 days' THEN 10800
                        WHEN observed_at >= (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                                - interval '366 days' THEN 21600
                        ELSE 86400
                    END),
                    observed_at,
                    TIMESTAMPTZ '1970-01-01 00:00:00+00'
                )
                    AS bucket_start
            FROM sequenced, cutoff
            WHERE observed_at < cutoff.value
              AND NOT inbound_promoted
            GROUP BY source_kind, interface, origin_kind,
                     destination_secs, bucket_start
            ORDER BY bucket_start, source_kind, interface, origin_kind
            LIMIT $3
        ), candidate_rows AS MATERIALIZED (
            SELECT sequenced.*,
                   groups.origin_kind,
                   groups.bucket_start,
                   groups.destination_secs,
                   count(*) OVER (
                       PARTITION BY sequenced.source_kind, sequenced.interface,
                                    groups.origin_kind, groups.destination_secs,
                                    groups.bucket_start
                   ) AS expected_rows
            FROM sequenced
            JOIN candidate_groups groups
              ON groups.source_kind = sequenced.source_kind
             AND groups.interface = sequenced.interface
             AND groups.origin_kind = CASE
                    WHEN sequenced.sample_source LIKE 'vnstat_import:%'
                    THEN 'vnstat_import' ELSE 'live' END
             AND groups.destination_secs = CASE
                    WHEN sequenced.observed_at >= (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                            - interval '91 days' THEN 3600
                    WHEN sequenced.observed_at >= (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                            - interval '181 days' THEN 10800
                    WHEN sequenced.observed_at >= (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                            - interval '366 days' THEN 21600
                    ELSE 86400
                 END
             AND groups.bucket_start = date_bin(
                    make_interval(secs => groups.destination_secs),
                    sequenced.observed_at,
                    TIMESTAMPTZ '1970-01-01 00:00:00+00'
                 )
            WHERE NOT sequenced.inbound_promoted
        ), locked AS MATERIALIZED (
            SELECT candidate_rows.*
            FROM candidate_rows
            JOIN traffic_counter_samples source ON source.ctid = candidate_rows.ctid
            FOR UPDATE OF source SKIP LOCKED
        ), complete_groups AS MATERIALIZED (
            SELECT source_kind, interface, origin_kind,
                   destination_secs, bucket_start
            FROM locked
            GROUP BY source_kind, interface, origin_kind,
                     destination_secs, bucket_start, expected_rows
            HAVING count(*) = expected_rows
        ), aggregated AS MATERIALIZED (
            SELECT
                $1::text AS client_id,
                locked.source_kind,
                locked.interface,
                locked.origin_kind,
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
            JOIN complete_groups USING (
                source_kind, interface, origin_kind,
                destination_secs, bucket_start
            )
            GROUP BY locked.source_kind, locked.interface,
                     locked.origin_kind, locked.destination_secs,
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
            RETURNING source_kind, interface, origin_kind, bucket_start
        ), promoted_rows AS MATERIALIZED (
            SELECT locked.*
            FROM locked
            JOIN inserted
              ON inserted.source_kind = locked.source_kind
             AND inserted.interface = locked.interface
             AND inserted.origin_kind = locked.origin_kind
             AND inserted.bucket_start = locked.bucket_start
        ), marked_boundary AS (
            UPDATE traffic_counter_samples source
            SET inbound_promoted = TRUE
            FROM promoted_rows promoted
            WHERE source.ctid = promoted.ctid
              AND NOT EXISTS (
                    SELECT 1
                    FROM promoted_rows newer
                    WHERE newer.source_kind = source.source_kind
                      AND newer.interface = source.interface
                      AND newer.observed_at > source.observed_at
              )
            RETURNING source.ctid
        ), deleted_new AS (
            DELETE FROM traffic_counter_samples source
            USING promoted_rows promoted
            WHERE source.ctid = promoted.ctid
              AND EXISTS (
                    SELECT 1
                    FROM promoted_rows newer
                    WHERE newer.source_kind = source.source_kind
                      AND newer.interface = source.interface
                      AND newer.observed_at > source.observed_at
              )
            RETURNING source.ctid
        ), accounted_candidates AS MATERIALIZED (
            SELECT source.ctid
            FROM traffic_counter_samples source
            WHERE source.client_id = $1
              AND source.inbound_promoted
              AND EXISTS (
                    SELECT 1
                    FROM promoted_rows newer
                    WHERE newer.source_kind = source.source_kind
                      AND newer.interface = source.interface
                      AND newer.observed_at > source.observed_at
              )
            ORDER BY source.observed_at, source.source_kind, source.interface
            LIMIT $3
            FOR UPDATE OF source SKIP LOCKED
        ), deleted_accounted AS (
            DELETE FROM traffic_counter_samples source
            USING accounted_candidates
            WHERE source.ctid = accounted_candidates.ctid
            RETURNING source.ctid
        )
        SELECT
            ((SELECT count(*) FROM deleted_new)
                + (SELECT count(*) FROM deleted_accounted))::bigint AS deleted_rows,
            ((SELECT count(*) FROM aggregated)
                - (SELECT count(*) FROM inserted))::bigint AS conflicts
    "#
}

fn rollup_promotion_sql() -> &'static str {
    r#"
        WITH cutoff AS (
            SELECT (date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                - make_interval(days => $4) AS value
        ), candidate_groups AS MATERIALIZED (
            SELECT
                source_kind,
                interface,
                origin_kind,
                date_bin(
                    make_interval(secs => $3), bucket_start,
                    TIMESTAMPTZ '1970-01-01 00:00:00+00'
                ) AS destination_start,
                count(*) AS expected_rows
            FROM traffic_counter_rollups, cutoff
            WHERE client_id = $1
              AND bucket_secs = ANY($2::integer[])
              AND bucket_start + make_interval(secs => bucket_secs) <= cutoff.value
            GROUP BY source_kind, interface, origin_kind, destination_start
            ORDER BY destination_start, source_kind, interface, origin_kind
            LIMIT $5
        ), candidate_rows AS MATERIALIZED (
            SELECT source.ctid, source.*,
                   groups.destination_start, groups.expected_rows
            FROM traffic_counter_rollups source
            JOIN candidate_groups groups
              ON groups.source_kind = source.source_kind
             AND groups.interface = source.interface
             AND groups.origin_kind = source.origin_kind
             AND groups.destination_start = date_bin(
                    make_interval(secs => $3), source.bucket_start,
                    TIMESTAMPTZ '1970-01-01 00:00:00+00'
                 )
            WHERE source.client_id = $1
              AND source.bucket_secs = ANY($2::integer[])
        ), locked AS MATERIALIZED (
            SELECT candidate_rows.*
            FROM candidate_rows
            JOIN traffic_counter_rollups source ON source.ctid = candidate_rows.ctid
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
                $3::integer AS bucket_secs,
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
            RETURNING source_kind, interface, origin_kind, bucket_start
        ), deleted AS (
            DELETE FROM traffic_counter_rollups source
            USING locked, inserted
            WHERE source.ctid = locked.ctid
              AND inserted.source_kind = locked.source_kind
              AND inserted.interface = locked.interface
              AND inserted.origin_kind = locked.origin_kind
              AND inserted.bucket_start = locked.destination_start
            RETURNING source.ctid
        )
        SELECT
            (SELECT count(*) FROM deleted)::bigint AS deleted_rows,
            ((SELECT count(*) FROM aggregated)
                - (SELECT count(*) FROM inserted))::bigint AS conflicts
    "#
}

fn rollup_prune_sql() -> &'static str {
    r#"
        WITH candidates AS (
            SELECT ctid
            FROM traffic_counter_rollups
            WHERE client_id = $1
              AND bucket_start + make_interval(secs => bucket_secs) <= (
                    date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                  ) - make_interval(days => $2)
            ORDER BY bucket_start, source_kind, interface, origin_kind
            LIMIT $3
            FOR UPDATE SKIP LOCKED
        )
        DELETE FROM traffic_counter_rollups
        WHERE ctid IN (SELECT ctid FROM candidates)
    "#
}

#[cfg(test)]
#[path = "tests_traffic_retention.rs"]
mod tests;
