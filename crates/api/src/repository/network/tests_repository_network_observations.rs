use super::*;
use std::{path::Path, str::FromStr};

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

#[test]
fn network_trends_budget_coordinates_before_reading_retained_values() {
    let source = include_str!("repository_network_observations.rs");
    let query = source
        .split_once("const NETWORK_OBSERVATION_TRENDS_QUERY")
        .unwrap()
        .1
        .split_once("const NETWORK_OBSERVATION_ROLLUPS_EXPORT_QUERY")
        .unwrap()
        .0;

    assert!(query.contains("series_catalog AS MATERIALIZED"));
    assert!(query.contains("budgeted_series AS MATERIALIZED"));
    assert!(query.contains("slot_numbers(slot) AS MATERIALIZED"));
    assert!(query.contains("point.physical_series_id = physical.series_id"));
    assert!(query.contains("ORDER BY point.bucket_start DESC"));
    assert!(query.contains("ORDER BY point.bucket_start, point.bucket_secs"));
    assert!(query.contains("LIMIT 1"));
    assert!(query.contains("selected_coordinates AS MATERIALIZED"));
    assert!(
        query.find("selected_coordinates AS MATERIALIZED").unwrap()
            < query.find("summarized AS").unwrap()
    );
    assert!(query.contains("sample.accepted_seq > head.materialized_seq"));
    assert!(query.contains("sample.accepted_seq <= projection.projected_seq"));
    assert!(query.contains(
        "rollup.bucket_start + make_interval(secs => rollup.bucket_secs) > to_timestamp($1)"
    ));
    assert!(query.contains("FROM manual_points point\n    GROUP BY"));
    assert!(!query.contains("series_points"));
    assert!(!query.contains("total_points"));
}

