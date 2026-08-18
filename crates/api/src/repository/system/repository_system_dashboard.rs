use anyhow::Result;
use sqlx::Row;

use crate::{
    model::{
        SystemDashboardCancellationsView, SystemDashboardDbPoolView, SystemDashboardDispatchView,
        SystemDashboardTargetsView, SystemMetricRollupView,
    },
    repository::Repository,
};

pub(crate) const SYSTEM_METRIC_ROLLUP_AT_STEP_SQL: &str = r#"
WITH candidates AS (
    SELECT
        row.*,
        max(row.bucket_secs) OVER (PARTITION BY row.metric) AS max_bucket_secs
    FROM system_metric_rollups row
    WHERE
        row.bucket_start <= to_timestamp($2::double precision)
        AND row.bucket_start + make_interval(secs => row.bucket_secs)
            > to_timestamp($1::double precision)
),
selected AS (
    SELECT
        row.metric,
        to_timestamp(
            floor(
                extract(epoch FROM row.bucket_start) / $3::double precision
            ) * $3::double precision
        ) AS chart_bucket_start,
        GREATEST(row.bucket_secs, $3)::integer AS chart_bucket_secs,
        row.sample_count,
        row.value_sum,
        row.max_value,
        row.latest_value,
        row.latest_observed_at
    FROM candidates row
    LEFT JOIN LATERAL (
        SELECT TRUE AS overlaps
        FROM system_metric_rollups coarser
        WHERE row.bucket_secs < row.max_bucket_secs
          AND coarser.metric = row.metric
          AND coarser.bucket_secs > row.bucket_secs
          AND coarser.bucket_start <= to_timestamp($2::double precision)
          AND coarser.bucket_start + make_interval(secs => coarser.bucket_secs)
                > to_timestamp($1::double precision)
          AND coarser.bucket_start
                < row.bucket_start + make_interval(secs => row.bucket_secs)
          AND coarser.bucket_start + make_interval(secs => coarser.bucket_secs)
                > row.bucket_start
        LIMIT 1
    ) coarser_overlap ON TRUE
    WHERE coarser_overlap.overlaps IS NULL
)
SELECT
    metric,
    chart_bucket_start::text AS bucket_start,
    chart_bucket_secs AS bucket_secs,
    LEAST(sum(sample_count)::bigint, 2147483647)::integer AS sample_count,
    COALESCE(
        sum(value_sum) / NULLIF(sum(sample_count)::double precision, 0),
        0
    ) AS avg_value,
    max(max_value)::double precision AS max_value,
    (array_agg(latest_value ORDER BY latest_observed_at DESC))[1]
        AS latest_value
FROM selected
GROUP BY metric, chart_bucket_start, chart_bucket_secs
ORDER BY chart_bucket_start ASC, metric ASC
LIMIT $4
"#;

const SYSTEM_METRIC_BUCKET_SECS: i32 = 60;

#[derive(Clone, Debug)]
pub(crate) struct SystemDashboardRepositorySnapshot {
    pub(crate) db_pool: SystemDashboardDbPoolView,
    pub(crate) dispatch: SystemDashboardDispatchView,
    pub(crate) targets: SystemDashboardTargetsView,
    pub(crate) cancellations: SystemDashboardCancellationsView,
}

#[derive(Clone, Debug)]
pub(crate) struct SystemMetricSample {
    pub(crate) metric: &'static str,
    pub(crate) value: f64,
}

