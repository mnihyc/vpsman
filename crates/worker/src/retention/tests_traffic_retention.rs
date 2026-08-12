use super::{
    process_traffic_retention, raw_promotion_sql, rollup_promotion_sql, rollup_prune_sql,
    traffic_candidate_clients_sql, TIERS,
};
use crate::test_support::PgWorkerTestDb;

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
fn raw_promotion_is_locked_conflict_safe_and_keeps_a_predecessor() {
    let query = raw_promotion_sql();
    assert!(query.contains("FOR UPDATE OF source SKIP LOCKED"));
    assert!(query.contains("ON CONFLICT DO NOTHING"));
    assert!(query.contains("newer.observed_at > source.observed_at"));
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
    assert!(promote.contains("LIMIT $5"));
    assert!(promote.contains("FOR UPDATE OF source SKIP LOCKED"));
    assert!(promote.contains("ON CONFLICT DO NOTHING"));
    let prune = rollup_prune_sql();
    assert!(prune.contains("make_interval(secs => bucket_secs)"));
    assert!(prune.contains("LIMIT $3"));
    assert!(prune.contains("FOR UPDATE SKIP LOCKED"));
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
    let query = traffic_candidate_clients_sql();
    for bucket_secs in [3600, 10800, 21600, 86400] {
        assert!(query.contains(&format!("rollup.bucket_secs = {bucket_secs}")));
    }
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
        .bind(vec![3_600_i32, 10_800, 21_600])
        .bind(86_400_i32)
        .bind(366_i32)
        .bind(128_i64)
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
        ), source(offset_secs, rx_bytes, tx_bytes) AS (
            VALUES (0, 3, 5), (18000, 7, 11)
        )
        INSERT INTO traffic_counter_rollups (
            client_id, source_kind, interface, origin_kind,
            bucket_secs, bucket_start, rx_bytes, tx_bytes,
            rx_valid_count, tx_valid_count, any_valid_count,
            rx_reset_count, tx_reset_count, any_reset_count,
            first_observed_at, latest_observed_at
        )
        SELECT
            'traffic-sparse-tier', 'host', 'eth0', 'live', 3600,
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
            .bind(vec![3_600_i32, 10_800])
            .bind(21_600_i32)
            .bind(181_i32)
            .bind(128_i64)
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
        .bind(vec![3_600_i32, 10_800])
        .bind(21_600_i32)
        .bind(181_i32)
        .bind(128_i64)
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
        .bind(vec![3_600_i32])
        .bind(21_600_i32)
        .bind(181_i32)
        .bind(128_i64)
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
        .bind(vec![3_600_i32, 10_800, 21_600])
        .bind(86_400_i32)
        .bind(366_i32)
        .bind(128_i64)
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

    let run = process_traffic_retention(&db.pool).await.unwrap();
    assert_eq!(run.conflicts, 0);
    assert!(run.raw_rows_promoted >= 4);
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
async fn raw_promotion_preserves_the_predecessor_across_group_batches() {
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
            $1 + sample_number * interval '1 hour',
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
    assert_eq!(first.raw_rows_promoted, 127);
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
    assert_eq!(boundary, anchor + chrono::Duration::hours(127));

    let second = process_traffic_retention(&db.pool).await.unwrap();
    assert_eq!(second.conflicts, 0);
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
    assert_eq!(retained, (130, 1_290, 2_580, 129));
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
