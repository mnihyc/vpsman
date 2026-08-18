use super::*;

fn context() -> Value {
    json!({
        "rule": {"id": "rule-1", "name": "edge-alert", "expression": "alert.triggered && alert.category:resource"},
        "event": {"kind": "alert.triggered", "id": "event-1", "predicates": ["alert.triggered", "alert.severity:critical"]},
        "query": {"expression": "alert.triggered && alert.category:resource"},
        "alert": {"severity": "critical", "category": "resource", "lifecycle_state": "triggered"},
        "matched_vps": [
            {"id": "edge-a", "display_name": "edge-a", "status": "online", "tags": ["edge"]},
            {"id": "edge-b", "display_name": "edge-b", "status": "stale", "tags": ["edge", "prod"]}
        ]
    })
}

#[test]
fn renders_placeholders_loops_and_conditionals() {
    let rendered = render_template(
        "{rule.name} {event.kind} {matched_vps.length} [if alert.severity = critical]critical[else]other[endif] [for v in matched_vps]{v.name}:{v.status} [endfor]",
        &context(),
    )
    .unwrap();
    assert_eq!(
        rendered,
        "edge-alert alert.triggered 2 critical edge-a:online edge-b:stale "
    );
}

#[test]
fn helpers_map_filter_count_join_and_missing_paths() {
    let rendered = render_template(
        "{matched_vps.filter(vps.status = online).map(vps.name).join(\", \")} {matched_vps.count(vps.status != online)} {missing.path}",
        &context(),
    )
    .unwrap();
    assert_eq!(rendered, "edge-a 1 ");
}

#[test]
fn scalar_templates_reject_missing_and_composite_interpolations() {
    assert_eq!(
        render_scalar_template(
            "{alert.severity}:{matched_vps.length}:[if alert.severity = critical]yes[endif]",
            &context(),
        )
        .unwrap(),
        "critical:2:yes"
    );
    assert!(render_scalar_template("{missing.path}", &context()).is_err());
    assert!(render_scalar_template("{matched_vps}", &context()).is_err());
    assert!(render_scalar_template("{alert}", &context()).is_err());
    assert!(
        render_scalar_template("[for v in matched_vps]{v.display_name}[endfor]", &context(),)
            .is_err()
    );
}

#[test]
fn malformed_blocks_and_conditions_are_rejected() {
    assert!(validate_template("[if alert.severity =]x[endif]").is_err());
    assert!(validate_template("[for 1bad in matched_vps]x[endfor]").is_err());
    assert!(validate_template("[if alert.triggered]x").is_err());
    assert!(validate_template("{matched_vps.filter()}").is_err());
}

#[test]
fn retired_alert_event_aliases_are_rejected_in_template_conditions() {
    let error = validate_template("[if alert.open]triggered[endif]").unwrap_err();
    assert!(error
        .to_string()
        .contains("retired alert event predicate `alert.open`; use `alert.triggered`"));

    let error = validate_template("[if alert.triggered]x[elseif alert.policy_resolved]y[endif]")
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("retired alert event predicate `alert.policy_resolved`; use `alert.resolved`"));

    let error = validate_template("{matched_vps.filter(alert.policy_triggered)}").unwrap_err();
    assert!(error
        .to_string()
        .contains("retired alert event predicate `alert.policy_triggered`; use `alert.triggered`"));

    let error = validate_template("[if event.kind = alert.open]x[endif]").unwrap_err();
    assert!(error
        .to_string()
        .contains("retired alert event kind value `alert.open`; use `alert.triggered`"));
    let error = validate_template("{matched_vps.count(event.kind in [alert.policy_resolved])}")
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("retired alert event kind value `alert.policy_resolved`; use `alert.resolved`"));
}

#[test]
fn rewrites_retired_aliases_and_fields_in_if_and_elseif_conditions() {
    let template = concat!(
        "{# [if alert.open]comment example[endif] #}\n",
        "prefix [if ALERT.OPEN && policy_rule.window_secs = 0]triggered",
        "[elseif alert.policy_resolved && policy_rule.condition_expression = ready]resolved[else]other[endif] ",
        "{event.kind} [alert.open]",
    );
    let rewritten = rewrite_template_retired_alert_event_aliases(template).unwrap();

    assert_eq!(
        rewritten,
        concat!(
            "{# [if alert.open]comment example[endif] #}\n",
            "prefix [if (alert.triggered && policy_rule.trigger_meta_condition.window_seconds = \"0\")]triggered",
            "[elseif (alert.resolved && policy_rule.trigger_condition_expression = \"ready\")]resolved[else]other[endif] ",
            "{event.kind} [alert.open]",
        )
    );
    validate_template(&rewritten).unwrap();

    let canonical = "[if   alert.triggered ]yes[endif]";
    assert_eq!(
        rewrite_template_retired_alert_event_aliases(canonical).unwrap(),
        canonical
    );
}

