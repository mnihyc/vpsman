use std::collections::{HashMap, HashSet};

use chrono::{TimeZone, Utc};
use serde_json::json;
use vpsman_common::{AgentCapabilitySnapshot, ExpressionTruth};

use crate::{
    model::AgentView,
    model_alert_policies::{
        AlertPolicyCorrelationMode, AlertPolicyMetaCondition, AlertPolicyRuleKind,
    },
    repository::{MemoryState, Repository},
};

use super::{
    aggregate_memory_raw_traffic_history, aggregate_memory_traffic_counter_usage,
    aggregate_memory_traffic_history, claim_traffic_selector_directions, derive_cycle_usage,
    evaluate_rule_for_client, metric_policy_expression_truth, network_rate_selector_spec_from_rule,
    next_policy_rule_state, parse_billing_cycle, parse_billing_price, parse_byte_size,
    parse_network_rate_interfaces, parse_persisted_traffic_selector_list, parse_port_speed,
    parse_traffic_selector, parse_traffic_selector_list, parse_vps_rule_value,
    policy_alert_for_evaluation, policy_alert_resolution_timestamp,
    policy_alert_resolved_webhook_event, policy_identifier_value, policy_state_is_alert_eligible,
    policy_vps_rule_inputs, policy_webhook_repair_is_recent,
    resolve_memory_policy_alerts_for_rules, resolve_memory_policy_states_outside_scope,
    resolve_network_rate_interface_selection, resolve_policy_alert, traffic_accounting_for_client,
    traffic_accounting_for_client_with_selector_override, traffic_cycle_starts_for_clients,
    validate_billing_rule_group, NetworkRateSelectorReference, NetworkRateSelectorSpec,
    PolicyEvaluation, PolicyGroupRecord, PolicyRuleRecord, PolicyRuleRequest,
    PolicyRuleStateRecord, TelemetryRollupView, TrafficAccountingQuery, TrafficCounterRollupRecord,
    TrafficCounterSampleRecord, TrafficCounterStreamUsage, TrafficHistoryStream,
    TrafficStreamRequest, VpsRuleValueRecord, NO_RESET_TRAFFIC_START_UNIX,
    VPS_RULE_KEY_NETWORK_RATE_INTERFACES, VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL,
    VPS_RULE_KEY_TRAFFIC_RESET_DAY, VPS_RULE_KEY_TRAFFIC_SELECTORS,
};

#[test]
fn metric_policy_runtime_uses_arithmetic_kleene_truth() {
    let evidence = json!({
        "cpu": {"utilization_ratio": 0.91},
        "memory": {"available_ratio": null},
        "traffic": {"cycle_percent": 82.0}
    });
    assert_eq!(
        metric_policy_expression_truth(
            "cpu.utilization_ratio * 100 >= 90 && traffic.cycle_percent > 80",
            &evidence,
            true,
        )
        .unwrap(),
        ExpressionTruth::True
    );
    assert_eq!(
        metric_policy_expression_truth(
            "cpu.utilization_ratio > 0.95 || memory.available_ratio < 0.1",
            &evidence,
            true,
        )
        .unwrap(),
        ExpressionTruth::Unknown
    );
    assert_eq!(
        metric_policy_expression_truth(
            "cpu.utilization_ratio < 0.95 || memory.available_ratio < 0.1",
            &evidence,
            true,
        )
        .unwrap(),
        ExpressionTruth::True
    );
    assert_eq!(
        metric_policy_expression_truth("cpu.utilization_ratio > 0.5", &evidence, false).unwrap(),
        ExpressionTruth::Unknown
    );
}

