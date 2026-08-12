use super::*;
use std::{path::Path, str::FromStr};

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

#[tokio::test]
async fn postgres_reconciles_automatic_series_from_authoritative_tunnel_inventory() {
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
    let db_name = format!("vpsman_series_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {db_name}"))
        .execute(&admin_pool)
        .await
        .unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options.database(&db_name))
        .await
        .unwrap();
    sqlx::migrate::Migrator::new(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("migrations"),
    )
    .await
    .unwrap()
    .run(&pool)
    .await
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let suffix = Uuid::new_v4().simple().to_string();
    let left = format!("series-left-{suffix}");
    let right = format!("series-right-{suffix}");
    sqlx::query(
        "INSERT INTO clients (id, display_name, public_key, status) VALUES ($1,$1,decode('', 'hex'),'online'),($2,$2,decode('', 'hex'),'online')",
    )
    .bind(&left)
    .bind(&right)
    .execute(&mut *tx)
    .await
    .unwrap();
    let plan_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO tunnel_plans (
            id, name, kind, left_client_id, right_client_id, input, plan
        ) VALUES ($1,$2,'wireguard',$3,$4,'{}'::jsonb,'{}'::jsonb)
        "#,
    )
    .bind(plan_id)
    .bind(format!("series-plan-{suffix}"))
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
    .bind(format!("series-plan-{suffix}"))
    .bind(&left)
    .bind(&right)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        deactivate_postgres_automatic_observation_series_for_plan(
            &mut tx,
            plan_id,
            Some("identity"),
        )
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        deactivate_postgres_automatic_observation_series_for_plan(
            &mut tx,
            plan_id,
            Some("replacement-identity"),
        )
        .await
        .unwrap(),
        1
    );
    sqlx::query("UPDATE network_observation_series SET active = TRUE WHERE id = $1")
        .bind(series_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        r#"
        INSERT INTO telemetry_tunnels (
            client_id, observed_at, interface, kind, ownership_mode,
            mutation_policy, source, telemetry_plan_id, telemetry_endpoint_side,
            telemetry_peer_client_id, latency_monitoring_enabled,
            latency_primary_family, latency_target
        ) VALUES ($1,now(),'tun0','wireguard','builtin','managed','runtime',$2,
            'left',$3,TRUE,'ipv4','10.0.0.2')
        "#,
    )
    .bind(&left)
    .bind(plan_id.to_string())
    .bind(&right)
    .execute(&mut *tx)
    .await
    .unwrap();

    assert_eq!(
        reconcile_postgres_automatic_observation_series_for_client(&mut tx, &left)
            .await
            .unwrap(),
        0
    );
    sqlx::query(
        "UPDATE telemetry_tunnels SET latency_monitoring_enabled = FALSE WHERE client_id = $1",
    )
    .bind(&left)
    .execute(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        reconcile_postgres_automatic_observation_series_for_client(&mut tx, &left)
            .await
            .unwrap(),
        1
    );
    sqlx::query("UPDATE network_observation_series SET active = TRUE WHERE id = $1")
        .bind(series_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        r#"
        UPDATE telemetry_tunnels
        SET latency_monitoring_enabled = TRUE, latency_target = '10.0.0.3'
        WHERE client_id = $1
        "#,
    )
    .bind(&left)
    .execute(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        reconcile_postgres_automatic_observation_series_for_client(&mut tx, &left)
            .await
            .unwrap(),
        1
    );
    sqlx::query("UPDATE network_observation_series SET active = TRUE WHERE id = $1")
        .bind(series_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("DELETE FROM telemetry_tunnels WHERE client_id = $1")
        .bind(&left)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        reconcile_postgres_automatic_observation_series_for_client(&mut tx, &left)
            .await
            .unwrap(),
        1
    );
    let active: bool =
        sqlx::query_scalar("SELECT active FROM network_observation_series WHERE id = $1")
            .bind(series_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(!active);
    tx.rollback().await.unwrap();
    pool.close().await;
    sqlx::query(&format!("DROP DATABASE {db_name}"))
        .execute(&admin_pool)
        .await
        .unwrap();
    admin_pool.close().await;
}

#[tokio::test]
async fn postgres_trends_preserve_each_recent_automatic_and_manual_observation() {
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
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options.database(&db_name))
        .await
        .unwrap();
    sqlx::migrate::Migrator::new(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("migrations"),
    )
    .await
    .unwrap()
    .run(&pool)
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
    let end_unix = Utc::now().timestamp();
    let automatic_times = [end_unix - 240, end_unix - 180];
    let manual_times = [end_unix - 120, end_unix - 60];
    for (observed_at, latency, loss) in [
        (automatic_times[0], 10.0, 0.0),
        (automatic_times[1], 30.0, 1.0),
    ] {
        sqlx::query(
            r#"
            INSERT INTO network_observations (
                id, client_id, kind, source, role, plan_id,
                topology_identity_hash, plan_name, interface_name,
                peer_client_id, target, endpoint_side, address_family,
                stale_after_secs, healthy, transmitted, received,
                latency_min_ms, latency_avg_ms, latency_max_ms,
                packet_loss_ratio, observed_at, received_at
            ) VALUES (
                $1,$2,'tunnel_reachability','automatic','endpoint',$3,
                'identity',$4,'tun0',$5,'10.0.0.2','left','ipv4',
                180,$6,3,$7,$8,$8,$8,$9,to_timestamp($10),to_timestamp($10)
            )
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(&left)
        .bind(plan_id)
        .bind(&plan_name)
        .bind(&right)
        .bind(loss == 0.0)
        .bind(if loss == 0.0 { 3 } else { 0 })
        .bind(latency)
        .bind(loss)
        .bind(observed_at)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
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
    assert!(trends
        .iter()
        .all(|trend| !trend.retained && trend.bucket_secs.is_none()));
    assert!(trends.iter().all(|trend| trend.sample_count == 1));

    let mut automatic = trends
        .iter()
        .filter(|trend| trend.kind == "tunnel_reachability")
        .collect::<Vec<_>>();
    automatic.sort_by_key(|trend| postgres_timestamp_unix(trend.bucket_start.as_deref().unwrap()));
    assert_eq!(automatic.len(), 2);
    assert_eq!(automatic[0].latency_avg_ms, Some(10.0));
    assert_eq!(automatic[1].latency_avg_ms, Some(30.0));
    assert_eq!(automatic[0].packet_loss_avg_ratio, Some(0.0));
    assert_eq!(automatic[1].packet_loss_avg_ratio, Some(1.0));
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
    assert_eq!(manual[0].throughput_avg_mbps, Some(80.0));
    assert_eq!(manual[1].throughput_avg_mbps, Some(120.0));
    assert_eq!(
        manual
            .iter()
            .map(|trend| postgres_timestamp_unix(trend.bucket_start.as_deref().unwrap()))
            .collect::<Vec<_>>(),
        manual_times
    );

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
