use super::*;
use crate::test_support::PgWorkerTestDb;
use sqlx::Row;

#[test]
fn retention_config_clamps_operational_bounds() {
    assert_eq!(
        AlertPolicyRetentionConfig::new(0, 0),
        AlertPolicyRetentionConfig {
            lifecycle_retention_days: 1,
            prune_limit: 1,
        }
    );
    assert_eq!(
        AlertPolicyRetentionConfig::new(10_000, 20_000),
        AlertPolicyRetentionConfig {
            lifecycle_retention_days: 3_650,
            prune_limit: 10_000,
        }
    );
}

#[tokio::test]
async fn postgres_retention_fails_closed_without_required_indexes() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let missing = missing_retention_indexes(&db.pool).await.unwrap();
    if missing.is_empty() {
        db.cleanup().await;
        return;
    }
    let run = process_alert_policy_retention(&db.pool, AlertPolicyRetentionConfig::new(30, 100))
        .await
        .unwrap();
    assert!(run.skipped_missing_indexes);
    assert_eq!(run.evidence_pruned, 0);
    assert_eq!(run.lifecycle_events_pruned, 0);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_evidence_retention_keeps_latest_baselines_and_references() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    if !missing_retention_indexes(&db.pool)
        .await
        .unwrap()
        .is_empty()
    {
        db.cleanup().await;
        return;
    }
    sqlx::query(
        r#"
        UPDATE alert_policy_lifecycle_meta
        SET evidence_retention_days = 1,
            evidence_pruned_through_seq = COALESCE(
                (SELECT max(evidence_seq) FROM alert_policy_evidence), 0
            )
        WHERE singleton
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let old_prunable = insert_metric_evidence(&db.pool, "prunable", 10, 10).await;
    let newest_baseline = insert_metric_evidence(&db.pool, "prunable", 9, 9).await;
    let referenced = insert_metric_evidence(&db.pool, "referenced", 10, 10).await;
    let referenced_newest = insert_metric_evidence(&db.pool, "referenced", 9, 9).await;
    let recently_evaluated = insert_metric_evidence(&db.pool, "recent-receipt", 10, 10).await;
    let recent_receipt_newest = insert_metric_evidence(&db.pool, "recent-receipt", 9, 9).await;
    for evidence in [
        old_prunable,
        newest_baseline,
        referenced,
        referenced_newest,
        recently_evaluated,
        recent_receipt_newest,
    ] {
        insert_terminal_receipts_for_metric_rules(&db.pool, evidence.0, evidence.1).await;
    }
    sqlx::query(
        "UPDATE alert_policy_evidence_receipts SET evaluated_at=now() WHERE evidence_id=$1",
    )
    .bind(recently_evaluated.0)
    .execute(&db.pool)
    .await
    .unwrap();
    let confirmation_rule = sqlx::query(
        r#"
        SELECT rule.id, rule.rule_version
        FROM policy_rules rule
        WHERE rule.evidence_source = 'telemetry.combined'
        ORDER BY rule.id
        LIMIT 1
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO alert_policy_confirmations (
            policy_rule_id, rule_version, confirmation_bucket_key,
            phase, evidence_id, accepted_at
        ) VALUES ($1, $2, 'referenced', 'trigger', $3, now() - interval '10 days')
        "#,
    )
    .bind(confirmation_rule.try_get::<Uuid, _>("id").unwrap())
    .bind(confirmation_rule.try_get::<i32, _>("rule_version").unwrap())
    .bind(referenced.0)
    .execute(&db.pool)
    .await
    .unwrap();

    let run = process_alert_policy_retention(&db.pool, AlertPolicyRetentionConfig::new(1, 1_000))
        .await
        .unwrap();
    assert!(!run.skipped_missing_indexes);
    assert_eq!(run.evidence_pruned, 1);
    assert!(run.evidence_receipts_pruned > 0);
    assert_eq!(run.evidence_pruned_through_seq, referenced_newest.1);
    let remaining = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM alert_policy_evidence
        WHERE id = ANY($1::uuid[])
        ORDER BY id
        "#,
    )
    .bind([
        old_prunable.0,
        newest_baseline.0,
        referenced.0,
        referenced_newest.0,
    ])
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert!(!remaining.contains(&old_prunable.0));
    assert!(remaining.contains(&newest_baseline.0));
    assert!(remaining.contains(&referenced.0));
    assert!(remaining.contains(&referenced_newest.0));
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM alert_policy_evidence WHERE id=$1)",
    )
    .bind(recently_evaluated.0)
    .fetch_one(&db.pool)
    .await
    .unwrap());

    sqlx::query(
        r#"
        UPDATE alert_policy_evidence_receipts
        SET evaluated_at=now() - interval '9 days'
        WHERE evidence_id=$1
        "#,
    )
    .bind(recently_evaluated.0)
    .execute(&db.pool)
    .await
    .unwrap();
    let second =
        process_alert_policy_retention(&db.pool, AlertPolicyRetentionConfig::new(1, 1_000))
            .await
            .unwrap();
    assert_eq!(second.evidence_pruned, 1);
    assert_eq!(second.evidence_pruned_through_seq, recent_receipt_newest.1);
    assert!(!sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM alert_policy_evidence WHERE id=$1)",
    )
    .bind(recently_evaluated.0)
    .fetch_one(&db.pool)
    .await
    .unwrap());
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM alert_policy_evidence WHERE id=$1)",
    )
    .bind(recent_receipt_newest.0)
    .fetch_one(&db.pool)
    .await
    .unwrap());

    db.cleanup().await;
}

