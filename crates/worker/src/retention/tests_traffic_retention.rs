use super::{
    count_frontier_queries_for_test, process_next_traffic_active_cycle_rebuild,
    process_traffic_retention, process_traffic_retention_phase, raw_frontier_after_sql,
    raw_frontier_start_sql, raw_promotion_sql, raw_stream_resume_sql,
    reset_traffic_phase_cursors_for_test, rollup_frontier_start_sql, rollup_promotion_sql,
    traffic_retention_phase_has_remaining_work, traffic_retention_phase_next_at,
    TrafficActiveCycleRebuildOutcome, TrafficRetentionPhase, GROUP_BATCH, MAX_RAW_UNIT_SOURCE_ROWS,
    PROMOTION_RAW_PREFIX_LIMIT, PROMOTION_SOURCE_ROW_LIMIT, TIERS,
};
use crate::{
    history_retention::{TelemetryHistoryRetentionDrain, TelemetryHistoryRetentionPage},
    test_support::PgWorkerTestDb,
};
use serde_json::Value;
use std::time::{Duration, Instant};
use vpsman_common::TRAFFIC_COUNTER_RAW_RETENTION_DAYS;

#[derive(Debug, Default, PartialEq)]
struct TrafficPhaseCursorTestState {
    traffic_client_id: Option<String>,
    traffic_source_kind: Option<String>,
    traffic_interface: Option<String>,
    traffic_lane: Option<String>,
    traffic_frontier_start: Option<chrono::DateTime<chrono::Utc>>,
    traffic_scan_after: Option<chrono::DateTime<chrono::Utc>>,
}

async fn wait_for_advisory_waiter(pool: &sqlx::PgPool, failure: &str) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                r#"
                SELECT EXISTS (
                    SELECT 1
                    FROM pg_locks
                    WHERE locktype = 'advisory'
                      AND database = (
                          SELECT oid FROM pg_database
                          WHERE datname = current_database()
                      )
                      AND NOT granted
                )
                "#,
            )
            .fetch_one(pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect(failure);
}

async fn wait_for_committed_frontier_progress(
    listener: &mut sqlx::postgres::PgListener,
    channel: &str,
    payload: &str,
    failure: &str,
) {
    let notification = tokio::time::timeout(Duration::from_secs(5), listener.recv())
        .await
        .expect(failure)
        .unwrap();
    assert_eq!(notification.channel(), channel);
    assert_eq!(notification.payload(), payload);
}

#[tokio::test]
async fn reset_rule_producer_only_advances_exact_rebuild_revision() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "traffic-reset-owner";
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1, $1, decode('', 'hex'), 'online')",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_streams (
            client_id, source_kind, interface, promoted_boundary_safe
        ) VALUES ($1, 'host', 'eth0', TRUE)
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let bucket: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT date_bin('1 hour', now() - interval '2 hours', TIMESTAMPTZ '1970-01-01')",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let mut fixture = db.pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('vpsman.traffic_hourly_derivations_prepublished', 'on', true)")
        .execute(&mut *fixture)
        .await
        .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_hourly_usage (
            client_id, source_kind, interface, bucket_start,
            rx_bytes, tx_bytes, rx_reset_count, tx_reset_count,
            sample_count, first_observed_at, latest_observed_at
        ) VALUES ($1, 'host', 'eth0', $2, 11, 17, 0, 0, 1, $2, $2)
        "#,
    )
    .bind(client_id)
    .bind(bucket)
    .execute(&mut *fixture)
    .await
    .unwrap();
    fixture.commit().await.unwrap();

    sqlx::query(
        r#"
        INSERT INTO vps_rule_values (client_id, key, value_raw, value_json)
        VALUES ($1, 'traffic.reset_day', '1 05:00', '{"day":1,"hour":5}')
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM traffic_counter_active_cycle_usage WHERE client_id=$1",
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        before, 0,
        "the rule producer must not rebuild retained usage"
    );
    let revisions: (i64, i64) = sqlx::query_as(
        "SELECT requested_revision, materialized_revision FROM traffic_counter_active_cycle_rebuild_work WHERE client_id=$1",
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(revisions, (1, 0));

    assert_eq!(
        process_next_traffic_active_cycle_rebuild(&db.pool)
            .await
            .unwrap(),
        TrafficActiveCycleRebuildOutcome::Published
    );
    assert_eq!(
        process_next_traffic_active_cycle_rebuild(&db.pool)
            .await
            .unwrap(),
        TrafficActiveCycleRebuildOutcome::Current
    );
    let revisions: (i64, i64, bool) = sqlx::query_as(
        "SELECT requested_revision, materialized_revision, lease_id IS NULL FROM traffic_counter_active_cycle_rebuild_work WHERE client_id=$1",
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(revisions, (1, 1, true));
    let after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM traffic_counter_active_cycle_usage WHERE client_id=$1",
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        after, 1,
        "the named consumer publishes the exact client cache"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn failed_active_cycle_owner_is_deferred_without_blocking_healthy_owner() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    for (client_id, source_revision, materialized_revision) in [
        ("traffic-active-defer-a", 1_i64, 0_i64),
        ("traffic-active-defer-b", 0_i64, 0_i64),
    ] {
        sqlx::query(
            "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1, $1, decode('', 'hex'), 'online')",
        )
        .bind(client_id)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO traffic_counter_streams (
                client_id, source_kind, interface,
                source_revision, materialized_revision,
                sample_edge_revision, promoted_boundary_safe
            ) VALUES ($1, 'host', 'eth0', $2, $3, $3, TRUE)
            "#,
        )
        .bind(client_id)
        .bind(source_revision)
        .bind(materialized_revision)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO vps_rule_values (client_id, key, value_raw, value_json)
            VALUES ($1, 'traffic.reset_day', '1 00:00',
                    '{"day":1,"hour":0}'::jsonb)
            "#,
        )
        .bind(client_id)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    sqlx::query(
        r#"
        UPDATE traffic_counter_active_cycle_rebuild_work
        SET requested_at = CASE client_id
                WHEN 'traffic-active-defer-a' THEN now() - interval '2 seconds'
                ELSE now() - interval '1 second'
            END,
            next_attempt_at = now()
        WHERE client_id IN ('traffic-active-defer-a', 'traffic-active-defer-b')
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let first = process_next_traffic_active_cycle_rebuild(&db.pool)
        .await
        .unwrap();
    let TrafficActiveCycleRebuildOutcome::Deferred { client_id, error } = first else {
        panic!("unready exact owner was not deferred: {first:?}");
    };
    assert_eq!(client_id, "traffic-active-defer-a");
    assert!(error.contains("unready stream authority"), "{error}");
    let failed_state: (i64, i64, bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT requested_revision, materialized_revision,
               lease_id IS NULL, last_error IS NOT NULL, next_attempt_at > now()
        FROM traffic_counter_active_cycle_rebuild_work
        WHERE client_id = 'traffic-active-defer-a'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(failed_state, (1, 0, true, true, true));

    assert_eq!(
        process_next_traffic_active_cycle_rebuild(&db.pool)
            .await
            .unwrap(),
        TrafficActiveCycleRebuildOutcome::Published
    );
    assert_eq!(
        process_next_traffic_active_cycle_rebuild(&db.pool)
            .await
            .unwrap(),
        TrafficActiveCycleRebuildOutcome::Current
    );
    let healthy_state: (i64, i64, bool) = sqlx::query_as(
        r#"
        SELECT requested_revision, materialized_revision, lease_id IS NULL
        FROM traffic_counter_active_cycle_rebuild_work
        WHERE client_id = 'traffic-active-defer-b'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(healthy_state, (1, 1, true));

    db.cleanup().await;
}

#[tokio::test]
async fn active_cycle_writers_lock_only_exact_stream_owners_in_canonical_order() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    for client_id in ["traffic-active-lock-a", "traffic-active-lock-b"] {
        sqlx::query(
            "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1, $1, decode('', 'hex'), 'online')",
        )
        .bind(client_id)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO traffic_counter_streams (
                client_id, source_kind, interface, promoted_boundary_safe
            ) VALUES ($1, 'host', 'eth0', TRUE)
            "#,
        )
        .bind(client_id)
        .execute(&db.pool)
        .await
        .unwrap();
    }

    let mut owner = db.pool.begin().await.unwrap();
    sqlx::query(
        r#"
        SELECT 1
        FROM traffic_counter_streams
        WHERE client_id = 'traffic-active-lock-a'
          AND source_kind = 'host'
          AND interface = 'eth0'
        FOR UPDATE
        "#,
    )
    .execute(&mut *owner)
    .await
    .unwrap();

    let contender_pool = db.additional_pool(1).await.unwrap();
    let mut contender = contender_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout = '250ms'")
        .execute(&mut *contender)
        .await
        .unwrap();
    let blocked = sqlx::query(
        "SELECT refresh_traffic_counter_active_cycle_usage(ARRAY['traffic-active-lock-a']::text[])",
    )
    .execute(&mut *contender)
    .await
    .unwrap_err();
    assert_eq!(
        blocked
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("55P03"),
        "the active-cycle rebuild bypassed its exact stream owner"
    );
    contender.rollback().await.unwrap();

    let active_delta = r#"
        SELECT apply_traffic_counter_active_cycle_usage_deltas(
            ARRAY[$1]::text[], ARRAY['host']::text[], ARRAY['eth0']::text[],
            ARRAY[date_bin(
                '1 hour', now() - interval '2 hours',
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
            )]::timestamptz[],
            ARRAY[0]::bigint[], ARRAY[0]::bigint[],
            ARRAY[0]::bigint[], ARRAY[0]::bigint[]
        )
    "#;
    let mut delta_contender = contender_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout = '250ms'")
        .execute(&mut *delta_contender)
        .await
        .unwrap();
    let blocked = sqlx::query(active_delta)
        .bind("traffic-active-lock-a")
        .execute(&mut *delta_contender)
        .await
        .unwrap_err();
    assert_eq!(
        blocked
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("55P03"),
        "the active-cycle delta bypassed its exact stream owner"
    );
    delta_contender.rollback().await.unwrap();

    let mut unrelated = contender_pool.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout = '250ms'")
        .execute(&mut *unrelated)
        .await
        .unwrap();
    sqlx::query(
        "SELECT refresh_traffic_counter_active_cycle_usage(ARRAY['traffic-active-lock-b']::text[])",
    )
    .execute(&mut *unrelated)
    .await
    .unwrap();
    sqlx::query(active_delta)
        .bind("traffic-active-lock-b")
        .execute(&mut *unrelated)
        .await
        .unwrap();
    unrelated.commit().await.unwrap();
    owner.rollback().await.unwrap();

    contender_pool.close().await;
    db.cleanup().await;
}

async fn traffic_phase_cursor_test_state(
    pool: &sqlx::PgPool,
    source_bucket_secs: i32,
    destination_bucket_secs: i32,
) -> TrafficPhaseCursorTestState {
    let row = sqlx::query(
        r#"
        SELECT
            traffic_client_id,
            traffic_source_kind,
            traffic_interface,
            traffic_lane,
            traffic_frontier_start,
            traffic_scan_after
        FROM traffic_history_retention_cursors
        WHERE domain = 'traffic_counter_samples'
          AND source_bucket_secs = $1
          AND destination_bucket_secs = $2
        "#,
    )
    .bind(source_bucket_secs)
    .bind(destination_bucket_secs)
    .fetch_one(pool)
    .await
    .unwrap();
    TrafficPhaseCursorTestState {
        traffic_client_id: sqlx::Row::try_get(&row, "traffic_client_id").unwrap(),
        traffic_source_kind: sqlx::Row::try_get(&row, "traffic_source_kind").unwrap(),
        traffic_interface: sqlx::Row::try_get(&row, "traffic_interface").unwrap(),
        traffic_lane: sqlx::Row::try_get(&row, "traffic_lane").unwrap(),
        traffic_frontier_start: sqlx::Row::try_get(&row, "traffic_frontier_start").unwrap(),
        traffic_scan_after: sqlx::Row::try_get(&row, "traffic_scan_after").unwrap(),
    }
}

#[test]
fn traffic_tiers_match_the_accepted_schedule() {
    assert_eq!(
        TIERS
            .iter()
            .map(|tier| (
                tier.source_secs.to_vec(),
                tier.destination_secs,
                tier.source_retention_days
            ))
            .collect::<Vec<_>>(),
        vec![
            (vec![3600, 10800, 21600], 86400, 366),
            (vec![3600, 10800], 21600, 181),
            (vec![3600], 10800, 91),
        ]
    );
}

#[test]
fn worst_case_raw_transaction_fits_the_source_row_budget() {
    assert_eq!(MAX_RAW_UNIT_SOURCE_ROWS, 1_441);
    const MAX_DAILY_UNITS: i64 = PROMOTION_SOURCE_ROW_LIMIT / MAX_RAW_UNIT_SOURCE_ROWS;
    assert_eq!(MAX_DAILY_UNITS, 13);
    const { assert!(MAX_DAILY_UNITS * MAX_RAW_UNIT_SOURCE_ROWS <= PROMOTION_SOURCE_ROW_LIMIT) };
    const { assert!((MAX_DAILY_UNITS + 1) * MAX_RAW_UNIT_SOURCE_ROWS > PROMOTION_SOURCE_ROW_LIMIT) };
}

#[test]
fn bounded_pages_do_not_impose_a_retention_throughput_ceiling() {
    const FINEST_SOURCE_SECS: i64 = 60;
    const STEADY_DESTINATION_SECS: i64 = 3_600;
    const STEADY_UNIT_ROWS: i64 = STEADY_DESTINATION_SECS / FINEST_SOURCE_SECS + 1;
    const UNITS_PER_STREAM_PER_DAILY_BOUNDARY: i64 = 86_400 / STEADY_DESTINATION_SECS;
    const UNITS_PER_TRANSACTION: i64 = PROMOTION_SOURCE_ROW_LIMIT / MAX_RAW_UNIT_SOURCE_ROWS;
    const TRANSACTIONS_PER_STREAM: i64 =
        (UNITS_PER_STREAM_PER_DAILY_BOUNDARY + UNITS_PER_TRANSACTION - 1) / UNITS_PER_TRANSACTION;

    assert_eq!(STEADY_UNIT_ROWS, 61);
    assert_eq!(UNITS_PER_STREAM_PER_DAILY_BOUNDARY, 24);
    assert_eq!(UNITS_PER_TRANSACTION, 13);
    assert_eq!(TRANSACTIONS_PER_STREAM, 2);
    const {
        assert!(UNITS_PER_TRANSACTION * MAX_RAW_UNIT_SOURCE_ROWS <= PROMOTION_SOURCE_ROW_LIMIT);
    }
    // This is a transaction quantum, not a throughput ceiling: the runtime
    // immediately continues while mutations remain and has no page/pass cap.
}

#[tokio::test]
async fn common_drain_processes_traffic_beyond_one_registry_page() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "traffic-work-conserving";
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1, $1, decode('', 'hex'), 'online')",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        WITH anchor AS (
            SELECT date_bin(
                '1 hour', now() - interval '45 days',
                TIMESTAMPTZ '1970-01-01'
            ) AS observed_at
        )
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source
        )
        SELECT
            $1, 'host', format('eth-%s', lpad(stream_number::text, 2, '0')),
            anchor.observed_at + sample.minute_offset * interval '1 minute',
            1000 + sample.minute_offset * 10,
            2000 + sample.minute_offset * 20,
            0, 0, 'agent_networks'
        FROM generate_series(0, 1) stream(stream_number)
        CROSS JOIN anchor
        CROSS JOIN (VALUES (0), (1), (60), (61)) sample(minute_offset)
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    reset_traffic_phase_cursors_for_test(&db.pool)
        .await
        .unwrap();

    let mut drain = TelemetryHistoryRetentionDrain::default();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match drain.process_page(&db.pool).await.unwrap() {
                TelemetryHistoryRetentionPage::MoreWork => {}
                TelemetryHistoryRetentionPage::CurrentUntil(_) => break,
                TelemetryHistoryRetentionPage::OwnerFailed(error) => {
                    panic!("retention owner failed: {error}")
                }
            }
        }
    })
    .await
    .expect("work-conserving bounded rotations did not converge");
    let run = drain.finish();
    assert_eq!(run.traffic_raw_rows_promoted, 6);
    let state: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            count(*)::bigint,
            count(*) FILTER (WHERE inbound_promoted)::bigint,
            (SELECT count(*)::bigint
             FROM traffic_counter_rollups
             WHERE client_id = $1)
        FROM traffic_counter_samples
        WHERE client_id = $1
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(state, (2, 2, 4));

    reset_traffic_phase_cursors_for_test(&db.pool)
        .await
        .unwrap();
    db.cleanup().await;
}

