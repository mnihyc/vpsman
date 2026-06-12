use anyhow::Result;
use sqlx::Row;

use crate::{
    model::{
        SystemDashboardCancellationsView, SystemDashboardDbPoolView, SystemDashboardDispatchView,
        SystemDashboardTargetsView, SystemMetricRollupView,
    },
    repository::Repository,
};

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
                let pending_jobs = jobs
                    .iter()
                    .filter(|job| job.completed_at.is_none() && job.status == "pending")
                    .count() as i64;
                let running_jobs = jobs
                    .iter()
                    .filter(|job| job.completed_at.is_none() && job.status == "running")
                    .count() as i64;
                let pending = targets
                    .iter()
                    .filter(|target| target.completed_at.is_none() && target.status == "pending")
                    .count() as i64;
                let delivering = targets
                    .iter()
                    .filter(|target| target.completed_at.is_none() && target.status == "delivering")
                    .count() as i64;
                let running = targets
                    .iter()
                    .filter(|target| target.completed_at.is_none() && target.status == "running")
                    .count() as i64;
                let control_timed_out = targets
                    .iter()
                    .filter(|target| target.status == "control_timed_out")
                    .count() as i64;
                let agent_timed_out = targets
                    .iter()
                    .filter(|target| target.status == "agent_timed_out")
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
                        active_jobs: pending_jobs + running_jobs,
                        pending_jobs,
                        running_jobs,
                        queue_depth: pending + delivering,
                        total_dispatch_attempts: 0,
                        retried_targets: 0,
                    },
                    targets: SystemDashboardTargetsView {
                        pending,
                        delivering,
                        running,
                        active: delivering + running,
                        deadline_expired_active: 0,
                        control_timed_out_last_24h: control_timed_out,
                        agent_timed_out_last_24h: agent_timed_out,
                        canceled_last_24h: canceled,
                    },
                    cancellations: SystemDashboardCancellationsView::default(),
                })
            }
            Self::Postgres(pool) => {
                let target_row = sqlx::query(
                    r#"
                    SELECT
                        COUNT(*) FILTER (WHERE completed_at IS NULL AND status = 'pending')::bigint AS pending,
                        COUNT(*) FILTER (WHERE completed_at IS NULL AND status = 'delivering')::bigint AS delivering,
                        COUNT(*) FILTER (WHERE completed_at IS NULL AND status = 'running')::bigint AS running,
                        COUNT(*) FILTER (WHERE completed_at IS NULL AND status IN ('delivering', 'running'))::bigint AS active,
                        COUNT(*) FILTER (
                            WHERE completed_at IS NULL
                              AND status IN ('delivering', 'running')
                              AND deadline_at IS NOT NULL
                              AND deadline_at <= now()
                        )::bigint AS deadline_expired_active,
                        COUNT(*) FILTER (
                            WHERE status = 'control_timed_out'
                              AND COALESCE(completed_at, result_received_at, started_at) >= now() - interval '24 hours'
                        )::bigint AS control_timed_out_last_24h,
                        COUNT(*) FILTER (
                            WHERE status = 'agent_timed_out'
                              AND COALESCE(completed_at, result_received_at, started_at) >= now() - interval '24 hours'
                        )::bigint AS agent_timed_out_last_24h,
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
                    "#,
                )
                .fetch_one(pool)
                .await?;
                let job_row = sqlx::query(
                    r#"
                    SELECT
                        COUNT(*) FILTER (WHERE completed_at IS NULL)::bigint AS active_jobs,
                        COUNT(*) FILTER (WHERE completed_at IS NULL AND status = 'pending')::bigint AS pending_jobs,
                        COUNT(*) FILTER (WHERE completed_at IS NULL AND status = 'running')::bigint AS running_jobs
                    FROM jobs
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
                        active_jobs: job_row.try_get("active_jobs")?,
                        pending_jobs: job_row.try_get("pending_jobs")?,
                        running_jobs: job_row.try_get("running_jobs")?,
                        queue_depth: target_row.try_get::<i64, _>("pending")?
                            + target_row.try_get::<i64, _>("delivering")?,
                        total_dispatch_attempts: target_row.try_get("total_dispatch_attempts")?,
                        retried_targets: target_row.try_get("retried_targets")?,
                    },
                    targets: SystemDashboardTargetsView {
                        pending: target_row.try_get("pending")?,
                        delivering: target_row.try_get("delivering")?,
                        running: target_row.try_get("running")?,
                        active: target_row.try_get("active")?,
                        deadline_expired_active: target_row.try_get("deadline_expired_active")?,
                        control_timed_out_last_24h: target_row
                            .try_get("control_timed_out_last_24h")?,
                        agent_timed_out_last_24h: target_row.try_get("agent_timed_out_last_24h")?,
                        canceled_last_24h: target_row.try_get("canceled_last_24h")?,
                    },
                    cancellations: SystemDashboardCancellationsView {
                        requested: target_row.try_get("cancel_requested")?,
                        sent: target_row.try_get("cancel_sent")?,
                        acked: target_row.try_get("cancel_acked")?,
                        awaiting_ack: target_row.try_get("cancel_awaiting_ack")?,
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
                for sample in samples {
                    sqlx::query(
                        r#"
                        INSERT INTO system_metric_rollups (
                            metric,
                            bucket_start,
                            bucket_secs,
                            sample_count,
                            avg_value,
                            max_value,
                            latest_value,
                            latest_observed_at,
                            updated_at
                        )
                        VALUES (
                            $1,
                            to_timestamp($2::double precision),
                            $3,
                            1,
                            $4,
                            $4,
                            $4,
                            to_timestamp($5::double precision),
                            now()
                        )
                        ON CONFLICT (metric, bucket_secs, bucket_start) DO UPDATE SET
                            sample_count = system_metric_rollups.sample_count + 1,
                            avg_value = (
                                system_metric_rollups.avg_value
                                    * system_metric_rollups.sample_count::double precision
                                + EXCLUDED.latest_value
                            ) / (system_metric_rollups.sample_count + 1)::double precision,
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
                    .bind(sample.metric)
                    .bind(bucket_start as f64)
                    .bind(SYSTEM_METRIC_BUCKET_SECS)
                    .bind(sample.value)
                    .bind(observed_unix as f64)
                    .execute(pool)
                    .await?;
                }
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
        match self {
            Self::Memory(memory) => {
                let mut rows = memory.system_metric_rollups.read().await.clone();
                rows.retain(|row| timestamp_in_bounds(&row.bucket_start, start_unix, end_unix));
                rows.sort_by(|left, right| {
                    left.bucket_start
                        .cmp(&right.bucket_start)
                        .then_with(|| left.metric.cmp(&right.metric))
                });
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH selected AS (
                        SELECT
                            metric,
                            to_timestamp(
                                floor(
                                    extract(epoch FROM bucket_start) / $3::double precision
                                ) * $3::double precision
                            ) AS chart_bucket_start,
                            sample_count,
                            avg_value,
                            max_value,
                            latest_value
                        FROM system_metric_rollups
                        WHERE
                            bucket_secs = $4
                            AND bucket_start >= to_timestamp($1::double precision)
                            AND bucket_start <= to_timestamp($2::double precision)
                    )
                    SELECT
                        metric,
                        chart_bucket_start::text AS bucket_start,
                        LEAST(sum(sample_count)::bigint, 2147483647)::integer AS sample_count,
                        COALESCE(
                            sum(avg_value * sample_count::double precision)
                                / NULLIF(sum(sample_count)::double precision, 0),
                            0
                        ) AS avg_value,
                        max(max_value)::double precision AS max_value,
                        (array_agg(latest_value ORDER BY chart_bucket_start DESC))[1] AS latest_value
                    FROM selected
                    GROUP BY metric, chart_bucket_start
                    ORDER BY chart_bucket_start ASC, metric ASC
                    LIMIT $5
                    "#,
                )
                .bind(start_unix as f64)
                .bind(end_unix as f64)
                .bind(step_secs)
                .bind(SYSTEM_METRIC_BUCKET_SECS)
                .bind(chart_points.clamp(1, 1440) * 64)
                .fetch_all(pool)
                .await?;

                rows.into_iter()
                    .map(|row| {
                        Ok(SystemMetricRollupView {
                            metric: row.try_get("metric")?,
                            bucket_start: row.try_get("bucket_start")?,
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
        sample(
            "dispatch.pending_jobs",
            snapshot.dispatch.pending_jobs as f64,
        ),
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
        sample("targets.pending", snapshot.targets.pending as f64),
        sample("targets.delivering", snapshot.targets.delivering as f64),
        sample("targets.running", snapshot.targets.running as f64),
        sample("targets.active", snapshot.targets.active as f64),
        sample(
            "targets.deadline_expired_active",
            snapshot.targets.deadline_expired_active as f64,
        ),
        sample(
            "targets.control_timed_out_last_24h",
            snapshot.targets.control_timed_out_last_24h as f64,
        ),
        sample(
            "targets.agent_timed_out_last_24h",
            snapshot.targets.agent_timed_out_last_24h as f64,
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
    if gateway_events.status == "live" {
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

fn timestamp_in_bounds(value: &str, start_unix: u64, end_unix: u64) -> bool {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|timestamp| {
            let unix = timestamp.timestamp().max(0) as u64;
            unix >= start_unix && unix <= end_unix
        })
        .unwrap_or(true)
}
