use super::*;
use vpsman_common::JobCommand;

#[test]
fn terminal_job_commands_use_operation_payload_and_type() {
    let session_id = Uuid::new_v4();
    let mut request = CreateJobRequest {
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

    request.operation = Some(JobCommand::TerminalInput {
        session_id,
        input_seq: 8,
        data_base64: "aWQK".to_string(),
    });
    assert_eq!(request.command_type_label(), "terminal_input");
    assert!(matches!(
        request.job_command().unwrap(),
        JobCommand::TerminalInput { input_seq: 8, .. }
    ));

    request.operation = Some(JobCommand::TerminalResize {
        session_id,
        cols: 100,
        rows: 30,
    });
    assert_eq!(request.command_type_label(), "terminal_resize");
    assert!(matches!(
        request.job_command().unwrap(),
        JobCommand::TerminalResize {
            cols: 100,
            rows: 30,
            ..
        }
    ));

    request.operation = Some(JobCommand::TerminalPoll {
        session_id,
        replay_from_seq: Some(3),
    });
    assert_eq!(request.command_type_label(), "terminal_poll");
    assert!(matches!(
        request.job_command().unwrap(),
        JobCommand::TerminalPoll {
            replay_from_seq: Some(3),
            ..
        }
    ));

    request.operation = Some(JobCommand::TerminalClose {
        session_id,
        reason: Some("operator requested".to_string()),
    });
    assert_eq!(request.command_type_label(), "terminal_close");
    assert!(matches!(
        request.job_command().unwrap(),
        JobCommand::TerminalClose {
            reason: Some(_),
            ..
        }
    ));
}

#[test]
fn terminal_job_commands_reject_unsafe_or_oversized_payloads() {
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

    request.operation = Some(JobCommand::TerminalInput {
        session_id: Uuid::new_v4(),
        input_seq: 1,
        data_base64: String::new(),
    });
    assert_eq!(
        request.job_command().unwrap_err().code,
        "terminal_input_size_invalid"
    );

    request.operation = Some(JobCommand::TerminalPoll {
        session_id: Uuid::nil(),
        replay_from_seq: None,
    });
    assert_eq!(
        request.job_command().unwrap_err().code,
        "terminal_session_id_invalid"
    );

    request.operation = Some(JobCommand::TerminalResize {
        session_id: Uuid::new_v4(),
        cols: 10,
        rows: 40,
    });
    assert_eq!(
        request.job_command().unwrap_err().code,
        "terminal_cols_out_of_range"
    );
}