#[tokio::test]
async fn common_drain_returns_preserved_traffic_destination_conflict_without_mutation() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "traffic-work-conflict";
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1, $1, decode('', 'hex'), 'online')",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let anchor: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT date_bin('1 hour', now() - interval '45 days', TIMESTAMPTZ '1970-01-01')",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source
        ) VALUES
            ($1, 'host', 'eth0', $2, 100, 200, 0, 0, 'agent_networks'),
            ($1, 'host', 'eth0', $2 + interval '1 minute',
                110, 220, 0, 0, 'agent_networks')
        "#,
    )
    .bind(client_id)
    .bind(anchor)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        ) VALUES (
            $1, 'host', 'eth0', 'live', 3600, $2,
            1, 1, 1, 1, 1, 0, 0, 0, $2, $2
        )
        "#,
    )
    .bind(client_id)
    .bind(anchor)
    .execute(&db.pool)
    .await
    .unwrap();
    reset_traffic_phase_cursors_for_test(&db.pool)
        .await
        .unwrap();

    for attempt in 0..2 {
        let error = tokio::time::timeout(
            Duration::from_secs(30),
            crate::process_telemetry_history_retention_drain(&db.pool),
        )
        .await
        .expect("preserved conflict caused catch-up to spin")
        .unwrap_err();
        let error = format!("{error:#}");
        assert!(
            error.contains("processing one traffic retention phase"),
            "attempt {attempt}: {error}"
        );
        assert!(
            error.contains(
                "traffic raw promotion found 1 unsupported pre-existing destination conflicts"
            ),
            "attempt {attempt}: {error}"
        );

        let counts: (i64, i64) = sqlx::query_as(
            r#"
            SELECT
                (SELECT count(*) FROM traffic_counter_samples WHERE client_id = $1),
                (SELECT count(*) FROM traffic_counter_rollups WHERE client_id = $1)
            "#,
        )
        .bind(client_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let cursor = traffic_phase_cursor_test_state(&db.pool, 0, -1).await;
        assert_eq!(
            counts,
            (2, 1),
            "attempt {attempt} mutated evidence or certified the failed page"
        );
        assert_eq!(
            cursor,
            TrafficPhaseCursorTestState {
                traffic_client_id: Some(client_id.to_string()),
                traffic_source_kind: Some("host".to_string()),
                traffic_interface: Some("eth0".to_string()),
                traffic_lane: Some("raw_deferred".to_string()),
                traffic_frontier_start: Some(anchor),
                traffic_scan_after: Some(anchor),
            },
            "the cursor is only a scheduling position; unchanged source still proves the page due",
        );
    }

    sqlx::query("DELETE FROM traffic_counter_rollups WHERE client_id = $1 AND bucket_secs = 3600")
        .bind(client_id)
        .execute(&db.pool)
        .await
        .unwrap();
    let repaired = tokio::time::timeout(
        Duration::from_secs(30),
        crate::process_telemetry_history_retention_drain(&db.pool),
    )
    .await
    .expect("repaired traffic retention did not converge")
    .unwrap();
    assert_eq!(repaired.traffic_raw_rows_promoted, 1);
    let repaired_state: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM traffic_counter_samples WHERE client_id = $1),
            (SELECT count(*) FROM traffic_counter_rollups
             WHERE client_id = $1 AND bucket_secs = 3600)
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(repaired_state, (1, 1));

    reset_traffic_phase_cursors_for_test(&db.pool)
        .await
        .unwrap();
    db.cleanup().await;
}

#[tokio::test]
async fn raw_host_handoff_is_atomic_disjoint_and_retry_idempotent_after_destination_race() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "traffic-network-owner-handoff";
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1, $1, decode('', 'hex'), 'online')",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let anchor: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT date_trunc('hour', now()) - interval '3 days'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch,
            sample_source
        ) VALUES
            ($1, 'host', 'eth0', $2, 100, 200, 0, 0, 'agent_networks'),
            ($1, 'host', 'eth0', $2 + interval '1 minute',
             300, 500, 0, 0, 'agent_networks')
        "#,
    )
    .bind(client_id)
    .bind(anchor)
    .execute(&db.pool)
    .await
    .unwrap();

    sqlx::query(
        r#"
        UPDATE traffic_counter_samples
        SET sample_count = 3,
            rx_bytes_sum = 450,
            tx_bytes_sum = 900,
            latest_observed_at = observed_at + interval '45 seconds'
        WHERE client_id = $1 AND source_kind = 'host'
          AND interface = 'eth0' AND observed_at = $2
        "#,
    )
    .bind(client_id)
    .bind(anchor)
    .execute(&db.pool)
    .await
    .unwrap();

    // Commit an out-of-band destination at the exact moment the natural raw
    // owner is attempting its handoff. The raw transaction must wait for the
    // unique coordinate, observe the unsupported race, and publish nothing.
    let mut destination_writer = db.pool.begin().await.unwrap();
    sqlx::query(
        r#"
        INSERT INTO telemetry_network_rates (
            client_id, interface, bucket_start, bucket_secs,
            sample_count, rx_bytes_sum, tx_bytes_sum,
            rx_bytes_avg, tx_bytes_avg, rx_bytes_last, tx_bytes_last,
            rx_counter_epoch, tx_counter_epoch,
            latest_observed_at, updated_at
        )
        SELECT client_id, interface, observed_at, 60,
               sample_count, rx_bytes_sum, tx_bytes_sum,
               round(rx_bytes_sum / sample_count::numeric)::bigint,
               round(tx_bytes_sum / sample_count::numeric)::bigint,
               rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch,
               latest_observed_at, updated_at
        FROM traffic_counter_samples
        WHERE client_id = $1 AND source_kind = 'host'
          AND observed_at = $2
        "#,
    )
    .bind(client_id)
    .bind(anchor)
    .execute(&mut *destination_writer)
    .await
    .unwrap();

    let promotion_pool = db.pool.clone();
    let mut promotion = tokio::spawn(async move {
        process_traffic_retention_phase(&promotion_pool, TrafficRetentionPhase::RawPromotion).await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(250), &mut promotion)
            .await
            .is_err(),
        "raw handoff did not wait for its conflicting destination coordinate"
    );
    destination_writer.commit().await.unwrap();
    let error = tokio::time::timeout(Duration::from_secs(5), promotion)
        .await
        .expect("raw handoff remained blocked after the destination committed")
        .unwrap()
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("unsupported concurrent traffic destination insert"),
        "unexpected raw handoff error: {error:#}"
    );
    let rolled_back: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM traffic_counter_samples
             WHERE client_id = $1),
            (SELECT count(*) FROM traffic_counter_samples
             WHERE client_id = $1 AND inbound_promoted),
            (SELECT count(*) FROM traffic_counter_rollups
             WHERE client_id = $1),
            (SELECT count(*) FROM telemetry_network_rates
             WHERE client_id = $1 AND bucket_secs = 60)
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(rolled_back, (2, 0, 0, 1));
    assert_eq!(
        traffic_phase_cursor_test_state(&db.pool, 0, -1).await,
        TrafficPhaseCursorTestState {
            traffic_client_id: Some(client_id.to_string()),
            traffic_source_kind: Some("host".to_string()),
            traffic_interface: Some("eth0".to_string()),
            traffic_lane: Some("raw_deferred".to_string()),
            traffic_frontier_start: Some(anchor),
            traffic_scan_after: Some(anchor),
        },
        "a failed exact replacement leaves source evidence due behind a scheduling-only frontier",
    );

    sqlx::query("DELETE FROM telemetry_network_rates WHERE client_id = $1 AND bucket_secs = 60")
        .bind(client_id)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(
        "DELETE FROM telemetry_dashboard_block_events WHERE client_id = $1 AND domain = 'network'",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let retry = process_traffic_retention_phase(&db.pool, TrafficRetentionPhase::RawPromotion)
        .await
        .unwrap();
    assert_eq!(retry.run.raw_rows_promoted, 1);
    let committed: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM traffic_counter_samples
             WHERE client_id = $1),
            (SELECT count(*) FROM traffic_counter_samples
             WHERE client_id = $1 AND inbound_promoted),
            (SELECT count(*) FROM traffic_counter_rollups
             WHERE client_id = $1),
            (SELECT count(*) FROM telemetry_network_rates
             WHERE client_id = $1 AND bucket_secs = 60),
            (SELECT count(*) FROM telemetry_network_rate_points_source(ARRAY[$1::TEXT])
             WHERE client_id = $1 AND interface = 'eth0'),
            (SELECT count(*) FROM (
                SELECT bucket_secs, bucket_start
                FROM telemetry_network_rate_points_source(ARRAY[$1::TEXT])
                WHERE client_id = $1 AND interface = 'eth0'
                GROUP BY bucket_secs, bucket_start
                HAVING count(*) > 1
             ) duplicate_coordinates)
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(committed, (1, 1, 1, 2, 2, 0));
    let transferred_values: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            sum(sample_count)::bigint,
            sum(rx_bytes_sum)::bigint,
            sum(tx_bytes_sum)::bigint,
            sum(rx_bytes_last)::bigint,
            sum(tx_bytes_last)::bigint,
            max(extract(epoch FROM latest_observed_at - bucket_start))::bigint
        FROM telemetry_network_rates
        WHERE client_id = $1 AND interface = 'eth0' AND bucket_secs = 60
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(transferred_values, (4, 750, 1400, 400, 700, 45));
    let due_schedule_is_day_two: bool = sqlx::query_scalar(
        r#"
        SELECT bool_and(
            due_at = destination_start + interval '2 days 5 minutes'
            AND coalesce_ready_at = destination_start + interval '5 minutes'
        )
        FROM telemetry_history_due_events
        WHERE domain = 'telemetry_network_rates'
          AND source_bucket_secs = 60
          AND destination_bucket_secs = 300
          AND destination_start = date_bin(
              interval '5 minutes', $1::timestamptz,
              TIMESTAMPTZ '1970-01-01 00:00:00+00'
          )
        "#,
    )
    .bind(anchor)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(due_schedule_is_day_two);
    let dashboard_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM telemetry_dashboard_block_events WHERE client_id = $1 AND domain = 'network'",
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(dashboard_events, 0);

    let idempotent = process_traffic_retention_phase(&db.pool, TrafficRetentionPhase::RawPromotion)
        .await
        .unwrap();
    assert!(!idempotent.attempted);
    let after_retry: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM traffic_counter_samples WHERE client_id = $1),
            (SELECT count(*) FROM traffic_counter_rollups WHERE client_id = $1),
            (SELECT count(*) FROM telemetry_network_rates
             WHERE client_id = $1 AND bucket_secs = 60)
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(after_retry, (1, 1, 2));

    // Tunnel counters share traffic accounting but are not host network-rate
    // history. Their day-one traffic handoff must never create a network row.
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch,
            sample_source
        ) VALUES
            ($1, 'tunnel', 'wg0', $2, 10, 20, 0, 0, 'agent_tunnels'),
            ($1, 'tunnel', 'wg0', $2 + interval '1 minute',
             30, 50, 0, 0, 'agent_tunnels')
        "#,
    )
    .bind(client_id)
    .bind(anchor)
    .execute(&db.pool)
    .await
    .unwrap();
    let tunnel = process_traffic_retention_phase(&db.pool, TrafficRetentionPhase::RawPromotion)
        .await
        .unwrap();
    assert_eq!(tunnel.run.raw_rows_promoted, 1);
    let tunnel_owners: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM traffic_counter_samples
             WHERE client_id = $1 AND source_kind = 'tunnel'),
            (SELECT count(*) FROM traffic_counter_rollups
             WHERE client_id = $1 AND source_kind = 'tunnel'),
            (SELECT count(*) FROM telemetry_network_rates
             WHERE client_id = $1 AND interface = 'wg0')
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(tunnel_owners, (1, 1, 0));

    db.cleanup().await;
}

