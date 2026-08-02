use super::parse_vty_schedule_create_options;

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