#[test]
fn rewritten_policy_rule_conditions_render_against_the_normalized_root() {
    let template = rewrite_template_retired_alert_event_aliases(concat!(
        "[if alert.open && policy_rule.window_secs = 0]",
        "{policy_rule.condition_expression}",
        "[else]not-triggered[endif]",
    ))
    .unwrap();
    let context = json!({
        "event": {"predicates": ["alert.triggered"]},
        "policy_rule": {
            "trigger_condition_expression": "traffic.cycle_percent >= 80",
            "trigger_meta_condition": {"window_seconds": 0}
        }
    });

    assert_eq!(
        render_template(&template, &context).unwrap(),
        "traffic.cycle_percent >= 80"
    );
}

#[test]
fn rewrites_helper_conditions_and_exact_placeholder_paths() {
    let template = concat!(
        "{items.filter(alert.open && policy_rule.window_secs > 0)",
        ".where(alert.policy_resolved || policy_rule.condition_expression = \"policy_rule.window_secs\")",
        ".count(alert.policy_triggered)} ",
        "{policy_rule.condition_expression} {policy_rule.window_secs} ",
        "{items.map(policy_rule.condition_expression)} ",
        "[for p in policy_rule.window_secs]{p}[endfor] ",
        "{policy_rule.window_secs_extra} ",
        "{items.join(\"alert.open policy_rule.window_secs\")}",
    );
    let rewritten = rewrite_template_retired_alert_event_aliases(template).unwrap();

    assert_eq!(
        rewritten,
        concat!(
            "{items.filter((alert.triggered && policy_rule.trigger_meta_condition.window_seconds > \"0\"))",
            ".where((alert.resolved || policy_rule.trigger_condition_expression = \"policy_rule.window_secs\"))",
            ".count(alert.triggered)} ",
            "{policy_rule.trigger_condition_expression} ",
            "{policy_rule.trigger_meta_condition.window_seconds} ",
            "{items.map(policy_rule.trigger_condition_expression)} ",
            "[for p in policy_rule.trigger_meta_condition.window_seconds]{p}[endfor] ",
            "{policy_rule.window_secs_extra} ",
            "{items.join(\"alert.open policy_rule.window_secs\")}",
        )
    );
    validate_template(&rewritten).unwrap();
}

#[test]
fn rewrites_event_kind_alias_values_in_template_conditions_and_helpers() {
    let template = concat!(
        "[if event.kind = alert.open]open[endif] ",
        "{items.count(event.kind in [alert.policy_triggered, alert.policy_resolved])} ",
        "{alert.detail}",
    );
    let rewritten = rewrite_template_retired_alert_event_aliases(template).unwrap();
    assert_eq!(
        rewritten,
        concat!(
            "[if event.kind = \"alert.triggered\"]open[endif] ",
            "{items.count(event.kind in [\"alert.triggered\", \"alert.resolved\"])} ",
            "{alert.detail}",
        )
    );
}

#[test]
fn comments_can_hold_selectable_examples_without_rendering_or_references() {
    let template = concat!(
        "{#\n",
        "Alert: [{alert.severity}] {alert.title} on {vps.display_name}\n",
        "Threshold: {traffic.cycle_percent}% for [if alert.triggered]{policy.name}[endif]\n",
        "#}\n",
        "{rule.name}: {event.kind}",
    );

    assert_eq!(
        render_template(template, &context()).unwrap(),
        "edge-alert: alert.triggered"
    );
    assert_eq!(
        template_referenced_paths(template).unwrap(),
        BTreeSet::from(["event.kind".to_string(), "rule.name".to_string()])
    );
}

#[test]
fn multiline_comments_preserve_surrounding_text_and_unmatched_comments_fail() {
    assert_eq!(
        render_template("before\n{#\noperator note\n#}\nafter", &context()).unwrap(),
        "before\nafter"
    );
    assert!(validate_template("before\n{#\nunfinished").is_err());
}