#[test]
fn automatic_reachability_uses_one_setwise_mutation_and_database_owned_wakes() {
    let ingest = include_str!("../fleet/repository_ingest.rs");
    let snapshot = ingest
        .split_once("async fn load_current_tunnel_plan_snapshot_for_ids_in_tx")
        .unwrap()
        .1
        .split_once("#[cfg(test)]\npub(crate) async fn lock_current_tunnel_plan_snapshot_for_test")
        .unwrap()
        .0;
    assert_eq!(snapshot.matches("sqlx::query(").count(), 1);
    assert!(snapshot.contains("WITH projection_client AS MATERIALIZED"));
    assert!(snapshot.contains("FOR KEY SHARE OF client"));
    assert!(snapshot.contains("ORDER BY plan.id\n        FOR SHARE OF plan"));
    assert!(snapshot.find("FOR KEY SHARE OF client") < snapshot.find("FOR SHARE OF plan"));

    let tunnel_writer = ingest
        .rsplit_once("async fn upsert_postgres_telemetry_tunnels")
        .unwrap()
        .1
        .split_once("fn persistent_disk_totals")
        .unwrap()
        .0;
    assert_eq!(
        tunnel_writer
            .matches("WITH incoming AS MATERIALIZED")
            .count(),
        1
    );
    assert!(tunnel_writer.contains(".fetch_one(&mut **tx)"));
    assert!(tunnel_writer.contains("reconciled_series AS"));
    assert!(tunnel_writer.contains("FROM incoming"));
    assert!(!tunnel_writer.contains("pg_notify"));
    assert!(!tunnel_writer.contains("telemetry_current_tunnels"));

    let projection = ingest
        .split_once("async fn project_claimed_telemetry_suffix_in_tx")
        .unwrap()
        .1
        .split_once("async fn load_network_interface_policy_in_tx")
        .unwrap()
        .0;
    assert!(!projection.contains("network_observation_series_deactivated"));

    let observations = include_str!("repository_network_observations.rs");
    let id_lock = observations
        .split_once("async fn lock_network_observation_ids_in_tx")
        .unwrap()
        .1
        .split_once("#[derive(Clone, Debug)]")
        .unwrap()
        .0;
    assert_eq!(id_lock.matches("sqlx::query_scalar::<_, i64>").count(), 1);
    assert!(id_lock.contains("SELECT DISTINCT observation_id"));
    assert!(id_lock.contains("ORDER BY observation_id"));
    assert!(id_lock.contains("pg_advisory_xact_lock(hashtextextended("));
    assert!(id_lock.contains("vpsman.network_observation.id:"));
    assert!(id_lock.contains("NETWORK_OBSERVATION_ID_LOCK_HASH_SEED"));
    let recorder = observations
        .split_once(
            "pub(crate) async fn record_postgres_automatic_tunnel_reachability_suffix_in_tx",
        )
        .unwrap()
        .1
        .split_once("const NETWORK_OBSERVATION_TRENDS_QUERY")
        .unwrap()
        .0;
    assert!(!recorder.contains("FROM tunnel_plans"));
    assert_eq!(recorder.matches("sqlx::query(").count(), 1);
    assert!(recorder.contains("payload_ordinal"));
    assert!(!recorder.contains("pg_notify"));
    assert!(
        recorder.find("lock_network_observation_ids_in_tx").unwrap()
            < recorder
                .find("AUTOMATIC_TUNNEL_REACHABILITY_BATCH_SQL")
                .unwrap()
    );
    let manual_batch = observations
        .split_once("async fn upsert_manual_observations")
        .unwrap()
        .1
        .split_once("/// Validates and projects a complete claimed suffix")
        .unwrap()
        .0;
    assert!(!manual_batch.contains("pg_notify"));
    assert!(
        manual_batch
            .find("lock_network_observation_ids_in_tx")
            .unwrap()
            < manual_batch
                .find("for observation in observations")
                .unwrap()
    );
    let batch = observations
        .split_once("const AUTOMATIC_TUNNEL_REACHABILITY_BATCH_SQL")
        .unwrap()
        .1
        .split_once("pub(crate) async fn deactivate_postgres_automatic_observation_series_for_plan")
        .unwrap()
        .0;
    assert!(batch.contains("automatic_sample_id"));
    assert!(batch.contains("ON CONFLICT DO NOTHING"));
    assert!(batch.contains("FROM inserted"));
    assert!(batch.contains("AS deactivated_series"));
    assert!(!batch.contains("network_observation_rollups"));

    let deactivation = observations
        .split_once("pub(crate) async fn deactivate_postgres_automatic_observation_series_for_plan")
        .unwrap()
        .1
        .split_once("/// Manual job evidence")
        .unwrap()
        .0;
    assert!(deactivation.contains("let changed = result.rows_affected()"));
    assert!(!deactivation.contains("pg_notify"));

    let schema = include_str!("../../../../../migrations/0004_network_tunnels.sql");
    for trigger in [
        "network_observations_retention_publish_insert",
        "network_observations_retention_publish_update",
        "network_observations_retention_delete",
        "network_observation_rollups_retention_delete",
        "network_observation_latest_retention_delete",
        "network_observation_series_retention_deactivate",
    ] {
        assert!(schema.contains(trigger), "missing {trigger}");
    }
    assert!(schema.contains("REFERENCING OLD TABLE AS old_telemetry_retention_rows"));
    assert!(schema.contains("NEW TABLE AS new_telemetry_retention_rows"));
    assert!(schema.contains("'network_observation_history_published'"));
    assert!(schema.contains("'network_observation_history_deleted'"));
    assert!(schema.contains("'network_observation_latest_deleted'"));
    assert!(schema.contains("'network_observation_series_deactivated'"));

    let core_schema = include_str!("../../../../../migrations/0003_telemetry_core.sql");
    let publisher = core_schema
        .split_once("CREATE FUNCTION public.publish_telemetry_retention_effect()")
        .unwrap()
        .1
        .split_once("CREATE FUNCTION public.enqueue_telemetry_history_due_events()")
        .unwrap()
        .0;
    let manual_filter = publisher
        .find("effect_name = 'network_observation_history_published'")
        .unwrap();
    let generic_insert = publisher.find("TG_OP = 'INSERT'").unwrap();
    assert!(manual_filter < generic_insert);
    assert!(publisher.contains("WHERE source = 'manual'"));

    let suffix_call = ingest
        .rfind("record_postgres_automatic_tunnel_reachability_suffix_in_tx")
        .unwrap();
    let sample_loop = ingest
        .find("for (sample, admission) in samples.iter()")
        .unwrap();
    assert!(suffix_call < sample_loop);
    assert_eq!(
        ingest
            .matches("record_postgres_automatic_tunnel_reachability_suffix_in_tx")
            .count(),
        2,
        "one import and one whole-suffix call are expected",
    );
}

