use anyhow::Result;
use sqlx::Row;

use crate::{
    model::{
        DashboardServerCancellationsView, DashboardServerDbPoolView, DashboardServerDispatchView,
        DashboardServerTargetsView,
    },
    repository::Repository,
};

#[derive(Clone, Debug)]
pub(crate) struct DashboardServerRepositorySnapshot {
    pub(crate) db_pool: DashboardServerDbPoolView,
    pub(crate) dispatch: DashboardServerDispatchView,
    pub(crate) targets: DashboardServerTargetsView,
    pub(crate) cancellations: DashboardServerCancellationsView,
}

impl Repository {
    pub(crate) async fn dashboard_server_snapshot(
        &self,
    ) -> Result<DashboardServerRepositorySnapshot> {
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
                Ok(DashboardServerRepositorySnapshot {
                    db_pool: DashboardServerDbPoolView {
                        max_connections: 0,
                        open_connections: 0,
                        idle_connections: 0,
                        in_use_connections: 0,
                    },
                    dispatch: DashboardServerDispatchView {
                        active_jobs: pending_jobs + running_jobs,
                        pending_jobs,
                        running_jobs,
                        queue_depth: pending + delivering,
                        total_dispatch_attempts: 0,
                        retried_targets: 0,
                    },
                    targets: DashboardServerTargetsView {
                        pending,
                        delivering,
                        running,
                        active: delivering + running,
                        deadline_expired_active: 0,
                        control_timed_out_last_24h: control_timed_out,
                        agent_timed_out_last_24h: agent_timed_out,
                        canceled_last_24h: canceled,
                    },
                    cancellations: DashboardServerCancellationsView::default(),
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
                Ok(DashboardServerRepositorySnapshot {
                    db_pool: DashboardServerDbPoolView {
                        max_connections: pool.options().get_max_connections(),
                        open_connections,
                        idle_connections,
                        in_use_connections,
                    },
                    dispatch: DashboardServerDispatchView {
                        active_jobs: job_row.try_get("active_jobs")?,
                        pending_jobs: job_row.try_get("pending_jobs")?,
                        running_jobs: job_row.try_get("running_jobs")?,
                        queue_depth: target_row.try_get::<i64, _>("pending")?
                            + target_row.try_get::<i64, _>("delivering")?,
                        total_dispatch_attempts: target_row.try_get("total_dispatch_attempts")?,
                        retried_targets: target_row.try_get("retried_targets")?,
                    },
                    targets: DashboardServerTargetsView {
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
                    cancellations: DashboardServerCancellationsView {
                        requested: target_row.try_get("cancel_requested")?,
                        sent: target_row.try_get("cancel_sent")?,
                        acked: target_row.try_get("cancel_acked")?,
                        awaiting_ack: target_row.try_get("cancel_awaiting_ack")?,
                    },
                })
            }
        }
    }
}
