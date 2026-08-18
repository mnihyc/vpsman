use super::{parse_vty_event_schedule_create_options, parse_vty_schedule_create_options};

#[test]
fn parses_schedule_create_policy_options() {
    let options = parse_vty_schedule_create_options(&[
        "--catch-up-policy",
        "run_all_limited",
        "--catch-up-limit=4",
        "--retry-delay-secs",
        "120",
        "--max-failures=7",
        "tag:edge",
    ])
    .unwrap();

    assert_eq!(options.catch_up_policy, "run_all_limited");
    assert_eq!(options.catch_up_limit, 4);
    assert_eq!(options.retry_delay_secs, 120);
    assert_eq!(options.max_failures, 7);
    assert_eq!(options.target_tokens, vec!["tag:edge"]);
    assert!(parse_vty_schedule_create_options(&["--catch-up-policy", "bad"]).is_err());
}

#[test]
fn parses_explicit_alert_event_schedule_options() {
    let options = parse_vty_event_schedule_create_options(&[
        "--event-argv-template",
        "/usr/local/bin/limit-traffic",
        "--event-argv-template={event.kind}",
        "--event-argv-template",
        "{alert.target_id}",
        "--max-failures=7",
        "--disabled",
        "tag:edge",
        "--confirmed",
    ])
    .unwrap();

    assert_eq!(
        options.event_argv_template,
        vec![
            "/usr/local/bin/limit-traffic",
            "{event.kind}",
            "{alert.target_id}"
        ]
    );
    assert_eq!(options.max_failures, 7);
    assert_eq!(options.target_tokens, vec!["tag:edge"]);
    assert!(options.disabled);
    assert!(options.confirmed);
}

#[test]
fn event_schedule_options_default_to_the_documented_noop() {
    let options = parse_vty_event_schedule_create_options(&["tag:edge", "--confirmed"]).unwrap();
    assert!(options.event_argv_template.is_empty());
    assert_eq!(options.max_failures, 3);
    assert!(
        parse_vty_event_schedule_create_options(&["--catch-up-policy=run_once", "tag:edge"])
            .is_err()
    );
}