#[tokio::test]
async fn postgres_trends_separate_retained_automatic_from_exact_manual_evidence() {
    fn postgres_timestamp_unix(value: &str) -> i64 {
        DateTime::parse_from_rfc3339(value)
            .or_else(|_| DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f%#z"))
            .unwrap()
            .timestamp()
    }

    let base_url = match std::env::var("VPSMAN_TEST_POSTGRES_URL") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => return,
    };
    let options = PgConnectOptions::from_str(&base_url).unwrap();
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone().database("postgres"))
        .await
        .unwrap();
    let db_name = format!("vpsman_trend_exact_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {db_name}"))
        .execute(&admin_pool)
        .await
        .unwrap();
    let database_options = options.database(&db_name);
    let migrations_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("migrations");
    crate::repository::migrate_postgres_database(&database_options, &migrations_dir)
        .await
        .unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(database_options.options([("search_path", "public")]))
        .await
        .unwrap();

    let mut tx = pool.begin().await.unwrap();
    let suffix = Uuid::new_v4().simple().to_string();
    let left = format!("trend-left-{suffix}");
    let right = format!("trend-right-{suffix}");
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1,$1,decode('', 'hex'),'online'),($2,$2,decode('', 'hex'),'online')",
    )
    .bind(&left)
    .bind(&right)
    .execute(&mut *tx)
    .await
    .unwrap();
    let plan_id = Uuid::new_v4();
    let plan_name = format!("trend-plan-{suffix}");
    sqlx::query(
        r#"
        INSERT INTO tunnel_plans (
            id, name, kind, left_client_id, right_client_id, input, plan
        ) VALUES ($1,$2,'wireguard',$3,$4,'{}'::jsonb,'{}'::jsonb)
        "#,
    )
    .bind(plan_id)
    .bind(&plan_name)
    .bind(&left)
    .bind(&right)
    .execute(&mut *tx)
    .await
    .unwrap();
    let series_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO network_observation_series (
            plan_id, topology_identity_hash, plan_name, interface_name,
            client_id, peer_client_id, endpoint_side, address_family, target
        ) VALUES ($1,'identity',$2,'tun0',$3,$4,'left','ipv4','10.0.0.2')
        RETURNING id
        "#,
    )
    .bind(plan_id)
    .bind(&plan_name)
    .bind(&left)
    .bind(&right)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let end_unix = (Utc::now().timestamp() / 60) * 60;
    let automatic_times = [end_unix - 240, end_unix - 180];
    let pending_automatic_time = end_unix - 170;
    let manual_times = [end_unix - 120, end_unix - 60];
    let automatic_ids = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
    let sample_id = Uuid::new_v4();
    let automatic_payload = automatic_ids[..2]
        .iter()
        .zip([
            (automatic_times[0], 10.0, 0.0),
            (automatic_times[1], 30.0, 1.0),
        ])
        .map(|(id, (measured_unix, latency, loss))| {
            serde_json::json!({
                "id": id,
                "measured_unix": measured_unix,
                "stale_after_secs": 180,
                "healthy": loss == 0.0,
                "transmitted": 3,
                "received": if loss == 0.0 { 3 } else { 0 },
                "latency_min_ms": latency,
                "latency_avg_ms": latency,
                "latency_max_ms": latency,
                "latency_mdev_ms": null,
                "packet_loss_ratio": loss,
                "reason": null,
            })
        })
        .collect::<Vec<_>>();
    sqlx::query(
        r#"
        INSERT INTO telemetry_samples (
            id, client_id, observed_at, cpu_cores,
            cpu_load_1, cpu_load_5, cpu_load_15,
            memory_total_bytes, memory_available_bytes,
            disk_total_bytes, disk_available_bytes,
            tcp_sockets, udp_sockets, payload,
            accepted_seq, accepted_at, source_gateway_id,
            source_gateway_session_id, source_process_incarnation_id,
            source_telemetry_seq, reported_observed_unix
        ) VALUES (
            $1, $2, to_timestamp($3), 0,
            0.0, 0.0, 0.0, 0, 0, 0, 0, 0, 0, $4,
            1, now(), 'trend-test', $5, $6, 1, $3
        )
        "#,
    )
    .bind(sample_id)
    .bind(&left)
    .bind(end_unix - 150)
    .bind(serde_json::json!({ "tunnel_reachability": automatic_payload }))
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .execute(&mut *tx)
    .await
    .unwrap();
    let pending_sample_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO telemetry_samples (
            id, client_id, observed_at, cpu_cores,
            cpu_load_1, cpu_load_5, cpu_load_15,
            memory_total_bytes, memory_available_bytes,
            disk_total_bytes, disk_available_bytes,
            tcp_sockets, udp_sockets, payload,
            accepted_seq, accepted_at, source_gateway_id,
            source_gateway_session_id, source_process_incarnation_id,
            source_telemetry_seq, reported_observed_unix
        ) VALUES (
            $1, $2, to_timestamp($3), 0,
            0.0, 0.0, 0.0, 0, 0, 0, 0, 0, 0, $4,
            2, now(), 'trend-test', $5, $6, 2, $3
        )
        "#,
    )
    .bind(pending_sample_id)
    .bind(&left)
    .bind(pending_automatic_time)
    .bind(serde_json::json!({
        "tunnel_reachability": [{
            "id": automatic_ids[2],
            "measured_unix": pending_automatic_time,
            "stale_after_secs": 180,
            "healthy": true,
            "transmitted": 3,
            "received": 3,
            "latency_min_ms": 50.0,
            "latency_avg_ms": 50.0,
            "latency_max_ms": 50.0,
            "latency_mdev_ms": null,
            "packet_loss_ratio": 0.0,
            "reason": null,
        }]
    }))
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE telemetry_projection_heads
        SET accepted_seq = 2, projected_seq = 2,
            accepted_at = now(), projected_at = now()
        WHERE client_id = $1
        "#,
    )
    .bind(&left)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE telemetry_minute_materialization_heads
        SET materialized_seq = 1, materialized_at = now(), updated_at = now()
        WHERE client_id = $1
        "#,
    )
    .bind(&left)
    .execute(&mut *tx)
    .await
    .unwrap();
    for (ordinal, (observation_id, (observed_at, latency, loss))) in automatic_ids
        .iter()
        .zip([
            (automatic_times[0], 10.0, 0.0),
            (automatic_times[1], 30.0, 1.0),
        ])
        .enumerate()
    {
        let ordinal = i16::try_from(ordinal + 1).unwrap();
        sqlx::query(
            r#"
            INSERT INTO network_observations (
                id, source, automatic_series_id, automatic_sample_id,
                automatic_payload_ordinal, plan_name, observed_at, received_at
            ) VALUES (
                $1, 'automatic', $2, $3, $4, $5,
                to_timestamp($6), to_timestamp($6)
            )
            "#,
        )
        .bind(*observation_id)
        .bind(series_id)
        .bind(sample_id)
        .bind(ordinal)
        .bind(&plan_name)
        .bind(observed_at)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO network_observation_rollups (
                series_id, bucket_secs, bucket_start, health_state,
                sample_count, transmitted_total, transmitted_sample_count,
                received_total, received_sample_count, latency_sum_ms,
                latency_sample_count, latency_min_ms, latency_max_ms,
                latency_mdev_sum_ms, latency_mdev_sample_count,
                packet_loss_sum_ratio, packet_loss_sample_count,
                packet_loss_min_ratio, packet_loss_max_ratio,
                latest_observation_id, latest_stale_after_secs, latest_healthy,
                latest_transmitted, latest_received, latest_latency_min_ms,
                latest_latency_avg_ms, latest_latency_max_ms,
                latest_latency_mdev_ms, latest_packet_loss_ratio, latest_reason,
                latest_observed_at, latest_received_at
            ) VALUES (
                $1, 60,
                date_bin(interval '1 minute', to_timestamp($2),
                         TIMESTAMPTZ '1970-01-01 00:00:00+00'),
                CASE $3::boolean WHEN TRUE THEN 1 ELSE 0 END,
                1, 3, 1, $4, 1, $5, 1, $5, $5, 0.0, 0,
                $6, 1, $6, $6, $7, 180, $3, 3, $4,
                $5, $5, $5, NULL, $6, NULL, to_timestamp($2), to_timestamp($2)
            )
            "#,
        )
        .bind(series_id)
        .bind(observed_at)
        .bind(loss == 0.0)
        .bind(if loss == 0.0 { 3 } else { 0 })
        .bind(latency)
        .bind(loss)
        .bind(*observation_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    sqlx::query(
        r#"
        INSERT INTO network_observations (
            id, source, automatic_series_id, automatic_sample_id,
            automatic_payload_ordinal, plan_name, observed_at, received_at
        ) VALUES (
            $1, 'automatic', $2, $3, 1, $4,
            to_timestamp($5), to_timestamp($5)
        )
        "#,
    )
    .bind(automatic_ids[2])
    .bind(series_id)
    .bind(pending_sample_id)
    .bind(&plan_name)
    .bind(pending_automatic_time)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE network_observation_series
        SET plan_name = $1
        WHERE id = $2
        "#,
    )
    .bind(format!("{plan_name}-renamed"))
    .bind(series_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO network_observation_latest (
            series_id, observation_id, stale_after_secs, healthy,
            transmitted, received, latency_min_ms, latency_avg_ms,
            latency_max_ms, packet_loss_ratio, observed_at, received_at
        )
        VALUES (
            $1, $2, 180, TRUE, 3, 3, 50.0, 50.0, 50.0, 0.0,
            to_timestamp($3), to_timestamp($3)
        )
        "#,
    )
    .bind(series_id)
    .bind(automatic_ids[2])
    .bind(pending_automatic_time)
    .execute(&mut *tx)
    .await
    .unwrap();
    for (observed_at, throughput) in [(manual_times[0], 80.0), (manual_times[1], 120.0)] {
        sqlx::query(
            r#"
            INSERT INTO network_observations (
                id, client_id, kind, source, role, plan_id,
                topology_identity_hash, plan_name, interface_name,
                peer_client_id, target, healthy, throughput_mbps, bytes,
                observed_at, received_at
            ) VALUES (
                $1,$2,'network_speed_test','manual','client',$3,
                'identity',$4,'tun0',$5,'10.0.0.2:5201',TRUE,$6,1048576,
                to_timestamp($7),to_timestamp($7)
            )
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&left)
        .bind(plan_id)
        .bind(&plan_name)
        .bind(&right)
        .bind(throughput)
        .bind(observed_at)
        .execute(&mut *tx)
        .await
        .unwrap();
    }

    let rows = sqlx::query(NETWORK_OBSERVATION_TRENDS_QUERY)
        .bind(automatic_times[0] - 1)
        .bind(end_unix)
        .bind(vec![plan_id])
        .bind(None::<&str>)
        .bind(None::<&str>)
        .bind(None::<&str>)
        .bind(None::<&str>)
        .bind(None::<&str>)
        .bind(false)
        .bind(100_i64)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    let trends = rows
        .into_iter()
        .map(network_observation_trend_from_row)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(trends.len(), 4);
    assert_eq!(
        trends.iter().map(|trend| trend.sample_count).sum::<i64>(),
        5
    );
    assert_eq!(
        trends
            .iter()
            .map(|trend| trend.automatic_count)
            .sum::<i64>(),
        3
    );
    assert_eq!(
        trends.iter().map(|trend| trend.manual_count).sum::<i64>(),
        2
    );

    let mut automatic = trends
        .iter()
        .filter(|trend| trend.kind == "tunnel_reachability")
        .collect::<Vec<_>>();
    automatic.sort_by_key(|trend| postgres_timestamp_unix(trend.bucket_start.as_deref().unwrap()));
    assert_eq!(automatic.len(), 2);
    assert!(automatic
        .iter()
        .all(|trend| trend.retained && trend.bucket_secs == Some(60)));
    assert_eq!(automatic[0].sample_count, 1);
    assert_eq!(automatic[1].sample_count, 2);
    assert_eq!(automatic[0].latency_avg_ms, Some(10.0));
    assert_eq!(automatic[1].latency_avg_ms, Some(40.0));
    assert_eq!(automatic[0].packet_loss_avg_ratio, Some(0.0));
    assert_eq!(automatic[1].packet_loss_avg_ratio, Some(0.5));
    let renamed_plan = format!("{plan_name}-renamed");
    assert!(automatic
        .iter()
        .all(|trend| trend.plan_name.as_deref() == Some(renamed_plan.as_str())));
    assert_eq!(
        automatic
            .iter()
            .map(|trend| postgres_timestamp_unix(trend.bucket_start.as_deref().unwrap()))
            .collect::<Vec<_>>(),
        automatic_times
    );

    let mut manual = trends
        .iter()
        .filter(|trend| trend.kind == "network_speed_test")
        .collect::<Vec<_>>();
    manual.sort_by_key(|trend| postgres_timestamp_unix(trend.bucket_start.as_deref().unwrap()));
    assert_eq!(manual.len(), 2);
    assert!(manual
        .iter()
        .all(|trend| !trend.retained && trend.bucket_secs.is_none()));
    assert_eq!(manual[0].throughput_avg_mbps, Some(80.0));
    assert_eq!(manual[1].throughput_avg_mbps, Some(120.0));
    assert_eq!(
        manual
            .iter()
            .map(|trend| postgres_timestamp_unix(trend.bucket_start.as_deref().unwrap()))
            .collect::<Vec<_>>(),
        manual_times
    );

    let exact_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM network_observation_exact_evidence WHERE plan_id = $1",
    )
    .bind(plan_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        exact_count, 5,
        "latest must not duplicate its exact raw copy"
    );
    let automatic_exact_names: Vec<String> = sqlx::query_scalar(
        r#"
        SELECT plan_name
        FROM network_observation_exact_evidence
        WHERE plan_id = $1 AND source = 'automatic'
        ORDER BY observed_at, id
        "#,
    )
    .bind(plan_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(automatic_exact_names, vec![plan_name.clone(); 3]);

    for (source, expected_kind, expected_retained, expected_samples) in [
        ("automatic", "tunnel_reachability", true, 3),
        ("manual", "network_speed_test", false, 2),
    ] {
        let filtered = sqlx::query(NETWORK_OBSERVATION_TRENDS_QUERY)
            .bind(automatic_times[0] - 1)
            .bind(end_unix)
            .bind(vec![plan_id])
            .bind(None::<&str>)
            .bind(Some(source))
            .bind(None::<&str>)
            .bind(None::<&str>)
            .bind(None::<&str>)
            .bind(false)
            .bind(100_i64)
            .fetch_all(&mut *tx)
            .await
            .unwrap()
            .into_iter()
            .map(network_observation_trend_from_row)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(filtered.len(), 2);
        assert!(filtered
            .iter()
            .all(|trend| trend.kind == expected_kind && trend.retained == expected_retained));
        assert_eq!(
            filtered.iter().map(|trend| trend.sample_count).sum::<i64>(),
            expected_samples
        );
    }

    sqlx::query("SAVEPOINT reject_ownerless_automatic")
        .execute(&mut *tx)
        .await
        .unwrap();
    let invalid = sqlx::query(
        r#"
        INSERT INTO network_observations (
            id, client_id, kind, source, plan_id, topology_identity_hash,
            plan_name, interface_name, peer_client_id, healthy
        ) VALUES ($1,$2,'tunnel_reachability','automatic',$3,
                  'identity',$4,'tun0',$5,TRUE)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(&left)
    .bind(plan_id)
    .bind(&plan_name)
    .bind(&right)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(invalid
        .to_string()
        .contains("network_observations_automatic_series_check"));
    sqlx::query("ROLLBACK TO SAVEPOINT reject_ownerless_automatic")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("RELEASE SAVEPOINT reject_ownerless_automatic")
        .execute(&mut *tx)
        .await
        .unwrap();

    tx.rollback().await.unwrap();
    pool.close().await;
    sqlx::query(&format!("DROP DATABASE {db_name}"))
        .execute(&admin_pool)
        .await
        .unwrap();
    admin_pool.close().await;
}

#[test]
fn parses_probe_and_speed_status_observations() {
    let job_id = Uuid::new_v4();
    let probe = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "tunnel_reachability",
            "plan": "edge",
            "interface": "tun0",
            "peer_client_id": "right",
            "target": "10.0.0.1",
            "parsed": {
                "healthy": true,
                "latency_avg_ms": 12.5,
                "packet_loss_ratio": 0.01
            }
        }))
        .unwrap(),
        exit_code: Some(0),
        done: true,
    };
    let speed = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "network_speed_test",
            "role": "client",
            "plan": "edge",
            "interface": "tun0",
            "peer_client_id": "left",
            "server_address": "10.0.0.0",
            "port": 5201,
            "success": true,
            "bytes": 1048576,
            "throughput_mbps": 33.3
        }))
        .unwrap(),
        exit_code: Some(0),
        done: true,
    };

    let parsed_probe = parse_network_observation(job_id, "left", 0, &probe, "1").unwrap();
    let parsed_speed = parse_network_observation(job_id, "right", 1, &speed, "1").unwrap();

    assert_eq!(parsed_probe.kind, "tunnel_reachability");
    assert_eq!(parsed_probe.latency_avg_ms, Some(12.5));
    assert_eq!(parsed_probe.packet_loss_ratio, Some(0.01));
    assert_eq!(parsed_probe.healthy, Some(true));
    assert_eq!(parsed_speed.kind, "network_speed_test");
    assert_eq!(parsed_speed.role.as_deref(), Some("client"));
    assert_eq!(parsed_speed.target.as_deref(), Some("10.0.0.0:5201"));
    assert_eq!(parsed_speed.bytes, Some(1_048_576));
    assert_eq!(parsed_speed.throughput_mbps, Some(33.3));
}

