use std::{fmt::Debug, future::Future};

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};

use crate::{
    error::ApiError,
    model::ListQuery,
    model_fleet_snapshot::FleetSnapshotSource,
    model_home_snapshot::HomeSnapshotResponse,
    routes_dashboard::{
        dashboard_overview_prepared_singleflight_key, load_prepared_dashboard_overview,
        prepare_dashboard_overview, DashboardOverviewQuery, PreparedDashboardOverview,
    },
    routes_fleet_snapshot::{load_home_agents, load_home_fleet_sources, FLEET_DETAIL_LIMIT},
    routes_monitoring::monitoring_cards_for_agents_projection,
    routes_system::{load_system_dashboard, SystemDashboardQuery},
    security::{
        operator_has_scope, SCOPE_AUDIT_READ, SCOPE_BACKUPS_READ, SCOPE_FLEET_READ,
        SCOPE_JOBS_READ, SCOPE_SCHEDULES_READ, SCOPE_TERMINAL_READ,
    },
    state::AppState,
    unix_now,
};

const HISTORY_DETAIL_LIMIT: i64 = 1_000;

pub(crate) async fn home_snapshot(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(dashboard_query): Query<DashboardOverviewQuery>,
) -> Result<Json<HomeSnapshotResponse>, ApiError> {
    let auth = state.require_operator(&headers).await?;
    let fleet_read = operator_has_scope(&auth.operator.scopes, SCOPE_FLEET_READ);
    let prepared_dashboard = if fleet_read {
        Some(prepare_dashboard_overview(&dashboard_query, unix_now())?)
    } else {
        None
    };
    let key = home_snapshot_singleflight_key(
        &auth.operator,
        &dashboard_query,
        prepared_dashboard.as_ref(),
    );
    let events = state.events.clone();
    let response = events
        .singleflight_home_snapshot(key, move || async move {
            let _admission = state.events.acquire_heavy_read_permit().await?;
            let prepared_dashboard = if fleet_read {
                Some(prepare_dashboard_overview(&dashboard_query, unix_now())?)
            } else {
                None
            };
            build_home_snapshot(&state, auth.operator, prepared_dashboard).await
        })
        .await?;
    Ok(Json(response))
}

fn home_snapshot_singleflight_key(
    operator: &crate::model::OperatorView,
    dashboard_query: &DashboardOverviewQuery,
    prepared_dashboard: Option<&PreparedDashboardOverview>,
) -> String {
    let dashboard = prepared_dashboard.map_or_else(
        || serde_json::json!({ "permitted": false }),
        |prepared| dashboard_overview_prepared_singleflight_key(dashboard_query, prepared),
    );
    serde_json::json!({
        "endpoint": "home_snapshot",
        "auth": crate::state::read_singleflight_auth_key(
            operator.id,
            &operator.scopes,
        ),
        "operator": operator,
        "dashboard": dashboard,
    })
    .to_string()
}

async fn build_home_snapshot(
    state: &AppState,
    operator: crate::model::OperatorView,
    prepared_dashboard: Option<PreparedDashboardOverview>,
) -> Result<HomeSnapshotResponse, ApiError> {
    let scopes = &operator.scopes;
    let fleet_read = operator_has_scope(scopes, SCOPE_FLEET_READ);
    let jobs_read = operator_has_scope(scopes, SCOPE_JOBS_READ);
    let terminal_read = operator_has_scope(scopes, SCOPE_TERMINAL_READ);
    let backups_read = operator_has_scope(scopes, SCOPE_BACKUPS_READ);
    let audit_read = operator_has_scope(scopes, SCOPE_AUDIT_READ);
    let schedules_read = operator_has_scope(scopes, SCOPE_SCHEDULES_READ);
    let agents = load_home_agents(state, scopes).await;
    let monitoring_agents = agents.data.clone();
    let system_query = SystemDashboardQuery {
        window: Some("1d".to_string()),
        chart_points: Some(240),
    };
    let jobs_query = history_query();
    let backups_query = history_query();
    let backup_artifacts_query = history_query();
    let audit_query = history_query();
    let schedules_query = schedule_query();

    let (
        fleet,
        monitoring_cards,
        jobs,
        file_transfers,
        terminal_sessions,
        backups,
        backup_artifacts,
        audit,
        schedules,
        system_dashboard,
        dashboard_overview,
    ) = tokio::join!(
        load_home_fleet_sources(state, scopes, agents),
        load_home_monitoring_cards(state, fleet_read, monitoring_agents),
        load_source("jobs", fleet_read, load_jobs(state, &jobs_query)),
        load_source(
            "file_transfers",
            jobs_read,
            load_file_transfer_sessions(state),
        ),
        load_source(
            "terminal_sessions",
            terminal_read,
            load_terminal_sessions(state),
        ),
        load_source(
            "backups",
            backups_read,
            state.repo.query_backup_requests(&backups_query),
        ),
        load_source(
            "backup_artifacts",
            backups_read,
            state.repo.query_backup_artifacts(&backup_artifacts_query),
        ),
        load_source(
            "audit",
            audit_read,
            state.repo.query_audit_logs(&audit_query),
        ),
        load_source(
            "schedules",
            schedules_read,
            state.repo.query_schedules(&schedules_query),
        ),
        load_source(
            "system_dashboard",
            fleet_read,
            load_system_dashboard(state, &system_query),
        ),
        load_home_dashboard_overview(state, fleet_read, prepared_dashboard, &operator.preferences,),
    );

    Ok(HomeSnapshotResponse {
        generated_at: unix_now().to_string(),
        operator,
        summary: fleet.summary,
        agents: fleet.agents,
        telemetry_rollups: fleet.telemetry_rollups,
        telemetry_network_rates: fleet.telemetry_network_rates,
        fleet_alerts: fleet.fleet_alerts,
        fleet_alerts_truncated: fleet.fleet_alerts_truncated,
        monitoring_cards,
        jobs,
        file_transfers,
        terminal_sessions,
        backups,
        backup_artifacts,
        audit,
        schedules,
        system_dashboard,
        dashboard_overview,
    })
}

