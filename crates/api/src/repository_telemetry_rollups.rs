use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use chrono::DateTime;
use sqlx::Row;

use crate::{
    model::{
        TelemetryNetworkRateView, TelemetryRollupView, TelemetryTunnelAdapterHealthView,
        TelemetryTunnelView, TunnelPlanView,
    },
    repository::Repository,
};

const TELEMETRY_LIST_LIMIT_MAX: i64 = 50_000;

impl Repository {
    pub(crate) async fn list_dashboard_telemetry_rollups(
        &self,
        limit: i64,
        start_unix: Option<u64>,
        end_unix: Option<u64>,
        bucket_secs: Option<i32>,
        step_secs: i32,
    ) -> Result<Vec<TelemetryRollupView>> {
        let step_secs = normalized_dashboard_step_secs(step_secs);
        match self {
            Self::Memory(memory) => {
                let mut rows = memory
                    .telemetry_rollups
                    .read()
                    .await
                    .iter()
                    .filter(|rollup| {
                        bucket_secs.is_none_or(|bucket_secs| rollup.bucket_secs == bucket_secs)
                            && timestamp_in_bounds(&rollup.bucket_start, start_unix, end_unix)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                rows.sort_by(|left, right| {
                    left.bucket_start
                        .cmp(&right.bucket_start)
                        .then_with(|| left.client_id.cmp(&right.client_id))
                });
                rows.truncate(limit.clamp(1, 50_000) as usize);
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH selected AS (
                        SELECT
                            client_id,
                            to_timestamp(
                                floor(
                                    extract(epoch FROM bucket_start) / $4::double precision
                                ) * $4::double precision
                            ) AS chart_bucket_start,
                            sample_count,
                            cpu_load_1_avg,
                            cpu_load_1_max,
                            memory_total_bytes_max,
                            memory_available_bytes_avg,
                            memory_available_bytes_min,
                            disk_total_bytes_max,
                            disk_available_bytes_avg,
                            disk_available_bytes_min,
                            network_rx_bytes_max,
                            network_tx_bytes_max,
                            latest_observed_at,
                            updated_at
                        FROM telemetry_rollups
                        WHERE
                            ($1::INTEGER IS NULL OR bucket_secs = $1)
                            AND ($2::BIGINT IS NULL OR bucket_start >= to_timestamp($2))
                            AND ($3::BIGINT IS NULL OR bucket_start <= to_timestamp($3))
                    )
                    SELECT
                        client_id,
                        chart_bucket_start::text AS bucket_start,
                        $4::INTEGER AS bucket_secs,
                        LEAST(sum(sample_count)::bigint, 2147483647)::integer AS sample_count,
                        COALESCE(
                            sum(cpu_load_1_avg * sample_count::double precision)
                                / NULLIF(sum(sample_count)::double precision, 0),
                            0
                        ) AS cpu_load_1_avg,
                        max(cpu_load_1_max)::double precision AS cpu_load_1_max,
                        max(memory_total_bytes_max)::bigint AS memory_total_bytes_max,
                        round(COALESCE(
                            sum(memory_available_bytes_avg::numeric * sample_count::numeric)
                                / NULLIF(sum(sample_count)::numeric, 0),
                            0
                        ))::bigint AS memory_available_bytes_avg,
                        min(memory_available_bytes_min)::bigint AS memory_available_bytes_min,
                        max(disk_total_bytes_max)::bigint AS disk_total_bytes_max,
                        round(COALESCE(
                            sum(disk_available_bytes_avg::numeric * sample_count::numeric)
                                / NULLIF(sum(sample_count)::numeric, 0),
                            0
                        ))::bigint AS disk_available_bytes_avg,
                        min(disk_available_bytes_min)::bigint AS disk_available_bytes_min,
                        max(network_rx_bytes_max)::bigint AS network_rx_bytes_max,
                        max(network_tx_bytes_max)::bigint AS network_tx_bytes_max,
                        max(latest_observed_at)::text AS latest_observed_at,
                        max(updated_at)::text AS updated_at
                    FROM selected
                    GROUP BY client_id, chart_bucket_start
                    ORDER BY chart_bucket_start ASC, client_id ASC
                    LIMIT $5
                    "#,
                )
                .bind(bucket_secs)
                .bind(start_unix.map(|value| value as i64))
                .bind(end_unix.map(|value| value as i64))
                .bind(step_secs)
                .bind(limit.clamp(1, 50_000))
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
    ) -> Result<Vec<TelemetryRollupView>> {
        match self {
            Self::Memory(memory) => {
                let mut rows = memory
                    .telemetry_rollups
                    .read()
                    .await
                    .iter()
                    .filter(|rollup| {
                        client_id.is_none_or(|client_id| rollup.client_id == client_id)
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
                        cpu_load_1_avg,
                        cpu_load_1_max,
                        memory_total_bytes_max,
                        memory_available_bytes_avg,
                        memory_available_bytes_min,
                        disk_total_bytes_max,
                        disk_available_bytes_avg,
                        disk_available_bytes_min,
                        network_rx_bytes_max,
                        network_tx_bytes_max,
                        latest_observed_at::text AS latest_observed_at,
                        updated_at::text AS updated_at
                    FROM telemetry_rollups
                    WHERE
                        ($1::TEXT IS NULL OR client_id = $1)
                        AND ($2::INTEGER IS NULL OR bucket_secs = $2)
                    ORDER BY bucket_start DESC, client_id ASC
                    LIMIT $3
                    "#,
                )
                .bind(client_id)
                .bind(bucket_secs)
                .bind(limit.clamp(1, TELEMETRY_LIST_LIMIT_MAX))
                .fetch_all(pool)
                .await?;

                rows.into_iter().map(telemetry_rollup_from_row).collect()
            }
        }
    }

    pub(crate) async fn list_dashboard_telemetry_network_rates(
        &self,
        limit: i64,
        start_unix: Option<u64>,
        end_unix: Option<u64>,
        bucket_secs: Option<i32>,
        step_secs: i32,
    ) -> Result<Vec<TelemetryNetworkRateView>> {
        let step_secs = normalized_dashboard_step_secs(step_secs);
        match self {
            Self::Memory(memory) => {
                let rows = memory
                    .telemetry_network_rates
                    .read()
                    .await
                    .iter()
                    .filter(|rate| {
                        bucket_secs.is_none_or(|bucket_secs| rate.bucket_secs == bucket_secs)
                            && timestamp_in_bounds(&rate.bucket_start, start_unix, end_unix)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let mut rows =
                    derive_network_rates(aggregate_memory_network_rates(rows, step_secs));
                rows.sort_by(|left, right| {
                    left.bucket_start
                        .cmp(&right.bucket_start)
                        .then_with(|| left.client_id.cmp(&right.client_id))
                        .then_with(|| left.interface.cmp(&right.interface))
                });
                rows.truncate(limit.clamp(1, 50_000) as usize);
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH selected AS (
                        SELECT
                            client_id,
                            interface,
                            to_timestamp(
                                floor(
                                    extract(epoch FROM bucket_start) / $4::double precision
                                ) * $4::double precision
                            ) AS chart_bucket_start,
                            sample_count,
                            rx_bytes_avg,
                            tx_bytes_avg,
                            updated_at
                        FROM telemetry_network_rates
                        WHERE
                            ($1::INTEGER IS NULL OR bucket_secs = $1)
                            AND ($2::BIGINT IS NULL OR bucket_start >= to_timestamp($2))
                            AND ($3::BIGINT IS NULL OR bucket_start <= to_timestamp($3))
                    ),
                    bucketed AS (
                        SELECT
                            client_id,
                            interface,
                            chart_bucket_start,
                            $4::INTEGER AS bucket_secs,
                            LEAST(sum(sample_count)::bigint, 2147483647)::integer AS sample_count,
                            round(COALESCE(
                                sum(rx_bytes_avg::numeric * sample_count::numeric)
                                    / NULLIF(sum(sample_count)::numeric, 0),
                                0
                            ))::bigint AS rx_bytes_avg,
                            round(COALESCE(
                                sum(tx_bytes_avg::numeric * sample_count::numeric)
                                    / NULLIF(sum(sample_count)::numeric, 0),
                                0
                            ))::bigint AS tx_bytes_avg,
                            max(updated_at)::text AS updated_at
                        FROM selected
                        GROUP BY client_id, interface, chart_bucket_start
                    ),
                    derived AS (
                        SELECT
                            bucketed.*,
                            lag(rx_bytes_avg) OVER rate_window AS previous_rx_bytes_avg,
                            lag(tx_bytes_avg) OVER rate_window AS previous_tx_bytes_avg,
                            lag(chart_bucket_start) OVER rate_window AS previous_bucket_start
                        FROM bucketed
                        WINDOW rate_window AS (
                            PARTITION BY client_id, interface
                            ORDER BY chart_bucket_start ASC
                        )
                    )
                    SELECT
                        client_id,
                        interface,
                        chart_bucket_start::text AS bucket_start,
                        bucket_secs,
                        sample_count,
                        rx_bytes_avg,
                        tx_bytes_avg,
                        GREATEST(rx_bytes_avg - COALESCE(previous_rx_bytes_avg, rx_bytes_avg), 0::bigint)
                            AS rx_bytes_delta,
                        GREATEST(tx_bytes_avg - COALESCE(previous_tx_bytes_avg, tx_bytes_avg), 0::bigint)
                            AS tx_bytes_delta,
                        CASE
                            WHEN previous_bucket_start IS NULL THEN 0::double precision
                            ELSE (
                                GREATEST(rx_bytes_avg - previous_rx_bytes_avg, 0::bigint) * 8
                            )::double precision / GREATEST(
                                extract(epoch FROM (chart_bucket_start - previous_bucket_start)),
                                1
                            )::double precision
                        END AS rx_bps_avg,
                        CASE
                            WHEN previous_bucket_start IS NULL THEN 0::double precision
                            ELSE (
                                GREATEST(tx_bytes_avg - previous_tx_bytes_avg, 0::bigint) * 8
                            )::double precision / GREATEST(
                                extract(epoch FROM (chart_bucket_start - previous_bucket_start)),
                                1
                            )::double precision
                        END AS tx_bps_avg,
                        updated_at
                    FROM derived
                    ORDER BY chart_bucket_start ASC, client_id ASC, interface ASC
                    LIMIT $5
                    "#,
                )
                .bind(bucket_secs)
                .bind(start_unix.map(|value| value as i64))
                .bind(end_unix.map(|value| value as i64))
                .bind(step_secs)
                .bind(limit.clamp(1, 50_000))
                .fetch_all(pool)
                .await?;

                rows.into_iter()
                    .map(telemetry_network_rate_from_row)
                    .collect()
            }
        }
    }

    pub(crate) async fn list_telemetry_network_rates(
        &self,
        limit: i64,
        client_id: Option<&str>,
        interface: Option<&str>,
        bucket_secs: Option<i32>,
    ) -> Result<Vec<TelemetryNetworkRateView>> {
        match self {
            Self::Memory(memory) => {
                let rows = memory
                    .telemetry_network_rates
                    .read()
                    .await
                    .iter()
                    .filter(|rate| {
                        client_id.is_none_or(|client_id| rate.client_id == client_id)
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
                            updated_at,
                            lag(rx_bytes_avg) OVER rate_window AS previous_rx_bytes_avg,
                            lag(tx_bytes_avg) OVER rate_window AS previous_tx_bytes_avg,
                            lag(bucket_start) OVER rate_window AS previous_bucket_start
                        FROM telemetry_network_rates
                        WHERE
                            ($1::TEXT IS NULL OR client_id = $1)
                            AND ($2::TEXT IS NULL OR interface = $2)
                            AND ($3::INTEGER IS NULL OR bucket_secs = $3)
                        WINDOW rate_window AS (
                            PARTITION BY client_id, interface, bucket_secs
                            ORDER BY bucket_start ASC
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
                        GREATEST(rx_bytes_avg - COALESCE(previous_rx_bytes_avg, rx_bytes_avg), 0::bigint)
                            AS rx_bytes_delta,
                        GREATEST(tx_bytes_avg - COALESCE(previous_tx_bytes_avg, tx_bytes_avg), 0::bigint)
                            AS tx_bytes_delta,
                        CASE
                            WHEN previous_bucket_start IS NULL THEN 0::double precision
                            ELSE (
                                GREATEST(rx_bytes_avg - previous_rx_bytes_avg, 0::bigint) * 8
                            )::double precision / GREATEST(
                                extract(epoch FROM (bucket_start - previous_bucket_start)),
                                1
                            )::double precision
                        END AS rx_bps_avg,
                        CASE
                            WHEN previous_bucket_start IS NULL THEN 0::double precision
                            ELSE (
                                GREATEST(tx_bytes_avg - previous_tx_bytes_avg, 0::bigint) * 8
                            )::double precision / GREATEST(
                                extract(epoch FROM (bucket_start - previous_bucket_start)),
                                1
                            )::double precision
                        END AS tx_bps_avg,
                        updated_at::text AS updated_at
                    FROM selected
                    ORDER BY bucket_start DESC, client_id ASC, interface ASC
                    LIMIT $4
                    "#,
                )
                .bind(client_id)
                .bind(interface)
                .bind(bucket_secs)
                .bind(limit.clamp(1, TELEMETRY_LIST_LIMIT_MAX))
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
        match self {
            Self::Memory(memory) => {
                let mut records = memory.telemetry_tunnels.read().await.clone();
                let plans = memory.tunnel_plans.read().await.clone();
                correlate_telemetry_tunnels_with_plans(&mut records, &plans);
                records.retain(|record| {
                    client_id.is_none_or(|expected| record.client_id == expected)
                        && interface.is_none_or(|expected| record.interface == expected)
                });
                records.sort_by(|left, right| {
                    right
                        .observed_at
                        .cmp(&left.observed_at)
                        .then_with(|| left.client_id.cmp(&right.client_id))
                        .then_with(|| left.interface.cmp(&right.interface))
                });
                records.truncate(limit.clamp(1, 200) as usize);
                Ok(records)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        client_id,
                        observed_at::text AS observed_at,
                        interface,
                        kind,
                        ownership_mode,
                        mutation_policy,
                        promotion_required,
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
                        adapter_health
                    FROM telemetry_tunnels
                    WHERE ($1::TEXT IS NULL OR client_id = $1)
                      AND ($2::TEXT IS NULL OR interface = $2)
                    ORDER BY observed_at DESC, client_id ASC, interface ASC
                    LIMIT $3
                    "#,
                )
                .bind(client_id)
                .bind(interface)
                .bind(limit.clamp(1, 200))
                .fetch_all(pool)
                .await?;

                let mut records = rows
                    .into_iter()
                    .map(|row| {
                        let telemetry_plan_id = row
                            .try_get::<Option<String>, _>("telemetry_plan_id")?
                            .and_then(|value| uuid::Uuid::parse_str(&value).ok());
                        let telemetry_plan_name =
                            row.try_get::<Option<String>, _>("telemetry_plan_name")?;
                        let plan_correlation =
                            if telemetry_plan_id.is_some() || telemetry_plan_name.is_some() {
                                "telemetry_reported_plan"
                            } else {
                                "unmatched"
                            };
                        Ok(TelemetryTunnelView {
                            client_id: row.try_get("client_id")?,
                            observed_at: row.try_get("observed_at")?,
                            interface: row.try_get("interface")?,
                            kind: row.try_get("kind")?,
                            ownership_mode: row.try_get("ownership_mode")?,
                            mutation_policy: row.try_get("mutation_policy")?,
                            promotion_required: row.try_get("promotion_required")?,
                            plan_correlation: plan_correlation.to_string(),
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
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let plans = self.list_tunnel_plans().await?;
                correlate_telemetry_tunnels_with_plans(&mut records, &plans);
                Ok(records)
            }
        }
    }
}

#[derive(Default)]
struct MemoryNetworkAggregate {
    sample_count: i32,
    rx_weighted_total: i128,
    tx_weighted_total: i128,
    updated_at: String,
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
        aggregate.rx_weighted_total +=
            i128::from(row.rx_bytes_avg) * i128::from(sample_count.max(1));
        aggregate.tx_weighted_total +=
            i128::from(row.tx_bytes_avg) * i128::from(sample_count.max(1));
        if row.updated_at > aggregate.updated_at {
            aggregate.updated_at = row.updated_at;
        }
    }

    groups
        .into_iter()
        .map(
            |((client_id, interface, bucket_secs, bucket_start), aggregate)| {
                let sample_count = aggregate.sample_count.max(1);
                TelemetryNetworkRateView {
                    client_id,
                    interface,
                    bucket_start: bucket_start.to_string(),
                    bucket_secs,
                    sample_count,
                    rx_bytes_avg: round_i128_div(aggregate.rx_weighted_total, sample_count),
                    tx_bytes_avg: round_i128_div(aggregate.tx_weighted_total, sample_count),
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
            .then_with(|| left.bucket_secs.cmp(&right.bucket_secs))
            .then_with(|| {
                parse_timestamp_unix(&left.bucket_start)
                    .unwrap_or(0)
                    .cmp(&parse_timestamp_unix(&right.bucket_start).unwrap_or(0))
            })
    });
    let mut previous_by_key = HashMap::<(String, String, i32), TelemetryNetworkRateView>::new();
    for row in &mut rows {
        let key = (
            row.client_id.clone(),
            row.interface.clone(),
            row.bucket_secs,
        );
        if let Some(previous) = previous_by_key.get(&key) {
            let current_ts = parse_timestamp_unix(&row.bucket_start).unwrap_or(0);
            let previous_ts = parse_timestamp_unix(&previous.bucket_start).unwrap_or(0);
            let duration = current_ts.saturating_sub(previous_ts).max(1) as f64;
            row.rx_bytes_delta = (row.rx_bytes_avg - previous.rx_bytes_avg).max(0);
            row.tx_bytes_delta = (row.tx_bytes_avg - previous.tx_bytes_avg).max(0);
            row.rx_bps_avg = (row.rx_bytes_delta * 8) as f64 / duration;
            row.tx_bps_avg = (row.tx_bytes_delta * 8) as f64 / duration;
        } else {
            row.rx_bytes_delta = 0;
            row.tx_bytes_delta = 0;
            row.rx_bps_avg = 0.0;
            row.tx_bps_avg = 0.0;
        }
        previous_by_key.insert(key, row.clone());
    }
    rows
}

fn round_i128_div(numerator: i128, denominator: i32) -> i64 {
    let denominator = i128::from(denominator.max(1));
    ((numerator + denominator / 2) / denominator).clamp(i128::from(i64::MIN), i128::from(i64::MAX))
        as i64
}

fn correlate_telemetry_tunnels_with_plans(
    records: &mut [TelemetryTunnelView],
    plans: &[TunnelPlanView],
) {
    for record in records {
        record.plan_correlation = if record.promotion_required {
            "unmatched_import_candidate".to_string()
        } else if record.plan_id.is_some() || record.plan_name.is_some() {
            "telemetry_reported_plan".to_string()
        } else {
            "unmatched".to_string()
        };
        let Some(match_record) = plans.iter().find_map(|plan| {
            if plan.plan.interface_name != record.interface {
                return None;
            }
            if plan.left_client_id == record.client_id {
                return Some((plan, "left", plan.right_client_id.as_str()));
            }
            if plan.right_client_id == record.client_id {
                return Some((plan, "right", plan.left_client_id.as_str()));
            }
            None
        }) else {
            continue;
        };
        let (plan, side, peer_client_id) = match_record;
        let manager = plan.plan.runtime_control.manager;
        let runtime_manager = runtime_manager_label(manager);
        record.plan_correlation = "matched_saved_plan".to_string();
        record.plan_id = Some(plan.id);
        record.plan_name = Some(plan.name.clone());
        record.plan_runtime_manager = Some(runtime_manager.to_string());
        record.endpoint_side = Some(side.to_string());
        record.peer_client_id = Some(peer_client_id.to_string());
        record.ownership_mode = runtime_manager.to_string();
        record.mutation_policy = matched_plan_mutation_policy(manager).to_string();
        record.promotion_required = false;
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
        cpu_load_1_avg: row.try_get("cpu_load_1_avg")?,
        cpu_load_1_max: row.try_get("cpu_load_1_max")?,
        memory_total_bytes_max: row.try_get("memory_total_bytes_max")?,
        memory_available_bytes_avg: row.try_get("memory_available_bytes_avg")?,
        memory_available_bytes_min: row.try_get("memory_available_bytes_min")?,
        disk_total_bytes_max: row.try_get("disk_total_bytes_max")?,
        disk_available_bytes_avg: row.try_get("disk_available_bytes_avg")?,
        disk_available_bytes_min: row.try_get("disk_available_bytes_min")?,
        network_rx_bytes_max: row.try_get("network_rx_bytes_max")?,
        network_tx_bytes_max: row.try_get("network_tx_bytes_max")?,
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
        rx_bytes_delta: row.try_get("rx_bytes_delta")?,
        tx_bytes_delta: row.try_get("tx_bytes_delta")?,
        rx_bps_avg: row.try_get("rx_bps_avg")?,
        tx_bps_avg: row.try_get("tx_bps_avg")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn timestamp_in_bounds(value: &str, start_unix: Option<u64>, end_unix: Option<u64>) -> bool {
    parse_timestamp_unix(value)
        .map(|timestamp| {
            start_unix.is_none_or(|start| timestamp >= start)
                && end_unix.is_none_or(|end| timestamp <= end)
        })
        .unwrap_or(true)
}

fn normalized_dashboard_step_secs(step_secs: i32) -> i32 {
    step_secs.clamp(60, 86_400)
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
