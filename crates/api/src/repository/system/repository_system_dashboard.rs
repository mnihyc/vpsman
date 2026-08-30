use anyhow::{anyhow, Result};
use sqlx::Row;
use vpsman_common::TELEMETRY_HISTORY_TIERS;

use crate::{
    model::{
        SystemDashboardCancellationsView, SystemDashboardDbPoolView, SystemDashboardDispatchView,
        SystemDashboardTargetsView, SystemMetricRollupView,
    },
    repository::Repository,
};

pub(crate) const SYSTEM_DASHBOARD_SNAPSHOT_SQL: &str = r#"
WITH recent_terminal AS (
    SELECT target.status, count(*)::bigint AS target_count
    FROM job_targets target
    WHERE target.status IN (
            'control_timeout',
            'agent_timeout',
            'agent_lost',
            'canceled'
        )
      AND public.job_target_effective_terminal_at(
            target.status,
            target.completed_at,
            target.result_received_at,
            target.started_at,
            target.cancel_acked_at,
            target.cancel_sent_at,
            target.cancel_requested_at
          ) >= now() - interval '24 hours'
    GROUP BY target.status
),
recent_metrics AS (
    SELECT
        COALESCE(max(target_count) FILTER (
            WHERE status = 'control_timeout'
        ), 0)::bigint AS control_timeout_last_24h,
        COALESCE(max(target_count) FILTER (
            WHERE status = 'agent_timeout'
        ), 0)::bigint AS agent_timeout_last_24h,
        COALESCE(max(target_count) FILTER (
            WHERE status = 'agent_lost'
        ), 0)::bigint AS agent_lost_last_24h,
        COALESCE(max(target_count) FILTER (
            WHERE status = 'canceled'
        ), 0)::bigint AS canceled_last_24h
    FROM recent_terminal
),
target_metrics AS (
    SELECT
        COALESCE(sum(target_queued), 0)::bigint AS target_queued,
        COALESCE(sum(target_dispatching), 0)::bigint AS target_dispatching,
        COALESCE(sum(target_running), 0)::bigint AS target_running,
        COALESCE(sum(total_dispatch_attempts), 0)::bigint
            AS total_dispatch_attempts,
        COALESCE(sum(retried_targets), 0)::bigint AS retried_targets,
        COALESCE(sum(cancel_requested), 0)::bigint AS cancel_requested,
        COALESCE(sum(cancel_sent), 0)::bigint AS cancel_sent,
        COALESCE(sum(cancel_acked), 0)::bigint AS cancel_acked,
        COALESCE(sum(cancel_awaiting_ack), 0)::bigint AS cancel_awaiting_ack
    FROM system_dashboard_target_metrics
),
job_metrics AS (
    SELECT
        count(*)::bigint AS active_jobs,
        count(*) FILTER (WHERE status = 'queued')::bigint AS queued_jobs,
        count(*) FILTER (WHERE status = 'running')::bigint AS running_jobs
    FROM jobs
    WHERE completed_at IS NULL
)
SELECT
    metrics.target_queued AS queued,
    metrics.target_dispatching AS dispatching,
    metrics.target_running AS running,
    (
        metrics.target_dispatching
        + metrics.target_running
    )::bigint AS active,
    (
        SELECT count(*)::bigint
        FROM job_targets target
        WHERE target.completed_at IS NULL
          AND target.status IN ('dispatching', 'running')
          AND target.deadline_at IS NOT NULL
          AND target.deadline_at <= now()
    ) AS deadline_expired_active,
    recent_metrics.control_timeout_last_24h,
    recent_metrics.agent_timeout_last_24h,
    recent_metrics.agent_lost_last_24h,
    recent_metrics.canceled_last_24h,
    metrics.total_dispatch_attempts,
    metrics.retried_targets,
    metrics.cancel_requested,
    metrics.cancel_sent,
    metrics.cancel_acked,
    metrics.cancel_awaiting_ack,
    jobs.active_jobs,
    jobs.queued_jobs,
    jobs.running_jobs
FROM target_metrics metrics
CROSS JOIN job_metrics jobs
CROSS JOIN recent_metrics
"#;

pub(crate) const SYSTEM_METRIC_ROLLUP_AT_STEP_SQL: &str = r#"
SELECT
    row.metric,
    floor(
        extract(epoch FROM row.bucket_start)::double precision
            / $3::double precision
    )::bigint * $3::bigint AS chart_bucket_unix,
    LEAST(sum(row.sample_count), 2147483647)::integer AS sample_count,
    sum(row.value_sum)::double precision
        / sum(row.sample_count)::double precision AS avg_value,
    max(row.max_value)::double precision AS max_value,
    (max(ARRAY[
        extract(epoch FROM row.latest_observed_at)::double precision,
        row.latest_value
    ]))[2] AS latest_value
FROM system_metric_rollups row
WHERE row.bucket_secs = ANY($4::integer[])
  AND row.bucket_start >= to_timestamp($1::double precision)
        - make_interval(secs => $5::double precision)
  AND row.bucket_start <= to_timestamp($2::double precision)
  AND row.bucket_start + make_interval(secs => row.bucket_secs)
        > to_timestamp($1::double precision)
GROUP BY row.metric, chart_bucket_unix
"#;

pub(crate) const SYSTEM_METRIC_EXPORT_SQL: &str = r#"
SELECT
    metric,
    extract(epoch FROM bucket_start)::bigint AS bucket_start_unix,
    bucket_secs,
    sample_count,
    avg_value,
    max_value,
    latest_value
