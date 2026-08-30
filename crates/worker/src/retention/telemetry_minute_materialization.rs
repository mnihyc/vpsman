use std::collections::HashSet;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{types::Json as SqlJson, PgPool, Postgres, Row, Transaction};
use uuid::Uuid;
use vpsman_common::{ordinal_admission_mask_has_exact_shape, AgentMetrics};

use crate::history_retention::{optional_database_deadline, DatabaseDeadline};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TelemetryMinuteConsumer {
    Core,
    Traffic,
}

impl TelemetryMinuteConsumer {
    fn head_relation(self) -> &'static str {
        match self {
            Self::Core => "telemetry_minute_materialization_heads",
            Self::Traffic => "traffic_counter_minute_heads",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TelemetryMinuteMaterializationRun {
    pub(crate) source_rows: u64,
    pub(crate) derived_rows: u64,
    pub(crate) owner_contended: bool,
}

#[derive(Clone, Debug)]
struct ClosedMinuteClaim {
    client_id: String,
    after_seq: i64,
    through_seq: i64,
    bucket_start: DateTime<Utc>,
}

#[derive(Debug, Default)]
struct ClosedMinuteClaims {
    claims: Vec<ClosedMinuteClaim>,
    owner_contended: bool,
}

#[derive(Debug)]
struct RawMinuteSample {
    client_id: String,
    id: Uuid,
    accepted_seq: i64,
    bucket_start: DateTime<Utc>,
    observed_at: DateTime<Utc>,
    metrics: AgentMetrics,
    ping_source_checked_unix: Vec<i64>,
    network_admission_mask: Vec<u8>,
    tunnel_admission_mask: Vec<u8>,
}

#[derive(Debug)]
struct TrafficMinuteObservation {
    client_id: String,
    bucket_start: DateTime<Utc>,
    accepted_seq: i64,
    observed_at: DateTime<Utc>,
    source_kind: &'static str,
    interface: String,
    rx_bytes: i64,
    tx_bytes: i64,
    sample_source: String,
}

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
struct TrafficMinuteStreamKey {
    client_id: String,
    bucket_start: DateTime<Utc>,
    source_kind: &'static str,
    interface: String,
}

/// Read-only producer frontier. `LIMIT 1` only short-circuits this existential
/// proof; the processing transaction below claims the complete oldest
/// coordinate without a numeric client or source-row limit.
pub(crate) async fn telemetry_minute_consumer_has_ready_work(
    pool: &PgPool,
    consumer: TelemetryMinuteConsumer,
) -> Result<bool> {
    let sql = format!(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM {head} head
            JOIN telemetry_projection_heads projection USING (client_id)
            JOIN telemetry_samples first_sample
              ON first_sample.client_id = head.client_id
             AND first_sample.accepted_seq = head.materialized_seq + 1
            CROSS JOIN LATERAL (
                SELECT COALESCE((
                    SELECT later.accepted_seq - 1
                    FROM telemetry_samples later
                    WHERE later.client_id = head.client_id
                      AND later.accepted_seq > head.materialized_seq
                      AND later.accepted_seq <= projection.accepted_seq
                      AND date_trunc('minute', later.observed_at)
                            IS DISTINCT FROM
                          date_trunc('minute', first_sample.observed_at)
                    ORDER BY later.accepted_seq
                    LIMIT 1
                ), projection.accepted_seq) AS through_seq
            ) accepted_minute
            WHERE first_sample.observed_at
                    < date_trunc('minute', clock_timestamp())
              AND projection.projected_seq >= accepted_minute.through_seq
            LIMIT 1
        )
        "#,
        head = consumer.head_relation(),
    );
    Ok(sqlx::query_scalar(&sql).fetch_one(pool).await?)
}

/// Exact clock boundary for the oldest producer-owned journal coordinate.
/// This is the same head -> projected suffix -> first sample frontier used by
/// the readiness proof and claim; an empty suffix has no clock deadline and is
/// woken only by its producer (with the scheduler watchdog as loss recovery).
pub(crate) async fn telemetry_minute_consumer_next_at(
    pool: &PgPool,
    consumer: TelemetryMinuteConsumer,
) -> Result<Option<DatabaseDeadline>> {
    let sql = format!(
        r#"
        WITH frontier AS (
            SELECT date_trunc('minute', first_sample.observed_at)
                       + interval '1 minute' AS database_at
            FROM {head} head
            JOIN telemetry_projection_heads projection USING (client_id)
            JOIN telemetry_samples first_sample
              ON first_sample.client_id = head.client_id
             AND first_sample.accepted_seq = head.materialized_seq + 1
            CROSS JOIN LATERAL (
                SELECT COALESCE((
                    SELECT later.accepted_seq - 1
                    FROM telemetry_samples later
                    WHERE later.client_id = head.client_id
                      AND later.accepted_seq > head.materialized_seq
                      AND later.accepted_seq <= projection.accepted_seq
                      AND date_trunc('minute', later.observed_at)
                            IS DISTINCT FROM
                          date_trunc('minute', first_sample.observed_at)
                    ORDER BY later.accepted_seq
                    LIMIT 1
                ), projection.accepted_seq) AS through_seq
            ) accepted_minute
            WHERE projection.projected_seq >= accepted_minute.through_seq
            ORDER BY first_sample.observed_at, head.client_id
            LIMIT 1
        )
        SELECT database_at,
               GREATEST(
                   EXTRACT(EPOCH FROM database_at - clock_timestamp()), 0
               )::DOUBLE PRECISION AS remaining_seconds
        FROM frontier
        "#,
        head = consumer.head_relation(),
    );
    optional_database_deadline(sqlx::query_as(&sql).fetch_optional(pool).await?)
}

/// Consume the naturally oldest closed UTC-minute coordinate from the
/// immutable projected journal. Every independently stable client at that
/// coordinate is claimed in canonical client order and published setwise;
/// contention remains scoped to its exact client. Producers never acquire
/// these heads, and independent core/traffic consumers never acquire each
/// other's heads.
pub(crate) async fn materialize_next_telemetry_minute(
    pool: &PgPool,
    consumer: TelemetryMinuteConsumer,
) -> Result<TelemetryMinuteMaterializationRun> {
    // Close producer ownership in a separate, short transaction. An acceptance
    // that already owns its projection row is skipped; every projection row we
    // can lock is re-read before this transaction commits. Because this proof
    // runs only after the UTC boundary, later acceptances belong to a later
    // natural minute. No producer row lock therefore spans source loading or
    // derived publication below.
    let certified = certify_closed_minute(pool, consumer).await?;
    if certified.claims.is_empty() {
        return Ok(TelemetryMinuteMaterializationRun {
            owner_contended: certified.owner_contended,
            ..TelemetryMinuteMaterializationRun::default()
        });
    }

    let mut tx = pool.begin().await?;
    let claimed = claim_certified_minute(&mut tx, consumer, &certified.claims).await?;
    let claims = claimed.claims;
    if claims.is_empty() {
        tx.rollback().await?;
        return Ok(TelemetryMinuteMaterializationRun {
            owner_contended: certified.owner_contended || claimed.owner_contended,
            ..TelemetryMinuteMaterializationRun::default()
        });
    }
    let source = load_raw_minute(&mut tx, &claims).await?;
    anyhow::ensure!(
        !source.is_empty(),
        "claimed telemetry minute coordinate has no contiguous raw source"
    );
    let mut run = match consumer {
        TelemetryMinuteConsumer::Core => materialize_core_minute(&mut tx, &claims, &source).await?,
        TelemetryMinuteConsumer::Traffic => {
            let traffic_counter_rows =
                materialize_traffic_minute(&mut tx, &claims, &source).await?;
            TelemetryMinuteMaterializationRun {
                derived_rows: traffic_counter_rows,
                ..TelemetryMinuteMaterializationRun::default()
            }
        }
    };
    advance_consumer_heads(&mut tx, consumer, &claims).await?;
    tx.commit().await?;
    run.source_rows = source.len() as u64;
    run.owner_contended = certified.owner_contended || claimed.owner_contended;
    Ok(run)
}

async fn certify_closed_minute(
    pool: &PgPool,
    consumer: TelemetryMinuteConsumer,
) -> Result<ClosedMinuteClaims> {
    let mut tx = pool.begin().await?;
    let candidate_sql = format!(
        r#"
        WITH eligible AS MATERIALIZED (
            SELECT
                head.client_id,
                head.materialized_seq,
                date_trunc('minute', first_sample.observed_at) AS bucket_start
            FROM {head} head
            JOIN telemetry_projection_heads projection USING (client_id)
            JOIN LATERAL (
                SELECT sample.observed_at
                FROM telemetry_samples sample
                WHERE sample.client_id = head.client_id
                  AND sample.accepted_seq = head.materialized_seq + 1
                LIMIT 1
            ) first_sample ON TRUE
            CROSS JOIN LATERAL (
                SELECT COALESCE((
                    SELECT later.accepted_seq - 1
                    FROM telemetry_samples later
                    WHERE later.client_id = head.client_id
                      AND later.accepted_seq > head.materialized_seq
                      AND later.accepted_seq <= projection.accepted_seq
                      AND date_trunc('minute', later.observed_at)
                            IS DISTINCT FROM
                          date_trunc('minute', first_sample.observed_at)
                    ORDER BY later.accepted_seq
                    LIMIT 1
                ), projection.accepted_seq) AS through_seq
            ) accepted_minute
            WHERE head.materialized_seq < projection.accepted_seq
              AND first_sample.observed_at
                    < date_trunc('minute', clock_timestamp())
              AND projection.projected_seq >= accepted_minute.through_seq
        ), oldest AS MATERIALIZED (
            SELECT min(bucket_start) AS bucket_start
            FROM eligible
        )
        SELECT eligible.client_id, eligible.materialized_seq,
               eligible.bucket_start
        FROM eligible
        JOIN oldest USING (bucket_start)
        ORDER BY eligible.client_id
        "#,
        head = consumer.head_relation(),
    );
    let candidate_rows = sqlx::query(&candidate_sql).fetch_all(&mut *tx).await?;
    if candidate_rows.is_empty() {
        tx.commit().await?;
        return Ok(ClosedMinuteClaims::default());
    }
    let client_ids = candidate_rows
        .iter()
        .map(|row| row.try_get::<String, _>("client_id"))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let after_seq = candidate_rows
        .iter()
        .map(|row| row.try_get::<i64, _>("materialized_seq"))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let bucket_start: DateTime<Utc> = candidate_rows[0].try_get("bucket_start")?;

    // Consumer heads are independent exact owners. Never wait behind another
    // consumer's publication, and never let that one owner discard peers that
    // can be certified now.
    let head_lock_sql = format!(
        r#"
        WITH candidate AS MATERIALIZED (
            SELECT *
            FROM UNNEST($1::TEXT[], $2::BIGINT[])
                AS candidate(client_id, materialized_seq)
        )
        SELECT head.client_id, head.materialized_seq
        FROM {head} head
        JOIN candidate
          ON candidate.client_id = head.client_id
         AND candidate.materialized_seq = head.materialized_seq
        ORDER BY head.client_id
        FOR UPDATE OF head SKIP LOCKED
        "#,
        head = consumer.head_relation(),
    );
    let head_rows = sqlx::query(&head_lock_sql)
        .bind(&client_ids)
        .bind(&after_seq)
        .fetch_all(&mut *tx)
        .await?;
    let mut owner_contended = head_rows.len() != client_ids.len();
    let head_client_ids = head_rows
        .iter()
        .map(|row| row.try_get::<String, _>("client_id"))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let head_after_seq = head_rows
        .iter()
        .map(|row| row.try_get::<i64, _>("materialized_seq"))
        .collect::<std::result::Result<Vec<_>, _>>()?;

    // Projection rows are the producer fence. SKIP LOCKED rejects only the
    // exact acceptance/projector owner already in flight. The acquired SHARE
    // locks survive one fresh statement snapshot below, then this short
    // transaction commits before any aggregation starts.
    let projection_rows = sqlx::query(
        r#"
        SELECT projection.client_id
        FROM telemetry_projection_heads projection
        WHERE projection.client_id = ANY($1::TEXT[])
        ORDER BY projection.client_id
        FOR SHARE OF projection SKIP LOCKED
        "#,
    )
    .bind(&head_client_ids)
    .fetch_all(&mut *tx)
    .await?;
    owner_contended |= projection_rows.len() != head_client_ids.len();
    let projection_client_ids = projection_rows
        .into_iter()
        .map(|row| row.try_get::<String, _>("client_id"))
        .collect::<std::result::Result<HashSet<_>, _>>()?;
    let mut stable_client_ids = Vec::with_capacity(projection_client_ids.len());
    let mut stable_after_seq = Vec::with_capacity(projection_client_ids.len());
    for (client_id, after_seq) in head_client_ids.iter().zip(&head_after_seq) {
        if projection_client_ids.contains(client_id) {
            stable_client_ids.push(client_id.as_str());
            stable_after_seq.push(*after_seq);
        }
    }

    let certification_sql = format!(
        r#"
        WITH candidate AS MATERIALIZED (
            SELECT *
            FROM UNNEST($1::TEXT[], $2::BIGINT[])
                AS candidate(client_id, materialized_seq)
        )
        SELECT candidate.client_id, candidate.materialized_seq,
               accepted_minute.through_seq, $3::TIMESTAMPTZ AS bucket_start
        FROM candidate
        JOIN {head} head
          ON head.client_id = candidate.client_id
         AND head.materialized_seq = candidate.materialized_seq
        JOIN telemetry_projection_heads projection
          ON projection.client_id = candidate.client_id
        JOIN telemetry_samples first_sample
          ON first_sample.client_id = candidate.client_id
         AND first_sample.accepted_seq = candidate.materialized_seq + 1
        CROSS JOIN LATERAL (
            SELECT COALESCE((
                SELECT later.accepted_seq - 1
                FROM telemetry_samples later
                WHERE later.client_id = candidate.client_id
                  AND later.accepted_seq > candidate.materialized_seq
                  AND later.accepted_seq <= projection.accepted_seq
                  AND date_trunc('minute', later.observed_at)
                        IS DISTINCT FROM $3::TIMESTAMPTZ
                ORDER BY later.accepted_seq
                LIMIT 1
            ), projection.accepted_seq) AS through_seq
        ) accepted_minute
        WHERE date_trunc('minute', first_sample.observed_at) = $3::TIMESTAMPTZ
          AND first_sample.observed_at < date_trunc('minute', clock_timestamp())
          AND projection.projected_seq >= accepted_minute.through_seq
        ORDER BY candidate.client_id
        "#,
        head = consumer.head_relation(),
    );
    let certified_rows = sqlx::query(&certification_sql)
        .bind(&stable_client_ids)
        .bind(&stable_after_seq)
        .bind(bucket_start)
        .fetch_all(&mut *tx)
        .await?;
    owner_contended |= certified_rows.len() != stable_client_ids.len();
    let claims = certified_rows
        .into_iter()
        .map(|row| {
            let after_seq: i64 = row.try_get("materialized_seq")?;
            let through_seq: i64 = row.try_get("through_seq")?;
            anyhow::ensure!(
                through_seq > after_seq,
                "closed telemetry minute certification has an empty sequence fence"
            );
            Ok(ClosedMinuteClaim {
                client_id: row.try_get("client_id")?,
                after_seq,
                through_seq,
                bucket_start: row.try_get("bucket_start")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    tx.commit().await?;
    Ok(ClosedMinuteClaims {
        claims,
        owner_contended,
    })
}

/// Reclaim only independently certified client owners for publication. The
/// producer fence was already closed and released; this transaction owns the
/// parent lifetime and consumer cursor only, both in canonical client order.
async fn claim_certified_minute(
    tx: &mut Transaction<'_, Postgres>,
    consumer: TelemetryMinuteConsumer,
    certified: &[ClosedMinuteClaim],
) -> Result<ClosedMinuteClaims> {
    let client_ids = certified
        .iter()
        .map(|claim| claim.client_id.as_str())
        .collect::<Vec<_>>();
    let after_seq = certified
        .iter()
        .map(|claim| claim.after_seq)
        .collect::<Vec<_>>();
    let through_seq = certified
        .iter()
        .map(|claim| claim.through_seq)
        .collect::<Vec<_>>();
    let bucket_start = certified
        .iter()
        .map(|claim| claim.bucket_start)
        .collect::<Vec<_>>();

    let locked_clients = sqlx::query_scalar::<_, String>(
        r#"
        SELECT client.id
        FROM clients client
        WHERE client.id = ANY($1::TEXT[])
        ORDER BY client.id
        FOR KEY SHARE OF client SKIP LOCKED
        "#,
    )
    .bind(&client_ids)
    .fetch_all(&mut **tx)
    .await?;
    let locked_clients = locked_clients.into_iter().collect::<HashSet<_>>();
    let mut available_client_ids = Vec::with_capacity(locked_clients.len());
    let mut available_after_seq = Vec::with_capacity(locked_clients.len());
    let mut available_through_seq = Vec::with_capacity(locked_clients.len());
    let mut available_bucket_start = Vec::with_capacity(locked_clients.len());
    for (((client_id, after_seq), through_seq), bucket_start) in client_ids
        .iter()
        .zip(&after_seq)
        .zip(&through_seq)
        .zip(&bucket_start)
    {
        if locked_clients.contains(*client_id) {
            available_client_ids.push(*client_id);
            available_after_seq.push(*after_seq);
            available_through_seq.push(*through_seq);
            available_bucket_start.push(*bucket_start);
        }
    }

    let claim_sql = format!(
        r#"
        WITH certified AS MATERIALIZED (
            SELECT *
            FROM UNNEST(
                $1::TEXT[], $2::BIGINT[], $3::BIGINT[], $4::TIMESTAMPTZ[]
            ) AS claim(client_id, after_seq, through_seq, bucket_start)
        ), ready AS MATERIALIZED (
            SELECT claim.*
            FROM certified claim
            JOIN {head} head
              ON head.client_id = claim.client_id
             AND head.materialized_seq = claim.after_seq
            JOIN telemetry_projection_heads projection
              ON projection.client_id = claim.client_id
            JOIN telemetry_samples first_sample
             ON first_sample.client_id = claim.client_id
             AND first_sample.accepted_seq = claim.after_seq + 1
             AND date_trunc('minute', first_sample.observed_at)
                    = claim.bucket_start
            CROSS JOIN LATERAL (
                SELECT COALESCE((
                    SELECT later.accepted_seq - 1
                    FROM telemetry_samples later
                    WHERE later.client_id = claim.client_id
                      AND later.accepted_seq > claim.after_seq
                      AND later.accepted_seq <= projection.accepted_seq
                      AND date_trunc('minute', later.observed_at)
                            IS DISTINCT FROM claim.bucket_start
                    ORDER BY later.accepted_seq
                    LIMIT 1
                ), projection.accepted_seq) AS through_seq
            ) fresh
            WHERE fresh.through_seq = claim.through_seq
              AND projection.projected_seq >= claim.through_seq
              AND first_sample.observed_at
                    < date_trunc('minute', clock_timestamp())
        ), claimed AS MATERIALIZED (
            SELECT ready.*
            FROM {head} head
            JOIN ready
              ON ready.client_id = head.client_id
             AND ready.after_seq = head.materialized_seq
            ORDER BY head.client_id
            FOR UPDATE OF head SKIP LOCKED
        )
        SELECT client_id, after_seq, through_seq, bucket_start
        FROM claimed
        ORDER BY client_id
        "#,
        head = consumer.head_relation(),
    );
    let claimed_rows = sqlx::query(&claim_sql)
        .bind(&available_client_ids)
        .bind(&available_after_seq)
        .bind(&available_through_seq)
        .bind(&available_bucket_start)
        .fetch_all(&mut **tx)
        .await?;
    let owner_contended = claimed_rows.len() != certified.len();
    let claims = claimed_rows
        .into_iter()
        .map(|row| {
            Ok(ClosedMinuteClaim {
                client_id: row.try_get("client_id")?,
                after_seq: row.try_get("after_seq")?,
                through_seq: row.try_get("through_seq")?,
                bucket_start: row.try_get("bucket_start")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(ClosedMinuteClaims {
        claims,
        owner_contended,
    })
}

async fn load_raw_minute(
    tx: &mut Transaction<'_, Postgres>,
    claims: &[ClosedMinuteClaim],
) -> Result<Vec<RawMinuteSample>> {
    let client_ids = claims
        .iter()
        .map(|claim| claim.client_id.as_str())
        .collect::<Vec<_>>();
    let after_seq = claims
        .iter()
        .map(|claim| claim.after_seq)
        .collect::<Vec<_>>();
    let through_seq = claims
        .iter()
        .map(|claim| claim.through_seq)
        .collect::<Vec<_>>();
    let bucket_start = claims
        .iter()
        .map(|claim| claim.bucket_start)
        .collect::<Vec<_>>();
    let rows = sqlx::query(
        r#"
        WITH claims AS MATERIALIZED (
            SELECT *
            FROM UNNEST(
                $1::TEXT[], $2::BIGINT[], $3::BIGINT[], $4::TIMESTAMPTZ[]
            ) AS claim(client_id, after_seq, through_seq, bucket_start)
        )
        SELECT sample.client_id, claims.bucket_start,
               sample.id, sample.accepted_seq,
               sample.observed_at, sample.payload,
               ping_source_checked_unix,
               network_admission_mask, tunnel_admission_mask
        FROM claims
        JOIN telemetry_samples sample
          ON sample.client_id = claims.client_id
         AND sample.accepted_seq > claims.after_seq
         AND sample.accepted_seq <= claims.through_seq
        ORDER BY sample.client_id, sample.accepted_seq
        "#,
    )
    .bind(&client_ids)
    .bind(&after_seq)
    .bind(&through_seq)
    .bind(&bucket_start)
    .fetch_all(&mut **tx)
    .await?;
    let expected = claims.iter().try_fold(0_i64, |total, claim| {
        claim
            .through_seq
            .checked_sub(claim.after_seq)
            .and_then(|count| total.checked_add(count))
            .context("telemetry minute source cardinality overflow")
    })?;
    anyhow::ensure!(
        i64::try_from(rows.len()).ok() == Some(expected),
        "telemetry minute source sequence is not contiguous"
    );
    let mut claim_index = 0_usize;
    let mut expected_seq = claims.first().map(|claim| claim.after_seq + 1);
    rows.into_iter()
        .map(|row| {
            let client_id: String = row.try_get("client_id")?;
            while claims
                .get(claim_index)
                .is_some_and(|claim| claim.client_id < client_id)
            {
                claim_index += 1;
                expected_seq = claims.get(claim_index).map(|claim| claim.after_seq + 1);
            }
            let claim = claims
                .get(claim_index)
                .context("telemetry minute source has an unclaimed client")?;
            anyhow::ensure!(
                claim.client_id == client_id,
                "telemetry minute source has an unclaimed client"
            );
            let accepted_seq: i64 = row.try_get("accepted_seq")?;
            anyhow::ensure!(
                Some(accepted_seq) == expected_seq,
                "telemetry minute source sequence is not contiguous"
            );
            expected_seq = accepted_seq.checked_add(1);
            let observed_at: DateTime<Utc> = row.try_get("observed_at")?;
            anyhow::ensure!(
                observed_at >= claim.bucket_start
                    && observed_at < claim.bucket_start + chrono::Duration::minutes(1),
                "telemetry minute claim crossed its natural UTC boundary"
            );
            Ok(RawMinuteSample {
                client_id,
                id: row.try_get("id")?,
                accepted_seq,
                bucket_start: row.try_get("bucket_start")?,
                observed_at,
                metrics: row.try_get::<SqlJson<AgentMetrics>, _>("payload")?.0,
                ping_source_checked_unix: row.try_get("ping_source_checked_unix")?,
                network_admission_mask: row.try_get("network_admission_mask")?,
                tunnel_admission_mask: row.try_get("tunnel_admission_mask")?,
            })
        })
        .collect()
}

async fn advance_consumer_heads(
    tx: &mut Transaction<'_, Postgres>,
    consumer: TelemetryMinuteConsumer,
    claims: &[ClosedMinuteClaim],
) -> Result<()> {
    let sql = format!(
        r#"
        WITH claims AS MATERIALIZED (
            SELECT *
            FROM UNNEST($1::TEXT[], $2::BIGINT[], $3::BIGINT[])
                AS claim(client_id, after_seq, through_seq)
        )
        UPDATE {head} head
        SET materialized_seq = claims.through_seq,
            materialized_at = clock_timestamp(),
            updated_at = clock_timestamp()
        FROM claims
        WHERE head.client_id = claims.client_id
          AND head.materialized_seq = claims.after_seq
        "#,
        head = consumer.head_relation(),
    );
    let client_ids = claims
        .iter()
        .map(|claim| claim.client_id.as_str())
        .collect::<Vec<_>>();
    let after_seq = claims
        .iter()
        .map(|claim| claim.after_seq)
        .collect::<Vec<_>>();
    let through_seq = claims
        .iter()
        .map(|claim| claim.through_seq)
        .collect::<Vec<_>>();
    let result = sqlx::query(&sql)
        .bind(&client_ids)
        .bind(&after_seq)
        .bind(&through_seq)
        .execute(&mut **tx)
        .await?;
    anyhow::ensure!(
        result.rows_affected() == claims.len() as u64,
        "telemetry minute consumer cursor set changed outside its exact owners"
    );
    Ok(())
}

async fn materialize_core_minute(
    tx: &mut Transaction<'_, Postgres>,
    claims: &[ClosedMinuteClaim],
    source: &[RawMinuteSample],
) -> Result<TelemetryMinuteMaterializationRun> {
    let resources = materialize_resource_minute(tx, claims).await?;
    let ping = materialize_ping_minute(tx, source).await?;
    let reachability = materialize_network_observation_minute(tx, claims).await?;
    Ok(TelemetryMinuteMaterializationRun {
        derived_rows: resources.saturating_add(ping).saturating_add(reachability),
        ..TelemetryMinuteMaterializationRun::default()
    })
}

async fn materialize_resource_minute(
    tx: &mut Transaction<'_, Postgres>,
    claims: &[ClosedMinuteClaim],
) -> Result<u64> {
    let result = sqlx::query(
        r#"
        WITH claims AS MATERIALIZED (
            SELECT *
            FROM UNNEST(
                $1::TEXT[], $2::BIGINT[], $3::BIGINT[], $4::TIMESTAMPTZ[]
            ) AS claim(client_id, after_seq, through_seq, bucket_start)
        ), source AS MATERIALIZED (
            SELECT sample.client_id, claims.bucket_start,
                   sample.accepted_seq, sample.observed_at,
                   sample.cpu_utilization_ratio, sample.cpu_cores,
                   sample.cpu_load_1, sample.cpu_load_5, sample.cpu_load_15,
                   sample.memory_total_bytes, sample.memory_available_bytes,
                   sample.swap_total_bytes, sample.swap_available_bytes,
                   sample.disk_total_bytes, sample.disk_available_bytes,
                   sample.tcp_sockets, sample.udp_sockets
            FROM claims
            JOIN telemetry_samples sample
              ON sample.client_id = claims.client_id
             AND sample.accepted_seq > claims.after_seq
             AND sample.accepted_seq <= claims.through_seq
        )
        INSERT INTO telemetry_rollups (
            client_id, bucket_start, bucket_secs, sample_count,
            cpu_usage_sample_count, cpu_usage_sum, cpu_usage_avg, cpu_usage_max,
            cpu_cores_max,
            cpu_load_1_avg, cpu_load_1_sum, cpu_load_1_max,
            cpu_load_5_avg, cpu_load_5_sum, cpu_load_5_max,
            cpu_load_15_avg, cpu_load_15_sum, cpu_load_15_max,
            memory_total_bytes_max,
            memory_available_bytes_avg, memory_available_bytes_sum,
            memory_available_bytes_min,
            memory_used_ratio_avg, memory_used_ratio_sum, memory_used_ratio_max,
            swap_sample_count, swap_total_bytes_max,
            swap_available_bytes_avg, swap_available_bytes_sum,
            swap_available_bytes_min,
            swap_used_ratio_avg, swap_used_ratio_sum, swap_used_ratio_max,
            disk_sample_count, disk_total_bytes_max,
            disk_available_bytes_avg, disk_available_bytes_sum,
            disk_available_bytes_min,
            disk_used_ratio_avg, disk_used_ratio_sum, disk_used_ratio_max,
            connections_sample_count, tcp_sockets_latest, udp_sockets_latest,
            connections_observed_at, latest_observed_at, updated_at
        )
        SELECT
            client_id, bucket_start, 60, count(*)::integer,
            count(cpu_utilization_ratio)::integer,
            COALESCE(sum(cpu_utilization_ratio), 0),
            avg(cpu_utilization_ratio), max(cpu_utilization_ratio),
            max(cpu_cores),
            avg(cpu_load_1), sum(cpu_load_1), max(cpu_load_1),
            avg(cpu_load_5), sum(cpu_load_5), max(cpu_load_5),
            avg(cpu_load_15), sum(cpu_load_15), max(cpu_load_15),
            max(memory_total_bytes),
            round(avg(memory_available_bytes::numeric))::bigint,
            sum(memory_available_bytes::numeric), min(memory_available_bytes),
            avg(CASE WHEN memory_total_bytes > 0 THEN
                    (memory_total_bytes - memory_available_bytes)::double precision
                        / memory_total_bytes::double precision
                ELSE 0 END),
            sum(CASE WHEN memory_total_bytes > 0 THEN
                    (memory_total_bytes - memory_available_bytes)::double precision
                        / memory_total_bytes::double precision
                ELSE 0 END),
            max(CASE WHEN memory_total_bytes > 0 THEN
                    (memory_total_bytes - memory_available_bytes)::double precision
                        / memory_total_bytes::double precision
                ELSE 0 END),
            count(*) FILTER (WHERE swap_total_bytes > 0)::integer,
            max(swap_total_bytes),
            CASE WHEN count(*) FILTER (WHERE swap_total_bytes > 0) > 0
                THEN round(avg(swap_available_bytes::numeric)
                    FILTER (WHERE swap_total_bytes > 0))::bigint
                WHEN max(swap_total_bytes) = 0 THEN 0 ELSE NULL END,
            COALESCE(sum(swap_available_bytes::numeric)
                FILTER (WHERE swap_total_bytes > 0), 0),
            CASE WHEN count(*) FILTER (WHERE swap_total_bytes > 0) > 0
                THEN min(swap_available_bytes) FILTER (WHERE swap_total_bytes > 0)
                WHEN max(swap_total_bytes) = 0 THEN 0 ELSE NULL END,
            avg((swap_total_bytes - swap_available_bytes)::double precision
                    / swap_total_bytes::double precision)
                FILTER (WHERE swap_total_bytes > 0),
            COALESCE(sum((swap_total_bytes - swap_available_bytes)::double precision
                    / swap_total_bytes::double precision)
                FILTER (WHERE swap_total_bytes > 0), 0),
            max((swap_total_bytes - swap_available_bytes)::double precision
                    / swap_total_bytes::double precision)
                FILTER (WHERE swap_total_bytes > 0),
            count(*) FILTER (WHERE disk_total_bytes > 0)::integer,
            COALESCE(max(disk_total_bytes) FILTER (WHERE disk_total_bytes > 0), 0),
            COALESCE(round(avg(disk_available_bytes::numeric)
                FILTER (WHERE disk_total_bytes > 0))::bigint, 0),
            COALESCE(sum(disk_available_bytes::numeric)
                FILTER (WHERE disk_total_bytes > 0), 0),
            COALESCE(min(disk_available_bytes) FILTER (WHERE disk_total_bytes > 0), 0),
            COALESCE(avg((disk_total_bytes - disk_available_bytes)::double precision
                    / disk_total_bytes::double precision)
                FILTER (WHERE disk_total_bytes > 0), 0),
            COALESCE(sum((disk_total_bytes - disk_available_bytes)::double precision
                    / disk_total_bytes::double precision)
                FILTER (WHERE disk_total_bytes > 0), 0),
            COALESCE(max((disk_total_bytes - disk_available_bytes)::double precision
                    / disk_total_bytes::double precision)
                FILTER (WHERE disk_total_bytes > 0), 0),
            count(*) FILTER (WHERE tcp_sockets <> 9223372036854775807)::integer,
            (array_agg(tcp_sockets ORDER BY observed_at DESC, accepted_seq DESC)
                FILTER (WHERE tcp_sockets <> 9223372036854775807))[1],
            (array_agg(udp_sockets ORDER BY observed_at DESC, accepted_seq DESC)
                FILTER (WHERE tcp_sockets <> 9223372036854775807))[1],
            max(observed_at) FILTER (WHERE tcp_sockets <> 9223372036854775807),
            max(observed_at), clock_timestamp()
        FROM source
        GROUP BY client_id, bucket_start
        ORDER BY client_id
        "#,
    )
    .bind(
        claims
            .iter()
            .map(|claim| claim.client_id.as_str())
            .collect::<Vec<_>>(),
    )
    .bind(
        claims
            .iter()
            .map(|claim| claim.after_seq)
            .collect::<Vec<_>>(),
    )
    .bind(
        claims
            .iter()
            .map(|claim| claim.through_seq)
            .collect::<Vec<_>>(),
    )
    .bind(
        claims
            .iter()
            .map(|claim| claim.bucket_start)
            .collect::<Vec<_>>(),
    )
    .execute(&mut **tx)
    .await?;
    anyhow::ensure!(
        result.rows_affected() == claims.len() as u64,
        "resource minute did not publish every exact client coordinate"
    );
    Ok(result.rows_affected())
}

/// Converts only locators belonging to this exact closed source coordinate.
/// The core cursor and these additive fragments commit together, so a locator
/// contributes once even when its measured minute is older than its enclosing
/// telemetry sample minute.
async fn materialize_network_observation_minute(
    tx: &mut Transaction<'_, Postgres>,
    claims: &[ClosedMinuteClaim],
) -> Result<u64> {
    let result = sqlx::query(
        r#"
        WITH claims AS MATERIALIZED (
            SELECT *
            FROM UNNEST($1::TEXT[], $2::BIGINT[], $3::BIGINT[])
                AS claim(client_id, after_seq, through_seq)
        ), source_samples AS MATERIALIZED (
            SELECT sample.id, sample.payload
            FROM claims
            JOIN telemetry_samples sample
              ON sample.client_id = claims.client_id
             AND sample.accepted_seq > claims.after_seq
             AND sample.accepted_seq <= claims.through_seq
        ), exact_rows AS MATERIALIZED (
            SELECT locator.automatic_series_id AS series_id,
                   locator.id, locator.observed_at, locator.received_at,
                   raw.observation
            FROM source_samples sample
            JOIN network_observations locator
              ON locator.automatic_sample_id = sample.id
             AND locator.source = 'automatic'
            CROSS JOIN LATERAL (
                SELECT sample.payload -> 'tunnel_reachability'
                             -> (locator.automatic_payload_ordinal::integer - 1)
                             AS observation
            ) raw
            WHERE raw.observation IS NOT NULL
              AND (raw.observation ->> 'id')::uuid = locator.id
        ), minute_rows AS MATERIALIZED (
            SELECT series_id, id, observed_at, received_at,
                   date_bin(
                       interval '1 minute', observed_at,
                       TIMESTAMPTZ '1970-01-01 00:00:00+00'
                   ) AS bucket_start,
                   CASE (observation ->> 'healthy')::boolean
                       WHEN TRUE THEN 1 ELSE 0 END::smallint AS health_state,
                   (observation ->> 'stale_after_secs')::bigint
                       AS stale_after_secs,
                   (observation ->> 'healthy')::boolean AS healthy,
                   (observation ->> 'transmitted')::integer AS transmitted,
                   (observation ->> 'received')::integer AS received,
                   (observation ->> 'latency_min_ms')::double precision
                       AS latency_min_ms,
                   (observation ->> 'latency_avg_ms')::double precision
                       AS latency_avg_ms,
                   (observation ->> 'latency_max_ms')::double precision
                       AS latency_max_ms,
                   (observation ->> 'latency_mdev_ms')::double precision
                       AS latency_mdev_ms,
                   (observation ->> 'packet_loss_ratio')::double precision
                       AS packet_loss_ratio,
                   observation ->> 'reason' AS reason
            FROM exact_rows
        ), aggregates AS MATERIALIZED (
            SELECT series_id, bucket_start, health_state,
                   count(*)::bigint AS sample_count,
                   sum(transmitted::numeric) AS transmitted_total,
                   count(transmitted)::bigint AS transmitted_sample_count,
                   sum(received::numeric) AS received_total,
                   count(received)::bigint AS received_sample_count,
                   sum(COALESCE(latency_avg_ms, 0.0)) AS latency_sum_ms,
                   count(latency_avg_ms)::bigint AS latency_sample_count,
                   min(latency_avg_ms) AS latency_min_ms,
                   max(latency_avg_ms) AS latency_max_ms,
                   sum(COALESCE(latency_mdev_ms, 0.0))
                       AS latency_mdev_sum_ms,
                   count(latency_mdev_ms)::bigint
                       AS latency_mdev_sample_count,
                   sum(packet_loss_ratio) AS packet_loss_sum_ratio,
                   count(packet_loss_ratio)::bigint
                       AS packet_loss_sample_count,
                   min(packet_loss_ratio) AS packet_loss_min_ratio,
                   max(packet_loss_ratio) AS packet_loss_max_ratio
            FROM minute_rows
            GROUP BY series_id, bucket_start, health_state
        ), latest AS MATERIALIZED (
            SELECT DISTINCT ON (series_id, bucket_start, health_state)
                   series_id, bucket_start, health_state, id,
                   stale_after_secs, healthy, transmitted, received,
                   latency_min_ms, latency_avg_ms, latency_max_ms,
                   latency_mdev_ms, packet_loss_ratio, reason,
                   observed_at, received_at
            FROM minute_rows
            ORDER BY series_id, bucket_start, health_state,
                     observed_at DESC, id DESC
        ), merged AS (
            INSERT INTO network_observation_rollups AS current (
                series_id, bucket_secs, bucket_start, health_state,
                sample_count, transmitted_total, transmitted_sample_count,
                received_total, received_sample_count, latency_sum_ms,
                latency_sample_count, latency_min_ms, latency_max_ms,
                latency_mdev_sum_ms, latency_mdev_sample_count,
                packet_loss_sum_ratio, packet_loss_sample_count,
                packet_loss_min_ratio, packet_loss_max_ratio,
                latest_observation_id, latest_stale_after_secs, latest_healthy,
                latest_transmitted, latest_received, latest_latency_min_ms,
                latest_latency_avg_ms, latest_latency_max_ms,
                latest_latency_mdev_ms, latest_packet_loss_ratio, latest_reason,
                latest_observed_at, latest_received_at
            )
            SELECT aggregate.series_id, 60, aggregate.bucket_start,
                   aggregate.health_state, aggregate.sample_count,
                   aggregate.transmitted_total,
                   aggregate.transmitted_sample_count,
                   aggregate.received_total,
                   aggregate.received_sample_count,
                   aggregate.latency_sum_ms,
                   aggregate.latency_sample_count,
                   aggregate.latency_min_ms,
                   aggregate.latency_max_ms,
                   aggregate.latency_mdev_sum_ms,
                   aggregate.latency_mdev_sample_count,
                   aggregate.packet_loss_sum_ratio,
                   aggregate.packet_loss_sample_count,
                   aggregate.packet_loss_min_ratio,
                   aggregate.packet_loss_max_ratio,
                   latest.id, latest.stale_after_secs, latest.healthy,
                   latest.transmitted, latest.received,
                   latest.latency_min_ms, latest.latency_avg_ms,
                   latest.latency_max_ms, latest.latency_mdev_ms,
                   latest.packet_loss_ratio, latest.reason,
                   latest.observed_at, latest.received_at
            FROM aggregates aggregate
            JOIN latest USING (series_id, bucket_start, health_state)
            ON CONFLICT (series_id, bucket_secs, bucket_start, health_state)
            DO UPDATE SET
                sample_count = current.sample_count + EXCLUDED.sample_count,
                transmitted_total = current.transmitted_total
                    + EXCLUDED.transmitted_total,
                transmitted_sample_count = current.transmitted_sample_count
                    + EXCLUDED.transmitted_sample_count,
                received_total = current.received_total + EXCLUDED.received_total,
                received_sample_count = current.received_sample_count
                    + EXCLUDED.received_sample_count,
                latency_sum_ms = current.latency_sum_ms + EXCLUDED.latency_sum_ms,
                latency_sample_count = current.latency_sample_count
                    + EXCLUDED.latency_sample_count,
                latency_min_ms = CASE
                    WHEN current.latency_min_ms IS NULL
                        THEN EXCLUDED.latency_min_ms
                    WHEN EXCLUDED.latency_min_ms IS NULL
                        THEN current.latency_min_ms
                    ELSE LEAST(current.latency_min_ms, EXCLUDED.latency_min_ms)
                END,
                latency_max_ms = CASE
                    WHEN current.latency_max_ms IS NULL
                        THEN EXCLUDED.latency_max_ms
                    WHEN EXCLUDED.latency_max_ms IS NULL
                        THEN current.latency_max_ms
                    ELSE GREATEST(current.latency_max_ms, EXCLUDED.latency_max_ms)
                END,
                latency_mdev_sum_ms = current.latency_mdev_sum_ms
                    + EXCLUDED.latency_mdev_sum_ms,
                latency_mdev_sample_count = current.latency_mdev_sample_count
                    + EXCLUDED.latency_mdev_sample_count,
                packet_loss_sum_ratio = current.packet_loss_sum_ratio
                    + EXCLUDED.packet_loss_sum_ratio,
                packet_loss_sample_count = current.packet_loss_sample_count
                    + EXCLUDED.packet_loss_sample_count,
                packet_loss_min_ratio = CASE
                    WHEN current.packet_loss_min_ratio IS NULL
                        THEN EXCLUDED.packet_loss_min_ratio
                    WHEN EXCLUDED.packet_loss_min_ratio IS NULL
                        THEN current.packet_loss_min_ratio
                    ELSE LEAST(
                        current.packet_loss_min_ratio,
                        EXCLUDED.packet_loss_min_ratio
                    )
                END,
                packet_loss_max_ratio = CASE
                    WHEN current.packet_loss_max_ratio IS NULL
                        THEN EXCLUDED.packet_loss_max_ratio
                    WHEN EXCLUDED.packet_loss_max_ratio IS NULL
                        THEN current.packet_loss_max_ratio
                    ELSE GREATEST(
                        current.packet_loss_max_ratio,
                        EXCLUDED.packet_loss_max_ratio
                    )
                END,
                latest_observation_id = CASE WHEN
                    (EXCLUDED.latest_observed_at,
                     EXCLUDED.latest_observation_id)
                        > (current.latest_observed_at,
                           current.latest_observation_id)
                    THEN EXCLUDED.latest_observation_id
                    ELSE current.latest_observation_id
                END,
                latest_stale_after_secs = CASE WHEN
                    (EXCLUDED.latest_observed_at,
                     EXCLUDED.latest_observation_id)
                        > (current.latest_observed_at,
                           current.latest_observation_id)
                    THEN EXCLUDED.latest_stale_after_secs
                    ELSE current.latest_stale_after_secs
                END,
                latest_healthy = CASE WHEN
                    (EXCLUDED.latest_observed_at,
                     EXCLUDED.latest_observation_id)
                        > (current.latest_observed_at,
                           current.latest_observation_id)
                    THEN EXCLUDED.latest_healthy ELSE current.latest_healthy
                END,
                latest_transmitted = CASE WHEN
                    (EXCLUDED.latest_observed_at,
                     EXCLUDED.latest_observation_id)
                        > (current.latest_observed_at,
                           current.latest_observation_id)
                    THEN EXCLUDED.latest_transmitted ELSE current.latest_transmitted
                END,
                latest_received = CASE WHEN
                    (EXCLUDED.latest_observed_at,
                     EXCLUDED.latest_observation_id)
                        > (current.latest_observed_at,
                           current.latest_observation_id)
                    THEN EXCLUDED.latest_received ELSE current.latest_received
                END,
                latest_latency_min_ms = CASE WHEN
                    (EXCLUDED.latest_observed_at,
                     EXCLUDED.latest_observation_id)
                        > (current.latest_observed_at,
                           current.latest_observation_id)
                    THEN EXCLUDED.latest_latency_min_ms
                    ELSE current.latest_latency_min_ms
                END,
                latest_latency_avg_ms = CASE WHEN
                    (EXCLUDED.latest_observed_at,
                     EXCLUDED.latest_observation_id)
                        > (current.latest_observed_at,
                           current.latest_observation_id)
                    THEN EXCLUDED.latest_latency_avg_ms
                    ELSE current.latest_latency_avg_ms
                END,
                latest_latency_max_ms = CASE WHEN
                    (EXCLUDED.latest_observed_at,
                     EXCLUDED.latest_observation_id)
                        > (current.latest_observed_at,
                           current.latest_observation_id)
                    THEN EXCLUDED.latest_latency_max_ms
                    ELSE current.latest_latency_max_ms
                END,
                latest_latency_mdev_ms = CASE WHEN
                    (EXCLUDED.latest_observed_at,
                     EXCLUDED.latest_observation_id)
                        > (current.latest_observed_at,
                           current.latest_observation_id)
                    THEN EXCLUDED.latest_latency_mdev_ms
                    ELSE current.latest_latency_mdev_ms
                END,
                latest_packet_loss_ratio = CASE WHEN
                    (EXCLUDED.latest_observed_at,
                     EXCLUDED.latest_observation_id)
                        > (current.latest_observed_at,
                           current.latest_observation_id)
                    THEN EXCLUDED.latest_packet_loss_ratio
                    ELSE current.latest_packet_loss_ratio
                END,
                latest_reason = CASE WHEN
                    (EXCLUDED.latest_observed_at,
                     EXCLUDED.latest_observation_id)
                        > (current.latest_observed_at,
                           current.latest_observation_id)
                    THEN EXCLUDED.latest_reason ELSE current.latest_reason
                END,
                latest_observed_at = GREATEST(
                    current.latest_observed_at, EXCLUDED.latest_observed_at
                ),
                latest_received_at = CASE WHEN
                    (EXCLUDED.latest_observed_at,
                     EXCLUDED.latest_observation_id)
                        > (current.latest_observed_at,
                           current.latest_observation_id)
                    THEN EXCLUDED.latest_received_at
                    ELSE current.latest_received_at
                END,
                updated_at = clock_timestamp()
            RETURNING series_id
        )
        SELECT count(*)::bigint FROM merged
        "#,
    )
    .bind(
        claims
            .iter()
            .map(|claim| claim.client_id.as_str())
            .collect::<Vec<_>>(),
    )
    .bind(
        claims
            .iter()
            .map(|claim| claim.after_seq)
            .collect::<Vec<_>>(),
    )
    .bind(
        claims
            .iter()
            .map(|claim| claim.through_seq)
            .collect::<Vec<_>>(),
    )
    .fetch_one(&mut **tx)
    .await?;
    u64::try_from(result.try_get::<i64, _>(0)?).map_err(Into::into)
}

async fn materialize_ping_minute(
    tx: &mut Transaction<'_, Postgres>,
    source: &[RawMinuteSample],
) -> Result<u64> {
    let mut client_ids = Vec::new();
    let mut ordinals = Vec::new();
    let mut evidence_ids = Vec::new();
    let mut observed_at = Vec::new();
    let mut target_ids = Vec::new();
    let mut generations = Vec::new();
    let mut source_checked = Vec::new();
    let mut checked = Vec::new();
    let mut statuses = Vec::new();
    let mut latency = Vec::new();
    let mut loss = Vec::new();
    let mut reasons = Vec::new();
    for sample in source {
        anyhow::ensure!(
            sample.metrics.ping_results.len() == sample.ping_source_checked_unix.len(),
            "Ping fact source identity cardinality mismatch"
        );
        for (result, source_checked_unix) in sample
            .metrics
            .ping_results
            .iter()
            .zip(&sample.ping_source_checked_unix)
        {
            client_ids.push(sample.client_id.as_str());
            ordinals.push(i64::try_from(ordinals.len()).unwrap_or(i64::MAX));
            evidence_ids.push(sample.id);
            observed_at.push(sample.observed_at);
            target_ids.push(Uuid::parse_str(result.target_id.trim())?);
            generations.push(i64::try_from(result.generation).unwrap_or(i64::MAX));
            source_checked.push(*source_checked_unix);
            checked.push(i64::try_from(result.checked_unix).unwrap_or(i64::MAX));
            statuses.push(result.status.clone());
            latency.push(result.latency_avg_ms);
            loss.push(result.loss_ratio);
            reasons.push(result.reason.clone());
        }
    }
    if ordinals.is_empty() {
        return Ok(0);
    }

    let changed = sqlx::query(
        r#"
        WITH input AS MATERIALIZED (
            SELECT *
            FROM UNNEST(
                $1::TEXT[], $2::BIGINT[], $3::UUID[], $4::TIMESTAMPTZ[],
                $5::UUID[], $6::BIGINT[], $7::BIGINT[], $8::BIGINT[],
                $9::TEXT[], $10::DOUBLE PRECISION[],
                $11::DOUBLE PRECISION[], $12::TEXT[]
            ) AS item(
                client_id, ordinal, evidence_id, observed_at,
                target_id, generation, source_checked_unix, checked_unix,
                status, latency_avg_ms, loss_ratio, reason
            )
        ), deduplicated AS MATERIALIZED (
            SELECT DISTINCT ON (
                client_id, target_id, generation, source_checked_unix
            ) *
            FROM input
            ORDER BY client_id, target_id, generation,
                     source_checked_unix, ordinal DESC
        ), changed AS (
            INSERT INTO telemetry_ping_facts (
                series_id, observed_at, evidence_id,
                source_checked_unix, checked_unix,
                status, latency_avg_ms, loss_ratio, reason
            )
            SELECT series.id, fact.observed_at, fact.evidence_id,
                   fact.source_checked_unix, fact.checked_unix,
                   fact.status, fact.latency_avg_ms, fact.loss_ratio, fact.reason
            FROM deduplicated fact
            JOIN telemetry_ping_series series
              ON series.client_id = fact.client_id
             AND series.target_id = fact.target_id
             AND series.generation = fact.generation
            ON CONFLICT (series_id, source_checked_unix) DO UPDATE SET
                evidence_id = EXCLUDED.evidence_id,
                status = EXCLUDED.status,
                latency_avg_ms = EXCLUDED.latency_avg_ms,
                loss_ratio = EXCLUDED.loss_ratio,
                reason = EXCLUDED.reason
            WHERE ROW(
                telemetry_ping_facts.status,
                telemetry_ping_facts.latency_avg_ms,
                telemetry_ping_facts.loss_ratio,
                telemetry_ping_facts.reason
            ) IS DISTINCT FROM ROW(
                EXCLUDED.status, EXCLUDED.latency_avg_ms,
                EXCLUDED.loss_ratio, EXCLUDED.reason
            )
            RETURNING series_id, checked_unix
        )
        SELECT DISTINCT series_id, checked_unix / 60 * 60 AS bucket_start_unix
        FROM changed
        ORDER BY series_id, bucket_start_unix
        "#,
    )
    .bind(&client_ids)
    .bind(&ordinals)
    .bind(&evidence_ids)
    .bind(&observed_at)
    .bind(&target_ids)
    .bind(&generations)
    .bind(&source_checked)
    .bind(&checked)
    .bind(&statuses)
    .bind(&latency)
    .bind(&loss)
    .bind(&reasons)
    .fetch_all(&mut **tx)
    .await?;
    if changed.is_empty() {
        return Ok(0);
    }
    let series_ids = changed
        .iter()
        .map(|row| row.try_get::<i64, _>("series_id"))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let bucket_starts = changed
        .iter()
        .map(|row| row.try_get::<i64, _>("bucket_start_unix"))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let result = sqlx::query(
        r#"
        WITH requested AS MATERIALIZED (
            SELECT DISTINCT series_id, bucket_start_unix
            FROM UNNEST($1::BIGINT[], $2::BIGINT[])
                AS item(series_id, bucket_start_unix)
        ), aggregated AS MATERIALIZED (
            -- Every requested coordinate came from the fact write above, so
            -- this inner join cannot discard a valid request. Read and group
            -- the complete changed coordinate set once instead of executing
            -- one correlated aggregate for every series/minute pair.
            SELECT requested.series_id, requested.bucket_start_unix,
                count(*)::integer AS sample_count,
                count(fact.latency_avg_ms)::integer AS success_count,
                sum(COALESCE(fact.latency_avg_ms, 0))::double precision
                    AS latency_sum_ms,
                avg(fact.latency_avg_ms)::double precision AS latency_avg_ms,
                min(fact.latency_avg_ms)::double precision AS latency_min_ms,
                max(fact.latency_avg_ms)::double precision AS latency_max_ms,
                avg(fact.loss_ratio)::double precision AS loss_ratio_avg,
                sum(fact.loss_ratio)::double precision AS loss_ratio_sum,
                max(fact.loss_ratio)::double precision AS loss_ratio_max,
                (array_agg(fact.status ORDER BY fact.checked_unix DESC,
                    fact.source_checked_unix DESC))[1] AS latest_status,
                (array_agg(left(fact.reason, 512)
                    ORDER BY fact.checked_unix DESC,
                    fact.source_checked_unix DESC))[1] AS latest_reason,
                max(fact.checked_unix)::bigint AS latest_checked_unix
            FROM requested
            JOIN telemetry_ping_facts fact
              ON fact.series_id = requested.series_id
             AND fact.checked_unix >= requested.bucket_start_unix
             AND fact.checked_unix < requested.bucket_start_unix + 60
            GROUP BY requested.series_id, requested.bucket_start_unix
        ), rollup_write AS (
            INSERT INTO telemetry_ping_rollups AS current (
                series_id, bucket_start, bucket_secs,
                sample_count, success_count, latency_sum_ms,
                latency_avg_ms, latency_min_ms, latency_max_ms,
                loss_ratio_avg, loss_ratio_sum, loss_ratio_max,
                latest_status, latest_reason, latest_checked_at, updated_at
            )
            SELECT series_id, to_timestamp(bucket_start_unix), 60,
                sample_count, success_count, latency_sum_ms,
                latency_avg_ms, latency_min_ms, latency_max_ms,
                loss_ratio_avg, loss_ratio_sum, loss_ratio_max,
                latest_status, latest_reason,
                to_timestamp(latest_checked_unix), clock_timestamp()
            FROM aggregated
            ON CONFLICT (series_id, bucket_secs, bucket_start) DO UPDATE SET
                sample_count = EXCLUDED.sample_count,
                success_count = EXCLUDED.success_count,
                latency_sum_ms = EXCLUDED.latency_sum_ms,
                latency_avg_ms = EXCLUDED.latency_avg_ms,
                latency_min_ms = EXCLUDED.latency_min_ms,
                latency_max_ms = EXCLUDED.latency_max_ms,
                loss_ratio_avg = EXCLUDED.loss_ratio_avg,
                loss_ratio_sum = EXCLUDED.loss_ratio_sum,
                loss_ratio_max = EXCLUDED.loss_ratio_max,
                latest_status = EXCLUDED.latest_status,
                latest_reason = EXCLUDED.latest_reason,
                latest_checked_at = EXCLUDED.latest_checked_at,
                updated_at = clock_timestamp()
            RETURNING series_id
        )
        SELECT count(*)::bigint AS rows_written
        FROM rollup_write
        "#,
    )
    .bind(&series_ids)
    .bind(&bucket_starts)
    .fetch_one(&mut **tx)
    .await?;
    Ok(result.try_get::<i64, _>("rows_written")?.max(0) as u64)
}

fn ordinal_admitted(mask: &[u8], ordinal: usize) -> bool {
    mask.get(ordinal / 8)
        .is_some_and(|byte| byte & (1_u8 << (ordinal % 8)) != 0)
}

fn traffic_minute_observations(
    source: &[RawMinuteSample],
) -> Result<Vec<TrafficMinuteObservation>> {
    let mut observations = Vec::new();
    for sample in source {
        anyhow::ensure!(
            ordinal_admission_mask_has_exact_shape(
                &sample.network_admission_mask,
                sample.metrics.networks.len(),
            ) && ordinal_admission_mask_has_exact_shape(
                &sample.tunnel_admission_mask,
                sample.metrics.tunnels.len(),
            ),
            "traffic minute source has an incomplete admission mask"
        );
        observations.extend(
            sample
                .metrics
                .networks
                .iter()
                .enumerate()
                .filter(|(ordinal, _)| ordinal_admitted(&sample.network_admission_mask, *ordinal))
                .map(|(_, network)| TrafficMinuteObservation {
                    client_id: sample.client_id.clone(),
                    bucket_start: sample.bucket_start,
                    accepted_seq: sample.accepted_seq,
                    observed_at: sample.observed_at,
                    source_kind: "host",
                    interface: network.interface.clone(),
                    rx_bytes: network.rx_bytes.min(i64::MAX as u64) as i64,
                    tx_bytes: network.tx_bytes.min(i64::MAX as u64) as i64,
                    sample_source: "agent_networks".to_string(),
                }),
        );
        observations.extend(
            sample
                .metrics
                .tunnels
                .iter()
                .enumerate()
                .filter(|(ordinal, _)| ordinal_admitted(&sample.tunnel_admission_mask, *ordinal))
                .map(|(_, tunnel)| TrafficMinuteObservation {
                    client_id: sample.client_id.clone(),
                    bucket_start: sample.bucket_start,
                    accepted_seq: sample.accepted_seq,
                    observed_at: sample.observed_at,
                    source_kind: "tunnel",
                    interface: tunnel.interface.clone(),
                    rx_bytes: tunnel.rx_bytes.min(i64::MAX as u64) as i64,
                    tx_bytes: tunnel.tx_bytes.min(i64::MAX as u64) as i64,
                    sample_source: tunnel
                        .traffic_source
                        .as_deref()
                        .filter(|source| !source.is_empty())
                        .unwrap_or("runtime_tunnel")
                        .to_string(),
                }),
        );
    }
    Ok(observations)
}

fn traffic_minute_stream_keys(
    observations: &[TrafficMinuteObservation],
) -> Vec<TrafficMinuteStreamKey> {
    let mut keys = observations
        .iter()
        .map(|observation| TrafficMinuteStreamKey {
            client_id: observation.client_id.clone(),
            bucket_start: observation.bucket_start,
            source_kind: observation.source_kind,
            interface: observation.interface.clone(),
        })
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys.dedup();
    keys
}

async fn materialize_traffic_minute(
    tx: &mut Transaction<'_, Postgres>,
    _claims: &[ClosedMinuteClaim],
    source: &[RawMinuteSample],
) -> Result<u64> {
    let observations = traffic_minute_observations(source)?;
    let keys = traffic_minute_stream_keys(&observations);

    // Establish the exact stream owners before locking them. This statement
    // is setwise over the complete minute and creates at most one bounded
    // summary row per admitted stream; raw producers never touch these rows.
    sqlx::query(
        r#"
        WITH keys AS MATERIALIZED (
            SELECT *
            FROM UNNEST($1::TEXT[], $2::TEXT[], $3::TEXT[])
                AS stream(client_id, source_kind, interface)
        )
        INSERT INTO traffic_counter_streams (client_id, source_kind, interface)
        SELECT client_id, source_kind, interface
        FROM keys
        ORDER BY client_id, source_kind, interface
        ON CONFLICT (client_id, source_kind, interface) DO NOTHING
        "#,
    )
    .bind(
        keys.iter()
            .map(|key| key.client_id.as_str())
            .collect::<Vec<_>>(),
    )
    .bind(keys.iter().map(|key| key.source_kind).collect::<Vec<_>>())
    .bind(
        keys.iter()
            .map(|key| key.interface.as_str())
            .collect::<Vec<_>>(),
    )
    .execute(&mut **tx)
    .await?;

    // Fold ordered raw counters into one closed sample per admitted stream.
    // The previous closed endpoint is one primary-key predecessor seek per
    // stream. Counter epochs, usage, and resets are derived from every raw
    // observation, while the database receives one setwise INSERT/UPSERT. The
    // transaction-local marker tells the billing consumer that it may attempt
    // its fenced append classifier for every client in this exact setwise
    // publication. Dashboard membership remains independently owned by its
    // normal trigger, including the first interface for a new client.
    sqlx::query(
        r#"
        SELECT set_config(
                   'vpsman.traffic_live_minute_publication', 'on', TRUE
               )
        "#,
    )
    .execute(&mut **tx)
    .await?;
    let (locked_streams, written): (i64, i64) = sqlx::query_as(
        r#"
        WITH observations AS MATERIALIZED (
            SELECT *
            FROM UNNEST(
                $1::TEXT[], $2::TIMESTAMPTZ[], $3::BIGINT[],
                $4::TIMESTAMPTZ[], $5::TEXT[], $6::TEXT[],
                $7::BIGINT[], $8::BIGINT[], $9::TEXT[]
            ) AS observation(
                client_id, bucket_start, accepted_seq, observed_at,
                source_kind, interface, rx_bytes, tx_bytes, sample_source
            )
        ), keys AS MATERIALIZED (
            SELECT *
            FROM UNNEST(
                $10::TEXT[], $11::TIMESTAMPTZ[], $12::TEXT[], $13::TEXT[]
            ) AS stream(client_id, bucket_start, source_kind, interface)
        ), locked AS MATERIALIZED (
            SELECT keys.client_id, keys.bucket_start,
                   keys.source_kind, keys.interface,
                   stream.latest_sample_observed_at,
                   stream.latest_sample_rx_bytes,
                   stream.latest_sample_tx_bytes,
                   stream.latest_sample_rx_counter_epoch,
                   stream.latest_sample_tx_counter_epoch,
                   stream.latest_sample_source
            FROM keys
            JOIN traffic_counter_streams stream
              ON stream.client_id = keys.client_id
             AND stream.source_kind = keys.source_kind
             AND stream.interface = keys.interface
            ORDER BY stream.client_id, stream.source_kind, stream.interface
            FOR UPDATE OF stream
        ), baseline AS MATERIALIZED (
            SELECT stream.client_id, stream.bucket_start,
                   stream.source_kind, stream.interface,
                   CASE WHEN stream.latest_sample_observed_at < stream.bucket_start
                        THEN stream.latest_sample_observed_at
                        ELSE previous.observed_at END AS observed_at,
                   CASE WHEN stream.latest_sample_observed_at < stream.bucket_start
                        THEN stream.latest_sample_rx_bytes
                        ELSE previous.rx_bytes END AS rx_bytes,
                   CASE WHEN stream.latest_sample_observed_at < stream.bucket_start
                        THEN stream.latest_sample_tx_bytes
                        ELSE previous.tx_bytes END AS tx_bytes,
                   CASE WHEN stream.latest_sample_observed_at < stream.bucket_start
                        THEN stream.latest_sample_rx_counter_epoch
                        ELSE previous.rx_counter_epoch END AS rx_counter_epoch,
                   CASE WHEN stream.latest_sample_observed_at < stream.bucket_start
                        THEN stream.latest_sample_tx_counter_epoch
                        ELSE previous.tx_counter_epoch END AS tx_counter_epoch,
                   CASE WHEN stream.latest_sample_observed_at < stream.bucket_start
                        THEN stream.latest_sample_source
                        ELSE previous.sample_source END AS sample_source
            FROM locked stream
            LEFT JOIN LATERAL (
                SELECT sample.observed_at, sample.rx_bytes, sample.tx_bytes,
                       sample.rx_counter_epoch, sample.tx_counter_epoch,
                       sample.sample_source
                FROM traffic_counter_samples sample
                WHERE sample.client_id = stream.client_id
                  AND sample.source_kind = stream.source_kind
                  AND sample.interface = stream.interface
                  AND sample.observed_at < stream.bucket_start
                ORDER BY sample.observed_at DESC
                LIMIT 1
            ) previous ON stream.latest_sample_observed_at >= stream.bucket_start
        ), windowed AS MATERIALIZED (
            SELECT observations.*,
                   row_number() OVER stream_order AS stream_ordinal,
                   lag(observations.observed_at) OVER stream_order AS lag_observed_at,
                   lag(observations.rx_bytes) OVER stream_order AS lag_rx_bytes,
                   lag(observations.tx_bytes) OVER stream_order AS lag_tx_bytes,
                   lag(observations.sample_source) OVER stream_order AS lag_sample_source,
                   baseline.observed_at AS baseline_observed_at,
                   baseline.rx_bytes AS baseline_rx_bytes,
                   baseline.tx_bytes AS baseline_tx_bytes,
                   baseline.rx_counter_epoch AS baseline_rx_counter_epoch,
                   baseline.tx_counter_epoch AS baseline_tx_counter_epoch,
                   baseline.sample_source AS baseline_sample_source
            FROM observations
            JOIN baseline USING (client_id, bucket_start, source_kind, interface)
            WINDOW stream_order AS (
                PARTITION BY observations.client_id,
                             observations.source_kind, observations.interface
                ORDER BY observations.observed_at, observations.accepted_seq
            )
        ), compared AS MATERIALIZED (
            SELECT windowed.*,
                CASE WHEN stream_ordinal = 1
                    THEN baseline_observed_at ELSE lag_observed_at END AS prior_observed_at,
                CASE WHEN stream_ordinal = 1
                    THEN baseline_rx_bytes ELSE lag_rx_bytes END AS prior_rx_bytes,
                CASE WHEN stream_ordinal = 1
                    THEN baseline_tx_bytes ELSE lag_tx_bytes END AS prior_tx_bytes,
                CASE WHEN stream_ordinal = 1
                    THEN baseline_sample_source ELSE lag_sample_source END AS prior_sample_source
            FROM windowed
        ), classified AS MATERIALIZED (
            SELECT compared.*,
                (
                    prior_observed_at IS NOT NULL
                    AND observed_at >= prior_observed_at
                    AND NOT (
                        prior_sample_source LIKE 'vnstat_import:%'
                        AND sample_source NOT LIKE 'vnstat_import:%'
                    )
                    AND rx_bytes >= prior_rx_bytes
                )::integer AS rx_valid_count,
                (
                    prior_observed_at IS NOT NULL
                    AND observed_at >= prior_observed_at
                    AND NOT (
                        prior_sample_source LIKE 'vnstat_import:%'
                        AND sample_source NOT LIKE 'vnstat_import:%'
                    )
                    AND tx_bytes >= prior_tx_bytes
                )::integer AS tx_valid_count,
                (
                    prior_observed_at IS NOT NULL
                    AND observed_at >= prior_observed_at
                    AND rx_bytes < prior_rx_bytes
                    AND NOT (
                        prior_sample_source LIKE 'vnstat_import:%'
                        AND sample_source NOT LIKE 'vnstat_import:%'
                    )
                )::integer AS rx_reset_count,
                (
                    prior_observed_at IS NOT NULL
                    AND observed_at >= prior_observed_at
                    AND tx_bytes < prior_tx_bytes
                    AND NOT (
                        prior_sample_source LIKE 'vnstat_import:%'
                        AND sample_source NOT LIKE 'vnstat_import:%'
                    )
                )::integer AS tx_reset_count
            FROM compared
        ), deltas AS MATERIALIZED (
            SELECT classified.*,
                (
                    prior_observed_at IS NOT NULL
                    AND observed_at >= prior_observed_at
                    AND (
                        rx_bytes < prior_rx_bytes
                        OR (
                            prior_sample_source LIKE 'vnstat_import:%'
                            AND sample_source NOT LIKE 'vnstat_import:%'
                        )
                    )
                )::integer AS rx_epoch_step,
                (
                    prior_observed_at IS NOT NULL
                    AND observed_at >= prior_observed_at
                    AND (
                        tx_bytes < prior_tx_bytes
                        OR (
                            prior_sample_source LIKE 'vnstat_import:%'
                            AND sample_source NOT LIKE 'vnstat_import:%'
                        )
                    )
                )::integer AS tx_epoch_step,
                CASE WHEN rx_valid_count > 0
                    THEN rx_bytes - prior_rx_bytes ELSE 0 END AS rx_usage_bytes,
                CASE WHEN tx_valid_count > 0
                    THEN tx_bytes - prior_tx_bytes ELSE 0 END AS tx_usage_bytes,
                GREATEST(rx_valid_count, tx_valid_count)::integer
                    AS any_valid_count,
                GREATEST(rx_reset_count, tx_reset_count)::integer
                    AS any_reset_count
            FROM classified
        ), epoch_values AS MATERIALIZED (
            SELECT deltas.*,
                COALESCE(baseline_rx_counter_epoch, 0)
                    + sum(rx_epoch_step) OVER stream_order AS rx_counter_epoch,
                COALESCE(baseline_tx_counter_epoch, 0)
                    + sum(tx_epoch_step) OVER stream_order AS tx_counter_epoch
            FROM deltas
            WINDOW stream_order AS (
                PARTITION BY client_id, source_kind, interface
                ORDER BY observed_at, accepted_seq
                ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
            )
        ), folded AS MATERIALIZED (
            SELECT
                client_id, bucket_start, source_kind, interface,
                count(*)::integer AS sample_count,
                sum(rx_bytes::numeric) AS rx_bytes_sum,
                sum(tx_bytes::numeric) AS tx_bytes_sum,
                (array_agg(rx_bytes ORDER BY observed_at DESC, accepted_seq DESC))[1]
                    AS rx_bytes,
                (array_agg(tx_bytes ORDER BY observed_at DESC, accepted_seq DESC))[1]
                    AS tx_bytes,
                (array_agg(rx_counter_epoch ORDER BY observed_at DESC, accepted_seq DESC))[1]
                    AS rx_counter_epoch,
                (array_agg(tx_counter_epoch ORDER BY observed_at DESC, accepted_seq DESC))[1]
                    AS tx_counter_epoch,
                (array_agg(sample_source ORDER BY observed_at DESC, accepted_seq DESC))[1]
                    AS sample_source,
                max(observed_at) AS latest_observed_at,
                sum(rx_usage_bytes)::bigint AS rx_usage_bytes,
                sum(tx_usage_bytes)::bigint AS tx_usage_bytes,
                sum(rx_valid_count)::integer AS rx_valid_count,
                sum(tx_valid_count)::integer AS tx_valid_count,
                sum(any_valid_count)::integer AS any_valid_count,
                sum(rx_reset_count)::integer AS rx_reset_count,
                sum(tx_reset_count)::integer AS tx_reset_count,
                sum(any_reset_count)::integer AS any_reset_count
            FROM epoch_values
            GROUP BY client_id, bucket_start, source_kind, interface
        ), written AS (
            INSERT INTO traffic_counter_samples AS current (
                client_id, source_kind, interface, observed_at,
                rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch,
                sample_source, inbound_promoted,
                sample_count, rx_bytes_sum, tx_bytes_sum,
                latest_observed_at,
                rx_usage_bytes, tx_usage_bytes,
                rx_valid_count, tx_valid_count, any_valid_count,
                rx_reset_count, tx_reset_count, any_reset_count,
                usage_authoritative, updated_at
            )
            SELECT client_id, source_kind, interface, bucket_start,
                rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch,
                sample_source, FALSE,
                sample_count, rx_bytes_sum, tx_bytes_sum,
                latest_observed_at,
                rx_usage_bytes, tx_usage_bytes,
                rx_valid_count, tx_valid_count, any_valid_count,
                rx_reset_count, tx_reset_count, any_reset_count,
                TRUE, clock_timestamp()
            FROM folded
            ORDER BY client_id, source_kind, interface
            ON CONFLICT (client_id, source_kind, interface, observed_at)
            DO UPDATE SET
                rx_bytes = EXCLUDED.rx_bytes,
                tx_bytes = EXCLUDED.tx_bytes,
                rx_counter_epoch = EXCLUDED.rx_counter_epoch,
                tx_counter_epoch = EXCLUDED.tx_counter_epoch,
                sample_source = EXCLUDED.sample_source,
                inbound_promoted = FALSE,
                sample_count = EXCLUDED.sample_count,
                rx_bytes_sum = EXCLUDED.rx_bytes_sum,
                tx_bytes_sum = EXCLUDED.tx_bytes_sum,
                latest_observed_at = EXCLUDED.latest_observed_at,
                rx_usage_bytes = EXCLUDED.rx_usage_bytes,
                tx_usage_bytes = EXCLUDED.tx_usage_bytes,
                rx_valid_count = EXCLUDED.rx_valid_count,
                tx_valid_count = EXCLUDED.tx_valid_count,
                any_valid_count = EXCLUDED.any_valid_count,
                rx_reset_count = EXCLUDED.rx_reset_count,
                tx_reset_count = EXCLUDED.tx_reset_count,
                any_reset_count = EXCLUDED.any_reset_count,
                usage_authoritative = TRUE,
                updated_at = EXCLUDED.updated_at
            RETURNING 1
        )
        SELECT (SELECT count(*)::bigint FROM locked) AS locked_streams,
               count(*)::bigint AS written_streams
        FROM written
        "#,
    )
    .bind(
        observations
            .iter()
            .map(|observation| observation.client_id.as_str())
            .collect::<Vec<_>>(),
    )
    .bind(
        observations
            .iter()
            .map(|observation| observation.bucket_start)
            .collect::<Vec<_>>(),
    )
    .bind(
        observations
            .iter()
            .map(|observation| observation.accepted_seq)
            .collect::<Vec<_>>(),
    )
    .bind(
        observations
            .iter()
            .map(|observation| observation.observed_at)
            .collect::<Vec<_>>(),
    )
    .bind(
        observations
            .iter()
            .map(|observation| observation.source_kind)
            .collect::<Vec<_>>(),
    )
    .bind(
        observations
            .iter()
            .map(|observation| observation.interface.as_str())
            .collect::<Vec<_>>(),
    )
    .bind(
        observations
            .iter()
            .map(|observation| observation.rx_bytes)
            .collect::<Vec<_>>(),
    )
    .bind(
        observations
            .iter()
            .map(|observation| observation.tx_bytes)
            .collect::<Vec<_>>(),
    )
    .bind(
        observations
            .iter()
            .map(|observation| observation.sample_source.as_str())
            .collect::<Vec<_>>(),
    )
    .bind(
        keys.iter()
            .map(|key| key.client_id.as_str())
            .collect::<Vec<_>>(),
    )
    .bind(keys.iter().map(|key| key.bucket_start).collect::<Vec<_>>())
    .bind(keys.iter().map(|key| key.source_kind).collect::<Vec<_>>())
    .bind(
        keys.iter()
            .map(|key| key.interface.as_str())
            .collect::<Vec<_>>(),
    )
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        SELECT set_config(
                   'vpsman.traffic_live_minute_publication', 'off', TRUE
               )
        "#,
    )
    .execute(&mut **tx)
    .await?;
    anyhow::ensure!(
        written == locked_streams,
        "traffic minute did not publish every exact stream owner"
    );
    Ok(written.max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::PgWorkerTestDb;
    use vpsman_common::{
        ConnectionStat, CpuStat, LoadAverage, MemoryStat, NetworkStat, PingTargetResult,
        RuntimeTunnelStat,
    };

    fn minute_metrics(
        observed_unix: u64,
        rx_bytes: u64,
        tx_bytes: u64,
        ping_target_id: Uuid,
        checked_unix: u64,
        latency_ms: f64,
    ) -> AgentMetrics {
        AgentMetrics {
            observed_unix,
            hostname: "minute-consumer".to_string(),
            uptime_secs: observed_unix,
            cpu: CpuStat {
                load: LoadAverage {
                    one: 1.0,
                    five: 2.0,
                    fifteen: 3.0,
                },
                cores: 2,
                utilization_ratio: Some(0.5),
            },
            memory: MemoryStat {
                total_bytes: 1_000,
                available_bytes: 400,
                swap_total_bytes: Some(100),
                swap_available_bytes: Some(40),
            },
            networks: vec![NetworkStat {
                interface: "eth0".to_string(),
                rx_bytes,
                tx_bytes,
            }],
            connections: Some(ConnectionStat { tcp: 3, udp: 4 }),
            ping_results: vec![PingTargetResult {
                target_id: ping_target_id.to_string(),
                generation: 1,
                checked_unix,
                status: "ok".to_string(),
                latency_avg_ms: Some(latency_ms),
                loss_ratio: 0.0,
                reason: None,
            }],
            ..AgentMetrics::default()
        }
    }

    fn ping_minute_sample(
        client_id: &str,
        accepted_seq: i64,
        bucket_start: DateTime<Utc>,
        observed_at: DateTime<Utc>,
        target_id: Uuid,
        checked_unix: i64,
        status: &str,
        latency_avg_ms: Option<f64>,
        loss_ratio: f64,
        reason: Option<&str>,
    ) -> RawMinuteSample {
        RawMinuteSample {
            client_id: client_id.to_string(),
            id: Uuid::new_v4(),
            accepted_seq,
            bucket_start,
            observed_at,
            metrics: AgentMetrics {
                ping_results: vec![PingTargetResult {
                    target_id: target_id.to_string(),
                    generation: 1,
                    checked_unix: u64::try_from(checked_unix).unwrap(),
                    status: status.to_string(),
                    latency_avg_ms,
                    loss_ratio,
                    reason: reason.map(str::to_string),
                }],
                ..AgentMetrics::default()
            },
            ping_source_checked_unix: vec![checked_unix],
            network_admission_mask: Vec::new(),
            tunnel_admission_mask: Vec::new(),
        }
    }

    async fn insert_raw_sample<'e, E>(
        executor: E,
        client_id: &str,
        accepted_seq: i64,
        observed_at: DateTime<Utc>,
        metrics: &AgentMetrics,
    ) where
        E: sqlx::Executor<'e, Database = Postgres>,
    {
        let all_admitted_mask = |item_count: usize| {
            let mut mask = vec![0xff; item_count.div_ceil(8)];
            let remainder = item_count % 8;
            if remainder != 0 {
                *mask.last_mut().unwrap() = (1_u8 << remainder) - 1;
            }
            mask
        };
        let network_admission_mask = all_admitted_mask(metrics.networks.len());
        let tunnel_admission_mask = all_admitted_mask(metrics.tunnels.len());
        sqlx::query(
            r#"
            INSERT INTO telemetry_samples (
                id, client_id, observed_at,
                cpu_utilization_ratio, cpu_cores,
                cpu_load_1, cpu_load_5, cpu_load_15,
                memory_total_bytes, memory_available_bytes,
                swap_total_bytes, swap_available_bytes,
                disk_total_bytes, disk_available_bytes,
                tcp_sockets, udp_sockets, payload,
                accepted_seq, accepted_at,
                source_gateway_id, source_gateway_session_id,
                source_process_incarnation_id, source_telemetry_seq,
                reported_observed_unix, ping_source_checked_unix,
                network_admission_mask, tunnel_admission_mask
            ) VALUES (
                $1, $2, $3,
                $4, $5, $6, $7, $8,
                $9, $10, $11, $12,
                1000, 500, $13, $14, $15,
                $16, clock_timestamp(),
                'minute-test-gateway', $17, $18, $16,
                $19, $20, $21, $22
            )
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(client_id)
        .bind(observed_at)
        .bind(metrics.cpu.utilization_ratio)
        .bind(i32::from(metrics.cpu.cores))
        .bind(metrics.cpu.load.one)
        .bind(metrics.cpu.load.five)
        .bind(metrics.cpu.load.fifteen)
        .bind(i64::try_from(metrics.memory.total_bytes).unwrap())
        .bind(i64::try_from(metrics.memory.available_bytes).unwrap())
        .bind(metrics.memory.swap_total_bytes.map(|value| value as i64))
        .bind(
            metrics
                .memory
                .swap_available_bytes
                .map(|value| value as i64),
        )
        .bind(metrics.connections.as_ref().unwrap().tcp as i64)
        .bind(metrics.connections.as_ref().unwrap().udp as i64)
        .bind(SqlJson(metrics))
        .bind(accepted_seq)
        .bind(Uuid::new_v4())
        .bind(Uuid::new_v4())
        .bind(observed_at.timestamp())
        .bind(
            metrics
                .ping_results
                .iter()
                .map(|ping| ping.checked_unix as i64)
                .collect::<Vec<_>>(),
        )
        .bind(network_admission_mask)
        .bind(tunnel_admission_mask)
        .execute(executor)
        .await
        .unwrap();
    }

    #[test]
    fn natural_minute_closure_is_short_and_publication_remains_setwise() {
        let source = include_str!("telemetry_minute_materialization.rs");
        let (production, _) = source
            .split_once("#[cfg(test)]\nmod tests")
            .expect("minute materialization production boundary");
        assert_eq!(production.matches("pool.begin().await?").count(), 2);
        assert!(production.contains("SELECT min(bucket_start) AS bucket_start"));
        assert!(production.contains("FOR SHARE OF projection SKIP LOCKED"));
        assert_eq!(
            production.matches("FOR UPDATE OF head SKIP LOCKED").count(),
            2
        );
        let (_, certification) = production
            .split_once("async fn certify_closed_minute")
            .expect("short producer certification boundary");
        let (certification, _) = certification
            .split_once("async fn claim_certified_minute")
            .expect("certification transaction boundary");
        assert!(certification.contains("tx.commit().await?"));
        assert!(!certification.contains("load_raw_minute"));
        assert!(!certification.contains("materialize_core_minute"));
        assert!(!certification.contains("materialize_traffic_minute"));
        assert!(!production.contains("validate_closed_minute"));
        assert!(!production.contains("for claim in claims"));
        assert_eq!(
            production
                .matches("'vpsman.traffic_live_minute_publication'")
                .count(),
            2,
            "the exact traffic statement must bracket its live owner marker"
        );
        assert!(!production.contains("'vpsman.telemetry_ownership_transfer'"));
        assert!(production
            .contains("UPDATE {head} head\n        SET materialized_seq = claims.through_seq"));
        let (_, resource) = production
            .split_once("async fn materialize_resource_minute")
            .expect("resource minute materializer");
        let (resource, _) = resource
            .split_once("async fn materialize_network_observation_minute")
            .expect("resource minute materializer boundary");
        assert!(resource.contains("INSERT INTO telemetry_rollups ("));
        assert!(!resource.contains("ON CONFLICT"));
        assert_eq!(
            production
                .matches("INSERT INTO traffic_counter_samples AS current")
                .count(),
            1,
            "the traffic minute consumer must publish every admitted stream setwise"
        );
        let (_, traffic) = production
            .split_once("async fn materialize_traffic_minute")
            .expect("traffic minute materializer");
        assert!(!traffic.contains("JOIN telemetry_samples"));
        assert!(!traffic.contains("jsonb_array_elements"));
        assert_eq!(
            traffic.matches("WITH observations AS MATERIALIZED").count(),
            1
        );
        assert_eq!(traffic.matches("FOR UPDATE OF stream").count(), 1);
        assert!(traffic.contains("FROM locked stream"));
        assert!(traffic.contains("FROM locked) AS locked_streams"));
    }

    #[test]
    fn ping_minute_aggregate_is_setwise_without_coordinate_lateral() {
        let source = include_str!("telemetry_minute_materialization.rs");
        let (production, _) = source
            .split_once("#[cfg(test)]\nmod tests")
            .expect("minute materialization production boundary");
        let (_, ping) = production
            .split_once("async fn materialize_ping_minute")
            .expect("ping minute materializer");
        let (ping, _) = ping
            .split_once("fn ordinal_admitted")
            .expect("ping minute materializer boundary");
        assert!(!ping.contains("CROSS JOIN LATERAL"));
        assert_eq!(ping.matches("JOIN telemetry_ping_facts fact").count(), 1);
        assert_eq!(
            ping.matches("GROUP BY requested.series_id, requested.bucket_start_unix")
                .count(),
            1,
            "all exact changed ping coordinates must share one setwise aggregate"
        );
        assert_eq!(
            ping.matches(
                "ORDER BY fact.checked_unix DESC,\n                    fact.source_checked_unix DESC"
            )
            .count(),
            2,
            "latest status and reason must retain their canonical tie order"
        );
        assert!(ping.contains("fact.checked_unix >= requested.bucket_start_unix"));
        assert!(ping.contains("fact.checked_unix < requested.bucket_start_unix + 60"));
    }

    #[tokio::test]
    async fn ping_minute_setwise_aggregate_preserves_corrections_and_latest_value() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let client_a = "ping-minute-setwise-a";
        let client_b = "ping-minute-setwise-b";
        insert_client(&db.pool, client_a).await;
        insert_client(&db.pool, client_b).await;
        let target_a = Uuid::new_v4();
        let target_b = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO ping_targets (id, name, host, probe_kind)
            VALUES ($1, 'ping-minute-setwise-a', '127.0.0.1', 'icmp'),
                   ($2, 'ping-minute-setwise-b', '127.0.0.2', 'icmp')
            "#,
        )
        .bind(target_a)
        .bind(target_b)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO ping_target_assignments (target_id, client_id)
            VALUES ($1, $3), ($2, $4)
            "#,
        )
        .bind(target_a)
        .bind(target_b)
        .bind(client_a)
        .bind(client_b)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO telemetry_ping_series (client_id, target_id, generation)
            VALUES ($3, $1, 1), ($4, $2, 1)
            "#,
        )
        .bind(target_a)
        .bind(target_b)
        .bind(client_a)
        .bind(client_b)
        .execute(&db.pool)
        .await
        .unwrap();

        let bucket_start: DateTime<Utc> = sqlx::query_scalar(
            "SELECT date_trunc('minute', clock_timestamp()) - interval '2 minutes'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let bucket_unix = bucket_start.timestamp();
        let source = vec![
            ping_minute_sample(
                client_a,
                1,
                bucket_start,
                bucket_start + chrono::Duration::seconds(10),
                target_a,
                bucket_unix + 5,
                "ok",
                Some(10.0),
                0.0,
                Some("first"),
            ),
            ping_minute_sample(
                client_a,
                2,
                bucket_start,
                bucket_start + chrono::Duration::seconds(50),
                target_a,
                bucket_unix + 45,
                "degraded",
                Some(30.0),
                0.5,
                Some("latest degraded"),
            ),
            ping_minute_sample(
                client_b,
                1,
                bucket_start,
                bucket_start + chrono::Duration::seconds(20),
                target_b,
                bucket_unix + 15,
                "down",
                None,
                1.0,
                Some("remote down"),
            ),
        ];
        let mut tx = db.pool.begin().await.unwrap();
        assert_eq!(materialize_ping_minute(&mut tx, &[]).await.unwrap(), 0);
        assert_eq!(materialize_ping_minute(&mut tx, &source).await.unwrap(), 2);
        tx.commit().await.unwrap();

        assert_eq!(
            sqlx::query_as::<
                _,
                (
                    String,
                    i32,
                    i32,
                    f64,
                    Option<f64>,
                    f64,
                    f64,
                    f64,
                    String,
                    Option<String>,
                    i64,
                ),
            >(
                r#"
                SELECT series.client_id, rollup.sample_count,
                       rollup.success_count, rollup.latency_sum_ms,
                       rollup.latency_avg_ms, rollup.loss_ratio_avg,
                       rollup.loss_ratio_sum, rollup.loss_ratio_max,
                       rollup.latest_status, rollup.latest_reason,
                       extract(epoch FROM rollup.latest_checked_at)::bigint
                FROM telemetry_ping_rollups rollup
                JOIN telemetry_ping_series series ON series.id=rollup.series_id
                WHERE rollup.bucket_secs=60 AND rollup.bucket_start=$1
                  AND series.client_id = ANY($2::TEXT[])
                ORDER BY series.client_id
                "#,
            )
            .bind(bucket_start)
            .bind([client_a, client_b])
            .fetch_all(&db.pool)
            .await
            .unwrap(),
            vec![
                (
                    client_a.to_string(),
                    2,
                    2,
                    40.0,
                    Some(20.0),
                    0.25,
                    0.5,
                    0.5,
                    "degraded".to_string(),
                    Some("latest degraded".to_string()),
                    bucket_unix + 45,
                ),
                (
                    client_b.to_string(),
                    1,
                    0,
                    0.0,
                    None,
                    1.0,
                    1.0,
                    1.0,
                    "down".to_string(),
                    Some("remote down".to_string()),
                    bucket_unix + 15,
                ),
            ]
        );
        assert_eq!(
            sqlx::query_as::<_, (String, Option<f64>, Option<f64>)>(
                r#"
                SELECT series.client_id, rollup.latency_min_ms,
                       rollup.latency_max_ms
                FROM telemetry_ping_rollups rollup
                JOIN telemetry_ping_series series ON series.id=rollup.series_id
                WHERE rollup.bucket_secs=60 AND rollup.bucket_start=$1
                  AND series.client_id = ANY($2::TEXT[])
                ORDER BY series.client_id
                "#,
            )
            .bind(bucket_start)
            .bind([client_a, client_b])
            .fetch_all(&db.pool)
            .await
            .unwrap(),
            vec![
                (client_a.to_string(), Some(10.0), Some(30.0)),
                (client_b.to_string(), None, None),
            ]
        );

        let corrected = vec![ping_minute_sample(
            client_a,
            2,
            bucket_start,
            bucket_start + chrono::Duration::seconds(50),
            target_a,
            bucket_unix + 45,
            "down",
            None,
            1.0,
            Some("corrected down"),
        )];
        let mut tx = db.pool.begin().await.unwrap();
        assert_eq!(
            materialize_ping_minute(&mut tx, &corrected).await.unwrap(),
            1
        );
        assert_eq!(
            materialize_ping_minute(&mut tx, &corrected).await.unwrap(),
            0
        );
        tx.commit().await.unwrap();

        assert_eq!(
            sqlx::query_as::<_, (i64, i32, i32, f64, Option<f64>, f64, String, Option<String>,)>(
                r#"
                SELECT count(*) OVER (), rollup.sample_count,
                       rollup.success_count, rollup.latency_sum_ms,
                       rollup.latency_avg_ms, rollup.loss_ratio_sum,
                       rollup.latest_status, rollup.latest_reason
                FROM telemetry_ping_rollups rollup
                JOIN telemetry_ping_series series ON series.id=rollup.series_id
                WHERE rollup.bucket_secs=60 AND rollup.bucket_start=$1
                  AND series.client_id = ANY($2::TEXT[])
                ORDER BY series.client_id
                LIMIT 1
                "#,
            )
            .bind(bucket_start)
            .bind([client_a, client_b])
            .fetch_one(&db.pool)
            .await
            .unwrap(),
            (
                2,
                2,
                1,
                10.0,
                Some(10.0),
                1.0,
                "down".to_string(),
                Some("corrected down".to_string()),
            )
        );
        db.cleanup().await;
    }

    #[test]
    fn traffic_minute_source_is_flattened_once_with_exact_masks_and_names() {
        let bucket_start = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let observed_at = bucket_start + chrono::Duration::seconds(10);
        let metrics = AgentMetrics {
            networks: vec![
                NetworkStat {
                    interface: "eth0".to_string(),
                    rx_bytes: u64::MAX,
                    tx_bytes: 20,
                },
                NetworkStat {
                    interface: "docker0".to_string(),
                    rx_bytes: 30,
                    tx_bytes: 40,
                },
            ],
            tunnels: vec![
                RuntimeTunnelStat {
                    interface: "wg0".to_string(),
                    rx_bytes: 50,
                    tx_bytes: 60,
                    traffic_source: Some("vnstat_import:hourly".to_string()),
                    ..RuntimeTunnelStat::default()
                },
                RuntimeTunnelStat {
                    interface: "wg1".to_string(),
                    rx_bytes: 70,
                    tx_bytes: u64::MAX,
                    traffic_source: Some(String::new()),
                    ..RuntimeTunnelStat::default()
                },
            ],
            ..AgentMetrics::default()
        };
        let sample = RawMinuteSample {
            client_id: "flatten-client".to_string(),
            id: Uuid::new_v4(),
            accepted_seq: 7,
            bucket_start,
            observed_at,
            metrics,
            ping_source_checked_unix: Vec::new(),
            network_admission_mask: vec![0x01],
            tunnel_admission_mask: vec![0x03],
        };

        let observations = traffic_minute_observations(&[sample]).unwrap();
        assert_eq!(observations.len(), 3);
        assert_eq!(
            observations
                .iter()
                .map(|observation| (
                    observation.accepted_seq,
                    observation.source_kind,
                    observation.interface.as_str(),
                    observation.rx_bytes,
                    observation.tx_bytes,
                    observation.sample_source.as_str(),
                ))
                .collect::<Vec<_>>(),
            vec![
                (7, "host", "eth0", i64::MAX, 20, "agent_networks"),
                (7, "tunnel", "wg0", 50, 60, "vnstat_import:hourly"),
                (7, "tunnel", "wg1", 70, i64::MAX, "runtime_tunnel"),
            ]
        );
        assert!(observations
            .iter()
            .all(|observation| observation.bucket_start == bucket_start
                && observation.observed_at == observed_at));
        assert_eq!(traffic_minute_stream_keys(&observations).len(), 3);
    }

    async fn insert_client(pool: &PgPool, client_id: &str) {
        sqlx::query(
            "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1, $1, decode('', 'hex'), 'online')",
        )
        .bind(client_id)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn advance_projection(pool: &PgPool, client_ids: &[&str], projected_seq: i64) {
        sqlx::query(
            r#"
            UPDATE telemetry_projection_heads
            SET accepted_seq=$2, projected_seq=$2, projected_at=clock_timestamp()
            WHERE client_id = ANY($1::TEXT[])
            "#,
        )
        .bind(client_ids)
        .bind(projected_seq)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn closed_minute_contention_isolates_peer_then_retries_exact_suffix() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let stable_client = "natural-minute-stable-peer";
        let racing_client = "natural-minute-producer-race";
        insert_client(&db.pool, stable_client).await;
        insert_client(&db.pool, racing_client).await;
        let ping_target_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO ping_targets (id, name, host, probe_kind)
            VALUES ($1, 'minute-producer-race', '127.0.0.1', 'icmp')
            "#,
        )
        .bind(ping_target_id)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO ping_target_assignments (target_id, client_id) VALUES ($1, $2), ($1, $3)",
        )
        .bind(ping_target_id)
        .bind(stable_client)
        .bind(racing_client)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO telemetry_ping_series (client_id, target_id, generation) VALUES ($2, $1, 1), ($3, $1, 1)",
        )
        .bind(ping_target_id)
        .bind(stable_client)
        .bind(racing_client)
        .execute(&db.pool)
        .await
        .unwrap();

        let bucket_start: DateTime<Utc> = sqlx::query_scalar(
            "SELECT date_trunc('minute', clock_timestamp()) - interval '2 minutes'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let first_at = bucket_start + chrono::Duration::seconds(10);
        let second_at = bucket_start + chrono::Duration::seconds(40);
        let first = minute_metrics(
            first_at.timestamp() as u64,
            100,
            200,
            ping_target_id,
            first_at.timestamp() as u64,
            10.0,
        );
        let second = minute_metrics(
            second_at.timestamp() as u64,
            150,
            260,
            ping_target_id,
            second_at.timestamp() as u64,
            20.0,
        );
        insert_raw_sample(&db.pool, stable_client, 1, first_at, &first).await;
        insert_raw_sample(&db.pool, racing_client, 1, first_at, &first).await;
        advance_projection(&db.pool, &[stable_client, racing_client], 1).await;

        // Model one acceptance that already owns the racing client's producer
        // fence and chose another sample in the same closed minute. The stable
        // peer must still publish in both independent consumers.
        let mut acceptance = db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM clients WHERE id=$1 FOR NO KEY UPDATE")
            .bind(racing_client)
            .fetch_one(&mut *acceptance)
            .await
            .unwrap();
        sqlx::query(
            r#"
            UPDATE telemetry_projection_heads
            SET accepted_seq=2,
                accepted_at=GREATEST(accepted_at, clock_timestamp())
            WHERE client_id=$1
            "#,
        )
        .bind(racing_client)
        .execute(&mut *acceptance)
        .await
        .unwrap();
        insert_raw_sample(&mut *acceptance, racing_client, 2, second_at, &second).await;

        assert!(
            telemetry_minute_consumer_has_ready_work(&db.pool, TelemetryMinuteConsumer::Core)
                .await
                .unwrap()
        );
        let core_peer = materialize_next_telemetry_minute(&db.pool, TelemetryMinuteConsumer::Core)
            .await
            .unwrap();
        assert_eq!(core_peer.source_rows, 1);
        assert!(core_peer.owner_contended);
        let traffic_peer =
            materialize_next_telemetry_minute(&db.pool, TelemetryMinuteConsumer::Traffic)
                .await
                .unwrap();
        assert_eq!(traffic_peer.source_rows, 1);
        assert!(traffic_peer.owner_contended);
        assert_eq!(
            sqlx::query_as::<_, (String, i64, i64, i64, i64, i64)>(
                r#"
                SELECT client.id, core.materialized_seq, traffic.materialized_seq,
                       (SELECT count(*) FROM telemetry_rollups
                        WHERE client_id=client.id AND bucket_secs=60
                          AND bucket_start=$2),
                       (SELECT count(*) FROM telemetry_ping_facts fact
                        JOIN telemetry_ping_series series ON series.id=fact.series_id
                        WHERE series.client_id=client.id),
                       (SELECT count(*) FROM traffic_counter_samples sample
                        WHERE sample.client_id=client.id
                          AND sample.source_kind='host'
                          AND sample.interface='eth0'
                          AND sample.observed_at=$2)
                FROM clients client
                JOIN telemetry_minute_materialization_heads core
                  ON core.client_id=client.id
                JOIN traffic_counter_minute_heads traffic
                  ON traffic.client_id=client.id
                WHERE client.id = ANY($1::TEXT[])
                ORDER BY client.id
                "#,
            )
            .bind([racing_client, stable_client])
            .bind(bucket_start)
            .fetch_all(&db.pool)
            .await
            .unwrap(),
            vec![
                (racing_client.to_string(), 0, 0, 0, 0, 0),
                (stable_client.to_string(), 1, 1, 1, 1, 1),
            ],
            "one producer owner widened contention to its stable peer"
        );

        acceptance.commit().await.unwrap();
        assert!(
            !telemetry_minute_consumer_has_ready_work(&db.pool, TelemetryMinuteConsumer::Core)
                .await
                .unwrap(),
            "an accepted but unprojected suffix became ready"
        );
        advance_projection(&db.pool, &[racing_client], 2).await;

        let core = materialize_next_telemetry_minute(&db.pool, TelemetryMinuteConsumer::Core)
            .await
            .unwrap();
        assert_eq!(core.source_rows, 2);
        assert!(!core.owner_contended);
        let traffic = materialize_next_telemetry_minute(&db.pool, TelemetryMinuteConsumer::Traffic)
            .await
            .unwrap();
        assert_eq!(traffic.source_rows, 2);
        assert!(!traffic.owner_contended);
        assert_eq!(
            sqlx::query_as::<_, (i64, i32, i32, i64, i64, i64, i64, i32)>(
                r#"
                SELECT core.materialized_seq,
                       resource.sample_count,
                       ping.sample_count,
                       traffic.rx_bytes_sum::bigint,
                       traffic.tx_bytes_sum::bigint,
                       traffic.rx_usage_bytes,
                       traffic.tx_usage_bytes,
                       traffic.sample_count
                FROM telemetry_minute_materialization_heads core
                JOIN telemetry_rollups resource
                  ON resource.client_id=core.client_id
                 AND resource.bucket_secs=60 AND resource.bucket_start=$2
                JOIN telemetry_ping_series series ON series.client_id=core.client_id
                JOIN telemetry_ping_rollups ping ON ping.series_id=series.id
                 AND ping.bucket_secs=60 AND ping.bucket_start=$2
                JOIN traffic_counter_samples traffic
                  ON traffic.client_id=core.client_id
                 AND traffic.source_kind='host' AND traffic.interface='eth0'
                 AND traffic.observed_at=$2
                WHERE core.client_id=$1
                "#,
            )
            .bind(racing_client)
            .bind(bucket_start)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
            (2, 2, 2, 250, 460, 50, 60, 2)
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT materialized_seq FROM traffic_counter_minute_heads WHERE client_id=$1",
            )
            .bind(racing_client)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
            2
        );

        for consumer in [
            TelemetryMinuteConsumer::Core,
            TelemetryMinuteConsumer::Traffic,
        ] {
            let idle = materialize_next_telemetry_minute(&db.pool, consumer)
                .await
                .unwrap();
            assert_eq!((idle.source_rows, idle.derived_rows), (0, 0));
        }
        db.cleanup().await;
    }

    #[tokio::test]
    async fn traffic_minute_derive_once_preserves_host_and_vnstat_transition_folds() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let client_id = "traffic-minute-derived-once";
        insert_client(&db.pool, client_id).await;
        let bucket_start: DateTime<Utc> = sqlx::query_scalar(
            "SELECT date_trunc('minute', clock_timestamp()) - interval '2 minutes'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let ping_target_id = Uuid::new_v4();
        for (sample_offset, seconds) in [10_i64, 40_i64].into_iter().enumerate() {
            let observed_at = bucket_start + chrono::Duration::seconds(seconds);
            let mut metrics = minute_metrics(
                observed_at.timestamp() as u64,
                100 + sample_offset as u64 * 50,
                200 + sample_offset as u64 * 60,
                ping_target_id,
                observed_at.timestamp() as u64,
                5.0,
            );
            metrics.tunnels.push(RuntimeTunnelStat {
                interface: "wg0".to_string(),
                rx_bytes: 1_000 + sample_offset as u64 * 100,
                tx_bytes: 2_000 + sample_offset as u64 * 200,
                traffic_source: (sample_offset == 0).then(|| "vnstat_import:hourly".to_string()),
                ..RuntimeTunnelStat::default()
            });
            insert_raw_sample(
                &db.pool,
                client_id,
                sample_offset as i64 + 1,
                observed_at,
                &metrics,
            )
            .await;
        }
        advance_projection(&db.pool, &[client_id], 2).await;

        let run = materialize_next_telemetry_minute(&db.pool, TelemetryMinuteConsumer::Traffic)
            .await
            .unwrap();
        assert_eq!(run.source_rows, 2);
        assert_eq!(run.derived_rows, 2);
        assert_eq!(
            sqlx::query_as::<_, (String, String, i64, i64, i64, i64, String, i32)>(
                r#"
                SELECT source_kind, interface,
                       rx_counter_epoch, tx_counter_epoch,
                       rx_usage_bytes, tx_usage_bytes,
                       sample_source, sample_count
                FROM traffic_counter_samples
                WHERE client_id=$1 AND observed_at=$2
                ORDER BY source_kind, interface
                "#,
            )
            .bind(client_id)
            .bind(bucket_start)
            .fetch_all(&db.pool)
            .await
            .unwrap(),
            vec![
                (
                    "host".to_string(),
                    "eth0".to_string(),
                    0,
                    0,
                    50,
                    60,
                    "agent_networks".to_string(),
                    2,
                ),
                (
                    "tunnel".to_string(),
                    "wg0".to_string(),
                    1,
                    1,
                    0,
                    0,
                    "runtime_tunnel".to_string(),
                    2,
                ),
            ]
        );
        db.cleanup().await;
    }

    #[tokio::test]
    async fn natural_minute_coordinate_is_setwise_atomic_and_excludes_open_samples() {
        let Some(db) = PgWorkerTestDb::maybe_new().await else {
            return;
        };
        let closed_clients = ["natural-minute-a", "natural-minute-b"];
        let open_client = "natural-minute-open";
        for client_id in closed_clients.iter().copied().chain([open_client]) {
            insert_client(&db.pool, client_id).await;
        }
        let ping_target_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO ping_targets (id, name, host, probe_kind) VALUES ($1, 'minute-consumer', '127.0.0.1', 'icmp')",
        )
        .bind(ping_target_id)
        .execute(&db.pool)
        .await
        .unwrap();
        let assigned_clients = closed_clients
            .iter()
            .copied()
            .chain([open_client])
            .collect::<Vec<_>>();
        sqlx::query(
            r#"
            INSERT INTO ping_target_assignments (target_id, client_id)
            SELECT $1, client_id
            FROM UNNEST($2::TEXT[]) AS installed(client_id)
            "#,
        )
        .bind(ping_target_id)
        .bind(&assigned_clients)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO telemetry_ping_series (client_id, target_id, generation)
            SELECT client_id, $1, 1
            FROM UNNEST($2::TEXT[]) AS projected(client_id)
            "#,
        )
        .bind(ping_target_id)
        .bind(&assigned_clients)
        .execute(&db.pool)
        .await
        .unwrap();
        let bucket_start: DateTime<Utc> = sqlx::query_scalar(
            "SELECT date_trunc('minute', clock_timestamp()) - interval '2 minutes'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        for (client_offset, client_id) in closed_clients.iter().enumerate() {
            for (sample_offset, seconds) in [10_i64, 40_i64].into_iter().enumerate() {
                let observed_at = bucket_start + chrono::Duration::seconds(seconds);
                let counter_base = 100 + (client_offset as u64 * 1_000);
                let counter_step = sample_offset as u64 * 50;
                let mut metrics = minute_metrics(
                    observed_at.timestamp() as u64,
                    counter_base + counter_step,
                    counter_base * 2 + counter_step + sample_offset as u64 * 10,
                    ping_target_id,
                    observed_at.timestamp() as u64,
                    10.0 + sample_offset as f64 * 10.0,
                );
                if client_offset == 0 {
                    let eth0 = metrics.networks[0].clone();
                    metrics.networks = (0..8)
                        .map(|interface_number| {
                            if interface_number == 0 {
                                return eth0.clone();
                            }
                            let resets = interface_number < 4;
                            let low = 10 + interface_number as u64;
                            let high = 1_000 + interface_number as u64;
                            let rx_bytes = if resets == (sample_offset == 0) {
                                high
                            } else {
                                low
                            };
                            NetworkStat {
                                interface: format!("eth{interface_number}"),
                                rx_bytes,
                                tx_bytes: if interface_number == 3 {
                                    500 + sample_offset as u64 * 25
                                } else {
                                    rx_bytes * 2
                                },
                            }
                        })
                        .collect();
                }
                insert_raw_sample(
                    &db.pool,
                    client_id,
                    sample_offset as i64 + 1,
                    observed_at,
                    &metrics,
                )
                .await;
            }
        }
        advance_projection(&db.pool, &closed_clients, 2).await;

        let open_at: DateTime<Utc> = sqlx::query_scalar(
            "SELECT date_trunc('minute', clock_timestamp()) + interval '5 seconds'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        insert_raw_sample(
            &db.pool,
            open_client,
            1,
            open_at,
            &minute_metrics(
                open_at.timestamp() as u64,
                10,
                20,
                ping_target_id,
                open_at.timestamp() as u64,
                5.0,
            ),
        )
        .await;
        advance_projection(&db.pool, &[open_client], 1).await;

        let core = materialize_next_telemetry_minute(&db.pool, TelemetryMinuteConsumer::Core)
            .await
            .unwrap();
        assert_eq!(core.source_rows, 4);
        assert_eq!(
            sqlx::query_as::<_, (i64, i64, i64, i64)>(
                r#"
                SELECT min(head.materialized_seq), max(head.materialized_seq),
                       count(DISTINCT rollup.client_id),
                       (SELECT count(*) FROM telemetry_ping_facts)
                FROM telemetry_minute_materialization_heads head
                JOIN telemetry_rollups rollup ON rollup.client_id=head.client_id
                  AND rollup.bucket_secs=60 AND rollup.bucket_start=$2
                WHERE head.client_id = ANY($1::TEXT[])
                "#,
            )
            .bind(closed_clients)
            .bind(bucket_start)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
            (2, 2, 2, 4)
        );

        let traffic = materialize_next_telemetry_minute(&db.pool, TelemetryMinuteConsumer::Traffic)
            .await
            .unwrap();
        assert_eq!(traffic.source_rows, 4);
        assert_eq!(
            sqlx::query_as::<_, (String, i64, i32, i64, i64, i64, i64)>(
                r#"
                SELECT head.client_id, head.materialized_seq, sample.sample_count,
                       sample.rx_bytes_sum::bigint, sample.tx_bytes_sum::bigint,
                       sample.rx_usage_bytes, sample.tx_usage_bytes
                FROM traffic_counter_minute_heads head
                JOIN traffic_counter_samples sample ON sample.client_id=head.client_id
                  AND sample.source_kind='host' AND sample.interface='eth0'
                  AND sample.observed_at=$2
                WHERE head.client_id = ANY($1::TEXT[])
                ORDER BY head.client_id
                "#,
            )
            .bind(closed_clients)
            .bind(bucket_start)
            .fetch_all(&db.pool)
            .await
            .unwrap(),
            vec![
                ("natural-minute-a".to_string(), 2, 2, 250, 460, 50, 60),
                ("natural-minute-b".to_string(), 2, 2, 2_250, 4_460, 50, 60,),
            ]
        );
        assert_eq!(
            sqlx::query_as::<_, (String, i64, i64, i64, i64, i32, i32, i32, i32, i32, i32)>(
                r#"
                SELECT interface, rx_counter_epoch, tx_counter_epoch,
                       rx_usage_bytes, tx_usage_bytes,
                       rx_valid_count, tx_valid_count, any_valid_count,
                       rx_reset_count, tx_reset_count, any_reset_count
                FROM traffic_counter_samples
                WHERE client_id='natural-minute-a'
                  AND source_kind='host' AND observed_at=$1
                ORDER BY interface
                "#,
            )
            .bind(bucket_start)
            .fetch_all(&db.pool)
            .await
            .unwrap(),
            (0..8)
                .map(|interface_number| {
                    let rx_reset = i64::from((1..4).contains(&interface_number));
                    let tx_reset = i64::from((1..3).contains(&interface_number));
                    let rx_usage = match interface_number {
                        0 => 50,
                        1..=3 => 0,
                        _ => 990,
                    };
                    let tx_usage = match interface_number {
                        0 => 60,
                        1..=2 => 0,
                        3 => 25,
                        _ => 1_980,
                    };
                    (
                        format!("eth{interface_number}"),
                        rx_reset,
                        tx_reset,
                        rx_usage,
                        tx_usage,
                        i32::from(rx_reset == 0),
                        i32::from(tx_reset == 0),
                        i32::from(rx_reset == 0 || tx_reset == 0),
                        rx_reset as i32,
                        tx_reset as i32,
                        i32::from(rx_reset > 0 || tx_reset > 0),
                    )
                })
                .collect::<Vec<_>>(),
            "one setwise traffic-minute publication lost an interface or reset epoch"
        );

        assert!(
            !telemetry_minute_consumer_has_ready_work(&db.pool, TelemetryMinuteConsumer::Core)
                .await
                .unwrap()
        );
        assert!(!telemetry_minute_consumer_has_ready_work(
            &db.pool,
            TelemetryMinuteConsumer::Traffic
        )
        .await
        .unwrap());
        assert_eq!(
            sqlx::query_as::<_, (i64, i64)>(
                r#"
                SELECT core.materialized_seq, traffic.materialized_seq
                FROM telemetry_minute_materialization_heads core
                JOIN traffic_counter_minute_heads traffic USING (client_id)
                WHERE client_id=$1
                "#,
            )
            .bind(open_client)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
            (0, 0)
        );

        let failed_bucket = bucket_start + chrono::Duration::minutes(1);
        for client_id in closed_clients {
            for (sample_offset, seconds) in [10_i64, 40_i64].into_iter().enumerate() {
                let observed_at = failed_bucket + chrono::Duration::seconds(seconds);
                let mut metrics = minute_metrics(
                    observed_at.timestamp() as u64,
                    200 + sample_offset as u64 * 50,
                    400 + sample_offset as u64 * 60,
                    ping_target_id,
                    observed_at.timestamp() as u64,
                    12.0,
                );
                if client_id == "natural-minute-b" && sample_offset == 1 {
                    metrics.ping_results[0].target_id = "invalid-target-id".to_string();
                }
                insert_raw_sample(
                    &db.pool,
                    client_id,
                    sample_offset as i64 + 3,
                    observed_at,
                    &metrics,
                )
                .await;
            }
        }
        advance_projection(&db.pool, &closed_clients, 4).await;
        assert!(
            materialize_next_telemetry_minute(&db.pool, TelemetryMinuteConsumer::Core)
                .await
                .is_err()
        );
        assert_eq!(
            sqlx::query_as::<_, (i64, i64, i64)>(
                r#"
                SELECT min(materialized_seq), max(materialized_seq),
                       (SELECT count(*) FROM telemetry_rollups
                        WHERE client_id = ANY($1::TEXT[])
                          AND bucket_secs=60 AND bucket_start=$2)
                FROM telemetry_minute_materialization_heads
                WHERE client_id = ANY($1::TEXT[])
                "#,
            )
            .bind(closed_clients)
            .bind(failed_bucket)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
            (2, 2, 0)
        );
        db.cleanup().await;
    }
}