impl Repository {
    pub(crate) async fn system_dashboard_snapshot(
        &self,
    ) -> Result<SystemDashboardRepositorySnapshot> {
        match self {
            Self::Memory(state) => {
                let jobs = state.jobs.read().await;
                let targets = state.job_targets.read().await;
                let queued_jobs = jobs
                    .iter()
                    .filter(|job| job.completed_at.is_none() && job.status == "queued")
                    .count() as i64;
                let running_jobs = jobs
                    .iter()
                    .filter(|job| job.completed_at.is_none() && job.status == "running")
                    .count() as i64;
                let queued = targets
                    .iter()
                    .filter(|target| target.completed_at.is_none() && target.status == "queued")
                    .count() as i64;
                let dispatching = targets
                    .iter()
                    .filter(|target| {
                        target.completed_at.is_none() && target.status == "dispatching"
                    })
                    .count() as i64;
                let running = targets
                    .iter()
                    .filter(|target| target.completed_at.is_none() && target.status == "running")
                    .count() as i64;
                let control_timeout = targets
                    .iter()
                    .filter(|target| target.status == "control_timeout")
                    .count() as i64;
                let agent_timeout = targets
                    .iter()
                    .filter(|target| target.status == "agent_timeout")
                    .count() as i64;
                let agent_lost = targets
                    .iter()
                    .filter(|target| target.status == "agent_lost")
                    .count() as i64;
                let canceled = targets
                    .iter()
                    .filter(|target| target.status == "canceled")
                    .count() as i64;
                Ok(SystemDashboardRepositorySnapshot {
                    db_pool: SystemDashboardDbPoolView {
                        max_connections: 0,
                        open_connections: 0,
                        idle_connections: 0,
                        in_use_connections: 0,
                    },
                    dispatch: SystemDashboardDispatchView {
                        active_jobs: queued_jobs + running_jobs,
                        queued_jobs,
                        running_jobs,
                        queue_depth: queued + dispatching,
                        total_dispatch_attempts: 0,
                        retried_targets: 0,
                    },
                    targets: SystemDashboardTargetsView {
                        queued,
                        dispatching,
                        running,
                        active: dispatching + running,
                        deadline_expired_active: 0,
                        control_timeout_last_24h: control_timeout,
                        agent_timeout_last_24h: agent_timeout,
                        agent_lost_last_24h: agent_lost,
                        canceled_last_24h: canceled,
                    },
                    cancellations: SystemDashboardCancellationsView::default(),
                })
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        target_metrics.queued,
                        target_metrics.dispatching,
                        target_metrics.running,
                        target_metrics.active,
                        target_metrics.deadline_expired_active,
                        target_metrics.control_timeout_last_24h,
                        target_metrics.agent_timeout_last_24h,
                        target_metrics.agent_lost_last_24h,
                        target_metrics.canceled_last_24h,
                        target_metrics.total_dispatch_attempts,
                        target_metrics.retried_targets,
                        target_metrics.cancel_requested,
                        target_metrics.cancel_sent,
                        target_metrics.cancel_acked,
                        target_metrics.cancel_awaiting_ack,
                        job_metrics.active_jobs,
                        job_metrics.queued_jobs,
                        job_metrics.running_jobs
                    FROM (
                        SELECT
                            COUNT(*) FILTER (WHERE completed_at IS NULL AND status = 'queued')::bigint AS queued,
                            COUNT(*) FILTER (WHERE completed_at IS NULL AND status = 'dispatching')::bigint AS dispatching,
                            COUNT(*) FILTER (WHERE completed_at IS NULL AND status = 'running')::bigint AS running,
                            COUNT(*) FILTER (WHERE completed_at IS NULL AND status IN ('dispatching', 'running'))::bigint AS active,
                            COUNT(*) FILTER (
                                WHERE completed_at IS NULL
                                  AND status IN ('dispatching', 'running')
                                  AND deadline_at IS NOT NULL
                                  AND deadline_at <= now()
                            )::bigint AS deadline_expired_active,
                            COUNT(*) FILTER (
                                WHERE status = 'control_timeout'
                                  AND COALESCE(completed_at, result_received_at, started_at) >= now() - interval '24 hours'
                            )::bigint AS control_timeout_last_24h,
                            COUNT(*) FILTER (
                                WHERE status = 'agent_timeout'
                                  AND COALESCE(completed_at, result_received_at, started_at) >= now() - interval '24 hours'
                            )::bigint AS agent_timeout_last_24h,
                            COUNT(*) FILTER (
                                WHERE status = 'agent_lost'
                                  AND COALESCE(completed_at, result_received_at, started_at) >= now() - interval '24 hours'
                            )::bigint AS agent_lost_last_24h,
                            COUNT(*) FILTER (
                                WHERE status = 'canceled'
                                  AND COALESCE(completed_at, cancel_acked_at, cancel_sent_at, cancel_requested_at, started_at) >= now() - interval '24 hours'
                            )::bigint AS canceled_last_24h,
                            COALESCE(SUM(dispatch_attempts), 0)::bigint AS total_dispatch_attempts,
                            COUNT(*) FILTER (WHERE dispatch_attempts > 1)::bigint AS retried_targets,
                            COUNT(*) FILTER (WHERE cancel_requested_at IS NOT NULL)::bigint AS cancel_requested,
                            COUNT(*) FILTER (WHERE cancel_sent_at IS NOT NULL)::bigint AS cancel_sent,
                            COUNT(*) FILTER (WHERE cancel_acked_at IS NOT NULL)::bigint AS cancel_acked,
                            COUNT(*) FILTER (
                                WHERE cancel_sent_at IS NOT NULL
                                  AND cancel_acked_at IS NULL
                                  AND completed_at IS NULL
                            )::bigint AS cancel_awaiting_ack
                        FROM job_targets
                    ) target_metrics
                    CROSS JOIN (
                        SELECT
                            COUNT(*) FILTER (WHERE completed_at IS NULL)::bigint AS active_jobs,
                            COUNT(*) FILTER (WHERE completed_at IS NULL AND status = 'queued')::bigint AS queued_jobs,
                            COUNT(*) FILTER (WHERE completed_at IS NULL AND status = 'running')::bigint AS running_jobs
                        FROM jobs
                    ) job_metrics
                    "#,
                )
                .fetch_one(pool)
                .await?;
                let open_connections = pool.size();
                let idle_connections = pool.num_idle() as u32;
                let in_use_connections = open_connections.saturating_sub(idle_connections);
                Ok(SystemDashboardRepositorySnapshot {
                    db_pool: SystemDashboardDbPoolView {
                        max_connections: pool.options().get_max_connections(),
                        open_connections,
                        idle_connections,
                        in_use_connections,
                    },
                    dispatch: SystemDashboardDispatchView {
                        active_jobs: row.try_get("active_jobs")?,
                        queued_jobs: row.try_get("queued_jobs")?,
                        running_jobs: row.try_get("running_jobs")?,
                        queue_depth: row.try_get::<i64, _>("queued")?
                            + row.try_get::<i64, _>("dispatching")?,
                        total_dispatch_attempts: row.try_get("total_dispatch_attempts")?,
                        retried_targets: row.try_get("retried_targets")?,
                    },
                    targets: SystemDashboardTargetsView {
                        queued: row.try_get("queued")?,
                        dispatching: row.try_get("dispatching")?,
                        running: row.try_get("running")?,
                        active: row.try_get("active")?,
                        deadline_expired_active: row.try_get("deadline_expired_active")?,
                        control_timeout_last_24h: row.try_get("control_timeout_last_24h")?,
                        agent_timeout_last_24h: row.try_get("agent_timeout_last_24h")?,
                        agent_lost_last_24h: row.try_get("agent_lost_last_24h")?,
                        canceled_last_24h: row.try_get("canceled_last_24h")?,
                    },
                    cancellations: SystemDashboardCancellationsView {
                        requested: row.try_get("cancel_requested")?,
                        sent: row.try_get("cancel_sent")?,
                        acked: row.try_get("cancel_acked")?,
                        awaiting_ack: row.try_get("cancel_awaiting_ack")?,
                    },
                })
            }
        }
    }

    pub(crate) async fn record_system_metric_samples(
        &self,
        observed_unix: u64,
        samples: &[SystemMetricSample],
    ) -> Result<()> {
        let bucket_start = observed_unix - (observed_unix % SYSTEM_METRIC_BUCKET_SECS as u64);
        match self {
            Self::Memory(memory) => {
                let bucket_start_text = chrono::DateTime::from_timestamp(bucket_start as i64, 0)
                    .unwrap_or_else(chrono::Utc::now)
                    .to_rfc3339();
                let mut rows = memory.system_metric_rollups.write().await;
                for sample in samples {
                    if let Some(existing) = rows.iter_mut().find(|row| {
                        row.metric == sample.metric && row.bucket_start == bucket_start_text
                    }) {
                        let next_count = existing.sample_count.saturating_add(1);
                        existing.avg_value = ((existing.avg_value * existing.sample_count as f64)
                            + sample.value)
                            / next_count as f64;
                        existing.max_value = existing.max_value.max(sample.value);
                        existing.latest_value = sample.value;
                        existing.sample_count = next_count;
                    } else {
                        rows.push(SystemMetricRollupView {
                            metric: sample.metric.to_string(),
                            bucket_start: bucket_start_text.clone(),
                            bucket_secs: SYSTEM_METRIC_BUCKET_SECS,
                            sample_count: 1,
                            avg_value: sample.value,
                            max_value: sample.value,
                            latest_value: sample.value,
                        });
                    }
                }
                Ok(())
            }
            Self::Postgres(pool) => {
                if samples.is_empty() {
                    return Ok(());
                }
                let metrics = samples
                    .iter()
                    .map(|sample| sample.metric.to_string())
                    .collect::<Vec<_>>();
                let values = samples
                    .iter()
                    .map(|sample| sample.value)
                    .collect::<Vec<_>>();
                sqlx::query(
                    r#"
                    WITH input AS (
                        SELECT sample.metric, sample.value, sample.ordinality
                        FROM unnest($1::text[], $2::double precision[])
                            WITH ORDINALITY AS sample(metric, value, ordinality)
                    ),
                    aggregated AS (
                        SELECT
                            metric,
                            count(*)::integer AS sample_count,
                            sum(value)::double precision AS value_sum,
                            avg(value)::double precision AS avg_value,
                            max(value)::double precision AS max_value,
                            (array_agg(value ORDER BY ordinality DESC))[1]
                                AS latest_value
                        FROM input
                        GROUP BY metric
                    )
                    INSERT INTO system_metric_rollups (
                        metric,
                        bucket_start,
                        bucket_secs,
                        sample_count,
                        value_sum,
                        avg_value,
                        max_value,
                        latest_value,
                        latest_observed_at,
                        updated_at
                    )
                    SELECT
                        metric,
                        to_timestamp($3::double precision),
                        $4,
                        sample_count,
                        value_sum,
                        avg_value,
                        max_value,
                        latest_value,
                        to_timestamp($5::double precision),
                        now()
                    FROM aggregated
                    ON CONFLICT (metric, bucket_secs, bucket_start) DO UPDATE SET
                        sample_count = system_metric_rollups.sample_count
                            + EXCLUDED.sample_count,
                        value_sum = system_metric_rollups.value_sum + EXCLUDED.value_sum,
                        avg_value = (
                            system_metric_rollups.value_sum + EXCLUDED.value_sum
                        ) / (
                            system_metric_rollups.sample_count + EXCLUDED.sample_count
                        )::double precision,
                        max_value = GREATEST(
                            system_metric_rollups.max_value,
                            EXCLUDED.max_value
                        ),
                        latest_value = EXCLUDED.latest_value,
                        latest_observed_at = GREATEST(
                            system_metric_rollups.latest_observed_at,
                            EXCLUDED.latest_observed_at
                        ),
                        updated_at = now()
                    "#,
                )
                .bind(&metrics)
                .bind(&values)
                .bind(bucket_start as f64)
                .bind(SYSTEM_METRIC_BUCKET_SECS)
                .bind(observed_unix as f64)
                .execute(pool)
                .await?;
                Ok(())
            }
        }
    }

    pub(crate) async fn list_system_metric_rollups(
        &self,
        start_unix: u64,
        end_unix: u64,
        chart_points: i64,
    ) -> Result<Vec<SystemMetricRollupView>> {
        let step_secs = system_metric_step_secs(start_unix, end_unix, chart_points);
        self.list_system_metric_rollups_at_step(
            start_unix,
            end_unix,
            chart_points,
            step_secs as u64,
        )
        .await
    }

    pub(crate) async fn list_system_metric_rollups_at_step(
        &self,
        start_unix: u64,
        end_unix: u64,
        chart_points: i64,
        step_secs: u64,
    ) -> Result<Vec<SystemMetricRollupView>> {
        let step_secs = step_secs.clamp(SYSTEM_METRIC_BUCKET_SECS as u64, i32::MAX as u64) as i64;
        match self {
            Self::Memory(memory) => {
                let physical_rows = memory
                    .system_metric_rollups
                    .read()
                    .await
                    .iter()
                    .filter_map(|row| {
                        let start = parse_system_metric_timestamp(&row.bucket_start)?;
                        (start <= end_unix
                            && start.saturating_add(row.bucket_secs.max(1) as u64) > start_unix)
                            .then_some((row.clone(), start))
                    })
                    .collect::<Vec<_>>();
                let mut groups = std::collections::BTreeMap::<
                    (u64, i32, String),
                    (i64, f64, f64, f64, u64),
                >::new();
                for (row, start) in &physical_rows {
                    if physical_rows.iter().any(|(coarser, coarser_start)| {
                        coarser.metric == row.metric
                            && coarser.bucket_secs > row.bucket_secs
                            && *coarser_start < start.saturating_add(row.bucket_secs.max(1) as u64)
                            && coarser_start.saturating_add(coarser.bucket_secs.max(1) as u64)
                                > *start
                    }) {
                        continue;
                    }
                    let chart_step = (step_secs as i32).max(row.bucket_secs);
                    let chart_start = *start / chart_step as u64 * chart_step as u64;
                    let entry = groups
                        .entry((chart_start, chart_step, row.metric.clone()))
                        .or_insert((0, 0.0, f64::NEG_INFINITY, 0.0, 0));
                    entry.0 = entry.0.saturating_add(i64::from(row.sample_count.max(0)));
                    entry.1 += row.avg_value * f64::from(row.sample_count.max(0));
                    entry.2 = entry.2.max(row.max_value);
                    if *start >= entry.4 {
                        entry.3 = row.latest_value;
                        entry.4 = *start;
                    }
                }
                let mut rows = groups
                    .into_iter()
                    .map(|((start, bucket_secs, metric), aggregate)| {
                        let sample_count = aggregate.0.clamp(1, i64::from(i32::MAX)) as i32;
                        SystemMetricRollupView {
                            metric,
                            bucket_start: chrono::DateTime::from_timestamp(start as i64, 0)
                                .unwrap_or_else(chrono::Utc::now)
                                .to_rfc3339(),
                            bucket_secs,
                            sample_count,
                            avg_value: aggregate.1 / f64::from(sample_count),
                            max_value: aggregate.2,
                            latest_value: aggregate.3,
                        }
                    })
                    .collect::<Vec<_>>();
                rows.sort_by(|left, right| {
                    left.bucket_start
                        .cmp(&right.bucket_start)
                        .then_with(|| left.metric.cmp(&right.metric))
                });
                rows.truncate((chart_points.clamp(1, 1440) * 64) as usize);
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(SYSTEM_METRIC_ROLLUP_AT_STEP_SQL)
                    .bind(start_unix as f64)
                    .bind(end_unix as f64)
                    .bind(step_secs)
                    .bind(chart_points.clamp(1, 1440) * 64)
                    .fetch_all(pool)
                    .await?;

                rows.into_iter()
                    .map(|row| {
                        Ok(SystemMetricRollupView {
                            metric: row.try_get("metric")?,
                            bucket_start: row.try_get("bucket_start")?,
                            bucket_secs: row.try_get("bucket_secs")?,
                            sample_count: row.try_get("sample_count")?,
                            avg_value: row.try_get("avg_value")?,
                            max_value: row.try_get("max_value")?,
                            latest_value: row.try_get("latest_value")?,
                        })
                    })
                    .collect()
            }
        }
    }
}

