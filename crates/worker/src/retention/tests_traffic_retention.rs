use super::{
    candidate_stream_prefix_after_sql, candidate_stream_prefix_start_sql,
    load_candidate_stream_prefix, process_traffic_retention, raw_promotion_sql,
    rollup_promotion_sql, rollup_prune_sql, set_candidate_stream_cursor,
    traffic_candidate_streams_sql, CANDIDATE_RAW_PREFIX_LIMIT, CANDIDATE_STREAM_SCAN_LIMIT,
    CLIENT_BATCH, GROUP_BATCH, MAX_RAW_UNIT_SOURCE_ROWS, PROMOTION_SOURCE_ROW_LIMIT,
    RAW_RETENTION_DAYS, TIERS,
};
use crate::test_support::PgWorkerTestDb;
use serde_json::Value;
use std::time::Duration;

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
fn candidate_stream_page_fits_the_worst_case_raw_budget() {
    assert_eq!(MAX_RAW_UNIT_SOURCE_ROWS, 1_441);
    assert_eq!(CANDIDATE_STREAM_SCAN_LIMIT, 13);
    const { assert!(CANDIDATE_STREAM_SCAN_LIMIT <= CLIENT_BATCH) };
    const {
        assert!(
            CANDIDATE_STREAM_SCAN_LIMIT * MAX_RAW_UNIT_SOURCE_ROWS <= PROMOTION_SOURCE_ROW_LIMIT
        )
    };
    const {
        assert!(
            (CANDIDATE_STREAM_SCAN_LIMIT + 1) * MAX_RAW_UNIT_SOURCE_ROWS
                > PROMOTION_SOURCE_ROW_LIMIT
        )
    };
}

#[test]
fn candidate_stream_pages_keep_the_cursor_predicate_sargable() {
    let start = candidate_stream_prefix_start_sql();
    assert!(!start.contains("WHERE"));
    assert!(start.contains("ORDER BY client_id, source_kind, interface"));
    assert!(start.contains("LIMIT $1"));

    let after = candidate_stream_prefix_after_sql();
    assert!(after.contains("WHERE (client_id, source_kind, interface) > ($1, $2, $3)"));
    assert!(!after.contains("IS NULL"));
    assert!(!after.contains(" OR "));
    assert!(after.contains("ORDER BY client_id, source_kind, interface"));
    assert!(after.contains("LIMIT $4"));
}

#[test]
fn raw_promotion_is_locked_conflict_safe_and_keeps_a_predecessor() {
    let query = raw_promotion_sql();
    assert!(!query.contains("sample.client_id ="));
    assert_eq!(
        query
            .matches("ORDER BY sample.client_id, sample.source_kind")
            .count(),
        2
    );
    assert!(query.contains("ORDER BY sample.client_id DESC, sample.source_kind DESC"));
    assert_eq!(query.matches("WITH seek AS MATERIALIZED").count(), 4);
    assert_eq!(query.matches("sample.observed_at) >= (").count(), 2);
    assert_eq!(query.matches("sample.observed_at) < (").count(), 1);
    assert_eq!(query.matches("seek.observed_at <").count(), 3);
    assert_eq!(query.matches("ORDER BY destination.client_id").count(), 1);
    assert!(query.contains("FOR UPDATE OF source SKIP LOCKED"));
    assert!(query.contains("ON CONFLICT DO NOTHING"));
    assert!(query.contains("earliest_units AS MATERIALIZED"));
    assert!(query.contains("complete_units AS MATERIALIZED"));
    assert!(query.contains("inserted_origins = expected_origins"));
    assert!(query.contains("AS insert_race_conflicts"));
    assert!(query.contains("previous_sample_source LIKE 'vnstat_import:%'"));
    assert!(query.contains("THEN 'vnstat_import' ELSE 'live'"));
    assert!(query.contains("interval '91 days' THEN 3600"));
    assert!(query.contains("interval '181 days' THEN 10800"));
    assert!(query.contains("interval '366 days' THEN 21600"));
    assert!(query.contains("ELSE 86400"));
}

