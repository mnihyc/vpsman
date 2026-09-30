use super::*;
use vpsman_common::{JOB_STATUS_QUEUED, JOB_STATUS_RUNNING};

#[tokio::test]
async fn postgres_default_traffic_policy_keeps_episode_across_disconnect_and_reconnect() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client = "traffic-restart";
    insert_client(&db.pool, client, Some(Uuid::new_v4())).await;
    let rule = Uuid::parse_str("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb3").unwrap();
    sqlx::query("UPDATE policy_groups SET enabled=TRUE WHERE id=(SELECT group_id FROM policy_rules WHERE id=$1)").bind(rule).execute(&db.pool).await.unwrap();
    crate::repository_telemetry_policy_activation::reconcile_and_settle_telemetry_policy_activation_for_test(&db.repo).await.unwrap();
    let mut episode = None;
    for (index, status, complete) in [
        (0, "online", true),
        (1, "online", true),
        (2, "disconnected", false),
        (3, "online", true),
    ] {
        sqlx::query("UPDATE clients SET status=$2 WHERE id=$1")
            .bind(client)
            .bind(status)
            .execute(&db.pool)
            .await
            .unwrap();
        let now = Utc::now();
        record_test_policy_fact(
            &db.pool,
            crate::repository_policy_lifecycle::PolicyEvidenceFact {
                source_kind: "telemetry.combined".into(),
                source_event_id: format!("restart-{index}"),
                fact_kind: AlertPolicyRuleKind::Metric,
                natural_key: client.into(),
                confirmation_bucket_key: client.into(),
                subject_client_id: Some(client.into()),
                target_kind: "client".into(),
                target_id: client.into(),
                source_status: if complete { "complete" } else { "unknown" }.into(),
                complete,
                subject_snapshot: json!({}),
                payload: json!({"traffic":{"cycle_percent":96.0}}),
                observed_at: now,
                state_started_at: Some(now),
                causation_id: None,
                schedule_lineage: vec![],
            },
        )
        .await;
        if index == 0 {
            // Cross the built-in 300s dwell with a fresh second sample, without
            // sleeping or changing the seeded trigger/resolve expressions.
            sqlx::query("UPDATE alert_policy_evaluation_states SET trigger_segment_started_at=clock_timestamp()-interval '301 seconds' WHERE policy_rule_id=$1").bind(rule).execute(&db.pool).await.unwrap();
        } else {
            let active: Uuid = sqlx::query_scalar(
                "SELECT id FROM alert_episodes WHERE policy_rule_id=$1 AND resolved_at IS NULL",
            )
            .bind(rule)
            .fetch_one(&db.pool)
            .await
            .unwrap();
            assert_eq!(*episode.get_or_insert(active), active);
            let edges: Vec<String> = sqlx::query_scalar("SELECT edge_kind FROM alert_lifecycle_events WHERE episode_id=$1 ORDER BY event_seq").bind(active).fetch_all(&db.pool).await.unwrap();
            assert_eq!(edges, vec!["alert.triggered"]);
        }
    }
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_schedule_last_result_reads_the_recorded_event_job() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client = "failure-policy-client";
    insert_client(&db.pool, client, Some(Uuid::new_v4())).await;
    let operator = postgres_network_operator(&db.repo).await;
    let mut request = postgres_shell_schedule_request("event-results", client);
    request.trigger_kind = ScheduleTriggerKind::Event;
    request.operation = None;
    request.cron_expr = None;
    request.timezone = None;
    request.catch_up_policy = None;
    request.catch_up_limit = None;
    request.retry_delay_secs = None;
    request.event_expression = Some("alert.triggered".to_string());
    let schedule = db.repo.create_schedule(request, &operator).await.unwrap();
    assert!(schedule.last_job_id.is_none());
    let job = Uuid::new_v4();
    insert_job_target_with_operation(
        &db.pool,
        job,
        client,
        JobCommand::Shell {
            argv: vec!["/bin/true".into()],
            pty: false,
        },
        "shell",
        Some(schedule.id),
        JOB_STATUS_QUEUED,
        false,
        Some(Uuid::new_v4()),
        30,
        false,
    )
    .await;
    // This is the pointer written by the receipt's dispatch owner. Cron timing
    // intentionally remains NULL for an event schedule.
    sqlx::query("UPDATE schedules SET last_job_id=$2,last_job_status='queued' WHERE id=$1")
        .bind(schedule.id)
        .bind(job)
        .execute(&db.pool)
        .await
        .unwrap();
    for status in [JOB_STATUS_QUEUED, JOB_STATUS_RUNNING, JOB_STATUS_COMPLETED] {
        sqlx::query("UPDATE jobs SET status=$2,completed_at=CASE WHEN $2='completed' THEN clock_timestamp() ELSE NULL END WHERE id=$1").bind(job).bind(status).execute(&db.pool).await.unwrap();
        let view = db.repo.schedule_by_id(schedule.id).await.unwrap();
        let listed = db
            .repo
            .list_schedules()
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.id == schedule.id)
            .unwrap();
        for view in [view, listed] {
            assert!(view.last_run_at.is_none());
            assert_eq!(view.last_job_id, Some(job));
            assert_eq!(view.last_job_status.as_deref(), Some(status));
            assert!(view.last_job_created_at.is_some());
            assert_eq!(
                view.last_job_completed_at.is_some(),
                status == JOB_STATUS_COMPLETED
            );
        }
    }
    let failed = finish_scheduled_job(&db, schedule.id, JOB_STATUS_FAILED).await;
    let view = db.repo.schedule_by_id(schedule.id).await.unwrap();
    assert_eq!(view.last_job_id, Some(failed));
    assert_eq!(view.last_job_error.as_deref(), Some("failed"));
    assert!(view.last_job_completed_at.is_some());
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_schedule_failure_tolerance_migration_preserves_existing_values() {
    let Ok(base_url) = std::env::var("VPSMAN_TEST_POSTGRES_URL") else {
        eprintln!("skipping migration test: VPSMAN_TEST_POSTGRES_URL is unset");
        return;
    };
    let options = PgConnectOptions::from_str(&base_url).unwrap();
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone().database("postgres"))
        .await
        .unwrap();
    let db_name = format!("vpsman_reliability_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {}", quote_ident(&db_name)))
        .execute(&admin_pool)
        .await
        .unwrap();
    let database_options = options.database(&db_name);
    let mut connection = sqlx::PgConnection::connect_with(&database_options)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE SCHEMA vpsman_internal; SET search_path=vpsman_internal,public;")
        .execute(&mut connection)
        .await
        .unwrap();
    let mut baseline = sqlx::migrate::Migrator::new(workspace_migrations_dir())
        .await
        .unwrap();
    baseline.migrations = std::borrow::Cow::Owned(
        baseline
            .iter()
            .filter(|migration| migration.version < 25)
            .cloned()
            .collect(),
    );
    baseline.run(&mut connection).await.unwrap();
    connection.close().await.unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(database_options.clone())
        .await
        .unwrap();
    let db = PgReliabilityTestDb {
        repo: Repository::Postgres(pool.clone()),
        pool,
        admin_pool,
        db_name,
    };
    let operation = json!({"type": "shell", "argv": ["/bin/true"], "pty": false});
    for (cutoff, failures, enabled) in [(1, 0, true), (3, 2, true), (100, 100, false)] {
        sqlx::query(
            "INSERT INTO schedules (id,name,operation,selector_expression,target_client_ids,cron_expr,next_run_at,max_failures,failure_count,enabled,definition_revision) VALUES ($1,$2,$3,'id:legacy-client',ARRAY['legacy-client'],'* * * * *',now(),$4,$5,$6,7)"
        ).bind(Uuid::new_v4()).bind(format!("legacy-cutoff-{cutoff}"))
            .bind(SqlJson(&operation)).bind(cutoff).bind(failures).bind(enabled)
            .execute(&db.pool).await.unwrap();
    }
    let snapshot = "SELECT to_jsonb(schedule) FROM schedules schedule ORDER BY name";
    let before = sqlx::query_scalar::<_, SqlJson<Value>>(snapshot)
        .fetch_all(&db.pool)
        .await
        .unwrap();
    let custom_policy = Uuid::new_v4();
    sqlx::query("INSERT INTO policy_groups(id,name,enabled,selector_expression) VALUES($1,'custom-policy',TRUE,'status:online')")
        .bind(custom_policy).execute(&db.pool).await.unwrap();
    let policy_snapshot =
        "SELECT to_jsonb(policy) - 'selector_expression' FROM policy_groups policy ORDER BY id";
    let policies_before: Vec<SqlJson<Value>> = sqlx::query_scalar(policy_snapshot)
        .fetch_all(&db.pool)
        .await
        .unwrap();
    let rules_before: Vec<SqlJson<Value>> =
        sqlx::query_scalar("SELECT to_jsonb(rule) FROM policy_rules rule ORDER BY id")
            .fetch_all(&db.pool)
            .await
            .unwrap();
    let existing_webhook = Uuid::new_v4();
    sqlx::query("INSERT INTO webhook_rules(id,name,expression,target,body_template) VALUES($1,'existing','alert.triggered','https://hooks.example.invalid/test','')")
        .bind(existing_webhook).execute(&db.pool).await.unwrap();
    for _ in 0..2 {
        // Migrations and subsequent restarts preserve stored values.
        crate::repository::migrate_postgres_database(
            &database_options,
            &workspace_migrations_dir(),
        )
        .await
        .unwrap();
        let limits: Vec<i32> =
            sqlx::query_scalar("SELECT max_failures FROM schedules ORDER BY max_failures")
                .fetch_all(&db.pool)
                .await
                .unwrap();
        assert_eq!(limits, vec![1, 3, 100]);
        let after = sqlx::query_scalar::<_, SqlJson<Value>>(snapshot)
            .fetch_all(&db.pool)
            .await
            .unwrap();
        assert_eq!(before, after, "migration changed existing schedule values");
        let policies_after: Vec<SqlJson<Value>> = sqlx::query_scalar(policy_snapshot)
            .fetch_all(&db.pool)
            .await
            .unwrap();
        assert_eq!(policies_before, policies_after);
        let rules_after: Vec<SqlJson<Value>> =
            sqlx::query_scalar("SELECT to_jsonb(rule) FROM policy_rules rule ORDER BY id")
                .fetch_all(&db.pool)
                .await
                .unwrap();
        assert_eq!(rules_before, rules_after);
        let scopes: Vec<(Uuid, String)> =
            sqlx::query_as("SELECT id,selector_expression FROM policy_groups")
                .fetch_all(&db.pool)
                .await
                .unwrap();
        assert_eq!(scopes.len(), 6);
        for (id, selector) in scopes {
            assert_eq!(
                selector,
                if id == custom_policy {
                    "status:online"
                } else {
                    "*"
                }
            );
        }
        let cooldown: i64 =
            sqlx::query_scalar("SELECT cooldown_secs FROM webhook_rules WHERE id=$1")
                .bind(existing_webhook)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(cooldown, 300, "existing webhook cooldown changed");
    }
    let cooldown: i64 = sqlx::query_scalar("INSERT INTO webhook_rules(id,name,expression,target,body_template) VALUES($1,'new','alert.triggered','https://hooks.example.invalid/test','') RETURNING cooldown_secs")
        .bind(Uuid::new_v4()).fetch_one(&db.pool).await.unwrap();
    assert_eq!(cooldown, 0);
    let default_id = Uuid::new_v4();
    let default: i32 = sqlx::query_scalar(
        "INSERT INTO schedules(id,name,operation,selector_expression,target_client_ids,cron_expr,next_run_at) VALUES($1,'new-default',$2,'id:legacy-client',ARRAY['legacy-client'],'* * * * *',now()) RETURNING max_failures"
    ).bind(default_id).bind(SqlJson(operation)).fetch_one(&db.pool).await.unwrap();
    assert_eq!(default, -1);
    for allowed in [-1, 0, 100] {
        sqlx::query("UPDATE schedules SET max_failures=$2 WHERE id=$1")
            .bind(default_id)
            .bind(allowed)
            .execute(&db.pool)
            .await
            .unwrap();
    }
    for rejected in [-2, 101] {
        let error = sqlx::query("UPDATE schedules SET max_failures=$2 WHERE id=$1")
            .bind(default_id)
            .bind(rejected)
            .execute(&db.pool)
            .await
            .unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().constraint(),
            Some("schedules_max_failures_check")
        );
    }
    db.cleanup().await;
}

