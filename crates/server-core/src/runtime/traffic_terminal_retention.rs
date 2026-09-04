use anyhow::{ensure, Result};
use sqlx::{postgres::PgRow, PgPool, Postgres, Row, Transaction};

const ROLLUP_BUCKET_WIDTHS: [i32; 4] = [3_600, 10_800, 21_600, 86_400];
const TRAFFIC_TERMINAL_RETENTION_CUTOFF_SQL: &str = r#"
    SELECT EXTRACT(EPOCH FROM (
        (
            date_trunc('day', clock_timestamp() AT TIME ZONE 'UTC')
            - make_interval(days => $1)
        ) AT TIME ZONE 'UTC'
    ))::bigint
"#;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TrafficTerminalRetentionPage {
    pub attempted: bool,
    pub pruned_rows: u64,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct TrafficStreamKey {
    client_id: String,
    source_kind: String,
    interface: String,
}

#[derive(Clone, Debug)]
struct TrafficTerminalCursor {
    stream: Option<TrafficStreamKey>,
    lane: Option<String>,
    scan_after_unix: Option<i64>,
}

#[derive(Clone, Debug)]
struct TrafficPruneCandidate {
    stream: TrafficStreamKey,
    origin_kind: String,
    bucket_secs: i32,
    bucket_start_unix: i64,
}

#[derive(Clone, Debug)]
struct TrafficPruneReservation {
    candidate: TrafficPruneCandidate,
    work_available: bool,
}

pub async fn traffic_terminal_retention_cutoff_unix(
    pool: &PgPool,
    retention_days: i32,
) -> Result<i64> {
    ensure!(
        retention_days >= 1,
        "traffic retention days must be positive"
    );
    Ok(sqlx::query_scalar(TRAFFIC_TERMINAL_RETENTION_CUTOFF_SQL)
        .bind(retention_days)
        .fetch_one(pool)
        .await?)
}

pub async fn preview_traffic_terminal_retention(
    pool: &PgPool,
    cutoff_unix: i64,
    limit: i32,
) -> Result<i64> {
    validate_request(cutoff_unix, limit)?;
    Ok(sqlx::query_scalar(preview_sql())
        .bind(cutoff_unix)
        .bind(i64::from(limit))
        .fetch_one(pool)
        .await?)
}

pub async fn process_traffic_terminal_retention_page(
    pool: &PgPool,
    cutoff_unix: i64,
    limit: i32,
) -> Result<TrafficTerminalRetentionPage> {
    validate_request(cutoff_unix, limit)?;
    let Some(reservation) = reserve_candidate(pool, cutoff_unix).await? else {
        return Ok(TrafficTerminalRetentionPage::default());
    };
    if !reservation.work_available {
        return Ok(TrafficTerminalRetentionPage {
            attempted: true,
            pruned_rows: 0,
        });
    }
    let candidate = reservation.candidate;

    // Discovery has committed its short singleton-frontier turn. Terminal
    // deletion now owns only this exact client and its source rows, so another
    // replica can discover an independent stream without waiting for the page.
    let mut tx = pool.begin().await?;
    if !try_lock_client_row_then_traffic(&mut tx, &candidate.stream.client_id).await? {
        // No source was consumed. It remains durably due and the keyset wrap
        // will revisit it after independent streams get a discovery turn.
        tx.rollback().await?;
        return Ok(TrafficTerminalRetentionPage {
            attempted: true,
            pruned_rows: 0,
        });
    }

    let per_lane_limit = i64::from(limit).saturating_add(7) / 8;
    let pruned_rows = sqlx::query(prune_sql())
        .bind(&candidate.stream.client_id)
        .bind(vec![candidate.stream.source_kind.as_str()])
        .bind(vec![candidate.stream.interface.as_str()])
        .bind(cutoff_unix)
        .bind(i64::from(limit))
        .bind(per_lane_limit.max(1))
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(TrafficTerminalRetentionPage {
        attempted: true,
        pruned_rows,
    })
}

pub async fn traffic_terminal_retention_has_remaining_work(
    pool: &PgPool,
    cutoff_unix: i64,
) -> Result<bool> {
    ensure!(
        cutoff_unix >= 0,
        "traffic retention cutoff must not be negative"
    );
    let mut tx = pool.begin().await?;
    let cursor = read_cursor(&mut tx).await?;
    let remaining = find_candidate(&mut tx, &cursor, cutoff_unix, &[])
        .await?
        .is_some();
    tx.rollback().await?;
    Ok(remaining)
}

fn validate_request(cutoff_unix: i64, limit: i32) -> Result<()> {
    ensure!(
        cutoff_unix >= 0,
        "traffic retention cutoff must not be negative"
    );
    ensure!(
        (1..=100_000).contains(&limit),
        "traffic retention limit is out of range"
    );
    Ok(())
}

