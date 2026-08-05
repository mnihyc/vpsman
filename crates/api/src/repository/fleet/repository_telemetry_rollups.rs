use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Context, Result};
use chrono::DateTime;
use sqlx::Row;
use vpsman_common::AgentMetrics;

use crate::{
    model::{
        TelemetryNetworkRateView, TelemetryRollupView, TelemetrySampleView,
        TelemetryTunnelAdapterHealthView, TelemetryTunnelView, TunnelPlanView,
    },
    model_alert_policies::NetworkRateInterfaceSelection,
    repository::Repository,
    util::compare_timestamps_desc,
};

const TELEMETRY_LIST_LIMIT_MAX: i64 = 50_000;
const DASHBOARD_TELEMETRY_RESULT_LIMIT: usize = 50_000;

fn raw_sample_rollup(sample: TelemetrySampleView) -> Result<TelemetryRollupView> {
    let metrics: AgentMetrics =
        serde_json::from_value(sample.payload.clone()).with_context(|| {
            format!(
                "invalid raw telemetry payload for {} at {}",
                sample.client_id, sample.observed_at
            )
        })?;
    let disk_total = metrics
        .disks
        .iter()
        .fold(0_u64, |total, disk| total.saturating_add(disk.total_bytes));
    let disk_available = metrics.disks.iter().fold(0_u64, |total, disk| {
        total.saturating_add(disk.available_bytes)
    });
    let disk_total = saturating_u64_to_i64(disk_total);
    let disk_available = saturating_u64_to_i64(disk_available);
    let disk_used_ratio = used_ratio_or_zero(disk_total, disk_available);
    let network_rx = metrics.networks.iter().fold(0_u64, |total, network| {
        total.saturating_add(network.rx_bytes)
    });
    let network_tx = metrics.networks.iter().fold(0_u64, |total, network| {
        total.saturating_add(network.tx_bytes)
    });
    let cpu_usage = metrics.cpu.utilization_ratio;
    let memory_total = saturating_u64_to_i64(metrics.memory.total_bytes);
    let memory_available = saturating_u64_to_i64(metrics.memory.available_bytes);
    let memory_used_ratio = used_ratio_or_zero(memory_total, memory_available);
    let swap = match (
        metrics.memory.swap_total_bytes,
        metrics.memory.swap_available_bytes,
    ) {
        (None, None) => None,
        (Some(total), Some(available)) if available <= total => Some((
            saturating_u64_to_i64(total),
            saturating_u64_to_i64(available),
        )),
        (Some(_), Some(_)) => anyhow::bail!(
            "invalid raw telemetry payload for {} at {}: swap available exceeds total",
            sample.client_id,
            sample.observed_at
        ),
        _ => anyhow::bail!(
            "invalid raw telemetry payload for {} at {}: swap evidence is one-sided",
            sample.client_id,
            sample.observed_at
        ),
    };
    let positive_swap = swap.filter(|(total, _)| *total > 0);

    Ok(TelemetryRollupView {
        client_id: sample.client_id,
        bucket_start: sample.observed_at.clone(),
        bucket_secs: 60,
        sample_count: 1,
        cpu_usage_sample_count: i32::from(cpu_usage.is_some()),
        cpu_usage_avg: cpu_usage,
        cpu_usage_max: cpu_usage,
        cpu_cores_max: i32::from(metrics.cpu.cores),
        cpu_load_1_avg: metrics.cpu.load.one,
        cpu_load_1_max: metrics.cpu.load.one,
        cpu_load_5_avg: metrics.cpu.load.five,
        cpu_load_5_max: metrics.cpu.load.five,
        cpu_load_15_avg: metrics.cpu.load.fifteen,
        cpu_load_15_max: metrics.cpu.load.fifteen,
        memory_total_bytes_max: memory_total,
        memory_available_bytes_avg: memory_available,
        memory_available_bytes_min: memory_available,
        memory_used_ratio_avg: memory_used_ratio,
        memory_used_ratio_max: memory_used_ratio,
        swap_sample_count: i32::from(positive_swap.is_some()),
        swap_total_bytes_max: swap.map(|(total, _)| total),
        swap_available_bytes_avg: swap.map(|(_, available)| available),
        swap_available_bytes_min: swap.map(|(_, available)| available),
        swap_used_ratio_avg: positive_swap
            .and_then(|(total, available)| used_ratio(total, available)),
        swap_used_ratio_max: positive_swap
            .and_then(|(total, available)| used_ratio(total, available)),
        disk_total_bytes_max: disk_total,
        disk_available_bytes_avg: disk_available,
        disk_available_bytes_min: disk_available,
        disk_used_ratio_avg: disk_used_ratio,
        disk_used_ratio_max: disk_used_ratio,
        network_rx_bytes_max: saturating_u64_to_i64(network_rx),
        network_tx_bytes_max: saturating_u64_to_i64(network_tx),
        connections_sample_count: i32::from(metrics.connections.is_some()),
        tcp_sockets_latest: metrics
            .connections
            .as_ref()
            .map(|connections| saturating_u64_to_i64(connections.tcp)),
        udp_sockets_latest: metrics
            .connections
            .as_ref()
            .map(|connections| saturating_u64_to_i64(connections.udp)),
        connections_observed_at: metrics
            .connections
            .as_ref()
            .map(|_| sample.observed_at.clone()),
        latest_observed_at: sample.observed_at.clone(),
        updated_at: sample.observed_at,
    })
}

fn saturating_u64_to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn used_ratio(total: i64, available: i64) -> Option<f64> {
    (total > 0)
        .then(|| (total.saturating_sub(available).max(0) as f64 / total as f64).clamp(0.0, 1.0))
}

fn used_ratio_or_zero(total: i64, available: i64) -> f64 {
    used_ratio(total, available).unwrap_or(0.0)
}

impl Repository {
    pub(crate) async fn raw_telemetry_covers_range_start(
        &self,
        client_ids: &[String],
        start_unix: u64,
    ) -> Result<bool> {
        if client_ids.is_empty() {
            return Ok(true);
        }
        match self {
            Self::Memory(memory) => {
                let samples = memory.telemetry_samples.read().await;
                let resource_rollups = memory.telemetry_rollups.read().await;
                let network_rollups = memory.telemetry_network_rates.read().await;
                let ping_rollups = memory.telemetry_ping_rollups.read().await;
                let traffic_samples = memory.traffic_counter_samples.read().await;
                Ok(client_ids.iter().all(|client_id| {
                    let raw_start = samples
                        .iter()
                        .filter(|row| row.client_id == *client_id)
                        .filter_map(|row| parse_timestamp_unix(&row.observed_at))
                        .min();
                    let minute_start = resource_rollups
                        .iter()
                        .filter(|row| row.client_id == *client_id)
                        .filter_map(|row| parse_timestamp_unix(&row.bucket_start))
                        .chain(
                            network_rollups
                                .iter()
                                .filter(|row| row.client_id == *client_id)
                                .filter_map(|row| parse_timestamp_unix(&row.bucket_start)),
                        )
                        .chain(
                            ping_rollups
                                .iter()
                                .filter(|row| row.client_id == *client_id)
                                .filter_map(|row| parse_timestamp_unix(&row.bucket_start)),
                        )
                        .chain(
                            traffic_samples
                                .iter()
                                .filter(|row| row.client_id == *client_id)
                                .filter_map(|row| u64::try_from(row.observed_unix).ok()),
                        )
                        .min();
                    minute_start.is_none_or(|minute_start| {
                        raw_start.is_some_and(|raw_start| raw_start <= minute_start.max(start_unix))
                    })
                }))
            }
            Self::Postgres(pool) => {
                let covers = sqlx::query_scalar::<_, bool>(
                    r#"
                    WITH requested AS (
                        SELECT unnest($1::TEXT[]) AS client_id
                    ), minute_bounds AS (
                        SELECT client_id, min(bucket_start) AS minute_start
                        FROM (
                            SELECT client_id, bucket_start FROM telemetry_rollups
                            UNION ALL
                            SELECT client_id, bucket_start FROM telemetry_network_rates
                            UNION ALL
                            SELECT client_id, bucket_start FROM telemetry_ping_rollups
                            UNION ALL
                            SELECT client_id, observed_at AS bucket_start
                            FROM traffic_counter_samples
                        ) retained
                        WHERE client_id = ANY($1::TEXT[])
                        GROUP BY client_id
                    ), raw_bounds AS (
                        SELECT client_id, min(observed_at) AS raw_start
                        FROM telemetry_samples
                        WHERE client_id = ANY($1::TEXT[])
                        GROUP BY client_id
                    )
                    SELECT COALESCE(bool_and(
                        minute_bounds.minute_start IS NULL
                        OR (
                            raw_bounds.raw_start IS NOT NULL
                            AND raw_bounds.raw_start <= GREATEST(
                                minute_bounds.minute_start,
                                to_timestamp($2)
                            )
                        )
                    ), TRUE)
                    FROM requested
                    LEFT JOIN minute_bounds USING (client_id)
                    LEFT JOIN raw_bounds USING (client_id)
                    "#,
                )
                .bind(client_ids)
                .bind(start_unix as i64)
                .fetch_one(pool)
                .await?;
                Ok(covers)
            }
        }
    }

