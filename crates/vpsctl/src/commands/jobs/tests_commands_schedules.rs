use super::*;

fn base_options(trigger_kind: ScheduleTriggerKindArg) -> ScheduleDefinitionOptions {
    ScheduleDefinitionOptions {
        name: "traffic guard".to_string(),
        trigger_kind,
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
    }
}

#[test]
fn cron_definition_preserves_the_existing_defaults() {
    let mut options = base_options(ScheduleTriggerKindArg::Cron);
    options.command = Some("/bin/true".to_string());

    let definition = ScheduleDefinition::from_options(options).unwrap();

    assert_eq!(definition.trigger_kind, ScheduleTriggerKindArg::Cron);
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