#[test]
fn rollup_promotion_and_prune_are_bounded() {
    let promote = rollup_promotion_sql();
    assert!(!promote.contains("source.client_id ="));
    assert_eq!(
        promote
            .matches("ORDER BY source.client_id, source.source_kind")
            .count(),
        2
    );
    assert_eq!(promote.matches("WITH seek AS MATERIALIZED").count(), 3);
    assert_eq!(promote.matches("source.bucket_start) >= (").count(), 2);
    assert_eq!(promote.matches("source.bucket_start) <= (").count(), 0);
    assert_eq!(promote.matches("seek.bucket_start <=").count(), 2);
    assert_eq!(promote.matches("ORDER BY destination.client_id").count(), 1);
    assert!(promote.contains("LIMIT $8"));
    assert!(promote.contains("WHERE running_rows <= $9"));
    assert!(promote.contains("LIMIT ($5 / tier.bucket_secs)"));
    assert!(promote.contains("LIMIT groups.maximum_rows"));
    assert!(promote.contains("seek.bucket_secs = $6"));
    assert!(promote.contains("FOR UPDATE OF source SKIP LOCKED"));
    assert!(promote.contains("ON CONFLICT DO NOTHING"));
    let prune = rollup_prune_sql();
    assert!(!prune.contains("source.client_id ="));
    assert_eq!(prune.matches("WITH seek AS MATERIALIZED").count(), 1);
    assert!(prune.contains("ORDER BY source.client_id, source.source_kind"));
    assert_eq!(prune.matches("source.bucket_start) >= (").count(), 1);
    assert_eq!(prune.matches("source.bucket_start) <= (").count(), 0);
    assert_eq!(prune.matches("seek.bucket_start <=").count(), 1);
    assert!(prune.contains("LIMIT $6"));
    assert!(prune.contains("LIMIT $5"));
    assert!(prune.contains("FOR UPDATE OF source SKIP LOCKED"));
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
        ) VALUES ('traffic_counter_samples', 32, 100, TRUE, FALSE, TRUE)
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
    register_rollup_streams(&db.pool).await;

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
        ) VALUES ('traffic_counter_samples', 32, 1, TRUE, FALSE, TRUE)
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
    register_rollup_streams(&db.pool).await;

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
async fn disabled_policy_still_promotes_losslessly_without_pruning_history() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ('traffic-disabled-policy', 'traffic-disabled-policy', decode('', 'hex'), 'online')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO history_retention_policies (
            domain, retention_days, prune_limit, enabled,
            metadata_only, export_enabled
        ) VALUES ('traffic_counter_samples', 32, 100, FALSE, FALSE, TRUE)
        ON CONFLICT (domain) DO UPDATE SET
            retention_days = excluded.retention_days,
            prune_limit = excluded.prune_limit,
            enabled = excluded.enabled
        "#,
    )
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
            ('traffic-disabled-policy', 'host', 'eth0', $1,
                100, 200, 0, 0, 'agent_networks'),
            ('traffic-disabled-policy', 'host', 'eth0', $1 + interval '1 minute',
                130, 250, 0, 0, 'agent_networks')
        "#,
    )
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
            'traffic-disabled-policy', 'host', 'eth0', 'live', 86400,
            date_bin('1 day', now() - interval '70 days', TIMESTAMPTZ '1970-01-01'),
            1, 2, 1, 1, 1, 0, 0, 0,
            now() - interval '70 days', now() - interval '70 days'
        )
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let run = process_traffic_retention(&db.pool).await.unwrap();
    assert_eq!(run.raw_rows_promoted, 1);
    assert_eq!(run.rollup_rows_pruned, 0);
    let exact_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM traffic_counter_samples WHERE client_id = 'traffic-disabled-policy'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(exact_rows, 1, "the newest predecessor remains available");
    let buckets: Vec<i32> = sqlx::query_scalar(
        "SELECT bucket_secs FROM traffic_counter_rollups WHERE client_id = 'traffic-disabled-policy' ORDER BY bucket_secs",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(buckets, vec![3_600, 86_400]);
    db.cleanup().await;
}

