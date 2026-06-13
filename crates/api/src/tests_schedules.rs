use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use tokio::sync::broadcast;
use vpsman_common::{AgentCapabilitySnapshot, AgentHello, AgentPrivilegeMode, JobCommand};

use super::*;
use crate::{
    gateway_client::GatewayDispatchClient, repository_ingest::upsert_memory_agent,
    routes_schedules::apply_schedule_now,
};

fn schedule_test_operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "test-operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    }
}

fn shell_schedule_request(name: &str, enabled: bool) -> CreateScheduleRequest {
    CreateScheduleRequest {
        name: name.to_string(),
        operation: JobCommand::Shell {
            argv: vec!["/usr/bin/uptime".to_string()],
            pty: false,
        },
        selector_expression: "tag:edge".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        cron_expr: "0 * * * *".to_string(),
        timezone: "UTC".to_string(),
        enabled,
        catch_up_policy: "run_all_limited".to_string(),
        catch_up_limit: 3,
        retry_delay_secs: 120,
        max_failures: 5,
        privilege_assertion: None,
    }
}

fn schedule_test_state(repo: Repository) -> AppState {
    let (events, _) = broadcast::channel(1);
    AppState {
        repo,
        events,
        internal_token: None,
        gateway: GatewayDispatchClient::test_privilege_auto_approve(),
        backup_object_store: None,
        update_object_store: None,
        update_artifact_public_base_url: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        require_registered_agent_updates: false,
        suite_config_path: std::path::PathBuf::from("config/vpsman.toml"),
        dispatcher_config: crate::state::DispatcherRuntimeConfig::default(),
    }
}

async fn seed_unprivileged_agent(repo: &crate::repository::Repository, client_id: &str) {
    let Repository::Memory(memory) = repo else {
        unreachable!();
    };
    upsert_memory_agent(
        &memory.agents,
        &AgentHello {
            client_id: client_id.to_string(),
            agent_version: "test".to_string(),
            internal_build_number: 1,
            os_release: "test".to_string(),
            arch: "x86_64".to_string(),
            update_heartbeat: None,
            capabilities: AgentCapabilitySnapshot {
                privilege_mode: AgentPrivilegeMode::Unprivileged,
                effective_uid: Some(1000),
                can_attempt_privileged_ops: false,
                unprivileged_hint: Some("running as normal user".to_string()),
                ..Default::default()
            },
        },
    )
    .await;
}

#[tokio::test]
async fn schedule_create_lists_durable_selector_without_plaintext_privilege_material() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = schedule_test_operator();
    let request = shell_schedule_request("nightly-uptime", true);

    validate_schedule_request(&request).unwrap();
    let schedule = repo.create_schedule(request, &operator).await.unwrap();
    let schedules = repo.list_schedules().await.unwrap();
    let audits = repo.list_audit_logs(10).await.unwrap();

    assert_eq!(schedule.name, "nightly-uptime");
    assert_eq!(schedule.command_type, "shell_argv");
    assert_eq!(schedule.selector_expression, "tag:edge");
    assert_eq!(schedule.cron_expr, "0 * * * *");
    assert_eq!(schedule.timezone, "UTC");
    assert_eq!(schedule.next_runs.len(), 5);
    assert_eq!(schedule.catch_up_policy, "run_all_limited");
    assert_eq!(schedule.catch_up_limit, 3);
    assert_eq!(schedule.retry_delay_secs, 120);
    assert_eq!(schedule.max_failures, 5);
    assert_eq!(schedule.failure_count, 0);
    assert_eq!(schedule.last_error, None);
    assert_eq!(schedules.len(), 1);
    assert_eq!(audits[0].action, "schedule.created");
    assert!(!serde_json::to_string(&audits[0].metadata)
        .unwrap()
        .contains("correct horse"));
}

#[tokio::test]
async fn schedule_uuid_lifecycle_hides_soft_deleted_rows() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = schedule_test_operator();
    let first = repo
        .create_schedule(shell_schedule_request("shared-name", true), &operator)
        .await
        .unwrap();
    let second = repo
        .create_schedule(shell_schedule_request("shared-name", true), &operator)
        .await
        .unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(repo.list_schedules().await.unwrap().len(), 2);

    let updated = repo
        .update_schedule_record(
            first.id,
            UpdateScheduleRequest {
                name: "shared-name".to_string(),
                operation: JobCommand::Shell {
                    argv: vec!["/bin/true".to_string()],
                    pty: false,
                },
                selector_expression: "tag:edge && id:client-a".to_string(),
                target_client_ids: vec!["client-a".to_string()],
                cron_expr: "15 * * * *".to_string(),
                timezone: "UTC".to_string(),
                enabled: false,
                catch_up_policy: "skip_missed".to_string(),
                catch_up_limit: 1,
                retry_delay_secs: 60,
                max_failures: 2,
                privilege_assertion: None,
            }
            .into(),
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(updated.id, first.id);
    assert_eq!(updated.name, "shared-name");
    assert_eq!(updated.cron_expr, "15 * * * *");
    assert!(!updated.enabled);

    assert!(
        repo.set_schedule_enabled(first.id, true, &operator)
            .await
            .unwrap()
            .enabled
    );
    assert!(
        !repo
            .set_schedule_enabled(first.id, false, &operator)
            .await
            .unwrap()
            .enabled
    );

    let deferred_until = (chrono::Utc::now() + chrono::Duration::hours(3)).to_rfc3339();
    let deferred = repo
        .defer_schedule(
            first.id,
            &deferred_until,
            Some("maintenance window"),
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(
        deferred.deferred_until.as_deref(),
        Some(deferred_until.as_str())
    );

    repo.soft_delete_schedule(first.id, &operator)
        .await
        .unwrap();
    let visible = repo.list_schedules().await.unwrap();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, second.id);
    assert!(repo.schedule_by_id(first.id).await.is_err());

    let audit_actions = repo
        .list_audit_logs(20)
        .await
        .unwrap()
        .into_iter()
        .map(|entry| entry.action)
        .collect::<Vec<_>>();
    assert!(audit_actions.contains(&"schedule.created".to_string()));
    assert!(audit_actions.contains(&"schedule.updated".to_string()));
    assert!(audit_actions.contains(&"schedule.enabled".to_string()));
    assert!(audit_actions.contains(&"schedule.disabled".to_string()));
    assert!(audit_actions.contains(&"schedule.deferred".to_string()));
    assert!(audit_actions.contains(&"schedule.deleted".to_string()));
}