#[test]
fn api_vps_rule_constants_follow_the_common_registry() {
    let mut api_keys = vec![
        crate::model_alert_policies::VPS_RULE_KEY_BILLING_CYCLE,
        crate::model_alert_policies::VPS_RULE_KEY_BILLING_PRICE,
        crate::model_alert_policies::VPS_RULE_KEY_NETWORK_PORT_SPEED,
        crate::model_alert_policies::VPS_RULE_KEY_NETWORK_RATE_INTERFACES,
        crate::model_alert_policies::VPS_RULE_KEY_PRODUCT_NAME,
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
    let padded_cycle = parse_billing_cycle("06-05").unwrap();
    assert_eq!(padded_cycle.raw, "06-05");
    assert_eq!(padded_cycle.display, "06-05");
    assert_eq!(padded_cycle.json["display"], "06-05");
    assert_eq!(parse_billing_cycle("6-5").unwrap(), padded_cycle);
    validate_billing_rule_group(Some("29.90 CNY/m"), Some("7")).unwrap();
    validate_billing_rule_group(Some("60.00 EUR/hy"), Some("06-05")).unwrap();
    assert!(
        validate_billing_rule_group(Some("29.90 CNY/m"), Some("6-5"))
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

    let aliases = parse_network_rate_interfaces("eth2+tx+rx, eth3+rx/tx").unwrap();
    assert_eq!(aliases.raw, "eth2,eth3+tx/rx");
    assert_eq!(aliases.json["selectors"][0]["direction"], "total");
    assert_eq!(aliases.json["selectors"][1]["direction"], "tx/rx");

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
fn historical_counter_resets_remain_evidence_without_degrading_current_accounting() {
    let now = Utc.timestamp_opt(2_000_000_000, 0).single().unwrap();
    let mut reset_usage = usage("eth0", now.timestamp());
    reset_usage.rx_counter_epochs_seen = 2;
    reset_usage.tx_counter_epochs_seen = 2;

    let accounting =
        traffic_accounting_for_client("edge-a", &traffic_rules("eth0"), &[reset_usage], now);

    assert_eq!(accounting.counter_epochs_seen, 2);
    assert_eq!(accounting.state, "ok");
    assert!(accounting.incomplete_reasons.is_empty());
    assert_eq!(accounting.selector_breakdown[0].state, "ok");
    assert!(accounting.selector_breakdown[0]
        .incomplete_reasons
        .is_empty());
}

#[test]
fn traffic_selectors_reject_overlapping_directions() {
    let error = parse_traffic_selector_list("eth0,eth0+rx").unwrap_err();
    assert!(error
        .to_string()
        .contains("traffic_selector_direction_overlap"));

    let selectors = parse_traffic_selector_list("eth0+rx,eth0+tx").unwrap();
    assert_eq!(selectors.len(), 2);

    for input in ["eth0+tx/rx,eth0+rx", "eth0+rx/tx,eth0+tx"] {
        let error = parse_traffic_selector_list(input).unwrap_err();
        assert!(error
            .to_string()
            .contains("traffic_selector_direction_overlap"));
    }

    let max = parse_traffic_selector("eth0+rx/tx").unwrap();
    assert_eq!(max.direction, "tx/rx");
    assert_eq!(max.canonical, "eth0+tx/rx");
    for input in ["eth0+rx+tx", "eth0+tx+rx"] {
        let total = parse_traffic_selector(input).unwrap();
        assert_eq!(total.direction, "total");
        assert_eq!(total.canonical, "eth0");
    }

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
    let max = parse_traffic_selector("eth1+tx/rx").unwrap();
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
    assert_eq!(
        claim_traffic_selector_directions(&mut counted, &max),
        (true, true)
    );
    let max_rx = parse_traffic_selector("eth1+rx").unwrap();
    assert_eq!(
        claim_traffic_selector_directions(&mut counted, &max_rx),
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
fn max_traffic_keeps_directional_values_but_uses_per_selector_maximum_totals() {
    let now = Utc.timestamp_opt(2_000_000_000, 0).single().unwrap();
    let eth0 = usage("eth0", now.timestamp());

    let tx_wins = traffic_accounting_for_client(
        "edge-a",
        &traffic_rules("eth0+rx/tx"),
        std::slice::from_ref(&eth0),
        now,
    );
    assert_eq!(tx_wins.selectors, vec!["eth0+tx/rx"]);
    assert_eq!(tx_wins.rx_bytes, 100);
    assert_eq!(tx_wins.tx_bytes, 200);
    assert_eq!(tx_wins.total_bytes, 200);
    assert_eq!(tx_wins.latest_rx_bytes, 1_000);
    assert_eq!(tx_wins.latest_tx_bytes, 2_000);
    assert_eq!(tx_wins.latest_total_bytes, 2_000);
    assert_eq!(tx_wins.diagnostic_total_bytes, 300);
    assert_eq!(tx_wins.selector_breakdown[0].direction, "tx/rx");
    assert_eq!(tx_wins.selector_breakdown[0].cycle_rx_bytes, 100);
    assert_eq!(tx_wins.selector_breakdown[0].cycle_tx_bytes, 200);
    assert_eq!(tx_wins.selector_breakdown[0].cycle_total_bytes, 200);

    let mut eth1 = usage("eth1", now.timestamp());
    eth1.cycle_rx = 400;
    eth1.cycle_tx = 50;
    eth1.latest_rx = 4_000;
    eth1.latest_tx = 500;
    let multiple = traffic_accounting_for_client(
        "edge-a",
        &traffic_rules("eth0+tx/rx,eth1+rx/tx"),
        &[eth0.clone(), eth1],
        now,
    );
    assert_eq!(multiple.rx_bytes, 500);
    assert_eq!(multiple.tx_bytes, 250);
    assert_eq!(multiple.total_bytes, 600);
    assert_eq!(multiple.latest_rx_bytes, 5_000);
    assert_eq!(multiple.latest_tx_bytes, 2_500);
    assert_eq!(multiple.latest_total_bytes, 6_000);
    assert_eq!(multiple.diagnostic_total_bytes, 750);
    assert_eq!(multiple.selector_breakdown[0].cycle_total_bytes, 200);
    assert_eq!(multiple.selector_breakdown[1].cycle_total_bytes, 400);

    let mut tie = eth0.clone();
    tie.cycle_tx = tie.cycle_rx;
    tie.latest_tx = tie.latest_rx;
    let tied = traffic_accounting_for_client(
        "edge-a",
        &traffic_rules("eth0+tx/rx"),
        std::slice::from_ref(&tie),
        now,
    );
    assert_eq!(tied.total_bytes, 100);
    assert_eq!(tied.latest_total_bytes, 1_000);

    let override_accounting = traffic_accounting_for_client_with_selector_override(
        "edge-a",
        &traffic_rules("eth0"),
        &[eth0],
        now,
        Some("eth0+rx/tx"),
    );
    assert_eq!(override_accounting.selectors, vec!["eth0+tx/rx"]);
    assert_eq!(override_accounting.total_bytes, 200);
    assert_eq!(override_accounting.latest_total_bytes, 2_000);
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
fn cpu_utilization_policy_uses_rollup_max_and_missing_is_incomplete() {
    let rollup = policy_resource_rollup();
    let rule = PolicyRuleRequest {
        id: None,
        name: "cpu busy".to_string(),
        enabled: true,
        rule_kind: AlertPolicyRuleKind::Metric,
        evidence_source: "telemetry.combined".to_string(),
        correlation_mode: AlertPolicyCorrelationMode::NaturalKey,
        traffic_selector: None,
        trigger_condition_expression: "cpu.utilization_ratio >= 0.75".to_string(),
        trigger_meta_condition: None,
        resolve_condition_expression: None,
        resolve_meta_condition: None,
        severity: "warning".to_string(),
        category: "resource".to_string(),
        title_template: "CPU busy".to_string(),
        detail_template: "CPU utilization is high".to_string(),
    };
    let evaluation = evaluate_rule_for_client(&rule, None, Some(&rollup));

    assert!(evaluation.condition_true);
    assert!(!evaluation.incomplete);
    assert_eq!(evaluation.actual_value, Some(0.82));
    assert_eq!(evaluation.threshold_value, Some(0.75));

    let mut missing_utilization = rollup.clone();
    missing_utilization.cpu_usage_sample_count = 0;
    missing_utilization.cpu_usage_avg = None;
    missing_utilization.cpu_usage_max = None;
    let incomplete = evaluate_rule_for_client(&rule, None, Some(&missing_utilization));
    assert!(!incomplete.condition_true);
    assert!(incomplete.incomplete);
    assert_eq!(
        incomplete.incomplete_reasons,
        vec!["cpu.utilization_ratio missing".to_string()]
    );

    let mut load_incomplete = Vec::new();
    let raw_load = policy_identifier_value("cpu.load_1", None, Some(&rollup), &mut load_incomplete);
    assert_eq!(raw_load, Some(0.1));
    assert_eq!(
        policy_identifier_value(
            "cpu.load_saturation",
            None,
            Some(&rollup),
            &mut load_incomplete,
        ),
        Some(0.025)
    );
    assert!(load_incomplete.is_empty());

    let mut no_cores = rollup;
    no_cores.cpu_cores_max = 0;
    let mut no_cores_incomplete = Vec::new();
    assert_eq!(
        policy_identifier_value(
            "cpu.load_saturation",
            None,
            Some(&no_cores),
            &mut no_cores_incomplete,
        ),
        None
    );
    assert_eq!(
        no_cores_incomplete,
        vec!["cpu.load_saturation missing".to_string()]
    );
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
fn unknown_policy_evidence_pauses_dwell_without_resolving_or_rearming() {
    let mut rule = policy_rule(1);
    rule.trigger_meta_condition = Some(AlertPolicyMetaCondition::Sustained { seconds: 300 });
    let started_at = Utc.timestamp_opt(2_000_000_000, 0).single().unwrap();
    let started = next_policy_rule_state(
        &rule,
        "edge-a",
        &true_policy_evaluation(),
        None,
        0,
        started_at,
    )
    .unwrap();
    assert_eq!(started.trigger_generation, 1);
    assert!(!started.window_satisfied);

    let confirmed_at = started_at + chrono::Duration::seconds(100);
    let confirmed = next_policy_rule_state(
        &rule,
        "edge-a",
        &true_policy_evaluation(),
        Some(&started),
        1,
        confirmed_at,
    )
    .unwrap();
    assert!(!confirmed.window_satisfied);

    let unknown_at = started_at + chrono::Duration::seconds(200);
    let unknown = next_policy_rule_state(
        &rule,
        "edge-a",
        &unknown_policy_evaluation(),
        Some(&confirmed),
        1,
        unknown_at,
    )
    .unwrap();
    assert!(unknown.condition_true);
    assert!(unknown.incomplete);
    assert_eq!(unknown.trigger_generation, 1);
    assert!(!unknown.window_satisfied);

    let resumed_at = unknown_at + chrono::Duration::seconds(200);
    let resumed = next_policy_rule_state(
        &rule,
        "edge-a",
        &true_policy_evaluation(),
        Some(&unknown),
        1,
        resumed_at,
    )
    .unwrap();
    assert_eq!(resumed.trigger_generation, 1);
    assert!(!resumed.window_satisfied);

    let satisfied = next_policy_rule_state(
        &rule,
        "edge-a",
        &true_policy_evaluation(),
        Some(&resumed),
        1,
        resumed_at + chrono::Duration::seconds(200),
    )
    .unwrap();
    assert!(satisfied.window_satisfied);

    let recovered = next_policy_rule_state(
        &rule,
        "edge-a",
        &PolicyEvaluation {
            condition_true: false,
            incomplete: false,
            incomplete_reasons: Vec::new(),
            actual_value: Some(0.0),
            threshold_value: Some(1.0),
            category: "resource".to_string(),
            payload: json!({}),
        },
        Some(&satisfied),
        1,
        resumed_at + chrono::Duration::seconds(360),
    )
    .unwrap();
    assert!(!recovered.condition_true);
    assert_eq!(recovered.trigger_generation, 1);

    let recurred = next_policy_rule_state(
        &rule,
        "edge-a",
        &true_policy_evaluation(),
        Some(&recovered),
        1,
        resumed_at + chrono::Duration::seconds(420),
    )
    .unwrap();
    assert_eq!(recurred.trigger_generation, 2);
    assert!(!recurred.window_satisfied);
}

#[test]
fn repeated_fractional_unknown_intervals_do_not_leak_into_policy_dwell() {
    let mut rule = policy_rule(1);
    rule.trigger_meta_condition = Some(AlertPolicyMetaCondition::Sustained { seconds: 300 });
    let started_at = Utc
        .timestamp_opt(2_000_000_000, 100_000_000)
        .single()
        .unwrap();
    let mut state = next_policy_rule_state(
        &rule,
        "edge-a",
        &true_policy_evaluation(),
        None,
        0,
        started_at,
    )
    .unwrap();

    for interval in 1..=334 {
        state = next_policy_rule_state(
            &rule,
            "edge-a",
            &unknown_policy_evaluation(),
            Some(&state),
            1,
            started_at + chrono::Duration::milliseconds(900 * interval),
        )
        .unwrap();
        assert!(!state.window_satisfied);
    }

    let resumed_at = started_at + chrono::Duration::milliseconds(900 * 335);
    state = next_policy_rule_state(
        &rule,
        "edge-a",
        &true_policy_evaluation(),
        Some(&state),
        1,
        resumed_at,
    )
    .unwrap();
    assert!(
        !state.window_satisfied,
        "fractional Unknown intervals must pause rather than accrue dwell"
    );

    let almost_satisfied = next_policy_rule_state(
        &rule,
        "edge-a",
        &true_policy_evaluation(),
        Some(&state),
        1,
        resumed_at + chrono::Duration::seconds(299),
    )
    .unwrap();
    assert!(!almost_satisfied.window_satisfied);
    let satisfied = next_policy_rule_state(
        &rule,
        "edge-a",
        &true_policy_evaluation(),
        Some(&almost_satisfied),
        1,
        resumed_at + chrono::Duration::seconds(300),
    )
    .unwrap();
    assert!(satisfied.window_satisfied);
}

#[tokio::test]
async fn stale_matching_snapshot_cannot_persist_after_current_selector_exit() {
    let memory = MemoryState::default();
    let repo = Repository::Memory(memory.clone());
    let group = policy_group("tag:monitored");
    memory.policy_groups.write().await.push(group.clone());
    memory.agents.write().await.push(policy_agent(Vec::new()));
    let stale_matching_agent = policy_agent(vec!["monitored".to_string()]);
    let now = Utc.timestamp_opt(2_000_000_000, 0).single().unwrap();

    assert!(!repo
        .persist_policy_evaluation(
            &group,
            &group.rules[0],
            &stale_matching_agent,
            &[],
            true_policy_evaluation(),
            now,
        )
        .await
        .unwrap());
    assert!(memory.policy_rule_states.read().await.is_empty());
    assert!(memory.policy_alerts.read().await.is_empty());
    assert!(memory.webhook_events.read().await.is_empty());
}

#[tokio::test]
async fn stale_vps_rule_inputs_cannot_persist_a_policy_evaluation() {
    let memory = MemoryState::default();
    let repo = Repository::Memory(memory.clone());
    let group = policy_group("status:online");
    let evaluated_rules = vec![parsed_rule_for(
        "edge-a",
        VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL,
        "1GB",
    )];
    let evaluated_inputs = policy_vps_rule_inputs(&evaluated_rules, "edge-a");
    memory.policy_groups.write().await.push(group.clone());
    memory.agents.write().await.push(policy_agent(Vec::new()));
    memory.vps_rule_values.write().await.push(parsed_rule_for(
        "edge-a",
        VPS_RULE_KEY_TRAFFIC_QUOTA_TOTAL,
        "2GB",
    ));
    let now = Utc.timestamp_opt(2_000_000_000, 0).single().unwrap();

    assert!(!repo
        .persist_policy_evaluation(
            &group,
            &group.rules[0],
            &policy_agent(Vec::new()),
            &evaluated_inputs,
            true_policy_evaluation(),
            now,
        )
        .await
        .unwrap());
    assert!(memory.policy_rule_states.read().await.is_empty());
    assert!(memory.policy_alerts.read().await.is_empty());
    assert!(memory.webhook_events.read().await.is_empty());
}

#[tokio::test]
async fn stale_nonmatching_snapshot_cannot_resolve_after_current_selector_entry() {
    let memory = MemoryState::default();
    let group = policy_group("tag:monitored");
    let agent = policy_agent(vec!["monitored".to_string()]);
    let now = Utc.timestamp_opt(2_000_000_000, 0).single().unwrap();
    let state = policy_state(1, true, &now.to_rfc3339());
    let alert = policy_alert_for_evaluation(
        &group,
        &group.rules[0],
        &agent,
        &state,
        &true_policy_evaluation(),
        &now.to_rfc3339(),
    );
    memory.policy_groups.write().await.push(group.clone());
    memory.agents.write().await.push(agent);
    memory.policy_rule_states.write().await.push(state);
    memory.policy_alerts.write().await.push(alert);

    // The evaluator's earlier match set was empty; scope reconciliation must
    // derive membership again from current agent tags instead of trusting it.
    resolve_memory_policy_states_outside_scope(&memory, &group, &group.rules[0])
        .await
        .unwrap();

    assert_eq!(memory.policy_rule_states.read().await.len(), 1);
    let alerts = memory.policy_alerts.read().await;
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].lifecycle_state, "triggered");
    assert!(alerts[0].resolved_at.is_none());
    assert_eq!(
        alerts[0].payload.pointer("/alert/trigger_generation"),
        Some(&json!(1))
    );
    let mut resolved = alerts[0].clone();
    drop(alerts);
    let resolved_at = now + chrono::Duration::seconds(1);
    resolve_policy_alert(&mut resolved, resolved_at, None, "condition_recovered").unwrap();
    assert_eq!(
        policy_alert_resolved_webhook_event(&resolved)
            .payload
            .pointer("/alert/trigger_generation"),
        Some(&json!(1))
    );
    assert!(memory.webhook_events.read().await.is_empty());
}

#[tokio::test]
async fn memory_scope_exit_resolution_and_event_follow_locked_episode_evidence() {
    let memory = MemoryState::default();
    let group = policy_group("tag:monitored");
    let agent = policy_agent(Vec::new());
    let confirmed_at = Utc.timestamp_opt(4_102_444_800, 0).single().unwrap();
    let state_evaluated_at = confirmed_at + chrono::Duration::seconds(20);
    let state = policy_state(1, true, &state_evaluated_at.to_rfc3339());
    let alert = policy_alert_for_evaluation(
        &group,
        &group.rules[0],
        &agent,
        &state,
        &true_policy_evaluation(),
        &confirmed_at.to_rfc3339(),
    );
    memory.policy_groups.write().await.push(group.clone());
    memory.agents.write().await.push(agent);
    memory.policy_rule_states.write().await.push(state);
    memory.policy_alerts.write().await.push(alert);

    resolve_memory_policy_states_outside_scope(&memory, &group, &group.rules[0])
        .await
        .unwrap();

    assert!(memory.policy_rule_states.read().await.is_empty());
    let alerts = memory.policy_alerts.read().await;
    let resolved_at = policy_alert_resolution_timestamp(&alerts[0]).unwrap();
    assert!(resolved_at >= confirmed_at);
    assert!(resolved_at >= state_evaluated_at);
    assert_eq!(
        alerts[0].resolution_reason.as_deref(),
        Some("policy_scope_exited")
    );
    drop(alerts);

    let events = memory.webhook_events.read().await;
    assert_eq!(events.len(), 1);
    let event_occurred_at = super::parse_policy_lifecycle_timestamp(
        &events[0].occurred_at,
        "webhook event occurred_at",
    )
    .unwrap();
    let payload_occurred_at = super::parse_policy_lifecycle_timestamp(
        events[0]
            .payload
            .pointer("/event/occurred_at")
            .and_then(serde_json::Value::as_str)
            .unwrap(),
        "webhook payload event occurred_at",
    )
    .unwrap();
    assert_eq!(event_occurred_at, resolved_at);
    assert_eq!(payload_occurred_at, resolved_at);
}

#[tokio::test]
async fn memory_policy_change_resolution_clamps_each_episode_to_current_state() {
    let memory = MemoryState::default();
    let group = policy_group("status:online");
    let agent = policy_agent(Vec::new());
    let confirmed_at = Utc.timestamp_opt(4_102_444_800, 0).single().unwrap();
    let state_evaluated_at = confirmed_at + chrono::Duration::seconds(30);
    let state = policy_state(1, true, &state_evaluated_at.to_rfc3339());
    let alert = policy_alert_for_evaluation(
        &group,
        &group.rules[0],
        &agent,
        &state,
        &true_policy_evaluation(),
        &confirmed_at.to_rfc3339(),
    );
    memory.policy_rule_states.write().await.push(state);
    memory.policy_alerts.write().await.push(alert);

    resolve_memory_policy_alerts_for_rules(
        &memory,
        &HashSet::from([group.rules[0].id]),
        "policy_changed",
    )
    .await
    .unwrap();

    let alerts = memory.policy_alerts.read().await;
    let resolved_at = policy_alert_resolution_timestamp(&alerts[0]).unwrap();
    assert!(resolved_at >= confirmed_at);
    assert!(resolved_at >= state_evaluated_at);
    assert_eq!(
        alerts[0].resolution_reason.as_deref(),
        Some("policy_changed")
    );
    drop(alerts);
    let events = memory.webhook_events.read().await;
    let event_occurred_at = super::parse_policy_lifecycle_timestamp(
        &events[0].occurred_at,
        "webhook event occurred_at",
    )
    .unwrap();
    let payload_occurred_at = super::parse_policy_lifecycle_timestamp(
        events[0]
            .payload
            .pointer("/event/occurred_at")
            .and_then(serde_json::Value::as_str)
            .unwrap(),
        "webhook payload event occurred_at",
    )
    .unwrap();
    assert_eq!(event_occurred_at, resolved_at);
    assert_eq!(payload_occurred_at, resolved_at);
}

#[tokio::test]
async fn policy_summary_counts_only_confirmed_triggered_or_persisting_episodes() {
    let memory = MemoryState::default();
    let repo = Repository::Memory(memory.clone());
    let group = policy_group("tag:monitored");
    let agent = policy_agent(vec!["monitored".to_string()]);
    let now = Utc.timestamp_opt(2_000_000_000, 0).single().unwrap();
    let state = policy_state(1, true, &now.to_rfc3339());
    let mut alert = policy_alert_for_evaluation(
        &group,
        &group.rules[0],
        &agent,
        &state,
        &true_policy_evaluation(),
        &now.to_rfc3339(),
    );
    memory.policy_rule_states.write().await.push(state);

    let summarize = |repo: Repository, group: PolicyGroupRecord, agent: AgentView| async move {
        let mut groups = vec![group];
        repo.enrich_policy_group_summaries_with_rule_contexts(
            &mut groups,
            &[agent],
            &HashMap::new(),
        )
        .await
        .unwrap();
        groups.remove(0)
    };

    let summary = summarize(repo.clone(), group.clone(), agent.clone()).await;
    assert_eq!(summary.active_warning_count, 0);

    alert.lifecycle_state = "unknown".to_string();
    alert.last_confirmed_at = None;
    memory.policy_alerts.write().await.push(alert.clone());
    let summary = summarize(repo.clone(), group.clone(), agent.clone()).await;
    assert_eq!(summary.active_warning_count, 0);

    alert.last_confirmed_at = Some(now.to_rfc3339());
    memory.policy_alerts.write().await[0] = alert.clone();
    let summary = summarize(repo.clone(), group.clone(), agent.clone()).await;
    assert_eq!(summary.active_warning_count, 0);

    alert.lifecycle_state = "persisting".to_string();
    memory.policy_alerts.write().await[0] = alert;
    let summary = summarize(repo, group, agent).await;
    assert_eq!(summary.active_warning_count, 1);
}

#[tokio::test]
async fn snapshot_traffic_prelimit_matches_post_projection_sort_and_limit() {
    let memory = MemoryState::default();
    let repo = Repository::Memory(memory.clone());
    let mut agents = Vec::new();
    for client_id in ["z-last", "a-first", "m-middle"] {
        let mut agent = policy_agent(Vec::new());
        agent.id = client_id.to_string();
        agent.display_name = client_id.to_string();
        agents.push(agent);
    }
    *memory.agents.write().await = agents.clone();
    let rules = repo
        .list_all_vps_rules_for_clients(
            &agents
                .iter()
                .map(|agent| agent.id.clone())
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
    let legacy = repo
        .list_traffic_accounting(&TrafficAccountingQuery {
            selector_expression: None,
            client_id: None,
            state: None,
            limit: Some(2),
        })
        .await
        .unwrap();
    let prelimited = repo
        .list_snapshot_traffic_accounting_with_context(&agents, &rules, 2)
        .await
        .unwrap();

    assert_eq!(
        prelimited
            .iter()
            .map(|row| row.client_id.as_str())
            .collect::<Vec<_>>(),
        legacy
            .iter()
            .map(|row| row.client_id.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        prelimited
            .iter()
            .map(|row| (&row.selectors, &row.state, row.total_bytes))
            .collect::<Vec<_>>(),
        legacy
            .iter()
            .map(|row| (&row.selectors, &row.state, row.total_bytes))
            .collect::<Vec<_>>()
    );
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

fn policy_agent(tags: Vec<String>) -> AgentView {
    AgentView {
        id: "edge-a".to_string(),
        display_name: "Edge A".to_string(),
        status: "online".to_string(),
        tags,
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: AgentCapabilitySnapshot::default(),
    }
}

fn policy_group(selector_expression: &str) -> PolicyGroupRecord {
    PolicyGroupRecord {
        id: uuid::Uuid::from_u128(2),
        name: "resource policy".to_string(),
        enabled: true,
        selector_expression: selector_expression.to_string(),
        notes: None,
        matched_vps_count: 0,
        rule_count: 1,
        enabled_rule_count: 1,
        active_info_count: 0,
        active_warning_count: 0,
        active_critical_count: 0,
        incomplete_vps_count: 0,
        last_evaluated_at: None,
        rules: vec![policy_rule(1)],
        created_by: None,
        updated_by: None,
        created_at: "test".to_string(),
        updated_at: "test".to_string(),
    }
}

fn policy_rule(rule_version: i32) -> PolicyRuleRecord {
    PolicyRuleRecord {
        id: uuid::Uuid::from_u128(1),
        group_id: uuid::Uuid::from_u128(2),
        rule_version,
        sort_order: 0,
        name: "quota".to_string(),
        enabled: true,
        rule_kind: AlertPolicyRuleKind::Metric,
        evidence_source: "telemetry.combined".to_string(),
        correlation_mode: AlertPolicyCorrelationMode::NaturalKey,
        traffic_selector: None,
        trigger_condition_expression: "cpu.load_1 > 1".to_string(),
        trigger_meta_condition: None,
        resolve_condition_expression: None,
        resolve_meta_condition: None,
        severity: "warning".to_string(),
        category: "resource".to_string(),
        title_template: "Resource threshold reached".to_string(),
        detail_template: "CPU load is high".to_string(),
        system_seed_key: None,
        armed_after_evidence_seq: 0,
        armed_at: "test".to_string(),
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

fn unknown_policy_evaluation() -> PolicyEvaluation {
    PolicyEvaluation {
        condition_true: false,
        incomplete: true,
        incomplete_reasons: vec!["cpu.utilization_ratio missing".to_string()],
        actual_value: None,
        threshold_value: Some(0.75),
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

fn policy_resource_rollup() -> TelemetryRollupView {
    TelemetryRollupView {
        client_id: "edge-a".to_string(),
        bucket_start: "100".to_string(),
        bucket_secs: 60,
        sample_count: 3,
        cpu_usage_sample_count: 3,
        cpu_usage_avg: Some(0.2),
        cpu_usage_max: Some(0.82),
        cpu_cores_max: 4,
        cpu_load_1_avg: 0.05,
        cpu_load_1_max: 0.1,
        cpu_load_5_avg: 0.05,
        cpu_load_5_max: 0.1,
        cpu_load_15_avg: 0.05,
        cpu_load_15_max: 0.1,
        memory_total_bytes_max: 1000,
        memory_available_bytes_avg: 400,
        memory_available_bytes_min: 300,
        memory_used_ratio_avg: 0.6,
        memory_used_ratio_max: 0.7,
        swap_sample_count: 0,
        swap_total_bytes_max: None,
        swap_available_bytes_avg: None,
        swap_available_bytes_min: None,
        swap_used_ratio_avg: None,
        swap_used_ratio_max: None,
        disk_total_bytes_max: 2000,
        disk_available_bytes_avg: 1000,
        disk_available_bytes_min: 800,
        disk_used_ratio_avg: 0.5,
        disk_used_ratio_max: 0.6,
        network_rx_bytes_max: 0,
        network_tx_bytes_max: 0,
        connections_sample_count: 0,
        tcp_sockets_latest: None,
        udp_sockets_latest: None,
        connections_observed_at: None,
        latest_observed_at: "120".to_string(),
        updated_at: "121".to_string(),
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