async fn insert_metric_evidence(
    pool: &PgPool,
    natural_key: &str,
    observed_days_ago: i64,
    created_days_ago: i64,
) -> (Uuid, i64) {
    let id = Uuid::new_v4();
    let source_event_id = format!("retention-test:{id}");
    let row = sqlx::query(
        r#"
        INSERT INTO alert_policy_evidence (
            id, source_kind, source_event_id, fact_kind, natural_key,
            confirmation_bucket_key, subject_client_id, target_kind, target_id,
            source_status, completeness, subject_snapshot, payload, observed_at,
            state_started_at, created_at
        ) VALUES (
            $1, 'telemetry.combined', $2, 'metric', $3, $3, 'retention-client',
            'agent', 'retention-client', 'sampled', 'complete',
            '{"id":"retention-client"}'::jsonb,
            '{"cpu":{"utilization_percent":50}}'::jsonb,
            now() - ($4::bigint * interval '1 day'),
            now() - ($4::bigint * interval '1 day'),
            now() - ($5::bigint * interval '1 day')
        )
        RETURNING evidence_seq
        "#,
    )
    .bind(id)
    .bind(source_event_id)
    .bind(natural_key)
    .bind(observed_days_ago)
    .bind(created_days_ago)
    .fetch_one(pool)
    .await
    .unwrap();
    (id, row.try_get("evidence_seq").unwrap())
}