#[tokio::test]
async fn raw_destination_conflict_rolls_back_source_and_restart_retries_after_frontier_wrap() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "traffic-raw-same-stream-fairness";
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1, $1, decode('', 'hex'), 'online')",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let anchor: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT date_bin('1 day', now() - interval '400 days', TIMESTAMPTZ '1970-01-01')",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch,
            sample_source
        )
        SELECT
            $1, 'host', 'eth0',
            $2 + day_number * interval '1 day'
                + minute_number * interval '1 minute',
            100 + day_number * 100 + minute_number * 10,
            200 + day_number * 200 + minute_number * 20,
            0, 0, 'agent_networks'
        FROM generate_series(0, 13) day(day_number)
        CROSS JOIN (VALUES (0), (1)) minute(minute_number)
        "#,
    )
    .bind(client_id)
    .bind(anchor)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        )
        SELECT
            $1, 'host', 'eth0', 'live', 86400,
            $2 + day_number * interval '1 day',
            1, 1, 1, 1, 1, 0, 0, 0,
            $2 + day_number * interval '1 day',
            $2 + day_number * interval '1 day'
        FROM generate_series(0, 12) day(day_number)
        "#,
    )
    .bind(client_id)
    .bind(anchor)
    .execute(&db.pool)
    .await
    .unwrap();

    let restarted = db.additional_pool(2).await.unwrap();
    for (attempt, pool) in [(0, &db.pool), (1, &restarted)] {
        let error = process_traffic_retention(pool).await.unwrap_err();
        let error = format!("{error:#}");
        assert!(
            error.contains(
                "traffic raw promotion found 13 unsupported pre-existing destination conflicts"
            ),
            "attempt {attempt}: {error}"
        );
        let counts: (i64, i64, i64) = sqlx::query_as(
            r#"
            SELECT
                (SELECT count(*) FROM traffic_counter_samples WHERE client_id = $1),
                (SELECT count(*) FROM traffic_counter_rollups
                 WHERE client_id = $1 AND bucket_secs = 86400),
                (SELECT count(*) FROM traffic_counter_rollups
                 WHERE client_id = $1 AND bucket_secs = 86400
                   AND bucket_start = $2 + interval '13 days')
            "#,
        )
        .bind(client_id)
        .bind(anchor)
        .fetch_one(pool)
        .await
        .unwrap();
        let cursor = traffic_phase_cursor_test_state(pool, 0, -1).await;
        assert_eq!(counts, (28, 13, 0), "attempt {attempt} mutated evidence");
        assert_eq!(
            cursor,
            TrafficPhaseCursorTestState {
                traffic_client_id: Some(client_id.to_string()),
                traffic_source_kind: Some("host".to_string()),
                traffic_interface: Some("eth0".to_string()),
                traffic_lane: Some("raw_deferred".to_string()),
                traffic_frontier_start: Some(anchor),
                traffic_scan_after: Some(anchor),
            },
            "the scheduling frontier may advance, but unchanged source remains due on wrap",
        );
    }

    sqlx::query(
        r#"
        DELETE FROM traffic_counter_rollups
        WHERE client_id = $1 AND bucket_secs = 86400
          AND bucket_start < $2 + interval '13 days'
        "#,
    )
    .bind(client_id)
    .bind(anchor)
    .execute(&restarted)
    .await
    .unwrap();
    let repaired_first = process_traffic_retention(&restarted).await.unwrap();
    let repaired_second = process_traffic_retention(&restarted).await.unwrap();
    assert!(repaired_first.raw_rows_promoted > 0);
    assert!(repaired_second.raw_rows_promoted > 0);
    let repaired: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM traffic_counter_samples WHERE client_id = $1),
            (SELECT count(*) FROM traffic_counter_samples
             WHERE client_id = $1 AND inbound_promoted),
            (SELECT count(*) FROM traffic_counter_rollups
             WHERE client_id = $1 AND bucket_secs = 86400)
        "#,
    )
    .bind(client_id)
    .fetch_one(&restarted)
    .await
    .unwrap();
    assert_eq!(repaired, (1, 1, 14));
    restarted.close().await;
    db.cleanup().await;
}

#[tokio::test]
async fn rollup_destination_conflict_rolls_back_both_groups_and_remains_due_after_wrap() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "traffic-rollup-same-stream-fairness";
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1, $1, decode('', 'hex'), 'online')",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let anchor: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT date_bin('3 hours', now() - interval '100 days', TIMESTAMPTZ '1970-01-01')",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        )
        SELECT $1, 'host', 'eth0', 'live', 3600,
               $2 + hour_number * interval '1 hour',
               10, 20, 1, 1, 1, 0, 0, 0,
               $2 + hour_number * interval '1 hour',
               $2 + hour_number * interval '1 hour'
        FROM generate_series(0, 5) hour(hour_number)
        "#,
    )
    .bind(client_id)
    .bind(anchor)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        ) VALUES (
            $1, 'host', 'eth0', 'live', 10800, $2,
            1, 1, 1, 1, 1, 0, 0, 0, $2, $2
        )
        "#,
    )
    .bind(client_id)
    .bind(anchor)
    .execute(&db.pool)
    .await
    .unwrap();
    register_retained_only_rollup_streams(&db.pool).await;
    sqlx::query("SELECT refresh_traffic_counter_active_cycle_usage(ARRAY[$1]::text[])")
        .bind(client_id)
        .execute(&db.pool)
        .await
        .unwrap();

    let restarted = db.additional_pool(2).await.unwrap();
    for (attempt, pool) in [(0, &db.pool), (1, &restarted)] {
        let error = process_traffic_retention(pool).await.unwrap_err();
        let error = format!("{error:#}");
        assert!(
            error.contains(
                "traffic rollup promotion from 3600s to 10800s found 1 unsupported destination conflicts"
            ),
            "attempt {attempt}: {error}"
        );
        let counts: (i64, i64) = sqlx::query_as(
            r#"
            SELECT
                (SELECT count(*) FROM traffic_counter_rollups
                 WHERE client_id = $1 AND bucket_secs = 3600),
                (SELECT count(*) FROM traffic_counter_rollups
                 WHERE client_id = $1 AND bucket_secs = 10800)
            "#,
        )
        .bind(client_id)
        .fetch_one(pool)
        .await
        .unwrap();
        let cursor = traffic_phase_cursor_test_state(pool, 3600, 10800).await;
        assert_eq!(counts, (6, 1), "attempt {attempt} mutated either group");
        assert_eq!(cursor.traffic_client_id.as_deref(), Some(client_id));
        assert_eq!(cursor.traffic_source_kind.as_deref(), Some("host"));
        assert_eq!(cursor.traffic_interface.as_deref(), Some("eth0"));
        assert_eq!(cursor.traffic_lane.as_deref(), Some("live"));
        assert_eq!(cursor.traffic_frontier_start, None);
        let positioned_bucket = cursor
            .traffic_scan_after
            .expect("the failed group left no scheduling position");
        assert!(positioned_bucket >= anchor);
        assert!(positioned_bucket <= anchor + chrono::Duration::hours(5));
        assert!(
            traffic_retention_phase_has_remaining_work(
                pool,
                TrafficRetentionPhase::RollupToThreeHours,
            )
            .await
            .unwrap(),
            "the scheduling frontier certified Current despite unchanged conflicting source",
        );
    }

    sqlx::query("DELETE FROM traffic_counter_rollups WHERE client_id = $1 AND bucket_secs = 10800")
        .bind(client_id)
        .execute(&restarted)
        .await
        .unwrap();
    let repaired = process_traffic_retention(&restarted).await.unwrap();
    assert_eq!(repaired.rollup_rows_promoted, 6);
    let state: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            count(*) FILTER (WHERE bucket_secs = 3600)::bigint,
            count(*) FILTER (WHERE bucket_secs = 10800)::bigint
        FROM traffic_counter_rollups
        WHERE client_id = $1
        "#,
    )
    .bind(client_id)
    .fetch_one(&restarted)
    .await
    .unwrap();
    assert_eq!(state, (0, 2));
    restarted.close().await;
    db.cleanup().await;
}

#[tokio::test]
async fn large_raw_retention_updates_only_affected_hours_and_keeps_active_cycle_exact() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "traffic-large-retention-hourly";
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1, $1, decode('', 'hex'), 'online')",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let (reset_day, reset_hour, cycle_start, old_anchor, recent_anchor): (
        i32,
        i32,
        chrono::DateTime<chrono::Utc>,
        chrono::DateTime<chrono::Utc>,
        chrono::DateTime<chrono::Utc>,
    ) = sqlx::query_as(
        r#"
        WITH reset AS (
            SELECT
                extract(day FROM (now() - interval '14 days')
                    AT TIME ZONE 'UTC')::integer AS reset_day,
                13::integer AS reset_hour
        ), cycle AS (
            SELECT reset.*,
                traffic_counter_cycle_start_utc(
                    reset_day, reset_hour, now()
                ) AS cycle_start
            FROM reset
        )
        SELECT reset_day, reset_hour, cycle_start,
            cycle_start + interval '1 hour',
            date_bin('1 hour', now(), TIMESTAMPTZ '1970-01-01')
        FROM cycle
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(recent_anchor - cycle_start >= chrono::Duration::days(10));

    sqlx::query("ALTER TABLE traffic_counter_samples DISABLE TRIGGER USER")
        .execute(&db.pool)
        .await
        .unwrap();
    // This large fixture deliberately bypasses every user trigger, including
    // aggregate normalization, so it must supply the complete physical row.
    sqlx::query(
        r#"
        WITH raw AS (
            SELECT $1::text AS client_id, 'host'::text AS source_kind,
                   'eth0'::text AS interface,
                   $2 + sample_number * interval '1 minute' AS observed_at,
                   CASE WHEN sample_number < 3000
                        THEN 100 + sample_number
                        ELSE 10 + sample_number - 3000 END::bigint AS rx_bytes,
                   CASE WHEN sample_number < 3000
                        THEN 200 + sample_number * 2
                        ELSE 20 + (sample_number - 3000) * 2 END::bigint AS tx_bytes,
                   CASE WHEN sample_number < 3000 THEN 0 ELSE 1 END::bigint
                       AS counter_epoch
            FROM generate_series(0, 18720) sample(sample_number)
            UNION ALL
            SELECT $1, 'host', 'eth0', $3 + interval '1 minute',
                   20000, 40000, 1
            UNION ALL
            SELECT $1, 'host', 'eth0', $3 + interval '2 minutes',
                   20010, 40020, 1
        )
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch,
            sample_source, sample_count, rx_bytes_sum, tx_bytes_sum,
            latest_observed_at, rx_usage_bytes, tx_usage_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            usage_authoritative, updated_at
        )
        SELECT client_id, source_kind, interface, observed_at,
               rx_bytes, tx_bytes, counter_epoch, counter_epoch,
               'agent_networks', 1, rx_bytes::numeric, tx_bytes::numeric,
               observed_at, 0, 0, 0, 0, 0, 0, 0, 0, FALSE,
               clock_timestamp()
        FROM raw
        "#,
    )
    .bind(client_id)
    .bind(old_anchor)
    .bind(recent_anchor)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("ALTER TABLE traffic_counter_samples ENABLE TRIGGER USER")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(
        r#"
        SELECT refresh_traffic_counter_hourly_usage(
            ARRAY[$1]::text[], ARRAY['host']::text[], ARRAY['eth0']::text[],
            ARRAY[$2]::timestamptz[], TRUE
        )
        "#,
    )
    .bind(client_id)
    .bind(old_anchor)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "SELECT refresh_traffic_counter_sample_edges(ARRAY[$1]::text[], ARRAY['host']::text[], ARRAY['eth0']::text[])",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO vps_rule_values (client_id, key, value_raw, value_json)
        VALUES (
            $1, 'traffic.reset_day',
            format('%s %s:00', $2, lpad($3::text, 2, '0')),
            jsonb_build_object('day', $2, 'hour', $3)
        )
        "#,
    )
    .bind(client_id)
    .bind(reset_day)
    .bind(reset_hour)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("SELECT refresh_traffic_counter_active_cycle_usage(ARRAY[$1]::text[])")
        .bind(client_id)
        .execute(&db.pool)
        .await
        .unwrap();

    let sentinel = chrono::DateTime::parse_from_rfc3339("2000-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    sqlx::query(
        r#"
        UPDATE traffic_counter_hourly_usage
        SET updated_at = $3
        WHERE client_id = $1 AND source_kind = 'host' AND interface = 'eth0'
          AND bucket_start = $2
        "#,
    )
    .bind(client_id)
    .bind(recent_anchor)
    .bind(sentinel)
    .execute(&db.pool)
    .await
    .unwrap();
    let before_usage = retained_traffic_usage(&db.pool, client_id).await;
    let before_active: Option<(i64, i64, i64, i64)> = sqlx::query_as(
        r#"
        SELECT rx_bytes, tx_bytes, rx_reset_count, tx_reset_count
        FROM traffic_counter_active_cycle_usage
        WHERE client_id = $1 AND source_kind = 'host' AND interface = 'eth0'
        "#,
    )
    .bind(client_id)
    .fetch_optional(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        before_active.map(|(_, _, rx_resets, tx_resets)| (rx_resets, tx_resets)),
        Some((1, 1)),
        "the >7d active cycle fixture must include a reset transition"
    );
    let run = process_traffic_retention(&db.pool).await.unwrap();
    assert!(
        run.raw_rows_promoted > 4096,
        "the proof must exercise the former whole-stream trigger threshold"
    );
    assert_eq!(
        retained_traffic_usage(&db.pool, client_id).await,
        before_usage
    );
    let after_active: Option<(i64, i64, i64, i64)> = sqlx::query_as(
        r#"
        SELECT rx_bytes, tx_bytes, rx_reset_count, tx_reset_count
        FROM traffic_counter_active_cycle_usage
        WHERE client_id = $1 AND source_kind = 'host' AND interface = 'eth0'
        "#,
    )
    .bind(client_id)
    .fetch_optional(&db.pool)
    .await
    .unwrap();
    assert_eq!(after_active, before_active);
    let recent_hour: (i64, i64, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
        r#"
        SELECT rx_bytes, tx_bytes, updated_at
        FROM traffic_counter_hourly_usage
        WHERE client_id = $1 AND source_kind = 'host' AND interface = 'eth0'
          AND bucket_start = $2
        "#,
    )
    .bind(client_id)
    .bind(recent_anchor)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(recent_hour.2, sentinel, "an unaffected hour was rebuilt");
    let revisions: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT source_revision, materialized_revision, sample_edge_revision
        FROM traffic_counter_streams
        WHERE client_id = $1 AND source_kind = 'host' AND interface = 'eth0'
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(revisions.0, revisions.1);
    assert_eq!(revisions.1, revisions.2);
    db.cleanup().await;
}

#[tokio::test]
async fn raw_next_at_treats_all_null_stream_frontiers_as_producer_only() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "traffic-null-next-at";
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1, $1, decode('', 'hex'), 'online')",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_streams (client_id, source_kind, interface)
        VALUES ($1, 'host', 'eth0')
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let next_at = traffic_retention_phase_next_at(&db.pool, TrafficRetentionPhase::RawPromotion)
        .await
        .unwrap();
    assert!(next_at.is_none());
    db.cleanup().await;
}

