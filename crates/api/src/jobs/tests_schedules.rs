use vpsman_common::JobCommand;

use crate::{
    model::{CreateScheduleRequest, ScheduleTriggerKind, UpdateScheduleRequest},
    routes_schedules::validate_schedule_request,
};

fn shell_schedule_request(name: &str, enabled: bool) -> CreateScheduleRequest {
    CreateScheduleRequest {
        name: name.to_string(),
        operation: Some(JobCommand::Shell {
            argv: vec!["/usr/bin/uptime".to_string()],
            pty: false,
        }),
        event_argv_template: None,
        selector_expression: "tag:edge".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        trigger_kind: ScheduleTriggerKind::Cron,
        cron_expr: Some("0 * * * *".to_string()),
        timezone: Some("UTC".to_string()),
        event_expression: None,
        enabled,
        catch_up_policy: Some("run_all_limited".to_string()),
        catch_up_limit: Some(3),
        retry_delay_secs: Some(120),
        max_failures: 5,
        privilege_assertion: None,
        confirmed: true,
    }
}

#[test]
fn schedule_validation_rejects_unsafe_or_empty_requests() {
    let mut request = CreateScheduleRequest {
        name: "bad".to_string(),
        operation: Some(JobCommand::Shell {
            argv: vec!["/bin/true".to_string()],
            pty: false,
        }),
        event_argv_template: None,
        selector_expression: "".to_string(),
        target_client_ids: Vec::new(),
        trigger_kind: ScheduleTriggerKind::Cron,
        cron_expr: Some("*/5 * * * *".to_string()),
        timezone: Some("UTC".to_string()),
        event_expression: None,
        enabled: true,
        catch_up_policy: Some("skip_missed".to_string()),
        catch_up_limit: Some(1),
        retry_delay_secs: Some(300),
        max_failures: 3,
        privilege_assertion: None,
        confirmed: true,
    };

    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
    request.selector_expression = "tag:edge".to_string();
    request.target_client_ids = vec!["client-a".to_string()];
    request.cron_expr = Some("bad cron".to_string());
    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
    request.cron_expr = Some("0 0 31 2 *".to_string());
    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
    request.cron_expr = Some("*/5 * * * *".to_string());
    request.timezone = Some("America/New_York".to_string());
    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
    request.timezone = Some("UTC".to_string());
    request.operation = Some(JobCommand::Shell {
        argv: vec!["/bin/sh".to_string()],
        pty: true,
    });
    assert!(validate_schedule_request(&request).is_ok());
    request.operation = Some(JobCommand::Shell {
        argv: Vec::new(),
        pty: false,
    });
    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
    request.operation = Some(JobCommand::Shell {
        argv: vec!["/bin/true".to_string()],
        pty: false,
    });
    request.catch_up_policy = Some("retry_everything".to_string());
    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
    request.catch_up_policy = Some("skip_missed".to_string());
    request.catch_up_limit = Some(0);
    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
    request.catch_up_limit = Some(1);
    request.retry_delay_secs = Some(0);
    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
    request.retry_delay_secs = Some(300);
    request.max_failures = 0;
    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
}

#[test]
fn schedule_validation_rejects_agent_lifecycle_commands() {
    for operation in [JobCommand::AgentStop, JobCommand::AgentRestart] {
        let mut request = shell_schedule_request("invalid-agent-lifecycle", true);
        request.operation = Some(operation);
        assert_eq!(
            validate_schedule_request(&request).unwrap_err().code,
            "agent_lifecycle_not_schedulable"
        );
    }
}

#[test]
fn schedule_update_validation_rejects_cadence_without_a_future_occurrence() {
    let request = UpdateScheduleRequest {
        name: "legacy-cadence-repair".to_string(),
        operation: Some(JobCommand::Shell {
            argv: vec!["/bin/true".to_string()],
            pty: false,
        }),
        event_argv_template: None,
        selector_expression: "tag:edge".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        expected_selector_expression: "tag:edge".to_string(),
        expected_target_client_ids: vec!["client-a".to_string()],
        expected_definition_revision: 1,
        trigger_kind: ScheduleTriggerKind::Cron,
        cron_expr: Some("0 0 31 2 *".to_string()),
        timezone: Some("UTC".to_string()),
        event_expression: None,
        enabled: true,
        catch_up_policy: Some("skip_missed".to_string()),
        catch_up_limit: Some(1),
        retry_delay_secs: Some(300),
        max_failures: 3,
        privilege_assertion: None,
        confirmed: true,
    };

    let error = crate::routes_schedules::validate_update_schedule_request(&request).unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "schedule_cron_invalid");
}