async fn insert_terminal_receipts_for_metric_rules(
    pool: &PgPool,
    evidence_id: Uuid,
    evidence_seq: i64,
) {
    sqlx::query(
        r#"
        INSERT INTO alert_policy_evidence_receipts (
            policy_rule_id, rule_version, evidence_seq, evidence_id,
            natural_key, confirmation_bucket_key, result, evaluated_at
        )
        SELECT rule.id, rule.rule_version, $2, $1,
               evidence.natural_key, evidence.confirmation_bucket_key,
               'not_matched', now() - interval '9 days'
        FROM policy_rules rule
        JOIN alert_policy_evidence evidence ON evidence.id = $1
        WHERE rule.evidence_source = evidence.source_kind
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(evidence_id)
    .bind(evidence_seq)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn postgres_lifecycle_retention_waits_for_every_consumer_and_terminal_owner() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    if !missing_retention_indexes(&db.pool)
        .await
        .unwrap()
        .is_empty()
    {
        db.cleanup().await;
        return;
    }
    let terminal = insert_projected_lifecycle_event(&db.pool, true).await;
    let (schedule_receipt, schedule_job) =
        insert_terminal_schedule_receipt(&db.pool, terminal.0, terminal.1, terminal.2).await;
    set_lifecycle_cursors(&db.pool, terminal.0, terminal.0.saturating_sub(1)).await;

    let blocked_by_cursor =
        process_alert_policy_retention(&db.pool, AlertPolicyRetentionConfig::new(1, 100))
            .await
            .unwrap();
    assert_eq!(blocked_by_cursor.lifecycle_events_pruned, 0);
    assert!(lifecycle_event_exists(&db.pool, terminal.0).await);

    set_lifecycle_cursors(&db.pool, terminal.0, terminal.0).await;
    let terminal_run =
        process_alert_policy_retention(&db.pool, AlertPolicyRetentionConfig::new(1, 100))
            .await
            .unwrap();
    assert_eq!(terminal_run.lifecycle_events_pruned, 1);
    assert_eq!(terminal_run.webhook_receipts_pruned, 1);
    assert_eq!(terminal_run.schedule_receipts_pruned, 1);
    assert_eq!(terminal_run.schedule_dependencies_pruned, 1);
    assert!(!lifecycle_event_exists(&db.pool, terminal.0).await);
    assert!(!sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM schedule_event_receipts WHERE id=$1)",
    )
    .bind(schedule_receipt)
    .fetch_one(&db.pool)
    .await
    .unwrap());
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM jobs WHERE id=$1)",)
            .bind(schedule_job)
            .fetch_one(&db.pool)
            .await
            .unwrap()
    );

    let blocked = insert_projected_lifecycle_event(&db.pool, false).await;
    let later_terminal = insert_projected_lifecycle_event(&db.pool, true).await;
    set_lifecycle_cursors(&db.pool, later_terminal.0, later_terminal.0).await;
    let unprocessed_run =
        process_alert_policy_retention(&db.pool, AlertPolicyRetentionConfig::new(1, 1))
            .await
            .unwrap();
    assert_eq!(unprocessed_run.lifecycle_events_pruned, 0);
    assert!(lifecycle_event_exists(&db.pool, blocked.0).await);
    assert!(lifecycle_event_exists(&db.pool, later_terminal.0).await);

    sqlx::query("UPDATE webhook_events SET processed_at=now() WHERE alert_lifecycle_event_seq=$1")
        .bind(blocked.0)
        .execute(&db.pool)
        .await
        .unwrap();
    let delivery_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO webhook_rule_deliveries (
            id, rule_id, rule_name, event_kind, event_id, status, target,
            dedupe_key, payload, matched_vps, message, cooldown_until_unix,
            created_at
        ) VALUES (
            $1, $2, 'retention-test', 'alert.triggered', $3, 'queued',
            'https://hooks.example.invalid/retention', $4, '{}'::jsonb,
            '[]'::jsonb, 'retention', 0, now() - interval '2 days'
        )
        "#,
    )
    .bind(delivery_id)
    .bind(Uuid::new_v4())
    .bind(&blocked.3)
    .bind(format!("retention:{delivery_id}"))
    .execute(&db.pool)
    .await
    .unwrap();
    let queued_run =
        process_alert_policy_retention(&db.pool, AlertPolicyRetentionConfig::new(1, 1))
            .await
            .unwrap();
    assert_eq!(queued_run.lifecycle_events_pruned, 1);
    assert!(lifecycle_event_exists(&db.pool, blocked.0).await);
    assert!(!lifecycle_event_exists(&db.pool, later_terminal.0).await);

    sqlx::query("UPDATE webhook_rule_deliveries SET status='permanently_failed' WHERE id=$1")
        .bind(delivery_id)
        .execute(&db.pool)
        .await
        .unwrap();
    let wrap_run = process_alert_policy_retention(&db.pool, AlertPolicyRetentionConfig::new(1, 1))
        .await
        .unwrap();
    assert_eq!(wrap_run.lifecycle_events_pruned, 0);
    let final_run = process_alert_policy_retention(&db.pool, AlertPolicyRetentionConfig::new(1, 1))
        .await
        .unwrap();
    assert_eq!(final_run.lifecycle_events_pruned, 1);
    assert!(!lifecycle_event_exists(&db.pool, blocked.0).await);

    db.cleanup().await;
}

