use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use tokio::sync::broadcast;
use vpsman_common::{
    encode_json, payload_hash, AgentCapabilitySnapshot, AgentHello, AgentPrivilegeMode, JobCommand,
};

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
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
        },
        session_id: None,
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
        confirmed: true,
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
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
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
            process_incarnation_id: uuid::Uuid::new_v4(),
            agent_version: "test".to_string(),
            internal_build_number: 1,
            os_release: "test".to_string(),
            arch: "x86_64".to_string(),
            cpu_model: None,
            kernel_release: None,
            virtualization: None,
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

async fn seed_never_connected_agent(repo: &crate::repository::Repository, client_id: &str) {
    let Repository::Memory(memory) = repo else {
        unreachable!();
    };
    memory.agents.write().await.push(AgentView {
        id: client_id.to_string(),
        display_name: client_id.to_string(),
        status: "never".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: AgentCapabilitySnapshot::default(),
    });
}

async fn record_scheduled_memory_job(
    repo: &Repository,
    operator: &AuthContext,
    schedule: &ScheduleView,
    fingerprint_suffix: &str,
) -> Uuid {
    let request = CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: schedule.selector_expression.clone(),
        target_client_ids: schedule.target_client_ids.clone(),
        destructive: false,
        confirmed: true,
        command: "operation".to_string(),
        argv: Vec::new(),
        operation: schedule.operation.clone(),
        max_timeout_secs: Some(30),
        force_unprivileged: false,
        privileged: false,
        privilege_assertion: None,
        rollout: None,
    };
    let command_hash = payload_hash(&encode_json(schedule.operation.as_ref().unwrap()).unwrap());
    repo.record_dispatching_job_from_schedule(
        Uuid::new_v4(),
        &request,
        &command_hash,
        &format!("scheduled_finish_{fingerprint_suffix}"),
        operator,
        &schedule.target_client_ids,
        schedule.id,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn schedule_create_lists_durable_selector_without_plaintext_privilege_material() {
    let repo = Repository::Memory(MemoryState::default());
    seed_never_connected_agent(&repo, "client-a").await;
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
async fn malformed_memory_schedule_stays_visible_blocks_partial_actions_and_can_be_repaired() {
    let repo = Repository::Memory(MemoryState::default());
    seed_never_connected_agent(&repo, "client-a").await;
    let operator = schedule_test_operator();
    let malformed = repo
        .create_schedule(
            shell_schedule_request("malformed-operation", true),
            &operator,
        )
        .await
        .unwrap();
    let healthy = repo
        .create_schedule(shell_schedule_request("healthy-operation", true), &operator)
        .await
        .unwrap();
    let raw_operation = serde_json::json!({"type": "removed_legacy_operation"});
    let raw_hash = payload_hash(raw_operation.to_string().as_bytes());
    let Repository::Memory(memory) = &repo else {
        unreachable!();
    };
    let mut schedules = memory.schedules.write().await;
    let stored = schedules
        .iter_mut()
        .find(|schedule| schedule.id == malformed.id)
        .unwrap();
    stored.operation = None;
    stored.operation_error = Some("schedule_operation_invalid".to_string());
    stored.operation_payload_hash = raw_hash.clone();
    stored.command_type = "invalid_operation".to_string();
    drop(schedules);

    let page = repo
        .query_schedules(&ListQuery {
            limit: Some(10),
            ..ListQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(page.len(), 2);
    assert!(page.iter().any(|schedule| schedule.id == healthy.id));
    let visible = page
        .iter()
        .find(|schedule| schedule.id == malformed.id)
        .unwrap();
    assert!(visible.operation.is_none());
    assert_eq!(
        visible.operation_error.as_deref(),
        Some("schedule_operation_invalid")
    );
    assert_eq!(visible.operation_payload_hash, raw_hash);
    assert!(repo
        .update_schedule_targets(malformed.id, vec!["client-a".to_string()], None, &operator,)
        .await
        .is_err());
    assert!(repo
        .defer_schedule(malformed.id, "2099-01-01T00:00:00Z", None, &operator,)
        .await
        .is_err());
    assert!(repo
        .set_schedule_enabled(malformed.id, true, &operator)
        .await
        .is_err());
    assert!(
        !repo
            .set_schedule_enabled(malformed.id, false, &operator)
            .await
            .unwrap()
            .enabled
    );

    let repaired = repo
        .update_schedule_record(
            malformed.id,
            UpdateScheduleRequest {
                name: "repaired-operation".to_string(),
                operation: JobCommand::Shell {
                    argv: vec!["/bin/true".to_string()],
                    pty: false,
                },
                selector_expression: "tag:edge".to_string(),
                target_client_ids: vec!["client-a".to_string()],
                expected_selector_expression: "tag:edge".to_string(),
                expected_target_client_ids: vec!["client-a".to_string()],
                cron_expr: "0 * * * *".to_string(),
                timezone: "UTC".to_string(),
                enabled: false,
                catch_up_policy: "skip_missed".to_string(),
                catch_up_limit: 1,
                retry_delay_secs: 60,
                max_failures: 3,
                privilege_assertion: None,
                confirmed: true,
            }
            .into(),
            None,
            &operator,
        )
        .await
        .unwrap();
    assert!(repaired.operation.is_some());
    assert!(repaired.operation_error.is_none());
}

#[tokio::test]
async fn tag_impact_preview_includes_schedules_beyond_one_thousand() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = schedule_test_operator();
    seed_never_connected_agent(&repo, "client-a").await;
    let schedule = repo
        .create_schedule(
            shell_schedule_request("edge-schedule-0000", true),
            &operator,
        )
        .await
        .unwrap();
    if let Repository::Memory(memory) = &repo {
        let mut schedules = memory.schedules.write().await;
        schedules.extend((1..=1_000).map(|index| {
            let mut schedule = schedule.clone();
            schedule.id = Uuid::new_v4();
            schedule.name = format!("edge-schedule-{index:04}");
            schedule
        }));
    }

    let preview = repo
        .assign_agent_tag_mutation("client-a", "edge", false, false)
        .await
        .unwrap();

    assert_eq!(preview.schedule_impacts.len(), 1_001);
    assert!(preview.confirmation_required);
}

#[tokio::test]
async fn tag_impact_preview_checks_rule_scope_at_the_schedule_repository_boundary() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = schedule_test_operator();
    seed_never_connected_agent(&repo, "client-a").await;
    let schedule = repo
        .create_schedule(shell_schedule_request("rule-schedule", true), &operator)
        .await
        .unwrap();
    if let Repository::Memory(memory) = &repo {
        memory
            .schedules
            .write()
            .await
            .iter_mut()
            .find(|stored| stored.id == schedule.id)
            .unwrap()
            .selector_expression = "vps.rules:traffic.reset_day".to_string();
    }

    let denied = repo
        .assign_agent_tag_mutation("client-a", "edge", false, false)
        .await
        .unwrap_err();
    assert!(denied
        .to_string()
        .contains("vps_rule_selector_scope_required"));
    assert!(repo
        .assign_agent_tag_mutation("client-a", "edge", false, true)
        .await
        .is_ok());
}

#[tokio::test]
async fn public_schedule_list_is_bounded_while_internal_listing_remains_complete() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = schedule_test_operator();
    seed_never_connected_agent(&repo, "client-a").await;
    let schedule = repo
        .create_schedule(
            shell_schedule_request("public-list-schedule-0000", true),
            &operator,
        )
        .await
        .unwrap();
    if let Repository::Memory(memory) = &repo {
        let mut schedules = memory.schedules.write().await;
        schedules.extend((1..=1_000).map(|index| {
            let mut schedule = schedule.clone();
            schedule.id = Uuid::new_v4();
            schedule.name = format!("public-list-schedule-{index:04}");
            schedule
        }));
    }
    let state = schedule_test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;

    let Json(default_page) = crate::routes_schedules::list_schedules(
        State(state.clone()),
        headers.clone(),
        Query(ListQuery::default()),
    )
    .await
    .unwrap();
    let Json(max_page) = crate::routes_schedules::list_schedules(
        State(state),
        headers,
        Query(ListQuery {
            limit: Some(10_000),
            ..ListQuery::default()
        }),
    )
    .await
    .unwrap();

    assert_eq!(default_page.len(), 100);
    assert_eq!(max_page.len(), 1_000);
    assert_eq!(repo.list_schedules().await.unwrap().len(), 1_001);
}

#[tokio::test]
async fn schedule_mutations_require_explicit_confirmation() {
    let repo = Repository::Memory(MemoryState::default());
    let state = schedule_test_state(repo);
    let headers = crate::test_auth_headers(&state).await;
    let mut request = shell_schedule_request("unconfirmed-schedule", true);
    request.confirmed = false;

    let error = crate::routes_schedules::create_schedule(State(state), headers, Json(request))
        .await
        .unwrap_err();

    assert_eq!(error.status, StatusCode::CONFLICT);
    assert_eq!(error.code, "schedule_mutation_requires_confirmation");
}

#[tokio::test]
async fn unchanged_schedule_selector_preserves_frozen_targets_until_manual_update() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = schedule_test_operator();
    seed_never_connected_agent(&repo, "client-a").await;
    seed_never_connected_agent(&repo, "client-b").await;
    if let Repository::Memory(memory) = &repo {
        for agent in memory.agents.write().await.iter_mut() {
            agent.tags = vec!["edge".to_string()];
        }
    }
    let schedule = repo
        .create_schedule(shell_schedule_request("frozen-edge", true), &operator)
        .await
        .unwrap();
    assert_eq!(schedule.target_client_ids, vec!["client-a"]);
    repo.delete_agent("client-a", Some("retired"), &operator)
        .await
        .unwrap();
    let state = schedule_test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;

    let Json(updated) = crate::routes_schedules::update_schedule(
        State(state.clone()),
        headers.clone(),
        Path(schedule.id),
        Json(UpdateScheduleRequest {
            name: "frozen-edge-renamed".to_string(),
            operation: schedule.operation.clone().unwrap(),
            selector_expression: schedule.selector_expression.clone(),
            target_client_ids: schedule.target_client_ids.clone(),
            expected_selector_expression: schedule.selector_expression.clone(),
            expected_target_client_ids: schedule.target_client_ids.clone(),
            cron_expr: schedule.cron_expr.clone(),
            timezone: schedule.timezone.clone(),
            enabled: schedule.enabled,
            catch_up_policy: schedule.catch_up_policy.clone(),
            catch_up_limit: schedule.catch_up_limit,
            retry_delay_secs: schedule.retry_delay_secs,
            max_failures: schedule.max_failures,
            privilege_assertion: None,
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    assert_eq!(updated.target_client_ids, vec!["client-a"]);

    let Json(retargeted) = crate::routes_schedules::update_schedule_targets(
        State(state.clone()),
        headers.clone(),
        Path(schedule.id),
        Json(UpdateScheduleTargetsRequest {
            privilege_assertion: None,
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    assert_eq!(retargeted.target_client_ids, vec!["client-b"]);

    let error = crate::routes_schedules::update_schedule_targets(
        State(state),
        headers,
        Path(schedule.id),
        Json(UpdateScheduleTargetsRequest {
            privilege_assertion: None,
            confirmed: true,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::CONFLICT);
    assert_eq!(error.code, "schedule_targets_already_current");
}

#[tokio::test]
async fn schedule_target_update_persists_an_empty_resolution() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = schedule_test_operator();
    seed_never_connected_agent(&repo, "client-a").await;
    let mut request = shell_schedule_request("empty-retarget", true);
    request.selector_expression = "tag:missing".to_string();
    let schedule = repo.create_schedule(request, &operator).await.unwrap();
    let state = schedule_test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;

    let Json(updated) = crate::routes_schedules::update_schedule_targets(
        State(state.clone()),
        headers.clone(),
        Path(schedule.id),
        Json(UpdateScheduleTargetsRequest {
            privilege_assertion: None,
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    assert!(updated.target_client_ids.is_empty());
    let Json(edited) = crate::routes_schedules::update_schedule(
        State(state),
        headers,
        Path(schedule.id),
        Json(UpdateScheduleRequest {
            name: "empty-retarget-renamed".to_string(),
            operation: updated.operation.clone().unwrap(),
            selector_expression: updated.selector_expression.clone(),
            target_client_ids: Vec::new(),
            expected_selector_expression: updated.selector_expression.clone(),
            expected_target_client_ids: Vec::new(),
            cron_expr: updated.cron_expr.clone(),
            timezone: updated.timezone.clone(),
            enabled: updated.enabled,
            catch_up_policy: updated.catch_up_policy.clone(),
            catch_up_limit: updated.catch_up_limit,
            retry_delay_secs: updated.retry_delay_secs,
            max_failures: updated.max_failures,
            privilege_assertion: None,
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    assert_eq!(edited.name, "empty-retarget-renamed");
    assert!(edited.target_client_ids.is_empty());
    assert!(repo
        .schedule_by_id(schedule.id)
        .await
        .unwrap()
        .target_client_ids
        .is_empty());
}

#[tokio::test]
async fn concurrent_schedule_edit_cannot_overwrite_retargeted_clients() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = schedule_test_operator();
    seed_never_connected_agent(&repo, "client-a").await;
    seed_never_connected_agent(&repo, "client-b").await;
    let schedule = repo
        .create_schedule(
            shell_schedule_request("concurrent-snapshot", true),
            &operator,
        )
        .await
        .unwrap();
    let expectation = crate::repository_schedules::ScheduleSnapshotExpectation {
        selector_expression: schedule.selector_expression.clone(),
        target_client_ids: schedule.target_client_ids.clone(),
    };
    let update = crate::repository_schedules::ScheduleCreateInput {
        name: "concurrent-snapshot-edited".to_string(),
        operation: schedule.operation.clone().unwrap(),
        selector_expression: schedule.selector_expression.clone(),
        target_client_ids: schedule.target_client_ids.clone(),
        cron_expr: schedule.cron_expr.clone(),
        timezone: schedule.timezone.clone(),
        enabled: schedule.enabled,
        catch_up_policy: schedule.catch_up_policy.clone(),
        catch_up_limit: schedule.catch_up_limit,
        retry_delay_secs: schedule.retry_delay_secs,
        max_failures: schedule.max_failures,
    };

    let (edit_result, retarget_result) = tokio::join!(
        repo.update_schedule_record(schedule.id, update, Some(&expectation), &operator),
        repo.update_schedule_targets(
            schedule.id,
            vec!["client-b".to_string()],
            Some(&expectation),
            &operator,
        ),
    );
    assert!(retarget_result.is_ok());
    if let Err(error) = edit_result {
        assert!(error.to_string().contains("schedule_snapshot_stale"));
    }
    assert_eq!(
        repo.schedule_by_id(schedule.id)
            .await
            .unwrap()
            .target_client_ids,
        vec!["client-b".to_string()]
    );
}

#[tokio::test]
async fn schedule_uuid_lifecycle_hides_soft_deleted_rows() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = schedule_test_operator();
    seed_never_connected_agent(&repo, "client-a").await;
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
                expected_selector_expression: "tag:edge".to_string(),
                expected_target_client_ids: vec!["client-a".to_string()],
                cron_expr: "15 * * * *".to_string(),
                timezone: "UTC".to_string(),
                enabled: false,
                catch_up_policy: "skip_missed".to_string(),
                catch_up_limit: 1,
                retry_delay_secs: 60,
                max_failures: 2,
                privilege_assertion: None,
                confirmed: true,
            }
            .into(),
            None,
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
async fn scheduled_failed_job_updates_retry_controls_on_finish() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = schedule_test_operator();
    seed_never_connected_agent(&repo, "client-a").await;
    let mut schedule_request = shell_schedule_request("retrying-schedule", true);
    schedule_request.max_failures = 2;
    schedule_request.retry_delay_secs = 120;
    let schedule = repo
        .create_schedule(schedule_request, &operator)
        .await
        .unwrap();

    let job_id = record_scheduled_memory_job(&repo, &operator, &schedule, "first").await;
    repo.finish_job(job_id, "failed").await.unwrap();
    let failed_once = repo.schedule_by_id(schedule.id).await.unwrap();
    assert_eq!(failed_once.failure_count, 1);
    assert_eq!(failed_once.last_error.as_deref(), Some("failed"));
    assert!(failed_once.enabled);
    assert_ne!(failed_once.next_run_at, schedule.next_run_at);
    let first_job_id = job_id.to_string();
    let Repository::Memory(memory) = &repo else {
        unreachable!();
    };
    assert!(memory
        .webhook_events
        .read()
        .await
        .iter()
        .any(|event| event.kind == "schedule.failed"
            && event.payload["schedule"]["last_job_id"].as_str() == Some(first_job_id.as_str())));
    assert!(memory
        .audits
        .read()
        .await
        .iter()
        .any(|audit| audit.action == "schedule.job_failed"
            && audit.metadata["job_id"].as_str() == Some(first_job_id.as_str())));

    let job_id = record_scheduled_memory_job(&repo, &operator, &schedule, "second").await;
    repo.finish_job(job_id, "failed").await.unwrap();
    let failed_twice = repo.schedule_by_id(schedule.id).await.unwrap();
    assert_eq!(failed_twice.failure_count, 2);
    assert_eq!(failed_twice.last_error.as_deref(), Some("failed"));
    assert!(!failed_twice.enabled);
}

#[tokio::test]
async fn scheduled_job_finish_is_idempotent_for_failure_accounting() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = schedule_test_operator();
    seed_never_connected_agent(&repo, "client-a").await;
    let schedule = repo
        .create_schedule(shell_schedule_request("idempotent-finish", true), &operator)
        .await
        .unwrap();
    let job_id = record_scheduled_memory_job(&repo, &operator, &schedule, "failed-once").await;

    assert!(repo.finish_job(job_id, "failed").await.unwrap());
    assert!(!repo.finish_job(job_id, "failed").await.unwrap());
    assert_eq!(
        repo.refresh_job_status_from_targets(job_id).await.unwrap(),
        None
    );

    let failed_once = repo.schedule_by_id(schedule.id).await.unwrap();
    assert_eq!(failed_once.failure_count, 1);
    let Repository::Memory(memory) = &repo else {
        unreachable!();
    };
    let job_id_string = job_id.to_string();
    let schedule_failed_events = memory
        .webhook_events
        .read()
        .await
        .iter()
        .filter(|event| {
            event.kind == "schedule.failed"
                && event.payload["schedule"]["last_job_id"].as_str() == Some(job_id_string.as_str())
        })
        .count();
    assert_eq!(schedule_failed_events, 1);
    let schedule_failed_audits = memory
        .audits
        .read()
        .await
        .iter()
        .filter(|audit| {
            audit.action == "schedule.job_failed"
                && audit.metadata["job_id"].as_str() == Some(job_id_string.as_str())
        })
        .count();
    assert_eq!(schedule_failed_audits, 1);
}

#[tokio::test]
async fn scheduled_successful_job_resets_failure_controls_on_finish() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = schedule_test_operator();
    seed_never_connected_agent(&repo, "client-a").await;
    let schedule = repo
        .create_schedule(
            shell_schedule_request("recovering-schedule", true),
            &operator,
        )
        .await
        .unwrap();

    let job_id = record_scheduled_memory_job(&repo, &operator, &schedule, "failed").await;
    repo.finish_job(job_id, "failed").await.unwrap();
    assert_eq!(
        repo.schedule_by_id(schedule.id)
            .await
            .unwrap()
            .failure_count,
        1
    );

    let job_id = record_scheduled_memory_job(&repo, &operator, &schedule, "completed").await;
    repo.finish_job(job_id, "completed").await.unwrap();
    let recovered = repo.schedule_by_id(schedule.id).await.unwrap();
    assert_eq!(recovered.failure_count, 0);
    assert_eq!(recovered.last_error, None);
}

#[tokio::test]
async fn scheduled_partial_success_and_skipped_preserve_failure_controls_on_finish() {
    for (status, expected_failure_count, expected_last_error) in [
        ("partial_success", 1, Some("failed")),
        ("skipped", 1, Some("failed")),
    ] {
        let repo = Repository::Memory(MemoryState::default());
        let operator = schedule_test_operator();
        seed_never_connected_agent(&repo, "client-a").await;
        let schedule = repo
            .create_schedule(
                shell_schedule_request(&format!("recovering-schedule-{status}"), true),
                &operator,
            )
            .await
            .unwrap();

        let job_id =
            record_scheduled_memory_job(&repo, &operator, &schedule, &format!("{status}-failed"))
                .await;
        repo.finish_job(job_id, "failed").await.unwrap();
        assert_eq!(
            repo.schedule_by_id(schedule.id)
                .await
                .unwrap()
                .failure_count,
            1
        );

        let job_id = record_scheduled_memory_job(&repo, &operator, &schedule, status).await;
        repo.finish_job(job_id, status).await.unwrap();
        let recovered = repo.schedule_by_id(schedule.id).await.unwrap();
        assert_eq!(recovered.failure_count, expected_failure_count);
        assert_eq!(recovered.last_error.as_deref(), expected_last_error);
        assert!(recovered.enabled);
    }
}

#[tokio::test]
async fn scheduled_partial_success_with_failed_target_counts_as_failure() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = schedule_test_operator();
    seed_never_connected_agent(&repo, "client-a").await;
    let schedule = repo
        .create_schedule(shell_schedule_request("degraded-schedule", true), &operator)
        .await
        .unwrap();
    let job_id = record_scheduled_memory_job(&repo, &operator, &schedule, "partial-failed").await;
    let Repository::Memory(memory) = &repo else {
        unreachable!();
    };
    let mut targets = memory.job_targets.write().await;
    let target = targets
        .iter_mut()
        .find(|target| target.job_id == job_id)
        .unwrap();
    target.status = "failed".to_string();
    target.completed_at = Some(crate::util::unix_now().to_string());
    drop(targets);

    repo.finish_job(job_id, "partial_success").await.unwrap();
    let recovered = repo.schedule_by_id(schedule.id).await.unwrap();
    assert_eq!(recovered.failure_count, 1);
    assert_eq!(recovered.last_error.as_deref(), Some("failed"));
    assert!(recovered.enabled);
}

#[tokio::test]
async fn schedule_apply_now_allows_disabled_schedule_without_advancing_next_run() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = schedule_test_operator();
    seed_unprivileged_agent(&repo, "client-a").await;

    let mut request = shell_schedule_request("update-window", false);
    request.selector_expression = "id:client-a".to_string();
    request.operation = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "ab".repeat(32),
    };
    let schedule = repo.create_schedule(request, &operator).await.unwrap();
    let next_run_before = schedule.next_run_at.clone();

    let state = schedule_test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;
    let (status, Json(response)) = apply_schedule_now(
        State(state),
        headers,
        Path(schedule.id),
        Json(SchedulePrivilegeMutationRequest {
            privilege_assertion: None,
            confirmed: true,
        }),
    )
    .await
    .unwrap();

    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(response.status, "skipped");
    wait_for_job_status(&repo, response.job_id, "skipped").await;
    assert_eq!(
        repo.schedule_by_id(schedule.id).await.unwrap().next_run_at,
        next_run_before
    );
    assert!(!repo.schedule_by_id(schedule.id).await.unwrap().enabled);

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
    assert_eq!(
        dispatch_audit.metadata["target_client_ids"],
        serde_json::json!(["client-a"])
    );
    assert!(dispatch_audit.metadata.get("resolved_targets").is_none());
}

#[tokio::test]
async fn schedule_apply_now_skips_saved_fixed_target_that_no_longer_resolves() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = schedule_test_operator();
    seed_unprivileged_agent(&repo, "client-a").await;

    let mut request = shell_schedule_request("stale-target-window", true);
    request.selector_expression = "id:client-a".to_string();
    let schedule = repo.create_schedule(request, &operator).await.unwrap();
    if let Repository::Memory(memory) = &repo {
        memory
            .hidden_clients
            .write()
            .await
            .insert("client-a".to_string());
    }

    let state = schedule_test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;
    let (status, Json(response)) = apply_schedule_now(
        State(state),
        headers,
        Path(schedule.id),
        Json(SchedulePrivilegeMutationRequest {
            privilege_assertion: None,
            confirmed: true,
        }),
    )
    .await
    .unwrap();

    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(response.status, "skipped");
    assert_eq!(response.target_counts.total, 1);
    assert_eq!(response.target_counts.skipped, 1);
    let targets = repo.list_job_targets(response.job_id).await.unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].client_id, "client-a");
    assert_eq!(targets[0].status, "skipped");
    assert_eq!(
        targets[0].message.as_deref(),
        Some("fixed_target_unavailable: saved schedule target no longer resolves to a dispatchable VPS; target skipped")
    );
    let outputs = repo.list_job_outputs(response.job_id).await.unwrap();
    let output_bytes = BASE64_STANDARD.decode(&outputs[0].data_base64).unwrap();
    let output: serde_json::Value = serde_json::from_slice(&output_bytes).unwrap();
    assert_eq!(output["type"], "fixed_target_unavailable");
    assert_eq!(output["reason"], "fixed_target_unavailable");
}

#[tokio::test]
async fn saved_schedule_job_skips_never_connected_targets_immediately() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = schedule_test_operator();
    seed_never_connected_agent(&repo, "client-never").await;
    let state = schedule_test_state(repo.clone());
    let request = CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: "id:client-never".to_string(),
        target_client_ids: vec!["client-never".to_string()],
        destructive: false,
        confirmed: true,
        command: "true".to_string(),
        argv: vec!["true".to_string()],
        operation: None,
        max_timeout_secs: Some(30),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
    };

    let (status, Json(response)) = crate::routes_jobs::create_job_from_saved_schedule(
        &state,
        &operator,
        request,
        Uuid::new_v4(),
    )
    .await
    .unwrap();

    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(response.status, "skipped");
    assert_eq!(response.target_counts.total, 1);
    assert_eq!(response.target_counts.skipped, 1);
    let targets = repo.list_job_targets(response.job_id).await.unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].client_id, "client-never");
    assert_eq!(targets[0].status, "skipped");
    assert_eq!(
        targets[0].message.as_deref(),
        Some("target_never_connected: target has never connected; job skipped")
    );

    let outputs = repo.list_job_outputs(response.job_id).await.unwrap();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].client_id, "client-never");
    assert_eq!(outputs[0].stream, "status");
    assert!(outputs[0].done);
    let output_bytes = BASE64_STANDARD.decode(&outputs[0].data_base64).unwrap();
    let output: serde_json::Value = serde_json::from_slice(&output_bytes).unwrap();
    assert_eq!(output["type"], "target_never_connected");
    assert_eq!(output["reason"], "target_never_connected");
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
        confirmed: true,
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
    request.cron_expr = "0 0 31 2 *".to_string();
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

#[test]
fn schedule_validation_rejects_agent_lifecycle_commands() {
    for operation in [JobCommand::AgentStop, JobCommand::AgentRestart] {
        let mut request = shell_schedule_request("invalid-agent-lifecycle", true);
        request.operation = operation;
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
        operation: JobCommand::Shell {
            argv: vec!["/bin/true".to_string()],
            pty: false,
        },
        selector_expression: "tag:edge".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        expected_selector_expression: "tag:edge".to_string(),
        expected_target_client_ids: vec!["client-a".to_string()],
        cron_expr: "0 0 31 2 *".to_string(),
        timezone: "UTC".to_string(),
        enabled: true,
        catch_up_policy: "skip_missed".to_string(),
        catch_up_limit: 1,
        retry_delay_secs: 300,
        max_failures: 3,
        privilege_assertion: None,
        confirmed: true,
    };

    let error = crate::routes_schedules::validate_update_schedule_request(&request).unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "schedule_cron_invalid");
}