#[tokio::test]
async fn raw_frontier_initializes_exactly_and_steady_append_stays_hot() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "traffic-frontier-hot";
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1, $1, decode('', 'hex'), 'online')",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let anchor: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT date_bin('1 hour', now(), TIMESTAMPTZ '1970-01-01') + interval '1 minute'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source
        ) VALUES ($1, 'host', 'eth0', $2, 100, 200, 0, 0, 'agent_networks')
        "#,
    )
    .bind(client_id)
    .bind(anchor)
    .execute(&db.pool)
    .await
    .unwrap();
    let initialized: (
        chrono::DateTime<chrono::Utc>,
        chrono::DateTime<chrono::Utc>,
        chrono::DateTime<chrono::Utc>,
        i64,
        i64,
        i64,
    ) = sqlx::query_as(
        r#"
        SELECT first_unpromoted_observed_at, first_exact_observed_at,
               last_exact_observed_at, source_revision,
               materialized_revision, sample_edge_revision
        FROM traffic_counter_streams
        WHERE client_id = $1 AND source_kind = 'host' AND interface = 'eth0'
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(initialized.0, anchor);
    assert_eq!(initialized.1, anchor);
    assert_eq!(initialized.2, anchor);
    assert_eq!(initialized.3, initialized.4);
    assert_eq!(initialized.4, initialized.5);

    let mut hot_connection = db.pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_stat_reset_single_table_counters('traffic_counter_streams'::regclass)")
        .execute(&mut *hot_connection)
        .await
        .unwrap();
    let appended_at: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source
        ) VALUES (
            $1, 'host', 'eth0',
            date_bin(
                interval '1 hour', statement_timestamp(),
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
            ) + interval '2 minutes',
            130, 250, 0, 0, 'agent_networks'
        )
        RETURNING observed_at
        "#,
    )
    .bind(client_id)
    .fetch_one(&mut *hot_connection)
    .await
    .unwrap();
    sqlx::query("SELECT pg_stat_force_next_flush()")
        .execute(&mut *hot_connection)
        .await
        .unwrap();
    let hot: (i64, i64) = sqlx::query_as(
        r#"
        SELECT n_tup_upd, n_tup_hot_upd
        FROM pg_stat_user_tables
        WHERE relname = 'traffic_counter_streams'
        "#,
    )
    .fetch_one(&mut *hot_connection)
    .await
    .unwrap();
    assert!(
        hot.0 >= 1,
        "ordinary append did not update its stream authority"
    );
    assert!(
        hot.1 >= 1,
        "ordinary append produced no HOT authority update"
    );
    assert!(
        hot.0.saturating_sub(hot.1) <= 1,
        "more than the required first-frontier initialization was non-HOT: {hot:?}"
    );
    let appended: (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
        r#"
        SELECT first_unpromoted_observed_at, latest_sample_observed_at
        FROM traffic_counter_streams
        WHERE client_id = $1 AND source_kind = 'host' AND interface = 'eth0'
        "#,
    )
    .bind(client_id)
    .fetch_one(&mut *hot_connection)
    .await
    .unwrap();
    assert_eq!(appended.0, anchor, "append rewrote the exact frontier");
    assert_eq!(appended.1, appended_at);
    drop(hot_connection);

    // Historical and imported writes intentionally leave the steady HOT path
    // and recompute the exact minimum. Deleting that minimum recomputes it
    // again; no queue or eventually-consistent repair is involved.
    for (offset, source) in [(-1_i64, "agent_networks"), (-2, "vnstat_import:test")] {
        sqlx::query(
            r#"
            INSERT INTO traffic_counter_samples (
                client_id, source_kind, interface, observed_at,
                rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch,
                sample_source
            ) VALUES (
                $1, 'host', 'eth0', $2 + $3 * interval '1 minute',
                90, 180, 0, 0, $4
            )
            "#,
        )
        .bind(client_id)
        .bind(anchor)
        .bind(offset)
        .bind(source)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    let reimport_frontier: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT first_unpromoted_observed_at FROM traffic_counter_streams WHERE client_id = $1",
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(reimport_frontier, anchor - chrono::Duration::minutes(2));
    sqlx::query(
        "DELETE FROM traffic_counter_samples WHERE client_id = $1 AND observed_at = $2 - interval '2 minutes'",
    )
    .bind(client_id)
    .bind(anchor)
    .execute(&db.pool)
    .await
    .unwrap();
    let after_delete: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT first_unpromoted_observed_at FROM traffic_counter_streams WHERE client_id = $1",
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(after_delete, anchor - chrono::Duration::minutes(1));
    db.cleanup().await;
}

#[tokio::test]
async fn idle_global_frontiers_are_constant_and_stably_null_at_960_streams() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        r#"
        INSERT INTO clients (id, display_name, public_key, status)
        SELECT format('traffic-idle-%s', lpad(client_number::text, 3, '0')),
               format('traffic-idle-%s', lpad(client_number::text, 3, '0')),
               decode('', 'hex'), 'online'
        FROM generate_series(0, 119) client(client_number)
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_streams (
            client_id, source_kind, interface
        )
        SELECT client.id, 'host', format('eth%s', interface_number)
        FROM clients client
        CROSS JOIN generate_series(0, 7) interface(interface_number)
        WHERE client.id LIKE 'traffic-idle-%'
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("ANALYZE traffic_counter_streams")
        .execute(&db.pool)
        .await
        .unwrap();

    // Begin from a durable non-NULL position so the first empty pass proves a
    // wrap and clears it. The next pass must neither rotate nor rewrite it.
    sqlx::query(
        r#"
        UPDATE traffic_history_retention_cursors
        SET traffic_client_id = 'traffic-idle-000',
            traffic_source_kind = 'host',
            traffic_interface = 'eth0',
            traffic_lane = CASE
                WHEN source_bucket_secs = 0 AND destination_bucket_secs = -1
                    THEN 'raw'
                WHEN source_bucket_secs = 0 AND destination_bucket_secs = 0
                    THEN 'prune_1h_live'
                ELSE 'live'
            END,
            traffic_frontier_start = CASE
                WHEN source_bucket_secs = 0 AND destination_bucket_secs = -1
                    THEN date_trunc('minute', now())
                ELSE NULL
            END,
            traffic_scan_after = date_trunc('minute', now()),
            updated_at = clock_timestamp()
        WHERE domain = 'traffic_counter_samples'
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let transition = process_traffic_retention(&db.pool).await.unwrap();
    assert_eq!(transition.raw_rows_promoted, 0);
    assert_eq!(transition.rollup_rows_promoted, 0);
    assert_eq!(transition.rollup_rows_pruned, 0);
    let positioned_after_transition: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*)
        FROM traffic_history_retention_cursors
        WHERE domain = 'traffic_counter_samples'
          AND traffic_client_id IS NOT NULL
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(positioned_after_transition, 0);
    let timestamps_before: Vec<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        r#"
        SELECT updated_at
        FROM traffic_history_retention_cursors
        WHERE domain = 'traffic_counter_samples'
        ORDER BY source_bucket_secs, destination_bucket_secs
        "#,
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();

    let started = Instant::now();
    let (stable, frontier_queries) =
        count_frontier_queries_for_test(process_traffic_retention(&db.pool)).await;
    let elapsed = started.elapsed();
    let stable = stable.unwrap();
    assert_eq!(stable.raw_rows_promoted, 0);
    assert_eq!(stable.rollup_rows_promoted, 0);
    assert_eq!(stable.rollup_rows_pruned, 0);
    assert_eq!(
        frontier_queries, 4,
        "stable idle worker-local frontier statement count"
    );
    let timestamps_after: Vec<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        r#"
        SELECT updated_at
        FROM traffic_history_retention_cursors
        WHERE domain = 'traffic_counter_samples'
        ORDER BY source_bucket_secs, destination_bucket_secs
        "#,
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        timestamps_after, timestamps_before,
        "stable idle rewrote cursors"
    );

    let raw_plan: Value = sqlx::query_scalar(&format!(
        "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {}",
        raw_frontier_start_sql()
    ))
    .bind(TRAFFIC_COUNTER_RAW_RETENTION_DAYS)
    .bind(Vec::<String>::new())
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_named_index_bounded_relation(
        &raw_plan,
        "traffic_counter_streams",
        "traffic_counter_streams_first_unpromoted_idx",
        0,
        true,
    );
    let frontier_index_bytes: i64 = sqlx::query_scalar(
        "SELECT pg_relation_size('traffic_counter_streams_first_unpromoted_idx')",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    eprintln!(
        "960-stream traffic idle: {frontier_queries} frontier reads in {elapsed:?}; frontier index {frontier_index_bytes} bytes"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn configured_short_retention_prunes_every_coarse_width_by_bucket_end() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ('traffic-short-policy', 'traffic-short-policy', decode('', 'hex'), 'online')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO history_retention_policies (
            domain, retention_days, prune_limit, enabled,
            metadata_only, export_enabled
        ) VALUES ('traffic_counter_rollups', 32, 100, TRUE, FALSE, TRUE)
        ON CONFLICT (domain) DO UPDATE SET
            retention_days = excluded.retention_days,
            prune_limit = excluded.prune_limit,
            enabled = excluded.enabled
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        WITH source(bucket_secs, age_days) AS (
            VALUES (3600, 40), (10800, 50), (21600, 60), (86400, 70)
        ), bucketed AS (
            SELECT
                source.bucket_secs,
                date_bin(
                    make_interval(secs => source.bucket_secs),
                    now() - make_interval(days => source.age_days),
                    TIMESTAMPTZ '1970-01-01 00:00:00+00'
                ) AS bucket_start
            FROM source
        )
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        )
        SELECT
            'traffic-short-policy', 'host', 'eth0', 'live',
            bucket_secs, bucket_start, 1, 2, 1, 1, 1, 0, 0, 0,
            bucket_start, bucket_start
        FROM bucketed
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    register_retained_only_rollup_streams(&db.pool).await;
    sqlx::query(
        "SELECT refresh_traffic_counter_active_cycle_usage(ARRAY['traffic-short-policy']::text[])",
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let run = process_traffic_retention(&db.pool).await.unwrap();
    assert_eq!(run.rollup_rows_pruned, 4);
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM traffic_counter_rollups WHERE client_id = 'traffic-short-policy'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(remaining, 0);
    db.cleanup().await;
}

#[tokio::test]
async fn configured_prune_limit_is_shared_across_traffic_clients() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        r#"
        INSERT INTO clients (id, display_name, public_key, status) VALUES
            ('traffic-prune-budget-a', 'traffic-prune-budget-a', decode('', 'hex'), 'online'),
            ('traffic-prune-budget-b', 'traffic-prune-budget-b', decode('', 'hex'), 'online')
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO history_retention_policies (
            domain, retention_days, prune_limit, enabled,
            metadata_only, export_enabled
        ) VALUES ('traffic_counter_rollups', 32, 1, TRUE, FALSE, TRUE)
        ON CONFLICT (domain) DO UPDATE SET
            retention_days = excluded.retention_days,
            prune_limit = excluded.prune_limit,
            enabled = excluded.enabled
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        WITH source(client_id, age_days) AS (
            VALUES
                ('traffic-prune-budget-a', 50),
                ('traffic-prune-budget-a', 40),
                ('traffic-prune-budget-b', 49),
                ('traffic-prune-budget-b', 39)
        )
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        )
        SELECT
            client_id, 'host', 'eth0', 'live', 86400,
            date_trunc('day', now() - make_interval(days => age_days)),
            1, 2, 1, 1, 1, 0, 0, 0,
            now() - make_interval(days => age_days),
            now() - make_interval(days => age_days)
        FROM source
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    register_retained_only_rollup_streams(&db.pool).await;

    let first = process_traffic_retention(&db.pool).await.unwrap();
    assert_eq!(first.rollup_rows_pruned, 1);
    let after_first: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM traffic_counter_rollups WHERE client_id LIKE 'traffic-prune-budget-%'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(after_first, 3);

    let second = process_traffic_retention(&db.pool).await.unwrap();
    assert_eq!(second.rollup_rows_pruned, 1);
    let after_second: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM traffic_counter_rollups WHERE client_id LIKE 'traffic-prune-budget-%'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(after_second, 2);
    db.cleanup().await;
}

#[tokio::test]
async fn bounded_traffic_rollup_policy_cannot_be_disabled() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let error = sqlx::query(
        "INSERT INTO history_retention_policies (domain, retention_days, prune_limit, enabled, metadata_only, export_enabled) VALUES ($1, 32, 100, FALSE, FALSE, TRUE)",
    )
    .bind("traffic_counter_rollups")
    .execute(&db.pool)
    .await
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("history_retention_policies_bounded_domains_enabled_check"),
        "unexpected constraint error: {error}"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn mixed_finer_tiers_converge_directly_to_one_daily_bucket() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ('traffic-mixed-tier', 'traffic-mixed-tier', decode('', 'hex'), 'online')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        WITH anchor AS (
            SELECT to_timestamp(
                floor(extract(epoch FROM now() - interval '400 days') / 86400)
                    * 86400
            ) AS bucket_start
        ), source(bucket_secs, offset_secs, rx_bytes, tx_bytes) AS (
            VALUES
                (21600, 0, 6, 12), (10800, 21600, 3, 6),
                (3600, 32400, 1, 2), (3600, 36000, 1, 2),
                (3600, 39600, 1, 2), (21600, 43200, 6, 12),
                (10800, 64800, 3, 6), (3600, 75600, 1, 2),
                (3600, 79200, 1, 2), (3600, 82800, 1, 2)
        )
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        )
        SELECT
            'traffic-mixed-tier', 'host', 'eth0', 'live',
            source.bucket_secs,
            anchor.bucket_start + make_interval(secs => source.offset_secs),
            source.rx_bytes, source.tx_bytes, 1, 1, 1, 0, 0, 0,
            anchor.bucket_start + make_interval(secs => source.offset_secs),
            anchor.bucket_start + make_interval(
                secs => source.offset_secs + source.bucket_secs - 1
            )
        FROM anchor, source
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let row = sqlx::query(super::rollup_promotion_sql())
        .bind("traffic-mixed-tier")
        .bind(vec!["host"])
        .bind(vec!["eth0"])
        .bind(vec![3_600_i32, 10_800, 21_600])
        .bind(86_400_i32)
        .bind(21_600_i32)
        .bind(366_i32)
        .bind(128_i64)
        .bind(PROMOTION_SOURCE_ROW_LIMIT)
        .bind(None::<String>)
        .bind(None::<chrono::DateTime<chrono::Utc>>)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        sqlx::Row::try_get::<i64, _>(&row, "deleted_rows").unwrap(),
        10
    );
    assert_eq!(sqlx::Row::try_get::<i64, _>(&row, "conflicts").unwrap(), 0);
    let retained: (i32, i64, i64, i32) = sqlx::query_as(
        r#"
        SELECT bucket_secs, rx_bytes, tx_bytes, any_valid_count
        FROM traffic_counter_rollups
        WHERE client_id = 'traffic-mixed-tier'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(retained, (86_400, 24, 48, 10));
    db.cleanup().await;
}

