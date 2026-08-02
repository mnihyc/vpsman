use super::*;
use vpsman_common::JobCommand;

#[test]
fn terminal_open_job_uses_operation_payload_and_type() {
    let session_id = Uuid::new_v4();
    let request = CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: "ignored".to_string(),
        argv: Vec::new(),
        operation: Some(JobCommand::TerminalOpen {
            session_id,
            argv: vec!["/bin/sh".to_string(), "-l".to_string()],
            cwd: Some("/root".to_string()),
            user: None,
            user_policy: vpsman_common::TerminalUserPolicy::Fail,
            cols: 120,
            rows: 40,
            replay_from_seq: Some(7),
            idle_timeout_secs: 1800,
            flow_window_bytes: 65_536,
        }),
        max_timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
    };

    assert_eq!(request.command_type_label(), "terminal_open");
    assert!(matches!(
        request.job_command().unwrap(),
        JobCommand::TerminalOpen {
            cols: 120,
            rows: 40,
            ..
        }
    ));
}

#[test]
fn terminal_open_job_rejects_unsafe_payloads() {
    let mut request = CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::TerminalOpen {
            session_id: Uuid::nil(),
            argv: vec!["/bin/sh".to_string()],
            cwd: None,
            user: None,
            user_policy: vpsman_common::TerminalUserPolicy::Fail,
            cols: 120,
            rows: 40,
            replay_from_seq: None,
            idle_timeout_secs: 1800,
            flow_window_bytes: 65_536,
        }),
        max_timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
    };

    assert_eq!(
        request.job_command().unwrap_err().code,
        "terminal_session_id_invalid"
    );

    request.operation = Some(JobCommand::TerminalOpen {
        session_id: Uuid::new_v4(),
        argv: vec!["sh".to_string()],
        cwd: None,
        user: None,
        user_policy: vpsman_common::TerminalUserPolicy::Fail,
        cols: 120,
        rows: 40,
        replay_from_seq: None,
        idle_timeout_secs: 1800,
        flow_window_bytes: 65_536,
    });
    assert_eq!(
        request.job_command().unwrap_err().code,
        "terminal_executable_must_be_absolute"
    );
}
