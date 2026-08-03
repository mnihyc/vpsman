use std::collections::HashMap;

use chrono::{TimeZone, Utc};
use serde_json::json;

use super::{
    aggregate_memory_traffic_counter_usage, aggregate_memory_traffic_history,
    claim_traffic_selector_directions, derive_cycle_usage, next_policy_rule_state,
    parse_billing_cycle, parse_billing_price, parse_byte_size, parse_network_rate_interfaces,
    parse_persisted_traffic_selector_list, parse_port_speed,
    parse_stored_network_rate_selector_spec, parse_traffic_selector, parse_traffic_selector_list,
    parse_vps_rule_value, policy_identifier_value, policy_state_is_alert_eligible,
    policy_webhook_repair_is_recent, resolve_network_rate_interface_selection,
    traffic_accounting_for_client, validate_billing_rule_group, NetworkRateSelectorReference,
    NetworkRateSelectorSpec, PolicyEvaluation, PolicyRuleRecord, PolicyRuleStateRecord,
    TrafficCounterSampleRecord, TrafficCounterStreamUsage, TrafficHistoryStream,
    TrafficStreamRequest, VpsRuleValueRecord, VPS_RULE_KEY_NETWORK_RATE_INTERFACES,
    VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL, VPS_RULE_KEY_TRAFFIC_RESET_DAY,
    VPS_RULE_KEY_TRAFFIC_SELECTORS,
};

#[test]
fn billing_price_and_cycle_are_canonical_and_period_aware() {
    let monthly = parse_billing_price("029.9 cny/m").unwrap();
    assert_eq!(monthly.raw, "29.90 CNY/m");
    assert_eq!(monthly.json["price"], "29.90");
    assert_eq!(monthly.json["period"], "month");
    let half_year = parse_billing_price("60 €/h").unwrap();
    assert_eq!(half_year.raw, "60.00 €/hy");
    assert_eq!(half_year.json["currency"], "EUR");
    assert_eq!(
        parse_billing_price("500USD / m").unwrap().raw,
        "500.00 USD/m"
    );
    assert_eq!(parse_billing_price("10.2 ￥/m").unwrap().raw, "10.20 ¥/m");
    assert_eq!(
        parse_billing_price("\u{2003}10.2\u{a0}￥\u{2009}/\u{3000}m\u{202f}")
            .unwrap()
            .raw,
        "10.20 ¥/m"
    );
    assert_eq!(parse_billing_cycle("7").unwrap().raw, "7");
    assert_eq!(parse_billing_cycle("7-6").unwrap().raw, "07-06");
    validate_billing_rule_group(Some("29.90 CNY/m"), Some("7")).unwrap();
    validate_billing_rule_group(Some("60.00 EUR/hy"), Some("07-06")).unwrap();
    assert!(
        validate_billing_rule_group(Some("29.90 CNY/m"), Some("07-06"))
            .unwrap_err()
            .to_string()
            .contains("billing_month_cycle_requires_day")
    );
    assert!(validate_billing_rule_group(None, Some("7"))
        .unwrap_err()
        .to_string()
        .contains("billing_cycle_requires_price"));
}

#[test]
fn explicit_disabled_billing_and_unlimited_quota_are_not_unset() {
    let disabled = parse_billing_price("-1").unwrap();
    assert_eq!(disabled.raw, "-1");
    assert_eq!(disabled.display, "n/a");
    assert!(disabled.json["disabled"].as_bool().unwrap());
    assert!(validate_billing_rule_group(Some("-1"), None).is_ok());
    assert!(validate_billing_rule_group(Some("-1"), Some("7")).is_err());

    let unlimited = parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL, "-1").unwrap();
    assert_eq!(unlimited.raw, "-1");
    assert_eq!(unlimited.json["bytes"], -1);
    assert_eq!(unlimited.display, "Unlimited");
}

#[test]
fn port_speed_is_display_only_but_strictly_normalized() {
    let mbps = parse_port_speed("400Mbps").unwrap();
    assert_eq!(mbps.raw, "400 Mbps");
    assert_eq!(mbps.json["bps"], 400_000_000_i64);
    let gbps = parse_port_speed("1.500 Gbps").unwrap();
    assert_eq!(gbps.raw, "1.5 Gbps");
    assert_eq!(gbps.json["bps"], 1_500_000_000_i64);
    assert!(parse_port_speed("fast").is_err());
}