async fn read_cursor(tx: &mut Transaction<'_, Postgres>) -> Result<TrafficTerminalCursor> {
    let row = sqlx::query(
        r#"
        SELECT traffic_client_id, traffic_source_kind, traffic_interface,
               traffic_lane,
               EXTRACT(EPOCH FROM traffic_scan_after)::bigint AS scan_after_unix
        FROM traffic_history_retention_cursors
        WHERE domain = 'traffic_counter_samples'
          AND source_bucket_secs = 0
          AND destination_bucket_secs = 0
        "#,
    )
    .fetch_one(&mut **tx)
    .await?;
    cursor_from_row(row)
}

async fn lock_cursor(tx: &mut Transaction<'_, Postgres>) -> Result<TrafficTerminalCursor> {
    let row = sqlx::query(
        r#"
        SELECT traffic_client_id, traffic_source_kind, traffic_interface,
               traffic_lane,
               EXTRACT(EPOCH FROM traffic_scan_after)::bigint AS scan_after_unix
        FROM traffic_history_retention_cursors
        WHERE domain = 'traffic_counter_samples'
          AND source_bucket_secs = 0
          AND destination_bucket_secs = 0
        FOR UPDATE
        "#,
    )
    .fetch_one(&mut **tx)
    .await?;
    cursor_from_row(row)
}

fn cursor_from_row(row: PgRow) -> Result<TrafficTerminalCursor> {
    let client_id = row.try_get::<Option<String>, _>("traffic_client_id")?;
    let source_kind = row.try_get::<Option<String>, _>("traffic_source_kind")?;
    let interface = row.try_get::<Option<String>, _>("traffic_interface")?;
    let lane = row.try_get::<Option<String>, _>("traffic_lane")?;
    let scan_after_unix = row.try_get::<Option<i64>, _>("scan_after_unix")?;
    let stream = match (client_id, source_kind, interface) {
        (Some(client_id), Some(source_kind), Some(interface)) => Some(TrafficStreamKey {
            client_id,
            source_kind,
            interface,
        }),
        (None, None, None) => None,
        _ => anyhow::bail!("traffic terminal cursor has an invalid partial stream key"),
    };
    ensure!(
        stream.is_some() == lane.is_some() && lane.is_some() == scan_after_unix.is_some(),
        "traffic terminal cursor has an invalid partial frontier"
    );
    Ok(TrafficTerminalCursor {
        stream,
        lane,
        scan_after_unix,
    })
}

async fn reserve_candidate(
    pool: &PgPool,
    cutoff_unix: i64,
) -> Result<Option<TrafficPruneReservation>> {
    let mut tx = pool.begin().await?;
    let cursor = lock_cursor(&mut tx).await?;
    let mut unavailable_clients = Vec::new();
    let mut last_unavailable = None;
    let (candidate, work_available) = loop {
        let Some(candidate) =
            find_candidate(&mut tx, &cursor, cutoff_unix, &unavailable_clients).await?
        else {
            let Some(candidate) = last_unavailable else {
                clear_cursor_if_positioned(&mut tx, &cursor).await?;
                tx.commit().await?;
                return Ok(None);
            };
            break (candidate, false);
        };
        if try_lock_client_row_then_traffic(&mut tx, &candidate.stream.client_id).await? {
            break (candidate, true);
        }
        unavailable_clients.push(candidate.stream.client_id.clone());
        last_unavailable = Some(candidate);
    };
    let lane = prune_lane(candidate.bucket_secs, &candidate.origin_kind)?;
    update_cursor(
        &mut tx,
        Some(&candidate.stream),
        Some(lane),
        Some(candidate.bucket_start_unix),
    )
    .await?;
    tx.commit().await?;
    Ok(Some(TrafficPruneReservation {
        candidate,
        work_available,
    }))
}

async fn update_cursor(
    tx: &mut Transaction<'_, Postgres>,
    stream: Option<&TrafficStreamKey>,
    lane: Option<&str>,
    scan_after_unix: Option<i64>,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE traffic_history_retention_cursors
        SET traffic_client_id = $1,
            traffic_source_kind = $2,
            traffic_interface = $3,
            traffic_lane = $4,
            traffic_frontier_start = NULL,
            traffic_scan_after = to_timestamp($5::double precision),
            updated_at = clock_timestamp()
        WHERE domain = 'traffic_counter_samples'
          AND source_bucket_secs = 0
          AND destination_bucket_secs = 0
        "#,
    )
    .bind(stream.map(|value| value.client_id.as_str()))
    .bind(stream.map(|value| value.source_kind.as_str()))
    .bind(stream.map(|value| value.interface.as_str()))
    .bind(lane)
    .bind(scan_after_unix)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn clear_cursor_if_positioned(
    tx: &mut Transaction<'_, Postgres>,
    cursor: &TrafficTerminalCursor,
) -> Result<()> {
    if cursor.stream.is_some() {
        update_cursor(tx, None, None, None).await?;
    }
    Ok(())
}

