use std::collections::BTreeMap;

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use chrono::Utc;
use serde::Deserialize;
use vpsman_common::{GatewayForwardMetricsSnapshot, SuiteConfig};

use crate::{
    error::ApiError,
    model::{
        SystemDashboardCapacityView, SystemDashboardGatewayEventsView, SystemDashboardSnapshotView,
        SystemDashboardView, SystemMetricPointView, SystemMetricRollupView, SystemMetricSeriesView,
    },
    repository_system_dashboard::system_metric_samples_from_snapshot,
    state::AppState,
    unix_now,
};

const DEFAULT_CHART_POINTS: i64 = 240;
const MAX_CHART_POINTS: i64 = 1440;

#[derive(Debug, Deserialize)]
pub(crate) struct SystemDashboardQuery {
    pub(crate) window: Option<String>,
    pub(crate) chart_points: Option<i64>,
}

pub(crate) async fn system_dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SystemDashboardQuery>,
) -> Result<Json<SystemDashboardView>, ApiError> {
    let operator = state.require_operator_scope(&headers, "fleet:read").await?;
    let (window, chart_points) = normalize_system_dashboard_query(&query)?;
    let key = system_dashboard_singleflight_key(
        operator.operator.id,
        &operator.operator.scopes,
        window,
        chart_points,
    );
    let events = state.events.clone();
    let response = events
        .singleflight_system_dashboard(key, move || async move {
            load_prepared_system_dashboard(&state, window, chart_points).await
        })
        .await?;
    Ok(Json(response))
}

pub(crate) async fn load_system_dashboard(
    state: &AppState,
    query: &SystemDashboardQuery,
) -> Result<SystemDashboardView, ApiError> {
    let (window, chart_points) = normalize_system_dashboard_query(query)?;
    load_prepared_system_dashboard(state, window, chart_points).await
}

fn normalize_system_dashboard_query(
    query: &SystemDashboardQuery,
) -> Result<(&'static str, i64), ApiError> {
    Ok((
        validate_window(query.window.as_deref())?,
        query
            .chart_points
            .unwrap_or(DEFAULT_CHART_POINTS)
            .clamp(1, MAX_CHART_POINTS),
    ))
}

fn system_dashboard_singleflight_key(
    operator_id: uuid::Uuid,
    scopes: &[String],
    window: &'static str,
    chart_points: i64,
) -> String {
    serde_json::json!({
        "endpoint": "system_dashboard",
        "auth": crate::state::read_singleflight_auth_key(operator_id, scopes),
        "window": window,
        "chart_points": chart_points,
    })
    .to_string()
}

async fn load_prepared_system_dashboard(
    state: &AppState,
    window: &'static str,
    chart_points: i64,
) -> Result<SystemDashboardView, ApiError> {
    let now = unix_now();
    let earliest_system_bucket = if window == "all" {
        state
            .repo
            .earliest_system_metric_bucket_unix()
            .await
            .map_err(ApiError::internal_mapper(
                "system_metrics_unavailable",
                "System metric history could not be loaded.",
            ))?
    } else {
        None
    };
    let start = system_dashboard_start(now, window, earliest_system_bucket);
    let span = now.saturating_sub(start);
    let requested_step_secs = requested_chart_step_secs(span, chart_points);
    let effective_resolution_secs = retained_system_resolution_for_age(span);
    let bucket_secs = tier_aligned_system_step_secs(
        span,
        requested_step_secs,
        effective_resolution_secs,
        chart_points as u64,
    );
    let effective_points = span
        .checked_div(bucket_secs.max(1) as u64)
        .unwrap_or(0)
        .saturating_add(1)
        .min(MAX_CHART_POINTS as u64) as i64;
    let (collected, rollups) = tokio::join!(
        collect_system_dashboard_snapshot(state),
        state
            .repo
            .list_system_metric_rollups_at_step(start, now, bucket_secs as u64,),
    );
    let collected = collected.map_err(ApiError::internal_mapper(
        "system_dashboard_unavailable",
        "The system dashboard could not be loaded.",
    ))?;
    let rollups = rollups.map_err(ApiError::internal_mapper(
        "system_metrics_unavailable",
        "System metric history could not be loaded.",
    ))?;
    Ok(SystemDashboardView {
        generated_at: Utc::now().to_rfc3339(),
        window: window.to_string(),
        requested_step_secs,
        effective_resolution_secs,
        bucket_secs,
        effective_points,
        current: collected.current,
        capacity: suite_capacity(state),
        series: system_metric_series(rollups),
        notes: collected.notes,
    })
}