#[tokio::test]
async fn sparse_finer_rows_promote_once_without_filling_gaps() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ('traffic-sparse-tier', 'traffic-sparse-tier', decode('', 'hex'), 'online')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        WITH anchor AS (
            SELECT to_timestamp(
                floor(extract(epoch FROM now() - interval '200 days') / 21600)
                    * 21600
            ) AS bucket_start
        ), source(bucket_secs, offset_secs, rx_bytes, tx_bytes) AS (
            VALUES (10800, 0, 3, 5), (3600, 18000, 7, 11)
        )
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        )
        SELECT
            'traffic-sparse-tier', 'host', 'eth0', 'live', source.bucket_secs,
            anchor.bucket_start + make_interval(secs => source.offset_secs),
            source.rx_bytes, source.tx_bytes, 1, 1, 1, 0, 0, 0,
            anchor.bucket_start + make_interval(secs => source.offset_secs),
            anchor.bucket_start + make_interval(secs => source.offset_secs)
        FROM anchor, source
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let promote = || {
        sqlx::query(super::rollup_promotion_sql())
            .bind("traffic-sparse-tier")
            .bind(vec!["host"])
            .bind(vec!["eth0"])
            .bind(vec![3_600_i32, 10_800])
            .bind(21_600_i32)
            .bind(10_800_i32)
            .bind(181_i32)
            .bind(128_i64)
            .bind(PROMOTION_SOURCE_ROW_LIMIT)
            .bind(None::<String>)
            .bind(None::<chrono::DateTime<chrono::Utc>>)
            .fetch_one(&db.pool)
    };
    let first = promote().await.unwrap();
    assert_eq!(
        sqlx::Row::try_get::<i64, _>(&first, "deleted_rows").unwrap(),
        2
    );
    assert_eq!(
        sqlx::Row::try_get::<i64, _>(&first, "conflicts").unwrap(),
        0
    );
    let retained: (i32, i64, i64, i32) = sqlx::query_as(
        r#"
        SELECT bucket_secs, rx_bytes, tx_bytes, any_valid_count
        FROM traffic_counter_rollups
        WHERE client_id = 'traffic-sparse-tier'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(retained, (21_600, 10, 16, 2));

    let second = promote().await.unwrap();
    assert_eq!(
        sqlx::Row::try_get::<i64, _>(&second, "deleted_rows").unwrap(),
        0
    );
    assert_eq!(
        sqlx::Row::try_get::<i64, _>(&second, "conflicts").unwrap(),
        0
    );
    let retained_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM traffic_counter_rollups WHERE client_id = 'traffic-sparse-tier'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(retained_count, 1);
    db.cleanup().await;
}

#[tokio::test]
async fn overlapping_finer_tiers_are_preserved_instead_of_double_counted() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ('traffic-overlap-tier', 'traffic-overlap-tier', decode('', 'hex'), 'online')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        WITH anchor AS (
            SELECT to_timestamp(
                floor(extract(epoch FROM now() - interval '200 days') / 21600)
                    * 21600
            ) AS bucket_start
        ), source(bucket_secs, offset_secs) AS (
            -- Six hours of nominal source duration, but hour zero overlaps
            -- the 3h row while hour three is missing.
            VALUES (10800, 0), (3600, 0), (3600, 14400), (3600, 18000)
        )
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        )
        SELECT
            'traffic-overlap-tier', 'host', 'eth0', 'live',
            source.bucket_secs,
            anchor.bucket_start + make_interval(secs => source.offset_secs),
            10, 20, 1, 1, 1, 0, 0, 0,
            anchor.bucket_start + make_interval(secs => source.offset_secs),
            anchor.bucket_start + make_interval(
                secs => source.offset_secs + source.bucket_secs - 1
            )
        FROM anchor, source
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let row = sqlx::query(super::rollup_promotion_sql())
        .bind("traffic-overlap-tier")
        .bind(vec!["host"])
        .bind(vec!["eth0"])
        .bind(vec![3_600_i32, 10_800])
        .bind(21_600_i32)
        .bind(10_800_i32)
        .bind(181_i32)
        .bind(128_i64)
        .bind(PROMOTION_SOURCE_ROW_LIMIT)
        .bind(None::<String>)
        .bind(None::<chrono::DateTime<chrono::Utc>>)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        sqlx::Row::try_get::<i64, _>(&row, "deleted_rows").unwrap(),
        0
    );
    let buckets: Vec<i32> = sqlx::query_scalar(
        "SELECT bucket_secs FROM traffic_counter_rollups WHERE client_id = 'traffic-overlap-tier' ORDER BY bucket_secs, bucket_start",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(buckets, vec![3_600, 3_600, 3_600, 10_800]);
    db.cleanup().await;
}

#[tokio::test]
async fn destination_conflict_sources_cannot_overlap_into_a_later_daily_bucket() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ('traffic-destination-conflict', 'traffic-destination-conflict', decode('', 'hex'), 'online')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        WITH anchor AS (
            SELECT to_timestamp(
                floor(extract(epoch FROM now() - interval '400 days') / 86400)
                    * 86400
            ) AS bucket_start
        ), source(bucket_secs, offset_secs) AS (
            VALUES
                -- The existing 6h destination conflicts with promotion of
                -- the six finer rows covering the same interval.
                (21600, 0),
                (3600, 0), (3600, 3600), (3600, 7200),
                (3600, 10800), (3600, 14400), (3600, 18000),
                -- Keep total nominal source duration at one day while
                -- leaving a real 6h gap. Only overlap detection rejects it.
                (21600, 43200), (21600, 64800)
        )
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        )
        SELECT
            'traffic-destination-conflict', 'host', 'eth0', 'live',
            source.bucket_secs,
            anchor.bucket_start + make_interval(secs => source.offset_secs),
            1, 2, 1, 1, 1, 0, 0, 0,
            anchor.bucket_start + make_interval(secs => source.offset_secs),
            anchor.bucket_start + make_interval(
                secs => source.offset_secs + source.bucket_secs - 1
            )
        FROM anchor, source
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let conflict = sqlx::query(super::rollup_promotion_sql())
        .bind("traffic-destination-conflict")
        .bind(vec!["host"])
        .bind(vec!["eth0"])
        .bind(vec![3_600_i32])
        .bind(21_600_i32)
        .bind(3_600_i32)
        .bind(181_i32)
        .bind(128_i64)
        .bind(PROMOTION_SOURCE_ROW_LIMIT)
        .bind(None::<String>)
        .bind(None::<chrono::DateTime<chrono::Utc>>)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        sqlx::Row::try_get::<i64, _>(&conflict, "deleted_rows").unwrap(),
        0
    );
    assert_eq!(
        sqlx::Row::try_get::<i64, _>(&conflict, "conflicts").unwrap(),
        1
    );

    let daily = sqlx::query(super::rollup_promotion_sql())
        .bind("traffic-destination-conflict")
        .bind(vec!["host"])
        .bind(vec!["eth0"])
        .bind(vec![3_600_i32, 10_800, 21_600])
        .bind(86_400_i32)
        .bind(21_600_i32)
        .bind(366_i32)
        .bind(128_i64)
        .bind(PROMOTION_SOURCE_ROW_LIMIT)
        .bind(None::<String>)
        .bind(None::<chrono::DateTime<chrono::Utc>>)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        sqlx::Row::try_get::<i64, _>(&daily, "deleted_rows").unwrap(),
        0
    );
    let buckets: Vec<i32> = sqlx::query_scalar(
        "SELECT bucket_secs FROM traffic_counter_rollups WHERE client_id = 'traffic-destination-conflict' ORDER BY bucket_secs, bucket_start",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        buckets,
        vec![3_600; 6]
            .into_iter()
            .chain(vec![21_600; 3])
            .collect::<Vec<_>>()
    );
    db.cleanup().await;
}

#[tokio::test]
async fn raw_promotion_preserves_no_reset_usage_and_import_boundary_semantics() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ('traffic-ledger-parity', 'traffic-ledger-parity', decode('', 'hex'), 'online')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let anchor: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT date_bin('3 hours', now() - interval '100 days', TIMESTAMPTZ '1970-01-01')",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source
        ) VALUES
            ('traffic-ledger-parity', 'host', 'eth0', $1, 100, 200, 0, 0,
                'vnstat_import:test'),
            ('traffic-ledger-parity', 'host', 'eth0', $1 + interval '1 minute',
                130, 250, 0, 0, 'vnstat_import:test'),
            ('traffic-ledger-parity', 'host', 'eth0', $1 + interval '2 minutes',
                10, 20, 1, 1, 'agent_networks'),
            ('traffic-ledger-parity', 'host', 'eth0', $1 + interval '3 minutes',
                25, 45, 1, 1, 'agent_networks'),
            ('traffic-ledger-parity', 'host', 'eth0', $1 + interval '4 hours',
                50, 80, 1, 1, 'agent_networks')
        "#,
    )
    .bind(anchor)
    .execute(&db.pool)
    .await
    .unwrap();

    let usage_sql = r#"
        WITH sequenced AS (
            SELECT *,
                lag(rx_bytes) OVER (ORDER BY observed_at) previous_rx,
                lag(tx_bytes) OVER (ORDER BY observed_at) previous_tx,
                lag(rx_counter_epoch) OVER (ORDER BY observed_at) previous_rx_epoch,
                lag(tx_counter_epoch) OVER (ORDER BY observed_at) previous_tx_epoch,
                lag(sample_source) OVER (ORDER BY observed_at) previous_source
            FROM traffic_counter_samples
            WHERE client_id = 'traffic-ledger-parity'
        ), raw AS (
            SELECT
                coalesce(sum(CASE WHEN rx_counter_epoch = previous_rx_epoch
                                      AND rx_bytes >= previous_rx
                                  THEN rx_bytes - previous_rx ELSE 0 END), 0)::bigint rx,
                coalesce(sum(CASE WHEN tx_counter_epoch = previous_tx_epoch
                                      AND tx_bytes >= previous_tx
                                  THEN tx_bytes - previous_tx ELSE 0 END), 0)::bigint tx,
                count(*) FILTER (
                    WHERE previous_rx_epoch IS NOT NULL
                      AND rx_counter_epoch <> previous_rx_epoch
                      AND NOT (previous_source LIKE 'vnstat_import:%'
                               AND sample_source NOT LIKE 'vnstat_import:%')
                )::bigint resets
            FROM sequenced
        ), retained AS (
            SELECT coalesce(sum(rx_bytes), 0)::bigint rx,
                   coalesce(sum(tx_bytes), 0)::bigint tx,
                   coalesce(sum(rx_reset_count), 0)::bigint resets
            FROM traffic_counter_rollups
            WHERE client_id = 'traffic-ledger-parity'
        )
        SELECT raw.rx + retained.rx, raw.tx + retained.tx,
               raw.resets + retained.resets
        FROM raw, retained
    "#;
    let before: (i64, i64, i64) = sqlx::query_as(usage_sql).fetch_one(&db.pool).await.unwrap();
    assert_eq!(before, (70, 110, 0));

    let first = process_traffic_retention(&db.pool).await.unwrap();
    let second = process_traffic_retention(&db.pool).await.unwrap();
    assert!(first.raw_rows_promoted + second.raw_rows_promoted >= 4);
    let after: (i64, i64, i64) = sqlx::query_as(usage_sql).fetch_one(&db.pool).await.unwrap();
    assert_eq!(after, before);
    let predecessor: (i64, i64, bool) = sqlx::query_as(
        r#"
        SELECT rx_bytes, tx_bytes, inbound_promoted
        FROM traffic_counter_samples
        WHERE client_id = 'traffic-ledger-parity'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(predecessor, (50, 80, true));
    db.cleanup().await;
}

#[tokio::test]
async fn raw_promotion_preserves_authoritative_usage_validity_and_resets() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "traffic-authoritative-promotion";
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1, $1, decode('', 'hex'), 'online')",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let anchor: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT date_bin('1 hour', now() - interval '45 days', TIMESTAMPTZ '1970-01-01')",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch,
            sample_source, sample_count, rx_bytes_sum, tx_bytes_sum,
            latest_observed_at, rx_usage_bytes, tx_usage_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            usage_authoritative, updated_at
        ) VALUES
            ($1, 'host', 'eth0', $2,
                100, 200, 0, 0, 'agent_networks',
                1, 100, 200, $2,
                0, 0, 0, 0, 0, 0, 0, 0, FALSE, $2),
            ($1, 'host', 'eth0', $2 + interval '1 minute',
                50, 150, 2, 1, 'agent_networks',
                2, 150, 350, $2 + interval '1 minute 30 seconds',
                0, 7, 0, 1, 1, 2, 1, 2, TRUE, $2),
            ($1, 'host', 'eth0', $2 + interval '2 minutes',
                70, 20, 2, 3, 'agent_networks',
                2, 120, 170, $2 + interval '2 minutes 30 seconds',
                11, 0, 1, 0, 1, 0, 2, 2, TRUE, $2)
        "#,
    )
    .bind(client_id)
    .bind(anchor)
    .execute(&db.pool)
    .await
    .unwrap();

    assert_eq!(retained_traffic_usage(&db.pool, client_id).await, (11, 7));
    let promoted = promote_raw_stream(&db.pool, client_id).await;
    assert_eq!(promoted, (2, 0, 0));
    assert_eq!(retained_traffic_usage(&db.pool, client_id).await, (11, 7));
    let retained: (i64, i64, i32, i32, i32, i32, i32, i32) = sqlx::query_as(
        r#"
        SELECT rx_bytes, tx_bytes,
               rx_valid_count, tx_valid_count, any_valid_count,
               rx_reset_count, tx_reset_count, any_reset_count
        FROM traffic_counter_rollups
        WHERE client_id = $1 AND source_kind = 'host'
          AND interface = 'eth0' AND origin_kind = 'live'
          AND bucket_secs = 3600 AND bucket_start = $2
        "#,
    )
    .bind(client_id)
    .bind(anchor)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(retained, (11, 7, 1, 1, 2, 2, 3, 4));
    db.cleanup().await;
}