#[test]
fn live_rate_selector_reuses_traffic_selector_syntax_and_explicit_all_marker() {
    for all in ["", "[]"] {
        let parsed = parse_network_rate_interfaces(all).unwrap();
        assert_eq!(parsed.raw, "[]");
        assert_eq!(parsed.json["mode"], "all");
    }

    let referenced = parse_network_rate_interfaces("[traffic.selectors]").unwrap();
    assert_eq!(referenced.raw, "[traffic.selectors]");
    assert_eq!(referenced.json["mode"], "reference");
    assert_eq!(referenced.json["reference"]["rule"], "traffic.selectors");
    assert!(matches!(
        parse_stored_network_rate_selector_spec(&referenced.json).unwrap(),
        NetworkRateSelectorSpec::Reference(NetworkRateSelectorReference::TrafficSelectors)
    ));
    let singular = parse_network_rate_interfaces("[traffic.selector]").unwrap();
    assert_eq!(singular.json["mode"], "exact");

    let exact = parse_network_rate_interfaces("host:eth0, eth1+tx").unwrap();
    assert_eq!(exact.raw, "eth0,eth1+tx");
    assert_eq!(exact.json["mode"], "exact");
    assert_eq!(exact.json["selectors"][0]["direction"], "total");
    assert_eq!(exact.json["selectors"][1]["direction"], "tx");

    assert_eq!(
        parse_network_rate_interfaces("tunnel:wg0")
            .unwrap_err()
            .to_string(),
        "network_rate_selector_source_invalid"
    );
    assert!(parse_network_rate_interfaces("eth0,eth0+tx")
        .unwrap_err()
        .to_string()
        .contains("traffic_selector_direction_overlap"));

    let mut inconsistent = exact.json;
    inconsistent["selectors"][0]["source"] = json!("tunnel");
    assert!(parse_stored_network_rate_selector_spec(&inconsistent).is_err());
}

#[test]
fn live_rate_selector_defaults_to_reference_unless_all_or_exact_is_explicit() {
    let client_ids = vec![
        "v-1".to_string(),
        "v-2".to_string(),
        "v-3".to_string(),
        "v-4".to_string(),
        "v-6".to_string(),
        "v-7".to_string(),
        "v-8".to_string(),
    ];
    let mut stored_reference =
        parsed_rule_for("v-4", VPS_RULE_KEY_NETWORK_RATE_INTERFACES, "eth9+tx");
    stored_reference.value_json = json!({
        "mode": "reference",
        "reference": {"rule": "traffic.selectors"},
    });
    let rules = vec![
        parsed_rule_for(
            "v-1",
            VPS_RULE_KEY_TRAFFIC_SELECTORS,
            "eth0+rx,eth1+tx,tunnel:wg0",
        ),
        parsed_rule_for(
            "v-1",
            VPS_RULE_KEY_NETWORK_RATE_INTERFACES,
            "[traffic.selectors]",
        ),
        parsed_rule_for("v-2", VPS_RULE_KEY_NETWORK_RATE_INTERFACES, "[]"),
        parsed_rule_for("v-3", VPS_RULE_KEY_TRAFFIC_SELECTORS, "eth0"),
        parsed_rule_for("v-3", VPS_RULE_KEY_NETWORK_RATE_INTERFACES, "eth9+tx"),
        parsed_rule_for("v-4", VPS_RULE_KEY_TRAFFIC_SELECTORS, "eth4+rx"),
        stored_reference,
        parsed_rule_for("v-6", VPS_RULE_KEY_TRAFFIC_SELECTORS, "tunnel:wg0"),
        parsed_rule_for(
            "v-6",
            VPS_RULE_KEY_NETWORK_RATE_INTERFACES,
            "[traffic.selectors]",
        ),
        parsed_rule_for("v-7", VPS_RULE_KEY_TRAFFIC_SELECTORS, "eth7+rx"),
    ];

    let selected = resolve_network_rate_interface_selection(&client_ids, &rules).unwrap();
    assert!(selected.allows("v-1", "eth0"));
    assert!(selected.allows("v-1", "eth1"));
    assert!(!selected.allows("v-1", "wg0"));
    assert!(selected.allows("v-2", "anything"));
    assert!(!selected.allows("v-3", "eth0"));
    assert!(selected.allows("v-3", "eth9"));
    assert!(!selected.allows("v-4", "eth9"));
    assert!(selected.allows("v-4", "eth4"));
    assert!(!selected.allows("v-6", "wg0"));
    assert!(selected.allows("v-7", "eth7"));
    assert!(!selected.allows("v-7", "anything"));
    assert!(!selected.allows("v-8", "anything"));

    let mut changed_rules = rules.clone();
    changed_rules
        .retain(|rule| !(rule.client_id == "v-1" && rule.key == VPS_RULE_KEY_TRAFFIC_SELECTORS));
    changed_rules.push(parsed_rule_for(
        "v-1",
        VPS_RULE_KEY_TRAFFIC_SELECTORS,
        "eth7+tx",
    ));
    let changed = resolve_network_rate_interface_selection(&client_ids, &changed_rules).unwrap();
    assert!(!changed.allows("v-1", "eth0"));
    assert!(changed.allows("v-1", "eth7"));

    let mut invalid_reference =
        parsed_rule_for("v-5", VPS_RULE_KEY_NETWORK_RATE_INTERFACES, "eth9+tx");
    invalid_reference.value_json = json!({
        "mode": "reference",
        "reference": {"rule": "billing.price"},
    });
    assert!(
        resolve_network_rate_interface_selection(&["v-5".to_string()], &[invalid_reference])
            .is_err()
    );
}