#[test]
fn legacy_network_probe_status_is_not_a_reachability_observation() {
    let job_id = Uuid::new_v4();
    let output = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "network_probe",
            "plan": "edge",
            "interface": "tun0",
            "peer_client_id": "right",
            "target": "10.0.0.1",
            "parsed": {
                "healthy": true,
                "latency_avg_ms": 12.5,
                "packet_loss_ratio": 0.0
            }
        }))
        .unwrap(),
        exit_code: Some(0),
        done: true,
    };

    assert!(parse_network_observation(job_id, "left", 0, &output, "1").is_none());
}

#[test]
fn parses_network_status_runtime_summary_and_adapter_evidence() {
    let job_id = Uuid::new_v4();
    let status = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "network_status",
            "plan": "external-edge",
            "interface": "ovpn42",
            "peer_client_id": "right",
            "runtime": {
                "summary": {
                    "manager": "custom_adapter",
                    "status": "adapter_unhealthy",
                    "healthy": false,
                    "drift": false,
                    "reasons": ["adapter_status_failed"]
                },
                "adapter": {
                    "configured": true,
                    "success": false,
                    "exit_code": 7
                }
            }
        }))
        .unwrap(),
        exit_code: Some(0),
        done: true,
    };

    let parsed = parse_network_observation(job_id, "left", 3, &status, "1").unwrap();

    assert_eq!(parsed.kind, "network_status");
    assert_eq!(parsed.plan_name.as_deref(), Some("external-edge"));
    assert_eq!(parsed.interface_name.as_deref(), Some("ovpn42"));
    assert_eq!(parsed.healthy, Some(false));
    assert_eq!(
        parsed.metadata["runtime"]["summary"]["status"],
        "adapter_unhealthy"
    );
}