async fn insert_projected_lifecycle_event(
    pool: &PgPool,
    processed: bool,
) -> (i64, Uuid, Uuid, String) {
    let episode_id = Uuid::new_v4();
    let rule = sqlx::query(
        r#"
        SELECT rule.id AS rule_id, rule.rule_version, rule.rule_kind,
               rule.evidence_source, rule.name AS rule_name,
               rule.system_seed_key, group_row.id AS group_id,
               group_row.name AS group_name
        FROM policy_rules rule
        JOIN policy_groups group_row ON group_row.id = rule.group_id
        WHERE rule.rule_kind = 'occurrence'
        ORDER BY rule.id
        LIMIT 1
        "#,
    )
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO alert_episodes (
            id, public_id, producer_kind, natural_key, record_kind,
            trigger_generation, trigger_severity, trigger_category, severity,
            category, target_kind, target_id, title, detail, source_status,
            evidence, lifecycle_state, triggered_at, last_confirmed_at,
            resolved_at, resolution_reason, policy_group_id, policy_rule_id,
            policy_rule_version, policy_rule_kind, policy_group_name,
            policy_rule_name, policy_rule_system_seed_key, created_at, updated_at
        ) VALUES (
            $1, $2, $4, $3, 'event', 1,
            'warning', 'job', 'warning', 'job', 'system', $3,
            'Retention test', 'Retention test episode', 'failed', '{}'::jsonb,
            'resolved', now() - interval '10 days', now() - interval '10 days',
            now() - interval '9 days', 'policy_time_elapsed', $5, $6, $7,
            $8, $9, $10, $11,
            now() - interval '10 days', now() - interval '9 days'
        )
        "#,
    )
    .bind(episode_id)
    .bind(format!("retention-test:{episode_id}"))
    .bind(format!("retention-natural:{episode_id}"))
    .bind(rule.try_get::<String, _>("evidence_source").unwrap())
    .bind(rule.try_get::<Uuid, _>("group_id").unwrap())
    .bind(rule.try_get::<Uuid, _>("rule_id").unwrap())
    .bind(rule.try_get::<i32, _>("rule_version").unwrap())
    .bind(rule.try_get::<String, _>("rule_kind").unwrap())
    .bind(rule.try_get::<String, _>("group_name").unwrap())
    .bind(rule.try_get::<String, _>("rule_name").unwrap())
    .bind(
        rule.try_get::<Option<String>, _>("system_seed_key")
            .unwrap(),
    )
    .execute(pool)
    .await
    .unwrap();
    let lifecycle_id = Uuid::new_v4();
    let event_id = format!("retention-event:{lifecycle_id}");
    let row = sqlx::query(
        r#"
        INSERT INTO alert_lifecycle_events (
            id, episode_id, trigger_generation, edge_kind, event_id,
            event_predicates, payload, occurred_at, created_at
        ) VALUES (
            $1, $2, 1, 'alert.triggered', $3,
            ARRAY['alert.triggered','alert.category:job','alert.severity:warning'],
            '{}'::jsonb, now() - interval '10 days', now() - interval '10 days'
        )
        RETURNING event_seq
        "#,
    )
    .bind(lifecycle_id)
    .bind(episode_id)
    .bind(&event_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let event_seq: i64 = row.try_get("event_seq").unwrap();
    let webhook_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO webhook_events (
            id, kind, event_id, event_predicates, payload, occurred_at,
            processed_at, alert_lifecycle_event_seq
        ) VALUES (
            $1, 'alert.triggered', $2, ARRAY['alert.triggered'], '{}'::jsonb,
            now() - interval '10 days',
            CASE WHEN $3 THEN now() - interval '9 days' ELSE NULL END, $4
        )
        "#,
    )
    .bind(webhook_id)
    .bind(&event_id)
    .bind(processed)
    .bind(event_seq)
    .execute(pool)
    .await
    .unwrap();
    let webhook_occurred_at: DateTime<Utc> = sqlx::query_scalar(
        "SELECT occurred_at FROM webhook_events WHERE alert_lifecycle_event_seq=$1",
    )
    .bind(event_seq)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO alert_lifecycle_webhook_receipts (
            event_seq, webhook_event_id, webhook_event_occurred_at, status
        ) VALUES ($1, $2, $3, 'projected')
        "#,
    )
    .bind(event_seq)
    .bind(webhook_id)
    .bind(webhook_occurred_at)
    .execute(pool)
    .await
    .unwrap();
    (event_seq, lifecycle_id, episode_id, event_id)
}

