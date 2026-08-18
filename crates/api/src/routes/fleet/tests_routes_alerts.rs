use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, header::CONTENT_TYPE, Request},
};
use tower::ServiceExt;

use crate::{
    gateway_client::GatewayDispatchClient,
    model::BootstrapOperatorRequest,
    model_alert_states::BulkFleetAlertStateItem,
    repository::{MemoryState, Repository},
    state::DispatcherRuntimeConfig,
};

#[test]
fn condition_parse_errors_include_operator_actionable_detail() {
    let error = fleet_alert_policy_error(anyhow::anyhow!(
        "fleet_alert_policy_condition_invalid: unknown metric cpu.load1"
    ));

    assert_eq!(error.code, "fleet_alert_policy_condition_invalid");
    let message = error.public_message.expect("public condition detail");
    assert!(message.contains("unknown metric cpu.load1"));
    assert!(message.contains("cpu.utilization_ratio"));
    assert!(message.contains("cpu.load_saturation (load per core)"));
    assert!(message.contains("cpu.load_1 (raw load)"));
}

#[test]
fn fleet_alert_policy_regression_input_errors_are_not_server_failures() {
    let overlap = vps_rules_error(anyhow::anyhow!("traffic_selector_direction_overlap"));
    assert_eq!(overlap.status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(overlap.code, "traffic_selector_direction_overlap");

    let invalid_bytes = vps_rules_error(anyhow::anyhow!("byte_size_number_invalid"));
    assert_eq!(invalid_bytes.status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(invalid_bytes.code, "byte_size_number_invalid");

    let name_conflict =
        fleet_alert_policy_error(anyhow::anyhow!("fleet_alert_policy_name_conflict"));
    assert_eq!(name_conflict.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(name_conflict.code, "fleet_alert_policy_name_conflict");

    let missing = fleet_alert_policy_error(anyhow::anyhow!("fleet_alert_policy_not_found"));
    assert_eq!(missing.status, axum::http::StatusCode::NOT_FOUND);
    assert_eq!(missing.code, "fleet_alert_policy_not_found");
}

#[test]
fn vps_rule_billing_and_port_speed_errors_are_bad_requests() {
    for code in [
        "billing_plan_price_required",
        "billing_plan_price_invalid",
        "billing_plan_currency_required",
        "billing_plan_currency_invalid",
        "billing_plan_period_required",
        "billing_plan_period_invalid",
        "billing_cycle_day_invalid",
        "billing_cycle_month_invalid",
        "billing_cycle_requires_price",
        "billing_cycle_disabled_price_invalid",
        "billing_month_cycle_requires_day",
        "billing_long_cycle_requires_month_day",
        "port_speed_unit_required",
        "port_speed_unit_invalid",
        "port_speed_value_invalid",
        "port_speed_value_too_large",
        "network_rate_selector_source_invalid",
    ] {
        let error = vps_rules_error(anyhow::anyhow!(code));
        assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST, "{code}");
        assert_eq!(error.code, code);
    }

    let unknown = vps_rules_error(anyhow::anyhow!("unexpected_storage_failure"));
    assert_eq!(
        unknown.status,
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(unknown.code, "vps_rules_mutation_failed");
    assert_eq!(
        unknown.public_message.as_deref(),
        Some("The VPS rule change could not be completed.")
    );
}

#[test]
fn fleet_alert_policy_delete_review_regression_maps_stale_and_missing() {
    let stale = fleet_alert_policy_error(anyhow::anyhow!("fleet_alert_policy_delete_review_stale"));
    assert_eq!(stale.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(stale.code, "fleet_alert_policy_delete_review_stale");

    let missing = fleet_alert_policy_error(anyhow::anyhow!("fleet_alert_policy_not_found"));
    assert_eq!(missing.status, axum::http::StatusCode::NOT_FOUND);
    assert_eq!(missing.code, "fleet_alert_policy_not_found");
}

#[test]
fn notification_channel_rejects_unsafe_targets_and_maps_name_conflicts() {
    let request = CreateFleetAlertNotificationChannelRequest {
        id: None,
        name: "Operations".to_string(),
        scope_kind: "global".to_string(),
        scope_value: None,
        min_severity: Some("warning".to_string()),
        categories: None,
        operator_states: None,
        delivery_kind: "webhook".to_string(),
        target: "not-a-webhook-url".to_string(),
        cooldown_secs: Some(300),
        enabled: Some(true),
        notes: None,
        confirmed: true,
    };
    let invalid = validate_alert_notification_channel_request(&request)
        .expect_err("an invalid webhook target must be rejected before persistence");
    assert_eq!(invalid.status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(invalid.code, "fleet_alert_notification_target_invalid");
    assert!(
        invalid
            .public_message
            .as_deref()
            .is_some_and(|message| message.contains("absolute URL")),
        "the API should preserve the actionable URL validation reason"
    );

    let conflict = fleet_alert_notification_channel_error(anyhow::anyhow!(
        "fleet_alert_notification_channel_name_conflict"
    ));
    assert_eq!(conflict.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(
        conflict.code,
        "fleet_alert_notification_channel_name_conflict"
    );

    let unexpected = fleet_alert_notification_channel_error(anyhow::anyhow!("storage_unavailable"));
    assert_eq!(
        unexpected.status,
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        unexpected.code,
        "fleet_alert_notification_channel_mutation_failed"
    );
    assert_eq!(
        unexpected.public_message.as_deref(),
        Some("The notification channel change could not be completed.")
    );
}

#[test]
fn notification_channel_request_is_strict_and_reports_its_own_scope_domain() {
    let missing_confirmation: CreateFleetAlertNotificationChannelRequest =
        serde_json::from_value(serde_json::json!({
            "name": "Operations",
            "scope_kind": "global",
            "delivery_kind": "webhook",
            "target": "https://hooks.acme.com/vpsman"
        }))
        .expect("omitted confirmation must deserialize as false for explicit validation");
    let confirmation_error = validate_alert_notification_channel_request(&missing_confirmation)
        .expect_err("unconfirmed writes must reach the reviewed-write validator");
    assert_eq!(
        confirmation_error.code,
        "fleet_alert_notification_channel_confirmation_required"
    );

    assert!(
        serde_json::from_value::<CreateFleetAlertNotificationChannelRequest>(serde_json::json!({
            "name": "Operations",
            "scope_kind": "global",
            "delivery_kind": "webhook",
            "target": "https://hooks.acme.com/vpsman",
            "confirmed": true,
            "confimed": true
        }))
        .is_err()
    );

    let invalid_scope = CreateFleetAlertNotificationChannelRequest {
        scope_kind: "policy-only-value".to_string(),
        confirmed: true,
        ..missing_confirmation
    };
    let scope_error = validate_alert_notification_channel_request(&invalid_scope)
        .expect_err("invalid notification scope must use the notification error domain");
    assert_eq!(
        scope_error.code,
        "fleet_alert_notification_scope_kind_invalid"
    );
}

#[test]
fn fleet_alert_bulk_validation_rejects_duplicates_and_overflow() {
    let duplicate = BulkUpdateFleetAlertStatesRequest {
        action: "acknowledge".to_string(),
        items: vec![
            BulkFleetAlertStateItem {
                alert_id: "alert:duplicate".to_string(),
                expected_revision: 0,
            },
            BulkFleetAlertStateItem {
                alert_id: "alert:duplicate".to_string(),
                expected_revision: 0,
            },
        ],
        muted_for_secs: None,
        reason: None,
        confirmed: true,
    };
    let error = validate_bulk_alert_state_request(&duplicate).unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "fleet_alert_state_duplicate_item");

    let overflow = BulkUpdateFleetAlertStatesRequest {
        items: (0..=1_000)
            .map(|index| BulkFleetAlertStateItem {
                alert_id: format!("alert:overflow:{index}"),
                expected_revision: 0,
            })
            .collect(),
        ..duplicate
    };
    let error = validate_bulk_alert_state_request(&overflow).unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "fleet_alert_state_items_invalid");
}

#[tokio::test]
async fn fleet_alert_bulk_route_accepts_more_than_one_hundred_items_in_one_response() {
    let memory = MemoryState::default();
    let repo = Repository::Memory(memory.clone());
    let auth = repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "bulk-alert-admin".to_string(),
            password: "bulk-alert-password-123".to_string(),
        })
        .await
        .unwrap();
    let (events, _) = crate::state::WsEventBus::new(1);
    let router = crate::routes::build_router(AppState {
        repo,
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
    });
    let body = serde_json::json!({
        "action": "acknowledge",
        "items": (0..101).map(|index| serde_json::json!({
            "alert_id": format!("alert:route:{index:03}"),
            "expected_revision": 0,
        })).collect::<Vec<_>>(),
        "reason": "reviewed as one batch",
        "confirmed": true,
    });
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/fleet-alert-states/bulk")
                .header(AUTHORIZATION, format!("Bearer {}", auth.access_token))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let response: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert!(response["batch_id"].as_str().is_some());
    assert_eq!(response["states"].as_array().unwrap().len(), 101);
    assert!(response["states"]
        .as_array()
        .unwrap()
        .iter()
        .all(|state| state["revision"] == 1));
    assert_eq!(memory.audits.read().await.len(), 101);
}