async fn load_home_dashboard_overview(
    state: &AppState,
    permitted: bool,
    prepared: Option<PreparedDashboardOverview>,
    preferences: &crate::model::OperatorPreferences,
) -> FleetSnapshotSource<crate::model_dashboard::DashboardOverviewView> {
    if !permitted {
        return FleetSnapshotSource::unavailable("operator_scope_insufficient");
    }
    let Some(prepared) = prepared else {
        return FleetSnapshotSource::unavailable("home_snapshot_dashboard_overview_unavailable");
    };
    load_source(
        "dashboard_overview",
        true,
        load_prepared_dashboard_overview(state, prepared, preferences),
    )
    .await
}

async fn load_home_monitoring_cards(
    state: &AppState,
    permitted: bool,
    agents: Option<Vec<crate::model::AgentView>>,
) -> FleetSnapshotSource<Vec<crate::model_monitoring::MonitoringCardView>> {
    if !permitted {
        return FleetSnapshotSource::unavailable("operator_scope_insufficient");
    }
    let Some(agents) = agents else {
        return FleetSnapshotSource::unavailable("home_snapshot_monitoring_cards_unavailable");
    };
    load_source(
        "monitoring_cards",
        true,
        monitoring_cards_for_agents_projection(state, agents, false),
    )
    .await
}

async fn load_source<T, E, F>(
    source: &'static str,
    permitted: bool,
    future: F,
) -> FleetSnapshotSource<T>
where
    E: Debug,
    F: Future<Output = Result<T, E>>,
{
    if !permitted {
        return FleetSnapshotSource::unavailable("operator_scope_insufficient");
    }
    match future.await {
        Ok(data) => FleetSnapshotSource::available(data),
        Err(error) => {
            tracing::warn!(source, ?error, "home snapshot source failed");
            FleetSnapshotSource::unavailable(format!("home_snapshot_{source}_unavailable"))
        }
    }
}

fn history_query() -> ListQuery {
    ListQuery {
        limit: Some(HISTORY_DETAIL_LIMIT),
        sort: Some("created_at".to_string()),
        dir: Some("desc".to_string()),
        ..ListQuery::default()
    }
}

fn schedule_query() -> ListQuery {
    ListQuery {
        limit: Some(HISTORY_DETAIL_LIMIT),
        sort: Some("next_run_at".to_string()),
        dir: Some("asc".to_string()),
        ..ListQuery::default()
    }
}

async fn load_jobs(
    state: &AppState,
    query: &ListQuery,
) -> anyhow::Result<Vec<crate::model::JobHistoryView>> {
    let jobs = state.repo.query_jobs(query).await?;
    let mut reconciled = false;
    for job in jobs
        .iter()
        .filter(|job| job.command_type == "terminal_open")
    {
        state.repo.reconcile_terminal_job_by_id(job.id).await?;
        reconciled = true;
    }
    if reconciled {
        state.repo.query_jobs(query).await
    } else {
        Ok(jobs)
    }
}

async fn load_file_transfer_sessions(
    state: &AppState,
) -> anyhow::Result<Vec<crate::model_file_transfer::FileTransferSessionView>> {
    let mut sessions = state
        .repo
        .list_file_transfer_sessions(FLEET_DETAIL_LIMIT, None, None)
        .await?;
    state
        .repo
        .annotate_file_transfer_handoff_evidence(&mut sessions)
        .await?;
    Ok(sessions)
}

async fn load_terminal_sessions(
    state: &AppState,
) -> anyhow::Result<Vec<crate::model_terminal::TerminalSessionView>> {
    let sessions = state
        .repo
        .list_terminal_sessions(FLEET_DETAIL_LIMIT, None, None)
        .await?;
    for session in &sessions {
        state
            .repo
            .reconcile_terminal_job_by_id(session.job_id)
            .await?;
    }
    state
        .repo
        .list_terminal_sessions(FLEET_DETAIL_LIMIT, None, None)
        .await
}

#[cfg(test)]
#[path = "tests_routes_home_snapshot.rs"]
mod tests;