async fn insert_terminal_schedule_receipt(
    pool: &PgPool,
    event_seq: i64,
    _lifecycle_id: Uuid,
    episode_id: Uuid,
) -> (Uuid, Uuid) {
    let schedule_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO schedules (
            id, name, enabled, operation, selector_expression,
            target_client_ids, cron_expr, timezone, next_run_at,
            catch_up_policy, catch_up_limit, retry_delay_secs,
            trigger_kind, event_expression, definition_revision,
            event_armed_at, armed_after_event_seq
        ) VALUES (
            $1, $2, TRUE, NULL, '*', ARRAY[]::text[], NULL, NULL, NULL,
            NULL, NULL, NULL, 'event', 'alert.triggered', 1,
            now() - interval '10 days', 0
        )
        "#,
    )
    .bind(schedule_id)
    .bind(format!("retention-schedule-{schedule_id}"))
    .execute(pool)
    .await
    .unwrap();
    let job_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, command_type, privileged, status, target_count, payload_hash,
            operation, source_schedule_id, request_fingerprint,
            max_timeout_secs, created_at, completed_at
        ) VALUES (
            $1, 'command', TRUE, 'completed', 0, $2,
            '{"type":"command","argv":["/bin/true"]}'::jsonb,
            $3, $4, 30, now() - interval '10 days', now() - interval '9 days'
        )
        "#,
    )
    .bind(job_id)
    .bind("0".repeat(64))
    .bind(schedule_id)
    .bind(format!("retention-job:{job_id}"))
    .execute(pool)
    .await
    .unwrap();
    let receipt_id = Uuid::new_v4();
    let lifecycle_event_id: String =
        sqlx::query_scalar("SELECT event_id FROM alert_lifecycle_events WHERE event_seq=$1")
            .bind(event_seq)
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query(
        r#"
        INSERT INTO schedule_event_receipts (
            id, schedule_id, definition_revision, schedule_name, event_seq,
            event_kind, event_id, episode_id, trigger_generation, edge_ordinal,
            status, source_occurred_at, source_payload_hash,
            matched_subject_client_ids, fixed_target_client_ids, causation_id,
            job_id, dispatched_at, created_at, updated_at
        ) VALUES (
            $1, $2, 1, $3, $4, 'alert.triggered', $5, $6, 1, 1,
            'dispatched', now() - interval '10 days', $7,
            ARRAY[]::text[], ARRAY[]::text[], $8, $9,
            now() - interval '9 days', now() - interval '10 days',
            now() - interval '9 days'
        )
        "#,
    )
    .bind(receipt_id)
    .bind(schedule_id)
    .bind(format!("retention-schedule-{schedule_id}"))
    .bind(event_seq)
    .bind(lifecycle_event_id)
    .bind(episode_id)
    .bind("1".repeat(64))
    .bind(Uuid::new_v4())
    .bind(job_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO schedule_event_dependencies (receipt_id, prerequisite_job_id)
        VALUES ($1, $2)
        "#,
    )
    .bind(receipt_id)
    .bind(job_id)
    .execute(pool)
    .await
    .unwrap();
    (receipt_id, job_id)
}

async fn set_lifecycle_cursors(pool: &PgPool, webhook: i64, schedule: i64) {
    sqlx::query(
        r#"
        UPDATE alert_lifecycle_consumer_cursors
        SET last_event_seq = CASE consumer_kind
            WHEN 'webhook' THEN $1
            WHEN 'schedule' THEN $2
        END
        "#,
    )
    .bind(webhook)
    .bind(schedule)
    .execute(pool)
    .await
    .unwrap();
}

async fn lifecycle_event_exists(pool: &PgPool, event_seq: i64) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM alert_lifecycle_events WHERE event_seq=$1)")
        .bind(event_seq)
        .fetch_one(pool)
        .await
        .unwrap()
}
