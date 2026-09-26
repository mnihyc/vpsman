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

#[test]
fn references_include_conditions_and_collection_helpers_without_literal_values() {
    let template = concat!(
        "[if job.output = present]ready",
        "[elseif !(job.status = failed) || event.kind = job.output]other[endif] ",
        "{matched_vps.filter(job.output = present).count(vps.status in [online])} ",
        "[for v in matched_vps.filter(job.output = present).map(vps.name)]{v}[endfor]",
        "{# {secret.output} #}",
    );
    let mut data = context();
    data["job"] = json!({"output": "present", "status": "completed"});
    assert_eq!(
        render_template(template, &data).unwrap(),
        "ready 1 edge-aedge-b"
    );
    assert_eq!(
        template_referenced_paths(template).unwrap(),
        BTreeSet::from(
            [
                "job.output",
                "job.status",
                "event.kind",
                "matched_vps",
                "vps.status",
                "vps.name",
                "v"
            ]
            .map(String::from)
        )
    );
    assert!(
        !template_referenced_paths("[if event.kind = job.output]literal[endif]")
            .unwrap()
            .contains("job.output")
    );
    let literal_condition = "[if job.output]event[else]no such event[endif]";
    assert_eq!(
        render_template(literal_condition, &data).unwrap(),
        "no such event"
    );
    assert!(!template_referenced_paths(literal_condition)
        .unwrap()
        .contains("job.output"));
}

#[test]
fn output_references_follow_the_renderers_path_segment_rules() {
    let data = json!({"job":{"output":"retained"}});
    for template in ["{job . output}", "{job..output}", "{.job.output.}"] {
        assert_eq!(render_template(template, &data).unwrap(), "retained");
        assert_eq!(
            template_referenced_paths(template).unwrap(),
            BTreeSet::from(["job.output".to_string()])
        );
    }
}

fn bounded_options() -> TemplateRenderOptions<'static> {
    TemplateRenderOptions {
        max_message_bytes: 16 * 1024,
        max_substitution_bytes: Some(4096),
        literal_object_paths: &["job.output"],
    }
}

fn assert_truncated_substitution(full: &str, rendered: &str, limit: usize) {
    let (prefix, marker) = rendered.rsplit_once("...[").unwrap();
    let omitted: usize = marker
        .strip_suffix(" bytes remaining]")
        .unwrap()
        .parse()
        .unwrap();
    assert!(omitted > 0);
    assert_eq!(full.len() - prefix.len(), omitted);
    assert!(full.starts_with(prefix));
    assert!(rendered.len() <= limit);
    let next = full[prefix.len()..].chars().next().unwrap();
    let next_len = prefix.len() + next.len_utf8();
    assert!(next_len + format!("...[{} bytes remaining]", full.len() - next_len).len() > limit);
}

