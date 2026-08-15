use std::collections::HashMap;

use chrono::{TimeZone, Utc};
use serde_json::json;

use super::{
    aggregate_memory_raw_traffic_history, aggregate_memory_traffic_counter_usage,
    aggregate_memory_traffic_history, claim_traffic_selector_directions, derive_cycle_usage,
    network_rate_selector_spec_from_rule, next_policy_rule_state, parse_billing_cycle,
    parse_billing_price, parse_byte_size, parse_network_rate_interfaces,
    parse_persisted_traffic_selector_list, parse_port_speed, parse_traffic_selector,
    parse_traffic_selector_list, parse_vps_rule_value, policy_identifier_value,
    policy_state_is_alert_eligible, policy_webhook_repair_is_recent,
    resolve_network_rate_interface_selection, traffic_accounting_for_client,
    traffic_cycle_starts_for_clients, validate_billing_rule_group, NetworkRateSelectorReference,
    NetworkRateSelectorSpec, PolicyEvaluation, PolicyRuleRecord, PolicyRuleStateRecord,
    TrafficCounterRollupRecord, TrafficCounterSampleRecord, TrafficCounterStreamUsage,
    TrafficHistoryStream, TrafficStreamRequest, VpsRuleValueRecord, NO_RESET_TRAFFIC_START_UNIX,
    VPS_RULE_KEY_NETWORK_RATE_INTERFACES, VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL,
    VPS_RULE_KEY_TRAFFIC_RESET_DAY, VPS_RULE_KEY_TRAFFIC_SELECTORS,
};

#[test]
fn api_vps_rule_constants_follow_the_common_registry() {
    let mut api_keys = vec![
        crate::model_alert_policies::VPS_RULE_KEY_BILLING_CYCLE,
        crate::model_alert_policies::VPS_RULE_KEY_BILLING_PRICE,
        crate::model_alert_policies::VPS_RULE_KEY_NETWORK_PORT_SPEED,
        crate::model_alert_policies::VPS_RULE_KEY_NETWORK_RATE_INTERFACES,
        crate::model_alert_policies::VPS_RULE_KEY_TRAFFIC_QUOTA_RX,
        crate::model_alert_policies::VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL,
        crate::model_alert_policies::VPS_RULE_KEY_TRAFFIC_QUOTA_TX,
        crate::model_alert_policies::VPS_RULE_KEY_TRAFFIC_RESET_DAY,
        crate::model_alert_policies::VPS_RULE_KEY_TRAFFIC_SELECTORS,
    ];
    api_keys.sort_unstable();
    let mut common_keys = vpsman_common::SUPPORTED_VPS_RULE_KEYS.to_vec();
    common_keys.sort_unstable();
    assert_eq!(api_keys, common_keys);
}

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
    assert_eq!(parse_billing_cycle("6-15").unwrap().raw, "06-15");
    validate_billing_rule_group(Some("29.90 CNY/m"), Some("7")).unwrap();
    validate_billing_rule_group(Some("60.00 EUR/hy"), Some("06-15")).unwrap();
    assert!(
        validate_billing_rule_group(Some("29.90 CNY/m"), Some("06-15"))
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
    assert_eq!(disabled.display, "-");
    assert_eq!(disabled.json["display"], "-");
    assert!(disabled.json["disabled"].as_bool().unwrap());
    assert!(validate_billing_rule_group(Some("-1"), None).is_ok());
    assert!(validate_billing_rule_group(Some("-1"), Some("7")).is_err());

    let unlimited = parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL, "-1").unwrap();
    assert_eq!(unlimited.raw, "-1");
    assert_eq!(unlimited.json["bytes"], -1);
    assert_eq!(unlimited.display, "Unlimited");
}