fn candidate_from_row(row: PgRow) -> Result<TrafficPruneCandidate> {
    Ok(TrafficPruneCandidate {
        stream: TrafficStreamKey {
            client_id: row.try_get("client_id")?,
            source_kind: row.try_get("source_kind")?,
            interface: row.try_get("interface")?,
        },
        origin_kind: row.try_get("origin_kind")?,
        bucket_secs: row.try_get("bucket_secs")?,
        bucket_start_unix: row.try_get("bucket_start_unix")?,
    })
}

async fn fetch_frontier_start(
    tx: &mut Transaction<'_, Postgres>,
    bucket_secs: i32,
    cutoff_unix: i64,
    unavailable_clients: &[String],
) -> Result<Option<TrafficPruneCandidate>> {
    sqlx::query(frontier_start_sql())
        .bind(bucket_secs)
        .bind(cutoff_unix)
        .bind(unavailable_clients)
        .fetch_optional(&mut **tx)
        .await?
        .map(candidate_from_row)
        .transpose()
}

async fn fetch_frontier_after(
    tx: &mut Transaction<'_, Postgres>,
    bucket_secs: i32,
    cutoff_unix: i64,
    cursor: &TrafficTerminalCursor,
    origin_kind: &str,
    unavailable_clients: &[String],
) -> Result<Option<TrafficPruneCandidate>> {
    let stream = cursor
        .stream
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("traffic terminal cursor is missing its stream"))?;
    let scan_after_unix = cursor
        .scan_after_unix
        .ok_or_else(|| anyhow::anyhow!("traffic terminal cursor is missing its bucket"))?;
    sqlx::query(frontier_after_sql())
        .bind(bucket_secs)
        .bind(cutoff_unix)
        .bind(scan_after_unix)
        .bind(&stream.client_id)
        .bind(&stream.source_kind)
        .bind(&stream.interface)
        .bind(origin_kind)
        .bind(unavailable_clients)
        .fetch_optional(&mut **tx)
        .await?
        .map(candidate_from_row)
        .transpose()
}

async fn find_candidate(
    tx: &mut Transaction<'_, Postgres>,
    cursor: &TrafficTerminalCursor,
    cutoff_unix: i64,
    unavailable_clients: &[String],
) -> Result<Option<TrafficPruneCandidate>> {
    let Some(lane) = cursor.lane.as_deref() else {
        for bucket_secs in ROLLUP_BUCKET_WIDTHS {
            if let Some(candidate) =
                fetch_frontier_start(tx, bucket_secs, cutoff_unix, unavailable_clients).await?
            {
                return Ok(Some(candidate));
            }
        }
        return Ok(None);
    };
    let (cursor_bucket_secs, cursor_origin) = parse_prune_lane(lane)?;
    let cursor_index = ROLLUP_BUCKET_WIDTHS
        .iter()
        .position(|value| *value == cursor_bucket_secs)
        .ok_or_else(|| anyhow::anyhow!("traffic terminal cursor has an invalid tier"))?;
    if let Some(candidate) = fetch_frontier_after(
        tx,
        cursor_bucket_secs,
        cutoff_unix,
        cursor,
        cursor_origin,
        unavailable_clients,
    )
    .await?
    {
        return Ok(Some(candidate));
    }
    for &bucket_secs in &ROLLUP_BUCKET_WIDTHS[(cursor_index + 1)..] {
        if let Some(candidate) =
            fetch_frontier_start(tx, bucket_secs, cutoff_unix, unavailable_clients).await?
        {
            return Ok(Some(candidate));
        }
    }
    for &bucket_secs in &ROLLUP_BUCKET_WIDTHS[..cursor_index] {
        if let Some(candidate) =
            fetch_frontier_start(tx, bucket_secs, cutoff_unix, unavailable_clients).await?
        {
            return Ok(Some(candidate));
        }
    }
    fetch_frontier_start(tx, cursor_bucket_secs, cutoff_unix, unavailable_clients).await
}

