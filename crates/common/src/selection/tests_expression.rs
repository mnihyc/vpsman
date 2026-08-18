use super::*;
use serde_json::Value;

fn vps() -> ExpressionContext {
    ExpressionContext::for_vps(VpsMetadata {
        id: "edge-01".to_string(),
        display_name: "Edge One".to_string(),
        status: "online".to_string(),
        tags: vec![
            "edge".to_string(),
            "prod".to_string(),
            "provider:alpha".to_string(),
            "country:US".to_string(),
            "region:IAD".to_string(),
        ],
        last_seen_at: Some("2026-06-08T01:00:00Z".to_string()),
        internal_build_number: Some(42),
        extra: Some(serde_json::json!({"role": "ingress"})),
        ..VpsMetadata::default()
    })
}

fn matches(input: &str, context: &ExpressionContext) -> bool {
    parse_and_match_expression(input, context).unwrap()
}

fn rule_context(values: &[(&str, &str, Value)]) -> VpsRuleContext {
    let mut rules = VpsRuleContext::default();
    for (key, raw, json) in values {
        rules.insert(*key, *raw, json.clone());
    }
    rules
}

#[test]
fn partial_evidence_uses_strong_kleene_truth_tables() {
    let context = ExpressionContext::default()
        .with_json_root("evidence", serde_json::json!({"status": "failed"}));

    let truth = |input: &str| {
        let expression = parse_expression(input).unwrap().unwrap();
        expression_truth(&context, &expression)
    };

    assert_eq!(truth("evidence.status = failed"), ExpressionTruth::True);
    assert_eq!(truth("evidence.missing = value"), ExpressionTruth::Unknown);
    assert_eq!(
        truth("!(evidence.missing = value)"),
        ExpressionTruth::Unknown
    );
    assert_eq!(
        truth("evidence.status = failed || evidence.missing = value"),
        ExpressionTruth::True
    );
    assert_eq!(
        truth("evidence.status = ok || evidence.missing = value"),
        ExpressionTruth::Unknown
    );
    assert_eq!(
        truth("evidence.status = ok && evidence.missing = value"),
        ExpressionTruth::False
    );
    assert_eq!(
        truth("evidence.status = failed && evidence.missing = value"),
        ExpressionTruth::Unknown
    );
}

#[test]
fn missing_seeded_policy_fields_never_match_through_negation() {
    let context = ExpressionContext::default()
        .with_json_root("evidence", serde_json::json!({"status": "failed"}));
    let job_expression =
        parse_expression("evidence.status = failed && !(evidence.command_type in [\"*backup*\"])")
            .unwrap()
            .unwrap();
    let traffic_expression = parse_expression("evidence.traffic.status != ok")
        .unwrap()
        .unwrap();

    assert_eq!(
        expression_truth(&context, &job_expression),
        ExpressionTruth::Unknown
    );
    assert_eq!(
        expression_truth(&context, &traffic_expression),
        ExpressionTruth::Unknown
    );
}

fn fixture_context(value: &Value) -> ExpressionContext {
    let vps_value = value.get("vps").expect("fixture vps");
    let tags = vps_value
        .get("tags")
        .and_then(Value::as_array)
        .expect("fixture tags")
        .iter()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let mut context = ExpressionContext::for_vps(VpsMetadata {
        id: vps_value
            .get("id")
            .and_then(Value::as_str)
            .expect("fixture id")
            .to_string(),
        display_name: vps_value
            .get("display_name")
            .and_then(Value::as_str)
            .expect("fixture display name")
            .to_string(),
        status: vps_value
            .get("status")
            .and_then(Value::as_str)
            .expect("fixture status")
            .to_string(),
        tags,
        last_seen_at: vps_value
            .get("last_seen_at")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        internal_build_number: vps_value
            .get("internal_build_number")
            .and_then(Value::as_u64),
        extra: Some(vps_value.clone()),
        ..VpsMetadata::default()
    });
    for root in ["job", "schedule", "alert", "server", "telemetry"] {
        if let Some(root_value) = value.get(root) {
            context = context.with_json_root(root, root_value.clone());
        }
    }
    if let Some(events) = value.get("event_predicates").and_then(Value::as_array) {
        for event in events.iter().filter_map(Value::as_str) {
            context = context.with_event_predicate(event);
        }
    }
    context
}