pub(crate) async fn record_system_dashboard_sample(state: &AppState) -> anyhow::Result<()> {
    let collected = collect_system_dashboard_snapshot(state).await?;
    let samples = system_metric_samples_from_snapshot(
        &collected.repository,
        &collected.current.gateway_events,
    );
    state
        .repo
        .record_system_metric_samples(unix_now(), &samples)
        .await
}

pub(crate) struct CollectedSystemDashboard {
    pub(crate) repository: crate::repository_system_dashboard::SystemDashboardRepositorySnapshot,
    pub(crate) current: SystemDashboardSnapshotView,
    pub(crate) notes: Vec<String>,
}

pub(crate) async fn collect_system_dashboard_snapshot(
    state: &AppState,
) -> anyhow::Result<CollectedSystemDashboard> {
    let snapshot = state.repo.system_dashboard_snapshot().await?;
    let mut notes = Vec::new();
    state.refresh_gateway_dispatch_timeouts();
    let gateway_events = match state.gateway.forward_metrics().await {
        Ok(metrics) => gateway_events_view(metrics),
        Err(error) => {
            notes.push(format!("gateway event metrics unavailable: {error}"));
            SystemDashboardGatewayEventsView {
                status: "unavailable".to_string(),
                ..SystemDashboardGatewayEventsView::default()
            }
        }
    };
    Ok(CollectedSystemDashboard {
        current: SystemDashboardSnapshotView {
            db_pool: snapshot.db_pool.clone(),
            dispatch: snapshot.dispatch.clone(),
            targets: snapshot.targets.clone(),
            cancellations: snapshot.cancellations.clone(),
            gateway_events,
        },
        repository: snapshot,
        notes,
    })
}

fn gateway_events_view(metrics: GatewayForwardMetricsSnapshot) -> SystemDashboardGatewayEventsView {
    SystemDashboardGatewayEventsView {
        queued_events: Some(metrics.queued_events),
        delivered_events: Some(metrics.delivered_events),
        retry_attempts: Some(metrics.retry_attempts),
        active_queues: Some(metrics.active_queues),
        current_queue_depth: Some(metrics.current_queue_depth),
        oldest_event_age_secs: metrics.oldest_event_age_secs,
        dropped_events: Some(metrics.dropped_events),
        telemetry_dropped_events: Some(metrics.telemetry_dropped_events),
        expired_events: Some(metrics.expired_events),
        critical_failures: Some(metrics.critical_failures),
        dropped_by_kind: metrics.dropped_by_kind,
        dropped_by_reason: metrics.dropped_by_reason,
        critical_failures_by_reason: metrics.critical_failures_by_reason,
        retained_output_truncated_events: Some(metrics.retained_output_truncated_events),
        rejected_agent_connections: Some(metrics.rejected_agent_connections),
        telemetry_admission_limit: Some(metrics.telemetry_admission_limit),
        telemetry_admission_active: Some(metrics.telemetry_admission_active),
        telemetry_admission_waiting: Some(metrics.telemetry_admission_waiting),
        status: if metrics.unhealthy {
            "unhealthy".to_string()
        } else {
            "live".to_string()
        },
    }
}

fn validate_window(value: Option<&str>) -> Result<&'static str, ApiError> {
    let window = match value.unwrap_or("1d").trim() {
        "15m" => "15m",
        "1h" => "1h",
        "8h" => "8h",
        "1d" => "1d",
        "7d" => "7d",
        "30d" => "30d",
        "90d" => "90d",
        "180d" => "180d",
        "1y" => "1y",
        "all" => "all",
        _ => return Err(ApiError::bad_request("invalid_system_dashboard_window")),
    };
    Ok(window)
}

fn window_seconds(window: &str) -> u64 {
    match window {
        "15m" => 900,
        "1h" => 3_600,
        "8h" => 28_800,
        "1d" => 86_400,
        "7d" => 604_800,
        "30d" => 2_592_000,
        "90d" => 7_776_000,
        "180d" => 15_552_000,
        "1y" => 31_536_000,
        "all" => u64::MAX,
        _ => unreachable!("validated system dashboard window"),
    }
}

