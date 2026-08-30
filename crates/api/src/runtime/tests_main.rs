use super::*;
use vpsman_common::{job_command_type_label, JobCommand};

#[tokio::test]
async fn long_lived_consumer_reports_normal_exit_as_unexpected() {
    let mut consumer = LongLivedConsumer::new("test producer consumer", tokio::spawn(async {}));

    let error = consumer.wait_for_unexpected_exit().await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "test producer consumer exited unexpectedly"
    );
    consumer.join_after_abort().await.unwrap();
}

#[tokio::test]
async fn long_lived_consumer_reports_panics_with_its_exact_name() {
    let mut consumer = LongLivedConsumer::new(
        "panicking test consumer",
        tokio::spawn(async { panic!("consumer panic") }),
    );

    let error = consumer.wait_for_unexpected_exit().await.unwrap_err();
    assert!(error.to_string().contains("panicking test consumer failed"));
    consumer.join_after_abort().await.unwrap();
}

#[tokio::test]
async fn intentional_long_lived_consumer_abort_is_cleanly_joined() {
    let consumer = LongLivedConsumer::new(
        "stopped test consumer",
        tokio::spawn(std::future::pending()),
    );

    consumer.abort();
    consumer.join_after_abort().await.unwrap();
}

#[test]
fn retired_resource_alert_cli_flags_are_rejected() {
    for flag in [
        "--alert-memory-available-warning-ratio",
        "--alert-memory-available-critical-ratio",
        "--alert-disk-available-warning-ratio",
        "--alert-disk-available-critical-ratio",
        "--alert-cpu-load-warning",
        "--alert-cpu-load-critical",
    ] {
        let error = Args::try_parse_from(["vpsman-api", flag, "0.2"])
            .expect_err("retired resource alert flags must not remain parse-compatible");
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::UnknownArgument,
            "{flag}"
        );
    }
}

#[test]
fn file_pull_job_command_uses_operation_payload_and_type() {
    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::FilePull {
            path: "/etc/hostname".to_string(),
            follow_symlinks: false,
        }),
        max_timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
    };

    assert_eq!(request.command_type_label(), "file_pull");
    match request.job_command().unwrap() {
        JobCommand::FilePull {
            path,
            follow_symlinks,
        } => {
            assert_eq!(path, "/etc/hostname");
            assert!(!follow_symlinks);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn shell_pty_job_command_uses_operation_payload_and_type() {
    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: "ignored".to_string(),
        argv: Vec::new(),
        operation: Some(JobCommand::Shell {
            argv: vec!["/usr/bin/tty".to_string()],
            pty: true,
        }),
        max_timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
    };

    let command = request.job_command().unwrap();
    assert_eq!(request.command_type_label(), "shell_pty");
    match command {
        JobCommand::Shell { argv, pty } => {
            assert_eq!(argv, vec!["/usr/bin/tty".to_string()]);
            assert!(pty);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn file_pull_job_command_requires_absolute_path() {
    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::FilePull {
            path: "relative/path".to_string(),
            follow_symlinks: false,
        }),
        max_timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
    };

    let error = request.job_command().unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
}

#[test]
fn file_browser_job_commands_use_operation_payload_and_type() {
    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: true,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::FileListDir {
            path: "/var/log".to_string(),
            offset: 0,
            limit: 250,
            show_hidden: false,
        }),
        max_timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
    };

    assert_eq!(request.command_type_label(), "file_list_dir");
    match request.job_command().unwrap() {
        JobCommand::FileListDir { path, limit, .. } => {
            assert_eq!(path, "/var/log");
            assert_eq!(limit, 250);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn file_browser_job_commands_validate_paths_and_limits() {
    let mut request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: true,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::FileListDir {
            path: "var/log".to_string(),
            offset: 0,
            limit: 250,
            show_hidden: false,
        }),
        max_timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
    };
    assert_eq!(
        request.job_command().unwrap_err().code,
        "file_path_must_be_absolute"
    );

    request.operation = Some(JobCommand::FileListDir {
        path: "/var/log".to_string(),
        offset: 0,
        limit: 0,
        show_hidden: false,
    });
    assert_eq!(
        request.job_command().unwrap_err().code,
        "file_list_limit_out_of_range"
    );

    request.operation = Some(JobCommand::FileWriteText {
        path: "/etc/service.conf".to_string(),
        mode: 0o644,
        size_bytes: 7,
        sha256_hex: vpsman_common::payload_hash(b"updated"),
        content_base64: "dXBkYXRlZA==".to_string(),
        expected_sha256_hex: None,
        create: false,
        policy: vpsman_common::FileActionPolicy::Fail,
    });
    assert_eq!(
        request.job_command().unwrap_err().code,
        "file_write_expected_sha256_required"
    );
}

#[test]
fn shell_script_job_command_uses_operation_payload_and_type() {
    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::ShellScript {
            script: "echo vpsman".to_string(),
        }),
        max_timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
    };

    assert_eq!(request.command_type_label(), "shell_script");
    match request.job_command().unwrap() {
        JobCommand::ShellScript { script } => assert_eq!(script, "echo vpsman"),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn shell_script_job_command_rejects_empty_and_control_payloads() {
    let mut request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::ShellScript {
            script: " ".to_string(),
        }),
        max_timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
    };

    let error = request.job_command().unwrap_err();
    assert_eq!(error.code, "shell_script_is_empty");

    request.operation = Some(JobCommand::ShellScript {
        script: "echo ok\u{0007}".to_string(),
    });
    let error = request.job_command().unwrap_err();
    assert_eq!(error.code, "shell_script_contains_control_character");
}

#[test]
fn user_sessions_job_command_uses_operation_payload_and_type() {
    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::UserSessions),
        max_timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
    };

    assert_eq!(request.command_type_label(), "user_sessions");
    assert!(matches!(
        request.job_command().unwrap(),
        JobCommand::UserSessions
    ));
}

#[test]
fn process_list_job_command_uses_operation_payload_and_type() {
    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::ProcessList { limit: 25 }),
        max_timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
    };

    assert_eq!(request.command_type_label(), "process_list");
    match request.job_command().unwrap() {
        JobCommand::ProcessList { limit } => assert_eq!(limit, 25),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn process_list_job_command_bounds_limit() {
    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:client-a".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        destructive: false,
        confirmed: false,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(JobCommand::ProcessList { limit: 0 }),
        max_timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
    };

    let error = request.job_command().unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
}

pub(crate) fn test_operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "test-operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
        },
        session_id: None,
    }
}

pub(crate) fn operation_job_request(operation: JobCommand, clients: &[&str]) -> CreateJobRequest {
    CreateJobRequest {
        job_id: None,
        selector_expression: test_selector_expression_for_clients(clients),
        target_client_ids: clients.iter().map(|client| (*client).to_string()).collect(),
        destructive: true,
        confirmed: true,
        command: job_command_type_label(&operation).to_string(),
        argv: Vec::new(),
        operation: Some(operation),
        max_timeout_secs: Some(5),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
    }
}
