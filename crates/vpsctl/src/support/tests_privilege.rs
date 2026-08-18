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
            definition_revision: None,
            name: "nightly",
            payload: SchedulePrivilegePayload::Operation(&command),
            command_type: "shell_argv",
            selector_expression: "id:client-a",
            resolved_targets: &clients,
            trigger_kind: "cron",
            cron_expr: Some("0 3 * * *"),
            timezone: Some("UTC"),
            event_expression: None,
            enabled: true,
            catch_up_policy: Some("skip_missed"),
            catch_up_limit: Some(1),
            retry_delay_secs: Some(60),
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

#[test]
fn builds_event_schedule_privilege_from_the_unrendered_template() {
    let clients = vec!["client-a".to_string()];
    let template = vec![
        "/usr/local/bin/limit-traffic".to_string(),
        "{event.kind}".to_string(),
        "{alert.target_id}".to_string(),
    ];
    let assertion = build_privilege_for_schedule(
        SchedulePrivilegeRequest {
            action: "schedule.update",
            schedule_id: Some("00000000-0000-4000-8000-000000000001"),
            definition_revision: Some(7),
            name: "traffic guard",
            payload: SchedulePrivilegePayload::AlertEventArgv(Some(template.as_slice())),
            command_type: "shell",
            selector_expression: "id:client-a",
            resolved_targets: &clients,
            trigger_kind: "event",
            cron_expr: None,
            timezone: None,
            event_expression: Some("alert.triggered && alert.category:traffic"),
            enabled: true,
            catch_up_policy: None,
            catch_up_limit: None,
            retry_delay_secs: None,
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
    assert_eq!(assertion.assertion_hex.len(), 64);
    let template_hash = alert_event_argv_template_hash(Some(template.as_slice())).unwrap();
    let intent = canonical_schedule_privilege_intent(SchedulePrivilegeIntentInput {
        action: "schedule.update",
        schedule_id: Some("00000000-0000-4000-8000-000000000001"),
        definition_revision: Some(7),
        name: "traffic guard",
        command_type: "shell",
        operation_payload_hash: &template_hash,
        selector_expression: "id:client-a",
        resolved_targets: &clients,
        trigger_kind: "event",
        cron_expr: None,
        timezone: None,
        event_expression: Some("alert.triggered && alert.category:traffic"),
        enabled: true,
        catch_up_policy: None,
        catch_up_limit: None,
        retry_delay_secs: None,
        max_failures: 3,
        deferred_until: None,
        deleted: false,
    })
    .unwrap();
    let verifier_key = derive_super_key("correct horse", &[1, 2, 3, 4]);
    assert!(verify_privilege_assertion(
        &verifier_key,
        &intent,
        &assertion,
        assertion.issued_unix,
        &mut PrivilegeAssertionReplayCache::default(),
    )
    .is_ok());
}