fn system_dashboard_start(now: u64, window: &str, earliest_system_bucket: Option<u64>) -> u64 {
    if window == "all" {
        earliest_system_bucket.unwrap_or(now).min(now)
    } else {
        now.saturating_sub(window_seconds(window))
    }
}

fn requested_chart_step_secs(span: u64, points: i64) -> i32 {
    let intervals = points.clamp(2, MAX_CHART_POINTS).saturating_sub(1) as u64;
    let raw = span.div_ceil(intervals.max(1)).max(60);
    raw.div_ceil(60).saturating_mul(60).min(i32::MAX as u64) as i32
}

fn retained_system_resolution_for_age(age_secs: u64) -> i32 {
    const DAY: u64 = 24 * 60 * 60;
    match age_secs {
        value if value <= 2 * DAY => 60,
        value if value <= 8 * DAY => 5 * 60,
        value if value <= 31 * DAY => 30 * 60,
        value if value <= 91 * DAY => 60 * 60,
        value if value <= 181 * DAY => 3 * 60 * 60,
        value if value <= 366 * DAY => 6 * 60 * 60,
        _ => DAY as i32,
    }
}

fn tier_aligned_system_step_secs(
    span: u64,
    requested_step_secs: i32,
    effective_resolution_secs: i32,
    requested_points: u64,
) -> i32 {
    let resolution = effective_resolution_secs.max(60) as u64;
    let requested = (requested_step_secs.max(60) as u64).max(resolution);
    let lower = requested / resolution * resolution;
    if lower >= resolution && span / lower < requested_points.saturating_add(12) {
        return lower.min(i32::MAX as u64) as i32;
    }
    requested
        .div_ceil(resolution)
        .saturating_mul(resolution)
        .min(i32::MAX as u64) as i32
}

fn suite_capacity(state: &AppState) -> SystemDashboardCapacityView {
    let Ok(config) = SuiteConfig::load_optional(&state.suite_config_path) else {
        return SystemDashboardCapacityView::default();
    };
    let dispatcher_config = state.dispatcher_runtime_config();
    SystemDashboardCapacityView {
        api_db_pool: config.capacity.api_db_pool,
        worker_db_pool: config.capacity.worker_db_pool,
        dispatcher_batch: Some(dispatcher_config.batch_limit),
        dispatcher_in_flight: Some(dispatcher_config.in_flight),
        gateway_telemetry_in_flight: config.capacity.gateway_telemetry_in_flight,
        dispatch_ack_secs: Some(dispatcher_config.dispatch_ack_secs),
        event_post_secs: Some(dispatcher_config.event_post_secs),
        internal_http_read_secs: Some(dispatcher_config.internal_http_read_secs),
        control_deadline_grace_secs: Some(dispatcher_config.control_deadline_grace_secs),
        max_job_timeout_secs: Some(dispatcher_config.max_job_timeout_secs),
        schedule_job_max_timeout_secs: config.worker.schedule_job_max_timeout_secs,
        agent_offline_timeout_secs: config.worker.agent_offline_timeout_secs,
    }
}

fn system_metric_series(rollups: Vec<SystemMetricRollupView>) -> Vec<SystemMetricSeriesView> {
    let mut grouped: BTreeMap<String, Vec<SystemMetricPointView>> = BTreeMap::new();
    for rollup in rollups {
        grouped
            .entry(rollup.metric)
            .or_default()
            .push(SystemMetricPointView {
                bucket_start: rollup.bucket_start,
                avg_value: rollup.avg_value,
                max_value: rollup.max_value,
                latest_value: rollup.latest_value,
                sample_count: rollup.sample_count,
            });
    }
    grouped
        .into_iter()
        .map(|(metric, points)| {
            let (label, unit) = system_metric_label_unit(&metric);
            SystemMetricSeriesView {
                metric,
                label: label.to_string(),
                unit: unit.to_string(),
                points,
            }
        })
        .collect()
}