#[tokio::test]
async fn raw_promotion_preserves_the_predecessor_across_destination_batches() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ('traffic-batch-boundary', 'traffic-batch-boundary', decode('', 'hex'), 'online')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let anchor: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT date_bin('1 hour', now() - interval '45 days', TIMESTAMPTZ '1970-01-01')",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source
        )
        SELECT
            'traffic-batch-boundary', 'host', 'eth0',
            $1 + sample_number * interval '1 minute',
            1000 + sample_number * 10,
            2000 + sample_number * 20,
            0, 0, 'agent_networks'
        FROM generate_series(0, 129) AS sample_number
        "#,
    )
    .bind(anchor)
    .execute(&db.pool)
    .await
    .unwrap();

    let first = process_traffic_retention(&db.pool).await.unwrap();
    assert_eq!(first.raw_rows_promoted, 129);
    let boundary: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        r#"
        SELECT observed_at
        FROM traffic_counter_samples
        WHERE client_id = 'traffic-batch-boundary' AND inbound_promoted
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(boundary, anchor + chrono::Duration::minutes(129));

    let second = process_traffic_retention(&db.pool).await.unwrap();
    assert_eq!(second.raw_rows_promoted, 0);
    let third = process_traffic_retention(&db.pool).await.unwrap();
    assert_eq!(third.raw_rows_promoted, 0);
    let retained: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT count(*)::bigint, sum(rx_bytes)::bigint, sum(tx_bytes)::bigint,
               sum(any_valid_count)::bigint
        FROM traffic_counter_rollups
        WHERE client_id = 'traffic-batch-boundary'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(retained, (3, 1_290, 2_580, 129));
    let predecessor: (i64, i64, bool) = sqlx::query_as(
        r#"
        SELECT rx_bytes, tx_bytes, inbound_promoted
        FROM traffic_counter_samples
        WHERE client_id = 'traffic-batch-boundary'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(predecessor, (2_290, 4_580, true));
    let promoted_registry_parity: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (
                SELECT count(*)::bigint
                FROM traffic_counter_samples sample
                WHERE sample.client_id = 'traffic-batch-boundary'
                  AND sample.inbound_promoted
            ),
            (
                SELECT count(*)::bigint
                FROM traffic_counter_promoted_boundaries boundary
                WHERE boundary.client_id = 'traffic-batch-boundary'
            ),
            (
                SELECT count(*)::bigint
                FROM traffic_counter_promoted_boundaries boundary
                JOIN traffic_counter_samples sample USING (
                    client_id, source_kind, interface, observed_at
                )
                WHERE boundary.client_id = 'traffic-batch-boundary'
                  AND sample.inbound_promoted
            )
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        promoted_registry_parity,
        (1, 1, 1),
        "multi-batch retention must replace the promoted raw boundary and its registry key together"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn raw_lock_hole_cannot_advance_a_later_bucket() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "traffic-raw-lock-hole";
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1, $1, decode('', 'hex'), 'online')",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let anchor: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT date_bin('1 hour', now() - interval '40 days', TIMESTAMPTZ '1970-01-01')",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch,
            sample_source, inbound_promoted
        ) VALUES
            ($1, 'host', 'eth0', $2 - interval '1 minute',
                100, 100, 0, 0, 'agent_networks', TRUE),
            ($1, 'host', 'eth0', $2,
                110, 110, 0, 0, 'agent_networks', FALSE),
            ($1, 'host', 'eth0', $2 + interval '1 hour',
                120, 120, 0, 0, 'agent_networks', FALSE)
        "#,
    )
    .bind(client_id)
    .bind(anchor)
    .execute(&db.pool)
    .await
    .unwrap();
    assert_eq!(retained_traffic_usage(&db.pool, client_id).await, (20, 20));

    let mut locked_a = db.pool.begin().await.unwrap();
    sqlx::query(
        "SELECT observed_at FROM traffic_counter_samples WHERE client_id = $1 AND observed_at = $2 FOR UPDATE",
    )
    .bind(client_id)
    .bind(anchor)
    .execute(&mut *locked_a)
    .await
    .unwrap();
    let skipped = tokio::time::timeout(
        Duration::from_secs(2),
        promote_raw_stream(&db.pool, client_id),
    )
    .await
    .expect("raw promotion waited for the locked earliest row");
    assert_eq!(skipped, (0, 0, 0));
    assert_eq!(retained_traffic_usage(&db.pool, client_id).await, (20, 20));
    let unchanged: (i64, i64) = sqlx::query_as(
        r#"
        SELECT count(*) FILTER (WHERE inbound_promoted),
               (SELECT count(*) FROM traffic_counter_rollups WHERE client_id = $1)
        FROM traffic_counter_samples
        WHERE client_id = $1
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(unchanged, (1, 0), "later bucket B advanced across locked A");

    locked_a.rollback().await.unwrap();
    let retried = promote_raw_stream(&db.pool, client_id).await;
    assert_eq!(retried, (2, 0, 0));
    assert_eq!(retained_traffic_usage(&db.pool, client_id).await, (20, 20));
    let retained: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT count(*)::bigint,
               count(*) FILTER (WHERE inbound_promoted)::bigint,
               extract(epoch FROM min(observed_at) FILTER (
                   WHERE inbound_promoted
               ))::bigint
        FROM traffic_counter_samples
        WHERE client_id = $1
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(retained.0, 1);
    assert_eq!(retained.1, 1);
    assert_eq!(
        retained.2,
        (anchor + chrono::Duration::hours(1)).timestamp()
    );
    db.cleanup().await;
}

#[tokio::test]
async fn raw_origin_siblings_are_one_atomic_lock_unit() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "traffic-raw-origin-hole";
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1, $1, decode('', 'hex'), 'online')",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let anchor: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT date_bin('1 hour', now() - interval '40 days', TIMESTAMPTZ '1970-01-01')",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch,
            sample_source, inbound_promoted
        ) VALUES
            ($1, 'host', 'eth0', $2 - interval '1 minute',
                100, 100, 0, 0, 'agent_networks', TRUE),
            ($1, 'host', 'eth0', $2,
                110, 110, 0, 0, 'vnstat_import:test', FALSE),
            ($1, 'host', 'eth0', $2 + interval '1 minute',
                120, 120, 0, 0, 'agent_networks', FALSE)
        "#,
    )
    .bind(client_id)
    .bind(anchor)
    .execute(&db.pool)
    .await
    .unwrap();

    let mut locked_import = db.pool.begin().await.unwrap();
    sqlx::query(
        "SELECT observed_at FROM traffic_counter_samples WHERE client_id = $1 AND observed_at = $2 FOR UPDATE",
    )
    .bind(client_id)
    .bind(anchor)
    .execute(&mut *locked_import)
    .await
    .unwrap();
    let skipped = tokio::time::timeout(
        Duration::from_secs(2),
        promote_raw_stream(&db.pool, client_id),
    )
    .await
    .expect("sibling-origin promotion waited for a locked origin row");
    assert_eq!(skipped, (0, 0, 0));
    let skipped_state: (i64, i64) = sqlx::query_as(
        r#"
        SELECT count(*) FILTER (WHERE inbound_promoted),
               (SELECT count(*) FROM traffic_counter_rollups WHERE client_id = $1)
        FROM traffic_counter_samples
        WHERE client_id = $1
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(skipped_state, (1, 0));

    locked_import.rollback().await.unwrap();
    assert_eq!(promote_raw_stream(&db.pool, client_id).await, (2, 0, 0));
    assert_eq!(retained_traffic_usage(&db.pool, client_id).await, (20, 20));
    let origins: Vec<(String, i64, i64)> = sqlx::query_as(
        r#"
        SELECT origin_kind, rx_bytes, tx_bytes
        FROM traffic_counter_rollups
        WHERE client_id = $1
        ORDER BY origin_kind
        "#,
    )
    .bind(client_id)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        origins,
        vec![
            ("live".to_string(), 10, 10),
            ("vnstat_import".to_string(), 10, 10),
        ]
    );
    let boundary_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM traffic_counter_samples WHERE client_id = $1 AND inbound_promoted",
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(boundary_count, 1);
    db.cleanup().await;
}

#[tokio::test]
async fn blocked_raw_stream_does_not_hold_the_phase_frontier() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        r#"
        INSERT INTO clients (id, display_name, public_key, status) VALUES
            ('traffic-timeout-a', 'traffic-timeout-a', decode('', 'hex'), 'online'),
            ('traffic-timeout-b', 'traffic-timeout-b', decode('', 'hex'), 'online')
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source
        ) VALUES
            ('traffic-timeout-a', 'host', 'eth0', date_trunc('minute', now() - interval '41 days'),
                100, 200, 0, 0, 'agent_networks'),
            ('traffic-timeout-a', 'host', 'eth0', date_trunc('minute', now() - interval '41 days') + interval '1 minute',
                110, 220, 0, 0, 'agent_networks'),
            ('traffic-timeout-b', 'host', 'eth0', date_trunc('minute', now() - interval '40 days'),
                300, 400, 0, 0, 'agent_networks'),
            ('traffic-timeout-b', 'host', 'eth0', date_trunc('minute', now() - interval '40 days') + interval '1 minute',
                330, 440, 0, 0, 'agent_networks')
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let mut progress = db.notification_listener().await.unwrap();
    progress
        .listen("traffic_raw_frontier_progress")
        .await
        .unwrap();
    let mut blocker = db.pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('traffic-raw-frontier-test', 0))")
        .execute(&mut *blocker)
        .await
        .unwrap();
    sqlx::query(
        r#"
        CREATE FUNCTION block_one_traffic_client() RETURNS trigger
        LANGUAGE plpgsql AS $$
        BEGIN
            IF NEW.client_id = 'traffic-timeout-a' THEN
                PERFORM pg_advisory_xact_lock(
                    hashtextextended('traffic-raw-frontier-test', 0)
                );
            ELSIF NEW.client_id = 'traffic-timeout-b' THEN
                PERFORM pg_notify(
                    'traffic_raw_frontier_progress', NEW.client_id
                );
            END IF;
            RETURN NEW;
        END
        $$
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        CREATE TRIGGER block_one_traffic_client_trigger
        BEFORE INSERT ON traffic_counter_rollups
        FOR EACH ROW EXECUTE FUNCTION block_one_traffic_client()
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let first_pool = db.pool.clone();
    let first = tokio::spawn(async move {
        process_traffic_retention_phase(&first_pool, TrafficRetentionPhase::RawPromotion).await
    });
    wait_for_advisory_waiter(
        &db.pool,
        "the first raw page never reached its exact owned work",
    )
    .await;

    let second_pool = db.pool.clone();
    let second = tokio::spawn(async move {
        process_traffic_retention_phase(&second_pool, TrafficRetentionPhase::RawPromotion).await
    });
    wait_for_committed_frontier_progress(
        &mut progress,
        "traffic_raw_frontier_progress",
        "traffic-timeout-b",
        "the second raw page did not commit while the first exact owner remained blocked",
    )
    .await;
    blocker.rollback().await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), first)
        .await
        .expect("the blocked first raw page did not resume")
        .unwrap()
        .unwrap();
    let second = tokio::time::timeout(Duration::from_secs(5), second)
        .await
        .expect("the second raw page did not resume")
        .unwrap()
        .unwrap();
    assert_eq!(first.run.raw_rows_promoted, 1);
    assert_eq!(second.run.raw_rows_promoted, 1);
    let state: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM traffic_counter_samples
             WHERE client_id = 'traffic-timeout-a'),
            (SELECT count(*) FROM traffic_counter_samples
             WHERE client_id = 'traffic-timeout-b'),
            (SELECT count(*) FROM traffic_counter_rollups
             WHERE client_id = 'traffic-timeout-a'),
            (SELECT count(*) FROM traffic_counter_rollups
             WHERE client_id = 'traffic-timeout-b')
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(state, (1, 1, 1, 1));
    db.cleanup().await;
}

#[tokio::test]
async fn blocked_rollup_stream_does_not_hold_the_phase_frontier() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        r#"
        INSERT INTO clients (id, display_name, public_key, status) VALUES
            ('traffic-rollup-frontier-a', 'traffic-rollup-frontier-a', decode('', 'hex'), 'online'),
            ('traffic-rollup-frontier-b', 'traffic-rollup-frontier-b', decode('', 'hex'), 'online')
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        WITH clients(client_id, age_days) AS (
            VALUES
                ('traffic-rollup-frontier-a'::text, 100),
                ('traffic-rollup-frontier-b'::text, 99)
        ), anchors AS (
            SELECT client_id,
                   date_bin(
                       '3 hours', now() - make_interval(days => age_days),
                       TIMESTAMPTZ '1970-01-01'
                   ) AS bucket_start
            FROM clients
        )
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        )
        SELECT
            anchors.client_id, 'host', 'eth0', 'live', 3600,
            anchors.bucket_start + part.part_number * interval '1 hour',
            10, 20, 1, 1, 1, 0, 0, 0,
            anchors.bucket_start + part.part_number * interval '1 hour',
            anchors.bucket_start + part.part_number * interval '1 hour'
        FROM anchors
        CROSS JOIN generate_series(0, 2) part(part_number)
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    register_retained_only_rollup_streams(&db.pool).await;
    sqlx::query(
        "SELECT refresh_traffic_counter_active_cycle_usage(ARRAY['traffic-rollup-frontier-a', 'traffic-rollup-frontier-b']::text[])",
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let mut progress = db.notification_listener().await.unwrap();
    progress
        .listen("traffic_rollup_frontier_progress")
        .await
        .unwrap();
    let mut blocker = db.pool.begin().await.unwrap();
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('traffic-rollup-frontier-test', 0))",
    )
    .execute(&mut *blocker)
    .await
    .unwrap();
    sqlx::query(
        r#"
        CREATE FUNCTION block_one_traffic_rollup() RETURNS trigger
        LANGUAGE plpgsql AS $$
        BEGIN
            IF NEW.client_id = 'traffic-rollup-frontier-a'
               AND NEW.bucket_secs = 10800 THEN
                PERFORM pg_advisory_xact_lock(
                    hashtextextended('traffic-rollup-frontier-test', 0)
                );
            ELSIF NEW.client_id = 'traffic-rollup-frontier-b'
                  AND NEW.bucket_secs = 10800 THEN
                PERFORM pg_notify(
                    'traffic_rollup_frontier_progress', NEW.client_id
                );
            END IF;
            RETURN NEW;
        END
        $$
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        CREATE TRIGGER block_one_traffic_rollup_trigger
        BEFORE INSERT ON traffic_counter_rollups
        FOR EACH ROW EXECUTE FUNCTION block_one_traffic_rollup()
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let first_pool = db.pool.clone();
    let first = tokio::spawn(async move {
        process_traffic_retention_phase(&first_pool, TrafficRetentionPhase::RollupToThreeHours)
            .await
    });
    wait_for_advisory_waiter(
        &db.pool,
        "the first rollup page never reached its exact owned work",
    )
    .await;
    let second_pool = db.pool.clone();
    let second = tokio::spawn(async move {
        process_traffic_retention_phase(&second_pool, TrafficRetentionPhase::RollupToThreeHours)
            .await
    });
    wait_for_committed_frontier_progress(
        &mut progress,
        "traffic_rollup_frontier_progress",
        "traffic-rollup-frontier-b",
        "the second rollup page did not commit while the first exact owner remained blocked",
    )
    .await;
    blocker.rollback().await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), first)
        .await
        .expect("the blocked first rollup page did not resume")
        .unwrap()
        .unwrap();
    let second = tokio::time::timeout(Duration::from_secs(5), second)
        .await
        .expect("the second rollup page did not resume")
        .unwrap()
        .unwrap();
    assert_eq!(first.run.rollup_rows_promoted, 3);
    assert_eq!(second.run.rollup_rows_promoted, 3);
    let state: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            count(*) FILTER (
                WHERE client_id = 'traffic-rollup-frontier-a'
                  AND bucket_secs = 3600
            )::bigint,
            count(*) FILTER (
                WHERE client_id = 'traffic-rollup-frontier-b'
                  AND bucket_secs = 3600
            )::bigint,
            count(*) FILTER (
                WHERE client_id = 'traffic-rollup-frontier-a'
                  AND bucket_secs = 10800
            )::bigint,
            count(*) FILTER (
                WHERE client_id = 'traffic-rollup-frontier-b'
                  AND bucket_secs = 10800
            )::bigint
        FROM traffic_counter_rollups
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(state, (0, 0, 1, 1));
    db.cleanup().await;
}

