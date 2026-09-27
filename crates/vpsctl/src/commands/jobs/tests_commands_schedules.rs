use super::*;
use vpsman_common::{parse_and_match_expression, ExpressionContext, VpsMetadata};

#[test]
fn target_selector_literals_cannot_become_comments_or_another_target() {
    for literal in [
        "edge#note",
        "edge/*note*/",
        "edge/*unfinished",
        "edge\"#\\note",
    ] {
        let selector =
            selector_expression_from_targets(&[literal.to_string(), "core".to_string()], &[]);
        let context = |id: &str| {
            ExpressionContext::for_vps(VpsMetadata {
                id: id.to_string(),
                ..VpsMetadata::default()
            })
        };
        assert!(parse_and_match_expression(&selector, &context(literal)).unwrap());
        assert!(parse_and_match_expression(&selector, &context("core")).unwrap());
        assert!(!parse_and_match_expression(&selector, &context("edge")).unwrap());

        for prefix in [
            "",
            "tag:",
            "provider:",
            "country:",
            "region:",
            "name:",
            "id:",
        ] {
            // Bare tags are literal data; namespaced arguments are authored
            // selector syntax and must quote their literal values explicitly.
            let argument = if prefix.is_empty() {
                literal.to_string()
            } else {
                format!("{prefix}{}", quote_selector_value(literal))
            };
            let selector = selector_expression_from_targets(&[], &[argument]);
            let context = |value: &str| {
                ExpressionContext::for_vps(VpsMetadata {
                    id: value.to_string(),
                    display_name: value.to_string(),
                    tags: vec![
                        value.to_string(),
                        format!("provider:{value}"),
                        format!("country:{value}"),
                        format!("region:{value}"),
                    ],
                    ..VpsMetadata::default()
                })
            };
            assert!(
                parse_and_match_expression(&selector, &context(literal)).unwrap(),
                "{selector}"
            );
            assert!(
                !parse_and_match_expression(&selector, &context("edge")).unwrap(),
                "{selector}"
            );
        }
    }
}

#[test]
fn target_selector_preserves_namespaced_quoted_segments_and_compound_syntax() {
    let authored = "name:Edge\" One\" && !tag:excluded # Keep the authored condition";
    let selector =
        selector_expression_from_targets(&[], &[authored.to_string(), "tag:core".to_string()]);
    assert_eq!(selector_token_from_tag_argument(authored), authored);
    for (name, tags, expected) in [
        ("Edge One", vec![], true),
        ("Edge One", vec!["excluded".to_string()], false),
        ("Other", vec!["core".to_string()], true),
        ("Edge\" One\"", vec![], false),
    ] {
        let context = ExpressionContext::for_vps(VpsMetadata {
            display_name: name.to_string(),
            tags,
            ..VpsMetadata::default()
        });
        assert_eq!(
            parse_and_match_expression(&selector, &context).unwrap(),
            expected
        );
    }
}

#[test]
fn target_selector_preserves_explicit_quoted_tokens_and_following_targets() {
    let selector = selector_expression_from_targets(
        &[],
        &["name:\"Edge#1\" # note".to_string(), "tag:core".to_string()],
    );
    for (name, tags, expected) in [
        ("Edge#1", vec![], true),
        ("Other", vec!["core".to_string()], true),
        ("Edge", vec![], false),
    ] {
        let context = ExpressionContext::for_vps(VpsMetadata {
            display_name: name.to_string(),
            tags,
            ..VpsMetadata::default()
        });
        assert_eq!(
            parse_and_match_expression(&selector, &context).unwrap(),
            expected
        );
    }
    assert_eq!(
        selector_token_from_tag_argument("provider:alpha"),
        "provider:alpha"
    );
    assert_eq!(
        selector_token_from_tag_argument("pool:edge"),
        "tag:pool:edge"
    );
}

fn base_options(trigger_kind: ScheduleTriggerKindArg) -> ScheduleDefinitionOptions {
    ScheduleDefinitionOptions {
        name: "traffic guard".to_string(),
        trigger_kind,
        run_on: None,
        command: None,
        argv: Vec::new(),
        pty: false,
        event_expression: None,
        event_argv_template: Vec::new(),
        cron_expr: None,
        timezone: None,
        disabled: false,
        catch_up_policy: None,
        catch_up_limit: None,
        retry_delay_secs: None,
        max_failures: 3,
        max_timeout_secs: None,
    }
}

#[test]
fn schedule_timeout_override_preserves_defaults_and_existing_bounds() {
    for trigger in [ScheduleTriggerKindArg::Cron, ScheduleTriggerKindArg::Event] {
        for (requested, expected) in [(None, None), (Some(0), Some(1)), (Some(120), Some(120))] {
            let mut options = base_options(trigger);
            match trigger {
                ScheduleTriggerKindArg::Cron => options.command = Some("/bin/true".to_string()),
                ScheduleTriggerKindArg::Event => {
                    options.event_expression = Some("alert.triggered".to_string())
                }
            }
            options.max_timeout_secs = requested;
            assert_eq!(
                ScheduleDefinition::from_options(options)
                    .unwrap()
                    .max_timeout_secs,
                expected
            );
        }
        let mut options = base_options(trigger);
        options.max_timeout_secs = Some(vpsman_common::MAX_CONFIGURABLE_JOB_TIMEOUT_SECS + 1);
        assert!(ScheduleDefinition::from_options(options)
            .unwrap_err()
            .to_string()
            .contains("--max-timeout-secs"));
    }
}

