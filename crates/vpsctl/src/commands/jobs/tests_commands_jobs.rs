use super::*;

#[test]
fn renders_followed_job_output_as_text_and_json() {
    let output = JobOutputRecord {
        client_id: "edge-a".to_string(),
        seq: 7,
        stream: "pty".to_string(),
        storage: None,
        artifact_object_key: None,
        artifact_sha256_hex: None,
        artifact_size_bytes: None,
        data_base64: BASE64.encode("hello\r\n"),
        done: true,
    };

    let text = render_job_output(&output, false).unwrap();
    assert_eq!(text, "[edge-a pty #7 done] hello\n");

    let json = render_job_output(&output, true).unwrap();
    let value = serde_json::from_str::<serde_json::Value>(&json).unwrap();
    assert_eq!(value["event"], "job_output");
    assert_eq!(value["client_id"], "edge-a");
    assert_eq!(value["stream"], "pty");
    assert_eq!(value["done"], true);
}

#[test]
fn job_follow_uses_common_terminal_statuses() {
    for status in vpsman_common::job_terminal_statuses() {
        assert!(JobStatus::parse(status).is_some_and(JobStatus::is_terminal));
    }
    for status in vpsman_common::job_statuses()
        .iter()
        .filter(|status| !vpsman_common::job_terminal_statuses().contains(status))
    {
        assert!(!JobStatus::parse(status).is_some_and(JobStatus::is_terminal));
    }
}

#[test]
fn staged_rollout_requires_explicit_resolved_canaries() {
    let targets = vec!["client-a".to_string(), "client-b".to_string()];
    let policy =
        build_job_rollout_policy(&["client-a".to_string()], None, None, None, false, &targets)
            .unwrap()
            .unwrap();
    assert_eq!(policy.canary_client_ids, vec!["client-a"]);
    assert_eq!(policy.batch_size, 5);
    assert_eq!(policy.max_failures, 0);
    assert!(policy.pause_after_canary);

    assert!(build_job_rollout_policy(
        &["client-missing".to_string()],
        Some(1),
        Some(0),
        Some(0),
        false,
        &targets,
    )
    .unwrap_err()
    .to_string()
    .contains("not in the resolved target snapshot"));
}

#[test]
fn rollout_modifiers_without_canary_are_rejected() {
    let targets = vec!["client-a".to_string(), "client-b".to_string()];
    assert!(
        build_job_rollout_policy(&[], Some(10), None, None, false, &targets)
            .unwrap_err()
            .to_string()
            .contains("explicit --rollout-canary")
    );
    assert!(
        build_job_rollout_policy(&[], None, None, None, false, &targets)
            .unwrap()
            .is_none()
    );
}