#[test]
fn traffic_reset_day_accepts_explicit_no_reset_without_weakening_monthly_validation() {
    let no_reset = parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_RESET_DAY, "-1").unwrap();
    assert_eq!(no_reset.raw, "-1");
    assert_eq!(no_reset.json, json!({"day": -1}));
    assert_eq!(no_reset.display, "-");

    let monthly = parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_RESET_DAY, "31").unwrap();
    assert_eq!(monthly.json, json!({"day": 31}));
    assert_eq!(monthly.display, "31 UTC");

    for invalid in ["-2", "0", "32"] {
        assert!(parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_RESET_DAY, invalid).is_err());
    }
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
    let referenced_rule =
        parsed_rule_for("v-1", VPS_RULE_KEY_NETWORK_RATE_INTERFACES, &referenced.raw);
    assert!(matches!(
        network_rate_selector_spec_from_rule(&referenced_rule).unwrap(),
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

    let mut invalid_stored =
        parsed_rule_for("v-1", VPS_RULE_KEY_NETWORK_RATE_INTERFACES, &exact.raw);
    invalid_stored.value_raw = "tunnel:wg0".to_string();
    assert!(network_rate_selector_spec_from_rule(&invalid_stored).is_err());
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
    let stored_reference = parsed_rule_for(
        "v-4",
        VPS_RULE_KEY_NETWORK_RATE_INTERFACES,
        "[traffic.selectors]",
    );
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
    assert!(selected.expects_rates("v-1"));
    assert!(!selected.allows("v-1", "wg0"));
    assert!(selected.allows("v-2", "anything"));
    assert!(selected.expects_rates("v-2"));
    assert!(!selected.allows("v-3", "eth0"));
    assert!(selected.allows("v-3", "eth9"));
    assert!(!selected.allows("v-4", "eth9"));
    assert!(selected.allows("v-4", "eth4"));
    assert!(!selected.allows("v-6", "wg0"));
    assert!(!selected.expects_rates("v-6"));
    assert!(selected.allows("v-7", "eth7"));
    assert!(selected.expects_rates("v-7"));
    assert!(!selected.allows("v-7", "anything"));
    assert!(!selected.allows("v-8", "anything"));
    assert!(!selected.expects_rates("v-8"));

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
    invalid_reference.value_raw = "tunnel:wg0".to_string();
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
        &[],
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
fn no_reset_traffic_uses_epoch_lower_bound_and_preserves_counter_reset_gaps() {
    let now = Utc.timestamp_opt(2_000_000_000, 0).single().unwrap();
    let rules = vec![parsed_rule_for(
        "edge-a",
        VPS_RULE_KEY_TRAFFIC_RESET_DAY,
        "-1",
    )];
    assert_eq!(
        traffic_cycle_starts_for_clients(["edge-a"], &rules, now),
        vec![("edge-a".to_string(), 0)]
    );

    let mut samples = vec![
        sample(60, 100, 200, 0, 0),
        sample(31_536_000, 160, 260, 0, 0),
        sample(31_536_060, 10, 300, 1, 0),
        sample(63_072_000, 40, 340, 1, 0),
        sample(63_072_060, 50, 5, 1, 1),
        sample(94_608_000, 70, 25, 1, 1),
        sample(94_608_060, 5, 30, 2, 1),
        sample(126_144_000, 15, 50, 2, 1),
    ];
    samples[0].sample_source = "vnstat_import:test".to_string();
    samples[1].sample_source = "vnstat_import:test".to_string();
    samples[2].sample_source = "interface_counters".to_string();
    samples.reverse();

    let usage = aggregate_memory_traffic_counter_usage(
        &samples,
        &[],
        &[TrafficStreamRequest {
            client_id: "edge-a".to_string(),
            source_kind: "host".to_string(),
            interface: "eth0".to_string(),
            cycle_start_unix: NO_RESET_TRAFFIC_START_UNIX,
        }],
        126_144_000,
    );
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0].cycle_rx, 130);
    assert_eq!(usage[0].cycle_tx, 185);
    assert_eq!(usage[0].latest_rx, 15);
    assert_eq!(usage[0].latest_tx, 50);
    assert_eq!(usage[0].last_sample_unix, 126_144_000);
    assert_eq!(usage[0].rx_counter_epochs_seen, 2);
    assert_eq!(usage[0].tx_counter_epochs_seen, 2);
}

