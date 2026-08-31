use vpsman_common::{GatewayPrivilegeVerificationBatchItemResult, JobCommand};

use crate::{
    model::{
        BulkUpdateScheduleTargetsItemRequest, BulkUpdateScheduleTargetsRequest,
        CreateScheduleRequest, ScheduleTriggerKind, UpdateScheduleRequest,
    },
    repository_schedules::{ScheduleSnapshotExpectation, ScheduleTargetBatchUpdate},
    routes_schedules::{
        apply_schedule_privilege_batch_results, validate_bulk_schedule_target_selection,
        validate_schedule_request,
    },
};
use uuid::Uuid;

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

#[test]
fn bulk_schedule_target_selection_is_bounded_and_unique_before_mutation() {
    let item = |value| BulkUpdateScheduleTargetsItemRequest {
        schedule_id: Uuid::from_u128(value),
        expected_definition_revision: 1,
        privilege_assertion: None,
    };
    let bounded = BulkUpdateScheduleTargetsRequest {
        items: (1..=500).map(item).collect(),
        confirmed: true,
    };
    validate_bulk_schedule_target_selection(&bounded).expect("500 unique schedules are valid");

    let duplicate = BulkUpdateScheduleTargetsRequest {
        items: vec![item(1), item(1)],
        confirmed: true,
    };
    assert_eq!(
        validate_bulk_schedule_target_selection(&duplicate)
            .unwrap_err()
            .code,
        "schedule_target_selection_duplicate"
    );
    let oversized = BulkUpdateScheduleTargetsRequest {
        items: (1..=501).map(item).collect(),
        confirmed: true,
    };
    assert_eq!(
        validate_bulk_schedule_target_selection(&oversized)
            .unwrap_err()
            .code,
        "schedule_target_selection_too_large"
    );
    let empty = BulkUpdateScheduleTargetsRequest {
        items: Vec::new(),
        confirmed: true,
    };
    assert_eq!(
        validate_bulk_schedule_target_selection(&empty)
            .unwrap_err()
            .code,
        "schedule_target_selection_required"
    );
}

#[test]
fn bulk_schedule_privilege_results_preserve_order_and_partial_rejections() {
    let update = |value| ScheduleTargetBatchUpdate {
        schedule_id: Uuid::from_u128(value),
        target_client_ids: vec![format!("client-{value}")],
        expectation: ScheduleSnapshotExpectation {
            selector_expression: format!("id:client-{value}"),
            target_client_ids: Vec::new(),
            definition_revision: 1,
        },
    };
    let candidates = vec![(2, update(1)), (0, update(2))];
    let results = vec![
        GatewayPrivilegeVerificationBatchItemResult {
            request_id: Uuid::from_u128(1).to_string(),
            approved: true,
            intent_hash_hex: Some("approved".to_string()),
            message: "approved".to_string(),
            error_code: None,
        },
        GatewayPrivilegeVerificationBatchItemResult {
            request_id: Uuid::from_u128(2).to_string(),
            approved: false,
            intent_hash_hex: None,
            message: "denied".to_string(),
            error_code: Some("privilege_assertion_invalid".to_string()),
        },
    ];
    let mut outcomes = vec![None, None, None];
    let accepted =
        apply_schedule_privilege_batch_results(results, candidates, &mut outcomes).unwrap();
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].0, 2);
    assert_eq!(accepted[0].1.schedule_id, Uuid::from_u128(1));
    let rejected = outcomes[0].as_ref().expect("second item is rejected");
    assert_eq!(rejected.schedule_id, Uuid::from_u128(2));
    assert_eq!(rejected.status, "rejected");
    assert_eq!(
        rejected.error_code.as_deref(),
        Some("privilege_verification_failed")
    );
}

#[test]
fn bulk_schedule_route_uses_exactly_one_internal_privilege_batch_call() {
    let source = include_str!("../routes/jobs/routes_schedules.rs");
    let (_, bulk) = source
        .split_once("pub(crate) async fn bulk_update_schedule_targets")
        .expect("bulk schedule-target route");
    let (bulk, _) = bulk
        .split_once("pub(crate) async fn enable_schedule")
        .expect("bulk schedule-target route end");
    assert_eq!(
        bulk.matches(".verify_privileges(verification_items)")
            .count(),
        1
    );
    assert!(!bulk.contains("verify_schedule_privilege_for_stored_view("));
}