#[tokio::test]
async fn blocked_terminal_stream_does_not_hold_the_phase_frontier() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        r#"
        INSERT INTO clients (id, display_name, public_key, status) VALUES
            ('traffic-terminal-frontier-a', 'traffic-terminal-frontier-a', decode('', 'hex'), 'online'),
            ('traffic-terminal-frontier-b', 'traffic-terminal-frontier-b', decode('', 'hex'), 'online')
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO history_retention_policies (
            domain, retention_days, prune_limit, enabled,
            metadata_only, export_enabled
        ) VALUES (
            'traffic_counter_rollups', 32, 100, TRUE, FALSE, TRUE
        )
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        WITH clients(client_id, age_days) AS (
            VALUES
                ('traffic-terminal-frontier-a'::text, 100),
                ('traffic-terminal-frontier-b'::text, 99)
        ), anchors AS (
            SELECT client_id,
                   date_bin(
                       '1 hour', now() - make_interval(days => age_days),
                       TIMESTAMPTZ '1970-01-01'
                   ) AS bucket_start
            FROM clients
        )
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        )
        SELECT client_id, 'host', 'eth0', 'live', 3600, bucket_start,
               10, 20, 1, 1, 1, 0, 0, 0, bucket_start, bucket_start
        FROM anchors
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    register_retained_only_rollup_streams(&db.pool).await;
    sqlx::query(
        "SELECT refresh_traffic_counter_active_cycle_usage(ARRAY['traffic-terminal-frontier-a', 'traffic-terminal-frontier-b']::text[])",
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let mut progress = db.notification_listener().await.unwrap();
    progress
        .listen("traffic_terminal_frontier_progress")
        .await
        .unwrap();
    let mut blocker = db.pool.begin().await.unwrap();
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('traffic-terminal-frontier-test', 0))",
    )
    .execute(&mut *blocker)
    .await
    .unwrap();
    sqlx::query(
        r#"
        CREATE FUNCTION block_one_terminal_prune() RETURNS trigger
        LANGUAGE plpgsql AS $$
        BEGIN
            IF OLD.client_id = 'traffic-terminal-frontier-a' THEN
                PERFORM pg_advisory_xact_lock(
                    hashtextextended('traffic-terminal-frontier-test', 0)
                );
            ELSIF OLD.client_id = 'traffic-terminal-frontier-b' THEN
                PERFORM pg_notify(
                    'traffic_terminal_frontier_progress', OLD.client_id
                );
            END IF;
            RETURN OLD;
        END
        $$
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        CREATE TRIGGER block_one_terminal_prune_trigger
        BEFORE DELETE ON traffic_counter_rollups
        FOR EACH ROW EXECUTE FUNCTION block_one_terminal_prune()
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let first_pool = db.pool.clone();
    let first = tokio::spawn(async move {
        process_traffic_retention_phase(&first_pool, TrafficRetentionPhase::TerminalPrune).await
    });
    wait_for_advisory_waiter(
        &db.pool,
        "the first terminal page never reached its exact owned work",
    )
    .await;
    let second_pool = db.pool.clone();
    let second = tokio::spawn(async move {
        process_traffic_retention_phase(&second_pool, TrafficRetentionPhase::TerminalPrune).await
    });
    wait_for_committed_frontier_progress(
        &mut progress,
        "traffic_terminal_frontier_progress",
        "traffic-terminal-frontier-b",
        "the second terminal page did not commit while the first exact owner remained blocked",
    )
    .await;
    blocker.rollback().await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), first)
        .await
        .expect("the blocked first terminal page did not resume")
        .unwrap()
        .unwrap();
    let second = tokio::time::timeout(Duration::from_secs(5), second)
        .await
        .expect("the second terminal page did not resume")
        .unwrap()
        .unwrap();
    assert_eq!(first.run.rollup_rows_pruned, 1);
    assert_eq!(second.run.rollup_rows_pruned, 1);
    let remaining: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*)
        FROM traffic_counter_rollups
        WHERE client_id LIKE 'traffic-terminal-frontier-%'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(remaining, 0);
    db.cleanup().await;
}

#[tokio::test]
async fn locked_global_frontier_does_not_starve_the_next_eligible_client() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        r#"
        WITH generated AS (
            SELECT format(
                'traffic-fair-%s', lpad(client_number::text, 2, '0')
            ) AS client_id
            FROM generate_series(0, 1) client(client_number)
        )
        INSERT INTO clients (id, display_name, public_key, status)
        SELECT client_id, client_id, decode('', 'hex'), 'online'
        FROM generated
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        WITH generated AS (
            SELECT format(
                'traffic-fair-%s', lpad(client_number::text, 2, '0')
            ) AS client_id
            FROM generate_series(0, 1) client(client_number)
        ), anchor AS (
            SELECT date_bin(
                '1 hour', now() - interval '40 days', TIMESTAMPTZ '1970-01-01'
            ) AS observed_at
        )
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source
        )
        SELECT
            generated.client_id, 'host', 'eth0',
            anchor.observed_at + sample.offset_number * interval '1 minute',
            100 + sample.offset_number * 10,
            200 + sample.offset_number * 20,
            0, 0, 'agent_networks'
        FROM generated
        CROSS JOIN anchor
        CROSS JOIN (VALUES (0), (1)) sample(offset_number)
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_streams (
            client_id, source_kind, interface
        )
        SELECT id, 'host', 'eth0'
        FROM clients
        WHERE id LIKE 'traffic-fair-%'
        ON CONFLICT DO NOTHING
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let blocked_ids = vec!["traffic-fair-00".to_string()];
    let mut blockers = db.pool.begin().await.unwrap();
    let locked = sqlx::query_scalar::<_, String>(
        "SELECT id FROM clients WHERE id = ANY($1) ORDER BY id FOR UPDATE",
    )
    .bind(&blocked_ids)
    .fetch_all(&mut *blockers)
    .await
    .unwrap();
    assert_eq!(locked, blocked_ids);

    reset_traffic_phase_cursors_for_test(&db.pool)
        .await
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), process_traffic_retention(&db.pool))
        .await
        .expect("the locked global frontier made retention wait")
        .unwrap();
    assert_eq!(
        first.raw_rows_promoted, 1,
        "the reservation must skip the locked client and consume the next eligible client"
    );
    let later_state: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM traffic_counter_samples
             WHERE client_id = 'traffic-fair-01'),
            (SELECT count(*) FROM traffic_counter_rollups
             WHERE client_id = 'traffic-fair-01')
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(later_state, (1, 1));
    let blocked_rollups: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM traffic_counter_rollups WHERE client_id = ANY($1)",
    )
    .bind(&blocked_ids)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(blocked_rollups, 0);
    assert!(
        traffic_retention_phase_has_remaining_work(&db.pool, TrafficRetentionPhase::RawPromotion,)
            .await
            .unwrap(),
        "the skipped locked client must remain durably due",
    );

    // With the only remaining candidate still locked, another bounded pass is
    // a no-op but must neither wait nor certify the raw phase complete.
    let second = tokio::time::timeout(Duration::from_secs(5), process_traffic_retention(&db.pool))
        .await
        .expect("the persistently locked frontier starved the next client")
        .unwrap();
    assert_eq!(second.raw_rows_promoted, 0);
    assert!(
        traffic_retention_phase_has_remaining_work(&db.pool, TrafficRetentionPhase::RawPromotion,)
            .await
            .unwrap(),
        "a lock-deferred no-op must remain durably due",
    );

    blockers.rollback().await.unwrap();
    reset_traffic_phase_cursors_for_test(&db.pool)
        .await
        .unwrap();
    let retried = tokio::time::timeout(Duration::from_secs(5), process_traffic_retention(&db.pool))
        .await
        .expect("the released client was not retried")
        .unwrap();
    assert_eq!(retried.raw_rows_promoted, 1);
    let recovered: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM traffic_counter_samples
             WHERE client_id = 'traffic-fair-00'),
            (SELECT count(*) FROM traffic_counter_rollups
             WHERE client_id = 'traffic-fair-00')
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(recovered, (1, 1));
    db.cleanup().await;
}

#[tokio::test]
async fn client_row_lock_precedes_advisory_and_retry_conserves_usage() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "traffic-lock-order";
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1, $1, decode('', 'hex'), 'online')",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let anchor: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT date_bin('1 hour', now() - interval '40 days', TIMESTAMPTZ '1970-01-01')",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source
        ) VALUES
            ($1, 'host', 'eth0', $2, 100, 200, 0, 0, 'agent_networks'),
            ($1, 'host', 'eth0', $2 + interval '1 minute',
                130, 250, 0, 0, 'agent_networks')
        "#,
    )
    .bind(client_id)
    .bind(anchor)
    .execute(&db.pool)
    .await
    .unwrap();
    let before = retained_traffic_usage(&db.pool, client_id).await;
    assert_eq!(before, (30, 50));

    // This is the ingest/import half of the historical inversion: it already
    // owns the client row and has not requested the traffic advisory yet.
    let mut importer = db.pool.begin().await.unwrap();
    sqlx::query_scalar::<_, String>("SELECT id FROM clients WHERE id = $1 FOR UPDATE")
        .bind(client_id)
        .fetch_one(&mut *importer)
        .await
        .unwrap();
    let skipped = tokio::time::timeout(Duration::from_secs(2), process_traffic_retention(&db.pool))
        .await
        .expect("retention waited behind the importer's client row")
        .unwrap();
    assert_eq!(skipped.raw_rows_promoted, 0);
    let importer_got_advisory: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("traffic-counters:{client_id}"))
            .fetch_one(&mut *importer)
            .await
            .unwrap();
    assert!(
        importer_got_advisory,
        "retention acquired advisory before skipping the locked client row"
    );
    importer.commit().await.unwrap();
    assert_eq!(retained_traffic_usage(&db.pool, client_id).await, before);

    let retry = process_traffic_retention(&db.pool).await.unwrap();
    assert_eq!(retry.raw_rows_promoted, 1);
    assert_eq!(retained_traffic_usage(&db.pool, client_id).await, before);
    let boundary_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM traffic_counter_samples WHERE client_id = $1 AND inbound_promoted",
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(boundary_count, 1);
    db.cleanup().await;
}