fn prune_lane(bucket_secs: i32, origin_kind: &str) -> Result<&'static str> {
    match (bucket_secs, origin_kind) {
        (3_600, "live") => Ok("prune_1h_live"),
        (3_600, "vnstat_import") => Ok("prune_1h_vnstat_import"),
        (10_800, "live") => Ok("prune_3h_live"),
        (10_800, "vnstat_import") => Ok("prune_3h_vnstat_import"),
        (21_600, "live") => Ok("prune_6h_live"),
        (21_600, "vnstat_import") => Ok("prune_6h_vnstat_import"),
        (86_400, "live") => Ok("prune_1d_live"),
        (86_400, "vnstat_import") => Ok("prune_1d_vnstat_import"),
        _ => anyhow::bail!("invalid traffic terminal frontier lane"),
    }
}

fn parse_prune_lane(lane: &str) -> Result<(i32, &'static str)> {
    match lane {
        "prune_1h_live" => Ok((3_600, "live")),
        "prune_1h_vnstat_import" => Ok((3_600, "vnstat_import")),
        "prune_3h_live" => Ok((10_800, "live")),
        "prune_3h_vnstat_import" => Ok((10_800, "vnstat_import")),
        "prune_6h_live" => Ok((21_600, "live")),
        "prune_6h_vnstat_import" => Ok((21_600, "vnstat_import")),
        "prune_1d_live" => Ok((86_400, "live")),
        "prune_1d_vnstat_import" => Ok((86_400, "vnstat_import")),
        _ => anyhow::bail!("invalid traffic terminal cursor lane"),
    }
}

async fn try_lock_client_row_then_traffic(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
) -> Result<bool> {
    let key = format!("traffic-counters:{client_id}");
    Ok(sqlx::query_scalar::<_, bool>(
        r#"
        WITH client AS MATERIALIZED (
            SELECT id
            FROM clients
            WHERE id = $1
            FOR KEY SHARE SKIP LOCKED
        )
        SELECT CASE WHEN EXISTS (SELECT 1 FROM client)
            THEN pg_try_advisory_xact_lock(hashtextextended($2, 0))
            ELSE FALSE
        END
        "#,
    )
    .bind(client_id)
    .bind(key)
    .fetch_one(&mut **tx)
    .await?)
}

fn frontier_start_sql() -> &'static str {
    r#"
        SELECT client_id, source_kind, interface, origin_kind, bucket_secs,
               EXTRACT(EPOCH FROM bucket_start)::bigint AS bucket_start_unix
        FROM traffic_counter_rollups
        WHERE bucket_secs = $1
          AND bucket_start <= to_timestamp($2)
                - make_interval(secs => $1)
          AND NOT (client_id = ANY($3::text[]))
        ORDER BY bucket_start, client_id, source_kind, interface, origin_kind
        LIMIT 1
    "#
}

fn frontier_after_sql() -> &'static str {
    r#"
        SELECT client_id, source_kind, interface, origin_kind, bucket_secs,
               EXTRACT(EPOCH FROM bucket_start)::bigint AS bucket_start_unix
        FROM traffic_counter_rollups
        WHERE bucket_secs = $1
          AND bucket_start <= to_timestamp($2)
                - make_interval(secs => $1)
          AND (bucket_start, client_id, source_kind, interface, origin_kind)
              > (to_timestamp($3), $4, $5, $6, $7)
          AND NOT (client_id = ANY($8::text[]))
        ORDER BY bucket_start, client_id, source_kind, interface, origin_kind
        LIMIT 1
    "#
}

fn preview_sql() -> &'static str {
    r#"
        WITH tiers(bucket_secs) AS (
            VALUES (3600), (10800), (21600), (86400)
        ), bounded AS MATERIALIZED (
            SELECT candidate.bucket_start, candidate.client_id,
                   candidate.source_kind, candidate.interface,
                   candidate.origin_kind, tiers.bucket_secs
            FROM tiers
            JOIN LATERAL (
                SELECT source.bucket_start, source.client_id,
                       source.source_kind, source.interface,
                       source.origin_kind
                FROM traffic_counter_rollups source
                WHERE source.bucket_secs = tiers.bucket_secs
                  AND source.bucket_start <= to_timestamp($1)
                        - make_interval(secs => tiers.bucket_secs)
                ORDER BY source.bucket_start, source.client_id,
                         source.source_kind, source.interface,
                         source.origin_kind
                LIMIT $2
            ) candidate ON TRUE
        ), selected AS MATERIALIZED (
            SELECT 1
            FROM bounded
            ORDER BY bucket_start, client_id, source_kind, interface,
                     origin_kind, bucket_secs
            LIMIT $2
        )
        SELECT count(*)::bigint FROM selected
    "#
}

fn prune_sql() -> &'static str {
    r#"
        WITH requested AS MATERIALIZED (
            SELECT source_kind, interface
            FROM UNNEST($2::text[], $3::text[])
                AS stream(source_kind, interface)
        ), cutoff AS MATERIALIZED (
            SELECT to_timestamp($4) AS value
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
