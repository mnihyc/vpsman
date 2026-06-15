use std::collections::BTreeMap;

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use chrono::Utc;
use serde::Deserialize;
use vpsman_common::SuiteConfig;

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

const SYSTEM_DASHBOARD_BUCKET_SECS: i32 = 60;
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
    state.require_operator_scope(&headers, "fleet:read").await?;
    let window = normalize_window(query.window.as_deref());
    let chart_points = query
        .chart_points
        .unwrap_or(DEFAULT_CHART_POINTS)
        .clamp(1, MAX_CHART_POINTS);
    let now = unix_now();
    let start = now.saturating_sub(window_seconds(window));
    let collected = collect_system_dashboard_snapshot(&state).await?;
    let rollups = state
        .repo
        .list_system_metric_rollups(start, now, chart_points)
        .await?;
    Ok(Json(SystemDashboardView {
        generated_at: Utc::now().to_rfc3339(),
        window: window.to_string(),
        bucket_secs: SYSTEM_DASHBOARD_BUCKET_SECS,
        current: collected.current,
        capacity: suite_capacity(&state),
        series: system_metric_series(rollups),
        notes: collected.notes,
    }))
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
        Ok(metrics) => SystemDashboardGatewayEventsView {
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
            status: if metrics.unhealthy {
                "unhealthy".to_string()
            } else {
                "live".to_string()
            },
        },
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

fn normalize_window(value: Option<&str>) -> &'static str {
    match value.unwrap_or("24h").trim() {
        "15m" => "15m",
        "1h" => "1h",
        "6h" => "6h",
        "7d" => "7d",
        "30d" => "30d",
        _ => "24h",
    }
}

fn window_seconds(window: &str) -> u64 {
    match window {
        "15m" => 900,
        "1h" => 3_600,
        "6h" => 21_600,
        "7d" => 604_800,
        "30d" => 2_592_000,
        _ => 86_400,
    }
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
        dispatch_ack_secs: Some(dispatcher_config.dispatch_ack_secs),
        event_post_secs: Some(dispatcher_config.event_post_secs),
        internal_http_read_secs: Some(dispatcher_config.internal_http_read_secs),
        worker_schedule_command_secs: config
            .timeout
            .worker_schedule_command_secs
            .or(config.worker.schedule_command_timeout_secs),
        agent_offline_secs: config
            .timeout
            .agent_offline_secs
            .or(config.worker.agent_offline_timeout_secs),
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
        "db_pool.max_connections" => ("DB max connections", "connections"),
        "db_pool.open_connections" => ("DB open connections", "connections"),
        "db_pool.idle_connections" => ("DB idle connections", "connections"),
        "db_pool.in_use_connections" => ("DB in-use connections", "connections"),
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
        _ => ("System metric", "count"),
    }
}
