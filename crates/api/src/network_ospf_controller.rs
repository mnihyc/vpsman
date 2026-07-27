use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use tokio::time;
use tracing::{info, warn};
use uuid::Uuid;
use vpsman_common::{OspfControlMode, TunnelEndpointSide};

use crate::{
    internal_operator::system_operator,
    model::{AuthContext, NetworkOspfUpdatePlanView, TunnelPlanView},
    routes_network::{dispatch_routing_jobs, resolve_plan_routing_adapters},
    state::AppState,
};

const CONTROLLER_INTERVAL_SECS: u64 = 60;
const CONTROLLER_UPDATE_PLAN_LIMIT: usize = 1_000;
const ORPHANED_STAGE_AFTER_SECS: i64 = 120;
const FAILED_STATUS_RETRY_AFTER_SECS: i64 = 300;
const VERIFIED_STATUS_REFRESH_AFTER_SECS: i64 = 600;

pub(crate) fn spawn_automatic_ospf_controller(state: AppState) {
    tokio::spawn(async move {
        let mut ticker = time::interval(std::time::Duration::from_secs(CONTROLLER_INTERVAL_SECS));
        loop {
            ticker.tick().await;
            match run_controller_sweep(&state).await {
                Ok(dispatched) if dispatched > 0 => {
                    info!(
                        dispatched,
                        "automatic OSPF controller dispatched endpoint jobs"
                    );
                }
                Ok(_) => {}
                Err(error) => warn!(%error, "automatic OSPF controller sweep failed"),
            }
        }
    });
}

pub(crate) async fn run_controller_sweep(state: &AppState) -> Result<usize> {
    reconcile_orphaned_pending_jobs(state).await?;
    let scanned_plan_ids = state
        .repo
        .list_automatic_tunnel_plan_ids_for_controller(CONTROLLER_UPDATE_PLAN_LIMIT)
        .await?;
    let update_batch = state
        .repo
        .list_automatic_network_ospf_update_plan_batch(&scanned_plan_ids)
        .await?;
    let operator = system_operator("network-ospf-controller");
    let mut dispatched = 0;

    for failure in update_batch.failures {
        let _failed = isolate_controller_plan_result::<()>(
            failure.plan_id,
            failure.phase,
            Err(failure.error),
        );
    }
    for update in update_batch.updates {
        let plan_id = update.plan_id;
        if let Some(plan_dispatched) = isolate_controller_plan_result(
            plan_id,
            "automatic_update",
            process_automatic_ospf_update(state, &operator, update).await,
        ) {
            dispatched += plan_dispatched;
        }
    }
    state
        .repo
        .mark_automatic_tunnel_plans_scanned(&scanned_plan_ids)
        .await?;
    Ok(dispatched)
}

async fn reconcile_orphaned_pending_jobs(state: &AppState) -> Result<()> {
    let now = Utc::now();
    let reconciled_plan_ids = state
        .repo
        .list_pending_tunnel_plan_ids_for_reconciliation(CONTROLLER_UPDATE_PLAN_LIMIT)
        .await?;
    let attempts = state
        .repo
        .tunnel_plan_record_attempts(&reconciled_plan_ids)
        .await?;
    for attempt in attempts {
        match attempt.plan {
            Ok(plan) => {
                let _reconciled = isolate_controller_plan_result(
                    attempt.plan_id,
                    "pending_job_reconciliation",
                    reconcile_orphaned_pending_plan(state, now, plan).await,
                );
            }
            Err(error) => {
                let _failed = isolate_controller_plan_result::<()>(
                    attempt.plan_id,
                    "pending_plan_decode",
                    Err(error),
                );
            }
        }
    }
    state
        .repo
        .mark_pending_tunnel_plans_reconciled(&reconciled_plan_ids)
        .await?;
    Ok(())
}

fn isolate_controller_plan_result<T>(
    plan_id: Uuid,
    phase: &'static str,
    result: Result<T>,
) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            warn!(
                ?error,
                %plan_id,
                phase,
                "automatic OSPF controller plan attempt failed; continuing fair sweep"
            );
            None
        }
    }
}