#[test]
fn traffic_stream_aggregation_preserves_usage_across_resets_and_epochs() {
    let samples = vec![
        sample(90, 100, 200, 0, 0),
        sample(105, 110, 220, 0, 0),
        sample(110, 130, 260, 0, 0),
        sample(120, 10, 300, 1, 0),
        sample(125, 20, 320, 1, 0),
        sample(130, 30, 340, 1, 0),
        sample(140, 50, 60, 1, 1),
        sample(145, 60, 70, 1, 1),
        sample(150, 70, 80, 1, 1),
    ];
    let full_usage = derive_cycle_usage(&samples, 100, 150);
    let aggregated = aggregate_memory_traffic_counter_usage(
        &samples,
        &[TrafficStreamRequest {
            client_id: "edge-a".to_string(),
            source_kind: "host".to_string(),
            interface: "eth0".to_string(),
            cycle_start_unix: 100,
        }],
        150,
    );
    assert_eq!(full_usage.cycle_rx, 90);
    assert_eq!(full_usage.cycle_tx, 160);
    assert_eq!(aggregated.len(), 1);
    assert_eq!(aggregated[0].cycle_rx, full_usage.cycle_rx);
    assert_eq!(aggregated[0].cycle_tx, full_usage.cycle_tx);
    assert_eq!(aggregated[0].latest_rx, full_usage.latest_rx);
    assert_eq!(aggregated[0].latest_tx, full_usage.latest_tx);
    assert_eq!(aggregated[0].last_sample_unix, 150);
    assert_eq!(aggregated[0].rx_counter_epochs_seen, 2);
    assert_eq!(aggregated[0].tx_counter_epochs_seen, 2);
}

#[test]
fn traffic_history_ignores_resets_in_unselected_direction() {
    let samples = vec![sample(90, 100, 200, 0, 0), sample(120, 120, 10, 0, 1)];
    let rx = aggregate_memory_traffic_history(
        samples.clone(),
        &[TrafficHistoryStream {
            source_kind: "host".to_string(),
            interface: "eth0".to_string(),
            direction_mask: 0b01,
        }],
        100,
        130,
        60,
    );
    assert_eq!(rx.len(), 1);
    assert_eq!(rx[0].sample_count, 1);
    assert_eq!(rx[0].reset_count, 0);
    assert_eq!(rx[0].rx_bytes, Some(20));

    let tx = aggregate_memory_traffic_history(
        samples,
        &[TrafficHistoryStream {
            source_kind: "host".to_string(),
            interface: "eth0".to_string(),
            direction_mask: 0b10,
        }],
        100,
        130,
        60,
    );
    assert_eq!(tx.len(), 1);
    assert_eq!(tx[0].sample_count, 0);
    assert_eq!(tx[0].reset_count, 1);
    assert_eq!(tx[0].tx_bytes, None);

    let total = aggregate_memory_traffic_history(
        vec![sample(90, 100, 200, 0, 0), sample(120, 120, 10, 0, 1)],
        &[TrafficHistoryStream {
            source_kind: "host".to_string(),
            interface: "eth0".to_string(),
            direction_mask: 0b11,
        }],
        100,
        130,
        60,
    );
    assert_eq!(total.len(), 1);
    assert_eq!(total[0].sample_count, 1);
    assert_eq!(total[0].reset_count, 1);
    assert_eq!(total[0].rx_bytes, Some(20));
    assert_eq!(total[0].tx_bytes, None);
    assert_eq!(total[0].total_bytes, None);
}

