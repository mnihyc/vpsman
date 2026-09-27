use super::*;

#[tokio::test]
async fn postgres_schedule_timeout_survives_backup_policy_edits() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "backup-timeout", None).await;
    let operator = postgres_network_operator(&db.repo).await;
    let request = || CreateBackupPolicyRequest {
        name: "backup-timeout".to_string(),
        selector_expression: String::new(),
        target_client_ids: vec!["backup-timeout".to_string()],
        paths: vec!["/etc/hostname".to_string()],
        include_config: false,
        follow_symlinks: false,
        missing_path_policy: vpsman_common::BackupMissingPathPolicy::Fail,
        retention_days: Some(7),
        keep_last: Some(2),
        rotation_generation: None,
        cron_expr: "0 3 * * *".to_string(),
        timezone: "UTC".to_string(),
        enabled: false,
        catch_up_policy: "skip_missed".to_string(),
        catch_up_limit: 1,
        retry_delay_secs: 120,
        max_failures: 3,
        confirmed: true,
        privilege_assertion: None,
    };
    let policy = db
        .repo
        .create_backup_policy(request(), &operator)
        .await
        .unwrap();
    assert_eq!(
        db.repo
            .schedule_by_id(policy.schedule_id)
            .await
            .unwrap()
            .max_timeout_secs,
        None
    );
    // Simulate a separate Schedule editor save of its execution setting.
    sqlx::query("UPDATE schedules SET max_timeout_secs=7200, definition_revision=definition_revision+1 WHERE id=$1")
        .bind(policy.schedule_id).execute(&db.pool).await.unwrap();
    let saved = db.repo.schedule_by_id(policy.schedule_id).await.unwrap();
    let mut edited = request();
    edited.retention_days = Some(14);
    let updated = db
        .repo
        .update_backup_policy(
            policy.schedule_id,
            edited,
            &crate::repository_schedules::ScheduleSnapshotExpectation {
                selector_expression: saved.selector_expression,
                target_client_ids: saved.target_client_ids,
                definition_revision: saved.definition_revision,
            },
            &operator,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.retention_days, 14);
    assert_eq!(
        db.repo
            .schedule_by_id(policy.schedule_id)
            .await
            .unwrap()
            .max_timeout_secs,
        Some(7200)
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_schedule_timeout_api_rejects_above_cap_before_privilege_and_apply_now_honors_saved(
) {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "timeout-api", None).await;
    let (operator, headers) = postgres_operator_session(&db.repo, "timeout-api-operator").await;
    let mut state = postgres_app_state(&db);
    state.suite_config_path = std::path::PathBuf::from("target/schedule-timeout-no-config.toml");
    state.dispatcher_config.max_job_timeout_secs = 240;
    let mut request = postgres_shell_schedule_request("timeout-api", "timeout-api");
    request.max_timeout_secs = Some(241);
    // No privilege assertion or gateway: exceeding the cap must yield the
    // normal job validation error, not a misleading privilege failure.
    let error = crate::routes_schedules::create_schedule(
        State(state.clone()),
        headers.clone(),
        Json(request),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "max_timeout_exceeds_configured_job_max");

    state.gateway = GatewayDispatchClient::default().with_test_privilege_auto_approve();
    let mut request = postgres_shell_schedule_request("saved-timeout", "timeout-api");
    request.max_timeout_secs = Some(120);
    let (_, Json(schedule)) = crate::routes_schedules::create_schedule(
        State(state.clone()),
        headers.clone(),
        Json(request),
    )
    .await
    .unwrap();
    assert_eq!(schedule.max_timeout_secs, Some(120));
    // Both override execution and a later administrative cap reduction use
    // the same semantics as ordinary scheduled worker materialization.
    for (cap, expected) in [(240, 120_i64), (90, 90_i64)] {
        state.dispatcher_config.max_job_timeout_secs = cap;
        let (_, Json(response)) = crate::routes_schedules::apply_schedule_now(
            State(state.clone()),
            headers.clone(),
            axum::extract::Path(schedule.id),
            Json(crate::model::SchedulePrivilegeMutationRequest {
                expected_definition_revision: schedule.definition_revision,
                privilege_assertion: None,
                confirmed: true,
            }),
        )
        .await
        .unwrap();
        let timeout: i64 = sqlx::query_scalar("SELECT max_timeout_secs FROM jobs WHERE id=$1")
            .bind(response.job_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(timeout, expected);
    }

    let inherited = db
        .repo
        .create_schedule(
            postgres_shell_schedule_request("inherited-api", "timeout-api"),
            &operator,
        )
        .await
        .unwrap();
    let expected = state.schedule_apply_now_max_timeout_secs() as i64;
    let (_, Json(response)) = crate::routes_schedules::apply_schedule_now(
        State(state),
        headers,
        axum::extract::Path(inherited.id),
        Json(crate::model::SchedulePrivilegeMutationRequest {
            expected_definition_revision: inherited.definition_revision,
            privilege_assertion: None,
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    let timeout: i64 = sqlx::query_scalar("SELECT max_timeout_secs FROM jobs WHERE id=$1")
        .bind(response.job_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(timeout, expected);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_schedule_timeout_round_trips_create_update_and_default() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "timeout-target", None).await;
    let operator = postgres_network_operator(&db.repo).await;
    let mut request = postgres_shell_schedule_request("timeout-round-trip", "timeout-target");
    request.max_timeout_secs = Some(7_200);
    let mut schedule = db.repo.create_schedule(request, &operator).await.unwrap();
    assert_eq!(schedule.max_timeout_secs, Some(7_200));
    assert_eq!(
        db.repo
            .schedule_by_id(schedule.id)
            .await
            .unwrap()
            .max_timeout_secs,
        Some(7_200)
    );
    assert!(db
        .repo
        .query_schedules(&ListQuery::default())
        .await
        .unwrap()
        .iter()
        .any(|row| row.id == schedule.id && row.max_timeout_secs == Some(7_200)));

    // A full edit can change the override or explicitly restore inheritance.
    for timeout in [Some(120), None] {
        let expectation = crate::repository_schedules::ScheduleSnapshotExpectation {
            selector_expression: schedule.selector_expression.clone(),
            target_client_ids: schedule.target_client_ids.clone(),
            definition_revision: schedule.definition_revision,
        };
        schedule = db
            .repo
            .update_schedule_record(
                schedule.id,
                crate::repository_schedules::ScheduleCreateInput {
                    max_timeout_secs: timeout,
                    name: schedule.name.clone(),
                    operation: schedule.operation.clone(),
                    event_argv_template: schedule.event_argv_template.clone(),
                    selector_expression: schedule.selector_expression.clone(),
                    target_client_ids: schedule.target_client_ids.clone(),
                    trigger_kind: schedule.trigger_kind,
                    run_on: schedule.run_on,
                    cron_expr: schedule.cron_expr.clone(),
                    timezone: schedule.timezone.clone(),
                    event_expression: schedule.event_expression.clone(),
                    enabled: schedule.enabled,
                    catch_up_policy: schedule.catch_up_policy.clone(),
                    catch_up_limit: schedule.catch_up_limit,
                    retry_delay_secs: schedule.retry_delay_secs,
                    max_failures: schedule.max_failures,
                    expected_definition_revision: Some(schedule.definition_revision),
                },
                Some(&expectation),
                &operator,
            )
            .await
            .unwrap();
        assert_eq!(schedule.max_timeout_secs, timeout);
        let stored: Option<i64> =
            sqlx::query_scalar("SELECT max_timeout_secs FROM schedules WHERE id=$1")
                .bind(schedule.id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(stored, timeout.map(|value| value as i64));
    }

    let inherited = db
        .repo
        .create_schedule(
            postgres_shell_schedule_request("default-timeout", "timeout-target"),
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(inherited.max_timeout_secs, None);
    for invalid in [
        0_i64,
        vpsman_common::MAX_CONFIGURABLE_JOB_TIMEOUT_SECS as i64 + 1,
    ] {
        assert!(
            sqlx::query("UPDATE schedules SET max_timeout_secs=$2 WHERE id=$1")
                .bind(schedule.id)
                .bind(invalid)
                .execute(&db.pool)
                .await
                .is_err()
        );
    }
    db.cleanup().await;
}
