use super::*;
use axum::extract::{Query, State};
use axum::http::HeaderMap;

use crate::{
    gateway_client::GatewayDispatchClient,
    repository::{MemoryState, Repository},
    state::{AppState, DispatcherRuntimeConfig},
};

fn test_state() -> AppState {
    let (events, _) = crate::state::WsEventBus::new(1);
    AppState {
        repo: Repository::Memory(MemoryState::default()),
        events,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        backup_object_store: None,
        update_release_policy: Default::default(),
        job_output_artifact_min_bytes: 32_768,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: "config/vpsman.toml".into(),
        dispatcher_config: DispatcherRuntimeConfig::default(),
    }
}

fn dashboard_query() -> DashboardOverviewQuery {
    DashboardOverviewQuery {
        window: Some("1d".to_string()),
        start_unix: None,
        end_unix: None,
        start_at: None,
        end_at: None,
        scope_kind: Some("all".to_string()),
        scope_value: None,
        group_by: Some("labels".to_string()),
        resource_metric: Some("cpu_load".to_string()),
        chart_points: Some(240),
    }
}

#[tokio::test]
async fn home_snapshot_projects_every_initial_home_source_in_one_response() {
    let state = test_state();
    let headers = crate::test_auth_headers(&state).await;

    let Json(snapshot) = home_snapshot(State(state), headers, Query(dashboard_query()))
        .await
        .unwrap();
    let value = serde_json::to_value(snapshot).unwrap();

    assert_eq!(value["operator"]["role"], "admin");
    for source in [
        "summary",
        "agents",
        "telemetry_rollups",
        "telemetry_network_rates",
        "fleet_alerts",
        "monitoring_cards",
        "jobs",
        "file_transfers",
        "terminal_sessions",
        "backups",
        "backup_artifacts",
        "audit",
        "schedules",
        "system_dashboard",
        "dashboard_overview",
    ] {
        assert!(
            !value[source]["data"].is_null(),
            "missing Home source {source}: {}",
            value[source]
        );
        assert!(
            value[source]["error"].is_null(),
            "failed Home source {source}"
        );
    }
}

#[tokio::test]
async fn home_snapshot_keeps_scope_failures_local_to_each_source() {
    let state = test_state();
    let (context, headers) = crate::test_auth_context_and_headers(&state).await;
    let Repository::Memory(memory) = &state.repo else {
        unreachable!();
    };
    memory
        .operators
        .write()
        .await
        .iter_mut()
        .find(|operator| operator.id == context.operator.id)
        .unwrap()
        .scopes = vec![SCOPE_FLEET_READ.to_string()];

    let Json(snapshot) = home_snapshot(State(state), headers, Query(dashboard_query()))
        .await
        .unwrap();
    let value = serde_json::to_value(snapshot).unwrap();

    for source in [
        "summary",
        "agents",
        "telemetry_rollups",
        "telemetry_network_rates",
        "monitoring_cards",
        "jobs",
        "system_dashboard",
        "dashboard_overview",
    ] {
        assert!(
            !value[source]["data"].is_null(),
            "permitted source {source}"
        );
        assert!(value[source]["error"].is_null());
    }
    for source in [
        "fleet_alerts",
        "file_transfers",
        "terminal_sessions",
        "backups",
        "backup_artifacts",
        "audit",
        "schedules",
    ] {
        assert!(value[source]["data"].is_null(), "forbidden source {source}");
        assert_eq!(value[source]["error"], "operator_scope_insufficient");
    }
}

#[tokio::test]
async fn home_snapshot_requires_authentication() {
    let error = home_snapshot(
        State(test_state()),
        HeaderMap::new(),
        Query(dashboard_query()),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "missing_bearer_token");
}

#[tokio::test]
async fn home_snapshot_source_preserves_partial_failure_isolation() {
    let available = load_source("available", true, async {
        Ok::<_, anyhow::Error>(vec![1, 2])
    })
    .await;
    assert_eq!(available.data, Some(vec![1, 2]));
    assert_eq!(available.error, None);

    let unavailable = load_source("failed", true, async {
        anyhow::bail!("fixture source failed")
    })
    .await;
    assert_eq!(unavailable.data, None::<Vec<i32>>);
    assert_eq!(
        unavailable.error.as_deref(),
        Some("home_snapshot_failed_unavailable")
    );
}
