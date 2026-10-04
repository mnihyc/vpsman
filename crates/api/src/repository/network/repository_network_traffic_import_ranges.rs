//! Import-owned, range-scoped patches. Historical ambiguity is preservation,
//! not permission to replace live evidence or widen the operator's range.
use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Raw {
    at: i64,
    rx: i64,
    tx: i64,
    rx_epoch: i64,
    tx_epoch: i64,
    source: String,
    promoted: bool,
    samples: i32,
    latest: i64,
    rx_usage: i64,
    tx_usage: i64,
    rx_resets: i32,
    tx_resets: i32,
    authoritative: bool,
    rx_valid: i32,
    tx_valid: i32,
    any_valid: i32,
    any_resets: i32,
}

impl Raw {
    fn imported(&self) -> bool {
        is_vnstat_import_source(&self.source)
    }
}

impl<'r> sqlx::FromRow<'r, PgRow> for Raw {
    fn from_row(row: &'r PgRow) -> std::result::Result<Self, sqlx::Error> {
        Ok(Self {
            at: row.try_get("at")?,
            rx: row.try_get("rx")?,
            tx: row.try_get("tx")?,
            rx_epoch: row.try_get("rx_epoch")?,
            tx_epoch: row.try_get("tx_epoch")?,
            source: row.try_get("source")?,
            promoted: row.try_get("promoted")?,
            samples: row.try_get("samples")?,
            latest: row.try_get("latest")?,
            rx_usage: row.try_get("rx_usage")?,
            tx_usage: row.try_get("tx_usage")?,
            rx_resets: row.try_get("rx_resets")?,
            tx_resets: row.try_get("tx_resets")?,
            authoritative: row.try_get("authoritative")?,
            rx_valid: row.try_get("rx_valid")?,
            tx_valid: row.try_get("tx_valid")?,
            any_valid: row.try_get("any_valid")?,
            any_resets: row.try_get("any_resets")?,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Rollup {
    size: i32,
    start: i64,
    origin: String,
    rx: i64,
    tx: i64,
    rx_count: i32,
    tx_count: i32,
    count: i32,
    resets: i32,
    first: i64,
    last: i64,
}

impl<'r> sqlx::FromRow<'r, PgRow> for Rollup {
    fn from_row(row: &'r PgRow) -> std::result::Result<Self, sqlx::Error> {
        Ok(Self {
            size: row.try_get("size")?,
            start: row.try_get("start")?,
            origin: row.try_get("origin")?,
            rx: row.try_get("rx")?,
            tx: row.try_get("tx")?,
            rx_count: row.try_get("rx_count")?,
            tx_count: row.try_get("tx_count")?,
            count: row.try_get("count")?,
            resets: row.try_get("resets")?,
            first: row.try_get("first")?,
            last: row.try_get("last")?,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InterfaceSnapshot {
    interface: String,
    start: u64,
    end: u64,
    admitted: bool,
    first_live: Option<i64>,
    raw: Vec<Raw>,
    rollups: Vec<Rollup>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot {
    day: u64,
    cutoff: u64,
    interfaces: Vec<InterfaceSnapshot>,
}

#[derive(Debug)]
struct RawPatch {
    at: i64,
    rx: i64,
    tx: i64,
    rx_usage: i64,
    tx_usage: i64,
}

#[derive(Debug)]
struct BoundaryPatch {
    at: i64,
    rx_usage: i64,
    tx_usage: i64,
    rx_resets: i32,
    tx_resets: i32,
    rx_valid: i32,
    tx_valid: i32,
    any_valid: i32,
    any_resets: i32,
}

#[derive(Debug)]
struct Patch {
    interface: String,
    rx_epoch: i64,
    tx_epoch: i64,
    raw: Vec<RawPatch>,
    rollups: Vec<PreparedImportRollup>,
    boundaries: Vec<BoundaryPatch>,
    preserved: bool,
}

const RAW_COLUMNS: &str = "extract(epoch FROM observed_at)::bigint AS at, rx_bytes AS rx, tx_bytes AS tx, rx_counter_epoch AS rx_epoch, tx_counter_epoch AS tx_epoch, sample_source AS source, inbound_promoted AS promoted, sample_count AS samples, extract(epoch FROM latest_observed_at)::bigint AS latest, rx_usage_bytes AS rx_usage, tx_usage_bytes AS tx_usage, rx_reset_count AS rx_resets, tx_reset_count AS tx_resets, usage_authoritative AS authoritative, rx_valid_count AS rx_valid, tx_valid_count AS tx_valid, any_valid_count AS any_valid, any_reset_count AS any_resets";

pub(crate) fn raw_snapshot_sql() -> String {
    format!(
        r#"
        SELECT * FROM (
            (SELECT {RAW_COLUMNS} FROM traffic_counter_samples
             WHERE client_id=$1 AND source_kind='host' AND interface=$2
               AND observed_at < to_timestamp($3::double precision)
             ORDER BY observed_at DESC LIMIT 1)
            UNION ALL
            (SELECT {RAW_COLUMNS} FROM traffic_counter_samples
             WHERE client_id=$1 AND source_kind='host' AND interface=$2
               AND observed_at >= to_timestamp($3::double precision)
               AND observed_at < to_timestamp($4::double precision)
             ORDER BY observed_at LIMIT $5)
            UNION ALL
            (SELECT {RAW_COLUMNS} FROM traffic_counter_samples
             WHERE client_id=$1 AND source_kind='host' AND interface=$2
               AND observed_at >= to_timestamp($4::double precision)
             ORDER BY observed_at LIMIT 1)
        ) samples ORDER BY at
    "#
    )
}

pub(crate) const FIRST_LIVE_SQL: &str = r#"
            SELECT min(at) FROM (
                (SELECT extract(epoch FROM observed_at)::bigint AS at FROM traffic_counter_samples
                 WHERE client_id=$1 AND source_kind='host' AND interface=$2
                   AND (sample_source LIKE 'vnstat_import:%') = FALSE ORDER BY observed_at LIMIT 1)
                UNION ALL
                (SELECT retained.at FROM unnest(ARRAY[3600,10800,21600,86400]) tier(size)
                 CROSS JOIN LATERAL (
                     SELECT extract(epoch FROM first_observed_at)::bigint AS at FROM traffic_counter_rollups
                     WHERE client_id=$1 AND source_kind='host' AND interface=$2 AND origin_kind='live'
                       AND bucket_secs=tier.size ORDER BY bucket_start LIMIT 1
                 ) retained)
            ) first_live
        "#;

pub(crate) const ROLLUPS_SQL: &str = r#"
            SELECT bucket_secs AS size, extract(epoch FROM bucket_start)::bigint AS start,
                   origin_kind AS origin, rx_bytes AS rx, tx_bytes AS tx,
                   rx_valid_count AS rx_count, tx_valid_count AS tx_count, any_valid_count AS count,
                   GREATEST(rx_reset_count,tx_reset_count,any_reset_count) AS resets,
                   extract(epoch FROM first_observed_at)::bigint AS first,
                   extract(epoch FROM latest_observed_at)::bigint AS last
            FROM traffic_counter_rollups
            WHERE client_id=$1 AND source_kind='host' AND interface=$2
              AND bucket_start >= to_timestamp($3::double precision) - interval '1 day'
              AND bucket_start < to_timestamp($4::double precision)
              AND bucket_start + make_interval(secs => bucket_secs) > to_timestamp($3::double precision)
            ORDER BY bucket_start, bucket_secs, origin_kind
        "#;

async fn snapshot(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    client: &str,
    start: u64,
    result: &NetworkTrafficImportResult,
    coverage_ends: &BTreeMap<&str, u64>,
) -> Result<Snapshot> {
    let (day, cutoff) = postgres_import_retention_boundaries(tx).await?;
    let mut interfaces = Vec::new();
    let mut sources = result.sources.iter().collect::<Vec<_>>();
    sources.sort_by(|left, right| left.interface.cmp(&right.interface));
    for source in sources {
        let start = start.max(source.retained_start_unix);
        let end = result
            .collected_until_unix
            .min(floor_minute(source.source_updated_unix.unwrap_or(0)))
            .min(
                coverage_ends
                    .get(source.interface.as_str())
                    .copied()
                    .unwrap_or(0),
            );
        let admitted: bool = sqlx::query_scalar("SELECT COALESCE(bool_and(public.telemetry_interface_is_admitted_resolved(admission_mode, interface_patterns, managed_tunnel_interfaces, 'host', $2)), FALSE) FROM public.resolve_telemetry_interface_policies(ARRAY[$1])")
            .bind(client).bind(&source.interface).fetch_one(&mut **tx).await?;
        let mut item = InterfaceSnapshot {
            interface: source.interface.clone(),
            start,
            end,
            admitted,
            first_live: None,
            raw: Vec::new(),
            rollups: Vec::new(),
        };
        if !admitted || start >= end {
            interfaces.push(item);
            continue;
        }
        // The current raw-retention bound plus the two range neighbours. A
        // damaged/unpromoted oversized stream is preserved without hydrating it.
        let query = raw_snapshot_sql();
        item.raw = sqlx::query_as(&query)
            .bind(client)
            .bind(&item.interface)
            .bind(start as f64)
            .bind(end as f64)
            .bind((POSTGRES_IMPORT_MAX_RAW_ROWS_PER_INTERFACE + 1) as i64)
            .fetch_all(&mut **tx)
            .await?;
        if item
            .raw
            .iter()
            .filter(|row| row.at >= start as i64 && row.at < end as i64)
            .count()
            > POSTGRES_IMPORT_MAX_RAW_ROWS_PER_INTERFACE
        {
            interfaces.push(item);
            continue;
        }
        item.first_live = sqlx::query_scalar::<_, Option<i64>>(FIRST_LIVE_SQL)
            .bind(client)
            .bind(&item.interface)
            .fetch_one(&mut **tx)
            .await?;
        item.rollups = sqlx::query_as(ROLLUPS_SQL)
            .bind(client)
            .bind(&item.interface)
            .bind(start as f64)
            .bind(end as f64)
            .fetch_all(&mut **tx)
            .await?;
        interfaces.push(item);
    }
    Ok(Snapshot {
        day,
        cutoff,
        interfaces,
    })
}

fn merged(mut ranges: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    ranges.sort_unstable();
    let mut out: Vec<(u64, u64)> = Vec::new();
    for (start, end) in ranges {
        if start >= end {
            continue;
        }
        if let Some(last) = out.last_mut().filter(|last| start <= last.1) {
            last.1 = last.1.max(end);
        } else {
            out.push((start, end));
        }
    }
    out
}

fn uncovered(start: u64, end: u64, protected: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let mut cursor = start;
    let mut out = Vec::new();
    for &(left, right) in protected {
        if left >= end {
            break;
        }
        if cursor < left {
            out.push((cursor, left.min(end)));
        }
        cursor = cursor.max(right);
        if cursor >= end {
            return out;
        }
    }
    if cursor < end {
        out.push((cursor, end));
    }
    out
}

fn contains(ranges: &[(u64, u64)], start: u64, end: u64) -> bool {
    ranges
        .partition_point(|&(left, _)| left <= start)
        .checked_sub(1)
        .is_some_and(|index| ranges[index].1 >= end)
}

fn overlaps(ranges: &[(u64, u64)], start: u64, end: u64) -> bool {
    ranges
        .get(ranges.partition_point(|&(_, right)| right <= start))
        .is_some_and(|&(left, _)| left < end)
}

fn prepare(snapshot: &Snapshot, buckets: &[NetworkTrafficImportBucket]) -> Vec<Patch> {
    let mut patches = Vec::with_capacity(snapshot.interfaces.len());
    for item in &snapshot.interfaces {
        match prepare_interface(item, snapshot.day, snapshot.cutoff, buckets) {
            Ok(patch) => patches.push(patch),
            Err(error) => {
                // Pure data preparation cannot partially publish a source.
                // Unsupported/overflowing history is a successful preservation;
                // database and task failures outside this stage remain errors.
                tracing::info!(interface=%item.interface,%error,"preserving unreconcilable vnStat history");
                patches.push(Patch {
                    interface: item.interface.clone(),
                    rx_epoch: 0,
                    tx_epoch: 0,
                    raw: Vec::new(),
                    rollups: Vec::new(),
                    boundaries: Vec::new(),
                    preserved: true,
                });
            }
        }
    }
    patches
}

// A one-observation resumed minute may contain the whole outage delta. Keep
// its residual (including the observed boundary minute), subtracting only the
// newly imported bytes. A reset has no comparable bridge. If the source would
// require a negative residual, the evidence disagrees: preserve the gap.
fn reconcile_gap_boundary(left: &Raw, right: &Raw, rx: u64, tx: u64) -> Option<(i64, i64)> {
    let direction = |before: i64,
                     after: i64,
                     before_epoch: i64,
                     after_epoch: i64,
                     usage: i64,
                     imported: u64| {
        if before_epoch != after_epoch || after < before {
            return if !right.authoritative || usage == 0 {
                Some(0)
            } else {
                None
            };
        }
        let bridge = if right.authoritative {
            usage
        } else {
            after - before
        };
        bridge
            .checked_sub(i64::try_from(imported).ok()?)
            .filter(|remaining| *remaining >= 0)
    };
    Some((
        direction(
            left.rx,
            right.rx,
            left.rx_epoch,
            right.rx_epoch,
            right.rx_usage,
            rx,
        )?,
        direction(
            left.tx,
            right.tx,
            left.tx_epoch,
            right.tx_epoch,
            right.tx_usage,
            tx,
        )?,
    ))
}

fn prepare_interface(
    item: &InterfaceSnapshot,
    day: u64,
    cutoff: u64,
    buckets: &[NetworkTrafficImportBucket],
) -> Result<Patch> {
    let mut patch = Patch {
        interface: item.interface.clone(),
        rx_epoch: 0,
        tx_epoch: 0,
        raw: Vec::new(),
        rollups: Vec::new(),
        boundaries: Vec::new(),
        preserved: false,
    };
    if !item.admitted
        || item.start >= item.end
        || item
            .raw
            .iter()
            .filter(|row| row.at >= item.start as i64 && row.at < item.end as i64)
            .count()
            > POSTGRES_IMPORT_MAX_RAW_ROWS_PER_INTERFACE
    {
        patch.preserved = true;
        return Ok(patch);
    }
    let mut protected = item
        .raw
        .iter()
        .filter(|row| {
            !row.imported()
                || row.at < cutoff as i64
                || row.promoted
                || row.samples != 1
                || row.latest != row.at
                || row.rx_resets != 0
                || row.tx_resets != 0
                || row.any_resets != 0
        })
        .map(|row| (row.at as u64, row.at as u64 + 60))
        .collect::<Vec<_>>();
    let owned = merged(
        item.rollups
            .iter()
            .filter(|row| {
                row.origin == "vnstat_import"
                    && row.resets == 0
                    && row.count as i64 == (row.last - row.first) / 60 + 1
            })
            .map(|row| (row.first as u64, row.last as u64 + 60))
            .chain(
                item.raw
                    .iter()
                    .filter(|row| row.imported() && row.at >= cutoff as i64)
                    .map(|row| (row.at as u64, row.at as u64 + 60)),
            )
            .collect(),
    );
    // Live bounds prove no observation exists before/after their support, but
    // say nothing about holes inside it. Safe prefixes may share a bucket.
    protected.extend(
        item.rollups
            .iter()
            .filter(|row| row.origin == "live")
            .map(|row| {
                (
                    floor_minute(row.first as u64),
                    floor_minute(row.last as u64) + 60,
                )
            }),
    );
    let protected_evidence = merged(protected);
    let mut protected = protected_evidence.clone();
    for row in item.rollups.iter().filter(|row| row.origin != "live") {
        let start = row.start as u64;
        let end = start + row.size as u64;
        if row.resets != 0
            || row.count as i64 != (row.last - row.first) / 60 + 1
            || row.rx_count != row.count
            || row.tx_count != row.count
            || overlaps(&protected_evidence, row.first as u64, row.last as u64 + 60)
            || row.last >= cutoff as i64
            || row.first < item.start as i64
            || row.last >= item.end as i64
            || import_rollup_bucket_secs(start, day) != row.size
        {
            // Sparse support or protected observations cannot be split using
            // only a rollup total. Protect its whole destination bucket before
            // reconciling any gap: skipping the write later must not leave a
            // boundary subtraction for traffic that was never published.
            let width = import_rollup_bucket_secs(start, day) as u64;
            protected.push((start - start % width, end.div_ceil(width) * width));
            patch.preserved |= row.origin == "vnstat_import";
        }
    }
    let protected = merged(protected);
    let candidates = uncovered(item.start, item.end, &protected);
    if candidates.is_empty() {
        return Ok(patch);
    }
    let traffic = match expand_buckets_to_minutes(buckets, &item.interface, item.start, item.end) {
        Ok(traffic) => traffic,
        Err(error) => {
            tracing::info!(interface=%item.interface,%error,"preserving vnStat history that cannot be reconciled");
            patch.preserved = true;
            return Ok(patch);
        }
    };
    let mut allowed = Vec::new();
    let mut repaired_boundaries = BTreeMap::new();
    for (start, end) in candidates {
        let prefix = item.first_live.is_none_or(|first| end <= first as u64);
        let right = item
            .raw
            .iter()
            .find(|row| row.at == end as i64 && !row.imported());
        let left = item
            .raw
            .iter()
            .rev()
            .find(|row| row.at < start as i64 && !row.imported());
        let complete_gap = left.is_some_and(|left| left.at + 60 == start as i64)
            && right.is_some_and(|right| {
                !right.promoted && right.samples == 1 && right.latest == right.at
            });
        let zero_boundary = right
            .is_some_and(|right| right.authoritative && right.rx_usage == 0 && right.tx_usage == 0);
        let already_owned = contains(&owned, start, end);
        let no_previous_import = owned
            .get(owned.partition_point(|&(_, right)| right <= start))
            .is_none_or(|&(left, _)| left >= end);
        let repaired_usage = if !prefix && complete_gap && no_previous_import {
            let (rx, tx) = assignment_totals_in_range(&traffic.segments, start, end)?;
            reconcile_gap_boundary(left.unwrap(), right.unwrap(), rx, tx)
        } else {
            None
        };
        if prefix || already_owned || repaired_usage.is_some() || zero_boundary {
            allowed.push((start, end));
            if let Some(usage) = repaired_usage {
                repaired_boundaries.insert(end as i64, usage);
            }
        } else {
            patch.preserved = true;
            // Even when a missing interval cannot be reconciled, already-owned
            // dense data in it can be refreshed without widening its coverage.
            for &(left, right) in &owned[owned.partition_point(|range| range.1 <= start)..] {
                if left >= end {
                    break;
                }
                allowed.push((left.max(start), right.min(end)));
            }
        }
    }
    let allowed = merged(allowed);
    if allowed.is_empty() {
        return Ok(patch);
    }
    // Network-rate readers also consume synthetic counter endpoints. Keep new
    // endpoints in a different epoch from both live neighbours, so they never
    // create a fabricated rate across a synthetic/live boundary. Authoritative
    // usage owns accounting; no live epoch or unrelated suffix is rewritten.
    patch.rx_epoch = item
        .raw
        .iter()
        .map(|row| row.rx_epoch)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .context("network_traffic_import_rx_epoch_overflow")?;
    patch.tx_epoch = item
        .raw
        .iter()
        .map(|row| row.tx_epoch)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .context("network_traffic_import_tx_epoch_overflow")?;
    tracing::debug!(interface=%item.interface, minutes=traffic.minute_count, rx_bytes=traffic.total_rx_bytes,
        tx_bytes=traffic.total_tx_bytes, ranges=allowed.len(), "prepared bounded vnStat range patch");
    let mut rollups = BTreeMap::<(i32, u64), PreparedImportRollup>::new();
    for &(start, end) in &allowed {
        let (rx, tx) = assignment_totals_in_range(&traffic.segments, start, end)?;
        let segments = traffic.segments[traffic
            .segments
            .partition_point(|segment| segment.end_unix <= start)..]
            .iter()
            .take_while(|segment| segment.start_unix < end)
            .map(|segment| MinuteAssignmentSegment {
                start_unix: segment.start_unix.max(start),
                end_unix: segment.end_unix.min(end),
                ..*segment
            })
            .collect();
        let prepared = PreparedInterfaceImport {
            interface: item.interface.clone(),
            start_unix: start,
            end_unix: end,
            initial_rx_bytes: 0,
            initial_tx_bytes: 0,
            include_baseline: false,
            traffic: ExpandedMinuteTraffic {
                segments,
                minute_count: (end - start) / 60,
                total_rx_bytes: rx,
                total_tx_bytes: tx,
            },
        };
        for row in prepare_import_rollups(&prepared, day, cutoff)? {
            let key = (row.bucket_secs, row.bucket_start_unix);
            if let Some(existing) = rollups.get_mut(&key) {
                existing.rx_bytes = existing
                    .rx_bytes
                    .checked_add(row.rx_bytes)
                    .context("network_traffic_import_rx_overflow")?;
                existing.tx_bytes = existing
                    .tx_bytes
                    .checked_add(row.tx_bytes)
                    .context("network_traffic_import_tx_overflow")?;
                existing.rx_valid_count += row.rx_valid_count;
                existing.tx_valid_count += row.tx_valid_count;
                existing.any_valid_count += row.any_valid_count;
                existing.first_observed_unix =
                    existing.first_observed_unix.min(row.first_observed_unix);
                existing.latest_observed_unix =
                    existing.latest_observed_unix.max(row.latest_observed_unix);
            } else {
                rollups.insert(key, row);
            }
        }
        let raw = prepare_import_raw_rows(&prepared, cutoff, cutoff)?;
        for (index, &at) in raw.observed_unix.iter().enumerate() {
            let (rx, tx) =
                assignment_totals_in_range(&prepared.traffic.segments, at as u64, at as u64 + 60)?;
            patch.raw.push(RawPatch {
                at,
                rx: raw.rx_bytes[index],
                tx: raw.tx_bytes[index],
                rx_usage: i64::try_from(rx)?,
                tx_usage: i64::try_from(tx)?,
            });
        }
    }
    let previous_rollups = item
        .rollups
        .iter()
        .filter(|old| old.origin == "vnstat_import")
        .map(|old| ((old.size, old.start), old))
        .collect::<BTreeMap<_, _>>();
    for (_, row) in rollups {
        // Several disjoint ranges can contribute to one retained bucket. Check
        // the combined database bounds during best-effort preparation too.
        validate_prepared_import_rollup(&row)?;
        let existing = previous_rollups.get(&(row.bucket_secs, row.bucket_start_unix as i64));
        if existing.is_some_and(|old| !contains(&allowed, old.first as u64, old.last as u64 + 60)) {
            patch.preserved = true;
            continue;
        }
        if existing.is_some_and(|old| {
            old.rx == row.rx_bytes as i64
                && old.tx == row.tx_bytes as i64
                && old.rx_count == row.rx_valid_count as i32
                && old.tx_count == row.tx_valid_count as i32
                && old.count == row.any_valid_count as i32
                && old.first == row.first_observed_unix as i64
                && old.last == row.latest_observed_unix as i64
        }) {
            continue;
        }
        patch.rollups.push(row);
    }
    patch.raw.retain(|row| {
        !item.raw.iter().any(|old| {
            old.at == row.at
                && old.imported()
                && old.authoritative
                && old.rx_usage == row.rx_usage
                && old.tx_usage == row.tx_usage
                && old.rx_valid == 1
                && old.tx_valid == 1
                && old.any_valid == 1
        })
    });
    // Freeze only the immediate successor's derived usage before changing its
    // synthetic predecessor. Physical live counters and epochs never change.
    for (index, row) in item.raw.iter().enumerate() {
        if row.promoted {
            continue;
        }
        if contains(&allowed, row.at as u64, row.at as u64 + 60) {
            continue;
        }
        let repair = repaired_boundaries.get(&row.at);
        let previous = index.checked_sub(1).and_then(|index| item.raw.get(index));
        let preceding_patch = repair.is_some()
            || patch
                .raw
                .iter()
                .rev()
                .find(|patch| patch.at < row.at)
                .is_some_and(|patch| previous.is_none_or(|previous| previous.at <= patch.at));
        if !preceding_patch {
            continue;
        }
        if row.authoritative && repair.is_none() {
            continue;
        }
        let rx_usage = if let Some(&(rx, _)) = repair {
            rx
        } else if row.authoritative {
            row.rx_usage
        } else {
            previous
                .filter(|old| old.rx_epoch == row.rx_epoch && old.rx <= row.rx)
                .map_or(0, |old| row.rx - old.rx)
        };
        let tx_usage = if let Some(&(_, tx)) = repair {
            tx
        } else if row.authoritative {
            row.tx_usage
        } else {
            previous
                .filter(|old| old.tx_epoch == row.tx_epoch && old.tx <= row.tx)
                .map_or(0, |old| row.tx - old.tx)
        };
        let rx_resets = if row.authoritative {
            row.rx_resets
        } else {
            i32::from(previous.is_some_and(|old| old.rx_epoch != row.rx_epoch && !old.imported()))
        };
        let tx_resets = if row.authoritative {
            row.tx_resets
        } else {
            i32::from(previous.is_some_and(|old| old.tx_epoch != row.tx_epoch && !old.imported()))
        };
        let rx_valid = if row.authoritative {
            row.rx_valid
        } else {
            i32::from(previous.is_some_and(|old| old.rx_epoch == row.rx_epoch && old.rx <= row.rx))
        };
        let tx_valid = if row.authoritative {
            row.tx_valid
        } else {
            i32::from(previous.is_some_and(|old| old.tx_epoch == row.tx_epoch && old.tx <= row.tx))
        };
        let any_valid = if row.authoritative {
            row.any_valid
        } else {
            rx_valid.max(tx_valid)
        };
        let any_resets = if row.authoritative {
            row.any_resets
        } else {
            rx_resets.max(tx_resets)
        };
        if !row.authoritative || row.rx_usage != rx_usage || row.tx_usage != tx_usage {
            patch.boundaries.push(BoundaryPatch {
                at: row.at,
                rx_usage,
                tx_usage,
                rx_resets,
                tx_resets,
                rx_valid,
                tx_valid,
                any_valid,
                any_resets,
            });
        }
    }
    Ok(patch)
}

pub(super) async fn import(
    repo: &Repository,
    job_id: Uuid,
    client: &str,
    selectors: &[String],
    start: u64,
    result: &NetworkTrafficImportResult,
    buckets: &[NetworkTrafficImportBucket],
) -> Result<NetworkTrafficImportSummary> {
    let Repository::Postgres(pool) = repo;
    // Contract validation already proves each source's latest continuous
    // component. Its end is the largest bucket end, which can precede the
    // database update time. Resolve it once without another sort or SQL read.
    let mut coverage_ends = BTreeMap::<&str, u64>::new();
    for bucket in buckets {
        let end = bucket
            .start_unix
            .saturating_add(u64::from(bucket.duration_secs));
        coverage_ends
            .entry(&bucket.interface)
            .and_modify(|previous| *previous = (*previous).max(end))
            .or_insert(end);
    }
    for attempt in 0..POSTGRES_IMPORT_MAX_PREPARATION_ATTEMPTS {
        let mut read = pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *read)
            .await?;
        let before = snapshot(&mut read, client, start, result, &coverage_ends).await?;
        read.commit().await?;
        let input = before.clone();
        let buckets = buckets.to_vec();
        let patches = tokio::task::spawn_blocking(move || prepare(&input, &buckets)).await?;
        let mut tx = pool.begin().await?;
        lock_postgres_traffic_import_client(&mut tx, client).await?;
        lock_postgres_traffic_counter_streams(&mut tx, client).await?;
        if snapshot(&mut tx, client, start, result, &coverage_ends).await? != before {
            tx.rollback().await?;
            if attempt + 1 == POSTGRES_IMPORT_MAX_PREPARATION_ATTEMPTS {
                anyhow::bail!(
                    "network_traffic_import_preflight_changed_after_{}_attempts",
                    POSTGRES_IMPORT_MAX_PREPARATION_ATTEMPTS
                );
            }
            continue;
        }
        sqlx::query(POSTGRES_IMPORT_WORK_MEM_SQL)
            .execute(&mut *tx)
            .await?;
        // Bulk UNNEST statements must use their actual array cardinality, not
        // a cached ten-row estimate. This setting ends with this transaction.
        sqlx::query("SET LOCAL plan_cache_mode = 'force_custom_plan'")
            .execute(&mut *tx)
            .await?;
        let mut changed = 0;
        let mut preserved = result.interfaces.len().saturating_sub(result.sources.len());
        for patch in &patches {
            preserved += usize::from(patch.preserved);
            if patch.raw.is_empty() && patch.rollups.is_empty() && patch.boundaries.is_empty() {
                continue;
            }
            ensure_postgres_vnstat_interfaces_admitted(
                &mut tx,
                client,
                std::slice::from_ref(&patch.interface),
            )
            .await?;
            apply(&mut tx, client, job_id, patch).await?;
            changed += 1;
        }
        if changed > 0 {
            sqlx::query("SELECT refresh_traffic_counter_active_cycle_usage($1::text[])")
                .bind(vec![client])
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        let unmatched = selectors
            .iter()
            .filter(|selector| {
                !result.interfaces.iter().any(|interface| {
                    vpsman_common::network_interface_pattern_matches(selector, interface)
                })
            })
            .count();
        return Ok(NetworkTrafficImportSummary { message: format!("vnStat import completed: {changed} interface(s) updated in the selected range; {preserved} interface(s) with unavailable or unreconcilable history preserved; {unmatched} unmatched selector(s). Live counters and imported history outside the range are preserved; unchanged data requires no rewrite.") });
    }
    unreachable!()
}

async fn apply(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    client: &str,
    job: Uuid,
    patch: &Patch,
) -> Result<()> {
    if !patch.boundaries.is_empty() {
        sqlx::query(r#"
            UPDATE traffic_counter_samples sample SET
                rx_usage_bytes=changed.rx, tx_usage_bytes=changed.tx,
                rx_reset_count=changed.rx_resets, tx_reset_count=changed.tx_resets,
                rx_valid_count=changed.rx_valid,tx_valid_count=changed.tx_valid,any_valid_count=changed.any_valid,any_reset_count=changed.any_resets,
                usage_authoritative=TRUE, updated_at=clock_timestamp()
            FROM unnest($3::bigint[], $4::bigint[], $5::bigint[], $6::int[], $7::int[],$8::int[],$9::int[],$10::int[],$11::int[]) changed(at,rx,tx,rx_resets,tx_resets,rx_valid,tx_valid,any_valid,any_resets)
            WHERE sample.client_id=$1 AND sample.source_kind='host' AND sample.interface=$2
              AND sample.observed_at=to_timestamp(changed.at::double precision)
        "#).bind(client).bind(&patch.interface)
            .bind(patch.boundaries.iter().map(|row| row.at).collect::<Vec<_>>())
            .bind(patch.boundaries.iter().map(|row| row.rx_usage).collect::<Vec<_>>())
            .bind(patch.boundaries.iter().map(|row| row.tx_usage).collect::<Vec<_>>())
            .bind(patch.boundaries.iter().map(|row| row.rx_resets).collect::<Vec<_>>())
            .bind(patch.boundaries.iter().map(|row| row.tx_resets).collect::<Vec<_>>())
            .bind(patch.boundaries.iter().map(|row| row.rx_valid).collect::<Vec<_>>())
            .bind(patch.boundaries.iter().map(|row| row.tx_valid).collect::<Vec<_>>())
            .bind(patch.boundaries.iter().map(|row| row.any_valid).collect::<Vec<_>>())
            .bind(patch.boundaries.iter().map(|row| row.any_resets).collect::<Vec<_>>()).execute(&mut **tx).await?;
    }
    if !patch.raw.is_empty() {
        sqlx::query(r#"
            INSERT INTO traffic_counter_samples (client_id,source_kind,interface,observed_at,
                rx_bytes,tx_bytes,sample_source,rx_usage_bytes,tx_usage_bytes,
                rx_valid_count,tx_valid_count,any_valid_count,usage_authoritative,
                rx_counter_epoch,tx_counter_epoch)
            SELECT $1,'host',$2,to_timestamp(row.at::double precision),row.rx,row.tx,$3,
                   row.rx_usage,row.tx_usage,1,1,1,TRUE,$9,$10
            FROM unnest($4::bigint[],$5::bigint[],$6::bigint[],$7::bigint[],$8::bigint[]) row(at,rx,tx,rx_usage,tx_usage)
            ORDER BY row.at
            ON CONFLICT (client_id,source_kind,interface,observed_at) DO UPDATE SET
                rx_bytes=EXCLUDED.rx_bytes,tx_bytes=EXCLUDED.tx_bytes,sample_source=EXCLUDED.sample_source,
                rx_counter_epoch=EXCLUDED.rx_counter_epoch,tx_counter_epoch=EXCLUDED.tx_counter_epoch,
                rx_usage_bytes=EXCLUDED.rx_usage_bytes,tx_usage_bytes=EXCLUDED.tx_usage_bytes,
                rx_valid_count=1,tx_valid_count=1,any_valid_count=1,usage_authoritative=TRUE,
                rx_reset_count=0,tx_reset_count=0,any_reset_count=0,
                updated_at=clock_timestamp()
            WHERE traffic_counter_samples.sample_source LIKE 'vnstat_import:%'
        "#).bind(client).bind(&patch.interface).bind(format!("{VNSTAT_IMPORT_SOURCE_PREFIX}{job}"))
            .bind(patch.raw.iter().map(|row| row.at).collect::<Vec<_>>())
            .bind(patch.raw.iter().map(|row| row.rx).collect::<Vec<_>>())
            .bind(patch.raw.iter().map(|row| row.tx).collect::<Vec<_>>())
            .bind(patch.raw.iter().map(|row| row.rx_usage).collect::<Vec<_>>())
            .bind(patch.raw.iter().map(|row| row.tx_usage).collect::<Vec<_>>())
            .bind(patch.rx_epoch).bind(patch.tx_epoch).execute(&mut **tx).await?;
    }
    let rows = prepare_import_rollup_rows(&patch.interface, patch.rollups.clone())?;
    insert_postgres_import_rollups(tx, client, &patch.interface, &rows).await?;
    Ok(())
}