#[test]
fn cron_definition_preserves_the_existing_defaults() {
    let mut options = base_options(ScheduleTriggerKindArg::Cron);
    options.command = Some("/bin/true".to_string());

    let definition = ScheduleDefinition::from_options(options).unwrap();

    assert_eq!(definition.trigger_kind, ScheduleTriggerKindArg::Cron);
    assert_eq!(definition.run_on, ScheduleRunOnArg::AllAtOnce);
    assert!(matches!(
        definition.operation,
        Some(JobCommand::Shell { ref argv, pty: false })
            if argv.len() == 1 && argv.first().map(String::as_str) == Some("/bin/true")
    ));
    assert_eq!(definition.cron_expr.as_deref(), Some("0 * * * *"));
    assert_eq!(definition.timezone.as_deref(), Some("UTC"));
    assert_eq!(definition.catch_up_policy.as_deref(), Some("skip_missed"));
    assert_eq!(definition.catch_up_limit, Some(1));
    assert_eq!(definition.retry_delay_secs, Some(300));
    assert_eq!(definition.max_timeout_secs, None);
    assert!(definition.event_expression.is_none());
    assert!(definition.event_argv_template.is_none());
}

#[test]
fn event_definition_uses_nullable_cron_shape_and_default_noop() {
    let mut options = base_options(ScheduleTriggerKindArg::Event);
    options.event_expression = Some(
        "(alert.triggered && alert.category:traffic) || (alert.resolved && alert.category:traffic)"
            .to_string(),
    );

    let definition = ScheduleDefinition::from_options(options).unwrap();

    assert_eq!(definition.trigger_kind, ScheduleTriggerKindArg::Event);
    assert_eq!(definition.run_on, ScheduleRunOnArg::TriggeredOnly);
    assert!(definition.operation.is_none());
    assert!(definition.event_argv_template.is_none());
    assert!(definition.cron_expr.is_none());
    assert!(definition.timezone.is_none());
    assert!(definition.catch_up_policy.is_none());
    assert!(definition.catch_up_limit.is_none());
    assert!(definition.retry_delay_secs.is_none());
    assert_eq!(definition.command_type(), "shell");
}

#[test]
fn event_definition_accepts_only_direct_scalar_argv_templates() {
    let mut options = base_options(ScheduleTriggerKindArg::Event);
    options.event_expression = Some("alert.triggered && alert.category:traffic".to_string());
    options.event_argv_template = vec![
        "/usr/local/bin/limit-traffic".to_string(),
        "{event.kind}".to_string(),
        "{alert.target_id}".to_string(),
    ];
    assert!(ScheduleDefinition::from_options(options).is_ok());

    let mut options = base_options(ScheduleTriggerKindArg::Event);
    options.event_expression = Some("alert.triggered".to_string());
    options.event_argv_template = vec!["{alert.title}".to_string()];
    assert!(ScheduleDefinition::from_options(options).is_err());
}

#[test]
fn trigger_specific_options_cannot_leak_across_schedule_kinds() {
    let mut event = base_options(ScheduleTriggerKindArg::Event);
    event.event_expression = Some("alert.triggered".to_string());
    event.cron_expr = Some("0 * * * *".to_string());
    assert!(ScheduleDefinition::from_options(event).is_err());

    let mut cron = base_options(ScheduleTriggerKindArg::Cron);
    cron.command = Some("/bin/true".to_string());
    cron.event_expression = Some("alert.triggered".to_string());
    assert!(ScheduleDefinition::from_options(cron).is_err());
}

#[test]
fn event_run_on_can_explicitly_include_all_reviewed_targets_but_cron_cannot_narrow_to_subjects() {
    let mut event = base_options(ScheduleTriggerKindArg::Event);
    event.event_expression = Some("alert.triggered".to_string());
    event.run_on = Some(ScheduleRunOnArg::AllAtOnce);
    assert_eq!(
        ScheduleDefinition::from_options(event).unwrap().run_on,
        ScheduleRunOnArg::AllAtOnce
    );

    let mut cron = base_options(ScheduleTriggerKindArg::Cron);
    cron.command = Some("/bin/true".to_string());
    cron.run_on = Some(ScheduleRunOnArg::TriggeredOnly);
    assert!(ScheduleDefinition::from_options(cron).is_err());
}

#[test]
fn apply_now_is_rejected_for_alert_event_schedules() {
    assert!(validate_apply_now_trigger(ScheduleTriggerKindArg::Cron).is_ok());
    let error = validate_apply_now_trigger(ScheduleTriggerKindArg::Event).unwrap_err();
    assert!(error
        .to_string()
        .contains("only available for cron schedules"));
}

#[test]
fn backup_policy_updates_remain_cron_only() {
    assert!(validate_backup_schedule_trigger(ScheduleTriggerKindArg::Cron).is_ok());
    assert!(validate_backup_schedule_trigger(ScheduleTriggerKindArg::Event).is_err());
}