async fn process_automatic_ospf_update(
    state: &AppState,
    operator: &AuthContext,
    update: NetworkOspfUpdatePlanView,
) -> Result<usize> {
    let Some(plan) = state.repo.get_tunnel_plan_record(update.plan_id).await? else {
        return Ok(0);
    };
    if plan.revision != update.plan_revision {
        return Ok(0);
    }
    let Some(ospf) = plan.plan.ospf.as_ref() else {
        return Ok(0);
    };
    if ospf.mode != OspfControlMode::Automatic || !plan.enabled {
        return Ok(0);
    }
    let should_refresh_status = automatic_status_refresh_due(
        &plan.left_ospf_status,
        &plan.right_ospf_status,
        &plan.updated_at,
        Utc::now(),
    );
    let should_apply = update.status == "automatic_ready" && !should_refresh_status;
    if !should_refresh_status && !should_apply {
        return Ok(0);
    }

    let (left_adapter, right_adapter) = match resolve_plan_routing_adapters(state, &plan).await {
        Ok(adapters) => adapters,
        Err(error) => {
            warn!(
                ?error,
                plan_id = %plan.id,
                "automatic OSPF adapter binding unavailable"
            );
            return Ok(0);
        }
    };
    let left_job_id = Uuid::new_v4();
    let right_job_id = Uuid::new_v4();
    let desired_cost = should_apply
        .then(|| u16::try_from(update.recommended_ospf_cost).ok())
        .flatten();
    let staged = match state
        .repo
        .stage_tunnel_plan_ospf_jobs(
            plan.id,
            update.plan_revision,
            plan.left_current_ospf_cost
                .and_then(|value| u16::try_from(value).ok()),
            plan.right_current_ospf_cost
                .and_then(|value| u16::try_from(value).ok()),
            desired_cost,
            left_job_id,
            right_job_id,
            operator,
        )
        .await
    {
        Ok(staged) => staged,
        Err(error)
            if error.to_string().contains("snapshot_stale")
                || error.to_string().contains("job_in_progress") =>
        {
            return Ok(0);
        }
        Err(error) => return Err(error),
    };
    let apply = desired_cost.map(|desired| {
        (
            plan.left_current_ospf_cost
                .and_then(|value| u16::try_from(value).ok()),
            plan.right_current_ospf_cost
                .and_then(|value| u16::try_from(value).ok()),
            desired,
        )
    });
    let (jobs, dispatch) = dispatch_routing_jobs(
        state,
        operator,
        &staged,
        left_job_id,
        right_job_id,
        left_adapter,
        right_adapter,
        apply,
    )
    .await;
    for outcome in dispatch
        .into_iter()
        .filter(|outcome| outcome.status != "queued")
    {
        warn!(
            plan_id = %plan.id,
            client_id = outcome.client_id,
            endpoint_side = ?outcome.endpoint_side,
            error = outcome.error.as_deref().unwrap_or("job_not_queued"),
            "automatic OSPF endpoint dispatch was not queued"
        );
    }
    Ok(jobs.len())
}

async fn reconcile_orphaned_pending_plan(
    state: &AppState,
    now: DateTime<Utc>,
    plan: TunnelPlanView,
) -> Result<()> {
    let Some(updated_at) = parse_timestamp(&plan.updated_at) else {
        return Ok(());
    };
    if updated_at > now - Duration::seconds(ORPHANED_STAGE_AFTER_SECS) {
        return Ok(());
    }
    for (side, status, job_id) in [
        (
            TunnelEndpointSide::Left,
            plan.left_ospf_status.as_str(),
            plan.left_ospf_job_id,
        ),
        (
            TunnelEndpointSide::Right,
            plan.right_ospf_status.as_str(),
            plan.right_ospf_job_id,
        ),
    ] {
        if status != "pending" {
            continue;
        }
        let terminal_or_missing = match job_id {
            Some(job_id) => state.repo.get_job(job_id).await?.is_none_or(|job| {
                matches!(
                    job.status.as_str(),
                    "completed" | "failed" | "canceled" | "rejected" | "skipped"
                )
            }),
            None => true,
        };
        if terminal_or_missing {
            if let Some(job_id) = job_id {
                state
                    .repo
                    .record_tunnel_plan_ospf_job_result(plan.id, side, job_id, None, false)
                    .await?;
            }
        }
    }
    Ok(())
}

fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    if let Ok(unix) = value.parse::<i64>() {
        return DateTime::from_timestamp(unix, 0);
    }
    DateTime::parse_from_rfc3339(value)
        .or_else(|_| DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f%#z"))
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn automatic_status_refresh_due(
    left_status: &str,
    right_status: &str,
    updated_at: &str,
    now: DateTime<Utc>,
) -> bool {
    if left_status == "pending" || right_status == "pending" {
        return false;
    }
    if matches!(left_status, "unverified" | "stale")
        || matches!(right_status, "unverified" | "stale")
    {
        return true;
    }
    let Some(updated_at) = parse_timestamp(updated_at) else {
        return true;
    };
    let age = now.signed_duration_since(updated_at).num_seconds();
    if left_status == "failed" || right_status == "failed" {
        return age >= FAILED_STATUS_RETRY_AFTER_SECS;
    }
    left_status == "verified"
        && right_status == "verified"
        && age >= VERIFIED_STATUS_REFRESH_AFTER_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_plan_error_is_isolated_from_following_plan() {
        let failed_plan_id = Uuid::new_v4();
        let healthy_plan_id = Uuid::new_v4();

        let failed = isolate_controller_plan_result::<usize>(
            failed_plan_id,
            "automatic_update",
            Err(anyhow::anyhow!("poison plan")),
        );
        let healthy =
            isolate_controller_plan_result(healthy_plan_id, "automatic_update", Ok(2_usize));

        assert_eq!(failed, None);
        assert_eq!(healthy, Some(2));
    }

    #[test]
    fn automatic_status_refreshes_initial_stale_failed_and_periodic_verified_states() {
        let now = Utc::now();
        let recent = (now - Duration::seconds(60)).to_rfc3339();
        let retry_due = (now - Duration::seconds(FAILED_STATUS_RETRY_AFTER_SECS)).to_rfc3339();
        let refresh_due =
            (now - Duration::seconds(VERIFIED_STATUS_REFRESH_AFTER_SECS)).to_rfc3339();

        assert!(automatic_status_refresh_due(
            "unverified",
            "verified",
            &recent,
            now
        ));
        assert!(!automatic_status_refresh_due(
            "failed", "verified", &recent, now
        ));
        assert!(automatic_status_refresh_due(
            "failed", "verified", &retry_due, now
        ));
        assert!(!automatic_status_refresh_due(
            "verified", "verified", &recent, now
        ));
        assert!(automatic_status_refresh_due(
            "verified",
            "verified",
            &refresh_due,
            now
        ));
        assert!(!automatic_status_refresh_due(
            "pending",
            "verified",
            &refresh_due,
            now
        ));
    }
}
