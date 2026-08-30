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

#[test]
fn retention_only_treats_foreground_lock_wait_as_skippable() {
    assert!(is_retention_lock_timeout_code(Some("55P03")));
    assert!(!is_retention_lock_timeout_code(Some("57014")));
    assert!(!is_retention_lock_timeout_code(None));

    let source = include_str!("alert_policy_retention.rs");
    assert!(source.contains("SET LOCAL lock_timeout = '2s'"));
    assert!(!source.contains("statement_timeout"));
    assert!(!source.contains("pg_advisory"));
    assert!(source.contains("FROM alert_policy_lifecycle_meta"));
    assert!(source.contains("FOR UPDATE OF lifecycle SKIP LOCKED"));
}

#[test]
fn stream_evidence_pruning_is_owned_by_fact_kind() {
    let schema = include_str!("../../../../migrations/0012_alert_lifecycle.sql");
    assert!(schema.contains("AND OLD.fact_kind IN ('metric', 'state')"));
    assert!(schema.contains("IF NEW.fact_kind IN ('metric', 'state')"));

    let retention = include_str!("alert_policy_retention.rs");
    assert_eq!(
        retention
            .matches("evidence.fact_kind IN ('metric', 'state')")
            .count(),
        2,
        "stream facts must bypass age in both eligibility stages"
    );
    assert!(!retention.contains("evidence.source_kind = 'telemetry.combined'"));
}