#[test]
fn candidate_scan_reaches_every_traffic_tier() {
    let query = traffic_candidate_streams_sql();
    assert!(!query.contains("sample.client_id ="));
    assert!(!query.contains("rollup.client_id ="));
    assert_eq!(query.matches("WITH seek AS MATERIALIZED").count(), 3);
    assert!(query.contains("ORDER BY sample.client_id, sample.source_kind"));
    assert_eq!(query.matches("sample.observed_at) >= (").count(), 1);
    assert_eq!(query.matches("sample.observed_at) < (").count(), 0);
    assert_eq!(query.matches("seek.observed_at <").count(), 1);
    assert_eq!(
        query
            .matches("ORDER BY rollup.client_id, rollup.source_kind")
            .count(),
        2
    );
    assert_eq!(query.matches("rollup.bucket_start) >= (").count(), 2);
    assert_eq!(query.matches("rollup.bucket_start) <= (").count(), 0);
    assert_eq!(query.matches("seek.bucket_start <=").count(), 2);
    assert!(query.contains("VALUES (3600, 10800, 91)"));
    assert!(query.contains("(10800, 21600, 181)"));
    assert!(query.contains("(21600, 86400, 366)"));
    assert!(query.contains("VALUES (3600), (10800), (21600), (86400)"));
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
    assert_eq!(first.conflicts + second.conflicts, 0);
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
    assert_eq!(first.conflicts, 0);
    assert_eq!(first.raw_rows_promoted, 59);
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
    assert_eq!(boundary, anchor + chrono::Duration::minutes(59));

    let second = process_traffic_retention(&db.pool).await.unwrap();
    assert_eq!(second.conflicts, 0);
    assert_eq!(second.raw_rows_promoted, 60);
    let third = process_traffic_retention(&db.pool).await.unwrap();
    assert_eq!(third.conflicts, 0);
    assert_eq!(third.raw_rows_promoted, 10);
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
    assert_eq!(retried, (1, 0, 0));
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
    assert_eq!(retained.0, 2);
    assert_eq!(retained.1, 1);
    assert_eq!(retained.2, anchor.timestamp());
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
async fn one_client_timeout_does_not_starve_later_traffic_candidates() {
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
    sqlx::query(
        r#"
        CREATE FUNCTION slow_one_traffic_client() RETURNS trigger
        LANGUAGE plpgsql AS $$
        BEGIN
            IF NEW.client_id = 'traffic-timeout-a' THEN
                PERFORM pg_sleep(3);
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
        CREATE TRIGGER slow_one_traffic_client_trigger
        BEFORE INSERT ON traffic_counter_rollups
        FOR EACH ROW EXECUTE FUNCTION slow_one_traffic_client()
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let run = process_traffic_retention(&db.pool).await.unwrap();
    assert_eq!(run.conflicts, 0);
    let fast_rollups: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM traffic_counter_rollups WHERE client_id = 'traffic-timeout-b'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(fast_rollups, 1);
    let slow_samples: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM traffic_counter_samples WHERE client_id = 'traffic-timeout-a'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(slow_samples, 2);
    db.cleanup().await;
}

#[tokio::test]
async fn locked_first_client_page_does_not_starve_the_next_eligible_client() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        r#"
        WITH generated AS (
            SELECT format(
                'traffic-fair-%s', lpad(client_number::text, 2, '0')
            ) AS client_id
            FROM generate_series(0, 16) client(client_number)
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
            FROM generate_series(0, 16) client(client_number)
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
        INSERT INTO traffic_counter_hourly_usage_streams (
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

    let blocked_ids = (0..CLIENT_BATCH)
        .map(|client_number| format!("traffic-fair-{client_number:02}"))
        .collect::<Vec<_>>();
    let mut blockers = db.pool.begin().await.unwrap();
    let locked = sqlx::query_scalar::<_, String>(
        "SELECT id FROM clients WHERE id = ANY($1) ORDER BY id FOR UPDATE",
    )
    .bind(&blocked_ids)
    .fetch_all(&mut *blockers)
    .await
    .unwrap();
    assert_eq!(locked, blocked_ids);

    set_candidate_stream_cursor(None);
    let first = tokio::time::timeout(Duration::from_secs(5), process_traffic_retention(&db.pool))
        .await
        .expect("the locked first registry page made retention wait")
        .unwrap();
    assert_eq!(first.raw_rows_promoted, 0);

    // The first sixteen clients remain locked. The next pass must nevertheless
    // inspect the following registry page instead of restarting at the oldest
    // blocked clients or having discarded client 17 when the cursor advanced.
    let second = tokio::time::timeout(Duration::from_secs(5), process_traffic_retention(&db.pool))
        .await
        .expect("the persistently locked first page starved the next client")
        .unwrap();
    assert_eq!(second.raw_rows_promoted, 1);
    let later_state: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM traffic_counter_samples
             WHERE client_id = 'traffic-fair-16'),
            (SELECT count(*) FROM traffic_counter_rollups
             WHERE client_id = 'traffic-fair-16')
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

    blockers.rollback().await.unwrap();
    set_candidate_stream_cursor(None);
    db.cleanup().await;
}

#[tokio::test]
async fn maximum_cost_raw_units_advance_past_a_persistently_conflicted_page() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "traffic-max-cost-fairness";
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1, $1, decode('', 'hex'), 'online')",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        WITH interfaces AS (
            SELECT format(
                'daily-%s', lpad(interface_number::text, 2, '0')
            ) AS interface
            FROM generate_series(0, 15) numbered(interface_number)
        ), anchor AS (
            SELECT date_bin(
                '1 day', now() - interval '400 days', TIMESTAMPTZ '1970-01-01'
            ) AS observed_at
        )
        INSERT INTO traffic_counter_samples (
            client_id, source_kind, interface, observed_at,
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source
        )
        SELECT
            $1, 'host', interfaces.interface,
            anchor.observed_at + sample.offset_number * interval '1 minute',
            100 + sample.offset_number * 10,
            200 + sample.offset_number * 20,
            0, 0, 'agent_networks'
        FROM interfaces
        CROSS JOIN anchor
        CROSS JOIN (VALUES (0), (1)) sample(offset_number)
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    // Keep the first full page persistently destination-conflicted. These are
    // daily units, so each consumes the raw query's worst-case 1,441-row cost.
    sqlx::query(
        r#"
        WITH interfaces AS (
            SELECT format(
                'daily-%s', lpad(interface_number::text, 2, '0')
            ) AS interface
            FROM generate_series(0, 12) numbered(interface_number)
        ), anchor AS (
            SELECT date_bin(
                '1 day', min(observed_at), TIMESTAMPTZ '1970-01-01'
            ) AS bucket_start
            FROM traffic_counter_samples
            WHERE client_id = $1
        )
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        )
        SELECT
            $1, 'host', interfaces.interface, 'live',
            86400, anchor.bucket_start, 10, 20,
            1, 1, 1, 0, 0, 0,
            anchor.bucket_start, anchor.bucket_start + interval '1 minute'
        FROM interfaces
        CROSS JOIN anchor
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_hourly_usage_streams (
            client_id, source_kind, interface
        )
        SELECT DISTINCT client_id, source_kind, interface
        FROM traffic_counter_samples
        WHERE client_id = $1
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();

    set_candidate_stream_cursor(None);
    let conflicted = process_traffic_retention(&db.pool).await.unwrap();
    assert_eq!(conflicted.raw_rows_promoted, 0);
    assert_eq!(conflicted.conflicts, 13);

    let later = process_traffic_retention(&db.pool).await.unwrap();
    assert_eq!(later.conflicts, 0);
    assert_eq!(later.raw_rows_promoted, 3);
    let later_state: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM traffic_counter_samples
             WHERE client_id = $1 AND interface >= 'daily-13'),
            (SELECT count(*) FROM traffic_counter_rollups
             WHERE client_id = $1 AND interface >= 'daily-13')
        "#,
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(later_state, (3, 3));

    // The next call wraps to the still-conflicted first page. This proves the
    // cursor neither skipped the three budget-excluded streams nor abandoned
    // the conflicted prefix after advancing beyond it.
    let wrapped = process_traffic_retention(&db.pool).await.unwrap();
    assert_eq!(wrapped.raw_rows_promoted, 0);
    assert_eq!(wrapped.conflicts, 13);
    let conflicted_samples: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM traffic_counter_samples WHERE client_id = $1 AND interface < 'daily-13'",
    )
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(conflicted_samples, 26);

    set_candidate_stream_cursor(None);
    db.cleanup().await;
}

