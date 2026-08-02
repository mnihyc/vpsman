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
        "provider:alpha && country:US && region:US",
        &context
    ));
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
fn event_objects_support_decimal_comparisons_and_policy_alert_predicates() {
    let context = ExpressionContext::default()
        .with_json_root("traffic", serde_json::json!({"cycle_percent": 82.5}))
        .with_event_predicate("alert.policy_reached");
    assert!(matches(
        "alert.policy_reached && traffic.cycle_percent >= 82.25",
        &context
    ));
    assert!(matches("traffic.cycle_percent = 82.5", &context));
    assert!(!matches("traffic.cycle_percent < 80", &context));
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