fn system_metric_label_unit(metric: &str) -> (&'static str, &'static str) {
    match metric {
        "db_pool.max_connections" => ("Foreground DB max connections", "connections"),
        "db_pool.open_connections" => ("Foreground DB open connections", "connections"),
        "db_pool.idle_connections" => ("Foreground DB idle connections", "connections"),
        "db_pool.in_use_connections" => ("Foreground DB in-use connections", "connections"),
        "dispatch.active_jobs" => ("Active jobs", "jobs"),
        "dispatch.queued_jobs" => ("Queued jobs", "jobs"),
        "dispatch.running_jobs" => ("Running jobs", "jobs"),
        "dispatch.queue_depth" => ("Dispatch queue depth", "targets"),
        "dispatch.total_dispatch_attempts" => ("Dispatch attempts", "attempts"),
        "dispatch.retried_targets" => ("Retried targets", "targets"),
        "targets.queued" => ("Queued targets", "targets"),
        "targets.dispatching" => ("Dispatching targets", "targets"),
        "targets.running" => ("Running targets", "targets"),
        "targets.active" => ("Active targets", "targets"),
        "targets.deadline_expired_active" => ("Expired active targets", "targets"),
        "targets.control_timeout_last_24h" => ("Control timeouts", "targets"),
        "targets.agent_timeout_last_24h" => ("Agent timeouts", "targets"),
        "targets.agent_lost_last_24h" => ("Agent lost", "targets"),
        "targets.canceled_last_24h" => ("Canceled targets", "targets"),
        "cancellations.requested" => ("Cancel requested", "targets"),
        "cancellations.sent" => ("Cancel sent", "targets"),
        "cancellations.acked" => ("Cancel acked", "targets"),
        "cancellations.awaiting_ack" => ("Cancel awaiting ack", "targets"),
        "gateway_events.queued_events" => ("Gateway queued events", "events"),
        "gateway_events.delivered_events" => ("Gateway delivered events", "events"),
        "gateway_events.retry_attempts" => ("Gateway retry attempts", "attempts"),
        "gateway_events.active_queues" => ("Gateway active queues", "queues"),
        "gateway_events.current_queue_depth" => ("Gateway queue depth", "events"),
        "gateway_events.oldest_event_age_secs" => ("Gateway oldest event age", "seconds"),
        "gateway_events.dropped_events" => ("Gateway dropped events", "events"),
        "gateway_events.telemetry_dropped_events" => ("Gateway telemetry drops", "events"),
        "gateway_events.expired_events" => ("Gateway expired events", "events"),
        "gateway_events.critical_failures" => ("Gateway critical failures", "events"),
        "gateway_events.dropped_by_kind.telemetry" => ("Gateway telemetry drops by kind", "events"),
        "gateway_events.dropped_by_kind.command_output" => {
            ("Gateway command output drops", "events")
        }
        "gateway_events.dropped_by_kind.lifecycle" => ("Gateway lifecycle drops", "events"),
        "gateway_events.dropped_by_kind.terminal_output" => {
            ("Gateway terminal output drops", "events")
        }
        "gateway_events.dropped_by_kind.other" => ("Gateway other drops", "events"),
        "gateway_events.dropped_by_reason.global_queue_full" => {
            ("Gateway global queue full drops", "events")
        }
        "gateway_events.dropped_by_reason.target_queue_full" => {
            ("Gateway target queue full drops", "events")
        }
        "gateway_events.dropped_by_reason.expired" => ("Gateway expired drops", "events"),
        "gateway_events.dropped_by_reason.coalesced" => ("Gateway coalesced telemetry", "events"),
        "gateway_events.critical_failures_by_reason.global_queue_full" => {
            ("Gateway critical global queue failures", "events")
        }
        "gateway_events.critical_failures_by_reason.target_queue_full" => {
            ("Gateway critical target queue failures", "events")
        }
        "gateway_events.critical_failures_by_reason.expired" => {
            ("Gateway critical expired failures", "events")
        }
        "gateway_events.retained_output_truncated_events" => {
            ("Gateway retained output truncations", "events")
        }
        "gateway_events.rejected_agent_connections" => {
            ("Gateway rejected agent connections", "connections")
        }
        "gateway_events.telemetry_admission_limit" => {
            ("Gateway telemetry admission limit", "posts")
        }
        "gateway_events.telemetry_admission_active" => ("Gateway telemetry posts active", "posts"),
        "gateway_events.telemetry_admission_waiting" => {
            ("Gateway telemetry posts waiting", "posts")
        }
        _ => ("System metric", "count"),
    }
}

#[cfg(test)]
#[path = "tests_routes_system.rs"]
mod window_tests;
