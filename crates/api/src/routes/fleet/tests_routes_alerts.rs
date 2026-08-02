use super::*;

#[test]
fn condition_parse_errors_include_operator_actionable_detail() {
    let error = fleet_alert_policy_error(anyhow::anyhow!(
        "fleet_alert_policy_condition_invalid: unknown metric cpu.load1"
    ));

    assert_eq!(error.code, "fleet_alert_policy_condition_invalid");
    let message = error.public_message.expect("public condition detail");
    assert!(message.contains("unknown metric cpu.load1"));
    assert!(message.contains("cpu.load_1"));
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
        "billing_long_cycle_requires_day_month",
        "port_speed_unit_required",
        "port_speed_unit_invalid",
        "port_speed_value_invalid",
        "port_speed_value_too_large",
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
    assert_eq!(unknown.code, "internal_server_error");
    assert!(unknown.public_message.is_none());
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