#[test]
fn resolved_retention_uses_an_indexable_database_cutoff() {
    assert!(
        REQUIRED_RETENTION_INDEXES.contains(&"alert_policy_evaluation_states_active_episode_idx")
    );

    let source = include_str!("alert_policy_retention.rs");
    assert!(source.contains("SELECT transaction_timestamp()"));
    assert!(source.contains("episode.resolved_at <= $2::timestamptz"));
    assert!(!source.contains("episode.resolved_at <= clock_timestamp()"));

    let schema = include_str!("../../../../migrations/0012_alert_lifecycle.sql");
    assert!(schema.contains(
        "CREATE INDEX alert_policy_evaluation_states_active_episode_idx ON public.\
alert_policy_evaluation_states USING btree (active_episode_id) WHERE (active_episode_id IS NOT NULL);"
    ));
    assert!(schema.contains(
        "CREATE INDEX alert_episodes_resolved_retention_idx ON public.alert_episodes USING btree \
(resolved_at DESC, id DESC) WHERE (lifecycle_state = 'resolved'::text);"
    ));
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
async fn postgres_evidence_retention_bounds_state_streams_and_keeps_references() {
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
    ensure_retention_client(&db.pool).await;
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
    let displaced_state = insert_state_evidence(&db.pool, "state-stream", 0, 0).await;
    let current_state = insert_state_evidence(&db.pool, "state-stream", 0, 0).await;
    let fresh_occurrence = insert_occurrence_evidence(&db.pool, "fresh-occurrence", 0).await;
    sqlx::query(
        r#"
        INSERT INTO alert_policy_evidence_prune_candidates (
            evidence_id, source_kind, subject_client_id, natural_key
        ) VALUES ($1, 'backup.failure', NULL, 'fresh-occurrence')
        "#,
    )
    .bind(fresh_occurrence.0)
    .execute(&db.pool)
    .await
    .unwrap();
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

    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM alert_policy_evidence WHERE id=$1)",
    )
    .bind(old_prunable.0)
    .fetch_one(&db.pool)
    .await
    .unwrap());
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM alert_policy_evidence_prune_candidates WHERE evidence_id=$1)",
    )
    .bind(old_prunable.0)
    .fetch_one(&db.pool)
    .await
    .unwrap(),
    "the source transaction only queues a displaced fact; retention owns deletion"
    );
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM alert_policy_evidence_prune_candidates WHERE evidence_id=$1)",
    )
    .bind(displaced_state.0)
    .fetch_one(&db.pool)
    .await
    .unwrap(),
    "a displaced state fact must enter the same immediate retention queue"
    );

    let run = process_alert_policy_retention(&db.pool, AlertPolicyRetentionConfig::new(1, 1_000))
        .await
        .unwrap();
    assert!(!run.skipped_missing_indexes);
    assert_eq!(run.evidence_pruned, 3);
    assert!(run.evidence_receipts_pruned > 0);
    assert_eq!(run.evidence_pruned_through_seq, recent_receipt_newest.1);
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
    assert!(!sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM alert_policy_evidence WHERE id=$1)",
    )
    .bind(displaced_state.0)
    .fetch_one(&db.pool)
    .await
    .unwrap());
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM alert_policy_evidence WHERE id=$1)",
    )
    .bind(current_state.0)
    .fetch_one(&db.pool)
    .await
    .unwrap());
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM alert_policy_evidence WHERE id=$1)",
        )
        .bind(fresh_occurrence.0)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        "occurrence history must retain the configured age window"
    );
    let candidates_after_first: Vec<Uuid> = sqlx::query_scalar(
        r#"
        SELECT evidence_id
        FROM alert_policy_evidence_prune_candidates
        WHERE evidence_id = ANY($1::uuid[])
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
    assert!(!candidates_after_first.contains(&old_prunable.0));
    assert!(candidates_after_first.contains(&newest_baseline.0));
    assert!(candidates_after_first.contains(&referenced.0));
    assert!(candidates_after_first.contains(&referenced_newest.0));
    // Superseded metric/state evidence is bounded stream state, not 30-day
    // occurrence history. A fresh terminal receipt cannot retain every sample.
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
    assert!(!sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM alert_policy_evidence_prune_candidates WHERE evidence_id=$1)",
    )
    .bind(recently_evaluated.0)
    .fetch_one(&db.pool)
    .await
    .unwrap());

    // A permanently retained latest row must not starve an older queued row
    // once its temporary reference disappears, even with a one-row retry scan.
    sqlx::query("DELETE FROM alert_policy_confirmations WHERE evidence_id=$1")
        .bind(referenced.0)
        .execute(&db.pool)
        .await
        .unwrap();
    process_alert_policy_retention(&db.pool, AlertPolicyRetentionConfig::new(1, 1))
        .await
        .unwrap();
    assert!(!sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM alert_policy_evidence WHERE id=$1)",
    )
    .bind(referenced.0)
    .fetch_one(&db.pool)
    .await
    .unwrap());

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_evidence_enqueue_round_excludes_post_boundary_appends() {
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
    ensure_retention_client(&db.pool).await;
    sqlx::query(
        r#"
        UPDATE alert_policy_lifecycle_meta
        SET evidence_retention_days=1,
            evidence_pruned_through_seq=COALESCE(
                (SELECT max(evidence_seq) FROM alert_policy_evidence),0
            )
        WHERE singleton
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let before_boundary = insert_metric_evidence(&db.pool, "round-before", 10, 10).await;
    let scan_through_seq: i64 =
        sqlx::query_scalar("SELECT max(evidence_seq) FROM alert_policy_evidence")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(scan_through_seq, before_boundary.1);
    let mut appended = Vec::new();
    for suffix in 0..8 {
        appended
            .push(insert_metric_evidence(&db.pool, &format!("round-after-{suffix}"), 10, 10).await);
    }

    let mut pages = 0_usize;
    let mut scanned = 0_usize;
    loop {
        pages += 1;
        let page = process_policy_evidence_page(&db.pool, 1, Some(scan_through_seq), None)
            .await
            .unwrap();
        scanned += page.run.evidence_scanned;
        if page.run.evidence_scanned == 0 {
            break;
        }
    }
    assert_eq!(pages, 2, "post-boundary producers extended this round");
    assert_eq!(scanned, 1);
    for (id, _) in appended {
        let queued = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM alert_policy_evidence_prune_candidates WHERE evidence_id=$1)",
        )
        .bind(id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert!(!queued, "post-boundary evidence entered the active round");
    }

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_evidence_retention_preserves_pending_owner_until_terminal() {
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
    ensure_retention_client(&db.pool).await;
    sqlx::query(
        r#"
        UPDATE alert_policy_lifecycle_meta
        SET evidence_retention_days=1,
            evidence_pruned_through_seq=COALESCE(
                (SELECT max(evidence_seq) FROM alert_policy_evidence),0
            )
        WHERE singleton
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let pending = insert_metric_evidence(&db.pool, "pending-owner", 10, 10).await;
    let current = insert_metric_evidence(&db.pool, "pending-owner", 9, 9).await;
    insert_terminal_receipts_for_metric_rules(&db.pool, current.0, current.1).await;
    sqlx::query("UPDATE alert_policy_evidence SET evaluation_pending=TRUE WHERE id=$1")
        .bind(pending.0)
        .execute(&db.pool)
        .await
        .unwrap();

    process_alert_policy_retention(&db.pool, AlertPolicyRetentionConfig::new(1, 1_000))
        .await
        .unwrap();
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM alert_policy_evidence WHERE id=$1 AND evaluation_pending)",
    )
    .bind(pending.0)
    .fetch_one(&db.pool)
    .await
    .unwrap());

    insert_terminal_receipts_for_metric_rules(&db.pool, pending.0, pending.1).await;
    sqlx::query("UPDATE alert_policy_evidence SET evaluation_pending=FALSE WHERE id=$1")
        .bind(pending.0)
        .execute(&db.pool)
        .await
        .unwrap();
    for _ in 0..8 {
        process_alert_policy_retention(&db.pool, AlertPolicyRetentionConfig::new(1, 1))
            .await
            .unwrap();
        if !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM alert_policy_evidence WHERE id=$1)",
        )
        .bind(pending.0)
        .fetch_one(&db.pool)
        .await
        .unwrap()
        {
            break;
        }
    }
    assert!(!sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM alert_policy_evidence WHERE id=$1)",
    )
    .bind(pending.0)
    .fetch_one(&db.pool)
    .await
    .unwrap());

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_evidence_retention_does_not_fence_an_unrelated_writer() {
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
    ensure_retention_client(&db.pool).await;
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

    let expired = insert_metric_evidence(&db.pool, "writer-fence-expired", 10, 10).await;
    let current = insert_metric_evidence(&db.pool, "writer-fence-expired", 9, 9).await;
    for evidence in [expired, current] {
        insert_terminal_receipts_for_metric_rules(&db.pool, evidence.0, evidence.1).await;
    }

    // Keep a fresh, unrelated fact uncommitted while retention removes the old
    // eligible row. Evidence producers and retention own disjoint natural work
    // identities, so neither path needs a repository-global coordination arm.
    let mut writer = db.pool.begin().await.unwrap();
    let writer_evidence_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO alert_policy_evidence (
            id, source_kind, source_event_id, fact_kind, natural_key,
            confirmation_bucket_key, subject_client_id, target_kind, target_id,
            source_status, completeness, subject_snapshot, payload, observed_at,
            state_started_at, evaluation_pending
        ) VALUES (
            $1, 'telemetry.combined', $2, 'metric', 'writer-fence-current',
            'writer-fence-current', 'retention-client', 'agent',
            'retention-client', 'sampled', 'complete',
            jsonb_build_object(
                'id', 'retention-client',
                'scope_revision', (
                    SELECT policy_scope_revision FROM clients
                    WHERE id='retention-client'
                )
            ),
            '{"cpu":{"utilization_percent":25}}'::jsonb,
            clock_timestamp(), clock_timestamp(), FALSE
        )
        "#,
    )
    .bind(writer_evidence_id)
    .bind(format!("retention-writer-test:{writer_evidence_id}"))
    .execute(&mut *writer)
    .await
    .unwrap();

    let run = prune_policy_evidence(&db.pool, 1_000).await.unwrap();
    assert_eq!(run.evidence_pruned, 1);
    assert!(!sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM alert_policy_evidence WHERE id=$1)",
    )
    .bind(expired.0)
    .fetch_one(&db.pool)
    .await
    .unwrap());
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM alert_policy_evidence WHERE id=$1)",
    )
    .bind(current.0)
    .fetch_one(&db.pool)
    .await
    .unwrap());

    writer.commit().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT evidence_id
            FROM alert_policy_current_evidence
            WHERE subject_client_id='retention-client'
              AND source_kind='telemetry.combined'
              AND natural_key='writer-fence-current'
            "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        writer_evidence_id
    );

    db.cleanup().await;
}