#[tokio::test]
async fn candidate_stream_cursor_walks_every_bounded_page_and_wraps() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        r#"
        WITH generated AS (
            SELECT format(
                'traffic-cursor-%s', lpad(client_number::text, 2, '0')
            ) AS client_id
            FROM generate_series(0, 32) client(client_number)
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
        INSERT INTO traffic_counter_hourly_usage_streams (
            client_id, source_kind, interface
        )
        SELECT id, 'host', 'eth0'
        FROM clients
        WHERE id LIKE 'traffic-cursor-%'
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    set_candidate_stream_cursor(None);
    let first = load_candidate_stream_prefix(&db.pool)
        .await
        .unwrap()
        .into_iter()
        .map(|stream| stream.client_id)
        .collect::<Vec<_>>();
    let second = load_candidate_stream_prefix(&db.pool)
        .await
        .unwrap()
        .into_iter()
        .map(|stream| stream.client_id)
        .collect::<Vec<_>>();
    let third = load_candidate_stream_prefix(&db.pool)
        .await
        .unwrap()
        .into_iter()
        .map(|stream| stream.client_id)
        .collect::<Vec<_>>();
    let wrapped = load_candidate_stream_prefix(&db.pool)
        .await
        .unwrap()
        .into_iter()
        .map(|stream| stream.client_id)
        .collect::<Vec<_>>();

    assert_eq!(
        first,
        (0..13)
            .map(|client_number| format!("traffic-cursor-{client_number:02}"))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        second,
        (13..26)
            .map(|client_number| format!("traffic-cursor-{client_number:02}"))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        third,
        (26..33)
            .map(|client_number| format!("traffic-cursor-{client_number:02}"))
            .collect::<Vec<_>>()
    );
    assert_eq!(wrapped, first);

    set_candidate_stream_cursor(None);
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
    assert_eq!(retry.conflicts, 0);
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
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source
        )
        SELECT
            $1, 'host', 'eth0',
            date_trunc('minute', now() - interval '6 years')
                + sample_number * interval '1 minute',
            sample_number, sample_number * 2, 0, 0, 'agent_networks'
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
            rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch, sample_source
        ) VALUES
            ($1, 'host', 'eth0',
                date_bin('1 hour', now() - interval '45 days', TIMESTAMPTZ '1970-01-01'),
                10, 20, 0, 0, 'agent_networks'),
            ($1, 'host', 'eth0',
                date_bin('1 hour', now() - interval '45 days', TIMESTAMPTZ '1970-01-01')
                    + interval '1 minute',
                13, 25, 0, 0, 'agent_networks')
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
        INSERT INTO traffic_counter_hourly_usage_streams (
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
        INSERT INTO traffic_counter_hourly_usage_streams (
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
        "ANALYZE traffic_counter_samples, traffic_counter_rollups, traffic_counter_hourly_usage_streams",
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let registry_start_explain = format!(
        "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {}",
        candidate_stream_prefix_start_sql()
    );
    let registry_start_plan: Value = sqlx::query_scalar(&registry_start_explain)
        .bind(CANDIDATE_STREAM_SCAN_LIMIT)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_index_bounded_relation(
        &registry_start_plan,
        "traffic_counter_hourly_usage_streams",
        CANDIDATE_STREAM_SCAN_LIMIT as u64,
        false,
    );

    // Select a real key close to the end of the large same-client registry
    // prefix. The following page must start at that key even when PostgreSQL is
    // forced to reuse a generic prepared plan.
    let registry_cursor: (String, String, String) = sqlx::query_as(
        r#"
        SELECT client_id, source_kind, interface
        FROM traffic_counter_hourly_usage_streams
        WHERE client_id = $1
        ORDER BY client_id DESC, source_kind DESC, interface DESC
        OFFSET $2
        LIMIT 1
        "#,
    )
    .bind(backlog)
    .bind(CANDIDATE_STREAM_SCAN_LIMIT)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let registry_after_explain = format!(
        "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {}",
        candidate_stream_prefix_after_sql()
    );
    let mut registry_plan_tx = db.pool.begin().await.unwrap();
    for plan_mode in ["force_custom_plan", "force_generic_plan"] {
        let set_plan_mode = format!("SET LOCAL plan_cache_mode = '{plan_mode}'");
        sqlx::query(&set_plan_mode)
            .execute(&mut *registry_plan_tx)
            .await
            .unwrap();
        let registry_after_plan: Value = sqlx::query_scalar(&registry_after_explain)
            .bind(&registry_cursor.0)
            .bind(&registry_cursor.1)
            .bind(&registry_cursor.2)
            .bind(CANDIDATE_STREAM_SCAN_LIMIT)
            .fetch_one(&mut *registry_plan_tx)
            .await
            .unwrap();
        assert_index_bounded_relation(
            &registry_after_plan,
            "traffic_counter_hourly_usage_streams",
            CANDIDATE_STREAM_SCAN_LIMIT as u64,
            true,
        );
    }
    registry_plan_tx.rollback().await.unwrap();

    let candidate_explain = format!(
        "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {}",
        traffic_candidate_streams_sql()
    );
    let candidate_plan: Value = sqlx::query_scalar(&candidate_explain)
        .bind(vec![target])
        .bind(vec!["host"])
        .bind(vec!["eth0"])
        .bind(RAW_RETENTION_DAYS)
        .bind(CANDIDATE_RAW_PREFIX_LIMIT)
        .bind(3_650_i32)
        .bind(true)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_index_bounded_relation(&candidate_plan, "traffic_counter_samples", 32, true);
    assert_index_bounded_relation(&candidate_plan, "traffic_counter_rollups", 64, true);

    let raw_explain = format!(
        "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {}",
        raw_promotion_sql()
    );
    let raw_plan: Value = sqlx::query_scalar(&raw_explain)
        .bind(target)
        .bind(vec!["host"])
        .bind(vec!["eth0"])
        .bind(RAW_RETENTION_DAYS)
        .bind(GROUP_BATCH)
        .bind(PROMOTION_SOURCE_ROW_LIMIT)
        .bind(CANDIDATE_RAW_PREFIX_LIMIT)
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
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_index_bounded_relation(&rollup_plan, "traffic_counter_rollups", 512, true);

    let prune_explain = format!(
        "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {}",
        rollup_prune_sql()
    );
    let prune_plan: Value = sqlx::query_scalar(&prune_explain)
        .bind(target)
        .bind(vec!["host"])
        .bind(vec!["eth0"])
        .bind(3_650_i32)
        .bind(100_i64)
        .bind(13_i64)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_index_bounded_relation(&prune_plan, "traffic_counter_rollups", 128, true);
    db.cleanup().await;
}

async fn promote_raw_stream(pool: &sqlx::PgPool, client_id: &str) -> (i64, i64, i64) {
    let row = sqlx::query(raw_promotion_sql())
        .bind(client_id)
        .bind(vec!["host"])
        .bind(vec!["eth0"])
        .bind(RAW_RETENTION_DAYS)
        .bind(GROUP_BATCH)
        .bind(PROMOTION_SOURCE_ROW_LIMIT)
        .bind(CANDIDATE_RAW_PREFIX_LIMIT)
        .fetch_one(pool)
        .await
        .unwrap();
    (
        sqlx::Row::try_get(&row, "deleted_rows").unwrap(),
        sqlx::Row::try_get(&row, "conflicts").unwrap(),
        sqlx::Row::try_get(&row, "insert_race_conflicts").unwrap(),
    )
}

async fn register_rollup_streams(pool: &sqlx::PgPool) {
    sqlx::query(
        r#"
        INSERT INTO traffic_counter_hourly_usage_streams (
            client_id, source_kind, interface
        )
        SELECT DISTINCT client_id, source_kind, interface
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
                    WHEN rx_counter_epoch = previous_rx_epoch
                     AND rx_bytes >= previous_rx
                    THEN rx_bytes - previous_rx ELSE 0 END), 0)::bigint AS rx,
                coalesce(sum(CASE
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

    let expected_index = format!("{relation}_pkey");
    assert!(
        explain_uses_index(root, &expected_index),
        "{relation} did not use {expected_index}: {plan}"
    );
    if expect_index_condition {
        assert!(
            explain_uses_index_condition(root, &expected_index),
            "{relation} did not expose a bounded Index Cond on {expected_index}: {plan}"
        );
    }
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
        assert!(
            !explain_uses_index(root, forbidden_index),
            "{relation} used time-first index {forbidden_index}: {plan}"
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