#[tokio::test]
async fn traffic_retention_plans_bound_actual_rows_with_large_unrelated_backlogs() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let target = "traffic-plan-target";
    let backlog = "traffic-plan-backlog";
    sqlx::query(
        r#"
        INSERT INTO clients (id, display_name, public_key, status) VALUES
            ($1, $1, decode('', 'hex'), 'online'),
            ($2, $2, decode('', 'hex'), 'online')
        "#,
    )
    .bind(target)
    .bind(backlog)
    .execute(&db.pool)
    .await
    .unwrap();

    // The ledger trigger is unrelated to retention planning and would turn
    // fixture construction into the dominant cost. The isolated test schema
    // restores it before any statement under test executes.
    sqlx::query("ALTER TABLE traffic_counter_samples DISABLE TRIGGER USER")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source,
            sample_count, rx_bytes_sum, tx_bytes_sum, latest_observed_at,
            rx_usage_bytes, tx_usage_bytes, rx_reset_count, tx_reset_count,
            usage_authoritative, updated_at
        )
        SELECT
            $1, 'host', 'eth0',
            date_trunc('minute', now() - interval '180 days')
                + sample_number * interval '1 minute',
            sample_number, sample_number * 2, 0, 0, 'agent_networks',
            1, sample_number, sample_number * 2,
            date_trunc('minute', now() - interval '180 days')
                + sample_number * interval '1 minute',
            0, 0, 0, 0, FALSE,
            date_trunc('minute', now() - interval '180 days')
                + sample_number * interval '1 minute'
        FROM generate_series(0, 249999) sample(sample_number)
        "#,
    )
    .bind(backlog)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source,
            sample_count, rx_bytes_sum, tx_bytes_sum, latest_observed_at,
            rx_usage_bytes, tx_usage_bytes, rx_reset_count, tx_reset_count,
            usage_authoritative, updated_at
        ) VALUES
            ($1, 'host', 'eth0',
                date_bin('1 hour', now() - interval '45 days', TIMESTAMPTZ '1970-01-01'),
                10, 20, 0, 0, 'agent_networks',
                1, 10, 20,
                date_bin('1 hour', now() - interval '45 days', TIMESTAMPTZ '1970-01-01'),
                0, 0, 0, 0, FALSE,
                date_bin('1 hour', now() - interval '45 days', TIMESTAMPTZ '1970-01-01')),
            ($1, 'host', 'eth0',
                date_bin('1 hour', now() - interval '45 days', TIMESTAMPTZ '1970-01-01')
                    + interval '1 minute',
                13, 25, 0, 0, 'agent_networks',
                1, 13, 25,
                date_bin('1 hour', now() - interval '45 days', TIMESTAMPTZ '1970-01-01')
                    + interval '1 minute',
                0, 0, 0, 0, FALSE,
                date_bin('1 hour', now() - interval '45 days', TIMESTAMPTZ '1970-01-01')
                    + interval '1 minute')
        "#,
    )
    .bind(target)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("ALTER TABLE traffic_counter_samples ENABLE TRIGGER USER")
        .execute(&db.pool)
        .await
        .unwrap();

    // 43,800 hourly buckets are five years of retained traffic. None belongs
    // to the requested stream, so every query below must avoid visiting it.
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        )
        SELECT
            $1, 'host', 'eth0', 'live', 3600, bucket_start,
            1, 2, 1, 1, 1, 0, 0, 0, bucket_start, bucket_start
        FROM generate_series(0, 43799) bucket(bucket_number)
        CROSS JOIN LATERAL (
            SELECT date_bin(
                '1 hour', now() - interval '6 years', TIMESTAMPTZ '1970-01-01'
            ) + bucket.bucket_number * interval '1 hour' AS bucket_start
        ) aligned
        "#,
    )
    .bind(backlog)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        WITH bucket AS (
            SELECT date_bin(
                '1 hour', now() - interval '200 days', TIMESTAMPTZ '1970-01-01'
            ) AS bucket_start
        )
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        )
        SELECT $1, 'host', 'eth0', 'live', 3600, bucket_start,
               7, 11, 1, 1, 1, 0, 0, 0, bucket_start, bucket_start
        FROM bucket
        "#,
    )
    .bind(target)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_streams (
            client_id, source_kind, interface
        ) VALUES ($1, 'host', 'eth0'), ($2, 'host', 'eth0')
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(target)
    .bind(backlog)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_streams (
            client_id, source_kind, interface
        )
        SELECT $1, 'host', format('unused-%s', stream_number)
        FROM generate_series(1, 10000) stream(stream_number)
        "#,
    )
    .bind(backlog)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE traffic_counter_streams stream
        SET first_unpromoted_observed_at = (
            SELECT min(sample.observed_at) AS observed_at
            FROM traffic_counter_samples sample
            WHERE sample.client_id = stream.client_id
              AND sample.source_kind = stream.source_kind
              AND sample.interface = stream.interface
              AND NOT sample.inbound_promoted
        )
        WHERE stream.client_id = ANY($1)
        "#,
    )
    .bind(vec![target, backlog])
    .execute(&db.pool)
    .await
    .unwrap();
    // EXPLAIN ANALYZE executes the promotion CTE and its rollup triggers. Give
    // only the target stream the same complete authority an import/repair
    // owner publishes; the unrelated backlog remains an isolated plan load.
    sqlx::query(
        r#"
        SELECT refresh_traffic_counter_hourly_usage(
            ARRAY[$1]::text[], ARRAY['host']::text[], ARRAY['eth0']::text[],
            ARRAY[
                date_bin(
                    '1 hour', now() - interval '45 days',
                    TIMESTAMPTZ '1970-01-01'
                )
            ]::timestamptz[], TRUE
        )
        "#,
    )
    .bind(target)
    .execute(&db.pool)
    .await
    .unwrap();
    let target_authority: (i64, i64, i64, bool, bool, i64, i64) = sqlx::query_as(
        r#"
            SELECT
                stream.source_revision,
                stream.materialized_revision,
                stream.sample_edge_revision,
                stream.promoted_boundary_safe,
                num_nonnulls(
                    stream.latest_sample_observed_at,
                    stream.latest_sample_rx_bytes,
                    stream.latest_sample_tx_bytes,
                    stream.latest_sample_rx_counter_epoch,
                    stream.latest_sample_tx_counter_epoch,
                    stream.latest_sample_source
                ) = 6 AS complete_sample_edge,
                stream.usage_row_count,
                (
                    SELECT count(*)
                    FROM traffic_counter_hourly_usage hourly
                    WHERE hourly.client_id = stream.client_id
                      AND hourly.source_kind = stream.source_kind
                      AND hourly.interface = stream.interface
                ) AS actual_hourly_rows
            FROM traffic_counter_streams stream
            WHERE stream.client_id = $1
              AND stream.source_kind = 'host'
              AND stream.interface = 'eth0'
            "#,
    )
    .bind(target)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(target_authority.0 > 0);
    assert_eq!(target_authority.0, target_authority.1);
    assert_eq!(target_authority.1, target_authority.2);
    assert!(target_authority.3);
    assert!(target_authority.4);
    assert!(target_authority.5 > 0);
    assert_eq!(target_authority.5, target_authority.6);
    let active_authority: (bool, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            billing.reset_day = 1
                AND billing.reset_hour = 0
                AND billing.cycle_start = active.cycle_start AS default_cycle,
            active.source_revision,
            active.materialized_revision,
            active.rx_bytes,
            active.tx_bytes
        FROM traffic_counter_active_cycle_usage active
        CROSS JOIN LATERAL traffic_counter_billing_context(
            active.client_id, active.completed_through
        ) billing
        WHERE active.client_id = $1
          AND active.source_kind = 'host'
          AND active.interface = 'eth0'
        "#,
    )
    .bind(target)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(active_authority.0);
    assert!(active_authority.1 > 0);
    assert_eq!(active_authority.1, active_authority.2);
    assert_eq!((active_authority.3, active_authority.4), (0, 0));
    sqlx::query(
        "ANALYZE traffic_counter_samples, traffic_counter_rollups, traffic_counter_streams",
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let raw_start_explain = format!(
        "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {}",
        raw_frontier_start_sql()
    );
    let raw_start_plan: Value = sqlx::query_scalar(&raw_start_explain)
        .bind(TRAFFIC_COUNTER_RAW_RETENTION_DAYS)
        .bind(Vec::<String>::new())
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_named_index_bounded_relation(
        &raw_start_plan,
        "traffic_counter_streams",
        "traffic_counter_streams_first_unpromoted_idx",
        2,
        true,
    );

    // The durable global keyset position reaches the next due stream directly;
    // 10,000 unrelated NULL frontiers are absent from the partial index.
    let backlog_frontier: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT first_unpromoted_observed_at FROM traffic_counter_streams WHERE client_id = $1 AND interface = 'eth0'",
    )
    .bind(backlog)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let raw_after_explain = format!(
        "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {}",
        raw_frontier_after_sql()
    );
    let raw_after_plan: Value = sqlx::query_scalar(&raw_after_explain)
        .bind(TRAFFIC_COUNTER_RAW_RETENTION_DAYS)
        .bind(backlog_frontier)
        .bind(backlog)
        .bind("host")
        .bind("eth0")
        .bind(Vec::<String>::new())
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_named_index_bounded_relation(
        &raw_after_plan,
        "traffic_counter_streams",
        "traffic_counter_streams_first_unpromoted_idx",
        2,
        true,
    );

    let target_anchor: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT min(observed_at) FROM traffic_counter_samples WHERE client_id = $1",
    )
    .bind(target)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let raw_resume_explain = format!(
        "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {}",
        raw_stream_resume_sql()
    );
    let raw_resume_plan: Value = sqlx::query_scalar(&raw_resume_explain)
        .bind(target)
        .bind("host")
        .bind("eth0")
        .bind(target_anchor)
        .bind(TRAFFIC_COUNTER_RAW_RETENTION_DAYS)
        .bind(target_anchor)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_index_bounded_relation(&raw_resume_plan, "traffic_counter_samples", 2, true);

    let rollup_frontier_explain = format!(
        "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {}",
        rollup_frontier_start_sql()
    );
    let rollup_frontier_plan: Value = sqlx::query_scalar(&rollup_frontier_explain)
        .bind(3_600_i32)
        .bind(91_i32)
        .bind(Vec::<String>::new())
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_named_index_bounded_relation(
        &rollup_frontier_plan,
        "traffic_counter_rollups",
        "traffic_counter_rollups_retention_idx",
        8,
        true,
    );

    let raw_explain = format!(
        "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {}",
        raw_promotion_sql()
    );
    let raw_plan: Value = sqlx::query_scalar(&raw_explain)
        .bind(target)
        .bind(vec!["host"])
        .bind(vec!["eth0"])
        .bind(TRAFFIC_COUNTER_RAW_RETENTION_DAYS)
        .bind(GROUP_BATCH)
        .bind(PROMOTION_SOURCE_ROW_LIMIT)
        .bind(PROMOTION_RAW_PREFIX_LIMIT)
        .bind(None::<chrono::DateTime<chrono::Utc>>)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_index_bounded_relation(&raw_plan, "traffic_counter_samples", 512, true);

    let rollup_explain = format!(
        "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {}",
        rollup_promotion_sql()
    );
    let rollup_plan: Value = sqlx::query_scalar(&rollup_explain)
        .bind(target)
        .bind(vec!["host"])
        .bind(vec!["eth0"])
        .bind(vec![3_600_i32])
        .bind(10_800_i32)
        .bind(3_600_i32)
        .bind(91_i32)
        .bind(GROUP_BATCH)
        .bind(PROMOTION_SOURCE_ROW_LIMIT)
        .bind(None::<String>)
        .bind(None::<chrono::DateTime<chrono::Utc>>)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_index_bounded_relation(&rollup_plan, "traffic_counter_rollups", 512, true);

    db.cleanup().await;
}

async fn promote_raw_stream(pool: &sqlx::PgPool, client_id: &str) -> (i64, i64, i64) {
    let row = sqlx::query(raw_promotion_sql())
        .bind(client_id)
        .bind(vec!["host"])
        .bind(vec!["eth0"])
        .bind(TRAFFIC_COUNTER_RAW_RETENTION_DAYS)
        .bind(GROUP_BATCH)
        .bind(PROMOTION_SOURCE_ROW_LIMIT)
        .bind(PROMOTION_RAW_PREFIX_LIMIT)
        .bind(None::<chrono::DateTime<chrono::Utc>>)
        .fetch_one(pool)
        .await
        .unwrap();
    (
        sqlx::Row::try_get(&row, "deleted_rows").unwrap(),
        sqlx::Row::try_get(&row, "conflicts").unwrap(),
        sqlx::Row::try_get(&row, "insert_race_conflicts").unwrap(),
    )
}

async fn register_retained_only_rollup_streams(pool: &sqlx::PgPool) {
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_streams (
            client_id, source_kind, interface, promoted_boundary_safe
        )
        SELECT DISTINCT client_id, source_kind, interface, TRUE
        FROM traffic_counter_rollups
        ON CONFLICT DO NOTHING
        "#,
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn retained_traffic_usage(pool: &sqlx::PgPool, client_id: &str) -> (i64, i64) {
    sqlx::query_as(
        r#"
        WITH sequenced AS (
            SELECT *,
                   lag(rx_bytes) OVER stream AS previous_rx,
                   lag(tx_bytes) OVER stream AS previous_tx,
                   lag(rx_counter_epoch) OVER stream AS previous_rx_epoch,
                   lag(tx_counter_epoch) OVER stream AS previous_tx_epoch
            FROM traffic_counter_samples
            WHERE client_id = $1
            WINDOW stream AS (
                PARTITION BY source_kind, interface ORDER BY observed_at
            )
        ), exact AS (
            SELECT
                coalesce(sum(CASE
                    WHEN inbound_promoted THEN 0
                    WHEN usage_authoritative THEN rx_usage_bytes
                    WHEN rx_counter_epoch = previous_rx_epoch
                     AND rx_bytes >= previous_rx
                    THEN rx_bytes - previous_rx ELSE 0 END), 0)::bigint AS rx,
                coalesce(sum(CASE
                    WHEN inbound_promoted THEN 0
                    WHEN usage_authoritative THEN tx_usage_bytes
                    WHEN tx_counter_epoch = previous_tx_epoch
                     AND tx_bytes >= previous_tx
                    THEN tx_bytes - previous_tx ELSE 0 END), 0)::bigint AS tx
            FROM sequenced
        ), retained AS (
            SELECT coalesce(sum(rx_bytes), 0)::bigint AS rx,
                   coalesce(sum(tx_bytes), 0)::bigint AS tx
            FROM traffic_counter_rollups
            WHERE client_id = $1
        )
        SELECT exact.rx + retained.rx, exact.tx + retained.tx
        FROM exact, retained
        "#,
    )
    .bind(client_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

fn assert_index_bounded_relation(
    plan: &Value,
    relation: &str,
    maximum_rows: u64,
    expect_index_condition: bool,
) {
    let expected_index = format!("{relation}_pkey");
    assert_named_index_bounded_relation(
        plan,
        relation,
        &expected_index,
        maximum_rows,
        expect_index_condition,
    );

    // Exact conflict and overlap probes may legitimately use stream-leading
    // lookup/range indexes. The dangerous alternatives are the time-first
    // indexes, which can satisfy LIMIT only after filtering unrelated streams;
    // the row and buffer budgets above independently bound every other probe.
    let forbidden_index = match relation {
        "traffic_counter_samples" => Some("traffic_counter_samples_observed_idx"),
        "traffic_counter_rollups" => Some("traffic_counter_rollups_retention_idx"),
        _ => None,
    };
    if let Some(forbidden_index) = forbidden_index {
        let root = &plan[0]["Plan"];
        assert!(
            !explain_uses_index(root, forbidden_index),
            "{relation} used time-first index {forbidden_index}: {plan}"
        );
    }
}

fn assert_named_index_bounded_relation(
    plan: &Value,
    relation: &str,
    expected_index: &str,
    maximum_rows: u64,
    expect_index_condition: bool,
) {
    let root = &plan[0]["Plan"];
    assert!(
        !explain_uses_sequential_scan(root, relation),
        "{relation} used a sequential scan: {plan}"
    );
    let examined = explain_relation_examined_rows(root, relation);
    assert!(
        examined <= maximum_rows as f64,
        "{relation} examined {examined} actual+filtered rows; budget is {maximum_rows}: {plan}"
    );
    let maximum_blocks = maximum_rows.saturating_mul(4).saturating_add(128);
    let block_visits = explain_relation_block_visits(root, relation);
    assert!(
        block_visits <= maximum_blocks as f64,
        "{relation} visited {block_visits} shared hit/read blocks; budget is {maximum_blocks}: {plan}"
    );

    assert!(
        explain_uses_index(root, expected_index),
        "{relation} did not use {expected_index}: {plan}"
    );
    if expect_index_condition {
        assert!(
            explain_uses_index_condition(root, expected_index),
            "{relation} did not expose a bounded Index Cond on {expected_index}: {plan}"
        );
    }
}

fn explain_relation_examined_rows(plan: &Value, relation: &str) -> f64 {
    let own_rows = if plan.get("Relation Name").and_then(Value::as_str) == Some(relation) {
        let loops = plan
            .get("Actual Loops")
            .and_then(Value::as_f64)
            .unwrap_or(1.0);
        let actual = plan
            .get("Actual Rows")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        let filtered = plan
            .get("Rows Removed by Filter")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        (actual + filtered) * loops
    } else {
        0.0
    };
    own_rows
        + plan
            .get("Plans")
            .and_then(Value::as_array)
            .map(|children| {
                children
                    .iter()
                    .map(|child| explain_relation_examined_rows(child, relation))
                    .sum::<f64>()
            })
            .unwrap_or_default()
}

fn explain_relation_block_visits(plan: &Value, relation: &str) -> f64 {
    let owns_relation = plan.get("Relation Name").and_then(Value::as_str) == Some(relation)
        || plan
            .get("Index Name")
            .and_then(Value::as_str)
            .is_some_and(|index| index.starts_with(relation));
    let own_blocks = if owns_relation {
        // PostgreSQL reports BUFFERS totals across all loops already. Dirtied
        // blocks are a subset of the hit/read categories, not extra visits.
        ["Shared Hit Blocks", "Shared Read Blocks"]
            .iter()
            .map(|key| plan.get(*key).and_then(Value::as_f64).unwrap_or_default())
            .sum::<f64>()
    } else {
        0.0
    };
    own_blocks
        + plan
            .get("Plans")
            .and_then(Value::as_array)
            .map(|children| {
                children
                    .iter()
                    .map(|child| explain_relation_block_visits(child, relation))
                    .sum::<f64>()
            })
            .unwrap_or_default()
}

fn explain_uses_index(plan: &Value, index: &str) -> bool {
    plan.get("Index Name").and_then(Value::as_str) == Some(index)
        || plan
            .get("Plans")
            .and_then(Value::as_array)
            .is_some_and(|children| {
                children
                    .iter()
                    .any(|child| explain_uses_index(child, index))
            })
}

fn explain_uses_index_condition(plan: &Value, index: &str) -> bool {
    let own_bounded_index = plan.get("Index Name").and_then(Value::as_str) == Some(index)
        && plan
            .get("Index Cond")
            .and_then(Value::as_str)
            .is_some_and(|condition| !condition.trim().is_empty());
    own_bounded_index
        || plan
            .get("Plans")
            .and_then(Value::as_array)
            .is_some_and(|children| {
                children
                    .iter()
                    .any(|child| explain_uses_index_condition(child, index))
            })
}

fn explain_uses_sequential_scan(plan: &Value, relation: &str) -> bool {
    let scans_relation = plan.get("Relation Name").and_then(Value::as_str) == Some(relation);
    let is_sequential = plan
        .get("Node Type")
        .and_then(Value::as_str)
        .is_some_and(|node| node.contains("Seq Scan"));
    (scans_relation && is_sequential)
        || plan
            .get("Plans")
            .and_then(Value::as_array)
            .is_some_and(|children| {
                children
                    .iter()
                    .any(|child| explain_uses_sequential_scan(child, relation))
            })
}