    pub(crate) async fn list_telemetry_samples(
        &self,
        limit: i64,
        client_id: Option<&str>,
        start_unix: Option<u64>,
        end_unix: Option<u64>,
        visible_only: bool,
    ) -> Result<Vec<TelemetrySampleView>> {
        match self {
            Self::Memory(memory) => {
                let hidden = memory.hidden_clients.read().await;
                let mut rows = memory
                    .telemetry_samples
                    .read()
                    .await
                    .iter()
                    .filter(|sample| {
                        (!visible_only || !hidden.contains(&sample.client_id))
                            && client_id.is_none_or(|client_id| sample.client_id == client_id)
                            && timestamp_in_bounds(&sample.observed_at, start_unix, end_unix)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                rows.sort_by(|left, right| {
                    compare_timestamps_desc(&left.observed_at, &right.observed_at)
                        .then_with(|| left.client_id.cmp(&right.client_id))
                        .then_with(|| right.id.cmp(&left.id))
                });
                rows.truncate(limit.clamp(1, TELEMETRY_LIST_LIMIT_MAX) as usize);
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        client_id,
                        observed_at::text AS observed_at,
                        cpu_load_1,
                        memory_total_bytes,
                        memory_available_bytes,
                        payload
                    FROM telemetry_samples
                    WHERE
                        ($1::TEXT IS NULL OR client_id = $1)
                        AND ($2::BIGINT IS NULL OR observed_at >= to_timestamp($2))
                        AND ($3::BIGINT IS NULL OR observed_at <= to_timestamp($3))
                        AND (
                            NOT $4
                            OR EXISTS (
                                SELECT 1 FROM visible_clients
                                WHERE visible_clients.id = telemetry_samples.client_id
                            )
                        )
                    ORDER BY observed_at DESC, id DESC
                    LIMIT $5
                    "#,
                )
                .bind(client_id)
                .bind(start_unix.map(|value| value as i64))
                .bind(end_unix.map(|value| value as i64))
                .bind(visible_only)
                .bind(limit.clamp(1, TELEMETRY_LIST_LIMIT_MAX))
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(TelemetrySampleView {
                            id: row.try_get("id")?,
                            client_id: row.try_get("client_id")?,
                            observed_at: row.try_get("observed_at")?,
                            cpu_load_1: row.try_get("cpu_load_1")?,
                            memory_total_bytes: row.try_get("memory_total_bytes")?,
                            memory_available_bytes: row.try_get("memory_available_bytes")?,
                            payload: row.try_get("payload")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn list_dashboard_raw_telemetry_rollups(
        &self,
        points_per_client: i64,
        start_unix: u64,
        end_unix: u64,
        step_secs: i32,
        client_ids: &[String],
    ) -> Result<Vec<TelemetryRollupView>> {
        let points_per_client = points_per_client.clamp(2, 1_440) as usize;
        let step_secs = normalized_dashboard_step_secs(step_secs);
        if let Self::Postgres(pool) = self {
            let rows = sqlx::query(
                r#"
                WITH expanded AS (
                    SELECT
                        sample.client_id,
                        floor(
                            extract(epoch FROM sample.observed_at)::numeric / $4::numeric
                        )::bigint * $4::bigint AS chart_epoch,
                        sample.observed_at,
                        NULLIF(sample.payload #>> '{cpu,utilization_ratio}', '')
                            ::double precision AS cpu_usage,
                        COALESCE((sample.payload #>> '{cpu,cores}')::integer, 0)
                            AS cpu_cores,
                        sample.cpu_load_1,
                        COALESCE((sample.payload #>> '{cpu,load,five}')::double precision, 0)
                            AS cpu_load_5,
                        COALESCE((sample.payload #>> '{cpu,load,fifteen}')::double precision, 0)
                            AS cpu_load_15,
                        sample.memory_total_bytes,
                        sample.memory_available_bytes,
                        CASE
                            WHEN sample.payload #>> '{memory,swap_total_bytes}' IS NOT NULL
                             AND sample.payload #>> '{memory,swap_available_bytes}' IS NOT NULL
                            THEN LEAST(
                                (sample.payload #>> '{memory,swap_total_bytes}')::numeric,
                                9223372036854775807
                            )::bigint
                        END AS swap_total_bytes,
                        CASE
                            WHEN sample.payload #>> '{memory,swap_total_bytes}' IS NOT NULL
                             AND sample.payload #>> '{memory,swap_available_bytes}' IS NOT NULL
                            THEN LEAST(
                                (sample.payload #>> '{memory,swap_available_bytes}')::numeric,
                                9223372036854775807
                            )::bigint
                        END AS swap_available_bytes,
                        LEAST(COALESCE((
                            SELECT sum((disk ->> 'total_bytes')::numeric)
                            FROM jsonb_array_elements(
                                CASE
                                    WHEN jsonb_typeof(sample.payload -> 'disks') = 'array'
                                    THEN sample.payload -> 'disks'
                                    ELSE '[]'::jsonb
                                END
                            ) AS disk
                        ), 0), 9223372036854775807)::bigint AS disk_total_bytes,
                        LEAST(COALESCE((
                            SELECT sum((disk ->> 'available_bytes')::numeric)
                            FROM jsonb_array_elements(
                                CASE
                                    WHEN jsonb_typeof(sample.payload -> 'disks') = 'array'
                                    THEN sample.payload -> 'disks'
                                    ELSE '[]'::jsonb
                                END
                            ) AS disk
                        ), 0), 9223372036854775807)::bigint AS disk_available_bytes,
                        LEAST(COALESCE((
                            SELECT sum((network ->> 'rx_bytes')::numeric)
                            FROM jsonb_array_elements(
                                CASE
                                    WHEN jsonb_typeof(sample.payload -> 'networks') = 'array'
                                    THEN sample.payload -> 'networks'
                                    ELSE '[]'::jsonb
                                END
                            ) AS network
                        ), 0), 9223372036854775807)::bigint AS network_rx_bytes,
                        LEAST(COALESCE((
                            SELECT sum((network ->> 'tx_bytes')::numeric)
                            FROM jsonb_array_elements(
                                CASE
                                    WHEN jsonb_typeof(sample.payload -> 'networks') = 'array'
                                    THEN sample.payload -> 'networks'
                                    ELSE '[]'::jsonb
                                END
                            ) AS network
                        ), 0), 9223372036854775807)::bigint AS network_tx_bytes,
                        LEAST(
                            NULLIF(sample.payload #>> '{connections,tcp}', '')::numeric,
                            9223372036854775807
                        )::bigint AS tcp_sockets,
                        LEAST(
                            NULLIF(sample.payload #>> '{connections,udp}', '')::numeric,
                            9223372036854775807
                        )::bigint AS udp_sockets
                    FROM telemetry_samples sample
                    WHERE sample.client_id = ANY($1::TEXT[])
                      AND sample.observed_at >= to_timestamp($2)
                      AND sample.observed_at <= to_timestamp($3)
                ), bucketed AS (
                    SELECT
                        client_id,
                        chart_epoch,
                        LEAST(count(*)::bigint, 2147483647)::integer AS sample_count,
                        LEAST(count(cpu_usage)::bigint, 2147483647)::integer
                            AS cpu_usage_sample_count,
                        avg(cpu_usage)::double precision AS cpu_usage_avg,
                        max(cpu_usage)::double precision AS cpu_usage_max,
                        max(cpu_cores)::integer AS cpu_cores_max,
                        avg(cpu_load_1)::double precision AS cpu_load_1_avg,
                        max(cpu_load_1)::double precision AS cpu_load_1_max,
                        avg(cpu_load_5)::double precision AS cpu_load_5_avg,
                        max(cpu_load_5)::double precision AS cpu_load_5_max,
                        avg(cpu_load_15)::double precision AS cpu_load_15_avg,
                        max(cpu_load_15)::double precision AS cpu_load_15_max,
                        max(memory_total_bytes)::bigint AS memory_total_bytes_max,
                        round(avg(memory_available_bytes::numeric))::bigint
                            AS memory_available_bytes_avg,
                        min(memory_available_bytes)::bigint AS memory_available_bytes_min,
                        avg(CASE WHEN memory_total_bytes = 0 THEN 0::double precision
                            ELSE (memory_total_bytes - memory_available_bytes)::double precision
                                / memory_total_bytes::double precision
                        END)::double precision AS memory_used_ratio_avg,
                        max(CASE WHEN memory_total_bytes = 0 THEN 0::double precision
                            ELSE (memory_total_bytes - memory_available_bytes)::double precision
                                / memory_total_bytes::double precision
                        END)::double precision AS memory_used_ratio_max,
                        LEAST(
                            count(*) FILTER (WHERE swap_total_bytes > 0),
                            2147483647
                        )::integer AS swap_sample_count,
                        max(swap_total_bytes)::bigint AS swap_total_bytes_max,
                        CASE
                            WHEN max(swap_total_bytes) = 0 THEN 0
                            ELSE round(avg(swap_available_bytes::numeric)
                                FILTER (WHERE swap_total_bytes > 0))::bigint
                        END AS swap_available_bytes_avg,
                        CASE
                            WHEN max(swap_total_bytes) = 0 THEN 0
                            ELSE min(swap_available_bytes)
                                FILTER (WHERE swap_total_bytes > 0)
                        END::bigint AS swap_available_bytes_min,
                        avg(CASE
                            WHEN swap_total_bytes > 0
                                THEN (swap_total_bytes - swap_available_bytes)::double precision
                                / swap_total_bytes::double precision
                            ELSE NULL
                        END)::double precision AS swap_used_ratio_avg,
                        max(CASE
                            WHEN swap_total_bytes > 0
                                THEN (swap_total_bytes - swap_available_bytes)::double precision
                                / swap_total_bytes::double precision
                            ELSE NULL
                        END)::double precision AS swap_used_ratio_max,
                        max(disk_total_bytes)::bigint AS disk_total_bytes_max,
                        round(avg(disk_available_bytes::numeric))::bigint
                            AS disk_available_bytes_avg,
                        min(disk_available_bytes)::bigint AS disk_available_bytes_min,
                        avg(CASE WHEN disk_total_bytes = 0 THEN 0::double precision
                            ELSE (disk_total_bytes - disk_available_bytes)::double precision
                                / disk_total_bytes::double precision
                        END)::double precision AS disk_used_ratio_avg,
                        max(CASE WHEN disk_total_bytes = 0 THEN 0::double precision
                            ELSE (disk_total_bytes - disk_available_bytes)::double precision
                                / disk_total_bytes::double precision
                        END)::double precision AS disk_used_ratio_max,
                        max(network_rx_bytes)::bigint AS network_rx_bytes_max,
                        max(network_tx_bytes)::bigint AS network_tx_bytes_max,
                        LEAST(count(tcp_sockets)::bigint, 2147483647)::integer
                            AS connections_sample_count,
                        (array_agg(tcp_sockets ORDER BY observed_at DESC)
                            FILTER (WHERE tcp_sockets IS NOT NULL))[1]
                            AS tcp_sockets_latest,
                        (array_agg(udp_sockets ORDER BY observed_at DESC)
                            FILTER (WHERE tcp_sockets IS NOT NULL))[1]
                            AS udp_sockets_latest,
                        max(observed_at) FILTER (WHERE tcp_sockets IS NOT NULL)
                            AS connections_observed_at,
                        max(observed_at) AS latest_observed_at
                    FROM expanded
                    GROUP BY client_id, chart_epoch
                ), ranked AS (
                    SELECT
                        bucketed.*,
                        row_number() OVER (
                            PARTITION BY client_id ORDER BY chart_epoch DESC
                        ) AS point_rank
                    FROM bucketed
                ), globally_bounded AS (
                    SELECT *
                    FROM ranked
                    WHERE point_rank <= $5
                    ORDER BY point_rank, chart_epoch DESC, client_id
                    LIMIT $6
                )
                SELECT
                    client_id,
                    to_timestamp(chart_epoch)::text AS bucket_start,
                    $4::integer AS bucket_secs,
                    sample_count,
                    cpu_usage_sample_count,
                    cpu_usage_avg,
                    cpu_usage_max,
                    cpu_cores_max,
                    cpu_load_1_avg,
                    cpu_load_1_max,
                    cpu_load_5_avg,
                    cpu_load_5_max,
                    cpu_load_15_avg,
                    cpu_load_15_max,
                    memory_total_bytes_max,
                    memory_available_bytes_avg,
                    memory_available_bytes_min,
                    memory_used_ratio_avg,
                    memory_used_ratio_max,
                    swap_sample_count,
                    swap_total_bytes_max,
                    swap_available_bytes_avg,
                    swap_available_bytes_min,
                    swap_used_ratio_avg,
                    swap_used_ratio_max,
                    disk_total_bytes_max,
                    disk_available_bytes_avg,
                    disk_available_bytes_min,
                    disk_used_ratio_avg,
                    disk_used_ratio_max,
                    network_rx_bytes_max,
                    network_tx_bytes_max,
                    connections_sample_count,
                    tcp_sockets_latest,
                    udp_sockets_latest,
                    connections_observed_at::text AS connections_observed_at,
                    latest_observed_at::text AS latest_observed_at,
                    latest_observed_at::text AS updated_at
                FROM globally_bounded
                ORDER BY chart_epoch, client_id
                "#,
            )
            .bind(client_ids)
            .bind(start_unix as i64)
            .bind(end_unix as i64)
            .bind(step_secs)
            .bind(points_per_client as i64)
            .bind(DASHBOARD_TELEMETRY_RESULT_LIMIT as i64)
            .fetch_all(pool)
            .await?;
            return rows.into_iter().map(telemetry_rollup_from_row).collect();
        }
        let samples = self
            .list_raw_telemetry_samples_for_clients(client_ids, start_unix, end_unix, usize::MAX)
            .await?;
        let rows = samples
            .into_iter()
            .map(raw_sample_rollup)
            .collect::<Result<Vec<_>>>()?;
        let mut rows = aggregate_memory_telemetry_rollups(rows, step_secs);
        retain_fair_rollup_points(
            &mut rows,
            points_per_client,
            DASHBOARD_TELEMETRY_RESULT_LIMIT,
        );
        Ok(rows)
    }

    #[cfg(test)]
    pub(crate) async fn list_dashboard_raw_telemetry_network_rates(
        &self,
        points_per_series: i64,
        start_unix: u64,
        end_unix: u64,
        step_secs: i32,
        client_ids: &[String],
    ) -> Result<Vec<TelemetryNetworkRateView>> {
        let selection = NetworkRateInterfaceSelection::all(client_ids);
        self.list_dashboard_raw_telemetry_network_rates_selected(
            points_per_series,
            start_unix,
            end_unix,
            step_secs,
            &selection,
        )
        .await
    }

    pub(crate) async fn list_dashboard_raw_telemetry_network_rates_selected(
        &self,
        points_per_series: i64,
        start_unix: u64,
        end_unix: u64,
        step_secs: i32,
        selection: &NetworkRateInterfaceSelection,
    ) -> Result<Vec<TelemetryNetworkRateView>> {
        if selection.is_empty() {
            return Ok(Vec::new());
        }
        let client_ids = selection.client_ids();
        let (all_client_ids, exact_client_ids, exact_interfaces) = selection.query_parts();
        let points_per_series = points_per_series.clamp(2, 1_440) as usize;
        let step_secs = normalized_dashboard_step_secs(step_secs);
        // Include one maximum telemetry interval before the visible range so
        // the first visible counter delta has an explicit baseline.
        let query_start = start_unix.saturating_sub(3_600);
        if let Self::Postgres(pool) = self {
            let rows = sqlx::query(
                r#"
                WITH expanded AS (
                    SELECT
                        sample.id AS sample_id,
                        sample.client_id,
                        network ->> 'interface' AS interface,
                        extract(epoch FROM sample.observed_at)::bigint AS sample_epoch,
                        sample.observed_at,
                        LEAST((network ->> 'rx_bytes')::numeric, 9223372036854775807)
                            ::bigint AS rx_bytes,
                        LEAST((network ->> 'tx_bytes')::numeric, 9223372036854775807)
                            ::bigint AS tx_bytes
                    FROM telemetry_samples sample
                    CROSS JOIN LATERAL jsonb_array_elements(
                        CASE
                            WHEN jsonb_typeof(sample.payload -> 'networks') = 'array'
                            THEN sample.payload -> 'networks'
                            ELSE '[]'::jsonb
                        END
                    ) AS network
                    WHERE (
                            sample.client_id = ANY($1::TEXT[])
                            OR EXISTS (
                                SELECT 1
                                FROM UNNEST($8::TEXT[], $9::TEXT[])
                                    AS selected(client_id, interface)
                                WHERE selected.client_id = sample.client_id
                                  AND selected.interface = network ->> 'interface'
                            )
                      )
                      AND sample.observed_at >= to_timestamp($2)
                      AND sample.observed_at <= to_timestamp($3)
                      AND length(network ->> 'interface') BETWEEN 1 AND 128
                ), sequenced AS (
                    SELECT
                        expanded.*,
                        lag(rx_bytes) OVER samples AS previous_rx_bytes,
                        lag(tx_bytes) OVER samples AS previous_tx_bytes
                    FROM expanded
                    WINDOW samples AS (
                        PARTITION BY client_id, interface
                        ORDER BY sample_epoch, observed_at, sample_id
                    )
                ), marked AS (
                    SELECT
                        sequenced.*,
                        sum(CASE
                            WHEN previous_rx_bytes IS NOT NULL
                                AND rx_bytes < previous_rx_bytes
                            THEN 1 ELSE 0
                        END) OVER samples AS rx_counter_epoch,
                        sum(CASE
                            WHEN previous_tx_bytes IS NOT NULL
                                AND tx_bytes < previous_tx_bytes
                            THEN 1 ELSE 0
                        END) OVER samples AS tx_counter_epoch
                    FROM sequenced
                    WINDOW samples AS (
                        PARTITION BY client_id, interface
                        ORDER BY sample_epoch, observed_at, sample_id
                        ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
                    )
                ), visible_bucketed AS (
                    SELECT
                        client_id,
                        interface,
                        floor(sample_epoch::numeric / $4::numeric)::bigint
                            * $4::bigint AS chart_epoch,
                        min(sample_epoch)::bigint AS first_sample_epoch,
                        max(sample_epoch)::bigint AS effective_epoch,
                        LEAST(count(*)::bigint, 2147483647)::integer AS sample_count,
                        (array_agg(rx_bytes ORDER BY sample_epoch DESC, observed_at DESC, sample_id DESC))[1]
                            AS rx_bytes,
                        (array_agg(tx_bytes ORDER BY sample_epoch DESC, observed_at DESC, sample_id DESC))[1]
                            AS tx_bytes,
                        (array_agg(rx_counter_epoch ORDER BY sample_epoch DESC, observed_at DESC, sample_id DESC))[1]
                            AS rx_counter_epoch,
                        (array_agg(tx_counter_epoch ORDER BY sample_epoch DESC, observed_at DESC, sample_id DESC))[1]
                            AS tx_counter_epoch,
                        max(observed_at) AS updated_at
                    FROM marked
                    WHERE sample_epoch >= $5
                    GROUP BY client_id, interface, chart_epoch
                ), series_start AS (
                    SELECT
                        client_id,
                        interface,
                        min(first_sample_epoch)::bigint AS first_effective_epoch,
                        min(chart_epoch)::bigint AS first_chart_epoch
                    FROM visible_bucketed
                    GROUP BY client_id, interface
                ), preceding AS (
                    SELECT
                        series_start.client_id,
                        series_start.interface,
                        series_start.first_chart_epoch - $4::bigint AS chart_epoch,
                        baseline.sample_epoch AS effective_epoch,
                        0::integer AS sample_count,
                        baseline.rx_bytes,
                        baseline.tx_bytes,
                        baseline.rx_counter_epoch,
                        baseline.tx_counter_epoch,
                        baseline.observed_at AS updated_at,
                        FALSE AS visible
                    FROM series_start
                    JOIN LATERAL (
                        SELECT
                            sample_epoch,
                            rx_bytes,
                            tx_bytes,
                            rx_counter_epoch,
                            tx_counter_epoch,
                            observed_at
                        FROM marked candidate
                        WHERE candidate.client_id = series_start.client_id
                          AND candidate.interface = series_start.interface
                          AND candidate.sample_epoch < series_start.first_effective_epoch
                        ORDER BY candidate.sample_epoch DESC, candidate.observed_at DESC
                        LIMIT 1
                    ) baseline ON TRUE
                ), combined AS (
                    SELECT
                        client_id,
                        interface,
                        chart_epoch,
                        effective_epoch,
                        sample_count,
                        rx_bytes,
                        tx_bytes,
                        rx_counter_epoch,
                        tx_counter_epoch,
                        updated_at,
                        TRUE AS visible
                    FROM visible_bucketed
                    UNION ALL
                    SELECT * FROM preceding
                ), derived AS (
                    SELECT
                        combined.*,
                        lag(effective_epoch) OVER series AS previous_effective_epoch,
                        lag(rx_bytes) OVER series AS previous_rx_bytes,
                        lag(tx_bytes) OVER series AS previous_tx_bytes,
                        lag(rx_counter_epoch) OVER series AS previous_rx_counter_epoch,
                        lag(tx_counter_epoch) OVER series AS previous_tx_counter_epoch
                    FROM combined
                    WINDOW series AS (
                        PARTITION BY client_id, interface
                        ORDER BY effective_epoch, chart_epoch
                    )
                ), bounded AS (
                    SELECT
                        client_id,
                        interface,
                        chart_epoch,
                        effective_epoch,
                        sample_count,
                        rx_bytes,
                        tx_bytes,
                        rx_counter_epoch,
                        tx_counter_epoch,
                        rx_bytes - previous_rx_bytes AS rx_delta,
                        tx_bytes - previous_tx_bytes AS tx_delta,
                        (
                            (rx_bytes - previous_rx_bytes) * 8
                        )::double precision / GREATEST(
                            effective_epoch - previous_effective_epoch,
                            1
                        )::double precision AS rx_bps,
                        (
                            (tx_bytes - previous_tx_bytes) * 8
                        )::double precision / GREATEST(
                            effective_epoch - previous_effective_epoch,
                            1
                        )::double precision AS tx_bps,
                        updated_at
                    FROM derived
                    WHERE visible
                      AND previous_effective_epoch IS NOT NULL
                      AND rx_counter_epoch = previous_rx_counter_epoch
                      AND tx_counter_epoch = previous_tx_counter_epoch
                      AND rx_bytes >= previous_rx_bytes
                      AND tx_bytes >= previous_tx_bytes
                ), ranked AS (
                    SELECT
                        bounded.*,
                        row_number() OVER (
                            PARTITION BY client_id, interface ORDER BY chart_epoch DESC
                        ) AS point_rank
                    FROM bounded
                ), globally_bounded AS (
                    SELECT *
                    FROM ranked
                    WHERE point_rank <= $6
                    ORDER BY point_rank, chart_epoch DESC, client_id, interface
                    LIMIT $7
                )
                SELECT
                    client_id,
                    interface,
                    to_timestamp(chart_epoch)::text AS bucket_start,
                    LEAST(
                        GREATEST(effective_epoch - chart_epoch + 60, 60),
                        2147483647
                    )::integer AS bucket_secs,
                    sample_count,
                    rx_bytes AS rx_bytes_avg,
                    tx_bytes AS tx_bytes_avg,
                    rx_bytes AS rx_bytes_last,
                    tx_bytes AS tx_bytes_last,
                    rx_counter_epoch,
                    tx_counter_epoch,
                    rx_delta AS rx_bytes_delta,
                    tx_delta AS tx_bytes_delta,
                    rx_bps AS rx_bps_avg,
                    tx_bps AS tx_bps_avg,
                    updated_at::text AS updated_at
                FROM globally_bounded
                ORDER BY chart_epoch, client_id, interface
                "#,
            )
            .bind(&all_client_ids)
            .bind(query_start as i64)
            .bind(end_unix as i64)
            .bind(step_secs)
            .bind(start_unix as i64)
            .bind(points_per_series as i64)
            .bind(DASHBOARD_TELEMETRY_RESULT_LIMIT as i64)
            .bind(&exact_client_ids)
            .bind(&exact_interfaces)
            .fetch_all(pool)
            .await?;
            let rows = rows
                .into_iter()
                .map(telemetry_network_rate_from_row)
                .collect::<Result<Vec<_>>>()?;
            return Ok(project_network_rate_selection(rows, selection));
        }
        let samples = self
            .list_raw_telemetry_samples_for_clients(&client_ids, query_start, end_unix, usize::MAX)
            .await?;
        let mut counters = Vec::new();
        for sample in samples {
            let metrics: AgentMetrics = serde_json::from_value(sample.payload.clone())
                .with_context(|| {
                    format!(
                        "invalid raw telemetry payload for {} at {}",
                        sample.client_id, sample.observed_at
                    )
                })?;
            for network in metrics.networks {
                if !selection.allows(&sample.client_id, &network.interface) {
                    continue;
                }
                if network.interface.is_empty() || network.interface.len() > 64 {
                    continue;
                }
                counters.push(TelemetryNetworkRateView {
                    client_id: sample.client_id.clone(),
                    interface: network.interface,
                    bucket_start: sample.observed_at.clone(),
                    bucket_secs: 60,
                    sample_count: 1,
                    rx_bytes_avg: saturating_u64_to_i64(network.rx_bytes),
                    tx_bytes_avg: saturating_u64_to_i64(network.tx_bytes),
                    rx_bytes_last: saturating_u64_to_i64(network.rx_bytes),
                    tx_bytes_last: saturating_u64_to_i64(network.tx_bytes),
                    rx_counter_epoch: 0,
                    tx_counter_epoch: 0,
                    rx_bytes_delta: 0,
                    tx_bytes_delta: 0,
                    rx_bps_avg: 0.0,
                    tx_bps_avg: 0.0,
                    updated_at: sample.observed_at.clone(),
                });
            }
        }
        let mut rows = derive_network_rates(aggregate_memory_network_rates(
            mark_memory_network_counter_epochs(counters),
            step_secs,
        ));
        rows.retain(|row| timestamp_in_bounds(&row.bucket_start, Some(start_unix), Some(end_unix)));
        retain_fair_network_points(
            &mut rows,
            points_per_series,
            DASHBOARD_TELEMETRY_RESULT_LIMIT,
        );
        Ok(project_network_rate_selection(rows, selection))
    }

    async fn list_raw_telemetry_samples_for_clients(
        &self,
        client_ids: &[String],
        start_unix: u64,
        end_unix: u64,
        samples_per_client: usize,
    ) -> Result<Vec<TelemetrySampleView>> {
        if client_ids.is_empty() || start_unix > end_unix {
            return Ok(Vec::new());
        }
        match self {
            Self::Memory(memory) => {
                let allowed = client_ids
                    .iter()
                    .map(String::as_str)
                    .collect::<HashSet<_>>();
                let mut rows = memory
                    .telemetry_samples
                    .read()
                    .await
                    .iter()
                    .filter(|sample| allowed.contains(sample.client_id.as_str()))
                    .filter(|sample| {
                        timestamp_in_bounds(&sample.observed_at, Some(start_unix), Some(end_unix))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                rows.sort_by(|left, right| {
                    left.client_id
                        .cmp(&right.client_id)
                        .then_with(|| {
                            compare_timestamps_desc(&left.observed_at, &right.observed_at)
                        })
                        .then_with(|| right.id.cmp(&left.id))
                });
                let mut counts = HashMap::<String, usize>::new();
                rows.retain(|row| {
                    let count = counts.entry(row.client_id.clone()).or_default();
                    let keep = *count < samples_per_client;
                    *count = count.saturating_add(1);
                    keep
                });
                rows.sort_by(|left, right| {
                    parse_timestamp_unix(&left.observed_at)
                        .cmp(&parse_timestamp_unix(&right.observed_at))
                        .then_with(|| left.client_id.cmp(&right.client_id))
                        .then_with(|| left.id.cmp(&right.id))
                });
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH ranked AS (
                        SELECT
                            id,
                            client_id,
                            observed_at,
                            cpu_load_1,
                            memory_total_bytes,
                            memory_available_bytes,
                            payload,
                            row_number() OVER (
                                PARTITION BY client_id
                                ORDER BY observed_at DESC, id DESC
                            ) AS sample_rank
                        FROM telemetry_samples
                        WHERE
                            client_id = ANY($1::TEXT[])
                            AND observed_at >= to_timestamp($2)
                            AND observed_at <= to_timestamp($3)
                    )
                    SELECT
                        id,
                        client_id,
                        observed_at::text AS observed_at,
                        cpu_load_1,
                        memory_total_bytes,
                        memory_available_bytes,
                        payload
                    FROM ranked
                    WHERE sample_rank <= $4
                    ORDER BY observed_at ASC, client_id ASC, id ASC
                    LIMIT $5
                    "#,
                )
                .bind(client_ids)
                .bind(start_unix as i64)
                .bind(end_unix as i64)
                .bind(samples_per_client as i64)
                .bind(DASHBOARD_TELEMETRY_RESULT_LIMIT as i64)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(TelemetrySampleView {
                            id: row.try_get("id")?,
                            client_id: row.try_get("client_id")?,
                            observed_at: row.try_get("observed_at")?,
                            cpu_load_1: row.try_get("cpu_load_1")?,
                            memory_total_bytes: row.try_get("memory_total_bytes")?,
                            memory_available_bytes: row.try_get("memory_available_bytes")?,
                            payload: row.try_get("payload")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn dashboard_telemetry_start_unix(
        &self,
        client_ids: &[String],
    ) -> Result<Option<u64>> {
        if client_ids.is_empty() {
            return Ok(None);
        }
        match self {
            Self::Memory(memory) => {
                let rollup_start = memory
                    .telemetry_rollups
                    .read()
                    .await
                    .iter()
                    .filter(|row| client_ids.contains(&row.client_id))
                    .filter_map(|row| parse_timestamp_unix(&row.bucket_start))
                    .min();
                let network_start = memory
                    .telemetry_network_rates
                    .read()
                    .await
                    .iter()
                    .filter(|row| client_ids.contains(&row.client_id))
                    .filter_map(|row| parse_timestamp_unix(&row.bucket_start))
                    .min();
                let current_ping_keys = {
                    let generations = memory
                        .ping_targets
                        .read()
                        .await
                        .iter()
                        .map(|target| (target.id, target.generation))
                        .collect::<std::collections::HashMap<_, _>>();
                    memory
                        .ping_target_assignments
                        .read()
                        .await
                        .iter()
                        .filter(|assignment| client_ids.contains(&assignment.client_id))
                        .filter_map(|assignment| {
                            generations.get(&assignment.target_id).map(|generation| {
                                (
                                    assignment.client_id.clone(),
                                    assignment.target_id,
                                    *generation,
                                )
                            })
                        })
                        .collect::<std::collections::HashSet<_>>()
                };
                let ping_start = memory
                    .telemetry_ping_rollups
                    .read()
                    .await
                    .iter()
                    .filter(|row| {
                        current_ping_keys.contains(&(
                            row.client_id.clone(),
                            row.target_id,
                            row.generation,
                        ))
                    })
                    .filter_map(|row| parse_timestamp_unix(&row.bucket_start))
                    .min();
                Ok(rollup_start
                    .into_iter()
                    .chain(network_start)
                    .chain(ping_start)
                    .min())
            }
            Self::Postgres(pool) => {
                let value = sqlx::query_scalar::<_, Option<f64>>(
                    r#"
                    SELECT extract(epoch FROM min(first_bucket))::double precision
                    FROM (
                        SELECT min(bucket_start) AS first_bucket
                        FROM telemetry_rollups
                        WHERE client_id = ANY($1::TEXT[])
                        UNION ALL
                        SELECT min(bucket_start) AS first_bucket
                        FROM telemetry_network_rates
                        WHERE client_id = ANY($1::TEXT[])
                        UNION ALL
                        SELECT min(p.bucket_start) AS first_bucket
                        FROM telemetry_ping_rollups p
                        JOIN ping_targets t
                          ON t.id = p.target_id AND t.generation = p.generation
                        JOIN ping_target_assignments a
                          ON a.target_id = p.target_id AND a.client_id = p.client_id
                        WHERE p.client_id = ANY($1::TEXT[])
                    ) AS bounds
                    "#,
                )
                .bind(client_ids)
                .fetch_one(pool)
                .await?;
                Ok(value
                    .filter(|value| value.is_finite() && *value >= 0.0)
                    .map(|value| value as u64))
            }
        }
    }

    pub(crate) async fn list_dashboard_telemetry_rollups(
        &self,
        points_per_client: i64,
        start_unix: Option<u64>,
        end_unix: Option<u64>,
        bucket_secs: Option<i32>,
        step_secs: i32,
        client_ids: &[String],
    ) -> Result<Vec<TelemetryRollupView>> {
        if client_ids.is_empty() {
            return Ok(Vec::new());
        }
        let step_secs = normalized_dashboard_step_secs(step_secs);
        let points_per_client = points_per_client.clamp(2, 1_440) as usize;
        match self {
            Self::Memory(memory) => {
                let rows = memory
                    .telemetry_rollups
                    .read()
                    .await
                    .iter()
                    .filter(|rollup| {
                        client_ids.contains(&rollup.client_id)
                            && bucket_secs
                                .is_none_or(|bucket_secs| rollup.bucket_secs == bucket_secs)
                            && bucket_overlaps_bounds(
                                &rollup.bucket_start,
                                rollup.bucket_secs,
                                start_unix,
                                end_unix,
                            )
                    })
                    .cloned()
                    .flat_map(|rollup| {
                        fragment_telemetry_rollup(rollup, start_unix, end_unix, step_secs)
                    })
                    .collect::<Vec<_>>();
                let mut rows = aggregate_memory_telemetry_rollups(rows, step_secs);
                retain_fair_rollup_points(
                    &mut rows,
                    points_per_client,
                    DASHBOARD_TELEMETRY_RESULT_LIMIT,
                );
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH candidates AS (
                        SELECT
                            client_id,
                            extract(epoch FROM bucket_start)::bigint AS source_start,
                            (bucket_secs / 60)::bigint AS source_minutes,
                            sample_count,
                            cpu_usage_sample_count,
                            cpu_usage_avg,
                            cpu_usage_max,
                            cpu_cores_max,
                            cpu_load_1_avg,
                            cpu_load_1_max,
                            cpu_load_5_avg,
                            cpu_load_5_max,
                            cpu_load_15_avg,
                            cpu_load_15_max,
                            memory_total_bytes_max,
                            memory_available_bytes_avg,
                            memory_available_bytes_min,
                            memory_used_ratio_avg,
                            memory_used_ratio_max,
                            swap_sample_count,
                            swap_total_bytes_max,
                            swap_available_bytes_avg,
                            swap_available_bytes_min,
                            swap_used_ratio_avg,
                            swap_used_ratio_max,
                            disk_total_bytes_max,
                            disk_available_bytes_avg,
                            disk_available_bytes_min,
                            disk_used_ratio_avg,
                            disk_used_ratio_max,
                            network_rx_bytes_max,
                            network_tx_bytes_max,
                            connections_sample_count,
                            tcp_sockets_latest,
                            udp_sockets_latest,
                            connections_observed_at,
                            latest_observed_at,
                            updated_at
                        FROM telemetry_rollups
                        WHERE
                            ($1::INTEGER IS NULL OR bucket_secs = $1)
                            AND bucket_secs >= 60
                            AND bucket_secs % 60 = 0
                            AND ($2::BIGINT IS NULL OR bucket_start
                                + make_interval(secs => bucket_secs - 60) >= to_timestamp($2))
                            AND ($3::BIGINT IS NULL OR bucket_start <= to_timestamp($3))
                            AND client_id = ANY($6::TEXT[])
                    ), physical AS (
                        SELECT
                            candidates.*,
                            CASE
                                WHEN $2::BIGINT IS NULL OR $2 <= source_start THEN 0::bigint
                                ELSE LEAST(
                                    source_minutes,
                                    ($2 - source_start + 59) / 60
                                )
                            END AS first_minute,
                            CASE
                                WHEN $3::BIGINT IS NULL THEN source_minutes
                                WHEN $3 < source_start THEN 0::bigint
                                ELSE LEAST(
                                    source_minutes,
                                    ($3 - source_start) / 60 + 1
                                )
                            END AS end_minute
                        FROM candidates
                    ), fragments AS (
                        SELECT
                            physical.*,
                            chart_epoch,
                            GREATEST(
                                first_minute,
                                ceil((chart_epoch - source_start)::numeric / 60)::bigint
                            ) AS fragment_first_minute,
                            LEAST(
                                end_minute,
                                ceil((chart_epoch + $4::bigint - source_start)::numeric / 60)::bigint
                            ) AS fragment_end_minute
                        FROM physical
                        CROSS JOIN LATERAL generate_series(
                            floor(
                                (source_start + first_minute * 60)::numeric
                                    / $4::numeric
                            )::bigint * $4::bigint,
                            floor(
                                (source_start + (end_minute - 1) * 60)::numeric
                                    / $4::numeric
                            )::bigint * $4::bigint,
                            $4::bigint
                        ) AS generated(chart_epoch)
                        WHERE first_minute < end_minute
                    ), selected AS (
                        SELECT
                            client_id,
                            to_timestamp(chart_epoch) AS chart_bucket_start,
                            (
                                sample_count::bigint * fragment_end_minute / source_minutes
                                - sample_count::bigint * fragment_first_minute / source_minutes
                            )::integer AS sample_count,
                            (
                                cpu_usage_sample_count::bigint * fragment_end_minute / source_minutes
                                - cpu_usage_sample_count::bigint * fragment_first_minute / source_minutes
                            )::integer AS cpu_usage_sample_count,
                            cpu_usage_avg,
                            cpu_usage_max,
                            cpu_cores_max,
                            cpu_load_1_avg,
                            cpu_load_1_max,
                            cpu_load_5_avg,
                            cpu_load_5_max,
                            cpu_load_15_avg,
                            cpu_load_15_max,
                            memory_total_bytes_max,
                            memory_available_bytes_avg,
                            memory_available_bytes_min,
                            memory_used_ratio_avg,
                            memory_used_ratio_max,
                            (
                                swap_sample_count::bigint * fragment_end_minute / source_minutes
                                - swap_sample_count::bigint * fragment_first_minute / source_minutes
                            )::integer AS swap_sample_count,
                            CASE
                                WHEN swap_sample_count::bigint * fragment_end_minute / source_minutes
                                    - swap_sample_count::bigint * fragment_first_minute / source_minutes > 0
                                    OR (swap_sample_count = 0 AND swap_total_bytes_max = 0)
                                    THEN swap_total_bytes_max
                                ELSE NULL
                            END AS swap_total_bytes_max,
                            CASE
                                WHEN swap_sample_count::bigint * fragment_end_minute / source_minutes
                                    - swap_sample_count::bigint * fragment_first_minute / source_minutes > 0
                                    OR (swap_sample_count = 0 AND swap_total_bytes_max = 0)
                                    THEN swap_available_bytes_avg
                                ELSE NULL
                            END AS swap_available_bytes_avg,
                            CASE
                                WHEN swap_sample_count::bigint * fragment_end_minute / source_minutes
                                    - swap_sample_count::bigint * fragment_first_minute / source_minutes > 0
                                    OR (swap_sample_count = 0 AND swap_total_bytes_max = 0)
                                    THEN swap_available_bytes_min
                                ELSE NULL
                            END AS swap_available_bytes_min,
                            CASE
                                WHEN swap_sample_count::bigint * fragment_end_minute / source_minutes
                                    - swap_sample_count::bigint * fragment_first_minute / source_minutes > 0
                                    THEN swap_used_ratio_avg
                                ELSE NULL
                            END AS swap_used_ratio_avg,
                            CASE
                                WHEN swap_sample_count::bigint * fragment_end_minute / source_minutes
                                    - swap_sample_count::bigint * fragment_first_minute / source_minutes > 0
                                    THEN swap_used_ratio_max
                                ELSE NULL
                            END AS swap_used_ratio_max,
                            disk_total_bytes_max,
                            disk_available_bytes_avg,
                            disk_available_bytes_min,
                            disk_used_ratio_avg,
                            disk_used_ratio_max,
                            network_rx_bytes_max,
                            network_tx_bytes_max,
                            (
                                connections_sample_count::bigint * fragment_end_minute / source_minutes
                                - connections_sample_count::bigint * fragment_first_minute / source_minutes
                            )::integer AS connections_sample_count,
                            tcp_sockets_latest,
                            udp_sockets_latest,
                            CASE
                                WHEN connections_sample_count::bigint * fragment_end_minute / source_minutes
                                    - connections_sample_count::bigint * fragment_first_minute / source_minutes = 0
                                    THEN NULL
                                ELSE connections_observed_at - make_interval(
                                    secs => (
                                        (source_minutes - fragment_end_minute) * 60
                                    )::double precision
                                )
                            END AS connections_observed_at,
                            latest_observed_at - make_interval(
                                secs => (
                                    (source_minutes - fragment_end_minute) * 60
                                )::double precision
                            ) AS latest_observed_at,
                            updated_at
                        FROM fragments
                        WHERE sample_count::bigint * fragment_end_minute / source_minutes
                            - sample_count::bigint * fragment_first_minute / source_minutes > 0
                    ),
                    bucketed AS (
                        SELECT
                            client_id,
                            chart_bucket_start,
                            $4::INTEGER AS bucket_secs,
                            LEAST(sum(sample_count)::bigint, 2147483647)::integer AS sample_count,
                            LEAST(sum(cpu_usage_sample_count)::bigint, 2147483647)::integer
                                AS cpu_usage_sample_count,
                            sum(cpu_usage_avg * cpu_usage_sample_count::double precision)
                                / NULLIF(sum(cpu_usage_sample_count)::double precision, 0)
                                AS cpu_usage_avg,
                            max(cpu_usage_max)::double precision AS cpu_usage_max,
                            max(cpu_cores_max)::integer AS cpu_cores_max,
                            COALESCE(
                                sum(cpu_load_1_avg * sample_count::double precision)
                                    / NULLIF(sum(sample_count)::double precision, 0),
                                0
                            ) AS cpu_load_1_avg,
                            max(cpu_load_1_max)::double precision AS cpu_load_1_max,
                            COALESCE(
                                sum(cpu_load_5_avg * sample_count::double precision)
                                    / NULLIF(sum(sample_count)::double precision, 0),
                                0
                            ) AS cpu_load_5_avg,
                            max(cpu_load_5_max)::double precision AS cpu_load_5_max,
                            COALESCE(
                                sum(cpu_load_15_avg * sample_count::double precision)
                                    / NULLIF(sum(sample_count)::double precision, 0),
                                0
                            ) AS cpu_load_15_avg,
                            max(cpu_load_15_max)::double precision AS cpu_load_15_max,
                            max(memory_total_bytes_max)::bigint AS memory_total_bytes_max,
                            round(COALESCE(
                                sum(memory_available_bytes_avg::numeric * sample_count::numeric)
                                    / NULLIF(sum(sample_count)::numeric, 0),
                                0
                            ))::bigint AS memory_available_bytes_avg,
                            min(memory_available_bytes_min)::bigint
                                AS memory_available_bytes_min,
                            COALESCE(
                                sum(memory_used_ratio_avg::numeric * sample_count::numeric)
                                    / NULLIF(sum(sample_count)::numeric, 0),
                                0
                            )::double precision AS memory_used_ratio_avg,
                            max(memory_used_ratio_max)::double precision AS memory_used_ratio_max,
                            LEAST(sum(swap_sample_count)::bigint, 2147483647)::integer
                                AS swap_sample_count,
                            max(swap_total_bytes_max)::bigint AS swap_total_bytes_max,
                            CASE
                                WHEN sum(swap_sample_count) > 0 THEN round(
                                    sum(swap_available_bytes_avg::numeric
                                        * swap_sample_count::numeric)
                                        / sum(swap_sample_count)::numeric
                                )::bigint
                                WHEN max(swap_total_bytes_max) = 0 THEN 0
                                ELSE NULL
                            END AS swap_available_bytes_avg,
                            CASE
                                WHEN sum(swap_sample_count) > 0 THEN
                                    (min(swap_available_bytes_min)
                                        FILTER (WHERE swap_sample_count > 0))::bigint
                                WHEN max(swap_total_bytes_max) = 0 THEN 0
                                ELSE NULL
                            END AS swap_available_bytes_min,
                            (
                                sum(swap_used_ratio_avg::numeric * swap_sample_count::numeric)
                                    / NULLIF(sum(swap_sample_count)::numeric, 0)
                            )::double precision AS swap_used_ratio_avg,
                            (max(swap_used_ratio_max)
                                FILTER (WHERE swap_sample_count > 0))::double precision
                                AS swap_used_ratio_max,
                            max(disk_total_bytes_max)::bigint AS disk_total_bytes_max,
                            round(COALESCE(
                                sum(disk_available_bytes_avg::numeric * sample_count::numeric)
                                    / NULLIF(sum(sample_count)::numeric, 0),
                                0
                            ))::bigint AS disk_available_bytes_avg,
                            min(disk_available_bytes_min)::bigint
                                AS disk_available_bytes_min,
                            COALESCE(
                                sum(disk_used_ratio_avg::numeric * sample_count::numeric)
                                    / NULLIF(sum(sample_count)::numeric, 0),
                                0
                            )::double precision AS disk_used_ratio_avg,
                            max(disk_used_ratio_max)::double precision AS disk_used_ratio_max,
                            max(network_rx_bytes_max)::bigint AS network_rx_bytes_max,
                            max(network_tx_bytes_max)::bigint AS network_tx_bytes_max,
                            LEAST(sum(connections_sample_count)::bigint, 2147483647)::integer
                                AS connections_sample_count,
                            (array_agg(tcp_sockets_latest ORDER BY connections_observed_at DESC)
                                FILTER (WHERE connections_observed_at IS NOT NULL))[1]
                                AS tcp_sockets_latest,
                            (array_agg(udp_sockets_latest ORDER BY connections_observed_at DESC)
                                FILTER (WHERE connections_observed_at IS NOT NULL))[1]
                                AS udp_sockets_latest,
                            max(connections_observed_at)::text AS connections_observed_at,
                            max(latest_observed_at)::text AS latest_observed_at,
                            max(updated_at)::text AS updated_at
                        FROM selected
                        GROUP BY client_id, chart_bucket_start
                    ), ranked AS (
                        SELECT
                            bucketed.*,
                            row_number() OVER (
                                PARTITION BY client_id
                                ORDER BY chart_bucket_start DESC
                            ) AS point_rank
                        FROM bucketed
                    ), globally_bounded AS (
                        SELECT *
                        FROM ranked
                        WHERE point_rank <= $5
                        ORDER BY
                            point_rank ASC,
                            chart_bucket_start DESC,
                            client_id ASC
                        LIMIT $7
                    )
                    SELECT
                        client_id,
                        chart_bucket_start::text AS bucket_start,
                        bucket_secs,
                        sample_count,
                        cpu_usage_sample_count,
                        cpu_usage_avg,
                        cpu_usage_max,
                        cpu_cores_max,
                        cpu_load_1_avg,
                        cpu_load_1_max,
                        cpu_load_5_avg,
                        cpu_load_5_max,
                        cpu_load_15_avg,
                        cpu_load_15_max,
                        memory_total_bytes_max,
                        memory_available_bytes_avg,
                        memory_available_bytes_min,
                        memory_used_ratio_avg,
                        memory_used_ratio_max,
                        swap_sample_count,
                        swap_total_bytes_max,
                        swap_available_bytes_avg,
                        swap_available_bytes_min,
                        swap_used_ratio_avg,
                        swap_used_ratio_max,
                        disk_total_bytes_max,
                        disk_available_bytes_avg,
                        disk_available_bytes_min,
                        disk_used_ratio_avg,
                        disk_used_ratio_max,
                        network_rx_bytes_max,
                        network_tx_bytes_max,
                        connections_sample_count,
                        tcp_sockets_latest,
                        udp_sockets_latest,
                        connections_observed_at,
                        latest_observed_at,
                        updated_at
                    FROM globally_bounded
                    ORDER BY chart_bucket_start ASC, client_id ASC
                    "#,
                )
                .bind(bucket_secs)
                .bind(start_unix.map(|value| value as i64))
                .bind(end_unix.map(|value| value as i64))
                .bind(step_secs)
                .bind(points_per_client as i64)
                .bind(client_ids)
                .bind(DASHBOARD_TELEMETRY_RESULT_LIMIT as i64)
                .fetch_all(pool)
                .await?;

                rows.into_iter().map(telemetry_rollup_from_row).collect()
            }
        }
    }

    pub(crate) async fn list_telemetry_rollups(
        &self,
        limit: i64,
        client_id: Option<&str>,
        bucket_secs: Option<i32>,
        visible_only: bool,
    ) -> Result<Vec<TelemetryRollupView>> {
        match self {
            Self::Memory(memory) => {
                let hidden = memory.hidden_clients.read().await;
                let mut rows = memory
                    .telemetry_rollups
                    .read()
                    .await
                    .iter()
                    .filter(|rollup| {
                        (!visible_only || !hidden.contains(&rollup.client_id))
                            && client_id.is_none_or(|client_id| rollup.client_id == client_id)
                            && bucket_secs
                                .is_none_or(|bucket_secs| rollup.bucket_secs == bucket_secs)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                rows.sort_by(|left, right| {
                    right
                        .bucket_start
                        .cmp(&left.bucket_start)
                        .then_with(|| left.client_id.cmp(&right.client_id))
                });
                rows.truncate(limit.clamp(1, TELEMETRY_LIST_LIMIT_MAX) as usize);
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        client_id,
                        bucket_start::text AS bucket_start,
                        bucket_secs,
                        sample_count,
                        cpu_usage_sample_count,
                        cpu_usage_avg,
                        cpu_usage_max,
                        cpu_cores_max,
                        cpu_load_1_avg,
                        cpu_load_1_max,
                        cpu_load_5_avg,
                        cpu_load_5_max,
                        cpu_load_15_avg,
                        cpu_load_15_max,
                        memory_total_bytes_max,
                        memory_available_bytes_avg,
                        memory_available_bytes_min,
                        memory_used_ratio_avg,
                        memory_used_ratio_max,
                        swap_sample_count,
                        swap_total_bytes_max,
                        swap_available_bytes_avg,
                        swap_available_bytes_min,
                        swap_used_ratio_avg,
                        swap_used_ratio_max,
                        disk_total_bytes_max,
                        disk_available_bytes_avg,
                        disk_available_bytes_min,
                        disk_used_ratio_avg,
                        disk_used_ratio_max,
                        network_rx_bytes_max,
                        network_tx_bytes_max,
                        connections_sample_count,
                        tcp_sockets_latest,
                        udp_sockets_latest,
                        connections_observed_at::text AS connections_observed_at,
                        latest_observed_at::text AS latest_observed_at,
                        updated_at::text AS updated_at
                    FROM telemetry_rollups
                    WHERE
                        ($1::TEXT IS NULL OR client_id = $1)
                        AND ($2::INTEGER IS NULL OR bucket_secs = $2)
                        AND (
                            NOT $3
                            OR EXISTS (
                                SELECT 1 FROM visible_clients
                                WHERE visible_clients.id = telemetry_rollups.client_id
                            )
                        )
                    ORDER BY bucket_start DESC, client_id ASC
                    LIMIT $4
                    "#,
                )
                .bind(client_id)
                .bind(bucket_secs)
                .bind(visible_only)
                .bind(limit.clamp(1, TELEMETRY_LIST_LIMIT_MAX))
                .fetch_all(pool)
                .await?;

                rows.into_iter().map(telemetry_rollup_from_row).collect()
            }
        }
    }

    pub(crate) async fn list_latest_telemetry_rollups(
        &self,
        limit: i64,
        client_id: Option<&str>,
        bucket_secs: Option<i32>,
    ) -> Result<Vec<TelemetryRollupView>> {
        self.list_latest_telemetry_rollups_matching(
            Some(limit.clamp(1, TELEMETRY_LIST_LIMIT_MAX) as usize),
            client_id,
            None,
            bucket_secs,
        )
        .await
    }

    pub(crate) async fn list_latest_telemetry_rollups_for_clients(
        &self,
        client_ids: &[String],
        bucket_secs: Option<i32>,
    ) -> Result<Vec<TelemetryRollupView>> {
        // Policy evaluation must cover its complete, already-resolved target
        // set. Keep this internal and require concrete client IDs rather than
        // widening the page-bounded public telemetry query.
        if client_ids.is_empty() {
            return Ok(Vec::new());
        }
        self.list_latest_telemetry_rollups_matching(None, None, Some(client_ids), bucket_secs)
            .await
    }

    async fn list_latest_telemetry_rollups_matching(
        &self,
        result_limit: Option<usize>,
        client_id: Option<&str>,
        client_ids: Option<&[String]>,
        bucket_secs: Option<i32>,
    ) -> Result<Vec<TelemetryRollupView>> {
        match self {
            Self::Memory(memory) => {
                let hidden = memory.hidden_clients.read().await;
                let allowed_client_ids = client_ids.map(|client_ids| {
                    client_ids
                        .iter()
                        .map(String::as_str)
                        .collect::<HashSet<_>>()
                });
                let mut latest = HashMap::<String, TelemetryRollupView>::new();
                for rollup in memory
                    .telemetry_rollups
                    .read()
                    .await
                    .iter()
                    .filter(|rollup| {
                        !hidden.contains(&rollup.client_id)
                            && client_id.is_none_or(|client_id| rollup.client_id == client_id)
                            && allowed_client_ids.as_ref().is_none_or(|client_ids| {
                                client_ids.contains(rollup.client_id.as_str())
                            })
                            && bucket_secs
                                .is_none_or(|bucket_secs| rollup.bucket_secs == bucket_secs)
                    })
                {
                    let replace = latest.get(&rollup.client_id).is_none_or(|current| {
                        (
                            parse_timestamp_unix(&current.bucket_start).unwrap_or(0),
                            parse_timestamp_unix(&current.latest_observed_at).unwrap_or(0),
                        ) < (
                            parse_timestamp_unix(&rollup.bucket_start).unwrap_or(0),
                            parse_timestamp_unix(&rollup.latest_observed_at).unwrap_or(0),
                        )
                    });
                    if replace {
                        latest.insert(rollup.client_id.clone(), rollup.clone());
                    }
                }
                let mut rows = latest.into_values().collect::<Vec<_>>();
                rows.sort_by(|left, right| {
                    parse_timestamp_unix(&right.latest_observed_at)
                        .unwrap_or(0)
                        .cmp(&parse_timestamp_unix(&left.latest_observed_at).unwrap_or(0))
                        .then_with(|| left.client_id.cmp(&right.client_id))
                });
                if let Some(result_limit) = result_limit {
                    rows.truncate(result_limit);
                }
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH latest AS (
                        SELECT DISTINCT ON (client_id)
                            client_id,
                            bucket_start,
                            bucket_secs,
                            sample_count,
                            cpu_usage_sample_count,
                            cpu_usage_avg,
                            cpu_usage_max,
                            cpu_cores_max,
                            cpu_load_1_avg,
                            cpu_load_1_max,
                            cpu_load_5_avg,
                            cpu_load_5_max,
                            cpu_load_15_avg,
                            cpu_load_15_max,
                            memory_total_bytes_max,
                            memory_available_bytes_avg,
                            memory_available_bytes_min,
                            memory_used_ratio_avg,
                            memory_used_ratio_max,
                            swap_sample_count,
                            swap_total_bytes_max,
                            swap_available_bytes_avg,
                            swap_available_bytes_min,
                            swap_used_ratio_avg,
                            swap_used_ratio_max,
                            disk_total_bytes_max,
                            disk_available_bytes_avg,
                            disk_available_bytes_min,
                            disk_used_ratio_avg,
                            disk_used_ratio_max,
                            network_rx_bytes_max,
                            network_tx_bytes_max,
                            connections_sample_count,
                            tcp_sockets_latest,
                            udp_sockets_latest,
                            connections_observed_at,
                            latest_observed_at,
                            updated_at
                        FROM telemetry_rollups
                        WHERE
                            EXISTS (
                                SELECT 1 FROM visible_clients
                                WHERE visible_clients.id = telemetry_rollups.client_id
                            )
                            AND ($1::TEXT IS NULL OR client_id = $1)
                            AND ($2::TEXT[] IS NULL OR client_id = ANY($2))
                            AND ($3::INTEGER IS NULL OR bucket_secs = $3)
                        ORDER BY client_id, bucket_start DESC, latest_observed_at DESC, bucket_secs ASC
                    )
                    SELECT
                        client_id,
                        bucket_start::text AS bucket_start,
                        bucket_secs,
                        sample_count,
                        cpu_usage_sample_count,
                        cpu_usage_avg,
                        cpu_usage_max,
                        cpu_cores_max,
                        cpu_load_1_avg,
                        cpu_load_1_max,
                        cpu_load_5_avg,
                        cpu_load_5_max,
                        cpu_load_15_avg,
                        cpu_load_15_max,
                        memory_total_bytes_max,
                        memory_available_bytes_avg,
                        memory_available_bytes_min,
                        memory_used_ratio_avg,
                        memory_used_ratio_max,
                        swap_sample_count,
                        swap_total_bytes_max,
                        swap_available_bytes_avg,
                        swap_available_bytes_min,
                        swap_used_ratio_avg,
                        swap_used_ratio_max,
                        disk_total_bytes_max,
                        disk_available_bytes_avg,
                        disk_available_bytes_min,
                        disk_used_ratio_avg,
                        disk_used_ratio_max,
                        network_rx_bytes_max,
                        network_tx_bytes_max,
                        connections_sample_count,
                        tcp_sockets_latest,
                        udp_sockets_latest,
                        connections_observed_at::text AS connections_observed_at,
                        latest_observed_at::text AS latest_observed_at,
                        updated_at::text AS updated_at
                    FROM latest
                    ORDER BY latest_observed_at DESC, client_id ASC
                    LIMIT $4
                    "#,
                )
                .bind(client_id)
                .bind(client_ids)
                .bind(bucket_secs)
                .bind(result_limit.map(|limit| limit as i64))
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(telemetry_rollup_from_row).collect()
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn list_dashboard_telemetry_network_rates(
        &self,
        points_per_series: i64,
        start_unix: Option<u64>,
        end_unix: Option<u64>,
        bucket_secs: Option<i32>,
        step_secs: i32,
        client_ids: &[String],
    ) -> Result<Vec<TelemetryNetworkRateView>> {
        let selection = NetworkRateInterfaceSelection::all(client_ids);
        self.list_dashboard_telemetry_network_rates_selected(
            points_per_series,
            start_unix,
            end_unix,
            bucket_secs,
            step_secs,
            &selection,
        )
        .await
    }

    pub(crate) async fn list_dashboard_telemetry_network_rates_selected(
        &self,
        points_per_series: i64,
        start_unix: Option<u64>,
        end_unix: Option<u64>,
        bucket_secs: Option<i32>,
        step_secs: i32,
        selection: &NetworkRateInterfaceSelection,
    ) -> Result<Vec<TelemetryNetworkRateView>> {
        if selection.is_empty() {
            return Ok(Vec::new());
        }
        let (all_client_ids, exact_client_ids, exact_interfaces) = selection.query_parts();
        let step_secs = normalized_dashboard_step_secs(step_secs);
        let points_per_series = points_per_series.clamp(2, 1_440) as usize;
        match self {
            Self::Memory(memory) => {
                let rows = memory
                    .telemetry_network_rates
                    .read()
                    .await
                    .iter()
                    .filter(|rate| {
                        selection.allows(&rate.client_id, &rate.interface)
                            && bucket_secs.is_none_or(|bucket_secs| rate.bucket_secs == bucket_secs)
                            && end_unix.is_none_or(|end| {
                                parse_timestamp_unix(&rate.bucket_start)
                                    .is_some_and(|timestamp| timestamp <= end)
                            })
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let rows = select_dashboard_network_rows(rows, start_unix, end_unix, step_secs);
                let mut rows = derive_network_rates(rows);
                rows.retain(|rate| rate.sample_count > 0);
                retain_fair_network_points(
                    &mut rows,
                    points_per_series,
                    DASHBOARD_TELEMETRY_RESULT_LIMIT,
                );
                Ok(project_network_rate_selection(rows, selection))
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH candidates AS (
                        SELECT
                            client_id,
                            interface,
                            extract(epoch FROM bucket_start)::bigint AS source_start,
                            (bucket_secs / 60)::bigint AS source_minutes,
                            sample_count,
                            rx_bytes_avg,
                            tx_bytes_avg,
                            rx_bytes_last,
                            tx_bytes_last,
                            rx_counter_epoch,
                            tx_counter_epoch,
                            updated_at
                        FROM telemetry_network_rates
                        WHERE
                            ($1::INTEGER IS NULL OR bucket_secs = $1)
                            AND bucket_secs >= 60
                            AND bucket_secs % 60 = 0
                            AND ($2::BIGINT IS NULL OR bucket_start
                                + make_interval(secs => bucket_secs - 60) >= to_timestamp($2))
                            AND ($3::BIGINT IS NULL OR bucket_start <= to_timestamp($3))
                            AND (
                                client_id = ANY($6::TEXT[])
                                OR EXISTS (
                                    SELECT 1
                                    FROM UNNEST($8::TEXT[], $9::TEXT[])
                                        AS selected(client_id, interface)
                                    WHERE selected.client_id = telemetry_network_rates.client_id
                                      AND selected.interface = telemetry_network_rates.interface
                                )
                            )
                    ), physical AS (
                        SELECT
                            candidates.*,
                            CASE
                                WHEN $2::BIGINT IS NULL OR $2 <= source_start THEN 0::bigint
                                ELSE LEAST(
                                    source_minutes,
                                    ($2 - source_start + 59) / 60
                                )
                            END AS first_minute,
                            CASE
                                WHEN $3::BIGINT IS NULL THEN source_minutes
                                WHEN $3 < source_start THEN 0::bigint
                                ELSE LEAST(
                                    source_minutes,
                                    ($3 - source_start) / 60 + 1
                                )
                            END AS end_minute
                        FROM candidates
                    ), fragments AS (
                        SELECT
                            physical.*,
                            chart_epoch,
                            GREATEST(
                                first_minute,
                                ceil((chart_epoch - source_start)::numeric / 60)::bigint
                            ) AS fragment_first_minute,
                            LEAST(
                                end_minute,
                                ceil((chart_epoch + $4::bigint - source_start)::numeric / 60)::bigint
                            ) AS fragment_end_minute
                        FROM physical
                        CROSS JOIN LATERAL generate_series(
                            floor(
                                (source_start + first_minute * 60)::numeric
                                    / $4::numeric
                            )::bigint * $4::bigint,
                            floor(
                                (source_start + (end_minute - 1) * 60)::numeric
                                    / $4::numeric
                            )::bigint * $4::bigint,
                            $4::bigint
                        ) AS generated(chart_epoch)
                        WHERE first_minute < end_minute
                    ), selected AS (
                        SELECT
                            client_id,
                            interface,
                            chart_epoch,
                            source_start + (fragment_first_minute * 60) AS first_sample_epoch,
                            source_start + ((fragment_end_minute - 1) * 60) AS effective_epoch,
                            (
                                sample_count::bigint * fragment_end_minute / source_minutes
                                - sample_count::bigint * fragment_first_minute / source_minutes
                            )::integer AS sample_count,
                            rx_bytes_avg,
                            tx_bytes_avg,
                            rx_bytes_last,
                            tx_bytes_last,
                            rx_counter_epoch,
                            tx_counter_epoch,
                            updated_at
                        FROM fragments
                        WHERE sample_count::bigint * fragment_end_minute / source_minutes
                            - sample_count::bigint * fragment_first_minute / source_minutes > 0
                    ), bucketed AS (
                        SELECT
                            client_id,
                            interface,
                            chart_epoch,
                            min(first_sample_epoch)::bigint AS first_sample_epoch,
                            max(effective_epoch)::bigint AS effective_epoch,
                            LEAST(sum(sample_count)::bigint, 2147483647)::integer AS sample_count,
                            (array_agg(rx_bytes_avg ORDER BY effective_epoch DESC))[1]
                                AS rx_bytes_avg,
                            (array_agg(tx_bytes_avg ORDER BY effective_epoch DESC))[1]
                                AS tx_bytes_avg,
                            (array_agg(rx_bytes_last ORDER BY effective_epoch DESC))[1]
                                AS rx_bytes_last,
                            (array_agg(tx_bytes_last ORDER BY effective_epoch DESC))[1]
                                AS tx_bytes_last,
                            (array_agg(rx_counter_epoch ORDER BY effective_epoch DESC))[1]
                                AS rx_counter_epoch,
                            (array_agg(tx_counter_epoch ORDER BY effective_epoch DESC))[1]
                                AS tx_counter_epoch,
                            max(updated_at)::text AS updated_at
                        FROM selected
                        GROUP BY client_id, interface, chart_epoch
                    ), series_start AS (
                        SELECT
                            client_id,
                            interface,
                            min(first_sample_epoch)::bigint AS first_sample_epoch,
                            min(chart_epoch)::bigint AS first_chart_epoch
                        FROM bucketed
                        GROUP BY client_id, interface
                    ), preceding AS (
                        SELECT
                            series_start.client_id,
                            series_start.interface,
                            series_start.first_chart_epoch - $4::bigint AS chart_epoch,
                            candidate.effective_epoch AS effective_epoch,
                            0::integer AS sample_count,
                            candidate.rx_bytes_avg,
                            candidate.tx_bytes_avg,
                            candidate.rx_bytes_last,
                            candidate.tx_bytes_last,
                            candidate.rx_counter_epoch,
                            candidate.tx_counter_epoch,
                            candidate.updated_at::text AS updated_at,
                            FALSE AS visible
                        FROM series_start
                        JOIN LATERAL (
                            SELECT
                                LEAST(
                                    extract(epoch FROM rate.bucket_start)::bigint
                                        + rate.bucket_secs::bigint - 60,
                                    series_start.first_sample_epoch - 60
                                ) AS effective_epoch,
                                rate.rx_bytes_avg,
                                rate.tx_bytes_avg,
                                rate.rx_bytes_last,
                                rate.tx_bytes_last,
                                rate.rx_counter_epoch,
                                rate.tx_counter_epoch,
                                rate.updated_at
                            FROM telemetry_network_rates rate
                            WHERE rate.client_id = series_start.client_id
                              AND rate.interface = series_start.interface
                              AND ($1::INTEGER IS NULL OR rate.bucket_secs = $1)
                              AND rate.bucket_secs >= 60
                              AND rate.bucket_secs % 60 = 0
                              AND extract(epoch FROM rate.bucket_start)::bigint
                                    < series_start.first_sample_epoch
                              AND extract(epoch FROM rate.bucket_start)::bigint
                                    <= series_start.first_sample_epoch - 60
                            ORDER BY effective_epoch DESC, rate.bucket_start DESC
                            LIMIT 1
                        ) AS candidate ON TRUE
                    ), combined AS (
                        SELECT
                            client_id,
                            interface,
                            chart_epoch,
                            effective_epoch,
                            sample_count,
                            rx_bytes_avg,
                            tx_bytes_avg,
                            rx_bytes_last,
                            tx_bytes_last,
                            rx_counter_epoch,
                            tx_counter_epoch,
                            updated_at,
                            TRUE AS visible
                        FROM bucketed
                        UNION ALL
                        SELECT * FROM preceding
                    ), derived AS (
                        SELECT
                            combined.*,
                            lag(rx_bytes_avg) OVER rate_window AS previous_rx_bytes_avg,
                            lag(tx_bytes_avg) OVER rate_window AS previous_tx_bytes_avg,
                            lag(rx_bytes_last) OVER rate_window AS previous_rx_bytes_last,
                            lag(tx_bytes_last) OVER rate_window AS previous_tx_bytes_last,
                            lag(rx_counter_epoch) OVER rate_window AS previous_rx_counter_epoch,
                            lag(tx_counter_epoch) OVER rate_window AS previous_tx_counter_epoch,
                            lag(effective_epoch) OVER rate_window AS previous_effective_epoch
                        FROM combined
                        WINDOW rate_window AS (
                            PARTITION BY client_id, interface
                            ORDER BY effective_epoch ASC, visible ASC
                        )
                    ),
                    bounded AS (
                        SELECT
                            client_id,
                            interface,
                            to_timestamp(chart_epoch) AS chart_bucket_start,
                            LEAST(
                                GREATEST(effective_epoch - chart_epoch + 60, 60),
                                2147483647
                            )::integer AS bucket_secs,
                            sample_count,
                            rx_bytes_avg,
                            tx_bytes_avg,
                            rx_bytes_last,
                            tx_bytes_last,
                            rx_counter_epoch,
                            tx_counter_epoch,
                            rx_bytes_last - previous_rx_bytes_last AS rx_bytes_delta,
                            tx_bytes_last - previous_tx_bytes_last AS tx_bytes_delta,
                            ((rx_bytes_last - previous_rx_bytes_last) * 8)::double precision
                                / GREATEST(effective_epoch - previous_effective_epoch, 1)::double precision
                                AS rx_bps_avg,
                            ((tx_bytes_last - previous_tx_bytes_last) * 8)::double precision
                                / GREATEST(effective_epoch - previous_effective_epoch, 1)::double precision
                                AS tx_bps_avg,
                            updated_at
                        FROM derived
                        WHERE visible
                          AND previous_effective_epoch IS NOT NULL
                          AND rx_counter_epoch = previous_rx_counter_epoch
                          AND tx_counter_epoch = previous_tx_counter_epoch
                          AND rx_bytes_last >= previous_rx_bytes_last
                          AND tx_bytes_last >= previous_tx_bytes_last
                    ), ranked AS (
                        SELECT
                            bounded.*,
                            row_number() OVER (
                                PARTITION BY client_id, interface
                                ORDER BY chart_bucket_start DESC
                            ) AS point_rank
                        FROM bounded
                    ), globally_bounded AS (
                        SELECT *
                        FROM ranked
                        WHERE point_rank <= $5
                        ORDER BY
                            point_rank ASC,
                            chart_bucket_start DESC,
                            client_id ASC,
                            interface ASC
                        LIMIT $7
                    )
                    SELECT
                        client_id,
                        interface,
                        chart_bucket_start::text AS bucket_start,
                        bucket_secs,
                        sample_count,
                        rx_bytes_avg,
                        tx_bytes_avg,
                        rx_bytes_last,
                        tx_bytes_last,
                        rx_counter_epoch,
                        tx_counter_epoch,
                        rx_bytes_delta,
                        tx_bytes_delta,
                        rx_bps_avg,
                        tx_bps_avg,
                        updated_at
                    FROM globally_bounded
                    ORDER BY chart_bucket_start ASC, client_id ASC, interface ASC
                    "#,
                )
                .bind(bucket_secs)
                .bind(start_unix.map(|value| value as i64))
                .bind(end_unix.map(|value| value as i64))
                .bind(step_secs)
                .bind(points_per_series as i64)
                .bind(&all_client_ids)
                .bind(DASHBOARD_TELEMETRY_RESULT_LIMIT as i64)
                .bind(&exact_client_ids)
                .bind(&exact_interfaces)
                .fetch_all(pool)
                .await?;

                let rows = rows
                    .into_iter()
                    .map(telemetry_network_rate_from_row)
                    .collect::<Result<Vec<_>>>()?;
                Ok(project_network_rate_selection(rows, selection))
            }
        }
    }

    pub(crate) async fn list_telemetry_network_rates(
        &self,
        limit: i64,
        client_id: Option<&str>,
        interface: Option<&str>,
        bucket_secs: Option<i32>,
        visible_only: bool,
    ) -> Result<Vec<TelemetryNetworkRateView>> {
        match self {
            Self::Memory(memory) => {
                let hidden = memory.hidden_clients.read().await;
                let rows = memory
                    .telemetry_network_rates
                    .read()
                    .await
                    .iter()
                    .filter(|rate| {
                        (!visible_only || !hidden.contains(&rate.client_id))
                            && client_id.is_none_or(|client_id| rate.client_id == client_id)
                            && interface.is_none_or(|interface| rate.interface == interface)
                            && bucket_secs.is_none_or(|bucket_secs| rate.bucket_secs == bucket_secs)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let mut rows = derive_network_rates(rows);
                rows.sort_by(|left, right| {
                    right
                        .bucket_start
                        .cmp(&left.bucket_start)
                        .then_with(|| left.client_id.cmp(&right.client_id))
                        .then_with(|| left.interface.cmp(&right.interface))
                });
                rows.truncate(limit.clamp(1, TELEMETRY_LIST_LIMIT_MAX) as usize);
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH selected AS (
                        SELECT
                            client_id,
                            interface,
                            bucket_start,
                            bucket_secs,
                            sample_count,
                            rx_bytes_avg,
                            tx_bytes_avg,
                            rx_bytes_last,
                            tx_bytes_last,
                            rx_counter_epoch,
                            tx_counter_epoch,
                            updated_at,
                            lag(rx_bytes_last) OVER rate_window AS previous_rx_bytes_last,
                            lag(tx_bytes_last) OVER rate_window AS previous_tx_bytes_last,
                            lag(rx_counter_epoch) OVER rate_window AS previous_rx_counter_epoch,
                            lag(tx_counter_epoch) OVER rate_window AS previous_tx_counter_epoch,
                            lag(
                                bucket_start
                                    + make_interval(secs => GREATEST(bucket_secs - 60, 0))
                            ) OVER rate_window AS previous_effective_at,
                            bucket_start
                                + make_interval(secs => GREATEST(bucket_secs - 60, 0))
                                AS effective_at
                        FROM telemetry_network_rates
                        WHERE
                            ($1::TEXT IS NULL OR client_id = $1)
                            AND ($2::TEXT IS NULL OR interface = $2)
                            AND ($3::INTEGER IS NULL OR bucket_secs = $3)
                            AND (
                                NOT $4
                                OR EXISTS (
                                    SELECT 1 FROM visible_clients
                                    WHERE visible_clients.id = telemetry_network_rates.client_id
                                )
                            )
                        WINDOW rate_window AS (
                            PARTITION BY client_id, interface
                            ORDER BY
                                bucket_start
                                    + make_interval(secs => GREATEST(bucket_secs - 60, 0)) ASC,
                                bucket_start ASC
                        )
                    )
                    SELECT
                        client_id,
                        interface,
                        bucket_start::text AS bucket_start,
                        bucket_secs,
                        sample_count,
                        rx_bytes_avg,
                        tx_bytes_avg,
                        rx_bytes_last,
                        tx_bytes_last,
                        rx_counter_epoch,
                        tx_counter_epoch,
                        rx_bytes_last - previous_rx_bytes_last AS rx_bytes_delta,
                        tx_bytes_last - previous_tx_bytes_last AS tx_bytes_delta,
                        ((rx_bytes_last - previous_rx_bytes_last) * 8)::double precision
                            / GREATEST(
                                extract(epoch FROM (effective_at - previous_effective_at)),
                                1
                            )::double precision AS rx_bps_avg,
                        ((tx_bytes_last - previous_tx_bytes_last) * 8)::double precision
                            / GREATEST(
                                extract(epoch FROM (effective_at - previous_effective_at)),
                                1
                            )::double precision AS tx_bps_avg,
                        updated_at::text AS updated_at
                    FROM selected
                    WHERE previous_effective_at IS NOT NULL
                      AND rx_counter_epoch = previous_rx_counter_epoch
                      AND tx_counter_epoch = previous_tx_counter_epoch
                      AND rx_bytes_last >= previous_rx_bytes_last
                      AND tx_bytes_last >= previous_tx_bytes_last
                    ORDER BY effective_at DESC, client_id ASC, interface ASC
                    LIMIT $5
                    "#,
                )
                .bind(client_id)
                .bind(interface)
                .bind(bucket_secs)
                .bind(visible_only)
                .bind(limit.clamp(1, TELEMETRY_LIST_LIMIT_MAX))
                .fetch_all(pool)
                .await?;

                rows.into_iter()
                    .map(telemetry_network_rate_from_row)
                    .collect()
            }
        }
    }

    pub(crate) async fn list_latest_telemetry_network_rates(
        &self,
        limit: i64,
        client_id: Option<&str>,
        interface: Option<&str>,
        bucket_secs: Option<i32>,
    ) -> Result<Vec<TelemetryNetworkRateView>> {
        self.list_latest_telemetry_network_rates_matching(
            Some(limit.clamp(1, TELEMETRY_LIST_LIMIT_MAX) as usize),
            client_id,
            None,
            interface,
            bucket_secs,
            None,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn list_latest_telemetry_network_rates_for_clients(
        &self,
        client_ids: &[String],
    ) -> Result<Vec<TelemetryNetworkRateView>> {
        if client_ids.is_empty() {
            return Ok(Vec::new());
        }
        self.list_latest_telemetry_network_rates_matching(
            None,
            None,
            Some(client_ids),
            None,
            None,
            None,
        )
        .await
    }

    pub(crate) async fn list_latest_telemetry_network_rates_for_selection(
        &self,
        selection: &NetworkRateInterfaceSelection,
    ) -> Result<Vec<TelemetryNetworkRateView>> {
        if selection.is_empty() {
            return Ok(Vec::new());
        }
        let client_ids = selection.client_ids();
        let rows = self
            .list_latest_telemetry_network_rates_matching(
                None,
                None,
                Some(&client_ids),
                None,
                None,
                Some(selection),
            )
            .await?;
        Ok(project_network_rate_selection(rows, selection))
    }

    async fn list_latest_telemetry_network_rates_matching(
        &self,
        result_limit: Option<usize>,
        client_id: Option<&str>,
        client_ids: Option<&[String]>,
        interface: Option<&str>,
        bucket_secs: Option<i32>,
        selection: Option<&NetworkRateInterfaceSelection>,
    ) -> Result<Vec<TelemetryNetworkRateView>> {
        let selected_client_ids = client_ids.map(|client_ids| {
            client_ids
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>()
        });
        let unrestricted_selection = selection.is_none();
        let (all_client_ids, exact_client_ids, exact_interfaces) = selection
            .map(NetworkRateInterfaceSelection::query_parts)
            .unwrap_or_default();
        match self {
            Self::Memory(memory) => {
                let hidden = memory.hidden_clients.read().await;
                let rows = memory
                    .telemetry_network_rates
                    .read()
                    .await
                    .iter()
                    .filter(|rate| {
                        !hidden.contains(&rate.client_id)
                            && client_id.is_none_or(|client_id| rate.client_id == client_id)
                            && selected_client_ids.as_ref().is_none_or(|client_ids| {
                                client_ids.contains(rate.client_id.as_str())
                            })
                            && selection.is_none_or(|selection| {
                                selection.allows(&rate.client_id, &rate.interface)
                            })
                            && interface.is_none_or(|interface| rate.interface == interface)
                            && bucket_secs.is_none_or(|bucket_secs| rate.bucket_secs == bucket_secs)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let mut latest_physical_by_key = HashMap::<(String, String), u64>::new();
                for row in &rows {
                    let key = (row.client_id.clone(), row.interface.clone());
                    let timestamp = bucket_last_sample_unix(&row.bucket_start, row.bucket_secs);
                    latest_physical_by_key
                        .entry(key)
                        .and_modify(|latest| *latest = (*latest).max(timestamp))
                        .or_insert(timestamp);
                }
                let rows = derive_network_rates(rows);
                let mut latest = HashMap::<(String, String), TelemetryNetworkRateView>::new();
                for rate in rows {
                    let key = (rate.client_id.clone(), rate.interface.clone());
                    if latest_physical_by_key.get(&key).copied()
                        != Some(bucket_last_sample_unix(
                            &rate.bucket_start,
                            rate.bucket_secs,
                        ))
                    {
                        continue;
                    }
                    let replace = latest.get(&key).is_none_or(|current| {
                        bucket_last_sample_unix(&current.bucket_start, current.bucket_secs)
                            < bucket_last_sample_unix(&rate.bucket_start, rate.bucket_secs)
                    });
                    if replace {
                        latest.insert(key, rate);
                    }
                }
                let mut rows = latest.into_values().collect::<Vec<_>>();
                rows.sort_by(|left, right| {
                    bucket_last_sample_unix(&right.bucket_start, right.bucket_secs)
                        .cmp(&bucket_last_sample_unix(
                            &left.bucket_start,
                            left.bucket_secs,
                        ))
                        .then_with(|| left.client_id.cmp(&right.client_id))
                        .then_with(|| left.interface.cmp(&right.interface))
                });
                if let Some(limit) = result_limit {
                    rows.truncate(limit);
                }
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH latest AS (
                        SELECT DISTINCT ON (client_id, interface)
                            client_id,
                            interface,
                            bucket_start,
                            bucket_secs,
                            sample_count,
                            rx_bytes_avg,
                            tx_bytes_avg,
                            rx_bytes_last,
                            tx_bytes_last,
                            rx_counter_epoch,
                            tx_counter_epoch,
                            updated_at
                        FROM telemetry_network_rates
                        WHERE
                            EXISTS (
                                SELECT 1 FROM visible_clients
                                WHERE visible_clients.id = telemetry_network_rates.client_id
                            )
                            AND ($1::TEXT IS NULL OR client_id = $1)
                            AND ($2::TEXT[] IS NULL OR client_id = ANY($2))
                            AND ($3::TEXT IS NULL OR interface = $3)
                            AND ($4::INTEGER IS NULL OR bucket_secs = $4)
                            AND (
                                $6::BOOLEAN
                                OR client_id = ANY($7::TEXT[])
                                OR EXISTS (
                                    SELECT 1
                                    FROM UNNEST($8::TEXT[], $9::TEXT[])
                                        AS selected(client_id, interface)
                                    WHERE selected.client_id = telemetry_network_rates.client_id
                                      AND selected.interface = telemetry_network_rates.interface
                                )
                            )
                        ORDER BY
                            client_id,
                            interface,
                            bucket_start
                                + make_interval(secs => GREATEST(bucket_secs - 60, 0)) DESC,
                            bucket_start DESC
                    )
                    SELECT
                        latest.client_id,
                        latest.interface,
                        latest.bucket_start::text AS bucket_start,
                        latest.bucket_secs,
                        latest.sample_count,
                        latest.rx_bytes_avg,
                        latest.tx_bytes_avg,
                        latest.rx_bytes_last,
                        latest.tx_bytes_last,
                        latest.rx_counter_epoch,
                        latest.tx_counter_epoch,
                        latest.rx_bytes_last - previous.rx_bytes_last AS rx_bytes_delta,
                        latest.tx_bytes_last - previous.tx_bytes_last AS tx_bytes_delta,
                        ((latest.rx_bytes_last - previous.rx_bytes_last) * 8)::double precision
                            / GREATEST(
                                extract(epoch FROM (
                                    latest.bucket_start
                                        + make_interval(secs => GREATEST(latest.bucket_secs - 60, 0))
                                    - previous.effective_at
                                )),
                                1
                            )::double precision AS rx_bps_avg,
                        ((latest.tx_bytes_last - previous.tx_bytes_last) * 8)::double precision
                            / GREATEST(
                                extract(epoch FROM (
                                    latest.bucket_start
                                        + make_interval(secs => GREATEST(latest.bucket_secs - 60, 0))
                                    - previous.effective_at
                                )),
                                1
                            )::double precision AS tx_bps_avg,
                        latest.updated_at::text AS updated_at
                    FROM latest
                    LEFT JOIN LATERAL (
                        SELECT
                            bucket_start
                                + make_interval(secs => GREATEST(bucket_secs - 60, 0))
                                AS effective_at,
                            rx_bytes_last,
                            tx_bytes_last,
                            rx_counter_epoch,
                            tx_counter_epoch
                        FROM telemetry_network_rates AS candidate
                        WHERE
                            candidate.client_id = latest.client_id
                            AND candidate.interface = latest.interface
                            AND candidate.bucket_start
                                + make_interval(secs => GREATEST(candidate.bucket_secs - 60, 0))
                                < latest.bucket_start
                                    + make_interval(secs => GREATEST(latest.bucket_secs - 60, 0))
                        ORDER BY
                            candidate.bucket_start
                                + make_interval(secs => GREATEST(candidate.bucket_secs - 60, 0)) DESC
                        LIMIT 1
                    ) AS previous ON TRUE
                    WHERE previous.effective_at IS NOT NULL
                      AND latest.rx_counter_epoch = previous.rx_counter_epoch
                      AND latest.tx_counter_epoch = previous.tx_counter_epoch
                      AND latest.rx_bytes_last >= previous.rx_bytes_last
                      AND latest.tx_bytes_last >= previous.tx_bytes_last
                    ORDER BY
                        latest.bucket_start
                            + make_interval(secs => GREATEST(latest.bucket_secs - 60, 0)) DESC,
                        latest.client_id ASC,
                        latest.interface ASC
                    LIMIT $5
                    "#,
                )
                .bind(client_id)
                .bind(client_ids)
                .bind(interface)
                .bind(bucket_secs)
                .bind(result_limit.map(|limit| limit as i64))
                .bind(unrestricted_selection)
                .bind(&all_client_ids)
                .bind(&exact_client_ids)
                .bind(&exact_interfaces)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(telemetry_network_rate_from_row)
                    .collect()
            }
        }
    }

    pub(crate) async fn list_telemetry_tunnels(
        &self,
        limit: i64,
        client_id: Option<&str>,
        interface: Option<&str>,
    ) -> Result<Vec<TelemetryTunnelView>> {
        self.list_telemetry_tunnels_matching(
            Some(limit.clamp(1, TELEMETRY_LIST_LIMIT_MAX) as usize),
            client_id,
            None,
            interface,
            false,
            None,
            None,
            None,
        )
        .await
    }

    pub(crate) async fn list_fleet_alert_tunnel_candidates(
        &self,
        client_id: Option<&str>,
        client_ids: Option<&[String]>,
        severity: Option<&str>,
        start_unix: Option<u64>,
        end_unix: Option<u64>,
        limit: usize,
    ) -> Result<Vec<TelemetryTunnelView>> {
        self.list_telemetry_tunnels_matching(
            Some(limit),
            client_id,
            client_ids,
            None,
            true,
            severity,
            start_unix,
            end_unix,
        )
        .await
    }

    pub(crate) async fn list_declared_telemetry_tunnels_for_clients(
        &self,
        client_ids: &[String],
    ) -> Result<Vec<TelemetryTunnelView>> {
        if client_ids.is_empty() {
            return Ok(Vec::new());
        }
        self.list_telemetry_tunnels_matching(
            None,
            None,
            Some(client_ids),
            None,
            false,
            None,
            None,
            None,
        )
        .await
    }

    async fn list_telemetry_tunnels_matching(
        &self,
        result_limit: Option<usize>,
        client_id: Option<&str>,
        client_ids: Option<&[String]>,
        interface: Option<&str>,
        alert_candidates_only: bool,
        severity: Option<&str>,
        start_unix: Option<u64>,
        end_unix: Option<u64>,
    ) -> Result<Vec<TelemetryTunnelView>> {
        match self {
            Self::Memory(memory) => {
                let mut records = memory.telemetry_tunnels.read().await.clone();
                let hidden = memory.hidden_clients.read().await;
                let allowed_client_ids = client_ids.map(|client_ids| {
                    client_ids
                        .iter()
                        .map(String::as_str)
                        .collect::<HashSet<_>>()
                });
                let plans = memory
                    .tunnel_plans
                    .read()
                    .await
                    .iter()
                    .filter(|plan| plan.deleted_at.is_none())
                    .cloned()
                    .collect::<Vec<_>>();
                retain_declared_telemetry_tunnels(&mut records, &plans);
                records.retain(|record| {
                    !hidden.contains(&record.client_id)
                        && client_id.is_none_or(|expected| record.client_id == expected)
                        && allowed_client_ids
                            .as_ref()
                            .is_none_or(|client_ids| client_ids.contains(record.client_id.as_str()))
                        && interface.is_none_or(|expected| record.interface == expected)
                        && timestamp_in_bounds(&record.observed_at, start_unix, end_unix)
                        && (!alert_candidates_only
                            || telemetry_tunnel_matches_alert_candidate(record, severity))
                });
                records.sort_by(|left, right| {
                    tunnel_alert_priority(left, alert_candidates_only, severity)
                        .cmp(&tunnel_alert_priority(
                            right,
                            alert_candidates_only,
                            severity,
                        ))
                        .then_with(|| {
                            compare_timestamps_desc(&left.observed_at, &right.observed_at)
                        })
                        .then_with(|| left.client_id.cmp(&right.client_id))
                        .then_with(|| left.interface.cmp(&right.interface))
                });
                if let Some(result_limit) = result_limit {
                    records.truncate(result_limit);
                }
                Ok(records)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        client_id,
                        observed_at::text AS observed_at,
                        interface,
                        telemetry.kind AS kind,
                        ownership_mode,
                        mutation_policy,
                        source,
                        operstate,
                        mtu,
                        link_type,
                        address,
                        rx_bytes,
                        tx_bytes,
                        traffic_source,
                        traffic_status,
                        traffic_reason,
                        traffic_checked_unix,
                        telemetry_plan_id,
                        telemetry_plan_name,
                        telemetry_plan_runtime_manager,
                        telemetry_endpoint_side,
                        telemetry_peer_client_id,
                        adapter_health,
                        latency_monitoring_enabled,
                        latency_status,
                        latency_reason,
                        latency_primary_family,
                        latency_target,
                        latency_checked_unix,
                        latency_avg_ms,
                        packet_loss_ratio,
                        latency_healthy_windows,
                        latency_missed_windows
                    FROM telemetry_tunnels telemetry
                    JOIN visible_clients visible_client
                      ON visible_client.id = telemetry.client_id
                    JOIN tunnel_plans current_plan
                      ON current_plan.id::TEXT = telemetry.telemetry_plan_id
                     AND current_plan.deleted_at IS NULL
                     AND current_plan.enabled
                     AND current_plan.name = telemetry.telemetry_plan_name
                     AND current_plan.plan->>'interface_name' = telemetry.interface
                     AND (
                        (
                            telemetry.telemetry_endpoint_side = 'left'
                            AND current_plan.left_client_id = telemetry.client_id
                            AND current_plan.right_client_id = telemetry.telemetry_peer_client_id
                        )
                        OR (
                            telemetry.telemetry_endpoint_side = 'right'
                            AND current_plan.right_client_id = telemetry.client_id
                            AND current_plan.left_client_id = telemetry.telemetry_peer_client_id
                        )
                     )
                    WHERE ($1::TEXT IS NULL OR telemetry.client_id = $1)
                      AND ($2::TEXT[] IS NULL OR telemetry.client_id = ANY($2))
                      AND ($3::TEXT IS NULL OR telemetry.interface = $3)
                      AND (
                        $6::DOUBLE PRECISION IS NULL
                        OR telemetry.observed_at >= to_timestamp($6)
                      )
                      AND (
                        $7::DOUBLE PRECISION IS NULL
                        OR telemetry.observed_at <= to_timestamp($7)
                      )
                      AND (
                        NOT $4::BOOLEAN
                        OR (
                            ($5::TEXT IS NULL OR $5 = 'critical')
                            AND current_plan.plan#>>'{runtime_control,manager}'
                                = 'external_managed_adapter'
                            AND jsonb_typeof(telemetry.adapter_health) = 'object'
                            AND jsonb_typeof(telemetry.adapter_health->'status') = 'string'
                            AND telemetry.adapter_health->'success'
                                IS DISTINCT FROM 'true'::jsonb
                        )
                        OR (
                            ($5::TEXT IS NULL OR $5 = 'warning')
                            AND telemetry.traffic_status IS NOT NULL
                            AND telemetry.traffic_status <> 'ok'
                        )
                    )
                    ORDER BY
                        CASE
                            WHEN $4::BOOLEAN
                             AND $5::TEXT IS NULL
                             AND current_plan.plan#>>'{runtime_control,manager}'
                                = 'external_managed_adapter'
                             AND jsonb_typeof(telemetry.adapter_health) = 'object'
                             AND jsonb_typeof(telemetry.adapter_health->'status') = 'string'
                             AND telemetry.adapter_health->'success'
                                IS DISTINCT FROM 'true'::jsonb
                            THEN 0
                            ELSE 1
                        END ASC,
                        telemetry.observed_at DESC,
                        telemetry.client_id ASC,
                        telemetry.interface ASC
                    LIMIT $8
                    "#,
                )
                .bind(client_id)
                .bind(client_ids)
                .bind(interface)
                .bind(alert_candidates_only)
                .bind(severity)
                .bind(start_unix.map(|value| value as f64))
                .bind(end_unix.map(|value| value as f64))
                .bind(result_limit.map(|limit| limit as i64))
                .fetch_all(pool)
                .await?;

                let mut records = rows
                    .into_iter()
                    .map(|row| {
                        let telemetry_plan_id = row
                            .try_get::<Option<String>, _>("telemetry_plan_id")?
                            .and_then(|value| uuid::Uuid::parse_str(&value).ok());
                        let telemetry_plan_name = row.try_get("telemetry_plan_name")?;
                        Ok(TelemetryTunnelView {
                            client_id: row.try_get("client_id")?,
                            observed_at: row.try_get("observed_at")?,
                            interface: row.try_get("interface")?,
                            kind: row.try_get("kind")?,
                            ownership_mode: row.try_get("ownership_mode")?,
                            mutation_policy: row.try_get("mutation_policy")?,
                            plan_id: telemetry_plan_id,
                            plan_name: telemetry_plan_name,
                            plan_runtime_manager: row.try_get("telemetry_plan_runtime_manager")?,
                            endpoint_side: row.try_get("telemetry_endpoint_side")?,
                            peer_client_id: row.try_get("telemetry_peer_client_id")?,
                            source: row.try_get("source")?,
                            operstate: row.try_get("operstate")?,
                            mtu: row.try_get("mtu")?,
                            link_type: row.try_get("link_type")?,
                            address: row.try_get("address")?,
                            rx_bytes: row.try_get("rx_bytes")?,
                            tx_bytes: row.try_get("tx_bytes")?,
                            traffic_source: row.try_get("traffic_source")?,
                            traffic_status: row.try_get("traffic_status")?,
                            traffic_reason: row.try_get("traffic_reason")?,
                            traffic_checked_unix: row.try_get("traffic_checked_unix")?,
                            adapter_health: parse_adapter_health(row.try_get("adapter_health")?),
                            latency_monitoring_enabled: row
                                .try_get("latency_monitoring_enabled")?,
                            latency_status: row.try_get("latency_status")?,
                            latency_reason: row.try_get("latency_reason")?,
                            latency_primary_family: row.try_get("latency_primary_family")?,
                            latency_target: row.try_get("latency_target")?,
                            latency_checked_unix: row.try_get("latency_checked_unix")?,
                            latency_avg_ms: row.try_get("latency_avg_ms")?,
                            packet_loss_ratio: row.try_get("packet_loss_ratio")?,
                            latency_healthy_windows: row.try_get("latency_healthy_windows")?,
                            latency_missed_windows: row.try_get("latency_missed_windows")?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let plans = self.list_tunnel_plans().await?;
                retain_declared_telemetry_tunnels(&mut records, &plans);
                if alert_candidates_only {
                    records.retain(|record| {
                        telemetry_tunnel_matches_alert_candidate(record, severity)
                    });
                }
                Ok(records)
            }
        }
    }
}

#[derive(Default)]
struct MemoryRollupAggregate {
    sample_count: i64,
    cpu_usage_sample_count: i64,
    cpu_usage_weighted_total: f64,
    cpu_usage_max: Option<f64>,
    cpu_cores_max: i32,
    cpu_weighted_total: f64,
    cpu_max: Option<f64>,
    cpu_load_5_weighted_total: f64,
    cpu_load_5_max: Option<f64>,
    cpu_load_15_weighted_total: f64,
    cpu_load_15_max: Option<f64>,
    memory_total_max: Option<i64>,
    memory_available_weighted_total: i128,
    memory_available_min: Option<i64>,
    memory_used_ratio_weighted_total: f64,
    memory_used_ratio_max: Option<f64>,
    swap_sample_count: i64,
    swap_total_max: Option<i64>,
    swap_explicit_no_swap: bool,
    swap_available_weighted_total: i128,
    swap_available_min: Option<i64>,
    swap_used_ratio_weighted_total: f64,
    swap_used_ratio_max: Option<f64>,
    disk_total_max: Option<i64>,
    disk_available_weighted_total: i128,
    disk_available_min: Option<i64>,
    disk_used_ratio_weighted_total: f64,
    disk_used_ratio_max: Option<f64>,
    network_rx_max: Option<i64>,
    network_tx_max: Option<i64>,
    connections_sample_count: i64,
    tcp_sockets_latest: Option<i64>,
    udp_sockets_latest: Option<i64>,
    connections_observed_at: Option<String>,
    latest_observed_at: String,
    updated_at: String,
}

fn aggregate_memory_telemetry_rollups(
    rows: Vec<TelemetryRollupView>,
    step_secs: i32,
) -> Vec<TelemetryRollupView> {
    let step_secs = step_secs.max(60) as u64;
    let mut groups = BTreeMap::<(String, u64), MemoryRollupAggregate>::new();
    for row in rows {
        let timestamp = parse_timestamp_unix(&row.bucket_start).unwrap_or(0);
        let chart_bucket = timestamp / step_secs * step_secs;
        let aggregate = groups
            .entry((row.client_id.clone(), chart_bucket))
            .or_default();
        let sample_count = i64::from(row.sample_count.max(0));
        aggregate.sample_count = aggregate.sample_count.saturating_add(sample_count);
        let cpu_usage_sample_count = i64::from(row.cpu_usage_sample_count.max(0));
        aggregate.cpu_usage_sample_count = aggregate
            .cpu_usage_sample_count
            .saturating_add(cpu_usage_sample_count);
        if let Some(value) = row.cpu_usage_avg {
            aggregate.cpu_usage_weighted_total += value * cpu_usage_sample_count as f64;
        }
        if let Some(value) = row.cpu_usage_max {
            aggregate.cpu_usage_max = Some(
                aggregate
                    .cpu_usage_max
                    .map_or(value, |current| current.max(value)),
            );
        }
        aggregate.cpu_cores_max = aggregate.cpu_cores_max.max(row.cpu_cores_max);
        aggregate.cpu_weighted_total += row.cpu_load_1_avg * sample_count as f64;
        aggregate.cpu_max = Some(aggregate.cpu_max.map_or(row.cpu_load_1_max, |current| {
            current.max(row.cpu_load_1_max)
        }));
        aggregate.cpu_load_5_weighted_total += row.cpu_load_5_avg * sample_count as f64;
        aggregate.cpu_load_5_max = Some(
            aggregate
                .cpu_load_5_max
                .map_or(row.cpu_load_5_max, |current| {
                    current.max(row.cpu_load_5_max)
                }),
        );
        aggregate.cpu_load_15_weighted_total += row.cpu_load_15_avg * sample_count as f64;
        aggregate.cpu_load_15_max = Some(
            aggregate
                .cpu_load_15_max
                .map_or(row.cpu_load_15_max, |current| {
                    current.max(row.cpu_load_15_max)
                }),
        );
        aggregate.memory_total_max = Some(
            aggregate
                .memory_total_max
                .map_or(row.memory_total_bytes_max, |current| {
                    current.max(row.memory_total_bytes_max)
                }),
        );
        aggregate.memory_available_weighted_total =
            aggregate.memory_available_weighted_total.saturating_add(
                i128::from(row.memory_available_bytes_avg).saturating_mul(i128::from(sample_count)),
            );
        aggregate.memory_available_min = Some(
            aggregate
                .memory_available_min
                .map_or(row.memory_available_bytes_min, |current| {
                    current.min(row.memory_available_bytes_min)
                }),
        );
        aggregate.memory_used_ratio_weighted_total +=
            row.memory_used_ratio_avg * sample_count as f64;
        aggregate.memory_used_ratio_max = Some(
            aggregate
                .memory_used_ratio_max
                .map_or(row.memory_used_ratio_max, |current| {
                    current.max(row.memory_used_ratio_max)
                }),
        );
        let swap_sample_count = i64::from(row.swap_sample_count.max(0));
        if let Some(total) = row.swap_total_bytes_max {
            aggregate.swap_total_max = Some(
                aggregate
                    .swap_total_max
                    .map_or(total, |current| current.max(total)),
            );
            aggregate.swap_explicit_no_swap |= swap_sample_count == 0 && total == 0;
        }
        if let (Some(used_ratio_avg), Some(used_ratio_max)) =
            (row.swap_used_ratio_avg, row.swap_used_ratio_max)
        {
            aggregate.swap_sample_count = aggregate
                .swap_sample_count
                .saturating_add(swap_sample_count);
            aggregate.swap_used_ratio_weighted_total += used_ratio_avg * swap_sample_count as f64;
            aggregate.swap_used_ratio_max = Some(
                aggregate
                    .swap_used_ratio_max
                    .map_or(used_ratio_max, |current| current.max(used_ratio_max)),
            );
            if let (Some(available_avg), Some(available_min)) =
                (row.swap_available_bytes_avg, row.swap_available_bytes_min)
            {
                aggregate.swap_available_weighted_total =
                    aggregate.swap_available_weighted_total.saturating_add(
                        i128::from(available_avg).saturating_mul(i128::from(swap_sample_count)),
                    );
                aggregate.swap_available_min = Some(
                    aggregate
                        .swap_available_min
                        .map_or(available_min, |current| current.min(available_min)),
                );
            }
        }
        aggregate.disk_total_max = Some(
            aggregate
                .disk_total_max
                .map_or(row.disk_total_bytes_max, |current| {
                    current.max(row.disk_total_bytes_max)
                }),
        );
        aggregate.disk_available_weighted_total =
            aggregate.disk_available_weighted_total.saturating_add(
                i128::from(row.disk_available_bytes_avg).saturating_mul(i128::from(sample_count)),
            );
        aggregate.disk_available_min = Some(
            aggregate
                .disk_available_min
                .map_or(row.disk_available_bytes_min, |current| {
                    current.min(row.disk_available_bytes_min)
                }),
        );
        aggregate.disk_used_ratio_weighted_total += row.disk_used_ratio_avg * sample_count as f64;
        aggregate.disk_used_ratio_max = Some(
            aggregate
                .disk_used_ratio_max
                .map_or(row.disk_used_ratio_max, |current| {
                    current.max(row.disk_used_ratio_max)
                }),
        );
        aggregate.network_rx_max = Some(
            aggregate
                .network_rx_max
                .map_or(row.network_rx_bytes_max, |current| {
                    current.max(row.network_rx_bytes_max)
                }),
        );
        aggregate.network_tx_max = Some(
            aggregate
                .network_tx_max
                .map_or(row.network_tx_bytes_max, |current| {
                    current.max(row.network_tx_bytes_max)
                }),
        );
        aggregate.connections_sample_count = aggregate
            .connections_sample_count
            .saturating_add(i64::from(row.connections_sample_count.max(0)));
        if row
            .connections_observed_at
            .as_deref()
            .is_some_and(|observed_at| {
                aggregate
                    .connections_observed_at
                    .as_deref()
                    .is_none_or(|stored| {
                        parse_timestamp_unix(observed_at).unwrap_or(0)
                            >= parse_timestamp_unix(stored).unwrap_or(0)
                    })
            })
        {
            aggregate.tcp_sockets_latest = row.tcp_sockets_latest;
            aggregate.udp_sockets_latest = row.udp_sockets_latest;
            aggregate.connections_observed_at = row.connections_observed_at;
        }
        retain_newer_timestamp(&mut aggregate.latest_observed_at, &row.latest_observed_at);
        retain_newer_timestamp(&mut aggregate.updated_at, &row.updated_at);
    }

    groups
        .into_iter()
        .map(|((client_id, bucket_start), aggregate)| {
            let sample_count = aggregate.sample_count.max(0);
            TelemetryRollupView {
                client_id,
                bucket_start: bucket_start.to_string(),
                bucket_secs: step_secs as i32,
                sample_count: sample_count.min(i64::from(i32::MAX)) as i32,
                cpu_usage_sample_count: aggregate.cpu_usage_sample_count.min(i64::from(i32::MAX))
                    as i32,
                cpu_usage_avg: (aggregate.cpu_usage_sample_count > 0).then_some(
                    aggregate.cpu_usage_weighted_total / aggregate.cpu_usage_sample_count as f64,
                ),
                cpu_usage_max: aggregate.cpu_usage_max,
                cpu_cores_max: aggregate.cpu_cores_max,
                cpu_load_1_avg: if sample_count == 0 {
                    0.0
                } else {
                    aggregate.cpu_weighted_total / sample_count as f64
                },
                cpu_load_1_max: aggregate.cpu_max.unwrap_or(0.0),
                cpu_load_5_avg: if sample_count == 0 {
                    0.0
                } else {
                    aggregate.cpu_load_5_weighted_total / sample_count as f64
                },
                cpu_load_5_max: aggregate.cpu_load_5_max.unwrap_or(0.0),
                cpu_load_15_avg: if sample_count == 0 {
                    0.0
                } else {
                    aggregate.cpu_load_15_weighted_total / sample_count as f64
                },
                cpu_load_15_max: aggregate.cpu_load_15_max.unwrap_or(0.0),
                memory_total_bytes_max: aggregate.memory_total_max.unwrap_or(0),
                memory_available_bytes_avg: round_i128_div_i64(
                    aggregate.memory_available_weighted_total,
                    sample_count,
                ),
                memory_available_bytes_min: aggregate.memory_available_min.unwrap_or(0),
                memory_used_ratio_avg: if sample_count == 0 {
                    0.0
                } else {
                    aggregate.memory_used_ratio_weighted_total / sample_count as f64
                },
                memory_used_ratio_max: aggregate.memory_used_ratio_max.unwrap_or(0.0),
                swap_sample_count: aggregate.swap_sample_count.min(i64::from(i32::MAX)) as i32,
                swap_total_bytes_max: aggregate.swap_total_max,
                swap_available_bytes_avg: if aggregate.swap_sample_count > 0 {
                    Some(round_i128_div_i64(
                        aggregate.swap_available_weighted_total,
                        aggregate.swap_sample_count,
                    ))
                } else {
                    aggregate.swap_explicit_no_swap.then_some(0)
                },
                swap_available_bytes_min: if aggregate.swap_sample_count > 0 {
                    aggregate.swap_available_min
                } else {
                    aggregate.swap_explicit_no_swap.then_some(0)
                },
                swap_used_ratio_avg: (aggregate.swap_sample_count > 0).then_some(
                    aggregate.swap_used_ratio_weighted_total / aggregate.swap_sample_count as f64,
                ),
                swap_used_ratio_max: aggregate.swap_used_ratio_max,
                disk_total_bytes_max: aggregate.disk_total_max.unwrap_or(0),
                disk_available_bytes_avg: round_i128_div_i64(
                    aggregate.disk_available_weighted_total,
                    sample_count,
                ),
                disk_available_bytes_min: aggregate.disk_available_min.unwrap_or(0),
                disk_used_ratio_avg: if sample_count == 0 {
                    0.0
                } else {
                    aggregate.disk_used_ratio_weighted_total / sample_count as f64
                },
                disk_used_ratio_max: aggregate.disk_used_ratio_max.unwrap_or(0.0),
                network_rx_bytes_max: aggregate.network_rx_max.unwrap_or(0),
                network_tx_bytes_max: aggregate.network_tx_max.unwrap_or(0),
                connections_sample_count: aggregate
                    .connections_sample_count
                    .min(i64::from(i32::MAX)) as i32,
                tcp_sockets_latest: aggregate.tcp_sockets_latest,
                udp_sockets_latest: aggregate.udp_sockets_latest,
                connections_observed_at: aggregate.connections_observed_at,
                latest_observed_at: aggregate.latest_observed_at,
                updated_at: aggregate.updated_at,
            }
        })
        .collect()
}

fn retain_fair_rollup_points(
    rows: &mut Vec<TelemetryRollupView>,
    points_per_client: usize,
    total_limit: usize,
) {
    // Assign each client a local recency rank before applying the global cap.
    // This keeps one noisy client from crowding out newer points from its peers.
    rows.sort_by(|left, right| {
        left.client_id
            .cmp(&right.client_id)
            .then_with(|| {
                parse_timestamp_unix(&right.bucket_start)
                    .cmp(&parse_timestamp_unix(&left.bucket_start))
            })
            .then_with(|| right.bucket_start.cmp(&left.bucket_start))
    });
    let mut counts = HashMap::<String, usize>::new();
    let mut ranked = std::mem::take(rows)
        .into_iter()
        .filter_map(|row| {
            let count = counts.entry(row.client_id.clone()).or_default();
            let rank = *count;
            *count = count.saturating_add(1);
            (rank < points_per_client).then_some((rank, row))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|(left_rank, left), (right_rank, right)| {
        left_rank
            .cmp(right_rank)
            .then_with(|| {
                parse_timestamp_unix(&right.bucket_start)
                    .cmp(&parse_timestamp_unix(&left.bucket_start))
            })
            .then_with(|| left.client_id.cmp(&right.client_id))
            .then_with(|| right.bucket_start.cmp(&left.bucket_start))
    });
    ranked.truncate(total_limit);
    rows.extend(ranked.into_iter().map(|(_, row)| row));
    rows.sort_by(|left, right| {
        parse_timestamp_unix(&left.bucket_start)
            .cmp(&parse_timestamp_unix(&right.bucket_start))
            .then_with(|| left.client_id.cmp(&right.client_id))
            .then_with(|| left.bucket_start.cmp(&right.bucket_start))
    });
}

fn retain_fair_network_points(
    rows: &mut Vec<TelemetryNetworkRateView>,
    points_per_series: usize,
    total_limit: usize,
) {
    // Use the same rank-first policy for every client/interface series.
    rows.sort_by(|left, right| {
        left.client_id
            .cmp(&right.client_id)
            .then_with(|| left.interface.cmp(&right.interface))
            .then_with(|| {
                parse_timestamp_unix(&right.bucket_start)
                    .cmp(&parse_timestamp_unix(&left.bucket_start))
            })
            .then_with(|| right.bucket_start.cmp(&left.bucket_start))
    });
    let mut counts = HashMap::<(String, String), usize>::new();
    let mut ranked = std::mem::take(rows)
        .into_iter()
        .filter_map(|row| {
            let count = counts
                .entry((row.client_id.clone(), row.interface.clone()))
                .or_default();
            let rank = *count;
            *count = count.saturating_add(1);
            (rank < points_per_series).then_some((rank, row))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|(left_rank, left), (right_rank, right)| {
        left_rank
            .cmp(right_rank)
            .then_with(|| {
                parse_timestamp_unix(&right.bucket_start)
                    .cmp(&parse_timestamp_unix(&left.bucket_start))
            })
            .then_with(|| left.client_id.cmp(&right.client_id))
            .then_with(|| left.interface.cmp(&right.interface))
            .then_with(|| right.bucket_start.cmp(&left.bucket_start))
    });
    ranked.truncate(total_limit);
    rows.extend(ranked.into_iter().map(|(_, row)| row));
    rows.sort_by(|left, right| {
        parse_timestamp_unix(&left.bucket_start)
            .cmp(&parse_timestamp_unix(&right.bucket_start))
            .then_with(|| left.client_id.cmp(&right.client_id))
            .then_with(|| left.interface.cmp(&right.interface))
            .then_with(|| left.bucket_start.cmp(&right.bucket_start))
    });
}

fn retain_newer_timestamp(current: &mut String, candidate: &str) {
    let replace = current.is_empty()
        || match (
            parse_timestamp_unix(candidate),
            parse_timestamp_unix(current),
        ) {
            (Some(candidate), Some(current)) => candidate > current,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => candidate > current.as_str(),
        };
    if replace {
        candidate.clone_into(current);
    }
}

fn round_i128_div_i64(numerator: i128, denominator: i64) -> i64 {
    let denominator = i128::from(denominator.max(1));
    ((numerator + denominator / 2) / denominator).clamp(i128::from(i64::MIN), i128::from(i64::MAX))
        as i64
}

#[derive(Default)]
struct MemoryNetworkAggregate {
    sample_count: i32,
    last_sample_unix: u64,
    rx_bytes_avg: i64,
    tx_bytes_avg: i64,
    rx_bytes_last: i64,
    tx_bytes_last: i64,
    rx_counter_epoch: i64,
    tx_counter_epoch: i64,
    updated_at: String,
}

fn select_dashboard_network_rows(
    rows: Vec<TelemetryNetworkRateView>,
    start_unix: Option<u64>,
    end_unix: Option<u64>,
    step_secs: i32,
) -> Vec<TelemetryNetworkRateView> {
    let step_secs = normalized_dashboard_step_secs(step_secs);
    let mut visible = BTreeMap::<(String, String, u64), MemoryNetworkAggregate>::new();
    let mut preceding = HashMap::<(String, String), TelemetryNetworkRateView>::new();
    for row in rows {
        let Some(bucket_start) = parse_timestamp_unix(&row.bucket_start) else {
            continue;
        };
        if row.bucket_secs < 60 || row.bucket_secs % 60 != 0 {
            continue;
        }
        for fragment in logical_span_fragments(
            bucket_start,
            row.bucket_secs,
            start_unix,
            end_unix,
            step_secs,
        ) {
            let sample_count = proportional_fragment_count(row.sample_count, fragment);
            if sample_count == 0 {
                continue;
            }
            let last_sample_unix = bucket_start.saturating_add(
                fragment
                    .first_minute
                    .saturating_add(fragment.minute_count)
                    .saturating_sub(1)
                    .saturating_mul(60),
            );
            let aggregate = visible
                .entry((
                    row.client_id.clone(),
                    row.interface.clone(),
                    fragment.chart_bucket_start,
                ))
                .or_default();
            aggregate.sample_count = aggregate.sample_count.saturating_add(sample_count);
            if last_sample_unix >= aggregate.last_sample_unix {
                aggregate.last_sample_unix = last_sample_unix;
                aggregate.rx_bytes_avg = row.rx_bytes_avg;
                aggregate.tx_bytes_avg = row.tx_bytes_avg;
                aggregate.rx_bytes_last = row.rx_bytes_last;
                aggregate.tx_bytes_last = row.tx_bytes_last;
                aggregate.rx_counter_epoch = row.rx_counter_epoch;
                aggregate.tx_counter_epoch = row.tx_counter_epoch;
            }
            retain_newer_timestamp(&mut aggregate.updated_at, &row.updated_at);
        }

        let Some(start) = start_unix else {
            continue;
        };
        if bucket_start >= start {
            continue;
        }
        let source_minutes = row.bucket_secs as u64 / 60;
        let preceding_minute = start
            .saturating_sub(1)
            .saturating_sub(bucket_start)
            .saturating_div(60)
            .min(source_minutes.saturating_sub(1));
        let effective_at = bucket_start.saturating_add(preceding_minute.saturating_mul(60));
        let key = (row.client_id.clone(), row.interface.clone());
        if preceding.get(&key).is_none_or(|current| {
            bucket_last_sample_unix(&current.bucket_start, current.bucket_secs) < effective_at
        }) {
            preceding.insert(
                key,
                TelemetryNetworkRateView {
                    bucket_start: effective_at.to_string(),
                    bucket_secs: 60,
                    sample_count: 0,
                    rx_bytes_delta: 0,
                    tx_bytes_delta: 0,
                    rx_bps_avg: 0.0,
                    tx_bps_avg: 0.0,
                    ..row.clone()
                },
            );
        }
    }
    let mut selected = visible
        .into_iter()
        .map(
            |((client_id, interface, chart_bucket_start), aggregate)| TelemetryNetworkRateView {
                client_id,
                interface,
                bucket_start: chart_bucket_start.to_string(),
                bucket_secs: aggregate
                    .last_sample_unix
                    .saturating_sub(chart_bucket_start)
                    .saturating_add(60)
                    .clamp(60, i32::MAX as u64) as i32,
                sample_count: aggregate.sample_count,
                rx_bytes_avg: aggregate.rx_bytes_avg,
                tx_bytes_avg: aggregate.tx_bytes_avg,
                rx_bytes_last: aggregate.rx_bytes_last,
                tx_bytes_last: aggregate.tx_bytes_last,
                rx_counter_epoch: aggregate.rx_counter_epoch,
                tx_counter_epoch: aggregate.tx_counter_epoch,
                rx_bytes_delta: 0,
                tx_bytes_delta: 0,
                rx_bps_avg: 0.0,
                tx_bps_avg: 0.0,
                updated_at: aggregate.updated_at,
            },
        )
        .collect::<Vec<_>>();
    selected.extend(preceding.into_values());
    selected
}

fn aggregate_memory_network_rates(
    rows: Vec<TelemetryNetworkRateView>,
    step_secs: i32,
) -> Vec<TelemetryNetworkRateView> {
    let step_secs = step_secs.max(60) as u64;
    let mut groups = BTreeMap::<(String, String, i32, u64), MemoryNetworkAggregate>::new();
    for row in rows {
        let timestamp = parse_timestamp_unix(&row.bucket_start).unwrap_or(0);
        let chart_bucket = timestamp / step_secs * step_secs;
        let key = (
            row.client_id.clone(),
            row.interface.clone(),
            step_secs as i32,
            chart_bucket,
        );
        let aggregate = groups.entry(key).or_default();
        let sample_count = row.sample_count.max(0);
        aggregate.sample_count = aggregate.sample_count.saturating_add(sample_count);
        let last_sample_unix = bucket_last_sample_unix(&row.bucket_start, row.bucket_secs);
        if last_sample_unix >= aggregate.last_sample_unix {
            aggregate.last_sample_unix = last_sample_unix;
            aggregate.rx_bytes_avg = row.rx_bytes_avg;
            aggregate.tx_bytes_avg = row.tx_bytes_avg;
            aggregate.rx_bytes_last = row.rx_bytes_last;
            aggregate.tx_bytes_last = row.tx_bytes_last;
            aggregate.rx_counter_epoch = row.rx_counter_epoch;
            aggregate.tx_counter_epoch = row.tx_counter_epoch;
        }
        retain_newer_timestamp(&mut aggregate.updated_at, &row.updated_at);
    }

    groups
        .into_iter()
        .map(
            |((client_id, interface, _bucket_secs, bucket_start), aggregate)| {
                let sample_count = aggregate.sample_count.max(1);
                TelemetryNetworkRateView {
                    client_id,
                    interface,
                    bucket_start: bucket_start.to_string(),
                    bucket_secs: aggregate
                        .last_sample_unix
                        .saturating_sub(bucket_start)
                        .saturating_add(60)
                        .clamp(60, i32::MAX as u64) as i32,
                    sample_count,
                    rx_bytes_avg: aggregate.rx_bytes_avg,
                    tx_bytes_avg: aggregate.tx_bytes_avg,
                    rx_bytes_last: aggregate.rx_bytes_last,
                    tx_bytes_last: aggregate.tx_bytes_last,
                    rx_counter_epoch: aggregate.rx_counter_epoch,
                    tx_counter_epoch: aggregate.tx_counter_epoch,
                    rx_bytes_delta: 0,
                    tx_bytes_delta: 0,
                    rx_bps_avg: 0.0,
                    tx_bps_avg: 0.0,
                    updated_at: aggregate.updated_at,
                }
            },
        )
        .collect()
}

fn derive_network_rates(mut rows: Vec<TelemetryNetworkRateView>) -> Vec<TelemetryNetworkRateView> {
    rows.sort_by(|left, right| {
        left.client_id
            .cmp(&right.client_id)
            .then_with(|| left.interface.cmp(&right.interface))
            .then_with(|| {
                bucket_last_sample_unix(&left.bucket_start, left.bucket_secs).cmp(
                    &bucket_last_sample_unix(&right.bucket_start, right.bucket_secs),
                )
            })
    });
    let mut previous_by_key = HashMap::<(String, String), TelemetryNetworkRateView>::new();
    let mut derived = Vec::with_capacity(rows.len());
    for mut row in rows {
        let key = (row.client_id.clone(), row.interface.clone());
        if let Some(previous) = previous_by_key.get(&key) {
            if row.rx_counter_epoch == previous.rx_counter_epoch
                && row.tx_counter_epoch == previous.tx_counter_epoch
                && row.rx_bytes_last >= previous.rx_bytes_last
                && row.tx_bytes_last >= previous.tx_bytes_last
            {
                let current_ts = bucket_last_sample_unix(&row.bucket_start, row.bucket_secs);
                let previous_ts =
                    bucket_last_sample_unix(&previous.bucket_start, previous.bucket_secs);
                let duration = current_ts.saturating_sub(previous_ts).max(1) as f64;
                row.rx_bytes_delta = row.rx_bytes_last - previous.rx_bytes_last;
                row.tx_bytes_delta = row.tx_bytes_last - previous.tx_bytes_last;
                row.rx_bps_avg = (row.rx_bytes_delta * 8) as f64 / duration;
                row.tx_bps_avg = (row.tx_bytes_delta * 8) as f64 / duration;
                derived.push(row.clone());
            }
        }
        // A reset row is unavailable as a rate point, but it is the baseline
        // for the next sample. Advancing here preserves a single explicit gap
        // instead of smearing traffic across the reset boundary.
        previous_by_key.insert(key, row);
    }
    derived
}

fn mark_memory_network_counter_epochs(
    mut rows: Vec<TelemetryNetworkRateView>,
) -> Vec<TelemetryNetworkRateView> {
    rows.sort_by(|left, right| {
        left.client_id
            .cmp(&right.client_id)
            .then_with(|| left.interface.cmp(&right.interface))
            .then_with(|| {
                bucket_last_sample_unix(&left.bucket_start, left.bucket_secs).cmp(
                    &bucket_last_sample_unix(&right.bucket_start, right.bucket_secs),
                )
            })
    });
    let mut previous = HashMap::<(String, String), (i64, i64, i64, i64)>::new();
    for row in &mut rows {
        let key = (row.client_id.clone(), row.interface.clone());
        let (rx_epoch, tx_epoch) = previous.get(&key).map_or((0, 0), |state| {
            (
                state.2 + i64::from(row.rx_bytes_last < state.0),
                state.3 + i64::from(row.tx_bytes_last < state.1),
            )
        });
        row.rx_counter_epoch = rx_epoch;
        row.tx_counter_epoch = tx_epoch;
        previous.insert(
            key,
            (row.rx_bytes_last, row.tx_bytes_last, rx_epoch, tx_epoch),
        );
    }
    rows
}

fn retain_declared_telemetry_tunnels(
    records: &mut Vec<TelemetryTunnelView>,
    plans: &[TunnelPlanView],
) {
    let plans_by_id = plans
        .iter()
        .map(|plan| (plan.id, plan))
        .collect::<HashMap<_, _>>();
    records.retain_mut(|record| {
        let Some(plan) = record
            .plan_id
            .and_then(|plan_id| plans_by_id.get(&plan_id).copied())
        else {
            return false;
        };
        if !plan.enabled {
            return false;
        }
        let (side, peer_client_id) = match record.endpoint_side.as_deref() {
            Some("left")
                if plan.left_client_id == record.client_id
                    && record.peer_client_id.as_deref() == Some(plan.right_client_id.as_str()) =>
            {
                ("left", plan.right_client_id.as_str())
            }
            Some("right")
                if plan.right_client_id == record.client_id
                    && record.peer_client_id.as_deref() == Some(plan.left_client_id.as_str()) =>
            {
                ("right", plan.left_client_id.as_str())
            }
            _ => return false,
        };
        if record.interface != plan.plan.interface_name
            || record.plan_name.as_deref() != Some(plan.name.as_str())
        {
            return false;
        }
        let manager = plan.plan.runtime_control.manager;
        let runtime_manager = runtime_manager_label(manager);
        record.plan_runtime_manager = Some(runtime_manager.to_string());
        record.endpoint_side = Some(side.to_string());
        record.peer_client_id = Some(peer_client_id.to_string());
        record.ownership_mode = runtime_manager.to_string();
        record.mutation_policy = matched_plan_mutation_policy(manager).to_string();
        true
    });
}

pub(crate) fn tunnel_adapter_health_is_degraded(tunnel: &TelemetryTunnelView) -> bool {
    tunnel.plan_runtime_manager.as_deref() == Some("external_managed_adapter")
        && tunnel
            .adapter_health
            .as_ref()
            .is_some_and(|health| !health.success)
}

fn telemetry_tunnel_matches_alert_candidate(
    tunnel: &TelemetryTunnelView,
    severity: Option<&str>,
) -> bool {
    ((severity.is_none() || severity == Some("critical"))
        && tunnel_adapter_health_is_degraded(tunnel))
        || ((severity.is_none() || severity == Some("warning"))
            && tunnel
                .traffic_status
                .as_deref()
                .is_some_and(|status| status != "ok"))
}

fn tunnel_alert_priority(
    tunnel: &TelemetryTunnelView,
    alert_candidates_only: bool,
    severity: Option<&str>,
) -> usize {
    if alert_candidates_only && severity.is_none() && tunnel_adapter_health_is_degraded(tunnel) {
        0
    } else {
        1
    }
}

fn parse_adapter_health(
    value: Option<serde_json::Value>,
) -> Option<TelemetryTunnelAdapterHealthView> {
    let value = value?;
    if !value.is_object() {
        return None;
    }
    Some(TelemetryTunnelAdapterHealthView {
        status: value.get("status")?.as_str()?.to_string(),
        checked_unix: value
            .get("checked_unix")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        configured: value
            .get("configured")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        success: value
            .get("success")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        exit_code: value
            .get("exit_code")
            .and_then(|value| value.as_i64())
            .and_then(|value| i32::try_from(value).ok()),
        reason: value
            .get("reason")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        duration_ms: value
            .get("duration_ms")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        command_sha256_hex: value
            .get("command_sha256_hex")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        timed_out: value
            .get("timed_out")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        output_truncated: value
            .get("output_truncated")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        stdout_sha256_hex: value
            .get("stdout_sha256_hex")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        stderr_sha256_hex: value
            .get("stderr_sha256_hex")
            .and_then(|value| value.as_str())
            .map(str::to_string),
    })
}

fn telemetry_rollup_from_row(row: sqlx::postgres::PgRow) -> Result<TelemetryRollupView> {
    Ok(TelemetryRollupView {
        client_id: row.try_get("client_id")?,
        bucket_start: row.try_get("bucket_start")?,
        bucket_secs: row.try_get("bucket_secs")?,
        sample_count: row.try_get("sample_count")?,
        cpu_usage_sample_count: row.try_get("cpu_usage_sample_count")?,
        cpu_usage_avg: row.try_get("cpu_usage_avg")?,
        cpu_usage_max: row.try_get("cpu_usage_max")?,
        cpu_cores_max: row.try_get("cpu_cores_max")?,
        cpu_load_1_avg: row.try_get("cpu_load_1_avg")?,
        cpu_load_1_max: row.try_get("cpu_load_1_max")?,
        cpu_load_5_avg: row.try_get("cpu_load_5_avg")?,
        cpu_load_5_max: row.try_get("cpu_load_5_max")?,
        cpu_load_15_avg: row.try_get("cpu_load_15_avg")?,
        cpu_load_15_max: row.try_get("cpu_load_15_max")?,
        memory_total_bytes_max: row.try_get("memory_total_bytes_max")?,
        memory_available_bytes_avg: row.try_get("memory_available_bytes_avg")?,
        memory_available_bytes_min: row.try_get("memory_available_bytes_min")?,
        memory_used_ratio_avg: row.try_get("memory_used_ratio_avg")?,
        memory_used_ratio_max: row.try_get("memory_used_ratio_max")?,
        swap_sample_count: row.try_get("swap_sample_count")?,
        swap_total_bytes_max: row.try_get("swap_total_bytes_max")?,
        swap_available_bytes_avg: row.try_get("swap_available_bytes_avg")?,
        swap_available_bytes_min: row.try_get("swap_available_bytes_min")?,
        swap_used_ratio_avg: row.try_get("swap_used_ratio_avg")?,
        swap_used_ratio_max: row.try_get("swap_used_ratio_max")?,
        disk_total_bytes_max: row.try_get("disk_total_bytes_max")?,
        disk_available_bytes_avg: row.try_get("disk_available_bytes_avg")?,
        disk_available_bytes_min: row.try_get("disk_available_bytes_min")?,
        disk_used_ratio_avg: row.try_get("disk_used_ratio_avg")?,
        disk_used_ratio_max: row.try_get("disk_used_ratio_max")?,
        network_rx_bytes_max: row.try_get("network_rx_bytes_max")?,
        network_tx_bytes_max: row.try_get("network_tx_bytes_max")?,
        connections_sample_count: row.try_get("connections_sample_count")?,
        tcp_sockets_latest: row.try_get("tcp_sockets_latest")?,
        udp_sockets_latest: row.try_get("udp_sockets_latest")?,
        connections_observed_at: row.try_get("connections_observed_at")?,
        latest_observed_at: row.try_get("latest_observed_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn telemetry_network_rate_from_row(row: sqlx::postgres::PgRow) -> Result<TelemetryNetworkRateView> {
    Ok(TelemetryNetworkRateView {
        client_id: row.try_get("client_id")?,
        interface: row.try_get("interface")?,
        bucket_start: row.try_get("bucket_start")?,
        bucket_secs: row.try_get("bucket_secs")?,
        sample_count: row.try_get("sample_count")?,
        rx_bytes_avg: row.try_get("rx_bytes_avg")?,
        tx_bytes_avg: row.try_get("tx_bytes_avg")?,
        rx_bytes_last: row.try_get("rx_bytes_last")?,
        tx_bytes_last: row.try_get("tx_bytes_last")?,
        rx_counter_epoch: row.try_get("rx_counter_epoch")?,
        tx_counter_epoch: row.try_get("tx_counter_epoch")?,
        rx_bytes_delta: row.try_get("rx_bytes_delta")?,
        tx_bytes_delta: row.try_get("tx_bytes_delta")?,
        rx_bps_avg: row.try_get("rx_bps_avg")?,
        tx_bps_avg: row.try_get("tx_bps_avg")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn project_network_rate_selection(
    rows: Vec<TelemetryNetworkRateView>,
    selection: &NetworkRateInterfaceSelection,
) -> Vec<TelemetryNetworkRateView> {
    rows.into_iter()
        .filter(|row| selection.allows(&row.client_id, &row.interface))
        .collect()
}

fn timestamp_in_bounds(value: &str, start_unix: Option<u64>, end_unix: Option<u64>) -> bool {
    if start_unix.is_none() && end_unix.is_none() {
        return true;
    }
    parse_timestamp_unix(value).is_some_and(|timestamp| {
        start_unix.is_none_or(|start| timestamp >= start)
            && end_unix.is_none_or(|end| timestamp <= end)
    })
}

fn bucket_end_unix(bucket_start: u64, bucket_secs: i32) -> u64 {
    bucket_start.saturating_add(bucket_secs.max(1) as u64)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LogicalSpanFragment {
    pub(crate) chart_bucket_start: u64,
    pub(crate) first_minute: u64,
    pub(crate) minute_count: u64,
    pub(crate) source_minutes: u64,
}

pub(crate) fn logical_span_fragments(
    bucket_start: u64,
    bucket_secs: i32,
    start_unix: Option<u64>,
    end_unix: Option<u64>,
    step_secs: i32,
) -> Vec<LogicalSpanFragment> {
    if bucket_secs < 60 || bucket_secs % 60 != 0 {
        return Vec::new();
    }
    let source_minutes = bucket_secs as u64 / 60;
    let first_minute = start_unix
        .map(|start| start.saturating_sub(bucket_start).saturating_add(59) / 60)
        .unwrap_or(0)
        .min(source_minutes);
    let end_minute = end_unix
        .map(|end| {
            if end < bucket_start {
                0
            } else {
                end.saturating_sub(bucket_start)
                    .saturating_div(60)
                    .saturating_add(1)
                    .min(source_minutes)
            }
        })
        .unwrap_or(source_minutes);
    if first_minute >= end_minute {
        return Vec::new();
    }

    let step_secs = normalized_dashboard_step_secs(step_secs) as u64;
    let mut fragments = Vec::new();
    let mut minute = first_minute;
    while minute < end_minute {
        let logical_start = bucket_start.saturating_add(minute.saturating_mul(60));
        let chart_bucket_start = logical_start / step_secs * step_secs;
        let next_chart_start = chart_bucket_start.saturating_add(step_secs);
        let next_minute = next_chart_start
            .saturating_sub(bucket_start)
            .saturating_add(59)
            .saturating_div(60)
            .clamp(minute.saturating_add(1), end_minute);
        fragments.push(LogicalSpanFragment {
            chart_bucket_start,
            first_minute: minute,
            minute_count: next_minute.saturating_sub(minute),
            source_minutes,
        });
        minute = next_minute;
    }
    fragments
}

pub(crate) fn proportional_fragment_count(total: i32, fragment: LogicalSpanFragment) -> i32 {
    let total = i128::from(total.max(0));
    let source_minutes = i128::from(fragment.source_minutes.max(1));
    let first = i128::from(fragment.first_minute);
    let end = i128::from(fragment.first_minute.saturating_add(fragment.minute_count));
    let count =
        total.saturating_mul(end) / source_minutes - total.saturating_mul(first) / source_minutes;
    count.clamp(0, i128::from(i32::MAX)) as i32
}

pub(crate) fn fragment_final_minute_timestamp(
    value: &str,
    fragment: LogicalSpanFragment,
) -> String {
    let fragment_end_minute = fragment.first_minute.saturating_add(fragment.minute_count);
    let omitted_seconds = fragment
        .source_minutes
        .saturating_sub(fragment_end_minute)
        .saturating_mul(60);
    parse_timestamp_unix(value)
        .map(|timestamp| timestamp.saturating_sub(omitted_seconds).to_string())
        .unwrap_or_else(|| value.to_string())
}

fn fragment_telemetry_rollup(
    row: TelemetryRollupView,
    start_unix: Option<u64>,
    end_unix: Option<u64>,
    step_secs: i32,
) -> Vec<TelemetryRollupView> {
    let Some(bucket_start) = parse_timestamp_unix(&row.bucket_start) else {
        return Vec::new();
    };
    logical_span_fragments(
        bucket_start,
        row.bucket_secs,
        start_unix,
        end_unix,
        step_secs,
    )
    .into_iter()
    .filter_map(|fragment| {
        let sample_count = proportional_fragment_count(row.sample_count, fragment);
        if sample_count == 0 {
            return None;
        }
        let cpu_usage_sample_count =
            proportional_fragment_count(row.cpu_usage_sample_count, fragment).min(sample_count);
        let swap_sample_count =
            proportional_fragment_count(row.swap_sample_count, fragment).min(sample_count);
        let carry_swap_capacity = swap_sample_count > 0
            || (row.swap_sample_count == 0 && row.swap_total_bytes_max == Some(0));
        let connections_sample_count =
            proportional_fragment_count(row.connections_sample_count, fragment).min(sample_count);
        let latest_observed_at = fragment_final_minute_timestamp(&row.latest_observed_at, fragment);
        let connections_observed_at = (connections_sample_count > 0)
            .then(|| {
                row.connections_observed_at
                    .as_deref()
                    .map(|observed| fragment_final_minute_timestamp(observed, fragment))
            })
            .flatten();
        Some(TelemetryRollupView {
            bucket_start: fragment.chart_bucket_start.to_string(),
            bucket_secs: normalized_dashboard_step_secs(step_secs),
            sample_count,
            cpu_usage_sample_count,
            swap_sample_count,
            swap_total_bytes_max: carry_swap_capacity
                .then_some(row.swap_total_bytes_max)
                .flatten(),
            swap_available_bytes_avg: carry_swap_capacity
                .then_some(row.swap_available_bytes_avg)
                .flatten(),
            swap_available_bytes_min: carry_swap_capacity
                .then_some(row.swap_available_bytes_min)
                .flatten(),
            swap_used_ratio_avg: (swap_sample_count > 0)
                .then_some(row.swap_used_ratio_avg)
                .flatten(),
            swap_used_ratio_max: (swap_sample_count > 0)
                .then_some(row.swap_used_ratio_max)
                .flatten(),
            connections_sample_count,
            tcp_sockets_latest: (connections_sample_count > 0)
                .then_some(row.tcp_sockets_latest)
                .flatten(),
            udp_sockets_latest: (connections_sample_count > 0)
                .then_some(row.udp_sockets_latest)
                .flatten(),
            connections_observed_at,
            latest_observed_at,
            ..row.clone()
        })
    })
    .collect()
}

fn bucket_last_sample_unix(bucket_start: &str, bucket_secs: i32) -> u64 {
    parse_timestamp_unix(bucket_start)
        .unwrap_or(0)
        .saturating_add(bucket_secs.max(60) as u64)
        .saturating_sub(60)
}

fn bucket_overlaps_bounds(
    bucket_start: &str,
    bucket_secs: i32,
    start_unix: Option<u64>,
    end_unix: Option<u64>,
) -> bool {
    let Some(bucket_start) = parse_timestamp_unix(bucket_start) else {
        return false;
    };
    end_unix.is_none_or(|end| bucket_start <= end)
        && start_unix.is_none_or(|start| bucket_end_unix(bucket_start, bucket_secs) > start)
}

fn normalized_dashboard_step_secs(step_secs: i32) -> i32 {
    step_secs.max(60).saturating_add(59) / 60 * 60
}

fn parse_timestamp_unix(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(timestamp) = value.parse::<i64>() {
        return (timestamp >= 0).then_some(timestamp as u64);
    }
    DateTime::parse_from_rfc3339(value)
        .ok()
        .or_else(|| DateTime::parse_from_rfc3339(&normalize_postgres_timestamp(value)).ok())
        .map(|timestamp| timestamp.timestamp())
        .filter(|timestamp| *timestamp >= 0)
        .map(|timestamp| timestamp as u64)
}

fn normalize_postgres_timestamp(value: &str) -> String {
    let mut normalized = value.replacen(' ', "T", 1);
    if let Some(offset_start) = normalized.rfind(['+', '-']) {
        let offset = &normalized[offset_start..];
        if offset.len() == 3 {
            normalized.push_str(":00");
        } else if offset.len() == 5 && !offset.contains(':') {
            normalized.insert(offset_start + 3, ':');
        }
    }
    normalized
}

fn runtime_manager_label(manager: vpsman_common::RuntimeTunnelManager) -> &'static str {
    match manager {
        vpsman_common::RuntimeTunnelManager::AgentIproute2Managed => "agent_iproute2_managed",
        vpsman_common::RuntimeTunnelManager::ExternalObserved => "external_observed",
        vpsman_common::RuntimeTunnelManager::ExternalManagedAdapter => "external_managed_adapter",
    }
}

fn matched_plan_mutation_policy(manager: vpsman_common::RuntimeTunnelManager) -> &'static str {
    match manager {
        vpsman_common::RuntimeTunnelManager::ExternalObserved => "observe_only_saved_plan",
        vpsman_common::RuntimeTunnelManager::AgentIproute2Managed
        | vpsman_common::RuntimeTunnelManager::ExternalManagedAdapter => "managed_desired",
    }
}

#[cfg(test)]
#[path = "tests_repository_telemetry_rollups.rs"]
mod fairness_tests;
