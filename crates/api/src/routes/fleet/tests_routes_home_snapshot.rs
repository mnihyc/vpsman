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
async fn home_snapshot_key_normalizes_relative_time_and_fences_operator_and_custom_bounds() {
    let state = test_state();
    let (context, _) = crate::test_auth_context_and_headers(&state).await;
    let relative = dashboard_query();
    let first = prepare_dashboard_overview(&relative, 1_700_000_000).unwrap();
    let second = prepare_dashboard_overview(&relative, 1_700_000_001).unwrap();
    assert_eq!(
        home_snapshot_singleflight_key(&context.operator, &relative, Some(&first)),
        home_snapshot_singleflight_key(&context.operator, &relative, Some(&second)),
    );

    let mut changed_operator = context.operator.clone();
    changed_operator.username.push_str("-changed");
    assert_ne!(
        home_snapshot_singleflight_key(&context.operator, &relative, Some(&first)),
        home_snapshot_singleflight_key(&changed_operator, &relative, Some(&first)),
    );

    let mut custom = dashboard_query();
    custom.window = None;
    custom.start_unix = Some(1_700_000_000);
    custom.end_unix = Some(1_700_003_600);
    let custom_first = prepare_dashboard_overview(&custom, 1_800_000_000).unwrap();
    let mut later_custom = dashboard_query();
    later_custom.window = None;
    later_custom.start_unix = Some(1_700_000_000);
    later_custom.end_unix = Some(1_700_003_601);
    let custom_second = prepare_dashboard_overview(&later_custom, 1_800_000_000).unwrap();
    assert_ne!(
        home_snapshot_singleflight_key(&context.operator, &custom, Some(&custom_first)),
        home_snapshot_singleflight_key(&context.operator, &later_custom, Some(&custom_second),),
    );

    let now = 1_800_000_000;
    let mut near_future = dashboard_query();
    near_future.window = None;
    near_future.start_unix = Some(now - 100);
    near_future.end_unix = Some(now + 5);
    let near_prepared = prepare_dashboard_overview(&near_future, now).unwrap();
    let mut far_future = near_future.clone();
    far_future.end_unix = Some(now + 100);
    let far_prepared = prepare_dashboard_overview(&far_future, now).unwrap();
    assert_ne!(
        home_snapshot_singleflight_key(&context.operator, &near_future, Some(&near_prepared),),
        home_snapshot_singleflight_key(&context.operator, &far_future, Some(&far_prepared),),
    );
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
async fn home_snapshot_does_not_validate_forbidden_dashboard_parameters() {
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
        .scopes = vec![SCOPE_JOBS_READ.to_string()];
    let mut invalid_dashboard = dashboard_query();
    invalid_dashboard.window = Some("not-a-dashboard-window".to_string());

    let Json(snapshot) = home_snapshot(State(state), headers, Query(invalid_dashboard))
        .await
        .expect("a forbidden dashboard source must not reject authorized Home sources");
    let value = serde_json::to_value(snapshot).unwrap();
    assert_eq!(
        value["dashboard_overview"]["error"],
        "operator_scope_insufficient"
    );
    assert!(!value["file_transfers"]["data"].is_null());
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
