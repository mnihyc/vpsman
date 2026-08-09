use super::*;

fn context() -> Value {
    json!({
        "rule": {"id": "rule-1", "name": "edge-alert", "expression": "alert.open && tag:edge"},
        "event": {"kind": "alert.open", "id": "event-1", "predicates": ["alert.open", "alert.severity:critical"]},
        "query": {"expression": "alert.open && tag:edge"},
        "alert": {"severity": "critical", "category": "disk", "state": "open"},
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
        "edge-alert alert.open 2 critical edge-a:online edge-b:stale "
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
fn malformed_blocks_and_conditions_are_rejected() {
    assert!(validate_template("[if alert.severity =]x[endif]").is_err());
    assert!(validate_template("[for 1bad in matched_vps]x[endfor]").is_err());
    assert!(validate_template("[if alert.open]x").is_err());
    assert!(validate_template("{matched_vps.filter()}").is_err());
}

#[test]
fn comments_can_hold_selectable_examples_without_rendering_or_references() {
    let template = concat!(
        "{#\n",
        "Alert: [{alert.severity}] {alert.title} on {vps.display_name}\n",
        "Threshold: {traffic.cycle_percent}% for [if alert.open]{policy.name}[endif]\n",
        "#}\n",
        "{rule.name}: {event.kind}",
    );

    assert_eq!(
        render_template(template, &context()).unwrap(),
        "edge-alert: alert.open"
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