async fn ensure_retention_client(pool: &PgPool) {
    sqlx::query(
        r#"
        INSERT INTO clients (id, display_name, public_key, status)
        VALUES (
            'retention-client', 'retention-client',
            decode(repeat('ab', 32), 'hex'), 'online'
        )
        ON CONFLICT (id) DO NOTHING
        "#,
    )
    .execute(pool)
    .await
    .unwrap();
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
            state_started_at, created_at, evaluation_pending
        ) VALUES (
            $1, 'telemetry.combined', $2, 'metric', $3, $3, 'retention-client',
            'agent', 'retention-client', 'sampled', 'complete',
            '{"id":"retention-client"}'::jsonb,
            '{"cpu":{"utilization_percent":50}}'::jsonb,
            now() - ($4::bigint * interval '1 day'),
            now() - ($4::bigint * interval '1 day'),
            now() - ($5::bigint * interval '1 day'), FALSE
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

async fn insert_state_evidence(
    pool: &PgPool,
    natural_key: &str,
    observed_days_ago: i64,
    created_days_ago: i64,
) -> (Uuid, i64) {
    let id = Uuid::new_v4();
    let source_event_id = format!("retention-state-test:{id}");
    let row = sqlx::query(
        r#"
        INSERT INTO alert_policy_evidence (
            id, source_kind, source_event_id, fact_kind, natural_key,
            confirmation_bucket_key, subject_client_id, target_kind, target_id,
            source_status, completeness, subject_snapshot, payload, observed_at,
            state_started_at, created_at, evaluation_pending
        ) VALUES (
            $1, 'agent.access', $2, 'state', $3, $3, 'retention-client',
            'agent', 'retention-client', 'available', 'complete',
            '{"id":"retention-client"}'::jsonb,
            '{"state":"available"}'::jsonb,
            now() - ($4::bigint * interval '1 day'),
            now() - ($4::bigint * interval '1 day'),
            now() - ($5::bigint * interval '1 day'), FALSE
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

async fn insert_occurrence_evidence(
    pool: &PgPool,
    natural_key: &str,
    created_days_ago: i64,
) -> (Uuid, i64) {
    let id = Uuid::new_v4();
    let source_event_id = format!("retention-occurrence-test:{id}");
    let row = sqlx::query(
        r#"
        INSERT INTO alert_policy_evidence (
            id, source_kind, source_event_id, fact_kind, natural_key,
            confirmation_bucket_key, subject_client_id, target_kind, target_id,
            source_status, completeness, subject_snapshot, payload, observed_at,
            state_started_at, created_at, evaluation_pending
        ) VALUES (
            $1, 'backup.failure', $2, 'occurrence', $3, $3, NULL,
            'backup', $3, 'failed', 'complete', '{}'::jsonb,
            '{"status":"failed"}'::jsonb,
            now() - ($4::bigint * interval '1 day'), NULL,
            now() - ($4::bigint * interval '1 day'), FALSE
        )
        RETURNING evidence_seq
        "#,
    )
    .bind(id)
    .bind(source_event_id)
    .bind(natural_key)
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
    set_lifecycle_receipt_frontiers(&db.pool, terminal.0, terminal.0.saturating_sub(1)).await;

    let blocked_by_cursor =
        process_alert_policy_retention(&db.pool, AlertPolicyRetentionConfig::new(1, 100))
            .await
            .unwrap();
    assert_eq!(blocked_by_cursor.lifecycle_events_pruned, 0);
    assert!(lifecycle_event_exists(&db.pool, terminal.0).await);

    set_lifecycle_receipt_frontiers(&db.pool, terminal.0, terminal.0).await;
    let terminal_run =
        process_alert_policy_retention(&db.pool, AlertPolicyRetentionConfig::new(1, 100))
            .await
            .unwrap();
    assert_eq!(terminal_run.lifecycle_events_pruned, 1);
    assert_eq!(terminal_run.consumer_receipts_pruned, 2);
    assert_eq!(terminal_run.schedule_receipts_pruned, 1);
    assert_eq!(terminal_run.schedule_dependencies_pruned, 1);
    assert_eq!(terminal_run.resolved_episodes_pruned, 1);
    assert!(!lifecycle_event_exists(&db.pool, terminal.0).await);
    assert!(!episode_exists(&db.pool, terminal.2).await);
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
    set_lifecycle_receipt_frontiers(&db.pool, later_terminal.0, later_terminal.0).await;
    // With a one-row transaction page, the older unsafe event must not occupy
    // the page ahead of the later independent event that is already terminal.
    let unprocessed_run =
        process_alert_policy_retention(&db.pool, AlertPolicyRetentionConfig::new(1, 1))
            .await
            .unwrap();
    assert_eq!(unprocessed_run.lifecycle_events_pruned, 1);
    assert!(lifecycle_event_exists(&db.pool, blocked.0).await);
    assert!(!lifecycle_event_exists(&db.pool, later_terminal.0).await);
    assert!(episode_exists(&db.pool, blocked.2).await);
    assert!(!episode_exists(&db.pool, later_terminal.2).await);

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
    assert_eq!(queued_run.lifecycle_events_pruned, 0);
    assert!(lifecycle_event_exists(&db.pool, blocked.0).await);
    assert!(episode_exists(&db.pool, blocked.2).await);

    sqlx::query("UPDATE webhook_rule_deliveries SET status='permanently_failed' WHERE id=$1")
        .bind(delivery_id)
        .execute(&db.pool)
        .await
        .unwrap();
    let terminal_run =
        process_alert_policy_retention(&db.pool, AlertPolicyRetentionConfig::new(1, 1))
            .await
            .unwrap();
    assert_eq!(terminal_run.lifecycle_events_pruned, 1);
    assert!(!lifecycle_event_exists(&db.pool, blocked.0).await);
    assert!(!episode_exists(&db.pool, blocked.2).await);

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_lifecycle_retention_skips_an_independently_locked_event() {
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
    let first = insert_projected_lifecycle_event(&db.pool, true).await;
    let second = insert_projected_lifecycle_event(&db.pool, true).await;
    set_lifecycle_receipt_frontiers(&db.pool, second.0, second.0).await;

    let mut holder = db.pool.begin().await.unwrap();
    sqlx::query("SELECT event_seq FROM alert_lifecycle_events WHERE event_seq=$1 FOR UPDATE")
        .bind(first.0)
        .execute(&mut *holder)
        .await
        .unwrap();

    let run = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        prune_lifecycle_events(&db.pool, AlertPolicyRetentionConfig::new(1, 1)),
    )
    .await
    .expect("exact lifecycle ownership must not wait for an unrelated event")
    .unwrap();
    assert_eq!(run.lifecycle_events_pruned, 1);
    assert!(lifecycle_event_exists(&db.pool, first.0).await);
    assert!(!lifecycle_event_exists(&db.pool, second.0).await);

    holder.rollback().await.unwrap();
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_resolved_episode_retention_bounds_orphans_and_releases_evidence() {
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
    ensure_retention_client(&db.pool).await;
    let evidence_high_water: i64 =
        sqlx::query_scalar("SELECT COALESCE(max(evidence_seq), 0) FROM alert_policy_evidence")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    sqlx::query(
        r#"
        UPDATE alert_policy_lifecycle_meta
        SET evidence_retention_days=30,
            evidence_pruned_through_seq=$1
        WHERE singleton
        "#,
    )
    .bind(evidence_high_water)
    .execute(&db.pool)
    .await
    .unwrap();

    let held = insert_metric_evidence(&db.pool, "episode-held", 10, 10).await;
    let current = insert_metric_evidence(&db.pool, "episode-held", 9, 9).await;
    for evidence in [held, current] {
        insert_terminal_receipts_for_metric_rules(&db.pool, evidence.0, evidence.1).await;
    }
    sqlx::query("DELETE FROM alert_policy_evidence_prune_candidates WHERE evidence_id=$1")
        .bind(held.0)
        .execute(&db.pool)
        .await
        .unwrap();

    let eligible = insert_resolved_retention_episode(&db.pool).await;
    sqlx::query("UPDATE alert_episodes SET trigger_evidence_id=$2,last_evidence_id=$2 WHERE id=$1")
        .bind(eligible)
        .bind(held.0)
        .execute(&db.pool)
        .await
        .unwrap();
    let eligible_public_id = episode_public_id(&db.pool, eligible).await;
    sqlx::query(
        r#"
        INSERT INTO fleet_alert_states (alert_id, state, revision)
        VALUES ($1, 'acknowledged', 1)
        "#,
    )
    .bind(&eligible_public_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let active = insert_resolved_retention_episode(&db.pool).await;
    sqlx::query(
        r#"
        UPDATE alert_episodes
        SET lifecycle_state='triggered', resolved_at=NULL,
            resolution_reason=NULL, updated_at=clock_timestamp()
        WHERE id=$1
        "#,
    )
    .bind(active)
    .execute(&db.pool)
    .await
    .unwrap();

    let fresh = insert_resolved_retention_episode(&db.pool).await;
    sqlx::query(
        "UPDATE alert_episodes SET resolved_at=clock_timestamp(),updated_at=clock_timestamp() WHERE id=$1",
    )
    .bind(fresh)
    .execute(&db.pool)
    .await
    .unwrap();

    let delivery_blocked = insert_resolved_retention_episode(&db.pool).await;
    let blocked_public_id = episode_public_id(&db.pool, delivery_blocked).await;
    let delivery_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO fleet_alert_notification_deliveries (
            id, channel_id, channel_name, alert_id, alert_severity,
            alert_category, status, delivery_kind, target, dedupe_key,
            payload, cooldown_until_unix
        ) VALUES (
            $1, $2, 'retention', $3, 'warning', 'job', 'queued',
            'webhook', 'https://hooks.example.invalid/retention', $4,
            '{}'::jsonb, 0
        )
        "#,
    )
    .bind(delivery_id)
    .bind(Uuid::new_v4())
    .bind(&blocked_public_id)
    .bind(format!("episode-retention:{delivery_id}"))
    .execute(&db.pool)
    .await
    .unwrap();

    let first = process_alert_policy_retention(&db.pool, AlertPolicyRetentionConfig::new(1, 1_000))
        .await
        .unwrap();
    assert_eq!(first.resolved_episodes_pruned, 1);
    assert_eq!(first.episode_evidence_enqueued, 1);
    assert!(!episode_exists(&db.pool, eligible).await);
    assert!(episode_exists(&db.pool, active).await);
    assert!(episode_exists(&db.pool, fresh).await);
    assert!(episode_exists(&db.pool, delivery_blocked).await);
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM fleet_alert_states WHERE alert_id=$1)",
    )
    .bind(&eligible_public_id)
    .fetch_one(&db.pool)
    .await
    .unwrap());
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM alert_policy_evidence WHERE id=$1)",
    )
    .bind(held.0)
    .fetch_one(&db.pool)
    .await
    .unwrap());

    sqlx::query(
        "UPDATE fleet_alert_notification_deliveries SET status='delivered',delivered_at=now() WHERE id=$1",
    )
    .bind(delivery_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let second =
        process_alert_policy_retention(&db.pool, AlertPolicyRetentionConfig::new(1, 1_000))
            .await
            .unwrap();
    assert_eq!(second.resolved_episodes_pruned, 1);
    assert!(!episode_exists(&db.pool, delivery_blocked).await);
    assert!(!sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM alert_policy_evidence WHERE id=$1)",
    )
    .bind(held.0)
    .fetch_one(&db.pool)
    .await
    .unwrap());

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_referenced_resolved_episode_cannot_starve_later_safe_episode() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let safe = insert_resolved_retention_episode(&db.pool).await;
    let blocked = insert_resolved_retention_episode(&db.pool).await;
    sqlx::query(
        r#"
        UPDATE alert_episodes
        SET resolved_at = CASE
                WHEN id=$1 THEN now() - interval '4 days'
                ELSE now() - interval '3 days'
            END,
            updated_at=now() - interval '3 days'
        WHERE id = ANY($2::uuid[])
        "#,
    )
    .bind(safe)
    .bind(vec![safe, blocked])
    .execute(&db.pool)
    .await
    .unwrap();
    let blocked_public_id = episode_public_id(&db.pool, blocked).await;
    let delivery_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO fleet_alert_notification_deliveries (
            id, channel_id, channel_name, alert_id, alert_severity,
            alert_category, status, delivery_kind, target, dedupe_key,
            payload, cooldown_until_unix
        ) VALUES (
            $1, $2, 'retention-starvation', $3, 'warning', 'job', 'queued',
            'webhook', 'https://hooks.example.invalid/retention', $4,
            '{}'::jsonb, 0
        )
        "#,
    )
    .bind(delivery_id)
    .bind(Uuid::new_v4())
    .bind(&blocked_public_id)
    .bind(format!("episode-retention-starvation:{delivery_id}"))
    .execute(&db.pool)
    .await
    .unwrap();

    let run = prune_lifecycle_events(&db.pool, AlertPolicyRetentionConfig::new(1, 1))
        .await
        .unwrap();
    assert_eq!(run.resolved_episodes_pruned, 1);
    assert!(episode_exists(&db.pool, blocked).await);
    assert!(!episode_exists(&db.pool, safe).await);
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM fleet_alert_notification_deliveries WHERE id=$1)",
    )
    .bind(delivery_id)
    .fetch_one(&db.pool)
    .await
    .unwrap());

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_resolved_episode_retention_uses_only_configured_time_horizon() {
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
    ensure_retention_client(&db.pool).await;

    let fresh_rows = 300_i64;
    let history_key = format!("retention-horizon:{}", Uuid::new_v4());
    sqlx::query(
        r#"
        WITH selected_rule AS (
            SELECT rule.id AS rule_id, rule.rule_version, rule.rule_kind,
                   rule.evidence_source, rule.name AS rule_name,
                   rule.system_seed_key, group_row.id AS group_id,
                   group_row.name AS group_name
            FROM policy_rules rule
            JOIN policy_groups group_row ON group_row.id = rule.group_id
            WHERE rule.rule_kind = 'occurrence'
            ORDER BY rule.id
            LIMIT 1
        )
        INSERT INTO alert_episodes (
            id, public_id, producer_kind, natural_key, record_kind,
            trigger_generation, trigger_severity, trigger_category, severity,
            category, target_kind, target_id, title, detail, source_status,
            evidence, lifecycle_state, triggered_at, last_confirmed_at,
            resolved_at, resolution_reason, policy_group_id, policy_rule_id,
            policy_rule_version, policy_rule_kind, policy_group_name,
            policy_rule_name, policy_rule_system_seed_key, created_at, updated_at
        )
        SELECT
            gen_random_uuid(), $1 || ':' || generation::text,
            selected_rule.evidence_source, $1, 'event', generation,
            'warning', 'job', 'warning', 'job', 'system', $1,
            'Retention horizon test', 'Retention horizon test episode', 'failed',
            '{}'::jsonb, 'resolved',
            now() - interval '2 hours', now() - interval '2 hours',
            now() - interval '1 hour', 'policy_time_elapsed',
            selected_rule.group_id, selected_rule.rule_id,
            selected_rule.rule_version, selected_rule.rule_kind,
            selected_rule.group_name, selected_rule.rule_name,
            selected_rule.system_seed_key,
            now() - interval '2 hours', now() - interval '1 hour'
        FROM selected_rule
        CROSS JOIN generate_series(1, $2) generation
        "#,
    )
    .bind(&history_key)
    .bind(fresh_rows)
    .execute(&db.pool)
    .await
    .unwrap();

    let event_episode: Uuid = sqlx::query_scalar(
        "SELECT id FROM alert_episodes WHERE natural_key=$1 AND trigger_generation=1",
    )
    .bind(&history_key)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let aged_orphan: Uuid = sqlx::query_scalar(
        "SELECT id FROM alert_episodes WHERE natural_key=$1 AND trigger_generation=2",
    )
    .bind(&history_key)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let lifecycle_event_id = format!("retention-horizon-event:{}", Uuid::new_v4());
    let event_seq: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO alert_lifecycle_events (
            id, episode_id, trigger_generation, edge_kind, event_id,
            event_predicates, payload, occurred_at, created_at
        ) VALUES (
            gen_random_uuid(), $1, 1, 'alert.resolved', $2,
            ARRAY['alert.resolved','alert.category:job','alert.severity:warning'],
            '{}'::jsonb, now() - interval '2 hours', now() - interval '2 hours'
        )
        RETURNING event_seq
        "#,
    )
    .bind(event_episode)
    .bind(&lifecycle_event_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO webhook_events (
            id, kind, event_id, event_predicates, payload, occurred_at,
            processed_at, alert_lifecycle_event_seq
        ) VALUES (
            gen_random_uuid(), 'alert.resolved', $1,
            ARRAY['alert.resolved'], '{}'::jsonb,
            now() - interval '2 hours', now() - interval '1 hour', $2
        )
        "#,
    )
    .bind(&lifecycle_event_id)
    .bind(event_seq)
    .execute(&db.pool)
    .await
    .unwrap();
    set_lifecycle_receipt_frontiers(&db.pool, event_seq, event_seq).await;

    let within_horizon =
        process_alert_policy_retention(&db.pool, AlertPolicyRetentionConfig::new(90, 17))
            .await
            .unwrap();
    assert_eq!(within_horizon.lifecycle_events_pruned, 0);
    assert_eq!(within_horizon.resolved_episodes_pruned, 0);
    assert!(lifecycle_event_exists(&db.pool, event_seq).await);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM alert_episodes WHERE natural_key=$1 AND lifecycle_state='resolved'",
        )
        .bind(&history_key)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        fresh_rows
    );

    sqlx::query(
        r#"
        UPDATE alert_episodes
        SET triggered_at=now() - interval '92 days',
            last_confirmed_at=now() - interval '92 days',
            resolved_at=now() - interval '91 days',
            created_at=now() - interval '92 days',
            updated_at=now() - interval '91 days'
        WHERE id=$1
        "#,
    )
    .bind(aged_orphan)
    .execute(&db.pool)
    .await
    .unwrap();

    let beyond_horizon =
        process_alert_policy_retention(&db.pool, AlertPolicyRetentionConfig::new(90, 17))
            .await
            .unwrap();
    assert_eq!(beyond_horizon.lifecycle_events_pruned, 0);
    assert_eq!(beyond_horizon.resolved_episodes_pruned, 1);
    assert!(!episode_exists(&db.pool, aged_orphan).await);
    assert!(episode_exists(&db.pool, event_episode).await);
    assert!(lifecycle_event_exists(&db.pool, event_seq).await);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM alert_episodes WHERE natural_key=$1 AND lifecycle_state='resolved'",
        )
        .bind(&history_key)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        fresh_rows - 1
    );

    let stable = process_alert_policy_retention(&db.pool, AlertPolicyRetentionConfig::new(90, 17))
        .await
        .unwrap();
    assert_eq!(stable.resolved_episodes_pruned, 0);
    db.cleanup().await;
}

