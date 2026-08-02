use super::*;
use vpsman_common::{verify_privilege_assertion, PrivilegeAssertionReplayCache};

#[test]
fn builds_job_privilege_assertion_without_command_envelopes() {
    let clients = vec!["client-b".to_string(), "client-a".to_string()];
    let command = JobCommand::Shell {
        argv: vec!["/bin/true".to_string()],
        pty: false,
    };
    let built = build_privilege_for_job_command(
        &clients,
        &command,
        "shell_argv",
        "id:client-a || id:client-b",
        "correct horse",
        "01020304",
        300,
        30,
        false,
        true,
    )
    .unwrap();

    let payload_hash_hex = payload_hash(&encode_json(&command).unwrap());
    assert_eq!(
        built.privilege_assertion.expires_unix,
        built.privilege_assertion.issued_unix + 300
    );

    let verifier_key = derive_super_key("correct horse", &[1, 2, 3, 4]);
    let intent = canonical_job_privilege_intent(JobPrivilegeIntentInput {
        selector_expression: "id:client-a || id:client-b",
        command_type: "shell_argv",
        operation_payload_hash: &payload_hash_hex,
        rollout_policy_hash: None,
        resolved_targets: &clients,
        max_timeout_secs: 30,
        force_unprivileged: false,
        privileged: true,
    })
    .unwrap();
    assert!(verify_privilege_assertion(
        &verifier_key,
        &intent,
        &built.privilege_assertion,
        built.privilege_assertion.issued_unix,
        &mut PrivilegeAssertionReplayCache::default(),
    )
    .is_ok());
}

#[test]
fn builds_schedule_privilege_assertion_for_resolved_targets() {
    let clients = vec!["client-a".to_string()];
    let command = JobCommand::Shell {
        argv: vec!["/bin/true".to_string()],
        pty: false,
    };
    let assertion = build_privilege_for_schedule(
        SchedulePrivilegeRequest {
            action: "schedule.create",
            schedule_id: None,
            name: "nightly",
            command: &command,
            command_type: "shell_argv",
            selector_expression: "id:client-a",
            resolved_targets: &clients,
            cron_expr: "0 3 * * *",
            timezone: "UTC",
            enabled: true,
            catch_up_policy: "skip_missed",
            catch_up_limit: 1,
            retry_delay_secs: 60,
            max_failures: 3,
            deferred_until: None,
            deleted: false,
        },
        "correct horse",
        "01020304",
        120,
    )
    .unwrap();

    assert_eq!(assertion.expires_unix, assertion.issued_unix + 120);
    assert_eq!(assertion.nonce_hex.len(), 32);
    assert_eq!(assertion.assertion_hex.len(), 64);
}