FROM system_metric_rollups
ORDER BY bucket_start DESC, metric ASC, bucket_secs ASC
LIMIT $1
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
    pub(crate) async fn earliest_system_metric_bucket_unix(&self) -> Result<Option<u64>> {
        match self {
            Self::Postgres(pool) => {
                let earliest: Option<i64> = sqlx::query_scalar(
                    r#"
                    SELECT extract(epoch FROM min(bucket_start))::bigint
                    FROM system_metric_rollups
                    "#,
                )
                .fetch_one(pool)
                .await?;
                Ok(earliest.map(|value| value.max(0) as u64))
            }
        }
    }

    pub(crate) async fn system_dashboard_snapshot(
        &self,
    ) -> Result<SystemDashboardRepositorySnapshot> {
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(SYSTEM_DASHBOARD_SNAPSHOT_SQL)
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
        match self {
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
                        aggregate.metric,
                        to_timestamp(
                            floor(
                                $3::double precision
                                    / $4::double precision
                            ) * $4::double precision
                        ),
                        $4,
                        aggregate.sample_count,
                        aggregate.value_sum,
                        aggregate.avg_value,
                        aggregate.max_value,
                        aggregate.latest_value,
                        to_timestamp($3::double precision),
                        now()
                    FROM aggregated aggregate
                    -- The supported control plane has one API sampler, and its
                    -- cadence and source tier are both 60 seconds. A bucket row
                    -- is therefore its natural ownership token: startup near a
                    -- minute boundary or an in-minute restart may repeat the
                    -- observation, but only the first statement contributes a
                    -- value for each metric. Multi-API aggregation would need
                    -- explicit source identities and metric-specific reducers.
                    ON CONFLICT (bucket_secs, bucket_start, metric) DO NOTHING
                    "#,
                )
                .bind(&metrics)
                .bind(&values)
                .bind(observed_unix as f64)
                .bind(SYSTEM_METRIC_BUCKET_SECS)
                .execute(pool)
                .await?;
                Ok(())
            }
        }
    }

    pub(crate) async fn list_system_metric_rollups_at_step(
        &self,
        start_unix: u64,
        end_unix: u64,
        step_secs: u64,
    ) -> Result<Vec<SystemMetricRollupView>> {
        let step_secs = step_secs.clamp(SYSTEM_METRIC_BUCKET_SECS as u64, i32::MAX as u64) as i64;
        let read_tiers =
            system_metric_read_tiers(end_unix.saturating_sub(start_unix), step_secs as u64)?;
        let max_read_tier = *read_tiers.last().expect("system metric read tiers");
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(SYSTEM_METRIC_ROLLUP_AT_STEP_SQL)
                    .bind(start_unix as f64)
                    .bind(end_unix as f64)
                    .bind(step_secs)
                    .bind(&read_tiers)
                    .bind(max_read_tier)
                    .fetch_all(pool)
                    .await?;
                let mut views = rows
                    .into_iter()
                    .map(|row| -> Result<_> {
                        let chart_bucket_unix: i64 = row.try_get("chart_bucket_unix")?;
                        let metric: String = row.try_get("metric")?;
                        let bucket_start = chrono::DateTime::from_timestamp(chart_bucket_unix, 0)
                            .ok_or_else(|| anyhow!("system metric chart bucket is out of range"))?
                            .to_rfc3339();
                        Ok((
                            chart_bucket_unix,
                            SystemMetricRollupView {
                                metric,
                                bucket_start,
                                bucket_secs: step_secs as i32,
                                sample_count: row.try_get("sample_count")?,
                                avg_value: row.try_get("avg_value")?,
                                max_value: row.try_get("max_value")?,
                                latest_value: row.try_get("latest_value")?,
                            },
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?;
                views.sort_unstable_by(|left, right| {
                    left.0
                        .cmp(&right.0)
                        .then_with(|| left.1.metric.cmp(&right.1.metric))
                });
                Ok(views.into_iter().map(|(_, view)| view).collect())
            }
        }
    }

    pub(crate) async fn list_system_metric_rollups_for_export(
        &self,
        limit: i64,
    ) -> Result<Vec<SystemMetricRollupView>> {
        let limit = limit.clamp(1, 1_000);
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(SYSTEM_METRIC_EXPORT_SQL)
                    .bind(limit)
                    .fetch_all(pool)
                    .await?;
                rows.into_iter()
                    .map(|row| {
                        let bucket_start_unix: i64 = row.try_get("bucket_start_unix")?;
                        let bucket_start = chrono::DateTime::from_timestamp(bucket_start_unix, 0)
                            .ok_or_else(|| anyhow!("system metric export bucket is out of range"))?
                            .to_rfc3339();
                        Ok(SystemMetricRollupView {
                            metric: row.try_get("metric")?,
                            bucket_start,
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

fn system_metric_read_tiers(span_secs: u64, step_secs: u64) -> Result<Vec<i32>> {
    let final_tier = TELEMETRY_HISTORY_TIERS
        .last()
        .expect("system history tiers");
    let selected_index = TELEMETRY_HISTORY_TIERS
        .iter()
        .enumerate()
        .rev()
        .find(|(_, tier)| {
            step_secs.is_multiple_of(tier.bucket_secs as u64)
                && (tier.bucket_secs == final_tier.bucket_secs
                    || span_secs <= (tier.retain_days as u64).saturating_mul(24 * 60 * 60))
        })
        .map(|(index, _)| index)
        .ok_or_else(|| {
            anyhow!("system metric step {step_secs} does not preserve an available retained tier")
        })?;
    Ok(TELEMETRY_HISTORY_TIERS[..=selected_index]
        .iter()
        .map(|tier| tier.bucket_secs)
        .collect())
}
