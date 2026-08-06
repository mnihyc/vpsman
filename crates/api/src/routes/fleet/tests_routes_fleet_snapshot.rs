use axum::extract::{Query, State};
use tokio::sync::broadcast;

use super::*;
use crate::{
    gateway_client::GatewayDispatchClient,
    repository::{MemoryState, Repository},
    state::{AppState, DispatcherRuntimeConfig},
};

fn test_state() -> AppState {
    let (events, _) = broadcast::channel(1);
    AppState {
        repo: Repository::Memory(MemoryState::default()),
        events,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        backup_object_store: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32_768,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: "config/vpsman.toml".into(),
        dispatcher_config: DispatcherRuntimeConfig::default(),
    }
}

#[tokio::test]
async fn live_snapshot_contains_only_the_five_live_sources() {
    let state = test_state();
    let headers = crate::test_auth_headers(&state).await;

    let Json(snapshot) = fleet_snapshot(
        State(state),
        headers,
        Query(FleetSnapshotQuery {
            mode: Some("live".to_string()),
        }),
    )
    .await
    .unwrap();
    let value = serde_json::to_value(snapshot).unwrap();

    assert_eq!(value["mode"], "live");
    assert!(value["generated_at"].as_str().is_some());
    for source in [
        "summary",
        "agents",
        "telemetry_rollups",
        "telemetry_network_rates",
        "telemetry_tunnels",
    ] {
        assert!(value[source]["data"].is_object() || value[source]["data"].is_array());
        assert!(value[source]["error"].is_null());
    }
    for source in [
        "fleet_alerts",
        "fleet_alert_states",
        "fleet_alert_policies",
        "vps_rule_values",
        "traffic_accounting",
        "policy_alerts",
        "fleet_alert_notification_channels",
        "fleet_alert_notifications",
        "webhook_rules",
        "webhook_rule_deliveries",
    ] {
        assert!(
            value.get(source).is_none(),
            "unexpected live source {source}"
        );
    }
}

#[tokio::test]
async fn full_snapshot_contains_all_current_fleet_detail_sources() {
    let state = test_state();
    let headers = crate::test_auth_headers(&state).await;

    let Json(snapshot) = fleet_snapshot(
        State(state),
        headers,
        Query(FleetSnapshotQuery {
            mode: Some("full".to_string()),
        }),
    )
    .await
    .unwrap();
    let value = serde_json::to_value(snapshot).unwrap();

    assert_eq!(value["mode"], "full");
    for source in [
        "summary",
        "agents",
        "telemetry_rollups",
        "telemetry_network_rates",
        "telemetry_tunnels",
        "fleet_alerts",
        "fleet_alert_states",
        "fleet_alert_policies",
        "vps_rule_values",
        "traffic_accounting",
        "policy_alerts",
        "fleet_alert_notification_channels",
        "fleet_alert_notifications",
        "webhook_rules",
        "webhook_rule_deliveries",
    ] {
        assert!(
            !value[source]["data"].is_null(),
            "missing full source {source}"
        );
        assert!(
            value[source]["error"].is_null(),
            "failed full source {source}"
        );
    }
}

#[tokio::test]
async fn full_snapshot_keeps_scope_failures_local_to_each_source() {
    let state = test_state();
    let (context, headers) = crate::test_auth_context_and_headers(&state).await;
    let Repository::Memory(memory) = &state.repo else {
        unreachable!();
    };
    let mut operators = memory.operators.write().await;
    operators
        .iter_mut()
        .find(|operator| operator.id == context.operator.id)
        .unwrap()
        .scopes = vec![SCOPE_FLEET_READ.to_string()];
    drop(operators);

    let Json(snapshot) = fleet_snapshot(
        State(state),
        headers,
        Query(FleetSnapshotQuery {
            mode: Some("full".to_string()),
        }),
    )
    .await
    .unwrap();
    let value = serde_json::to_value(snapshot).unwrap();

    for source in [
        "summary",
        "agents",
        "telemetry_rollups",
        "telemetry_network_rates",
        "telemetry_tunnels",
        "fleet_alert_states",
        "fleet_alert_policies",
        "traffic_accounting",
        "policy_alerts",
    ] {
        assert!(
            !value[source]["data"].is_null(),
            "permitted source {source}"
        );
        assert!(value[source]["error"].is_null());
    }
    for source in [
        "fleet_alerts",
        "vps_rule_values",
        "fleet_alert_notification_channels",
        "fleet_alert_notifications",
        "webhook_rules",
        "webhook_rule_deliveries",
    ] {
        assert!(value[source]["data"].is_null(), "forbidden source {source}");
        assert_eq!(value[source]["error"], "operator_scope_insufficient");
    }
}

#[tokio::test]
async fn snapshot_requires_authentication_and_an_explicit_valid_mode() {
    let state = test_state();
    let error = fleet_snapshot(
        State(state.clone()),
        HeaderMap::new(),
        Query(FleetSnapshotQuery {
            mode: Some("live".to_string()),
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "missing_bearer_token");

    let headers = crate::test_auth_headers(&state).await;
    for (mode, expected) in [
        (None, "fleet_snapshot_mode_required"),
        (Some("other".to_string()), "fleet_snapshot_mode_invalid"),
    ] {
        let error = fleet_snapshot(
            State(state.clone()),
            headers.clone(),
            Query(FleetSnapshotQuery { mode }),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, expected);
    }
}