#[test]
fn traffic_selectors_reject_overlapping_directions() {
    let error = parse_traffic_selector_list("eth0,eth0+rx").unwrap_err();
    assert!(error
        .to_string()
        .contains("traffic_selector_direction_overlap"));

    let selectors = parse_traffic_selector_list("eth0+rx,eth0+tx").unwrap();
    assert_eq!(selectors.len(), 2);

    assert!(parse_traffic_selector(&format!("{}+rx", "x".repeat(129))).is_err());
    assert!(parse_traffic_selector("eth0\0+rx").is_err());
}

#[test]
fn policy_regression_persisted_traffic_selectors_accept_legacy_direction_overlap() {
    let selectors = parse_persisted_traffic_selector_list("eth0,eth0+rx").unwrap();
    assert_eq!(selectors.len(), 2);
    assert!(parse_traffic_selector_list("eth0,eth0+rx").is_err());
    assert_eq!(
        parse_byte_size("wat").unwrap_err().to_string(),
        "byte_size_number_invalid"
    );
    assert_eq!(
        parse_byte_size("1..2GB").unwrap_err().to_string(),
        "byte_size_number_invalid"
    );
}

#[test]
fn traffic_direction_accounting_is_defensively_idempotent() {
    let rx = parse_traffic_selector("eth0+rx").unwrap();
    let total = parse_traffic_selector("eth0").unwrap();
    let tx = parse_traffic_selector("eth0+tx").unwrap();
    let mut counted = HashMap::new();

    assert_eq!(
        claim_traffic_selector_directions(&mut counted, &rx),
        (true, false)
    );
    assert_eq!(
        claim_traffic_selector_directions(&mut counted, &total),
        (false, true)
    );
    assert_eq!(
        claim_traffic_selector_directions(&mut counted, &tx),
        (false, false)
    );
}

#[test]
fn stale_traffic_fails_policy_evaluation_closed() {
    let now = Utc.timestamp_opt(2_000_000_000, 0).single().unwrap();
    let rules = traffic_rules("eth0");
    let accounting = traffic_accounting_for_client(
        "edge-a",
        &rules,
        &[usage("eth0", now.timestamp() - 901)],
        now,
    );

    assert_eq!(accounting.state, "stale");
    assert_eq!(
        accounting.incomplete_reasons,
        vec!["eth0 sample stale".to_string()]
    );
    let mut incomplete = Vec::new();
    assert_eq!(
        policy_identifier_value(
            "traffic.cycle.total",
            Some(&accounting),
            None,
            &mut incomplete,
        ),
        None
    );
    assert_eq!(incomplete, vec!["eth0 sample stale".to_string()]);
}

#[test]
fn mixed_stream_freshness_uses_the_oldest_selected_sample() {
    let now = Utc.timestamp_opt(2_000_000_000, 0).single().unwrap();
    let rules = traffic_rules("eth0,eth1");
    let accounting = traffic_accounting_for_client(
        "edge-a",
        &rules,
        &[
            usage("eth0", now.timestamp() - 10),
            usage("eth1", now.timestamp() - 901),
        ],
        now,
    );

    assert_eq!(accounting.state, "stale");
    assert_eq!(
        accounting
            .last_sample_at
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.timestamp()),
        Some(now.timestamp() - 901)
    );
    assert_eq!(
        accounting.incomplete_reasons,
        vec!["eth1 sample stale".to_string()]
    );
}

#[test]
fn policy_state_generation_advances_past_historical_alerts_after_rule_recreation() {
    let now = Utc.timestamp_opt(2_000_000_000, 0).single().unwrap();
    let state = next_policy_rule_state(
        &policy_rule(2),
        "edge-a",
        &true_policy_evaluation(),
        None,
        7,
        now,
    )
    .unwrap();

    assert_eq!(state.trigger_generation, 8);
    assert!(policy_state_is_alert_eligible(&state));
}

#[test]
fn sustained_true_policy_state_remains_eligible_for_outbox_repair_without_refiring() {
    let now = Utc.timestamp_opt(2_000_000_060, 0).single().unwrap();
    let existing = policy_state(7, true, "2033-05-18T03:32:20+00:00");
    let state = next_policy_rule_state(
        &policy_rule(1),
        "edge-a",
        &true_policy_evaluation(),
        Some(&existing),
        7,
        now,
    )
    .unwrap();

    assert_eq!(state.trigger_generation, 7);
    assert!(state.previous_condition_true);
    assert!(policy_state_is_alert_eligible(&state));
}

#[test]
fn webhook_repair_is_bounded_so_retention_does_not_redeliver_old_alerts() {
    let now = Utc.timestamp_opt(2_000_000_000, 0).single().unwrap();
    assert!(policy_webhook_repair_is_recent(
        &Utc.timestamp_opt(1_999_999_940, 0)
            .single()
            .unwrap()
            .to_rfc3339(),
        now,
    ));
    assert!(!policy_webhook_repair_is_recent(
        &Utc.timestamp_opt(1_999_990_000, 0)
            .single()
            .unwrap()
            .to_rfc3339(),
        now,
    ));
}