#[test]
fn parser_honors_precedence_implicit_and_and_not() {
    let context = vps();
    assert!(matches("status:online tag:edge || provider:beta", &context));
    assert!(matches(
        "status:online && !(provider:beta || tag:test)",
        &context
    ));
    assert!(!matches("~status:online || tag:test", &context));
}

#[test]
fn equality_inequality_and_aliases_match_inventory_fields() {
    let context = vps();
    assert!(matches(r#"status = "online""#, &context));
    assert!(matches("vps.status != stale", &context));
    assert!(matches(
        "provider:alpha && country:US && region:IAD",
        &context
    ));
    assert!(!matches("region:US", &context));
    assert!(matches("vps.provider = alpha", &context));
    assert!(matches(
        "role:edge",
        &ExpressionContext::for_vps(VpsMetadata::new(
            "edge-02",
            "Edge Two",
            "online",
            vec!["role:edge".to_string()],
        ))
    ));
    assert!(matches("vps.role = ingress", &context));
}

#[test]
fn membership_lists_and_regex_values_match() {
    let context = vps();
    assert!(matches("status in [stale, online]", &context));
    assert!(matches(r#"vps.tag in ["edge", /^pr/]"#, &context));
    assert!(matches("vps.tag not in [/^test-.*/]", &context));
    assert!(!matches("vps.tag not in [/^prod$/]", &context));
}

#[test]
fn untagged_requires_vps_metadata_with_empty_tags() {
    assert!(matches(
        "untagged",
        &ExpressionContext::for_vps(VpsMetadata::new("id", "name", "online", Vec::new()))
    ));
    assert!(!matches("untagged", &vps()));
    assert!(!matches("untagged", &ExpressionContext::default()));
}

#[test]
fn ordering_supports_rfc3339_unix_seconds_and_numbers() {
    let context = vps();
    assert!(matches("last_seen < 2026-06-08T02:00:00Z", &context));
    assert!(matches("vps.last_seen_at > 1780880000", &context));
    assert!(matches("vps.internal_build_number > 10", &context));
    assert!(!matches("vps.internal_build_number < 10", &context));
}

#[test]
fn event_objects_support_decimal_comparisons_and_canonical_alert_predicates() {
    let context = ExpressionContext::default()
        .with_json_root("traffic", serde_json::json!({"cycle_percent": 82.5}))
        .with_event_predicate("alert.triggered");
    assert!(matches(
        "alert.triggered && traffic.cycle_percent >= 82.25",
        &context
    ));
    assert!(matches("traffic.cycle_percent = 82.5", &context));
    assert!(!matches("traffic.cycle_percent < 80", &context));
}

#[test]
fn canonical_alert_lifecycle_predicates_are_explicit() {
    let generic_triggered = ExpressionContext::default().with_event_predicate("alert.triggered");
    assert!(matches("alert.triggered", &generic_triggered));
    assert!(!matches("alert.resolved", &generic_triggered));

    let resolved = ExpressionContext::default().with_event_predicate("alert.resolved");
    assert!(matches("alert.resolved", &resolved));
    assert!(!matches("alert.triggered", &resolved));
}

#[test]
fn retired_alert_event_aliases_report_canonical_replacements() {
    for (alias, canonical) in [
        ("alert.open", "alert.triggered"),
        ("alert.policy_reached", "alert.triggered"),
        ("alert.policy_triggered", "alert.triggered"),
        ("alert.policy_resolved", "alert.resolved"),
    ] {
        assert_eq!(
            parse_expression(alias).unwrap_err(),
            format!("retired alert event predicate `{alias}`; use `{canonical}`")
        );
        assert_eq!(
            parse_expression(&format!("event.kind = {alias}")).unwrap_err(),
            format!("retired alert event kind value `{alias}`; use `{canonical}`")
        );
        assert_eq!(
            parse_expression(&format!("event.kind in [canonical, {alias}]")).unwrap_err(),
            format!("retired alert event kind value `{alias}`; use `{canonical}`")
        );
    }
    assert!(parse_expression("alert.detail = alert.open").is_ok());
    for expression in ["alert.state", "alert.state:open", "alert.state = open"] {
        assert_eq!(
            parse_expression(expression).unwrap_err(),
            "retired alert field `alert.state`; use `alert.lifecycle_state`"
        );
    }
    for (field, canonical) in [
        (
            "policy_rule.condition_expression",
            "policy_rule.trigger_condition_expression",
        ),
        (
            "policy_rule.window_secs",
            "policy_rule.trigger_meta_condition.window_seconds",
        ),
    ] {
        assert_eq!(
            parse_expression(&format!("{field} = value")).unwrap_err(),
            format!("retired policy-rule field `{field}`; use `{canonical}`")
        );
        assert_eq!(
            parse_expression(&format!("{field}:value")).unwrap_err(),
            format!("retired policy-rule field `{field}`; use `{canonical}`")
        );
    }
}

#[test]
fn retired_alert_event_alias_rewrite_preserves_the_expression_tree() {
    let rewritten = rewrite_retired_alert_event_aliases(
        "ALERT.OPEN || (alert.policy_reached && !(alert.policy_triggered || alert.policy_resolved))",
    )
    .unwrap();
    let actual = parse_expression(&rewritten).unwrap().unwrap();
    let expected = parse_expression(
        "alert.triggered || (alert.triggered && !(alert.triggered || alert.resolved))",
    )
    .unwrap()
    .unwrap();

    assert_eq!(actual, expected);
    assert_eq!(
        expression_referenced_events(&actual),
        BTreeSet::from(["alert.resolved".to_string(), "alert.triggered".to_string()])
    );

    let source = r#"alert.open && (provider:alpha || (name = "A \"quoted\" \\ path" && tag in ["a,b", /^prod\//] && !alert.policy_resolved))"#;
    let rewritten = rewrite_retired_alert_event_aliases(source).unwrap();
    let actual = parse_expression(&rewritten).unwrap().unwrap();
    let expected = parse_expression(
        r#"alert.triggered && (provider:alpha || (name = "A \"quoted\" \\ path" && tag in ["a,b", /^prod\//] && !alert.resolved))"#,
    )
    .unwrap()
    .unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn retired_alert_event_rewrite_only_changes_exact_event_nodes() {
    let input = "alert.open = alert.policy_resolved && alert.open:literal && alert.opened";
    assert_eq!(rewrite_retired_alert_event_aliases(input).unwrap(), input);
    assert!(rewrite_retired_alert_event_aliases("alert.open && (").is_err());
}

#[test]
fn retired_alert_alias_rewrite_updates_exact_event_kind_values_only() {
    let rewritten = rewrite_retired_alert_event_aliases(concat!(
        "event.kind = alert.open || ",
        "event.kind in [alert.policy_reached, alert.policy_triggered, alert.policy_resolved, other] || ",
        "alert.detail = alert.open",
    ))
    .unwrap();
    let actual = parse_expression(&rewritten).unwrap().unwrap();
    let expected = parse_expression(concat!(
        "event.kind = alert.triggered || ",
        "event.kind in [alert.triggered, alert.triggered, alert.resolved, other] || ",
        "alert.detail = alert.open",
    ))
    .unwrap()
    .unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn rewrite_updates_only_exact_policy_rule_field_references() {
    let source = concat!(
        "policy_rule.condition_expression = \"policy_rule.window_secs\" && ",
        "policy_rule.window_secs in [0, 60] && ",
        "policy_rule.window_secs_extra = 9",
    );
    let rewritten = rewrite_retired_alert_event_aliases(source).unwrap();
    let actual = parse_expression(&rewritten).unwrap().unwrap();
    let expected = parse_expression(concat!(
        "policy_rule.trigger_condition_expression = \"policy_rule.window_secs\" && ",
        "policy_rule.trigger_meta_condition.window_seconds in [0, 60] && ",
        "policy_rule.window_secs_extra = 9",
    ))
    .unwrap()
    .unwrap();

    assert_eq!(actual, expected);
    assert_eq!(
        rewrite_retired_alert_event_aliases("policy_rule.window_secs_extra = 9").unwrap(),
        "policy_rule.window_secs_extra = 9"
    );
    assert_eq!(
        rewrite_retired_alert_event_aliases("policy_rule.window_secs").unwrap(),
        "policy_rule.window_secs"
    );
}

#[test]
fn policy_rule_is_a_first_class_expression_context_root() {
    let context = ExpressionContext::default().with_json_root(
        "policy_rule",
        serde_json::json!({
            "trigger_condition_expression": "traffic.cycle_percent >= 80",
            "trigger_meta_condition": {"window_seconds": 0}
        }),
    );

    assert!(matches(
        "policy_rule.trigger_condition_expression = \"traffic.cycle_percent >= 80\"",
        &context
    ));
    assert!(matches(
        "policy_rule.trigger_meta_condition.window_seconds = 0",
        &context
    ));
}

#[test]
fn quoted_list_values_preserve_commas() {
    let context = ExpressionContext::for_vps(VpsMetadata::new(
        "id",
        "abc, def",
        "online",
        vec!["abc, def".to_string()],
    ));
    assert!(matches(r#"name in ["abc, def"]"#, &context));
    assert!(matches(r#"vps.tag in ["abc, def"]"#, &context));
}

#[test]
fn missing_metadata_is_false_but_boolean_not_can_invert() {
    let context = ExpressionContext::default();
    assert!(!matches("vps.status = online", &context));
    assert!(!matches("vps.tag not in [edge]", &context));
    assert!(matches("!(vps.status = online)", &context));
}

#[test]
fn event_predicates_and_event_fields_match_context() {
    let context = ExpressionContext {
        job: Some(serde_json::json!({
            "status": "running",
            "target": {"status": "online"},
            "type": "shell"
        })),
        schedule: Some(serde_json::json!({"id": "sched-a", "name": "Nightly"})),
        ..ExpressionContext::default()
    }
    .with_event_predicate("job.created")
    .with_event_predicate("job.status:running")
    .with_event_predicate("schedule.name:nightly")
    .with_event_predicate("interval.1min");
    assert!(matches("job.created && job.status:running", &context));
    assert!(matches("job.status = running", &context));
    assert!(matches("job.target.status = online", &context));
    assert!(matches("schedule.name:Nightly", &context));
    assert!(matches("schedule.name = Nightly", &context));
    assert!(!matches("server.on_start", &context));
}

#[test]
fn shared_expression_fixture_cases_match() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/expression-cases.json")).unwrap();
    let contexts = fixture
        .get("contexts")
        .and_then(Value::as_object)
        .expect("fixture contexts");
    let cases = fixture
        .get("cases")
        .and_then(Value::as_array)
        .expect("fixture cases");
    for case in cases {
        let name = case.get("name").and_then(Value::as_str).expect("case name");
        let expression = case
            .get("expression")
            .and_then(Value::as_str)
            .expect("case expression");
        let expected = case
            .get("matches")
            .and_then(Value::as_array)
            .expect("case matches")
            .iter()
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>();
        let parsed = parse_expression(expression)
            .unwrap_or_else(|error| panic!("{name}: parse failed: {error}"))
            .expect("fixture expression");
        let actual = contexts
            .iter()
            .filter_map(|(context_name, context)| {
                expression_matches(&fixture_context(context), &parsed)
                    .then_some(context_name.as_str())
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, expected, "fixture case {name}");
    }
    let suggestions = fixture
        .get("parseable_suggestions")
        .and_then(Value::as_array)
        .expect("fixture parseable suggestions");
    for suggestion in suggestions.iter().filter_map(Value::as_str) {
        parse_expression(suggestion)
            .unwrap_or_else(|error| panic!("suggestion {suggestion}: parse failed: {error}"))
            .expect("fixture suggestion expression");
    }
}

#[test]
fn invalid_expressions_report_errors() {
    assert!(parse_expression("(provider:alpha").is_err());
    assert!(parse_expression("provider:").is_err());
    assert!(parse_expression("status in []").is_err());
    assert!(parse_expression("tag in [/edge/i]").is_err());
}

#[test]
fn vps_rules_support_key_presence_patterns_and_explicit_absence() {
    let context = ExpressionContext::default().with_vps_rules(rule_context(&[
        ("traffic.reset_day", "15", serde_json::json!({"day": 15})),
        (
            "network.port_speed",
            "1.5 Gbps",
            serde_json::json!({"bps": 1_500_000_000_i64}),
        ),
    ]));
    assert!(matches("vps.rules:*", &context));
    assert!(matches("vps.rules:traffic.reset_day", &context));
    assert!(matches("vps.rules:traffic.*", &context));
    assert!(matches("vps.rules in [/^network\\./]", &context));
    assert!(!matches("vps.rules:billing.price", &context));
    assert!(matches("!(vps.rules:billing.price)", &context));
    assert!(!matches(
        "vps.rules:*",
        &ExpressionContext::default().with_vps_rules(VpsRuleContext::default())
    ));
}

#[test]
fn vps_rule_exact_comparisons_share_save_time_normalization() {
    let context = ExpressionContext::default().with_vps_rules(rule_context(&[
        (
            "network.port_speed",
            "1.5 Gbps",
            serde_json::json!({"bps": 1_500_000_000_i64}),
        ),
        (
            "billing.price",
            "29.90 CNY/m",
            serde_json::json!({
                "disabled": false,
                "price": "29.90",
                "currency": "CNY",
                "period_code": "m"
            }),
        ),
        (
            "traffic.selectors",
            "ens3,eth0+tx",
            serde_json::json!({"selectors": []}),
        ),
        (
            "traffic.quota.total",
            "4TB",
            serde_json::json!({"bytes": 4_000_000_000_000_i64}),
        ),
    ]));
    assert!(matches(
        "vps.rules:network.port_speed = 1.500gbps",
        &context
    ));
    assert!(!matches(
        "vps.rules:network.port_speed != 1.500gbps",
        &context
    ));
    assert!(matches(
        "vps.rules:billing.price = \"29.9 cny / M\"",
        &context
    ));
    assert!(matches(
        "vps.rules:traffic.selectors = \"ens3, eth0+tx\"",
        &context
    ));
    assert!(matches(
        "vps.rules:traffic.quota.total = \"04.00 tb\"",
        &context
    ));
}

#[test]
fn vps_rule_globs_and_regexes_match_canonical_raw_text() {
    let context = ExpressionContext::default().with_vps_rules(rule_context(&[(
        "network.port_speed",
        "1.5 Gbps",
        serde_json::json!({"bps": 1_500_000_000_i64}),
    )]));
    assert!(matches("vps.rules:network.port_speed = *Gbps", &context));
    assert!(matches(
        "vps.rules:network.port_speed in [/^1\\.5 Gbps$/]",
        &context
    ));
    assert!(!matches(
        "vps.rules:network.port_speed in [/^1\\.500gbps$/]",
        &context
    ));
}

#[test]
fn vps_rule_ordering_is_typed_and_excludes_sentinels() {
    let context = ExpressionContext::default().with_vps_rules(rule_context(&[
        ("traffic.reset_day", "15", serde_json::json!({"day": 15})),
        (
            "traffic.quota.total",
            "4TB",
            serde_json::json!({"bytes": 4_000_000_000_000_i64}),
        ),
        (
            "network.port_speed",
            "1.5 Gbps",
            serde_json::json!({"bps": 1_500_000_000_i64}),
        ),
        (
            "billing.price",
            "29.90 CNY/m",
            serde_json::json!({
                "disabled": false,
                "price": "29.90",
                "currency": "CNY",
                "period_code": "m"
            }),
        ),
    ]));
    assert!(matches("vps.rules:traffic.reset_day >= 15", &context));
    assert!(matches("vps.rules:traffic.quota.total > 3.5TiB", &context));
    assert!(matches("vps.rules:network.port_speed > 1000Mbps", &context));
    assert!(matches("vps.rules:billing.price < \"50 CNY/m\"", &context));
    assert!(!matches("vps.rules:billing.price < \"50 USD/m\"", &context));
    assert!(!matches("vps.rules:billing.price < \"50 CNY/y\"", &context));

    let sentinels = ExpressionContext::default().with_vps_rules(rule_context(&[
        ("traffic.reset_day", "-1", serde_json::json!({"day": -1})),
        (
            "traffic.quota.total",
            "-1",
            serde_json::json!({"bytes": -1, "unlimited": true}),
        ),
        ("billing.price", "-1", serde_json::json!({"disabled": true})),
    ]));
    assert!(matches("vps.rules:traffic.quota.total = -1", &sentinels));
    assert!(!matches("vps.rules:traffic.quota.total < 1TB", &sentinels));
}

#[test]
fn vps_rule_ordering_reparses_authoritative_raw_values_exactly() {
    let context = ExpressionContext::default().with_vps_rules(rule_context(&[
        (
            "traffic.quota.total",
            "9007199254740993B",
            // Deliberately stale legacy JSON: ordered evaluation must use raw.
            serde_json::json!({"bytes": 9_007_199_254_740_992_i64}),
        ),
        (
            "billing.price",
            "29.90 CNY/m",
            // A syntactically valid but inconsistent JSON value must not win.
            serde_json::json!({
                "disabled": false,
                "price": "29.90",
                "currency": "USD",
                "period_code": "y"
            }),
        ),
    ]));
    assert!(matches(
        "vps.rules:traffic.quota.total >= 9007199254740993B",
        &context
    ));
    assert!(matches(
        "vps.rules:traffic.quota.total > 9007199254740992B",
        &context
    ));
    assert!(matches("vps.rules:billing.price < \"50 CNY/m\"", &context));
    assert!(!matches("vps.rules:billing.price < \"50 USD/m\"", &context));
    assert!(!matches("vps.rules:billing.price < \"50 CNY/y\"", &context));
}

#[test]
fn vps_rule_missing_values_keep_direct_inequality_false() {
    let context = ExpressionContext::default().with_vps_rules(VpsRuleContext::default());
    assert!(!matches("vps.rules:network.port_speed != 1Gbps", &context));
    assert!(matches("!(vps.rules:network.port_speed = 1Gbps)", &context));
}

#[test]
fn vps_rule_semantic_validation_rejects_ambiguous_or_invalid_operations() {
    let cases = [
        (
            "vps.rules:billing.cycle > 7",
            "does not support ordered comparisons",
        ),
        ("vps.rules:traffic.reset_day > -1", "day from 1 to 31"),
        (
            "vps.rules:traffic.quota.total > plenty",
            "positive byte size",
        ),
        ("vps.rules:network.port_speed > 100", "positive speed"),
        ("vps.rules:billing.price > 50", "currency and period"),
        (
            "vps.rules:billing.price > \"1000000000 USD/m\"",
            "positive amount",
        ),
        (
            "vps.rules:billing.price > \"50 USD/month\"",
            "currency and period",
        ),
        (
            "vps.rules:traffic.quota.total > 9223372036854775808B",
            "positive byte size",
        ),
        ("vps.rules:unknown.rule = value", "unsupported VPS rule key"),
        (
            "vps.rules.billing.price = \"29.90 CNY/m\"",
            "use vps.rules:<key>",
        ),
        ("vps.rules", "use vps.rules:<key>"),
        ("vps.rules.billing.price", "use vps.rules:<key>"),
    ];
    for (expression, expected_error) in cases {
        let error = parse_expression(expression).unwrap_err();
        assert!(
            error.contains(expected_error),
            "{expression}: expected {expected_error:?}, got {error:?}"
        );
    }
}

#[test]
fn vps_rule_reference_detection_walks_boolean_expressions() {
    let expression =
        parse_expression("status:online && (vps.rules:traffic.reset_day >= 15 || tag:edge)")
            .unwrap()
            .unwrap();
    assert!(expression_references_vps_rules(&expression));
    assert!(!expression_references_vps_rules(
        &parse_expression("status:online && tag:edge")
            .unwrap()
            .unwrap()
    ));
}

#[test]
fn shared_vps_rule_fixture_matches_and_normalizes() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/vps-rule-cases.json")).unwrap();
    let fixture_input = |case: &Value| {
        if let Some(input) = case["input"].as_str() {
            return input.to_string();
        }
        let parts = &case["input_parts"];
        format!(
            "{}{}{}",
            parts["prefix"].as_str().unwrap_or_default(),
            parts["repeat"]
                .as_str()
                .unwrap()
                .repeat(parts["count"].as_u64().unwrap() as usize),
            parts["suffix"].as_str().unwrap_or_default()
        )
    };
    for case in fixture["normalization_cases"].as_array().unwrap() {
        let input = fixture_input(case);
        let parsed = parse_vps_rule_value(case["key"].as_str().unwrap(), &input).unwrap();
        assert_eq!(parsed.raw, case["canonical"].as_str().unwrap());
        assert_eq!(
            parse_vps_rule_value(case["key"].as_str().unwrap(), &parsed.raw)
                .unwrap()
                .raw,
            parsed.raw,
            "fixture normalization must be idempotent for {}",
            case["name"].as_str().unwrap()
        );
    }
    for case in fixture["invalid_normalization_cases"].as_array().unwrap() {
        let input = fixture_input(case);
        let error = parse_vps_rule_value(case["key"].as_str().unwrap(), &input).unwrap_err();
        assert!(
            error.contains(case["error_contains"].as_str().unwrap()),
            "invalid normalization fixture {}: {error}",
            case["name"].as_str().unwrap()
        );
    }

    let contexts = fixture["contexts"].as_object().unwrap();
    for case in fixture["expression_cases"].as_array().unwrap() {
        let expression = parse_expression(case["expression"].as_str().unwrap())
            .unwrap()
            .unwrap();
        let expected = case["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<BTreeSet<_>>();
        let actual = contexts
            .iter()
            .filter_map(|(name, context)| {
                let mut rules = VpsRuleContext::default();
                for rule in context["rules"].as_array().unwrap() {
                    rules.insert(
                        rule["key"].as_str().unwrap(),
                        rule["raw"].as_str().unwrap(),
                        rule["json"].clone(),
                    );
                }
                expression_matches(
                    &ExpressionContext::default().with_vps_rules(rules),
                    &expression,
                )
                .then_some(name.as_str())
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, expected, "fixture case {}", case["name"]);
    }
    for case in fixture["invalid_expressions"].as_array().unwrap() {
        let error = parse_expression(case["expression"].as_str().unwrap()).unwrap_err();
        assert!(error.contains(case["error_contains"].as_str().unwrap()));
    }
}
