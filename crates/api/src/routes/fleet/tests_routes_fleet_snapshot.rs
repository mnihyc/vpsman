use axum::extract::{Query, State};

use super::*;
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
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32_768,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: "config/vpsman.toml".into(),
        dispatcher_config: DispatcherRuntimeConfig::default(),
    }
}

#[tokio::test]
async fn live_snapshot_contains_only_the_live_sources() {
    let state = test_state();
    let Repository::Memory(memory) = &state.repo else {
        unreachable!();
    };
    memory.agents.write().await.push(uptime_test_agent("v-1"));
    memory
        .telemetry_samples
        .write()
        .await
        .push(crate::model::TelemetrySampleView {
            id: uuid::Uuid::new_v4(),
            client_id: "v-1".to_string(),
            observed_at: "200".to_string(),
            cpu_load_1: 0.1,
            memory_total_bytes: 1,
            memory_available_bytes: 1,
            payload: serde_json::json!({"uptime_secs": 123}),
        });
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
        "telemetry_uptimes",
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
    assert_eq!(
        value["telemetry_uptimes"]["data"],
        serde_json::json!([{
            "client_id": "v-1",
            "uptime_secs": 123,
            "observed_at": "200",
        }])
    );
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
        "telemetry_uptimes",
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
        "telemetry_uptimes",
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

#[tokio::test]
async fn uptime_snapshot_source_requires_fleet_read_scope() {
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
        .scopes = vec![SCOPE_CONFIG_READ.to_string()];

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

    assert!(value["telemetry_uptimes"]["data"].is_null());
    assert_eq!(
        value["telemetry_uptimes"]["error"],
        "operator_scope_insufficient"
    );
}

#[tokio::test]
async fn snapshot_source_failures_name_the_failed_source_without_leaking_the_cause() {
    let source: FleetSnapshotSource<Vec<String>> =
        load_source("telemetry_network_rates", true, async {
            Err(anyhow::anyhow!("private database address and query"))
        })
        .await;

    assert!(source.data.is_none());
    assert_eq!(
        source.error.as_deref(),
        Some("fleet_snapshot_telemetry_network_rates_unavailable")
    );
    assert!(!source.error.unwrap().contains("database"));
}

fn uptime_test_agent(client_id: &str) -> crate::model::AgentView {
    crate::model::AgentView {
        id: client_id.to_string(),
        display_name: client_id.to_string(),
        status: "online".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: vpsman_common::AgentCapabilitySnapshot::default(),
    }
}