async fn finish_scheduled_job(db: &PgReliabilityTestDb, schedule_id: Uuid, status: &str) -> Uuid {
    let job_id = Uuid::new_v4();
    insert_job_target_with_operation(
        &db.pool,
        job_id,
        "failure-policy-client",
        JobCommand::Shell {
            argv: vec!["/bin/true".to_string()],
            pty: false,
        },
        "shell",
        Some(schedule_id),
        status,
        true,
        Some(Uuid::new_v4()),
        30,
        false,
    )
    .await;
    sqlx::query("UPDATE jobs SET status=$2,completed_at=clock_timestamp() WHERE id=$1")
        .bind(job_id)
        .bind(status)
        .execute(&db.pool)
        .await
        .unwrap();
    db.repo
        .record_job_terminal_side_effects(job_id, status)
        .await
        .unwrap();
    job_id
}

#[tokio::test]
async fn postgres_schedule_failure_tolerance_controls_job_outcomes_and_preserves_manual_pauses() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "failure-policy-client", Some(Uuid::new_v4())).await;
    let operator = postgres_network_operator(&db.repo).await;
    for trigger in [ScheduleTriggerKind::Cron, ScheduleTriggerKind::Event] {
        for tolerance in [-1, 0, 2, 100] {
            let mut request =
                postgres_shell_schedule_request("failure-policy", "failure-policy-client");
            request.max_failures = tolerance;
            if trigger == ScheduleTriggerKind::Event {
                request.trigger_kind = trigger;
                request.operation = None;
                request.cron_expr = None;
                request.timezone = None;
                request.catch_up_policy = None;
                request.catch_up_limit = None;
                request.retry_delay_secs = None;
                request.event_expression = Some("alert.triggered".to_string());
            }
            let schedule = db.repo.create_schedule(request, &operator).await.unwrap();
            assert_eq!(schedule.max_failures, tolerance);
            // Reach both sides of the upper bound without manufacturing 101 jobs.
            let initial_count = if tolerance == 100 { 99 } else { 0 };
            sqlx::query("UPDATE schedules SET failure_count=$2 WHERE id=$1")
                .bind(schedule.id)
                .bind(initial_count)
                .execute(&db.pool)
                .await
                .unwrap();
            let attempts = match tolerance {
                -1 => 4,
                100 => 2,
                _ => tolerance + 1,
            };
            for attempt in 1..=attempts {
                let before = db.repo.schedule_by_id(schedule.id).await.unwrap();
                let job_id = finish_scheduled_job(&db, schedule.id, JOB_STATUS_FAILED).await;
                let next_run = db
                    .repo
                    .schedule_by_id(schedule.id)
                    .await
                    .unwrap()
                    .next_run_at;
                if trigger == ScheduleTriggerKind::Cron
                    && (tolerance == -1 || initial_count + attempt <= tolerance)
                {
                    let retry_scheduled: bool = sqlx::query_scalar("SELECT next_run_at=last_job_completed_at+(retry_delay_secs*interval '1 second') FROM schedules WHERE id=$1")
                        .bind(schedule.id).fetch_one(&db.pool).await.unwrap();
                    assert!(retry_scheduled);
                }
                // Duplicate terminal processing must not spend the tolerance twice.
                db.repo
                    .record_job_terminal_side_effects(job_id, JOB_STATUS_FAILED)
                    .await
                    .unwrap();
                let after = db.repo.schedule_by_id(schedule.id).await.unwrap();
                assert_eq!(
                    after.next_run_at, next_run,
                    "duplicate outcome must not move the retry"
                );
                let count = initial_count + attempt;
                assert_eq!(after.failure_count, count);
                assert_eq!(after.enabled, tolerance == -1 || count <= tolerance);
                assert_eq!(after.last_error.as_deref(), Some("failed"));
                if !after.enabled {
                    assert_eq!(after.next_run_at, before.next_run_at);
                }
            }
            let before = db.repo.schedule_by_id(schedule.id).await.unwrap();
            finish_scheduled_job(&db, schedule.id, JOB_STATUS_CANCELED).await;
            assert_eq!(
                db.repo
                    .schedule_by_id(schedule.id)
                    .await
                    .unwrap()
                    .failure_count,
                before.failure_count
            );
            finish_scheduled_job(&db, schedule.id, JOB_STATUS_COMPLETED).await;
            let reset = db.repo.schedule_by_id(schedule.id).await.unwrap();
            assert_eq!(reset.failure_count, 0);
            assert!(reset.last_error.is_none());
            assert_eq!(
                reset.enabled, before.enabled,
                "success must not re-enable a paused schedule"
            );
            if tolerance == -1 {
                sqlx::query("UPDATE schedules SET failure_count=$2 WHERE id=$1")
                    .bind(schedule.id)
                    .bind(i32::MAX)
                    .execute(&db.pool)
                    .await
                    .unwrap();
                finish_scheduled_job(&db, schedule.id, JOB_STATUS_FAILED).await;
                let saturated = db.repo.schedule_by_id(schedule.id).await.unwrap();
                assert!(saturated.enabled);
                assert_eq!(saturated.failure_count, i32::MAX);
                sqlx::query("UPDATE schedules SET enabled=FALSE WHERE id=$1")
                    .bind(schedule.id)
                    .execute(&db.pool)
                    .await
                    .unwrap();
                finish_scheduled_job(&db, schedule.id, JOB_STATUS_FAILED).await;
                assert!(!db.repo.schedule_by_id(schedule.id).await.unwrap().enabled);
            }
        }
    }
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_schedule_failure_tolerance_backup_default_and_updates_share_the_model() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "backup-failures", None).await;
    let operator = postgres_network_operator(&db.repo).await;
    let request = || {
        serde_json::from_value::<CreateBackupPolicyRequest>(json!({
            "name": "backup-failures", "target_client_ids": ["backup-failures"],
            "paths": ["/etc/hostname"], "cron_expr": "0 3 * * *", "confirmed": true
        }))
        .unwrap()
    };
    assert_eq!(request().max_failures, -1);
    let policy = db
        .repo
        .create_backup_policy(request(), &operator)
        .await
        .unwrap();
    assert_eq!(policy.max_failures, -1);
    for tolerance in [0, 100, -1] {
        let saved = db.repo.schedule_by_id(policy.schedule_id).await.unwrap();
        let mut edited = request();
        edited.max_failures = tolerance;
        edited.retention_days = Some(policy.retention_days);
        edited.keep_last = Some(policy.keep_last);
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
        assert_eq!(updated.max_failures, tolerance);
        assert_eq!(
            db.repo
                .schedule_by_id(policy.schedule_id)
                .await
                .unwrap()
                .max_failures,
            tolerance
        );
    }
    db.cleanup().await;
}