async fn insert_projected_lifecycle_event(
    pool: &PgPool,
    processed: bool,
) -> (i64, Uuid, Uuid, String) {
    let episode_id = insert_resolved_retention_episode(pool).await;
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
        INSERT INTO alert_lifecycle_consumer_receipts (
            consumer_kind, event_seq, status, output_id, output_occurred_at
        ) VALUES (
            'webhook', $1,
            CASE WHEN $4 THEN 'completed' ELSE 'pending' END,
            CASE WHEN $4 THEN $2 ELSE NULL END,
            CASE WHEN $4 THEN $3 ELSE NULL END
        )
        "#,
    )
    .bind(event_seq)
    .bind(webhook_id)
    .bind(webhook_occurred_at)
    .bind(processed)
    .execute(pool)
    .await
    .unwrap();
    (event_seq, lifecycle_id, episode_id, event_id)
}

async fn insert_resolved_retention_episode(pool: &PgPool) -> Uuid {
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
    episode_id
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
            event_armed_at
        ) VALUES (
            $1, $2, TRUE, NULL, '*', ARRAY[]::text[], NULL, NULL, NULL,
            NULL, NULL, NULL, 'event', 'alert.triggered', 1,
            now() - interval '10 days'
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

async fn set_lifecycle_receipt_frontiers(pool: &PgPool, webhook: i64, schedule: i64) {
    sqlx::query(
        r#"
        INSERT INTO alert_lifecycle_consumer_receipts (
            consumer_kind, event_seq, status, output_id, output_occurred_at
        )
        SELECT consumer_kind, lifecycle.event_seq,
               CASE
                   WHEN consumer_kind='webhook' AND lifecycle.event_seq <= $1
                     OR consumer_kind='schedule' AND lifecycle.event_seq <= $2
                   THEN 'completed'
                   ELSE 'pending'
               END,
               CASE WHEN consumer_kind='webhook' AND lifecycle.event_seq <= $1
                    THEN webhook.id ELSE NULL END,
               CASE WHEN consumer_kind='webhook' AND lifecycle.event_seq <= $1
                    THEN webhook.occurred_at ELSE NULL END
        FROM alert_lifecycle_events lifecycle
        CROSS JOIN (VALUES ('webhook'::text), ('schedule'::text)) consumer(consumer_kind)
        LEFT JOIN webhook_events webhook
          ON webhook.alert_lifecycle_event_seq=lifecycle.event_seq
        ON CONFLICT (consumer_kind, event_seq) DO UPDATE SET
            status=EXCLUDED.status,
            claim_id=NULL,
            output_id=EXCLUDED.output_id,
            output_occurred_at=EXCLUDED.output_occurred_at,
            updated_at=clock_timestamp()
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

async fn episode_exists(pool: &PgPool, episode_id: Uuid) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM alert_episodes WHERE id=$1)")
        .bind(episode_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn episode_public_id(pool: &PgPool, episode_id: Uuid) -> String {
    sqlx::query_scalar("SELECT public_id FROM alert_episodes WHERE id=$1")
        .bind(episode_id)
        .fetch_one(pool)
        .await
        .unwrap()
}