pub(crate) fn system_metric_samples_from_snapshot(
    snapshot: &SystemDashboardRepositorySnapshot,
    gateway_events: &crate::model::SystemDashboardGatewayEventsView,
) -> Vec<SystemMetricSample> {
    let mut samples = vec![
        sample(
            "db_pool.max_connections",
            snapshot.db_pool.max_connections as f64,
        ),
        sample(
            "db_pool.open_connections",
            snapshot.db_pool.open_connections as f64,
        ),
        sample(
            "db_pool.idle_connections",
            snapshot.db_pool.idle_connections as f64,
        ),
        sample(
            "db_pool.in_use_connections",
            snapshot.db_pool.in_use_connections as f64,
        ),
        sample("dispatch.active_jobs", snapshot.dispatch.active_jobs as f64),
        sample("dispatch.queued_jobs", snapshot.dispatch.queued_jobs as f64),
        sample(
            "dispatch.running_jobs",
            snapshot.dispatch.running_jobs as f64,
        ),
        sample("dispatch.queue_depth", snapshot.dispatch.queue_depth as f64),
        sample(
            "dispatch.total_dispatch_attempts",
            snapshot.dispatch.total_dispatch_attempts as f64,
        ),
        sample(
            "dispatch.retried_targets",
            snapshot.dispatch.retried_targets as f64,
        ),
        sample("targets.queued", snapshot.targets.queued as f64),
        sample("targets.dispatching", snapshot.targets.dispatching as f64),
        sample("targets.running", snapshot.targets.running as f64),
        sample("targets.active", snapshot.targets.active as f64),
        sample(
            "targets.deadline_expired_active",
            snapshot.targets.deadline_expired_active as f64,
        ),
        sample(
            "targets.control_timeout_last_24h",
            snapshot.targets.control_timeout_last_24h as f64,
        ),
        sample(
            "targets.agent_timeout_last_24h",
            snapshot.targets.agent_timeout_last_24h as f64,
        ),
        sample(
            "targets.agent_lost_last_24h",
            snapshot.targets.agent_lost_last_24h as f64,
        ),
        sample(
            "targets.canceled_last_24h",
            snapshot.targets.canceled_last_24h as f64,
        ),
        sample(
            "cancellations.requested",
            snapshot.cancellations.requested as f64,
        ),
        sample("cancellations.sent", snapshot.cancellations.sent as f64),
        sample("cancellations.acked", snapshot.cancellations.acked as f64),
        sample(
            "cancellations.awaiting_ack",
            snapshot.cancellations.awaiting_ack as f64,
        ),
    ];
    if matches!(gateway_events.status.as_str(), "live" | "unhealthy") {
        samples.extend([
            sample(
                "gateway_events.queued_events",
                gateway_events.queued_events.unwrap_or_default() as f64,
            ),
            sample(
                "gateway_events.delivered_events",
                gateway_events.delivered_events.unwrap_or_default() as f64,
            ),
            sample(
                "gateway_events.retry_attempts",
                gateway_events.retry_attempts.unwrap_or_default() as f64,
            ),
            sample(
                "gateway_events.active_queues",
                gateway_events.active_queues.unwrap_or_default() as f64,
            ),
            sample(
                "gateway_events.current_queue_depth",
                gateway_events.current_queue_depth.unwrap_or_default() as f64,
            ),
            sample(
                "gateway_events.oldest_event_age_secs",
                gateway_events.oldest_event_age_secs.unwrap_or_default() as f64,
            ),
            sample(
                "gateway_events.dropped_events",
                gateway_events.dropped_events.unwrap_or_default() as f64,
            ),
            sample(
                "gateway_events.telemetry_dropped_events",
                gateway_events.telemetry_dropped_events.unwrap_or_default() as f64,
            ),
            sample(
                "gateway_events.expired_events",
                gateway_events.expired_events.unwrap_or_default() as f64,
            ),
            sample(
                "gateway_events.critical_failures",
                gateway_events.critical_failures.unwrap_or_default() as f64,
            ),
            sample(
                "gateway_events.dropped_by_kind.telemetry",
                gateway_events.dropped_by_kind.telemetry as f64,
            ),
            sample(
                "gateway_events.dropped_by_kind.command_output",
                gateway_events.dropped_by_kind.command_output as f64,
            ),
            sample(
                "gateway_events.dropped_by_kind.lifecycle",
                gateway_events.dropped_by_kind.lifecycle as f64,
            ),
            sample(
                "gateway_events.dropped_by_kind.terminal_output",
                gateway_events.dropped_by_kind.terminal_output as f64,
            ),
            sample(
                "gateway_events.dropped_by_kind.other",
                gateway_events.dropped_by_kind.other as f64,
            ),
            sample(
                "gateway_events.dropped_by_reason.global_queue_full",
                gateway_events.dropped_by_reason.global_queue_full as f64,
            ),
            sample(
                "gateway_events.dropped_by_reason.target_queue_full",
                gateway_events.dropped_by_reason.target_queue_full as f64,
            ),
            sample(
                "gateway_events.dropped_by_reason.expired",
                gateway_events.dropped_by_reason.expired as f64,
            ),
            sample(
                "gateway_events.dropped_by_reason.coalesced",
                gateway_events.dropped_by_reason.coalesced as f64,
            ),
            sample(
                "gateway_events.critical_failures_by_reason.global_queue_full",
                gateway_events.critical_failures_by_reason.global_queue_full as f64,
            ),
            sample(
                "gateway_events.critical_failures_by_reason.target_queue_full",
                gateway_events.critical_failures_by_reason.target_queue_full as f64,
            ),
            sample(
                "gateway_events.critical_failures_by_reason.expired",
                gateway_events.critical_failures_by_reason.expired as f64,
            ),
            sample(
                "gateway_events.retained_output_truncated_events",
                gateway_events
                    .retained_output_truncated_events
                    .unwrap_or_default() as f64,
            ),
            sample(
                "gateway_events.rejected_agent_connections",
                gateway_events
                    .rejected_agent_connections
                    .unwrap_or_default() as f64,
            ),
            sample(
                "gateway_events.telemetry_admission_limit",
                gateway_events.telemetry_admission_limit.unwrap_or_default() as f64,
            ),
            sample(
                "gateway_events.telemetry_admission_active",
                gateway_events
                    .telemetry_admission_active
                    .unwrap_or_default() as f64,
            ),
            sample(
                "gateway_events.telemetry_admission_waiting",
                gateway_events
                    .telemetry_admission_waiting
                    .unwrap_or_default() as f64,
            ),
        ]);
    }
    samples
}

fn sample(metric: &'static str, value: f64) -> SystemMetricSample {
    SystemMetricSample { metric, value }
}

fn system_metric_step_secs(start_unix: u64, end_unix: u64, chart_points: i64) -> i32 {
    let span = end_unix
        .saturating_sub(start_unix)
        .max(SYSTEM_METRIC_BUCKET_SECS as u64);
    let requested = chart_points.clamp(1, 1440) as u64;
    let raw = span
        .div_ceil(requested)
        .max(SYSTEM_METRIC_BUCKET_SECS as u64);
    (raw.div_ceil(SYSTEM_METRIC_BUCKET_SECS as u64) * SYSTEM_METRIC_BUCKET_SECS as u64).min(86_400)
        as i32
}

fn parse_system_metric_timestamp(value: &str) -> Option<u64> {
    value
        .trim()
        .parse::<i64>()
        .ok()
        .and_then(|timestamp| (timestamp >= 0).then_some(timestamp as u64))
        .or_else(|| {
            chrono::DateTime::parse_from_rfc3339(value)
                .ok()
                .and_then(|timestamp| u64::try_from(timestamp.timestamp()).ok())
        })
}