#[test]
fn fair_limit_keeps_every_plan_visible_when_one_plan_is_noisy() {
    fn observation(plan_id: Uuid, endpoint: &str, observed_at: i64) -> NetworkObservationView {
        NetworkObservationView {
            id: Uuid::new_v4(),
            job_id: None,
            client_id: format!("{plan_id}-{endpoint}"),
            seq: None,
            kind: "tunnel_reachability".to_string(),
            source: "automatic".to_string(),
            role: Some("endpoint".to_string()),
            plan_id: Some(plan_id),
            topology_identity_hash: Some(plan_id.simple().to_string()),
            plan_name: Some(format!("plan-{plan_id}")),
            interface_name: Some("tun0".to_string()),
            peer_client_id: Some("peer".to_string()),
            target: Some("10.0.0.2".to_string()),
            endpoint_side: Some(endpoint.to_string()),
            address_family: Some("ipv4".to_string()),
            stale_after_secs: Some(180),
            healthy: Some(true),
            transmitted: Some(3),
            received: Some(3),
            latency_min_ms: Some(1.0),
            latency_avg_ms: Some(2.0),
            latency_max_ms: Some(3.0),
            latency_mdev_ms: Some(0.1),
            packet_loss_ratio: Some(0.0),
            reason: None,
            throughput_mbps: None,
            bytes: None,
            metadata: serde_json::json!({}),
            observed_at: observed_at.to_string(),
            received_at: observed_at.to_string(),
        }
    }

    let plan_ids = (0..25).map(|_| Uuid::new_v4()).collect::<Vec<_>>();
    let mut rows = Vec::new();
    for plan_id in &plan_ids {
        rows.push(observation(*plan_id, "left", 1_000));
        rows.push(observation(*plan_id, "right", 999));
    }
    for offset in 0..100 {
        rows.push(observation(plan_ids[0], "left", 2_000 + offset));
    }
    rows.sort_by(compare_network_observations_desc);

    let limited = limit_observations_fairly(rows, 2, 10_000);
    let represented = limited
        .iter()
        .filter_map(|row| row.plan_id)
        .collect::<std::collections::HashSet<_>>();

    assert_eq!(represented.len(), plan_ids.len());
    assert!(plan_ids.iter().all(|plan_id| represented.contains(plan_id)));
    assert!(
        limited
            .iter()
            .filter(|row| row.plan_id == Some(plan_ids[0])
                && row.endpoint_side.as_deref() == Some("left"))
            .count()
            <= 2
    );
}