#[tokio::test]
async fn schedule_apply_now_uses_saved_schedule_without_advancing_next_run() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = schedule_test_operator();
    seed_unprivileged_agent(&repo, "client-a").await;

    let mut request = shell_schedule_request("update-window", true);
    request.selector_expression = "id:client-a".to_string();
    request.operation = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "ab".repeat(32),
        artifact_signature_hex: None,
        artifact_signing_key_hex: None,
    };
    let schedule = repo.create_schedule(request, &operator).await.unwrap();
    let next_run_before = schedule.next_run_at.clone();

    let state = schedule_test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;
    let (status, Json(response)) = apply_schedule_now(State(state), headers, Path(schedule.id))
        .await
        .unwrap();

    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(response.status, "partial_success");
    wait_for_job_status(&repo, response.job_id, "partial_success").await;
    assert_eq!(
        repo.schedule_by_id(schedule.id).await.unwrap().next_run_at,
        next_run_before
    );

    let jobs = repo.list_jobs(10).await.unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, response.job_id);
    assert_eq!(jobs[0].command_type, "agent_update");
    let targets = repo.list_job_targets(response.job_id).await.unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].client_id, "client-a");
    assert_eq!(targets[0].status, "skipped");

    let audits = repo.list_audit_logs(20).await.unwrap();
    let dispatch_audit = audits
        .iter()
        .find(|entry| entry.action == "job.dispatch_requested")
        .expect("missing job dispatch audit for schedule apply-now");
    let schedule_id = schedule.id.to_string();
    assert_eq!(
        dispatch_audit
            .metadata
            .get("source_schedule_id")
            .and_then(serde_json::Value::as_str),
        Some(schedule_id.as_str())
    );
}

async fn wait_for_job_status(
    repo: &crate::repository::Repository,
    job_id: uuid::Uuid,
    expected: &str,
) {
    for _ in 0..50 {
        let jobs = repo.list_jobs(100).await.unwrap();
        if jobs
            .iter()
            .any(|job| job.id == job_id && job.status == expected)
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("job {job_id} did not reach status {expected}");
}

#[test]
fn schedule_validation_rejects_unsafe_or_empty_requests() {
    let mut request = CreateScheduleRequest {
        name: "bad".to_string(),
        operation: JobCommand::Shell {
            argv: vec!["/bin/true".to_string()],
            pty: false,
        },
        selector_expression: "".to_string(),
        target_client_ids: Vec::new(),
        cron_expr: "*/5 * * * *".to_string(),
        timezone: "UTC".to_string(),
        enabled: true,
        catch_up_policy: "skip_missed".to_string(),
        catch_up_limit: 1,
        retry_delay_secs: 300,
        max_failures: 3,
        privilege_assertion: None,
    };

    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
    request.selector_expression = "tag:edge".to_string();
    request.target_client_ids = vec!["client-a".to_string()];
    request.cron_expr = "bad cron".to_string();
    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
    request.cron_expr = "*/5 * * * *".to_string();
    request.timezone = "America/New_York".to_string();
    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
    request.timezone = "UTC".to_string();
    request.operation = JobCommand::Shell {
        argv: vec!["/bin/sh".to_string()],
        pty: true,
    };
    assert!(validate_schedule_request(&request).is_ok());
    request.operation = JobCommand::Shell {
        argv: Vec::new(),
        pty: false,
    };
    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
    request.operation = JobCommand::Shell {
        argv: vec!["/bin/true".to_string()],
        pty: false,
    };
    request.catch_up_policy = "retry_everything".to_string();
    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
    request.catch_up_policy = "skip_missed".to_string();
    request.catch_up_limit = 0;
    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
    request.catch_up_limit = 1;
    request.retry_delay_secs = 0;
    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
    request.retry_delay_secs = 300;
    request.max_failures = 0;
    assert_eq!(
        validate_schedule_request(&request).unwrap_err().status,
        axum::http::StatusCode::BAD_REQUEST
    );
}