#[test]
fn no_reset_accounting_has_no_cycle_boundaries_while_monthly_and_missing_semantics_remain() {
    let now = Utc.with_ymd_and_hms(2026, 8, 9, 12, 0, 0).single().unwrap();
    let current_usage = usage("eth0", now.timestamp());
    let mut no_reset_rules = traffic_rules("eth0");
    no_reset_rules.retain(|rule| rule.key != VPS_RULE_KEY_TRAFFIC_RESET_DAY);
    no_reset_rules.push(parsed_rule_for(
        "edge-a",
        VPS_RULE_KEY_TRAFFIC_RESET_DAY,
        "-1",
    ));
    let no_reset = traffic_accounting_for_client(
        "edge-a",
        &no_reset_rules,
        std::slice::from_ref(&current_usage),
        now,
    );
    assert_eq!(no_reset.reset_day, Some(-1));
    assert_eq!(no_reset.cycle_start, None);
    assert_eq!(no_reset.cycle_end, None);

    let monthly =
        traffic_accounting_for_client("edge-a", &traffic_rules("eth0"), &[current_usage], now);
    assert_eq!(monthly.reset_day, Some(1));
    assert_eq!(
        monthly.cycle_start.as_deref(),
        Some("2026-08-01T00:00:00+00:00")
    );
    assert_eq!(
        monthly.cycle_end.as_deref(),
        Some("2026-09-01T00:00:00+00:00")
    );

    let mut missing_rules = traffic_rules("eth0");
    missing_rules.retain(|rule| rule.key != VPS_RULE_KEY_TRAFFIC_RESET_DAY);
    let missing = traffic_accounting_for_client(
        "edge-a",
        &missing_rules,
        &[usage("eth0", now.timestamp())],
        now,
    );
    assert_eq!(missing.reset_day, None);
    assert_eq!(
        missing.cycle_start.as_deref(),
        Some("2026-08-01T00:00:00+00:00")
    );
    assert_eq!(
        missing.cycle_end.as_deref(),
        Some("2026-09-01T00:00:00+00:00")
    );
    assert!(missing
        .incomplete_reasons
        .iter()
        .any(|reason| reason == "traffic.reset_day missing"));

    let february = Utc
        .with_ymd_and_hms(2026, 2, 15, 12, 0, 0)
        .single()
        .unwrap();
    let mut day_31_rules = traffic_rules("eth0");
    day_31_rules.retain(|rule| rule.key != VPS_RULE_KEY_TRAFFIC_RESET_DAY);
    day_31_rules.push(parsed_rule_for(
        "edge-a",
        VPS_RULE_KEY_TRAFFIC_RESET_DAY,
        "31",
    ));
    let day_31 = traffic_accounting_for_client(
        "edge-a",
        &day_31_rules,
        &[usage("eth0", february.timestamp())],
        february,
    );
    assert_eq!(
        day_31.cycle_start.as_deref(),
        Some("2026-01-31T00:00:00+00:00")
    );
    assert_eq!(
        day_31.cycle_end.as_deref(),
        Some("2026-02-28T00:00:00+00:00")
    );
}

#[test]
fn traffic_history_ignores_resets_in_unselected_direction() {
    let samples = vec![sample(90, 100, 200, 0, 0), sample(120, 120, 10, 0, 1)];
    let rx = aggregate_memory_traffic_history(
        samples.clone(),
        &[],
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
        &[],
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
        &[],
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
fn retained_traffic_history_returns_native_bucket_without_interpolation() {
    let history = aggregate_memory_traffic_history(
        vec![sample(7_200, 100, 200, 0, 0), sample(7_260, 110, 220, 0, 0)],
        &[traffic_rollup(0, 3_600, 300, 600, 4, 1)],
        &[TrafficHistoryStream {
            source_kind: "host".to_string(),
            interface: "eth0".to_string(),
            direction_mask: 0b11,
        }],
        0,
        7_300,
        60,
    );
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].bucket_start, "0");
    assert_eq!(history[0].bucket_secs, 3_600);
    assert_eq!(history[0].sample_count, 4);
    assert_eq!(history[0].reset_count, 1);
    assert_eq!(history[0].rx_bytes, Some(300));
    assert_eq!(history[0].tx_bytes, Some(600));
    assert_eq!(history[1].bucket_secs, 60);
    assert_eq!(history[1].rx_bytes, Some(10));
    assert_eq!(history[1].tx_bytes, Some(20));
}

#[test]
fn raw_traffic_history_adds_exact_imports_without_cross_source_transitions() {
    let live_samples = vec![sample(730, 100, 200, 0, 0), sample(745, 130, 240, 0, 0)];
    let mut imported_samples = vec![
        sample(540, 0, 0, 0, 0),
        sample(600, 10, 20, 0, 0),
        sample(660, 25, 50, 0, 0),
    ];
    for sample in &mut imported_samples {
        sample.sample_source = "vnstat_import:test".to_string();
    }
    // Durable live rows are deliberately different from the raw observations:
    // source=raw must not substitute or add them.
    imported_samples.extend([
        sample(700, 1_000, 2_000, 1, 1),
        sample(760, 1_500, 2_600, 1, 1),
    ]);

    let history = aggregate_memory_raw_traffic_history(
        live_samples,
        &imported_samples,
        &[TrafficHistoryStream {
            source_kind: "host".to_string(),
            interface: "eth0".to_string(),
            direction_mask: 0b11,
        }],
        600,
        899,
        300,
    );

    assert_eq!(history.len(), 1);
    assert_eq!(history[0].sample_count, 3);
    assert_eq!(history[0].reset_count, 0);
    assert_eq!(history[0].rx_bytes, Some(55));
    assert_eq!(history[0].tx_bytes, Some(90));
    assert_eq!(history[0].total_bytes, Some(145));
}