#[test]
fn string_helpers_split_join_and_substr_chain_with_unicode_indices() {
    let data = json!({"text":"ab🙂界cd", "lines":"one\ntwo\nthree", "empty":""});
    for (template, expected) in [
        (r#"{lines.split("\n").join("|").substr(4,3)}"#, "two"),
        (r"{lines.split('\n').last}", "three"),
        (r#"{text.split("").join("-")}"#, "a-b-🙂-界-c-d"),
        ("{text.substr(2,2)}", "🙂界"),
        ("{text.substr(-3)}", "界cd"),
        ("{text.substr(-3,2)}", "界c"),
        ("{text.substr(-100,2)}", "ab"),
        ("{text.substr(100)}", ""),
        ("{text.substr(1,0)}", ""),
        ("{text.substr(1,-1)}", ""),
        ("{text.substr(-9223372036854775808,1)}", "a"),
        (r#"{empty.split("").length}"#, "0"),
    ] {
        assert_eq!(
            render_template(template, &data).unwrap(),
            expected,
            "{template}"
        );
    }
    // Join's established literal separator behavior is unchanged.
    assert_eq!(
        render_template(r#"{lines.split("\n").join("\n")}"#, &data).unwrap(),
        r"one\ntwo\nthree"
    );
    assert_eq!(
        render_scalar_template("{text.substr(2,2)}", &data).unwrap(),
        "🙂界"
    );
}

#[test]
fn new_string_helpers_reject_missing_arguments_bad_indices_and_non_strings() {
    for template in [
        "{text.split()}",
        "{text.substr()}",
        "{text.substr(1,)}",
        "{text.substr(1,2,3)}",
        "{text.substr(no)}",
        "{text.substr(1.5)}",
        r#"{text.split("\q")}"#,
    ] {
        assert!(validate_template(template).is_err(), "{template}");
    }
    let data = json!({"map":{"id":"one"}, "array":["one"], "number":1});
    for template in [
        "{map.split(',')}",
        "{array.substr(0)}",
        "{number.split('')}",
    ] {
        assert!(render_template(template, &data).is_err(), "{template}");
    }
}

#[test]
fn substitution_limits_apply_after_helpers_not_to_the_source_stream() {
    let data = json!({
        "job":{"output":{"stdout":format!("{}|tail🙂", "x".repeat(6000))}},
        "parts":["a".repeat(3000), "b".repeat(3000)],
    });
    assert_eq!(
        render_template_with_options("{job.output.stdout.substr(6001)}", &data, bounded_options())
            .unwrap(),
        "tail🙂"
    );
    assert_eq!(
        render_template_with_options(
            r#"{job.output.stdout.split("|").last.substr(-1)}"#,
            &data,
            bounded_options()
        )
        .unwrap(),
        "🙂"
    );
    let full = format!("{},{}", "a".repeat(3000), "b".repeat(3000));
    let rendered =
        render_template_with_options(r#"{parts.join(",")}"#, &data, bounded_options()).unwrap();
    assert_truncated_substitution(&full, &rendered, 4096);
}

#[test]
fn each_substitution_has_its_own_budget_and_the_message_limit_is_unchanged() {
    let value = "a".repeat(5000);
    let data = json!({"text":value});
    let rendered = render_template_with_options("{text}|{text}", &data, bounded_options()).unwrap();
    let parts = rendered.split('|').collect::<Vec<_>>();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0], parts[1]);
    assert_truncated_substitution(&value, parts[0], 4096);
    assert_eq!(parts[0].len(), 4096);
    assert_eq!(
        render_template_with_options("{text}{text}{text}{text}", &data, bounded_options())
            .unwrap()
            .len(),
        16 * 1024
    );
    assert!(
        render_template_with_options("{text}{text}{text}{text}x", &data, bounded_options())
            .is_err()
    );
    assert_eq!(render_template("{text}", &data).unwrap(), value);
    assert_eq!(
        render_template_with_limit("{text}", &data, 5000).unwrap(),
        value
    );
    assert_eq!(render_scalar_template("{text}", &data).unwrap(), value);
    assert!(render_template_with_limit("{text}", &data, 4096).is_err());
}

#[test]
fn substitution_truncation_preserves_utf8_and_counts_rendered_json_bytes() {
    let text = "🙂界".repeat(1000);
    let data = json!({"text":text, "job":{"output":{"stdout":{"id":"\"\n\\".repeat(2000)}}}});
    let rendered = render_template_with_options("{text}", &data, bounded_options()).unwrap();
    assert_truncated_substitution(&text, &rendered, 4096);
    let full_json = data["job"]["output"]["stdout"].to_string();
    let rendered =
        render_template_with_options("{job.output.stdout}", &data, bounded_options()).unwrap();
    assert_truncated_substitution(&full_json, &rendered, 4096);
    let options = TemplateRenderOptions {
        max_substitution_bytes: Some(1),
        ..bounded_options()
    };
    assert!(render_template_with_options("{text}", &data, options).is_err());
    assert_eq!(
        render_template_with_options(
            "{text}",
            &json!({"text":"x".repeat(4096)}),
            bounded_options()
        )
        .unwrap()
        .len(),
        4096
    );
}

#[test]
fn literal_object_paths_preserve_map_keys_json_escaping_and_lookup_identity() {
    let map = json!({"id":"first\n\"", "name":"second", "display_name":"third", "other":"fourth"});
    let data = json!({"job":{"output":{"stdout":map}, "output_extra":map}});
    for template in ["{job.output.stdout}", "{.job .. output .stdout.}"] {
        let rendered = render_template_with_options(template, &data, bounded_options()).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&rendered).unwrap(), map);
    }
    assert_eq!(
        render_template_with_options("{job.output.stdout.name}", &data, bounded_options()).unwrap(),
        "second"
    );
    assert_eq!(
        render_template("{job.output.stdout.name}", &data).unwrap(),
        "third"
    );
    assert_eq!(
        render_template_with_options("{job.output_extra}", &data, bounded_options()).unwrap(),
        "third (first\n\")"
    );
    assert_eq!(
        render_template("{job.output.stdout}", &data).unwrap(),
        "third (first\n\")"
    );
}

#[test]
fn literal_object_mode_follows_loop_aliases_without_leaking_to_shadowed_vps() {
    let data = json!({
        "job":{"output":{"stdout":[{"id":"map-key", "name":"literal", "display_name":"display"}]}},
        "matched_vps":[{"id":"edge", "name":"node"}],
    });
    let rendered = render_template_with_options(
        "[for row in job.output.stdout]{row.name}:[for row in matched_vps]{row}[endfor][endfor]",
        &data,
        bounded_options(),
    )
    .unwrap();
    assert_eq!(rendered, "literal:node (edge)");
}

#[test]
fn map_helper_arguments_preserve_their_own_literal_object_provenance() {
    let mut data = context();
    let output_map =
        json!({"id":"id-output", "name":"name-output", "display_name":"display-output"});
    data["job"] = json!({"output":{"stdout":output_map}});
    for template in [
        "{matched_vps.map(job.output.stdout.name).join(',')}",
        "{matched_vps.map(job . output.stdout.name).join(',')}",
        "{matched_vps.map(job.output.stdout).map(item.name).join(',')}",
    ] {
        assert_eq!(
            render_template_with_options(template, &data, bounded_options()).unwrap(),
            "name-output,name-output",
            "{template}"
        );
        assert_eq!(
            render_template(template, &data).unwrap(),
            "display-output,display-output"
        );
    }
    assert_eq!(
        render_template_with_options(
            "{matched_vps.map(job.output.stdout)}",
            &data,
            bounded_options()
        )
        .unwrap(),
        format!("{output_map} {output_map}")
    );
    assert_eq!(
        render_template_with_options(
            "{matched_vps.map(job.output.stdout).map(rule).first}",
            &data,
            bounded_options()
        )
        .unwrap(),
        "edge-alert (rule-1)"
    );
}