fn policy_rule(rule_version: i32) -> PolicyRuleRecord {
    PolicyRuleRecord {
        id: uuid::Uuid::from_u128(1),
        group_id: uuid::Uuid::from_u128(2),
        rule_version,
        sort_order: 0,
        name: "quota".to_string(),
        enabled: true,
        traffic_selector: None,
        condition_expression: "cpu.load_1 > 1".to_string(),
        window_secs: 0,
        severity: "warning".to_string(),
        created_at: "test".to_string(),
        updated_at: "test".to_string(),
    }
}

fn true_policy_evaluation() -> PolicyEvaluation {
    PolicyEvaluation {
        condition_true: true,
        incomplete: false,
        incomplete_reasons: Vec::new(),
        actual_value: Some(2.0),
        threshold_value: Some(1.0),
        category: "resource".to_string(),
        payload: json!({}),
    }
}

fn policy_state(
    trigger_generation: i64,
    condition_true: bool,
    evaluated_at: &str,
) -> PolicyRuleStateRecord {
    PolicyRuleStateRecord {
        policy_rule_id: uuid::Uuid::from_u128(1),
        client_id: "edge-a".to_string(),
        rule_version: 1,
        condition_true,
        previous_condition_true: false,
        window_satisfied: condition_true,
        first_true_at: condition_true.then(|| evaluated_at.to_string()),
        last_true_at: condition_true.then(|| evaluated_at.to_string()),
        last_false_at: (!condition_true).then(|| evaluated_at.to_string()),
        last_evaluated_at: evaluated_at.to_string(),
        incomplete: false,
        incomplete_reasons: Vec::new(),
        last_actual_value: Some(2.0),
        last_threshold_value: Some(1.0),
        last_fired_at: condition_true.then(|| evaluated_at.to_string()),
        trigger_generation,
        updated_at: evaluated_at.to_string(),
    }
}

fn traffic_rules(selectors: &str) -> Vec<VpsRuleValueRecord> {
    vec![
        rule(VPS_RULE_KEY_TRAFFIC_RESET_DAY, "1", json!({"day": 1})),
        rule(
            VPS_RULE_KEY_TRAFFIC_SELECTORS,
            selectors,
            json!({"selectors": []}),
        ),
        rule(
            VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL,
            "1GB",
            json!({"bytes": 1_000_000_000_i64}),
        ),
    ]
}

fn rule(key: &str, value_raw: &str, value_json: serde_json::Value) -> VpsRuleValueRecord {
    VpsRuleValueRecord {
        client_id: "edge-a".to_string(),
        key: key.to_string(),
        value_raw: value_raw.to_string(),
        value_json,
        parsed_display: value_raw.to_string(),
        state: "valid".to_string(),
        validation_errors: Vec::new(),
        source_kind: "test".to_string(),
        source_id: None,
        updated_by: None,
        updated_at: "test".to_string(),
    }
}

fn parsed_rule_for(client_id: &str, key: &str, value: &str) -> VpsRuleValueRecord {
    let parsed = parse_vps_rule_value(key, value).unwrap();
    let mut record = rule(key, &parsed.raw, parsed.json);
    record.client_id = client_id.to_string();
    record
}

fn usage(interface: &str, last_sample_unix: i64) -> TrafficCounterStreamUsage {
    TrafficCounterStreamUsage {
        client_id: "edge-a".to_string(),
        source_kind: "host".to_string(),
        interface: interface.to_string(),
        cycle_rx: 100,
        cycle_tx: 200,
        latest_rx: 1_000,
        latest_tx: 2_000,
        last_sample_unix,
        rx_counter_epochs_seen: 1,
        tx_counter_epochs_seen: 1,
    }
}

fn sample(
    observed_unix: i64,
    rx_bytes: i64,
    tx_bytes: i64,
    rx_counter_epoch: i64,
    tx_counter_epoch: i64,
) -> TrafficCounterSampleRecord {
    TrafficCounterSampleRecord {
        client_id: "edge-a".to_string(),
        source_kind: "host".to_string(),
        interface: "eth0".to_string(),
        observed_at: observed_unix.to_string(),
        observed_unix,
        rx_bytes,
        tx_bytes,
        rx_counter_epoch,
        tx_counter_epoch,
        sample_source: "test".to_string(),
    }
}