#[test]
fn retained_traffic_history_masks_unselected_direction_counts_and_bytes() {
    let mut rollup = traffic_rollup(0, 3_600, 300, 600, 4, 1);
    rollup.rx_valid_count = 4;
    rollup.tx_valid_count = 0;
    rollup.rx_reset_count = 0;
    rollup.tx_reset_count = 1;

    let rx = aggregate_memory_traffic_history(
        Vec::new(),
        &[rollup.clone()],
        &[TrafficHistoryStream {
            source_kind: "host".to_string(),
            interface: "eth0".to_string(),
            direction_mask: 0b01,
        }],
        0,
        3_599,
        60,
    );
    assert_eq!(rx[0].sample_count, 4);
    assert_eq!(rx[0].reset_count, 0);
    assert_eq!(rx[0].rx_bytes, Some(300));
    assert_eq!(rx[0].tx_bytes, Some(0));

    let tx = aggregate_memory_traffic_history(
        Vec::new(),
        &[rollup],
        &[TrafficHistoryStream {
            source_kind: "host".to_string(),
            interface: "eth0".to_string(),
            direction_mask: 0b10,
        }],
        0,
        3_599,
        60,
    );
    assert_eq!(tx[0].sample_count, 0);
    assert_eq!(tx[0].reset_count, 1);
    assert_eq!(tx[0].rx_bytes, None);
    assert_eq!(tx[0].tx_bytes, None);
}

#[test]
fn no_reset_accounting_adds_retained_ledger_without_changing_latest_counter() {
    let usage = aggregate_memory_traffic_counter_usage(
        &[sample(7_200, 100, 200, 0, 0), sample(7_260, 110, 220, 0, 0)],
        &[traffic_rollup(0, 3_600, 300, 600, 4, 1)],
        &[TrafficStreamRequest {
            client_id: "edge-a".to_string(),
            source_kind: "host".to_string(),
            interface: "eth0".to_string(),
            cycle_start_unix: NO_RESET_TRAFFIC_START_UNIX,
        }],
        7_260,
    );
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0].cycle_rx, 310);
    assert_eq!(usage[0].cycle_tx, 620);
    assert_eq!(usage[0].latest_rx, 110);
    assert_eq!(usage[0].latest_tx, 220);
    assert_eq!(usage[0].rx_counter_epochs_seen, 2);
    assert_eq!(usage[0].tx_counter_epochs_seen, 2);
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
fn directional_traffic_keeps_diagnostics_without_changing_billing_totals() {
    let now = Utc.timestamp_opt(2_000_000_000, 0).single().unwrap();
    let usage = usage("eth0", now.timestamp());

    let tx_only = traffic_accounting_for_client(
        "edge-a",
        &traffic_rules("eth0+tx"),
        std::slice::from_ref(&usage),
        now,
    );
    assert_eq!(tx_only.rx_bytes, 0);
    assert_eq!(tx_only.tx_bytes, 200);
    assert_eq!(tx_only.total_bytes, 200);
    assert_eq!(tx_only.diagnostic_rx_bytes, 100);
    assert_eq!(tx_only.diagnostic_tx_bytes, 200);
    assert_eq!(tx_only.diagnostic_total_bytes, 300);

    let split =
        traffic_accounting_for_client("edge-a", &traffic_rules("eth0+rx,eth0+tx"), &[usage], now);
    assert_eq!(split.rx_bytes, 100);
    assert_eq!(split.tx_bytes, 200);
    assert_eq!(split.total_bytes, 300);
    assert_eq!(split.diagnostic_rx_bytes, 100);
    assert_eq!(split.diagnostic_tx_bytes, 200);
    assert_eq!(split.diagnostic_total_bytes, 300);
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
        stored_value_raw: None,
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

fn traffic_rollup(
    bucket_start_unix: i64,
    bucket_secs: i32,
    rx_bytes: i64,
    tx_bytes: i64,
    any_valid_count: i32,
    any_reset_count: i32,
) -> TrafficCounterRollupRecord {
    TrafficCounterRollupRecord {
        client_id: "edge-a".to_string(),
        source_kind: "host".to_string(),
        interface: "eth0".to_string(),
        origin_kind: "live".to_string(),
        bucket_start: bucket_start_unix.to_string(),
        bucket_start_unix,
        bucket_secs,
        rx_bytes,
        tx_bytes,
        rx_valid_count: any_valid_count,
        tx_valid_count: any_valid_count,
        any_valid_count,
        rx_reset_count: any_reset_count,
        tx_reset_count: any_reset_count,
        any_reset_count,
        first_observed_unix: bucket_start_unix,
        latest_observed_unix: bucket_start_unix + i64::from(bucket_secs) - 1,
    }
}
